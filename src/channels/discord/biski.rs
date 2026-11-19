//! Biski: right-click a message → Apps → Biski, and Loduchand posts a
//! screenshot of it (and the message it replied to) in the biski channel.
//!
//! Anyone may use it. The channel is meant to be read-only for members, so
//! the bot's posts are the only thing in it. A message is posted once; asking
//! again answers with the link to the first post. Nothing is taken from
//! #safe-corner.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::DateTime;
use regex::Regex;
use serenity::all::{ChannelId, Context, CreateAllowedMentions, CreateAttachment, CreateMessage, GuildId, Message, UserId};

use super::awards;
use super::biski_card::{self, Said, Shot};
use super::stats;
use crate::storage::state::StateStorage;

type Storage = Arc<crate::storage::VizierStorage>;

/// The channel made for it on 19 Nov 2026; `VIZIER_BISKI_CHANNEL` moves it.
const BISKI_CHANNEL: u64 = 1_572_834_797_758_193_714;
const MAX_CHARS: usize = 1_500;

static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>").expect("regex"));
static CUSTOM_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").expect("regex"));

pub fn channel() -> ChannelId {
    ChannelId::new(super::control::id("VIZIER_BISKI_CHANNEL").unwrap_or(BISKI_CHANNEL))
}

fn key(message: u64) -> String {
    format!("biski__{}", message)
}

/// What came of a Biski.
pub enum Done {
    Posted(String),
    Already(String),
}

/// The words as people read them: names instead of ids, custom emoji by name.
fn words(m: &Message, bot_id: u64) -> String {
    let mut t = super::humanise_mentions(m, bot_id);
    t = CUSTOM_EMOJI.replace_all(&t, ":$1:").to_string();
    t = MENTION.replace_all(&t, "@someone").to_string();
    let mut t = t.trim().to_string();
    if t.is_empty() {
        if let Some(sticker) = m.sticker_items.first() {
            t = format!("[sticker: {}]", sticker.name);
        }
    }
    if t.chars().count() > MAX_CHARS {
        t = format!("{}…", t.chars().take(MAX_CHARS).collect::<String>().trim_end());
    }
    t
}

async fn fetch(url: &str) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(15)).build().ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.bytes().await.ok().map(|b| b.to_vec())
}

fn is_picture(a: &serenity::all::Attachment) -> bool {
    let by_type = a.content_type.as_deref().is_some_and(|t| t.starts_with("image/"));
    let name = a.filename.to_lowercase();
    by_type || [".png", ".jpg", ".jpeg", ".gif", ".webp"].iter().any(|e| name.ends_with(e))
}

/// A moving picture to post under the card, so the GIF actually plays.
pub struct Moving {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Discord takes 8 MB per file from a bot without boosts; stay well under.
const MAX_MOVING: usize = 7 * 1024 * 1024;

fn ext_of(url: &str) -> String {
    let path = url.split('?').next().unwrap_or(url).to_lowercase();
    path.rsplit('.').next().filter(|e| e.len() <= 4).unwrap_or("gif").to_string()
}

/// The message's picture: an attached image, a sticker, or the preview of a
/// GIF link (Tenor, Giphy, Klipy...). Returns the still the card draws, the
/// animation to post beside it if there is one, and whether the message's
/// words were only the link (Discord hides those, so the card does too).
async fn media(m: &Message) -> (Option<Vec<u8>>, Option<Moving>, bool) {
    if let Some(a) = m.attachments.iter().find(|a| is_picture(a)) {
        let bytes = fetch(&a.url).await;
        let moving = (a.filename.to_lowercase().ends_with(".gif"))
            .then(|| bytes.clone())
            .flatten()
            .filter(|b| b.len() <= MAX_MOVING)
            .map(|b| Moving { filename: "biski.gif".into(), bytes: b });
        return (bytes, moving, false);
    }
    if let Some(st) = m.sticker_items.first() {
        use serenity::all::StickerFormatType as F;
        let ext = match st.format_type {
            F::Gif => Some("gif"),
            F::Png | F::Apng => Some("png"),
            _ => None,
        };
        if let Some(ext) = ext {
            let bytes = fetch(&format!("https://media.discordapp.net/stickers/{}.{}?size=320", st.id.get(), ext)).await;
            let moving = (ext == "gif")
                .then(|| bytes.clone())
                .flatten()
                .map(|b| Moving { filename: "biski-sticker.gif".into(), bytes: b });
            return (bytes, moving, false);
        }
    }
    let only_link = {
        let t = m.content.trim();
        !t.is_empty() && !t.contains(char::is_whitespace) && t.starts_with("http")
    };
    for e in &m.embeds {
        let still = e
            .image
            .as_ref()
            .map(|i| i.proxy_url.clone().unwrap_or_else(|| i.url.clone()))
            .or_else(|| e.thumbnail.as_ref().map(|t| t.proxy_url.clone().unwrap_or_else(|| t.url.clone())));
        let Some(still_url) = still else { continue };
        let Some(bytes) = fetch(&still_url).await else { continue };
        let moving_url = e
            .video
            .as_ref()
            .map(|v| v.proxy_url.clone().unwrap_or_else(|| v.url.clone()))
            .or_else(|| (ext_of(&still_url) == "gif").then(|| still_url.clone()));
        let moving = match moving_url {
            Some(u) => fetch(&u)
                .await
                .filter(|b| b.len() <= MAX_MOVING)
                .map(|b| Moving { filename: format!("biski.{}", ext_of(&u)), bytes: b }),
            None => None,
        };
        let gifv = e.kind.as_deref() == Some("gifv") || moving.is_some();
        return (Some(bytes), moving, only_link && gifv);
    }
    // No preview from Discord (yet): read the share page ourselves.
    if let Some(link) = m.content.split_whitespace().find(|w| w.starts_with("http")) {
        if let Some((name, bytes)) = super::fetch_embedded_media(link).await {
            let moving = (name.to_lowercase().ends_with(".gif") && bytes.len() <= MAX_MOVING)
                .then(|| Moving { filename: "biski.gif".into(), bytes: bytes.clone() });
            return (Some(bytes), moving, only_link);
        }
    }
    (None, None, false)
}

/// One message as the card draws it, and its animation if it has one.
async fn said(ctx: &Context, guild: GuildId, m: &Message, bot_id: u64) -> (Said, Option<Moving>) {
    let member = guild.member(ctx, m.author.id).await.ok();
    let name = member
        .as_ref()
        .map(|mm| awards::clean_name(mm.display_name()))
        .unwrap_or_else(|| m.author.display_name().to_string());
    let colour = member.as_ref().and_then(|mm| mm.colour(&ctx.cache)).map(|c| [c.r(), c.g(), c.b()]).filter(|c| *c != [0, 0, 0]);
    let face = member.as_ref().map(|mm| mm.face()).unwrap_or_else(|| m.author.face()).replace("size=1024", "size=128");
    let when = DateTime::from_timestamp(m.timestamp.unix_timestamp(), 0)
        .map(|t| t.with_timezone(&stats::ist()).format("%-d %b %Y, %-I:%M %p").to_string().replace("AM", "am").replace("PM", "pm"))
        .unwrap_or_default();
    let (picture, moving, only_link) = media(m).await;
    let text = if only_link { String::new() } else { words(m, bot_id) };
    (Said { name, colour, avatar: fetch(&face).await, text, when, picture }, moving)
}

/// The message `m` replied to, if it replied to one that still exists.
async fn replied_to(ctx: &Context, m: &Message) -> Option<Message> {
    if let Some(r) = &m.referenced_message {
        return Some((**r).clone());
    }
    let reference = m.message_reference.as_ref()?;
    let id = reference.message_id?;
    reference.channel_id.message(&ctx.http, id).await.ok()
}

/// Screenshot `original` (and what it replied to) into the biski channel.
pub async fn run(ctx: &Context, storage: &Storage, original: &Message, by: UserId, guild: GuildId) -> Result<Done, String> {
    let channel_name = ctx
        .cache
        .guild(guild)
        .and_then(|g| g.channels.get(&original.channel_id).map(|c| c.name.clone()))
        .unwrap_or_default();
    if super::weekly::is_safe_corner(original.channel_id.get(), &channel_name) {
        return Err("safe-corner".into());
    }
    if let Some(url) = storage.get_state(key(original.id.get())).await.ok().flatten().and_then(|v| v.as_str().map(String::from)) {
        return Ok(Done::Already(url));
    }

    let bot_id = ctx.cache.current_user().id.get();
    let reply = match replied_to(ctx, original).await {
        Some(r) => Some(said(ctx, guild, &r, bot_id).await.0),
        None => None,
    };
    let (main, moving) = said(ctx, guild, original, bot_id).await;
    let shot = Shot { replied_to: reply, said: main };
    let png = tokio::task::spawn_blocking(move || {
        let mut fs = awards::fonts().lock();
        if fs.db().len() == 0 {
            return Err("no fonts installed to draw with".to_string());
        }
        biski_card::render(&shot, &mut fs).ok_or_else(|| "drawing the screenshot failed".to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let jump = original.link();
    let mut post = CreateMessage::new()
        .content(format!("📸 <@{}> ne biski kiya · {}", by.get(), jump))
        .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles())
        .add_file(CreateAttachment::bytes(png, "biski.png"));
    if let Some(m) = moving {
        post = post.add_file(CreateAttachment::bytes(m.bytes, m.filename));
    }
    let posted = channel()
        .send_message(&ctx.http, post)
        .await
        .map_err(|e| e.to_string())?;
    let url = posted.link();
    if let Err(err) = storage.save_state(key(original.id.get()), serde_json::json!(url)).await {
        tracing::warn!("biski: couldn't remember {}: {}", original.id, err);
    }
    Ok(Done::Posted(url))
}
