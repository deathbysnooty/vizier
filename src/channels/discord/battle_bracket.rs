//! The battle royale bracket: the whole draw as one picture, posted before each
//! round and once more with the champion. `battle.rs` owns the tournament; this
//! only draws what it is handed.

use super::battle_theme::Theme;
use super::house::House;

/// Someone in the draw.
pub struct Entrant {
    pub name: String,
    /// Raw profile picture bytes, already downloaded; `None` draws a blank.
    pub avatar: Option<Vec<u8>>,
    pub house: Option<&'static House>,
}

/// One match in the draw. `a` and `b` index `Bracket::entrants`; `None` is a
/// slot nobody has reached yet, or the empty side of a free pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub a: Option<usize>,
    pub b: Option<usize>,
    /// 0 if `a` won, 1 if `b` won.
    pub winner: Option<usize>,
    /// The winner's health left at the end of the fight.
    pub hp: Option<i32>,
    /// A first-round free pass: `a` goes through without a fight.
    pub bye: bool,
    /// Being fought right now.
    pub live: bool,
}

pub struct Bracket<'a> {
    pub entrants: &'a [Entrant],
    /// Every round, first to last; round `r` holds `2^(rounds - 1 - r)` matches in
    /// draw order, so matches `2j` and `2j + 1` feed match `j` of the next round.
    /// The final is the last round, with one match.
    pub rounds: &'a [Vec<Slot>],
    /// "16 warriors · Quarter-finals".
    pub subtitle: String,
    pub theme: Theme,
}

/// What a round is called on the bracket, by how many matches it has.
pub fn round_title(matches: usize) -> String {
    match matches {
        1 => "FINAL".to_string(),
        2 => "SEMI-FINALS".to_string(),
        4 => "QUARTER-FINALS".to_string(),
        n => format!("ROUND OF {}", n * 2),
    }
}

/// PNG bytes of the bracket, or `None` if drawing failed.
pub fn bracket_png(_bracket: &Bracket) -> Option<Vec<u8>> {
    None
}
