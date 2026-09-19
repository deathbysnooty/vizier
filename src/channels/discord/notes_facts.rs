//! Member notes, the free part: facts about a member read straight from the
//! bot's own databases when someone asks (and kept for a few minutes), and the
//! answer they are written up into. No model is involved anywhere here.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::TimeZone;
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, params};

use super::notes_build;
use super::notes_store::Note;

/// Discord's limit on one message.
pub const DISCORD_LIMIT: usize = 2000;
/// How long a member's facts are reused before being read again.
pub const CACHE_SECS: i64 = 10 * 60;
/// Of their newest messages, this many are read for the emoji and phrases.
pub const OWN_MESSAGES: usize = 3000;
/// And this many of everyone's, to know what is ordinary on the server.
pub const SERVER_MESSAGES: usize = 20_000;
/// The window "who they talk with" looks at.
pub const PARTNER_DAYS: i64 = 90;

#[derive(Clone, Debug, PartialEq)]
pub struct GameLine {
    pub name: &'static str,
    /// How often they have played, for ordering.
    pub plays: i64,
    /// How they do there, in words: "40 games, 22 won".
    pub detail: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub member_since: Option<i64>,
    pub messages_month: i64,
    pub messages_all: i64,
    pub top_channels: Vec<u64>,
    pub games: Vec<GameLine>,
    pub house: Option<&'static str>,
    pub muggle: bool,
    pub points_month: i64,
    /// Their place among the house's scorers this month, and how many scored.
    pub house_place: Option<(usize, usize)>,
    pub frog_cards: i64,
    pub voice_month_secs: i64,
    pub partners: Vec<u64>,
    pub emoji: Option<String>,
    pub phrases: Vec<String>,
}

// --- reading -----------------------------------------------------------------------------------

fn count(conn: &Connection, sql: &str, user: u64) -> i64 {
    conn.query_row(sql, params![user as i64], |r| r.get::<_, Option<i64>>(0)).ok().flatten().unwrap_or(0)
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// Their game records, from each game's own store: the games they play most
/// first, only the ones they have played.
pub fn games_of(user: u64) -> Vec<GameLine> {
    let mut out = Vec::new();
    if let Some(db) = super::anagram_store::db() {
        let conn = db.lock();
        let n = count(&conn, "SELECT COUNT(*) FROM solves WHERE user_id = ?1", user);
        let best = count(&conn, "SELECT MIN(seconds) FROM solves WHERE user_id = ?1 AND seconds > 0", user);
        if n > 0 {
            let fast = if best > 0 { format!(", fastest in {}s", best) } else { String::new() };
            out.push(GameLine { name: "Anagrams", plays: n, detail: format!("{} solved{}", n, fast) });
        }
    }
    if let Some(db) = super::guess_store::db() {
        let n = count(&db.lock(), "SELECT COUNT(*) FROM solves WHERE user_id = ?1", user);
        if n > 0 {
            out.push(GameLine { name: "Guess the Word", plays: n, detail: format!("{} guessed first", n) });
        }
    }
    if let Some(db) = super::sudoku_store::db() {
        let conn = db.lock();
        let played = count(&conn, "SELECT COUNT(*) FROM players WHERE user_id = ?1", user);
        let solved = count(&conn, "SELECT COUNT(*) FROM solves WHERE user_id = ?1", user);
        let won = count(&conn, "SELECT COUNT(*) FROM solves WHERE user_id = ?1 AND kind = 'win'", user);
        if played.max(solved) > 0 {
            out.push(GameLine { name: "Sudoku", plays: played.max(solved), detail: format!("{} solved, {} first", solved, won) });
        }
    }
    if let Some(db) = super::chess_store::db() {
        let conn = db.lock();
        let games = count(&conn, "SELECT COUNT(*) FROM games WHERE (white = ?1 OR black = ?1) AND status = 'done'", user);
        let won = count(&conn, "SELECT COUNT(*) FROM games WHERE winner = ?1 AND status = 'done'", user);
        let drawn = count(&conn, "SELECT COUNT(*) FROM games WHERE (white = ?1 OR black = ?1) AND status = 'done' AND result = 'draw'", user);
        if games > 0 {
            let draws = if drawn > 0 { format!(", {} drawn", drawn) } else { String::new() };
            out.push(GameLine { name: "Chess", plays: games, detail: format!("{}, {} won{}", plural(games, "game", "games"), won, draws) });
        }
    }
    if let Some(db) = super::puzzle_store::db() {
        let conn = db.lock();
        let tried = count(&conn, "SELECT COUNT(*) FROM players WHERE user_id = ?1", user);
        let solved = count(&conn, "SELECT COUNT(*) FROM solves WHERE user_id = ?1", user);
        let first = count(&conn, "SELECT COUNT(*) FROM solves WHERE user_id = ?1 AND first = 1", user);
        if tried.max(solved) > 0 {
            out.push(GameLine { name: "Chess puzzles", plays: tried.max(solved), detail: format!("{} solved, {} first", solved, first) });
        }
    }
    if let Some(db) = super::duel_store::db() {
        let conn = db.lock();
        let games = count(&conn, "SELECT COUNT(*) FROM players p JOIN games g ON g.id = p.game_id WHERE p.user_id = ?1 AND g.status = 'over'", user);
        let best = count(&conn, "SELECT MAX(p.score) FROM players p JOIN games g ON g.id = p.game_id WHERE p.user_id = ?1 AND g.status = 'over'", user);
        if games > 0 {
            out.push(GameLine { name: "Letter Duel", plays: games, detail: format!("{}, best score {}", plural(games, "game", "games"), best) });
        }
    }
    if let Some(db) = super::npat_store::db() {
        let conn = db.lock();
        let games = count(&conn, "SELECT COUNT(*) FROM game_scores WHERE user_id = ?1", user);
        let won = count(&conn, "SELECT COUNT(*) FROM game_scores WHERE user_id = ?1 AND rank = 1", user);
        if games > 0 {
            out.push(GameLine { name: "Name Place Animal Thing", plays: games, detail: format!("{}, {} won", plural(games, "game", "games"), won) });
        }
    }
    let (quiz, _) = super::quiz::points_of(user, 0);
    if quiz > 0 {
        out.push(GameLine { name: "Quiz", plays: quiz, detail: format!("{} answered first", quiz) });
    }
    let (fights, wins, crowns) = super::battle::record_of(user);
    if fights > 0 {
        let crowns = if crowns > 0 { format!(", {} royale {}", crowns, if crowns == 1 { "crown" } else { "crowns" }) } else { String::new() };
        out.push(GameLine { name: "Fights", plays: fights, detail: format!("{}, {} won{}", plural(fights, "fight", "fights"), wins, crowns) });
    }
    out.sort_by(|a, b| b.plays.cmp(&a.plays).then(a.name.cmp(b.name)));
    out.truncate(3);
    out
}

/// Everything about a member, read from the stores one lock at a time.
/// Blocking: call off the async runtime.
pub fn read(user: u64, now: i64, joined_at: Option<i64>, name_tokens: &HashSet<String>) -> Facts {
    let mut f = Facts { member_since: joined_at, ..Default::default() };
    let month = super::points::month_start(now);
    let sensitive = super::control::insights::sensitive_channels();
    // These take the house lock themselves, so before it is taken below.
    let home = super::house::house_of(user);
    let optouts = super::house::optout_set();
    f.muggle = optouts.contains(&user);
    f.house = home.map(|h| h.key);
    if let Some(db) = super::house::db() {
        let conn = db.lock();
        f.points_month = conn
            .query_row("SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1 AND ts >= ?2", params![user as i64, month], |r| r.get(0))
            .unwrap_or(0);
        if let Some(h) = home {
            if let Ok(scorers) = super::points::top_members(&conn, h.key, month, i64::MAX) {
                let ranked: Vec<u64> = scorers.into_iter().map(|(id, _)| id).filter(|id| !optouts.contains(id)).collect();
                f.house_place = ranked.iter().position(|id| *id == user).map(|i| (i + 1, ranked.len()));
            }
        }
    }
    if let Some(db) = super::stats::db() {
        let conn = db.lock();
        let month_day = super::points::ist_day(month);
        f.messages_all = count(&conn, "SELECT COALESCE(SUM(count), 0) FROM msg_counts WHERE user_id = ?1", user);
        f.messages_month = conn
            .query_row("SELECT COALESCE(SUM(count), 0) FROM msg_counts WHERE user_id = ?1 AND day >= ?2", params![user as i64, month_day], |r| r.get(0))
            .unwrap_or(0);
        if let Ok(mut stmt) = conn.prepare("SELECT channel_id, SUM(count) AS n FROM msg_counts WHERE user_id = ?1 GROUP BY channel_id ORDER BY n DESC LIMIT 8") {
            if let Ok(rows) = stmt.query_map(params![user as i64], |r| r.get::<_, i64>(0)) {
                f.top_channels = rows.flatten().map(|c| c as u64).filter(|c| !sensitive.contains(c)).take(3).collect();
            }
        }
        if let Ok(voice) = super::activity::voice_between(&conn, Some(user), month, now, now) {
            f.voice_month_secs = voice.get(&user).copied().unwrap_or(0);
        }
    }
    if let Some(db) = super::frog_store::db() {
        f.frog_cards = super::frog_store::cards_of(&db.lock(), user).len() as i64;
    }
    f.games = games_of(user);
    f.partners = super::control::insights::partners(user, now - PARTNER_DAYS * 86_400, 6).into_iter().map(|(u, _)| u).collect();
    if let Some(reader) = super::msglog::reader() {
        // The background reads the same connection: this lock is let go first.
        let own = own_messages(&reader.conn.lock(), user, &sensitive);
        let background = background();
        f.emoji = favourite_emoji(&own);
        f.phrases = phrases(&own, &background, name_tokens);
    }
    f
}

fn own_messages(conn: &Connection, user: u64, sensitive: &[u64]) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT content, channel_id, parent_id FROM recent WHERE author_id = ?1 ORDER BY message_id DESC LIMIT ?2") else {
        return Vec::new();
    };
    stmt.query_map(params![user as i64, OWN_MESSAGES as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, Option<i64>>(2)?.map(|p| p as u64)))
    })
    .map(|rows| rows.flatten().filter(|(_, c, p)| !sensitive.contains(c) && !p.is_some_and(|p| sensitive.contains(&p))).map(|(t, _, _)| t).collect())
    .unwrap_or_default()
}

// --- emoji and phrases -------------------------------------------------------------------------

static CUSTOM_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:\w+:\d+>").expect("regex"));

fn is_emoji(c: char) -> bool {
    let n = c as u32;
    (0x1F300..=0x1FAFF).contains(&n) && !(0x1F3FB..=0x1F3FF).contains(&n) || (0x2600..=0x27BF).contains(&n)
}

/// The emoji they use most (at most three counted per message, so one message
/// of thirty skulls doesn't win it). Needs at least five uses.
pub fn favourite_emoji(messages: &[String]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for m in messages {
        let mut here: HashMap<String, usize> = HashMap::new();
        for e in CUSTOM_EMOJI.find_iter(m) {
            *here.entry(e.as_str().to_string()).or_default() += 1;
        }
        let bare = CUSTOM_EMOJI.replace_all(m, "");
        for c in bare.chars().filter(|c| is_emoji(*c)) {
            *here.entry(c.to_string()).or_default() += 1;
        }
        for (e, n) in here {
            *counts.entry(e).or_default() += n.min(3);
        }
    }
    counts.into_iter().filter(|(_, n)| *n >= 5).max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(e, _)| e)
}

/// Words that say nothing about anyone: English and Hinglish filler.
const STOPWORDS: &[&str] = &[
    "the", "and", "you", "for", "are", "but", "not", "this", "that", "with", "have", "was", "what", "just", "all", "can", "how",
    "why", "who", "its", "like", "get", "got", "yes", "yeah", "yea", "okay", "there", "they", "them", "then", "than", "from",
    "your", "about", "will", "would", "could", "should", "been", "were", "when", "where", "which", "one", "out", "now", "too",
    "very", "really", "also", "some", "any", "our", "his", "her", "she", "him", "has", "had", "did", "does", "don", "dont",
    "didnt", "cant", "wont", "im", "ive", "its", "it", "is", "me", "my", "we", "us", "so", "if", "or", "of", "on", "in", "at",
    "an", "a", "i", "to", "be", "do", "go", "no", "up", "by", "as", "ok", "oh", "hai", "hain", "ho", "hoga", "hogi", "kya",
    "kyu", "kyun", "kyon", "nahi", "nhi", "nahin", "mai", "main", "mein", "tu", "tum", "aap", "hum", "bhai", "yaar", "yr",
    "bro", "toh", "bhi", "se", "ka", "ki", "ke", "ko", "na", "haan", "ha", "han", "par", "pe", "aur", "ya", "kar", "karo", "kr",
    "kro", "raha", "rahi", "rahe", "tha", "thi", "gaya", "gya", "gayi", "diya", "liya", "abhi", "sab", "kuch", "koi", "kaise",
    "kaisa", "kab", "kaha", "kahan", "woh", "wo", "ye", "yeh", "us", "uska", "uski", "mera", "meri", "mere", "tera", "teri",
    "tere", "apna", "apni", "hi", "bas", "ek", "lol", "lmao", "haha", "hahaha", "xd", "bruh", "sahi", "accha", "acha", "achha",
    "matlab", "wala", "wali", "wale", "kal", "aaj", "fir", "phir", "jab", "tab", "agar", "lekin", "bohot", "bahut", "bht",
    "bhot", "ab", "sirf", "hua", "hui", "hue", "rha", "rhi", "rhe", "hota", "hoti", "hote", "kiya", "kiye", "karna", "karke",
    "krke", "dekh", "dekho", "bol", "bolo", "bata", "batao", "mujhe", "tujhe", "usko", "isko", "unko", "apne", "someone",
    "channel", "hmm", "hmmm", "arey", "are", "re", "ji", "aa", "ja", "jaa", "de", "le", "lo", "do", "kr", "ni", "nai", "nah",
];

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z]+").expect("regex"));
static STRIP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[@#][!&]?\d+>|<a?:\w+:\d+>|(?i)https?://\S+").expect("regex"));

fn tokens(text: &str) -> Vec<String> {
    let clean = STRIP.replace_all(text, " ").to_lowercase();
    WORD.find_iter(&clean).map(|m| m.as_str().to_string()).collect()
}

/// For each word and each pair of words, how many of these messages have it.
pub fn doc_freq(messages: &[String]) -> HashMap<String, u32> {
    let mut out: HashMap<String, u32> = HashMap::new();
    for m in messages {
        let t = tokens(m);
        let mut here: HashSet<String> = t.iter().cloned().collect();
        for pair in t.windows(2) {
            here.insert(format!("{} {}", pair[0], pair[1]));
        }
        for k in here {
            *out.entry(k).or_default() += 1;
        }
    }
    out
}

/// What's ordinary on the server: word and pair counts over everyone's recent
/// messages, and how many messages that was. Read at most every six hours.
pub struct Background {
    pub freq: HashMap<String, u32>,
    pub messages: usize,
}

fn background() -> std::sync::Arc<Background> {
    static CACHE: LazyLock<Mutex<Option<(i64, std::sync::Arc<Background>)>>> = LazyLock::new(|| Mutex::new(None));
    let now = chrono::Utc::now().timestamp();
    if let Some((at, bg)) = CACHE.lock().as_ref() {
        if now - at < 6 * 3600 {
            return bg.clone();
        }
    }
    let messages: Vec<String> = super::msglog::reader()
        .map(|r| {
            let conn = r.conn.lock();
            conn.prepare("SELECT content FROM recent ORDER BY message_id DESC LIMIT ?1")
                .and_then(|mut stmt| stmt.query_map(params![SERVER_MESSAGES as i64], |r| r.get::<_, String>(0)).map(|rows| rows.flatten().collect()))
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let bg = std::sync::Arc::new(Background { freq: doc_freq(&messages), messages: messages.len() });
    *CACHE.lock() = Some((now, bg.clone()));
    bg
}

fn stop(w: &str) -> bool {
    w.len() < 3 || STOPWORDS.contains(&w)
}

/// One or two words or phrases that are genuinely theirs: used in a fair share
/// of their messages, and several times more often by them than by the server
/// as a whole - never filler, never a name, never anything the notes' filter
/// would refuse.
pub fn phrases(own: &[String], bg: &Background, names: &HashSet<String>) -> Vec<String> {
    if own.len() < 30 {
        return Vec::new();
    }
    let mine = doc_freq(own);
    let n = own.len() as f64;
    let bg_n = bg.messages.max(1) as f64;
    let none = HashSet::new();
    let mut scored: Vec<(String, f64)> = mine
        .iter()
        .filter_map(|(k, &c)| {
            let words: Vec<&str> = k.split(' ').collect();
            let pair = words.len() == 2;
            if pair && (words.iter().all(|w| stop(w)) || words[0] == words[1]) {
                return None;
            }
            if !pair && stop(k) {
                return None;
            }
            if words.iter().any(|w| names.contains(*w)) || notes_build::rejection(k, &none).is_some() {
                return None;
            }
            let min = if pair { 4 } else { 6 };
            let share = c as f64 / n;
            if c < min || share < 0.015 {
                return None;
            }
            let ratio = share / ((bg.freq.get(k).copied().unwrap_or(0) as f64 + 1.0) / bg_n);
            (ratio >= if pair { 4.0 } else { 3.0 }).then(|| (k.clone(), ratio * share.sqrt() * if pair { 1.3 } else { 1.0 }))
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let mut out: Vec<String> = Vec::new();
    for (k, _) in scored {
        if out.iter().any(|o| o.contains(k.as_str()) || k.contains(o.as_str())) {
            continue;
        }
        out.push(k);
        if out.len() == 2 {
            break;
        }
    }
    out
}

// --- the cache ---------------------------------------------------------------------------------

static CACHE: LazyLock<Mutex<HashMap<u64, (i64, Facts)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn cached(user: u64, now: i64) -> Option<Facts> {
    CACHE.lock().get(&user).filter(|(at, _)| now - at < CACHE_SECS).map(|(_, f)| f.clone())
}

pub fn remember(user: u64, now: i64, facts: &Facts) {
    let mut cache = CACHE.lock();
    cache.retain(|_, (at, _)| now - *at < CACHE_SECS);
    cache.insert(user, (now, facts.clone()));
}

// --- writing it up -----------------------------------------------------------------------------

pub fn date(ts: i64) -> String {
    super::stats::ist().timestamp_opt(ts, 0).single().map(|t| t.format("%-d %b %Y").to_string()).unwrap_or_default()
}

fn thousands(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{}", out) } else { out }
}

fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{} and {}", a, b),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

/// What the stored notes look like to the answer: built, never built, or
/// kept out because they opted out.
pub enum Summary<'a> {
    Note(&'a Note),
    OptedOut,
    TooLittle,
    NotYet,
}

/// The whole answer: the facts first, then "what they're like" and when it was
/// last updated. Always inside Discord's limit.
pub fn render(name: &str, you: bool, f: &Facts, summary: Summary, channel_name: &dyn Fn(u64) -> Option<String>, member_name: &dyn Fn(u64) -> Option<String>) -> String {
    let opted_out = matches!(summary, Summary::OptedOut);
    let mut lines: Vec<String> = vec![format!("## About {}", name), "**The facts**".to_string()];
    if let Some(ts) = f.member_since {
        lines.push(format!("- Member since {}", date(ts)));
    }
    lines.push(format!("- Messages: {} this month · {} all time", thousands(f.messages_month), thousands(f.messages_all)));
    if !opted_out {
        let channels: Vec<String> = f.top_channels.iter().filter_map(|c| channel_name(*c)).map(|n| format!("#{}", n)).collect();
        if !channels.is_empty() {
            lines.push(format!("- Mostly in {}", and_list(&channels)));
        }
    }
    if !f.games.is_empty() {
        let games: Vec<String> = f.games.iter().map(|g| format!("{} ({})", g.name, g.detail)).collect();
        lines.push(format!("- Plays most: {}", games.join(" · ")));
    }
    let house = f.house.and_then(super::house::house);
    match (house, f.muggle) {
        (_, true) => lines.push("- House: none - stepped out as a Muggle".to_string()),
        (Some(h), _) => {
            let place = f.house_place.map(|(p, of)| format!(", {} of {} in the house", ordinal(p), of)).unwrap_or_default();
            lines.push(format!("- House: {} {} · {} points this month{}", h.crest, h.name, thousands(f.points_month), place));
        }
        (None, _) => lines.push("- House: not sorted yet".to_string()),
    }
    if f.frog_cards > 0 {
        lines.push(format!("- Frog cards: {}", f.frog_cards));
    }
    if f.voice_month_secs >= 60 {
        let min = f.voice_month_secs / 60;
        lines.push(format!("- Voice this month: {} h {} min", min / 60, min % 60));
    }
    if !opted_out {
        let people: Vec<String> = f.partners.iter().filter_map(|u| member_name(*u)).take(3).collect();
        if !people.is_empty() {
            lines.push(format!("- Talks most with {}", and_list(&people)));
        }
        let mut bits = Vec::new();
        if let Some(e) = &f.emoji {
            bits.push(format!("favourite emoji {}", e));
        }
        if !f.phrases.is_empty() {
            let quoted: Vec<String> = f.phrases.iter().map(|p| format!("\"{}\"", p)).collect();
            bits.push(format!("says {} a lot", and_list(&quoted)));
        }
        if !bits.is_empty() {
            let mut s = bits.join(" · ");
            s[..1].make_ascii_uppercase();
            lines.push(format!("- {}", s));
        }
    }
    let facts_end = lines.len();
    match summary {
        Summary::Note(note) => {
            lines.push(format!("**What {} like** (updated {})", if you { "you're" } else { "they're" }, date(note.built_ts)));
            for b in &note.bullets {
                lines.push(format!("- {}", notes_build::cut(b, 200)));
            }
        }
        Summary::OptedOut => lines.push(format!(
            "-# {} opted out of member notes, so nothing about what {} like is kept or built. The facts above are the ones \
             already public elsewhere (points, cards, games).",
            if you { "You've" } else { "They've" },
            if you { "you're" } else { "they're" }
        )),
        Summary::TooLittle => lines.push(format!(
            "-# No \"what {} like\" yet: there isn't enough of {} chat to go on.",
            if you { "you're" } else { "they're" },
            if you { "your" } else { "their" }
        )),
        Summary::NotYet => lines.push(format!(
            "-# No \"what {} like\" yet - it's written once a week for members who chat enough.",
            if you { "you're" } else { "they're" }
        )),
    }
    fit(lines, facts_end)
}

/// Joins the lines inside Discord's limit: facts are dropped from the end
/// first (the summary is what people ask for), then anything still over.
fn fit(mut lines: Vec<String>, mut facts_end: usize) -> String {
    loop {
        let text = lines.join("\n");
        if text.chars().count() <= DISCORD_LIMIT {
            return text;
        }
        if facts_end > 3 {
            lines.remove(facts_end - 1);
            facts_end -= 1;
        } else {
            return notes_build::cut(&text, DISCORD_LIMIT - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> Facts {
        Facts {
            member_since: Some(1_741_737_600),
            messages_month: 1234,
            messages_all: 20511,
            top_channels: vec![1, 2, 3],
            games: vec![
                GameLine { name: "Chess", plays: 40, detail: "40 games, 22 won".into() },
                GameLine { name: "Anagrams", plays: 30, detail: "30 solved, fastest in 4s".into() },
            ],
            house: None,
            muggle: false,
            points_month: 820,
            house_place: Some((3, 41)),
            frog_cards: 17,
            voice_month_secs: 12 * 3600 + 40 * 60,
            partners: vec![10, 11, 12],
            emoji: Some("💀".into()),
            phrases: vec!["scene kya".into(), "arre".into()],
        }
    }

    fn channel(c: u64) -> Option<String> {
        Some(["general", "memes", "gaming"][(c as usize - 1) % 3].to_string())
    }

    fn member(u: u64) -> Option<String> {
        Some(format!("Friend{}", u))
    }

    fn note() -> Note {
        Note {
            user_id: 5,
            bullets: vec!["Brings up cricket every match day".into(), "Dry, deadpan humour".into()],
            built_ts: 1_789_367_400,
            input_tokens: 4200,
            output_tokens: 150,
            model: "m".into(),
            messages_used: 180,
            rejected: 0,
            built_by: 0,
        }
    }

    #[test]
    fn the_answer_puts_the_facts_first_and_dates_the_summary() {
        let n = note();
        let text = render("Riya", false, &facts(), Summary::Note(&n), &channel, &member);
        let facts_at = text.find("**The facts**").unwrap();
        let like_at = text.find("**What they're like** (updated 14 Sep 2026)").unwrap();
        assert!(facts_at < like_at);
        for bit in ["1,234 this month", "20,511 all time", "#general, #memes and #gaming", "Chess (40 games, 22 won)", "Frog cards: 17", "12 h 40 min", "Friend10, Friend11 and Friend12", "💀", "\"scene kya\""] {
            assert!(text.contains(bit), "missing {:?} in\n{}", bit, text);
        }
        assert!(text.contains("- Dry, deadpan humour"));
    }

    #[test]
    fn someone_who_opted_out_gets_only_public_facts_and_a_note_saying_so() {
        let text = render("Riya", false, &facts(), Summary::OptedOut, &channel, &member);
        assert!(text.contains("opted out"));
        assert!(text.contains("already public elsewhere"));
        assert!(text.contains("Frog cards: 17"));
        for private in ["Talks most", "emoji", "Mostly in", "What they're like"] {
            assert!(!text.contains(private), "{:?} shown for an opted-out member", private);
        }
    }

    #[test]
    fn the_answer_always_fits_in_one_discord_message() {
        let mut f = facts();
        f.games = (0..3).map(|_| GameLine { name: "Name Place Animal Thing", plays: 9, detail: "x".repeat(400) }).collect();
        f.phrases = vec!["y".repeat(300), "z".repeat(300)];
        let n = Note { bullets: (0..6).map(|i| format!("{} {}", i, "long words ".repeat(40))).collect(), ..note() };
        let text = render(&"N".repeat(80), true, &f, Summary::Note(&n), &|_| Some("c".repeat(90)), &|_| Some("m".repeat(90)));
        assert!(text.chars().count() <= DISCORD_LIMIT, "{} chars", text.chars().count());
        assert!(text.contains("**What you're like**"), "the summary survives the trimming");
        let plain = render("Riya", true, &facts(), Summary::NotYet, &channel, &member);
        assert!(plain.chars().count() <= DISCORD_LIMIT);
    }

    #[test]
    fn the_favourite_emoji_needs_real_use_and_one_spammy_message_doesnt_win() {
        let mut msgs: Vec<String> = (0..6).map(|_| "haha 😂 sahi".to_string()).collect();
        msgs.push("💀".repeat(40));
        assert_eq!(favourite_emoji(&msgs).as_deref(), Some("😂"));
        assert_eq!(favourite_emoji(&["ok 😂".to_string()]), None);
        let custom: Vec<String> = (0..5).map(|_| "<:kekw:123456789012345678> lol".to_string()).collect();
        assert_eq!(favourite_emoji(&custom).as_deref(), Some("<:kekw:123456789012345678>"));
    }

    #[test]
    fn phrases_are_characteristic_not_filler() {
        let mut everyone: Vec<String> = (0..2000).map(|i| format!("bhai kya hai yaar the match was good {}", i % 7)).collect();
        everyone.extend((0..50).map(|_| "arre scene kya hai".to_string()));
        let bg = Background { freq: doc_freq(&everyone), messages: everyone.len() };
        let mut mine: Vec<String> = (0..60).map(|_| "bhai kya hai yaar the".to_string()).collect();
        mine.extend((0..20).map(|_| "arre bawaal scene tha".to_string()));
        mine.extend((0..10).map(|_| "aryan come online".to_string()));
        let names: HashSet<String> = ["aryan".to_string()].into();
        let got = phrases(&mine, &bg, &names);
        assert!(!got.is_empty(), "something characteristic should be found");
        for p in &got {
            for filler in ["bhai", "hai", "yaar", "the", "kya"] {
                assert_ne!(p, filler);
            }
            assert!(!p.contains("aryan"), "never a name");
            assert!(!p.contains("online"), "never something the filter refuses");
        }
        assert!(got.iter().any(|p| p.contains("bawaal")), "{:?}", got);
    }
}
