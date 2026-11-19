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

/// One message as the card draws it.
async fn said(ctx: &Context, guild: GuildId, m: &Message, bot_id: u64) -> Said {
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
    let picture = match m.attachments.iter().find(|a| is_picture(a)) {
        Some(a) => fetch(&a.url).await,
        None => None,
    };
    Said { name, colour, avatar: fetch(&face).await, text: words(m, bot_id), when, picture }
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
        Some(r) => Some(said(ctx, guild, &r, bot_id).await),
        None => None,
    };
    let shot = Shot { replied_to: reply, said: said(ctx, guild, original, bot_id).await };
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
    let posted = channel()
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(format!("📸 <@{}> ne biski kiya · {}", by.get(), jump))
                .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles())
                .add_file(CreateAttachment::bytes(png, "biski.png")),
        )
        .await
        .map_err(|e| e.to_string())?;
    let url = posted.link();
    if let Err(err) = storage.save_state(key(original.id.get()), serde_json::json!(url)).await {
        tracing::warn!("biski: couldn't remember {}: {}", original.id, err);
    }
    Ok(Done::Posted(url))
}
