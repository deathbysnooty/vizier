//! The Insights page: tidbits about who talks to whom, a pair's detail, a
//! member's connections, and rebuilding the counts from the bot's stored
//! history. Only metadata ever leaves the server: who, to whom, where, when.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::super::insights::{self, BackfillRules, Interaction, Kind, Pair, StoredRequest};
use super::{ApiError, ApiResult, Panel, ok, parse_id};

const IST: i64 = 5 * 3600 + 30 * 60;
pub const LIST: usize = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Period {
    Today,
    Week,
    Month,
    All,
}

impl Period {
    pub fn parse(raw: Option<&str>) -> Result<Period, ApiError> {
        match raw.unwrap_or("7d") {
            "today" => Ok(Period::Today),
            "7d" => Ok(Period::Week),
            "30d" => Ok(Period::Month),
            "all" => Ok(Period::All),
            _ => Err(ApiError::bad("Period is today, 7d, 30d or all.")),
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Period::Today => "today",
            Period::Week => "7d",
            Period::Month => "30d",
            Period::All => "all",
        }
    }

    /// Where the period starts, at `now`: India midnight for today.
    pub fn since(self, now: i64) -> i64 {
        match self {
            Period::Today => (now + IST).div_euclid(86_400) * 86_400 - IST,
            Period::Week => now - 7 * 86_400,
            Period::Month => now - 30 * 86_400,
            Period::All => 0,
        }
    }
}

#[derive(Deserialize)]
pub struct PeriodQuery {
    #[serde(default)]
    period: Option<String>,
}

fn person(panel: &Panel, id: u64) -> Value {
    match panel.data.cached_member(id) {
        Some(m) => json!({ "id": id.to_string(), "name": m.name, "avatar": m.avatar }),
        None => json!({ "id": id.to_string(), "name": Value::Null, "avatar": Value::Null }),
    }
}

fn is_bot(panel: &Panel, id: u64) -> bool {
    panel.data.cached_member(id).is_some_and(|m| m.bot)
}

fn channel_name(panel: &Panel, id: u64) -> Option<String> {
    panel.data.channels().into_iter().find(|c| c.id == id.to_string()).map(|c| c.name)
}

/// The period's rows with bots taken out.
fn rows(panel: &Panel, since: i64, now: i64) -> Vec<Interaction> {
    let mut cache: HashMap<u64, bool> = HashMap::new();
    let mut bot = |id: u64| *cache.entry(id).or_insert_with(|| is_bot(panel, id));
    insights::rows_between(since, now + 1).into_iter().filter(|r| !bot(r.from_user) && !bot(r.to_user)).collect()
}

fn run_json(panel: &Panel, run: &insights::Run, pair: (u64, u64)) -> Value {
    let other = if run.starter == pair.0 { pair.1 } else { pair.0 };
    json!({
        "len": run.len,
        "start_ts": run.start_ts,
        "end_ts": run.end_ts,
        "minutes": (run.end_ts - run.start_ts + 59) / 60,
        "channel": channel_name(panel, run.channel_id),
        "starter": person(panel, run.starter),
        "other": person(panel, other),
    })
}

fn pair_json(panel: &Panel, p: &Pair) -> Value {
    json!({
        "a": person(panel, p.a),
        "b": person(panel, p.b),
        "a_to_b": p.a_to_b,
        "b_to_a": p.b_to_a,
        "replies": p.replies(),
        "balance": p.balance(),
        "mentions_a_to_b": p.mentions_a_to_b,
        "mentions_b_to_a": p.mentions_b_to_a,
        "longest": if p.longest.len > 1 { run_json(panel, &p.longest, (p.a, p.b)) } else { Value::Null },
        "last_ts": p.last_ts,
        "top_channel": p.top_channel.map(|(c, n)| json!({ "name": channel_name(panel, c), "replies": n })),
    })
}

/// Tallies per member of one kind, as (member, count, distinct others).
fn tally(rows: &[Interaction], kind: Kind, by_sender: bool) -> Vec<(u64, usize, usize)> {
    let mut counts: HashMap<u64, (usize, HashSet<u64>)> = HashMap::new();
    for r in rows.iter().filter(|r| r.kind == kind) {
        let (me, other) = if by_sender { (r.from_user, r.to_user) } else { (r.to_user, r.from_user) };
        let e = counts.entry(me).or_default();
        e.0 += 1;
        e.1.insert(other);
    }
    let mut out: Vec<(u64, usize, usize)> = counts.into_iter().map(|(id, (n, set))| (id, n, set.len())).collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.cmp(&b.0)));
    out.truncate(LIST);
    out
}

/// Head-to-head records from (winner, loser, ts) duels, per pair.
pub fn rivalries(duels: &[(u64, u64, i64)]) -> Vec<(u64, u64, usize, usize, i64)> {
    let mut by_pair: HashMap<(u64, u64), (usize, usize, i64)> = HashMap::new();
    for (w, l, ts) in duels {
        if w == l {
            continue;
        }
        let k = insights::key(*w, *l);
        let e = by_pair.entry(k).or_insert((0, 0, 0));
        if *w == k.0 { e.0 += 1 } else { e.1 += 1 }
        e.2 = e.2.max(*ts);
    }
    let mut out: Vec<(u64, u64, usize, usize, i64)> = by_pair.into_iter().map(|((a, b), (wa, wb, ts))| (a, b, wa, wb, ts)).collect();
    out.sort_by(|x, y| (y.2 + y.3).cmp(&(x.2 + x.3)).then(y.4.cmp(&x.4)));
    out
}

pub async fn overview(State(panel): State<Panel>, Query(q): Query<PeriodQuery>) -> ApiResult {
    let period = Period::parse(q.period.as_deref())?;
    let now = chrono::Utc::now().timestamp();
    static CACHE: LazyLock<Mutex<HashMap<Period, (i64, Value)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
    if !cfg!(test) {
        if let Some((at, v)) = CACHE.lock().get(&period) {
            if now - at < 60 {
                return ok(v.clone());
            }
        }
    }
    let since = period.since(now);
    let rows = rows(&panel, since, now);
    let pairs = insights::pairs(&rows);
    let mut duos: Vec<&Pair> = pairs.values().filter(|p| p.replies() > 0).collect();
    duos.sort_by(|a, b| b.replies().cmp(&a.replies()).then(b.longest.len.cmp(&a.longest.len)));
    let longest = pairs.values().filter(|p| p.longest.len > 1).max_by_key(|p| (p.longest.len, std::cmp::Reverse(p.longest.start_ts)));
    let mut one_sided: Vec<(u64, u64, usize, usize)> = pairs.values().filter_map(insights::one_sided).collect();
    one_sided.sort_by(|a, b| b.2.cmp(&a.2));
    let people = |list: Vec<(u64, usize, usize)>| list.iter().map(|(id, n, d)| json!({ "member": person(&panel, *id), "count": n, "people": d })).collect::<Vec<_>>();

    let duels: Vec<(u64, u64, i64)> = panel.data.duels(since).into_iter().filter(|(w, l, _)| !is_bot(&panel, *w) && !is_bot(&panel, *l)).collect();
    let arena: Vec<Value> = rivalries(&duels)
        .into_iter()
        .take(LIST)
        .map(|(a, b, wa, wb, ts)| json!({ "a": person(&panel, a), "b": person(&panel, b), "a_wins": wa, "b_wins": wb, "fights": wa + wb, "last_ts": ts }))
        .collect();

    let first_day = super::super::super::points::ist_day(since.max(now - 400 * 86_400));
    let hours: Vec<(u64, i64, i64, i64)> = panel.data.hour_counts(&first_day).into_iter().filter(|(u, _, _, t)| *t >= 100 && !is_bot(&panel, *u)).collect();
    let share_list = |pick: fn(&(u64, i64, i64, i64)) -> i64| {
        let mut list: Vec<(u64, f64, i64, i64)> = hours.iter().map(|h| (h.0, pick(h) as f64 / h.3 as f64, pick(h), h.3)).filter(|x| x.2 > 0).collect();
        list.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        list.into_iter()
            .take(10)
            .map(|(u, share, n, total)| json!({ "member": person(&panel, u), "share": share, "messages": n, "total": total }))
            .collect::<Vec<_>>()
    };

    let firsts = insights::first_replies();
    let mut new_pairs: Vec<&Pair> = if period == Period::All {
        Vec::new()
    } else {
        pairs.values().filter(|p| p.replies() >= 2 && firsts.get(&(p.a, p.b)).is_some_and(|ts| *ts >= since)).collect()
    };
    new_pairs.sort_by(|a, b| b.replies().cmp(&a.replies()));
    let (earliest, live_since) = insights::coverage();

    let magnets = {
        let mut list = tally(&rows, Kind::Reply, false);
        list.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));
        list
    };
    let value = json!({
        "period": period.key(),
        "since": since,
        "now": now,
        "counts": {
            "replies": rows.iter().filter(|r| r.kind == Kind::Reply).count(),
            "mentions": rows.iter().filter(|r| r.kind == Kind::Mention).count(),
            "pairs": pairs.len(),
            "people": rows.iter().flat_map(|r| [r.from_user, r.to_user]).collect::<HashSet<_>>().len(),
        },
        "duos": duos.iter().take(LIST).map(|p| pair_json(&panel, p)).collect::<Vec<_>>(),
        "longest": longest.map(|p| json!({ "pair": pair_json(&panel, p), "run": run_json(&panel, &p.longest, (p.a, p.b)) })),
        "magnets": people(magnets),
        "replies_sent": people(tally(&rows, Kind::Reply, true)),
        "mentions_sent": people(tally(&rows, Kind::Mention, true)),
        "mentioned": people(tally(&rows, Kind::Mention, false)),
        "one_sided": one_sided.iter().take(LIST).map(|(f, t, n, back)| json!({ "from": person(&panel, *f), "to": person(&panel, *t), "replies": n, "back": back })).collect::<Vec<_>>(),
        "arena": arena,
        "night_owls": share_list(|h| h.1),
        "early_birds": share_list(|h| h.2),
        "new_connections": new_pairs.iter().take(LIST).map(|p| {
            let mut v = pair_json(&panel, p);
            v["first_ts"] = json!(firsts.get(&(p.a, p.b)));
            v
        }).collect::<Vec<_>>(),
        "coverage": {
            "earliest": earliest,
            "live_since": live_since,
            "backfilled_ts": insights::meta_get("backfill_done").and_then(|v| v.parse::<i64>().ok()),
            "retention_days": insights::RETENTION_DAYS,
        },
    });
    CACHE.lock().insert(period, (now, value.clone()));
    ok(value)
}

#[derive(Deserialize)]
pub struct PairQuery {
    a: String,
    b: String,
    #[serde(default)]
    period: Option<String>,
}

pub async fn pair(State(panel): State<Panel>, Query(q): Query<PairQuery>) -> ApiResult {
    let (Some(a), Some(b)) = (parse_id(&q.a), parse_id(&q.b)) else {
        return Err(ApiError::bad("Send two member ids as a and b."));
    };
    if a == b {
        return Err(ApiError::bad("Pick two different members."));
    }
    let period = Period::parse(q.period.as_deref())?;
    let now = chrono::Utc::now().timestamp();
    let since = period.since(now);
    let (a, b) = insights::key(a, b);
    let rows: Vec<Interaction> = rows(&panel, since, now).into_iter().filter(|r| insights::key(r.from_user, r.to_user) == (a, b)).collect();
    let pair = insights::pairs(&rows).remove(&(a, b));
    let mut days: std::collections::BTreeMap<String, (usize, usize)> = std::collections::BTreeMap::new();
    let mut channels: HashMap<u64, usize> = HashMap::new();
    for r in rows.iter().filter(|r| r.kind == Kind::Reply) {
        let e = days.entry(super::super::super::points::ist_day(r.ts)).or_insert((0, 0));
        if r.from_user == a { e.0 += 1 } else { e.1 += 1 }
        *channels.entry(r.channel_id).or_insert(0) += 1;
    }
    let mut channels: Vec<(u64, usize)> = channels.into_iter().collect();
    channels.sort_by(|x, y| y.1.cmp(&x.1));
    let recent: Vec<Value> = rows
        .iter()
        .rev()
        .take(20)
        .map(|r| json!({ "ts": r.ts, "channel": channel_name(&panel, r.channel_id), "from": r.from_user.to_string(), "to": r.to_user.to_string(), "kind": r.kind }))
        .collect();
    let duels: Vec<(u64, u64, i64)> = panel.data.duels(since).into_iter().filter(|(w, l, _)| insights::key(*w, *l) == (a, b)).collect();
    let (a_wins, b_wins) = duels.iter().fold((0, 0), |acc, (w, _, _)| if *w == a { (acc.0 + 1, acc.1) } else { (acc.0, acc.1 + 1) });
    ok(json!({
        "period": period.key(),
        "a": person(&panel, a),
        "b": person(&panel, b),
        "summary": pair.as_ref().map(|p| pair_json(&panel, p)),
        "days": days.into_iter().map(|(d, (x, y))| json!({ "day": d, "a_to_b": x, "b_to_a": y })).collect::<Vec<_>>(),
        "channels": channels.iter().take(10).map(|(c, n)| json!({ "name": channel_name(&panel, *c), "replies": n })).collect::<Vec<_>>(),
        "recent": recent,
        "arena": { "a_wins": a_wins, "b_wins": b_wins },
    }))
}

pub async fn connections(State(panel): State<Panel>, Path(id): Path<String>, Query(q): Query<PeriodQuery>) -> ApiResult {
    let id = parse_id(&id).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
    let period = Period::parse(q.period.as_deref())?;
    let now = chrono::Utc::now().timestamp();
    let since = period.since(now);
    let rows: Vec<Interaction> = rows(&panel, since, now).into_iter().filter(|r| r.from_user == id || r.to_user == id).collect();
    let pairs = insights::pairs(&rows);
    let duels = rivalries(&panel.data.duels(since).into_iter().filter(|(w, l, _)| *w == id || *l == id).collect::<Vec<_>>());
    let mut list: Vec<&Pair> = pairs.values().collect();
    let weight = |p: &Pair| p.replies() * 2 + p.mentions_a_to_b + p.mentions_b_to_a;
    list.sort_by(|x, y| weight(y).cmp(&weight(x)).then(y.longest.len.cmp(&x.longest.len)));
    let partners: Vec<Value> = list
        .iter()
        .take(10)
        .map(|p| {
            let me_a = p.a == id;
            let other = if me_a { p.b } else { p.a };
            let h2h = duels.iter().find(|(a, b, ..)| insights::key(*a, *b) == insights::key(id, other));
            let (my_wins, their_wins) = match h2h {
                Some((a, _, wa, wb, _)) => if *a == id { (*wa, *wb) } else { (*wb, *wa) },
                None => (0, 0),
            };
            json!({
                "member": person(&panel, other),
                "to_them": if me_a { p.a_to_b } else { p.b_to_a },
                "from_them": if me_a { p.b_to_a } else { p.a_to_b },
                "mentions_to_them": if me_a { p.mentions_a_to_b } else { p.mentions_b_to_a },
                "mentions_from_them": if me_a { p.mentions_b_to_a } else { p.mentions_a_to_b },
                "longest": if p.longest.len > 1 { run_json(&panel, &p.longest, (p.a, p.b)) } else { Value::Null },
                "arena": { "wins": my_wins, "losses": their_wins },
                "last_ts": p.last_ts,
            })
        })
        .collect();
    ok(json!({
        "period": period.key(),
        "member": person(&panel, id),
        "sent": rows.iter().filter(|r| r.from_user == id && r.kind == Kind::Reply).count(),
        "received": rows.iter().filter(|r| r.to_user == id && r.kind == Kind::Reply).count(),
        "partners": partners,
    }))
}

// --- rebuilding from history -------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct Rebuild {
    pub running: bool,
    pub started_ts: i64,
    pub finished_ts: Option<i64>,
    pub scanned: usize,
    pub found: usize,
    pub added: usize,
    pub error: Option<String>,
}

static REBUILD: LazyLock<Mutex<Rebuild>> = LazyLock::new(|| Mutex::new(Rebuild::default()));

/// Starts filling the counts in from the bot's stored history, unless one is
/// already running. Rows from an earlier fill are replaced; live rows stay.
pub fn start_rebuild(panel: Panel) -> bool {
    {
        let mut state = REBUILD.lock();
        if state.running {
            return false;
        }
        *state = Rebuild { running: true, started_ts: chrono::Utc::now().timestamp(), ..Default::default() };
    }
    tokio::spawn(async move {
        let now = chrono::Utc::now().timestamp();
        let since = now - insights::RETENTION_DAYS * 86_400;
        let progress: Arc<dyn Fn(usize) + Send + Sync> = Arc::new(|n| REBUILD.lock().scanned = n);
        let result = match panel.data.history_requests(since, progress).await {
            Ok(requests) => {
                let rows = {
                    let known: HashSet<u64> = panel.data.channels().iter().filter_map(|c| c.id.parse().ok()).collect();
                    let bot_id = panel.data.bot_id();
                    let rules = BackfillRules {
                        known_channel: &|c| known.contains(&c),
                        parent_of: &|c| panel.data.thread_parent(c),
                        is_bot: &|u| bot_id == Some(u) || is_bot(&panel, u),
                        sensitive: panel.data.sensitive_channels(),
                    };
                    insights::resolve(&requests, &rules)
                };
                REBUILD.lock().found = rows.len();
                let rows_for_insert = rows.clone();
                tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
                    insights::delete_history_rows()?;
                    insights::insert(&rows_for_insert)
                })
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))
                .and_then(|r| r)
            }
            Err(e) => Err(e),
        };
        let mut state = REBUILD.lock();
        state.running = false;
        state.finished_ts = Some(chrono::Utc::now().timestamp());
        match result {
            Ok(added) => {
                state.added = added;
                insights::meta_set("backfill_done", &now.to_string());
                tracing::info!("insights: filled in {} rows from {} stored messages", added, state.scanned);
            }
            Err(e) => {
                tracing::warn!("insights: filling in from history failed: {}", e);
                state.error = Some(e.to_string());
            }
        }
    });
    true
}

pub async fn rebuild(State(panel): State<Panel>) -> ApiResult {
    if !start_rebuild(panel) {
        return Err(ApiError(StatusCode::CONFLICT, "Already filling in from history.".into()));
    }
    Ok((StatusCode::ACCEPTED, axum::Json(json!({ "rebuild": REBUILD.lock().clone() }))).into_response())
}

pub async fn rebuild_status() -> ApiResult {
    ok(json!({ "rebuild": REBUILD.lock().clone() }))
}

/// Reads the stored requests for a backfill from the live history database.
pub async fn read_live_history(
    deps: &crate::dependencies::VizierDependencies,
    agent_id: &str,
    since: i64,
    progress: Arc<dyn Fn(usize) + Send + Sync>,
) -> anyhow::Result<Vec<StoredRequest>> {
    let conn = super::members::history_conn(deps).ok_or_else(|| anyhow::anyhow!("the history database isn't available"))?;
    let agent = agent_id.to_string();
    tokio::task::spawn_blocking(move || {
        insights::read_history(&conn.lock(), &agent, since, Duration::from_millis(40), &|n| progress(n)).map_err(anyhow::Error::from)
    })
    .await
    .map_err(|e| anyhow::anyhow!(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_start_where_they_should() {
        let now = 1_789_331_400; // 02:00 IST
        assert_eq!(super::super::super::super::points::ist_day(Period::Today.since(now)), "2026-09-14");
        assert_eq!(Period::Today.since(now) % 60, 0);
        assert_eq!(Period::Week.since(now), now - 7 * 86_400);
        assert_eq!(Period::Month.since(now), now - 30 * 86_400);
        assert_eq!(Period::All.since(now), 0);
        assert!(Period::parse(Some("year")).is_err());
    }

    #[test]
    fn rivalries_count_head_to_head() {
        let r = rivalries(&[(1, 2, 10), (2, 1, 20), (1, 2, 30), (3, 4, 5), (5, 5, 1)]);
        assert_eq!(r[0], (1, 2, 2, 1, 30));
        assert_eq!(r.len(), 2);
    }
}
