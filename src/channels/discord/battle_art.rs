//! The month's painted art for the arena's cards.
//!
//! The crests, the eggs, the dragons, the halls behind a champion and the
//! frames round them are painted pieces, a megabyte or two each. They are far
//! too heavy to sit in the binary, so they live in `{workspace}/arenaart/`
//! beside the other banks - `frogcards/`, `wordbank/`, `riddlebank/` - and are
//! read off disk the first time a card wants one.
//!
//! **Nothing here is required.** Every call returns `Option`, and every caller
//! falls back to the shape it drew before. A checkout without the art, a server
//! where the directory has not been copied yet, a test: all of them still draw
//! a card, just a plainer one. That is deliberate - a missing picture must
//! never be the reason a fight cannot be posted.
//!
//! Decoding and scaling a 1024px painting takes long enough that it is done
//! once per size and kept. The cache is keyed by file and size, so the fight
//! card's 72px crest and the champion's 100px crest are two entries and a
//! hundred fights are none.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;
use tiny_skia::Pixmap;

/// Where the painted pieces live, under the workspace.
pub const DIR: &str = "arenaart";

/// A piece of art the arena knows how to ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Art {
    /// A house's painted crest, by its art slug - `stark`, `watch`.
    Crest(&'static str),
    /// A dragon egg at one of its five stages.
    Egg(u8),
    /// A house's dragon, by its art slug.
    Dragon(&'static str),
    /// The hall behind a champion.
    Throne,
    /// The Wall, for a card that wants cold instead of gold.
    Wall,
    /// Westeros itself, faint, under a scroll.
    Map,
    /// A painted border. These were drawn as tall card frames, so they are
    /// never stretched onto a landscape card - see [`nine_slice`].
    Frame(&'static str),
}

impl Art {
    /// The file it is read from.
    pub fn file(self) -> String {
        match self {
            Art::Crest(house) => format!("crest-{}.png", house),
            Art::Egg(stage) => format!("egg-{}.png", stage.clamp(1, EGG_ART_STAGES)),
            Art::Dragon(house) => format!("dragon-{}.png", house),
            Art::Throne => "throne.png".to_string(),
            Art::Wall => "wall.png".to_string(),
            Art::Map => "map.png".to_string(),
            Art::Frame(kind) => format!("frame-{}.png", kind),
        }
    }
}

/// How many egg paintings there are: `egg-1.png` to `egg-5.png`.
pub const EGG_ART_STAGES: u8 = 5;

/// Which painting the egg week is showing today, by the day of it. The crack is
/// the calendar's, not the owner's: everybody's egg looks the same on day four,
/// and what their own points buy is how warmly it glows. This is the live
/// page's own table, and it has to stay the live page's own table - an egg that
/// is cracked on the website and smooth on a fight card is a bug people will
/// report.
pub const STAGE_BY_DAY: [u8; 7] = [1, 1, 2, 3, 3, 4, 5];

/// The painting for a day of the egg week, counting day 1 as the first.
pub fn egg_for_day(day: i64) -> u8 {
    let at = day.clamp(1, STAGE_BY_DAY.len() as i64) as usize - 1;
    STAGE_BY_DAY[at]
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();

/// Points the arena at its art. Called once at startup, like the stores.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &[DIR]);
    let found = dir.is_dir();
    let _ = ROOT.set(dir.clone());
    if found {
        tracing::info!("battle: arena art from {}", dir.display());
    } else {
        // Worth one line at startup: the cards will be the plain ones, and
        // somebody will want to know why without having to read the code.
        tracing::info!("battle: no {} directory at {} - the cards draw their plain shapes", DIR, dir.display());
    }
}

/// Where the art is: the workspace's own directory, or one named in the
/// environment. `ARENA_ART_DIR` is a developer's knob, not a panel setting -
/// it is what lets the card previews and the tests draw the real paintings
/// without a workspace around them.
fn root() -> Option<PathBuf> {
    if let Some(set) = ROOT.get() {
        return Some(set.clone());
    }
    std::env::var("ARENA_ART_DIR").ok().filter(|d| !d.trim().is_empty()).map(PathBuf::from)
}

/// Decoded pieces, by file and the size they were asked for. A `None` is cached
/// too: a file that is not there must not be looked for again on every card.
type Cache = HashMap<(String, u32, u32), Option<Arc<Pixmap>>>;
static DECODED: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// A piece of art, scaled to exactly `w` by `h`, or `None` if it is not there
/// or will not decode. The aspect is not kept: ask for the shape you want.
pub fn sized(art: Art, w: u32, h: u32) -> Option<Arc<Pixmap>> {
    if w == 0 || h == 0 {
        return None;
    }
    let key = (art.file(), w, h);
    if let Some(found) = DECODED.lock().get(&key) {
        return found.clone();
    }
    let made = load(&key.0, w, h, false).map(Arc::new);
    DECODED.lock().insert(key, made.clone());
    made
}

/// A square piece of art, scaled and then cut to a circle. The paintings were
/// made on their own dark ground rather than on transparency, so dropping one
/// straight into a round portrait leaves a black square sitting in the ring.
/// The cut is done once per size and kept, which also keeps it off the bracket,
/// where there are thirty-two of these on one card.
pub fn round(art: Art, side: u32) -> Option<Arc<Pixmap>> {
    if side == 0 {
        return None;
    }
    let key = (format!("round:{}", art.file()), side, side);
    if let Some(found) = DECODED.lock().get(&key) {
        return found.clone();
    }
    let made = load(&art.file(), side, side, false).map(|mut px| {
        cut_to_circle(&mut px);
        Arc::new(px)
    });
    DECODED.lock().insert(key, made.clone());
    made
}

/// Clears everything outside the biggest circle that fits, with one pixel of
/// softness at the edge so it does not come out jagged.
fn cut_to_circle(px: &mut Pixmap) {
    let (w, h) = (px.width() as f32, px.height() as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r = cx.min(cy);
    let width = px.width() as usize;
    for (i, pixel) in px.pixels_mut().iter_mut().enumerate() {
        let (x, y) = ((i % width) as f32 + 0.5, (i / width) as f32 + 0.5);
        let away = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        if away <= r - 1.0 {
            continue;
        }
        // Premultiplied, so every channel is scaled, not just the alpha.
        let keep = if away >= r { 0.0 } else { r - away };
        let c = pixel.demultiply();
        let a = (c.alpha() as f32 * keep) as u8;
        *pixel = tiny_skia::ColorU8::from_rgba(c.red(), c.green(), c.blue(), a).premultiply();
    }
}

/// A piece of art scaled to COVER `w` by `h` and cropped to it, the way a
/// backdrop wants: filled edge to edge with nothing squashed.
pub fn cover(art: Art, w: u32, h: u32) -> Option<Arc<Pixmap>> {
    if w == 0 || h == 0 {
        return None;
    }
    let key = (format!("cover:{}", art.file()), w, h);
    if let Some(found) = DECODED.lock().get(&key) {
        return found.clone();
    }
    let made = load(&art.file(), w, h, true).map(Arc::new);
    DECODED.lock().insert(key, made.clone());
    made
}

/// Reads a file and scales it. `cover` fills the box and crops the overflow;
/// otherwise it is scaled to exactly the size asked for.
fn load(file: &str, w: u32, h: u32, cover: bool) -> Option<Pixmap> {
    let path = root()?.join(file);
    // The file name is ours, never a member's, but joining a path from a string
    // is worth one guard all the same.
    if file.contains("..") || file.contains('/') {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    // These are reductions - a 1024px painting into a 72px badge - so a filter
    // that averages properly is what keeps them from going to mush.
    let filter = image::imageops::FilterType::Lanczos3;
    let scaled = if cover {
        let (sw, sh) = (decoded.width().max(1), decoded.height().max(1));
        let scale = (w as f32 / sw as f32).max(h as f32 / sh as f32);
        let (fw, fh) = ((sw as f32 * scale).ceil() as u32, (sh as f32 * scale).ceil() as u32);
        let filled = decoded.resize_exact(fw.max(w), fh.max(h), filter);
        let (x, y) = ((filled.width().saturating_sub(w)) / 2, (filled.height().saturating_sub(h)) / 2);
        image::imageops::crop_imm(&filled, x, y, w, h).to_image()
    } else {
        decoded.resize_exact(w, h, filter).into_rgba8()
    };
    let mut px = Pixmap::new(w, h)?;
    for (slot, pixel) in px.pixels_mut().iter_mut().zip(scaled.pixels()) {
        let [r, g, b, a] = pixel.0;
        // tiny-skia keeps its pixels premultiplied; a PNG's are not.
        *slot = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    Some(px)
}

/// A painted border fitted to a box of `w` by `h`, as a nine-slice: the four
/// corners at their own proportions, the four edges stretched only along their
/// length, and nothing in the middle. The frames were drawn tall, for the
/// collectible cards; squashing one onto a landscape card would pull its
/// corner ornaments out of shape, which is the whole reason this exists.
///
/// `corner` is how much of the source each corner takes, as a fraction of its
/// shorter side; `band` is how thick the border lands on the card. Both corners
/// are square in the source and square on the card, so a corner ornament keeps
/// its shape exactly - only the four edges stretch, and only along their length.
pub fn nine_slice(art: Art, w: u32, h: u32, corner: f32, band: u32) -> Option<Arc<Pixmap>> {
    if w < 8 || h < 8 {
        return None;
    }
    let key = (format!("nine:{}:{:.3}:{}", art.file(), corner, band), w, h);
    if let Some(found) = DECODED.lock().get(&key) {
        return found.clone();
    }
    let made = build_nine(&art.file(), w, h, corner, band).map(Arc::new);
    DECODED.lock().insert(key, made.clone());
    made
}

fn build_nine(file: &str, w: u32, h: u32, corner: f32, band: u32) -> Option<Pixmap> {
    let src = load_native(file)?;
    let (sw, sh) = (src.width(), src.height());
    // The corner block in the source: a square, so it can go down on the card
    // as a square and keep its proportions whatever shape the card is.
    let cut = ((sw.min(sh) as f32) * corner.clamp(0.05, 0.45)).round().max(1.0) as u32;
    let cut = cut.min(sw / 2).min(sh / 2).max(1);
    // How thick it lands. Capped at a third of the box so a small card still
    // has some edge between its corners.
    let put = band.clamp(1, (w / 3).min(h / 3).max(1));
    let mut out = Pixmap::new(w, h)?;
    let how = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
    // (source rect, destination rect). The middle is left out on purpose.
    let (mw, mh) = (sw.saturating_sub(2 * cut).max(1), sh.saturating_sub(2 * cut).max(1));
    let (dw, dh) = (w.saturating_sub(2 * put).max(1), h.saturating_sub(2 * put).max(1));
    let pieces: [(u32, u32, u32, u32, f32, f32, f32, f32); 8] = [
        (0, 0, cut, cut, 0.0, 0.0, put as f32, put as f32),
        (sw - cut, 0, cut, cut, (w - put) as f32, 0.0, put as f32, put as f32),
        (0, sh - cut, cut, cut, 0.0, (h - put) as f32, put as f32, put as f32),
        (sw - cut, sh - cut, cut, cut, (w - put) as f32, (h - put) as f32, put as f32, put as f32),
        (cut, 0, mw, cut, put as f32, 0.0, dw as f32, put as f32),
        (cut, sh - cut, mw, cut, put as f32, (h - put) as f32, dw as f32, put as f32),
        (0, cut, cut, mh, 0.0, put as f32, put as f32, dh as f32),
        (sw - cut, cut, cut, mh, (w - put) as f32, put as f32, put as f32, dh as f32),
    ];
    for (sx, sy, pw, ph, dx, dy, dw, dh) in pieces {
        let Some(piece) = cut_out(&src, sx, sy, pw, ph) else { continue };
        let (kx, ky) = (dw / pw.max(1) as f32, dh / ph.max(1) as f32);
        let at = tiny_skia::Transform::from_row(kx, 0.0, 0.0, ky, dx, dy);
        out.draw_pixmap(0, 0, piece.as_ref(), &how, at, None);
    }
    Some(out)
}

/// One rectangle of a pixmap, copied out so it can be scaled on its own.
fn cut_out(src: &Pixmap, x: u32, y: u32, w: u32, h: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(w.max(1), h.max(1))?;
    let how = tiny_skia::PixmapPaint::default();
    out.draw_pixmap(-(x as i32), -(y as i32), src.as_ref(), &how, tiny_skia::Transform::identity(), None);
    Some(out)
}

/// A file at the size it was painted, kept because the nine-slice has to cut
/// its corners before anything is scaled.
fn load_native(file: &str) -> Option<Arc<Pixmap>> {
    let key = (format!("native:{}", file), 0, 0);
    if let Some(found) = DECODED.lock().get(&key) {
        return found.clone();
    }
    let made = read_native(file).map(Arc::new);
    DECODED.lock().insert(key, made.clone());
    made
}

fn read_native(file: &str) -> Option<Pixmap> {
    if file.contains("..") || file.contains('/') {
        return None;
    }
    let bytes = std::fs::read(root()?.join(file)).ok()?;
    let decoded = image::load_from_memory(&bytes).ok()?.into_rgba8();
    let mut px = Pixmap::new(decoded.width(), decoded.height())?;
    for (slot, pixel) in px.pixels_mut().iter_mut().zip(decoded.pixels()) {
        let [r, g, b, a] = pixel.0;
        *slot = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    Some(px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The egg a day shows is the live page's own table, and a day outside the
    /// week still shows an egg rather than nothing.
    #[test]
    fn the_egg_of_the_day_follows_the_live_pages_table() {
        assert_eq!(STAGE_BY_DAY, [1, 1, 2, 3, 3, 4, 5], "the website and the cards share this");
        for (day, want) in (1..=7).zip(STAGE_BY_DAY) {
            assert_eq!(egg_for_day(day), want, "day {}", day);
        }
        // Before the week and after it: the first and last paintings.
        assert_eq!(egg_for_day(0), 1);
        assert_eq!(egg_for_day(-99), 1);
        assert_eq!(egg_for_day(8), EGG_ART_STAGES);
        assert_eq!(egg_for_day(9_999), EGG_ART_STAGES);
        // The crack is the calendar's: it only ever goes forward through the week.
        let through: Vec<u8> = (1..=7).map(egg_for_day).collect();
        assert!(through.windows(2).all(|p| p[1] >= p[0]), "{:?}", through);
    }

    /// Every piece names a file, and the egg's number is held inside the five
    /// that exist however it is asked for.
    #[test]
    fn every_piece_names_a_file_that_could_exist() {
        assert_eq!(Art::Crest("stark").file(), "crest-stark.png");
        assert_eq!(Art::Dragon("watch").file(), "dragon-watch.png");
        assert_eq!(Art::Throne.file(), "throne.png");
        assert_eq!(Art::Wall.file(), "wall.png");
        assert_eq!(Art::Map.file(), "map.png");
        for stage in 0..=9u8 {
            let file = Art::Egg(stage).file();
            let want = stage.clamp(1, EGG_ART_STAGES);
            assert_eq!(file, format!("egg-{}.png", want), "stage {}", stage);
        }
        // Nothing names a path: these all sit directly in the one directory.
        for art in [Art::Crest("stark"), Art::Egg(3), Art::Dragon("stark"), Art::Throne, Art::Wall, Art::Map] {
            let file = art.file();
            assert!(!file.contains('/') && !file.contains(".."), "{}", file);
        }
    }

    /// With no art directory there is no art, and every caller falls back to
    /// the shape it drew before. This is the state a fresh checkout and the
    /// test suite are in, so it is the state most of the card tests cover - and
    /// it is the one that must never become a panic or a blank card.
    #[test]
    fn without_the_directory_there_is_simply_no_art() {
        // A run that has been pointed at the real paintings is testing the
        // other path; this one is about their absence.
        if root().is_some() {
            return;
        }
        for art in [Art::Crest("stark"), Art::Egg(3), Art::Dragon("stark"), Art::Throne, Art::Wall, Art::Map] {
            assert!(sized(art, 64, 64).is_none(), "{:?}", art);
            assert!(cover(art, 64, 64).is_none(), "{:?}", art);
            assert!(round(art, 64).is_none(), "{:?}", art);
        }
        assert!(nine_slice(Art::Frame("rare"), 400, 300, 0.2, 30).is_none());
        // And a house with no painting named never asks for one.
        assert_eq!(Art::Crest("").file(), "crest-.png");
    }

    /// A size nobody could draw is asked for by nobody, and answered with
    /// nothing rather than a panic.
    #[test]
    fn an_impossible_size_is_no_art_rather_than_a_panic() {
        assert!(sized(Art::Throne, 0, 10).is_none());
        assert!(sized(Art::Throne, 10, 0).is_none());
        assert!(cover(Art::Throne, 0, 0).is_none());
        assert!(nine_slice(Art::Frame("rare"), 4, 400, 0.2, 10).is_none());
        assert!(nine_slice(Art::Frame("rare"), 400, 4, 0.2, 10).is_none());
    }
}
