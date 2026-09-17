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
    ("guess", &["guess"]),
    ("cat", &["cat"]),
    ("wordle", &["wordle"]),
    ("arena", &["arena"]),
    ("snitch", &["snitch", "golden_snitch"]),
    ("frog", &["frog"]),
    ("npat", &["npat"]),
    ("sudoku", &["sudoku"]),
    ("chess", &["chess"]),
];

pub fn activity_label(key: &str) -> String {
    match key {
        "snitch" => "🪽 Snitch".to_string(),
        "frog" => "🐸 Frogs".to_string(),
        "npat" => "🔤 Name Place Animal Thing".to_string(),
        "sudoku" => "🔢 Sudoku".to_string(),
        "guess" => "🎨 Guess the Word".to_string(),
        "chess" => "♟️ Chess".to_string(),
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
///
/// Anagrams, Guess the Word and sudoku are here only as the fallback for a day
/// their own store knows nothing about: `with_anagram_measured`,
/// `with_guess_measured` and `with_sudoku_measured` decide a real day on each
/// game's own uncapped points instead.
const COUNTED: &[&str] = &["quiz", "koto", "anagram", "guess", "cat", "arena", "snitch", "sudoku"];

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

/// Chess's top: the most CHESS points that day — every finished game at what it
/// was worth, with no limit over it. Level on points, the most games; level on
/// both, whoever got there first.
///
/// Eligibility is unchanged. Chess pays no house points any more, but it still
/// writes its zero row through the same door, and only someone with a row can
/// win the card — so mods, Muggles and the unsorted are tallied, shown their
/// chess points, and stay out of the running, exactly as before. `tally` is the
/// chess store's day.
pub fn chess_top(rows: &[LedgerRow], tally: &[super::chess_store::Tally]) -> Option<(u64, i64)> {
    let paid = |u: u64| rows.iter().any(|r| r.user == u && r.source == "chess");
    let mut mine: Vec<super::chess_store::Tally> =
        tally.iter().filter(|t| t.games > 0 && t.points > 0 && paid(t.user)).cloned().collect();
    super::chess_store::rank(&mut mine);
    mine.first().map(|t| (t.user, t.points))
}

/// Anagrams' top: the most ANAGRAM points that day — every solve at its full
/// value, so ten four-letter words no longer beat five eight-letter ones and a
/// day spent past the house-points cap still counts. Level on points, the most
/// rounds won; level on both, whoever got there first.
///
/// Eligibility is unchanged: only people the ledger paid for anagrams that day
/// can win the card, which keeps mods, Muggles and the unsorted out of it. Their
/// anagram points are still kept and still shown to them — they are simply not
/// in the running for a card. `tally` is the store's day.
pub fn anagram_top(rows: &[LedgerRow], tally: &[super::anagram_store::Tally]) -> Option<(u64, i64)> {
    let paid = |u: u64| rows.iter().any(|r| r.user == u && r.source == "anagram");
    let mut mine: Vec<super::anagram_store::Tally> = tally.iter().filter(|t| t.solves > 0 && paid(t.user)).cloned().collect();
    super::anagram_store::rank(&mut mine);
    mine.first().map(|t| (t.user, t.points))
}

/// Swaps anagrams' count-of-rows top for the store's uncapped anagram points. A
/// day the store knows nothing about — one before this was built, or a run with
/// no store open — keeps the ledger's answer, so old days still resolve.
fn with_anagram_measured(tops: Vec<(&'static str, u64, i64)>, rows: &[LedgerRow], day: &str) -> Vec<(&'static str, u64, i64)> {
    let Some(db) = super::anagram_store::db() else {
        return tops;
    };
    let tally = super::anagram_store::day_tally(&db.lock(), day);
    with_anagram_tally(tops, rows, &tally)
}

/// The swap itself, with the day's tally already read. An EMPTY tally means the
/// store has nothing for that day, and the ledger's answer is left exactly as it
/// was.
pub fn with_anagram_tally(
    mut tops: Vec<(&'static str, u64, i64)>,
    rows: &[LedgerRow],
    tally: &[super::anagram_store::Tally],
) -> Vec<(&'static str, u64, i64)> {
    if tally.is_empty() {
        return tops;
    }
    tops.retain(|(a, _, _)| *a != "anagram");
    if let Some((user, points)) = anagram_top(rows, tally) {
        tops.push(("anagram", user, points));
    }
    tops
}

/// Guess the Word's top: the most GUESS points that day — every solve at its
/// full value, so ten cheap rounds no longer beat five dear ones and a day spent
/// past the house-points cap still counts. Level on points, the most rounds won;
/// level on both, whoever got there first.
///
/// Eligibility is unchanged: only people the ledger paid for guessing that day
/// can win the card, which keeps mods, Muggles and the unsorted out of it. Their
/// guess points are still kept and still shown to them — they are simply not in
/// the running for a card. `tally` is the store's day.
pub fn guess_top(rows: &[LedgerRow], tally: &[super::guess_store::Tally]) -> Option<(u64, i64)> {
    let paid = |u: u64| rows.iter().any(|r| r.user == u && r.source == "guess");
    let mut mine: Vec<super::guess_store::Tally> = tally.iter().filter(|t| t.solves > 0 && paid(t.user)).cloned().collect();
    super::guess_store::rank(&mut mine);
    mine.first().map(|t| (t.user, t.points))
}

/// Swaps Guess the Word's count-of-rows top for the store's uncapped guess
/// points. A day the store knows nothing about — one before this was built, or a
/// run with no store open — keeps the ledger's answer, so old days still
/// resolve.
fn with_guess_measured(tops: Vec<(&'static str, u64, i64)>, rows: &[LedgerRow], day: &str) -> Vec<(&'static str, u64, i64)> {
    let Some(db) = super::guess_store::db() else {
        return tops;
    };
    let tally = super::guess_store::day_tally(&db.lock(), day);
    with_guess_tally(tops, rows, &tally)
}

/// The swap itself, with the day's tally already read. An EMPTY tally means the
/// store has nothing for that day, and the ledger's answer is left exactly as it
/// was.
pub fn with_guess_tally(
    mut tops: Vec<(&'static str, u64, i64)>,
    rows: &[LedgerRow],
    tally: &[super::guess_store::Tally],
) -> Vec<(&'static str, u64, i64)> {
    if tally.is_empty() {
        return tops;
    }
    tops.retain(|(a, _, _)| *a != "guess");
    if let Some((user, points)) = guess_top(rows, tally) {
        tops.push(("guess", user, points));
    }
    tops
}

/// Sudoku's top: the most SUDOKU points that day — every puzzle won at what it
/// was worth to that solver, hints taken off and nothing capped, so three easy
/// puzzles no longer beat two hard ones. Level on points, the most puzzles won;
/// level on both, whoever got there first.
///
/// Eligibility is unchanged. Sudoku pays no house points any more, but it still
/// writes its zero row through the same door, and only someone with a row can
/// win the card — so mods, Muggles and the unsorted are tallied, shown their
/// sudoku points, and stay out of the running, exactly as before. `tally` is the
/// sudoku store's day.
pub fn sudoku_top(rows: &[LedgerRow], tally: &[super::sudoku_store::Tally]) -> Option<(u64, i64)> {
    let paid = |u: u64| rows.iter().any(|r| r.user == u && r.source == "sudoku");
    let mut mine: Vec<super::sudoku_store::Tally> = tally.iter().filter(|t| t.solves > 0 && paid(t.user)).cloned().collect();
    super::sudoku_store::rank(&mut mine);
    mine.first().map(|t| (t.user, t.points))
}

/// Swaps sudoku's count-of-rows top for the store's uncapped sudoku points. A
/// day the store knows nothing about — one before this was built, or a run with
/// no store open — keeps the ledger's answer, so old days still resolve and the
/// `frogtop:<day>:sudoku` key is unchanged.
fn with_sudoku_measured(tops: Vec<(&'static str, u64, i64)>, rows: &[LedgerRow], day: &str) -> Vec<(&'static str, u64, i64)> {
    let Some(db) = super::sudoku_store::db() else {
        return tops;
    };
    let tally = super::sudoku_store::day_tally(&db.lock(), day);
    with_sudoku_tally(tops, rows, &tally)
}

/// The swap itself, with the day's tally already read. An EMPTY tally means the
/// store has nothing for that day, and the ledger's answer is left exactly as it
/// was.
pub fn with_sudoku_tally(
    mut tops: Vec<(&'static str, u64, i64)>,
    rows: &[LedgerRow],
    tally: &[super::sudoku_store::Tally],
) -> Vec<(&'static str, u64, i64)> {
    if tally.is_empty() {
        return tops;
    }
    tops.retain(|(a, _, _)| *a != "sudoku");
    if let Some((user, points)) = sudoku_top(rows, tally) {
        tops.push(("sudoku", user, points));
    }
    tops
}

/// Swaps chess's count-of-rows top for the store's uncapped chess points. A day
/// the store knows nothing about — one before chess kept a score of its own, or
/// a run with no store open — keeps the ledger's answer, so old days still
/// resolve.
fn with_chess_measured(tops: Vec<(&'static str, u64, i64)>, rows: &[LedgerRow], day: &str) -> Vec<(&'static str, u64, i64)> {
    let (Some(db), Some(start)) = (super::chess_store::db(), day_start(day)) else {
        return tops;
    };
    let tally = super::chess_store::tally_between(&db.lock(), start, start + DAY);
    with_chess_tally(tops, rows, &tally)
}

/// The swap itself, with the day's tally already read. An EMPTY tally means the
/// store has nothing for that day, and the ledger's answer is left exactly as it
/// was.
pub fn with_chess_tally(
    mut tops: Vec<(&'static str, u64, i64)>,
    rows: &[LedgerRow],
    tally: &[super::chess_store::Tally],
) -> Vec<(&'static str, u64, i64)> {
    if tally.is_empty() {
        return tops;
    }
    tops.retain(|(a, _, _)| *a != "chess");
    if let Some((user, points)) = chess_top(rows, tally) {
        tops.push(("chess", user, points));
    }
    tops
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
    let tops = daily_tops(&rows);
    let tops = with_chat_and_voice_measured(tops, &rows, day);
    let tops = with_npat_measured(tops, &rows, day);
    let tops = with_chess_measured(tops, &rows, day);
    let tops = with_anagram_measured(tops, &rows, day);
    let tops = with_guess_measured(tops, &rows, day);
    let tops = order_like_activities(with_sudoku_measured(tops, &rows, day));
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

    fn chess_tally(user: u64, points: i64, games: i64, reached: i64) -> super::super::chess_store::Tally {
        super::super::chess_store::Tally { user, points, games, reached }
    }

    #[test]
    fn the_days_chess_card_goes_to_the_most_chess_points_then_the_most_games() {
        // Chess pays no house points now, but it still writes the zero row that
        // says who played, and the card is decided on the game's own uncapped
        // score rather than on wins.
        let rows = vec![row(1, "chess", 0, 100), row(2, "chess", 0, 110)];
        let scores = vec![chess_tally(1, 4, 1, 100), chess_tally(2, 8, 2, 120)];
        assert_eq!(chess_top(&rows, &scores), Some((2, 8)), "eight chess points beat four");
        // Level on points: the one who played more games.
        let even = vec![chess_tally(1, 8, 1, 100), chess_tally(2, 8, 3, 120)];
        assert_eq!(chess_top(&rows, &even), Some((2, 8)));
        // Level on both: whoever finished their last game first.
        let dead_heat = vec![chess_tally(1, 8, 2, 300), chess_tally(2, 8, 2, 120)];
        assert_eq!(chess_top(&rows, &dead_heat), Some((2, 8)), "whoever got there first");
        assert_eq!(chess_top(&rows, &[]), None, "nobody played");
        // A game that scored nothing — a rematch, or one given up at move two —
        // never wins the card on its own.
        assert_eq!(chess_top(&rows, &[chess_tally(1, 0, 3, 100)]), None);
        assert_eq!(activity_label("chess"), "♟️ Chess");
        assert!(ACTIVITIES.iter().any(|(a, s)| *a == "chess" && s.contains(&"chess")));
    }

    #[test]
    fn a_mod_can_out_play_the_channel_at_chess_and_still_never_take_the_card() {
        // 9 is a mod or a Muggle: the ledger turns them away at the door, so they
        // have no row and take no card — exactly as before. The chess points are
        // still theirs and still shown to them.
        let rows = vec![row(1, "chess", 0, 110)];
        let scores = vec![chess_tally(9, 40, 9, 90), chess_tally(1, 4, 1, 110)];
        assert_eq!(chess_top(&rows, &scores), Some((1, 4)));
        assert_eq!(chess_top(&rows, &[chess_tally(9, 40, 9, 90)]), None, "nobody the ledger knows played");
        assert_eq!(chess_top(&[], &scores), None, "and a day with no chess row at all has no card");
    }

    #[test]
    fn a_day_the_chess_store_knows_nothing_about_falls_back_to_the_ledger() {
        let rows = vec![row(1, "chess", 4, 10), row(2, "chess", 1, 20)];
        let ledgers_answer = daily_tops(&rows);
        assert_eq!(ledgers_answer, vec![("chess", 1, 4)], "the ledger's own points");
        assert_eq!(with_chess_tally(ledgers_answer.clone(), &rows, &[]), ledgers_answer, "an empty day keeps it");
        // Once the store does have the day, its answer replaces the ledger's.
        let scores = vec![chess_tally(2, 9, 3, 120)];
        assert_eq!(with_chess_tally(ledgers_answer.clone(), &rows, &scores), vec![("chess", 2, 9)]);
        // And every other activity is left exactly alone.
        let mixed = vec![("quiz", 5, 4), ("chess", 1, 4), ("anagram", 7, 3)];
        assert_eq!(with_chess_tally(mixed, &rows, &scores), vec![("quiz", 5, 4), ("anagram", 7, 3), ("chess", 2, 9)]);
        // A day with no finished game in the store — whether or not the store is
        // open at all in this run — leaves the ledger's answer alone.
        assert_eq!(with_chess_measured(ledgers_answer.clone(), &rows, "2019-01-01"), ledgers_answer);
    }

    fn tally(user: u64, points: i64, solves: i64, reached: i64) -> super::super::anagram_store::Tally {
        super::super::anagram_store::Tally { user, points, solves, reached }
    }

    #[test]
    fn the_anagram_card_goes_to_the_most_uncapped_points_not_the_most_solves() {
        // The whole point of the change: 1 wins ten four-letter rounds and is
        // stopped by the cap; 2 wins five eight-letter ones. Counting rows made
        // 1 the winner; counting what the rounds were worth makes it 2.
        // Two rounds paid, the next eight capped to zero but still in the ledger.
        let mut rows: Vec<LedgerRow> = (0..10).map(|i| row(1, "anagram", if i < 2 { 1 } else { 0 }, 100 + i)).collect();
        rows.extend((0..5).map(|i| row(2, "anagram", 3, 200 + i)));
        assert_eq!(daily_tops(&rows), vec![("anagram", 1, 10)], "the old rule: ten rows beat five");
        let scores = vec![tally(1, 10, 10, 109), tally(2, 15, 5, 204)];
        assert_eq!(anagram_top(&rows, &scores), Some((2, 15)));
        // And the swap puts that answer in the day's tops in place of the count.
        assert_eq!(with_anagram_tally(daily_tops(&rows), &rows, &scores), vec![("anagram", 2, 15)]);
        // Level on points: the one who won more rounds. Level on both: first there.
        let even = vec![tally(1, 12, 4, 109), tally(2, 12, 3, 204)];
        assert_eq!(anagram_top(&rows, &even), Some((1, 12)));
        let dead_heat = vec![tally(1, 12, 3, 300), tally(2, 12, 3, 204)];
        assert_eq!(anagram_top(&rows, &dead_heat), Some((2, 12)), "whoever got there first");
    }

    #[test]
    fn a_mod_can_out_score_the_channel_and_still_never_take_the_card() {
        // 9 is a mod: no house, so no ledger row at all. Their anagram points are
        // kept and shown to them, but the card is the house cup's and stays with
        // a housed player — the owner was explicit about that.
        let rows = vec![row(1, "anagram", 2, 100), row(1, "anagram", 0, 110)];
        let scores = vec![tally(9, 40, 9, 90), tally(1, 4, 2, 110)];
        assert_eq!(anagram_top(&rows, &scores), Some((1, 4)));
        // Nobody housed played: no card, rather than one for the mod.
        assert_eq!(anagram_top(&[], &scores), None);
        assert_eq!(anagram_top(&rows, &[]), None);
        // A capped day is still a paid day: the zero rows keep 1 eligible.
        let all_capped = vec![row(1, "anagram", 0, 100)];
        assert_eq!(anagram_top(&all_capped, &[tally(1, 9, 5, 150)]), Some((1, 9)));
        // And a row for some other game is not an anagram row.
        assert_eq!(anagram_top(&[row(1, "quiz", 5, 100)], &[tally(1, 9, 5, 150)]), None);
    }

    #[test]
    fn a_day_the_anagram_store_knows_nothing_about_falls_back_to_the_ledger() {
        // Days older than this change have no store rows; the counted-rows rule
        // still resolves them, so no old day is left without a card.
        let rows = vec![row(1, "anagram", 1, 100), row(1, "anagram", 0, 110), row(2, "anagram", 3, 120)];
        let ledgers_answer = daily_tops(&rows);
        assert_eq!(ledgers_answer, vec![("anagram", 1, 2)], "the count of rows, capped ones included");
        assert_eq!(with_anagram_tally(ledgers_answer.clone(), &rows, &[]), ledgers_answer, "an empty day keeps it");
        // The same when the store isn't open at all, as in this test run.
        assert!(super::super::anagram_store::db().is_none());
        assert_eq!(with_anagram_measured(ledgers_answer.clone(), &rows, "2026-09-13"), ledgers_answer);
        // Once the store does have the day, its answer replaces the count — and
        // a day nobody housed played takes the anagram card off the list.
        assert_eq!(with_anagram_tally(ledgers_answer.clone(), &rows, &[tally(2, 3, 1, 120)]), vec![("anagram", 2, 3)]);
        assert!(with_anagram_tally(ledgers_answer, &rows, &[tally(9, 30, 8, 120)]).is_empty());
        // Every other activity is left exactly alone.
        let mixed = vec![("quiz", 5, 4), ("anagram", 1, 2), ("chess", 7, 3)];
        assert_eq!(with_anagram_tally(mixed, &rows, &[tally(2, 3, 1, 120)]), vec![("quiz", 5, 4), ("chess", 7, 3), ("anagram", 2, 3)]);
    }

    fn guess_tally(user: u64, points: i64, solves: i64, reached: i64) -> super::super::guess_store::Tally {
        super::super::guess_store::Tally { user, points, solves, reached }
    }

    #[test]
    fn the_guess_card_goes_to_the_most_uncapped_points_not_the_most_solves() {
        // The whole point of the change: 1 names ten hinted doodles and is
        // stopped by the cap; 2 names five at their full worth. Counting rows
        // made 1 the winner; counting what the rounds were worth makes it 2.
        // Two rounds paid, the next eight capped to zero but still in the ledger.
        let mut rows: Vec<LedgerRow> = (0..10).map(|i| row(1, "guess", if i < 2 { 1 } else { 0 }, 100 + i)).collect();
        rows.extend((0..5).map(|i| row(2, "guess", 3, 200 + i)));
        assert_eq!(daily_tops(&rows), vec![("guess", 1, 10)], "the old rule: ten rows beat five");
        let scores = vec![guess_tally(1, 10, 10, 109), guess_tally(2, 15, 5, 204)];
        assert_eq!(guess_top(&rows, &scores), Some((2, 15)));
        // And the swap puts that answer in the day's tops in place of the count.
        assert_eq!(with_guess_tally(daily_tops(&rows), &rows, &scores), vec![("guess", 2, 15)]);
        // Level on points: the one who won more rounds. Level on both: first there.
        let even = vec![guess_tally(1, 12, 4, 109), guess_tally(2, 12, 3, 204)];
        assert_eq!(guess_top(&rows, &even), Some((1, 12)));
        let dead_heat = vec![guess_tally(1, 12, 3, 300), guess_tally(2, 12, 3, 204)];
        assert_eq!(guess_top(&rows, &dead_heat), Some((2, 12)), "whoever got there first");
    }

    #[test]
    fn a_mod_can_out_guess_the_channel_and_still_never_take_the_card() {
        // 9 is a mod: no house, so no ledger row at all. Their guess points are
        // kept and shown to them, but the card is the house cup's and stays with
        // a housed player — the owner was explicit about that.
        let rows = vec![row(1, "guess", 2, 100), row(1, "guess", 0, 110)];
        let scores = vec![guess_tally(9, 40, 9, 90), guess_tally(1, 4, 2, 110)];
        assert_eq!(guess_top(&rows, &scores), Some((1, 4)));
        // Nobody housed played: no card, rather than one for the mod.
        assert_eq!(guess_top(&[], &scores), None);
        assert_eq!(guess_top(&rows, &[]), None);
        // A capped day is still a paid day: the zero rows keep 1 eligible.
        let all_capped = vec![row(1, "guess", 0, 100)];
        assert_eq!(guess_top(&all_capped, &[guess_tally(1, 9, 5, 150)]), Some((1, 9)));
        // And a row for some other game is not a guess row.
        assert_eq!(guess_top(&[row(1, "anagram", 5, 100)], &[guess_tally(1, 9, 5, 150)]), None);
    }

    #[test]
    fn a_day_the_guess_store_knows_nothing_about_falls_back_to_the_ledger() {
        // Days older than this change have no store rows; the counted-rows rule
        // still resolves them, so no old day is left without a card.
        let rows = vec![row(1, "guess", 2, 100), row(1, "guess", 0, 110), row(2, "guess", 3, 120)];
        let ledgers_answer = daily_tops(&rows);
        assert_eq!(ledgers_answer, vec![("guess", 1, 2)], "the count of rows, capped ones included");
        assert_eq!(with_guess_tally(ledgers_answer.clone(), &rows, &[]), ledgers_answer, "an empty day keeps it");
        // The same when the store isn't open at all, as in this test run.
        assert!(super::super::guess_store::db().is_none());
        assert_eq!(with_guess_measured(ledgers_answer.clone(), &rows, "2026-09-13"), ledgers_answer);
        // Once the store does have the day, its answer replaces the count — and
        // a day nobody housed played takes the guess card off the list.
        assert_eq!(with_guess_tally(ledgers_answer.clone(), &rows, &[guess_tally(2, 3, 1, 120)]), vec![("guess", 2, 3)]);
        assert!(with_guess_tally(ledgers_answer, &rows, &[guess_tally(9, 30, 8, 120)]).is_empty());
        // Every other activity is left exactly alone, anagrams included.
        let mixed = vec![("quiz", 5, 4), ("guess", 1, 2), ("anagram", 7, 3)];
        assert_eq!(with_guess_tally(mixed, &rows, &[guess_tally(2, 3, 1, 120)]), vec![("quiz", 5, 4), ("anagram", 7, 3), ("guess", 2, 3)]);
    }

    fn sudoku_tally(user: u64, points: i64, solves: i64, reached: i64) -> super::super::sudoku_store::Tally {
        super::super::sudoku_store::Tally { user, points, solves, reached }
    }

    #[test]
    fn the_sudoku_card_goes_to_the_most_sudoku_points_not_the_most_puzzles() {
        // Sudoku pays no house points now, so every row is a zero receipt and
        // counting rows says whoever solved the MOST: 1, with five easy puzzles.
        // Counting what the puzzles were worth says 2, with three hard ones.
        let mut rows: Vec<LedgerRow> = (0..5).map(|i| row(1, "sudoku", 0, 100 + i)).collect();
        rows.extend((0..3).map(|i| row(2, "sudoku", 0, 200 + i)));
        assert_eq!(daily_tops(&rows), vec![("sudoku", 1, 5)], "the old rule: five rows beat three");
        let scores = vec![sudoku_tally(1, 10, 5, 104), sudoku_tally(2, 18, 3, 202)];
        assert_eq!(sudoku_top(&rows, &scores), Some((2, 18)));
        // And the swap puts that answer in the day's tops in place of the count.
        assert_eq!(with_sudoku_tally(daily_tops(&rows), &rows, &scores), vec![("sudoku", 2, 18)]);
        // Level on points: the one who solved more puzzles. Level on both: first there.
        let even = vec![sudoku_tally(1, 12, 4, 104), sudoku_tally(2, 12, 3, 202)];
        assert_eq!(sudoku_top(&rows, &even), Some((1, 12)));
        let dead_heat = vec![sudoku_tally(1, 12, 3, 300), sudoku_tally(2, 12, 3, 202)];
        assert_eq!(sudoku_top(&rows, &dead_heat), Some((2, 12)), "whoever got there first");
    }

    #[test]
    fn a_mod_can_out_solve_the_channel_and_still_never_take_the_sudoku_card() {
        // 9 is a mod: no house, so no ledger row at all. Their sudoku points are
        // kept and shown to them, but the card is the house cup's and stays with
        // a housed player — eligibility is exactly what it was.
        let rows = vec![row(1, "sudoku", 0, 100), row(1, "sudoku", 0, 110)];
        let scores = vec![sudoku_tally(9, 40, 9, 90), sudoku_tally(1, 6, 2, 110)];
        assert_eq!(sudoku_top(&rows, &scores), Some((1, 6)));
        // Nobody housed played: no card, rather than one for the mod.
        assert_eq!(sudoku_top(&[], &scores), None);
        assert_eq!(sudoku_top(&rows, &[]), None);
        // Every sudoku row is a zero now, and a zero row still makes 1 eligible.
        assert_eq!(sudoku_top(&[row(1, "sudoku", 0, 100)], &[sudoku_tally(1, 9, 3, 150)]), Some((1, 9)));
        // And a row for some other game is not a sudoku row.
        assert_eq!(sudoku_top(&[row(1, "quiz", 5, 100)], &[sudoku_tally(1, 9, 3, 150)]), None);
    }

    #[test]
    fn a_day_the_sudoku_store_knows_nothing_about_falls_back_to_the_ledger() {
        // Days older than this change have no `worth` at all; the counted-rows
        // rule still resolves them, so no old day is left without a card and the
        // frogtop:<day>:sudoku key is unchanged.
        let rows = vec![row(1, "sudoku", 4, 100), row(1, "sudoku", 0, 110), row(2, "sudoku", 6, 120)];
        let ledgers_answer = daily_tops(&rows);
        assert_eq!(ledgers_answer, vec![("sudoku", 1, 2)], "the count of rows, capped ones included");
        assert_eq!(with_sudoku_tally(ledgers_answer.clone(), &rows, &[]), ledgers_answer, "an empty day keeps it");
        // And through the real reader: a day the store has nothing for — one from
        // before this was built, or a run with no store open at all — is left
        // exactly as the ledger decided it.
        assert_eq!(with_sudoku_measured(ledgers_answer.clone(), &rows, "2026-09-13"), ledgers_answer);
        // Once the store does have the day, its answer replaces the count — and
        // a day nobody housed played takes the sudoku card off the list.
        assert_eq!(with_sudoku_tally(ledgers_answer.clone(), &rows, &[sudoku_tally(2, 6, 1, 120)]), vec![("sudoku", 2, 6)]);
        assert!(with_sudoku_tally(ledgers_answer, &rows, &[sudoku_tally(9, 30, 8, 120)]).is_empty());
        // Every other activity is left exactly alone.
        let mixed = vec![("quiz", 5, 4), ("sudoku", 1, 2), ("anagram", 7, 3)];
        assert_eq!(with_sudoku_tally(mixed, &rows, &[sudoku_tally(2, 6, 1, 120)]), vec![("quiz", 5, 4), ("anagram", 7, 3), ("sudoku", 2, 6)]);
    }

    #[test]
    fn activity_names_read_well() {
        assert_eq!(activity_label("chat"), "💬 Chat");
        assert_eq!(activity_label("snitch"), "🪽 Snitch");
        assert_eq!(activity_label("frog"), "🐸 Frogs");
        assert_eq!(activity_label("wordle"), "🟩 Wordle");
    }
}
