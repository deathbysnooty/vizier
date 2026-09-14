//! /fight and /battle - the arena.
//!
//! `/fight @someone` works in any channel: the challenge card goes up in the
//! fight channel and the caller's channel gets a link to it. `/battle` is for
//! admins, opens a lobby for as many minutes as they ask for, pings the warrior
//! role, and then knocks the joiners out in pairs until one is left. Winners are
//! a coin toss - this is banter, not a ladder - and the last one standing wears
//! the champion role until the next battle.
//!
//! Fights live in memory; only results go to battle.db, so a restart loses an
//! open lobby but never the record of who won.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditAttachments, EditMessage, EditRole,
    GuildId, Member, Message, MessageId, RoleId, UserId,
};

use super::battle_card::{self, Champion, Fight, Fighter, Outcome};
use super::battle_bracket::{self, Bracket, Entrant, Slot, round_title};
use super::battle_theme::{Lines, Theme};

/// Fight channel, from `VIZIER_FIGHT_CHANNEL`; otherwise found by name.
const CHANNEL_NAME: &str = "fight-fight-fight";
/// Pinged when a battle opens. Created if the server has no such role.
const WARRIOR_ROLE: &str = "Warrior";
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
const MIN_WAIT: i64 = 1;
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
    theme: Theme,
}

struct Challenge {
    from: u64,
    to: u64,
    accepted: Option<bool>,
}

/// A fighter with everything the card needs.
#[derive(Clone)]
struct Warrior {
    id: u64,
    name: String,
    avatar: Option<Vec<u8>>,
    house: Option<&'static super::house::House>,
    /// Where their picture is, for fetching it later.
    face: String,
}

impl Warrior {
    fn card(&self, hp: i32) -> Fighter {
        Fighter {
            name: self.name.clone(),
            avatar: self.avatar.clone(),
            hp: hp.max(0) as u32,
            max_hp: START_HP as u32,
            house: self.house,
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
    let _ = DB.set(Mutex::new(conn));
    Ok(())
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

/// Logs one finished fight. `loser` is `None` for a battle championship, where
/// the whole field lost rather than one person.
fn record(kind: &str, winner: u64, loser: Option<u64>) {
    let now = Utc::now().timestamp();
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO results (kind, winner, loser, ts) VALUES (?1, ?2, ?3, ?4)",
            params![kind, winner as i64, loser.map(|u| u as i64), now],
        );
    }
    // House points for a 1v1 challenge: 1 to the winner. The dedupe key is the
    // PAIR and the day, not the winner, so two friends fighting over and over earn
    // only their first fight of the day - whoever wins it. Fights inside a battle
    // royale score through `award_royale` instead.
    if let ("fight", Some(loser)) = (kind, loser) {
        let (low, high) = if winner < loser { (winner, loser) } else { (loser, winner) };
        let key = format!("arena:{}:{}:{}", super::points::ist_day(now), low, high);
        let points = super::control::number("VIZIER_POINTS_ARENA_WIN", 1) as i64;
        if points > 0 {
            super::house::award_person(winner, super::points::Source::Arena, points, "won a 1v1", None, Some(key), None);
        }
    }
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

async fn warrior_role(ctx: &Context, guild: GuildId) -> Option<RoleId> {
    role_named(ctx, guild, WARRIOR_ROLE, 0xE67E22, "warrior_role").await
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
async fn warrior(ctx: &Context, guild: GuildId, user: u64) -> Option<Warrior> {
    let mut w = warrior_named(ctx, guild, user).await?;
    w.avatar = picture(&w.face).await;
    Some(w)
}

/// A member ready to fight, without their picture yet: a big royale only needs
/// pictures for the last sixteen.
async fn warrior_named(ctx: &Context, guild: GuildId, user: u64) -> Option<Warrior> {
    // The cache guard is let go before any await.
    let cached = ctx.cache.guild(guild).and_then(|g| g.members.get(&UserId::new(user)).cloned());
    let member = match cached {
        Some(m) => m,
        None => guild.member(&ctx.http, UserId::new(user)).await.ok()?,
    };
    let face = member.face().replace("size=1024", "size=256");
    // Stepped-out members fight without a badge, as they asked to be left out.
    let house = if super::house::opted_out(user) { None } else { super::house::house_of(user) };
    Some(Warrior { id: user, name: display(&member), avatar: None, house, face })
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
    theme: Theme,
) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        battle_card::fight_png(&Fight { stage, left: &left, right: &right, line, outcome, hit, theme })
    })
    .await
    .ok()
    .flatten()
}

async fn champion_card(who: Fighter, subtitle: String, line: String, theme: Theme) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || battle_card::champion_png(&Champion { who: &who, subtitle, line, theme }))
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

/// One fight: the card goes up, the exchanges land under it, then the result.
/// With `picks`, both fighters choose a move first and the clash decides the
/// fight; without, every blow is a coin toss as before. Returns the winner.
#[allow(clippy::too_many_arguments)]
async fn play(
    ctx: &Context,
    channel: ChannelId,
    stage: &str,
    a: &Warrior,
    b: &Warrior,
    seed: &mut u64,
    theme: Theme,
    picks: bool,
) -> (Warrior, i32) {
    let lines = theme.lines();
    let mut hp = [START_HP; 2];
    // One picture at the start, one at the end: the blow-by-blow rides on the
    // text, which edits without an upload.
    let opening =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), String::new(), Outcome::Open, None, theme).await;
    let head = format!("**{}** · <@{}> vs <@{}>", stage, a.id, b.id);
    let mut log: Vec<String> = Vec::new();
    // The clash table is dealt once, so a fighter can learn it as the fight goes.
    let clash = picks.then(|| Clash::deal(seed));
    let fight_id = if picks { roll(seed, u64::MAX >> 12) + 1 } else { 0 };
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

    // With picks, one clash before the first blow decides the fight. The blows
    // that follow are dealt from a script that ends with the clash winner standing.
    let mut decided: Option<usize> = None;
    let mut script: std::collections::VecDeque<(usize, Swing)> = std::collections::VecDeque::new();
    if let Some(table) = &clash {
        let chosen = pick_moves(ctx, channel, &mut message, &head, &log, a, b, &hp, fight_id, opening.as_ref(), seed).await;
        PICKS.lock().remove(&fight_id);
        let side = table.winner(chosen.moves[0], chosen.moves[1]);
        let auto = |i: usize| if chosen.auto[i] { " (auto)" } else { "" };
        log.push(format!(
            "{}{} vs {}{} → **{}** wins the clash ⚔️",
            MOVES[chosen.moves[0]].0,
            auto(0),
            MOVES[chosen.moves[1]].0,
            auto(1),
            if side == 0 { &a.name } else { &b.name }
        ));
        text = fight_text(&head, &log, a, b, &hp);
        keep_at_bottom(ctx, channel, &mut message, &text, None, opening.as_ref(), Some(Vec::new())).await;
        decided = Some(side);
        script = script_fight(seed, side).into();
        tokio::time::sleep(REVEAL).await;
    }

    // Trade blows until someone's health runs out.
    let mut turns = 0;
    while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
        turns += 1;
        tokio::time::sleep(BEAT).await;
        let (attacker, swing) = match script.pop_front() {
            Some(blow) => blow,
            None => {
                let attacker = roll(seed, 2) as usize;
                (attacker, swing(seed, attacker))
            }
        };
        let (x, y) = if attacker == 0 { (a, b) } else { (b, a) };
        hp = land(hp, attacker, &swing);
        let line = fill(pick(swing.blow.lines(lines), seed), &x.name, &y.name);
        log.push(format!("{} · **{}**", line, swing.tail()));
        text = fight_text(&head, &log, a, b, &hp);
        keep_at_bottom(ctx, channel, &mut message, &text, None, opening.as_ref(), None).await;
    }

    let a_wins = match decided {
        Some(side) => side == 0,
        None => hp[0] > hp[1] || (hp[0] == hp[1] && roll(seed, 2) == 0),
    };
    let (winner, loser) = if a_wins { (a, b) } else { (b, a) };
    let finish = fill(pick(lines.finish, seed), &winner.name, &loser.name);
    text.push_str(&format!("\n\n🏆 {}", finish));
    let side = if a_wins { 0 } else { 1 };
    let done =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), finish, Outcome::Won(side), None, theme).await;
    tokio::time::sleep(BEAT).await;
    keep_at_bottom(ctx, channel, &mut message, &text, done, opening.as_ref(), None).await;
    tracing::info!("battle: {} won ({} - {})", winner.name, hp[0].max(0), hp[1].max(0));
    // Between fights nobody is watching a message, so stop counting chat.
    BELOW.lock().remove(&channel.get());
    (winner.clone(), hp[if a_wins { 0 } else { 1 }].max(0))
}

// --- clash picks ------------------------------------------------------------

/// The four moves: the symbol shown, the button's name, and its colour.
const MOVES: [(&str, &str, ButtonStyle); 4] = [
    ("△", "triangle", ButtonStyle::Success),
    ("○", "circle", ButtonStyle::Danger),
    ("□", "square", ButtonStyle::Secondary),
    ("✕", "cross", ButtonStyle::Primary),
];
/// How long both fighters have to pick before the bot picks for them,
/// `VIZIER_FIGHT_PICK_SECS`.
const PICK_WAIT: u64 = 15;
/// How long the clash result stays up before the first blow.
const REVEAL: Duration = Duration::from_millis(1800);

/// Who beats whom for one fight. Every move beats exactly two of the other
/// side's moves and loses to the other two, for both fighters, so no button is
/// ever better than another: each side wins a turn half the time, whatever
/// they press, until they start reading the pattern.
struct Clash {
    left: [usize; 4],
    right: [usize; 4],
}

impl Clash {
    fn deal(seed: &mut u64) -> Clash {
        let mut perm = |seed: &mut u64| {
            let mut p = [0, 1, 2, 3];
            for i in (1..4).rev() {
                p.swap(i, roll(seed, i as u64 + 1) as usize);
            }
            p
        };
        let left = perm(seed);
        let right = perm(seed);
        Clash { left, right }
    }

    /// 0 if the left fighter's move wins the clash, 1 if the right's does.
    fn winner(&self, left_move: usize, right_move: usize) -> usize {
        if (self.left[left_move % 4] + self.right[right_move % 4]) % 4 < 2 { 0 } else { 1 }
    }
}

/// One turn's picks, filled in by the buttons.
struct Picks {
    fighters: [u64; 2],
    moves: [Option<usize>; 2],
    open: bool,
}

/// Open picks, by fight.
static PICKS: LazyLock<Mutex<HashMap<u64, Picks>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct Chosen {
    moves: [usize; 2],
    /// Which side the bot picked for.
    auto: [bool; 2],
}

fn pick_rows(fight_id: u64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(
        MOVES
            .iter()
            .enumerate()
            .map(|(i, (symbol, _, style))| {
                CreateButton::new(format!("fightpick:{}:{}", fight_id, i)).label(*symbol).style(*style)
            })
            .collect(),
    )]
}

/// Opens a turn, waits for both picks or the timer, and fills in whoever didn't press.
#[allow(clippy::too_many_arguments)]
async fn pick_moves(
    ctx: &Context,
    channel: ChannelId,
    message: &mut Message,
    head: &str,
    log: &[String],
    a: &Warrior,
    b: &Warrior,
    hp: &[i32; 2],
    fight_id: u64,
    carry: Option<&Vec<u8>>,
    seed: &mut u64,
) -> Chosen {
    PICKS.lock().insert(fight_id, Picks { fighters: [a.id, b.id], moves: [None, None], open: true });
    let pick_wait = Duration::from_secs(super::control::number("VIZIER_FIGHT_PICK_SECS", PICK_WAIT).max(3));
    let closes = Utc::now().timestamp() + pick_wait.as_secs() as i64;
    let prompt = format!(
        "{}\n\n🎮 <@{}> and <@{}>, pick a move! **Win the clash, win the fight.** Closes <t:{}:R>",
        fight_text(head, log, a, b, hp),
        a.id,
        b.id,
        closes
    );
    keep_at_bottom(ctx, channel, message, &prompt, None, carry, Some(pick_rows(fight_id))).await;
    let deadline = tokio::time::Instant::now() + pick_wait;
    loop {
        let both = PICKS.lock().get(&fight_id).is_some_and(|p| p.moves.iter().all(Option::is_some));
        if both || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let taken = {
        let mut picks = PICKS.lock();
        match picks.get_mut(&fight_id) {
            Some(p) => {
                p.open = false;
                p.moves
            }
            None => [None, None],
        }
    };
    let mut chosen = Chosen { moves: [0, 0], auto: [false, false] };
    for side in 0..2 {
        match taken[side] {
            Some(m) => chosen.moves[side] = m,
            None => {
                chosen.moves[side] = roll(seed, 4) as usize;
                chosen.auto[side] = true;
            }
        }
    }
    chosen
}

/// A move button. Only the two fighters can press, once per turn, while it is open.
async fn on_pick(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let mut parts = rest.split(':');
    let (Some(Ok(fight_id)), Some(Ok(choice))) =
        (parts.next().map(str::parse::<u64>), parts.next().map(str::parse::<usize>))
    else {
        return;
    };
    let user = component.user.id.get();
    let reply = {
        let mut picks = PICKS.lock();
        match picks.get_mut(&fight_id) {
            None => "This fight is over.".to_string(),
            Some(p) => match p.fighters.iter().position(|f| *f == user) {
                None => "This isn't your fight. Grab some popcorn 🍿".to_string(),
                Some(_) if !p.open => "Too late, this turn is already done.".to_string(),
                Some(side) => match p.moves[side] {
                    Some(m) => format!("You already picked {} this turn.", MOVES[m].0),
                    None if choice < MOVES.len() => {
                        p.moves[side] = Some(choice);
                        format!("You picked {}. Waiting for the clash…", MOVES[choice].0)
                    }
                    None => "That isn't a move.".to_string(),
                },
            },
        }
    };
    let _ = component.create_response(&ctx.http, whisper(reply)).await;
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

/// The blows for a fight whose winner the clash already decided. A fight is
/// rolled as usual; if it went the other way it is played mirrored, which swaps
/// every blow between the two and so the result, and it reads just as natural.
/// A dead level fight is rolled again; the empty fallback leaves the blows to
/// chance, and the clash still names the winner.
fn script_fight(seed: &mut u64, winner: usize) -> Vec<(usize, Swing)> {
    for _ in 0..8 {
        let (blows, hp) = roll_fight(seed);
        if hp[winner] > hp[1 - winner] {
            return blows;
        }
        if hp[winner] < hp[1 - winner] {
            return blows
                .into_iter()
                .map(|(attacker, s)| (1 - attacker, Swing { blow: s.blow, hits: [s.hits[1], s.hits[0]] }))
                .collect();
        }
    }
    Vec::new()
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
fn fight_text(head: &str, log: &[String], a: &Warrior, b: &Warrior, hp: &[i32; 2]) -> String {
    let side = |who: &Warrior, hp: i32| {
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
    let theme = theme_option(&command.data.options);
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
    let (Some(a), Some(b)) = (warrior(ctx, guild, me).await, warrior(ctx, guild, them).await) else {
        let _ = command
            .edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content("Couldn't find that member."))
            .await;
        return;
    };

    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let card = fight_card(
        "Challenge".into(),
        a.card(START_HP),
        b.card(START_HP),
        format!("{} vs {}", a.name, b.name),
        Outcome::Open,
        None,
        theme,
    )
    .await;
    let flavour = match theme {
        Theme::Classic => String::new(),
        other => format!(" **{}** style", other.label()),
    };
    let wait = challenge_wait();
    let content = format!(
        "⚔️ <@{}> has challenged <@{}> to a{} fight!\n<@{}>, accept or decline — the challenge expires in {}.\n         -# When the fight starts, both fighters pick △ ○ □ ✕. Win the clash, win the fight.",
        me,
        them,
        if flavour.is_empty() { String::new() } else { flavour },
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
            let (winner, _) = play(ctx, arena, "Challenge", &a, &b, &mut seed, theme, true).await;
            let loser = if winner.id == a.id { b.id } else { a.id };
            record("fight", winner.id, Some(loser));
            let (fights, wins) = tally(winner.id);
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!(
                            "🏆 **{}** won! That is **{}** wins from **{}** fights. `/fightboard` for the rest.",
                            winner.name, wins, fights
                        ))
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
            BUSY.lock().remove(&arena.get());
        }
        Some(false) => {
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!("🏃 <@{}> declined the challenge.", them))
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
        let _ = command.create_response(&ctx.http, whisper("Only admins can start a battle.".into())).await;
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
    let theme = theme_option(&command.data.options);

    let here = command.channel_id;
    let arena = arena(ctx, guild, here).await;
    if !BUSY.lock().insert(arena.get()) {
        let _ = command.create_response(&ctx.http, whisper("A battle is already running.".into())).await;
        return;
    }
    let _ = command.create_response(&ctx.http, whisper(format!("Lobby open for {} minutes.", minutes))).await;
    open_lobby(ctx, guild, arena, here, minutes, theme, Ping::Warriors).await;
}

/// Who a lobby tags when it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ping {
    Warriors,
    Houses,
    Everyone,
    Nobody,
}

impl Ping {
    fn from_key(key: &str) -> Ping {
        match key.trim().to_ascii_lowercase().as_str() {
            "warriors" | "warrior" => Ping::Warriors,
            "everyone" => Ping::Everyone,
            "none" | "nobody" => Ping::Nobody,
            _ => Ping::Houses,
        }
    }
}

/// Opens a lobby in the arena, waits it out, and runs the battle. The arena
/// must already be marked busy; it is freed at the end.
#[allow(clippy::too_many_arguments)]
async fn open_lobby(ctx: &Context, guild: GuildId, arena: ChannelId, here: ChannelId, minutes: i64, theme: Theme, ping: Ping) {
    let ends = Utc::now().timestamp() + minutes * 60;
    let (tag, mentions) = match ping {
        Ping::Warriors => {
            let role = warrior_role(ctx, guild).await;
            let tag = role.map(|r| format!("<@&{}>", r)).unwrap_or_else(|| "Warriors".into());
            (tag, CreateAllowedMentions::new().roles(role.into_iter().collect::<Vec<_>>()))
        }
        Ping::Houses => {
            let roles = super::house::house_roles(ctx, guild).await;
            let tag = roles.iter().map(|r| format!("<@&{}>", r)).collect::<Vec<_>>().join(" ");
            (if tag.is_empty() { "Houses".into() } else { tag }, CreateAllowedMentions::new().roles(roles))
        }
        Ping::Everyone => ("@everyone".into(), CreateAllowedMentions::new().everyone(true)),
        Ping::Nobody => ("⚔️".into(), CreateAllowedMentions::new()),
    };
    // The tags go in a message of their own that stays put: the lobby card moves
    // down as chat piles up (and the old copy is deleted), which used to take
    // the tags with it.
    if ping != Ping::Nobody {
        let heads_up = CreateMessage::new()
            .content(format!("{} — a battle royale is starting! Join the lobby below 👇", tag))
            .allowed_mentions(mentions);
        if let Err(err) = call(arena.send_message(&ctx.http, heads_up)).await {
            tracing::warn!("battle: lobby tags not posted in {}: {}", arena, err);
        }
    }
    let embed = lobby_embed(&[], ends, minutes, theme);
    let msg = CreateMessage::new()
        .content("⚔️ Battle royale lobby · hit Join 👇")
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
        .insert(lobby_id, Lobby { joined: Vec::new(), names: HashMap::new(), open: true, ends, minutes, theme });
    if arena != here {
        let _ = here
            .send_message(
                &ctx.http,
                CreateMessage::new().content(format!("⚔️ The battle royale is running here → {}", posted.link())),
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
            .content("⚔️ Battle royale lobby · hit Join 👇")
            .allowed_mentions(CreateAllowedMentions::new())
            .embed(lobby_embed(&names, ends, minutes, theme))
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
            .say(&ctx.http, format!("Only {} joined. Battle cancelled — {} are needed.", joined.len(), needed))
            .await;
        BUSY.lock().remove(&arena.get());
        return;
    }
    run_battle(ctx, guild, arena, joined, theme).await;
    BUSY.lock().remove(&arena.get());
}

fn lobby_buttons(id: u64, open: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("battlejoin:{}", id))
            .label("⚔️ Join")
            .style(ButtonStyle::Success)
            .disabled(!open),
        CreateButton::new(format!("battlewarrior:{}", id))
            .label("🔔 Warrior role")
            .style(ButtonStyle::Secondary)
            .disabled(!open),
    ])]
}

fn lobby_embed(names: &[String], ends: i64, minutes: i64, theme: Theme) -> CreateEmbed {
    let list = if names.is_empty() {
        "Nobody yet. Who's first?".to_string()
    } else {
        let shown = lobby_names();
        let mut list = names.iter().take(shown).map(|n| format!("• {}", n)).collect::<Vec<_>>().join("\n");
        if names.len() > shown {
            list.push_str(&format!("\n…and **{}** more", names.len() - shown));
        }
        list
    };
    CreateEmbed::new()
        .title(match theme {
            Theme::Classic => "⚔️ Battle Royale".to_string(),
            other => format!("⚔️ Battle Royale · {} edition", other.label()),
        })
        .description(format!(
            "Hit Join and get ready to fight. The winner takes the **{}** role.\n\n\
             ⏳ Closes <t:{}:R> ({} min)\n👥 **{}** joined (at least {} needed)\n\n{}",
            CHAMPION_ROLE,
            ends,
            minutes,
            names.len(),
            min_players(),
            list
        ))
        .colour(0xE67E22)
        .footer(CreateEmbedFooter::new("Every fight is a coin toss — just here for the banter"))
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

/// The theme a daily battle uses: a fixed one, or a different one at random.
fn daily_theme(key: &str, roll: u64) -> Theme {
    match Theme::from_key(key) {
        Some(theme) => theme,
        None => Theme::ALL[(roll % Theme::ALL.len() as u64) as usize],
    }
}

/// Opens a battle royale every day at `VIZIER_BATTLE_DAILY_TIME` (India time)
/// when `VIZIER_BATTLE_DAILY` is on - the same lobby /battle opens, so nothing
/// about the battle itself differs. If a fight is running at that moment it
/// tries again every half minute for up to half an hour.
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
                tracing::warn!("battle: daily battle is on but VIZIER_FIGHT_CHANNEL is not set");
                meta_set(&slot_key, "skipped");
                continue;
            };
            let arena = arena(&ctx, guild, fallback).await;
            if !BUSY.lock().insert(arena.get()) {
                continue;
            }
            meta_set(&slot_key, "opened");
            let minutes = (super::control::number("VIZIER_BATTLE_DAILY_MINUTES", 10) as i64).clamp(MIN_WAIT, max_lobby_minutes());
            let theme = daily_theme(
                &super::control::var("VIZIER_BATTLE_DAILY_THEME").unwrap_or_else(|| "classic".into()),
                now as u64,
            );
            let ping = Ping::from_key(&super::control::var("VIZIER_BATTLE_DAILY_PING").unwrap_or_default());
            tracing::info!("battle: daily battle opening for {} {} ({} min, {:?}, {:?})", day, time, minutes, theme, ping);
            let ctx = ctx.clone();
            tokio::spawn(async move {
                open_lobby(&ctx, guild, arena, arena, minutes, theme, ping).await;
            });
        }
    });
}

/// Knockout rounds until one is left, on a draw fixed at the start: winners
/// meet the winner beside them, and the bracket goes up before every round.
async fn run_battle(ctx: &Context, guild: GuildId, arena: ChannelId, joined: Vec<u64>, theme: Theme) {
    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let mut fighters: Vec<Warrior> = Vec::new();
    for id in joined {
        if let Some(w) = warrior_named(ctx, guild, id).await {
            fighters.push(w);
        }
    }
    if fighters.len() < min_players() {
        let _ = arena.say(&ctx.http, "Not enough fighters could be loaded. Battle cancelled.").await;
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
        let title = title_case(&round_title(matches));
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
                        .map(|w| Entrant { name: w.name.clone(), avatar: w.avatar.clone(), house: w.house })
                        .collect(),
                ));
            }
            if let Some(entrants) = &entrants {
                let subtitle = format!("{} warriors · {}", started, title);
                let caption = format!("🗺️ **{}**: here's the bracket", title);
                post_bracket(ctx, arena, entrants, &rounds[chart_start..], subtitle, theme, &caption).await;
            }
        }
        if r == 0 {
            let passes: Vec<String> = rounds[0].iter().filter(|m| m.bye).filter_map(|m| m.a).map(|i| tag(&fighters[i])).collect();
            if !passes.is_empty() {
                say_chunks(ctx, arena, &format!("☕ **Free pass to the next round** ({})", passes.len()), &passes, ", ").await;
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
            let head = format!("⚡ **{}** · quick round · {} fights", title, results.len());
            say_chunks(ctx, arena, &head, &results, "\n").await;
        } else {
            let stage = stage_title(matches);
            for j in 0..matches {
                let (Some(ai), Some(bi), false) = (rounds[r][j].a, rounds[r][j].b, rounds[r][j].bye) else {
                    continue;
                };
                let (winner, hp) = play(ctx, arena, &stage, &fighters[ai], &fighters[bi], &mut seed, theme, false).await;
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
        let subtitle = format!("{} warriors · {} rounds · 👑 {}", started, total, champion.name);
        post_bracket(ctx, arena, entrants, &rounds[chart_start..], subtitle, theme, "🗺️ **The final bracket**").await;
    }
    record("champion", champion.id, None);
    let battle_id = award_royale(champion.id, runner_up);
    // A Chocolate Frog card each for the champion and runner-up of a big enough royale.
    let card_lines = super::frog_rewards::royale_cards(battle_id, champion.id, runner_up, started);
    let won = crowns(champion.id);
    crown(ctx, guild, champion.id).await;
    let subtitle = format!("{} warriors · {} rounds · 1 champion", started, total);
    let line = pick(theme.lines().champion, &mut seed).to_string();
    let card = champion_card(champion.card(START_HP), subtitle, line, theme).await;
    let mut msg = CreateMessage::new()
        .content(
            std::iter::once(format!("👑 <@{}> is the **{}**! Battles won: **{}**", champion.id, CHAMPION_ROLE, won))
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
fn tag(w: &Warrior) -> String {
    let crest = w.house.map(|h| format!("{} ", h.crest)).unwrap_or_default();
    format!("{}{}", crest, plain(&w.name))
}

fn tag_bold(w: &Warrior) -> String {
    let crest = w.house.map(|h| format!("{} ", h.crest)).unwrap_or_default();
    format!("{}**{}**", crest, plain(&w.name))
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
    theme: Theme,
    caption: &str,
) {
    let (entrants, rounds) = (entrants.clone(), rounds.to_vec());
    let png = tokio::task::spawn_blocking(move || {
        battle_bracket::bracket_png(&Bracket { entrants: &entrants, rounds: &rounds, subtitle, theme })
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

/// The stage on a fight card, by how many matches its round has.
fn stage_title(matches: usize) -> String {
    match matches {
        1 => "Final".to_string(),
        2 => "Semi-final".to_string(),
        4 => "Quarter-final".to_string(),
        n => format!("Round of {}", n * 2),
    }
}

/// "QUARTER-FINALS" as "Quarter-finals".
fn title_case(upper: &str) -> String {
    let lower = upper.to_lowercase();
    let mut chars = lower.chars();
    chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default()
}

/// The `type` option of /fight and /battle; classic when left out.
fn theme_option(options: &[serenity::all::CommandDataOption]) -> Theme {
    options
        .iter()
        .find_map(|o| match (o.name.as_str(), &o.value) {
            ("type", CommandDataOptionValue::String(key)) => Theme::from_key(key),
            _ => None,
        })
        .unwrap_or_default()
}

/// The `type` option, to add to /fight and /battle.
pub fn theme_command_option() -> serenity::all::CreateCommandOption {
    let mut option = serenity::all::CreateCommandOption::new(
        serenity::all::CommandOptionType::String,
        "type",
        "fight style (classic if left out)",
    );
    for theme in Theme::ALL {
        option = option.add_string_choice(theme.label(), theme.key());
    }
    option
}

fn shuffle<T>(list: &mut [T], seed: &mut u64) {
    for i in (1..list.len()).rev() {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        list.swap(i, (*seed % (i as u64 + 1)) as usize);
    }
}

// --- /warrior ---------------------------------------------------------------

pub async fn warrior_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.".into())).await;
        return;
    };
    let text = match toggle_warrior(ctx, guild, command.user.id.get()).await {
        Some(true) => format!("🔔 You have the {} role. You will be pinged for every battle.", WARRIOR_ROLE),
        Some(false) => format!("🔕 {} role removed. No more battle pings.", WARRIOR_ROLE),
        None => "Could not set the role — the bot may not have Manage Roles.".to_string(),
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

/// Adds the warrior role, or takes it away if they already have it.
async fn toggle_warrior(ctx: &Context, guild: GuildId, user: u64) -> Option<bool> {
    let role = warrior_role(ctx, guild).await?;
    let member = guild.member(&ctx.http, UserId::new(user)).await.ok()?;
    if member.roles.contains(&role) {
        member.remove_role(&ctx.http, role).await.ok()?;
        Some(false)
    } else {
        member.add_role(&ctx.http, role).await.ok()?;
        Some(true)
    }
}

/// `/battlestop` - lets an admin free a channel whose battle died mid-fight,
/// which otherwise stays "busy" until the bot restarts.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only admins can stop a battle.".into())).await;
        return;
    }
    let arena = match command.guild_id {
        Some(guild) => arena(ctx, guild, command.channel_id).await,
        None => command.channel_id,
    };
    let freed = BUSY.lock().remove(&arena.get());
    LOBBIES.lock().clear();
    BELOW.lock().remove(&arena.get());
    let text = if freed {
        "Cleared. A fight that was still running will stop at its next step, and `/battle` works again."
    } else {
        "Nothing was running there."
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
        text.push_str("No fights yet. Challenge someone with `/fight @name`.");
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
        .title("⚔️ Arena board")
        .description(text)
        .colour(0xE67E22)
        .footer(CreateEmbedFooter::new("👑 = battles won · every 1v1 counts, battle fights included"));
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
    } else if id.starts_with("battlewarrior:") {
        let text = match component.guild_id {
            Some(guild) => match toggle_warrior(ctx, guild, component.user.id.get()).await {
                Some(true) => format!("🔔 You have the {} role.", WARRIOR_ROLE),
                Some(false) => format!("🔕 {} role removed.", WARRIOR_ROLE),
                None => "Could not set the role.".to_string(),
            },
            None => "This only works in a server.".to_string(),
        };
        let _ = component.create_response(&ctx.http, whisper(&text)).await;
    } else if let Some(rest) = id.strip_prefix("fightpick:") {
        on_pick(ctx, component, rest).await;
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
    let (result, names, ends, minutes, theme) = {
        let mut lobbies = LOBBIES.lock();
        let roster = |lobby: &Lobby| -> Vec<String> {
            lobby.joined.iter().filter_map(|u| lobby.names.get(u).cloned()).collect()
        };
        match lobbies.get_mut(&lobby_id) {
            None => ("closed", Vec::new(), 0, 0, Theme::Classic),
            Some(lobby) if !lobby.open => ("closed", Vec::new(), lobby.ends, lobby.minutes, lobby.theme),
            Some(lobby) if lobby.joined.contains(&user) => ("already", roster(lobby), lobby.ends, lobby.minutes, lobby.theme),
            Some(lobby) => {
                lobby.joined.push(user);
                lobby.names.insert(user, name);
                ("joined", roster(lobby), lobby.ends, lobby.minutes, lobby.theme)
            }
        }
    };
    match result {
        "joined" => {
            let _ = component.create_response(&ctx.http, whisper("⚔️ You are in. Get ready.")).await;
            // Everyone should see the roster fill up, countdown untouched.
            let mut message = component.message.clone();
            let _ = message.edit(&ctx.http, EditMessage::new().embed(lobby_embed(&names, ends, minutes, theme))).await;
        }
        "already" => {
            let _ = component.create_response(&ctx.http, whisper("You are already in.")).await;
        }
        _ => {
            let _ = component.create_response(&ctx.http, whisper("That battle is closed.")).await;
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
        let text = if yes { "⚔️ Let's go." } else { "🏃 Fine, backing out." };
        let _ = component.create_response(&ctx.http, whisper(text)).await;
    } else {
        let _ = component.create_response(&ctx.http, whisper("That challenge is not yours.")).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fill_both_names_and_stages_read_right() {
        let line = fill("{a} ne {b} ko chappal dikhayi", "Ravi", "Sneha");
        assert_eq!(line, "Ravi ne Sneha ko chappal dikhayi");
        assert_eq!(stage_title(1), "Final");
        assert_eq!(stage_title(2), "Semi-final");
        assert_eq!(stage_title(4), "Quarter-final");
        assert_eq!(stage_title(8), "Round of 16");
        assert_eq!(title_case("QUARTER-FINALS"), "Quarter-finals");
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
        assert_eq!(Ping::from_key("anything"), Ping::Houses);
        assert_eq!(daily_theme("pokemon", 3), Theme::Pokemon);
        let themes: std::collections::HashSet<_> = (0..20).map(|r| daily_theme("random", r)).collect();
        assert!(themes.len() > 3);
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

    #[test]
    fn every_clash_is_fifty_fifty_whatever_is_pressed() {
        let mut seed = 99u64;
        let mut tables = std::collections::HashSet::new();
        for _ in 0..200 {
            let clash = Clash::deal(&mut seed);
            tables.insert((clash.left, clash.right));
            for mine in 0..4 {
                let left_wins = (0..4).filter(|theirs| clash.winner(mine, *theirs) == 0).count();
                assert_eq!(left_wins, 2, "left move {} wins {} of 4", mine, left_wins);
                let right_wins = (0..4).filter(|theirs| clash.winner(*theirs, mine) == 1).count();
                assert_eq!(right_wins, 2, "right move {} wins {} of 4", mine, right_wins);
            }
        }
        assert!(tables.len() > 50, "fights keep getting the same table: {}", tables.len());
    }

    #[test]
    fn a_scripted_fight_ends_with_the_clash_winner_ahead() {
        let mut seed = 31u64;
        for i in 0..1000 {
            let winner = i % 2;
            let blows = script_fight(&mut seed, winner);
            assert!(!blows.is_empty() && blows.len() <= MAX_EXCHANGES, "fight {} has {} blows", i, blows.len());
            let hp = blows.iter().fold([START_HP; 2], |hp, (attacker, s)| land(hp, *attacker, s));
            assert!(hp[winner] > hp[1 - winner], "fight {}: {:?} should favour side {}", i, hp, winner);
        }
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
        let warrior = |id: u64, name: &str| Warrior { id, name: name.to_string(), avatar: None, house: None, face: String::new() };
        let (a, b) = (warrior(1, "Ravi"), warrior(2, "Sneha"));
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
        let warriors = |n: usize| {
            (0..n)
                .map(|i| Warrior { id: i as u64, name: format!("w{}", i), avatar: None, house: None, face: String::new() })
                .collect::<Vec<_>>()
        };
        let mut list = warriors(9);
        let mut seed = 99u64;
        shuffle(&mut list, &mut seed);
        let ids: std::collections::HashSet<u64> = list.iter().map(|w| w.id).collect();
        assert_eq!(ids.len(), 9);
        let chunks: Vec<usize> = list.chunks(2).map(|c| c.len()).collect();
        assert_eq!(chunks.iter().filter(|n| **n == 1).count(), 1, "odd rounds need exactly one bye");
    }
}
