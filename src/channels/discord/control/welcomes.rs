//! Special welcomes: a message the bot posts the moment one particular member
//! joins the server - usually someone everyone has been waiting on to come
//! back, though it works for a first join too. Made on the panel's Welcomes
//! page. This file is the store, how a welcome's words are filled in, and
//! posting one; the join itself is handled in `guild_member_addition`.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serenity::all::{ChannelId, Context, CreateAllowedMentions, CreateMessage, UserId};

use super::DB;

pub const MAX_RULES: usize = 50;
pub const MAX_LINES: usize = 10;
pub const MAX_LINE: usize = 1500;
pub const MAX_ALSO_PING: usize = 10;
pub const MAX_NOTE: usize = 500;
pub const MAX_NAME: usize = 100;

/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);

/// Whether a welcome goes out once or on every join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Switches itself off after it has gone out.
    #[default]
    Once,
    Every,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Welcome {
    /// 0 for one not saved yet.
    #[serde(default)]
    pub id: i64,
    /// Discord ids travel as strings: they are too big for JavaScript numbers.
    pub user_id: String,
    /// Their name when the welcome was made: they are usually not in the
    /// server then, so nothing else can name them.
    #[serde(default)]
    pub user_name: String,
    /// Empty posts in the welcome channel.
    #[serde(default)]
    pub channel_id: String,
    /// One is picked at random. Placeholders: {mention}, {name}, {n}, {away},
    /// {days}, {hours}; `<@id>` for anyone else.
    #[serde(default)]
    pub lines: Vec<String>,
    /// Who else may be pinged by the post.
    #[serde(default)]
    pub also_ping: Vec<String>,
    #[serde(default = "yes")]
    pub ping_member: bool,
    #[serde(default)]
    pub mode: Mode,
    /// Post instead of the usual welcome line; off posts under it.
    #[serde(default = "yes")]
    pub replace_normal: bool,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// For admins only.
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_by: String,
    #[serde(default)]
    pub created_ts: i64,
    #[serde(default)]
    pub updated_ts: i64,
    /// Kept by the bot when it posts.
    #[serde(default)]
    pub fired_count: i64,
    #[serde(default)]
    pub last_fired_ts: i64,
    #[serde(default)]
    pub last_fired_message_id: String,
}

impl Default for Welcome {
    fn default() -> Self {
        Self {
            id: 0,
            user_id: String::new(),
            user_name: String::new(),
            channel_id: String::new(),
            lines: Vec::new(),
            also_ping: Vec::new(),
            ping_member: true,
            mode: Mode::Once,
            replace_normal: true,
            enabled: true,
            note: String::new(),
            created_by: String::new(),
            created_ts: 0,
            updated_ts: 0,
            fired_count: 0,
            last_fired_ts: 0,
            last_fired_message_id: String::new(),
        }
    }
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS welcomes (
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id TEXT NOT NULL, body TEXT NOT NULL,
        updated_by INTEGER NOT NULL, updated_ts INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS welcomes_user ON welcomes (user_id);
    CREATE TABLE IF NOT EXISTS welcomes_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
";

pub(super) fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    if seed(conn, Utc::now().timestamp())? {
        tracing::info!("welcomes: Lucky's welcome is waiting for him");
    }
    Ok(())
}

// --- the words ----------------------------------------------------------------------

/// What the join log knows when the member arrives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// Joins counting this one; 0 when unknown.
    pub joins: u32,
    /// When they last left, if ever.
    pub last_leave: Option<i64>,
}

pub fn moment(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text.trim()).ok().map(|t| t.timestamp())
}

fn count(n: i64, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {}", one) } else { format!("{} {}", n, many) }
}

/// "5 hours", "7 days", "3 months": how long someone was away, in the biggest
/// unit that still reads naturally.
pub fn away_words(secs: i64) -> String {
    let secs = secs.max(0);
    const HOUR: i64 = 3600;
    const DAY: i64 = 86_400;
    if secs < HOUR {
        count((secs / 60).max(1), "minute", "minutes")
    } else if secs < 2 * DAY {
        count(secs / HOUR, "hour", "hours")
    } else if secs < 60 * DAY {
        count(secs / DAY, "day", "days")
    } else if secs < 365 * DAY {
        count(secs / (30 * DAY), "month", "months")
    } else {
        count(secs / (365 * DAY), "year", "years")
    }
}

/// Fills a line's placeholders. `{away}` reads "a while" when nobody saw them
/// leave; `{days}` and `{hours}` are then 0.
pub fn render(template: &str, user_id: &str, name: &str, facts: &Facts, now: i64) -> String {
    let away = facts.last_leave.map(|at| (now - at).max(0));
    let mention = if user_id.trim().is_empty() { name.to_string() } else { format!("<@{}>", user_id.trim()) };
    template
        .replace("{mention}", &mention)
        .replace("{name}", name)
        .replace("{n}", &super::super::ordinal(facts.joins.max(1)))
        .replace("{away}", &away.map_or_else(|| "a while".to_string(), away_words))
        .replace("{days}", &(away.unwrap_or(0) / 86_400).to_string())
        .replace("{hours}", &(away.unwrap_or(0) / 3600).to_string())
}

/// One of the lines, from `roll` in `[0, 1)`; blank ones never.
pub fn pick_line(lines: &[String], roll: f64) -> Option<&str> {
    let lines: Vec<&str> = lines.iter().map(String::as_str).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    Some(lines[((roll.clamp(0.0, 1.0) * lines.len() as f64) as usize).min(lines.len() - 1)])
}

/// Who the post may ping: the member when that's on, and the people listed.
/// Never @everyone or a role.
pub fn pings(w: &Welcome) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    let member = w.user_id.trim().parse::<u64>().ok().filter(|id| *id != 0);
    if w.ping_member {
        out.extend(member);
    }
    for id in w.also_ping.iter().filter_map(|p| p.trim().parse::<u64>().ok()).filter(|id| *id != 0) {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// The ids written as `<@id>` in a text.
pub fn mentioned(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<@") {
        let after = rest[start + 2..].trim_start_matches('!');
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() && after[digits.len()..].starts_with('>') && !out.contains(&digits) {
            out.push(digits.clone());
        }
        rest = &rest[start + 2..];
    }
    out
}

/// Where a welcome posts: its own channel, else the welcome channel.
pub fn channel_for(w: &Welcome, welcome_channel: Option<u64>) -> Option<u64> {
    w.channel_id.trim().parse::<u64>().ok().filter(|id| *id != 0).or(welcome_channel.filter(|id| *id != 0))
}

/// The bookkeeping after a welcome went out: counted, and a once-only welcome
/// switched off.
pub fn fired(w: &mut Welcome, now: i64, message_id: u64) {
    w.fired_count += 1;
    w.last_fired_ts = now;
    w.last_fired_message_id = message_id.to_string();
    if w.mode == Mode::Once {
        w.enabled = false;
    }
}

/// A Discord id: 17 to 20 digits.
pub fn snowflake(raw: &str) -> Option<u64> {
    let raw = raw.trim();
    ((17..=20).contains(&raw.len()) && raw.bytes().all(|b| b.is_ascii_digit())).then(|| raw.parse().ok()).flatten()
}

/// Checks the limits and tidies what the page sent; the panel checks the member
/// and channel against Discord on top of this.
pub fn tidy(mut w: Welcome) -> Result<Welcome, String> {
    w.user_id = w.user_id.trim().to_string();
    if w.user_id.is_empty() {
        return Err("Pick the member to welcome, or paste their Discord ID.".into());
    }
    w.user_name = w.user_name.trim().to_string();
    if w.user_name.chars().count() > MAX_NAME {
        return Err(format!("Keep the name under {} characters.", MAX_NAME));
    }
    w.channel_id = w.channel_id.trim().to_string();
    w.lines = w.lines.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
    if w.lines.is_empty() {
        return Err("Write the message to post when they join.".into());
    }
    if w.lines.len() > MAX_LINES {
        return Err(format!("Use at most {} versions of the message.", MAX_LINES));
    }
    if let Some(n) = w.lines.iter().position(|l| l.chars().count() > MAX_LINE) {
        return Err(format!("Message {} is longer than {} characters.", n + 1, MAX_LINE));
    }
    let mut also: Vec<String> = Vec::new();
    for raw in w.also_ping.iter().map(|p| p.trim()).filter(|p| !p.is_empty()) {
        if raw == w.user_id {
            continue;
        }
        if !raw.bytes().all(|b| b.is_ascii_digit()) || raw.len() > 20 {
            return Err(format!("“{}” isn't a Discord ID.", raw));
        }
        if !also.iter().any(|a| a == raw) {
            also.push(raw.to_string());
        }
    }
    if also.len() > MAX_ALSO_PING {
        return Err(format!("Ping at most {} other people.", MAX_ALSO_PING));
    }
    w.also_ping = also;
    w.note = w.note.trim().to_string();
    if w.note.chars().count() > MAX_NOTE {
        return Err(format!("Keep the note under {} characters.", MAX_NOTE));
    }
    Ok(w)
}

// --- the store ----------------------------------------------------------------------

fn from_row(id: i64, body: &str) -> Option<Welcome> {
    serde_json::from_str::<Welcome>(body).ok().map(|w| Welcome { id, ..w })
}

pub fn list() -> Vec<Welcome> {
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT id, body FROM welcomes ORDER BY id") else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| rows.flatten().filter_map(|(id, body)| from_row(id, &body)).collect())
        .unwrap_or_default()
}

pub fn get(id: i64) -> Option<Welcome> {
    let db = DB.get()?;
    let body: String = db
        .lock()
        .query_row("SELECT body FROM welcomes WHERE id = ?1", params![id], |r| r.get(0))
        .optional()
        .ok()
        .flatten()?;
    from_row(id, &body)
}

/// The switched-on welcome for a member who just joined, if there is one: a
/// single indexed read.
pub fn waiting_for(user: u64) -> Option<Welcome> {
    let db = DB.get()?;
    let conn = db.lock();
    let mut stmt = conn.prepare("SELECT id, body FROM welcomes WHERE user_id = ?1 ORDER BY id").ok()?;
    let rows: Vec<(i64, String)> =
        stmt.query_map(params![user.to_string()], |r| Ok((r.get(0)?, r.get(1)?))).ok()?.flatten().collect();
    rows.iter().filter_map(|(id, body)| from_row(*id, body)).find(|w| w.enabled)
}

/// Saves a welcome, new when its id is 0, and returns its id. A change made on
/// the panel (`by` not 0) goes to the audit trail as `welcome:<id>`.
pub fn save(w: &Welcome, by: u64) -> anyhow::Result<i64> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let conn = db.lock();
    let now = Utc::now().timestamp();
    let mut w = w.clone();
    w.updated_ts = now;
    if w.created_ts == 0 {
        w.created_ts = now;
    }
    let (id, old) = if w.id == 0 {
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM welcomes", [], |r| r.get(0))?;
        if rows as usize >= MAX_RULES {
            anyhow::bail!("there are already {} welcomes", MAX_RULES);
        }
        conn.execute(
            "INSERT INTO welcomes (user_id, body, updated_by, updated_ts) VALUES (?1, '{}', ?2, ?3)",
            params![w.user_id, by as i64, now],
        )?;
        (conn.last_insert_rowid(), None)
    } else {
        let old: Option<String> =
            conn.query_row("SELECT body FROM welcomes WHERE id = ?1", params![w.id], |r| r.get(0)).optional()?;
        if old.is_none() {
            anyhow::bail!("no welcome {}", w.id);
        }
        (w.id, old)
    };
    w.id = id;
    let body = serde_json::to_string(&w)?;
    conn.execute(
        "UPDATE welcomes SET user_id = ?1, body = ?2, updated_by = ?3, updated_ts = ?4 WHERE id = ?5",
        params![w.user_id, body, by as i64, now, id],
    )?;
    if by != 0 {
        conn.execute(
            "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![now, by as i64, format!("welcome:{}", id), old, body],
        )?;
    }
    Ok(id)
}

pub fn delete(id: i64, by: u64) -> anyhow::Result<()> {
    let db = DB.get().ok_or_else(|| anyhow::anyhow!("control store not open"))?;
    let conn = db.lock();
    let old: Option<String> =
        conn.query_row("SELECT body FROM welcomes WHERE id = ?1", params![id], |r| r.get(0)).optional()?;
    conn.execute("DELETE FROM welcomes WHERE id = ?1", params![id])?;
    conn.execute(
        "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, NULL)",
        params![Utc::now().timestamp(), by as i64, format!("welcome:{}", id), old],
    )?;
    Ok(())
}

/// Counts a post on the stored copy read afresh, so an edit saved on the panel
/// a moment ago is kept. Like the reminders scheduler, the bot's own
/// bookkeeping isn't written to the audit trail.
pub fn record_fired(id: i64, now: i64, message_id: u64) -> Option<Welcome> {
    let db = DB.get()?;
    let conn = db.lock();
    let body: String = conn.query_row("SELECT body FROM welcomes WHERE id = ?1", params![id], |r| r.get(0)).optional().ok()??;
    let mut w = from_row(id, &body)?;
    fired(&mut w, now, message_id);
    let body = serde_json::to_string(&w).ok()?;
    conn.execute("UPDATE welcomes SET body = ?1 WHERE id = ?2", params![body, id]).ok()?;
    Some(w)
}

// --- the seed -------------------------------------------------------------------------

/// Def Not Kohli and MahoganyDesk, who missed Lucky most.
const KOHLI: &str = "302861753409208322";
const MAHOGANY: &str = "1453059765692272765";

/// The welcome asked for on 12 Sep: Lucky, and who missed him.
pub fn lucky() -> Welcome {
    let both = format!("<@{}> aur <@{}>", KOHLI, MAHOGANY);
    Welcome {
        user_id: "459076776266694670".into(),
        user_name: "Lucky".into(),
        channel_id: "1516492867642593443".into(),
        lines: vec![
            format!("🎉 {{mention}} wapas aa gaya! **{{away}}** ho gaye the. {} ne tujhe sabse zyada miss kiya — roz poochte the kab aayega. Welcome home ❤️", both),
            format!("Dekho kaun laut aaya! 🥹 {{mention}}, poore **{{away}}** baad. {} toh roz tera naam le rahe the — ab jaake unko chain milega. Welcome back, bhai ❤️", both),
            format!("{{mention}} is BACK 🎊 **{{away}}** ka break bahut lamba tha yaar. Sach bolein toh {} ne tujhe sabse zyada miss kiya. Ab kahin mat jaana, teri seat khaali rakhi thi 🪑❤️", both),
        ],
        also_ping: vec![KOHLI.into(), MAHOGANY.into()],
        ping_member: true,
        mode: Mode::Once,
        replace_normal: true,
        enabled: true,
        note: "From the 12 Sep request: tell Lucky how much Def Not Kohli and MahoganyDesk missed him.".into(),
        created_by: "0".into(),
        ..Default::default()
    }
}

/// Puts Lucky's welcome in the store once: only while the table is empty and
/// has never been seeded, so deleting it on the panel doesn't bring it back.
/// True when it was added.
fn seed(conn: &Connection, now: i64) -> rusqlite::Result<bool> {
    let seeded = conn
        .query_row("SELECT 1 FROM welcomes_meta WHERE key = 'seeded'", [], |_| Ok(()))
        .optional()?
        .is_some();
    if seeded {
        return Ok(false);
    }
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM welcomes", [], |r| r.get(0))?;
    let added = rows == 0;
    if added {
        let w = Welcome { created_ts: now, updated_ts: now, ..lucky() };
        conn.execute(
            "INSERT INTO welcomes (user_id, body, updated_by, updated_ts) VALUES (?1, '{}', 0, ?2)",
            params![w.user_id, now],
        )?;
        let id = conn.last_insert_rowid();
        let body = serde_json::to_string(&Welcome { id, ..w }).unwrap_or_default();
        conn.execute("UPDATE welcomes SET body = ?1 WHERE id = ?2", params![body, id])?;
    }
    conn.execute("INSERT OR REPLACE INTO welcomes_meta (key, value) VALUES ('seeded', ?1)", params![now.to_string()])?;
    Ok(added)
}

// --- posting ------------------------------------------------------------------------------

async fn send(ctx: &Context, channel: ChannelId, text: String, allowed: Vec<u64>) -> Result<u64, String> {
    let mentions = CreateAllowedMentions::new().users(allowed.into_iter().map(UserId::new).collect::<Vec<_>>());
    let message = CreateMessage::new().content(text).allowed_mentions(mentions);
    match tokio::time::timeout(HTTP_WAIT, channel.send_message(&ctx.http, message)).await {
        Ok(Ok(sent)) => Ok(sent.id.get()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!("no answer from Discord in {}s", HTTP_WAIT.as_secs())),
    }
}

/// Posts a member's special welcome as they join, and counts it. A failure is
/// logged and leaves the welcome switched on for their next join.
pub async fn greet(ctx: &Context, w: &Welcome, channel: u64, display_name: &str, facts: &Facts) {
    let name = if w.user_name.trim().is_empty() { display_name } else { w.user_name.trim() };
    let now = Utc::now().timestamp();
    let Some(line) = pick_line(&w.lines, rand::random::<f64>()) else {
        tracing::warn!("welcomes: special welcome for {} has no message, skipped", name);
        return;
    };
    let text = render(line, &w.user_id, name, facts, now);
    match send(ctx, ChannelId::new(channel), text, pings(w)).await {
        Ok(message) => {
            tracing::info!("welcomes: special welcome for {} posted", name);
            match record_fired(w.id, now, message) {
                Some(fresh) if !fresh.enabled && w.enabled => {
                    tracing::info!("welcomes: special welcome for {} was once only, switched off", name)
                }
                Some(_) => {}
                None => tracing::warn!("welcomes: special welcome {} posted but not counted", w.id),
            }
        }
        Err(err) => tracing::warn!("welcomes: special welcome for {} not posted: {}", name, err),
    }
}

/// Posts text with no pings at all: the panel's "Send a test now".
pub async fn post_unpinged(ctx: &Context, channel: u64, text: String) -> Result<(), String> {
    send(ctx, ChannelId::new(channel), text, Vec::new()).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_789_381_800;

    #[test]
    fn away_reads_in_the_natural_unit() {
        assert_eq!(away_words(20), "1 minute");
        assert_eq!(away_words(45 * 60), "45 minutes");
        assert_eq!(away_words(3600), "1 hour");
        assert_eq!(away_words(5 * 3600 + 1200), "5 hours");
        assert_eq!(away_words(47 * 3600), "47 hours");
        assert_eq!(away_words(7 * 86_400 + 3600), "7 days");
        assert_eq!(away_words(59 * 86_400), "59 days");
        assert_eq!(away_words(95 * 86_400), "3 months");
        assert_eq!(away_words(800 * 86_400), "2 years");
        assert_eq!(away_words(-50), "1 minute");
    }

    #[test]
    fn placeholders_fill_from_the_join_log() {
        let facts = Facts { joins: 3, last_leave: Some(NOW - 7 * 86_400 - 5 * 3600) };
        let text = render("{mention} {name} {n} {away} {days}/{hours} <@42>", "459076776266694670", "Lucky", &facts, NOW);
        assert_eq!(text, "<@459076776266694670> Lucky 3rd 7 days 7/173 <@42>");
        // Never seen leaving: a while, and zeros.
        let first = Facts { joins: 1, last_leave: None };
        assert_eq!(render("{n} after {away} ({days}d {hours}h)", "1", "x", &first, NOW), "1st after a while (0d 0h)");
        // Unknown joins read as a first; ordinals past the teens.
        assert_eq!(render("{n}", "1", "x", &Facts::default(), NOW), "1st");
        assert_eq!(render("{n}", "1", "x", &Facts { joins: 12, last_leave: None }, NOW), "12th");
        assert_eq!(render("{n}", "1", "x", &Facts { joins: 22, last_leave: None }, NOW), "22nd");
        // A clock behind the leave reads zero, and no member id means {mention} is the name.
        let ahead = Facts { joins: 2, last_leave: Some(NOW + 100) };
        assert_eq!(render("{mention} {hours}", "", "Lucky", &ahead, NOW), "Lucky 0");
    }

    #[test]
    fn only_the_member_and_the_listed_people_are_pinged() {
        let w = Welcome { user_id: "459076776266694670".into(), also_ping: vec!["302861753409208322".into(), " 302861753409208322".into(), "x".into(), "0".into()], ..Default::default() };
        assert_eq!(pings(&w), vec![459076776266694670, 302861753409208322]);
        let quiet = Welcome { ping_member: false, ..w.clone() };
        assert_eq!(pings(&quiet), vec![302861753409208322]);
        assert!(pings(&Welcome { ping_member: false, also_ping: vec![], ..w }).is_empty());
        assert_eq!(mentioned("hi <@12> and <@!34>, <@12> again, <@x> <@56"), vec!["12".to_string(), "34".to_string()]);
    }

    #[test]
    fn once_switches_itself_off_and_every_stays_on() {
        let mut once = Welcome { user_id: "1".into(), ..Default::default() };
        fired(&mut once, NOW, 99);
        assert_eq!((once.enabled, once.fired_count, once.last_fired_ts, once.last_fired_message_id.as_str()), (false, 1, NOW, "99"));
        let mut every = Welcome { mode: Mode::Every, fired_count: 3, ..Default::default() };
        fired(&mut every, NOW + 5, 100);
        assert_eq!((every.enabled, every.fired_count, every.last_fired_ts), (true, 4, NOW + 5));
    }

    #[test]
    fn channels_fall_back_to_the_welcome_channel() {
        let w = Welcome::default();
        assert_eq!(channel_for(&w, Some(11)), Some(11));
        assert_eq!(channel_for(&w, None), None);
        assert_eq!(channel_for(&Welcome { channel_id: "23".into(), ..Default::default() }, Some(11)), Some(23));
        assert_eq!(channel_for(&Welcome { channel_id: "0".into(), ..Default::default() }, Some(0)), None);
    }

    #[test]
    fn lines_are_picked_at_random_never_blank() {
        let lines: Vec<String> = ["a", " ", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_line(&lines, 0.0), Some("a"));
        assert_eq!(pick_line(&lines, 0.99), Some("b"));
        assert_eq!(pick_line(&lines, 1.0), Some("b"));
        assert_eq!(pick_line(&[" ".to_string()], 0.3), None);
    }

    #[test]
    fn validation_keeps_to_the_limits() {
        assert_eq!(snowflake("459076776266694670"), Some(459076776266694670));
        assert_eq!(snowflake(" 12345678901234567 "), Some(12345678901234567));
        for bad in ["1234567890123456", "123456789012345678901", "45907677626669467x", "", "-45907677626669467"] {
            assert_eq!(snowflake(bad), None, "{bad}");
        }
        let good = Welcome { user_id: " 459076776266694670 ".into(), lines: vec!["  hi {mention} ".into(), "".into()], also_ping: vec!["459076776266694670".into(), "302861753409208322".into(), "302861753409208322".into()], note: " n ".into(), ..Default::default() };
        let tidy_one = tidy(good.clone()).unwrap();
        assert_eq!(tidy_one.user_id, "459076776266694670");
        assert_eq!(tidy_one.lines, vec!["hi {mention}".to_string()]);
        assert_eq!(tidy_one.also_ping, vec!["302861753409208322".to_string()], "the member and repeats drop out");
        assert_eq!(tidy_one.note, "n");
        let fails = |w: Welcome, says: &str| {
            let err = tidy(w).unwrap_err();
            assert!(err.contains(says), "{err}");
        };
        fails(Welcome { user_id: " ".into(), ..good.clone() }, "Pick the member");
        fails(Welcome { lines: vec![" ".into()], ..good.clone() }, "Write the message");
        fails(Welcome { lines: vec!["x".into(); 11], ..good.clone() }, "at most 10");
        fails(Welcome { lines: vec!["é".repeat(1501)], ..good.clone() }, "longer than 1500");
        assert!(tidy(Welcome { lines: vec!["é".repeat(1500)], ..good.clone() }).is_ok());
        fails(Welcome { also_ping: (0..11).map(|i| format!("30286175340920{:04}", i)).collect(), ..good.clone() }, "at most 10 other");
        fails(Welcome { also_ping: vec!["@kohli".into()], ..good.clone() }, "isn't a Discord ID");
        fails(Welcome { note: "n".repeat(501), ..good.clone() }, "note under 500");
        fails(Welcome { user_name: "n".repeat(101), ..good }, "name under 100");
    }

    #[test]
    fn stored_bodies_fill_in_defaults() {
        let w: Welcome = serde_json::from_str(r#"{"user_id":"1","lines":["hi"]}"#).unwrap();
        assert!(w.enabled && w.ping_member && w.replace_normal);
        assert_eq!(w.mode, Mode::Once);
        let every: Welcome = serde_json::from_str(r#"{"user_id":"1","mode":"every"}"#).unwrap();
        assert_eq!(every.mode, Mode::Every);
        assert!(serde_json::from_str::<Welcome>(r#"{"user_id":"1","mode":"sometimes"}"#).is_err());
    }

    #[test]
    fn lucky_is_seeded_once_and_stays_deleted() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        assert!(seed(&conn, NOW).unwrap());
        let body: String = conn.query_row("SELECT body FROM welcomes", [], |r| r.get(0)).unwrap();
        let w: Welcome = serde_json::from_str(&body).unwrap();
        assert_eq!((w.user_id.as_str(), w.user_name.as_str(), w.channel_id.as_str()), ("459076776266694670", "Lucky", "1516492867642593443"));
        assert!(w.enabled && w.ping_member && w.replace_normal && w.mode == Mode::Once && w.created_ts == NOW);
        assert_eq!(w.also_ping, vec![KOHLI.to_string(), MAHOGANY.to_string()]);
        assert!(w.lines.len() >= 2 && w.lines.iter().all(|l| l.contains("{mention}") && l.contains("{away}") && l.contains(KOHLI) && l.contains(MAHOGANY)));
        // Every line pings only who it should.
        for line in &w.lines {
            let text = render(line, &w.user_id, &w.user_name, &Facts { joins: 2, last_leave: Some(NOW - 7 * 86_400) }, NOW);
            assert!(text.contains("<@459076776266694670>") && text.contains("7 days") && !text.contains('{'), "{text}");
            assert!(mentioned(&text).iter().all(|id| pings(&w).contains(&id.parse().unwrap())));
        }
        // Again: nothing new. Deleted: still nothing.
        assert!(!seed(&conn, NOW + 1).unwrap());
        conn.execute("DELETE FROM welcomes", []).unwrap();
        assert!(!seed(&conn, NOW + 2).unwrap());
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM welcomes", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 0);
        // A store that already had welcomes before seeding existed isn't given Lucky's.
        let other = Connection::open_in_memory().unwrap();
        other.execute_batch(SCHEMA).unwrap();
        other.execute("INSERT INTO welcomes (user_id, body, updated_by, updated_ts) VALUES ('5', '{}', 1, 1)", []).unwrap();
        assert!(!seed(&other, NOW).unwrap());
        assert!(!seed(&other, NOW).unwrap());
    }
}
