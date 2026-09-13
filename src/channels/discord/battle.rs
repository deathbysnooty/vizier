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
/// Blocks in a health bar.
const BAR_BLOCKS: usize = 14;
/// How long any one Discord call may take before the fight gives up on it and
/// carries on. Without this a wedged upload freezes the whole battle.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Between two fights of the same round.
const FIGHT_GAP: Duration = Duration::from_secs(3);
/// Between rounds.
const ROUND_GAP: Duration = Duration::from_secs(6);
/// A challenge nobody answers expires.
const CHALLENGE_WAIT: Duration = Duration::from_secs(120);
/// Between two `/fight`s by the same member.
const FIGHT_COOLDOWN: Duration = Duration::from_secs(60);
/// Lobby length an admin may ask for.
const MIN_WAIT: i64 = 1;
const MAX_WAIT: i64 = 15;
/// Fewer joiners than this and the battle is called off.
const MIN_PLAYERS: usize = 4;
/// Discord takes a while over each card, so keep a battle under a few minutes.
const MAX_PLAYERS: usize = 32;

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
        super::house::award_person(winner, super::points::Source::Arena, 1, "won a 1v1", None, Some(key), None);
    }
}

/// House points for a battle royale: 8 to the champion, 3 to the runner-up.
fn award_royale(champion: u64, runner_up: Option<u64>) {
    let battle = Utc::now().timestamp();
    let give = |user: u64, amount: i64, reason: &str| {
        let key = format!("royale:{}:{}", battle, user);
        super::house::award_person(user, super::points::Source::Royale, amount, reason, None, Some(key), None);
    };
    give(champion, 8, "won the battle royale");
    if let Some(user) = runner_up {
        give(user, 3, "runner-up in the battle royale");
    }
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
    if let Some(id) = std::env::var("VIZIER_FIGHT_CHANNEL").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
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

async fn warrior(ctx: &Context, guild: GuildId, user: u64) -> Option<Warrior> {
    let member = guild.member(&ctx.http, UserId::new(user)).await.ok()?;
    let face = member.face().replace("size=1024", "size=256");
    let avatar = reqwest::Client::builder()
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
        .map(|b| b.to_vec());
    // Stepped-out members fight without a badge, as they asked to be left out.
    let house = if super::house::opted_out(user) { None } else { super::house::house_of(user) };
    Some(Warrior { id: user, name: display(&member), avatar, house })
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
/// With `picks`, both fighters choose a move every turn and the clash decides who
/// lands it; without, the attacker is a coin toss as before. Returns the winner.
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
) -> Warrior {
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
                return if roll(seed, 2) == 0 { a.clone() } else { b.clone() };
            }
        }
    };

    // Trade blows until someone's health runs out.
    let mut turns = 0;
    while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
        turns += 1;
        let (attacker, clash_note) = match &clash {
            Some(table) => {
                let chosen = pick_moves(ctx, channel, &mut message, &head, &log, a, b, &hp, fight_id, turns, opening.as_ref(), seed)
                    .await;
                let side = table.winner(chosen.moves[0], chosen.moves[1]);
                let auto = |i: usize| if chosen.auto[i] { " (auto)" } else { "" };
                let note = format!(
                    "{}{} vs {}{} → **{}** wins the clash · ",
                    MOVES[chosen.moves[0]].0,
                    auto(0),
                    MOVES[chosen.moves[1]].0,
                    auto(1),
                    if side == 0 { &a.name } else { &b.name }
                );
                (side, note)
            }
            None => {
                tokio::time::sleep(BEAT).await;
                (roll(seed, 2) as usize, String::new())
            }
        };
        let (x, y) = if attacker == 0 { (a, b) } else { (b, a) };
        // Winning a clash always does the winner some good: no backfires or
        // misses for them, or pressing the right button would feel pointless.
        let swing = if clash.is_some() { clash_swing(seed, attacker) } else { swing(seed, attacker) };
        let before = hp;
        for side in 0..2 {
            hp[side] = (hp[side] + swing.hits[side]).clamp(0, START_HP);
        }
        // A chaos turn hurts both, so both bars can empty at once. Someone has
        // to be left standing: whoever was healthier keeps a sliver, and on a
        // dead tie it goes to the one who swung.
        if hp == [0, 0] {
            let standing = match before[0].cmp(&before[1]) {
                std::cmp::Ordering::Greater => 0,
                std::cmp::Ordering::Less => 1,
                std::cmp::Ordering::Equal => attacker,
            };
            hp[standing] = 1;
        }
        let line = fill(pick(swing.blow.lines(lines), seed), &x.name, &y.name);
        log.push(format!("{}{} · **{}**", clash_note, line, swing.tail()));
        text = fight_text(&head, &log, a, b, &hp);
        let rows = if clash.is_some() { Some(Vec::new()) } else { None };
        keep_at_bottom(ctx, channel, &mut message, &text, None, opening.as_ref(), rows).await;
        if clash.is_some() {
            tokio::time::sleep(REVEAL).await;
        }
    }
    if picks {
        PICKS.lock().remove(&fight_id);
    }

    let a_wins = hp[0] > hp[1] || (hp[0] == hp[1] && roll(seed, 2) == 0);
    let (winner, loser) = if a_wins { (a, b) } else { (b, a) };
    let finish = fill(pick(lines.finish, seed), &winner.name, &loser.name);
    text.push_str(&format!("\n\n🏆 {}", finish));
    let side = if a_wins { 0 } else { 1 };
    let done =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), finish, Outcome::Won(side), None, theme).await;
    tokio::time::sleep(BEAT).await;
    let rows = if picks { Some(Vec::new()) } else { None };
    keep_at_bottom(ctx, channel, &mut message, &text, done, opening.as_ref(), rows).await;
    tracing::info!("battle: {} won ({} - {})", winner.name, hp[0].max(0), hp[1].max(0));
    // Between fights nobody is watching a message, so stop counting chat.
    BELOW.lock().remove(&channel.get());
    winner.clone()
}

// --- clash picks ------------------------------------------------------------

/// The four moves: the symbol shown, the button's name, and its colour.
const MOVES: [(&str, &str, ButtonStyle); 4] = [
    ("△", "triangle", ButtonStyle::Success),
    ("○", "circle", ButtonStyle::Danger),
    ("□", "square", ButtonStyle::Secondary),
    ("✕", "cross", ButtonStyle::Primary),
];
/// How long both fighters have to pick before the bot picks for them.
const PICK_WAIT: Duration = Duration::from_secs(8);
/// How long a clash result stays up before the next turn opens.
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
    turn: usize,
    carry: Option<&Vec<u8>>,
    seed: &mut u64,
) -> Chosen {
    PICKS.lock().insert(fight_id, Picks { fighters: [a.id, b.id], moves: [None, None], open: true });
    let closes = Utc::now().timestamp() + PICK_WAIT.as_secs() as i64;
    let prompt = format!(
        "{}\n\n🎮 **Turn {}**: <@{}> and <@{}>, pick a move! Beat the other pick to land the hit. Closes <t:{}:R>",
        fight_text(head, log, a, b, hp),
        turn,
        a.id,
        b.id,
        closes
    );
    keep_at_bottom(ctx, channel, message, &prompt, None, carry, Some(pick_rows(fight_id))).await;
    let deadline = tokio::time::Instant::now() + PICK_WAIT;
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

/// A swing for the fighter who won the clash: rolled like any other, but a blow
/// that would hurt them or do nothing is rolled again.
fn clash_swing(seed: &mut u64, attacker: usize) -> Swing {
    for _ in 0..16 {
        let s = swing(seed, attacker);
        if !matches!(s.blow, Blow::Miss | Blow::Sip | Blow::Backfire | Blow::Chaos | Blow::Crowd) {
            return s;
        }
    }
    let other = 1 - attacker;
    let mut hits = [0i32; 2];
    hits[other] = -(18 + roll(seed, 11) as i32);
    Swing { blow: Blow::Hit, hits }
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
            .and_then(|at| FIGHT_COOLDOWN.checked_sub(at.elapsed()))
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
    let content = format!(
        "⚔️ <@{}> has challenged <@{}> to a{} fight!\n<@{}>, accept or decline — the challenge expires in 2 minutes.\n         -# Every turn both fighters pick △ ○ □ ✕. Win the clash to land the hit.",
        me,
        them,
        if flavour.is_empty() { String::new() } else { flavour },
        them
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
    let deadline = std::time::Instant::now() + CHALLENGE_WAIT;
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
            let winner = play(ctx, arena, "Challenge", &a, &b, &mut seed, theme, true).await;
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
        .unwrap_or(5)
        .clamp(MIN_WAIT, MAX_WAIT);
    let theme = theme_option(&command.data.options);

    let here = command.channel_id;
    let arena = arena(ctx, guild, here).await;
    if !BUSY.lock().insert(arena.get()) {
        let _ = command.create_response(&ctx.http, whisper("A battle is already running.".into())).await;
        return;
    }
    let _ = command.create_response(&ctx.http, whisper(format!("Lobby open for {} minutes.", minutes))).await;

    let ends = Utc::now().timestamp() + minutes * 60;
    let role = warrior_role(ctx, guild).await;
    let ping = role.map(|r| format!("<@&{}>", r)).unwrap_or_else(|| "Warriors".into());
    let embed = lobby_embed(&[], ends, minutes, theme);
    let msg = CreateMessage::new()
        .content(format!("{} — a battle royale is starting! Join below 👇", ping))
        .allowed_mentions(CreateAllowedMentions::new().roles(role.into_iter().collect::<Vec<_>>()))
        .embed(embed)
        .components(lobby_buttons(0, true));
    let Ok(posted) = arena.send_message(&ctx.http, msg).await else {
        BUSY.lock().remove(&arena.get());
        return;
    };
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

    tokio::time::sleep(Duration::from_secs((minutes * 60) as u64)).await;
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

    if joined.len() < MIN_PLAYERS {
        let _ = arena
            .say(&ctx.http, format!("Only {} joined. Battle cancelled — {} are needed.", joined.len(), MIN_PLAYERS))
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
        names.iter().map(|n| format!("• {}", n)).collect::<Vec<_>>().join("\n")
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
            MIN_PLAYERS,
            list
        ))
        .colour(0xE67E22)
        .footer(CreateEmbedFooter::new("Every fight is a coin toss — just here for the banter"))
}

/// Knockout rounds until one is left.
async fn run_battle(ctx: &Context, guild: GuildId, arena: ChannelId, joined: Vec<u64>, theme: Theme) {
    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let mut fighters: Vec<Warrior> = Vec::new();
    for id in joined.into_iter().take(MAX_PLAYERS) {
        if let Some(w) = warrior(ctx, guild, id).await {
            fighters.push(w);
        }
    }
    if fighters.len() < MIN_PLAYERS {
        let _ = arena.say(&ctx.http, "Not enough fighters could be loaded. Battle cancelled.").await;
        return;
    }
    let started = fighters.len();
    let mut round = 1;
    // The loser of the last fight fought is the runner-up: the final is always
    // the battle's last fight, however many byes came before it.
    let mut runner_up: Option<u64> = None;
    while fighters.len() > 1 {
        shuffle(&mut fighters, &mut seed);
        let stage = stage_name(fighters.len(), round);
        let _ = arena
            .say(&ctx.http, format!("**{}** — {} warriors left.", stage, fighters.len()))
            .await;
        tokio::time::sleep(FIGHT_GAP).await;
        let mut next = Vec::new();
        let mut pairs = fighters.chunks(2);
        while let Some(pair) = pairs.next() {
            match pair {
                [a, b] => {
                    let winner = play(ctx, arena, &stage, a, b, &mut seed, theme, false).await;
                    let loser = if winner.id == a.id { b.id } else { a.id };
                    record("battle", winner.id, Some(loser));
                    runner_up = Some(loser);
                    next.push(winner);
                    tokio::time::sleep(FIGHT_GAP).await;
                }
                [alone] => {
                    let line = fill(pick(theme.lines().bye, &mut seed), &alone.name, "");
                    let _ = arena.say(&ctx.http, format!("☕ {}", line)).await;
                    next.push(alone.clone());
                }
                _ => {}
            }
        }
        fighters = next;
        round += 1;
        if fighters.len() > 1 {
            tokio::time::sleep(ROUND_GAP).await;
        }
    }

    let Some(champion) = fighters.into_iter().next() else {
        return;
    };
    record("champion", champion.id, None);
    award_royale(champion.id, runner_up);
    let won = crowns(champion.id);
    crown(ctx, guild, champion.id).await;
    let subtitle = format!("{} warriors · {} rounds · 1 champion", started, round - 1);
    let line = pick(theme.lines().champion, &mut seed).to_string();
    let card = champion_card(champion.card(START_HP), subtitle, line, theme).await;
    let mut msg = CreateMessage::new()
        .content(format!(
            "👑 <@{}> is the **{}**! Battles won: **{}**",
            champion.id, CHAMPION_ROLE, won
        ))
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(champion.id)]));
    if let Some(png) = card {
        msg = msg.add_file(CreateAttachment::bytes(png, "champion.png"));
    }
    let _ = arena.send_message(&ctx.http, msg).await;
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

fn stage_name(left: usize, round: u32) -> String {
    match left {
        2 => "Final".to_string(),
        3..=4 => "Semi-final".to_string(),
        5..=8 => "Quarter-final".to_string(),
        _ => format!("Round {}", round),
    }
}

fn shuffle(list: &mut [Warrior], seed: &mut u64) {
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
            Some(lobby) if lobby.joined.len() >= MAX_PLAYERS => ("full", Vec::new(), lobby.ends, lobby.minutes, lobby.theme),
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
        "full" => {
            let _ = component.create_response(&ctx.http, whisper("The lobby is full.")).await;
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
        assert_eq!(stage_name(2, 4), "Final");
        assert_eq!(stage_name(4, 3), "Semi-final");
        assert_eq!(stage_name(8, 2), "Quarter-final");
        assert_eq!(stage_name(16, 1), "Round 1");
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
    fn a_won_clash_never_hurts_the_winner() {
        let mut seed = 5u64;
        for _ in 0..2000 {
            for side in 0..2 {
                let s = clash_swing(&mut seed, side);
                assert!(s.hits[side] >= 0, "{:?} hurt the clash winner", s.blow);
                assert!(s.hits.iter().any(|h| *h != 0), "{:?} did nothing", s.blow);
            }
        }
    }

    #[test]
    fn every_line_has_placeholders_and_picks_spread() {
        for line in Theme::Classic.lines().exchange {
            assert!(line.contains("{a}") && line.contains("{b}"), "{}", line);
        }
        for line in Theme::Classic.lines().finish {
            assert!(line.contains("{w}") && line.contains("{l}"), "{}", line);
        }
        let mut seed = 12345u64;
        let exchange = Theme::Classic.lines().exchange;
        let picks: std::collections::HashSet<&str> = (0..200).map(|_| pick(exchange, &mut seed)).collect();
        assert!(picks.len() > exchange.len() / 2, "picks bunched up: {}", picks.len());
    }

    /// Play the exchange loop the way `play` does, without Discord in the way.
    fn simulate(seed: &mut u64) -> ([i32; 2], usize) {
        let mut hp = [START_HP; 2];
        let mut turns = 0;
        while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
            turns += 1;
            let attacker = roll(seed, 2) as usize;
            let swing = swing(seed, attacker);
            let before = hp;
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
        }
        (hp, turns)
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
        let warrior = |id: u64, name: &str| Warrior { id, name: name.to_string(), avatar: None, house: None };
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
                .map(|i| Warrior { id: i as u64, name: format!("w{}", i), avatar: None, house: None })
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
