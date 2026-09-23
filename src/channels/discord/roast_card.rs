//! The `/ship` card: two round avatars, the ship name between them, the score
//! big underneath and a bar filled to it, cold at the bottom and hot at the
//! top.
//!
//! Drawn with the same kit as the battle and awards cards - `battle_card::Pen`
//! for type (cosmic-text, so a name full of script letters or emoji still
//! draws), `awards_card`'s circle crop and rounded rectangles, and the shared
//! font system from `awards::fonts`. Only the numbers go in the picture: the
//! verdict stays in the message beside it, where it can be read and quoted.

use cosmic_text::{FontSystem, Weight};
use tiny_skia::{
    Color as SkColor, FillRule, GradientStop, LinearGradient, Paint, Point, PixmapPaint, Rect, SpreadMode, Stroke,
    Transform,
};

use super::awards_card::{avatar_pixmap, fill_circle, paint, rrect};
use super::battle_card::Pen;

/// One side of the ship.
#[derive(Clone, Debug, Default)]
pub struct Face {
    pub name: String,
    /// The raw bytes of their profile picture, if one arrived in time.
    pub avatar: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Default)]
pub struct Card {
    pub ship: String,
    pub percent: u8,
    pub left: Face,
    pub right: Face,
    /// One real number about the pair, counted by the bot, drawn under the bar.
    /// Never the model's words: see `roast_build::headline`.
    pub line: String,
    /// "up 6 since 5 days ago", when the score has moved since the last time
    /// this pair was shipped. Nothing at all when it hasn't.
    pub moved: Option<String>,
}

pub const W: f32 = 1000.0;
pub const H: f32 = 470.0;
/// What the picture is called where it is attached.
pub const FILE: &str = "ship.png";

const BG: [u8; 3] = [15, 16, 20];
const PANEL: [u8; 3] = [30, 32, 38];
const TRACK: [u8; 3] = [42, 45, 54];
const INK: [u8; 3] = [244, 245, 247];
const MUTED: [u8; 3] = [148, 154, 166];

/// Avatar size, the ring around it and the gap inside the ring.
const AV: f32 = 168.0;
const RING: f32 = 6.0;
const GAP: f32 = 4.0;
const CY: f32 = 180.0;
const LEFT_CX: f32 = 150.0;
const RIGHT_CX: f32 = W - LEFT_CX;
/// The bar.
const BAR_X: f32 = 70.0;
const BAR_Y: f32 = 344.0;
const BAR_H: f32 = 30.0;
const CAPTION: &str = "Loduchand ship-o-meter";
/// The score, the movement under it, the counted line under the bar, and the
/// caption under that.
const SCORE_Y: f32 = 284.0;
const MOVED_Y: f32 = 320.0;
const LINE_Y: f32 = 412.0;
const CAPTION_Y: f32 = H - 16.0;
/// Up is green, down is red, whatever the score's own colour is.
const UP: [u8; 3] = [74, 222, 128];
const DOWN: [u8; 3] = [248, 113, 113];

/// Cut to a sane number of characters before anything is measured. Fitting
/// works by shaping the text once per character it drops, so a name someone
/// pasted a novel into would be shaped a thousand times over; nothing this
/// card draws is ever longer than a line anyway.
fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => text[..at].to_string(),
        None => text.to_string(),
    }
}

fn outer() -> f32 {
    AV / 2.0 + RING + GAP
}

fn bar_w() -> f32 {
    W - 2.0 * BAR_X
}

/// Cold at nothing, hot at everything, by way of cyan and amber. Every stop is
/// bright enough to read on Discord's dark background.
pub fn heat(percent: u8) -> [u8; 3] {
    const STOPS: [(f32, [u8; 3]); 5] = [
        (0.0, [96, 132, 232]),
        (25.0, [56, 189, 248]),
        (50.0, [250, 204, 21]),
        (75.0, [251, 146, 60]),
        (100.0, [244, 63, 94]),
    ];
    let p = (percent as f32).clamp(0.0, 100.0);
    let mut out = STOPS[STOPS.len() - 1].1;
    for pair in STOPS.windows(2) {
        let ((a_at, a), (b_at, b)) = (pair[0], pair[1]);
        if p <= b_at {
            let t = ((p - a_at) / (b_at - a_at)).clamp(0.0, 1.0);
            out = [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8);
            break;
        }
    }
    out
}

/// The same colour, darkened, for a ring or a wash behind the number.
fn dim(c: [u8; 3], by: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 * by) as u8)
}

/// The first letter of a name for the circle when no picture arrived. Emoji and
/// script letters are skipped in favour of something that reads at this size.
fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn portrait(pen: &mut Pen, cx: f32, face: &Face, accent: [u8; 3]) {
    let o = outer();
    fill_circle(&mut pen.px, cx, CY, o, accent);
    fill_circle(&mut pen.px, cx, CY, o - RING, PANEL);
    match avatar_pixmap(face.avatar.as_deref(), AV as u32, false) {
        Some(pm) => pen.px.draw_pixmap(
            (cx - AV / 2.0).round() as i32,
            (CY - AV / 2.0).round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        // No picture, or one that wouldn't decode: their initial instead.
        None => {
            fill_circle(&mut pen.px, cx, CY, AV / 2.0, dim(accent, 0.34));
            pen.centered(&initial(&face.name), cx, CY + 28.0, 78.0, Weight::EXTRA_BOLD, INK);
        }
    }
}

fn bar(pen: &mut Pen, percent: u8, hot: [u8; 3]) {
    let r = BAR_H / 2.0;
    if let Some(track) = rrect(BAR_X, BAR_Y, bar_w(), BAR_H, r) {
        pen.px.fill_path(&track, &paint(TRACK, 255), FillRule::Winding, Transform::identity(), None);
        pen.px.stroke_path(&track, &paint([58, 62, 72], 255), &Stroke { width: 2.0, ..Stroke::default() }, Transform::identity(), None);
    }
    // Always a sliver, so 0% still reads as a bar rather than an empty box.
    let filled = (bar_w() * (percent as f32 / 100.0)).max(BAR_H);
    let Some(fill) = rrect(BAR_X, BAR_Y, filled, BAR_H, r.min(filled / 2.0)) else { return };
    let cold = heat(0);
    match LinearGradient::new(
        Point::from_xy(BAR_X, BAR_Y),
        Point::from_xy(BAR_X + filled, BAR_Y),
        vec![
            GradientStop::new(0.0, SkColor::from_rgba8(cold[0], cold[1], cold[2], 255)),
            GradientStop::new(1.0, SkColor::from_rgba8(hot[0], hot[1], hot[2], 255)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        Some(shader) => {
            let mut p = Paint { shader, anti_alias: true, ..Paint::default() };
            p.anti_alias = true;
            pen.px.fill_path(&fill, &p, FillRule::Winding, Transform::identity(), None);
        }
        None => pen.px.fill_path(&fill, &paint(hot, 255), FillRule::Winding, Transform::identity(), None),
    }
}

/// Draws the card. `None` when the canvas or the encoder gives out - the
/// caller then posts the text version instead.
pub fn render(card: &Card, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let hot = heat(card.percent);
    let mut pen = Pen::new(W, H, fs)?;
    pen.px.fill(SkColor::from_rgba8(BG[0], BG[1], BG[2], 255));

    // A wash of the score's own colour across the top, fading out.
    for row in 0..300 {
        let alpha = (110.0 * (1.0 - row as f32 / 300.0).powf(1.6)) as u8;
        if let (Some(r), Some(shader)) = (
            Rect::from_xywh(0.0, row as f32, W, 1.0),
            LinearGradient::new(
                Point::from_xy(0.0, 0.0),
                Point::from_xy(W, 0.0),
                vec![
                    GradientStop::new(0.0, SkColor::from_rgba8(90, 60, 150, alpha)),
                    GradientStop::new(1.0, SkColor::from_rgba8(hot[0], hot[1], hot[2], alpha)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ),
        ) {
            let p = Paint { shader, ..Paint::default() };
            pen.px.fill_rect(r, &p, Transform::identity(), None);
        }
    }

    portrait(&mut pen, LEFT_CX, &card.left, dim(hot, 0.75));
    portrait(&mut pen, RIGHT_CX, &card.right, hot);

    // Their names under their pictures.
    let name_w = 2.0 * outer() + 40.0;
    for (cx, face) in [(LEFT_CX, &card.left), (RIGHT_CX, &card.right)] {
        let name = pen.fit(&clip(&face.name, 48), 30.0, Weight::BOLD, name_w);
        pen.centered(&name, cx, CY + outer() + 42.0, 30.0, Weight::BOLD, INK);
    }

    // The ship name between them, shrunk until it fits the gap.
    let middle = RIGHT_CX - LEFT_CX - 2.0 * outer() - 32.0;
    let wanted = clip(&card.ship, 48);
    let mut size = 56.0_f32;
    while size > 26.0 && pen.measure(&wanted, size, Weight::EXTRA_BOLD) > middle {
        size -= 4.0;
    }
    let ship = pen.fit(&wanted, size, Weight::EXTRA_BOLD, middle);
    pen.centered("💞", W / 2.0, 92.0, 38.0, Weight::NORMAL, INK);
    pen.centered(&ship, W / 2.0, 92.0 + size + 16.0, size, Weight::EXTRA_BOLD, INK);

    // The number, big, and which way it has moved since last time - when it has.
    pen.centered(&format!("{}%", card.percent), W / 2.0, SCORE_Y, 96.0, Weight::EXTRA_BOLD, hot);
    if let Some(moved) = card.moved.as_deref().filter(|m| !m.trim().is_empty()) {
        let colour = if moved.starts_with("down") { DOWN } else { UP };
        let arrow = if moved.starts_with("down") { "▼" } else { "▲" };
        let moved = pen.fit(&clip(&format!("{} {}", arrow, moved), 72), 22.0, Weight::SEMIBOLD, W - 2.0 * BAR_X);
        pen.centered(&moved, W / 2.0, MOVED_Y, 22.0, Weight::SEMIBOLD, colour);
    }

    bar(&mut pen, card.percent, hot);

    // One counted fact about the pair. `fit` is a second belt on top of the
    // cut in `roast_build::headline`: nothing here can run off the card.
    let line = pen.fit(&clip(&card.line, 80), 24.0, Weight::SEMIBOLD, W - 2.0 * BAR_X);
    pen.centered(&line, W / 2.0, LINE_Y, 24.0, Weight::SEMIBOLD, dim(hot, 0.92));
    pen.centered(CAPTION, W / 2.0, CAPTION_Y, 16.0, Weight::MEDIUM, MUTED);
    pen.px.encode_png().ok()
}

/// The card as PNG bytes, on the shared font system. Blocking: call it from
/// `spawn_blocking`.
pub fn png(card: &Card) -> Option<Vec<u8>> {
    render(card, &mut super::awards::fonts().lock())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

    /// Stands in for a downloaded profile picture, so the circle crop has
    /// something in it.
    fn fake_avatar(tint: [u8; 3]) -> Vec<u8> {
        let n = 160.0_f32;
        let mut img = image::RgbaImage::new(n as u32, n as u32);
        for (px, py, p) in img.enumerate_pixels_mut() {
            let (x, y) = (px as f32, py as f32);
            let t = 0.35 + 0.65 * (x + y) / (2.0 * n);
            let mut c = [tint[0] as f32 * t, tint[1] as f32 * t, tint[2] as f32 * t];
            let head = ((x - n * 0.5).powi(2) + (y - n * 0.42).powi(2)).sqrt();
            let body = ((x - n * 0.5).powi(2) + (y - n * 1.15).powi(2)).sqrt();
            if head < n * 0.20 || body < n * 0.52 {
                c = [c[0] * 0.45 + 60.0, c[1] * 0.45 + 60.0, c[2] * 0.45 + 70.0];
            }
            *p = image::Rgba([c[0] as u8, c[1] as u8, c[2] as u8, 255]);
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).expect("encoding the test avatar");
        out.into_inner()
    }

    fn pair() -> Card {
        Card {
            ship: "Arjya".into(),
            percent: 87,
            left: Face { name: "arjun".into(), avatar: Some(fake_avatar([92, 124, 214])) },
            right: Face { name: "riya".into(), avatar: Some(fake_avatar([214, 96, 140])) },
            line: "3 fights called · 0 apologies logged".into(),
            moved: Some("down 9 since 5 days ago".into()),
        }
    }

    fn drawn(card: &Card) -> Vec<u8> {
        let png = png(card).expect("the card draws");
        assert_eq!(png[..4], PNG_MAGIC, "a real PNG");
        let img = image::load_from_memory(&png).expect("the PNG decodes");
        assert_eq!((img.width(), img.height()), (W as u32, H as u32));
        png
    }

    #[test]
    fn a_normal_pair_draws() {
        let png = drawn(&pair());
        assert!(png.len() > 4_000, "a card with two pictures on it is not a blank: {} bytes", png.len());
        // Every score draws, and the bar never runs off the end.
        for percent in [0u8, 1, 7, 50, 99, 100] {
            drawn(&Card { percent, ..pair() });
        }
    }

    #[test]
    fn a_failed_avatar_fetch_falls_back_to_the_initial() {
        // Nothing arrived at all.
        let none = Card { left: Face { name: "arjun".into(), avatar: None }, right: Face { name: "riya".into(), avatar: None }, ..pair() };
        drawn(&none);
        // One arrived, the other didn't.
        drawn(&Card { right: Face { name: "riya".into(), avatar: None }, ..pair() });
        // Bytes that aren't a picture at all are the same as nothing: no panic.
        drawn(&Card { left: Face { name: "arjun".into(), avatar: Some(b"<html>404 not found</html>".to_vec()) }, ..pair() });
        // A name with no letter in it still gets a circle with something in it.
        assert_eq!(initial("arjun"), "A");
        assert_eq!(initial("🐸🐸"), "?");
        assert_eq!(initial("₹99"), "9");
        drawn(&Card { left: Face { name: "🐸🐸".into(), avatar: None }, ..pair() });
    }

    #[test]
    fn long_names_and_emoji_do_not_break_the_renderer() {
        drawn(&Card { ship: "Supercalifragilisticexpialidociousandthensome".into(), ..pair() });
        drawn(&Card {
            ship: "Aʀᴊʏᴀ 💞🔥".into(),
            left: Face { name: "🔥 arjun 𝔤𝔬𝔡 🔥".into(), avatar: Some(fake_avatar([92, 124, 214])) },
            right: Face { name: "riya 👩‍👩‍👧‍👦🏳️‍🌈".into(), avatar: None },
            ..pair()
        });
        // The awkward edges: nothing at all, and far more than anyone would type.
        drawn(&Card { ship: String::new(), left: Face::default(), right: Face::default(), ..pair() });
        drawn(&Card { ship: "💞".repeat(20), left: Face { name: "x".repeat(200), avatar: None }, right: Face { name: "😭".repeat(20), avatar: None }, ..pair() });
        // The movement line, both ways, and its absence.
        drawn(&Card { moved: Some("up 6 since 7 days ago".into()), ..pair() });
        drawn(&Card { moved: None, ..pair() });
        drawn(&Card { moved: Some("   ".into()), ..pair() });
        drawn(&Card { moved: Some("down 9 since ".to_string() + &"9".repeat(200)), ..pair() });
        // The longest line the counter can produce, and then some.
        let longest = "W".repeat(super::super::roast_build::HEADLINE_CHARS);
        drawn(&Card { line: longest, ..pair() });
        drawn(&Card { line: "💞🔥".repeat(20), ..pair() });
        drawn(&Card { line: String::new(), ..pair() });
    }

    #[test]
    fn the_bar_runs_cold_to_hot_and_stays_readable() {
        let (cold, warm, hot) = (heat(0), heat(50), heat(100));
        assert_eq!(cold, [96, 132, 232]);
        assert_eq!(warm, [250, 204, 21]);
        assert_eq!(hot, [244, 63, 94]);
        // Blue at the bottom, red at the top: blue loses and red gains all the way up.
        let blues: Vec<i32> = (0..=100).step_by(5).map(|p| heat(p as u8)[2] as i32).collect();
        assert!(blues[0] > blues[blues.len() - 1], "{blues:?}");
        assert!(heat(100)[0] > heat(0)[0], "red gains");
        // Bright enough to read on Discord's dark background at every step.
        for p in 0..=100u8 {
            let c = heat(p);
            let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
            assert!(lum > 100.0, "{}% is too dark to read: {:?} ({})", p, c, lum);
        }
        // It moves smoothly: no step between neighbouring scores is a jump.
        for p in 1..=100u8 {
            let (a, b) = (heat(p - 1), heat(p));
            let step: i32 = (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).sum();
            assert!(step <= 30, "{}% jumps: {:?} -> {:?}", p, a, b);
        }
    }

    /// Writes a card out so it can be looked at. Ignored by default - run with
    /// `cargo test ship_card_sample -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ship_card_sample() {
        let dir = std::env::var("SHIP_CARD_DIR").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
        for (name, card) in [
            ("ship-87", pair()),
            ("ship-4", Card { ship: "Devya".into(), percent: 4, line: "Not one word between them in 60 days".into(), moved: None, ..pair() }),
            ("ship-99", Card { ship: "Gootus".into(), percent: 99, line: "1,204 replies in 60 days".into(), moved: Some("up 6 since 7 days ago".into()), ..pair() }),
            ("ship-noavatar", Card { left: Face { name: "arjun".into(), avatar: None }, right: Face { name: "riya".into(), avatar: None }, percent: 61, line: "412 replies in 60 days".into(), ..pair() }),
            ("ship-emoji", Card { ship: "Aʀᴊʏᴀ 💞".into(), left: Face { name: "🔥 arjun".into(), avatar: Some(fake_avatar([92, 124, 214])) }, right: Face { name: "riya 😭".into(), avatar: None }, percent: 33, line: "61 pings at each other, 0 replies".into(), moved: None }),
            ("ship-longest", Card { ship: "Supercalifragilistic".into(), percent: 72, line: "W".repeat(super::super::roast_build::HEADLINE_CHARS), ..pair() }),
        ] {
            let path = std::path::Path::new(&dir).join(format!("{}.png", name));
            std::fs::write(&path, png(&card).expect("the card draws")).expect("writing the sample");
            println!("wrote {}", path.display());
        }
    }
}
