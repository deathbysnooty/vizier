//! The chess behind the puzzle: setting a position up from the bank, judging a
//! solver's move, and playing the opponent's reply.
//!
//! Everything a move has to be to be legal is decided here, by the same reader
//! the chess game uses ([`super::chess_rules::read_move`]), so the page can be
//! as wrong as it likes without a bad move ever landing.
//!
//! Two fairness rules live here, and both matter:
//!
//! 1. **Any move that mates is right.** Half the bank is mates, and a solver who
//!    finds a DIFFERENT mate has solved the puzzle: refusing it because it is
//!    not the recorded move would be plainly wrong to the player. A mating move
//!    ends the attempt there and then, whatever the recorded line still had to
//!    say.
//! 2. **Nothing cleverer than that.** There is no engine here, so an
//!    "equally good" move that is not a mate cannot be told from a bad one. Any
//!    other move that is not the recorded one is refused — and refused
//!    gently: the attempt goes on and they may try again.

use shakmaty::fen::Fen;
use shakmaty::san::SanPlus;
use shakmaty::{CastlingMode, Chess, Color, EnPassantMode, Position};

use super::chess_rules::read_move;

/// A position from a FEN, or nothing when the FEN isn't one.
pub fn position_of(fen: &str) -> Option<Chess> {
    fen.parse::<Fen>().ok()?.into_position(CastlingMode::Standard).ok()
}

/// The FEN a position writes itself as.
pub fn fen_of(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

/// Plays a move written in UCI (`e2e4`, `e7e8q`, `e1g1` for castling, `e5d6`
/// for an en-passant capture) and gives back the position after it. `None` when
/// the move isn't legal here, which is also how a promotion missing its piece
/// and a castle written as the rook's move are turned away.
pub fn play(pos: &Chess, uci: &str) -> Option<Chess> {
    let m = read_move(pos, uci).ok()?;
    pos.clone().play(m).ok()
}

/// The same move in the notation a person reads: `Nf3`, `O-O`, `e8=Q+`, `Qxf7#`.
pub fn san_of(pos: &Chess, uci: &str) -> Option<String> {
    let m = read_move(pos, uci).ok()?;
    Some(SanPlus::from_move(pos.clone(), m).to_string())
}

/// The two squares a UCI move goes between, as the board picture wants them
/// ("e2e4"). Castling is drawn on the king's squares.
pub fn squares_of(pos: &Chess, uci: &str) -> Option<String> {
    let m = read_move(pos, uci).ok()?;
    let (from, to) = super::chess_rules::highlight(m);
    Some(format!("{}{}", from, to))
}

/// The square of the king that is in check, or empty.
pub fn check_square(pos: &Chess) -> String {
    if !pos.is_check() {
        return String::new();
    }
    pos.board().king_of(pos.turn()).map(|sq| sq.to_string()).unwrap_or_default()
}

/// What the channel is shown, worked out from a bank row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opening {
    /// The position the solver sees, as a FEN.
    pub fen: String,
    /// The opponent's move that reached it, in the notation a person reads.
    pub setup_san: String,
    /// The squares it went between, for the picture's highlight.
    pub setup_squares: String,
    /// Whether the solver plays white.
    pub solver_is_white: bool,
}

/// Sets a puzzle up: the bank's FEN is the position BEFORE the opponent's move,
/// so that move is played here and the result is what everybody is shown. The
/// solver's colour is whichever side is to move AFTER it — the opposite of the
/// FEN's own side to move.
///
/// `None` when the FEN or the opponent's move doesn't hold up, which keeps a bad
/// row in the pack out of the channel rather than putting up a nonsense board.
pub fn open_with(fen: &str, setup: &str) -> Option<Opening> {
    let before = position_of(fen)?;
    let setup_san = san_of(&before, setup)?;
    let setup_squares = squares_of(&before, setup)?;
    let after = play(&before, setup)?;
    Some(Opening { fen: fen_of(&after), setup_san, setup_squares, solver_is_white: after.turn() == Color::White })
}

/// What became of a move somebody played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The recorded move. The line goes on.
    Right,
    /// Checkmate — theirs or the book's, it makes no difference. The puzzle is
    /// solved outright, however much of the recorded line was left.
    Mate,
    /// Not the move, and not a mate either. They may try again.
    Wrong,
}

/// Judges one solver move against the line. `expected` is the move the bank
/// recorded for this turn.
///
/// The order matters: a mate is checked FIRST, so a solver who mates by another
/// road is told they have solved it rather than told they are wrong. A move that
/// isn't legal at all is simply wrong — the page should never send one, and a
/// person typing into the box certainly can.
pub fn judge(pos: &Chess, typed: &str, expected: &str) -> (Verdict, Option<Chess>) {
    let Some(after) = play(pos, typed) else {
        return (Verdict::Wrong, None);
    };
    if after.is_checkmate() {
        return (Verdict::Mate, Some(after));
    }
    // The same move can be written more than one way, so the two are compared
    // as POSITIONS rather than as text: `e1g1` and `O-O` reach the same board.
    let recorded = play(pos, expected);
    match recorded {
        Some(book) if fen_of(&book) == fen_of(&after) => (Verdict::Right, Some(after)),
        _ => (Verdict::Wrong, None),
    }
}

/// What a wrong move is answered with. One line, the same every time: nothing
/// here may hint at what the move should have been.
pub const NOT_IT: &str = "That's not it — try again.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::puzzle_bank::tests::fixture;

    /// The four shipped rows the bank fixture holds, by id.
    fn bank_row(id: &str) -> super::super::puzzle_bank::Puzzle {
        fixture().get(id).expect("a puzzle").clone()
    }

    #[test]
    fn the_opponents_move_is_played_before_anybody_sees_the_board() {
        // The worked example from the bank's own guide: the FEN says black to
        // move, black plays Bc8, and the member is therefore playing WHITE.
        let p = bank_row("WqnOB");
        let before = position_of(&p.fen).expect("a legal position");
        assert_eq!(before.turn(), Color::Black, "the stored FEN is one move early");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        assert_eq!(open.setup_san, "Bc8", "the blunder that set the puzzle up");
        assert_eq!(open.setup_squares, "e6c8");
        assert!(open.solver_is_white, "the solver plays the side to move AFTER the opponent's move");
        // The board everybody is shown is NOT the stored one: the bishop has moved.
        assert_ne!(open.fen, p.fen);
        assert!(open.fen.starts_with("2b3k1/"), "the bishop is on c8: {}", open.fen);
        let shown = position_of(&open.fen).expect("the shown position");
        assert_eq!(shown.turn(), Color::White);
        // And the solver's first move is legal in THAT position, not in the stored one.
        assert!(play(&shown, p.solver_move(0).unwrap()).is_some());
        assert!(play(&before, p.solver_move(0).unwrap()).is_none(), "it is not even legal a move earlier");
    }

    #[test]
    fn a_line_is_accepted_move_by_move_and_a_wrong_move_doesnt_end_it() {
        let p = bank_row("WqnOB");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        let mut pos = position_of(&open.fen).expect("the shown position");

        // A wrong move: refused, and the position has not moved on, so the next
        // try is judged against exactly the same board.
        let before = fen_of(&pos);
        let (verdict, after) = judge(&pos, "g2f3", p.solver_move(0).unwrap());
        assert_eq!((verdict, after), (Verdict::Wrong, None));
        // One that isn't a legal move at all is simply wrong, not a crash.
        assert_eq!(judge(&pos, "g2g9", p.solver_move(0).unwrap()).0, Verdict::Wrong);
        assert_eq!(judge(&pos, "", p.solver_move(0).unwrap()).0, Verdict::Wrong);
        assert_eq!(fen_of(&pos), before, "a refusal changes nothing");

        // The recorded move: right, and the line goes on.
        let (verdict, after) = judge(&pos, p.solver_move(0).unwrap(), p.solver_move(0).unwrap());
        assert_eq!(verdict, Verdict::Right);
        pos = after.expect("the position after it");
        assert_eq!(san_of(&position_of(&open.fen).unwrap(), "g2d5").as_deref(), Some("Bd5+"));

        // The opponent's reply is played for them.
        pos = play(&pos, p.reply_to(0).unwrap()).expect("the reply");
        // And the last move of the line finishes it.
        let (verdict, after) = judge(&pos, p.solver_move(1).unwrap(), p.solver_move(1).unwrap());
        assert_eq!(verdict, Verdict::Right);
        assert!(after.is_some());
        assert_eq!(p.solver_move(2), None, "there is nothing after it");
    }

    #[test]
    fn any_move_that_mates_is_right_even_when_it_is_not_the_recorded_one() {
        // Half the bank is mates. A solver who finds a different mate has solved
        // the puzzle, and being told "that's not it" would be plainly wrong.
        let p = bank_row("4kYFv");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        let pos = position_of(&open.fen).expect("the shown position");
        assert_eq!(san_of(&pos, "d1h5").as_deref(), Some("Qxh5#"));
        // The recorded move mates, so it comes back as a mate rather than as a
        // plain "right": either way the puzzle is over.
        assert_eq!(judge(&pos, "d1h5", "d1h5").0, Verdict::Mate);
        // Now pretend the book wanted something else entirely. The mate still wins.
        let (verdict, after) = judge(&pos, "d1h5", "a2a3");
        assert_eq!(verdict, Verdict::Mate);
        assert!(after.expect("the mating position").is_checkmate());
        // A move that is merely good is still refused: there is no engine here.
        assert_eq!(judge(&pos, "h5f7", "d1h5").0, Verdict::Wrong);
    }

    #[test]
    fn a_promotion_a_castle_and_an_en_passant_capture_all_play() {
        // The three moves a naive UCI handler gets wrong, each from a real row.

        // A promotion as the opponent's setting-up move: f7f8q.
        let p = bank_row("Zs7l3");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        assert_eq!(open.setup_san, "f8=Q", "a promotion is played, piece and all");
        assert!(!open.solver_is_white, "white promoted, so the solver is black");
        assert!(open.fen.starts_with("3K1Q2/"), "the new queen stands on f8: {}", open.fen);
        // And a promotion written WITHOUT its piece is not a move at all.
        assert!(play(&position_of(&p.fen).unwrap(), "f7f8").is_none());

        // A promotion the SOLVER plays: b7b8q, the first move of the line.
        let p = bank_row("65RdP");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        let pos = position_of(&open.fen).expect("the shown position");
        assert_eq!(san_of(&pos, "b7b8q").as_deref(), Some("b8=Q+"));
        let (verdict, after) = judge(&pos, "b7b8q", p.solver_move(0).unwrap());
        assert_eq!(verdict, Verdict::Right);
        assert!(after.expect("the position after it").is_check());
        // The same squares with the WRONG piece on the end is a different move.
        assert_eq!(judge(&pos, "b7b8n", p.solver_move(0).unwrap()).0, Verdict::Wrong);

        // A castle, written as the KING's two-square move: e1c1 is O-O-O.
        let p = bank_row("LhHU9");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        assert_eq!(open.setup_san, "O-O-O", "the king's two-square move castles");
        assert_eq!(open.setup_squares, "e1c1", "and is drawn on the king's squares, not the rook's");
        assert!(open.fen.starts_with("3rkb1r/pp3ppp/8/1Np1Pb2/1n3B2/4PP2/PPPnB1PP/2KR2NR"), "the rook came with it: {}", open.fen);
        assert!(!open.solver_is_white);
        let pos = position_of(&open.fen).expect("the shown position");
        assert_eq!(judge(&pos, "b4a2", "b4a2").0, Verdict::Mate, "Nxa2#");
        // The rook ends up on d1 because the KING's move was played: a reader
        // that took e1c1 for a plain king step would leave the rook on a1.
        assert!(!open.fen.starts_with("3rkb1r/pp3ppp/8/1Np1Pb2/1n3B2/4PP2/PPPnB1PP/R1K"), "{}", open.fen);

        // An en-passant capture, which the solver plays: the pawn's diagonal
        // move onto an empty square, taking the pawn beside it.
        let p = bank_row("I4ZCY");
        let open = open_with(&p.fen, &p.setup).expect("the opening");
        let mut pos = position_of(&open.fen).expect("the shown position");
        assert!(open.solver_is_white);
        pos = judge(&pos, p.solver_move(0).unwrap(), p.solver_move(0).unwrap()).1.expect("the recapture");
        pos = play(&pos, p.reply_to(0).unwrap()).expect("the pawn coming two squares");
        assert_eq!(san_of(&pos, "e5f6").as_deref(), Some("exf6"), "the capture is written as the pawn's move");
        let (verdict, after) = judge(&pos, "e5f6", p.solver_move(1).unwrap());
        assert_eq!(verdict, Verdict::Right);
        let after = after.expect("the position after it");
        // The capture is the whole point: the black pawn that had just come two
        // squares is taken, even though nothing stood on the square landed on.
        let pawns = |pos: &Chess| fen_of(pos).split(' ').next().unwrap_or_default().matches('p').count();
        assert_eq!(pawns(&after) + 1, pawns(&pos), "one black pawn fewer: {}", fen_of(&after));
        let board = fen_of(&after);
        let ranks: Vec<&str> = board.split(' ').next().unwrap_or_default().split('/').collect();
        assert!(ranks[2].contains('P'), "a white pawn stands on the sixth rank: {}", ranks[2]);
        assert_eq!(ranks[3], "8", "and the rank it captured past is empty: {}", ranks[3]);
    }

    #[test]
    fn every_shipped_puzzle_sets_up_and_its_whole_line_plays() {
        // The pack is verified where it is built; this is the check that THIS
        // code can run what it ships, move for move.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("puzzlebank");
        let Ok(bank) = super::super::puzzle_bank::load(&dir) else {
            return; // Not every checkout has the pack.
        };
        let mut checked = 0;
        // Every hundredth puzzle: the whole pack takes a while, and a systematic
        // fault would show up in forty of them as readily as in four thousand.
        for (n, id) in (0..).zip(["4kYFv", "WqnOB", "Zs7l3", "NydFH"]) {
            let _ = n;
            assert!(bank.get(id).is_some(), "{} is in the pack", id);
        }
        for p in (0..bank.count()).step_by(100).filter_map(|i| bank.nth(i)) {
            let open = open_with(&p.fen, &p.setup).unwrap_or_else(|| panic!("{} does not set up", p.id));
            let mut pos = position_of(&open.fen).unwrap_or_else(|| panic!("{} has no position", p.id));
            assert_eq!(pos.turn() == Color::White, open.solver_is_white, "{}", p.id);
            for (i, uci) in p.line.iter().enumerate() {
                pos = play(&pos, uci).unwrap_or_else(|| panic!("{} move {} ({}) is not legal", p.id, i, uci));
            }
            checked += 1;
        }
        assert!(checked > 30, "only {} puzzles checked", checked);
    }
}
