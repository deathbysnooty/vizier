//! Member notes: a few bullet points about a member, answered at once and
//! without a model call. Two halves:
//!
//! * the facts (`notes_facts.rs`) - member since, messages, main channels,
//!   games and how they do, house and points, frog cards, voice, who they talk
//!   with, a favourite emoji and a phrase or two - read from the bot's own
//!   databases when someone asks;
//! * "what they're like" (`notes_build.rs`) - four to six bullets a cheap model
//!   writes from a sample of their own messages, built ahead of time by a
//!   weekly job and stored in `.runtime/notes.db` (`notes_store.rs`).
//!
//! Who may see them: a member their own, mods and admins anyone's, nobody else.
//! A mod looking someone else up is written to the audit trail. `/forgetme`
//! deletes a member's notes and keeps them out of every build until they opt
//! back in. The whole feature is off unless `VIZIER_NOTES` is on.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use rusqlite::{Connection, params};
use serenity::all::{
    ButtonStyle, CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction, Context, CreateActionRow,
    CreateAllowedMentions, CreateButton, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, GuildId, Message, Permissions, UserId,
};

use super::control;
use super::notes_build::{self as build, Reply, Said, Settings};
use super::notes_facts::{self as facts, Facts, Summary};
use super::notes_store as store;

/// Every model call member notes make, counted. The tests hold the answering
/// paths to zero.
pub static MODEL_CALLS: AtomicUsize = AtomicUsize::new(0);
/// How long one model call may take.
const MODEL_WAIT: Duration = Duration::from_secs(120);
/// A weekly run is due this long after the last.
const WEEK: i64 = 7 * 86_400;
/// While the first build is still covering everyone, a run a day.
const FIRST_BUILD_GAP: i64 = 86_400;
/// Newest messages of theirs read for a build, from the message log and then the stored history.
const LOG_MESSAGES: usize = 6000;
const HISTORY_MESSAGES: usize = 1500;

// --- settings --------------------------------------------------------------------------------

pub fn enabled() -> bool {
    control::on("VIZIER_NOTES", false)
}

/// The model that writes "what they're like": a model name on the bot's own
/// provider, the NPAT judge's cheap one by default, or the bot's usual model
/// with "agent".
pub fn model_name() -> Option<String> {
    match control::var("VIZIER_NOTES_MODEL") {
        None => Some(super::npat::DEFAULT_JUDGE_MODEL.to_string()),
        Some(v) if matches!(v.to_ascii_lowercase().as_str(), "agent" | "default" | "none") => None,
        Some(v) => Some(v),
    }
}

pub fn settings() -> Settings {
    Settings {
        min_messages: control::number("VIZIER_NOTES_MIN_MESSAGES", 100).clamp(20, 10_000) as usize,
        new_messages: control::number("VIZIER_NOTES_NEW_MESSAGES", 50).clamp(1, 10_000) as i64,
        max_per_run: control::number("VIZIER_NOTES_MAX_PER_RUN", 30).clamp(0, 1000) as usize,
        token_budget: control::number("VIZIER_NOTES_READ_BUDGET", 4000).clamp(500, 30_000) as usize,
    }
}

// --- who may see what --------------------------------------------------------------------------

/// A member may see their own notes; mods and admins anyone's; nobody else.
pub fn may_view(asker: u64, target: u64, asker_is_mod: bool) -> bool {
    asker == target || asker_is_mod
}

/// What an ask comes to, before anything is read.
#[derive(Debug, PartialEq)]
pub enum Decision {
    Denied,
    /// Allowed; `audit` when it's a mod looking at someone else.
    Allowed { audit: bool },
}

pub fn decide(asker: u64, target: u64, asker_is_mod: bool) -> Decision {
    if !may_view(asker, target, asker_is_mod) {
        Decision::Denied
    } else {
        Decision::Allowed { audit: asker != target }
    }
}

pub const DENIED: &str = "Sorry - member notes are private. You can look at your own (`/about` with nobody named), and \
    mods can look anyone up, but members can't look at each other's.";

const MOD_POWERS: Permissions = Permissions::ADMINISTRATOR
    .union(Permissions::MANAGE_GUILD)
    .union(Permissions::MANAGE_ROLES)
    .union(Permissions::MANAGE_MESSAGES)
    .union(Permissions::KICK_MEMBERS)
    .union(Permissions::BAN_MEMBERS)
    .union(Permissions::MODERATE_MEMBERS);

/// Whether the person using a command is a mod: the bot's admin list, the
/// permissions Discord sends with the command, or their roles in the cache.
/// When it can't be told, no.
fn command_is_mod(ctx: &Context, command: &CommandInteraction) -> bool {
    if super::admin_ids().contains(&command.user.id.get()) {
        return true;
    }
    let Some(member) = command.member.as_deref() else { return false };
    if member.permissions.is_some_and(|p| p.intersects(MOD_POWERS)) {
        return true;
    }
    command.guild_id.and_then(|g| ctx.cache.guild(g).map(|guild| super::house::has_mod_powers(&guild.roles, &member.roles))).unwrap_or(false)
}

fn message_is_mod(ctx: &Context, msg: &Message) -> bool {
    if super::admin_ids().contains(&msg.author.id.get()) {
        return true;
    }
    let (Some(guild_id), Some(member)) = (msg.guild_id, msg.member.as_ref()) else { return false };
    ctx.cache.guild(guild_id).map(|guild| super::house::has_mod_powers(&guild.roles, &member.roles)).unwrap_or(false)
}

// --- asking in chat ---------------------------------------------------------------------------

static ASK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)^\s*(?:(?:tell\s+me\s+about|tell\s+me\s+abt|who\s+is|who's|whos|about|abt)\s+(?:<@!?(\d+)>|(me|myself))",
        r"|<@!?(\d+)>\s+(?:kaun|kon|kaon)\s+(?:hai|h|he))\s*[?.!]*\s*$",
    ))
    .expect("regex")
});

/// "@Loduchand about @someone", "@Loduchand tell me about @someone", "@Loduchand
/// who is @someone" (or "about me"): the member being asked about. `None` for
/// anything else, which goes on to the model as before.
pub fn chat_target(content: &str, bot_id: u64, author: u64) -> Option<u64> {
    let bot = Regex::new(&format!(r"<@!?{}>", bot_id)).ok()?;
    if !bot.is_match(content) {
        return None;
    }
    let rest = bot.replace_all(content, " ");
    let caps = ASK.captures(&rest)?;
    if caps.get(2).is_some() {
        return Some(author);
    }
    let id: u64 = caps.get(1).or_else(|| caps.get(3))?.as_str().parse().ok()?;
    (id != bot_id).then_some(id)
}

/// The answer to one ask: the refusal, or the notes (and whether a lookup was
/// written down). Reads only the store and the facts it is handed - no model.
#[derive(Debug, PartialEq)]
pub struct Answer {
    pub allowed: bool,
    pub text: String,
    pub audited: bool,
}

pub struct Names<'a> {
    pub target: &'a str,
    pub channel: &'a dyn Fn(u64) -> Option<String>,
    pub member: &'a dyn Fn(u64) -> Option<String>,
}

/// What `/about` and the chat ask both come down to. The facts are only read
/// (through `facts_of`) once the ask is allowed.
pub fn respond(conn: &Connection, asker: u64, target: u64, asker_is_mod: bool, via: &str, now: i64, facts_of: &dyn Fn() -> Facts, names: &Names) -> Answer {
    let audit = match decide(asker, target, asker_is_mod) {
        Decision::Denied => return Answer { allowed: false, text: DENIED.to_string(), audited: false },
        Decision::Allowed { audit } => audit,
    };
    if audit {
        if let Err(err) = store::record_lookup(conn, asker, target, via, now) {
            tracing::warn!("notes: lookup of {} by {} not recorded: {}", target, asker, err);
        }
    }
    let note = store::get(conn, target);
    let summary = if store::opted_out(conn, target) {
        Summary::OptedOut
    } else if let Some(n) = &note {
        Summary::Note(n)
    } else if store::attempts(conn).get(&target).is_some_and(|(_, o)| o.starts_with("too little")) {
        Summary::TooLittle
    } else {
        Summary::NotYet
    };
    let f = facts_of();
    Answer { allowed: true, text: facts::render(names.target, asker == target, &f, summary, names.channel, names.member), audited: audit }
}

// --- reading the facts for real ------------------------------------------------------------------

fn guild_of(ctx: &Context) -> Option<GuildId> {
    ctx.cache.guilds().first().copied()
}

/// Name lookups from the cache, and every member's name as words (so a name is
/// never picked as someone's catchphrase).
pub(super) fn cache_names(ctx: &Context) -> (HashMap<u64, String>, HashMap<u64, String>, HashSet<String>) {
    let mut channels = HashMap::new();
    let mut members = HashMap::new();
    let mut words = HashSet::new();
    if let Some(guild) = guild_of(ctx).and_then(|g| ctx.cache.guild(g)) {
        for (id, c) in &guild.channels {
            channels.insert(id.get(), c.name.clone());
        }
        for (id, m) in &guild.members {
            let name = m.display_name().to_string();
            for n in [name.as_str(), m.user.name.as_str()] {
                for w in n.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
                    words.insert(w.to_string());
                }
            }
            if !m.user.bot {
                members.insert(id.get(), name);
            }
        }
    }
    (channels, members, words)
}

async fn joined(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64) -> Option<i64> {
    let from_cache = guild_of(ctx)
        .and_then(|g| ctx.cache.guild(g).and_then(|guild| guild.members.get(&UserId::new(user)).and_then(|m| m.joined_at.map(|t| t.unix_timestamp()))));
    let from_log = super::joinlog_get(storage, user)
        .await
        .first_join
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.timestamp());
    match (from_cache, from_log) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The facts about `user`, from the cache of the last few minutes or read now.
pub(super) async fn facts_for(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64, words: HashSet<String>) -> Facts {
    let now = Utc::now().timestamp();
    if let Some(f) = facts::cached(user, now) {
        return f;
    }
    let joined_at = joined(ctx, storage, user).await;
    let f = tokio::task::spawn_blocking(move || facts::read(user, now, joined_at, &words)).await.unwrap_or_default();
    facts::remember(user, now, &f);
    f
}

/// The full answer for `asker` about `target`, with the lookup written to both
/// the notes store and the panel's audit trail when a mod looks someone else up.
async fn answer(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, asker: u64, target: u64, is_mod: bool, target_name: &str, via: &str) -> Answer {
    let Some(db) = store::db() else {
        return Answer { allowed: true, text: "Member notes aren't available right now.".into(), audited: false };
    };
    if !may_view(asker, target, is_mod) {
        return Answer { allowed: false, text: DENIED.to_string(), audited: false };
    }
    let (channels, members, words) = cache_names(ctx);
    let f = facts_for(ctx, storage, target, words).await;
    let now = Utc::now().timestamp();
    let names = Names { target: target_name, channel: &|c| channels.get(&c).cloned(), member: &|u| members.get(&u).cloned() };
    let out = respond(&db.lock(), asker, target, is_mod, via, now, &|| f.clone(), &names);
    if out.audited {
        let _ = control::log_change(&format!("notes:lookup:{}", target), None, Some(&format!("{} · {}", target_name, via)), asker);
        tracing::info!("notes: {} looked up {} ({})", asker, target, via);
    }
    out
}

pub(super) fn display_name(ctx: &Context, user: u64, fallback: &str) -> String {
    guild_of(ctx)
        .and_then(|g| ctx.cache.guild(g).and_then(|guild| guild.members.get(&UserId::new(user)).map(|m| m.display_name().to_string())))
        .unwrap_or_else(|| fallback.to_string())
}

// --- /about ---------------------------------------------------------------------------------------

pub fn about_builder() -> CreateCommand {
    CreateCommand::new("about").description("what the bot knows about you - mods can look anyone up").add_option(CreateCommandOption::new(
        CommandOptionType::User,
        "member",
        "leave empty for yourself (only mods can name someone else)",
    ))
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
}

pub async fn about_command(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, command: &CommandInteraction) {
    if !enabled() {
        let _ = command.create_response(&ctx.http, whisper("Member notes aren't switched on.")).await;
        return;
    }
    let asker = command.user.id.get();
    let picked = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    });
    let target = picked.unwrap_or(asker);
    let is_mod = command_is_mod(ctx, command);
    // Refused before anything is read.
    if !may_view(asker, target, is_mod) {
        let _ = command.create_response(&ctx.http, whisper(DENIED)).await;
        return;
    }
    if command.data.resolved.users.get(&UserId::new(target)).is_some_and(|u| u.bot) {
        let _ = command.create_response(&ctx.http, whisper("Bots don't get notes.")).await;
        return;
    }
    let fallback = command
        .data
        .resolved
        .members
        .get(&UserId::new(target))
        .and_then(|m| m.nick.clone())
        .or_else(|| command.data.resolved.users.get(&UserId::new(target)).map(|u| u.display_name().to_string()))
        .unwrap_or_else(|| if target == asker { command.user.display_name().to_string() } else { "that member".into() });
    let name = display_name(ctx, target, &fallback);
    let _ = command.defer_ephemeral(&ctx.http).await;
    let out = answer(ctx, storage, asker, target, is_mod, &name, "/about").await;
    let _ = command
        .edit_response(&ctx.http, EditInteractionResponse::new().content(out.text).allowed_mentions(CreateAllowedMentions::new()))
        .await;
}

// --- the chat ask -------------------------------------------------------------------------------

/// "@Loduchand about @someone" in chat. True when the message was one of
/// these and has been dealt with, so it must go no further - never to the
/// model. The notes go to the asker's DMs: chat is public, the notes are not.
pub async fn on_message(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, msg: &Message) -> bool {
    if !enabled() || msg.guild_id.is_none() {
        return false;
    }
    let bot = ctx.cache.current_user().id.get();
    let asker = msg.author.id.get();
    let Some(target) = chat_target(&msg.content, bot, asker) else { return false };
    let is_mod = message_is_mod(ctx, msg);
    if !may_view(asker, target, is_mod) {
        let _ = msg.reply(&ctx.http, DENIED).await;
        return true;
    }
    if msg.mentions.iter().any(|u| u.id.get() == target && u.bot) {
        let _ = msg.reply(&ctx.http, "Bots don't get notes.").await;
        return true;
    }
    let fallback = msg.mentions.iter().find(|u| u.id.get() == target).map(|u| u.display_name().to_string()).unwrap_or_else(|| msg.author.display_name().to_string());
    let name = display_name(ctx, target, &fallback);
    let out = answer(ctx, storage, asker, target, is_mod, &name, "chat").await;
    let dm = CreateMessage::new().content(out.text).allowed_mentions(CreateAllowedMentions::new());
    let reply = match msg.author.direct_message(&ctx.http, dm).await {
        Ok(_) => "Sent to your DMs 📬 - notes are private, so they don't go in the channel.",
        Err(_) => "I couldn't DM you (your DMs look closed). Use `/about` for a private answer instead.",
    };
    let _ = msg.reply(&ctx.http, reply).await;
    true
}

// --- /forgetme -------------------------------------------------------------------------------------

pub fn forgetme_builder() -> CreateCommand {
    CreateCommand::new("forgetme").description("delete the bot's notes about you and keep you out of them - run it again to opt back in")
}

const OPTED_OUT: &str = "Done - your notes are deleted, and \"what you're like\" will never be built for you while you stay opted out. \
    Facts that are already public elsewhere (house points, frog cards, game records) can still show when you or a mod look you up.\n\
    -# Changed your mind? Press the button, or run `/forgetme` again.";
const OPTED_IN: &str = "You're back in. Your notes can be built again at the next weekly run, if you've chatted enough.";

fn opt_in_button(user: u64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![CreateButton::new(format!("notes:optin:{}", user)).label("Opt back in").style(ButtonStyle::Secondary)])
}

/// Flips a member's opt-out; true when they are now opted out.
pub fn toggle_opt_out(conn: &mut Connection, user: u64, now: i64) -> rusqlite::Result<bool> {
    if store::opted_out(conn, user) {
        store::opt_in(conn, user)?;
        Ok(false)
    } else {
        store::opt_out(conn, user, now)?;
        Ok(true)
    }
}

pub async fn forgetme_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("That isn't available right now - try again later.")).await;
        return;
    };
    let result = toggle_opt_out(&mut db.lock(), user, Utc::now().timestamp());
    let response = match result {
        Ok(true) => {
            let _ = control::log_change(&format!("notes:optout:{}", user), None, Some("opted out with /forgetme"), user);
            CreateInteractionResponseMessage::new().content(OPTED_OUT).ephemeral(true).components(vec![opt_in_button(user)])
        }
        Ok(false) => {
            let _ = control::log_change(&format!("notes:optout:{}", user), Some("opted out"), Some("opted back in with /forgetme"), user);
            CreateInteractionResponseMessage::new().content(OPTED_IN).ephemeral(true)
        }
        Err(err) => {
            tracing::warn!("notes: /forgetme for {} failed: {}", user, err);
            CreateInteractionResponseMessage::new().content("That didn't work - try again in a minute.").ephemeral(true)
        }
    };
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(response)).await;
}

/// The "Opt back in" button under a `/forgetme` reply.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let Some(owner) = component.data.custom_id.strip_prefix("notes:optin:").and_then(|id| id.parse::<u64>().ok()) else { return };
    let user = component.user.id.get();
    let text = if owner != user {
        "That button is someone else's.".to_string()
    } else {
        match store::db().map(|db| store::opt_in(&db.lock(), user)) {
            Some(Ok(_)) => {
                let _ = control::log_change(&format!("notes:optout:{}", user), Some("opted out"), Some("opted back in with the button"), user);
                OPTED_IN.to_string()
            }
            _ => "That didn't work - try again in a minute.".to_string(),
        }
    };
    let update = CreateInteractionResponseMessage::new().content(text).components(vec![]);
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
}

// --- building for real -------------------------------------------------------------------------------

/// One call to the notes model, with what it cost. Providers that don't report
/// usage get an estimate from the text.
pub async fn ask_live(prompt: String) -> anyhow::Result<Reply> {
    use crate::agents::agent::model::{VizierModel, VizierModelTrait};
    use crate::storage::agent::AgentStorage;
    use rig_core::message::{AssistantContent, Message as ModelMessage};

    MODEL_CALLS.fetch_add(1, Ordering::SeqCst);
    let (deps, agent_id) = control::web::bot_agent().ok_or_else(|| anyhow::anyhow!("the agent isn't reachable"))?;
    let config = deps.storage.get_agent(agent_id).await?.ok_or_else(|| anyhow::anyhow!("no config for {}", agent_id))?;
    let name = model_name();
    let named = name.clone().map(|n| (config.provider.clone(), n));
    let model = VizierModel::new_with_override(deps, &config, named).await?;
    let prompt_tokens = build::estimate_tokens(&prompt) as u64;
    let (_, choice, usage) = tokio::time::timeout(MODEL_WAIT, model.completion(ModelMessage::user(prompt), vec![], vec![]))
        .await
        .map_err(|_| anyhow::anyhow!("the model took over {}s", MODEL_WAIT.as_secs()))??;
    let text: String = choice
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let (input_tokens, output_tokens) = if usage.input_tokens + usage.output_tokens > 0 {
        (usage.input_tokens, usage.output_tokens)
    } else {
        (prompt_tokens, build::estimate_tokens(&text) as u64)
    };
    Ok(Reply { text, input_tokens, output_tokens, model: name.unwrap_or(config.model) })
}

/// A member's own messages for a build: the message log first (every channel
/// but #safe-corner), then the older stored history. Blocking.
pub fn load_messages(user: u64) -> Vec<Said> {
    let sensitive = control::insights::sensitive_channels();
    let mut out: Vec<Said> = Vec::new();
    let mut oldest_ms = i64::MAX;
    if let Some(reader) = super::msglog::reader() {
        let conn = reader.conn.lock();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT content, created_ts, channel_id, parent_id FROM recent WHERE author_id = ?1 ORDER BY message_id DESC LIMIT ?2",
        ) {
            if let Ok(rows) = stmt.query_map(params![user as i64, LOG_MESSAGES as i64], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)? as u64, r.get::<_, Option<i64>>(3)?.map(|p| p as u64)))
            }) {
                for (text, ts_ms, channel, parent) in rows.flatten() {
                    oldest_ms = oldest_ms.min(ts_ms);
                    if sensitive.contains(&channel) || parent.is_some_and(|p| sensitive.contains(&p)) {
                        continue;
                    }
                    out.push(Said { ts: ts_ms / 1000, text });
                }
            }
        }
    }
    for (ts, channel, text) in control::web::stored_messages(user, oldest_ms, HISTORY_MESSAGES) {
        if channel.is_some_and(|c| sensitive.contains(&c)) {
            continue;
        }
        out.push(Said { ts, text });
    }
    out
}

/// Every member's messages on record, all time: the larger of the chat counts
/// and the message log's count, so the log's reach and the counts' reach both show.
fn message_totals() -> HashMap<u64, i64> {
    let mut totals: HashMap<u64, i64> = HashMap::new();
    if let Some(db) = super::stats::db() {
        let conn = db.lock();
        if let Ok(mut stmt) = conn.prepare("SELECT user_id, SUM(count) FROM msg_counts GROUP BY user_id") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?))) {
                totals.extend(rows.flatten());
            }
        }
    }
    if let Some(reader) = super::msglog::reader() {
        let conn = reader.conn.lock();
        if let Ok(mut stmt) = conn.prepare("SELECT author_id, COUNT(*) FROM recent GROUP BY author_id") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?))) {
                for (u, n) in rows.flatten() {
                    let t = totals.entry(u).or_default();
                    *t = (*t).max(n);
                }
            }
        }
    }
    totals
}

/// Their messages in the log since `since` (seconds).
fn messages_since(user: u64, since: i64) -> i64 {
    let Some(reader) = super::msglog::reader() else { return 0 };
    reader
        .conn
        .lock()
        .query_row(
            "SELECT COUNT(*) FROM recent WHERE author_id = ?1 AND message_id >= ?2",
            params![user as i64, super::msglog::first_id_at(since * 1000) as i64],
            |r| r.get(0),
        )
        .unwrap_or(0)
}

/// Everyone who could be built, from the counts, leaving out bots and anyone
/// no longer in the server (when the cache knows the server). Blocking.
fn live_candidates(members: &Option<HashSet<u64>>) -> Vec<build::Candidate> {
    let Some(db) = store::db() else { return Vec::new() };
    let (built, attempts) = {
        let conn = db.lock();
        (store::built_at(&conn), store::attempts(&conn))
    };
    let mut totals = message_totals();
    if let Some(m) = members {
        totals.retain(|u, _| m.contains(u));
    }
    build::candidates(&totals, &built, &messages_since, &attempts)
}

/// Members in the server who aren't bots, from the cache; `None` if it doesn't know.
fn server_members(ctx: &Context) -> Option<HashSet<u64>> {
    let guild = guild_of(ctx).and_then(|g| ctx.cache.guild(g))?;
    let ids: HashSet<u64> = guild.members.iter().filter(|(_, m)| !m.user.bot).map(|(id, _)| id.get()).collect();
    (!ids.is_empty()).then_some(ids)
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// One scheduled run: first builds while the first build is still covering
/// everyone, then weekly refreshes, within the cap.
pub async fn run_scheduled(ctx: &Context, kind: &str) -> Option<store::Run> {
    let db = store::db()?;
    if RUNNING.swap(true, Ordering::SeqCst) {
        return None;
    }
    let s = settings();
    let now = Utc::now().timestamp();
    let members = server_members(ctx);
    let cands = tokio::task::spawn_blocking(move || live_candidates(&members)).await.unwrap_or_default();
    let optouts = store::optouts(&db.lock());
    let (picked, waiting) = build::pick(&cands, &optouts, &s, now, kind == "first");
    if kind == "first" {
        let (n, input, output) = build::first_build_estimate(&cands, &optouts, &s);
        tracing::info!(
            "notes: a full first build covers {} member(s) and costs at most about {} tokens in + {} out; {} per run",
            n,
            input,
            output,
            s.max_per_run
        );
    }
    let users: Vec<u64> = picked.iter().map(|(u, _)| *u).collect();
    let report = build::run(
        db,
        kind,
        &users,
        waiting,
        &s,
        0,
        now,
        |u| async move { tokio::task::spawn_blocking(move || load_messages(u)).await.unwrap_or_default() },
        ask_live,
    )
    .await;
    if kind == "first" && waiting == 0 {
        let _ = store::meta_set(&db.lock(), "first_done", &now.to_string());
        tracing::info!("notes: the first build has covered everyone who qualifies");
    }
    RUNNING.store(false, Ordering::SeqCst);
    Some(report)
}

/// A mod's "Rebuild now" on the panel: this member only, whatever the new
/// messages say, but never for someone who opted out or has too little to go on.
pub async fn rebuild_now(user: u64, by: u64) -> Result<store::Note, String> {
    let db = store::db().ok_or("the notes store isn't open")?;
    if store::opted_out(&db.lock(), user) {
        return Err("They've opted out with /forgetme, so no notes are built for them.".into());
    }
    let s = settings();
    let now = Utc::now().timestamp();
    let messages = tokio::task::spawn_blocking(move || load_messages(user)).await.unwrap_or_default();
    let report = build::run(db, "manual", &[user], 0, &s, by, now, |_| std::future::ready(messages.clone()), ask_live).await;
    let conn = db.lock();
    match store::get(&conn, user) {
        Some(n) if n.built_ts == now => Ok(n),
        _ if report.skipped > 0 => Err(format!("Too little to go on: fewer than {} usable messages of theirs.", s.min_messages)),
        _ if report.asked > 0 => Err("The model answered, but every bullet it wrote was thrown away by the filter.".into()),
        _ => Err("The model call failed - see the log.".into()),
    }
}

/// What a full first build would cost now, roughly: (members, tokens in, tokens out).
pub async fn estimate() -> (usize, i64, i64) {
    let members = control::web::context().and_then(server_members);
    let s = settings();
    let optouts = store::db().map(|db| store::optouts(&db.lock())).unwrap_or_default();
    let cands = tokio::task::spawn_blocking(move || live_candidates(&members)).await.unwrap_or_default();
    build::first_build_estimate(&cands, &optouts, &s)
}

/// The answer as `/about` would give it, for the panel's member page. A panel
/// admin looking at it is a lookup like any other and is written down.
pub async fn panel_view(target: u64, by: u64) -> Option<String> {
    let ctx = control::web::context()?;
    let (deps, _) = control::web::bot_agent()?;
    let name = display_name(ctx, target, &target.to_string());
    Some(answer(ctx, &deps.storage, by, target, true, &name, "panel").await.text)
}

/// Which run is due, if any: first builds a day apart until everyone is
/// covered, then a weekly refresh.
pub fn due(now: i64, last_run: Option<i64>, first_done: bool) -> Option<&'static str> {
    let gap = if first_done { WEEK } else { FIRST_BUILD_GAP };
    match last_run {
        Some(t) if now - t < gap => None,
        _ => Some(if first_done { "weekly" } else { "first" }),
    }
}

/// Checks every half hour whether a run is due. Starts once per process.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(180)).await;
        loop {
            if enabled() && settings().max_per_run > 0 {
                if let Some(db) = store::db() {
                    let now = Utc::now().timestamp();
                    // Claimed before the run, so one that crashes the task can't repeat every half hour.
                    let kind = {
                        let conn = db.lock();
                        let last = store::meta_get(&conn, "last_run").and_then(|v| v.parse().ok());
                        let first_done = store::meta_get(&conn, "first_done").is_some();
                        let kind = due(now, last, first_done);
                        if kind.is_some() {
                            let _ = store::meta_set(&conn, "last_run", &now.to_string());
                        }
                        kind
                    };
                    if let Some(kind) = kind {
                        run_scheduled(&ctx, kind).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1800)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOT: u64 = 900_000_000_000_000_001;
    const ME: u64 = 111_111_111_111_111_111;
    const YOU: u64 = 222_222_222_222_222_222;

    fn names() -> (Box<dyn Fn(u64) -> Option<String>>, Box<dyn Fn(u64) -> Option<String>>) {
        (Box::new(|_| Some("general".into())), Box::new(|u| Some(format!("m{}", u % 10))))
    }

    fn some_facts() -> Facts {
        Facts { messages_all: 900, messages_month: 40, frog_cards: 3, ..Default::default() }
    }

    #[test]
    fn a_member_sees_their_own_a_mod_sees_anyone_and_nobody_else_does() {
        assert!(may_view(ME, ME, false));
        assert!(!may_view(ME, YOU, false));
        assert!(may_view(ME, YOU, true));
        assert_eq!(decide(ME, ME, false), Decision::Allowed { audit: false });
        assert_eq!(decide(ME, YOU, false), Decision::Denied);
        assert_eq!(decide(ME, YOU, true), Decision::Allowed { audit: true });
        assert_eq!(decide(ME, ME, true), Decision::Allowed { audit: false }, "a mod looking at themselves isn't a lookup");
    }

    #[test]
    fn the_chat_ask_is_recognised_and_other_messages_are_left_alone() {
        let at = |id: u64| format!("<@{}>", id);
        assert_eq!(chat_target(&format!("{} about {}", at(BOT), at(YOU)), BOT, ME), Some(YOU));
        assert_eq!(chat_target(&format!("{} tell me about <@!{}>?", at(BOT), YOU), BOT, ME), Some(YOU));
        assert_eq!(chat_target(&format!("{} who is {}", at(BOT), at(YOU)), BOT, ME), Some(YOU));
        assert_eq!(chat_target(&format!("{} {} kaun hai", at(BOT), at(YOU)), BOT, ME), Some(YOU));
        assert_eq!(chat_target(&format!("{} about me", at(BOT)), BOT, ME), Some(ME));
        assert_eq!(chat_target(&format!("Who is {} {}", at(YOU), at(BOT)), BOT, ME), Some(YOU));
        for pass in [
            format!("about {}", at(YOU)),                                 // the bot isn't asked
            format!("{} what do you think about {}", at(BOT), at(YOU)),  // a real question for the model
            format!("{} about {} and cricket", at(BOT), at(YOU)),
            format!("{} about", at(BOT)),
            format!("{} who is {}", at(BOT), at(BOT)),
        ] {
            assert_eq!(chat_target(&pass, BOT, ME), None, "{:?}", pass);
        }
    }

    #[test]
    fn answering_reads_the_store_and_facts_only_never_the_model() {
        let conn = store::memory();
        let (channel, member) = names();
        let n = Names { target: "You", channel: &*channel, member: &*member };
        let before = MODEL_CALLS.load(Ordering::SeqCst);
        // Self, other member, mod - over the command and the chat ask alike.
        for via in ["/about", "chat"] {
            let own = respond(&conn, ME, ME, false, via, 10, &some_facts, &n);
            assert!(own.allowed && own.text.contains("**The facts**"));
            let refused = respond(&conn, ME, YOU, false, via, 10, &|| panic!("facts read for a refused ask"), &n);
            assert_eq!(refused, Answer { allowed: false, text: DENIED.to_string(), audited: false });
            let by_mod = respond(&conn, ME, YOU, true, via, 10, &some_facts, &n);
            assert!(by_mod.allowed && by_mod.audited);
        }
        assert_eq!(MODEL_CALLS.load(Ordering::SeqCst), before, "no model call on any answering path");
    }

    #[test]
    fn a_mod_lookup_is_written_down_and_a_self_lookup_is_not() {
        let conn = store::memory();
        let (channel, member) = names();
        let n = Names { target: "Them", channel: &*channel, member: &*member };
        respond(&conn, ME, ME, true, "/about", 5, &some_facts, &n);
        respond(&conn, ME, YOU, false, "/about", 6, &some_facts, &n);
        assert!(store::lookups(&conn, None, 10).is_empty(), "neither a self-lookup nor a refused one is a lookup");
        respond(&conn, ME, YOU, true, "chat", 7, &some_facts, &n);
        assert_eq!(store::lookups(&conn, Some(YOU), 10), vec![store::Lookup { ts: 7, asker: ME, target: YOU, via: "chat".into() }]);
    }

    #[test]
    fn the_answer_says_why_there_is_no_summary() {
        let mut conn = store::memory();
        let (channel, member) = names();
        let n = Names { target: "Them", channel: &*channel, member: &*member };
        let text = respond(&conn, ME, ME, false, "/about", 5, &some_facts, &n).text;
        assert!(text.contains("written once a week"));
        store::note_attempt(&conn, ME, 5, "too little: 12 usable messages").unwrap();
        assert!(respond(&conn, ME, ME, false, "/about", 5, &some_facts, &n).text.contains("isn't enough"));
        assert!(toggle_opt_out(&mut conn, ME, 6).unwrap());
        let text = respond(&conn, ME, ME, false, "/about", 7, &some_facts, &n).text;
        assert!(text.contains("opted out") && text.contains("public elsewhere"));
        assert!(!toggle_opt_out(&mut conn, ME, 8).unwrap(), "running it again opts back in");
        assert!(!store::opted_out(&conn, ME));
    }

    #[test]
    fn runs_are_due_daily_while_the_first_build_covers_everyone_then_weekly() {
        assert_eq!(due(1000, None, false), Some("first"));
        assert_eq!(due(1000 + FIRST_BUILD_GAP - 1, Some(1000), false), None);
        assert_eq!(due(1000 + FIRST_BUILD_GAP, Some(1000), false), Some("first"));
        assert_eq!(due(1000 + FIRST_BUILD_GAP, Some(1000), true), None);
        assert_eq!(due(1000 + WEEK, Some(1000), true), Some("weekly"));
    }
}
