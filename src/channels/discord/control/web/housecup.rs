//! The public House Cup page, at `/housecup/<token>`.
//!
//! The second public corner of the panel's server, after the sudoku page: no
//! sign-in, no cookie, no `X-Panel` header and nothing under `/api`. It is the
//! link `/housecup` in Discord hands out, and it is meant to be left open on a
//! second screen all month.
//!
//! WHY THERE IS A TOKEN IN THE ADDRESS. Everything on the page - display names,
//! their points, who holds which Chocolate Frog cards - is the sort of thing the
//! bot says in public Discord channels, so a member seeing it is no surprise.
//! But a bare `/housecup` on the panel's own host is a guess away for anyone who
//! ever sees the panel's address, including people who have never been on the
//! server, and a per-member points table is not something the bot publishes to
//! the open web. So the page sits behind one long random token, minted once and
//! kept in house.db: the link is unguessable, stable (so it can be pinned in a
//! channel and bookmarked), shareable, and replaceable if it ever gets out.
//!
//! What it gives away, and no more: display names, points, card counts. No user
//! ids, no join dates, no message counts, nothing about who is looking.
//!
//! The whole state is computed behind a short cache (`VIZIER_HOUSECUP_CACHE_SECS`,
//! ten seconds), so a hundred people with the page open cost one pass over
//! SQLite per window rather than a hundred.

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::{Value, json};

use crate::channels::discord::control;

use super::super::super::frog_store::{self, Rarity};
use super::super::super::house::{self, HOUSES};
use super::super::super::points;
use super::super::ui;
use super::{Panel, scorers};

// Built into the binary; a copy in `<runtime>/ui` is served instead when there
// is one - see [`super::super::ui`].
const PAGE_HTML: &str = include_str!("../ui/housecup.html");
const PAGE_CSS: &str = include_str!("../ui/housecup.css");
const PAGE_JS: &str = include_str!("../ui/housecup.js");

/// Where the page's own address is kept, in house.db's `meta`.
const TOKEN_KEY: &str = "housecup_token";

/// Names in each house's top list.
const TOP_LIST: usize = 10;
/// Collectors named per house before the rest become "and N more".
const COLLECTORS: usize = 12;

/// Whether the page and `/housecup` work at all.
pub fn enabled() -> bool {
    control::on("VIZIER_HOUSECUP", true)
}

/// How long one computed state is served for.
fn cache_secs() -> u64 {
    control::number("VIZIER_HOUSECUP_CACHE_SECS", 10).clamp(1, 300)
}

// --- the address ----------------------------------------------------------------

/// One minting at a time, so two members running `/housecup` at once can't end
/// up with two different links.
static MINT: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

fn plausible(token: &str) -> bool {
    token.len() >= 24 && token.len() <= 64 && token.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The page's token: the one in house.db, or a fresh one written there now.
/// `None` only when the house store isn't open.
pub fn token() -> Option<String> {
    let _minting = MINT.lock();
    if let Some(found) = house::meta_get(TOKEN_KEY).filter(|t| plausible(t)) {
        return Some(found);
    }
    let fresh = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
    house::meta_set(TOKEN_KEY, &fresh);
    // Reading it back proves the store took it: without that the link would be
    // handed out and then not open.
    house::meta_get(TOKEN_KEY).filter(|t| plausible(t))
}

/// The address to give a member, when the panel has a public one.
pub fn link() -> Option<String> {
    Some(format!("{}/housecup/{}", super::panel_url()?, token()?))
}

/// True when `given` is the page's token. Length-independent comparison, so the
/// reply time says nothing about how much of a guess was right.
fn opens(given: &str) -> bool {
    let Some(real) = house::meta_get(TOKEN_KEY).filter(|t| plausible(t)) else { return false };
    if !plausible(given) || given.len() != real.len() {
        return false;
    }
    given.bytes().zip(real.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

// --- what the page shows ----------------------------------------------------------

/// One kind of Chocolate Frog card in play.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CardType {
    pub wizard_id: i64,
    pub name: String,
    pub rarity: Rarity,
}

/// Someone holding cards for a house. Their name is added later; nothing here
/// leaves this module with an id on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Collector {
    pub user: u64,
    /// Every card they hold.
    pub cards: i64,
    /// How many of the cards in play they hold at least one of.
    pub types: i64,
    /// All of them: they can `/sellset` on their own.
    pub full_set: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HouseState {
    pub key: &'static str,
    pub total: i64,
    /// The top scorers this month, biggest first: (member, points).
    pub top: Vec<(u64, i64)>,
    /// Every card the house's members hold.
    pub cards: i64,
    /// How many of each card in play, in `Cup::types` order.
    pub held: Vec<i64>,
    /// Who holds them, biggest first.
    pub collectors: Vec<Collector>,
}

/// Everything the page draws, before any name is put to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cup {
    /// Midnight on the 1st, India time: what "this month" counts from.
    pub since: i64,
    pub types: Vec<CardType>,
    /// The four houses, most points first.
    pub houses: Vec<HouseState>,
}

/// The cards in play, in the order the bot lists them.
pub fn types_in_play(frog: &Connection) -> Vec<CardType> {
    frog_store::wizards(frog)
        .into_iter()
        .filter(|w| w.enabled)
        .map(|w| CardType { wizard_id: w.id, name: w.name, rarity: w.rarity })
        .collect()
}

/// Who holds how many of what: (member, card, copies). Spent cards are nobody's.
pub fn holdings(frog: &Connection) -> Vec<(u64, i64, i64)> {
    frog.prepare("SELECT user_id, wizard_id, COUNT(*) FROM cards WHERE status = 'owned' GROUP BY user_id, wizard_id")
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?)))?.collect()
        })
        .unwrap_or_default()
}

/// The whole state, from the house ledger and a snapshot of the frog store.
///
/// Cards follow their holder: a card counts for whichever house its holder is
/// in NOW, not the one they were in when they caught it. Someone who stepped
/// out with `/houseopt` is a Muggle, so neither their points nor their cards
/// count for anybody, the same rule the rest of the house tables use.
pub fn assemble(
    house_conn: &Connection,
    now: i64,
    optouts: &HashSet<u64>,
    types: Vec<CardType>,
    holdings: &[(u64, i64, i64)],
) -> rusqlite::Result<Cup> {
    let since = points::month_start(now);
    let members = scorers::read_members(house_conn)?;
    let slot: HashMap<i64, usize> = types.iter().enumerate().map(|(i, t)| (t.wizard_id, i)).collect();

    // Gather each house's holders first, so one pass over the cards does for all four.
    let mut by_house: HashMap<&'static str, (i64, Vec<i64>, HashMap<u64, (i64, i64)>)> =
        HOUSES.iter().map(|h| (h.key, (0, vec![0; types.len()], HashMap::new()))).collect();
    for (user, wizard, copies) in holdings {
        if *copies <= 0 || optouts.contains(user) {
            continue;
        }
        let Some(key) = members.get(user).and_then(|k| house::house(k)).map(|h| h.key) else { continue };
        let Some((cards, held, who)) = by_house.get_mut(key) else { continue };
        *cards += copies;
        let entry = who.entry(*user).or_insert((0, 0));
        entry.0 += copies;
        if let Some(i) = slot.get(wizard) {
            held[*i] += copies;
            entry.1 += 1;
        }
    }

    let wanted = types.len() as i64;
    let mut houses = Vec::new();
    for h in HOUSES {
        let total = points::house_total(house_conn, h.key, since, i64::MAX)?;
        let top: Vec<(u64, i64)> = points::top_members(house_conn, h.key, since, i64::MAX)?
            .into_iter()
            .filter(|(user, _)| !optouts.contains(user))
            .take(TOP_LIST)
            .collect();
        let (cards, held, who) = by_house.remove(h.key).unwrap_or_else(|| (0, vec![0; types.len()], HashMap::new()));
        let mut collectors: Vec<Collector> = who
            .into_iter()
            .map(|(user, (cards, kinds))| Collector { user, cards, types: kinds, full_set: wanted > 0 && kinds >= wanted })
            .collect();
        // Most cards first, then the widest collection, then oldest member id so
        // the order never wobbles between two equal rows.
        collectors.sort_by(|a, b| b.cards.cmp(&a.cards).then(b.types.cmp(&a.types)).then(a.user.cmp(&b.user)));
        houses.push(HouseState { key: h.key, total, top, cards, held, collectors });
    }
    // Most points first; level houses fall back to their name, never to chance.
    houses.sort_by(|a, b| {
        b.total.cmp(&a.total).then_with(|| {
            let name = |k: &str| house::house(k).map(|h| h.name).unwrap_or("");
            name(a.key).cmp(name(b.key))
        })
    });
    Ok(Cup { since, types, houses })
}

/// Reads the live stores. The frog store is read and let go of BEFORE the house
/// store is locked: selling a set takes the frog lock and then the house lock,
/// so taking them the other way round at the same time could wedge both.
pub fn read_live(now: i64) -> Option<Cup> {
    let optouts = house::optout_set();
    let (types, held) = {
        let db = frog_store::db()?;
        let frog = db.lock();
        (types_in_play(&frog), holdings(&frog))
    };
    let db = house::db()?;
    let conn = db.lock();
    assemble(&conn, now, &optouts, types, &held).ok()
}

// --- the state the page reads -------------------------------------------------------

/// "September 2026".
fn month_label(day: &str) -> String {
    const NAMES: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let mut parts = day.split('-');
    match (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok())) {
        (Some(year), Some(month)) if (1..=12).contains(&month) => format!("{} {}", NAMES[month - 1], year),
        _ => day.to_string(),
    }
}

/// A member as the page may know them: their display name and nothing else.
/// Someone the bot's cache can't name is still counted, just not named.
fn named(panel: &Panel, user: u64) -> String {
    panel.cached_name(user).unwrap_or_else(|| "A member".to_string())
}

/// The state as JSON. Every id is left behind here: only names, points and
/// counts go out.
pub fn render(panel: &Panel, cup: &Cup, generated: i64) -> Value {
    let types: Vec<Value> = cup
        .types
        .iter()
        .map(|t| json!({ "name": t.name, "rarity": t.rarity.key(), "rarity_name": t.rarity.name(), "emoji": t.rarity.emoji() }))
        .collect();
    let best = cup.houses.iter().map(|h| h.total).max().unwrap_or(0);
    let totals: Vec<i64> = cup.houses.iter().map(|h| h.total).collect();
    let houses: Vec<Value> = cup
        .houses
        .iter()
        .map(|s| {
            let meta = house::house(s.key);
            let held: Vec<Value> = s
                .held
                .iter()
                .zip(cup.types.iter())
                .map(|(n, t)| json!({ "name": t.name, "rarity": t.rarity.key(), "emoji": t.rarity.emoji(), "held": n }))
                .collect();
            json!({
                "key": s.key,
                "name": meta.map(|h| h.name).unwrap_or(s.key),
                "crest": meta.map(|h| h.crest).unwrap_or(""),
                "colour": meta.map(|h| format!("#{:06x}", h.colour)),
                "secondary": meta.map(|h| format!("#{:02x}{:02x}{:02x}", h.colours.1[0], h.colours.1[1], h.colours.1[2])),
                "total": s.total,
                // Level houses share a place.
                "rank": totals.iter().position(|t| *t == s.total).map(|i| i + 1).unwrap_or(1),
                "gap": best - s.total,
                "share": if best > 0 { s.total as f64 / best as f64 } else { 0.0 },
                "top": s.top.iter().map(|(user, points)| json!({ "name": named(panel, *user), "points": points })).collect::<Vec<_>>(),
                "cards": s.cards,
                "missing": s.held.iter().filter(|n| **n == 0).count(),
                "held": held,
                "collectors": s.collectors.iter().take(COLLECTORS).map(|c| json!({
                    "name": named(panel, c.user),
                    "cards": c.cards,
                    "types": c.types,
                    "full_set": c.full_set,
                })).collect::<Vec<_>>(),
                "more_collectors": s.collectors.len().saturating_sub(COLLECTORS),
                "full_sets": s.collectors.iter().filter(|c| c.full_set).count(),
            })
        })
        .collect();
    json!({
        "generated": generated,
        "month": {
            "since": cup.since,
            "label": month_label(&points::ist_day(cup.since)),
        },
        "types": types,
        "houses": houses,
    })
}

// --- the cache ----------------------------------------------------------------------

struct Cached {
    at: Instant,
    body: Value,
}

static CACHE: LazyLock<Mutex<Option<Cached>>> = LazyLock::new(|| Mutex::new(None));
static BUILDS: AtomicUsize = AtomicUsize::new(0);

/// The state, computed at most once per window however many people are reading.
///
/// The lock is held across the rebuild on purpose - it is plain SQLite work with
/// no `.await` in it - so a hundred pages arriving together make ONE pass over
/// the databases and then all read the same answer.
fn cached_state(panel: &Panel, now: i64, at: Instant) -> Option<Value> {
    let window = Duration::from_secs(cache_secs());
    let mut cache = CACHE.lock();
    if let Some(held) = cache.as_ref()
        && at.saturating_duration_since(held.at) < window
    {
        return Some(held.body.clone());
    }
    let cup = panel.data.housecup(now)?;
    BUILDS.fetch_add(1, Ordering::Relaxed);
    let body = render(panel, &cup, now);
    *cache = Some(Cached { at, body: body.clone() });
    Some(body)
}

/// How many times the state has actually been computed, for tests.
#[cfg(test)]
pub fn builds() -> usize {
    BUILDS.load(Ordering::Relaxed)
}

/// Throws the cached state away, for tests.
#[cfg(test)]
pub fn forget() {
    *CACHE.lock() = None;
}

// --- the routes ----------------------------------------------------------------------

/// The page's routes. They go on the outer router, so neither the panel session
/// nor the `X-Panel` header is in the way.
pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/housecup", get(no_token))
        .route("/housecup/{token}", get(page))
        .route("/housecup/{token}/state", get(state))
        .route("/assets/housecup.css", get(|| async { asset("text/css; charset=utf-8", ui::file("housecup.css", PAGE_CSS).into_owned()) }))
        .route("/assets/housecup.js", get(|| async { asset("text/javascript; charset=utf-8", ui::file("housecup.js", PAGE_JS).into_owned()) }))
        .route_layer(middleware::from_fn(rate_limit))
}

/// The public bucket, kept apart from the sudoku page's so a busy scoreboard
/// can't use up a puzzle's allowance or the other way about.
pub async fn rate_limit(req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = super::client_ip(req.headers(), peer);
    if !super::sudoku::allow(&format!("housecup:{}", ip), Instant::now()) {
        return (StatusCode::TOO_MANY_REQUESTS, "Slow down a moment, then reload.").into_response();
    }
    next.run(req).await
}

fn asset(kind: &'static str, body: String) -> Response {
    let mut res = Response::new(Body::from(body));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
    res
}

fn html(status: StatusCode, body: String) -> Response {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    res
}

fn json_out(status: StatusCode, body: Value) -> Response {
    let mut res = Response::new(Body::from(body.to_string()));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

/// The same page for a made-up token, a stale one and no token at all: none of
/// them may learn anything from which.
pub fn closed_page() -> String {
    "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
     <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
     <meta name=\"referrer\" content=\"no-referrer\"><meta name=\"robots\" content=\"noindex, nofollow\">\
     <title>House Cup</title><link rel=\"stylesheet\" href=\"/assets/housecup.css\"></head>\
     <body><main class=\"shut\"><h1>🏆 House Cup</h1>\
     <p>This isn't the scoreboard's address. Run <b>/housecup</b> in Discord and the bot will give you the link.</p>\
     </main></body></html>"
        .to_string()
}

async fn no_token() -> Response {
    html(StatusCode::NOT_FOUND, closed_page())
}

async fn page(Path(token): Path<String>) -> Response {
    if !enabled() || !opens(&token) {
        return html(StatusCode::NOT_FOUND, closed_page());
    }
    html(StatusCode::OK, ui::file("housecup.html", PAGE_HTML).into_owned())
}

async fn state(State(panel): State<Panel>, Path(token): Path<String>) -> Response {
    if !enabled() || !opens(&token) {
        return json_out(StatusCode::NOT_FOUND, json!({"error": "This scoreboard link isn't open."}));
    }
    let now = chrono::Utc::now().timestamp();
    match cached_state(&panel, now, Instant::now()) {
        // `now` rides beside the cached body so the page can say how old it is
        // without trusting the clock on the machine it is drawn on.
        Some(mut body) => {
            body["now"] = json!(now);
            json_out(StatusCode::OK, body)
        }
        None => json_out(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "The house points aren't available right now."})),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        frog_store::init(&conn).unwrap();
        conn
    }

    fn ledger() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(points::SCHEMA).unwrap();
        conn.execute_batch(
            "CREATE TABLE members (user_id INTEGER PRIMARY KEY, house TEXT NOT NULL, sorted_by TEXT NOT NULL DEFAULT '', ts INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn
    }

    fn sort(conn: &Connection, user: u64, house: &str) {
        conn.execute("INSERT INTO members (user_id, house) VALUES (?1, ?2)", rusqlite::params![user as i64, house]).unwrap();
    }

    fn score(conn: &Connection, user: Option<u64>, house: &str, source: &str, pts: i64, ts: i64) {
        conn.execute(
            "INSERT INTO ledger (user_id, house, source, points, reason, day, ts) VALUES (?1, ?2, ?3, ?4, '', ?5, ?6)",
            rusqlite::params![user.map(|u| u as i64), house, source, pts, points::ist_day(ts), ts],
        )
        .unwrap();
    }

    /// One owned card of `wizard` for `user`.
    fn give(conn: &Connection, serial: i64, user: u64, wizard: i64) {
        conn.execute(
            "INSERT INTO cards (serial, user_id, wizard_id, edition, ts, original_owner) VALUES (?1, ?2, ?3, ?1, 0, ?2)",
            rusqlite::params![serial, user as i64, wizard],
        )
        .unwrap();
    }

    /// 2026-09-14 02:00 India time.
    const NOW: i64 = 1_789_331_400;

    fn state_of(house_conn: &Connection, frog_conn: &Connection, optouts: &[u64]) -> Cup {
        let out: HashSet<u64> = optouts.iter().copied().collect();
        assemble(house_conn, NOW, &out, types_in_play(frog_conn), &holdings(frog_conn)).unwrap()
    }

    #[test]
    fn the_month_starts_at_midnight_on_the_first_india_time() {
        let conn = ledger();
        let frog = frog();
        let cup = state_of(&conn, &frog, &[]);
        assert_eq!(points::ist_day(cup.since), "2026-09-01");
        assert_eq!(month_label(&points::ist_day(cup.since)), "September 2026");
        // A second earlier is the month before, so a point at that second is out.
        assert_eq!(points::ist_day(cup.since - 1), "2026-08-31");
        score(&conn, Some(1), "gryffindor", "quiz", 9, cup.since - 1);
        score(&conn, Some(1), "gryffindor", "quiz", 4, cup.since);
        let cup = state_of(&conn, &frog, &[]);
        let g = cup.houses.iter().find(|h| h.key == "gryffindor").unwrap();
        assert_eq!((g.total, g.top.len()), (4, 1), "only this month's points count");
    }

    #[test]
    fn houses_are_ordered_and_scorers_ranked() {
        let conn = ledger();
        let frog = frog();
        score(&conn, Some(1), "slytherin", "quiz", 30, NOW);
        score(&conn, Some(2), "slytherin", "chat", 12, NOW);
        score(&conn, Some(3), "ravenclaw", "arena", 20, NOW);
        score(&conn, None, "ravenclaw", "mod", 5, NOW);
        let cup = state_of(&conn, &frog, &[]);
        assert_eq!(cup.houses.iter().map(|h| h.key).collect::<Vec<_>>(), ["slytherin", "ravenclaw", "gryffindor", "hufflepuff"]);
        assert_eq!(cup.houses[0].total, 42);
        assert_eq!(cup.houses[0].top, vec![(1, 30), (2, 12)], "biggest first");
        assert_eq!(cup.houses[1].top, vec![(3, 20)], "a house-only award has no scorer");
        // Two houses level fall back to their name, not to chance.
        score(&conn, Some(4), "gryffindor", "quiz", 25, NOW);
        score(&conn, Some(5), "hufflepuff", "quiz", 25, NOW);
        let cup = state_of(&conn, &frog, &[]);
        assert_eq!(cup.houses.iter().map(|h| h.key).collect::<Vec<_>>(), ["slytherin", "gryffindor", "hufflepuff", "ravenclaw"]);
    }

    #[test]
    fn cards_belong_to_the_house_their_holder_is_in_now() {
        let conn = ledger();
        let frog = frog();
        sort(&conn, 1, "gryffindor");
        give(&frog, 1, 1, 1);
        give(&frog, 2, 1, 2);
        let first = state_of(&conn, &frog, &[]);
        assert_eq!(first.houses.iter().find(|h| h.key == "gryffindor").unwrap().cards, 2);
        assert_eq!(first.houses.iter().find(|h| h.key == "slytherin").unwrap().cards, 0);
        // The same cards, the same member, a different house: they move with her.
        conn.execute("UPDATE members SET house = 'slytherin' WHERE user_id = 1", []).unwrap();
        let moved = state_of(&conn, &frog, &[]);
        assert_eq!(moved.houses.iter().find(|h| h.key == "gryffindor").unwrap().cards, 0);
        assert_eq!(moved.houses.iter().find(|h| h.key == "slytherin").unwrap().cards, 2);
        // A card handed in is nobody's.
        frog.execute("UPDATE cards SET status = 'spent' WHERE serial = 1", []).unwrap();
        assert_eq!(state_of(&conn, &frog, &[]).houses.iter().find(|h| h.key == "slytherin").unwrap().cards, 1);
        // And a Muggle's cards count for nobody at all.
        assert_eq!(state_of(&conn, &frog, &[1]).houses.iter().find(|h| h.key == "slytherin").unwrap().cards, 0);
    }

    #[test]
    fn a_house_with_no_cards_and_a_member_with_none_are_still_drawn() {
        let conn = ledger();
        let frog = frog();
        sort(&conn, 1, "gryffindor");
        sort(&conn, 2, "gryffindor");
        score(&conn, Some(2), "gryffindor", "quiz", 7, NOW);
        give(&frog, 1, 1, 1);
        let cup = state_of(&conn, &frog, &[]);
        assert_eq!(cup.types.len(), 10, "the ten cards in play");
        for h in &cup.houses {
            assert_eq!(h.held.len(), cup.types.len(), "{} must have a slot for every card", h.key);
        }
        let empty = cup.houses.iter().find(|h| h.key == "hufflepuff").unwrap();
        assert_eq!((empty.cards, empty.collectors.len()), (0, 0));
        assert!(empty.held.iter().all(|n| *n == 0), "a house with nothing shows ten zeroes, not a short list");
        let g = cup.houses.iter().find(|h| h.key == "gryffindor").unwrap();
        assert_eq!(g.collectors.len(), 1, "a member holding nothing is not a collector");
        assert_eq!(g.collectors[0].user, 1);
        assert_eq!(g.held.iter().filter(|n| **n == 0).count(), 9, "the nine it lacks are visible as zeroes");
    }

    #[test]
    fn a_whole_set_in_one_pair_of_hands_is_marked() {
        let conn = ledger();
        let frog = frog();
        sort(&conn, 1, "ravenclaw");
        sort(&conn, 2, "ravenclaw");
        let all: Vec<i64> = types_in_play(&frog).iter().map(|t| t.wizard_id).collect();
        for (i, wizard) in all.iter().enumerate() {
            give(&frog, 100 + i as i64, 1, *wizard);
        }
        give(&frog, 200, 2, all[0]);
        give(&frog, 201, 2, all[0]);
        let cup = state_of(&conn, &frog, &[]);
        let r = cup.houses.iter().find(|h| h.key == "ravenclaw").unwrap();
        assert_eq!(r.cards, 12);
        assert_eq!(r.collectors[0].user, 1, "most cards first");
        assert!(r.collectors[0].full_set, "ten of ten can /sellset unaided");
        assert_eq!((r.collectors[1].cards, r.collectors[1].types), (2, 1));
        assert!(!r.collectors[1].full_set, "two of one card is not a set");
        assert!(r.held.iter().all(|n| *n >= 1), "the house holds every card");
    }

    #[test]
    fn a_retired_card_is_off_the_list_but_still_counted() {
        let conn = ledger();
        let frog = frog();
        sort(&conn, 1, "hufflepuff");
        frog.execute("UPDATE wizards SET enabled = 0 WHERE id = 1", []).unwrap();
        give(&frog, 1, 1, 1);
        give(&frog, 2, 1, 2);
        let cup = state_of(&conn, &frog, &[]);
        assert_eq!(cup.types.len(), 9, "a card out of play is not one to collect");
        let h = cup.houses.iter().find(|h| h.key == "hufflepuff").unwrap();
        assert_eq!(h.cards, 2, "the house still holds both");
        assert_eq!(h.collectors[0].types, 1, "only the one in play counts towards a set");
    }

    #[test]
    fn a_token_is_a_long_unguessable_one() {
        assert!(!plausible(""));
        assert!(!plausible("short"));
        assert!(!plausible("../../etc/passwd"));
        assert!(!plausible(&"a".repeat(65)));
        assert!(plausible(&"a".repeat(32)));
        let a = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
        let b = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 24]>());
        assert_eq!(a.len(), 32);
        assert!(plausible(&a) && a != b);
    }
}
