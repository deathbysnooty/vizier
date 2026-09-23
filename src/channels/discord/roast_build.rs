//! `/roast` and `/ship`, the part that is plain logic: what the bot knows about
//! a member gathered into a dossier, the prompts built from it, the check every
//! model reply must pass before anything is posted, the ship name and the ship
//! percentage, and where the result goes when the command was run somewhere
//! else.
//!
//! Nothing here touches Discord or a model - `roast.rs` does that - so the
//! tests can hand it a fake model and a made-up dossier.
//!
//! The rules are narrower than member notes'. A roast is meant to be mean: the
//! "unkind" filter notes use is deliberately not here, and neither is the ban on
//! talking about someone's routine or the games they lose. What stays banned is
//! the cruelty the owner ruled out - slurs, family, death, looks and body,
//! mental health, gender and sexuality, religion and caste - and those stay out
//! even when the member jokes about them themselves. #safe-corner never reaches
//! the model at all: its messages are dropped before the prompt is built.

use std::collections::HashSet;
use std::future::Future;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

pub use super::notes_build::Said;
use super::notes_build::{cut, estimate_tokens, sample, usable};

/// The messages one prompt may carry, in tokens, after the count cap. The same
/// idea as the notes budget: a sample spread over their whole time, not the
/// last hour.
pub const READ_BUDGET: usize = 3500;
/// The longest a roast may be once tidied. Discord allows 4096 in an embed
/// description; this leaves room and keeps the model honest.
pub const ROAST_CHARS: usize = 1400;
/// The longest a ship verdict may be.
pub const SHIP_CHARS: usize = 500;
/// A member with fewer messages on record than this has nothing to go on.
pub const THIN_MESSAGES: i64 = 40;
/// Tries at one model answer: one, then one more if the first is refused.
pub const TRIES: usize = 2;

// --- the dossier ------------------------------------------------------------------------------

/// Everything the bot can honestly say about one member. Only what is in here
/// may end up in a roast: the prompt says so, and nothing else is sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dossier {
    pub id: u64,
    pub name: String,
    /// How long they have been on the server.
    pub days_here: Option<i64>,
    pub messages_all: i64,
    pub messages_month: i64,
    /// The channels they live in, busiest first, by name.
    pub top_channels: Vec<String>,
    /// Their record in each game: (game, how they do).
    pub games: Vec<(String, String)>,
    pub house: Option<String>,
    pub points_month: i64,
    /// Where they come among their house's scorers this month, and how many scored.
    pub house_place: Option<(usize, usize)>,
    pub frog_cards: i64,
    pub voice_month_mins: i64,
    /// Messages of theirs Discord's AutoMod blocked.
    pub automod_blocked: i64,
    /// Messages of theirs that were deleted.
    pub deleted: i64,
    pub emoji: Option<String>,
    /// Phrases they say far more than anyone else does.
    pub phrases: Vec<String>,
    /// Who they talk with most, by name.
    pub partners: Vec<String>,
    /// A sample of their own messages, oldest first. Never #safe-corner.
    pub messages: Vec<Said>,
}

impl Dossier {
    /// Barely anything on record: the roast has to be about that.
    pub fn thin(&self) -> bool {
        self.messages.len() < 12 && self.messages_all < THIN_MESSAGES && self.games.is_empty() && self.voice_month_mins < 30
    }

    /// The record as the model reads it, one fact a line. Anything the bot
    /// doesn't know is simply missing rather than written as zero.
    pub fn facts(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(d) = self.days_here {
            out.push(format!("On the server for about {} days.", d));
        }
        out.push(format!("{} messages on record all time, {} in the last month.", self.messages_all, self.messages_month));
        if !self.top_channels.is_empty() {
            out.push(format!("Lives in: {}.", self.top_channels.iter().map(|c| format!("#{}", c)).collect::<Vec<_>>().join(", ")));
        }
        for (game, detail) in &self.games {
            out.push(format!("{}: {}.", game, detail));
        }
        if let Some(house) = &self.house {
            let place = match self.house_place {
                Some((place, of)) => format!(", {} of {} scorers in their house this month", ordinal(place), of),
                None => String::new(),
            };
            out.push(format!("House {}, {} points this month{}.", house, self.points_month, place));
        }
        if self.frog_cards > 0 {
            out.push(format!("{} frog cards hoarded.", self.frog_cards));
        }
        if self.voice_month_mins > 0 {
            out.push(format!("{} in voice chat in the last month.", hours(self.voice_month_mins)));
        }
        if self.automod_blocked > 0 {
            out.push(format!("AutoMod has eaten {} of their messages.", self.automod_blocked));
        }
        if self.deleted > 0 {
            out.push(format!("{} of their messages were deleted afterwards.", self.deleted));
        }
        if let Some(e) = &self.emoji {
            out.push(format!("Favourite emoji: {}.", e));
        }
        if !self.phrases.is_empty() {
            out.push(format!("Says far more often than anyone else: {}.", self.phrases.join(", ")));
        }
        if !self.partners.is_empty() {
            out.push(format!("Talks most with: {}.", self.partners.join(", ")));
        }
        out
    }
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

/// "2 hours 10 min", "45 min".
pub fn hours(mins: i64) -> String {
    match (mins / 60, mins % 60) {
        (0, m) => format!("{} min", m),
        (h, 0) => format!("{} hour{}", h, if h == 1 { "" } else { "s" }),
        (h, m) => format!("{} hour{} {} min", h, if h == 1 { "" } else { "s" }, m),
    }
}

/// How two members behave around each other, for `/ship`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Together {
    /// Channels they both turn up in, by name.
    pub shared_channels: Vec<String>,
    /// Replies from the first to the second, and back.
    pub replies_ab: usize,
    pub replies_ba: usize,
    /// Times one pinged the other.
    pub mentions: usize,
    /// Stretches of back-and-forth found in the period looked at.
    pub stretches: usize,
    /// Fights the kalesh detector called with both of them in it.
    pub fights: usize,
    /// Games they both play.
    pub shared_games: Vec<String>,
    /// A few of the things they said at each other: (who, what).
    pub sample: Vec<(String, String)>,
}

impl Together {
    pub fn nothing(&self) -> bool {
        self.replies_ab + self.replies_ba + self.mentions + self.stretches == 0 && self.sample.is_empty()
    }

    pub fn facts(&self, a: &str, b: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.shared_channels.is_empty() {
            out.push("They don't really share a channel.".to_string());
        } else {
            out.push(format!("Both turn up in: {}.", self.shared_channels.iter().map(|c| format!("#{}", c)).collect::<Vec<_>>().join(", ")));
        }
        out.push(format!("{} replied to {} {} times; {} replied to {} {} times.", a, b, self.replies_ab, b, a, self.replies_ba));
        out.push(format!("{} times one of them pinged the other.", self.mentions));
        out.push(format!("{} separate back-and-forths in the period looked at.", self.stretches));
        out.push(match self.fights {
            0 => "The kalesh detector has never called a fight with both of them in it.".to_string(),
            1 => "The kalesh detector has called one fight with both of them in it.".to_string(),
            n => format!("The kalesh detector has called {} fights with both of them in them.", n),
        });
        if !self.shared_games.is_empty() {
            out.push(format!("Both play: {}.", self.shared_games.join(", ")));
        }
        out
    }
}

// --- what may be read -------------------------------------------------------------------------

/// One message as it comes out of the log, before anything is decided about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Logged {
    pub channel: u64,
    pub parent: Option<u64>,
    pub ts_ms: i64,
    pub text: String,
}

/// The messages that may be shown to the model: never #safe-corner or a thread
/// inside it (or any other channel the owner has hidden), never a bot command
/// or a one-word reply, each once, newest first, at most `cap` of them.
///
/// This is the only door: a message that doesn't come through here never
/// reaches a prompt.
pub fn keep_messages(rows: &[Logged], sensitive: &[u64], cap: usize) -> Vec<Said> {
    let hidden: HashSet<u64> = sensitive.iter().copied().collect();
    let said: Vec<Said> = rows
        .iter()
        .filter(|r| !hidden.contains(&r.channel) && !r.parent.is_some_and(|p| hidden.contains(&p)))
        .filter_map(|r| usable(&r.text).map(|text| Said { ts: r.ts_ms / 1000, text }))
        .collect();
    let mut out = said;
    out.sort_by(|a, b| b.ts.cmp(&a.ts));
    let mut seen = HashSet::new();
    out.retain(|m| seen.insert(m.text.to_lowercase()));
    out.truncate(cap);
    out
}

/// The sample that goes in the prompt, inside the token budget, oldest first.
pub fn read_sample(messages: &[Said]) -> Vec<Said> {
    sample(messages, READ_BUDGET)
}

// --- the prompts ------------------------------------------------------------------------------

/// The server's voice, said once and shared by both prompts.
const VOICE: &str = "You are Loduchand, the house bot of MLCI, an Indian Discord server of friends who roast each other \
all day (bakchodi). Everyone there writes Hinglish - Hindi typed in Roman letters, mixed freely with English, full of \
Indian slang and short forms (\"bhai\", \"yaar\", \"scene kya hai\", \"op\", \"sahi hai\", \"bakchodi\", \"lmao\"). Read \
their messages the way a fluent member of that server would: it is not broken English and not typos. Write the same \
way they talk - mostly English sentences with the Hinglish thrown in where it lands - so it sounds like a friend in \
the channel, not a stand-up set and not a greeting card.";

/// The lines both prompts are held to.
const LIMITS: &str = "HARD LIMITS - break one and this is thrown away:
- No slur of any kind: caste, religion, region, race, sexual orientation, disability. Not as a joke, not quoting anyone.
- Nothing about their family, anyone's death, their looks or their body, their mental health, their gender or \
sexuality, their religion or their caste. These stay out even if they joke about them themselves.
- Invent nothing. Every joke lands on something written in the record below - no made-up incidents, no made-up quotes, \
no guessing at what they are like off the server.
- Punch at what they do, never at what they are.
- No emoji spam: one at most, and none is better. No @everyone or @here.";

pub fn roast_prompt(d: &Dossier) -> String {
    let thin = if d.thin() {
        "\nThere is barely anything on record for them: almost no messages, nothing in the games, no time in voice. Do \
not invent a personality to fill the gap. Make the roast about exactly that - how little there is to work with, how \
forgettable their presence is - using the few scraps below and admitting straight out that they gave you nothing.\n"
    } else {
        ""
    };
    let facts = d.facts();
    let facts = if facts.is_empty() { "(nothing on record)".to_string() } else { facts.iter().map(|f| format!("- {}", f)).collect::<Vec<_>>().join("\n") };
    let said = if d.messages.is_empty() {
        "(they have not said anything worth reading)".to_string()
    } else {
        d.messages.iter().map(|m| format!("- {}", m.text)).collect::<Vec<_>>().join("\n")
    };
    format!(
        "{voice}

Write ONE roast of {name}.

Be MEAN and be SPECIFIC. Every line must land on something in the record below: the way they type, their catchphrases, \
the games they keep losing, the hours they sit in voice, how often AutoMod eats their messages, their chess or sudoku \
record, the channel they never leave, who they are always replying to. A roast that would work on any other member is \
a failure - cut it and find something only true of them.
{thin}
{limits}

Length: five to eight short lines, under 120 words in all.

Reply with only a JSON object and nothing else: {{\"roast\": \"...\"}} - newlines inside the string are fine.

WHAT THE BOT CAN SEE ABOUT {name}
{facts}

THINGS {name} HAS ACTUALLY SAID (a sample of their own messages, oldest first):
{said}",
        voice = VOICE,
        name = d.name,
        thin = thin,
        limits = LIMITS,
        facts = facts,
        said = said,
    )
}

pub fn ship_prompt(a: &Dossier, b: &Dossier, t: &Together, percent: u8, ship_name: &str) -> String {
    let each = |d: &Dossier| {
        let facts = d.facts();
        let facts = if facts.is_empty() { "- (nothing on record)".to_string() } else { facts.iter().map(|f| format!("- {}", f)).collect::<Vec<_>>().join("\n") };
        format!("{}:\n{}", d.name, facts)
    };
    let between = t.facts(&a.name, &b.name).iter().map(|f| format!("- {}", f)).collect::<Vec<_>>().join("\n");
    let sample = if t.sample.is_empty() {
        "(nothing on record between them)".to_string()
    } else {
        t.sample.iter().map(|(who, what)| format!("- {}: {}", who, what)).collect::<Vec<_>>().join("\n")
    };
    let nothing = if t.nothing() {
        "\nThey have basically never interacted. Say so, and make the verdict about two strangers being shoved together \
by a bot - do not invent a history for them.\n"
    } else {
        ""
    };
    format!(
        "{voice}

Two members of the server have been shipped, as a joke, by a bot. The pairing is nonsense and everyone knows it: it is \
a bit, not a claim about anybody's real life. The score is already fixed at {percent}% and the ship name is already \
\"{ship}\" - do not argue with either, do not repeat the number, just write the verdict that goes under them.

Write the verdict: two or three short lines about how these two actually behave around each other, drawn only from the \
record below - who replies to whom, the channels they share, the games they both play, the fights they have had, the \
way each of them talks. Funny and sharp, the way the server talks to itself. It is a joke about their chat history and \
nothing more: no romance, no claims about anyone's real relationships, no advice.
{nothing}
{limits}

Length: under 60 words in all.

Reply with only a JSON object and nothing else: {{\"verdict\": \"...\"}}

THE TWO OF THEM
{a}

{b}

HOW THEY ARE AROUND EACH OTHER
{between}

SOME OF WHAT THEY HAVE SAID AT EACH OTHER:
{sample}",
        voice = VOICE,
        percent = percent,
        ship = ship_name,
        nothing = nothing,
        limits = LIMITS,
        a = each(a),
        b = each(b),
        between = between,
        sample = sample,
    )
}

// --- the check every reply must pass -------------------------------------------------------------

/// Identity slurs. Ordinary gaalis are not here on purpose: this server swears
/// as punctuation and a roast without it would not sound like the server. What
/// is banned is the kind aimed at who somebody is.
pub const SLURS: &str = r"n[i1]gg(?:er|ers|a|as)|negro|chink|chinki|paki|raghead|towelhead|kafir|katua|landya|chamar|bhangi|chuhra|madrasi|bihari|fag|fags|faggot|dyke|tranny|shemale|retard|retards|retarded|spastic|spaz|cripple|mongoloid|gypsy|gypsies";

/// Each area the owner ruled out, and the words that give it away. Deliberately
/// narrow: a false rejection only costs one retry, but a word that is ordinary
/// Hinglish ("bhai", "yaar") must never be in here or every roast would fail.
pub const BANNED: &[(&str, &str)] = &[
    ("a slur", SLURS),
    (
        "family",
        r"family|families|mom|moms|mum|mother|mothers|dad|dads|father|fathers|parent|parents|sister|sisters|brother|brothers|sibling|siblings|daughter|daughters|cousin|cousins|uncle|uncles|aunt|aunts|aunty|auntie|grandma|grandpa|grandmother|grandfather|granny|mummy|papa|maa|behen|didi|nani|dadi|wife|wives|husband|husbands|in[- ]laws?",
    ),
    (
        "death",
        r"death|deaths|died|dies|dying|funeral|funerals|grave|graves|graveyard|corpse|coffin|suicide|suicidal|kys|kill yourself|killing yourself|hang yourself|marja|mar ja|mar jao|rip",
    ),
    (
        "looks or body",
        r"ugly|fat|fatty|fatso|obese|skinny|bald|balding|chubby|overweight|underweight|complexion|dark[- ]skinned|acne|pimples|your looks|your face|your body|your weight|your height|your nose|your teeth|your hair|how you look",
    ),
    (
        "mental health",
        r"depressed|depression|anxiety|anxious|adhd|autistic|autism|bipolar|mental|mentally|therapy|therapist|psychiatrist|psychiatric|trauma|traumatised|traumatized|ptsd|disorder|disorders|medication|meds|psycho|schizo|schizophrenic|asylum|pagal|pagalpan|mentally ill",
    ),
    (
        "gender or sexuality",
        r"gay|lesbian|lesbians|bisexual|queer|lgbt|lgbtq|lgbtqia|transgender|non[- ]?binary|nonbinary|asexual|sexuality|sexual|sexually|sex|gender|genders|pronoun|pronouns|closeted|coming out|came out|femboy|virgin|horny",
    ),
    (
        "religion or caste",
        r"religion|religious|hindu|hindus|muslim|muslims|islam|islamic|christian|christians|sikh|sikhs|jain|jains|buddhist|atheist|atheists|god|gods|allah|bhagwan|temple|mandir|mosque|masjid|church|gurudwara|namaz|pray|prays|praying|prayer|prayers|puja|pooja|roza|ramadan|ramzan|eid|navratri|caste|castes|brahmin|dalit|rajput|hindutva",
    ),
];

static BANNED_RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    BANNED.iter().map(|(area, words)| (*area, Regex::new(&format!(r"(?i)\b(?:{})\b", words)).expect("banned words"))).collect()
});

static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+").expect("regex"));
static BLANK_LINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").expect("regex"));
static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)^\s*```(?:json)?\s*(.*?)\s*```\s*$").expect("regex"));
/// The model declining rather than writing anything. Never posted as a roast.
static REFUSAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:i'?m sorry|i am sorry|sorry[,.]? (?:but )?i\b|i can'?t\b|i cannot\b|i won'?t\b|i'?m not able|i am not able|as an ai|unfortunately,? i\b)")
        .expect("regex")
});

/// Why a piece of writing may not be posted, or `None` when it may.
pub fn rejection(text: &str) -> Option<&'static str> {
    if text.contains("@everyone") || text.contains("@here") {
        return Some("a mass ping");
    }
    BANNED_RES.iter().find(|(_, re)| re.is_match(text)).map(|(area, _)| *area)
}

fn tidy(raw: &str) -> String {
    let inner = FENCE.captures(raw).map(|c| c[1].to_string()).unwrap_or_else(|| raw.to_string());
    let lines: Vec<String> = inner.lines().map(|l| SPACES.replace_all(l.trim(), " ").to_string()).collect();
    BLANK_LINES.replace_all(lines.join("\n").trim(), "\n\n").to_string()
}

/// The named field of the JSON object in the reply, or - when the model ignored
/// the JSON and just wrote the thing - the whole reply.
fn field(raw: &str, key: &str) -> Option<String> {
    let text = tidy(raw);
    match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if b > a => {
            let v: Value = serde_json::from_str(&text[a..=b]).ok()?;
            v.get(key).and_then(Value::as_str).map(|s| tidy(s))
        }
        // No JSON at all: the model wrote the roast straight out. Take it.
        (None, None) => Some(text),
        _ => None,
    }
}

/// Reads a reply and holds it to the rules. `Ok` is what may be posted; `Err`
/// says why not, in words the log can carry.
pub fn check(raw: &str, key: &str, max_chars: usize) -> Result<String, &'static str> {
    let Some(text) = field(raw, key) else { return Err("the reply couldn't be read") };
    if REFUSAL.is_match(&text) {
        return Err("the model refused");
    }
    if text.chars().filter(|c| c.is_alphabetic()).count() < 20 {
        return Err("it came back empty");
    }
    if text.chars().count() > max_chars {
        return Err("it was too long to post");
    }
    match rejection(&text) {
        Some(why) => Err(why),
        None => Ok(text),
    }
}

pub fn check_roast(raw: &str) -> Result<String, &'static str> {
    check(raw, "roast", ROAST_CHARS)
}

pub fn check_ship(raw: &str) -> Result<String, &'static str> {
    check(raw, "verdict", SHIP_CHARS)
}

// --- asking, and asking once more -----------------------------------------------------------------

/// What came of asking the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Made {
    Ok { text: String, tries: usize },
    /// Both tries came back with something that broke a rule.
    Refused { why: &'static str },
    /// The model itself failed both times.
    Failed { err: String },
}

/// What is added to the prompt on the second try, so the model knows what went wrong.
pub fn nudge(why: &str) -> String {
    format!(
        "\n\nYour last answer was thrown away: {}. Try again, same brief, and keep well inside the limits this time. \
         Reply with the JSON object only.",
        why
    )
}

/// Asks the model, checks what comes back, and on a refusal or a failure asks
/// once more. Never returns something that hasn't passed [`check`].
pub async fn make<F, Fut>(prompt: String, check: impl Fn(&str) -> Result<String, &'static str>, ask: F) -> Made
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = anyhow::Result<String>>,
{
    let mut last = Made::Failed { err: "the model wasn't reachable".to_string() };
    for attempt in 1..=TRIES {
        // A second try is told what was wrong with the first.
        let asked = match &last {
            Made::Refused { why } => format!("{}{}", prompt, nudge(why)),
            _ => prompt.clone(),
        };
        last = match ask(asked).await {
            Ok(raw) => match check(&raw) {
                Ok(text) => return Made::Ok { text, tries: attempt },
                Err(why) => {
                    tracing::info!("roast: a model answer was thrown away ({})", why);
                    Made::Refused { why }
                }
            },
            Err(err) => {
                tracing::warn!("roast: the model failed (try {}): {}", attempt, err);
                Made::Failed { err: err.to_string() }
            }
        };
    }
    last
}

/// What the person is told when nothing usable came back. Never a raw error.
pub const COULDNT: &str = "Couldn't come up with anything clean for that one. Try again in a bit.";
pub const MODEL_DOWN: &str = "The AI isn't answering right now, so no roast. Try again in a few minutes.";

pub fn failure_message(made: &Made) -> &'static str {
    match made {
        Made::Failed { .. } => MODEL_DOWN,
        _ => COULDNT,
    }
}

// --- opting out ----------------------------------------------------------------------------------

/// Who, if anyone, has put themselves out of reach - the target first, because
/// that is the one that stops the whole thing.
pub fn blocked(optouts: &[u64], people: &[u64]) -> Option<u64> {
    people.iter().copied().find(|p| optouts.contains(p))
}

/// What the person who ran the command is told. Kind, and it names nobody else's
/// business beyond the fact that they are out.
pub fn opted_out_message(who: u64, caller: u64, name: &str) -> String {
    if who == caller {
        "You've opted out of roasts and ships, so I'm not going to do one. Run `/noroast` again to come back in."
            .to_string()
    } else {
        format!("**{}** has opted out of roasts and ships, so that one's off the table. Nothing was posted.", name)
    }
}

// --- the ship ---------------------------------------------------------------------------------------

/// The same score for the same two people, every time, whichever way round they
/// are named: a hash of the pair and nothing else. It is a joke, so it must at
/// least be a joke that doesn't change its mind.
pub fn ship_percent(a: u64, b: u64) -> u8 {
    let (lo, hi) = (a.min(b), a.max(b));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lo.to_le_bytes().iter().chain(hi.to_le_bytes().iter()) {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % 101) as u8
}

fn letters(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The front of one name on the back of the other: "gooner" + "potus" = "Gootus".
/// Falls back to whatever there is when a name is all emoji.
pub fn ship_name(a: &str, b: &str) -> String {
    let (a, b) = (letters(a), letters(b));
    if a.is_empty() || b.is_empty() {
        let joined = format!("{}{}", a, b);
        return if joined.is_empty() { "Ship".to_string() } else { capitalise(&joined) };
    }
    let head: String = a.chars().take(a.chars().count().div_ceil(2).max(2)).collect();
    let keep = b.chars().count().div_ceil(2).max(2).min(b.chars().count());
    let tail: String = b.chars().skip(b.chars().count() - keep).collect();
    capitalise(&cut(&format!("{}{}", head, tail), 28))
}

fn capitalise(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// The percentage drawn as a bar, ten steps wide.
pub fn bar(percent: u8) -> String {
    let filled = (percent as usize * 10).div_ceil(100).min(10);
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

// --- where it is posted ------------------------------------------------------------------------------

/// Both commands only ever post in the roast channel. When one was run
/// somewhere else, this is what that other place is told.
pub fn posted_elsewhere(roast_channel: u64, link: &str) -> String {
    format!("Posted in <#{}> → {}", roast_channel, link)
}

pub fn message_link(guild: u64, channel: u64, message: u64) -> String {
    format!("https://discord.com/channels/{}/{}/{}", guild, channel, message)
}

/// Whether the place the command was run in needs telling: only when it isn't
/// the roast channel itself.
pub fn needs_redirect_note(used_in: u64, roast_channel: u64) -> bool {
    used_in != roast_channel
}

/// The line above the card in the roast channel: both of them pinged, so they
/// see it.
pub fn ping_line(caller: u64, targets: &[u64]) -> String {
    let mut ids: Vec<u64> = Vec::new();
    for id in std::iter::once(caller).chain(targets.iter().copied()) {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids.iter().map(|id| format!("<@{}>", id)).collect::<Vec<_>>().join(" ")
}

/// A rough cost of one prompt, for the log.
pub fn prompt_tokens(prompt: &str) -> usize {
    estimate_tokens(prompt)
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const ARJUN: u64 = 771;
    const RIYA: u64 = 982;
    const SAFE: u64 = 1_516_000_000_000_000_001;
    const T0: i64 = 1_780_000_000_000;

    /// A member with a real history: the test data the samples in the report come from.
    pub fn arjun() -> Dossier {
        Dossier {
            id: ARJUN,
            name: "arjun".into(),
            days_here: Some(412),
            messages_all: 18_420,
            messages_month: 1_930,
            top_channels: vec!["chatting-hori".into(), "chess".into(), "games".into()],
            games: vec![
                ("Chess".into(), "61 games, 12 won, 4 drawn".into()),
                ("Sudoku".into(), "88 solved, 3 first".into()),
                ("Anagrams".into(), "9 solved, fastest in 41s".into()),
            ],
            house: Some("Rankhoj".into()),
            points_month: 140,
            house_place: Some((9, 11)),
            frog_cards: 212,
            voice_month_mins: 3_180,
            automod_blocked: 37,
            deleted: 96,
            emoji: Some("💀".into()),
            phrases: vec!["ek minute".into(), "bhai sun".into(), "gg wp".into()],
            partners: vec!["riya".into(), "dev".into()],
            messages: vec![
                Said { ts: T0 / 1000, text: "ek minute bhai rematch dedo abhi".into() },
                Said { ts: T0 / 1000 + 90, text: "gg wp but you got lucky with that knight".into() },
                Said { ts: T0 / 1000 + 400, text: "vc me aa jao sab, akela baitha hu".into() },
            ],
        }
    }

    pub fn riya() -> Dossier {
        Dossier {
            id: RIYA,
            name: "riya".into(),
            days_here: Some(300),
            messages_all: 9_100,
            messages_month: 2_400,
            top_channels: vec!["chatting-hori".into(), "chess".into()],
            games: vec![("Chess".into(), "58 games, 41 won, 3 drawn".into())],
            house: Some("Rankhoj".into()),
            points_month: 890,
            house_place: Some((1, 11)),
            frog_cards: 4,
            voice_month_mins: 120,
            automod_blocked: 0,
            deleted: 3,
            emoji: Some("😭".into()),
            phrases: vec!["nahi bhai".into(), "cope".into()],
            partners: vec!["arjun".into()],
            messages: vec![
                Said { ts: T0 / 1000 + 10, text: "cope harder, the knight was always going there".into() },
                Said { ts: T0 / 1000 + 200, text: "nahi bhai vc me nahi aungi, padhna hai".into() },
            ],
        }
    }

    pub fn together() -> Together {
        Together {
            shared_channels: vec!["chatting-hori".into(), "chess".into()],
            replies_ab: 214,
            replies_ba: 198,
            mentions: 61,
            stretches: 22,
            fights: 3,
            shared_games: vec!["Chess".into()],
            sample: vec![
                ("arjun".into(), "ek minute rematch dedo".into()),
                ("riya".into(), "cope harder".into()),
                ("arjun".into(), "bhai sun ye galat tha".into()),
            ],
        }
    }

    fn nobody() -> Dossier {
        Dossier { id: 55, name: "ghost".into(), days_here: Some(390), messages_all: 11, messages_month: 0, ..Default::default() }
    }

    fn fake(answers: Vec<anyhow::Result<&'static str>>, calls: Arc<AtomicUsize>) -> impl Fn(String) -> std::future::Ready<anyhow::Result<String>> {
        let answers = Arc::new(parking_lot::Mutex::new(answers.into_iter().collect::<Vec<_>>()));
        move |_prompt: String| {
            let i = calls.fetch_add(1, Ordering::SeqCst);
            let mut list = answers.lock();
            let out = if i < list.len() {
                match std::mem::replace(&mut list[i], Ok("")) {
                    Ok(s) => Ok(s.to_string()),
                    Err(e) => Err(e),
                }
            } else {
                Ok(String::new())
            };
            std::future::ready(out)
        }
    }

    // --- where it is posted ---------------------------------------------------

    #[test]
    fn a_command_run_anywhere_else_still_posts_in_the_roast_channel_and_links_back() {
        let roast_channel = 1_516_534_303_968_858_312u64;
        assert!(needs_redirect_note(99, roast_channel), "run in #general: the channel is told");
        assert!(!needs_redirect_note(roast_channel, roast_channel), "run in the roast channel itself: nothing to say");
        let link = message_link(900, roast_channel, 7_001);
        assert_eq!(link, format!("https://discord.com/channels/900/{}/7001", roast_channel));
        let note = posted_elsewhere(roast_channel, &link);
        assert_eq!(note, format!("Posted in <#{}> → {}", roast_channel, link));
        // Both of them are pinged where it lands, and nobody twice.
        assert_eq!(ping_line(ARJUN, &[RIYA]), format!("<@{}> <@{}>", ARJUN, RIYA));
        assert_eq!(ping_line(ARJUN, &[ARJUN]), format!("<@{}>", ARJUN), "roasting yourself pings you once");
    }

    // --- opting out -----------------------------------------------------------

    #[test]
    fn someone_who_opted_out_is_never_roasted_or_shipped() {
        let optouts = vec![RIYA];
        assert_eq!(blocked(&optouts, &[ARJUN]), None);
        assert_eq!(blocked(&optouts, &[RIYA]), Some(RIYA));
        assert_eq!(blocked(&optouts, &[ARJUN, RIYA]), Some(RIYA), "either half of a ship stops it");
        assert!(blocked(&[], &[ARJUN, RIYA]).is_none());
        let theirs = opted_out_message(RIYA, ARJUN, "riya");
        assert!(theirs.contains("riya") && theirs.contains("opted out") && theirs.contains("Nothing was posted"), "{theirs}");
        let mine = opted_out_message(ARJUN, ARJUN, "arjun");
        assert!(mine.contains("You've opted out") && mine.contains("/noroast"), "{mine}");
    }

    // --- the check ------------------------------------------------------------

    #[test]
    fn the_check_lets_a_mean_roast_through_and_stops_a_cruel_one() {
        let fine = r#"{"roast": "61 chess games and 12 wins, bhai that's not a record that's a cry for help. 53 hours in vc this month and still nobody remembers what your voice sounds like. AutoMod has eaten 37 of your messages, which is 37 more thoughts than you've had. You say ek minute like it's a personality. Riya has beaten you so many times the chess bot should just auto-resign for you."}"#;
        let out = check_roast(fine).expect("a mean, specific roast is fine");
        assert!(out.contains("ek minute"));
        for (why, bad) in [
            ("a slur", r#"{"roast": "you absolute retard, 12 wins in 61 games"}"#),
            ("family", r#"{"roast": "even your mother mutes you in vc, 12 wins in 61 games bhai"}"#),
            ("death", r#"{"roast": "your chess rating died a slow death, honestly go dig a grave for it bhai"}"#),
            ("looks or body", r#"{"roast": "your face is the reason the vc has cameras off, 12 wins in 61 games"}"#),
            ("mental health", r#"{"roast": "212 frog cards is not a hobby it is a disorder, get therapy bhai"}"#),
            ("gender or sexuality", r#"{"roast": "61 chess games and you still play like a virgin, sorry bhai"}"#),
            ("religion or caste", r#"{"roast": "you pray to the chess bot every night and it still says no, 12 wins bhai"}"#),
            ("a mass ping", r#"{"roast": "@everyone come look at this man's 12 wins in 61 chess games, genuinely tragic"}"#),
        ] {
            assert_eq!(check_roast(bad), Err(why), "{bad}");
        }
        // Too long for the embed, and too short to be anything.
        let long = format!(r#"{{"roast": "{}"}}"#, "bhai ye bohot lamba hai ".repeat(100));
        assert_eq!(check_roast(&long), Err("it was too long to post"));
        assert_eq!(check_roast(r#"{"roast": "lol"}"#), Err("it came back empty"));
        // The model declining is never posted as if it were the joke.
        assert_eq!(check_roast("I'm sorry, I can't help with that."), Err("the model refused"));
        assert_eq!(check_roast(r#"{"roast": "I cannot write a roast about this member without more information to go on."}"#), Err("the model refused"));
        // A fenced answer, and a wrong-shaped one.
        assert!(check_roast("```json\n{\"roast\": \"61 chess games, 12 wins. ek minute, ek minute, and still no rematch won bhai.\"}\n```").is_ok());
        assert_eq!(check_roast(r#"{"something_else": "..."}"#), Err("the reply couldn't be read"));
        // The ship verdict is held to the same lines, on a shorter leash.
        assert!(check_ship(r#"{"verdict": "412 replies between them and riya has won 41 of 58 chess games. This is not a ship, it is a hostage situation with extra steps."}"#).is_ok());
        assert_eq!(check_ship(r#"{"verdict": "they fight like brother and sister honestly, 412 replies between them bhai"}"#), Err("family"));
    }

    #[tokio::test]
    async fn a_refused_answer_is_asked_again_once_and_then_given_up_on() {
        let bad = r#"{"roast": "your mother has 12 wins in 61 chess games and she doesn't even play, bhai"}"#;
        let good = r#"{"roast": "12 wins in 61 chess games. You say ek minute like it is a magic spell and it has never once worked. 53 hours of vc to say gg wp and nothing else."}"#;
        // Refused, then fine: two calls, and the good one is what comes out.
        let calls = Arc::new(AtomicUsize::new(0));
        let made = make("p".into(), check_roast, fake(vec![Ok(bad), Ok(good)], calls.clone())).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(&made, Made::Ok { tries: 2, text } if text.contains("ek minute")), "{made:?}");
        // Refused twice: given up on, and the person is told plainly.
        let calls = Arc::new(AtomicUsize::new(0));
        let made = make("p".into(), check_roast, fake(vec![Ok(bad), Ok(bad)], calls.clone())).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(made, Made::Refused { why: "family" });
        assert_eq!(failure_message(&made), COULDNT);
        // The nudge tells the second try what went wrong.
        assert!(nudge("family").contains("thrown away: family"));
    }

    #[tokio::test]
    async fn a_model_that_fails_gives_a_plain_message_and_never_a_raw_error() {
        let calls = Arc::new(AtomicUsize::new(0));
        let made = make(
            "p".into(),
            check_roast,
            fake(vec![Err(anyhow::anyhow!("error sending request for url (https://api.example/v1/chat): connection closed")), Err(anyhow::anyhow!("timed out"))], calls.clone()),
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 2, "a failed call is worth one retry");
        assert!(matches!(made, Made::Failed { .. }));
        let shown = failure_message(&made);
        assert_eq!(shown, MODEL_DOWN);
        assert!(!shown.contains("http") && !shown.contains("connection closed"), "the raw error is never shown: {shown}");
        // It fails, then works: one retry is enough.
        let calls = Arc::new(AtomicUsize::new(0));
        let good = r#"{"roast": "12 wins in 61 chess games, and you still ask for a rematch like it will help bhai."}"#;
        let made = make("p".into(), check_roast, fake(vec![Err(anyhow::anyhow!("dropped")), Ok(good)], calls.clone())).await;
        assert!(matches!(made, Made::Ok { tries: 2, .. }), "{made:?}");
    }

    // --- what reaches the model -----------------------------------------------

    #[test]
    fn safe_corner_never_reaches_the_model() {
        let rows = vec![
            Logged { channel: 21, parent: None, ts_ms: T0, text: "bhai ye chess game to gaya".into() },
            Logged { channel: SAFE, parent: None, ts_ms: T0 + 1, text: "mujhe kuch batana hai sabko, bohot mushkil hai".into() },
            Logged { channel: 77, parent: Some(SAFE), ts_ms: T0 + 2, text: "thread me bhi wahi baat likhi thi maine".into() },
            Logged { channel: 21, parent: None, ts_ms: T0 + 3, text: "ek minute rematch dedo abhi".into() },
            Logged { channel: 21, parent: None, ts_ms: T0 + 4, text: "!play despacito".into() },
            Logged { channel: 21, parent: None, ts_ms: T0 + 5, text: "lol".into() },
        ];
        let kept = keep_messages(&rows, &[SAFE], 100);
        let texts: Vec<&str> = kept.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts.contains(&"ek minute rematch dedo abhi") && texts.contains(&"bhai ye chess game to gaya"));
        assert!(!texts.iter().any(|t| t.contains("batana")), "the #safe-corner message is gone");
        assert!(!texts.iter().any(|t| t.contains("thread me")), "a thread inside #safe-corner is gone too");
        // And nothing from there can get into a prompt.
        let d = Dossier { messages: kept.clone(), ..arjun() };
        let p = roast_prompt(&d);
        assert!(!p.contains("batana") && !p.contains("thread me"), "{p}");
        // The cap is a hard cap.
        let many: Vec<Logged> = (0..500).map(|i| Logged { channel: 21, parent: None, ts_ms: T0 + i, text: format!("message number {} about the chess game", i) }).collect();
        assert_eq!(keep_messages(&many, &[], 120).len(), 120);
        // And the sample that goes in the prompt stays inside the token budget.
        let picked = read_sample(&keep_messages(&many, &[], 5_000));
        let spent: usize = picked.iter().map(|m| estimate_tokens(&m.text) + 2).sum();
        assert!(spent <= READ_BUDGET, "over budget: {spent}");
        assert!(picked.windows(2).all(|w| w[0].ts <= w[1].ts), "oldest first");
    }

    #[test]
    fn the_roast_prompt_carries_the_record_and_the_limits() {
        let p = roast_prompt(&arjun());
        for must in [
            "Hinglish",
            "Be MEAN and be SPECIFIC",
            "No slur of any kind",
            "their family",
            "anyone's death",
            "their looks or their body",
            "their mental health",
            "their gender or sexuality",
            "their religion or their caste",
            "even if they joke about them themselves",
            "Invent nothing",
            "No emoji spam",
            "Chess: 61 games, 12 won, 4 drawn",
            "AutoMod has eaten 37 of their messages",
            "53 hours in voice chat",
            "ek minute",
            "9th of 11 scorers",
            "ek minute bhai rematch dedo abhi",
        ] {
            assert!(p.contains(must), "the prompt lacks {must:?}");
        }
        assert!(!p.contains("barely anything on record"), "arjun is not a thin case");
    }

    #[test]
    fn a_member_with_almost_no_history_still_gets_a_roast_about_being_boring() {
        let ghost = nobody();
        assert!(ghost.thin());
        assert!(!arjun().thin());
        let p = roast_prompt(&ghost);
        assert!(p.contains("barely anything on record"), "{p}");
        assert!(p.contains("Do not invent a personality to fill the gap"));
        assert!(p.contains("admitting straight out that they gave you nothing"));
        assert!(p.contains("(they have not said anything worth reading)"));
        // 11 messages in 390 days is still a fact the roast may use.
        assert!(p.contains("11 messages on record all time"));
        // And a clean answer about exactly that passes the check.
        let answer = r#"{"roast": "11 messages in 390 days. Not a joke, not a bit, just eleven. You have been here longer than most of the chess games and the bot still had to check twice that you exist. There is nothing to roast here, which is somehow the roast."}"#;
        assert!(check_roast(answer).is_ok());
    }

    // --- the ship ---------------------------------------------------------------

    #[test]
    fn a_pair_always_scores_the_same_whichever_way_round_it_is_asked() {
        for (a, b) in [(ARJUN, RIYA), (1, 2), (999_999_999_999_999_999, 4), (7, 7)] {
            assert_eq!(ship_percent(a, b), ship_percent(b, a), "{a} and {b}");
        }
        // The same every time it is asked, not a fresh roll.
        let first = ship_percent(ARJUN, RIYA);
        for _ in 0..50 {
            assert_eq!(ship_percent(RIYA, ARJUN), first);
        }
        assert!((0..=100).contains(&(first as i32)));
        // Different pairs don't all land on the same number.
        let spread: HashSet<u8> = (1..40u64).map(|i| ship_percent(i, i * 7 + 3)).collect();
        assert!(spread.len() > 15, "only {} distinct scores in 39 pairs", spread.len());
        assert_eq!(bar(0), "░░░░░░░░░░");
        assert_eq!(bar(100), "██████████");
        assert_eq!(bar(41).chars().filter(|c| *c == '█').count(), 5);
    }

    #[test]
    fn the_ship_name_is_the_front_of_one_and_the_back_of_the_other() {
        assert_eq!(ship_name("gooner", "potus"), "Gootus");
        assert_eq!(ship_name("arjun", "riya"), "Arjya");
        assert_eq!(ship_name("dev", "kavya"), "Devya");
        assert_eq!(ship_name("a", "b"), "Ab");
        assert_eq!(ship_name("🐸🐸", "riya"), "Riya", "a name that is all emoji still makes something");
        assert_eq!(ship_name("🐸", "🐸"), "Ship");
        assert!(ship_name(&"x".repeat(80), &"y".repeat(80)).chars().count() <= 29);
    }

    #[test]
    fn the_ship_prompt_says_it_is_a_joke_and_carries_how_they_behave() {
        let (a, b) = (arjun(), riya());
        let t = together();
        let pct = ship_percent(a.id, b.id);
        let name = ship_name(&a.name, &b.name);
        let p = ship_prompt(&a, &b, &t, pct, &name);
        for must in [
            "it is a bit, not a claim about anybody's real life",
            "no romance, no claims about anyone's real relationships",
            "No slur of any kind",
            "arjun replied to riya 214 times; riya replied to arjun 198 times",
            "The kalesh detector has called 3 fights with both of them in them",
            "Both turn up in: #chatting-hori, #chess",
            "Both play: Chess",
            "Chess: 58 games, 41 won, 3 drawn",
            "arjun: ek minute rematch dedo",
        ] {
            assert!(p.contains(must), "the ship prompt lacks {must:?}");
        }
        assert!(p.contains(&format!("{}%", pct)) && p.contains(&name));
        assert!(!p.contains("basically never interacted"));
        // Two strangers: the prompt says so instead of letting the model invent one.
        let strangers = Together { shared_channels: vec![], ..Default::default() };
        assert!(strangers.nothing());
        let p = ship_prompt(&a, &b, &strangers, 4, "Arjiya");
        assert!(p.contains("basically never interacted") && p.contains("do not invent a history for them"), "{p}");
        assert!(p.contains("They don't really share a channel."));
    }

    #[test]
    fn a_dossier_only_says_what_it_knows() {
        let facts = nobody().facts();
        assert_eq!(facts, vec!["On the server for about 390 days.".to_string(), "11 messages on record all time, 0 in the last month.".to_string()]);
        assert_eq!(hours(3_180), "53 hours");
        assert_eq!(hours(45), "45 min");
        assert_eq!(hours(61), "1 hour 1 min");
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }
}
