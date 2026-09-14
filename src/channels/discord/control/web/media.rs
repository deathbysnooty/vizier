//! The picture library for scheduled posts: upload, list, show, delete.
//!
//! Uploads come as multipart form data (field `file`) or as the raw bytes with
//! an `X-Filename` header. What a file really is comes from its first bytes,
//! never from its name or declared type, and only PNG, JPEG, GIF and WebP pass.
//! Pictures are served only to a signed-in admin.

use axum::extract::{FromRequest, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use super::super::media::{self, MediaInfo};
use super::super::reminders;
use super::{ApiError, ApiResult, Caller, Panel, ok};

/// The body limit on the upload route: the largest picture plus room for the form around it.
pub const UPLOAD_LIMIT: usize = media::MAX_BYTES + 64 * 1024;

fn too_big() -> ApiError {
    ApiError(StatusCode::PAYLOAD_TOO_LARGE, format!("Pictures can be at most {} MB.", media::MAX_BYTES / (1024 * 1024)))
}

fn valid_id(raw: &str) -> Option<&str> {
    (raw.len() == 16 && raw.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())).then_some(raw)
}

/// The reminders that use a picture, as (id, name).
fn used_by(id: &str) -> Vec<(i64, String)> {
    reminders::list().into_iter().filter(|r| r.images.iter().any(|i| i == id)).map(|r| (r.id, r.name)).collect()
}

fn info_json(m: &MediaInfo) -> serde_json::Value {
    json!({
        "id": m.id,
        "name": m.name,
        "mime": m.mime,
        "size": m.size,
        "created_ts": m.created_ts,
        "created_by": m.created_by,
        "url": format!("/api/media/{}", m.id),
        "used_by": used_by(&m.id).into_iter().map(|(id, name)| json!({ "id": id, "name": name })).collect::<Vec<_>>(),
    })
}

pub async fn list() -> ApiResult {
    ok(json!({
        "items": media::list().iter().map(info_json).collect::<Vec<_>>(),
        "max_bytes": media::MAX_BYTES,
        "max_files": media::MAX_FILES,
    }))
}

pub async fn upload(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, req: Request) -> ApiResult {
    let kind = req.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    let (name, bytes) = if kind.starts_with("multipart/form-data") {
        let mut form = axum_extra::extract::Multipart::from_request(req, &panel)
            .await
            .map_err(|_| ApiError::bad("That upload couldn't be read. Try again."))?;
        let mut found = None;
        while let Some(mut field) = form.next_field().await.map_err(|e| {
            if e.body_text().to_ascii_lowercase().contains("limit") { too_big() } else { ApiError::bad("That upload couldn't be read. Try again.") }
        })? {
            if field.name() != Some("file") {
                continue;
            }
            let name = field.file_name().unwrap_or("picture").to_string();
            let mut data = Vec::new();
            while let Some(chunk) = field.chunk().await.map_err(|e| {
                if e.body_text().to_ascii_lowercase().contains("limit") { too_big() } else { ApiError::bad("That upload couldn't be read. Try again.") }
            })? {
                if data.len() + chunk.len() > media::MAX_BYTES {
                    return Err(too_big());
                }
                data.extend_from_slice(&chunk);
            }
            found = Some((name, data));
            break;
        }
        found.ok_or_else(|| ApiError::bad("Choose a picture to upload."))?
    } else {
        let name = req
            .headers()
            .get("x-filename")
            .and_then(|v| v.to_str().ok())
            .map(|v| percent_decode(v))
            .unwrap_or_else(|| "picture".to_string());
        let data = axum::body::to_bytes(req.into_body(), media::MAX_BYTES).await.map_err(|_| too_big())?;
        (name, data.to_vec())
    };
    let saved = media::save(&name, &bytes, user).map_err(ApiError::bad)?;
    let facts = json!({ "name": saved.name, "size": saved.size, "mime": saved.mime });
    super::super::log_change(&format!("media:{}", saved.id), None, Some(&facts.to_string()), user).map_err(ApiError::internal)?;
    tracing::info!("panel: {} uploaded picture {} ({} bytes)", user, saved.id, saved.size);
    Ok((StatusCode::CREATED, axum::Json(info_json(&saved))).into_response())
}

/// `X-Filename` may be percent-encoded so names outside ASCII survive a header.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn serve(Path(id): Path<String>) -> Result<Response, ApiError> {
    let id = valid_id(&id).ok_or_else(|| ApiError::not_found("No such picture."))?;
    let (info, bytes) = media::get(id).ok_or_else(|| ApiError::not_found("No such picture."))?;
    let mut res = Response::new(axum::body::Body::from(bytes));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(&info.mime).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    // An id always means the same bytes, so the browser can keep it; only for this admin's browser.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=31536000, immutable"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("inline"));
    Ok(res)
}

pub async fn delete(Path(id): Path<String>, axum::Extension(Caller(user)): axum::Extension<Caller>) -> ApiResult {
    let id = valid_id(&id).ok_or_else(|| ApiError::not_found("No such picture."))?;
    let info = media::info(id).ok_or_else(|| ApiError::not_found("No such picture."))?;
    let card = super::super::super::frog_store::db()
        .and_then(|db| super::super::super::frog_store::wizards(&db.lock()).into_iter().find(|w| w.image == id));
    if let Some(w) = card {
        return Err(ApiError(StatusCode::CONFLICT, format!("{} is the picture on the {} frog card. Give that card another picture first.", info.name, w.name)));
    }
    let users = used_by(id);
    if !users.is_empty() {
        let names: Vec<String> = users.iter().map(|(_, n)| format!("“{}”", n)).collect();
        let list = match names.len() {
            1 => names[0].clone(),
            _ => format!("{} and {}", names[..names.len() - 1].join(", "), names[names.len() - 1]),
        };
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("{} is used by {}. Take it out of {} first.", info.name, list, if users.len() == 1 { "that reminder" } else { "those reminders" }),
        ));
    }
    if !media::delete(id) {
        return Err(ApiError::not_found("No such picture."));
    }
    let facts = json!({ "name": info.name, "size": info.size, "mime": info.mime });
    super::super::log_change(&format!("media:{}", id), Some(&facts.to_string()), None, user).map_err(ApiError::internal)?;
    tracing::info!("panel: {} deleted picture {}", user, id);
    ok(json!({ "ok": true }))
}
