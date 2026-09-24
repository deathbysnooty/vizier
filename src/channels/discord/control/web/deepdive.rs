//! The Deep dive page: one member, one period, in one screen.
//!
//! A mod picks somebody — still here or long gone — and a period (3, 7, 14 or
//! 30 days, or two dates of their own), and gets everything the bot kept about
//! them in that window, read in one go rather than a page at a time: their
//! messages in time order with the channel on each, deleted ones back in place
//! and marked, the ones AutoMod stopped marked too, pictures as thumbnails;
//! the voice rooms they sat in, when, for how long and who was in there with
//! them; and the plain shape of their days — messages per day, busiest
//! channels and hours, house points, what was deleted or blocked.
//!
//! On a button, the model writes a few paragraphs saying what they have been up
//! to. That is the Kalesh summary plumbing — the same model setting, the same
//! three tries, the same stored answer so the same window is never paid for
//! twice — under its own instructions, in [`super::super::super::deepdive`].
//!
//! Everything comes from `msglog`, which never kept a word of #safe-corner, and
//! the never-shown list is applied on top on the way out. Admins only, and
//! every dive is one line in the activity log naming who looked at whom.

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::deepdive::{self as dive, Period, VoiceSession, VoiceStay};
use super::super::super::kalesh::{self as k, summary_max};
use super::super::super::kalesh_store::{self as store, Summary};
use super::super::super::msglog::{self, Place, SaidFilter, SaidRow};
use super::super::super::points;
use super::kalesh::{Running, ask_model, why_failed};
use super::msglog::{channel_json, house_json, houses_for, member_json, never_shown, unavailable};
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

/// Tries at the model for one summary. The Kalesh page's number, because it is
/// the same call to the same provider.
pub use super::kalesh::TRIES;

const DAY: i64 = 86_400;

fn store_db() -> Result<&'static parking_lot::Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The summary store isn't open. Restart the bot.".into()))
}

fn db_error(err: rusqlite::Error) -> ApiError {
    ApiError::internal(format!("summary store: {}", err))
}

// --- what was asked for ----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct DiveQuery {
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    days: Option<String>,
    /// A range of one's own, unix seconds, instead of a period.
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

#[derive(Clone, Debug)]
struct Asked {
    member: u64,
    days_key: String,
    days: i64,
    /// Two dates of the mod's own, which are never moved for somebody who left.
    custom: Option<(i64, i64)>,
}

fn blank(v: &Option<String>) -> Option<String> {
    v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from)
}

fn read_query(q: &DiveQuery) -> Result<Asked, ApiError> {
    let raw = blank(&q.member).ok_or_else(|| ApiError::bad("Pick a member to look at."))?;
    let member = parse_id(&raw).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
    let number = |v: &Option<String>| blank(v).and_then(|r| r.parse::<i64>().ok()).filter(|n| *n > 0);
    let custom = match (number(&q.from), number(&q.to)) {
        (Some(from), Some(to)) if to > from => {
            if to - from > dive::MAX_RANGE_DAYS * DAY {
                return Err(ApiError::bad(format!("A range of your own covers at most {} days.", dive::MAX_RANGE_DAYS)));
            }
            Some((from, to))
        }
        (Some(_), Some(_)) => return Err(ApiError::bad("The range ends before it starts.")),
        (None, None) => None,
        _ => return Err(ApiError::bad("A range of your own needs both a start and an end.")),
    };
    let days_key = blank(&q.days).unwrap_or_else(|| dive::DEFAULT_DAYS.to_string());
    let days = match custom {
        Some((from, to)) => ((to - from) + DAY - 1) / DAY,
        None => dive::PERIODS
            .iter()
            .find(|(k, _)| *k == days_key)
            .map(|(_, d)| *d)
            .ok_or_else(|| ApiError::bad("The period is 3, 7, 14 or 30 days, or a range of your own."))?,
    };
    Ok(Asked { member, days_key, days, custom })
}

// --- loading one dive ------------------------------------------------------------------------

struct Loaded {
    asked: Asked,
    period: Period,
    /// Their own kept messages in the window, oldest first, never a channel
    /// that is never shown.
    rows: Vec<SaidRow>,
    /// True when the window held more than [`dive::MAX_ROWS`] and was cut.
    cut: bool,
    name: String,
    in_server: bool,
    /// When they went, if the join log says they are gone now.
    gone_at: Option<i64>,
}

impl Loaded {
    fn refs(&self) -> Vec<&SaidRow> {
        self.rows.iter().collect()
    }

    fn label(&self, custom: bool) -> String {
        format!("@{} · {}", self.name, dive::period_words(&self.period, custom))
    }
}

/// The last moment the bot has a kept message from them, for placing the window
/// of somebody who is gone. One row, off the (author, message) index.
async fn last_kept(panel: &Panel, member: u64) -> Option<i64> {
    let filter = SaidFilter {
        member: Some(member),
        channel: None,
        since_id: 0,
        before: None,
        q: None,
        matches: |_, _| true,
        limit: 1,
        budget: msglog::SAID_BUDGET,
        sensitive: never_shown(panel),
    };
    // A second past it, so the window's own end never cuts off the very
    // message it was placed by.
    panel.data.msglog_said(filter).await.ok()?.rows.first().map(|r| r.created_ms / 1000 + 1)
}

async fn load(panel: &Panel, asked: Asked, now: i64) -> Result<Loaded, ApiError> {
    let member = asked.member;
    let live = panel.data.cached_member(member).or(panel.data.member(member).await);
    let in_server = live.is_some();
    let name = match &live {
        Some(m) => m.name.clone(),
        None => super::members::name_or_id(panel, member).await,
    };
    // Someone the bot has never seen at all has nothing to dive into.
    let known = live.is_some() || super::members::known_one(panel, member).await.is_some();
    if !known {
        return Err(ApiError::not_found("Nobody here has ever seen that member."));
    }
    let gone_at = super::left::logs(panel).await.iter().find(|r| r.id == member).and_then(super::left::gone_at);
    let period = match asked.custom {
        Some((from, to)) => Period { from, to, shifted: false },
        None => {
            let last_seen = if in_server && gone_at.is_none() { None } else { last_kept(panel, member).await };
            // Somebody gone whom the join log never caught leaving is still
            // gone: their last kept message stands in for the date.
            let gone = gone_at.or(if in_server { None } else { last_seen });
            dive::period(now, asked.days, gone, last_seen)
        }
    };
    let mut rows = panel.data.deepdive_messages(member, period.from * 1000, period.to * 1000).await.map_err(unavailable)?;
    // The log never kept a word of #safe-corner, but the skip list can grow
    // after a message was kept, so what is never shown is dropped here too —
    // threads inside it included.
    let sensitive = never_shown(panel);
    let listed = panel.data.channels();
    rows.retain(|r| {
        let parent_name = r.parent_id.and_then(|p| listed.iter().find(|c| c.id == p.to_string()).map(|c| c.name.clone()));
        let place = Place { channel_id: r.channel_id, parent_id: r.parent_id, channel_name: r.channel_name.clone() };
        !msglog::excluded(&place, parent_name.as_deref(), &sensitive)
    });
    rows.sort_by_key(|r| r.message_id);
    rows.dedup_by_key(|r| r.message_id);
    let cut = rows.len() > dive::MAX_ROWS;
    if cut {
        // Keep the end: the newest of a very long window is what a mod is after.
        rows.drain(..rows.len() - dive::MAX_ROWS);
    }
    Ok(Loaded { asked, period, rows, cut, name, in_server, gone_at })
}

// --- what the page is handed ------------------------------------------------------------------

fn message_json(panel: &Panel, n: usize, r: &SaidRow, guild: Option<&String>, channel: Value, house: Value) -> Value {
    json!({
        "n": n,
        "id": r.message_id.to_string(),
        "ts": r.created_ms / 1000,
        "ts_ms": r.created_ms,
        "member": member_json(panel, r.author_id, &r.author_name, &r.avatar),
        "house": house,
        "channel": channel,
        "text": r.content,
        "reply_to": r.reply_to.map(|id| json!({ "id": id.to_string(), "author": r.reply_author, "text": r.reply_text })),
        // The Kalesh page already serves the log's pictures; this is the same store.
        "images": r.images.iter().map(|f| json!({ "n": f.n, "name": f.name, "url": format!("/api/kalesh/picture/{}/{}", r.message_id, f.n) })).collect::<Vec<_>>(),
        "stickers": r.attachments.iter().filter(|a| msglog::is_sticker(a)).map(|a| json!({ "name": a.filename, "url": msglog::sticker_url(a) })).collect::<Vec<_>>(),
        "files": r.attachments.iter().filter(|a| !msglog::is_sticker(a) && !r.images.iter().any(|f| f.name == a.filename)).map(|a| json!({
            "name": a.filename,
            "image": msglog::image_ext(a.content_type.as_deref(), &a.filename).is_some(),
        })).collect::<Vec<_>>(),
        // Deleted or blocked: it isn't in Discord, so there is nothing to jump to.
        "url": if r.gone.is_some() { None } else { guild.map(|g| search::jump_url(g, r.channel_id, r.message_id)) },
        "gone": r.gone,
    })
}

/// Their voice sessions in the window, with the company each one had named.
async fn voice_json(panel: &Panel, member: u64, p: &Period, now: i64) -> Value {
    let stays: Vec<VoiceStay> = panel.data.voice_stays(p.from, p.to, now);
    let sessions = dive::sessions(&stays, member, p.from, p.to);
    let channels = panel.data.channels();
    let room = |id: u64| channels.iter().find(|c| c.id == id.to_string()).map(|c| c.name.clone());
    let total: i64 = sessions.iter().map(|s| s.secs).sum();
    let mut partners: HashMap<u64, i64> = HashMap::new();
    for s in &sessions {
        for (id, secs) in &s.with {
            *partners.entry(*id).or_insert(0) += secs;
        }
    }
    let mut top: Vec<(u64, i64)> = partners.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(10);
    let who = |id: u64| match panel.data.cached_member(id) {
        Some(m) => json!({ "id": id.to_string(), "name": m.name, "avatar": m.avatar, "in_server": true }),
        None => json!({ "id": id.to_string(), "name": id.to_string(), "avatar": Value::Null, "in_server": false }),
    };
    json!({
        "total_secs": total,
        "total_words": dive::spell_secs(total),
        "sessions": sessions.iter().map(|s| json!({
            "channel": { "id": s.channel_id.to_string(), "name": room(s.channel_id) },
            "start": s.start,
            "end": s.end,
            "secs": s.secs,
            "words": dive::spell_secs(s.secs),
            "with": s.with.iter().map(|(id, secs)| {
                let mut v = who(*id);
                v["secs"] = json!(secs);
                v
            }).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "partners": top.iter().map(|(id, secs)| {
            let mut v = who(*id);
            v["secs"] = json!(secs);
            v["words"] = json!(dive::spell_secs(*secs));
            v
        }).collect::<Vec<_>>(),
    })
}

async fn shape_json(panel: &Panel, loaded: &Loaded) -> Value {
    let p = &loaded.period;
    let shape = dive::shape(&loaded.refs(), p.from, p.to, points::ist_day, points::ist_hour);
    let channels = panel.data.channels();
    let name = |id: u64| channels.iter().find(|c| c.id == id.to_string()).map(|c| c.name.clone());
    let sources = panel.data.points_between(loaded.asked.member, p.from, p.to).await;
    json!({
        "messages": shape.messages,
        "deleted": shape.deleted,
        "blocked": shape.blocked,
        "avg_len": shape.avg_len,
        "per_day": shape.per_day.iter().map(|(d, n)| json!({ "day": d, "messages": n })).collect::<Vec<_>>(),
        "channels": shape.channels.iter().take(8).map(|(c, n)| json!({ "id": c.to_string(), "name": name(*c), "messages": n })).collect::<Vec<_>>(),
        "hours": shape.hours,
        "points": sources.iter().map(|(k, n)| json!({ "source": k, "points": n })).collect::<Vec<_>>(),
        "points_total": sources.iter().map(|(_, n)| n).sum::<i64>(),
        "ship_sheet": super::members::ship_sheet_of(loaded.asked.member),
    })
}

fn summary_json(s: &Summary) -> Value {
    json!({
        "id": s.id,
        "summary": s.new.summary,
        "raw": s.new.summary.is_none().then(|| s.new.raw.clone()),
        "messages": s.new.message_ids.len(),
        "sent": s.new.sent_count,
        "trimmed": s.new.trimmed,
        "run_by": s.new.run_by.to_string(),
        "run_ts": s.new.run_ts,
        "model": s.new.model,
        "input_tokens": s.new.input_tokens,
        "output_tokens": s.new.output_tokens,
        "message_ids": s.new.message_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
    })
}

/// The note the Kalesh page carries, in the same words: a summary is a reading
/// of the messages, never a substitute for them.
pub const READ_FIRST: &str =
    "This is the model's reading of the messages, not evidence. Read the messages above before you act on anything here.";

async fn envelope(panel: &Panel, loaded: &Loaded, now: i64) -> Result<Value, ApiError> {
    let sensitive = never_shown(panel);
    let listed = panel.data.channels();
    let guild = panel.data.guild().map(|g| g.id);
    let houses = houses_for(panel, loaded.rows.iter().map(|r| r.author_id).collect()).await;
    let house = house_json(houses.get(&loaded.asked.member));
    let messages: Vec<Value> = loaded
        .rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let place = Place { channel_id: r.channel_id, parent_id: r.parent_id, channel_name: r.channel_name.clone() };
            let channel = channel_json(&place, &listed, &sensitive).unwrap_or(Value::Null);
            message_json(panel, i + 1, r, guild.as_ref(), channel, house.clone())
        })
        .collect();
    let custom = loaded.asked.custom.is_some();
    let stored = {
        let key = dive::key(loaded.asked.member, loaded.asked.days, &loaded.refs());
        store::summary_for(&store_db()?.lock(), &key).map_err(db_error)?
    };
    let avatar = panel.data.cached_member(loaded.asked.member).map(|m| m.avatar);
    Ok(json!({
        "member": {
            "id": loaded.asked.member.to_string(),
            "name": loaded.name,
            "avatar": avatar,
            "in_server": loaded.in_server,
            "left_ts": loaded.gone_at,
            "house": house,
        },
        "period": {
            "days": loaded.asked.days_key,
            "from": loaded.period.from,
            "to": loaded.period.to,
            "custom": custom,
            // True when the window was moved back to their last days here.
            "shifted": loaded.period.shifted,
            "words": dive::period_words(&loaded.period, custom),
        },
        "messages": messages,
        "count": messages.len(),
        "cut": loaded.cut,
        "max_rows": dive::MAX_ROWS,
        "voice": voice_json(panel, loaded.asked.member, &loaded.period, now).await,
        "shape": shape_json(panel, loaded).await,
        "summary": stored.as_ref().map(summary_json),
        "read_first": READ_FIRST,
        "text_days": msglog::text_days(),
    }))
}

// --- the endpoints ------------------------------------------------------------------------------

pub async fn dive(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<DiveQuery>) -> ApiResult {
    let asked = read_query(&q)?;
    let now = chrono::Utc::now().timestamp();
    let loaded = load(&panel, asked, now).await?;
    search::log_quietly("deepdive:look", user, &loaded.label(loaded.asked.custom.is_some()));
    ok(envelope(&panel, &loaded, now).await?)
}

#[derive(Deserialize)]
pub struct SummariseBody {
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    days: Option<String>,
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
}

pub async fn summarise(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let body: SummariseBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Pick a member and a period."))?;
    let asked = read_query(&DiveQuery { member: body.member, days: body.days, from: body.from, to: body.to })?;
    let custom = asked.custom.is_some();
    let now = chrono::Utc::now().timestamp();
    let loaded = load(&panel, asked, now).await?;
    let rows = loaded.refs();
    if rows.is_empty() {
        return Err(ApiError::bad("Nothing of theirs is kept for that period, so there is nothing to summarise."));
    }
    let key = dive::key(loaded.asked.member, loaded.asked.days, &rows);
    let label = loaded.label(custom);

    // Already written: hand back the same one rather than paying for it twice.
    if let Some(done) = store::summary_for(&store_db()?.lock(), &key).map_err(db_error)? {
        search::log_quietly("deepdive:summary", user, &label);
        return ok(json!({ "reused": true, "summary": summary_json(&done) }));
    }
    let Some(_running) = Running::claim(&key) else {
        return Err(ApiError(StatusCode::CONFLICT, "Someone is summarising this right now. Give it a minute and open it again.".into()));
    };

    let sessions = {
        let stays = panel.data.voice_stays(loaded.period.from, loaded.period.to, now);
        dive::sessions(&stays, loaded.asked.member, loaded.period.from, loaded.period.to)
    };
    let channels = panel.data.channels();
    let rooms: HashMap<u64, String> = sessions
        .iter()
        .map(|s| s.channel_id)
        .collect::<HashSet<u64>>()
        .into_iter()
        .filter_map(|id| channels.iter().find(|c| c.id == id.to_string()).map(|c| (id, c.name.clone())))
        .collect();
    let mut names: HashMap<u64, String> = HashMap::new();
    for id in sessions.iter().flat_map(|s| s.with.iter().map(|(id, _)| *id)).collect::<HashSet<u64>>() {
        if let Some(m) = panel.data.cached_member(id) {
            names.insert(id, m.name);
        }
    }
    let prompt = dive::build_prompt(&loaded.name, &dive::period_words(&loaded.period, custom), &rows, &sessions, &rooms, &names, summary_max());
    let reply = ask_model(&panel, &prompt.text).await.map_err(|err| {
        tracing::warn!("deepdive: a summary failed after {} tries: {}", TRIES, err);
        ApiError(
            StatusCode::BAD_GATEWAY,
            format!(
                "The summary model didn't answer ({}), even after {} tries. Nothing was saved or charged twice — press Try again in a minute.",
                why_failed(&err),
                TRIES
            ),
        )
    })?;
    let parsed = dive::parse_summary(&reply.text, rows.len());
    let new = store::NewSummary {
        stretch_key: key,
        channel_id: 0,
        // One person, in all three places the store keeps people.
        a_id: loaded.asked.member,
        b_id: loaded.asked.member,
        people: vec![loaded.asked.member],
        scope: dive::SCOPE_MEMBER.to_string(),
        start_ms: loaded.period.from * 1000,
        end_ms: loaded.period.to * 1000,
        detection_id: None,
        message_ids: rows.iter().map(|r| r.message_id).collect(),
        sent_count: prompt.sent,
        trimmed: prompt.trimmed,
        run_by: user,
        run_ts: now,
        model: reply.model,
        input_tokens: reply.input_tokens,
        output_tokens: reply.output_tokens,
        summary: parsed,
        raw: reply.text,
    };
    let id = store::add_summary(&store_db()?.lock(), &new).map_err(db_error)?;
    search::log_quietly("deepdive:summary", user, &label);
    tracing::info!("deepdive: {} summarised {} ({} + {} tokens)", user, label, new.input_tokens, new.output_tokens);
    ok(json!({ "reused": false, "summary": summary_json(&Summary { id, new }) }))
}

/// How a deep dive reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let summarised = e.key == "deepdive:summary";
    obj.insert("label".into(), json!(if summarised { "Summarised a deep dive" } else { "Deep dive into a member" }));
    obj.insert("section".into(), json!({ "id": "deepdive", "title": "Deep dive", "icon": "🔎" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A look and a summary read differently, and neither dumps what was read.
    #[test]
    fn the_activity_log_tells_a_look_from_a_summary() {
        let entry = |key: &str| super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".into(),
            key: key.into(),
            old: None,
            new: Some("@notyourbhai · last 7 days".into()),
        };
        let look = audit_entry(&entry("deepdive:look"));
        assert_eq!(look["label"], json!("Deep dive into a member"));
        assert_eq!(look["change"], json!("@notyourbhai · last 7 days"));
        assert_eq!(look["section"]["id"], json!("deepdive"));
        assert!(look["new"].is_null() && look["old"].is_null(), "the messages themselves never go in the log");
        assert_eq!(audit_entry(&entry("deepdive:summary"))["label"], json!("Summarised a deep dive"));
    }

    /// The page can be asked for a period or for two dates, and nothing else.
    #[test]
    fn the_query_takes_a_period_or_a_range_of_your_own() {
        let q = |member: &str, days: &str, from: &str, to: &str| DiveQuery {
            member: Some(member.into()),
            days: Some(days.into()),
            from: Some(from.into()),
            to: Some(to.into()),
        };
        let asked = read_query(&q("3003", "14", "", "")).unwrap();
        assert_eq!((asked.member, asked.days), (3003, 14));
        assert!(asked.custom.is_none());
        assert_eq!(read_query(&q("3003", "", "", "")).unwrap().days, dive::DEFAULT_DAYS, "seven by default");

        let range = read_query(&q("3003", "", "1000", &(1000 + 3 * DAY).to_string())).unwrap();
        assert_eq!(range.custom, Some((1000, 1000 + 3 * DAY)));
        assert_eq!(range.days, 3, "the range says how many days it is");

        for bad in [q("", "7", "", ""), q("nope", "7", "", ""), q("3003", "9", "", ""), q("3003", "", "5000", "1000"), q("3003", "", "1000", "")] {
            assert!(read_query(&bad).is_err());
        }
        let too_long = q("3003", "", "1000", &(1000 + (dive::MAX_RANGE_DAYS + 1) * DAY).to_string());
        assert!(read_query(&too_long).is_err(), "a range of your own is bounded too");
    }
}
