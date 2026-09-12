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
}

impl Warrior {
    fn card(&self, hp: i32) -> Fighter {
        Fighter {
            name: self.name.clone(),
            avatar: self.avatar.clone(),
            hp: hp.max(0) as u32,
            max_hp: START_HP as u32,
        }
    }
}

// --- lines ------------------------------------------------------------------
// Roasts stay on the fight itself: nothing about looks, family, caste or faith.

const EXCHANGE: &[&str] = &[
    "{a} ne {b} ko chappal dikhayi 🩴",
    "{b} ne {a} ki DP screenshot kar li, blackmail ki taiyari",
    "{a} ne {b} ko ek dum se 'bhai sun' bola aur block kar diya",
    "{b} ne {a} pe pura jug paani daal diya",
    "{a} ne {b} ki keyboard ke keycaps nikaal diye",
    "{b} ne {a} ko VC mein ghaseet liya, mic mute karke",
    "{a} ne {b} ko 'seen' kar diya, reply nahi bheja",
    "{b} ne {a} ka WiFi router unplug kar diya",
    "{a} ne {b} pe tagda meme daga",
    "{b} ne {a} ko typing... typing... pe 10 minute rakha",
    "{a} ne {b} ki maggi bina namak ke bana di",
    "{b} ne {a} ko 'tera match to Jio pe hi atka hai' bola",
    "{a} ne {b} ka phone 1% battery pe chhod diya",
    "{b} ne {a} ki chai mein cheeni double kar di",
    "{a} ne {b} ko group se remove karke wapas add kiya, sirf dikhane ke liye",
    "{b} ne {a} ka last seen chhupa diya",
    "{a} ne {b} ko ludo mein teen baar chhakka maar ke hara diya",
    "{b} ne {a} ki playlist mein sirf sad songs bhar diye",
    "{a} ne {b} ka chair khinch liya, classic",
    "{b} ne {a} ko 'aur bata' bolke mool baat hi nahi batayi",
];

const FINISH: &[&str] = &[
    "{w} won. {l} is out 🏆",
    "{l} is down — {w} took it without breaking a sweat",
    "{w} wins, and {l} already has an excuse ready",
    "{l} made a dramatic exit, {w} waved them off",
    "{w} survives. {l} is out, and yes, someone screenshotted it",
    "Game over for {l}. {w} threw in a victory dance",
    "{w} landed the finisher: the silent treatment. {l} is done",
    "{l} gave up, {w} picked the chai back up",
    "{w} won. {l} moves to commentary",
    "{w} showed {l} the exit 🚪",
];

const CRIT: &[&str] = &[
    "{a} ne {b} pe poora combo chala diya, bina saans liye 💥",
    "{a} ka jhakaas headshot - {b} ka WiFi tak hil gaya",
    "{a} ne {b} ko ek hi taane mein udaa diya",
    "{a} ne {b} ki puri chat history nikal ke padh di 😳",
    "{a} ne {b} ko uske hi meme se maara",
];

const MISS: &[&str] = &[
    "{a} ne haath ghumaya... aur hawa mein reh gaya",
    "{a} ka taana miss, {b} ne duck kar liya",
    "{a} laga raha tha ki ye landega - nahi landa",
];

const HEAL: &[&str] = &[
    "{a} ne chai ka ghoont liya, thodi jaan wapas aayi ☕",
    "{a} ne maggi khaayi aur fresh ho gaya 🍜",
    "{a} ne Hanuman Chalisa laga di, thodi power aayi",
];

/// A sip, a biscuit, a deep breath: barely anything, purely for the joke.
const SIP: &[&str] = &[
    "{a} ne ek ghoont chai maari. Bas itna hi. ☕",
    "{a} ko kahin se ek Parle-G mil gaya 🍪",
    "{a} ne lamba saans liya aur collar theek kiya",
    "{a} ne apni DP badal di, confidence +1",
];

/// The swing comes back at whoever threw it.
const BACKFIRE: &[&str] = &[
    "{a} ne chappal feki, wapas aake khud ko lagi 🩴",
    "{a} apne hi jokes pe has ke gir gaya",
    "{a} ne block karne ki koshish ki, khud ko block kar liya",
    "{a} ka screenshot ulta pad gaya, apni hi baat pakdi gayi",
];

/// {b} loses, {a} gains: the fight's only real comeback move.
const DRAIN: &[&str] = &[
    "{a} ne {b} ki plate se samosa utha liya 🥟",
    "{a} ne {b} ka charger le liya - ab {a} full, {b} khali 🔌",
    "{a} ne {b} ki chai pi li, seedha energy transfer",
    "{a} ne {b} ka WiFi password chura liya",
];

/// Two in a row, before anyone can answer.
const DOUBLE: &[&str] = &[
    "{a} ne do baar maara - ek taana, ek screenshot 📸",
    "{a} ne back to back do meme daag diye",
    "{a} ne {b} ko group aur DM, dono mein sunaya",
    "{a} ne double chappal combo lagaya 🩴🩴",
];

/// Everyone in the frame suffers.
const CHAOS: &[&str] = &[
    "Beech mein aunty aa gayi, dono ko daant padi 👵",
    "Light chali gayi - dono andhere mein gir gaye 💡",
    "Dono ek hi kele ke chhilke pe phisal gaye 🍌",
    "Kisi ne dono ka naam mummy ko bata diya 😰",
];

/// A proper feed: the biggest ordinary heal.
const SNACK: &[&str] = &[
    "{a} ne garam samosa khaaya, shakti aa gayi 🥟",
    "{a} ne biryani ka dabba khol liya, ab mood set hai 🍛",
    "{a} ne do minute mein Maggi bana li 🍜",
    "{a} ne thanda Rooh Afza gatak liya 🥤",
    "{a} ne pani puri ka ek aur round maanga 🥣",
];

/// Rare, and worth it.
const BLESSING: &[&str] = &[
    "{a} ki mummy ne sar pe haath rakh diya - poori jaan wapas 🙏",
    "{a} ko kisi ne 'jeete raho beta' bol diya, full power up ✨",
    "{a} ne prasad kha liya, ab kaun rok sakta hai 🪔",
    "{a} ke papa ne kandha thapthapaya - motivation overload",
];

/// Both sides gain: the fight pauses for something nicer.
const CROWD: &[&str] = &[
    "Crowd ne dono ko cheer kar diya, dono ka mann bhar aaya 📣",
    "Kisi ne dono ko chai pila di, ladai thodi der ke liye band ☕",
    "Dono ne ek hi thali se kha liya, dosti ho gayi 🍽️",
    "Aunty ne dono ko laddoo pakda diya 🍬",
];

const BYE: &[&str] = &[
    "{a} had nobody to fight this round and went for chai ☕",
    "{a} gets a free pass to the next round",
    "{a}'s opponent never turned up — walkover",
];

const CHAMPION_LINE: &[&str] = &[
    "Took on the whole server and won 👑",
    "Last one standing, crown and all",
    "Today's champion. Everyone else is on commentary",
    "Undisputed. The rest can wait for the next battle",
];

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
    fn lines(self) -> &'static [&'static str] {
        match self {
            Blow::Miss => MISS,
            Blow::Crit => CRIT,
            Blow::Heal => HEAL,
            Blow::Hit => EXCHANGE,
            Blow::Sip => SIP,
            Blow::Backfire => BACKFIRE,
            Blow::Drain => DRAIN,
            Blow::Double => DOUBLE,
            Blow::Chaos => CHAOS,
            Blow::Snack => SNACK,
            Blow::Blessing => BLESSING,
            Blow::Crowd => CROWD,
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
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO results (kind, winner, loser, ts) VALUES (?1, ?2, ?3, ?4)",
            params![kind, winner as i64, loser.map(|u| u as i64), Utc::now().timestamp()],
        );
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
    Some(Warrior { id: user, name: display(&member), avatar })
}

/// Drawing a card is CPU work, so it never runs on the gateway thread.
async fn fight_card(
    stage: String,
    left: Fighter,
    right: Fighter,
    line: String,
    outcome: Outcome,
    hit: Option<(usize, i32)>,
) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        battle_card::fight_png(&Fight { stage, left: &left, right: &right, line, outcome, hit })
    })
    .await
    .ok()
    .flatten()
}

async fn champion_card(who: Fighter, subtitle: String, line: String) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || battle_card::champion_png(&Champion { who: &who, subtitle, line }))
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
) {
    let buried = BELOW.lock().get(&channel.get()).copied().unwrap_or(0) >= STICKY_AFTER;
    if !buried {
        let mut edit = EditMessage::new().content(text);
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
/// Returns the winner.
async fn play(ctx: &Context, channel: ChannelId, stage: &str, a: &Warrior, b: &Warrior, seed: &mut u64) -> Warrior {
    let mut hp = [START_HP; 2];
    // One picture at the start, one at the end: the blow-by-blow rides on the
    // text, which edits without an upload.
    let opening =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), String::new(), Outcome::Open, None).await;
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
                return if roll(seed, 2) == 0 { a.clone() } else { b.clone() };
            }
        }
    };

    // Trade blows until someone's health runs out. What lands is luck, not skill.
    let mut turns = 0;
    while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
        turns += 1;
        tokio::time::sleep(BEAT).await;
        let attacker = roll(seed, 2) as usize;
        let (x, y) = if attacker == 0 { (a, b) } else { (b, a) };
        let swing = swing(seed, attacker);
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
        let line = fill(pick(swing.blow.lines(), seed), &x.name, &y.name);
        log.push(format!("{} · **{}**", line, swing.tail()));
        text = fight_text(&head, &log, a, b, &hp);
        keep_at_bottom(ctx, channel, &mut message, &text, None, opening.as_ref()).await;
    }

    let a_wins = hp[0] > hp[1] || (hp[0] == hp[1] && roll(seed, 2) == 0);
    let (winner, loser) = if a_wins { (a, b) } else { (b, a) };
    let finish = fill(pick(FINISH, seed), &winner.name, &loser.name);
    text.push_str(&format!("\n\n🏆 {}", finish));
    let side = if a_wins { 0 } else { 1 };
    let done =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), finish, Outcome::Won(side), None).await;
    tokio::time::sleep(BEAT).await;
    keep_at_bottom(ctx, channel, &mut message, &text, done, opening.as_ref()).await;
    tracing::info!("battle: {} won ({} - {})", winner.name, hp[0].max(0), hp[1].max(0));
    // Between fights nobody is watching a message, so stop counting chat.
    BELOW.lock().remove(&channel.get());
    winner.clone()
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
    )
    .await;
    let content = format!(
        "⚔️ <@{}> has challenged <@{}>!\n<@{}>, accept or decline — the challenge expires in 2 minutes.",
        me, them, them
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
            let winner = play(ctx, arena, "Challenge", &a, &b, &mut seed).await;
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
    let embed = lobby_embed(&[], ends, minutes);
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
        .insert(lobby_id, Lobby { joined: Vec::new(), names: HashMap::new(), open: true, ends, minutes });
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
    run_battle(ctx, guild, arena, joined).await;
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

fn lobby_embed(names: &[String], ends: i64, minutes: i64) -> CreateEmbed {
    let list = if names.is_empty() {
        "Nobody yet. Who's first?".to_string()
    } else {
        names.iter().map(|n| format!("• {}", n)).collect::<Vec<_>>().join("\n")
    };
    CreateEmbed::new()
        .title("⚔️ Battle Royale")
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
async fn run_battle(ctx: &Context, guild: GuildId, arena: ChannelId, joined: Vec<u64>) {
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
                    let winner = play(ctx, arena, &stage, a, b, &mut seed).await;
                    record("battle", winner.id, Some(if winner.id == a.id { b.id } else { a.id }));
                    next.push(winner);
                    tokio::time::sleep(FIGHT_GAP).await;
                }
                [alone] => {
                    let line = fill(pick(BYE, &mut seed), &alone.name, "");
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
    let won = crowns(champion.id);
    crown(ctx, guild, champion.id).await;
    let subtitle = format!("{} warriors · {} rounds · 1 champion", started, round - 1);
    let line = pick(CHAMPION_LINE, &mut seed).to_string();
    let card = champion_card(champion.card(START_HP), subtitle, line).await;
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
            Some(lobby) if lobby.joined.len() >= MAX_PLAYERS => ("full", Vec::new(), lobby.ends, lobby.minutes),
            Some(lobby) => {
                lobby.joined.push(user);
                lobby.names.insert(user, name);
                ("joined", roster(lobby), lobby.ends, lobby.minutes)
            }
        }
    };
    match result {
        "joined" => {
            let _ = component.create_response(&ctx.http, whisper("⚔️ You are in. Get ready.")).await;
            // Everyone should see the roster fill up, countdown untouched.
            let mut message = component.message.clone();
            let _ = message.edit(&ctx.http, EditMessage::new().embed(lobby_embed(&names, ends, minutes))).await;
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
    fn every_line_has_placeholders_and_picks_spread() {
        for line in EXCHANGE {
            assert!(line.contains("{a}") && line.contains("{b}"), "{}", line);
        }
        for line in FINISH {
            assert!(line.contains("{w}") && line.contains("{l}"), "{}", line);
        }
        let mut seed = 12345u64;
        let picks: std::collections::HashSet<&str> = (0..200).map(|_| pick(EXCHANGE, &mut seed)).collect();
        assert!(picks.len() > EXCHANGE.len() / 2, "picks bunched up: {}", picks.len());
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
        let warrior = |id: u64, name: &str| Warrior { id, name: name.to_string(), avatar: None };
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
                .map(|i| Warrior { id: i as u64, name: format!("w{}", i), avatar: None })
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
