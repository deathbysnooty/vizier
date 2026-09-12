//! The sorting-hat card.
//!
//! Drawn when someone is sorted into a house: their picture, the house's
//! colours and crest, and the hat's verdict. Drawing is CPU work, so callers
//! run it on a blocking thread.
//!
//! Everything is painted from paths and gradients, with no image assets: a
//! ground washed in the house's own colour, the portrait ringed in both of
//! them, and a slumped hat drawn into the bottom corner. The four houses run
//! from deep scarlet to pale yellow, so nothing here may depend on the ground
//! being dark - the type carries its own outline and the crest sits on its own
//! disc, which is what keeps one design readable on all four.
//!
//! Fonts come from the list `awards::fonts()` holds, so a caller must not
//! already be holding that lock when it calls in here.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Stretch, Style as FontStyle, SwashCache,
    Weight, Wrap,
};
use tiny_skia::{
    Color as SkColor, FillRule, GradientStop, LinearGradient, Paint, PathBuilder, Pixmap, PixmapPaint, Point,
    RadialGradient, Rect, Shader, SpreadMode, Stroke, Transform,
};

use super::awards_card::{avatar_pixmap, blend_rect, fill_circle, paint, rrect};

/// Everything the card needs about the member and the house they landed in.
pub struct Sorted {
    /// Member's display name.
    pub name: String,
    /// Raw bytes of their profile picture; `None` draws a silhouette.
    pub avatar: Option<Vec<u8>>,
    /// "Gryffindor", "Slytherin", "Ravenclaw", "Hufflepuff".
    pub house: String,
    /// The house's emoji: 🦁 🐍 🦅 🦡
    pub crest: String,
    /// The house's two colours, primary first.
    pub colours: ([u8; 3], [u8; 3]),
    /// The hat's line, e.g. "Brave to the point of trouble - GRYFFINDOR!".
    pub line: String,
}

const W: f32 = 1000.0;
const H: f32 = 520.0;

// The portrait, slightly left, with the type column beside it.
const AV: f32 = 300.0;
/// Dark gap between the picture and the inner band.
const GAP: f32 = 6.0;
/// Outer band, in the primary colour.
const RING_OUT: f32 = 8.0;
/// Inner band, in the secondary.
const RING_IN: f32 = 6.0;
const OUTER: f32 = AV / 2.0 + GAP + RING_OUT + RING_IN;
const PORTRAIT_CX: f32 = 232.0;
const PORTRAIT_CY: f32 = 196.0;

/// The house crests, as supplied. Baked into the binary so there is nothing to
/// copy at deploy time and nothing that can go missing or drift out of step
/// with the code.
const CRESTS: [(&str, &[u8]); 4] = [
    ("gryffindor", include_bytes!("crests/gryffindor.png")),
    ("slytherin", include_bytes!("crests/slytherin.png")),
    ("ravenclaw", include_bytes!("crests/ravenclaw.png")),
    ("hufflepuff", include_bytes!("crests/hufflepuff.png")),
];

/// Side of the crest artwork on the card. The source is 64px, so this is an
/// enlargement; kept a little under the badge so the ivory keyline has room.
const CREST_ART: f32 = 112.0;

/// A house's artwork, square at `side` pixels, or `None` for an unknown house
/// or bytes that don't decode.
fn crest_art(house: &str, side: u32) -> Option<Pixmap> {
    let wanted = house.trim().to_ascii_lowercase();
    let (_, bytes) = CRESTS.iter().find(|(key, _)| *key == wanted)?;
    let decoded = image::load_from_memory(bytes).ok()?;
    // Being enlarged, not reduced: Lanczos keeps these bold outlines crisp
    // where the default filter would leave them soft.
    let scaled = decoded.resize_exact(side, side, image::imageops::FilterType::Lanczos3).into_rgba8();
    let mut px = Pixmap::new(side, side)?;
    for (slot, pixel) in px.pixels_mut().iter_mut().zip(scaled.pixels()) {
        let [r, g, b, a] = pixel.0;
        // tiny-skia keeps its pixels premultiplied; the PNG's are not.
        *slot = tiny_skia::ColorU8::from_rgba(r, g, b, a).premultiply();
    }
    Some(px)
}

// The crest badge, on the portrait's lower-right shoulder.
const CREST_R: f32 = 74.0;
const CREST_SIZE: f32 = 116.0;

// The type column: eyebrow, house, member name.
const TEXT_X: f32 = 446.0;
const TEXT_W: f32 = W - TEXT_X - 44.0;
const EYEBROW_BASE: f32 = 140.0;
const HOUSE_BASE: f32 = 208.0;
const HOUSE_SIZE: f32 = 60.0;
/// A house name may shrink this far before it is cut short instead.
const HOUSE_FLOOR: f32 = 38.0;
const NAME_BASE: f32 = 258.0;
const NAME_SIZE: f32 = 30.0;

// The hat's line, along the foot.
const PANEL_X: f32 = 44.0;
const PANEL_Y: f32 = 414.0;
const PANEL_W: f32 = W - 2.0 * PANEL_X;
const PANEL_H: f32 = 82.0;
const PANEL_PAD: f32 = 34.0;
const PANEL_R: f32 = 22.0;

// The drawn hat, in the band left empty between the name and the panel.
const HAT_CX: f32 = 858.0;
const HAT_BASE: f32 = 396.0;
const HAT_H: f32 = 150.0;

const INK: [u8; 3] = [252, 252, 253];
const SUB: [u8; 3] = [233, 235, 241];
const LINE_INK: [u8; 3] = [235, 237, 243];
/// The outline under every piece of type, and the shadow colour.
const DARK: [u8; 3] = [9, 9, 12];

/// Unit offsets that put a dark edge round a word; scaled by `Label::edge`.
const OUTLINE: [(f32, f32); 8] =
    [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (-0.7, -0.7), (0.7, -0.7), (-0.7, 0.7), (0.7, 0.7)];

/// Sparkles off the hat's tip, as (dx, dy, radius, alpha) from the hat's base.
/// Fixed, not random: the same card drawn twice should come out identical.
const SPARKS: [(f32, f32, f32, u8); 6] = [
    (56.0, -104.0, 7.0, 205),
    (26.0, -118.0, 4.5, 155),
    (-8.0, -96.0, 5.5, 175),
    (-38.0, -112.0, 3.5, 125),
    (74.0, -126.0, 4.0, 140),
    (-20.0, -70.0, 3.0, 105),
];

/// PNG bytes of the sorting card, or `None` if drawing failed.
pub fn sorting_png(sorted: &Sorted) -> Option<Vec<u8>> {
    let mut fs = super::awards::fonts().lock();
    // cosmic-text panics rather than drawing with no fonts at all.
    if fs.db().len() == 0 {
        return None;
    }
    let mut pen = Pen::new(W, H, &mut fs)?;
    draw(&mut pen, sorted);
    pen.px.encode_png().ok()
}

fn draw(pen: &mut Pen<'_>, s: &Sorted) {
    let (primary, secondary) = s.colours;
    ground(&mut pen.px, primary);
    // The glow behind the portrait is the secondary colour, so both house
    // colours are in the ground before anything is drawn on top of it.
    glow(&mut pen.px, PORTRAIT_CX, PORTRAIT_CY, OUTER + 230.0, lift(secondary, 0.32), 92);
    vignette(&mut pen.px, 168);
    hat(&mut pen.px, lift(primary, 0.80));
    for (dx, dy, r, alpha) in SPARKS {
        sparkle(&mut pen.px, HAT_CX + dx, HAT_BASE + dy, r, lift(secondary, 0.78), alpha);
    }

    portrait(pen, s.avatar.as_deref(), primary, secondary);
    crest(pen, &s.house, s.crest.trim(), secondary);
    type_column(pen, s, secondary);
    foot(pen, s.line.trim());
}

/// A deep vertical wash of the house colour: lit across the top, sinking to
/// near-black at the foot so the panel and the type have something to sit on.
fn ground(px: &mut Pixmap, primary: [u8; 3]) {
    // Opaque first. The card is viewed in both Discord themes, so a gradient
    // that failed to build must not leave the canvas transparent.
    px.fill(sk(mix(primary, 0.58, [14, 13, 20]), 255));
    let stops = [
        (0.0, mix(primary, 0.42, [20, 18, 28])),
        (0.34, mix(primary, 0.58, [14, 13, 20])),
        (0.72, mix(primary, 0.76, [10, 10, 15])),
        (1.0, mix(primary, 0.88, [8, 8, 12])),
    ];
    if let (Some(shader), Some(area)) = (down(0.0, H, &stops), Rect::from_xywh(0.0, 0.0, W, H)) {
        let mut p = Paint::default();
        p.shader = shader;
        px.fill_rect(area, &p, Transform::identity(), None);
    }
}

/// Halo, shadow, both bands, then the picture. The halo goes down before the
/// shadow so the portrait reads as standing off the ground rather than pasted
/// onto it.
fn portrait(pen: &mut Pen<'_>, avatar: Option<&[u8]>, primary: [u8; 3], secondary: [u8; 3]) {
    glow(&mut pen.px, PORTRAIT_CX, PORTRAIT_CY, OUTER + 86.0, lift(secondary, 0.46), 116);
    shadow(&mut pen.px, PORTRAIT_CX, PORTRAIT_CY + 18.0, OUTER);
    band(&mut pen.px, PORTRAIT_CX, PORTRAIT_CY, OUTER, primary);
    band(&mut pen.px, PORTRAIT_CX, PORTRAIT_CY, OUTER - RING_OUT, secondary);
    // A dark gap, so a light picture never bleeds into the inner band.
    fill_circle(&mut pen.px, PORTRAIT_CX, PORTRAIT_CY, AV / 2.0 + GAP, [11, 11, 15]);
    pen.picture(PORTRAIT_CX, PORTRAIT_CY, AV, avatar);
}

/// One band of the ring, lit from the top left. Hufflepuff's secondary is
/// black, so the gradient runs from a lifted tint rather than the raw colour -
/// otherwise that band would vanish into the gap behind it.
fn band(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
    let Some(path) = PathBuilder::from_circle(cx, cy, r) else { return };
    let stops = vec![GradientStop::new(0.0, sk(lift(c, 0.50), 255)), GradientStop::new(1.0, sk(dim(c, 0.62), 255))];
    let shader = LinearGradient::new(
        Point::from_xy(cx - r, cy - r),
        Point::from_xy(cx + r, cy + r),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    fill_shaded(px, &path, shader, c);
}

/// The house emoji on a dark translucent disc. The disc is dark and its
/// hairline is a lifted secondary, so the badge reads the same on scarlet as
/// it does on yellow.
fn crest(pen: &mut Pen<'_>, house: &str, crest: &str, secondary: [u8; 3]) {
    let art = crest_art(house, CREST_ART as u32);
    if art.is_none() && crest.is_empty() {
        return;
    }
    // 45 degrees down and right of centre, on the portrait's shoulder.
    let d = OUTER * std::f32::consts::FRAC_1_SQRT_2;
    let (cx, cy) = (PORTRAIT_CX + d, PORTRAIT_CY + d);
    wash(&mut pen.px, cx, cy + 5.0, CREST_R + 7.0, DARK, 96);
    fill_circle(&mut pen.px, cx, cy, CREST_R, [16, 16, 21]);
    wash(&mut pen.px, cx, cy, CREST_R, [255, 255, 255], 16);
    if let Some(edge) = PathBuilder::from_circle(cx, cy, CREST_R - 1.5) {
        let stroke = Stroke { width: 3.0, ..Stroke::default() };
        pen.px.stroke_path(&edge, &paint(lift(secondary, 0.55), 235), &stroke, Transform::identity(), None);
    }
    match art {
        Some(art) => {
            let corner = |centre: f32| (centre - CREST_ART / 2.0).round() as i32;
            let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..PixmapPaint::default() };
            pen.px.draw_pixmap(corner(cx), corner(cy), art.as_ref(), &paint, Transform::identity(), None);
        }
        // No artwork for this house: fall back to the emoji rather than an
        // empty badge. Colour emoji come from whichever installed face has the
        // glyph, so the weight asked for here only steers the fallback.
        None => pen.centered(crest, cx, cy + CREST_SIZE * 0.36, CREST_SIZE, Weight::NORMAL, INK),
    }
}

/// The eyebrow, the house and the member's name, stacked to the right of the
/// portrait and optically centred against it.
fn type_column(pen: &mut Pen<'_>, s: &Sorted, secondary: [u8; 3]) {
    pen.label(Label {
        text: &spaced("SORTED INTO"),
        x: TEXT_X,
        baseline: EYEBROW_BASE,
        size: 16.0,
        weight: Weight::SEMIBOLD,
        ink: lift(secondary, 0.62),
        edge: 1.5,
    });

    let house = s.house.trim().to_uppercase();
    let house = spaced(if house.is_empty() { "UNSORTED" } else { &house });
    // The biggest type on the card, so it shrinks to fit rather than being cut
    // short: an ellipsised house name would read as a drawing bug.
    let (house, size) = pen.shrink(&house, HOUSE_SIZE, HOUSE_FLOOR, Weight::EXTRA_BOLD, TEXT_W);
    pen.label(Label {
        text: &house,
        x: TEXT_X,
        baseline: HOUSE_BASE,
        size,
        weight: Weight::EXTRA_BOLD,
        ink: INK,
        edge: 3.0,
    });

    let name = s.name.trim();
    if name.is_empty() {
        return;
    }
    let name = pen.fit(name, NAME_SIZE, Weight::BOLD, TEXT_W);
    pen.label(Label {
        text: &name,
        x: TEXT_X,
        baseline: NAME_BASE,
        size: NAME_SIZE,
        weight: Weight::BOLD,
        ink: SUB,
        edge: 2.0,
    });
}

/// The hat's verdict, wrapped to two lines on a translucent slab with a lit
/// top edge. An empty line leaves the ground bare rather than an empty box.
fn foot(pen: &mut Pen<'_>, line: &str) {
    if line.is_empty() {
        return;
    }
    pen.panel();
    let buf = pen.paragraph(line, 21.0, 28.0, PANEL_W - 2.0 * PANEL_PAD, 2);
    let top = PANEL_Y + (PANEL_H - Pen::height(&buf)) / 2.0;
    pen.draw(&buf, PANEL_X + PANEL_PAD, top, LINE_INK);
}

/// A wizard's hat, drawn rather than dropped in as an asset: a cone leaning
/// right whose point flops over and hooks down, sitting on a brim. Low alpha,
/// and in a band nothing else uses, so it reads as a mark rather than clip-art.
fn hat(px: &mut Pixmap, c: [u8; 3]) {
    let (cx, base, h) = (HAT_CX, HAT_BASE, HAT_H);
    let hw = h * 0.40;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - hw, base);
    pb.cubic_to(cx - hw * 0.85, base - h * 0.50, cx - hw * 0.45, base - h * 0.78, cx + hw * 0.12, base - h * 0.94);
    pb.cubic_to(cx + hw * 0.62, base - h * 1.00, cx + hw * 1.05, base - h * 0.86, cx + hw * 0.88, base - h * 0.66);
    pb.cubic_to(cx + hw * 0.62, base - h * 0.40, cx + hw * 0.80, base - h * 0.16, cx + hw, base);
    pb.close();
    if let Some(cone) = pb.finish() {
        px.fill_path(&cone, &paint(c, 50), FillRule::Winding, Transform::identity(), None);
    }
    // tiny-skia has no ellipse primitive, and at this alpha a squat rounded
    // rectangle stands in for the brim well enough.
    if let Some(brim) = rrect(cx - hw * 1.55, base - h * 0.05, hw * 3.10, h * 0.13, h * 0.065) {
        px.fill_path(&brim, &paint(c, 56), FillRule::Winding, Transform::identity(), None);
    }
    if let Some(strap) = rrect(cx - hw * 0.92, base - h * 0.17, hw * 1.84, h * 0.07, h * 0.02) {
        px.fill_path(&strap, &paint(c, 38), FillRule::Winding, Transform::identity(), None);
    }
}

/// A four-point sparkle: a diamond with its sides pulled in to the centre.
fn sparkle(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3], alpha: u8) {
    let waist = r * 0.22;
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - r);
    pb.quad_to(cx + waist, cy - waist, cx + r, cy);
    pb.quad_to(cx + waist, cy + waist, cx, cy + r);
    pb.quad_to(cx - waist, cy + waist, cx - r, cy);
    pb.quad_to(cx - waist, cy - waist, cx, cy - r);
    pb.close();
    if let Some(path) = pb.finish() {
        px.fill_path(&path, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

/// A soft shadow, faked as stacked discs: tiny-skia has no blur.
fn shadow(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    for i in 0..12 {
        let t = i as f32 / 12.0;
        wash(px, cx, cy + 14.0, r + 2.0 + t * 30.0, DARK, (32.0 * (1.0 - t)) as u8);
    }
}

/// Edges pulled into darkness, so the middle of the card carries the eye.
fn vignette(px: &mut Pixmap, strength: u8) {
    let stops = vec![
        GradientStop::new(0.40, sk([0, 0, 0], 0)),
        GradientStop::new(1.0, sk([0, 0, 0], strength)),
    ];
    let shader = RadialGradient::new(
        Point::from_xy(W / 2.0, H / 2.0),
        Point::from_xy(W / 2.0, H / 2.0),
        W * 0.70,
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(shader), Some(area)) = (shader, Rect::from_xywh(0.0, 0.0, W, H)) {
        let mut p = Paint::default();
        p.shader = shader;
        px.fill_rect(area, &p, Transform::identity(), None);
    }
}

/// A neutral stand-in when somebody has no profile picture.
fn silhouette(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    fill_circle(px, cx, cy, r, [52, 55, 64]);
    let (Some(mut mask), Some(disc)) =
        (tiny_skia::Mask::new(px.width(), px.height()), PathBuilder::from_circle(cx, cy, r))
    else {
        return;
    };
    mask.fill_path(&disc, FillRule::Winding, true, Transform::identity());
    let figure = paint([96, 101, 115], 255);
    let body = PathBuilder::from_circle(cx, cy + r * 0.80, r * 0.62);
    let head = PathBuilder::from_circle(cx, cy - r * 0.22, r * 0.30);
    for shape in [body, head] {
        if let Some(path) = shape {
            px.fill_path(&path, &figure, FillRule::Winding, Transform::identity(), Some(&mask));
        }
    }
}

fn sk(c: [u8; 3], a: u8) -> SkColor {
    SkColor::from_rgba8(c[0], c[1], c[2], a)
}

/// A top-to-bottom gradient between `y0` and `y1`.
fn down(y0: f32, y1: f32, stops: &[(f32, [u8; 3])]) -> Option<Shader<'static>> {
    let stops = stops.iter().map(|(at, c)| GradientStop::new(*at, sk(*c, 255))).collect();
    LinearGradient::new(
        Point::from_xy(0.0, y0),
        Point::from_xy(0.0, y1),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    )
}

/// Fill `path` with `shader`, falling back to flat `c` if it could not be
/// built (two identical stops, a zero-length axis).
fn fill_shaded(px: &mut Pixmap, path: &tiny_skia::Path, shader: Option<Shader<'_>>, c: [u8; 3]) {
    let mut p = paint(c, 255);
    if let Some(shader) = shader {
        p.shader = shader;
    }
    px.fill_path(path, &p, FillRule::Winding, Transform::identity(), None);
}

/// A soft pool of colour, fading out to nothing at radius `r`.
fn glow(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3], alpha: u8) {
    let shader = RadialGradient::new(
        Point::from_xy(cx, cy),
        Point::from_xy(cx, cy),
        r,
        vec![GradientStop::new(0.0, sk(c, alpha)), GradientStop::new(1.0, sk(c, 0))],
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(shader), Some(area)) = (shader, Rect::from_xywh(cx - r, cy - r, 2.0 * r, 2.0 * r)) {
        let mut p = Paint::default();
        p.shader = shader;
        p.anti_alias = true;
        px.fill_rect(area, &p, Transform::identity(), None);
    }
}

/// A flat translucent disc: the shadow's layers, and the sheen on the crest.
fn wash(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3], alpha: u8) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        px.fill_path(&path, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

/// The same hue pulled `t` of the way towards white.
fn lift(c: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 + (255.0 - c[i] as f32) * t) as u8)
}

/// The same hue at `t` of its brightness.
fn dim(c: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 * t) as u8)
}

/// `a` pulled `t` of the way towards `b`.
fn mix(a: [u8; 3], t: f32, b: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8)
}

/// Letters opened up, the way heavy uppercase wants them.
fn spaced(s: &str) -> String {
    s.chars().map(String::from).collect::<Vec<_>>().join("\u{2009}")
}

/// Prefer a heavy geometric sans that is actually installed.
fn pick_family(fs: &FontSystem) -> String {
    for name in ["Montserrat", "Avenir Next", "Noto Sans"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

/// One left-aligned line, and what keeps it legible on the house colour.
struct Label<'t> {
    text: &'t str,
    x: f32,
    baseline: f32,
    size: f32,
    weight: Weight,
    ink: [u8; 3],
    /// How far the dark outline is pushed out, in pixels.
    edge: f32,
}

struct Pen<'a> {
    px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    family: String,
    /// Every face `family` has, to snap a wanted weight onto a real one.
    faces: Vec<(FontStyle, Stretch, Weight)>,
}

impl<'a> Pen<'a> {
    fn new(w: f32, h: f32, fs: &'a mut FontSystem) -> Option<Pen<'a>> {
        let family = pick_family(fs);
        let faces = fs
            .db()
            .faces()
            .filter(|f| f.families.iter().any(|(n, _)| *n == family))
            .map(|f| (f.style, f.stretch, f.weight))
            .collect();
        Some(Pen { px: Pixmap::new(w as u32, h as u32)?, fs, cache: SwashCache::new(), family, faces })
    }

    /// The upright face of `family` nearest the weight asked for. The text
    /// engine only draws with a face it can match on style, width and weight:
    /// ask for a weight the family hasn't got and the words come out in the
    /// system sans instead, with no error anywhere.
    fn snap(&self, weight: Weight) -> (FontStyle, Stretch, Weight) {
        self.faces
            .iter()
            .filter(|f| f.0 == FontStyle::Normal)
            .min_by_key(|f| (f.2 .0 as i32 - weight.0 as i32).abs())
            .copied()
            .unwrap_or((FontStyle::Normal, Stretch::Normal, weight))
    }

    fn layout(&mut self, text: &str, size: f32, line_h: f32, weight: Weight, width: Option<f32>) -> Buffer {
        let (style, stretch, weight) = self.snap(weight);
        let family = self.family.clone();
        let fs = &mut *self.fs;
        let mut buf = Buffer::new(fs, Metrics::new(size, line_h));
        buf.set_wrap(fs, Wrap::WordOrGlyph);
        buf.set_size(fs, width, None);
        let attrs = Attrs::new().family(Family::Name(&family)).weight(weight).style(style).stretch(stretch);
        buf.set_text(fs, text, attrs, Shaping::Advanced);
        if width.is_some() {
            for line in buf.lines.iter_mut() {
                line.set_align(Some(Align::Center));
            }
        }
        buf.shape_until_scroll(fs, false);
        buf
    }

    fn height(buf: &Buffer) -> f32 {
        buf.layout_runs().last().map(|r| r.line_top + buf.metrics().line_height).unwrap_or(0.0)
    }

    fn draw(&mut self, buf: &Buffer, x: f32, top: f32, color: [u8; 3]) {
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        let (ox, oy) = (x.round() as i32, top.round() as i32);
        buf.draw(fs, cache, Color::rgb(color[0], color[1], color[2]), |gx, gy, w, h, c| {
            blend_rect(px, ox + gx, oy + gy, w, h, c);
        });
    }

    fn measure(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        let buf = self.layout(text, size, size * 1.3, weight, None);
        buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// One line centred on `cx`, with its baseline at `baseline`.
    fn centered(&mut self, text: &str, cx: f32, baseline: f32, size: f32, weight: Weight, color: [u8; 3]) {
        let buf = self.layout(text, size, size * 1.3, weight, None);
        let (line_y, w) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
        self.draw(&buf, cx - w / 2.0, baseline - line_y, color);
    }

    /// A left-aligned line with a dark outline behind it. The ground runs from
    /// deep scarlet to dimmed yellow across the four houses, and the outline is
    /// what lets one ink stay legible on all of them.
    fn label(&mut self, l: Label<'_>) {
        let buf = self.layout(l.text, l.size, l.size * 1.3, l.weight, None);
        let line_y = buf.layout_runs().next().map(|r| r.line_y).unwrap_or(l.size);
        let top = l.baseline - line_y;
        for (dx, dy) in OUTLINE {
            self.draw(&buf, l.x + dx * l.edge, top + dy * l.edge, DARK);
        }
        self.draw(&buf, l.x, top, l.ink);
    }

    /// `text` cut down with an ellipsis until it fits `max_w`.
    fn fit(&mut self, text: &str, size: f32, weight: Weight, max_w: f32) -> String {
        if self.measure(text, size, weight) <= max_w {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t = format!("{}…", chars.iter().collect::<String>().trim_end());
            if self.measure(&t, size, weight) <= max_w {
                return t;
            }
        }
        "…".to_string()
    }

    /// The largest size from `size` down to `floor` at which `text` fits
    /// `max_w`, and only at the floor is it cut short instead.
    fn shrink(&mut self, text: &str, size: f32, floor: f32, weight: Weight, max_w: f32) -> (String, f32) {
        let mut s = size;
        while s > floor {
            if self.measure(text, s, weight) <= max_w {
                return (text.to_string(), s);
            }
            s -= 2.0;
        }
        (self.fit(text, floor, weight, max_w), floor)
    }

    /// `text` wrapped and centred in `width`, cut down to `max_lines`.
    fn paragraph(&mut self, text: &str, size: f32, line_h: f32, width: f32, max_lines: usize) -> Buffer {
        // Nobody reads past this, and it bounds the work of the loop below.
        let mut body: String = text.chars().take(240).collect();
        loop {
            let buf = self.layout(&body, size, line_h, Weight::MEDIUM, Some(width));
            if buf.layout_runs().count() <= max_lines || body.chars().count() <= 1 {
                return buf;
            }
            let mut chars: Vec<char> = body.chars().collect();
            chars.pop();
            if chars.last() == Some(&'…') {
                chars.pop();
            }
            body = format!("{}…", chars.iter().collect::<String>().trim_end());
        }
    }

    /// The slab the hat's line sits on: translucent, so the glow behind it
    /// still shows, with a hairline along the top edge to lift it off.
    fn panel(&mut self) {
        let Some(shape) = rrect(PANEL_X, PANEL_Y, PANEL_W, PANEL_H, PANEL_R) else { return };
        self.px.fill_path(&shape, &paint([6, 6, 9], 150), FillRule::Winding, Transform::identity(), None);
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        self.px.stroke_path(&shape, &paint([255, 255, 255], 40), &stroke, Transform::identity(), None);
        if let Some(top) = rrect(PANEL_X + PANEL_R, PANEL_Y + 1.5, PANEL_W - 2.0 * PANEL_R, 1.5, 0.75) {
            self.px.fill_path(&top, &paint([255, 255, 255], 92), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// The picture itself, circle-cropped. Bytes that are missing or that no
    /// decoder recognises both fall through to the silhouette.
    fn picture(&mut self, cx: f32, cy: f32, d: f32, bytes: Option<&[u8]>) {
        let r = d / 2.0;
        match avatar_pixmap(bytes, d as u32, false) {
            Some(pm) => self.px.draw_pixmap(
                (cx - r).round() as i32,
                (cy - r).round() as i32,
                pm.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            ),
            None => silhouette(&mut self.px, cx, cy, r),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

    /// The four houses, as the caller will pass them.
    const HOUSES: [(&str, &str, [u8; 3], [u8; 3]); 4] = [
        ("Gryffindor", "🦁", [174, 26, 32], [238, 186, 48]),
        ("Slytherin", "🐍", [26, 71, 42], [170, 170, 170]),
        ("Ravenclaw", "🦅", [34, 47, 91], [148, 100, 52]),
        ("Hufflepuff", "🦡", [240, 199, 94], [0, 0, 0]),
    ];

    /// Stands in for a downloaded profile picture: a lit background with a
    /// couple of shapes on it, so the circle crop has something in it.
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

    /// One card per house, with the same member in each.
    fn card(house: usize, name: &str, avatar: Option<Vec<u8>>, line: &str) -> Sorted {
        let (house, crest, primary, secondary) = HOUSES[house];
        Sorted {
            name: name.to_string(),
            avatar,
            house: house.to_string(),
            crest: crest.to_string(),
            colours: (primary, secondary),
            line: line.to_string(),
        }
    }

    #[test]
    fn every_house_renders() {
        for i in 0..HOUSES.len() {
            let s = card(i, "Aarav", Some(fake_avatar([214, 96, 92])), "Brave to the point of trouble!");
            let png = sorting_png(&s).expect("sorting card");
            assert_eq!(&png[..4], &PNG_MAGIC, "{} card", HOUSES[i].0);
        }
    }

    /// The awkward cases in one go: no picture, a name far too long for the
    /// column, no line from the hat, a name that is all emoji, bytes no
    /// decoder will take, and empty strings throughout.
    #[test]
    fn cards_survive_the_awkward_cases() {
        let long = "Aaravpreet Singh Chaudhary the Third Jr.";
        assert_eq!(long.chars().count(), 40, "the long-name case must be the 40-character one");
        let cases = [
            card(0, "Koi nahi", None, "Bina photo bhi Gryffindor!"),
            card(1, long, Some(fake_avatar([90, 190, 160])), "Naam lamba, chaal tedhi"),
            card(2, "Meera", Some(fake_avatar([112, 104, 220])), ""),
            card(3, "🦡🐍🦁🦅✨🔥", Some(fake_avatar([240, 199, 94])), "Emoji hi emoji"),
            card(0, "Broken", Some(b"not an image at all".to_vec()), "Photo toot gayi"),
            card(1, "", Some(Vec::new()), ""),
        ];
        for s in cases {
            let png = sorting_png(&s).expect("sorting card");
            assert_eq!(&png[..4], &PNG_MAGIC, "{} / {}", s.house, s.name);
        }
    }

    /// A house name and a crest the caller never promised, so neither the
    /// shrink-to-fit nor the badge may panic on them.
    #[test]
    fn odd_houses_still_draw() {
        let mut s = card(2, "Ishaan", Some(fake_avatar([60, 80, 160])), "Kitaabein hi dost hain");
        s.house = String::new();
        s.crest = String::new();
        assert_eq!(&sorting_png(&s).expect("sorting card")[..4], &PNG_MAGIC);
        s.house = "Wampus House Of Very Long Names".to_string();
        s.crest = "🦁🦁".to_string();
        assert_eq!(&sorting_png(&s).expect("sorting card")[..4], &PNG_MAGIC);
    }

    /// Writes one card per house out to look at:
    /// `HOUSE_CARD_PREVIEW=/tmp cargo test house_card -- --ignored`.
    #[test]
    #[ignore = "writes files; only useful when looking at the design"]
    fn preview() {
        let Ok(dir) = std::env::var("HOUSE_CARD_PREVIEW") else { return };
        let lines = [
            "Brave to the point of trouble. GRYFFINDOR!",
            "Ambition, and the patience to use it. SLYTHERIN!",
            "A ready mind, and questions for everything. RAVENCLAW!",
            "Loyal, patient, and unafraid of the work. HUFFLEPUFF!",
        ];
        for (i, line) in lines.iter().enumerate() {
            let s = card(i, "Aarav Sharma 🔥", Some(fake_avatar([206, 120, 96])), line);
            let bytes = sorting_png(&s).expect("preview card");
            let name = format!("house_{}.png", HOUSES[i].0.to_lowercase());
            std::fs::write(std::path::Path::new(&dir).join(name), bytes).expect("writing the preview");
        }
    }
}
