//! The scrolls a duel is fought over: the puzzle bank, the forgiving answer
//! matcher, and the `puzzles` table that remembers every instance used.
//!
//! A duel is no longer a clash of buttons. Three scrolls go up (a fourth if the
//! first three leave it level); each carries one puzzle, drawn on the card
//! itself rather than hidden in a modal, and the first fighter to answer it
//! correctly wins the scroll and lands a real blow. First to two scrolls takes
//! the duel.
//!
//! **Templates, not a list.** Nothing here is a hand-written puzzle. Every
//! template generates a fresh instance each time - how many, which one is odd,
//! where they sit, what colour they are - so the bank is effectively endless and
//! there is nothing to memorise. [`TEMPLATES`] holds them; [`generate`] rolls
//! one.
//!
//! **Visual puzzles are the point.** They resist being handed to a chat bot,
//! because screenshotting and uploading a card is slower than just looking at
//! it. The mix is about three visual to two text ([`VISUAL_IN_FIVE`]).
//!
//! **Under three seconds.** Every instance has to be solvable at a glance: no
//! template draws ten of anything, and no card carries two puzzles' worth of
//! detail.
//!
//! **One riddle bank, one matcher.** The desi/Hinglish riddles are not written
//! here: they come from [`super::riddle`], the month's shared bank, which also
//! keeps the `used` flag that stops a raven and a duel asking the same riddle
//! the same evening. Answers go through that module's matcher too, so there is
//! one idea of what "close enough" means in the whole bot.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::riddle;

/// Whether a puzzle is drawn on the card or simply asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Visual,
    Text,
}

impl Mode {
    pub fn key(self) -> &'static str {
        match self {
            Mode::Visual => "visual",
            Mode::Text => "text",
        }
    }

    pub fn from_key(key: &str) -> Mode {
        if key.trim().eq_ignore_ascii_case("text") { Mode::Text } else { Mode::Visual }
    }
}

/// How many of every five scrolls are drawn rather than asked.
pub const VISUAL_IN_FIVE: u64 = 3;

/// A shape the card can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Glyph {
    Sword,
    Raven,
    Brazier,
    Crown,
    Shield,
    Dragon,
    Banner,
    /// A three-headed sigil, or a two-headed one when `heads` says so.
    Sigil,
    Eye,
}

impl Glyph {
    /// What to call it in a prompt, singular and plural.
    pub fn words(self) -> (&'static str, &'static str) {
        match self {
            Glyph::Sword => ("sword", "swords"),
            Glyph::Raven => ("raven", "ravens"),
            Glyph::Brazier => ("brazier", "braziers"),
            Glyph::Crown => ("crown", "crowns"),
            Glyph::Shield => ("shield", "shields"),
            Glyph::Dragon => ("dragon", "dragons"),
            Glyph::Banner => ("banner", "banners"),
            Glyph::Sigil => ("sigil", "sigils"),
            Glyph::Eye => ("eye", "eyes"),
        }
    }
}

/// One shape, placed. `x` and `y` are fractions of the puzzle panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub glyph: Glyph,
    pub x: f32,
    pub y: f32,
    /// 1.0 is the panel's ordinary size for that shape.
    pub scale: f32,
    pub colour: [u8; 3],
    /// Mirrored left to right.
    #[serde(default)]
    pub flip: bool,
    /// Turned upside down.
    #[serde(default)]
    pub upside: bool,
    /// A brazier alight, or a sword still whole.
    #[serde(default)]
    pub lit: bool,
    /// Heads on a sigil: 3 usually, 2 for the odd one out.
    #[serde(default)]
    pub heads: u8,
}

impl Item {
    fn new(glyph: Glyph, x: f32, y: f32, colour: [u8; 3]) -> Item {
        Item { glyph, x, y, scale: 1.0, colour, flip: false, upside: false, lit: true, heads: 3 }
    }
}

/// Everything the card renderer needs to draw a puzzle. One flat shape covers
/// every template: a scene of placed glyphs, with digits on a shield and two
/// blocks of figures as the two special cases.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spec {
    #[serde(default)]
    pub items: Vec<Item>,
    /// Scorched digits on a shield, when there are any.
    #[serde(default)]
    pub digits: String,
    /// Two blocks of figures: how many stand on each side.
    #[serde(default)]
    pub armies: Option<(usize, usize)>,
}

impl Spec {
    /// The spec as the `spec` column holds it. An unserialisable spec stores as
    /// an empty string rather than failing the fight.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn from_json(raw: &str) -> Spec {
        serde_json::from_str(raw).unwrap_or_default()
    }
}

/// One puzzle instance: the question, what counts as right, and what to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Puzzle {
    /// Its row in `puzzles`, or 0 before it is stored.
    pub id: i64,
    /// For a riddle out of the shared bank, that riddle's id, so a scroll can
    /// be read back without the answer ever being kept anywhere a player could
    /// reach. Empty for a puzzle this module generated.
    pub riddle: String,
    /// The template that made it, e.g. `count_swords`.
    pub kind: &'static str,
    pub mode: Mode,
    /// The question, as the card prints it.
    pub prompt: String,
    /// The canonical answer.
    pub answer: String,
    /// Everything else that counts as right.
    pub alts: Vec<String>,
    pub spec: Spec,
}

// --- rolling ----------------------------------------------------------------

/// A small xorshift, so a duel's scrolls are reproducible in a test and the
/// bank needs no rand dependency.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }

    /// 0..n.
    pub fn upto(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }

    /// `low..=high`.
    pub fn between(&mut self, low: u64, high: u64) -> u64 {
        low + self.upto(high.saturating_sub(low) + 1)
    }

    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.upto(xs.len() as u64) as usize]
    }

    fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            let j = self.upto(i as u64 + 1) as usize;
            xs.swap(i, j);
        }
    }
}

// --- words ------------------------------------------------------------------

const NUMBERS: [&str; 13] =
    ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve"];
const ORDINALS: [&str; 9] =
    ["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth"];
const SHORT_ORDINALS: [&str; 9] = ["1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th", "9th"];

/// A count, as a digit plus every other way of saying it.
fn count_answer(n: usize) -> (String, Vec<String>) {
    let mut alts = Vec::new();
    if let Some(word) = NUMBERS.get(n) {
        alts.push(word.to_string());
    }
    (n.to_string(), alts)
}

/// A position in a row of `n`, as a digit plus the ordinal, the short ordinal,
/// and left/middle/right where those are not ambiguous.
fn position_answer(i: usize, n: usize) -> (String, Vec<String>) {
    let mut alts = Vec::new();
    if let Some(word) = ORDINALS.get(i) {
        alts.push(word.to_string());
    }
    if let Some(short) = SHORT_ORDINALS.get(i) {
        alts.push(short.to_string());
    }
    if i == 0 {
        alts.push("left".into());
        alts.push("leftmost".into());
    }
    if i + 1 == n {
        alts.push("right".into());
        alts.push("rightmost".into());
        alts.push("last".into());
    }
    // "Middle" only means something in an odd row with a real middle.
    if n % 2 == 1 && i == n / 2 && n > 1 {
        alts.push("middle".into());
        alts.push("centre".into());
        alts.push("center".into());
    }
    ((i + 1).to_string(), alts)
}

/// Colours a puzzle may dye a shape, with the names an answer may use.
const DYES: [(&str, [u8; 3], &[&str]); 6] = [
    ("red", [196, 48, 44], &["crimson", "scarlet"]),
    ("gold", [214, 168, 62], &["yellow", "amber"]),
    ("green", [74, 158, 96], &[]),
    ("blue", [84, 134, 206], &["azure"]),
    ("grey", [96, 104, 120], &["gray", "slate", "silver"]),
    ("purple", [146, 102, 200], &["violet"]),
];

// --- the forgiving matcher --------------------------------------------------

/// An answer boiled down to what it actually says. The bot's one idea of that,
/// borrowed from the riddle bank: lower case, accents folded, punctuation gone,
/// apostrophes closed up, a leading a/an/the dropped.
pub use riddle::normalise;

/// Whether an answer counts. The shared matcher: forgiving about case, spacing
/// and a slipped letter in a long word, and not forgiving at all about a number
/// - a count or a position has to be the number it is.
pub fn matches(given: &str, puzzle: &Puzzle) -> bool {
    if given.trim().is_empty() {
        return false;
    }
    let answers: Vec<String> =
        std::iter::once(puzzle.answer.clone()).chain(puzzle.alts.iter().cloned()).collect();
    riddle::is_correct(given, &answers)
}

// --- the templates ----------------------------------------------------------

/// One template: its key, whether it is drawn or asked, and how to roll an
/// instance of it.
pub struct Template {
    pub kind: &'static str,
    pub mode: Mode,
    /// Rolls one instance, or `None` when this template has nothing to offer
    /// right now - the riddle bank is not open on a fresh workspace, and a
    /// template that cannot roll is passed over rather than faked. Public so
    /// the card's own tests and the preview can draw every template without
    /// going through [`generate`]'s dice.
    pub make: fn(&mut Rng) -> Option<Puzzle>,
}

/// Lays `n` shapes out in a row across the panel, nudged a little so a row
/// never looks like a ruler.
fn row(rng: &mut Rng, glyph: Glyph, n: usize, colour: [u8; 3]) -> Vec<Item> {
    (0..n)
        .map(|i| {
            let x = (i as f32 + 0.5) / n as f32;
            let y = 0.5 + (rng.upto(9) as f32 - 4.0) / 90.0;
            Item::new(glyph, x, y, colour)
        })
        .collect()
}

/// A count puzzle over one kind of shape: "How many swords on this banner?".
fn counted(rng: &mut Rng, kind: &'static str, glyph: Glyph) -> Puzzle {
    let n = rng.between(5, 9) as usize;
    let (_, dye, _) = *rng.pick(&DYES);
    let (answer, alts) = count_answer(n);
    let (_, plural) = glyph.words();
    Puzzle {
        id: 0,
        riddle: String::new(),
        kind,
        mode: Mode::Visual,
        prompt: format!("How many {} do you see?", plural),
        answer,
        alts,
        spec: Spec { items: row(rng, glyph, n, dye), ..Spec::default() },
    }
}

/// One shape in a row is wrong in some way; the answer is where it sits.
fn odd_one_out(
    rng: &mut Rng,
    kind: &'static str,
    glyph: Glyph,
    prompt: &str,
    spoil: fn(&mut Item),
) -> Puzzle {
    let n = rng.between(4, 6) as usize;
    let (_, dye, _) = *rng.pick(&DYES);
    let mut items = row(rng, glyph, n, dye);
    let odd = rng.upto(n as u64) as usize;
    spoil(&mut items[odd]);
    let (answer, alts) = position_answer(odd, n);
    Puzzle {
        id: 0,
        riddle: String::new(),
        kind,
        mode: Mode::Visual,
        prompt: format!("{} (count from the left)", prompt),
        answer,
        alts,
        spec: Spec { items, ..Spec::default() },
    }
}

pub static TEMPLATES: &[Template] = &[
    // --- counting ---------------------------------------------------------
    Template { kind: "count_swords", mode: Mode::Visual, make: |r| Some(counted(r, "count_swords", Glyph::Sword)) },
    Template { kind: "count_ravens", mode: Mode::Visual, make: |r| Some(counted(r, "count_ravens", Glyph::Raven)) },
    Template { kind: "count_crowns", mode: Mode::Visual, make: |r| Some(counted(r, "count_crowns", Glyph::Crown)) },
    Template { kind: "count_shields", mode: Mode::Visual, make: |r| Some(counted(r, "count_shields", Glyph::Shield)) },
    Template { kind: "count_dragons", mode: Mode::Visual, make: |r| Some(counted(r, "count_dragons", Glyph::Dragon)) },
    Template {
        kind: "count_flames",
        mode: Mode::Visual,
        make: |rng| {
            let n = rng.between(6, 9) as usize;
            let lit = rng.between(3, (n as u64 - 1).max(3)) as usize;
            let mut items = row(rng, Glyph::Brazier, n, [206, 142, 62]);
            let mut which: Vec<usize> = (0..n).collect();
            rng.shuffle(&mut which);
            for item in items.iter_mut() {
                item.lit = false;
            }
            for i in which.into_iter().take(lit) {
                items[i].lit = true;
            }
            let (answer, alts) = count_answer(lit);
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "count_flames",
                mode: Mode::Visual,
                prompt: "How many braziers are lit?".into(),
                answer,
                alts,
                spec: Spec { items, ..Spec::default() },
            })
        },
    },
    Template {
        kind: "count_eyes",
        mode: Mode::Visual,
        make: |rng| {
            let n = rng.between(4, 8) as usize;
            // Scattered rather than ranked: eyes in the dark do not queue up.
            let items: Vec<Item> = (0..n)
                .map(|i| {
                    let x = 0.12 + (i as f32 + 0.5) / n as f32 * 0.76;
                    let y = 0.26 + rng.upto(50) as f32 / 100.0;
                    let mut item = Item::new(Glyph::Eye, x, y, [255, 186, 72]);
                    item.scale = 0.8 + rng.upto(40) as f32 / 100.0;
                    item
                })
                .collect();
            let (answer, alts) = count_answer(n);
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "count_eyes",
                mode: Mode::Visual,
                prompt: "How many eyes are watching?".into(),
                answer,
                alts,
                spec: Spec { items, ..Spec::default() },
            })
        },
    },
    // --- which one is wrong -----------------------------------------------
    Template {
        kind: "flipped_dragon",
        mode: Mode::Visual,
        make: |r| Some(odd_one_out(r, "flipped_dragon", Glyph::Dragon, "Which dragon faces the other way?", |i| i.flip = true)),
    },
    Template {
        kind: "upside_banner",
        mode: Mode::Visual,
        make: |r| Some(odd_one_out(r, "upside_banner", Glyph::Banner, "Which banner hangs upside down?", |i| i.upside = true)),
    },
    Template {
        kind: "missing_head",
        mode: Mode::Visual,
        make: |r| Some(odd_one_out(r, "missing_head", Glyph::Sigil, "Which sigil has only two heads?", |i| i.heads = 2)),
    },
    Template {
        kind: "broken_sword",
        mode: Mode::Visual,
        make: |r| Some(odd_one_out(r, "broken_sword", Glyph::Sword, "Which sword is snapped?", |i| i.lit = false)),
    },
    Template {
        kind: "tallest_torch",
        mode: Mode::Visual,
        make: |r| Some(odd_one_out(r, "tallest_torch", Glyph::Brazier, "Which brazier burns tallest?", |i| i.scale = 1.5)),
    },
    Template {
        kind: "shortest_sword",
        mode: Mode::Visual,
        make: |r| Some(odd_one_out(r, "shortest_sword", Glyph::Sword, "Which sword is shortest?", |i| i.scale = 0.6)),
    },
    Template {
        kind: "odd_colour",
        mode: Mode::Visual,
        make: |rng| {
            let n = rng.between(4, 6) as usize;
            let mut dyes: Vec<usize> = (0..DYES.len()).collect();
            rng.shuffle(&mut dyes);
            let (field, odd_dye) = (dyes[0], dyes[1]);
            let mut items = row(rng, Glyph::Sigil, n, DYES[field].1);
            let odd = rng.upto(n as u64) as usize;
            items[odd].colour = DYES[odd_dye].1;
            let (answer, mut alts) = position_answer(odd, n);
            // The colour itself is just as good an answer as the position.
            alts.push(DYES[odd_dye].0.to_string());
            alts.extend(DYES[odd_dye].2.iter().map(|a| a.to_string()));
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "odd_colour",
                mode: Mode::Visual,
                prompt: "One sigil is a different colour - which one? (its place, or its colour)".into(),
                answer,
                alts,
                spec: Spec { items, ..Spec::default() },
            })
        },
    },
    // --- which shape ------------------------------------------------------
    Template {
        kind: "duplicate_crest",
        mode: Mode::Visual,
        make: |rng| {
            // Shapes nobody could mistake for each other: a dragon and a raven
            // are too close in silhouette for a three-second glance.
            let shapes = [Glyph::Sword, Glyph::Crown, Glyph::Shield, Glyph::Eye, Glyph::Raven];
            let n = rng.between(4, 6) as usize;
            let mut order: Vec<Glyph> = shapes.to_vec();
            rng.shuffle(&mut order);
            // One shape twice, the rest once each.
            let twice = order[0];
            let mut drawn: Vec<Glyph> = vec![twice, twice];
            drawn.extend(order.iter().skip(1).take(n - 2).copied());
            rng.shuffle(&mut drawn);
            let (_, dye, _) = *rng.pick(&DYES);
            let items: Vec<Item> = drawn
                .iter()
                .enumerate()
                .map(|(i, glyph)| {
                    let x = (i as f32 + 0.5) / drawn.len() as f32;
                    Item::new(*glyph, x, 0.5, dye)
                })
                .collect();
            let (singular, plural) = twice.words();
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "duplicate_crest",
                mode: Mode::Visual,
                prompt: "One shape on this shield appears twice - which?".into(),
                answer: singular.to_string(),
                alts: vec![plural.to_string()],
                spec: Spec { items, ..Spec::default() },
            })
        },
    },
    Template {
        kind: "which_crest_missing",
        mode: Mode::Visual,
        make: |rng| {
            let shapes = [Glyph::Sword, Glyph::Crown, Glyph::Shield, Glyph::Eye];
            let mut order: Vec<Glyph> = shapes.to_vec();
            rng.shuffle(&mut order);
            let missing = order[3];
            let (_, dye, _) = *rng.pick(&DYES);
            let items: Vec<Item> = order[..3]
                .iter()
                .enumerate()
                .map(|(i, glyph)| Item::new(*glyph, (i as f32 + 0.5) / 3.0, 0.5, dye))
                .collect();
            let names: Vec<&str> = shapes.iter().map(|g| g.words().0).collect();
            let (singular, plural) = missing.words();
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "which_crest_missing",
                mode: Mode::Visual,
                prompt: format!("{} - which one is NOT on the shield?", names.join(", ")),
                answer: singular.to_string(),
                alts: vec![plural.to_string()],
                spec: Spec { items, ..Spec::default() },
            })
        },
    },
    Template {
        kind: "most_common_sigil",
        mode: Mode::Visual,
        make: |rng| {
            let shapes = [Glyph::Sword, Glyph::Crown, Glyph::Shield];
            let mut order: Vec<Glyph> = shapes.to_vec();
            rng.shuffle(&mut order);
            // A clear winner: four of one, and never more than two of another.
            let most = rng.between(4, 5) as usize;
            let second = rng.between(1, 2) as usize;
            let third = rng.between(1, 2) as usize;
            let mut drawn: Vec<Glyph> = Vec::new();
            for (glyph, count) in [(order[0], most), (order[1], second), (order[2], third)] {
                drawn.extend(std::iter::repeat_n(glyph, count));
            }
            rng.shuffle(&mut drawn);
            let (_, dye, _) = *rng.pick(&DYES);
            let per_row = drawn.len().div_ceil(2);
            let items: Vec<Item> = drawn
                .iter()
                .enumerate()
                .map(|(i, glyph)| {
                    let (col, line) = (i % per_row, i / per_row);
                    let x = (col as f32 + 0.5) / per_row as f32;
                    let y = if line == 0 { 0.31 } else { 0.72 };
                    let mut item = Item::new(*glyph, x, y, dye);
                    item.scale = 0.74;
                    item
                })
                .collect();
            let (singular, plural) = order[0].words();
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "most_common_sigil",
                mode: Mode::Visual,
                prompt: "Which shape appears the most?".into(),
                answer: singular.to_string(),
                alts: vec![plural.to_string()],
                spec: Spec { items, ..Spec::default() },
            })
        },
    },
    Template {
        kind: "bigger_army",
        mode: Mode::Visual,
        make: |rng| {
            // Never close: one side has at least three more than the other.
            let small = rng.between(2, 5) as usize;
            let big = small + rng.between(3, 4) as usize;
            let left_wins = rng.upto(2) == 0;
            let (left, right) = if left_wins { (big, small) } else { (small, big) };
            let (answer, alts) = if left_wins {
                ("left".to_string(), vec!["1".into(), "first".into()])
            } else {
                ("right".to_string(), vec!["2".into(), "second".into()])
            };
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "bigger_army",
                mode: Mode::Visual,
                prompt: "Which host is the bigger one - left or right?".into(),
                answer,
                alts,
                spec: Spec { armies: Some((left, right)), ..Spec::default() },
            })
        },
    },
    Template {
        kind: "read_shield",
        mode: Mode::Visual,
        make: |rng| {
            let digits = rng.between(100, 999).to_string();
            Some(Puzzle {
                id: 0,
                riddle: String::new(),
                kind: "read_shield",
                mode: Mode::Visual,
                prompt: "Read the number burned into the shield.".into(),
                answer: digits.clone(),
                alts: Vec::new(),
                spec: Spec { digits, ..Spec::default() },
            })
        },
    },
    // --- asked rather than drawn -------------------------------------------
    // The month's own riddle bank. One bank for the ravens and the scrolls
    // both, so the same riddle is never asked twice in an evening.
    Template { kind: "riddle", mode: Mode::Text, make: riddle_scroll },
    Template { kind: "house_words", mode: Mode::Text, make: |r| Some(house_words(r)) },
    Template { kind: "anagram", mode: Mode::Text, make: |r| Some(anagram(r)) },
    Template { kind: "odd_word_out", mode: Mode::Text, make: |r| Some(odd_word_out(r)) },
];

// --- the asked puzzles ------------------------------------------------------

/// House words, with the one word taken out. The lore of the world, not the
/// server's four houses: these read the same before and after the hatch.
const WORDS: [(&str, &str, &[&str]); 12] = [
    ("Ours is the ___", "fury", &[]),
    ("Winter is ___", "coming", &[]),
    ("Hear me ___", "roar", &[]),
    ("Fire and ___", "blood", &[]),
    ("We do not ___", "sow", &[]),
    ("Growing ___", "strong", &[]),
    ("Unbowed, unbent, ___", "unbroken", &[]),
    ("Family, duty, ___", "honour", &["honor"]),
    ("As high as ___", "honour", &["honor"]),
    ("A Lannister always pays his ___", "debts", &["debt"]),
    ("The night is dark and full of ___", "terrors", &["terror"]),
    ("Valar ___ (all men must die)", "morghulis", &[]),
];

fn house_words(rng: &mut Rng) -> Puzzle {
    let (line, answer, alts) = *rng.pick(&WORDS);
    Puzzle {
        id: 0,
        riddle: String::new(),
        kind: "house_words",
        mode: Mode::Text,
        prompt: format!("Finish the words: \u{201c}{}\u{201d}", line),
        answer: answer.to_string(),
        alts: alts.iter().map(|a| a.to_string()).collect(),
        spec: Spec::default(),
    }
}

/// Words a scroll may jumble. Plain enough that nobody needs the wiki.
const JUMBLE: [&str; 24] = [
    "dragon", "winter", "throne", "raven", "sword", "castle", "wolf", "crown", "knight", "banner", "shield", "север",
    "ember", "melee", "squire", "maester", "septa", "harvest", "lantern", "frost", "armour", "tourney", "chalice",
    "rookery",
];

fn anagram(rng: &mut Rng) -> Puzzle {
    // The non-latin entry is a guard against a word list nobody checked; skip
    // anything that is not plain ascii rather than asking an unfair scroll.
    let word = loop {
        let candidate = *rng.pick(&JUMBLE);
        if candidate.is_ascii() && candidate.len() >= 4 {
            break candidate;
        }
    };
    let mut letters: Vec<char> = word.chars().collect();
    // Shuffled until it actually differs, so the answer is never printed.
    for _ in 0..12 {
        rng.shuffle(&mut letters);
        if letters.iter().collect::<String>() != word {
            break;
        }
    }
    Puzzle {
        id: 0,
        riddle: String::new(),
        kind: "anagram",
        mode: Mode::Text,
        prompt: format!("Unjumble it: {}", letters.iter().collect::<String>().to_uppercase()),
        answer: word.to_string(),
        alts: Vec::new(),
        spec: Spec::default(),
    }
}

/// Three of a kind and one that does not belong.
const SETS: [(&[&str], &str); 8] = [
    (&["sword", "spear", "axe"], "shield"),
    (&["wolf", "lion", "stag"], "castle"),
    (&["north", "south", "east"], "winter"),
    (&["raven", "owl", "eagle"], "dragon"),
    (&["helm", "gauntlet", "breastplate"], "chalice"),
    (&["samosa", "pakora", "kachori"], "chai"),
    (&["maester", "septa", "squire"], "dragon"),
    (&["frost", "snow", "ice"], "ember"),
];

fn odd_word_out(rng: &mut Rng) -> Puzzle {
    let (group, odd) = *rng.pick(&SETS);
    let mut all: Vec<&str> = group.to_vec();
    all.push(odd);
    rng.shuffle(&mut all);
    Puzzle {
        id: 0,
        riddle: String::new(),
        kind: "odd_word_out",
        mode: Mode::Text,
        prompt: format!("Which does not belong? {}", all.join(", ")),
        answer: odd.to_string(),
        alts: Vec::new(),
        spec: Spec::default(),
    }
}

/// A riddle out of the month's shared bank. The answer is never kept on the
/// puzzle row in a form anybody could read back out of the card: the riddle's
/// own id goes on the row, and the bank is asked again when an answer comes in.
///
/// An easy one: a scroll has to be read in a few seconds, not pondered. When
/// the bank is not open - a fresh workspace, or a test - this rolls nothing and
/// [`generate`] takes another template instead, rather than this file growing a
/// riddle bank of its own to fall back on.
fn riddle_scroll(rng: &mut Rng) -> Option<Puzzle> {
    let roll = rng.upto(10_000) as f64 / 10_000.0;
    let found = riddle::pick(riddle::Difficulty::Easy, roll)?;
    Some(Puzzle {
        id: 0,
        riddle: found.id.clone(),
        kind: "riddle",
        mode: Mode::Text,
        prompt: found.riddle.clone(),
        answer: found.canonical().to_string(),
        alts: found.answers.iter().skip(1).cloned().collect(),
        spec: Spec::default(),
    })
}

// --- rolling a scroll -------------------------------------------------------

/// Rolls one puzzle, avoiding any template in `used`. Three in five are drawn
/// rather than asked; if every template of the wanted mode has been used this
/// duel, the other mode is taken rather than repeating one, and a template that
/// declines to roll - the riddle bank is not open - is passed over.
pub fn generate(rng: &mut Rng, used: &[&'static str]) -> Puzzle {
    let want = if rng.upto(5) < VISUAL_IN_FIVE { Mode::Visual } else { Mode::Text };
    let other = if want == Mode::Visual { Mode::Text } else { Mode::Visual };
    let free = |mode: Mode| -> Vec<&Template> {
        TEMPLATES.iter().filter(|t| t.mode == mode && !used.contains(&t.kind)).collect()
    };
    // The wanted mode first, then the other, then everything: a duel that has
    // somehow used the lot starts over rather than handing back nothing.
    for mut pool in [free(want), free(other), TEMPLATES.iter().collect()] {
        while !pool.is_empty() {
            let at = rng.upto(pool.len() as u64) as usize;
            if let Some(puzzle) = (pool[at].make)(rng) {
                return puzzle;
            }
            pool.remove(at);
        }
    }
    // Every template declining at once, which cannot happen while one of them
    // needs nothing behind it. A count of swords always can.
    counted(rng, "count_swords", Glyph::Sword)
}

// --- the store --------------------------------------------------------------

/// The `puzzles` table, created by `battle::open` alongside the rest.
pub const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS puzzles (
     id INTEGER PRIMARY KEY AUTOINCREMENT,
     kind TEXT NOT NULL,
     mode TEXT NOT NULL,
     prompt TEXT NOT NULL,
     answer TEXT NOT NULL,
     alts TEXT NOT NULL DEFAULT '',
     spec TEXT NOT NULL DEFAULT '',
     riddle TEXT NOT NULL DEFAULT '',
     seen INTEGER NOT NULL DEFAULT 0,
     solved INTEGER NOT NULL DEFAULT 0,
     ts INTEGER NOT NULL);
 CREATE INDEX IF NOT EXISTS puzzles_kind ON puzzles (kind);";

/// Writes one instance down and returns its row id, with `seen` already at one:
/// a stored puzzle is a puzzle somebody was shown.
pub fn record(conn: &Connection, puzzle: &Puzzle, now: i64) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO puzzles (kind, mode, prompt, answer, alts, spec, riddle, seen, solved, ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, ?8)",
        params![
            puzzle.kind,
            puzzle.mode.key(),
            puzzle.prompt,
            puzzle.answer,
            puzzle.alts.join("|"),
            puzzle.spec.to_json(),
            puzzle.riddle,
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Marks a stored instance as solved.
pub fn solved(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE puzzles SET solved = solved + 1 WHERE id = ?1", params![id])?;
    Ok(())
}

/// How often a template has been shown and how often it was answered, newest
/// first, for seeing later which scrolls are too hard or too easy.
pub fn tally(conn: &Connection) -> Vec<(String, i64, i64)> {
    conn.prepare("SELECT kind, SUM(seen), SUM(solved) FROM puzzles GROUP BY kind ORDER BY SUM(seen) DESC")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
        .unwrap_or_default()
}

/// One stored instance read back, for a scroll that outlived a restart.
pub fn read(conn: &Connection, id: i64) -> Option<Puzzle> {
    let row = conn
        .query_row(
            "SELECT kind, mode, prompt, answer, alts, spec, riddle FROM puzzles WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6).unwrap_or_default(),
                ))
            },
        )
        .optional()
        .ok()
        .flatten()?;
    let (kind, mode, prompt, answer, alts, spec, riddle) = row;
    // The kind comes back as the template's own static name, so a row written
    // by an older build with a template since renamed reads as unknown.
    let kind = TEMPLATES.iter().find(|t| t.kind == kind).map(|t| t.kind).unwrap_or("unknown");
    Some(Puzzle {
        id,
        riddle,
        kind,
        mode: Mode::from_key(&mode),
        prompt,
        answer,
        alts: alts.split('|').filter(|a| !a.is_empty()).map(str::to_string).collect(),
        spec: Spec::from_json(&spec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch(SCHEMA).expect("puzzle schema");
        conn
    }

    #[test]
    fn the_bank_has_plenty_of_both_kinds_and_leans_visual() {
        let visual = TEMPLATES.iter().filter(|t| t.mode == Mode::Visual).count();
        let text = TEMPLATES.iter().filter(|t| t.mode == Mode::Text).count();
        assert!(visual >= 18, "only {} visual templates", visual);
        assert!(text >= 3, "only {} text templates", text);
        // Every key is its own: a repeated key would make "never twice in one
        // duel" silently wrong.
        let keys: std::collections::HashSet<&str> = TEMPLATES.iter().map(|t| t.kind).collect();
        assert_eq!(keys.len(), TEMPLATES.len(), "two templates share a key");
    }

    /// The mix over many scrolls: about three in five drawn rather than asked.
    #[test]
    fn about_three_scrolls_in_five_are_drawn() {
        let mut rng = Rng::new(7);
        let mut visual = 0;
        let n = 4000;
        for _ in 0..n {
            if generate(&mut rng, &[]).mode == Mode::Visual {
                visual += 1;
            }
        }
        let share = visual as f64 / n as f64;
        assert!((0.52..0.68).contains(&share), "visual share {}", share);
    }

    /// Every template, many instances each: nothing panics, every instance has
    /// a prompt and an answer, and nothing draws more than nine of anything.
    #[test]
    fn every_template_rolls_a_fair_instance() {
        for template in TEMPLATES {
            let mut seen = std::collections::HashSet::new();
            for seed in 1..200u64 {
                let mut rng = Rng::new(seed * 2_654_435_761);
                // The riddle bank is not open in a test, so that one template
                // rolls nothing; every other must always roll.
                let Some(p) = (template.make)(&mut rng) else {
                    assert_eq!(template.kind, "riddle", "{} declined to roll", template.kind);
                    continue;
                };
                assert_eq!(p.kind, template.kind);
                assert_eq!(p.mode, template.mode);
                assert!(!p.prompt.trim().is_empty(), "{} has a blank prompt", p.kind);
                assert!(!p.answer.trim().is_empty(), "{} has a blank answer", p.kind);
                assert!(!normalise(&p.answer).is_empty(), "{} answers with punctuation only", p.kind);
                // Solvable at a glance: never a crowd, and never two puzzles
                // worth of detail on one card.
                assert!(p.spec.items.len() <= 10, "{} draws {} things", p.kind, p.spec.items.len());
                if let Some((l, r)) = p.spec.armies {
                    assert!(l + r <= 20 && l != r, "{} armies {} vs {}", p.kind, l, r);
                }
                assert!(p.spec.digits.chars().all(|c| c.is_ascii_digit()));
                // A visual puzzle must actually draw something.
                if p.mode == Mode::Visual {
                    let drawn = !p.spec.items.is_empty() || !p.spec.digits.is_empty() || p.spec.armies.is_some();
                    assert!(drawn, "{} is visual but draws nothing", p.kind);
                }
                // The answer must never be sitting in the prompt.
                if p.kind == "anagram" {
                    assert!(!normalise(&p.prompt).contains(&normalise(&p.answer)), "{}", p.prompt);
                }
                // Every placed item is inside the panel.
                for item in &p.spec.items {
                    assert!((0.0..=1.0).contains(&item.x) && (0.0..=1.0).contains(&item.y), "{:?}", item);
                    assert!((0.4..=1.8).contains(&item.scale), "{:?}", item);
                }
                // The whole instance, not just the question: a template with
                // one fixed prompt still has to lay its shapes out afresh.
                seen.insert(format!("{}|{}|{:?}", p.prompt, p.answer, p.spec));
            }
            // Randomised: a template that always rolls the same instance could
            // be memorised in an afternoon. A handful of templates have a small
            // space by nature - two hosts is two hosts - but even those move
            // the answer about, so a dozen is enough that a bracket of fifteen
            // duels never shows the same scroll twice.
            if template.kind != "riddle" {
                assert!(seen.len() >= 10, "{} only ever rolls {} instances", template.kind, seen.len());
            }
        }
    }

    #[test]
    fn a_duel_never_sees_the_same_template_twice() {
        let mut rng = Rng::new(99);
        for _ in 0..200 {
            let mut used: Vec<&'static str> = Vec::new();
            for _ in 0..4 {
                let p = generate(&mut rng, &used);
                assert!(!used.contains(&p.kind), "{} came up twice in one duel", p.kind);
                used.push(p.kind);
            }
        }
    }

    #[test]
    fn the_matcher_forgives_everything_but_a_wrong_answer() {
        let puzzle = Puzzle {
            id: 0,
            riddle: String::new(),
            kind: "count_swords",
            mode: Mode::Visual,
            prompt: "How many swords do you see?".into(),
            answer: "7".into(),
            alts: vec!["seven".into()],
            spec: Spec::default(),
        };
        for given in ["7", " 7 ", "seven", "SEVEN", "Seven!", "  the   seven  ", "a seven", "an seven."] {
            assert!(matches(given, &puzzle), "{} should count", given);
        }
        for given in ["", "   ", "6", "eight", "sevenish", "?"] {
            assert!(!matches(given, &puzzle), "{} should not count", given);
        }
    }

    /// The matcher is the bot's one matcher - the riddle bank's - so this says
    /// what the arena relies on it for rather than restating its rules.
    #[test]
    fn the_shared_normaliser_is_what_the_scrolls_use() {
        assert_eq!(normalise("  The  Red!! "), normalise("red"));
        assert_eq!(normalise("A  Dragon"), normalise("dragon"));
        assert_eq!(normalise("an ember"), normalise("ember"));
        assert_eq!(normalise("2ND."), normalise("2nd"));
        assert_eq!(normalise("Night's Watch"), normalise("nights watch"));
        assert!(normalise("!!!").is_empty(), "punctuation alone says nothing");
    }

    /// A position answer takes the digit, the ordinal and the side.
    #[test]
    fn a_position_is_answered_any_sensible_way() {
        let (answer, alts) = position_answer(0, 5);
        assert_eq!(answer, "1");
        assert!(alts.contains(&"first".to_string()) && alts.contains(&"left".to_string()));
        let (answer, alts) = position_answer(2, 5);
        assert_eq!(answer, "3");
        assert!(alts.contains(&"middle".to_string()) && alts.contains(&"3rd".to_string()));
        let (answer, alts) = position_answer(4, 5);
        assert_eq!(answer, "5");
        assert!(alts.contains(&"right".to_string()) && alts.contains(&"last".to_string()));
        // An even row has no middle, so "middle" is never accepted in one.
        let (_, alts) = position_answer(2, 4);
        assert!(!alts.contains(&"middle".to_string()), "{:?}", alts);
        // And the position templates really do offer all of that.
        let mut rng = Rng::new(5);
        for _ in 0..50 {
            let p = (TEMPLATES.iter().find(|t| t.kind == "flipped_dragon").unwrap().make)(&mut rng).expect("an instance");
            let n = p.spec.items.len();
            let odd = p.spec.items.iter().position(|i| i.flip).expect("one faces the other way");
            assert!(matches(&(odd + 1).to_string(), &p));
            assert!(matches(ORDINALS[odd], &p));
            if odd == 0 {
                assert!(matches("left", &p));
            }
            assert_eq!(n, p.spec.items.len());
        }
    }

    /// A count answer takes the digit and the number word.
    #[test]
    fn a_count_is_answered_in_digits_or_words() {
        let mut rng = Rng::new(11);
        for _ in 0..60 {
            let p = (TEMPLATES.iter().find(|t| t.kind == "count_swords").unwrap().make)(&mut rng).expect("an instance");
            let n = p.spec.items.len();
            assert!((5..=9).contains(&n), "{} swords", n);
            assert!(matches(&n.to_string(), &p));
            assert!(matches(NUMBERS[n], &p));
            assert!(!matches(&(n + 1).to_string(), &p));
        }
    }

    /// The lit braziers are the answer, and there is always at least one unlit
    /// one - otherwise "how many are lit" is just "how many are there".
    #[test]
    fn the_lit_braziers_are_the_ones_counted() {
        let mut rng = Rng::new(3);
        for _ in 0..200 {
            let p = (TEMPLATES.iter().find(|t| t.kind == "count_flames").unwrap().make)(&mut rng).expect("an instance");
            let lit = p.spec.items.iter().filter(|i| i.lit).count();
            assert!(lit >= 1 && lit < p.spec.items.len(), "{} of {} lit", lit, p.spec.items.len());
            assert!(matches(&lit.to_string(), &p));
        }
    }

    #[test]
    fn exactly_one_shape_is_doubled_and_the_missing_one_is_really_missing() {
        let mut rng = Rng::new(21);
        let dup = TEMPLATES.iter().find(|t| t.kind == "duplicate_crest").unwrap();
        let gone = TEMPLATES.iter().find(|t| t.kind == "which_crest_missing").unwrap();
        for _ in 0..200 {
            let p = (dup.make)(&mut rng).expect("an instance");
            let mut counts: std::collections::HashMap<Glyph, usize> = std::collections::HashMap::new();
            for item in &p.spec.items {
                *counts.entry(item.glyph).or_default() += 1;
            }
            let twice: Vec<&Glyph> = counts.iter().filter(|(_, n)| **n == 2).map(|(g, _)| g).collect();
            assert_eq!(twice.len(), 1, "{:?}", counts);
            assert!(counts.values().all(|n| *n <= 2), "{:?}", counts);
            assert!(matches(twice[0].words().0, &p));

            let p = (gone.make)(&mut rng).expect("an instance");
            let shown: Vec<&str> = p.spec.items.iter().map(|i| i.glyph.words().0).collect();
            assert_eq!(shown.len(), 3);
            assert!(!shown.contains(&p.answer.as_str()), "{} is on the shield", p.answer);
            // The prompt lists the four to choose between, the answer included.
            assert!(normalise(&p.prompt).contains(&normalise(&p.answer)), "{}", p.prompt);
        }
    }

    #[test]
    fn the_bigger_host_is_never_a_close_call() {
        let mut rng = Rng::new(31);
        for _ in 0..200 {
            let p = (TEMPLATES.iter().find(|t| t.kind == "bigger_army").unwrap().make)(&mut rng).expect("an instance");
            let (l, r) = p.spec.armies.expect("two hosts");
            assert!(l.abs_diff(r) >= 3, "{} vs {} is too close to call at a glance", l, r);
            assert!(matches(if l > r { "left" } else { "right" }, &p));
            assert!(!matches(if l > r { "right" } else { "left" }, &p));
        }
    }

    #[test]
    fn a_stored_puzzle_reads_back_as_itself_and_counts_as_seen() {
        let conn = memory();
        let mut rng = Rng::new(1234);
        let mut ids = Vec::new();
        for _ in 0..30 {
            let p = generate(&mut rng, &[]);
            let id = record(&conn, &p, 1_700_000_000).expect("stored");
            let back = read(&conn, id).expect("read back");
            assert_eq!((back.kind, back.mode, &back.prompt, &back.answer), (p.kind, p.mode, &p.prompt, &p.answer));
            assert_eq!(back.alts, p.alts);
            assert_eq!(back.spec, p.spec, "the spec the card draws from must survive the round trip");
            // Whatever the answer was, it still matches after the round trip.
            assert!(matches(&p.answer, &back));
            ids.push(id);
        }
        // Every one is seen once; the ones we solve are counted as solved.
        for id in ids.iter().take(10) {
            solved(&conn, *id).expect("solved");
        }
        let tally = tally(&conn);
        let seen: i64 = tally.iter().map(|(_, s, _)| s).sum();
        let done: i64 = tally.iter().map(|(_, _, s)| s).sum();
        assert_eq!((seen, done), (30, 10));
        assert!(tally.iter().all(|(kind, _, _)| TEMPLATES.iter().any(|t| t.kind == kind)), "{:?}", tally);
    }

    /// A row written by an older build, with a template that no longer exists,
    /// still reads rather than bringing a fight down.
    #[test]
    fn a_row_from_an_older_build_still_reads() {
        let conn = memory();
        conn.execute(
            "INSERT INTO puzzles (kind, mode, prompt, answer, alts, spec, seen, solved, ts)
             VALUES ('count_chappals', 'visual', 'How many?', '4', 'four', 'not json at all', 1, 0, 1)",
            [],
        )
        .expect("legacy row");
        let back = read(&conn, 1).expect("read back");
        assert_eq!(back.kind, "unknown");
        assert_eq!(back.spec, Spec::default(), "a spec that will not parse draws nothing");
        assert!(matches("four", &back) && matches("4", &back));
        assert_eq!(read(&conn, 999), None);
    }
}
