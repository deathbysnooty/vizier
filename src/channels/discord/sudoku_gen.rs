//! Making and solving a sudoku.
//!
//! Everything here is plain arithmetic: no clock, no database, no Discord, so
//! the whole of it can be tested. A puzzle is 81 squares in reading order, 0
//! for a blank; a `Grid` with no blanks is a finished sudoku.
//!
//! Making one goes: fill an empty grid at random, then take squares away one at
//! a time, keeping only the removals that leave the puzzle with exactly ONE
//! answer. How many squares are left, and which techniques it takes to fill
//! them in, decide the difficulty. Easy puzzles fall to "this square has only
//! one candidate" alone; medium ones also need "this digit fits only here in
//! this row, column or box"; hard ones need a guess somewhere.
//!
//! The solver is a bitmask backtracker that always fills the square with the
//! fewest candidates first, so counting a puzzle's answers is fast enough to do
//! after every single removal.

/// Squares in a grid.
pub const CELLS: usize = 81;

/// A grid in reading order: `grid[9 * row + column]`, 0 for a blank.
pub type Grid = [u8; CELLS];

pub const EMPTY: Grid = [0; CELLS];

/// How hard a puzzle is meant to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Easy,
    Medium,
    Hard,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Easy, Level::Medium, Level::Hard];

    /// How a level is stored and read back.
    pub fn key(self) -> &'static str {
        match self {
            Level::Easy => "easy",
            Level::Medium => "medium",
            Level::Hard => "hard",
        }
    }

    pub fn from_key(key: &str) -> Option<Level> {
        Self::ALL.into_iter().find(|l| l.key() == key.trim().to_ascii_lowercase())
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Easy => "Easy",
            Level::Medium => "Medium",
            Level::Hard => "Hard",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Level::Easy => "🟢",
            Level::Medium => "🟡",
            Level::Hard => "🔴",
        }
    }

    /// The colour its card is drawn in.
    pub fn colour(self) -> u32 {
        match self {
            Level::Easy => 0x3BA55C,
            Level::Medium => 0xE8B923,
            Level::Hard => 0xED4245,
        }
    }

    /// How many squares a puzzle of this level starts with, lowest first.
    pub fn givens(self) -> (usize, usize) {
        match self {
            Level::Easy => (36, 40),
            Level::Medium => (30, 34),
            Level::Hard => (26, 29),
        }
    }
}

/// A made puzzle and the answer only the bot keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Puzzle {
    pub givens: Grid,
    pub solution: Grid,
    pub level: Level,
}

impl Puzzle {
    pub fn blanks(&self) -> usize {
        self.givens.iter().filter(|d| **d == 0).count()
    }

    pub fn given_count(&self) -> usize {
        CELLS - self.blanks()
    }
}

// --- random numbers -------------------------------------------------------------------

/// SplitMix64: a few lines, good enough to shuffle with, and the same numbers
/// from the same seed every time, so a puzzle can be made again in a test.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn seeded(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// Seeded from the machine's randomness, for a real puzzle.
    pub fn fresh() -> Rng {
        Rng::seeded(rand::random::<u64>())
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number under `n` (0 for `n == 0`).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next_u64() % n as u64) as usize }
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
    }
}

// --- the board ------------------------------------------------------------------------

const ALL_DIGITS: u16 = 0x1FF;

pub fn box_of(cell: usize) -> usize {
    (cell / 27) * 3 + (cell % 9) / 3
}

/// "d5" for row 4, column 5: rows a–i from the top, columns 1–9 from the left.
pub fn cell_name(cell: usize) -> String {
    format!("{}{}", (b'a' + (cell / 9) as u8) as char, cell % 9 + 1)
}

/// The square a name like "d5" points at.
pub fn cell_of_name(name: &str) -> Option<usize> {
    let lower = name.trim().to_ascii_lowercase();
    let mut chars = lower.chars();
    let row = chars.next().filter(|c| ('a'..='i').contains(c))? as usize - 'a' as usize;
    let col = chars.next().and_then(|c| c.to_digit(10)).filter(|d| (1..=9).contains(d))? as usize - 1;
    chars.next().is_none().then_some(row * 9 + col)
}

/// A grid with its rows, columns and boxes kept as bitmasks, so placing and
/// taking back a digit is a handful of instructions.
#[derive(Clone)]
struct Board {
    grid: Grid,
    rows: [u16; 9],
    cols: [u16; 9],
    boxes: [u16; 9],
}

impl Board {
    /// `None` when the grid already breaks a rule.
    fn new(grid: &Grid) -> Option<Board> {
        let mut b = Board { grid: EMPTY, rows: [0; 9], cols: [0; 9], boxes: [0; 9] };
        for (cell, digit) in grid.iter().enumerate() {
            if *digit != 0 {
                if !b.allows(cell, *digit) {
                    return None;
                }
                b.place(cell, *digit);
            }
        }
        Some(b)
    }

    fn bit(digit: u8) -> u16 {
        1 << (digit - 1)
    }

    fn allows(&self, cell: usize, digit: u8) -> bool {
        let used = self.rows[cell / 9] | self.cols[cell % 9] | self.boxes[box_of(cell)];
        used & Self::bit(digit) == 0
    }

    fn place(&mut self, cell: usize, digit: u8) {
        self.grid[cell] = digit;
        let bit = Self::bit(digit);
        self.rows[cell / 9] |= bit;
        self.cols[cell % 9] |= bit;
        self.boxes[box_of(cell)] |= bit;
    }

    fn lift(&mut self, cell: usize) {
        let digit = self.grid[cell];
        if digit == 0 {
            return;
        }
        let bit = !Self::bit(digit);
        self.grid[cell] = 0;
        self.rows[cell / 9] &= bit;
        self.cols[cell % 9] &= bit;
        self.boxes[box_of(cell)] &= bit;
    }

    /// The digits still allowed in a blank square, as bits 0–8.
    fn candidates(&self, cell: usize) -> u16 {
        ALL_DIGITS & !(self.rows[cell / 9] | self.cols[cell % 9] | self.boxes[box_of(cell)])
    }

    /// The blank square with the fewest candidates, and how many it has.
    fn tightest(&self) -> Option<(usize, u32)> {
        let mut best: Option<(usize, u32)> = None;
        for cell in 0..CELLS {
            if self.grid[cell] != 0 {
                continue;
            }
            let n = self.candidates(cell).count_ones();
            if n <= 1 {
                return Some((cell, n));
            }
            if best.is_none_or(|(_, b)| n < b) {
                best = Some((cell, n));
            }
        }
        best
    }

    /// Answers this grid has, counted no further than `cap`. `found` keeps the
    /// first one.
    fn count(&mut self, cap: usize, found: &mut Option<Grid>) -> usize {
        let Some((cell, n)) = self.tightest() else {
            found.get_or_insert(self.grid);
            return 1;
        };
        if n == 0 {
            return 0;
        }
        let bits = self.candidates(cell);
        let mut total = 0;
        for digit in 1..=9u8 {
            if bits & Self::bit(digit) == 0 {
                continue;
            }
            self.place(cell, digit);
            total += self.count(cap - total, found);
            self.lift(cell);
            if total >= cap {
                return total;
            }
        }
        total
    }

    /// Fills the grid in at random; used to make a finished sudoku.
    fn fill(&mut self, rng: &mut Rng) -> bool {
        let Some((cell, n)) = self.tightest() else {
            return true;
        };
        if n == 0 {
            return false;
        }
        let bits = self.candidates(cell);
        let mut digits: Vec<u8> = (1..=9u8).filter(|d| bits & Self::bit(*d) != 0).collect();
        rng.shuffle(&mut digits);
        for digit in digits {
            self.place(cell, digit);
            if self.fill(rng) {
                return true;
            }
            self.lift(cell);
        }
        false
    }
}

/// The puzzle's one answer, or `None` when it has none — or more than one.
pub fn solve(grid: &Grid) -> Option<Grid> {
    let mut board = Board::new(grid)?;
    let mut found = None;
    if board.count(2, &mut found) == 1 { found } else { None }
}

/// Answers, counted no further than `cap` (so `count_solutions(g, 2) == 1`
/// means "exactly one").
pub fn count_solutions(grid: &Grid, cap: usize) -> usize {
    match Board::new(grid) {
        Some(mut board) => board.count(cap.max(1), &mut None),
        None => 0,
    }
}

pub fn has_one_solution(grid: &Grid) -> bool {
    count_solutions(grid, 2) == 1
}

/// Whether a finished grid breaks no rule and has no blank.
pub fn is_complete(grid: &Grid) -> bool {
    grid.iter().all(|d| (1..=9).contains(d)) && Board::new(grid).is_some()
}

// --- how hard it is -------------------------------------------------------------------

/// How far plain logic gets, from easiest to hardest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Technique {
    /// Every square falls to "only one digit fits here".
    NakedSingles,
    /// Some squares need "this digit fits nowhere else in this row, column or box".
    HiddenSingles,
    /// Logic runs out: somewhere you have to try a digit and see.
    Guessing,
}

/// The simplest set of techniques that finishes the puzzle.
fn technique(grid: &Grid) -> Technique {
    let Some(mut board) = Board::new(grid) else {
        return Technique::Guessing;
    };
    let mut hidden_used = false;
    loop {
        if board.grid.iter().all(|d| *d != 0) {
            return if hidden_used { Technique::HiddenSingles } else { Technique::NakedSingles };
        }
        // A square with exactly one candidate.
        let naked = (0..CELLS).find(|c| board.grid[*c] == 0 && board.candidates(*c).count_ones() == 1);
        if let Some(cell) = naked {
            let bits = board.candidates(cell);
            board.place(cell, bits.trailing_zeros() as u8 + 1);
            continue;
        }
        // A digit with exactly one home left in a row, a column or a box.
        match hidden_single(&board) {
            Some((cell, digit)) => {
                hidden_used = true;
                board.place(cell, digit);
            }
            None => return Technique::Guessing,
        }
    }
}

/// A digit that fits in only one square of some row, column or box.
fn hidden_single(board: &Board) -> Option<(usize, u8)> {
    let groups = |kind: usize, index: usize| -> [usize; 9] {
        std::array::from_fn(|i| match kind {
            0 => index * 9 + i,
            1 => i * 9 + index,
            _ => (index / 3) * 27 + (index % 3) * 3 + (i / 3) * 9 + i % 3,
        })
    };
    for kind in 0..3 {
        for index in 0..9 {
            let cells = groups(kind, index);
            for digit in 1..=9u8 {
                let bit = 1u16 << (digit - 1);
                let mut homes = cells.iter().filter(|c| board.grid[**c] == 0 && board.candidates(**c) & bit != 0);
                if let (Some(cell), None) = (homes.next(), homes.next()) {
                    return Some((*cell, digit));
                }
            }
        }
    }
    None
}

/// The level a finished puzzle really is: the harder of what its techniques
/// and its number of givens say. A puzzle that needs a guess is hard however
/// many squares it starts with.
pub fn rate(givens: &Grid) -> Level {
    let by_count = match CELLS - givens.iter().filter(|d| **d == 0).count() {
        n if n >= Level::Easy.givens().0 => Level::Easy,
        n if n >= Level::Medium.givens().0 => Level::Medium,
        _ => Level::Hard,
    };
    let by_technique = match technique(givens) {
        Technique::NakedSingles => Level::Easy,
        Technique::HiddenSingles => Level::Medium,
        Technique::Guessing => Level::Hard,
    };
    by_count.max(by_technique)
}

// --- making one -----------------------------------------------------------------------

/// A finished sudoku, filled at random.
pub fn full_grid(rng: &mut Rng) -> Grid {
    let mut board = Board { grid: EMPTY, rows: [0; 9], cols: [0; 9], boxes: [0; 9] };
    board.fill(rng);
    board.grid
}

/// Takes squares out of a finished grid, in a random order, keeping only the
/// removals that leave one answer, until `target` squares are left.
fn dig(full: &Grid, target: usize, rng: &mut Rng) -> Grid {
    let mut grid = *full;
    let mut order: Vec<usize> = (0..CELLS).collect();
    rng.shuffle(&mut order);
    let mut left = CELLS;
    for cell in order {
        if left <= target {
            break;
        }
        let digit = grid[cell];
        grid[cell] = 0;
        if has_one_solution(&grid) {
            left -= 1;
        } else {
            grid[cell] = digit;
        }
    }
    grid
}

/// How many attempts a level gets at landing on its own band before the last
/// good puzzle is used anyway. A puzzle is always returned.
const ATTEMPTS: usize = 24;

/// A puzzle of this level: one answer, the right number of givens, and — as
/// far as the tries allow — the techniques the level is named for.
pub fn generate(level: Level, rng: &mut Rng) -> Puzzle {
    let (low, high) = level.givens();
    let mut fallback: Option<Puzzle> = None;
    for _ in 0..ATTEMPTS {
        let solution = full_grid(rng);
        let target = low + rng.below(high - low + 1);
        let givens = dig(&solution, target, rng);
        let count = CELLS - givens.iter().filter(|d| **d == 0).count();
        if count > high {
            // Digging stalled above the band: nothing to keep.
            continue;
        }
        let puzzle = Puzzle { givens, solution, level };
        if rate(&givens) == level {
            return puzzle;
        }
        fallback.get_or_insert(puzzle);
    }
    fallback.unwrap_or_else(|| {
        let solution = full_grid(rng);
        let givens = dig(&solution, high, rng);
        Puzzle { givens, solution, level }
    })
}

// --- picking the next level -----------------------------------------------------------

/// The default mix of difficulties, as the setting holds it.
pub const DEFAULT_MIX: &str = "easy:40,medium:40,hard:20";

/// Reads a mix like "easy:40,medium:40,hard:20" into a weight per level.
/// Unknown names and bad numbers are ignored; a mix that says nothing at all
/// falls back to the default.
pub fn parse_mix(raw: &str) -> [u32; 3] {
    let mut weights = [0u32; 3];
    for part in raw.split(',') {
        let Some((name, value)) = part.split_once(':') else { continue };
        let (Some(level), Ok(weight)) = (Level::from_key(name), value.trim().parse::<u32>()) else { continue };
        weights[level as usize] = weights[level as usize].saturating_add(weight.min(1000));
    }
    if weights.iter().all(|w| *w == 0) { [40, 40, 20] } else { weights }
}

/// The level a roll of `roll` picks out of a mix.
pub fn pick_level(weights: [u32; 3], roll: u64) -> Level {
    let total: u64 = weights.iter().map(|w| *w as u64).sum();
    if total == 0 {
        return Level::Medium;
    }
    let mut at = roll % total;
    for level in Level::ALL {
        let w = weights[level as usize] as u64;
        if at < w {
            return level;
        }
        at -= w;
    }
    Level::Hard
}

/// Turns 81 characters (digits, with 0 or . for a blank) into a grid.
pub fn grid_from_str(text: &str) -> Option<Grid> {
    let digits: Vec<u8> = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '.' || c == '0' { 0 } else { c.to_digit(10).map(|d| d as u8).unwrap_or(255) })
        .collect();
    if digits.len() != CELLS || digits.iter().any(|d| *d > 9) {
        return None;
    }
    let mut grid = EMPTY;
    grid.copy_from_slice(&digits);
    Some(grid)
}

/// 81 characters, 0 for a blank: how a grid is stored and sent to the page.
pub fn grid_to_str(grid: &Grid) -> String {
    grid.iter().map(|d| char::from(b'0' + d)).collect()
}

/// How many filled squares differ from the answer, and how many are still blank.
pub fn wrong_squares(attempt: &Grid, solution: &Grid) -> (usize, usize) {
    let mut wrong = 0;
    let mut blank = 0;
    for (a, s) in attempt.iter().zip(solution.iter()) {
        match a {
            0 => blank += 1,
            d if d != s => wrong += 1,
            _ => {}
        }
    }
    (wrong, blank)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    const SAMPLE: &str = "530070000600195000098000060800060003400803001700020006060000280000419005000080079";

    #[test]
    fn the_solver_finishes_a_known_puzzle_and_refuses_a_broken_one() {
        let grid = grid_from_str(SAMPLE).expect("81 digits");
        let solved = solve(&grid).expect("one answer");
        assert!(is_complete(&solved));
        assert_eq!(&grid_to_str(&solved)[..9], "534678912");
        // Every given is still where it was.
        for (cell, digit) in grid.iter().enumerate() {
            if *digit != 0 {
                assert_eq!(solved[cell], *digit, "given at {} moved", cell_name(cell));
            }
        }
        // Two of the same digit in one row: no answer at all.
        let mut broken = grid;
        broken[1] = 5;
        assert_eq!(count_solutions(&broken, 2), 0);
        assert!(solve(&broken).is_none());
        // An empty grid has far more than one answer.
        assert_eq!(count_solutions(&EMPTY, 2), 2);
        assert!(solve(&EMPTY).is_none());
        assert!(grid_from_str("123").is_none());
        assert!(grid_from_str(&"1".repeat(81)).is_some());
    }

    #[test]
    fn every_made_puzzle_has_one_answer_and_the_right_number_of_givens() {
        for level in Level::ALL {
            for seed in 0..6u64 {
                let mut rng = Rng::seeded(seed * 97 + level as u64);
                let p = generate(level, &mut rng);
                let (low, high) = level.givens();
                assert!(
                    (low..=high).contains(&p.given_count()),
                    "{:?} made {} givens, wanted {}–{}",
                    level,
                    p.given_count(),
                    low,
                    high
                );
                assert!(has_one_solution(&p.givens), "{:?} puzzle has more than one answer", level);
                assert_eq!(solve(&p.givens), Some(p.solution), "the kept answer is not the puzzle's answer");
                assert!(is_complete(&p.solution));
                for (cell, digit) in p.givens.iter().enumerate() {
                    if *digit != 0 {
                        assert_eq!(p.solution[cell], *digit);
                    }
                }
            }
        }
    }

    #[test]
    fn difficulty_matches_the_techniques_a_puzzle_needs() {
        for level in Level::ALL {
            for seed in 0..5u64 {
                let mut rng = Rng::seeded(1_000 + seed * 31 + level as u64);
                let p = generate(level, &mut rng);
                assert_eq!(rate(&p.givens), level, "{:?} puzzle rated {:?}", level, rate(&p.givens));
            }
        }
        // An easy puzzle falls to single candidates; a hard one needs a guess.
        let mut rng = Rng::seeded(7);
        let easy = generate(Level::Easy, &mut rng);
        assert_eq!(technique(&easy.givens), Technique::NakedSingles);
        let hard = generate(Level::Hard, &mut rng);
        assert_eq!(technique(&hard.givens), Technique::Guessing);
        // A finished grid needs nothing at all, and a nearly full one is easy.
        assert_eq!(technique(&easy.solution), Technique::NakedSingles);
        assert_eq!(rate(&easy.solution), Level::Easy);
    }

    #[test]
    fn making_a_puzzle_is_quick() {
        for level in Level::ALL {
            let mut rng = Rng::seeded(4242 + level as u64);
            let start = Instant::now();
            let p = generate(level, &mut rng);
            let took = start.elapsed();
            assert!(has_one_solution(&p.givens));
            assert!(took.as_millis() < 200, "{:?} took {} ms", level, took.as_millis());
        }
    }

    #[test]
    fn the_same_seed_makes_the_same_puzzle() {
        let one = generate(Level::Medium, &mut Rng::seeded(99));
        let two = generate(Level::Medium, &mut Rng::seeded(99));
        assert_eq!(one, two);
        assert_ne!(generate(Level::Medium, &mut Rng::seeded(100)), one);
    }

    #[test]
    fn the_mix_decides_how_often_each_level_comes_up() {
        assert_eq!(parse_mix(DEFAULT_MIX), [40, 40, 20]);
        assert_eq!(parse_mix("EASY:1, hard : 3"), [1, 0, 3]);
        assert_eq!(parse_mix("nonsense"), [40, 40, 20], "an unreadable mix falls back to the default");
        assert_eq!(parse_mix("easy:0,medium:0,hard:0"), [40, 40, 20]);
        assert_eq!(pick_level([1, 0, 0], 12_345), Level::Easy, "one level in the mix always wins");
        assert_eq!(pick_level([0, 0, 1], 12_345), Level::Hard);
        let weights = parse_mix(DEFAULT_MIX);
        let mut seen = [0usize; 3];
        for roll in 0..1_000u64 {
            seen[pick_level(weights, roll) as usize] += 1;
        }
        assert_eq!(seen, [400, 400, 200]);
    }

    #[test]
    fn squares_are_named_from_a1_to_i9() {
        assert_eq!(cell_name(0), "a1");
        assert_eq!(cell_name(40), "e5");
        assert_eq!(cell_name(80), "i9");
        assert_eq!(cell_of_name("d5"), Some(3 * 9 + 4));
        assert_eq!(cell_of_name(" I9 "), Some(80));
        assert_eq!(cell_of_name("j1"), None);
        assert_eq!(cell_of_name("a0"), None);
        assert_eq!(cell_of_name("a12"), None);
        for cell in 0..CELLS {
            assert_eq!(cell_of_name(&cell_name(cell)), Some(cell));
        }
        assert_eq!(box_of(0), 0);
        assert_eq!(box_of(80), 8);
        assert_eq!(box_of(30), 4);
    }

    #[test]
    fn wrong_squares_counts_mistakes_without_saying_where() {
        let solution = solve(&grid_from_str(SAMPLE).unwrap()).unwrap();
        assert_eq!(wrong_squares(&solution, &solution), (0, 0));
        let mut attempt = solution;
        attempt[0] = if solution[0] == 1 { 2 } else { 1 };
        attempt[5] = 0;
        assert_eq!(wrong_squares(&attempt, &solution), (1, 1));
    }
}
