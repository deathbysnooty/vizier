//! The nightly scan: who, out of a couple of hundred people, is worth a look
//! this morning.
//!
//! It runs in two halves, and the cheap half runs first. **The picking is
//! counts and nothing else** — no model, no reading, no tokens: a fight the
//! kalesh detector called, AutoMod stopping them over and over, a pile of
//! deleted messages, a day far quieter or far louder than their own fortnight,
//! a new member who has suddenly settled in. Every one of those is arithmetic
//! over numbers the bot already has, so the scan over the whole server costs
//! nothing worth measuring.
//!
//! Only then does the expensive half run, and only for the five to twenty
//! people the first half picked: each gets the ordinary deep-dive summary, the
//! same prompt and the same retrying model call a moderator would have paid for
//! by pressing the button. Those land on the Deep dives page marked with why
//! they were picked, so opening the panel in the morning is opening a list of
//! people who need attention rather than a search box.
//!
//! Nothing is guessed about anybody here. A flag says "their day stands out
//! from their own other days", never "they are a problem"; the summary that
//! follows is written under `deepdive`'s neutral instructions, and a moderator
//! reads the messages themselves before acting on any of it.

use std::collections::HashMap;

use serde::Serialize;

use super::control;

// --- settings ---------------------------------------------------------------------------------

/// Off stops the whole thing: no scan, no nightly dives.
pub fn watch_on() -> bool {
    control::on("VIZIER_WATCH", true)
}

/// The hour of the India day the scan runs at. After the topic pass, so the
/// morning has both.
pub fn watch_hour() -> u32 {
    control::number("VIZIER_WATCH_HOUR", 6).clamp(0, 23) as u32
}

/// Members one night may dive into. The point of the page is a short list a
/// moderator will actually read.
pub fn watch_max() -> usize {
    control::number("VIZIER_WATCH_MAX", 12).clamp(0, 60) as usize
}

/// Messages AutoMod blocked in one day before it is worth a look.
pub fn automod_at() -> i64 {
    control::number("VIZIER_WATCH_AUTOMOD", 3).clamp(1, 500) as i64
}

/// Messages of theirs deleted in one day before it is worth a look.
pub fn deleted_at() -> i64 {
    control::number("VIZIER_WATCH_DELETED", 8).clamp(1, 1_000) as i64
}

/// How chatty somebody's ordinary day has to be before a quiet one or a loud
/// one means anything. Below this the numbers are too small to read.
pub fn quiet_floor() -> i64 {
    control::number("VIZIER_WATCH_QUIET", 25).clamp(1, 5_000) as i64
}

/// How new a member still counts as, in days, for "settled in".
pub fn new_days() -> i64 {
    control::number("VIZIER_WATCH_NEW_DAYS", 14).clamp(1, 120) as i64
}

/// The day that counts as a new member becoming properly active.
pub fn new_messages() -> i64 {
    control::number("VIZIER_WATCH_NEW_MESSAGES", 30).clamp(1, 5_000) as i64
}

/// The period each nightly dive covers.
pub const DIVE_DAYS: i64 = 7;
/// Days of their own history a drop or a jump is judged against.
pub const BASELINE_DAYS: usize = 14;
/// Today is a drop when it is at or under this share of their ordinary day.
pub const DROP_SHARE: f64 = 0.25;
/// Today is a jump when it is at or over this many times their ordinary day.
pub const JUMP_TIMES: f64 = 3.0;
/// Below this many ordinary days there is no baseline worth comparing to.
pub const MIN_BASELINE_DAYS: usize = 5;

const DAY: i64 = 86_400;

/// The numbers a scan runs on, so a test can set its own.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    pub automod: i64,
    pub deleted: i64,
    pub quiet: i64,
    pub new_days: i64,
    pub new_messages: i64,
    pub cap: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { automod: 3, deleted: 8, quiet: 25, new_days: 14, new_messages: 30, cap: 12 }
    }
}

/// The thresholds as the panel has them now.
pub fn thresholds() -> Thresholds {
    Thresholds {
        automod: automod_at(),
        deleted: deleted_at(),
        quiet: quiet_floor(),
        new_days: new_days(),
        new_messages: new_messages(),
        cap: watch_max(),
    }
}

// --- one member's day --------------------------------------------------------------------------

/// Everything the scan knows about one member's day. Counts only: nothing here
/// is a word anybody wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Day {
    pub user_id: u64,
    /// Messages they sent today.
    pub messages: i64,
    /// Of theirs, deleted today.
    pub deleted: i64,
    /// Of theirs, AutoMod stopped today.
    pub blocked: i64,
    /// Fights the live detector called today that they were in.
    pub kaleshes: i64,
    /// Their messages on each of the days before today, newest first.
    pub before: Vec<i64>,
    /// When they joined, when the join log knows.
    pub joined_ts: Option<i64>,
}

/// Why somebody was picked. The kinds are stored as they are spelled here, so a
/// page written later still reads a row written tonight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Kalesh,
    Automod,
    Deleted,
    Drop,
    Jump,
    NewlyActive,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Kalesh => "kalesh",
            Kind::Automod => "automod",
            Kind::Deleted => "deleted",
            Kind::Drop => "drop",
            Kind::Jump => "jump",
            Kind::NewlyActive => "newly_active",
        }
    }

    /// How much this one matters when the cap bites: a fight before a quiet day.
    pub fn weight(self) -> i64 {
        match self {
            Kind::Kalesh => 100,
            Kind::Automod => 80,
            Kind::Deleted => 60,
            Kind::Jump => 40,
            Kind::Drop => 30,
            Kind::NewlyActive => 20,
        }
    }
}

/// One reason somebody is on tonight's list, with the numbers behind it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Flag {
    pub kind: Kind,
    /// The plain sentence the page prints.
    pub what: String,
}

/// Their ordinary day: the middle of the last fortnight, so one silent Sunday
/// or one all-nighter does not become the thing everything else is judged
/// against. `None` when there are too few days to say.
pub fn baseline(before: &[i64]) -> Option<i64> {
    let mut days: Vec<i64> = before.iter().copied().take(BASELINE_DAYS).collect();
    if days.len() < MIN_BASELINE_DAYS {
        return None;
    }
    days.sort_unstable();
    Some(days[days.len() / 2])
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// Every reason this day stands out, strongest first — or nothing, which is
/// what almost everybody gets on almost every night.
pub fn flags(d: &Day, t: &Thresholds, now: i64) -> Vec<Flag> {
    let mut out: Vec<Flag> = Vec::new();
    if d.kaleshes > 0 {
        out.push(Flag { kind: Kind::Kalesh, what: format!("In {} the detector called today", plural(d.kaleshes, "fight", "fights")) });
    }
    if d.blocked >= t.automod {
        out.push(Flag { kind: Kind::Automod, what: format!("AutoMod stopped {} today", plural(d.blocked, "message", "messages")) });
    }
    if d.deleted >= t.deleted {
        out.push(Flag { kind: Kind::Deleted, what: format!("{} of theirs were deleted today", plural(d.deleted, "message", "messages")) });
    }
    if let Some(usual) = baseline(&d.before) {
        // A drop is only readable for somebody who normally says a fair amount.
        if usual >= t.quiet && (d.messages as f64) <= usual as f64 * DROP_SHARE {
            out.push(Flag { kind: Kind::Drop, what: format!("Down to {} today, from about {} on an ordinary day", d.messages, usual) });
        }
        // A jump needs a baseline to jump from, and a today worth noticing.
        if usual >= MIN_BASELINE_DAYS as i64 && d.messages >= t.quiet && (d.messages as f64) >= usual as f64 * JUMP_TIMES {
            out.push(Flag { kind: Kind::Jump, what: format!("Up to {} today, from about {} on an ordinary day", d.messages, usual) });
        }
    }
    // Somebody who joined recently and has just had their first real day. Their
    // own history is what says "first": a fortnight of quiet days behind them.
    if let Some(joined) = d.joined_ts {
        let fresh = now - joined <= t.new_days * DAY && joined <= now;
        let first = d.before.iter().all(|n| *n < t.new_messages);
        if fresh && first && d.messages >= t.new_messages {
            out.push(Flag {
                kind: Kind::NewlyActive,
                what: format!("New here, and properly active for the first time: {} today", plural(d.messages, "message", "messages")),
            });
        }
    }
    out.sort_by(|a, b| b.kind.weight().cmp(&a.kind.weight()));
    out
}

/// Somebody the scan picked, and why.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Picked {
    pub user_id: u64,
    pub flags: Vec<Flag>,
}

impl Picked {
    /// The reasons as one line, for the row on the Deep dives page.
    pub fn why(&self) -> String {
        self.flags.iter().map(|f| f.what.as_str()).collect::<Vec<_>>().join(" · ")
    }

    /// The kinds, comma-separated: what the store files it under.
    pub fn kinds(&self) -> String {
        self.flags.iter().map(|f| f.kind.key()).collect::<Vec<_>>().join(",")
    }

    fn score(&self) -> i64 {
        self.flags.iter().map(|f| f.kind.weight()).sum()
    }
}

/// Tonight's list, worst first and no longer than the cap. Deterministic: the
/// same night's numbers always pick the same people in the same order, so a
/// restart that runs the scan again would do exactly what the first one did.
pub fn pick(days: &[Day], t: &Thresholds, now: i64) -> Vec<Picked> {
    let mut out: Vec<Picked> = days
        .iter()
        .filter_map(|d| {
            let flags = flags(d, t, now);
            (!flags.is_empty()).then(|| Picked { user_id: d.user_id, flags })
        })
        .collect();
    out.sort_by(|a, b| b.score().cmp(&a.score()).then(b.flags.len().cmp(&a.flags.len())).then(a.user_id.cmp(&b.user_id)));
    out.truncate(t.cap);
    out
}

/// What one night's scan did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scan {
    /// Members whose day was read.
    pub looked_at: usize,
    /// Of those, the ones whose day stood out.
    pub flagged: usize,
    /// Of those, the ones a summary was written for.
    pub dived: usize,
    /// Members whose summary the model would not write. Their row is not on the
    /// page at all: half a summary is worse than none.
    pub failed: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,
}

/// Each member's messages per India day, newest day first, out of a table of
/// (member, day, count) rows. `days` are the days to read, newest first.
pub fn history(counts: &[(u64, String, i64)], days: &[String]) -> HashMap<u64, Vec<i64>> {
    let mut by: HashMap<(u64, &str), i64> = HashMap::new();
    for (user, day, n) in counts {
        *by.entry((*user, day.as_str())).or_insert(0) += n;
    }
    let mut out: HashMap<u64, Vec<i64>> = HashMap::new();
    for (user, _, _) in counts {
        out.entry(*user).or_insert_with(|| days.iter().map(|d| by.get(&(*user, d.as_str())).copied().unwrap_or(0)).collect());
    }
    out
}

// --- reading the numbers off the stores --------------------------------------------------------

/// The India days a day's history is judged against: the [`BASELINE_DAYS`]
/// before it, newest first.
pub fn days_before(day: &str, back: usize) -> Vec<String> {
    let Some(start) = super::points::ist_day_start(day) else { return Vec::new() };
    (1..=back as i64).map(|n| super::points::ist_day(start - n * DAY)).collect()
}

/// Everybody's messages per India day over a stretch of days. From the activity
/// store, which already counts a message the moment it arrives — so the whole
/// scan over a couple of hundred members is one query and no model at all.
fn day_counts(from_day: &str, to_day: &str) -> Vec<(u64, String, i64)> {
    let Some(db) = super::stats::db() else { return Vec::new() };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT user_id, day, SUM(count) FROM msg_counts WHERE day >= ?1 AND day <= ?2 GROUP BY user_id, day") else {
        return Vec::new();
    };
    stmt.query_map(rusqlite::params![from_day, to_day], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)))
        .map(|rows| rows.flatten().filter(|(user, _, _)| *user != 0).collect())
        .unwrap_or_default()
}

/// Per member, how many of their messages were deleted in the window and how
/// many AutoMod stopped. Counts only: no text is read out of either table.
fn gone_counts(from_ms: i64, to_ms: i64) -> (HashMap<u64, i64>, HashMap<u64, i64>) {
    let Some(reader) = super::msglog::reader() else { return (HashMap::new(), HashMap::new()) };
    let conn = reader.conn.lock();
    let tally = |sql: &str| -> HashMap<u64, i64> {
        let Ok(mut stmt) = conn.prepare_cached(sql) else { return HashMap::new() };
        stmt.query_map(rusqlite::params![from_ms, to_ms], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?)))
            .map(|rows| rows.flatten().filter(|(user, _)| *user != 0).collect())
            .unwrap_or_default()
    };
    (
        tally("SELECT author_id, COUNT(*) FROM deleted WHERE deleted_ts >= ?1 AND deleted_ts < ?2 AND author_id IS NOT NULL GROUP BY author_id"),
        tally("SELECT author_id, COUNT(*) FROM blocked WHERE created_ts >= ?1 AND created_ts < ?2 GROUP BY author_id"),
    )
}

/// Fights the live detector called in the window, per member in them.
fn kalesh_counts(from_ms: i64, to_ms: i64) -> HashMap<u64, i64> {
    let Some(db) = super::kalesh_store::db() else { return HashMap::new() };
    let conn = db.lock();
    let mut out: HashMap<u64, i64> = HashMap::new();
    for d in super::kalesh_store::detections(&conn, 1_000).unwrap_or_default() {
        if d.start_ms < from_ms || d.start_ms >= to_ms {
            continue;
        }
        for p in &d.participants {
            *out.entry(p.id).or_insert(0) += 1;
        }
    }
    out
}

/// When each member joined, for the ones the join log caught.
fn joined_at() -> HashMap<u64, i64> {
    let Some(db) = super::invites_store::db() else { return HashMap::new() };
    let conn = db.lock();
    let mut out: HashMap<u64, i64> = HashMap::new();
    for j in super::invites_store::all_joins(&conn).unwrap_or_default() {
        let slot = out.entry(j.new.member_id).or_insert(j.new.joined_ts);
        *slot = (*slot).max(j.new.joined_ts);
    }
    out
}

/// Everybody's day, ready to be judged. Blocking: every read here is a query
/// against a store the bot already keeps, and not one token is spent.
pub fn gather(day: &str, real: Option<&std::collections::HashSet<u64>>) -> Vec<Day> {
    let Some(start) = super::points::ist_day_start(day) else { return Vec::new() };
    let (from_ms, to_ms) = (start * 1000, (start + DAY) * 1000);
    let history_days = days_before(day, BASELINE_DAYS);
    let oldest = history_days.last().cloned().unwrap_or_else(|| day.to_string());
    let counts = day_counts(&oldest, day);
    let before = history(&counts, &history_days);
    let today: HashMap<u64, i64> = counts.iter().filter(|(_, d, _)| d == day).map(|(u, _, n)| (*u, *n)).collect();
    let (deleted, blocked) = gone_counts(from_ms, to_ms);
    let kaleshes = kalesh_counts(from_ms, to_ms);
    let joined = joined_at();

    let mut everyone: Vec<u64> = counts.iter().map(|(u, _, _)| *u).collect();
    everyone.extend(deleted.keys().copied());
    everyone.extend(blocked.keys().copied());
    everyone.extend(kaleshes.keys().copied());
    everyone.sort_unstable();
    everyone.dedup();
    everyone
        .into_iter()
        .filter(|u| real.is_none_or(|set| set.contains(u)))
        .map(|user_id| Day {
            user_id,
            messages: today.get(&user_id).copied().unwrap_or(0),
            deleted: deleted.get(&user_id).copied().unwrap_or(0),
            blocked: blocked.get(&user_id).copied().unwrap_or(0),
            kaleshes: kaleshes.get(&user_id).copied().unwrap_or(0),
            before: before.get(&user_id).cloned().unwrap_or_default(),
            joined_ts: joined.get(&user_id).copied(),
        })
        .collect()
}

// --- the dives ----------------------------------------------------------------------------------

/// What only the gateway can tell the scan: what things are called, and who is
/// really a member rather than a bot or somebody long gone.
#[derive(Clone, Debug, Default)]
pub struct Cache {
    pub channels: HashMap<u64, String>,
    pub members: HashMap<u64, String>,
    pub real: std::collections::HashSet<u64>,
}

/// One member's own kept messages over the dive's window, oldest first, with
/// the channels that are never shown left out. Blocking.
fn read_rows(member: u64, from_ms: i64, to_ms: i64, channels: &HashMap<u64, String>) -> Vec<super::msglog::SaidRow> {
    let Some(reader) = super::msglog::reader() else { return Vec::new() };
    let never = super::msglog::never_logged();
    let mut rows = {
        let conn = reader.conn.lock();
        super::kalesh::authors_between(&conn, &[member], from_ms, to_ms, None, super::deepdive::MAX_ROWS + 1).unwrap_or_default()
    };
    // The log never kept a word of #safe-corner, but the skip list can grow
    // after a message was kept, so what is never shown goes here too.
    rows.retain(|r| {
        let place = super::msglog::Place { channel_id: r.channel_id, parent_id: r.parent_id, channel_name: r.channel_name.clone() };
        let parent_name = r.parent_id.and_then(|p| channels.get(&p)).map(String::as_str);
        !super::msglog::excluded(&place, parent_name, &never)
    });
    rows.sort_by_key(|r| r.message_id);
    rows.dedup_by_key(|r| r.message_id);
    if rows.len() > super::deepdive::MAX_ROWS {
        // Keep the end: the newest of a long window is what a moderator is after.
        rows.drain(..rows.len() - super::deepdive::MAX_ROWS);
    }
    rows
}

/// Everybody's voice stretches over the window. Blocking.
fn read_stays(from: i64, to: i64, now: i64) -> Vec<super::deepdive::VoiceStay> {
    let Some(db) = super::stats::db() else { return Vec::new() };
    let conn = db.lock();
    super::activity::voice_stays(&conn, from, to, now)
        .unwrap_or_default()
        .into_iter()
        .map(|(user_id, channel_id, start, end)| super::deepdive::VoiceStay { user_id, channel_id, start, end })
        .collect()
}

/// One nightly deep dive, written and filed with the reason it was run.
///
/// Nothing is stored unless the model answered: a dive is either the whole
/// thing or nothing at all, because a half-written summary on a page a
/// moderator is about to act on is worse than an honest gap. A window that has
/// already been summarised is handed back rather than paid for again.
async fn dive_into(picked: &Picked, cache: &Cache, now: i64) -> Result<Option<super::kalesh::Reply>, String> {
    let member = picked.user_id;
    let Some(store) = super::kalesh_store::db() else { return Err("the summary store isn't open".into()) };
    let period = super::deepdive::Period { from: now - DIVE_DAYS * DAY, to: now, shifted: false };
    let channels = cache.channels.clone();
    let (from_ms, to_ms) = (period.from * 1000, period.to * 1000);
    let rows = tokio::task::spawn_blocking(move || read_rows(member, from_ms, to_ms, &channels)).await.unwrap_or_default();
    if rows.is_empty() {
        return Ok(None);
    }
    let refs: Vec<&super::msglog::SaidRow> = rows.iter().collect();
    let key = super::deepdive::key(member, DIVE_DAYS, &refs);
    // Somebody already paid for exactly this window today.
    if super::kalesh_store::summary_for(&store.lock(), &key).unwrap_or(None).is_some() {
        tracing::debug!("watch: {}'s week is already summarised", member);
        return Ok(None);
    }

    let stays = tokio::task::spawn_blocking(move || read_stays(period.from, period.to, now)).await.unwrap_or_default();
    let sessions = super::deepdive::sessions(&stays, member, period.from, period.to);
    let rooms: HashMap<u64, String> =
        sessions.iter().map(|s| s.channel_id).collect::<std::collections::HashSet<u64>>().into_iter().filter_map(|id| cache.channels.get(&id).map(|n| (id, n.clone()))).collect();
    let names: HashMap<u64, String> =
        sessions.iter().flat_map(|s| s.with.iter().map(|(id, _)| *id)).collect::<std::collections::HashSet<u64>>().into_iter().filter_map(|id| cache.members.get(&id).map(|n| (id, n.clone()))).collect();
    let name = cache.members.get(&member).cloned().unwrap_or_else(|| refs.last().map(|r| r.author_name.clone()).unwrap_or_else(|| member.to_string()));
    let words = super::deepdive::period_words(&period, false);
    let prompt = super::deepdive::build_prompt(&name, &words, &refs, &sessions, &rooms, &names, super::kalesh::summary_max());

    // The same retrying path a moderator's own press goes through.
    let reply = super::kalesh::ask_retrying("watch: dive", || super::kalesh::ask_live(prompt.text.clone())).await?;
    let filed = super::deepdive::Filed {
        member,
        days: DIVE_DAYS,
        period,
        message_ids: refs.iter().map(|r| r.message_id).collect(),
        // Nobody pressed a button: this one is the night's.
        run_by: 0,
        run_ts: now,
        reason: &picked.why(),
    };
    let new = super::deepdive::new_summary(&filed, key, &prompt, reply.clone());
    super::kalesh_store::add_summary(&store.lock(), &new).map_err(|err| format!("the summary store would not take it: {}", err))?;
    tracing::info!("watch: dived into {} ({}) — {} + {} tokens", name, picked.kinds(), new.input_tokens, new.output_tokens);
    Ok(Some(reply))
}

// --- one night -----------------------------------------------------------------------------------

static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Between dives, so a night of twenty never holds the bot up.
const BREATH: std::time::Duration = std::time::Duration::from_secs(2);
/// How often the job looks to see whether tonight's scan is due.
const CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(1800);
/// Nothing runs until the bot has been up a few minutes.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(360);

/// One night's scan over one India day: the cheap half, then the expensive half
/// for whoever the cheap half picked. `None` when the night was already
/// somebody else's — which is what makes a restart harmless.
pub async fn run_night(day: String, now: i64, cache: Cache) -> Option<Scan> {
    use std::sync::atomic::Ordering;
    let db = super::topics_store::db()?;
    if RUNNING.swap(true, Ordering::SeqCst) {
        return None;
    }
    let out = run_claimed(&day, now, cache).await;
    RUNNING.store(false, Ordering::SeqCst);
    let _ = db;
    out
}

async fn run_claimed(day: &str, now: i64, cache: Cache) -> Option<Scan> {
    use super::topics_store::{self as runs, Done, KIND_SCAN};
    let db = runs::db()?;
    if !runs::claim(&db.lock(), KIND_SCAN, day, now).unwrap_or(false) {
        tracing::debug!("watch: {} has already been scanned", day);
        return None;
    }
    let real = (!cache.real.is_empty()).then(|| cache.real.clone());
    let scan_day = day.to_string();
    let days = tokio::task::spawn_blocking(move || gather(&scan_day, real.as_ref())).await.unwrap_or_default();
    let t = thresholds();
    let picked = pick(&days, &t, now);
    let mut scan = Scan { looked_at: days.len(), flagged: picked.len(), ..Scan::default() };
    let mut last_err = String::new();
    tracing::info!("watch: {} — {} of {} members had a day worth a look", day, picked.len(), days.len());

    for one in &picked {
        match dive_into(one, &cache, now).await {
            Ok(Some(reply)) => {
                scan.dived += 1;
                scan.input_tokens += reply.input_tokens;
                scan.output_tokens += reply.output_tokens;
                if scan.model.is_empty() {
                    scan.model = reply.model;
                }
            }
            // Nothing of theirs to read, or it was summarised already.
            Ok(None) => {}
            Err(err) => {
                tracing::error!("watch: {} was picked but not summarised: {}", one.user_id, err);
                last_err = err;
                scan.failed += 1;
            }
        }
        tokio::time::sleep(BREATH).await;
    }

    // A failure the page can read, not only the log: how many, and why.
    let note = if scan.failed > 0 {
        format!("{} of {} dives failed — {}", scan.failed, scan.flagged, super::kalesh::why_failed(&last_err))
    } else {
        String::new()
    };
    let done = Done {
        chunks: scan.looked_at as i64,
        failed: scan.failed as i64,
        members: scan.dived as i64,
        input_tokens: scan.input_tokens as i64,
        output_tokens: scan.output_tokens as i64,
        model: scan.model.clone(),
        note,
        ..Done::default()
    };
    let _ = runs::finish(&db.lock(), KIND_SCAN, day, &done, now);
    Some(scan)
}

/// Everything the gateway knows that a scan needs.
pub fn cache_of(ctx: &serenity::all::Context) -> Cache {
    let mut out = Cache::default();
    for guild in ctx.cache.guilds() {
        let Some(g) = ctx.cache.guild(guild) else { continue };
        for (id, channel) in g.channels.iter() {
            out.channels.insert(id.get(), channel.name.clone());
        }
        for thread in g.threads.iter() {
            out.channels.insert(thread.id.get(), thread.name.clone());
        }
        for (id, m) in g.members.iter() {
            if !m.user.bot {
                out.members.insert(id.get(), m.display_name().to_string());
                out.real.insert(id.get());
            }
        }
    }
    out
}

/// Checks every half hour whether tonight's scan is due. Starts once per process.
pub fn spawn(ctx: serenity::all::Context) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        loop {
            if watch_on() && watch_max() > 0 && super::topics_store::db().is_some() {
                let now = chrono::Utc::now().timestamp();
                if super::points::ist_hour(now) as u32 == watch_hour() {
                    // The last whole India day, the same one the topic pass read.
                    let day = super::points::ist_day(now - DAY);
                    run_night(day, now, cache_of(&ctx)).await;
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    /// Somebody with a settled fortnight behind them: the day everything else
    /// is measured against.
    fn ordinary(user: u64) -> Day {
        Day { user_id: user, messages: 100, before: vec![100; BASELINE_DAYS], joined_ts: Some(NOW - 400 * DAY), ..Day::default() }
    }

    fn kinds(d: &Day, t: &Thresholds) -> Vec<Kind> {
        flags(d, t, NOW).into_iter().map(|f| f.kind).collect()
    }

    /// An ordinary day for an ordinary member is not worth anybody's morning.
    #[test]
    fn a_normal_day_flags_nothing() {
        let t = Thresholds::default();
        assert!(flags(&ordinary(11), &t, NOW).is_empty());
        // Nor a day a bit either way, nor a couple of deletions.
        for messages in [60, 140, 250] {
            assert!(flags(&Day { messages, ..ordinary(11) }, &t, NOW).is_empty(), "{messages} is a normal day");
        }
        assert!(flags(&Day { deleted: 7, blocked: 2, ..ordinary(11) }, &t, NOW).is_empty(), "just under both thresholds");
        // And somebody the bot barely knows is never judged against a baseline
        // it does not have.
        let new = Day { user_id: 12, messages: 0, before: vec![80, 90], joined_ts: None, ..Day::default() };
        assert!(flags(&new, &t, NOW).is_empty(), "two days is not a fortnight");
        assert_eq!(baseline(&[80, 90]), None);
        assert_eq!(baseline(&[10, 100, 20, 30, 40]), Some(30), "the middle day, not the average");
    }

    /// A fight the detector called is the first thing a moderator wants.
    #[test]
    fn a_kalesh_fires_and_a_quiet_day_does_not() {
        let t = Thresholds::default();
        assert_eq!(kinds(&Day { kaleshes: 1, ..ordinary(11) }, &t), vec![Kind::Kalesh]);
        assert_eq!(flags(&Day { kaleshes: 2, ..ordinary(11) }, &t, NOW)[0].what, "In 2 fights the detector called today");
        assert_eq!(flags(&Day { kaleshes: 1, ..ordinary(11) }, &t, NOW)[0].what, "In 1 fight the detector called today");
        assert!(kinds(&Day { kaleshes: 0, ..ordinary(11) }, &t).is_empty());
    }

    /// AutoMod stopping somebody over and over, and deletions piling up.
    #[test]
    fn automod_blocks_and_deletions_fire_at_their_thresholds_and_not_below() {
        let t = Thresholds::default();
        assert!(kinds(&Day { blocked: 2, ..ordinary(11) }, &t).is_empty(), "two is not a pattern");
        assert_eq!(kinds(&Day { blocked: 3, ..ordinary(11) }, &t), vec![Kind::Automod], "three is");
        assert!(kinds(&Day { deleted: 7, ..ordinary(11) }, &t).is_empty());
        assert_eq!(kinds(&Day { deleted: 8, ..ordinary(11) }, &t), vec![Kind::Deleted]);
        // Both at once, worst first.
        assert_eq!(kinds(&Day { blocked: 5, deleted: 20, ..ordinary(11) }, &t), vec![Kind::Automod, Kind::Deleted]);
        // And the thresholds are settings, not constants.
        let strict = Thresholds { automod: 1, deleted: 2, ..t };
        assert_eq!(kinds(&Day { blocked: 1, ..ordinary(11) }, &strict), vec![Kind::Automod]);
        let loose = Thresholds { automod: 50, deleted: 100, ..t };
        assert!(kinds(&Day { blocked: 20, deleted: 40, ..ordinary(11) }, &loose).is_empty());
    }

    /// Somebody who talks all day going quiet, and somebody quiet going loud.
    #[test]
    fn a_sharp_drop_and_a_sharp_jump_each_fire_and_neither_fires_on_noise() {
        let t = Thresholds::default();
        // A hundred a day, then twenty: that is a drop.
        assert_eq!(kinds(&Day { messages: 20, ..ordinary(11) }, &t), vec![Kind::Drop]);
        assert_eq!(kinds(&Day { messages: 25, ..ordinary(11) }, &t), vec![Kind::Drop], "a quarter is the line");
        assert!(kinds(&Day { messages: 26, ..ordinary(11) }, &t).is_empty(), "just over it is an ordinary quiet day");
        assert_eq!(
            flags(&Day { messages: 5, ..ordinary(11) }, &t, NOW)[0].what,
            "Down to 5 today, from about 100 on an ordinary day"
        );
        // Somebody who says almost nothing anyway cannot "go quiet".
        let small = Day { user_id: 12, messages: 0, before: vec![6; BASELINE_DAYS], joined_ts: Some(NOW - 400 * DAY), ..Day::default() };
        assert!(!kinds(&small, &t).contains(&Kind::Drop), "six a day is below the floor the drop is judged at");

        // Ten a day, then sixty: that is a jump.
        let steady = Day { user_id: 13, messages: 60, before: vec![10; BASELINE_DAYS], joined_ts: Some(NOW - 400 * DAY), ..Day::default() };
        assert_eq!(kinds(&steady, &t), vec![Kind::Jump]);
        assert!(!kinds(&Day { messages: 24, ..steady.clone() }, &t).contains(&Kind::Jump), "under the floor, however many times the baseline");
        assert!(!kinds(&Day { messages: 29, before: vec![20; BASELINE_DAYS], ..steady.clone() }, &t).contains(&Kind::Jump), "not three times");
        // A jump needs something to jump from: one message a day is noise.
        let noise = Day { user_id: 14, messages: 40, before: vec![1; BASELINE_DAYS], joined_ts: Some(NOW - 400 * DAY), ..Day::default() };
        assert!(!kinds(&noise, &t).contains(&Kind::Jump), "a baseline of one is not a baseline");
        // A day cannot be both.
        for messages in [0, 20, 60, 400] {
            let k = kinds(&Day { messages, ..ordinary(11) }, &t);
            assert!(!(k.contains(&Kind::Drop) && k.contains(&Kind::Jump)), "{messages}: {k:?}");
        }
    }

    /// A new member who has just settled in — once, not every night after.
    #[test]
    fn a_new_member_settling_in_fires_once() {
        let t = Thresholds::default();
        let quiet_first_week = vec![4, 6, 2, 0, 5, 3, 1, 0];
        let settling = Day { user_id: 15, messages: 40, before: quiet_first_week.clone(), joined_ts: Some(NOW - 8 * DAY), ..Day::default() };
        assert!(kinds(&settling, &t).contains(&Kind::NewlyActive));
        assert!(flags(&settling, &t, NOW).iter().any(|f| f.what.contains("properly active for the first time")));

        // Under the day that counts: not yet.
        assert!(!kinds(&Day { messages: 29, ..settling.clone() }, &t).contains(&Kind::NewlyActive));
        // Already had a day like it: this is not the first.
        let again = Day { before: vec![40, 6, 2, 0, 5, 3, 1, 0], ..settling.clone() };
        assert!(!kinds(&again, &t).contains(&Kind::NewlyActive), "it fires the once");
        // Not new any more.
        assert!(!kinds(&Day { joined_ts: Some(NOW - 30 * DAY), ..settling.clone() }, &t).contains(&Kind::NewlyActive));
        // Nobody knows when they joined: no claim either way.
        assert!(!kinds(&Day { joined_ts: None, ..settling.clone() }, &t).contains(&Kind::NewlyActive));
        // And the window is a setting.
        assert!(kinds(&Day { joined_ts: Some(NOW - 30 * DAY), ..settling }, &Thresholds { new_days: 60, ..t }).contains(&Kind::NewlyActive));
    }

    /// The cap is a cap, and the worst days are the ones that fit inside it.
    #[test]
    fn the_cap_holds_and_takes_the_worst_first() {
        let t = Thresholds { cap: 3, ..Thresholds::default() };
        let mut days: Vec<Day> = Vec::new();
        // Twenty people whose day was merely quiet.
        for i in 0..20u64 {
            days.push(Day { messages: 10, ..ordinary(100 + i) });
        }
        // And four whose day was rather more than that.
        days.push(Day { kaleshes: 1, ..ordinary(7) });
        days.push(Day { blocked: 9, ..ordinary(8) });
        days.push(Day { deleted: 30, ..ordinary(9) });
        days.push(ordinary(10));

        let got = pick(&days, &t, NOW);
        assert_eq!(got.len(), 3, "the cap holds however many stood out");
        assert_eq!(got.iter().map(|p| p.user_id).collect::<Vec<_>>(), vec![7, 8, 9], "the fight, the blocks and the deletions, in that order");
        assert!(!got.iter().any(|p| p.user_id == 10), "somebody with an ordinary day is never on the list");
        assert_eq!(got[0].kinds(), "kalesh");
        assert!(got[0].why().starts_with("In 1 fight"));

        // Uncapped, everybody who stood out is on it, and nobody else.
        let all = pick(&days, &Thresholds { cap: 100, ..t }, NOW);
        assert_eq!(all.len(), 23, "twenty quiet days and three louder ones");
        // Nought is the same as off.
        assert!(pick(&days, &Thresholds { cap: 0, ..t }, NOW).is_empty());
        // And it is the same list every time, so a restart repeats itself exactly.
        assert_eq!(pick(&days, &t, NOW), got);

        // Several reasons at once outrank one.
        let stacked = vec![Day { kaleshes: 1, blocked: 9, deleted: 40, ..ordinary(20) }, Day { kaleshes: 1, ..ordinary(21) }];
        let ranked = pick(&stacked, &Thresholds { cap: 10, ..t }, NOW);
        assert_eq!(ranked[0].user_id, 20);
        assert_eq!(ranked[0].flags.len(), 3);
        assert_eq!(ranked[0].kinds(), "kalesh,automod,deleted");
        assert!(ranked[0].why().contains(" · "), "the reasons read as one line: {}", ranked[0].why());
    }

    /// The history each day is judged against comes out of a table of counts.
    #[test]
    fn a_members_fortnight_is_read_off_the_counts_table() {
        let days: Vec<String> = (1..=5).map(|d| format!("2026-09-{:02}", 20 - d)).collect();
        assert_eq!(days, vec!["2026-09-19", "2026-09-18", "2026-09-17", "2026-09-16", "2026-09-15"]);
        let counts = vec![
            (11u64, "2026-09-19".to_string(), 40i64),
            (11, "2026-09-19".to_string(), 10), // two channels on one day add up
            (11, "2026-09-17".to_string(), 30),
            (22, "2026-09-18".to_string(), 5),
        ];
        let h = history(&counts, &days);
        assert_eq!(h[&11], vec![50, 0, 30, 0, 0], "newest day first, a silent day is a nought not a gap");
        assert_eq!(h[&22], vec![0, 5, 0, 0, 0]);
        assert!(!h.contains_key(&99));
    }

    /// The fortnight a day is judged against is the fortnight before it.
    #[test]
    fn the_days_a_day_is_judged_against_are_the_ones_before_it() {
        let before = days_before("2026-03-02", 4);
        assert_eq!(before, vec!["2026-03-01", "2026-02-28", "2026-02-27", "2026-02-26"], "newest first, and it counts back over a month end");
        assert_eq!(days_before("2026-03-02", BASELINE_DAYS).len(), BASELINE_DAYS);
        assert!(days_before("not a day", 4).is_empty());
    }

    /// A scan is done once a night. A restart in the morning does nothing.
    #[tokio::test]
    async fn the_same_night_scanned_again_is_a_no_op() {
        use super::super::topics_store::{self as runs, KIND_SCAN};
        runs::open_for_tests();
        let day = "2026-02-12";
        let now = super::super::points::ist_day_start("2026-02-13").unwrap() + 6 * 3600;
        // No stats store in a test, so nobody has a day to read — but the night
        // is still claimed, worked and finished, which is the part that has to
        // happen once.
        let first = run_night(day.into(), now, Cache::default()).await.expect("the first run took the night");
        assert_eq!((first.looked_at, first.flagged, first.dived, first.failed), (0, 0, 0, 0));
        let db = runs::db().expect("the test store");
        assert!(!runs::run_for(&db.lock(), KIND_SCAN, day).unwrap().expect("a row").unfinished());
        assert!(run_night(day.into(), now + 60, Cache::default()).await.is_none(), "a restart a minute later does nothing");
        assert_eq!(runs::runs(&db.lock(), KIND_SCAN, 50).unwrap().iter().filter(|r| r.day == day).count(), 1, "one row, not two");
    }

    #[test]
    fn the_settings_have_the_defaults_they_are_documented_with() {
        assert!(watch_on(), "the nightly scan is on by default");
        assert_eq!(watch_hour(), 6);
        assert_eq!(watch_max(), 12);
        let t = thresholds();
        assert_eq!((t.automod, t.deleted, t.quiet), (3, 8, 25));
        assert_eq!((t.new_days, t.new_messages), (14, 30));
        assert_eq!(t.cap, 12, "five to twenty a night is what the page is for");
    }
}
