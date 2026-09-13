//! The Snitch: a small card that flies into chat a few times a day and scores
//! house points for the first three people who reply `accio` to it.
//!
//! Three or four drops a day, at random India times between 10:00 and midnight
//! and never within two hours of each other. A drop only goes into a channel
//! somebody has spoken in for the last few minutes: a Snitch nobody sees is a
//! wasted drop, and one that lands in a dead channel reads as the bot talking
//! to itself.
//!
//! Catching is deliberately strict - a reply to the card that says exactly
//! `accio` - and silent for everyone who misses, so a busy catch doesn't bury
//! the channel. Two minutes after the drop the same card is edited to show the
//! Snitch has flown, and nothing counts after that.
//!
//! Points never touch a database here: every catch goes through
//! `house::award_person` with a key naming the Snitch and the catcher, so a
//! restart or a replayed message can never score twice. snitch.db only holds
//! what this module needs to survive a restart - today's plan and the live card.
//!
//! The scheduled drops are off unless `VIZIER_SNITCH_CHANNELS` is set.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions,
    CreateAttachment, CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditAttachments, EditMessage, Message, MessageId,
    MessageReferenceKind,
};

use super::house::House;
use super::points::{Outcome, Source};

/// chatting-hori, then the second channel. The first entry is the home channel
/// a drop falls back to when the channel it rolled has gone quiet.
const DEFAULT_CHANNELS: &[(u64, u32)] = &[(1516492867642593443, 3), (1521136385741029496, 1)];
/// How long a Snitch stays catchable.
const LIFETIME: i64 = 120;
/// The least time between two scheduled drops.
const MIN_GAP: i64 = 2 * 3600;
/// Drops happen between these India times, as seconds after midnight. The day
/// ends early by a Snitch's lifetime so a late card has flown by midnight and its
/// points land on the day it was dropped.
const WINDOW_START: i64 = 10 * 3600;
const WINDOW_END: i64 = 24 * 3600 - LIFETIME;
/// A channel counts as awake if a person spoke in it this recently.
const QUIET_AFTER: i64 = 5 * 60;
/// A scheduled Snitch that flies away uncaught gets a second chance this long
/// after it flew, picked at random - sooner than the two-hour gap.
const REMATCH_MIN: i64 = 20 * 60;
const REMATCH_MAX: i64 = 40 * 60;
/// Second chances a day, so a quiet day can't turn into a Snitch every half hour.
const REMATCHES_PER_DAY: usize = 3;
/// When a drop is due but every channel is quiet, look again after this long.
const RETRY_AFTER: i64 = 3 * 60;
/// How often the scheduler wakes to see whether a drop is due.
const TICK: Duration = Duration::from_secs(30);
/// India is UTC+5:30 all year, so its midnight is plain arithmetic.
const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;
const DAY: i64 = 86_400;
/// How long one Discord call may take. A request that never answers must not
/// hold up the scheduler or a fly-away behind it.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Waits before each attempt at turning a card into "flown away". Losing that
/// edit would leave a card inviting catches that can never score.
const FLY_RETRIES: [u64; 3] = [0, 10, 60];
/// The grey of the flown-away card.
const FLOWN_COLOUR: u32 = 0x4E5058;
const FLOWN_FILE: &str = "flown.png";
const FLOWN_PNG: &[u8] = include_bytes!("snitch/flown.png");

// --- the Snitch itself ------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Bronze,
    Silver,
    Golden,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Bronze, Kind::Silver, Kind::Golden];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Bronze => "bronze",
            Kind::Silver => "silver",
            Kind::Golden => "golden",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        Self::ALL.into_iter().find(|kind| kind.key().eq_ignore_ascii_case(key.trim()))
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Bronze => "Bronze",
            Kind::Silver => "Silver",
            Kind::Golden => "Golden",
        }
    }

    fn colour(self) -> u32 {
        match self {
            Kind::Bronze => 0xB87333,
            Kind::Silver => 0xC7CCD3,
            Kind::Golden => 0xF1C40F,
        }
    }

    /// Points for first, second and third.
    fn points(self) -> [i64; 3] {
        match self {
            Kind::Bronze => [2, 1, 1],
            Kind::Silver => [3, 2, 1],
            Kind::Golden => [6, 4, 2],
        }
    }

    /// The Golden Snitch is the jackpot, so it books to the uncapped source.
    fn source(self) -> Source {
        match self {
            Kind::Golden => Source::GoldenSnitch,
            Kind::Bronze | Kind::Silver => Source::Snitch,
        }
    }

    fn file(self) -> &'static str {
        match self {
            Kind::Bronze => "bronze.png",
            Kind::Silver => "silver.png",
            Kind::Golden => "golden.png",
        }
    }

    fn png(self) -> &'static [u8] {
        match self {
            Kind::Bronze => include_bytes!("snitch/bronze.png"),
            Kind::Silver => include_bytes!("snitch/silver.png"),
            Kind::Golden => include_bytes!("snitch/golden.png"),
        }
    }
}

/// Which Snitch a drop is, from a roll in `[0, 1)`: about 1 in 12 golden, 1 in 4
/// silver, and bronze the rest. At three or four drops a day that is a golden
/// one two or three times a week.
fn choose_kind(roll: f64) -> Kind {
    if roll < 1.0 / 12.0 {
        Kind::Golden
    } else if roll < 1.0 / 12.0 + 1.0 / 4.0 {
        Kind::Silver
    } else {
        Kind::Bronze
    }
}

/// The owner's rule: a reply that says `accio`, any case, and nothing else.
fn is_accio(text: &str) -> bool {
    text.trim().eq_ignore_ascii_case("accio")
}

// --- lines ------------------------------------------------------------------
// One goes in italics on each card. Kept short, light and true, and clear of
// anything anyone could take personally: no politics, faith or real people.

const FACTS: &[&str] = &[
    "Octopuses have three hearts and blue blood.",
    "A group of flamingos is called a flamboyance.",
    "Bananas are berries, but strawberries are not.",
    "Sea otters hold hands while they sleep so they don't drift apart.",
    "A day on Venus is longer than its whole year.",
    "Wombat droppings are shaped like little cubes.",
    "Sharks were swimming around long before trees existed.",
    "The Eiffel Tower grows a few centimetres taller in summer heat.",
    "Hummingbirds are the only birds that can fly backwards.",
    "Koalas have fingerprints that look remarkably like ours.",
    "Butterflies taste their food with their feet.",
    "A bolt of lightning is several times hotter than the surface of the Sun.",
    "Owls can't move their eyes, so they turn their whole heads instead.",
    "Penguins do have knees, tucked away inside their bodies.",
    "Scotland's national animal is the unicorn.",
    "The dot over a lowercase i is called a tittle.",
    "Peanuts aren't nuts at all; they're legumes, like peas.",
    "A fluffy cumulus cloud can weigh as much as a hundred elephants.",
    "At just the right pressure, water can boil and freeze at once.",
    "Crows can remember individual human faces for years.",
    "Honey can stay edible for thousands of years.",
    "A group of owls is called a parliament.",
    "Goats have rectangular pupils, which help them spot danger.",
    "Sloths can hold their breath longer than dolphins can.",
    "Neptune finished its first full orbit since its discovery in 2011.",
    "Apples float because about a quarter of an apple is air.",
    "Male seahorses are the ones who carry the babies.",
    "A shrimp's heart is in its head.",
    "There are more possible games of chess than atoms in the observable universe.",
    "Tardigrades can survive years of drying out, then spring back to life.",
    "The Moon drifts about 4 centimetres further from Earth every year.",
    "Dragonflies can hover in place and even fly backwards.",
    "Honeybees share directions to flowers by dancing.",
    "The first cultivated carrots were purple and yellow, not orange.",
    "Pineapples can take up to two years to grow.",
    "Jellyfish have been drifting through the oceans for over 500 million years.",
    "Cows moo with regional accents, or so some farmers insist.",
    "Seekers recommend quick fingers and a steady keyboard.",
    "Rumour says this one has never been caught. Prove the rumour wrong.",
    "Snitches are famously unimpressed by slow typists.",
    "This Snitch has been practising its loop-the-loops all week.",
    "Tip from a retired Seeker: blink later.",
];

fn pick_fact(roll: f64) -> &'static str {
    let index = ((roll.clamp(0.0, 1.0) * FACTS.len() as f64) as usize).min(FACTS.len().saturating_sub(1));
    FACTS.get(index).copied().unwrap_or("Keep your eyes open.")
}

// --- the card ---------------------------------------------------------------

fn live_title(kind: Kind) -> String {
    format!("A {} Snitch just flew in! ✨", kind.name())
}

fn live_description(fact: &str) -> String {
    format!("*{}*\n\nReply to this message with **ACCIO** to catch it!", fact)
}

fn live_footer(kind: Kind) -> String {
    let [first, second, third] = kind.points();
    format!("First 3 catchers score {} · {} · {} • Flies away in 2 minutes", first, second, third)
}

fn live_embed(kind: Kind, fact: &str) -> CreateEmbed {
    CreateEmbed::new()
        .title(live_title(kind))
        .description(live_description(fact))
        .colour(kind.colour())
        .thumbnail(format!("attachment://{}", kind.file()))
        .footer(CreateEmbedFooter::new(live_footer(kind)))
}

fn flown_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("The Snitch has flown away")
        .description("Missed it? Keep your eyes open — it'll turn up again when you least expect it.")
        .colour(FLOWN_COLOUR)
        .thumbnail(format!("attachment://{}", FLOWN_FILE))
}

fn caught_text(points: i64, house: &House) -> String {
    format!("Accio! **+{}** to {} **{}**", points, house.crest, house.name)
}

// --- catching ---------------------------------------------------------------

/// A Snitch that is still in the air.
#[derive(Clone, Debug)]
struct Flight {
    message: u64,
    channel: u64,
    kind: Kind,
    dropped_at: i64,
    /// People who took a place, in order.
    catchers: Vec<u64>,
    /// People who tried and could not score (capped). Kept so a second
    /// `accio` from them doesn't go back to the ledger.
    tried: HashSet<u64>,
}

/// A catch that scored.
struct Catch {
    place: usize,
    points: i64,
    house: &'static House,
}

impl Flight {
    fn new(message: u64, channel: u64, kind: Kind, dropped_at: i64) -> Flight {
        Flight { message, channel, kind, dropped_at, catchers: Vec::new(), tried: HashSet::new() }
    }

    fn open_at(&self, now: i64) -> bool {
        now >= self.dropped_at && now < self.dropped_at + LIFETIME
    }

    /// Someone replied `accio` at `now`. `award` books the points and says what
    /// the ledger did; a place is only taken when points were actually granted,
    /// so anyone unsorted, stepped out or capped leaves it open for the next.
    fn try_catch(
        &mut self,
        user: u64,
        now: i64,
        award: impl FnOnce(i64) -> Option<(&'static House, Outcome)>,
    ) -> Option<Catch> {
        if !self.open_at(now) || self.catchers.len() >= 3 {
            return None;
        }
        if self.catchers.contains(&user) || self.tried.contains(&user) {
            return None;
        }
        let place = self.catchers.len();
        let points = self.kind.points()[place];
        match award(points)? {
            (house, Outcome::Granted(granted)) if granted > 0 => {
                self.catchers.push(user);
                Some(Catch { place, points: granted, house })
            }
            // Capped, or this exact catch already on the books: no place, and no
            // second trip to the ledger for them on this Snitch.
            _ => {
                self.tried.insert(user);
                None
            }
        }
    }
}

fn dedupe_key(message: u64, user: u64) -> String {
    format!("snitch:{}:{}", message, user)
}

fn ordinal(place: usize) -> &'static str {
    match place {
        0 => "1st",
        1 => "2nd",
        _ => "3rd",
    }
}

// --- channels ---------------------------------------------------------------

/// `id:weight,id:weight`. `None` when the variable is unset, which switches the
/// scheduled drops off. Set but with nothing usable in it (blank, or just `on`)
/// means the two default channels.
fn parse_channels(raw: Option<&str>) -> Option<Vec<(u64, u32)>> {
    let raw = raw?;
    let parsed: Vec<(u64, u32)> = raw
        .split(',')
        .filter_map(|part| {
            let part = part.trim();
            let (id, weight) = match part.split_once(':') {
                Some((id, weight)) => (id.trim(), weight.trim().parse::<u32>().ok()?),
                None => (part, 1),
            };
            let id = id.parse::<u64>().ok()?;
            (weight > 0).then_some((id, weight))
        })
        .collect();
    Some(if parsed.is_empty() { DEFAULT_CHANNELS.to_vec() } else { parsed })
}

fn channels() -> Option<Vec<(u64, u32)>> {
    parse_channels(std::env::var("VIZIER_SNITCH_CHANNELS").ok().as_deref())
}

/// Picks where a drop goes from a roll in `[0, 1)`, by weight. A quiet pick falls
/// back to the home channel (the first); if that is quiet too there is nowhere
/// worth dropping, and the caller tries again later with a fresh roll.
fn choose_channel(channels: &[(u64, u32)], roll: f64, awake: impl Fn(u64) -> bool) -> Option<u64> {
    let total: u64 = channels.iter().map(|(_, w)| *w as u64).sum();
    if total == 0 {
        return None;
    }
    let mut target = (roll.clamp(0.0, 1.0) * total as f64) as u64;
    let mut picked = channels.last().map(|(id, _)| *id)?;
    for (id, weight) in channels {
        if target < *weight as u64 {
            picked = *id;
            break;
        }
        target -= *weight as u64;
    }
    if awake(picked) {
        return Some(picked);
    }
    channels.first().map(|(id, _)| *id).filter(|home| awake(*home))
}

/// When a person last spoke in each channel.
static LAST_SEEN: LazyLock<Mutex<HashMap<u64, i64>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn awake(channel: u64, now: i64) -> bool {
    LAST_SEEN.lock().get(&channel).is_some_and(|seen| now - seen <= QUIET_AFTER)
}

// --- the day's plan ---------------------------------------------------------

/// Midnight India time at the start of the day `ts` falls on.
fn ist_midnight(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(DAY) * DAY - IST_OFFSET
}

fn in_window(ts: i64) -> bool {
    let into_day = ts - ist_midnight(ts);
    (WINDOW_START..=WINDOW_END).contains(&into_day)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    /// The India day, as the ledger writes it.
    day: String,
    /// Drop times, earliest first.
    times: Vec<i64>,
    /// How many of them have dropped.
    done: usize,
    /// The last scheduled drop, whatever day it was on.
    last_drop: Option<i64>,
}

/// `want` drop times between `start` and `end`, at least `MIN_GAP` apart, fewer
/// if the time left can't hold them. Each draw is a point in the spare time
/// after the gaps are set aside, so every spacing that fits is equally likely.
fn spread(start: i64, end: i64, want: usize, mut roll: impl FnMut() -> f64) -> Vec<i64> {
    if end < start {
        return Vec::new();
    }
    let fits = 1 + ((end - start) / MIN_GAP) as usize;
    let n = want.min(fits);
    let slack = (end - start) - MIN_GAP * n.saturating_sub(1) as i64;
    let mut offsets: Vec<i64> = (0..n).map(|_| (roll().clamp(0.0, 1.0) * slack as f64) as i64).collect();
    offsets.sort_unstable();
    offsets.iter().enumerate().map(|(i, offset)| start + (*offset).min(slack) + MIN_GAP * i as i64).collect()
}

/// Today's plan: the stored one if it is today's, otherwise a fresh one. A plan
/// made partway through the day only uses what is left of it, so switching the
/// feature on at 9pm doesn't fire a morning's worth of drops at once.
fn plan_for(now: i64, stored: Option<Plan>, mut roll: impl FnMut() -> f64) -> Plan {
    let day = super::points::ist_day(now);
    let last_drop = stored.as_ref().and_then(|plan| plan.last_drop);
    if let Some(plan) = stored.filter(|plan| plan.day == day) {
        return plan;
    }
    let midnight = ist_midnight(now);
    let want = if roll() < 0.5 { 3 } else { 4 };
    let times = spread((midnight + WINDOW_START).max(now), midnight + WINDOW_END, want, roll);
    Plan { day, times, done: 0, last_drop }
}

/// When a second chance for an uncaught Snitch should drop: a random time
/// between `REMATCH_MIN` and `REMATCH_MAX` after it flew, or none if the day's
/// second chances are used up or the time falls outside the drop hours.
fn rematch_time(flew: i64, used: usize, roll: f64) -> Option<i64> {
    let at = flew + REMATCH_MIN + (roll.clamp(0.0, 1.0) * (REMATCH_MAX - REMATCH_MIN) as f64) as i64;
    (used < REMATCHES_PER_DAY && in_window(at) && ist_midnight(at) == ist_midnight(flew)).then_some(at)
}

/// Whether the next drop should go now. A slot that slipped - quiet chat, a
/// restart - still waits out the full gap after the drop before it, so the day's
/// drops can run late but never bunch up.
fn due(plan: &Plan, now: i64) -> bool {
    let Some(next) = plan.times.get(plan.done) else {
        return false;
    };
    now >= *next && in_window(now) && plan.last_drop.is_none_or(|last| now - last >= MIN_GAP)
}

// --- store ------------------------------------------------------------------

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
/// Snitches that flew away with nobody catching them, for the scheduler to see.
static UNCAUGHT: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Snitches in the air, by message id.
static LIVE: LazyLock<Mutex<HashMap<u64, Flight>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS flights (
        message_id INTEGER PRIMARY KEY, channel_id INTEGER NOT NULL, kind TEXT NOT NULL,
        dropped_at INTEGER NOT NULL, flown INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS flights_flown ON flights (flown);
    CREATE TABLE IF NOT EXISTS catches (
        message_id INTEGER NOT NULL, user_id INTEGER NOT NULL, place INTEGER NOT NULL,
        points INTEGER NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (message_id, user_id));
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("snitch.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(SCHEMA)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

fn load_plan(conn: &Connection) -> Option<Plan> {
    let get = |key: &str| -> Option<String> {
        conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
    };
    let day = get("day")?;
    let times = get("times")
        .unwrap_or_default()
        .split(',')
        .filter_map(|t| t.trim().parse::<i64>().ok())
        .collect();
    let done = get("done").and_then(|d| d.parse().ok()).unwrap_or(0);
    let last_drop = get("last_drop").and_then(|d| d.parse().ok());
    Some(Plan { day, times, done, last_drop })
}

fn save_plan(conn: &Connection, plan: &Plan) -> rusqlite::Result<()> {
    let times = plan.times.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(",");
    let last = plan.last_drop.map(|t| t.to_string()).unwrap_or_default();
    let tx = conn.unchecked_transaction()?;
    let rows = [("day", plan.day.clone()), ("times", times), ("done", plan.done.to_string()), ("last_drop", last)];
    for (key, value) in rows {
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    tx.commit()
}

fn save_flight(conn: &Connection, flight: &Flight) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO flights (message_id, channel_id, kind, dropped_at) VALUES (?1, ?2, ?3, ?4)",
        params![flight.message as i64, flight.channel as i64, flight.kind.key(), flight.dropped_at],
    )
    .map(|_| ())
}

fn save_catch(conn: &Connection, message: u64, user: u64, catch: &Catch, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO catches (message_id, user_id, place, points, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![message as i64, user as i64, catch.place as i64, catch.points, now],
    )
    .map(|_| ())
}

fn mark_flown(conn: &Connection, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE flights SET flown = 1 WHERE message_id = ?1", params![message as i64]).map(|_| ())
}

/// Every card not yet turned into "flown away", with its catchers in order.
fn unflown(conn: &Connection) -> rusqlite::Result<Vec<Flight>> {
    let mut stmt = conn.prepare("SELECT message_id, channel_id, kind, dropped_at FROM flights WHERE flown = 0")?;
    let rows: Vec<(i64, i64, String, i64)> =
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.flatten().collect();
    let mut out = Vec::new();
    for (message, channel, kind, dropped_at) in rows {
        // An unknown kind can only come from a future version; it still has to fly.
        let kind = Kind::from_key(&kind).unwrap_or(Kind::Bronze);
        let mut flight = Flight::new(message as u64, channel as u64, kind, dropped_at);
        let mut stmt = conn.prepare("SELECT user_id FROM catches WHERE message_id = ?1 ORDER BY place")?;
        let catchers = stmt.query_map(params![message], |r| r.get::<_, i64>(0))?;
        flight.catchers = catchers.flatten().map(|u| u as u64).collect();
        out.push(flight);
    }
    Ok(out)
}

fn is_snitch(conn: &Connection, message: u64) -> bool {
    conn.query_row("SELECT 1 FROM flights WHERE message_id = ?1", params![message as i64], |_| Ok(()))
        .optional()
        .ok()
        .flatten()
        .is_some()
}

// --- Discord ----------------------------------------------------------------

/// Every Discord call goes through here, so one hung request can't wedge the
/// scheduler or leave a card live.
async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, String> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!("no answer from Discord in {}s", HTTP_WAIT.as_secs())),
    }
}

/// Records that a person spoke, so drops only go where someone is around.
/// Call it for every server message, before any channel filtering.
pub fn note_message(msg: &Message) {
    if msg.author.bot || msg.guild_id.is_none() {
        return;
    }
    LAST_SEEN.lock().insert(msg.channel_id.get(), Utc::now().timestamp());
}

/// Posts a Snitch and puts it in the air. `None` if Discord refused it.
async fn release(ctx: &Context, channel: ChannelId, kind: Kind) -> Option<Flight> {
    let fact = pick_fact(rand::random::<f64>());
    let message = CreateMessage::new()
        .embed(live_embed(kind, fact))
        .add_file(CreateAttachment::bytes(kind.png(), kind.file()))
        .allowed_mentions(CreateAllowedMentions::new());
    let posted = match call(channel.send_message(&ctx.http, message)).await {
        Ok(posted) => posted,
        Err(err) => {
            tracing::warn!("snitch: {} Snitch not dropped in {}: {}", kind.key(), channel, err);
            return None;
        }
    };
    let flight = Flight::new(posted.id.get(), channel.get(), kind, Utc::now().timestamp());
    if let Some(db) = DB.get() {
        if let Err(err) = save_flight(&db.lock(), &flight) {
            tracing::warn!("snitch: drop {} not saved: {}", flight.message, err);
        }
    }
    LIVE.lock().insert(flight.message, flight.clone());
    arm(ctx.clone(), flight.message, flight.channel, flight.dropped_at);
    tracing::info!("snitch: {} Snitch dropped in {} ({})", kind.key(), channel, flight.message);
    Some(flight)
}

/// Flies the Snitch away exactly `LIFETIME` after its drop.
fn arm(ctx: Context, message: u64, channel: u64, dropped_at: i64) {
    tokio::spawn(async move {
        let wait = (dropped_at + LIFETIME - Utc::now().timestamp()).max(0) as u64;
        tokio::time::sleep(Duration::from_secs(wait)).await;
        fly_away(&ctx, message, channel, false).await;
    });
}

/// Closes a Snitch and edits its card. Catching is closed first, in memory, so
/// nothing scores while Discord is slow. A failed edit is left for the next
/// start to try again, unless this already is that start - a card whose message
/// was deleted must not be retried on every restart forever.
async fn fly_away(ctx: &Context, message: u64, channel: u64, give_up_after: bool) {
    let flown = LIVE.lock().remove(&message);
    if flown.is_some_and(|f| f.catchers.is_empty()) {
        UNCAUGHT.lock().insert(message);
    }
    let mut edited = false;
    for wait in FLY_RETRIES {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        let edit = EditMessage::new()
            .embed(flown_embed())
            .attachments(EditAttachments::new().add(CreateAttachment::bytes(FLOWN_PNG, FLOWN_FILE)));
        match call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
            Ok(_) => {
                edited = true;
                break;
            }
            Err(err) => tracing::warn!("snitch: card {} not flown away yet: {}", message, err),
        }
    }
    if edited || give_up_after {
        if let Some(db) = DB.get() {
            if let Err(err) = mark_flown(&db.lock(), message) {
                tracing::warn!("snitch: card {} not marked flown: {}", message, err);
            }
        }
    }
}

/// A reply to a Snitch. Returns true when the message was an `accio` aimed at a
/// Snitch card - live or already flown - so the caller stops there; anything
/// else goes on through the bot untouched. Misses, repeats and late catches
/// get no answer at all, by the owner's wish.
///
/// Call it before the channel allowlist: drops can land in channels the chat
/// side of the bot doesn't read.
pub async fn on_message(ctx: &Context, msg: &Message) -> bool {
    if msg.author.bot || msg.guild_id.is_none() || !is_accio(&msg.content) {
        return false;
    }
    let Some(reference) = msg.message_reference.as_ref() else {
        return false;
    };
    // A forward also carries a reference, but only a reply is a catch.
    if reference.kind != MessageReferenceKind::Default || reference.channel_id != msg.channel_id {
        return false;
    }
    let Some(target) = reference.message_id.map(|id| id.get()) else {
        return false;
    };
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();

    let caught = {
        let mut live = LIVE.lock();
        let Some(flight) = live.get_mut(&target) else {
            // Not in the air: a late accio on a flown card is still swallowed
            // quietly, while an accio to any other message carries on as chat.
            drop(live);
            return DB.get().is_some_and(|db| is_snitch(&db.lock(), target));
        };
        let kind = flight.kind;
        let caught = flight.try_catch(user, now, |points| {
            let reason = format!("{} Snitch", kind.name());
            super::house::award_person(user, kind.source(), points, &reason, None, Some(dedupe_key(target, user)), None)
        });
        // Saved under the same lock, so a restart straight after can't hand this
        // place to someone else.
        if let (Some(catch), Some(db)) = (&caught, DB.get()) {
            if let Err(err) = save_catch(&db.lock(), target, user, catch, now) {
                tracing::warn!("snitch: catch by {} on {} not saved: {}", user, target, err);
            }
        }
        caught.map(|catch| (kind, catch))
    };

    if let Some((kind, catch)) = caught {
        tracing::info!(
            "snitch: {} caught the {} Snitch {} ({}, +{} to {})",
            user,
            kind.key(),
            target,
            ordinal(catch.place),
            catch.points,
            catch.house.name
        );
        let reply = CreateMessage::new()
            .content(caught_text(catch.points, catch.house))
            .reference_message(msg)
            .allowed_mentions(CreateAllowedMentions::new());
        if let Err(err) = call(msg.channel_id.send_message(&ctx.http, reply)).await {
            tracing::warn!("snitch: catch reply to {} not sent: {}", user, err);
        }
    }
    true
}

/// Brings back cards from before a restart, then runs the day's drops. Safe to
/// call on every `ready`: it only starts once per run.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    recover(&ctx);
    drop_once(&ctx);
    match channels() {
        Some(channels) => {
            tracing::info!("snitch: dropping into {:?}", channels);
            tokio::spawn(schedule(ctx, channels));
        }
        None => tracing::info!("snitch: VIZIER_SNITCH_CHANNELS not set, no scheduled drops"),
    }
}

/// A one-off drop asked for from outside the bot: a channel id stored under the
/// `drop_once` meta key is taken, cleared, and gets a Snitch shortly after start.
/// Like `/snitchdrop`, it doesn't use up a scheduled drop.
fn drop_once(ctx: &Context) {
    let Some(db) = DB.get() else {
        return;
    };
    let channel = {
        let conn = db.lock();
        let value: Option<String> =
            conn.query_row("SELECT value FROM meta WHERE key = 'drop_once'", [], |r| r.get(0)).optional().ok().flatten();
        let _ = conn.execute("DELETE FROM meta WHERE key = 'drop_once'", []);
        value.and_then(|v| v.trim().parse::<u64>().ok())
    };
    let Some(channel) = channel else {
        return;
    };
    let ctx = ctx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let kind = choose_kind(rand::random::<f64>());
        tracing::info!("snitch: one-off {} drop asked for in {}", kind.key(), channel);
        let _ = release(&ctx, ChannelId::new(channel), kind).await;
    });
}

/// Cards left live by a restart. One past its two minutes flies away now; one
/// still in the air picks up where it was, catchers and all.
fn recover(ctx: &Context) {
    let Some(db) = DB.get() else {
        return;
    };
    let loaded = unflown(&db.lock());
    let flights = match loaded {
        Ok(flights) => flights,
        Err(err) => {
            tracing::warn!("snitch: live cards not loaded: {}", err);
            return;
        }
    };
    let now = Utc::now().timestamp();
    for flight in flights {
        if flight.open_at(now) {
            arm(ctx.clone(), flight.message, flight.channel, flight.dropped_at);
            LIVE.lock().insert(flight.message, flight);
        } else {
            let ctx = ctx.clone();
            tokio::spawn(async move { fly_away(&ctx, flight.message, flight.channel, true).await });
        }
    }
}

async fn schedule(ctx: Context, channels: Vec<(u64, u32)>) {
    let mut plan: Option<Plan> = DB.get().and_then(|db| load_plan(&db.lock()));
    let mut retry_at = 0i64;
    // Second chances live in memory: a restart just skips a pending one.
    let mut watching: Option<u64> = None;
    let mut rematch_at: Option<i64> = None;
    let mut rematches = (String::new(), 0usize);
    // Any drop, scheduled or second chance, so the two never land back to back.
    let mut last_any = 0i64;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let before = plan.as_ref().map(|p| p.day.clone());
        let stored = plan.take();
        let current = plan.insert(plan_for(now, stored, rand::random::<f64>));
        if before.as_deref() != Some(current.day.as_str()) {
            tracing::info!("snitch: {} drops planned for {}", current.times.len(), current.day);
            persist(current);
        }
        if rematches.0 != current.day {
            rematches = (current.day.clone(), 0);
        }
        {
            // Mods' test drops land here too, and nobody waits on them.
            let mut uncaught = UNCAUGHT.lock();
            if uncaught.len() > 50 {
                uncaught.retain(|id| Some(*id) == watching);
            }
        }
        if let Some(id) = watching.filter(|id| UNCAUGHT.lock().remove(id)) {
            watching = None;
            rematch_at = rematch_time(now, rematches.1, rand::random::<f64>());
            match rematch_at {
                Some(at) => tracing::info!("snitch: {} flew away uncaught, another comes in {} min", id, (at - now) / 60),
                None => tracing::info!("snitch: {} flew away uncaught, no second chance left today", id),
            }
        }
        if now < retry_at {
            continue;
        }
        let rematch_due = rematch_at.is_some_and(|at| now >= at && in_window(now));
        let scheduled_due = due(current, now) && now - last_any >= REMATCH_MIN;
        if rematch_at.is_some_and(|at| !in_window(at.max(now))) {
            rematch_at = None;
        }
        if !rematch_due && !scheduled_due {
            continue;
        }
        // A channel with a card already in the air counts as unavailable, so a
        // mod's test drop and a scheduled one never share a channel.
        let busy: HashSet<u64> = LIVE.lock().values().map(|f| f.channel).collect();
        let Some(channel) = choose_channel(&channels, rand::random::<f64>(), |id| awake(id, now) && !busy.contains(&id))
        else {
            retry_at = now + RETRY_AFTER;
            continue;
        };
        match release(&ctx, ChannelId::new(channel), choose_kind(rand::random::<f64>())).await {
            Some(flight) => {
                // A scheduled drop takes the place of any pending second chance.
                if scheduled_due {
                    current.done += 1;
                    current.last_drop = Some(flight.dropped_at);
                    persist(current);
                } else {
                    rematches.1 += 1;
                }
                rematch_at = None;
                watching = Some(flight.message);
                last_any = flight.dropped_at;
            }
            None => retry_at = now + RETRY_AFTER,
        }
    }
}

fn persist(plan: &Plan) {
    if let Some(db) = DB.get() {
        if let Err(err) = save_plan(&db.lock(), plan) {
            tracing::warn!("snitch: plan for {} not saved: {}", plan.day, err);
        }
    }
}

/// The `/snitchdrop` command, to register.
pub fn command() -> CreateCommand {
    let mut kind = CreateCommandOption::new(CommandOptionType::String, "type", "which Snitch (random if left out)");
    for k in Kind::ALL {
        kind = kind.add_string_choice(k.name(), k.key());
    }
    CreateCommand::new("snitchdrop").description("admin only: drop a Snitch right here, right now").add_option(kind)
}

/// `/snitchdrop [type]` - admins only. Drops one in the channel it is run in,
/// straight away. It is a real Snitch under the same catch rules, and it doesn't
/// use up a scheduled drop or reset the gap between them.
pub async fn drop_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if command.guild_id.is_none() {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.")).await;
        return;
    }
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can drop a Snitch.")).await;
        return;
    }
    let kind = command
        .data
        .options
        .iter()
        .find_map(|o| match (&o.name[..], &o.value) {
            ("type", CommandDataOptionValue::String(key)) => Kind::from_key(key),
            _ => None,
        })
        .unwrap_or_else(|| choose_kind(rand::random::<f64>()));
    let text = format!("Releasing a {} Snitch.", kind.name());
    let _ = command.create_response(&ctx.http, whisper(&text)).await;
    if release(ctx, command.channel_id, kind).await.is_none() {
        let _ = command
            .create_followup(
                &ctx.http,
                serenity::all::CreateInteractionResponseFollowup::new()
                    .content("Discord wouldn't take the Snitch - check my permissions here.")
                    .ephemeral(true),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::super::house::HOUSES;
    use super::*;

    /// 2026-09-14 00:00 India time.
    const MIDNIGHT: i64 = 1_789_324_200;
    const HOUR: i64 = 3600;

    fn xorshift(seed: u64) -> impl FnMut() -> f64 {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    #[test]
    fn the_test_midnight_really_is_india_midnight() {
        assert_eq!(super::super::points::ist_day(MIDNIGHT), "2026-09-14");
        assert_eq!(super::super::points::ist_day(MIDNIGHT - 1), "2026-09-13");
        assert_eq!(ist_midnight(MIDNIGHT + 23 * HOUR), MIDNIGHT);
        assert_eq!(ist_midnight(MIDNIGHT - 1), MIDNIGHT - DAY);
    }

    #[test]
    fn only_a_bare_accio_counts() {
        for yes in ["accio", "ACCIO", "Accio", " Accio ", "aCcIo", "\taccio\n", "  ACCIO  "] {
            assert!(is_accio(yes), "{:?} should catch", yes);
        }
        for no in [
            "", " ", "accio!", "accio.", "acc io", "accio pls", "accio accio", "!accio", "accios", "acci",
            "accio snitch", "**accio**", "`accio`", "'accio'", "accio?", "a c c i o", "accio\nhi", "hi accio",
        ] {
            assert!(!is_accio(no), "{:?} should not catch", no);
        }
    }

    #[test]
    fn the_second_channel_gets_about_one_drop_in_four() {
        let mut counts: HashMap<u64, usize> = HashMap::new();
        for i in 0..4000 {
            let roll = i as f64 / 4000.0;
            let picked = choose_channel(DEFAULT_CHANNELS, roll, |_| true).expect("both awake");
            *counts.entry(picked).or_default() += 1;
        }
        assert_eq!(counts[&DEFAULT_CHANNELS[0].0], 3000);
        assert_eq!(counts[&DEFAULT_CHANNELS[1].0], 1000);
        // The very top of the range still lands on a channel.
        assert!(choose_channel(DEFAULT_CHANNELS, 0.999_999, |_| true).is_some());
        assert!(choose_channel(DEFAULT_CHANNELS, 1.0, |_| true).is_some());
    }

    #[test]
    fn a_quiet_channel_hands_the_drop_to_the_home_channel_or_nobody() {
        let (home, second) = (DEFAULT_CHANNELS[0].0, DEFAULT_CHANNELS[1].0);
        let rolled_second = 0.9;
        let rolled_home = 0.1;
        // The second channel rolled but is asleep: chatting-hori takes it.
        assert_eq!(choose_channel(DEFAULT_CHANNELS, rolled_second, |id| id == home), Some(home));
        // Both asleep: nothing, and the scheduler retries later.
        assert_eq!(choose_channel(DEFAULT_CHANNELS, rolled_second, |_| false), None);
        assert_eq!(choose_channel(DEFAULT_CHANNELS, rolled_home, |_| false), None);
        // Home rolled but asleep: the home fallback is itself, so wait for a re-roll.
        assert_eq!(choose_channel(DEFAULT_CHANNELS, rolled_home, |id| id == second), None);
        assert_eq!(choose_channel(DEFAULT_CHANNELS, rolled_second, |id| id == second), Some(second));
        assert_eq!(choose_channel(&[], 0.5, |_| true), None);
    }

    #[test]
    fn the_channel_setting_is_off_when_unset_and_defaults_when_blank() {
        assert_eq!(parse_channels(None), None);
        assert_eq!(parse_channels(Some("")), Some(DEFAULT_CHANNELS.to_vec()));
        assert_eq!(parse_channels(Some("on")), Some(DEFAULT_CHANNELS.to_vec()));
        assert_eq!(parse_channels(Some("11:5, 22:2")), Some(vec![(11, 5), (22, 2)]));
        assert_eq!(parse_channels(Some("11, 22:0, x:3, 33:y, 44:2")), Some(vec![(11, 1), (44, 2)]));
        assert_eq!(
            parse_channels(Some("1516492867642593443:3,1521136385741029496:1")),
            Some(DEFAULT_CHANNELS.to_vec())
        );
    }

    #[test]
    fn bronze_is_common_silver_one_in_four_golden_rare() {
        let n = 12_000;
        let mut counts: HashMap<Kind, usize> = HashMap::new();
        for i in 0..n {
            *counts.entry(choose_kind(i as f64 / n as f64)).or_default() += 1;
        }
        assert_eq!(counts[&Kind::Golden], 1000);
        assert_eq!(counts[&Kind::Silver], 3000);
        assert_eq!(counts[&Kind::Bronze], 8000);
        // And with real-looking randomness, inside a comfortable band.
        let mut roll = xorshift(42);
        let mut golden = 0;
        for _ in 0..n {
            if choose_kind(roll()) == Kind::Golden {
                golden += 1;
            }
        }
        assert!((800..1200).contains(&golden), "golden came up {} times in {}", golden, n);
    }

    #[test]
    fn each_kind_pays_and_books_as_the_owner_set() {
        assert_eq!(Kind::Bronze.points(), [2, 1, 1]);
        assert_eq!(Kind::Silver.points(), [3, 2, 1]);
        assert_eq!(Kind::Golden.points(), [6, 4, 2]);
        assert_eq!(Kind::Bronze.source(), Source::Snitch);
        assert_eq!(Kind::Silver.source(), Source::Snitch);
        assert_eq!(Kind::Golden.source(), Source::GoldenSnitch);
        for kind in Kind::ALL {
            assert_eq!(Kind::from_key(kind.key()), Some(kind));
            assert!(!kind.png().is_empty());
        }
        assert_eq!(Kind::from_key(" GOLDEN "), Some(Kind::Golden));
        assert_eq!(Kind::from_key("platinum"), None);
    }

    #[test]
    fn a_day_gets_three_or_four_drops_two_hours_apart_inside_the_window() {
        let mut threes = 0;
        for seed in 1..500u64 {
            let plan = plan_for(MIDNIGHT + HOUR, None, xorshift(seed));
            let n = plan.times.len();
            assert!((3..=4).contains(&n), "seed {} planned {}", seed, n);
            threes += usize::from(n == 3);
            for t in &plan.times {
                assert!(in_window(*t), "seed {}: {} is outside 10:00-24:00", seed, (t - MIDNIGHT) / 60);
                assert!(*t >= MIDNIGHT + 10 * HOUR && *t + LIFETIME <= MIDNIGHT + DAY);
            }
            for pair in plan.times.windows(2) {
                assert!(pair[1] - pair[0] >= MIN_GAP, "seed {}: drops {}s apart", seed, pair[1] - pair[0]);
            }
        }
        assert!((150..350).contains(&threes), "three-drop days: {}", threes);
    }

    #[test]
    fn a_plan_made_late_in_the_day_only_uses_what_is_left() {
        let now = MIDNIGHT + 21 * HOUR;
        let plan = plan_for(now, None, || 0.99);
        assert_eq!(plan.times.len(), 2, "21:00 to 23:58 holds two drops, not four");
        assert!(plan.times.iter().all(|t| *t >= now));
        assert!(plan_for(MIDNIGHT + 23 * HOUR + 59 * 60, None, || 0.5).times.is_empty());
        // Before the window opens it is a whole day's plan.
        assert_eq!(plan_for(MIDNIGHT + 3 * HOUR, None, || 0.99).times.len(), 4);
    }

    #[test]
    fn the_same_day_keeps_its_plan_and_a_new_day_starts_fresh() {
        let plan = Plan {
            day: "2026-09-14".into(),
            times: vec![MIDNIGHT + 11 * HOUR, MIDNIGHT + 15 * HOUR, MIDNIGHT + 20 * HOUR],
            done: 2,
            last_drop: Some(MIDNIGHT + 15 * HOUR),
        };
        assert_eq!(plan_for(MIDNIGHT + 16 * HOUR, Some(plan.clone()), || 0.3), plan);
        let tomorrow = plan_for(MIDNIGHT + DAY + HOUR, Some(plan), || 0.3);
        assert_eq!(tomorrow.day, "2026-09-15");
        assert_eq!(tomorrow.done, 0);
        assert_eq!(tomorrow.last_drop, Some(MIDNIGHT + 15 * HOUR), "the gap is remembered across midnight");
    }

    #[test]
    fn an_uncaught_snitch_gets_a_second_chance_within_the_hour() {
        let noon = MIDNIGHT + 12 * HOUR;
        assert_eq!(rematch_time(noon, 0, 0.0), Some(noon + REMATCH_MIN));
        assert_eq!(rematch_time(noon, 0, 1.0), Some(noon + REMATCH_MAX));
        assert_eq!(rematch_time(noon, REMATCHES_PER_DAY, 0.5), None, "the day's second chances are used up");
        assert_eq!(rematch_time(MIDNIGHT + 23 * HOUR + 50 * 60, 0, 0.0), None, "too late: past the drop hours");
        assert_eq!(rematch_time(MIDNIGHT + 9 * HOUR, 0, 0.0), None, "before the drop hours");
    }

    #[test]
    fn a_late_drop_pushes_the_next_one_back_to_keep_the_gap() {
        let mut plan = Plan {
            day: "2026-09-14".into(),
            times: vec![MIDNIGHT + 11 * HOUR, MIDNIGHT + 13 * HOUR],
            done: 0,
            last_drop: None,
        };
        assert!(!due(&plan, MIDNIGHT + 10 * HOUR));
        assert!(due(&plan, MIDNIGHT + 11 * HOUR));
        // The first one slipped to 12:30 on quiet chat.
        plan.done = 1;
        plan.last_drop = Some(MIDNIGHT + 12 * HOUR + 30 * 60);
        assert!(!due(&plan, MIDNIGHT + 13 * HOUR), "only half an hour after the last drop");
        assert!(!due(&plan, MIDNIGHT + 14 * HOUR + 29 * 60));
        assert!(due(&plan, MIDNIGHT + 14 * HOUR + 30 * 60));
        plan.done = 2;
        assert!(!due(&plan, MIDNIGHT + 18 * HOUR), "the day's drops are all used");

        // A slot the window closed on is simply lost.
        let late = Plan { day: "2026-09-14".into(), times: vec![MIDNIGHT + 23 * HOUR], done: 0, last_drop: None };
        assert!(due(&late, MIDNIGHT + 23 * HOUR + 50 * 60));
        assert!(!due(&late, MIDNIGHT + DAY - 60));
        assert!(!due(&late, MIDNIGHT + DAY + 5 * 60), "just after midnight is outside the window");
    }

    #[test]
    fn the_window_is_ten_am_to_midnight_india_time() {
        assert!(!in_window(MIDNIGHT + 10 * HOUR - 1));
        assert!(in_window(MIDNIGHT + 10 * HOUR));
        assert!(in_window(MIDNIGHT + 23 * HOUR + 58 * 60));
        assert!(!in_window(MIDNIGHT + DAY - 1));
        assert!(!in_window(MIDNIGHT + 3 * HOUR));
    }

    fn house(i: usize) -> &'static House {
        &HOUSES[i]
    }

    #[test]
    fn places_go_to_the_first_three_who_actually_score() {
        let mut flight = Flight::new(900, 1, Kind::Silver, MIDNIGHT);
        let now = MIDNIGHT + 10;
        let mut asked: Vec<i64> = Vec::new();

        // A mod or unsorted member: nothing, place stays open.
        assert!(flight.try_catch(10, now, |_| None).is_none());
        // First place pays 3.
        let first = flight
            .try_catch(11, now, |p| {
                asked.push(p);
                Some((house(0), Outcome::Granted(p)))
            })
            .expect("first scores");
        assert_eq!((first.place, first.points, first.house.name), (0, 3, "Gryffindor"));
        // Same person again: ignored, and the ledger isn't even asked.
        assert!(flight.try_catch(11, now, |_| panic!("no second trip to the ledger")).is_none());
        // Capped: no place, and the next person still gets second.
        assert!(flight.try_catch(12, now, |_| Some((house(1), Outcome::Capped))).is_none());
        assert!(flight.try_catch(12, now, |_| panic!("capped once is enough")).is_none());
        assert!(flight.try_catch(13, now, |_| Some((house(1), Outcome::Duplicate))).is_none());
        let second = flight
            .try_catch(14, now, |p| {
                asked.push(p);
                Some((house(2), Outcome::Granted(p)))
            })
            .expect("second scores");
        assert_eq!((second.place, second.points), (1, 2));
        // Nearly capped: the ledger trims third place's point, but it still counts.
        let third = flight
            .try_catch(15, now, |p| {
                asked.push(p);
                Some((house(3), Outcome::Granted(p)))
            })
            .expect("third scores");
        assert_eq!((third.place, third.points), (2, 1));
        assert_eq!(asked, vec![3, 2, 1]);
        // Full.
        assert!(flight.try_catch(16, now, |_| panic!("no fourth place")).is_none());
        assert_eq!(flight.catchers, vec![11, 14, 15]);
    }

    #[test]
    fn a_partly_capped_catch_takes_the_place_with_what_fits() {
        let mut flight = Flight::new(1, 1, Kind::Bronze, MIDNIGHT);
        let catch = flight.try_catch(7, MIDNIGHT, |_| Some((house(0), Outcome::Granted(1)))).expect("scores");
        assert_eq!((catch.place, catch.points), (0, 1));
        assert!(flight.try_catch(8, MIDNIGHT, |_| Some((house(0), Outcome::Granted(0)))).is_none());
        assert_eq!(flight.catchers, vec![7]);
    }

    #[test]
    fn nothing_scores_once_two_minutes_are_up() {
        let mut flight = Flight::new(1, 1, Kind::Golden, MIDNIGHT);
        let paid = |_| Some((house(0), Outcome::Granted(6)));
        assert!(flight.try_catch(1, MIDNIGHT + LIFETIME, paid).is_none());
        assert!(flight.try_catch(1, MIDNIGHT - 1, paid).is_none());
        assert!(flight.try_catch(1, MIDNIGHT + LIFETIME - 1, paid).is_some());
    }

    #[test]
    fn the_dedupe_key_names_the_snitch_and_the_catcher() {
        assert_eq!(dedupe_key(1_521_000_000_000_000_001, 42), "snitch:1521000000000000001:42");
    }

    #[test]
    fn the_fact_pool_is_big_short_and_has_no_repeats() {
        assert!(FACTS.len() >= 40, "only {} lines", FACTS.len());
        let mut seen = HashSet::new();
        for fact in FACTS {
            assert!(!fact.trim().is_empty());
            assert!(fact.chars().count() <= 90, "too long: {}", fact);
            // They sit inside *italics*: stray markdown would break the card.
            assert!(!fact.contains(['*', '_', '~', '`', '|', '<', '@']), "markdown in: {}", fact);
            assert!(seen.insert(fact.to_lowercase()), "duplicate: {}", fact);
        }
        assert_eq!(pick_fact(0.0), FACTS[0]);
        assert_eq!(pick_fact(1.0), FACTS[FACTS.len() - 1]);
    }

    #[test]
    fn the_cards_read_exactly_as_the_owner_wrote_them() {
        assert_eq!(live_title(Kind::Bronze), "A Bronze Snitch just flew in! ✨");
        assert_eq!(live_title(Kind::Golden), "A Golden Snitch just flew in! ✨");
        assert_eq!(live_footer(Kind::Bronze), "First 3 catchers score 2 · 1 · 1 • Flies away in 2 minutes");
        assert_eq!(live_footer(Kind::Silver), "First 3 catchers score 3 · 2 · 1 • Flies away in 2 minutes");
        assert_eq!(live_footer(Kind::Golden), "First 3 catchers score 6 · 4 · 2 • Flies away in 2 minutes");
        assert_eq!(
            live_description("Owls turn their heads."),
            "*Owls turn their heads.*\n\nReply to this message with **ACCIO** to catch it!"
        );
        assert_eq!(caught_text(3, house(0)), "Accio! **+3** to 🦁 **Gryffindor**");

        let live = serde_json::to_value(live_embed(Kind::Silver, "x")).expect("embed json");
        assert_eq!(live["color"], 0xC7CCD3);
        assert_eq!(live["thumbnail"]["url"], "attachment://silver.png");
        let flown = serde_json::to_value(flown_embed()).expect("embed json");
        assert_eq!(flown["title"], "The Snitch has flown away");
        assert_eq!(flown["color"], 0x4E5058);
        assert_eq!(flown["thumbnail"]["url"], "attachment://flown.png");
        assert_eq!(Kind::Bronze.colour(), 0xB87333);
        assert_eq!(Kind::Golden.colour(), 0xF1C40F);
    }

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(SCHEMA).expect("schema");
        conn
    }

    #[test]
    fn the_plan_and_the_live_card_survive_a_restart() {
        let conn = db();
        assert_eq!(load_plan(&conn), None);
        let plan = Plan {
            day: "2026-09-14".into(),
            times: vec![MIDNIGHT + 11 * HOUR, MIDNIGHT + 14 * HOUR],
            done: 1,
            last_drop: Some(MIDNIGHT + 11 * HOUR + 60),
        };
        save_plan(&conn, &plan).expect("save");
        assert_eq!(load_plan(&conn), Some(plan.clone()));
        let fresh = Plan { day: "2026-09-15".into(), times: vec![], done: 0, last_drop: None };
        save_plan(&conn, &fresh).expect("save again");
        assert_eq!(load_plan(&conn), Some(fresh));

        let flight = Flight::new(555, 77, Kind::Golden, MIDNIGHT + 12 * HOUR);
        save_flight(&conn, &flight).expect("flight");
        let catch = |place| Catch { place, points: 1, house: house(0) };
        save_catch(&conn, 555, 9, &catch(1), 0).expect("catch");
        save_catch(&conn, 555, 8, &catch(0), 0).expect("catch");
        save_catch(&conn, 555, 8, &catch(0), 0).expect("a repeat is ignored, not an error");
        let back = unflown(&conn).expect("load");
        assert_eq!(back.len(), 1);
        assert_eq!((back[0].message, back[0].channel, back[0].kind), (555, 77, Kind::Golden));
        assert_eq!(back[0].catchers, vec![8, 9], "in place order");
        assert!(is_snitch(&conn, 555));
        assert!(!is_snitch(&conn, 556));

        mark_flown(&conn, 555).expect("flown");
        assert!(unflown(&conn).expect("load").is_empty());
        assert!(is_snitch(&conn, 555), "a flown card is still known, so late accios stay quiet");
    }
}
