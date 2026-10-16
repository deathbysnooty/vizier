//! The family tree, as a page anybody can open.
//!
//! Two views of one thing, which is why it is one page rather than two:
//!
//! * `/love/u/<handle>` opens on somebody's own household - their spouses,
//!   parents and children laid out and readable. This is the link `/family`
//!   hands a member, because it is what they actually want to see.
//! * `/love` opens on the whole server: every household at once, settling
//!   itself. Good to browse, and a poor landing page once there are hundreds.
//!
//! No Discord ids leave this module. A member is named by the same opaque
//! handle the dragon page uses, so a link somebody pastes carries nothing about
//! their account - see [`super::housecup::handle`].
use axum::extract::{Path, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use super::super::live_ui;
use super::Panel;
use crate::channels::discord::marriage;
use crate::channels::discord::marriage_store as store;

fn enabled() -> bool {
    crate::channels::discord::marriage::on()
}

/// Everyone the tree draws, and what joins them.
///
/// Built from the marriage store and the gateway's own cache of names and
/// faces - never a call to Discord per visitor.
fn tree(panel: &Panel) -> Value {
    let Some(db) = store::db() else {
        return json!({ "people": [], "links": [], "households": 0 });
    };
    let conn = db.lock();
    let unions = store::all_unions(&conn);
    let kin = store::all_kin(&conn);
    drop(conn);

    let mut ids: Vec<u64> = Vec::new();
    for (a, b, _) in &unions {
        ids.push(*a);
        ids.push(*b);
    }
    for (p, c) in &kin {
        ids.push(*p);
        ids.push(*c);
    }
    ids.sort_unstable();
    ids.dedup();

    let people: Vec<Value> = ids
        .iter()
        .map(|id| {
            let (name, face) = panel.cached_face(*id).unwrap_or_else(|| (String::from("someone"), String::new()));
            json!({ "handle": super::housecup::handle(*id), "name": name, "avatar": face })
        })
        .collect();
    let links: Vec<Value> = unions
        .iter()
        .map(|(a, b, since)| {
            json!({ "a": super::housecup::handle(*a), "b": super::housecup::handle(*b),
                    "kind": "marriage", "since": since })
        })
        .chain(kin.iter().map(|(p, c)| {
            json!({ "a": super::housecup::handle(*p), "b": super::housecup::handle(*c), "kind": "child" })
        }))
        .collect();

    json!({
        "people": people,
        "links": links,
        "households": marriage::households(&unions, &kin).len(),
        "now": chrono::Utc::now().timestamp(),
    })
}

async fn tree_json(State(panel): State<Panel>) -> Response {
    if !enabled() {
        return (StatusCode::NOT_FOUND, "Not here.").into_response();
    }
    let body = tree(&panel);
    let mut res = Response::new(super::text_body(serde_json::to_string(&body).unwrap_or_default().into()));
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    res
}

/// The page itself. The same file at every address it answers on: which
/// household it opens on is read out of the path by the page.
async fn page() -> Response {
    if !enabled() {
        return (StatusCode::NOT_FOUND, "The family tree is switched off.").into_response();
    }
    let mut res = Response::new(super::text_body(live_ui::tree_page()));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // The same policy the hall sets: a CDN for the layout library, Google
    // Fonts, and Discord's own avatars. The panel's stricter one is left alone
    // for every other page.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; img-src 'self' data: https://cdn.discordapp.com; \
             script-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com; \
             style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
             font-src https://fonts.gstatic.com; connect-src 'self'",
        ),
    );
    res
}

async fn one_page(Path(_handle): Path<String>) -> Response {
    page().await
}

pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/love", get(page))
        .route("/love/", get(page))
        .route("/love/u/{handle}", get(one_page))
        .route("/love/api/tree.json", get(tree_json))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_body_names_nobody_by_their_discord_id() {
        // Handles only: a link somebody pastes carries nothing about an account.
        let unions = vec![(111_222_333_444_555_666u64, 222_333_444_555_666_777u64, 10i64)];
        let kin = vec![(111_222_333_444_555_666u64, 333_444_555_666_777_888u64)];
        let body = json!({
            "links": unions.iter().map(|(a, b, s)| json!({
                "a": super::super::housecup::handle(*a), "b": super::super::housecup::handle(*b),
                "kind": "marriage", "since": s })).collect::<Vec<_>>(),
        });
        let text = serde_json::to_string(&body).expect("json");
        for (a, b, _) in &unions {
            assert!(!text.contains(&a.to_string()) && !text.contains(&b.to_string()), "an id reached the page");
        }
        for (p, c) in &kin {
            assert!(!text.contains(&p.to_string()) && !text.contains(&c.to_string()));
        }
    }

    #[test]
    fn the_tree_is_empty_rather_than_broken_before_anybody_marries() {
        let body = json!({ "people": [], "links": [], "households": 0 });
        assert_eq!(body["people"].as_array().map(Vec::len), Some(0));
        assert_eq!(body["households"], 0, "nobody has married yet, and that is not an error");
    }
}
