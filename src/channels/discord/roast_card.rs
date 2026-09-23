//! The `/ship` card: a valentine, not a dashboard.
//!
//! Two faces lit like candles, joined by a garland, with a heart between them
//! filled to the score the way a glass fills. Everything on it answers to the
//! number: the ground warms from a cold blue-plum to a deep rose, the petals
//! scattered behind go from a grey handful to a shower, and a pair scoring
//! almost nothing gets a dim, wilted card with a crack down the heart - which
//! is its own joke, and looks deliberate rather than broken.
//!
//! Drawn with the kit the other cards use: `awards_card`'s circle crop,
//! rounded rectangles and glyph blend, the shared font system from
//! `awards::fonts`, tiny-skia for the paths and cosmic-text for the type. The
//! type is in two faces - a script or high-contrast serif for the ship name,
//! the number and the two names, the plain sans for the small lines - both
//! chosen from whatever is actually installed, falling back to the sans the
//! other cards already use, so the card still draws on a bare server.
//!
//! Only what the bot counted goes in the picture. The verdict stays in the
//! message beside it, where it can be read and quoted.

use std::collections::HashMap;

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Stretch, Style as FontStyle, SwashCache, Weight};
use tiny_skia::{
    Color as SkColor, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap, PixmapPaint, Point,
    RadialGradient, Rect, SpreadMode, Stroke, Transform,
};

use super::awards_card::{avatar_pixmap, blend_rect, fill_circle, paint};

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
    /// One real number about the pair, counted by the bot, drawn under the
    /// heart. Never the model's words: see `roast_build::headline`.
    pub line: String,
    /// "up 6 since 5 days ago", when the score has moved since the last time
    /// this pair was shipped. Nothing at all when it hasn't.
    pub moved: Option<String>,
}

pub const W: f32 = 1000.0;
pub const H: f32 = 500.0;
/// What the picture is called where it is attached.
pub const FILE: &str = "ship.png";

/// Below this the card is a sad valentine: dim, wilted, cracked.
pub const LONELY: u8 = 25;

// --- where everything sits ---------------------------------------------------------------------

/// Avatar size, the ring around it and the gap inside the ring.
const AV: f32 = 152.0;
const RING: f32 = 5.0;
const GAP: f32 = 4.0;
const FACE_CY: f32 = 250.0;
const LEFT_CX: f32 = 170.0;
const RIGHT_CX: f32 = W - LEFT_CX;
/// The heart in the middle, and the score written on it.
const HEART_CX: f32 = W / 2.0;
const HEART_CY: f32 = 240.0;
const HEART_W: f32 = 272.0;
const SHIP_Y: f32 = 88.0;
const NAME_Y: f32 = 392.0;
const LINE_Y: f32 = 444.0;
const CAPTION_Y: f32 = 484.0;
const CAPTION: &str = "Loduchand ke dil se";

fn outer() -> f32 {
    AV / 2.0 + RING + GAP
}

// --- the palette -------------------------------------------------------------------------------

/// Cold at nothing, hot at everything - and a valentine the whole way, by way
/// of periwinkle, lilac and blush rather than a traffic light. Every stop is
/// bright enough to read on Discord's dark background. This is the card's
/// accent and the embed's stripe both, so the two agree.
pub fn heat(percent: u8) -> [u8; 3] {
    const STOPS: [(f32, [u8; 3]); 5] = [
        (0.0, [126, 148, 200]),
        (25.0, [176, 150, 205]),
        (50.0, [230, 150, 175]),
        (75.0, [245, 110, 150]),
        (100.0, [255, 70, 110]),
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

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8)
}

fn dim(c: [u8; 3], by: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 * by).clamp(0.0, 255.0) as u8)
}

/// Everything about how the card looks, decided by the score alone.
#[derive(Clone, Copy, Debug)]
pub struct Look {
    /// The ground, top to bottom.
    pub sky: [u8; 3],
    pub floor: [u8; 3],
    /// The candlelight behind the heart.
    pub candle: [u8; 3],
    /// The heart, the rings and the filled part of the meter.
    pub accent: [u8; 3],
    /// The petals scattered behind everything.
    pub petal: [u8; 3],
    pub gold: [u8; 3],
    pub ink: [u8; 3],
    pub soft: [u8; 3],
    /// How many petals, and how bright they are.
    pub petals: usize,
    pub petal_alpha: u8,
    /// A sad valentine: dim, wilted, and a crack down the heart.
    pub lonely: bool,
}

pub fn look(percent: u8) -> Look {
    // A gentle curve, so the middle of the range already feels warm rather than
    // waiting until the very top to turn.
    let t = (percent as f32 / 100.0).clamp(0.0, 1.0).powf(0.75);
    let lonely = percent < LONELY;
    Look {
        sky: mix([28, 26, 46], [62, 14, 52], t),
        floor: mix([11, 10, 18], [26, 7, 20], t),
        candle: mix([70, 82, 122], [255, 96, 138], t),
        accent: heat(percent),
        petal: mix([92, 100, 128], [255, 130, 165], t),
        gold: mix([150, 152, 168], [255, 208, 140], t),
        ink: mix([226, 228, 238], [255, 244, 246], t),
        soft: mix([150, 154, 170], [226, 178, 192], t),
        petals: 8 + (percent as usize * 52) / 100,
        petal_alpha: (46.0 + 92.0 * t) as u8,
        lonely,
    }
}

// --- the type ----------------------------------------------------------------------------------

/// Faces with romance in them, best first. Nothing is installed everywhere, so
/// this is a wish list: whatever is there wins, and the plain sans catches it
/// if none of them is.
const SCRIPT: &[&str] = &["Snell Roundhand", "Apple Chancery", "Parisienne", "Great Vibes", "Pinyon Script", "Dancing Script", "URW Chancery L", "Z003"];
const SERIF: &[&str] =
    &["Playfair Display", "Bodoni 72", "Didot", "Hoefler Text", "Baskerville", "Palatino", "Book Antiqua", "Charter", "Georgia", "Noto Serif Display", "Noto Serif", "DejaVu Serif", "Liberation Serif", "Times New Roman"];
const PLAIN: &[&str] = &["Montserrat", "Avenir Next", "Noto Sans", "DejaVu Sans", "Liberation Sans"];

fn installed(fs: &FontSystem, names: &[&str]) -> Option<String> {
    names.iter().find(|name| fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == *name))).map(|n| n.to_string())
}

/// Any installed family whose name reads like one of these words, so a server
/// with some serif nobody has heard of still gets a serif.
fn family_like(fs: &FontSystem, words: &[&str], not: &[&str]) -> Option<String> {
    fs.db()
        .faces()
        .flat_map(|f| f.families.iter().map(|(n, _)| n.clone()))
        .find(|name| {
            let lower = name.to_lowercase();
            words.iter().any(|w| lower.contains(w)) && !not.iter().any(|w| lower.contains(w))
        })
}

/// The three faces the card draws with: the ship name, the number and names,
/// and the small lines. Any of them may come out the same as another.
fn faces(fs: &FontSystem) -> (String, String, String) {
    let plain = installed(fs, PLAIN).or_else(|| family_like(fs, &["sans", "helvetica", "arial"], &[])).unwrap_or_else(|| "sans-serif".to_string());
    let serif = installed(fs, SERIF)
        .or_else(|| family_like(fs, &["serif", "roman", "georgia", "garamond", "didone"], &["sans"]))
        .unwrap_or_else(|| plain.clone());
    let script = installed(fs, SCRIPT).or_else(|| family_like(fs, &["script", "chancery", "cursive", "hand"], &["sans"])).unwrap_or_else(|| serif.clone());
    (script, serif, plain)
}

struct Ink<'a> {
    px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    /// Every face each family has, to snap a wanted weight onto a real one: ask
    /// for a weight a family hasn't got and the words come out in the system
    /// sans instead, with no error anywhere.
    faces: HashMap<String, Vec<(FontStyle, Stretch, Weight)>>,
}

impl<'a> Ink<'a> {
    fn new(fs: &'a mut FontSystem, families: &[&str]) -> Option<Ink<'a>> {
        let mut faces = HashMap::new();
        for family in families {
            let list: Vec<(FontStyle, Stretch, Weight)> =
                fs.db().faces().filter(|f| f.families.iter().any(|(n, _)| n == family)).map(|f| (f.style, f.stretch, f.weight)).collect();
            faces.insert(family.to_string(), list);
        }
        Some(Ink { px: Pixmap::new(W as u32, H as u32)?, fs, cache: SwashCache::new(), faces })
    }

    /// The nearest real face of that family. Upright for preference - but a
    /// script is often filed as an italic, and every one of its faces then
    /// fails an upright-only search, which quietly drops the words into the
    /// system fallback with no error anywhere. So: upright if there is one,
    /// otherwise whatever the family actually has.
    fn snap(&self, family: &str, weight: Weight) -> (FontStyle, Stretch, Weight) {
        let list = self.faces.get(family);
        let nearest = |upright: bool| {
            list.and_then(|l| {
                l.iter().filter(|f| !upright || f.0 == FontStyle::Normal).min_by_key(|f| (f.2.0 as i32 - weight.0 as i32).abs()).copied()
            })
        };
        nearest(true).or_else(|| nearest(false)).unwrap_or((FontStyle::Normal, Stretch::Normal, weight))
    }

    fn layout(&mut self, text: &str, family: &str, size: f32, weight: Weight) -> Buffer {
        let (style, stretch, weight) = self.snap(family, weight);
        let family = family.to_string();
        let fs = &mut *self.fs;
        let mut buf = Buffer::new(fs, Metrics::new(size, size * 1.25));
        buf.set_size(fs, None, None);
        let attrs = Attrs::new().family(Family::Name(&family)).weight(weight).style(style).stretch(stretch);
        buf.set_text(fs, text, attrs, Shaping::Advanced);
        buf.shape_until_scroll(fs, false);
        buf
    }

    fn measure(&mut self, text: &str, family: &str, size: f32, weight: Weight) -> f32 {
        let buf = self.layout(text, family, size, weight);
        buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// One line centred on `cx`, with its baseline at `baseline`.
    fn centred(&mut self, text: &str, family: &str, cx: f32, baseline: f32, size: f32, weight: Weight, colour: [u8; 3]) {
        let buf = self.layout(text, family, size, weight);
        let (line_y, w) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
        let (ox, oy) = ((cx - w / 2.0).round() as i32, (baseline - line_y).round() as i32);
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        buf.draw(fs, cache, Color::rgb(colour[0], colour[1], colour[2]), |gx, gy, gw, gh, c| {
            blend_rect(px, ox + gx, oy + gy, gw, gh, c);
        });
    }

    /// The same, sitting on its own shadow, so it reads over a lit heart and
    /// over a dark ground alike.
    fn lifted(&mut self, text: &str, family: &str, cx: f32, baseline: f32, size: f32, weight: Weight, colour: [u8; 3], shadow: [u8; 3]) {
        for (dx, dy) in [(0.0, 3.0), (2.0, 2.0), (-2.0, 2.0)] {
            self.centred(text, family, cx + dx, baseline + dy, size, weight, shadow);
        }
        self.centred(text, family, cx, baseline, size, weight, colour);
    }

    fn fit(&mut self, text: &str, family: &str, size: f32, weight: Weight, max_w: f32) -> String {
        if self.measure(text, family, size, weight) <= max_w {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t = format!("{}…", chars.iter().collect::<String>().trim_end());
            if self.measure(&t, family, size, weight) <= max_w {
                return t;
            }
        }
        "…".to_string()
    }
}

/// Cut to a sane number of characters before anything is measured. Fitting
/// shapes the text once per character it drops, so a name someone pasted a
/// novel into would be shaped a thousand times over; nothing this card draws is
/// ever longer than a line anyway.
fn clip(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => text[..at].to_string(),
        None => text.to_string(),
    }
}

// --- hearts ------------------------------------------------------------------------------------

/// A heart about two units wide and one and three quarters tall, centred on the
/// origin, point down. Scaled and turned into place by the callers.
fn unit_heart() -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 1.00);
    pb.cubic_to(-1.15, 0.22, -0.98, -0.78, -0.44, -0.78);
    pb.cubic_to(-0.19, -0.78, 0.0, -0.58, 0.0, -0.36);
    pb.cubic_to(0.0, -0.58, 0.19, -0.78, 0.44, -0.78);
    pb.cubic_to(0.98, -0.78, 1.15, 0.22, 0.0, 1.00);
    pb.close();
    pb.finish()
}

/// That heart, `w` wide, centred on `(cx, cy)` and turned `deg` degrees.
fn heart_at(cx: f32, cy: f32, w: f32, deg: f32) -> Option<Path> {
    let s = w / 2.15;
    unit_heart()?.transform(Transform::from_translate(cx, cy).pre_rotate(deg).pre_scale(s, s))
}

/// Its bounding box, for filling it to a level.
fn heart_box(cx: f32, cy: f32, w: f32) -> (f32, f32) {
    let s = w / 2.15;
    (cy - 0.78 * s, cy + 1.0 * s)
}

/// A soft halo. One radial gradient rather than a stack of circles: rings of
/// falling alpha band visibly at this size, and a real blur would cost far more
/// than it is worth.
fn glow(px: &mut Pixmap, cx: f32, cy: f32, inner: f32, outer: f32, colour: [u8; 3], strength: u8) {
    let hold = (inner / outer).clamp(0.0, 0.95);
    let Some(rect) = Rect::from_xywh(cx - outer, cy - outer, outer * 2.0, outer * 2.0) else { return };
    let stop = |at: f32, a: u8| GradientStop::new(at, SkColor::from_rgba8(colour[0], colour[1], colour[2], a));
    if let Some(shader) = RadialGradient::new(
        Point::from_xy(cx, cy),
        Point::from_xy(cx, cy),
        outer,
        vec![stop(0.0, strength), stop(hold, strength), stop(hold + (1.0 - hold) * 0.45, strength / 3), stop(1.0, 0)],
        SpreadMode::Pad,
        Transform::identity(),
    ) {
        px.fill_rect(rect, &Paint { shader, anti_alias: true, ..Paint::default() }, Transform::identity(), None);
    }
}

/// A small, repeatable number generator, so a card drawn twice is the same
/// card: the petals must not dance about between one posting and the next.
struct Seeded(u64);

impl Seeded {
    fn of(text: &str, salt: u64) -> Seeded {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ salt;
        for byte in text.as_bytes() {
            h ^= *byte as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Seeded(h | 1)
    }

    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f32 / (1u64 << 53) as f32
    }

    fn between(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// Petals across the whole card, behind everything: a grey handful for a pair
/// with nothing between them, a shower for a pair with everything. A lonely
/// card's petals fall upside down, like they have come off the stem.
fn scatter(px: &mut Pixmap, l: &Look, seed: &str) {
    let mut rng = Seeded::of(seed, 0x5eed);
    for i in 0..l.petals {
        let (x, y) = (rng.between(-20.0, W + 20.0), rng.between(-20.0, H + 20.0));
        // Keep the middle band clear so the faces and the heart stay clean.
        let middle = (y - FACE_CY).abs() < 120.0 && (x - HEART_CX).abs() < 400.0;
        let size = rng.between(9.0, 26.0) * if middle { 0.55 } else { 1.0 };
        let deg = if l.lonely { rng.between(150.0, 210.0) } else { rng.between(-32.0, 32.0) };
        let alpha = (l.petal_alpha as f32 * rng.between(0.45, 1.0) * if middle { 0.5 } else { 1.0 }) as u8;
        let colour = if i % 5 == 0 { l.gold } else { l.petal };
        if let Some(path) = heart_at(x, y, size, deg) {
            px.fill_path(&path, &paint(colour, alpha), FillRule::Winding, Transform::identity(), None);
        }
    }
}

// --- the pieces --------------------------------------------------------------------------------

fn ground(px: &mut Pixmap, l: &Look) {
    px.fill(SkColor::from_rgba8(l.floor[0], l.floor[1], l.floor[2], 255));
    if let (Some(rect), Some(shader)) = (
        Rect::from_xywh(0.0, 0.0, W, H),
        LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(0.0, H),
            vec![
                GradientStop::new(0.0, SkColor::from_rgba8(l.sky[0], l.sky[1], l.sky[2], 255)),
                GradientStop::new(0.62, SkColor::from_rgba8(l.floor[0], l.floor[1], l.floor[2], 255)),
                GradientStop::new(1.0, SkColor::from_rgba8(l.floor[0], l.floor[1], l.floor[2], 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ),
    ) {
        px.fill_rect(rect, &Paint { shader, ..Paint::default() }, Transform::identity(), None);
    }
    // Candlelight behind the heart.
    if let (Some(rect), Some(shader)) = (
        Rect::from_xywh(0.0, 0.0, W, H),
        RadialGradient::new(
            Point::from_xy(HEART_CX, HEART_CY),
            Point::from_xy(HEART_CX, HEART_CY),
            420.0,
            vec![
                GradientStop::new(0.0, SkColor::from_rgba8(l.candle[0], l.candle[1], l.candle[2], 92)),
                GradientStop::new(1.0, SkColor::from_rgba8(l.candle[0], l.candle[1], l.candle[2], 0)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ),
    ) {
        px.fill_rect(rect, &Paint { shader, anti_alias: true, ..Paint::default() }, Transform::identity(), None);
    }
}

/// A point on the garland, as a fraction along it.
fn garland_at(t: f32) -> (f32, f32) {
    let (p0, p1, p2) = ((LEFT_CX + outer() - 12.0, FACE_CY + 18.0), (HEART_CX, FACE_CY + 132.0), (RIGHT_CX - outer() + 12.0, FACE_CY + 18.0));
    let u = 1.0 - t;
    (u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0, u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1)
}

/// The swag joining the two of them, with small hearts strung along it. Drawn
/// before the heart and the faces, so it passes behind them.
fn garland(px: &mut Pixmap, l: &Look) {
    let (sx, sy) = garland_at(0.0);
    let (ex, ey) = garland_at(1.0);
    let mut pb = PathBuilder::new();
    pb.move_to(sx, sy);
    pb.quad_to(HEART_CX, FACE_CY + 132.0, ex, ey);
    let Some(path) = pb.finish() else { return };
    let strength = if l.lonely { 130 } else { 235 };
    px.stroke_path(&path, &paint(l.gold, strength / 4), &Stroke { width: 12.0, ..Stroke::default() }, Transform::identity(), None);
    px.stroke_path(&path, &paint(l.gold, strength), &Stroke { width: 4.0, ..Stroke::default() }, Transform::identity(), None);
    // Only out at the ends, where the heart doesn't cover them.
    for (i, t) in [0.06_f32, 0.15, 0.25, 0.75, 0.85, 0.94].into_iter().enumerate() {
        let (x, y) = garland_at(t);
        let size = if i % 3 == 1 { 22.0 } else { 15.0 };
        let deg = if l.lonely { 180.0 } else { (t - 0.5) * 44.0 };
        if let Some(h) = heart_at(x, y + size * 0.3, size, deg) {
            px.fill_path(&h, &paint(mix(l.accent, [255, 255, 255], 0.15), if l.lonely { 150 } else { 245 }), FillRule::Winding, Transform::identity(), None);
        }
    }
}

fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase().to_string()).unwrap_or_else(|| "?".to_string())
}

fn portrait(ink: &mut Ink, serif: &str, cx: f32, face: &Face, l: &Look) {
    let o = outer();
    glow(&mut ink.px, cx, FACE_CY, o, o + 46.0, l.candle, if l.lonely { 46 } else { 120 });
    fill_circle(&mut ink.px, cx, FACE_CY, o, l.accent);
    fill_circle(&mut ink.px, cx, FACE_CY, o - RING, dim(l.sky, 0.8));
    match avatar_pixmap(face.avatar.as_deref(), AV as u32, false) {
        Some(pm) => ink.px.draw_pixmap(
            (cx - AV / 2.0).round() as i32,
            (FACE_CY - AV / 2.0).round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        // No picture, or one that wouldn't decode: their initial instead.
        None => {
            fill_circle(&mut ink.px, cx, FACE_CY, AV / 2.0, dim(l.accent, 0.30));
            ink.centred(&initial(&face.name), serif, cx, FACE_CY + 26.0, 74.0, Weight::BOLD, l.ink);
        }
    }
}

/// The heart in the middle, filled to the score the way a glass fills, with the
/// number on it. A lonely one is cracked down the middle.
fn meter(ink: &mut Ink, serif: &str, percent: u8, l: &Look) {
    let Some(shape) = heart_at(HEART_CX, HEART_CY, HEART_W, 0.0) else { return };
    let (top, bottom) = heart_box(HEART_CX, HEART_CY, HEART_W);
    glow(&mut ink.px, HEART_CX, HEART_CY, HEART_W * 0.42, HEART_W * 0.78, l.candle, if l.lonely { 54 } else { 140 });

    // The empty part is clear glass over the candlelight - an unfilled heart,
    // not a burnt one - and then the filled part is clipped to the same outline.
    ink.px.fill_path(&shape, &paint(mix(l.sky, [255, 255, 255], 0.18), 140), FillRule::Winding, Transform::identity(), None);
    let level = bottom - (bottom - top) * (percent as f32 / 100.0);
    if let (Some(mut mask), Some(rect)) = (Mask::new(W as u32, H as u32), Rect::from_xywh(HEART_CX - HEART_W, level, HEART_W * 2.0, bottom - level + 2.0)) {
        mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
        let hot = l.accent;
        let cool = mix(l.accent, l.candle, 0.55);
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(0.0, bottom),
            Point::from_xy(0.0, top),
            vec![
                GradientStop::new(0.0, SkColor::from_rgba8(hot[0], hot[1], hot[2], 255)),
                GradientStop::new(1.0, SkColor::from_rgba8(cool[0], cool[1], cool[2], 255)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            ink.px.fill_rect(rect, &Paint { shader, anti_alias: true, ..Paint::default() }, Transform::identity(), Some(&mask));
        }
        // A soft band of light where it has filled to, so the level reads at a
        // glance without a hard rule cutting the heart in two.
        if percent > 3 && percent < 98 {
            for a in [(0.0_f32, 98u8), (2.5, 62), (5.5, 34), (9.0, 15)] {
                if let Some(band) = Rect::from_xywh(HEART_CX - HEART_W, level + a.0, HEART_W * 2.0, 3.0) {
                    ink.px.fill_rect(band, &paint([255, 255, 255], a.1), Transform::identity(), Some(&mask));
                }
            }
        }
    }

    // A crack down a lonely heart: the joke, not a fault.
    if l.lonely {
        let mut pb = PathBuilder::new();
        pb.move_to(HEART_CX + 6.0, top + HEART_W * 0.06);
        for (dx, dy) in [(-16.0, 0.16), (14.0, 0.30), (-12.0, 0.46), (16.0, 0.62), (-6.0, 0.80), (2.0, 0.97)] {
            pb.line_to(HEART_CX + dx, top + (bottom - top) * dy);
        }
        if let Some(crack) = pb.finish() {
            ink.px.stroke_path(&crack, &paint(dim(l.floor, 0.9), 255), &Stroke { width: 9.0, ..Stroke::default() }, Transform::identity(), None);
            ink.px.stroke_path(&crack, &paint(l.accent, 90), &Stroke { width: 2.0, ..Stroke::default() }, Transform::identity(), None);
        }
    }

    // A gloss on the upper left lobe, so it reads as something with a surface.
    if let (Some(mut mask), Some(oval)) = (
        Mask::new(W as u32, H as u32),
        Rect::from_xywh(HEART_CX - HEART_W * 0.38, top + HEART_W * 0.06, HEART_W * 0.30, HEART_W * 0.17),
    ) {
        mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
        if let Some(shine) = PathBuilder::from_oval(oval) {
            ink.px.fill_path(&shine, &paint([255, 255, 255], 46), FillRule::Winding, Transform::from_rotate_at(-22.0, HEART_CX, HEART_CY), Some(&mask));
        }
    }

    // A bright rim, so the heart holds its shape whatever it is filled with.
    ink.px.stroke_path(&shape, &paint(mix(l.accent, [255, 255, 255], 0.55), 240), &Stroke { width: 4.0, ..Stroke::default() }, Transform::identity(), None);
    ink.px.stroke_path(&shape, &paint(l.gold, 90), &Stroke { width: 9.0, ..Stroke::default() }, Transform::identity(), None);
    ink.lifted(&format!("{}%", percent), serif, HEART_CX, HEART_CY + 32.0, 88.0, Weight::BOLD, l.ink, dim(l.floor, 0.5));
}

/// Draws the card. `None` when the canvas or the encoder gives out - the
/// caller then posts the text version instead.
pub fn render(card: &Card, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let l = look(card.percent);
    let (script, serif, plain) = faces(fs);
    let mut ink = Ink::new(fs, &[script.as_str(), serif.as_str(), plain.as_str()])?;

    ground(&mut ink.px, &l);
    scatter(&mut ink.px, &l, &format!("{}{}", card.ship, card.left.name));
    garland(&mut ink.px, &l);
    portrait(&mut ink, &serif, LEFT_CX, &card.left, &l);
    portrait(&mut ink, &serif, RIGHT_CX, &card.right, &l);
    meter(&mut ink, &serif, card.percent, &l);

    // The ship name across the top, shrunk until it fits, with a heart either side.
    let wanted = clip(&card.ship, 48);
    let room = W - 2.0 * 140.0;
    let mut size = 92.0_f32;
    while size > 34.0 && ink.measure(&wanted, &script, size, Weight::BOLD) > room {
        size -= 6.0;
    }
    let ship = ink.fit(&wanted, &script, size, Weight::BOLD, room);
    let half = ink.measure(&ship, &script, size, Weight::BOLD) / 2.0;
    ink.lifted(&ship, &script, HEART_CX, SHIP_Y, size, Weight::BOLD, l.ink, dim(l.floor, 0.6));
    for side in [-1.0_f32, 1.0] {
        if let Some(h) = heart_at(HEART_CX + side * (half + 40.0), SHIP_Y - size * 0.22, 26.0, side * 18.0) {
            ink.px.fill_path(&h, &paint(l.accent, if l.lonely { 140 } else { 235 }), FillRule::Winding, Transform::identity(), None);
        }
    }

    // Their names under their faces, and - only when it has moved - which way.
    for (cx, face) in [(LEFT_CX, &card.left), (RIGHT_CX, &card.right)] {
        let name = ink.fit(&clip(&face.name, 48), &serif, 32.0, Weight::BOLD, 2.0 * outer() + 60.0);
        ink.centred(&name, &serif, cx, NAME_Y, 32.0, Weight::BOLD, l.ink);
    }
    if let Some(moved) = card.moved.as_deref().filter(|m| !m.trim().is_empty()) {
        let down = moved.starts_with("down");
        let colour = if down { mix(l.soft, [248, 113, 113], 0.75) } else { mix(l.soft, [134, 239, 172], 0.75) };
        let text = ink.fit(&clip(&format!("{} {}", if down { "▾" } else { "▴" }, moved), 72), &plain, 23.0, Weight::SEMIBOLD, 420.0);
        ink.centred(&text, &plain, HEART_CX, NAME_Y - 2.0, 23.0, Weight::SEMIBOLD, colour);
    }

    // One counted fact, and the signature.
    let line = ink.fit(&clip(&card.line, 80), &plain, 25.0, Weight::SEMIBOLD, W - 160.0);
    ink.centred(&line, &plain, HEART_CX, LINE_Y, 25.0, Weight::SEMIBOLD, l.soft);
    ink.centred(CAPTION, &plain, HEART_CX, CAPTION_Y, 16.0, Weight::MEDIUM, dim(l.soft, 0.7));
    ink.px.encode_png().ok()
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
        // Every score draws, and the meter never runs off the heart.
        for percent in [0u8, 1, 7, 24, 25, 50, 99, 100] {
            drawn(&Card { percent, ..pair() });
        }
    }

    #[test]
    fn the_same_card_drawn_twice_is_the_same_card() {
        assert_eq!(drawn(&pair()), drawn(&pair()), "the petals must not dance about between postings");
        assert_ne!(drawn(&pair()), drawn(&Card { ship: "Devya".into(), ..pair() }), "a different pair gets its own scattering");
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
        drawn(&Card { ship: String::new(), left: Face::default(), right: Face::default(), moved: None, ..pair() });
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
    fn the_card_warms_up_with_the_score_and_a_lonely_one_looks_lonely() {
        let (cold, warm, hot) = (look(0), look(50), look(100));
        // The accent is a valentine at every score: never a traffic-light amber.
        assert_eq!((heat(0), heat(50), heat(100)), ([126, 148, 200], [230, 150, 175], [255, 70, 110]));
        for p in 0..=100u8 {
            let c = heat(p);
            assert!(c[0] as i32 >= c[1] as i32 - 60, "{}% has gone green-yellow: {:?}", p, c);
        }
        // A shower of petals at the top, a handful at the bottom.
        assert!(cold.petals < 12 && hot.petals > 50, "{} then {}", cold.petals, hot.petals);
        assert!(warm.petals > cold.petals && hot.petals > warm.petals);
        assert!(hot.petal_alpha > cold.petal_alpha);
        // Warmer ground as it climbs: a cold navy turns a deep rose plum, so
        // what matters is red against blue, not blue falling on its own.
        let warmth = |c: [u8; 3]| c[0] as f32 / c[2].max(1) as f32;
        assert!(hot.sky[0] > cold.sky[0] && warmth(hot.sky) > warmth(cold.sky), "{:?} then {:?}", cold.sky, hot.sky);
        assert!(hot.candle[0] > warm.candle[0] && warm.candle[0] > cold.candle[0]);
        // Only the bottom of the range is the sad valentine.
        assert!(look(0).lonely && look(LONELY - 1).lonely);
        assert!(!look(LONELY).lonely && !look(100).lonely);
        // Every accent stays bright enough to read on Discord's dark theme.
        for p in 0..=100u8 {
            let c = heat(p);
            let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
            assert!(lum > 100.0, "{}% is too dark to read: {:?} ({})", p, c, lum);
            // And the ground stays dark enough for it to read against.
            let l = look(p);
            let ground = 0.299 * l.sky[0] as f32 + 0.587 * l.sky[1] as f32 + 0.114 * l.sky[2] as f32;
            assert!(ground < 60.0, "{}% has too light a ground: {:?}", p, l.sky);
        }
        // It moves smoothly: no step between neighbouring scores is a jump.
        for p in 1..=100u8 {
            let (a, b) = (heat(p - 1), heat(p));
            let step: i32 = (0..3).map(|i| (a[i] as i32 - b[i] as i32).abs()).sum();
            assert!(step <= 30, "{}% jumps: {:?} -> {:?}", p, a, b);
        }
    }

    #[test]
    fn the_hearts_are_real_shapes_and_the_meter_fills_from_the_bottom() {
        let h = heart_at(100.0, 100.0, 50.0, 0.0).expect("a heart");
        // `bounds` is the control hull, not the drawn outline, so it runs a
        // little wide of the curve it describes - close is all this can ask.
        let b = h.bounds();
        assert!((b.width() - 50.0).abs() < 8.0, "the heart is about the width it was asked for: {}", b.width());
        assert!(b.height() > 35.0 && b.height() < 55.0, "and about as tall: {}", b.height());
        assert!(heart_at(0.0, 0.0, 1.0, 45.0).is_some(), "a tiny turned one still builds");
        let (top, bottom) = heart_box(100.0, 100.0, 50.0);
        assert!(top < 100.0 && bottom > 100.0 && bottom - top > 35.0);
        // The garland starts and ends at the two faces and dips between them.
        let (sx, sy) = garland_at(0.0);
        let (mx, my) = garland_at(0.5);
        let (ex, _) = garland_at(1.0);
        assert!(sx < HEART_CX && ex > HEART_CX && (mx - HEART_CX).abs() < 1.0);
        assert!(my > sy, "it hangs down between them");
    }

    /// Writes the cards out so they can be looked at. Ignored by default - run
    /// with `SHIP_CARD_DIR=<dir> cargo test ship_card_sample -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn ship_card_sample() {
        {
            let fs = super::super::awards::fonts();
            let (script, serif, plain) = faces(&fs.lock());
            println!("faces: ship name = {script:?}, number and names = {serif:?}, small lines = {plain:?}");
        }
        let dir = std::env::var("SHIP_CARD_DIR").unwrap_or_else(|_| std::env::temp_dir().display().to_string());
        for (name, card) in [
            ("ship-87", pair()),
            ("ship-4", Card { ship: "Devya".into(), percent: 4, line: "Not one word between them in 60 days".into(), moved: None, ..pair() }),
            ("ship-99", Card { ship: "Gootus".into(), percent: 99, line: "1,204 replies in 60 days".into(), moved: Some("up 6 since 7 days ago".into()), ..pair() }),
            ("ship-52", Card { ship: "Kavyaan".into(), percent: 52, line: "212 replies in 60 days".into(), moved: None, ..pair() }),
            ("ship-noavatar", Card { left: Face { name: "arjun".into(), avatar: None }, right: Face { name: "riya".into(), avatar: None }, percent: 61, line: "412 replies in 60 days".into(), moved: None, ..pair() }),
            (
                "ship-emoji",
                Card {
                    ship: "Aʀᴊʏᴀ 💞".into(),
                    left: Face { name: "🔥 arjun".into(), avatar: Some(fake_avatar([92, 124, 214])) },
                    right: Face { name: "riya 😭".into(), avatar: None },
                    percent: 33,
                    line: "61 pings at each other, 0 replies".into(),
                    moved: None,
                },
            ),
            ("ship-longest", Card { ship: "Supercalifragilistic".into(), percent: 72, line: "W".repeat(super::super::roast_build::HEADLINE_CHARS), ..pair() }),
        ] {
            let path = std::path::Path::new(&dir).join(format!("{}.png", name));
            std::fs::write(&path, png(&card).expect("the card draws")).expect("writing the sample");
            println!("wrote {}", path.display());
        }
    }
}
