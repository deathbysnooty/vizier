//! The picture on a Guess the Word card: somebody's doodle, drawn out again.
//!
//! Drawn the way the other cards are - tiny-skia for the ink, cosmic-text for
//! the words on it - but it is meant to look like a drawing on paper and not a
//! plot: a warm off-white sheet, near-black ink a few pixels thick, round caps
//! and round joins so no stroke ends in a corner, and the drawing scaled to fill
//! its square with a margin. A stroke of one point is a dot somebody meant to
//! make, and is drawn as one.
//!
//! One drawing to a card, or two side by side once `!hint` has put a second
//! drawing of the same thing up. The picture for a round is drawn once and then
//! remembered, and the drawing itself happens off the async runtime, the way the
//! sudoku card does it.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Weight};
use parking_lot::Mutex;
use tiny_skia::{Color as SkColor, FillRule, LineCap, LineJoin, PathBuilder, Pixmap, Stroke, Transform};

use super::awards_card::{fill_circle, paint, rrect};
use super::guess_bank::Doodle;

/// One drawing's square, and the room around it.
pub const PANEL: f32 = 520.0;
const MARGIN: f32 = 30.0;
/// The band at the top the title and the round number sit in.
const TITLE_BAND: f32 = 62.0;
/// Between two drawings, once a hint has put a second one up.
const GAP: f32 = 26.0;
/// The white space inside a panel, so no stroke touches the next drawing.
const PAD: f32 = 26.0;

/// Warm paper, and the ink on it.
const PAPER: [u8; 3] = [250, 246, 238];
const EDGE: [u8; 3] = [228, 220, 204];
const INK: [u8; 3] = [38, 34, 30];
const TITLE: [u8; 3] = [74, 66, 58];
const MUTED: [u8; 3] = [150, 138, 122];

/// The most drawings one card shows: the doodle, and a hint's second one.
pub const MOST: usize = 2;

/// What the card says above the drawing.
pub const CARD_TITLE: &str = "What is this?";

/// How wide and how tall a card with this many drawings on it is.
pub fn card_size(drawings: usize) -> (u32, u32) {
    let n = drawings.clamp(1, MOST) as f32;
    let width = 2.0 * MARGIN + n * PANEL + (n - 1.0) * GAP;
    (width as u32, (2.0 * MARGIN + TITLE_BAND + PANEL) as u32)
}

fn pick_family(fs: &FontSystem) -> String {
    for name in ["Montserrat", "Avenir Next", "Noto Sans"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

/// One line of text, drawn from its left edge (or its right, when `right` is
/// the edge it should end at).
#[allow(clippy::too_many_arguments)]
fn write(
    px: &mut Pixmap,
    fs: &mut FontSystem,
    cache: &mut cosmic_text::SwashCache,
    family: &str,
    text: &str,
    size: f32,
    weight: Weight,
    colour: [u8; 3],
    at: (f32, f32),
    right: bool,
) {
    let mut buf = Buffer::new(fs, Metrics::new(size, size * 1.3));
    buf.set_size(fs, None, None);
    buf.set_text(fs, text, Attrs::new().family(Family::Name(family)).weight(weight), Shaping::Advanced);
    buf.shape_until_scroll(fs, false);
    let (line_y, width) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
    let ox = (if right { at.0 - width } else { at.0 }).round() as i32;
    let oy = (at.1 - line_y).round() as i32;
    buf.draw(fs, cache, Color::rgb(colour[0], colour[1], colour[2]), |gx, gy, w, h, c| {
        super::awards_card::blend_rect(px, ox + gx, oy + gy, w, h, c);
    });
}

/// The box a drawing really fills, which is not always the whole 0-255 square.
fn bounds(doodle: &Doodle) -> Option<(f32, f32, f32, f32)> {
    let mut seen = false;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (255.0f32, 255.0f32, 0.0f32, 0.0f32);
    for (x, y) in doodle.iter().flatten() {
        seen = true;
        min_x = min_x.min(*x as f32);
        min_y = min_y.min(*y as f32);
        max_x = max_x.max(*x as f32);
        max_y = max_y.max(*y as f32);
    }
    seen.then_some((min_x, min_y, max_x, max_y))
}

/// One drawing, scaled into a square of `side` at `(x, y)` and inked in.
fn draw_doodle(px: &mut Pixmap, doodle: &Doodle, x: f32, y: f32, side: f32) {
    let Some((min_x, min_y, max_x, max_y)) = bounds(doodle) else { return };
    let (w, h) = ((max_x - min_x).max(1.0), (max_y - min_y).max(1.0));
    let room = side - 2.0 * PAD;
    let scale = (room / w).min(room / h);
    // Centred on what the drawing actually covers, so a wide doodle sits in the
    // middle of its square rather than up against one edge.
    let ox = x + (side - w * scale) / 2.0 - min_x * scale;
    let oy = y + (side - h * scale) / 2.0 - min_y * scale;
    let at = |(px, py): &(u8, u8)| (ox + *px as f32 * scale, oy + *py as f32 * scale);
    let width = (side / 92.0).max(2.0);
    let pen = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
    let ink = paint(INK, 248);
    for stroke in doodle {
        match stroke.len() {
            0 => {}
            // A dot - an eye, a freckle - is a stroke like any other and belongs
            // on the paper.
            1 => {
                let (cx, cy) = at(&stroke[0]);
                fill_circle(px, cx, cy, width * 0.55, INK);
            }
            _ => {
                let mut pb = PathBuilder::new();
                let (sx, sy) = at(&stroke[0]);
                pb.move_to(sx, sy);
                for point in &stroke[1..] {
                    let (lx, ly) = at(point);
                    pb.line_to(lx, ly);
                }
                if let Some(path) = pb.finish() {
                    px.stroke_path(&path, &ink, &pen, Transform::identity(), None);
                }
            }
        }
    }
}

/// The card as a PNG, or `None` if drawing failed. More drawings than a card
/// holds are simply not shown.
pub fn render(round: i64, doodles: &[Doodle], fs: &mut FontSystem) -> Option<Vec<u8>> {
    let doodles: &[Doodle] = &doodles[..doodles.len().min(MOST)];
    let (width, height) = card_size(doodles.len().max(1));
    let family = pick_family(fs);
    let mut px = Pixmap::new(width, height)?;
    px.fill(SkColor::from_rgba8(EDGE[0], EDGE[1], EDGE[2], 255));
    // The sheet of paper itself, a hair inside the edge.
    if let Some(sheet) = rrect(4.0, 4.0, width as f32 - 8.0, height as f32 - 8.0, 16.0) {
        px.fill_path(&sheet, &paint(PAPER, 255), FillRule::Winding, Transform::identity(), None);
    }
    let mut cache = cosmic_text::SwashCache::new();
    let baseline = MARGIN + 34.0;
    write(&mut px, fs, &mut cache, &family, CARD_TITLE, 30.0, Weight::BOLD, TITLE, (MARGIN, baseline), false);
    let number = format!("Round #{}", round);
    write(&mut px, fs, &mut cache, &family, &number, 21.0, Weight::NORMAL, MUTED, (width as f32 - MARGIN, baseline), true);
    // A pencil rule under the heading, the width of the paper.
    if let Some(rule) = rrect(MARGIN, MARGIN + TITLE_BAND - 16.0, width as f32 - 2.0 * MARGIN, 1.5, 0.75) {
        px.fill_path(&rule, &paint(EDGE, 255), FillRule::Winding, Transform::identity(), None);
    }
    for (i, doodle) in doodles.iter().enumerate() {
        let x = MARGIN + i as f32 * (PANEL + GAP);
        // A fold down the middle of the sheet, so two drawings of the same thing
        // read as two drawings and not one muddle.
        if i > 0 {
            let fold = rrect(x - GAP / 2.0 - 0.75, MARGIN + TITLE_BAND + PAD, 1.5, PANEL - 2.0 * PAD, 0.75);
            if let Some(fold) = fold {
                px.fill_path(&fold, &paint(EDGE, 255), FillRule::Winding, Transform::identity(), None);
            }
        }
        draw_doodle(&mut px, doodle, x, MARGIN + TITLE_BAND, PANEL);
    }
    px.encode_png().ok()
}

type Cache = HashMap<(i64, usize), Arc<Vec<u8>>>;

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How many rounds' pictures are kept before the oldest are dropped.
const KEEP: usize = 12;

/// The picture for a round, drawn once and then remembered - once with the one
/// drawing, and again with the hint's two. Drawing is real work, so it never
/// happens on the gateway thread.
pub async fn png(round: i64, doodles: Vec<Doodle>) -> Option<Arc<Vec<u8>>> {
    let key = (round, doodles.len().min(MOST));
    if let Some(png) = cache().lock().get(&key).cloned() {
        return Some(png);
    }
    let drawn = tokio::task::spawn_blocking(move || {
        let mut fs = super::awards::fonts().lock();
        render(round, &doodles, &mut fs)
    })
    .await
    .ok()
    .flatten()?;
    let png = Arc::new(drawn);
    let mut cache = cache().lock();
    if cache.len() >= KEEP {
        let oldest: Vec<(i64, usize)> = {
            let mut keys: Vec<(i64, usize)> = cache.keys().copied().collect();
            keys.sort_unstable();
            keys.into_iter().take(cache.len() + 1 - KEEP).collect()
        };
        for key in oldest {
            cache.remove(&key);
        }
    }
    cache.insert(key, png.clone());
    Some(png)
}

/// What the picture is called on the message. The hinted card is a different
/// file from the one it replaces, so nothing anywhere shows yesterday's picture
/// for today's round.
pub fn file_name(round: i64, drawings: usize) -> String {
    match drawings {
        0 | 1 => format!("guess-{}.png", round),
        n => format!("guess-{}-{}.png", round, n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::guess_bank::{self, Bank};

    /// Every drawing a bank can give for one word, in order.
    fn doodles(bank: &Bank, word: usize, which: &[usize]) -> Vec<Doodle> {
        which.iter().filter_map(|d| bank.doodle(word, *d)).collect()
    }

    fn drawn(round: i64, doodles: &[Doodle]) -> Pixmap {
        let mut fs = crate::channels::discord::awards::fonts().lock();
        let png = render(round, doodles, &mut fs).expect("a picture");
        assert_eq!(&png[1..4], b"PNG");
        assert!(png.len() < 400_000, "{} bytes is too heavy for a chat message", png.len());
        Pixmap::decode_png(&png).expect("readable png")
    }

    #[test]
    fn a_doodle_is_drawn_onto_a_card_of_the_expected_size() {
        let bank = guess_bank::tests::fixture();
        let word = bank.find("guitar").expect("the guitar");
        let one = drawn(7, &doodles(&bank, word, &[0]));
        assert_eq!((one.width(), one.height()), card_size(1));
        // Ink went down: a card that is only paper would be one or two colours.
        let colours: std::collections::HashSet<[u8; 4]> = one.pixels().iter().map(|p| [p.red(), p.green(), p.blue(), p.alpha()]).collect();
        assert!(colours.len() > 8, "the card looks blank ({} colours)", colours.len());
        // A hint puts a second drawing of the same thing beside the first.
        let two = drawn(7, &doodles(&bank, word, &[0, 1]));
        assert_eq!((two.width(), two.height()), card_size(2));
        assert!(two.width() > one.width() && two.height() == one.height());
        // Both halves of a hinted card are drawn on, not just the left one.
        let inked = |px: &Pixmap, from: u32, to: u32| {
            (from..to).any(|x| (0..px.height()).any(|y| px.pixel(x, y).map(|p| p.red() < 120).unwrap_or(false)))
        };
        assert!(inked(&two, 0, two.width() / 2) && inked(&two, two.width() / 2, two.width()));
        // Nothing is ever asked to draw more than a card holds.
        let many = drawn(7, &doodles(&bank, word, &[0, 1, 2]));
        assert_eq!((many.width(), many.height()), card_size(2));
    }

    #[test]
    fn several_words_all_draw_and_stay_small() {
        let bank = guess_bank::tests::fixture();
        for word in 0..bank.word_count() {
            let png = drawn(word as i64 + 1, &doodles(&bank, word, &[0]));
            assert_eq!((png.width(), png.height()), card_size(1));
        }
        // An empty drawing, and one whose strokes have no points, leave paper
        // rather than a panic.
        let blank = drawn(1, &[Vec::new()]);
        assert_eq!((blank.width(), blank.height()), card_size(1));
        drawn(2, &[vec![Vec::new()]]);
        drawn(3, &[]);
    }

    /// The shipped bank when it is there: a real drawing off the real pack.
    #[test]
    fn the_shipped_doodles_draw_as_well() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("drawbank");
        let Ok(bank) = guess_bank::load(&dir) else { return };
        for word in [0, 5, 40, 100] {
            if bank.word(word).is_none() {
                continue;
            }
            let png = drawn(word as i64, &doodles(&bank, word, &[0, 1]));
            assert_eq!((png.width(), png.height()), card_size(2));
        }
    }

    #[test]
    fn a_picture_is_named_after_its_round() {
        assert_eq!(file_name(128, 1), "guess-128.png");
        assert_eq!(file_name(128, 0), "guess-128.png");
        assert_eq!(file_name(128, 2), "guess-128-2.png", "the hinted card is its own file");
    }

    /// Cards to look at with your own eyes. Ignored by default because it writes
    /// files: run it with somewhere to put them.
    ///
    /// `VIZIER_GUESS_SAMPLE_DIR=/tmp/cards cargo test guess_draw -- --ignored`
    #[test]
    #[ignore]
    fn sample_cards_to_look_at() {
        let Ok(out) = std::env::var("VIZIER_GUESS_SAMPLE_DIR") else { return };
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("drawbank");
        let bank = guess_bank::load(&dir).expect("the shipped bank");
        std::fs::create_dir_all(&out).expect("somewhere to write");
        let mut fs = crate::channels::discord::awards::fonts().lock();
        for (round, word) in [12, 34, 77, 120, 200].into_iter().enumerate() {
            let Some(entry) = bank.word(word) else { continue };
            let name = entry.word.replace(' ', "-");
            let one = render(round as i64 + 1, &doodles(&bank, word, &[0]), &mut fs).expect("a picture");
            std::fs::write(format!("{}/{}.png", out, name), &one).expect("written");
            let two = render(round as i64 + 1, &doodles(&bank, word, &[0, 1]), &mut fs).expect("a picture");
            std::fs::write(format!("{}/{}-hint.png", out, name), &two).expect("written");
        }
    }
}
