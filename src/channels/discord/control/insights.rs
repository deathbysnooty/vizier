//! Insights: who replies to and mentions whom, kept as one small row per reply
//! or mention between two members, plus the tidbits worked out from them (top
//! duos, the longest back-and-forth, one-sided pairs...).
//!
//! The rows live in their own `insights.db` beside `stats.db`: they are
//! high-volume, append-only counters like the activity counts, pruned after 120
//! days, and keeping them out of `control.db` means a busy chat never waits on
//! the lock that settings, sessions and the audit trail use. No message text is
//! ever stored - only who, to whom, where and when.
//!
//! Recording never blocks the message handler: rows go into a buffer that a
//! background task writes every few seconds. #safe-corner, threads inside it,
//! channels the bot can't place, and DMs are never recorded.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serenity::all::{Context, Message};

/// Rows older than this are pruned once a day.
pub const RETENTION_DAYS: i64 = 120;
/// Two replies further apart than this don't make one back-and-forth.
pub const RUN_GAP_SECS: i64 = 15 * 60;
const FLUSH_EVERY: Duration = Duration::from_secs(5);

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
static BUFFER: LazyLock<Mutex<Vec<Interaction>>> = LazyLock::new(|| Mutex::new(Vec::new()));

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS interactions (
        message_id INTEGER NOT NULL, to_user INTEGER NOT NULL, kind TEXT NOT NULL,
        ts INTEGER NOT NULL, channel_id INTEGER NOT NULL, from_user INTEGER NOT NULL,
        replied_message_id INTEGER, source TEXT NOT NULL DEFAULT 'live',
        PRIMARY KEY (message_id, to_user, kind)) WITHOUT ROWID;
    CREATE INDEX IF NOT EXISTS interactions_ts ON interactions (ts);
    CREATE INDEX IF NOT EXISTS interactions_from ON interactions (from_user, ts);
    CREATE INDEX IF NOT EXISTS interactions_to ON interactions (to_user, ts);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("insights.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(SCHEMA)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Reply,
    Mention,
}

impl Kind {
    fn key(self) -> &'static str {
        match self {
            Kind::Reply => "reply",
            Kind::Mention => "mention",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Interaction {
    pub ts: i64,
    pub channel_id: u64,
    pub from_user: u64,
    pub to_user: u64,
    pub message_id: u64,
    pub replied_message_id: Option<u64>,
    pub kind: Kind,
    /// "live" or "history".
    pub source: &'static str,
}

// --- recording -------------------------------------------------------------------------

/// What recording needs to know about one message, taken off the event.
#[derive(Clone, Debug)]
pub struct Seen {
    pub ts: i64,
    pub in_server: bool,
    pub channel_id: u64,
    /// The parent channel when the message is in a thread.
    pub parent_id: Option<u64>,
    /// Whether the channel (or thread) is one the server lists.
    pub known_channel: bool,
    pub message_id: u64,
    pub author: u64,
    pub author_bot: bool,
    /// The message replied to: its id, author and whether that author is a bot.
    pub replied: Option<(u64, u64, bool)>,
    /// Users mentioned: id and whether a bot.
    pub mentions: Vec<(u64, bool)>,
}

/// Channels whose messages are never counted.
pub fn sensitive_channels() -> Vec<u64> {
    vec![super::super::weekly::SAFE_CORNER]
}

/// Whether messages in this channel may be counted at all.
pub fn countable(channel: u64, parent: Option<u64>, known: bool, sensitive: &[u64]) -> bool {
    known && !sensitive.contains(&channel) && !parent.is_some_and(|p| sensitive.contains(&p))
}

/// The rows one message makes: a reply to another human, and mentions of other
/// humans (not the replied-to person again, who the reply already counts).
pub fn rows_for(seen: &Seen, sensitive: &[u64]) -> Vec<Interaction> {
    if !seen.in_server || seen.author_bot || !countable(seen.channel_id, seen.parent_id, seen.known_channel, sensitive) {
        return Vec::new();
    }
    let row = |to: u64, kind: Kind, replied: Option<u64>| Interaction {
        ts: seen.ts,
        channel_id: seen.channel_id,
        from_user: seen.author,
        to_user: to,
        message_id: seen.message_id,
        replied_message_id: replied,
        kind,
        source: "live",
    };
    let mut out = Vec::new();
    let mut replied_to = None;
    if let Some((replied_id, to, bot)) = seen.replied {
        if !bot && to != seen.author && to != 0 {
            out.push(row(to, Kind::Reply, Some(replied_id)));
            replied_to = Some(to);
        }
    }
    let mut seen_ids = HashSet::new();
    for (to, bot) in &seen.mentions {
        if *bot || *to == seen.author || Some(*to) == replied_to || *to == 0 || !seen_ids.insert(*to) {
            continue;
        }
        out.push(row(*to, Kind::Mention, None));
    }
    out
}

/// The hook the message handler calls for every human message. Cheap: it reads
/// the event and the cache, and queues rows for the background writer.
pub fn on_message(ctx: &Context, msg: &Message) {
    let Some(guild_id) = msg.guild_id else { return };
    if msg.author.bot {
        return;
    }
    let channel = msg.channel_id.get();
    let (known, parent) = match ctx.cache.guild(guild_id) {
        Some(g) => {
            let thread = g.threads.iter().find(|t| t.id.get() == channel);
            (g.channels.contains_key(&msg.channel_id) || thread.is_some(), thread.and_then(|t| t.parent_id).map(|p| p.get()))
        }
        None => (false, None),
    };
    let seen = Seen {
        ts: msg.timestamp.unix_timestamp(),
        in_server: true,
        channel_id: channel,
        parent_id: parent,
        known_channel: known,
        message_id: msg.id.get(),
        author: msg.author.id.get(),
        author_bot: false,
        replied: msg.referenced_message.as_deref().map(|m| (m.id.get(), m.author.id.get(), m.author.bot)),
        mentions: msg.mentions.iter().map(|u| (u.id.get(), u.bot)).collect(),
    };
    let rows = rows_for(&seen, &sensitive_channels());
    if rows.is_empty() {
        return;
    }
    BUFFER.lock().extend(rows);
    start_writer();
}

fn start_writer() {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        let mut last_prune = 0i64;
        loop {
            tokio::time::sleep(FLUSH_EVERY).await;
            let rows: Vec<Interaction> = std::mem::take(&mut *BUFFER.lock());
            let now = chrono::Utc::now().timestamp();
            let prune = now - last_prune > 86_400;
            if rows.is_empty() && !prune {
                continue;
            }
            let _ = tokio::task::spawn_blocking(move || {
                if !rows.is_empty() {
                    if let Err(err) = insert(&rows) {
                        tracing::warn!("insights: could not write {} rows: {}", rows.len(), err);
                    }
                }
                if prune {
                    let _ = prune_before(now - RETENTION_DAYS * 86_400);
                }
            })
            .await;
            if prune {
                last_prune = now;
            }
        }
    });
}

// --- the store ---------------------------------------------------------------------------

/// Writes rows, ignoring any already recorded (same message, target and kind).
pub fn insert(rows: &[Interaction]) -> anyhow::Result<usize> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("insights store not open"))?;
    let mut conn = db.lock();
    let mut added = 0;
    for chunk in rows.chunks(5000) {
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO interactions (message_id, to_user, kind, ts, channel_id, from_user, replied_message_id, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for r in chunk {
                added += stmt.execute(params![
                    r.message_id as i64,
                    r.to_user as i64,
                    r.kind.key(),
                    r.ts,
                    r.channel_id as i64,
                    r.from_user as i64,
                    r.replied_message_id.map(|m| m as i64),
                    r.source
                ])?;
            }
        }
        tx.commit()?;
    }
    Ok(added)
}

pub fn prune_before(ts: i64) -> anyhow::Result<usize> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("insights store not open"))?;
    Ok(db.lock().execute("DELETE FROM interactions WHERE ts < ?1", params![ts])?)
}

pub fn delete_history_rows() -> anyhow::Result<usize> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("insights store not open"))?;
    Ok(db.lock().execute("DELETE FROM interactions WHERE source = 'history'", [])?)
}

/// Every row in `[since, until)`, oldest first.
pub fn rows_between(since: i64, until: i64) -> Vec<Interaction> {
    let Some(db) = DB.get() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT ts, channel_id, from_user, to_user, message_id, replied_message_id, kind, source FROM interactions
         WHERE ts >= ?1 AND ts < ?2 ORDER BY ts, message_id",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![since, until], |r| {
        Ok(Interaction {
            ts: r.get(0)?,
            channel_id: r.get::<_, i64>(1)? as u64,
            from_user: r.get::<_, i64>(2)? as u64,
            to_user: r.get::<_, i64>(3)? as u64,
            message_id: r.get::<_, i64>(4)? as u64,
            replied_message_id: r.get::<_, Option<i64>>(5)?.map(|m| m as u64),
            kind: if r.get::<_, String>(6)? == "mention" { Kind::Mention } else { Kind::Reply },
            source: if r.get::<_, String>(7)? == "history" { "history" } else { "live" },
        })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// The people `user` exchanges replies and mentions with most since `since`,
/// both ways added up, most first. For member notes.
pub fn partners(user: u64, since: i64, limit: usize) -> Vec<(u64, i64)> {
    let Some(db) = DB.get() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT other, SUM(n) AS total FROM (
             SELECT to_user AS other, COUNT(*) AS n FROM interactions WHERE from_user = ?1 AND ts >= ?2 GROUP BY to_user
             UNION ALL
             SELECT from_user AS other, COUNT(*) AS n FROM interactions WHERE to_user = ?1 AND ts >= ?2 GROUP BY from_user)
         WHERE other != ?1 GROUP BY other ORDER BY total DESC, other LIMIT ?3",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![user as i64, since, limit as i64], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// When each pair first replied to each other, over everything recorded.
pub fn first_replies() -> HashMap<(u64, u64), i64> {
    let Some(db) = DB.get() else { return HashMap::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(
        "SELECT MIN(from_user, to_user), MAX(from_user, to_user), MIN(ts) FROM interactions WHERE kind = 'reply'
         GROUP BY MIN(from_user, to_user), MAX(from_user, to_user)",
    ) else {
        return HashMap::new();
    };
    stmt.query_map([], |r| Ok(((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64), r.get::<_, i64>(2)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// The oldest recorded moment, and where the history ends and live recording began.
pub fn coverage() -> (Option<i64>, Option<i64>) {
    let Some(db) = DB.get() else { return (None, None) };
    let conn = db.lock();
    let earliest = conn.query_row("SELECT MIN(ts) FROM interactions", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten();
    let live = conn.query_row("SELECT MIN(ts) FROM interactions WHERE source = 'live'", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten();
    (earliest, live)
}

pub fn meta_get(key: &str) -> Option<String> {
    let db = DB.get()?;
    db.lock().query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(key: &str, value: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }
}

// --- back-and-forth -----------------------------------------------------------------------

/// One stretch of alternating replies between two people.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Default)]
pub struct Run {
    pub len: usize,
    pub start_ts: i64,
    pub end_ts: i64,
    pub channel_id: u64,
    /// Who sent the first reply of the stretch.
    pub starter: u64,
}

/// The longest alternating stretch in a pair's replies (both directions, any
/// order): each reply goes the other way from the one before, in the same
/// channel, within `RUN_GAP_SECS` of it. A same-way reply, a channel change or a
/// long gap starts a new stretch.
pub fn longest_run(replies: &[&Interaction]) -> Run {
    let mut sorted: Vec<&Interaction> = replies.to_vec();
    sorted.sort_by_key(|r| (r.ts, r.message_id));
    let mut best = Run::default();
    let mut current = Run::default();
    let mut prev: Option<&Interaction> = None;
    for r in sorted {
        let continues = prev.is_some_and(|p| p.from_user == r.to_user && p.to_user == r.from_user && p.channel_id == r.channel_id && r.ts - p.ts <= RUN_GAP_SECS);
        if continues {
            current.len += 1;
            current.end_ts = r.ts;
        } else {
            current = Run { len: 1, start_ts: r.ts, end_ts: r.ts, channel_id: r.channel_id, starter: r.from_user };
        }
        if current.len > best.len {
            best = current;
        }
        prev = Some(r);
    }
    best
}

#[derive(Clone, Debug, Serialize)]
pub struct Pair {
    /// The lower id first.
    pub a: u64,
    pub b: u64,
    pub a_to_b: usize,
    pub b_to_a: usize,
    pub mentions_a_to_b: usize,
    pub mentions_b_to_a: usize,
    pub longest: Run,
    pub last_ts: i64,
    pub top_channel: Option<(u64, usize)>,
}

impl Pair {
    pub fn replies(&self) -> usize {
        self.a_to_b + self.b_to_a
    }
    /// 1.0 when both sides reply equally, 0.0 when only one does.
    pub fn balance(&self) -> f64 {
        let (lo, hi) = (self.a_to_b.min(self.b_to_a), self.a_to_b.max(self.b_to_a));
        if hi == 0 { 0.0 } else { lo as f64 / hi as f64 }
    }
}

pub fn key(x: u64, y: u64) -> (u64, u64) {
    (x.min(y), x.max(y))
}

/// Every pair in the rows, with both directions, mentions and their longest run.
pub fn pairs(rows: &[Interaction]) -> HashMap<(u64, u64), Pair> {
    let mut grouped: HashMap<(u64, u64), Vec<&Interaction>> = HashMap::new();
    for r in rows {
        grouped.entry(key(r.from_user, r.to_user)).or_default().push(r);
    }
    grouped
        .into_iter()
        .map(|((a, b), list)| {
            let replies: Vec<&Interaction> = list.iter().copied().filter(|r| r.kind == Kind::Reply).collect();
            let count = |from: u64, kind: Kind| list.iter().filter(|r| r.from_user == from && r.kind == kind).count();
            let mut channels: HashMap<u64, usize> = HashMap::new();
            for r in &replies {
                *channels.entry(r.channel_id).or_insert(0) += 1;
            }
            let top_channel = channels.into_iter().max_by_key(|(c, n)| (*n, std::cmp::Reverse(*c)));
            let pair = Pair {
                a,
                b,
                a_to_b: count(a, Kind::Reply),
                b_to_a: count(b, Kind::Reply),
                mentions_a_to_b: count(a, Kind::Mention),
                mentions_b_to_a: count(b, Kind::Mention),
                longest: longest_run(&replies),
                last_ts: list.iter().map(|r| r.ts).max().unwrap_or(0),
                top_channel,
            };
            ((a, b), pair)
        })
        .collect()
}

/// A keeps replying to B and B rarely answers: at least 15 replies one way and
/// no more than a fifth of that back. Returns (from, to, replies, back).
pub fn one_sided(pair: &Pair) -> Option<(u64, u64, usize, usize)> {
    let check = |from: u64, to: u64, there: usize, back: usize| (there >= 15 && back * 5 <= there).then_some((from, to, there, back));
    check(pair.a, pair.b, pair.a_to_b, pair.b_to_a).or_else(|| check(pair.b, pair.a, pair.b_to_a, pair.a_to_b))
}

// --- filling in from history -----------------------------------------------------------------

static DISCORD_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(DiscordId: (\d+)\)\s*$").unwrap());
static RAW_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@!?(\d+)>").unwrap());

/// One stored request, as the backfill needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRequest {
    pub ts: i64,
    pub author: u64,
    pub message_id: u64,
    pub channel_id: Option<u64>,
    pub is_dm: bool,
    pub replied_message_id: Option<u64>,
    pub mentions: Vec<u64>,
}

pub fn parse_stored(ts_ms: i64, data: &str) -> Option<StoredRequest> {
    let v: Value = serde_json::from_str(data).ok()?;
    let req = v.get("content")?.get("Request")?;
    let author: u64 = DISCORD_ID.captures(req.get("user")?.as_str()?)?.get(1)?.as_str().parse().ok()?;
    let meta = req.get("metadata")?;
    let id = |k: &str| meta.get(k).and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_u64()));
    let message_id = id("message_id").or_else(|| req.get("platform_message_id").and_then(|p| p.get("Discord")).and_then(Value::as_u64))?;
    let text = req.get("content").and_then(Value::as_object).and_then(|o| o.values().next()).and_then(Value::as_str).unwrap_or("");
    Some(StoredRequest {
        ts: ts_ms / 1000,
        author,
        message_id,
        channel_id: id("discord_channel_id"),
        is_dm: meta.get("is_dm").and_then(Value::as_bool).unwrap_or(false),
        replied_message_id: id("replied_message_id"),
        mentions: RAW_MENTION.captures_iter(text).filter_map(|c| c[1].parse().ok()).collect(),
    })
}

/// How the backfill judges channels and people.
pub struct BackfillRules<'a> {
    pub known_channel: &'a dyn Fn(u64) -> bool,
    pub parent_of: &'a dyn Fn(u64) -> Option<u64>,
    pub is_bot: &'a dyn Fn(u64) -> bool,
    pub sensitive: Vec<u64>,
}

/// Turns stored requests into interactions: a reply counts only when the
/// message it answers was stored too (so its author is known) and both people
/// are members; DMs, #safe-corner and its threads, and channels the server
/// doesn't list are left out.
pub fn resolve(requests: &[StoredRequest], rules: &BackfillRules) -> Vec<Interaction> {
    let authors: HashMap<u64, u64> = requests.iter().map(|r| (r.message_id, r.author)).collect();
    let mut out = Vec::new();
    for r in requests {
        let Some(channel) = r.channel_id else { continue };
        if r.is_dm || (rules.is_bot)(r.author) {
            continue;
        }
        let parent = (rules.parent_of)(channel);
        let known = (rules.known_channel)(channel) || parent.is_some_and(|p| (rules.known_channel)(p));
        let seen = Seen {
            ts: r.ts,
            in_server: true,
            channel_id: channel,
            parent_id: parent,
            known_channel: known,
            message_id: r.message_id,
            author: r.author,
            author_bot: false,
            replied: r.replied_message_id.and_then(|m| authors.get(&m).map(|a| (m, *a, (rules.is_bot)(*a)))),
            mentions: r.mentions.iter().map(|m| (*m, (rules.is_bot)(*m))).collect(),
        };
        out.extend(rows_for(&seen, &rules.sensitive).into_iter().map(|row| Interaction { source: "history", ..row }));
    }
    out
}

/// Reads the bot's stored requests since `since` from a read-only connection to
/// vizier.db, a batch at a time with a short pause so the database is never
/// hogged. `progress` hears the rows scanned so far.
pub fn read_history(conn: &Connection, agent_id: &str, since: i64, pause: Duration, progress: &dyn Fn(usize)) -> rusqlite::Result<Vec<StoredRequest>> {
    const BATCH: i64 = 2000;
    let mut out = Vec::new();
    let (mut last_ts, mut last_uid) = (since * 1000, String::new());
    let mut scanned = 0;
    let mut stmt = conn.prepare(
        "SELECT uid, timestamp, data FROM session_history INDEXED BY idx_sh_agent_time
         WHERE agent_id = ?1 AND (timestamp > ?2 OR (timestamp = ?2 AND uid > ?3)) AND content_type = 'Request'
         ORDER BY timestamp, uid LIMIT ?4",
    )?;
    loop {
        let batch: Vec<(String, i64, String)> = stmt
            .query_map(params![agent_id, last_ts, last_uid, BATCH], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        if batch.is_empty() {
            break;
        }
        scanned += batch.len();
        if let Some((uid, ts, _)) = batch.last() {
            last_ts = *ts;
            last_uid = uid.clone();
        }
        out.extend(batch.iter().filter_map(|(_, ts, data)| parse_stored(*ts, data)));
        progress(scanned);
        if (batch.len() as i64) < BATCH {
            break;
        }
        std::thread::sleep(pause);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFE: u64 = 1543162777642868736;

    fn seen() -> Seen {
        Seen {
            ts: 1000,
            in_server: true,
            channel_id: 10,
            parent_id: None,
            known_channel: true,
            message_id: 500,
            author: 1,
            author_bot: false,
            replied: Some((499, 2, false)),
            mentions: vec![(3, false), (2, false), (3, false), (1, false), (9, true)],
        }
    }

    #[test]
    fn recording_keeps_member_to_member_only() {
        let rows = rows_for(&seen(), &[SAFE]);
        let got: Vec<(u64, Kind)> = rows.iter().map(|r| (r.to_user, r.kind)).collect();
        assert_eq!(got, vec![(2, Kind::Reply), (3, Kind::Mention)], "no self, no bot, no double count of the replied person");
        assert!(rows_for(&Seen { author_bot: true, ..seen() }, &[SAFE]).is_empty());
        assert!(rows_for(&Seen { in_server: false, ..seen() }, &[SAFE]).is_empty(), "DMs");
        assert!(rows_for(&Seen { channel_id: SAFE, ..seen() }, &[SAFE]).is_empty(), "#safe-corner");
        assert!(rows_for(&Seen { channel_id: 77, parent_id: Some(SAFE), ..seen() }, &[SAFE]).is_empty(), "a thread in #safe-corner");
        assert!(rows_for(&Seen { known_channel: false, ..seen() }, &[SAFE]).is_empty(), "a channel the bot can't place");
        let to_bot = rows_for(&Seen { replied: Some((499, 9, true)), mentions: vec![], ..seen() }, &[SAFE]);
        assert!(to_bot.is_empty(), "replies to bots don't count");
        let to_self = rows_for(&Seen { replied: Some((499, 1, false)), mentions: vec![], ..seen() }, &[SAFE]);
        assert!(to_self.is_empty());
    }

    fn reply(ts: i64, from: u64, to: u64, channel: u64, id: u64) -> Interaction {
        Interaction { ts, channel_id: channel, from_user: from, to_user: to, message_id: id, replied_message_id: None, kind: Kind::Reply, source: "live" }
    }

    #[test]
    fn back_and_forth_needs_alternation_one_channel_and_short_gaps() {
        let rows = vec![
            reply(0, 1, 2, 10, 1),
            reply(60, 2, 1, 10, 2),
            reply(120, 1, 2, 10, 3),
            reply(180, 2, 1, 10, 4), // four in a row
            reply(240, 2, 1, 10, 5), // same way again: a new stretch of one
            reply(300, 1, 2, 10, 6), // 2
            reply(360, 2, 1, 11, 7), // other channel: new stretch
            reply(420, 1, 2, 11, 8), // 2
            reply(420 + RUN_GAP_SECS + 1, 2, 1, 11, 9), // too late: new stretch
        ];
        let refs: Vec<&Interaction> = rows.iter().collect();
        let run = longest_run(&refs);
        assert_eq!((run.len, run.start_ts, run.end_ts, run.channel_id, run.starter), (4, 0, 180, 10, 1));
        let exactly_gap = vec![reply(0, 1, 2, 10, 1), reply(RUN_GAP_SECS, 2, 1, 10, 2)];
        assert_eq!(longest_run(&exactly_gap.iter().collect::<Vec<_>>()).len, 2, "fifteen minutes still counts");
        let p = pairs(&rows);
        let pair = &p[&(1, 2)];
        assert_eq!((pair.a_to_b, pair.b_to_a, pair.longest.len), (4, 5, 4));
        assert_eq!(pair.top_channel, Some((10, 6)));
    }

    #[test]
    fn one_sided_needs_fifteen_and_a_fifth_back() {
        let mut rows: Vec<Interaction> = (0..15).map(|i| reply(i * 10_000, 1, 2, 10, i as u64)).collect();
        rows.extend((0..3).map(|i| reply(i * 10_000 + 5, 2, 1, 10, 100 + i as u64)));
        assert_eq!(one_sided(&pairs(&rows)[&(1, 2)]), Some((1, 2, 15, 3)));
        rows.push(reply(999_999, 2, 1, 10, 999));
        assert_eq!(one_sided(&pairs(&rows)[&(1, 2)]), None, "four back out of fifteen is more than a fifth");
        let few: Vec<Interaction> = (0..14).map(|i| reply(i * 10_000, 3, 4, 10, 500 + i as u64)).collect();
        assert_eq!(one_sided(&pairs(&few)[&(3, 4)]), None);
    }

    fn stored(ts: i64, author: u64, id: u64, channel: u64, replied: Option<u64>, dm: bool) -> String {
        serde_json::json!({ "content": { "Request": {
            "user": format!("@someone (DiscordId: {})", author),
            "content": { "silent_read": "hey <@3>" },
            "platform_message_id": { "Discord": id },
            "metadata": {
                "message_id": id.to_string(),
                "discord_channel_id": channel.to_string(),
                "replied_message_id": replied.map(|r| r.to_string()),
                "is_dm": dm
            }
        }}})
        .to_string()
        .replace("\"ts\"", &ts.to_string())
    }

    #[test]
    fn history_resolves_authors_and_skips_what_it_must() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_history (uid TEXT PRIMARY KEY, agent_id TEXT NOT NULL, channel TEXT NOT NULL, topic TEXT,
                 timestamp INTEGER NOT NULL, content_type TEXT NOT NULL, data TEXT NOT NULL);
             CREATE INDEX idx_sh_agent_time ON session_history(agent_id, timestamp);",
        )
        .unwrap();
        let add = |uid: &str, ts: i64, data: String, ctype: &str| {
            conn.execute("INSERT INTO session_history VALUES (?1, 'lodu', 'discord__x', NULL, ?2, ?3, ?4)", params![uid, ts * 1000, ctype, data]).unwrap();
        };
        add("a", 100, stored(100, 1, 1001, 10, None, false), "Request");
        add("b", 200, stored(200, 2, 1002, 10, Some(1001), false), "Request"); // 2 -> 1
        add("c", 300, stored(300, 1, 1003, 10, Some(4242), false), "Request"); // replied message never stored
        add("d", 400, stored(400, 2, 1004, SAFE, Some(1001), false), "Request"); // safe corner
        add("e", 500, stored(500, 2, 1005, 77, Some(1001), false), "Request"); // thread in safe corner
        add("f", 600, stored(600, 2, 1006, 20, Some(1001), true), "Request"); // DM
        add("g", 700, stored(700, 1, 1007, 10, Some(1008), false), "Request"); // to the bot's message id 1008 (bot author 9)
        add("h", 710, stored(710, 9, 1008, 10, None, false), "Request");
        add("i", 800, stored(800, 5, 1009, 999, Some(1001), false), "Request"); // unknown channel
        add("j", 900, "{}".into(), "Response");
        let scanned = std::cell::Cell::new(0);
        let requests = read_history(&conn, "lodu", 0, Duration::ZERO, &|n| scanned.set(n)).unwrap();
        assert_eq!(scanned.get(), 9);
        let rules = BackfillRules {
            known_channel: &|c| c == 10 || c == 20 || c == SAFE,
            parent_of: &|c| (c == 77).then_some(SAFE),
            is_bot: &|u| u == 9,
            sensitive: vec![SAFE],
        };
        let rows = resolve(&requests, &rules);
        let replies: Vec<(u64, u64, u64)> = rows.iter().filter(|r| r.kind == Kind::Reply).map(|r| (r.from_user, r.to_user, r.message_id)).collect();
        assert_eq!(replies, vec![(2, 1, 1002)]);
        assert!(rows.iter().all(|r| r.source == "history" && r.channel_id == 10));
        let mentions: Vec<(u64, u64)> = rows.iter().filter(|r| r.kind == Kind::Mention).map(|r| (r.from_user, r.to_user)).collect();
        assert!(mentions.contains(&(1, 3)), "raw <@id> mentions are picked up: {mentions:?}");
        assert!(!rows.iter().any(|r| r.from_user == 9 || r.to_user == 9));
    }
}
