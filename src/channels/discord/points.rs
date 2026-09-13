//! The house points ledger.
//!
//! Every point is one row: who earned it, for which house, from what, how many,
//! and on which India day. Keeping the PERSON and not just the house is what the
//! monthly Nitro draw needs, and what lets anyone see where their points came
//! from. Nothing is ever edited or deleted - a mistake is corrected with a
//! negative row - so the history stays honest.
//!
//! Caps live here, at the moment of writing, rather than in each game. A game
//! only says "this person earned 3 from the quiz"; the ledger decides how much of
//! that still fits under today's limit, so no game can forget to apply one.

use std::collections::HashMap;

use chrono::{Datelike, Duration, TimeZone};
use rusqlite::{Connection, OptionalExtension, params};

use super::house::{HOUSES, House, house};

/// Where points came from. Also decides how much one person may earn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Chat,
    Voice,
    Quiz,
    Koto,
    Anagram,
    Cat,
    Arena,
    Royale,
    Snitch,
    GoldenSnitch,
    Weekly,
    Mod,
}

/// How much one person may earn from a source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    PerDay(i64),
    /// The weekly scan: counted separately for each channel.
    PerWeekPerChannel(i64),
    None,
}

impl Source {
    pub const ALL: [Source; 12] = [
        Source::Chat,
        Source::Voice,
        Source::Quiz,
        Source::Koto,
        Source::Anagram,
        Source::Cat,
        Source::Arena,
        Source::Royale,
        Source::Snitch,
        Source::GoldenSnitch,
        Source::Weekly,
        Source::Mod,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Source::Chat => "chat",
            Source::Voice => "voice",
            Source::Quiz => "quiz",
            Source::Koto => "koto",
            Source::Anagram => "anagram",
            Source::Cat => "cat",
            Source::Arena => "arena",
            Source::Royale => "royale",
            Source::Snitch => "snitch",
            Source::GoldenSnitch => "golden_snitch",
            Source::Weekly => "weekly",
            Source::Mod => "mod",
        }
    }

    pub fn from_key(key: &str) -> Option<Source> {
        Self::ALL.into_iter().find(|source| source.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::Chat => "💬 Chat",
            Source::Voice => "🎙️ Voice",
            Source::Quiz => "🧠 Quiz",
            Source::Koto => "🔤 Koto",
            Source::Anagram => "🔡 Anagram",
            Source::Cat => "🐱 Cat Bot",
            Source::Arena => "⚔️ Arena",
            Source::Royale => "👑 Battle Royale",
            Source::Snitch => "🪽 Snitch",
            Source::GoldenSnitch => "🥇 Golden Snitch",
            Source::Weekly => "📝 Weekly posts",
            Source::Mod => "🛡️ Mods",
        }
    }

    pub fn cap(self) -> Cap {
        match self {
            Source::Chat | Source::Voice => Cap::PerDay(1),
            Source::Quiz | Source::Anagram | Source::Snitch => Cap::PerDay(6),
            Source::Koto => Cap::PerDay(4),
            Source::Cat | Source::Arena => Cap::PerDay(3),
            Source::Weekly => Cap::PerWeekPerChannel(3),
            // A battle royale is a rare event, the Golden Snitch is meant to be a
            // jackpot, and mods decide their own amounts.
            Source::Royale | Source::GoldenSnitch | Source::Mod => Cap::None,
        }
    }
}

/// What happened to an entry.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// This many points were written - possibly fewer than asked, if a cap bit.
    Granted(i64),
    /// Already at the limit for this source; nothing earned.
    Capped,
    /// This exact event was counted before.
    Duplicate,
}

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS ledger (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        user_id INTEGER,
        house TEXT NOT NULL,
        source TEXT NOT NULL,
        scope TEXT,
        points INTEGER NOT NULL,
        reason TEXT NOT NULL DEFAULT '',
        awarded_by INTEGER,
        day TEXT NOT NULL,
        ts INTEGER NOT NULL,
        dedupe TEXT UNIQUE);
    CREATE INDEX IF NOT EXISTS ledger_cap ON ledger (user_id, source, day);
    CREATE INDEX IF NOT EXISTS ledger_house_ts ON ledger (house, ts);";

/// Folds the old house-only `awards` rows into the ledger. Safe at every start:
/// each old row carries its own dedupe key, so it lands exactly once.
pub const MIGRATE_AWARDS: &str = "
    INSERT OR IGNORE INTO ledger (user_id, house, source, scope, points, reason, awarded_by, day, ts, dedupe)
    SELECT NULL, house, 'mod', NULL, points, reason, awarded_by, date(ts + 19800, 'unixepoch'), ts,
           'legacy-award:' || id
    FROM awards";

/// One thing to write.
pub struct Entry<'a> {
    /// `None` for a house-only award, which is never capped.
    pub user: Option<u64>,
    pub house: &'a House,
    pub source: Source,
    /// For the weekly scan: the channel the points were earned in.
    pub scope: Option<String>,
    pub points: i64,
    pub reason: &'a str,
    pub by: Option<u64>,
    /// Names this exact event, so replaying it can never score twice.
    pub dedupe: Option<String>,
}

fn ist() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset")
}

/// The India calendar day a moment falls on, as the caps count days.
pub fn ist_day(ts: i64) -> String {
    ist().timestamp_opt(ts, 0).single().map(|t| t.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// The Monday that starts a moment's India week.
pub fn week_start_day(ts: i64) -> String {
    let Some(t) = ist().timestamp_opt(ts, 0).single() else {
        return String::new();
    };
    let monday = t.date_naive() - Duration::days(t.weekday().num_days_from_monday() as i64);
    monday.format("%Y-%m-%d").to_string()
}

/// Midnight on the 1st of a moment's India month.
pub fn month_start(ts: i64) -> i64 {
    let Some(t) = ist().timestamp_opt(ts, 0).single() else {
        return 0;
    };
    t.date_naive()
        .with_day(1)
        .and_then(|first| first.and_hms_opt(0, 0, 0))
        .and_then(|midnight| midnight.and_local_timezone(ist()).single())
        .map(|m| m.timestamp())
        .unwrap_or(0)
}

/// Writes an entry, applying its source's cap. `ts` comes in rather than being
/// read here, so tests and replays control the clock.
pub fn write(conn: &Connection, entry: &Entry, ts: i64) -> rusqlite::Result<Outcome> {
    if let Some(key) = &entry.dedupe {
        let seen: Option<i64> =
            conn.query_row("SELECT 1 FROM ledger WHERE dedupe = ?1", params![key], |r| r.get(0)).optional()?;
        if seen.is_some() {
            return Ok(Outcome::Duplicate);
        }
    }

    let granted = match entry.user {
        // Deductions and house-only awards never meet a cap.
        Some(user) if entry.points > 0 => {
            let cap = entry.source.cap();
            // Only positive rows fill a cap: a deduction must not buy room back.
            let used: i64 = match cap {
                Cap::PerDay(_) => conn.query_row(
                    "SELECT COALESCE(SUM(points), 0) FROM ledger
                     WHERE user_id = ?1 AND source = ?2 AND day = ?3 AND points > 0",
                    params![user as i64, entry.source.key(), ist_day(ts)],
                    |r| r.get(0),
                )?,
                Cap::PerWeekPerChannel(_) => conn.query_row(
                    "SELECT COALESCE(SUM(points), 0) FROM ledger
                     WHERE user_id = ?1 AND source = ?2 AND scope IS ?3 AND day >= ?4 AND points > 0",
                    params![user as i64, entry.source.key(), entry.scope, week_start_day(ts)],
                    |r| r.get(0),
                )?,
                Cap::None => 0,
            };
            match cap {
                Cap::PerDay(limit) | Cap::PerWeekPerChannel(limit) => entry.points.min(limit - used).max(0),
                Cap::None => entry.points,
            }
        }
        _ => entry.points,
    };

    let capped = granted == 0 && entry.points > 0;
    // A capped event with a dedupe key is still written, as a zero, so replaying
    // it tomorrow - when the cap has reset - can't turn it into points.
    if capped && entry.dedupe.is_none() {
        return Ok(Outcome::Capped);
    }
    conn.execute(
        "INSERT INTO ledger (user_id, house, source, scope, points, reason, awarded_by, day, ts, dedupe)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            entry.user.map(|u| u as i64),
            entry.house.key,
            entry.source.key(),
            entry.scope,
            granted,
            entry.reason,
            entry.by.map(|b| b as i64),
            ist_day(ts),
            ts,
            entry.dedupe
        ],
    )?;
    Ok(if capped { Outcome::Capped } else { Outcome::Granted(granted) })
}

/// Points per house since `since`.
pub fn house_totals(conn: &Connection, since: i64) -> rusqlite::Result<HashMap<&'static str, i64>> {
    let mut out: HashMap<&'static str, i64> = HOUSES.iter().map(|h| (h.key, 0)).collect();
    let mut stmt = conn.prepare("SELECT house, SUM(points) FROM ledger WHERE ts >= ?1 GROUP BY house")?;
    let rows = stmt.query_map(params![since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for (key, sum) in rows.flatten() {
        if let Some(slot) = house(&key).and_then(|h| out.get_mut(h.key)) {
            *slot = sum;
        }
    }
    Ok(out)
}

/// Points per house per source between `since` and `until` - the hourly
/// summary's "where this hour's points came from".
pub fn by_source(conn: &Connection, since: i64, until: i64) -> rusqlite::Result<HashMap<(&'static str, Source), i64>> {
    let mut out = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT house, source, SUM(points) FROM ledger WHERE ts >= ?1 AND ts < ?2 GROUP BY house, source",
    )?;
    let rows = stmt.query_map(params![since, until], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
    })?;
    for (key, source, sum) in rows.flatten() {
        if let (Some(h), Some(s)) = (house(&key), Source::from_key(&source)) {
            if sum != 0 {
                out.insert((h.key, s), sum);
            }
        }
    }
    Ok(out)
}

/// One person's points since `since`, split by source, biggest first.
pub fn breakdown(conn: &Connection, user: u64, since: i64) -> rusqlite::Result<Vec<(Source, i64)>> {
    let mut stmt = conn.prepare(
        "SELECT source, SUM(points) FROM ledger WHERE user_id = ?1 AND ts >= ?2 GROUP BY source",
    )?;
    let rows = stmt.query_map(params![user as i64, since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    let mut out: Vec<(Source, i64)> =
        rows.flatten().filter_map(|(key, sum)| Source::from_key(&key).map(|s| (s, sum))).filter(|(_, n)| *n != 0).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1));
    Ok(out)
}

/// The monthly result.
#[derive(Debug, PartialEq, Eq)]
pub enum DrawResult {
    /// No points at all in the window.
    Empty,
    /// Two or more houses level on top - a call for the mods, not a coin.
    Tie(Vec<&'static str>),
    Winner {
        house: &'static str,
        points: i64,
        captain: Option<u64>,
        /// One member drawn at random, or `None` if nobody qualified.
        member: Option<u64>,
        /// How many were in the draw.
        pool: usize,
    },
}

/// The minimum a member needs in the month to be in the Nitro draw.
pub const DRAW_MINIMUM: i64 = 10;

/// Works out the month's winning house and draws its random Nitro winner.
///
/// `can_win` rules people out (Muggles, anyone who has left). The captain is
/// never also the random winner, so the two Nitros go to two people. `roll` is
/// the random number, passed in so a draw can be reproduced and tested.
pub fn draw(
    conn: &Connection,
    since: i64,
    until: i64,
    captain_of: impl Fn(&House) -> Option<u64>,
    can_win: impl Fn(u64) -> bool,
    roll: u64,
) -> rusqlite::Result<DrawResult> {
    let mut stmt = conn.prepare("SELECT house, SUM(points) FROM ledger WHERE ts >= ?1 AND ts < ?2 GROUP BY house")?;
    let totals: Vec<(String, i64)> =
        stmt.query_map(params![since, until], |r| Ok((r.get(0)?, r.get(1)?)))?.flatten().collect();
    let Some(best) = totals.iter().map(|(_, n)| *n).max().filter(|n| *n > 0) else {
        return Ok(DrawResult::Empty);
    };
    let top: Vec<&'static str> =
        totals.iter().filter(|(_, n)| *n == best).filter_map(|(key, _)| house(key).map(|h| h.key)).collect();
    if top.len() > 1 {
        return Ok(DrawResult::Tie(top));
    }
    let Some(winner) = top.first().and_then(|key| house(key)) else {
        return Ok(DrawResult::Empty);
    };

    let captain = captain_of(winner);
    let mut stmt = conn.prepare(
        "SELECT user_id FROM ledger WHERE house = ?1 AND user_id IS NOT NULL AND ts >= ?2 AND ts < ?3
         GROUP BY user_id HAVING SUM(points) >= ?4 ORDER BY user_id",
    )?;
    let pool: Vec<u64> = stmt
        .query_map(params![winner.key, since, until, DRAW_MINIMUM], |r| r.get::<_, i64>(0))?
        .flatten()
        .map(|id| id as u64)
        .filter(|u| Some(*u) != captain && can_win(*u))
        .collect();
    let member = if pool.is_empty() { None } else { Some(pool[(roll % pool.len() as u64) as usize]) };
    Ok(DrawResult::Winner { house: winner.key, points: best, captain, member, pool: pool.len() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(
            "CREATE TABLE awards (id INTEGER PRIMARY KEY AUTOINCREMENT, house TEXT NOT NULL, points INTEGER NOT NULL,
                                  reason TEXT NOT NULL, awarded_by INTEGER NOT NULL, ts INTEGER NOT NULL);",
        )
        .expect("awards table");
        conn.execute_batch(SCHEMA).expect("ledger schema");
        conn
    }

    /// 2026-09-14 12:00 India time, a Monday.
    const MON: i64 = 1_789_367_400;
    const DAY: i64 = 86_400;

    fn entry(user: u64, source: Source, points: i64) -> Entry<'static> {
        Entry { user: Some(user), house: &HOUSES[0], source, scope: None, points, reason: "", by: None, dedupe: None }
    }

    #[test]
    fn a_daily_cap_stops_at_the_limit_and_resets_the_next_day() {
        let conn = db();
        assert_eq!(write(&conn, &entry(1, Source::Quiz, 3), MON).unwrap(), Outcome::Granted(3));
        assert_eq!(write(&conn, &entry(1, Source::Quiz, 2), MON).unwrap(), Outcome::Granted(2));
        // 5 used of 6: a 3 is trimmed to the 1 that fits, then nothing.
        assert_eq!(write(&conn, &entry(1, Source::Quiz, 3), MON).unwrap(), Outcome::Granted(1));
        assert_eq!(write(&conn, &entry(1, Source::Quiz, 2), MON).unwrap(), Outcome::Capped);
        // Someone else, and a different game, are unaffected.
        assert_eq!(write(&conn, &entry(2, Source::Quiz, 2), MON).unwrap(), Outcome::Granted(2));
        assert_eq!(write(&conn, &entry(1, Source::Cat, 2), MON).unwrap(), Outcome::Granted(2));
        // A new India day, a fresh limit.
        assert_eq!(write(&conn, &entry(1, Source::Quiz, 3), MON + DAY).unwrap(), Outcome::Granted(3));
    }

    #[test]
    fn the_same_event_never_scores_twice_even_after_the_cap_resets() {
        let conn = db();
        let mut koto = entry(1, Source::Koto, 3);
        koto.dedupe = Some("koto:887:1".into());
        assert_eq!(write(&conn, &koto, MON).unwrap(), Outcome::Granted(3));
        assert_eq!(write(&conn, &koto, MON).unwrap(), Outcome::Duplicate);

        // Capped with a key: recorded as a zero, so a replay the next day, when
        // the limit is fresh again, still can't score.
        let mut full = entry(2, Source::Koto, 4);
        full.dedupe = Some("koto:1:2".into());
        write(&conn, &full, MON).unwrap();
        let mut late = entry(2, Source::Koto, 3);
        late.dedupe = Some("koto:2:2".into());
        assert_eq!(write(&conn, &late, MON).unwrap(), Outcome::Capped);
        assert_eq!(write(&conn, &late, MON + DAY).unwrap(), Outcome::Duplicate);
    }

    #[test]
    fn the_weekly_scan_is_capped_per_channel_per_week() {
        let conn = db();
        let weekly = |channel: &str, points| {
            let mut e = entry(1, Source::Weekly, points);
            e.scope = Some(channel.to_string());
            e
        };
        assert_eq!(write(&conn, &weekly("tech", 2), MON).unwrap(), Outcome::Granted(2));
        assert_eq!(write(&conn, &weekly("tech", 2), MON + DAY).unwrap(), Outcome::Granted(1));
        assert_eq!(write(&conn, &weekly("gaming", 3), MON + DAY).unwrap(), Outcome::Granted(3));
        // The next Monday starts a new week.
        assert_eq!(write(&conn, &weekly("tech", 3), MON + 7 * DAY).unwrap(), Outcome::Granted(3));
    }

    #[test]
    fn deductions_are_never_capped_and_never_buy_room_back() {
        let conn = db();
        write(&conn, &entry(1, Source::Quiz, 6), MON).unwrap();
        assert_eq!(write(&conn, &entry(1, Source::Quiz, -3), MON).unwrap(), Outcome::Granted(-3));
        assert_eq!(write(&conn, &entry(1, Source::Quiz, 1), MON).unwrap(), Outcome::Capped);
    }

    #[test]
    fn the_golden_snitch_ignores_the_cap_that_binds_the_others() {
        let conn = db();
        assert_eq!(write(&conn, &entry(1, Source::Snitch, 6), MON).unwrap(), Outcome::Granted(6));
        assert_eq!(write(&conn, &entry(1, Source::Snitch, 2), MON).unwrap(), Outcome::Capped);
        assert_eq!(write(&conn, &entry(1, Source::GoldenSnitch, 6), MON).unwrap(), Outcome::Granted(6));
        assert_eq!(write(&conn, &entry(1, Source::GoldenSnitch, 6), MON).unwrap(), Outcome::Granted(6));
    }

    #[test]
    fn old_house_awards_move_into_the_ledger_exactly_once() {
        let conn = db();
        conn.execute("INSERT INTO awards (house, points, reason, awarded_by, ts) VALUES ('ravenclaw', 25, 'quiz night', 9, ?1)", params![MON])
            .unwrap();
        conn.execute_batch(MIGRATE_AWARDS).unwrap();
        conn.execute_batch(MIGRATE_AWARDS).unwrap();
        let totals = house_totals(&conn, 0).unwrap();
        assert_eq!(totals["ravenclaw"], 25);
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM ledger", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, 1, "a restart must not copy the old awards again");
    }

    #[test]
    fn the_month_starts_at_midnight_on_the_first_india_time() {
        let start = month_start(MON);
        assert_eq!(ist_day(start), "2026-09-01");
        assert_eq!(ist_day(start - 1), "2026-08-31");
        assert_eq!(week_start_day(MON + 3 * DAY), "2026-09-14");
    }

    #[test]
    fn the_draw_picks_the_top_house_and_a_different_person_from_its_captain() {
        let conn = db();
        let g = &HOUSES[0];
        let s = &HOUSES[1];
        let put = |user: u64, h: &'static House, points: i64| {
            let e = Entry { user: Some(user), house: h, source: Source::Mod, scope: None, points, reason: "", by: None, dedupe: None };
            write(&conn, &e, MON).unwrap();
        };
        put(10, g, 30); // the captain
        put(11, g, 12);
        put(12, g, 10);
        put(13, g, 9); // under the minimum
        put(14, g, 40); // a Muggle now
        put(20, s, 50);
        let result = draw(&conn, 0, MON + DAY, |_| Some(10), |u| u != 14, 1).unwrap();
        match result {
            DrawResult::Winner { house, captain, member, pool, .. } => {
                assert_eq!(house, g.key);
                assert_eq!(captain, Some(10));
                assert_eq!(pool, 2, "only 11 and 12 qualify: not the captain, not under 10, not the Muggle");
                assert!(matches!(member, Some(11) | Some(12)));
            }
            other => panic!("expected a winner, got {:?}", other),
        }
    }

    #[test]
    fn a_tie_at_the_top_is_left_to_the_mods() {
        let conn = db();
        for (user, h) in [(1, &HOUSES[0]), (2, &HOUSES[1])] {
            let e = Entry { user: Some(user), house: h, source: Source::Mod, scope: None, points: 20, reason: "", by: None, dedupe: None };
            write(&conn, &e, MON).unwrap();
        }
        assert!(matches!(draw(&conn, 0, MON + DAY, |_| None, |_| true, 0).unwrap(), DrawResult::Tie(t) if t.len() == 2));
        assert_eq!(draw(&conn, MON + DAY, MON + 2 * DAY, |_| None, |_| true, 0).unwrap(), DrawResult::Empty);
    }
}
