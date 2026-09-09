use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serenity::all::{
    ChannelId, Command, CreateAttachment, CreateCommand, CreateCommandOption,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateMessage, Http,
    Interaction, Ready, Typing,
};
use serenity::async_trait;
use serenity::model::channel::Message;
use serenity::prelude::*;
use tokio::sync::watch;

use crate::channels::VizierChannel;
use crate::dependencies::VizierDependencies;
use crate::error::VizierError;
use crate::schema::{
    PlatformMessageId, TopicId, VizierAttachment, VizierAttachmentContent, VizierChannelId,
    VizierRequest, VizierRequestContent, VizierResponse, VizierResponseContent, VizierSession,
};
use crate::storage::session::SessionStorage;
use crate::storage::state::StateStorage;
use crate::transport::VizierTransport;
use crate::utils::remove_think_tags;

pub struct DiscordChannelReader {
    deps: VizierDependencies,
    token: String,
    agent_id: String,
    shutdown: (flume::Sender<bool>, flume::Receiver<bool>),
}

impl DiscordChannelReader {
    pub async fn new(agent_id: String, token: String, deps: VizierDependencies) -> Result<Self> {
        Ok(Self {
            deps,
            agent_id,
            token,
            shutdown: flume::bounded(1),
        })
    }
}

#[async_trait::async_trait]
impl VizierChannel for DiscordChannelReader {
    async fn run(&self) -> Result<()> {
        let intents = GatewayIntents::all();
        let mut client = Client::builder(self.token.clone(), intents)
            .event_handler(Handler(self.agent_id.clone(), self.deps.clone()))
            .await?;
        let shard_manager = client.shard_manager.clone();

        let handle = tokio::spawn(async move {
            let result = client.start();
            result.await
        });

        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            if shutdown.1.recv_async().await.is_ok() {
                shard_manager.shutdown_all().await;
            }
        });

        Ok(handle.await??)
    }

    async fn shutdown(&self) -> Result<()> {
        let res = self.shutdown.0.send_async(true).await;
        Ok(())
    }
}

struct Handler(String, VizierDependencies);

#[derive(Debug, Deserialize, Serialize)]
struct ChannelState {
    active_topic: Option<TopicId>,
    #[serde(default)]
    show_thinking: bool,
    #[serde(default)]
    show_tool_calls: bool,
}

/// Discord user IDs allowed to run `/stop` and `/resume`.
///
/// Override at runtime with `VIZIER_DISCORD_ADMIN_IDS` (comma-separated) so the
/// admin list can change without rebuilding the binary.
const DEFAULT_ADMIN_IDS: &[u64] = &[
    1417834414368362596,
    1378616145929703497,
    280982228228505600,
];

fn admin_ids() -> Vec<u64> {
    match std::env::var("VIZIER_DISCORD_ADMIN_IDS") {
        Ok(raw) if !raw.trim().is_empty() => raw
            .split(',')
            .filter_map(|s| s.trim().parse::<u64>().ok())
            .collect(),
        _ => DEFAULT_ADMIN_IDS.to_vec(),
    }
}

/// State key holding the agent-wide pause flag (applies across every channel).
fn paused_key(agent_id: &str) -> String {
    format!("{}__paused", agent_id)
}

/// Whether the agent is currently paused. Defaults to running on any error, so
/// a storage failure can never leave the bot silently dead.
async fn is_paused(storage: &Arc<crate::storage::VizierStorage>, agent_id: &str) -> bool {
    matches!(
        storage.get_state(paused_key(agent_id)).await,
        Ok(Some(serde_json::Value::Bool(true)))
    )
}

/// Channels the bot is allowed to see, from `VIZIER_DISCORD_CHANNELS`
/// (comma-separated ids). Empty or unset means every channel it can view,
/// which is the upstream behaviour.
fn allowed_channels() -> Vec<u64> {
    std::env::var("VIZIER_DISCORD_CHANNELS")
        .ok()
        .map(|raw| {
            raw.split(',')
                .filter_map(|s| s.trim().parse::<u64>().ok())
                .collect::<Vec<u64>>()
        })
        .unwrap_or_default()
}

/// State key holding the agent-wide admin-only flag.
fn admin_only_key(agent_id: &str) -> String {
    format!("{}__admin_only", agent_id)
}

/// Whether the agent is restricted to admins. Fails open like `is_paused`.
async fn is_admin_only(storage: &Arc<crate::storage::VizierStorage>, agent_id: &str) -> bool {
    matches!(
        storage.get_state(admin_only_key(agent_id)).await,
        Ok(Some(serde_json::Value::Bool(true)))
    )
}

// ---------------------------------------------------------------------------
// Kalesh detection
//
// Two stages, so the model is not run on every message. Stage one is a free
// heuristic over a rolling in-memory window: a burst of messages from only a
// few people, heavy on replies, looks like an argument. Stage two asks a model
// whether it actually is one, because on a bakchodi server that shape is also
// exactly what excited banter looks like. Only stage two can trigger a ping.
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct SeenMessage {
    at: std::time::Instant,
    author: u64,
    author_name: String,
    text: String,
    is_reply: bool,
}

#[derive(Default)]
struct ChannelWindow {
    messages: Vec<SeenMessage>,
    last_alert: Option<std::time::Instant>,
}

static KALESH_STATE: std::sync::OnceLock<
    std::sync::Mutex<HashMap<u64, ChannelWindow>>,
> = std::sync::OnceLock::new();

fn kalesh_state() -> &'static std::sync::Mutex<HashMap<u64, ChannelWindow>> {
    KALESH_STATE.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn kalesh_role_id() -> Option<u64> {
    std::env::var("VIZIER_KALESH_ROLE_ID").ok()?.trim().parse().ok()
}

/// Channels to watch for kalesh. Unset means the feature is off entirely.
fn kalesh_channels() -> Vec<u64> {
    std::env::var("VIZIER_KALESH_CHANNELS")
        .ok()
        .map(|raw| raw.split(',').filter_map(|s| s.trim().parse::<u64>().ok()).collect())
        .unwrap_or_default()
}

/// Record a message and report the recent window if it looks like a fight.
/// Returns None when the shape is unremarkable, the channel is not watched, or
/// the cooldown is still running.
fn note_message_and_check(msg: &Message) -> Option<Vec<SeenMessage>> {
    let channel = msg.channel_id.get();
    if !kalesh_channels().contains(&channel) {
        return None;
    }

    let window_secs = env_u64("VIZIER_KALESH_WINDOW_SECS", 90);
    let min_msgs = env_u64("VIZIER_KALESH_MIN_MSGS", 10) as usize;
    let max_authors = env_u64("VIZIER_KALESH_MAX_AUTHORS", 4) as usize;
    let min_replies = env_u64("VIZIER_KALESH_MIN_REPLIES", 4) as usize;
    let cooldown_secs = env_u64("VIZIER_KALESH_COOLDOWN_SECS", 900);

    let now = std::time::Instant::now();
    let mut guard = kalesh_state().lock().ok()?;
    let entry = guard.entry(channel).or_default();

    entry.messages.push(SeenMessage {
        at: now,
        author: msg.author.id.get(),
        author_name: msg.author.display_name().to_string(),
        text: msg.content.clone(),
        is_reply: msg.referenced_message.is_some(),
    });
    entry
        .messages
        .retain(|m| now.duration_since(m.at).as_secs() <= window_secs);
    // Keep the buffer bounded even in a very fast channel.
    if entry.messages.len() > 80 {
        let cut = entry.messages.len() - 80;
        entry.messages.drain(0..cut);
    }

    if let Some(last) = entry.last_alert {
        if now.duration_since(last).as_secs() < cooldown_secs {
            return None;
        }
    }

    if entry.messages.len() < min_msgs {
        return None;
    }
    let authors: std::collections::HashSet<u64> =
        entry.messages.iter().map(|m| m.author).collect();
    if authors.len() > max_authors {
        return None;
    }
    let replies = entry.messages.iter().filter(|m| m.is_reply).count();
    if replies < min_replies {
        return None;
    }

    // Stage one passed. Claim the cooldown now so a burst cannot queue several
    // model calls before the first one answers.
    entry.last_alert = Some(now);
    Some(entry.messages.clone())
}

/// Ask a model whether the window is a real fight, and for a line to announce
/// it with. Returns the line only when it says yes.
async fn classify_kalesh(window: &[SeenMessage]) -> Option<String> {
    let api_key = std::env::var("OPENROUTER_API_KEY").ok()?;
    let model = std::env::var("VIZIER_KALESH_MODEL")
        .unwrap_or_else(|_| "google/gemini-2.5-flash-lite".to_string());

    let transcript = window
        .iter()
        .map(|m| format!("{}: {}", m.author_name, m.text))
        .collect::<Vec<_>>()
        .join("\n");

    let prompt = format!(
        "You watch an Indian Discord server where people banter constantly (bakchodi). \
         Below is a burst of recent messages from one channel.\n\n\
         Decide whether this is a real fight - people genuinely angry at each other, \
         personal attacks, someone upset - or just loud banter, roasting between friends, \
         hype, or an animated discussion.\n\n\
         Loud is not a fight. Swearing is not a fight. Friends roasting each other is not \
         a fight. Only say yes if someone actually seems angry or hurt.\n\n\
         Reply with JSON only: {{\"kalesh\": true|false, \"line\": \"...\"}}\n\
         If kalesh is true, `line` is one short playful Hinglish line announcing the drama, \
         popcorn energy, taking nobody's side and naming no winner. If false, `line` is \"\".\n\n\
         Messages:\n{}",
        transcript
    );

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 300,
        "response_format": {"type": "json_object"}
    });

    let resp = reqwest::Client::new()
        .post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .ok()?;
    let json: serde_json::Value = resp.json().await.ok()?;
    let content = json["choices"][0]["message"]["content"].as_str()?;
    let parsed: serde_json::Value = serde_json::from_str(content.trim()).ok()?;

    if parsed["kalesh"].as_bool() != Some(true) {
        return None;
    }
    let line = parsed["line"].as_str().unwrap_or("").trim().to_string();
    Some(if line.is_empty() {
        "Kalesh detected 🍿".to_string()
    } else {
        line
    })
}

/// Largest embedded GIF we will pull in. Animated GIFs get big, and the whole
/// thing is base64'd into the prompt, so this is a hard stop rather than a hint.
const MAX_EMBED_BYTES: usize = 3 * 1024 * 1024;

/// Pull the sharable URLs out of a message body.
///
/// Tenor and Giphy links are not Discord attachments - the message contains
/// only a link and Discord renders the preview itself - so the bot sees nothing
/// unless we go and resolve them.
fn embedded_media_links(content: &str) -> Vec<String> {
    content
        .split_whitespace()
        .filter(|w| w.starts_with("http://") || w.starts_with("https://"))
        .filter(|w| {
            let l = w.to_lowercase();
            l.contains("tenor.com") || l.contains("giphy.com")
        })
        .map(|w| w.trim_end_matches(&[',', '.', ')', ']', '>'][..]).to_string())
        // One per message. Base64 inflates by a third and the provider caps
        // total request size, so two large GIFs would blow the limit.
        .take(1)
        .collect()
}

/// Find the media URL a share page points at, via its OpenGraph tags.
fn extract_og_media(html: &str) -> Option<String> {
    for tag in ["og:image", "twitter:image", "og:video"] {
        if let Some(i) = html.find(&format!("property=\"{}\"", tag))
            .or_else(|| html.find(&format!("name=\"{}\"", tag)))
        {
            let rest = &html[i..];
            if let Some(c) = rest.find("content=\"") {
                let after = &rest[c + 9..];
                if let Some(end) = after.find('"') {
                    let url = &after[..end];
                    if url.starts_with("http") {
                        return Some(url.to_string());
                    }
                }
            }
        }
    }
    None
}

fn filename_for(url: &str) -> String {
    let clean = url.split('?').next().unwrap_or(url);
    let ext = [".gif", ".png", ".jpg", ".jpeg", ".webp"]
        .iter()
        .find(|e| clean.to_lowercase().ends_with(*e))
        .map(|e| e.trim_start_matches('.'))
        .unwrap_or("gif");
    format!("embedded.{}", ext)
}

/// Resolve one Tenor/Giphy link to real image bytes, or None if anything is off.
async fn fetch_embedded_media(link: &str) -> Option<(String, Vec<u8>)> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?;

    // A direct media link needs no page lookup.
    // Direct media links (media1.tenor.com/..., media.giphy.com/..., or any
    // plain image URL) need no page lookup. Note the numbered Tenor CDN hosts.
    let lower = link.to_lowercase();
    let is_direct = lower.contains("tenor.com/m/")
        || lower.contains("media.giphy.com")
        || lower.contains("giphy.com/media/")
        || [".gif", ".png", ".jpg", ".jpeg", ".webp"]
            .iter()
            .any(|e| lower.split('?').next().unwrap_or(&lower).ends_with(e));
    let media_url = if is_direct {
        link.to_string()
    } else {
        let html = client
            .get(link)
            .header("User-Agent", "Mozilla/5.0 (compatible; vizier-bot)")
            .send()
            .await
            .ok()?
            .text()
            .await
            .ok()?;
        extract_og_media(&html)?
    };

    let resp = client.get(&media_url).send().await.ok()?;
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_EMBED_BYTES {
            tracing::debug!("skipping embedded media, {} bytes is over the cap", len);
            return None;
        }
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > MAX_EMBED_BYTES {
        tracing::debug!("skipping embedded media, {} bytes is over the cap", bytes.len());
        return None;
    }
    Some((filename_for(&media_url), bytes.to_vec()))
}

/// Discord puts mentions on the wire as raw ids - `<@123>` for a person,
/// `<#123>` for a channel - so the model never sees who or what was tagged and
/// ends up quoting numbers back at people. Rewrite them into readable names
/// before anything downstream touches the text.
fn humanise_mentions(msg: &Message, bot_id: u64) -> String {
    let mut out = msg.content.clone();

    // Strip our own mention first: it carries nothing, and doing it before the
    // name substitutions stops the bot being renamed into its own prompt.
    for pat in [format!("<@{}>", bot_id), format!("<@!{}>", bot_id)] {
        out = out.replace(&pat, " ");
    }

    for user in &msg.mentions {
        if user.id.get() == bot_id {
            continue;
        }
        let name = format!("@{}", user.display_name());
        for pat in [
            format!("<@{}>", user.id.get()),
            format!("<@!{}>", user.id.get()),
        ] {
            out = out.replace(&pat, &name);
        }
    }

    for ch in &msg.mention_channels {
        out = out.replace(&format!("<#{}>", ch.id.get()), &format!("#{}", ch.name));
    }

    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Role names for whoever sent a message, resolved through the guild cache.
///
/// Discord puts only role ids on the message, so without this the model cannot
/// tell a moderator from anyone else and treats the whole server identically.
fn author_role_names(ctx: &Context, msg: &Message) -> Vec<String> {
    let Some(guild_id) = msg.guild_id else {
        return vec![];
    };
    let Some(member) = msg.member.as_ref() else {
        return vec![];
    };
    let Some(guild) = ctx.cache.guild(guild_id) else {
        return vec![];
    };
    member
        .roles
        .iter()
        .filter_map(|rid| guild.roles.get(rid).map(|r| r.name.clone()))
        .filter(|n| n != "@everyone")
        .collect()
}

// ---------------------------------------------------------------------------
// Anonymous letters
//
// /letter picks a recipient and writes to them. The bot posts a notice in the
// letters channel; only the named recipient can open it, they see it privately,
// and it opens once. They can reply, and the reply travels back the same way.
//
// No model is involved anywhere in this, so it costs nothing per letter and is
// open to every member even while the AI side is admin-only.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Letter {
    id: String,
    from_id: u64,
    from_name: String,
    to_id: u64,
    to_name: String,
    body: String,
    sent_at: String,
    #[serde(default)]
    opened: bool,
    /// Set when this letter is itself a reply, so the notice can say so.
    #[serde(default)]
    in_reply_to: Option<String>,
    /// The public notice announcing this letter, removed once it is read.
    #[serde(default)]
    notice_msg: Option<u64>,
}

fn letter_key(id: &str) -> String {
    format!("letter__{}", id)
}

/// Index of letter ids per recipient. State storage is get/put by key with no
/// way to scan, so an inbox has to be maintained alongside the letters.
fn inbox_key(user_id: u64) -> String {
    format!("letterbox__{}", user_id)
}

/// Members who have turned letters off for themselves.
fn optout_key(user_id: u64) -> String {
    format!("letteroff__{}", user_id)
}

async fn letters_off(storage: &Arc<crate::storage::VizierStorage>, user_id: u64) -> bool {
    matches!(
        storage.get_state(optout_key(user_id)).await,
        Ok(Some(serde_json::Value::Bool(true)))
    )
}

async fn inbox_ids(storage: &Arc<crate::storage::VizierStorage>, user_id: u64) -> Vec<String> {
    match storage.get_state(inbox_key(user_id)).await {
        Ok(Some(v)) => serde_json::from_value(v).unwrap_or_default(),
        _ => Vec::new(),
    }
}

async fn inbox_push(storage: &Arc<crate::storage::VizierStorage>, user_id: u64, id: &str) {
    let mut ids = inbox_ids(storage, user_id).await;
    ids.push(id.to_string());
    // Bounded: an inbox is for what is still unread, not a permanent archive.
    if ids.len() > 100 {
        let cut = ids.len() - 100;
        ids.drain(0..cut);
    }
    if let Ok(v) = serde_json::to_value(ids) {
        let _ = storage.save_state(inbox_key(user_id), v).await;
    }
}

fn letters_channel() -> Option<u64> {
    std::env::var("VIZIER_LETTERS_CHANNEL").ok()?.trim().parse().ok()
}

/// Per-sender send times, for the rate limit. In memory on purpose: a restart
/// clearing it is harmless, and it keeps letters out of the write path.
static LETTER_RL: std::sync::OnceLock<std::sync::Mutex<HashMap<u64, Vec<std::time::Instant>>>> =
    std::sync::OnceLock::new();

/// True when the sender is within their allowance, recording the send if so.
fn letter_rate_ok(user: u64) -> bool {
    let max = env_u64("VIZIER_LETTERS_PER_HOUR", 5) as usize;
    let now = std::time::Instant::now();
    let Ok(mut guard) = LETTER_RL
        .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock()
    else {
        return true; // never block on a poisoned lock
    };
    let sends = guard.entry(user).or_default();
    sends.retain(|t| now.duration_since(*t).as_secs() < 3600);
    if sends.len() >= max {
        return false;
    }
    sends.push(now);
    true
}

async fn load_letter(
    storage: &Arc<crate::storage::VizierStorage>,
    id: &str,
) -> Option<Letter> {
    let v = storage.get_state(letter_key(id)).await.ok()??;
    serde_json::from_value(v).ok()
}

async fn save_letter(storage: &Arc<crate::storage::VizierStorage>, letter: &Letter) {
    if let Ok(v) = serde_json::to_value(letter) {
        let _ = storage.save_state(letter_key(&letter.id), v).await;
    }
}

/// Public announcements. The shout is loud on purpose - the whole fun is the
/// channel knowing somebody got one - while the letter itself stays private.
const LETTER_SHOUTS: &[&str] = &[
    "Oye Lodu <@{}>, tere naam ki gumnaam chitthi aayi hai 📬",
    "📬 <@{}> ko kisi ne anonymous letter bheja hai. Kaun? Pata nahi. Suspense.",
    "Breaking news: <@{}> ke naam ek gumnaam chitthi. Popcorn nikaalo 🍿",
    "Postman aaya hai <@{}> ke liye. Sender ka naam nahi likha 👀",
    "<@{}>, koi tumhe yaad kar raha hai... anonymously.",
    "Kisi ne <@{}> ko dil ki baat likhi hai. Ya gaali. Khol ke pata karo.",
    "<@{}> tere liye kuch aaya hai. Sirf tu padh sakta hai, baaki sab tadapenge.",
    "Attention <@{}>: gumnaam chitthi mili hai. Ab raat bhar sochna kaun tha.",
];

/// Rejections in the bot's own voice - it is the same character here as
/// anywhere else, and a flat error reads like a different bot.
const SELF_LETTER_LINES: &[&str] = &[
    "Kya bhai? Itna self obsessed mat ho, BC.",
    "Khud ko letter? Bhai therapy sasti hai isse.",
    "Apne aap ko chitthi bhej raha hai? Sach me itne akele ho?",
    "Nahi. Khud se pyaar ghar pe kar, yahan nahi.",
    "Bhai kam se kam ek dost bana le pehle.",
];

fn pick<'a>(pool: &'a [&'a str]) -> &'a str {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    pool[n % pool.len()]
}

/// Post the "you have a letter" notice with its Open button, returning the
/// message id so it can be removed once the letter is read.
async fn post_letter_notice(http: Arc<Http>, channel: u64, letter: &Letter) -> Option<u64> {
    // Replies are never posted to the channel. Naming both parties on every
    // exchange draws an obvious A, B, A, B pattern that identifies a pair of
    // correspondents to anyone reading, no matter what any single notice says.
    // The reply reaches its recipient privately and through /letterbox instead.
    if letter.in_reply_to.is_some() {
        let uid = serenity::all::UserId::new(letter.to_id);
        if let Ok(dm) = uid.create_dm_channel(&http).await {
            let _ = dm
                .id
                .send_message(
                    &http,
                    CreateMessage::new().content(
                        "Tumhari gumnaam chitthi ka jawab aa gaya hai. `/letterbox` likh ke kholo.",
                    ),
                )
                .await;
        }
        return None;
    }

    let pool = LETTER_SHOUTS;
    // Keyed off the letter id so a given letter always reads the same, while
    // consecutive letters vary.
    let idx = letter.id.bytes().map(|b| b as usize).sum::<usize>() % pool.len();
    let shout = pool[idx].replace("{}", &letter.to_id.to_string());

    let embed = serenity::all::CreateEmbed::new()
        .description("Only they can open it, and it opens once.")
        .colour(serenity::all::Colour::new(0x9B59B6));

    let button = serenity::all::CreateButton::new(format!("lopen:{}", letter.id))
        .label("Kholo")
        .style(serenity::all::ButtonStyle::Primary);

    let msg = CreateMessage::new()
        .content(shout)
        .embed(embed)
        .components(vec![serenity::all::CreateActionRow::Buttons(vec![button])]);

    match ChannelId::new(channel).send_message(&http, msg).await {
        Ok(sent) => Some(sent.id.get()),
        Err(err) => {
            tracing::error!("failed to post letter notice: {:?}", err);
            None
        }
    }
}

/// Validate and dispatch a `/letter`. Returns the private reply for the sender.
async fn handle_letter_command(
    ctx: &Context,
    command: &serenity::all::CommandInteraction,
    storage: &Arc<crate::storage::VizierStorage>,
) -> String {
    let Some(channel) = letters_channel() else {
        return "Anonymous letters are not set up on this server yet.".to_string();
    };

    let recipient = command.data.options.iter().find(|o| o.name == "recipient").and_then(|o| {
        match &o.value {
            serenity::all::CommandDataOptionValue::User(id) => Some(*id),
            _ => None,
        }
    });
    let body = command
        .data
        .options
        .iter()
        .find(|o| o.name == "message")
        .and_then(|o| o.value.as_str().map(|s| s.trim().to_string()))
        .unwrap_or_default();

    let Some(to_id) = recipient else {
        return "Kisko bhejna hai? Naam toh bata.".to_string();
    };
    if body.is_empty() {
        return "Khaali chitthi? Kuch likh toh sahi.".to_string();
    }
    if body.chars().count() > 1500 {
        return "Novel likh raha hai kya? 1500 characters se kam mein nipta.".to_string();
    }
    let from_id = command.user.id.get();
    if to_id.get() == from_id {
        return pick(SELF_LETTER_LINES).to_string();
    }
    if to_id.get() == ctx.cache.current_user().id.get() {
        return "Mujhe letter bhej ke kya milega? Kisi insaan ko bhej.".to_string();
    }
    if letters_off(storage, to_id.get()).await {
        // Said plainly on purpose: the sender needs to know it will not arrive,
        // and it reveals nothing beyond a preference they set themselves.
        return "Unhone gumnaam chitthiyan band kar rakhi hain. Kuch nahi jaayega.".to_string();
    }
    if !letter_rate_ok(from_id) {
        return "Bas kar bhai, postman thak gaya. Ek ghante baad aana.".to_string();
    }

    let to_name = to_id
        .to_user(&ctx.http)
        .await
        .map(|u| u.display_name().to_string())
        .unwrap_or_else(|_| "someone".to_string());

    let letter = Letter {
        id: nanoid::nanoid!(8),
        from_id,
        from_name: command.user.name.clone(),
        to_id: to_id.get(),
        to_name,
        body,
        sent_at: Utc::now().to_rfc3339(),
        opened: false,
        in_reply_to: None,
        notice_msg: None,
    };
    let mut letter = letter;
    letter.notice_msg = post_letter_notice(ctx.http.clone(), channel, &letter).await;
    save_letter(storage, &letter).await;
    inbox_push(storage, letter.to_id, &letter.id).await;

    format!(
        "Sent. They will see a notice in <#{}> and only they can open it.\nYour name is not shown. Letter id `{}`.",
        channel, letter.id
    )
}

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, ctx: Context, _ready: Ready) {
        let ping = CreateCommand::new("ping").description("a simple ping");

        let new = CreateCommand::new("new").description("create fresh new session");
        let session = CreateCommand::new("session")
            .description("list or select session")
            .add_option(CreateCommandOption::new(
                serenity::all::CommandOptionType::String,
                "topic_id",
                "switch to the topic if not empty",
            ));

        let _ = Command::create_global_command(ctx.http.clone(), ping).await;

        let _ = Command::create_global_command(ctx.http.clone(), new).await;
        let _ = Command::create_global_command(ctx.http.clone(), session).await;

        let abort = CreateCommand::new("abort").description("abort current thinking");
        let _ = Command::create_global_command(ctx.http.clone(), abort).await;

        let checkpoint =
            CreateCommand::new("checkpoint").description("save checkpoint with handover summary");
        let _ = Command::create_global_command(ctx.http.clone(), checkpoint).await;

        let lobotomy = CreateCommand::new("lobotomy")
            .description("save checkpoint without handover (clean break)");
        let _ = Command::create_global_command(ctx.http.clone(), lobotomy).await;

        let thinking = CreateCommand::new("thinking").description("toggle showing thinking output");
        let _ = Command::create_global_command(ctx.http.clone(), thinking).await;

        let tool_calls = CreateCommand::new("tool_calls").description("toggle showing tool call details");
        let _ = Command::create_global_command(ctx.http.clone(), tool_calls).await;

        let stop = CreateCommand::new("stop")
            .description("admin only: silence the bot everywhere until /resume");
        let _ = Command::create_global_command(ctx.http.clone(), stop).await;

        let resume =
            CreateCommand::new("resume").description("admin only: bring the bot back after /stop");
        let _ = Command::create_global_command(ctx.http.clone(), resume).await;

        let admin_only = CreateCommand::new("adminonly")
            .description("admin only: toggle whether the bot answers admins and nobody else");
        let _ = Command::create_global_command(ctx.http.clone(), admin_only).await;

        // Upstream handles /help but never registers it, so it never appears.
        let help = CreateCommand::new("help").description("what I do and how to use me");
        let _ = Command::create_global_command(ctx.http.clone(), help).await;

        let letter = CreateCommand::new("letter")
            .description("send someone an anonymous letter")
            .add_option(
                CreateCommandOption::new(
                    serenity::all::CommandOptionType::User,
                    "recipient",
                    "who it is for",
                )
                .required(true),
            )
            .add_option(
                CreateCommandOption::new(
                    serenity::all::CommandOptionType::String,
                    "message",
                    "what you want to say",
                )
                .required(true),
            );
        let _ = Command::create_global_command(ctx.http.clone(), letter).await;

        let inbox = CreateCommand::new("letterbox")
            .description("check your unopened anonymous letters (only you see this)");
        let _ = Command::create_global_command(ctx.http.clone(), inbox).await;

        // Deliberately not "letters": one character from "letter" meant people
        // could mute themselves while trying to send one.
        let toggle = CreateCommand::new("nochitthi")
            .description("stop or resume anonymous letters coming to you");
        let _ = Command::create_global_command(ctx.http.clone(), toggle).await;

        let trace = CreateCommand::new("letter_trace")
            .description("admin only: who sent a letter")
            .add_option(
                CreateCommandOption::new(
                    serenity::all::CommandOptionType::String,
                    "letter_id",
                    "the id shown at the bottom of the letter",
                )
                .required(true),
            );
        let _ = Command::create_global_command(ctx.http.clone(), trace).await;
    }

    async fn interaction_create(&self, ctx: Context, interaction: Interaction) {
        // Letter buttons: open, and the reply box.
        if let Interaction::Component(ref component) = interaction {
            let id = component.data.custom_id.clone();
            if let Some(letter_id) = id.strip_prefix("lopen:") {
                let clicker = component.user.id.get();
                let letter = load_letter(&self.1.storage, letter_id).await;

                let (text, offer_reply) = match letter {
                    None => ("That letter has gone missing.".to_string(), false),
                    Some(l) if l.to_id != clicker => {
                        // Say nothing about who it is for beyond that it is not them.
                        ("Ye teri chitthi nahi hai. Chal nikal.".to_string(), false)
                    }
                    Some(l) if l.opened => (
                        "Ek baar khol chuka hai. Dobara nahi milegi.".to_string(),
                        false,
                    ),
                    Some(mut l) => {
                        l.opened = true;

                        // Take the notice down. Leaving it up keeps a public
                        // record of who receives letters, and a notice that
                        // lingers marks its letter as still unread.
                        if let (Some(mid), Some(ch)) = (l.notice_msg, letters_channel()) {
                            let http = ctx.http.clone();
                            tokio::spawn(async move {
                                let _ = ChannelId::new(ch)
                                    .delete_message(&http, serenity::all::MessageId::new(mid))
                                    .await;
                            });
                        }
                        l.notice_msg = None;
                        save_letter(&self.1.storage, &l).await;

                        // On a reply, show what they wrote first - without it the
                        // answer arrives with no idea which letter it belongs to.
                        let mut text = String::new();
                        if let Some(ref orig_id) = l.in_reply_to {
                            if let Some(orig) = load_letter(&self.1.storage, orig_id).await {
                                text.push_str(&format!(
                                    "**Tumne likha tha:**\n> {}\n\n",
                                    orig.body.replace('\n', "\n> ")
                                ));
                            }
                            text.push_str("**Jawab:**\n");
                        } else {
                            text.push_str("**Gumnaam chitthi**\n\n");
                        }
                        text.push_str(&l.body);
                        text.push_str(&format!(
                            "\n\n-# id `{}` - dobara nahi khulegi",
                            l.id
                        ));
                        (text, true)
                    }
                };

                let mut resp = CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .content(text);
                if offer_reply {
                    resp = resp.components(vec![serenity::all::CreateActionRow::Buttons(vec![
                        serenity::all::CreateButton::new(format!("lreply:{}", letter_id))
                            .label("Reply anonymously")
                            .style(serenity::all::ButtonStyle::Secondary),
                    ])]);
                }
                let _ = component
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(resp),
                    )
                    .await;
                return;
            }

            if let Some(letter_id) = id.strip_prefix("lreply:") {
                let modal = serenity::all::CreateModal::new(
                    format!("lrmodal:{}", letter_id),
                    "Reply anonymously",
                )
                .components(vec![serenity::all::CreateActionRow::InputText(
                    serenity::all::CreateInputText::new(
                        serenity::all::InputTextStyle::Paragraph,
                        "Your reply",
                        "reply_body",
                    )
                    .required(true),
                )]);
                let _ = component
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Modal(modal),
                    )
                    .await;
                return;
            }
        }

        // Reply modal submitted: send it back the way it came.
        if let Interaction::Modal(ref modal) = interaction {
            if let Some(orig_id) = modal.data.custom_id.strip_prefix("lrmodal:") {
                let body = modal
                    .data
                    .components
                    .iter()
                    .flat_map(|row| row.components.iter())
                    .find_map(|c| match c {
                        serenity::all::ActionRowComponent::InputText(t) => t.value.clone(),
                        _ => None,
                    })
                    .unwrap_or_default();

                let sender = modal.user.id.get();
                let text = match (letters_channel(), load_letter(&self.1.storage, orig_id).await) {
                    (None, _) => "Anonymous letters are not set up on this server.".to_string(),
                    (_, None) => "That letter has gone missing.".to_string(),
                    (Some(channel), Some(orig)) => {
                        if orig.to_id != sender {
                            "Ye chitthi teri thi hi nahi, jawab kya dega.".to_string()
                        } else if body.trim().is_empty() {
                            "Khaali jawab? Rehne de.".to_string()
                        } else if !letter_rate_ok(sender) {
                            "Bas kar bhai, ek ghante baad aana.".to_string()
                        } else {
                            let reply = Letter {
                                id: nanoid::nanoid!(8),
                                from_id: sender,
                                from_name: modal.user.name.clone(),
                                // Back to whoever wrote the original.
                                to_id: orig.from_id,
                                to_name: orig.from_name.clone(),
                                body: body.chars().take(1500).collect(),
                                sent_at: Utc::now().to_rfc3339(),
                                opened: false,
                                in_reply_to: Some(orig.id.clone()),
                                notice_msg: None,
                            };
                            let mut reply = reply;
                            reply.notice_msg =
                                post_letter_notice(ctx.http.clone(), channel, &reply).await;
                            save_letter(&self.1.storage, &reply).await;
                            inbox_push(&self.1.storage, reply.to_id, &reply.id).await;
                            format!("Reply sent. They will see a notice in <#{}>.", channel)
                        }
                    }
                };
                let _ = modal
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new().ephemeral(true).content(text),
                        ),
                    )
                    .await;
                return;
            }
        }

        if let Interaction::Command(command) = interaction {
            let agent_id = self.0.clone();

            if command.data.name == "ping" {
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new().content("Pong!"),
                        ),
                    )
                    .await;
            }

            if command.data.name == "stop" || command.data.name == "resume" {
                let pause = command.data.name == "stop";
                let caller = command.user.id.get();

                // Authorisation is on the Discord-supplied user id, not on anything
                // the caller can put in a message, so it cannot be talked around.
                let is_admin = admin_ids().contains(&caller);
                let reply = if !is_admin {
                    tracing::warn!(
                        "rejected /{} from non-admin {} ({})",
                        command.data.name,
                        command.user.name,
                        caller
                    );
                    "Only server admins can use this.".to_string()
                } else {
                    match self
                        .1
                        .storage
                        .save_state(paused_key(&agent_id), serde_json::Value::Bool(pause))
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(
                                "agent '{}' {} by admin {} ({})",
                                agent_id,
                                if pause { "paused" } else { "resumed" },
                                command.user.name,
                                caller
                            );
                            if pause {
                                "Paused. I'll ignore everyone until an admin runs `/resume`."
                                    .to_string()
                            } else {
                                "Back online.".to_string()
                            }
                        }
                        Err(err) => {
                            tracing::error!("failed to persist pause flag: {:?}", err);
                            "Couldn't save that — try again.".to_string()
                        }
                    }
                };

                // Refusals go only to the caller, so a non-admin poking at this
                // cannot spam the channel.
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .ephemeral(!is_admin)
                                .content(reply),
                        ),
                    )
                    .await;
            }

            if command.data.name == "letter" {
                let reply = handle_letter_command(&ctx, &command, &self.1.storage).await;
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            // Always private: the whole point is that the channel
                            // shows a notice, not who sent what.
                            CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content(reply),
                        ),
                    )
                    .await;
            }

            if command.data.name == "nochitthi" {
                let me = command.user.id.get();
                let now_off = !letters_off(&self.1.storage, me).await;
                let _ = self
                    .1
                    .storage
                    .save_state(optout_key(me), serde_json::Value::Bool(now_off))
                    .await;
                let text = if now_off {
                    "Ab tumhe koi gumnaam chitthi nahi aayegi. Wapas chalu karne ke liye phir se `/nochitthi` likho."
                } else {
                    "Gumnaam chitthiyan wapas chalu. Ab log tumhe likh sakte hain."
                };
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content(text),
                        ),
                    )
                    .await;
            }

            if command.data.name == "letterbox" {
                let me = command.user.id.get();
                let ids = inbox_ids(&self.1.storage, me).await;
                let mut unopened: Vec<Letter> = vec![];
                // Already-read letters stay listed so they can still be replied
                // to. Dismissing the ephemeral message used to lose the reply
                // button for good, and there is no other route back to it.
                let mut read: Vec<Letter> = vec![];
                for id in ids.iter().rev() {
                    if let Some(l) = load_letter(&self.1.storage, id).await {
                        if l.to_id != me {
                            continue;
                        }
                        if l.opened {
                            if read.len() < 5 {
                                read.push(l);
                            }
                        } else if unopened.len() < 5 {
                            unopened.push(l);
                        }
                    }
                    if unopened.len() >= 5 && read.len() >= 5 {
                        break;
                    }
                }

                let (text, buttons) = if unopened.is_empty() && read.is_empty() {
                    ("Koi chitthi nahi hai. Sannata hai.".to_string(), vec![])
                } else if unopened.is_empty() {
                    (
                        format!(
                            "Koi nayi chitthi nahi hai.\n\n**Padhi hui {} — jawab de sakte ho:**",
                            read.len()
                        ),
                        vec![],
                    )
                } else {
                    let lines: Vec<String> = unopened
                        .iter()
                        .map(|l| {
                            let kind = if l.in_reply_to.is_some() {
                                "jawab"
                            } else {
                                "chitthi"
                            };
                            format!("- ek {} (`{}`)", kind, l.id)
                        })
                        .collect();
                    (
                        format!(
                            "**Tumhare {} unopened:**\n{}\n\nKholne ke liye niche button dabao.",
                            unopened.len(),
                            lines.join("\n")
                        ),
                        unopened
                            .iter()
                            .map(|l| {
                                serenity::all::CreateButton::new(format!("lopen:{}", l.id))
                                    .label(if l.in_reply_to.is_some() {
                                        "Jawab kholo"
                                    } else {
                                        "Chitthi kholo"
                                    })
                                    .style(serenity::all::ButtonStyle::Primary)
                            })
                            .collect::<Vec<_>>(),
                    )
                };

                let mut rows = vec![];
                if !buttons.is_empty() {
                    rows.push(serenity::all::CreateActionRow::Buttons(buttons));
                }
                if !read.is_empty() {
                    rows.push(serenity::all::CreateActionRow::Buttons(
                        read.iter()
                            .map(|l| {
                                serenity::all::CreateButton::new(format!("lreply:{}", l.id))
                                    .label(format!("Jawab do ({})", l.id))
                                    .style(serenity::all::ButtonStyle::Secondary)
                            })
                            .collect::<Vec<_>>(),
                    ));
                }

                let mut resp = CreateInteractionResponseMessage::new()
                    .ephemeral(true)
                    .content(text);
                if !rows.is_empty() {
                    resp = resp.components(rows);
                }
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(resp),
                    )
                    .await;
            }

            if command.data.name == "letter_trace" {
                let caller = command.user.id.get();
                let reply = if !admin_ids().contains(&caller) {
                    "Only server admins can use this.".to_string()
                } else {
                    let id = command
                        .data
                        .options
                        .iter()
                        .find(|o| o.name == "letter_id")
                        .and_then(|o| o.value.as_str().map(|s| s.to_string()))
                        .unwrap_or_default();
                    match load_letter(&self.1.storage, id.trim()).await {
                        Some(l) => {
                            tracing::info!(
                                "letter {} traced by admin {} ({})",
                                l.id,
                                command.user.name,
                                caller
                            );
                            format!(
                                "Letter `{}`\nFrom: {} (<@{}>)\nTo: {} (<@{}>)\nSent: {}\nOpened: {}\n\n{}",
                                l.id, l.from_name, l.from_id, l.to_name, l.to_id, l.sent_at, l.opened, l.body
                            )
                        }
                        None => format!("No letter found with id `{}`.", id.trim()),
                    }
                };
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .ephemeral(true)
                                .content(reply),
                        ),
                    )
                    .await;
            }

            if command.data.name == "adminonly" {
                let caller = command.user.id.get();
                let is_admin = admin_ids().contains(&caller);

                let reply = if !is_admin {
                    tracing::warn!(
                        "rejected /adminonly from non-admin {} ({})",
                        command.user.name,
                        caller
                    );
                    "Only server admins can use this.".to_string()
                } else {
                    let enable = !is_admin_only(&self.1.storage, &agent_id).await;
                    match self
                        .1
                        .storage
                        .save_state(admin_only_key(&agent_id), serde_json::Value::Bool(enable))
                        .await
                    {
                        Ok(()) => {
                            tracing::info!(
                                "agent '{}' admin-only mode {} by admin {} ({})",
                                agent_id,
                                if enable { "enabled" } else { "disabled" },
                                command.user.name,
                                caller
                            );
                            if enable {
                                "Admin-only mode **on**. I'll only answer admins — everyone else gets nothing.".to_string()
                            } else {
                                "Admin-only mode **off**. Back to answering everyone.".to_string()
                            }
                        }
                        Err(err) => {
                            tracing::error!("failed to persist admin-only flag: {:?}", err);
                            "Couldn't save that — try again.".to_string()
                        }
                    }
                };

                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .ephemeral(!is_admin)
                                .content(reply),
                        ),
                    )
                    .await;
            }

            if command.data.name == "new" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let topic_id = nanoid::nanoid!(10);

                let _ = self
                    .1
                    .storage
                    .save_state(
                        format!("{}__{}", agent_id, channel.to_slug()),
                        serde_json::to_value(ChannelState {
                            active_topic: Some(topic_id.clone()),
                            show_thinking: false,
                            show_tool_calls: false,
                        })
                        .unwrap(),
                    )
                    .await;

                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content(format!("switch to new session: **{}**", topic_id)),
                        ),
                    )
                    .await;
            }

            if command.data.name == "session" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let opt = command.data.options.clone();

                if let Some(raw_topic_id) = opt.iter().find_map(|opt| {
                    if opt.name == "topic_id".to_string() {
                        Some(opt.value.as_str().unwrap().to_string())
                    } else {
                        None
                    }
                }) {
                    let topic_id: Option<TopicId> = if raw_topic_id == "DEFAULT".to_string() {
                        None
                    } else {
                        Some(raw_topic_id.clone())
                    };

                    if let Ok(Some(_)) = self
                        .1
                        .storage
                        .get_session_detail_by_topic(
                            agent_id.clone(),
                            channel.clone(),
                            topic_id.clone(),
                        )
                        .await
                    {
                        let key = format!("{}__{}", agent_id, channel.to_slug());
                        let existing_state = if let Ok(Some(value)) = self.1.storage.get_state(key.clone()).await {
                            serde_json::from_value::<ChannelState>(value).unwrap_or(ChannelState {
                                active_topic: None,
                                show_thinking: false,
                                show_tool_calls: false,
                            })
                        } else {
                            ChannelState {
                                active_topic: None,
                                show_thinking: false,
                                show_tool_calls: false,
                            }
                        };
                        let _ = self
                            .1
                            .storage
                            .save_state(
                                key,
                                serde_json::to_value(ChannelState {
                                    active_topic: topic_id,
                                    show_thinking: existing_state.show_thinking,
                                    show_tool_calls: existing_state.show_tool_calls,
                                })
                                .unwrap(),
                            )
                            .await;

                        let _ = command
                            .create_response(
                                ctx.http.clone(),
                                serenity::all::CreateInteractionResponse::Message(
                                    CreateInteractionResponseMessage::new().content(format!(
                                        "switch to session: **{}**",
                                        raw_topic_id
                                    )),
                                ),
                            )
                            .await;
                    } else {
                        let _ = command
                            .create_response(
                                ctx.http.clone(),
                                serenity::all::CreateInteractionResponse::Message(
                                    CreateInteractionResponseMessage::new()
                                        .content("topic not found"),
                                ),
                            )
                            .await;
                    }
                } else {
                    if let Ok(sessions) = self
                        .1
                        .storage
                        .get_session_list(agent_id.clone(), Some(channel))
                        .await
                    {
                        let mut res = vec![];
                        for session in &sessions {
                            res.push(format!(
                                "topic_id: {}\ntitle: {}",
                                session.topic.clone().unwrap_or("DEFAULT".into()),
                                session.title.clone()
                            ));
                        }

                        let output = res.join("\n\n");
                        let _ = command
                            .create_response(
                                ctx.http.clone(),
                                serenity::all::CreateInteractionResponse::Message(
                                    CreateInteractionResponseMessage::new().content(output),
                                ),
                            )
                            .await;
                    }
                }
            }

            if command.data.name == "help" {
                if let Err(err) = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new().ephemeral(true).content(
                                r#"**Loduchand** — MLCI ka apna bot.

Mujhe channel mein @mention karo, tabhi reply karunga. Baaki time bas padhta rehta hoon.
DM ka jawab nahi deta — sab kuch yahin server mein.

**Commands**
• `/help` — yehi message
• `/new` — nayi baat, purani bhool jaunga
• `/session` — purani conversations dekho ya switch karo
• `/abort` — bahut der laga raha hoon toh rok do
• `/checkpoint` — ab tak ka summary save karo
• `/lobotomy` — sab bhula ke clean start
• `/thinking` — meri soch dikhaun ya nahi
• `/tool_calls` — background actions dikhaun ya nahi

**Sirf admins ke liye**
• `/stop` — mujhe chup kara do
• `/resume` — wapas online
• `/adminonly` — sirf admins se baat karun

Main galat bhi ho sakta hoon — check kar lena. Web browse nahi kar sakta, links nahi khol sakta, images nahi dekh sakta.
Ye message sirf tumhe dikh raha hai."#,
                            ),
                        ),
                    )
                    .await
                {
                    tracing::error!("{}", err)
                }
            }

            if command.data.name == "abort" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let key = format!("{}__{}", agent_id, channel.to_slug());
                let topic_id = if let Ok(Some(value)) = self.1.storage.get_state(key).await {
                    serde_json::from_value::<ChannelState>(value)
                        .ok()
                        .and_then(|s| s.active_topic)
                } else {
                    None
                };

                let session = VizierSession(agent_id.clone(), channel, topic_id);
                let _ = self
                    .1
                    .transport
                    .send_request(
                        session,
                        VizierRequest {
                            timestamp: Utc::now(),
                            user: agent_id.clone(),
                            content: VizierRequestContent::Command("abort".to_string()),
                            platform_message_id: None,
                            metadata: serde_json::json!({}),
                            attachments: vec![],
                            expect_audio_reply: None,
                        },
                        None,
                    )
                    .await;

                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new().content("aborting..."),
                        ),
                    )
                    .await;
            }

            if command.data.name == "checkpoint" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let key = format!("{}__{}", agent_id, channel.to_slug());
                let topic_id = if let Ok(Some(value)) = self.1.storage.get_state(key).await {
                    serde_json::from_value::<ChannelState>(value)
                        .ok()
                        .and_then(|s| s.active_topic)
                } else {
                    None
                };

                let session = VizierSession(agent_id.clone(), channel, topic_id);
                let (response_tx, response_rx) = flume::unbounded();
                let _ = self
                    .1
                    .transport
                    .send_request(
                        session,
                        VizierRequest {
                            timestamp: Utc::now(),
                            user: agent_id.clone(),
                            content: VizierRequestContent::Command("checkpoint".to_string()),
                            platform_message_id: None,
                            metadata: serde_json::json!({}),
                            attachments: vec![],
                            expect_audio_reply: None,
                        },
                        Some(response_tx),
                    )
                    .await;

                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content("creating checkpoint..."),
                        ),
                    )
                    .await;

                let followup_command = command.clone();
                let followup_http = ctx.http.clone();
                tokio::spawn(async move {
                    while let Ok(response) = response_rx.recv_async().await {
                        match response.content {
                            VizierResponseContent::Checkpoint { handover: Some(_) } => {
                                let _ = followup_command
                                    .create_followup(
                                        followup_http.clone(),
                                        CreateInteractionResponseFollowup::new()
                                            .content("✅ checkpoint saved"),
                                    )
                                    .await;
                                break;
                            }
                            VizierResponseContent::Checkpoint { handover: None } => {
                                let _ = followup_command
                                    .create_followup(
                                        followup_http.clone(),
                                        CreateInteractionResponseFollowup::new()
                                            .content("✅ lobotomy performed"),
                                    )
                                    .await;
                                break;
                            }
                            VizierResponseContent::Error { kind, message } => {
                                let kind_str = match kind {
                                    crate::schema::ErrorKind::Completion => "Completion Error",
                                    crate::schema::ErrorKind::ToolTimeout => "Tool Timeout",
                                    crate::schema::ErrorKind::PromptTimeout => "Prompt Timeout",
                                };
                                let _ = followup_command
                                    .create_followup(
                                        followup_http.clone(),
                                        CreateInteractionResponseFollowup::new().content(format!(
                                            "**{}**: {}",
                                            kind_str, message
                                        )),
                                    )
                                    .await;
                                break;
                            }
                            _ => {}
                        }
                    }
                });
            }

            if command.data.name == "lobotomy" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let key = format!("{}__{}", agent_id, channel.to_slug());
                let topic_id = if let Ok(Some(value)) = self.1.storage.get_state(key).await {
                    serde_json::from_value::<ChannelState>(value)
                        .ok()
                        .and_then(|s| s.active_topic)
                } else {
                    None
                };

                let session = VizierSession(agent_id.clone(), channel, topic_id);
                let (response_tx, response_rx) = flume::unbounded();
                let _ = self
                    .1
                    .transport
                    .send_request(
                        session,
                        VizierRequest {
                            timestamp: Utc::now(),
                            user: agent_id.clone(),
                            content: VizierRequestContent::Command("lobotomy".to_string()),
                            platform_message_id: None,
                            metadata: serde_json::json!({}),
                            attachments: vec![],
                            expect_audio_reply: None,
                        },
                        Some(response_tx),
                    )
                    .await;

                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content("performing lobotomy..."),
                        ),
                    )
                    .await;

                let followup_command = command.clone();
                let followup_http = ctx.http.clone();
                tokio::spawn(async move {
                    while let Ok(response) = response_rx.recv_async().await {
                        match response.content {
                            VizierResponseContent::Checkpoint { handover: Some(_) } => {
                                let _ = followup_command
                                    .create_followup(
                                        followup_http.clone(),
                                        CreateInteractionResponseFollowup::new()
                                            .content("✅ checkpoint saved"),
                                    )
                                    .await;
                                break;
                            }
                            VizierResponseContent::Checkpoint { handover: None } => {
                                let _ = followup_command
                                    .create_followup(
                                        followup_http.clone(),
                                        CreateInteractionResponseFollowup::new()
                                            .content("✅ lobotomy performed"),
                                    )
                                    .await;
                                break;
                            }
                            VizierResponseContent::Error { kind, message } => {
                                let kind_str = match kind {
                                    crate::schema::ErrorKind::Completion => "Completion Error",
                                    crate::schema::ErrorKind::ToolTimeout => "Tool Timeout",
                                    crate::schema::ErrorKind::PromptTimeout => "Prompt Timeout",
                                };
                                let _ = followup_command
                                    .create_followup(
                                        followup_http.clone(),
                                        CreateInteractionResponseFollowup::new().content(format!(
                                            "**{}**: {}",
                                            kind_str, message
                                        )),
                                    )
                                    .await;
                                break;
                            }
                            _ => {}
                        }
                    }
                });
            }

            if command.data.name == "thinking" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let key = format!("{}__{}", agent_id, channel.to_slug());
                let mut state = if let Ok(Some(value)) = self.1.storage.get_state(key.clone()).await {
                    serde_json::from_value::<ChannelState>(value).unwrap_or(ChannelState {
                        active_topic: None,
                        show_thinking: false,
                        show_tool_calls: false,
                    })
                } else {
                    ChannelState {
                        active_topic: None,
                        show_thinking: false,
                        show_tool_calls: false,
                    }
                };
                state.show_thinking = !state.show_thinking;
                let _ = self.1.storage.save_state(key, serde_json::to_value(&state).unwrap()).await;
                let status = if state.show_thinking { "ON" } else { "OFF" };
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content(format!("thinking output: **{}**", status)),
                        ),
                    )
                    .await;
            }

            if command.data.name == "tool_calls" {
                let channel = VizierChannelId::DiscordChanel(command.channel_id.get());
                let key = format!("{}__{}", agent_id, channel.to_slug());
                let mut state = if let Ok(Some(value)) = self.1.storage.get_state(key.clone()).await {
                    serde_json::from_value::<ChannelState>(value).unwrap_or(ChannelState {
                        active_topic: None,
                        show_thinking: false,
                        show_tool_calls: false,
                    })
                } else {
                    ChannelState {
                        active_topic: None,
                        show_thinking: false,
                        show_tool_calls: false,
                    }
                };
                state.show_tool_calls = !state.show_tool_calls;
                let _ = self.1.storage.save_state(key, serde_json::to_value(&state).unwrap()).await;
                let status = if state.show_tool_calls { "ON" } else { "OFF" };
                let _ = command
                    .create_response(
                        ctx.http.clone(),
                        serenity::all::CreateInteractionResponse::Message(
                            CreateInteractionResponseMessage::new()
                                .content(format!("tool call details: **{}**", status)),
                        ),
                    )
                    .await;
            }
        }
    }

    async fn message(&self, ctx: Context, msg: Message) {
        let agent_id = self.0.clone();

        let is_dm = msg.guild_id.is_none();
        let author_is_admin = admin_ids().contains(&msg.author.id.get());

        // DMs are for admins only. Anyone sharing a server with the bot can DM
        // it and there is no per-user rate limit, so an open inbox is an
        // unbounded way for one member to spend tokens - but admins need a
        // private channel to brief it without the whole server reading along.
        if is_dm && !author_is_admin {
            return;
        }

        // The channel allowlist governs server channels. A DM has no place on
        // that list, so it must not be filtered by it.
        if !is_dm {
            let allowed = allowed_channels();
            if !allowed.is_empty() && !allowed.contains(&msg.channel_id.get()) {
                return;
            }
        }

        // Paused by an admin: read nothing, store nothing, spend nothing.
        // Slash commands still work, so /resume can lift it.
        if is_paused(&self.1.storage, &agent_id).await {
            return;
        }

        // Kalesh watch. Runs on every message in the watched channels, before the
        // admin-only demotion, so it works while the bot is otherwise silent.
        // Stage one is free; only a burst that looks like a fight reaches the
        // model, and the cooldown is claimed before the call is made.
        if let Some(window) = note_message_and_check(&msg) {
            if let Some(role_id) = kalesh_role_id() {
                let http = ctx.http.clone();
                let channel_id = msg.channel_id;
                tokio::spawn(async move {
                    if let Some(line) = classify_kalesh(&window).await {
                        tracing::info!("kalesh detected in channel {}", channel_id.get());
                        let _ = crate::utils::discord::send_message(
                            http,
                            &channel_id,
                            format!("<@&{}> {}", role_id, line),
                        )
                        .await;
                    }
                });
            }
        }

        // Admin-only mode: keep reading and recording every monitored channel,
        // but answer nobody except the admins. Non-admin messages are demoted to
        // a silent read further down rather than dropped, so the moderator
        // history keeps building while the bot stays quiet to members.
        let silence_this_author = is_admin_only(&self.1.storage, &agent_id).await
            && !admin_ids().contains(&msg.author.id.get());

        let channel = VizierChannelId::DiscordChanel(msg.channel_id.get());

        let key = format!("{}__{}", agent_id, channel.to_slug());
        let (topic_id, show_thinking, show_tool_calls) = if let Ok(Some(value)) = self.1.storage.get_state(key).await {
            if let Ok(state) = serde_json::from_value::<ChannelState>(value) {
                (state.active_topic, state.show_thinking, state.show_tool_calls)
            } else {
                (None, false, false)
            }
        } else {
            (None, false, false)
        };

        // Never gate the whole handler on this call succeeding. It resolves the
        // bot's own user, over HTTP when the cache is cold, and an Err used to
        // skip every message silently - no reply, nothing logged. A DM is
        // addressed to us by definition, so it counts as a mention regardless.
        let is_mention = match msg.mentions_me(&ctx.http).await {
            Ok(m) => m || is_dm,
            Err(err) => {
                tracing::warn!("mentions_me failed, assuming {}: {:?}", is_dm, err);
                is_dm
            }
        };
        {
            let mut attachments = vec![];
            for attachment in &msg.attachments {
                let bytes_result = async {
                    let resp = reqwest::get(&attachment.url).await?;
                    resp.bytes().await
                }
                .await;
                if let Ok(bytes) = bytes_result {
                    if let Ok(file_record) = self
                        .1
                        .transport
                        .send_file_upload(attachment.filename.clone(), bytes.to_vec())
                        .await
                    {
                        attachments.push(VizierAttachment {
                            filename: attachment.filename.clone(),
                            content: VizierAttachmentContent::Local(file_record.url),
                        });
                    }
                }
            }

            // Tenor/Giphy links are not attachments, so resolve them to real
            // image bytes and feed them in as if they had been uploaded.
            for link in embedded_media_links(&msg.content) {
                if let Some((filename, bytes)) = fetch_embedded_media(&link).await {
                    if let Ok(file_record) =
                        self.1.transport.send_file_upload(filename.clone(), bytes).await
                    {
                        tracing::debug!("resolved embedded media from {}", link);
                        attachments.push(VizierAttachment {
                            filename,
                            content: VizierAttachmentContent::Local(file_record.url),
                        });
                    }
                }
            }

            let agent_id = self.0.clone();
            let transport = self.1.transport.clone();
            let file_manager = self.1.file_manager.clone();
            let http = ctx.http.clone();
            let bot_user_id = ctx.cache.current_user().id.get();
            // Both must happen before `referenced_message` is moved out of `msg`.
            let readable = humanise_mentions(&msg, bot_user_id);
            let roles = author_role_names(&ctx, &msg);
            let current_user = ctx.cache.current_user().discriminator;
            if msg.author.discriminator == current_user {
                return;
            }
            let bot_name = ctx.cache.current_user().name.clone();

            // Carry the quoted message's text, not just its id. Without this a
            // "@bot factcheck this" reply gives the model an id it would have to
            // go and fetch, and it usually just guesses instead.
            let (replied_to, replied_author, replied_content) = match msg.referenced_message {
                None => (None, None, None),
                Some(message) => (
                    Some(message.id.to_string()),
                    Some(message.author.display_name().to_string()),
                    Some(message.content.clone()),
                ),
            };

            let metadata = json!({
                "sender_roles": roles,
                "sent_at": Utc::now().to_string(),
                "is_reply_message": replied_to.is_some(),
                "replied_message_id": replied_to,
                "replied_message_author": replied_author,
                "replied_message_content": replied_content,
                "message_id": msg.id.to_string(),
                "discord_channel_id": msg.channel_id.to_string(),
                "is_dm": is_dm,
            });

            let session = VizierSession(
                agent_id.clone(),
                VizierChannelId::DiscordChanel(msg.channel_id.get()),
                topic_id,
            );

            // Put the quoted message inline, ahead of the request. It is also in
            // the metadata, but a weaker model does not reliably connect a
            // frontmatter field to the word "this" in "factcheck this".
            let readable = match (&replied_author, &replied_content) {
                (Some(a), Some(c)) if !c.trim().is_empty() => format!(
                    "[replying to @{}: \"{}\"]\n{}",
                    a,
                    c.replace('\n', " ").chars().take(1500).collect::<String>(),
                    readable
                ),
                _ => readable,
            };

            // Names, not ids - both for what we answer and for what we record,
            // so the stored history is searchable by name later too.
            let (content, request_content) = if silence_this_author || (!is_mention && !is_dm) {
                (
                    readable.clone(),
                    VizierRequestContent::SilentRead(readable),
                )
            } else {
                (readable.clone(), VizierRequestContent::Chat(readable))
            };

            let request = VizierRequest {
                timestamp: chrono::Utc::now(),
                user: format!(
                    "@{} (DiscordId: {})",
                    msg.author.display_name(),
                    msg.author.id.to_string()
                ),
                content: request_content,
                platform_message_id: Some(PlatformMessageId::Discord(msg.id.get())),
                metadata,
                attachments,
                ..Default::default()
            };

            let discord_channel_id = ChannelId::new(msg.channel_id.get());
            let is_chat = matches!(request.content, VizierRequestContent::Chat(_));

            tokio::spawn(async move {
                let (response_tx, response_rx) = flume::unbounded();

                if let Err(err) = transport
                    .send_request(session.clone(), request, Some(response_tx))
                    .await
                {
                    tracing::error!("{}", err);
                    return;
                }

                let mut typing_state: Option<Typing> = None;

                while let Ok(response) = response_rx.recv_async().await {
                    match response {
                        VizierResponse {
                            content: VizierResponseContent::ThinkingStart,
                            ..
                        } => {
                            typing_state = Some(Typing::start(http.clone(), discord_channel_id));
                        }
                        VizierResponse {
                            content: VizierResponseContent::ToolChoice { name, args },
                            ..
                        } => {
                            if show_tool_calls {
                                let _ = crate::utils::discord::send_message(
                                    http.clone(),
                                    &discord_channel_id,
                                    crate::utils::format_thinking(&name, &args),
                                )
                                .await;
                            }
                        }
                        VizierResponse {
                            content: VizierResponseContent::Thinking(thought),
                            ..
                        } => {
                            if show_thinking {
                                let _ = crate::utils::discord::send_message(
                                    http.clone(),
                                    &discord_channel_id,
                                    format!("> {}", thought),
                                )
                                .await;
                            }
                        }
                        VizierResponse {
                            content: VizierResponseContent::Message { content, stats: _ },
                            attachments,
                            ..
                        } => {
                            if let Some(typing) = typing_state.take() {
                                typing.stop();
                            }
                            let content = remove_think_tags(&content);
                            let _ = crate::utils::discord::send_message(
                                http.clone(),
                                &discord_channel_id,
                                content,
                            )
                            .await;

                            for attachment in &attachments {
                                match file_manager.resolve(attachment).await {
                                    Ok((filename, bytes)) => {
                                        let files = vec![CreateAttachment::bytes(bytes, &filename)];
                                        let builder = CreateMessage::new();
                                        if let Err(err) = discord_channel_id
                                            .send_files(&http, files, builder)
                                            .await
                                        {
                                            tracing::error!(
                                                "Failed to send attachment {}: {:?}",
                                                filename,
                                                err
                                            );
                                        }
                                    }
                                    Err(err) => {
                                        tracing::error!(
                                            "Failed to resolve attachment {:?}: {:?}",
                                            attachment.filename,
                                            err
                                        );
                                    }
                                }
                            }

                            break;
                        }
                        VizierResponse {
                            content: VizierResponseContent::AudioReply(audio_att, text, _),
                            ..
                        } => {
                            if let Some(typing) = typing_state.take() {
                                typing.stop();
                            }
                            if let Some(content) = text {
                                let content = remove_think_tags(&content);
                                let _ = crate::utils::discord::send_message(
                                    http.clone(),
                                    &discord_channel_id,
                                    content,
                                )
                                .await;
                            }
                            match file_manager.resolve(&audio_att).await {
                                Ok((filename, bytes)) => {
                                    let files = vec![CreateAttachment::bytes(bytes, &filename)];
                                    let builder = CreateMessage::new();
                                    if let Err(err) =
                                        discord_channel_id.send_files(&http, files, builder).await
                                    {
                                        tracing::error!("Failed to send audio reply: {:?}", err);
                                    }
                                }
                                Err(err) => {
                                    tracing::error!(
                                        "Failed to resolve audio reply {:?}: {:?}",
                                        audio_att.filename,
                                        err
                                    );
                                }
                            }

                            break;
                        }
                        VizierResponse {
                            content: VizierResponseContent::Abort,
                            ..
                        } => {
                            if let Some(typing) = typing_state.take() {
                                typing.stop();
                            }
                            let _ = crate::utils::discord::send_message(
                                http.clone(),
                                &discord_channel_id,
                                "thinking aborted".into(),
                            )
                            .await;

                            break;
                        }
                        VizierResponse {
                            content: VizierResponseContent::Error { kind, message },
                            ..
                        } => {
                            if let Some(typing) = typing_state.take() {
                                typing.stop();
                            }
                            let kind_str = match kind {
                                crate::schema::ErrorKind::Completion => "Completion Error",
                                crate::schema::ErrorKind::ToolTimeout => "Tool Timeout",
                                crate::schema::ErrorKind::PromptTimeout => "Prompt Timeout",
                            };
                            let _ = crate::utils::discord::send_message(
                                http.clone(),
                                &discord_channel_id,
                                format!("**{}**: {}", kind_str, message),
                            )
                            .await;

                            break;
                        }
                        _ => {
                            break;
                        }
                    }
                }

                if let Some(typing) = typing_state.take() {
                    typing.stop();
                }
            });
        }
    }
}
