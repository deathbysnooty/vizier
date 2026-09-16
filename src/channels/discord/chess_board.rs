//! The board picture on the game card.
//!
//! Drawn with tiny-skia like the other cards, and with no image assets and no
//! chess font: every piece is a handful of paths in a 0-100 square, scaled onto
//! whichever square it stands on. That keeps the binary free of a font licence
//! and means the pieces stay sharp at any size the card ever wants.
//!
//! A white piece is a pale shape with a dark outline and a black piece a dark
//! shape with a pale one, so both read on both colours of square. The last move
//! is washed yellow and a king in check red, as everyone expects.
//!
//! Drawing is CPU work, so callers run it on a blocking thread. Pictures are
//! cached by exactly what they show, because a card is redrawn every time
//! anything in the channel moves it back to the bottom, and a position that has
//! not changed must not be painted again.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

use parking_lot::Mutex;
use shakmaty::{Board, Color, Piece, Role, Square};
use tiny_skia::{
    Color as SkColor, FillRule, Paint, Path, PathBuilder, Pixmap, Stroke, Transform,
};

use super::awards_card::{paint, rrect};

/// One square, in pixels.
const SQUARE: f32 = 68.0;
/// The frame round the eight-by-eight, where nothing is drawn.
const EDGE: f32 = 14.0;
pub const SIDE: f32 = 8.0 * SQUARE + 2.0 * EDGE;

const LIGHT: [u8; 3] = [0xE9, 0xDA, 0xB9];
const DARK: [u8; 3] = [0x9C, 0x7A, 0x5B];
const FRAME: [u8; 3] = [0x3E, 0x30, 0x26];
const MOVED: [u8; 3] = [0xF4, 0xD3, 0x5E];
const CHECK: [u8; 3] = [0xE2, 0x56, 0x4A];
const WHITE_PIECE: [u8; 3] = [0xFA, 0xF7, 0xF0];
const WHITE_LINE: [u8; 3] = [0x2A, 0x24, 0x1E];
const BLACK_PIECE: [u8; 3] = [0x30, 0x2A, 0x24];
const BLACK_LINE: [u8; 3] = [0xEC, 0xE6, 0xDA];

/// How many drawn positions are kept. A board is ~40 KB, so this is a few
/// megabytes at the very most, and games rarely share positions after the
/// opening.
const CACHE_MAX: usize = 160;

/// Everything a picture depends on. Two boards that agree on all of this are
/// the same picture, so one may be drawn and the other taken from the cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct View {
    /// The board part of the FEN - the piece placement only.
    pub placement: String,
    /// The squares the last move went between, as "e2e4", or empty.
    pub last: String,
    /// The square of a king in check, as "e1", or empty.
    pub check: String,
    /// True to draw it from black's side.
    pub flipped: bool,
}

type Cache = HashMap<View, std::sync::Arc<Vec<u8>>>;

fn cache() -> &'static Mutex<(Cache, Vec<View>)> {
    static CACHE: OnceLock<Mutex<(Cache, Vec<View>)>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

/// The PNG for a view, drawn once and then remembered.
pub fn board_png(view: &View) -> Option<std::sync::Arc<Vec<u8>>> {
    if let Some(hit) = cache().lock().0.get(view).cloned() {
        return Some(hit);
    }
    let png = std::sync::Arc::new(draw(view)?);
    let mut guard = cache().lock();
    let (map, order) = &mut *guard;
    if map.insert(view.clone(), png.clone()).is_none() {
        order.push(view.clone());
        while order.len() > CACHE_MAX {
            let oldest = order.remove(0);
            map.remove(&oldest);
        }
    }
    Some(png)
}

/// How many pictures are being kept, for the tests.
pub fn cached_count() -> usize {
    cache().lock().0.len()
}

fn square_at(placement_board: &Board, sq: Square) -> Option<Piece> {
    placement_board.piece_at(sq)
}

/// Reads the piece placement field of a FEN into a board.
fn board_of(placement: &str) -> Option<Board> {
    placement.split_whitespace().next()?.parse::<Board>().ok()
}

/// Where a square is drawn: its top-left corner.
fn corner(sq: Square, flipped: bool) -> (f32, f32) {
    let (file, rank) = (sq.file() as u32 as f32, sq.rank() as u32 as f32);
    let (col, row) = if flipped { (7.0 - file, rank) } else { (file, 7.0 - rank) };
    (EDGE + col * SQUARE, EDGE + row * SQUARE)
}

fn parse_square(text: &str) -> Option<Square> {
    let mut chars = text.chars();
    let file = chars.next()?.to_ascii_lowercase() as u32;
    let rank = chars.next()? as u32;
    let f = file.checked_sub('a' as u32).filter(|f| *f < 8)?;
    let r = rank.checked_sub('1' as u32).filter(|r| *r < 8)?;
    Some(Square::new(r * 8 + f))
}

fn draw(view: &View) -> Option<Vec<u8>> {
    let board = board_of(&view.placement)?;
    let side = SIDE as u32;
    let mut px = Pixmap::new(side, side)?;
    px.fill(SkColor::from_rgba8(FRAME[0], FRAME[1], FRAME[2], 255));
    if let Some(edge) = rrect(0.0, 0.0, SIDE, SIDE, 10.0) {
        px.fill_path(&edge, &paint(FRAME, 255), FillRule::Winding, Transform::identity(), None);
    }

    let last: Vec<Square> = if view.last.len() >= 4 {
        [parse_square(&view.last[0..2]), parse_square(&view.last[2..4])].into_iter().flatten().collect()
    } else {
        Vec::new()
    };
    let check = parse_square(&view.check);

    for index in 0..64u32 {
        let sq = Square::new(index);
        let (x, y) = corner(sq, view.flipped);
        let light = (sq.file() as u32 + sq.rank() as u32) % 2 == 1;
        let ground = if light { LIGHT } else { DARK };
        if let Some(rect) = tiny_skia::Rect::from_xywh(x, y, SQUARE, SQUARE) {
            px.fill_rect(rect, &paint(ground, 255), Transform::identity(), None);
            if last.contains(&sq) {
                px.fill_rect(rect, &paint(MOVED, 150), Transform::identity(), None);
            }
            if check == Some(sq) {
                px.fill_rect(rect, &paint(CHECK, 165), Transform::identity(), None);
            }
        }
        // Coordinates in the corners, in the other colour of square, the way a
        // board has them printed: files along the bottom, ranks up the side.
        let ink = if light { DARK } else { LIGHT };
        let bottom_row = if view.flipped { sq.rank() as u32 == 7 } else { sq.rank() as u32 == 0 };
        let left_column = if view.flipped { sq.file() as u32 == 7 } else { sq.file() as u32 == 0 };
        if bottom_row {
            let letter = (b'a' + sq.file() as u32 as u8) as char;
            glyph(&mut px, letter, x + SQUARE - 14.0, y + SQUARE - 21.0, 11.0, ink);
        }
        if left_column {
            let digit = (b'1' + sq.rank() as u32 as u8) as char;
            glyph(&mut px, digit, x + 6.0, y + 6.0, 11.0, ink);
        }
        if let Some(piece) = square_at(&board, sq) {
            piece_on(&mut px, piece, x, y);
        }
    }
    px.encode_png().ok()
}

// --- the pieces -------------------------------------------------------------------------

/// A piece drawn inside the square whose top-left corner is (x, y). The shapes
/// are written in a 0-100 box and scaled here, so the same numbers work at any
/// square size.
fn piece_on(px: &mut Pixmap, piece: Piece, x: f32, y: f32) {
    let (fill, line) = match piece.color {
        Color::White => (WHITE_PIECE, WHITE_LINE),
        Color::Black => (BLACK_PIECE, BLACK_LINE),
    };
    let scale = SQUARE / 100.0;
    let at = Transform::from_translate(x, y).pre_scale(scale, scale);
    let stroke = Stroke { width: 4.0, ..Stroke::default() };
    // A soft shadow under the piece grounds it on both colours of square.
    if let Some(shade) = PathBuilder::from_oval(tiny_skia::Rect::from_xywh(26.0, 82.0, 48.0, 12.0).unwrap()) {
        px.fill_path(&shade, &paint([0, 0, 0], 40), FillRule::Winding, at, None);
    }
    for path in shapes(piece.role) {
        px.fill_path(&path, &paint(fill, 255), FillRule::Winding, at, None);
        px.stroke_path(&path, &paint(line, 255), &stroke, at, None);
    }
    for path in marks(piece.role) {
        px.stroke_path(&path, &paint(line, 255), &Stroke { width: 4.0, ..Stroke::default() }, at, None);
    }
}

fn poly(points: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let (fx, fy) = *points.first()?;
    pb.move_to(fx, fy);
    for (x, y) in &points[1..] {
        pb.line_to(*x, *y);
    }
    pb.close();
    pb.finish()
}

fn base() -> Option<Path> {
    rrect(23.0, 77.0, 54.0, 12.0, 5.0)
}

/// The body of each piece, filled and outlined.
fn shapes(role: Role) -> Vec<Path> {
    let mut out: Vec<Path> = Vec::new();
    let mut push = |p: Option<Path>| {
        if let Some(path) = p {
            out.push(path);
        }
    };
    match role {
        Role::Pawn => {
            push(base());
            push(poly(&[(38.0, 77.0), (41.0, 57.0), (59.0, 57.0), (62.0, 77.0)]));
            push(poly(&[(41.0, 57.0), (44.0, 48.0), (56.0, 48.0), (59.0, 57.0)]));
            push(PathBuilder::from_circle(50.0, 35.0, 14.0));
        }
        Role::Rook => {
            push(base());
            push(poly(&[(30.0, 65.0), (70.0, 65.0), (75.0, 77.0), (25.0, 77.0)]));
            push(poly(&[(36.0, 36.0), (64.0, 36.0), (67.0, 65.0), (33.0, 65.0)]));
            push(poly(&[
                (27.0, 36.0),
                (27.0, 16.0),
                (38.0, 16.0),
                (38.0, 24.0),
                (45.0, 24.0),
                (45.0, 16.0),
                (55.0, 16.0),
                (55.0, 24.0),
                (62.0, 24.0),
                (62.0, 16.0),
                (73.0, 16.0),
                (73.0, 36.0),
            ]));
        }
        Role::Knight => {
            push(base());
            push(poly(&[(30.0, 77.0), (74.0, 77.0), (74.0, 60.0), (70.0, 46.0), (74.0, 33.0), (64.0, 21.0), (50.0, 15.0), (44.0, 22.0), (47.0, 30.0), (32.0, 39.0), (23.0, 55.0), (31.0, 61.0), (39.0, 51.0), (45.0, 55.0), (40.0, 66.0), (34.0, 77.0)]));
        }
        Role::Bishop => {
            push(base());
            push(poly(&[(33.0, 66.0), (67.0, 66.0), (73.0, 77.0), (27.0, 77.0)]));
            push(mitre());
            push(PathBuilder::from_circle(50.0, 15.0, 6.0));
        }
        Role::Queen => {
            push(base());
            push(poly(&[(31.0, 64.0), (69.0, 64.0), (75.0, 77.0), (25.0, 77.0)]));
            push(poly(&[(34.0, 44.0), (66.0, 44.0), (69.0, 64.0), (31.0, 64.0)]));
            push(poly(&[(28.0, 44.0), (22.0, 20.0), (36.0, 36.0), (44.0, 16.0), (50.0, 34.0), (56.0, 16.0), (64.0, 36.0), (78.0, 20.0), (72.0, 44.0)]));
            for (cx, cy) in [(22.0, 19.0), (44.0, 15.0), (56.0, 15.0), (78.0, 19.0)] {
                push(PathBuilder::from_circle(cx, cy, 5.5));
            }
        }
        Role::King => {
            push(base());
            push(poly(&[(31.0, 64.0), (69.0, 64.0), (75.0, 77.0), (25.0, 77.0)]));
            push(poly(&[(34.0, 42.0), (66.0, 42.0), (69.0, 64.0), (31.0, 64.0)]));
            push(poly(&[(30.0, 42.0), (26.0, 26.0), (40.0, 35.0), (50.0, 24.0), (60.0, 35.0), (74.0, 26.0), (70.0, 42.0)]));
            push(poly(&[(46.0, 25.0), (46.0, 17.0), (39.0, 17.0), (39.0, 10.0), (46.0, 10.0), (46.0, 4.0), (54.0, 4.0), (54.0, 10.0), (61.0, 10.0), (61.0, 17.0), (54.0, 17.0), (54.0, 25.0)]));
        }
    }
    out
}

/// The mitre of a bishop: two curves meeting at a point.
fn mitre() -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(50.0, 20.0);
    pb.cubic_to(66.0, 32.0, 70.0, 52.0, 66.0, 66.0);
    pb.line_to(34.0, 66.0);
    pb.cubic_to(30.0, 52.0, 34.0, 32.0, 50.0, 20.0);
    pb.close();
    pb.finish()
}

/// Lines drawn on top of a piece rather than round it: the bishop's slit and
/// the knight's eye.
fn marks(role: Role) -> Vec<Path> {
    let mut out = Vec::new();
    match role {
        Role::Bishop => {
            let mut pb = PathBuilder::new();
            pb.move_to(50.0, 34.0);
            pb.line_to(60.0, 48.0);
            if let Some(p) = pb.finish() {
                out.push(p);
            }
            let mut band = PathBuilder::new();
            band.move_to(34.0, 62.0);
            band.line_to(66.0, 62.0);
            if let Some(p) = band.finish() {
                out.push(p);
            }
        }
        Role::Knight => {
            if let Some(eye) = PathBuilder::from_circle(57.0, 32.0, 2.5) {
                out.push(eye);
            }
            let mut mane = PathBuilder::new();
            mane.move_to(62.0, 24.0);
            mane.line_to(70.0, 40.0);
            if let Some(p) = mane.finish() {
                out.push(p);
            }
        }
        _ => {}
    }
    out
}

// --- the coordinates -----------------------------------------------------------------

/// The letters and digits in the corners of the board, drawn as paths from a
/// tiny seven-segment-style alphabet rather than with a font: only `a`-`h` and
/// `1`-`8` are ever needed, and this keeps the board free of the font lock.
static STROKES: LazyLock<HashMap<char, Vec<Vec<(f32, f32)>>>> = LazyLock::new(|| {
    // Each glyph is a list of polylines in a 0-1 box, top-left origin.
    let mut m: HashMap<char, Vec<Vec<(f32, f32)>>> = HashMap::new();
    m.insert('a', vec![vec![(0.1, 0.45), (0.55, 0.35), (0.85, 0.5), (0.85, 1.0)], vec![(0.85, 0.75), (0.35, 0.8), (0.15, 1.0), (0.5, 1.05), (0.85, 0.9)]]);
    m.insert('b', vec![vec![(0.12, 0.0), (0.12, 1.0)], vec![(0.12, 0.45), (0.55, 0.35), (0.85, 0.7), (0.55, 1.05), (0.12, 0.95)]]);
    m.insert('c', vec![vec![(0.85, 0.45), (0.45, 0.33), (0.12, 0.7), (0.45, 1.05), (0.85, 0.92)]]);
    m.insert('d', vec![vec![(0.85, 0.0), (0.85, 1.0)], vec![(0.85, 0.45), (0.45, 0.35), (0.12, 0.7), (0.45, 1.05), (0.85, 0.95)]]);
    m.insert('e', vec![vec![(0.12, 0.72), (0.85, 0.72), (0.8, 0.42), (0.4, 0.33), (0.12, 0.66), (0.35, 1.03), (0.82, 0.95)]]);
    m.insert('f', vec![vec![(0.8, 0.02), (0.5, 0.0), (0.35, 0.25), (0.35, 1.0)], vec![(0.12, 0.45), (0.7, 0.45)]]);
    m.insert('g', vec![vec![(0.85, 0.35), (0.85, 1.15), (0.5, 1.35), (0.2, 1.2)], vec![(0.85, 0.45), (0.45, 0.35), (0.12, 0.7), (0.45, 1.05), (0.85, 0.95)]]);
    m.insert('h', vec![vec![(0.15, 0.0), (0.15, 1.0)], vec![(0.15, 0.5), (0.5, 0.35), (0.82, 0.55), (0.82, 1.0)]]);
    m.insert('1', vec![vec![(0.3, 0.2), (0.55, 0.0), (0.55, 1.0)], vec![(0.3, 1.0), (0.8, 1.0)]]);
    m.insert('2', vec![vec![(0.15, 0.22), (0.5, 0.0), (0.85, 0.25), (0.6, 0.6), (0.15, 1.0), (0.85, 1.0)]]);
    m.insert('3', vec![vec![(0.15, 0.1), (0.6, 0.0), (0.85, 0.25), (0.5, 0.5), (0.85, 0.75), (0.6, 1.0), (0.15, 0.9)]]);
    m.insert('4', vec![vec![(0.7, 1.0), (0.7, 0.0), (0.12, 0.7), (0.9, 0.7)]]);
    m.insert('5', vec![vec![(0.85, 0.0), (0.2, 0.0), (0.15, 0.45), (0.55, 0.4), (0.85, 0.7), (0.55, 1.02), (0.15, 0.92)]]);
    m.insert('6', vec![vec![(0.8, 0.05), (0.35, 0.1), (0.15, 0.55), (0.15, 0.85), (0.5, 1.03), (0.85, 0.85), (0.6, 0.55), (0.18, 0.62)]]);
    m.insert('7', vec![vec![(0.12, 0.0), (0.88, 0.0), (0.42, 1.0)]]);
    m.insert('8', vec![vec![(0.5, 0.0), (0.18, 0.2), (0.5, 0.45), (0.85, 0.7), (0.5, 1.0), (0.18, 0.75), (0.5, 0.45), (0.82, 0.2), (0.5, 0.0)]]);
    m
});

/// One coordinate glyph, with its top-left at (x, y) and `size` tall.
fn glyph(px: &mut Pixmap, c: char, x: f32, y: f32, size: f32, colour: [u8; 3]) {
    let Some(lines) = STROKES.get(&c) else { return };
    let stroke = Stroke { width: (size * 0.17).max(1.4), line_cap: tiny_skia::LineCap::Round, ..Stroke::default() };
    let width = size * 0.72;
    for line in lines {
        let mut pb = PathBuilder::new();
        for (i, (px_, py_)) in line.iter().enumerate() {
            let (gx, gy) = (x + px_ * width, y + py_ * size);
            if i == 0 {
                pb.move_to(gx, gy);
            } else {
                pb.line_to(gx, gy);
            }
        }
        if let Some(path) = pb.finish() {
            px.stroke_path(&path, &paint(colour, 235), &stroke, Transform::identity(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR";

    fn view(placement: &str) -> View {
        View { placement: placement.to_string(), last: String::new(), check: String::new(), flipped: false }
    }

    #[test]
    fn the_opening_board_draws_as_a_png_of_the_right_size() {
        let png = board_png(&view(START)).expect("a board");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "a PNG");
        let img = image::load_from_memory(&png).expect("readable");
        assert_eq!((img.width(), img.height()), (SIDE as u32, SIDE as u32));
        assert!(png.len() > 2_000, "not a blank square: {} bytes", png.len());
    }

    #[test]
    fn a_position_is_only_drawn_once_and_each_view_is_its_own_picture() {
        let plain = View { placement: "8/8/8/8/8/8/8/4K3".into(), ..view(START) };
        let first = board_png(&plain).expect("drawn");
        let again = board_png(&plain).expect("remembered");
        assert!(std::sync::Arc::ptr_eq(&first, &again), "the same picture came back, not a second painting");
        assert!(cached_count() >= 1);
        assert!(cache().lock().0.contains_key(&plain));

        let flipped = View { flipped: true, ..plain.clone() };
        let lit = View { last: "e1e2".into(), ..plain.clone() };
        let checked = View { check: "e1".into(), ..plain.clone() };
        for other in [&flipped, &lit, &checked] {
            assert_ne!(**board_png(other).expect("drawn"), **first, "{:?} must look different", other);
            assert!(cache().lock().0.contains_key(other), "each view is kept on its own");
        }
    }

    #[test]
    fn the_board_turns_round_for_black() {
        // a1 is bottom-left for white and top-right for black.
        assert_eq!(corner(Square::A1, false), (EDGE, EDGE + 7.0 * SQUARE));
        assert_eq!(corner(Square::A1, true), (EDGE + 7.0 * SQUARE, EDGE));
        assert_eq!(corner(Square::H8, false), (EDGE + 7.0 * SQUARE, EDGE));
        assert_eq!(corner(Square::H8, true), (EDGE, EDGE + 7.0 * SQUARE));
    }

    #[test]
    fn squares_are_read_and_a_wrong_one_is_refused() {
        assert_eq!(parse_square("e4"), Some(Square::E4));
        assert_eq!(parse_square("A1"), Some(Square::A1));
        assert_eq!(parse_square("h8"), Some(Square::H8));
        assert_eq!(parse_square("j9"), None);
        assert_eq!(parse_square("e"), None);
        assert_eq!(parse_square(""), None);
    }

    #[test]
    fn a_broken_placement_draws_nothing_rather_than_panicking() {
        assert!(board_of("not a board").is_none());
        assert!(board_png(&view("not a board")).is_none());
        assert!(board_of(START).is_some());
        // A whole FEN is accepted too: only the first field is read.
        assert!(board_of("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1").is_some());
    }

    /// Writes the boards out as files when `CHESS_SHOTS` names a directory, so
    /// they can be looked at with human eyes. Draws nothing otherwise.
    #[test]
    fn the_boards_can_be_written_out_to_be_looked_at() {
        let Ok(dir) = std::env::var("CHESS_SHOTS") else { return };
        std::fs::create_dir_all(&dir).expect("a place to put them");
        let boards: [(&str, View); 4] = [
            ("start", view(START)),
            (
                "midgame",
                View {
                    placement: "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R".into(),
                    last: "f1c4".into(),
                    check: String::new(),
                    flipped: false,
                },
            ),
            (
                "check",
                View {
                    placement: "rnbqkbnr/ppp2ppp/8/3pp3/6PQ/5P2/PPPPP2P/RNB1KBNR".into(),
                    last: "d1h4".into(),
                    check: "e8".into(),
                    flipped: false,
                },
            ),
            (
                "flipped",
                View {
                    placement: "r1bqkb1r/pppp1ppp/2n2n2/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R".into(),
                    last: "f1c4".into(),
                    check: String::new(),
                    flipped: true,
                },
            ),
        ];
        for (name, v) in boards {
            let png = board_png(&v).expect("a board");
            std::fs::write(format!("{}/board-{}.png", dir, name), &*png).expect("written");
        }
    }

    #[test]
    fn every_piece_has_a_shape_and_both_colours_differ() {
        for role in Role::ALL {
            assert!(!shapes(role).is_empty(), "{:?} has no shape", role);
        }
        let white = board_png(&view("4K3/8/8/8/8/8/8/8")).expect("white king");
        let black = board_png(&view("4k3/8/8/8/8/8/8/8")).expect("black king");
        assert_ne!(*white, *black);
    }
}
