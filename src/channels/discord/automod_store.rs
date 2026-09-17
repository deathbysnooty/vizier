//! Where automatic moderation keeps what it did: `.runtime/automod.db`.
//!
//! Three things live here.
//!
//! * **Flags** — one row every time a rule fired: spam that was deleted, or a
//!   message a human was asked to look at. The row holds who, where, when, the
//!   text (truncated), why, whether the model was asked and what it said, and
//!   what a moderator decided about it.
//! * **Decisions** — every press of a moderator's button, kept separately, so
//!   pressing twice or changing one's mind is all on the record. The honest
//!   measure of whether the AI half is worth keeping is how often a moderator
//!   said "not AI", and that number can only be trusted if nothing overwrites
//!   it.
//! * **Style** — running totals of how each member normally writes (message
//!   length, punctuation, emoji, Hinglish share, capitalisation) plus a few
//!   samples of their ordinary messages. This is what lets the AI half measure
//!   a message against *that member*, which is the only signal here that is
//!   fair to someone writing English as a second language.
//!
//! Nothing in this file deletes a message or takes a point off anybody; it only
//! writes rows. Deleting is done by the spam half of [`super::automod`], and an
//! AI score never reaches it.
//!
//! Style totals arrive on a bounded queue and are applied by one writer thread,
//! so the gateway never waits for the disk. Flags, decisions and the panel's
//! reads go through a second connection behind a mutex, taken only inside
//! `spawn_blocking` — never across an `.await`.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Style updates waiting for the writer before new ones are dropped.
pub const QUEUE: usize = 4096;
/// Samples of a member's own ordinary writing kept for the model prompt.
pub const MAX_SAMPLES: usize = 8;
/// The longest a kept sample may be.
pub const SAMPLE_CHARS: usize = 400;
/// The shortest a message may be to be worth keeping as a sample.
pub const SAMPLE_MIN_CHARS: usize = 25;
/// The most flagged text kept on a row.
pub const MAX_TEXT: usize = 1500;

const DAY: i64 = 86_400;

// --- what a row says ---------------------------------------------------------------

/// Which half of the feature made the flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// An objective spam rule. These rows are the only ones whose messages were
    /// deleted by the bot itself.
    Spam,
    /// A message that might have been written by an AI. Flagged for a human and
    /// nothing else: see the rule in [`super::automod`].
    Ai,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Spam => "spam",
            Kind::Ai => "ai",
        }
    }

    pub fn from_str(raw: &str) -> Option<Kind> {
        match raw {
            "spam" => Some(Kind::Spam),
            "ai" => Some(Kind::Ai),
            _ => None,
        }
    }

    /// Whether the bot itself may remove a message for this kind of flag. Only
    /// ever true for spam, and nothing configurable changes that.
    pub fn may_delete(self) -> bool {
        matches!(self, Kind::Spam)
    }
}

/// What became of a flagged message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    /// The bot deleted it (spam), or a moderator pressed Delete it.
    Deleted,
    /// A moderator said the flag was wrong.
    Dismissed,
    /// Nobody has done anything about it.
    Untouched,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Deleted => "deleted",
            Outcome::Dismissed => "dismissed",
            Outcome::Untouched => "untouched",
        }
    }

    pub fn from_str(raw: &str) -> Option<Outcome> {
        match raw {
            "deleted" => Some(Outcome::Deleted),
            "dismissed" => Some(Outcome::Dismissed),
            "untouched" => Some(Outcome::Untouched),
            _ => None,
        }
    }
}

/// What the model said, when it was asked at all.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelSay {
    pub model: String,
    /// 0..1. How sure the model was that an AI wrote it.
    pub confidence: f64,
    /// Its reasons, in its own words, already shortened.
    pub reasons: String,
    /// It said the text is too short or too plain to judge.
    pub unjudgeable: bool,
}

/// A flag about to be written.
#[derive(Clone, Debug, PartialEq)]
pub struct NewFlag {
    pub kind: Kind,
    /// The rule that fired, as a short key ("repeat", "burst", "ai"...).
    pub rule: String,
    pub member_id: u64,
    pub member_name: String,
    pub channel_id: u64,
    pub channel_name: String,
    pub guild_id: u64,
    /// The message the flag is about; for a burst, the last of them.
    pub message_id: u64,
    pub ts: i64,
    pub text: String,
    /// How many messages the flag covers. More than one only for a burst or a
    /// repeat, which are always one log entry however many messages they were.
    pub messages: u32,
    /// The free score, 0..1. Only meaningful for [`Kind::Ai`].
    pub score: Option<f64>,
    /// Why the rule fired, in plain words.
    pub reasons: Vec<String>,
    pub model: Option<ModelSay>,
    /// Whether the model was actually called, whatever came back. This, not
    /// whether an answer arrived, is what the hourly cap counts: a bad hour of
    /// timeouts must not be able to run up an unlimited number of calls.
    pub model_asked: bool,
    /// Whether there was enough of the member's own writing to compare against.
    pub had_baseline: bool,
    pub outcome: Outcome,
}

/// A flag as the panel and the log read it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Flag {
    pub id: i64,
    pub kind: Kind,
    pub rule: String,
    pub member_id: u64,
    pub member_name: String,
    pub channel_id: u64,
    pub channel_name: String,
    pub guild_id: u64,
    pub message_id: u64,
    pub ts: i64,
    /// Empty once the text has been cleared by the retention cut.
    pub text: String,
    /// True when the text was cleared because the row got old.
    pub text_cleared: bool,
    pub messages: u32,
    pub score: Option<f64>,
    pub reasons: Vec<String>,
    pub model: Option<ModelSay>,
    pub model_asked: bool,
    pub had_baseline: bool,
    pub outcome: Outcome,
    pub decided_by: Option<u64>,
    pub decided_ts: Option<i64>,
    /// The bot's own message in the moderation log, so a decision can edit it.
    pub log_message_id: Option<u64>,
}

/// One press of a moderator's button.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decision {
    pub id: i64,
    pub flag_id: i64,
    pub outcome: Outcome,
    pub by: u64,
    pub ts: i64,
}

// --- how a member normally writes ------------------------------------------------

/// One message's contribution to a member's style totals.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleUpdate {
    pub member_id: u64,
    pub ts: i64,
    pub chars: u32,
    pub punct: u32,
    pub emoji: u32,
    pub tokens: u32,
    pub hinglish_tokens: u32,
    pub sentences: u32,
    pub caps_starts: u32,
    /// The message itself, when it is worth keeping as a sample of their
    /// ordinary writing. Already shortened.
    pub sample: Option<String>,
}

/// The running totals for one member, straight off the table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StyleRow {
    pub messages: u32,
    pub chars: u64,
    pub punct: u64,
    pub emoji: u64,
    pub tokens: u64,
    pub hinglish_tokens: u64,
    pub sentences: u64,
    pub caps_starts: u64,
    pub samples: Vec<String>,
}

// --- the store -------------------------------------------------------------------

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS flags (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL,
        rule TEXT NOT NULL,
        member_id INTEGER NOT NULL,
        member_name TEXT NOT NULL DEFAULT '',
        channel_id INTEGER NOT NULL,
        channel_name TEXT NOT NULL DEFAULT '',
        guild_id INTEGER NOT NULL DEFAULT 0,
        message_id INTEGER NOT NULL,
        ts INTEGER NOT NULL,
        text TEXT NOT NULL DEFAULT '',
        text_cleared INTEGER NOT NULL DEFAULT 0,
        messages INTEGER NOT NULL DEFAULT 1,
        score REAL,
        reasons_json TEXT NOT NULL DEFAULT '[]',
        model_json TEXT,
        model_asked INTEGER NOT NULL DEFAULT 0,
        had_baseline INTEGER NOT NULL DEFAULT 0,
        outcome TEXT NOT NULL DEFAULT 'untouched',
        decided_by INTEGER,
        decided_ts INTEGER,
        log_message_id INTEGER);
    CREATE INDEX IF NOT EXISTS flags_ts ON flags (ts);
    CREATE INDEX IF NOT EXISTS flags_member ON flags (member_id);
    CREATE INDEX IF NOT EXISTS flags_kind ON flags (kind);
    CREATE UNIQUE INDEX IF NOT EXISTS flags_ai_message ON flags (message_id, kind);
    CREATE TABLE IF NOT EXISTS decisions (
        id INTEGER PRIMARY KEY AUTOINCREMENT, flag_id INTEGER NOT NULL, outcome TEXT NOT NULL,
        by_user INTEGER NOT NULL, ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS decisions_flag ON decisions (flag_id);
    CREATE INDEX IF NOT EXISTS decisions_ts ON decisions (ts);
    CREATE TABLE IF NOT EXISTS style (
        member_id INTEGER PRIMARY KEY,
        messages INTEGER NOT NULL DEFAULT 0,
        chars INTEGER NOT NULL DEFAULT 0,
        punct INTEGER NOT NULL DEFAULT 0,
        emoji INTEGER NOT NULL DEFAULT 0,
        tokens INTEGER NOT NULL DEFAULT 0,
        hinglish_tokens INTEGER NOT NULL DEFAULT 0,
        sentences INTEGER NOT NULL DEFAULT 0,
        caps_starts INTEGER NOT NULL DEFAULT 0,
        samples_json TEXT NOT NULL DEFAULT '[]',
        updated_ts INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Opens (or makes) the database at `path`.
pub fn open_at(path: &Path) -> anyhow::Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// An in-memory store, for tests.
pub fn open_memory() -> rusqlite::Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

fn reasons_of(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

fn samples_of(json: &str) -> Vec<String> {
    serde_json::from_str(json).unwrap_or_default()
}

// --- writing flags ----------------------------------------------------------------

/// Writes a flag and gives back its row id. A second flag of the same kind
/// about the same message changes nothing and gives back `None`: the AI half
/// must never judge one message twice, and a spam rule must never log the same
/// removal twice.
pub fn add_flag(conn: &Connection, f: &NewFlag) -> rusqlite::Result<Option<i64>> {
    let text: String = f.text.chars().take(MAX_TEXT).collect();
    let added = conn.execute(
        "INSERT OR IGNORE INTO flags (kind, rule, member_id, member_name, channel_id, channel_name, guild_id, message_id, ts,
             text, messages, score, reasons_json, model_json, model_asked, had_baseline, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        params![
            f.kind.as_str(),
            f.rule,
            f.member_id as i64,
            f.member_name,
            f.channel_id as i64,
            f.channel_name,
            f.guild_id as i64,
            f.message_id as i64,
            f.ts,
            text,
            f.messages,
            f.score,
            serde_json::to_string(&f.reasons).unwrap_or_else(|_| "[]".into()),
            f.model.as_ref().and_then(|m| serde_json::to_string(m).ok()),
            f.model_asked,
            f.had_baseline,
            f.outcome.as_str(),
        ],
    )?;
    Ok((added > 0).then(|| conn.last_insert_rowid()))
}

/// Remembers the bot's own log message for a flag, so a moderator's decision
/// can edit it later.
pub fn set_log_message(conn: &Connection, flag: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE flags SET log_message_id = ?2 WHERE id = ?1", params![flag, message as i64])?;
    Ok(())
}

/// Whether the AI half has already judged this message. Checked before the free
/// signals are even scored, so nothing is paid for twice.
pub fn already_judged(conn: &Connection, message: u64) -> rusqlite::Result<bool> {
    Ok(conn
        .query_row("SELECT 1 FROM flags WHERE message_id = ?1 AND kind = 'ai'", params![message as i64], |_| Ok(()))
        .optional()?
        .is_some())
}

/// How many times the model was asked since `since`, counted from the rows
/// themselves so the cap survives a restart. A call that failed or timed out
/// counts: it cost just as much time.
pub fn model_calls_since(conn: &Connection, since: i64) -> rusqlite::Result<u32> {
    conn.query_row("SELECT COUNT(*) FROM flags WHERE ts >= ?1 AND model_asked = 1", params![since], |r| {
        r.get::<_, i64>(0).map(|n| n as u32)
    })
}

/// Records a moderator's press: the decision is appended to the record and the
/// flag's own outcome is brought up to date. Both, always: the appended row is
/// what makes the "was wrong" count trustworthy.
pub fn decide(conn: &Connection, flag: i64, outcome: Outcome, by: u64, ts: i64) -> rusqlite::Result<bool> {
    let exists = conn.query_row("SELECT 1 FROM flags WHERE id = ?1", params![flag], |_| Ok(())).optional()?.is_some();
    if !exists {
        return Ok(false);
    }
    conn.execute(
        "INSERT INTO decisions (flag_id, outcome, by_user, ts) VALUES (?1, ?2, ?3, ?4)",
        params![flag, outcome.as_str(), by as i64, ts],
    )?;
    conn.execute(
        "UPDATE flags SET outcome = ?2, decided_by = ?3, decided_ts = ?4 WHERE id = ?1",
        params![flag, outcome.as_str(), by as i64, ts],
    )?;
    Ok(true)
}

/// Every press about one flag, oldest first.
pub fn decisions_for(conn: &Connection, flag: i64) -> rusqlite::Result<Vec<Decision>> {
    let mut stmt = conn.prepare("SELECT id, flag_id, outcome, by_user, ts FROM decisions WHERE flag_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map(params![flag], |r| {
        Ok(Decision {
            id: r.get(0)?,
            flag_id: r.get(1)?,
            outcome: Outcome::from_str(&r.get::<_, String>(2)?).unwrap_or(Outcome::Untouched),
            by: r.get::<_, i64>(3)? as u64,
            ts: r.get(4)?,
        })
    })?;
    rows.collect()
}

// --- reading, for the panel and the log ------------------------------------------

fn flag_from(r: &rusqlite::Row) -> rusqlite::Result<Flag> {
    Ok(Flag {
        id: r.get(0)?,
        kind: Kind::from_str(&r.get::<_, String>(1)?).unwrap_or(Kind::Spam),
        rule: r.get(2)?,
        member_id: r.get::<_, i64>(3)? as u64,
        member_name: r.get(4)?,
        channel_id: r.get::<_, i64>(5)? as u64,
        channel_name: r.get(6)?,
        guild_id: r.get::<_, i64>(7)? as u64,
        message_id: r.get::<_, i64>(8)? as u64,
        ts: r.get(9)?,
        text: r.get(10)?,
        text_cleared: r.get(11)?,
        messages: r.get(12)?,
        score: r.get(13)?,
        reasons: reasons_of(&r.get::<_, String>(14)?),
        model: r.get::<_, Option<String>>(15)?.as_deref().and_then(|j| serde_json::from_str(j).ok()),
        model_asked: r.get(16)?,
        had_baseline: r.get(17)?,
        outcome: Outcome::from_str(&r.get::<_, String>(18)?).unwrap_or(Outcome::Untouched),
        decided_by: r.get::<_, Option<i64>>(19)?.map(|x| x as u64),
        decided_ts: r.get(20)?,
        log_message_id: r.get::<_, Option<i64>>(21)?.map(|x| x as u64),
    })
}

const FLAG_COLUMNS: &str = "id, kind, rule, member_id, member_name, channel_id, channel_name, guild_id, message_id, ts,
     text, text_cleared, messages, score, reasons_json, model_json, model_asked, had_baseline, outcome, decided_by, decided_ts,
     log_message_id";

pub fn flag(conn: &Connection, id: i64) -> rusqlite::Result<Option<Flag>> {
    conn.query_row(&format!("SELECT {} FROM flags WHERE id = ?1", FLAG_COLUMNS), params![id], flag_from).optional()
}

/// What the panel asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListFilter {
    pub kind: Option<Kind>,
    pub member: Option<u64>,
    pub outcome: Option<Outcome>,
    pub since: i64,
    /// Row id to carry on below (exclusive).
    pub before: Option<i64>,
    pub limit: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Page {
    pub rows: Vec<Flag>,
    /// Pass back as `before` for the next page; none when this was the last.
    pub next_before: Option<i64>,
}

/// Flags newest first, one page at a time.
pub fn list(conn: &Connection, f: &ListFilter) -> rusqlite::Result<Page> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM flags
         WHERE id < ?1 AND ts >= ?2 AND (?3 IS NULL OR kind = ?3) AND (?4 IS NULL OR member_id = ?4)
               AND (?5 IS NULL OR outcome = ?5)
         ORDER BY id DESC LIMIT ?6",
        FLAG_COLUMNS
    ))?;
    let rows = stmt.query_map(
        params![
            f.before.unwrap_or(i64::MAX),
            f.since,
            f.kind.map(|k| k.as_str()),
            f.member.map(|m| m as i64),
            f.outcome.map(|o| o.as_str()),
            // One more than asked for, to learn whether there is another page.
            (f.limit + 1) as i64,
        ],
        flag_from,
    )?;
    let mut all: Vec<Flag> = rows.collect::<rusqlite::Result<_>>()?;
    let next_before = (all.len() > f.limit).then(|| all[f.limit - 1].id);
    all.truncate(f.limit);
    Ok(Page { rows: all, next_before })
}

/// The counts the panel puts at the top of the page.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Totals {
    pub spam: i64,
    pub ai: i64,
    pub deleted: i64,
    pub dismissed: i64,
    pub untouched: i64,
    /// AI flags a moderator has actually judged, either way.
    pub ai_decided: i64,
    /// AI flags a moderator said were wrong. The headline figure.
    pub ai_dismissed: i64,
}

impl Totals {
    /// How often moderators said "not AI", of the AI flags they judged, as a
    /// percentage. `None` while nobody has judged one yet — an unjudged feature
    /// has no accuracy, and guessing one would be dishonest.
    pub fn wrong_pct(&self) -> Option<f64> {
        (self.ai_decided > 0).then(|| self.ai_dismissed as f64 * 100.0 / self.ai_decided as f64)
    }
}

/// Every count over the period, whatever the page is filtered to.
pub fn totals(conn: &Connection, since: i64) -> rusqlite::Result<Totals> {
    let mut t = Totals::default();
    let mut stmt = conn.prepare_cached("SELECT kind, outcome, COUNT(*) FROM flags WHERE ts >= ?1 GROUP BY kind, outcome")?;
    let rows = stmt.query_map(params![since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))?;
    for row in rows {
        let (kind, outcome, n) = row?;
        match Kind::from_str(&kind) {
            Some(Kind::Spam) => t.spam += n,
            Some(Kind::Ai) => t.ai += n,
            None => {}
        }
        match Outcome::from_str(&outcome) {
            Some(Outcome::Deleted) => t.deleted += n,
            Some(Outcome::Dismissed) => t.dismissed += n,
            Some(Outcome::Untouched) | None => t.untouched += n,
        }
        if Kind::from_str(&kind) == Some(Kind::Ai) {
            match Outcome::from_str(&outcome) {
                Some(Outcome::Dismissed) => {
                    t.ai_decided += n;
                    t.ai_dismissed += n;
                }
                Some(Outcome::Deleted) => t.ai_decided += n,
                _ => {}
            }
        }
    }
    Ok(t)
}

/// One member's counts, for the per-member list on the panel.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MemberCount {
    pub member_id: u64,
    pub member_name: String,
    pub spam: i64,
    pub ai: i64,
    pub dismissed: i64,
}

/// Who has been flagged over the period, most flags first.
pub fn by_member(conn: &Connection, since: i64, limit: usize) -> rusqlite::Result<Vec<MemberCount>> {
    let mut stmt = conn.prepare_cached(
        "SELECT member_id, MAX(member_name),
                SUM(kind = 'spam'), SUM(kind = 'ai'), SUM(outcome = 'dismissed')
         FROM flags WHERE ts >= ?1 GROUP BY member_id ORDER BY COUNT(*) DESC, member_id LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![since, limit as i64], |r| {
        Ok(MemberCount {
            member_id: r.get::<_, i64>(0)? as u64,
            member_name: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            spam: r.get(2)?,
            ai: r.get(3)?,
            dismissed: r.get(4)?,
        })
    })?;
    rows.collect()
}

// --- style totals -------------------------------------------------------------------

/// Adds one message to a member's style totals, keeping a sample when the
/// message is a useful example of their ordinary writing.
pub fn add_style(conn: &Connection, u: &StyleUpdate) -> rusqlite::Result<()> {
    let existing: Option<String> =
        conn.query_row("SELECT samples_json FROM style WHERE member_id = ?1", params![u.member_id as i64], |r| r.get(0)).optional()?;
    let samples = match &u.sample {
        Some(text) => {
            let mut list = samples_of(existing.as_deref().unwrap_or("[]"));
            list.push(text.chars().take(SAMPLE_CHARS).collect());
            while list.len() > MAX_SAMPLES {
                list.remove(0);
            }
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
        }
        None => existing.unwrap_or_else(|| "[]".into()),
    };
    conn.execute(
        "INSERT INTO style (member_id, messages, chars, punct, emoji, tokens, hinglish_tokens, sentences, caps_starts, samples_json, updated_ts)
         VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(member_id) DO UPDATE SET
             messages = messages + 1,
             chars = chars + excluded.chars,
             punct = punct + excluded.punct,
             emoji = emoji + excluded.emoji,
             tokens = tokens + excluded.tokens,
             hinglish_tokens = hinglish_tokens + excluded.hinglish_tokens,
             sentences = sentences + excluded.sentences,
             caps_starts = caps_starts + excluded.caps_starts,
             samples_json = excluded.samples_json,
             updated_ts = excluded.updated_ts",
        params![
            u.member_id as i64,
            u.chars,
            u.punct,
            u.emoji,
            u.tokens,
            u.hinglish_tokens,
            u.sentences,
            u.caps_starts,
            samples,
            u.ts
        ],
    )?;
    Ok(())
}

/// A member's style totals, or nothing at all when the bot has never seen them
/// write. A member with little history gets no style score — see
/// [`super::automod::baseline`].
pub fn style(conn: &Connection, member: u64) -> rusqlite::Result<Option<StyleRow>> {
    conn.query_row(
        "SELECT messages, chars, punct, emoji, tokens, hinglish_tokens, sentences, caps_starts, samples_json
         FROM style WHERE member_id = ?1",
        params![member as i64],
        |r| {
            Ok(StyleRow {
                messages: r.get(0)?,
                chars: r.get::<_, i64>(1)? as u64,
                punct: r.get::<_, i64>(2)? as u64,
                emoji: r.get::<_, i64>(3)? as u64,
                tokens: r.get::<_, i64>(4)? as u64,
                hinglish_tokens: r.get::<_, i64>(5)? as u64,
                sentences: r.get::<_, i64>(6)? as u64,
                caps_starts: r.get::<_, i64>(7)? as u64,
                samples: samples_of(&r.get::<_, String>(8)?),
            })
        },
    )
    .optional()
}

// --- the retention cut ----------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PurgeReport {
    /// Flags whose text was cleared.
    pub cleared: usize,
    /// Members whose kept samples were dropped for being stale.
    pub samples: usize,
}

/// Clears flagged text older than `keep_days`, and the kept writing samples of
/// members nobody has seen for that long.
///
/// The rows themselves stay: they are the record of what this feature did and
/// how often a moderator said it was wrong, and that number must not quietly
/// shrink. What goes is the members' own words, which is the part there is no
/// reason to keep.
pub fn purge(conn: &Connection, now: i64, keep_days: i64) -> rusqlite::Result<PurgeReport> {
    let cut = now - keep_days * DAY;
    let cleared = conn.execute(
        "UPDATE flags SET text = '', text_cleared = 1, model_json = NULL WHERE ts < ?1 AND text_cleared = 0",
        params![cut],
    )?;
    let samples = conn.execute(
        "UPDATE style SET samples_json = '[]' WHERE updated_ts < ?1 AND samples_json != '[]'",
        params![cut],
    )?;
    Ok(PurgeReport { cleared, samples })
}

// --- live wiring -----------------------------------------------------------------------

/// What the writer thread applies.
#[derive(Clone, Debug)]
pub enum Event {
    Style(StyleUpdate),
    Purge { now: i64, keep_days: i64 },
}

/// Style updates dropped because the queue was full.
pub static DROPPED: AtomicU64 = AtomicU64::new(0);

static TX: OnceLock<mpsc::Sender<Event>> = OnceLock::new();
static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

/// The connection flags, decisions and the panel's reads go through. Take the
/// lock inside `spawn_blocking` only: never across an `.await`.
pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

pub fn started() -> bool {
    DB.get().is_some()
}

/// Queues a style update without waiting. A full queue drops it and counts the drop.
pub fn push(event: Event) {
    let Some(tx) = TX.get() else { return };
    match tx.try_send(event) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(_)) => {
            let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() || n % 1000 == 0 {
                tracing::warn!("automod: the style queue is full; {} updates dropped so far", n);
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {}
    }
}

/// Applies events in arrival order until every sender is gone.
pub fn run_writer(conn: Connection, mut rx: mpsc::Receiver<Event>) {
    while let Some(first) = rx.blocking_recv() {
        let mut batch = vec![first];
        while batch.len() < 400 {
            match rx.try_recv() {
                Ok(e) => batch.push(e),
                Err(_) => break,
            }
        }
        let _ = conn.execute_batch("BEGIN");
        for event in batch {
            let result = match event {
                Event::Style(u) => add_style(&conn, &u),
                Event::Purge { now, keep_days } => purge(&conn, now, keep_days).map(|r| {
                    if r != PurgeReport::default() {
                        tracing::info!("automod: cleared the text of {} old flags and {} members' writing samples", r.cleared, r.samples);
                    }
                }),
            };
            if let Err(err) = result {
                tracing::warn!("automod: couldn't store an event: {}", err);
            }
        }
        if let Err(err) = conn.execute_batch("COMMIT") {
            tracing::warn!("automod: couldn't commit: {}", err);
            let _ = conn.execute_batch("ROLLBACK");
        }
    }
}

/// Opens the store and starts the writer. Call once, from inside the runtime.
pub fn start(workspace: &str) -> anyhow::Result<()> {
    if DB.get().is_some() {
        return Ok(());
    }
    let path = crate::utils::build_path(workspace, &[".runtime"]).join("automod.db");
    let writer = open_at(&path)?;
    let shared = open_at(&path)?;
    if DB.set(Mutex::new(shared)).is_err() {
        return Ok(());
    }
    let (tx, rx) = mpsc::channel(QUEUE);
    if TX.set(tx).is_err() {
        return Ok(());
    }
    std::thread::Builder::new().name("automod-writer".into()).spawn(move || {
        run_writer(writer, rx);
        tracing::warn!("automod: the writer stopped");
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_789_000_000;

    fn spam_flag(message: u64, member: u64, ts: i64) -> NewFlag {
        NewFlag {
            kind: Kind::Spam,
            rule: "repeat".into(),
            member_id: member,
            member_name: "Rohan".into(),
            channel_id: 22,
            channel_name: "memes".into(),
            guild_id: 900,
            message_id: message,
            ts,
            text: "FREE NITRO claim yours".into(),
            messages: 3,
            score: None,
            reasons: vec!["the same message 3 times in 47 seconds".into()],
            model: None,
            model_asked: false,
            had_baseline: false,
            outcome: Outcome::Deleted,
        }
    }

    fn ai_flag(message: u64, member: u64, ts: i64) -> NewFlag {
        NewFlag {
            kind: Kind::Ai,
            rule: "ai".into(),
            member_id: member,
            member_name: "Zoya".into(),
            channel_id: 21,
            channel_name: "general".into(),
            guild_id: 900,
            message_id: message,
            ts,
            text: "Moreover, it is worth noting that the tapestry of this discussion...".into(),
            messages: 1,
            score: Some(0.71),
            reasons: vec!["headings and bullet lists in chat".into()],
            model: None,
            model_asked: false,
            had_baseline: true,
            outcome: Outcome::Untouched,
        }
    }

    #[test]
    fn a_flag_is_written_once_and_read_back_whole() {
        let conn = open_memory().unwrap();
        let id = add_flag(&conn, &spam_flag(5001, 1003, NOW)).unwrap().expect("first insert");
        // The same message and kind again is ignored, so nothing is logged twice.
        assert_eq!(add_flag(&conn, &spam_flag(5001, 1003, NOW)).unwrap(), None);
        let got = flag(&conn, id).unwrap().unwrap();
        assert_eq!((got.kind, got.rule.as_str(), got.member_id, got.messages), (Kind::Spam, "repeat", 1003, 3));
        assert_eq!(got.outcome, Outcome::Deleted);
        assert_eq!(got.reasons, vec!["the same message 3 times in 47 seconds".to_string()]);
        assert!(!got.text_cleared);
        // An AI flag about the same message is a different kind, so it fits.
        assert!(add_flag(&conn, &ai_flag(5001, 1003, NOW)).unwrap().is_some());
        assert!(already_judged(&conn, 5001).unwrap());
        assert!(!already_judged(&conn, 9999).unwrap());
    }

    #[test]
    fn every_press_is_kept_even_when_a_mod_changes_their_mind() {
        let conn = open_memory().unwrap();
        let id = add_flag(&conn, &ai_flag(6001, 1004, NOW)).unwrap().unwrap();
        assert_eq!(flag(&conn, id).unwrap().unwrap().outcome, Outcome::Untouched);
        assert!(decide(&conn, id, Outcome::Dismissed, 1001, NOW + 60).unwrap());
        assert!(decide(&conn, id, Outcome::Deleted, 1002, NOW + 120).unwrap());
        let row = flag(&conn, id).unwrap().unwrap();
        assert_eq!((row.outcome, row.decided_by, row.decided_ts), (Outcome::Deleted, Some(1002), Some(NOW + 120)));
        let history = decisions_for(&conn, id).unwrap();
        assert_eq!(history.len(), 2, "both presses are on the record");
        assert_eq!((history[0].outcome, history[0].by), (Outcome::Dismissed, 1001));
        assert_eq!((history[1].outcome, history[1].by), (Outcome::Deleted, 1002));
        // A press about a flag that isn't there records nothing.
        assert!(!decide(&conn, 4242, Outcome::Dismissed, 1001, NOW).unwrap());
        assert!(decisions_for(&conn, 4242).unwrap().is_empty());
    }

    #[test]
    fn the_wrong_count_is_only_about_ai_flags_a_mod_judged() {
        let conn = open_memory().unwrap();
        // Two spam removals, three AI flags: one dismissed, one deleted, one untouched.
        for (i, m) in [(1u64, 1003u64), (2, 1003)] {
            add_flag(&conn, &spam_flag(7000 + i, m, NOW)).unwrap();
        }
        let a = add_flag(&conn, &ai_flag(7100, 1004, NOW)).unwrap().unwrap();
        let b = add_flag(&conn, &ai_flag(7101, 1005, NOW)).unwrap().unwrap();
        add_flag(&conn, &ai_flag(7102, 1005, NOW)).unwrap().unwrap();
        decide(&conn, a, Outcome::Dismissed, 1001, NOW).unwrap();
        decide(&conn, b, Outcome::Deleted, 1001, NOW).unwrap();
        let t = totals(&conn, NOW - DAY).unwrap();
        assert_eq!((t.spam, t.ai), (2, 3));
        assert_eq!((t.deleted, t.dismissed, t.untouched), (3, 1, 1));
        assert_eq!((t.ai_decided, t.ai_dismissed), (2, 1));
        assert_eq!(t.wrong_pct(), Some(50.0));
        // Nothing judged yet means no accuracy to report, not a flattering zero.
        let fresh = open_memory().unwrap();
        add_flag(&fresh, &ai_flag(7200, 1004, NOW)).unwrap();
        assert_eq!(totals(&fresh, NOW - DAY).unwrap().wrong_pct(), None);
    }

    #[test]
    fn the_list_filters_and_pages_newest_first() {
        let conn = open_memory().unwrap();
        for i in 0..7u64 {
            add_flag(&conn, &spam_flag(8000 + i, 1003, NOW - i as i64 * 60)).unwrap();
        }
        let ai = add_flag(&conn, &ai_flag(8100, 1004, NOW)).unwrap().unwrap();
        decide(&conn, ai, Outcome::Dismissed, 1001, NOW).unwrap();
        // Old enough to fall outside a one-day period.
        add_flag(&conn, &spam_flag(8200, 1003, NOW - 10 * DAY)).unwrap();

        let ask = |f: ListFilter| list(&conn, &f).unwrap();
        let recent = ask(ListFilter { since: NOW - DAY, limit: 50, ..Default::default() });
        assert_eq!(recent.rows.len(), 8, "the ten-day-old one is outside the period");
        assert!(recent.rows[0].id > recent.rows[1].id, "newest first");
        assert_eq!(recent.next_before, None);

        let first = ask(ListFilter { since: NOW - DAY, limit: 3, ..Default::default() });
        assert_eq!(first.rows.len(), 3);
        let carry = first.next_before.expect("another page");
        let second = ask(ListFilter { since: NOW - DAY, limit: 3, before: Some(carry), ..Default::default() });
        assert_eq!(second.rows.len(), 3);
        assert!(second.rows.iter().all(|r| r.id < carry));

        assert_eq!(ask(ListFilter { since: NOW - DAY, limit: 50, kind: Some(Kind::Ai), ..Default::default() }).rows.len(), 1);
        assert_eq!(ask(ListFilter { since: NOW - DAY, limit: 50, member: Some(1004), ..Default::default() }).rows.len(), 1);
        assert_eq!(
            ask(ListFilter { since: NOW - DAY, limit: 50, outcome: Some(Outcome::Dismissed), ..Default::default() }).rows.len(),
            1
        );
        assert!(ask(ListFilter { since: NOW - DAY, limit: 50, member: Some(4242), ..Default::default() }).rows.is_empty());

        let people = by_member(&conn, NOW - DAY, 10).unwrap();
        assert_eq!(people[0].member_id, 1003);
        assert_eq!((people[0].spam, people[0].ai), (7, 0));
        assert_eq!((people[1].member_id, people[1].ai, people[1].dismissed), (1004, 1, 1));
    }

    #[test]
    fn style_totals_add_up_and_keep_a_few_samples() {
        let conn = open_memory().unwrap();
        let mut update = |sample: Option<&str>, chars: u32| StyleUpdate {
            member_id: 1003,
            ts: NOW,
            chars,
            punct: 2,
            emoji: 1,
            tokens: 6,
            hinglish_tokens: 3,
            sentences: 1,
            caps_starts: 0,
            sample: sample.map(String::from),
        };
        assert_eq!(style(&conn, 1003).unwrap(), None, "a member never seen has no totals at all");
        for i in 0..12 {
            add_style(&conn, &update(Some(&format!("bhai message number {}", i)), 30)).unwrap();
        }
        // A message too short to be a useful sample still counts in the totals.
        add_style(&conn, &update(None, 4)).unwrap();
        let row = style(&conn, 1003).unwrap().unwrap();
        assert_eq!(row.messages, 13);
        assert_eq!(row.chars, 12 * 30 + 4);
        assert_eq!((row.punct, row.emoji, row.tokens, row.hinglish_tokens), (26, 13, 78, 39));
        assert_eq!(row.samples.len(), MAX_SAMPLES, "the newest few are kept");
        assert_eq!(row.samples.last().unwrap(), "bhai message number 11");
        assert_eq!(row.samples.first().unwrap(), "bhai message number 4");
    }

    #[test]
    fn the_retention_cut_clears_old_words_but_keeps_the_record() {
        let conn = open_memory().unwrap();
        let old = add_flag(&conn, &ai_flag(9001, 1004, NOW - 40 * DAY)).unwrap().unwrap();
        decide(&conn, old, Outcome::Dismissed, 1001, NOW - 40 * DAY).unwrap();
        let new = add_flag(&conn, &spam_flag(9002, 1003, NOW - 2 * DAY)).unwrap().unwrap();
        add_style(&conn, &StyleUpdate {
            member_id: 1004,
            ts: NOW - 40 * DAY,
            chars: 30,
            punct: 1,
            emoji: 0,
            tokens: 5,
            hinglish_tokens: 1,
            sentences: 1,
            caps_starts: 1,
            sample: Some("an old sample".into()),
        })
        .unwrap();

        let report = purge(&conn, NOW, 30).unwrap();
        assert_eq!((report.cleared, report.samples), (1, 1));
        let gone = flag(&conn, old).unwrap().unwrap();
        assert!(gone.text.is_empty() && gone.text_cleared, "the words go");
        assert_eq!(gone.outcome, Outcome::Dismissed, "but what a mod decided stays");
        assert_eq!(totals(&conn, NOW - 90 * DAY).unwrap().ai_dismissed, 1, "the wrong count survives the cut");
        assert_eq!(style(&conn, 1004).unwrap().unwrap().samples.len(), 0);
        assert!(!flag(&conn, new).unwrap().unwrap().text.is_empty(), "recent text is left alone");
        // Running it again clears nothing twice.
        assert_eq!(purge(&conn, NOW, 30).unwrap(), PurgeReport::default());
    }

    #[test]
    fn model_calls_are_counted_from_the_rows_themselves() {
        let conn = open_memory().unwrap();
        let asked = |message: u64, ts: i64| {
            let mut f = ai_flag(message, 1004, ts);
            f.model_asked = true;
            f.model = Some(ModelSay { model: "lite".into(), confidence: 0.6, reasons: "because".into(), unjudgeable: false });
            f
        };
        add_flag(&conn, &asked(9100, NOW - 30)).unwrap();
        add_flag(&conn, &asked(9101, NOW - 4000)).unwrap();
        // Flagged on the free signals alone: the model was never asked.
        add_flag(&conn, &ai_flag(9102, 1004, NOW)).unwrap();
        // Asked, but the answer never came back. It still cost a call, so it
        // still counts against the hour's budget.
        let mut failed = ai_flag(9103, 1004, NOW - 60);
        failed.model_asked = true;
        add_flag(&conn, &failed).unwrap();
        assert_eq!(model_calls_since(&conn, NOW - 3600).unwrap(), 2);
        assert_eq!(model_calls_since(&conn, NOW - 7200).unwrap(), 3);
    }
}
