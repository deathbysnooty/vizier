//! The rules of Letter Duel: the board, the bag, what a play is worth, and
//! whether a play is allowed at all.
//!
//! Nothing here knows about Discord, the database or the web page. It is the
//! one place that decides what is legal, so the private board page and the
//! server agree by construction: the page is convenience, the checks below are
//! the game.
//!
//! The board is fifteen by fifteen, written down as 225 characters so a whole
//! position is one string in a database row and one field in a JSON reply:
//!
//! * `.` — an empty square.
//! * `A`–`Z` — a tile of that letter, worth what the letter is worth.
//! * `a`–`z` — a BLANK being that letter. It reads as the letter everywhere and
//!   is worth nothing, which is the whole difference between the two cases.
//!
//! A rack is the same idea in seven characters, with `?` for a blank nobody has
//! chosen a letter for yet, and the bag is simply the tiles still to be drawn,
//! in the order they will come out.
//!
//! Premium squares belong to the BOARD, not to the tiles: a square is worth its
//! premium to whoever first covers it and to nobody afterwards, which is why
//! scoring takes the board as it was BEFORE the play and the squares the play
//! is covering, and never looks at a premium under a tile that was already
//! there.

use std::collections::HashMap;

use super::sudoku_gen::Rng;

/// The board's side, and how many squares that is.
pub const SIZE: usize = 15;
pub const SQUARES: usize = SIZE * SIZE;
/// The middle square, which the first word has to cover.
pub const CENTRE: usize = SIZE * (SIZE / 2) + SIZE / 2;
/// How many tiles a player holds.
pub const RACK: usize = 7;
/// What using all seven of them is worth on top.
pub const BINGO: i64 = 50;
/// The blank, on a rack and in the bag.
pub const BLANK: char = '?';
/// An empty square.
pub const EMPTY: char = '.';

// --- the tiles ----------------------------------------------------------------------------

/// The standard English set: the letter, how many of it there are, and what one
/// is worth. The blank is last, worth nothing and standing for anything.
pub const DISTRIBUTION: &[(char, usize, i64)] = &[
    ('A', 9, 1),
    ('B', 2, 3),
    ('C', 2, 3),
    ('D', 4, 2),
    ('E', 12, 1),
    ('F', 2, 4),
    ('G', 3, 2),
    ('H', 2, 4),
    ('I', 9, 1),
    ('J', 1, 8),
    ('K', 1, 5),
    ('L', 4, 1),
    ('M', 2, 3),
    ('N', 6, 1),
    ('O', 8, 1),
    ('P', 2, 3),
    ('Q', 1, 10),
    ('R', 6, 1),
    ('S', 4, 1),
    ('T', 6, 1),
    ('U', 4, 1),
    ('V', 2, 4),
    ('W', 2, 4),
    ('X', 1, 8),
    ('Y', 2, 4),
    ('Z', 1, 10),
    (BLANK, 2, 0),
];

/// What one letter is worth. A blank — either the `?` on a rack or a lower-case
/// letter on the board — is worth nothing, wherever it sits.
pub fn value(tile: char) -> i64 {
    if tile.is_ascii_lowercase() || tile == BLANK {
        return 0;
    }
    DISTRIBUTION.iter().find(|(c, _, _)| *c == tile).map(|(_, _, v)| *v).unwrap_or(0)
}

/// A full bag, shuffled: every tile in the order it will be drawn.
pub fn fresh_bag(rng: &mut Rng) -> String {
    let mut tiles: Vec<char> = DISTRIBUTION.iter().flat_map(|(c, n, _)| std::iter::repeat_n(*c, *n)).collect();
    rng.shuffle(&mut tiles);
    tiles.into_iter().collect()
}

/// Takes up to `want` tiles off the front of the bag. Returns what came out and
/// what is left, so a caller can write both down in one go.
pub fn draw(bag: &str, want: usize) -> (String, String) {
    let taken: String = bag.chars().take(want).collect();
    let left: String = bag.chars().skip(taken.chars().count()).collect();
    (taken, left)
}

/// Puts tiles back and shuffles the bag, which is what an exchange does.
pub fn put_back(bag: &str, tiles: &str, rng: &mut Rng) -> String {
    let mut all: Vec<char> = bag.chars().chain(tiles.chars()).collect();
    rng.shuffle(&mut all);
    all.into_iter().collect()
}

/// What a rack of tiles is worth, for the sum an unfinished game settles with.
pub fn rack_value(rack: &str) -> i64 {
    rack.chars().map(value).sum()
}

// --- the premium squares --------------------------------------------------------------------

/// What a square is worth to the tile or the word that covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Premium {
    Plain,
    DoubleLetter,
    TripleLetter,
    DoubleWord,
    TripleWord,
    /// The middle star. It counts as a double word, and is drawn differently.
    Centre,
}

impl Premium {
    /// How much this square multiplies the tile standing on it.
    pub fn letter_times(self) -> i64 {
        match self {
            Premium::DoubleLetter => 2,
            Premium::TripleLetter => 3,
            _ => 1,
        }
    }

    /// How much this square multiplies a word running through it.
    pub fn word_times(self) -> i64 {
        match self {
            Premium::DoubleWord | Premium::Centre => 2,
            Premium::TripleWord => 3,
            _ => 1,
        }
    }

    /// The short name the board picture and the web page use.
    pub fn key(self) -> &'static str {
        match self {
            Premium::Plain => "",
            Premium::DoubleLetter => "dl",
            Premium::TripleLetter => "tl",
            Premium::DoubleWord => "dw",
            Premium::TripleWord => "tw",
            Premium::Centre => "centre",
        }
    }

    /// What is printed on an empty one.
    pub fn label(self) -> &'static str {
        match self {
            Premium::Plain => "",
            Premium::DoubleLetter => "DL",
            Premium::TripleLetter => "TL",
            Premium::DoubleWord => "DW",
            Premium::TripleWord => "TW",
            Premium::Centre => "★",
        }
    }
}

/// The standard layout, written out so it can be read rather than worked out:
/// `T` triple word, `D` double word, `t` triple letter, `d` double letter,
/// `S` the middle star.
const LAYOUT: [&str; SIZE] = [
    "T..d...T...d..T",
    ".D...t...t...D.",
    "..D...d.d...D..",
    "d..D...d...D..d",
    "....D.....D....",
    ".t...t...t...t.",
    "..d...d.d...d..",
    "T..d...S...d..T",
    "..d...d.d...d..",
    ".t...t...t...t.",
    "....D.....D....",
    "d..D...d...D..d",
    "..D...d.d...D..",
    ".D...t...t...D.",
    "T..d...T...d..T",
];

/// What the square at an index is worth.
pub fn premium(at: usize) -> Premium {
    if at >= SQUARES {
        return Premium::Plain;
    }
    match LAYOUT[at / SIZE].as_bytes()[at % SIZE] {
        b'T' => Premium::TripleWord,
        b'D' => Premium::DoubleWord,
        b't' => Premium::TripleLetter,
        b'd' => Premium::DoubleLetter,
        b'S' => Premium::Centre,
        _ => Premium::Plain,
    }
}

/// A square's name the way a person says it: "H8" for the middle, columns A–O
/// across and rows 1–15 down.
pub fn square_name(at: usize) -> String {
    if at >= SQUARES {
        return String::new();
    }
    format!("{}{}", (b'A' + (at % SIZE) as u8) as char, at / SIZE + 1)
}

// --- the board -------------------------------------------------------------------------------

/// An empty board, as the 225 characters a game starts from.
pub fn empty_board() -> String {
    std::iter::repeat_n(EMPTY, SQUARES).collect()
}

/// A board string as characters, padded or cut to exactly 225 so a damaged row
/// can never make the rest of this panic.
pub fn cells(board: &str) -> Vec<char> {
    let mut out: Vec<char> = board.chars().take(SQUARES).collect();
    out.resize(SQUARES, EMPTY);
    out
}

/// Whether a square has a tile on it.
pub fn filled(cells: &[char], at: usize) -> bool {
    cells.get(at).is_some_and(|c| *c != EMPTY)
}

/// The letter a square reads as, upper case whether or not it is a blank.
pub fn letter_at(cells: &[char], at: usize) -> Option<char> {
    cells.get(at).copied().filter(|c| *c != EMPTY).map(|c| c.to_ascii_uppercase())
}

/// Whether anything at all has been played.
pub fn board_is_empty(cells: &[char]) -> bool {
    cells.iter().all(|c| *c == EMPTY)
}

// --- a play ------------------------------------------------------------------------------------

/// One tile going down: which square, and what it is. `blank` means the tile
/// leaving the rack is a blank being used as `letter`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub at: usize,
    pub letter: char,
    pub blank: bool,
}

impl Placement {
    /// The character this placement writes onto the board.
    pub fn cell(&self) -> char {
        if self.blank { self.letter.to_ascii_lowercase() } else { self.letter.to_ascii_uppercase() }
    }
}

/// Which way a play runs. A single tile is called across; it makes no
/// difference to anything, because one square is both a row and a column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Across {
    Row,
    Column,
}

/// One word a play made, and what that word scored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Word {
    pub word: String,
    pub score: i64,
}

/// A legal play, worked out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Play {
    /// Every word made, the one along the line of play first.
    pub words: Vec<Word>,
    /// What the whole turn scored, the fifty for seven tiles included.
    pub score: i64,
    /// Whether all seven tiles went down.
    pub bingo: bool,
    /// The squares that were covered, in the order they were given.
    pub covered: Vec<usize>,
    /// The board after the play.
    pub board: String,
    /// What is left on the rack afterwards, before any draw.
    pub rack_left: String,
}

impl Play {
    /// The word the play is named by on the card: the longest one made, which
    /// for an ordinary play is the one along the line.
    pub fn headline(&self) -> String {
        self.words.first().map(|w| w.word.clone()).unwrap_or_default()
    }
}

/// Why a play cannot be made. Every one of these is shown to a player as it is,
/// so the words are the ones a person would use.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No tiles were put down at all.
    Nothing,
    /// More tiles than a rack holds, or the same square twice.
    Impossible,
    /// A tile was put somewhere there is already one.
    Occupied(usize),
    /// A tile that isn't on the rack.
    NotYours(char),
    /// The tiles are not all in one row or one column.
    NotInLine,
    /// There is a hole in the line.
    Gap,
    /// The first word has to cover the middle.
    MissesCentre,
    /// Nothing the play put down touches anything already on the board.
    Floating,
    /// A single tile on its own, making no word at all.
    TooShort,
    /// A word that isn't in the dictionary.
    NotAWord(String),
}

impl Refusal {
    /// The refusal in plain words, the way the page and the pop-up show it.
    pub fn words(&self) -> String {
        match self {
            Refusal::Nothing => "You haven't put any tiles down yet.".to_string(),
            Refusal::Impossible => "That isn't a play I can make sense of — pick the tiles up and try again.".to_string(),
            Refusal::Occupied(at) => format!("There's already a tile on {}.", square_name(*at)),
            Refusal::NotYours(letter) => format!("You don't have a {} on your rack.", letter.to_ascii_uppercase()),
            Refusal::NotInLine => "Your tiles have to go in one row or one column, all together.".to_string(),
            Refusal::Gap => "There's a gap in your word. The tiles have to run without a break.".to_string(),
            Refusal::MissesCentre => "The first word has to cross the middle.".to_string(),
            Refusal::Floating => "That doesn't touch anything. Every word after the first has to join what's already there.".to_string(),
            Refusal::TooShort => "One tile on its own isn't a word. Play at least two letters.".to_string(),
            Refusal::NotAWord(word) => format!("{} isn't a word I know.", word.to_uppercase()),
        }
    }
}

/// A run of tiles on the board: where it starts, which way it goes, how long.
fn run(cells: &[char], at: usize, across: Across) -> (usize, usize) {
    let (row, col) = (at / SIZE, at % SIZE);
    let step = match across {
        Across::Row => 1,
        Across::Column => SIZE,
    };
    let (mut lo, mut hi) = (at, at);
    // Walk back to the start of the run, then on to its end.
    let mut before = match across {
        Across::Row => col,
        Across::Column => row,
    };
    while before > 0 && filled(cells, lo - step) {
        lo -= step;
        before -= 1;
    }
    let mut after = match across {
        Across::Row => SIZE - 1 - col,
        Across::Column => SIZE - 1 - row,
    };
    while after > 0 && filled(cells, hi + step) {
        hi += step;
        after -= 1;
    }
    (lo, (hi - lo) / step + 1)
}

/// The word a run spells.
fn spell(cells: &[char], from: usize, length: usize, across: Across) -> String {
    let step = match across {
        Across::Row => 1,
        Across::Column => SIZE,
    };
    (0..length).filter_map(|i| letter_at(cells, from + i * step)).collect()
}

/// What one run scores, given which of its squares were covered THIS turn: only
/// those squares' premiums count, and a word multiplier is applied after every
/// letter has been counted.
fn score_run(cells: &[char], from: usize, length: usize, across: Across, fresh: &[usize]) -> i64 {
    let step = match across {
        Across::Row => 1,
        Across::Column => SIZE,
    };
    let mut total = 0i64;
    let mut times = 1i64;
    for i in 0..length {
        let at = from + i * step;
        let tile = cells.get(at).copied().unwrap_or(EMPTY);
        let new = fresh.contains(&at);
        let square = if new { premium(at) } else { Premium::Plain };
        total += value(tile) * square.letter_times();
        times *= square.word_times();
    }
    total * times
}

/// Whether a square has a tile beside it.
fn touches(cells: &[char], at: usize) -> bool {
    let (row, col) = (at / SIZE, at % SIZE);
    (col > 0 && filled(cells, at - 1))
        || (col + 1 < SIZE && filled(cells, at + 1))
        || (row > 0 && filled(cells, at - SIZE))
        || (row + 1 < SIZE && filled(cells, at + SIZE))
}

/// Takes the tiles a play needs off a rack, or says which one isn't there. A
/// blank placement takes a `?`; an ordinary one takes its own letter.
pub fn spend(rack: &str, placements: &[Placement]) -> Result<String, Refusal> {
    let mut left: Vec<char> = rack.chars().collect();
    for p in placements {
        let want = if p.blank { BLANK } else { p.letter.to_ascii_uppercase() };
        match left.iter().position(|c| *c == want) {
            Some(i) => {
                left.remove(i);
            }
            None => return Err(Refusal::NotYours(if p.blank { BLANK } else { p.letter })),
        }
    }
    Ok(left.into_iter().collect())
}

/// Reads a play: checks it against the board and the rack, works out every word
/// it makes, looks each up, and adds the score. The ONE door every way of
/// playing goes through.
///
/// `known` is asked about each word in lower case; it is the dictionary.
pub fn check(board: &str, rack: &str, placements: &[Placement], known: impl Fn(&str) -> bool) -> Result<Play, Refusal> {
    if placements.is_empty() {
        return Err(Refusal::Nothing);
    }
    if placements.len() > RACK {
        return Err(Refusal::Impossible);
    }
    let mut seen: Vec<usize> = Vec::new();
    for p in placements {
        if p.at >= SQUARES || !p.letter.is_ascii_alphabetic() || seen.contains(&p.at) {
            return Err(Refusal::Impossible);
        }
        seen.push(p.at);
    }
    let before = cells(board);
    for p in placements {
        if filled(&before, p.at) {
            return Err(Refusal::Occupied(p.at));
        }
    }
    let rack_left = spend(rack, placements)?;

    // One row or one column. A single tile is both, and is called a row.
    let rows: Vec<usize> = placements.iter().map(|p| p.at / SIZE).collect();
    let cols: Vec<usize> = placements.iter().map(|p| p.at % SIZE).collect();
    let across = if rows.iter().all(|r| *r == rows[0]) {
        Across::Row
    } else if cols.iter().all(|c| *c == cols[0]) {
        Across::Column
    } else {
        return Err(Refusal::NotInLine);
    };

    let mut after = before.clone();
    for p in placements {
        after[p.at] = p.cell();
    }

    // No holes between the first and the last tile of the play: squares the
    // board already filled count, which is how a word is extended.
    let step = match across {
        Across::Row => 1,
        Across::Column => SIZE,
    };
    let (lo, hi) = (seen.iter().copied().min().unwrap_or(0), seen.iter().copied().max().unwrap_or(0));
    let mut at = lo;
    while at <= hi {
        if !filled(&after, at) {
            return Err(Refusal::Gap);
        }
        at += step;
    }

    let first_move = board_is_empty(&before);
    if first_move {
        if !seen.contains(&CENTRE) {
            return Err(Refusal::MissesCentre);
        }
    } else if !seen.iter().any(|at| touches(&before, *at)) {
        return Err(Refusal::Floating);
    }

    // Every word the play made: the run along the line of play, then the run
    // across each tile that grew one. A run of one letter is not a word.
    let mut words: Vec<Word> = Vec::new();
    let sideways = match across {
        Across::Row => Across::Column,
        Across::Column => Across::Row,
    };
    let (main_from, main_len) = run(&after, lo, across);
    if main_len > 1 {
        words.push(Word {
            word: spell(&after, main_from, main_len, across),
            score: score_run(&after, main_from, main_len, across, &seen),
        });
    }
    for p in placements {
        let (from, length) = run(&after, p.at, sideways);
        if length > 1 {
            words.push(Word {
                word: spell(&after, from, length, sideways),
                score: score_run(&after, from, length, sideways, &seen),
            });
        }
    }
    if words.is_empty() {
        return Err(Refusal::TooShort);
    }
    for word in &words {
        if !known(&word.word.to_lowercase()) {
            return Err(Refusal::NotAWord(word.word.clone()));
        }
    }

    let bingo = placements.len() == RACK;
    let score: i64 = words.iter().map(|w| w.score).sum::<i64>() + if bingo { BINGO } else { 0 };
    Ok(Play {
        words,
        score,
        bingo,
        covered: placements.iter().map(|p| p.at).collect(),
        board: after.into_iter().collect(),
        rack_left,
    })
}

// --- how a game ends ---------------------------------------------------------------------------

/// How many passes in a row end a game: everybody passing twice.
pub fn passes_to_end(players: usize) -> i64 {
    (players.max(1) as i64) * 2
}

/// Whether the passing has gone on long enough to stop the game.
pub fn passed_out(passes_in_a_row: i64, players: usize) -> bool {
    passes_in_a_row >= passes_to_end(players)
}

/// What the racks left over do to the scores when a game ends.
///
/// `racks` is every player's remaining tiles, in seat order, and `went_out` is
/// the player who used their last tile — `None` when everyone simply passed.
/// The one who went out gains what everybody else is holding; everybody else
/// loses what they are holding. Nobody gains when the game merely petered out.
pub fn adjustments(racks: &[&str], went_out: Option<usize>) -> Vec<i64> {
    let held: Vec<i64> = racks.iter().map(|r| rack_value(r)).collect();
    let mut out: Vec<i64> = held.iter().map(|v| -v).collect();
    if let Some(winner) = went_out {
        if winner < out.len() {
            out[winner] = held.iter().enumerate().filter(|(i, _)| *i != winner).map(|(_, v)| *v).sum();
        }
    }
    out
}

/// The places players finished in, best score first. Ties share a place, so two
/// on the same score are both second and nobody is third.
pub fn placings(scores: &[i64]) -> Vec<usize> {
    let mut sorted: Vec<i64> = scores.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    scores.iter().map(|s| sorted.iter().position(|o| o == s).unwrap_or(0) + 1).collect()
}

// --- what a turn is worth in house points -------------------------------------------------------

/// What one player earned from a finished game, BEFORE the ledger's own daily
/// limit is applied.
///
/// A game with only two people in it is where friends could farm each other, so
/// it pays the winner a consolation and nobody else anything at all. Three or
/// more and the full prizes are paid: the winner, the runner-up, and one point
/// to everyone who was still playing at the end, so turning up pays.
pub fn prize(place: usize, played_to_the_end: bool, players: usize, win: i64, second: i64, played: i64) -> i64 {
    if players < 3 {
        return if place == 1 && played_to_the_end { win.min(second) } else { 0 };
    }
    if !played_to_the_end {
        return 0;
    }
    match place {
        1 => win,
        2 => second,
        _ => played,
    }
}

// --- reading what a page sent ---------------------------------------------------------------

/// Reads the placements a page sent: `[{"at":112,"letter":"S"},…]` has already
/// become this shape by the time it gets here, but the letters may be anything
/// at all, so they are tidied and anything unreadable is refused.
pub fn read_placements(raw: &[(i64, String)]) -> Result<Vec<Placement>, Refusal> {
    let mut out = Vec::new();
    for (at, letter) in raw {
        let letter = letter.trim();
        let mut chars = letter.chars();
        let (Some(c), None) = (chars.next(), chars.next()) else {
            return Err(Refusal::Impossible);
        };
        if !c.is_ascii_alphabetic() || *at < 0 || *at as usize >= SQUARES {
            return Err(Refusal::Impossible);
        }
        out.push(Placement { at: *at as usize, letter: c.to_ascii_uppercase(), blank: c.is_ascii_lowercase() });
    }
    Ok(out)
}

/// How many of each tile a bag or rack holds, for the tests and the panel.
pub fn counts(tiles: &str) -> HashMap<char, usize> {
    let mut out = HashMap::new();
    for c in tiles.chars() {
        *out.entry(c).or_insert(0) += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A tiny dictionary: everything the tests below want to be a word.
    fn words() -> HashSet<String> {
        [
            "cat", "cats", "car", "cars", "at", "as", "is", "it", "ha", "hat", "hats", "he", "her", "here", "an", "and",
            "band", "bands", "ban", "bane", "be", "bead", "bed", "den", "end", "ends", "no", "on", "one", "or", "ore",
            "so", "son", "to", "too", "tone", "toned", "shrimp", "quiz", "quizzes", "ax", "axe", "dog", "dogs", "go",
            "god", "gods", "oh", "ohs", "id", "ids", "ea", "tea", "team", "teams", "eat", "eats", "ate", "ae", "ex",
            "farming", "farm", "arm", "am", "ma", "mat", "mats", "rat", "rats", "art", "tar", "arts", "star", "stare",
            "we", "wed", "new", "news", "sew", "sewn", "own", "owns", "now", "win", "wins", "pig", "pigs", "in", "if",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    fn known(set: &HashSet<String>) -> impl Fn(&str) -> bool + '_ {
        move |w: &str| set.contains(w)
    }

    fn at(square: &str) -> usize {
        let col = square.as_bytes()[0].to_ascii_uppercase() - b'A';
        let row: usize = square[1..].parse().expect("a row");
        (row - 1) * SIZE + col as usize
    }

    fn put(square: &str, letter: char) -> Placement {
        Placement { at: at(square), letter, blank: false }
    }

    fn blank(square: &str, letter: char) -> Placement {
        Placement { at: at(square), letter, blank: true }
    }

    /// Lays a word down without checking anything, for setting a position up.
    fn lay(board: &str, square: &str, word: &str, across: Across) -> String {
        let mut cells = cells(board);
        let step = match across {
            Across::Row => 1,
            Across::Column => SIZE,
        };
        for (i, c) in word.chars().enumerate() {
            cells[at(square) + i * step] = c;
        }
        cells.into_iter().collect()
    }

    #[test]
    fn the_bag_is_the_standard_hundred_tiles() {
        let bag = fresh_bag(&mut Rng::seeded(7));
        assert_eq!(bag.chars().count(), 100);
        let counts = counts(&bag);
        assert_eq!(counts.get(&'E'), Some(&12));
        assert_eq!(counts.get(&'Z'), Some(&1));
        assert_eq!(counts.get(&'A'), Some(&9));
        assert_eq!(counts.get(&BLANK), Some(&2));
        assert_eq!(counts.values().sum::<usize>(), 100);
        // Every letter of the alphabet is in there, and only those plus blanks.
        assert_eq!(counts.len(), 27);
        // Two fresh bags are not the same order.
        assert_ne!(fresh_bag(&mut Rng::seeded(1)), fresh_bag(&mut Rng::seeded(2)));
    }

    #[test]
    fn letters_are_worth_what_they_are_worth_and_a_blank_is_worth_nothing() {
        assert_eq!(value('A'), 1);
        assert_eq!(value('Q'), 10);
        assert_eq!(value('Z'), 10);
        assert_eq!(value('J'), 8);
        assert_eq!(value('K'), 5);
        assert_eq!(value(BLANK), 0, "a blank on the rack");
        assert_eq!(value('q'), 0, "a blank being a Q is still worth nothing");
        assert_eq!(rack_value("QUIZ"), 10 + 1 + 1 + 10);
        assert_eq!(rack_value("?"), 0);
    }

    #[test]
    fn the_premium_squares_are_the_standard_layout() {
        let mut tally: HashMap<&str, usize> = HashMap::new();
        for square in 0..SQUARES {
            *tally.entry(premium(square).key()).or_insert(0) += 1;
        }
        assert_eq!(tally.get("tw"), Some(&8), "eight triple words, one at each corner and mid-edge");
        assert_eq!(tally.get("dw"), Some(&16), "sixteen double words, and the star makes seventeen");
        assert_eq!(tally.get("centre"), Some(&1));
        assert_eq!(tally.get("tl"), Some(&12));
        assert_eq!(tally.get("dl"), Some(&24));
        assert_eq!(premium(0), Premium::TripleWord);
        assert_eq!(premium(CENTRE), Premium::Centre);
        assert_eq!(premium(SQUARES - 1), Premium::TripleWord);
        assert_eq!(square_name(CENTRE), "H8");
        assert_eq!(square_name(0), "A1");
        assert_eq!(square_name(SQUARES - 1), "O15");
        // The layout is symmetric, the way a real board is.
        for row in 0..SIZE {
            for col in 0..SIZE {
                assert_eq!(premium(row * SIZE + col), premium(row * SIZE + (SIZE - 1 - col)), "left and right differ at {},{}", row, col);
                assert_eq!(premium(row * SIZE + col), premium((SIZE - 1 - row) * SIZE + col), "top and bottom differ at {},{}", row, col);
            }
        }
    }

    // --- the first word ----------------------------------------------------------

    #[test]
    fn the_first_word_has_to_cross_the_middle() {
        let dict = words();
        let board = empty_board();
        let away = check(&board, "CATS???", &[put("A1", 'C'), put("B1", 'A'), put("C1", 'T')], known(&dict));
        assert_eq!(away, Err(Refusal::MissesCentre));
        assert!(away.unwrap_err().words().contains("cross the middle"));

        let ok = check(&board, "CATS???", &[put("H8", 'C'), put("I8", 'A'), put("J8", 'T')], known(&dict)).expect("a legal first word");
        assert_eq!(ok.headline(), "CAT");
        // The star doubles the word: C 3 + A 1 + T 1 = 5, doubled.
        assert_eq!(ok.score, 10);
        assert_eq!(ok.rack_left, "S???");
    }

    #[test]
    fn one_tile_on_its_own_is_not_a_word() {
        let dict = words();
        let board = empty_board();
        assert_eq!(check(&board, "A??????", &[put("H8", 'A')], known(&dict)), Err(Refusal::TooShort));
    }

    // --- the shape of a play -----------------------------------------------------

    #[test]
    fn tiles_go_in_one_line_with_no_gaps() {
        let dict = words();
        let board = empty_board();
        let bent = check(&board, "CAT????", &[put("H8", 'C'), put("I8", 'A'), put("I9", 'T')], known(&dict));
        assert_eq!(bent, Err(Refusal::NotInLine));
        assert!(bent.unwrap_err().words().contains("one row or one column"));

        let holed = check(&board, "CAT????", &[put("H8", 'C'), put("J8", 'A')], known(&dict));
        assert_eq!(holed, Err(Refusal::Gap));
        assert!(holed.unwrap_err().words().contains("gap"));
    }

    #[test]
    fn a_later_play_has_to_touch_what_is_there() {
        let dict = words();
        let board = lay(&empty_board(), "H8", "CAT", Across::Row);
        let adrift = check(&board, "DOG????", &[put("A14", 'D'), put("B14", 'O'), put("C14", 'G')], known(&dict));
        assert_eq!(adrift, Err(Refusal::Floating));
        assert!(adrift.unwrap_err().words().contains("doesn't touch anything"));

        // Hanging a word off the T of CAT is fine.
        let joined = check(&board, "OO?????", &[put("J9", 'O'), put("J10", 'O')], known(&dict)).expect("TOO runs down from the T");
        assert_eq!(joined.headline(), "TOO");
    }

    #[test]
    fn a_tile_cannot_go_where_one_already_is() {
        let dict = words();
        let board = lay(&empty_board(), "H8", "CAT", Across::Row);
        let on_top = check(&board, "S??????", &[put("H8", 'S')], known(&dict));
        assert_eq!(on_top, Err(Refusal::Occupied(at("H8"))));
        assert!(on_top.unwrap_err().words().contains("H8"));
    }

    #[test]
    fn you_can_only_play_tiles_you_hold() {
        let dict = words();
        let board = empty_board();
        let borrowed = check(&board, "CAT????", &[put("H8", 'C'), put("I8", 'A'), put("J8", 'R')], known(&dict));
        assert_eq!(borrowed, Err(Refusal::NotYours('R')));
        assert!(borrowed.unwrap_err().words().contains("don't have a R"));
        assert_eq!(spend("CATS", &[put("H8", 'C'), put("I8", 'A')]).unwrap(), "TS");
    }

    // --- words sideways ----------------------------------------------------------

    #[test]
    fn every_word_a_play_makes_has_to_be_a_word_sideways_as_well() {
        let dict = words();
        // CAT across the middle. A play under it that spells a word along its
        // own row but rubbish downwards has to be refused.
        let board = lay(&empty_board(), "H8", "CAT", Across::Row);
        // EA reads along row 9, but downwards it makes CE and AA.
        let sideways = check(&board, "EA?????", &[put("H9", 'E'), put("I9", 'A')], known(&dict));
        match sideways {
            Err(Refusal::NotAWord(word)) => assert_eq!(word, "CE", "the first bad word it met"),
            other => panic!("a play that spells rubbish sideways must be refused, got {:?}", other),
        }
        // AT reads along row 9 too, and makes AA and TT downwards.
        let bad = check(&board, "AT?????", &[put("I9", 'A'), put("J9", 'T')], known(&dict));
        assert!(matches!(bad, Err(Refusal::NotAWord(_))), "got {:?}", bad);

        // And one that works: HER down column I, through the E of TEA.
        let board = lay(&empty_board(), "H8", "TEA", Across::Row);
        let good = check(&board, "HR?????", &[put("I7", 'H'), put("I9", 'R')], known(&dict)).expect("HER down the column");
        assert_eq!(good.headline(), "HER");
        assert_eq!(good.words.len(), 1, "neither tile grew a word sideways: {:?}", good.words);
    }

    #[test]
    fn a_play_is_scored_for_the_main_word_and_every_cross_word() {
        let dict = words();
        // AT across the middle, then MA in the row above it: MA along row 7,
        // and downwards MA through the A and AT through the T.
        let board = lay(&empty_board(), "H8", "AT", Across::Row);
        let play = check(&board, "MA?????", &[put("H7", 'M'), put("I7", 'A')], known(&dict)).expect("MA above AT");
        let made: Vec<&str> = play.words.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(made, vec!["MA", "MA", "AT"], "the row, then each column");
        // I7 is a double letter and nothing else is fresh on a premium:
        // row MA = 3 + 1x2 = 5 · column MA = 3 + 1 = 4 · column AT = 1x2 + 1 = 3.
        assert_eq!(play.words.iter().map(|w| w.score).collect::<Vec<_>>(), vec![5, 4, 3]);
        assert_eq!(play.score, 12);
        assert!(!play.bingo);
    }

    // --- premium squares ---------------------------------------------------------

    #[test]
    fn a_premium_square_multiplies_the_letter_then_the_whole_word() {
        let dict = words();
        // D8 is a double letter and H8 the star, so a word across the middle
        // row covering both doubles the S and then doubles the whole word.
        let play = check(
            &empty_board(),
            "STARE??",
            &[put("D8", 'S'), put("E8", 'T'), put("F8", 'A'), put("G8", 'R'), put("H8", 'E')],
            known(&dict),
        )
        .expect("STARE through the middle");
        assert_eq!(premium(at("D8")), Premium::DoubleLetter);
        // S 1 doubled = 2, T 1, A 1, R 1, E 1 = 6, doubled by the star = 12.
        assert_eq!(play.score, 12);
        assert_eq!(play.headline(), "STARE");
    }

    #[test]
    fn a_double_word_through_a_double_letter_multiplies_both() {
        let dict = words();
        assert_eq!(premium(at("A4")), Premium::DoubleLetter);
        assert_eq!(premium(at("D4")), Premium::DoubleWord);
        // An S already on E4, and BAND laid before it makes BANDS: the B lands
        // on the double letter and the D on the double word.
        let board = lay(&empty_board(), "E4", "S", Across::Row);
        let play = check(&board, "BAND???", &[put("A4", 'B'), put("B4", 'A'), put("C4", 'N'), put("D4", 'D')], known(&dict))
            .expect("BANDS along row 4");
        assert_eq!(play.headline(), "BANDS");
        // B 3 doubled = 6, A 1, N 1, D 2, S 1 = 11, doubled by D4 = 22.
        assert_eq!(play.score, 22);
        // With both premiums already spent, the same word is worth its
        // letters and nothing more: 3 + 1 + 1 + 2 + 1.
        let spent: Vec<usize> = Vec::new();
        assert_eq!(score_run(&cells(&play.board), at("A4"), 5, Across::Row, &spent), 8);
    }

    #[test]
    fn a_premium_square_is_spent_once_and_never_counts_again() {
        let dict = words();
        // AT through the star, so the star is used up. HA down column H then
        // scores plainly: if the star still doubled it would be worth ten.
        let board = lay(&empty_board(), "H8", "AT", Across::Row);
        let play = check(&board, "H??????", &[put("H7", 'H')], known(&dict)).expect("HA down column H");
        assert_eq!(play.headline(), "HA");
        assert_eq!(play.score, 5, "H 4 + A 1, with the star already spent");
    }

    // --- blanks -----------------------------------------------------------------

    #[test]
    fn a_blank_can_be_any_letter_and_scores_nothing() {
        let dict = words();
        let board = empty_board();
        // A blank standing for C, then A and T from the rack.
        let play = check(&board, "?AT????", &[blank("H8", 'C'), put("I8", 'A'), put("J8", 'T')], known(&dict))
            .expect("a blank spells the C");
        assert_eq!(play.headline(), "CAT", "the blank reads as the letter it stands for");
        // The blank on the star is worth 0, doubled is still 0: (0+1+1) × 2 = 4.
        assert_eq!(play.score, 4);
        assert_eq!(play.rack_left, "????", "the blank left the rack, not a C");
        // It is written down in lower case, so it stays worth nothing forever.
        assert_eq!(cells(&play.board)[CENTRE], 'c');
    }

    #[test]
    fn a_blank_still_reads_as_its_letter_for_the_words_around_it() {
        let dict = words();
        let board = empty_board();
        let play = check(&board, "?AT????", &[blank("H8", 'C'), put("I8", 'A'), put("J8", 'T')], known(&dict)).expect("CAT");
        // Now an S on the end makes CATS: the blank C still counts as a C.
        let more = check(&play.board, "S??????", &[put("K8", 'S')], known(&dict)).expect("CATS");
        assert_eq!(more.headline(), "CATS");
        // c 0 + A 1 + T 1 + S 1, and K8 is plain, so 3.
        assert_eq!(more.score, 3);
    }

    // --- seven tiles ------------------------------------------------------------

    #[test]
    fn using_all_seven_tiles_is_worth_fifty_more() {
        let dict = words();
        let board = empty_board();
        let play = check(
            &board,
            "FARMING",
            &[
                put("B8", 'F'),
                put("C8", 'A'),
                put("D8", 'R'),
                put("E8", 'M'),
                put("F8", 'I'),
                put("G8", 'N'),
                put("H8", 'G'),
            ],
            known(&dict),
        )
        .expect("FARMING through the middle");
        assert!(play.bingo);
        assert_eq!(play.rack_left, "");
        // F 4 + A 1 + R(D8, double letter) 1×2 + M 3 + I 1 + N 1 + G 2 = 14,
        // doubled by the star = 28, and fifty for the seven tiles.
        assert_eq!(play.words[0].score, 28);
        assert_eq!(play.score, 78);

        // Six tiles is no bonus.
        let six = check(&board, "FARMIN?", &[put("C8", 'F'), put("D8", 'A'), put("E8", 'R'), put("F8", 'M'), put("G8", 'I'), put("H8", 'N')], known(&dict));
        assert!(six.is_err(), "FARMIN is not a word — the bonus never rescues a non-word");
    }

    // --- the dictionary ---------------------------------------------------------

    #[test]
    fn a_word_nobody_knows_is_refused_by_name() {
        let dict = words();
        let board = empty_board();
        let refused = check(
            &board,
            "SHRIMPS",
            &[put("B8", 'S'), put("C8", 'H'), put("D8", 'R'), put("E8", 'I'), put("F8", 'M'), put("G8", 'P'), put("H8", 'S')],
            known(&dict),
        );
        assert_eq!(refused, Err(Refusal::NotAWord("SHRIMPS".into())));
        assert_eq!(refused.unwrap_err().words(), "SHRIMPS isn't a word I know.");
    }

    // --- the end of a game -------------------------------------------------------

    #[test]
    fn going_out_takes_what_everybody_else_is_holding() {
        // Three players; the first used their last tile.
        let sums = adjustments(&["", "QI", "AB"], Some(0));
        assert_eq!(sums, vec![10 + 1 + 1 + 3, -(10 + 1), -(1 + 3)]);
        assert_eq!(sums.iter().sum::<i64>(), 0, "what one gains the others lose");

        // Nobody went out: everyone simply loses their own.
        let passed = adjustments(&["AB", "Q", ""], None);
        assert_eq!(passed, vec![-4, -10, 0]);
    }

    #[test]
    fn everybody_passing_twice_ends_the_game() {
        assert_eq!(passes_to_end(2), 4);
        assert_eq!(passes_to_end(4), 8);
        assert!(!passed_out(3, 2));
        assert!(passed_out(4, 2), "two players, two passes each");
        assert!(!passed_out(7, 4));
        assert!(passed_out(8, 4));
    }

    #[test]
    fn places_are_worked_out_with_ties_sharing() {
        assert_eq!(placings(&[30, 10, 20]), vec![1, 3, 2]);
        assert_eq!(placings(&[10, 10, 5]), vec![1, 1, 3], "two firsts and no second");
        assert_eq!(placings(&[5, 9]), vec![2, 1]);
    }

    // --- what a game pays ---------------------------------------------------------

    #[test]
    fn two_friends_cannot_farm_each_other() {
        // Three or more: the full prizes.
        assert_eq!(prize(1, true, 3, 4, 2, 1), 4);
        assert_eq!(prize(2, true, 3, 4, 2, 1), 2);
        assert_eq!(prize(3, true, 3, 4, 2, 1), 1);
        assert_eq!(prize(4, true, 4, 4, 2, 1), 1, "turning up pays");
        assert_eq!(prize(1, false, 3, 4, 2, 1), 0, "somebody dropped pays nothing");

        // Two: the winner gets the runner-up's share and the loser nothing.
        assert_eq!(prize(1, true, 2, 4, 2, 1), 2);
        assert_eq!(prize(2, true, 2, 4, 2, 1), 0);
        assert_eq!(prize(1, false, 2, 4, 2, 1), 0);
    }

    // --- the bag ------------------------------------------------------------------

    #[test]
    fn tiles_are_drawn_off_the_front_and_put_back_shuffled() {
        let (drawn, left) = draw("ABCDEFGHIJ", 7);
        assert_eq!(drawn, "ABCDEFG");
        assert_eq!(left, "HIJ");
        // A short bag hands over what it has and no more.
        let (drawn, left) = draw("AB", 7);
        assert_eq!(drawn, "AB");
        assert_eq!(left, "");
        let (drawn, left) = draw("", 7);
        assert!(drawn.is_empty() && left.is_empty());

        let back = put_back("ABC", "XYZ", &mut Rng::seeded(4));
        assert_eq!(back.chars().count(), 6);
        let counts = counts(&back);
        for c in "ABCXYZ".chars() {
            assert_eq!(counts.get(&c), Some(&1), "{} came back", c);
        }
    }

    #[test]
    fn a_play_that_is_nothing_at_all_is_refused_before_anything_else() {
        let dict = words();
        assert_eq!(check(&empty_board(), "AAAAAAA", &[], known(&dict)), Err(Refusal::Nothing));
        let too_many: Vec<Placement> = (0..8).map(|i| Placement { at: CENTRE + i, letter: 'A', blank: false }).collect();
        assert_eq!(check(&empty_board(), "AAAAAAA", &too_many, known(&dict)), Err(Refusal::Impossible));
        let twice = [put("H8", 'A'), put("H8", 'T')];
        assert_eq!(check(&empty_board(), "AT?????", &twice, known(&dict)), Err(Refusal::Impossible));
    }

    #[test]
    fn what_a_page_sent_is_read_and_rubbish_refused() {
        let read = read_placements(&[(112, "C".into()), (113, "a".into())]).expect("two tiles");
        assert_eq!(read[0], Placement { at: 112, letter: 'C', blank: false });
        assert_eq!(read[1], Placement { at: 113, letter: 'A', blank: true }, "lower case means a blank");
        assert!(read_placements(&[(112, "CA".into())]).is_err());
        assert!(read_placements(&[(112, "4".into())]).is_err());
        assert!(read_placements(&[(-1, "C".into())]).is_err());
        assert!(read_placements(&[(225, "C".into())]).is_err());
        assert!(read_placements(&[(112, "".into())]).is_err());
    }
}
