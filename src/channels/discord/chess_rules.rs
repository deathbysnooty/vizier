//! The rules of chess, the clock and the points, with no Discord in sight.
//!
//! Legality, FEN and SAN come from [`shakmaty`]; everything here is the thin
//! layer the game needs on top of it: replaying a move list, reading a typed
//! move in either notation, spotting the endings shakmaty leaves to the players
//! (threefold repetition and the fifty-move rule), the per-move clock, and who
//! is paid what.
//!
//! The SAN move list is the game's source of truth. A position is always
//! rebuilt by replaying it from the start, which is a few microseconds for a
//! whole game and means repetition can be counted without keeping a second
//! history that could drift out of step. The FEN stored beside it is a cache
//! for drawing and for the web page.

use std::collections::HashMap;

use shakmaty::fen::Fen;
use shakmaty::san::{ParseSanError, San, SanPlus};
use shakmaty::uci::UciMove;
use shakmaty::{CastlingMode, Chess, Color, EnPassantMode, Move, Position, Role, Square};

/// How a game ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Checkmate { winner: Color },
    Stalemate,
    /// The same position, with the same rights, for the third time.
    Threefold,
    /// Fifty moves by each side with no capture and no pawn moved.
    FiftyMove,
    /// Neither side has the pieces to mate.
    InsufficientMaterial,
}

impl Ending {
    pub fn winner(self) -> Option<Color> {
        match self {
            Ending::Checkmate { winner } => Some(winner),
            _ => None,
        }
    }

    /// The key stored in the database and shown on the result card.
    pub fn key(self) -> &'static str {
        match self {
            Ending::Checkmate { .. } => "checkmate",
            Ending::Stalemate => "stalemate",
            Ending::Threefold => "threefold",
            Ending::FiftyMove => "fifty",
            Ending::InsufficientMaterial => "material",
        }
    }
}

/// A game rebuilt from its move list.
#[derive(Clone, Debug)]
pub struct Replay {
    pub position: Chess,
    /// Every move so far, in SAN.
    pub sans: Vec<String>,
    /// How often each position has come up, by its FEN without the clocks.
    counts: HashMap<String, u32>,
}

/// The key a repetition is counted by: the board, the side to move, the
/// castling rights and the en-passant square, but not the clocks.
fn repetition_key(pos: &Chess) -> String {
    let fen = Fen::from_position(pos, EnPassantMode::Legal).to_string();
    fen.split_whitespace().take(4).collect::<Vec<_>>().join(" ")
}

impl Replay {
    pub fn new() -> Replay {
        let position = Chess::default();
        let mut counts = HashMap::new();
        counts.insert(repetition_key(&position), 1);
        Replay { position, sans: Vec::new(), counts }
    }

    /// Replays a move list. `Err` names the first move that doesn't fit, which
    /// only a corrupted row could produce.
    pub fn from_sans<S: AsRef<str>>(sans: &[S]) -> Result<Replay, String> {
        let mut replay = Replay::new();
        for san in sans {
            let text = san.as_ref();
            let parsed: San = text.parse().map_err(|_: ParseSanError| format!("couldn't read {}", text))?;
            let m = parsed.to_move(&replay.position).map_err(|_| format!("{} isn't legal here", text))?;
            replay.push(m);
        }
        Ok(replay)
    }

    /// Plays a move that is known to be legal in this position.
    pub fn push(&mut self, m: Move) {
        let san = SanPlus::from_move_and_play_unchecked(&mut self.position, m).to_string();
        self.sans.push(san);
        *self.counts.entry(repetition_key(&self.position)).or_insert(0) += 1;
    }

    pub fn fen(&self) -> String {
        Fen::from_position(&self.position, EnPassantMode::Legal).to_string()
    }

    pub fn turn(&self) -> Color {
        self.position.turn()
    }

    /// Plies played, so 1. e4 e5 is two.
    pub fn plies(&self) -> usize {
        self.sans.len()
    }

    /// The move number a card shows: whole moves, counting from 1.
    pub fn move_number(&self) -> u32 {
        self.position.fullmoves().get()
    }

    /// How many times the position now on the board has been seen.
    pub fn repetitions(&self) -> u32 {
        self.counts.get(&repetition_key(&self.position)).copied().unwrap_or(1)
    }

    /// The ending the position is in, if any. Checked in the order the laws
    /// give them: mate and stalemate first, because they end the game outright.
    pub fn ending(&self) -> Option<Ending> {
        if self.position.is_checkmate() {
            return Some(Ending::Checkmate { winner: !self.position.turn() });
        }
        if self.position.is_stalemate() {
            return Some(Ending::Stalemate);
        }
        if self.position.is_insufficient_material() {
            return Some(Ending::InsufficientMaterial);
        }
        if self.repetitions() >= 3 {
            return Some(Ending::Threefold);
        }
        if self.position.halfmoves() >= 100 {
            return Some(Ending::FiftyMove);
        }
        None
    }
}

impl Default for Replay {
    fn default() -> Self {
        Replay::new()
    }
}

// --- reading a typed move ---------------------------------------------------------------

/// Why a typed move was refused, in words a player can act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveError {
    /// Nothing that could be a move at all.
    Unreadable,
    /// Read, but not legal here.
    Illegal,
    /// Read, but several pieces could do it (e.g. "Nd2" with knights on b1 and f3).
    Ambiguous,
    /// A long move that needs a promotion piece and didn't name one.
    NeedsPromotion,
}

impl MoveError {
    /// The ephemeral reply a player gets.
    pub fn words(self, typed: &str) -> String {
        let typed = tidy(typed);
        match self {
            MoveError::Unreadable => format!(
                "❓ **{}** doesn't look like a move. Try SAN like `Nf3`, `exd5`, `O-O`, `e8=Q` — or the squares, like `g1f3`.",
                typed
            ),
            MoveError::Illegal => format!("🚫 **{}** isn't legal in this position. It's still your move.", typed),
            MoveError::Ambiguous => {
                format!("🤔 **{}** could be more than one piece. Say which file or rank, like `Nbd2`, or type the squares: `b1d2`.", typed)
            }
            MoveError::NeedsPromotion => {
                format!("👑 **{}** is a promotion — add the piece, like `{}q` (or `e8=Q` in SAN).", typed, typed)
            }
        }
    }
}

/// What a player typed, with the decoration that doesn't change the move taken
/// off: spaces, check and mate marks, "e.p.", and the annotation glyphs.
pub fn tidy(input: &str) -> String {
    input.trim().chars().filter(|c| !c.is_whitespace()).collect::<String>().chars().take(20).collect()
}

/// Castling written every way people actually write it, folded to SAN's own.
fn castling(input: &str) -> Option<&'static str> {
    let folded: String = input.chars().filter(|c| !matches!(c, '-' | '+' | '#')).collect::<String>().to_ascii_lowercase();
    match folded.as_str() {
        "oo" | "00" => Some("O-O"),
        "ooo" | "000" => Some("O-O-O"),
        _ => None,
    }
}

/// Reads a move in SAN ("Nf3", "O-O", "exd5", "Qxf7#", "e8=Q") or in long
/// algebraic ("g1f3", "e7e8q", "e2-e4"), and returns it if it is legal.
pub fn read_move(pos: &Chess, input: &str) -> Result<Move, MoveError> {
    let typed = tidy(input);
    if typed.is_empty() {
        return Err(MoveError::Unreadable);
    }
    if let Some(castle) = castling(&typed) {
        let san: San = castle.parse().map_err(|_: ParseSanError| MoveError::Unreadable)?;
        return san.to_move(pos).map_err(|_| MoveError::Illegal);
    }
    // Long algebraic first, since "b1c3" is not SAN and "e4" is not long.
    if let Some(m) = read_long(pos, &typed)? {
        return Ok(m);
    }
    let cleaned: String = typed.chars().filter(|c| !matches!(c, '!' | '?')).collect();
    let cleaned = cleaned.strip_suffix("e.p.").unwrap_or(&cleaned).to_string();
    match cleaned.parse::<San>() {
        Ok(san) => match san.to_move(pos) {
            Ok(m) => Ok(m),
            Err(shakmaty::san::SanError::AmbiguousSan) => Err(MoveError::Ambiguous),
            Err(_) => Err(MoveError::Illegal),
        },
        Err(_) => Err(MoveError::Unreadable),
    }
}

fn square_of(file: char, rank: char) -> Option<Square> {
    let f = (file as u32).checked_sub('a' as u32).filter(|f| *f < 8)?;
    let r = (rank as u32).checked_sub('1' as u32).filter(|r| *r < 8)?;
    Some(Square::new(r * 8 + f))
}

fn promotion_role(c: char) -> Option<Role> {
    match c.to_ascii_lowercase() {
        'q' => Some(Role::Queen),
        'r' => Some(Role::Rook),
        'b' => Some(Role::Bishop),
        'n' => Some(Role::Knight),
        _ => None,
    }
}

/// `Ok(None)` when the text isn't long algebraic at all, so SAN can have a go.
fn read_long(pos: &Chess, typed: &str) -> Result<Option<Move>, MoveError> {
    let raw: Vec<char> =
        typed.chars().filter(|c| !matches!(c, '-' | 'x' | 'X' | '=' | '+' | '#' | '!' | '?')).collect();
    if raw.len() < 4 || raw.len() > 5 {
        return Ok(None);
    }
    let lower: Vec<char> = raw.iter().map(|c| c.to_ascii_lowercase()).collect();
    let (Some(from), Some(to)) = (square_of(lower[0], lower[1]), square_of(lower[2], lower[3])) else {
        return Ok(None);
    };
    let promotion = match raw.get(4) {
        Some(c) => match promotion_role(*c) {
            Some(role) => Some(role),
            None => return Ok(None),
        },
        None => None,
    };
    let legal = pos.legal_moves();
    let matching: Vec<&Move> = legal
        .iter()
        .filter(|m| {
            let uci = UciMove::from_move(**m, CastlingMode::Standard);
            matches!(uci, UciMove::Normal { from: f, to: t, promotion: p } if f == from && t == to && p == promotion)
        })
        .collect();
    match matching.as_slice() {
        [one] => Ok(Some(**one)),
        [] => {
            // A promotion typed without its piece is worth its own message.
            let needs = promotion.is_none()
                && legal.iter().any(|m| {
                    matches!(UciMove::from_move(*m, CastlingMode::Standard), UciMove::Normal { from: f, to: t, promotion: Some(_) } if f == from && t == to)
                });
            if needs { Err(MoveError::NeedsPromotion) } else { Err(MoveError::Illegal) }
        }
        _ => Err(MoveError::Ambiguous),
    }
}

/// The long-algebraic form of a move, as the web page sends it back.
pub fn long_form(m: Move) -> String {
    UciMove::from_move(m, CastlingMode::Standard).to_string()
}

/// The two squares a move is drawn between: where the piece left and where it
/// landed. Castling is drawn on the KING's squares, not the rook's, which is
/// what shakmaty's `to()` gives.
pub fn highlight(m: Move) -> (Square, Square) {
    match m {
        Move::Castle { king, rook } => {
            let to = if rook > king { Square::new(king.to_u32() + 2) } else { Square::new(king.to_u32() - 2) };
            (king, to)
        }
        other => (other.from().unwrap_or(other.to()), other.to()),
    }
}

/// One position of a game, for a board to be drawn from: the watching page
/// steps through these rather than working the moves out for itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub fen: String,
    /// The squares the move into this position went between, as "e2e4"; empty
    /// for the position the game started from.
    pub last: String,
    /// The square of a king in check, or empty.
    pub check: String,
    /// The move in SAN, or empty for the starting position.
    pub san: String,
}

/// Every position a move list passed through, oldest first, starting with the
/// board before a piece was touched.
pub fn frames<S: AsRef<str>>(sans: &[S]) -> Vec<Frame> {
    let mut replay = Replay::new();
    let mut out = vec![Frame { fen: replay.fen(), last: String::new(), check: String::new(), san: String::new() }];
    for text in sans {
        let Ok(san) = text.as_ref().parse::<San>() else { break };
        let Ok(m) = san.to_move(&replay.position) else { break };
        let (from, to) = highlight(m);
        replay.push(m);
        let check = if replay.position.is_check() {
            replay.position.board().king_of(replay.position.turn()).map(|sq| sq.to_string()).unwrap_or_default()
        } else {
            String::new()
        };
        out.push(Frame {
            fen: replay.fen(),
            last: format!("{}{}", from, to),
            check,
            san: replay.sans.last().cloned().unwrap_or_default(),
        });
    }
    out
}

/// The squares the last move of a list went between, as "e2e4".
pub fn last_move_squares<S: AsRef<str>>(sans: &[S]) -> Option<String> {
    frames(sans).pop().map(|f| f.last).filter(|l| !l.is_empty())
}

// --- the clock -------------------------------------------------------------------------

/// How long a side has per move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeControl {
    /// Hours per move, for a game played over a day or two.
    Casual,
    /// Seconds per move, for a game played in one sitting.
    Live,
}

impl TimeControl {
    pub fn key(self) -> &'static str {
        match self {
            TimeControl::Casual => "casual",
            TimeControl::Live => "live",
        }
    }

    pub fn from_key(key: &str) -> TimeControl {
        match key {
            "live" => TimeControl::Live,
            _ => TimeControl::Casual,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeControl::Casual => "Casual",
            TimeControl::Live => "Live",
        }
    }
}

/// When the side to move runs out: the moment of the last move plus the
/// allowance. A game that has not started counts from when it was created.
pub fn deadline(last_move_ts: i64, per_move_secs: i64) -> i64 {
    last_move_ts + per_move_secs.max(1)
}

/// Whether the side to move has run out of time.
pub fn flagged(last_move_ts: i64, per_move_secs: i64, now: i64) -> bool {
    now >= deadline(last_move_ts, per_move_secs)
}

/// Seconds left for the side to move, never below zero.
pub fn seconds_left(last_move_ts: i64, per_move_secs: i64, now: i64) -> i64 {
    (deadline(last_move_ts, per_move_secs) - now).max(0)
}

/// A clock as a card shows it: "11 h 42 m", "4 m 05 s", "38 s".
pub fn clock_words(secs: i64) -> String {
    let secs = secs.max(0);
    if secs >= 3600 {
        format!("{} h {:02} m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} m {:02} s", secs / 60, secs % 60)
    } else {
        format!("{} s", secs)
    }
}

/// How long a game lasted, in the words the result card uses.
pub fn span_words(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        s if s >= 86_400 => format!("{} d {} h", s / 86_400, (s % 86_400) / 3600),
        s if s >= 3600 => format!("{} h {} m", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{} m", s / 60),
        s => format!("{} s", s),
    }
}

// --- colours ---------------------------------------------------------------------------

/// Who plays white. `roll` is a number in `0.0..1.0`, passed in so a draw can
/// be reproduced in a test.
pub fn assign_colours(challenger: u64, opponent: u64, roll: f64) -> (u64, u64) {
    if roll < 0.5 { (challenger, opponent) } else { (opponent, challenger) }
}

// --- the move list ----------------------------------------------------------------------

/// The move list as the card shows it: "1. e4 e5 2. Nf3 Nc6", wrapped so no
/// line runs past `width`, and cut from the FRONT when it gets long - the last
/// moves are the ones anyone reads.
pub fn move_list(sans: &[String], width: usize, max_lines: usize) -> String {
    if sans.is_empty() {
        return "No moves yet.".to_string();
    }
    let mut pairs: Vec<String> = Vec::new();
    for (i, chunk) in sans.chunks(2).enumerate() {
        let mut text = format!("{}. {}", i + 1, chunk[0]);
        if let Some(black) = chunk.get(1) {
            text.push(' ');
            text.push_str(black);
        }
        pairs.push(text);
    }
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for pair in pairs {
        if !line.is_empty() && line.chars().count() + pair.chars().count() + 2 > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push_str("  ");
        }
        line.push_str(&pair);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    let cut = lines.len() > max_lines;
    if cut {
        lines = lines.split_off(lines.len() - max_lines);
    }
    let body = lines.join("\n");
    if cut { format!("…\n{}", body) } else { body }
}

// --- points -----------------------------------------------------------------------------

/// What a finished game is worth, before the ledger's own cap and rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Payout {
    /// Chess points for white and for black.
    pub white: i64,
    pub black: i64,
    /// Why nothing is paid, for the result card. Empty when it does pay.
    pub why_nothing: &'static str,
}

impl Payout {
    pub const NOTHING: Payout = Payout { white: 0, black: 0, why_nothing: "" };

    pub fn pays(&self) -> bool {
        self.white > 0 || self.black > 0
    }
}

/// How a game ended, as far as the points care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Result_ {
    Win { winner: Color },
    Draw,
    /// A mod cancelled it.
    Cancelled,
}

/// What the two players score.
///
/// Chess points belong to the game, not to the House Cup, so who is in which
/// house makes no difference here any more: two friends in one house play for
/// the same score as anyone else. A resignation inside the first `min_plies` is
/// still worth nothing, so two friends can't farm a board by resigning at move
/// two; a game lost on TIME does score, however short, because the winner did
/// nothing wrong.
pub fn payout(result: Result_, by_resignation: bool, plies: usize, min_plies: usize, win: i64, draw: i64) -> Payout {
    if matches!(result, Result_::Cancelled) {
        return Payout { why_nothing: "the game was cancelled", ..Payout::NOTHING };
    }
    if by_resignation && plies < min_plies {
        return Payout { why_nothing: "it was given up too early to count", ..Payout::NOTHING };
    }
    match result {
        Result_::Win { winner } => match winner {
            Color::White => Payout { white: win.max(0), black: 0, why_nothing: "" },
            Color::Black => Payout { white: 0, black: win.max(0), why_nothing: "" },
        },
        Result_::Draw => Payout { white: draw.max(0), black: draw.max(0), why_nothing: "" },
        Result_::Cancelled => Payout::NOTHING,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A position set up from a FEN, for the endings that take a hundred moves
    /// to reach by hand.
    fn from_fen(fen: &str) -> Replay {
        let position: Chess =
            fen.parse::<Fen>().expect("a fen").into_position(CastlingMode::Standard).expect("a legal position");
        let mut counts = HashMap::new();
        counts.insert(repetition_key(&position), 1);
        Replay { position, sans: Vec::new(), counts }
    }

    fn play(moves: &[&str]) -> Replay {
        let mut replay = Replay::new();
        for text in moves {
            let m = read_move(&replay.position, text).unwrap_or_else(|e| panic!("{} refused: {:?}", text, e));
            replay.push(m);
        }
        replay
    }

    #[test]
    fn the_opening_position_has_the_moves_it_should() {
        let replay = Replay::new();
        assert_eq!(replay.position.legal_moves().len(), 20);
        assert_eq!(replay.turn(), Color::White);
        assert_eq!(replay.move_number(), 1);
        assert!(replay.ending().is_none());
        assert!(replay.fen().starts_with("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq -"));
    }

    #[test]
    fn san_and_long_algebraic_reach_the_same_move() {
        for text in ["e4", "e2e4", "e2-e4"] {
            let mut replay = Replay::new();
            let m = read_move(&replay.position, text).expect("e4");
            replay.push(m);
            assert_eq!(replay.sans, vec!["e4".to_string()], "{} should be e4", text);
        }
        // Case and spacing are forgiven on the long form only: SAN's case means something.
        let replay = Replay::new();
        assert!(read_move(&replay.position, " G1F3 ").is_ok());
        assert!(matches!(read_move(&replay.position, "nf3"), Err(MoveError::Unreadable | MoveError::Illegal)));
    }

    #[test]
    fn castling_is_read_however_it_is_written() {
        let replay = play(&["e4", "e5", "Nf3", "Nc6", "Bc4", "Bc5"]);
        for text in ["O-O", "0-0", "oo", "00", "e1g1"] {
            let m = read_move(&replay.position, text).unwrap_or_else(|e| panic!("{}: {:?}", text, e));
            assert!(matches!(m, Move::Castle { .. }), "{} should castle", text);
            let (from, to) = highlight(m);
            assert_eq!((from, to), (Square::E1, Square::G1), "{} is drawn on the king's squares", text);
        }
        // Long castling from the same position is not available yet.
        assert!(read_move(&replay.position, "O-O-O").is_err());
    }

    #[test]
    fn en_passant_and_promotion_work_both_ways() {
        let replay = play(&["e4", "d5", "e5", "f5"]);
        let m = read_move(&replay.position, "exf6").expect("en passant in SAN");
        assert!(m.is_en_passant());
        assert!(read_move(&replay.position, "e5f6").expect("en passant long").is_en_passant());

        // A pawn on the seventh: the long form must name the piece it becomes.
        let mut race = from_fen("r3k3/1P6/8/8/8/8/8/4K3 w - - 0 20");
        assert_eq!(race.turn(), Color::White);
        assert!(read_move(&race.position, "b8").is_err(), "a promotion without a piece is no move");
        assert_eq!(read_move(&race.position, "b7b8"), Err(MoveError::NeedsPromotion));
        let knight = read_move(&race.position, "b7b8n").expect("underpromotion");
        assert!(matches!(knight, Move::Normal { promotion: Some(Role::Knight), .. }));
        let queen = read_move(&race.position, "bxa8=Q").expect("promotion with a capture");
        race.push(queen);
        assert_eq!(race.sans.last().map(String::as_str), Some("bxa8=Q+"));
    }

    #[test]
    fn an_ambiguous_move_is_refused_rather_than_guessed() {
        // Knights on b1 and f3 can both reach d2 once the d-pawn is out of the way.
        let replay = play(&["d4", "d5", "Nf3", "Nf6"]);
        assert_eq!(read_move(&replay.position, "Nd2"), Err(MoveError::Ambiguous));
        assert!(read_move(&replay.position, "Nbd2").is_ok(), "saying which knight is enough");
        assert!(read_move(&replay.position, "b1d2").is_ok(), "so is naming the squares");
        assert_eq!(read_move(&replay.position, "Nh5"), Err(MoveError::Illegal));
        assert_eq!(read_move(&replay.position, "hello"), Err(MoveError::Unreadable));
        assert_eq!(read_move(&replay.position, ""), Err(MoveError::Unreadable));
        assert!(MoveError::Ambiguous.words("Ne4").contains("Ne4"));
    }

    #[test]
    fn the_fools_mate_is_mate_and_the_stalemate_is_a_draw() {
        let mate = play(&["f3", "e5", "g4", "Qh4"]);
        assert_eq!(mate.ending(), Some(Ending::Checkmate { winner: Color::Black }));
        assert_eq!(mate.ending().unwrap().key(), "checkmate");
        assert_eq!(mate.sans.last().map(String::as_str), Some("Qh4#"));

        // The shortest stalemate there is.
        let stale = play(&[
            "e3", "a5", "Qh5", "Ra6", "Qxa5", "h5", "Qxc7", "Rah6", "h4", "f6", "Qxd7+", "Kf7", "Qxb7", "Qd3",
            "Qxb8", "Qh7", "Qxc8", "Kg6", "Qe6",
        ]);
        assert_eq!(stale.ending(), Some(Ending::Stalemate));
        assert_eq!(stale.ending().unwrap().winner(), None);
    }

    #[test]
    fn a_position_seen_three_times_is_a_draw_and_two_kings_are_too() {
        let mut replay = Replay::new();
        assert_eq!(replay.repetitions(), 1);
        for text in ["Nf3", "Nf6", "Ng1", "Ng8", "Nf3", "Nf6", "Ng1"] {
            let m = read_move(&replay.position, text).expect("shuffle");
            replay.push(m);
            assert!(replay.ending().is_none(), "not yet after {}", text);
        }
        let m = read_move(&replay.position, "Ng8").expect("back to the start a third time");
        replay.push(m);
        assert_eq!(replay.repetitions(), 3);
        assert_eq!(replay.ending(), Some(Ending::Threefold));

        let bare = from_fen("8/8/4k3/8/8/3K4/8/8 w - - 0 60");
        assert_eq!(bare.ending(), Some(Ending::InsufficientMaterial));
    }

    #[test]
    fn a_hundred_quiet_half_moves_end_the_game() {
        let mut replay = from_fen("8/8/4k3/8/8/3K4/7R/7r w - - 99 60");
        assert!(replay.ending().is_none(), "99 is not yet fifty moves each");
        let m = read_move(&replay.position, "Kc3").expect("a quiet move");
        replay.push(m);
        assert_eq!(replay.ending(), Some(Ending::FiftyMove));
    }

    #[test]
    fn a_move_list_is_rebuilt_exactly_and_a_broken_one_says_so() {
        let sans = vec!["e4".to_string(), "e5".to_string(), "Nf3".to_string()];
        let replay = Replay::from_sans(&sans).expect("legal");
        assert_eq!(replay.sans, sans);
        assert_eq!(replay.plies(), 3);
        assert_eq!(replay.move_number(), 2);
        assert_eq!(replay.turn(), Color::Black);
        assert!(Replay::from_sans(&["e4", "e5", "Nf9"]).is_err());
        assert!(Replay::from_sans(&["e4", "e4"]).unwrap_err().contains("legal"));
    }

    #[test]
    fn a_game_is_broken_into_the_positions_it_passed_through() {
        let sans: Vec<String> = ["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7#"].iter().map(|s| s.to_string()).collect();
        let played = frames(&sans);
        assert_eq!(played.len(), sans.len() + 1, "the board before the first move counts");
        assert_eq!(played[0].san, "");
        assert_eq!(played[0].last, "");
        assert_eq!(played[0].check, "");
        assert!(played[0].fen.starts_with("rnbqkbnr/pppppppp"));
        assert_eq!(played[1].san, "e4");
        assert_eq!(played[1].last, "e2e4");
        let last = played.last().unwrap();
        assert_eq!(last.san, "Qxf7#");
        assert_eq!(last.last, "h5f7");
        assert_eq!(last.check, "e8", "the mated king is lit");
        assert_eq!(last_move_squares(&sans).as_deref(), Some("h5f7"));

        // Castling is drawn on the king's squares, and a game with no moves has none.
        let castled: Vec<String> = ["e4", "e5", "Nf3", "Nc6", "Bc4", "Bc5", "O-O"].iter().map(|s| s.to_string()).collect();
        assert_eq!(last_move_squares(&castled).as_deref(), Some("e1g1"));
        assert_eq!(frames(&Vec::<String>::new()).len(), 1);
        assert_eq!(last_move_squares(&Vec::<String>::new()), None);
        // A broken list stops where it breaks rather than panicking.
        assert_eq!(frames(&["e4".to_string(), "Nf9".to_string()]).len(), 2);
    }

    #[test]
    fn the_clock_counts_down_and_flags() {
        let (start, hour) = (1_000_000i64, 3600i64);
        assert_eq!(deadline(start, 12 * hour), start + 12 * hour);
        assert_eq!(seconds_left(start, 12 * hour, start), 12 * hour);
        assert!(!flagged(start, 12 * hour, start + 12 * hour - 1));
        assert!(flagged(start, 12 * hour, start + 12 * hour));
        assert_eq!(seconds_left(start, 12 * hour, start + 13 * hour), 0, "never below zero");
        assert_eq!(clock_words(12 * hour - 18 * 60), "11 h 42 m");
        assert_eq!(clock_words(245), "4 m 05 s");
        assert_eq!(clock_words(38), "38 s");
        assert_eq!(clock_words(-5), "0 s");
        assert_eq!(span_words(6 * hour + 12 * 60), "6 h 12 m");
        assert_eq!(span_words(90), "1 m");
        assert_eq!(span_words(2 * 86_400 + 3 * hour), "2 d 3 h");
    }

    #[test]
    fn colours_are_drawn_and_both_players_can_be_white() {
        assert_eq!(assign_colours(7, 9, 0.1), (7, 9));
        assert_eq!(assign_colours(7, 9, 0.9), (9, 7));
        assert_eq!(assign_colours(7, 9, 0.5), (9, 7), "the halfway roll goes to the challenged player");
    }

    #[test]
    fn the_move_list_wraps_and_keeps_the_end() {
        assert_eq!(move_list(&[], 40, 3), "No moves yet.");
        let sans: Vec<String> = ["e4", "e5", "Nf3", "Nc6", "Bc4"].iter().map(|s| s.to_string()).collect();
        assert_eq!(move_list(&sans, 60, 3), "1. e4 e5  2. Nf3 Nc6  3. Bc4");
        let wrapped = move_list(&sans, 20, 3);
        assert!(wrapped.lines().all(|l| l.chars().count() <= 20), "{:?}", wrapped);
        let long: Vec<String> = (0..60).map(|_| "Nf3".to_string()).collect();
        let cut = move_list(&long, 40, 3);
        assert!(cut.starts_with("…\n"), "a long game shows its last lines: {:?}", cut);
        assert_eq!(cut.lines().count(), 4);
    }

    #[test]
    fn every_real_game_scores_and_a_quick_resignation_scores_nothing() {
        let win = |plies, resigned| payout(Result_::Win { winner: Color::White }, resigned, plies, 10, 4, 1);
        assert_eq!(win(40, false), Payout { white: 4, black: 0, why_nothing: "" });
        assert_eq!(win(40, true), Payout { white: 4, black: 0, why_nothing: "" }, "a real game given up still scores");
        assert!(!win(6, true).pays());
        assert_eq!(win(6, true).why_nothing, "it was given up too early to count");
        assert!(win(6, false).pays(), "a six-ply MATE is a win, however silly");

        let drawn = payout(Result_::Draw, false, 80, 10, 4, 1);
        assert_eq!((drawn.white, drawn.black), (1, 1));
        assert!(!payout(Result_::Cancelled, false, 80, 10, 4, 1).pays());
        assert_eq!(payout(Result_::Cancelled, false, 80, 10, 4, 1).why_nothing, "the game was cancelled");
        // A timeout is not a resignation, so even a short one scores.
        assert!(payout(Result_::Win { winner: Color::Black }, false, 3, 10, 4, 1).pays());
    }
}
