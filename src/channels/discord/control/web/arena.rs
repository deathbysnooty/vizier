//! The Arena page's one action: "Start a battle royale now" - a lobby in the
//! fight channel this instant, tagging the four houses, exactly as the daily
//! battle does. Starts go to the activity log as `battle:now:<channel>`.

use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::super::super::battle;
use super::{ApiError, ApiResult, Caller, Panel, ok};

/// The channel a battle would open in, as the panel can see it: the fight
/// channel, else one named `fight-fight-fight`. The bot resolves it again for
/// real when the battle starts - this is only for saying where beforehand. A
/// fight channel the panel can't put a name to is still the arena, so it comes
/// back with a blank name rather than as nothing at all.
fn channel_json(panel: &Panel) -> Value {
    let channels = panel.data.channels();
    if let Some(id) = super::super::id("VIZIER_FIGHT_CHANNEL") {
        let name = channels.iter().find(|c| c.id == id.to_string()).map(|c| c.name.clone());
        return json!({ "id": id.to_string(), "name": name });
    }
    match channels.into_iter().find(|c| c.name == "fight-fight-fight") {
        Some(c) => json!({ "id": c.id, "name": c.name }),
        None => Value::Null,
    }
}

/// What the "start one now" dialog needs: where it will post, how long a lobby
/// runs by default and who it can tag.
pub async fn overview(State(panel): State<Panel>) -> ApiResult {
    ok(json!({
        "channel": channel_json(&panel),
        "min_minutes": battle::MIN_WAIT,
        "max_minutes": battle::max_lobby_minutes(),
        "default_minutes": battle::default_lobby_minutes(),
        "pings": [
            { "key": "houses", "emoji": "🏰", "label": "All four houses", "about": "Tags every house role, like the daily battle." },
            { "key": "warriors", "emoji": "⚔️", "label": "Warriors role", "about": "Only the people who asked to be tagged for fights." },
            { "key": "none", "emoji": "🤫", "label": "Nobody", "about": "The lobby goes up quietly, with no tag at all." },
        ],
    }))
}

#[derive(Deserialize)]
pub struct StartBody {
    #[serde(default)]
    minutes: Option<i64>,
    #[serde(default)]
    ping: Option<String>,
    #[serde(default)]
    theme: Option<String>,
}

/// Opens a battle royale lobby in the arena now.
pub async fn start(
    State(panel): State<Panel>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let b: StartBody = if body.is_empty() {
        StartBody { minutes: None, ping: None, theme: None }
    } else {
        serde_json::from_slice(&body).map_err(|_| ApiError::bad("That isn't a battle to start."))?
    };
    let started = panel
        .data
        .start_battle(b.minutes, b.ping.as_deref(), b.theme.as_deref())
        .await
        .map_err(|e| ApiError(StatusCode::CONFLICT, e))?;
    let channel = panel.data.channels().into_iter().find(|c| c.id == started.channel.to_string());
    let name = channel.as_ref().map(|c| format!("#{}", c.name)).unwrap_or_else(|| "the arena".into());
    let facts = json!({
        "channel_id": started.channel.to_string(),
        "channel": channel.as_ref().map(|c| c.name.clone()),
        "minutes": started.minutes,
        "ping": started.ping,
        "ping_label": started.ping_label,
        "theme": started.theme,
        "theme_label": started.theme_label,
    })
    .to_string();
    super::super::log_change(&format!("battle:now:{}", started.channel), None, Some(&facts), admin)
        .map_err(ApiError::internal)?;
    tracing::info!("panel: {} started a battle royale in {} ({} min, {})", admin, name, started.minutes, started.ping);
    ok(json!({
        "channel": { "id": started.channel.to_string(), "name": channel.map(|c| c.name) },
        "minutes": started.minutes,
        "ping": started.ping,
        "ping_label": started.ping_label,
        "theme": started.theme,
        "theme_label": started.theme_label,
    }))
}

/// "Started a battle royale in #fight-fight-fight (10 min, the four houses)".
pub fn audit_entry(e: &super::super::AuditEntry) -> Map<String, Value> {
    let facts: Value = e.new.as_deref().and_then(|t| serde_json::from_str(t).ok()).unwrap_or(Value::Null);
    let text = |k: &str| facts.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let where_ = match facts.get("channel").and_then(Value::as_str) {
        Some(name) if !name.is_empty() => format!("#{}", name),
        _ => "the arena".to_string(),
    };
    let minutes = facts.get("minutes").and_then(Value::as_i64).unwrap_or(0);
    let mut how = Vec::new();
    if minutes > 0 {
        how.push(format!("{} min", minutes));
    }
    let tagged = text("ping_label");
    if !tagged.is_empty() {
        how.push(tagged);
    }
    let style = text("theme_label");
    if !style.is_empty() {
        how.push(style);
    }
    let mut obj = Map::new();
    obj.insert("label".into(), json!("Battle royale"));
    obj.insert("section".into(), json!({ "id": "arena", "title": "Arena", "icon": "⚔️" }));
    obj.insert(
        "change".into(),
        json!(if how.is_empty() {
            format!("Started a battle royale in {}", where_)
        } else {
            format!("Started a battle royale in {} ({})", where_, how.join(", "))
        }),
    );
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}
