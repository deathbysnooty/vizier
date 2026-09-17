//! The private chess-puzzle page.
//!
//! Whoever presses **🧩 Solve it** gets a random link of their own,
//! `/puzzle/<id>/<token>`, handed to them in an ephemeral Discord reply. The
//! link is the whole of the authentication: it names the puzzle AND the person,
//! and it is thrown away when the puzzle is replaced. These routes therefore sit
//! OUTSIDE `/api`, where the panel's session cookie and admin check live: a
//! member is not an admin and must never need to sign in to solve a puzzle.
//!
//! The page itself is static; its script reads the puzzle and the token out of
//! the address bar and talks to the two endpoints below. **Nothing it is ever
//! sent contains the solution**: the legal moves are every legal move in the
//! position, not the right one, and the line only ever comes back one move at a
//! time as it is played. Whether a move is right is decided here, by the server.

use std::borrow::Cow;

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};
use shakmaty::Position;

use super::super::super::puzzle;
use super::super::super::puzzle_rules as rules;
use super::super::super::puzzle_store::{self as store, Row};
use super::super::ui;
use super::Panel;

// Built into the binary; a copy in `<runtime>/ui` is served instead when there
// is one — see [`super::super::ui`].
const PAGE_HTML: &str = include_str!("../ui/puzzle.html");
const PAGE_JS: &str = include_str!("../ui/puzzle.js");

/// The puzzle page's routes. They go on the outer router, so neither the panel
/// session nor the `X-Panel` header is in the way.
pub fn routes() -> Router<Panel> {
    Router::new()
        .route("/puzzle/{id}/{token}", get(page))
        .route("/puzzle/{id}/{token}/state", get(state))
        .route("/puzzle/{id}/{token}/move", post(play))
        .route("/assets/puzzle.js", get(|| async { asset("text/javascript; charset=utf-8", ui::file("puzzle.js", PAGE_JS)) }))
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

/// The same answer for a link that never existed, a link for another puzzle,
/// and a link whose puzzle has been replaced: nothing here may say which.
fn unknown() -> Response {
    json_out(StatusCode::NOT_FOUND, json!({"error": "This puzzle link isn't open."}))
}

/// Who a link belongs to, and only if it belongs to the puzzle in the address.
pub fn opens(puzzle_id: i64, token: &str) -> Option<(Row, u64)> {
    let db = store::db()?;
    let conn = db.lock();
    let (owned, user) = store::token_owner(&conn, token)?;
    if owned != puzzle_id {
        return None;
    }
    let row = store::get(&conn, puzzle_id)?;
    if !row.open() {
        return None;
    }
    Some((row, user))
}

async fn page(Path((id, token)): Path<(i64, String)>) -> Response {
    let page = ui::file("puzzle.html", PAGE_HTML);
    match opens(id, &token) {
        Some(_) => html(StatusCode::OK, page),
        None => html(StatusCode::NOT_FOUND, page),
    }
}

/// Everything the page draws, for ONE person.
///
/// The `legal` list is every legal move in the position — which is what lets the
/// page light up where a piece may go — and NOT the solution. The solution
/// reaches the page one move at a time, as each move is played and accepted.
pub fn state_json(row: &Row, player: Option<&store::Player>) -> Value {
    let done = player.map(|p| p.done).unwrap_or(0) as usize;
    let solved = player.map(|p| p.solved).unwrap_or(false);
    let shown = position_after(row, done);
    let legal: Vec<Value> = shown
        .as_ref()
        .filter(|_| !solved)
        .map(|pos| {
            pos.legal_moves()
                .iter()
                .map(|m| {
                    let long = super::super::super::chess_rules::long_form(*m);
                    json!({
                        "from": long.get(0..2).unwrap_or_default(),
                        "to": long.get(2..4).unwrap_or_default(),
                        "promotion": long.get(4..5),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let fen = shown.as_ref().map(rules::fen_of).unwrap_or_else(|| row.fen.clone());
    let check = shown.as_ref().map(rules::check_square).unwrap_or_default();
    json!({
        "puzzle": row.id,
        "you": if row.solver_is_white() { "white" } else { "black" },
        "fen": fen,
        "legal": legal,
        "last": last_squares(row, done),
        "check": check,
        "done": done,
        "total": row.solver_moves(),
        "solved": solved,
        "wrongs": player.map(|p| p.wrongs).unwrap_or(0),
        "band": row.band,
        "rating": row.rating,
        "setup": row.setup_san,
        "played": played_san(row, done),
        "worth": puzzle::band_points(&row.band),
        "first_taken": row.first_solver.is_some(),
        "pays_house_points": puzzle::pays_house_points(),
    })
}

/// The position after `done` of the solver's moves, replies included.
fn position_after(row: &Row, done: usize) -> Option<shakmaty::Chess> {
    let mut pos = rules::position_of(&row.fen)?;
    for uci in row.line.iter().take(done * 2) {
        pos = rules::play(&pos, uci)?;
    }
    Some(pos)
}

/// The squares the last move on the board went between: the opponent's
/// setting-up move at the start, and their latest reply after that.
fn last_squares(row: &Row, done: usize) -> String {
    if done == 0 {
        // UCI writes castling as the king's own two-square move, so the first
        // four characters are always the pair to highlight.
        return row.setup.get(..4).unwrap_or_default().to_string();
    }
    // The move before the position they are looking at: the opponent's reply to
    // their last one, or their own move when the line has no reply left.
    let back = if row.line.len() > done * 2 - 1 { done * 2 - 1 } else { done * 2 - 2 };
    let Some(pos) = position_after(row, 0).map(|mut pos| {
        for uci in row.line.iter().take(back) {
            match rules::play(&pos, uci) {
                Some(next) => pos = next,
                None => break,
            }
        }
        pos
    }) else {
        return String::new();
    };
    row.line.get(back).and_then(|uci| rules::squares_of(&pos, uci)).unwrap_or_default()
}

/// The moves played so far — theirs and the answers — in the notation a person
/// reads. Only as far as they have got: the rest is the answer.
fn played_san(row: &Row, done: usize) -> Vec<String> {
    let Some(mut pos) = rules::position_of(&row.fen) else { return Vec::new() };
    let mut out = Vec::new();
    for uci in row.line.iter().take(done * 2) {
        let Some(san) = rules::san_of(&pos, uci) else { break };
        out.push(san);
        match rules::play(&pos, uci) {
            Some(next) => pos = next,
            None => break,
        }
    }
    out
}

fn look(row: &Row, user: u64) -> Value {
    let player = store::db().and_then(|db| {
        let conn = db.lock();
        store::player(&conn, row.id, user)
    });
    state_json(row, player.as_ref())
}

async fn state(Path((id, token)): Path<(i64, String)>) -> Response {
    match opens(id, &token) {
        None => unknown(),
        Some((row, user)) => json_out(StatusCode::OK, look(&row, user)),
    }
}

#[derive(Deserialize)]
pub struct MoveBody {
    /// A move in long form; the page sends `e2e4` or `e7e8q`.
    #[serde(rename = "move")]
    pub the_move: String,
}

async fn play(Path((id, token)): Path<(i64, String)>, body: axum::body::Bytes) -> Response {
    let Some((_, me)) = opens(id, &token) else { return unknown() };
    let typed = serde_json::from_slice::<MoveBody>(&body).map(|b| b.the_move).unwrap_or_default();
    let Some(attempt) = puzzle::attempt(me, id, &typed) else { return unknown() };
    let after = opens(id, &token).map(|(row, _)| look(&row, me));
    json_out(
        StatusCode::OK,
        json!({
            "ok": attempt.ok,
            "solved": attempt.solved,
            "first": attempt.first,
            "san": attempt.san,
            "reply": attempt.reply,
            "message": strip_markup(&attempt.words),
            "state": after,
        }),
    )
}

/// Discord's markdown means nothing on a web page, so the lines the game writes
/// for both — "**Bd5+** — right." — are flattened before they go out.
fn strip_markup(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body as AxumBody;
    use axum::http::Request;
    use tower::ServiceExt as _;

    use super::super::super::super::puzzle_bank::tests::fixture;

    /// A real puzzle in the test store, with a link for one player. `id` is the
    /// bank id; the position the solver sees is worked out the way the game
    /// works it out, so these tests exercise the real setting-up.
    fn live_puzzle(bank_id: &str) -> (i64, String) {
        super::super::tests::store();
        let p = fixture().get(bank_id).expect("a puzzle").clone();
        let open = rules::open_with(&p.fen, &p.setup).expect("the opening");
        let db = store::db().expect("the puzzle store");
        let conn = db.lock();
        let row = store::add(
            &conn,
            &p.id,
            &open.fen,
            &p.setup,
            &open.setup_san,
            &p.line,
            open.solver_is_white,
            p.rating as i64,
            &p.band,
            &p.themes,
            5,
            chrono::Utc::now().timestamp(),
        )
        .expect("a puzzle");
        let mut n = row.id as u64 * 7 + 1;
        let mut roll = || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (n >> 11) as f64 / (1u64 << 53) as f64
        };
        let token = store::token_for(&conn, row.id, 111, 1_000, &mut roll).expect("a link");
        (row.id, token)
    }

    /// A second person's link for the same puzzle.
    fn link_for(puzzle_id: i64, user: u64) -> String {
        let db = store::db().expect("the puzzle store");
        let conn = db.lock();
        let mut n = puzzle_id as u64 * 13 + user;
        let mut roll = || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (n >> 11) as f64 / (1u64 << 53) as f64
        };
        store::begin(&conn, puzzle_id, user, 1_000).expect("an attempt");
        store::token_for(&conn, puzzle_id, user, 1_000, &mut roll).expect("a link")
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
    async fn the_page_shows_the_position_after_the_opponents_move_not_the_stored_one() {
        let (id, token) = live_puzzle("WqnOB");
        let (status, state) = hit("GET", &format!("/puzzle/{}/{}/state", id, token), None).await;
        assert_eq!(status, StatusCode::OK);
        // The bank's FEN says black to move; the page is white to move, with the
        // bishop already on c8.
        assert_eq!(state["you"], "white");
        assert!(state["fen"].as_str().unwrap().starts_with("2b3k1/"), "{}", state["fen"]);
        assert!(state["fen"].as_str().unwrap().contains(" w "), "white to move: {}", state["fen"]);
        assert_eq!(state["setup"], "Bc8", "the card and the page name the blunder");
        assert_eq!(state["last"], "e6c8", "and the picture highlights it");
        assert_eq!((state["done"].as_i64(), state["total"].as_i64()), (Some(0), Some(2)));
        assert_eq!(state["solved"], false);
        assert!(state["played"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn nothing_the_page_is_sent_gives_the_solution_away() {
        let (id, token) = live_puzzle("WqnOB");
        let (_, state) = hit("GET", &format!("/puzzle/{}/{}/state", id, token), None).await;
        let text = state.to_string();
        assert!(!text.contains(&token), "a page is not told its own link either: {}", text);
        // The legal list is EVERY legal move, which is what lights the squares
        // up — and the solution is one of many, which gives nothing away.
        let legal = state["legal"].as_array().expect("moves");
        assert!(legal.len() > 5, "every legal move is sent, not the right one: {}", legal.len());
        // The rest of the line is nowhere in it.
        assert!(!text.contains("d5a2") && !text.contains("Bxa2"), "the answer must not be sent: {}", text);
    }

    #[tokio::test]
    async fn a_correct_line_is_taken_move_by_move_and_a_wrong_move_doesnt_end_the_attempt() {
        let (id, token) = live_puzzle("WqnOB");
        let path = format!("/puzzle/{}/{}/move", id, token);

        // Wrong: refused, counted, and the attempt goes on from the same board.
        let (status, answer) = hit("POST", &path, Some(json!({"move": "g2f3"}))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["message"], "That's not it — try again.");
        assert_eq!(answer["state"]["done"], 0, "they are still at the start");
        assert_eq!(answer["state"]["wrongs"], 1);
        assert_eq!(answer["state"]["solved"], false);
        // Nonsense is refused the same way, and just as harmlessly.
        let (_, answer) = hit("POST", &path, Some(json!({"move": "nonsense"}))).await;
        assert_eq!(answer["ok"], false);
        assert_eq!(answer["state"]["done"], 0);

        // The first move of the line: right, and the opponent answers for itself.
        let (_, answer) = hit("POST", &path, Some(json!({"move": "g2d5"}))).await;
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["solved"], false);
        assert_eq!(answer["san"], "Bd5+");
        assert_eq!(answer["reply"], "Kg7", "the bot plays the other side");
        assert_eq!(answer["message"], "Bd5+ — right. They answered Kg7.", "no Discord markup on a web page");
        assert_eq!(answer["state"]["done"], 1);
        assert_eq!(answer["state"]["played"].as_array().unwrap().len(), 2);

        // And the last move finishes it.
        let (_, answer) = hit("POST", &path, Some(json!({"move": "d5a2"}))).await;
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["solved"], true);
        assert_eq!(answer["san"], "Bxa2");
        assert_eq!(answer["state"]["solved"], true);
        assert_eq!(answer["state"]["legal"].as_array().unwrap().len(), 0, "a solved puzzle offers no more moves");
        // Playing on changes nothing.
        let (_, again) = hit("POST", &path, Some(json!({"move": "a2b1"}))).await;
        assert_eq!(again["ok"], false);
        assert!(again["message"].as_str().unwrap().contains("already solved"));
    }

    #[tokio::test]
    async fn a_different_mate_is_accepted_because_it_is_a_mate() {
        // The recorded answer here is Qxh5#. A solver who mates another way has
        // solved it, and being told "that's not it" would be plainly wrong.
        let (id, token) = live_puzzle("4kYFv");
        let (_, answer) = hit("POST", &format!("/puzzle/{}/{}/move", id, token), Some(json!({"move": "d1h5"}))).await;
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["solved"], true);
        assert_eq!(answer["san"], "Qxh5#");
    }

    #[tokio::test]
    async fn a_promotion_a_castle_and_an_en_passant_move_all_go_through_the_page() {
        // The three moves a naive UCI handler gets wrong. The promotion and the
        // castle are the puzzles' own setting-up moves, so a page that could not
        // play them would not have a board at all.
        let (promo, token) = live_puzzle("Zs7l3");
        let (_, state) = hit("GET", &format!("/puzzle/{}/{}/state", promo, token), None).await;
        assert_eq!(state["setup"], "f8=Q", "a promotion was played to reach this board");
        assert_eq!(state["you"], "black");
        assert!(state["fen"].as_str().unwrap().starts_with("3K1Q2/"), "the new queen is on f8: {}", state["fen"]);
        // A promotion the solver plays goes through with its piece on the end.
        assert!(state["legal"].as_array().unwrap().iter().any(|m| m["from"] == "a2" && m["to"] == "a8"));
        let (_, answer) = hit("POST", &format!("/puzzle/{}/{}/move", promo, token), Some(json!({"move": "a2a8"}))).await;
        assert_eq!(answer["ok"], true, "{}", answer["message"]);
        assert_eq!(answer["san"], "Ra8+");

        let (castle, token) = live_puzzle("LhHU9");
        let (_, state) = hit("GET", &format!("/puzzle/{}/{}/state", castle, token), None).await;
        assert_eq!(state["setup"], "O-O-O", "e1c1 is the king's move and it castles");
        assert_eq!(state["last"], "e1c1", "and the picture highlights the KING's squares");
        assert!(state["fen"].as_str().unwrap().contains("/2KR2NR"), "the rook came with it: {}", state["fen"]);
        let (_, answer) = hit("POST", &format!("/puzzle/{}/{}/move", castle, token), Some(json!({"move": "b4a2"}))).await;
        assert_eq!(answer["ok"], true);
        assert_eq!(answer["solved"], true);
        assert_eq!(answer["san"], "Nxa2#");

        // And an en-passant capture the SOLVER plays, at the end of its line.
        let (ep, token) = live_puzzle("I4ZCY");
        let path = format!("/puzzle/{}/{}/move", ep, token);
        let (_, answer) = hit("POST", &path, Some(json!({"move": "e3e4"}))).await;
        assert_eq!(answer["ok"], true, "{}", answer["message"]);
        assert_eq!(answer["reply"], "f5", "the pawn comes two squares, which is what makes it possible");
        let (_, answer) = hit("POST", &path, Some(json!({"move": "e5f6"}))).await;
        assert_eq!(answer["ok"], true, "{}", answer["message"]);
        assert_eq!(answer["san"], "exf6", "written as the pawn's own diagonal move");
        assert_eq!(answer["solved"], true);
    }

    #[tokio::test]
    async fn only_the_first_solver_takes_the_house_point_and_later_ones_still_score() {
        let (id, mine) = live_puzzle("4kYFv");
        let theirs = link_for(id, 222);
        // 111 gets there first.
        let (_, first) = hit("POST", &format!("/puzzle/{}/{}/move", id, mine), Some(json!({"move": "d1h5"}))).await;
        assert_eq!(first["solved"], true);
        assert_eq!(first["first"], true);
        // 222 solves the same puzzle afterwards: still a solve, still scored,
        // but not first.
        let (_, second) = hit("POST", &format!("/puzzle/{}/{}/move", id, theirs), Some(json!({"move": "d1h5"}))).await;
        assert_eq!(second["solved"], true);
        assert_eq!(second["first"], false, "there is only ever one first");
        let db = store::db().expect("the store");
        let conn = db.lock();
        let row = store::get(&conn, id).expect("the puzzle");
        assert_eq!(row.first_solver, Some(111));
        assert_eq!(row.solvers, 2, "both are counted");
        assert!(row.ends_at.is_some(), "the window for everyone else starts when the first one lands");
        let solves = store::solvers_of(&conn, id);
        assert_eq!(solves.iter().map(|s| (s.user, s.first, s.worth)).collect::<Vec<_>>(), vec![(111, true, 1), (222, false, 1)]);
        // House points are OFF by default, so neither of them was paid one —
        // and both still scored their puzzle point.
        assert!(solves.iter().all(|s| s.points == 0), "no house points while the limit is nought");
    }

    #[tokio::test]
    async fn a_wrong_link_gets_nothing_and_says_nothing() {
        let (id, token) = live_puzzle("4kYFv");
        let (other, _) = live_puzzle("WqnOB");
        assert_ne!(id, other);
        for path in [
            format!("/puzzle/{}/{}/state", other, token),
            format!("/puzzle/{}/{}/state", id, "bcdfghjkmnpqrstvwxyz2345"),
            format!("/puzzle/{}/{}/state", id, "short"),
            format!("/puzzle/{}/{}/state", id, token.to_uppercase()),
        ] {
            let (status, body) = hit("GET", &path, None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{} should open nothing", path);
            assert_eq!(body["error"], "This puzzle link isn't open.", "every refusal reads the same");
        }
        // And a move through a link that isn't open lands nowhere.
        let (status, _) = hit("POST", &format!("/puzzle/{}/{}/move", other, token), Some(json!({"move": "d1h5"}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_page_is_served_for_a_good_link_and_a_bad_one_looks_the_same() {
        let (id, token) = live_puzzle("4kYFv");
        let good = hit("GET", &format!("/puzzle/{}/{}", id, token), None).await;
        assert_eq!(good.0, StatusCode::OK);
        assert!(good.1.as_str().unwrap_or_default().contains("/assets/puzzle.js"));
        let bad = hit("GET", &format!("/puzzle/{}/{}", id, "bcdfghjkmnpqrstvwxyz2345"), None).await;
        assert_eq!(bad.0, StatusCode::NOT_FOUND);
        assert_eq!(bad.1, good.1, "the same page either way: the status is the only difference");
    }

    /// Serves one real puzzle page so it can be looked at in a browser:
    /// `cargo test puzzle_page_demo -- --ignored --nocapture`, then open the
    /// links it prints. `PUZZLE_DEMO_SECS` says how long it stays up.
    #[tokio::test]
    #[ignore = "a demo server, not a test"]
    async fn puzzle_page_demo() {
        let (two, token) = live_puzzle("WqnOB");
        let (mate, mate_token) = live_puzzle("4kYFv");
        let (castle, castle_token) = live_puzzle("LhHU9");
        let app = super::super::tests::panel();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:8798").await.expect("a port");
        println!("puzzle demo: http://127.0.0.1:8798/puzzle/{}/{} (a two-move line, white to play)", two, token);
        println!("puzzle demo: http://127.0.0.1:8798/puzzle/{}/{} (a mate in one)", mate, mate_token);
        println!("puzzle demo: http://127.0.0.1:8798/puzzle/{}/{} (black to play, after a castle)", castle, castle_token);
        let secs: u64 = std::env::var("PUZZLE_DEMO_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(120);
        tokio::select! {
            _ = async { axum::serve(listener, app.into_make_service()).await } => {}
            _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
        }
    }

    #[test]
    fn the_lines_the_page_shows_lose_their_discord_markup() {
        assert_eq!(strip_markup("**Bd5+** — right. They answered **Kg7**."), "Bd5+ — right. They answered Kg7.");
        assert_eq!(strip_markup("`/puzzletop`"), "/puzzletop");
    }

    #[test]
    fn the_page_and_its_script_are_built_in_and_the_script_is_a_separate_file() {
        assert!(PAGE_HTML.contains("/assets/puzzle.js"), "the script is loaded, not inlined");
        assert!(!PAGE_HTML.contains("<script>"), "an inline script would be blocked by the page's own policy");
        assert!(PAGE_JS.contains("/state"), "the script talks to the state endpoint");
        assert!(!PAGE_JS.contains("localStorage"), "a puzzle link must leave nothing behind in the browser");
        // The page says the quiet part out loud, the way the card does.
        assert!(PAGE_HTML.contains("engine"), "the page admits an engine would solve it instantly");
    }
}
