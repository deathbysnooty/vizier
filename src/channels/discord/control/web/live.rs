//! The Hall of Dragons: the month's public page, and the two things it reads.
//!
//! ```text
//! GET /live                     the page (the hall)
//! GET /live/u/<discord id>      the page, opened on that person's own dragon
//! GET /live/api/hall.json       everybody: name, avatar, points, house, rank
//! GET /live/api/u/<id>.json     one person's whole page, in one call
//! GET /live/<file>              the page's own art, models and textures
//! GET /live.json                the hall's body, under its first address
//! ```
//!
//! Public, no sign-in, no cookie, no token: the owner's decision is that anyone
//! with the link can open it. Which is the whole reason the shape of what it
//! says is decided here and not left to whatever a query happened to return.
//!
//! A PERSON'S PAGE IS KEYED BY THEIR DISCORD ID, by the owner's decision: the
//! link `/egg` and `/livepoints` hand somebody is `<live url>/live/u/<their
//! id>`. So a page for one named account can be reached by anyone who learns an
//! id, and ids can be read in bulk out of any channel the bot posts in. What
//! that page then says is only what the bot already says out loud in public
//! channels - a display name, Discord's own avatar, points, a rank, cards and
//! badges - and the id is in the ADDRESS only: it is never echoed into any
//! body, any attribute or any query string, and the privacy test below enforces
//! that. If the owner would rather the addresses were unguessable, the House Cup
//! page already has the mechanism to copy - `housecup::handle`, a keyed hash of
//! the id - and the only change needed here is what `one_json` parses.
//!
//! WHAT IT WILL AND WILL NOT SAY. It says display names, Discord's own public
//! avatar urls, dragons, houses, points, ranks, the serial numbers of the cards
//! people hold, and badges earned from those same numbers - every one of which
//! the bot already says out loud in a public Discord channel many times a day.
//! It says nothing else at all. There are **no user ids in any body** (so a row
//! cannot be joined to a Discord profile by a stranger), no message of
//! anybody's, no message counts, no join dates, no voice minutes, no channel
//! names, no email, and nothing whatever about who is looking. A person's own
//! page is reached BY their id because the bot hands them that link - the id is
//! in the address, never in the answer. The privacy test below walks the
//! finished JSON key by key and fails on anything that looks like one of those,
//! so a field added carelessly in six months' time fails a test rather than
//! reaching the web.
//!
//! WHAT IT COSTS. One pass over three SQLite files, behind a one-minute cache
//! (`VIZIER_LIVE_CACHE_SECS`). A hundred people with the page open cost one
//! pass a minute between them, not a hundred. The hall's own body is the light
//! one - a dozen-odd small fields a member and not one list - so the view a
//! hundred and fifty people load on mobile data is a few tens of kilobytes; the
//! long per-game, per-day, cards and badges lists are sent only on the one page
//! that shows them. The page is the same: nobody downloads two megabytes of
//! dragon during a week in which there are no dragons. Names
//! and avatars come from the gateway cache only - never a call to Discord per
//! member - so a member the cache has not seen is drawn by the name their egg
//! was claimed under.
//!
//! EGG WEEK IS THE CLOCK'S DECISION, not a switch: [`Live::egg_week`] is true
//! while the hatch moment (`VIZIER_MONTH_HATCH`, read through
//! [`month::hatch_at`]) is still ahead, and stays true afterwards until a mod
//! has actually run `/hatch` - because dragons with no names and no houses are
//! worse than eggs. The page has no toggle and no day stepper: the ones the
//! design was previewed with are gone.
//!
//! The SCORING rule is deliberately not this rule and must not be made to
//! match it: [`egg::decide`] ends the egg week on the clock alone, so points
//! start flowing normally at noon on the 8th whether or not a mod ran the
//! ceremony - nobody loses points because a mod was asleep. `hatched` says
//! whether the ceremony has been run; `egg_week` says what the page draws.
//!
//! BADGES ARE DEFINED IN ONE PLACE, [`BADGES`] here, and nowhere else: there is
//! no achievements store and no second list. Every one of them is derived from
//! numbers already on the page, so a badge can never say more than the page
//! already does. Keep that rule when adding one.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::channels::discord::control;
use crate::channels::discord::control::live_ui;
use crate::channels::discord::egg;
use crate::channels::discord::egg_store;
use crate::channels::discord::frog_store::{self, Rarity};
use crate::channels::discord::house::{self, HOUSES};
use crate::channels::discord::month;
use crate::channels::discord::points::{self as ledger, Group, Source};

use super::Panel;

/// Whether the page works at all. On by default: a server with no themed month
/// running sees a page that says so, which is better than a dead link.
pub fn enabled() -> bool {
    control::on("VIZIER_LIVE", true)
}

/// How long one assembled page is served for.
fn cache_secs() -> u64 {
    control::number("VIZIER_LIVE_CACHE_SECS", 60).clamp(5, 600)
}

/// How many days of the day-by-day line to send.
const DAYS: usize = 31;

/// How long a browser may keep one of the page's art files. They are pushed by
/// hand and never change under their own name, so this is long.
const ART_CACHE: &str = "public, max-age=86400";

// --- what one row is built from ------------------------------------------------------

/// One card somebody holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    pub name: String,
    /// The card's slug, which is what the page's art is named after.
    pub slug: String,
    pub rarity: Rarity,
    pub serial: i64,
    /// The day of the India month it was caught on, for the card's own page.
    pub day: u32,
}

/// One card in play, held or not: the page greys the ones nobody has caught
/// yet, so it has to be told the whole deck rather than only what is held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kind {
    pub name: String,
    pub slug: String,
    pub rarity: Rarity,
}

/// One member's whole row, before any of it is counted or ranked.
///
/// Everything is handed in rather than read here, so [`assemble`] is a pure
/// function and the tests can hand it September's real shape.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    /// Used to look the member up, and never put in any body.
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
    pub cards: Vec<Held>,
    /// They hold at least one of every card in play.
    pub full_set: bool,
    /// Their egg's stage by what it has been fed, 0 to 4.
    pub stage: usize,
    /// When THEY claimed their egg, and when THEIR egg opens. The shared egg
    /// week is the watch that ends at the hatch moment, but somebody who
    /// claimed late is on a watch of their own, so the five pictures are dated
    /// from these rather than from the month's.
    pub egg_from: i64,
    pub egg_until: i64,
}

/// One house as the standings show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HouseRow {
    pub key: &'static str,
    /// Which of the four slots it is, in `HOUSES` order. The month's names are
    /// paint and move about; the slot never does, so it is what the page's own
    /// crest art is matched on.
    pub slot: usize,
    pub name: String,
    pub crest: String,
    pub colour: u32,
    pub points: i64,
    pub members: usize,
}

/// The whole month, as both bodies are cut from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub month: String,
    pub on: bool,
    /// A mod has run `/hatch`.
    pub hatched: bool,
    /// Eggs, not dragons: the hatch moment is still ahead, or it has passed and
    /// nobody has run `/hatch` yet.
    pub egg_week: bool,
    pub hatch_at: i64,
    pub hatched_at: Option<i64>,
    /// How long the shared egg week is, which is what dates its first day.
    pub egg_week_days: i64,
    pub craving_hours: i64,
    pub window_secs: i64,
    pub houses: Vec<HouseRow>,
    /// Every card in play, in rarity order. Filled by [`read_live`] after
    /// [`assemble`], which counts points and knows nothing about cards.
    pub deck: Vec<Kind>,
    /// Members, most points first.
    pub members: Vec<Member>,
}

/// One member, counted and ranked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// Never in a body: this is what `/live/api/u/<id>.json` looks them up by.
    pub user: u64,
    pub name: String,
    pub avatar: String,
    pub house: Option<&'static str>,
    pub slot: Option<usize>,
    pub dragon: String,
    pub points: i64,
    /// 1-based; members level on points share a place.
    pub rank: usize,
    /// How brightly their egg burns, 0 to 100: their points against the biggest
    /// pile on the page. Every egg in the hall is drawn the same SIZE - scale is
    /// the dragons' alone, and hatch day is the first time anybody sees it - so
    /// heat is the only thing that says who has been feeding theirs.
    pub heat: i64,
    pub per_game: Vec<(Source, i64)>,
    pub per_day: Vec<(String, i64)>,
    /// (group, earned today, the limit)
    pub today: Vec<(Group, i64, i64)>,
    pub craving: Vec<Source>,
    pub craving_until: i64,
    pub cards: Vec<Held>,
    pub achievements: Vec<&'static str>,
    pub stage: usize,
    pub egg_from: i64,
    pub egg_until: i64,
}

// --- badges -----------------------------------------------------------------------

/// What somebody has done worth a badge, earned from the numbers already on the
/// page - so a badge can never say more than the page already does, and there is
/// no second store to keep in step. This list is the only place they are
/// defined: the page draws whatever it is sent, icon, title and words included.
pub const BADGES: &[(&str, &str, &str, &str)] = &[
    ("top_of_the_server", "👑", "Top of the server", "first on the board this month"),
    ("thousand_points", "🐉", "Dragon grown", "passed a thousand points this month"),
    ("five_hundred_points", "🔥", "Five hundred", "passed five hundred points this month"),
    ("hundred_points", "✨", "First hundred", "passed a hundred points this month"),
    ("maxed_a_limit_today", "🎯", "Filled a limit", "earned everything a daily limit allows today"),
    ("full_set", "🃏", "Full set", "holds at least one of every card in play"),
    ("holds_a_legendary", "🌟", "Legendary", "holds a legendary card"),
    ("single_digit_serial", "🥇", "Single digits", "holds a card numbered under ten"),
    ("collector", "📜", "Collector", "holds ten cards or more"),
];

/// A badge's icon, title and words.
fn badge(key: &str) -> Option<&'static (&'static str, &'static str, &'static str, &'static str)> {
    BADGES.iter().find(|(name, ..)| *name == key)
}

fn achievements(points: i64, cards: &[Held], full_set: bool, rank: usize, maxed: bool) -> Vec<&'static str> {
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
    if cards.iter().any(|c| c.rarity == Rarity::Legendary) {
        out.push("holds_a_legendary");
    }
    if cards.iter().any(|c| c.serial <= 10) {
        out.push("single_digit_serial");
    }
    if cards.len() >= 10 {
        out.push("collector");
    }
    out
}

/// How brightly one egg burns against the best-fed one, 0 to 100.
///
/// Square-rooted on purpose. Straight proportion against a top of three
/// thousand would leave everybody under a hundred points indistinguishable from
/// nothing at all, and the point of the field of eggs is that you can see at a
/// glance who has been playing. Nought points is still cold and dark.
fn heat_of(points: i64, top: i64) -> i64 {
    if points <= 0 || top <= 0 {
        return 0;
    }
    let share = (points as f64 / top as f64).clamp(0.0, 1.0);
    (share.sqrt() * 100.0).round() as i64
}

// --- counting -------------------------------------------------------------------------

/// Counts, ranks and sorts the rows, and totals the four houses.
///
/// House totals are counted from the rows rather than from the ledger's own
/// house column on purpose: a member's points belong to the house they are in
/// NOW, which after the hatch is not the house they earned the first week in.
///
/// `hatched` is when a mod ran `/hatch`, and `now` decides nothing else: the
/// one thing the clock is asked is whether the hatch moment has arrived.
pub fn assemble(rows: Vec<Row>, hatched: Option<i64>, now: i64) -> Live {
    let top = rows.iter().map(|row| row.per_game.iter().map(|(_, n)| n).sum::<i64>()).max().unwrap_or(0);
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
            cards.sort_by_key(|card| card.serial);
            let found = house::house(&row.house);
            Member {
                user: row.user,
                name: row.name,
                avatar: row.avatar,
                house: found.map(|h| h.key),
                slot: found.and_then(|h| HOUSES.iter().position(|one| one.key == h.key)),
                dragon: row.dragon,
                points,
                rank: 0,
                heat: heat_of(points, top),
                per_game,
                per_day: row.per_day,
                today,
                craving: row.craving,
                craving_until: row.craving_until,
                achievements: achievements(points, &cards, row.full_set, 1, maxed),
                cards,
                stage: row.stage,
                egg_from: row.egg_from,
                egg_until: row.egg_until,
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
        .enumerate()
        .map(|(slot, worn)| {
            let mine = members.iter().filter(|m| m.house == Some(worn.key));
            HouseRow {
                key: worn.key,
                slot,
                name: worn.name,
                crest: worn.crest,
                colour: worn.colour,
                points: mine.clone().map(|m| m.points).sum(),
                members: mine.count(),
            }
        })
        .collect();
    houses.sort_by(|a, b| b.points.cmp(&a.points).then(a.name.cmp(&b.name)));

    let hatch_at = month::hatch_at();
    Live {
        month: month::title(),
        on: month::running(),
        hatched: hatched.is_some(),
        // The clock decides, and a hatch nobody has run keeps the eggs on the
        // page rather than showing dragons with no names.
        egg_week: now < hatch_at || hatched.is_none(),
        hatch_at,
        hatched_at: hatched,
        egg_week_days: month::watch_days(),
        craving_hours: month::craving_hours(),
        window_secs: month::window_secs(),
        houses,
        deck: Vec::new(),
        members,
    }
}

// --- the two bodies -------------------------------------------------------------------

/// What both bodies say about the month itself.
fn head(live: &Live, now: i64) -> Value {
    json!({
        "month": live.month,
        "on": live.on,
        "hatched": live.hatched,
        "egg_week": live.egg_week,
        "hatch_at": live.hatch_at,
        "hatched_at": live.hatched_at,
        "egg_week_days": live.egg_week_days,
        "craving_hours": live.craving_hours,
        "answer_window_seconds": live.window_secs,
        "now": now,
        "cache_seconds": cache_secs(),
        "total": live.members.len(),
    })
}

fn deck_json(live: &Live) -> Value {
    live.deck
        .iter()
        .map(|card| {
            json!({
                "name": card.name,
                "slug": card.slug,
                "rarity": card.rarity.key(),
                "rarity_name": card.rarity.name(),
                "worth": card.rarity.points(),
                // The picture's name, not a path: where the page keeps its art
                // is the page's business, and the owner's instruction was that
                // the cards are referenced by name.
                "art": format!("card-{}.png", card.slug),
            })
        })
        .collect()
}

fn houses_json(live: &Live) -> Value {
    live.houses
        .iter()
        .map(|h| {
            json!({
                "key": h.key,
                "slot": h.slot,
                "name": h.name,
                "crest": h.crest,
                "colour": format!("#{:06X}", h.colour),
                "points": h.points,
                "members": h.members,
            })
        })
        .collect()
}

/// The hall and the leaderboard: everybody, and a baker's dozen of small
/// fields each. This is the body a hundred and fifty people load on a phone, so
/// nothing long is in it - no cards, no day-by-day line, no per-game list and
/// no badges, only the handful of totals the hall itself draws.
pub fn hall(live: &Live, now: i64) -> Value {
    let mut body = head(live, now);
    body["houses"] = houses_json(live);
    body["deck"] = deck_json(live);
    body["members"] = live
        .members
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "avatar": m.avatar,
                "house": m.house,
                "slot": m.slot,
                "dragon": m.dragon,
                "points": m.points,
                "rank": m.rank,
                "egg_stage": m.stage,
                "heat": m.heat,
                // Four small numbers rather than the four long lists: enough
                // for the houses table and the quick panel the hall opens when
                // somebody is tapped, and a fifth of the bytes.
                "today_used": m.today.iter().map(|(_, used, _)| used).sum::<i64>(),
                "today_limit": m.today.iter().map(|(_, _, limit)| limit).sum::<i64>(),
                "days_played": m.per_day.iter().filter(|(_, n)| *n > 0).count(),
                "best_day": m.per_day.iter().map(|(_, n)| *n).max().unwrap_or(0),
            })
        })
        .collect();
    body
}

/// One person's whole page, in one call: their own numbers, and the standings
/// beside them so the page needs nothing else to draw itself.
///
/// None when that id has no egg - which, during a themed month, is the honest
/// answer for somebody who never claimed one.
pub fn one(live: &Live, user: u64, now: i64) -> Option<Value> {
    let me = live.members.iter().find(|m| m.user == user)?;
    let mut body = head(live, now);
    body["houses"] = houses_json(live);
    body["deck"] = deck_json(live);
    body["me"] = json!({
        "name": me.name,
        "avatar": me.avatar,
        "house": me.house,
        "slot": me.slot,
        "dragon": me.dragon,
        "points": me.points,
        "rank": me.rank,
        "of": live.members.len(),
        "heat": me.heat,
        "egg_stage": me.stage,
        "egg_stage_label": egg::STAGES.get(me.stage).map(|(_, words)| *words).unwrap_or(""),
        "egg_stage_icon": egg::STAGES.get(me.stage).map(|(icon, _)| *icon).unwrap_or(""),
        // Their OWN week, which for somebody who claimed late is not the
        // month's. The page dates its five egg pictures from these.
        "egg_from": me.egg_from,
        "egg_until": me.egg_until,
        "per_game": me.per_game.iter().map(|(source, n)| json!({
            "game": source.key(),
            "label": source.label(),
            "points": n,
        })).collect::<Vec<_>>(),
        "per_day": me.per_day.iter().map(|(day, n)| json!({"day": day, "points": n})).collect::<Vec<_>>(),
        "today": me.today.iter().map(|(group, used, limit)| json!({
            "group": group.key(),
            "label": group.label(),
            // The games that share the limit, so the page needs no table of its own.
            "games": group.sources().iter().map(|s| s.label()).collect::<Vec<_>>(),
            "used": used,
            "limit": limit,
            "left": (limit - used).max(0),
        })).collect::<Vec<_>>(),
        "craving": me.craving.iter().map(|s| json!({"game": s.key(), "label": s.label()})).collect::<Vec<_>>(),
        "craving_until": me.craving_until,
        "cards": me.cards.iter().map(|card| json!({
            "name": card.name,
            "slug": card.slug,
            "rarity": card.rarity.key(),
            "serial": card.serial,
            "serial_label": frog_store::serial_label(card.serial),
            "day": card.day,
        })).collect::<Vec<_>>(),
        "achievements": me.achievements.iter().filter_map(|key| badge(key)).map(|(key, icon, title, what)| json!({
            "key": key,
            "icon": icon,
            "title": title,
            "what": what,
        })).collect::<Vec<_>>(),
    });
    Some(body)
}

// --- reading it off the stores ---------------------------------------------------------

/// Everything the page needs, read in one pass over the three stores.
///
/// Each store's lock is taken and let go of in turn, never two at once, which
/// is the rule the rest of the panel's public pages follow.
pub fn read_live(panel: &Panel, now: i64) -> Option<Live> {
    // The eggs, their hunger and whether the hatch has been run: one lock.
    //
    // Only members who are actually in the month: an egg, and not opted out.
    // Somebody who has opted out is hidden from the hall entirely - out means
    // out, and that includes a page of their own - and nothing of theirs is
    // deleted by being hidden.
    let (eggs, hatched, mut craving) = {
        let db = egg_store::db()?;
        let conn = db.lock();
        let eggs = egg_store::playing(&conn);
        let hatched = egg_store::hatched_at(&conn);
        let until = egg::slot_ends(egg::slot_of(now));
        let craving: HashMap<u64, (Vec<Source>, i64)> =
            eggs.iter().map(|one| (one.user, (egg::craving_of(&conn, one, now), until))).collect();
        (eggs, hatched, craving)
    };
    if eggs.is_empty() {
        return Some(assemble(Vec::new(), hatched, now));
    }

    // The cards, from the frog store.
    let mut cards: HashMap<u64, Vec<Held>> = HashMap::new();
    let mut deck: Vec<Kind> = Vec::new();
    let mut in_play = 0usize;
    if let Some(db) = frog_store::db() {
        let conn = db.lock();
        let wizards = frog_store::wizards(&conn);
        in_play = wizards.iter().filter(|w| w.enabled).count();
        deck = wizards
            .iter()
            .filter(|w| w.enabled)
            .map(|w| Kind { name: w.name.clone(), slug: w.slug.clone(), rarity: w.rarity })
            .collect();
        deck.sort_by(|a, b| a.rarity.points().cmp(&b.rarity.points()).then(a.name.cmp(&b.name)));
        let slugs: HashMap<i64, String> = wizards.into_iter().map(|w| (w.id, w.slug)).collect();
        let users: Vec<u64> = eggs.iter().map(|e| e.user).collect();
        for card in frog_store::cards_of_users(&conn, &users) {
            cards.entry(card.user_id).or_default().push(Held {
                slug: slugs.get(&card.wizard_id).cloned().unwrap_or_default(),
                name: card.wizard_name,
                rarity: card.rarity,
                serial: card.serial,
                day: day_of_month(card.ts),
            });
        }
    }

    // The points, from the house ledger.
    let since = ledger::month_start(now);
    let mut rows = Vec::with_capacity(eggs.len());
    let db = house::db()?;
    let conn = db.lock();
    for one in &eggs {
        let per_game = ledger::breakdown(&conn, one.user, since).unwrap_or_default();
        let per_day = day_by_day(&conn, one.user, since);
        let today = Group::ALL
            .into_iter()
            .map(|g| (g, g.sources().iter().map(|s| earned_today(&conn, one.user, *s, now)).sum()))
            .collect();
        let theirs = cards.remove(&one.user).unwrap_or_default();
        let kinds: std::collections::HashSet<&String> = theirs.iter().map(|card| &card.name).collect();
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
            egg_from: one.claimed_ts,
            egg_until: one.hatch_ts,
        });
    }
    let mut live = assemble(rows, hatched, now);
    live.deck = deck;
    Some(live)
}

/// The day of the India month a moment falls on, for a card's own page.
fn day_of_month(ts: i64) -> u32 {
    use chrono::Datelike;
    chrono::DateTime::from_timestamp(ts + month::IST_OFFSET, 0).map(|t| t.day()).unwrap_or(1)
}

/// One member's points per India day since `since`, oldest first.
fn day_by_day(conn: &rusqlite::Connection, user: u64, since: i64) -> Vec<(String, i64)> {
    conn.prepare("SELECT day, SUM(points) FROM ledger WHERE user_id = ?1 AND ts >= ?2 GROUP BY day ORDER BY day")
        .and_then(|mut s| {
            s.query_map(rusqlite::params![user as i64, since], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
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

// --- the routes ---------------------------------------------------------------------------

static CACHE: LazyLock<Mutex<Option<(Instant, Live)>>> = LazyLock::new(|| Mutex::new(None));

/// The month, from the cache when it is fresh and from the stores otherwise.
/// Both bodies are cut from one of these, so a person's page and the hall can
/// never disagree about who is first.
fn cached(panel: &Panel, now: i64, at: Instant) -> Option<Live> {
    {
        let held = CACHE.lock();
        if let Some((made, live)) = held.as_ref()
            && at.duration_since(*made) < Duration::from_secs(cache_secs())
        {
            return Some(live.clone());
        }
    }
    let live = read_live(panel, now)?;
    *CACHE.lock() = Some((at, live.clone()));
    Some(live)
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

fn off() -> Response {
    json_out(StatusCode::NOT_FOUND, json!({"error": "The live page is switched off."}))
}

fn unavailable() -> Response {
    json_out(StatusCode::SERVICE_UNAVAILABLE, json!({"error": "The month's numbers aren't available right now."}))
}

async fn hall_json(State(panel): State<Panel>) -> Response {
    if !enabled() {
        return off();
    }
    let now = chrono::Utc::now().timestamp();
    match cached(&panel, now, Instant::now()) {
        // `now` is taken afresh beside the cached numbers, so the page can say
        // how old they are without trusting the clock it is drawn on.
        Some(live) => json_out(StatusCode::OK, hall(&live, now)),
        None => unavailable(),
    }
}

async fn one_json(State(panel): State<Panel>, Path(id): Path<String>) -> Response {
    if !enabled() {
        return off();
    }
    // The address ends in `.json` so the page can be fetched by a plain link;
    // what is in front of it is a Discord id and nothing else.
    let Some(user) = id.strip_suffix(".json").unwrap_or(&id).parse::<u64>().ok() else {
        return json_out(StatusCode::NOT_FOUND, json!({"error": "That isn't a member."}));
    };
    let now = chrono::Utc::now().timestamp();
    let Some(live) = cached(&panel, now, Instant::now()) else { return unavailable() };
    match one(&live, user, now) {
        Some(body) => json_out(StatusCode::OK, body),
        None => json_out(
            StatusCode::NOT_FOUND,
            json!({"error": "Nobody here has that egg. Claim one with /egg in the server.", "egg_week": live.egg_week}),
        ),
    }
}

/// The page itself. The same HTML at every address it is served under: which
/// person it opens on is read from the path by the page, so one cached file
/// serves everybody.
async fn page() -> Response {
    if !enabled() {
        return (StatusCode::NOT_FOUND, "The live page is switched off.").into_response();
    }
    let mut res = Response::new(super::text_body(live_ui::page()));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // The hall's own policy, instead of the panel's: three.js comes from a CDN
    // and the page is one file with its script inside it. The panel's
    // `security_headers` leaves a policy a handler has already set alone.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; \
             script-src 'self' 'unsafe-inline' https://cdn.jsdelivr.net; \
             style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
             font-src 'self' https://fonts.gstatic.com data:; \
             img-src 'self' https://cdn.discordapp.com data: blob:; \
             connect-src 'self' https://cdn.jsdelivr.net; \
             worker-src 'self' blob:; base-uri 'self'; frame-ancestors 'none'; form-action 'none'",
        ),
    );
    res
}

/// One of the page's own files - art, a model, a texture - from
/// `<workspace>/.runtime/live`. None of them are built into the binary: a
/// release is a bad place to keep two megabytes of dragon.
async fn file(Path(path): Path<String>) -> Response {
    if !enabled() {
        return (StatusCode::NOT_FOUND, "The live page is switched off.").into_response();
    }
    match live_ui::asset(&path) {
        Some((kind, bytes)) => {
            let mut res = Response::new(Body::from(bytes));
            let h = res.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_str(kind).unwrap_or(HeaderValue::from_static("application/octet-stream")));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static(ART_CACHE));
            res
        }
        None => (StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

/// The page's own bucket, kept apart from the sudoku and House Cup pages' so a
/// busy hall can't use up a puzzle's allowance or the other way about. Only the
/// page and the two endpoints go through it: the art is asked for twenty files
/// at a time and carries a day's cache header instead.
async fn rate_limit(req: Request, next: Next) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let ip = super::client_ip(req.headers(), peer);
    if !super::sudoku::allow(&format!("live:{}", ip), Instant::now()) {
        return (StatusCode::TOO_MANY_REQUESTS, "Slow down a moment, then reload.").into_response();
    }
    next.run(req).await
}

pub fn routes() -> Router<Panel> {
    let watched = Router::new()
        .route("/live", get(page))
        .route("/live/", get(page))
        .route("/live/u/{id}", get(page))
        .route("/live/api/hall.json", get(hall_json))
        // The address the hall's body was first served under, before the page
        // had two of them. Still the hall: nothing has to be changed anywhere
        // that already reads it.
        .route("/live.json", get(hall_json))
        .route("/live/api/u/{id}", get(one_json))
        .route_layer(middleware::from_fn(rate_limit));
    // The art is not in the bucket, and is not in the binary either.
    watched.route("/live/{*path}", get(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::month::testing::Month;

    /// A moment inside the egg week of the shipped hatch.
    const IN_EGG_WEEK: i64 = 1_791_000_000;
    /// Well after it.
    const AFTER: i64 = 1_793_000_000;

    fn held(name: &str, slug: &str, rarity: Rarity, serial: i64) -> Held {
        Held { name: name.into(), slug: slug.into(), rarity, serial, day: 3 }
    }

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
            egg_from: 1_790_400_000,
            egg_until: 1_791_000_000,
        }
    }

    /// Real-shaped Discord snowflakes, so the privacy test's search for an id in
    /// a body means something: no points total, day or timestamp on the page can
    /// look like one of these.
    const FIRST_ID: u64 = 701234567890123456;

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
                r.user = FIRST_ID + i;
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
        let live = assemble(rows, None, AFTER);
        assert_eq!(
            live.members.iter().map(|m| (m.name.as_str(), m.points, m.rank)).collect::<Vec<_>>(),
            vec![("Bela", 100, 1), ("Ayan", 50, 2), ("Chetan", 50, 2), ("Dia", 0, 4)],
            "level members share a place, and the next place skips"
        );
        assert_eq!(live.members[0].achievements, vec!["top_of_the_server", "hundred_points", "maxed_a_limit_today"]);
        assert!(!live.members[1].achievements.contains(&"top_of_the_server"), "only one person is top");
    }

    #[test]
    fn every_badge_the_page_can_be_sent_has_words_of_its_own() {
        let mut rows = a_server();
        rows[0].cards = (1..=11).map(|n| held("House Stark", "stark", Rarity::Legendary, n)).collect();
        rows[0].full_set = true;
        let live = assemble(rows, Some(1_000), AFTER);
        let body = one(&live, FIRST_ID, AFTER).expect("the top member's page");
        let badges = body["me"]["achievements"].as_array().unwrap();
        assert!(badges.len() >= 6, "the one who has done everything wears everything: {:?}", badges);
        for one in badges {
            for key in ["key", "icon", "title", "what"] {
                assert!(one[key].as_str().is_some_and(|s| !s.is_empty()), "a badge needs `{}`: {}", key, one);
            }
        }
        // And every badge the counter can award is one the page can draw.
        for key in ["top_of_the_server", "full_set", "holds_a_legendary", "single_digit_serial", "collector"] {
            assert!(badge(key).is_some(), "`{}` has no words", key);
        }
    }

    #[test]
    fn an_eggs_heat_is_its_points_against_the_best_fed_one() {
        assert_eq!(heat_of(0, 3000), 0, "an untouched egg sits cold and dark");
        assert_eq!(heat_of(3000, 3000), 100, "and the best fed one burns bright");
        // Straight proportion would make this 2, which draws as nothing.
        assert_eq!(heat_of(100, 3000), 18);
        assert!(heat_of(50, 3000) > 0, "a member who played once can see they played once");
        assert_eq!(heat_of(10, 0), 0, "and nobody's points at all is nobody's glow");

        let live = assemble(a_server(), None, IN_EGG_WEEK);
        assert_eq!(live.members[0].heat, 100);
        assert!(live.members.last().unwrap().heat < live.members[0].heat);
        assert!(live.members.windows(2).all(|w| w[0].heat >= w[1].heat), "more points is never less glow");
    }

    #[test]
    fn the_four_houses_are_totalled_from_the_members_and_sorted() {
        let _month = Month::off();
        let live = assemble(a_server(), Some(1_000), AFTER);
        assert_eq!(live.houses.len(), 4);
        assert!(live.hatched);
        let total: i64 = live.members.iter().map(|m| m.points).sum();
        assert_eq!(live.houses.iter().map(|h| h.points).sum::<i64>(), total, "every point is in a house");
        assert!(live.houses.windows(2).all(|w| w[0].points >= w[1].points), "most points first");
        assert_eq!(live.houses.iter().map(|h| h.members).sum::<usize>(), 53);
        for h in &live.houses {
            assert!(!h.name.is_empty() && !h.crest.is_empty());
        }
        // However the four sort, each one keeps the slot its art is named after.
        let mut slots: Vec<usize> = live.houses.iter().map(|h| h.slot).collect();
        slots.sort();
        assert_eq!(slots, vec![0, 1, 2, 3]);
    }

    #[test]
    fn the_houses_are_named_as_the_month_paints_them() {
        let _month = Month::on();
        let live = assemble(a_server(), Some(1_000), AFTER);
        let names: Vec<&str> = live.houses.iter().map(|h| h.name.as_str()).collect();
        for wanted in ["Stark", "Lannister", "Targaryen", "Night's Watch"] {
            assert!(names.contains(&wanted), "{} is missing from {:?}", wanted, names);
        }
        assert_eq!(live.month, "Fire & Blood");
        assert!(live.on);
    }

    #[test]
    fn the_hall_body_is_the_light_one() {
        let _month = Month::on();
        let mut rows = a_server();
        rows[0].cards = vec![held("The Faceless Man", "faceless", Rarity::Legendary, 187)];
        let live = assemble(rows, Some(1_000), AFTER);
        let body = hall(&live, 1_700_000_000);

        for key in ["month", "on", "hatched", "egg_week", "hatch_at", "now", "cache_seconds", "total", "houses", "members"] {
            assert!(body.get(key).is_some(), "the hall needs `{}`", key);
        }
        assert_eq!(body["total"], 53);
        let member = &body["members"][0];
        for key in [
            "name", "avatar", "house", "slot", "dragon", "points", "rank", "egg_stage", "heat", "today_used",
            "today_limit", "days_played", "best_day",
        ] {
            assert!(member.get(key).is_some(), "a member in the hall needs `{}`", key);
        }
        // The long lists are what makes it heavy, and none of them is here.
        for key in ["per_game", "per_day", "today", "cards", "achievements", "craving"] {
            assert!(member.get(key).is_none(), "`{}` has no business in the hall's body", key);
        }
        // Fifty-three members with a card each, and still small enough for a
        // phone on mobile data. A hundred and fifty is under a hundred
        // kilobytes at this rate.
        let bytes = body.to_string().len();
        assert!(bytes < 30_000, "the hall's body is {} bytes, which is too much to load on a phone", bytes);
    }

    #[test]
    fn one_persons_page_arrives_in_one_call() {
        let _month = Month::on();
        let mut rows = a_server();
        rows[0].cards =
            vec![held("The Faceless Man", "faceless", Rarity::Legendary, 187), held("House Stark", "stark", Rarity::Common, 3)];
        rows[0].full_set = true;
        let live = assemble(rows, Some(1_000), AFTER);
        let body = one(&live, FIRST_ID, 1_700_000_000).expect("the member's own page");

        // The standings come with it, so the page needs no second call.
        assert_eq!(body["houses"].as_array().unwrap().len(), 4);
        let me = &body["me"];
        for key in [
            "name", "avatar", "house", "slot", "dragon", "points", "rank", "of", "heat", "egg_stage",
            "egg_stage_label", "per_game", "per_day", "today", "craving", "craving_until", "cards", "achievements",
        ] {
            assert!(me.get(key).is_some(), "a person's page needs `{}`", key);
        }
        assert_eq!(me["rank"], 1);
        assert_eq!(me["of"], 53);
        // The cards carry their serials, lowest first, and the slug the art is
        // named after.
        let cards = me["cards"].as_array().unwrap();
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0]["serial"], 3);
        assert_eq!(cards[0]["slug"], "stark");
        assert_eq!(cards[1]["serial_label"], "No. 0187");
        assert_eq!(cards[1]["rarity"], "legendary");
        assert_eq!(cards[1]["day"], 3, "and the day it was caught, for the card's own page");
        // Per-game, per-day and today all carry their labels and the games that
        // share each limit, so the page needs no table of its own.
        assert_eq!(me["per_game"][0]["game"], "anagram");
        assert!(me["per_day"][0]["day"].is_string());
        let quick = me["today"].as_array().unwrap().iter().find(|t| t["group"] == "quick").unwrap();
        assert_eq!((quick["used"].as_i64(), quick["limit"].as_i64(), quick["left"].as_i64()), (Some(20), Some(20), Some(0)));
        assert!(quick["games"].as_array().unwrap().len() >= 3, "the page is told what shares the limit");
        assert_eq!(me["craving"][0]["game"], "anagram");
        assert!(me["craving"][0]["label"].as_str().is_some_and(|l| !l.is_empty()));
        assert!(me["achievements"].as_array().unwrap().iter().any(|a| a["key"] == "full_set"));

        // Somebody with no egg is not on the page at all.
        assert!(one(&live, 999_999_999_999_999_999, 1_700_000_000).is_none());
    }

    /// The one that has to keep passing: nothing private may ever be on the
    /// page, however the code around it changes.
    #[test]
    fn the_page_gives_away_nothing_private() {
        let _month = Month::on();
        let mut rows = a_server();
        rows[0].cards = vec![held("House Stark", "stark", Rarity::Common, 1)];
        let live = assemble(rows, Some(1_000), AFTER);
        let bodies = [hall(&live, 1_700_000_000), one(&live, FIRST_ID, 1_700_000_000).expect("a page")];

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

        for body in &bodies {
            let text = body.to_string();
            let mut found = Vec::new();
            keys(body, &mut found);
            for key in &found {
                let lower = key.to_ascii_lowercase();
                for banned in [
                    "id", "user", "discord", "email", "mention", "token", "ip", "message", "content", "text",
                    "said", "join", "voice", "channel", "chat", "note", "dm", "hash", "session", "secret",
                ] {
                    assert!(lower != banned, "`{}` has no business on a public page", key);
                    assert!(!lower.ends_with(&format!("_{}", banned)), "`{}` has no business on a public page", key);
                }
            }
            // No Discord snowflake is anywhere in the body at all. Every member
            // in the fixture has one, and not one of them may appear - the id in
            // the ADDRESS of a person's own page is never echoed into it.
            for user in FIRST_ID..FIRST_ID + 53 {
                assert!(!text.contains(&user.to_string()), "member {}'s id reached the page", user);
            }
            // And the only url on it is Discord's own public avatar CDN.
            let faces = body["members"].as_array().cloned().unwrap_or_else(|| vec![body["me"].clone()]);
            for member in faces {
                let avatar = member["avatar"].as_str().unwrap_or("");
                assert!(
                    avatar.is_empty() || avatar.starts_with("https://cdn.discordapp.com/"),
                    "an avatar came from somewhere unexpected: {}",
                    avatar
                );
            }
        }
    }

    #[test]
    fn an_empty_server_is_still_a_page() {
        let _month = Month::on();
        let live = assemble(Vec::new(), None, IN_EGG_WEEK);
        assert!(live.members.is_empty());
        assert_eq!(live.houses.len(), 4, "the four houses are always there, at nought");
        assert!(live.houses.iter().all(|h| h.points == 0 && h.members == 0));
        assert!(!live.hatched);
        let body = hall(&live, 0);
        assert_eq!(body["members"], json!([]));
        assert_eq!(body["total"], 0);
    }

    #[test]
    fn the_clock_decides_whether_it_is_egg_week_not_a_toggle() {
        let mut month = Month::on();
        month.set("VIZIER_MONTH_HATCH", "2026-10-08 12:00");
        let hatch = month::hatch_at();

        // Before the moment: eggs, hatch run or not.
        assert!(assemble(a_server(), None, hatch - 1).egg_week);
        assert!(assemble(a_server(), Some(hatch), hatch - 1).egg_week);
        // After it, with the hatch run: dragons.
        assert!(!assemble(a_server(), Some(hatch), hatch + 1).egg_week);
        // After it, with nobody having run /hatch: still eggs, because dragons
        // with no names and no houses are worse than eggs.
        assert!(assemble(a_server(), None, hatch + 86_400).egg_week);

        // And the page is told the moment and the length of the week, so its
        // five egg pictures can be dated without a rebuild.
        let live = assemble(a_server(), None, hatch - 1);
        assert_eq!(live.hatch_at, hatch);
        assert_eq!(live.egg_week_days, 7);
        let body = hall(&live, hatch - 1);
        assert_eq!(body["egg_week"], true);
        assert_eq!(body["hatch_at"], hatch);
        assert_eq!(body["egg_week_days"], 7);
        assert_eq!(body["hatched_at"], Value::Null);
    }

    #[test]
    fn before_the_hatch_nobody_has_a_house_or_a_dragon_and_the_eggs_have_stages() {
        let _month = Month::on();
        let rows: Vec<Row> =
            a_server().into_iter().map(|r| Row { house: String::new(), dragon: String::new(), ..r }).collect();
        let live = assemble(rows, None, IN_EGG_WEEK);
        assert!(live.egg_week);
        assert!(live.members.iter().all(|m| m.house.is_none() && m.slot.is_none() && m.dragon.is_empty()));
        assert!(live.houses.iter().all(|h| h.members == 0 && h.points == 0), "the standings wait for the hatch");
        let body = hall(&live, 0);
        assert_eq!(body["members"][0]["house"], Value::Null);
        assert_eq!(body["members"][0]["slot"], Value::Null);
        assert_eq!(body["members"][0]["egg_stage"], 2);
        // And every egg on the floor is told what it has been fed.
        assert!(body["members"].as_array().unwrap().iter().all(|m| m["heat"].is_i64()));
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

    /// The page greys the cards nobody has caught, so it is told the whole deck
    /// - once, beside the members, rather than once per member.
    #[test]
    fn the_whole_deck_rides_on_both_bodies() {
        let _month = Month::on();
        let mut live = assemble(a_server(), Some(1_000), AFTER);
        live.deck = vec![
            Kind { name: "House Stark".into(), slug: "stark".into(), rarity: Rarity::Common },
            Kind { name: "The Faceless Man".into(), slug: "faceless".into(), rarity: Rarity::Legendary },
        ];
        for body in [hall(&live, AFTER), one(&live, FIRST_ID, AFTER).expect("a page")] {
            let deck = body["deck"].as_array().expect("the deck").clone();
            assert_eq!(deck.len(), 2);
            for key in ["name", "slug", "rarity", "rarity_name", "worth"] {
                assert!(deck[0].get(key).is_some(), "a card in play needs `{}`", key);
            }
            assert_eq!(deck[1]["slug"], "faceless");
            assert_eq!(deck[1]["art"], "card-faceless.png", "the page is told the picture's name, not left to build it");
            assert_eq!(deck[1]["worth"], 25, "what catching one pays, as the settings have it");
        }
    }

    #[test]
    fn a_members_own_egg_week_is_theirs_and_not_the_months() {
        let _month = Month::on();
        let mut rows = a_server();
        // Somebody who claimed late is on a watch of their own.
        rows[0].egg_from = 1_792_000_000;
        rows[0].egg_until = 1_792_604_800;
        let live = assemble(rows, None, IN_EGG_WEEK);
        let me = one(&live, FIRST_ID, IN_EGG_WEEK).expect("their page");
        assert_eq!(me["me"]["egg_from"], 1_792_000_000, "their own claim, not the month's start");
        assert_eq!(me["me"]["egg_until"], 1_792_604_800);
        // The hall's body carries the shared week, which is all it draws.
        assert_eq!(hall(&live, IN_EGG_WEEK)["egg_week_days"], 7);
    }

    #[test]
    fn the_page_can_be_switched_off() {
        let mut month = Month::on();
        assert!(enabled(), "on by default");
        month.set("VIZIER_LIVE", "off");
        assert!(!enabled());
    }

    // --- through the router ------------------------------------------------------
    //
    // The public corner, as the web sees it: no session, no cookie, no
    // `X-Panel` header, nothing under `/api`.

    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    /// One egg store for the whole test binary, with two eggs in it.
    static EGGS: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    /// Somebody who has claimed an egg, and somebody who has not.
    const CLAIMED: u64 = 7_010_000_000_000_000_01;
    const STRANGER: u64 = 7_010_000_000_000_000_02;

    fn with_eggs() -> Router {
        let app = super::super::tests::panel();
        EGGS.get_or_init(|| {
            let dir = tempfile::tempdir().expect("temp dir");
            egg_store::open_at(&dir.path().join("eggs.db")).expect("egg store");
            if let Some(db) = egg_store::db() {
                let conn = db.lock();
                let _ = egg_store::claim(&conn, CLAIMED, "Ayan", 1_791_526_800, 1_790_900_000);
            }
            dir
        });
        super::super::sudoku::forget_all();
        forget();
        app
    }

    async fn get(app: &Router, path: &str) -> (StatusCode, String, axum::http::HeaderMap) {
        let req = Request::builder().method("GET").uri(path).body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let headers = res.headers().clone();
        let bytes = axum::body::to_bytes(res.into_body(), 20 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string(), headers)
    }

    #[tokio::test]
    async fn the_page_is_open_to_anyone_at_both_its_addresses() {
        let _month = Month::on();
        let app = with_eggs();
        for path in ["/live", "/live/", &format!("/live/u/{}", CLAIMED)] {
            let (status, html, headers) = get(&app, path).await;
            assert_eq!(status, StatusCode::OK, "{} needs no sign-in: {}", path, html);
            assert_eq!(headers["content-type"], "text/html; charset=utf-8");
            assert!(html.contains("The Hall of Dragons"), "{} is the hall", path);
            // Its own policy, not the panel's: three.js comes from a CDN.
            let csp = headers["content-security-policy"].to_str().unwrap();
            assert!(csp.contains("https://cdn.jsdelivr.net"), "the page's own policy is the one that reached the web: {}", csp);
            assert!(csp.contains("frame-ancestors 'none'"), "and it is still not embeddable: {}", csp);
        }
        // One file, whatever address it came from: the page reads the id out of
        // its own address rather than having it written into it.
        let (_, hall, _) = get(&app, "/live").await;
        let (_, mine, _) = get(&app, &format!("/live/u/{}", CLAIMED)).await;
        assert_eq!(hall, mine);
        assert!(!mine.contains(&CLAIMED.to_string()), "the id in the address is never written into the page");
    }

    #[tokio::test]
    async fn the_shipped_page_has_no_invented_people_and_no_preview_controls() {
        let page = live_ui::INDEX_HTML;
        // The sample's cast, and the sample's way of inventing their numbers.
        for gone in ["Ninja Billori", "Kaju_Katli", "Vermithor", "const NAMES = [", "const DRAGONS = [", "stands in for"] {
            assert!(!page.contains(gone), "`{}` is sample data and must not ship", gone);
        }
        // The week toggle and the day stepper were preview controls.
        for gone in ["data-week", "id=\"week\"", "id=\"egg-days\"", "previewDay", "data-day="] {
            assert!(!page.contains(gone), "`{}` was a preview control and must not ship", gone);
        }
        // And it reads the real thing instead.
        assert!(page.contains("api/hall.json"), "the page reads the hall");
        assert!(page.contains("u/${MY_ID}.json"), "and a person's own page");
    }

    #[tokio::test]
    async fn the_hall_and_a_persons_page_are_public_json() {
        let _month = Month::on();
        let app = with_eggs();

        let (status, body, headers) = get(&app, "/live/api/hall.json").await;
        assert_eq!(status, StatusCode::OK, "{}", body);
        assert_eq!(headers["content-type"], "application/json");
        let hall: Value = serde_json::from_str(&body).expect("json");
        assert_eq!(hall["houses"].as_array().map(|h| h.len()), Some(4));
        let names: Vec<&str> = hall["members"].as_array().unwrap().iter().filter_map(|m| m["name"].as_str()).collect();
        assert!(names.contains(&"Ayan"), "the member whose egg was claimed is on it: {:?}", names);
        assert!(!body.contains(&CLAIMED.to_string()), "no id reached the hall");

        // The first address the hall was served under still answers.
        let (status, first, _) = get(&app, "/live.json").await;
        assert_eq!(status, StatusCode::OK);
        assert!(first.contains("\"members\""), "{}", first);

        let (status, body, _) = get(&app, &format!("/live/api/u/{}.json", CLAIMED)).await;
        assert_eq!(status, StatusCode::OK, "{}", body);
        let mine: Value = serde_json::from_str(&body).expect("json");
        assert_eq!(mine["me"]["name"], "Ayan");
        assert_eq!(mine["houses"].as_array().map(|h| h.len()), Some(4), "the standings ride along, so one call fills the page");
        assert!(!body.contains(&CLAIMED.to_string()), "nor into their own page");

        // Somebody with no egg, and something that is not a member at all.
        let (status, _, _) = get(&app, &format!("/live/api/u/{}.json", STRANGER)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        for bad in ["/live/api/u/nobody.json", "/live/api/u/0x1.json", "/live/api/u/.json"] {
            let (status, _, _) = get(&app, bad).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{} is not a member", bad);
        }
    }

    #[tokio::test]
    async fn the_pages_art_is_not_in_the_binary_and_cannot_be_walked_out_of() {
        let _month = Month::on();
        let app = with_eggs();
        // No runtime directory in a test, so there is no art - and that is a
        // plain Not Found rather than an error.
        for path in ["/live/eggs/egg-1.png", "/live/models-v5.js", "/live/dragon.glb"] {
            let (status, _, _) = get(&app, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{}", path);
        }
        // And nothing that is not one of the page's own files is ever opened.
        for path in ["/live/control.db", "/live/../../etc/passwd", "/live/eggs/../../control.db", "/live/.env"] {
            let (status, _, _) = get(&app, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{} must not be served", path);
        }
    }

    #[tokio::test]
    async fn switched_off_every_address_is_simply_not_there() {
        let mut month = Month::on();
        let app = with_eggs();
        month.set("VIZIER_LIVE", "off");
        for path in [
            "/live",
            "/live/u/1",
            "/live/api/hall.json",
            "/live.json",
            "/live/api/u/1.json",
            "/live/eggs/egg-1.png",
        ] {
            let (status, _, _) = get(&app, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{} is off", path);
        }
    }

    /// The hall sets a policy of its own because three.js comes from a CDN. That
    /// must never reach anything else: the panel serves member names, message
    /// logs and the confession queue behind the strict one.
    #[tokio::test]
    async fn the_halls_looser_policy_reaches_the_hall_and_nothing_else() {
        let _month = Month::on();
        let app = with_eggs();
        let (_, _, hall) = get(&app, "/live").await;
        let loose = hall["content-security-policy"].to_str().unwrap();
        assert!(loose.contains("https://cdn.jsdelivr.net"));

        // Every other page on the server, public corner or panel, keeps the
        // strict policy - which allows no script from anywhere but itself.
        for path in ["/", "/login", "/assets/app.js", "/assets/app.css", "/housecup", "/live.json", "/live/api/hall.json"] {
            let (_, _, headers) = get(&app, path).await;
            let csp = headers["content-security-policy"].to_str().unwrap_or("");
            assert!(!csp.contains("cdn.jsdelivr.net"), "{} must not have got the hall's policy: {}", path, csp);
            assert!(csp.contains("default-src 'self'"), "{} lost the strict policy: {}", path, csp);
            assert!(csp.contains("base-uri 'none'"), "{} lost the strict policy: {}", path, csp);
        }
    }

    #[tokio::test]
    async fn the_rest_of_the_panel_still_wants_a_session() {
        let _month = Month::on();
        let app = with_eggs();
        // The live corner opens no door into the panel: it is outside /api, and
        // there is no live endpoint in there to find.
        for path in ["/api/live", "/api/live.json", "/api/live/api/hall.json"] {
            let (status, _, _) = get(&app, path).await;
            assert_ne!(status, StatusCode::OK, "{} must not exist", path);
        }
    }
}
