//! The code a player copies off the web page and pastes into Discord.
//!
//! A code carries only the squares the player filled in — the puzzle's own
//! givens are already known to both sides — so it stays short:
//!
//! ```text
//! S128-4KQ7X-2PJ9D-…
//! ```
//!
//! `S` then the puzzle's number, then the answers in Crockford-free RFC 4648
//! base32, dashed every five characters so it can be read aloud. Five squares
//! (each 0 for blank, or 1–9) pack into one number under 100 000 and so into 17
//! bits; the bits run end to end, and a CRC-16 of the puzzle number and those
//! bytes is appended, so a code that lost a character is refused rather than
//! read as a wrong answer.
//!
//! [`encode`] and [`decode`] are mirrored character for character by
//! `sudokuCode()` in `control/ui/sudoku.js`; [`tests::TEST_VECTOR`] is the
//! example both sides must produce.

use super::sudoku_gen::{CELLS, Grid};

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/// Squares packed into one 17-bit number.
const PER_GROUP: usize = 5;
const GROUP_BITS: usize = 17;
/// How many characters a dash comes after.
const DASH_EVERY: usize = 5;

/// Why a pasted code wasn't accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeError {
    /// Not a code at all: no `S`, no number, nothing to read.
    NotACode,
    /// A perfectly good code — for another puzzle.
    OtherPuzzle(i64),
    /// The right puzzle, but the code is damaged: a wrong length, a character
    /// that isn't in the alphabet, or a checksum that doesn't match.
    Damaged,
}

impl CodeError {
    /// What the player is told, in plain words.
    pub fn words(self, live: i64) -> String {
        match self {
            CodeError::NotACode => {
                "That doesn't look like a code. Press ▶️ Play, finish the grid, then press **Copy code** and paste the whole thing.".to_string()
            }
            CodeError::OtherPuzzle(id) => format!(
                "That code is for **puzzle #{}**, and the live one is **#{}**. Press ▶️ Play for the puzzle that's up now.",
                id, live
            ),
            CodeError::Damaged => {
                "That code got cut off or changed on the way. Press **Copy code** on the page again and paste the whole thing.".to_string()
            }
        }
    }
}

// --- bits ---------------------------------------------------------------------------

struct BitWriter {
    bytes: Vec<u8>,
    /// Bits already written into the last byte.
    used: u32,
}

impl BitWriter {
    fn new() -> BitWriter {
        BitWriter { bytes: Vec::new(), used: 8 }
    }

    /// Writes the low `count` bits of `value`, biggest bit first.
    fn push(&mut self, value: u32, count: u32) {
        for i in (0..count).rev() {
            if self.used == 8 {
                self.bytes.push(0);
                self.used = 0;
            }
            let bit = ((value >> i) & 1) as u8;
            let last = self.bytes.len() - 1;
            self.bytes[last] |= bit << (7 - self.used);
            self.used += 1;
        }
    }
}

struct BitReader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl BitReader<'_> {
    /// The next `count` bits, biggest first; `None` past the end.
    fn take(&mut self, count: u32) -> Option<u32> {
        let mut out = 0u32;
        for _ in 0..count {
            let byte = *self.bytes.get(self.at / 8)?;
            out = (out << 1) | ((byte >> (7 - self.at % 8)) & 1) as u32;
            self.at += 1;
        }
        Some(out)
    }
}

// --- base32 -------------------------------------------------------------------------

fn base32(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | *byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn unbase32(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0u32, 0u32);
    for c in text.chars() {
        let value = ALPHABET.iter().position(|a| *a as char == c)? as u32;
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// CRC-16/CCITT-FALSE: short, well known, and easy to write the same way twice.
pub fn crc16(bytes: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for byte in bytes {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// What the checksum is taken over: the puzzle's number, then the packed squares.
fn checked(puzzle_id: i64, payload: &[u8]) -> Vec<u8> {
    let id = (puzzle_id.max(0) as u32).to_be_bytes();
    let mut all = id.to_vec();
    all.extend_from_slice(payload);
    all
}

// --- the code -----------------------------------------------------------------------

/// The blank squares of a puzzle, in reading order.
fn blanks(givens: &Grid) -> Vec<usize> {
    (0..CELLS).filter(|c| givens[*c] == 0).collect()
}

/// How many bytes the packed squares take.
fn payload_len(blank_count: usize) -> usize {
    let groups = blank_count.div_ceil(PER_GROUP);
    (groups * GROUP_BITS).div_ceil(8)
}

/// The code for what a player has filled in. `filled` is the whole grid as they
/// see it; only the squares the puzzle left blank are carried.
pub fn encode(puzzle_id: i64, givens: &Grid, filled: &Grid) -> String {
    let cells = blanks(givens);
    let mut writer = BitWriter::new();
    for group in cells.chunks(PER_GROUP) {
        let mut value = 0u32;
        let mut place = 1u32;
        for cell in group {
            value += (filled[*cell].min(9) as u32) * place;
            place *= 10;
        }
        writer.push(value, GROUP_BITS as u32);
    }
    let mut bytes = writer.bytes;
    bytes.resize(payload_len(cells.len()), 0);
    let crc = crc16(&checked(puzzle_id, &bytes));
    bytes.push((crc >> 8) as u8);
    bytes.push((crc & 0xFF) as u8);
    let body = base32(&bytes);
    let groups: Vec<String> = body.as_bytes().chunks(DASH_EVERY).map(|c| String::from_utf8_lossy(c).to_string()).collect();
    format!("S{}-{}", puzzle_id, groups.join("-"))
}

/// The puzzle a code belongs to, without reading the rest of it.
pub fn puzzle_id_of(code: &str) -> Option<i64> {
    let tidy: String = code.chars().filter(|c| !c.is_whitespace()).collect();
    let rest = tidy.strip_prefix('S').or_else(|| tidy.strip_prefix('s'))?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|id| *id > 0)
}

/// Reads a code back into a whole grid: the puzzle's givens with the player's
/// squares filled in. Blank squares come back as 0, so an unfinished grid is
/// read fine and the caller can say how much is left.
pub fn decode(code: &str, puzzle_id: i64, givens: &Grid) -> Result<Grid, CodeError> {
    let tidy: String = code.chars().filter(|c| !c.is_whitespace() && *c != '-' && *c != '_').collect::<String>().to_ascii_uppercase();
    let rest = tidy.strip_prefix('S').ok_or(CodeError::NotACode)?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let id: i64 = digits.parse().map_err(|_| CodeError::NotACode)?;
    if id <= 0 {
        return Err(CodeError::NotACode);
    }
    if id != puzzle_id {
        return Err(CodeError::OtherPuzzle(id));
    }
    let body: String = rest.chars().skip(digits.chars().count()).collect();
    if body.is_empty() {
        return Err(CodeError::NotACode);
    }
    let bytes = unbase32(&body).ok_or(CodeError::Damaged)?;
    let cells = blanks(givens);
    let want = payload_len(cells.len());
    if bytes.len() != want + 2 {
        return Err(CodeError::Damaged);
    }
    let (payload, crc) = bytes.split_at(want);
    let given_crc = ((crc[0] as u16) << 8) | crc[1] as u16;
    if given_crc != crc16(&checked(puzzle_id, payload)) {
        return Err(CodeError::Damaged);
    }
    let mut grid = *givens;
    let mut reader = BitReader { bytes: payload, at: 0 };
    for group in cells.chunks(PER_GROUP) {
        let mut value = reader.take(GROUP_BITS as u32).ok_or(CodeError::Damaged)?;
        if value >= 100_000 {
            return Err(CodeError::Damaged);
        }
        for cell in group {
            grid[*cell] = (value % 10) as u8;
            value /= 10;
        }
    }
    Ok(grid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::sudoku_gen::{Level, Rng, generate, grid_from_str, solve};

    const SAMPLE: &str = "530070000600195000098000060800060003400803001700020006060000280000419005000080079";

    /// The example the page's JavaScript must produce for the same puzzle, so
    /// the two encoders can be compared without running a browser:
    /// puzzle 128, the sample grid above, filled in with its answer.
    pub const TEST_VECTOR: &str = "S128-E3GCU-QQL37-JW254-ZBHZZ-53GSX-D3UPV-Z5CPA-AAIBG-LI";

    fn sample() -> (Grid, Grid) {
        let givens = grid_from_str(SAMPLE).expect("81 digits");
        (givens, solve(&givens).expect("one answer"))
    }

    #[test]
    fn a_finished_grid_survives_the_round_trip() {
        let (givens, solution) = sample();
        let code = encode(128, &givens, &solution);
        assert_eq!(code, TEST_VECTOR, "the shared example changed: the page's copy must change with it");
        assert!(code.starts_with("S128-"));
        assert_eq!(decode(&code, 128, &givens), Ok(solution));
        assert_eq!(puzzle_id_of(&code), Some(128));
        // Dashes, spaces and lower case are all forgiven.
        let messy = format!("  {}  ", code.replace('-', "").to_lowercase());
        assert_eq!(decode(&messy, 128, &givens), Ok(solution));
    }

    #[test]
    fn the_code_is_short_enough_to_paste() {
        for level in Level::ALL {
            let p = generate(level, &mut Rng::seeded(11 + level as u64));
            let code = encode(4096, &p.givens, &p.solution);
            assert!(
                (30..=70).contains(&code.chars().count()),
                "{:?} code is {} characters: {}",
                level,
                code.chars().count(),
                code
            );
        }
    }

    #[test]
    fn a_damaged_code_is_refused_rather_than_misread() {
        let (givens, solution) = sample();
        let code = encode(128, &givens, &solution);
        // One character changed.
        let mut broken: Vec<char> = code.chars().collect();
        let at = broken.iter().position(|c| *c == 'Z').expect("a Z in the body");
        broken[at] = 'Q';
        let changed: String = broken.into_iter().collect();
        assert_eq!(decode(&changed, 128, &givens), Err(CodeError::Damaged));
        // A character cut off the end, and a character outside the alphabet.
        assert_eq!(decode(&code[..code.len() - 4], 128, &givens), Err(CodeError::Damaged));
        assert_eq!(decode(&format!("{}!", code), 128, &givens), Err(CodeError::Damaged));
        // The same squares, claimed for a different puzzle: the number is part
        // of the checksum, so swapping it over is caught too.
        let relabelled = code.replacen("S128", "S129", 1);
        assert_eq!(decode(&relabelled, 129, &givens), Err(CodeError::Damaged));
        for nonsense in ["", "hello", "S", "S0-ABCDE", "128-ABCDE"] {
            assert_eq!(decode(nonsense, 128, &givens), Err(CodeError::NotACode), "{}", nonsense);
        }
    }

    #[test]
    fn a_code_for_another_puzzle_says_so() {
        let (givens, solution) = sample();
        let code = encode(127, &givens, &solution);
        assert_eq!(decode(&code, 128, &givens), Err(CodeError::OtherPuzzle(127)));
        let words = CodeError::OtherPuzzle(127).words(128);
        assert!(words.contains("#127") && words.contains("#128"), "{}", words);
        assert_eq!(puzzle_id_of("s127-abc"), Some(127));
        assert_eq!(puzzle_id_of("nope"), None);
    }

    #[test]
    fn an_unfinished_grid_comes_back_with_its_blanks() {
        let (givens, solution) = sample();
        let mut partial = solution;
        let blank_cells: Vec<usize> = blanks(&givens).into_iter().take(3).collect();
        for cell in &blank_cells {
            partial[*cell] = 0;
        }
        let back = decode(&encode(9, &givens, &partial), 9, &givens).expect("readable");
        assert_eq!(back, partial);
        for cell in &blank_cells {
            assert_eq!(back[*cell], 0);
        }
        // The givens themselves always come back, whatever the code carries.
        for (cell, digit) in givens.iter().enumerate() {
            if *digit != 0 {
                assert_eq!(back[cell], *digit);
            }
        }
    }

    #[test]
    fn every_puzzle_of_every_level_round_trips() {
        for level in Level::ALL {
            for seed in 0..3u64 {
                let p = generate(level, &mut Rng::seeded(500 + seed * 7 + level as u64));
                let id = 1 + seed as i64 * 37;
                assert_eq!(decode(&encode(id, &p.givens, &p.solution), id, &p.givens), Ok(p.solution));
            }
        }
    }

    #[test]
    fn the_checksum_catches_a_single_bit() {
        assert_eq!(crc16(b""), 0xFFFF);
        assert_eq!(crc16(b"123456789"), 0x29B1, "CRC-16/CCITT-FALSE's own test value");
        assert_ne!(crc16(&[0, 0, 1]), crc16(&[0, 0, 2]));
    }
}
