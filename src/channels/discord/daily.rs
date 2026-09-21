//! Daily posts: twice a day (by default), each topic channel gets one long card
//! written by the AI from a real source - a Wikipedia article, a news article
//! from a feed, or a hand-picked poem or quote - with the source linked on the
//! card. The model is told to use nothing but the source, and a second call
//! checks the draft against it; a draft that still claims things the source
//! doesn't say after one correction is thrown away rather than posted.
//!
//! The first time the bot starts with a desk it has never posted for, that
//! desk posts once straight away, so every channel gets a card on the day this
//! ships instead of waiting for its first time.
//!
//! Slots are claimed in `daily.db` before anything is written, so a restart
//! never posts the same slot twice, and a failed slot is tried again a few
//! times before it is given up.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use chrono::{Datelike, TimeZone, Utc};
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateEmbedAuthor, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseFollowup,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, Http,
};

use super::control;
use super::daily_bank as bank;
use super::daily_sources as src;
use super::sudoku_gen::Rng;
use crate::dependencies::VizierDependencies;

// --- the desks --------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Desk {
    History,
    Movies,
    Tech,
    Music,
    Poetry,
    Books,
    Gaming,
    Sports,
    Fitness,
}

/// One topic channel and the settings that drive it.
pub struct DeskInfo {
    pub desk: Desk,
    /// Stored and used as the /dailypost choice.
    pub key: &'static str,
    pub title: &'static str,
    /// The line above the card's title.
    pub banner: &'static str,
    pub on_key: &'static str,
    pub channel_key: &'static str,
    pub times_key: &'static str,
    /// The panel's labels for the channel and times settings.
    pub channel_label: &'static str,
    pub times_label: &'static str,
    pub channel: &'static str,
    pub times: &'static str,
    pub colour: u32,
    /// What the panel says it posts.
    pub about: &'static str,
}

pub const DESKS: &[DeskInfo] = &[
    DeskInfo {
        desk: Desk::History,
        key: "history",
        title: "History",
        channel_label: "History: channel",
        times_label: "History: times",
        banner: "📜 A story from history",
        on_key: "VIZIER_DAILY_HISTORY",
        channel_key: "VIZIER_DAILY_HISTORY_CHANNEL",
        times_key: "VIZIER_DAILY_HISTORY_TIMES",
        channel: "1551291423394111599",
        times: "11:00,19:00",
        colour: 0xB08D57,
        about: "One true story from one Wikipedia article: mostly Indian history from a hand-picked list, and every third post a story tied to today's date.",
    },
    DeskInfo {
        desk: Desk::Movies,
        key: "movies",
        title: "TV & movies",
        channel_label: "TV & movies: channel",
        times_label: "TV & movies: times",
        banner: "🎬 Behind the screen",
        on_key: "VIZIER_DAILY_MOVIES",
        channel_key: "VIZIER_DAILY_MOVIES_CHANNEL",
        times_key: "VIZIER_DAILY_MOVIES_TIMES",
        channel: "1518241428231426098",
        times: "11:00,19:00",
        colour: 0xE50914,
        about: "Alternates: a behind-the-scenes story of a film or show from the Guess the Movie bank (from its Wikipedia article), and news - an upcoming release, a first look or a controversy - from Variety, Deadline, The Hollywood Reporter and Indian Express.",
    },
    DeskInfo {
        desk: Desk::Tech,
        key: "tech",
        title: "Tech",
        channel_label: "Tech: channel",
        times_label: "Tech: times",
        banner: "💻 Tech worth knowing",
        on_key: "VIZIER_DAILY_TECH",
        channel_key: "VIZIER_DAILY_TECH_CHANNEL",
        times_key: "VIZIER_DAILY_TECH_TIMES",
        channel: "1539692322726613065",
        times: "11:00,19:00",
        colour: 0x5865F2,
        about: "The most interesting tech or AI story of the last two days from Ars Technica, The Verge, TechCrunch, MIT Technology Review and Hacker News, explained.",
    },
    DeskInfo {
        desk: Desk::Music,
        key: "music",
        title: "Music",
        channel_label: "Music: channel",
        times_label: "Music: times",
        banner: "🎵 Music, but make it mind-blowing",
        on_key: "VIZIER_DAILY_MUSIC",
        channel_key: "VIZIER_DAILY_MUSIC_CHANNEL",
        times_key: "VIZIER_DAILY_MUSIC_TIMES",
        channel: "1523780873496035328",
        times: "11:00,19:00",
        colour: 0x1DB954,
        about: "Mostly a surprising story about an artist, band, album or song (Western and South Asian) from its Wikipedia article; every third post is music news from Pitchfork, Rolling Stone, NME and Rolling Stone India.",
    },
    DeskInfo {
        desk: Desk::Poetry,
        key: "poetry",
        title: "Poetry",
        channel_label: "Poetry: channel",
        times_label: "Poetry: times",
        banner: "🪶 Sher of the moment",
        on_key: "VIZIER_DAILY_POETRY",
        channel_key: "VIZIER_DAILY_POETRY_CHANNEL",
        times_key: "VIZIER_DAILY_POETRY_TIMES",
        channel: "1520362506596520028",
        times: "11:00,19:00",
        colour: 0x8E6CC7,
        about: "A couplet or a few lines - mostly Urdu in Roman script, sometimes English - from a hand-picked list with the poet named, then a translation and what it means.",
    },
    DeskInfo {
        desk: Desk::Books,
        key: "books",
        title: "Books",
        channel_label: "Books: channel",
        times_label: "Books: times",
        banner: "📚 From the shelf",
        on_key: "VIZIER_DAILY_BOOKS",
        channel_key: "VIZIER_DAILY_BOOKS_CHANNEL",
        times_key: "VIZIER_DAILY_BOOKS_TIMES",
        channel: "1522910442861887498",
        times: "11:00,19:00",
        colour: 0xC2703D,
        about: "An interesting (sometimes controversial) story about a book or writer - world, Indian and Urdu literature - from its Wikipedia article.",
    },
    DeskInfo {
        desk: Desk::Gaming,
        key: "gaming",
        title: "Gaming",
        channel_label: "Gaming: channel",
        times_label: "Gaming: times",
        banner: "🎮 Game on",
        on_key: "VIZIER_DAILY_GAMING",
        channel_key: "VIZIER_DAILY_GAMING_CHANNEL",
        times_key: "VIZIER_DAILY_GAMING_TIMES",
        channel: "1519318756420096070",
        times: "11:00,19:00",
        colour: 0x9146FF,
        about: "Alternates: news on new and upcoming games from Eurogamer, PC Gamer, Polygon, Rock Paper Shotgun and GameSpot, and the story behind a classic game from its Wikipedia article.",
    },
    DeskInfo {
        desk: Desk::Sports,
        key: "sports",
        title: "Sports",
        channel_label: "Sports: channel",
        times_label: "Sports: times",
        banner: "🏏 Sports roundup",
        on_key: "VIZIER_DAILY_SPORTS",
        channel_key: "VIZIER_DAILY_SPORTS_CHANNEL",
        times_key: "VIZIER_DAILY_SPORTS_TIMES",
        channel: "1526595367716655104",
        times: "11:00,19:00",
        colour: 0x2E8B57,
        about: "A roundup of five or six stories from the last day - cricket and football first, then tennis, F1 and the rest, no American leagues - from BBC Sport, ESPNcricinfo, The Hindu and Indian Express.",
    },
    DeskInfo {
        desk: Desk::Fitness,
        key: "fitness",
        title: "Fitness",
        channel_label: "Fitness: channel",
        times_label: "Fitness: times",
        banner: "💪 Get moving",
        on_key: "VIZIER_DAILY_FITNESS",
        channel_key: "VIZIER_DAILY_FITNESS_CHANNEL",
        times_key: "VIZIER_DAILY_FITNESS_TIMES",
        channel: "1525136885112897698",
        times: "11:00,19:00",
        colour: 0xF39C12,
        about: "A motivational quote from a hand-picked list with who said it, and a few lines to get people moving.",
    },
];

pub fn info(desk: Desk) -> &'static DeskInfo {
    DESKS.iter().find(|d| d.desk == desk).expect("every desk is listed")
}

fn by_key(key: &str) -> Option<&'static DeskInfo> {
    DESKS.iter().find(|d| d.key == key)
}

type Feeds = &'static [(&'static str, &'static str)];

const MOVIE_FEEDS: Feeds = &[
    ("Variety", "https://variety.com/feed/"),
    ("Deadline", "https://deadline.com/feed/"),
    ("The Hollywood Reporter", "https://www.hollywoodreporter.com/feed/"),
    ("Indian Express", "https://indianexpress.com/section/entertainment/bollywood/feed/"),
];
const TECH_FEEDS: Feeds = &[
    ("Ars Technica", "https://feeds.arstechnica.com/arstechnica/index"),
    ("The Verge", "https://www.theverge.com/rss/index.xml"),
    ("TechCrunch", "https://techcrunch.com/feed/"),
    ("MIT Technology Review", "https://www.technologyreview.com/feed/"),
    ("Hacker News", "https://hnrss.org/best"),
];
const MUSIC_FEEDS: Feeds = &[
    ("Pitchfork", "https://pitchfork.com/rss/news/"),
    ("Rolling Stone", "https://www.rollingstone.com/music/music-news/feed/"),
    ("NME", "https://www.nme.com/news/music/feed"),
    ("Rolling Stone India", "https://rollingstoneindia.com/feed/"),
];
const GAMING_FEEDS: Feeds = &[
    ("Eurogamer", "https://www.eurogamer.net/feed"),
    ("PC Gamer", "https://www.pcgamer.com/rss/"),
    ("Polygon", "https://www.polygon.com/rss/index.xml"),
    ("Rock Paper Shotgun", "https://www.rockpapershotgun.com/feed"),
    ("GameSpot", "https://www.gamespot.com/feeds/news/"),
];
const SPORTS_FEEDS: Feeds = &[
    ("BBC Sport", "https://feeds.bbci.co.uk/sport/cricket/rss.xml"),
    ("BBC Sport", "https://feeds.bbci.co.uk/sport/football/rss.xml"),
    ("BBC Sport", "https://feeds.bbci.co.uk/sport/tennis/rss.xml"),
    ("BBC Sport", "https://feeds.bbci.co.uk/sport/formula1/rss.xml"),
    ("ESPNcricinfo", "https://www.espncricinfo.com/rss/content/story/feeds/0.xml"),
    // The Hindu and Indian Express were here, and both answer a datacenter
    // with HTTP 403 - a Cloudflare challenge and a hard block - while a home
    // connection reads them fine. From the server that left sports as BBC and
    // nothing Indian but cricket. These two let a server read them (checked
    // from the bot's own host, 2026-09-21).
    ("Hindustan Times", "https://www.hindustantimes.com/feeds/rss/sports/rssfeed.xml"),
    ("Times of India", "https://timesofindia.indiatimes.com/rssfeeds/4719148.cms"),
];

// --- settings ---------------------------------------------------------------------------

pub const DEFAULT_MODEL: &str = "google/gemini-2.5-flash";
const DEFAULT_CATCH_UP_HOURS: u64 = 3;
const MAX_ATTEMPTS: i64 = 3;
const RETRY_AFTER: i64 = 20 * 60;
const IST_OFFSET: i64 = 19_800;
/// Discord allows 4096 in a description; the rest is margin.
const BODY_LIMIT: usize = 4000;

fn enabled(desk: &DeskInfo) -> bool {
    control::on("VIZIER_DAILY", true) && control::on(desk.on_key, true)
}

fn channel(desk: &DeskInfo) -> Option<u64> {
    control::var(desk.channel_key).unwrap_or_else(|| desk.channel.to_string()).trim().parse().ok().filter(|id| *id > 0)
}

/// The desk's times as minutes after midnight India time, in order.
fn slots(desk: &DeskInfo) -> Vec<i64> {
    parse_times(&control::var(desk.times_key).unwrap_or_else(|| desk.times.to_string()))
}

fn parse_times(text: &str) -> Vec<i64> {
    let mut times: Vec<i64> = text
        .split(',')
        .filter_map(|t| {
            let (h, m) = t.trim().split_once(':')?;
            let (h, m) = (h.trim().parse::<i64>().ok()?, m.trim().parse::<i64>().ok()?);
            ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
        })
        .collect();
    times.sort_unstable();
    times.dedup();
    times
}

/// A model name on the bot's own provider, or the bot's usual model with "agent".
fn model_name() -> Option<String> {
    match control::var("VIZIER_DAILY_MODEL") {
        None => Some(DEFAULT_MODEL.to_string()),
        Some(v) if matches!(v.to_ascii_lowercase().as_str(), "agent" | "default" | "none") => None,
        Some(v) => Some(v),
    }
}

fn catch_up_minutes() -> i64 {
    control::number("VIZIER_DAILY_CATCH_UP_HOURS", DEFAULT_CATCH_UP_HOURS).min(23) as i64 * 60
}

// --- storage ----------------------------------------------------------------------------

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS seen (desk TEXT NOT NULL, key TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY (desk, key));
    CREATE TABLE IF NOT EXISTS slots (
        desk TEXT NOT NULL, slot TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, last INTEGER NOT NULL DEFAULT 0,
        done INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (desk, slot));";

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("daily.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    conn.execute_batch(SCHEMA)?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

fn seen(desk: Desk) -> HashSet<String> {
    let Some(db) = DB.get() else {
        return HashSet::new();
    };
    let conn = db.lock();
    let Ok(mut stmt) = conn.prepare("SELECT key FROM seen WHERE desk = ?1") else {
        return HashSet::new();
    };
    stmt.query_map(params![info(desk).key], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()).unwrap_or_default()
}

fn mark_seen(desk: Desk, keys: &[String]) {
    if let Some(db) = DB.get() {
        let conn = db.lock();
        let now = Utc::now().timestamp();
        for key in keys {
            let _ = conn.execute("INSERT OR REPLACE INTO seen (desk, key, ts) VALUES (?1, ?2, ?3)", params![info(desk).key, key, now]);
        }
    }
}

/// Forgets which bank entries with this prefix were used, once every one has been.
fn forget_seen(desk: Desk, prefix: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute("DELETE FROM seen WHERE desk = ?1 AND key LIKE ?2", params![info(desk).key, format!("{}%", prefix)]);
    }
}

/// Claims a slot for one more attempt: true when it is neither done, out of
/// attempts, nor tried too recently.
fn claim(desk: Desk, slot: &str, now: i64) -> bool {
    let Some(db) = DB.get() else {
        return false;
    };
    let conn = db.lock();
    let row: Option<(i64, i64, i64)> = conn
        .query_row("SELECT attempts, last, done FROM slots WHERE desk = ?1 AND slot = ?2", params![info(desk).key, slot], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .ok()
        .flatten();
    if let Some((attempts, last, done)) = row
        && (done != 0 || attempts >= MAX_ATTEMPTS || now - last < RETRY_AFTER)
    {
        return false;
    }
    conn.execute(
        "INSERT INTO slots (desk, slot, attempts, last) VALUES (?1, ?2, 1, ?3)
         ON CONFLICT (desk, slot) DO UPDATE SET attempts = attempts + 1, last = ?3",
        params![info(desk).key, slot, now],
    )
    .is_ok()
}

/// Whether this desk has never posted nor had a timed slot - true only on the first
/// boot after the feature ships (or when a new desk is added), which is when
/// each channel gets one post straight away instead of waiting for its time.
fn never_posted(desk: Desk) -> bool {
    let Some(db) = DB.get() else {
        return false;
    };
    db.lock()
        .query_row("SELECT COUNT(*) FROM slots WHERE desk = ?1 AND (slot != 'launch' OR done = 1)", params![info(desk).key], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n == 0)
        .unwrap_or(false)
}

fn finish(desk: Desk, slot: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO slots (desk, slot, attempts, last, done) VALUES (?1, ?2, 1, ?3, 1)
             ON CONFLICT (desk, slot) DO UPDATE SET done = 1",
            params![info(desk).key, slot, Utc::now().timestamp()],
        );
    }
}

// --- scheduling -------------------------------------------------------------------------

/// The slot that should be posting now, if any: the latest of today's times
/// that has passed, and only while it is within the catch-up window. An
/// earlier slot that was missed is skipped rather than posted late next to it.
fn due_slot(times: &[i64], minute_of_day: i64, catch_up: i64) -> Option<i64> {
    let latest = times.iter().copied().filter(|t| *t <= minute_of_day).max()?;
    (minute_of_day - latest <= catch_up).then_some(latest)
}

fn ist_day(now: i64) -> (String, i64, i64) {
    let local = now + IST_OFFSET;
    let date = Utc.timestamp_opt(local, 0).single().map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
    (date, local.div_euclid(86_400), local.rem_euclid(86_400) / 60)
}

static RUNNING: LazyLock<Mutex<HashSet<Desk>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

pub fn spawn(ctx: Context, deps: VizierDependencies, agent_id: String) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let now = Utc::now().timestamp();
            let (date, _, minute) = ist_day(now);
            for desk in DESKS {
                if !enabled(desk) {
                    continue;
                }
                let Some(channel) = channel(desk) else {
                    continue;
                };
                let slot = match due_slot(&slots(desk), minute, catch_up_minutes()) {
                    Some(time) => format!("{} {:02}:{:02}", date, time / 60, time % 60),
                    None if never_posted(desk.desk) => "launch".to_string(),
                    None => continue,
                };
                if RUNNING.lock().contains(&desk.desk) || !claim(desk.desk, &slot, now) {
                    continue;
                }
                RUNNING.lock().insert(desk.desk);
                let (http, deps, agent_id, which) = (ctx.http.clone(), deps.clone(), agent_id.clone(), desk.desk);
                tokio::spawn(async move {
                    match publish(&http, &deps, &agent_id, which, channel).await {
                        Ok(title) => {
                            finish(which, &slot);
                            tracing::info!("daily: {} posted \"{}\" for {}", info(which).key, title, slot);
                        }
                        Err(err) => tracing::warn!("daily: {} failed for {}: {}", info(which).key, slot, err),
                    }
                    RUNNING.lock().remove(&which);
                });
            }
        }
    });
}

/// Writes one post and sends it to the channel. The keys it used are recorded
/// only once it is posted.
async fn publish(http: &Http, deps: &VizierDependencies, agent_id: &str, desk: Desk, channel: u64) -> anyhow::Result<String> {
    let post = compose(deps, agent_id, desk).await?;
    ChannelId::new(channel).send_message(http, CreateMessage::new().embed(embed(desk, &post))).await?;
    mark_seen(desk, &post.keys);
    Ok(post.title)
}

// --- composing --------------------------------------------------------------------------

/// A finished post.
pub struct Post {
    pub title: String,
    pub url: Option<String>,
    pub body: String,
    pub image: Option<String>,
    pub links: Vec<(String, String)>,
    pub footer: String,
    pub keys: Vec<String>,
}

/// What the writer gets: the source text and what to do with it.
struct Brief {
    task: String,
    length: &'static str,
    material: String,
    url: Option<String>,
    image: Option<String>,
    links: Vec<(String, String)>,
    footer: String,
    keys: Vec<String>,
    /// Checked against the source; the poem and quote posts only interpret a
    /// fixed text, so there is nothing to check.
    verify: bool,
    /// Placed above the written part exactly as given.
    top: Option<String>,
}

const LONG: &str = "Length: 450 to 650 words.";

async fn compose(deps: &VizierDependencies, agent_id: &str, desk: Desk) -> anyhow::Result<Post> {
    let seen = seen(desk);
    let mut rng = Rng::fresh();
    // Two posts a day: counted in half-days, so the two posts of one day take
    // different turns where a desk alternates.
    let (_, day, minute) = ist_day(Utc::now().timestamp());
    let turn = day * 2 + i64::from(minute >= 14 * 60);
    let brief = match desk {
        Desk::History => history(deps, agent_id, turn, &seen, &mut rng).await?,
        Desk::Movies if turn % 2 == 0 => match film(&seen, &mut rng).await {
            Some(brief) => brief,
            None => news(deps, agent_id, desk, MOVIE_FEEDS, 72, MOVIE_PICK, MOVIE_TASK, &seen).await?,
        },
        Desk::Movies => news(deps, agent_id, desk, MOVIE_FEEDS, 72, MOVIE_PICK, MOVIE_TASK, &seen).await?,
        Desk::Tech => news(deps, agent_id, desk, TECH_FEEDS, 48, TECH_PICK, TECH_TASK, &seen).await?,
        Desk::Music if turn % 3 == 2 => news(deps, agent_id, desk, MUSIC_FEEDS, 72, MUSIC_PICK, MUSIC_NEWS_TASK, &seen).await?,
        Desk::Music => listed(bank::MUSIC, MUSIC_FOCUS, MUSIC_TASK, &seen, &mut rng).await?,
        Desk::Books => listed(bank::BOOKS, BOOK_FOCUS, BOOK_TASK, &seen, &mut rng).await?,
        Desk::Gaming if turn % 2 == 0 => news(deps, agent_id, desk, GAMING_FEEDS, 72, GAMING_PICK, GAMING_NEWS_TASK, &seen).await?,
        Desk::Gaming => listed(bank::GAMES, GAME_FOCUS, GAME_TASK, &seen, &mut rng).await?,
        Desk::Sports => sports(deps, agent_id, &seen).await?,
        Desk::Poetry => poem(desk, &seen, &mut rng),
        Desk::Fitness => fitness(desk, &seen, &mut rng),
    };
    write(deps, agent_id, desk, brief).await
}

// History: two posts from the hand-picked Indian list for every one tied to today's date.
async fn history(deps: &VizierDependencies, agent_id: &str, turn: i64, seen: &HashSet<String>, rng: &mut Rng) -> anyhow::Result<Brief> {
    if turn % 3 == 2
        && let Some(brief) = on_this_day(deps, agent_id, seen).await
    {
        return Ok(brief);
    }
    listed(bank::INDIA_HISTORY, &[], HISTORY_TASK, seen, rng).await
}

const HISTORY_TASK: &str = "Tell ONE gripping true story from this article. Choose the single most surprising, dramatic or \
    little-known episode in it and tell it as a narrative with a strong opening hook - not a summary of the whole article. \
    Finish with why it still matters, or with its most striking detail.";

async fn on_this_day(deps: &VizierDependencies, agent_id: &str, seen: &HashSet<String>) -> Option<Brief> {
    let today = Utc::now().with_timezone(&super::stats::ist());
    let events: Vec<src::DayEvent> = src::on_this_day(today.month(), today.day()).await.into_iter().filter(|e| !seen.contains(&wiki_key(&e.page))).collect();
    if events.is_empty() {
        return None;
    }
    let lines: Vec<String> = events.iter().map(|e| format!("{}: {} (article: {})", e.year, e.text, e.page)).collect();
    let picks = pick(
        deps,
        agent_id,
        &lines,
        3,
        "the events whose article would make the most gripping single story for young readers. Prefer the Indian \
         subcontinent when a good one exists. Skip events that are only a death toll, an election result or a sports score.",
    )
    .await;
    for i in picks {
        let event = &events[i];
        let Some(page) = src::wiki_page(&event.page).await else {
            continue;
        };
        let material = format!(
            "ON THIS DAY ({} {}), in {}: {}\n\nARTICLE \"{}\":\n{}",
            today.day(),
            today.format("%B"),
            event.year,
            event.text,
            page.title,
            src::focus(&page.text, &[], src::ARTICLE_LIMIT)
        );
        return Some(Brief {
            task: format!("{} Open with the date: this happened on this day in {}.", HISTORY_TASK, event.year),
            length: LONG,
            material,
            url: Some(page.url.clone()),
            image: page.image.clone(),
            links: vec![(format!("Wikipedia: {}", page.title), page.url.clone())],
            footer: format!("On this day · Source: Wikipedia, \"{}\"", page.title),
            keys: vec![wiki_key(&event.page), wiki_key(&page.title)],
            verify: true,
            top: None,
        });
    }
    None
}

fn wiki_key(title: &str) -> String {
    format!("wiki:{}", title.to_lowercase())
}

/// A story from an article on one of a hand-picked list of titles, one not
/// used before (the list starts over once every title has been).
async fn listed(titles: &[&str], wanted: &[&str], task: &str, seen: &HashSet<String>, rng: &mut Rng) -> anyhow::Result<Brief> {
    let mut fresh: Vec<&str> = titles.iter().copied().filter(|t| !seen.contains(&wiki_key(t))).collect();
    if fresh.is_empty() {
        fresh = titles.to_vec();
    }
    rng.shuffle(&mut fresh);
    for title in fresh.into_iter().take(4) {
        let Some(page) = src::wiki_find(title).await else {
            tracing::warn!("daily: no Wikipedia article for \"{}\"", title);
            continue;
        };
        return Ok(Brief {
            task: format!("{}\nThe article is about: {}.", task, page.title),
            length: LONG,
            material: format!("ARTICLE \"{}\":\n{}", page.title, src::focus(&page.text, wanted, src::ARTICLE_LIMIT)),
            url: Some(page.url.clone()),
            image: page.image.clone(),
            links: vec![(format!("Wikipedia: {}", page.title), page.url.clone())],
            footer: format!("Source: Wikipedia, \"{}\"", page.title),
            keys: vec![wiki_key(title), wiki_key(&page.title)],
            verify: true,
            top: None,
        });
    }
    anyhow::bail!("no article could be loaded")
}

const MUSIC_FOCUS: &[&str] =
    &["background", "history", "career", "recording", "writing", "composition", "production", "legacy", "influence", "controvers", "personal life", "style", "release"];
const MUSIC_TASK: &str = "Find the most mind-blowing, surprising or little-known story in this article - how something was made, \
    an unlikely origin, a record, a feud, a coincidence - and tell it as a narrative for music fans. Don't list the \
    discography.";
const BOOK_FOCUS: &[&str] =
    &["background", "writing", "publication", "composition", "controvers", "ban", "censor", "reception", "legacy", "adaptation", "personal life", "legal", "influence", "criticism", "obscenity", "trial"];
const BOOK_TASK: &str = "Tell one genuinely interesting story or fact about this book or writer - it may be a controversy: how \
    it was written, a ban, trial or scandal, a surprising influence, a strange-but-true detail. Tell it as a story; keep \
    plot summary to a line or two.";
const GAME_FOCUS: &[&str] = &["development", "production", "release", "reception", "legacy", "controvers", "sales", "design", "history", "impact"];
const GAME_TASK: &str = "Tell the most interesting story behind this game - how it was made, a near-cancellation, a surprising \
    design decision, a controversy, a record or its legacy - as a narrative for gamers. Don't just review it.";

// Movies: a film or show from the Guess the Movie bank, from its Wikipedia article.
const FILM_FOCUS: &[&str] =
    &["production", "development", "pre-production", "casting", "filming", "photography", "music", "soundtrack", "release", "controvers", "legal", "reception", "legacy", "box office", "effects", "writing"];

async fn film(seen: &HashSet<String>, rng: &mut Rng) -> Option<Brief> {
    let films = super::movie_bank::bank()?;
    for _ in 0..6 {
        let movie = films.movie(rng.below(films.count().max(1)))?;
        let key = format!("film:{}", movie.id);
        if seen.contains(&key) {
            continue;
        }
        let kind = if movie.kind == super::movie_bank::Kind::Series { "TV series" } else { "film" };
        let Some(title) = src::wiki_search(&format!("{} {} {}", movie.title, movie.year, kind)).await else {
            continue;
        };
        let Some(page) = src::wiki_page(&title).await else {
            continue;
        };
        // The search found something else when the article never mentions the year.
        if !page.text.contains(&movie.year.to_string()) {
            continue;
        }
        return Some(Brief {
            task: format!(
                "Write a behind-the-scenes story about the making of {} ({}): the most surprising production story, casting \
                 twist, on-set incident, controversy or legacy fact in the article. Tell it as a story; give the plot one line \
                 of context at most.",
                movie.title, movie.year
            ),
            length: LONG,
            material: format!("ARTICLE \"{}\":\n{}", page.title, src::focus(&page.text, FILM_FOCUS, src::ARTICLE_LIMIT)),
            url: Some(page.url.clone()),
            image: page.image.clone(),
            links: vec![(format!("Wikipedia: {}", page.title), page.url.clone())],
            footer: format!("Behind the scenes · Source: Wikipedia, \"{}\"", page.title),
            keys: vec![key, wiki_key(&page.title)],
            verify: true,
            top: None,
        });
    }
    None
}

const MOVIE_PICK: &str = "the most interesting story about films or TV for young Indian viewers: an upcoming Hindi or English \
    film or series (trailer, release date, first look, casting), a behind-the-scenes story, or a real controversy. Skip \
    pure box-office tallies, obituaries, awards-season lists and business deals.";
const MOVIE_TASK: &str = "Write an engaging post about this film/TV story: what's happening, the background a fan needs, and why \
    people are talking about it.";
const TECH_PICK: &str = "the most interesting story for curious students: AI, surprising science-tech, gadgets, space tech, a big \
    security incident, a major launch or internet culture. Skip funding rounds, earnings, executive moves, deals and \
    coupon posts, and anything only about US politics.";
const TECH_TASK: &str = "Write an engaging explainer of this tech story: what happened, how it works in plain words, why it's cool \
    or important, and what it could mean for ordinary people.";
const MUSIC_PICK: &str = "genuinely surprising or mind-blowing music news, Western or South Asian: records, reunions, legal fights, \
    discoveries, big announcements. Skip tour-date lists, minor single releases and obituaries.";
const MUSIC_NEWS_TASK: &str = "Write an engaging post about this music story: what happened, the background a fan needs, and why \
    it's remarkable.";
const GAMING_PICK: &str = "news on a new or upcoming game (announcement, release date, trailer, big reveal) or a remarkable gaming \
    story. Skip deals and sales roundups, guides, walkthroughs, reviews of hardware accessories and 'best X' lists.";
const GAMING_NEWS_TASK: &str = "Write an engaging post about this gaming story: what's coming or what happened, what we know so \
    far, and why gamers should care.";

/// One story from recent feed items, the model choosing which.
#[allow(clippy::too_many_arguments)]
async fn news(
    deps: &VizierDependencies,
    agent_id: &str,
    desk: Desk,
    feeds: Feeds,
    max_age_hours: i64,
    criteria: &str,
    task: &str,
    seen: &HashSet<String>,
) -> anyhow::Result<Brief> {
    let items = recent(feeds, max_age_hours, 12, seen).await;
    if items.is_empty() {
        anyhow::bail!("no fresh {} news in the feeds", info(desk).key);
    }
    let lines: Vec<String> = items.iter().map(|i| format!("[{}] {} — {}", i.source, i.title, src::clip_chars(&i.summary, 200))).collect();
    for i in pick(deps, agent_id, &lines, 3, criteria).await {
        let item = &items[i];
        let (text, image) = match src::article(&item.link).await {
            Some((text, image)) => (text, image.or_else(|| item.image.clone())),
            None if item.summary.chars().count() >= 400 => (item.summary.clone(), item.image.clone()),
            None => continue,
        };
        return Ok(Brief {
            task: task.to_string(),
            length: LONG,
            material: format!("HEADLINE: {}\nPUBLISHED BY: {}\n\nARTICLE:\n{}", item.title, item.source, text),
            url: Some(item.link.clone()),
            image,
            links: vec![(format!("Full story on {}", item.source), item.link.clone())],
            footer: format!("Source: {}", item.source),
            keys: vec![news_key(&item.link)],
            verify: true,
            top: None,
        });
    }
    anyhow::bail!("none of the picked {} stories could be read", info(desk).key)
}

fn news_key(link: &str) -> String {
    format!("link:{}", link.split(['?', '#']).next().unwrap_or(link))
}

/// Items from the last `max_age_hours`, not posted before, at most
/// `per_feed` from each feed, newest first.
async fn recent(feeds: Feeds, max_age_hours: i64, per_feed: usize, seen: &HashSet<String>) -> Vec<src::FeedItem> {
    let now = Utc::now().timestamp();
    let loaded = futures::future::join_all(feeds.iter().map(|(name, url)| src::feed(name, url))).await;
    let mut items: Vec<src::FeedItem> = loaded
        .into_iter()
        .flat_map(|items| {
            items
                .into_iter()
                .filter(|i| i.published == 0 || now - i.published <= max_age_hours * 3600)
                .filter(|i| !seen.contains(&news_key(&i.link)))
                .take(per_feed)
        })
        .collect();
    let mut titles = HashSet::new();
    items.retain(|i| titles.insert(i.title.to_lowercase()));
    items.sort_by_key(|i| -i.published);
    items.truncate(45);
    items
}

// Sports: a roundup of several stories, never American leagues.
static AMERICAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(NFL|NBA|MLB|NHL|NCAA|WNBA|MLS|Super Bowl|World Series|American football|baseball|Stanley Cup)\b").unwrap());

async fn sports(deps: &VizierDependencies, agent_id: &str, seen: &HashSet<String>) -> anyhow::Result<Brief> {
    let items: Vec<src::FeedItem> =
        recent(SPORTS_FEEDS, 30, 12, seen).await.into_iter().filter(|i| !AMERICAN.is_match(&i.title) && !AMERICAN.is_match(&i.summary)).collect();
    if items.len() < 3 {
        anyhow::bail!("too little fresh sports news in the feeds");
    }
    let lines: Vec<String> = items.iter().map(|i| format!("[{}] {} — {}", i.source, i.title, src::clip_chars(&i.summary, 160))).collect();
    let picks = pick(
        deps,
        agent_id,
        &lines,
        6,
        "a varied roundup of the day's biggest sports news for Indian fans: cricket (India and Pakistan especially) and football \
         first, then tennis, F1, hockey, badminton, athletics, chess and the rest. No two items about the same story. Results, \
         big moments and big announcements over previews and opinion columns.",
    )
    .await;
    let mut material = String::new();
    let mut links = Vec::new();
    let mut keys = Vec::new();
    let mut image = None;
    for (n, i) in picks.into_iter().enumerate() {
        let item = &items[i];
        let text = match src::article(&item.link).await {
            Some((text, img)) => {
                image = image.or(img);
                src::clip_chars(&text, 2500)
            }
            None => item.summary.clone(),
        };
        material.push_str(&format!("ITEM {} ({}): {}\n{}\n\n", n + 1, item.source, item.title, text));
        links.push((src::clip_chars(&item.title, 90), item.link.clone()));
        keys.push(news_key(&item.link));
    }
    Ok(Brief {
        task: format!(
            "Write a sports news roundup of these {} items: a one-line intro, then for each item a bold headline-style line \
             starting with an emoji for its sport, followed by two to four sentences on what happened and why it matters.",
            links.len()
        ),
        length: "Length: 350 to 550 words in total.",
        material,
        url: None,
        image,
        links,
        footer: "Sources: BBC Sport, ESPNcricinfo, The Hindu, Indian Express".to_string(),
        keys,
        verify: true,
        top: None,
    })
}

// Poetry and fitness: the text is fixed; the model only writes around it.
fn poem(desk: Desk, seen: &HashSet<String>, rng: &mut Rng) -> Brief {
    let mut fresh: Vec<&bank::Poem> = bank::POEMS.iter().filter(|p| !seen.contains(&format!("poem:{}", p.id))).collect();
    if fresh.is_empty() {
        forget_seen(desk, "poem:");
        fresh = bank::POEMS.iter().collect();
    }
    // Mostly Urdu, as asked: an English poem about one time in five.
    let urdu: Vec<&bank::Poem> = fresh.iter().copied().filter(|p| p.language == "urdu").collect();
    let english: Vec<&bank::Poem> = fresh.iter().copied().filter(|p| p.language != "urdu").collect();
    let pool = if english.is_empty() || (!urdu.is_empty() && rng.below(5) != 0) { urdu } else { english };
    let poem = pool[rng.below(pool.len())];
    let verse = poem.lines.join("\n");
    let task = if poem.language == "urdu" {
        "These lines are Urdu written in Roman script. Write:\n1. **Translation** - a faithful English translation, line by \
         line, in italics.\n2. Two short paragraphs, in simple English, on what the lines mean and why they hit so hard - \
         the feeling, the images, any wordplay.\nMention the poet only by name. Say NOTHING about the poet's life, dates, or \
         when or why the lines were written."
    } else {
        "Write two or three short paragraphs, in simple English, on what these lines mean and why they hit so hard - the \
         feeling, the images, the craft. Mention the poet only by name. Say NOTHING about the poet's life, dates, or when or \
         why the lines were written."
    };
    Brief {
        task: task.to_string(),
        length: "Length: 120 to 220 words, not counting the poem.",
        material: format!("POEM by {}:\n{}", poem.poet, verse),
        url: None,
        image: None,
        links: Vec::new(),
        footer: format!("Poetry · {}", poem.poet),
        keys: vec![format!("poem:{}", poem.id)],
        verify: false,
        top: Some(format!("{}\n\n— **{}**", poem.lines.iter().map(|l| format!("*{}*", l)).collect::<Vec<_>>().join("\n"), poem.poet)),
    }
}

fn fitness(desk: Desk, seen: &HashSet<String>, rng: &mut Rng) -> Brief {
    let mut fresh: Vec<&bank::Quote> = bank::QUOTES.iter().filter(|q| !seen.contains(&format!("quote:{}", q.id))).collect();
    if fresh.is_empty() {
        forget_seen(desk, "quote:");
        fresh = bank::QUOTES.iter().collect();
    }
    let quote = fresh[rng.below(fresh.len())];
    Brief {
        task: "Write a short, punchy motivational note built on this quote for people who want to get fitter: what it means \
               in a workout or in daily life, and end with one small thing to do today. Warm, energetic, no preaching. No \
               medical claims, no numbers or statistics, and nothing about the speaker's life."
            .to_string(),
        length: "Length: 70 to 130 words, not counting the quote.",
        material: format!("QUOTE: \"{}\"\nSAID BY: {}", quote.text, quote.who),
        url: None,
        image: None,
        links: Vec::new(),
        footer: "Fitness · quote of the day".to_string(),
        keys: vec![format!("quote:{}", quote.id)],
        verify: false,
        top: Some(format!("> *\"{}\"*\n> — **{}**", quote.text, quote.who)),
    }
}

// --- the model calls --------------------------------------------------------------------

async fn ask(deps: &VizierDependencies, agent_id: &str, prompt: String) -> anyhow::Result<String> {
    super::weekly::ask_model_with(deps, agent_id, prompt, model_name()).await
}

/// Up to `want` indexes into `lines`, best first, chosen by the model; the
/// first ones in order when the model's answer can't be read.
async fn pick(deps: &VizierDependencies, agent_id: &str, lines: &[String], want: usize, criteria: &str) -> Vec<usize> {
    let want = want.min(lines.len());
    let listing: String = lines.iter().enumerate().map(|(i, l)| format!("{}. {}\n", i + 1, l)).collect();
    let prompt = format!(
        "Here are numbered candidates:\n\n{}\nChoose {} of them: {}\nReply with JSON only, best first, like {{\"pick\": [4, 1, 9]}}.",
        listing, want, criteria
    );
    let chosen = match ask(deps, agent_id, prompt).await {
        Ok(reply) => parse_pick(&reply, lines.len()),
        Err(err) => {
            tracing::warn!("daily: pick call failed: {}", err);
            Vec::new()
        }
    };
    let mut chosen: Vec<usize> = chosen.into_iter().take(want).collect();
    if chosen.is_empty() {
        chosen = (0..want).collect();
    }
    chosen
}

fn parse_pick(reply: &str, count: usize) -> Vec<usize> {
    let json = first_json(reply).unwrap_or_default();
    let mut out = Vec::new();
    for n in json["pick"].as_array().into_iter().flatten().filter_map(|v| v.as_u64()) {
        let i = n as usize;
        if (1..=count).contains(&i) && !out.contains(&(i - 1)) {
            out.push(i - 1);
        }
    }
    out
}

/// The checker's instructions, apart from the call so they can be tested.
fn check_prompt(material: &str, title: &str, body: &str) -> String {
    format!(
        "You check a short social-media POST against its SOURCE for FACTUAL ERRORS.\n\n\
         A problem is ONLY a statement the POST makes that is FALSE according to the SOURCE, or a specific fact the POST asserts \
         - a name, date, number, quote, event or cause - that the SOURCE never mentions.\n\n\
         These are NOT problems. Do not list them:\n\
         - leaving facts out: the POST is a short retelling, not a copy\n\
         - summarising, simplifying or paraphrasing, as long as the meaning holds\n\
         - rounding, or giving fewer figures than the SOURCE (one estimate where it gives a range)\n\
         - vaguer time words (\"last week\", \"recently\") where the SOURCE gives a date\n\
         - not naming someone the SOURCE names\n\
         - opinion, tone, framing or storytelling (\"a twist\", \"remarkable\", \"the story goes\")\n\
         - anything the SOURCE does state, however it is worded\n\n\
         For each real problem, quote the POST's exact words, then name the fact that differs: \"the POST says X, the SOURCE \
         says Y\". If the SOURCE says the same thing in other words, it is NOT a problem - do not list it. If you cannot name a \
         fact that differs, it is not a problem.\n\n\
         Reply with JSON only: {{\"ok\": true}} or {{\"ok\": false, \"problems\": [\"<exact POST words> - the POST says X, the SOURCE says Y\"]}}\n\n\
         SOURCE:\n{}\n\nPOST:\nTITLE: {}\n\n{}",
        material, title, body
    )
}

fn first_json(text: &str) -> Option<serde_json::Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(text.get(start..=end)?).ok()
}

/// The writer's rules.
///
/// The first one used to read "No names, dates, numbers, quotes or claims from
/// memory", meant as "nothing from memory". The model read it as "no names",
/// wrote "a player scored 100*" where the source named Shafali, and the checker
/// then failed the post for leaving her out - the two prompts working against
/// each other. It now says outright that what the SOURCE states is welcome.
const WRITER_RULES: &str = "RULES:
- Use ONLY facts the SOURCE states. Names, dates, numbers and quotes are welcome WHEN THE SOURCE GIVES THEM - use them, don't blur them into \"a player\" or \"recently\". Never add any the SOURCE doesn't give, and nothing from memory.
- Keep the source's hedges (\"reportedly\", \"according to\", \"is expected to\"). Never present a rumour as fact.
- Engaging, vivid storytelling in plain English for young Indian readers; short paragraphs.
- Discord markdown only: **bold** for a few key names or moments, *italics* sparingly. No # headings, no tables, no hashtags.
- Output exactly this shape and nothing else:
TITLE: <a catchy title, at most 90 characters>

<the post>
- No source list, no preamble, no sign-off question to the reader.";

async fn write(deps: &VizierDependencies, agent_id: &str, desk: Desk, brief: Brief) -> anyhow::Result<Post> {
    let base = format!(
        "You write the {} post for a friendly Discord server of students and young people in India.\n\nTASK: {}\n{}\n\n{}\n\nSOURCE:\n{}",
        info(desk).title,
        brief.task,
        brief.length,
        WRITER_RULES,
        brief.material
    );
    let (mut title, mut body) = draft(deps, agent_id, &base).await?;
    if brief.verify {
        let mut problems = check(deps, agent_id, &brief.material, &title, &body).await?;
        if !problems.is_empty() {
            tracing::info!("daily: {} draft had {} unsupported claim(s), rewriting", info(desk).key, problems.len());
            let retry = format!(
                "{}\n\nYOUR PREVIOUS DRAFT:\nTITLE: {}\n\n{}\n\nA fact-checker found these statements wrong against the SOURCE:\n- {}\n\nWrite the \
                 post again in the same shape, correcting or removing ONLY those statements. Keep everything else, including \
                 names and numbers the SOURCE gives.",
                base,
                title,
                body,
                problems.join("\n- ")
            );
            (title, body) = draft(deps, agent_id, &retry).await?;
            problems = check(deps, agent_id, &brief.material, &title, &body).await?;
            if !problems.is_empty() {
                // Don't offer the same source again.
                mark_seen(desk, &brief.keys);
                anyhow::bail!("draft still unsupported after a rewrite: {}", problems.join("; "));
            }
        }
    }
    let body = match &brief.top {
        Some(top) => format!("{}\n\n{}", top, body),
        None => body,
    };
    Ok(Post {
        title,
        url: brief.url,
        body: src::clip_chars(&body, BODY_LIMIT),
        image: brief.image.filter(|u| usable_image(u)),
        links: brief.links,
        footer: brief.footer,
        keys: brief.keys,
    })
}

async fn draft(deps: &VizierDependencies, agent_id: &str, prompt: &str) -> anyhow::Result<(String, String)> {
    for attempt in 1..=2 {
        match ask(deps, agent_id, prompt.to_string()).await {
            Ok(reply) => match split_title(&reply) {
                Some(parts) => return Ok(parts),
                None => tracing::warn!("daily: draft unreadable (attempt {}): {}", attempt, src::clip_chars(&reply, 200)),
            },
            Err(err) => tracing::warn!("daily: draft call failed (attempt {}): {}", attempt, err),
        }
    }
    anyhow::bail!("the model gave no usable draft")
}

/// Claims in the draft the source doesn't support; empty when it checks out.
/// Asks whether a draft states anything its SOURCE does not.
///
/// The first version called itself "a strict fact-checker" and asked for
/// "every factual claim" the source did not support, and one item on that list
/// sank the post. On its first morning it sank six of nine: for leaving out a
/// lower estimate, for "last week" being vaguer than a date, for a summary
/// being "slightly less precise", for "a twist" being opinion - and once for a
/// fact the source stated, quoted back in the same sentence. None of those is a
/// false statement. A post is a short retelling; leaving things out is what a
/// retelling does.
///
/// So a problem is now only something the POST ASSERTS that the SOURCE
/// contradicts or never mentions, the omissions and tone that are fine are
/// named outright, and each problem has to quote the POST's own words - which
/// makes the checker point at a sentence rather than at what is missing.
async fn check(deps: &VizierDependencies, agent_id: &str, material: &str, title: &str, body: &str) -> anyhow::Result<Vec<String>> {
    let prompt = check_prompt(material, title, body);
    for attempt in 1..=2 {
        match ask(deps, agent_id, prompt.clone()).await {
            Ok(reply) => match first_json(&reply) {
                Some(json) if json["ok"].as_bool() == Some(true) => return Ok(Vec::new()),
                Some(json) if json["ok"].as_bool() == Some(false) => {
                    let problems: Vec<String> =
                        json["problems"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect();
                    return Ok(if problems.is_empty() { vec!["unspecified unsupported claim".to_string()] } else { problems });
                }
                _ => tracing::warn!("daily: check unreadable (attempt {}): {}", attempt, src::clip_chars(&reply, 200)),
            },
            Err(err) => tracing::warn!("daily: check call failed (attempt {}): {}", attempt, err),
        }
    }
    anyhow::bail!("the fact-check gave no usable answer")
}

/// "TITLE: ..." then the body; markdown headings turned into bold lines.
/// Splits a draft into its title and body.
///
/// The writer is told to open with `TITLE: ...`, and mostly does. When it does
/// not - the sports desk wrote `SPORTS SHORTS: Your Weekly Recap!` twice on its
/// first morning and the whole post was thrown away as unreadable - the first
/// line IS the title, only unlabelled, so it is taken as one. The marker is
/// matched whatever its case and however it is dressed in markdown.
fn split_title(reply: &str) -> Option<(String, String)> {
    let reply = reply.trim().trim_start_matches("```").trim_end_matches("```").trim();
    // Searched for in the reply itself, not in an upper-cased copy: a few
    // characters change length when upper-cased (`ß` becomes `SS`), and an
    // offset found in the copy can land mid-character in the original.
    let marker = reply.char_indices().map(|(i, _)| i).find(|&i| reply.get(i..i + 6).is_some_and(|s| s.eq_ignore_ascii_case("TITLE:")));
    let rest = match marker {
        // Only a marker near the top is a marker; the word in the middle of a
        // paragraph is just prose.
        Some(at) if !reply[..at].contains("\n\n") => &reply[at + "TITLE:".len()..],
        _ => reply,
    };
    let (title, body) = rest.split_once('\n')?;
    let title = title
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches(|c| c == '*' || c == '"' || c == '_')
        .trim()
        .to_string();
    let body: String = body
        .trim()
        .lines()
        .map(|l| {
            let t = l.trim_start();
            if t.starts_with('#') { format!("**{}**", t.trim_start_matches('#').trim()) } else { l.to_string() }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let body = body.trim().to_string();
    (!title.is_empty() && body.chars().count() >= 40).then(|| (src::clip_chars(&title, 250), body))
}

fn usable_image(url: &str) -> bool {
    let lower = url.to_lowercase();
    lower.starts_with("https://") && !lower.split('?').next().unwrap_or_default().ends_with(".svg")
}

fn embed(desk: Desk, post: &Post) -> CreateEmbed {
    let desk = info(desk);
    let mut e = CreateEmbed::new()
        .author(CreateEmbedAuthor::new(desk.banner))
        .title(post.title.clone())
        .description(post.body.clone())
        .colour(desk.colour)
        .footer(CreateEmbedFooter::new(format!("{} · written by AI from the source", post.footer)));
    if let Some(url) = &post.url {
        e = e.url(url.clone());
    }
    if let Some(image) = &post.image {
        e = e.image(image.clone());
    }
    let mut links = String::new();
    for (label, url) in &post.links {
        let line = format!("• [{}]({})\n", label.replace(['[', ']'], ""), url);
        if links.chars().count() + line.chars().count() > 1024 {
            break;
        }
        links.push_str(&line);
    }
    if !links.is_empty() {
        e = e.field("Read more", links.trim_end().to_string(), false);
    }
    e
}

// --- the command ------------------------------------------------------------------------

pub fn command() -> CreateCommand {
    let mut desk = CreateCommandOption::new(CommandOptionType::String, "desk", "which topic").required(true);
    for d in DESKS {
        desk = desk.add_string_choice(d.title, d.key);
    }
    CreateCommand::new("dailypost")
        .description("admin only: write a daily topic post now - a private preview, or straight into its channel")
        .add_option(desk)
        .add_option(CreateCommandOption::new(
            CommandOptionType::Boolean,
            "post",
            "true posts it in the topic's channel (counts as that slot); false or empty shows only you a preview",
        ))
}

pub async fn post_command(ctx: &Context, deps: &VizierDependencies, agent_id: &str, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let reply = CreateInteractionResponseMessage::new().content("Only admins can do this.").ephemeral(true);
        let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
        return;
    }
    let option = |name: &str| command.data.options.iter().find(|o| o.name == name).map(|o| o.value.clone());
    let desk = match option("desk") {
        Some(CommandDataOptionValue::String(key)) => by_key(&key),
        _ => None,
    };
    let for_real = matches!(option("post"), Some(CommandDataOptionValue::Boolean(true)));
    let Some(desk) = desk else {
        let reply = CreateInteractionResponseMessage::new().content("Pick a topic.").ephemeral(true);
        let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
        return;
    };
    let _ = command.defer_ephemeral(&ctx.http).await;
    let text = if for_real {
        match channel(desk) {
            None => format!("{} has no channel set in the panel.", desk.title),
            Some(channel) => match publish(&ctx.http, deps, agent_id, desk.desk, channel).await {
                Ok(title) => {
                    // It stands in for the slot that is due, so the schedule doesn't post again straight after.
                    let now = Utc::now().timestamp();
                    let (date, _, minute) = ist_day(now);
                    if let Some(time) = due_slot(&slots(desk), minute, catch_up_minutes()) {
                        finish(desk.desk, &format!("{} {:02}:{:02}", date, time / 60, time % 60));
                    }
                    format!("Posted \"{}\" in <#{}>.", title, channel)
                }
                Err(err) => format!("Couldn't write the {} post: {}", desk.title, err),
            },
        }
    } else {
        match compose(deps, agent_id, desk.desk).await {
            Ok(post) => {
                let followup = CreateInteractionResponseFollowup::new().embed(embed(desk.desk, &post)).ephemeral(true);
                let _ = command.create_followup(&ctx.http, followup).await;
                "Preview above - only you can see it, and nothing was marked as used.".to_string()
            }
            Err(err) => format!("Couldn't write the {} post: {}", desk.title, err),
        }
    };
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(text)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checker sank six of nine posts on its first morning for things that
    /// are not errors. Each of these was a real rejection reason that day.
    #[test]
    fn the_checker_is_told_what_is_not_an_error() {
        let p = check_prompt("SRC", "T", "BODY");
        for fine in ["leaving facts out", "rounding", "vaguer time words", "not naming someone", "opinion, tone", "summarising"] {
            assert!(p.contains(fine), "the checker is not told that {fine:?} is fine");
        }
        assert!(p.contains("quote the POST's exact words"), "a problem must point at the post's own words");
        assert!(p.contains("FALSE according to the SOURCE"));
        // The words that made it hunt for anything at all.
        assert!(!p.contains("strict"), "\"strict\" is back");
        assert!(!p.contains("every factual claim"), "\"every factual claim\" is back");
        assert!(p.contains("SOURCE:\nSRC") && p.contains("TITLE: T\n\nBODY"), "the material is not passed through");
    }

    /// A draft's title comes back however the model happened to write it.
    #[test]
    fn a_title_is_found_labelled_or_not() {
        let body = "A body long enough to count as a post, with a second sentence in it.";
        let want = |reply: String| split_title(&reply).map(|(t, _)| t);
        assert_eq!(want(format!("TITLE: Big Win\n\n{body}")).as_deref(), Some("Big Win"));
        assert_eq!(want(format!("**Title:** Big Win\n\n{body}")).as_deref(), Some("Big Win"), "bold and lower case");
        // What the sports desk actually wrote, twice, and was thrown away for.
        assert_eq!(want(format!("SPORTS SHORTS: Your Weekly Recap!\n\n{body}")).as_deref(), Some("SPORTS SHORTS: Your Weekly Recap!"));
        assert_eq!(want(format!("# A Heading Title\n\n{body}")).as_deref(), Some("A Heading Title"));
        // "title:" in the middle of the prose is not a marker.
        let prose = format!("Opening Line\n\n{body}\n\nHis book's title: Dune.");
        assert_eq!(want(prose).as_deref(), Some("Opening Line"));
        assert!(split_title("TITLE: Only a title\n\ntoo short").is_none(), "a body under 40 characters is not a post");
        // Characters that grow when upper-cased must not throw the marker off.
        assert_eq!(want(format!("Straße und Fußball: ein Tag\n\n{body}")).as_deref(), Some("Straße und Fußball: ein Tag"));
        assert_eq!(want(format!("ßßß TITLE: Weiß\n\n{body}")).as_deref(), Some("Weiß"));
    }

    /// Both used to be in the list, and both refuse the server outright.
    #[test]
    fn sports_reads_no_feed_that_blocks_a_server() {
        for (_, url) in SPORTS_FEEDS {
            assert!(!url.contains("thehindu.com") && !url.contains("indianexpress.com"), "{url} answers the server with 403");
        }
        assert!(SPORTS_FEEDS.iter().filter(|(_, u)| u.contains("hindustantimes") || u.contains("indiatimes") || u.contains("cricinfo")).count() >= 3,
            "sports needs Indian sources, not only the BBC");
    }

    /// The books post was failed for a paraphrase: its quote and the source's
    /// words said the same thing. A problem must now name what differs.
    #[test]
    fn the_checker_must_name_what_differs() {
        let p = check_prompt("S", "T", "B");
        assert!(p.contains("the POST says X, the SOURCE says Y"));
        assert!(p.contains("same thing in other words, it is NOT a problem"));
    }

    /// "No names ... from memory" was read as "no names", and the sports post
    /// wrote "a player" where the source named Shafali.
    #[test]
    fn the_writer_is_told_sourced_names_are_welcome() {
        assert!(WRITER_RULES.contains("welcome WHEN THE SOURCE GIVES THEM"));
        assert!(WRITER_RULES.contains("nothing from memory"));
        assert!(!WRITER_RULES.contains("No names, dates"), "the ambiguous rule is back");
    }

    #[test]
    fn times_are_read_sorted_and_bad_ones_dropped() {
        assert_eq!(parse_times("20:00, 09:30,25:00,x,09:30"), vec![9 * 60 + 30, 20 * 60]);
        assert!(parse_times("").is_empty());
    }

    #[test]
    fn only_the_latest_passed_slot_is_due_and_only_while_fresh() {
        let times = [9 * 60, 20 * 60];
        assert_eq!(due_slot(&times, 8 * 60, 180), None);
        assert_eq!(due_slot(&times, 9 * 60 + 5, 180), Some(9 * 60));
        assert_eq!(due_slot(&times, 13 * 60, 180), None);
        // Both passed: the morning one isn't posted late next to the evening one.
        assert_eq!(due_slot(&times, 20 * 60 + 30, 900), Some(20 * 60));
    }

    #[test]
    fn a_draft_splits_into_title_and_body() {
        let (title, body) = split_title("TITLE: **The Iron Pillar**\n\n## Why it doesn't rust\nIt has stood in Delhi for centuries without rusting away.").unwrap();
        assert_eq!(title, "The Iron Pillar");
        assert!(body.starts_with("**Why it doesn't rust**"));
        assert!(split_title("no title here").is_none());
    }

    #[test]
    fn picks_are_one_based_deduped_and_in_range() {
        assert_eq!(parse_pick("sure: {\"pick\": [3, 1, 3, 99]}", 5), vec![2, 0]);
        assert!(parse_pick("nope", 5).is_empty());
    }

    #[test]
    fn every_desk_has_a_valid_default_channel_and_two_times() {
        for d in DESKS {
            assert!(d.channel.parse::<u64>().is_ok(), "{}", d.key);
            assert_eq!(parse_times(d.times).len(), 2, "{}", d.key);
        }
        assert_eq!(DESKS.len(), 9);
    }

    #[test]
    fn the_banks_have_no_repeated_ids() {
        let mut ids = HashSet::new();
        assert!(bank::POEMS.iter().all(|p| ids.insert(p.id)));
        assert!(bank::QUOTES.iter().all(|q| ids.insert(q.id)));
        assert!(bank::POEMS.iter().all(|p| !p.lines.is_empty()));
    }

    #[test]
    fn american_leagues_are_filtered() {
        assert!(AMERICAN.is_match("NFL week 3: Chiefs win"));
        assert!(!AMERICAN.is_match("India beat Pakistan in Asia Cup final"));
    }
}
