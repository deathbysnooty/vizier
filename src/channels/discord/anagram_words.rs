//! The Anagrams word bank: the words the bot scrambles, and the words it takes
//! as answers.
//!
//! Two plain files in `<workspace>/wordbank/`, one lowercase word per line, the
//! way the Chocolate Frog reads its riddles from `riddlebank/`:
//!
//! * `puzzles.txt` — the words worth setting. Nothing else is ever scrambled.
//! * `dictionary.txt` — every word that counts as an answer.
//!
//! An answer is any dictionary word made of EXACTLY the letters on the card, so
//! the letters of BEAST are solved by `beast`, `bates`, `betas` and `tabes`
//! alike: the bank is keyed by a word's sorted letters, and a guess is looked up
//! by its own.
//!
//! If the bank isn't there the game simply never starts - [`open`] says so in
//! the log and leaves [`bank`] empty, and every part of the game checks it
//! before doing anything.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use super::sudoku_gen::Rng;

/// The shortest and longest word the game will set.
pub const MIN_LETTERS: usize = 4;
pub const MAX_LETTERS: usize = 12;

/// Longer than this and a message can't be a guess at all.
const MAX_GUESS: usize = 64;

/// How many arrangements are tried before a word is given up as unscrambleable
/// (a word whose letters are all the same, say).
const SHUFFLES: usize = 60;

static BANK: OnceLock<Bank> = OnceLock::new();

/// A word's letters in order, which is what an anagram really is: `beast`,
/// `bates` and `tabes` all come to "abest".
pub fn key(word: &str) -> String {
    let mut letters: Vec<char> = word.chars().flat_map(char::to_lowercase).filter(|c| c.is_alphabetic()).collect();
    letters.sort_unstable();
    letters.into_iter().collect()
}

/// What someone typed, as a word to look up - or `None` when the message isn't
/// one word at all.
///
/// Case and the punctuation around a word are forgiven ("BEAST!", "*beast*",
/// " beast "), because people type in this channel like people. Anything with a
/// space or a punctuation mark INSIDE it is not one word, and is ignored.
pub fn tidy(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_GUESS {
        return None;
    }
    // `!hint` and `!skip` steer the round; they are never guesses, however
    // forgiving the rest of this is.
    if trimmed.starts_with('!') || trimmed.starts_with('/') {
        return None;
    }
    let core = trimmed.trim_matches(|c: char| !c.is_alphabetic());
    if core.is_empty() || !core.chars().all(char::is_alphabetic) {
        return None;
    }
    Some(core.to_lowercase())
}

/// The words in play.
#[derive(Debug, Default)]
pub struct Bank {
    /// The words that may be set, in the order the file listed them.
    puzzles: Vec<String>,
    /// Every accepted word, under its sorted letters.
    answers: HashMap<String, Vec<String>>,
}

/// One line of a bank file as a word, or nothing.
fn word_of(line: &str) -> Option<String> {
    let word = line.trim().to_lowercase();
    (!word.is_empty() && word.chars().all(|c| c.is_ascii_alphabetic())).then_some(word)
}

impl Bank {
    /// Reads the two files as they stand. Anything that isn't a plain word is
    /// skipped rather than refused: a bank with a stray blank line still works.
    pub fn from_text(puzzles: &str, dictionary: &str) -> Bank {
        let mut answers: HashMap<String, Vec<String>> = HashMap::new();
        let mut add = |word: String| {
            let list = answers.entry(key(&word)).or_default();
            if !list.contains(&word) {
                list.push(word);
            }
        };
        for word in dictionary.lines().filter_map(word_of) {
            add(word);
        }
        let mut seen = HashSet::new();
        let mut words = Vec::new();
        for word in puzzles.lines().filter_map(word_of) {
            let length = word.chars().count();
            if !(MIN_LETTERS..=MAX_LETTERS).contains(&length) || !seen.insert(word.clone()) {
                continue;
            }
            // A word the bot sets is always an answer to its own letters, even
            // if the dictionary forgot it.
            add(word.clone());
            words.push(word);
        }
        Bank { puzzles: words, answers }
    }

    /// How many words can be set, and how many words are accepted in all.
    pub fn puzzle_count(&self) -> usize {
        self.puzzles.len()
    }

    pub fn word_count(&self) -> usize {
        self.answers.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.puzzles.is_empty()
    }

    /// Every word that uses exactly these letters, the set word included.
    pub fn answers_for(&self, letters: &str) -> &[String] {
        self.answers.get(letters).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether a guess wins the round: a dictionary word using ALL the letters
    /// and no others.
    pub fn accepts(&self, letters: &str, guess: &str) -> bool {
        let guess = guess.to_lowercase();
        key(&guess) == letters && self.answers_for(letters).iter().any(|w| *w == guess)
    }

    /// A word to set: one whose letters haven't been used lately. When every
    /// word has been used it starts over rather than leaving the channel empty.
    pub fn pick(&self, used: &HashSet<String>, rng: &mut Rng) -> Option<&str> {
        if self.puzzles.is_empty() {
            return None;
        }
        let start = rng.below(self.puzzles.len());
        let fresh = (0..self.puzzles.len())
            .map(|step| &self.puzzles[(start + step) % self.puzzles.len()])
            .find(|word| !used.contains(&key(word)));
        Some(fresh.unwrap_or(&self.puzzles[start]).as_str())
    }

    /// A word to set with an arrangement to show, trying other words when one
    /// can't be scrambled into anything that isn't a give-away.
    pub fn pick_scrambled(&self, used: &HashSet<String>, rng: &mut Rng) -> Option<(String, String)> {
        for _ in 0..20 {
            let word = self.pick(used, rng)?.to_string();
            if let Some(scramble) = self.scramble(&word, rng) {
                return Some((word, scramble));
            }
        }
        None
    }

    /// The letters shuffled into something that gives nothing away, or `None`
    /// for a word there is nothing to rearrange in ("aaaa").
    pub fn scramble(&self, word: &str, rng: &mut Rng) -> Option<String> {
        let letters: Vec<char> = word.chars().collect();
        let is_answer = |candidate: &str| self.accepts(&key(word), candidate);
        for _ in 0..SHUFFLES {
            let mut shuffled = letters.clone();
            rng.shuffle(&mut shuffled);
            let candidate: String = shuffled.into_iter().collect();
            if !obvious(word, &candidate, is_answer) {
                return Some(candidate);
            }
        }
        None
    }
}

/// Whether an arrangement gives the game away: the word itself, the word
/// backwards, another word anyone could simply read off the card, or one that
/// leaves most of the letters where they were.
pub fn obvious(word: &str, candidate: &str, is_answer: impl Fn(&str) -> bool) -> bool {
    if candidate == word || candidate.chars().rev().collect::<String>() == word {
        return true;
    }
    if is_answer(candidate) {
        return true;
    }
    let letters: Vec<char> = word.chars().collect();
    let shown: Vec<char> = candidate.chars().collect();
    if letters.len() != shown.len() {
        return true;
    }
    let kept = letters.iter().zip(&shown).filter(|(a, b)| a == b).count();
    kept * 2 > letters.len()
}

/// Reads the bank out of a folder. The error says which file is missing or
/// unreadable, so the log tells a mod what to do about it.
pub fn load(dir: &Path) -> anyhow::Result<Bank> {
    let read = |name: &str| -> anyhow::Result<String> {
        std::fs::read_to_string(dir.join(name)).map_err(|err| anyhow::anyhow!("{}: {}", dir.join(name).display(), err))
    };
    let bank = Bank::from_text(&read("puzzles.txt")?, &read("dictionary.txt")?);
    if bank.is_empty() {
        anyhow::bail!("{} has no words in it", dir.join("puzzles.txt").display());
    }
    Ok(bank)
}

/// Reads `<workspace>/wordbank/` once, at start. A bank that isn't there is not
/// an error worth stopping for: the game stays off and says so.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["wordbank"]);
    match load(&dir) {
        Ok(bank) => {
            tracing::info!("anagram: {} words to set, {} accepted, read from {}", bank.puzzle_count(), bank.word_count(), dir.display());
            let _ = BANK.set(bank);
        }
        Err(err) => tracing::warn!(
            "anagram: no word bank ({}) — the game stays off. Copy wordbank/ with puzzles.txt and dictionary.txt into the workspace.",
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

    /// A bank small enough to read, with everything the game can meet in it:
    /// sets of letters several words share, and a word with only one anagram.
    pub const PUZZLES: &str = "beast\nlisten\nteaching\nplanet\n";
    pub const DICTIONARY: &str = "beast\nbates\nbetas\ntabes\nbeats\nlisten\nsilent\ntinsel\nenlist\nteaching\ncheating\nplanet\nplaten\nsees\nbeat\nbest\nstab\nlist\nplan\n";

    pub fn fixture() -> Bank {
        Bank::from_text(PUZZLES, DICTIONARY)
    }

    #[test]
    fn a_bank_is_two_plain_files_and_skips_anything_that_isnt_a_word() {
        let bank = Bank::from_text("beast\n\n  LISTEN \nab\nhippopotamuses\nbeast\n", DICTIONARY);
        // "ab" is too short, the long one too long, the blank line nothing, and
        // the same word twice is one word.
        assert_eq!(bank.puzzle_count(), 2);
        assert!(bank.accepts(&key("beast"), "bates"));
        assert!(bank.accepts(&key("listen"), "silent"), "the upper-case line is read as a word");
    }

    #[test]
    fn any_word_that_uses_all_the_letters_wins_and_nothing_else_does() {
        let bank = fixture();
        let letters = key("beast");
        for answer in ["beast", "bates", "betas", "tabes", "beats"] {
            assert!(bank.accepts(&letters, answer), "{} uses exactly those letters", answer);
        }
        // Case and the punctuation around a word are forgiven.
        assert!(bank.accepts(&letters, &tidy("  Bates! ").unwrap()));
        assert!(bank.accepts(&letters, &tidy("**BEAST**").unwrap()));
        // Different letters, only some of them, and some of them twice.
        assert!(!bank.accepts(&letters, "least"), "one letter is not among them");
        assert!(!bank.accepts(&letters, "beat"), "a word that leaves a letter over");
        assert!(!bank.accepts(&letters, "beasts"), "a word with a letter too many");
        assert!(!bank.accepts(&letters, "abest"), "the right letters, but not a word");
        assert!(!bank.accepts(&letters, ""), "nothing at all");
        assert_eq!(bank.answers_for(&key("teaching")).len(), 2);
        assert!(bank.answers_for("zzzz").is_empty());
    }

    #[test]
    fn a_message_that_isnt_one_word_is_not_a_guess() {
        assert_eq!(tidy("Beast"), Some("beast".into()));
        assert_eq!(tidy("  beast?  "), Some("beast".into()));
        assert_eq!(tidy("\"beast\""), Some("beast".into()));
        assert_eq!(tidy("is it beast"), None, "a sentence is not a guess");
        assert_eq!(tidy("be-ast"), None, "punctuation inside a word is not forgiven");
        assert_eq!(tidy("!hint"), None);
        assert_eq!(tidy("😀"), None);
        assert_eq!(tidy("   "), None);
        assert_eq!(tidy(&"a".repeat(MAX_GUESS + 1)), None, "an essay is not a guess");
    }

    #[test]
    fn a_scramble_is_never_the_word_and_never_a_give_away() {
        let bank = fixture();
        let mut rng = Rng::seeded(7);
        for word in ["beast", "listen", "teaching", "planet"] {
            for _ in 0..40 {
                let scramble = bank.scramble(word, &mut rng).expect("a scramble");
                assert_eq!(key(&scramble), key(word), "the same letters, rearranged");
                assert_ne!(scramble, word, "the word itself is never shown");
                assert_ne!(scramble.chars().rev().collect::<String>(), word, "nor the word backwards");
                assert!(!bank.accepts(&key(word), &scramble), "nor another answer, which anyone could read off");
                let kept = word.chars().zip(scramble.chars()).filter(|(a, b)| a == b).count();
                assert!(kept * 2 <= word.chars().count(), "most letters must move: {} -> {}", word, scramble);
            }
        }
        // A word with nothing to rearrange is handed back, not forced up as a
        // round that shows itself.
        assert_eq!(bank.scramble("aaaa", &mut rng), None);
    }

    #[test]
    fn what_counts_as_a_give_away() {
        let answer = |w: &str| w == "beast" || w == "bates";
        assert!(obvious("beast", "beast", answer), "the word itself");
        assert!(obvious("beast", "tsaeb", answer), "the word backwards");
        assert!(obvious("beast", "bates", answer), "another word, read straight off the card");
        assert!(obvious("beast", "beats", |_| false), "four letters of five left where they were");
        assert!(!obvious("beast", "tsabe", |_| false));
    }

    #[test]
    fn a_word_is_picked_without_repeating_the_letters_used_lately() {
        let bank = fixture();
        let mut rng = Rng::seeded(3);
        let mut used: HashSet<String> = HashSet::new();
        // Everything but one set of letters has been used this month.
        for word in ["beast", "listen", "planet"] {
            used.insert(key(word));
        }
        for _ in 0..10 {
            assert_eq!(bank.pick(&used, &mut rng), Some("teaching"));
        }
        // Once everything has come up, the month starts over rather than the
        // channel sitting empty.
        used.insert(key("teaching"));
        assert!(bank.pick(&used, &mut rng).is_some());
        // The letters, not the word: another word with the same letters is also
        // held back.
        let only = HashSet::from([key("silent")]);
        let bank = Bank::from_text("listen\n", DICTIONARY);
        assert_eq!(bank.pick(&only, &mut rng), Some("listen"), "nothing else to set");
        assert_eq!(bank.pick(&HashSet::new(), &mut rng), Some("listen"));
        // And a word that can't be scrambled is passed over for one that can.
        let awkward = Bank::from_text("aaaa\nbeast\n", DICTIONARY);
        let (word, scramble) = awkward.pick_scrambled(&HashSet::new(), &mut rng).expect("a round");
        assert_eq!((word.as_str(), key(&scramble).as_str()), ("beast", "abest"));
        // Nothing to set at all.
        assert_eq!(Bank::default().pick_scrambled(&HashSet::new(), &mut rng), None);
    }

    /// The shipped bank when it is there — it is data, kept beside the code
    /// rather than in it, so this test passes quietly on a checkout without it.
    #[test]
    fn the_shipped_bank_plays_if_it_is_there() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("wordbank");
        let Ok(bank) = load(&dir) else { return };
        assert!(bank.puzzle_count() > 100, "only {} words to set", bank.puzzle_count());
        let mut rng = Rng::seeded(2026);
        let mut set = HashSet::new();
        for _ in 0..300 {
            let (word, scramble) = bank.pick_scrambled(&set, &mut rng).expect("a round");
            assert!((MIN_LETTERS..=MAX_LETTERS).contains(&word.chars().count()), "{} is an odd length", word);
            assert!(bank.accepts(&key(&word), &word), "{} doesn't answer its own letters", word);
            assert!(!bank.accepts(&key(&word), &scramble), "{} shows an answer on the card", scramble);
            assert!(!obvious(&word, &scramble, |c| bank.accepts(&key(&word), c)), "{} -> {} gives it away", word, scramble);
            set.insert(key(&word));
        }
    }

    #[test]
    fn a_missing_word_bank_is_a_warning_and_not_a_crash() {
        let dir = tempfile::tempdir().expect("a folder");
        assert!(load(dir.path()).is_err(), "nothing there at all");
        std::fs::write(dir.path().join("puzzles.txt"), "beast\n").unwrap();
        assert!(load(dir.path()).is_err(), "no dictionary");
        std::fs::write(dir.path().join("dictionary.txt"), DICTIONARY).unwrap();
        let bank = load(dir.path()).expect("a bank");
        assert_eq!(bank.puzzle_count(), 1);
        assert!(bank.accepts(&key("beast"), "tabes"));
        // A bank whose puzzle file has nothing usable in it is no bank at all.
        std::fs::write(dir.path().join("puzzles.txt"), "\n# nothing here\n").unwrap();
        assert!(load(dir.path()).is_err());
        // And `open` on a workspace without one leaves the bank empty.
        let empty = tempfile::tempdir().expect("a folder");
        open(&empty.path().display().to_string());
        assert!(bank_missing_means_off());
    }

    /// The game asks this before it does anything; with no bank set in a test
    /// run it is always true.
    fn bank_missing_means_off() -> bool {
        bank().is_none()
    }
}
