//! The Deep dives page: every dive that has ever been run, newest first.
//!
//! A deep dive costs a long prompt and a few thousand tokens, and until now the
//! only way back to one was to ask for exactly the same window again and hope
//! the stored one still matched. This lists them: who it was about, what period,
//! who ran it and when, and the summary itself — so opening yesterday's dive is
//! free, however long ago it was paid for.
//!
//! The nightly ones are on the same list, marked with why the scan picked that
//! member out. That is the point of the page in the morning: a moderator opens
//! the panel and sees who needs a look, rather than a search box and a guess.
//!
//! Nothing new is summarised here — this page only reads. Admins only, like
//! every panel page, and a look is one line in the activity log.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::deepdive::{self as dive, SCOPE_MEMBER};
use super::super::super::kalesh_store::{self as store, Summary};
use super::super::super::topics_store::{self as runs, KIND_SCAN};
use super::msglog::member_json;
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

/// Dives one page lists at most.
pub const LIST_LIMIT: usize = 200;

fn store_db() -> Result<&'static parking_lot::Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The summary store isn't open. Restart the bot.".into()))
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// One member's dives only.
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

/// "last 7 days", or the two dates: how long one stored dive covered. Worked
/// out from the period the summary was filed with, so a dive written months ago
/// still reads the way it did the day it was run.
pub fn period_words(s: &Summary) -> String {
    let period = dive::Period { from: s.new.start_ms / 1000, to: s.new.end_ms / 1000, shifted: false };
    dive::period_words(&period, false)
}

/// One row of the list. The whole summary rides along, so opening a row never
/// costs another request — and never another token.
fn row_json(panel: &Panel, s: &Summary) -> Value {
    let nightly = s.new.run_by == 0;
    json!({
        "id": s.id,
        "member": member_json(panel, s.new.a_id, "", ""),
        "period": {
            "days": (s.new.end_ms - s.new.start_ms) / 86_400_000,
            "from": s.new.start_ms / 1000,
            "to": s.new.end_ms / 1000,
            "words": period_words(s),
        },
        // Nobody pressed a button for a nightly one, so there is no member to name.
        "run_by": (!nightly).then(|| member_json(panel, s.new.run_by, "", "")),
        "nightly": nightly,
        "why": s.new.reason,
        "run_ts": s.new.run_ts,
        "model": s.new.model,
        "messages": s.new.message_ids.len(),
        "sent": s.new.sent_count,
        "trimmed": s.new.trimmed,
        "input_tokens": s.new.input_tokens,
        "output_tokens": s.new.output_tokens,
        "summary": s.new.summary,
        // A reply that was not JSON is kept as it came, rather than lost.
        "raw": s.new.summary.is_none().then(|| s.new.raw.clone()),
        // Where the page sends a mod who wants the messages themselves.
        "href": format!("#/deepdive?member={}&days={}", s.new.a_id, (s.new.end_ms - s.new.start_ms) / 86_400_000),
    })
}

pub async fn list(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<ListQuery>) -> ApiResult {
    let member = match q.member.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        Some(raw) => Some(parse_id(raw).ok_or_else(|| ApiError::bad("That isn't a member id."))?),
        None => None,
    };
    let limit = q.limit.as_deref().and_then(|l| l.parse::<usize>().ok()).unwrap_or(LIST_LIMIT).clamp(1, LIST_LIMIT);
    let (rows, counts) = {
        let conn = store_db()?.lock();
        let rows = store::member_summaries(&conn, member, limit).map_err(|err| ApiError::internal(format!("summary store: {}", err)))?;
        (rows, store::member_summary_counts(&conn))
    };
    // The last few nights of the scan, so a night that failed says so on the
    // page rather than only in the log. A morning with no rows is either a quiet
    // night or a broken one, and a mod must be able to tell which.
    let scans: Vec<Value> = runs::db()
        .map(|db| {
            runs::runs(&db.lock(), KIND_SCAN, 7)
                .unwrap_or_default()
                .into_iter()
                .map(|r| {
                    json!({
                        "day": r.day,
                        "state": if r.unfinished() { "failed" } else { "ran" },
                        "looked_at": r.chunks,
                        "dived": r.members,
                        "failed": r.failed,
                        "note": r.note,
                        "ts": r.finished_ts.unwrap_or(r.started_ts),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    search::log_quietly("deepdives:list", user, &match member {
        Some(m) => format!("Dives into @{}", super::members::name_or_id(&panel, m).await),
        None => "Every deep dive".to_string(),
    });
    ok(json!({
        "dives": rows.iter().map(|s| row_json(&panel, s)).collect::<Vec<_>>(),
        "total": counts.0,
        "nightly": counts.1,
        "scope": SCOPE_MEMBER,
        "watch_on": super::super::super::watchlist::watch_on(),
        "scans": scans,
        "read_first": super::deepdive::READ_FIRST,
    }))
}

/// How the Deep dives page reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    obj.insert("label".into(), json!("Looked at the deep dives"));
    obj.insert("section".into(), json!({ "id": "deepdives", "title": "Deep dives", "icon": "🔎" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::super::super::kalesh_store::NewSummary;

    fn dive(member: u64, days: i64, run_by: u64, reason: &str) -> Summary {
        Summary {
            id: 1,
            new: NewSummary {
                stretch_key: "k".into(),
                channel_id: 0,
                a_id: member,
                b_id: member,
                people: vec![member],
                scope: SCOPE_MEMBER.into(),
                start_ms: 1_700_000_000_000,
                end_ms: 1_700_000_000_000 + days * 86_400_000,
                detection_id: None,
                message_ids: vec![1, 2, 3],
                sent_count: 3,
                trimmed: false,
                run_by,
                run_ts: 1_700_000_500,
                model: "cheap".into(),
                input_tokens: 900,
                output_tokens: 200,
                summary: Some(json!({ "overview": "A quiet week." })),
                raw: "{}".into(),
                reason: reason.into(),
            },
        }
    }

    /// A stored dive says how long it covered without being asked again.
    #[test]
    fn a_stored_dive_remembers_its_own_period() {
        assert_eq!(period_words(&dive(11, 7, 99, "")), "last 7 days");
        assert_eq!(period_words(&dive(11, 30, 99, "")), "last 30 days");
    }

    /// The one a mod asked for and the one the night picked read differently.
    #[test]
    fn a_nightly_dive_carries_why_it_was_run_and_an_asked_for_one_does_not() {
        let asked = dive(11, 7, 99, "");
        let nightly = dive(22, 7, 0, "In 1 fight the detector called today");
        assert!(asked.new.reason.is_empty(), "a dive somebody pressed for has no reason on it");
        assert_eq!(asked.new.run_by, 99, "and names who pressed");
        assert_eq!(nightly.new.run_by, 0, "nobody pressed a button");
        assert_eq!(nightly.new.reason, "In 1 fight the detector called today");
        // And the stored summary comes along, so opening it is free.
        assert_eq!(nightly.new.summary, Some(json!({ "overview": "A quiet week." })));
    }

    /// A night that failed is on the page, not only in the log: the page can
    /// tell a quiet night from a broken one.
    #[test]
    fn a_scan_that_failed_reads_differently_from_one_that_found_nothing() {
        let row = |unfinished: bool, failed: i64, note: &str| {
            let r = super::super::super::super::topics_store::Run {
                kind: KIND_SCAN.into(),
                day: "2026-09-21".into(),
                started_ts: 100,
                finished_ts: (!unfinished).then_some(200),
                chunks: 173,
                members: if unfinished { 0 } else { 9 },
                failed,
                note: note.into(),
                ..Default::default()
            };
            json!({ "state": if r.unfinished() { "failed" } else { "ran" }, "looked_at": r.chunks, "dived": r.members, "failed": r.failed, "note": r.note })
        };
        let quiet = row(false, 0, "");
        assert_eq!(quiet["state"], "ran");
        assert_eq!((quiet["failed"].as_i64(), quiet["dived"].as_i64()), (Some(0), Some(9)));

        let broken = row(true, 0, "");
        assert_eq!(broken["state"], "failed", "picked up and never came back");
        assert_eq!(broken["dived"], 0);

        let partly = row(false, 3, "3 of 12 dives failed — the provider is rate-limiting us");
        assert_eq!(partly["state"], "ran");
        assert_eq!(partly["failed"], 3);
        assert!(partly["note"].as_str().unwrap().contains("rate-limiting"), "the reason reaches the page in words");
    }

    /// A look is logged, and the log never holds what was read.
    #[test]
    fn the_activity_log_says_who_looked_and_nothing_else() {
        let entry = super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".into(),
            key: "deepdives:list".into(),
            new: Some("Dives into @notyourbhai".into()),
            old: None,
        };
        let got = audit_entry(&entry);
        assert_eq!(got["label"], json!("Looked at the deep dives"));
        assert_eq!(got["change"], json!("Dives into @notyourbhai"));
        assert_eq!(got["section"]["id"], json!("deepdives"));
        assert!(got["new"].is_null() && got["old"].is_null(), "a summary never goes in the log");
    }
}
