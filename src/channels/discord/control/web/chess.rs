//! The private chess board page.
//!
//! Each player of each game gets a random link of their own,
//! `/chess/<game>/<token>`, which is handed to them in an ephemeral Discord
//! reply. The link is the whole of the authentication: it names the game AND
//! the side, it is thrown away when the game ends, and nothing the page is ever
//! sent mentions the other player's link. These routes therefore sit OUTSIDE
//! `/api`, where the panel's session cookie and admin check live: a player is
//! not an admin and must never need to sign in to move a piece.
//!
//! The page itself is static; its script reads the game and the token out of
//! the address bar and talks to the three endpoints below. Everything a move
//! needs to be legal is decided here, by the same code the Discord pop-up uses,
//! so the page can be as wrong as it likes without a bad move ever landing.

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use shakmaty::{Color, Position};

use super::super::super::chess::{self, DrawMove};
use super::super::super::chess_rules::{self as rules, Replay, TimeControl};
use super::super::super::chess_store::{self as store, Game};
use super::Panel;

const PAGE_HTML: &str = include_str!("../ui/chess.html");
const PAGE_JS: &str = include_str!("../ui/chess.js");

/// The board page's routes. They go on the outer router, so neither the panel
/// session nor the `X-Panel` header is in the way.
pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/chess/{game}/{token}", get(page))
        .route("/chess/{game}/{token}/state", get(state))
        .route("/chess/{game}/{token}/move", post(play))
        .route("/chess/{game}/{token}/resign", post(resign))
        .route("/chess/{game}/{token}/draw", post(draw))
        .route("/assets/chess.js", get(|| async { asset("text/javascript; charset=utf-8", PAGE_JS) }))
}

fn asset(kind: &'static str, body: &'static str) -> Response {
    let mut res = Response::new(Body::from(body));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static(kind));
    h.insert(header::CACHE_CONTROL, header::HeaderValue::from_static("no-cache"));
    res
}

fn html(status: StatusCode, body: &'static str) -> Response {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, header::HeaderValue::from_static("text/html; charset=utf-8"));
    // A board link must never be kept by a shared browser or a proxy.
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

/// Who a link belongs to, and only if it belongs to the game in the address.
/// A token is never trusted for the game it was not made for.
pub fn opens(game_id: i64, token: &str) -> Option<(Game, u64)> {
    let db = store::db()?;
    let conn = db.lock();
    let (owned_game, user) = store::token_owner(&conn, token)?;
    if owned_game != game_id {
        return None;
    }
    let game = store::get_game(&conn, game_id)?;
    if !game.has(user) || !game.running() {
        return None;
    }
    Some((game, user))
}

async fn page(Path((game_id, token)): Path<(i64, String)>) -> Response {
    match opens(game_id, &token) {
        Some(_) => html(StatusCode::OK, PAGE_HTML),
        None => html(StatusCode::NOT_FOUND, PAGE_HTML),
    }
}

/// Everything the page draws, for ONE side. The other player's link is not in
/// here, and neither is anything else of theirs but their name.
pub fn state_json(panel: &Panel, game: &Game, me: u64, now: i64) -> Value {
    let them = game.other(me);
    let i_am_white = game.is_white(me);
    let replay = Replay::from_sans(&game.moves).unwrap_or_default();
    let my_move = game.to_move() == me;
    let legal: Vec<Value> = if my_move {
        replay
            .position
            .legal_moves()
            .iter()
            .map(|m| {
                let long = rules::long_form(*m);
                json!({
                    "from": long.get(0..2).unwrap_or_default(),
                    "to": long.get(2..4).unwrap_or_default(),
                    "promotion": long.get(4..5),
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    let check = replay
        .position
        .is_check()
        .then(|| replay.position.board().king_of(replay.position.turn()).map(|sq| sq.to_string()))
        .flatten();
    let last = game.moves.last().and_then(|_| last_squares(&game.moves));
    let name = |id: u64| panel.cached_name(id).unwrap_or_else(|| "your opponent".to_string());
    let time = TimeControl::from_key(&game.time_control);
    json!({
        "game": game.id,
        "you": if i_am_white { "white" } else { "black" },
        "turn": if replay.turn() == Color::White { "white" } else { "black" },
        "your_move": my_move,
        "fen": game.fen,
        "moves": game.moves,
        "legal": legal,
        "last": last,
        "check": check,
        "seconds_left": rules::seconds_left(game.last_move_ts, game.per_move_secs, now),
        "clock": rules::clock_words(rules::seconds_left(game.last_move_ts, game.per_move_secs, now)),
        "deadline": rules::deadline(game.last_move_ts, game.per_move_secs),
        "time_control": time.key(),
        "pace": chess::control_words(time, game.per_move_secs),
        "me": panel.cached_name(me).unwrap_or_else(|| "you".to_string()),
        "them": name(them),
        "my_crest": chess::crest(me),
        "their_crest": chess::crest(them),
        "same_house": game.same_house(),
        "draw_offer": match game.draw_offer {
            Some(who) if who == me => "you",
            Some(_) => "them",
            None => "none",
        },
        "move_number": replay.move_number(),
    })
}

/// The squares the last move went between, as "e2e4".
fn last_squares(sans: &[String]) -> Option<String> {
    let last = sans.last()?;
    let before = Replay::from_sans(&sans[..sans.len() - 1]).ok()?;
    let m = last.parse::<shakmaty::san::San>().ok()?.to_move(&before.position).ok()?;
    let (from, to) = rules::highlight(m);
    Some(format!("{}{}", from, to))
}

async fn state(State(panel): State<Panel>, Path((game_id, token)): Path<(i64, String)>) -> Response {
    match opens(game_id, &token) {
        None => unknown(),
        Some((game, me)) => json_out(StatusCode::OK, state_json(&panel, &game, me, chrono::Utc::now().timestamp())),
    }
}

#[derive(Deserialize)]
pub struct MoveBody {
    /// A move in either notation; the page sends the long form.
    #[serde(rename = "move")]
    pub the_move: String,
}

async fn play(State(panel): State<Panel>, Path((game_id, token)): Path<(i64, String)>, body: axum::body::Bytes) -> Response {
    let Some((_, me)) = opens(game_id, &token) else { return unknown() };
    let typed = serde_json::from_slice::<MoveBody>(&body).map(|b| b.the_move).unwrap_or_default();
    match chess::play(me, game_id, &typed) {
        Err(why) => json_out(StatusCode::OK, json!({"ok": false, "error": strip_markup(&why)})),
        Ok(played) => {
            let after = opens(game_id, &token).map(|(g, _)| state_json(&panel, &g, me, chrono::Utc::now().timestamp()));
            json_out(StatusCode::OK, json!({"ok": true, "san": played.san, "state": after}))
        }
    }
}

async fn resign(Path((game_id, token)): Path<(i64, String)>) -> Response {
    let Some((game, me)) = opens(game_id, &token) else { return unknown() };
    chess::finish(game.id, "resign", Some(game.other(me)), true);
    json_out(StatusCode::OK, json!({"ok": true, "over": true, "message": "You resigned. The result card is in Discord."}))
}

async fn draw(Path((game_id, token)): Path<(i64, String)>) -> Response {
    let Some((game, me)) = opens(game_id, &token) else { return unknown() };
    let (over, message) = match chess::offer_or_accept_draw(&game, me) {
        DrawMove::Agreed => (true, "That's a draw. The result card is in Discord."),
        DrawMove::Offered => (false, "Draw offered. It's up to them now."),
        DrawMove::AlreadyOpen => (false, "Your draw offer is already waiting."),
    };
    json_out(StatusCode::OK, json!({"ok": true, "over": over, "message": message}))
}

/// Discord's markdown means nothing on a web page, and a mention would show as
/// a raw id, so the reasons a move was refused are flattened before they go out.
fn strip_markup(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(moves: &[&str]) -> Game {
        let replay = Replay::from_sans(moves).expect("legal");
        Game {
            id: 42,
            channel: 5,
            white: 111,
            black: 222,
            white_house: "ravenclaw".into(),
            black_house: "gryffindor".into(),
            fen: replay.fen(),
            moves: replay.sans.clone(),
            status: "running".into(),
            time_control: "casual".into(),
            per_move_secs: 43_200,
            started_at: 1_000,
            last_move_ts: 1_000,
            message: None,
            result: None,
            winner: None,
            by_resignation: false,
            finished_at: None,
            points_white: 0,
            points_black: 0,
            paid_out: false,
            why_nothing: String::new(),
            draw_offer: None,
            draw_announced: false,
            nudged_white: false,
            nudged_black: false,
        }
    }

    fn panel() -> Panel {
        super::super::tests::fake_panel()
    }

    #[test]
    fn the_page_tells_a_player_only_about_their_own_side() {
        let g = game(&["e4", "e5"]);
        let white = state_json(&panel(), &g, 111, 1_100);
        let text = white.to_string();
        assert_eq!(white["you"], "white");
        assert_eq!(white["your_move"], true);
        assert!(!white["legal"].as_array().unwrap().is_empty());
        assert!(!text.contains("token"), "no link of any kind is ever sent: {}", text);
        assert!(!text.contains("chess/42/"), "and certainly not the other player's: {}", text);

        let black = state_json(&panel(), &g, 222, 1_100);
        assert_eq!(black["you"], "black");
        assert_eq!(black["your_move"], false);
        assert!(black["legal"].as_array().unwrap().is_empty(), "the side not to move is given no moves");
    }

    #[test]
    fn the_state_carries_the_clock_the_last_move_and_a_check() {
        let g = game(&["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7"]);
        let s = state_json(&panel(), &g, 222, 1_000);
        assert_eq!(s["last"], "h5f7");
        assert_eq!(s["check"], "e8");
        assert_eq!(s["turn"], "black");
        assert_eq!(s["seconds_left"], 43_200);
        assert_eq!(s["clock"], "12 h 00 m");
        assert_eq!(s["move_number"], 4);
        assert_eq!(s["pace"], "Casual · 12 h 00 m per move");
        assert_eq!(s["same_house"], false);
        assert_eq!(s["draw_offer"], "none");

        let fresh = game(&[]);
        assert_eq!(state_json(&panel(), &fresh, 111, 1_000)["last"], Value::Null);
        assert_eq!(state_json(&panel(), &fresh, 111, 1_000)["check"], Value::Null);
    }

    #[test]
    fn a_draw_offer_is_shown_from_each_players_own_side() {
        let g = Game { draw_offer: Some(111), ..game(&["e4"]) };
        assert_eq!(state_json(&panel(), &g, 111, 1_000)["draw_offer"], "you");
        assert_eq!(state_json(&panel(), &g, 222, 1_000)["draw_offer"], "them");
    }

    #[test]
    fn the_legal_moves_name_their_promotions() {
        let mut g = game(&[]);
        g.fen = "r3k3/1P6/8/8/8/8/8/4K3 w - - 0 20".into();
        g.moves = Vec::new();
        // The move list is what is replayed, so a made-up FEN gives the opening
        // position: what matters here is the shape of each entry.
        let s = state_json(&panel(), &g, 111, 1_000);
        let legal = s["legal"].as_array().expect("moves");
        assert_eq!(legal.len(), 20, "twenty first moves");
        let first = &legal[0];
        assert!(first["from"].as_str().is_some_and(|f| f.len() == 2));
        assert!(first["to"].as_str().is_some_and(|t| t.len() == 2));
        assert_eq!(first["promotion"], Value::Null);
    }

    #[test]
    fn a_link_only_opens_its_own_game_and_a_wrong_one_opens_nothing() {
        // Without a store open every lookup refuses rather than panicking.
        assert!(opens(42, "bcdfghjkmnpqrstvwxyz2345").is_none());
        assert!(opens(42, "").is_none());
    }

    #[test]
    fn the_reasons_a_move_is_refused_lose_their_discord_markup() {
        assert_eq!(strip_markup("🚫 **Nf9** isn't legal"), "🚫 Nf9 isn't legal");
        assert_eq!(strip_markup("try `Nf3`"), "try Nf3");
    }

    // --- the routes, against the fake panel -------------------------------------

    use axum::body::Body as AxumBody;
    use axum::http::Request;
    use tower::ServiceExt as _;

    /// A real game in the test store, with a link for each player.
    fn live_game() -> (i64, String, String) {
        super::super::tests::store();
        let db = store::db().expect("chess store");
        let conn = db.lock();
        // Wound now, not at the epoch: a game whose clock has already run out
        // refuses every move, which would test nothing.
        let now = chrono::Utc::now().timestamp();
        let g = store::start_game(&conn, 5, 111, 222, "ravenclaw", "gryffindor", &Replay::new().fen(), "casual", 43_200, now, "2026-09-16")
            .expect("a game");
        let mut n = g.id as u64 * 7;
        let mut roll = || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (n >> 11) as f64 / (1u64 << 53) as f64
        };
        let white = store::token_for(&conn, g.id, 111, now, &mut roll).expect("white's link");
        let black = store::token_for(&conn, g.id, 222, now, &mut roll).expect("black's link");
        (g.id, white, black)
    }

    async fn hit(method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let app = super::super::tests::panel();
        let mut req = Request::builder().method(method).uri(path);
        let request = match body {
            Some(v) => {
                req = req.header("content-type", "application/json");
                req.body(AxumBody::from(v.to_string())).unwrap()
            }
            None => req.body(AxumBody::empty()).unwrap(),
        };
        let res = app.oneshot(request).await.expect("a response");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.expect("a body");
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()));
        (status, json)
    }

    #[tokio::test]
    async fn a_link_opens_its_own_side_and_never_hints_at_the_others() {
        let (id, white, black) = live_game();
        let (status, state) = hit("GET", &format!("/chess/{}/{}/state", id, white), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state["you"], "white");
        assert_eq!(state["your_move"], true);
        let text = state.to_string();
        assert!(!text.contains(&black), "black's link must never leave the server");
        assert!(!text.contains(&white), "and a page is not told its own link either");

        let (status, state) = hit("GET", &format!("/chess/{}/{}/state", id, black), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(state["you"], "black");
        assert_eq!(state["your_move"], false);
        assert!(state["legal"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_wrong_link_gets_nothing_and_says_nothing() {
        let (id, white, _) = live_game();
        let (other, _, _) = live_game();
        assert_ne!(id, other);
        for path in [
            format!("/chess/{}/{}/state", other, white),
            format!("/chess/{}/{}/state", id, "bcdfghjkmnpqrstvwxyz2345"),
            format!("/chess/{}/{}/state", id, "short"),
            format!("/chess/{}/{}/state", id, white.to_uppercase()),
        ] {
            let (status, body) = hit("GET", &path, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{} should open nothing", path);
            assert_eq!(body["error"], "This board link isn't open.", "every refusal reads the same");
        }
    }

    #[tokio::test]
    async fn the_page_is_served_for_a_good_link_and_a_bad_one_looks_the_same() {
        let (id, white, _) = live_game();
        let good = hit("GET", &format!("/chess/{}/{}", id, white), None).await;
        assert_eq!(good.0, StatusCode::OK);
        assert!(good.1.as_str().unwrap_or_default().contains("/assets/chess.js"));
        let bad = hit("GET", &format!("/chess/{}/{}", id, "bcdfghjkmnpqrstvwxyz2345"), None).await;
        assert_eq!(bad.0, StatusCode::NOT_FOUND);
        assert_eq!(bad.1, good.1, "the same page either way: the status is the only difference");
    }

    #[tokio::test]
    async fn a_move_only_lands_from_the_side_whose_turn_it_is() {
        let (id, white, black) = live_game();
        // Black tries first: refused, and the board has not moved.
        let (status, answer) = hit("POST", &format!("/chess/{}/{}/move", id, black), Some(json!({"move": "e7e5"}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["ok"], false);
        assert!(answer["error"].as_str().unwrap().contains("isn't your move"));

        // A move that isn't legal is refused with a reason, in plain words.
        let (_, answer) = hit("POST", &format!("/chess/{}/{}/move", id, white), Some(json!({"move": "e2e5"}))).await;
        assert_eq!(answer["ok"], false);
        assert!(answer["error"].as_str().unwrap().contains("isn't legal"), "{}", answer["error"]);
        assert!(!answer["error"].as_str().unwrap().contains("**"), "no Discord markup on the page");

        let (_, answer) = hit("POST", &format!("/chess/{}/{}/move", id, white), Some(json!({"move": "e2e4"}))).await;
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["san"], "e4");
        assert_eq!(answer["state"]["your_move"], false, "the answer already shows the new position");
        let (_, state) = hit("GET", &format!("/chess/{}/{}/state", id, black), None).await;
        assert_eq!(state["your_move"], true);
        assert_eq!(state["last"], "e2e4");
        assert_eq!(state["moves"][0], "e4");
    }

    #[tokio::test]
    async fn resigning_from_the_page_ends_the_game_and_kills_both_links() {
        let (id, white, black) = live_game();
        let (status, answer) = hit("POST", &format!("/chess/{}/{}/resign", id, white), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["over"], true);
        let done = store::db().and_then(|db| store::get_game(&db.lock(), id)).expect("the game");
        assert_eq!(done.result.as_deref(), Some("resign"));
        assert_eq!(done.winner, Some(222));
        for token in [&white, &black] {
            let (status, _) = hit("GET", &format!("/chess/{}/{}/state", id, token), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "a finished game's links are gone");
        }
    }

    #[tokio::test]
    async fn a_draw_can_be_offered_and_taken_from_the_page() {
        let (id, white, black) = live_game();
        let (_, answer) = hit("POST", &format!("/chess/{}/{}/draw", id, white), None).await;
        assert_eq!(answer["over"], false);
        let (_, again) = hit("POST", &format!("/chess/{}/{}/draw", id, white), None).await;
        assert_eq!(again["message"], "Your draw offer is already waiting.");
        let (_, seen) = hit("GET", &format!("/chess/{}/{}/state", id, black), None).await;
        assert_eq!(seen["draw_offer"], "them");
        let (_, taken) = hit("POST", &format!("/chess/{}/{}/draw", id, black), None).await;
        assert_eq!(taken["over"], true);
        let done = store::db().and_then(|db| store::get_game(&db.lock(), id)).expect("the game");
        assert_eq!(done.result.as_deref(), Some("draw"));
        assert_eq!(done.winner, None);
    }

    /// Serves one real board page so it can be looked at in a browser:
    /// `cargo test chess_board_demo -- --ignored --nocapture`, then open the
    /// links it prints. `CHESS_DEMO_SECS` says how long it stays up.
    #[tokio::test]
    #[ignore = "a demo server, not a test"]
    async fn chess_board_demo() {
        let (id, white, black) = live_game();
        if let Some(db) = store::db() {
            let conn = db.lock();
            let mut replay = Replay::new();
            for text in ["e4", "e5", "Nf3", "Nc6", "Bc4", "Nf6", "d3", "Bc5", "O-O"] {
                let m = rules::read_move(&replay.position, text).expect("a legal opening");
                replay.push(m);
            }
            let now = chrono::Utc::now().timestamp();
            store::play_move(&conn, id, &[], &replay.sans.join(" "), &replay.fen(), now - 18 * 60).expect("the moves");
        }
        let app = super::super::tests::panel();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:8797").await.expect("a port");
        println!("chess demo: http://127.0.0.1:8797/chess/{}/{} (white)", id, white);
        println!("chess demo: http://127.0.0.1:8797/chess/{}/{} (black)", id, black);
        let secs: u64 = std::env::var("CHESS_DEMO_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
        tokio::select! {
            _ = async { axum::serve(listener, app.into_make_service()).await } => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
        }
    }

    #[test]
    fn the_page_and_its_script_are_built_in_and_the_script_is_a_separate_file() {
        assert!(PAGE_HTML.contains("/assets/chess.js"), "the script is loaded, not inlined");
        assert!(!PAGE_HTML.contains("<script>"), "an inline script would be blocked by the page's own policy");
        assert!(PAGE_JS.contains("/state"), "the script talks to the state endpoint");
        assert!(!PAGE_JS.contains("localStorage"), "a board link must leave nothing behind in the browser");
    }
}
