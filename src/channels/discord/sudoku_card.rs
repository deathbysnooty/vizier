//! The picture on a sudoku card: the grid as it was posted.
//!
//! Drawn the way the other cards are — tiny-skia for the lines, cosmic-text for
//! the digits, so the numbers look the same on a phone as in the panel. Dark
//! board, white givens, thick lines around each three-by-three box. The same
//! puzzle is only ever drawn once: the PNG is kept until the channel has moved
//! well past that puzzle.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Weight};
use parking_lot::Mutex;
use tiny_skia::{Color as SkColor, FillRule, Pixmap, Rect, Transform};

use super::awards_card::{paint, rrect};
use super::sudoku_gen::Grid;

/// The picture's side, in pixels.
pub const SIDE: f32 = 540.0;
const MARGIN: f32 = 18.0;
const CELL: f32 = (SIDE - 2.0 * MARGIN) / 9.0;
const THIN: f32 = 1.0;
const THICK: f32 = 3.0;

const BG: [u8; 3] = [20, 21, 26];
const BOARD: [u8; 3] = [37, 40, 47];
const BOX_TINT: [u8; 3] = [27, 29, 36];
const THIN_LINE: [u8; 3] = [64, 68, 79];
const THICK_LINE: [u8; 3] = [132, 138, 152];
const INK: [u8; 3] = [238, 240, 245];

fn pick_family(fs: &FontSystem) -> String {
    for name in ["Montserrat", "Avenir Next", "Noto Sans"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

/// A rectangle on whole pixels, with no anti-aliasing: grid lines are thin and
/// straight, and tiny-skia's hairline drawing is best left alone.
fn fill(px: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 3]) {
    let mut p = paint(c, 255);
    p.anti_alias = false;
    if let Some(r) = Rect::from_xywh(x.round(), y.round(), w.round().max(1.0), h.round().max(1.0)) {
        px.fill_rect(r, &p, Transform::identity(), None);
    }
}

/// The grid as a PNG, or `None` if drawing failed.
pub fn render(givens: &Grid, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let family = pick_family(fs);
    let mut px = Pixmap::new(SIDE as u32, SIDE as u32)?;
    px.fill(SkColor::from_rgba8(BG[0], BG[1], BG[2], 255));
    if let Some(board) = rrect(MARGIN - 6.0, MARGIN - 6.0, 9.0 * CELL + 12.0, 9.0 * CELL + 12.0, 10.0) {
        px.fill_path(&board, &paint(BOARD, 255), FillRule::Winding, Transform::identity(), None);
    }

    // A slightly darker wash on the four corner boxes, so the three-by-threes
    // read even before the lines do.
    for box_row in 0..3 {
        for box_col in 0..3 {
            if (box_row + box_col) % 2 == 1 {
                continue;
            }
            fill(
                &mut px,
                MARGIN + box_col as f32 * 3.0 * CELL,
                MARGIN + box_row as f32 * 3.0 * CELL,
                3.0 * CELL,
                3.0 * CELL,
                BOX_TINT,
            );
        }
    }

    for i in 0..=9 {
        let thick = i % 3 == 0;
        let (w, c) = if thick { (THICK, THICK_LINE) } else { (THIN, THIN_LINE) };
        let at = MARGIN + i as f32 * CELL - w / 2.0;
        fill(&mut px, at, MARGIN - w / 2.0, w, 9.0 * CELL + w, c);
        fill(&mut px, MARGIN - w / 2.0, at, 9.0 * CELL + w, w, c);
    }

    let size = CELL * 0.56;
    let mut cache = cosmic_text::SwashCache::new();
    for (cell, digit) in givens.iter().enumerate() {
        if *digit == 0 {
            continue;
        }
        let text = digit.to_string();
        let mut buf = Buffer::new(fs, Metrics::new(size, size * 1.3));
        buf.set_size(fs, None, None);
        buf.set_text(fs, &text, Attrs::new().family(Family::Name(family.as_str())).weight(Weight::BOLD), Shaping::Advanced);
        buf.shape_until_scroll(fs, false);
        let (line_y, width) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, size * 0.5));
        let cx = MARGIN + (cell % 9) as f32 * CELL + CELL / 2.0;
        let cy = MARGIN + (cell / 9) as f32 * CELL + CELL / 2.0;
        let ox = (cx - width / 2.0).round() as i32;
        let oy = (cy + size * 0.36 - line_y).round() as i32;
        buf.draw(fs, &mut cache, Color::rgb(INK[0], INK[1], INK[2]), |gx, gy, w, h, c| {
            super::awards_card::blend_rect(&mut px, ox + gx, oy + gy, w, h, c);
        });
    }
    px.encode_png().ok()
}

type Cache = HashMap<i64, Arc<Vec<u8>>>;

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many puzzles' pictures are kept before the oldest are dropped.
const KEEP: usize = 12;

/// The picture for a puzzle, drawn once and then remembered. Drawing is real
/// work, so it never happens on the gateway thread.
pub async fn png(puzzle_id: i64, givens: Grid) -> Option<Arc<Vec<u8>>> {
    if let Some(png) = cache().lock().get(&puzzle_id).cloned() {
        return Some(png);
    }
    let drawn = tokio::task::spawn_blocking(move || {
        let mut fs = super::awards::fonts().lock();
        render(&givens, &mut fs)
    })
    .await
    .ok()
    .flatten()?;
    let png = Arc::new(drawn);
    let mut cache = cache().lock();
    if cache.len() >= KEEP {
        let oldest: Vec<i64> = {
            let mut ids: Vec<i64> = cache.keys().copied().collect();
            ids.sort_unstable();
            ids.into_iter().take(cache.len() + 1 - KEEP).collect()
        };
        for id in oldest {
            cache.remove(&id);
        }
    }
    cache.insert(puzzle_id, png.clone());
    Some(png)
}

/// What the picture is called on the message.
pub fn file_name(puzzle_id: i64) -> String {
    format!("sudoku-{}.png", puzzle_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::sudoku_gen::{Level, Rng, generate};

    #[test]
    fn the_grid_draws_to_a_png_of_the_right_size() {
        let p = generate(Level::Easy, &mut Rng::seeded(3));
        let mut fs = crate::channels::discord::awards::fonts().lock();
        let png = render(&p.givens, &mut fs).expect("a picture");
        assert_eq!(&png[1..4], b"PNG");
        let decoded = Pixmap::decode_png(&png).expect("readable png");
        assert_eq!((decoded.width(), decoded.height()), (SIDE as u32, SIDE as u32));
        // The board is drawn, not left as flat background.
        let colours: std::collections::HashSet<[u8; 4]> =
            decoded.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colours.len() > 4, "the grid looks blank");
    }

    #[test]
    fn a_picture_is_named_after_its_puzzle() {
        assert_eq!(file_name(128), "sudoku-128.png");
    }
}
