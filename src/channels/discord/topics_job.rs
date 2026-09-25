//! The nightly run of the topic pass: reading yesterday out of the message log,
//! putting it through the model channel by channel, and writing down what came
//! back.
//!
//! The shape is `ship_sheet`'s, for the same reasons. It wakes every half hour,
//! does nothing at all unless the India hour has come round, and **claims the
//! night in the store before it spends a token** — so a restart in the middle of
//! a pass cannot make the bot read the same day twice, and running it again once
//! it has finished does nothing whatever.
//!
//! The work is spread: a breath between chunks, and every read off the database
//! on a blocking thread. Twenty thousand messages is a lot to walk through, and
//! the bot has a server to answer while it does.
//!
//! Nothing is written until the whole day has been read. A chunk the model
//! refuses is counted and stepped over; a night where nothing at all came back
//! gives its claim up again with the reason on the row, so the page says what
//! happened rather than showing an empty day.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rusqlite::params;
use serenity::all::Context;

use super::topics::{self, Said};
use super::topics_store::{self as store, Done, KIND_TOPICS};

/// Messages one night reads out of the log at most. A day four times the size
/// of the busiest one on record is well inside it.
pub const DAY_ROWS: usize = 120_000;
/// Between chunks, so a pass never holds the bot up.
const BREATH: Duration = Duration::from_millis(400);
/// How often the job looks to see whether tonight's pass is due.
const CHECK_EVERY: Duration = Duration::from_secs(1800);
/// Nothing runs until the bot has been up a few minutes.
const SETTLE: Duration = Duration::from_secs(300);
/// Days of entries kept before the oldest are dropped.
const KEEP_DAYS: i64 = 400;

const DAY: i64 = 86_400;

/// The India day a pass started at `now` should read: the last whole one.
pub fn target_day(now: i64) -> String {
    super::points::ist_day(now - DAY)
}

/// Whether the India hour has come round. Whether the night has already been
/// done is the store's business, not this one's — that is what the claim is for.
pub fn due(hour_now: u32, wanted: u32) -> bool {
    hour_now == wanted
}

// --- reading the day ----------------------------------------------------------------------------

/// One India day's messages out of the log, oldest first, with the channels
/// that are never shown left out. Blocking.
///
/// The window is walked over `message_id` rather than `created_ts`: a Discord id
/// carries the moment it was made, so the primary key is already the time order
/// and no second index is touched.
fn read_day(from_ms: i64, to_ms: i64, names: &HashMap<u64, String>) -> Vec<Said> {
    let Some(reader) = super::msglog::reader() else { return Vec::new() };
    let never = super::msglog::never_logged();
    let (lo, hi) = (super::msglog::first_id_at(from_ms), super::msglog::first_id_at(to_ms));
    let conn = reader.conn.lock();
    let Ok(mut stmt) = conn.prepare_cached(
        "SELECT channel_id, parent_id, channel_name, author_id, author_name, content, created_ts
           FROM recent WHERE message_id >= ?1 AND message_id < ?2 ORDER BY message_id LIMIT ?3",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params![lo as i64, hi as i64, DAY_ROWS as i64], |r| {
        Ok((
            r.get::<_, i64>(0)? as u64,
            r.get::<_, Option<i64>>(1)?.map(|p| p as u64),
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)? as u64,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    });
    let Ok(rows) = rows else { return Vec::new() };
    rows.flatten()
        .filter_map(|(channel_id, parent_id, channel_name, author_id, author_name, content, created_ms)| {
            if author_id == 0 || content.trim().is_empty() {
                return None;
            }
            let place = super::msglog::Place { channel_id, parent_id, channel_name: channel_name.clone() };
            let parent_name = parent_id.and_then(|p| names.get(&p)).map(String::as_str);
            if super::msglog::excluded(&place, parent_name, &never) {
                return None;
            }
            // The channel's own name as the guild has it now, so a thread reads
            // as itself rather than as whatever it was called when it was kept.
            let channel_name = names.get(&channel_id).cloned().unwrap_or(channel_name);
            Some(Said { author_id, author_name, channel_id, channel_name, ts: created_ms / 1000, text: content })
        })
        .collect()
}

/// Every channel and thread the cache can name.
fn channel_names(ctx: &Context) -> HashMap<u64, String> {
    let mut out: HashMap<u64, String> = HashMap::new();
    for guild in ctx.cache.guilds() {
        let Some(g) = ctx.cache.guild(guild) else { continue };
        for (id, channel) in g.channels.iter() {
            out.insert(id.get(), channel.name.clone());
        }
        for thread in g.threads.iter() {
            out.insert(thread.id.get(), thread.name.clone());
        }
    }
    out
}

// --- one night ----------------------------------------------------------------------------------

static RUNNING: AtomicBool = AtomicBool::new(false);

/// One night's pass over one India day. Returns what was done, or nothing when
/// the night was already somebody else's.
///
/// The order matters and is the whole point: claim, read, ask, write, finish.
/// Nothing between the claim and the finish can leave the store holding half a
/// day, because the entries go in one transaction after every chunk has been
/// tried.
pub async fn run_night(day: String, now: i64, names: HashMap<u64, String>) -> Option<topics::Pass> {
    let db = store::db()?;
    if RUNNING.swap(true, Ordering::SeqCst) {
        return None;
    }
    let out = run_claimed(&day, now, names).await;
    RUNNING.store(false, Ordering::SeqCst);
    let _ = db;
    out
}

async fn run_claimed(day: &str, now: i64, names: HashMap<u64, String>) -> Option<topics::Pass> {
    let db = store::db()?;
    let claimed = { store::claim(&db.lock(), KIND_TOPICS, day, now).unwrap_or(false) };
    if !claimed {
        tracing::debug!("topics: {} is already done or being done", day);
        return None;
    }
    let from = super::points::ist_day_start(day)?;
    let (from_ms, to_ms) = (from * 1000, (from + DAY) * 1000);
    let read_names = names.clone();
    let day_rows = tokio::task::spawn_blocking(move || read_day(from_ms, to_ms, &read_names)).await.unwrap_or_default();
    if day_rows.is_empty() {
        tracing::info!("topics: nothing in the log for {}", day);
        let empty = topics::Pass::default();
        let _ = store::finish(&db.lock(), KIND_TOPICS, day, &Done { note: "nothing in the log for that day".into(), ..Done::from(&empty) }, now);
        return Some(empty);
    }

    let set = topics::settings();
    let model = topics::topics_model();
    let pass = topics::run_day(day, &day_rows, &set, |prompt| {
        let model = model.clone();
        async move {
            // A breath between chunks: it is nobody's hurry.
            tokio::time::sleep(BREATH).await;
            super::kalesh::ask_retrying("topics", || super::kalesh::ask_live_as(prompt.clone(), model.clone())).await
        }
    })
    .await;

    // Every chunk failed: nothing is written, the claim goes back, and the row
    // says why. A morning with no topics is better than a morning with wrong ones.
    if pass.chunks > 0 && pass.failed == pass.chunks {
        tracing::error!("topics: {} failed outright — all {} chunks", day, pass.chunks);
        let _ = store::release(&db.lock(), KIND_TOPICS, day, &format!("the model answered none of the {} chunks", pass.chunks));
        return Some(pass);
    }

    let entries = pass.entries.clone();
    let note = if pass.failed > 0 { format!("{} of {} chunks failed", pass.failed, pass.chunks) } else { String::new() };
    let written = {
        let mut conn = db.lock();
        match store::save(&mut conn, &entries, now) {
            Ok(n) => n,
            Err(err) => {
                tracing::error!("topics: {} was read but not written down: {}", day, err);
                let _ = store::release(&conn, KIND_TOPICS, day, &format!("the store would not take it: {}", err));
                return Some(pass);
            }
        }
    };
    {
        let conn = db.lock();
        let _ = store::finish(&conn, KIND_TOPICS, day, &Done { note, ..Done::from(&pass) }, now);
        let _ = store::trim(&conn, &super::points::ist_day(now - KEEP_DAYS * DAY));
    }
    tracing::info!(
        "topics: {} — {} members written down from {} messages in {} chunk(s), {} failed, {} too thin, {} + {} tokens on {}",
        day,
        written,
        day_rows.len(),
        pass.chunks,
        pass.failed,
        pass.too_thin,
        pass.input_tokens,
        pass.output_tokens,
        if pass.model.is_empty() { "the bot's model" } else { pass.model.as_str() }
    );
    Some(pass)
}

/// Checks every half hour whether tonight's pass is due. Starts once per process.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        loop {
            if topics::topics_on() && store::db().is_some() {
                let now = chrono::Utc::now().timestamp();
                if due(super::points::ist_hour(now) as u32, topics::run_hour()) {
                    run_night(target_day(now), now, channel_names(&ctx)).await;
                }
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pass reads the last whole India day, and only at the hour it is set to.
    #[test]
    fn it_reads_yesterday_at_the_hour_it_is_set_to() {
        // 21 Sep 2026, 05:30 IST.
        let now = super::super::points::ist_day_start("2026-09-21").unwrap() + 5 * 3600 + 1800;
        assert_eq!(target_day(now), "2026-09-20", "the last whole day, not the one going on");
        assert_eq!(super::super::points::ist_hour(now), 5);
        assert!(due(5, 5));
        assert!(!due(4, 5) && !due(6, 5));
        // Just after midnight it is still yesterday that is whole.
        let midnight = super::super::points::ist_day_start("2026-09-21").unwrap() + 60;
        assert_eq!(target_day(midnight), "2026-09-20");
    }

    /// A night is done once. The second run of the same night — a restart, a
    /// second bot, the loop coming round again half an hour later — does nothing
    /// at all rather than reading the day twice.
    #[tokio::test]
    async fn the_same_night_run_again_is_a_no_op() {
        store::open_for_tests();
        let now = super::super::points::ist_day_start("2026-02-11").unwrap() + 5 * 3600;
        let day = target_day(now);
        assert_eq!(day, "2026-02-10");
        // No message log in a test, so the day reads as empty — but it is still
        // claimed, worked and finished, which is the part that has to be once.
        let first = run_night(day.clone(), now, HashMap::new()).await;
        assert!(first.is_some(), "the first run took the night");
        let db = store::db().expect("the test store");
        let row = store::run_for(&db.lock(), KIND_TOPICS, &day).unwrap().expect("a row for the night");
        assert!(!row.unfinished(), "and finished it");
        assert_eq!(row.note, "nothing in the log for that day", "with the reason on the row, where the page can read it");

        assert!(run_night(day.clone(), now + 60, HashMap::new()).await.is_none(), "a restart a minute later does nothing");
        assert!(run_night(day.clone(), now + 86_400, HashMap::new()).await.is_none(), "and nor does tomorrow");
        assert_eq!(store::runs(&db.lock(), KIND_TOPICS, 50).unwrap().iter().filter(|r| r.day == day).count(), 1, "one row, not two");
    }

    /// With no message log the job reads an empty day rather than panicking.
    #[tokio::test]
    async fn no_message_log_means_an_empty_day_not_a_crash() {
        assert!(super::super::msglog::reader().is_none(), "the tests never open the real log");
        assert!(read_day(0, 1, &HashMap::new()).is_empty());
    }
}
