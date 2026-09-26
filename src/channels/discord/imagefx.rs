//! `/image` - the free half of what NotSoBot does, done locally.
//!
//! One command, a list of effects, and a picture found in whichever of five
//! places it is: an attachment on the command, a member (their avatar), a URL,
//! a custom emoji, or - and this is the one people actually use - nothing at
//! all, in which case the last picture posted in the channel is picked up.
//! The remembering is `imagefx_recent`; the pixels are `imagefx_engine`; the
//! risky business of opening a URL a member typed is `imagefx_fetch`.
//!
//! **Where it works.** One channel, in a setting, defaulting to the owner's.
//! Run anywhere else it answers quietly, to the caller alone, with a pointer -
//! and nothing is posted where it was asked for. That is deliberate: an effect
//! command loose in a server is a way to put any picture in any channel.
//!
//! **How a refusal sounds.** A rate limit that answers in words is a rate limit
//! that fills the channel with its own noise, so a member who is over their
//! allowance and asked again within the second gets an emoji and nothing else
//! (`Verdict::Quiet`). A slash command has no message to react to, so the emoji
//! arrives as an ephemeral reply with nothing in it but the emoji - quiet in the
//! only sense that matters, which is that the channel never sees it. The first
//! refusal in a window does get words, once, also only to the caller.
//!
//! **Who may be the subject.** Anybody on the `/noroast` list cannot have their
//! avatar put through an effect. It is the same list, on purpose: somebody who
//! said they did not want to be the bot's material meant all of it.

use std::time::Duration;

use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions, CreateAttachment,
    CreateAutocompleteResponse, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, Message, UserId,
};

use super::control;
use super::imagefx_engine::{self as engine, Effect, Job, Limits, Out, Trouble};
use super::imagefx_fetch as fetch;
use super::imagefx_recent::{self as recent, Rules, Verdict};
use super::imagefx_text as words;

/// The channel the owner asked for, with nothing saved.
pub const DEFAULT_CHANNEL: u64 = 1_516_534_303_968_858_312;

/// Discord will take a 10MiB upload on a server with no boosts; 8 leaves room
/// for the rest of the message and for the day they change their minds.
const UPLOAD_CAP: usize = 8 * 1024 * 1024;

const MB: usize = 1024 * 1024;

// --- settings ----------------------------------------------------------------------------------

pub fn on() -> bool {
    control::on("VIZIER_IMAGE", true)
}

/// The one channel `/image` works in.
pub fn channel() -> u64 {
    control::id("VIZIER_IMAGE_CHANNEL").unwrap_or(DEFAULT_CHANNEL)
}

/// Everything the engine is allowed to spend.
pub fn limits() -> Limits {
    Limits {
        max_side: control::number("VIZIER_IMAGE_MAX_SIDE", 800).clamp(64, 2048) as u32,
        anim_side: control::number("VIZIER_IMAGE_ANIM_SIDE", 256).clamp(32, 768) as u32,
        max_frames: control::number("VIZIER_IMAGE_MAX_FRAMES", 60).clamp(1, 300) as usize,
        budget: Duration::from_secs(control::number("VIZIER_IMAGE_SECONDS", 20).clamp(1, 120)),
        upload_cap: UPLOAD_CAP,
    }
}

/// The most a fetched picture may weigh.
pub fn fetch_cap() -> usize {
    control::number("VIZIER_IMAGE_FETCH_MB", 8).clamp(1, 25) as usize * MB
}

/// How often anyone may ask.
pub fn rules() -> Rules {
    Rules {
        per_member: control::number("VIZIER_IMAGE_PER_MEMBER", 4).clamp(1, 60) as u32,
        per_channel: control::number("VIZIER_IMAGE_PER_CHANNEL", 12).clamp(1, 240) as u32,
        window: Duration::from_secs(control::number("VIZIER_IMAGE_WINDOW_SECS", 60).clamp(5, 3600)),
    }
}

/// How long one fetch may take. Kept in step with the engine's budget rather
/// than given a setting of its own: two numbers to tune is one too many.
fn fetch_wait() -> Duration {
    Duration::from_secs(control::number("VIZIER_IMAGE_SECONDS", 20).clamp(1, 120)).min(Duration::from_secs(10))
}

// --- where the picture comes from ---------------------------------------------------------------

/// The five places a picture may come from, in the order they are tried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file put on the command itself.
    Attachment(String),
    /// A member's avatar - the id comes with it, because the opt-out list is
    /// checked against it.
    Member(u64, String),
    /// An address somebody typed.
    Url(String),
    /// A custom emoji from this or another server.
    Emoji(String),
    /// The last picture posted in the channel.
    Recent(String),
}

impl Source {
    pub fn url(&self) -> &str {
        match self {
            Source::Attachment(u) | Source::Url(u) | Source::Emoji(u) | Source::Recent(u) => u,
            Source::Member(_, u) => u,
        }
    }

    /// Whose avatar this is, when it is an avatar.
    pub fn about(&self) -> Option<u64> {
        match self {
            Source::Member(id, _) => Some(*id),
            _ => None,
        }
    }

    /// Whether the address came from a member rather than from Discord. Only
    /// these need the full SSRF treatment; Discord's own CDN is checked too,
    /// because one code path is easier to be sure of than two.
    pub fn typed_in(&self) -> bool {
        matches!(self, Source::Url(_))
    }
}

/// The resolution order, all in one place so it can be tested without Discord:
/// an attachment beats a member, a member beats a URL, a URL beats an emoji, and
/// the last picture in the channel is what happens when nobody said anything.
pub fn pick(
    attachment: Option<String>,
    member: Option<(u64, String)>,
    url: Option<String>,
    emoji: Option<String>,
    recent: Option<String>,
) -> Option<Source> {
    if let Some(u) = attachment.filter(|u| !u.is_empty()) {
        return Some(Source::Attachment(u));
    }
    if let Some((id, u)) = member.filter(|(_, u)| !u.is_empty()) {
        return Some(Source::Member(id, u));
    }
    if let Some(u) = url.filter(|u| !u.trim().is_empty()) {
        return Some(Source::Url(u.trim().to_string()));
    }
    if let Some(u) = emoji.filter(|u| !u.is_empty()) {
        return Some(Source::Emoji(u));
    }
    recent.filter(|u| !u.is_empty()).map(Source::Recent)
}

/// A custom emoji's picture. Takes what Discord puts in a message
/// (`<:name:123>`, `<a:name:123>` for an animated one) and a bare id, since
/// people paste those too. A unicode emoji has no picture to fetch and comes
/// back as `None`.
pub fn emoji_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let (animated, id) = if let Some(inner) = raw.strip_prefix("<a:").and_then(|s| s.strip_suffix('>')) {
        (true, inner.rsplit(':').next()?.to_string())
    } else if let Some(inner) = raw.strip_prefix("<:").and_then(|s| s.strip_suffix('>')) {
        (false, inner.rsplit(':').next()?.to_string())
    } else if raw.chars().all(|c| c.is_ascii_digit()) && (15..=25).contains(&raw.len()) {
        (false, raw.to_string())
    } else {
        return None;
    };
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) || id.len() > 25 {
        return None;
    }
    let ext = if animated { "gif" } else { "png" };
    Some(format!("https://cdn.discordapp.com/emojis/{id}.{ext}?size=256"))
}

/// A member's picture, at a size worth working on rather than the 1024 Discord
/// hands out by default.
fn avatar_url(raw: &str) -> String {
    if raw.contains("size=") { raw.replace("size=1024", "size=512") } else { format!("{raw}?size=512") }
}

// --- the picker --------------------------------------------------------------------------------

/// What the autocomplete offers for what has been typed. Discord takes
/// twenty-five, so the list is filtered rather than paged: typing "mir" gets
/// the four mirrors, typing "gif" gets the animated ones, typing a group name
/// gets the group.
pub fn suggest(typed: &str) -> Vec<(String, &'static str)> {
    let needle = typed.trim().to_ascii_lowercase();
    engine::ALL
        .iter()
        .filter(|e| {
            needle.is_empty()
                || e.key().contains(&needle)
                || e.label().to_ascii_lowercase().contains(&needle)
                || e.group().to_ascii_lowercase().contains(&needle)
        })
        .take(25)
        .map(|e| {
            let shown = format!("{} · {}", e.label(), e.group());
            (shown.chars().take(100).collect::<String>(), e.key())
        })
        .collect()
}

pub fn builder() -> CreateCommand {
    CreateCommand::new("image")
        .description("run an effect on a picture - magik, deepfry, caption and the rest")
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "effect", "which effect - start typing to search")
                .required(true)
                .set_autocomplete(true),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "text", "for caption and meme - meme splits on a |")
                .max_length(words::MAX_TEXT as u16),
        )
        .add_option(
            CreateCommandOption::new(CommandOptionType::Integer, "strength", "how much, 1 to 10 (rotate: x45 degrees)")
                .min_int_value(1)
                .max_int_value(10),
        )
        .add_option(CreateCommandOption::new(CommandOptionType::Attachment, "picture", "a file to use"))
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "use their profile picture"))
        .add_option(CreateCommandOption::new(CommandOptionType::String, "url", "a link to a picture").max_length(500))
        .add_option(CreateCommandOption::new(CommandOptionType::String, "emoji", "a custom emoji").max_length(80))
}

/// Answers the picker as it is typed in.
pub async fn autocomplete(ctx: &Context, command: &CommandInteraction) {
    let typed = command.data.autocomplete().map(|o| o.value.to_string()).unwrap_or_default();
    let mut response = CreateAutocompleteResponse::new();
    for (shown, key) in suggest(&typed) {
        response = response.add_string_choice(shown, key);
    }
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Autocomplete(response)).await {
        tracing::debug!("image: the picker did not answer: {}", err);
    }
}

// --- remembering what was posted ----------------------------------------------------------------

/// Every picture in a message, best first. Attachments before embeds, because an
/// attachment is what somebody meant to post.
pub fn pictures_in(msg: &Message) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in &msg.attachments {
        if recent::looks_like_an_image(&a.filename, a.content_type.as_deref()) {
            out.push(a.url.clone());
        }
    }
    for e in &msg.embeds {
        for url in [e.image.as_ref().map(|i| i.url.clone()), e.thumbnail.as_ref().map(|t| t.url.clone())].into_iter().flatten() {
            if recent::looks_like_an_image(&url, None) {
                out.push(url);
            }
        }
    }
    out
}

/// Anything posted in the image channel leaves its picture behind, so `/image`
/// with no arguments has something to work on. Bots included: that is how one
/// effect gets run on the output of another.
pub fn on_message(msg: &Message) {
    if !on() {
        return;
    }
    let home = channel();
    if msg.channel_id.get() != home {
        return;
    }
    // Noted back to front, so the first picture in the message ends up newest.
    for url in pictures_in(msg).into_iter().rev() {
        recent::note_image(home, url);
    }
}

// --- the command --------------------------------------------------------------------------------

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
}

fn option<'a>(command: &'a CommandInteraction, name: &str) -> Option<&'a CommandDataOptionValue> {
    command.data.options.iter().find(|o| o.name == name).map(|o| &o.value)
}

fn string_option(command: &CommandInteraction, name: &str) -> Option<String> {
    match option(command, name) {
        Some(CommandDataOptionValue::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// What to say when somebody has not given a picture and there is none to find.
const NOTHING_TO_WORK_ON: &str = "I can't find a picture. Attach one, name a member, paste a link or a custom emoji - \
     or post a picture in here first and I'll pick it up.";

/// Whether `/image` should answer with a pointer instead of a picture. The whole
/// gate, in one line, so there is something to point a test at.
pub fn wrong_channel(here: u64, home: u64) -> bool {
    here != home
}

/// Whoever this effect would be aimed at, if they have asked to be left out.
/// `None` means go ahead - either it is not a person, or they never opted out.
pub fn blocked_by_optout(source: &Source, optouts: &[u64]) -> Option<u64> {
    let about = source.about()?;
    optouts.contains(&about).then_some(about)
}

/// What to say about a rate-limit verdict, and whether to carry on. `None` is
/// "carry on"; the string is shown to the caller alone and never in the channel.
/// A quiet refusal is the emoji by itself - no sentence, no explanation, nothing
/// anybody else can see.
pub fn answer_for(verdict: &Verdict) -> Option<String> {
    match verdict {
        Verdict::Go => None,
        Verdict::Quiet(emoji) => Some((*emoji).to_string()),
        Verdict::Tell(said) => Some(said.clone()),
    }
}

/// The kind refusal for somebody who has opted out.
fn opted_out_words(name: &str) -> String {
    format!("{name} has asked to be left out of this sort of thing, so I'll leave their picture alone. Anything else is fair game.")
}

pub async fn command(ctx: &Context, command: &CommandInteraction) {
    if !on() {
        let _ = command.create_response(&ctx.http, whisper("The image toolkit is switched off just now.")).await;
        return;
    }
    let home = channel();
    let here = command.channel_id.get();
    if wrong_channel(here, home) {
        // Nothing is posted, anywhere. Just a pointer, to the caller only.
        let _ = command.create_response(&ctx.http, whisper(format!("`/image` only works in <#{home}> - try it in there."))).await;
        return;
    }

    let who = command.user.id.get();
    if let Some(said) = answer_for(&recent::ask(here, who, &rules())) {
        // Either the emoji by itself or one sentence, and in both cases only
        // the caller sees it.
        let _ = command.create_response(&ctx.http, whisper(said)).await;
        return;
    }

    let Some(effect) = string_option(command, "effect").as_deref().and_then(Effect::from_key) else {
        let _ = command
            .create_response(&ctx.http, whisper("I don't know that effect - pick one from the list as you type."))
            .await;
        return;
    };

    // The words go through the ping stripper before they are drawn and before
    // they are stored, not only before they are posted.
    let text = string_option(command, "text").map(|t| words::depinged(&t)).filter(|t| !t.trim().is_empty());
    if effect.wants_text() && text.is_none() {
        let _ = command.create_response(&ctx.http, whisper(Trouble::NeedsText.plainly())).await;
        return;
    }
    let strength = match option(command, "strength") {
        Some(CommandDataOptionValue::Integer(n)) => Some((*n).clamp(1, 10) as u8),
        _ => None,
    };

    let attachment = match option(command, "picture") {
        Some(CommandDataOptionValue::Attachment(id)) => command.data.resolved.attachments.get(id).and_then(|a| {
            if recent::looks_like_an_image(&a.filename, a.content_type.as_deref()) {
                Some(a.url.clone())
            } else {
                None
            }
        }),
        _ => None,
    };
    let member = match option(command, "member") {
        Some(CommandDataOptionValue::User(id)) => {
            let url = command.data.resolved.users.get(id).map(|u| avatar_url(&u.face()));
            url.map(|u| (id.get(), u))
        }
        _ => None,
    };
    let typed_emoji = string_option(command, "emoji");
    let emoji = typed_emoji.as_deref().and_then(emoji_url);
    if emoji.is_none() && typed_emoji.is_some_and(|e| !e.trim().is_empty()) {
        let _ = command
            .create_response(&ctx.http, whisper("That has to be a *custom* emoji from a server - a plain one has no picture to fetch."))
            .await;
        return;
    }

    let Some(source) = pick(attachment, member, string_option(command, "url"), emoji, recent::last_image(home)) else {
        let _ = command.create_response(&ctx.http, whisper(NOTHING_TO_WORK_ON)).await;
        return;
    };

    // Somebody who said no to being the bot's material said no to this too.
    {
        if let Some(about) = blocked_by_optout(&source, &super::roast::optouts()) {
            let name = command
                .data
                .resolved
                .members
                .get(&UserId::new(about))
                .and_then(|m| m.nick.clone())
                .or_else(|| command.data.resolved.users.get(&UserId::new(about)).map(|u| u.display_name().to_string()))
                .unwrap_or_else(|| "They".to_string());
            let _ = command.create_response(&ctx.http, whisper(opted_out_words(&name))).await;
            return;
        }
    }

    if command.defer(&ctx.http).await.is_err() {
        return;
    }

    let bytes = match fetch::get(source.url(), fetch_cap(), fetch_wait()).await {
        Ok(b) => b,
        Err(refusal) => {
            if !source.typed_in() {
                tracing::warn!("image: could not fetch {} from Discord: {:?}", source.url(), refusal);
            }
            let said = match &source {
                Source::Recent(_) => format!("{} That was the last picture posted in here - try attaching one.", refusal.plainly()),
                _ => refusal.plainly(),
            };
            let _ = command.create_followup(&ctx.http, follow(said).ephemeral(true)).await;
            return;
        }
    };

    let job = Job { effect, text, strength };
    let limits = limits();
    let started = std::time::Instant::now();
    let done = tokio::task::spawn_blocking(move || engine::run(&bytes, &job, &limits)).await;
    let out = match done {
        Ok(Ok(out)) => out,
        Ok(Err(trouble)) => {
            if let Trouble::Broke(detail) = &trouble {
                tracing::warn!("image: {} failed: {}", effect.key(), detail);
            }
            let _ = command.create_followup(&ctx.http, follow(trouble.plainly()).ephemeral(true)).await;
            return;
        }
        Err(err) => {
            tracing::error!("image: the worker for {} died: {}", effect.key(), err);
            let _ = command.create_followup(&ctx.http, follow(Trouble::Broke(String::new()).plainly()).ephemeral(true)).await;
            return;
        }
    };
    tracing::info!("image: {} took {}ms, {} bytes", effect.key(), started.elapsed().as_millis(), out.weight());

    let message = match out {
        Out::Text(block) => follow(block),
        Out::Png(bytes) | Out::Gif(bytes) => {
            let name = match effect {
                _ if bytes.starts_with(b"GIF") => format!("{}.gif", effect.key()),
                _ => format!("{}.png", effect.key()),
            };
            follow(String::new()).add_file(CreateAttachment::bytes(bytes, name))
        }
    };
    if let Err(err) = command.create_followup(&ctx.http, message).await {
        tracing::warn!("image: {} could not be posted: {}", effect.key(), err);
    }
}

/// A followup that can never ping anybody, whatever is in it.
fn follow(text: impl Into<String>) -> CreateInteractionResponseFollowup {
    CreateInteractionResponseFollowup::new().content(text).allowed_mentions(CreateAllowedMentions::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "https://cdn.discordapp.com/attachments/1/2/a.png";
    const AV: &str = "https://cdn.discordapp.com/avatars/7/abc.webp?size=512";
    const U: &str = "https://example.com/b.jpg";
    const E: &str = "https://cdn.discordapp.com/emojis/9.png?size=256";
    const R: &str = "https://cdn.discordapp.com/attachments/1/3/last.gif";

    /// The order the brief asked for, tried one drop at a time.
    #[test]
    fn the_picture_comes_from_the_first_place_it_is_found() {
        let all = || {
            (
                Some(A.to_string()),
                Some((7u64, AV.to_string())),
                Some(U.to_string()),
                Some(E.to_string()),
                Some(R.to_string()),
            )
        };
        let (a, m, u, e, r) = all();
        assert_eq!(pick(a, m, u, e, r), Some(Source::Attachment(A.into())), "an attachment wins");
        let (_, m, u, e, r) = all();
        assert_eq!(pick(None, m, u, e, r), Some(Source::Member(7, AV.into())), "then a member");
        let (_, _, u, e, r) = all();
        assert_eq!(pick(None, None, u, e, r), Some(Source::Url(U.into())), "then a url");
        let (_, _, _, e, r) = all();
        assert_eq!(pick(None, None, None, e, r), Some(Source::Emoji(E.into())), "then an emoji");
        let (_, _, _, _, r) = all();
        assert_eq!(pick(None, None, None, None, r), Some(Source::Recent(R.into())), "and last, the channel");
        assert_eq!(pick(None, None, None, None, None), None, "nothing anywhere");
    }

    #[test]
    fn blank_arguments_are_skipped_rather_than_used() {
        assert_eq!(
            pick(Some(String::new()), None, Some("   ".into()), Some(String::new()), Some(R.into())),
            Some(Source::Recent(R.into()))
        );
        assert_eq!(pick(None, Some((7, String::new())), None, None, None), None);
        assert_eq!(pick(None, None, Some("  http://x/y.png ".into()), None, None), Some(Source::Url("http://x/y.png".into())));
    }

    #[test]
    fn only_a_members_avatar_is_about_somebody() {
        assert_eq!(Source::Member(7, AV.into()).about(), Some(7));
        assert_eq!(Source::Attachment(A.into()).about(), None);
        assert_eq!(Source::Recent(R.into()).about(), None);
        assert!(Source::Url(U.into()).typed_in());
        assert!(!Source::Attachment(A.into()).typed_in());
    }

    #[test]
    fn a_custom_emoji_becomes_a_picture_and_a_plain_one_does_not() {
        assert_eq!(emoji_url("<:blob:123456789012345678>").as_deref(), Some("https://cdn.discordapp.com/emojis/123456789012345678.png?size=256"));
        assert_eq!(emoji_url("<a:spin:123456789012345678>").as_deref(), Some("https://cdn.discordapp.com/emojis/123456789012345678.gif?size=256"));
        assert_eq!(emoji_url("123456789012345678").as_deref(), Some("https://cdn.discordapp.com/emojis/123456789012345678.png?size=256"));
        for bad in ["🙂", "", "  ", "<:broken", "not an emoji", "<:name:notanumber>", "12", "<@123456789012345678>"] {
            assert_eq!(emoji_url(bad), None, "{:?} was taken for a custom emoji", bad);
        }
    }

    #[test]
    fn an_avatar_is_asked_for_at_a_workable_size() {
        assert!(avatar_url("https://cdn.discordapp.com/avatars/7/a.webp?size=1024").ends_with("size=512"));
        assert!(avatar_url("https://cdn.discordapp.com/embed/avatars/3.png").ends_with("?size=512"));
    }

    /// The picker must never hand Discord more than it takes, and must find the
    /// things people will type.
    #[test]
    fn the_picker_finds_things_and_stays_inside_discords_limits() {
        let empty = suggest("");
        assert!(!empty.is_empty() && empty.len() <= 25, "{} suggestions with nothing typed", empty.len());
        for (shown, _) in &empty {
            assert!(shown.chars().count() <= 100, "{:?} is too long for Discord", shown);
        }
        assert_eq!(suggest("")[0].1, "magik", "the signature effect goes first");

        let mirrors = suggest("mirror");
        assert_eq!(mirrors.len(), 4, "all four mirrors: {:?}", mirrors);

        let melt = suggest("melt");
        assert!(melt.iter().any(|(_, k)| *k == "magik"), "a group name should find its effects: {:?}", melt);

        assert!(suggest("fry").iter().any(|(_, k)| *k == "deepfry"));
        assert!(suggest("GIF").iter().any(|(_, k)| *k == "magikgif"));
        assert!(suggest("zzz").is_empty(), "nonsense should find nothing");
        // Everything offered is a real effect.
        for (_, key) in suggest("") {
            assert!(Effect::from_key(key).is_some(), "{} is not an effect", key);
        }
    }

    #[test]
    fn the_command_is_shaped_the_way_discord_needs() {
        let built = serde_json::to_value(builder()).expect("serialisable");
        assert_eq!(built["name"], "image");
        let options = built["options"].as_array().expect("options");
        assert_eq!(options.len(), 7, "effect, text, strength, and the four sources");
        assert_eq!(options[0]["name"], "effect");
        assert_eq!(options[0]["required"], true);
        assert_eq!(options[0]["autocomplete"], true);
        let names: Vec<&str> = options.iter().map(|o| o["name"].as_str().unwrap_or("")).collect();
        for want in ["effect", "text", "strength", "picture", "member", "url", "emoji"] {
            assert!(names.contains(&want), "{} is missing from /image", want);
        }
    }

    /// The channel gate: the default is the owner's channel, and anywhere else
    /// is a pointer rather than a picture.
    #[test]
    fn the_image_channel_defaults_to_the_owners_channel() {
        assert_eq!(DEFAULT_CHANNEL, 1_516_534_303_968_858_312);
        assert_eq!(channel(), DEFAULT_CHANNEL, "nothing saved: the owner's channel");
    }

    #[test]
    fn the_settings_have_sane_defaults_and_cannot_be_set_to_nonsense() {
        let l = limits();
        assert_eq!(l.max_side, 800, "NotSoBot's 800x800 rule");
        assert_eq!(l.anim_side, 256);
        assert_eq!(l.max_frames, 60);
        assert_eq!(l.budget, Duration::from_secs(20));
        assert_eq!(l.upload_cap, UPLOAD_CAP);
        assert_eq!(fetch_cap(), 8 * MB);
        let r = rules();
        assert_eq!((r.per_member, r.per_channel, r.window), (4, 12, Duration::from_secs(60)));
        assert!(on(), "on unless somebody turns it off");
        assert!(fetch_wait() <= Duration::from_secs(10));
    }

    /// The gate: one channel, and everywhere else is a pointer. The picture is
    /// never posted where it was asked for.
    #[test]
    fn the_command_works_in_one_channel_and_points_everywhere_else() {
        assert!(!wrong_channel(DEFAULT_CHANNEL, DEFAULT_CHANNEL));
        assert!(wrong_channel(999, DEFAULT_CHANNEL), "another channel is refused");
        assert!(wrong_channel(0, DEFAULT_CHANNEL), "a DM is refused");
        assert!(!wrong_channel(4242, 4242), "whatever the setting says");
    }

    /// Somebody on the /noroast list cannot be the subject - and nothing else
    /// they might have posted is affected.
    #[test]
    fn a_member_who_opted_out_cannot_be_the_target() {
        let out = [11u64, 22];
        assert_eq!(blocked_by_optout(&Source::Member(22, AV.into()), &out), Some(22));
        assert_eq!(blocked_by_optout(&Source::Member(11, AV.into()), &out), Some(11));
        assert_eq!(blocked_by_optout(&Source::Member(33, AV.into()), &out), None, "anyone else is fine");
        // Only an avatar is about a person. A picture they attached is not.
        assert_eq!(blocked_by_optout(&Source::Attachment(A.into()), &out), None);
        assert_eq!(blocked_by_optout(&Source::Url(U.into()), &out), None);
        assert_eq!(blocked_by_optout(&Source::Recent(R.into()), &out), None);
        assert_eq!(blocked_by_optout(&Source::Emoji(E.into()), &out), None);
        // And with nobody opted out, nothing is blocked.
        assert_eq!(blocked_by_optout(&Source::Member(22, AV.into()), &[]), None);
    }

    /// The quiet path really is quiet: the emoji on its own, no sentence
    /// wrapped round it, nothing added.
    #[test]
    fn a_rapid_refusal_is_the_emoji_and_nothing_else() {
        assert_eq!(answer_for(&Verdict::Go), None, "allowed means no reply at all");
        let quiet = answer_for(&Verdict::Quiet(recent::SLOW_DOWN)).expect("something");
        assert_eq!(quiet, recent::SLOW_DOWN);
        assert_eq!(quiet.chars().filter(|c| c.is_alphanumeric()).count(), 0, "no words: {:?}", quiet);
        let told = answer_for(&Verdict::Tell("slow down a moment".into())).expect("something");
        assert_eq!(told, "slow down a moment");
    }

    #[test]
    fn the_refusals_are_kind_and_say_what_to_do() {
        assert!(NOTHING_TO_WORK_ON.contains("Attach") && NOTHING_TO_WORK_ON.contains("post a picture"));
        let said = opted_out_words("Riya");
        assert!(said.starts_with("Riya") && said.contains("leave their picture alone"));
        assert!(!said.contains("refuse") && !said.contains("not allowed"));
    }

    /// Whatever goes into the message beside the picture, nothing in it can
    /// ping - the caption text is stripped before it is drawn *and* before it
    /// is stored on the job.
    #[test]
    fn caption_text_is_stripped_before_it_ever_reaches_the_engine() {
        let raw = "@everyone <@1234567890> look";
        let cleaned = words::depinged(raw);
        assert!(!cleaned.contains('@') && !cleaned.contains("<@"));
        let job = Job { effect: Effect::Caption, text: Some(cleaned.clone()), strength: None };
        assert_eq!(job.text.as_deref(), Some(cleaned.as_str()));
    }
}
