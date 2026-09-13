//! The Auto-responses page's API: the rules in `autoreplies.rs`, checked and
//! tidied on the way in, plus a "would this message fire?" tester.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::autoreplies::{self, AutoReply};
use super::{ApiError, ApiResult, Caller, Panel, check_channel, ok};

const MAX_COOLDOWN: u32 = 7 * 24 * 3600;

fn rule_from(body: &Value) -> Result<AutoReply, ApiError> {
    let mut value = body.clone();
    if let Some(obj) = value.as_object_mut() {
        obj.entry("id").or_insert(json!(0));
        obj.entry("enabled").or_insert(json!(true));
    }
    serde_json::from_value(value).map_err(|e| ApiError::bad(format!("The rule is missing something: {}", e)))
}

fn tidy_list(list: &[String], max_len: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in list {
        let t: String = item.trim().chars().take(max_len).collect();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Checks a rule from the page and tidies it into the stored shape.
pub fn check(panel: &Panel, mut r: AutoReply) -> Result<AutoReply, ApiError> {
    r.name = r.name.trim().to_string();
    if r.name.chars().count() > 100 {
        return Err(ApiError::bad("Keep the name under 100 characters."));
    }
    if r.triggers.iter().any(|t| t.trim().chars().count() > 200) {
        return Err(ApiError::bad("A trigger is longer than 200 characters."));
    }
    r.triggers = tidy_list(&r.triggers, 200);
    if r.triggers.len() > 50 {
        return Err(ApiError::bad("Keep it to 50 triggers per rule."));
    }
    let channels = |list: &[String], what: &str| -> Result<Vec<String>, ApiError> {
        let mut out = Vec::new();
        for c in tidy_list(list, 24) {
            let id = check_channel(panel, &c, false).map_err(|e| ApiError::bad(format!("{} {}", what, e)))?;
            if !out.contains(&id) {
                out.push(id);
            }
        }
        Ok(out)
    };
    r.channels = channels(&r.channels, "Channels:")?;
    r.exclude_channels = channels(&r.exclude_channels, "Excluded channels:")?;
    if r.exclude_channels.iter().any(|c| r.channels.contains(c)) {
        return Err(ApiError::bad("A channel can't be both included and excluded."));
    }
    r.replies = r.replies.iter().map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
    if r.replies.len() > 50 {
        return Err(ApiError::bad("Keep it to 50 replies per rule."));
    }
    r.reactions = tidy_list(&r.reactions, 80);
    if r.reactions.len() > 20 {
        return Err(ApiError::bad("Discord allows 20 reactions at most."));
    }
    if r.cooldown_secs > MAX_COOLDOWN {
        return Err(ApiError::bad("The cooldown can be at most 7 days."));
    }
    autoreplies::validate(&r).map_err(ApiError::bad)?;
    Ok(r)
}

fn rule_id(raw: &str) -> Result<i64, ApiError> {
    raw.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| ApiError::not_found("No such auto-response."))
}

fn body_json(body: &[u8]) -> Result<Value, ApiError> {
    serde_json::from_slice(body).map_err(|_| ApiError::bad("That isn't valid JSON."))
}

pub async fn list() -> ApiResult {
    ok(autoreplies::list())
}

pub async fn get(Path(id): Path<String>) -> ApiResult {
    autoreplies::get(rule_id(&id)?).map(|r| axum::Json(r).into_response()).ok_or_else(|| ApiError::not_found("No such auto-response."))
}

pub async fn create(
    State(panel): State<Panel>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let mut r = check(&panel, rule_from(&body_json(&body)?)?)?;
    r.id = 0;
    r.hits = 0;
    r.last_hit = 0;
    let id = autoreplies::save(&r, user).map_err(|e| ApiError::bad(e.to_string()))?;
    let saved = autoreplies::get(id).ok_or_else(|| ApiError::internal("auto-response vanished after saving"))?;
    Ok((StatusCode::CREATED, axum::Json(saved)).into_response())
}

pub async fn update(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let id = rule_id(&id)?;
    let old = autoreplies::get(id).ok_or_else(|| ApiError::not_found("No such auto-response."))?;
    let mut r = check(&panel, rule_from(&body_json(&body)?)?)?;
    // The bot keeps the counts; the page can't rewrite them.
    r.id = id;
    r.hits = old.hits;
    r.last_hit = old.last_hit;
    autoreplies::save(&r, user).map_err(|e| ApiError::bad(e.to_string()))?;
    ok(autoreplies::get(id))
}

pub async fn toggle(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let mut r = autoreplies::get(rule_id(&id)?).ok_or_else(|| ApiError::not_found("No such auto-response."))?;
    r.enabled = !r.enabled;
    autoreplies::save(&r, user).map_err(|e| ApiError::bad(e.to_string()))?;
    ok(autoreplies::get(r.id))
}

pub async fn delete(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = rule_id(&id)?;
    if autoreplies::get(id).is_none() {
        return Err(ApiError::not_found("No such auto-response."));
    }
    autoreplies::delete(id, user).map_err(ApiError::internal)?;
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
pub struct TestBody {
    rule: Value,
    text: String,
    #[serde(default)]
    channel_id: Option<String>,
}

/// Would this sample message set the rule off? Chance and cooldown are left
/// out; the answer says why when it wouldn't.
pub async fn test(State(panel): State<Panel>, body: axum::body::Bytes) -> ApiResult {
    let body: TestBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Send {\"rule\": ..., \"text\": ...}."))?;
    if body.text.chars().count() > 2000 {
        return Err(ApiError::bad("A Discord message is at most 2000 characters."));
    }
    let mut rule = rule_from(&body.rule)?;
    rule.triggers = tidy_list(&rule.triggers, 200);
    if rule.match_mode == autoreplies::Match::Pattern {
        for t in &rule.triggers {
            if let Err(err) = regex::Regex::new(t) {
                return ok(json!({ "fires": false, "trigger": null, "why": format!("\"{}\" isn't a valid pattern: {}", t, err) }));
            }
        }
    }
    let channel = match body.channel_id.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) => Some(check_channel(&panel, c, false).map_err(ApiError::bad)?.parse::<u64>().unwrap_or(0)),
        None => None,
    };
    if rule.triggers.is_empty() {
        return ok(json!({ "fires": false, "trigger": null, "why": "Add a trigger first." }));
    }
    if let Some(c) = channel {
        if !autoreplies::applies_in(&rule, c) {
            return ok(json!({ "fires": false, "trigger": null, "why": "The rule doesn't work in that channel." }));
        }
    }
    let trigger = autoreplies::matched_trigger(&rule, &body.text, channel);
    let why = match (&trigger, rule.enabled) {
        (None, _) if body.text.trim().is_empty() => "Type a message to try.".to_string(),
        (None, _) => "No trigger matches this message.".to_string(),
        (Some(t), true) => format!("Matches “{}”.", t),
        (Some(t), false) => format!("Matches “{}”, but the rule is switched off.", t),
    };
    ok(json!({ "fires": trigger.is_some() && rule.enabled, "matches": trigger.is_some(), "trigger": trigger, "why": why }))
}
