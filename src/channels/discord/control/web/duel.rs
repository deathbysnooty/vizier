//! The private Letter Duel board page.
//!
//! Each player of each game gets a random link of their own,
//! `/duel/<game>/<token>`, handed to them in an ephemeral Discord reply. The
//! link is the whole of the authentication: it names the game AND the seat, it
//! is thrown away when the game ends, and nothing the page is ever sent
//! mentions anybody else's rack — only how many tiles they are holding. These
//! routes therefore sit OUTSIDE `/api`, where the panel's session cookie and
//! admin check live: a player is not an admin and must never need to sign in to
//! put a tile down.
//!
//! The page is a convenience and never evidence. It works out what a play would
//! score before it is committed, and refuses an illegal one in plain words —
//! and then the server works the whole thing out again from the board and the
//! rack it holds, with the same code, so the page can be as wrong as it likes
//! without a bad word ever landing.

use std::borrow::Cow;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::duel::{self, state_json};
use super::super::super::duel_rules::{self as rules, Placement};
use super::super::super::duel_store::{self as store, Game};
use super::super::ui;
use super::Panel;

// Built into the binary; a copy in `<runtime>/ui` is served instead when there
// is one — see [`super::super::ui`].
const PAGE_HTML: &str = include_str!("../ui/duel.html");
const PAGE_JS: &str = include_str!("../ui/duel.js");

/// The board page's routes. They go on the outer router, so neither the panel
/// session nor the `X-Panel` header is in the way.
pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/duel/{game}/{token}", get(page))
        .route("/duel/{game}/{token}/state", get(state))
        .route("/duel/{game}/{token}/preview", post(preview))
        .route("/duel/{game}/{token}/play", post(play))
        .route("/duel/{game}/{token}/pass", post(pass))
        .route("/duel/{game}/{token}/exchange", post(exchange))
        .route("/duel/{game}/{token}/leave", post(leave))
        .route("/assets/duel.js", get(|| async { asset("text/javascript; charset=utf-8", ui::file("duel.js", PAGE_JS)) }))
}

fn asset(kind: &'static str, body: Cow<'static, str>) -> Response {
    let mut res = Response::new(super::text_body(body));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static(kind));
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
    res
}

fn html(status: StatusCode, body: Cow<'static, str>) -> Response {
    let mut res = Response::new(super::text_body(body));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/html; charset=utf-8"));
    // A rack link must never be kept by a shared browser or a proxy.
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store, private"));
    h.insert(header::REFERRER_POLICY, header::HeaderValue::from_static("no-referrer"));
    res
}

fn json_out(status: StatusCode, body: Value) -> Response {
    let mut res = Response::new(Body::from(body.to_string()));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-store, private"));
    res
}

/// The same answer for a link that never existed, a link for another game, and
/// a link whose game has ended: nothing here may say which.
fn unknown() -> Response {
    json_out(StatusCode::NOT_FOUND, json!({"error": "This board link isn't open."}))
}

/// Who a link belongs to, and only if it belongs to the game in the address. A
/// token is never trusted for the game it was not made for.
pub fn opens(game_id: i64, token: &str) -> Option<(Game, u64)> {
    let db = store::db()?;
    let conn = db.lock();
    let (owned_game, user) = store::token_owner(&conn, token)?;
    if owned_game != game_id {
        return None;
    }
    let game = store::get_game(&conn, game_id)?;
    if !game.running() {
        return None;
    }
    // A player who has left keeps nothing: their link stops working the moment
    // they go, exactly as it does when the game ends.
    let seat = game.seat_of(user)?;
    if game.player(seat).is_some_and(|p| p.dropped) {
        return None;
    }
    Some((game, user))
}

async fn page(Path((game_id, token)): Path<(i64, String)>) -> Response {
    let page = ui::file("duel.html", PAGE_HTML);
    match opens(game_id, &token) {
        Some(_) => html(StatusCode::OK, page),
        None => html(StatusCode::NOT_FOUND, page),
    }
}

async fn state(State(panel): State<Panel>, Path((game_id, token)): Path<(i64, String)>) -> Response {
    match opens(game_id, &token) {
        None => unknown(),
        Some((game, me)) => json_out(
            StatusCode::OK,
            state_json(&game, me, |id| name_of(&panel, id), chrono::Utc::now().timestamp()),
        ),
    }
}

fn name_of(panel: &Panel, id: u64) -> String {
    panel.cached_name(id).unwrap_or_else(|| format!("member {}", id))
}

/// One tile the page wants to put down. A lower-case letter means a blank being
/// used as that letter, which is exactly how the board writes one.
#[derive(Deserialize)]
pub struct Tile {
    pub at: i64,
    pub letter: String,
}

#[derive(Deserialize, Default)]
pub struct PlayBody {
    #[serde(default)]
    pub tiles: Vec<Tile>,
}

#[derive(Deserialize, Default)]
pub struct SwapBody {
    /// The tiles to put back, as they are written on the rack.
    #[serde(default)]
    pub tiles: String,
}

/// Reads the body into placements, or says what was wrong with it.
pub fn placements_of(body: &[u8]) -> Result<Vec<Placement>, String> {
    let parsed: PlayBody = serde_json::from_slice(body).unwrap_or_default();
    if parsed.tiles.len() > rules::RACK {
        return Err(rules::Refusal::Impossible.words());
    }
    let raw: Vec<(i64, String)> = parsed.tiles.into_iter().map(|t| (t.at, t.letter)).collect();
    rules::read_placements(&raw).map_err(|why| why.words())
}

async fn preview(Path((game_id, token)): Path<(i64, String)>, body: axum::body::Bytes) -> Response {
    let Some((game, me)) = opens(game_id, &token) else { return unknown() };
    match placements_of(&body) {
        Err(why) => json_out(StatusCode::OK, json!({"ok": false, "error": why})),
        Ok(tiles) => json_out(StatusCode::OK, duel::preview(&game, me, &tiles)),
    }
}

async fn play(State(panel): State<Panel>, Path((game_id, token)): Path<(i64, String)>, body: axum::body::Bytes) -> Response {
    let Some((_, me)) = opens(game_id, &token) else { return unknown() };
    let tiles = match placements_of(&body) {
        Err(why) => return json_out(StatusCode::OK, json!({"ok": false, "error": why})),
        Ok(tiles) => tiles,
    };
    match duel::play(me, game_id, &tiles) {
        Err(why) => json_out(StatusCode::OK, json!({"ok": false, "error": strip_markup(&why)})),
        Ok(took) => finished(&panel, game_id, &token, me, strip_markup(&took.words)),
    }
}

async fn pass(State(panel): State<Panel>, Path((game_id, token)): Path<(i64, String)>) -> Response {
    let Some((_, me)) = opens(game_id, &token) else { return unknown() };
    match duel::pass(me, game_id) {
        Err(why) => json_out(StatusCode::OK, json!({"ok": false, "error": strip_markup(&why)})),
        Ok(took) => finished(&panel, game_id, &token, me, strip_markup(&took.words)),
    }
}

async fn exchange(State(panel): State<Panel>, Path((game_id, token)): Path<(i64, String)>, body: axum::body::Bytes) -> Response {
    let Some((_, me)) = opens(game_id, &token) else { return unknown() };
    let wanted: SwapBody = serde_json::from_slice(&body).unwrap_or_default();
    match duel::exchange(me, game_id, &wanted.tiles) {
        Err(why) => json_out(StatusCode::OK, json!({"ok": false, "error": strip_markup(&why)})),
        Ok(took) => finished(&panel, game_id, &token, me, strip_markup(&took.words)),
    }
}

async fn leave(Path((game_id, token)): Path<(i64, String)>) -> Response {
    let Some((_, me)) = opens(game_id, &token) else { return unknown() };
    match duel::leave(me, game_id) {
        Err(why) => json_out(StatusCode::OK, json!({"ok": false, "error": strip_markup(&why)})),
        Ok(took) => json_out(
            StatusCode::OK,
            json!({"ok": true, "over": true, "message": strip_markup(&took.words)}),
        ),
    }
}

/// The answer to a turn that went through: what happened, and the position
/// afterwards — or, when that turn ended the game, the news that it did.
fn finished(panel: &Panel, game_id: i64, token: &str, me: u64, message: String) -> Response {
    match opens(game_id, token) {
        Some((game, _)) => {
            let state = state_json(&game, me, |id| name_of(panel, id), chrono::Utc::now().timestamp());
            json_out(StatusCode::OK, json!({"ok": true, "message": message, "state": state}))
        }
        None => json_out(
            StatusCode::OK,
            json!({"ok": true, "over": true, "message": format!("{}\nThat's the game — the result is in Discord.", message)}),
        ),
    }
}

/// Discord's markdown means nothing on a web page, and a mention would show as
/// a raw number, so both are taken out of anything shown there.
pub fn strip_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' | '`' | '_' => {}
            '-' if chars.peek() == Some(&'#') => {
                chars.next();
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
            }
            _ => out.push(c),
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_of_tiles_is_read_and_rubbish_refused() {
        let tiles = placements_of(br#"{"tiles":[{"at":112,"letter":"C"},{"at":113,"letter":"a"}]}"#).expect("two tiles");
        assert_eq!(tiles.len(), 2);
        assert_eq!(tiles[0], Placement { at: 112, letter: 'C', blank: false });
        assert_eq!(tiles[1], Placement { at: 113, letter: 'A', blank: true }, "lower case is a blank");

        // Nothing at all is an empty play, which the rules refuse by name.
        assert_eq!(placements_of(b"{}").expect("nothing").len(), 0);
        assert_eq!(placements_of(b"not json").expect("nothing").len(), 0);
        // Rubbish in the tiles themselves is refused here rather than reaching
        // the board.
        assert!(placements_of(br#"{"tiles":[{"at":112,"letter":"CAT"}]}"#).is_err());
        assert!(placements_of(br#"{"tiles":[{"at":-5,"letter":"C"}]}"#).is_err());
        assert!(placements_of(br#"{"tiles":[{"at":999,"letter":"C"}]}"#).is_err());
        assert!(placements_of(br#"{"tiles":[{"at":1,"letter":"4"}]}"#).is_err());
        // More tiles than a rack holds never even gets read.
        let many: String = (0..9).map(|i| format!(r#"{{"at":{},"letter":"A"}}"#, i)).collect::<Vec<_>>().join(",");
        assert!(placements_of(format!(r#"{{"tiles":[{}]}}"#, many).as_bytes()).is_err());
    }

    #[test]
    fn discord_markup_is_taken_out_of_anything_the_page_shows() {
        assert_eq!(strip_markup("✅ **QUARTZ** for **48**"), "✅ QUARTZ for 48");
        assert_eq!(strip_markup("-# only a note"), "only a note");
        assert_eq!(strip_markup("a `code` span"), "a code span");
        assert_eq!(strip_markup("plain"), "plain");
        // A hyphen that isn't a Discord note is left alone.
        assert_eq!(strip_markup("a well-known word"), "a well-known word");
    }

    // --- the whole thing, through the real store and the real routes ------------

    use super::super::super::super::duel_rules::{Across, RACK};
    use super::super::super::super::{duel_store, duel_words};
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    const ONE: u64 = 4_001;
    const TWO: u64 = 4_002;
    const THREE: u64 = 4_003;

    fn at(name: &str) -> usize {
        let col = name.as_bytes()[0] - b'A';
        let row: usize = name[1..].parse().expect("a row");
        (row - 1) * rules::SIZE + col as usize
    }

    /// Lays tiles on a board without checking anything, for setting a position
    /// up that a test wants to start from.
    fn lay(board: &str, from: usize, word: &str, across: Across) -> String {
        let mut cells = rules::cells(board);
        let step = match across {
            Across::Row => 1,
            Across::Column => rules::SIZE,
        };
        for (i, c) in word.chars().enumerate() {
            cells[from + i * step] = c;
        }
        cells.into_iter().collect()
    }

    /// A game in the real store, with racks the test chooses and a bag it can
    /// run down. Returns the game and a link for each player.
    fn a_game(seats: &[(u64, &str)], bag: &str, turn_secs: i64, now: i64) -> (duel_store::Game, Vec<String>) {
        super::super::tests::store();
        let db = duel_store::db().expect("the duel store");
        let conn = db.lock();
        let seated: Vec<(u64, String, String)> =
            seats.iter().map(|(u, rack)| (*u, "ravenclaw".to_string(), (*rack).to_string())).collect();
        let game = duel_store::start_game(&conn, 5, &seated, &rules::empty_board(), bag, turn_secs, now, "2026-09-17")
            .expect("a game");
        let links = seats
            .iter()
            .map(|(u, _)| duel_store::token_for(&conn, game.id, *u, now, || rand::random::<f64>()).expect("a link"))
            .collect();
        (game, links)
    }

    async fn hit(app: &axum::Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 4 << 20).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into())))
    }

    /// Whether this checkout has the real dictionary. Without it the game is
    /// off by design, and the word tests below have nothing to say.
    fn has_words() -> bool {
        super::super::tests::store();
        duel_words::knows("quartz")
    }

    #[tokio::test]
    async fn a_whole_game_is_played_through_the_page_and_the_server_checks_it_all() {
        if !has_words() {
            return;
        }
        let app = super::super::tests::panel();
        let now = chrono::Utc::now().timestamp();
        // Three players, so the full prizes are in play, and a bag with enough
        // in it to draw from.
        let (game, links) = a_game(&[(ONE, "QUARTZS"), (TWO, "ANTEDIR"), (THREE, "BCDFGHJ")], &"E".repeat(40), 900, now);
        let mine = format!("/duel/{}/{}", game.id, links[0]);
        let theirs = format!("/duel/{}/{}", game.id, links[1]);

        // The page opens, and says what only this player may know.
        let (status, state) = hit(&app, "GET", &mine, None).await;
        assert_eq!(status, StatusCode::OK);
        let (status, state) = hit(&app, "GET", &format!("{}/state", mine), None).await;
        assert_eq!(status, StatusCode::OK, "{state}");
        assert_eq!(state["rack"], "QUARTZS");
        assert_eq!(state["your_turn"], true);
        assert!(!state.to_string().contains("ANTEDIR"), "another player's rack must never be sent");
        assert_eq!(state["seats"][1]["tiles"], 7);

        // A play is priced before it is committed, and the price is the one the
        // server will pay.
        let quartz = |from: usize| {
            json!({"tiles": "QUARTZ".chars().enumerate().map(|(i, c)| json!({"at": from + i, "letter": c})).collect::<Vec<_>>()})
        };
        let (_, priced) = hit(&app, "POST", &format!("{}/preview", mine), Some(quartz(at("F8")))).await;
        assert_eq!(priced["ok"], true, "{priced}");
        assert_eq!(priced["score"], 48, "Q10+U1+A1+R1+T1+Z10, doubled by the star");
        assert_eq!(priced["bingo"], false);

        // The first word has to cross the middle, and the refusal says so.
        let (_, refused) = hit(&app, "POST", &format!("{}/preview", mine), Some(quartz(at("A1")))).await;
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap_or_default().contains("cross the middle"), "{refused}");
        // And trying it for real is refused too — the page is never trusted.
        let (_, refused) = hit(&app, "POST", &format!("{}/play", mine), Some(quartz(at("A1")))).await;
        assert_eq!(refused["ok"], false);
        assert!(refused["error"].as_str().unwrap_or_default().contains("cross the middle"), "{refused}");

        // The other player cannot play out of turn, and cannot use this link.
        let (_, out_of_turn) = hit(&app, "POST", &format!("{}/play", theirs), Some(quartz(at("F8")))).await;
        assert_eq!(out_of_turn["ok"], false);
        assert!(out_of_turn["error"].as_str().unwrap_or_default().contains("isn't your turn"), "{out_of_turn}");
        let (status, _) = hit(&app, "GET", &format!("/duel/{}/{}/state", game.id + 999, links[0]), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "a link is only good for its own game");

        // Now the real thing.
        let (_, played) = hit(&app, "POST", &format!("{}/play", mine), Some(quartz(at("F8")))).await;
        assert_eq!(played["ok"], true, "{played}");
        assert_eq!(played["message"], "✅ QUARTZ for 48", "one word, so no breakdown after it");
        assert_eq!(played["state"]["seats"][0]["score"], 48);
        assert_eq!(played["state"]["your_turn"], false, "the turn has moved on");
        assert_eq!(played["state"]["rack"].as_str().map(|r| r.chars().count()), Some(RACK), "six drawn to replace six");

        // Seat two hangs a word off it, and a word that is rubbish sideways is
        // refused by name.
        let nte = |letters: &str| {
            json!({"tiles": letters.chars().enumerate().map(|(i, c)| json!({"at": at("H9") + i * rules::SIZE, "letter": c})).collect::<Vec<_>>()})
        };
        let (_, nonsense) = hit(&app, "POST", &format!("{}/play", theirs), Some(nte("DRT"))).await;
        assert_eq!(nonsense["ok"], false);
        assert!(nonsense["error"].as_str().unwrap_or_default().contains("isn't a word I know"), "{nonsense}");
        let (_, good) = hit(&app, "POST", &format!("{}/play", theirs), Some(nte("NTE"))).await;
        assert_eq!(good["ok"], true, "{good}");
        assert!(good["message"].as_str().unwrap_or_default().contains("ANTE"), "{good}");
        assert!(good["message"].as_str().unwrap_or_default().contains("for"), "{good}");
        // Nothing the page is sent carries Discord's markdown.
        assert!(!good["message"].as_str().unwrap_or_default().contains('*'));

        // Seat three swaps, passes, and then leaves.
        let third = format!("/duel/{}/{}", game.id, links[2]);
        let (_, swapped) = hit(&app, "POST", &format!("{}/exchange", third), Some(json!({"tiles": "BCD"}))).await;
        assert_eq!(swapped["ok"], true, "{swapped}");
        assert_eq!(swapped["state"]["rack"].as_str().map(|r| r.chars().count()), Some(RACK));
        let (_, passed) = hit(&app, "POST", &format!("{}/pass", mine), None).await;
        assert_eq!(passed["ok"], true, "{passed}");
        let (_, gone) = hit(&app, "POST", &format!("{}/leave", third), None).await;
        assert_eq!(gone["ok"], true, "{gone}");
        assert_eq!(gone["over"], true, "the link stops working the moment they go");
        let (status, _) = hit(&app, "GET", &format!("{}/state", third), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        // Two are left, and the seat that went is stepped over.
        let after = super::super::super::super::duel_store::db()
            .and_then(|db| duel_store::get_game(&db.lock(), game.id))
            .expect("still there");
        assert!(after.running(), "two is still a game");
        assert_eq!(after.playing(), vec![0, 1]);
        assert!(after.player(2).map(|p| p.dropped).unwrap_or(false));
        assert!(after.player(2).map(|p| p.rack.is_empty()).unwrap_or(false), "their tiles went back in the bag");
        duel::finish(game.id, "cancelled", None);
    }

    #[tokio::test]
    async fn a_short_bag_stops_a_swap_and_the_words_are_plain() {
        if !has_words() {
            return;
        }
        let app = super::super::tests::panel();
        let now = chrono::Utc::now().timestamp();
        let (game, links) = a_game(&[(ONE, "QUARTZS"), (TWO, "ANTEDIR")], "ABC", 900, now);
        let mine = format!("/duel/{}/{}", game.id, links[0]);
        let (_, state) = hit(&app, "GET", &format!("{}/state", mine), None).await;
        assert_eq!(state["can_exchange"], false, "three tiles is not enough to swap");
        let (_, refused) = hit(&app, "POST", &format!("{}/exchange", mine), Some(json!({"tiles": "QU"}))).await;
        assert_eq!(refused["ok"], false);
        let why = refused["error"].as_str().unwrap_or_default();
        assert!(why.contains("only 3 tiles left in the bag") && why.contains("needs 7"), "{why}");
        duel::finish(game.id, "cancelled", None);

        // With a full bag, a tile that isn't on the rack is what is refused.
        let (other, links) = a_game(&[(ONE, "QUARTZS"), (TWO, "ANTEDIR")], &"E".repeat(30), 900, now);
        let full = format!("/duel/{}/{}", other.id, links[0]);
        let (_, nope) = hit(&app, "POST", &format!("{}/exchange", full), Some(json!({"tiles": "K"}))).await;
        assert!(nope["error"].as_str().unwrap_or_default().contains("don't have a K"), "{nope}");
        // And nothing at all is refused before anything else.
        let (_, empty) = hit(&app, "POST", &format!("{}/exchange", full), Some(json!({"tiles": ""}))).await;
        assert!(empty["error"].as_str().unwrap_or_default().contains("Pick the tiles"), "{empty}");
        duel::finish(other.id, "cancelled", None);
    }

    #[tokio::test]
    async fn the_last_tile_down_ends_the_game_and_the_leftovers_change_the_scores() {
        if !has_words() {
            return;
        }
        super::super::tests::store();
        let now = chrono::Utc::now().timestamp();
        // An empty bag from the start, and a rack of exactly the four tiles the
        // play below needs, so putting them down goes out.
        let (game, _) = a_game(&[(ONE, "ANTE"), (TWO, "QI")], "", 900, now);
        let tiles: Vec<Placement> = "ANTE"
            .chars()
            .enumerate()
            .map(|(i, c)| Placement { at: at("F8") + i, letter: c, blank: false })
            .collect();
        let took = duel::play(ONE, game.id, &tiles).expect("ANTE through the middle");
        assert!(took.words.contains("ANTE"), "{}", took.words);
        // The game ended itself, and the links went with it.
        let db = duel_store::db().expect("the store");
        let over = duel_store::get_game(&db.lock(), game.id).expect("still there");
        assert!(!over.running(), "the bag was empty and a rack is now too");
        assert_eq!(over.result.as_deref(), Some("played out"));
        assert_eq!(over.went_out, Some(0));

        // What each side is holding settles it: Q 10 + I 1 to the one who went
        // out, and off the one who did not.
        let racks: Vec<&str> = over.players.iter().map(|p| p.rack.as_str()).collect();
        assert_eq!(rules::adjustments(&racks, over.went_out), vec![11, -11]);
    }

    #[tokio::test]
    async fn everybody_passing_twice_ends_it_too() {
        super::super::tests::store();
        let now = chrono::Utc::now().timestamp();
        let (game, _) = a_game(&[(ONE, "AAAAAAA"), (TWO, "BBBBBBB")], &"C".repeat(30), 900, now);
        // Two players, so two passes each ends it — and the third round below
        // never gets the chance to happen.
        for _ in 0..2 {
            duel::pass(ONE, game.id).expect("a pass");
            duel::pass(TWO, game.id).expect("a pass");
        }
        assert!(duel::pass(ONE, game.id).is_err(), "there is nothing left to pass on");
        let db = duel_store::db().expect("the store");
        let over = duel_store::get_game(&db.lock(), game.id).expect("still there");
        assert!(!over.running());
        assert_eq!(over.result.as_deref(), Some("passed out"));
        assert_eq!(over.turns, 4, "two passes each was enough — the third round never happened");
    }

    #[tokio::test]
    async fn a_turn_that_runs_out_is_passed_and_three_of_them_drop_the_player() {
        super::super::tests::store();
        let now = chrono::Utc::now().timestamp();
        // Three players and a short clock, so the game goes on after one goes.
        let (game, _) = a_game(&[(ONE, "AAAAAAA"), (TWO, "BBBBBBB"), (THREE, "CCCCCCC")], &"D".repeat(40), 5, now - 3_600);
        let db = duel_store::db().expect("the store");
        // Seat 0 never plays, so their clock runs out three times round. Between
        // rounds the other two are taken to have played real words, so the
        // game is not ended by the passing instead — the turn is simply handed
        // back to seat 0 with the pass counter clear.
        for round in 1..=3 {
            let later = chrono::Utc::now().timestamp() + 60;
            duel::run_clocks(later, later - 3_600);
            let after = duel_store::get_game(&db.lock(), game.id).expect("still there");
            assert_eq!(after.player(0).map(|p| p.misses), Some(round), "round {}", round);
            assert_eq!(
                duel_store::last_turn(&db.lock(), game.id).map(|t| t.kind),
                Some(if round == 3 { "dropped".to_string() } else { "missed".to_string() }),
                "round {}",
                round
            );
            if round < 3 {
                assert!(!after.player(0).map(|p| p.dropped).unwrap_or(true), "not out yet");
                db.lock()
                    .execute(
                        "UPDATE games SET turn = 0, passes = 0, turn_started_at = 0 WHERE id = ?1",
                        rusqlite::params![game.id],
                    )
                    .unwrap();
            }
        }
        let after = duel_store::get_game(&db.lock(), game.id).expect("still there");
        assert!(after.player(0).map(|p| p.dropped).unwrap_or(false), "three missed turns and they are out");
        assert_eq!(after.playing(), vec![1, 2], "the other two play on");
        assert!(after.running());
        assert_ne!(after.turn, 0, "the empty seat is not left holding the turn");
        duel::finish(game.id, "cancelled", None);
    }

    #[tokio::test]
    async fn nothing_is_judged_in_the_first_moments_after_the_bot_wakes_up() {
        super::super::tests::store();
        let now = chrono::Utc::now().timestamp();
        let (game, _) = a_game(&[(ONE, "AAAAAAA"), (TWO, "BBBBBBB")], &"C".repeat(30), 5, now - 600);
        // Woken a moment ago: the clock is long gone, and still nobody loses a
        // turn to it.
        let later = chrono::Utc::now().timestamp() + 60;
        duel::run_clocks(later, later - 1);
        let db = duel_store::db().expect("the store");
        assert_eq!(
            duel_store::get_game(&db.lock(), game.id).and_then(|g| g.player(0).map(|p| p.misses)),
            Some(0),
            "the grace window has to pass first"
        );
        duel::run_clocks(later, later - duel::GRACE_SECS - 1);
        assert_eq!(duel_store::get_game(&db.lock(), game.id).and_then(|g| g.player(0).map(|p| p.misses)), Some(1));
        duel::finish(game.id, "cancelled", None);
    }

    #[tokio::test]
    async fn a_blank_plays_as_a_letter_and_scores_nothing() {
        if !has_words() {
            return;
        }
        super::super::tests::store();
        let now = chrono::Utc::now().timestamp();
        let (game, _) = a_game(&[(ONE, "?NTE"), (TWO, "QIQIQIQ")], &"E".repeat(20), 900, now);
        let mut tiles = vec![Placement { at: at("F8"), letter: 'A', blank: true }];
        for (i, c) in "NTE".chars().enumerate() {
            tiles.push(Placement { at: at("F8") + 1 + i, letter: c, blank: false });
        }
        duel::play(ONE, game.id, &tiles).expect("ANTE with a blank A");
        let db = duel_store::db().expect("the store");
        let after = duel_store::get_game(&db.lock(), game.id).expect("still there");
        // A blank A, N 1, T 1, E 1 — and H8 is the star, so it is doubled: 6.
        assert_eq!(after.player(0).map(|p| p.score), Some(6));
        assert_eq!(rules::cells(&after.board)[at("F8")], 'a', "written in lower case, so it stays worth nothing");
        duel::finish(game.id, "cancelled", None);
    }

    #[tokio::test]
    async fn a_word_hung_where_it_touches_nothing_is_refused() {
        if !has_words() {
            return;
        }
        super::super::tests::store();
        let now = chrono::Utc::now().timestamp();
        let (game, _) = a_game(&[(ONE, "ANTECAT"), (TWO, "QIQIQIQ")], &"E".repeat(20), 900, now);
        // Put something on the board first, then try to play adrift from it.
        let db = duel_store::db().expect("the store");
        {
            let conn = db.lock();
            let board = lay(&rules::empty_board(), at("F8"), "ANTE", Across::Row);
            conn.execute("UPDATE games SET board = ?2 WHERE id = ?1", rusqlite::params![game.id, board]).unwrap();
        }
        let tiles: Vec<Placement> =
            "CAT".chars().enumerate().map(|(i, c)| Placement { at: at("A14") + i, letter: c, blank: false }).collect();
        let why = duel::play(ONE, game.id, &tiles).expect_err("adrift");
        assert!(why.contains("doesn't touch anything"), "{}", why);
        duel::finish(game.id, "cancelled", None);
    }

    #[test]
    fn a_link_for_another_game_opens_nothing() {
        super::super::tests::store();
        assert!(opens(1, "nonsense").is_none());
        assert!(opens(999_999, "").is_none());
    }
}
