//! The one riddle bank, and the one answer matcher, for everything that asks a
//! riddle: the ravens, the arena's scroll duels, and whatever comes next.
//!
//! There is only ever ONE bank. It is the JSONL files in
//! `<workspace>/riddlebank/ai/` - `desi.jsonl`, `paheli.jsonl`, `logic.jsonl`,
//! `nature.jsonl`, `objects.jsonl` - written in the server's own
//! desi/Hinglish voice, read into `frog.db` at startup by
//! [`super::frog_store::import_riddles`] and picked from there. A second bank
//! would mean the same riddle could be asked twice in one evening by two
//! different games, and the "already used" flag the bank keeps would stop
//! meaning anything.
//!
//! So this module is the door, not the cupboard. It holds no riddles of its
//! own: it opens the one store, picks, and hands the riddle back. Call it from
//! anywhere; it takes no connection and no lock of its own, and it is cheap
//! enough to call inside a command.
//!
//! The matcher is [`is_correct`], which lives in [`super::frog_answer`]: both
//! sides are folded to one plain form (lower case, accents folded, punctuation
//! gone, a leading "a"/"an"/"the"/"my" dropped, compared with and without
//! spaces) and then a spelling slip is forgiven on longer answers only. Every
//! riddle carries its own list of accepted answers, canonical first, so
//! "Hinglish or English" is a bank question rather than a code question.

use super::frog_store::{self as store, Rarity};

pub use super::frog_answer::{is_correct, levenshtein, matches_one, normalise};
pub use super::frog_store::Riddle;

/// How hard a riddle is. The bank's three levels, named rather than spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Difficulty {
    Easy,
    Medium,
    Hard,
}

impl Difficulty {
    pub const ALL: [Difficulty; 3] = [Difficulty::Easy, Difficulty::Medium, Difficulty::Hard];

    /// The word the bank files use.
    pub fn key(self) -> &'static str {
        match self {
            Difficulty::Easy => "easy",
            Difficulty::Medium => "medium",
            Difficulty::Hard => "hard",
        }
    }

    pub fn from_key(key: &str) -> Option<Difficulty> {
        Self::ALL.into_iter().find(|d| d.key().eq_ignore_ascii_case(key.trim()))
    }

    /// The difficulty a card of this rarity is guarded by: the rarer the card,
    /// the harder the scroll.
    pub fn for_rarity(rarity: Rarity) -> Difficulty {
        Difficulty::from_key(rarity.difficulty()).unwrap_or(Difficulty::Easy)
    }
}

/// One riddle at a difficulty, from the shared bank.
///
/// `roll` is a number in `[0, 1)`, passed in rather than drawn here so a caller
/// can reproduce a pick in a test. The bank prefers riddles it has not used
/// before and falls back to neighbouring difficulties when a level runs dry, so
/// this returns `None` only when the bank is empty or the store isn't open.
pub fn pick(difficulty: Difficulty, roll: f64) -> Option<Riddle> {
    let db = store::db()?;
    let conn = db.lock();
    store::pick_riddle(&conn, difficulty.key(), roll)
}

/// One riddle by its id - what to call when a round was started earlier and the
/// id is all that was kept.
pub fn get(id: &str) -> Option<Riddle> {
    let db = store::db()?;
    let conn = db.lock();
    store::riddle(&conn, id)
}

/// Takes a riddle out of play - a bad one, or one whose answer turned out to be
/// arguable. `true` when this call retired it.
pub fn retire(id: &str) -> bool {
    let Some(db) = store::db() else { return false };
    let conn = db.lock();
    store::set_retired(&conn, id, true).unwrap_or(false)
}

/// How many playable riddles there are at each level, for a panel page or a
/// startup line.
pub fn counts() -> Vec<(Difficulty, i64)> {
    let Some(db) = store::db() else { return Difficulty::ALL.into_iter().map(|d| (d, 0)).collect() };
    let conn = db.lock();
    Difficulty::ALL
        .into_iter()
        .map(|d| {
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM riddles WHERE difficulty = ?1 AND in_bank = 1 AND retired = 0",
                    rusqlite::params![d.key()],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            (d, n)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank of its own, so nothing here touches the live one.
    fn bank() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        store::init(&conn).expect("schema");
        for (id, difficulty, text, answers) in [
            ("e1", "easy", "Kaun hai jo subah uthta hai?", r#"["suraj","sun"]"#),
            ("e2", "easy", "Barf ka rang?", r#"["safed","white"]"#),
            ("m1", "medium", "Jitna lo utna badhta hai?", r#"["gaddha","hole"]"#),
            ("h1", "hard", "It has a bite and no teeth.", r#"["winter"]"#),
        ] {
            conn.execute(
                "INSERT INTO riddles (id, topic, difficulty, riddle, answers, hint) VALUES (?1, 'test', ?2, ?3, ?4, '')",
                rusqlite::params![id, difficulty, text, answers],
            )
            .expect("a riddle");
        }
        conn
    }

    #[test]
    fn the_levels_are_the_words_the_bank_files_use() {
        for level in Difficulty::ALL {
            assert_eq!(Difficulty::from_key(level.key()), Some(level));
            assert_eq!(Difficulty::from_key(&level.key().to_uppercase()), Some(level));
        }
        assert_eq!(Difficulty::from_key("impossible"), None);
        // And the deck's rarities land on them, hardest for the rare ones.
        assert_eq!(Difficulty::for_rarity(Rarity::Common), Difficulty::Easy);
        assert_eq!(Difficulty::for_rarity(Rarity::Uncommon), Difficulty::Medium);
        assert_eq!(Difficulty::for_rarity(Rarity::Rare), Difficulty::Hard);
        assert_eq!(Difficulty::for_rarity(Rarity::Legendary), Difficulty::Hard);
    }

    #[test]
    fn a_pick_comes_from_the_level_asked_for_and_the_same_roll_picks_the_same_one() {
        let conn = bank();
        let once = store::pick_riddle(&conn, "easy", 0.2).expect("an easy riddle");
        assert_eq!(once.difficulty, "easy");
        // The bank marks a riddle used so it isn't asked twice in a round. Put
        // that back and the same roll picks the same riddle again - which is
        // what makes a round reproducible in a test.
        conn.execute("UPDATE riddles SET used = 0", []).unwrap();
        assert_eq!(store::pick_riddle(&conn, "easy", 0.2).map(|r| r.id), Some(once.id.clone()), "same roll, same riddle");
        assert_eq!(store::pick_riddle(&conn, "hard", 0.5).map(|r| r.id), Some("h1".to_string()));
        // A level with nothing left hands over to the nearest one that has some
        // rather than refusing to ask anything at all.
        conn.execute("DELETE FROM riddles WHERE difficulty = 'hard'", []).unwrap();
        assert!(store::pick_riddle(&conn, "hard", 0.5).is_some(), "an empty level falls back");
    }

    /// The matcher is the whole reason there is one module: a second one would
    /// forgive different things.
    #[test]
    fn the_matcher_forgives_what_a_phone_does_and_nothing_more() {
        let answers = vec!["suraj".to_string(), "sun".to_string()];
        for typed in ["suraj", "SURAJ", "  Suraj  ", "the suraj", "sun", "Sun."] {
            assert!(is_correct(typed, &answers), "{:?} should be accepted", typed);
        }
        for typed in ["chand", "", "s"] {
            assert!(!is_correct(typed, &answers), "{:?} should not be accepted", typed);
        }
        // An apostrophe closes a word up rather than splitting it, and a
        // leading "the" is dropped: both sides land on the same plain form.
        assert_eq!(normalise("  The Night's Watch! "), "nights watch");
        assert!(matches_one("the nights watch", "Night's Watch"));
        assert_eq!(levenshtein("winter", "wnter"), 1);
    }

    /// The door has to answer whether or not the store behind it is open: in a
    /// test process it may or may not be, depending on what else has run, and a
    /// caller must survive either.
    #[test]
    fn the_door_answers_whether_or_not_the_store_is_open() {
        let _ = pick(Difficulty::Easy, 0.5);
        assert!(get("no-such-riddle-at-all").is_none(), "a riddle that doesn't exist is None, never a panic");
        assert!(!retire("no-such-riddle-at-all"), "and retiring one that doesn't exist does nothing");
        let counted = counts();
        assert_eq!(counted.len(), 3, "a number for each level, always");
        assert_eq!(counted.iter().map(|(d, _)| *d).collect::<Vec<_>>(), Difficulty::ALL.to_vec());
        assert!(counted.iter().all(|(_, n)| *n >= 0));
    }
}
