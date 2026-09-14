//! Members' own reminders: "@Loduchand remind me in 2 hours to call mum", or
//! `/remind`. Saved in control.db so a restart loses nothing; the reminders
//! scheduler posts them when they fall due, pinging only the member.

use chrono::{Datelike, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use super::DB;

/// Longest a reminder can be set ahead.
pub const MAX_AHEAD_DAYS: i64 = 60;
/// Reminders one member can have waiting at once.
pub const MAX_PENDING: i64 = 20;
pub const MAX_TEXT: usize = 400;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Memo {
    pub id: i64,
    /// Discord ids travel as strings.
    pub user_id: String,
    pub channel_id: String,
    pub text: String,
    pub due_ts: i64,
    pub created_ts: i64,
    /// "chat" (asked the bot), "command" (/remind) or "panel".
    pub via: String,
    /// "pending", "sent", "cancelled" or "failed".
    pub status: String,
    pub sent_ts: i64,
    /// Who set it, when an admin set it for someone else ("0" otherwise).
    pub set_by: String,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS member_reminders (
        id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, channel_id INTEGER NOT NULL,
        text TEXT NOT NULL, due_ts INTEGER NOT NULL, created_ts INTEGER NOT NULL, via TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending', sent_ts INTEGER NOT NULL DEFAULT 0, tries INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS member_reminders_due ON member_reminders (status, due_ts);
    CREATE INDEX IF NOT EXISTS member_reminders_user ON member_reminders (user_id, status);
";

pub(super) fn migrate(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    // Added after the first version: who set a reminder for someone else.
    let has_set_by: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('member_reminders') WHERE name = 'set_by'")?
        .exists([])?;
    if !has_set_by {
        conn.execute_batch("ALTER TABLE member_reminders ADD COLUMN set_by INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

fn row(r: &rusqlite::Row) -> rusqlite::Result<Memo> {
    Ok(Memo {
        id: r.get(0)?,
        user_id: r.get::<_, i64>(1)?.to_string(),
        channel_id: r.get::<_, i64>(2)?.to_string(),
        text: r.get(3)?,
        due_ts: r.get(4)?,
        created_ts: r.get(5)?,
        via: r.get(6)?,
        status: r.get(7)?,
        sent_ts: r.get(8)?,
        set_by: r.get::<_, i64>(9)?.to_string(),
    })
}

const COLUMNS: &str = "id, user_id, channel_id, text, due_ts, created_ts, via, status, sent_ts, set_by";

// --- when --------------------------------------------------------------------------

fn ist() -> chrono::FixedOffset {
    super::super::stats::ist()
}

/// A time of day like "21:30", "9pm", "9:15 am", "noon".
fn clock(text: &str) -> Option<NaiveTime> {
    let t = text.trim().to_ascii_lowercase().replace('.', ":");
    match t.as_str() {
        "noon" | "midday" => return NaiveTime::from_hms_opt(12, 0, 0),
        "midnight" => return NaiveTime::from_hms_opt(0, 0, 0),
        _ => {}
    }
    let (body, meridiem) = if let Some(b) = t.strip_suffix("am") {
        (b.trim(), Some(false))
    } else if let Some(b) = t.strip_suffix("pm") {
        (b.trim(), Some(true))
    } else {
        (t.as_str(), None)
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    if meridiem.is_none() && !body.contains(':') {
        return None; // a bare "9" is too ambiguous
    }
    let h = match meridiem {
        Some(pm) if (1..=12).contains(&h) => (h % 12) + if pm { 12 } else { 0 },
        Some(_) => return None,
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0)
}

/// A span like "10 minutes", "2h", "1h30m", "3 days".
fn span(text: &str) -> Option<i64> {
    let t = text.trim().to_ascii_lowercase().replace(" and ", " ");
    let mut total = 0i64;
    let mut number = String::new();
    let mut unit = String::new();
    let mut found = false;
    let mut flush = |number: &mut String, unit: &mut String| -> Option<()> {
        if number.is_empty() {
            return if unit.trim().is_empty() { Some(()) } else { None };
        }
        let n: f64 = number.parse().ok()?;
        let secs = match unit.trim() {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
            "d" | "day" | "days" => 86_400.0,
            "w" | "wk" | "week" | "weeks" => 604_800.0,
            _ => return None,
        };
        total += (n * secs) as i64;
        found = true;
        number.clear();
        unit.clear();
        Some(())
    };
    for c in t.chars() {
        if c.is_ascii_digit() || c == '.' {
            if !unit.trim().is_empty() {
                flush(&mut number, &mut unit)?;
            }
            number.push(c);
        } else if c.is_alphabetic() {
            unit.push(c);
        } else if c.is_whitespace() || c == ',' {
            if !unit.is_empty() {
                flush(&mut number, &mut unit)?;
            }
        } else {
            return None;
        }
    }
    flush(&mut number, &mut unit)?;
    (found && total > 0).then_some(total)
}

/// When a reminder is due, from how people say it (India time), or `None` if
/// it can't be read: "in 2 hours", "30m", "1h30m", "at 9pm", "21:30",
/// "tomorrow 9am", "tomorrow at 18:00", "2026-09-20 18:00", "20/09 18:00".
pub fn parse_when(raw: &str, now: i64) -> Option<i64> {
    let text = raw.trim().to_ascii_lowercase();
    let text = text.strip_prefix("in ").unwrap_or(&text).trim().to_string();
    let local_now = ist().timestamp_opt(now, 0).single()?;
    let at_day = |date: NaiveDate, time: NaiveTime| ist().from_local_datetime(&date.and_time(time)).single().map(|t| t.timestamp());

    if let Some(secs) = span(&text) {
        return Some(now + secs);
    }
    for prefix in ["tomorrow at ", "tomorrow ", "tmrw at ", "tmrw "] {
        if let Some(rest) = text.strip_prefix(prefix) {
            return at_day(local_now.date_naive() + Duration::days(1), clock(rest)?);
        }
    }
    if text == "tomorrow" || text == "tmrw" {
        return at_day(local_now.date_naive() + Duration::days(1), NaiveTime::from_hms_opt(9, 0, 0)?);
    }
    let bare = text.strip_prefix("at ").or_else(|| text.strip_prefix("today at ")).or_else(|| text.strip_prefix("today ")).unwrap_or(&text);
    if let Some(time) = clock(bare) {
        let today = at_day(local_now.date_naive(), time)?;
        return Some(if today > now { today } else { at_day(local_now.date_naive() + Duration::days(1), time)? });
    }
    // A date and a time.
    let (date_part, time_part) = text.split_once(' ')?;
    let time = clock(time_part.trim().strip_prefix("at ").unwrap_or(time_part.trim()))?;
    let date = NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok().or_else(|| {
        let (d, m) = date_part.split_once('/')?;
        let (d, m) = (d.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
        let this_year = NaiveDate::from_ymd_opt(local_now.year(), m, d)?;
        Some(if this_year < local_now.date_naive() { NaiveDate::from_ymd_opt(local_now.year() + 1, m, d)? } else { this_year })
    })?;
    at_day(date, time)
}

/// "today at 21:30", "tomorrow at 09:00", "Sat 20 Sep at 18:00" - India time.
pub fn describe(due: i64, now: i64) -> String {
    let (Some(d), Some(n)) = (ist().timestamp_opt(due, 0).single(), ist().timestamp_opt(now, 0).single()) else {
        return format!("<t:{}:f>", due);
    };
    let day = if d.date_naive() == n.date_naive() {
        "today".to_string()
    } else if d.date_naive() == n.date_naive() + Duration::days(1) {
        "tomorrow".to_string()
    } else {
        d.format("%a %-d %b").to_string()
    };
    format!("{} at {} IST", day, d.format("%H:%M"))
}

// --- store ----------------------------------------------------------------------------

/// Saves a reminder for `user` after checking it; the error says what to fix.
/// `set_by` is who asked: someone setting a reminder for another member must
/// be a bot admin.
pub fn create(user: u64, channel: u64, text: &str, due: i64, via: &str, set_by: u64) -> Result<Memo, String> {
    if set_by != 0 && set_by != user && !super::super::admin_ids().contains(&set_by) {
        return Err("Only admins can set reminders for someone else - you can remind yourself.".into());
    }
    let now = Utc::now().timestamp();
    let text = text.trim();
    if text.is_empty() {
        return Err("What should the reminder say?".into());
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("Keep the reminder under {} characters.", MAX_TEXT));
    }
    if due < now + 30 {
        return Err("That time has already passed (or is under a minute away).".into());
    }
    if due > now + MAX_AHEAD_DAYS * 86_400 {
        return Err(format!("Reminders can be at most {} days ahead.", MAX_AHEAD_DAYS));
    }
    let db = DB.get().ok_or("Reminders aren't available right now.")?;
    let conn = db.lock();
    let pending: i64 = conn
        .query_row("SELECT COUNT(*) FROM member_reminders WHERE user_id = ?1 AND status = 'pending'", params![user as i64], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if pending >= MAX_PENDING {
        return Err(format!("You already have {} reminders waiting. Cancel one with /reminders first.", MAX_PENDING));
    }
    conn.execute(
        "INSERT INTO member_reminders (user_id, channel_id, text, due_ts, created_ts, via, set_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![user as i64, channel as i64, text, due, now, via, if set_by == user { 0 } else { set_by as i64 }],
    )
    .map_err(|e| e.to_string())?;
    let id = conn.last_insert_rowid();
    conn.query_row(&format!("SELECT {} FROM member_reminders WHERE id = ?1", COLUMNS), params![id], row)
        .map_err(|e| e.to_string())
}

/// A member's waiting reminders, soonest first.
pub fn pending_for(user: u64) -> Vec<Memo> {
    let Some(db) = DB.get() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {} FROM member_reminders WHERE user_id = ?1 AND status = 'pending' ORDER BY due_ts",
        COLUMNS
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![user as i64], row).map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Every reminder for the panel: waiting ones first, then the latest done.
pub fn all(limit: usize) -> Vec<Memo> {
    let Some(db) = DB.get() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {} FROM member_reminders ORDER BY (status = 'pending') DESC, CASE WHEN status = 'pending' THEN due_ts ELSE -created_ts END LIMIT ?1",
        COLUMNS
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![limit as i64], row).map(|r| r.flatten().collect()).unwrap_or_default()
}

/// Cancels a waiting reminder; `owner` limits it to that member's own (None for admins).
pub fn cancel(id: i64, owner: Option<u64>) -> bool {
    let Some(db) = DB.get() else { return false };
    let conn = db.lock();
    let changed = match owner {
        Some(user) => conn.execute(
            "UPDATE member_reminders SET status = 'cancelled' WHERE id = ?1 AND user_id = ?2 AND status = 'pending'",
            params![id, user as i64],
        ),
        None => conn.execute("UPDATE member_reminders SET status = 'cancelled' WHERE id = ?1 AND status = 'pending'", params![id]),
    };
    changed.unwrap_or(0) > 0
}

/// Reminders due by `now`, oldest first, at most `limit`.
pub(super) fn due(now: i64, limit: usize) -> Vec<Memo> {
    let Some(db) = DB.get() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {} FROM member_reminders WHERE status = 'pending' AND due_ts <= ?1 ORDER BY due_ts LIMIT ?2",
        COLUMNS
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![now, limit as i64], row).map(|r| r.flatten().collect()).unwrap_or_default()
}

pub(super) fn mark_sent(id: i64, now: i64) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute("UPDATE member_reminders SET status = 'sent', sent_ts = ?2 WHERE id = ?1", params![id, now]);
    }
}

/// A failed post is tried again on later ticks, up to three times.
pub(super) fn mark_failed_try(id: i64) {
    if let Some(db) = DB.get() {
        let conn = db.lock();
        let tries: i64 = conn
            .query_row("SELECT tries FROM member_reminders WHERE id = ?1", params![id], |r| r.get(0))
            .optional()
            .ok()
            .flatten()
            .unwrap_or(0);
        let status = if tries + 1 >= 3 { "failed" } else { "pending" };
        let _ = conn.execute("UPDATE member_reminders SET tries = tries + 1, status = ?2 WHERE id = ?1", params![id, status]);
    }
}

/// What a reminder posts.
pub fn message_text(m: &Memo) -> String {
    if m.set_by != "0" && m.set_by != m.user_id {
        format!("⏰ <@{}> reminder from <@{}>: {}", m.user_id, m.set_by, m.text)
    } else {
        format!("⏰ <@{}> reminder: {}", m.user_id, m.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-14 16:00 India time.
    const NOW: i64 = 1_789_381_800;

    fn ist_text(ts: i64) -> String {
        ist().timestamp_opt(ts, 0).unwrap().format("%Y-%m-%d %H:%M").to_string()
    }

    #[test]
    fn spans_and_relative_times() {
        assert_eq!(parse_when("in 2 hours", NOW), Some(NOW + 7200));
        assert_eq!(parse_when("30m", NOW), Some(NOW + 1800));
        assert_eq!(parse_when("1h30m", NOW), Some(NOW + 5400));
        assert_eq!(parse_when("in 1 hour and 15 minutes", NOW), Some(NOW + 4500));
        assert_eq!(parse_when("3 days", NOW), Some(NOW + 3 * 86_400));
        assert_eq!(parse_when("in a bit", NOW), None);
    }

    #[test]
    fn clock_times_today_or_tomorrow() {
        assert_eq!(ist_text(NOW), "2026-09-14 16:00");
        assert_eq!(ist_text(parse_when("at 9pm", NOW).unwrap()), "2026-09-14 21:00");
        assert_eq!(ist_text(parse_when("21:30", NOW).unwrap()), "2026-09-14 21:30");
        assert_eq!(ist_text(parse_when("9am", NOW).unwrap()), "2026-09-15 09:00", "already past today");
        assert_eq!(ist_text(parse_when("tomorrow 9am", NOW).unwrap()), "2026-09-15 09:00");
        assert_eq!(ist_text(parse_when("tomorrow at 18:15", NOW).unwrap()), "2026-09-15 18:15");
        assert_eq!(ist_text(parse_when("2026-09-20 18:00", NOW).unwrap()), "2026-09-20 18:00");
        assert_eq!(ist_text(parse_when("20/09 6pm", NOW).unwrap()), "2026-09-20 18:00");
        assert_eq!(parse_when("9", NOW), None, "a bare number is ambiguous");
        assert_eq!(parse_when("13pm", NOW), None);
    }

    #[test]
    fn descriptions_say_the_day() {
        assert_eq!(describe(NOW + 3600, NOW), "today at 17:00 IST");
        assert_eq!(describe(NOW + 18 * 3600, NOW), "tomorrow at 10:00 IST");
        assert_eq!(describe(parse_when("2026-09-20 18:00", NOW).unwrap(), NOW), "Sun 20 Sep at 18:00 IST");
    }
}
