//! /fight and /battle - the lists.
//!
//! Two shapes, one arena. **A duel** (`/fight @someone`) works in any channel:
//! the challenge card goes up in the fight channel and the caller's channel
//! gets a link to it. Nobody but the two of them is tagged. **A melee**
//! (`/battle`) is for admins, opens a lobby for as many minutes as they ask
//! for, calls the server games role to the lists, and then knocks the joiners
//! out in pairs until one is left. Winners are a coin toss - this is banter,
//! not a ladder - and the last one standing wears the champion role until the
//! next melee.
//!
//! The arena has **one world and no styles**: Westeros, spoken in the server's
//! own Hinglish (see [`super::battle_lines`]). There used to be a `type` option
//! that picked between seven of them; it is gone, along with every branch that
//! read it.
//!
//! It has **two seasons**, which is not the same thing. Before the hatch nobody
//! has a house, so the cards show every fighter as an unhatched dragon egg and
//! name no house anywhere; after it they wear their house's banner and mark.
//! [`hatched`] is the one place that decides which.
//!
//! Winning earns a card from the month's deck as well as the points - see
//! [`super::battle_prize`].
//!
//! Fights live in memory; only results go to battle.db, so a restart loses an
//! open lobby but never the record of who won. The daily melee's slot is
//! remembered, so a restart neither re-opens a melee that already ran nor
//! forgets one that was due while the bot was down.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, CreateModal,
    EditAttachments, EditMessage, EditRole, GuildId, InputTextStyle, Member, Message, MessageId, RoleId, UserId,
};

use super::battle_card::{self, Champion, Fight, Fighter, HouseLook, Outcome, Season, EGG_STAGES};
use super::battle_bracket::{self, Bracket, Entrant, Slot, round_title};
use super::battle_lines::{Lines, lines};
use super::battle_scroll;

/// Fight channel, from `VIZIER_FIGHT_CHANNEL`; otherwise found by name.
const CHANNEL_NAME: &str = "fight-fight-fight";
/// Called to the lists when a melee opens: the people who opted into the
/// month, and the only role the arena ever mentions.
/// `VIZIER_ARENA_GAMES_ROLE`, or this role if it is not set; "0" or "none"
/// turns the tag off and the lobby simply goes up untagged.
const GAMES_ROLE: u64 = 1_554_720_685_514_035_241;
/// Worn by the last battle's winner.
const CHAMPION_ROLE: &str = "Battle Champion";
/// Health both fighters start a fight with.
const START_HP: i32 = 100;
/// Exchanges before the fight is called on health left, so nobody waits forever.
/// Blows land on either side at random and a fifth of turns heal, so a fight
/// usually runs a dozen or so turns.
const MAX_EXCHANGES: usize = 20;
/// Between exchanges of one fight. Short, because a battle is many fights.
const BEAT: Duration = Duration::from_secs(2);
/// Messages under the fight before it is moved back to the bottom of the channel.
const STICKY_AFTER: u32 = 4;
/// How often an open lobby checks whether chat has buried it.
const LOBBY_TICK: Duration = Duration::from_secs(3);
/// Blocks in a health bar.
const BAR_BLOCKS: usize = 14;
/// How long any one Discord call may take before the fight gives up on it and
/// carries on. Without this a wedged upload freezes the whole battle.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Between two fights of the same round.
const FIGHT_GAP: Duration = Duration::from_secs(3);
/// Between rounds.
const ROUND_GAP: Duration = Duration::from_secs(6);
/// A challenge nobody answers expires, `VIZIER_FIGHT_EXPIRY_SECS`.
const CHALLENGE_WAIT: u64 = 120;
/// Between two `/fight`s by the same member, `VIZIER_FIGHT_COOLDOWN_SECS`.
const FIGHT_COOLDOWN: u64 = 60;
/// Lobby length an admin may ask for: at least a minute, at most
/// `VIZIER_BATTLE_LOBBY_MAX_MINUTES`, `VIZIER_BATTLE_LOBBY_MINUTES` if not said.
pub const MIN_WAIT: i64 = 1;
const MAX_WAIT: u64 = 15;
const DEFAULT_WAIT: u64 = 5;
/// Fewer joiners than this and the battle is called off, `VIZIER_BATTLE_MIN_PLAYERS`.
const MIN_PLAYERS: usize = 4;
/// Discord takes a while over each card, so keep a battle under a few minutes.
/// Rounds with more matches than this are quick rounds: every match decided at
/// once and posted as a list. From the quarter-finals on, fights play out -
/// `VIZIER_BATTLE_FULL_FIGHTS_FROM` moves that to the semi-finals or the round of 16.
const FULL_FIGHTS_UP_TO: usize = 4;
/// The bracket picture covers the draw from the round with this many matches
/// (the round of 16); anything bigger would be unreadable.
const CHART_FROM: usize = 8;
/// Names shown in the lobby before it says how many more joined, `VIZIER_BATTLE_LOBBY_NAMES`.
const LOBBY_NAMES: u64 = 40;

fn challenge_wait() -> Duration {
    Duration::from_secs(super::control::number("VIZIER_FIGHT_EXPIRY_SECS", CHALLENGE_WAIT).max(10))
}

fn fight_cooldown() -> Duration {
    Duration::from_secs(super::control::number("VIZIER_FIGHT_COOLDOWN_SECS", FIGHT_COOLDOWN))
}

pub fn max_lobby_minutes() -> i64 {
    super::control::number("VIZIER_BATTLE_LOBBY_MAX_MINUTES", MAX_WAIT).clamp(1, 60) as i64
}

pub fn default_lobby_minutes() -> i64 {
    (super::control::number("VIZIER_BATTLE_LOBBY_MINUTES", DEFAULT_WAIT) as i64).clamp(MIN_WAIT, max_lobby_minutes())
}

fn min_players() -> usize {
    (super::control::number("VIZIER_BATTLE_MIN_PLAYERS", MIN_PLAYERS as u64) as usize).max(2)
}

/// The biggest round whose fights are played out in full.
fn full_fights_up_to() -> usize {
    match super::control::var("VIZIER_BATTLE_FULL_FIGHTS_FROM").as_deref() {
        Some("semi") => 2,
        Some("r16") => 8,
        _ => FULL_FIGHTS_UP_TO,
    }
}

fn lobby_names() -> usize {
    super::control::number("VIZIER_BATTLE_LOBBY_NAMES", LOBBY_NAMES) as usize
}

/// "2 minutes", "1 minute", "90 seconds".
fn span(secs: u64) -> String {
    match secs {
        60 => "1 minute".to_string(),
        s if s % 60 == 0 => format!("{} minutes", s / 60),
        s => format!("{} seconds", s),
    }
}

/// The role the melee calls to the lists, or `None` for no tag at all.
fn games_role_id() -> Option<u64> {
    games_role_from(super::control::var("VIZIER_ARENA_GAMES_ROLE").as_deref())
}

/// What a stored setting means. Split out so it can be read without a panel:
/// blank is "not said" and the month's own role, "0" or "none" switches the tag
/// off, and anything that isn't a role id falls back rather than tagging the
/// wrong thing.
fn games_role_from(raw: Option<&str>) -> Option<u64> {
    match raw.map(str::trim) {
        None => Some(GAMES_ROLE),
        Some(raw) if raw.is_empty() => Some(GAMES_ROLE),
        Some(raw) if raw == "0" || raw.eq_ignore_ascii_case("none") => None,
        Some(raw) => raw.parse().ok().or(Some(GAMES_ROLE)),
    }
}

/// Whether the eggs have hatched. Before the hatch nobody has a house, so every
/// card the arena draws shows eggs and names no house; after it they wear their
/// houses. The month owns the moment - [`super::month::hatch_at`] - so the
/// arena reads the same clock everything else in the month reads.
pub(super) fn hatched() -> bool {
    hatched_at(Utc::now().timestamp())
}

/// The same question at a given moment, so a test can hold time still.
fn hatched_at(now: i64) -> bool {
    let at = super::month::hatch_at();
    at > 0 && now >= at
}

/// Which season the cards are drawn in.
fn season() -> Season {
    if hatched() { Season::Houses } else { Season::Eggs }
}

/// How a house is shown this month: the name, crest and colour come from the
/// month's own paint, so a server that renames the four renames them here too.
/// The banner's trim is the same colour lifted, because a banner needs two and
/// the month only names one - one source, two shades of it.
fn house_look(house: &'static super::house::House) -> HouseLook {
    let themed = super::month::themed(house);
    let colour = themed.colour;
    let field = [(colour >> 16) as u8, (colour >> 8) as u8, colour as u8];
    HouseLook {
        key: house.key,
        initial: themed.name.chars().find(|c| c.is_alphabetic()).map(|c| c.to_uppercase().to_string()).unwrap_or_default(),
        name: themed.name,
        crest: themed.crest,
        colours: (field, battle_card::lift(field, 0.58)),
    }
}

/// How far along a member's egg is, 1..=[`EGG_STAGES`], by the points they have
/// towards the month. Cold at nothing, splitting open once they have really
/// played: the thresholds are wide, so an egg warms over a week rather than in
/// an afternoon.
fn egg_stage(points: i64) -> u8 {
    match points {
        p if p <= 0 => 1,
        p if p < 10 => 2,
        p if p < 30 => 3,
        p if p < 70 => 4,
        _ => EGG_STAGES,
    }
}

/// Everything a member has earned towards the month, read from the house
/// ledger. Read-only, and 0 when the store is not open.
fn points_of(user: u64) -> i64 {
    let Some(db) = super::house::db() else {
        return 0;
    };
    db.lock()
        .query_row("SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1", params![user as i64], |r| r.get(0))
        .unwrap_or(0)
}

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
/// Open lobbies, by the lobby message id.
static LOBBIES: LazyLock<Mutex<HashMap<u64, Lobby>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Open challenges, by the challenge message id.
static CHALLENGES: LazyLock<Mutex<HashMap<u64, Challenge>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Channels with a battle or fight running, so two never overlap.
static BUSY: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Last `/fight` per member, for the cooldown.
static LAST_FIGHT: LazyLock<Mutex<HashMap<u64, std::time::Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
/// Messages posted under the live fight, per channel, for the sticky move.
static BELOW: LazyLock<Mutex<HashMap<u64, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct Lobby {
    joined: Vec<u64>,
    names: HashMap<u64, String>,
    open: bool,
    /// Kept so a join can redraw the lobby with its own countdown.
    ends: i64,
    minutes: i64,
}

struct Challenge {
    from: u64,
    to: u64,
    accepted: Option<bool>,
}

/// Somebody in the lists, with everything the card needs to draw them.
#[derive(Clone)]
struct Contender {
    id: u64,
    name: String,
    avatar: Option<Vec<u8>>,
    /// Their house as the month paints it. `None` for anyone unsorted or
    /// stepped out, who stays neutral rather than being handed one.
    house: Option<HouseLook>,
    /// How far along their egg is, for the cards drawn before the hatch.
    stage: u8,
    /// Where their picture is, for fetching it later.
    face: String,
}

impl Contender {
    fn card(&self, hp: i32) -> Fighter {
        Fighter {
            name: self.name.clone(),
            avatar: self.avatar.clone(),
            hp: hp.max(0) as u32,
            max_hp: START_HP as u32,
            house: self.house.clone(),
            stage: self.stage,
        }
    }
}

/// A tiny xorshift keeps rolls spread without dragging rand into here.
fn roll(seed: &mut u64, n: u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed % n.max(1)
}

fn pick<'a>(pool: &'a [&'a str], seed: &mut u64) -> &'a str {
    pool[roll(seed, pool.len() as u64) as usize]
}

fn fill(line: &str, a: &str, b: &str) -> String {
    line.replace("{a}", a).replace("{b}", b).replace("{w}", a).replace("{l}", b)
}

/// What a turn turned out to be.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Blow {
    Miss,
    Crit,
    Heal,
    Hit,
    Sip,
    Backfire,
    Drain,
    Double,
    Chaos,
    Snack,
    Blessing,
    Crowd,
}

impl Blow {
    fn lines(self, l: &'static Lines) -> &'static [&'static str] {
        match self {
            Blow::Miss => l.miss,
            Blow::Crit => l.crit,
            Blow::Heal => l.heal,
            Blow::Hit => l.exchange,
            Blow::Sip => l.sip,
            Blow::Backfire => l.backfire,
            Blow::Drain => l.drain,
            Blow::Double => l.double,
            Blow::Chaos => l.chaos,
            Blow::Snack => l.snack,
            Blow::Blessing => l.blessing,
            Blow::Crowd => l.crowd,
        }
    }
}

/// One turn: what happened, and what it did to each side's health.
struct Swing {
    blow: Blow,
    /// Change to health, by side: negative hurts, positive heals.
    hits: [i32; 2],
}

impl Swing {
    /// The numbers that go after the line, e.g. "-24 HP" or "-18 HP · +7 HP".
    fn tail(&self) -> String {
        let parts: Vec<String> = self
            .hits
            .iter()
            .filter(|change| **change != 0)
            .map(|change| format!("{}{} HP", if *change > 0 { "+" } else { "" }, change))
            .collect();
        if parts.is_empty() { "no damage".to_string() } else { parts.join(" · ") }
    }
}

/// A turn of the fight. Plain trades still carry it, but roughly a fifth of
/// turns give health back, so a fight lasts a dozen or so turns and can swing
/// late instead of being a straight slide to zero.
fn swing(seed: &mut u64, attacker: usize) -> Swing {
    let other = 1 - attacker;
    let mut hits = [0i32; 2];
    let blow = match roll(seed, 100) {
        0..=4 => Blow::Miss,
        5..=8 => {
            hits[attacker] = 1 + roll(seed, 3) as i32;
            Blow::Sip
        }
        9..=13 => {
            hits[attacker] = -(10 + roll(seed, 9) as i32);
            Blow::Backfire
        }
        14..=20 => {
            hits[other] = -(16 + roll(seed, 9) as i32);
            hits[attacker] = 5 + roll(seed, 5) as i32;
            Blow::Drain
        }
        21..=26 => {
            hits[other] = -(28 + roll(seed, 11) as i32);
            Blow::Double
        }
        27..=32 => {
            hits[other] = -(32 + roll(seed, 11) as i32);
            Blow::Crit
        }
        33..=36 => {
            let both = 8 + roll(seed, 7) as i32;
            hits = [-both, -both];
            Blow::Chaos
        }
        37..=41 => {
            hits[attacker] = 6 + roll(seed, 7) as i32;
            Blow::Heal
        }
        42..=47 => {
            hits[attacker] = 10 + roll(seed, 9) as i32;
            Blow::Snack
        }
        48..=50 => {
            hits[attacker] = 20 + roll(seed, 11) as i32;
            Blow::Blessing
        }
        51..=53 => {
            let both = 4 + roll(seed, 5) as i32;
            hits = [both, both];
            Blow::Crowd
        }
        _ => {
            hits[other] = -(18 + roll(seed, 11) as i32);
            Blow::Hit
        }
    };
    Swing { blow, hits }
}

// --- store ------------------------------------------------------------------

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("battle.db"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS wins (
             id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, kind TEXT NOT NULL,
             beat INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS wins_user ON wins (user_id, kind);
         CREATE TABLE IF NOT EXISTS results (
             id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, winner INTEGER NOT NULL,
             loser INTEGER, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS results_winner ON results (winner);
         CREATE INDEX IF NOT EXISTS results_loser ON results (loser);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    // The scrolls a duel is fought over, written down as they are shown.
    conn.execute_batch(battle_scroll::SCHEMA)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

/// Runs `f` against battle.db, or nothing at all when it is not open - a fight
/// has to carry on without its record rather than stop.
fn with_db<T>(f: impl FnOnce(&Connection) -> T) -> Option<T> {
    DB.get().map(|db| f(&db.lock()))
}

fn meta_get(key: &str) -> Option<String> {
    let db = DB.get()?;
    let conn = db.lock();
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }
}

/// Logs one finished fight. `loser` is `None` for a melee championship, where
/// the whole field lost rather than one person. Points are paid separately:
/// a duel through [`pay_duel`], a melee through [`award_royale`].
fn record(kind: &str, winner: u64, loser: Option<u64>) {
    let now = Utc::now().timestamp();
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO results (kind, winner, loser, ts) VALUES (?1, ?2, ?3, ?4)",
            params![kind, winner as i64, loser.map(|u| u as i64), now],
        );
    }
}

/// Only the first two duels between the same two people each day pay anything.
/// The third is still fought and still recorded - it simply earns nothing, so
/// two friends cannot farm the Cup between them.
const PAID_DUELS_A_DAY: i64 = 2;

/// What a duel's win paid, and why, so the result can say so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Paid {
    Points(i64),
    /// This pair has already had their two paying duels today.
    Enough,
    /// The setting is at zero, so a duel pays nothing to anybody.
    Nothing,
}

impl Paid {
    /// What to add after "won the duel", or nothing at all.
    fn said(self) -> String {
        match self {
            Paid::Points(n) => format!(" (+{} house points)", n),
            Paid::Enough => " — no points, you two have fought enough today".to_string(),
            Paid::Nothing => String::new(),
        }
    }
}

/// The unix second India's day containing `now` began at.
fn ist_midnight(now: i64) -> i64 {
    let offset = super::stats::ist().local_minus_utc() as i64;
    (now + offset).div_euclid(86_400) * 86_400 - offset
}

/// How many duels this pair has already had today, the one just recorded
/// included. 0 when the store is not open, which pays as a first duel.
fn duels_today(low: u64, high: u64, now: i64) -> i64 {
    with_db(|conn| {
        conn.query_row(
            "SELECT COUNT(*) FROM results WHERE kind = 'fight' AND ts >= ?1
               AND ((winner = ?2 AND loser = ?3) OR (winner = ?3 AND loser = ?2))",
            params![ist_midnight(now), low as i64, high as i64],
            |r| r.get(0),
        )
        .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// House points for a duel: three to the winner, nothing whatsoever to the
/// loser - not for turning up, and not for reading a scroll. The dedupe key is
/// the PAIR, the day and which duel of the day it was, so a replayed message
/// cannot pay twice and the third duel of the day cannot pay at all.
fn pay_duel(winner: u64, loser: u64, now: i64) -> Paid {
    let points = super::control::number("VIZIER_POINTS_ARENA_WIN", 3) as i64;
    if points <= 0 {
        return Paid::Nothing;
    }
    let (low, high) = if winner < loser { (winner, loser) } else { (loser, winner) };
    let so_far = duels_today(low, high, now).max(1);
    if so_far > PAID_DUELS_A_DAY {
        return Paid::Enough;
    }
    let key = format!("arena:{}:{}:{}:{}", super::points::ist_day(now), low, high, so_far);
    super::house::award_person(winner, super::points::Source::Arena, points, "won a duel", None, Some(key), None);
    Paid::Points(points)
}

/// House points for a battle royale: 8 to the champion, 3 to the runner-up.
/// Returns the battle's id, the moment it was paid.
fn award_royale(champion: u64, runner_up: Option<u64>) -> i64 {
    let battle = Utc::now().timestamp();
    let give = |user: u64, amount: i64, reason: &str| {
        if amount <= 0 {
            return;
        }
        let key = format!("royale:{}:{}", battle, user);
        super::house::award_person(user, super::points::Source::Royale, amount, reason, None, Some(key), None);
    };
    give(champion, super::control::number("VIZIER_POINTS_ROYALE_CHAMPION", 8) as i64, "won the battle royale");
    if let Some(user) = runner_up {
        give(user, super::control::number("VIZIER_POINTS_ROYALE_RUNNER_UP", 3) as i64, "runner-up in the battle royale");
    }
    battle
}

/// Fights fought and fights won, counting every 1v1 - the ones inside a battle too.
fn tally(user: u64) -> (i64, i64) {
    let Some(db) = DB.get() else {
        return (0, 0);
    };
    let conn = db.lock();
    let count = |sql: &str| conn.query_row(sql, params![user as i64], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let fights = count(
        "SELECT COUNT(*) FROM results WHERE kind != 'champion' AND (winner = ?1 OR loser = ?1)",
    );
    let wins = count("SELECT COUNT(*) FROM results WHERE kind != 'champion' AND winner = ?1");
    (fights, wins)
}

/// Battles won outright.
/// Fights and battles won per person, for the house draft. `since` is a unix
/// time to count from, or `None` for all time. Read-only.
pub fn wins_per_user(since: Option<i64>) -> std::collections::HashMap<u64, u64> {
    let mut out = std::collections::HashMap::new();
    let Some(db) = DB.get() else {
        return out;
    };
    let conn = db.lock();
    let sql = "SELECT winner, COUNT(*) FROM results WHERE ts >= ?1 GROUP BY winner";
    let Ok(mut stmt) = conn.prepare(sql) else {
        return out;
    };
    if let Ok(rows) = stmt.query_map(params![since.unwrap_or(0)], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
    }) {
        out.extend(rows.flatten());
    }
    out
}

/// Every 1v1 since `since` as (winner, loser, ts), for the panel's rivalries.
pub(crate) fn duels_since(since: i64) -> Vec<(u64, u64, i64)> {
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT winner, loser, ts FROM results WHERE loser IS NOT NULL AND kind != 'champion' AND ts >= ?1") else {
        return Vec::new();
    };
    stmt.query_map(params![since], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Fights fought, fights won and battles won outright, for the control panel.
pub(crate) fn record_of(user: u64) -> (i64, i64, i64) {
    let (fights, wins) = tally(user);
    (fights, wins, crowns(user))
}

fn crowns(user: u64) -> i64 {
    let Some(db) = DB.get() else {
        return 0;
    };
    db.lock()
        .query_row("SELECT COUNT(*) FROM results WHERE kind = 'champion' AND winner = ?1", params![user as i64], |r| {
            r.get(0)
        })
        .unwrap_or(0)
}

// --- channel and roles ------------------------------------------------------

/// The arena channel: `VIZIER_FIGHT_CHANNEL`, else the channel named
/// `fight-fight-fight`, else wherever the command was used.
async fn arena(ctx: &Context, guild: GuildId, fallback: ChannelId) -> ChannelId {
    if let Some(id) = super::control::id("VIZIER_FIGHT_CHANNEL") {
        return ChannelId::new(id);
    }
    if let Ok(channels) = guild.channels(&ctx.http).await {
        if let Some(found) = channels.values().find(|c| c.name == CHANNEL_NAME) {
            return found.id;
        }
    }
    fallback
}

/// Finds a role by name, creating it when the server has none.
async fn role_named(ctx: &Context, guild: GuildId, name: &str, colour: u32, key: &str) -> Option<RoleId> {
    let roles = guild.roles(&ctx.http).await.ok()?;
    if let Some(id) = meta_get(key).and_then(|v| v.parse::<u64>().ok()).map(RoleId::new) {
        if roles.contains_key(&id) {
            return Some(id);
        }
    }
    let wanted = name.to_lowercase();
    let id = match roles.values().find(|r| r.name.to_lowercase() == wanted) {
        Some(role) => role.id,
        None => {
            let builder = EditRole::new().name(name).colour(colour).hoist(false).mentionable(true);
            guild.create_role(&ctx.http, builder).await.ok()?.id
        }
    };
    meta_set(key, &id.get().to_string());
    Some(id)
}

/// The role a melee calls to the lists, if the server really has it. It is
/// never created: it is a role the server already owns, and a melee that
/// cannot find it goes up untagged rather than inventing one.
async fn games_role(ctx: &Context, guild: GuildId) -> Option<RoleId> {
    let wanted = RoleId::new(games_role_id()?);
    match guild.roles(&ctx.http).await {
        Ok(roles) => {
            if roles.contains_key(&wanted) {
                return Some(wanted);
            }
            tracing::warn!("battle: the games role {} is not in the server, so the melee goes up untagged", wanted);
            None
        }
        // The roles could not be read at all; the id is still the best we have.
        Err(err) => {
            tracing::warn!("battle: roles not read ({}), tagging the games role anyway", err);
            Some(wanted)
        }
    }
}

/// Moves the champion role to the winner, taking it off whoever held it.
async fn crown(ctx: &Context, guild: GuildId, winner: u64) {
    let Some(role) = role_named(ctx, guild, CHAMPION_ROLE, 0xF1C40F, "champion_role").await else {
        return;
    };
    if let Some(previous) = meta_get("champion").and_then(|v| v.parse::<u64>().ok()) {
        if previous != winner {
            if let Ok(member) = guild.member(&ctx.http, UserId::new(previous)).await {
                let _ = member.remove_role(&ctx.http, role).await;
            }
        }
    }
    if let Ok(member) = guild.member(&ctx.http, UserId::new(winner)).await {
        if let Err(err) = member.add_role(&ctx.http, role).await {
            tracing::warn!("battle: champion role not given: {}", err);
            return;
        }
    }
    meta_set("champion", &winner.to_string());
}

// --- fighters ---------------------------------------------------------------

fn display(member: &Member) -> String {
    let name = member.display_name().to_string();
    if name.chars().count() > 22 { name.chars().take(21).collect::<String>() + "…" } else { name }
}

/// A member ready to fight, picture included.
async fn contender(ctx: &Context, guild: GuildId, user: u64) -> Option<Contender> {
    let mut w = contender_named(ctx, guild, user).await?;
    w.avatar = picture(&w.face).await;
    Some(w)
}

/// A member ready to fight, without their picture yet: a big royale only needs
/// pictures for the last sixteen.
async fn contender_named(ctx: &Context, guild: GuildId, user: u64) -> Option<Contender> {
    // The cache guard is let go before any await.
    let cached = ctx.cache.guild(guild).and_then(|g| g.members.get(&UserId::new(user)).cloned());
    let member = match cached {
        Some(m) => m,
        None => guild.member(&ctx.http, UserId::new(user)).await.ok()?,
    };
    let face = member.face().replace("size=1024", "size=256");
    // Stepped-out members fight without a badge, as they asked to be left out -
    // and while the House Cup is paused NOBODY wears one, so the fight card, the
    // bracket and the lobby lists carry no crest at all.
    let house = if super::house::opted_out(user) || super::house_cup::paused() {
        None
    } else {
        super::house::house_of(user).map(house_look)
    };
    Some(Contender { id: user, name: display(&member), avatar: None, house, stage: egg_stage(points_of(user)), face })
}

async fn picture(face: &str) -> Option<Vec<u8>> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?
        .get(face)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()
        .map(|b| b.to_vec())
}

/// Drawing a card is CPU work, so it never runs on the gateway thread.
async fn fight_card(
    stage: String,
    left: Fighter,
    right: Fighter,
    line: String,
    outcome: Outcome,
    hit: Option<(usize, i32)>,
    season: Season,
) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        battle_card::fight_png(&Fight { stage, left: &left, right: &right, line, outcome, hit, season })
    })
    .await
    .ok()
    .flatten()
}

async fn champion_card(who: Fighter, subtitle: String, line: String, season: Season) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || battle_card::champion_png(&Champion { who: &who, subtitle, line, season }))
        .await
        .ok()
        .flatten()
}

/// Puts the fight message back at the bottom once chat has buried it, so the
/// fight is always the last thing in the channel. The old copy goes away.
/// `set` attaches a new picture; `carry` is the one already on the message, kept
/// when the fight has to be reposted lower down.
async fn keep_at_bottom(
    ctx: &Context,
    channel: ChannelId,
    message: &mut Message,
    text: &str,
    set: Option<Vec<u8>>,
    carry: Option<&Vec<u8>>,
    // Buttons to show: `Some(vec![])` clears them, `None` leaves them as they are.
    rows: Option<Vec<CreateActionRow>>,
) {
    let buried = BELOW.lock().get(&channel.get()).copied().unwrap_or(0) >= STICKY_AFTER;
    if !buried {
        let mut edit = EditMessage::new().content(text);
        if let Some(rows) = rows {
            edit = edit.components(rows);
        }
        if let Some(png) = set {
            edit = edit.attachments(EditAttachments::new().add(CreateAttachment::bytes(png, "fight.png")));
        }
        if let Err(err) = call(message.edit(&ctx.http, edit)).await {
            tracing::warn!("battle: fight edit failed: {}", err);
        }
        return;
    }
    // Rebuilt rather than edited: a message can't move, only be replaced.
    let mut fresh = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Some(rows) = rows.filter(|r| !r.is_empty()) {
        fresh = fresh.components(rows);
    }
    if let Some(png) = set.or_else(|| carry.cloned()) {
        fresh = fresh.add_file(CreateAttachment::bytes(png, "fight.png"));
    }
    match call(channel.send_message(&ctx.http, fresh)).await {
        Ok(posted) => {
            BELOW.lock().insert(channel.get(), 0);
            let old = std::mem::replace(message, posted);
            let http = ctx.http.clone();
            tokio::spawn(async move {
                let _ = channel.delete_message(&http, old.id).await;
            });
        }
        Err(err) => tracing::warn!("battle: fight message not moved down: {}", err),
    }
}

/// Every Discord call in a fight goes through here. A request that never comes
/// back used to freeze the whole battle behind it.
async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, String> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!("no answer from Discord in {}s", HTTP_WAIT.as_secs())),
    }
}

/// One fight of a melee: the card goes up, the exchanges land under it, then
/// the result. Every blow is a coin toss - this is the melee, and it is banter.
/// A duel is a different thing entirely; see [`duel`]. Returns the winner.
async fn play(
    ctx: &Context,
    channel: ChannelId,
    stage: &str,
    a: &Contender,
    b: &Contender,
    seed: &mut u64,
    season: Season,
) -> (Contender, i32) {
    let lines = lines();
    let mut hp = [START_HP; 2];
    // One picture at the start, one at the end: the blow-by-blow rides on the
    // text, which edits without an upload.
    let opening =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), String::new(), Outcome::Open, None, season).await;
    let head = format!("**{}** · <@{}> vs <@{}>", stage, a.id, b.id);
    let mut log: Vec<String> = Vec::new();
    let mut text = fight_text(&head, &log, a, b, &hp);
    tracing::info!("battle: {} - {} vs {}", stage, a.name, b.name);
    BELOW.lock().insert(channel.get(), 0);
    let mut message = {
        let mut msg = CreateMessage::new().content(&text).allowed_mentions(CreateAllowedMentions::new());
        if let Some(png) = opening.clone() {
            msg = msg.add_file(CreateAttachment::bytes(png, "fight.png"));
        }
        match call(channel.send_message(&ctx.http, msg)).await {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!("battle: fight card not sent: {}", err);
                return (if roll(seed, 2) == 0 { a.clone() } else { b.clone() }, START_HP);
            }
        }
    };

    // Trade blows until someone's health runs out.
    let mut turns = 0;
    while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
        turns += 1;
        tokio::time::sleep(BEAT).await;
        let attacker = roll(seed, 2) as usize;
        let swing = swing(seed, attacker);
        let (x, y) = if attacker == 0 { (a, b) } else { (b, a) };
        hp = land(hp, attacker, &swing);
        let line = fill(pick(swing.blow.lines(lines), seed), &x.name, &y.name);
        log.push(format!("{} · **{}**", line, swing.tail()));
        text = fight_text(&head, &log, a, b, &hp);
        keep_at_bottom(ctx, channel, &mut message, &text, None, opening.as_ref(), None).await;
    }

    let a_wins = hp[0] > hp[1] || (hp[0] == hp[1] && roll(seed, 2) == 0);
    let (winner, loser) = if a_wins { (a, b) } else { (b, a) };
    let finish = fill(pick(lines.finish, seed), &winner.name, &loser.name);
    text.push_str(&format!("\n\n🏆 {}", finish));
    let side = if a_wins { 0 } else { 1 };
    let done =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), finish, Outcome::Won(side), None, season).await;
    tokio::time::sleep(BEAT).await;
    keep_at_bottom(ctx, channel, &mut message, &text, done, opening.as_ref(), None).await;
    tracing::info!("battle: {} won ({} - {})", winner.name, hp[0].max(0), hp[1].max(0));
    // Between fights nobody is watching a message, so stop counting chat.
    BELOW.lock().remove(&channel.get());
    (winner.clone(), hp[if a_wins { 0 } else { 1 }].max(0))
}

// --- the scrolls: a duel ----------------------------------------------------

/// The button that opens a scroll, and the modal it opens.
const SCROLL_BUTTON: &str = "fightscroll:";
const SCROLL_MODAL: &str = "fightans:";
const SCROLL_FIELD: &str = "scrollanswer";
/// How long a scroll stands before it burns, `VIZIER_FIGHT_SCROLL_SECS`.
const SCROLL_WAIT: u64 = 25;
/// How long a fighter waits after a wrong answer. Theirs alone: the other one
/// is not held up by somebody else guessing.
const SCROLL_LOCKOUT: Duration = Duration::from_secs(6);
/// Scrolls in a duel, how many win it, and the blow reading one lands.
const SCROLLS: usize = 3;
const TO_WIN: u32 = 2;
const SCROLL_BLOW: (i32, u64) = (25, 11);

fn scroll_wait() -> Duration {
    Duration::from_secs(super::control::number("VIZIER_FIGHT_SCROLL_SECS", SCROLL_WAIT).clamp(5, 120))
}

/// One scroll that is open right now.
struct Live {
    fighters: [u64; 2],
    puzzle: battle_scroll::Puzzle,
    /// Who read it first, and how long the card had been up when they did.
    won_by: Option<(usize, Duration)>,
    /// When the card went up. Both clocks start there, so opening the box late
    /// is not a way to get more reading time.
    opened: std::time::Instant,
    /// Each fighter's own lockout after a wrong answer.
    locked: [Option<std::time::Instant>; 2],
    open: bool,
}

/// Scrolls with answers still coming in, by the message they ride on.
static OPEN_SCROLLS: LazyLock<Mutex<HashMap<u64, Live>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Whether another scroll goes up, by how many have been and the score. Three
/// scrolls; a fourth only to break a tie that somebody actually scored in - all
/// three burning is a coin flip, not a reason to burn a fourth.
fn another_scroll(done: usize, score: [u32; 2]) -> bool {
    if score[0] >= TO_WIN || score[1] >= TO_WIN {
        return false;
    }
    if done < SCROLLS {
        return true;
    }
    done == SCROLLS && score[0] == score[1] && score[0] > 0
}

/// Who took a duel: whoever read the most scrolls, or `None` when they are
/// level and the gods have to decide.
fn duel_winner(score: [u32; 2]) -> Option<usize> {
    match score[0].cmp(&score[1]) {
        std::cmp::Ordering::Greater => Some(0),
        std::cmp::Ordering::Less => Some(1),
        std::cmp::Ordering::Equal => None,
    }
}

/// The one button under a scroll.
fn scroll_rows(scroll: u64, open: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{}{}", SCROLL_BUTTON, scroll))
            .label("📜 Open the scroll")
            .style(ButtonStyle::Primary)
            .disabled(!open),
    ])]
}

/// Drawing a scroll card is CPU work, so it never runs on the gateway thread.
async fn scroll_card(
    left: Fighter,
    right: Fighter,
    number: String,
    score: [u32; 2],
    puzzle: &battle_scroll::Puzzle,
    season: Season,
    seconds: u64,
) -> Option<Vec<u8>> {
    let (prompt, spec) = (puzzle.prompt.clone(), puzzle.spec.clone());
    tokio::task::spawn_blocking(move || {
        battle_card::scroll_png(&battle_card::Scroll {
            left: &left,
            right: &right,
            number,
            score,
            prompt: &prompt,
            spec: &spec,
            season,
            seconds,
        })
    })
    .await
    .ok()
    .flatten()
}

/// A duel: best of three scrolls, first to two. Each scroll goes up as a card
/// with the puzzle drawn on it - a modal is text only, so a puzzle hidden in
/// one would be no puzzle at all - and the first fighter to read it lands a
/// real blow. Nothing here is scripted: the health shown is the health.
async fn duel(ctx: &Context, channel: ChannelId, a: &Contender, b: &Contender, seed: &mut u64, season: Season) -> (Contender, i32) {
    let mut hp = [START_HP; 2];
    let mut score = [0u32; 2];
    let mut rng = battle_scroll::Rng::new(*seed);
    let mut used: Vec<&'static str> = Vec::new();
    let mut done = 0usize;
    let wait = scroll_wait();
    tracing::info!("battle: a duel - {} vs {}", a.name, b.name);

    while another_scroll(done, score) {
        done += 1;
        let mut puzzle = battle_scroll::generate(&mut rng, &used);
        used.push(puzzle.kind);
        // Written down before it is shown, so the bank can be looked over later
        // for scrolls nobody can read and scrolls everybody can.
        if let Some(Ok(id)) = with_db(|conn| battle_scroll::record(conn, &puzzle, Utc::now().timestamp())) {
            puzzle.id = id;
        }
        let number = format!("Scroll {} of {}", done, SCROLLS.max(done));
        let card =
            scroll_card(a.card(hp[0]), b.card(hp[1]), number.clone(), score, &puzzle, season, wait.as_secs()).await;
        let head = format!(
            "📜 **{}** · <@{}> vs <@{}> — first to read it lands the blow",
            number, a.id, b.id
        );
        // The scroll's own id, settled before the card goes up, so the button
        // is live in the very first message: a card that went up and then had
        // its button added a moment later would be a card nobody could answer
        // while its clock was already running.
        let scroll = roll(seed, u64::MAX >> 12) + 1;
        let mut msg = CreateMessage::new()
            .content(&head)
            .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(a.id), UserId::new(b.id)]))
            .components(scroll_rows(scroll, true));
        if let Some(png) = card {
            msg = msg.add_file(CreateAttachment::bytes(png, "scroll.png"));
        }
        let mut posted = match call(channel.send_message(&ctx.http, msg)).await {
            Ok(posted) => posted,
            Err(err) => {
                tracing::warn!("battle: scroll not sent: {}", err);
                break;
            }
        };
        // The clock starts the instant the card is up, for both of them: not
        // when either of them gets round to pressing the button.
        OPEN_SCROLLS.lock().insert(
            scroll,
            Live {
                fighters: [a.id, b.id],
                puzzle,
                won_by: None,
                opened: std::time::Instant::now(),
                locked: [None, None],
                open: true,
            },
        );

        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let read = OPEN_SCROLLS.lock().get(&scroll).and_then(|l| l.won_by);
            if read.is_some() || tokio::time::Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let closed = {
            let mut scrolls = OPEN_SCROLLS.lock();
            match scrolls.get_mut(&scroll) {
                Some(live) => {
                    live.open = false;
                    (live.won_by, live.puzzle.answer.clone(), live.puzzle.id)
                }
                None => (None, String::new(), 0),
            }
        };
        OPEN_SCROLLS.lock().remove(&scroll);
        let (read, answer, puzzle_id) = closed;

        let outcome = match read {
            Some((side, took)) => {
                let blow = SCROLL_BLOW.0 + roll(seed, SCROLL_BLOW.1) as i32;
                hp[1 - side] = (hp[1 - side] - blow).max(0);
                score[side] += 1;
                if puzzle_id > 0 {
                    let _ = with_db(|conn| battle_scroll::solved(conn, puzzle_id));
                }
                let who = if side == 0 { a } else { b };
                let other = if side == 0 { b } else { a };
                format!(
                    "✅ **{}** read it in **{:.1}s** — “{}”. **{}** takes **-{} HP**.\nScrolls: **{} – {}**",
                    who.name,
                    took.as_secs_f32(),
                    answer,
                    other.name,
                    blow,
                    score[0],
                    score[1]
                )
            }
            None => format!(
                "🔥 Nobody could read it. The scroll burns — the answer was “{}”.\nScrolls: **{} – {}**",
                answer, score[0], score[1]
            ),
        };
        let text = format!("{}\n\n{}", head, outcome);
        let _ = call(posted.edit(&ctx.http, EditMessage::new().content(text).components(scroll_rows(scroll, false))))
            .await;
        if another_scroll(done, score) {
            tokio::time::sleep(FIGHT_GAP).await;
        }
    }

    // Whoever read the most scrolls. Level means every scroll burned, and the
    // duel is settled the only way left.
    let (side, gods) = match duel_winner(score) {
        Some(side) => (side, false),
        None => (roll(seed, 2) as usize, true),
    };
    // A coin flip still leaves somebody standing, so the bars have to say so.
    if gods {
        hp[1 - side] = (hp[1 - side] - (SCROLL_BLOW.0 + roll(seed, SCROLL_BLOW.1) as i32)).max(0);
    }
    let (winner, loser) = if side == 0 { (a, b) } else { (b, a) };
    let lines = lines();
    let finish = if gods {
        format!("Na {} padh paaya, na {} — so the gods decided. 🎲", a.name, b.name)
    } else {
        fill(pick(lines.finish, seed), &winner.name, &loser.name)
    };
    let card =
        fight_card("Challenge".into(), a.card(hp[0]), b.card(hp[1]), finish.clone(), Outcome::Won(side), None, season)
            .await;
    let mut msg = CreateMessage::new()
        .content(format!("🏆 **{}** · {} – {} on the scrolls\n{}", winner.name, score[0], score[1], finish))
        .allowed_mentions(CreateAllowedMentions::new());
    if let Some(png) = card {
        msg = msg.add_file(CreateAttachment::bytes(png, "fight.png"));
    }
    let _ = call(channel.send_message(&ctx.http, msg)).await;
    tracing::info!("battle: {} won the duel ({} - {})", winner.name, score[0], score[1]);
    (winner.clone(), hp[side].max(0))
}

/// "Open the scroll": only the two fighters, only while it stands, and only
/// once their own lockout has run out.
async fn on_scroll_button(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Ok(scroll) = rest.parse::<u64>() else {
        return;
    };
    let user = component.user.id.get();
    let refusal = {
        let scrolls = OPEN_SCROLLS.lock();
        match scrolls.get(&scroll) {
            None => Some("That scroll is ash.".to_string()),
            Some(live) => match live.fighters.iter().position(|f| *f == user) {
                None => Some("This isn't your fight. Grab some popcorn 🍿".to_string()),
                Some(_) if !live.open => Some("Too late — that scroll has already gone.".to_string()),
                Some(side) => locked_for(live, side).map(|left| {
                    format!("The scroll still smoulders — try again in {}s.", left.as_secs().max(1))
                }),
            },
        }
    };
    let answer = match refusal {
        Some(text) => return drop(component.create_response(&ctx.http, whisper(text)).await),
        None => CreateModal::new(format!("{}{}", SCROLL_MODAL, scroll), "Read the scroll").components(vec![
            CreateActionRow::InputText(
                CreateInputText::new(InputTextStyle::Short, "Your answer", SCROLL_FIELD)
                    .placeholder("Whatever the scroll asks for")
                    .required(true)
                    .max_length(80),
            ),
        ]),
    };
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Modal(answer)).await;
}

/// How long is left on a fighter's own lockout, if any.
fn locked_for(live: &Live, side: usize) -> Option<Duration> {
    let at = live.locked.get(side).copied().flatten()?;
    SCROLL_LOCKOUT.checked_sub(at.elapsed()).filter(|left| !left.is_zero())
}

/// An answer. The first right one takes the scroll; a wrong one costs that
/// fighter six seconds and nobody else anything.
pub async fn on_modal(ctx: &Context, modal: &serenity::all::ModalInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(rest) = modal.data.custom_id.strip_prefix(SCROLL_MODAL) else {
        return;
    };
    let Ok(scroll) = rest.parse::<u64>() else {
        return;
    };
    let given = modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|c| match c {
            serenity::all::ActionRowComponent::InputText(t) if t.custom_id == SCROLL_FIELD => t.value.clone(),
            _ => None,
        })
        .unwrap_or_default();
    let user = modal.user.id.get();
    let reply = {
        let mut scrolls = OPEN_SCROLLS.lock();
        match scrolls.get_mut(&scroll) {
            None => "That scroll is ash.".to_string(),
            Some(live) => match live.fighters.iter().position(|f| *f == user) {
                None => "This isn't your fight. Grab some popcorn 🍿".to_string(),
                Some(_) if !live.open || live.won_by.is_some() => "Too late — that scroll has already gone.".to_string(),
                Some(side) => match locked_for(live, side) {
                    Some(left) => format!("The scroll still smoulders — try again in {}s.", left.as_secs().max(1)),
                    None if battle_scroll::matches(&given, &live.puzzle) => {
                        live.won_by = Some((side, live.opened.elapsed()));
                        live.open = false;
                        "Read it. The blow lands ⚔️".to_string()
                    }
                    None => {
                        live.locked[side] = Some(std::time::Instant::now());
                        format!("Wrong. The scroll smoulders — try again in {}s.", SCROLL_LOCKOUT.as_secs())
                    }
                },
            },
        }
    };
    let _ = modal.create_response(&ctx.http, whisper(reply)).await;
}

/// Health after a blow. A chaos turn hurts both, so both bars can empty at once;
/// someone has to be left standing: whoever was healthier keeps a sliver, and
/// on a dead tie it goes to the one who swung.
fn land(before: [i32; 2], attacker: usize, swing: &Swing) -> [i32; 2] {
    let mut hp = before;
    for side in 0..2 {
        hp[side] = (hp[side] + swing.hits[side]).clamp(0, START_HP);
    }
    if hp == [0, 0] {
        let standing = match before[0].cmp(&before[1]) {
            std::cmp::Ordering::Greater => 0,
            std::cmp::Ordering::Less => 1,
            std::cmp::Ordering::Equal => attacker,
        };
        hp[standing] = 1;
    }
    hp
}

/// A whole fight rolled the ordinary way, without Discord: the blows and the
/// health left at the end.
fn roll_fight(seed: &mut u64) -> (Vec<(usize, Swing)>, [i32; 2]) {
    let mut hp = [START_HP; 2];
    let mut blows = Vec::new();
    while hp[0] > 0 && hp[1] > 0 && blows.len() < MAX_EXCHANGES {
        let attacker = roll(seed, 2) as usize;
        let swing = swing(seed, attacker);
        hp = land(hp, attacker, &swing);
        blows.push((attacker, swing));
    }
    (blows, hp)
}

/// Counts chat under the live fight so it can be moved back to the bottom.
/// Returns false for everything else, so the rest of the bot still sees it.
pub fn note_chat(channel: ChannelId) -> bool {
    let mut below = BELOW.lock();
    match below.get_mut(&channel.get()) {
        Some(count) => {
            *count += 1;
            true
        }
        None => false,
    }
}

/// Health as blocks, so the bar moves on a plain message edit with nothing to upload.
fn bar(hp: i32) -> String {
    let full = ((hp.clamp(0, START_HP) as f32 / START_HP as f32) * BAR_BLOCKS as f32).round() as usize;
    // Anything still standing keeps one block, so a bar never reads as empty too early.
    let full = if hp > 0 { full.max(1) } else { 0 };
    format!("{}{}", "█".repeat(full), "░".repeat(BAR_BLOCKS - full))
}

/// The message under the card: both bars and the last few exchanges, trimmed so
/// a long fight never runs past Discord's message limit.
fn fight_text(head: &str, log: &[String], a: &Contender, b: &Contender, hp: &[i32; 2]) -> String {
    let side = |who: &Contender, hp: i32| {
        let name: String = who.name.chars().take(14).collect();
        format!("`{:<14}` `{}` **{:>3}**", name, bar(hp), hp.max(0))
    };
    let recent = log.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join("\n");
    format!("{}\n❤️ {}\n💙 {}\n{}", head, side(a, hp[0]), side(b, hp[1]), recent)
}

// --- /fight -----------------------------------------------------------------

pub async fn fight_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.".into())).await;
        return;
    };
    let target = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id),
        _ => None,
    });
    let Some(target) = target else {
        let _ = command.create_response(&ctx.http, whisper("Who do you want to fight? Use `/fight @name`.".into())).await;
        return;
    };
    let me = command.user.id.get();
    let them = target.get();
    let refusal = if them == me {
        Some("You can't fight yourself.".to_string())
    } else if ctx.cache.user(target).map(|u| u.bot).unwrap_or(false) {
        Some("Pick a human, not a bot.".to_string())
    } else {
        LAST_FIGHT
            .lock()
            .get(&me)
            .and_then(|at| fight_cooldown().checked_sub(at.elapsed()))
            .filter(|left| !left.is_zero())
            .map(|left| format!("Take a breather — you can challenge again in {}s.", left.as_secs().max(1)))
    };
    if let Some(text) = refusal {
        let _ = command.create_response(&ctx.http, whisper(text)).await;
        return;
    }

    let here = command.channel_id;
    let arena = arena(ctx, guild, here).await;
    let _ = command.defer_ephemeral(&ctx.http).await;
    let (Some(a), Some(b)) = (contender(ctx, guild, me).await, contender(ctx, guild, them).await) else {
        let _ = command
            .edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content("Couldn't find that member."))
            .await;
        return;
    };

    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let season = season();
    let card = fight_card(
        "Challenge".into(),
        a.card(START_HP),
        b.card(START_HP),
        format!("{} vs {}", a.name, b.name),
        Outcome::Open,
        None,
        season,
    )
    .await;
    let wait = challenge_wait();
    // A duel is between two named people, so only those two are tagged: no
    // role is called to the lists for a challenge.
    let content = format!(
        "⚔️ <@{}> has called <@{}> out for a duel in the lists!\n<@{}>, accept or decline — the challenge expires in {}.\n         -# Three scrolls, first to two. Read what is on the card and hit Open the scroll.",
        me,
        them,
        them,
        span(wait.as_secs())
    );
    let mut msg = CreateMessage::new()
        .content(content)
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(them)]))
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new("fightyes:0").label("⚔️ Accept").style(ButtonStyle::Success),
            CreateButton::new("fightno:0").label("🏃 Decline").style(ButtonStyle::Secondary),
        ])]);
    if let Some(png) = card {
        msg = msg.add_file(CreateAttachment::bytes(png, "challenge.png"));
    }
    let Ok(posted) = arena.send_message(&ctx.http, msg).await else {
        let _ = command
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new().content("Couldn't post in the fight channel."),
            )
            .await;
        return;
    };

    // The buttons carry the message id, so the handler finds this challenge.
    let id = posted.id.get();
    let rows = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("fightyes:{}", id)).label("⚔️ Accept").style(ButtonStyle::Success),
        CreateButton::new(format!("fightno:{}", id)).label("🏃 Decline").style(ButtonStyle::Secondary),
    ])];
    let mut posted = posted;
    let _ = posted.edit(&ctx.http, EditMessage::new().components(rows)).await;
    CHALLENGES.lock().insert(id, Challenge { from: me, to: them, accepted: None });
    LAST_FIGHT.lock().insert(me, std::time::Instant::now());

    let link = posted.link();
    let note = if arena == here {
        format!("Challenge sent: {}", link)
    } else {
        format!("⚔️ <@{}> challenged <@{}> → {}", me, them, link)
    };
    let _ = command.edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content("Challenge sent.")).await;
    if arena != here {
        let _ =
            here.send_message(&ctx.http, CreateMessage::new().content(note).allowed_mentions(CreateAllowedMentions::new()))
                .await;
    }

    // Wait for the answer, then either fight or let the challenge lapse.
    let deadline = std::time::Instant::now() + wait;
    let answer = loop {
        if let Some(answer) = CHALLENGES.lock().get(&id).and_then(|c| c.accepted) {
            break Some(answer);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    CHALLENGES.lock().remove(&id);
    let _ = posted.edit(&ctx.http, EditMessage::new().components(vec![])).await;
    match answer {
        Some(true) => {
            if !BUSY.lock().insert(arena.get()) {
                let _ = arena.say(&ctx.http, "A fight is already running here — try again in a moment.").await;
                return;
            }
            let (winner, _) = duel(ctx, arena, &a, &b, &mut seed, season).await;
            let loser = if winner.id == a.id { b.id } else { a.id };
            record("fight", winner.id, Some(loser));
            // The points, and the reason when there are none.
            let now = Utc::now().timestamp();
            let paid = pay_duel(winner.id, loser, now);
            let (fights, wins) = tally(winner.id);
            // A duel's winner earns a card from the deck as well as the points.
            let prize = super::battle_prize::award("duel", now, winner.id, roll(&mut seed, 10_000) as f64 / 10_000.0);
            let text = std::iter::once(format!(
                "🏆 **{}** won the duel{}! That is **{}** wins from **{}** fights. `/fightboard` for the rest.",
                winner.name,
                paid.said(),
                wins,
                fights
            ))
            .chain(prize.as_ref().map(super::battle_prize::prize_line))
            .collect::<Vec<_>>()
            .join("\n");
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
            BUSY.lock().remove(&arena.get());
        }
        Some(false) => {
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!("🏃 <@{}> walked away from the lists.", them))
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
        }
        None => {
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!("⌛ <@{}> never answered. Challenge cancelled.", them))
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
        }
    }
}

// --- /battle ----------------------------------------------------------------

pub async fn battle_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.".into())).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only admins can call a melee.".into())).await;
        return;
    }
    let minutes = command
        .data
        .options
        .iter()
        .find_map(|o| match o.value {
            CommandDataOptionValue::Integer(n) => Some(n),
            _ => None,
        })
        .unwrap_or_else(default_lobby_minutes)
        .clamp(MIN_WAIT, max_lobby_minutes());

    let here = command.channel_id;
    let arena = arena(ctx, guild, here).await;
    if !BUSY.lock().insert(arena.get()) {
        let _ = command.create_response(&ctx.http, whisper("A fight is already running in the lists.".into())).await;
        return;
    }
    let _ = command.create_response(&ctx.http, whisper(format!("Lobby open for {} minutes.", minutes))).await;
    open_lobby(ctx, guild, arena, here, minutes, Ping::Games).await;
}

/// Who a melee's lobby calls to the lists when it opens. A duel never tags a
/// role at all, so there is no `Ping` anywhere in that path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ping {
    /// The month's own role: the people who opted in. The default everywhere,
    /// and the only role a melee announcement ever mentions.
    Games,
    Houses,
    Everyone,
    Nobody,
}

impl Ping {
    fn from_key(key: &str) -> Ping {
        match key.trim().to_ascii_lowercase().as_str() {
            "houses" | "house" => Ping::Houses,
            "everyone" => Ping::Everyone,
            "none" | "nobody" => Ping::Nobody,
            _ => Ping::Games,
        }
    }

    /// The same keys, but a word nobody recognises is refused instead of
    /// quietly tagging the whole server: the panel names who it will tag, so it
    /// must not tag someone else.
    fn parse(key: &str) -> Result<Ping, String> {
        match key.trim().to_ascii_lowercase().as_str() {
            "games" | "game" => Ok(Ping::Games),
            "houses" | "house" => Ok(Ping::Houses),
            "everyone" => Ok(Ping::Everyone),
            "none" | "nobody" => Ok(Ping::Nobody),
            other => Err(format!(
                "“{}” isn't someone to tag. Pick the server games role, the houses, @everyone or nobody.",
                other
            )),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Ping::Games => "games",
            Ping::Houses => "houses",
            Ping::Everyone => "everyone",
            Ping::Nobody => "none",
        }
    }

    /// How the panel says who was tagged, as the end of "… were tagged".
    fn label(self) -> &'static str {
        match self {
            Ping::Games => "the server games role",
            Ping::Houses => "the four houses",
            Ping::Everyone => "everyone",
            Ping::Nobody => "nobody",
        }
    }
}

/// The roles a lobby's heads-up may mention, as the server actually has them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Tags {
    games: Option<RoleId>,
    houses: Vec<RoleId>,
}

/// What a lobby's heads-up says, who it is allowed to mention, and whether it
/// is an @everyone. `None` means no heads-up goes up at all: a role that is
/// unset, gone from the server or switched off leaves the lobby card to go up
/// untagged rather than tagging the wrong people or failing.
fn heads_up(ping: Ping, tags: &Tags) -> Option<(String, Vec<RoleId>, bool)> {
    let roles = |ids: Vec<RoleId>| {
        (!ids.is_empty())
            .then(|| (ids.iter().map(|r| format!("<@&{}>", r)).collect::<Vec<_>>().join(" "), ids, false))
    };
    let (text, ids, everyone) = match ping {
        Ping::Nobody => return None,
        Ping::Games => roles(tags.games.into_iter().collect())?,
        // While the Cup is paused the four house roles are not tagged by a
        // game, so the month's own role is called instead - it says the same
        // thing and it is who opted in.
        Ping::Houses if super::house_cup::paused() => roles(tags.games.into_iter().collect())?,
        Ping::Houses => roles(tags.houses.clone())?,
        Ping::Everyone => ("@everyone".to_string(), Vec::new(), true),
    };
    Some((format!("{} — a melee is forming in the lists! Join the lobby below 👇", text), ids, everyone))
}

/// Opens a lobby in the lists, waits it out, and runs the melee. The arena must
/// already be marked busy; it is freed at the end.
async fn open_lobby(ctx: &Context, guild: GuildId, arena: ChannelId, here: ChannelId, minutes: i64, ping: Ping) {
    let ends = Utc::now().timestamp() + minutes * 60;
    // Only look up the roles a heads-up could actually want.
    let tags = Tags {
        games: match ping {
            Ping::Games | Ping::Houses => games_role(ctx, guild).await,
            _ => None,
        },
        houses: match ping {
            Ping::Houses if !super::house_cup::paused() => super::house::house_roles(ctx, guild).await,
            _ => Vec::new(),
        },
    };
    // The tags go in a message of their own that stays put: the lobby card
    // moves down as chat piles up (and the old copy is deleted), which used to
    // take the tags with it. No role to tag means no heads-up: the lobby below
    // still goes up.
    match heads_up(ping, &tags) {
        Some((text, roles, everyone)) => {
            let mentions = CreateAllowedMentions::new().roles(roles).everyone(everyone);
            let msg = CreateMessage::new().content(text).allowed_mentions(mentions);
            if let Err(err) = call(arena.send_message(&ctx.http, msg)).await {
                tracing::warn!("battle: lobby tags not posted in {}: {}", arena, err);
            }
        }
        None => tracing::info!("battle: lobby in {} goes up untagged ({:?})", arena, ping),
    }
    let embed = lobby_embed(&[], ends, minutes);
    let msg = CreateMessage::new()
        .content("⚔️ The melee · hit Join 👇")
        .allowed_mentions(CreateAllowedMentions::new())
        .embed(embed)
        .components(lobby_buttons(0, true));
    let posted = match call(arena.send_message(&ctx.http, msg)).await {
        Ok(posted) => posted,
        Err(err) => {
            tracing::warn!("battle: lobby not posted in {}: {}", arena, err);
            BUSY.lock().remove(&arena.get());
            return;
        }
    };
    tracing::info!("battle: lobby {} open in {} for {} min", posted.id, arena, minutes);
    let lobby_id = posted.id.get();
    let rows = lobby_buttons(lobby_id, true);
    let mut posted = posted;
    let _ = posted.edit(&ctx.http, EditMessage::new().components(rows)).await;
    LOBBIES
        .lock()
        .insert(lobby_id, Lobby { joined: Vec::new(), names: HashMap::new(), open: true, ends, minutes });
    if arena != here {
        let _ = here
            .send_message(
                &ctx.http,
                CreateMessage::new().content(format!("⚔️ The melee is running here → {}", posted.link())),
            )
            .await;
    }

    // Keep the lobby at the bottom while it is open, like a live fight: once
    // enough chat piles on top of it (an @everyone brings a rush), it is posted
    // again below and the old copy removed. The repost doesn't ping again.
    BELOW.lock().insert(arena.get(), 0);
    while Utc::now().timestamp() < ends {
        tokio::time::sleep(LOBBY_TICK).await;
        let buried = BELOW.lock().get(&arena.get()).copied().unwrap_or(0) >= STICKY_AFTER;
        if !buried {
            continue;
        }
        let names = LOBBIES.lock().get(&lobby_id).map(|l| l.joined.iter().filter_map(|u| l.names.get(u).cloned()).collect::<Vec<_>>());
        let Some(names) = names else {
            break;
        };
        let fresh = CreateMessage::new()
            .content("⚔️ The melee · hit Join 👇")
            .allowed_mentions(CreateAllowedMentions::new())
            .embed(lobby_embed(&names, ends, minutes))
            .components(lobby_buttons(lobby_id, true));
        match call(arena.send_message(&ctx.http, fresh)).await {
            Ok(moved) => {
                BELOW.lock().insert(arena.get(), 0);
                let old = std::mem::replace(&mut posted, moved);
                let http = ctx.http.clone();
                tokio::spawn(async move {
                    let _ = arena.delete_message(&http, old.id).await;
                });
            }
            Err(err) => tracing::warn!("battle: lobby not moved down: {}", err),
        }
    }
    BELOW.lock().remove(&arena.get());
    let joined = {
        let mut lobbies = LOBBIES.lock();
        let lobby = lobbies.get_mut(&lobby_id);
        match lobby {
            Some(l) => {
                l.open = false;
                l.joined.clone()
            }
            None => Vec::new(),
        }
    };
    let _ = posted.edit(&ctx.http, EditMessage::new().components(lobby_buttons(lobby_id, false))).await;
    LOBBIES.lock().remove(&lobby_id);

    let needed = min_players();
    tracing::info!("battle: lobby {} closed with {} joined", lobby_id, joined.len());
    if joined.len() < needed {
        let _ = arena
            .say(&ctx.http, format!("Only {} came to the lists. Melee called off — {} are needed.", joined.len(), needed))
            .await;
        BUSY.lock().remove(&arena.get());
        return;
    }
    run_battle(ctx, guild, arena, joined).await;
    BUSY.lock().remove(&arena.get());
}

fn lobby_buttons(id: u64, open: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("battlejoin:{}", id)).label("⚔️ Join").style(ButtonStyle::Success).disabled(!open),
    ])]
}

fn lobby_embed(names: &[String], ends: i64, minutes: i64) -> CreateEmbed {
    let list = if names.is_empty() {
        "Nobody yet. Who rides first?".to_string()
    } else {
        let shown = lobby_names();
        let mut list = names.iter().take(shown).map(|n| format!("• {}", n)).collect::<Vec<_>>().join("\n");
        if names.len() > shown {
            list.push_str(&format!("\n…and **{}** more", names.len() - shown));
        }
        list
    };
    CreateEmbed::new()
        .title("⚔️ The Great Melee")
        .description(format!(
            "Hit Join and ride into the lists. The last one standing takes the **{}** role and a card from the deck.\n\n             ⏳ Closes <t:{}:R> ({} min)\n👥 **{}** in the lists (at least {} needed)\n\n{}",
            CHAMPION_ROLE,
            ends,
            minutes,
            names.len(),
            min_players(),
            list
        ))
        .colour(0xB0742A)
        .footer(CreateEmbedFooter::new("Every pass is a coin toss — this is banter, not a ladder"))
}

// --- the daily battle ---------------------------------------------------------

/// How late a daily battle may still open after its time, if the arena was busy
/// or the bot was down.
const DAILY_GRACE_SECS: i64 = 30 * 60;

/// Seconds after India midnight for an "HH:MM" time.
fn clock_secs(time: &str) -> Option<i64> {
    let (h, m) = time.trim().split_once(':')?;
    let (h, m) = (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 3600 + m * 60)
}

/// The first of today's slots that is due now, as `(day, time)`: `now` is within
/// the grace after it and it hasn't opened yet (`done` says whether a day's slot
/// already ran).
fn daily_due(now: i64, times: &str, done: impl Fn(&str, &str) -> bool) -> Option<(String, String)> {
    let offset = super::stats::ist().local_minus_utc() as i64;
    let midnight = (now + offset).div_euclid(86_400) * 86_400 - offset;
    times.split(',').map(str::trim).filter(|t| !t.is_empty()).find_map(|time| {
        let slot = midnight + clock_secs(time)?;
        let day = super::points::ist_day(slot);
        (now >= slot && now - slot <= DAILY_GRACE_SECS && !done(&day, time)).then(|| (day, time.to_string()))
    })
}

/// Opens a melee every day at `VIZIER_BATTLE_DAILY_TIME` (India time) when
/// `VIZIER_BATTLE_DAILY` is on - the same lobby /battle opens, so nothing about
/// the melee itself differs. If a fight is running at that moment it tries
/// again every half minute for up to half an hour, which is also what carries a
/// slot that came due while the bot was down.
pub fn spawn_daily(ctx: Context) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if !super::control::on("VIZIER_BATTLE_DAILY", false) {
                continue;
            }
            let times = super::control::var("VIZIER_BATTLE_DAILY_TIME").unwrap_or_else(|| "21:00".into());
            let now = Utc::now().timestamp();
            // Each time of day opens once: its day and time are remembered. The
            // single key from before several times were allowed still counts.
            let legacy = meta_get("daily_battle_day");
            let done = |day: &str, time: &str| {
                meta_get(&format!("daily_battle:{}:{}", day, time)).is_some() || legacy.as_deref() == Some(day) && time == times.split(',').next().unwrap_or("").trim()
            };
            let Some((day, time)) = daily_due(now, &times, done) else {
                continue;
            };
            let slot_key = format!("daily_battle:{}:{}", day, time);
            let Some(guild) = ctx.cache.guilds().first().copied() else {
                continue;
            };
            let Some(fallback) = super::control::id("VIZIER_FIGHT_CHANNEL").map(ChannelId::new) else {
                tracing::warn!("battle: the daily melee is on but VIZIER_FIGHT_CHANNEL is not set");
                meta_set(&slot_key, "skipped");
                continue;
            };
            let arena = arena(&ctx, guild, fallback).await;
            if !BUSY.lock().insert(arena.get()) {
                continue;
            }
            meta_set(&slot_key, "opened");
            let minutes = (super::control::number("VIZIER_BATTLE_DAILY_MINUTES", 10) as i64).clamp(MIN_WAIT, max_lobby_minutes());
            let ping = Ping::from_key(&super::control::var("VIZIER_BATTLE_DAILY_PING").unwrap_or_default());
            tracing::info!("battle: daily melee opening for {} {} ({} min, {:?})", day, time, minutes, ping);
            let ctx = ctx.clone();
            tokio::spawn(async move {
                open_lobby(&ctx, guild, arena, arena, minutes, ping).await;
            });
        }
    });
}

// --- starting a battle from the web panel -----------------------------------

/// Why a panel start was refused, in words an admin can act on.
const NO_GUILD: &str = "The bot isn't in a server right now, so there's nowhere to fight. Try again once it's back online.";
const NO_ARENA: &str = "There's no arena to fight in. Set the fight channel in Arena → Fight channel, or make a channel called #fight-fight-fight.";
const ARENA_BUSY: &str = "A melee or duel is already running in the arena. Wait for it to finish, or clear it with /battlestop.";

/// A lobby the panel has just opened, so it can say what it did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedBattle {
    /// The arena the lobby went up in.
    pub channel: u64,
    pub minutes: i64,
    /// `games`, `houses`, `everyone` or `none`.
    pub ping: &'static str,
    /// "the server games role", for the sentence the panel shows.
    pub ping_label: &'static str,
}

/// Marks the arena busy. `true` means the claim is ours: it has to be handed to
/// [`open_lobby`] (which frees it when the battle ends) or given back with
/// [`free_arena`], or nothing can fight there again until a restart.
fn claim_arena(arena: ChannelId) -> bool {
    BUSY.lock().insert(arena.get())
}

fn free_arena(arena: ChannelId) {
    BUSY.lock().remove(&arena.get());
}

/// The arena a panel start would use: the same channel `/battle` picks, but with
/// no command channel to fall back on, so "nowhere to fight" is an answer.
async fn arena_or_none(ctx: &Context, guild: GuildId) -> Option<ChannelId> {
    let nowhere = ChannelId::new(u64::MAX);
    let found = arena(ctx, guild, nowhere).await;
    (found != nowhere).then_some(found)
}

/// Everything a panel start settles once the arena is known: the claim on it,
/// the lobby length and who gets tagged. Split from the Discord
/// half so it can be tested without a server, and it owns the claim - a start
/// that ends up refused frees the arena again rather than wedging it.
fn start_plan(arena: ChannelId, minutes: Option<i64>, ping: Option<&str>) -> Result<(StartedBattle, Ping), String> {
    if !claim_arena(arena) {
        return Err(ARENA_BUSY.to_string());
    }
    let plan = (|| {
        let minutes = minutes.unwrap_or_else(default_lobby_minutes).clamp(MIN_WAIT, max_lobby_minutes());
        let ping = match ping.map(str::trim).filter(|k| !k.is_empty()) {
            Some(key) => Ping::parse(key)?,
            // The month's own role, like the daily melee: a panel start is
            // meant to wake up the people who opted in.
            None => Ping::Games,
        };
        let started =
            StartedBattle { channel: arena.get(), minutes, ping: ping.key(), ping_label: ping.label() };
        Ok((started, ping))
    })();
    if plan.is_err() {
        free_arena(arena);
    }
    plan
}

/// Opens a melee lobby this instant, with no command behind it: the web panel's
/// "Call a melee now". The same lobby `/battle` and the daily melee open, in
/// the same arena, so nothing about the melee itself differs.
pub async fn start_now(
    ctx: &Context,
    minutes: Option<i64>,
    ping: Option<&str>,
) -> Result<StartedBattle, String> {
    let Some(guild) = ctx.cache.guilds().first().copied() else {
        return Err(NO_GUILD.to_string());
    };
    let Some(arena) = arena_or_none(ctx, guild).await else {
        return Err(NO_ARENA.to_string());
    };
    let (started, tags) = start_plan(arena, minutes, ping)?;
    tracing::info!("battle: panel opened a lobby in {} ({} min, {:?})", arena, started.minutes, tags);
    let ctx = ctx.clone();
    let minutes = started.minutes;
    tokio::spawn(async move {
        open_lobby(&ctx, guild, arena, arena, minutes, tags).await;
    });
    Ok(started)
}

/// A panel start with the Discord half left out: the arena is handed in rather
/// than found in the server, and no lobby is posted. The panel's own tests drive
/// its endpoint through this, so a test never starts a real battle.
#[cfg(test)]
pub(crate) fn start_plan_for_tests(
    arena: u64,
    minutes: Option<i64>,
    ping: Option<&str>,
) -> Result<StartedBattle, String> {
    start_plan(ChannelId::new(arena), minutes, ping).map(|(plan, _)| plan)
}

/// Gives a test's claim on an arena back, as a finished lobby would.
#[cfg(test)]
pub(crate) fn free_arena_for_tests(arena: u64) {
    free_arena(ChannelId::new(arena));
}

/// Knockout rounds until one is left, on a draw fixed at the start: winners
/// meet the winner beside them, and the bracket goes up before every round.
async fn run_battle(ctx: &Context, guild: GuildId, arena: ChannelId, joined: Vec<u64>) {
    let season = season();
    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let mut fighters: Vec<Contender> = Vec::new();
    for id in joined {
        if let Some(w) = contender_named(ctx, guild, id).await {
            fighters.push(w);
        }
    }
    if fighters.len() < min_players() {
        let _ = arena.say(&ctx.http, "Not enough fighters could be loaded. Melee called off.").await;
        return;
    }
    shuffle(&mut fighters, &mut seed);
    let started = fighters.len();
    let mut rounds = draw_bracket(started, &mut seed);
    let total = rounds.len();
    // The chart starts at the first round small enough to draw.
    let chart_start = rounds.iter().position(|round| round.len() <= CHART_FROM).unwrap_or(0);
    let mut entrants: Option<Arc<Vec<Entrant>>> = None;
    // The final's loser is the runner-up.
    let mut runner_up: Option<u64> = None;
    // Read once, so a change mid-battle can't turn a played-out round back into a list.
    let full_fights_up_to = full_fights_up_to();
    for r in 0..total {
        let matches = rounds[r].len();
        let title = round_title(matches);
        if r >= chart_start {
            if entrants.is_none() {
                // Pictures for whoever is still in, fetched once.
                let still_in: Vec<usize> = rounds[r].iter().flat_map(|m| [m.a, m.b]).flatten().collect();
                for i in still_in {
                    if fighters[i].avatar.is_none() {
                        fighters[i].avatar = picture(&fighters[i].face).await;
                    }
                }
                entrants = Some(Arc::new(
                    fighters
                        .iter()
                        .map(|w| Entrant {
                            name: w.name.clone(),
                            avatar: w.avatar.clone(),
                            house: w.house.clone(),
                            stage: w.stage,
                        })
                        .collect(),
                ));
            }
            if let Some(entrants) = &entrants {
                let subtitle = format!("{} in the lists · {}", started, title);
                let caption = format!("🗺️ **{}**: here's the draw", title);
                post_bracket(ctx, arena, entrants, &rounds[chart_start..], subtitle, season, &caption).await;
            }
        }
        if r == 0 {
            let passes: Vec<String> = rounds[0].iter().filter(|m| m.bye).filter_map(|m| m.a).map(|i| tag(&fighters[i])).collect();
            if !passes.is_empty() {
                say_chunks(ctx, arena, &format!("⛺ **Rode on unopposed** ({})", passes.len()), &passes, ", ").await;
            }
        }
        tokio::time::sleep(FIGHT_GAP).await;

        if matches > full_fights_up_to {
            // A quick round: every match settled at once, posted as a list.
            let mut results = Vec::new();
            for j in 0..matches {
                let (Some(ai), Some(bi), false) = (rounds[r][j].a, rounds[r][j].b, rounds[r][j].bye) else {
                    continue;
                };
                let (side, hp) = quick_result(&mut seed);
                let (winner, loser) = if side == 0 { (ai, bi) } else { (bi, ai) };
                record("battle", fighters[winner].id, Some(fighters[loser].id));
                rounds[r][j].winner = Some(side);
                rounds[r][j].hp = Some(hp);
                advance(&mut rounds, r, j);
                results.push(format!("{} beat {} · {} HP", tag_bold(&fighters[winner]), tag(&fighters[loser]), hp));
            }
            let head = format!("⚡ **{}** · settled at once · {} passes", title, results.len());
            say_chunks(ctx, arena, &head, &results, "\n").await;
        } else {
            for j in 0..matches {
                let (Some(ai), Some(bi), false) = (rounds[r][j].a, rounds[r][j].b, rounds[r][j].bye) else {
                    continue;
                };
                let (winner, hp) = play(ctx, arena, &title, &fighters[ai], &fighters[bi], &mut seed, season).await;
                let (side, loser) = if winner.id == fighters[ai].id { (0, fighters[bi].id) } else { (1, fighters[ai].id) };
                record("battle", winner.id, Some(loser));
                runner_up = Some(loser);
                rounds[r][j].winner = Some(side);
                rounds[r][j].hp = Some(hp);
                advance(&mut rounds, r, j);
                tokio::time::sleep(FIGHT_GAP).await;
            }
        }
        if r + 1 < total {
            tokio::time::sleep(ROUND_GAP).await;
        }
    }

    let Some(champion) = champion_of(&rounds).map(|i| fighters[i].clone()) else {
        return;
    };
    if let Some(entrants) = &entrants {
        let subtitle = format!("{} in the lists · {} rounds · 👑 {}", started, total, champion.name);
        post_bracket(ctx, arena, entrants, &rounds[chart_start..], subtitle, season, "🗺️ **The final draw**").await;
    }
    record("champion", champion.id, None);
    let battle_id = award_royale(champion.id, runner_up);
    // A card from the deck for the champion and runner-up of a big enough melee,
    // and one for the champion from the lists themselves.
    let card_lines = super::frog_rewards::royale_cards(battle_id, champion.id, runner_up, started);
    let prize = super::battle_prize::award("melee", battle_id, champion.id, roll(&mut seed, 10_000) as f64 / 10_000.0);
    let won = crowns(champion.id);
    crown(ctx, guild, champion.id).await;
    // The champion's house is named on the card as well as the member, once
    // there is a house to name.
    let whose = match (season.houses(), champion.house.as_ref()) {
        (true, Some(house)) => format!("House {}", house.name),
        _ => "1 champion".to_string(),
    };
    let subtitle = format!("{} in the lists · {} rounds · {}", started, total, whose);
    let line = pick(lines().champion, &mut seed).to_string();
    let card = champion_card(champion.card(START_HP), subtitle, line, season).await;
    let mut msg = CreateMessage::new()
        .content(
            std::iter::once(format!(
                "👑 <@{}> is the **{}**! Melees won: **{}**",
                champion.id, CHAMPION_ROLE, won
            ))
            .chain(prize.as_ref().map(super::battle_prize::prize_line))
            .chain(card_lines)
            .collect::<Vec<_>>()
            .join("\n"),
        )
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(champion.id)]));
    if let Some(png) = card {
        msg = msg.add_file(CreateAttachment::bytes(png, "champion.png"));
    }
    let _ = arena.send_message(&ctx.http, msg).await;
}

/// A quick-round match: a whole fight rolled out of sight. The side left
/// healthier wins; a dead level fight is a coin toss. Returns the winning side
/// and their health left.
fn quick_result(seed: &mut u64) -> (usize, i32) {
    let (_, hp) = roll_fight(seed);
    let side = match hp[0].cmp(&hp[1]) {
        std::cmp::Ordering::Greater => 0,
        std::cmp::Ordering::Less => 1,
        std::cmp::Ordering::Equal => roll(seed, 2) as usize,
    };
    (side, hp[side].max(1))
}

/// A name as it goes in a list: house crest first, markdown taken out.
fn tag(w: &Contender) -> String {
    format!("{}{}", crest_of(w), plain(&w.name))
}

fn tag_bold(w: &Contender) -> String {
    format!("{}**{}**", crest_of(w), plain(&w.name))
}

/// The crest to put before a name in chat: nothing at all before the hatch,
/// when nobody has a house yet.
fn crest_of(w: &Contender) -> String {
    match (hatched(), w.house.as_ref()) {
        (true, Some(house)) => format!("{} ", house.crest),
        _ => String::new(),
    }
}

/// A display name with the characters Discord would read as formatting removed.
fn plain(name: &str) -> String {
    name.chars().filter(|c| !matches!(c, '*' | '_' | '~' | '`' | '|' | '>')).collect()
}

/// Posts a heading and a list, split across messages so none passes Discord's
/// 2000 characters.
async fn say_chunks(ctx: &Context, arena: ChannelId, head: &str, items: &[String], sep: &str) {
    for chunk in chunk_list(head, items, sep, 1900) {
        let msg = CreateMessage::new().content(chunk).allowed_mentions(CreateAllowedMentions::new());
        if let Err(err) = call(arena.send_message(&ctx.http, msg)).await {
            tracing::warn!("battle: list not posted: {}", err);
        }
    }
}

/// The heading and items as messages of at most `limit` characters, the heading
/// on the first.
fn chunk_list(head: &str, items: &[String], sep: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = head.to_string();
    let mut first_item = true;
    for item in items {
        let item: String = item.chars().take(limit / 2).collect();
        let joiner = if first_item { "\n" } else { sep };
        if current.chars().count() + joiner.chars().count() + item.chars().count() > limit {
            out.push(std::mem::take(&mut current));
            current = item;
        } else {
            current.push_str(joiner);
            current.push_str(&item);
        }
        first_item = false;
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Draws the bracket off the gateway thread and posts it.
#[allow(clippy::too_many_arguments)]
async fn post_bracket(
    ctx: &Context,
    arena: ChannelId,
    entrants: &Arc<Vec<Entrant>>,
    rounds: &[Vec<Slot>],
    subtitle: String,
    season: Season,
    caption: &str,
) {
    let (entrants, rounds) = (entrants.clone(), rounds.to_vec());
    let png = tokio::task::spawn_blocking(move || {
        battle_bracket::bracket_png(&Bracket { entrants: &entrants, rounds: &rounds, subtitle, season })
    })
    .await
    .ok()
    .flatten();
    let Some(png) = png else {
        return;
    };
    let msg = CreateMessage::new()
        .content(caption)
        .allowed_mentions(CreateAllowedMentions::new())
        .add_file(CreateAttachment::bytes(png, "bracket.png"));
    if let Err(err) = call(arena.send_message(&ctx.http, msg)).await {
        tracing::warn!("battle: bracket not posted: {}", err);
    }
}

/// The opening draw for `n` entrants, in the order they were shuffled. The
/// bracket is the next power of two in size; the spare places are free passes,
/// spread at random over the first round, each pairing one entrant with nobody.
/// Free passes are already moved on to round two.
fn draw_bracket(n: usize, seed: &mut u64) -> Vec<Vec<Slot>> {
    let size = n.next_power_of_two().max(2);
    let first = size / 2;
    let mut bye = vec![false; first];
    for pass in bye.iter_mut().take(size - n) {
        *pass = true;
    }
    shuffle(&mut bye, seed);
    let mut next = 0;
    let opening: Vec<Slot> = bye
        .iter()
        .map(|&pass| {
            let slot = if pass {
                Slot { a: Some(next), winner: Some(0), bye: true, ..Slot::default() }
            } else {
                Slot { a: Some(next), b: Some(next + 1), ..Slot::default() }
            };
            next += if pass { 1 } else { 2 };
            slot
        })
        .collect();
    let mut rounds = vec![opening];
    while rounds.last().is_some_and(|r| r.len() > 1) {
        let len = rounds.last().map_or(0, Vec::len) / 2;
        rounds.push(vec![Slot::default(); len]);
    }
    for j in 0..first {
        if rounds[0][j].bye {
            advance(&mut rounds, 0, j);
        }
    }
    rounds
}

/// Moves match `j` of round `r`'s winner into their place in the next round.
fn advance(rounds: &mut [Vec<Slot>], r: usize, j: usize) {
    let m = &rounds[r][j];
    let who = match m.winner {
        Some(0) => m.a,
        Some(1) => m.b,
        _ => None,
    };
    if let Some(next) = rounds.get_mut(r + 1).and_then(|round| round.get_mut(j / 2)) {
        if j % 2 == 0 {
            next.a = who;
        } else {
            next.b = who;
        }
    }
}

/// Whoever won the final, once it has been fought.
fn champion_of(rounds: &[Vec<Slot>]) -> Option<usize> {
    let last = rounds.last()?.first()?;
    match last.winner {
        Some(0) => last.a,
        Some(1) => last.b,
        _ => None,
    }
}

fn shuffle<T>(list: &mut [T], seed: &mut u64) {
    for i in (1..list.len()).rev() {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        list.swap(i, (*seed % (i as u64 + 1)) as usize);
    }
}

/// `/battlestop` - lets an admin free a channel whose battle died mid-fight,
/// which otherwise stays "busy" until the bot restarts.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only admins can clear the lists.".into())).await;
        return;
    }
    let arena = match command.guild_id {
        Some(guild) => arena(ctx, guild, command.channel_id).await,
        None => command.channel_id,
    };
    let freed = BUSY.lock().remove(&arena.get());
    LOBBIES.lock().clear();
    // A scroll left open would keep taking answers for a duel nobody is
    // watching, so the stuck duel's scrolls go with it.
    OPEN_SCROLLS.lock().clear();
    BELOW.lock().remove(&arena.get());
    let text = if freed {
        "Cleared. A fight that was still running will stop at its next step, and `/battle` works again."
    } else {
        "Nothing was running in the lists."
    };
    let _ = command.create_response(&ctx.http, whisper(text.into())).await;
}

// --- /fightboard ------------------------------------------------------------

/// Everyone who has fought, best win count first, with battles won alongside.
fn top_fighters(limit: usize) -> Vec<(u64, i64, i64, i64)> {
    let Some(db) = DB.get() else {
        return Vec::new();
    };
    let conn = db.lock();
    // Each fight puts its winner and its loser in the same pile, so one pass
    // counts both what someone won and how often they turned up.
    let rows: Vec<(i64, i64, i64)> = conn
        .prepare(
            "SELECT who, SUM(won) AS wins, COUNT(*) AS fights FROM (
                 SELECT winner AS who, 1 AS won FROM results WHERE kind != 'champion'
                 UNION ALL
                 SELECT loser AS who, 0 AS won FROM results WHERE kind != 'champion' AND loser IS NOT NULL)
             GROUP BY who ORDER BY wins DESC, fights ASC LIMIT ?1",
        )
        .and_then(|mut s| {
            s.query_map(params![limit as i64], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect()
        })
        .unwrap_or_default();
    rows.into_iter()
        .map(|(who, wins, fights)| {
            let crowns: i64 = conn
                .query_row("SELECT COUNT(*) FROM results WHERE kind = 'champion' AND winner = ?1", params![who], |r| {
                    r.get(0)
                })
                .unwrap_or(0);
            (who as u64, fights, wins, crowns)
        })
        .collect()
}

pub async fn board_command(ctx: &Context, command: &CommandInteraction) {
    let rows = top_fighters(10);
    let mut text = String::new();
    if rows.is_empty() {
        text.push_str("Nobody has ridden yet. Call someone out with `/fight @name`.");
    }
    for (i, (user, fights, wins, crowns)) in rows.iter().enumerate() {
        let place = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            _ => format!("`{:>2}.`", i + 1),
        };
        let crown = if *crowns > 0 { format!(" 👑×{}", crowns) } else { String::new() };
        text.push_str(&format!("{} <@{}>{} · **{}** won / {} fought\n", place, user, crown, wins, fights));
    }
    let (fights, wins) = tally(command.user.id.get());
    if fights > 0 {
        text.push_str(&format!("\n-# You: **{}** won out of **{}** fights", wins, fights));
    }
    let embed = CreateEmbed::new()
        .title("⚔️ The lists")
        .description(text)
        .colour(0xB0742A)
        .footer(CreateEmbedFooter::new("👑 = melees won · every pass counts, the melee's own included"));
    let reply = CreateInteractionResponseMessage::new().embed(embed);
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

// --- buttons ----------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if let Some(rest) = id.strip_prefix("battlejoin:") {
        join(ctx, component, rest).await;
    } else if let Some(rest) = id.strip_prefix(SCROLL_BUTTON) {
        on_scroll_button(ctx, component, rest).await;
    } else if let Some(rest) = id.strip_prefix("fightyes:") {
        answer(ctx, component, rest, true).await;
    } else if let Some(rest) = id.strip_prefix("fightno:") {
        answer(ctx, component, rest, false).await;
    }
}

async fn join(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Ok(lobby_id) = rest.parse::<u64>() else {
        return;
    };
    let user = component.user.id.get();
    let name = component
        .member
        .as_ref()
        .map(|m| display(m))
        .unwrap_or_else(|| component.user.name.clone());
    let (result, names, ends, minutes) = {
        let mut lobbies = LOBBIES.lock();
        let roster = |lobby: &Lobby| -> Vec<String> {
            lobby.joined.iter().filter_map(|u| lobby.names.get(u).cloned()).collect()
        };
        match lobbies.get_mut(&lobby_id) {
            None => ("closed", Vec::new(), 0, 0),
            Some(lobby) if !lobby.open => ("closed", Vec::new(), lobby.ends, lobby.minutes),
            Some(lobby) if lobby.joined.contains(&user) => ("already", roster(lobby), lobby.ends, lobby.minutes),
            Some(lobby) => {
                lobby.joined.push(user);
                lobby.names.insert(user, name);
                ("joined", roster(lobby), lobby.ends, lobby.minutes)
            }
        }
    };
    match result {
        "joined" => {
            let _ = component.create_response(&ctx.http, whisper("⚔️ You are in the lists. Get ready.")).await;
            // Everyone should see the roster fill up, countdown untouched.
            let mut message = component.message.clone();
            let _ = message.edit(&ctx.http, EditMessage::new().embed(lobby_embed(&names, ends, minutes))).await;
        }
        "already" => {
            let _ = component.create_response(&ctx.http, whisper("You are already in.")).await;
        }
        _ => {
            let _ = component.create_response(&ctx.http, whisper("That melee is closed.")).await;
        }
    }
}

async fn answer(ctx: &Context, component: &ComponentInteraction, rest: &str, yes: bool) {
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Ok(id) = rest.parse::<u64>() else {
        return;
    };
    let user = component.user.id.get();
    let allowed = {
        let mut challenges = CHALLENGES.lock();
        match challenges.get_mut(&id) {
            Some(challenge) if challenge.to == user => {
                challenge.accepted = Some(yes);
                true
            }
            _ => false,
        }
    };
    if allowed {
        let text = if yes { "⚔️ To the lists." } else { "🏃 Fine, backing out." };
        let _ = component.create_response(&ctx.http, whisper(text)).await;
    } else {
        let _ = component.create_response(&ctx.http, whisper("That challenge is not yours.")).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fill_both_names_and_the_rounds_read_right() {
        let line = fill("{a} ne {b} ko dhaal dikhayi", "Ravi", "Sneha");
        assert_eq!(line, "Ravi ne Sneha ko dhaal dikhayi");
        // One set of round names for the whole arena: the fight card's chip,
        // the bracket's columns and the caption all say the same thing.
        assert_eq!(round_title(1), "The Final Tilt");
        assert_eq!(round_title(2), "The Last Four");
        assert_eq!(round_title(4), "The Last Eight");
        assert_eq!(round_title(8), "Round of 16");
    }

    #[test]
    fn the_daily_battle_opens_once_in_its_window() {
        // 2026-09-14 21:00 India time is 15:30 UTC.
        let slot = 1_789_399_800;
        assert_eq!(super::super::points::ist_day(slot), "2026-09-14");
        let never = |_: &str, _: &str| false;
        assert_eq!(daily_due(slot - 60, "21:00", never), None, "not before the time");
        assert_eq!(daily_due(slot + 60, "21:00", never), Some(("2026-09-14".into(), "21:00".into())));
        assert_eq!(daily_due(slot + 60, "21:00", |d, t| d == "2026-09-14" && t == "21:00"), None, "once a day");
        assert_eq!(daily_due(slot + DAILY_GRACE_SECS + 1, "21:00", never), None, "too late after downtime");
        assert_eq!(daily_due(slot, "25:00", never), None);
        // Several times: the afternoon one is long past, the evening one is due.
        assert_eq!(daily_due(slot + 60, "14:00,21:00", never), Some(("2026-09-14".into(), "21:00".into())));
        assert_eq!(daily_due(slot - 7 * 3600 + 60, "14:00, 21:00", never), Some(("2026-09-14".into(), "14:00".into())));
        assert_eq!(Ping::from_key("everyone"), Ping::Everyone);
        // A word nobody recognises is the month's own role, not the houses.
        assert_eq!(Ping::from_key("anything"), Ping::Games);
        assert_eq!(Ping::from_key(""), Ping::Games);
    }

    #[test]
    fn the_draw_seats_everyone_once_and_byes_move_straight_on() {
        let mut seed = 77u64;
        for n in (MIN_PLAYERS..=40).chain([63, 64, 65, 200]) {
            let rounds = draw_bracket(n, &mut seed);
            let size = n.next_power_of_two();
            assert_eq!(rounds[0].len(), size / 2, "{} entrants", n);
            assert_eq!(rounds.last().map(Vec::len), Some(1));
            let mut seated: Vec<usize> = rounds[0].iter().flat_map(|m| [m.a, m.b]).flatten().collect();
            seated.sort_unstable();
            assert_eq!(seated, (0..n).collect::<Vec<_>>(), "{} entrants", n);
            let byes = rounds[0].iter().filter(|m| m.bye).count();
            assert_eq!(byes, size - n);
            if rounds.len() > 1 {
                for (j, m) in rounds[0].iter().enumerate() {
                    let next = &rounds[1][j / 2];
                    let placed = if j % 2 == 0 { next.a } else { next.b };
                    assert_eq!(placed, if m.bye { m.a } else { None }, "{} entrants, match {}", n, j);
                }
            }
        }
    }

    #[test]
    fn long_lists_split_under_the_limit_and_keep_every_item() {
        let items: Vec<String> = (0..300).map(|i| format!("🦁 **Fighter{}** beat 🦅 Other{} · 42 HP", i, i)).collect();
        let parts = chunk_list("⚡ **Round of 512** · quick round · 300 fights", &items, "\n", 1900);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.chars().count() <= 1900));
        let joined = parts.join("\n");
        for item in &items {
            assert!(joined.contains(item.as_str()), "lost {}", item);
        }
        assert!(parts[0].starts_with("⚡"));
    }

    #[test]
    fn a_quick_result_names_a_side_with_health_left() {
        let mut seed = 9u64;
        let mut left = 0;
        for _ in 0..2000 {
            let (side, hp) = quick_result(&mut seed);
            assert!(side < 2 && (1..=START_HP).contains(&hp));
            left += (side == 0) as usize;
        }
        assert!((850..1150).contains(&left), "left won {} of 2000", left);
    }

    #[test]
    fn winners_climb_the_bracket_to_a_champion() {
        let mut seed = 3u64;
        let mut rounds = draw_bracket(11, &mut seed);
        for r in 0..rounds.len() {
            for j in 0..rounds[r].len() {
                if rounds[r][j].bye {
                    continue;
                }
                assert!(rounds[r][j].a.is_some() && rounds[r][j].b.is_some(), "round {} match {} not filled", r, j);
                rounds[r][j].winner = Some(j % 2);
                advance(&mut rounds, r, j);
            }
        }
        assert!(champion_of(&rounds).is_some());
    }

    /// Play the exchange loop the way `play` does, without Discord in the way.
    fn simulate(seed: &mut u64) -> ([i32; 2], usize) {
        let (blows, hp) = roll_fight(seed);
        (hp, blows.len())
    }

    #[test]
    fn health_runs_out_in_a_reasonable_number_of_exchanges() {
        let mut seed = 7u64;
        let (mut knockouts, mut total) = (0, 0);
        for _ in 0..500 {
            let (hp, turns) = simulate(&mut seed);
            assert!(hp.iter().all(|h| (0..=START_HP).contains(h)), "health left the bar: {:?}", hp);
            assert!(turns <= MAX_EXCHANGES);
            assert!(hp[0] > 0 || hp[1] > 0, "both fighters can't be out");
            if hp.contains(&0) {
                knockouts += 1;
            }
            total += turns;
        }
        // Most fights should end with a knockout rather than on health left.
        assert!(knockouts > 400, "only {} knockouts in 500 fights", knockouts);
        let average = total as f64 / 500.0;
        assert!((8.0..17.0).contains(&average), "fights average {} exchanges", average);
    }

    #[test]
    fn every_twist_moves_the_right_side_by_the_right_amount() {
        let mut seen = std::collections::HashMap::new();
        let mut seed = 42u64;
        for _ in 0..6000 {
            let attacker = roll(&mut seed, 2) as usize;
            let other = 1 - attacker;
            let swing = swing(&mut seed, attacker);
            *seen.entry(swing.blow).or_insert(0) += 1;
            let (me, them) = (swing.hits[attacker], swing.hits[other]);
            match swing.blow {
                Blow::Miss => assert_eq!((me, them), (0, 0)),
                Blow::Sip => assert!((1..=3).contains(&me) && them == 0, "sip {:?}", swing.hits),
                Blow::Heal => assert!((6..=12).contains(&me) && them == 0, "heal {:?}", swing.hits),
                // The swing comes back at whoever threw it.
                Blow::Backfire => assert!((-18..=-10).contains(&me) && them == 0, "backfire {:?}", swing.hits),
                // The only move that takes from one side and gives to the other.
                Blow::Drain => assert!(
                    (5..=9).contains(&me) && (-24..=-16).contains(&them),
                    "drain {:?}",
                    swing.hits
                ),
                Blow::Double => assert!((-38..=-28).contains(&them) && me == 0, "double {:?}", swing.hits),
                Blow::Crit => assert!((-42..=-32).contains(&them) && me == 0, "crit {:?}", swing.hits),
                Blow::Snack => assert!((10..=18).contains(&me) && them == 0, "snack {:?}", swing.hits),
                Blow::Blessing => assert!((20..=30).contains(&me) && them == 0, "blessing {:?}", swing.hits),
                // The one turn that is kind to everybody.
                Blow::Crowd => assert!(me == them && (4..=8).contains(&me), "crowd {:?}", swing.hits),
                Blow::Chaos => assert!(
                    me == them && (-14..=-8).contains(&me),
                    "chaos should hurt both equally {:?}",
                    swing.hits
                ),
                Blow::Hit => assert!((-28..=-18).contains(&them) && me == 0, "hit {:?}", swing.hits),
            }
            // Only the crowd is generous to the other side.
            if swing.blow != Blow::Crowd {
                assert!(them <= 0, "{:?} healed the other side: {:?}", swing.blow, swing.hits);
            }
        }
        // Every twist should actually turn up, and plain trades stay the norm.
        for blow in [
            Blow::Miss, Blow::Sip, Blow::Backfire, Blow::Drain, Blow::Double, Blow::Crit, Blow::Chaos, Blow::Heal,
            Blow::Snack, Blow::Blessing, Blow::Crowd, Blow::Hit,
        ] {
            assert!(seen.get(&blow).copied().unwrap_or(0) > 80, "{:?} barely happens: {:?}", blow, seen.get(&blow));
        }
        assert!(seen[&Blow::Hit] > 1800, "plain hits should still be the most common: {}", seen[&Blow::Hit]);
        let healed: i32 = [Blow::Sip, Blow::Heal, Blow::Snack, Blow::Blessing, Blow::Crowd]
            .iter()
            .map(|blow| seen.get(blow).copied().unwrap_or(0))
            .sum();
        assert!((900..1600).contains(&healed), "about a fifth of turns should give health back: {}", healed);
    }

    #[test]
    fn the_tail_reads_the_numbers_out() {
        let swing = |hits| Swing { blow: Blow::Hit, hits };
        assert_eq!(swing([0, -24]).tail(), "-24 HP");
        assert_eq!(swing([7, -18]).tail(), "+7 HP · -18 HP");
        assert_eq!(swing([0, 0]).tail(), "no damage");
    }

    #[test]
    fn chat_is_only_counted_while_a_fight_is_live() {
        let channel = ChannelId::new(999);
        // No fight: the arena ignores the channel entirely.
        assert!(!note_chat(channel));
        BELOW.lock().insert(channel.get(), 0);
        for _ in 0..STICKY_AFTER {
            assert!(note_chat(channel));
        }
        assert_eq!(BELOW.lock().get(&channel.get()).copied(), Some(STICKY_AFTER));
        // The fight ends and the counter goes with it.
        BELOW.lock().remove(&channel.get());
        assert!(!note_chat(channel));
    }

    #[test]
    fn health_bars_fill_and_empty_with_the_numbers() {
        assert_eq!(bar(100), "█".repeat(BAR_BLOCKS));
        assert_eq!(bar(0), "░".repeat(BAR_BLOCKS));
        assert_eq!(bar(50).chars().filter(|c| *c == '█').count(), BAR_BLOCKS / 2);
        // One block left while alive, none once out, and always the same width.
        assert_eq!(bar(1).chars().filter(|c| *c == '█').count(), 1);
        for hp in 0..=START_HP {
            assert_eq!(bar(hp).chars().count(), BAR_BLOCKS, "hp {}", hp);
        }
    }

    #[test]
    fn fight_text_shows_both_bars_and_only_the_last_few_lines() {
        let fighter = |id: u64, name: &str| Contender {
            id,
            name: name.to_string(),
            avatar: None,
            house: None,
            stage: 1,
            face: String::new(),
        };
        let (a, b) = (fighter(1, "Ravi"), fighter(2, "Sneha"));
        let log: Vec<String> = (1..=6).map(|i| format!("line {}", i)).collect();
        let text = fight_text("head", &log, &a, &b, &[62, 0]);
        assert!(text.contains("Ravi") && text.contains("Sneha"), "{}", text);
        assert!(text.contains(&bar(62)) && text.contains(&bar(0)), "{}", text);
        assert!(text.contains("**  0**"), "the one who is out shows zero: {}", text);
        assert!(text.contains("line 6") && text.contains("line 4"), "{}", text);
        assert!(!text.contains("line 3"), "{}", text);
    }

    #[test]
    fn shuffle_keeps_everyone_and_pairs_leave_one_out_when_odd() {
        let fighters = |n: usize| {
            (0..n)
                .map(|i| Contender {
                    id: i as u64,
                    name: format!("w{}", i),
                    avatar: None,
                    house: None,
                    stage: 1,
                    face: String::new(),
                })
                .collect::<Vec<_>>()
        };
        let mut list = fighters(9);
        let mut seed = 99u64;
        shuffle(&mut list, &mut seed);
        let ids: std::collections::HashSet<u64> = list.iter().map(|w| w.id).collect();
        assert_eq!(ids.len(), 9);
        let chunks: Vec<usize> = list.chunks(2).map(|c| c.len()).collect();
        assert_eq!(chunks.iter().filter(|n| **n == 1).count(), 1, "odd rounds need exactly one bye");
    }



    // --- what survives a restart ---------------------------------------------

    /// A battle.db written before any of this - before the scrolls, before the
    /// styles went - still opens, still reads, and comes back with the puzzle
    /// table added. battle.db is a process-wide handle, so this is the one test
    /// that opens it.
    #[test]
    fn an_old_store_still_opens_and_reads() {
        let dir = std::env::temp_dir().join(format!("vizier-arena-{}", std::process::id()));
        let runtime = dir.join(".runtime");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&runtime).expect("a workspace");
        {
            let conn = Connection::open(runtime.join("battle.db")).expect("the old store");
            conn.execute_batch(
                "CREATE TABLE wins (
                     id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, kind TEXT NOT NULL,
                     beat INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL);
                 CREATE TABLE results (
                     id INTEGER PRIMARY KEY AUTOINCREMENT, kind TEXT NOT NULL, winner INTEGER NOT NULL,
                     loser INTEGER, ts INTEGER NOT NULL);
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO results (kind, winner, loser, ts) VALUES
                     ('fight', 7, 8, 1000), ('fight', 8, 7, 1100), ('battle', 7, 9, 1200),
                     ('champion', 7, NULL, 1300);
                 -- A store that still remembers the day's melee, and a fight
                 -- style setting nobody reads any more.
                 INSERT INTO meta (key, value) VALUES
                     ('daily_battle_day', '2026-09-14'), ('champion', '7'), ('battle_theme', 'pokemon');",
            )
            .expect("the old rows");
        }
        open(dir.to_str().expect("a path")).expect("an old store still opens");

        // Everything it recorded is still readable, with nothing about styles.
        assert_eq!(tally(7), (3, 2), "two wins from three fights");
        assert_eq!(tally(8), (2, 1));
        assert_eq!(crowns(7), 1);
        let per_user = wins_per_user(None);
        assert_eq!(per_user.get(&7).copied(), Some(3));
        assert_eq!(per_user.get(&8).copied(), Some(1));
        assert_eq!(duels_since(0).len(), 3, "the champion row is nobody's duel");
        assert_eq!(meta_get("champion").as_deref(), Some("7"));

        // The day a melee already opened is still remembered, so a restart
        // neither re-opens it nor forgets it.
        assert_eq!(meta_get("daily_battle_day").as_deref(), Some("2026-09-14"));
        meta_set("daily_battle:2026-09-15:21:00", "opened");
        assert_eq!(meta_get("daily_battle:2026-09-15:21:00").as_deref(), Some("opened"));

        // The old style setting is still sitting there and nothing reads it.
        assert_eq!(meta_get("battle_theme").as_deref(), Some("pokemon"));

        // And the scrolls' own table was added on the way in, so a duel fought
        // after the upgrade has somewhere to write its puzzles.
        let mut rng = battle_scroll::Rng::new(5);
        let puzzle = battle_scroll::generate(&mut rng, &[]);
        let id = with_db(|conn| battle_scroll::record(conn, &puzzle, 1_700_000_000))
            .expect("the store is open")
            .expect("a scroll is written down");
        assert!(id > 0);
        let back = with_db(|conn| battle_scroll::read(conn, id)).flatten().expect("read back");
        assert_eq!(back.answer, puzzle.answer);

        // The duels in it count towards the anti-farm rule from the day they
        // were fought, not from today.
        let day = ist_midnight(1_000) + 3_600;
        assert_eq!(duels_today(7, 8, day), 2, "both of that pair's duels, that day");
        assert_eq!(duels_today(7, 9, day), 0, "a melee pass is not a duel");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A melee that was due while the bot was down opens when it comes back,
    /// and one that already ran does not run again. This is what "resumes
    /// after a restart" means for the arena: a lobby and a fight in progress
    /// live in memory and are lost, by design, but the schedule is not.
    #[test]
    fn the_schedule_survives_a_restart() {
        // 2026-09-14 21:00 India time.
        let slot = 1_789_399_800;
        let never = |_: &str, _: &str| false;
        // The bot was down at 21:00 and comes back ten minutes later: the
        // melee still opens.
        assert_eq!(daily_due(slot + 600, "21:00", never), Some(("2026-09-14".into(), "21:00".into())));
        // It comes back an hour later: too late, and the day is let go rather
        // than a melee turning up at a time nobody expects.
        assert_eq!(daily_due(slot + DAILY_GRACE_SECS + 1, "21:00", never), None);
        // It comes back having already run that slot: it does not run twice.
        let ran = |d: &str, t: &str| d == "2026-09-14" && t == "21:00";
        assert_eq!(daily_due(slot + 600, "21:00", ran), None);
        // And a second slot that day is still its own.
        assert_eq!(daily_due(slot + 600, "21:00,21:05", ran), Some(("2026-09-14".into(), "21:05".into())));
    }

    // --- the types are gone ---------------------------------------------------

    /// Every file the arena owns, read at test time - the code only, with the
    /// test module cut off. A test that bans a word has to name it, and a scan
    /// that read its own banned list would find every word in it.
    fn arena_sources() -> Vec<(String, String)> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/channels/discord");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("discord sources").flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
            if name.starts_with("battle") && name.ends_with(".rs") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let code = text.split("#[cfg(test)]").next().unwrap_or("").to_string();
                assert!(code.len() > 500, "{} is all tests?", name);
                out.push((name, code));
            }
        }
        assert!(out.len() >= 5, "only found {:?}", out.iter().map(|(n, _)| n).collect::<Vec<_>>());
        out
    }

    /// The fight styles are gone: no enum, no picker, no setting, no leftover
    /// branch. A file that still names one of the seven worlds is the system
    /// growing back.
    #[test]
    fn nothing_in_the_arena_knows_what_a_fight_style_is() {
        // Named precisely: the month paints the houses through `month::themed`,
        // so a bare "theme" is a word the arena is allowed to say.
        let banned = [
            "battle_theme", "Theme::", "enum Theme", "pokemon", "Pokemon", "harrypotter", "eldenring",
            "Tarnished", "Saiyan", "Wrestling", "Tactical", "VIZIER_BATTLE_DAILY_THEME",
            "theme_command_option", "daily_theme", "fight style", "fight type", "fight styles",
        ];
        for (name, text) in arena_sources() {
            for word in banned {
                assert!(!text.contains(word), "{} still knows about \u{201c}{}\u{201d}", name, word);
            }
        }
    }

    /// The clash buttons went with them: a duel is read, not pressed.
    #[test]
    fn nothing_in_the_arena_remembers_the_clash_buttons() {
        let banned = ["Clash", "pick_moves", "on_pick", "script_fight", "fightpick", "VIZIER_FIGHT_PICK_SECS"];
        for (name, text) in arena_sources() {
            for word in banned {
                assert!(!text.contains(word), "{} still has {}", name, word);
            }
        }
    }

    /// And so did the Contender role: the games role is the only one the arena
    /// ever mentions.
    #[test]
    fn the_arena_mentions_no_role_but_the_games_one() {
        for (name, text) in arena_sources() {
            for word in ["WARRIOR_ROLE", "warrior_role", "toggle_warrior", "battlewarrior"] {
                assert!(!text.contains(word), "{} still has {}", name, word);
            }
        }
        assert_eq!(Ping::from_key("warriors"), Ping::Games, "an old setting value falls to the games role");
        assert!(Ping::parse("warriors").is_err(), "the Warrior role is not something to tag any more");
        assert_eq!(GAMES_ROLE, 1_554_720_685_514_035_241);
    }

    /// A whole fight, rolled with no types anywhere in it, still resolves - and
    /// nothing it says names one.
    #[test]
    fn a_fight_with_no_types_resolves_and_says_nothing_about_one() {
        let mut seed = 2024u64;
        let lines = lines();
        for _ in 0..300 {
            let (blows, hp) = roll_fight(&mut seed);
            assert!(!blows.is_empty(), "a fight with no types still has to happen");
            assert!(hp[0] > 0 || hp[1] > 0, "somebody is left standing");
            for (attacker, swing) in &blows {
                let (x, y) = if *attacker == 0 { ("Ravi", "Sneha") } else { ("Sneha", "Ravi") };
                let said = fill(pick(swing.blow.lines(lines), &mut seed), x, y);
                assert!(!said.contains('{'), "a line kept its placeholder: {}", said);
                let lower = said.to_lowercase();
                for word in ["style", "theme", "pokemon", "classic", " type"] {
                    assert!(!lower.contains(word), "a fight line names a style: {}", said);
                }
            }
        }
    }

    // --- the two seasons ------------------------------------------------------

    /// The hatch is the one thing that moves the arena from eggs to houses, and
    /// it is the month's own moment rather than a date of the arena's.
    #[test]
    fn the_season_follows_the_months_hatch() {
        let at = super::super::month::hatch_at();
        assert!(at > 0, "the month has to know when the eggs open");
        assert!(!hatched_at(at - 1), "a second before the hatch is still the egg week");
        assert!(hatched_at(at), "the hatch moment is the hatch");
        assert!(hatched_at(at + 86_400), "and it stays hatched");
        assert!(!hatched_at(0));
        // The season follows it, and nothing else does.
        assert_eq!(if hatched_at(at - 1) { Season::Houses } else { Season::Eggs }, Season::Eggs);
        assert_eq!(if hatched_at(at) { Season::Houses } else { Season::Eggs }, Season::Houses);
        assert!(!Season::Eggs.houses() && Season::Houses.houses());
    }

    /// The egg warms with the points behind it, and nothing outside the range
    /// can make it a stage that does not exist. It is paint only: nothing in a
    /// fight reads it.
    #[test]
    fn an_egg_warms_with_the_points_behind_it() {
        assert_eq!(egg_stage(-50), 1);
        assert_eq!(egg_stage(0), 1);
        assert_eq!(egg_stage(1), 2);
        assert_eq!(egg_stage(9), 2);
        assert_eq!(egg_stage(10), 3);
        assert_eq!(egg_stage(29), 3);
        assert_eq!(egg_stage(30), 4);
        assert_eq!(egg_stage(69), 4);
        assert_eq!(egg_stage(70), EGG_STAGES);
        assert_eq!(egg_stage(i64::MAX), EGG_STAGES);
        for points in [-1_000i64, 0, 7, 44, 1_000_000] {
            assert!((1..=EGG_STAGES).contains(&egg_stage(points)), "{}", points);
        }
    }

    /// The paint on a banner comes from the month and nowhere else: the arena
    /// keeps no second table of names, crests or colours.
    #[test]
    fn a_banner_is_painted_from_the_months_own_colours() {
        let painted: Vec<HouseLook> = super::super::house::HOUSES.iter().map(house_look).collect();
        assert_eq!(painted.len(), 4);
        for (house, look) in super::super::house::HOUSES.iter().zip(&painted) {
            let themed = super::super::month::themed(house);
            assert_eq!(look.name, themed.name, "the arena renamed a house behind the month's back");
            assert_eq!(look.crest, themed.crest);
            assert_eq!(look.key, house.key, "the ledger key never moves");
            // The banner's field is the month's colour, and its trim the same
            // colour lifted - never a second palette.
            let c = themed.colour;
            assert_eq!(look.colours.0, [(c >> 16) as u8, (c >> 8) as u8, c as u8]);
            assert_ne!(look.colours.0, look.colours.1, "{}'s banner is one flat colour", look.name);
            // The mark is the first letter of whatever the month calls it.
            let first = look.name.chars().find(|c| c.is_alphabetic()).unwrap().to_uppercase().to_string();
            assert_eq!(look.initial, first);
        }
        // No table of the four anywhere in the arena's own source.
        for (name, text) in arena_sources() {
            assert!(!text.contains("MONTH_PAINT"), "{} keeps a second set of paint", name);
        }
    }

    // --- who gets called to the lists ----------------------------------------

    #[test]
    fn a_melee_calls_the_games_role_and_a_duel_calls_nobody() {
        let games = RoleId::new(GAMES_ROLE);
        let houses = vec![RoleId::new(11), RoleId::new(12), RoleId::new(13), RoleId::new(14)];
        let tags = Tags { games: Some(games), houses: houses.clone() };
        let (text, roles, everyone) = heads_up(Ping::Games, &tags).expect("a melee calls somebody");
        assert!(text.contains(&format!("<@&{}>", GAMES_ROLE)), "{}", text);
        assert_eq!(roles, vec![games], "the games role, and nothing else");
        assert!(!everyone);
        assert!(text.contains("melee"), "{}", text);
        // Nobody is tagged for a lobby that asked for nobody.
        assert_eq!(heads_up(Ping::Nobody, &tags), None);
        // @everyone is its own thing and mentions no role.
        let (text, roles, everyone) = heads_up(Ping::Everyone, &tags).expect("everyone");
        assert_eq!((text.contains("@everyone"), roles.is_empty(), everyone), (true, true, true));
        // The houses tag their four.
        let (_, roles, _) = heads_up(Ping::Houses, &tags).expect("houses");
        assert_eq!(roles, houses);
    }

    /// A role the server hasn't got means no heads-up at all - the lobby still
    /// goes up, it simply goes up quietly.
    #[test]
    fn a_missing_games_role_posts_the_lobby_without_a_ping() {
        let nothing = Tags::default();
        assert_eq!(heads_up(Ping::Games, &nothing), None, "a melee with no role to call goes up untagged");
        assert_eq!(heads_up(Ping::Houses, &nothing), None, "and so does one with no houses to call");
        // @everyone needs no role, so it still goes out.
        assert!(heads_up(Ping::Everyone, &nothing).is_some());
    }

    /// The setting decides which role, and switching it off is a real option.
    #[test]
    fn the_games_role_can_be_moved_or_switched_off() {
        // Nothing stored: the month's own role.
        assert_eq!(games_role_from(None), Some(GAMES_ROLE));
        for (set, want) in [
            ("", Some(GAMES_ROLE)),
            ("   ", Some(GAMES_ROLE)),
            ("0", None),
            ("none", None),
            ("NONE", None),
            (" None ", None),
            ("42", Some(42)),
            ("not a role", Some(GAMES_ROLE)),
        ] {
            assert_eq!(games_role_from(Some(set)), want, "set to \u{201c}{}\u{201d}", set);
        }
    }

    // --- the scrolls ----------------------------------------------------------

    /// Three scrolls, first to two; a fourth only to break a tie somebody
    /// actually scored in.
    #[test]
    fn a_duel_is_best_of_three_with_a_decider_for_a_real_tie() {
        assert!(another_scroll(0, [0, 0]), "a duel starts");
        assert!(another_scroll(1, [1, 0]));
        assert!(another_scroll(2, [1, 0]));
        // Two scrolls takes it, whenever that happens.
        assert!(!another_scroll(2, [2, 0]));
        assert!(!another_scroll(3, [1, 2]));
        // One each after three: a fourth decides it.
        assert!(another_scroll(3, [1, 1]), "1-1 after three needs a decider");
        assert!(!another_scroll(4, [1, 1]), "and only one decider");
        // Every scroll burned: the gods decide, no fourth scroll.
        assert!(!another_scroll(3, [0, 0]), "all three burning is a coin flip, not a fourth scroll");
        // A duel can never run away: at most four scrolls, whatever the score.
        for done in 0..8usize {
            for a in 0..3u32 {
                for b in 0..3u32 {
                    assert!(!(done >= 4 && another_scroll(done, [a, b])), "{} scrolls at {}-{}", done, a, b);
                }
            }
        }
    }

    #[test]
    fn whoever_read_the_most_scrolls_takes_the_duel() {
        assert_eq!(duel_winner([2, 0]), Some(0));
        assert_eq!(duel_winner([1, 2]), Some(1));
        assert_eq!(duel_winner([1, 0]), Some(0), "a burned decider still leaves a winner");
        // Level means nobody read anything: the gods decide, and the duel says so.
        assert_eq!(duel_winner([0, 0]), None);
        assert_eq!(duel_winner([1, 1]), None);
    }

    /// A whole duel played out without Discord: the scrolls roll, nothing
    /// repeats, the blows are real, and somebody is left standing.
    #[test]
    fn a_duel_runs_end_to_end_and_lands_real_blows() {
        for seed in 1..200u64 {
            let mut rng = battle_scroll::Rng::new(seed * 2_654_435_761);
            let mut blow_seed = seed | 1;
            let mut hp = [START_HP; 2];
            let mut score = [0u32; 2];
            let mut used: Vec<&'static str> = Vec::new();
            let mut done = 0usize;
            while another_scroll(done, score) {
                done += 1;
                let puzzle = battle_scroll::generate(&mut rng, &used);
                assert!(!used.contains(&puzzle.kind), "{} twice in one duel", puzzle.kind);
                used.push(puzzle.kind);
                // Somebody reads it four times in five; the fifth burns.
                let read = rng.upto(5) < 4;
                if read {
                    let side = rng.upto(2) as usize;
                    let blow = SCROLL_BLOW.0 + roll(&mut blow_seed, SCROLL_BLOW.1) as i32;
                    assert!((25..=35).contains(&blow), "a scroll's blow was {}", blow);
                    hp[1 - side] = (hp[1 - side] - blow).max(0);
                    score[side] += 1;
                }
            }
            assert!((1..=4).contains(&done), "a duel ran {} scrolls", done);
            assert!(hp.iter().all(|h| (0..=START_HP).contains(h)), "health left the bar: {:?}", hp);
            // Two scrolls at thirty-odd each never empties a bar on its own, so
            // the winner is always left standing on real health.
            let side = duel_winner(score).unwrap_or(0);
            assert!(hp[side] > 0, "the winner of {:?} was left on {:?}", score, hp);
        }
    }

    /// The scroll's own clock and lockout: both start at the card, and a wrong
    /// answer costs only the one who gave it.
    #[test]
    fn a_wrong_answer_locks_out_only_the_one_who_gave_it() {
        let puzzle = battle_scroll::Puzzle {
            id: 0,
            riddle: String::new(),
            kind: "count_swords",
            mode: battle_scroll::Mode::Visual,
            prompt: "How many?".into(),
            answer: "7".into(),
            alts: vec!["seven".into()],
            spec: battle_scroll::Spec::default(),
        };
        let mut live = Live {
            fighters: [1, 2],
            puzzle,
            won_by: None,
            opened: std::time::Instant::now(),
            locked: [None, None],
            open: true,
        };
        assert_eq!(locked_for(&live, 0), None, "nobody starts locked out");
        live.locked[0] = Some(std::time::Instant::now());
        let left = locked_for(&live, 0).expect("a wrong answer costs six seconds");
        assert!(left <= SCROLL_LOCKOUT && left > Duration::from_secs(4), "{:?}", left);
        assert_eq!(locked_for(&live, 1), None, "the other fighter is not held up by it");
        // A lockout that has run out is no lockout.
        live.locked[0] = Some(std::time::Instant::now() - SCROLL_LOCKOUT * 2);
        assert_eq!(locked_for(&live, 0), None);
        assert_eq!(SCROLL_LOCKOUT, Duration::from_secs(6));
        assert_eq!((SCROLLS, TO_WIN), (3, 2));
    }

    // --- what a duel pays -----------------------------------------------------

    #[test]
    fn the_result_says_what_the_win_paid_and_why_it_did_not() {
        assert_eq!(Paid::Points(3).said(), " (+3 house points)");
        let enough = Paid::Enough.said();
        assert!(enough.contains("no points") && enough.contains("fought enough today"), "{}", enough);
        assert_eq!(Paid::Nothing.said(), "", "points switched off is not worth a sentence");
    }

    #[test]
    fn india_days_start_where_the_ledger_says_they_do() {
        // 2026-09-14 21:00 India time.
        let slot = 1_789_399_800;
        let midnight = ist_midnight(slot);
        assert!(midnight <= slot && slot - midnight < 86_400);
        assert_eq!(super::super::points::ist_day(midnight), "2026-09-14");
        assert_eq!(super::super::points::ist_day(midnight + 86_399), "2026-09-14");
        assert_eq!(ist_midnight(midnight), midnight, "midnight is its own midnight");
        assert_eq!(ist_midnight(midnight + 86_400), midnight + 86_400);
    }

    // --- "Call a melee now", from the panel ----------------------------------

    /// A channel of its own per test: BUSY is shared by the whole process.
    fn spare_arena(n: u64) -> ChannelId {
        ChannelId::new(7_000_000 + n)
    }

    #[test]
    fn a_panel_start_calls_the_games_role() {
        let arena = spare_arena(1);
        let (plan, ping) = start_plan(arena, None, None).expect("nothing in the way");
        assert_eq!(ping, Ping::Games, "a panel start calls the people who opted in");
        assert_eq!((plan.ping, plan.ping_label), ("games", "the server games role"));
        assert_eq!(plan.channel, arena.get());
        assert_eq!(plan.minutes, default_lobby_minutes());
        free_arena(arena);
    }

    #[test]
    fn a_panel_start_reads_who_to_tag_and_refuses_anyone_else() {
        for (asked, want, label) in [
            ("games", Ping::Games, "the server games role"),
            ("houses", Ping::Houses, "the four houses"),
            ("  Everyone ", Ping::Everyone, "everyone"),
            ("none", Ping::Nobody, "nobody"),
            ("nobody", Ping::Nobody, "nobody"),
        ] {
            assert_eq!(Ping::parse(asked), Ok(want), "{}", asked);
            assert_eq!(want.label(), label);
            assert_eq!(Ping::parse(want.key()), Ok(want), "{} survives the round trip", asked);
        }
        let refused = Ping::parse("the mods").expect_err("an unknown word is never a silent @everyone");
        assert!(refused.contains("the mods") && refused.contains("games role"), "{}", refused);
        // Blank means "not said", which is the games role, not a refusal.
        let arena = spare_arena(2);
        let (plan, ping) = start_plan(arena, None, Some("   ")).expect("nothing in the way");
        assert_eq!((ping, plan.ping), (Ping::Games, "games"));
        free_arena(arena);
    }

    #[test]
    fn a_panel_start_keeps_the_lobby_between_one_minute_and_the_longest_allowed() {
        let arena = spare_arena(3);
        let minutes = |asked| {
            let (plan, _) = start_plan(arena, asked, None).expect("nothing in the way");
            free_arena(arena);
            plan.minutes
        };
        assert_eq!(minutes(Some(7)), 7);
        assert_eq!(minutes(Some(0)), MIN_WAIT, "no zero-minute lobby nobody can join");
        assert_eq!(minutes(Some(-99)), MIN_WAIT);
        assert_eq!(minutes(Some(9_999)), max_lobby_minutes(), "never longer than the longest allowed");
        assert_eq!(minutes(None), default_lobby_minutes());
        assert!((MIN_WAIT..=max_lobby_minutes()).contains(&minutes(None)));
    }

    #[test]
    fn a_second_panel_start_is_refused_while_the_arena_is_busy() {
        let arena = spare_arena(5);
        let (first, _) = start_plan(arena, None, None).expect("the arena was free");
        assert_eq!(first.channel, arena.get());
        let busy = start_plan(arena, None, None).expect_err("one fight at a time");
        assert_eq!(busy, ARENA_BUSY);
        assert!(busy.contains("/battlestop"), "it says how to clear a stuck one: {}", busy);
        // A second arena is untouched by the first one being busy.
        let other = spare_arena(6);
        assert!(start_plan(other, None, None).is_ok());
        free_arena(other);
        // Only once the lobby gives the claim back does the arena open again.
        free_arena(arena);
        assert!(start_plan(arena, None, None).is_ok(), "free again");
        free_arena(arena);
    }

    #[test]
    fn a_refused_panel_start_leaves_the_arena_free() {
        let arena = spare_arena(7);
        // Everything is settled after the arena is claimed, so a refusal there
        // must give the claim back or nothing could ever fight in it again.
        assert!(start_plan(arena, None, Some("the mods")).is_err());
        assert!(!BUSY.lock().contains(&arena.get()), "a refused start never wedges the arena");
        assert!(start_plan(arena, None, Some("games")).is_ok(), "and the next start works");
        free_arena(arena);
        assert!(!BUSY.lock().contains(&arena.get()));
    }

    #[test]
    fn the_panel_says_in_plain_english_why_it_could_not_start_one() {
        assert!(NO_GUILD.contains("server"), "{}", NO_GUILD);
        // The one an admin can actually fix names the setting and the channel.
        assert!(NO_ARENA.contains("Fight channel") && NO_ARENA.contains("#fight-fight-fight"), "{}", NO_ARENA);
        for line in [NO_GUILD, NO_ARENA, ARENA_BUSY] {
            assert!(line.ends_with('.') && !line.contains("VIZIER_"), "no settings jargon: {}", line);
        }
    }
}
