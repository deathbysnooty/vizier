//! Member notes, the part a model writes: "what they're like", built ahead of
//! time from a sample of the member's own messages and stored, so answering a
//! question about someone never spends a token.
//!
//! Everything here is plain logic with the model passed in, so the tests can
//! hand it a fake one: which messages are worth reading, the sample that fits a
//! token budget, the prompt, reading the answer, the filter that throws away a
//! bullet touching a banned topic or quoting someone, who is due a build, and
//! the run that does the builds and counts what they cost.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::LazyLock;

use chrono::TimeZone;
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::Connection;
use serde_json::Value;

use super::notes_store::{self as store, Note, Run};

/// Asked for, and kept at most.
pub const MIN_BULLETS: usize = 4;
pub const MAX_BULLETS: usize = 6;
/// One bullet's length once tidied.
pub const BULLET_CHARS: usize = 170;
/// One message's length in the sample.
pub const MESSAGE_CHARS: usize = 280;
/// Share of the budget that goes to the newest messages; the rest is spread
/// over the older ones.
pub const RECENT_SHARE: f64 = 0.5;
/// Someone with too little to go on is looked at again after this long.
pub const RETRY_SECS: i64 = 6 * 86_400;
/// The prompt's own words, roughly, in tokens: for estimates.
pub const PROMPT_OVERHEAD_TOKENS: i64 = 520;
/// What an answer costs, roughly, in tokens: for estimates.
pub const ANSWER_TOKENS: i64 = 170;

/// One message of theirs: when, and the words.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub ts: i64,
    pub text: String,
}

/// What the model said and what it cost.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub min_messages: usize,
    pub new_messages: i64,
    pub max_per_run: usize,
    pub token_budget: usize,
}

// --- which messages are worth reading ------------------------------------------------------

static USER_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>").expect("regex"));
static CHANNEL_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<#\d+>").expect("regex"));
static CUSTOM_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").expect("regex"));
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:https?://|www\.)\S+").expect("regex"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));
/// `!play`, `/about`, `.fish`, `$bal`, `;;p`, `p!help`, `owo hunt`: a command
/// for some bot, not the member talking.
static COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:[!/.$%?;>~=+-]+[a-z]|[a-z]{1,3}!\w|owo\b|uwu\b|pls\s+(?:beg|fish|hunt|dig|daily)\b)").expect("regex"));

/// The words of a message as the model should read them - mentions and links
/// taken out, custom emoji by name - or `None` for a bot command, a one-word
/// reply or anything else too thin to say something about the person.
pub fn usable(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || COMMAND.is_match(trimmed) {
        return None;
    }
    let t = USER_MENTION.replace_all(trimmed, "@someone");
    let t = CHANNEL_MENTION.replace_all(&t, "#channel");
    let t = CUSTOM_EMOJI.replace_all(&t, ":$1:");
    let t = URL.replace_all(&t, "");
    let t = SPACES.replace_all(t.trim(), " ").to_string();
    let words = t.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric) && !w.starts_with('@') && !w.starts_with(':')).count();
    let letters = t.chars().filter(|c| c.is_alphanumeric()).count();
    if words < 2 || (words < 3 && letters < 12) {
        return None;
    }
    Some(cut(&t, MESSAGE_CHARS))
}

/// Cut at a word, with an ellipsis.
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let head = match head.rfind(' ') {
        Some(i) if i > max / 2 => head[..i].to_string(),
        _ => head,
    };
    format!("{}…", head.trim_end())
}

/// A rough token count for Hinglish in Roman script: about 3.5 characters a token.
pub fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() * 2).div_ceil(7)
}

/// The usable messages, newest first, each once.
pub fn usable_messages(messages: &[Said]) -> Vec<Said> {
    let mut out: Vec<Said> = messages.iter().filter_map(|m| usable(&m.text).map(|text| Said { ts: m.ts, text })).collect();
    out.sort_by(|a, b| b.ts.cmp(&a.ts));
    let mut seen = HashSet::new();
    out.retain(|m| seen.insert(m.text.to_lowercase()));
    out
}

/// A representative sample inside `budget` tokens: the newest messages for
/// about half of it, the rest picked evenly across everything older, so a
/// running joke from months ago has as much chance as last night's. Comes back
/// oldest first. `usable` must already be newest first.
pub fn sample(usable: &[Said], budget: usize) -> Vec<Said> {
    let cost = |m: &Said| estimate_tokens(&m.text) + 2;
    let total: usize = usable.iter().map(cost).sum();
    let mut picked: Vec<Said> = if total <= budget {
        usable.to_vec()
    } else {
        let recent_budget = (budget as f64 * RECENT_SHARE) as usize;
        let mut used = 0;
        let mut split = 0;
        for m in usable {
            if used + cost(m) > recent_budget {
                break;
            }
            used += cost(m);
            split += 1;
        }
        let mut out: Vec<Said> = usable[..split].to_vec();
        let older = &usable[split..];
        if !older.is_empty() {
            let avg = older.iter().map(cost).sum::<usize>() / older.len();
            let room = budget.saturating_sub(used);
            let want = (room / avg.max(1)).clamp(1, older.len());
            let stride = older.len() as f64 / want as f64;
            let mut i = 0.0;
            while (i as usize) < older.len() {
                let m = &older[i as usize];
                if used + cost(m) <= budget {
                    used += cost(m);
                    out.push(m.clone());
                }
                i += stride;
            }
        }
        out
    };
    picked.sort_by_key(|m| m.ts);
    picked
}

// --- the prompt ------------------------------------------------------------------------------

fn month(ts: i64) -> String {
    super::stats::ist().timestamp_opt(ts, 0).single().map(|t| t.format("%B %Y").to_string()).unwrap_or_default()
}

pub fn prompt(sample: &[Said]) -> String {
    let mut lines = String::new();
    let mut last_month = String::new();
    for m in sample {
        let mo = month(m.ts);
        if mo != last_month {
            lines.push_str(&format!("\n[{}]\n", mo));
            last_month = mo;
        }
        lines.push_str(&format!("- {}\n", m.text));
    }
    format!(
        "You are writing a few private notes about one member of MLCI, an Indian Discord server of friends. \
Most people there write in Hinglish: Hindi typed in Roman letters, mixed freely with English, with Indian slang, \
short forms and inside jokes (\"bhai\", \"yaar\", \"scene kya hai\", \"op\", \"sahi hai\", \"lmao\"). Read it the way a fluent \
Hinglish speaker from the server would - it is not broken English and not typos. Messages mentioning other people \
show them as @someone.

Below are {count} of this member's own messages, oldest first, grouped by month. They are a sample spread across \
their time on the server.

Write {min} to {max} short bullets (at most 20 words each) in plain, friendly English about:
- the interests and topics they bring up
- their humour and tone
- running jokes or recurring bits
- what they are known for in the server
Start each bullet with a verb or an adjective (\"Brings up cricket every match day\", \"Dry, deadpan humour\"), \
without their name.

HARD RULES - a bullet that breaks any of them is thrown away:
- Never mention health or mental health, religion or caste, politics, sexuality or gender identity, \
relationships or who they are seeing, family, where they live, study or work, or when they are online and their \
daily routine. Leave these out even if the messages are full of them.
- Never quote a message word for word. Paraphrase in your own words.
- Nothing mean: describe, don't judge. No insults, no roasts, no guesses beyond what the messages show.

Reply with only a JSON object and nothing else: {{\"bullets\": [\"...\", \"...\"]}}

Their messages:
{lines}",
        count = sample.len(),
        min = MIN_BULLETS,
        max = MAX_BULLETS,
        lines = lines.trim_end(),
    )
}

// --- reading the answer ----------------------------------------------------------------------

static MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(?:[-*•·]+|\d+[.)])\s*").expect("regex"));

fn tidy_bullet(raw: &str) -> String {
    let t = MARKER.replace(raw, "");
    let t = t.trim().trim_matches(|c| c == '"' || c == '“' || c == '”').trim();
    cut(&SPACES.replace_all(t, " "), BULLET_CHARS)
}

/// The bullets in the model's answer: the JSON it was asked for, or failing
/// that, the lines that look like bullets.
pub fn parse_bullets(reply: &str) -> Vec<String> {
    let from_json = match (reply.find('{'), reply.rfind('}')) {
        (Some(a), Some(b)) if b > a => serde_json::from_str::<Value>(&reply[a..=b]).ok().and_then(|v| {
            v.get("bullets").and_then(Value::as_array).map(|items| items.iter().filter_map(Value::as_str).map(tidy_bullet).collect::<Vec<_>>())
        }),
        _ => None,
    };
    let bullets = from_json.unwrap_or_else(|| reply.lines().filter(|l| MARKER.is_match(l)).map(tidy_bullet).collect());
    bullets.into_iter().filter(|b| b.chars().filter(|c| c.is_alphabetic()).count() >= 6).collect()
}

// --- the filter ------------------------------------------------------------------------------

/// Each banned area and the words that give it away. A bullet that trips one
/// is thrown away - better a note one bullet short than one that crosses a line.
pub const BANNED: &[(&str, &str)] = &[
    (
        "health",
        r"health|healthy|ill|illness|sick|sickness|disease|doctor|doctors|hospital|medicine|medicines|medication|meds|therapy|therapist|depress\w*|anxiety|anxious|adhd|autis\w*|bipolar|mental|suicid\w*|self[- ]harm|trauma\w*|ptsd|diagnos\w*|disorder|surgery|cancer|diabet\w*|pregnan\w*|injur\w*|insomnia|lonely|loneliness|burn(?:ed|t)?[- ]out|stress(?:ed|ful)?|panic|grief|grieving",
    ),
    (
        "religion or caste",
        r"religio\w*|hindu\w*|muslim\w*|islam\w*|christian\w*|sikh\w*|jain|jains|buddhis\w*|atheis\w*|god|gods|allah|bhagwan|temple|mandir|mosque|masjid|church|gurudwara|namaz|pray\w*|puja|pooja|fasting|roza|ramadan|ramzan|eid|navratri|spiritual\w*|caste\w*|brahmin\w*|dalit\w*|rajput\w*|jaat|yadav|obc|upper[- ]caste|lower[- ]caste|hindutva",
    ),
    (
        "politics",
        r"politic\w*|election\w*|vote|votes|voting|voter\w*|bjp|congress|aap|modi|gandhi|kejriwal|government|govt|leftist\w*|left[- ]wing|right[- ]wing|rightist\w*|liberal\w*|conservative\w*|sanghi\w*|bhakt\w*|libtard\w*|rss|protest\w*|parliament|minister\w*|trump|biden|communis\w*|socialis\w*|nationalis\w*|party line",
    ),
    (
        "sexuality or gender identity",
        r"gay|lesbian\w*|bisexual\w*|queer|lgbt\w*|trans|transgender|non[- ]?binary|asexual|sexuality|sexual\w*|sex|gender\w*|pronoun\w*|closeted|coming out|came out",
    ),
    (
        "relationships",
        r"girlfriend\w*|boyfriend\w*|gf|bf|crush\w*|dating|relationship\w*|partner|partners|married|marriage|wife|husband|spouse|fianc\w*|ex|breakup\w*|broke up|single|situationship\w*|simp\w*|love life|flirt\w*|shaadi|bandi|seeing someone|romantic\w*|romance",
    ),
    (
        "family",
        r"family|families|mom|moms|mum|mother\w*|dad|dads|father\w*|parent|parents|sister\w*|brother\w*|sibling\w*|son|sons|daughter\w*|kid|kids|child|children|cousin\w*|uncle\w*|aunt\w*|aunty|auntie|grand\w*|mummy|papa|maa|didi|behen|nani|dadi|in[- ]laws?",
    ),
    (
        "where they live, study or work",
        r"lives?|living in|hometown|home town|city|village|neighbou?rhood|delhi|mumbai|bombay|bangalore|bengaluru|kolkata|calcutta|chennai|hyderabad|pune|jaipur|lucknow|noida|gurgaon|gurugram|ahmedabad|chandigarh|indore|bhopal|patna|kerala|punjab\w*|bihar\w*|gujarat\w*|rajasthan\w*|maharashtra\w*|karnataka|bengal\w*|assam\w*|goa|kashmir\w*|tamil nadu|telangana|odisha|jharkhand|haryana|uttar pradesh|canada|dubai|usa|abroad|school|schools|college|colleges|university|uni|classes|student|students|exam\w*|jee|neet|upsc|cbse|icse|iit\w*|nit|degree|semester|sem|hostel|campus|studies|studying|job|jobs|office|company|employer|boss|salary|intern\w*|career|profession\w*|colleague\w*|workplace|works as|works at|working at|coworker\w*",
    ),
    (
        "when they are online or their routine",
        r"online|offline|active at|night|nights|nightly|late[- ]night|midnight|morning\w*|evening\w*|afternoon\w*|\d{1,2}\s?(?:am|pm)|o'?clock|routine|schedule\w*|sleep\w*|asleep|wakes?|waking|woke|night owl|weekday\w*|weekend\w*|every day at|daily at|usually around|always around|hours of",
    ),
    (
        "unkind",
        r"stupid|dumb|idiot\w*|annoying|annoys|cringe\w*|toxic|rude|lazy|ugly|fat|loser\w*|creepy|obnoxious|arrogant|pathetic|attention[- ]seek\w*|irritating|immature|childish|spams?|spamming|clueless|whin\w*|brat\w*|jerk\w*",
    ),
];

static BANNED_RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    BANNED.iter().map(|(area, words)| (*area, Regex::new(&format!(r"(?i)\b(?:{})\b", words)).expect("banned words"))).collect()
});

static HARMLESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:(?:game|movie|quiz|trivia|karaoke|music)\s+nights?|online\s+(?:games?|gaming|chess|matches))\b").expect("regex"));

/// Text between quote marks, three words or more: a quotation.
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"["“”«»]([^"“”«»]+)["“”«»]"#).expect("regex"));

fn words_of(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// Every run of `n` words in these texts.
pub fn shingles(texts: &[String], n: usize) -> HashSet<String> {
    let mut out = HashSet::new();
    for t in texts {
        let w = words_of(t);
        for window in w.windows(n) {
            out.insert(window.join(" "));
        }
    }
    out
}

/// Words in a row that count as quoting a message.
pub const QUOTE_RUN: usize = 5;

/// Why a bullet must go, or `None` when it may stay: a banned area it touches,
/// a quotation, or a run of words lifted from one of their messages.
pub fn rejection(bullet: &str, source: &HashSet<String>) -> Option<&'static str> {
    // Events, not a routine: "hosts game nights", "plays online games".
    let checked = HARMLESS.replace_all(bullet, "event");
    for (area, re) in BANNED_RES.iter() {
        if re.is_match(&checked) {
            return Some(area);
        }
    }
    if QUOTED.captures_iter(bullet).any(|c| c[1].split_whitespace().count() >= 3) {
        return Some("a quotation");
    }
    let w = words_of(bullet);
    if w.windows(QUOTE_RUN).any(|window| source.contains(&window.join(" "))) {
        return Some("words lifted from a message");
    }
    None
}

/// The bullets that pass, at most [`MAX_BULLETS`], and how many were thrown away.
pub fn filter(bullets: Vec<String>, sample: &[Said]) -> (Vec<String>, usize) {
    let source = shingles(&sample.iter().map(|m| m.text.clone()).collect::<Vec<_>>(), QUOTE_RUN);
    let mut kept = Vec::new();
    let mut rejected = 0;
    let mut seen = HashSet::new();
    for b in bullets {
        match rejection(&b, &source) {
            Some(why) => {
                tracing::info!("notes: a bullet was thrown away ({})", why);
                rejected += 1;
            }
            None if seen.insert(b.to_lowercase()) && kept.len() < MAX_BULLETS => kept.push(b),
            None => {}
        }
    }
    (kept, rejected)
}

// --- one member ------------------------------------------------------------------------------

#[derive(Debug)]
pub enum Outcome {
    /// Fewer usable messages than the minimum: the model was not asked.
    TooLittle(usize),
    Built(Note),
    /// Asked, but nothing usable came back (every bullet was thrown away, or
    /// the answer couldn't be read). Tokens were still spent.
    Empty { input_tokens: i64, output_tokens: i64, rejected: usize },
    Failed(String),
}

/// Builds one member's notes from their messages. The model is only asked
/// when they have at least the minimum of usable messages.
pub async fn build_one<F, Fut>(user: u64, messages: &[Said], settings: &Settings, by: u64, now: i64, ask: F) -> Outcome
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Reply>>,
{
    let usable = usable_messages(messages);
    if usable.len() < settings.min_messages {
        return Outcome::TooLittle(usable.len());
    }
    let picked = sample(&usable, settings.token_budget);
    let reply = match ask(prompt(&picked)).await {
        Ok(r) => r,
        Err(err) => return Outcome::Failed(err.to_string()),
    };
    let (bullets, rejected) = filter(parse_bullets(&reply.text), &picked);
    let (input_tokens, output_tokens) = (reply.input_tokens as i64, reply.output_tokens as i64);
    if bullets.is_empty() {
        return Outcome::Empty { input_tokens, output_tokens, rejected };
    }
    Outcome::Built(Note {
        user_id: user,
        bullets,
        built_ts: now,
        input_tokens,
        output_tokens,
        model: reply.model,
        messages_used: picked.len() as i64,
        rejected: rejected as i64,
        built_by: by,
    })
}

// --- who is due ------------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct Candidate {
    pub user: u64,
    /// Messages of theirs on record, all time.
    pub total: i64,
    /// When their notes were last built, if ever.
    pub built_ts: Option<i64>,
    /// Their messages since then.
    pub new_since_build: i64,
    /// The last build that came to nothing (too little, or failed).
    pub last_attempt: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    First,
    Refresh,
}

/// Who gets built this run, in order, and how many more qualified but wait
/// for a later run. Never someone who opted out. First builds come first, the
/// most active first; then refreshes, the most new messages first - unless
/// `first_only`, for the daily runs of the first build, which leave refreshes
/// to the weekly run.
pub fn pick(cands: &[Candidate], optouts: &HashSet<u64>, s: &Settings, now: i64, first_only: bool) -> (Vec<(u64, Why)>, usize) {
    let mut first: Vec<&Candidate> = Vec::new();
    let mut refresh: Vec<&Candidate> = Vec::new();
    for c in cands {
        if optouts.contains(&c.user) {
            continue;
        }
        let recently_tried = c.last_attempt.is_some_and(|t| now - t < RETRY_SECS);
        match c.built_ts {
            None if c.total >= s.min_messages as i64 && !recently_tried => first.push(c),
            Some(_) if !first_only && c.new_since_build >= s.new_messages && !recently_tried => refresh.push(c),
            _ => {}
        }
    }
    first.sort_by(|a, b| b.total.cmp(&a.total).then(a.user.cmp(&b.user)));
    refresh.sort_by(|a, b| b.new_since_build.cmp(&a.new_since_build).then(a.user.cmp(&b.user)));
    let all: Vec<(u64, Why)> =
        first.iter().map(|c| (c.user, Why::First)).chain(refresh.iter().map(|c| (c.user, Why::Refresh))).collect();
    let waiting = all.len().saturating_sub(s.max_per_run);
    (all.into_iter().take(s.max_per_run).collect(), waiting)
}

/// What a full first build over these candidates would cost, roughly: how
/// many members, and tokens in and out. Everyone is assumed to fill the
/// budget, so this errs high.
pub fn first_build_estimate(cands: &[Candidate], optouts: &HashSet<u64>, s: &Settings) -> (usize, i64, i64) {
    let n = cands.iter().filter(|c| !optouts.contains(&c.user) && c.built_ts.is_none() && c.total >= s.min_messages as i64).count();
    (n, n as i64 * (s.token_budget as i64 + PROMPT_OVERHEAD_TOKENS), n as i64 * ANSWER_TOKENS)
}

// --- a run -----------------------------------------------------------------------------------

/// Builds the picked members one after another and records the run. `load`
/// fetches a member's messages; `ask` is the model. Opt-outs are checked again
/// right before each member, and the store refuses them anyway.
pub async fn run<L, LF, A, AF>(db: &Mutex<Connection>, kind: &str, picked: &[u64], waiting: usize, s: &Settings, by: u64, now: i64, load: L, ask: A) -> Run
where
    L: Fn(u64) -> LF,
    LF: Future<Output = Vec<Said>>,
    A: Fn(String) -> AF,
    AF: Future<Output = anyhow::Result<Reply>>,
{
    let mut report = Run { ts: now, kind: kind.to_string(), waiting: waiting as i64, ..Default::default() };
    for &user in picked {
        if store::opted_out(&db.lock(), user) {
            continue;
        }
        let messages = load(user).await;
        let outcome = build_one(user, &messages, s, by, now, &ask).await;
        let conn = db.lock();
        match outcome {
            Outcome::TooLittle(n) => {
                report.skipped += 1;
                let _ = store::note_attempt(&conn, user, now, &format!("too little: {} usable messages", n));
            }
            Outcome::Built(note) => {
                report.asked += 1;
                report.input_tokens += note.input_tokens;
                report.output_tokens += note.output_tokens;
                report.rejected += note.rejected;
                match store::save(&conn, &note) {
                    Ok(true) => report.built += 1,
                    Ok(false) => {}
                    Err(err) => {
                        tracing::warn!("notes: {} not saved: {}", user, err);
                        report.failed += 1;
                    }
                }
            }
            Outcome::Empty { input_tokens, output_tokens, rejected } => {
                report.asked += 1;
                report.failed += 1;
                report.input_tokens += input_tokens;
                report.output_tokens += output_tokens;
                report.rejected += rejected as i64;
                let _ = store::note_attempt(&conn, user, now, "nothing usable came back");
            }
            Outcome::Failed(err) => {
                report.failed += 1;
                tracing::warn!("notes: the model failed for {}: {}", user, err);
                let _ = store::note_attempt(&conn, user, now, "the model failed");
            }
        }
    }
    let conn = db.lock();
    if let Err(err) = store::record_run(&conn, &report) {
        tracing::warn!("notes: run not recorded: {}", err);
    }
    tracing::info!(
        "notes: {} run: {} model call(s), {} built, {} with too little to go on, {} failed, {} bullet(s) thrown away, \
         {} tokens in + {} out, {} waiting for a later run",
        report.kind,
        report.asked,
        report.built,
        report.skipped,
        report.failed,
        report.rejected,
        report.input_tokens,
        report.output_tokens,
        report.waiting
    );
    report
}

/// Candidates from message counts: `totals` all time, `since` the count since
/// each member's last build.
pub fn candidates(
    totals: &HashMap<u64, i64>,
    built: &HashMap<u64, i64>,
    since: &dyn Fn(u64, i64) -> i64,
    attempts: &HashMap<u64, (i64, String)>,
) -> Vec<Candidate> {
    totals
        .iter()
        .map(|(&user, &total)| {
            let built_ts = built.get(&user).copied();
            Candidate {
                user,
                total,
                built_ts,
                new_since_build: built_ts.map(|t| since(user, t)).unwrap_or(0),
                last_attempt: attempts.get(&user).map(|(t, _)| *t),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn settings() -> Settings {
        Settings { min_messages: 100, new_messages: 50, max_per_run: 3, token_budget: 4000 }
    }

    fn chat(n: usize) -> Vec<Said> {
        (0..n).map(|i| Said { ts: 1_780_000_000 + i as i64 * 3600, text: format!("aaj ka match dekha kya bhai number {}", i) }).collect()
    }

    fn fake(text: &str, calls: Arc<AtomicUsize>) -> impl Fn(String) -> std::future::Ready<anyhow::Result<Reply>> {
        let text = text.to_string();
        move |_prompt: String| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Ok(Reply { text: text.clone(), input_tokens: 3000, output_tokens: 120, model: "fake".into() }))
        }
    }

    #[test]
    fn commands_one_word_replies_and_links_are_not_read() {
        for skip in ["/about", "!play despacito", ".fish", "$bal", "p!help", "owo hunt", "lol", "haan", "ok bhai", "https://youtu.be/x", "<@123456789012345678>"] {
            assert!(usable(skip).is_none(), "{:?} should be skipped", skip);
        }
        assert_eq!(usable("<@123456789012345678> kal chess khelte hain yaar").as_deref(), Some("@someone kal chess khelte hain yaar"));
        assert!(usable("bhai ye anagram toh easy tha").is_some());
    }

    #[test]
    fn the_sample_keeps_the_newest_and_reaches_back_within_the_budget() {
        let msgs: Vec<Said> = (0..2000).map(|i| Said { ts: i, text: format!("message number {} about something fun", i) }).collect();
        let usable = usable_messages(&msgs);
        let picked = sample(&usable, 1000);
        let spent: usize = picked.iter().map(|m| estimate_tokens(&m.text) + 2).sum();
        assert!(spent <= 1000, "over budget: {}", spent);
        assert!(picked.iter().any(|m| m.ts == 1999), "the newest is in");
        assert!(picked.iter().any(|m| m.ts < 500), "older months are in too");
        assert!(picked.windows(2).all(|w| w[0].ts <= w[1].ts), "oldest first");
    }

    #[test]
    fn the_prompt_says_the_server_is_hinglish_and_carries_the_rules() {
        let p = prompt(&chat(3));
        assert!(p.contains("Hinglish"));
        for rule in ["health", "religion or caste", "politics", "sexuality", "relationships", "family", "where they live", "online", "word for word", "Nothing mean"] {
            assert!(p.contains(rule), "the prompt should say {:?}", rule);
        }
    }

    #[test]
    fn answers_are_read_as_json_or_as_lines() {
        assert_eq!(parse_bullets("```json\n{\"bullets\": [\"Loves chess puzzles\", \"- Dry humour, lots of puns\"]}\n```"), vec!["Loves chess puzzles", "Dry humour, lots of puns"]);
        assert_eq!(parse_bullets("Sure!\n- Loves chess puzzles\n2. Dry humour"), vec!["Loves chess puzzles", "Dry humour"]);
    }

    #[test]
    fn the_filter_throws_out_every_banned_area() {
        let none = HashSet::new();
        for (area, bullet) in [
            ("health", "Often talks about their anxiety before exams"),
            ("health", "Has been open about therapy"),
            ("religion or caste", "Shares memes about their religion"),
            ("religion or caste", "Proud of being a Rajput"),
            ("politics", "Argues about Modi and the BJP"),
            ("politics", "Strong political opinions"),
            ("sexuality or gender identity", "Openly gay and jokes about it"),
            ("sexuality or gender identity", "Talks about their pronouns"),
            ("relationships", "Keeps mentioning their girlfriend"),
            ("relationships", "Has a crush on someone in voice"),
            ("family", "Complains about their mom a lot"),
            ("family", "Has a younger brother who games"),
            ("where they live, study or work", "Lives in Pune and loves the rain"),
            ("where they live, study or work", "Preparing for JEE"),
            ("where they live, study or work", "Talks about their office job"),
            ("when they are online or their routine", "Always online late at night"),
            ("when they are online or their routine", "Shows up around 2am"),
            ("unkind", "Can be pretty annoying in voice"),
        ] {
            assert_eq!(rejection(bullet, &none), Some(area), "{:?}", bullet);
        }
        for fine in ["Hosts movie nights and plays online games", "Brings up cricket every match day", "Dry, deadpan humour with lots of puns", "Known for fast anagram solves", "Hypes everyone up in the chess channel"] {
            assert_eq!(rejection(fine, &none), None, "{:?}", fine);
        }
    }

    #[test]
    fn quoting_a_message_is_thrown_out() {
        let sample = vec![Said { ts: 1, text: "bhai kal raat wala match ekdum paisa vasool tha".into() }];
        let (kept, rejected) = filter(
            vec![
                "Says \"ekdum paisa vasool tha\" about matches".into(),
                "Called a match kal raat wala match ekdum paisa vasool".into(),
                "Gets very excited about cricket matches".into(),
            ],
            &sample,
        );
        assert_eq!(kept, vec!["Gets very excited about cricket matches"]);
        assert_eq!(rejected, 2);
    }

    #[tokio::test]
    async fn a_member_under_the_minimum_is_never_sent_to_the_model() {
        let calls = Arc::new(AtomicUsize::new(0));
        let out = build_one(1, &chat(99), &settings(), 0, 10, fake("{\"bullets\": [\"x\"]}", calls.clone())).await;
        assert!(matches!(out, Outcome::TooLittle(99)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        // One-word replies don't count towards the minimum either.
        let mut thin = chat(60);
        thin.extend((0..200).map(|i| Said { ts: i, text: "lol".into() }));
        let out = build_one(1, &thin, &settings(), 0, 10, fake("{}", calls.clone())).await;
        assert!(matches!(out, Outcome::TooLittle(60)));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_build_keeps_the_clean_bullets_and_the_tokens() {
        let calls = Arc::new(AtomicUsize::new(0));
        let answer = r#"{"bullets": ["Brings up cricket every match day", "Always online at midnight", "Loves anagrams and chess puzzles", "Hypes up their house", "Friendly, teasing humour"]}"#;
        let out = build_one(5, &chat(150), &settings(), 0, 10, fake(answer, calls.clone())).await;
        let Outcome::Built(note) = out else { panic!("expected a note") };
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(note.bullets.len(), 4);
        assert!(!note.bullets.iter().any(|b| b.contains("midnight")));
        assert_eq!((note.input_tokens, note.output_tokens, note.rejected), (3000, 120, 1));
    }

    fn cand(user: u64, total: i64, built: Option<i64>, new: i64) -> Candidate {
        Candidate { user, total, built_ts: built, new_since_build: new, last_attempt: None }
    }

    #[test]
    fn a_refresh_only_picks_members_with_enough_new_messages_and_keeps_to_the_cap() {
        let s = settings();
        let cands = vec![
            cand(1, 500, Some(0), 49),  // built, not enough new
            cand(2, 500, Some(0), 50),  // built, just enough
            cand(3, 500, Some(0), 300), // built, lots new
            cand(4, 90, None, 0),       // never built, too little
            cand(5, 400, None, 0),      // never built
            cand(6, 900, None, 0),      // never built, most active
            cand(7, 800, Some(0), 400), // opted out
        ];
        let optouts: HashSet<u64> = [7].into();
        let (picked, waiting) = pick(&cands, &optouts, &s, 1_000_000, false);
        assert_eq!(picked, vec![(6, Why::First), (5, Why::First), (3, Why::Refresh)]);
        assert_eq!(waiting, 1, "member 2 waits for the next run");
        let (all, _) = pick(&cands, &optouts, &Settings { max_per_run: 50, ..s.clone() }, 1_000_000, false);
        let users: Vec<u64> = all.iter().map(|(u, _)| *u).collect();
        assert_eq!(users, vec![6, 5, 3, 2]);
        // The daily runs of the first build leave the refreshes to the weekly run.
        let (first, waiting) = pick(&cands, &optouts, &Settings { max_per_run: 50, ..s.clone() }, 1_000_000, true);
        assert_eq!(first, vec![(6, Why::First), (5, Why::First)]);
        assert_eq!(waiting, 0);
        // Someone tried recently with too little isn't read again every run.
        let tried = vec![Candidate { last_attempt: Some(999_000), ..cand(8, 300, None, 0) }];
        assert!(pick(&tried, &HashSet::new(), &s, 1_000_000, false).0.is_empty());
        assert_eq!(pick(&tried, &HashSet::new(), &s, 999_000 + RETRY_SECS, false).0.len(), 1);
    }

    #[tokio::test]
    async fn a_run_spends_no_more_calls_than_the_cap_and_skips_opt_outs() {
        let conn = Mutex::new(store::memory());
        let s = settings();
        let cands: Vec<Candidate> = (1..=6).map(|u| cand(u, 1000 - u as i64, None, 0)).collect();
        store::opt_out(&mut conn.lock(), 2, 1).unwrap();
        let optouts = store::optouts(&conn.lock());
        let (picked, waiting) = pick(&cands, &optouts, &s, 100, true);
        let users: Vec<u64> = picked.iter().map(|(u, _)| *u).collect();
        assert_eq!(users, vec![1, 3, 4]);
        let calls = Arc::new(AtomicUsize::new(0));
        let answer = r#"{"bullets": ["Brings up cricket every match day", "Loves anagrams"]}"#;
        let report = run(&conn, "first", &users, waiting, &s, 0, 100, |_| std::future::ready(chat(150)), fake(answer, calls.clone())).await;
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert_eq!((report.asked, report.built, report.input_tokens, report.waiting), (3, 3, 9000, 2));
        assert!(store::get(&conn.lock(), 2).is_none());
        assert_eq!(store::runs(&conn.lock(), 5).len(), 1);
        // Opting out later deletes the note, and the next pick leaves them out.
        store::opt_out(&mut conn.lock(), 3, 200).unwrap();
        assert!(store::get(&conn.lock(), 3).is_none());
        let built = store::built_at(&conn.lock());
        let later: Vec<Candidate> = (1..=6).map(|u| cand(u, 1000, built.get(&u).copied(), 500)).collect();
        let (again, _) = pick(&later, &store::optouts(&conn.lock()), &Settings { max_per_run: 50, ..s }, 300, false);
        assert!(again.iter().all(|(u, _)| *u != 2 && *u != 3), "{:?}", again);
    }

    #[tokio::test]
    async fn a_run_refuses_someone_who_opted_out_after_being_picked() {
        let conn = Mutex::new(store::memory());
        store::opt_out(&mut conn.lock(), 9, 1).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let report = run(&conn, "weekly", &[9], 0, &settings(), 0, 5, |_| std::future::ready(chat(150)), fake("{}", calls.clone())).await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(report.asked, 0);
    }

    #[test]
    fn the_first_build_estimate_counts_only_who_qualifies() {
        let s = settings();
        let cands = vec![cand(1, 500, None, 0), cand(2, 50, None, 0), cand(3, 500, Some(1), 0)];
        let (n, input, output) = first_build_estimate(&cands, &HashSet::new(), &s);
        assert_eq!(n, 1);
        assert_eq!(input, 4000 + PROMPT_OVERHEAD_TOKENS);
        assert_eq!(output, ANSWER_TOKENS);
    }
}
