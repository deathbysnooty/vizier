//! The Guess the Word doodle bank: the drawings the bot puts up, and the words
//! it takes as answers.
//!
//! Two files in `<workspace>/drawbank/`, read once at start the way the
//! anagrams word bank reads `wordbank/`:
//!
//! * `words.json` — 240 words, each with the spellings a member might type and
//!   the slice of the pack its drawings live in.
//! * `doodles.bin` — the drawings themselves, one byte per coordinate, in the
//!   `QDPACK1` format `drawbank/GUIDE.md` sets out.
//!
//! Every drawing is a real one, by a real person, out of Google's Quick, Draw!
//! dataset — which is why [`ATTRIBUTION`] goes wherever the doodles do.
//!
//! A guess is judged on a plain form of itself: lower case with everything that
//! isn't a letter or a digit thrown away, so "Ice-Cream", "ice cream" and
//! "icecream" are one word. Exact only, except for a slip or two on longer
//! answers the way the frog riddles forgive spelling — and never a slip that
//! could just as well have been a DIFFERENT word in the bank, so "car" can
//! never take a round whose word is "police car".
//!
//! If the bank isn't there the game simply never starts: [`open`] says so once
//! in the log and leaves [`bank`] empty, and every part of the game checks it
//! before doing anything.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use serde::Deserialize;

use super::frog_answer::levenshtein;
use super::sudoku_gen::Rng;

/// The pack's first eight bytes, "QDPACK\x01\n".
pub const MAGIC: [u8; 8] = [0x51, 0x44, 0x50, 0x41, 0x43, 0x4b, 0x01, 0x0a];

/// Longer than this and a message can't be a guess at all.
const MAX_GUESS: usize = 64;

/// The credit the dataset's licence asks for, wherever the doodles appear.
pub const ATTRIBUTION: &str = "Doodles from Google's Quick, Draw! dataset (https://quickdraw.withgoogle.com/data), used under CC BY 4.0.";

static BANK: OnceLock<Bank> = OnceLock::new();

/// One stroke of a drawing: pen down, a line through every point, pen up. A
/// stroke of one point is a dot somebody meant to make - an eye, a freckle -
/// and is drawn, not skipped.
pub type Stroke = Vec<(u8, u8)>;

/// One drawing, in the 0-255 box the pack stores it in.
pub type Doodle = Vec<Stroke>;

/// The plain form both a guess and an answer are compared in: lower case, and
/// nothing but letters and digits. "Ice-Cream", "ice cream" and "icecream" all
/// come to "icecream".
pub fn plain(text: &str) -> String {
    text.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// What someone typed, as something to look up - or `None` when the message
/// can't be a guess at all.
pub fn tidy(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_GUESS {
        return None;
    }
    // `!hint` and `!skip` steer the round; they are never guesses.
    if trimmed.starts_with('!') || trimmed.starts_with('/') {
        return None;
    }
    let plain = plain(trimmed);
    (!plain.is_empty()).then_some(plain)
}

/// How many slips an answer of this many letters forgives: none on short words,
/// where one letter makes a different thing altogether, one from five to eight,
/// two from nine up. The frog riddles' rule, and for the same reason - people
/// type on phones.
pub fn slack(letters: usize) -> usize {
    match letters {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

/// One word in the bank: what it is called, what counts as naming it, and where
/// its drawings are.
#[derive(Debug)]
pub struct Word {
    /// What the bot announces when the round ends, and what `!hint` takes its
    /// first letter from.
    pub word: String,
    /// Every spelling that counts, the canonical one first.
    pub answers: Vec<String>,
    /// Those spellings in their plain form, which is what a guess is compared
    /// against.
    keys: Vec<String>,
    /// Where each of this word's drawings starts in the pack.
    starts: Vec<usize>,
}

impl Word {
    pub fn doodles(&self) -> usize {
        self.starts.len()
    }

    /// The letter a hint gives away.
    pub fn first_letter(&self) -> char {
        self.word.chars().next().unwrap_or('?').to_ascii_uppercase()
    }

    /// The word's plain form, which is what the store writes down so the same
    /// thing isn't drawn twice in a fortnight.
    pub fn key(&self) -> String {
        plain(&self.word)
    }
}

/// What `words.json` says.
#[derive(Deserialize)]
struct Manifest {
    format: String,
    index: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    word: String,
    answers: Vec<String>,
    count: usize,
    offset: usize,
    bytes: usize,
}

/// The drawings in play.
#[derive(Debug, Default)]
pub struct Bank {
    /// `doodles.bin`, whole. Under a megabyte, and every drawing is read
    /// straight out of it rather than kept decoded.
    pack: Vec<u8>,
    words: Vec<Word>,
}

impl Bank {
    /// Reads the manifest and the pack as they stand, checking as it goes that
    /// every word's slice really does hold the drawings it claims: a truncated
    /// pack is no bank at all, and the game is better off off than putting up
    /// half a picture.
    pub fn from_parts(manifest: &str, pack: Vec<u8>) -> anyhow::Result<Bank> {
        let manifest: Manifest = serde_json::from_str(manifest).map_err(|err| anyhow::anyhow!("words.json: {}", err))?;
        if manifest.format != "QDPACK1" {
            anyhow::bail!("words.json says format {}, which this reader doesn't know", manifest.format);
        }
        if pack.len() < MAGIC.len() || pack[..MAGIC.len()] != MAGIC {
            anyhow::bail!("doodles.bin doesn't start with the QDPACK1 magic");
        }
        let mut words = Vec::with_capacity(manifest.index.len());
        for entry in manifest.index {
            let starts = walk(&pack, &entry).map_err(|err| anyhow::anyhow!("{}: {}", entry.word, err))?;
            let keys: Vec<String> = entry.answers.iter().map(|a| plain(a)).filter(|a| !a.is_empty()).collect();
            if keys.is_empty() || starts.is_empty() {
                continue;
            }
            words.push(Word { word: entry.word, answers: entry.answers, keys, starts });
        }
        Ok(Bank { pack, words })
    }

    pub fn word_count(&self) -> usize {
        self.words.len()
    }

    pub fn doodle_count(&self) -> usize {
        self.words.iter().map(Word::doodles).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn word(&self, index: usize) -> Option<&Word> {
        self.words.get(index)
    }

    /// The letter a hint gives away, when the bank still has that word.
    pub fn first_letter_of(&self, word: usize) -> Option<char> {
        self.words.get(word).map(Word::first_letter)
    }

    /// The word a round wrote down, found again by its plain form. A bank that
    /// has been edited under a live round simply loses it, which the game reads
    /// as a round it can no longer judge.
    pub fn find(&self, key: &str) -> Option<usize> {
        self.words.iter().position(|w| w.key() == key)
    }

    /// One drawing, decoded out of the pack.
    pub fn doodle(&self, word: usize, doodle: usize) -> Option<Doodle> {
        let start = *self.words.get(word)?.starts.get(doodle)?;
        read_doodle(&self.pack, start).map(|(strokes, _)| strokes)
    }

    /// A word to draw: one that hasn't been up lately. When every word has been
    /// used the fortnight starts over rather than the channel sitting empty.
    pub fn pick(&self, used: &HashSet<String>, rng: &mut Rng) -> Option<usize> {
        if self.words.is_empty() {
            return None;
        }
        let start = rng.below(self.words.len());
        let fresh = (0..self.words.len()).map(|step| (start + step) % self.words.len()).find(|i| !used.contains(&self.words[*i].key()));
        Some(fresh.unwrap_or(start))
    }

    /// A drawing of that word nobody has been shown lately. The same picture
    /// twice in a fortnight would be a round somebody simply remembers.
    pub fn pick_doodle(&self, word: usize, used: &HashSet<i64>, rng: &mut Rng) -> Option<usize> {
        let count = self.words.get(word)?.doodles();
        if count == 0 {
            return None;
        }
        let start = rng.below(count);
        let fresh = (0..count).map(|step| (start + step) % count).find(|i| !used.contains(&(*i as i64)));
        Some(fresh.unwrap_or(start))
    }

    /// Whether a typed guess takes the round.
    ///
    /// The word's own spellings win outright, however the guess is punctuated -
    /// and the same spelling belonging to several words is on purpose, because
    /// "boat" really is a fair answer to a sailboat and a speedboat alike.
    /// Beyond that a slip is forgiven by length, but ONLY when no other word in
    /// the bank is just as near: "gitar" is a guitar and nothing else, while a
    /// guess a letter away from two different words is a guess, not an answer.
    pub fn wins(&self, word: usize, guess: &str) -> bool {
        let Some(guess) = tidy(guess) else { return false };
        let Some(mine) = self.words.get(word) else { return false };
        if mine.keys.iter().any(|key| *key == guess) {
            return true;
        }
        let Some(near) = mine.keys.iter().filter_map(|key| near_miss(&guess, key)).min() else { return false };
        !self
            .words
            .iter()
            .enumerate()
            .any(|(i, other)| i != word && other.keys.iter().any(|key| levenshtein(&guess, key) <= near))
    }
}

/// How far a guess is from one answer, when that is near enough to forgive.
fn near_miss(guess: &str, key: &str) -> Option<usize> {
    let allowed = slack(key.chars().count());
    if allowed == 0 {
        return None;
    }
    // A digit has to be exactly right: "platform 9" is not "platform 8".
    if guess.chars().chain(key.chars()).any(|c| c.is_ascii_digit()) {
        return None;
    }
    let distance = levenshtein(guess, key);
    (distance > 0 && distance <= allowed).then_some(distance)
}

/// Walks one word's slice of the pack, handing back where each drawing starts.
/// Everything the format promises is checked here, so nothing downstream has to.
fn walk(pack: &[u8], entry: &Entry) -> anyhow::Result<Vec<usize>> {
    let end = entry.offset.checked_add(entry.bytes).ok_or_else(|| anyhow::anyhow!("silly offset"))?;
    if end > pack.len() {
        anyhow::bail!("its drawings run past the end of the pack");
    }
    let mut at = entry.offset;
    let mut starts = Vec::with_capacity(entry.count);
    for _ in 0..entry.count {
        starts.push(at);
        let (_, next) = read_doodle(pack, at).ok_or_else(|| anyhow::anyhow!("a drawing at {} is cut short", at))?;
        if next > end {
            anyhow::bail!("a drawing at {} runs past the word's slice", at);
        }
        at = next;
    }
    if at != end {
        anyhow::bail!("{} bytes of drawings, but the manifest says {}", at - entry.offset, entry.bytes);
    }
    Ok(starts)
}

/// One drawing out of the pack, and where the next one starts: a stroke count,
/// then per stroke a point count and that many x bytes followed by that many y
/// bytes. `None` for anything that doesn't fit inside the pack.
fn read_doodle(pack: &[u8], at: usize) -> Option<(Doodle, usize)> {
    let strokes = *pack.get(at)? as usize;
    let mut at = at + 1;
    let mut out = Vec::with_capacity(strokes);
    for _ in 0..strokes {
        let points = *pack.get(at)? as usize;
        at += 1;
        let xs = pack.get(at..at + points)?;
        let ys = pack.get(at + points..at + 2 * points)?;
        at += 2 * points;
        out.push(xs.iter().copied().zip(ys.iter().copied()).collect());
    }
    Some((out, at))
}

/// Reads the bank out of a folder. The error says which file is missing or
/// unreadable, so the log tells a mod what to do about it.
pub fn load(dir: &Path) -> anyhow::Result<Bank> {
    let manifest = std::fs::read_to_string(dir.join("words.json")).map_err(|err| anyhow::anyhow!("{}: {}", dir.join("words.json").display(), err))?;
    let pack = std::fs::read(dir.join("doodles.bin")).map_err(|err| anyhow::anyhow!("{}: {}", dir.join("doodles.bin").display(), err))?;
    let bank = Bank::from_parts(&manifest, pack)?;
    if bank.is_empty() {
        anyhow::bail!("{} has no words in it", dir.join("words.json").display());
    }
    Ok(bank)
}

/// Reads `<workspace>/drawbank/` once, at start. A bank that isn't there is not
/// an error worth stopping for: the game stays off and says so, once.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["drawbank"]);
    match load(&dir) {
        Ok(bank) => {
            tracing::info!("guess: {} words and {} doodles read from {}", bank.word_count(), bank.doodle_count(), dir.display());
            let _ = BANK.set(bank);
        }
        Err(err) => tracing::warn!(
            "guess: no doodle bank ({}) — the game stays off. Copy drawbank/ with words.json and doodles.bin into the workspace.",
            err
        ),
    }
}

/// The bank, when there is one.
pub fn bank() -> Option<&'static Bank> {
    BANK.get()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A bank small enough to read, drawn by hand: a square, a two-stroke cross
    /// and a lone dot. Everything the reader can meet is in it - several
    /// spellings to a word, a word two words long, a stroke of one point, and
    /// two words a single letter apart.
    pub fn fixture() -> Bank {
        let mut pack = MAGIC.to_vec();
        let mut index = Vec::new();
        let words: [(&str, &[&str], usize); 5] = [
            ("guitar", &["guitar", "electric guitar"], 3),
            ("police car", &["police car", "cop car", "squad car"], 2),
            ("ice cream", &["ice cream", "icecream", "cone"], 2),
            ("mouse", &["mouse"], 1),
            ("house", &["house"], 1),
        ];
        for (n, (word, answers, count)) in words.into_iter().enumerate() {
            let offset = pack.len();
            for d in 0..count {
                pack.extend(drawing(n + d));
            }
            let bytes = pack.len() - offset;
            index.push(format!(
                r#"{{"word": "{}", "answers": [{}], "count": {}, "offset": {}, "bytes": {}}}"#,
                word,
                answers.iter().map(|a| format!("\"{}\"", a)).collect::<Vec<_>>().join(", "),
                count,
                offset,
                bytes
            ));
        }
        let manifest = format!(r#"{{"format": "QDPACK1", "index": [{}]}}"#, index.join(", "));
        Bank::from_parts(&manifest, pack).expect("a bank")
    }

    /// One made-up drawing: a box, a stroke across it, and a dot.
    fn drawing(seed: usize) -> Vec<u8> {
        let o = (seed * 7) as u8;
        vec![
            3, // three strokes
            5,
            o,
            o + 100,
            o + 100,
            o,
            o, // x
            0,
            0,
            200,
            200,
            0, // y
            2,
            o,
            o + 100,
            10,
            190, // a diagonal
            1,
            o + 50,
            100, // a dot
        ]
    }

    #[test]
    fn a_guess_is_read_in_its_plainest_form() {
        assert_eq!(plain("Ice-Cream"), "icecream");
        assert_eq!(plain("ice cream"), "icecream");
        assert_eq!(plain("  A  POLICE   CAR!! "), "apolicecar");
        assert_eq!(plain("🎉"), "");
        assert_eq!(tidy("Guitar?"), Some("guitar".into()));
        assert_eq!(tidy("!hint"), None, "the two words that steer a round are never guesses");
        assert_eq!(tidy("/guess"), None);
        assert_eq!(tidy("   "), None);
        assert_eq!(tidy("😀😀"), None);
        assert_eq!(tidy(&"a".repeat(MAX_GUESS + 1)), None, "an essay is not a guess");
    }

    #[test]
    fn every_spelling_of_the_word_wins_and_nothing_else_does() {
        let bank = fixture();
        let guitar = bank.find("guitar").expect("the guitar");
        for yes in ["guitar", "GUITAR", " a Guitar! ", "electric guitar", "electric-guitar", "electricguitar"] {
            assert!(bank.wins(guitar, yes), "{:?} names the drawing", yes);
        }
        for no in ["banjo", "", "🎸", "guit", "police car"] {
            assert!(!bank.wins(guitar, no), "{:?} does not", no);
        }
        // Several words sharing a spelling is on purpose, and each round is its own.
        let police = bank.find("policecar").expect("the police car");
        assert!(bank.wins(police, "cop car") && bank.wins(police, "squadcar"));
        assert!(!bank.wins(police, "car"), "a part of the answer is not the answer");
        assert!(!bank.wins(guitar, "car"));
    }

    #[test]
    fn a_slip_is_forgiven_but_never_as_far_as_another_word() {
        let bank = fixture();
        let guitar = bank.find("guitar").expect("the guitar");
        assert!(bank.wins(guitar, "gitar"), "one letter short of six");
        assert!(bank.wins(guitar, "guitarr"));
        assert!(!bank.wins(guitar, "gtar"), "two slips on six letters is too many");
        // A near miss that another word answers just as well is no answer at all.
        let cream = bank.find("icecream").expect("the ice cream");
        assert!(bank.wins(cream, "icecrem"));
        assert!(bank.wins(cream, "Ice creem"));
        assert!(bank.wins(cream, "cone!"), "punctuation around an answer is forgiven");
        assert!(!bank.wins(cream, "cane"), "one letter off a four letter answer is a different thing");
        assert!(!bank.wins(cream, "copcar"), "that is the other round's word");
        // A slip that lands as near one word as another is a guess, not an
        // answer: nobody wins a mouse by nearly typing a house.
        let (mouse, house) = (bank.find("mouse").expect("a mouse"), bank.find("house").expect("a house"));
        assert!(bank.wins(mouse, "mouce") && !bank.wins(house, "mouce"), "one letter from a mouse and two from a house");
        assert!(!bank.wins(mouse, "wouse") && !bank.wins(house, "wouse"), "one letter from both");
        assert!(bank.wins(house, "House.") && bank.wins(mouse, "MOUSE"), "and each is still its own word typed out");
        // Short answers are exact, whatever the round.
        assert_eq!(slack(3), 0);
        assert_eq!(slack(8), 1);
        assert_eq!(slack(9), 2);
    }

    #[test]
    fn the_pack_decodes_to_the_strokes_it_was_written_from() {
        let bank = fixture();
        assert_eq!(bank.word_count(), 5);
        assert_eq!(bank.doodle_count(), 9);
        let guitar = bank.find("guitar").expect("the guitar");
        assert_eq!(bank.word(guitar).map(Word::doodles), Some(3));
        let doodle = bank.doodle(guitar, 0).expect("a drawing");
        assert_eq!(doodle.len(), 3, "three strokes");
        assert_eq!(doodle[0].len(), 5, "a five point box");
        assert_eq!(doodle[0][0], (0, 0));
        assert_eq!(doodle[2].len(), 1, "a dot is a stroke of one point, and is kept");
        // Every drawing of every word decodes, and none is empty.
        for w in 0..bank.word_count() {
            for d in 0..bank.word(w).expect("a word").doodles() {
                let doodle = bank.doodle(w, d).expect("a drawing");
                assert!(!doodle.is_empty() && doodle.iter().all(|s| !s.is_empty()));
            }
        }
        assert_eq!(bank.doodle(guitar, 99), None, "there is no hundredth drawing");
        assert_eq!(bank.doodle(99, 0), None);
        assert_eq!(bank.first_letter_of(guitar), Some('G'));
    }

    #[test]
    fn a_word_and_a_drawing_are_both_picked_without_repeating_themselves() {
        let bank = fixture();
        let mut rng = Rng::seeded(5);
        let used: HashSet<String> = ["guitar", "policecar", "mouse", "house"].iter().map(|w| w.to_string()).collect();
        for _ in 0..10 {
            assert_eq!(bank.pick(&used, &mut rng), bank.find("icecream"), "the only word not up lately");
        }
        // Once every word has been drawn the fortnight starts over.
        let all: HashSet<String> = (0..bank.word_count()).map(|i| bank.word(i).expect("a word").key()).collect();
        assert!(bank.pick(&all, &mut rng).is_some());
        // The same drawing of a word is never shown twice inside the window.
        let guitar = bank.find("guitar").expect("the guitar");
        let seen: HashSet<i64> = HashSet::from([0, 2]);
        for _ in 0..10 {
            assert_eq!(bank.pick_doodle(guitar, &seen, &mut rng), Some(1));
        }
        assert!(bank.pick_doodle(guitar, &HashSet::from([0, 1, 2]), &mut rng).is_some(), "and then starts over");
        assert_eq!(Bank::default().pick(&HashSet::new(), &mut rng), None);
    }

    #[test]
    fn a_pack_that_doesnt_match_its_manifest_is_refused() {
        let good = r#"{"format": "QDPACK1", "index": [{"word": "dot", "answers": ["dot"], "count": 1, "offset": 8, "bytes": 4}]}"#;
        let pack = [MAGIC.to_vec(), vec![1, 1, 5, 5]].concat();
        assert!(Bank::from_parts(good, pack.clone()).is_ok());
        // No magic, a slice that runs off the end, and a byte count that lies.
        assert!(Bank::from_parts(good, vec![0; 12]).is_err());
        assert!(Bank::from_parts(good, MAGIC.to_vec()).is_err());
        let long = r#"{"format": "QDPACK1", "index": [{"word": "dot", "answers": ["dot"], "count": 1, "offset": 8, "bytes": 9}]}"#;
        assert!(Bank::from_parts(long, pack.clone()).is_err());
        let two = r#"{"format": "QDPACK1", "index": [{"word": "dot", "answers": ["dot"], "count": 2, "offset": 8, "bytes": 4}]}"#;
        assert!(Bank::from_parts(two, pack.clone()).is_err());
        // And a format this reader doesn't know.
        let later = r#"{"format": "QDPACK9", "index": []}"#;
        assert!(Bank::from_parts(later, pack).is_err());
    }

    /// The shipped bank when it is there - it is data, kept beside the code
    /// rather than in it, so this test passes quietly on a checkout without it.
    #[test]
    fn the_shipped_bank_plays_if_it_is_there() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("drawbank");
        let Ok(bank) = load(&dir) else { return };
        assert!(bank.word_count() > 100, "only {} words", bank.word_count());
        assert!(bank.doodle_count() > 1_000, "only {} doodles", bank.doodle_count());
        let mut rng = Rng::seeded(2026);
        let mut seen = HashSet::new();
        for _ in 0..200 {
            let word = bank.pick(&seen, &mut rng).expect("a word");
            let entry = bank.word(word).expect("a word");
            assert!(bank.wins(word, &entry.word), "{} doesn't answer its own drawing", entry.word);
            assert!(entry.doodles() > 1, "{} has nothing to show for a hint", entry.word);
            let which = bank.pick_doodle(word, &HashSet::new(), &mut rng).expect("a drawing");
            let doodle = bank.doodle(word, which).expect("a drawing");
            // A drawing somebody really made: several strokes, and points in them.
            assert!((1..=25).contains(&doodle.len()), "{} has {} strokes", entry.word, doodle.len());
            let points: usize = doodle.iter().map(Vec::len).sum();
            assert!((12..=450).contains(&points), "{} has {} points", entry.word, points);
            seen.insert(entry.key());
        }
        // The two the bank's own guide warns about, on the real word list: a
        // part of an answer never wins, and a slip on a long word does.
        if let (Some(police), Some(car)) = (bank.find("policecar"), bank.find("car")) {
            assert!(!bank.wins(police, "car"), "a car is not a police car");
            assert!(bank.wins(police, "cop car") && bank.wins(car, "car"));
        }
        if let Some(guitar) = bank.find("guitar") {
            assert!(bank.wins(guitar, "gitar") && bank.wins(guitar, "Guitar!"));
        }
        // The licence's credit is the one the bank asks for.
        assert!(ATTRIBUTION.contains("Quick, Draw!") && ATTRIBUTION.contains("CC BY 4.0"));
    }

    #[test]
    fn a_missing_doodle_bank_is_a_warning_and_not_a_crash() {
        let dir = tempfile::tempdir().expect("a folder");
        assert!(load(dir.path()).is_err(), "nothing there at all");
        std::fs::write(dir.path().join("words.json"), r#"{"format": "QDPACK1", "index": []}"#).unwrap();
        assert!(load(dir.path()).is_err(), "no pack");
        std::fs::write(dir.path().join("doodles.bin"), MAGIC).unwrap();
        assert!(load(dir.path()).is_err(), "a manifest with no words in it is no bank");
        // And `open` on a workspace without one leaves the bank empty, which is
        // what every part of the game checks before it does anything.
        let empty = tempfile::tempdir().expect("a folder");
        open(&empty.path().display().to_string());
        assert!(bank().is_none());
    }
}
