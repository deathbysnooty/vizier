//! Automatic moderation: spam is deleted, suspected AI writing is only flagged.
//!
//! # The one rule that cannot be configured away
//!
//! **An AI score never deletes a message and never punishes anybody.** There is
//! no setting, no threshold and no combination of switches that makes it do so.
//! The two halves of this module do not share a code path: the spam half builds
//! a [`Hit`], which carries the message ids to remove, and the AI half builds a
//! [`Suspicion`], which carries a score and some words and *has no field a
//! message id could go in*. The only function that deletes anything takes a
//! `Hit`. A moderator reading a flag can press Delete it — a person deciding, on
//! the record, which is the whole point.
//!
//! The reason is not squeamishness. **There is no reliable way to tell whether a
//! human or a machine wrote a piece of text.** Every published detector has a
//! high false-positive rate, and its mistakes land hardest on people writing
//! English as a second language — which is most of this server. Careful, formal,
//! correctly punctuated English written by someone who learned it at school is
//! exactly what a detector calls "AI". Acting on that automatically would
//! punish the most careful writers here for being careful. So the AI half
//! flags, a human decides, and every time a human says the flag was wrong that
//! is counted and shown on the panel, because a "was wrong" count is the only
//! honest measure of whether this half is worth keeping at all.
//!
//! # Spam, which is objective, is deleted
//!
//! Spam is not a judgement about a person's writing, it is a shape: the same
//! text over and over, twenty messages in four seconds, a mass ping, an invite
//! to another server, a wall of one repeated character. Those are counted, not
//! guessed at, so they are removed and logged. Every rule's numbers are
//! settings, and setting a count to 0 switches that rule off.
//!
//! # Who is never touched
//!
//! Moderators and bot admins, other bots and webhooks, DMs, #safe-corner and
//! its threads, and the game channels — the anagram, guess-the-word,
//! word-chain, sudoku, chess and Name Place Animal Thing channels, where people
//! legitimately fire the same short word off again and again and a flood rule
//! would eat the game. Those channels are left alone whatever the exempt
//! setting says; the setting only ever adds more. A message the bot cannot
//! place — a thread whose parent the cache doesn't know, a member whose roles it
//! cannot see — is left alone too: not being sure is a reason to do nothing.
//!
//! # Cost
//!
//! Every check on the gateway thread is arithmetic over the last few messages
//! of one member, held in memory. The model is only asked about a message that
//! is already long enough to judge *and* has crossed the free threshold *and* is
//! inside the hourly cap, and never twice about the same message.

use std::collections::{HashMap, VecDeque};
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::Mutex;
use serenity::all::{
    ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateButton, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditMessage, GuildId, Message, MessageId,
};

use super::automod_store::{self as store, Kind, ModelSay, NewFlag, Outcome, StyleRow, StyleUpdate};
use super::control;

/// The moderation log the owner made for this: #📓moderation-logs.
pub const DEFAULT_LOG_CHANNEL: u64 = 1_516_779_799_865_987_101;

/// Game channels that are left alone whatever the exempt setting says. The
/// sudoku, chess and Name Place Animal Thing channels are added from their own
/// settings, since those move.
pub const GAME_CHANNELS: &[u64] = &[
    // 🔤anagram
    1_542_764_196_901_683_231,
    // ❓guess-the-word
    1_518_233_664_016_617_582,
    // 🔗word-chain
    1_516_846_913_452_769_280,
];

/// How much of a flagged message is shown in the log post.
const EXCERPT_CHARS: usize = 320;
/// Recent messages kept per member for the spam rules.
const WINDOW: usize = 40;
/// The shortest a repeated message may be to count as a repeat. Below this,
/// "haan", "same" and "😭" repeat perfectly innocently.
pub const REPEAT_MIN_CHARS: usize = 12;
/// How long the bot waits for the model before giving up on a second opinion.
const MODEL_WAIT: Duration = Duration::from_secs(20);
const PURGE_EVERY: Duration = Duration::from_secs(3600);
const HOUR: i64 = 3600;

// --- settings ------------------------------------------------------------------------

/// The whole feature. Off by default: the owner switches it on deliberately.
pub fn enabled() -> bool {
    control::on("VIZIER_AUTOMOD", false)
}

/// The half that deletes. Off by default.
pub fn spam_on() -> bool {
    enabled() && control::on("VIZIER_AUTOMOD_SPAM", false)
}

/// The half that only ever flags. Off by default.
pub fn ai_on() -> bool {
    enabled() && control::on("VIZIER_AUTOMOD_AI", false)
}

pub fn log_channel() -> Option<u64> {
    match control::var("VIZIER_AUTOMOD_LOG_CHANNEL") {
        Some(raw) => raw.parse().ok(),
        None => Some(DEFAULT_LOG_CHANNEL),
    }
}

/// Channels left alone: the game channels, always, plus anything the setting adds.
pub fn exempt_channels() -> Vec<u64> {
    let mut out = GAME_CHANNELS.to_vec();
    for key in ["VIZIER_SUDOKU_CHANNEL", "VIZIER_CHESS_CHANNEL", "VIZIER_NPAT_CHANNEL"] {
        if let Some(id) = control::id(key) {
            out.push(id);
        }
    }
    out.extend(control::ids("VIZIER_AUTOMOD_EXEMPT_CHANNELS"));
    out.sort_unstable();
    out.dedup();
    out
}

pub fn keep_days() -> i64 {
    control::number("VIZIER_AUTOMOD_KEEP_DAYS", 30).clamp(1, 365) as i64
}

/// Every number the spam rules use, read at the moment they are applied.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpamSettings {
    /// How many near-identical messages in the window before it is a repeat. 0 is off.
    pub repeat_count: usize,
    pub repeat_window: i64,
    /// How many messages in `burst_secs` before it is a flood. 0 is off.
    pub burst_count: usize,
    pub burst_secs: i64,
    /// Mentions in one message before it is a mass ping. 0 is off.
    pub max_mentions: usize,
    pub everyone: bool,
    pub invites: bool,
    /// The length at which a wall of one character is a wall. 0 is off.
    pub wall_chars: usize,
    /// The share of a wall that must be the same character or word, as a percentage.
    pub wall_pct: u64,
    /// The least time between two log posts about the same member. Messages
    /// inside it are still removed; they simply don't each get their own post.
    pub quiet_secs: i64,
}

impl Default for SpamSettings {
    fn default() -> Self {
        Self {
            repeat_count: 3,
            repeat_window: 60,
            burst_count: 8,
            burst_secs: 5,
            max_mentions: 6,
            everyone: true,
            invites: true,
            wall_chars: 400,
            wall_pct: 70,
            quiet_secs: 60,
        }
    }
}

pub fn spam_settings() -> SpamSettings {
    SpamSettings {
        repeat_count: control::number("VIZIER_AUTOMOD_REPEAT_COUNT", 3).min(50) as usize,
        repeat_window: control::number("VIZIER_AUTOMOD_REPEAT_WINDOW_SECS", 60).clamp(5, 3600) as i64,
        burst_count: control::number("VIZIER_AUTOMOD_BURST_COUNT", 8).min(100) as usize,
        burst_secs: control::number("VIZIER_AUTOMOD_BURST_SECS", 5).clamp(1, 600) as i64,
        max_mentions: control::number("VIZIER_AUTOMOD_MAX_MENTIONS", 6).min(50) as usize,
        everyone: control::on("VIZIER_AUTOMOD_EVERYONE", true),
        invites: control::on("VIZIER_AUTOMOD_INVITES", true),
        wall_chars: control::number("VIZIER_AUTOMOD_WALL_CHARS", 400).min(4000) as usize,
        wall_pct: control::number("VIZIER_AUTOMOD_WALL_PCT", 70).clamp(10, 100),
        quiet_secs: control::number("VIZIER_AUTOMOD_QUIET_SECS", 60).clamp(0, 3600) as i64,
    }
}

/// Every number the AI half uses.
#[derive(Clone, Debug, PartialEq)]
pub struct AiSettings {
    /// Below this many characters a message is not judged at all. Short text
    /// carries no evidence either way and pretending otherwise is how detectors
    /// end up accusing people who write briefly.
    pub min_chars: usize,
    /// The free score at which a message is worth a human's attention, 0..1.
    pub threshold: f64,
    /// The model asked for a second opinion, on the bot's own provider.
    pub model: Option<String>,
    /// The most model calls in an hour.
    pub max_hour: u32,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self { min_chars: 280, threshold: 0.62, model: Some(DEFAULT_MODEL.to_string()), max_hour: 30 }
    }
}

/// The same quick, cheap model the Name Place Animal Thing judging uses.
pub const DEFAULT_MODEL: &str = "google/gemini-2.5-flash-lite";

pub fn ai_settings() -> AiSettings {
    AiSettings {
        min_chars: control::number("VIZIER_AUTOMOD_AI_MIN_CHARS", 280).clamp(120, 4000) as usize,
        threshold: control::float("VIZIER_AUTOMOD_AI_THRESHOLD", 0.62).clamp(0.2, 1.0),
        model: match control::var("VIZIER_AUTOMOD_MODEL") {
            None => Some(DEFAULT_MODEL.to_string()),
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "agent" | "default" | "none") => None,
            Some(v) => Some(v),
        },
        max_hour: control::number("VIZIER_AUTOMOD_AI_MAX_HOUR", 30).min(10_000) as u32,
    }
}

// --- what one message looks like to the rules -------------------------------------

/// A message as the rules see it: no Discord types, so every rule is a plain
/// function that can be tested.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub member_id: u64,
    pub channel_id: u64,
    pub message_id: u64,
    /// Seconds.
    pub ts: i64,
    pub text: String,
    /// Distinct members and roles mentioned.
    pub mentions: usize,
}

impl Seen {
    pub fn new(member_id: u64, channel_id: u64, message_id: u64, ts: i64, text: &str) -> Self {
        Self { member_id, channel_id, message_id, ts, text: text.to_string(), mentions: 0 }
    }
}

/// Where a message was posted, and whether the bot is allowed to act there.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub channel_id: u64,
    pub parent_id: Option<u64>,
    pub channel_name: String,
    /// The name of the channel a thread sits in, so #safe-corner protects its
    /// threads by name as well as by id.
    pub parent_name: String,
}

/// Whether the rules may act on a message at all. Anything uncertain is a no:
/// the cost of doing nothing is a spam message left up for a minute, and the
/// cost of being wrong is deleting someone's words.
pub fn acts_on(place: &Place, in_guild: bool, bot: bool, is_mod: bool, exempt: &[u64], sensitive: &[u64]) -> bool {
    if !in_guild || bot || is_mod {
        return false;
    }
    let blocked = |id: u64, name: &str| exempt.contains(&id) || sensitive.contains(&id) || super::weekly::is_safe_corner(id, name);
    if blocked(place.channel_id, &place.channel_name) {
        return false;
    }
    // A thread is judged by the channel it is in as well as by itself.
    !place.parent_id.is_some_and(|p| blocked(p, &place.parent_name))
}

// --- the spam rules ------------------------------------------------------------------

/// Which spam rule fired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rule {
    /// The same or near-same text several times in a short while, channels included.
    Repeat,
    /// Too many messages in too few seconds.
    Burst,
    /// One message pinging a crowd.
    Mentions,
    /// An @everyone or @here from someone who isn't a moderator.
    Everyone,
    /// An invite to another Discord server.
    Invite,
    /// A very long message that is mostly one character or word over and over.
    Wall,
}

impl Rule {
    pub fn key(self) -> &'static str {
        match self {
            Rule::Repeat => "repeat",
            Rule::Burst => "burst",
            Rule::Mentions => "mentions",
            Rule::Everyone => "everyone",
            Rule::Invite => "invite",
            Rule::Wall => "wall",
        }
    }

    /// How the rule reads in the log and on the panel.
    pub fn label(self) -> &'static str {
        match self {
            Rule::Repeat => "The same message over and over",
            Rule::Burst => "Too many messages too fast",
            Rule::Mentions => "Pinging a crowd",
            Rule::Everyone => "An @everyone or @here",
            Rule::Invite => "An invite to another server",
            Rule::Wall => "A wall of one character",
        }
    }
}

/// Spam that was found: the rule, the messages to remove, and how to say it.
///
/// This is the only type in this module that carries message ids, and only the
/// spam rules build one. The AI half cannot make a `Hit`.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub rule: Rule,
    /// The messages to delete as `(channel, message)`, oldest first. The channel
    /// travels with the message because a repeat is often spread across several
    /// of them, and a message can only be deleted from the channel it is in.
    pub messages: Vec<(u64, u64)>,
    /// The offending text, already shortened.
    pub text: String,
    /// The counts behind the rule, in plain words.
    pub detail: String,
    /// The messages are removed but no new log post is made: there was already
    /// one about this member moments ago, so a flood is one entry, not twenty.
    pub quiet: bool,
}

impl Hit {
    /// The message ids alone, for counting and for the log.
    pub fn ids(&self) -> Vec<u64> {
        self.messages.iter().map(|(_, m)| *m).collect()
    }

    /// The distinct channels the messages were in, in the order first seen.
    pub fn channels(&self) -> Vec<u64> {
        let mut out: Vec<u64> = Vec::new();
        for (c, _) in &self.messages {
            if !out.contains(c) {
                out.push(*c);
            }
        }
        out
    }
}

/// One message remembered for a member.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    channel: u64,
    message: u64,
    ts: i64,
    print: String,
}

#[derive(Clone, Debug, Default)]
struct MemberWindow {
    entries: VecDeque<Entry>,
    /// No new log post about this member until then.
    quiet_until: i64,
    /// The flag the last post was about, so continuing spam is counted onto it.
    open_flag: Option<i64>,
}

/// The last few messages of every member the rules have seen.
#[derive(Clone, Debug, Default)]
pub struct Recent {
    members: HashMap<u64, MemberWindow>,
}

/// Text stripped to what it is actually saying: lower case, letters and digits
/// only. "FREE NITRO!!! 🎁" and "free nitro 🎁" are one message.
pub fn fingerprint(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Whether two fingerprints are the same message for the repeat rule: equal, or
/// one wholly inside the other (someone adding "please" each time).
pub fn near_same(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    short.chars().count() >= REPEAT_MIN_CHARS && long.contains(short)
}

/// Discord invite links, however they are written.
pub fn has_invite(text: &str) -> bool {
    let low = text.to_ascii_lowercase();
    // The code after the slash has to be there: "discord.gg" on its own, in a
    // sentence about Discord, is not an invite.
    let after = |host: &str| {
        low.match_indices(host).any(|(i, _)| {
            low[i + host.len()..].chars().next().is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
    };
    after("discord.gg/") || after("discord.com/invite/") || after("discordapp.com/invite/") || after("discord.me/")
}

/// An attempt at @everyone or @here. Whether Discord actually let them ping is
/// beside the point: a member without the permission typing it is the same act.
pub fn has_everyone(text: &str) -> bool {
    let low = text.to_ascii_lowercase();
    low.contains("@everyone") || low.contains("@here")
}

/// The longest short unit the wall rule will call a repeat: "hahaha",
/// "lolololol", "🎉🎊🎉🎊" and so on.
const WALL_MAX_PERIOD: usize = 8;

/// How much of `solid` is the same short unit repeated over and over, as a
/// share from 0 to 1. A period of one is a single character repeated.
fn repeat_share(solid: &[char], max_period: usize) -> f64 {
    let mut best = 0.0;
    for p in 1..=max_period.min(solid.len() / 2) {
        let same = solid.iter().enumerate().filter(|(i, c)| solid[i % p] == **c).count();
        let share = same as f64 / solid.len() as f64;
        if share > best {
            best = share;
        }
    }
    best
}

/// Whether a message is a wall: long, and mostly one character, one short unit
/// or one word repeated. Whitespace is ignored when measuring.
pub fn is_wall(text: &str, min_chars: usize, pct: u64) -> bool {
    if min_chars == 0 {
        return false;
    }
    let solid: Vec<char> = text.chars().filter(|c| !c.is_whitespace()).collect();
    if solid.len() < min_chars {
        return false;
    }
    let wanted = pct as f64 / 100.0;
    if repeat_share(&solid, WALL_MAX_PERIOD) >= wanted {
        return true;
    }
    // The same short word again and again: "lol lol lol lol …".
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() < 8 {
        return false;
    }
    let mut word_counts: HashMap<&str, usize> = HashMap::new();
    for w in &words {
        *word_counts.entry(*w).or_default() += 1;
    }
    word_counts.values().copied().max().is_some_and(|n| n as f64 / words.len() as f64 >= wanted)
}

impl Recent {
    /// Looks at one message and says what, if anything, is spam. The message is
    /// remembered either way.
    ///
    /// Single-message rules come first: they are certain, and they only ever
    /// take that one message. The counting rules come after.
    pub fn check(&mut self, seen: &Seen, s: &SpamSettings) -> Option<Hit> {
        let window = self.members.entry(seen.member_id).or_default();
        let oldest = seen.ts - s.repeat_window.max(s.burst_secs).max(1);
        while window.entries.front().is_some_and(|e| e.ts < oldest) {
            window.entries.pop_front();
        }
        while window.entries.len() >= WINDOW {
            window.entries.pop_front();
        }
        let print = fingerprint(&seen.text);
        window.entries.push_back(Entry { channel: seen.channel_id, message: seen.message_id, ts: seen.ts, print: print.clone() });
        let quiet = seen.ts < window.quiet_until;

        let one = |rule: Rule, detail: String| {
            Some(Hit { rule, messages: vec![(seen.channel_id, seen.message_id)], text: excerpt(&seen.text), detail, quiet })
        };
        if s.everyone && has_everyone(&seen.text) {
            return one(Rule::Everyone, "a member who isn't a moderator tried to ping everyone".into());
        }
        if s.invites && has_invite(&seen.text) {
            return one(Rule::Invite, "an invite link to another Discord server".into());
        }
        if s.max_mentions > 0 && seen.mentions >= s.max_mentions {
            return one(Rule::Mentions, format!("{} people and roles pinged in one message", seen.mentions));
        }
        if is_wall(&seen.text, s.wall_chars, s.wall_pct) {
            let len = seen.text.chars().count();
            return one(Rule::Wall, format!("{} characters, nearly all of them the same one", len));
        }

        // The same message again and again, this channel and others.
        if s.repeat_count >= 2 && print.chars().count() >= REPEAT_MIN_CHARS {
            let same: Vec<&Entry> =
                window.entries.iter().filter(|e| e.ts >= seen.ts - s.repeat_window && near_same(&e.print, &print)).collect();
            if same.len() >= s.repeat_count {
                let messages: Vec<(u64, u64)> = same.iter().map(|e| (e.channel, e.message)).collect();
                let hit = Hit { rule: Rule::Repeat, messages, text: excerpt(&seen.text), detail: String::new(), quiet };
                let spread = hit.channels().len();
                let span = seen.ts - same.first().map(|e| e.ts).unwrap_or(seen.ts);
                let where_ = if spread > 1 { format!(", across {} channels", spread) } else { String::new() };
                let detail = format!("{} near-identical messages in {}{}", same.len(), seconds_words(span), where_);
                window.entries.retain(|e| !near_same(&e.print, &print));
                return Some(Hit { detail, ..hit });
            }
        }

        // A flood, whatever the messages say.
        if s.burst_count >= 2 {
            let burst: Vec<&Entry> = window.entries.iter().filter(|e| e.ts >= seen.ts - s.burst_secs).collect();
            if burst.len() >= s.burst_count {
                let span = seen.ts - burst.first().map(|e| e.ts).unwrap_or(seen.ts);
                let detail = format!("{} messages in {}", burst.len(), seconds_words(span.max(1)));
                let messages: Vec<(u64, u64)> = burst.iter().map(|e| (e.channel, e.message)).collect();
                let hit = Hit { rule: Rule::Burst, messages, text: excerpt(&seen.text), detail, quiet };
                window.entries.clear();
                return Some(hit);
            }
        }
        None
    }

    /// Notes that a log post was made about this member, so the next few
    /// minutes of the same flood are removed quietly onto the same entry.
    pub fn note_post(&mut self, member: u64, flag: i64, until: i64) {
        let window = self.members.entry(member).or_default();
        window.open_flag = Some(flag);
        window.quiet_until = until;
    }

    /// The entry a quiet removal should be counted onto, if there is one.
    pub fn open_flag(&self, member: u64) -> Option<i64> {
        self.members.get(&member).and_then(|w| w.open_flag)
    }

    /// Forgets members nobody has heard from in a while, so the map cannot grow
    /// without end on a big server.
    pub fn sweep(&mut self, now: i64, older_than: i64) {
        self.members.retain(|_, w| {
            w.entries.back().is_some_and(|e| e.ts >= now - older_than) || w.quiet_until > now
        });
    }
}

fn seconds_words(secs: i64) -> String {
    match secs {
        s if s <= 1 => "a second".to_string(),
        s if s < 90 => format!("{} seconds", s),
        s => format!("{} minutes", (s + 29) / 60),
    }
}

/// A short, single-line, markdown-free piece of a message, for a log post.
pub fn excerpt(text: &str) -> String {
    let flat: String = text.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).filter(|c| *c != '`' && *c != '\u{200b}').collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > EXCERPT_CHARS {
        format!("{}…", flat.chars().take(EXCERPT_CHARS).collect::<String>())
    } else {
        flat
    }
}

// --- reading a message's shape -------------------------------------------------------

/// Roughly, is this character an emoji? Enough to count them, which is all the
/// style baseline needs.
pub fn is_emoji(c: char) -> bool {
    matches!(c as u32,
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF | 0xFE0F | 0x1F1E6..=0x1F1FF | 0x2190..=0x21FF | 0x2300..=0x23FF)
}

/// Common Roman-Hindi words. Not a dictionary — a sample big enough to tell
/// "bhai kal milte hain" from "Furthermore, it is worth noting".
const HINGLISH: &[&str] = &[
    "hai", "hain", "nahi", "nhi", "kya", "kyu", "kyun", "bhai", "yaar", "yar", "acha", "accha", "achha", "matlab", "bhi",
    "toh", "to", "kar", "karo", "karna", "raha", "rahe", "rahi", "mera", "meri", "tera", "teri", "koi", "sab", "abhi",
    "kal", "aaj", "kuch", "thoda", "bahut", "bohot", "chal", "chalo", "arre", "are", "haan", "han", "mast", "jaldi",
    "paisa", "log", "wala", "wali", "mein", "ko", "ka", "ki", "ke", "aur", "ye", "yeh", "wo", "woh", "hoga", "hona",
    "lagta", "pata", "samajh", "dekh", "dekho", "sun", "suno", "bol", "bolo", "khana", "ghar", "dost", "pyaar", "dil",
    "bas", "sirf", "phir", "jab", "tab", "agar", "magar", "lekin", "kaise", "kahan", "kaun", "kitna", "zyada", "kam",
    "theek", "thik", "sahi", "galat", "bura", "khush", "banda", "bandi", "scene", "jhol", "kalesh", "bakchodi", "masti",
    "sach", "jhooth", "apna", "apni", "hum", "tum", "aap", "main", "mai", "nahin", "kabhi", "hamesha", "dhang", "seedha",
];

/// Whether a word looks like Roman Hindi (or is Devanagari outright).
pub fn is_hinglish(word: &str) -> bool {
    if word.chars().any(|c| ('\u{0900}'..='\u{097F}').contains(&c)) {
        return true;
    }
    let w: String = word.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
    !w.is_empty() && HINGLISH.contains(&w.as_str())
}

/// The plain measurements of one message: how long, how punctuated, how many
/// emoji, how much of it is Hinglish, how the sentences start.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shape {
    pub chars: u32,
    pub punct: u32,
    pub emoji: u32,
    pub tokens: u32,
    pub hinglish_tokens: u32,
    pub sentences: u32,
    pub caps_starts: u32,
}

/// The sentences of a message, roughly: split on . ! ? and newlines.
pub fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if matches!(c, '.' | '!' | '?' | '\n') {
            if !current.trim().is_empty() {
                out.push(current.trim().to_string());
            }
            current.clear();
        } else {
            current.push(c);
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

pub fn shape_of(text: &str) -> Shape {
    let words: Vec<&str> = text.split_whitespace().collect();
    let sents = sentences(text);
    Shape {
        chars: text.chars().count() as u32,
        punct: text.chars().filter(|c| c.is_ascii_punctuation()).count() as u32,
        emoji: text.chars().filter(|c| is_emoji(*c)).count() as u32,
        tokens: words.len() as u32,
        hinglish_tokens: words.iter().filter(|w| is_hinglish(w)).count() as u32,
        sentences: sents.len() as u32,
        caps_starts: sents.iter().filter(|s| s.chars().next().is_some_and(|c| c.is_uppercase())).count() as u32,
    }
}

impl Shape {
    fn per_hundred(&self, n: u32) -> f64 {
        if self.chars == 0 { 0.0 } else { n as f64 * 100.0 / self.chars as f64 }
    }

    pub fn punct_rate(&self) -> f64 {
        self.per_hundred(self.punct)
    }

    pub fn emoji_rate(&self) -> f64 {
        self.per_hundred(self.emoji)
    }

    pub fn hinglish_share(&self) -> f64 {
        if self.tokens == 0 { 0.0 } else { self.hinglish_tokens as f64 / self.tokens as f64 }
    }

    pub fn caps_share(&self) -> f64 {
        if self.sentences == 0 { 0.0 } else { self.caps_starts as f64 / self.sentences as f64 }
    }
}

// --- how this member normally writes -------------------------------------------------

/// Messages the bot must have seen from someone before it will compare them
/// against themselves. Below this, they get the generic signals only, and the
/// flag says so.
pub const MIN_HISTORY_MESSAGES: u32 = 20;
/// And they must add up to at least this much writing.
pub const MIN_HISTORY_CHARS: u64 = 600;

/// How a member usually writes, worked out from their own stored messages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Baseline {
    pub messages: u32,
    pub mean_chars: f64,
    pub punct_rate: f64,
    pub emoji_rate: f64,
    pub hinglish_share: f64,
    pub caps_share: f64,
}

/// A member's baseline, or `None` when the bot has not seen enough of their
/// writing to have an opinion. No baseline means no style score at all — never
/// a guessed one.
pub fn baseline(row: &StyleRow) -> Option<Baseline> {
    if row.messages < MIN_HISTORY_MESSAGES || row.chars < MIN_HISTORY_CHARS {
        return None;
    }
    let per = |n: u64| if row.chars == 0 { 0.0 } else { n as f64 * 100.0 / row.chars as f64 };
    Some(Baseline {
        messages: row.messages,
        mean_chars: row.chars as f64 / row.messages as f64,
        punct_rate: per(row.punct),
        emoji_rate: per(row.emoji),
        hinglish_share: if row.tokens == 0 { 0.0 } else { row.hinglish_tokens as f64 / row.tokens as f64 },
        caps_share: if row.sentences == 0 { 0.0 } else { row.caps_starts as f64 / row.sentences as f64 },
    })
}

/// How far this message is from how that member normally writes, 0..1, with the
/// reasons in plain words.
///
/// This is the fairest signal here, because it compares somebody with
/// themselves rather than with an idea of what "native" English looks like. It
/// is also the one most likely to be innocently large — somebody who usually
/// sends one-line Hinglish can sit down and write a careful paragraph — which
/// is exactly why it flags and never acts.
pub fn style_gap(base: &Baseline, msg: &Shape) -> (f64, Vec<String>) {
    let mut axes: Vec<f64> = Vec::new();
    let mut why: Vec<String> = Vec::new();

    let ratio = if base.mean_chars > 0.0 { msg.chars as f64 / base.mean_chars } else { 0.0 };
    if ratio > 3.0 {
        let gap = ((ratio - 3.0) / 9.0).min(1.0);
        axes.push(gap);
        why.push(format!(
            "much longer than they usually write (about {} characters a message, this one {})",
            base.mean_chars.round() as i64, msg.chars
        ));
    } else if msg.chars > 0 {
        axes.push(0.0);
    }

    if base.hinglish_share >= 0.12 {
        let drop = (base.hinglish_share - msg.hinglish_share()).max(0.0) / base.hinglish_share;
        axes.push(drop.min(1.0));
        if drop > 0.75 {
            why.push(format!(
                "they usually mix in Hindi ({}% of their words), and this message has almost none",
                (base.hinglish_share * 100.0).round() as i64
            ));
        }
    }

    if base.emoji_rate >= 0.8 {
        let drop = if msg.emoji == 0 { 1.0 } else { ((base.emoji_rate - msg.emoji_rate()).max(0.0) / base.emoji_rate).min(1.0) };
        axes.push(drop);
        if drop > 0.9 && msg.chars > 200 {
            why.push("they normally use emoji and this message has none".to_string());
        }
    }

    if base.caps_share < 0.45 && msg.sentences >= 3 {
        let jump = ((msg.caps_share() - base.caps_share).max(0.0) / (1.0 - base.caps_share).max(0.05)).min(1.0);
        axes.push(jump);
        if jump > 0.8 {
            why.push("they don't normally start sentences with a capital, and every sentence here does".to_string());
        }
    }

    if base.punct_rate > 0.2 {
        let diff = ((msg.punct_rate() - base.punct_rate).abs() / base.punct_rate).min(1.0);
        axes.push(diff * 0.5);
    }

    if axes.is_empty() {
        return (0.0, why);
    }
    let gap = axes.iter().sum::<f64>() / axes.len() as f64;
    if why.is_empty() && gap > 0.5 {
        why.push("it doesn't read like their usual messages here".to_string());
    }
    (gap.clamp(0.0, 1.0), why)
}

// --- the free signals ------------------------------------------------------------------

/// Turns of phrase that show up far more often in generated text than in chat.
/// A single one of these proves nothing: plenty of people write "moreover".
const REGISTER: &[&str] = &[
    "delve", "moreover", "furthermore", "tapestry", "it's worth noting", "it is worth noting", "in conclusion",
    "multifaceted", "underscores", "a testament to", "navigate the complexities", "in the realm of", "pivotal",
    "crucial to note", "it is important to note", "firstly,", "secondly,", "in today's fast-paced", "landscape of",
    "ever-evolving", "seamless", "robust framework", "leverage the", "holistic approach",
];

/// The weight each family of signals can add. They add up to more than 1 on
/// purpose; the total is clamped. Nothing here except `STYLE` measures a person
/// against anything but the text in front of it.
/// Chat laid out like a document is the one signal here that is worth much, and
/// the one that is not an accent: nobody writes `### Heading` and a bulleted
/// list on a phone, in any language. It is weighted accordingly, and more when
/// several kinds of structure turn up together.
const W_MARKDOWN: f64 = 0.26;
const W_MARKDOWN_EXTRA: f64 = 0.06;
const W_EMDASH: f64 = 0.12;
const W_EMDASH_MANY: f64 = 0.04;
const W_CURLY: f64 = 0.03;
/// Essay register is deliberately cheap. "Moreover", "furthermore" and "in
/// conclusion" are what Indian schools teach, and a member writing carefully
/// uses all three in one message quite happily.
const W_REGISTER_EACH: f64 = 0.08;
const W_REGISTER_MAX: f64 = 0.16;
const W_UNIFORM: f64 = 0.10;
const W_CLEAN: f64 = 0.06;
/// The most the style comparison can add. Kept deliberately below the point
/// where it could carry a flag on its own: every long, careful message looks
/// unlike a member whose usual output is a line of Hinglish, and most of those
/// are just someone being serious for once.
const W_STYLE: f64 = 0.24;
/// How many different families of signal must agree before a message is worth a
/// human's time. One signal on its own is noise.
pub const MIN_FAMILIES: usize = 2;

/// What the free signals found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Suspicion {
    /// 0..1. Not a probability: a rough sum of weak signals.
    pub score: f64,
    /// Why, in words a moderator can weigh for themselves.
    pub reasons: Vec<String>,
    /// How many different kinds of signal fired.
    pub families: usize,
    /// Whether the member had enough history to be compared with themselves.
    pub had_baseline: bool,
}

impl Suspicion {
    /// Whether this is worth putting in front of a human: over the threshold,
    /// and more than one kind of signal.
    ///
    /// There is deliberately no method here that deletes, removes, mutes or
    /// warns, and no message id to do it to.
    pub fn worth_a_look(&self, threshold: f64) -> bool {
        self.score >= threshold && self.families >= MIN_FAMILIES
    }
}

/// Chat with a document's clothes on: headings, bullet lists, runs of bold.
pub fn markdown_structure(text: &str) -> Option<(f64, String)> {
    let lines: Vec<&str> = text.lines().map(str::trim_start).collect();
    let headings = lines.iter().filter(|l| l.starts_with("# ") || l.starts_with("## ") || l.starts_with("### ")).count();
    let bullets = lines
        .iter()
        .filter(|l| l.starts_with("- ") || l.starts_with("* ") || l.starts_with("• ") || l.starts_with("1. ") || l.starts_with("2. "))
        .count();
    let bold = text.matches("**").count() / 2;
    let mut bits = Vec::new();
    if headings > 0 {
        bits.push("headings");
    }
    if bullets >= 3 {
        bits.push("a bulleted list");
    }
    if bold >= 3 {
        bits.push("several runs of bold");
    }
    (!bits.is_empty()).then(|| {
        let weight = W_MARKDOWN + (bits.len() - 1) as f64 * W_MARKDOWN_EXTRA;
        (weight, format!("{} — the shape of a document, not of chat", bits.join(" and ")))
    })
}

/// Sentence lengths that are all much of a muchness. People vary; generated
/// prose tends not to.
pub fn uniform_sentences(text: &str) -> Option<String> {
    let lengths: Vec<f64> = sentences(text).iter().map(|s| s.split_whitespace().count() as f64).filter(|n| *n >= 3.0).collect();
    if lengths.len() < 4 {
        return None;
    }
    let mean = lengths.iter().sum::<f64>() / lengths.len() as f64;
    if mean < 6.0 {
        return None;
    }
    let var = lengths.iter().map(|n| (n - mean).powi(2)).sum::<f64>() / lengths.len() as f64;
    let cv = var.sqrt() / mean;
    (cv < 0.25).then(|| format!("{} sentences, all within a few words of each other", lengths.len()))
}

/// A long message with none of the small mess of typing: no stray double
/// spaces, no "..." or "!!", every sentence capitalised, no bare lower-case "i".
/// Weak on its own, and deliberately weighted as such.
pub fn too_clean(text: &str) -> Option<String> {
    let shape = shape_of(text);
    if shape.sentences < 4 {
        return None;
    }
    let messy = text.contains("  ")
        || text.contains("...")
        || text.contains("!!")
        || text.contains("??")
        || text.split_whitespace().any(|w| w == "i")
        || text.split_whitespace().any(|w| w.len() > 2 && w.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()));
    if messy || shape.caps_share() < 0.95 {
        return None;
    }
    Some("not one typo, stray capital or trailing dot in a long message".to_string())
}

/// Scores a message on the free signals alone. `base` is the member's own
/// baseline, when there is one.
///
/// Nothing this returns can delete anything: the type has no message id in it.
pub fn free_signals(text: &str, base: Option<&Baseline>) -> Suspicion {
    let mut score = 0.0;
    let mut reasons = Vec::new();
    let mut families = 0;

    if let Some((weight, why)) = markdown_structure(text) {
        score += weight;
        reasons.push(why);
        families += 1;
    }

    let em_dashes = text.matches('—').count();
    let curly = text.contains('\u{201c}') || text.contains('\u{201d}') || text.contains('\u{2019}');
    if em_dashes > 0 {
        score += W_EMDASH + if em_dashes >= 2 { W_EMDASH_MANY } else { 0.0 };
        reasons.push(if em_dashes >= 2 {
            format!("{} em dashes, which almost nobody types on a phone", em_dashes)
        } else {
            "an em dash, which almost nobody types on a phone".to_string()
        });
        families += 1;
    } else if curly {
        score += W_CURLY;
        reasons.push("curly quotes (though phones insert those by themselves)".to_string());
        families += 1;
    }

    let low = text.to_lowercase();
    let hits: Vec<&str> = REGISTER.iter().copied().filter(|w| low.contains(w)).collect();
    if !hits.is_empty() {
        score += (hits.len() as f64 * W_REGISTER_EACH).min(W_REGISTER_MAX);
        let shown: Vec<String> = hits.iter().take(3).map(|w| format!("“{}”", w.trim_end_matches(','))).collect();
        reasons.push(format!("turns of phrase like {}", shown.join(", ")));
        families += 1;
    }

    if let Some(why) = uniform_sentences(text) {
        score += W_UNIFORM;
        reasons.push(why);
        families += 1;
    }

    if let Some(why) = too_clean(text) {
        score += W_CLEAN;
        reasons.push(why);
        families += 1;
    }

    let had_baseline = base.is_some();
    if let Some(base) = base {
        let (gap, why) = style_gap(base, &shape_of(text));
        if gap > 0.35 {
            score += gap * W_STYLE;
            reasons.extend(why);
            families += 1;
        }
    } else {
        reasons.push("no comparison with their usual writing: the bot hasn't seen enough of it yet".to_string());
    }

    Suspicion { score: score.clamp(0.0, 1.0), reasons, families, had_baseline }
}

// --- the second opinion -----------------------------------------------------------

/// What the model is told before it is shown anything. The point of spelling
/// this out is that a model asked "is this AI?" will happily say yes about
/// careful English written by a careful person.
pub const RUBRIC: &str = "\
You are helping a Discord moderator decide whether a message was probably written by an AI assistant \
and pasted in, or by the member themselves. Your answer is only advice; a human decides, and nothing \
is deleted because of what you say.

Read these rules before you judge:
1. English as a second language is NOT evidence of AI. This server is mostly Indian; many members write \
   careful, formal, correctly punctuated English learned at school, and mix in Hindi. Formality, perfect \
   grammar, textbook vocabulary and an absence of slang are NOT evidence. Nor is the opposite.
2. Short or plain text CANNOT be judged. If the message is short, or is ordinary conversation with \
   nothing distinctive about it, answer judgeable=false and confidence 0. Say so rather than guessing.
3. What is worth something: chat that is laid out like a document (headings, bulleted lists, bold \
   run-ins), a tidy summarise-explain-conclude arc in a casual channel, the same sentence rhythm all the \
   way through, register words that belong to written essays, an answer that addresses a question nobody \
   quite asked, and hedging disclaimers.
4. Compare the message with the member's own earlier messages, which are given below. A sudden change in \
   how one person writes is worth more than anything about the text on its own — but remember people \
   write differently when they are being serious, when they are upset, and when they are at a keyboard \
   instead of a phone.
5. Never mention the member's nationality, and never treat an accent in writing as a fault.

Answer with JSON only, in this exact shape:
{\"judgeable\": true or false, \"confidence\": 0.0 to 1.0, \"reasons\": \"one or two plain sentences\"}";

/// The prompt for one message: the rubric, a few of the member's own messages,
/// and the message in question.
pub fn model_prompt(text: &str, samples: &[String], base: Option<&Baseline>) -> String {
    let mut out = String::from(RUBRIC);
    out.push_str("\n\n--- How this member usually writes ---\n");
    match base {
        Some(b) => out.push_str(&format!(
            "Across {} messages the bot has seen: about {} characters a message, {}% of their words are Hindi or Hinglish, \
             {} emoji per 100 characters, {}% of their sentences start with a capital.\n",
            b.messages,
            b.mean_chars.round() as i64,
            (b.hinglish_share * 100.0).round() as i64,
            (b.emoji_rate * 10.0).round() / 10.0,
            (b.caps_share * 100.0).round() as i64,
        )),
        None => out.push_str(
            "The bot has not seen enough of this member's writing to say. Judge the message on its own, and be \
             correspondingly less sure.\n",
        ),
    }
    if samples.is_empty() {
        out.push_str("No earlier messages of theirs are available.\n");
    } else {
        out.push_str("Some of their earlier messages, newest last:\n");
        for s in samples {
            out.push_str(&format!("- {}\n", excerpt(s)));
        }
    }
    out.push_str("\n--- The message to judge ---\n");
    out.push_str(text);
    out.push_str("\n\nJSON only.");
    out
}

/// Reads the model's answer. Anything unreadable is no answer at all, which is
/// treated as "the model was not asked" rather than as agreement.
pub fn parse_model(reply: &str, model: &str) -> Option<ModelSay> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    if end <= start {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&reply[start..=end]).ok()?;
    let judgeable = value.get("judgeable").and_then(|v| v.as_bool()).unwrap_or(true);
    let raw = value.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
    // Some models answer 0-100 however they are asked.
    let confidence = if raw > 1.0 { raw / 100.0 } else { raw };
    let reasons = match value.get("reasons") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join("; "),
        _ => String::new(),
    };
    Some(ModelSay {
        model: model.to_string(),
        confidence: if judgeable { confidence.clamp(0.0, 1.0) } else { 0.0 },
        reasons: excerpt(&reasons),
        unjudgeable: !judgeable,
    })
}

/// Whether another model call fits inside the hourly cap, given how many have
/// already been made this hour. Counted from the stored rows rather than from
/// memory, so a restart cannot quietly hand the feature a fresh budget.
pub fn allow_model_call(asked_this_hour: u32, cap: u32) -> bool {
    asked_this_hour < cap
}

// --- what the log posts say ------------------------------------------------------------

fn jump(guild: u64, channel: u64, message: u64) -> String {
    format!("https://discord.com/channels/{}/{}/{}", guild, channel, message)
}

/// The one post a spam removal makes, however many messages it removed.
pub fn spam_log_text(hit: &Hit, member: u64, member_name: &str, guild: u64) -> String {
    let (channel, last) = hit.messages.last().copied().unwrap_or((0, 0));
    let mut out = format!("🧹 **Spam deleted** · **{}** (`{}`) · <#{}>\n", member_name, member, channel);
    out.push_str(&format!("**Rule** · {} — {}\n", hit.rule.label(), hit.detail));
    out.push_str(&format!("**Deleted** · {}\n", plural(hit.messages.len(), "message")));
    if !hit.text.is_empty() {
        out.push_str(&format!("**What it said** · {}\n", hit.text));
    }
    out.push_str(&format!("**Where** · <{}>", jump(guild, channel, last)));
    out
}

/// Why the model was not asked, when it was not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotAsked {
    /// The hourly cap was already used up.
    Capped,
    /// The call failed or the answer was unreadable.
    Unreachable,
    /// No model is configured.
    Off,
}

/// The one post an AI flag makes. It says, every time, that nothing was done —
/// because a moderator reading it quickly should never be left with the
/// impression that the bot has already decided something.
pub fn ai_log_text(
    s: &Suspicion,
    threshold: f64,
    text: &str,
    member: u64,
    member_name: &str,
    channel: u64,
    message: u64,
    guild: u64,
    model: Result<&ModelSay, NotAsked>,
) -> String {
    let mut out = String::from("🤖 **Possibly AI-written — a human should decide**\n");
    out.push_str(&format!("**Member** · **{}** (`{}`) · <#{}>\n", member_name, member, channel));
    out.push_str(&format!("**Score** · {:.2} of 1 (a human is asked at {:.2})\n", s.score, threshold));
    let why = if s.reasons.is_empty() { "nothing it can put in words".to_string() } else { s.reasons.join(" · ") };
    out.push_str(&format!("**Why** · {}\n", why));
    out.push_str(&match model {
        Ok(m) if m.unjudgeable => format!("**The model** · {} says this text can't be judged either way\n", m.model),
        Ok(m) => format!("**The model** · {} is {:.2} sure — {}\n", m.model, m.confidence, if m.reasons.is_empty() { "no reason given" } else { &m.reasons }),
        Err(NotAsked::Capped) => "**The model** · not asked — this hour's limit of second opinions was used up\n".to_string(),
        Err(NotAsked::Unreachable) => "**The model** · not asked — it couldn't be reached\n".to_string(),
        Err(NotAsked::Off) => "**The model** · not asked — second opinions are switched off\n".to_string(),
    });
    out.push_str(&format!("**What it said** · {}\n", excerpt(text)));
    out.push_str(&format!("**Where** · <{}>\n", jump(guild, channel, message)));
    if !s.had_baseline {
        out.push_str("_The bot hasn't seen enough of this member's writing to compare this with their usual style, so this is the generic signals only — which are weaker._\n");
    }
    out.push_str(
        "_Nothing has been deleted and nobody has been punished. There is no reliable way to detect AI writing, and \
         careful English written by someone who learned it as a second language reads exactly like this. Treat the \
         score as a reason to look, never as evidence._",
    );
    out
}

/// How the post reads once a moderator has decided.
pub fn decided_note(outcome: Outcome, by: u64) -> String {
    match outcome {
        Outcome::Deleted => format!("\n\n🗑️ <@{}> deleted it.", by),
        Outcome::Dismissed => format!("\n\n✅ <@{}> said this was **not** AI. Counted against this feature.", by),
        Outcome::Untouched => String::new(),
    }
}

/// The post with any earlier decision line taken off, so changing one's mind
/// rewrites the note instead of stacking a second one under the first.
pub fn without_decision(post: &str) -> &str {
    let cut = ["\n\n🗑️", "\n\n✅"].iter().filter_map(|mark| post.find(mark)).min();
    match cut {
        Some(at) => &post[..at],
        None => post,
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {}", word) } else { format!("{} {}s", n, word) }
}

// --- live wiring -------------------------------------------------------------------------

static RECENT: LazyLock<Mutex<Recent>> = LazyLock::new(|| Mutex::new(Recent::default()));

/// Opens the store and starts the hourly retention cut. Call once, from inside
/// the runtime.
pub fn start(workspace: &str) -> anyhow::Result<()> {
    store::start(workspace)?;
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(PURGE_EVERY);
        loop {
            tick.tick().await;
            let now = chrono::Utc::now().timestamp();
            store::push(store::Event::Purge { now, keep_days: keep_days() });
            RECENT.lock().sweep(now, 3600);
        }
    });
    Ok(())
}

/// Where a message was posted, from the cache. Nothing for a channel the cache
/// cannot place: a thread whose parent is unknown could be in #safe-corner.
fn place(ctx: &Context, guild: GuildId, channel: serenity::all::ChannelId) -> Option<Place> {
    let g = ctx.cache.guild(guild)?;
    match g.channels.get(&channel) {
        Some(c) => Some(Place { channel_id: channel.get(), parent_id: None, channel_name: c.name.clone(), parent_name: String::new() }),
        None => {
            let t = g.threads.iter().find(|t| t.id == channel)?;
            let parent = t.parent_id?;
            // A thread whose parent the cache doesn't know can't be checked, so
            // it is left alone: it could be a thread in #safe-corner.
            let parent_name = g.channels.get(&parent)?.name.clone();
            Some(Place { channel_id: channel.get(), parent_id: Some(parent.get()), channel_name: t.name.clone(), parent_name })
        }
    }
}

/// Whether the author is someone the rules must never touch. When it cannot be
/// told — no member on the event, no cached roles — the answer is yes, so the
/// bot does nothing rather than act on a moderator.
fn is_mod_now(ctx: &Context, msg: &Message) -> bool {
    if super::admin_ids().contains(&msg.author.id.get()) {
        return true;
    }
    let (Some(guild_id), Some(member)) = (msg.guild_id, msg.member.as_ref()) else { return true };
    let Some(guild) = ctx.cache.guild(guild_id) else { return true };
    super::house::has_mod_powers(&guild.roles, &member.roles)
}

/// The message handler's hook, for every message. Cheap: reads the event and
/// the cache, keeps a few numbers, and hands anything slow to its own task.
pub fn on_message(ctx: &Context, msg: &Message) {
    if !enabled() || !store::started() {
        return;
    }
    let Some(guild) = msg.guild_id else { return };
    if msg.author.bot || msg.webhook_id.is_some() {
        return;
    }
    let Some(place) = place(ctx, guild, msg.channel_id) else { return };
    if !acts_on(&place, true, false, is_mod_now(ctx, msg), &exempt_channels(), &control::insights::sensitive_channels()) {
        return;
    }

    let ts = msg.timestamp.unix_timestamp();
    let member = msg.author.id.get();
    let name = msg
        .member
        .as_ref()
        .and_then(|m| m.nick.clone())
        .or_else(|| msg.author.global_name.clone())
        .unwrap_or_else(|| msg.author.name.clone());

    // Every message adds to how this member writes, whether or not the AI half
    // is on: the baseline has to exist before it can be switched on usefully.
    note_style(member, ts, &msg.content);

    if spam_on() {
        let mentions = msg.mentions.iter().map(|u| u.id.get()).count() + msg.mention_roles.len();
        let seen = Seen { member_id: member, channel_id: place.channel_id, message_id: msg.id.get(), ts, text: msg.content.clone(), mentions };
        let hit = RECENT.lock().check(&seen, &spam_settings());
        if let Some(hit) = hit {
            let (ctx, name) = (ctx.clone(), name.clone());
            let place = place.clone();
            tokio::spawn(async move {
                handle_spam(ctx, hit, member, name, place, guild.get(), ts).await;
            });
        }
    }

    if ai_on() {
        let settings = ai_settings();
        if msg.content.chars().count() < settings.min_chars {
            return;
        }
        let (ctx, content) = (ctx.clone(), msg.content.clone());
        let place = place.clone();
        let message_id = msg.id.get();
        tokio::spawn(async move {
            look_for_ai(ctx, settings, member, name, content, place, guild.get(), message_id, ts).await;
        });
    }
}

/// Adds one message to the member's style totals, and keeps it as a sample of
/// their ordinary writing when it is a useful length.
fn note_style(member: u64, ts: i64, text: &str) {
    let shape = shape_of(text);
    if shape.chars == 0 {
        return;
    }
    let len = shape.chars as usize;
    let sample = (len >= store::SAMPLE_MIN_CHARS && len <= store::SAMPLE_CHARS).then(|| text.to_string());
    store::push(store::Event::Style(StyleUpdate {
        member_id: member,
        ts,
        chars: shape.chars,
        punct: shape.punct,
        emoji: shape.emoji,
        tokens: shape.tokens,
        hinglish_tokens: shape.hinglish_tokens,
        sentences: shape.sentences,
        caps_starts: shape.caps_starts,
        sample,
    }));
}

/// Deletes the messages a spam rule found and writes one entry to the log.
async fn handle_spam(ctx: Context, hit: Hit, member: u64, name: String, place: Place, guild: u64, ts: i64) {
    let http = ctx.http.clone();
    // Each message is deleted from the channel it was actually in: a repeat is
    // usually spread across several.
    for (channel, id) in &hit.messages {
        if let Err(err) = serenity::all::ChannelId::new(*channel).delete_message(&http, MessageId::new(*id)).await {
            tracing::debug!("automod: couldn't delete message {} in {}: {}", id, channel, err);
        }
    }

    // A flood already logged moments ago is counted onto the same entry rather
    // than posted again, so one burst is one log post.
    if hit.quiet {
        let open = RECENT.lock().open_flag(member);
        if let Some(flag) = open {
            tracing::info!("automod: {} more messages removed from {} under flag {}", hit.messages.len(), member, flag);
            return;
        }
    }

    let flag = NewFlag {
        kind: Kind::Spam,
        rule: hit.rule.key().to_string(),
        member_id: member,
        member_name: name.clone(),
        channel_id: place.channel_id,
        channel_name: place.channel_name.clone(),
        guild_id: guild,
        message_id: hit.messages.last().map(|(_, m)| *m).unwrap_or(0),
        ts,
        text: hit.text.clone(),
        messages: hit.messages.len() as u32,
        score: None,
        reasons: vec![format!("{} — {}", hit.rule.label(), hit.detail)],
        model: None,
        model_asked: false,
        had_baseline: false,
        outcome: Outcome::Deleted,
    };
    let Some(id) = write_flag(flag).await else { return };
    let quiet_secs = spam_settings().quiet_secs;
    RECENT.lock().note_post(member, id, ts + quiet_secs);
    let text = spam_log_text(&hit, member, &name, guild);
    if let Some(posted) = post_to_log(&ctx, text, None).await {
        remember_log_message(id, posted).await;
    }
}

/// The AI half, end to end. Nothing in here can delete a message: the only
/// thing it produces is a [`Suspicion`] and a row for a human to read.
async fn look_for_ai(
    ctx: Context,
    settings: AiSettings,
    member: u64,
    name: String,
    text: String,
    place: Place,
    guild: u64,
    message_id: u64,
    ts: i64,
) {
    // Never judge the same message twice, whatever else happens.
    let seen_before = with_store(move |conn| store::already_judged(conn, message_id).unwrap_or(false)).await;
    if seen_before.unwrap_or(true) {
        return;
    }
    let row = with_store(move |conn| store::style(conn, member).ok().flatten()).await.flatten();
    let row = row.unwrap_or_default();
    let base = baseline(&row);
    let suspicion = free_signals(&text, base.as_ref());
    if !suspicion.worth_a_look(settings.threshold) {
        return;
    }

    // Only now, and only inside the hourly cap, is anything paid for.
    let mut model = Err(NotAsked::Off);
    let mut asked = false;
    if let Some(model_name) = settings.model.clone() {
        let used = with_store(move |conn| store::model_calls_since(conn, ts - HOUR).unwrap_or(u32::MAX)).await.unwrap_or(u32::MAX);
        if !allow_model_call(used, settings.max_hour) {
            model = Err(NotAsked::Capped);
        } else {
            asked = true;
            let prompt = model_prompt(&text, &row.samples, base.as_ref());
            model = match tokio::time::timeout(MODEL_WAIT, control::web::ask_bot_model_with(prompt, Some(model_name.clone()))).await {
                Ok(Ok(reply)) => parse_model(&reply, &model_name).ok_or(NotAsked::Unreachable),
                Ok(Err(err)) => {
                    tracing::warn!("automod: the second opinion failed: {}", err);
                    Err(NotAsked::Unreachable)
                }
                Err(_) => {
                    tracing::warn!("automod: the second opinion took over {}s", MODEL_WAIT.as_secs());
                    Err(NotAsked::Unreachable)
                }
            };
        }
    }

    let flag = NewFlag {
        kind: Kind::Ai,
        rule: "ai".into(),
        member_id: member,
        member_name: name.clone(),
        channel_id: place.channel_id,
        channel_name: place.channel_name.clone(),
        guild_id: guild,
        message_id,
        ts,
        text: text.clone(),
        messages: 1,
        score: Some(suspicion.score),
        reasons: suspicion.reasons.clone(),
        model: model.as_ref().ok().cloned(),
        model_asked: asked,
        had_baseline: suspicion.had_baseline,
        // Never anything else at this point: a flag is a question, not a verdict.
        outcome: Outcome::Untouched,
    };
    let Some(id) = write_flag(flag).await else { return };
    let post = ai_log_text(
        &suspicion,
        settings.threshold,
        &text,
        member,
        &name,
        place.channel_id,
        message_id,
        guild,
        model.as_ref().map_err(|e| *e),
    );
    if let Some(posted) = post_to_log(&ctx, post, Some(buttons(id, member))).await {
        remember_log_message(id, posted).await;
    }
}

/// The buttons under an AI flag. Only moderators may press them.
fn buttons(flag: i64, member: u64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("automod:del:{}", flag)).label("Delete it").emoji('🗑').style(serenity::all::ButtonStyle::Danger),
        CreateButton::new(format!("automod:ok:{}", flag)).label("Not AI").emoji('✅').style(serenity::all::ButtonStyle::Success),
        CreateButton::new(format!("automod:who:{}", member)).label("This member's flags").emoji('👤').style(serenity::all::ButtonStyle::Secondary),
    ])
}

async fn write_flag(flag: NewFlag) -> Option<i64> {
    with_store(move |conn| match store::add_flag(conn, &flag) {
        Ok(id) => id,
        Err(err) => {
            tracing::warn!("automod: couldn't write a flag: {}", err);
            None
        }
    })
    .await
    .flatten()
}

async fn remember_log_message(flag: i64, message: u64) {
    with_store(move |conn| {
        let _ = store::set_log_message(conn, flag, message);
    })
    .await;
}

/// Runs something against the store off the async threads. The lock is taken
/// inside the blocking task and dropped before it returns, so it is never held
/// across an `.await`.
async fn with_store<T: Send + 'static>(f: impl FnOnce(&rusqlite::Connection) -> T + Send + 'static) -> Option<T> {
    let db = store::db()?;
    tokio::task::spawn_blocking(move || {
        let conn = db.lock();
        f(&conn)
    })
    .await
    .ok()
}

async fn post_to_log(ctx: &Context, text: String, row: Option<CreateActionRow>) -> Option<u64> {
    let channel = serenity::all::ChannelId::new(log_channel()?);
    let mut message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Some(row) = row {
        message = message.components(vec![row]);
    }
    match channel.send_message(&ctx.http, message).await {
        Ok(m) => Some(m.id.get()),
        Err(err) => {
            tracing::warn!("automod: couldn't post to the moderation log: {}", err);
            None
        }
    }
}

// --- the buttons ---------------------------------------------------------------------

/// Whether whoever pressed a button may. The same test as the rules use for who
/// is never acted on, read off the interaction.
fn presser_is_mod(component: &ComponentInteraction, ctx: &Context) -> bool {
    if super::admin_ids().contains(&component.user.id.get()) {
        return true;
    }
    let (Some(guild_id), Some(member)) = (component.guild_id, component.member.as_ref()) else { return false };
    let Some(guild) = ctx.cache.guild(guild_id) else { return false };
    super::house::has_mod_powers(&guild.roles, &member.roles)
}

/// `automod:del:<flag>`, `automod:ok:<flag>` and `automod:who:<member>`.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let reply = |text: &str| {
        component.create_response(
            &ctx.http,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().ephemeral(true).content(text.to_string())),
        )
    };
    if !presser_is_mod(component, ctx) {
        let _ = reply("These buttons are for moderators.").await;
        return;
    }
    let by = component.user.id.get();
    let now = chrono::Utc::now().timestamp();

    if let Some(raw) = id.strip_prefix("automod:who:") {
        let Ok(member) = raw.parse::<u64>() else { return };
        let rows = with_store(move |conn| {
            store::list(conn, &store::ListFilter { member: Some(member), since: 0, limit: 10, ..Default::default() }).unwrap_or_default()
        })
        .await;
        let text = match rows {
            Some(page) if !page.rows.is_empty() => {
                let lines: Vec<String> = page
                    .rows
                    .iter()
                    .map(|f| {
                        format!(
                            "• <t:{}:d> · {} · {} · {}",
                            f.ts,
                            if f.kind == Kind::Spam { "spam" } else { "possibly AI" },
                            f.rule,
                            match f.outcome {
                                Outcome::Deleted => "deleted",
                                Outcome::Dismissed => "a mod said it was wrong",
                                Outcome::Untouched => "nothing done",
                            }
                        )
                    })
                    .collect();
                format!("**<@{}>'s last {} flags**\n{}", member, page.rows.len(), lines.join("\n"))
            }
            _ => format!("<@{}> has never been flagged.", member),
        };
        let _ = component
            .create_response(
                &ctx.http,
                CreateInteractionResponse::Message(
                    CreateInteractionResponseMessage::new()
                        .ephemeral(true)
                        .content(text)
                        .allowed_mentions(CreateAllowedMentions::new()),
                ),
            )
            .await;
        return;
    }

    let (raw, outcome) = match (id.strip_prefix("automod:del:"), id.strip_prefix("automod:ok:")) {
        (Some(raw), _) => (raw, Outcome::Deleted),
        (_, Some(raw)) => (raw, Outcome::Dismissed),
        _ => return,
    };
    let Ok(flag_id) = raw.parse::<i64>() else { return };
    let Some(Some(flag)) = with_store(move |conn| store::flag(conn, flag_id).ok().flatten()).await else {
        let _ = reply("That flag is gone.").await;
        return;
    };

    if outcome == Outcome::Deleted {
        let channel = serenity::all::ChannelId::new(flag.channel_id);
        if let Err(err) = channel.delete_message(&ctx.http, MessageId::new(flag.message_id)).await {
            tracing::debug!("automod: a mod's delete didn't go through: {}", err);
        }
    }
    let recorded = with_store(move |conn| store::decide(conn, flag_id, outcome, by, now).unwrap_or(false)).await;
    if recorded != Some(true) {
        let _ = reply("Couldn't record that. Try again.").await;
        return;
    }

    // The post itself says what happened, so the log reads as a record.
    let mut message = component.message.clone();
    let updated = format!("{}{}", without_decision(&message.content), decided_note(outcome, by));
    let _ = message
        .edit(&ctx.http, EditMessage::new().content(updated).components(vec![]).allowed_mentions(CreateAllowedMentions::new()))
        .await;
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await;
    tracing::info!("automod: flag {} marked {} by {}", flag_id, outcome.as_str(), by);
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAFE: u64 = super::super::weekly::SAFE_CORNER;
    const MEMBER: u64 = 1003;
    const NOW: i64 = 1_789_000_000;

    fn here(channel: u64, name: &str) -> Place {
        Place { channel_id: channel, parent_id: None, channel_name: name.into(), parent_name: String::new() }
    }

    fn thread(channel: u64, parent: u64, parent_name: &str) -> Place {
        Place { channel_id: channel, parent_id: Some(parent), channel_name: "thread".into(), parent_name: parent_name.into() }
    }

    fn msg(channel: u64, id: u64, ts: i64, text: &str) -> Seen {
        Seen::new(MEMBER, channel, id, ts, text)
    }

    // --- who is never touched ------------------------------------------------------

    #[test]
    fn mods_bots_dms_safe_corner_and_the_game_channels_are_never_touched() {
        let exempt = vec![1_542_764_196_901_683_231, 1_518_233_664_016_617_582, 1_516_846_913_452_769_280];
        let sensitive = vec![SAFE];
        let ok = |p: &Place, guild, bot, is_mod| acts_on(p, guild, bot, is_mod, &exempt, &sensitive);
        let general = here(21, "general");
        assert!(ok(&general, true, false, false), "an ordinary member in an ordinary channel");
        assert!(!ok(&general, true, false, true), "a moderator");
        assert!(!ok(&general, true, true, false), "a bot");
        assert!(!ok(&general, false, false, false), "a DM");
        assert!(!ok(&here(SAFE, "safe-corner"), true, false, false), "#safe-corner by id");
        assert!(!ok(&here(999, "🫂safe-corner"), true, false, false), "#safe-corner by name");
        for game in &exempt {
            assert!(!ok(&here(*game, "game"), true, false, false), "game channel {}", game);
        }
        // A thread inside an exempt channel, and inside #safe-corner - by id,
        // and by the parent's name, in case the channel was ever remade.
        assert!(!ok(&thread(77, exempt[0], "anagram"), true, false, false));
        assert!(!ok(&thread(77, SAFE, "safe-corner"), true, false, false));
        assert!(!ok(&thread(77, 4242, "🫂safe-corner"), true, false, false), "a safe corner under another id");
        assert!(ok(&thread(77, 21, "general"), true, false, false));
    }

    #[test]
    fn the_game_channels_are_exempt_whatever_the_setting_says() {
        // `exempt_channels` reads settings, so only the built-in part is checked
        // here; the setting can only ever add to it (see the function).
        for id in GAME_CHANNELS {
            assert!(exempt_channels().contains(id), "game channel {} must always be exempt", id);
        }
    }

    // --- the spam rules fire ---------------------------------------------------------

    #[test]
    fn the_same_message_over_and_over_is_spam_even_across_channels() {
        let s = SpamSettings::default();
        let mut r = Recent::default();
        let spam = "FREE NITRO 🎁 claim yours before it's gone";
        assert!(r.check(&msg(21, 1, NOW, spam), &s).is_none());
        assert!(r.check(&msg(22, 2, NOW + 5, spam), &s).is_none());
        let hit = r.check(&msg(23, 3, NOW + 11, "free nitro!! claim yours before it's gone"), &s).expect("the third one");
        assert_eq!(hit.rule, Rule::Repeat);
        assert_eq!(hit.ids(), vec![1, 2, 3], "all three go, not just the last");
        // Each message carries the channel it was in, so each is deleted where
        // it actually is rather than three times from the last channel.
        assert_eq!(hit.messages, vec![(21, 1), (22, 2), (23, 3)]);
        assert_eq!(hit.channels(), vec![21, 22, 23]);
        assert!(hit.detail.contains("3 near-identical messages"), "{}", hit.detail);
        assert!(hit.detail.contains("across 3 channels"), "{}", hit.detail);
        assert!(!hit.quiet);
    }

    #[test]
    fn a_repeat_outside_the_window_or_too_short_to_judge_is_not_spam() {
        let s = SpamSettings::default();
        let mut r = Recent::default();
        let spam = "join my server right now please";
        for (i, at) in [(1u64, NOW), (2, NOW + 40)] {
            assert!(r.check(&msg(21, i, at, spam), &s).is_none());
        }
        // The first has aged out of the 60-second window by now.
        assert!(r.check(&msg(21, 3, NOW + 90, spam), &s).is_none(), "only two are inside the window");
        // Short words repeat innocently all day long.
        let mut r = Recent::default();
        for (i, word) in ["haan", "same", "😭", "haan", "same", "ok"].iter().enumerate() {
            assert!(r.check(&msg(21, i as u64 + 1, NOW + i as i64 * 2, word), &s).is_none(), "{} is not spam", word);
        }
    }

    #[test]
    fn a_flood_is_spam_and_makes_exactly_one_log_post() {
        let s = SpamSettings::default();
        let mut r = Recent::default();
        let mut hits = Vec::new();
        // Twelve messages in six seconds, all different, as a paste-flood is.
        for i in 0..12u64 {
            let seen = msg(21, 100 + i, NOW + (i as i64 / 2), &format!("message number {} about nothing", i));
            if let Some(hit) = r.check(&seen, &s) {
                // What a moderator would do with the hit: the first one posts,
                // and the rest are quiet because a post was just made.
                if !hit.quiet {
                    r.note_post(MEMBER, 1, seen.ts + s.quiet_secs);
                }
                hits.push(hit);
            }
        }
        assert!(!hits.is_empty(), "a flood is caught");
        assert_eq!(hits[0].rule, Rule::Burst);
        assert_eq!(hits[0].messages.len(), s.burst_count, "the whole burst goes, in one go");
        let posts = hits.iter().filter(|h| !h.quiet).count();
        assert_eq!(posts, 1, "one burst is one log entry, not twenty");
        assert!(hits.iter().skip(1).all(|h| h.quiet), "the rest are removed quietly");
    }

    #[test]
    fn ordinary_chat_is_never_spam() {
        let s = SpamSettings::default();
        let mut r = Recent::default();
        // A real conversation: chatty, fast-ish, repetitive words, links, emoji.
        let chat = [
            (21u64, "bhai kal ka match dekha? kya scene tha", 0i64),
            (21, "RCB finally jeet gayi 😭😭", 6),
            (22, "ye meme dekho https://example.com/funny.png", 20),
            (21, "haan bro same feeling", 34),
            (21, "matlab kya hi bolun ab", 41),
            (23, "anyone up for valo tonight around 10?", 70),
            (23, "@Rohan tu aayega?", 75),
            (21, "lol", 120),
            (21, "lmao that's actually so true", 121),
            (21, "ok ok chalo", 123),
            (21, "sone ja raha hoon, good night sab log ❤️", 200),
        ];
        for (i, (channel, text, at)) in chat.iter().enumerate() {
            let seen = msg(*channel, 200 + i as u64, NOW + at, text);
            assert!(r.check(&seen, &s).is_none(), "ordinary chat flagged as spam: {}", text);
        }
    }

    #[test]
    fn mass_pings_everyone_invites_and_walls_are_spam() {
        let s = SpamSettings::default();
        let mut r = Recent::default();
        let mut seen = msg(21, 1, NOW, "oi @a @b @c @d @e @f come here");
        seen.mentions = 6;
        let hit = r.check(&seen, &s).expect("a mass ping");
        assert_eq!(hit.rule, Rule::Mentions);
        assert_eq!(hit.messages, vec![(21, 1)], "only the offending message");

        let mut r = Recent::default();
        assert_eq!(r.check(&msg(21, 2, NOW, "@everyone free stuff"), &s).unwrap().rule, Rule::Everyone);
        let mut r = Recent::default();
        assert_eq!(r.check(&msg(21, 3, NOW, "@here quick question"), &s).unwrap().rule, Rule::Everyone);

        let mut r = Recent::default();
        assert_eq!(r.check(&msg(21, 4, NOW, "join us at discord.gg/abc123"), &s).unwrap().rule, Rule::Invite);
        let mut r = Recent::default();
        assert_eq!(r.check(&msg(21, 5, NOW, "https://discord.com/invite/xYz9"), &s).unwrap().rule, Rule::Invite);

        let mut r = Recent::default();
        let wall = "a".repeat(500);
        assert_eq!(r.check(&msg(21, 6, NOW, &wall), &s).unwrap().rule, Rule::Wall);
        let mut r = Recent::default();
        let emoji_wall = "😭".repeat(420);
        assert_eq!(r.check(&msg(21, 7, NOW, &emoji_wall), &s).unwrap().rule, Rule::Wall);
    }

    #[test]
    fn near_misses_of_each_rule_are_left_alone() {
        let s = SpamSettings::default();
        let mut fresh = || Recent::default();

        // Five mentions when the limit is six.
        let mut seen = msg(21, 1, NOW, "@a @b @c @d @e look at this");
        seen.mentions = 5;
        assert!(fresh().check(&seen, &s).is_none());
        // Talking about Discord invites without posting one.
        for text in [
            "the discord.gg link expired, can someone repost",
            "i think discord.com/invite pages are down",
            "everyone should see this (not @everyone though, relax)",
        ] {
            let hit = fresh().check(&msg(21, 2, NOW, text), &s);
            let named = text.contains("@everyone");
            assert_eq!(hit.is_some(), named, "{}", text);
        }
        // A long, ordinary message: long is not a wall.
        let essay = "I've been thinking about this for a while and honestly the whole thing is a mess. \
                     We keep saying we'll fix the scheduling but nobody wants to be the one to say no to people. \
                     Anyway here is what I would do if it were up to me, which it is not, obviously."
            .repeat(2);
        assert!(fresh().check(&msg(21, 3, NOW, &essay), &s).is_none(), "a long message is not a wall");
        // A fast run of short reactions, just under the flood count. An excited
        // member firing off "lol" and "😭" must never lose them.
        let mut r = fresh();
        for i in 0..(s.burst_count as u64 - 1) {
            let seen = msg(21, 10 + i, NOW + i as i64, ["lol", "😭", "no way", "bhai", "same", "hahaha", "stop"][i as usize % 7]);
            assert!(r.check(&seen, &s).is_none(), "message {} of a fast reaction run", i);
        }
    }

    #[test]
    fn a_rule_with_its_number_at_zero_is_switched_off() {
        let off = SpamSettings {
            repeat_count: 0,
            burst_count: 0,
            max_mentions: 0,
            everyone: false,
            invites: false,
            wall_chars: 0,
            ..SpamSettings::default()
        };
        let mut r = Recent::default();
        let mut seen = msg(21, 1, NOW, "@everyone discord.gg/abc123 @a @b @c @d @e @f @g");
        seen.mentions = 20;
        assert!(r.check(&seen, &off).is_none());
        for i in 0..20u64 {
            assert!(r.check(&msg(21, 50 + i, NOW, "FREE NITRO claim yours now"), &off).is_none());
        }
        assert!(r.check(&msg(21, 99, NOW, &"x".repeat(5000)), &off).is_none());
    }

    // --- the AI half never deletes -----------------------------------------------------

    /// The promise in the module docs, checked against every score and setting
    /// this code can produce: nothing the AI half returns can remove a message.
    #[test]
    fn no_score_and_no_setting_makes_the_ai_half_delete_anything() {
        let very_ai = "# Understanding the Question\n\nMoreover, it is worth noting that this is a multifaceted \
            issue — one that requires us to delve into the tapestry of considerations.\n\n- First, the context matters.\n\
            - Second, the stakes are high.\n- Third, we must navigate the complexities.\n\nIn conclusion, it is a \
            testament to the ever-evolving landscape of the discussion that **no single answer suffices**.";
        let base = Baseline { messages: 400, mean_chars: 38.0, punct_rate: 2.0, emoji_rate: 3.0, hinglish_share: 0.4, caps_share: 0.1 };
        for threshold in [0.2, 0.4, 0.65, 0.9, 1.0] {
            for with_base in [None, Some(&base)] {
                let s = free_signals(very_ai, with_base);
                assert!((0.0..=1.0).contains(&s.score));
                // The type has nowhere to put a message id, and `Kind::Ai` is
                // never allowed to delete.
                assert!(!Kind::Ai.may_delete(), "an AI flag may never delete, at threshold {}", threshold);
                // It can be worth a look. That is the whole of what it can be.
                let _ = s.worth_a_look(threshold);
            }
        }
        assert!(Kind::Spam.may_delete(), "spam is the only thing the bot removes by itself");
    }

    #[test]
    fn a_message_too_short_to_judge_is_never_judged() {
        let settings = AiSettings::default();
        // This is the gate `on_message` applies before anything is scored.
        for text in ["ok", "haan bhai sahi hai", "I agree with this completely, well said.", &"word ".repeat(40)] {
            let short = text.chars().count() < settings.min_chars;
            assert_eq!(short, text.chars().count() < 280);
        }
        assert_eq!(settings.min_chars, 280);
        // And the rubric tells the model the same thing.
        assert!(RUBRIC.contains("CANNOT be judged"));
    }

    /// The regression that matters most: Hinglish and second-language English
    /// must not reach the threshold on the free signals alone.
    #[test]
    fn hinglish_and_esl_writing_does_not_reach_the_threshold() {
        let settings = AiSettings::default();
        let messages: &[&str] = &[
            // Long Hinglish rant, mobile-typed.
            "bhai sach batau toh mujhe samajh nahi aa raha ki ye log kar kya rahe hain. har baar meeting hoti hai, \
             sab log bolte hain haan haan kar denge, aur phir kuch nahi hota. main akela kitna karun yaar. kal bhi \
             raat ke 2 baje tak baitha tha laptop le kar aur subah uthke dekha toh koi reply hi nahi. ab bolo main \
             kya karun is situation mein, thoda toh sochna chahiye na sabko 😭",
            // Careful formal English by someone who learned it at school: no
            // contractions, textbook vocabulary, perfect punctuation.
            "Respected seniors, I would like to bring to your kind attention a matter which has been troubling me \
             for quite some time. Whenever we are organising the weekly events, the responsibilities are not \
             distributed in a proper manner. As a result, the same few members are doing all of the work while \
             others are simply remaining silent. I am not blaming anybody, but I feel that a clear system should \
             be made so that everyone gets an equal opportunity to contribute. Kindly consider my suggestion and \
             let me know your valuable opinion on the same. Thanking you in advance for your time and patience.",
            // A long, emotional, tidy message. Serious mood, careful typing.
            "I have been thinking about this the whole week and I want to say it properly instead of joking about \
             it like I usually do. When my father was in the hospital last year, a lot of people from this server \
             messaged me, and some of them I had never even spoken to before. I did not reply to most of them \
             because I did not know what to say at that time. But I read every single one of those messages, and \
             they helped me more than I can explain here. I just wanted all of you to know that, even if it is \
             very late to be saying it now.",
            // The hardest case there is: school-essay English. "Firstly",
            // "Moreover", "Furthermore", "In conclusion" is what Indian schools
            // teach, and it is also exactly what a detector calls AI. On the
            // free signals this sits within a hair of a genuinely pasted answer
            // (measured: 0.45 against 0.47), which is precisely why this half
            // flags for a person and never acts.
            "Respected all, I would like to share my opinion regarding the recent changes. Firstly, the timings are \
             not convenient for working members. Moreover, the announcements are being made at very short notice. \
             Furthermore, many of us are not able to participate because of examination season. In conclusion, I \
             would humbly request the organisers to kindly reconsider the schedule and inform everybody at least \
             one week in advance so that proper arrangements can be made by all concerned.",
            // A technical explanation with a couple of structural habits.
            "So the reason your build is failing is that the lockfile and the package file have gone out of sync. \
             When you run install with the flag for a clean install, it refuses to update the lockfile, and it \
             errors out instead of silently fixing it. The easiest fix is to delete the lockfile and the modules \
             folder and install again from scratch, then commit whatever the new lockfile says. If that still \
             does not work, check whether you are on the same node version as the pipeline, because that has \
             bitten us twice already this month.",
            // Mixed-script Hinglish with Devanagari.
            "यार सच में, मुझे लगता है कि हम लोग बहुत ज्यादा सोच रहे हैं इस बारे में। it is honestly not that deep. \
             kal milte hain aur baith ke decide kar lenge, abhi ke liye sab log thoda relax karo. jo hoga dekha \
             jaayega, itna tension lene ka koi matlab nahi banta. main subah se yahi soch raha tha ki kaise bolun \
             but ab bol diya, bas itna hi kehna tha mujhe sabse.",
        ];
        // With no baseline, and with a baseline that says they normally write
        // short Hinglish — the harder case, because a long careful message is
        // genuinely unlike their usual writing.
        let short_hinglish =
            Baseline { messages: 900, mean_chars: 36.0, punct_rate: 1.5, emoji_rate: 2.5, hinglish_share: 0.45, caps_share: 0.08 };
        for text in messages {
            for base in [None, Some(&short_hinglish)] {
                let s = free_signals(text, base);
                assert!(
                    !s.worth_a_look(settings.threshold),
                    "second-language writing reached the AI threshold ({:.2} of {:.2}, {} signals: {:?}):\n{}",
                    s.score,
                    settings.threshold,
                    s.families,
                    s.reasons,
                    text
                );
            }
        }
    }

    #[test]
    fn writing_that_looks_generated_does_reach_the_threshold() {
        let settings = AiSettings::default();
        let pasted = "# Should we change the schedule?\n\nThat's a great question — and one worth unpacking. \
            Moreover, it is worth noting that there are several factors at play here.\n\n\
            - **Consistency** is crucial for member engagement.\n\
            - **Flexibility** allows for a more inclusive environment.\n\
            - **Feedback** ensures the community feels heard.\n\n\
            In conclusion, the optimal approach is a balanced one that leverages the strengths of both models.";
        let s = free_signals(pasted, None);
        assert!(s.worth_a_look(settings.threshold), "score {:.2}, signals {:?}", s.score, s.reasons);
        assert!(s.families >= MIN_FAMILIES);
        assert!(!s.had_baseline);
        assert!(s.reasons.iter().any(|r| r.contains("hasn't seen enough")), "the flag says there was no baseline: {:?}", s.reasons);
    }

    #[test]
    fn one_signal_on_its_own_is_never_enough() {
        // An em dash and nothing else: a high enough weight would be silly.
        let only_dash = "I was going to go to the thing tonight — but honestly I am too tired now, so maybe next \
                         time. sorry for the late notice yaar, i know you were counting on me being there.";
        let s = free_signals(only_dash, None);
        assert!(!s.worth_a_look(0.2), "one family of signal must never carry a flag on its own");
    }

    #[test]
    fn a_member_with_no_history_gets_the_generic_signals_only() {
        let nothing = StyleRow::default();
        assert_eq!(baseline(&nothing), None, "a member the bot has never seen");
        let thin = StyleRow { messages: 5, chars: 200, tokens: 40, ..Default::default() };
        assert_eq!(baseline(&thin), None, "five messages is not a style");
        let wordy_but_few = StyleRow { messages: 3, chars: 5000, tokens: 900, ..Default::default() };
        assert_eq!(baseline(&wordy_but_few), None, "three long messages is not a style either");
        let enough = StyleRow {
            messages: 40,
            chars: 1600,
            punct: 40,
            emoji: 30,
            tokens: 300,
            hinglish_tokens: 120,
            sentences: 50,
            caps_starts: 5,
            samples: vec![],
        };
        let base = baseline(&enough).expect("enough history");
        assert_eq!(base.mean_chars, 40.0);
        assert!((base.hinglish_share - 0.4).abs() < 1e-9);
        assert!((base.caps_share - 0.1).abs() < 1e-9);

        // And the flag for someone with no baseline says so, in the post too.
        let s = Suspicion { score: 0.8, reasons: vec!["headings".into()], families: 2, had_baseline: false };
        let post = ai_log_text(&s, 0.65, "text", 1003, "Rohan", 21, 5, 900, Err(NotAsked::Off));
        assert!(post.contains("hasn't seen enough of this member's writing"));
    }

    #[test]
    fn the_style_gap_measures_someone_against_themselves() {
        let base = Baseline { messages: 500, mean_chars: 40.0, punct_rate: 2.0, emoji_rate: 3.0, hinglish_share: 0.4, caps_share: 0.1 };
        let usual = shape_of("bhai kal milte hain, abhi thoda busy hoon 😅");
        let (gap, _) = style_gap(&base, &usual);
        assert!(gap < 0.25, "their own usual writing is close to their baseline ({:.2})", gap);
        let unlike = shape_of(
            "Certainly. There are three considerations to weigh here. The first concerns the allocation of \
             responsibility. The second concerns the timeline we have agreed upon. The third concerns the \
             expectations of the wider group. Each deserves separate attention.",
        );
        let (gap, why) = style_gap(&base, &unlike);
        assert!(gap > 0.4, "a very different message shows up as one ({:.2})", gap);
        assert!(!why.is_empty(), "and it can say why");
    }

    // --- the model is asked sparingly ---------------------------------------------------

    #[test]
    fn the_model_is_only_asked_above_the_threshold_and_inside_the_cap() {
        let settings = AiSettings::default();
        // Below the threshold, nothing is asked: `look_for_ai` returns before
        // the call, which is exactly what `worth_a_look` decides.
        let ordinary = free_signals(
            "hey everyone, i will be a bit late to the call tonight because of traffic, please start without me \
             and i will catch up whenever i reach home. sorry about this, i did not plan it this way at all.",
            None,
        );
        assert!(!ordinary.worth_a_look(settings.threshold));

        // The cap counts the rows themselves, so a restart can't hand the
        // feature a fresh hour's budget.
        let conn = store::open_memory().unwrap();
        let ask = |conn: &rusqlite::Connection, message: u64, ts: i64, asked: bool| {
            let flag = NewFlag {
                kind: Kind::Ai,
                rule: "ai".into(),
                member_id: 1004,
                member_name: "Zoya".into(),
                channel_id: 21,
                channel_name: "general".into(),
                guild_id: 900,
                message_id: message,
                ts,
                text: "a long message".into(),
                messages: 1,
                score: Some(0.7),
                reasons: vec![],
                model: None,
                model_asked: asked,
                had_baseline: false,
                outcome: Outcome::Untouched,
            };
            store::add_flag(conn, &flag).unwrap();
        };
        for i in 0..settings.max_hour {
            let used = store::model_calls_since(&conn, NOW - HOUR).unwrap();
            assert!(allow_model_call(used, settings.max_hour), "call {} fits", i);
            ask(&conn, 9000 + i as u64, NOW, true);
        }
        let used = store::model_calls_since(&conn, NOW - HOUR).unwrap();
        assert_eq!(used, settings.max_hour);
        assert!(!allow_model_call(used, settings.max_hour), "the cap holds");
        // An hour on, the old calls are outside the window again.
        assert_eq!(store::model_calls_since(&conn, NOW + 3601 - HOUR).unwrap(), 0);
        assert!(allow_model_call(0, settings.max_hour));
        // A flag raised on the free signals alone never counted against it.
        ask(&conn, 9999, NOW, false);
        assert_eq!(store::model_calls_since(&conn, NOW - HOUR).unwrap(), settings.max_hour);
        // A cap of nothing asks nothing, ever.
        assert!(!allow_model_call(0, 0));
    }

    #[test]
    fn changing_a_mods_mind_rewrites_the_note_rather_than_stacking_one() {
        let post = "🤖 **Possibly AI-written**\n**Where** · <link>";
        assert_eq!(without_decision(post), post, "an undecided post is left as it is");
        let dismissed = format!("{}{}", post, decided_note(Outcome::Dismissed, 1001));
        assert_eq!(without_decision(&dismissed), post);
        let changed = format!("{}{}", without_decision(&dismissed), decided_note(Outcome::Deleted, 1002));
        assert_eq!(changed, format!("{}\n\n🗑️ <@1002> deleted it.", post));
        assert_eq!(changed.matches("<@100").count(), 1, "only the latest decision is shown");
    }

    #[test]
    fn the_models_answer_is_read_carefully_or_not_at_all() {
        let say = parse_model(r#"{"judgeable": true, "confidence": 0.72, "reasons": "Document-like structure."}"#, "lite").unwrap();
        assert_eq!((say.confidence, say.unjudgeable), (0.72, false));
        assert_eq!(say.reasons, "Document-like structure.");
        // Percentages, prose around the JSON, and a list of reasons.
        let say = parse_model("Sure! Here you go:\n{\"judgeable\": true, \"confidence\": 85, \"reasons\": [\"a\", \"b\"]}\nHope that helps.", "lite").unwrap();
        assert!((say.confidence - 0.85).abs() < 1e-9);
        assert_eq!(say.reasons, "a; b");
        // Told it cannot judge, its confidence is thrown away, not kept.
        let say = parse_model(r#"{"judgeable": false, "confidence": 0.9, "reasons": "Too short."}"#, "lite").unwrap();
        assert_eq!((say.confidence, say.unjudgeable), (0.0, true));
        // Nothing usable is no answer, never agreement.
        assert!(parse_model("I'm sorry, I can't help with that.", "lite").is_none());
        assert!(parse_model("", "lite").is_none());
        assert!(parse_model("}{", "lite").is_none());
    }

    #[test]
    fn the_rubric_tells_the_model_what_is_not_evidence() {
        for must_say in ["second language is NOT evidence", "CANNOT be judged", "nationality"] {
            assert!(RUBRIC.contains(must_say), "the rubric must say: {}", must_say);
        }
        let base = Baseline { messages: 40, mean_chars: 40.0, punct_rate: 2.0, emoji_rate: 3.0, hinglish_share: 0.4, caps_share: 0.1 };
        let prompt = model_prompt("the message", &["their earlier message".to_string()], Some(&base));
        assert!(prompt.contains("their earlier message"), "the member's own writing goes with it");
        assert!(prompt.contains("40 messages"));
        assert!(prompt.ends_with("JSON only."));
        // With no baseline it says so rather than inventing one.
        let bare = model_prompt("the message", &[], None);
        assert!(bare.contains("has not seen enough of this member's writing"));
        assert!(bare.contains("No earlier messages"));
    }

    // --- what the posts say --------------------------------------------------------------

    #[test]
    fn a_spam_post_says_who_which_rule_where_what_and_links_to_it() {
        let hit = Hit {
            rule: Rule::Repeat,
            messages: vec![(22, 11), (22, 12), (23, 13)],
            text: "FREE NITRO 🎁 claim yours".into(),
            detail: "3 near-identical messages in 47 seconds, across 2 channels".into(),
            quiet: false,
        };
        let post = spam_log_text(&hit, 1003, "Rohan", 900);
        assert_eq!(
            post,
            "🧹 **Spam deleted** · **Rohan** (`1003`) · <#23>\n\
             **Rule** · The same message over and over — 3 near-identical messages in 47 seconds, across 2 channels\n\
             **Deleted** · 3 messages\n\
             **What it said** · FREE NITRO 🎁 claim yours\n\
             **Where** · <https://discord.com/channels/900/23/13>",
            "the link goes to the last message, in the channel that message was in"
        );
    }

    #[test]
    fn an_ai_post_says_plainly_that_nothing_was_done() {
        let s = Suspicion {
            score: 0.78,
            reasons: vec!["headings and a bulleted list — the shape of a document, not of chat".into(), "2 em dashes, which almost nobody types on a phone".into()],
            families: 3,
            had_baseline: true,
        };
        let say = ModelSay { model: "google/gemini-2.5-flash-lite".into(), confidence: 0.66, reasons: "Uniform register throughout.".into(), unjudgeable: false };
        let post = ai_log_text(&s, 0.65, "Moreover, it is worth noting...", 1004, "Zoya", 21, 77, 900, Ok(&say));
        assert_eq!(
            post,
            "🤖 **Possibly AI-written — a human should decide**\n\
             **Member** · **Zoya** (`1004`) · <#21>\n\
             **Score** · 0.78 of 1 (a human is asked at 0.65)\n\
             **Why** · headings and a bulleted list — the shape of a document, not of chat · 2 em dashes, which almost nobody types on a phone\n\
             **The model** · google/gemini-2.5-flash-lite is 0.66 sure — Uniform register throughout.\n\
             **What it said** · Moreover, it is worth noting...\n\
             **Where** · <https://discord.com/channels/900/21/77>\n\
             _Nothing has been deleted and nobody has been punished. There is no reliable way to detect AI writing, and \
             careful English written by someone who learned it as a second language reads exactly like this. Treat the \
             score as a reason to look, never as evidence._"
        );
        // Every reason the model might not have been asked reads clearly.
        for (why, words) in [
            (NotAsked::Capped, "this hour's limit"),
            (NotAsked::Unreachable, "couldn't be reached"),
            (NotAsked::Off, "switched off"),
        ] {
            let post = ai_log_text(&s, 0.65, "text", 1004, "Zoya", 21, 77, 900, Err(why));
            assert!(post.contains(words), "{:?} must read clearly", why);
            assert!(post.contains("Nothing has been deleted"));
        }
    }

    #[test]
    fn a_mods_decision_is_written_onto_the_post() {
        assert_eq!(decided_note(Outcome::Deleted, 1001), "\n\n🗑️ <@1001> deleted it.");
        assert!(decided_note(Outcome::Dismissed, 1001).contains("**not** AI"));
        assert!(decided_note(Outcome::Dismissed, 1001).contains("Counted against this feature"));
        assert_eq!(decided_note(Outcome::Untouched, 1001), "");
    }

    #[test]
    fn excerpts_are_one_line_and_short() {
        assert_eq!(excerpt("one\ntwo   three"), "one two three");
        assert_eq!(excerpt("with `backticks` in it"), "with backticks in it");
        let long = "x".repeat(500);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), EXCERPT_CHARS + 1);
        assert!(cut.ends_with('…'));
    }

    // --- the small pieces ------------------------------------------------------------------

    #[test]
    fn fingerprints_ignore_case_punctuation_and_emoji() {
        assert_eq!(fingerprint("FREE NITRO!!! 🎁"), "freenitro");
        assert_eq!(fingerprint("free... nitro"), "freenitro");
        assert!(near_same(&fingerprint("join my server now please"), &fingerprint("Join my server now, please!!")));
        assert!(near_same(&fingerprint("join my server now please"), &fingerprint("hey join my server now please ok")));
        assert!(!near_same(&fingerprint("join my server"), &fingerprint("what time is the quiz")));
        // Short strings never swallow each other.
        assert!(!near_same("ok", "okay that works for me"));
    }

    #[test]
    fn hinglish_and_emoji_are_recognised_well_enough_to_count() {
        for word in ["bhai", "Nahi", "कैसे", "yaar", "matlab"] {
            assert!(is_hinglish(word), "{}", word);
        }
        for word in ["moreover", "the", "consideration", "framework"] {
            assert!(!is_hinglish(word), "{}", word);
        }
        assert!(is_emoji('😭') && is_emoji('❤') && !is_emoji('a') && !is_emoji('—'));
        let shape = shape_of("bhai kal milte hain 😅. Theek hai?");
        assert_eq!(shape.sentences, 2);
        assert_eq!(shape.emoji, 1);
        assert!(shape.hinglish_tokens >= 4, "{:?}", shape);
    }

    #[test]
    fn walls_are_told_from_long_messages() {
        assert!(is_wall(&"ha".repeat(300), 400, 70));
        assert!(is_wall(&"lol ".repeat(200), 400, 70));
        assert!(!is_wall("short", 400, 70));
        let real = "I really think we should talk about this properly instead of arguing in the group chat. \
                    Everyone has been on edge since last week and it is not helping anybody to keep going like this."
            .repeat(3);
        assert!(!is_wall(&real, 400, 70), "ordinary prose is not a wall");
    }
}

