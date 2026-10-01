//! The Month page: where the egg week stands, and the two buttons a mod needs.
//!
//! The panel's other pages are each about one feature. This one is about a
//! month, so it answers the five questions a mod actually asks during one, and
//! nothing else:
//!
//! 1. **Who is in it?** How many eggs are out, who is on the sign-up sheet
//!    without one, and - the question that matters most in the first three days
//!    - who has an egg and has not played at all. That list is the one a mod
//!    does something about.
//! 2. **What is everybody hungry for right now?** Per member, and counted per
//!    game, so "is anybody playing Koto this evening" has an answer before
//!    somebody complains that nobody is.
//! 3. **How long to the hatch?**
//! 4. **Did the sorting come out even?** After the hatch, the dragon names, the
//!    house each person landed in, and the four totals the deal produced - the
//!    split itself, so a mod can see it was fair rather than be told.
//! 5. **Where are the cards?** Who holds which, by serial, and which of the
//!    twelve nobody has caught yet.
//!
//! WRITES. Two, both audited by key like every other panel write: `month:hatch`
//! (and `month:hatch:dry`, which writes nothing but is logged all the same,
//! because a dry run is how a mod decides) and `month:craving`. The hatch is
//! the only destructive thing on the page and it refuses to run twice, the same
//! refusal `/hatch` gives.
//!
//! Everything is read from the month's own stores - no second source of truth
//! for a single number on it - and nothing here is public: admins only, behind
//! the panel's session, like every page under `/api`.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::extract::State;
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::egg::{self, GAMES};
use super::super::super::egg_store::{self as store, Egg};
use super::super::super::frog_store;
use super::super::super::hatch;
use super::super::super::house;
use super::super::super::month;
use super::super::super::points::{self as ledger, Group};
use super::super::super::raven;
use super::super::super::signup_store;
use super::msglog::member_json;
use super::{ApiError, ApiResult, Caller, Panel, ok};

fn eggs_db() -> Result<&'static parking_lot::Mutex<rusqlite::Connection>, ApiError> {
    store::db().ok_or_else(|| {
        ApiError(StatusCode::SERVICE_UNAVAILABLE, "The egg store isn't open. Restart the bot.".into())
    })
}

/// One member's row on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub user: u64,
    pub name: String,
    pub seat: i64,
    pub claimed_ts: i64,
    pub hatch_ts: i64,
    pub house: String,
    pub dragon: String,
    pub opened: bool,
    /// Points earned since their egg was claimed.
    pub points: i64,
    /// Their egg's stage, 0 to 4.
    pub stage: usize,
    pub craving: Vec<&'static str>,
    /// Cards held: (name, rarity key, serial).
    pub cards: Vec<(String, &'static str, i64)>,
}

impl Row {
    /// Has an egg and has not scored a single point. The list a mod acts on.
    pub fn quiet(&self) -> bool {
        self.points <= 0
    }
}

/// How the week stands, counted rather than guessed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub eggs: usize,
    /// On the sign-up sheet, said yes, no egg yet.
    pub unclaimed: usize,
    pub quiet: usize,
    pub opened: usize,
    /// Eggs by stage, 0 to 4.
    pub stages: [usize; 5],
}

/// Counts the rows. Pure, so the numbers on the page are a thing a test can
/// check against a hand-made week.
pub fn tally(rows: &[Row], signed_up: &HashSet<u64>) -> Tally {
    let have: HashSet<u64> = rows.iter().map(|r| r.user).collect();
    let mut stages = [0usize; 5];
    for row in rows.iter().filter(|r| !r.opened) {
        if let Some(slot) = stages.get_mut(row.stage) {
            *slot += 1;
        }
    }
    Tally {
        eggs: rows.len(),
        unclaimed: signed_up.iter().filter(|u| !have.contains(u)).count(),
        quiet: rows.iter().filter(|r| r.quiet()).count(),
        opened: rows.iter().filter(|r| r.opened).count(),
        stages,
    }
}

/// How many eggs are hungry for each game right now.
pub fn hunger(rows: &[Row]) -> BTreeMap<&'static str, usize> {
    let mut out: BTreeMap<&'static str, usize> = GAMES.iter().map(|g| (g.key(), 0)).collect();
    for row in rows {
        for game in &row.craving {
            if let Some(slot) = out.get_mut(game) {
                *slot += 1;
            }
        }
    }
    out
}

/// The four houses as the deal left them: points, headcount, and the gap.
pub fn split_now(rows: &[Row]) -> (Vec<(&'static str, String, String, i64, usize)>, i64) {
    let worn = month::themed_all();
    let mut out = Vec::new();
    for one in &worn {
        let mine = rows.iter().filter(|r| r.house == one.key);
        out.push((one.key, one.name.clone(), one.crest.clone(), mine.clone().map(|r| r.points).sum(), mine.count()));
    }
    let high = out.iter().map(|(_, _, _, n, _)| *n).max().unwrap_or(0);
    let low = out.iter().map(|(_, _, _, n, _)| *n).min().unwrap_or(0);
    out.sort_by(|a, b| b.3.cmp(&a.3).then(a.1.cmp(&b.1)));
    (out, high - low)
}

/// Every row the page shows, read off the month's own stores.
fn read_rows(now: i64) -> Result<Vec<Row>, ApiError> {
    let eggs: Vec<Egg> = {
        let conn = eggs_db()?.lock();
        store::all(&conn)
    };
    if eggs.is_empty() {
        return Ok(Vec::new());
    }
    let craving: HashMap<u64, Vec<&'static str>> = {
        let conn = eggs_db()?.lock();
        eggs.iter()
            .map(|e| (e.user, egg::craving_of(&conn, e, now).into_iter().map(|s| s.key()).collect()))
            .collect()
    };
    let mut cards: HashMap<u64, Vec<(String, &'static str, i64)>> = HashMap::new();
    if let Some(db) = frog_store::db() {
        let conn = db.lock();
        let users: Vec<u64> = eggs.iter().map(|e| e.user).collect();
        for card in frog_store::cards_of_users(&conn, &users) {
            cards.entry(card.user_id).or_default().push((card.wizard_name, card.rarity.key(), card.serial));
        }
    }
    let month_start = ledger::month_start(now);
    let points: HashMap<u64, i64> = match house::db() {
        Some(db) => {
            let conn = db.lock();
            eggs.iter()
                .map(|e| {
                    let since = e.claimed_ts.max(month_start);
                    let n: i64 = ledger::breakdown(&conn, e.user, since)
                        .map(|rows| rows.iter().map(|(_, p)| p).sum())
                        .unwrap_or(0);
                    (e.user, n)
                })
                .collect()
        }
        None => HashMap::new(),
    };
    Ok(eggs
        .into_iter()
        .map(|e| {
            let earned = points.get(&e.user).copied().unwrap_or(0);
            let (stage, _) = egg::stage(earned);
            let mut theirs = cards.remove(&e.user).unwrap_or_default();
            theirs.sort_by_key(|(_, _, serial)| *serial);
            Row {
                user: e.user,
                name: e.name,
                seat: e.seat,
                claimed_ts: e.claimed_ts,
                hatch_ts: e.hatch_ts,
                house: e.house,
                dragon: e.dragon,
                opened: e.hatched_ts.is_some(),
                points: earned,
                stage,
                craving: craving.get(&e.user).cloned().unwrap_or_default(),
                cards: theirs,
            }
        })
        .collect())
}

fn row_json(panel: &Panel, row: &Row) -> Value {
    let mut member = member_json(panel, row.user, &row.name, "");
    if let Value::Object(ref mut map) = member {
        map.insert("seat".into(), json!(row.seat));
        map.insert("claimed_ts".into(), json!(row.claimed_ts));
        map.insert("hatch_ts".into(), json!(row.hatch_ts));
        map.insert("points".into(), json!(row.points));
        map.insert("quiet".into(), json!(row.quiet()));
        map.insert("egg_stage".into(), json!(row.stage));
        map.insert("egg_stage_label".into(), json!(egg::STAGES[row.stage.min(4)].1));
        map.insert("opened".into(), json!(row.opened));
        map.insert("craving".into(), json!(row.craving));
        map.insert("dragon".into(), json!(row.dragon));
        map.insert(
            "house".into(),
            match house::house(&row.house).map(month::themed) {
                Some(worn) => json!({ "key": worn.key, "name": worn.name, "crest": worn.crest }),
                None => Value::Null,
            },
        );
        map.insert(
            "cards".into(),
            json!(
                row.cards
                    .iter()
                    .map(|(name, rarity, serial)| json!({
                        "name": name,
                        "rarity": rarity,
                        "serial": serial,
                        "serial_label": frog_store::serial_label(*serial),
                    }))
                    .collect::<Vec<_>>()
            ),
        );
    }
    member
}

/// The deck: every card in play, how many are out, and who holds them.
fn deck_json(panel: &Panel, rows: &[Row]) -> Value {
    let (wizards, copies) = match frog_store::db() {
        Some(db) => {
            let conn = db.lock();
            (frog_store::wizards(&conn), frog_store::copies(&conn))
        }
        None => (Vec::new(), HashMap::new()),
    };
    let held: HashMap<&str, Vec<(u64, i64)>> = {
        let mut out: HashMap<&str, Vec<(u64, i64)>> = HashMap::new();
        for row in rows {
            for (name, _, serial) in &row.cards {
                out.entry(name.as_str()).or_default().push((row.user, *serial));
            }
        }
        out
    };
    let cards: Vec<Value> = wizards
        .iter()
        .map(|w| {
            let mut holders = held.get(w.name.as_str()).cloned().unwrap_or_default();
            holders.sort_by_key(|(_, serial)| *serial);
            json!({
                "slug": w.slug,
                "name": w.name,
                "rarity": w.rarity.key(),
                "rarity_name": w.rarity.name(),
                "emoji": w.rarity.emoji(),
                "points": w.rarity.points(),
                "weight": w.rarity.weight(),
                "in_play": w.enabled,
                // The art is referenced by the slug: frogcards/<slug>.png.
                "art": format!("{}.png", w.slug),
                "printed": copies.get(&w.id).copied().unwrap_or(0),
                "uncaught": copies.get(&w.id).copied().unwrap_or(0) == 0,
                "holders": holders
                    .iter()
                    .map(|(user, serial)| {
                        let mut m = member_json(panel, *user, "", "");
                        if let Value::Object(ref mut map) = m {
                            map.insert("serial".into(), json!(serial));
                            map.insert("serial_label".into(), json!(frog_store::serial_label(*serial)));
                        }
                        m
                    })
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "cards": cards,
        "deck_line": raven::deck_line(),
        "in_play": wizards.iter().filter(|w| w.enabled).count(),
        "uncaught": wizards.iter().filter(|w| w.enabled && copies.get(&w.id).copied().unwrap_or(0) == 0).count(),
    })
}

// --- the page ----------------------------------------------------------------------------

pub async fn overview(
    State(panel): State<Panel>,
    axum::Extension(Caller(user)): axum::Extension<Caller>,
) -> ApiResult {
    let now = chrono::Utc::now().timestamp();
    let rows = read_rows(now)?;
    let hatched = {
        let conn = eggs_db()?.lock();
        store::hatched_at(&conn)
    };
    let signed_up: HashSet<u64> = match signup_store::db() {
        Some(db) => {
            let conn = db.lock();
            signup_store::listed(&conn, signup_store::Answer::In)
                .unwrap_or_default()
                .into_iter()
                .map(|e| e.user_id)
                .collect()
        }
        None => HashSet::new(),
    };
    let counted = tally(&rows, &signed_up);
    let (split, gap) = split_now(&rows);
    let slot = egg::slot_of(now);

    // Today's usage against each shared limit, server-wide, so the caps page
    // shows what the day has actually cost rather than only what it allows.
    let used_today: Vec<Value> = Group::ALL
        .into_iter()
        .map(|group| {
            let used: i64 = rows.iter().map(|r| r.user).map(|u| group.sources().iter().map(|s| house::earned_on(u, *s, now)).sum::<i64>()).sum();
            let maxed = rows
                .iter()
                .filter(|r| group.sources().iter().map(|s| house::earned_on(r.user, *s, now)).sum::<i64>() >= group.limit())
                .count();
            json!({
                "key": group.key(),
                "label": group.label(),
                "limit": group.limit(),
                "games": group.sources().iter().map(|s| s.key()).collect::<Vec<_>>(),
                "used_by_everyone": used,
                "members_maxed": maxed,
            })
        })
        .collect();

    // Sorted, because a set's order is nobody's friend and a list that
    // reshuffles itself on every refresh is unreadable.
    let no_egg = {
        let mut waiting: Vec<u64> =
            signed_up.iter().copied().filter(|u| !rows.iter().any(|r| r.user == *u)).collect();
        waiting.sort_unstable();
        let mut out: Vec<Value> = waiting.into_iter().map(|u| member_json(&panel, u, "", "")).collect();
        out.sort_by(|a, b| a["name"].as_str().unwrap_or("").cmp(b["name"].as_str().unwrap_or("")));
        out
    };
    let overridden = {
        let conn = eggs_db()?.lock();
        store::override_for(&conn, slot)
    };
    super::search::log_quietly("month:look", user, "Looked at the Month page");
    ok(json!({
        "on": month::running(),
        "title": month::title(),
        "hatch_at": hatch_at_json(),
        "hatched_at": hatched,
        "egg_week": hatched.is_none() && now < month::hatch_at(),
        "now": now,
        "watch_days": month::watch_days(),
        "craving": {
            "slot": slot,
            "changes_at": egg::slot_ends(slot),
            "hours": month::craving_hours(),
            "per_egg": month::cravings(),
            "games": GAMES.iter().map(|g| json!({ "key": g.key(), "label": g.label() })).collect::<Vec<_>>(),
            "counts": hunger(&rows),
            "overridden": overridden,
        },
        "tally": {
            "eggs": counted.eggs,
            "unclaimed": counted.unclaimed,
            "quiet": counted.quiet,
            "opened": counted.opened,
            "stages": counted.stages.iter().enumerate().map(|(i, n)| json!({
                "stage": i,
                "icon": egg::STAGES[i].0,
                "label": egg::STAGES[i].1,
                "eggs": n,
            })).collect::<Vec<_>>(),
        },
        "houses": split.iter().map(|(key, name, crest, points, members)| json!({
            "key": key, "name": name, "crest": crest, "points": points, "members": members,
        })).collect::<Vec<_>>(),
        "gap": gap,
        "caps": used_today,
        "members": rows.iter().map(|r| row_json(&panel, r)).collect::<Vec<_>>(),
        "quiet_members": rows.iter().filter(|r| r.quiet()).map(|r| row_json(&panel, r)).collect::<Vec<_>>(),
        "signed_up_without_an_egg": no_egg,
        "deck": deck_json(&panel, &rows),
    }))
}

/// The hatch as a moment and as the setting that spells it.
fn hatch_at_json() -> Value {
    json!(month::hatch_at())
}

#[derive(Deserialize, Default)]
struct HatchBody {
    #[serde(default)]
    dry: bool,
}

/// The split, from the panel. `dry` shows it and writes nothing.
pub async fn run_hatch(
    State(panel): State<Panel>,
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let b: HatchBody = if body.is_empty() { HatchBody::default() } else { serde_json::from_slice(&body).unwrap_or_default() };
    if !month::running() {
        return Err(ApiError::bad("There's no themed month running. Switch Themed month on first."));
    }
    let now = chrono::Utc::now().timestamp();
    let (already, waiting, taken) = {
        let conn = eggs_db()?.lock();
        (store::hatched_at(&conn), store::unhatched(&conn), store::dragons(&conn))
    };
    if already.is_some() && !b.dry {
        return Err(ApiError(StatusCode::CONFLICT, hatch::ALREADY.into()));
    }
    if waiting.is_empty() {
        return Err(ApiError::bad("There are no unopened eggs. Nobody has claimed one yet."));
    }
    // Exactly the weights `/hatch` uses, through exactly the same deal, so the
    // panel's dry run is the split the command would apply and not a second
    // opinion about it.
    let weights: Vec<(u64, i64)> = match house::db() {
        Some(db) => {
            let conn = db.lock();
            let month_start = ledger::month_start(now);
            waiting
                .iter()
                .map(|e| {
                    let since = e.claimed_ts.max(month_start);
                    let n: i64 = ledger::breakdown(&conn, e.user, since)
                        .map(|rows| rows.iter().map(|(_, p)| p).sum())
                        .unwrap_or(0);
                    (e.user, n.max(0))
                })
                .collect()
        }
        None => waiting.iter().map(|e| (e.user, 0)).collect(),
    };
    let split = hatch::deal(&weights, &taken);

    if !b.dry {
        let conn = eggs_db()?.lock();
        for one in &split.dealt {
            match store::hatch(&conn, one.user, one.house, &one.dragon, now) {
                Ok(true) => {}
                Ok(false) => tracing::warn!("month: {}'s egg was already open, left as it was", one.user),
                Err(err) => tracing::error!("month: {} not written ({})", one.user, err),
            }
            if let Some(h) = house::house(one.house) {
                house::place(one.user, h, "hatch");
            }
        }
        let _ = store::mark_hatched(&conn, now);
    }

    // Logged either way, and the dry run says so in the log: a dry run is how a
    // mod decides, so who looked and what they were shown is worth keeping.
    let facts = json!({
        "dry": b.dry,
        "eggs": split.dealt.len(),
        "gap": split.gap(),
        "houses": split.totals.iter().map(|(key, points, count)| json!({ "key": key, "points": points, "members": count })).collect::<Vec<_>>(),
    })
    .to_string();
    let key = if b.dry { "month:hatch:dry" } else { "month:hatch" };
    super::super::log_change(key, None, Some(&facts), admin).map_err(ApiError::internal)?;

    ok(json!({
        "dry": b.dry,
        "eggs": split.dealt.len(),
        "gap": split.gap(),
        "head_gap": split.head_gap(),
        "reveal": hatch::reveal_text(&split, b.dry),
        "houses": split.totals.iter().map(|(key, points, count)| {
            let worn = house::house(key).map(month::themed);
            json!({
                "key": key,
                "name": worn.as_ref().map(|w| w.name.clone()),
                "crest": worn.as_ref().map(|w| w.crest.clone()),
                "points": points,
                "members": count,
            })
        }).collect::<Vec<_>>(),
        "dealt": split.dealt.iter().map(|d| {
            let mut m = member_json(&panel, d.user, "", "");
            if let Value::Object(ref mut map) = m {
                map.insert("points".into(), json!(d.points));
                map.insert("dragon".into(), json!(d.dragon));
                map.insert("house".into(), json!(d.house));
            }
            m
        }).collect::<Vec<_>>(),
    }))
}

#[derive(Deserialize, Default)]
struct CravingBody {
    /// Comma-separated game keys, or empty to put the rotation back.
    #[serde(default)]
    games: String,
}

/// Overrides, or clears, what every egg is hungry for in this slot.
pub async fn set_craving(
    axum::Extension(Caller(admin)): axum::Extension<Caller>,
    body: axum::body::Bytes,
) -> ApiResult {
    let b: CravingBody = serde_json::from_slice(&body).map_err(|_| ApiError::bad("Name the games, or send none to clear it."))?;
    let wanted: Vec<&str> =
        b.games.split(',').map(|p| p.trim()).filter(|p| ledger::Source::from_key(p).is_some_and(egg::is_game)).collect();
    if !b.games.trim().is_empty() && wanted.is_empty() {
        let keys: Vec<&str> = GAMES.iter().map(|g| g.key()).collect();
        return Err(ApiError::bad(format!("I don't know those games. Pick from: {}", keys.join(", "))));
    }
    let now = chrono::Utc::now().timestamp();
    let slot = egg::slot_of(now);
    {
        let conn = eggs_db()?.lock();
        store::set_override(&conn, slot, &wanted, admin, now)
            .map_err(|err| ApiError::internal(format!("egg store: {}", err)))?;
    }
    let facts = json!({ "slot": slot, "games": wanted }).to_string();
    super::super::log_change(&format!("month:craving:{}", slot), None, Some(&facts), admin).map_err(ApiError::internal)?;
    ok(json!({ "slot": slot, "games": wanted, "changes_at": egg::slot_ends(slot), "cleared": wanted.is_empty() }))
}

/// How this page's writes read in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    let what = if e.key == "month:look" {
        "Looked at the Month page".to_string()
    } else if e.key == "month:hatch:dry" {
        "Ran a dry hatch - nothing was written".to_string()
    } else if e.key == "month:hatch" {
        "Opened every egg and dealt the houses".to_string()
    } else if e.key.starts_with("month:craving") {
        "Changed what the eggs are hungry for".to_string()
    } else {
        "The Month page".to_string()
    };
    obj.insert("what".into(), json!(what));
    obj.insert("area".into(), json!("Themed month"));
    obj
}

#[cfg(test)]
mod tests {
    use super::super::super::super::month::testing::Month;
    use super::*;

    fn row(user: u64, points: i64, house: &str, craving: &[&'static str]) -> Row {
        let (stage, _) = egg::stage(points);
        Row {
            user,
            name: format!("member{}", user),
            seat: user as i64,
            claimed_ts: 0,
            hatch_ts: 1_000,
            house: house.to_string(),
            dragon: if house.is_empty() { String::new() } else { format!("Dragon{}", user) },
            opened: !house.is_empty(),
            points,
            stage,
            craving: craving.to_vec(),
            cards: Vec::new(),
        }
    }

    #[test]
    fn the_tally_counts_the_eggs_the_quiet_and_the_ones_still_missing() {
        let _month = Month::on();
        let rows = vec![
            row(1, 300, "", &["anagram", "quiz"]),
            row(2, 40, "", &["cat", "koto"]),
            row(3, 0, "", &["guess", "geo"]),
            row(4, 0, "", &["movie", "anagram"]),
        ];
        // Two members said yes on the sheet and have no egg yet.
        let signed: HashSet<u64> = HashSet::from([1, 2, 3, 4, 90, 91]);
        let counted = tally(&rows, &signed);
        assert_eq!(counted.eggs, 4);
        assert_eq!(counted.unclaimed, 2, "the two on the sheet without an egg");
        assert_eq!(counted.quiet, 2, "the two who have not scored at all");
        assert_eq!(counted.opened, 0);
        // Stages: 300 is the top stage, 40 is the middle, the two noughts are cold.
        assert_eq!(counted.stages.iter().sum::<usize>(), 4);
        assert_eq!(counted.stages[0], 2, "two cold eggs");
        assert_eq!(counted.stages[4], 1, "one cracking");
        assert!(rows[2].quiet() && !rows[0].quiet());
    }

    #[test]
    fn the_hunger_is_counted_per_game_so_an_empty_game_shows() {
        let rows = vec![row(1, 0, "", &["anagram", "quiz"]), row(2, 0, "", &["anagram", "cat"])];
        let counts = hunger(&rows);
        assert_eq!(counts.len(), GAMES.len(), "every game has a number, even nought");
        assert_eq!(counts.get("anagram"), Some(&2));
        assert_eq!(counts.get("quiz"), Some(&1));
        assert_eq!(counts.get("movie"), Some(&0), "a game nobody wants reads as nought, not as missing");
    }

    #[test]
    fn the_split_shows_whether_the_deal_came_out_even() {
        let _month = Month::on();
        let rows = vec![
            row(1, 500, "gryffindor", &[]),
            row(2, 480, "slytherin", &[]),
            row(3, 490, "ravenclaw", &[]),
            row(4, 505, "hufflepuff", &[]),
            row(5, 10, "gryffindor", &[]),
        ];
        let (split, gap) = split_now(&rows);
        assert_eq!(split.len(), 4);
        assert_eq!(gap, 510 - 480, "the gap is the thing a mod is checking");
        assert!(split.windows(2).all(|w| w[0].3 >= w[1].3), "most points first");
        // Named as the month paints them, not as Hogwarts.
        let names: Vec<&str> = split.iter().map(|(_, name, _, _, _)| name.as_str()).collect();
        for wanted in ["Stark", "Lannister", "Targaryen", "Night's Watch"] {
            assert!(names.contains(&wanted), "{} is missing from {:?}", wanted, names);
        }
        assert_eq!(split.iter().map(|(_, _, _, _, n)| n).sum::<usize>(), 5);
    }

    #[test]
    fn with_nobody_dealt_the_four_houses_still_show_at_nought() {
        let _month = Month::on();
        let (split, gap) = split_now(&[]);
        assert_eq!(split.len(), 4);
        assert_eq!(gap, 0);
        assert!(split.iter().all(|(_, _, _, points, members)| *points == 0 && *members == 0));
        let counted = tally(&[], &HashSet::new());
        assert_eq!(counted, Tally::default());
    }

    #[test]
    fn the_log_says_which_of_the_two_hatches_was_run() {
        let entry = |key: &str| super::super::super::AuditEntry {
            ts: 0,
            user_id: "1".to_string(),
            key: key.to_string(),
            old: None,
            new: None,
        };
        assert_eq!(audit_entry(&entry("month:hatch"))["what"], "Opened every egg and dealt the houses");
        assert_eq!(audit_entry(&entry("month:hatch:dry"))["what"], "Ran a dry hatch - nothing was written");
        assert_eq!(audit_entry(&entry("month:craving:4"))["what"], "Changed what the eggs are hungry for");
        assert_eq!(audit_entry(&entry("month:look"))["area"], "Themed month");
    }
}
