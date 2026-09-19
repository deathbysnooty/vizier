//! Member notes on the panel: on a member's page, their notes as `/about` shows
//! them, when "what they're like" was built and what it cost, who has looked
//! them up, and buttons to rebuild or clear; plus the runs and their cost.

use axum::extract::{Path, State};
use serde_json::{Value, json};

use super::super::super::notes;
use super::super::super::notes_store as store;
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id};

fn user_id(raw: &str) -> Result<u64, ApiError> {
    parse_id(raw).ok_or_else(|| ApiError::bad("That isn't a member id."))
}

fn note_json(n: &store::Note) -> Value {
    json!({
        "bullets": n.bullets,
        "built_ts": n.built_ts,
        "input_tokens": n.input_tokens,
        "output_tokens": n.output_tokens,
        "model": n.model,
        "messages_used": n.messages_used,
        "rejected": n.rejected,
        "built_by": if n.built_by == 0 { Value::Null } else { json!(n.built_by.to_string()) },
    })
}

fn run_json(r: &store::Run) -> Value {
    json!({
        "ts": r.ts, "kind": r.kind, "asked": r.asked, "built": r.built, "skipped": r.skipped, "failed": r.failed,
        "rejected": r.rejected, "input_tokens": r.input_tokens, "output_tokens": r.output_tokens, "waiting": r.waiting,
    })
}

/// One member's notes. Opening them is a lookup, written down like any other.
pub async fn get(State(panel): State<Panel>, Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = user_id(&id)?;
    let db = store::db().ok_or_else(|| ApiError::not_found("The member notes store isn't open."))?;
    let preview = if notes::enabled() { notes::panel_view(id, user).await } else { None };
    let conn = db.lock();
    let lookups: Vec<Value> = store::lookups(&conn, Some(id), 10)
        .into_iter()
        .map(|l| json!({ "ts": l.ts, "asker": l.asker.to_string(), "asker_name": panel.cached_name(l.asker), "via": l.via }))
        .collect();
    let attempt = store::attempts(&conn).remove(&id).map(|(ts, outcome)| json!({ "ts": ts, "outcome": outcome }));
    let s = notes::settings();
    ok(json!({
        "enabled": notes::enabled(),
        "opted_out": store::opted_out(&conn, id),
        "note": store::get(&conn, id).as_ref().map(note_json),
        "attempt": attempt,
        "preview": preview,
        "lookups": lookups,
        "min_messages": s.min_messages,
        "new_messages": s.new_messages,
    }))
}

pub async fn rebuild(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = user_id(&id)?;
    let note = notes::rebuild_now(id, user).await.map_err(ApiError::bad)?;
    let _ = super::super::log_change(&format!("notes:rebuild:{}", id), None, Some(&format!("{} tokens", note.input_tokens + note.output_tokens)), user);
    ok(json!({ "note": note_json(&note) }))
}

/// Clears what's stored. They are not opted out: the next run can build them again.
pub async fn clear(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = user_id(&id)?;
    let db = store::db().ok_or_else(|| ApiError::not_found("The member notes store isn't open."))?;
    let gone = store::delete(&db.lock(), id).map_err(ApiError::internal)?;
    if gone {
        let _ = super::super::log_change(&format!("notes:clear:{}", id), Some("notes"), None, user);
    }
    ok(json!({ "cleared": gone }))
}

/// The runs so far and what they cost, and what a full first build would cost now.
pub async fn overview() -> ApiResult {
    let db = store::db().ok_or_else(|| ApiError::not_found("The member notes store isn't open."))?;
    let (members, input, output) = notes::estimate().await;
    let conn = db.lock();
    let (spent_in, spent_out) = store::tokens_spent(&conn);
    ok(json!({
        "enabled": notes::enabled(),
        "built": store::count(&conn),
        "opted_out": store::optouts(&conn).len(),
        "first_done": store::meta_get(&conn, "first_done").is_some(),
        "spent": { "input_tokens": spent_in, "output_tokens": spent_out },
        "runs": store::runs(&conn, 10).iter().map(run_json).collect::<Vec<_>>(),
        "estimate": { "members": members, "input_tokens": input, "output_tokens": output },
    }))
}

/// How a notes entry reads in the activity log.
pub fn audit_entry(panel: &Panel, e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let (what, uid) = e.key.strip_prefix("notes:").and_then(|rest| rest.split_once(':')).unwrap_or(("", ""));
    let name = uid.parse::<u64>().ok().and_then(|id| panel.cached_name(id)).unwrap_or_else(|| uid.to_string());
    let (label, change) = match what {
        "lookup" => (format!("Looked up @{}", name), format!("via {}", e.new.as_deref().and_then(|n| n.rsplit(" · ").next()).unwrap_or("?"))),
        "rebuild" => (format!("Notes for @{}", name), format!("Rebuilt ({})", e.new.as_deref().unwrap_or(""))),
        "clear" => (format!("Notes for @{}", name), "Cleared".to_string()),
        "optout" => (format!("Notes for @{}", name), e.new.clone().unwrap_or_default()),
        _ => (e.key.clone(), e.new.clone().unwrap_or_default()),
    };
    obj.insert("label".into(), json!(label));
    obj.insert("member_id".into(), json!(uid));
    obj.insert("section".into(), json!({ "id": "about", "title": "About members", "icon": "🪪" }));
    obj.insert("change".into(), json!(change));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}
