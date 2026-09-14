//! Special welcomes on the panel: list them with who they're for, make, edit
//! and delete them, preview the words, look a pasted Discord ID up, and send a
//! test copy with no pings. Changes go to the activity log as `welcome:<id>`.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::super::welcomes::{self, Facts, Welcome};
use super::{ApiError, ApiResult, Caller, MemberInfo, Panel, ok};

/// Least time between two test posts of one welcome.
const TEST_GAP: Duration = Duration::from_secs(10);
/// Members the list may ask Discord about when the cache doesn't know them.
const FETCHES: usize = 20;

static LAST_TEST: LazyLock<Mutex<HashMap<i64, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn welcome_id(raw: &str) -> Result<i64, ApiError> {
    raw.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| ApiError::not_found("No such welcome."))
}

fn welcome_channel() -> Option<u64> {
    super::super::id("VIZIER_WELCOME_CHANNEL").filter(|id| *id != 0)
}

/// A person's id as pasted or picked: a Discord snowflake, or someone the
/// server knows.
async fn person_id(panel: &Panel, raw: &str) -> Option<u64> {
    if let Some(id) = welcomes::snowflake(raw) {
        return Some(id);
    }
    let id = super::parse_id(raw)?;
    panel.data.member(id).await.map(|_| id)
}

/// People the panel names: from the cache, then the server, then Discord itself.
struct People {
    found: HashMap<u64, Option<(MemberInfo, bool)>>,
    fetches_left: usize,
}

impl People {
    fn new(fetches: usize) -> Self {
        Self { found: HashMap::new(), fetches_left: fetches }
    }

    /// The person and whether they are in the server.
    async fn find(&mut self, panel: &Panel, id: u64) -> Option<(MemberInfo, bool)> {
        if let Some(known) = self.found.get(&id) {
            return known.clone();
        }
        let found = match panel.data.cached_member(id) {
            Some(m) => Some((m, true)),
            None if self.fetches_left > 0 => {
                self.fetches_left -= 1;
                match panel.data.member(id).await {
                    Some(m) => Some((m, true)),
                    None => panel.data.user(id).await.map(|u| (u, false)),
                }
            }
            None => None,
        };
        self.found.insert(id, found.clone());
        found
    }

    async fn json(&mut self, panel: &Panel, id: &str, fallback_name: &str) -> Value {
        let found = match id.parse::<u64>() {
            Ok(n) if n != 0 => self.find(panel, n).await,
            _ => None,
        };
        match found {
            Some((m, in_server)) => json!({
                "id": id, "name": m.name, "username": m.username, "avatar": m.avatar, "in_server": in_server, "known": true,
            }),
            None => json!({
                "id": id,
                "name": if fallback_name.is_empty() { Value::Null } else { json!(fallback_name) },
                "username": Value::Null, "avatar": Value::Null, "in_server": Value::Null, "known": false,
            }),
        }
    }
}

fn channel_json(panel: &Panel, channel_id: &str) -> Value {
    let (id, default) = match channel_id.trim() {
        "" => match welcome_channel() {
            Some(id) => (id.to_string(), true),
            None => return Value::Null,
        },
        other => (other.to_string(), false),
    };
    let name = panel.data.channels().into_iter().find(|c| c.id == id).map(|c| c.name);
    json!({ "id": id, "name": name, "default": default })
}

async fn welcome_json(panel: &Panel, people: &mut People, w: &Welcome) -> Value {
    let mut obj = match serde_json::to_value(w) {
        Ok(Value::Object(obj)) => obj,
        _ => Map::new(),
    };
    obj.insert("member".into(), people.json(panel, &w.user_id, &w.user_name).await);
    let mut also = Vec::with_capacity(w.also_ping.len());
    for id in &w.also_ping {
        also.push(people.json(panel, id, "").await);
    }
    obj.insert("also".into(), Value::Array(also));
    obj.insert("channel".into(), channel_json(panel, &w.channel_id));
    Value::Object(obj)
}

pub async fn list(State(panel): State<Panel>) -> ApiResult {
    let all = welcomes::list();
    let mut people = People::new(FETCHES);
    let mut items = Vec::with_capacity(all.len());
    for w in &all {
        items.push(welcome_json(&panel, &mut people, w).await);
    }
    ok(json!({
        "enabled": super::super::on("VIZIER_SPECIAL_WELCOMES", true),
        "usual_enabled": super::super::on("VIZIER_WELCOME", true),
        "welcome_channel": channel_json(&panel, ""),
        "total": all.len(),
        "active": all.iter().filter(|w| w.enabled).count(),
        "max": welcomes::MAX_RULES,
        "items": items,
    }))
}

/// Checks a welcome from the panel and tidies it into the stored shape. `id` is
/// the welcome being edited, 0 for a new one.
async fn check(panel: &Panel, body: &[u8], id: i64) -> Result<Welcome, ApiError> {
    let mut value: Value = serde_json::from_slice(body).map_err(|_| ApiError::bad("That isn't valid JSON."))?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert("id".into(), json!(id));
    }
    let w: Welcome = serde_json::from_value(value).map_err(|e| ApiError::bad(format!("The welcome is missing something: {}", e)))?;
    let mut w = welcomes::tidy(w).map_err(ApiError::bad)?;
    let user = person_id(panel, &w.user_id)
        .await
        .ok_or_else(|| ApiError::bad(format!("“{}” isn't a Discord ID. A member's ID is 17 to 20 digits.", w.user_id)))?;
    w.user_id = user.to_string();
    let mut people = People::new(3);
    if let Some(other) = welcomes::list().into_iter().find(|o| o.user_id == w.user_id && o.id != id) {
        let name = if other.user_name.is_empty() { format!("member {}", other.user_id) } else { other.user_name };
        return Err(ApiError(StatusCode::CONFLICT, format!("There's already a welcome for {}. Edit that one instead.", name)));
    }
    if w.user_name.is_empty() {
        match people.find(panel, user).await {
            Some((m, _)) => w.user_name = m.name.chars().take(welcomes::MAX_NAME).collect(),
            None => {
                return Err(ApiError::bad(
                    "Type a name for them: Discord doesn't know this ID, so the panel can't name them.",
                ));
            }
        }
    }
    let mut also = Vec::with_capacity(w.also_ping.len());
    for raw in &w.also_ping {
        let id = person_id(panel, raw)
            .await
            .ok_or_else(|| ApiError::bad(format!("“{}” in Also ping isn't a Discord ID.", raw)))?;
        if id != user {
            also.push(id.to_string());
        }
    }
    w.also_ping = also;
    if w.channel_id.is_empty() {
        if welcome_channel().is_none() {
            return Err(ApiError::bad("Pick a channel: no welcome channel is set, so there's nowhere else for it to go."));
        }
    } else {
        w.channel_id = super::check_channel(panel, &w.channel_id, false).map_err(ApiError::bad)?;
    }
    Ok(w)
}

pub async fn create(
    State(panel): State<Panel>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    if welcomes::list().len() >= welcomes::MAX_RULES {
        return Err(ApiError::bad(format!("There are already {} welcomes. Delete one first.", welcomes::MAX_RULES)));
    }
    let mut w = check(&panel, &body, 0).await?;
    w.id = 0;
    w.created_by = admin.to_string();
    w.created_ts = 0;
    w.fired_count = 0;
    w.last_fired_ts = 0;
    w.last_fired_message_id.clear();
    let id = welcomes::save(&w, admin).map_err(ApiError::internal)?;
    tracing::info!("panel: {} made special welcome {} for {}", admin, id, w.user_id);
    let saved = welcomes::get(id).ok_or_else(|| ApiError::internal("welcome vanished after saving"))?;
    let json = welcome_json(&panel, &mut People::new(FETCHES), &saved).await;
    Ok((StatusCode::CREATED, axum::Json(json)).into_response())
}

pub async fn update(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let id = welcome_id(&id)?;
    let old = welcomes::get(id).ok_or_else(|| ApiError::not_found("No such welcome."))?;
    let mut w = check(&panel, &body, id).await?;
    // Kept by the bot and the first save; the page can't rewrite them.
    w.created_by = old.created_by;
    w.created_ts = old.created_ts;
    w.fired_count = old.fired_count;
    w.last_fired_ts = old.last_fired_ts;
    w.last_fired_message_id = old.last_fired_message_id;
    welcomes::save(&w, admin).map_err(ApiError::internal)?;
    tracing::info!("panel: {} edited special welcome {}", admin, id);
    let saved = welcomes::get(id).ok_or_else(|| ApiError::not_found("No such welcome."))?;
    ok(welcome_json(&panel, &mut People::new(FETCHES), &saved).await)
}

pub async fn delete(Path(id): Path<String>, axum::Extension(Caller(admin)): axum::Extension<Caller>) -> ApiResult {
    let id = welcome_id(&id)?;
    if welcomes::get(id).is_none() {
        return Err(ApiError::not_found("No such welcome."));
    }
    welcomes::delete(id, admin).map_err(ApiError::internal)?;
    tracing::info!("panel: {} deleted special welcome {}", admin, id);
    ok(json!({ "ok": true }))
}

/// What the join log says about the member's next join: this counts as one
/// more, and they're away since they last left.
async fn next_join(panel: &Panel, user: Option<u64>) -> (Facts, bool) {
    let summary = match user {
        Some(id) => panel.data.join_summary(id).await,
        None => None,
    };
    match summary {
        Some(s) => (Facts { joins: s.joins + 1, last_leave: s.last_leave.as_deref().and_then(welcomes::moment) }, true),
        None => (Facts { joins: 1, last_leave: None }, false),
    }
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
pub struct PreviewBody {
    #[serde(default)]
    user_id: String,
    #[serde(default)]
    user_name: String,
    #[serde(default)]
    lines: Vec<String>,
    #[serde(default = "yes")]
    ping_member: bool,
    #[serde(default)]
    also_ping: Vec<String>,
}

/// The lines as they'd post if the member joined now. Nothing is sent.
pub async fn preview(State(panel): State<Panel>, body: axum::body::Bytes) -> ApiResult {
    let body: PreviewBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Send the lines to preview."))?;
    if body.lines.len() > welcomes::MAX_LINES {
        return Err(ApiError::bad(format!("Use at most {} versions of the message.", welcomes::MAX_LINES)));
    }
    if body.lines.iter().any(|l| l.chars().count() > welcomes::MAX_LINE) {
        return Err(ApiError::bad(format!("A message is longer than {} characters.", welcomes::MAX_LINE)));
    }
    let user = match body.user_id.trim() {
        "" => None,
        raw => Some(person_id(&panel, raw).await.ok_or_else(|| ApiError::bad("That isn't a Discord ID."))?),
    };
    let now = chrono::Utc::now().timestamp();
    let (facts, from_log) = next_join(&panel, user).await;
    let mut people = People::new(12);
    let name = match (body.user_name.trim(), user) {
        ("", Some(id)) => people.find(&panel, id).await.map(|(m, _)| m.name).unwrap_or_else(|| "them".into()),
        ("", None) => "them".into(),
        (typed, _) => typed.chars().take(welcomes::MAX_NAME).collect(),
    };
    let user_id = user.map(|u| u.to_string()).unwrap_or_default();
    let items: Vec<String> = body.lines.iter().map(|l| welcomes::render(l.trim(), &user_id, &name, &facts, now)).collect();

    let rule = Welcome {
        user_id: user_id.clone(),
        ping_member: body.ping_member,
        also_ping: body.also_ping.iter().take(welcomes::MAX_ALSO_PING).cloned().collect(),
        ..Default::default()
    };
    let pinged: Vec<String> = welcomes::pings(&rule).iter().map(u64::to_string).collect();
    let mut names = Map::new();
    let mut unpinged = Vec::new();
    for id in items.iter().flat_map(|t| welcomes::mentioned(t)) {
        if names.contains_key(&id) {
            continue;
        }
        let shown = if id == user_id {
            json!(name)
        } else {
            match id.parse::<u64>() {
                Ok(n) => json!(people.find(&panel, n).await.map(|(m, _)| m.name)),
                Err(_) => Value::Null,
            }
        };
        if !pinged.contains(&id) {
            unpinged.push(id.clone());
        }
        names.insert(id, shown);
    }
    let away = facts.last_leave.map(|at| (now - at).max(0));
    ok(json!({
        "items": items,
        "name": name,
        "facts": {
            "n": super::super::super::ordinal(facts.joins),
            "away": away.map_or_else(|| "a while".to_string(), welcomes::away_words),
            "days": away.unwrap_or(0) / 86_400,
            "hours": away.unwrap_or(0) / 3600,
            "from_log": from_log,
        },
        "names": names,
        "pings": pinged,
        "unpinged": unpinged,
    }))
}

/// "Send a test now": one of the lines, marked as a test, with no pings at all.
/// The welcome isn't counted and stays as it was.
pub async fn send_test(
    State(panel): State<Panel>,
    Path(id): Path<String>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
) -> ApiResult {
    let id = welcome_id(&id)?;
    let w = welcomes::get(id).ok_or_else(|| ApiError::not_found("No such welcome."))?;
    let channel = welcomes::channel_for(&w, welcome_channel())
        .ok_or_else(|| ApiError::bad("Pick a channel first: no welcome channel is set."))?;
    {
        let mut last = LAST_TEST.lock();
        if last.get(&id).is_some_and(|at| at.elapsed() < TEST_GAP) {
            return Err(ApiError(StatusCode::TOO_MANY_REQUESTS, "A test just went out. Give it a few seconds.".into()));
        }
        last.insert(id, Instant::now());
    }
    let (facts, _) = next_join(&panel, w.user_id.parse().ok()).await;
    let line = welcomes::pick_line(&w.lines, rand::random::<f64>()).ok_or_else(|| ApiError::bad("Write a message first."))?;
    let text = format!("(test) {}", welcomes::render(line, &w.user_id, &w.user_name, &facts, chrono::Utc::now().timestamp()));
    tracing::info!("panel: {} sent a test of special welcome {}", admin, id);
    panel.data.send_unpinged(channel, text).await.map_err(ApiError::bad)?;
    ok(json!({ "ok": true, "channel_id": channel.to_string() }))
}

#[derive(Deserialize)]
pub struct LookupQuery {
    #[serde(default)]
    id: String,
}

/// Who a pasted ID belongs to.
pub async fn lookup(State(panel): State<Panel>, Query(q): Query<LookupQuery>) -> ApiResult {
    let id = person_id(&panel, &q.id)
        .await
        .ok_or_else(|| ApiError::bad("That isn't a Discord ID: they're 17 to 20 digits."))?;
    let existing = welcomes::list().into_iter().find(|w| w.user_id == id.to_string()).map(|w| w.id);
    match People::new(1).find(&panel, id).await {
        Some((m, in_server)) => ok(json!({
            "found": true, "id": id.to_string(), "name": m.name, "username": m.username, "avatar": m.avatar,
            "in_server": in_server, "bot": m.bot, "welcome_id": existing,
        })),
        None => Err(ApiError::not_found("Discord doesn't know that ID. Check it, or type the name to show for them.")),
    }
}

/// How a welcome change reads in the activity log.
pub fn audit_entry(panel: &Panel, e: &super::super::AuditEntry) -> Map<String, Value> {
    let body = |b: &Option<String>| b.as_deref().and_then(|t| serde_json::from_str::<Welcome>(t).ok());
    let (old, new) = (body(&e.old), body(&e.new));
    let facts = new.as_ref().or(old.as_ref());
    let uid = facts.map(|w| w.user_id.clone()).unwrap_or_default();
    let name = uid
        .parse::<u64>()
        .ok()
        .and_then(|id| panel.data.cached_member(id))
        .map(|m| m.name)
        .or_else(|| facts.map(|w| w.user_name.clone()).filter(|n| !n.is_empty()))
        .unwrap_or_else(|| if uid.is_empty() { "a member".to_string() } else { uid.clone() });
    let change = match (&old, &new) {
        (None, Some(_)) => "Created",
        (Some(_), None) => "Deleted",
        (Some(o), Some(n)) if o.enabled != n.enabled => {
            if n.enabled { "Switched on" } else { "Switched off" }
        }
        _ => "Edited",
    };
    let mut obj = Map::new();
    obj.insert("label".into(), json!(format!("Special welcome for @{}", name)));
    obj.insert("section".into(), json!({ "id": "welcomes", "title": "Welcomes", "icon": "👋" }));
    obj.insert("change".into(), json!(change));
    if !uid.is_empty() {
        obj.insert("member_id".into(), json!(uid));
    }
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}
