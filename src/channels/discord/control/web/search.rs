//! Search messages: who said something, and when, in what the bot has stored
//! from its channels. Never DMs, never #safe-corner or a thread inside it, and
//! never a channel the server doesn't list (an archived thread could be either).
//!
//! The stored history is read a window at a time, newest first: the (agent,
//! time) index picks the next few thousand rows, SQL keeps the member requests
//! whose JSON could hold the words (`LIKE`, only a prefilter), and the exact
//! case-insensitive match runs here on the text the member wrote. A request
//! stops at the page size or the scan budget and hands back a cursor, so a
//! search over months never holds the history connection for long.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::{Value, json};

use super::super::super::weekly;
use super::{ApiError, ApiResult, Caller, ChannelInfo, Panel, ok, parse_id};

pub const MIN_QUERY: usize = 2;
pub const MAX_QUERY: usize = 100;
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 100;
pub const PERIODS: [(&str, Option<i64>); 5] = [("1", Some(1)), ("7", Some(7)), ("30", Some(30)), ("90", Some(90)), ("all", None)];
/// Stored rows (of every kind) one window covers.
#[cfg(not(test))]
pub const WINDOW_ROWS: usize = 5_000;
#[cfg(test)]
pub const WINDOW_ROWS: usize = 40;
/// Stored rows one request may look through before handing back a cursor.
#[cfg(not(test))]
pub const SCAN_BUDGET: usize = 300_000;
#[cfg(test)]
pub const SCAN_BUDGET: usize = 400;
/// Member messages one request may read in full (the ones the prefilter let through).
pub const PARSE_BUDGET: usize = 20_000;
/// Wall-clock time one request may spend reading.
pub const TIME_BUDGET: Duration = Duration::from_secs(3);
/// A search repeated (its next page, say) within this long isn't logged again.
pub const AUDIT_QUIET: Duration = Duration::from_secs(15 * 60);
const SNIPPET_BEFORE: usize = 80;
const SNIPPET_AFTER: usize = 200;
const REPLY_CHARS: usize = 200;

// --- what a window is asked and gives back ------------------------------------------

#[derive(Clone, Debug)]
pub struct Filter {
    /// The words as typed, trimmed.
    pub needle: String,
    pub member: Option<u64>,
    pub channel: Option<u64>,
    /// Oldest timestamp (ms) the search reaches, inclusive.
    pub since_ms: i64,
    /// Newest timestamp (ms), exclusive: the cursor.
    pub before_ms: i64,
    /// How many stored rows the window covers.
    pub window_rows: usize,
}

/// One stored member message whose text holds the words.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub ts_ms: i64,
    pub author_id: u64,
    /// The name in the stored `user` string, for someone the cache doesn't know.
    pub author_name: String,
    pub channel_id: Option<u64>,
    pub is_dm: bool,
    pub message_id: Option<u64>,
    /// What they wrote: without the mods' notes block or the quoted message.
    pub text: String,
    /// Where the words are in `text`, in characters.
    pub start: usize,
    pub end: usize,
    /// Who they replied to and what that said.
    pub reply: Option<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct Window {
    pub hits: Vec<Hit>,
    /// Stored rows the window covered (about; the last window counts what it had).
    pub rows: usize,
    /// Member messages the prefilter let through and were read.
    pub parsed: usize,
    /// Everything at or after this timestamp (ms) has been looked at.
    pub next_before_ms: i64,
    /// Nothing older is left in the period.
    pub done: bool,
}

// --- matching --------------------------------------------------------------------------

/// Where `needle` first appears in `hay`, ignoring case (Unicode lowercase), as
/// character offsets into `hay`. A match never starts or ends inside a
/// character whose lowercase is longer than one character.
pub fn find_ci(hay: &str, needle: &str) -> Option<(usize, usize)> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return None;
    }
    let mut folded: Vec<(char, usize)> = Vec::with_capacity(hay.len());
    for (i, c) in hay.chars().enumerate() {
        folded.extend(c.to_lowercase().map(|l| (l, i)));
    }
    if folded.len() < needle.len() {
        return None;
    }
    'next: for s in 0..=folded.len() - needle.len() {
        if folded[s].0 != needle[0] {
            continue;
        }
        for (k, n) in needle.iter().enumerate().skip(1) {
            if folded[s + k].0 != *n {
                continue 'next;
            }
        }
        let start = folded[s].1;
        let last = folded[s + needle.len() - 1].1;
        let splits_start = s > 0 && folded[s - 1].1 == start;
        let splits_end = folded.get(s + needle.len()).is_some_and(|f| f.1 == last);
        if !splits_start && !splits_end {
            return Some((start, last + 1));
        }
    }
    None
}

/// Escapes `%`, `_` and `\` for a `LIKE … ESCAPE '\'` pattern.
pub fn like_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 4);
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The `LIKE` prefilter for the words, matched against the stored JSON. SQLite
/// ignores case only for ASCII, so the pattern uses the longest run of
/// characters that are ASCII or have no case at all (Hindi, emoji), written as
/// the JSON has them. `None` when no such run is left: no prefilter.
pub fn like_pattern(needle: &str) -> Option<String> {
    let caseless = |c: char| c.is_ascii() || (c.to_lowercase().eq(std::iter::once(c)) && c.to_uppercase().eq(std::iter::once(c)));
    let best = needle
        .split(|c: char| !caseless(c))
        .fold("", |best, run| if run.chars().count() > best.chars().count() { run } else { best });
    if best.is_empty() {
        return None;
    }
    let json = serde_json::to_string(best).ok()?;
    let inner = &json[1..json.len() - 1];
    Some(format!("%{}%", like_escape(inner)))
}

/// `(start, end)` char offsets shown as a short piece of the text around them,
/// with the offsets moved into that piece.
pub fn snippet(text: &str, start: usize, end: usize) -> (String, usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let from = start.saturating_sub(SNIPPET_BEFORE);
    let to = (end + SNIPPET_AFTER).min(chars.len());
    let mut out = String::new();
    let mut shift = from;
    if from > 0 {
        out.push('…');
        shift = from.saturating_sub(1);
    }
    out.extend(chars[from..to].iter().map(|c| if *c == '\n' { ' ' } else { *c }));
    if to < chars.len() {
        out.push('…');
    }
    (out, start - shift, end - shift)
}

/// The name and Discord id in a stored `user` string, "@Name (DiscordId: 123)".
pub fn parse_user(user: &str) -> Option<(String, u64)> {
    let (name, rest) = user.rsplit_once(" (DiscordId: ")?;
    let id = rest.strip_suffix(')')?.parse().ok()?;
    Some((name.trim().trim_start_matches('@').to_string(), id))
}

/// One stored request as a hit, when it's a member message whose own words
/// hold the needle (and it's from the member asked for, when one is).
pub fn parse_hit(channel: &str, ts_ms: i64, data: &str, needle: &str, member: Option<u64>) -> Option<Hit> {
    let v: Value = serde_json::from_str(data).ok()?;
    let req = v.get("content")?.get("Request")?;
    let (author_name, author_id) = parse_user(req.get("user")?.as_str()?)?;
    if member.is_some_and(|m| m != author_id) {
        return None;
    }
    let raw = req.get("content")?.as_object()?.values().next()?.as_str()?;
    let text = super::super::profiles::own_words(raw);
    let (start, end) = find_ci(&text, needle)?;
    let meta = req.get("metadata");
    let field = |k: &str| meta.and_then(|m| m.get(k));
    let id_of = |v: Option<&Value>| v.and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_u64()));
    let is_dm = field("is_dm").and_then(Value::as_bool).unwrap_or(false);
    let channel_id = id_of(field("discord_channel_id"))
        .or_else(|| channel.strip_prefix("discord__").and_then(|r| r.split("__").next()).and_then(|c| c.parse().ok()));
    let message_id = id_of(field("message_id")).or_else(|| req.get("platform_message_id").and_then(|p| p.get("discord")).and_then(Value::as_u64));
    let reply = field("is_reply_message").and_then(Value::as_bool).unwrap_or(false).then(|| {
        let author = field("replied_message_author").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let said = field("replied_message_content").and_then(Value::as_str).unwrap_or("").replace('\n', " ");
        let said = said.trim();
        let short: String = said.chars().take(REPLY_CHARS).collect();
        (author, if said.chars().count() > REPLY_CHARS { format!("{}…", short) } else { short })
    });
    Some(Hit { ts_ms, author_id, author_name, channel_id, is_dm, message_id, text, start, end, reply })
}

// --- reading a window ------------------------------------------------------------------------

/// Reads one window of the stored history, newest first.
pub fn read_window(conn: &Connection, agent_id: &str, f: &Filter) -> rusqlite::Result<Window> {
    // The oldest timestamp of the next `window_rows` rows; the window takes every
    // row at that timestamp too, so the cursor never splits a millisecond.
    let edge: Option<i64> = conn
        .query_row(
            "SELECT timestamp FROM session_history INDEXED BY idx_sh_agent_time
             WHERE agent_id = ?1 AND timestamp < ?2 AND timestamp >= ?3
             ORDER BY timestamp DESC LIMIT 1 OFFSET ?4",
            params![agent_id, f.before_ms, f.since_ms, f.window_rows.saturating_sub(1) as i64],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| if matches!(e, rusqlite::Error::QueryReturnedNoRows) { Ok(None) } else { Err(e) })?;
    let (lower, done) = match edge {
        Some(t) => (t, false),
        None => (f.since_ms, true),
    };
    let text = like_pattern(&f.needle);
    let member = f.member.map(|m| format!("%(DiscordId: {})%", m));
    let (channel_exact, channel_topic) = match f.channel {
        Some(c) => (Some(format!("discord__{}", c)), Some(format!("{}%", like_escape(&format!("discord__{}__", c))))),
        None => (None, None),
    };
    let mut stmt = conn.prepare_cached(
        "SELECT channel, timestamp, data FROM session_history INDEXED BY idx_sh_agent_time
         WHERE agent_id = ?1 AND timestamp < ?2 AND timestamp >= ?3 AND content_type = 'Request'
           AND (?4 IS NULL OR data LIKE ?4 ESCAPE '\\')
           AND (?5 IS NULL OR data LIKE ?5)
           AND (?6 IS NULL OR channel = ?6 OR channel LIKE ?7 ESCAPE '\\')
         ORDER BY timestamp DESC",
    )?;
    let rows = stmt.query_map(params![agent_id, f.before_ms, lower, text, member, channel_exact, channel_topic], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?))
    })?;
    let mut out = Window { next_before_ms: lower, done, rows: f.window_rows, ..Default::default() };
    for row in rows {
        let (channel, ts, data) = row?;
        out.parsed += 1;
        if let Some(hit) = parse_hit(&channel, ts, &data, &f.needle, f.member) {
            if f.channel.is_none_or(|c| hit.channel_id == Some(c)) {
                out.hits.push(hit);
            }
        }
    }
    if done {
        out.rows = conn.query_row(
            "SELECT COUNT(*) FROM session_history INDEXED BY idx_sh_agent_time WHERE agent_id = ?1 AND timestamp < ?2 AND timestamp >= ?3",
            params![agent_id, f.before_ms, f.since_ms],
            |r| r.get::<_, i64>(0),
        )? as usize;
    }
    Ok(out)
}

// --- where a message may be shown ----------------------------------------------------------------

/// Where a hit was posted, when it may be shown: a channel the server lists, or
/// a thread the cache knows inside one. Never a DM or #safe-corner.
pub struct Place {
    pub channel: u64,
    pub name: String,
    pub thread: bool,
}

pub fn place_of(hit: &Hit, channels: &[ChannelInfo], sensitive: &[u64], parent_of: &dyn Fn(u64) -> Option<u64>) -> Option<Place> {
    if hit.is_dm {
        return None;
    }
    let channel = hit.channel_id?;
    let find = |id: u64| channels.iter().find(|c| c.id == id.to_string());
    let (listed, thread) = match find(channel) {
        Some(c) => (c, false),
        None => (find(parent_of(channel)?)?, true),
    };
    let listed_id: u64 = listed.id.parse().ok()?;
    let safe = |id: u64, name: &str| sensitive.contains(&id) || weekly::is_safe_corner(id, name);
    if safe(channel, &listed.name) || safe(listed_id, &listed.name) {
        return None;
    }
    Some(Place { channel, name: listed.name.clone(), thread })
}

// --- the endpoint ----------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    days: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<String>,
}

static LOGGED: LazyLock<Mutex<HashMap<(u64, String), Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Writes the search to the activity log, unless this admin ran the very same
/// search a few minutes ago (loading its next page, say).
fn log_search(user: u64, value: &str) {
    log_quietly("messages:search", user, value);
}

/// Writes a look at stored messages to the activity log under `audit_key`,
/// unless this admin made the very same one within [`AUDIT_QUIET`].
pub fn log_quietly(audit_key: &str, user: u64, value: &str) {
    let key = (user, format!("{}\u{1f}{}", audit_key, value));
    {
        let mut logged = LOGGED.lock();
        logged.retain(|_, at| at.elapsed() < AUDIT_QUIET);
        if logged.contains_key(&key) {
            return;
        }
        logged.insert(key, Instant::now());
    }
    if let Err(err) = super::super::log_change(audit_key, None, Some(value), user) {
        tracing::warn!("panel: couldn't log a look at messages ({}): {}", audit_key, err);
    }
}

pub async fn search(State(panel): State<Panel>, axum::Extension(Caller(user)): axum::Extension<Caller>, Query(q): Query<SearchQuery>) -> ApiResult {
    let needle = q.q.as_deref().unwrap_or("").trim().to_string();
    let len = needle.chars().count();
    if len < MIN_QUERY {
        return Err(ApiError::bad(format!("Type at least {} characters to search.", MIN_QUERY)));
    }
    if len > MAX_QUERY {
        return Err(ApiError::bad(format!("Keep the search under {} characters.", MAX_QUERY)));
    }
    let blank = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty()).map(String::from);
    let member = match blank(&q.member) {
        Some(raw) => Some(parse_id(&raw).ok_or_else(|| ApiError::bad("That isn't a member id."))?),
        None => None,
    };
    let channels = panel.data.channels();
    let sensitive = panel.data.sensitive_channels();
    let channel = match blank(&q.channel) {
        Some(raw) => {
            let id = parse_id(&raw).ok_or_else(|| ApiError::bad("That isn't a channel id."))?;
            let name = channels.iter().find(|c| c.id == id.to_string()).map(|c| c.name.as_str()).unwrap_or("");
            if sensitive.contains(&id) || weekly::is_safe_corner(id, name) {
                return Err(ApiError::bad("#safe-corner is never searched."));
            }
            Some(id)
        }
        None => None,
    };
    let days_key = blank(&q.days).unwrap_or_else(|| "30".to_string());
    let days = PERIODS
        .iter()
        .find(|(k, _)| *k == days_key)
        .map(|(_, d)| *d)
        .ok_or_else(|| ApiError::bad("The period is 1, 7, 30, 90 or all (days)."))?;
    let before = match blank(&q.before) {
        Some(raw) => Some(raw.parse::<i64>().ok().filter(|b| *b > 0).ok_or_else(|| ApiError::bad("That isn't a place to carry on from."))?),
        None => None,
    };
    let limit = match blank(&q.limit) {
        Some(raw) => raw.parse::<usize>().map_err(|_| ApiError::bad("The limit is a number."))?.clamp(1, MAX_LIMIT),
        None => DEFAULT_LIMIT,
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    let since_ms = days.map(|d| now_ms - d * 86_400_000).unwrap_or(0);

    let member_name = match member {
        Some(id) => Some(match panel.data.cached_member(id) {
            Some(m) => m.name,
            None => panel.data.member(id).await.map(|m| m.name).unwrap_or_else(|| id.to_string()),
        }),
        None => None,
    };
    let channel_name = channel.map(|c| channels.iter().find(|x| x.id == c.to_string()).map(|x| format!("#{}", x.name)).unwrap_or_else(|| format!("channel {}", c)));
    let audit = format!(
        "“{}” · {} · {} · {}",
        needle,
        member_name.as_ref().map(|n| format!("@{}", n)).unwrap_or_else(|| "any member".into()),
        channel_name.clone().unwrap_or_else(|| "all channels".into()),
        match days {
            Some(1) => "last day".to_string(),
            Some(d) => format!("last {} days", d),
            None => "all time".to_string(),
        }
    );
    log_search(user, &audit);

    let started = Instant::now();
    let mut cursor = before.unwrap_or(i64::MAX).min(now_ms + 60_000);
    let (mut rows, mut parsed) = (0usize, 0usize);
    let mut found: Vec<(Hit, Place)> = Vec::new();
    let mut complete = false;
    loop {
        let filter = Filter { needle: needle.clone(), member, channel, since_ms, before_ms: cursor, window_rows: WINDOW_ROWS };
        let window = panel.data.search_window(filter).await.map_err(|e| {
            tracing::warn!("panel: message search failed: {}", e);
            ApiError(axum::http::StatusCode::SERVICE_UNAVAILABLE, "The stored messages can't be read right now. Try again in a moment.".into())
        })?;
        rows += window.rows;
        parsed += window.parsed;
        for hit in window.hits {
            if let Some(place) = place_of(&hit, &channels, &sensitive, &|c| panel.data.thread_parent(c)) {
                found.push((hit, place));
            }
        }
        cursor = window.next_before_ms;
        if window.done {
            complete = true;
            break;
        }
        if found.len() >= limit || rows >= SCAN_BUDGET || parsed >= PARSE_BUDGET || started.elapsed() >= TIME_BUDGET {
            break;
        }
    }
    if found.len() > limit {
        // Keep whole milliseconds: the cursor is exclusive.
        let edge = found[limit - 1].0.ts_ms;
        let keep = found.iter().take_while(|(h, _)| h.ts_ms >= edge).count();
        found.truncate(keep);
        cursor = edge;
        complete = false;
    }

    // House lookups read the house store: off the async threads.
    let ids: Vec<u64> = found.iter().map(|(h, _)| h.author_id).collect::<HashSet<_>>().into_iter().collect();
    let data = panel.data.clone();
    let houses: HashMap<u64, &'static super::super::super::house::House> =
        tokio::task::spawn_blocking(move || ids.into_iter().filter_map(|id| data.member_house(id).map(|h| (id, h))).collect()).await.unwrap_or_default();
    let guild = panel.data.guild().map(|g| g.id);
    let results: Vec<Value> = found
        .iter()
        .map(|(hit, place)| {
            let who = panel.data.cached_member(hit.author_id);
            let house = houses.get(&hit.author_id);
            let (snip, s_start, s_end) = snippet(&hit.text, hit.start, hit.end);
            json!({
                "ts": hit.ts_ms / 1000,
                "ts_ms": hit.ts_ms,
                "member": {
                    "id": hit.author_id.to_string(),
                    "name": who.as_ref().map(|m| m.name.clone()).unwrap_or_else(|| hit.author_name.clone()),
                    "avatar": who.as_ref().map(|m| m.avatar.clone()),
                    "bot": who.as_ref().is_some_and(|m| m.bot),
                },
                "house": house.map(|h| json!({ "key": h.key, "name": h.name, "crest": h.crest, "colour": format!("#{:06x}", h.colour) })),
                "channel": { "id": place.channel.to_string(), "name": place.name, "thread": place.thread },
                "text": hit.text,
                "match": [hit.start, hit.end],
                "snippet": { "text": snip, "start": s_start, "end": s_end },
                "reply_to": hit.reply.as_ref().map(|(author, text)| json!({ "author": author, "text": text })),
                "url": match (&guild, hit.message_id) {
                    (Some(g), Some(m)) => Some(jump_url(g, place.channel, m)),
                    _ => None,
                },
            })
        })
        .collect();
    let next_before = (!complete).then_some(cursor);
    ok(json!({
        "q": needle,
        "member": member.map(|m| json!({ "id": m.to_string(), "name": member_name })),
        "channel": channel.map(|c| json!({ "id": c.to_string(), "name": channel_name.map(|n| n.trim_start_matches('#').to_string()) })),
        "days": days_key,
        "limit": limit,
        "count": results.len(),
        "results": results,
        "next_before": next_before,
        "scanned_to": if complete { days.map(|_| since_ms / 1000) } else { Some(cursor / 1000) },
        "complete": complete,
        "rows_scanned": rows,
    }))
}

pub fn jump_url(guild: &str, channel: u64, message: u64) -> String {
    format!("https://discord.com/channels/{}/{}/{}", guild, channel, message)
}

/// How a search reads in the activity log.
pub fn audit_entry(e: &super::super::AuditEntry) -> serde_json::Map<String, Value> {
    let mut obj = serde_json::Map::new();
    obj.insert("label".into(), json!("Searched messages"));
    obj.insert("section".into(), json!({ "id": "search", "title": "Search messages", "icon": "🔎" }));
    obj.insert("change".into(), json!(e.new.clone().unwrap_or_else(|| "Searched".into())));
    obj.insert("old".into(), Value::Null);
    obj.insert("new".into(), Value::Null);
    obj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns_escape_wildcards_and_json() {
        assert_eq!(like_escape(r"50%_off\now"), r"50\%\_off\\now");
        assert_eq!(like_pattern("koto").as_deref(), Some("%koto%"));
        assert_eq!(like_pattern("100%").as_deref(), Some(r"%100\%%"));
        assert_eq!(like_pattern("a_b").as_deref(), Some(r"%a\_b%"));
        // The JSON has a quote as \" and a backslash as \\; both then escaped for LIKE.
        assert_eq!(like_pattern(r#"say "hi""#).as_deref(), Some(r#"%say \\"hi\\"%"#));
        assert_eq!(like_pattern(r"C:\x").as_deref(), Some(r"%C:\\\\x%"));
        // Letters with case outside ASCII can't be matched by LIKE: the longest other run is used.
        assert_eq!(like_pattern("Café crème").as_deref(), Some("%Caf%"));
        assert_eq!(like_pattern("éÉ"), None);
        // Caseless scripts go in whole.
        assert_eq!(like_pattern("नमस्ते दोस्त").as_deref(), Some("%नमस्ते दोस्त%"));
    }

    #[test]
    fn matching_ignores_case_in_any_script() {
        assert_eq!(find_ci("Koto was HARD today", "hard"), Some((9, 13)));
        assert_eq!(find_ci("ÉCLAIR at the Café", "éclair"), Some((0, 6)));
        assert_eq!(find_ci("the café", "CAFÉ"), Some((4, 8)));
        assert_eq!(find_ci("ΣΟΦΙΑ", "σοφια"), Some((0, 5)));
        assert_eq!(find_ci("bhai 💀 kya", "💀 KYA"), Some((5, 10)));
        assert_eq!(find_ci("nothing here", "koto"), None);
        assert_eq!(find_ci("x", ""), None);
        // İ lowercases to two characters: a match can't take only half of it.
        assert_eq!(find_ci("İstanbul", "i̇stan"), Some((0, 5)));
        assert_eq!(find_ci("İstanbul", "\u{307}stan"), None);
    }

    #[test]
    fn snippets_keep_the_match_offsets() {
        let text = format!("{}needle{}", "a".repeat(300), "b".repeat(300));
        let (s, start, end) = snippet(&text, 300, 306);
        let chars: Vec<char> = s.chars().collect();
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert_eq!(chars[start..end].iter().collect::<String>(), "needle");
        let (s, start, end) = snippet("short\nnéedle here", 6, 12);
        assert_eq!(s, "short néedle here");
        assert_eq!(s.chars().skip(start).take(end - start).collect::<String>(), "néedle");
    }

    #[test]
    fn stored_users_give_a_name_and_id() {
        assert_eq!(parse_user("@Riya (DiscordId: 42)"), Some(("Riya".into(), 42)));
        assert_eq!(parse_user("@a (b) (DiscordId: 7)"), Some(("a (b)".into(), 7)));
        assert_eq!(parse_user("@nobody"), None);
    }

    #[test]
    fn hits_carry_the_reply_and_ids_and_leave_out_quotes() {
        let data = json!({ "content": { "Request": {
            "user": "@Riya (DiscordId: 42)",
            "content": { "silent_read": "[replying to @Dev: \"koto is easy\"]\nno KOTO is hard" },
            "platform_message_id": { "discord": 9001 },
            "metadata": { "discord_channel_id": "555", "is_dm": false, "is_reply_message": true,
                "replied_message_author": "Dev", "replied_message_content": "koto is\neasy", "message_id": "9001" }
        }}})
        .to_string();
        let hit = parse_hit("discord__555", 1_789_000_000_123, &data, "koto", None).unwrap();
        assert_eq!(hit.text, "no KOTO is hard");
        assert_eq!((hit.start, hit.end), (3, 7));
        assert_eq!((hit.author_id, hit.author_name.as_str(), hit.channel_id, hit.message_id), (42, "Riya", Some(555), Some(9001)));
        assert_eq!(hit.reply, Some(("Dev".into(), "koto is easy".into())));
        // Words only in the quoted message are someone else's.
        assert!(parse_hit("discord__555", 0, &data, "easy", None).is_none());
        assert!(parse_hit("discord__555", 0, &data, "koto", Some(43)).is_none());
    }

    #[test]
    fn windows_never_split_a_millisecond() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_history (uid TEXT PRIMARY KEY, agent_id TEXT NOT NULL, channel TEXT NOT NULL, topic TEXT,
                 timestamp INTEGER NOT NULL, content_type TEXT NOT NULL, data TEXT NOT NULL);
             CREATE INDEX idx_sh_agent_time ON session_history(agent_id, timestamp);",
        )
        .unwrap();
        let add = |uid: &str, ts: i64, text: &str| {
            let data = json!({ "content": { "Request": { "user": "@a (DiscordId: 5)", "content": { "chat": text }, "metadata": { "discord_channel_id": "9" } } } }).to_string();
            conn.execute("INSERT INTO session_history VALUES (?1, 'lodu', 'discord__9', NULL, ?2, 'Request', ?3)", params![uid, ts, data]).unwrap();
        };
        add("a", 3000, "koto one");
        for (i, uid) in ["b", "c", "d", "e"].iter().enumerate() {
            add(uid, 2000, &format!("koto tie {}", i));
        }
        add("f", 1000, "koto last");
        let mut f = Filter { needle: "KOTO".into(), member: None, channel: None, since_ms: 0, before_ms: i64::MAX, window_rows: 2 };
        let first = read_window(&conn, "lodu", &f).unwrap();
        // Two rows reach back to 2000, and every row at 2000 comes with them.
        assert_eq!(first.hits.len(), 5);
        assert_eq!((first.next_before_ms, first.done), (2000, false));
        f.before_ms = first.next_before_ms;
        let second = read_window(&conn, "lodu", &f).unwrap();
        assert_eq!(second.hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(), vec!["koto last"]);
        assert!(second.done);
        assert_eq!(second.rows, 1);
    }

    #[test]
    fn jump_urls_point_at_the_message() {
        assert_eq!(jump_url("900", 21, 1234), "https://discord.com/channels/900/21/1234");
    }
}
