//! The live page's data, at `/live.json`.
//!
//! The page itself is the owner's; this is the one thing it reads. Public, no
//! sign-in, no cookie, no token - which is the whole reason the shape of it is
//! decided here and not left to whatever a query happened to return.
//!
//! WHAT IT WILL AND WILL NOT SAY. It says names, dragons, houses, points,
//! ranks and the serial numbers of the cards people hold - every one of which
//! the bot already says out loud in a public Discord channel many times a day.
//! It says nothing else at all. There are **no user ids** on it (so a row
//! cannot be joined to a Discord profile by a stranger), no message of
//! anybody's, no message counts, no join dates, no voice minutes, no channel
//! names, no email, no avatar hash beyond the public CDN url Discord serves to
//! anybody, and nothing whatever about who is looking. The privacy test below
//! walks the finished JSON key by key and fails on anything that looks like one
//! of those, so a field added carelessly in six months' time fails a test
//! rather than reaching the web.
//!
//! WHAT IT COSTS. One pass over three SQLite files, behind a one-minute cache
//! (`VIZIER_LIVE_CACHE_SECS`). A hundred people with the page open cost one
//! pass a minute between them, not a hundred. Names and avatars come from the
//! gateway cache only - never a call to Discord per member - so a member the
//! cache has not seen is drawn by the name their egg was claimed under.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::channels::discord::control;
use crate::channels::discord::egg;
use crate::channels::discord::egg_store;
use crate::channels::discord::frog_store::{self, Rarity};
use crate::channels::discord::house;
use crate::channels::discord::month;
use crate::channels::discord::points::{self as ledger, Group, Source};

use super::Panel;

/// How long one assembled page is served for.
fn cache_secs() -> u64 {
    control::number("VIZIER_LIVE_CACHE_SECS", 60).clamp(5, 600)
}

/// How many days of the day-by-day line to send.
const DAYS: usize = 31;

// --- what one row is built from ------------------------------------------------------

/// One member's whole row, before any of it is counted or ranked.
///
/// Everything is handed in rather than read here, so [`assemble`] is a pure
/// function and the tests can hand it September's real shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// Used to look the member up while assembling, and then dropped: it is
    /// never put in the JSON.
    pub user: u64,
    pub name: String,
    /// The public CDN url Discord serves to anybody, or empty.
    pub avatar: String,
    /// The ledger key of their house, empty before the hatch.
    pub house: String,
    pub dragon: String,
    /// Points per game this month, in no particular order.
    pub per_game: Vec<(Source, i64)>,
    /// Points per India day, oldest first: (`YYYY-MM-DD`, points).
    pub per_day: Vec<(String, i64)>,
    /// What they have earned today against each shared limit.
    pub today: Vec<(Group, i64)>,
    pub craving: Vec<Source>,
    pub craving_until: i64,
    /// The cards they hold: (card name, rarity, serial).
    pub cards: Vec<(String, Rarity, i64)>,
    /// They hold at least one of every card in play.
    pub full_set: bool,
    /// Their egg's stage, 0 to 4.
    pub stage: usize,
}

/// One house as the standings show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HouseRow {
    pub key: &'static str,
    pub name: String,
    pub crest: String,
    pub colour: u32,
    pub points: i64,
    pub members: usize,
}

/// The whole page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub month: String,
    pub on: bool,
    pub hatched: bool,
    pub hatch_at: i64,
    pub craving_hours: i64,
    pub window_secs: i64,
    pub houses: Vec<HouseRow>,
    /// Members, most points first.
    pub members: Vec<Member>,
}

/// One member, counted and ranked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub avatar: String,
    pub house: Option<&'static str>,
    pub dragon: String,
    pub points: i64,
    /// 1-based; members level on points share a place.
    pub rank: usize,
    pub per_game: Vec<(Source, i64)>,
    pub per_day: Vec<(String, i64)>,
    pub today: Vec<(Group, i64, i64)>,
    pub craving: Vec<Source>,
    pub craving_until: i64,
    pub cards: Vec<(String, Rarity, i64)>,
    pub achievements: Vec<&'static str>,
    pub stage: usize,
}

/// What somebody has done worth a badge. Earned from the numbers already on the
/// page, so a badge can never say more than the page already does.
fn achievements(points: i64, cards: &[(String, Rarity, i64)], full_set: bool, rank: usize, maxed: bool) -> Vec<&'static str> {
    let mut out = Vec::new();
    if rank == 1 && points > 0 {
        out.push("top_of_the_server");
    }
    if points >= 1000 {
        out.push("thousand_points");
    } else if points >= 500 {
        out.push("five_hundred_points");
    } else if points >= 100 {
        out.push("hundred_points");
    }
    if maxed {
        out.push("maxed_a_limit_today");
    }
    if full_set {
        out.push("full_set");
    }
    if cards.iter().any(|(_, rarity, _)| *rarity == Rarity::Legendary) {
        out.push("holds_a_legendary");
    }
    if cards.iter().any(|(_, _, serial)| *serial <= 10) {
        out.push("single_digit_serial");
    }
    if cards.len() >= 10 {
        out.push("collector");
    }
    out
}

/// Counts, ranks and sorts the rows, and totals the four houses.
///
/// House totals are counted from the rows rather than from the ledger's own
/// house column on purpose: a member's points belong to the house they are in
/// NOW, which after the hatch is not the house they earned the first week in.
pub fn assemble(rows: Vec<Row>, hatched: Option<i64>) -> Live {
    let mut members: Vec<Member> = rows
        .into_iter()
        .map(|row| {
            let points: i64 = row.per_game.iter().map(|(_, n)| n).sum();
            let today: Vec<(Group, i64, i64)> =
                row.today.iter().map(|(group, used)| (*group, *used, group.limit())).collect();
            let maxed = today.iter().any(|(_, used, limit)| *limit > 0 && used >= limit);
            let mut per_game = row.per_game;
            per_game.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.key().cmp(b.0.key())));
            let mut cards = row.cards;
            cards.sort_by_key(|(_, _, serial)| *serial);
            Member {
                name: row.name,
                avatar: row.avatar,
                house: house::house(&row.house).map(|h| h.key),
                dragon: row.dragon,
                points,
                rank: 0,
                per_game,
                per_day: row.per_day,
                today,
                craving: row.craving,
                craving_until: row.craving_until,
                achievements: achievements(points, &cards, row.full_set, 1, maxed),
                cards,
                stage: row.stage,
            }
        })
        .collect();
    // Most points first; level members in name order, never a wobbly one.
    members.sort_by(|a, b| b.points.cmp(&a.points).then(a.name.cmp(&b.name)));
    let mut place = 0usize;
    let mut last = i64::MIN;
    for (i, member) in members.iter_mut().enumerate() {
        if member.points != last {
            place = i + 1;
            last = member.points;
        }
        member.rank = place;
        // The badges are worked out again now the place is known, so
        // "top of the server" is only ever on the person who is.
        let maxed = member.today.iter().any(|(_, used, limit)| *limit > 0 && used >= limit);
        let full_set = member.achievements.contains(&"full_set");
        member.achievements = achievements(member.points, &member.cards, full_set, place, maxed);
    }

    let mut houses: Vec<HouseRow> = month::themed_all()
        .into_iter()
        .map(|worn| {
            let mine = members.iter().filter(|m| m.house == Some(worn.key));
            HouseRow {
                key: worn.key,
                name: worn.name,
                crest: worn.crest,
                colour: worn.colour,
                points: mine.clone().map(|m| m.points).sum(),
                members: mine.count(),
            }
        })
        .collect();
    houses.sort_by(|a, b| b.points.cmp(&a.points).then(a.name.cmp(&b.name)));

    Live {
        month: month::title(),
        on: month::running(),
        hatched: hatched.is_some(),
        hatch_at: month::hatch_at(),
        craving_hours: month::craving_hours(),
        window_secs: month::window_secs(),
        houses,
        members,
    }
}

/// The JSON the page gets. Nothing is in here that is not in [`Live`], and
/// nothing in [`Live`] carries an id.
pub fn body(live: &Live, now: i64) -> Value {
    json!({
        "month": live.month,
        "on": live.on,
        "hatched": live.hatched,
        "hatch_at": live.hatch_at,
        "craving_hours": live.craving_hours,
        "answer_window_seconds": live.window_secs,
        "now": now,
        "cache_seconds": cache_secs(),
        "houses": live.houses.iter().map(|h| json!({
            "key": h.key,
            "name": h.name,
            "crest": h.crest,
            "colour": format!("#{:06X}", h.colour),
            "points": h.points,
            "members": h.members,
        })).collect::<Vec<_>>(),
        "members": live.members.iter().map(|m| json!({
            "name": m.name,
            "avatar": m.avatar,
            "house": m.house,
            "dragon": m.dragon,
            "points": m.points,
            "rank": m.rank,
            "egg_stage": m.stage,
            "per_game": m.per_game.iter().map(|(source, n)| json!({
                "game": source.key(),
                "label": source.label(),
                "points": n,
            })).collect::<Vec<_>>(),
            "per_day": m.per_day.iter().map(|(day, n)| json!({"day": day, "points": n})).collect::<Vec<_>>(),
            "today": m.today.iter().map(|(group, used, limit)| json!({
                "group": group.key(),
                "label": group.label(),
                "used": used,
                "limit": limit,
                "left": (limit - used).max(0),
            })).collect::<Vec<_>>(),
            "craving": m.craving.iter().map(|s| s.key()).collect::<Vec<_>>(),
            "craving_until": m.craving_until,
            "cards": m.cards.iter().map(|(name, rarity, serial)| json!({
                "name": name,
                "rarity": rarity.key(),
                "serial": serial,
                "serial_label": frog_store::serial_label(*serial),
                "art": null,
            })).collect::<Vec<_>>(),
            "achievements": m.achievements,
        })).collect::<Vec<_>>(),
    })
}

// --- reading it off the stores ---------------------------------------------------------

/// Everything the page needs, read in one pass over the three stores.
///
/// Each store's lock is taken and let go of in turn, never two at once, which
/// is the rule the rest of the panel's public pages follow.
pub fn read_live(panel: &Panel, now: i64) -> Option<Live> {
    let since = ledger::month_start(now);
    // Only members who are actually in the month: an egg, and not opted out.
    // Somebody who has opted out is hidden from the hall entirely - out means
    // out - and nothing of theirs is deleted by being hidden.
    let eggs = egg_store::db().map(|db| {
        let conn = db.lock();
        (egg_store::playing(&conn), egg_store::hatched_at(&conn))
    });
    let (eggs, hatched) = eggs?;
    if eggs.is_empty() {
        return Some(assemble(Vec::new(), hatched));
    }

    // The hunger, from the egg store, for everybody at once.
    let mut craving: HashMap<u64, (Vec<Source>, i64)> = HashMap::new();
    if let Some(db) = egg_store::db() {
        let conn = db.lock();
        for one in &eggs {
            craving.insert(one.user, (egg::craving_of(&conn, one, now), egg::slot_ends(egg::slot_of(now))));
        }
    }

    // The cards, from the frog store.
    let mut cards: HashMap<u64, Vec<(String, Rarity, i64)>> = HashMap::new();
    let mut in_play = 0usize;
    if let Some(db) = frog_store::db() {
        let conn = db.lock();
        in_play = frog_store::wizards(&conn).into_iter().filter(|w| w.enabled).count();
        let users: Vec<u64> = eggs.iter().map(|e| e.user).collect();
        for card in frog_store::cards_of_users(&conn, &users) {
            cards.entry(card.user_id).or_default().push((card.wizard_name, card.rarity, card.serial));
        }
    }

    // The points, from the house ledger.
    let mut rows = Vec::with_capacity(eggs.len());
    let db = house::db()?;
    let conn = db.lock();
    for one in &eggs {
        let per_game = ledger::breakdown(&conn, one.user, since).unwrap_or_default();
        let per_day = day_by_day(&conn, one.user, since);
        let today =
            Group::ALL.into_iter().map(|g| (g, g.sources().iter().map(|s| earned_today(&conn, one.user, *s, now)).sum())).collect();
        let theirs = cards.remove(&one.user).unwrap_or_default();
        let kinds: std::collections::HashSet<&String> = theirs.iter().map(|(name, _, _)| name).collect();
        let (hungry, until) = craving.remove(&one.user).unwrap_or_default();
        let points: i64 = per_game.iter().map(|(_, n)| n).sum();
        let (stage, _) = egg::stage(points);
        let (name, avatar) = panel.cached_face(one.user).unwrap_or_else(|| (one.name.clone(), String::new()));
        rows.push(Row {
            user: one.user,
            name: if name.trim().is_empty() { "A member".to_string() } else { name },
            avatar,
            house: one.house.clone(),
            dragon: one.dragon.clone(),
            per_game,
            per_day,
            today,
            craving: hungry,
            craving_until: until,
            full_set: in_play > 0 && kinds.len() >= in_play,
            cards: theirs,
            stage,
        });
    }
    Some(assemble(rows, hatched))
}

/// One member's points per India day since `since`, oldest first.
fn day_by_day(conn: &rusqlite::Connection, user: u64, since: i64) -> Vec<(String, i64)> {
    conn.prepare(
        "SELECT day, SUM(points) FROM ledger WHERE user_id = ?1 AND ts >= ?2 GROUP BY day ORDER BY day",
    )
    .and_then(|mut s| {
        s.query_map(rusqlite::params![user as i64, since], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()
    })
    .map(|mut rows| {
        if rows.len() > DAYS {
            rows.drain(..rows.len() - DAYS);
        }
        rows
    })
    .unwrap_or_default()
}

fn earned_today(conn: &rusqlite::Connection, user: u64, source: Source, now: i64) -> i64 {
    conn.query_row(
        "SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1 AND source = ?2 AND day = ?3 AND points > 0",
        rusqlite::params![user as i64, source.key(), ledger::ist_day(now)],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

// --- the route ---------------------------------------------------------------------------

static CACHE: LazyLock<Mutex<Option<(Instant, Value)>>> = LazyLock::new(|| Mutex::new(None));

fn cached(panel: &Panel, now: i64, at: Instant) -> Option<Value> {
    {
        let held = CACHE.lock();
        if let Some((made, body)) = held.as_ref() {
            if at.duration_since(*made) < Duration::from_secs(cache_secs()) {
                return Some(body.clone());
            }
        }
    }
    let live = read_live(panel, now)?;
    let body = body(&live, now);
    *CACHE.lock() = Some((at, body.clone()));
    Some(body)
}

/// Drops the cache, so a setting changed on the panel shows on the next read
/// rather than up to a minute later.
pub fn forget() {
    *CACHE.lock() = None;
}

fn json_out(status: StatusCode, body: Value) -> Response {
    let mut res = Response::new(Body::from(body.to_string()));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=30"));
    // Any page may read it: it is public by design, and the alternative is the
    // owner's own page being refused by the browser.
    h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
    res
}

async fn live_json(State(panel): State<Panel>) -> Response {
    let now = chrono::Utc::now().timestamp();
    match cached(&panel, now, Instant::now()) {
        Some(mut body) => {
            // `now` rides beside the cached body, so the page can say how old
            // the numbers are without trusting the clock it is drawn on.
            body["now"] = json!(now);
            json_out(StatusCode::OK, body)
        }
        None => json_out(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "The month's numbers aren't available right now."})),
    }
}

pub fn routes() -> Router<Panel> {
    Router::new().route("/live.json", get(live_json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::month::testing::Month;

    fn row(name: &str, house: &str, points: &[(Source, i64)]) -> Row {
        Row {
            user: name.len() as u64,
            name: name.to_string(),
            avatar: format!("https://cdn.discordapp.com/avatars/1/{}.png", name),
            house: house.to_string(),
            dragon: format!("{}ax", name),
            per_game: points.to_vec(),
            per_day: vec![("2026-10-01".into(), 10)],
            today: vec![(Group::Quick, 20), (Group::Thinking, 4)],
            craving: vec![Source::Anagram, Source::Quiz],
            craving_until: 1_000,
            cards: Vec::new(),
            full_set: false,
            stage: 2,
        }
    }

    /// September's shape again: a few heavy players and a long tail.
    fn a_server() -> Vec<Row> {
        let houses = ["gryffindor", "slytherin", "ravenclaw", "hufflepuff"];
        (0..53u64)
            .map(|i| {
                let points = match i {
                    0..=4 => 3000 - i as i64 * 200,
                    5..=19 => 900 - i as i64 * 30,
                    _ => (53 - i as i64) * 2,
                };
                let mut r = row(&format!("member{:02}", i), houses[(i % 4) as usize], &[(Source::Anagram, points)]);
                // Real-shaped Discord snowflakes, so the privacy test's search
                // for an id in the body means something: no points total, day
                // or timestamp on the page can look like one of these.
                r.user = 701234567890123456 + i;
                r
            })
            .collect()
    }

    #[test]
    fn the_rows_are_ranked_biggest_first_and_level_members_share_a_place() {
        let rows = vec![
            row("Ayan", "gryffindor", &[(Source::Quiz, 50)]),
            row("Bela", "slytherin", &[(Source::Quiz, 100)]),
            row("Chetan", "ravenclaw", &[(Source::Quiz, 50)]),
            row("Dia", "hufflepuff", &[(Source::Quiz, 0)]),
        ];
        let live = assemble(rows, None);
        assert_eq!(
            live.members.iter().map(|m| (m.name.as_str(), m.points, m.rank)).collect::<Vec<_>>(),
            vec![("Bela", 100, 1), ("Ayan", 50, 2), ("Chetan", 50, 2), ("Dia", 0, 4)],
            "level members share a place, and the next place skips"
        );
        assert_eq!(live.members[0].achievements, vec!["top_of_the_server", "hundred_points", "maxed_a_limit_today"]);
        assert!(!live.members[1].achievements.contains(&"top_of_the_server"), "only one person is top");
    }

    #[test]
    fn the_four_houses_are_totalled_from_the_members_and_sorted() {
        let _month = Month::off();
        let live = assemble(a_server(), Some(1_000));
        assert_eq!(live.houses.len(), 4);
        assert!(live.hatched);
        let total: i64 = live.members.iter().map(|m| m.points).sum();
        assert_eq!(live.houses.iter().map(|h| h.points).sum::<i64>(), total, "every point is in a house");
        assert!(live.houses.windows(2).all(|w| w[0].points >= w[1].points), "most points first");
        assert_eq!(live.houses.iter().map(|h| h.members).sum::<usize>(), 53);
        for h in &live.houses {
            assert!(!h.name.is_empty() && !h.crest.is_empty());
        }
    }

    #[test]
    fn the_houses_are_named_as_the_month_paints_them() {
        let _month = Month::on();
        let live = assemble(a_server(), Some(1_000));
        let names: Vec<&str> = live.houses.iter().map(|h| h.name.as_str()).collect();
        for wanted in ["Stark", "Lannister", "Targaryen", "Night's Watch"] {
            assert!(names.contains(&wanted), "{} is missing from {:?}", wanted, names);
        }
        assert_eq!(live.month, "Fire & Blood");
        assert!(live.on);
    }

    #[test]
    fn the_json_has_the_shape_the_page_was_built_against() {
        let _month = Month::on();
        let mut rows = a_server();
        rows[0].cards = vec![
            ("The Faceless Man".into(), Rarity::Legendary, 187),
            ("House Stark".into(), Rarity::Common, 3),
        ];
        rows[0].full_set = true;
        let live = assemble(rows, Some(1_000));
        let body = body(&live, 1_700_000_000);

        for key in [
            "month",
            "on",
            "hatched",
            "hatch_at",
            "craving_hours",
            "answer_window_seconds",
            "now",
            "cache_seconds",
            "houses",
            "members",
        ] {
            assert!(body.get(key).is_some(), "the page needs `{}`", key);
        }
        let house = &body["houses"][0];
        for key in ["key", "name", "crest", "colour", "points", "members"] {
            assert!(house.get(key).is_some(), "a house needs `{}`", key);
        }
        assert!(house["colour"].as_str().unwrap().starts_with('#'), "the colour is ready to use in CSS");

        let member = body["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| !m["cards"].as_array().unwrap().is_empty())
            .expect("the member with cards");
        for key in [
            "name",
            "avatar",
            "house",
            "dragon",
            "points",
            "rank",
            "egg_stage",
            "per_game",
            "per_day",
            "today",
            "craving",
            "craving_until",
            "cards",
            "achievements",
        ] {
            assert!(member.get(key).is_some(), "a member needs `{}`", key);
        }
        // The cards carry their serials, lowest first.
        let cards = member["cards"].as_array().unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0]["serial"], 3);
        assert_eq!(cards[1]["serial_label"], "No. 0187");
        assert_eq!(cards[1]["rarity"], "legendary");
        // Per-game, per-day and today all carry their labels, so the page needs
        // no table of its own.
        assert_eq!(member["per_game"][0]["game"], "anagram");
        assert!(member["per_day"][0]["day"].is_string());
        let quick = member["today"].as_array().unwrap().iter().find(|t| t["group"] == "quick").unwrap();
        assert_eq!((quick["used"].as_i64(), quick["limit"].as_i64(), quick["left"].as_i64()), (Some(20), Some(20), Some(0)));
        assert_eq!(member["craving"], json!(["anagram", "quiz"]));
        assert!(member["achievements"].as_array().unwrap().iter().any(|a| a == "full_set"));
    }

    /// The one that has to keep passing: nothing private may ever be on the
    /// page, however the code around it changes.
    #[test]
    fn the_page_gives_away_nothing_private() {
        let _month = Month::on();
        let mut rows = a_server();
        rows[0].cards = vec![("House Stark".into(), Rarity::Common, 1)];
        let body = body(&assemble(rows, Some(1_000)), 1_700_000_000);
        let text = body.to_string();

        /// Every key anywhere in the tree.
        fn keys(value: &Value, out: &mut Vec<String>) {
            match value {
                Value::Object(map) => {
                    for (key, inner) in map {
                        out.push(key.clone());
                        keys(inner, out);
                    }
                }
                Value::Array(items) => items.iter().for_each(|i| keys(i, out)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        keys(&body, &mut found);
        for key in &found {
            let lower = key.to_ascii_lowercase();
            for banned in [
                "id", "user", "discord", "email", "mention", "token", "ip", "message", "content", "text", "said",
                "join", "voice", "channel", "chat", "note", "dm", "hash", "session", "secret",
            ] {
                assert!(lower != banned, "`{}` has no business on a public page", key);
                assert!(!lower.ends_with(&format!("_{}", banned)), "`{}` has no business on a public page", key);
            }
        }
        // No Discord snowflake is anywhere in the body at all. Every member in
        // the fixture has one, and not one of them may appear.
        for user in 701234567890123456..701234567890123456 + 53u64 {
            assert!(!text.contains(&user.to_string()), "member {}'s id reached the page", user);
        }
        // And the only url on it is Discord's own public avatar CDN.
        for member in body["members"].as_array().unwrap() {
            let avatar = member["avatar"].as_str().unwrap_or("");
            assert!(
                avatar.is_empty() || avatar.starts_with("https://cdn.discordapp.com/"),
                "an avatar came from somewhere unexpected: {}",
                avatar
            );
        }
    }

    #[test]
    fn an_empty_server_is_still_a_page() {
        let _month = Month::on();
        let live = assemble(Vec::new(), None);
        assert!(live.members.is_empty());
        assert_eq!(live.houses.len(), 4, "the four houses are always there, at nought");
        assert!(live.houses.iter().all(|h| h.points == 0 && h.members == 0));
        assert!(!live.hatched);
        let body = body(&live, 0);
        assert_eq!(body["members"], json!([]));
    }

    #[test]
    fn before_the_hatch_nobody_has_a_house_or_a_dragon_and_the_eggs_have_stages() {
        let _month = Month::on();
        let rows: Vec<Row> = a_server()
            .into_iter()
            .map(|r| Row { house: String::new(), dragon: String::new(), ..r })
            .collect();
        let live = assemble(rows, None);
        assert!(!live.hatched);
        assert!(live.members.iter().all(|m| m.house.is_none() && m.dragon.is_empty()));
        assert!(live.houses.iter().all(|h| h.members == 0 && h.points == 0), "the standings wait for the hatch");
        let body = body(&live, 0);
        assert_eq!(body["members"][0]["house"], Value::Null);
        assert_eq!(body["members"][0]["egg_stage"], 2);
    }

    #[test]
    fn the_cache_window_is_a_minute_and_can_be_dropped() {
        let mut month = Month::on();
        assert_eq!(cache_secs(), 60);
        month.set("VIZIER_LIVE_CACHE_SECS", "1");
        assert_eq!(cache_secs(), 5, "too short is clamped to five seconds");
        month.set("VIZIER_LIVE_CACHE_SECS", "120");
        assert_eq!(cache_secs(), 120);
        forget();
        assert!(CACHE.lock().is_none());
    }
}
