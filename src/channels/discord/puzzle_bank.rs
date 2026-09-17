//! The chess puzzle bank: the positions the puzzle game sets.
//!
//! Two plain files in `<workspace>/puzzlebank/`, the way the anagrams game reads
//! its words from `wordbank/`:
//!
//! * `puzzles.tsv` — one puzzle per line, six tab-separated columns:
//!   `id  fen  moves  rating  band  themes`.
//! * `index.json` — the pack's metadata. Only the attribution line is read from
//!   it; a missing or unreadable file falls back to [`ATTRIBUTION`].
//!
//! **The one thing to get right: the FEN is one move early.** `fen` is the
//! position BEFORE the opponent's last move, and `moves[0]` is that move. Play
//! it and you reach the position the solver is shown — which means the solver's
//! colour is the OPPOSITE of the FEN's side to move. The rest of `moves`
//! alternates solver, opponent, solver, …, and always ends on a solver move, so
//! the list is always an even length. [`Puzzle::setup`] and [`Puzzle::line`]
//! split it that way once, here, so nothing downstream has to remember the rule.
//!
//! If the bank isn't there the puzzle simply never starts — [`open`] says so in
//! the log and leaves [`bank`] empty, and the rest of chess is unaffected.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use super::sudoku_gen::Rng;

/// The line the help card has to carry. The puzzles are CC0, so no credit is
/// legally required — but they were mined from millions of real games and rated
/// by millions of solvers, and saying where they came from costs one line.
pub const ATTRIBUTION: &str = "Puzzles from the Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0.";

/// Where one puzzle can be looked at in full, solution and all.
pub fn lichess_link(id: &str) -> String {
    format!("https://lichess.org/training/{}", id)
}

static BANK: OnceLock<Bank> = OnceLock::new();

/// One puzzle, as the game uses it rather than as the file writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Puzzle {
    /// The Lichess id, which links back to the puzzle in full.
    pub id: String,
    /// The position BEFORE the opponent's move. Never shown to anybody.
    pub fen: String,
    /// The opponent's move, in UCI. Played to reach the position the solver
    /// sees — the blunder that set the puzzle up.
    pub setup: String,
    /// What follows it: solver, opponent, solver, … always starting and ending
    /// with a solver move, so the length is odd.
    pub line: Vec<String>,
    pub rating: u16,
    /// "easy", "medium" or "hard".
    pub band: String,
    pub themes: Vec<String>,
}

impl Puzzle {
    /// How many moves the solver has to find.
    pub fn solver_moves(&self) -> usize {
        self.line.len().div_ceil(2)
    }

    /// The solver's `n`th move (from 0), if the line has one.
    pub fn solver_move(&self, n: usize) -> Option<&str> {
        self.line.get(n * 2).map(String::as_str)
    }

    /// The opponent's reply to the solver's `n`th move, if there is one. The
    /// line ends on a solver move, so the last one never has a reply.
    pub fn reply_to(&self, n: usize) -> Option<&str> {
        self.line.get(n * 2 + 1).map(String::as_str)
    }
}

/// One line of the pack as a puzzle, or nothing when it isn't one. Anything
/// malformed is skipped rather than refused: a bank with a stray line still
/// works.
pub fn puzzle_of(line: &str) -> Option<Puzzle> {
    let mut columns = line.split('\t');
    let (id, fen, moves, rating, band, themes) =
        (columns.next()?, columns.next()?, columns.next()?, columns.next()?, columns.next()?, columns.next()?);
    if id.trim().is_empty() || fen.trim().is_empty() {
        return None;
    }
    let mut uci = moves.split_whitespace().map(str::to_string);
    let setup = uci.next()?;
    let line: Vec<String> = uci.collect();
    // The list is the opponent's move and then whole solver/opponent pairs, so
    // what is left after the setup move must be odd: it starts and ends with a
    // solver move. Anything else is not a puzzle this game knows how to run.
    if line.is_empty() || line.len() % 2 == 0 {
        return None;
    }
    Some(Puzzle {
        id: id.to_string(),
        fen: fen.to_string(),
        setup,
        line,
        rating: rating.trim().parse().unwrap_or(0),
        band: band.trim().to_string(),
        themes: themes.split_whitespace().map(str::to_string).collect(),
    })
}

/// The puzzles in play.
#[derive(Debug, Default)]
pub struct Bank {
    puzzles: Vec<Puzzle>,
    attribution: String,
}

impl Bank {
    /// Reads the pack as it stands. `index` is `index.json`; anything wrong with
    /// it costs only the attribution line, which falls back to the built-in one.
    pub fn from_text(pack: &str, index: &str) -> Bank {
        let puzzles: Vec<Puzzle> = pack.lines().filter_map(puzzle_of).collect();
        let attribution = serde_json::from_str::<serde_json::Value>(index)
            .ok()
            .and_then(|v| v.get("attribution").and_then(|a| a.as_str()).map(str::to_string))
            .filter(|a| !a.trim().is_empty())
            .unwrap_or_else(|| ATTRIBUTION.to_string());
        Bank { puzzles, attribution }
    }

    pub fn count(&self) -> usize {
        self.puzzles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.puzzles.is_empty()
    }

    /// The line the help card carries, as the pack itself words it.
    pub fn attribution(&self) -> &str {
        &self.attribution
    }

    /// One puzzle by its Lichess id, for a card that has to be redrawn.
    pub fn get(&self, id: &str) -> Option<&Puzzle> {
        self.puzzles.iter().find(|p| p.id == id)
    }

    /// The `n`th puzzle in the pack, for walking it.
    pub fn nth(&self, n: usize) -> Option<&Puzzle> {
        self.puzzles.get(n)
    }

    /// A puzzle to set: one that hasn't been set lately. When every puzzle has
    /// been used it starts over rather than leaving the channel empty.
    pub fn pick(&self, used: &HashSet<String>, rng: &mut Rng) -> Option<&Puzzle> {
        if self.puzzles.is_empty() {
            return None;
        }
        let start = rng.below(self.puzzles.len());
        let fresh = (0..self.puzzles.len()).map(|step| &self.puzzles[(start + step) % self.puzzles.len()]).find(|p| !used.contains(&p.id));
        Some(fresh.unwrap_or(&self.puzzles[start]))
    }
}

/// Reads the bank out of a folder. The error says which file is missing or
/// unreadable, so the log tells a mod what to do about it.
pub fn load(dir: &Path) -> anyhow::Result<Bank> {
    let pack = dir.join("puzzles.tsv");
    let raw = std::fs::read_to_string(&pack).map_err(|e| anyhow::anyhow!("{}: {}", pack.display(), e))?;
    // A missing index costs the attribution line and nothing else.
    let index = std::fs::read_to_string(dir.join("index.json")).unwrap_or_default();
    let bank = Bank::from_text(&raw, &index);
    if bank.is_empty() {
        anyhow::bail!("{} has no puzzles in it", pack.display());
    }
    Ok(bank)
}

pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["puzzlebank"]);
    match load(&dir) {
        Ok(bank) => {
            tracing::info!("puzzle: {} puzzles read from {}", bank.count(), dir.display());
            let _ = BANK.set(bank);
        }
        Err(err) => tracing::warn!(
            "puzzle: no puzzle bank ({}) — the puzzle stays off and the rest of chess is unaffected. Copy puzzlebank/ with puzzles.tsv into the workspace.",
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

    /// A bank small enough to read, with one puzzle of every shape the game can
    /// meet in it: a mate in one, a two-move line, a promotion and a castle
    /// played as the opponent's setting-up move, a promotion the SOLVER plays,
    /// and an en-passant capture the solver plays. Every line here is a real row
    /// from the shipped pack, copied out of it unchanged.
    pub const PACK: &str = "\
4kYFv\tr1bqkb1r/pp1pp1np/2n2p2/6NB/3p3B/4P3/PPP2PPP/RN1QK2R b KQkq - 1 9\tg7h5 d1h5\t802\teasy\tmate mateIn1 oneMove opening
WqnOB\t6k1/7p/p1P1b1p1/5p2/8/6P1/r5BP/2R3K1 b - - 0 29\te6c8 g2d5 g8g7 d5a2\t800\teasy\tcrushing endgame fork master short
Zs7l3\t3K4/3R1Pp1/6kp/5n2/8/8/r7/8 w - - 13 58\tf7f8q a2a8 d8c7 a8f8\t800\teasy\tcrushing endgame short skewer
LhHU9\t3rkb1r/pp3ppp/8/1Np1Pb2/1n3B2/4PP2/PPPnB1PP/R3K1NR w KQk - 3 11\te1c1 b4a2\t885\teasy\tmate mateIn1 middlegame oneMove queensideAttack
65RdP\t8/KP6/8/P6p/5k2/7P/8/1q6 b - - 2 59\tb1b5 b7b8q b5b8 a7b8\t840\teasy\tadvancedPawn crushing endgame promotion queenEndgame short
I4ZCY\tr4rk1/p1qnbp2/1p2p2p/3bP3/3PN3/2PBQ3/P5PP/R4RK1 b - - 1 18\td5e4 e3e4 f7f5 e5f6\t2091\thard\tadvantage enPassant master middlegame short
";

    pub const INDEX: &str = r#"{"attribution": "Puzzles from the Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0."}"#;

    pub fn fixture() -> Bank {
        Bank::from_text(PACK, INDEX)
    }

    #[test]
    fn a_row_splits_into_the_opponents_move_and_the_line_that_follows_it() {
        let bank = fixture();
        assert_eq!(bank.count(), 6);
        // A mate in one: the opponent blunders, the solver has one move.
        let one = bank.get("4kYFv").expect("the mate in one");
        assert_eq!(one.setup, "g7h5", "the FEN is one move early and this is that move");
        assert_eq!(one.line, vec!["d1h5"]);
        assert_eq!(one.solver_moves(), 1);
        assert_eq!(one.solver_move(0), Some("d1h5"));
        assert_eq!(one.reply_to(0), None, "the line ends on a solver move");
        assert_eq!(one.rating, 802);
        assert_eq!(one.band, "easy");
        assert!(one.themes.contains(&"mateIn1".to_string()));

        // A two-move line: solver, opponent's reply, solver.
        let two = bank.get("WqnOB").expect("the two-mover");
        assert_eq!(two.setup, "e6c8");
        assert_eq!(two.line, vec!["g2d5", "g8g7", "d5a2"]);
        assert_eq!(two.solver_moves(), 2);
        assert_eq!((two.solver_move(0), two.reply_to(0)), (Some("g2d5"), Some("g8g7")));
        assert_eq!((two.solver_move(1), two.reply_to(1)), (Some("d5a2"), None));
        assert_eq!(two.solver_move(2), None);
    }

    #[test]
    fn a_line_that_isnt_a_puzzle_is_skipped_rather_than_breaking_the_bank() {
        // No columns, too few columns, no moves at all, and an even line (which
        // would end on the opponent's move, not the solver's).
        for bad in [
            "",
            "id\tfen",
            "id\t8/8/8/8/8/8/8/8 w - - 0 1\t\t800\teasy\tmate",
            "id\t8/8/8/8/8/8/8/8 w - - 0 1\te2e4\t800\teasy\tmate",
            "id\t8/8/8/8/8/8/8/8 w - - 0 1\te2e4 e7e5 g1f3\t800\teasy\tmate",
            "\t8/8/8/8/8/8/8/8 w - - 0 1\te2e4 e7e5 g1f3\t800\teasy\tmate",
        ] {
            assert_eq!(puzzle_of(bad), None, "{:?} is not a puzzle", bad);
        }
        // And a bank with rubbish in it still keeps the good rows.
        let mixed = Bank::from_text(&format!("nonsense\n{}\n\n", PACK), INDEX);
        assert_eq!(mixed.count(), 6);
    }

    #[test]
    fn the_attribution_comes_from_the_pack_and_falls_back_to_the_built_in_one() {
        assert_eq!(fixture().attribution(), ATTRIBUTION);
        assert!(ATTRIBUTION.contains("Lichess") && ATTRIBUTION.contains("CC0 1.0"));
        // A missing, empty or broken index costs the line and nothing else.
        for index in ["", "{}", "not json at all", r#"{"attribution": "  "}"#] {
            let bank = Bank::from_text(PACK, index);
            assert_eq!(bank.attribution(), ATTRIBUTION, "index {:?}", index);
            assert_eq!(bank.count(), 6);
        }
        assert_eq!(lichess_link("WqnOB"), "https://lichess.org/training/WqnOB");
    }

    #[test]
    fn a_puzzle_set_lately_is_passed_over_until_they_all_have_been() {
        let bank = fixture();
        let mut rng = Rng::seeded(7);
        let mut used: HashSet<String> = HashSet::new();
        for _ in 0..bank.count() {
            let picked = bank.pick(&used, &mut rng).expect("a puzzle").id.clone();
            assert!(used.insert(picked), "the same puzzle is never set twice inside the window");
        }
        // Every one used: it starts over rather than leaving the channel empty.
        assert!(bank.pick(&used, &mut rng).is_some());
        assert!(Bank::default().pick(&used, &mut rng).is_none());
    }

    /// The shipped bank, read from the repository, so a change to the pack that
    /// the game could not run is caught here rather than in the channel.
    #[test]
    fn the_shipped_bank_is_readable_and_every_row_is_a_puzzle() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("puzzlebank");
        let Ok(raw) = std::fs::read_to_string(dir.join("puzzles.tsv")) else {
            return; // Not every checkout has the pack; `open` already says so.
        };
        let lines = raw.lines().filter(|l| !l.trim().is_empty()).count();
        let bank = load(&dir).expect("the shipped bank");
        assert_eq!(bank.count(), lines, "every line in the pack is a puzzle the game can run");
        assert!(bank.count() > 1_000, "only {} puzzles", bank.count());
        assert_eq!(bank.attribution(), ATTRIBUTION, "the pack's own line is the one the help card must carry");
        for id in ["4kYFv", "WqnOB"] {
            assert!(bank.get(id).is_some(), "{} is in the shipped pack", id);
        }
    }
}
