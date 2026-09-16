//! Cards earned by playing: every morning the day before's top scorer in each
//! activity gets a random Common or Uncommon card, and a battle royale's
//! champion and runner-up get one each. An earned card is a numbered copy like
//! any caught one, but it pays no house points: points only come from catching
//! a frog in chat.
//!
//! Every card is given under a key that can only be used once (`frogtop:<day>:
//! <activity>`, `frogroyale:<battle>:<role>`), so a restart or a second run
//! hands back the same card instead of a new one.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use serenity::all::{ChannelId, Context, CreateAllowedMentions, CreateEmbed, CreateEmbedFooter, CreateMessage};

use super::control;
use super::frog_store::{self as store, Award, AwardFor};
use super::points::Source;


/// India is UTC+5:30 all year.
const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;
const DAY: i64 = 86_400;
const TICK: Duration = Duration::from_secs(60);

/// The activities a day's top is found in, and the ledger sources each counts.
/// Mods' points, the weekly scan and battle royales are left out; so are
/// earned-card points themselves, which would otherwise feed the next day.
pub const ACTIVITIES: &[(&str, &[&str])] = &[
    ("chat", &["chat"]),
    ("voice", &["voice"]),
    ("quiz", &["quiz"]),
    ("koto", &["koto"]),
    ("anagram", &["anagram"]),
    ("cat", &["cat"]),
    ("wordle", &["wordle"]),
    ("arena", &["arena"]),
    ("snitch", &["snitch", "golden_snitch"]),
    ("frog", &["frog"]),
    ("npat", &["npat"]),
    ("sudoku", &["sudoku"]),
];

pub fn activity_label(key: &str) -> String {
    match key {
        "snitch" => "🪽 Snitch".to_string(),
        "frog" => "🐸 Frogs".to_string(),
        "npat" => "🔤 Name Place Animal Thing".to_string(),
        "sudoku" => "🔢 Sudoku".to_string(),
        other => Source::from_key(other).map(|s| s.label().to_string()).unwrap_or_else(|| other.to_string()),
    }
}

// --- settings -----------------------------------------------------------------------

fn frogs_on() -> bool {
    control::on("VIZIER_FROGS", false)
}

fn daily_top_on() -> bool {
    frogs_on() && control::on("VIZIER_FROG_DAILY_TOP", true)
}

/// Minutes into the India day the daily top cards go out.
fn daily_top_minutes() -> i64 {
    let raw = control::var("VIZIER_FROG_DAILY_TOP_TIME").unwrap_or_else(|| "10:00".into());
    parse_clock(&raw).unwrap_or(600)
}

fn parse_clock(raw: &str) -> Option<i64> {
    let (h, m) = raw.trim().split_once(':')?;
    let (h, m): (i64, i64) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

fn summary_channel() -> Option<ChannelId> {
    control::id("VIZIER_FROG_DAILY_TOP_CHANNEL").or_else(|| control::id("VIZIER_HOUSE_CHANNEL")).filter(|id| *id != 0).map(ChannelId::new)
}

fn royale_cards_on() -> bool {
    frogs_on() && control::on("VIZIER_FROG_ROYALE_CARDS", true)
}

fn royale_min_players() -> usize {
    control::number("VIZIER_FROG_ROYALE_MIN_PLAYERS", 6) as usize
}

// --- the day's tops -------------------------------------------------------------------

/// One ledger row, as the day's tops read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LedgerRow {
    pub user: u64,
    pub source: String,
    pub points: i64,
    pub ts: i64,
    pub dedupe: Option<String>,
}

/// Which activity a row counts for, if any. A frog row only counts when it is a
/// catch: earned cards and the collection bonus are not "playing frogs".
fn activity_of(row: &LedgerRow) -> Option<&'static str> {
    if row.source == "frog" && !row.dedupe.as_deref().is_some_and(|d| d.starts_with("frog:")) {
        return None;
    }
    ACTIVITIES.iter().find(|(_, sources)| sources.contains(&row.source.as_str())).map(|(key, _)| *key)
}

/// Activities whose top is the most wins or catches rather than the most
/// points: their daily limits leave many people level on points, and a capped
/// win is still in the ledger as a zero.
const COUNTED: &[&str] = &["quiz", "koto", "anagram", "cat", "arena", "snitch", "sudoku"];

/// The top of each activity from a day's rows (in ledger order). Wordle and
/// frogs: the most points. Quiz, Koto, Anagram, Cat Bot, fights, the Snitch and
/// sudoku: the most wins, catches or puzzles solved, capped ones included, then
/// the most points. Chat
/// and voice here are by points; `run_daily_top` swaps in messages and voice
/// time. Ties go to whoever got there first.
pub fn daily_tops(rows: &[LedgerRow]) -> Vec<(&'static str, u64, i64)> {
    let mut out = Vec::new();
    for (activity, _) in ACTIVITIES {
        let counted = COUNTED.contains(activity);
        let mine: Vec<&LedgerRow> = rows.iter().filter(|r| activity_of(r) == Some(activity)).collect();
        // (user, wins, points)
        let mut totals: Vec<(u64, i64, i64)> = Vec::new();
        for row in &mine {
            let win = if row.points < 0 { -1 } else { 1 };
            match totals.iter_mut().find(|(u, _, _)| *u == row.user) {
                Some((_, wins, points)) => {
                    *wins += win;
                    *points += row.points;
                }
                None => totals.push((row.user, win, row.points)),
            }
        }
        let best = totals
            .iter()
            .filter(|(_, wins, points)| if counted { *wins > 0 } else { *points > 0 })
            .map(|(u, wins, points)| {
                let target = if counted { *wins } else { *points };
                // When their running total first reached what they ended on.
                let mut running = 0;
                let reached = mine
                    .iter()
                    .filter(|r| r.user == *u)
                    .find_map(|r| {
                        running += if counted { if r.points < 0 { -1 } else { 1 } } else { r.points };
                        (running >= target).then_some(r.ts)
                    })
                    .unwrap_or(i64::MAX);
                (*u, target, *points, reached)
            });
        if let Some((user, total, _, _)) =
            best.min_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)).then(a.0.cmp(&b.0)))
        {
            out.push((*activity, user, total));
        }
    }
    out
}

/// Chat's and voice's tops are who sent the most messages / spent the most
/// voice time that day, not the most points: the daily limit leaves many people
/// level. Only people the ledger paid for that activity that day can win (so
/// opt-outs, mods and the unsorted can't); a tie goes to whoever was paid first.
pub fn measured_top(rows: &[LedgerRow], source: &str, measure: &HashMap<u64, i64>) -> Option<(u64, i64)> {
    let mut eligible: Vec<(u64, i64)> = Vec::new();
    for row in rows.iter().filter(|r| r.source == source && r.points > 0) {
        if !eligible.iter().any(|(u, _)| *u == row.user) {
            eligible.push((row.user, row.ts));
        }
    }
    eligible
        .iter()
        .filter_map(|(u, first)| measure.get(u).copied().filter(|n| *n > 0).map(|n| (*u, n, *first)))
        .min_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)))
        .map(|(u, n, _)| (u, n))
}

/// Name Place Animal Thing's top: the most game wins, then the most 2nd
/// places, then whoever got there first (their last placing earliest). `places`
/// is every 1st and 2nd of the day as (user, place, when); only people the
/// ledger paid for the game that day can win (so opt-outs and the unsorted
/// can't). Returns the winner and their wins.
pub fn npat_top(rows: &[LedgerRow], places: &[(u64, u8, i64)]) -> Option<(u64, i64)> {
    let paid = |u: u64| rows.iter().any(|r| r.user == u && r.source == "npat");
    // (user, wins, seconds, reached)
    let mut tally: Vec<(u64, i64, i64, i64)> = Vec::new();
    for (user, place, at) in places.iter().filter(|(u, p, _)| matches!(p, 1 | 2) && paid(*u)) {
        let (first, second) = if *place == 1 { (1, 0) } else { (0, 1) };
        match tally.iter_mut().find(|(u, _, _, _)| u == user) {
            Some(t) => {
                t.1 += first;
                t.2 += second;
                t.3 = t.3.max(*at);
            }
            None => tally.push((*user, first, second, *at)),
        }
    }
    tally
        .into_iter()
        .min_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.3.cmp(&b.3)).then(a.0.cmp(&b.0)))
        .map(|(user, wins, _, _)| (user, wins))
}

/// Swaps Name Place Animal Thing's points-based top for game wins.
fn with_npat_measured(mut tops: Vec<(&'static str, u64, i64)>, rows: &[LedgerRow], day: &str) -> Vec<(&'static str, u64, i64)> {
    let Some(db) = super::npat_store::db() else {
        return tops;
    };
    let places = super::npat_store::places_on(&db.lock(), day);
    tops.retain(|(a, _, _)| *a != "npat");
    if let Some((user, wins)) = npat_top(rows, &places) {
        tops.push(("npat", user, wins));
    }
    tops
}

/// India midnight at the start of `day` (YYYY-MM-DD).
fn day_start(day: &str) -> Option<i64> {
    let date = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp() - IST_OFFSET)
}

/// Swaps chat's and voice's points-based tops for messages and voice time.
fn with_chat_and_voice_measured(mut tops: Vec<(&'static str, u64, i64)>, rows: &[LedgerRow], day: &str) -> Vec<(&'static str, u64, i64)> {
    let Some(db) = super::stats::db() else {
        return tops;
    };
    let (messages, voice) = {
        let conn = db.lock();
        let messages = super::activity::messages_on(&conn, day, None).unwrap_or_default();
        let voice = match day_start(day) {
            Some(start) => {
                super::activity::voice_points_between(&conn, None, start, start + DAY, Utc::now().timestamp().min(start + DAY)).unwrap_or_default()
            }
            None => HashMap::new(),
        };
        (messages, voice)
    };
    for (activity, measure) in [("chat", &messages), ("voice", &voice)] {
        if let Some((user, amount)) = measured_top(rows, activity, measure) {
            match tops.iter_mut().find(|(a, _, _)| *a == activity) {
                Some(top) => *top = (activity, user, amount),
                None => tops.push((activity, user, amount)),
            }
        }
    }
    tops
}

/// Puts tops back in the activities' order, so card numbers follow the list.
fn order_like_activities(mut tops: Vec<(&'static str, u64, i64)>) -> Vec<(&'static str, u64, i64)> {
    tops.sort_by_key(|(a, _, _)| ACTIVITIES.iter().position(|(k, _)| k == a).unwrap_or(usize::MAX));
    tops
}

/// The India day whose tops should go out at `now`, if they haven't: yesterday,
/// once today's time has come.
pub fn due_day(now: i64, at_minutes: i64, done: Option<&str>) -> Option<String> {
    let midnight = (now + IST_OFFSET).div_euclid(DAY) * DAY - IST_OFFSET;
    if now < midnight + at_minutes * 60 {
        return None;
    }
    let yesterday = super::points::ist_day(midnight - 1);
    (done != Some(yesterday.as_str())).then_some(yesterday)
}

/// One line of the morning summary.
pub fn top_line(activity: &str, user: u64, crest: Option<&str>, award: &Award) -> String {
    let crest = crest.map(|c| format!(" ({})", c)).unwrap_or_default();
    format!(
        "{} — <@{}>{} · {} {}",
        activity_label(activity),
        user,
        crest,
        award.card.rarity.emoji(),
        store::card_label(&award.card.wizard_name, award.card.edition, award.card.serial)
    )
}

fn day_rows(day: &str) -> Vec<LedgerRow> {
    let Some(db) = super::house::db() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT user_id, source, points, ts, dedupe FROM ledger WHERE day = ?1 AND user_id IS NOT NULL ORDER BY ts, id") else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![day], |r| {
        Ok(LedgerRow { user: r.get::<_, i64>(0)? as u64, source: r.get(1)?, points: r.get(2)?, ts: r.get(3)?, dedupe: r.get(4)? })
    })
    .map(|rows| rows.flatten().collect())
    .unwrap_or_default()
}

/// Gives an earned card; the same key twice gives it once. No house points.
fn give(key: &str, user: u64, origin: &str, what: &AwardFor) -> Option<Award> {
    let db = store::db()?;
    let mut conn = db.lock();
    match store::award_card(&mut conn, key, user, origin, what, (rand::random::<f64>(), rand::random::<f64>()), Utc::now().timestamp()) {
        Ok(award) => award,
        Err(err) => {
            tracing::warn!("frog: card {} for {} not given: {}", key, user, err);
            None
        }
    }
}

/// Hands out a day's top cards and posts the summary.
pub async fn run_daily_top(ctx: &Context, day: &str) {
    let rows = day_rows(day);
    let tops = order_like_activities(with_npat_measured(with_chat_and_voice_measured(daily_tops(&rows), &rows, day), &rows, day));
    let mut lines = Vec::new();
    for (activity, user, total) in &tops {
        let what = AwardFor { kind: "daily_top", day: day.to_string(), activity: activity.to_string(), total: *total, ..Default::default() };
        let key = format!("frogtop:{}:{}", day, activity);
        match give(&key, *user, &format!("daily_top:{}", activity), &what) {
            Some(award) => {
                let crest = super::house::house_of(*user).map(|h| h.crest);
                lines.push(top_line(activity, *user, crest, &award));
            }
            None => tracing::warn!("frog: no card for {}'s top in {} ({})", day, activity, user),
        }
    }
    tracing::info!("frog: {} top-of-the-day cards for {}", lines.len(), day);
    let posted_key = format!("daily_top_posted:{}", day);
    let already = store::db().and_then(|db| store::meta_get(&db.lock(), &posted_key)).is_some();
    if let (false, false, Some(channel)) = (lines.is_empty(), already, summary_channel()) {
        let embed = CreateEmbed::new()
            .title("🐸 Yesterday's top frogs")
            .description(lines.join("\n"))
            .colour(0xC68E54)
            .footer(CreateEmbedFooter::new("Most messages, most VC time, most wins in each game, most Wordle and frog points, most Name Place Animal Thing game wins: each wins a card"));
        let message = CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new());
        match tokio::time::timeout(Duration::from_secs(20), channel.send_message(&ctx.http, message)).await {
            Ok(Ok(_)) => {
                if let Some(db) = store::db() {
                    let _ = store::meta_set(&db.lock(), &posted_key, "1");
                }
            }
            Ok(Err(err)) => tracing::warn!("frog: top-of-the-day summary not posted: {}", err),
            Err(_) => tracing::warn!("frog: top-of-the-day summary timed out"),
        }
    }
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), "daily_top_done", day);
    }
}

/// Runs yesterday's top cards at the set time, and catches up after a restart.
pub fn spawn_daily(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            if !daily_top_on() {
                continue;
            }
            let done = store::db().and_then(|db| store::meta_get(&db.lock(), "daily_top_done"));
            if let Some(day) = due_day(Utc::now().timestamp(), daily_top_minutes(), done.as_deref()) {
                run_daily_top(&ctx, &day).await;
            }
        }
    });
}

// --- battle royales -------------------------------------------------------------------

/// Whether a royale was big enough to hand out cards.
pub fn royale_eligible(entrants: usize, min_players: usize) -> bool {
    entrants >= min_players.max(2)
}

/// Cards for a finished royale's champion and runner-up, as lines for the
/// champion message. Nothing when switched off or the royale was too small.
pub fn royale_cards(battle_id: i64, champion: u64, runner_up: Option<u64>, entrants: usize) -> Vec<String> {
    if !royale_cards_on() || !royale_eligible(entrants, royale_min_players()) {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for (role, user) in [("champion", Some(champion)), ("runner_up", runner_up)] {
        let Some(user) = user else { continue };
        let what = AwardFor { kind: "royale", battle_id: Some(battle_id), role: role.to_string(), ..Default::default() };
        let key = format!("frogroyale:{}:{}", battle_id, role);
        if let Some(award) = give(&key, user, &format!("royale_{}", role), &what) {
            let card = format!("{} {}", award.card.rarity.emoji(), store::card_label(&award.card.wizard_name, award.card.edition, award.card.serial));
            lines.push(if role == "champion" { format!("🐸 Won a card: {}", card) } else { format!("🐸 Runner-up <@{}> won a card: {}", user, card) });
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-14 00:00 India time.
    const MIDNIGHT: i64 = 1_789_324_200;

    fn row(user: u64, source: &str, points: i64, ts: i64) -> LedgerRow {
        LedgerRow { user, source: source.into(), points, ts, dedupe: None }
    }

    #[test]
    fn games_go_to_the_most_wins_capped_ones_included_and_ties_to_the_first_there() {
        let rows = vec![
            row(1, "quiz", 3, 100),
            row(2, "quiz", 2, 110),
            row(2, "quiz", 1, 120),
            row(3, "quiz", 0, 130),
            row(3, "quiz", 0, 131),
            row(1, "chat", 1, 140),
            row(9, "voice", 0, 150),
            row(4, "snitch", 2, 160),
            row(5, "golden_snitch", 6, 170),
            row(4, "golden_snitch", 4, 180),
            row(6, "mod", 50, 190),
            row(6, "weekly", 3, 191),
            row(6, "royale", 8, 192),
            row(7, "wordle", 4, 193),
            row(8, "wordle", 5, 194),
        ];
        let tops = daily_tops(&rows);
        // Quiz: 2 and 3 both have two wins (3's were capped at zero); 2 has more points.
        // Snitch counts both kinds: 4 caught twice, 5 once.
        // Wordle is by points.
        assert_eq!(tops, vec![("chat", 1, 1), ("quiz", 2, 2), ("wordle", 8, 5), ("snitch", 4, 2)]);
        // Equal wins and points go to whoever got there first.
        let later = vec![row(1, "koto", 1, 100), row(2, "koto", 1, 110), row(2, "koto", 1, 115), row(1, "koto", 1, 120)];
        assert_eq!(daily_tops(&later), vec![("koto", 2, 2)]);
    }

    #[test]
    fn chat_and_voice_go_to_the_most_messages_or_time_among_people_paid_for_them() {
        let rows = vec![row(1, "chat", 1, 100), row(2, "chat", 1, 100), row(3, "chat", 1, 90), row(4, "quiz", 3, 50), row(6, "voice", 1, 10)];
        let messages: HashMap<u64, i64> = [(1, 40), (2, 760), (3, 760), (4, 9000), (5, 5000)].into_iter().collect();
        // 4 wasn't paid for chat and 5 isn't in the ledger at all; 2 and 3 tie and 3 was paid first.
        assert_eq!(measured_top(&rows, "chat", &messages), Some((3, 760)));
        assert_eq!(measured_top(&rows, "chat", &HashMap::new()), None);
        assert_eq!(measured_top(&[row(1, "chat", 0, 1)], "chat", &messages), None, "a capped zero row isn't a payout");
        let voice: HashMap<u64, i64> = [(6, 7200), (1, 90_000)].into_iter().collect();
        assert_eq!(measured_top(&rows, "voice", &voice), Some((6, 7200)));
        assert_eq!(day_start("2026-09-14"), Some(MIDNIGHT));
        let shuffled = vec![("snitch", 1, 1), ("chat", 2, 5), ("voice", 3, 9)];
        assert_eq!(order_like_activities(shuffled), vec![("chat", 2, 5), ("voice", 3, 9), ("snitch", 1, 1)]);
    }

    #[test]
    fn mods_weekly_royale_and_earned_cards_never_make_a_top() {
        let mut rows = vec![row(6, "mod", 50, 1), row(6, "weekly", 3, 2), row(6, "royale", 8, 3)];
        rows.push(LedgerRow { dedupe: Some("frogtop:2026-09-13:quiz".into()), ..row(7, "frog", 4, 4) });
        rows.push(LedgerRow { dedupe: Some("frogroyale:1:champion".into()), ..row(7, "frog", 4, 5) });
        rows.push(LedgerRow { dedupe: Some("frogset:7".into()), ..row(7, "frog", 15, 6) });
        assert!(daily_tops(&rows).is_empty());
        rows.push(LedgerRow { dedupe: Some("frog:12".into()), ..row(8, "frog", 2, 7) });
        assert_eq!(daily_tops(&rows), vec![("frog", 8, 2)]);
        // Deductions can take someone out of the running.
        assert!(daily_tops(&[row(1, "cat", 2, 1), row(1, "cat", -2, 2)]).is_empty());
    }

    #[test]
    fn the_morning_run_is_due_once_for_yesterday() {
        let ten = 600;
        assert_eq!(due_day(MIDNIGHT + 9 * 3600, ten, None), None, "not before 10:00");
        assert_eq!(due_day(MIDNIGHT + 10 * 3600, ten, None).as_deref(), Some("2026-09-13"));
        assert_eq!(due_day(MIDNIGHT + 23 * 3600, ten, Some("2026-09-13")), None, "done already");
        assert_eq!(due_day(MIDNIGHT + 23 * 3600, ten, Some("2026-09-12")).as_deref(), Some("2026-09-13"), "catches up after a restart");
        assert_eq!(parse_clock("10:00"), Some(600));
        assert_eq!(parse_clock(" 7:05 "), Some(425));
        assert_eq!(parse_clock("24:00"), None);
        assert_eq!(parse_clock("ten"), None);
    }

    #[test]
    fn royales_need_enough_fighters() {
        assert!(!royale_eligible(2, 6));
        assert!(!royale_eligible(5, 6));
        assert!(royale_eligible(6, 6));
        assert!(royale_eligible(2, 0), "a minimum below two is two");
        assert!(!royale_eligible(1, 0));
    }

    #[test]
    fn earned_cards_are_common_or_uncommon_and_a_key_pays_once() {
        let mut conn = store::tests::memory();
        let what = AwardFor { kind: "daily_top", day: "2026-09-13".into(), activity: "quiz".into(), total: 9, ..Default::default() };
        let mut seen = std::collections::HashSet::new();
        for i in 0..60 {
            let roll = (i as f64 / 60.0, (i * 7 % 60) as f64 / 60.0);
            let award = store::award_card(&mut conn, &format!("frogtop:d{}:quiz", i), 5, "daily_top:quiz", &what, roll, MIDNIGHT + i).unwrap().unwrap();
            assert!(award.fresh);
            assert_ne!(award.card.rarity, store::Rarity::Legendary, "never the phoenix");
            assert_eq!(award.points, 0, "earned cards pay no house points");
            assert_eq!((award.card.origin.as_str(), award.card.drop_id), ("daily_top:quiz", None));
            seen.insert(award.card.rarity);
        }
        assert_eq!(seen.len(), 2, "both Common and Uncommon come up");
        let first = store::award_card(&mut conn, "frogtop:2026-09-13:chat", 5, "daily_top:chat", &what, (0.1, 0.1), MIDNIGHT).unwrap().unwrap();
        let again = store::award_card(&mut conn, "frogtop:2026-09-13:chat", 5, "daily_top:chat", &what, (0.9, 0.9), MIDNIGHT + 99).unwrap().unwrap();
        assert!(first.fresh && !again.fresh);
        assert_eq!(first.card.serial, again.card.serial, "a rerun hands back the same card");
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 61);
        let logged = store::awards(&conn, "daily_top", 100);
        assert_eq!(logged.len(), 61);
        let chat = logged.iter().find(|a| a.key == "frogtop:2026-09-13:chat").unwrap();
        assert_eq!((chat.activity.as_str(), chat.total, chat.user_id.as_str(), chat.points), ("quiz", 9, "5", 0));
        let line = top_line("snitch", 5, Some("🦁"), &first);
        assert_eq!(line, format!("🪽 Snitch — <@5> (🦁) · {} {}", first.card.rarity.emoji(), store::card_label(&first.card.wizard_name, first.card.edition, first.card.serial)));
        assert!(!line.contains('+'), "no points shown");
        // With no Common or Uncommon card in play there's nothing to give.
        conn.execute("UPDATE wizards SET enabled = 0 WHERE rarity != 'legendary'", []).unwrap();
        assert!(store::award_card(&mut conn, "frogtop:x:quiz", 5, "daily_top:quiz", &what, (0.5, 0.5), MIDNIGHT).unwrap().is_none());
    }

    #[test]
    fn earned_cards_never_reach_the_ledger() {
        // The owner's rule: house points only come from catching frogs. Nothing in
        // this module may write to the ledger.
        let source = include_str!("frog_rewards.rs");
        let (code, _) = source.split_once("#[cfg(test)]").unwrap();
        for writer in [concat!("award", "_person"), concat!("points::", "write"), concat!("INSERT INTO ", "ledger")] {
            assert!(!code.contains(writer), "frog_rewards writes to the ledger via {writer}");
        }
    }

    #[test]
    fn name_place_animal_thing_goes_to_the_most_game_wins() {
        let rows = vec![row(1, "npat", 2, 10), row(2, "npat", 1, 10), row(3, "npat", 2, 20), row(2, "npat", 2, 30), row(4, "quiz", 1, 5)];
        // 1: one win · 2: one win, one 2nd · 3: one win · 4 never paid for the game · 9 never paid at all.
        let places = vec![(1, 1, 100), (2, 2, 100), (3, 1, 200), (4, 2, 200), (2, 1, 300), (9, 1, 400), (9, 1, 500)];
        assert_eq!(npat_top(&rows, &places), Some((2, 1)), "equal wins: more 2nd places");
        let no_seconds: Vec<(u64, u8, i64)> = places.iter().copied().filter(|p| p.1 == 1).collect();
        assert_eq!(npat_top(&rows, &no_seconds), Some((1, 1)), "all level: whoever got there first");
        assert_eq!(npat_top(&rows, &[]), None);
        assert_eq!(npat_top(&[], &places), None, "nobody paid for the game");
        // The ledger's own points don't decide it: daily_tops' points-based pick is swapped out.
        assert_eq!(daily_tops(&[row(5, "npat", 6, 1)]), vec![("npat", 5, 6)]);
        assert_eq!(activity_label("npat"), "🔤 Name Place Animal Thing");
    }

    #[test]
    fn activity_names_read_well() {
        assert_eq!(activity_label("chat"), "💬 Chat");
        assert_eq!(activity_label("snitch"), "🪽 Snitch");
        assert_eq!(activity_label("frog"), "🐸 Frogs");
        assert_eq!(activity_label("wordle"), "🟩 Wordle");
    }
}
