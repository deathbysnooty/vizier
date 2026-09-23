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
    /// Minutes in the same voice room at the same time, over the period.
    pub vc_minutes: i64,
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
        out.push(match self.vc_minutes {
            0 => "They have not once been in a voice room together.".to_string(),
            m => format!("{} in voice together.", hours(m)),
        });
        if !self.shared_games.is_empty() {
            out.push(format!("Both play: {}.", self.shared_games.join(", ")));
        }
        out
    }

    /// One of them doing nearly all the replying: the quiet one has, in effect,
    /// stopped answering. Only once there is enough traffic to mean anything.
    pub fn one_sided(&self) -> bool {
        let (lo, hi) = (self.replies_ab.min(self.replies_ba), self.replies_ab.max(self.replies_ba));
        hi >= 20 && lo * 5 < hi
    }

    /// What this pair's number is nudged by.
    pub fn signals(&self) -> Signals {
        Signals {
            replies: self.replies_ab + self.replies_ba,
            vc_minutes: self.vc_minutes,
            shared_games: self.shared_games.len(),
            fights: self.fights,
            one_sided: self.one_sided(),
        }
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

The number is their own, worked out from their member ids, moved {drift} by what they have actually done:
{moved}
Your verdict has to agree with that. If the number is low because they fight, the verdict knows it; if it is high \
because they never stop replying, the verdict knows that instead.

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
        drift = match drift(&t.signals()) {
            0 => "not at all".to_string(),
            d if d > 0 => format!("up {} points", d),
            d => format!("down {} points", -d),
        },
        moved = {
            let steps = steps(&t.signals());
            if steps.is_empty() {
                "- nothing has happened between them to move it either way".to_string()
            } else {
                steps.iter().map(|(what, by)| format!("- {} ({}{})", what, if *by > 0 { "+" } else { "" }, by)).collect::<Vec<_>>().join("\n")
            }
        },
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

/// What a pair's number is nudged by. Every one of these is symmetric, so the
/// score comes out the same whichever way round the pair was named.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// Replies between them, both ways, over the period looked at.
    pub replies: usize,
    /// Minutes in the same voice room at the same time.
    pub vc_minutes: i64,
    /// Games they both play.
    pub shared_games: usize,
    /// Fights the kalesh detector called with both of them in it.
    pub fights: usize,
    /// One of them doing nearly all the replying.
    pub one_sided: bool,
}

/// The furthest the things that happened may carry a pair from their own base,
/// either way. Past this the pair's own number would stop showing through.
pub const MAX_DRIFT: i32 = 25;
/// Never a suspiciously round nothing or everything.
pub const MIN_PERCENT: u8 = 1;
pub const MAX_PERCENT: u8 = 99;

/// In plain English, for the help text and the report: what moves a pair's
/// number, so nobody thinks it is a random roll.
pub const WHAT_MOVES_IT: &str = "Each pair has a number of their own that never changes, worked out from their two member ids - nobody can reroll it. What the two of them actually do then nudges it, in a few chunky steps and never by more than 25 either way: replying to each other, sitting in voice together, playing the same games and being on the same side push it up; fights the kalesh detector called, and one of them doing nearly all the replying, pull it down. It only moves when one of those crosses a step, so it drifts slowly and only when something real changed.";

/// Which rung of a ladder a count has reached. Saturating, so an absurd count
/// lands on the top rung rather than wrapping round to nothing.
fn step(value: usize, ladder: &[(i64, i32)]) -> i32 {
    let value = i64::try_from(value).unwrap_or(i64::MAX);
    ladder.iter().rev().find(|(at, _)| value >= *at).map(|(_, by)| *by).unwrap_or(0)
}

/// Every step this pair has earned, with what it is called. Coarse on purpose:
/// a handful of steps of a few points, not a sliding score, so the number holds
/// still until something real changes.
pub fn steps(s: &Signals) -> Vec<(&'static str, i32)> {
    let mut out: Vec<(&'static str, i32)> = Vec::new();
    let replies = step(s.replies, &[(1, 2), (25, 5), (100, 9), (400, 13)]);
    if replies != 0 {
        out.push(("they reply to each other", replies));
    }
    let vc = step(s.vc_minutes.max(0) as usize, &[(1, 2), (60, 5), (300, 8)]);
    if vc != 0 {
        out.push(("time in voice together", vc));
    }
    let games = step(s.shared_games, &[(1, 2), (2, 4)]);
    if games != 0 {
        out.push(("games they both play", games));
    }
    let fights = step(s.fights, &[(1, -4), (2, -7), (4, -10)]);
    if fights != 0 {
        out.push(("kalesh between them", fights));
    }
    if s.one_sided {
        out.push(("one of them has gone quiet on the other", -6));
    }
    out
}

/// How far the things that happened carry them from their base, capped.
pub fn drift(s: &Signals) -> i32 {
    steps(s).iter().map(|(_, by)| by).sum::<i32>().clamp(-MAX_DRIFT, MAX_DRIFT)
}

/// The pair's own number, from their two ids and nothing else: the same every
/// time, whichever way round they are named, and nobody can reroll it.
pub fn ship_base(a: u64, b: u64) -> i32 {
    let (lo, hi) = (a.min(b), a.max(b));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lo.to_le_bytes().iter().chain(hi.to_le_bytes().iter()) {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % 101) as i32
}

/// Their base, nudged by what the two of them have actually done. Stable in
/// both orders, because every signal is.
pub fn ship_percent(a: u64, b: u64, s: &Signals) -> u8 {
    (ship_base(a, b) + drift(s)).clamp(MIN_PERCENT as i32, MAX_PERCENT as i32) as u8
}

/// "up 6 since 5 days ago", or nothing at all when it hasn't moved. Silence is
/// the point: a line that appears every time would stop meaning anything.
pub fn movement(now: u8, last: Option<(u8, i64)>, now_ts: i64) -> Option<String> {
    let (was, at) = last?;
    let by = now as i32 - was as i32;
    if by == 0 {
        return None;
    }
    let ago = super::kalesh::later((now_ts - at).max(0) * 1000);
    Some(format!("{} {} since {} ago", if by > 0 { "up" } else { "down" }, by.abs(), ago))
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

/// The longest the card's one-line fact may be.
pub const HEADLINE_CHARS: usize = 58;

/// One real number about the pair, for the line under the bar on the card. It
/// comes from what the bot counted, never from the model, so it cannot be
/// wrong and cannot be unsafe. The most interesting thing available wins: a
/// fight beats a reply count, a reply count beats a ping count, and a pair who
/// have never said a word to each other get a line that says exactly that.
pub fn headline(t: &Together, days: i64) -> String {
    let replies = t.replies_ab + t.replies_ba;
    let line = if t.fights > 0 {
        format!("{} fight{} called · 0 apologies logged", t.fights, if t.fights == 1 { "" } else { "s" })
    } else if replies > 0 {
        format!("{} repl{} in {} days", replies, if replies == 1 { "y" } else { "ies" }, days)
    } else if t.mentions > 0 {
        format!("{} ping{} at each other, 0 replies", t.mentions, if t.mentions == 1 { "" } else { "s" })
    } else if t.stretches > 0 {
        format!("{} run-in{} in {} days, not one reply", t.stretches, if t.stretches == 1 { "" } else { "s" }, days)
    } else if let Some(channel) = t.shared_channels.first() {
        format!("Both live in #{} and have never once replied", cut(channel, 24))
    } else {
        format!("Not one word between them in {} days", days)
    };
    cut(&line, HEADLINE_CHARS)
}

/// The score as text, for the message when the card couldn't be drawn. With a
/// card there is no need: the picture says it better.
pub fn score_line(a: &str, b: &str, percent: u8, with_card: bool) -> String {
    if with_card {
        format!("**{}** × **{}**", a, b)
    } else {
        format!("**{}** × **{}**\n\n`{}` **{}%**", a, b, bar(percent), percent)
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

/// The line above the card in the roast channel: everyone it is about, pinged
/// so they see it, each once and in the order given. Who ran it is named in the
/// footer instead - they know, they asked.
pub fn ping_line(who: &[u64]) -> String {
    let mut ids: Vec<u64> = Vec::new();
    for id in who {
        if !ids.contains(id) {
            ids.push(*id);
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
            vc_minutes: 640,
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
        assert_eq!(ping_line(&[ARJUN, RIYA]), format!("<@{}> <@{}>", ARJUN, RIYA));
        assert_eq!(ping_line(&[ARJUN, ARJUN]), format!("<@{}>", ARJUN), "nobody is pinged twice");
        assert_eq!(ping_line(&[]), "");
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
        let s = together().signals();
        for (a, b) in [(ARJUN, RIYA), (1, 2), (999_999_999_999_999_999, 4), (7, 7)] {
            assert_eq!(ship_percent(a, b, &s), ship_percent(b, a, &s), "{a} and {b}");
            assert_eq!(ship_base(a, b), ship_base(b, a));
        }
        // The same every time it is asked, not a fresh roll.
        let first = ship_percent(ARJUN, RIYA, &s);
        for _ in 0..50 {
            assert_eq!(ship_percent(RIYA, ARJUN, &s), first);
        }
        // Never a suspiciously round nothing or everything, whatever happens.
        for base_pair in [(1u64, 2u64), (ARJUN, RIYA), (5, 900), (12, 13)] {
            for signals in [
                Signals::default(),
                Signals { replies: 100_000, vc_minutes: 100_000, shared_games: 9, ..Default::default() },
                Signals { fights: 500, one_sided: true, ..Default::default() },
            ] {
                let p = ship_percent(base_pair.0, base_pair.1, &signals);
                assert!((MIN_PERCENT..=MAX_PERCENT).contains(&p), "{p}% from {base_pair:?}");
            }
        }
        // Different pairs don't all land on the same number.
        let spread: HashSet<u8> = (1..40u64).map(|i| ship_percent(i, i * 7 + 3, &s)).collect();
        assert!(spread.len() > 15, "only {} distinct scores in 39 pairs", spread.len());
        assert_eq!(bar(0), "░░░░░░░░░░");
        assert_eq!(bar(100), "██████████");
        assert_eq!(bar(41).chars().filter(|c| *c == '█').count(), 5);
    }

    #[test]
    fn what_they_do_moves_the_number_in_chunky_steps_and_only_so_far() {
        // Nothing has happened: the pair's own number, untouched.
        let (a, b) = (ARJUN, RIYA);
        let base = ship_base(a, b);
        let nothing = Signals::default();
        assert_eq!(drift(&nothing), 0);
        assert!(steps(&nothing).is_empty());
        assert_eq!(ship_percent(a, b, &nothing) as i32, base.clamp(1, 99));
        // Replies push it up; a fight pulls it down.
        let chatty = Signals { replies: 412, ..Default::default() };
        let fighty = Signals { fights: 3, ..Default::default() };
        assert!(drift(&chatty) > 0, "lots of replies raise it: {:?}", steps(&chatty));
        assert!(drift(&fighty) < 0, "a fight lowers it: {:?}", steps(&fighty));
        assert!(ship_percent(a, b, &chatty) > ship_percent(a, b, &nothing));
        assert!(ship_percent(a, b, &fighty) < ship_percent(a, b, &nothing));
        // Coarse on purpose: within a step nothing moves at all.
        assert_eq!(drift(&Signals { replies: 100, ..Default::default() }), drift(&Signals { replies: 399, ..Default::default() }));
        assert!(drift(&Signals { replies: 400, ..Default::default() }) > drift(&Signals { replies: 399, ..Default::default() }));
        assert_eq!(drift(&Signals { vc_minutes: 60, ..Default::default() }), drift(&Signals { vc_minutes: 299, ..Default::default() }));
        // Every step is a few points, never a landslide.
        for (_, by) in steps(&together().signals()) {
            assert!((1..=13).contains(&by.abs()), "a step of {by} is not a nudge");
        }
        // The movement is capped, however much has happened either way.
        let everything = Signals { replies: usize::MAX, vc_minutes: i64::MAX, shared_games: 50, ..Default::default() };
        let worst = Signals { fights: 10_000, one_sided: true, ..Default::default() };
        assert_eq!(drift(&everything), MAX_DRIFT.min(13 + 8 + 4));
        assert!(drift(&everything) <= MAX_DRIFT && drift(&worst) >= -MAX_DRIFT);
        for s in [everything, worst] {
            assert!((ship_percent(a, b, &s) as i32 - base).abs() <= MAX_DRIFT, "the pair's own number still shows through");
        }
        // One of them going quiet counts, and only once there is enough to go on.
        assert!(Together { replies_ab: 200, replies_ba: 3, ..Default::default() }.one_sided());
        assert!(!Together { replies_ab: 200, replies_ba: 60, ..Default::default() }.one_sided(), "both talking is not ghosting");
        assert!(!Together { replies_ab: 4, replies_ba: 0, ..Default::default() }.one_sided(), "four replies proves nothing");
        assert!(steps(&Signals { one_sided: true, ..Default::default() }).iter().any(|(what, by)| what.contains("gone quiet") && *by < 0));
        // And it is the same signal read from either side.
        let t = together();
        let other_way = Together { replies_ab: t.replies_ba, replies_ba: t.replies_ab, ..t.clone() };
        assert_eq!(t.signals(), other_way.signals());
        // The plain-English line names what moves it.
        for must in ["never changes", "member ids", "replying to each other", "voice", "games", "kalesh", "25"] {
            assert!(WHAT_MOVES_IT.contains(must), "the explanation lacks {must:?}");
        }
    }

    #[test]
    fn the_card_says_which_way_the_number_moved_only_when_it_moved() {
        let now = 1_800_000_000;
        assert_eq!(movement(61, None, now), None, "the first time, there is nothing to compare with");
        assert_eq!(movement(61, Some((61, now - 86_400)), now), None, "it hasn't moved: say nothing");
        assert_eq!(movement(67, Some((61, now - 7 * 86_400)), now).as_deref(), Some("up 6 since 7 days ago"));
        assert_eq!(movement(52, Some((61, now - 2 * 3600)), now).as_deref(), Some("down 9 since 2 hours ago"));
        assert_eq!(movement(62, Some((61, now - 30)), now).as_deref(), Some("up 1 since under a minute ago"));
        // A clock that has gone backwards doesn't produce nonsense.
        assert!(movement(70, Some((61, now + 500)), now).is_some_and(|m| m.starts_with("up 9")));
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
        let pct = ship_percent(a.id, b.id, &t.signals());
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
    fn the_card_line_is_the_most_interesting_thing_the_bot_counted() {
        let t = together();
        assert_eq!(headline(&t, 60), "3 fights called · 0 apologies logged");
        // A fight beats a reply count; without one, the replies speak.
        let calm = Together { fights: 0, ..t.clone() };
        assert_eq!(headline(&calm, 60), "412 replies in 60 days");
        assert_eq!(headline(&Together { fights: 1, ..t.clone() }, 60), "1 fight called · 0 apologies logged");
        assert_eq!(headline(&Together { replies_ab: 1, replies_ba: 0, ..calm.clone() }, 60), "1 reply in 60 days");
        // Then pings, then run-ins, then a shared channel, then nothing at all.
        let quiet = Together { replies_ab: 0, replies_ba: 0, ..calm.clone() };
        assert_eq!(headline(&quiet, 60), "61 pings at each other, 0 replies");
        let silent = Together { mentions: 0, ..quiet.clone() };
        assert_eq!(headline(&silent, 60), "22 run-ins in 60 days, not one reply");
        let strangers = Together { stretches: 0, ..silent.clone() };
        assert_eq!(headline(&strangers, 60), "Both live in #chatting-hori and have never once replied");
        let nothing = Together { shared_channels: vec![], ..strangers };
        assert_eq!(headline(&nothing, 60), "Not one word between them in 60 days");
        // Nothing the bot can count makes a line too long for the card.
        let huge = Together {
            fights: 999_999_999,
            replies_ab: usize::MAX / 4,
            replies_ba: usize::MAX / 4,
            mentions: usize::MAX / 2,
            stretches: usize::MAX / 2,
            shared_channels: vec!["a-channel-name-that-someone-really-did-name-this-🥳".repeat(4)],
            ..Default::default()
        };
        for t in [
            huge.clone(),
            Together { fights: 0, ..huge.clone() },
            Together { fights: 0, replies_ab: 0, replies_ba: 0, ..huge.clone() },
            Together { fights: 0, replies_ab: 0, replies_ba: 0, mentions: 0, ..huge.clone() },
            Together { fights: 0, replies_ab: 0, replies_ba: 0, mentions: 0, stretches: 0, ..huge },
        ] {
            let line = headline(&t, 9_999_999);
            assert!(line.chars().count() <= HEADLINE_CHARS, "{} chars: {:?}", line.chars().count(), line);
        }
    }

    #[test]
    fn the_score_is_written_out_only_when_there_is_no_card() {
        let with_card = score_line("arjun", "riya", 87, true);
        assert_eq!(with_card, "**arjun** × **riya**");
        assert!(!with_card.contains('%'), "the picture carries the number");
        let text_only = score_line("arjun", "riya", 87, false);
        assert!(text_only.contains("**87%**") && text_only.contains("█"), "{text_only}");
        assert!(text_only.starts_with("**arjun** × **riya**"));
    }

    /// What one call actually costs, so a change that quietly doubles it is
    /// noticed here rather than on the bill. Run with `--nocapture` to print it.
    #[test]
    fn one_call_stays_inside_its_budget() {
        // A roast of someone with a full history: the whole message sample.
        let full = Dossier { messages: sample(&(0..4_000).map(|i| Said { ts: i, text: format!("bhai ye {} wala match dekha kya, ekdum scene tha", i) }).collect::<Vec<_>>(), READ_BUDGET), ..arjun() };
        let roast = estimate_tokens(&roast_prompt(&full));
        let thin = estimate_tokens(&roast_prompt(&nobody()));
        let ship = estimate_tokens(&ship_prompt(&arjun(), &riya(), &together(), 87, "Arjya"));
        println!("one /roast: {} tokens in (thin member: {}); one /ship: {} tokens in", roast, thin, ship);
        // The prompt's own words, without anyone's messages.
        let bare = estimate_tokens(&roast_prompt(&Dossier { messages: vec![], ..arjun() }));
        println!("the roast prompt's own words: {} tokens", bare);
        // Roughly: the instructions and the record are under a thousand tokens,
        // the member's own messages are the rest, and a ship - which sends both
        // records and what they said at each other, but neither member's whole
        // message sample - is far cheaper than a roast.
        assert!(bare < 1_000, "the instructions and the record alone cost {bare} tokens");
        assert!(thin < 1_000, "a member with nothing to go on costs {thin} tokens");
        assert!(roast < READ_BUDGET + 1_200, "a full roast costs {roast} tokens");
        assert!(ship < 3_000, "a ship costs {ship} tokens");
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
