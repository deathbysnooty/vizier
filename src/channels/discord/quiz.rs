//! /quiz - a never-ending quiz in one channel.
//!
//! One question at a time and the first right answer takes the point. Typed
//! questions forgive small spelling slips; multiple choice gives each person
//! one click. There is no time limit: a question stays, moved back to the
//! bottom of the channel as chat piles up, until someone answers it or it is
//! skipped (one admin or three members typing `!skip`). The quiz stays on
//! across restarts once started.
//!
//! Questions live in quiz.db, loaded at every startup from the JSONL files
//! under `{workspace}/quizbank/`. Points are a log, one row per point, so the
//! weekly board is a filter on the same table and the all-time leader holds
//! the Quiz Leader role.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use chrono::{Datelike, TimeZone, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, ComponentInteractionDataKind, Context,
    CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
    EditMessage, EditRole, GuildId, Message, MessageId, RoleId, UserId,
};
use regex::Regex;
use serenity::all::EditInteractionResponse;
use tokio::sync::Notify;

use crate::dependencies::VizierDependencies;
use crate::storage::VizierStorage;

const GAP: Duration = Duration::from_secs(4);
/// Time between two `!hint`s on the same question.
const HINT_COOLDOWN: Duration = Duration::from_secs(5);
/// `!skip` votes from different members that pass over a question. One admin is enough.
const SKIPS_NEEDED: usize = 3;
/// After a wrong multiple-choice pick, how long before that member may pick again.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// Wrong options `!hint` may knock out of a multiple-choice question.
const MAX_KNOCKOUTS: usize = 2;
/// Messages under the question before it is moved back to the bottom of the channel.
const STICKY_AFTER: u32 = 4;
/// Least time between two such moves, to stay clear of rate limits.
const STICKY_GAP: Duration = Duration::from_secs(6);
/// Pending /quizadd submissions one member may have waiting at once.
const MAX_PENDING: i64 = 5;
const ROLE_NAME: &str = "Quiz Leader";
const CREDITS: &str =
    "Questions: Open Trivia DB (CC BY-SA 4.0) · The Trivia API (CC BY-NC 4.0) · Wikidata · MLCI members";

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
static LIVE: LazyLock<Mutex<Option<Live>>> = LazyLock::new(|| Mutex::new(None));
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Set by `/quizstop`; the loop ends at its next step.
static STOP: AtomicBool = AtomicBool::new(false);
static LEADER_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
/// Held around every edit of the question message, so a late hint edit can
/// never land on top of the final "answered" / "time up" version.
static EDIT_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
/// Every answer, accepted form and option in the bank, normalised.
static KNOWN: LazyLock<parking_lot::RwLock<HashSet<String>>> =
    LazyLock::new(|| parking_lot::RwLock::new(HashSet::new()));

#[derive(Clone, Serialize, Deserialize)]
struct Question {
    id: String,
    kind: String,
    q: String,
    a: String,
    #[serde(default)]
    alt: Vec<String>,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    cat: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    diff: String,
    #[serde(default)]
    note: String,
    #[serde(default)]
    src: String,
    #[serde(skip)]
    added_by: Option<u64>,
    /// Scheduling theme and its side of the India/world split, from the database.
    #[serde(skip)]
    theme: String,
    #[serde(skip)]
    theme_region: String,
}

impl Question {
    fn is_mcq(&self) -> bool {
        self.kind == "mcq"
    }

    fn is_valid(&self) -> bool {
        let filled = !self.id.is_empty() && !self.q.trim().is_empty() && !self.a.trim().is_empty();
        let shape = match self.kind.as_str() {
            "text" => true,
            "mcq" => {
                let unique: HashSet<&String> = self.options.iter().collect();
                self.options.len() == 4 && unique.len() == 4 && self.options.contains(&self.a)
            }
            _ => false,
        };
        filled && shape && self.q.len() <= 1000
    }

    fn accepts(&self, guess: &str) -> bool {
        self.accepts_given(guess, &KNOWN.read())
    }

    /// An exact accepted form always scores. A slip is forgiven only when the
    /// guess isn't itself some other answer the bank knows: "Iceland" is not a
    /// typo of "Ireland", and "Jaipur" is not a typo of "Raipur".
    fn accepts_given(&self, guess: &str, known: &HashSet<String>) -> bool {
        let g = norm(guess);
        if g.is_empty() {
            return false;
        }
        let forms: Vec<&String> = std::iter::once(&self.a).chain(self.alt.iter()).collect();
        if forms.iter().any(|form| norm(form) == g) {
            return true;
        }
        !known.contains(&g) && forms.iter().any(|form| close_enough(guess, form))
    }
}

/// The question on screen right now.
struct Live {
    round: u64,
    question: Question,
    /// Multiple choice options in the order shown.
    options: Vec<String>,
    correct: usize,
    /// Wrong options knocked out by `!hint`.
    removed: Vec<usize>,
    /// Letters of each word the hint shows so far.
    hints: usize,
    last_hint: Option<std::time::Instant>,
    channel: ChannelId,
    /// The question message now showing; it changes when the question is moved down.
    message: MessageId,
    /// Messages posted under the question since it was last moved down.
    below: u32,
    last_bump: std::time::Instant,
    winner: Option<u64>,
    /// Multiple-choice guesses so far: when each member last picked wrong, and which options they ruled out.
    tried: std::collections::HashMap<u64, (std::time::Instant, HashSet<usize>)>,
    /// Who has typed `!skip` on this question.
    skip_votes: HashSet<u64>,
    /// Passed over by `!skip`.
    passed: bool,
    done: Arc<Notify>,
}

impl Live {
    /// Still waiting for an answer: not won and not skipped.
    fn is_open(&self) -> bool {
        self.winner.is_none() && !self.passed
    }
}

enum Shown {
    Open { hint: Option<String> },
    Won(u64),
    Skipped,
}

/// Clears the running flag however the quiz loop ends.
struct Running;

impl Drop for Running {
    fn drop(&mut self) {
        LIVE.lock().take();
        RUNNING.store(false, Ordering::SeqCst);
    }
}

/// How many questions come from the India pool, `VIZIER_QUIZ_INDIA_SHARE` (0 to 1).
fn india_share() -> f64 {
    std::env::var("VIZIER_QUIZ_INDIA_SHARE")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| (0.0..=1.0).contains(v))
        .unwrap_or(0.7)
}

/// Who gets the approval DMs for member and news questions, and may press
/// their buttons: `VIZIER_QUIZ_REVIEWERS` (comma-separated user ids), or the
/// bot admins when unset.
fn reviewers() -> Vec<u64> {
    std::env::var("VIZIER_QUIZ_REVIEWERS")
        .ok()
        .map(|raw| raw.split(',').filter_map(|s| s.trim().parse::<u64>().ok()).collect::<Vec<_>>())
        .filter(|ids| !ids.is_empty())
        .unwrap_or_else(super::admin_ids)
}

/// The quiz channel, from `VIZIER_QUIZ_CHANNEL`.
pub fn channel() -> Option<ChannelId> {
    std::env::var("VIZIER_QUIZ_CHANNEL").ok()?.trim().parse::<u64>().ok().map(ChannelId::new)
}

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let conn = open_conn(workspace)?;
    *KNOWN.write() = known_names(&conn);
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

fn known_names(conn: &Connection) -> HashSet<String> {
    let bodies: Vec<String> = conn
        .prepare("SELECT body FROM questions")
        .and_then(|mut s| s.query_map([], |r| r.get(0))?.collect())
        .unwrap_or_default();
    bodies
        .iter()
        .filter_map(|body| serde_json::from_str::<Question>(body).ok())
        .flat_map(|q| std::iter::once(q.a).chain(q.alt).chain(q.options).collect::<Vec<_>>())
        .map(|name| norm(&name))
        .filter(|name| !name.is_empty())
        .collect()
}

fn open_conn(workspace: &str) -> anyhow::Result<Connection> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let mut conn = Connection::open(dir.join("quiz.db"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS questions (
             id TEXT PRIMARY KEY, body TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'text',
             region TEXT NOT NULL, cat TEXT NOT NULL, src TEXT NOT NULL,
             active INTEGER NOT NULL DEFAULT 1, retired INTEGER NOT NULL DEFAULT 0,
             asked INTEGER NOT NULL DEFAULT 0, added_by INTEGER, added_ts INTEGER);
         CREATE INDEX IF NOT EXISTS questions_pick ON questions (active, retired, region, asked);
         CREATE TABLE IF NOT EXISTS flags (
             question_id TEXT NOT NULL, user_id INTEGER NOT NULL, PRIMARY KEY (question_id, user_id));
         CREATE TABLE IF NOT EXISTS points (
             id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL,
             question_id TEXT NOT NULL, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS points_user ON points (user_id, ts);
         CREATE TABLE IF NOT EXISTS submissions (
             id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, body TEXT NOT NULL,
             ts INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'pending', batch TEXT, link TEXT);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS rounds (
             id INTEGER PRIMARY KEY AUTOINCREMENT, genre TEXT NOT NULL, label TEXT NOT NULL,
             winner INTEGER, points INTEGER NOT NULL DEFAULT 0, answered INTEGER NOT NULL DEFAULT 0,
             asked INTEGER NOT NULL DEFAULT 0, top TEXT NOT NULL DEFAULT '[]', ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS rounds_genre ON rounds (genre, winner);",
    )?;
    // Themes came after the first deploy: add their columns to an existing database.
    let has_theme = {
        let mut columns = conn.prepare("PRAGMA table_info(questions)")?;
        let names = columns.query_map([], |r| r.get::<_, String>(1))?.collect::<Result<Vec<_>, _>>()?;
        names.iter().any(|name| name == "theme")
    };
    if !has_theme {
        conn.execute_batch(
            "ALTER TABLE questions ADD COLUMN theme TEXT NOT NULL DEFAULT '';
             ALTER TABLE questions ADD COLUMN theme_region TEXT NOT NULL DEFAULT 'india';",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS questions_theme ON questions (active, retired, theme_region, theme, asked);
         UPDATE questions SET theme = 'server', theme_region = 'india' WHERE src = 'member' AND theme = '';
         UPDATE questions SET theme = 'bollywood_news', theme_region = 'india' WHERE src = 'news' AND theme = '';",
    )?;
    let loaded = import_bank(&mut conn, &crate::utils::build_path(workspace, &["quizbank"]))?;
    let active: i64 =
        conn.query_row("SELECT COUNT(*) FROM questions WHERE active = 1 AND retired = 0", [], |r| r.get(0))?;
    tracing::info!("quiz: {} questions read from the bank, {} in play", loaded, active);
    Ok(conn)
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_jsonl(&path, out);
        } else if path.extension().is_some_and(|e| e == "jsonl") {
            out.push(path);
        }
    }
}

/// Upserts every question in the bank. Questions that have left the bank stop
/// being asked; member questions and report counts are never touched.
fn import_bank(conn: &mut Connection, dir: &Path) -> anyhow::Result<usize> {
    let mut files = Vec::new();
    collect_jsonl(dir, &mut files);
    if files.is_empty() {
        return Ok(0);
    }
    let moved = read_retheme(dir);
    let dropped = read_dropped(dir);
    let tx = conn.transaction()?;
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS seen (id TEXT PRIMARY KEY); DELETE FROM temp.seen;")?;
    let (mut loaded, mut skipped) = (0, 0);
    {
        let mut upsert = tx.prepare(
            "INSERT INTO questions (id, body, region, cat, src, kind, theme, theme_region)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET body = excluded.body, region = excluded.region,
                 cat = excluded.cat, src = excluded.src, kind = excluded.kind,
                 theme = excluded.theme, theme_region = excluded.theme_region",
        )?;
        let mut seen = tx.prepare("INSERT OR IGNORE INTO temp.seen (id) VALUES (?1)")?;
        for file in &files {
            let name = |p: Option<&std::ffi::OsStr>| p.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let folder = name(file.parent().and_then(|p| p.file_name()));
            let stem = name(file.file_stem());
            for line in std::fs::read_to_string(file)?.lines().filter(|l| !l.trim().is_empty()) {
                let Ok(mut q) = serde_json::from_str::<Question>(line) else {
                    skipped += 1;
                    continue;
                };
                if q.src.is_empty() || q.src == "member" {
                    q.src = q.id.split('-').next().unwrap_or("bank").to_string();
                }
                if q.region != "india" {
                    q.region = "world".into();
                }
                if !q.is_valid() {
                    skipped += 1;
                    continue;
                }
                // Left out on review: unseen, so it stops being asked like any removed question.
                if dropped.contains(&q.id) {
                    continue;
                }
                let (theme, theme_region) = match moved.get(&q.id) {
                    Some(theme) => (theme.clone(), side_of(theme).to_string()),
                    None => theme_for(&folder, &stem, &q.cat, &q.region),
                };
                upsert.execute(params![
                    q.id,
                    serde_json::to_string(&q)?,
                    q.region,
                    q.cat,
                    q.src,
                    q.kind,
                    theme,
                    theme_region
                ])?;
                seen.execute(params![q.id])?;
                loaded += 1;
            }
        }
    }
    if loaded > 0 {
        tx.execute(
            "UPDATE questions SET active = (id IN (SELECT id FROM temp.seen)) WHERE src != 'member'",
            [],
        )?;
    }
    tx.commit()?;
    if skipped > 0 {
        tracing::warn!("quiz: skipped {} malformed questions in the bank", skipped);
    }
    Ok(loaded)
}

/// A theme with at least this many questions gets a full turn; smaller ones
/// get a proportional share, so a handful of member questions don't repeat
/// every few rounds.
const THEME_FULL: i64 = 100;

/// Questions per round of the genre vote.
const BLOCK: u32 = 20;
/// How long the genre vote stays open.
const VOTE_TIME: Duration = Duration::from_secs(60);
const MIX: &str = "mix";

/// A group of themes players can vote for. Member questions are only in the mix.
struct Genre {
    key: &'static str,
    label: &'static str,
    /// Finishes "The next 20 questions are ...".
    about: &'static str,
    themes: &'static [&'static str],
}

/// In button order, five to a row: Indian entertainment, world entertainment,
/// fandoms, knowledge, then sports, life smarts and brain games beside the mix.
/// Every theme in the bank belongs to one genre.
const GENRES: &[Genre] = &[
    Genre {
        key: "bollywood",
        label: "🎬 Bollywood",
        about: "from Bollywood and Indian cinema",
        themes: &["bollywood", "bollywood_2000s", "bollywood_buzz", "bollywood_news", "regional_cinema", "india_films"],
    },
    Genre {
        key: "bollywood_songs",
        label: "🎶 Bollywood songs",
        about: "about Bollywood songs, singers and composers",
        themes: &["bollywood_songs"],
    },
    Genre {
        key: "indian_pop",
        label: "📺 Indian pop culture",
        about: "from Indian pop culture: TV, ads, memes and nostalgia",
        themes: &["indian_pop_culture", "desi_pop", "indian_tv_ott"],
    },
    Genre {
        key: "cricket",
        label: "🏏 Cricket",
        about: "from cricket and the IPL",
        themes: &["cricket", "cricket_players", "ipl"],
    },
    Genre {
        key: "india",
        label: "🗺️ India",
        about: "about India's states, places and culture",
        themes: &["north_india", "south_india", "east_northeast", "west_central", "india_map", "geography", "culture"],
    },
    Genre {
        key: "hollywood",
        label: "🎥 Hollywood movies",
        about: "from Hollywood and world cinema",
        themes: &["hollywood", "world_films"],
    },
    Genre {
        key: "tv",
        label: "📺 TV shows",
        about: "from TV series, sitcoms and cartoons",
        themes: &["world_tv", "tv_shows"],
    },
    Genre {
        key: "music",
        label: "🎵 Music",
        about: "about music from India and the world",
        themes: &["music_tv", "indian_pop_music", "world_music"],
    },
    Genre {
        key: "games",
        label: "🎮 Video games",
        about: "from video games and gaming",
        themes: &["video_games", "anime_gaming"],
    },
    Genre {
        key: "anime",
        label: "🍥 Anime & comics",
        about: "from anime, manga and comics",
        themes: &["anime_comics"],
    },
    Genre {
        key: "harry_potter",
        label: "⚡ Harry Potter",
        about: "from the world of Harry Potter",
        themes: &["harry_potter"],
    },
    Genre {
        key: "game_of_thrones",
        label: "🐉 Game of Thrones",
        about: "from Game of Thrones",
        themes: &["game_of_thrones"],
    },
    Genre {
        key: "mcu",
        label: "🦸 Marvel movies",
        about: "from the Marvel Cinematic Universe films",
        themes: &["mcu_movies"],
    },
    Genre {
        key: "pokemon",
        label: "🔴 Pokémon",
        about: "all about Pokémon",
        themes: &["pokemon"],
    },
    Genre {
        key: "disney",
        label: "🏰 Disney",
        about: "from Disney and Pixar films and Disney TV shows",
        themes: &["disney"],
    },
    Genre {
        key: "world_geography",
        label: "🌍 World geography",
        about: "about countries, capitals and the world map",
        themes: &["world_geography"],
    },
    Genre {
        key: "history",
        label: "📜 History & mythology",
        about: "from history and mythology",
        themes: &["history_civics", "mythology_tales", "world_history"],
    },
    Genre {
        key: "gk",
        label: "📚 GK & books",
        about: "general knowledge, books, words and business",
        themes: &["indian_gk", "general_knowledge", "society_culture", "arts_books", "hindi", "india_books"],
    },
    Genre {
        key: "science",
        label: "🔬 Science & tech",
        about: "from science, space and technology",
        themes: &["science_tech", "indian_science"],
    },
    Genre {
        key: "food",
        label: "🍛 Food",
        about: "about food, cooking and drinks",
        themes: &["indian_food", "food_drink", "food_science"],
    },
    Genre {
        key: "sports",
        label: "⚽ Sports",
        about: "from football, F1 and sport around the world",
        themes: &["football_f1", "world_sport", "indian_sports"],
    },
    Genre {
        key: "life",
        label: "💡 Life smarts",
        about: "useful home, money, work and tech smarts",
        themes: &[
            "home_science", "beauty_science", "work_abbreviations", "did_you_know", "daily_life_india", "money_smarts",
            "tech_smarts",
        ],
    },
    Genre {
        key: "brain",
        label: "🧩 Brain games",
        about: "riddles, logic puzzles and brain teasers",
        themes: &["riddles", "logical_reasoning", "brain_teasers"],
    },
];

fn genre(key: &str) -> Option<&'static Genre> {
    GENRES.iter().find(|g| g.key == key)
}

/// The genre vote while it is open: which keys are on offer and who picked what.
struct Vote {
    id: u32,
    keys: Vec<&'static str>,
    votes: std::collections::HashMap<u64, &'static str>,
}

static VOTE: LazyLock<Mutex<Option<Vote>>> = LazyLock::new(|| Mutex::new(None));

/// The current round (the questions between two genre votes): its own points,
/// separate from the all-time ones, announced as a top 3 when the round ends.
/// Kept in memory; a restart starts a fresh round.
#[derive(Default)]
struct Board {
    /// The genre key, or `MIX`.
    key: String,
    label: String,
    asked: u32,
    /// user -> (points this round, order of the point that reached that total)
    scores: HashMap<u64, (u32, u64)>,
    wins: u64,
}

static BOARD: LazyLock<Mutex<Board>> = LazyLock::new(|| Mutex::new(Board::default()));

fn new_round(key: &str, label: &str) {
    *BOARD.lock() = Board { key: key.to_string(), label: label.to_string(), ..Default::default() };
}

/// +1 for this round; returns the member's round total.
fn round_point(user: u64) -> u32 {
    let mut board = BOARD.lock();
    board.wins += 1;
    let order = board.wins;
    let entry = board.scores.entry(user).or_insert((0, 0));
    entry.0 += 1;
    entry.1 = order;
    entry.0
}

/// Most points first; on a tie, whoever got there first.
fn standings(scores: &HashMap<u64, (u32, u64)>) -> Vec<(u64, u32, u64)> {
    let mut rows: Vec<(u64, u32, u64)> = scores.iter().map(|(u, (p, o))| (*u, *p, *o)).collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    rows
}

/// The end-of-round message: the round's top 3, and its winner crowned with
/// `crowns`, how many rounds of this genre they have now won.
fn round_summary(board: &Board, crowns: Option<i64>) -> String {
    let rows = standings(&board.scores);
    let answered: u32 = rows.iter().map(|r| r.1).sum();
    let mut text = format!("🏁 **{} round over!** {} of {} questions answered.", board.label, answered, board.asked);
    if rows.is_empty() {
        text.push_str("\nNobody scored this round.");
        return text;
    }
    text.push_str(" Top of the round:");
    for (i, (user, points, _)) in rows.iter().take(3).enumerate() {
        let medal = ["🥇", "🥈", "🥉"][i];
        text.push_str(&format!("\n{} <@{}> · **{}**", medal, user, points));
    }
    if let (Some((user, _, _)), Some(n)) = (rows.first(), crowns) {
        let plural = if n == 1 { "" } else { "s" };
        text.push_str(&format!(
            "\n👑 <@{}> is crowned the **{}** champion · {} crown{} in this genre",
            user, board.label, n, plural
        ));
    }
    let shown = rows.len().min(3);
    let tied = rows.windows(2).take(shown).any(|w| w[0].1 == w[1].1);
    if tied {
        text.push_str("\n-# Tied scores go to whoever got there first.");
    }
    text
}

/// Saves a finished round for the genre champions board. Returns how many
/// rounds of this genre its winner has now won, or `None` if nobody scored.
fn record_round(conn: &Connection, board: &Board) -> Option<i64> {
    let rows = standings(&board.scores);
    let answered: u32 = rows.iter().map(|r| r.1).sum();
    let top: Vec<(u64, u32)> = rows.iter().take(3).map(|r| (r.0, r.1)).collect();
    let winner = rows.first().map(|r| (r.0, r.1));
    let saved = conn.execute(
        "INSERT INTO rounds (genre, label, winner, points, answered, asked, top, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            board.key,
            board.label,
            winner.map(|w| w.0 as i64),
            winner.map(|w| w.1).unwrap_or(0),
            answered,
            board.asked,
            serde_json::to_string(&top).unwrap_or_else(|_| "[]".into()),
            Utc::now().timestamp()
        ],
    );
    if let Err(err) = saved {
        tracing::warn!("quiz: round not recorded: {}", err);
    }
    let (user, _) = winner?;
    conn.query_row(
        "SELECT COUNT(*) FROM rounds WHERE genre = ?1 AND winner = ?2",
        params![board.key, user as i64],
        |r| r.get(0),
    )
    .ok()
}

/// One line per genre that has had a round won: its reigning champion (the
/// latest round's winner) and whoever has won it most often.
fn champion_lines(conn: &Connection) -> Vec<String> {
    let genres = GENRES.iter().map(|g| (g.key, g.label)).chain(std::iter::once((MIX, "🎲 Mix")));
    let mut lines = Vec::new();
    for (key, label) in genres {
        let reigning: Option<i64> = conn
            .query_row(
                "SELECT winner FROM rounds WHERE genre = ?1 AND winner IS NOT NULL ORDER BY id DESC LIMIT 1",
                params![key],
                |r| r.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let Some(reigning) = reigning else {
            continue;
        };
        // On a tie, whoever reached that many crowns first.
        let most: Option<(i64, i64)> = conn
            .query_row(
                "SELECT winner, COUNT(*) AS n FROM rounds WHERE genre = ?1 AND winner IS NOT NULL
                 GROUP BY winner ORDER BY n DESC, MAX(id) ASC LIMIT 1",
                params![key],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        lines.push(match most {
            Some((user, n)) if user != reigning => format!("{} · 👑 <@{}> · 🏆 <@{}> ×{}", label, reigning, user, n),
            Some((_, n)) => format!("{} · 👑🏆 <@{}> ×{}", label, reigning, n),
            None => format!("{} · 👑 <@{}>", label, reigning),
        });
    }
    lines
}

/// The theme a bank question is scheduled under, and which side of the
/// India/world split that theme sits on. Every written topic is its own theme;
/// the Wikidata templates and the downloaded databases' many categories are
/// grouped into broad themes, so each gets about as many turns as one written
/// topic. A question keeps its own region for the flag it shows.
fn theme_for(folder: &str, stem: &str, cat: &str, region: &str) -> (String, String) {
    let theme: &str = match folder {
        // The few early files that mixed subjects send each part to the theme it belongs with.
        "ai" => match (stem, cat) {
            ("culture", "food") => "indian_food",
            ("culture", "mythology") => "mythology_tales",
            ("desi_pop", "gaming") => "video_games",
            ("music_tv", "tv") => "indian_tv_ott",
            ("sports_science_biz", "business") => "indian_gk",
            ("sports_science_biz", "science" | "space") => "indian_science",
            ("sports_science_biz", _) => "indian_sports",
            _ => stem,
        },
        "wikidata" => match stem {
            "country_capital" | "country_currency" | "calling_code" => "world_geography",
            "element_symbol" => "science_tech",
            "hindi_film_director" | "hindi_film_year" => "india_films",
            "indian_book_author" => "india_books",
            _ => "india_map",
        },
        "api" => match cat {
            "film_and_tv" | "film" => "world_films",
            "television" | "cartoon_animations" => "tv_shows",
            "musicals_theatres" => "arts_books",
            "music" => "world_music",
            "science" | "science_nature" | "computers" | "mathematics" | "gadgets" | "animals" | "vehicles" => {
                "science_tech"
            }
            "geography" => "world_geography",
            "history" | "mythology" | "politics" => "world_history",
            "sport_and_leisure" | "sports" => "world_sport",
            "video_games" | "board_games" => "video_games",
            "japanese_anime_manga" | "comics" => "anime_comics",
            "arts_and_literature" | "art" | "books" => "arts_books",
            "food_and_drink" => "food_drink",
            "society_and_culture" => "society_culture",
            _ => "general_knowledge",
        },
        // Anything else (tests, ad-hoc files): the category is the theme, on the question's own side.
        _ => return (cat.to_string(), (if region == "india" { "india" } else { "world" }).to_string()),
    };
    (theme.to_string(), side_of(theme).to_string())
}

/// Themes on the world side of the India/world split; every other theme is India's.
const WORLD_THEMES: &[&str] = &[
    "world_tv", "hollywood", "anime_gaming", "football_f1", "harry_potter", "game_of_thrones", "mcu_movies", "pokemon",
    "disney", "world_films", "tv_shows", "video_games", "anime_comics", "world_music", "science_tech", "world_geography",
    "world_history", "world_sport", "arts_books", "food_drink", "society_culture", "general_knowledge",
];

fn side_of(theme: &str) -> &'static str {
    if WORLD_THEMES.contains(&theme) { "world" } else { "india" }
}

/// `dropped.json` in the bank: questions left out on review (too old, too
/// niche, dated...), as `{reason: [question ids]}`, so a fresh download of a
/// database doesn't bring them back.
fn read_dropped(dir: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("dropped.json")) else {
        return HashSet::new();
    };
    match serde_json::from_str::<HashMap<String, Vec<String>>>(&text) {
        Ok(reasons) => reasons.into_values().flatten().collect(),
        Err(err) => {
            tracing::warn!("quiz: dropped.json ignored: {}", err);
            HashSet::new()
        }
    }
}

/// `retheme.json` in the bank: hand-picked questions from other files that
/// belong to a newer topic's theme (the Pokémon questions from the downloaded
/// databases, say), as `{theme: [question ids]}`. Returns id -> theme.
fn read_retheme(dir: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(dir.join("retheme.json")) else {
        return HashMap::new();
    };
    match serde_json::from_str::<HashMap<String, Vec<String>>>(&text) {
        Ok(themes) => themes
            .into_iter()
            .flat_map(|(theme, ids)| ids.into_iter().map(move |id| (id, theme.clone())))
            .collect(),
        Err(err) => {
            tracing::warn!("quiz: retheme.json ignored: {}", err);
            HashMap::new()
        }
    }
}

fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

fn meta_set(conn: &Connection, key: &str, value: &str) {
    let _ = conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    );
}

/// What the last few questions were, so the next one can differ.
#[derive(Default)]
struct Recent {
    themes: Vec<String>,
    kinds: Vec<String>,
    sides: Vec<String>,
}

impl Recent {
    fn push(&mut self, q: &Question) {
        for (list, value, keep) in
            [(&mut self.themes, &q.theme, 3), (&mut self.kinds, &q.kind, 2), (&mut self.sides, &q.theme_region, 2)]
        {
            list.push(value.clone());
            if list.len() > keep {
                list.remove(0);
            }
        }
    }

    /// Set when the last two were the same, so a third in a row is avoided.
    fn streak(list: &[String]) -> Option<&str> {
        match list {
            [a, b] if a == b => Some(a.as_str()),
            _ => None,
        }
    }
}

/// News questions stop being asked after this long.
const NEWS_SHELF_LIFE: i64 = 90 * 24 * 3600;

/// India or world (`india_share`, never three world themes in a row), then a
/// theme on that side - every theme an equal chance, however many questions
/// it has - then that theme's least-asked question. The last few themes sit
/// out a turn, and a third typed or third multiple-choice question in a row
/// is avoided whenever the pool allows it.
fn pick(recent: &Recent, genre: Option<&[&str]>) -> Option<Question> {
    pick_from(&DB.get()?.lock(), recent, genre)
}

fn pick_from(conn: &Connection, recent: &Recent, genre: Option<&[&str]>) -> Option<Question> {
    if let Some(themes) = genre {
        // A voted genre: only its themes, from either side, with the same variety rules.
        let streak_kind = Recent::streak(&recent.kinds).unwrap_or("");
        let themes = serde_json::to_string(themes).ok()?;
        for (avoid_kind, fresh_topic) in [(streak_kind, true), (streak_kind, false), ("", true), ("", false)] {
            if let Some(q) = pick_one(conn, "", &themes, avoid_kind, fresh_topic.then_some(&recent.themes[..])) {
                return Some(q);
            }
        }
        // A genre with nothing left to ask falls back to the mix.
    }
    let world_streak = Recent::streak(&recent.sides) == Some("world");
    let preferred: &[&str] = if world_streak {
        &["india"]
    } else if rand::random::<f64>() < india_share() {
        &["india", "world"]
    } else {
        &["world", "india"]
    };
    let streak_kind = Recent::streak(&recent.kinds).unwrap_or("");
    // Loosen one rule at a time: kind and topic both fresh, then only the
    // kind, then only the topic, then anything. Only when all of that fails
    // in the preferred regions may a third world question in a row through.
    let rules = [(streak_kind, true), (streak_kind, false), ("", true), ("", false)];
    for regions in [preferred, &["india", "world"][..]] {
        for (avoid_kind, fresh_topic) in rules {
            for region in regions {
                if let Some(q) = pick_one(conn, region, "[]", avoid_kind, fresh_topic.then_some(&recent.themes[..])) {
                    return Some(q);
                }
            }
        }
    }
    None
}

/// The least-asked question of a random theme. `side` limits it to one side of
/// the India/world split ("" for both) and `only` to a JSON list of themes
/// ("[]" for all). Themes of at least `THEME_FULL` questions are equally
/// likely; smaller ones proportionally less.
fn pick_one(
    conn: &Connection,
    side: &str,
    only: &str,
    avoid_kind: &str,
    avoid_themes: Option<&[String]>,
) -> Option<Question> {
    let stale_news = Utc::now().timestamp() - NEWS_SHELF_LIFE;
    let playable = "active = 1 AND retired = 0 AND (?1 = '' OR theme_region = ?1)
                    AND (?4 = '[]' OR theme IN (SELECT value FROM json_each(?4)))
                    AND kind != ?2 AND NOT (src = 'news' AND COALESCE(added_ts, 0) < ?3)";
    let themes: Vec<(String, i64)> = conn
        .prepare(&format!("SELECT theme, COUNT(*) FROM questions WHERE {} GROUP BY theme", playable))
        .and_then(|mut s| {
            s.query_map(params![side, avoid_kind, stale_news, only], |r| Ok((r.get(0)?, r.get(1)?)))?.collect()
        })
        .unwrap_or_default();
    let pool: Vec<&(String, i64)> =
        themes.iter().filter(|(theme, _)| !avoid_themes.is_some_and(|recent| recent.contains(theme))).collect();
    let last = pool.last()?;
    let weight = |n: i64| n.min(THEME_FULL) as f64;
    let mut roll = rand::random::<f64>() * pool.iter().map(|(_, n)| weight(*n)).sum::<f64>();
    let (theme, _) = pool
        .iter()
        .find(|(_, n)| {
            roll -= weight(*n);
            roll <= 0.0
        })
        .unwrap_or(last);
    let (id, body, added_by, theme_region): (String, String, Option<i64>, String) = conn
        .query_row(
            &format!(
                "SELECT id, body, added_by, theme_region FROM questions WHERE {} AND theme = ?5
                 ORDER BY asked, random() LIMIT 1",
                playable
            ),
            params![side, avoid_kind, stale_news, only, theme],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()
        .ok()
        .flatten()?;
    let _ = conn.execute("UPDATE questions SET asked = asked + 1 WHERE id = ?1", params![id]);
    let mut q = serde_json::from_str::<Question>(&body).ok()?;
    q.added_by = added_by.map(|u| u as u64);
    q.theme = theme.clone();
    q.theme_region = theme_region;
    Some(q)
}

fn theme_count(conn: &Connection, themes: &[&str]) -> i64 {
    let only = serde_json::to_string(themes).unwrap_or_else(|_| "[]".into());
    conn.query_row(
        "SELECT COUNT(*) FROM questions WHERE active = 1 AND retired = 0
         AND theme IN (SELECT value FROM json_each(?1))",
        params![only],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

fn vote_buttons(
    id: u32,
    keys: &[&'static str],
    votes: &std::collections::HashMap<u64, &'static str>,
    open: bool,
) -> Vec<CreateActionRow> {
    let count = |key: &str| votes.values().filter(|v| **v == key).count();
    let top = keys.iter().map(|k| count(k)).max().unwrap_or(0);
    let buttons: Vec<CreateButton> = keys
        .iter()
        .map(|key| {
            let label = if *key == MIX { "🎲 Mix" } else { genre(key).map(|g| g.label).unwrap_or(key) };
            let n = count(key);
            let style = if n > 0 && n == top { ButtonStyle::Primary } else { ButtonStyle::Secondary };
            CreateButton::new(format!("quizvote:{}:{}", id, key))
                .label(format!("{} · {}", label, n))
                .style(style)
                .disabled(!open)
        })
        .collect();
    buttons.chunks(5).map(|row| CreateActionRow::Buttons(row.to_vec())).collect()
}

/// The genre vote between blocks of questions. Returns the winning genre, or
/// `None` for the mix (mix won, nobody voted, or there's nothing to choose).
async fn run_vote(ctx: &Context, channel: ChannelId) -> Option<&'static Genre> {
    let keys: Vec<&'static str> = {
        let db = DB.get()?;
        let conn = db.lock();
        GENRES
            .iter()
            .filter(|g| theme_count(&conn, g.themes) > 0)
            .map(|g| g.key)
            .chain(std::iter::once(MIX))
            .collect()
    };
    if keys.len() < 3 {
        return None;
    }
    let id = rand::random::<u32>();
    *VOTE.lock() = Some(Vote { id, keys: keys.clone(), votes: Default::default() });
    let ends = Utc::now().timestamp() + VOTE_TIME.as_secs() as i64;
    let invite = format!(
        "🗳️ **Vote for the next {} questions!** Voting closes <t:{}:R>. Most votes wins; a tie is settled at random.",
        BLOCK, ends
    );
    let sent = channel
        .send_message(
            &ctx.http,
            CreateMessage::new().content(invite).components(vote_buttons(id, &keys, &Default::default(), true)),
        )
        .await;
    tokio::time::sleep(VOTE_TIME).await;
    let vote = VOTE.lock().take()?;
    let tally = |key: &str| vote.votes.values().filter(|v| **v == key).count();
    let top = vote.keys.iter().map(|k| tally(k)).max().unwrap_or(0);
    let winner: Option<&'static str> = (top > 0).then(|| {
        let tied: Vec<&'static str> = vote.keys.iter().copied().filter(|k| tally(k) == top).collect();
        tied[rand::random::<u32>() as usize % tied.len()]
    });
    if let Ok(message) = sent {
        let closed = EditMessage::new()
            .content("🗳️ Voting closed.")
            .components(vote_buttons(id, &vote.keys, &vote.votes, false));
        let _ = channel.edit_message(&ctx.http, message.id, closed).await;
    }
    let plural = if top == 1 { "" } else { "s" };
    let chosen = winner.and_then(genre);
    let result = match (winner, chosen) {
        (None, _) => format!("Nobody voted, so the next {} questions are a mix of everything.", BLOCK),
        (Some(_), Some(g)) => {
            format!("{} wins with **{}** vote{}! The next {} questions are {}.", g.label, top, plural, BLOCK, g.about)
        }
        (Some(_), None) => {
            format!("🎲 **Mix** wins with **{}** vote{}! The next {} questions are from everything.", top, plural, BLOCK)
        }
    };
    let _ = channel.say(&ctx.http, result).await;
    tokio::time::sleep(GAP).await;
    chosen
}

async fn cast_vote(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let mut parts = rest.splitn(2, ':');
    let (Some(Ok(id)), Some(key)) = (parts.next().map(str::parse::<u32>), parts.next()) else {
        return;
    };
    let rows = {
        let mut guard = VOTE.lock();
        match guard.as_mut() {
            Some(vote) if vote.id == id => match vote.keys.iter().copied().find(|k| *k == key) {
                Some(choice) => {
                    vote.votes.insert(component.user.id.get(), choice);
                    Some(vote_buttons(vote.id, &vote.keys, &vote.votes, true))
                }
                None => None,
            },
            _ => None,
        }
    };
    match rows {
        Some(rows) => {
            let update = CreateInteractionResponseMessage::new().components(rows);
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
        }
        None => whisper(ctx, component, "This vote has closed.").await,
    }
}

fn next_round() -> u64 {
    let Some(db) = DB.get() else {
        return 0;
    };
    let conn = db.lock();
    let round = meta_get(&conn, "round").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + 1;
    meta_set(&conn, "round", &round.to_string());
    round
}

fn add_point(user: u64, question: &str) -> i64 {
    let Some(db) = DB.get() else {
        return 0;
    };
    let conn = db.lock();
    let _ = conn.execute(
        "INSERT INTO points (user_id, question_id, ts) VALUES (?1, ?2, ?3)",
        params![user as i64, question, Utc::now().timestamp()],
    );
    conn.query_row("SELECT COUNT(*) FROM points WHERE user_id = ?1", params![user as i64], |r| r.get(0))
        .unwrap_or(0)
}

// --- answers ----------------------------------------------------------------

/// Lower case, punctuation gone, a leading "the"/"a"/"an" dropped, spaces removed.
fn norm(text: &str) -> String {
    let lower = text.to_lowercase().replace('&', " and ");
    let cleaned: String = lower.chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let words = match words.first() {
        Some(&("the" | "a" | "an")) if words.len() > 1 => &words[1..],
        _ => &words[..],
    };
    words.concat()
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + usize::from(ca != cb));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Romanised Hindi is spelled many ways: Kabeer/Kabir, kotwaal/kotwal,
/// Pawan/Pavan, Phir/Fir. Folds those to one spelling before comparing.
fn fold(normed: &str) -> String {
    let swapped = normed.replace("aa", "a").replace("ee", "i").replace("oo", "u").replace("ph", "f").replace('w', "v");
    let mut out = String::with_capacity(swapped.len());
    for c in swapped.chars() {
        if out.chars().last() != Some(c) {
            out.push(c);
        }
    }
    out
}

/// Exact after normalising and folding Hinglish spellings, or a slip or two on
/// longer answers. Anything with a digit in it - years, scores, codes - has to
/// be exact.
fn close_enough(guess: &str, answer: &str) -> bool {
    let (g, a) = (norm(guess), norm(answer));
    if g.is_empty() || a.is_empty() {
        return false;
    }
    if g == a {
        return true;
    }
    let digits = a.chars().chain(g.chars()).any(|c| c.is_ascii_digit());
    let (g, a) = (fold(&g), fold(&a));
    if !digits && g == a {
        return true;
    }
    // A digit on either side must match exactly: "Dhoom 2" is not a typo of "Dhoom".
    if a.chars().chain(g.chars()).any(|c| c.is_ascii_digit()) {
        return false;
    }
    let slack = match a.chars().count() {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    };
    slack > 0 && levenshtein(&g, &a) <= slack
}

/// The first `level` letters of each word, the rest blanked: `M u _ _ _ _`.
/// Never more than half of a word (rounded up), so hints alone can't give it away.
fn hint_at(answer: &str, level: usize) -> String {
    answer
        .split_whitespace()
        .map(|word| {
            let letters = word.chars().filter(|c| c.is_alphanumeric()).count();
            let show = level.min(letters.div_ceil(2)).max(1);
            let mut seen = 0;
            word.chars()
                .map(|c| {
                    if !c.is_alphanumeric() {
                        return c.to_string();
                    }
                    seen += 1;
                    if seen <= show { c.to_string() } else { "_".into() }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("   ")
}

fn shuffled(options: &[String], answer: &str) -> (Vec<String>, usize) {
    let mut keyed: Vec<(u64, String)> = options.iter().map(|o| (rand::random::<u64>(), o.clone())).collect();
    keyed.sort_by_key(|(k, _)| *k);
    let options: Vec<String> = keyed.into_iter().map(|(_, o)| o).collect();
    let correct = options.iter().position(|o| o == answer).unwrap_or(0);
    (options, correct)
}

// --- drawing ----------------------------------------------------------------

fn pretty(cat: &str) -> String {
    cat.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn embed(round: u64, q: &Question, shown: &Shown) -> CreateEmbed {
    let mut text = if q.src == "news" { "📰 **Bollywood news**\n".to_string() } else { String::new() };
    text.push_str(&format!("**{}**\n", q.q));
    match shown {
        Shown::Open { hint } => {
            let how = if q.is_mcq() { "👇 Pick an option · a wrong pick waits 1 min" } else { "✍️ Type your answer" };
            text.push_str(&format!("\n{} · no time limit · stuck? `!hint` or `!skip`", how));
            if let Some(hint) = hint {
                text.push_str(&format!("\n💡 Hint: `{}`", hint));
            }
        }
        Shown::Won(user) => text.push_str(&format!("\n✅ <@{}> got it: **{}**", user, q.a)),
        Shown::Skipped => text.push_str(&format!("\n⏭️ Skipped. The answer was **{}**", q.a)),
    }
    if matches!(shown, Shown::Won(_) | Shown::Skipped) && !q.note.is_empty() {
        text.push_str(&format!("\n_{}_", q.note));
    }
    if let Some(by) = q.added_by {
        text.push_str(&format!("\n\nQuestion by <@{}>", by));
    }
    let colour = match shown {
        Shown::Open { .. } => 0x5865F2,
        Shown::Won(_) => 0x57F287,
        Shown::Skipped => 0x95A5A6,
    };
    let flag = if q.region == "india" { "🇮🇳" } else { "🌍" };
    let mut footer = format!("{} {}", flag, pretty(&q.cat));
    if !q.diff.is_empty() {
        footer.push_str(&format!(" · {}", q.diff));
    }
    {
        let board = BOARD.lock();
        if board.asked > 0 {
            footer.push_str(&format!(" · {} round {}/{}", board.label, board.asked, BLOCK));
        }
    }
    CreateEmbed::new()
        .title(format!("Question #{}", round))
        .description(text)
        .colour(colour)
        .footer(CreateEmbedFooter::new(footer))
}

fn components(
    round: u64,
    q: &Question,
    options: &[String],
    correct: usize,
    open: bool,
    removed: &[usize],
) -> Vec<CreateActionRow> {
    let mut rows = Vec::new();
    if q.is_mcq() {
        for (i, option) in options.iter().enumerate() {
            let mut label = format!("{}. {}", ['A', 'B', 'C', 'D'][i.min(3)], option);
            if label.chars().count() > 80 {
                label = label.chars().take(79).collect::<String>() + "…";
            }
            let style = match (open, i == correct) {
                (false, true) => ButtonStyle::Success,
                _ => ButtonStyle::Secondary,
            };
            let gone = open && removed.contains(&i);
            if gone {
                label = clip(&format!("✖ {}", label), 80);
            }
            let button =
                CreateButton::new(format!("quiz:{}:{}", round, i)).label(label).style(style).disabled(!open || gone);
            rows.push(CreateActionRow::Buttons(vec![button]));
        }
    }
    rows.push(CreateActionRow::Buttons(vec![
        CreateButton::new(format!("quizflag:{}", q.id)).label("🚩 Report question").style(ButtonStyle::Secondary),
    ]));
    rows
}

fn celebrate(user: u64, q: &Question, this_round: u32, total: i64) -> String {
    let verbs = ["got it", "nailed it", "was fastest", "is spot on", "takes the point"];
    let verb = verbs[rand::random::<u32>() as usize % verbs.len()];
    format!("✅ <@{}> {}! Answer: **{}** · +1 · this round **{}** · total **{}**", user, verb, q.a, this_round, total)
}

// --- the loop ---------------------------------------------------------------

pub async fn start_command(
    ctx: &Context,
    storage: &Arc<VizierStorage>,
    agent_id: &str,
    command: &CommandInteraction,
) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(home) = channel() else {
        let _ = command.create_response(&ctx.http, whisper("The quiz channel isn't set up yet.".into())).await;
        return;
    };
    let refusal = if command.channel_id != home {
        Some(format!("The quiz only runs in <#{}>.", home))
    } else if super::is_paused(storage, agent_id).await {
        Some("The bot is paused right now.".into())
    } else if DB.get().is_none() {
        Some("The quiz isn't working right now. Let an admin know.".into())
    } else if RUNNING.swap(true, Ordering::SeqCst) {
        Some("A quiz is already running. The current question is in the channel.".into())
    } else {
        None
    };
    if let Some(text) = refusal {
        let _ = command.create_response(&ctx.http, whisper(text)).await;
        return;
    }
    let count: i64 = DB
        .get()
        .map(|db| {
            db.lock()
                .query_row("SELECT COUNT(*) FROM questions WHERE active = 1 AND retired = 0", [], |r| r.get(0))
                .unwrap_or(0)
        })
        .unwrap_or(0);
    let intro = format!(
        "🧠 **Quiz started!** {} questions ready.\n\
         ✍️ Typed questions: the first correct answer wins, and small spelling slips are fine.\n\
         👇 Multiple choice: press a button. A wrong pick means a 1-minute wait before you can pick again.\n\
         💡 Type `!hint`: one more letter on a typed question, one wrong option removed on multiple choice.\n\
         ⏭️ No time limit: a question stays until someone gets it. Stuck? Try `!hint`, or skip it: {} people typing `!skip`, or one admin.\n\
         🗳️ Rounds of {} questions: each round has its own scores, its winner is crowned that genre's champion (`/quizleaderboard` → Genre champions), then everyone votes on the next round's genre.\n\
         Every correct answer = **+1 point** · `/quizleaderboard` · the top scorer gets 👑 **{}**\n\
         -# Send in your own question with `/quizadd`",
        count, SKIPS_NEEDED, BLOCK, ROLE_NAME
    );
    let _ = command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(intro)),
        )
        .await;
    set_running(true);
    tokio::spawn(run(ctx.clone(), storage.clone(), agent_id.to_string(), home));
}

/// `/quizstop`: an admin ends the quiz. It stays off, restarts included, until
/// someone runs `/quiz` again.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let text = if !super::admin_ids().contains(&command.user.id.get()) {
        "Only admins can stop the quiz."
    } else if !RUNNING.load(Ordering::SeqCst) {
        set_running(false);
        "The quiz isn't running."
    } else {
        set_running(false);
        STOP.store(true, Ordering::SeqCst);
        let mut guard = LIVE.lock();
        if let Some(live) = guard.as_mut().filter(|live| live.is_open()) {
            live.passed = true;
            live.done.notify_one();
        }
        "Stopping the quiz."
    };
    let reply = CreateInteractionResponseMessage::new().content(text).ephemeral(true);
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

/// Remembers across restarts whether the quiz is on.
fn set_running(on: bool) {
    if let Some(db) = DB.get() {
        meta_set(&db.lock(), "running", if on { "1" } else { "0" });
    }
}

/// After a restart, carries on a quiz that was running before it.
pub fn resume(ctx: &Context, storage: &Arc<VizierStorage>, agent_id: &str) {
    let Some(home) = channel() else {
        return;
    };
    let was_running = DB.get().is_some_and(|db| meta_get(&db.lock(), "running").as_deref() == Some("1"));
    if was_running && !RUNNING.swap(true, Ordering::SeqCst) {
        tracing::info!("quiz: resuming after restart");
        tokio::spawn(run(ctx.clone(), storage.clone(), agent_id.to_string(), home));
    }
}

async fn run(ctx: Context, storage: Arc<VizierStorage>, agent_id: String, channel: ChannelId) {
    let _running = Running;
    STOP.store(false, Ordering::SeqCst);
    let mut recent = Recent::default();
    // The first block after a start is a mix; every block after that is voted on.
    let mut genre: Option<&'static Genre> = None;
    let mut in_block = 0u32;
    new_round(MIX, "🎲 Mix");
    tokio::time::sleep(Duration::from_secs(2)).await;
    loop {
        if STOP.load(Ordering::SeqCst) {
            let _ = channel.say(&ctx.http, "🛑 The quiz has been stopped by an admin. Start it again with `/quiz`.").await;
            break;
        }
        // A pause holds the quiz rather than ending it, so it carries on after /resume.
        if super::is_paused(&storage, &agent_id).await {
            tokio::time::sleep(Duration::from_secs(60)).await;
            continue;
        }
        if in_block >= BLOCK {
            let finished = std::mem::take(&mut *BOARD.lock());
            let crowns = DB.get().and_then(|db| record_round(&db.lock(), &finished));
            let summary = round_summary(&finished, crowns);
            let _ = channel
                .send_message(&ctx.http, CreateMessage::new().content(summary).allowed_mentions(CreateAllowedMentions::new()))
                .await;
            tokio::time::sleep(GAP).await;
            genre = run_vote(&ctx, channel).await;
            new_round(genre.map(|g| g.key).unwrap_or(MIX), genre.map(|g| g.label).unwrap_or("🎲 Mix"));
            in_block = 0;
            continue;
        }
        let Some(question) = pick(&recent, genre.map(|g| g.themes)) else {
            let _ = channel.say(&ctx.http, "Out of questions! Ask an admin to add more.").await;
            set_running(false);
            break;
        };
        recent.push(&question);
        in_block += 1;
        BOARD.lock().asked = in_block;
        let round = next_round();
        let (options, correct) =
            if question.is_mcq() { shuffled(&question.options, &question.a) } else { (Vec::new(), 0) };
        let sent = channel
            .send_message(
                &ctx.http,
                CreateMessage::new()
                    .embed(embed(round, &question, &Shown::Open { hint: None }))
                    .components(components(round, &question, &options, correct, true, &[])),
            )
            .await;
        let message: MessageId = match sent {
            Ok(m) => m.id,
            Err(err) => {
                tracing::warn!("quiz question not sent: {}", err);
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        };
        let done = Arc::new(Notify::new());
        *LIVE.lock() = Some(Live {
            round,
            question: question.clone(),
            options: options.clone(),
            correct,
            removed: Vec::new(),
            hints: 0,
            last_hint: None,
            channel,
            message,
            below: 0,
            last_bump: std::time::Instant::now(),
            winner: None,
            tried: std::collections::HashMap::new(),
            skip_votes: HashSet::new(),
            passed: false,
            done: done.clone(),
        });

        // No time limit: the question stays until someone answers it, enough
        // people skip it, or a report removes it.
        done.notified().await;

        // Whoever got in before this take() won; nobody can after it.
        let Some(live) = LIVE.lock().take() else {
            break;
        };
        let shown = match live.winner {
            Some(winner) => Shown::Won(winner),
            None => Shown::Skipped,
        };
        let edits = EDIT_LOCK.lock().await;
        let _ = channel
            .edit_message(
                &ctx.http,
                live.message,
                EditMessage::new()
                    .embed(embed(round, &question, &shown))
                    .components(components(round, &question, &options, correct, false, &[])),
            )
            .await;
        drop(edits);
        if STOP.load(Ordering::SeqCst) {
            continue;
        }
        if matches!(shown, Shown::Skipped) {
            let _ = channel.say(&ctx.http, format!("⏭️ Skipped! The answer was **{}**.", question.a)).await;
        }
        tokio::time::sleep(GAP).await;
    }
}

/// A message in the quiz channel. Returns true when the channel is the quiz's,
/// answered or not, so nothing else in the bot reacts to quiz chatter.
pub async fn on_message(ctx: &Context, msg: &Message) -> bool {
    if channel() != Some(msg.channel_id) {
        return false;
    }
    if msg.content.trim().eq_ignore_ascii_case("!hint") {
        give_hint(ctx, msg).await;
        return true;
    }
    if msg.content.trim().eq_ignore_ascii_case("!skip") {
        vote_skip(ctx, msg).await;
        return true;
    }
    if msg.content.chars().count() > 80 {
        note_chatter(ctx, 1);
        return true;
    }
    let user = msg.author.id.get();
    let won = {
        let mut guard = LIVE.lock();
        match guard.as_mut() {
            Some(live)
                if !live.question.is_mcq()
                    && live.is_open()
                    && live.question.added_by != Some(user)
                    && live.question.accepts(&msg.content) =>
            {
                live.winner = Some(user);
                live.done.notify_one();
                Some((live.question.clone(), round_point(user)))
            }
            _ => None,
        }
    };
    if won.is_none() {
        note_chatter(ctx, 1);
    }
    if let Some((q, this_round)) = won {
        let total = add_point(user, &q.id);
        let _ = msg.react(&ctx.http, '✅').await;
        let reply = CreateMessage::new()
            .content(celebrate(user, &q, this_round, total))
            .reference_message(msg)
            .allowed_mentions(CreateAllowedMentions::new());
        let _ = msg.channel_id.send_message(&ctx.http, reply).await;
        if let Some(guild) = msg.guild_id {
            spawn_leader(ctx, guild, msg.channel_id);
        }
    }
    true
}

/// `!skip`: one admin, or `SKIPS_NEEDED` different members, pass over the open question.
async fn vote_skip(ctx: &Context, msg: &Message) {
    let user = msg.author.id.get();
    let admin = super::admin_ids().contains(&user);
    let votes = {
        let mut guard = LIVE.lock();
        match guard.as_mut() {
            Some(live) if live.is_open() => {
                live.skip_votes.insert(user);
                let count = live.skip_votes.len();
                if admin || count >= SKIPS_NEEDED {
                    live.passed = true;
                    live.done.notify_one();
                    None
                } else {
                    Some(count)
                }
            }
            _ => return,
        }
    };
    if let Some(count) = votes {
        let text = format!(
            "⏭️ Skip vote {}/{}. {} more to skip this question.",
            count,
            SKIPS_NEEDED,
            SKIPS_NEEDED - count
        );
        let reply = CreateMessage::new().content(text).reference_message(msg).allowed_mentions(CreateAllowedMentions::new());
        let _ = msg.channel_id.send_message(&ctx.http, reply).await;
        note_chatter(ctx, 2);
    }
}

/// Counts messages that pushed the open question up the channel. Once enough
/// pile up under it, the question is posted again at the bottom.
fn note_chatter(ctx: &Context, messages: u32) {
    let round = {
        let mut guard = LIVE.lock();
        match guard.as_mut() {
            Some(live) if live.is_open() => {
                live.below += messages;
                if live.below >= STICKY_AFTER && live.last_bump.elapsed() >= STICKY_GAP {
                    live.below = 0;
                    live.last_bump = std::time::Instant::now();
                    Some(live.round)
                } else {
                    None
                }
            }
            _ => None,
        }
    };
    if let Some(round) = round {
        let ctx = ctx.clone();
        tokio::spawn(async move { move_question_down(&ctx, round).await });
    }
}

/// Reposts the open question - with its hint and knocked-out options - at the
/// bottom of the channel and deletes the old copy. If the round ends while the
/// new copy is being sent, the new copy is the one deleted, so the final
/// "answered" / "time up" edit always lands on the message that stays.
async fn move_question_down(ctx: &Context, round: u64) {
    let _edits = EDIT_LOCK.lock().await;
    let snap = {
        let guard = LIVE.lock();
        match guard.as_ref() {
            Some(live) if live.round == round && live.is_open() => Some((
                live.question.clone(),
                live.options.clone(),
                live.correct,
                live.removed.clone(),
                live.hints,
                live.channel,
                live.message,
            )),
            _ => None,
        }
    };
    let Some((question, options, correct, removed, hints, channel, old)) = snap else {
        return;
    };
    let hint = (!question.is_mcq() && hints > 0).then(|| hint_at(&question.a, hints));
    let copy = CreateMessage::new()
        .embed(embed(round, &question, &Shown::Open { hint }))
        .components(components(round, &question, &options, correct, true, &removed));
    let Ok(new) = channel.send_message(&ctx.http, copy).await else {
        return;
    };
    let still_open = {
        let mut guard = LIVE.lock();
        match guard.as_mut() {
            Some(live) if live.round == round && live.is_open() => {
                live.message = new.id;
                true
            }
            _ => false,
        }
    };
    let stale = if still_open { old } else { new.id };
    let _ = channel.delete_message(&ctx.http, stale).await;
}

/// `!hint` in the quiz channel: one more letter of each word on a typed
/// question, one wrong option knocked out on multiple choice.
async fn give_hint(ctx: &Context, msg: &Message) {
    enum Hint {
        Idle,
        Wait,
        Enough,
        Letters(String),
        Knocked(String),
    }
    struct Snapshot {
        round: u64,
        question: Question,
        options: Vec<String>,
        correct: usize,
        removed: Vec<usize>,
        channel: ChannelId,
        message: MessageId,
    }
    let _edits = EDIT_LOCK.lock().await;
    let (hint, snap) = {
        let mut guard = LIVE.lock();
        match guard.as_mut() {
            Some(live) if live.is_open() => {
                let hint = if live.last_hint.is_some_and(|t| t.elapsed() < HINT_COOLDOWN) {
                    Hint::Wait
                } else if live.question.is_mcq() {
                    let wrong: Vec<usize> =
                        (0..live.options.len()).filter(|i| *i != live.correct && !live.removed.contains(i)).collect();
                    if live.removed.len() >= MAX_KNOCKOUTS || wrong.is_empty() {
                        Hint::Enough
                    } else {
                        let out = wrong[rand::random::<u32>() as usize % wrong.len()];
                        live.removed.push(out);
                        Hint::Knocked(live.options[out].clone())
                    }
                } else if live.hints > 0 && hint_at(&live.question.a, live.hints + 1) == hint_at(&live.question.a, live.hints) {
                    Hint::Enough
                } else {
                    live.hints += 1;
                    Hint::Letters(hint_at(&live.question.a, live.hints))
                };
                if matches!(hint, Hint::Letters(_) | Hint::Knocked(_)) {
                    live.last_hint = Some(std::time::Instant::now());
                }
                let snap = Snapshot {
                    round: live.round,
                    question: live.question.clone(),
                    options: live.options.clone(),
                    correct: live.correct,
                    removed: live.removed.clone(),
                    channel: live.channel,
                    message: live.message,
                };
                (hint, Some(snap))
            }
            _ => (Hint::Idle, None),
        }
    };
    let reply = |text: String| {
        CreateMessage::new().content(text).reference_message(msg).allowed_mentions(CreateAllowedMentions::new())
    };
    match (hint, snap) {
        (Hint::Letters(shown), Some(s)) => {
            let open = Shown::Open { hint: Some(shown.clone()) };
            let _ = s.channel.edit_message(&ctx.http, s.message, EditMessage::new().embed(embed(s.round, &s.question, &open))).await;
            let _ = msg.channel_id.send_message(&ctx.http, reply(format!("💡 `{}`", shown))).await;
        }
        (Hint::Knocked(option), Some(s)) => {
            let buttons = components(s.round, &s.question, &s.options, s.correct, true, &s.removed);
            let _ = s.channel.edit_message(&ctx.http, s.message, EditMessage::new().components(buttons)).await;
            let _ = msg.channel_id.send_message(&ctx.http, reply(format!("💡 **{}** is wrong, so it's gone.", option))).await;
        }
        (Hint::Wait, _) => {
            let _ = msg.react(&ctx.http, '⏳').await;
        }
        (Hint::Enough, _) => {
            let _ = msg.channel_id.send_message(&ctx.http, reply("That's all the hints you get. Over to you 😏".into())).await;
        }
        _ => {
            let text = if VOTE.lock().is_some() {
                "Voting is on. Pick the next genre above, then ask for a hint."
            } else if RUNNING.load(Ordering::SeqCst) {
                "Wait for the next question, then ask for a hint."
            } else {
                "No question is running. Start one with `/quiz`."
            };
            let _ = msg.channel_id.send_message(&ctx.http, reply(text.into())).await;
        }
    }
    // The "!hint" and the bot's reply both push the question up.
    note_chatter(ctx, 2);
}

// --- buttons and menus --------------------------------------------------------

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let reply = CreateInteractionResponseMessage::new().content(text).ephemeral(true);
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    if id == "quizlb" {
        let view = match &component.data.kind {
            ComponentInteractionDataKind::StringSelect { values } => values.first().cloned().unwrap_or_default(),
            _ => String::new(),
        };
        let embed = match view.as_str() {
            "genres" => champions(),
            "week" => board(true),
            _ => board(false),
        };
        let update = CreateInteractionResponseMessage::new().embed(embed).components(vec![board_menu(&view)]);
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
    } else if let Some(batch) = id.strip_prefix("quiznewsrej:") {
        news_select(ctx, component, batch).await;
    } else if let Some(batch) = id.strip_prefix("quiznewsok:") {
        news_finish(ctx, component, batch, true).await;
    } else if let Some(batch) = id.strip_prefix("quiznewsno:") {
        news_finish(ctx, component, batch, false).await;
    } else if let Some(qid) = id.strip_prefix("quizflag:") {
        flag(ctx, component, qid).await;
    } else if let Some(qid) = id.strip_prefix("quizretire:") {
        report_decision(ctx, component, qid, true).await;
    } else if let Some(qid) = id.strip_prefix("quizkeep:") {
        report_decision(ctx, component, qid, false).await;
    } else if let Some(sid) = id.strip_prefix("quizok:") {
        review(ctx, component, sid, true).await;
    } else if let Some(sid) = id.strip_prefix("quizno:") {
        review(ctx, component, sid, false).await;
    } else if let Some(rest) = id.strip_prefix("quizvote:") {
        cast_vote(ctx, component, rest).await;
    } else if let Some(rest) = id.strip_prefix("quiz:") {
        choose(ctx, component, rest).await;
    }
}

async fn choose(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let mut parts = rest.split(':');
    let (Some(Ok(round)), Some(Ok(choice))) =
        (parts.next().map(str::parse::<u64>), parts.next().map(str::parse::<usize>))
    else {
        return;
    };
    let user = component.user.id.get();
    enum Click {
        Over,
        Gone,
        Own,
        Ruled,
        Wait(u64),
        Wrong,
        Right(Question, Vec<String>, usize, u32),
    }
    let click = {
        let mut guard = LIVE.lock();
        match guard.as_mut() {
            Some(live) if live.round == round && live.is_open() => {
                let mine = live.tried.get(&user);
                let wait = mine
                    .and_then(|(at, _)| RETRY_AFTER.checked_sub(at.elapsed()))
                    .filter(|left| !left.is_zero());
                if live.removed.contains(&choice) {
                    Click::Gone
                } else if live.question.added_by == Some(user) {
                    Click::Own
                } else if mine.is_some_and(|(_, ruled)| ruled.contains(&choice)) {
                    Click::Ruled
                } else if let Some(left) = wait {
                    Click::Wait(left.as_secs().max(1))
                } else if choice == live.correct {
                    live.winner = Some(user);
                    live.done.notify_one();
                    Click::Right(live.question.clone(), live.options.clone(), live.correct, round_point(user))
                } else {
                    // A wrong pick costs a minute, and that option stays ruled out for them.
                    let entry = live.tried.entry(user).or_insert_with(|| (std::time::Instant::now(), HashSet::new()));
                    entry.0 = std::time::Instant::now();
                    entry.1.insert(choice);
                    Click::Wrong
                }
            }
            _ => Click::Over,
        }
    };
    match click {
        Click::Over => whisper(ctx, component, "This question is already over.").await,
        Click::Gone => whisper(ctx, component, "That option was removed by a hint.").await,
        Click::Own => whisper(ctx, component, "You can't answer your own question 😏").await,
        Click::Ruled => whisper(ctx, component, "You already tried that one. Pick a different option.").await,
        Click::Wait(left) => whisper(ctx, component, format!("⏳ You can pick again in {}s.", left)).await,
        Click::Wrong => whisper(ctx, component, "❌ Wrong! You can pick again in 1 minute.").await,
        Click::Right(q, options, correct, this_round) => {
            let total = add_point(user, &q.id);
            let update = CreateInteractionResponseMessage::new()
                .embed(embed(round, &q, &Shown::Won(user)))
                .components(components(round, &q, &options, correct, false, &[]));
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
            let note = CreateMessage::new()
                .content(celebrate(user, &q, this_round, total))
                .allowed_mentions(CreateAllowedMentions::new());
            let _ = component.channel_id.send_message(&ctx.http, note).await;
            if let Some(guild) = component.guild_id {
                spawn_leader(ctx, guild, component.channel_id);
            }
        }
    }
}

/// 🚩 on a question: recorded, and never skips or removes it by itself - a
/// report must not be a free skip. The first report of each question goes to
/// the reviewers by DM, to keep it or remove it for the future.
async fn flag(ctx: &Context, component: &ComponentInteraction, qid: &str) {
    let user = component.user.id.get();
    let Some(db) = DB.get() else {
        return;
    };
    let (count, first, body) = {
        let conn = db.lock();
        let added = conn
            .execute("INSERT OR IGNORE INTO flags (question_id, user_id) VALUES (?1, ?2)", params![qid, user as i64])
            .unwrap_or(0)
            > 0;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM flags WHERE question_id = ?1", params![qid], |r| r.get(0))
            .unwrap_or(0);
        let body: Option<String> = conn
            .query_row("SELECT body FROM questions WHERE id = ?1", params![qid], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        (count, added && count == 1, body)
    };
    tracing::info!("quiz: question {} reported by {} ({} reports so far)", qid, user, count);
    whisper(ctx, component, "🚩 Thanks, reported. An admin will check this question.").await;

    let (true, Some(q)) = (first, body.and_then(|b| serde_json::from_str::<Question>(&b).ok())) else {
        return;
    };
    let mut text = format!("**Question:** {}\n**Answer:** {}", q.q, q.a);
    if !q.alt.is_empty() {
        text.push_str(&format!("\n**Also accepted:** {}", q.alt.join(", ")));
    }
    if q.is_mcq() {
        let wrong: Vec<&str> = q.options.iter().filter(|o| **o != q.a).map(|o| o.as_str()).collect();
        text.push_str(&format!("\n**Wrong options:** {}", wrong.join(" / ")));
    }
    text.push_str(&format!("\n\nReported by <@{}> · {} · `{}`", user, pretty(&q.cat), q.id));
    let card = CreateMessage::new()
        .embed(CreateEmbed::new().title("🚩 Quiz question reported").description(text).colour(0xED4245))
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("quizretire:{}", q.id)).label("🗑️ Remove question").style(ButtonStyle::Danger),
            CreateButton::new(format!("quizkeep:{}", q.id)).label("✅ Keep").style(ButtonStyle::Secondary),
        ])]);
    for reviewer in reviewers() {
        if let Err(err) = UserId::new(reviewer).direct_message(&ctx.http, card.clone()).await {
            tracing::warn!("quiz report for {} not sent to {}: {}", q.id, reviewer, err);
        }
    }
}

/// A reviewer's call on a reported question. Removing only stops it coming up
/// again; if it is on screen right now it stays there - an admin can `!skip`.
async fn report_decision(ctx: &Context, component: &ComponentInteraction, qid: &str, remove: bool) {
    let reviewer = component.user.id.get();
    if !reviewers().contains(&reviewer) {
        whisper(ctx, component, "Only the quiz reviewer can do this.").await;
        return;
    }
    if remove {
        if let Some(db) = DB.get() {
            let _ = db.lock().execute("UPDATE questions SET retired = 1 WHERE id = ?1", params![qid]);
        }
        tracing::info!("quiz: question {} removed after review by {}", qid, reviewer);
    }
    let verdict = if remove { "🗑️ Removed. It won't come up again." } else { "✅ Kept." };
    let update = CreateInteractionResponseMessage::new().content(format!("{} · <@{}>", verdict, reviewer)).components(vec![]);
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
}

// --- leaderboard and leader role ------------------------------------------------

/// Monday 00:00 in India, this week.
fn week_start() -> i64 {
    let ist = super::stats::ist();
    let now = Utc::now().with_timezone(&ist);
    let monday = now.date_naive() - chrono::Duration::days(now.weekday().num_days_from_monday() as i64);
    monday
        .and_hms_opt(0, 0, 0)
        .and_then(|t| ist.from_local_datetime(&t).single())
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

fn board(week: bool) -> CreateEmbed {
    let since = if week { week_start() } else { 0 };
    let (rows, leader, answered) = DB
        .get()
        .map(|db| {
            let conn = db.lock();
            let rows: Vec<(u64, i64)> = conn
                .prepare(
                    "SELECT user_id, COUNT(*) AS n FROM points WHERE ts >= ?1
                     GROUP BY user_id ORDER BY n DESC, MAX(id) ASC LIMIT 10",
                )
                .and_then(|mut s| {
                    s.query_map(params![since], |r| Ok((r.get::<_, i64>(0)? as u64, r.get(1)?)))?.collect()
                })
                .unwrap_or_default();
            let answered: i64 = conn
                .query_row("SELECT COUNT(*) FROM points WHERE ts >= ?1", params![since], |r| r.get(0))
                .unwrap_or(0);
            (rows, meta_get(&conn, "leader").and_then(|v| v.parse::<u64>().ok()), answered)
        })
        .unwrap_or_default();
    let mut text = String::from(if week { "**This week** (since Monday)\n\n" } else { "**All time**\n\n" });
    if rows.is_empty() {
        text.push_str("No points yet. Start a quiz with `/quiz` in the quiz channel.");
    }
    for (i, (user, points)) in rows.iter().enumerate() {
        let place = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            _ => format!("`{:>2}.`", i + 1),
        };
        let crown = if leader == Some(*user) { " 👑" } else { "" };
        text.push_str(&format!("{} <@{}>{} · **{}**\n", place, user, crown, points));
    }
    if answered > 0 {
        text.push_str(&format!("\n-# {} questions answered correctly", answered));
    }
    CreateEmbed::new()
        .title("🏆 Quiz Leaderboard")
        .description(text)
        .colour(0xF1C40F)
        .footer(CreateEmbedFooter::new(CREDITS))
}

/// The genre champions board: who wears each genre's crown.
fn champions() -> CreateEmbed {
    let (lines, rounds) = DB
        .get()
        .map(|db| {
            let conn = db.lock();
            let rounds: i64 = conn.query_row("SELECT COUNT(*) FROM rounds", [], |r| r.get(0)).unwrap_or(0);
            (champion_lines(&conn), rounds)
        })
        .unwrap_or_default();
    let mut text =
        String::from("**Genre champions**\n-# 👑 won the latest round of that genre · 🏆 has won it most often\n\n");
    if lines.is_empty() {
        text.push_str(&format!(
            "No rounds won yet. After every {} questions, the round's top scorer is crowned that genre's champion.",
            BLOCK
        ));
    } else {
        text.push_str(&lines.join("\n"));
        text.push_str(&format!("\n\n-# {} round{} played", rounds, if rounds == 1 { "" } else { "s" }));
    }
    CreateEmbed::new()
        .title("🏆 Quiz Leaderboard")
        .description(text)
        .colour(0xF1C40F)
        .footer(CreateEmbedFooter::new(CREDITS))
}

fn board_menu(view: &str) -> CreateActionRow {
    let options = vec![
        CreateSelectMenuOption::new("All time", "all").default_selection(!matches!(view, "week" | "genres")),
        CreateSelectMenuOption::new("This week", "week").default_selection(view == "week"),
        CreateSelectMenuOption::new("Genre champions", "genres").default_selection(view == "genres"),
    ];
    CreateActionRow::SelectMenu(CreateSelectMenu::new("quizlb", CreateSelectMenuKind::String { options }))
}

pub async fn leaderboard_command(ctx: &Context, command: &CommandInteraction) {
    let reply = CreateInteractionResponseMessage::new().embed(board(false)).components(vec![board_menu("all")]);
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

fn spawn_leader(ctx: &Context, guild: GuildId, channel: ChannelId) {
    let ctx = ctx.clone();
    tokio::spawn(async move { update_leader(&ctx, guild, channel).await });
}

/// Moves the Quiz Leader role to the all-time top scorer. On a tie, whoever
/// reached the score first keeps it.
async fn update_leader(ctx: &Context, guild: GuildId, channel: ChannelId) {
    let _one_at_a_time = LEADER_LOCK.lock().await;
    let Some(db) = DB.get() else {
        return;
    };
    let (top, previous, stored_role) = {
        let conn = db.lock();
        let top: Option<(i64, i64)> = conn
            .query_row(
                "SELECT user_id, COUNT(*) AS n FROM points GROUP BY user_id ORDER BY n DESC, MAX(id) ASC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .ok()
            .flatten();
        (
            top,
            meta_get(&conn, "leader").and_then(|v| v.parse::<u64>().ok()),
            meta_get(&conn, "leader_role").and_then(|v| v.parse::<u64>().ok()),
        )
    };
    let Some((top, points)) = top else {
        return;
    };
    let top = top as u64;
    if previous == Some(top) {
        return;
    }
    let role = match leader_role(ctx, guild, stored_role).await {
        Ok(role) => role,
        Err(err) => {
            tracing::warn!("quiz leader role unavailable: {}", err);
            return;
        }
    };
    if let Err(err) = ctx.http.add_member_role(guild, UserId::new(top), role, Some("Top quiz scorer")).await {
        tracing::warn!("quiz leader role not given to {}: {}", top, err);
        return;
    }
    if let Some(previous) = previous {
        let _ = ctx
            .http
            .remove_member_role(guild, UserId::new(previous), role, Some("No longer the top quiz scorer"))
            .await;
    }
    meta_set(&db.lock(), "leader", &top.to_string());
    // Early on the lead changes hands every question; only announce once it means something.
    if points >= 3 {
        let text = match previous {
            Some(previous) => format!("👑 **New {}:** <@{}> has overtaken <@{}>!", ROLE_NAME, top, previous),
            None => format!("👑 <@{}> is the first **{}**!", top, ROLE_NAME),
        };
        let note = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
        let _ = channel.send_message(&ctx.http, note).await;
    }
}

async fn leader_role(ctx: &Context, guild: GuildId, stored: Option<u64>) -> Result<RoleId, String> {
    let roles = guild.roles(&ctx.http).await.map_err(|e| e.to_string())?;
    if let Some(id) = stored.map(RoleId::new) {
        if roles.contains_key(&id) {
            return Ok(id);
        }
    }
    let id = match roles.values().find(|r| r.name == ROLE_NAME) {
        Some(role) => role.id,
        None => {
            let builder = EditRole::new().name(ROLE_NAME).colour(0xF1C40F).hoist(false).mentionable(false);
            guild.create_role(&ctx.http, builder).await.map_err(|e| e.to_string())?.id
        }
    };
    if let Some(db) = DB.get() {
        meta_set(&db.lock(), "leader_role", &id.get().to_string());
    }
    Ok(id)
}

// --- member questions -----------------------------------------------------------

pub async fn add_command(ctx: &Context, command: &CommandInteraction) {
    let get = |name: &str| {
        command
            .data
            .options
            .iter()
            .find(|o| o.name == name)
            .and_then(|o| o.value.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let respond = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let (Some(q), Some(a)) = (get("question"), get("answer")) else {
        let _ = command.create_response(&ctx.http, respond("Both a question and an answer are needed.".into())).await;
        return;
    };
    let wrong: Vec<String> = ["wrong1", "wrong2", "wrong3"].iter().filter_map(|n| get(n)).collect();
    let mut options = Vec::new();
    if !wrong.is_empty() {
        options = std::iter::once(a.clone()).chain(wrong.iter().cloned()).collect();
        let unique: HashSet<String> = options.iter().map(|o| o.to_lowercase()).collect();
        if wrong.len() != 3 || unique.len() != 4 {
            let text = "For multiple choice, give three different wrong options (wrong1, wrong2, wrong3). For a typed question, give none.";
            let _ = command.create_response(&ctx.http, respond(text.into())).await;
            return;
        }
    }
    let alt: Vec<String> = match (&options.is_empty(), get("also")) {
        (true, Some(also)) => also.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        _ => Vec::new(),
    };
    let question = Question {
        id: String::new(),
        kind: if options.is_empty() { "text" } else { "mcq" }.into(),
        q,
        a,
        alt,
        options,
        cat: "server".into(),
        region: "india".into(),
        diff: String::new(),
        note: String::new(),
        src: "member".into(),
        added_by: None,
        theme: String::new(),
        theme_region: String::new(),
    };
    let user = command.user.id.get();
    let Some(db) = DB.get() else {
        return;
    };
    let saved = {
        let conn = db.lock();
        let pending: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM submissions WHERE user_id = ?1 AND status = 'pending'",
                params![user as i64],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if pending >= MAX_PENDING {
            None
        } else {
            let body = serde_json::to_string(&question).unwrap_or_default();
            conn.execute(
                "INSERT INTO submissions (user_id, body, ts) VALUES (?1, ?2, ?3)",
                params![user as i64, body, Utc::now().timestamp()],
            )
            .ok()
            .map(|_| conn.last_insert_rowid())
        }
    };
    let Some(sid) = saved else {
        let text = format!("You already have {} questions waiting for approval. Try again once they're reviewed.", MAX_PENDING);
        let _ = command.create_response(&ctx.http, respond(text)).await;
        return;
    };
    let _ = command
        .create_response(
            &ctx.http,
            respond("📨 Sent! It joins the quiz once it's approved. Don't tell anyone the answer 🤫".into()),
        )
        .await;

    let mut text = format!("**Question:** {}\n**Answer:** {}", question.q, question.a);
    if !question.alt.is_empty() {
        text.push_str(&format!("\n**Also accepted:** {}", question.alt.join(", ")));
    }
    if question.is_mcq() {
        text.push_str(&format!("\n**Wrong options:** {}", question.options[1..].join(", ")));
    }
    text.push_str(&format!("\n\nBheja: <@{}>", user));
    let card = CreateMessage::new()
        .embed(CreateEmbed::new().title(format!("New quiz question #{}", sid)).description(text).colour(0x5865F2))
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("quizok:{}", sid)).label("✅ Approve").style(ButtonStyle::Success),
            CreateButton::new(format!("quizno:{}", sid)).label("❌ Reject").style(ButtonStyle::Danger),
        ])]);
    for admin in reviewers() {
        if let Err(err) = UserId::new(admin).direct_message(&ctx.http, card.clone()).await {
            tracing::warn!("quiz submission {} not sent to admin {}: {}", sid, admin, err);
        }
    }
}

async fn review(ctx: &Context, component: &ComponentInteraction, sid: &str, approve: bool) {
    let admin = component.user.id.get();
    if !reviewers().contains(&admin) {
        whisper(ctx, component, "Only the quiz reviewer can do this.").await;
        return;
    }
    let (Ok(sid), Some(db)) = (sid.parse::<i64>(), DB.get()) else {
        return;
    };
    let outcome: Result<u64, String> = {
        let conn = db.lock();
        let row: Option<(i64, String, String)> = conn
            .query_row("SELECT user_id, body, status FROM submissions WHERE id = ?1", params![sid], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()
            .ok()
            .flatten();
        match row {
            None => Err("That submission wasn't found.".into()),
            Some((_, _, status)) if status != "pending" => Err(format!("This was already {}.", status)),
            Some((_, body, _)) if approve && serde_json::from_str::<Question>(&body).is_err() => {
                Err("This submission's data is broken.".into())
            }
            Some((author, body, _)) => {
                if let (true, Ok(mut q)) = (approve, serde_json::from_str::<Question>(&body)) {
                    q.id = format!("member-{}", sid);
                    q.src = "member".into();
                    let _ = conn.execute(
                        "INSERT OR IGNORE INTO questions (id, body, kind, region, cat, src, added_by, added_ts, theme, theme_region)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'member', ?6, ?7, 'server', 'india')",
                        params![
                            q.id,
                            serde_json::to_string(&q).unwrap_or_default(),
                            q.kind,
                            q.region,
                            q.cat,
                            author,
                            Utc::now().timestamp()
                        ],
                    );
                }
                let status = if approve { "approved" } else { "rejected" };
                let _ = conn.execute("UPDATE submissions SET status = ?1 WHERE id = ?2", params![status, sid]);
                Ok(author as u64)
            }
        }
    };
    match outcome {
        Err(text) => {
            let update = CreateInteractionResponseMessage::new().content(text).components(vec![]);
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
        }
        Ok(author) => {
            let verdict = if approve { "✅ Approved" } else { "❌ Rejected" };
            let update = CreateInteractionResponseMessage::new()
                .content(format!("{} by <@{}>", verdict, admin))
                .components(vec![]);
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
            if approve {
                let note = CreateMessage::new().content("🎉 Your quiz question was approved! It's now in the quiz.");
                let _ = UserId::new(author).direct_message(&ctx.http, note).await;
            }
        }
    }
}

// --- weekly news questions ---------------------------------------------------------
//
// Once a week the bot reads entertainment headlines, asks the model for
// questions built only from what those headlines say, and DMs them to the
// admins as one batch. Nothing reaches the quiz until an admin approves it,
// and news questions retire after `NEWS_SHELF_LIFE`.

const NEWS_FEEDS: &[&str] = &[
    "https://www.hindustantimes.com/feeds/rss/entertainment/bollywood/rssfeed.xml",
    "https://feeds.feedburner.com/ndtvmovies-latest",
    "https://www.bollywoodhungama.com/feed/",
    "https://www.koimoi.com/feed/",
    "https://www.indiatoday.in/rss/1206533",
    "https://timesofindia.indiatimes.com/rssfeeds/1081479906.cms",
];
const NEWS_MAX_ITEMS: usize = 90;
const NEWS_MAX_QUESTIONS: usize = 15;
/// Monday, this many hours after midnight in India.
const NEWS_HOUR: i64 = 11;

/// Gossip, tragedy, courts and politics: the news a quiz must not turn into points.
static SENSITIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(reported(ly)?|rumou?rs?|buzz|speculat\w*|sources|insiders?|spotted|troll\w*|slams?|lash(es|ed)?|arrest\w*|police|fir|court|lawsuit|legal|case|death|dead|dies|died|passe[sd] away|funeral|hospital\w*|health|ill|illness|accident|injur\w*|pregnan\w*|baby|babies|daughter|son|divorce\w*|split|break-?up|dating|affair|link-?up|controvers\w*|boycott\w*|feud|politic\w*|minister|mp|mla|bjp|congress|election|pakistan\w*|religio\w*|leak\w*|net worth|fees?|salary|body|weight)\b",
    )
    .expect("valid regex")
});

struct NewsItem {
    title: String,
    summary: String,
    link: String,
    published: chrono::DateTime<Utc>,
}

#[derive(Deserialize)]
struct Drafted {
    kind: String,
    q: String,
    a: String,
    #[serde(default)]
    alt: Vec<String>,
    #[serde(default)]
    options: Vec<String>,
    #[serde(default)]
    note: String,
    #[serde(default)]
    item: usize,
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    }
}

fn unescape(text: &str) -> String {
    static ENTITY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"&(#[xX][0-9a-fA-F]+|#[0-9]+|amp|lt|gt|quot|apos|nbsp|rsquo|lsquo|rdquo|ldquo|hellip|ndash|mdash);")
            .expect("valid regex")
    });
    ENTITY
        .replace_all(text, |c: &regex::Captures| -> String {
            match &c[1] {
                "amp" => "&".into(),
                "lt" => "<".into(),
                "gt" => ">".into(),
                "quot" | "rdquo" | "ldquo" => "\"".into(),
                "apos" | "rsquo" | "lsquo" => "'".into(),
                "nbsp" => " ".into(),
                "hellip" => "…".into(),
                "ndash" => "–".into(),
                "mdash" => "—".into(),
                other => {
                    let code = match other.strip_prefix("#x").or_else(|| other.strip_prefix("#X")) {
                        Some(hex) => u32::from_str_radix(hex, 16).ok(),
                        None => other.strip_prefix('#').and_then(|d| d.parse().ok()),
                    };
                    code.and_then(char::from_u32).map(String::from).unwrap_or_default()
                }
            }
        })
        .into_owned()
}

/// Text of an RSS field: CDATA unwrapped, entities decoded, markup dropped.
fn clean_text(raw: &str) -> String {
    static MARKUP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").expect("valid regex"));
    let raw = raw.replace("<![CDATA[", "").replace("]]>", "");
    let text = unescape(&MARKUP.replace_all(&unescape(&raw), " "));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn field(block: &str, name: &str) -> String {
    Regex::new(&format!(r"(?s)<{0}(?:\s[^>]*)?>(.*?)</{0}>", regex::escape(name)))
        .ok()
        .and_then(|re| re.captures(block).map(|c| clean_text(&c[1])))
        .unwrap_or_default()
}

fn parse_feed(xml: &str) -> Vec<NewsItem> {
    static ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<item[\s>].*?</item>").expect("valid regex"));
    ITEM.find_iter(xml)
        .filter_map(|m| {
            let block = m.as_str();
            let date = [field(block, "pubDate"), field(block, "dc:date")].into_iter().find(|d| !d.is_empty())?;
            let published = chrono::DateTime::parse_from_rfc2822(&date)
                .or_else(|_| chrono::DateTime::parse_from_rfc3339(&date))
                .ok()?
                .with_timezone(&Utc);
            let title = field(block, "title");
            (!title.is_empty()).then(|| NewsItem {
                title,
                summary: field(block, "description"),
                link: field(block, "link"),
                published,
            })
        })
        .collect()
}

/// The last week's entertainment headlines, newest first, with the sensitive ones left out.
async fn fetch_news() -> Vec<NewsItem> {
    let feeds: Vec<String> = std::env::var("VIZIER_QUIZ_NEWS_FEEDS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .filter(|v: &Vec<String>| !v.is_empty())
        .unwrap_or_else(|| NEWS_FEEDS.iter().map(|s| s.to_string()).collect());
    let Ok(client) = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (compatible; MLCI-quiz/1.0)")
        .timeout(Duration::from_secs(20))
        .build()
    else {
        return Vec::new();
    };
    let since = Utc::now() - chrono::Duration::days(7);
    let (mut items, mut seen) = (Vec::new(), HashSet::new());
    for feed in feeds {
        let body = match client.get(&feed).send().await {
            Ok(response) => response.text().await.unwrap_or_default(),
            Err(err) => {
                tracing::warn!("quiz news: {} unreadable: {}", feed, err);
                continue;
            }
        };
        for item in parse_feed(&body) {
            if item.published >= since && !SENSITIVE.is_match(&item.title) && seen.insert(norm(&item.title)) {
                items.push(item);
            }
        }
    }
    items.sort_by(|a, b| b.published.cmp(&a.published));
    items.truncate(NEWS_MAX_ITEMS);
    items
}

fn news_prompt(items: &[NewsItem]) -> String {
    let ist = super::stats::ist();
    let mut list = String::new();
    for (i, item) in items.iter().enumerate() {
        list.push_str(&format!(
            "{}. [{}] {} — {}\n",
            i + 1,
            item.published.with_timezone(&ist).format("%-d %b %Y"),
            item.title,
            clip(&item.summary, 300)
        ));
    }
    format!(
        r#"You write quiz questions for an Indian Discord server's Bollywood quiz, using this week's entertainment news.

Today is {today}. Below are {count} news items (date, headline, summary) from Indian entertainment sites.

Write up to {max} quiz questions. Rules:
- Use ONLY facts stated plainly in the items below. Never add facts from memory and never guess.
- Only confirmed, on-record news: film and series releases, trailers and teasers, titles and release dates announced by the makers, casting announced by the makers, box office milestones as stated, awards and nominations, songs released, new shows and their hosts, weddings or engagements announced by the couple themselves.
- Skip anything that is a rumour or "reportedly"/"sources said"/"buzz"; reviews and opinions; trolling and social media reactions; pregnancies, children, health, deaths, accidents; legal cases, police, controversies, feuds, boycotts; politics, politicians, religion, caste; India-Pakistan matters; looks, bodies, fees, money earned; anything sexual.
- Hindi cinema first. South Indian or Hollywood news only when it is big news in India.
- Every question names the month and year so it stays true later, e.g. "In {month}, which actor was announced as the lead of ...?"
- Exactly one correct answer, and the answer must appear word for word in the news item.
- kind "text": the answer is a name or title of at most 4 words; "alt" lists other fair ways to write it (a well-known short form, first name if unambiguous). kind "mcq": "options" has 4 entries, one exactly equal to the answer, three plausible wrong ones of the same type (real actors, real films). Roughly half each.
- "note": one short line of context from the item, at most 20 words.
- "item": the number of the news item the question comes from. At most one question per item.

Reply with ONLY a JSON array and no other text, like:
[{{"kind":"text","q":"...","a":"...","alt":["..."],"options":[],"note":"...","item":3}}]

News items:
{list}"#,
        today = Utc::now().with_timezone(&ist).format("%-d %B %Y"),
        month = Utc::now().with_timezone(&ist).format("%B %Y"),
        count = items.len(),
        max = NEWS_MAX_QUESTIONS,
        list = list,
    )
}

async fn ask_model(deps: &VizierDependencies, agent_id: &str, prompt: String) -> anyhow::Result<String> {
    use crate::agents::agent::model::{VizierModel, VizierModelTrait};
    use crate::storage::agent::AgentStorage;
    use rig_core::message::{AssistantContent, Message as ModelMessage};

    let config = deps.storage.get_agent(agent_id).await?.ok_or_else(|| anyhow::anyhow!("no config for {}", agent_id))?;
    let model = VizierModel::new_with_override(deps, &config, None).await?;
    let (_, choice, _) = model.completion(ModelMessage::user(prompt), vec![], vec![]).await?;
    Ok(choice
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(""))
}

fn parse_drafts(reply: &str) -> Vec<Drafted> {
    let (Some(start), Some(end)) = (reply.find('['), reply.rfind(']')) else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    serde_json::from_str::<Vec<serde_json::Value>>(&reply[start..=end])
        .map(|values| values.into_iter().filter_map(|v| serde_json::from_value(v).ok()).collect())
        .unwrap_or_default()
}

/// Turns model drafts into questions, keeping only the ones whose answer is
/// actually in the news item they cite and that pass every content check.
fn vet_drafts(drafts: Vec<Drafted>, items: &[NewsItem]) -> Vec<(Question, String)> {
    let mut out: Vec<(Question, String)> = Vec::new();
    let mut used = HashSet::new();
    for d in drafts {
        let Some(item) = d.item.checked_sub(1).and_then(|i| items.get(i)) else {
            continue;
        };
        let mcq = d.kind == "mcq";
        let q = Question {
            id: "news-draft".into(),
            kind: if mcq { "mcq" } else { "text" }.into(),
            q: d.q.trim().to_string(),
            a: d.a.trim().to_string(),
            alt: if mcq { Vec::new() } else { d.alt },
            options: if mcq { d.options } else { Vec::new() },
            cat: "bollywood_news".into(),
            region: "india".into(),
            diff: "medium".into(),
            note: clip(d.note.trim(), 160),
            src: "news".into(),
            added_by: None,
            theme: String::new(),
            theme_region: String::new(),
        };
        let source = norm(&format!("{} {}", item.title, item.summary));
        let grounded = !norm(&q.a).is_empty() && source.contains(&norm(&q.a));
        let short = mcq || q.a.split_whitespace().count() <= 4;
        let clean = !SENSITIVE.is_match(&format!("{} {} {}", q.q, q.a, q.note));
        if q.is_valid() && grounded && short && clean && q.q.len() <= 300 && used.insert(d.item) {
            out.push((q, item.link.clone()));
        }
        if out.len() == NEWS_MAX_QUESTIONS {
            break;
        }
    }
    out
}

/// Reads the week's news, drafts questions and DMs them to the admins for approval.
pub async fn news_round(ctx: &Context, deps: &VizierDependencies, agent_id: &str) -> Result<usize, String> {
    let items = fetch_news().await;
    if items.len() < 5 {
        return Err(format!("only {} usable news items this week", items.len()));
    }
    let reply = ask_model(deps, agent_id, news_prompt(&items)).await.map_err(|e| e.to_string())?;
    let vetted = vet_drafts(parse_drafts(&reply), &items);
    if vetted.is_empty() {
        return Err("none of the model's questions passed the checks".into());
    }
    let Some(db) = DB.get() else {
        return Err("the quiz database is unavailable".into());
    };
    let batch = format!("news-{}", Utc::now().format("%Y%m%d%H%M%S"));
    let saved: Vec<(i64, Question, String)> = {
        let conn = db.lock();
        vetted
            .into_iter()
            .filter_map(|(q, link)| {
                conn.execute(
                    "INSERT INTO submissions (user_id, body, ts, batch, link) VALUES (0, ?1, ?2, ?3, ?4)",
                    params![serde_json::to_string(&q).ok()?, Utc::now().timestamp(), batch, link],
                )
                .ok()?;
                Some((conn.last_insert_rowid(), q, link))
            })
            .collect()
    };
    send_news_batch(ctx, &batch, &saved).await;
    tracing::info!("quiz news: batch {} with {} questions sent for approval", batch, saved.len());
    Ok(saved.len())
}

async fn send_news_batch(ctx: &Context, batch: &str, saved: &[(i64, Question, String)]) {
    let cards: Vec<CreateEmbed> = saved
        .iter()
        .enumerate()
        .map(|(i, (_, q, link))| {
            let mut text = format!("**Answer:** {}", q.a);
            if !q.alt.is_empty() {
                text.push_str(&format!("\n**Also accepted:** {}", q.alt.join(", ")));
            }
            if q.is_mcq() {
                let wrong: Vec<&String> = q.options.iter().filter(|o| **o != q.a).collect();
                text.push_str(&format!("\n**Wrong options:** {}", wrong.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / ")));
            }
            if !q.note.is_empty() {
                text.push_str(&format!("\n_{}_", q.note));
            }
            if link.starts_with("http") {
                text.push_str(&format!("\n[Read the story]({})", link));
            }
            CreateEmbed::new().title(clip(&format!("{}. {}", i + 1, q.q), 256)).description(text).colour(0xE67E22)
        })
        .collect();
    let options: Vec<CreateSelectMenuOption> = saved
        .iter()
        .enumerate()
        .map(|(i, (sid, q, _))| CreateSelectMenuOption::new(clip(&format!("{}. {}", i + 1, q.q), 100), sid.to_string()))
        .collect();
    let controls = vec![
        CreateActionRow::SelectMenu(
            CreateSelectMenu::new(format!("quiznewsrej:{}", batch), CreateSelectMenuKind::String { options })
                .placeholder("❌ Pick any questions to reject")
                .min_values(0)
                .max_values(saved.len() as u8),
        ),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("quiznewsok:{}", batch)).label("✅ Approve the rest").style(ButtonStyle::Success),
            CreateButton::new(format!("quiznewsno:{}", batch)).label("🗑️ Reject all").style(ButtonStyle::Danger),
        ]),
    ];
    let intro = format!(
        "📰 **This week's Bollywood news questions** · {} made\nPick any that are wrong or weak in the menu below, then press **Approve the rest**. \
         Approved questions stay in the quiz for 90 days.",
        saved.len()
    );
    let chunks: Vec<&[CreateEmbed]> = cards.chunks(5).collect();
    for admin in reviewers() {
        for (i, chunk) in chunks.iter().enumerate() {
            let mut message = CreateMessage::new().embeds(chunk.to_vec());
            if i == 0 {
                message = message.content(intro.clone());
            }
            if i + 1 == chunks.len() {
                message = message.components(controls.clone());
            }
            if let Err(err) = UserId::new(admin).direct_message(&ctx.http, message).await {
                tracing::warn!("quiz news batch {} not sent to admin {}: {}", batch, admin, err);
                break;
            }
        }
    }
}

async fn news_select(ctx: &Context, component: &ComponentInteraction, batch: &str) {
    if !reviewers().contains(&component.user.id.get()) {
        whisper(ctx, component, "Only the quiz reviewer can do this.").await;
        return;
    }
    let chosen: Vec<i64> = match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.iter().filter_map(|v| v.parse().ok()).collect(),
        _ => Vec::new(),
    };
    if let Some(db) = DB.get() {
        let conn = db.lock();
        if meta_get(&conn, &format!("batch:{}", batch)).is_none() {
            let _ = conn.execute(
                "UPDATE submissions SET status = CASE WHEN id IN (SELECT value FROM json_each(?1)) THEN 'rejected' ELSE 'pending' END
                 WHERE batch = ?2 AND status IN ('pending', 'rejected')",
                params![serde_json::to_string(&chosen).unwrap_or_default(), batch],
            );
        }
    }
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await;
}

async fn news_finish(ctx: &Context, component: &ComponentInteraction, batch: &str, approve: bool) {
    let admin = component.user.id.get();
    if !reviewers().contains(&admin) {
        whisper(ctx, component, "Only the quiz reviewer can do this.").await;
        return;
    }
    let Some(db) = DB.get() else {
        return;
    };
    let text = {
        let conn = db.lock();
        let key = format!("batch:{}", batch);
        if let Some(done) = meta_get(&conn, &key) {
            format!("This batch was already handled: {}", done)
        } else {
            let rows: Vec<(i64, String)> = if approve {
                conn.prepare("SELECT id, body FROM submissions WHERE batch = ?1 AND status = 'pending'")
                    .and_then(|mut s| s.query_map(params![batch], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let now = Utc::now().timestamp();
            let mut approved = 0;
            for (sid, body) in rows {
                let Ok(mut q) = serde_json::from_str::<Question>(&body) else {
                    continue;
                };
                q.id = format!("news-{}", sid);
                let inserted = conn.execute(
                    "INSERT OR IGNORE INTO questions (id, body, kind, region, cat, src, added_ts, theme, theme_region)
                     VALUES (?1, ?2, ?3, ?4, ?5, 'news', ?6, 'bollywood_news', 'india')",
                    params![q.id, serde_json::to_string(&q).unwrap_or_default(), q.kind, q.region, q.cat, now],
                );
                if inserted.is_ok() {
                    approved += 1;
                    let _ = conn.execute("UPDATE submissions SET status = 'approved' WHERE id = ?1", params![sid]);
                }
            }
            let _ = conn.execute(
                "UPDATE submissions SET status = 'rejected' WHERE batch = ?1 AND status IN ('pending', 'rejected')",
                params![batch],
            );
            let total: i64 = conn
                .query_row("SELECT COUNT(*) FROM submissions WHERE batch = ?1", params![batch], |r| r.get(0))
                .unwrap_or(0);
            let summary = format!("✅ {} approved · ❌ {} rejected · <@{}>", approved, total - approved, admin);
            meta_set(&conn, &key, &summary);
            summary
        }
    };
    let update = CreateInteractionResponseMessage::new().content(text).components(vec![]);
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
}

/// `/quiznews`: an admin makes this week's batch now instead of waiting for Monday.
pub async fn news_command(ctx: &Context, deps: &VizierDependencies, agent_id: &str, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let reply = CreateInteractionResponseMessage::new().content("Only admins can do this.").ephemeral(true);
        let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
        return;
    }
    let _ = command.defer_ephemeral(&ctx.http).await;
    let text = match news_round(ctx, deps, agent_id).await {
        Ok(n) => format!("📰 Made {} news questions and sent them to you by DM for approval.", n),
        Err(err) => format!("Couldn't make news questions: {}", err),
    };
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(text)).await;
}

/// Every Monday at `NEWS_HOUR` in India, once per week, surviving restarts.
/// `VIZIER_QUIZ_NEWS=off` turns the weekly run off; `/quiznews` still works.
pub fn spawn_weekly_news(ctx: Context, deps: VizierDependencies, agent_id: String) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1800)).await;
            if std::env::var("VIZIER_QUIZ_NEWS").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off")) {
                continue;
            }
            let week = week_start();
            let due = Utc::now().timestamp() >= week + NEWS_HOUR * 3600
                && DB.get().is_some_and(|db| {
                    let conn = db.lock();
                    let fresh = meta_get(&conn, "news_week").as_deref() != Some(week.to_string().as_str());
                    if fresh {
                        meta_set(&conn, "news_week", &week.to_string());
                    }
                    fresh
                });
            if due {
                match news_round(&ctx, &deps, &agent_id).await {
                    Ok(n) => tracing::info!("quiz news: weekly batch of {} sent", n),
                    Err(err) => tracing::warn!("quiz news: weekly batch failed: {}", err),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_files_split_and_hand_picked_questions_move_theme() {
        let workspace = std::env::temp_dir().join(format!("quizretheme-{}", std::process::id()));
        let bank = workspace.join("quizbank");
        std::fs::create_dir_all(bank.join("ai")).unwrap();
        std::fs::create_dir_all(bank.join("api")).unwrap();
        let row = |id: &str, cat: &str, region: &str| {
            format!(
                r#"{{"id":"{}","kind":"text","q":"Question {}?","a":"A","alt":[],"options":[],"cat":"{}","region":"{}","diff":"easy","note":""}}"#,
                id, id, cat, region
            )
        };
        let culture = [row("c-food", "food", "india"), row("c-arts", "arts", "india")].join("\n");
        std::fs::write(bank.join("ai").join("culture.jsonl"), culture).unwrap();
        let api = [
            row("otdb-poke", "video_games", "world"),
            row("otdb-mario", "video_games", "world"),
            row("otdb-naruto", "japanese_anime_manga", "world"),
            row("otdb-1951", "film", "world"),
        ]
        .join("\n");
        std::fs::write(bank.join("api").join("opentdb.jsonl"), api).unwrap();
        std::fs::write(bank.join("retheme.json"), r#"{"pokemon": ["otdb-poke"], "bollywood_songs": ["c-arts"]}"#).unwrap();
        std::fs::write(bank.join("dropped.json"), r#"{"old": ["otdb-1951"]}"#).unwrap();
        let conn = open_conn(workspace.to_str().unwrap()).unwrap();
        let theme = |id: &str| -> (String, String) {
            conn.query_row("SELECT theme, theme_region FROM questions WHERE id = ?1", params![id], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap()
        };
        assert_eq!(theme("c-food"), ("indian_food".into(), "india".into()));
        assert_eq!(theme("c-arts"), ("bollywood_songs".into(), "india".into()));
        assert_eq!(theme("otdb-poke"), ("pokemon".into(), "world".into()));
        assert_eq!(theme("otdb-mario"), ("video_games".into(), "world".into()));
        assert_eq!(theme("otdb-naruto"), ("anime_comics".into(), "world".into()));
        let dropped: i64 =
            conn.query_row("SELECT COUNT(*) FROM questions WHERE id = 'otdb-1951'", [], |r| r.get(0)).unwrap();
        assert_eq!(dropped, 0);
        drop(conn);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    /// A new topic file must be added to a genre, or it only ever shows up in the mix.
    #[test]
    fn every_bank_theme_belongs_to_a_genre() {
        // The vote is one button per genre plus the mix, and a message holds at most 25 buttons.
        assert!(GENRES.len() + 1 <= 25, "{} genres won't fit on the vote", GENRES.len());
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("quizbank");
        let mut files = Vec::new();
        collect_jsonl(&root, &mut files);
        let moved = read_retheme(&root);
        let in_genre = |theme: &str| GENRES.iter().any(|g| g.themes.contains(&theme));
        let mut missing: std::collections::BTreeSet<String> = moved.values().filter(|t| !in_genre(t)).cloned().collect();
        for file in &files {
            let folder = file.parent().and_then(|p| p.file_name()).unwrap().to_string_lossy().into_owned();
            if !["ai", "api", "wikidata"].contains(&folder.as_str()) {
                continue;
            }
            let stem = file.file_stem().unwrap().to_string_lossy().into_owned();
            for line in std::fs::read_to_string(file).unwrap().lines().filter(|l| !l.trim().is_empty()) {
                let Ok(q) = serde_json::from_str::<Question>(line) else {
                    continue;
                };
                let theme = moved.get(&q.id).cloned().unwrap_or_else(|| theme_for(&folder, &stem, &q.cat, &q.region).0);
                if !in_genre(&theme) {
                    missing.insert(theme);
                }
            }
        }
        assert!(missing.is_empty(), "themes in no genre: {:?}", missing);
    }

    #[test]
    fn round_top_three_breaks_ties_by_who_got_there_first() {
        let board = Board {
            key: "bollywood".into(),
            label: "🎬 Bollywood".into(),
            asked: 20,
            // a and b both on 5, but b reached 5 first (win #9 before a's win #12).
            scores: HashMap::from([(1, (5, 12)), (2, (5, 9)), (3, (7, 15)), (4, (1, 2))]),
            wins: 18,
        };
        let order: Vec<u64> = standings(&board.scores).iter().map(|r| r.0).collect();
        assert_eq!(order, vec![3, 2, 1, 4]);
        let text = round_summary(&board, Some(2));
        assert!(text.contains("👑 <@3> is crowned the **🎬 Bollywood** champion · 2 crowns in this genre"), "{}", text);
        assert!(text.contains("18 of 20 questions answered"), "{}", text);
        assert!(text.contains("🥇 <@3> · **7**") && text.contains("🥈 <@2> · **5**") && text.contains("🥉 <@1> · **5**"));
        assert!(!text.contains("<@4>") && text.contains("Tied"));
        let empty = Board { label: "🎲 Mix".into(), asked: 20, ..Default::default() };
        assert!(round_summary(&empty, None).contains("Nobody scored"));
    }

    #[test]
    fn finished_rounds_crown_genre_champions() {
        let workspace = std::env::temp_dir().join(format!("quizcrown-{}", std::process::id()));
        std::fs::create_dir_all(workspace.join("quizbank")).unwrap();
        let conn = open_conn(workspace.to_str().unwrap()).unwrap();
        let board = |key: &str, label: &str, scores: &[(u64, u32)]| Board {
            key: key.into(),
            label: label.into(),
            asked: 20,
            scores: scores.iter().enumerate().map(|(i, (u, p))| (*u, (*p, i as u64))).collect(),
            wins: 0,
        };
        assert_eq!(record_round(&conn, &board("bollywood", "🎬 Bollywood", &[(1, 7), (2, 5)])), Some(1));
        assert_eq!(record_round(&conn, &board("bollywood", "🎬 Bollywood", &[(1, 6)])), Some(2));
        assert_eq!(record_round(&conn, &board("bollywood", "🎬 Bollywood", &[(2, 9), (1, 3)])), Some(1));
        assert_eq!(record_round(&conn, &board("pokemon", "🔴 Pokémon", &[])), None);
        assert_eq!(record_round(&conn, &board(MIX, "🎲 Mix", &[(3, 4)])), Some(1));
        // Bollywood: 2 won the latest round, 1 has the most crowns; nobody won Pokémon, so it isn't listed.
        assert_eq!(
            champion_lines(&conn),
            vec!["🎬 Bollywood · 👑 <@2> · 🏆 <@1> ×2".to_string(), "🎲 Mix · 👑🏆 <@3> ×1".to_string()]
        );
        drop(conn);
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn answers_forgive_slips_but_not_wrong_answers() {
        assert!(close_enough("mumbai", "Mumbai"));
        assert!(close_enough("  The Mumbai!! ", "Mumbai"));
        assert!(close_enough("new delhi", "New Delhi"));
        assert!(close_enough("newdelhi", "New Delhi"));
        assert!(close_enough("tendulker", "Tendulkar"));
        assert!(close_enough("ramesh sipy", "Ramesh Sippy"));
        assert!(!close_enough("iraq", "Iran"));
        assert!(!close_enough("australia", "Austria"));
        assert!(!close_enough("slovenia", "Slovakia"));
        assert!(!close_enough("1984", "1983"));
        assert!(!close_enough("dhoom 2", "Dhoom"));
        assert!(!close_enough("kahaani2", "Kahaani"));
        assert!(close_enough("Dhoom 2", "dhoom2"));
        assert!(!close_enough("", "Mumbai"));
        assert!(close_enough("Rock & Roll", "rock and roll"));
        // Hinglish spellings of the same word.
        assert!(close_enough("kabeer", "Kabir"));
        assert!(close_enough("kotwaal", "kotwal"));
        assert!(close_enough("ranee", "rani"));
        assert!(close_enough("pavan", "Pawan"));
        assert!(close_enough("Sachhin Tendulkar", "Sachin Tendulkar"));
        assert!(!close_enough("dhoom 22", "Dhoom 2"));
    }

    #[test]
    fn bank_imports_skips_bad_rows_and_picks_every_question_once() {
        let workspace = std::env::temp_dir().join(format!("quiztest-{}", std::process::id()));
        let bank = workspace.join("quizbank").join("ai");
        std::fs::create_dir_all(&bank).unwrap();
        let lines = [
            r#"{"id":"t-1","kind":"text","q":"Capital of Karnataka?","a":"Bengaluru","alt":["Bangalore"],"options":[],"cat":"geography","region":"india","diff":"easy","note":""}"#,
            r#"{"id":"t-2","kind":"mcq","q":"Largest planet?","a":"Jupiter","alt":[],"options":["Mars","Jupiter","Venus","Earth"],"cat":"science","region":"world","diff":"easy","note":""}"#,
            r#"{"id":"t-3","kind":"mcq","q":"Broken, answer not an option","a":"X","alt":[],"options":["A","B","C","D"],"cat":"science","region":"world","diff":"easy","note":""}"#,
            "not json",
        ];
        std::fs::write(bank.join("sample.jsonl"), lines.join("\n")).unwrap();
        open(workspace.to_str().unwrap()).unwrap();

        // India or world is a weighted coin toss per question, so look at many picks.
        let ids: std::collections::BTreeSet<String> =
            (0..40).filter_map(|_| pick(&Recent::default(), None)).map(|q| q.id).collect();
        assert_eq!(ids.into_iter().collect::<Vec<_>>(), ["t-1", "t-2"]);

        let bengaluru = {
            let conn = DB.get().unwrap().lock();
            conn.execute("UPDATE questions SET retired = 1 WHERE id = 't-2'", []).unwrap();
            let body: String = conn.query_row("SELECT body FROM questions WHERE id = 't-1'", [], |r| r.get(0)).unwrap();
            serde_json::from_str::<Question>(&body).unwrap()
        };
        assert!(bengaluru.accepts("bangalore") && bengaluru.accepts("Bengaluru") && !bengaluru.accepts("Mysore"));
        for _ in 0..5 {
            assert_eq!(pick(&Recent::default(), None).map(|q| q.id).as_deref(), Some("t-1"));
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn mix_never_runs_three_of_a_kind_or_three_world_in_a_row() {
        let workspace = std::env::temp_dir().join(format!("quizmix-{}", std::process::id()));
        let bank = workspace.join("quizbank");
        std::fs::create_dir_all(&bank).unwrap();
        let mut lines = Vec::new();
        let mut add = |n: usize, kind: &str, region: &str, cats: &[&str]| {
            for i in 0..n {
                let options = if kind == "mcq" { r#"["A","B","C","D"]"# } else { "[]" };
                lines.push(format!(
                    r#"{{"id":"{k}-{r}-{i}","kind":"{k}","q":"Q {k} {r} {i}","a":"A","alt":[],"options":{o},"cat":"{c}","region":"{r}","diff":"easy","note":""}}"#,
                    k = kind, r = region, i = i, o = options, c = cats[i % cats.len()]
                ));
            }
        };
        // Every region-and-kind pool has four topics, one more than the three
        // that sit out, so all three rules can always be met - as in the real bank.
        add(40, "text", "india", &["films", "food", "cricket", "history"]);
        add(20, "mcq", "india", &["films", "geography", "music", "tv"]);
        add(40, "mcq", "world", &["games", "science", "music", "sport"]);
        add(20, "text", "world", &["science", "capitals", "elements", "art"]);
        std::fs::write(bank.join("mix.jsonl"), lines.join("\n")).unwrap();
        let conn = open_conn(workspace.to_str().unwrap()).unwrap();

        let mut recent = Recent::default();
        let picked: Vec<Question> = (0..120)
            .map(|_| {
                let q = pick_from(&conn, &recent, None).expect("a question");
                recent.push(&q);
                q
            })
            .collect();
        for run in picked.windows(3) {
            assert!(!(run[0].kind == run[1].kind && run[1].kind == run[2].kind), "three {} in a row", run[0].kind);
            assert!(!run.iter().all(|q| q.region == "world"), "three world questions in a row");
            assert!(!(run[0].cat == run[1].cat || run[1].cat == run[2].cat || run[0].cat == run[2].cat));
        }
        let india = picked.iter().filter(|q| q.region == "india").count() as f64 / picked.len() as f64;
        assert!((0.55..=0.95).contains(&india), "india share {}", india);

        // A voted genre only ever serves its own themes, from either side.
        let genre: &[&str] = &["films", "science"];
        let mut recent = Recent::default();
        for _ in 0..30 {
            let q = pick_from(&conn, &recent, Some(genre)).expect("a genre question");
            assert!(genre.contains(&q.theme.as_str()), "{} is not in the genre", q.theme);
            recent.push(&q);
        }
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn news_items_parse_and_only_grounded_clean_drafts_survive() {
        let xml = r#"<rss><channel>
<item><title><![CDATA[Haiwaan trailer out: Akshay Kumar &amp; Saif Ali Khan reunite]]></title>
<description>&lt;p&gt;The makers released the trailer on Monday.&lt;/p&gt;</description>
<link>https://example.com/a</link><pubDate><![CDATA[Fri, 11 Sep 2026 12:43:16 +0530]]></pubDate></item>
<item><title>Star reportedly dating co-star after film wrap</title><description>Rumours fly.</description>
<link>https://example.com/b</link><pubDate>2026-09-10T07:20:38+05:30</pubDate></item>
<item><title>No date on this one</title></item>
</channel></rss>"#;
        let items = parse_feed(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Haiwaan trailer out: Akshay Kumar & Saif Ali Khan reunite");
        assert_eq!(items[0].summary, "The makers released the trailer on Monday.");
        assert!(SENSITIVE.is_match(&items[1].title) && !SENSITIVE.is_match(&items[0].title));

        let draft = |q: &str, a: &str, item: usize| Drafted {
            kind: "text".into(),
            q: q.into(),
            a: a.into(),
            alt: vec![],
            options: vec![],
            note: String::new(),
            item,
        };
        let reply = "```json\n[{\"kind\":\"text\",\"q\":\"x\",\"a\":\"y\",\"item\":1}]\n```";
        assert_eq!(parse_drafts(reply).len(), 1);
        let vetted = vet_drafts(
            vec![
                draft("In September 2026, who directed Haiwaan?", "Priyadarshan", 1),
                draft("In September 2026, which actor reunited with Saif Ali Khan in the Haiwaan trailer?", "Akshay Kumar", 1),
                draft("In September 2026, which trailer did Akshay Kumar share?", "Haiwaan", 1),
                draft("In September 2026, who was reportedly dating a co-star?", "Star", 2),
                draft("Out of range", "Akshay Kumar", 9),
            ],
            &items,
        );
        let answers: Vec<&str> = vetted.iter().map(|(q, _)| q.a.as_str()).collect();
        assert_eq!(answers, ["Akshay Kumar"]);
        assert_eq!(vetted[0].1, "https://example.com/a");
    }

    #[test]
    fn a_real_different_answer_is_never_a_typo() {
        let question = |a: &str| Question {
            id: "x".into(),
            kind: "text".into(),
            q: "?".into(),
            a: a.into(),
            alt: vec![],
            options: vec![],
            cat: String::new(),
            region: "world".into(),
            diff: String::new(),
            note: String::new(),
            src: String::new(),
            added_by: None,
            theme: String::new(),
            theme_region: String::new(),
        };
        let known: HashSet<String> = ["iceland", "ireland", "austria", "australia", "jaipur", "raipur"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(!question("Ireland").accepts_given("Iceland", &known));
        assert!(question("Ireland").accepts_given("Irelnd", &known));
        assert!(question("Ireland").accepts_given("ireland", &known));
        assert!(!question("Australia").accepts_given("austria", &known));
        assert!(!question("Raipur").accepts_given("Jaipur", &known));
        assert!(question("Raipur").accepts_given("Raipor", &known));
    }

    #[test]
    fn hints_show_first_letters() {
        assert_eq!(hint_at("Mumbai", 1), "M _ _ _ _ _");
        assert_eq!(hint_at("Ramesh Sippy", 1), "R _ _ _ _ _   S _ _ _ _");
        assert_eq!(hint_at("Mumbai", 2), "M u _ _ _ _");
        assert_eq!(hint_at("Ramesh Sippy", 2), "R a _ _ _ _   S i _ _ _");
        // Never past half a word, however many hints are asked for.
        assert_eq!(hint_at("Mumbai", 9), "M u m _ _ _");
        assert_eq!(hint_at("1983", 9), "1 9 _ _");
    }
}
