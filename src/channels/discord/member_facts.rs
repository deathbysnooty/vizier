//! Facts for /ship and the member tabs, read from the bot's databases.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use chrono::Utc;
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::{Connection, params};
use serenity::all::{Context, GuildId, UserId};

use super::message_text;

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
/// as a whole. Excludes filler, names and sensitive subjects.
pub fn phrases(own: &[String], bg: &Background, names: &HashSet<String>) -> Vec<String> {
    if own.len() < 30 {
        return Vec::new();
    }
    let mine = doc_freq(own);
    let n = own.len() as f64;
    let bg_n = bg.messages.max(1) as f64;
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
            if words.iter().any(|w| names.contains(*w)) || message_text::sensitive_area(k).is_some() {
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

fn guild_of(ctx: &Context) -> Option<GuildId> {
    ctx.cache.guilds().first().copied()
}

/// Name lookups from the cache, and every member's name as words (so a name is
/// never picked as someone's catchphrase).
pub(super) fn cache_names(ctx: &Context) -> (HashMap<u64, String>, HashMap<u64, String>, HashSet<String>) {
    let mut channels = HashMap::new();
    let mut members = HashMap::new();
    let mut words = HashSet::new();
    if let Some(guild) = guild_of(ctx).and_then(|g| ctx.cache.guild(g)) {
        for (id, c) in &guild.channels {
            channels.insert(id.get(), c.name.clone());
        }
        for (id, m) in &guild.members {
            let name = m.display_name().to_string();
            for n in [name.as_str(), m.user.name.as_str()] {
                for w in n.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3) {
                    words.insert(w.to_string());
                }
            }
            if !m.user.bot {
                members.insert(id.get(), name);
            }
        }
    }
    (channels, members, words)
}

async fn joined(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64) -> Option<i64> {
    let from_cache = guild_of(ctx)
        .and_then(|g| ctx.cache.guild(g).and_then(|guild| guild.members.get(&UserId::new(user)).and_then(|m| m.joined_at.map(|t| t.unix_timestamp()))));
    let from_log = super::joinlog_get(storage, user)
        .await
        .first_join
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok())
        .map(|t| t.timestamp());
    match (from_cache, from_log) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// The facts about `user`, from the cache of the last few minutes or read now.
pub(super) async fn facts_for(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64, words: HashSet<String>) -> Facts {
    let now = Utc::now().timestamp();
    if let Some(f) = cached(user, now) {
        return f;
    }
    let joined_at = joined(ctx, storage, user).await;
    let f = tokio::task::spawn_blocking(move || read(user, now, joined_at, &words)).await.unwrap_or_default();
    remember(user, now, &f);
    f
}

pub(super) fn display_name(ctx: &Context, user: u64, fallback: &str) -> String {
    guild_of(ctx)
        .and_then(|g| ctx.cache.guild(g).and_then(|guild| guild.members.get(&UserId::new(user)).map(|m| m.display_name().to_string())))
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
