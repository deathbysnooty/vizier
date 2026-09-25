//! The daily topic pass: once a night, what each member actually talked about.
//!
//! The server is about 180 active people and twenty-odd thousand messages a
//! day. Asking the model about each of them in turn would be 180 calls; this
//! reads the day once instead. The day is cut channel by channel into chunks
//! that fit a token budget, each chunk carries the messages with the name in
//! front, and the model comes back with a few short tags per person and one
//! plain line about their day. One pass over the day, whatever the server did.
//!
//! What is kept is deliberately thin: per member per day, a handful of tags, one
//! line, how many messages and which channels. No message text, ever. Weeks of
//! it stack up into "what they have been talking about lately", which is the
//! point.
//!
//! Two gates stand between a day and an entry. A **floor** — usable messages
//! and real characters, both settings — so somebody who dropped four "lol"s and
//! a sticker gets no entry at all rather than an invented one; `usable` is
//! `notes_build`'s, so bot commands, one-word replies and repeats do not count.
//! And a **filter** on the way out, which throws away a tag or a line touching
//! sexuality, gender, religion, caste, health or family, anything quoted, and
//! filler like "chatting" that says nothing. A rejected tag is dropped; a day
//! left with nothing is no entry, which the panel reads as "nothing much".
//!
//! Nothing here ever sees #safe-corner: the messages come from `msglog`, which
//! never kept a word of it. The store is `topics_store`, the nightly job is
//! `topics_job`, and the page is `control/web/topics.rs`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::control;
use super::kalesh::Reply;
use super::notes_build;

// --- settings ---------------------------------------------------------------------------------

/// Off stops the nightly pass. Days already recorded are still shown.
pub fn topics_on() -> bool {
    control::on("VIZIER_TOPICS", true)
}

/// The hour of the India day the pass runs at. It reads *yesterday*, so it wants
/// to be after midnight and before anybody is about.
pub fn run_hour() -> u32 {
    control::number("VIZIER_TOPICS_HOUR", 5).clamp(0, 23) as u32
}

/// How big one chunk of the day may be, in tokens of messages.
pub fn chunk_tokens() -> usize {
    control::number("VIZIER_TOPICS_CHUNK_SIZE", 12_000).clamp(1_000, 60_000) as usize
}

/// Chunks one night's pass may send at all: the cap on the work. A day that
/// needs more than this loses its quietest channels, never its busiest.
pub fn max_chunks() -> usize {
    control::number("VIZIER_TOPICS_MAX_CHUNKS", 40).clamp(0, 500) as usize
}

/// Usable messages a member must have said that day before they can get an
/// entry at all.
pub fn min_messages() -> usize {
    control::number("VIZIER_TOPICS_MIN_MESSAGES", 5).clamp(1, 500) as usize
}

/// Characters of real text, across those usable messages, before they can get an
/// entry at all. Five "lol"s clear the message floor and not this one.
pub fn min_chars() -> usize {
    control::number("VIZIER_TOPICS_MIN_CHARS", 120).clamp(0, 20_000) as usize
}

/// The model the pass uses. Its own setting so the whole day can go to a cheap
/// one; empty falls back to the summary model, which falls back to the bot's own.
pub fn topics_model() -> Option<String> {
    control::var("VIZIER_TOPICS_MODEL").or_else(super::kalesh::summary_model)
}

/// Tags kept for one member for one day.
pub const MAX_TAGS: usize = 5;
/// One tag's length.
pub const TAG_CHARS: usize = 40;
/// The one line's length.
pub const LINE_CHARS: usize = 200;
/// One message's length in a chunk.
pub const MESSAGE_CHARS: usize = 240;
/// Days of entries a member's own history shows at most.
pub const HISTORY_DAYS: i64 = 60;

/// The floors and caps one pass runs under, so a test can set its own.
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub chunk_tokens: usize,
    pub max_chunks: usize,
    pub min_messages: usize,
    pub min_chars: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { chunk_tokens: 12_000, max_chunks: 40, min_messages: 5, min_chars: 120 }
    }
}

/// The settings as the panel has them now.
pub fn settings() -> Settings {
    Settings { chunk_tokens: chunk_tokens(), max_chunks: max_chunks(), min_messages: min_messages(), min_chars: min_chars() }
}

// --- what the pass reads -------------------------------------------------------------------------

/// One message of the day, as the pass reads it. No ids beyond the author's, no
/// attachments, no reply chain: a name, a place, and the words.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub author_id: u64,
    pub author_name: String,
    pub channel_id: u64,
    pub channel_name: String,
    pub ts: i64,
    pub text: String,
}

/// What one member said that day once the thin messages are out: the floor is
/// read off this, and so is the message count an entry carries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Substance {
    /// Messages of theirs the log kept, thin ones included.
    pub messages: usize,
    /// Of those, the ones with something in them, each distinct text once.
    pub usable: usize,
    /// Characters across those usable messages.
    pub chars: usize,
    /// Where they said it, busiest first.
    pub channels: Vec<u64>,
    /// The usable texts themselves, for the quote check on the way out.
    pub texts: Vec<String>,
}

/// Every member of the day, with what they actually said. `usable` is
/// `notes_build`'s: a bot command, a one-word reply, or the same line again is
/// not somebody talking.
pub fn substance(day: &[Said]) -> HashMap<u64, Substance> {
    let mut out: HashMap<u64, Substance> = HashMap::new();
    let mut seen: HashMap<u64, HashSet<String>> = HashMap::new();
    let mut places: HashMap<u64, HashMap<u64, usize>> = HashMap::new();
    for m in day {
        let s = out.entry(m.author_id).or_default();
        s.messages += 1;
        *places.entry(m.author_id).or_default().entry(m.channel_id).or_insert(0) += 1;
        let Some(text) = notes_build::usable(&m.text) else { continue };
        if !seen.entry(m.author_id).or_default().insert(text.to_lowercase()) {
            continue;
        }
        s.usable += 1;
        s.chars += text.chars().count();
        s.texts.push(text);
    }
    for (user, s) in out.iter_mut() {
        let mut top: Vec<(u64, usize)> = places.remove(user).unwrap_or_default().into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        s.channels = top.into_iter().map(|(c, _)| c).collect();
    }
    out
}

/// Whether this member's day is worth recording at all. Both floors, not
/// either: a hundred one-word replies is a busy day and an empty one.
pub fn eligible(s: &Substance, set: &Settings) -> bool {
    s.usable >= set.min_messages && s.chars >= set.min_chars
}

// --- cutting the day into chunks -------------------------------------------------------------

/// One slice of one channel's day, by position in the day's messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    pub channel_id: u64,
    pub channel_name: String,
    /// Indexes into the day, in time order.
    pub lines: Vec<usize>,
}

/// One message's cost in a chunk: the words, the name in front, and the marks.
fn cost(m: &Said) -> usize {
    notes_build::estimate_tokens(&notes_build::cut(&m.text, MESSAGE_CHARS)) + notes_build::estimate_tokens(&m.author_name) + 4
}

/// The day cut channel by channel into chunks that fit `chunk_tokens`. Busiest
/// channel first, and in time order inside each, so a chunk reads as a stretch
/// of one conversation rather than a shuffle.
///
/// `max_chunks` is the cap on one night's work: when the day needs more than
/// that, the quietest channels fall off the end rather than every channel losing
/// its tail. Messages with nothing in them are left out of the chunks entirely —
/// they cost tokens and say nothing.
pub fn chunks(day: &[Said], set: &Settings) -> Vec<Chunk> {
    let mut by_channel: Vec<(u64, String, Vec<usize>)> = Vec::new();
    for (i, m) in day.iter().enumerate() {
        if notes_build::usable(&m.text).is_none() {
            continue;
        }
        match by_channel.iter_mut().find(|(c, _, _)| *c == m.channel_id) {
            Some((_, _, list)) => list.push(i),
            None => by_channel.push((m.channel_id, m.channel_name.clone(), vec![i])),
        }
    }
    // The busiest channel's chunks are the ones worth paying for first.
    by_channel.sort_by(|a, b| b.2.len().cmp(&a.2.len()).then(a.0.cmp(&b.0)));
    let mut out: Vec<Chunk> = Vec::new();
    for (channel_id, channel_name, mut list) in by_channel {
        list.sort_by_key(|i| (day[*i].ts, *i));
        let mut lines: Vec<usize> = Vec::new();
        let mut used = 0usize;
        for i in list {
            let c = cost(&day[i]);
            if !lines.is_empty() && used + c > set.chunk_tokens {
                out.push(Chunk { channel_id, channel_name: channel_name.clone(), lines: std::mem::take(&mut lines) });
                used = 0;
            }
            lines.push(i);
            used += c;
        }
        if !lines.is_empty() {
            out.push(Chunk { channel_id, channel_name, lines });
        }
    }
    out.truncate(set.max_chunks);
    out
}

// --- the prompt --------------------------------------------------------------------------------

/// The instructions every chunk is read under. Kept whole so the tests can hold
/// it to its promises.
pub const INSTRUCTIONS: &str = "\
You are helping the moderators of MLCI, an Indian Discord server, keep a light record of what their members talk \
about. Below is part of one day in one channel. Say, for each person, what they were talking about.

About this server: members write in Hinglish - Hindi and English mixed, mostly in Roman script, full of slang - and \
roast each other constantly (bakchodi). Read it the way a member of the server would. Gaalis used as punctuation, \
friendly abuse and loud banter are normal here and mean nothing by themselves.

For each person who said something worth recording, give:
- up to 5 short topic tags: one to three words each, the plain subject of what they were on about. \
\"cricket\", \"college exams\", \"anime\", \"the house cup\", \"valorant\", \"chess\", \"food\", \"music\". \
A tag is a subject, never a person, never a judgement, never a sentence.
- one plain line about their day in this channel: what they were doing, in under twenty words.

HARD RULES - anything that breaks one of them is thrown away:
1. Never record or infer anything about anyone's sexuality, gender, religion, caste, health or mental health, or \
family situation. Leave these out even when they are talked about openly and at length. If somebody spent the whole \
day discussing one of these, they simply get no tags for it.
2. Never quote anyone. Not a phrase, not a few words. Paraphrase, in your own words.
3. Nothing about a named third party's private business: do not record what somebody said about somebody else's \
life.
4. Describe, do not judge. No opinions about anyone, no guesses at their character, mood or motives.

If a person's messages here do not amount to anything worth recording - a few one-word replies, reactions, spam, \
a game command over and over - leave that person out entirely. Do not invent a topic to have something to say, and \
never use filler tags like \"chatting\", \"general conversation\", \"random\" or \"talking\". An empty list is the \
right answer for a quiet channel.

Reply with JSON only, in exactly this shape:
{\"people\": [{\"name\": \"exactly the name as it appears below\", \"topics\": [\"cricket\", \"the house cup\"], \
\"line\": \"one plain line about their day here\"}]}";

/// "Monday 21 Sep", India time: what the prompt calls the day.
pub fn day_words(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| d.format("%A %-d %B %Y").to_string()).unwrap_or_else(|_| day.to_string())
}

/// "21:04", India time.
fn clock(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|t| t.with_timezone(&chrono::FixedOffset::east_opt(5 * 3600 + 1800).expect("IST")).format("%H:%M").to_string())
        .unwrap_or_default()
}

/// The whole prompt for one chunk, and the names it may answer about.
pub fn build_prompt(day_label: &str, chunk: &Chunk, day: &[Said]) -> String {
    let mut who: Vec<String> = Vec::new();
    for i in &chunk.lines {
        let name = &day[*i].author_name;
        if !who.contains(name) {
            who.push(name.clone());
        }
    }
    let mut text = String::with_capacity(INSTRUCTIONS.len() + chunk.lines.len() * 80);
    text.push_str(INSTRUCTIONS);
    text.push_str("\n\n---\n\n");
    text.push_str(&format!("Channel: #{}. Day: {}. Times are India time (IST).\n", chunk.channel_name, day_label));
    text.push_str(&format!("The people who spoke here: {}.\n", who.join(", ")));
    text.push_str("Use these names exactly. Never answer about anybody who is not in that list.\n\nMessages:\n");
    for i in &chunk.lines {
        let m = &day[*i];
        text.push_str(&format!("[{}] {}: {}\n", clock(m.ts), m.author_name, notes_build::cut(&m.text, MESSAGE_CHARS)));
    }
    text
}

// --- reading the answer -------------------------------------------------------------------------

/// What the model said about one person in one chunk, before any filtering.
#[derive(Clone, Debug, PartialEq)]
pub struct About {
    pub user_id: u64,
    pub topics: Vec<String>,
    pub line: String,
    /// How many of the chunk's messages were theirs: which chunk's line wins.
    pub weight: usize,
}

static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));

fn tidy(raw: &str) -> String {
    let t = SPACES.replace_all(raw.trim(), " ");
    t.trim_matches(|c: char| c == '"' || c == '“' || c == '”' || c == '.' || c == ',').trim().to_string()
}

/// Who a chunk may be answered about: the names in it, each to one member.
/// A name two members share is dropped — a tag on the wrong person is worse
/// than no tag — and so is a name the chunk never had.
pub fn roster(chunk: &Chunk, day: &[Said]) -> HashMap<String, u64> {
    let mut by_name: HashMap<String, HashSet<u64>> = HashMap::new();
    let mut weight: HashMap<u64, usize> = HashMap::new();
    for i in &chunk.lines {
        let m = &day[*i];
        by_name.entry(m.author_name.trim().to_lowercase()).or_default().insert(m.author_id);
        *weight.entry(m.author_id).or_insert(0) += 1;
    }
    by_name.into_iter().filter(|(_, ids)| ids.len() == 1).map(|(name, ids)| (name, *ids.iter().next().expect("one"))).collect()
}

/// The model's answer for one chunk: only people the chunk actually had, with
/// the tags and the line as written. Nothing is judged here — the filter is a
/// separate step, so a test can see what came back before it was cleaned.
pub fn parse_chunk(raw: &str, roster: &HashMap<String, u64>, day: &[Said], chunk: &Chunk) -> Option<Vec<About>> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let v: Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let people = v.as_object()?.get("people")?.as_array()?.clone();
    let mut weight: HashMap<u64, usize> = HashMap::new();
    for i in &chunk.lines {
        *weight.entry(day[*i].author_id).or_insert(0) += 1;
    }
    let mut out: Vec<About> = Vec::new();
    for p in people {
        let Some(name) = p.get("name").and_then(Value::as_str) else { continue };
        let Some(user_id) = roster.get(&name.trim().to_lowercase()).copied() else { continue };
        if out.iter().any(|a| a.user_id == user_id) {
            continue;
        }
        let topics: Vec<String> = p
            .get("topics")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(Value::as_str).map(|s| notes_build::cut(&tidy(s), TAG_CHARS)).filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let line = notes_build::cut(&tidy(p.get("line").and_then(Value::as_str).unwrap_or("")), LINE_CHARS);
        out.push(About { user_id, topics, line, weight: weight.get(&user_id).copied().unwrap_or(0) });
    }
    Some(out)
}

// --- the filter ---------------------------------------------------------------------------------

/// The areas a topic may never touch, however openly they were discussed.
/// The word lists are `notes_build`'s, so the two features can never drift into
/// disagreeing about what counts as somebody's private business.
pub const BANNED_AREAS: [&str; 4] = ["health", "religion or caste", "sexuality or gender identity", "family"];

static BANNED_RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    notes_build::BANNED
        .iter()
        .filter(|(area, _)| BANNED_AREAS.contains(area))
        .map(|(area, words)| (*area, Regex::new(&format!(r"(?i)\b(?:{})\b", words)).expect("banned words")))
        .collect()
});

/// Tags that say nothing: what the model reaches for when it has been asked
/// about somebody who did not say anything.
static FILLER: LazyLock<Regex> = LazyLock::new(|| {
    // "the usual chat", "general conversation", "daily life": a qualifier that
    // narrows nothing, in front of a word that means "they were here".
    const LEAD: &str = r"(?:the\s+|a\s+|some\s+|just\s+)?";
    const QUALIFIER: &str =
        r"(?:general\s+|casual\s+|random\s+|daily\s+|normal\s+|usual\s+|various\s+|misc(?:ellaneous)?\s+|everyday\s+|idle\s+|light\s+)?";
    const NOTHING: &str = concat!(
        r"(?:chat(?:ting|s)?|conversation(?:s)?|talk(?:ing)?|banter|chit[- ]?chat|small\s+talk|discussion(?:s)?",
        r"|messages?|topics?|subjects?|stuff|things|life|activity|social(?:is|iz)ing|hanging\s+out|being\s+active",
        r"|nothing(?:\s+much)?|random|misc(?:ellaneous)?|general|various|day|server|discord|channel)",
    );
    Regex::new(&format!(r"(?i)^{}{}{}$", LEAD, QUALIFIER, NOTHING)).expect("filler")
});

/// Text between quote marks, three words or more: a quotation.
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["“”«»]([^"“”«»]+)["“”«»]"#).expect("regex"));
/// A mention left in, or a name with an @ in front.
static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>|@\w").expect("regex"));

fn words_of(text: &str) -> Vec<String> {
    text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Why a tag must go, or `None` when it may stay.
///
/// `names` are the day's members, lowercased: a tag is a subject, so one that
/// names a person is a tag about a person and goes. A tag touching a banned
/// area goes whatever it says, and so does filler.
pub fn reject_tag(tag: &str, names: &HashSet<String>) -> Option<&'static str> {
    let t = tag.trim();
    if t.chars().filter(|c| c.is_alphanumeric()).count() < 2 {
        return Some("nothing in it");
    }
    if t.split_whitespace().count() > 4 {
        return Some("a sentence, not a tag");
    }
    for (area, re) in BANNED_RES.iter() {
        if re.is_match(t) {
            return Some(area);
        }
    }
    if FILLER.is_match(t) {
        return Some("filler");
    }
    if MENTION.is_match(t) {
        return Some("names a person");
    }
    let w = words_of(t);
    if w.iter().any(|word| names.contains(word)) || names.contains(&t.to_lowercase()) {
        return Some("names a person");
    }
    None
}

/// Words in a row that count as quoting a message.
pub const QUOTE_RUN: usize = 5;

/// Why a line must go, or `None` when it may stay: a banned area, a quotation,
/// or a run of words lifted straight out of one of their messages.
pub fn reject_line(line: &str, source: &HashSet<String>) -> Option<&'static str> {
    let l = line.trim();
    if l.chars().filter(|c| c.is_alphabetic()).count() < 6 {
        return Some("nothing in it");
    }
    for (area, re) in BANNED_RES.iter() {
        if re.is_match(l) {
            return Some(area);
        }
    }
    if QUOTED.captures_iter(l).any(|c| c[1].split_whitespace().count() >= 3) {
        return Some("a quotation");
    }
    let w = words_of(l);
    if w.windows(QUOTE_RUN).any(|window| source.contains(&window.join(" "))) {
        return Some("words lifted from a message");
    }
    None
}

// --- one member's day ---------------------------------------------------------------------------

/// What is written down about one member for one day. Small on purpose: this
/// stacks up for every active member every night.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DayEntry {
    pub user_id: u64,
    pub day: String,
    /// A handful of short tags, in the order they were first seen.
    pub topics: Vec<String>,
    /// One plain line, or empty when every candidate line was thrown away.
    pub line: String,
    /// Their messages that day, thin ones included.
    pub messages: i64,
    /// Where they said it, busiest first.
    pub channels: Vec<u64>,
}

/// Everything one night's pass did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pass {
    pub entries: Vec<DayEntry>,
    /// Chunks sent to the model.
    pub chunks: usize,
    /// Of those, the ones that never came back. Their people are simply missing
    /// from the day; the rest of it is written down all the same.
    pub failed: usize,
    /// Members the floor kept out before anybody paid for them.
    pub too_thin: usize,
    /// Tags and lines the filter threw away.
    pub rejected: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,
}

/// The day's answers turned into entries: the floor applied for real, the filter
/// applied to every tag and every line, and the chunks merged per member.
///
/// A member's tags are the union of what each chunk said about them, in order,
/// capped; their line is the one from the chunk where they said most, because
/// that is the chunk that knew them best. Somebody under the floor gets nothing
/// even when the model was generous about them, and somebody whose every tag was
/// thrown away gets nothing rather than a bare entry.
pub fn merge(day_label: &str, day: &[Said], parts: Vec<About>, substance: &HashMap<u64, Substance>, set: &Settings) -> (Vec<DayEntry>, usize) {
    let names: HashSet<String> = day.iter().flat_map(|m| words_of(&m.author_name)).collect();
    let mut by_member: HashMap<u64, Vec<About>> = HashMap::new();
    for a in parts {
        by_member.entry(a.user_id).or_default().push(a);
    }
    let mut rejected = 0usize;
    let mut out: Vec<DayEntry> = Vec::new();
    for (user_id, mut said) in by_member {
        let Some(s) = substance.get(&user_id).filter(|s| eligible(s, set)) else { continue };
        // The chunk they said most in speaks for them.
        said.sort_by(|a, b| b.weight.cmp(&a.weight));
        let source = notes_build::shingles(&s.texts, QUOTE_RUN);
        let mut topics: Vec<String> = Vec::new();
        for tag in said.iter().flat_map(|a| a.topics.iter()) {
            match reject_tag(tag, &names) {
                Some(why) => {
                    tracing::debug!("topics: a tag was thrown away ({})", why);
                    rejected += 1;
                }
                None if topics.iter().any(|t| t.eq_ignore_ascii_case(tag)) => {}
                None if topics.len() < MAX_TAGS => topics.push(tag.clone()),
                None => {}
            }
        }
        let mut line = String::new();
        for candidate in said.iter().map(|a| a.line.as_str()).filter(|l| !l.trim().is_empty()) {
            match reject_line(candidate, &source) {
                Some(why) => {
                    tracing::debug!("topics: a line was thrown away ({})", why);
                    rejected += 1;
                }
                None => {
                    line = candidate.to_string();
                    break;
                }
            }
        }
        // Nothing survived: that is a day with nothing to say, not an empty row.
        if topics.is_empty() {
            continue;
        }
        out.push(DayEntry {
            user_id,
            day: day_label.to_string(),
            topics,
            line,
            messages: s.messages as i64,
            channels: s.channels.iter().copied().take(6).collect(),
        });
    }
    out.sort_by(|a, b| b.messages.cmp(&a.messages).then(a.user_id.cmp(&b.user_id)));
    (out, rejected)
}

/// One night's pass over one day, with the model passed in so the tests can hand
/// it a fake one — including one that fails on a chunk.
///
/// A chunk the model refuses is counted and stepped over: the rest of the day is
/// still read, still merged, and still written down. Nothing is written until
/// every chunk has been tried, so a half-finished pass never leaves half a day
/// in the store.
pub async fn run_day<F, Fut>(day_label: &str, day: &[Said], set: &Settings, ask: F) -> Pass
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Reply, String>>,
{
    let substance = substance(day);
    let too_thin = substance.values().filter(|s| !eligible(s, set)).count();
    let cut = chunks(day, set);
    let mut pass = Pass { chunks: cut.len(), too_thin, ..Pass::default() };
    let mut parts: Vec<About> = Vec::new();
    for chunk in &cut {
        let roster = roster(chunk, day);
        let prompt = build_prompt(day_label, chunk, day);
        match ask(prompt).await {
            Ok(reply) => {
                pass.input_tokens += reply.input_tokens;
                pass.output_tokens += reply.output_tokens;
                if pass.model.is_empty() {
                    pass.model = reply.model.clone();
                }
                match parse_chunk(&reply.text, &roster, day, chunk) {
                    Some(about) => parts.extend(about),
                    None => {
                        tracing::warn!("topics: #{} on {} came back unreadable", chunk.channel_name, day_label);
                        pass.failed += 1;
                    }
                }
            }
            Err(err) => {
                tracing::warn!("topics: #{} on {} failed: {}", chunk.channel_name, day_label, err);
                pass.failed += 1;
            }
        }
    }
    let (entries, rejected) = merge(day_label, day, parts, &substance, set);
    pass.entries = entries;
    pass.rejected = rejected;
    pass
}

// --- reading it back ------------------------------------------------------------------------------

/// One topic across a stretch of days: how often, and who.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TopicCount {
    pub topic: String,
    /// Members who had this tag, most days first.
    pub people: Vec<(u64, i64)>,
    /// Member-days it was tagged on.
    pub days: i64,
}

/// The week's topics across the server, commonest first, each with who was in
/// it. Tags are matched by their plain lowercase form, so "Cricket" and
/// "cricket" are one topic.
pub fn roll_up(entries: &[DayEntry], limit: usize) -> Vec<TopicCount> {
    let mut by_topic: HashMap<String, (String, HashMap<u64, i64>)> = HashMap::new();
    for e in entries {
        for t in &e.topics {
            let key = t.to_lowercase();
            let slot = by_topic.entry(key).or_insert_with(|| (t.clone(), HashMap::new()));
            *slot.1.entry(e.user_id).or_insert(0) += 1;
        }
    }
    let mut out: Vec<TopicCount> = by_topic
        .into_values()
        .map(|(topic, people)| {
            let days: i64 = people.values().sum();
            let mut who: Vec<(u64, i64)> = people.into_iter().collect();
            who.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            TopicCount { topic, people: who, days }
        })
        .collect();
    out.sort_by(|a, b| b.days.cmp(&a.days).then(b.people.len().cmp(&a.people.len())).then(a.topic.cmp(&b.topic)));
    out.truncate(limit);
    out
}

/// How one member's topics have moved: what is new in the recent stretch, what
/// they have kept up, and what they have dropped. `recent` and `before` are
/// their entries over two windows, the newer one first.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Drift {
    pub new: Vec<String>,
    pub kept: Vec<String>,
    pub dropped: Vec<String>,
}

pub fn drift(recent: &[DayEntry], before: &[DayEntry]) -> Drift {
    let set = |es: &[DayEntry]| -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        for e in es {
            for t in &e.topics {
                let key = t.to_lowercase();
                if !out.iter().any(|(k, _)| *k == key) {
                    out.push((key, t.clone()));
                }
            }
        }
        out
    };
    let (now, then) = (set(recent), set(before));
    Drift {
        new: now.iter().filter(|(k, _)| !then.iter().any(|(o, _)| o == k)).map(|(_, t)| t.clone()).collect(),
        kept: now.iter().filter(|(k, _)| then.iter().any(|(o, _)| o == k)).map(|(_, t)| t.clone()).collect(),
        dropped: then.iter().filter(|(k, _)| !now.iter().any(|(n, _)| n == k)).map(|(_, t)| t.clone()).collect(),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    const T0: i64 = 1_790_000_000;

    pub fn said(author: u64, name: &str, channel: u64, channel_name: &str, at: i64, text: &str) -> Said {
        Said { author_id: author, author_name: name.into(), channel_id: channel, channel_name: channel_name.into(), ts: T0 + at, text: text.into() }
    }

    /// A day across three channels, with four people who talked and two who
    /// did not: the shape every test here starts from.
    pub fn a_day() -> Vec<Said> {
        let mut out = Vec::new();
        for i in 0..8 {
            out.push(said(11, "gooner", 5, "chatting", i * 60, &format!("RCB ka batting order hi galat hai, {} number pe Maxwell kyun", i)));
            out.push(said(22, "potus", 5, "chatting", i * 60 + 10, &format!("bhai kohli ke bina kuch nahi hota, {} matches dekh le", i)));
        }
        for i in 0..7 {
            out.push(said(33, "riya", 6, "study-room", 3_600 + i * 60, &format!("semester exams {} din mein hai aur maine kuch padha hi nahi", i)));
            out.push(said(44, "dev", 6, "study-room", 3_600 + i * 60 + 20, &format!("same yaar, unit {} ka syllabus abhi tak khatam nahi hua", i)));
        }
        // Two who were about and said nothing worth writing down.
        for i in 0..9 {
            out.push(said(55, "quiet", 7, "games", 7_200 + i * 30, "lol"));
        }
        for i in 0..40 {
            out.push(said(66, "spammer", 7, "games", 7_200 + i * 5, "!hunt"));
        }
        out
    }

    fn reply(text: &str) -> Reply {
        Reply { text: text.into(), input_tokens: 100, output_tokens: 20, model: "cheap".into() }
    }

    fn answer(people: &[(&str, &[&str], &str)]) -> String {
        let list: Vec<Value> = people.iter().map(|(n, t, l)| serde_json::json!({ "name": n, "topics": t, "line": l })).collect();
        serde_json::json!({ "people": list }).to_string()
    }

    // --- the bulk pass ---------------------------------------------------------------------

    /// The whole day goes in a handful of chunks, and each person's topics come
    /// back on the right person — including when two channels ran at once.
    #[tokio::test]
    async fn one_pass_over_a_multi_channel_day_puts_topics_on_the_right_people() {
        let day = a_day();
        let set = Settings { min_messages: 5, min_chars: 60, ..Settings::default() };
        let seen = std::sync::Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
        let asked = seen.clone();
        let pass = run_day("2026-09-21", &day, &set, move |prompt: String| {
            let asked = asked.clone();
            async move {
                asked.lock().push(prompt.clone());
                // Answer for whoever the chunk actually named.
                Ok(if prompt.contains("#chatting") {
                    reply(&answer(&[
                        ("gooner", &["cricket", "RCB"], "Arguing about RCB's batting order all morning."),
                        ("potus", &["cricket", "Kohli"], "Defending Kohli against the usual."),
                    ]))
                } else if prompt.contains("#study-room") {
                    reply(&answer(&[
                        ("riya", &["college exams"], "Panicking about semester exams with nothing revised."),
                        ("dev", &["college exams", "syllabus"], "Comparing how much syllabus is left."),
                    ]))
                } else {
                    reply(&answer(&[]))
                })
            }
        })
        .await;

        assert_eq!(pass.failed, 0);
        assert!(pass.chunks >= 2, "the day was cut channel by channel: {} chunks", pass.chunks);
        // One call per chunk, not one per member: four people, not four calls.
        assert_eq!(seen.lock().len(), pass.chunks);
        assert!(pass.chunks < 10, "a day this size is a handful of chunks, not {}", pass.chunks);

        let by = |id: u64| pass.entries.iter().find(|e| e.user_id == id);
        assert_eq!(by(11).unwrap().topics, vec!["cricket", "RCB"]);
        assert_eq!(by(22).unwrap().topics, vec!["cricket", "Kohli"], "potus's tags did not land on gooner");
        assert_eq!(by(33).unwrap().topics, vec!["college exams"]);
        assert_eq!(by(44).unwrap().topics, vec!["college exams", "syllabus"]);
        assert_eq!(by(11).unwrap().day, "2026-09-21");
        assert_eq!(by(33).unwrap().channels, vec![6], "where they actually said it");
        assert!(by(33).unwrap().line.starts_with("Panicking"));
        assert_eq!(by(11).unwrap().messages, 8);
    }

    /// A chunk the model refuses costs that chunk's people, and nothing else.
    #[tokio::test]
    async fn a_chunk_the_model_fails_on_does_not_lose_the_rest_of_the_day() {
        let day = a_day();
        let set = Settings { min_messages: 5, min_chars: 60, ..Settings::default() };
        let pass = run_day("2026-09-21", &day, &set, |prompt: String| async move {
            if prompt.contains("#chatting") {
                return Err("the provider returned an error".to_string());
            }
            Ok(reply(&answer(&[("riya", &["college exams"], "Revising in a hurry."), ("dev", &["college exams"], "Same, with less done.")])))
        })
        .await;
        assert_eq!(pass.failed, 1, "one chunk was lost");
        assert!(pass.entries.iter().any(|e| e.user_id == 33), "the study-room chunk still landed");
        assert!(pass.entries.iter().any(|e| e.user_id == 44));
        assert!(!pass.entries.iter().any(|e| e.user_id == 11), "and the failed chunk's people simply have no entry");

        // An unreadable answer counts the same way and loses no more than itself.
        let pass = run_day("2026-09-21", &day, &set, |prompt: String| async move {
            if prompt.contains("#chatting") {
                return Ok(reply("sorry, I can't help with that"));
            }
            Ok(reply(&answer(&[("riya", &["college exams"], "Revising in a hurry.")])))
        })
        .await;
        assert_eq!(pass.failed, 1);
        assert!(pass.entries.iter().any(|e| e.user_id == 33));
    }

    /// The model may only answer about people the chunk actually had.
    #[test]
    fn a_person_the_chunk_never_had_is_not_written_down() {
        let day = a_day();
        let cut = chunks(&day, &Settings::default());
        let chatting = cut.iter().find(|c| c.channel_id == 5).expect("the chatting chunk");
        let r = roster(chatting, &day);
        let got = parse_chunk(
            &answer(&[("gooner", &["cricket"], "On about RCB."), ("nobody-here", &["anime"], "Watching anime."), ("riya", &["college exams"], "x")]),
            &r,
            &day,
            chatting,
        )
        .expect("read");
        assert_eq!(got.iter().map(|a| a.user_id).collect::<Vec<_>>(), vec![11], "only the one who was in this chunk");
        assert!(parse_chunk("not json", &r, &day, chatting).is_none());
    }

    // --- the floor -------------------------------------------------------------------------

    /// Somebody below the message floor gets nothing, however generous the model was.
    #[tokio::test]
    async fn below_the_message_floor_there_is_no_entry() {
        let mut day = a_day();
        // Four real messages: under a floor of five.
        for i in 0..4 {
            day.push(said(77, "barely", 5, "chatting", 10_000 + i * 60, &format!("haan bhai wo match dekha tha maine {} baar", i)));
        }
        let set = Settings { min_messages: 5, min_chars: 20, ..Settings::default() };
        let s = substance(&day);
        assert_eq!(s[&77].usable, 4);
        assert!(!eligible(&s[&77], &set), "four is under five");
        let pass = run_day("2026-09-21", &day, &set, |_| async {
            Ok(reply(&answer(&[("barely", &["cricket"], "Talked about a match.")])))
        })
        .await;
        assert!(!pass.entries.iter().any(|e| e.user_id == 77), "no entry, not an empty one");
        assert!(pass.too_thin > 0);
    }

    /// Enough messages, nothing in them: still no entry.
    #[tokio::test]
    async fn over_the_message_floor_but_under_the_text_floor_there_is_no_entry() {
        let mut day = a_day();
        for (i, word) in ["haan bhai sahi", "kya scene hai", "op op op", "lmao yaar lmao", "sach mein bhai", "arre nahi yaar"].iter().enumerate() {
            day.push(said(88, "shorty", 5, "chatting", 20_000 + i as i64 * 60, word));
        }
        let set = Settings { min_messages: 5, min_chars: 120, ..Settings::default() };
        let s = substance(&day);
        assert!(s[&88].usable >= 5, "six short messages clear the message floor: {}", s[&88].usable);
        assert!(s[&88].chars < 120, "and are nowhere near the text floor: {} characters", s[&88].chars);
        assert!(!eligible(&s[&88], &set));
        let pass = run_day("2026-09-21", &day, &set, |_| async {
            Ok(reply(&answer(&[("shorty", &["cricket"], "Agreed with people about cricket.")])))
        })
        .await;
        assert!(!pass.entries.iter().any(|e| e.user_id == 88), "busy is not the same as having said something");
    }

    /// A day of one-word replies and a day of bot commands are both no-entry days.
    #[tokio::test]
    async fn a_spam_only_day_is_a_no_entry_day() {
        let day = a_day();
        let set = Settings { min_messages: 5, min_chars: 60, ..Settings::default() };
        let s = substance(&day);
        // "lol" nine times is one usable message at most, and repeats do not count.
        assert!(s[&55].usable <= 1, "nine lols: {:?}", s[&55]);
        assert_eq!(s[&66].usable, 0, "forty bot commands are not somebody talking");
        assert_eq!(s[&66].messages, 40, "the counts still exist; this pass is only about what was said");

        let pass = run_day("2026-09-21", &day, &set, |_| async {
            Ok(reply(&answer(&[("quiet", &["chatting"], "Was around."), ("spammer", &["games"], "Playing a game bot.")])))
        })
        .await;
        assert!(!pass.entries.iter().any(|e| e.user_id == 55 || e.user_id == 66));
        // And their messages never went into a prompt to be paid for.
        let cut = chunks(&day, &set);
        let games: Vec<&Chunk> = cut.iter().filter(|c| c.channel_id == 7).collect();
        let lines: usize = games.iter().map(|c| c.lines.len()).sum();
        assert!(lines <= 1, "a channel of spam is not worth a chunk: {} lines", lines);
    }

    // --- the rules the topics are kept to ----------------------------------------------------

    /// Nothing about anyone's sexuality, gender, religion, caste, health or
    /// family is ever written down, however plainly it was talked about.
    #[tokio::test]
    async fn the_banned_inferences_never_reach_the_store() {
        let mut day = a_day();
        for i in 0..9 {
            day.push(said(99, "open", 5, "chatting", 30_000 + i * 60, &format!("aaj bahut kuch hua mere saath, story number {} suno pura", i)));
        }
        let set = Settings { min_messages: 5, min_chars: 60, ..Settings::default() };
        let pass = run_day("2026-09-21", &day, &set, |_| async {
            Ok(reply(&answer(&[(
                "open",
                &["his depression", "namaz timings", "being gay", "his mother's health", "caste politics", "cricket"],
                "Talked about coming out to his family and his therapy sessions.",
            )])))
        })
        .await;
        let e = pass.entries.iter().find(|e| e.user_id == 99).expect("cricket survived, so there is an entry");
        assert_eq!(e.topics, vec!["cricket"], "every banned tag was thrown away: {:?}", e.topics);
        assert_eq!(e.line, "", "and the line went with them");
        assert!(pass.rejected >= 6);

        // Each area, on its own, by name.
        let names = HashSet::new();
        for (tag, area) in [
            ("his depression", "health"),
            ("therapy", "health"),
            ("namaz", "religion or caste"),
            ("being hindu", "religion or caste"),
            ("brahmin stuff", "religion or caste"),
            ("being gay", "sexuality or gender identity"),
            ("her pronouns", "sexuality or gender identity"),
            ("his mother", "family"),
            ("family problems", "family"),
        ] {
            assert_eq!(reject_tag(tag, &names), Some(area), "{tag:?} should go as {area}");
        }
        // And the things that are allowed stay.
        for tag in ["cricket", "college exams", "anime", "the house cup", "valorant", "chess", "food"] {
            assert_eq!(reject_tag(tag, &names), None, "{tag:?} is exactly the level a tag is meant to be at");
        }
    }

    /// Filler is what the model reaches for when it has nothing: it never lands.
    #[tokio::test]
    async fn filler_topics_are_dropped() {
        let names = HashSet::new();
        for tag in [
            "chatting", "general conversation", "conversation", "talking", "random", "misc", "miscellaneous", "chit-chat", "small talk",
            "general chat", "casual conversation", "daily life", "banter", "stuff", "things", "socialising", "hanging out", "nothing much",
            "the usual chat", "various topics", "discord", "the server",
        ] {
            assert_eq!(reject_tag(tag, &names), Some("filler"), "{tag:?} says nothing and must go");
        }
        // A day whose every tag is filler is a day with no entry at all.
        let day = a_day();
        let set = Settings { min_messages: 5, min_chars: 60, ..Settings::default() };
        let pass = run_day("2026-09-21", &day, &set, |_| async {
            Ok(reply(&answer(&[
                ("gooner", &["chatting", "general conversation"], "Was chatting."),
                ("potus", &["cricket"], "Arguing about the batting order."),
            ])))
        })
        .await;
        assert!(!pass.entries.iter().any(|e| e.user_id == 11), "filler only is no entry");
        assert_eq!(pass.entries.iter().find(|e| e.user_id == 22).unwrap().topics, vec!["cricket"]);
    }

    /// Nothing is quoted, nothing names a person, and nothing carries a third
    /// party's private business.
    #[test]
    fn nothing_quoted_no_people_in_tags_and_no_third_party_business() {
        let day = a_day();
        let source = notes_build::shingles(&day.iter().filter(|m| m.author_id == 11).map(|m| m.text.clone()).collect::<Vec<_>>(), QUOTE_RUN);
        assert_eq!(reject_line("He said \"RCB ka batting order hi galat\" all day.", &source), Some("a quotation"));
        // Words lifted straight out of a message, without quote marks.
        assert_eq!(reject_line("Went on about how RCB ka batting order hi galat hai.", &source), Some("words lifted from a message"));
        assert_eq!(reject_line("Argued about the batting order for most of the morning.", &source), None);

        let names: HashSet<String> = ["gooner", "potus", "riya"].iter().map(|s| s.to_string()).collect();
        assert_eq!(reject_tag("riya", &names), Some("names a person"));
        assert_eq!(reject_tag("arguing with potus", &names), Some("names a person"));
        assert_eq!(reject_tag("@gooner", &names), Some("names a person"));
        assert_eq!(reject_tag("cricket", &names), None);
        // A third party's private business trips the same banned areas.
        assert_eq!(reject_line("Asked around about Dev's sister's wedding.", &source), Some("family"));
        assert_eq!(reject_line("Relaying gossip about who is in hospital.", &source), Some("health"));
        // And a tag has to stay a tag.
        assert_eq!(reject_tag("spent the whole day arguing about cricket scores", &names), Some("a sentence, not a tag"));
        assert_eq!(reject_tag("!!", &names), Some("nothing in it"));
    }

    /// #safe-corner cannot reach the pass: it is never in the messages, and a
    /// row that somehow arrived is still not what the prompt is built from.
    #[test]
    fn safe_corner_is_absent() {
        // The day the job hands over comes from `msglog`, which never kept it.
        let day = a_day();
        assert!(!day.iter().any(|m| m.channel_id == super::super::weekly::SAFE_CORNER));
        assert!(!day.iter().any(|m| m.channel_name.contains("safe-corner")));
        // The prompt only ever holds the chunk's own channel and its own lines.
        let cut = chunks(&day, &Settings::default());
        for chunk in &cut {
            let prompt = build_prompt("2026-09-21", chunk, &day);
            assert!(!prompt.contains("safe-corner"), "a prompt named #safe-corner");
            assert!(prompt.contains(&format!("Channel: #{}", chunk.channel_name)));
            for i in &chunk.lines {
                assert_eq!(day[*i].channel_id, chunk.channel_id, "a chunk only ever holds its own channel");
            }
        }
        // And the module's own filter is on the same list `msglog` gates on.
        assert!(super::super::msglog::never_logged().contains(&super::super::weekly::SAFE_CORNER));
    }

    /// The instructions have to keep saying what the store promises.
    #[test]
    fn the_instructions_hold_their_promises() {
        let i = INSTRUCTIONS.to_lowercase();
        for must in ["sexuality", "gender", "religion", "caste", "health", "family", "never quote", "third party", "cricket", "college exams", "anime", "the house cup"] {
            assert!(i.contains(must), "the instructions dropped “{must}”");
        }
        for must in ["leave that person out entirely", "do not invent a topic", "chatting", "general conversation"] {
            assert!(i.contains(must), "the instructions stopped forbidding “{must}”");
        }
    }

    // --- the cap and the shape of the work -----------------------------------------------------

    /// The cap is a cap, and it takes the quiet channels rather than every channel's tail.
    #[test]
    fn the_work_is_capped_and_the_busiest_channels_are_kept() {
        let mut day = a_day();
        // Ten more channels, each with a little in it.
        for c in 100..110u64 {
            for i in 0..6 {
                day.push(said(1_000 + c, &format!("m{}", c), c, &format!("side-{}", c), 40_000 + i * 60, "kal wala match kaafi close tha yaar sach mein"));
            }
        }
        let all = chunks(&day, &Settings { max_chunks: 500, chunk_tokens: 200, ..Settings::default() });
        assert!(all.len() > 12, "this day needs plenty of chunks: {}", all.len());
        let capped = chunks(&day, &Settings { max_chunks: 4, chunk_tokens: 200, ..Settings::default() });
        assert_eq!(capped.len(), 4, "the cap holds");
        assert!(capped.iter().all(|c| c.channel_id == 5 || c.channel_id == 6), "the busiest channels are the ones kept: {:?}", capped.iter().map(|c| c.channel_id).collect::<Vec<_>>());
        assert!(chunks(&day, &Settings { max_chunks: 0, ..Settings::default() }).is_empty(), "nought chunks is the whole thing off");

        // A chunk stays inside its budget, except where one message is bigger than it.
        for chunk in &all {
            let spent: usize = chunk.lines.iter().map(|i| cost(&day[*i])).sum();
            assert!(spent <= 200 || chunk.lines.len() == 1, "a chunk overflowed: {} tokens over {} lines", spent, chunk.lines.len());
        }
    }

    /// The whole day in one pass: no chunk repeats a message and none is lost.
    #[test]
    fn every_usable_message_is_in_exactly_one_chunk() {
        let day = a_day();
        let cut = chunks(&day, &Settings { chunk_tokens: 150, ..Settings::default() });
        let mut seen: Vec<usize> = cut.iter().flat_map(|c| c.lines.iter().copied()).collect();
        let before = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), before, "a message was in two chunks");
        let want: usize = day.iter().filter(|m| notes_build::usable(&m.text).is_some()).count();
        assert_eq!(seen.len(), want, "every message with something in it was sent exactly once");
    }

    // --- reading it back -------------------------------------------------------------------------

    #[test]
    fn the_week_rolls_up_into_topics_with_who_is_in_each() {
        let e = |user: u64, day: &str, topics: &[&str]| DayEntry {
            user_id: user,
            day: day.into(),
            topics: topics.iter().map(|t| t.to_string()).collect(),
            line: "x".into(),
            messages: 10,
            channels: vec![5],
        };
        let week = vec![
            e(11, "2026-09-21", &["cricket", "the house cup"]),
            e(22, "2026-09-21", &["Cricket"]),
            e(33, "2026-09-21", &["college exams"]),
            e(11, "2026-09-22", &["cricket"]),
            e(44, "2026-09-22", &["anime"]),
        ];
        let top = roll_up(&week, 10);
        assert_eq!(top[0].topic.to_lowercase(), "cricket");
        assert_eq!(top[0].days, 3, "three member-days");
        assert_eq!(top[0].people.iter().map(|(id, n)| (*id, *n)).collect::<Vec<_>>(), vec![(11, 2), (22, 1)], "who is in it, most days first");
        assert_eq!(roll_up(&week, 2).len(), 2, "the list is capped");
        assert!(roll_up(&[], 10).is_empty());

        // And one member's own drift: what is new, kept and gone.
        let d = drift(&[e(11, "2026-09-22", &["cricket", "anime"])], &[e(11, "2026-09-14", &["cricket", "the house cup"])]);
        assert_eq!(d.new, vec!["anime"]);
        assert_eq!(d.kept, vec!["cricket"]);
        assert_eq!(d.dropped, vec!["the house cup"]);
    }


    /// What a real day costs, measured rather than guessed.
    ///
    /// The server is about 180 active people and twenty-odd thousand messages a
    /// day, and most of those messages are two words long. This builds a day
    /// that size with that mix and adds up every prompt the pass would actually
    /// send — the instructions, repeated once per chunk, included.
    ///
    /// Two things are being held here. The bill, so a change that quietly
    /// doubles it fails here rather than on the bot. And, more importantly, that
    /// an ordinary day fits inside the cap with room to spare: a cap that
    /// silently drops half of every normal day would be worse than no cap.
    #[test]
    fn a_full_days_pass_costs_what_it_is_supposed_to() {
        // Eight channels of very different sizes, 180 people.
        let channels: [(u64, &str, usize); 8] =
            [(5, "chatting", 7_000), (25, "cricket-talk", 4_500), (7, "games", 3_500), (6, "study-room", 2_400),
             (30, "anime-club", 1_800), (22, "memes", 1_500), (24, "music", 900), (31, "vent-lite", 400)];
        // What people actually send: mostly reactions and two-word replies, with
        // a real sentence every few messages. Only the last four survive `usable`.
        let lines = [
            "lol", "haan", "😂😂", "same", "!hunt", "op", "bhai", "wtf", "nahi", "sahi hai",
            "bhai ye match dekha tha kal raat ko, last over mein kya hua",
            "mujhe kal ka paper dena hai aur maine abhi tak kuch nahi padha hai",
            "koi valorant khelega aaj raat ko, do log chahiye",
            "wo naya episode dekha kisi ne, ending ne toh hila diya",
        ];
        let mut day: Vec<Said> = Vec::new();
        let mut at = 0i64;
        for (channel_id, name, count) in channels {
            for i in 0..count {
                let author = 1_000 + (i % 180) as u64;
                at += 4;
                // A different line each time, so repeats do not swallow the day.
                let text = lines[i % lines.len()];
                let text = if i % lines.len() >= 10 { format!("{} ({})", text, i) } else { text.to_string() };
                day.push(said(author, &format!("member{}", author), channel_id, name, at, &text));
            }
        }
        assert_eq!(day.len(), 22_000, "a day the size of a real one");
        let worth_sending = day.iter().filter(|m| notes_build::usable(&m.text).is_some()).count();

        let set = Settings::default();
        let cut = chunks(&day, &set);
        let prompts: Vec<String> = cut.iter().map(|c| build_prompt("2026-09-21", c, &day)).collect();
        let sent: usize = prompts.iter().map(|p| notes_build::estimate_tokens(p)).sum();
        let overhead = cut.len() * notes_build::estimate_tokens(INSTRUCTIONS);
        println!(
            "a 22,000-message day: {} of them worth sending, {} chunks, {} input tokens in all \
             ({} of those the instructions, repeated once a chunk), {} tokens a chunk on average",
            worth_sending,
            cut.len(),
            sent,
            overhead,
            sent / cut.len().max(1)
        );

        // One pass over the day, not one call per member.
        assert!(cut.len() < 180, "{} calls is still far fewer than one per member", cut.len());
        // The whole day got sent: an ordinary day must not run into the cap.
        let in_chunks: usize = cut.iter().map(|c| c.lines.len()).sum();
        assert_eq!(in_chunks, worth_sending, "the cap cut an ordinary day short at {} chunks", cut.len());
        assert!(cut.len() * 2 <= set.max_chunks, "an ordinary day should leave room under the cap: {} of {}", cut.len(), set.max_chunks);
        // And the bill is in the band the feature was designed around.
        assert!((80_000..300_000).contains(&sent), "a day's input is {} tokens, which is not what was budgeted for", sent);
        assert!(overhead * 3 < sent, "the instructions must not be most of what is paid for: {} of {}", overhead, sent);
    }

    #[test]
    fn the_settings_have_the_defaults_they_are_documented_with() {
        assert!(topics_on(), "the nightly pass is on by default");
        assert_eq!(run_hour(), 5);
        assert_eq!((min_messages(), min_chars()), (5, 120));
        assert_eq!((chunk_tokens(), max_chunks()), (12_000, 40));
        assert_eq!(day_words("2026-09-21"), "Monday 21 September 2026");
        // With nothing set the pass falls through to the summary model, and on
        // through that to the bot's own.
        assert_eq!(topics_model(), super::super::kalesh::summary_model());
    }
}
