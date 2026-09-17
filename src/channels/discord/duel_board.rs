//! The board picture on a Letter Duel card.
//!
//! Drawn the way the sudoku card is — tiny-skia for the squares and the tiles,
//! cosmic-text for the letters, so the board looks the same on a phone as in
//! the panel. The premium squares are coloured and named, a tile is a cream
//! rounded square with its letter big and its value small in the corner, and
//! the squares the last play covered are ringed in gold so everyone can see
//! what just happened.
//!
//! Drawing is CPU work, so callers run it on a blocking thread. A picture is
//! kept under exactly what it shows, because a card is redrawn whenever
//! anything in the channel moves it back to the bottom and a board that has not
//! changed must not be painted twice.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use parking_lot::Mutex;
use tiny_skia::{Color as SkColor, FillRule, Pixmap, Transform};

use super::awards_card::{blend_rect, paint, rrect};
use super::duel_rules::{self as rules, Premium, SIZE};

/// One square, in pixels, and the frame round the fifteen-by-fifteen where the
/// column letters and row numbers go.
const CELL: f32 = 42.0;
const EDGE: f32 = 26.0;
pub const SIDE: f32 = SIZE as f32 * CELL + 2.0 * EDGE;

const BG: [u8; 3] = [21, 18, 15];
const FRAME: [u8; 3] = [46, 38, 30];
const PLAIN: [u8; 3] = [32, 62, 46];
const DOUBLE_LETTER: [u8; 3] = [93, 148, 184];
const TRIPLE_LETTER: [u8; 3] = [40, 96, 150];
const DOUBLE_WORD: [u8; 3] = [178, 102, 120];
const TRIPLE_WORD: [u8; 3] = [178, 62, 52];
const CENTRE: [u8; 3] = [186, 143, 47];
const TILE: [u8; 3] = [236, 219, 175];
const TILE_EDGE: [u8; 3] = [170, 148, 106];
const INK: [u8; 3] = [42, 33, 24];
const LABEL: [u8; 3] = [228, 235, 231];
const FRESH: [u8; 3] = [244, 211, 94];
const COORD: [u8; 3] = [150, 140, 126];

/// How many boards are kept. One is around 30 KB.
const CACHE_MAX: usize = 40;

/// Everything a picture depends on. Two boards that agree on all of this are
/// the same picture, so one may be drawn and the other taken from the cache.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct View {
    /// The 225 characters of the board.
    pub board: String,
    /// The squares the last play covered, smallest first.
    pub fresh: Vec<usize>,
}

impl View {
    pub fn new(board: &str, fresh: &[usize]) -> View {
        let mut fresh = fresh.to_vec();
        fresh.sort_unstable();
        fresh.dedup();
        View { board: board.to_string(), fresh }
    }
}

type Cache = HashMap<View, Arc<Vec<u8>>>;

fn cache() -> &'static Mutex<(Cache, Vec<View>)> {
    static CACHE: OnceLock<Mutex<(Cache, Vec<View>)>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new((HashMap::new(), Vec::new())))
}

fn pick_family(fs: &FontSystem) -> String {
    for name in ["Montserrat", "Avenir Next", "Noto Sans", "DejaVu Sans"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

/// What colour a square is painted when nothing is on it.
fn ground(square: Premium) -> [u8; 3] {
    match square {
        Premium::Plain => PLAIN,
        Premium::DoubleLetter => DOUBLE_LETTER,
        Premium::TripleLetter => TRIPLE_LETTER,
        Premium::DoubleWord => DOUBLE_WORD,
        Premium::TripleWord => TRIPLE_WORD,
        Premium::Centre => CENTRE,
    }
}

/// Where a square is drawn: its top-left corner.
fn corner(at: usize) -> (f32, f32) {
    (EDGE + (at % SIZE) as f32 * CELL, EDGE + (at / SIZE) as f32 * CELL)
}

/// One run of text, centred on a point, drawn with the font the card uses.
#[allow(clippy::too_many_arguments)]
fn centred(px: &mut Pixmap, fs: &mut FontSystem, cache: &mut SwashCache, family: &str, text: &str, cx: f32, cy: f32, size: f32, weight: Weight, colour: [u8; 3]) {
    if text.is_empty() {
        return;
    }
    let mut buf = Buffer::new(fs, Metrics::new(size, size * 1.3));
    buf.set_size(fs, None, None);
    buf.set_text(fs, text, Attrs::new().family(Family::Name(family)).weight(weight), Shaping::Advanced);
    buf.shape_until_scroll(fs, false);
    let (line_y, width) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, size * 0.5));
    let ox = (cx - width / 2.0).round() as i32;
    let oy = (cy + size * 0.36 - line_y).round() as i32;
    buf.draw(fs, cache, Color::rgb(colour[0], colour[1], colour[2]), |gx, gy, w, h, c| {
        blend_rect(px, ox + gx, oy + gy, w, h, c);
    });
}

/// The board as a PNG, or `None` if drawing failed.
pub fn render(view: &View, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let family = pick_family(fs);
    let cells = rules::cells(&view.board);
    let mut px = Pixmap::new(SIDE as u32, SIDE as u32)?;
    px.fill(SkColor::from_rgba8(BG[0], BG[1], BG[2], 255));
    if let Some(frame) = rrect(EDGE - 8.0, EDGE - 8.0, SIZE as f32 * CELL + 16.0, SIZE as f32 * CELL + 16.0, 12.0) {
        px.fill_path(&frame, &paint(FRAME, 255), FillRule::Winding, Transform::identity(), None);
    }
    let mut swash = SwashCache::new();

    for at in 0..rules::SQUARES {
        let (x, y) = corner(at);
        let square = rules::premium(at);
        // The square itself, always: a tile is drawn a little smaller on top,
        // so the premium under it still shows as a thin edge.
        if let Some(bed) = rrect(x + 1.0, y + 1.0, CELL - 2.0, CELL - 2.0, 4.0) {
            px.fill_path(&bed, &paint(ground(square), 255), FillRule::Winding, Transform::identity(), None);
        }
        match cells.get(at).copied().filter(|c| *c != rules::EMPTY) {
            None => {
                // An empty premium square says what it is.
                let (text, size) = match square {
                    Premium::Plain => ("", 0.0),
                    Premium::Centre => ("★", CELL * 0.44),
                    other => (other.label(), CELL * 0.29),
                };
                centred(&mut px, fs, &mut swash, &family, text, x + CELL / 2.0, y + CELL / 2.0, size, Weight::BOLD, LABEL);
            }
            Some(tile) => {
                if let Some(face) = rrect(x + 3.0, y + 3.0, CELL - 6.0, CELL - 6.0, 5.0) {
                    px.fill_path(&face, &paint(TILE, 255), FillRule::Winding, Transform::identity(), None);
                    px.stroke_path(
                        &face,
                        &paint(TILE_EDGE, 255),
                        &tiny_skia::Stroke { width: 1.4, ..tiny_skia::Stroke::default() },
                        Transform::identity(),
                        None,
                    );
                }
                let letter = tile.to_ascii_uppercase().to_string();
                centred(&mut px, fs, &mut swash, &family, &letter, x + CELL / 2.0 - 1.5, y + CELL / 2.0 - 1.0, CELL * 0.56, Weight::BOLD, INK);
                // A blank is worth nothing and shows no number, which is how
                // you tell one from the letter it is standing in for.
                let worth = rules::value(tile);
                if worth > 0 {
                    centred(
                        &mut px,
                        fs,
                        &mut swash,
                        &family,
                        &worth.to_string(),
                        x + CELL - 9.5,
                        y + CELL - 9.0,
                        CELL * 0.26,
                        Weight::SEMIBOLD,
                        INK,
                    );
                }
                if view.fresh.contains(&at) {
                    if let Some(ring) = rrect(x + 2.0, y + 2.0, CELL - 4.0, CELL - 4.0, 5.0) {
                        px.stroke_path(
                            &ring,
                            &paint(FRESH, 255),
                            &tiny_skia::Stroke { width: 2.6, ..tiny_skia::Stroke::default() },
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
        }
    }

    // The column letters along the top and the row numbers down the left, the
    // way a board has them printed, so the card and the page agree on names.
    for i in 0..SIZE {
        let letter = ((b'A' + i as u8) as char).to_string();
        let number = (i + 1).to_string();
        let (x, y) = corner(i);
        centred(&mut px, fs, &mut swash, &family, &letter, x + CELL / 2.0, EDGE / 2.0 - 1.0, 15.0, Weight::SEMIBOLD, COORD);
        let (_, y) = (x, corner(i * SIZE).1);
        centred(&mut px, fs, &mut swash, &family, &number, EDGE / 2.0 - 1.0, y + CELL / 2.0, 14.0, Weight::SEMIBOLD, COORD);
    }
    px.encode_png().ok()
}

/// The picture for a view, drawn once and then remembered.
pub fn board_png(view: &View) -> Option<Arc<Vec<u8>>> {
    if let Some(hit) = cache().lock().0.get(view).cloned() {
        return Some(hit);
    }
    let drawn = {
        let mut fs = super::awards::fonts().lock();
        render(view, &mut fs)?
    };
    let png = Arc::new(drawn);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::duel_rules::{Placement, check, empty_board};

    fn lay(board: &str, at: usize, word: &str, down: bool) -> String {
        let mut cells = rules::cells(board);
        let step = if down { SIZE } else { 1 };
        for (i, c) in word.chars().enumerate() {
            cells[at + i * step] = c;
        }
        cells.into_iter().collect()
    }

    #[test]
    fn an_empty_board_draws_as_a_png_of_the_right_size() {
        let png = board_png(&View::new(&empty_board(), &[])).expect("a board");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "a PNG");
        let decoded = Pixmap::decode_png(&png).expect("readable png");
        assert_eq!((decoded.width(), decoded.height()), (SIDE as u32, SIDE as u32));
        // The premium squares are painted, so the picture is not one colour.
        let colours: std::collections::HashSet<[u8; 4]> =
            decoded.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colours.len() > 8, "the board looks blank: {} colours", colours.len());
    }

    #[test]
    fn a_board_is_only_drawn_once_and_each_view_is_its_own_picture() {
        let board = lay(&empty_board(), rules::CENTRE, "QUIZ", false);
        let plain = View::new(&board, &[]);
        let first = board_png(&plain).expect("drawn");
        let again = board_png(&plain).expect("remembered");
        assert!(Arc::ptr_eq(&first, &again), "the same picture came back, not a second painting");
        assert!(cached_count() >= 1);

        let lit = View::new(&board, &[rules::CENTRE]);
        assert_ne!(**board_png(&lit).expect("drawn"), **first, "the last play has to show");
        let more = View::new(&lay(&board, rules::CENTRE + SIZE, "AT", false), &[]);
        assert_ne!(**board_png(&more).expect("drawn"), **first);
    }

    #[test]
    fn the_squares_a_view_lights_up_are_sorted_and_never_repeated() {
        let view = View::new("....", &[9, 3, 9, 1]);
        assert_eq!(view.fresh, vec![1, 3, 9]);
        assert_eq!(View::new("....", &[1, 3, 9]), view, "the same picture, however the squares came in");
    }

    #[test]
    fn a_blank_is_drawn_without_a_number_so_it_reads_as_a_blank() {
        // Lower case is a blank on the board; it must not look like the letter.
        let letter = View::new(&lay(&empty_board(), rules::CENTRE, "Q", false), &[]);
        let blank = View::new(&lay(&empty_board(), rules::CENTRE, "q", false), &[]);
        assert_ne!(**board_png(&letter).expect("drawn"), **board_png(&blank).expect("drawn"));
        assert_eq!(rules::value('q'), 0);
    }

    #[test]
    fn a_broken_board_draws_something_rather_than_panicking() {
        assert!(board_png(&View::new("", &[])).is_some(), "a short board is padded out");
        assert!(board_png(&View::new(&"Z".repeat(400), &[])).is_some(), "a long one is cut down");
        assert!(board_png(&View::new(&empty_board(), &[9_999])).is_some(), "a square off the board lights nothing");
    }

    /// Writes a board out as a file when `DUEL_SHOTS` names a directory, so it
    /// can be looked at with human eyes. Draws nothing otherwise — but the
    /// position it draws is played through the real rules either way, so the
    /// picture is always one that could actually happen.
    #[test]
    fn the_board_can_be_written_out_to_be_looked_at() {
        let bank = crate::channels::discord::duel_words::tests::real_or_fixture();
        if !bank.knows("quartz") {
            return; // no real bank in this checkout
        }
        let known = |w: &str| bank.knows(w);
        let square = |name: &str| {
            let col = name.as_bytes()[0] - b'A';
            let row: usize = name[1..].parse().expect("a row");
            (row - 1) * SIZE + col as usize
        };
        let mut board = empty_board();
        let mut fresh: Vec<usize> = Vec::new();
        // QUARTZ across the middle, ANTE hanging down off its A, ZIP down off
        // its Z, and SPEAR across through the E of ANTE.
        let plays: [(&str, &str, bool); 3] = [("F8", "QUARTZ", false), ("H9", "NTE", true), ("K9", "IP", true)];
        for (at, letters, down) in plays {
            let start = square(at);
            let step = if down { SIZE } else { 1 };
            let tiles: Vec<Placement> =
                letters.chars().enumerate().map(|(i, c)| Placement { at: start + i * step, letter: c, blank: false }).collect();
            let rack: String = letters.chars().collect();
            let play = check(&board, &rack, &tiles, known).unwrap_or_else(|why| panic!("{} {}: {}", at, letters, why.words()));
            board = play.board;
            fresh = play.covered;
        }
        // SPEAR across row 11, reaching round the E that ANTE already put
        // there: four tiles down, one square skipped, one word.
        let tiles: Vec<Placement> = [("F11", 'S'), ("G11", 'P'), ("I11", 'A'), ("J11", 'R')]
            .into_iter()
            .map(|(name, letter)| Placement { at: square(name), letter, blank: false })
            .collect();
        let play = check(&board, "SPAR???", &tiles, known).unwrap_or_else(|why| panic!("SPEAR: {}", why.words()));
        assert_eq!(play.headline(), "SPEAR");
        board = play.board;
        fresh = play.covered;

        let Ok(dir) = std::env::var("DUEL_SHOTS") else { return };
        std::fs::create_dir_all(&dir).expect("a place to put them");
        for (name, view) in [("empty", View::new(&empty_board(), &[])), ("game", View::new(&board, &fresh))] {
            let png = board_png(&view).expect("a board");
            std::fs::write(format!("{}/board-{}.png", dir, name), &**png).expect("written");
        }
    }
}
