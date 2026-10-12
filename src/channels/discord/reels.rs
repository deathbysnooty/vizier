//! Reels, quietly.
//!
//! A reel link pasted in chat becomes a player two hundred pixels tall, and a
//! channel where people share a lot of them is a wall of video that everybody
//! scrolls past. The server ran a third-party bot that made this worse: it
//! silenced the poster's own preview and posted a FULL-SIZE player of its own.
//!
//! This does the opposite. The preview on the poster's message is silenced -
//! the same trick, which needs Manage Messages - and in its place goes one
//! line with a button. Anyone who wants to watch presses it and gets the reel
//! **privately**, in a reply only they can see. The channel stays readable and
//! watching becomes a choice.
//!
//! HOW THE VIDEO IS GOT. Instagram serves nothing a bot can read, so the
//! well-known mirror domains do the work: asking `kkinstagram.com` for a reel
//! AS DISCORD ASKS redirects straight to an `.mp4`. Measured on a real reel
//! from this server: one redirect, `video/mp4`, 2.4 MB. Not every reel is a
//! video - a "reel" link can be a photo post, which comes back as a JPEG, and
//! that is posted as a picture instead.
//!
//! WHAT HAPPENS WHEN IT DOESN'T WORK. A reel too big for Discord, a mirror
//! having a bad day, a link that is not what it looked like: every one of them
//! ends with the member being handed the link, never an error. The worst case
//! is what they would have had anyway.

use std::time::Duration;

use serenity::all::{
    ButtonStyle, ComponentInteraction, Context, CreateActionRow, CreateAttachment, CreateButton,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage, Message,
};

/// Channels this runs in, `VIZIER_REELS_CHANNELS`. Empty means nowhere: it is
/// off until somebody names a channel, so it can be tried in one room first.
fn channels() -> Vec<u64> {
    super::control::ids("VIZIER_REELS_CHANNELS")
}

/// Whether to do anything at all, `VIZIER_REELS`.
fn on() -> bool {
    super::control::on("VIZIER_REELS", false)
}

/// How long to wait on the mirror before giving up and handing over the link.
fn fetch_timeout() -> Duration {
    Duration::from_secs(super::control::number("VIZIER_REELS_FETCH_SECS", 20).clamp(3, 60))
}

/// The largest file to put in front of somebody, in megabytes. Discord refuses
/// more than 10 MB from a bot on an unboosted server; a reel is usually 2-4.
fn size_cap() -> usize {
    super::control::number("VIZIER_REELS_MAX_MB", 9).clamp(1, 25) as usize * 1024 * 1024
}

/// A link worth handling, and where to ask for its video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reel {
    /// The link as the member wrote it.
    pub link: String,
    /// Which site it is, for the line in chat.
    pub site: &'static str,
    /// The mirror to ask for the file, or `None` where there is no mirror and
    /// the button can only hand over the link.
    pub mirror: Option<String>,
}

/// Finds the first reel link in a message.
///
/// The mirror domains are included on purpose: people paste those directly,
/// having learnt that plain Instagram links show nothing, and a `kkinstagram`
/// link embeds just as loudly as the bot's player did.
pub fn find(text: &str) -> Option<Reel> {
    let word = text.split_whitespace().find(|w| {
        let w = w.trim_start_matches('<').trim_end_matches('>');
        w.starts_with("http") && site_of(w).is_some()
    })?;
    let link = word.trim_start_matches('<').trim_end_matches('>').to_string();
    let site = site_of(&link)?;
    Some(Reel { mirror: mirror_for(&link), site, link })
}

/// Which site a link belongs to, or `None` for one this leaves alone.
fn site_of(link: &str) -> Option<&'static str> {
    let low = link.to_ascii_lowercase();
    let host = low.split("//").nth(1)?.split('/').next()?.trim_start_matches("www.");
    match host {
        "instagram.com" | "instagr.am" | "kkinstagram.com" | "ddinstagram.com" | "instagramez.com"
        | "instagramfix.com" => Some("Instagram"),
        "tiktok.com" | "vm.tiktok.com" | "vt.tiktok.com" | "vxtiktok.com" | "tnktok.com" => Some("TikTok"),
        _ => None,
    }
}

/// The address that answers with the file. Instagram's own pages give a bot
/// nothing, so every Instagram link is asked of one mirror, whichever mirror
/// the member happened to paste.
fn mirror_for(link: &str) -> Option<String> {
    let low = link.to_ascii_lowercase();
    let host = low.split("//").nth(1)?.split('/').next()?.to_string();
    let rest = link.splitn(4, '/').nth(3).unwrap_or("");
    match site_of(link)? {
        "Instagram" => Some(format!("https://www.kkinstagram.com/{}", rest)),
        "TikTok" => Some(format!("https://www.vxtiktok.com/{}", rest)),
        _ => {
            let _ = host;
            None
        }
    }
}

/// What came back from a mirror.
pub enum Got {
    /// A file small enough to hand over, and what to call it.
    File(Vec<u8>, &'static str),
    /// Nothing usable: the link is all there is.
    Only,
}

/// Asks the mirror for the file, the way Discord asks.
///
/// The user agent matters: these mirrors serve a human a web page and a crawler
/// the media itself, so asking as a browser gets HTML and asking as Discord
/// gets the video.
async fn fetch(url: &str, cap: usize, wait: Duration) -> Got {
    let client = match reqwest::Client::builder().timeout(wait).build() {
        Ok(client) => client,
        Err(err) => {
            tracing::warn!("reels: no http client: {err}");
            return Got::Only;
        }
    };
    let answer = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0 (compatible; Discordbot/2.0; +https://discordapp.com)")
        .send()
        .await;
    let answer = match answer {
        Ok(answer) if answer.status().is_success() => answer,
        Ok(answer) => {
            tracing::info!("reels: {} said {}", url, answer.status());
            return Got::Only;
        }
        Err(err) => {
            tracing::info!("reels: {} did not answer: {}", url, err);
            return Got::Only;
        }
    };
    let kind = answer.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let name = match kind.split(';').next().unwrap_or("").trim() {
        "video/mp4" => "reel.mp4",
        "image/jpeg" => "reel.jpg",
        "image/png" => "reel.png",
        "image/gif" => "reel.gif",
        // HTML means the mirror showed us a page rather than the media.
        _ => return Got::Only,
    };
    // Refuse an oversized one before downloading it, where the mirror says so.
    if answer.content_length().is_some_and(|n| n as usize > cap) {
        tracing::info!("reels: {} is bigger than the cap", url);
        return Got::Only;
    }
    match answer.bytes().await {
        Ok(body) if body.len() <= cap && !body.is_empty() => Got::File(body.to_vec(), name),
        Ok(body) => {
            tracing::info!("reels: {} came to {} bytes, over the cap", url, body.len());
            Got::Only
        }
        Err(err) => {
            tracing::info!("reels: {} stopped mid-download: {}", url, err);
            Got::Only
        }
    }
}

/// The line that replaces the player: who shared it, from where, and the way in.
fn card(who: &str, reel: &Reel) -> (String, Vec<CreateActionRow>) {
    let text = format!("🎬 **{}** shared a reel · *{}*", who, reel.site);
    let row = CreateActionRow::Buttons(vec![
        CreateButton::new("reel:watch").label("Watch").style(ButtonStyle::Primary).emoji('▶'),
        CreateButton::new_link(reel.link.clone()).label("Open"),
    ]);
    (text, vec![row])
}

/// A message in a watched channel: silence its preview and put the line up.
pub async fn on_message(ctx: &Context, msg: &Message) {
    if !on() || msg.author.bot {
        return;
    }
    let here = msg.channel_id.get();
    if !channels().contains(&here) {
        return;
    }
    let Some(reel) = find(&msg.content) else { return };
    // Silence the poster's own preview. Only the flags of somebody else's
    // message may be edited, and only with Manage Messages - which is exactly
    // what this needs and all it needs.
    let hushed = msg.channel_id.edit_message(&ctx.http, msg.id, EditMessage::new().suppress_embeds(true)).await;
    if let Err(err) = hushed {
        // Worth saying: without the permission the line still goes up, but the
        // player stays, and somebody will want to know why.
        tracing::warn!("reels: could not silence the preview in {}: {} - is Manage Messages missing?", here, err);
    }
    let who = msg.author.global_name.clone().unwrap_or_else(|| msg.author.name.clone());
    let (text, rows) = card(&who, &reel);
    let sent = msg
        .channel_id
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(text)
                .components(rows)
                .reference_message(msg)
                .allowed_mentions(serenity::all::CreateAllowedMentions::new()),
        )
        .await;
    if let Err(err) = sent {
        tracing::warn!("reels: the line did not go up in {}: {}", here, err);
    }
}

/// Whether a press belongs here.
pub fn owns_component(id: &str) -> bool {
    id == "reel:watch"
}

/// Someone pressed Watch: hand them the reel, privately.
///
/// Answered first and filled in after, because fetching a few megabytes takes
/// longer than the three seconds Discord allows for a first answer.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let link = component
        .message
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|one| match one {
            serenity::all::ActionRowComponent::Button(button) => match &button.data {
                serenity::all::ButtonKind::Link { url } => Some(url.clone()),
                _ => None,
            },
            _ => None,
        });
    let Some(link) = link else {
        let _ = component
            .create_response(
                ctx,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .content("That reel has gone - the link isn't on this message any more.")
                        .ephemeral(true),
                ),
            )
            .await;
        return;
    };
    let reel = find(&link).unwrap_or(Reel { link: link.clone(), site: "a reel", mirror: mirror_for(&link) });
    if component.create_response(ctx, CreateInteractionResponse::Defer(
        CreateInteractionResponseMessage::new().ephemeral(true),
    )).await.is_err()
    {
        return;
    }
    let got = match &reel.mirror {
        Some(mirror) => fetch(mirror, size_cap(), fetch_timeout()).await,
        None => Got::Only,
    };
    let follow = match got {
        Got::File(bytes, name) => serenity::all::CreateInteractionResponseFollowup::new()
            .ephemeral(true)
            .add_file(CreateAttachment::bytes(bytes, name)),
        Got::Only => serenity::all::CreateInteractionResponseFollowup::new().ephemeral(true).content(format!(
            "▶ {}\n-# This one wouldn't play here - too big, or the site wouldn't hand it over. The link opens it.",
            reel.link
        )),
    };
    if let Err(err) = component.create_followup(&ctx.http, follow).await {
        tracing::warn!("reels: could not hand over the reel: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reel_link_is_found_whatever_it_is_wrapped_in() {
        for (text, site) in [
            ("https://www.instagram.com/reel/DeQbry8C7Dm/", "Instagram"),
            ("look at this https://www.kkinstagram.com/reel/DeT7dIYMi8A/ <@&123>", "Instagram"),
            ("<https://vm.tiktok.com/ZSabc123/>", "TikTok"),
            ("Moto 2 crash - https://www.instagram.com/reel/DeO5QpzRIfb", "Instagram"),
        ] {
            let found = find(text).unwrap_or_else(|| panic!("nothing found in {text}"));
            assert_eq!(found.site, site, "{text}");
            assert!(found.link.starts_with("http") && !found.link.ends_with('>'), "{}", found.link);
        }
    }

    #[test]
    fn everything_else_is_left_alone() {
        for text in [
            "just talking",
            "https://youtube.com/watch?v=abc",
            "https://example.com/reel/nope",
            "https://klipy.com/gifs/baby-cry-crying-1",
        ] {
            assert!(find(text).is_none(), "should have been left alone: {text}");
        }
    }

    #[test]
    fn every_instagram_mirror_is_asked_of_one_place() {
        // Whichever domain was pasted, the file is asked for in the same way -
        // the path is what matters, and the member's choice of mirror does not.
        let one = find("https://www.instagram.com/reel/ABC123/").unwrap();
        let two = find("https://www.ddinstagram.com/reel/ABC123/").unwrap();
        assert_eq!(one.mirror, two.mirror);
        assert_eq!(one.mirror.as_deref(), Some("https://www.kkinstagram.com/reel/ABC123/"));
    }

    #[test]
    fn the_line_names_the_sharer_and_keeps_the_link_reachable() {
        let reel = find("https://www.instagram.com/reel/ABC123/").unwrap();
        let (text, rows) = card("Zoya", &reel);
        assert!(text.contains("Zoya") && text.contains("Instagram"), "{text}");
        // The Open button carries the link, which is also how a press finds it
        // again after a restart - nothing is remembered anywhere else.
        assert_eq!(rows.len(), 1);
    }
}
