//! The Bot behaviour page: the parts of the AI agent's configuration that shape
//! how it talks - its prompt, core notes, chattiness, model and limits.
//!
//! What was found about when they apply: the agent process reads its whole
//! config and its CORE once, when it starts (`VizierAgent::new` keeps a copy),
//! and the Discord client itself runs inside that process. Saving here writes
//! the stored config only, so chat picks the change up after a restart. The one
//! exception is `model`: quiz drafting and the weekly scan read the stored
//! config every time, so they use a new model at once.
//!
//! Only these fields ever reach the browser. Provider, credentials, tokens,
//! tools, owner and sharing are never read out or written.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{ApiError, ApiResult, Caller, Panel, ok};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentSettings {
    pub name: String,
    /// Read into "You are {name}, {description}."
    pub description: String,
    pub system_prompt: String,
    /// The agent's own notes document, which it can also rewrite itself.
    pub core: String,
    pub model: String,
    /// How many tool steps one reply may take.
    pub thinking_depth: usize,
    /// 0..=1: how often it answers a message that wasn't addressed to it.
    pub silent_read_initiative_chance: f32,
    pub max_tokens: Option<u64>,
}

pub const FIELDS: [&str; 8] =
    ["name", "description", "system_prompt", "core", "model", "thinking_depth", "silent_read_initiative_chance", "max_tokens"];

pub fn label(field: &str) -> &'static str {
    match field {
        "name" => "Name",
        "description" => "Description",
        "system_prompt" => "System prompt",
        "core" => "Core notes",
        "model" => "Model",
        "thinking_depth" => "Thinking depth",
        "silent_read_initiative_chance" => "Chiming in",
        "max_tokens" => "Max reply tokens",
        _ => "Setting",
    }
}

fn live_flags() -> Value {
    // Chat reads everything at start-up; see the module notes.
    Value::Object(FIELDS.iter().map(|f| (f.to_string(), Value::Bool(false))).collect())
}

pub async fn get(State(panel): State<Panel>) -> ApiResult {
    let Some(settings) = panel.data.agent_settings().await else {
        return Err(unavailable());
    };
    ok(json!({
        "settings": settings,
        "live": live_flags(),
        "applies": "restart",
        "notes": {
            "model": "Quiz drafting and the weekly scan use a new model straight away; chat switches after a restart.",
            "core": "The bot also rewrites its core notes itself when it learns something.",
        },
    }))
}

fn unavailable() -> ApiError {
    ApiError(StatusCode::SERVICE_UNAVAILABLE, "The bot's AI settings aren't available right now.".into())
}

fn text(value: &Value, field: &str) -> Result<String, ApiError> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Null => Ok(String::new()),
        _ => Err(ApiError::bad(format!("{} must be text.", label(field)))),
    }
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Applies a patch from the page to the current settings, checking each field.
pub fn apply(current: &AgentSettings, patch: &Map<String, Value>) -> Result<AgentSettings, ApiError> {
    let mut next = current.clone();
    for (field, value) in patch {
        match field.as_str() {
            "name" => {
                let v = text(value, field)?.trim().to_string();
                if v.is_empty() || chars(&v) > 64 {
                    return Err(ApiError::bad("The name needs 1 to 64 characters."));
                }
                next.name = v;
            }
            "description" => {
                let v = text(value, field)?.trim().to_string();
                if chars(&v) > 300 {
                    return Err(ApiError::bad("Keep the description under 300 characters."));
                }
                next.description = v;
            }
            "system_prompt" => {
                let v = text(value, field)?;
                if chars(&v) > 24_000 {
                    return Err(ApiError::bad("The system prompt is over 24,000 characters."));
                }
                next.system_prompt = v;
            }
            "core" => {
                let v = text(value, field)?;
                if v.trim().is_empty() {
                    return Err(ApiError::bad("The core notes can't be empty: the bot would lose everything it keeps there."));
                }
                if chars(&v) > 60_000 {
                    return Err(ApiError::bad("The core notes are over 60,000 characters."));
                }
                next.core = v;
            }
            "model" => {
                let v = text(value, field)?.trim().to_string();
                if v.is_empty() || chars(&v) > 200 || v.chars().any(|c| c.is_whitespace() || c.is_control()) {
                    return Err(ApiError::bad("The model is a name without spaces, like provider/model-name."));
                }
                next.model = v;
            }
            "thinking_depth" => {
                let n = value.as_u64().filter(|n| (1..=64).contains(n));
                next.thinking_depth = n.ok_or_else(|| ApiError::bad("Thinking depth is a whole number from 1 to 64."))? as usize;
            }
            "silent_read_initiative_chance" => {
                let n = value.as_f64().filter(|n| n.is_finite() && (0.0..=1.0).contains(n));
                next.silent_read_initiative_chance =
                    n.ok_or_else(|| ApiError::bad("Chiming in is a chance from 0 to 1."))? as f32;
            }
            "max_tokens" => {
                next.max_tokens = match value {
                    Value::Null => None,
                    v => Some(
                        v.as_u64()
                            .filter(|n| (1..=1_000_000).contains(n))
                            .ok_or_else(|| ApiError::bad("Max reply tokens is empty (the provider's default) or 1 to 1,000,000."))?,
                    ),
                };
            }
            "base" => {}
            other => {
                return Err(ApiError::bad(format!("\"{}\" can't be changed from the panel.", other)));
            }
        }
    }
    Ok(next)
}

fn as_text(s: &AgentSettings, field: &str) -> Option<String> {
    Some(match field {
        "name" => s.name.clone(),
        "description" => s.description.clone(),
        "system_prompt" => s.system_prompt.clone(),
        "core" => s.core.clone(),
        "model" => s.model.clone(),
        "thinking_depth" => s.thinking_depth.to_string(),
        "silent_read_initiative_chance" => format!("{}", (s.silent_read_initiative_chance * 1000.0).round() / 1000.0),
        "max_tokens" => return s.max_tokens.map(|n| n.to_string()),
        _ => return None,
    })
}

pub async fn put(
    State(panel): State<Panel>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let patch: Map<String, Value> = serde_json::from_slice(&body)
        .ok()
        .and_then(|v: Value| v.as_object().cloned())
        .ok_or_else(|| ApiError::bad("Send the fields to change as a JSON object."))?;
    let Some(current) = panel.data.agent_settings().await else {
        return Err(unavailable());
    };
    // The page sends what it loaded for the long texts; the bot may have
    // rewritten its core since, and that must not be overwritten unseen.
    if let Some(base) = patch.get("base").and_then(Value::as_object) {
        let base: HashMap<&str, &str> = base.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.as_str(), v))).collect();
        for field in ["core", "system_prompt", "description", "name"] {
            if let (Some(seen), Some(now)) = (base.get(field), as_text(&current, field)) {
                if patch.contains_key(field) && *seen != now {
                    return Err(ApiError(
                        StatusCode::CONFLICT,
                        format!(
                            "{} changed since you opened the page{}. Reload to see the latest before saving.",
                            label(field),
                            if field == "core" { " (the bot updates its own notes)" } else { "" }
                        ),
                    ));
                }
            }
        }
    }
    let next = apply(&current, &patch)?;
    let changed: Vec<&str> = FIELDS.iter().copied().filter(|f| as_text(&current, f) != as_text(&next, f)).collect();
    if !changed.is_empty() {
        panel.data.save_agent_settings(&next).await.map_err(ApiError::internal)?;
        for field in &changed {
            let _ = super::super::log_change(
                &format!("agent:{}", field),
                as_text(&current, field).as_deref(),
                as_text(&next, field).as_deref(),
                user,
            );
        }
        tracing::info!("panel: {} changed the bot's {}", user, changed.join(", "));
    }
    ok(json!({
        "settings": next,
        "live": live_flags(),
        "changed": changed,
        "restart_needed": !changed.is_empty(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> AgentSettings {
        AgentSettings {
            name: "Loduchand".into(),
            description: "the MLCI bot".into(),
            system_prompt: "Be nice.".into(),
            core: "# CORE".into(),
            model: "openrouter/some-model".into(),
            thinking_depth: 8,
            silent_read_initiative_chance: 0.05,
            max_tokens: None,
        }
    }

    fn patch(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn only_safe_fields_change() {
        let next = apply(&current(), &patch(json!({ "silent_read_initiative_chance": 0.2, "max_tokens": 900 }))).unwrap();
        assert_eq!(next.silent_read_initiative_chance, 0.2);
        assert_eq!(next.max_tokens, Some(900));
        assert_eq!(next.name, "Loduchand");
        for bad in [
            json!({ "provider": "openai" }),
            json!({ "discord_token": "x" }),
            json!({ "silent_read_initiative_chance": 1.5 }),
            json!({ "thinking_depth": 0 }),
            json!({ "thinking_depth": 3.5 }),
            json!({ "max_tokens": 0 }),
            json!({ "core": "  " }),
            json!({ "model": "has space" }),
            json!({ "name": "" }),
        ] {
            assert!(apply(&current(), &patch(bad.clone())).is_err(), "{bad}");
        }
        assert_eq!(apply(&current(), &patch(json!({ "max_tokens": null }))).unwrap().max_tokens, None);
    }
}
