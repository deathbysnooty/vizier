//! Looking back at a fight ("kalesh") after it happened.
//!
//! The live detector in `mod.rs` only pings. This is the other half: it keeps
//! a record of each fight the detector called, finds where two named members
//! went at each other in the message log, lays the exchange out in order with
//! whose side each message is on, and — only when a moderator asks — has the
//! bot's main model write a neutral summary of it.
//!
//! Everything here reads `msglog`'s `recent` table (every channel except
//! #safe-corner and the skip list, kept for a year); nothing reads the AI's own
//! history. The panel page is `control/web/kalesh.rs`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::ops::Range;
use std::time::Duration;

use chrono::{FixedOffset, TimeZone};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::control;
use super::kalesh_store::{self as store, NewDetection, Participant};
use super::msglog::{self, Gone, GoneFilter, SaidRow};

/// Both talking in one channel within this long of each other counts as engaging.
pub const NEAR_MS: i64 = 5 * 60_000;
/// Engagement further apart than this in one channel starts a new stretch.
pub const JOIN_GAP_MS: i64 = 20 * 60_000;
/// The period looked through when none is given.
pub const DEFAULT_PERIOD_MS: i64 = 24 * 3_600_000;
/// The longest period one search may cover.
pub const MAX_PERIOD_MS: i64 = 7 * 86_400_000;
/// The two members' own messages one search reads at most.
pub const AUTHOR_ROWS: usize = 20_000;
/// Messages one stretch shows at most.
pub const EXCHANGE_ROWS: usize = 3_000;
/// Longest a single message is in the prompt; longer ones are cut and say so.
pub const MAX_TEXT_CHARS: usize = 1_200;
/// How long one try at a summary may take. A whole period is a long prompt.
const MODEL_WAIT: Duration = Duration::from_secs(240);
/// Tries at the model for one answer, and the waits between them. Every feature
/// that asks the model to write something goes through [`ask_retrying`], so
/// there is one place where "the provider failed" is decided, and one place to
/// change how patient the bot is.
pub const TRIES: usize = 3;
#[cfg(not(test))]
const RETRY_WAITS: [Duration; 2] = [Duration::from_secs(3), Duration::from_secs(8)];
#[cfg(test)]
const RETRY_WAITS: [Duration; 2] = [Duration::from_millis(5), Duration::from_millis(5)];
/// The most members one look may be about.
pub const MAX_PEOPLE: usize = 6;

/// How many messages the model may be shown for one summary.
pub fn summary_max() -> usize {
    control::number("VIZIER_KALESH_SUMMARY_MAX_MESSAGES", 400).clamp(20, 5_000) as usize
}

/// The model summaries use; none means the bot's main model.
pub fn summary_model() -> Option<String> {
    control::var("VIZIER_KALESH_SUMMARY_MODEL")
}

fn ist() -> FixedOffset {
    FixedOffset::east_opt(5 * 3600 + 1800).expect("IST")
}

/// "21:04", India time.
pub fn clock(ms: i64) -> String {
    ist().timestamp_millis_opt(ms).single().map(|t| t.format("%H:%M").to_string()).unwrap_or_default()
}

/// "20 Sep", India time.
pub fn day(ms: i64) -> String {
    ist().timestamp_millis_opt(ms).single().map(|t| t.format("%-d %b").to_string()).unwrap_or_default()
}

/// "20 Sep 23:02 – 21 Sep 00:48", or "20 Sep 23:02–23:48" within one day.
pub fn span(start_ms: i64, end_ms: i64) -> String {
    if day(start_ms) == day(end_ms) {
        format!("{} {}–{}", day(start_ms), clock(start_ms), clock(end_ms))
    } else {
        format!("{} {} – {} {}", day(start_ms), clock(start_ms), day(end_ms), clock(end_ms))
    }
}

// --- recording what the live detector saw -----------------------------------------------------

/// One message in the detector's window, as much of it as a record needs.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub id: u64,
    pub ts_ms: i64,
    pub author: u64,
    pub author_name: String,
}

/// The record of a burst the detector called a fight: who, busiest first.
pub fn detection_of(channel: u64, window: &[Seen], line: &str, now_ts: i64) -> Option<NewDetection> {
    let start_ms = window.iter().map(|m| m.ts_ms).min()?;
    let end_ms = window.iter().map(|m| m.ts_ms).max()?;
    let mut people: Vec<(usize, Participant)> = Vec::new();
    for (i, m) in window.iter().enumerate() {
        match people.iter_mut().find(|(_, p)| p.id == m.author) {
            Some((_, p)) => p.messages += 1,
            None => people.push((i, Participant { id: m.author, name: m.author_name.clone(), messages: 1 })),
        }
    }
    people.sort_by(|x, y| y.1.messages.cmp(&x.1.messages).then(x.0.cmp(&y.0)));
    Some(NewDetection {
        channel_id: channel,
        start_ms,
        end_ms,
        participants: people.into_iter().map(|(_, p)| p).collect(),
        message_ids: window.iter().map(|m| m.id).filter(|id| *id > 0).collect(),
        line: line.to_string(),
        created_ts: now_ts,
    })
}

/// Writes a detection to the store. Never fails the caller: a record is a
/// nicety next to the ping.
pub fn record_detection(channel: u64, window: &[Seen], line: &str) -> Option<i64> {
    let detection = detection_of(channel, window, line, chrono::Utc::now().timestamp())?;
    let db = store::db()?;
    match store::add_detection(&db.lock(), &detection) {
        Ok(id) => Some(id),
        Err(err) => {
            tracing::warn!("kalesh: couldn't record a detection in {}: {}", channel, err);
            None
        }
    }
}

/// What happens once stage one has tripped: wait for the model's verdict and,
/// if it says fight, keep a record before handing the line back to be posted.
pub async fn judge_and_record(channel: u64, window: Vec<Seen>, verdict: impl Future<Output = Option<String>>) -> Option<String> {
    let line = verdict.await?;
    record_detection(channel, &window, &line);
    Some(line)
}

// --- reading the message log ------------------------------------------------------------------

/// Oldest first by id, one row per message, at most `limit`.
fn in_order(mut rows: Vec<SaidRow>, limit: usize) -> Vec<SaidRow> {
    rows.sort_by_key(|r| r.message_id);
    rows.dedup_by_key(|r| r.message_id);
    rows.truncate(limit);
    rows
}

/// The chosen members' own messages between two moments (`until` inclusive),
/// oldest first, optionally in one channel only. Their deleted and blocked
/// messages are in it too, in their place, marked.
pub fn authors_between(conn: &Connection, people: &[u64], since_ms: i64, until_ms: i64, channel: Option<u64>, limit: usize) -> rusqlite::Result<Vec<SaidRow>> {
    let (lo, hi) = (msglog::first_id_at(since_ms), msglog::first_id_at(until_ms + 1));
    let mut out = Vec::new();
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM recent WHERE author_id = ?1 AND message_id >= ?2 AND message_id < ?3
           AND (?4 IS NULL OR channel_id = ?4) ORDER BY message_id LIMIT ?5",
        msglog::SAID_COLUMNS
    ))?;
    for who in people {
        let rows = stmt.query_map(params![*who as i64, lo as i64, hi as i64, channel.map(|c| c as i64), limit as i64], msglog::said_row)?;
        for r in rows {
            out.push(r?);
        }
    }
    out.extend(msglog::gone_between(conn, &GoneFilter { authors: people.to_vec(), channel, with_threads: false, lo_id: lo, hi_id: hi, limit })?);
    Ok(in_order(out, limit))
}

/// Everything said in one channel (or thread) between two moments (`until`
/// inclusive), oldest first: what is still there, what was deleted, and what
/// AutoMod blocked, each in its place.
pub fn channel_between(conn: &Connection, channel: u64, since_ms: i64, until_ms: i64, limit: usize) -> rusqlite::Result<Vec<SaidRow>> {
    let (lo, hi) = (msglog::first_id_at(since_ms), msglog::first_id_at(until_ms + 1));
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT {} FROM recent WHERE channel_id = ?1 AND message_id >= ?2 AND message_id < ?3 ORDER BY message_id LIMIT ?4",
        msglog::SAID_COLUMNS
    ))?;
    let rows = stmt.query_map(params![channel as i64, lo as i64, hi as i64, limit as i64], msglog::said_row)?;
    let mut out: Vec<SaidRow> = rows.collect::<rusqlite::Result<_>>()?;
    out.extend(msglog::gone_between(conn, &GoneFilter { authors: Vec::new(), channel: Some(channel), with_threads: false, lo_id: lo, hi_id: hi, limit })?);
    Ok(in_order(out, limit))
}

// --- finding the stretches --------------------------------------------------------------------

/// Whether a message pings this member.
pub fn mentions(text: &str, id: u64) -> bool {
    text.contains(&format!("<@{}>", id)) || text.contains(&format!("<@!{}>", id))
}

/// How one message of theirs engages another of the chosen members.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Link {
    Reply,
    Mention,
    Nearby,
}

/// A stretch of one channel where the chosen members were engaging each other.
#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    pub channel_id: u64,
    pub parent_id: Option<u64>,
    pub channel_name: String,
    pub start_ms: i64,
    pub end_ms: i64,
    /// Each chosen member's own messages in it, in the order they were chosen.
    pub per_person: Vec<usize>,
    /// Replies from one chosen member to another: (from, to, how many), by position.
    pub replies: Vec<(usize, usize, usize)>,
    pub mentions: usize,
    pub nearby: usize,
}

impl Stretch {
    pub fn reply_count(&self) -> usize {
        self.replies.iter().map(|r| r.2).sum()
    }
    /// Replies from `from` to `to` (positions).
    pub fn replies_from(&self, from: usize, to: usize) -> usize {
        self.replies.iter().filter(|r| r.0 == from && r.1 == to).map(|r| r.2).sum()
    }
}

/// Where the chosen members (two to six) were engaging each other, newest first.
/// `rows` are their own messages (anyone else's are ignored), in any order.
///
/// A message counts when it replies to another of them, mentions one, or was
/// sent within [`NEAR_MS`] of another of them talking in the same channel. A
/// reply also pulls the message it answers into the stretch. Engaging messages
/// in one channel no more than [`JOIN_GAP_MS`] apart make one stretch; a stretch
/// is listed when it has a reply or a mention, or at least two messages from
/// each of at least two of them.
pub fn find_stretches(rows: &[SaidRow], people: &[u64]) -> Vec<Stretch> {
    let pos = |id: u64| people.iter().position(|p| *p == id);
    let mut own: Vec<&SaidRow> = rows.iter().filter(|r| pos(r.author_id).is_some()).collect();
    own.sort_by_key(|r| r.message_id);
    own.dedup_by_key(|r| r.message_id);
    let author_of: HashMap<u64, u64> = own.iter().map(|r| (r.message_id, r.author_id)).collect();
    let time_of: HashMap<u64, i64> = own.iter().map(|r| (r.message_id, r.created_ms)).collect();
    let mut names: HashMap<u64, HashSet<String>> = HashMap::new();
    for r in &own {
        names.entry(r.author_id).or_default().insert(r.author_name.to_lowercase());
    }
    let mut by_channel: Vec<(u64, Vec<&SaidRow>)> = Vec::new();
    for r in &own {
        match by_channel.iter_mut().find(|(c, _)| *c == r.channel_id) {
            Some((_, list)) => list.push(r),
            None => by_channel.push((r.channel_id, vec![r])),
        }
    }

    // (time, what it was: a link from one position, or a reply target's time).
    type Point = (i64, Option<(Link, usize, Option<usize>)>);
    let mut out = Vec::new();
    for (channel, list) in by_channel {
        let mut points: Vec<Point> = Vec::new();
        for (i, r) in list.iter().enumerate() {
            let me = pos(r.author_id).unwrap_or(0);
            // A blocked message was never in the channel: it can't be replied to,
            // but it is aimed at whoever it names, and it sits in time like any other.
            let replied_to = r.reply_to.and_then(|to| match author_of.get(&to) {
                Some(author) => pos(*author).filter(|p| *p != me),
                None => r.reply_author.as_ref().and_then(|n| {
                    let n = n.to_lowercase();
                    people.iter().position(|p| *p != r.author_id && names.get(p).is_some_and(|set| set.contains(&n)))
                }),
            });
            let mentioned = people.iter().position(|p| *p != r.author_id && mentions(&r.content, *p));
            let near = || {
                let other = |o: &&&SaidRow| o.author_id != r.author_id;
                let back = list[..i].iter().rev().take_while(|o| r.created_ms - o.created_ms <= NEAR_MS).any(|o| other(&o));
                back || list[i + 1..].iter().take_while(|o| o.created_ms - r.created_ms <= NEAR_MS).any(|o| other(&o))
            };
            let (link, to) = if let Some(to) = replied_to {
                (Link::Reply, Some(to))
            } else if let Some(to) = mentioned {
                (Link::Mention, Some(to))
            } else if near() {
                (Link::Nearby, None)
            } else {
                continue;
            };
            points.push((r.created_ms, Some((link, me, to))));
            if link == Link::Reply {
                if let Some(t) = r.reply_to.and_then(|to| time_of.get(&to)) {
                    if list.iter().any(|o| Some(o.message_id) == r.reply_to) {
                        points.push((*t, None));
                    }
                }
            }
        }
        points.sort_by_key(|p| p.0);
        let mut groups: Vec<Vec<Point>> = Vec::new();
        for p in points {
            match groups.last_mut() {
                Some(g) if p.0 - g.last().map(|x| x.0).unwrap_or(p.0) <= JOIN_GAP_MS => g.push(p),
                _ => groups.push(vec![p]),
            }
        }
        for g in groups {
            let (start_ms, end_ms) = (g[0].0, g[g.len() - 1].0);
            let per_person: Vec<usize> =
                people.iter().map(|p| list.iter().filter(|r| r.author_id == *p && r.created_ms >= start_ms && r.created_ms <= end_ms).count()).collect();
            let mut replies: Vec<(usize, usize, usize)> = Vec::new();
            for (_, l) in &g {
                if let Some((Link::Reply, from, Some(to))) = l {
                    match replies.iter_mut().find(|r| r.0 == *from && r.1 == *to) {
                        Some(r) => r.2 += 1,
                        None => replies.push((*from, *to, 1)),
                    }
                }
            }
            replies.sort();
            let count = |want: Link| g.iter().filter(|(_, l)| l.is_some_and(|(k, _, _)| k == want)).count();
            let s = Stretch {
                channel_id: channel,
                parent_id: list[0].parent_id,
                channel_name: list[0].channel_name.clone(),
                start_ms,
                end_ms,
                per_person,
                replies,
                mentions: count(Link::Mention),
                nearby: count(Link::Nearby),
            };
            if s.reply_count() + s.mentions > 0 || s.per_person.iter().filter(|n| **n >= 2).count() >= 2 {
                out.push(s);
            }
        }
    }
    out.sort_by(|x, y| y.start_ms.cmp(&x.start_ms));
    out
}

// --- laying the exchange out ----------------------------------------------------------------------

/// Whose side of the exchange a message is on: one of the chosen members (by
/// position, shown as A, B, C…), or a bystander.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Person(usize),
    Other,
}

impl Side {
    pub const A: Side = Side::Person(0);
    pub const B: Side = Side::Person(1);

    /// "A", "B", … for a chosen member.
    pub fn letter(self) -> Option<char> {
        match self {
            Side::Person(i) => Some((b'A' + (i as u8).min(25)) as char),
            Side::Other => None,
        }
    }
}

impl Serialize for Side {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self.letter() {
            Some(c) => s.serialize_str(&c.to_ascii_lowercase().to_string()),
            None => s.serialize_str("other"),
        }
    }
}

/// One message of a stretch, numbered from 1 in order.
#[derive(Clone, Debug)]
pub struct Line<'a> {
    pub n: usize,
    pub row: &'a SaidRow,
    pub side: Side,
    /// Which chosen member it replies to or mentions (never the author themselves).
    pub towards: Option<Side>,
    /// The number of the message it replies to, when that is in the stretch too.
    pub reply_n: Option<usize>,
}

/// Every message of a stretch in order, bystanders included, each marked with
/// whose it is and which of the chosen members it is aimed at.
pub fn exchange<'a>(rows: &'a [SaidRow], people: &[u64]) -> Vec<Line<'a>> {
    let mut sorted: Vec<&SaidRow> = rows.iter().collect();
    sorted.sort_by_key(|r| r.message_id);
    sorted.dedup_by_key(|r| r.message_id);
    let index: HashMap<u64, (usize, u64)> = sorted.iter().enumerate().map(|(i, r)| (r.message_id, (i + 1, r.author_id))).collect();
    let side_of = |author: u64| people.iter().position(|p| *p == author).map(Side::Person).unwrap_or(Side::Other);
    sorted
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let side = side_of(r.author_id);
            let target = r.reply_to.and_then(|to| index.get(&to).copied());
            let replied_side = target.map(|(_, author)| side_of(author)).filter(|s| *s != Side::Other && *s != side);
            let named: Vec<Side> = people.iter().enumerate().filter(|(_, p)| **p != r.author_id && mentions(&r.content, **p)).map(|(i, _)| Side::Person(i)).collect();
            let mentioned = (named.len() == 1).then(|| named[0]);
            Line { n: i + 1, row: *r, side, towards: replied_side.or(mentioned), reply_n: target.map(|(n, _)| n) }
        })
        .collect()
}

/// The chosen members, lowest id first: the same group whichever order it was asked in.
fn group(people: &[u64]) -> String {
    let mut ids = people.to_vec();
    ids.sort_unstable();
    ids.dedup();
    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join("-")
}

fn span_of(lines: &[Line]) -> String {
    let first = lines.first().map(|l| l.row.message_id).unwrap_or(0);
    let last = lines.last().map(|l| l.row.message_id).unwrap_or(0);
    format!("{}-{}:{}", first, last, lines.len())
}

/// What identifies one stretch's summary: the group, the channel and the exact
/// messages. A stretch that has grown since is a new stretch.
pub fn stretch_key(people: &[u64], channel: u64, lines: &[Line]) -> String {
    format!("{}:{}:{}", group(people), channel, span_of(lines))
}

/// What identifies a whole period's summary: the group and the exact messages.
pub fn period_key(people: &[u64], lines: &[Line]) -> String {
    format!("period:{}:{}", group(people), span_of(lines))
}

// --- the prompt ------------------------------------------------------------------------------------

/// Which messages the model sees when there are more than `max`: the opening,
/// the busiest run of the middle (the shortest span of time holding that many
/// messages) and the ending. `times` are in order.
pub fn pick(times: &[i64], max: usize) -> Vec<Range<usize>> {
    let n = times.len();
    if n <= max {
        return vec![0..n];
    }
    let max = max.max(3);
    let edge = (max / 8).max(1);
    let middle = max - 2 * edge;
    let (lo, hi) = (edge, n - edge);
    let mut best = lo;
    let mut best_span = i64::MAX;
    for i in lo..=hi - middle {
        let span = times[i + middle - 1] - times[i];
        if span < best_span {
            best_span = span;
            best = i;
        }
    }
    let mut out: Vec<Range<usize>> = Vec::new();
    for r in [0..edge, best..best + middle, hi..n] {
        match out.last_mut() {
            Some(last) if last.end >= r.start => last.end = last.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// The prompt, and what it covers.
#[derive(Clone, Debug)]
pub struct Prompt {
    pub text: String,
    pub sent: usize,
    pub total: usize,
    pub trimmed: bool,
}

/// What a summary covers: one stretch in one channel, or every stretch of a period.
#[derive(Clone, Copy, Debug)]
pub enum Scope<'a> {
    Stretch { channel_name: &'a str },
    Period { label: &'a str, stretches: usize },
}

/// The instructions every summary is written under. Kept whole so the tests can
/// hold it to its promises.
pub const INSTRUCTIONS: &str = "\
You are helping the moderators of MLCI, an Indian Discord server, understand an argument between members. \
A moderator asked for this. They will read the actual messages as well and decide for themselves; your job is to \
make the messages easier to follow, not to judge anyone.

About this server: members write in Hinglish - Hindi and English mixed, mostly in Roman script, full of slang - and \
roast each other constantly (bakchodi). Read it the way a member of the server would. Gaalis used as punctuation, \
friendly abuse, \"bhai chup kar\", memes and loud banter are normal here and are not a fight by themselves: read \
banter as banter, and say so when that is what you see. Do not translate insults literally out of context, and do \
not treat a swear word as serious unless it is aimed at someone to hurt them.

Some messages carry a marker in square brackets after the name:
- [DELETED ...] - the message was posted and seen in the channel, and removed later. The marker says how much later \
and by whom when that is known (a moderator, a bot, or probably the author themselves). Others may have read it and \
reacted to it.
- [BLOCKED BY AUTOMOD ...] - the person tried to send this and Discord's AutoMod stopped it. It never appeared in the \
channel: nobody there saw it, so nobody replied to it or reacted to it. It still shows what the person tried to say.
Both are part of what happened. Include them in the timeline and in the flags like any other message, say plainly \
when a message was deleted or blocked if it matters, and never write as if anyone responded to a blocked message.

Rules:
1. Be neutral. Do not say who was right or who won, do not take sides, do not assign blame, and do not guess at \
anyone's feelings, motives, character or mental state. Describe; do not diagnose.
2. Keep what was said separate from interpretation. The trigger, timeline, positions, others and ending report what \
people said and did, paraphrased fairly in plain English. Anything that is your reading between the lines goes only \
in \"interpretation\", hedged (\"seems\", \"may\"), and that list may be empty.
3. Give every person named below the same care. Paraphrase each person's main points the way they would put them, \
strongest version first, in similar length.
4. Refer to messages by their number, like [#12]. Only use numbers that appear below.
5. Flag anything a moderator may need to act on, each with the exact message number: slurs (caste, religious, \
regional, racial, sexual orientation, disability), threats of harm, sharing someone's personal information (real \
name, phone number, address, photos, school or workplace), sexual content aimed at a person, and harassment that \
continued after someone asked for it to stop. A deleted or blocked message is flagged the same way as any other. \
Ordinary swearing and roasting are not flags. If there is nothing, return an empty list: do not invent flags to \
seem thorough.
6. Write in plain English. Quote Hinglish only when the exact words matter, with a short translation in brackets.

Reply with JSON only, in exactly this shape:
{
  \"overview\": \"two or three neutral sentences: who, where, and roughly what it was about\",
  \"trigger\": \"what set it off, with message numbers\",
  \"timeline\": [{\"time\": \"HH:MM\", \"what\": \"what was said or done at this point\", \"refs\": [12, 14]}],
  \"positions\": [{\"who\": \"name\", \"points\": [\"a main point of theirs, paraphrased fairly\"]}],
  \"others\": \"whether anyone else joined in, who, and how (sided with someone, tried to calm it, joked); or \\\"No one else joined in.\\\"\",
  \"ending\": {\"state\": \"resolved | fizzled | ongoing\", \"what\": \"how it ended, with message numbers\"},
  \"interpretation\": [\"a hedged reading, clearly your interpretation\"],
  \"flags\": [{\"kind\": \"slur | threat | personal_info | sexual | harassment_after_stop\", \"who\": \"name\", \"message\": 12, \"what\": \"one line on what was said\"}]
}
Keep the timeline to at most 12 points, in order, with the time of the message each point starts at. \
\"positions\" has one entry for each person named below (A, B, …), in that order, even one who said little.";

fn flat(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > MAX_TEXT_CHARS {
        format!("{}… [cut, {} characters in all]", one.chars().take(MAX_TEXT_CHARS).collect::<String>(), one.chars().count())
    } else {
        one
    }
}

/// `<@123>` as `@name` for everyone the stretch knows.
fn named(text: &str, names: &HashMap<u64, String>) -> String {
    let mut out = text.to_string();
    for (id, name) in names {
        out = out.replace(&format!("<@{}>", id), &format!("@{}", name)).replace(&format!("<@!{}>", id), &format!("@{}", name));
    }
    out
}

/// "3 min", "under a minute", "2 hours": how long after it was sent.
pub fn later(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s < 60 {
        "under a minute".into()
    } else if s < 3600 {
        format!("{} min", (s + 30) / 60)
    } else if s < 86_400 {
        let h = (s + 1800) / 3600;
        format!("{} hour{}", h, if h == 1 { "" } else { "s" })
    } else {
        let d = (s + 43_200) / 86_400;
        format!("{} day{}", d, if d == 1 { "" } else { "s" })
    }
}

/// The marker a deleted or blocked message carries in the prompt, or nothing.
pub fn marker(row: &SaidRow, people: &HashMap<u64, String>) -> Option<String> {
    match row.gone.as_ref()? {
        Gone::Deleted { deleted_ms, checked, by, bulk } => {
            let who = match by {
                Some(d) if d.id == row.author_id => " by the author".to_string(),
                Some(d) if d.bot => format!(" by {} (a bot)", d.name),
                Some(d) => format!(" by {} (a moderator)", people.get(&d.id).cloned().unwrap_or_else(|| d.name.clone())),
                None if *bulk => ", in a bulk delete by a moderator or bot".to_string(),
                None if *checked => ", probably by the author themselves".to_string(),
                None => ", by someone unknown".to_string(),
            };
            Some(format!("[DELETED {} later{}]", later(deleted_ms - row.created_ms), who))
        }
        Gone::Blocked { rule, .. } => Some(match rule {
            Some(rule) => format!("[BLOCKED BY AUTOMOD, rule \"{}\" - never shown in the channel]", rule),
            None => "[BLOCKED BY AUTOMOD - never shown in the channel]".to_string(),
        }),
    }
}

/// The whole prompt for one stretch, or for every stretch of a period.
pub fn build_prompt(names: &[String], scope: Scope, lines: &[Line], max: usize) -> Prompt {
    let total = lines.len();
    let times: Vec<i64> = lines.iter().map(|l| l.row.created_ms).collect();
    let ranges = pick(&times, max);
    let sent: usize = ranges.iter().map(|r| r.len()).sum();
    let trimmed = sent < total;

    let mut known: HashMap<u64, String> = HashMap::new();
    for l in lines {
        known.entry(l.row.author_id).or_insert_with(|| l.row.author_name.clone());
    }
    let label = |l: &Line| match l.side.letter() {
        Some(c) => format!("{} ({})", l.row.author_name, c),
        None => l.row.author_name.clone(),
    };
    let mut others: Vec<String> = Vec::new();
    for l in lines.iter().filter(|l| l.side == Side::Other) {
        if !others.contains(&l.row.author_name) {
            others.push(l.row.author_name.clone());
        }
    }
    let (deleted, blocked) = (lines.iter().filter(|l| l.row.deleted()).count(), lines.iter().filter(|l| l.row.blocked()).count());

    let mut text = String::with_capacity(INSTRUCTIONS.len() + sent * 120);
    text.push_str(INSTRUCTIONS);
    text.push_str("\n\n---\n\n");
    let who: Vec<String> = names.iter().enumerate().map(|(i, n)| format!("{} = {}", Side::Person(i).letter().unwrap_or('?'), n)).collect();
    text.push_str(&format!("The {} people: {}.\n", if names.len() == 2 { "two".to_string() } else { names.len().to_string() }, who.join(", ")));
    text.push_str(&if others.is_empty() { "No one else spoke.\n".to_string() } else { format!("Others who spoke: {}.\n", others.join(", ")) });
    let multi_channel = lines.iter().any(|l| lines.first().is_some_and(|f| f.row.channel_id != l.row.channel_id));
    match scope {
        Scope::Stretch { channel_name } => text.push_str(&format!("Channel: #{}. Times are India time (IST).\n", channel_name)),
        Scope::Period { label, stretches } => {
            let mut chans: Vec<String> = Vec::new();
            for l in lines {
                let c = format!("#{}", l.row.channel_name);
                if !chans.contains(&c) {
                    chans.push(c);
                }
            }
            text.push_str(&format!(
                "This is everything they said to each other over a period ({}): {} separate stretches, put together in time order. \
                 Stretches can be hours apart; treat it as one story only where it really continues. Channels: {}. \
                 Times are India time (IST).\n",
                label,
                stretches,
                chans.join(", ")
            ));
        }
    }
    if let (Some(first), Some(last)) = (lines.first(), lines.last()) {
        text.push_str(&format!("{} messages, {}.\n", total, span(first.row.created_ms, last.row.created_ms)));
    }
    if deleted + blocked > 0 {
        text.push_str(&format!("Of these, {} were deleted later and {} were blocked by AutoMod; they are marked.\n", deleted, blocked));
    }
    if trimmed {
        text.push_str(&format!(
            "This was too long to show whole: you are shown {} of its {} messages - how it started, the busiest part, \
             and how it ended. Gaps are marked. Say in the overview that you saw only part of it, and do not guess at \
             what was left out.\n",
            sent, total
        ));
    }
    text.push_str("\nMessages, numbered in order (\"↪ #9\" means a reply to message 9):\n");
    let mut last_day = String::new();
    let mut last_channel = None;
    let mut next = 0usize;
    for r in &ranges {
        if r.start > next {
            text.push_str(&format!("[… {} messages left out …]\n", r.start - next));
        }
        for l in &lines[r.clone()] {
            if multi_channel && last_channel != Some(l.row.channel_id) {
                text.push_str(&format!("== in #{} ==\n", l.row.channel_name));
                last_channel = Some(l.row.channel_id);
            }
            let d = day(l.row.created_ms);
            if d != last_day {
                text.push_str(&format!("-- {} --\n", d));
                last_day = d;
            }
            let reply = match (l.reply_n, l.row.reply_to) {
                (Some(n), _) => format!(" ↪ #{}", n),
                (None, Some(_)) => {
                    let who = l.row.reply_author.clone().unwrap_or_else(|| "someone".into());
                    let what = l.row.reply_text.as_deref().map(|t| format!(": \"{}\"", flat(t))).unwrap_or_default();
                    format!(" ↪ an earlier message by {}{}", who, what)
                }
                _ => String::new(),
            };
            let mark = marker(l.row, &known).map(|m| format!(" {}", m)).unwrap_or_default();
            let mut body = flat(&named(&l.row.content, &known));
            // What was attached, by name: the model is told a picture was posted, never shown it.
            for a in &l.row.attachments {
                let what = if msglog::is_sticker(a) {
                    "sticker"
                } else if msglog::image_ext(a.content_type.as_deref(), &a.filename).is_some() {
                    "image"
                } else {
                    "file"
                };
                body = format!("{}{}[{}: {}]", body, if body.is_empty() { "" } else { " " }, what, a.filename);
            }
            text.push_str(&format!("#{} [{}] {}{}{}: {}\n", l.n, clock(l.row.created_ms), label(l), mark, reply, body));
        }
        next = r.end;
    }
    if next < total {
        text.push_str(&format!("[… {} messages left out …]\n", total - next));
    }
    Prompt { text, sent, total, trimmed }
}

// --- reading the answer ------------------------------------------------------------------------------

const FLAG_KINDS: [&str; 5] = ["slur", "threat", "personal_info", "sexual", "harassment_after_stop"];
const ENDINGS: [&str; 3] = ["resolved", "fizzled", "ongoing"];

fn text_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).map(str::trim).unwrap_or("").to_string()
}

fn number_in(v: &Value, total: usize) -> Option<usize> {
    let n = v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().trim_start_matches('#').parse().ok()))? as usize;
    (1..=total).contains(&n).then_some(n)
}

/// The model's answer in the page's shape, or none when it holds no JSON
/// object. Message numbers outside the stretch are dropped.
pub fn parse_summary(raw: &str, total: usize) -> Option<Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let v: Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let o = v.as_object()?;
    let list = |k: &str| o.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let timeline: Vec<Value> = list("timeline")
        .iter()
        .filter_map(|t| {
            let what = text_of(t.get("what"));
            (!what.is_empty()).then(|| {
                let refs: Vec<usize> = t.get("refs").and_then(Value::as_array).map(|r| r.iter().filter_map(|x| number_in(x, total)).collect()).unwrap_or_default();
                json!({ "time": text_of(t.get("time")), "what": what, "refs": refs })
            })
        })
        .take(20)
        .collect();
    let positions: Vec<Value> = list("positions")
        .iter()
        .filter_map(|p| {
            let points: Vec<String> = p.get("points").and_then(Value::as_array).map(|x| x.iter().map(|s| text_of(Some(s))).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
            let who = text_of(p.get("who"));
            (!who.is_empty()).then(|| json!({ "who": who, "points": points }))
        })
        .collect();
    let flags: Vec<Value> = list("flags")
        .iter()
        .filter_map(|f| {
            let kind = text_of(f.get("kind")).to_lowercase().replace([' ', '-'], "_");
            let what = text_of(f.get("what"));
            if what.is_empty() && kind.is_empty() {
                return None;
            }
            Some(json!({
                "kind": if FLAG_KINDS.contains(&kind.as_str()) { kind } else { "other".to_string() },
                "who": text_of(f.get("who")),
                "message": f.get("message").and_then(|m| number_in(m, total)),
                "what": what,
            }))
        })
        .collect();
    let ending = o.get("ending");
    let state = text_of(ending.and_then(|e| e.get("state"))).to_lowercase();
    let mut out = Map::new();
    out.insert("overview".into(), json!(text_of(o.get("overview"))));
    out.insert("trigger".into(), json!(text_of(o.get("trigger"))));
    out.insert("timeline".into(), json!(timeline));
    out.insert("positions".into(), json!(positions));
    out.insert("others".into(), json!(text_of(o.get("others"))));
    out.insert(
        "ending".into(),
        json!({ "state": if ENDINGS.contains(&state.as_str()) { state } else { "unclear".to_string() }, "what": text_of(ending.and_then(|e| e.get("what"))) }),
    );
    out.insert("interpretation".into(), json!(list("interpretation").iter().map(|s| text_of(Some(s))).filter(|s| !s.is_empty()).collect::<Vec<_>>()));
    out.insert("flags".into(), json!(flags));
    Some(Value::Object(out))
}

// --- asking the model ----------------------------------------------------------------------------------

/// What the model said and what it cost.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,
}

/// Why a model call failed, in words a moderator can act on.
pub fn why_failed(err: &str) -> &'static str {
    let e = err.to_ascii_lowercase();
    if e.contains("took over") || e.contains("timed out") || e.contains("timeout") {
        "it took too long to answer"
    } else if e.contains("error sending request") || e.contains("connect") || e.contains("dns") || e.contains("http client error") || e.contains("connection") {
        "network error"
    } else if e.contains("empty") {
        "it sent back nothing"
    } else if e.contains("429") || e.contains("rate") {
        "the provider is rate-limiting us"
    } else {
        "the provider returned an error"
    }
}

/// One answer from the model, tried up to [`TRIES`] times with a wait between:
/// the provider does fail once now and then, and a second try usually works. An
/// empty reply counts as a failure. The error is the reason, in words.
///
/// Every feature that writes with the model comes through here — the panel's
/// summaries, the deep dives, the nightly topic pass, the nightly scan — so a
/// failure means the same thing, and costs the same patience, everywhere.
pub async fn ask_retrying<F, Fut>(what: &str, ask: F) -> Result<Reply, String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Reply>>,
{
    let mut last = String::new();
    for attempt in 1..=TRIES {
        match ask().await {
            Ok(reply) if !reply.text.trim().is_empty() => return Ok(reply),
            Ok(_) => {
                last = "the model sent back an empty reply".into();
                tracing::warn!("{}: try {} of {} came back empty", what, attempt, TRIES);
            }
            Err(err) => {
                last = err.to_string();
                tracing::warn!("{}: try {} of {} failed: {}", what, attempt, TRIES, err);
            }
        }
        if let Some(wait) = RETRY_WAITS.get(attempt - 1) {
            tokio::time::sleep(*wait).await;
        }
    }
    Err(last)
}

/// One call to the summary model: `VIZIER_KALESH_SUMMARY_MODEL` on the bot's
/// own provider, or the bot's main model when that is empty. Providers that
/// don't report usage get an estimate from the text.
pub async fn ask_live(prompt: String) -> anyhow::Result<Reply> {
    ask_live_as(prompt, summary_model()).await
}

/// The same, on a named model of the caller's choosing: what lets the nightly
/// passes put a whole day through a cheap one without touching the model a
/// moderator's own summaries use.
pub async fn ask_live_as(prompt: String, name: Option<String>) -> anyhow::Result<Reply> {
    use crate::agents::agent::model::{VizierModel, VizierModelTrait};
    use crate::storage::agent::AgentStorage;
    use rig_core::message::{AssistantContent, Message as ModelMessage};

    let (deps, agent_id) = control::web::bot_agent().ok_or_else(|| anyhow::anyhow!("the agent isn't reachable"))?;
    let config = deps.storage.get_agent(agent_id).await?.ok_or_else(|| anyhow::anyhow!("no config for {}", agent_id))?;
    let named = name.clone().map(|n| (config.provider.clone(), n));
    let model = VizierModel::new_with_override(deps, &config, named).await?;
    let prompt_tokens = super::notes_build::estimate_tokens(&prompt) as u64;
    let (_, choice, usage) = tokio::time::timeout(MODEL_WAIT, model.completion(ModelMessage::user(prompt), vec![], vec![]))
        .await
        .map_err(|_| anyhow::anyhow!("the model took over {}s", MODEL_WAIT.as_secs()))??;
    let text: String = choice
        .iter()
        .filter_map(|c| match c {
            AssistantContent::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("");
    let (input_tokens, output_tokens) = if usage.input_tokens + usage.output_tokens > 0 {
        (usage.input_tokens, usage.output_tokens)
    } else {
        (prompt_tokens, super::notes_build::estimate_tokens(&text) as u64)
    };
    Ok(Reply { text, input_tokens, output_tokens, model: name.unwrap_or(config.model) })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

    pub fn id_at(ms: i64, seq: u64) -> u64 {
        (((ms - DISCORD_EPOCH_MS) as u64) << 22) | seq
    }

    pub fn row(ms: i64, seq: u64, author: u64, name: &str, channel: u64, text: &str, reply_to: Option<u64>) -> SaidRow {
        SaidRow {
            message_id: id_at(ms, seq),
            channel_id: channel,
            parent_id: None,
            channel_name: format!("ch{}", channel),
            author_id: author,
            author_name: name.into(),
            avatar: String::new(),
            content: text.into(),
            created_ms: ms,
            reply_to,
            reply_author: None,
            reply_text: None,
            attachments: vec![],
            images: vec![],
            gone: None,
        }
    }

    fn pair() -> Vec<String> {
        vec!["gooner".to_string(), "potus".to_string()]
    }

    const A: u64 = 711;
    const B: u64 = 935;
    const T0: i64 = 1_790_000_000_000;
    const MIN: i64 = 60_000;

    #[test]
    fn replies_both_ways_make_one_stretch_and_distant_talk_does_not() {
        let a1 = row(T0, 1, A, "gooner", 5, "RCB is the best team, period", None);
        let b1 = row(T0 + 2 * MIN, 2, B, "potus", 5, "lol no", Some(a1.message_id));
        let a2 = row(T0 + 9 * MIN, 3, A, "gooner", 5, "tu chup kar", Some(b1.message_id));
        let b2 = row(T0 + 16 * MIN, 4, B, "potus", 5, "stats dekh pehle", Some(a2.message_id));
        // Hours later in another channel, nowhere near each other: not engaging.
        let a3 = row(T0 + 5 * 60 * MIN, 5, A, "gooner", 6, "gm", None);
        let b3 = row(T0 + 7 * 60 * MIN, 6, B, "potus", 6, "gn", None);
        let rows = vec![b2.clone(), a1.clone(), a3, b1, a2, b3];
        let found = find_stretches(&rows, &[A, B]);
        assert_eq!(found.len(), 1, "{found:?}");
        let s = &found[0];
        assert_eq!((s.channel_id, s.start_ms, s.end_ms), (5, T0, T0 + 16 * MIN), "the reply pulls the first message in");
        assert_eq!((s.replies_from(0, 1), s.replies_from(1, 0), s.per_person[0], s.per_person[1]), (1, 2, 2, 2));
    }

    #[test]
    fn a_mention_counts_and_so_does_talking_within_minutes() {
        // A lone mention is enough to list a stretch.
        let m = row(T0, 1, A, "gooner", 5, "<@935> come to vc and say that", None);
        let found = find_stretches(&[m], &[A, B]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].mentions, 1);
        // Two each within a few minutes in the same channel, without a reply.
        let near = vec![
            row(T0, 1, A, "gooner", 7, "kohli overrated hai", None),
            row(T0 + MIN, 2, B, "potus", 7, "kya bol raha hai", None),
            row(T0 + 3 * MIN, 3, A, "gooner", 7, "sach bol raha hu", None),
            row(T0 + 4 * MIN, 4, B, "potus", 7, "clown", None),
        ];
        let found = find_stretches(&near, &[A, B]);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].nearby, found[0].mentions), (4, 0));
        // One each, minutes apart, with no reply or mention: not enough.
        assert!(find_stretches(&near[..2], &[A, B]).is_empty());
        // Too far apart in time to be "near".
        let far = vec![row(T0, 1, A, "g", 7, "x", None), row(T0 + 30 * MIN, 2, B, "p", 7, "y", None), row(T0 + 60 * MIN, 3, A, "g", 7, "x", None), row(T0 + 90 * MIN, 4, B, "p", 7, "y", None)];
        assert!(find_stretches(&far, &[A, B]).is_empty());
    }

    #[test]
    fn a_long_quiet_gap_splits_a_channel_into_two_stretches_newest_first() {
        let rows = vec![
            row(T0, 1, A, "g", 5, "<@935> oi", None),
            row(T0 + 3 * 60 * MIN, 2, B, "p", 5, "<@711> hello?", None),
        ];
        let found = find_stretches(&rows, &[A, B]);
        assert_eq!(found.len(), 2);
        assert!(found[0].start_ms > found[1].start_ms);
    }

    #[test]
    fn bystanders_are_in_the_exchange_and_marked_by_who_they_address() {
        let a1 = row(T0, 1, A, "gooner", 5, "RCB best", None);
        let b1 = row(T0 + MIN, 2, B, "potus", 5, "cope", Some(a1.message_id));
        let x1 = row(T0 + 2 * MIN, 3, 42, "rahul", 5, "bhai dono chill karo", None);
        let x2 = row(T0 + 3 * MIN, 4, 43, "kavya", 5, "potus is right tbh", Some(b1.message_id));
        let x3 = row(T0 + 4 * MIN, 5, 43, "kavya", 5, "<@711> accept it", None);
        let a2 = row(T0 + 5 * MIN, 6, A, "gooner", 5, "<@935> tu toh chup hi reh", None);
        let rows = vec![x3.clone(), a1, b1, x1, x2, a2];
        let lines = exchange(&rows, &[A, B]);
        let summary: Vec<(usize, Side, Option<Side>, Option<usize>)> = lines.iter().map(|l| (l.n, l.side, l.towards, l.reply_n)).collect();
        assert_eq!(
            summary,
            vec![
                (1, Side::A, None, None),
                (2, Side::B, Some(Side::A), Some(1)),
                (3, Side::Other, None, None),
                (4, Side::Other, Some(Side::B), Some(2)),
                (5, Side::Other, Some(Side::A), None),
                (6, Side::A, Some(Side::B), None),
            ]
        );
        let key = stretch_key(&[B, A], 5, &lines);
        assert_eq!(key, stretch_key(&[A, B], 5, &lines), "the pair is the same whichever way round it is asked");
    }

    #[test]
    fn a_stretch_that_fits_is_sent_whole() {
        let rows: Vec<SaidRow> = (0..30).map(|i| row(T0 + i * MIN, i as u64, if i % 2 == 0 { A } else { B }, if i % 2 == 0 { "gooner" } else { "potus" }, 5, "text", None)).collect();
        let lines = exchange(&rows, &[A, B]);
        let p = build_prompt(&pair(), Scope::Stretch { channel_name: "chatting" }, &lines, 400);
        assert_eq!((p.sent, p.total, p.trimmed), (30, 30, false));
        assert!(!p.text.contains("left out"));
        assert!(!p.text.contains("too long to show whole"));
        assert!(p.text.contains("#1 [") && p.text.contains("#30 ["));
    }

    #[test]
    fn a_long_stretch_keeps_its_start_its_busiest_part_and_its_end_and_says_so() {
        // 600 messages: slow at first, a dense burst in the middle, slow again.
        let mut ms = T0;
        let rows: Vec<SaidRow> = (0..600u64)
            .map(|i| {
                ms += if (250..450).contains(&i) { 5_000 } else { 60_000 };
                row(ms, i, if i % 2 == 0 { A } else { B }, if i % 2 == 0 { "gooner" } else { "potus" }, 5, &format!("message {}", i), None)
            })
            .collect();
        let lines = exchange(&rows, &[A, B]);
        let p = build_prompt(&pair(), Scope::Stretch { channel_name: "chatting" }, &lines, 400);
        assert_eq!((p.sent, p.total, p.trimmed), (400, 600, true));
        assert!(p.text.contains("you are shown 400 of its 600 messages"), "the prompt says it was trimmed");
        assert!(p.text.contains("messages left out"));
        assert!(p.text.contains("#1 [") && p.text.contains("#600 ["), "the start and the end are kept");
        // The burst (messages 251-450) is all in.
        for n in [251, 300, 450] {
            assert!(p.text.contains(&format!("#{} [", n)), "#{n} from the busiest part");
        }
        let ranges = pick(&lines.iter().map(|l| l.row.created_ms).collect::<Vec<_>>(), 400);
        assert_eq!(ranges.iter().map(|r| r.len()).sum::<usize>(), 400);
        assert_eq!(ranges.first().unwrap().start, 0);
        assert_eq!(ranges.last().unwrap().end, 600);
    }

    #[test]
    fn the_prompt_asks_for_neutrality_reads_hinglish_and_names_the_flags() {
        let a1 = row(T0, 1, A, "gooner", 5, "<@935> bhai tu pagal hai kya", None);
        let b1 = row(T0 + MIN, 2, B, "potus", 5, "tu hoga", Some(a1.message_id));
        let rows = vec![a1, b1];
        let lines = exchange(&rows, &[A, B]);
        let p = build_prompt(&pair(), Scope::Stretch { channel_name: "🥳chatting-hori" }, &lines, 400);
        for must in [
            "Be neutral",
            "Do not say who was right",
            "do not take sides",
            "mental state",
            "separate from interpretation",
            "Hinglish",
            "read banter as banter",
            "slurs",
            "threats",
            "personal information",
            "sexual content aimed at a person",
            "after someone asked for it to stop",
            "If there is nothing, return an empty list",
            "India time",
            "A = gooner, B = potus",
            "#🥳chatting-hori",
        ] {
            assert!(p.text.contains(must), "the prompt lacks {must:?}");
        }
        // Mentions read as names, replies as numbers.
        assert!(p.text.contains("@potus bhai tu pagal hai kya"), "{}", p.text);
        assert!(p.text.contains("#2 [") && p.text.contains("potus (B) ↪ #1: tu hoga"), "{}", p.text);
    }

    #[test]
    fn answers_are_read_and_bad_numbers_dropped() {
        let raw = "Here you go:\n```json\n{\"overview\":\"x\",\"trigger\":\"t [#1]\",\"timeline\":[{\"time\":\"21:04\",\"what\":\"w\",\"refs\":[1,99,\"#2\"]}],\
                   \"positions\":[{\"who\":\"gooner\",\"points\":[\"p\"]}],\"others\":\"none\",\"ending\":{\"state\":\"Fizzled\",\"what\":\"e\"},\
                   \"interpretation\":[],\"flags\":[{\"kind\":\"personal info\",\"who\":\"potus\",\"message\":2,\"what\":\"shared a number\"},{\"kind\":\"threat\",\"message\":400,\"what\":\"x\"}]}\n```";
        let v = parse_summary(raw, 2).unwrap();
        assert_eq!(v["timeline"][0]["refs"], json!([1, 2]));
        assert_eq!(v["ending"]["state"], "fizzled");
        assert_eq!(v["flags"][0]["kind"], "personal_info");
        assert_eq!(v["flags"][0]["message"], 2);
        assert!(v["flags"][1]["message"].is_null(), "a number outside the stretch is dropped");
        assert!(parse_summary("no json here", 2).is_none());
    }

    #[test]
    fn a_detection_names_its_people_busiest_first() {
        let seen = |id: u64, s: i64, author: u64, name: &str| Seen { id, ts_ms: T0 + s * 1000, author, author_name: name.into() };
        let window = vec![seen(1, 0, A, "gooner"), seen(2, 5, B, "potus"), seen(3, 9, B, "potus"), seen(4, 20, 42, "rahul"), seen(5, 30, B, "potus")];
        let d = detection_of(23, &window, "popcorn time", 99).unwrap();
        assert_eq!(d.participants.iter().map(|p| (p.id, p.messages)).collect::<Vec<_>>(), vec![(B, 3), (A, 1), (42, 1)]);
        assert_eq!((d.start_ms, d.end_ms, d.message_ids.len()), (T0, T0 + 30_000, 5));
        assert!(detection_of(23, &[], "x", 0).is_none());
    }

    /// When the live detector's model says fight, the burst is recorded; when it
    /// says banter, nothing is.
    #[tokio::test]
    async fn the_live_detector_records_a_fight_and_not_banter() {
        if store::db().is_none() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("kalesh.db");
            std::mem::forget(dir);
            store::open_at(&path).unwrap();
        }
        let channel = 880_001;
        let count = || store::detections(&store::db().unwrap().lock(), 10_000).unwrap().iter().filter(|d| d.channel_id == channel).count();
        let window = vec![
            Seen { id: 11, ts_ms: T0, author: A, author_name: "gooner".into() },
            Seen { id: 12, ts_ms: T0 + 4_000, author: B, author_name: "potus".into() },
        ];
        let before = count();
        let said = judge_and_record(channel, window.clone(), async { None }).await;
        assert!(said.is_none());
        assert_eq!(count(), before, "banter is not recorded");
        let said = judge_and_record(channel, window, async { Some("kalesh 🍿".to_string()) }).await;
        assert_eq!(said.as_deref(), Some("kalesh 🍿"));
        assert_eq!(count(), before + 1);
        let d = store::detections(&store::db().unwrap().lock(), 10_000).unwrap().into_iter().find(|d| d.channel_id == channel).unwrap();
        assert_eq!((d.message_ids.clone(), d.line.as_str(), d.participants.len()), (vec![11, 12], "kalesh 🍿", 2));
    }
}
