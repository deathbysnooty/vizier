//! The Chocolate Frog page: the wizards on the cards (with their pictures), the
//! latest drops, who owns which cards, the riddle bank, and a test drop.
//! Changes go to the activity log as `frog:wizard:<id>`, `frog:riddle:<id>` and
//! `frog:drop:<id>`.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::super::super::frog_store::{self as store, Rarity, Status, Wizard};
use super::super::super::points::Source;
use super::{ApiError, ApiResult, Caller, Panel, ok};

fn frogs() -> Result<&'static parking_lot::Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| ApiError(StatusCode::SERVICE_UNAVAILABLE, "The frog store isn't open.".into()))
}

fn colour(rarity: Rarity) -> String {
    format!("#{:06x}", rarity.colour())
}

fn person(panel: &Panel, id: u64) -> Value {
    match panel.data.cached_member(id) {
        Some(m) => json!({ "id": id.to_string(), "name": m.name, "avatar": m.avatar, "known": true }),
        None => json!({ "id": id.to_string(), "name": Value::Null, "avatar": Value::Null, "known": false }),
    }
}

fn wizard_json(w: &Wizard, copies: i64) -> Value {
    let source = store::image_source(&w.image, &w.slug);
    json!({
        "id": w.id,
        "slug": w.slug,
        "name": w.name,
        "rarity": w.rarity,
        "image": w.image,
        "enabled": w.enabled,
        "copies": copies,
        "image_source": source,
        "image_url": source.map(|s| format!("/api/frogs/wizards/{}/image?v={}", w.id, if s == "media" { w.image.as_str() } else { "file" })),
    })
}

fn channel_json(panel: &Panel, id: u64) -> Value {
    let name = panel.data.channels().into_iter().find(|c| c.id == id.to_string()).map(|c| c.name);
    json!({ "id": id.to_string(), "name": name })
}

pub async fn overview(State(panel): State<Panel>) -> ApiResult {
    let db = frogs()?;
    let (wizards, copies, totals, bank, drops) = {
        let conn = db.lock();
        let drops: Vec<(store::Drop, Option<store::Riddle>)> =
            store::recent_drops(&conn, 40).into_iter().map(|d| { let r = store::riddle(&conn, &d.riddle_id); (d, r) }).collect();
        (store::wizards(&conn), store::copies(&conn), store::totals(&conn), store::bank_stats(&conn), drops)
    };
    let enabled_wizards: Vec<&Wizard> = wizards.iter().filter(|w| w.enabled).collect();
    let weights: Vec<(Rarity, u64)> = Rarity::ALL
        .into_iter()
        .map(|r| (r, if enabled_wizards.iter().any(|w| w.rarity == r) { r.weight() } else { 0 }))
        .collect();
    let total_weight: u64 = weights.iter().map(|w| w.1).sum();
    // Only rarities some card has: an unused rarity would read as a promise.
    let rarities: Vec<Value> = Rarity::ALL
        .into_iter()
        .zip(weights.iter())
        .filter(|(r, _)| wizards.iter().any(|w| w.rarity == *r))
        .map(|(r, (_, w))| {
            json!({
                "key": r.key(), "name": r.name(), "emoji": r.emoji(), "colour": colour(r), "points": r.points(),
                "weight": r.weight(), "difficulty": r.difficulty(),
                "chance": if total_weight == 0 { 0.0 } else { (*w as f64 * 1000.0 / total_weight as f64).round() / 10.0 },
                "wizards": enabled_wizards.iter().filter(|x| x.rarity == r).count(),
            })
        })
        .collect();
    let own = super::super::var("VIZIER_FROG_CHANNELS").map(|raw| super::super::super::snitch::parse_weighted(&raw)).filter(|l| !l.is_empty());
    let (list, from) = match own {
        Some(list) => (list, "frog"),
        None => match super::super::var("VIZIER_SNITCH_CHANNELS").map(|raw| super::super::super::snitch::parse_weighted(&raw)) {
            Some(list) if !list.is_empty() => (list, "snitch"),
            _ => (Vec::new(), "none"),
        },
    };
    let channels: Vec<Value> = list
        .iter()
        .filter(|(id, _)| *id != super::super::super::weekly::SAFE_CORNER)
        .map(|(id, weight)| {
            let mut c = channel_json(&panel, *id);
            c["weight"] = json!(weight);
            c
        })
        .collect();
    let drops: Vec<Value> = drops
        .iter()
        .map(|(d, riddle)| {
            json!({
                "id": d.id,
                "ts": d.dropped_at,
                "closes_at": d.closes_at,
                "channel": channel_json(&panel, d.channel),
                "wizard_id": d.wizard_id,
                "wizard": d.wizard_name,
                "rarity": d.rarity,
                "points": d.points,
                "status": d.status,
                "winner": d.winner.map(|u| { let mut p = person(&panel, u); if p["name"].is_null() && !d.winner_name.is_empty() { p["name"] = json!(d.winner_name); } p }),
                "solved_secs": d.solved_secs,
                "serial": d.serial,
                "edition": d.edition,
                "answer_typed": d.answer_typed,
                "by": d.by.map(|b| person(&panel, b)),
                "riddle": riddle.as_ref().map(|r| json!({
                    "id": r.id, "text": r.riddle, "answer": r.canonical(), "answers": r.answers, "difficulty": r.difficulty,
                    "topic": r.topic, "retired": r.retired,
                })),
            })
        })
        .collect();
    ok(json!({
        "enabled": super::super::on("VIZIER_FROGS", false),
        "channels": channels,
        "channels_from": from,
        "open_minutes": super::super::number("VIZIER_FROG_OPEN_MINUTES", 5).clamp(1, 60),
        "set_bonus": super::super::number("VIZIER_FROG_SET_BONUS", 15),
        "modal_mode": super::super::super::frog::modal_mode(),
        "totals": totals,
        "rarities": rarities,
        "wizards": wizards.iter().map(|w| wizard_json(w, copies.get(&w.id).copied().unwrap_or(0))).collect::<Vec<_>>(),
        "max_wizards": store::MAX_WIZARDS,
        "max_name": store::MAX_NAME,
        "bank": bank,
        "drops": drops,
    }))
}

#[derive(Deserialize)]
pub struct WizardBody {
    #[serde(default)]
    name: String,
    #[serde(default)]
    rarity: String,
    #[serde(default)]
    image: String,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

fn check_wizard(body: &[u8]) -> Result<(String, Rarity, String, bool), ApiError> {
    let b: WizardBody = serde_json::from_slice(body).map_err(|_| ApiError::bad("That isn't a card."))?;
    let rarity = Rarity::from_key(&b.rarity).ok_or_else(|| ApiError::bad("Pick Common, Uncommon or Legendary."))?;
    let image = b.image.trim().to_string();
    if !image.is_empty() && super::super::media::info(&image).is_none() {
        return Err(ApiError::bad("That picture isn't in the library any more. Pick another."));
    }
    Ok((b.name, rarity, image, b.enabled))
}

fn wizard_facts(w: &Wizard) -> String {
    json!({ "name": w.name, "rarity": w.rarity, "image": w.image, "enabled": w.enabled }).to_string()
}

pub async fn create_wizard(axum::Extension(Caller(admin)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let (name, rarity, image, enabled) = check_wizard(&body)?;
    let saved = store::create_wizard(&frogs()?.lock(), &name, rarity, &image, enabled).map_err(ApiError::bad)?;
    super::super::log_change(&format!("frog:wizard:{}", saved.id), None, Some(&wizard_facts(&saved)), admin).map_err(ApiError::internal)?;
    tracing::info!("panel: {} added frog wizard {} ({})", admin, saved.name, saved.id);
    Ok((StatusCode::CREATED, axum::Json(wizard_json(&saved, 0))).into_response())
}

fn wizard_id(raw: &str) -> Result<i64, ApiError> {
    raw.parse::<i64>().ok().filter(|id| *id > 0).ok_or_else(|| ApiError::not_found("No such card."))
}

pub async fn update_wizard(Path(id): Path<String>, axum::Extension(Caller(admin)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let id = wizard_id(&id)?;
    let (name, rarity, image, enabled) = check_wizard(&body)?;
    let (old, saved, copies) = {
        let conn = frogs()?.lock();
        let old = store::wizard(&conn, id).ok_or_else(|| ApiError::not_found("No such card."))?;
        let saved = store::update_wizard(&conn, id, &name, rarity, &image, enabled).map_err(ApiError::bad)?;
        (old, saved, store::copies(&conn).get(&id).copied().unwrap_or(0))
    };
    if old != saved {
        super::super::log_change(&format!("frog:wizard:{}", id), Some(&wizard_facts(&old)), Some(&wizard_facts(&saved)), admin)
            .map_err(ApiError::internal)?;
        tracing::info!("panel: {} edited frog wizard {} ({})", admin, saved.name, id);
    }
    ok(wizard_json(&saved, copies))
}

pub async fn wizard_image(Path(id): Path<String>) -> Result<Response, ApiError> {
    let id = wizard_id(&id)?;
    let w = store::wizard(&frogs()?.lock(), id).ok_or_else(|| ApiError::not_found("No such card."))?;
    let found = tokio::task::spawn_blocking(move || store::thumbnail(&w.image, &w.slug)).await.map_err(ApiError::internal)?;
    let (bytes, ext) = found.ok_or_else(|| ApiError::not_found("This card has no picture."))?;
    let mime = match ext {
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "gif" => "image/gif",
        _ => "image/webp",
    };
    let mut res = Response::new(axum::body::Body::from(bytes));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=300"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline"));
    Ok(res)
}

#[derive(Deserialize)]
pub struct OwnersQuery {
    #[serde(default)]
    member: String,
    #[serde(default)]
    wizard: String,
}

fn frog_points(user: u64) -> i64 {
    super::super::super::house::db()
        .and_then(|db| {
            db.lock()
                .query_row(
                    "SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1 AND source = ?2",
                    rusqlite::params![user as i64, Source::Frog.key()],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
        })
        .unwrap_or(0)
}

/// Cards by member (`?member=<id>`) or owners by wizard (`?wizard=<id>`).
pub async fn owners(State(panel): State<Panel>, Query(q): Query<OwnersQuery>) -> ApiResult {
    if !q.member.trim().is_empty() {
        let user = super::parse_id(&q.member).ok_or_else(|| ApiError::bad("That isn't a member id."))?;
        let (cards, wizards) = {
            let conn = frogs()?.lock();
            (store::cards_of(&conn, user), store::wizards(&conn))
        };
        let enabled: Vec<&Wizard> = wizards.iter().filter(|w| w.enabled).collect();
        let collected = enabled.iter().filter(|w| cards.iter().any(|c| c.wizard_id == w.id)).count();
        let mut who = person(&panel, user);
        if who["name"].is_null() {
            if let Some(m) = panel.data.member(user).await {
                who = json!({ "id": user.to_string(), "name": m.name, "avatar": m.avatar, "known": true });
            }
        }
        return ok(json!({
            "member": who,
            "points": frog_points(user),
            "collected": collected,
            "of": enabled.len(),
            "cards": cards.iter().map(|c| json!({
                "serial": c.serial, "edition": c.edition, "wizard_id": c.wizard_id, "wizard": c.wizard_name, "rarity": c.rarity, "ts": c.ts, "drop_id": c.drop_id,
            })).collect::<Vec<_>>(),
        }));
    }
    if !q.wizard.trim().is_empty() {
        let id = wizard_id(q.wizard.trim())?;
        let (w, owners) = {
            let conn = frogs()?.lock();
            (store::wizard(&conn, id).ok_or_else(|| ApiError::not_found("No such card."))?, store::owners_of(&conn, id))
        };
        return ok(json!({
            "wizard": wizard_json(&w, owners.iter().map(|(_, s)| s.len() as i64).sum()),
            "owners": owners.iter().map(|(user, copies)| json!({
                "member": person(&panel, *user),
                "serials": copies.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
                "copies": copies.iter().map(|(s, e)| json!({ "serial": s, "edition": e })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        }));
    }
    Err(ApiError::bad("Pick a member or a card."))
}

#[derive(Deserialize)]
pub struct RetireBody {
    #[serde(default = "yes")]
    retired: bool,
}

pub async fn retire_riddle(Path(id): Path<String>, axum::Extension(Caller(admin)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let retired = if body.is_empty() {
        true
    } else {
        serde_json::from_slice::<RetireBody>(&body).map_err(|_| ApiError::bad("Say whether to retire it."))?.retired
    };
    let (old, now) = {
        let conn = frogs()?.lock();
        let old = store::riddle(&conn, &id).ok_or_else(|| ApiError::not_found("No such riddle."))?;
        store::set_retired(&conn, &id, retired).map_err(ApiError::internal)?;
        (old.clone(), store::riddle(&conn, &id).unwrap_or(old))
    };
    if old.retired != now.retired {
        let facts = |r: &store::Riddle| json!({ "retired": r.retired, "answer": r.canonical(), "difficulty": r.difficulty }).to_string();
        super::super::log_change(&format!("frog:riddle:{}", id), Some(&facts(&old)), Some(&facts(&now)), admin).map_err(ApiError::internal)?;
        tracing::info!("panel: {} {} riddle {}", admin, if retired { "retired" } else { "restored" }, id);
    }
    ok(json!({ "id": now.id, "retired": now.retired, "bank": store::bank_stats(&frogs()?.lock()) }))
}

#[derive(Deserialize)]
pub struct DropBody {
    #[serde(default)]
    channel_id: String,
}

/// "Drop a frog now", for testing: a real frog in the channel picked.
pub async fn drop_now(State(panel): State<Panel>, axum::Extension(Caller(admin)): axum::Extension<Caller>, body: axum::body::Bytes) -> ApiResult {
    let b: DropBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Pick a channel."))?;
    if b.channel_id.trim().is_empty() {
        return Err(ApiError::bad("Pick a channel."));
    }
    let channel = super::check_channel(&panel, &b.channel_id, false).map_err(ApiError::bad)?;
    let channel: u64 = channel.parse().map_err(|_| ApiError::bad("Pick a channel."))?;
    if channel == super::super::super::weekly::SAFE_CORNER
        || panel.data.channels().iter().any(|c| c.id == channel.to_string() && c.name.to_lowercase().contains("safe-corner"))
    {
        return Err(ApiError::bad("Frogs never drop in #safe-corner."));
    }
    let dropped = panel.data.drop_frog(channel, admin).await.map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    let facts = json!({ "channel_id": channel.to_string(), "wizard": dropped.wizard_name, "rarity": dropped.rarity }).to_string();
    super::super::log_change(&format!("frog:drop:{}", dropped.id), None, Some(&facts), admin).map_err(ApiError::internal)?;
    ok(json!({ "id": dropped.id, "wizard": dropped.wizard_name, "rarity": dropped.rarity, "channel": channel_json(&panel, channel), "status": dropped.status, "open": dropped.status == Status::Open }))
}

/// How a frog change reads in the activity log.
pub fn audit_entry(panel: &Panel, e: &super::super::AuditEntry) -> Map<String, Value> {
    let body = |b: &Option<String>| b.as_deref().and_then(|t| serde_json::from_str::<Value>(t).ok()).unwrap_or(Value::Null);
    let (old, new) = (body(&e.old), body(&e.new));
    let facts = if new.is_null() { &old } else { &new };
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut obj = Map::new();
    obj.insert("section".into(), json!({ "id": "frogs", "title": "Chocolate Frogs", "icon": "🐸" }));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    let rest = e.key.strip_prefix("frog:").unwrap_or("");
    if rest.starts_with("wizard:") {
        obj.insert("label".into(), json!(format!("Frog card “{}”", text(facts, "name"))));
        let change = if old.is_null() {
            "Added".to_string()
        } else {
            let mut parts = Vec::new();
            if old.get("name") != new.get("name") {
                parts.push(format!("renamed from {}", text(&old, "name")));
            }
            if old.get("rarity") != new.get("rarity") {
                parts.push(format!("now {}", text(&new, "rarity")));
            }
            if old.get("image") != new.get("image") {
                parts.push("picture changed".to_string());
            }
            if old.get("enabled") != new.get("enabled") {
                parts.push(if new.get("enabled").and_then(Value::as_bool) == Some(true) { "switched on".into() } else { "switched off".into() });
            }
            let joined = parts.join(", ");
            let mut chars = joined.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => "Edited".to_string(),
            }
        };
        obj.insert("change".into(), json!(change));
    } else if let Some(id) = rest.strip_prefix("riddle:") {
        obj.insert("label".into(), json!(format!("Riddle {}", id)));
        let retired = new.get("retired").and_then(Value::as_bool) == Some(true);
        obj.insert("change".into(), json!(if retired { format!("Retired · answer {}", text(facts, "answer")) } else { "Back in play".to_string() }));
    } else {
        let channel = text(facts, "channel_id");
        let name = panel.data.channels().into_iter().find(|c| c.id == channel).map(|c| format!("#{}", c.name)).unwrap_or_else(|| "a channel".into());
        obj.insert("label".into(), json!("Test frog drop"));
        obj.insert("change".into(), json!(format!("Dropped {} in {}", text(facts, "wizard"), name)));
    }
    obj
}
