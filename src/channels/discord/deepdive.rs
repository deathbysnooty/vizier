//! The Deep dive: one member over one period, read in one go.
//!
//! A mod picks somebody — here or long gone — and a period, and gets everything
//! the bot kept about them in that window: what they said and where, the voice
//! rooms they sat in and who was in there with them, the plain shape of their
//! days, and, on a button, a few paragraphs from the model saying what they have
//! been up to.
//!
//! This module is the part with no web in it: the period, the voice sessions
//! worked out from the raw voice log, the prompt the model is asked, and the
//! reading of what it says back. The summary is stored by `kalesh_store` under
//! [`SCOPE_MEMBER`], so the same window is never paid for twice; the model call,
//! its retries and its settings are the Kalesh ones.
//!
//! Nothing here ever sees #safe-corner: the messages come from `msglog`, which
//! never kept a word of it.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::kalesh::{Prompt, Reply, marker, pick};
use super::kalesh_store::NewSummary;
use super::msglog::SaidRow;

/// A summary of one member over one period, as `kalesh_store` files it.
pub const SCOPE_MEMBER: &str = "member";

/// The periods the page offers, in days.
pub const PERIODS: [(&str, i64); 4] = [("3", 3), ("7", 7), ("14", 14), ("30", 30)];
/// The one it opens on.
pub const DEFAULT_DAYS: i64 = 7;
/// The longest custom range: past this it stops being a deep dive.
pub const MAX_RANGE_DAYS: i64 = 90;
/// Messages one deep dive reads at most. A busy member over 90 days is well
/// inside it, and the page says so when it isn't.
pub const MAX_ROWS: usize = 6_000;
/// How long two people have to overlap in a room to be listed as company.
pub const TOGETHER_SECS: i64 = 60;
/// The most people listed as company in one voice session.
pub const MAX_COMPANY: usize = 8;
/// How much of a message the model is shown.
pub const MAX_TEXT_CHARS: usize = 600;

const DAY: i64 = 86_400;

// --- the period ----------------------------------------------------------------------------

/// The window a deep dive covers, and how it was arrived at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Period {
    pub from: i64,
    pub to: i64,
    /// True when the window was moved back off `now` to where they last spoke,
    /// because they are gone and the plain period would have been empty.
    pub shifted: bool,
}

impl Period {
    pub fn days(&self) -> i64 {
        ((self.to - self.from) + DAY - 1) / DAY
    }
}

/// The window to read. Normally it is the last `days` up to now. For somebody
/// who has left, `now` is long past anything they ever did, so it ends instead
/// at the last thing the bot saw of them — their last kept message, or failing
/// that the moment they went — and says it moved.
pub fn period(now: i64, days: i64, gone_at: Option<i64>, last_seen: Option<i64>) -> Period {
    let plain = Period { from: now - days * DAY, to: now, shifted: false };
    if gone_at.is_none() {
        return plain;
    }
    // The last thing the bot saw of them: the later of their last kept message
    // and the moment they went, so the window holds their last days here.
    let anchor = match (last_seen, gone_at) {
        (Some(seen), Some(left)) => seen.max(left),
        (None, Some(left)) => left,
        _ => return plain,
    }
    .min(now);
    // Already inside the plain window: nothing to move.
    if anchor >= plain.from {
        return plain;
    }
    Period { from: anchor - days * DAY, to: anchor, shifted: true }
}

/// "last 7 days", or the two dates of a custom range: what the audit log and
/// the prompt call this window.
pub fn period_words(p: &Period, custom: bool) -> String {
    let date = |ts: i64| {
        chrono::DateTime::from_timestamp(ts, 0)
            .map(|t| t.with_timezone(&chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap()).format("%-d %b %Y").to_string())
            .unwrap_or_default()
    };
    if custom || p.shifted {
        format!("{} to {}", date(p.from), date(p.to))
    } else {
        format!("last {} days", p.days())
    }
}

// --- voice ---------------------------------------------------------------------------------

/// One stretch somebody spent in a voice room.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct VoiceSession {
    pub channel_id: u64,
    pub start: i64,
    pub end: i64,
    pub secs: i64,
    /// Who overlapped with them for at least [`TOGETHER_SECS`], longest first.
    pub with: Vec<(u64, i64)>,
}

/// One person's stretch in a room, as the voice log keeps it. An open stretch
/// (still in the room) ends at `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoiceStay {
    pub user_id: u64,
    pub channel_id: u64,
    pub start: i64,
    pub end: i64,
}

/// One member's voice sessions in the window, with the company each one had.
///
/// `stays` is every stretch in every room over the window, everybody's, which
/// is what the voice log holds. Company is worked out from the overlap in the
/// same room, so two people who were never in at the same time are never paired.
pub fn sessions(stays: &[VoiceStay], member: u64, from: i64, to: i64) -> Vec<VoiceSession> {
    let clip = |s: &VoiceStay| (s.start.max(from), s.end.min(to));
    let mut mine: Vec<&VoiceStay> = stays.iter().filter(|s| s.user_id == member && s.end > from && s.start < to).collect();
    mine.sort_by_key(|s| s.start);
    mine.iter()
        .filter_map(|s| {
            let (start, end) = clip(s);
            if end <= start {
                return None;
            }
            let mut company: HashMap<u64, i64> = HashMap::new();
            for other in stays.iter().filter(|o| o.user_id != member && o.channel_id == s.channel_id) {
                let overlap = other.end.min(end) - other.start.max(start);
                if overlap > 0 {
                    *company.entry(other.user_id).or_insert(0) += overlap;
                }
            }
            let mut with: Vec<(u64, i64)> = company.into_iter().filter(|(_, secs)| *secs >= TOGETHER_SECS).collect();
            with.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            with.truncate(MAX_COMPANY);
            Some(VoiceSession { channel_id: s.channel_id, start, end, secs: end - start, with })
        })
        .collect()
}

/// "2h 15m", "40m", "under a minute": a stretch of voice in round words.
pub fn spell_secs(secs: i64) -> String {
    if secs < 60 {
        return "under a minute".into();
    }
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    match (h, m) {
        (0, m) => format!("{}m", m),
        (h, 0) => format!("{}h", h),
        (h, m) => format!("{}h {}m", h, m),
    }
}

// --- the shape of their days ------------------------------------------------------------------

/// The plain counts the page draws: no model, no reading between lines.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Shape {
    /// India day and how many messages, every day of the window, oldest first.
    pub per_day: Vec<(String, i64)>,
    /// Channel id and messages, busiest first.
    pub channels: Vec<(u64, i64)>,
    /// Messages per India hour, 24 entries.
    pub hours: Vec<i64>,
    pub messages: i64,
    pub deleted: i64,
    pub blocked: i64,
    /// Their longest message and their average, in characters.
    pub avg_len: i64,
}

/// [`Shape`] from the member's own rows. `day_of` turns a moment into the
/// India day the rest of the panel counts by, so a deep dive and the member
/// page never disagree about which day something fell on.
pub fn shape(rows: &[&SaidRow], from: i64, to: i64, day_of: impl Fn(i64) -> String, hour_of: impl Fn(i64) -> i64) -> Shape {
    let mut out = Shape { hours: vec![0; 24], ..Default::default() };
    let mut days: Vec<(String, i64)> = Vec::new();
    let mut day = from;
    while day <= to {
        days.push((day_of(day), 0));
        day += DAY;
    }
    if !days.iter().any(|(d, _)| *d == day_of(to)) {
        days.push((day_of(to), 0));
    }
    let mut channels: HashMap<u64, i64> = HashMap::new();
    let mut chars: i64 = 0;
    for r in rows {
        let ts = r.created_ms / 1000;
        out.messages += 1;
        if r.deleted() {
            out.deleted += 1;
        }
        if r.blocked() {
            out.blocked += 1;
        }
        chars += r.content.chars().count() as i64;
        *channels.entry(r.channel_id).or_insert(0) += 1;
        if let Some(slot) = out.hours.get_mut(hour_of(ts).clamp(0, 23) as usize) {
            *slot += 1;
        }
        let d = day_of(ts);
        match days.iter_mut().find(|(x, _)| *x == d) {
            Some((_, n)) => *n += 1,
            None => days.push((d, 1)),
        }
    }
    days.sort_by(|a, b| a.0.cmp(&b.0));
    out.per_day = days;
    let mut top: Vec<(u64, i64)> = channels.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out.channels = top;
    out.avg_len = if out.messages > 0 { chars / out.messages } else { 0 };
    out
}

// --- what identifies one deep dive's summary ----------------------------------------------------

/// The member, the period, and the exact messages it covered — not the window's
/// own ends, which move by a second between two presses of the button. A window
/// that has grown a message since is a different deep dive, so what comes back
/// out of the store is never stale.
pub fn key(member: u64, days: i64, rows: &[&SaidRow]) -> String {
    let first = rows.first().map(|r| r.message_id).unwrap_or(0);
    let last = rows.last().map(|r| r.message_id).unwrap_or(0);
    format!("{}:{}:{}d:{}-{}:{}", SCOPE_MEMBER, member, days, first, last, rows.len())
}

// --- the prompt ---------------------------------------------------------------------------------

/// The instructions every deep-dive summary is written under. Kept whole so the
/// tests can hold it to its promises.
pub const INSTRUCTIONS: &str = "\
You are helping the moderators of MLCI, an Indian Discord server, catch up on one member. A moderator asked for \
this because they want to know what this person has been up to lately. They will read the messages themselves as \
well; your job is to save them the scrolling, not to judge anyone.

About this server: members write in Hinglish - Hindi and English mixed, mostly in Roman script, full of slang - and \
roast each other constantly (bakchodi). Read it the way a member of the server would. Gaalis used as punctuation, \
friendly abuse and loud banter are normal here and are not a problem by themselves: read banter as banter, and say \
so when that is what you see.

Some messages carry a marker in square brackets after the time:
- [DELETED ...] - posted and seen in the channel, removed later. The marker says how much later and by whom when \
that is known.
- [BLOCKED BY AUTOMOD ...] - Discord's AutoMod stopped it. It never appeared in the channel, so nobody saw it or \
replied to it. It still shows what the person tried to say.
Both are part of the picture. Never write as if anyone responded to a blocked message.

Rules:
1. Be factual and neutral. Describe what this person said and did. Do not judge them, do not guess at their \
character, mental state, health, politics, religion, caste, sexuality or anything else about them as a person, and \
do not speculate about anything they did not say. Describe; do not diagnose.
2. Keep what was said apart from your reading of it. Everything but \"interpretation\" reports what is actually in \
the messages, paraphrased fairly in plain English. Anything you are reading between the lines goes in \
\"interpretation\", hedged (\"seems\", \"may\"), and that list may be empty.
3. Say when there is too little to go on. A handful of messages is a handful of messages: write that in \"thin\" \
rather than spinning a story out of nothing. Never pad.
4. Refer to messages by their number, like [#12]. Only use numbers that appear below.
5. In \"watch\", raise only things a moderator would actually want to know now: an argument brewing or unresolved, \
somebody who has clearly gone quiet, a rule problem (a slur, a threat, sharing someone's personal information, \
sexual content aimed at a person, harassment that carried on after being asked to stop). Give the message number. \
Ordinary swearing and roasting are not a rule problem. If there is nothing, return an empty list: never invent \
something to seem thorough.
6. Write in plain English. Quote Hinglish only where the exact words matter, with a short translation in brackets.
7. PRONOUNS. Never guess, infer or imply anyone's gender - not from their name, not from how they write, not from \
what anyone calls them, not from anything else. Each person's pronouns are given below, from this server's own \
roles. Use exactly those, and use they/them for anybody whose pronouns are not given.

Reply with JSON only, in exactly this shape:
{
  \"overview\": \"two to four neutral sentences: where this person has been, what they were mostly doing, and how much of it there was\",
  \"topics\": [{\"what\": \"something they talked about, paraphrased\", \"refs\": [12, 14]}],
  \"people\": [{\"who\": \"name\", \"how\": \"what the two of them were doing together, in one line\"}],
  \"rhythm\": \"how the period went for them: busier or quieter than it started, when of day they are around, anything that changed\",
  \"places\": \"which channels and voice rooms they spend their time in\",
  \"watch\": [{\"kind\": \"argument | going_quiet | rule_problem | other\", \"what\": \"one or two lines\", \"refs\": [12]}],
  \"thin\": \"one line if there is too little here to say much; otherwise an empty string\",
  \"interpretation\": [\"a hedged reading, clearly your interpretation\"]
}
Keep \"topics\" to at most 8 and \"people\" to at most 8, the ones that actually matter.";

fn flat(text: &str) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > MAX_TEXT_CHARS {
        format!("{}… [cut, {} characters in all]", one.chars().take(MAX_TEXT_CHARS).collect::<String>(), one.chars().count())
    } else {
        one
    }
}

/// `<@123>` as `@name` for everyone the window knows.
fn named(text: &str, names: &HashMap<u64, String>) -> String {
    let mut out = text.to_string();
    for (id, name) in names {
        out = out.replace(&format!("<@{}>", id), &format!("@{}", name)).replace(&format!("<@!{}>", id), &format!("@{}", name));
    }
    out
}

/// The India time of a moment, "Mon 21:40".
fn when(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).unwrap()).format("%a %d %b %H:%M").to_string())
        .unwrap_or_default()
}

/// What the model is told about the voice rooms: who, where, how long, with
/// whom. No content: the bot never records what is said in voice.
fn voice_lines(sessions: &[VoiceSession], rooms: &HashMap<u64, String>, names: &HashMap<u64, String>) -> String {
    if sessions.is_empty() {
        return "They were not in voice at all in this period.\n".to_string();
    }
    let total: i64 = sessions.iter().map(|s| s.secs).sum();
    let mut out = format!("Voice: {} session{}, {} in all.\n", sessions.len(), if sessions.len() == 1 { "" } else { "s" }, spell_secs(total));
    for s in sessions.iter().take(40) {
        let room = rooms.get(&s.channel_id).cloned().unwrap_or_else(|| format!("room {}", s.channel_id));
        let with: Vec<String> = s.with.iter().map(|(id, _)| names.get(id).cloned().unwrap_or_else(|| format!("member {}", id))).collect();
        out.push_str(&format!(
            "- {} in {} for {}{}\n",
            when(s.start * 1000),
            room,
            spell_secs(s.secs),
            if with.is_empty() { ", alone".to_string() } else { format!(", with {}", with.join(", ")) }
        ));
    }
    out
}

/// The whole prompt for one member over one window. `rows` are theirs alone, in
/// time order; the numbering the model refers to is their position in it.
/// `said` is everybody's pronouns as their roles have them. Anyone missing from
/// it is named to the model as they/them: the model is never left to work
/// somebody's gender out for itself.
pub fn build_prompt(
    member: u64,
    name: &str,
    period: &str,
    rows: &[&SaidRow],
    sessions: &[VoiceSession],
    rooms: &HashMap<u64, String>,
    names: &HashMap<u64, String>,
    max: usize,
    said: &HashMap<u64, super::pronouns::Pronouns>,
) -> Prompt {
    let total = rows.len();
    let times: Vec<i64> = rows.iter().map(|r| r.created_ms).collect();
    let ranges = pick(&times, max);
    let sent: usize = ranges.iter().map(|r| r.len()).sum();

    let mut known: HashMap<u64, String> = names.clone();
    for r in rows {
        known.entry(r.author_id).or_insert_with(|| r.author_name.clone());
    }

    let mut text = String::with_capacity(INSTRUCTIONS.len() + sent * 120);
    text.push_str(INSTRUCTIONS);
    text.push_str("\n\n---\n\n");
    text.push_str(&format!("The member: {}. Period: {}. Times are India time (IST).\n", name, period));
    let (deleted, blocked) = (rows.iter().filter(|r| r.deleted()).count(), rows.iter().filter(|r| r.blocked()).count());
    text.push_str(&format!(
        "They sent {} message{} the bot still has{}{}.\n",
        total,
        if total == 1 { "" } else { "s" },
        if deleted > 0 { format!(", {} of them since deleted", deleted) } else { String::new() },
        if blocked > 0 { format!(", and AutoMod blocked {} more", blocked) } else { String::new() }
    ));
    if sent < total {
        text.push_str(&format!("There were too many to send: you are seeing {} of them - the opening, the busiest run and the end.\n", sent));
    }
    text.push_str(&voice_lines(sessions, rooms, &known));
    // Whose pronouns are whose, off the server's roles rather than out of the
    // model's head. Sorted, so the same window always builds the same prompt.
    let mut who: Vec<(u64, String)> = known.iter().filter(|(id, _)| **id != member).map(|(id, n)| (*id, n.clone())).collect();
    who.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()).then(a.0.cmp(&b.0)));
    // The member this is about goes first, under the name the page calls them.
    who.insert(0, (member, name.to_string()));
    text.push('\n');
    text.push_str(&super::pronouns::block_for(&who, said));
    text.push_str("\nTheir messages, in order:\n");
    let mut last: Option<usize> = None;
    for range in &ranges {
        if last.is_some_and(|l| l < range.start) {
            text.push_str("[…]\n");
        }
        for i in range.clone() {
            let r = rows[i];
            let mark = marker(r, &known).map(|m| format!(" {}", m)).unwrap_or_default();
            text.push_str(&format!("[#{}] {} #{}{}: {}\n", i + 1, when(r.created_ms), r.channel_name, mark, flat(&named(&r.content, &known))));
        }
        last = Some(range.end);
    }
    Prompt { text, sent, total, trimmed: sent < total }
}

// --- reading the answer ----------------------------------------------------------------------

const WATCH_KINDS: [&str; 4] = ["argument", "going_quiet", "rule_problem", "other"];

fn text_of(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).map(str::trim).unwrap_or("").to_string()
}

fn number_in(v: &Value, total: usize) -> Option<usize> {
    let n = v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().trim_start_matches('#').parse().ok()))? as usize;
    (1..=total).contains(&n).then_some(n)
}

fn refs_in(v: Option<&Value>, total: usize) -> Vec<usize> {
    v.and_then(Value::as_array).map(|r| r.iter().filter_map(|x| number_in(x, total)).collect()).unwrap_or_default()
}

/// The model's answer in the page's shape, or none when it holds no JSON
/// object. Message numbers outside the window are dropped, and a `kind` the
/// page doesn't draw becomes "other" rather than being trusted through.
pub fn parse_summary(raw: &str, total: usize) -> Option<Value> {
    let start = raw.find('{')?;
    let end = raw.rfind('}')?;
    let v: Value = serde_json::from_str(raw.get(start..=end)?).ok()?;
    let o = v.as_object()?;
    let list = |k: &str| o.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    let topics: Vec<Value> = list("topics")
        .iter()
        .filter_map(|t| {
            let what = text_of(t.get("what"));
            (!what.is_empty()).then(|| json!({ "what": what, "refs": refs_in(t.get("refs"), total) }))
        })
        .take(12)
        .collect();
    let people: Vec<Value> = list("people")
        .iter()
        .filter_map(|p| {
            let who = text_of(p.get("who"));
            (!who.is_empty()).then(|| json!({ "who": who, "how": text_of(p.get("how")) }))
        })
        .take(12)
        .collect();
    let watch: Vec<Value> = list("watch")
        .iter()
        .filter_map(|w| {
            let kind = text_of(w.get("kind")).to_lowercase().replace([' ', '-'], "_");
            let what = text_of(w.get("what"));
            if what.is_empty() {
                return None;
            }
            Some(json!({
                "kind": if WATCH_KINDS.contains(&kind.as_str()) { kind } else { "other".to_string() },
                "what": what,
                "refs": refs_in(w.get("refs"), total),
            }))
        })
        .collect();
    let mut out = Map::new();
    out.insert("overview".into(), json!(text_of(o.get("overview"))));
    out.insert("topics".into(), json!(topics));
    out.insert("people".into(), json!(people));
    out.insert("rhythm".into(), json!(text_of(o.get("rhythm"))));
    out.insert("places".into(), json!(text_of(o.get("places"))));
    out.insert("watch".into(), json!(watch));
    out.insert("thin".into(), json!(text_of(o.get("thin"))));
    out.insert("interpretation".into(), json!(list("interpretation").iter().map(|s| text_of(Some(s))).filter(|s| !s.is_empty()).collect::<Vec<_>>()));
    Some(Value::Object(out))
}

// --- filing one away ---------------------------------------------------------------------------

/// Who a dive is about, over what, and how it came to be run. Everything the
/// store files a summary under, so a dive a moderator pressed for and one the
/// nightly scan picked are the same kind of row and read the same way back.
#[derive(Clone, Debug)]
pub struct Filed<'a> {
    pub member: u64,
    pub days: i64,
    pub period: Period,
    /// The exact messages it covered, in order: `[#n]` is `message_ids[n - 1]`.
    pub message_ids: Vec<u64>,
    /// The moderator who asked, or 0 when nobody did.
    pub run_by: u64,
    pub run_ts: i64,
    /// Why the nightly scan picked them, in words; empty when somebody asked.
    pub reason: &'a str,
}

/// One deep dive's summary as the store takes it. The model's answer is read
/// into the page's shape here, so a reply that was not JSON is filed with its
/// raw text and no summary rather than being lost.
pub fn new_summary(filed: &Filed, key: String, prompt: &Prompt, reply: Reply) -> NewSummary {
    let parsed = parse_summary(&reply.text, filed.message_ids.len());
    NewSummary {
        stretch_key: key,
        channel_id: 0,
        // One person, in all three places the store keeps people.
        a_id: filed.member,
        b_id: filed.member,
        people: vec![filed.member],
        scope: SCOPE_MEMBER.to_string(),
        start_ms: filed.period.from * 1000,
        end_ms: filed.period.to * 1000,
        detection_id: None,
        message_ids: filed.message_ids.clone(),
        sent_count: prompt.sent,
        trimmed: prompt.trimmed,
        run_by: filed.run_by,
        run_ts: filed.run_ts,
        model: reply.model,
        input_tokens: reply.input_tokens,
        output_tokens: reply.output_tokens,
        summary: parsed,
        raw: reply.text,
        reason: filed.reason.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stay(user: u64, channel: u64, start: i64, end: i64) -> VoiceStay {
        VoiceStay { user_id: user, channel_id: channel, start, end }
    }

    /// Somebody still here gets the plain window; somebody long gone gets their
    /// last days on the server instead of an empty one.
    #[test]
    fn the_window_follows_a_member_who_has_left() {
        let now = 1_800_000_000;
        let here = period(now, 7, None, Some(now - 3 * DAY));
        assert_eq!((here.from, here.to, here.shifted), (now - 7 * DAY, now, false));

        // Gone 40 days ago, last spoke 41 days ago: the window ends where they did.
        let gone = period(now, 7, Some(now - 40 * DAY), Some(now - 41 * DAY));
        assert!(gone.shifted, "it moved back to them");
        assert_eq!(gone.to, now - 40 * DAY, "up to the moment they went");
        assert_eq!(gone.from, now - 47 * DAY);
        assert_eq!(gone.days(), 7, "still the period that was asked for");

        // Gone yesterday: the plain window already covers them, so it stands.
        let fresh = period(now, 7, Some(now - DAY), Some(now - 2 * DAY));
        assert!(!fresh.shifted);
        assert_eq!(fresh.to, now);
        // Gone, and the bot never kept a word: the leaving date carries it.
        let quiet = period(now, 3, Some(now - 90 * DAY), None);
        assert_eq!((quiet.to, quiet.shifted), (now - 90 * DAY, true));
    }

    /// Company is the overlap in the same room, and nothing else.
    #[test]
    fn voice_sessions_carry_who_was_in_there_too() {
        let (from, to) = (0, 10_000);
        let stays = [
            stay(1, 41, 0, 3_600),          // ours
            stay(2, 41, 1_800, 5_400),      // half an hour of it
            stay(3, 42, 0, 3_600),          // another room entirely
            stay(4, 41, 3_500, 3_530),      // thirty seconds: not company
            stay(1, 42, 7_200, 9_000),      // ours again, elsewhere
            stay(2, 42, 7_200, 9_000),      // the whole of it
        ];
        let got = sessions(&stays, 1, from, to);
        assert_eq!(got.len(), 2, "one session per stretch of theirs");
        assert_eq!((got[0].channel_id, got[0].secs), (41, 3_600));
        assert_eq!(got[0].with, vec![(2, 1_800)], "the other room and the thirty seconds are both out");
        assert_eq!(got[1].with, vec![(2, 1_800)]);

        // The window clips the ends rather than counting time outside it.
        let clipped = sessions(&stays, 1, 1_800, 2_400);
        assert_eq!(clipped.len(), 1);
        assert_eq!((clipped[0].start, clipped[0].end, clipped[0].secs), (1_800, 2_400, 600));
        assert!(sessions(&stays, 1, 20_000, 30_000).is_empty(), "nothing in the window");
        assert_eq!(spell_secs(3_600 * 2 + 900), "2h 15m");
        assert_eq!(spell_secs(30), "under a minute");
    }

    /// Whatever the model says back, the page gets the shape it draws — and
    /// numbers for messages that aren't there are dropped.
    #[test]
    fn the_answer_is_read_into_the_pages_shape() {
        let raw = r##"here you go: {
            "overview": "  Quiet week.  ",
            "topics": [{"what": "cricket", "refs": [1, "#2", 99]}, {"what": "", "refs": []}],
            "people": [{"who": "Dev", "how": "arguing about RCB"}],
            "rhythm": "busier at night",
            "places": "#general",
            "watch": [{"kind": "Rule-Problem", "what": "a slur", "refs": [2]}, {"kind": "made_up", "what": "x"}, {"what": ""}],
            "thin": "",
            "interpretation": ["seems tired", ""]
        } thanks"##;
        let out = parse_summary(raw, 3).unwrap();
        assert_eq!(out["overview"], "Quiet week.");
        assert_eq!(out["topics"].as_array().unwrap().len(), 1, "an empty topic is not a topic");
        assert_eq!(out["topics"][0]["refs"], json!([1, 2]), "99 isn't one of the three messages");
        assert_eq!(out["watch"][0]["kind"], "rule_problem");
        assert_eq!(out["watch"][1]["kind"], "other", "a kind the page can't draw is not trusted through");
        assert_eq!(out["watch"].as_array().unwrap().len(), 2);
        assert_eq!(out["interpretation"], json!(["seems tired"]));
        assert!(parse_summary("no json at all", 3).is_none());
    }

    /// The instructions have to keep saying the things the page promises a mod.
    #[test]
    fn the_instructions_hold_their_promises() {
        let i = INSTRUCTIONS.to_lowercase();
        for must in ["neutral", "do not diagnose", "interpretation", "thin", "never invent", "only use numbers"] {
            assert!(i.contains(must), "the instructions dropped “{must}”");
        }
        assert!(i.contains("caste") && i.contains("sexuality"), "it still forbids guessing sensitive things");
        // A dive writes about somebody in the third person all the way through,
        // so it has to be handed the pronouns rather than reaching for them.
        for must in ["never guess", "they/them", "use exactly those"] {
            assert!(i.contains(must), "the instructions stopped saying “{must}”");
        }
    }

    /// The member's pronouns, and everyone else's, reach the prompt — from the
    /// server's roles, with they/them for anybody the roles cannot answer for.
    #[test]
    fn the_prompt_carries_the_pronouns_it_was_given() {
        use super::super::kalesh::tests::row;
        const MEMBER: u64 = 711;
        let rows = [row(1_780_000_000_000, 1, MEMBER, "gooner", 5, "kal ka match dekha", None)];
        let refs: Vec<&SaidRow> = rows.iter().collect();
        let names: HashMap<u64, String> = [(99u64, "riya".to_string())].into_iter().collect();
        let said: HashMap<u64, super::super::pronouns::Pronouns> =
            [(MEMBER, super::super::pronouns::Pronouns::He), (99, super::super::pronouns::Pronouns::She)].into_iter().collect();

        let p = build_prompt(MEMBER, "gooner", "the last 7 days", &refs, &[], &HashMap::new(), &names, 400, &said);
        assert!(p.text.contains("- gooner: he/him"), "{}", p.text);
        assert!(p.text.contains("- riya: she/her"), "{}", p.text);

        // Nothing known about anybody: everyone is named as they/them all the
        // same, so the model is never left to work it out.
        let p = build_prompt(MEMBER, "gooner", "the last 7 days", &refs, &[], &HashMap::new(), &names, 400, &HashMap::new());
        assert!(p.text.contains("- gooner: they/them") && p.text.contains("- riya: they/them"), "{}", p.text);
        assert!(p.text.to_lowercase().contains("never guess"));
    }
}
