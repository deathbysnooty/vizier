//! `/ship`, the part that is plain logic: what the bot knows about a member
//! gathered into a dossier, the prompt built from two of them, the check every
//! model reply must pass before anything is posted, the ship name and the ship
//! percentage, and where the result goes when the command was run somewhere
//! else.
//!
//! Nothing here touches Discord or a model - `roast.rs` does that - so the
//! tests can hand it a fake model and a made-up dossier.
//!
//! The rules are narrower than member notes'. A ship verdict is allowed to be
//! sharp: the "unkind" filter notes use is deliberately not here, and neither is
//! the ban on talking about someone's routine or the games they lose. What stays
//! banned is the cruelty the owner ruled out - slurs, family, death, looks and
//! body, mental health, gender and sexuality, religion and caste - and those
//! stay out even when the member jokes about them themselves. #safe-corner
//! never reaches the model: nothing said there is in the message log to begin
//! with.

use std::future::Future;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::notes_build::{cut, estimate_tokens};
use super::ship_score::{self, Between, Score};

pub use super::ship_score::{MAX_PERCENT, MIN_PERCENT, WHAT_MOVES_IT, base as ship_base};

/// The longest a ship verdict may be.
pub const SHIP_CHARS: usize = 500;
/// Tries at one model answer: one, then one more if the first is refused.
pub const TRIES: usize = 2;

// --- the dossier ------------------------------------------------------------------------------

/// Everything the bot can honestly say about one member. Only what is in here
/// may end up in a verdict: the prompt says so, and nothing else is sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Dossier {
    pub id: u64,
    pub name: String,
    /// Their pronouns, off this server's own roles. Never worked out from their
    /// name or their messages: unknown is they/them, like everywhere else.
    pub pronouns: super::pronouns::Pronouns,
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
}

impl Dossier {
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

    /// The two things about a pair that aren't on either member's nightly
    /// sheet: how often they got going, and how often it turned into a fight.
    /// Everything else the score uses is a share of somebody's own total, and
    /// those come off the sheets.
    pub fn between(&self) -> Between {
        Between { stretches: self.stretches, fights: self.fights }
    }
}

// --- the prompt -------------------------------------------------------------------------------

/// The server's voice.
const VOICE: &str = "You are Loduchand, the house bot of MLCI, an Indian Discord server of friends who roast each other \
all day (bakchodi). Everyone there writes Hinglish - Hindi typed in Roman letters, mixed freely with English, full of \
Indian slang and short forms (\"bhai\", \"yaar\", \"scene kya hai\", \"op\", \"sahi hai\", \"bakchodi\", \"lmao\"). Read \
their messages the way a fluent member of that server would: it is not broken English and not typos. Write the same \
way they talk - mostly English sentences with the Hinglish thrown in where it lands - so it sounds like a friend in \
the channel, not a stand-up set and not a greeting card.";

/// The lines the prompt is held to.
const LIMITS: &str = "HARD LIMITS - break one and this is thrown away:
- No slur of any kind: caste, religion, region, race, sexual orientation, disability. Not as a joke, not quoting anyone.
- Nothing about their family, anyone's death, their looks or their body, their mental health, their gender or \
sexuality, their religion or their caste. These stay out even if they joke about them themselves.
- Invent nothing. Every joke lands on something written in the record below - no made-up incidents, no made-up quotes, \
no guessing at what they are like off the server.
- Punch at what they do, never at what they are.
- Never guess, infer or imply anyone's gender - not from their name, not from how they write, not from anything else. \
Their pronouns are given below, from this server's own roles. Use exactly those.
- No emoji spam: one at most, and none is better. No @everyone or @here.";

pub fn ship_prompt(a: &Dossier, b: &Dossier, t: &Together, score: &Score, ship_name: &str) -> String {
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

The number starts as their own, worked out from their member ids, and is moved {drift} by what they have actually \
done - not by how much they talk, but by how much of each other's talking they get:
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

{pronouns}
THE TWO OF THEM
{a}

{b}

HOW THEY ARE AROUND EACH OTHER
{between}

SOME OF WHAT THEY HAVE SAID AT EACH OTHER:
{sample}",
        voice = VOICE,
        percent = score.percent,
        drift = match score.moved() {
            0 => "not at all".to_string(),
            d if d > 0 => format!("up {} points", d),
            d => format!("down {} points", -d),
        },
        moved = {
            let told = score.told();
            if told.is_empty() {
                "- nothing the bot can count has moved it either way".to_string()
            } else {
                told.iter().map(|(what, by)| format!("- {} ({}{})", what, if *by > 0 { "+" } else { "" }, by)).collect::<Vec<_>>().join("\n")
            }
        },
        ship = ship_name,
        nothing = nothing,
        limits = LIMITS,
        pronouns = super::pronouns::block(&[(a.name.clone(), a.pronouns), (b.name.clone(), b.pronouns)]),
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
/// Hinglish ("bhai", "yaar") must never be in here or every verdict would fail.
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
/// The model declining rather than writing anything. Never posted as a verdict.
static REFUSAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:i'?m sorry|i am sorry|sorry[,.]? (?:but )?i\b|i can'?t\b|i cannot\b|i won'?t\b|i'?m not able|i am not able|as an ai|unfortunately,? i\b)")
        .expect("regex")
});

/// Why a piece of writing may not be posted, or `None` when it may.
pub fn rejection(text: &str) -> Option<&'static str> {
    if text.contains("@everyone") || text.contains("@here") {
        return Some("a mass ping");
    }
    BANNED_RES.iter().find(|(_, re)| re.is_match(text)).map(|(area, matched)| {
        tracing::info!("roast: thrown away for {} ({:?})", area, matched.find(text).map(|m| m.as_str().to_string()));
        *area
    })
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
        // No JSON at all: the model wrote the verdict straight out. Take it.
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
pub const MODEL_DOWN: &str = "The AI isn't answering right now, so no ship. Try again in a few minutes.";

pub fn failure_message(made: &Made) -> &'static str {
    match made {
        Made::Failed { .. } => MODEL_DOWN,
        _ => COULDNT,
    }
}

// --- opting out ----------------------------------------------------------------------------------

/// Who, if anyone, has put themselves out of reach - the first one named wins,
/// because any one of them stops the whole thing.
pub fn blocked(optouts: &[u64], people: &[u64]) -> Option<u64> {
    people.iter().copied().find(|p| optouts.contains(p))
}

/// What the person who ran the command is told. Kind, and it names nobody else's
/// business beyond the fact that they are out.
pub fn opted_out_message(who: u64, caller: u64, name: &str) -> String {
    if who == caller {
        "You've opted out of ships, so I'm not going to do one. Run `/noroast` again to come back in.".to_string()
    } else {
        format!("**{}** has opted out of ships, so that one's off the table. Nothing was posted.", name)
    }
}

// --- the ship ---------------------------------------------------------------------------------------

/// The whole percentage lives in [`ship_score`]: shares and mutuality off the
/// two nightly sheets, the pair's own base number, and the guards. This is the
/// one way in, so nothing else has to know how it is put together.
pub fn ship_score(a: u64, b: u64, sa: &ship_score::Side, sb: &ship_score::Side, t: &Between) -> Score {
    ship_score::score(a, b, sa, sb, t)
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

/// `/ship` only ever posts in the roast channel. When it was run somewhere
/// else, this is what that other place is told.
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
    use std::collections::HashSet;
    use std::sync::Arc;

    const ARJUN: u64 = 771;
    const RIYA: u64 = 982;

    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A member with a real history: the test data the samples in the report come from.
    pub fn arjun() -> Dossier {
        Dossier {
            id: ARJUN,
            name: "arjun".into(),
            pronouns: super::super::pronouns::Pronouns::He,
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
        }
    }

    pub fn riya() -> Dossier {
        Dossier {
            id: RIYA,
            name: "riya".into(),
            pronouns: super::super::pronouns::Pronouns::She,
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

    use super::ship_score::Side;

    /// One member's nightly sheet, reduced to what the score reads: how much
    /// replying they do in all, and how much of it goes to the other one.
    fn side(replies_sent: i64, replies_to_them: i64) -> Side {
        let mut hours = [0u32; 24];
        for (i, slot) in hours.iter_mut().enumerate() {
            *slot = if (18..=23).contains(&i) { 200 } else { 5 };
        }
        Side {
            replies_sent,
            replies_to_them,
            vc_minutes: 600,
            vc_with_them: 300,
            messages: 9_000,
            hours,
            channels: vec![(1, 8_000), (2, 1_000)],
            games: vec!["Chess".into()],
        }
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
    fn someone_who_opted_out_is_never_shipped() {
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
    fn the_check_lets_a_sharp_verdict_through_and_stops_a_cruel_one() {
        let fine = r#"{"verdict": "412 replies between them and riya has won 41 of 58 chess games. This is not a ship, it is a hostage situation with extra steps. They say ek minute at each other for sixty days and call it chemistry."}"#;
        let out = check_ship(fine).expect("a sharp, specific verdict is fine");
        assert!(out.contains("ek minute"));
        for (why, bad) in [
            ("a slur", r#"{"verdict": "you absolute retard, 412 replies and 3 fights between them"}"#),
            ("family", r#"{"verdict": "they fight like brother and sister honestly, 412 replies between them bhai"}"#),
            ("death", r#"{"verdict": "this pairing died a slow death, honestly go dig a grave for those 412 replies"}"#),
            ("looks or body", r#"{"verdict": "two ugly people replying 412 times at each other does not make a ship"}"#),
            ("mental health", r#"{"verdict": "412 replies is not a friendship it is a disorder, get therapy both of you"}"#),
            ("gender or sexuality", r#"{"verdict": "58 chess games together and he still plays like a virgin, sorry bhai"}"#),
            ("religion or caste", r#"{"verdict": "they pray to the chess bot every night and it still says no, 3 fights bhai"}"#),
            ("a mass ping", r#"{"verdict": "@everyone come look at these two and their 412 replies, genuinely tragic"}"#),
        ] {
            assert_eq!(check_ship(bad), Err(why), "{bad}");
        }
        // Too long for the embed, and too short to be anything.
        let long = format!(r#"{{"verdict": "{}"}}"#, "bhai ye bohot lamba hai ".repeat(100));
        assert_eq!(check_ship(&long), Err("it was too long to post"));
        assert_eq!(check_ship(r#"{"verdict": "lol"}"#), Err("it came back empty"));
        // The model declining is never posted as if it were the joke.
        assert_eq!(check_ship("I'm sorry, I can't help with that."), Err("the model refused"));
        assert_eq!(check_ship(r#"{"verdict": "I cannot write a verdict about these members without more information to go on."}"#), Err("the model refused"));
        // A fenced answer, and a wrong-shaped one.
        assert!(check_ship("```json\n{\"verdict\": \"412 replies in 60 days and 3 fights. ek minute, ek minute, and still no rematch won bhai.\"}\n```").is_ok());
        assert_eq!(check_ship(r#"{"something_else": "..."}"#), Err("the reply couldn't be read"));
    }

    #[tokio::test]
    async fn a_refused_answer_is_asked_again_once_and_then_given_up_on() {
        let bad = r#"{"verdict": "her mother has 41 wins in 58 chess games and she doesn't even play, bhai"}"#;
        let good = r#"{"verdict": "412 replies in 60 days. They say ek minute at each other like it is a magic spell and it has never once worked."}"#;
        // Refused, then fine: two calls, and the good one is what comes out.
        let calls = Arc::new(AtomicUsize::new(0));
        let made = make("p".into(), check_ship, fake(vec![Ok(bad), Ok(good)], calls.clone())).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(matches!(&made, Made::Ok { tries: 2, text } if text.contains("ek minute")), "{made:?}");
        // Refused twice: given up on, and the person is told plainly.
        let calls = Arc::new(AtomicUsize::new(0));
        let made = make("p".into(), check_ship, fake(vec![Ok(bad), Ok(bad)], calls.clone())).await;
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
            check_ship,
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
        let good = r#"{"verdict": "412 replies in 60 days, and they still ask for a rematch like it will help bhai."}"#;
        let made = make("p".into(), check_ship, fake(vec![Err(anyhow::anyhow!("dropped")), Ok(good)], calls.clone())).await;
        assert!(matches!(made, Made::Ok { tries: 2, .. }), "{made:?}");
    }

    // --- the ship ---------------------------------------------------------------

    /// Two sheets and one pair, as `/ship` puts them together. The bands each
    /// scenario lands in are proved in `ship_score.rs`; what matters here is
    /// that the seam between the two is symmetric and doesn't lose anything.
    #[test]
    fn a_pair_always_scores_the_same_whichever_way_round_it_is_asked() {
        let (sa, sb) = (side(500, 400), side(3_000, 380));
        let t = together().between();
        for (a, b) in [(ARJUN, RIYA), (1, 2), (999_999_999_999_999_999, 4), (7, 7)] {
            assert_eq!(ship_score(a, b, &sa, &sb, &t).percent, ship_score(b, a, &sb, &sa, &t).percent, "{a} and {b}");
            assert_eq!(ship_base(a, b), ship_base(b, a));
        }
        // The same every time it is asked, not a fresh roll.
        let first = ship_score(ARJUN, RIYA, &sa, &sb, &t).percent;
        for _ in 0..50 {
            assert_eq!(ship_score(RIYA, ARJUN, &sb, &sa, &t).percent, first);
        }
        // Never a suspiciously round nothing or everything, whatever happens.
        for pair in [(1u64, 2u64), (ARJUN, RIYA), (5, 900), (12, 13)] {
            for (x, y, t) in [
                (Side::default(), Side::default(), Between::default()),
                (side(9_000, 9_000), side(9_000, 9_000), Between { stretches: 900, fights: 0 }),
                (side(9_000, 1), side(9_000, 1), Between { stretches: 1, fights: 900 }),
            ] {
                let p = ship_score(pair.0, pair.1, &x, &y, &t).percent;
                assert!((MIN_PERCENT..=MAX_PERCENT).contains(&p), "{p}% from {pair:?}");
            }
        }
        // Different pairs don't all land on the same number.
        // Not as many as there are pairs - the number only lands on whole steps
        // of 3 off each base - but nowhere near one shared answer either.
        let spread: HashSet<u8> = (1..40u64).map(|i| ship_score(i, i * 7 + 3, &sa, &sb, &t).percent).collect();
        assert!(spread.len() >= 8, "only {} distinct scores in 39 pairs", spread.len());
        assert_eq!(bar(0), "░░░░░░░░░░");
        assert_eq!(bar(100), "██████████");
        assert_eq!(bar(41).chars().filter(|c| *c == '█').count(), 5);
    }

    /// The pair's own two numbers - the ones that aren't on anybody's sheet -
    /// reach the score, and read the same from either side.
    #[test]
    fn what_happened_between_them_reaches_the_score_from_either_side() {
        let t = together();
        assert_eq!(t.between(), Between { stretches: 22, fights: 3 });
        let other_way = Together { replies_ab: t.replies_ba, replies_ba: t.replies_ab, ..t.clone() };
        assert_eq!(t.between(), other_way.between(), "it is the same pair read from the other side");
        // Fights pull the number down; back-and-forth pushes it up.
        let (sa, sb) = (side(500, 250), side(500, 250));
        let calm = ship_score(ARJUN, RIYA, &sa, &sb, &Between { stretches: 30, fights: 0 }).percent;
        let fighty = ship_score(ARJUN, RIYA, &sa, &sb, &Between { stretches: 30, fights: 8 }).percent;
        assert!(fighty < calm, "eight fights ({fighty}%) has to read below none ({calm}%)");
        // Nothing on record between them at all: their own number, untouched.
        let quiet = ship_score(ARJUN, RIYA, &Side::default(), &Side::default(), &Between::default());
        assert_eq!(quiet.percent as i32, ship_base(ARJUN, RIYA).clamp(1, 99));
        assert!(quiet.told().is_empty(), "{:?}", quiet.told());
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
        let scored = ship_score(a.id, b.id, &side(500, 400), &side(480, 390), &t.between());
        let name = ship_name(&a.name, &b.name);
        let p = ship_prompt(&a, &b, &t, &scored, &name);
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
            // Their pronouns come off their roles; the verdict never guesses.
            "- arjun: he/him",
            "- riya: she/her",
            "Never guess, infer or imply anyone's gender",
        ] {
            assert!(p.contains(must), "the ship prompt lacks {must:?}");
        }
        // Two members with no role the bot can read are both they/them.
        let unknown = ship_prompt(&nobody(), &Dossier { id: 56, name: "spectre".into(), ..Default::default() }, &t, &scored, "Ghospectre");
        assert!(unknown.contains("- ghost: they/them") && unknown.contains("- spectre: they/them"), "{unknown}");
        assert!(p.contains(&format!("{}%", scored.percent)) && p.contains(&name));
        // The verdict is told which way the number moved and what moved it.
        assert!(p.contains("how much of each other's talking they get"), "{p}");
        for (what, by) in scored.told() {
            assert!(p.contains(what), "the prompt lost {what:?}");
            assert!(p.contains(&format!("({}{})", if by > 0 { "+" } else { "" }, by)), "the prompt lost the {by} on {what:?}");
        }
        assert!(!p.contains("basically never interacted"));
        // Two strangers: the prompt says so instead of letting the model invent one.
        let strangers = Together { shared_channels: vec![], ..Default::default() };
        assert!(strangers.nothing());
        let nothing = ship_score(a.id, b.id, &Side::default(), &Side::default(), &strangers.between());
        let p = ship_prompt(&a, &b, &strangers, &nothing, "Arjiya");
        assert!(p.contains("nothing the bot can count has moved it either way"), "{p}");
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

    /// Nothing creepy reaches anybody. The nightly sheets are how the number is
    /// worked out and that is all they are for: not one thing off a sheet may
    /// show up on the card or in the prompt. The card's one line may only say
    /// something anyone in the server could have noticed - a count of replies,
    /// time in voice, a shared channel, or the plain absence of any of it.
    #[test]
    fn nothing_off_a_nightly_sheet_reaches_the_card_or_the_model() {
        // Every shape the card's line can take, over everything that can happen.
        let allowed = Regex::new(
            r"^(?:\d+ fights? called · 0 apologies logged|\d+ repl(?:y|ies) in \d+ days|\d+ pings? at each other, 0 replies|\d+ run-ins? in \d+ days, not one reply|Both live in #\S.* and have never once replied|Not one word between them in \d+ days)$",
        )
        .expect("regex");
        let full = together();
        for t in [
            full.clone(),
            Together { fights: 0, ..full.clone() },
            Together { fights: 0, replies_ab: 0, replies_ba: 0, ..full.clone() },
            Together { fights: 0, replies_ab: 0, replies_ba: 0, mentions: 0, ..full.clone() },
            Together { fights: 0, replies_ab: 0, replies_ba: 0, mentions: 0, stretches: 0, ..full.clone() },
            Together::default(),
        ] {
            let line = headline(&t, 60);
            assert!(allowed.is_match(&line), "the card said something it cannot count: {line:?}");
        }
        // A sheet with a whole habit on it, every number on it unmistakable, so
        // a leak of any one of them shows up as itself and not as a coincidence.
        use super::super::ship_sheet::{self, LogRow};
        let rows: Vec<LogRow> = (0..90)
            .map(|i| LogRow { channel_id: 606_060_606, created_ms: 1_700_000_000_000 + i * 86_400_000, len: 7_777, reply_author: Some(505_050_505) })
            .collect();
        let sheet = ship_sheet::build(ARJUN, &rows, vec!["Chess".into()], 4_242, vec![(RIYA, 3_131)], 1_700_000_000);
        assert_eq!((sheet.avg_len, sheet.channels[0].0, sheet.reply_partners[0].0), (7_777, 606_060_606, 505_050_505));
        let scored = ship_score(ARJUN, RIYA, &sheet.side(RIYA), &sheet.side(ARJUN), &full.between());
        let out = format!(
            "{}\n{}\n{}\n{}",
            ship_prompt(&arjun(), &riya(), &full, &scored, "Arjya"),
            headline(&full, 60),
            score_line("arjun", "riya", scored.percent, false),
            scored.told().iter().map(|(what, by)| format!("{what} {by}")).collect::<Vec<_>>().join(" "),
        );
        for private in [
            sheet.avg_len.to_string(),
            sheet.vc_minutes.to_string(),
            sheet.vc_with(RIYA).to_string(),
            sheet.channels[0].0.to_string(),
            sheet.reply_partners[0].0.to_string(),
        ] {
            assert!(!out.contains(&private), "a sheet's {private:?} got out");
        }
        for word in ["burst", "avg_len", "histogram", "hour of", "typical message", "personality", "interests"] {
            assert!(!out.to_lowercase().contains(word), "{word:?} has no business being in a ship");
        }
        // What the breakdown does say is plain English about counted things.
        for (what, _) in scored.told() {
            assert!(what.chars().all(|c| c.is_ascii_lowercase() || " -,'".contains(c)), "{what:?}");
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
        // Two members with a full history, and what they said at each other.
        let scored = ship_score(ARJUN, RIYA, &side(500, 400), &side(480, 390), &together().between());
        let ship = estimate_tokens(&ship_prompt(&arjun(), &riya(), &together(), &scored, "Arjya"));
        // Two strangers: both records, and nothing between them.
        let none = ship_score(55, 56, &Side::default(), &Side::default(), &Between::default());
        let strangers = estimate_tokens(&ship_prompt(&nobody(), &nobody(), &Together::default(), &none, "Ghosost"));
        println!("one /ship: {} tokens in (two strangers: {})", ship, strangers);
        // Roughly: the instructions and both records are the bulk of it, and
        // what they said at each other is capped at fourteen short lines - so a
        // ship never runs away with the bill. The floor moved once, by about
        // sixty tokens, when the verdict started being handed both members'
        // pronouns instead of working them out for itself.
        assert!(strangers < 1_100, "two members with nothing to go on cost {strangers} tokens");
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
