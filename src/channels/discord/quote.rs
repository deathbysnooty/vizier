//! Quote cards: turn a message into an image worth keeping.
//!
//! Asked for by replying to a message with "@Loduchand quote this", or from
//! the message's right-click Apps menu. The card goes up as a reply to the
//! original with a style picker and a Save button that only the person who
//! asked can use. Saving locks the style, drops the controls and copies the
//! card to the quotes channel (#gyaan14).

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use chrono::DateTime;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serenity::all::{
    ButtonStyle, ChannelId, ComponentInteraction, ComponentInteractionDataKind, Context, CreateActionRow,
    CreateAllowedMentions, CreateAttachment, CreateButton, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse, EditMessage,
    GuildId, Message, MessageId, UserId,
};

use super::awards;
use super::quote_card::{self, Quote, Style};
use super::stats;
use crate::storage::state::StateStorage;

type Storage = Arc<crate::storage::VizierStorage>;

/// A quote nobody saves locks in whatever style it shows after this long, so
/// a forgotten one still reaches the quotes channel.
const AUTO_SAVE_AFTER: Duration = Duration::from_secs(15 * 60);
const MAX_CHARS: usize = 420;

#[derive(Serialize, Deserialize, Clone, Debug)]
struct Record {
    id: String,
    guild_id: u64,
    channel_id: u64,
    message_id: u64,
    reply_id: u64,
    quoter_id: u64,
    author: String,
    handle: String,
    avatar_url: String,
    text: String,
    when: String,
    channel_name: String,
    style: String,
    saved: bool,
}

fn key(id: &str) -> String {
    format!("quote__{}", id)
}

async fn load(storage: &Storage, id: &str) -> Option<Record> {
    storage.get_state(key(id)).await.ok().flatten().and_then(|v| serde_json::from_value(v).ok())
}

async fn store(storage: &Storage, rec: &Record) {
    if let Ok(v) = serde_json::to_value(rec) {
        let _ = storage.save_state(key(&rec.id), v).await;
    }
}

static ASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:isko\s+|ye\s+|this\s+)?(?:quote|qoute)(?:\s+(?:this|it|kar|karo|kr|kardo|krdo|kar\s+do))?\s*[.!?]*\s*$")
        .expect("regex")
});
static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>").expect("regex"));
static CUSTOM_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").expect("regex"));

/// "@Loduchand quote this", sent as a reply to the message to quote.
pub fn is_request(msg: &Message, bot_id: u64) -> bool {
    msg.message_reference.is_some()
        && msg.mentions_user_id(UserId::new(bot_id))
        && ASK.is_match(&MENTION.replace_all(&msg.content, " "))
}

/// The words as people read them: names instead of ids, custom emoji by
/// name, markdown markers dropped, and a placeholder for a picture-only post.
fn quote_text(m: &Message, bot_id: u64) -> String {
    let mut t = super::humanise_mentions(m, bot_id);
    t = CUSTOM_EMOJI.replace_all(&t, ":$1:").to_string();
    t = MENTION.replace_all(&t, "@someone").to_string();
    for marker in ["**", "__", "~~", "||", "`"] {
        t = t.replace(marker, "");
    }
    let t = t.trim().to_string();
    let t = if !t.is_empty() {
        t
    } else if let Some(sticker) = m.sticker_items.first() {
        format!("[sticker: {}]", sticker.name)
    } else if !m.attachments.is_empty() {
        "📷 [photo]".to_string()
    } else {
        "…".to_string()
    };
    if t.chars().count() > MAX_CHARS {
        format!("{}…", t.chars().take(MAX_CHARS).collect::<String>().trim_end())
    } else {
        t
    }
}

fn escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| if "*_~|`>".contains(c) { vec!['\\', c] } else { vec![c] })
        .collect()
}

fn prompt_line(rec: &Record) -> String {
    format!("📜 Quote by <@{}> — style chuno, phir **Save** dabao. Sirf wahi kar sakta hai.", rec.quoter_id)
}

fn controls(id: &str, current: Style) -> Vec<CreateActionRow> {
    let options = Style::ALL
        .iter()
        .map(|s| CreateSelectMenuOption::new(s.label(), s.key()).default_selection(*s == current))
        .collect();
    vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(format!("qstyle:{}", id), CreateSelectMenuKind::String { options })
                .placeholder("Style chuno"),
        ),
        CreateActionRow::Buttons(vec![CreateButton::new(format!("qsave:{}", id)).label("Save ✓").style(ButtonStyle::Success)]),
    ]
}

fn quotes_channel(ctx: &Context, guild: GuildId) -> Option<ChannelId> {
    if let Some(id) = std::env::var("VIZIER_QUOTES_CHANNEL").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        return Some(ChannelId::new(id));
    }
    ctx.cache
        .guild(guild)
        .and_then(|g| g.channels.values().find(|c| c.name.to_lowercase().contains("gyaan")).map(|c| c.id))
}

async fn fetch(url: &str) -> Option<Vec<u8>> {
    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().ok()?;
    client.get(url).send().await.ok()?.bytes().await.ok().map(|b| b.to_vec())
}

async fn draw(rec: &Record) -> Result<Vec<u8>, String> {
    let quote = Quote {
        text: rec.text.clone(),
        author: rec.author.clone(),
        handle: rec.handle.clone(),
        avatar: fetch(&rec.avatar_url).await,
        when: rec.when.clone(),
        channel: if rec.channel_name.is_empty() { String::new() } else { format!("#{}", rec.channel_name) },
    };
    let style = Style::from_key(&rec.style).unwrap_or(Style::Noir);
    tokio::task::spawn_blocking(move || {
        let mut fs = awards::fonts().lock();
        if fs.db().len() == 0 {
            return Err("no fonts installed to draw with".to_string());
        }
        quote_card::render(&quote, style, &mut fs).ok_or_else(|| "drawing the quote failed".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Make a quote of `original` for `quoter` and post it as a reply.
pub async fn start(ctx: &Context, storage: &Storage, original: &Message, quoter: UserId, guild: GuildId) -> Result<(), String> {
    let bot_id = ctx.cache.current_user().id.get();
    let member = guild.member(ctx, original.author.id).await.ok();
    let author = member
        .as_ref()
        .map(|m| awards::clean_name(m.display_name()))
        .unwrap_or_else(|| original.author.display_name().to_string());
    let avatar_url = member
        .as_ref()
        .map(|m| m.face())
        .unwrap_or_else(|| original.author.face())
        .replace("size=1024", "size=256");
    let channel_name = ctx
        .cache
        .guild(guild)
        .and_then(|g| g.channels.get(&original.channel_id).map(|c| c.name.clone()))
        .unwrap_or_default();
    let when = DateTime::from_timestamp(original.timestamp.unix_timestamp(), 0)
        .map(|t| t.with_timezone(&stats::ist()).format("%-d %b %Y").to_string())
        .unwrap_or_default();

    let mut rec = Record {
        id: nanoid::nanoid!(10),
        guild_id: guild.get(),
        channel_id: original.channel_id.get(),
        message_id: original.id.get(),
        reply_id: 0,
        quoter_id: quoter.get(),
        author,
        handle: original.author.name.clone(),
        avatar_url,
        text: quote_text(original, bot_id),
        when,
        channel_name,
        style: Style::Noir.key().to_string(),
        saved: false,
    };
    let png = draw(&rec).await?;
    let sent = original
        .channel_id
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(prompt_line(&rec))
                .reference_message(original)
                .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles().replied_user(false))
                .add_file(CreateAttachment::bytes(png, "quote.png"))
                .components(controls(&rec.id, Style::Noir)),
        )
        .await
        .map_err(|e| e.to_string())?;
    rec.reply_id = sent.id.get();
    store(storage, &rec).await;

    let (ctx, storage, id) = (ctx.clone(), storage.clone(), rec.id.clone());
    tokio::spawn(async move {
        tokio::time::sleep(AUTO_SAVE_AFTER).await;
        if let Some(rec) = load(&storage, &id).await {
            if !rec.saved {
                save(&ctx, &storage, rec, None).await;
            }
        }
    });
    Ok(())
}

async fn deny(ctx: &Context, component: &ComponentInteraction, text: &str) {
    let _ = component
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .content(text)
                    .allowed_mentions(CreateAllowedMentions::new().empty_users()),
            ),
        )
        .await;
}

/// Style picker and Save button on a quote card.
pub async fn on_component(ctx: &Context, storage: &Storage, component: &ComponentInteraction) {
    let Some((action, id)) = component.data.custom_id.split_once(':') else { return };
    let Some(mut rec) = load(storage, id).await else {
        deny(ctx, component, "Ye quote ab mere paas nahi hai.").await;
        return;
    };
    if component.user.id.get() != rec.quoter_id {
        deny(ctx, component, &format!("Ye quote <@{}> ne banaya hai. Style bhi wahi chunega.", rec.quoter_id)).await;
        return;
    }
    if rec.saved {
        deny(ctx, component, "Ye already save ho chuka hai.").await;
        return;
    }
    // Acknowledge at once; drawing takes longer than Discord waits.
    let _ = component.defer(&ctx.http).await;
    match action {
        "qstyle" => {
            let picked = match &component.data.kind {
                ComponentInteractionDataKind::StringSelect { values } => values.first().and_then(|v| Style::from_key(v)),
                _ => None,
            };
            let Some(style) = picked else { return };
            rec.style = style.key().to_string();
            match draw(&rec).await {
                Ok(png) => {
                    let edit = EditInteractionResponse::new()
                        .content(prompt_line(&rec))
                        .clear_attachments()
                        .new_attachment(CreateAttachment::bytes(png, "quote.png"))
                        .components(controls(&rec.id, style));
                    if component.edit_response(&ctx.http, edit).await.is_ok() {
                        store(storage, &rec).await;
                    }
                }
                Err(err) => tracing::warn!("quote redraw failed: {}", err),
            }
        }
        "qsave" => save(ctx, storage, rec, Some(component)).await,
        _ => {}
    }
}

/// Lock the quote in its current style and copy it to the quotes channel.
async fn save(ctx: &Context, storage: &Storage, mut rec: Record, via: Option<&ComponentInteraction>) {
    let png = match draw(&rec).await {
        Ok(p) => p,
        Err(err) => {
            tracing::warn!("quote save failed: {}", err);
            return;
        }
    };
    let jump = format!("https://discord.com/channels/{}/{}/{}", rec.guild_id, rec.channel_id, rec.message_id);
    let posted = match quotes_channel(ctx, GuildId::new(rec.guild_id)) {
        Some(channel) => channel
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .content(format!(
                        "📜 **{}** ne kaha in <#{}> · quoted by <@{}> · [original](<{}>)",
                        escape(&rec.author),
                        rec.channel_id,
                        rec.quoter_id,
                        jump
                    ))
                    .allowed_mentions(CreateAllowedMentions::new().empty_users().empty_roles())
                    .add_file(CreateAttachment::bytes(png, "quote.png")),
            )
            .await
            .map_err(|e| tracing::warn!("quote: posting to the quotes channel failed: {}", e))
            .ok()
            .map(|m| (channel, m)),
        None => {
            tracing::warn!("quote: no quotes channel found");
            None
        }
    };
    let done = match &posted {
        Some((channel, m)) => format!("📜 Quote by <@{}> · saved in <#{}> → <{}>", rec.quoter_id, channel.get(), m.link()),
        None => format!("📜 Quote by <@{}> · saved", rec.quoter_id),
    };
    match via {
        Some(component) => {
            let _ = component.edit_response(&ctx.http, EditInteractionResponse::new().content(done).components(vec![])).await;
        }
        None => {
            let _ = ChannelId::new(rec.channel_id)
                .edit_message(&ctx.http, MessageId::new(rec.reply_id), EditMessage::new().content(done).components(vec![]))
                .await;
        }
    }
    rec.saved = true;
    store(storage, &rec).await;
}
