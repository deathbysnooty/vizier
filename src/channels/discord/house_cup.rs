//! One switch for the whole House Cup.
//!
//! September's Cup is over. The games all keep running and keep paying their
//! OWN scores - anagram points, guess points, sudoku points, chess points and
//! the rest - but the Cup itself is parked: no house points are written, no
//! Chocolate Frogs and no Snitches drop, and the games stop talking about
//! houses altogether until it is switched back on.
//!
//! `VIZIER_HOUSE_CUP` is the only setting any of that reads, and it defaults to
//! ON, so a server that never touches it sees no change whatever.
//!
//! What pausing does NOT touch: whatever is already in the ledger (September's
//! history stays readable), the cards people already own, trading them, looking
//! at a collection, and the sorting itself - everyone keeps their house, they
//! just stop hearing about it mid-game.
//!
//! Reads go through [`running`] at the moment they are needed, so flipping the
//! switch on the panel takes effect without a restart. The copy is threaded a
//! plain `bool` rather than reading the switch itself, so the wording tests can
//! ask for either state without touching process-wide settings.

use std::collections::HashSet;
use std::sync::LazyLock;

use parking_lot::Mutex;

use super::control;

/// The one setting. On/off, default on.
pub const KEY: &str = "VIZIER_HOUSE_CUP";

/// True while the House Cup is running - points, drops and all the house
/// wording. This is the default, so nothing changes for anyone who leaves the
/// setting alone.
pub fn running() -> bool {
    // Written out rather than read through `KEY` so the catalog's key-scan can
    // see it: that test reads the source for `control::on("VIZIER_…")` literals
    // and would otherwise call this setting described but never read.
    control::on("VIZIER_HOUSE_CUP", true)
}

/// True while the Cup is paused.
pub fn paused() -> bool {
    !running()
}

/// Paths that have already said they were skipped, so a paused month logs once
/// per path rather than once per event. Cleared the moment the Cup is running
/// again, so switching off a second time is logged again.
static NOTED: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// The gate every paused path uses: `true` to carry on, `false` to skip.
///
/// A skip is logged the FIRST time each `path` is reached and then stays quiet,
/// which is the difference between one line a month and one line per solve.
pub fn gate(path: &'static str) -> bool {
    if running() {
        let mut noted = NOTED.lock();
        if !noted.is_empty() {
            noted.clear();
        }
        return true;
    }
    if NOTED.lock().insert(path) {
        tracing::info!("housecup: paused, so nothing was paid or dropped for `{}` (said once, not per event)", path);
    }
    false
}

/// Said once at startup so the log makes plain which month this is.
pub fn log_state() {
    if running() {
        tracing::info!("housecup: running - house points, Chocolate Frog drops and Snitch drops are all live");
    } else {
        tracing::info!(
            "housecup: PAUSED ({}=off) - the games keep their own points, but no house points are written, \
             no frogs or Snitches drop, and the games say nothing about houses",
            KEY
        );
    }
}

// --- what people are told -------------------------------------------------------

/// The one sentence appended wherever a house command still answers.
pub const NOTE: &str = "-# The House Cup is paused. The games all still run and still keep their own scores.";

/// A mod trying to move house points while the Cup is paused.
pub const MOD_REFUSED: &str =
    "🏆 The House Cup is paused, so there are no house points to give or take. The games all still run and still pay their own points. \
     Switch **House Cup** back on in the panel and this works again.";

/// A mod asking for a manual drop while the Cup is paused. `what` is "a
/// Chocolate Frog" or "a Snitch".
pub fn drop_refused(what: &str) -> String {
    format!(
        "🏆 The House Cup is paused, so {} won't drop. Nobody's cards are touched - `/frogs`, `/frogcard` and trading all still work, \
         and the collections carry into next month. Switch **House Cup** back on in the panel to drop again.",
        what
    )
}

/// The heading a house command shows while paused: the month's numbers are a
/// closed result, not a live table.
pub fn closed_heading(month: &str) -> String {
    format!("🏆 **The House Cup is paused** · {} was the last month played, and these are its final numbers.", month)
}

/// Holding the switch one way or the other for the length of a test.
///
/// Every test that reads or writes the switch takes the same lock, so two of
/// them can't run over each other, and the value is put back when the test ends.
/// It writes into the settings cache rather than the environment, which is the
/// same lock a live read takes - no `set_var`, no undefined behaviour.
#[cfg(test)]
pub(super) mod testing {
    use std::sync::{Mutex, MutexGuard};

    static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) struct Switch(#[allow(dead_code)] MutexGuard<'static, ()>);

    impl Switch {
        /// The Cup paused for the length of the test.
        pub(crate) fn paused() -> Switch {
            let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let switch = Switch(guard);
            switch.set("off");
            switch
        }

        /// The Cup running for the length of the test - the shipped default, held
        /// explicitly so a test that asserts on it can't be raced.
        pub(crate) fn running() -> Switch {
            let guard = SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let switch = Switch(guard);
            switch.set("on");
            switch
        }

        /// Flips it mid-test, which is how "switching it back on restores
        /// everything" is exercised.
        pub(crate) fn set(&self, value: &str) {
            super::super::control::set_for_test(super::KEY, Some(value));
            super::NOTED.lock().clear();
        }
    }

    impl Drop for Switch {
        fn drop(&mut self) {
            super::super::control::set_for_test(super::KEY, None);
            super::NOTED.lock().clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::super::house::{self, HOUSES};
    use super::super::points::{self as ledger, Outcome, Source};
    use super::testing::Switch;
    use super::*;

    /// 2026-09-14 12:00 India time.
    const SEPT: i64 = 1_789_367_400;

    /// A ledger of its own, so nothing here can touch the real one.
    fn ledger() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        conn.execute_batch(ledger::SCHEMA).expect("ledger schema");
        conn
    }

    fn rows(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM ledger", [], |r| r.get(0)).unwrap_or(-1)
    }

    fn total(conn: &Connection) -> i64 {
        conn.query_row("SELECT COALESCE(SUM(points), 0) FROM ledger", [], |r| r.get(0)).unwrap_or(-1)
    }

    // --- the switch itself ----------------------------------------------------

    /// The switch defaults ON: a server that never touches it sees no change.
    #[test]
    fn the_cup_runs_unless_someone_turns_it_off() {
        let switch = Switch::running();
        assert_eq!(KEY, "VIZIER_HOUSE_CUP", "the key `running` reads must be the one the panel writes");
        assert!(running() && !paused(), "on means running");
        switch.set("off");
        assert!(paused() && !running(), "off means paused");
        // Every spelling the panel and the environment can produce.
        for on in ["on", "true", "yes", "1", "ON", "Yes"] {
            switch.set(on);
            assert!(running(), "{:?} should read as running", on);
        }
        for off in ["off", "false", "no", "0", "OFF", "No"] {
            switch.set(off);
            assert!(paused(), "{:?} should read as paused", off);
        }
        // Anything the panel could never write falls back to the default, which
        // is ON - a typo must never silently pause the Cup.
        switch.set("maybe");
        assert!(running(), "an unreadable value must default to running");
        drop(switch);
        let _guard = Switch::running();
        super::super::control::set_for_test(KEY, None);
        assert!(running(), "with nothing set at all the Cup runs");
    }

    #[test]
    fn the_gate_is_open_while_the_cup_runs_and_shut_while_it_is_paused() {
        let switch = Switch::running();
        for path in ["anagram", "snitch", "frog", "/modgive"] {
            assert!(gate(path), "{} should be open while the Cup runs", path);
        }
        switch.set("off");
        for path in ["anagram", "snitch", "frog", "/modgive"] {
            assert!(!gate(path), "{} should be shut while the Cup is paused", path);
        }
    }

    /// A paused path says so once, not once per solve - and says so again if the
    /// Cup is switched on and off a second time.
    #[test]
    fn a_paused_path_is_noted_once_and_not_once_per_event() {
        let switch = Switch::paused();
        assert!(NOTED.lock().is_empty(), "nothing noted yet");
        for _ in 0..500 {
            assert!(!gate("anagram"));
        }
        assert_eq!(NOTED.lock().len(), 1, "one line for five hundred solves");
        assert!(!gate("snitch"));
        assert_eq!(NOTED.lock().len(), 2, "a different path gets one line of its own");
        // Back on: the slate is wiped, so a second pause is logged again.
        switch.set("on");
        assert!(gate("anagram"));
        assert!(NOTED.lock().is_empty(), "the notes are dropped when the Cup comes back");
        switch.set("off");
        assert!(!gate("anagram"));
        assert_eq!(NOTED.lock().len(), 1, "the second pause says so again");
    }

    // --- nothing is awarded, and nothing is written ---------------------------

    /// Every source the ledger has, through the one door they all use. Paused,
    /// not one of them writes a row - not even a nought.
    #[test]
    fn no_source_writes_anything_while_the_cup_is_paused() {
        let conn = ledger();
        let house = &HOUSES[0];
        for (i, source) in Source::ALL.into_iter().enumerate() {
            let paid = house::award_into(
                &conn,
                false,
                100 + i as u64,
                house,
                source,
                5,
                "a round",
                None,
                Some(format!("paused:{}", source.key())),
                None,
                SEPT,
            );
            assert!(paid.is_none(), "{} should pay nothing while paused", source.key());
        }
        assert_eq!(rows(&conn), 0, "the paused month must leave the ledger untouched");
    }

    /// And with it back on, the very same call pays again - the same amount, the
    /// same row. This is the "switching it back restores everything" check.
    #[test]
    fn switching_it_back_on_pays_a_round_again() {
        let conn = ledger();
        let house = &HOUSES[0];
        let round = |cup: bool| {
            house::award_into(&conn, cup, 7, house, Source::Anagram, 2, "Anagrams: round 1", None, Some("anagram:1".into()), None, SEPT)
        };
        assert!(round(false).is_none(), "paused, the round pays nothing");
        assert_eq!(rows(&conn), 0);
        assert_eq!(round(true), Some(Outcome::Granted(2)), "back on, the round pays as it always did");
        assert_eq!((rows(&conn), total(&conn)), (1, 2));
        // The caps and the dedupe key still work exactly as before.
        assert_eq!(round(true), Some(Outcome::Duplicate), "and the same round still can't score twice");
    }

    /// September's rows are read-only history: a paused month adds nothing to
    /// them and takes nothing away.
    #[test]
    fn what_is_already_in_the_ledger_survives_a_paused_month() {
        let conn = ledger();
        let house = &HOUSES[2];
        for (user, points) in [(1u64, 6i64), (2, 4)] {
            house::award_into(&conn, true, user, house, Source::Quiz, points, "September", None, Some(format!("sept:{}", user)), None, SEPT);
        }
        let before = (rows(&conn), total(&conn));
        assert_eq!(before, (2, 10), "September as it was played");
        // A whole month of play, paused.
        for i in 0..50 {
            house::award_into(&conn, false, 3, house, Source::Guess, 2, "October", None, Some(format!("oct:{}", i)), None, SEPT + 30 * 86_400);
        }
        assert_eq!((rows(&conn), total(&conn)), before, "the history is exactly as it was");
        assert_eq!(ledger::house_total(&conn, house.key, 0, i64::MAX).unwrap(), 10, "and it still reads back");
    }

    // --- the wording, everywhere ---------------------------------------------

    /// What no user-facing string in a game path may contain while paused.
    /// "house point" is the owner's own words; the rest are the ways a house
    /// gets named without those two words.
    fn houseless(what: &str, text: &str) {
        let low = text.to_lowercase();
        for banned in ["house point", "house cup", "housepoints", "housecards", "housemate", "muggle"] {
            assert!(!low.contains(banned), "{} still says {:?}:\n{}", what, banned, text);
        }
        for house in HOUSES {
            assert!(!text.contains(house.name), "{} still names {}:\n{}", what, house.name, text);
            assert!(!text.contains(house.crest), "{} still draws {}'s crest:\n{}", what, house.name, text);
        }
        // "house" on its own, as a word rather than inside "household".
        for word in low.split(|c: char| !c.is_ascii_alphabetic()) {
            assert!(word != "house" && word != "houses", "{} still says \"house\":\n{}", what, text);
        }
    }

    /// The round cards and win lines of the word games: a solve says its OWN
    /// points and nothing else. "won by X for 2 house points (2 anagram points)"
    /// becomes the anagram points on their own.
    #[test]
    fn a_won_round_says_only_the_games_own_points() {
        use super::super::{anagram, guess, movie};

        let won = anagram::Won {
            round: 12,
            winner: 7,
            badge: String::new(),
            word: "beast".into(),
            set_word: "beats".into(),
            points: 0,
            worth: 2,
            full_points: 2,
            housed: true,
            tally: 9,
            hinted: false,
            seconds: 20,
            cup: false,
        };
        let text = anagram::won_text(&won);
        houseless("the anagram win line", &text);
        assert!(text.contains("+2 anagram points"), "it should say the anagram points: {}", text);
        assert!(text.contains("9 today"), "and the day's tally: {}", text);
        // With the Cup running the same round still names the house points.
        let paying = anagram::won_text(&anagram::Won { points: 2, cup: true, ..won });
        assert!(paying.contains("9 anagram points today"), "{}", paying);

        let g = guess::Won {
            round: 3,
            winner: 7,
            badge: String::new(),
            guess: "icecream".into(),
            word: "ice cream".into(),
            points: 0,
            worth: 2,
            full_points: 2,
            housed: true,
            tally: 4,
            hinted: false,
            seconds: 11,
            cup: false,
        };
        houseless("the guess win line", &guess::won_text(&g));
        assert!(guess::won_text(&g).contains("+2 guess points"), "{}", guess::won_text(&g));

        let m = movie::Won {
            round: 5,
            winner: 7,
            badge: String::new(),
            guess: "ddlj".into(),
            title: "Dilwale Dulhania Le Jayenge".into(),
            year: 1995,
            points: 0,
            worth: 3,
            full_points: 3,
            housed: true,
            tally: 6,
            hinted: false,
            seconds: 30,
            cup: false,
        };
        houseless("the movie win line", &movie::won_text(&m));
        assert!(movie::won_text(&m).contains("+3 movie points"), "{}", movie::won_text(&m));
    }

    /// Each game's own leaderboard, paused.
    #[test]
    fn the_game_boards_say_nothing_about_houses() {
        use super::super::{anagram, anagram_store, guess, guess_store, movie, movie_store, sudoku, sudoku_store};
        let a = [anagram_store::Tally { user: 7, points: 9, solves: 4, reached: 9 }];
        let g = [guess_store::Tally { user: 7, points: 9, solves: 4, reached: 9 }];
        let m = [movie_store::Tally { user: 7, points: 9, solves: 4, reached: 9 }];
        let sd = [sudoku_store::Tally { user: 7, points: 9, solves: 4, reached: 9 }];
        houseless("the anagram board", &anagram::top_text("today", &a, 7, false));
        houseless("the guess board", &guess::top_text("today", &g, 7, false));
        houseless("the movie board", &movie::top_text("today", &m, 7, false));
        houseless("the sudoku board", &sudoku::top_text("today", &sd, 7, false));
        // An empty board is just as quiet.
        houseless("an empty anagram board", &anagram::top_text("today", &[], 1, false));
        // And with the Cup on they still explain the daily limit.
        assert!(anagram::top_text("today", &a, 7, true).to_lowercase().contains("house-points limit"));
        assert!(sudoku::top_text("today", &sd, 7, true).contains("House Cup"));
    }

    /// Name Place Animal Thing: the lobby, the letter card and the final card.
    #[test]
    fn name_place_animal_thing_says_nothing_about_houses_while_paused() {
        use super::super::npat;
        use super::super::rules_text::tests::npat_defaults;
        use super::super::rules_text::NpatRules;

        // `stopped_line` and the review help read the live switch rather than a
        // parameter, so the test holds it.
        let _switch = Switch::paused();
        let paused = NpatRules { cup: false, prizes: [0, 0], min_houses: 1, ..npat_defaults() };
        for state in [npat::LobbyState::Idle, npat::LobbyState::Open { closes_at: 200 }] {
            houseless("the NPAT lobby", &npat::lobby_text(&[(1, "gryffindor"), (2, "slytherin")], state, &paused, 100));
        }
        let (_, letters, _) = npat::letter_results_text(1, 2, 5, 'P', &[], false, npat::points(), false);
        houseless("the NPAT letter card", &letters);
        let lines = [npat::FinalLine { user: 1, badge: String::new(), total: 30, rank: 1, prize: 0 }];
        let (_, body, footer) = npat::final_results_text(1, &['P', 'M'], &lines, true, false, &paused);
        houseless("the NPAT final card", &body);
        houseless("the NPAT final footer", &footer);
        houseless("a stopped NPAT round", npat::stopped_line());
        // The prize words disappear rather than reading "no house points".
        assert_eq!(npat::prize_words([2, 1], false), "");
        assert_eq!(npat::prize_words([2, 1], true), "🥇 +2 🥈 +1 house points");
        // With the Cup on, the house side is back exactly as it was.
        let running = NpatRules { cup: true, ..npat_defaults() };
        let (_, live, _) = npat::final_results_text(1, &['P'], &[npat::FinalLine { prize: 2, ..lines[0].clone() }], true, false, &running);
        assert!(live.contains("🏠 **House points:**"), "{}", live);
    }

    /// Every per-game help card.
    #[test]
    fn the_help_cards_say_nothing_about_houses_while_paused() {
        use super::super::rules_text::{self, tests as fixtures};
        houseless("/anagramhelp", &rules_text::anagram_help_text(&fixtures::anagram_defaults(), false));
        houseless("/guesshelp", &rules_text::guess_help_text(&fixtures::guess_defaults(), false));
        houseless("/moviehelp", &rules_text::movie_help_text(&fixtures::movie_defaults(), false));
        houseless("/geohelp", &rules_text::geo_help_text(&fixtures::geo_defaults(), false));
        houseless("/sudokuhelp", &rules_text::sudoku_help_text(&fixtures::sudoku_defaults(), false));
        houseless("/chesshelp", &rules_text::chess_help_text(&fixtures::chess_defaults(), false));
        houseless("/puzzlehelp", &rules_text::puzzle_help_text(&fixtures::puzzle_defaults(), false));
        // The sudoku rules post is the help plus the small print, so it follows.
        houseless("the sudoku rules post", &rules_text::sudoku_rules_text(&fixtures::sudoku_defaults(), false));
        // And with the Cup on every one of them still describes the house points.
        assert!(rules_text::anagram_help_text(&fixtures::anagram_defaults(), true).contains("**🏠 House points**"));
        assert!(rules_text::guess_help_text(&fixtures::guess_defaults(), true).contains("**🏠 House points**"));
    }

    /// The House Cup posts: the welcome, the beginner's guide and the "how to
    /// earn" card. None of them may advertise a point that isn't being paid.
    #[test]
    fn the_house_cup_posts_say_the_cup_is_paused_rather_than_promising_points() {
        use super::super::rules_text::{self, tests as fixtures};
        let paused = rules_text::Rules { cup: false, ..fixtures::defaults() };

        let welcome = rules_text::welcome_text(&paused);
        assert!(welcome.contains("The House Cup is paused"), "{}", welcome);
        assert!(!welcome.to_lowercase().contains("house point") || welcome.contains("No house points are being awarded"), "{}", welcome);

        // This card is one of the Cup's own posts, so it MAY say the Cup is
        // paused - what it must not do is advertise a point nobody is paid.
        let earn = rules_text::earn_text(&paused);
        assert!(earn.contains("The House Cup is paused"), "{}", earn);
        assert!(!earn.to_lowercase().contains("house point"), "nothing may promise a house point: {}", earn);
        for house in HOUSES {
            assert!(!earn.contains(house.name) && !earn.contains(house.crest), "no house belongs on this card: {}", earn);
        }
        assert!(earn.contains("`/anagramtop`") && earn.contains("`/sudokutop`"), "it should point at the game boards: {}", earn);

        let guide = rules_text::guide(&paused, false);
        assert!(guide[0].body.contains("The House Cup is paused"), "{}", guide[0].body);
        assert!(guide[0].body.contains("anagram points"), "the games' own scores are what is left: {}", guide[0].body);
        // The games are still listed - they are all still running.
        let games = guide.iter().find(|p| p.title.contains("Play the games")).expect("the games panel");
        assert!(games.body.contains("Sudoku") && games.body.contains("Chess"), "{}", games.body);
        for panel in &guide {
            assert!(!panel.body.contains("wins the **House Cup**"), "nothing may promise the Cup: {}", panel.body);
        }

        // The Snitch and frog post promises no drops, and promises the cards.
        let cards = rules_text::snitch_cards_text(&paused).expect("both are switched on");
        assert!(cards.contains("nothing is dropping"), "{}", cards);
        assert!(cards.contains("still theirs"), "the cards must be promised: {}", cards);
        assert!(!cards.contains("`/sellset`"), "a set is worth nothing, so it is not offered: {}", cards);
    }

    /// Letter Duel, Geo and the chess puzzle.
    #[test]
    fn the_other_games_say_nothing_about_houses_while_paused() {
        use super::super::{geo, puzzle};
        // Geo pays nothing, so the line is the geo points on their own.
        let result = geo::MatchResult { id: 1, rounds: 5, places: vec![(7, 12, 1, 0), (8, 9, 2, 0)] };
        houseless("the geo match line", &geo::result_line(&result));
        // The chess puzzle's card names only the puzzle points.
        let live = puzzle::Live {
            puzzle_id: 1,
            bank_id: "abc".into(),
            band: "easy".into(),
            rating: 900,
            solver_is_white: true,
            setup_san: "Qh5".into(),
            solver_moves: 2,
            worth: 3,
            // Nought is what a paused Cup leaves here, the same as a limit of nought.
            first_point: 0,
            first_solver: None,
            first_seconds: 0,
            since_first: 0,
            playing: 0,
            next_in: None,
        };
        houseless("the puzzle worth line", &puzzle::worth_words(&live));
        let attempt = puzzle::Attempt {
            ok: true,
            solved: true,
            first: true,
            san: "Qxf7#".into(),
            reply: String::new(),
            fen: String::new(),
            last: String::new(),
            check: String::new(),
            done: 2,
            total: 2,
            words: String::new(),
            // Nought paid: the Cup is paused, so the line is puzzle points only.
            points: 0,
            worth: 3,
            tally: 3,
        };
        houseless("the puzzle solved line", &puzzle::solved_words(&attempt));
    }

    /// The whole point of the pause: each game's OWN score is untouched. A round
    /// is recorded, scored and shown at its full value with no house point in
    /// sight - which is also what the win line above says.
    #[test]
    fn a_games_own_points_are_still_scored_and_still_paid_while_paused() {
        use super::super::anagram_store as store;
        let conn = store::tests::memory();
        let round = store::tests::put(&conn, "beast", "tsabe", 2, 1_000);
        assert!(store::claim(&conn, round.id, 11, "bates", 1_040).unwrap(), "the round is still won");
        // `granted` is the house points, nought while paused; `worth` is the
        // anagram points, which are the game's own and unchanged.
        store::add_solve(&conn, "2026-10-01", 11, round.id, 0, 2, "bates", 40, 1_040).expect("the solve is recorded");
        let board = store::day_tally(&conn, "2026-10-01");
        assert_eq!(board.len(), 1, "the board has the winner on it");
        assert_eq!((board[0].user, board[0].points), (11, 2), "and their anagram points are the full 2");
    }

    // --- the house commands themselves ---------------------------------------

    /// `/housepoints`, `/modgive` and a mod's `points 10` reply all refuse
    /// plainly rather than silently doing nothing.
    #[test]
    fn a_mod_is_refused_in_plain_words_rather_than_ignored() {
        let _switch = Switch::paused();
        assert!(!gate("a mod's house award"), "the mod path must be shut");
        assert!(!gate("/modgive"), "and so must giving");
        // Which is what each of them says.
        assert!(MOD_REFUSED.contains("House Cup is paused"), "{}", MOD_REFUSED);
        assert!(!MOD_REFUSED.trim().is_empty(), "silence is not a refusal");
    }

    /// `/frogdrop` and `/snitchdrop` say so rather than dropping.
    #[test]
    fn the_manual_drops_refuse_and_promise_the_cards() {
        let switch = Switch::paused();
        assert!(!gate("/frogdrop") && !gate("/snitchdrop"), "neither may drop while paused");
        for what in ["a Chocolate Frog", "a Snitch"] {
            let text = drop_refused(what);
            assert!(text.contains("House Cup is paused"), "{}", text);
        }
        // Back on and they work again.
        switch.set("on");
        assert!(gate("/frogdrop") && gate("/snitchdrop"), "the Cup is back, so the drops are");
    }

    /// `/housetop`, `/houses`, `/mypoints` and `/housecup` show a closed result,
    /// not a live table.
    #[test]
    fn the_house_commands_say_the_cup_is_paused_and_show_a_closed_result() {
        use super::super::points::Source;
        use super::super::standings;

        let top = standings::tests_housetop_text(&HOUSES[0], "September 2026", &[(7, 12, Some(Source::Quiz))], Some(7), 12, false);
        assert!(top.contains("paused") && top.contains("final"), "{}", top);
        assert!(top.contains("September 2026") && top.contains("12"), "September's numbers must still read: {}", top);
        assert!(top.contains(NOTE), "and it should say the games go on: {}", top);
        // Running, it is the live table it always was.
        let live = standings::tests_housetop_text(&HOUSES[0], "September 2026 so far", &[(7, 12, None)], None, 12, true);
        assert!(!live.contains("paused"), "{}", live);

        let mine = standings::tests_mypoints_text(&HOUSES[0], &[(Source::Quiz, 6), (Source::Chat, 3)], false, Some("September 2026"));
        assert!(mine.contains("paused") && mine.contains("September 2026") && mine.contains("**9**"), "{}", mine);
        assert!(mine.contains(NOTE), "{}", mine);

        let cup = standings::tests_housecup_words(Some("https://mlci.example/cup"), false, Some("September 2026".into()));
        assert!(cup.contains("paused") && cup.contains("final"), "{}", cup);
        assert!(cup.contains("https://mlci.example/cup"), "mods keep the page: {}", cup);
        assert!(!standings::tests_housecup_words(None, true, None).contains("paused"), "running, it is the live link");

        // `/today` stops pretending there are limits to fill.
        let today = standings::tests_today_paused_text(None);
        assert!(today.contains("paused") && today.contains("`/anagramtop`"), "{}", today);
        assert!(!today.contains("maxed"), "there are no limits running: {}", today);
        let theirs = standings::tests_today_paused_text(Some("Aarav"));
        assert!(theirs.contains("Aarav"), "{}", theirs);
    }

    /// The setting is in the catalog, described, and defaults to on.
    #[test]
    fn the_switch_is_in_the_panel_and_says_what_turning_it_off_does() {
        let setting = super::super::control::catalog::sections()
            .into_iter()
            .flat_map(|s| s.settings)
            .find(|s| s.key == KEY)
            .expect("the House Cup switch must be in the catalog");
        assert_eq!(setting.default, "on", "the panel must show it as on by default");
        assert!(matches!(setting.kind, super::super::control::catalog::Kind::Toggle), "it is a switch");
        let help = setting.help.to_lowercase();
        for promise in ["ledger", "frog", "snitch", "/housepoints", "/modgive", "crest", "trading"] {
            assert!(help.contains(promise), "the help should mention {:?}: {}", promise, setting.help);
        }
        assert!(help.contains("own score"), "and that the games keep going: {}", setting.help);
    }

    // --- what people are told ------------------------------------------------

    #[test]
    fn the_refusals_say_what_is_paused_and_what_is_not() {
        assert!(MOD_REFUSED.contains("paused"), "{}", MOD_REFUSED);
        assert!(MOD_REFUSED.to_lowercase().contains("still run"), "the games must be promised: {}", MOD_REFUSED);
        for what in ["a Chocolate Frog", "a Snitch"] {
            let text = drop_refused(what);
            assert!(text.contains("paused") && text.contains(what), "{}", text);
        }
        // The cards are the owner's promise: nothing may read as taking them.
        let frog = drop_refused("a Chocolate Frog");
        assert!(frog.contains("trading") && frog.contains("carry"), "the cards must be promised safe: {}", frog);
        assert!(NOTE.contains("paused") && NOTE.to_lowercase().contains("own scores"), "{}", NOTE);
        let closed = closed_heading("September 2026");
        assert!(closed.contains("September 2026") && closed.contains("final") && closed.contains("paused"), "{}", closed);
    }
}
