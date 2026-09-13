//! Auto-responses: rules made on the panel that answer or react to messages
//! containing certain words. The store, the matching, and the hook the message
//! handler calls.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::{Mutex, RwLock};
use regex::Regex;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serenity::all::{
    ChannelId, Context, CreateAllowedMentions, CreateMessage, EmojiId, Message, ReactionType,
};

use super::DB;

/// How a trigger is matched against a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Match {
    /// Anywhere in the message, even inside a longer word.
    Contains,
    /// As a whole word or phrase: "hi" matches "hi there" but not "this".
    #[default]
    WholeWord,
    /// The whole message is the trigger (spaces around it ignored).
    Exact,
    StartsWith,
    /// A regular expression.
    Pattern,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoReply {
    /// 0 for one not saved yet.
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub enabled: bool,
    /// Any of these sets the rule off.
    pub triggers: Vec<String>,
    #[serde(default)]
    pub match_mode: Match,
    #[serde(default)]
    pub case_sensitive: bool,
    /// Where the rule works; empty means every channel the bot can read.
    /// Discord ids travel as strings.
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub exclude_channels: Vec<String>,
    /// One is picked at random; empty sends no message. Placeholders:
    /// `{user}` (mentions the author) and `{name}` (their display name).
    #[serde(default)]
    pub replies: Vec<String>,
    /// Reply to the message (true) or just post in the channel.
    #[serde(default = "yes")]
    pub as_reply: bool,
    /// Unicode emoji ("🔥") or custom ones ("<:name:id>" or "name:id").
    #[serde(default)]
    pub reactions: Vec<String>,
    /// Percent of matching messages that get a response, 1..=100.
    #[serde(default = "hundred")]
    pub chance: u8,
    /// After responding in a channel, stay quiet there this long.
    #[serde(default)]
    pub cooldown_secs: u32,
    /// Kept by the bot.
    #[serde(default)]
    pub hits: i64,
    #[serde(default)]
    pub last_hit: i64,
}

fn yes() -> bool {
    true
}

fn hundred() -> u8 {
    100
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS autoreplies (
        id INTEGER PRIMARY KEY AUTOINCREMENT, body TEXT NOT NULL, updated_by INTEGER NOT NULL, updated_ts INTEGER NOT NULL);
";

pub(super) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)
}

/// Enabled rules with their compiled matchers, rebuilt whenever a rule is saved.
struct Compiled {
    rule: AutoReply,
    matchers: Vec<Regex>,
}

static RULES: LazyLock<RwLock<Option<Vec<Compiled>>>> = LazyLock::new(|| RwLock::new(None));
/// Last response per (rule, channel), for cooldowns.
static COOLDOWN: LazyLock<Mutex<HashMap<(i64, u64), Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Hit counts not yet written back, so a busy rule doesn't write on every message.
static PENDING_HITS: LazyLock<Mutex<HashMap<i64, (i64, i64)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// --- store ------------------------------------------------------------------

pub fn list() -> Vec<AutoReply> {
    flush_hits();
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT id, body FROM autoreplies ORDER BY id") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| {
            rows.flatten()
                .filter_map(|(id, body)| serde_json::from_str::<AutoReply>(&body).ok().map(|r| AutoReply { id, ..r }))
                .collect()
        })
        .unwrap_or_default()
}

pub fn get(id: i64) -> Option<AutoReply> {
    let db = DB.get()?;
    let body: String = db
        .lock()
        .query_row("SELECT body FROM autoreplies WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    serde_json::from_str::<AutoReply>(&body).ok().map(|r| AutoReply { id, ..r })
}

/// Checks a rule before it is saved; the message says what to fix.
pub fn validate(rule: &AutoReply) -> Result<(), String> {
    if rule.name.trim().is_empty() {
        return Err("Give the rule a name.".into());
    }
    let triggers: Vec<&String> = rule.triggers.iter().filter(|t| !t.trim().is_empty()).collect();
    if triggers.is_empty() {
        return Err("Add at least one trigger word or phrase.".into());
    }
    if rule.replies.iter().all(|r| r.trim().is_empty()) && rule.reactions.iter().all(|r| r.trim().is_empty()) {
        return Err("Add a reply or a reaction, or the rule does nothing.".into());
    }
    if rule.match_mode == Match::Pattern {
        for t in &triggers {
            if let Err(err) = Regex::new(t) {
                return Err(format!("\"{}\" isn't a valid pattern: {}", t, err));
            }
        }
    }
    if rule.replies.iter().any(|r| r.chars().count() > 1800) {
        return Err("Replies must be under 1800 characters.".into());
    }
    if rule.reactions.iter().filter(|r| !r.trim().is_empty()).any(|r| reaction(r).is_none()) {
        return Err("A reaction isn't an emoji the bot can use.".into());
    }
    if !(1..=100).contains(&rule.chance) {
        return Err("Chance must be between 1 and 100%.".into());
    }
    Ok(())
}

/// Saves a rule, new when its id is 0, and returns its id. The change goes to
/// the audit trail under `autoreply:<id>` unless `by` is 0 (the bot itself).
pub fn save(rule: &AutoReply, by: u64) -> anyhow::Result<i64> {
    validate(rule).map_err(|e| anyhow::anyhow!(e))?;
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let id = {
        let conn = db.lock();
        let now = Utc::now().timestamp();
        let body = serde_json::to_string(rule)?;
        let (id, old) = if rule.id == 0 {
            conn.execute(
                "INSERT INTO autoreplies (body, updated_by, updated_ts) VALUES (?1, ?2, ?3)",
                params![body, by as i64, now],
            )?;
            (conn.last_insert_rowid(), None)
        } else {
            let old: Option<String> = conn
                .query_row("SELECT body FROM autoreplies WHERE id = ?1", params![rule.id], |r| r.get(0))
                .optional()?;
            if old.is_none() {
                anyhow::bail!("no auto-response {}", rule.id);
            }
            conn.execute(
                "UPDATE autoreplies SET body = ?1, updated_by = ?2, updated_ts = ?3 WHERE id = ?4",
                params![body, by as i64, now, rule.id],
            )?;
            (rule.id, old)
        };
        if by != 0 {
            conn.execute(
                "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![now, by as i64, format!("autoreply:{}", id), old, body],
            )?;
        }
        id
    };
    *RULES.write() = None;
    Ok(id)
}

pub fn delete(id: i64, by: u64) -> anyhow::Result<()> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    {
        let conn = db.lock();
        let old: Option<String> =
            conn.query_row("SELECT body FROM autoreplies WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
        conn.execute("DELETE FROM autoreplies WHERE id = ?1", params![id])?;
        conn.execute(
            "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, NULL)",
            params![Utc::now().timestamp(), by as i64, format!("autoreply:{}", id), old],
        )?;
    }
    PENDING_HITS.lock().remove(&id);
    *RULES.write() = None;
    Ok(())
}

// --- matching ----------------------------------------------------------------

/// The regular expression a trigger becomes under a match mode.
fn matcher(trigger: &str, mode: Match, case_sensitive: bool) -> Option<Regex> {
    let t = trigger.trim();
    if t.is_empty() {
        return None;
    }
    let flags = if case_sensitive { "" } else { "(?i)" };
    let escaped = regex::escape(t);
    let body = match mode {
        Match::Contains => escaped,
        // Word edges that also work for emoji and punctuation at either end.
        Match::WholeWord => format!(r"(?:^|[^\p{{L}}\p{{N}}_]){}(?:$|[^\p{{L}}\p{{N}}_])", escaped),
        Match::Exact => format!(r"^\s*{}\s*$", escaped),
        Match::StartsWith => format!(r"^\s*{}", escaped),
        Match::Pattern => t.to_string(),
    };
    Regex::new(&format!("{}{}", flags, body)).ok()
}

fn compile(rule: AutoReply) -> Compiled {
    let matchers = rule.triggers.iter().filter_map(|t| matcher(t, rule.match_mode, rule.case_sensitive)).collect();
    Compiled { rule, matchers }
}

/// Whether a rule applies in a channel (and its parent, for threads).
fn in_scope(rule: &AutoReply, channel: u64, parent: Option<u64>) -> bool {
    let has = |list: &[String], id: u64| list.iter().any(|c| c.trim().parse::<u64>().ok() == Some(id));
    let here = |list: &[String]| has(list, channel) || parent.is_some_and(|p| has(list, p));
    if here(&rule.exclude_channels) {
        return false;
    }
    rule.channels.iter().all(|c| c.trim().is_empty()) || here(&rule.channels)
}

/// A reaction as Discord wants it: a unicode emoji, or a custom one written
/// `<:name:id>`, `<a:name:id>` or `name:id`.
fn reaction(text: &str) -> Option<ReactionType> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let inner = t.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(t);
    let (animated, inner) = match inner.strip_prefix("a:") {
        Some(rest) => (true, rest),
        None => (false, inner.strip_prefix(':').unwrap_or(inner)),
    };
    if let Some((name, id)) = inner.rsplit_once(':') {
        if let Ok(id) = id.parse::<u64>() {
            return Some(ReactionType::Custom { animated, id: EmojiId::new(id), name: Some(name.to_string()) });
        }
    }
    // Plain words aren't emoji; anything with a non-ASCII character is taken as one.
    (!t.is_ascii() && t.chars().count() <= 12).then(|| ReactionType::Unicode(t.to_string()))
}

fn fill(reply: &str, author: u64, name: &str) -> String {
    reply.replace("{user}", &format!("<@{}>", author)).replace("{name}", name)
}

// --- the hook ------------------------------------------------------------------

/// Called for every human message in a server. Never blocks the handler: any
/// reply or reaction is sent from its own task.
pub fn on_message(ctx: &Context, msg: &Message) {
    if msg.author.bot || msg.guild_id.is_none() || msg.content.trim().is_empty() {
        return;
    }
    if RULES.read().is_none() {
        let compiled: Vec<Compiled> = list().into_iter().filter(|r| r.enabled).map(compile).collect();
        *RULES.write() = Some(compiled);
    }
    let channel = msg.channel_id.get();
    let parent = msg.guild_id.and_then(|g| ctx.cache.guild(g)).and_then(|g| {
        g.threads.iter().find(|t| t.id.get() == channel).and_then(|t| t.parent_id).map(|p| p.get())
    });
    let now = Instant::now();
    let fired: Vec<AutoReply> = {
        let rules = RULES.read();
        let Some(rules) = rules.as_ref() else {
            return;
        };
        let mut cooldown = COOLDOWN.lock();
        let mut fired = Vec::new();
        for c in rules.iter() {
            if !in_scope(&c.rule, channel, parent) || !c.matchers.iter().any(|m| m.is_match(&msg.content)) {
                continue;
            }
            let wait = Duration::from_secs(c.rule.cooldown_secs as u64);
            if cooldown.get(&(c.rule.id, channel)).is_some_and(|at| now.duration_since(*at) < wait) {
                continue;
            }
            if c.rule.chance < 100 && rand::random::<f64>() * 100.0 >= c.rule.chance as f64 {
                continue;
            }
            cooldown.insert((c.rule.id, channel), now);
            fired.push(c.rule.clone());
        }
        fired
    };
    for rule in fired {
        count_hit(rule.id);
        let ctx = ctx.clone();
        let (channel_id, message_id, author) = (msg.channel_id, msg.id, msg.author.id.get());
        let name = msg.member.as_ref().and_then(|m| m.nick.clone()).or_else(|| msg.author.global_name.clone()).unwrap_or_else(|| msg.author.name.clone());
        tokio::spawn(async move {
            for emoji in rule.reactions.iter().filter_map(|r| reaction(r)) {
                if let Err(err) = channel_id.create_reaction(&ctx.http, message_id, emoji).await {
                    tracing::warn!("autoreply: '{}' could not react: {}", rule.name, err);
                }
            }
            let options: Vec<&String> = rule.replies.iter().filter(|r| !r.trim().is_empty()).collect();
            if options.is_empty() {
                return;
            }
            let pick = options[(rand::random::<f64>() * options.len() as f64) as usize % options.len()];
            let mut out = CreateMessage::new()
                .content(fill(pick, author, &name))
                .allowed_mentions(CreateAllowedMentions::new().users(vec![author]).replied_user(false));
            if rule.as_reply {
                out = out.reference_message((channel_id, message_id));
            }
            if let Err(err) = ChannelId::new(channel_id.get()).send_message(&ctx.http, out).await {
                tracing::warn!("autoreply: '{}' could not reply: {}", rule.name, err);
            }
        });
    }
}

fn count_hit(id: i64) {
    let now = Utc::now().timestamp();
    let mut pending = PENDING_HITS.lock();
    let entry = pending.entry(id).or_insert((0, now));
    entry.0 += 1;
    entry.1 = now;
    let total: i64 = pending.values().map(|(n, _)| n).sum();
    drop(pending);
    if total >= 20 {
        flush_hits();
    }
}

/// Writes counted hits back to the rules. Called before listing and every 20 hits.
fn flush_hits() {
    let pending: HashMap<i64, (i64, i64)> = std::mem::take(&mut *PENDING_HITS.lock());
    if pending.is_empty() {
        return;
    }
    let Some(db) = DB.get() else {
        return;
    };
    let conn = db.lock();
    for (id, (hits, last)) in pending {
        let body: Option<String> =
            conn.query_row("SELECT body FROM autoreplies WHERE id = ?1", params![id], |r| r.get(0)).optional().ok().flatten();
        let Some(mut rule) = body.and_then(|b| serde_json::from_str::<AutoReply>(&b).ok()) else {
            continue;
        };
        rule.hits += hits;
        rule.last_hit = last;
        if let Ok(body) = serde_json::to_string(&rule) {
            let _ = conn.execute("UPDATE autoreplies SET body = ?1 WHERE id = ?2", params![body, id]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(trigger: &str, mode: Match, text: &str) -> bool {
        matcher(trigger, mode, false).is_some_and(|m| m.is_match(text))
    }

    #[test]
    fn match_modes_behave() {
        assert!(hit("hi", Match::WholeWord, "Hi there"));
        assert!(!hit("hi", Match::WholeWord, "this is it"));
        assert!(hit("good night", Match::WholeWord, "ok good night all"));
        assert!(hit("gn", Match::WholeWord, "gn!"));
        assert!(hit("🔥", Match::WholeWord, "that was 🔥 bro"));
        assert!(hit("hi", Match::Contains, "this"));
        assert!(hit("gm", Match::Exact, "  GM "));
        assert!(!hit("gm", Match::Exact, "gm all"));
        assert!(hit("bhai", Match::StartsWith, "Bhai sun"));
        assert!(!hit("bhai", Match::StartsWith, "sun bhai"));
        assert!(hit(r"\bl+o+l+\b", Match::Pattern, "LOOOL"));
        assert!(!matcher("Hi", Match::WholeWord, true).is_some_and(|m| m.is_match("hi")));
        // Special characters in a plain trigger are taken literally.
        assert!(hit("c++", Match::WholeWord, "learning c++ today"));
    }

    #[test]
    fn scope_follows_channels_exclusions_and_threads() {
        let mut rule = sample();
        assert!(in_scope(&rule, 5, None));
        rule.channels = vec!["5".into()];
        assert!(in_scope(&rule, 5, None) && !in_scope(&rule, 6, None));
        assert!(in_scope(&rule, 99, Some(5)), "a thread inside an allowed channel counts");
        rule.exclude_channels = vec!["99".into()];
        assert!(!in_scope(&rule, 99, Some(5)));
    }

    #[test]
    fn reactions_parse_unicode_and_custom() {
        assert!(matches!(reaction("🔥"), Some(ReactionType::Unicode(_))));
        assert!(matches!(reaction("<:pog:123>"), Some(ReactionType::Custom { animated: false, .. })));
        assert!(matches!(reaction("<a:dance:456>"), Some(ReactionType::Custom { animated: true, .. })));
        assert!(matches!(reaction("pog:123"), Some(ReactionType::Custom { .. })));
        assert!(reaction("fire").is_none());
        assert!(reaction("").is_none());
    }

    #[test]
    fn validation_explains_what_is_missing() {
        let mut rule = sample();
        assert!(validate(&rule).is_ok());
        rule.triggers = vec!["  ".into()];
        assert!(validate(&rule).is_err());
        let mut rule = sample();
        rule.replies.clear();
        rule.reactions.clear();
        assert!(validate(&rule).unwrap_err().contains("does nothing"));
        let mut rule = sample();
        rule.match_mode = Match::Pattern;
        rule.triggers = vec!["(".into()];
        assert!(validate(&rule).is_err());
        let mut rule = sample();
        rule.reactions = vec!["fire".into()];
        assert!(validate(&rule).is_err());
    }

    #[test]
    fn replies_fill_the_author() {
        assert_eq!(fill("gm {user}! hi {name}", 42, "Riya"), "gm <@42>! hi Riya");
    }

    fn sample() -> AutoReply {
        AutoReply {
            id: 1,
            name: "gm".into(),
            enabled: true,
            triggers: vec!["gm".into()],
            match_mode: Match::WholeWord,
            case_sensitive: false,
            channels: vec![],
            exclude_channels: vec![],
            replies: vec!["gm {user} ☀️".into()],
            as_reply: true,
            reactions: vec!["☀️".into()],
            chance: 100,
            cooldown_secs: 30,
            hits: 0,
            last_hit: 0,
        }
    }
}
