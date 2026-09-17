//! The sudoku page, at `/sudoku/<number>`.
//!
//! This is the one part of the panel's server that is PUBLIC: no sign-in, no
//! cookie, no `X-Panel` header, and nothing under `/api`. It has to be — the
//! link goes to any member who presses ▶️ Play, and members are not panel
//! admins. So it gives away as little as it can:
//!
//! * only the puzzle's givens, its number, its difficulty and when it went up;
//! * never the answer, which stays in the bot;
//! * no cookies, no names, nothing about who is looking;
//! * a small bucket per address, so it can't be hammered.
//!
//! The page itself is three files built into the binary, the way the panel's
//! own page is.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;

use super::super::super::sudoku_gen::{Grid, Level, grid_to_str};
use super::super::super::sudoku_store as store;
use super::super::ui;

// Built into the binary; a copy in `<runtime>/ui` is served instead when there
// is one — see [`super::super::ui`].
const PAGE_HTML: &str = include_str!("../ui/sudoku.html");
const PAGE_CSS: &str = include_str!("../ui/sudoku.css");
const PAGE_JS: &str = include_str!("../ui/sudoku.js");

/// Requests one address may make in a minute before it is asked to wait.
pub const PER_MINUTE: usize = 60;

type Buckets = HashMap<String, (f64, Instant)>;

fn buckets() -> &'static Mutex<Buckets> {
    static BUCKETS: OnceLock<Mutex<Buckets>> = OnceLock::new();
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A leaky bucket: [`PER_MINUTE`] requests to spend, refilled steadily. True
/// when this address may be served now.
pub fn allow(ip: &str, now: Instant) -> bool {
    let full = PER_MINUTE as f64;
    let mut buckets = buckets().lock();
    // Addresses that have been quiet long enough to be full again are forgotten.
    buckets.retain(|_, (tokens, at)| *tokens + now.duration_since(*at).as_secs_f64() * full / 60.0 < full);
    let entry = buckets.entry(ip.to_string()).or_insert((full, now));
    let refilled = (entry.0 + now.duration_since(entry.1).as_secs_f64() * full / 60.0).min(full);
    entry.1 = now;
    if refilled < 1.0 {
        entry.0 = refilled;
        return false;
    }
    entry.0 = refilled - 1.0;
    true
}

/// Every public sudoku request goes through the bucket first.
pub async fn rate_limit(req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = super::client_ip(req.headers(), peer);
    if !allow(&ip, Instant::now()) {
        return (StatusCode::TOO_MANY_REQUESTS, "Slow down a moment, then reload.").into_response();
    }
    next.run(req).await
}

fn served(kind: &'static str, body: String, status: StatusCode) -> Response {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    // A puzzle's givens never change, but its page is small; a short cache
    // keeps a reload cheap without ever showing a stale puzzle.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
    res
}

pub async fn css() -> Response {
    served("text/css; charset=utf-8", ui::file("sudoku.css", PAGE_CSS).into_owned(), StatusCode::OK)
}

pub async fn js() -> Response {
    served("text/javascript; charset=utf-8", ui::file("sudoku.js", PAGE_JS).into_owned(), StatusCode::OK)
}

/// Text that can't break out of an attribute or a tag.
fn esc(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&#39;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// What the page says under the title: still up for grabs, or already won.
pub fn headline(status: store::Status) -> String {
    match status {
        store::Status::Open => "MLCI House Cup · the first correct code in #sudoku wins".to_string(),
        store::Status::Solved => "Someone has already won this one · finish it anyway and the bot will say if you were right".to_string(),
        store::Status::Skipped => "This puzzle was skipped · finish it anyway and the bot will say if you were right".to_string(),
    }
}

/// The page for one puzzle. Only the givens go in: the answer never does.
pub fn render(row: &store::Row) -> String {
    let points = plural(row.points, "sudoku point", "sudoku points");
    let fields: [(&str, String); 10] = [
        ("{{ID}}", row.id.to_string()),
        ("{{GIVENS}}", grid_to_str(&row.givens)),
        ("{{POINTS}}", row.points.to_string()),
        ("{{POINTS_WORDS}}", points),
        ("{{LEVEL_KEY}}", row.level.key().to_string()),
        ("{{LEVEL_NAME}}", row.level.name().to_string()),
        ("{{LEVEL_EMOJI}}", row.level.emoji().to_string()),
        ("{{POSTED}}", row.posted_ts.to_string()),
        ("{{OPEN}}", if row.status == store::Status::Open { "1".to_string() } else { "0".to_string() }),
        ("{{HEADLINE}}", headline(row.status)),
    ];
    let mut page = ui::file("sudoku.html", PAGE_HTML).into_owned();
    for (token, value) in fields {
        page = page.replace(token, &esc(&value));
    }
    page
}

/// What someone sees when the number doesn't name a puzzle.
fn missing(id: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"en\" data-theme=\"dark\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <meta name=\"robots\" content=\"noindex, nofollow\"><title>No such puzzle</title>\
         <link rel=\"stylesheet\" href=\"/sudoku/app.css\"></head><body><main class=\"wrap gone\">\
         <h1>No puzzle #{}</h1><p class=\"how\">That link doesn't point at a sudoku. \
         Press ▶️ <b>Play</b> on the puzzle card in Discord for the one that's up now.</p>\
         </main></body></html>",
        esc(id)
    )
}

pub async fn page(Path(id): Path<String>) -> Response {
    let found = id.parse::<i64>().ok().filter(|n| *n > 0).and_then(|n| store::db().and_then(|db| store::get(&db.lock(), n)));
    match found {
        Some(row) => served("text/html; charset=utf-8", render(&row), StatusCode::OK),
        None => served("text/html; charset=utf-8", missing(&id), StatusCode::NOT_FOUND),
    }
}

/// A puzzle as the page needs it, for tests and for anything else that wants
/// the givens without the answer.
pub fn givens_only(row: &store::Row) -> (i64, Level, Grid) {
    (row.id, row.level, row.givens)
}

/// How many pages are in the memory of the rate limiter, for tests.
#[cfg(test)]
pub fn watched() -> usize {
    buckets().lock().len()
}

/// A page's own copy of the bucket, so one test can't starve another.
#[cfg(test)]
pub fn forget_all() {
    buckets().lock().clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::sudoku_gen::grid_from_str;

    fn row() -> store::Row {
        let conn = crate::channels::discord::sudoku_store::tests::memory();
        crate::channels::discord::sudoku_store::tests::put(&conn, Level::Medium, 4, 1_700_000_000)
    }

    #[test]
    fn the_page_shows_the_givens_and_never_the_answer() {
        let row = row();
        let html = render(&row);
        assert!(html.contains(&format!("data-givens=\"{}\"", grid_to_str(&row.givens))), "the givens are missing");
        assert!(html.contains("Sudoku #1"), "{}", &html[..200]);
        assert!(html.contains("🟡 Medium · 4 sudoku points"));
        assert!(html.contains("data-posted=\"1700000000\""));
        let answer = grid_to_str(&row.solution);
        assert!(!html.contains(&answer), "the answer is on the page");
        // Not even a few squares of it: every run of digits on the page is the
        // givens, nothing longer.
        for start in 0..40 {
            let piece: String = answer.chars().skip(start).take(24).collect();
            assert!(!html.contains(&piece), "part of the answer is on the page: {}", piece);
        }
        assert!(html.contains("/sudoku/app.js") && html.contains("/sudoku/app.css"));
        assert!(!html.contains("{{"), "a placeholder was left behind");
    }

    #[test]
    fn a_won_puzzle_still_opens_and_says_so() {
        let conn = crate::channels::discord::sudoku_store::tests::memory();
        let row = crate::channels::discord::sudoku_store::tests::put(&conn, Level::Hard, 6, 1_000);
        store::claim(&conn, row.id, 5, 1_500).unwrap();
        let won = store::get(&conn, row.id).unwrap();
        let html = render(&won);
        assert!(html.contains("already won this one"), "{}", headline(won.status));
        assert!(html.contains("data-open=\"0\""));
        assert!(headline(store::Status::Open).contains("first correct code"));
        assert!(headline(store::Status::Skipped).contains("skipped"));
    }

    #[test]
    fn a_made_up_number_gets_a_friendly_page() {
        let html = missing("<script>alert(1)</script>");
        assert!(!html.contains("<script>"), "the number must not be able to write html");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("No puzzle"));
    }

    #[test]
    fn the_bucket_lets_a_burst_through_and_then_asks_for_a_wait() {
        forget_all();
        let start = Instant::now();
        for i in 0..PER_MINUTE {
            assert!(allow("1.2.3.4", start), "request {} was refused", i);
        }
        assert!(!allow("1.2.3.4", start), "the bucket never ran out");
        // Another address is untouched, and a wait refills the first.
        assert!(allow("5.6.7.8", start));
        assert!(allow("1.2.3.4", start + std::time::Duration::from_secs(2)), "two seconds buys two more");
        forget_all();
        assert_eq!(watched(), 0);
    }

    /// Writes the real page to disk so it can be looked at in a browser:
    ///
    /// ```sh
    /// SUDOKU_SHOT_DIR=/tmp/sudoku-page cargo test sudoku_page_to_disk -- --ignored
    /// python3 -m http.server -d /tmp/sudoku-page 8712
    /// ```
    ///
    /// `index.html` is a fresh puzzle; `demo.html` is the same page with some
    /// squares and pencil marks already filled in, for a picture of it in use.
    #[test]
    #[ignore]
    fn sudoku_page_to_disk() {
        let dir = std::env::var("SUDOKU_SHOT_DIR").unwrap_or_else(|_| "/tmp/sudoku-page".to_string());
        let root = std::path::Path::new(&dir);
        std::fs::create_dir_all(root.join("sudoku")).expect("a place to write");
        std::fs::write(root.join("sudoku/app.css"), PAGE_CSS).unwrap();
        std::fs::write(root.join("sudoku/app.js"), PAGE_JS).unwrap();
        let conn = crate::channels::discord::sudoku_store::tests::memory();
        let mut row = crate::channels::discord::sudoku_store::tests::put(&conn, Level::Medium, 4, 0);
        row.id = 128;
        row.posted_ts = chrono::Utc::now().timestamp() - 374;
        let page = render(&row);
        std::fs::write(root.join("index.html"), &page).unwrap();
        // The same page with a game in progress, so the picture shows the
        // colours: a few answers, a few pencil marks, one clash.
        let blanks: Vec<usize> = (0..81).filter(|c| row.givens[*c] == 0).collect();
        let mut filled: Vec<u8> = row.givens.to_vec();
        for cell in blanks.iter().take(8) {
            filled[*cell] = row.solution[*cell];
        }
        let clash = blanks[9];
        filled[clash] = row.givens[(clash / 9) * 9 + (0..9).find(|c| row.givens[(clash / 9) * 9 + c] != 0).unwrap_or(0)];
        let notes: Vec<u32> = (0..81).map(|c| if blanks.iter().skip(12).take(4).any(|b| *b == c) { 0b1_0001_0101 } else { 0 }).collect();
        let saved = format!(
            "{{\"filled\":[{}],\"notes\":[{}]}}",
            filled.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(","),
            notes.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",")
        );
        let seed = format!(
            "<script>try{{localStorage.setItem('sudoku:{}', {});}}catch(e){{}}\
             window.addEventListener('load', function () {{ document.getElementById('check').click(); }});</script>",
            row.id,
            serde_json::to_string(&saved).unwrap()
        );
        let demo = page.replace("<script src=\"/sudoku/app.js\" defer></script>", &format!("{}<script src=\"/sudoku/app.js\" defer></script>", seed));
        std::fs::write(root.join("demo.html"), demo).unwrap();
        println!("sudoku page written to {}", root.display());
    }

    #[test]
    fn nothing_in_the_page_can_be_written_by_a_puzzle() {
        assert_eq!(esc("<b>&\"'"), "&lt;b&gt;&amp;&quot;&#39;");
        let row = row();
        let (id, level, givens) = givens_only(&row);
        assert_eq!((id, level), (row.id, Level::Medium));
        assert_eq!(givens_only(&row).2, row.givens);
        assert_eq!(grid_from_str(&grid_to_str(&givens)), Some(givens));
    }
}
