//! The deleted and edited message log, shown on the panel only.
//!
//! Every new member message in a server channel the bot receives (text
//! channels, voice channel chats including temporary rooms, threads) is kept
//! for a short while: its text, who sent it, and its pictures, downloaded at
//! once because Discord's copy goes when the message does. When a message is
//! deleted its copy moves to the log, pictures included; when it is edited the
//! text before and after goes to the log. Copies of messages nobody deletes are
//! cleared after `VIZIER_MSGLOG_KEEP_DAYS`, log entries after
//! `VIZIER_MSGLOG_LOG_DAYS`.
//!
//! Never #safe-corner or a thread inside it, never DMs, never bots or webhooks,
//! and never a channel the cache can't place (it could be a thread in
//! #safe-corner). Discord doesn't tell bots who deleted a message.
//!
//! Nothing here blocks the gateway: the handlers queue events on a bounded
//! channel (dropping, with a count, when it is full) and one writer thread owns
//! the database, applying events in the order they arrived. Pictures download in
//! their own tasks and come back through the same queue, so a picture that lands
//! after its message was deleted still goes with the deleted copy.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serenity::all::{Context, GuildId, Message, MessageId, MessageType, MessageUpdateEvent};
use tokio::sync::mpsc;

/// Pictures kept from one message.
pub const MAX_IMAGES: usize = 4;
/// Events waiting for the writer before new ones are dropped.
pub const QUEUE: usize = 8192;
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15);
const DOWNLOADS_AT_ONCE: usize = 3;
const PURGE_EVERY: Duration = Duration::from_secs(3600);
const REPLY_CHARS: usize = 200;
const DAY_MS: i64 = 86_400_000;
/// Discord's epoch, for the time inside a message id.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;
/// Picture types kept, by extension. No SVG: it can carry script.
const IMAGE_EXTS: &[&str] = &["png", "jpg", "gif", "webp", "avif", "bmp"];

// --- settings ------------------------------------------------------------------------

pub fn enabled() -> bool {
    super::control::on("VIZIER_MSGLOG", true)
}

pub fn keep_days() -> i64 {
    super::control::number("VIZIER_MSGLOG_KEEP_DAYS", 7).clamp(1, 90) as i64
}

pub fn log_days() -> i64 {
    super::control::number("VIZIER_MSGLOG_LOG_DAYS", 30).clamp(1, 365) as i64
}

pub fn max_image_bytes() -> u64 {
    super::control::number("VIZIER_MSGLOG_MAX_IMAGE_MB", 8).clamp(1, 25) * 1024 * 1024
}

pub fn max_disk_bytes() -> u64 {
    super::control::number("VIZIER_MSGLOG_MAX_GB", 5).clamp(1, 500) * 1024 * 1024 * 1024
}

// --- what is queued --------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub id: u64,
    pub filename: String,
    pub content_type: Option<String>,
    pub size: u64,
    /// Discord's link; only used to download, never shown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
}

/// A picture saved to disk, relative to the log's folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredFile {
    /// Which of the message's kept pictures (0-3).
    pub n: usize,
    pub path: String,
    pub bytes: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub channel_id: u64,
    /// The channel a thread is in.
    pub parent_id: Option<u64>,
    pub channel_name: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NewMessage {
    pub message_id: u64,
    pub place: Place,
    pub guild_id: u64,
    pub author_id: u64,
    pub author_name: String,
    pub avatar: String,
    pub content: String,
    pub created_ms: i64,
    pub reply_to: Option<u64>,
    pub reply_author: Option<String>,
    pub reply_text: Option<String>,
    pub attachments: Vec<Attachment>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Deletion {
    pub ids: Vec<u64>,
    pub place: Place,
    pub ts_ms: i64,
    pub bulk: bool,
}

#[derive(Clone, Debug)]
pub enum Event {
    New(NewMessage),
    /// A bot's or webhook's message: remembered so its deletion isn't logged.
    Other { message_id: u64, ts_ms: i64 },
    Deleted(Deletion),
    Edited { message_id: u64, content: String, ts_ms: i64 },
    /// A picture finished downloading.
    Saved { message_id: u64, file: StoredFile },
    Purge { now_ms: i64 },
}

// --- deciding what to keep ---------------------------------------------------------------

/// Whether a channel is never logged: #safe-corner (by id or name), anything
/// listed as sensitive, and threads inside either.
pub fn excluded(place: &Place, parent_name: Option<&str>, sensitive: &[u64]) -> bool {
    let safe = |id: u64, name: &str| sensitive.contains(&id) || super::weekly::is_safe_corner(id, name);
    safe(place.channel_id, &place.channel_name) || place.parent_id.is_some_and(|p| safe(p, parent_name.unwrap_or("")))
}

/// What becomes of one message the gateway delivered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Capture {
    Skip,
    /// A bot or webhook: only its id is remembered.
    Other,
    Keep,
}

/// DMs, excluded and unknown channels are skipped; bots and webhooks only noted.
pub fn classify(in_guild: bool, bot: bool, webhook: bool, place_ok: bool) -> Capture {
    if !in_guild || !place_ok {
        Capture::Skip
    } else if bot || webhook {
        Capture::Other
    } else {
        Capture::Keep
    }
}

/// The extension a picture is saved under, when it is a picture worth keeping.
pub fn image_ext(content_type: Option<&str>, filename: &str) -> Option<&'static str> {
    let by_name = || {
        let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
        let ext = if ext == "jpeg" { "jpg".to_string() } else { ext };
        IMAGE_EXTS.iter().find(|e| **e == ext).copied()
    };
    match content_type.map(|t| t.split(';').next().unwrap_or("").trim().to_ascii_lowercase()) {
        Some(t) if !t.is_empty() => {
            let sub = t.strip_prefix("image/")?;
            let sub = if sub == "jpeg" || sub == "pjpeg" { "jpg" } else { sub };
            IMAGE_EXTS.iter().find(|e| **e == sub).copied()
        }
        _ => by_name(),
    }
}

/// The attachments to download: pictures no bigger than `max_bytes`, at most four.
pub fn pick_images(attachments: &[Attachment], max_bytes: u64) -> Vec<(usize, &Attachment, &'static str)> {
    attachments
        .iter()
        .filter(|a| a.size > 0 && a.size <= max_bytes)
        .filter_map(|a| image_ext(a.content_type.as_deref(), &a.filename).map(|ext| (a, ext)))
        .take(MAX_IMAGES)
        .enumerate()
        .map(|(n, (a, ext))| (n, a, ext))
        .collect()
}

/// When a message was sent, from its id.
pub fn snowflake_ms(id: u64) -> i64 {
    (id >> 22) as i64 + DISCORD_EPOCH_MS
}

/// "2026-09-16" for a moment, UTC: the folder a new message's pictures go in.
pub fn day_folder(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).unwrap_or_default().format("%Y-%m-%d").to_string()
}

/// A stored relative path that stays inside the log's folder: one or two plain parts.
pub fn safe_rel(path: &str) -> bool {
    let p = Path::new(path);
    let parts: Vec<Component> = p.components().collect();
    !path.is_empty() && !path.contains('\\') && (1..=2).contains(&parts.len()) && parts.iter().all(|c| matches!(c, Component::Normal(_)))
}

fn short(text: &str, limit: usize) -> String {
    let flat = text.replace('\n', " ");
    let flat = flat.trim();
    if flat.chars().count() > limit { format!("{}…", flat.chars().take(limit).collect::<String>()) } else { flat.to_string() }
}

// --- the store ----------------------------------------------------------------------------

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS recent (
        message_id INTEGER PRIMARY KEY, channel_id INTEGER NOT NULL, parent_id INTEGER, channel_name TEXT NOT NULL DEFAULT '',
        author_id INTEGER NOT NULL, author_name TEXT NOT NULL, avatar TEXT NOT NULL DEFAULT '', content TEXT NOT NULL,
        created_ts INTEGER NOT NULL, reply_to INTEGER, reply_author TEXT, reply_text TEXT,
        attachments_json TEXT NOT NULL DEFAULT '[]', stored_files_json TEXT NOT NULL DEFAULT '[]');
    CREATE INDEX IF NOT EXISTS recent_created ON recent (created_ts);
    CREATE TABLE IF NOT EXISTS others (message_id INTEGER PRIMARY KEY, ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS others_ts ON others (ts);
    CREATE TABLE IF NOT EXISTS deleted (
        id INTEGER PRIMARY KEY AUTOINCREMENT, message_id INTEGER NOT NULL UNIQUE, channel_id INTEGER NOT NULL, parent_id INTEGER,
        channel_name TEXT NOT NULL DEFAULT '', author_id INTEGER, author_name TEXT, avatar TEXT, content TEXT,
        created_ts INTEGER NOT NULL, deleted_ts INTEGER NOT NULL, reply_to INTEGER, reply_author TEXT, reply_text TEXT,
        attachments_json TEXT NOT NULL DEFAULT '[]', stored_files_json TEXT NOT NULL DEFAULT '[]',
        bulk INTEGER NOT NULL DEFAULT 0, reason TEXT);
    CREATE INDEX IF NOT EXISTS deleted_ts ON deleted (deleted_ts);
    CREATE INDEX IF NOT EXISTS deleted_author ON deleted (author_id);
    CREATE TABLE IF NOT EXISTS edited (
        id INTEGER PRIMARY KEY AUTOINCREMENT, message_id INTEGER NOT NULL, channel_id INTEGER NOT NULL, parent_id INTEGER,
        channel_name TEXT NOT NULL DEFAULT '', author_id INTEGER NOT NULL, author_name TEXT NOT NULL, avatar TEXT NOT NULL DEFAULT '',
        before TEXT NOT NULL, after TEXT NOT NULL, created_ts INTEGER NOT NULL, edited_ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS edited_ts ON edited (edited_ts);
    CREATE INDEX IF NOT EXISTS edited_author ON edited (author_id);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

/// Why a deleted message has no copy.
pub const REASON_BEFORE: &str = "before_logging";
pub const REASON_EXPIRED: &str = "expired";
pub const REASON_MISSED: &str = "missed";

/// A picture to download for a new message.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub message_id: u64,
    pub n: usize,
    pub url: String,
    pub rel: String,
    pub name: String,
    pub max_bytes: u64,
    pub expected: u64,
}

pub struct Store {
    conn: Connection,
    root: PathBuf,
    /// Bytes the pictures take (planned downloads included), recounted at each purge.
    pub used: u64,
    /// Whether the disk guard has already said it is full.
    pub full_logged: bool,
    /// When logging first started, ever.
    pub first_started_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PurgeReport {
    pub recent: usize,
    pub others: usize,
    pub deleted: usize,
    pub edited: usize,
    pub files: usize,
    pub folders: usize,
}

struct Kept {
    channel_id: i64,
    parent_id: Option<i64>,
    channel_name: String,
    author_id: i64,
    author_name: String,
    avatar: String,
    content: String,
    created_ts: i64,
    reply_to: Option<i64>,
    reply_author: Option<String>,
    reply_text: Option<String>,
    attachments_json: String,
    stored_files_json: String,
}

fn files_of(json: &str) -> Vec<StoredFile> {
    serde_json::from_str(json).unwrap_or_default()
}

impl Store {
    /// Opens (or makes) the database at `db` and the picture folder `root`.
    pub fn open(db: &Path, root: PathBuf, now_ms: i64) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&root)?;
        if let Some(dir) = db.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let conn = Connection::open(db)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        conn.execute_batch(SCHEMA)?;
        conn.execute("INSERT OR IGNORE INTO meta (key, value) VALUES ('first_started_ms', ?1)", params![now_ms.to_string()])?;
        let first_started_ms = conn
            .query_row("SELECT value FROM meta WHERE key = 'first_started_ms'", [], |r| r.get::<_, String>(0))?
            .parse()
            .unwrap_or(now_ms);
        let used = dir_size(&root);
        Ok(Self { conn, root, used, full_logged: false, first_started_ms })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn kept(&self, id: u64) -> rusqlite::Result<Option<Kept>> {
        self.conn
            .query_row(
                "SELECT channel_id, parent_id, channel_name, author_id, author_name, avatar, content, created_ts, reply_to, reply_author, reply_text,
                        attachments_json, stored_files_json FROM recent WHERE message_id = ?1",
                params![id as i64],
                |r| {
                    Ok(Kept {
                        channel_id: r.get(0)?,
                        parent_id: r.get(1)?,
                        channel_name: r.get(2)?,
                        author_id: r.get(3)?,
                        author_name: r.get(4)?,
                        avatar: r.get(5)?,
                        content: r.get(6)?,
                        created_ts: r.get(7)?,
                        reply_to: r.get(8)?,
                        reply_author: r.get(9)?,
                        reply_text: r.get(10)?,
                        attachments_json: r.get(11)?,
                        stored_files_json: r.get(12)?,
                    })
                },
            )
            .optional()
    }

    /// Keeps a new message. A repeat delivery changes nothing.
    pub fn insert_new(&mut self, m: &NewMessage) -> rusqlite::Result<bool> {
        let (mut reply_author, mut reply_text) = (m.reply_author.clone(), m.reply_text.clone());
        if let (Some(to), None) = (m.reply_to, &reply_author) {
            if let Some(k) = self.kept(to)? {
                reply_author = Some(k.author_name);
                reply_text = Some(short(&k.content, REPLY_CHARS));
            }
        }
        let attachments: Vec<Attachment> = m.attachments.iter().map(|a| Attachment { url: String::new(), ..a.clone() }).collect();
        let added = self.conn.execute(
            "INSERT OR IGNORE INTO recent (message_id, channel_id, parent_id, channel_name, author_id, author_name, avatar, content, created_ts,
                 reply_to, reply_author, reply_text, attachments_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                m.message_id as i64,
                m.place.channel_id as i64,
                m.place.parent_id.map(|p| p as i64),
                m.place.channel_name,
                m.author_id as i64,
                m.author_name,
                m.avatar,
                m.content,
                m.created_ms,
                m.reply_to.map(|r| r as i64),
                reply_author,
                reply_text,
                serde_json::to_string(&attachments).unwrap_or_else(|_| "[]".into()),
            ],
        )?;
        Ok(added > 0)
    }

    /// The downloads for a message just kept, unless the pictures folder is over
    /// `max_disk`: then nothing is downloaded (and it is logged once) until a
    /// purge makes room.
    pub fn plan(&mut self, m: &NewMessage, max_image: u64, max_disk: u64) -> Vec<Job> {
        let picks = pick_images(&m.attachments, max_image);
        if picks.is_empty() {
            return Vec::new();
        }
        let total: u64 = picks.iter().map(|(_, a, _)| a.size).sum();
        if self.used + total > max_disk {
            if !self.full_logged {
                self.full_logged = true;
                tracing::warn!(
                    "msglog: the pictures folder is at its limit ({} MB of {} MB); not downloading new pictures until old ones are cleared",
                    self.used / (1024 * 1024),
                    max_disk / (1024 * 1024)
                );
            }
            return Vec::new();
        }
        self.used += total;
        let day = day_folder(m.created_ms);
        picks
            .into_iter()
            .map(|(n, a, ext)| Job {
                message_id: m.message_id,
                n,
                url: a.url.clone(),
                rel: format!("{}/{}_{}.{}", day, m.message_id, n, ext),
                name: a.filename.clone(),
                max_bytes: max_image,
                expected: a.size,
            })
            .collect()
    }

    pub fn note_other(&mut self, id: u64, ts_ms: i64) -> rusqlite::Result<()> {
        self.conn.execute("INSERT OR IGNORE INTO others (message_id, ts) VALUES (?1, ?2)", params![id as i64, ts_ms])?;
        Ok(())
    }

    /// Moves pictures into `deleted/`, returning where they are now.
    fn move_to_deleted(&self, files: &[StoredFile]) -> Vec<StoredFile> {
        let dir = self.root.join("deleted");
        if !files.is_empty() {
            let _ = std::fs::create_dir_all(&dir);
        }
        files
            .iter()
            .filter(|f| safe_rel(&f.path))
            .filter_map(|f| {
                let name = Path::new(&f.path).file_name()?.to_string_lossy().to_string();
                let rel = format!("deleted/{}", name);
                if f.path != rel {
                    let from = self.root.join(&f.path);
                    let to = dir.join(&name);
                    if std::fs::rename(&from, &to).is_err() {
                        std::fs::copy(&from, &to).ok()?;
                        let _ = std::fs::remove_file(&from);
                    }
                }
                Some(StoredFile { path: rel, ..f.clone() })
            })
            .collect()
    }

    /// Logs deleted messages: a kept copy moves to the log with its pictures; a
    /// bot's message is forgotten; anything else becomes a row without text,
    /// saying why there is no copy. Returns the rows added to the log.
    pub fn delete(&mut self, d: &Deletion, keep_days: i64) -> rusqlite::Result<usize> {
        let mut added = 0;
        for &id in &d.ids {
            if let Some(k) = self.kept(id)? {
                let files = self.move_to_deleted(&files_of(&k.stored_files_json));
                added += self.conn.execute(
                    "INSERT OR IGNORE INTO deleted (message_id, channel_id, parent_id, channel_name, author_id, author_name, avatar, content,
                         created_ts, deleted_ts, reply_to, reply_author, reply_text, attachments_json, stored_files_json, bulk)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                    params![
                        id as i64,
                        k.channel_id,
                        k.parent_id,
                        k.channel_name,
                        k.author_id,
                        k.author_name,
                        k.avatar,
                        k.content,
                        k.created_ts,
                        d.ts_ms,
                        k.reply_to,
                        k.reply_author,
                        k.reply_text,
                        k.attachments_json,
                        serde_json::to_string(&files).unwrap_or_else(|_| "[]".into()),
                        d.bulk
                    ],
                )?;
                self.conn.execute("DELETE FROM recent WHERE message_id = ?1", params![id as i64])?;
                continue;
            }
            // A bot's message stays noted until the purge, so a repeated event is ignored too.
            if self.conn.query_row("SELECT 1 FROM others WHERE message_id = ?1", params![id as i64], |_| Ok(())).optional()?.is_some() {
                continue;
            }
            let created = snowflake_ms(id);
            let reason = if created < self.first_started_ms {
                REASON_BEFORE
            } else if created < d.ts_ms - keep_days * DAY_MS {
                REASON_EXPIRED
            } else {
                REASON_MISSED
            };
            added += self.conn.execute(
                "INSERT OR IGNORE INTO deleted (message_id, channel_id, parent_id, channel_name, created_ts, deleted_ts, bulk, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![id as i64, d.place.channel_id as i64, d.place.parent_id.map(|p| p as i64), d.place.channel_name, created, d.ts_ms, d.bulk, reason],
            )?;
        }
        Ok(added)
    }

    /// Logs an edit when the text really changed. A message without a kept copy
    /// has no "before", so it isn't logged.
    pub fn edit(&mut self, id: u64, content: &str, ts_ms: i64) -> rusqlite::Result<bool> {
        let Some(k) = self.kept(id)? else { return Ok(false) };
        if k.content == content {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO edited (message_id, channel_id, parent_id, channel_name, author_id, author_name, avatar, before, after, created_ts, edited_ts)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![id as i64, k.channel_id, k.parent_id, k.channel_name, k.author_id, k.author_name, k.avatar, k.content, content, k.created_ts, ts_ms],
        )?;
        self.conn.execute("UPDATE recent SET content = ?2 WHERE message_id = ?1", params![id as i64, content])?;
        Ok(true)
    }

    /// A downloaded picture: added to its message's copy, moved along if the
    /// message was deleted meanwhile, removed if the message is gone from both.
    pub fn saved(&mut self, message_id: u64, file: StoredFile) -> rusqlite::Result<()> {
        if !safe_rel(&file.path) {
            return Ok(());
        }
        let push = |json: &str, f: StoredFile| {
            let mut list = files_of(json);
            list.retain(|x| x.n != f.n);
            list.push(f);
            list.sort_by_key(|x| x.n);
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
        };
        let recent: Option<String> =
            self.conn.query_row("SELECT stored_files_json FROM recent WHERE message_id = ?1", params![message_id as i64], |r| r.get(0)).optional()?;
        if let Some(json) = recent {
            self.conn.execute("UPDATE recent SET stored_files_json = ?2 WHERE message_id = ?1", params![message_id as i64, push(&json, file)])?;
            return Ok(());
        }
        let deleted: Option<String> =
            self.conn.query_row("SELECT stored_files_json FROM deleted WHERE message_id = ?1", params![message_id as i64], |r| r.get(0)).optional()?;
        match deleted {
            Some(json) => {
                if let Some(moved) = self.move_to_deleted(std::slice::from_ref(&file)).pop() {
                    self.conn.execute("UPDATE deleted SET stored_files_json = ?2 WHERE message_id = ?1", params![message_id as i64, push(&json, moved)])?;
                }
            }
            None => {
                let _ = std::fs::remove_file(self.root.join(&file.path));
            }
        }
        Ok(())
    }

    fn remove_files(&self, json: &str) -> usize {
        files_of(json).iter().filter(|f| safe_rel(&f.path) && std::fs::remove_file(self.root.join(&f.path)).is_ok()).count()
    }

    /// Clears copies older than `keep_days` and log entries older than
    /// `log_days`, with their pictures, then day folders with nothing left to
    /// keep, and recounts the space used.
    pub fn purge(&mut self, now_ms: i64, keep_days: i64, log_days: i64) -> rusqlite::Result<PurgeReport> {
        let mut report = PurgeReport::default();
        let keep_cut = now_ms - keep_days * DAY_MS;
        let log_cut = now_ms - log_days * DAY_MS;
        let old_files = |sql: &str, cut: i64| -> rusqlite::Result<Vec<String>> {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map(params![cut], |r| r.get::<_, String>(0))?;
            rows.collect()
        };
        for json in old_files("SELECT stored_files_json FROM recent WHERE created_ts < ?1 AND stored_files_json != '[]'", keep_cut)? {
            report.files += self.remove_files(&json);
        }
        report.recent = self.conn.execute("DELETE FROM recent WHERE created_ts < ?1", params![keep_cut])?;
        report.others = self.conn.execute("DELETE FROM others WHERE ts < ?1", params![keep_cut])?;
        for json in old_files("SELECT stored_files_json FROM deleted WHERE deleted_ts < ?1 AND stored_files_json != '[]'", log_cut)? {
            report.files += self.remove_files(&json);
        }
        report.deleted = self.conn.execute("DELETE FROM deleted WHERE deleted_ts < ?1", params![log_cut])?;
        report.edited = self.conn.execute("DELETE FROM edited WHERE edited_ts < ?1", params![log_cut])?;

        // Day folders: a whole day older than the copies are kept goes (a picture
        // that finished downloading after its message was cleared is in one), and
        // an empty one goes too.
        if let Ok(entries) = std::fs::read_dir(&self.root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Ok(day) = chrono::NaiveDate::parse_from_str(&name, "%Y-%m-%d") else { continue };
                if !entry.path().is_dir() {
                    continue;
                }
                let day_end = day.and_hms_opt(0, 0, 0).map(|t| t.and_utc().timestamp_millis() + DAY_MS).unwrap_or(i64::MAX);
                let empty = std::fs::read_dir(entry.path()).map(|mut d| d.next().is_none()).unwrap_or(false);
                if day_end <= keep_cut {
                    if std::fs::remove_dir_all(entry.path()).is_ok() {
                        report.folders += 1;
                    }
                } else if empty && std::fs::remove_dir(entry.path()).is_ok() {
                    report.folders += 1;
                }
            }
        }
        self.used = dir_size(&self.root);
        if self.full_logged && self.used < max_disk_bytes() {
            self.full_logged = false;
        }
        Ok(report)
    }

    /// Applies a batch of events in one transaction. Downloads to start go to `spawn`.
    pub fn apply(&mut self, events: Vec<Event>, spawn: &mut dyn FnMut(Job)) {
        let _ = self.conn.execute_batch("BEGIN");
        for event in events {
            let result = match event {
                Event::New(m) => self.insert_new(&m).map(|added| {
                    if added {
                        for job in self.plan(&m, max_image_bytes(), max_disk_bytes()) {
                            spawn(job);
                        }
                    }
                }),
                Event::Other { message_id, ts_ms } => self.note_other(message_id, ts_ms),
                Event::Deleted(d) => self.delete(&d, keep_days()).map(|_| ()),
                Event::Edited { message_id, content, ts_ms } => self.edit(message_id, &content, ts_ms).map(|_| ()),
                Event::Saved { message_id, file } => self.saved(message_id, file),
                Event::Purge { now_ms } => {
                    // Commit what came before, so a slow purge never holds it back.
                    let _ = self.conn.execute_batch("COMMIT; BEGIN");
                    self.purge(now_ms, keep_days(), log_days()).map(|r| {
                        if r != PurgeReport::default() {
                            tracing::info!(
                                "msglog: cleared {} copies, {} deleted and {} edited log entries, {} pictures, {} folders; {} MB in use",
                                r.recent,
                                r.deleted,
                                r.edited,
                                r.files,
                                r.folders,
                                self.used / (1024 * 1024)
                            );
                        }
                    })
                }
            };
            if let Err(err) = result {
                tracing::warn!("msglog: couldn't store an event: {}", err);
            }
        }
        if let Err(err) = self.conn.execute_batch("COMMIT") {
            tracing::warn!("msglog: couldn't commit: {}", err);
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// The writer: applies events in arrival order until every sender is gone.
pub fn run_writer(mut store: Store, mut rx: mpsc::Receiver<Event>, mut spawn: impl FnMut(Job)) {
    while let Some(first) = rx.blocking_recv() {
        let mut batch = vec![first];
        while batch.len() < 500 {
            match rx.try_recv() {
                Ok(e) => batch.push(e),
                Err(_) => break,
            }
        }
        store.apply(batch, &mut spawn);
    }
}

/// Queues an event without waiting. A full queue drops it and counts the drop.
pub fn push(tx: &mpsc::Sender<Event>, event: Event) -> bool {
    match tx.try_send(event) {
        Ok(()) => true,
        Err(mpsc::error::TrySendError::Full(_)) => {
            let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
            if n.is_power_of_two() || n % 1000 == 0 {
                tracing::warn!("msglog: the queue is full; {} events dropped so far", n);
            }
            false
        }
        Err(mpsc::error::TrySendError::Closed(_)) => false,
    }
}

pub static DROPPED: AtomicU64 = AtomicU64::new(0);

// --- reading, for the panel -----------------------------------------------------------------

pub struct ListFilter {
    pub member: Option<u64>,
    pub channel: Option<u64>,
    pub since_ms: i64,
    /// Row id to carry on below (exclusive).
    pub before: Option<i64>,
    pub q: Option<String>,
    /// Whether `q` is in a text: `(text, q)`.
    pub matches: fn(&str, &str) -> bool,
    pub limit: usize,
    /// Channels never shown (with threads in them); #safe-corner by name always.
    pub sensitive: Vec<u64>,
}

impl ListFilter {
    fn shows(&self, channel_id: u64, parent_id: Option<u64>, channel_name: &str) -> bool {
        !excluded(&Place { channel_id, parent_id, channel_name: channel_name.to_string() }, None, &self.sensitive)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DeletedRow {
    pub id: i64,
    pub message_id: u64,
    pub channel_id: u64,
    pub parent_id: Option<u64>,
    pub channel_name: String,
    pub author_id: Option<u64>,
    pub author_name: Option<String>,
    pub avatar: Option<String>,
    pub content: Option<String>,
    pub created_ms: i64,
    pub deleted_ms: i64,
    pub reply_to: Option<u64>,
    pub reply_author: Option<String>,
    pub reply_text: Option<String>,
    pub attachments: Vec<Attachment>,
    pub files: Vec<StoredFile>,
    pub bulk: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EditedRow {
    pub id: i64,
    pub message_id: u64,
    pub channel_id: u64,
    pub parent_id: Option<u64>,
    pub channel_name: String,
    pub author_id: u64,
    pub author_name: String,
    pub avatar: String,
    pub before: String,
    pub after: String,
    pub created_ms: i64,
    pub edited_ms: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Page<T> {
    pub rows: Vec<T>,
    /// Pass back as `before` for the next page; none when this was the last.
    pub next_before: Option<i64>,
}

fn page<T>(rows: impl Iterator<Item = rusqlite::Result<T>>, keep: impl Fn(&T) -> bool, id: impl Fn(&T) -> i64, limit: usize) -> rusqlite::Result<Page<T>> {
    let mut out = Page { rows: Vec::new(), next_before: None };
    for row in rows {
        let row = row?;
        if !keep(&row) {
            continue;
        }
        if out.rows.len() == limit {
            out.next_before = out.rows.last().map(&id);
            break;
        }
        out.rows.push(row);
    }
    Ok(out)
}

/// Deleted messages, newest deletion first.
pub fn list_deleted(conn: &Connection, f: &ListFilter) -> rusqlite::Result<Page<DeletedRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, message_id, channel_id, parent_id, channel_name, author_id, author_name, avatar, content, created_ts, deleted_ts,
                reply_to, reply_author, reply_text, attachments_json, stored_files_json, bulk, reason
         FROM deleted WHERE id < ?1 AND deleted_ts >= ?2 AND (?3 IS NULL OR author_id = ?3) AND (?4 IS NULL OR channel_id = ?4 OR parent_id = ?4)
         ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![f.before.unwrap_or(i64::MAX), f.since_ms, f.member.map(|m| m as i64), f.channel.map(|c| c as i64)], |r| {
        Ok(DeletedRow {
            id: r.get(0)?,
            message_id: r.get::<_, i64>(1)? as u64,
            channel_id: r.get::<_, i64>(2)? as u64,
            parent_id: r.get::<_, Option<i64>>(3)?.map(|p| p as u64),
            channel_name: r.get(4)?,
            author_id: r.get::<_, Option<i64>>(5)?.map(|a| a as u64),
            author_name: r.get(6)?,
            avatar: r.get(7)?,
            content: r.get(8)?,
            created_ms: r.get(9)?,
            deleted_ms: r.get(10)?,
            reply_to: r.get::<_, Option<i64>>(11)?.map(|x| x as u64),
            reply_author: r.get(12)?,
            reply_text: r.get(13)?,
            attachments: serde_json::from_str(&r.get::<_, String>(14)?).unwrap_or_default(),
            files: files_of(&r.get::<_, String>(15)?),
            bulk: r.get(16)?,
            reason: r.get(17)?,
        })
    })?;
    let q = f.q.clone();
    page(
        rows,
        |row| f.shows(row.channel_id, row.parent_id, &row.channel_name) && q.as_deref().is_none_or(|q| row.content.as_deref().is_some_and(|t| (f.matches)(t, q))),
        |row| row.id,
        f.limit,
    )
}

/// Edits, newest first.
pub fn list_edited(conn: &Connection, f: &ListFilter) -> rusqlite::Result<Page<EditedRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT id, message_id, channel_id, parent_id, channel_name, author_id, author_name, avatar, before, after, created_ts, edited_ts
         FROM edited WHERE id < ?1 AND edited_ts >= ?2 AND (?3 IS NULL OR author_id = ?3) AND (?4 IS NULL OR channel_id = ?4 OR parent_id = ?4)
         ORDER BY id DESC",
    )?;
    let rows = stmt.query_map(params![f.before.unwrap_or(i64::MAX), f.since_ms, f.member.map(|m| m as i64), f.channel.map(|c| c as i64)], |r| {
        Ok(EditedRow {
            id: r.get(0)?,
            message_id: r.get::<_, i64>(1)? as u64,
            channel_id: r.get::<_, i64>(2)? as u64,
            parent_id: r.get::<_, Option<i64>>(3)?.map(|p| p as u64),
            channel_name: r.get(4)?,
            author_id: r.get::<_, i64>(5)? as u64,
            author_name: r.get(6)?,
            avatar: r.get(7)?,
            before: r.get(8)?,
            after: r.get(9)?,
            created_ms: r.get(10)?,
            edited_ms: r.get(11)?,
        })
    })?;
    let q = f.q.clone();
    page(
        rows,
        |row| f.shows(row.channel_id, row.parent_id, &row.channel_name) && q.as_deref().is_none_or(|q| (f.matches)(&row.before, q) || (f.matches)(&row.after, q)),
        |row| row.id,
        f.limit,
    )
}

/// A saved picture of a DELETED message: its bytes and content type. Nothing
/// for a message that wasn't deleted, a picture that isn't there, or a stored
/// path that would leave the folder.
pub fn deleted_file(conn: &Connection, root: &Path, message_id: u64, n: usize) -> Option<(Vec<u8>, &'static str)> {
    let json: String = conn.query_row("SELECT stored_files_json FROM deleted WHERE message_id = ?1", params![message_id as i64], |r| r.get(0)).optional().ok()??;
    let file = files_of(&json).into_iter().find(|f| f.n == n)?;
    if !safe_rel(&file.path) || !file.path.starts_with("deleted/") {
        return None;
    }
    let ext = Path::new(&file.path).extension()?.to_str()?.to_ascii_lowercase();
    let kind = content_type_for(&ext)?;
    let path = root.join(&file.path);
    let (base, real) = (root.canonicalize().ok()?, path.canonicalize().ok()?);
    if !real.starts_with(&base) {
        return None;
    }
    Some((std::fs::read(real).ok()?, kind))
}

pub fn content_type_for(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

// --- live wiring ----------------------------------------------------------------------------

static TX: OnceLock<mpsc::Sender<Event>> = OnceLock::new();

pub struct Reader {
    pub conn: Mutex<Connection>,
    pub root: PathBuf,
}

static READER: OnceLock<Reader> = OnceLock::new();

/// The panel's read-only side: its own connection and the pictures folder.
pub fn reader() -> Option<&'static Reader> {
    READER.get()
}

/// Opens the log and starts the writer, the downloads and the hourly purge.
/// Call once, from inside the runtime.
pub fn start(workspace: &str) -> anyhow::Result<()> {
    if TX.get().is_some() {
        return Ok(());
    }
    let runtime = crate::utils::build_path(workspace, &[".runtime"]);
    let db = runtime.join("msglog.db");
    let root = runtime.join("msglog");
    let store = Store::open(&db, root.clone(), chrono::Utc::now().timestamp_millis())?;
    let read = Connection::open(&db)?;
    read.execute_batch("PRAGMA busy_timeout=5000;")?;
    let _ = READER.set(Reader { conn: Mutex::new(read), root: root.clone() });

    let (tx, rx) = mpsc::channel(QUEUE);
    if TX.set(tx.clone()).is_err() {
        return Ok(());
    }
    let handle = tokio::runtime::Handle::current();
    let limit = Arc::new(tokio::sync::Semaphore::new(DOWNLOADS_AT_ONCE));
    let saved_tx = tx.clone();
    let spawn_root = root.clone();
    std::thread::Builder::new().name("msglog-writer".into()).spawn(move || {
        run_writer(store, rx, move |job| {
            handle.spawn(download(job, saved_tx.clone(), limit.clone(), spawn_root.clone()));
        });
        tracing::warn!("msglog: the writer stopped");
    })?;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PURGE_EVERY);
        loop {
            tick.tick().await;
            if tx.send(Event::Purge { now_ms: chrono::Utc::now().timestamp_millis() }).await.is_err() {
                break;
            }
        }
    });
    tracing::info!("msglog: logging deleted and edited messages");
    Ok(())
}

fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();
    CLIENT.get_or_init(|| reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).build().ok()).as_ref()
}

async fn fetch(url: &str, max: u64) -> anyhow::Result<Vec<u8>> {
    let client = client().ok_or_else(|| anyhow::anyhow!("no http client"))?;
    let mut res = client.get(url).send().await?.error_for_status()?;
    if res.content_length().is_some_and(|n| n > max) {
        anyhow::bail!("bigger than the limit");
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() as u64 > max {
            anyhow::bail!("bigger than the limit");
        }
    }
    Ok(body)
}

async fn download(job: Job, tx: mpsc::Sender<Event>, limit: Arc<tokio::sync::Semaphore>, root: PathBuf) {
    let Ok(_permit) = limit.acquire_owned().await else { return };
    let body = match fetch(&job.url, job.max_bytes).await {
        Ok(b) => b,
        Err(err) => {
            tracing::debug!("msglog: picture {} of message {} not saved: {}", job.n, job.message_id, err);
            return;
        }
    };
    let path = root.join(&job.rel);
    let bytes = body.len() as u64;
    let written = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, body)
    })
    .await;
    if !matches!(written, Ok(Ok(()))) {
        tracing::warn!("msglog: couldn't write picture {} of message {}", job.n, job.message_id);
        return;
    }
    let file = StoredFile { n: job.n, path: job.rel.clone(), bytes, name: job.name };
    if tx.send(Event::Saved { message_id: job.message_id, file }).await.is_err() {
        let _ = std::fs::remove_file(root.join(&job.rel));
    }
}

/// Where a message was posted, from the cache, when it may be logged. Nothing for
/// a channel the cache can't place, #safe-corner or a thread in it.
fn place(ctx: &Context, guild: GuildId, channel: serenity::all::ChannelId) -> Option<Place> {
    let g = ctx.cache.guild(guild)?;
    let (name, parent) = match g.channels.get(&channel) {
        Some(c) => (c.name.clone(), None),
        None => {
            let t = g.threads.iter().find(|t| t.id == channel)?;
            (t.name.clone(), t.parent_id)
        }
    };
    let parent_name = parent.and_then(|p| g.channels.get(&p).map(|c| c.name.clone()));
    let place = Place { channel_id: channel.get(), parent_id: parent.map(|p| p.get()), channel_name: name };
    let sensitive = super::control::insights::sensitive_channels();
    // A thread whose parent the cache doesn't know can't be checked.
    if parent.is_some() && parent_name.is_none() {
        return None;
    }
    (!excluded(&place, parent_name.as_deref(), &sensitive)).then_some(place)
}

fn new_message(msg: &Message, place: Place, guild: u64) -> NewMessage {
    let author_name = msg
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| msg.author.global_name.clone())
        .unwrap_or_else(|| msg.author.name.clone());
    let replied = msg.referenced_message.as_deref();
    let reply_to = replied
        .map(|m| m.id.get())
        .or_else(|| (msg.kind == MessageType::InlineReply).then(|| msg.message_reference.as_ref().and_then(|r| r.message_id)).flatten().map(|m| m.get()));
    NewMessage {
        message_id: msg.id.get(),
        place,
        guild_id: guild,
        author_id: msg.author.id.get(),
        author_name,
        avatar: msg.author.face(),
        content: msg.content.clone(),
        created_ms: msg.timestamp.unix_timestamp() * 1000,
        reply_to,
        reply_author: replied.map(|m| m.author.global_name.clone().unwrap_or_else(|| m.author.name.clone())),
        reply_text: replied.map(|m| short(&m.content, REPLY_CHARS)),
        attachments: msg
            .attachments
            .iter()
            .map(|a| Attachment { id: a.id.get(), filename: a.filename.clone(), content_type: a.content_type.clone(), size: a.size as u64, url: a.url.clone() })
            .collect(),
    }
}

/// The message handler's hook, for every message before anything else. Cheap:
/// reads the event and the cache and queues.
pub fn on_message(ctx: &Context, msg: &Message) {
    let (Some(tx), Some(guild)) = (TX.get(), msg.guild_id) else { return };
    if !enabled() {
        return;
    }
    let place = place(ctx, guild, msg.channel_id);
    match classify(true, msg.author.bot, msg.webhook_id.is_some(), place.is_some()) {
        Capture::Skip => {}
        Capture::Other => {
            push(tx, Event::Other { message_id: msg.id.get(), ts_ms: msg.timestamp.unix_timestamp() * 1000 });
        }
        Capture::Keep => {
            if let Some(place) = place {
                push(tx, Event::New(new_message(msg, place, guild.get())));
            }
        }
    }
}

/// `message_delete` and `message_delete_bulk`.
pub fn on_delete(ctx: &Context, channel: serenity::all::ChannelId, ids: &[MessageId], guild: Option<GuildId>, bulk: bool) {
    let (Some(tx), Some(guild)) = (TX.get(), guild) else { return };
    if !enabled() || ids.is_empty() {
        return;
    }
    let Some(place) = place(ctx, guild, channel) else { return };
    // Serenity's own message cache, when it has one of them and it was never queued.
    for id in ids {
        let cached = ctx.cache.message(channel, *id).map(|m| (*m).clone());
        if let Some(m) = cached.filter(|m| !m.author.bot && m.webhook_id.is_none()) {
            push(tx, Event::New(new_message(&m, place.clone(), guild.get())));
        }
    }
    push(tx, Event::Deleted(Deletion { ids: ids.iter().map(|i| i.get()).collect(), place, ts_ms: chrono::Utc::now().timestamp_millis(), bulk }));
}

/// `message_update`: only a change of text is logged.
pub fn on_edit(ctx: &Context, event: &MessageUpdateEvent) {
    let (Some(tx), Some(guild), Some(content)) = (TX.get(), event.guild_id, event.content.as_ref()) else { return };
    if !enabled() || event.author.as_ref().is_some_and(|a| a.bot) {
        return;
    }
    if place(ctx, guild, event.channel_id).is_none() {
        return;
    }
    let ts_ms = event.edited_timestamp.map(|t| t.unix_timestamp() * 1000).unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    push(tx, Event::Edited { message_id: event.id.get(), content: content.clone(), ts_ms });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFE: u64 = super::super::weekly::SAFE_CORNER;
    const NOW: i64 = 1_789_000_000_000;

    fn place(channel: u64) -> Place {
        Place { channel_id: channel, parent_id: None, channel_name: "general".into() }
    }

    /// A message id sent at `ms`.
    fn id_at(ms: i64, seq: u64) -> u64 {
        (((ms - DISCORD_EPOCH_MS) as u64) << 22) | seq
    }

    fn img(id: u64, name: &str, kind: Option<&str>, size: u64) -> Attachment {
        Attachment { id, filename: name.into(), content_type: kind.map(String::from), size, url: format!("https://cdn.example/{id}/{name}") }
    }

    fn msg(id: u64, created_ms: i64, text: &str, attachments: Vec<Attachment>) -> NewMessage {
        NewMessage {
            message_id: id,
            place: place(21),
            guild_id: 900,
            author_id: 42,
            author_name: "Riya".into(),
            avatar: "https://cdn/a.png".into(),
            content: text.into(),
            created_ms,
            reply_to: None,
            reply_author: None,
            reply_text: None,
            attachments,
        }
    }

    fn open(dir: &tempfile::TempDir) -> Store {
        Store::open(&dir.path().join("msglog.db"), dir.path().join("msglog"), NOW - 10 * DAY_MS).unwrap()
    }

    /// Stands in for a finished download: writes the file and hands it back.
    fn finish(store: &mut Store, job: &Job, bytes: &[u8]) {
        let path = store.root().join(&job.rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        store.saved(job.message_id, StoredFile { n: job.n, path: job.rel.clone(), bytes: bytes.len() as u64, name: job.name.clone() }).unwrap();
    }

    fn count(store: &Store, table: &str) -> i64 {
        store.conn().query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn only_server_members_in_allowed_channels_are_kept() {
        let sensitive = [SAFE];
        assert!(!excluded(&place(21), None, &sensitive));
        assert!(excluded(&place(SAFE), None, &sensitive), "#safe-corner by id");
        assert!(excluded(&Place { channel_id: 5, parent_id: None, channel_name: "safe-corner-2".into() }, None, &sensitive), "by name");
        assert!(excluded(&Place { channel_id: 77, parent_id: Some(SAFE), channel_name: "a thread".into() }, Some("x"), &sensitive), "a thread in it");
        assert!(excluded(&Place { channel_id: 77, parent_id: Some(9), channel_name: "vent".into() }, Some("safe-corner"), &sensitive));
        assert!(!excluded(&Place { channel_id: 78, parent_id: Some(21), channel_name: "koto talk".into() }, Some("general"), &sensitive));

        assert_eq!(classify(true, false, false, true), Capture::Keep);
        assert_eq!(classify(false, false, false, true), Capture::Skip, "DMs");
        assert_eq!(classify(true, false, false, false), Capture::Skip, "safe corner or unknown channel");
        assert_eq!(classify(true, true, false, true), Capture::Other, "bots");
        assert_eq!(classify(true, false, true, true), Capture::Other, "webhooks");
        assert_eq!(classify(true, true, false, false), Capture::Skip, "a bot in safe corner isn't even noted");
    }

    #[test]
    fn pictures_are_picked_by_type_size_and_count() {
        let max = 8 * 1024 * 1024;
        let list = vec![
            img(1, "a.PNG", Some("image/png"), 1000),
            img(2, "notes.pdf", Some("application/pdf"), 1000),
            img(3, "huge.jpg", Some("image/jpeg"), max + 1),
            img(4, "noType.JPEG", None, 2000),
            img(5, "vector.svg", Some("image/svg+xml"), 100),
            img(6, "clip.gif", Some("image/gif"), 3000),
            img(7, "empty.png", Some("image/png"), 0),
            img(8, "x.webp", None, 10),
            img(9, "fifth.png", Some("image/png"), 10),
            img(10, "song.mp3", None, 10),
        ];
        let picked: Vec<(usize, u64, &str)> = pick_images(&list, max).iter().map(|(n, a, e)| (*n, a.id, *e)).collect();
        assert_eq!(picked, vec![(0, 1, "png"), (1, 4, "jpg"), (2, 6, "gif"), (3, 8, "webp")], "four at most, in order");
        assert_eq!(image_ext(Some("image/jpeg; charset=binary"), "x"), Some("jpg"));
        assert_eq!(image_ext(Some("text/plain"), "fake.png"), None, "the type wins over the name");
        assert_eq!(image_ext(None, "noext"), None);
    }

    #[test]
    fn queued_messages_are_stored_in_order_and_downloads_planned() {
        let dir = tempfile::tempdir().unwrap();
        let store = open(&dir);
        let (tx, rx) = mpsc::channel(16);
        let id = id_at(NOW, 1);
        let mut m = msg(id, NOW, "hello there", vec![img(1, "meme.png", Some("image/png"), 500), img(2, "doc.txt", Some("text/plain"), 50)]);
        m.reply_to = Some(id_at(NOW - 1000, 0));
        let mut replied = msg(id_at(NOW - 1000, 0), NOW - 1000, "the original\nline", vec![]);
        replied.author_name = "Dev".into();
        assert!(push(&tx, Event::New(replied)));
        assert!(push(&tx, Event::New(m.clone())));
        assert!(push(&tx, Event::New(m.clone())), "a repeat delivery");
        assert!(push(&tx, Event::Other { message_id: 99, ts_ms: NOW }));
        drop(tx);
        let jobs = std::thread::scope(|s| {
            s.spawn(|| {
                let mut jobs = Vec::new();
                run_writer(store, rx, |j| jobs.push(j));
                jobs
            })
            .join()
            .unwrap()
        });
        assert_eq!(jobs.len(), 1, "one picture, planned once: {jobs:?}");
        assert_eq!(jobs[0].rel, format!("{}/{}_0.png", day_folder(NOW), id));
        assert_eq!(jobs[0].url, "https://cdn.example/1/meme.png");

        let store = open(&dir);
        assert_eq!(count(&store, "recent"), 2);
        assert_eq!(count(&store, "others"), 1);
        let (text, reply_author, reply_text, attachments): (String, String, String, String) = store
            .conn()
            .query_row("SELECT content, reply_author, reply_text, attachments_json FROM recent WHERE message_id = ?1", params![id as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .unwrap();
        assert_eq!((text.as_str(), reply_author.as_str(), reply_text.as_str()), ("hello there", "Dev", "the original line"));
        assert!(attachments.contains("doc.txt") && !attachments.contains("cdn.example"), "names and sizes, never the link: {attachments}");
    }

    #[test]
    fn a_full_queue_drops_and_counts() {
        let (tx, _rx) = mpsc::channel(1);
        let before = DROPPED.load(Ordering::Relaxed);
        assert!(push(&tx, Event::Other { message_id: 1, ts_ms: 0 }));
        assert!(!push(&tx, Event::Other { message_id: 2, ts_ms: 0 }));
        assert!(DROPPED.load(Ordering::Relaxed) > before);
    }

    #[test]
    fn deleting_moves_the_copy_and_its_pictures() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        let id = id_at(NOW - 60_000, 3);
        let m = msg(id, NOW - 60_000, "delete me", vec![img(1, "a.png", Some("image/png"), 4), img(2, "b.jpg", Some("image/jpeg"), 4)]);
        store.insert_new(&m).unwrap();
        let jobs = store.plan(&m, 1 << 20, 1 << 30);
        assert_eq!(jobs.len(), 2);
        finish(&mut store, &jobs[0], b"png!");
        let old_path = store.root().join(&jobs[0].rel);
        assert!(old_path.exists());

        let d = Deletion { ids: vec![id], place: place(21), ts_ms: NOW, bulk: false };
        assert_eq!(store.delete(&d, 7).unwrap(), 1);
        assert_eq!(count(&store, "recent"), 0);
        assert!(!old_path.exists(), "moved out of the day folder");
        let moved = store.root().join(format!("deleted/{}_0.png", id));
        assert_eq!(std::fs::read(&moved).unwrap(), b"png!");

        // The second picture finishes after the delete: it follows the message.
        finish(&mut store, &jobs[1], b"jpg!");
        assert!(store.root().join(format!("deleted/{}_1.jpg", id)).exists());
        assert!(!store.root().join(&jobs[1].rel).exists());

        let page = list_deleted(store.conn(), &filter()).unwrap();
        assert_eq!(page.rows.len(), 1);
        let row = &page.rows[0];
        assert_eq!((row.content.as_deref(), row.author_id, row.bulk, row.reason.as_deref()), (Some("delete me"), Some(42), false, None));
        assert_eq!((row.created_ms, row.deleted_ms), (NOW - 60_000, NOW));
        assert_eq!(row.files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(), vec![format!("deleted/{}_0.png", id), format!("deleted/{}_1.jpg", id)]);
        assert_eq!(deleted_file(store.conn(), store.root(), id, 1).unwrap(), (b"jpg!".to_vec(), "image/jpeg"));

        // A picture for a message that's gone everywhere is thrown away.
        let stray = Job { message_id: 12345, n: 0, url: String::new(), rel: "2026-01-01/12345_0.png".into(), name: "x.png".into(), max_bytes: 10, expected: 1 };
        finish(&mut store, &stray, b"x");
        assert!(!store.root().join(&stray.rel).exists());
    }

    fn filter() -> ListFilter {
        ListFilter { member: None, channel: None, since_ms: 0, before: None, q: None, matches: |t, q| t.to_lowercase().contains(&q.to_lowercase()), limit: 50, sensitive: vec![SAFE] }
    }

    #[test]
    fn unknown_deletes_say_why_and_bots_are_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        assert_eq!(store.first_started_ms, NOW - 10 * DAY_MS);
        let before_logging = id_at(NOW - 11 * DAY_MS + 3_600_000, 0);
        let expired = id_at(NOW - 8 * DAY_MS, 0);
        let missed = id_at(NOW - 3_600_000, 0);
        let bot = id_at(NOW - 1000, 0);
        store.note_other(bot, NOW - 1000).unwrap();
        let d = Deletion { ids: vec![before_logging, expired, missed, bot], place: Place { channel_id: 78, parent_id: Some(21), channel_name: "koto".into() }, ts_ms: NOW, bulk: true };
        assert_eq!(store.delete(&d, 7).unwrap(), 3, "the bot's message isn't logged");
        // The same event again changes nothing.
        assert_eq!(store.delete(&d, 7).unwrap(), 0);
        let rows = list_deleted(store.conn(), &filter()).unwrap().rows;
        let why: Vec<(u64, Option<&str>, bool, Option<&str>)> = rows.iter().map(|r| (r.message_id, r.reason.as_deref(), r.bulk, r.content.as_deref())).collect();
        assert_eq!(why, vec![(missed, Some(REASON_MISSED), true, None), (expired, Some(REASON_EXPIRED), true, None), (before_logging, Some(REASON_BEFORE), true, None)]);
        assert!(rows.iter().all(|r| r.author_id.is_none() && r.parent_id == Some(21) && r.channel_name == "koto"));
        assert_eq!(rows[0].created_ms, NOW - 3_600_000, "the time comes from the id");
        // The channel filter takes threads in the channel.
        let mut f = filter();
        f.channel = Some(21);
        assert_eq!(list_deleted(store.conn(), &f).unwrap().rows.len(), 3);
        f.member = Some(42);
        assert!(list_deleted(store.conn(), &f).unwrap().rows.is_empty(), "rows without an author never match a member");
    }

    #[test]
    fn a_bulk_delete_logs_every_message() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        let ids: Vec<u64> = (0..5).map(|i| id_at(NOW - 5000 + i, i as u64)).collect();
        for (i, id) in ids.iter().enumerate() {
            store.insert_new(&msg(*id, NOW - 5000 + i as i64, &format!("spam {i}"), vec![])).unwrap();
        }
        store.delete(&Deletion { ids: ids.clone(), place: place(21), ts_ms: NOW, bulk: true }, 7).unwrap();
        let rows = list_deleted(store.conn(), &filter()).unwrap().rows;
        assert_eq!(rows.len(), 5);
        assert!(rows.iter().all(|r| r.bulk && r.content.as_deref().is_some_and(|c| c.starts_with("spam"))));
        assert_eq!(count(&store, "recent"), 0);
    }

    #[test]
    fn edits_are_logged_only_when_the_text_changes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        let id = id_at(NOW - 5000, 0);
        store.insert_new(&msg(id, NOW - 5000, "helo", vec![])).unwrap();
        assert!(!store.edit(id, "helo", NOW - 4000).unwrap(), "an embed showing up isn't an edit");
        assert!(store.edit(id, "hello", NOW - 3000).unwrap());
        assert!(store.edit(id, "hello!!", NOW - 2000).unwrap());
        assert!(!store.edit(id_at(NOW, 9), "unknown", NOW).unwrap(), "no copy, no before");
        let rows = list_edited(store.conn(), &filter()).unwrap().rows;
        let pairs: Vec<(&str, &str)> = rows.iter().map(|r| (r.before.as_str(), r.after.as_str())).collect();
        assert_eq!(pairs, vec![("hello", "hello!!"), ("helo", "hello")], "newest first");
        assert_eq!(rows[0].edited_ms, NOW - 2000);
        // A delete after edits keeps the latest text.
        store.delete(&Deletion { ids: vec![id], place: place(21), ts_ms: NOW, bulk: false }, 7).unwrap();
        assert_eq!(list_deleted(store.conn(), &filter()).unwrap().rows[0].content.as_deref(), Some("hello!!"));
        let mut f = filter();
        f.q = Some("HELO".into());
        assert_eq!(list_edited(store.conn(), &f).unwrap().rows.len(), 1, "matches the before text too");
    }

    #[test]
    fn purge_clears_each_table_by_its_own_age() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        let day = |d: i64| NOW - d * DAY_MS;
        // Copies: one eight days old with a picture, one two days old with a picture.
        let old = msg(id_at(day(8), 0), day(8), "old copy", vec![img(1, "o.png", Some("image/png"), 3)]);
        let young = msg(id_at(day(2), 0), day(2), "young copy", vec![img(2, "y.png", Some("image/png"), 3)]);
        for m in [&old, &young] {
            store.insert_new(m).unwrap();
            let job = store.plan(m, 100, 1 << 30).remove(0);
            finish(&mut store, &job, b"img");
        }
        // A stray picture in a nine-day-old folder, and an empty folder from yesterday.
        let stray = store.root().join(format!("{}/1_0.png", day_folder(day(9))));
        std::fs::create_dir_all(stray.parent().unwrap()).unwrap();
        std::fs::write(&stray, b"orphan").unwrap();
        std::fs::create_dir_all(store.root().join(day_folder(day(1)))).unwrap();
        store.note_other(5, day(8)).unwrap();
        store.note_other(6, day(1)).unwrap();
        // Log entries: deleted 31 and 20 days ago (with pictures), edited 31 and 20 days ago.
        let gone_old = msg(id_at(day(35), 0), day(35), "deleted long ago", vec![img(3, "g.png", Some("image/png"), 3)]);
        let gone_new = msg(id_at(day(21), 0), day(21), "deleted recently", vec![img(4, "h.png", Some("image/png"), 3)]);
        for (m, at) in [(&gone_old, day(31)), (&gone_new, day(20))] {
            store.insert_new(m).unwrap();
            let job = store.plan(m, 100, 1 << 30).remove(0);
            finish(&mut store, &job, b"img");
            store.edit(m.message_id, "edited text", at).unwrap();
            store.delete(&Deletion { ids: vec![m.message_id], place: place(21), ts_ms: at, bulk: false }, 7).unwrap();
        }

        let report = store.purge(NOW, 7, 30).unwrap();
        assert_eq!((report.recent, report.others, report.deleted, report.edited), (1, 1, 1, 1), "{report:?}");
        assert_eq!(report.files, 2, "the old copy's picture and the old deleted one's");
        assert!(!stray.exists(), "a folder older than the copies goes whole");
        assert!(!store.root().join(day_folder(day(1))).exists(), "empty folders go");
        assert!(store.root().join(day_folder(day(2))).exists(), "the young copy's folder stays");
        let texts: Vec<String> = store.conn().prepare("SELECT content FROM recent").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
        assert_eq!(texts, vec!["young copy"]);
        let deleted = list_deleted(store.conn(), &filter()).unwrap().rows;
        assert_eq!(deleted.iter().map(|r| r.content.clone().unwrap()).collect::<Vec<_>>(), vec!["edited text"]);
        assert!(deleted_file(store.conn(), store.root(), gone_new.message_id, 0).is_some());
        assert!(deleted_file(store.conn(), store.root(), gone_old.message_id, 0).is_none());
        assert_eq!(list_edited(store.conn(), &filter()).unwrap().rows.len(), 1);
        assert_eq!(store.used, 6, "two pictures of three bytes left");
        // Nothing more to do on a second run.
        assert_eq!(store.purge(NOW, 7, 30).unwrap(), PurgeReport::default());
    }

    #[test]
    fn the_disk_guard_stops_downloads_until_a_purge_frees_space() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        let big = |i: u64, at: i64| msg(id_at(at, i), at, "pic", vec![img(i, "p.png", Some("image/png"), 600)]);
        let first = big(1, NOW - 9 * DAY_MS);
        store.insert_new(&first).unwrap();
        let job = store.plan(&first, 1000, 1000).remove(0);
        finish(&mut store, &job, &[7u8; 600]);
        let second = big(2, NOW);
        store.insert_new(&second).unwrap();
        assert!(store.plan(&second, 1000, 1000).is_empty(), "600 + 600 is over 1000");
        assert!(store.full_logged);
        assert!(store.plan(&second, 1000, 1000).is_empty());
        store.purge(NOW, 7, 30).unwrap();
        assert_eq!(store.used, 0);
        assert_eq!(store.plan(&second, 1000, 1000).len(), 1, "room again after the old picture was cleared");
    }

    #[test]
    fn stored_paths_never_leave_the_folder() {
        for bad in ["", "../x.png", "deleted/../../etc/passwd", "/etc/passwd", "a/b/c.png", "deleted\\..\\x.png", "./x.png"] {
            assert!(!safe_rel(bad), "{bad}");
        }
        assert!(safe_rel("deleted/1_0.png") && safe_rel("2026-09-16/1_0.png"));

        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        std::fs::write(dir.path().join("secret.png"), b"secret").unwrap();
        let id = id_at(NOW, 0);
        store.insert_new(&msg(id, NOW, "x", vec![])).unwrap();
        store.delete(&Deletion { ids: vec![id], place: place(21), ts_ms: NOW, bulk: false }, 7).unwrap();
        let evil = json_files(&[("../secret.png", 0), ("deleted/../../secret.png", 1), ("secret.png", 2)]);
        store.conn().execute("UPDATE deleted SET stored_files_json = ?1 WHERE message_id = ?2", params![evil, id as i64]).unwrap();
        for n in 0..3 {
            assert!(deleted_file(store.conn(), store.root(), id, n).is_none(), "n={n}");
        }
        // A kept (not deleted) message's picture is never served.
        let kept = msg(id_at(NOW, 1), NOW, "kept", vec![img(1, "k.png", Some("image/png"), 2)]);
        store.insert_new(&kept).unwrap();
        let job = store.plan(&kept, 10, 1000).remove(0);
        finish(&mut store, &job, b"ok");
        assert!(deleted_file(store.conn(), store.root(), kept.message_id, 0).is_none());
    }

    fn json_files(list: &[(&str, usize)]) -> String {
        serde_json::to_string(&list.iter().map(|(p, n)| StoredFile { n: *n, path: p.to_string(), bytes: 1, name: "x".into() }).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn paging_and_search_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = open(&dir);
        for i in 0..7u64 {
            let id = id_at(NOW - 10_000 + i as i64, i);
            let mut m = msg(id, NOW - 10_000 + i as i64, &format!("koto message {i}"), vec![]);
            m.author_id = if i % 2 == 0 { 42 } else { 43 };
            store.insert_new(&m).unwrap();
            store.delete(&Deletion { ids: vec![id], place: place(21), ts_ms: NOW - 1000 + i as i64, bulk: false }, 7).unwrap();
        }
        let mut f = filter();
        f.limit = 3;
        let mut seen = Vec::new();
        loop {
            let page = list_deleted(store.conn(), &f).unwrap();
            assert!(page.rows.len() <= 3);
            seen.extend(page.rows.iter().map(|r| r.content.clone().unwrap()));
            match page.next_before {
                Some(b) => f.before = Some(b),
                None => break,
            }
        }
        assert_eq!(seen, (0..7).rev().map(|i| format!("koto message {i}")).collect::<Vec<_>>());
        let mut f = filter();
        f.member = Some(43);
        assert_eq!(list_deleted(store.conn(), &f).unwrap().rows.len(), 3);
        f.q = Some("MESSAGE 5".into());
        assert_eq!(list_deleted(store.conn(), &f).unwrap().rows.len(), 1);
        let mut f = filter();
        f.since_ms = NOW - 997;
        assert_eq!(list_deleted(store.conn(), &f).unwrap().rows.len(), 4, "the period counts from the deletion");
        // Exactly a page: no cursor.
        let mut f = filter();
        f.limit = 7;
        assert!(list_deleted(store.conn(), &f).unwrap().next_before.is_none());
    }
}
