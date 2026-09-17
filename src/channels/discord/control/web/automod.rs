//! The Moderation page: every flag automatic moderation has raised, newest
//! first, with what a moderator decided about it.
//!
//! The headline this page exists for is `not_ai`: how often moderators pressed
//! "Not AI" on a flag they judged. There is no reliable way to detect AI
//! writing, so the only honest question about that half of the feature is how
//! often it is wrong, and the answer has to be somewhere it cannot be missed.
//! It is `None`, not a flattering zero, until a moderator has actually judged
//! one.
//!
//! Admin-only, like every page here, and every look is written to the activity
//! log — the rows quote what members wrote.

use axum::extract::{Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::automod;
use super::super::super::automod_store::{self as store, Flag, Kind, ListFilter, Outcome};
use super::{ApiError, ApiResult, Caller, Panel, ok, parse_id, search};

pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
/// How many members the per-member counts list.
pub const TOP_MEMBERS: usize = 25;
/// The periods the page offers, as days back from now.
pub const PERIODS: [(&str, Option<i64>); 4] = [("7", Some(7)), ("30", Some(30)), ("90", Some(90)), ("all", None)];
const DAY: i64 = 86_400;

#[derive(Deserialize)]
pub struct FlagQuery {
    #[serde(default)]
    days: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

struct Asked {
    days_key: String,
    days: Option<i64>,
    since: i64,
    kind: Option<Kind>,
    member: Option<u64>,
    outcome: Option<Outcome>,
    before: Option<i64>,
    limit: usize,
}

fn read_query(q: &FlagQuery, now: i64) -> Result<Asked, ApiError> {
    let blank = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    let days_key = blank(&q.days).unwrap_or_else(|| "30".to_string());
    let days = PERIODS
        .iter()
        .find(|(k, _)| *k == days_key)
        .map(|(_, d)| *d)
        .ok_or_else(|| ApiError::bad("The period is 7, 30, 90 or all (days)."))?;
    let kind = match blank(&q.kind) {
        Some(raw) => Some(Kind::from_str(&raw).ok_or_else(|| ApiError::bad("The kind is spam or ai."))?),
        None => None,
    };
    let outcome = match blank(&q.outcome) {
        Some(raw) => Some(Outcome::from_str(&raw).ok_or_else(|| ApiError::bad("The outcome is deleted, dismissed or untouched."))?),
        None => None,
    };
    let member = match blank(&q.member) {
        Some(raw) => Some(parse_id(&raw).ok_or_else(|| ApiError::bad("That isn't a member id."))?),
        None => None,
    };
    let before = match blank(&q.before) {
        Some(raw) => Some(raw.parse::<i64>().ok().filter(|b| *b > 0).ok_or_else(|| ApiError::bad("That isn't a place to carry on from."))?),
        None => None,
    };
    let limit = match blank(&q.limit) {
        Some(raw) => raw.parse::<usize>().map_err(|_| ApiError::bad("The limit is a number."))?.clamp(1, MAX_LIMIT),
        None => DEFAULT_LIMIT,
    };
    Ok(Asked { days_key, days, since: days.map(|d| now - d * DAY).unwrap_or(0), kind, member, outcome, before, limit })
}

/// How the look reads in the activity log.
fn audit_words(a: &Asked, member_name: Option<&str>) -> String {
    let mut parts = vec![match a.days {
        Some(d) => format!("last {} days", d),
        None => "all time".to_string(),
    }];
    parts.push(match a.kind {
        Some(Kind::Spam) => "spam".to_string(),
        Some(Kind::Ai) => "possibly AI".to_string(),
        None => "every kind".to_string(),
    });
    if let Some(name) = member_name {
        parts.push(format!("@{}", name));
    }
    if let Some(o) = a.outcome {
        parts.push(o.as_str().to_string());
    }
    parts.join(" · ")
}

fn unavailable(what: &str) -> ApiError {
    tracing::warn!("panel: the moderation log couldn't be read: {}", what);
    ApiError(axum::http::StatusCode::SERVICE_UNAVAILABLE, "The moderation log can't be read right now. Try again in a moment.".into())
}

fn member_json(panel: &Panel, id: u64, stored_name: &str) -> Value {
    let who = panel.data.cached_member(id);
    json!({
        "id": id.to_string(),
        "name": who.as_ref().map(|m| m.name.clone()).filter(|n| !n.trim().is_empty())
            .or_else(|| (!stored_name.trim().is_empty()).then(|| stored_name.to_string()))
            .unwrap_or_else(|| format!("Member {}", id)),
        "avatar": who.as_ref().map(|m| m.avatar.clone()),
        "in_server": who.is_some(),
    })
}

/// One row as the page draws it.
fn flag_json(panel: &Panel, f: &Flag) -> Value {
    let channels = panel.data.channels();
    let name = channels.iter().find(|c| c.id == f.channel_id.to_string()).map(|c| c.name.clone()).unwrap_or_else(|| f.channel_name.clone());
    json!({
        "id": f.id,
        "kind": f.kind,
        "rule": f.rule,
        "rule_label": rule_label(&f.rule),
        "member": member_json(panel, f.member_id, &f.member_name),
        "channel": { "id": f.channel_id.to_string(), "name": name },
        "ts": f.ts,
        "text": f.text,
        "text_cleared": f.text_cleared,
        "messages": f.messages,
        "score": f.score,
        "reasons": f.reasons,
        "model": f.model.as_ref().map(|m| json!({
            "name": m.model,
            "confidence": m.confidence,
            "reasons": m.reasons,
            "unjudgeable": m.unjudgeable,
        })),
        "had_baseline": f.had_baseline,
        "outcome": f.outcome,
        "decided_by": f.decided_by.map(|id| member_json(panel, id, "")),
        "decided_ts": f.decided_ts,
        "url": (f.guild_id > 0).then(|| search::jump_url(&f.guild_id.to_string(), f.channel_id, f.message_id)),
    })
}

/// A rule key in the words the page shows.
pub fn rule_label(rule: &str) -> &'static str {
    match rule {
        "repeat" => automod::Rule::Repeat.label(),
        "burst" => automod::Rule::Burst.label(),
        "mentions" => automod::Rule::Mentions.label(),
        "everyone" => automod::Rule::Everyone.label(),
        "invite" => automod::Rule::Invite.label(),
        "wall" => automod::Rule::Wall.label(),
        "ai" => "Might have been written by an AI",
        _ => "A rule that no longer exists",
    }
}

pub async fn list(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<FlagQuery>) -> ApiResult {
    let now = chrono::Utc::now().timestamp();
    let asked = read_query(&q, now)?;

    let member_name = match asked.member {
        Some(id) => Some(match panel.data.cached_member(id) {
            Some(m) => m.name,
            None => panel.data.member(id).await.map(|m| m.name).unwrap_or_else(|| id.to_string()),
        }),
        None => None,
    };
    search::log_quietly("automod:flags", user, &audit_words(&asked, member_name.as_deref()));

    let filter = ListFilter {
        kind: asked.kind,
        member: asked.member,
        outcome: asked.outcome,
        since: asked.since,
        before: asked.before,
        limit: asked.limit,
    };
    let page = panel.data.automod_flags(filter).await.ok_or_else(|| unavailable("the list"))?;
    let totals = panel.data.automod_totals(asked.since).await.ok_or_else(|| unavailable("the counts"))?;
    let people = panel.data.automod_by_member(asked.since, TOP_MEMBERS).await.unwrap_or_default();

    let results: Vec<Value> = page.rows.iter().map(|f| flag_json(&panel, f)).collect();
    ok(json!({
        "days": asked.days_key,
        "kind": asked.kind,
        "outcome": asked.outcome,
        "member": asked.member.map(|m| json!({ "id": m.to_string(), "name": member_name })),
        "limit": asked.limit,
        "count": results.len(),
        "results": results,
        "next_before": page.next_before,
        "totals": totals,
        // The number this page exists for. Null until a mod has judged one:
        // a feature nobody has checked has no accuracy to report.
        "not_ai": {
            "dismissed": totals.ai_dismissed,
            "decided": totals.ai_decided,
            "pct": totals.wrong_pct(),
        },
        "by_member": people.iter().map(|m| json!({
            "member": member_json(&panel, m.member_id, &m.member_name),
            "spam": m.spam,
            "ai": m.ai,
            "dismissed": m.dismissed,
        })).collect::<Vec<_>>(),
        "enabled": automod::enabled(),
        "spam_on": automod::spam_on(),
        "ai_on": automod::ai_on(),
        "keep_days": automod::keep_days(),
        "threshold": automod::ai_settings().threshold,
    }))
}

/// How a look at the page reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    obj.insert("label".into(), json!("Looked at the moderation flags"));
    obj.insert("section".into(), json!({ "id": "automod", "title": "Moderation", "icon": "🚨" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Looked".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

/// Reads the store off the async threads: the lock is taken inside the blocking
/// task and dropped there, never held across an `.await`.
pub async fn read<T: Send + 'static>(f: impl FnOnce(&rusqlite::Connection) -> T + Send + 'static) -> Option<T> {
    let db = store::db()?;
    tokio::task::spawn_blocking(move || {
        let conn = db.lock();
        f(&conn)
    })
    .await
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rule_has_words_for_the_page() {
        for rule in ["repeat", "burst", "mentions", "everyone", "invite", "wall", "ai"] {
            assert!(!rule_label(rule).is_empty(), "{}", rule);
            assert_ne!(rule_label(rule), "A rule that no longer exists", "{}", rule);
        }
        assert_eq!(rule_label("something-from-the-future"), "A rule that no longer exists");
    }

    #[test]
    fn the_period_kind_and_outcome_are_checked() {
        let now = 1_789_000_000;
        let q = |days: &str, kind: &str, outcome: &str| FlagQuery {
            days: Some(days.into()),
            kind: Some(kind.into()),
            member: None,
            outcome: Some(outcome.into()),
            before: None,
            limit: None,
        };
        let asked = read_query(&q("7", "ai", "dismissed"), now).expect("a good query");
        assert_eq!((asked.kind, asked.outcome), (Some(Kind::Ai), Some(Outcome::Dismissed)));
        assert_eq!(asked.since, now - 7 * DAY);
        assert!(read_query(&q("5", "ai", "dismissed"), now).is_err(), "5 days isn't offered");
        assert!(read_query(&q("7", "maybe", "dismissed"), now).is_err());
        assert!(read_query(&q("7", "ai", "burned"), now).is_err());
        // "all" reaches back to the beginning.
        let all = read_query(
            &FlagQuery { days: Some("all".into()), kind: None, member: None, outcome: None, before: None, limit: None },
            now,
        )
        .unwrap();
        assert_eq!((all.days, all.since), (None, 0));
    }

    #[test]
    fn the_audit_line_says_what_was_looked_at() {
        let now = 1_789_000_000;
        let asked = read_query(
            &FlagQuery { days: Some("7".into()), kind: Some("ai".into()), member: None, outcome: Some("dismissed".into()), before: None, limit: None },
            now,
        )
        .unwrap();
        assert_eq!(audit_words(&asked, Some("Zoya")), "last 7 days · possibly AI · @Zoya · dismissed");
        let plain = read_query(
            &FlagQuery { days: Some("all".into()), kind: None, member: None, outcome: None, before: None, limit: None },
            now,
        )
        .unwrap();
        assert_eq!(audit_words(&plain, None), "all time · every kind");
    }
}
