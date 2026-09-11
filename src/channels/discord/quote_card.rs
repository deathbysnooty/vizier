//! Quote cards: one message drawn as an image.
//!
//! Every style is built around the writer's own avatar blown up into a
//! portrait - the "Make it a Quote" look, pushed further: heavier fades, a
//! vignette, film grain, and type that fills its space. The quote is wrapped
//! and sized down step by step until it fits, so a one-liner is big and a
//! paragraph still reads.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Stretch, Style as FontStyle, SwashCache, Weight,
    Wrap,
};
use image::imageops::FilterType;
use image::DynamicImage;
use tiny_skia::{
    Color as SkColor, ColorU8, FillRule, GradientStop, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Point,
    RadialGradient, Rect, SpreadMode, Stroke, Transform,
};

use super::awards_card::{avatar_pixmap, blend_rect, fill_circle, paint, rrect};

pub struct Quote {
    pub text: String,
    pub author: String,
    pub handle: String,
    pub avatar: Option<Vec<u8>>,
    pub when: String,
    pub channel: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    Noir,
    Rang,
    Spotlight,
    Duotone,
    Marble,
    Pothi,
    Scroll,
    Chitthi,
    Illuminated,
    Cosmos,
    Zen,
}

impl Style {
    pub const ALL: [Style; 11] = [
        Style::Noir,
        Style::Rang,
        Style::Spotlight,
        Style::Duotone,
        Style::Marble,
        Style::Pothi,
        Style::Scroll,
        Style::Chitthi,
        Style::Illuminated,
        Style::Cosmos,
        Style::Zen,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Style::Noir => "noir",
            Style::Rang => "rang",
            Style::Spotlight => "spotlight",
            Style::Duotone => "duotone",
            Style::Marble => "marble",
            Style::Pothi => "parchment",
            Style::Scroll => "scroll",
            Style::Chitthi => "chitthi",
            Style::Illuminated => "illuminated",
            Style::Cosmos => "cosmos",
            Style::Zen => "zen",
        }
    }

    pub fn from_key(key: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Style::Noir => "🖤 Noir",
            Style::Rang => "🎨 Rang",
            Style::Spotlight => "🔦 Spotlight",
            Style::Duotone => "💜 Duotone",
            Style::Marble => "🏛️ Marble",
            Style::Pothi => "📜 Parchment",
            Style::Scroll => "🪶 Scroll",
            Style::Chitthi => "✉️ Chitthi",
            Style::Illuminated => "✨ Illuminated",
            Style::Cosmos => "🌌 Cosmos",
            Style::Zen => "🍃 Zen",
        }
    }
}

const W: f32 = 1200.0;
const H: f32 = 630.0;

#[derive(Clone, Copy)]
enum Face {
    Serif,
    SerifItalic,
    Sans,
    Script,
}

struct Pen<'a> {
    px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    serif: String,
    sans: String,
    script: String,
    faces: std::collections::HashMap<String, Vec<(FontStyle, Stretch, Weight)>>,
}

impl Pen<'_> {
    fn layout(&mut self, text: &str, size: f32, face: Face, weight: Weight, width: Option<f32>, align: Option<Align>) -> Buffer {
        self.layout_with(text, size, (size * 1.25).round(), face, weight, width, align)
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_with(&mut self, text: &str, size: f32, line_height: f32, face: Face, weight: Weight, width: Option<f32>, align: Option<Align>) -> Buffer {
        let family = match face {
            Face::Sans => self.sans.clone(),
            Face::Script => self.script.clone(),
            _ => self.serif.clone(),
        };
        let want = if matches!(face, Face::SerifItalic | Face::Script) { FontStyle::Italic } else { FontStyle::Normal };
        let (style, stretch, weight) = self.snap(&family, want, weight);
        let fs = &mut *self.fs;
        let mut buf = Buffer::new(fs, Metrics::new(size, line_height));
        buf.set_wrap(fs, Wrap::WordOrGlyph);
        buf.set_size(fs, width, None);
        let attrs = Attrs::new().family(Family::Name(&family)).weight(weight).style(style).stretch(stretch);
        buf.set_text(fs, text, attrs, Shaping::Advanced);
        if align.is_some() {
            for line in buf.lines.iter_mut() {
                line.set_align(align);
            }
        }
        buf.shape_until_scroll(fs, false);
        buf
    }

    /// The face of `family` nearest to what was asked for. The text engine only
    /// uses a face it can match on style, width and weight - ask Z003 (which
    /// exists only as a weight-500 italic) for weight 400 and the words come
    /// out in the system sans instead, with no error anywhere.
    fn snap(&mut self, family: &str, style: FontStyle, weight: Weight) -> (FontStyle, Stretch, Weight) {
        let fs = &*self.fs;
        let faces = self.faces.entry(family.to_string()).or_insert_with(|| {
            fs.db()
                .faces()
                .filter(|f| f.families.iter().any(|(n, _)| n == family))
                .map(|f| (f.style, f.stretch, f.weight))
                .collect()
        });
        let same_style: Vec<_> = faces.iter().filter(|f| f.0 == style).collect();
        let pool = if same_style.is_empty() { faces.iter().collect() } else { same_style };
        pool.into_iter()
            .min_by_key(|f| (f.2 .0 as i32 - weight.0 as i32).abs())
            .copied()
            .unwrap_or((style, Stretch::Normal, weight))
    }

    fn width(buf: &Buffer) -> f32 {
        buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
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

    /// Like `draw`, but the ink density wanders the way a quill's does.
    fn draw_ink(&mut self, buf: &Buffer, x: f32, top: f32, color: [u8; 3]) {
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        let (ox, oy) = (x.round() as i32, top.round() as i32);
        buf.draw(fs, cache, Color::rgb(color[0], color[1], color[2]), |gx, gy, w, h, c| {
            let (ax, ay) = (ox + gx, oy + gy);
            let density = 0.7 + 0.3 * noise(ax as f32 * 0.35, ay as f32 * 0.35, 5);
            blend_rect(px, ax, ay, w, h, Color::rgba(c.r(), c.g(), c.b(), (c.a() as f32 * density) as u8));
        });
    }

    fn measure(&mut self, text: &str, size: f32, face: Face, weight: Weight) -> f32 {
        let buf = self.layout(text, size, face, weight, None, None);
        Self::width(&buf)
    }

    /// One line with its left edge at `x` and baseline at `baseline`.
    fn line(&mut self, text: &str, x: f32, baseline: f32, size: f32, face: Face, weight: Weight, color: [u8; 3]) -> f32 {
        let buf = self.layout(text, size, face, weight, None, None);
        let (line_y, w) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
        self.draw(&buf, x, baseline - line_y, color);
        w
    }

    fn centered(&mut self, text: &str, cx: f32, baseline: f32, size: f32, face: Face, weight: Weight, color: [u8; 3]) {
        let w = self.measure(text, size, face, weight);
        self.line(text, cx - w / 2.0, baseline, size, face, weight, color);
    }

    fn fit(&mut self, text: &str, size: f32, face: Face, weight: Weight, max_w: f32) -> String {
        if self.measure(text, size, face, weight) <= max_w {
            return text.to_string();
        }
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            let t = format!("{}…", chars.iter().collect::<String>().trim_end());
            if self.measure(&t, size, face, weight) <= max_w {
                return t;
            }
        }
        "…".to_string()
    }

    /// The quote itself: wrapped into `width`, stepping the size down from
    /// `big` until it fits `max_h` or reaches `small`.
    #[allow(clippy::too_many_arguments)]
    fn paragraph(&mut self, text: &str, face: Face, weight: Weight, width: f32, max_h: f32, big: f32, small: f32, align: Align) -> Buffer {
        let mut size = big;
        loop {
            let buf = self.layout(text, size, face, weight, Some(width), Some(align));
            if Self::height(&buf) <= max_h || size <= small {
                return buf;
            }
            size -= 2.0;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn avatar(&mut self, bytes: Option<&[u8]>, cx: f32, cy: f32, size: f32, ring: [u8; 3], gap: [u8; 3], name: &str) {
        let r = size / 2.0;
        fill_circle(&mut self.px, cx, cy, r + 6.0, ring);
        fill_circle(&mut self.px, cx, cy, r + 3.0, gap);
        match avatar_pixmap(bytes, size as u32, false) {
            Some(pm) => self.px.draw_pixmap(
                (cx - r).round() as i32,
                (cy - r).round() as i32,
                pm.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            ),
            None => {
                fill_circle(&mut self.px, cx, cy, r, [64, 66, 76]);
                let fsz = size * 0.44;
                self.centered(&initial(name), cx, cy + fsz * 0.36, fsz, Face::Sans, Weight::BOLD, [240, 240, 240]);
            }
        }
    }
}

fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn sk(c: [u8; 3], a: u8) -> SkColor {
    SkColor::from_rgba8(c[0], c[1], c[2], a)
}

fn rect(px: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 3]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        px.fill_rect(r, &paint(c, 255), Transform::identity(), None);
    }
}

fn scale(c: [u8; 3], f: f32) -> [u8; 3] {
    [(c[0] as f32 * f) as u8, (c[1] as f32 * f) as u8, (c[2] as f32 * f) as u8]
}

/// The same hue pushed up to a vivid, readable light.
fn brighten(c: [u8; 3]) -> [u8; 3] {
    let k = 235.0 / c.iter().copied().max().unwrap_or(1).max(1) as f32;
    [(c[0] as f32 * k).min(255.0) as u8, (c[1] as f32 * k).min(255.0) as u8, (c[2] as f32 * k).min(255.0) as u8]
}

enum Tone {
    Grey,
    Colour,
    Duo([u8; 3], [u8; 3]),
}

/// Contrast pushed into an S-curve, for a moodier picture.
fn punch(l: f32) -> f32 {
    let x = ((l - 0.5) * 1.15 + 0.5).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn toned(img: &image::RgbaImage, tone: &Tone) -> Option<Pixmap> {
    let mut pm = Pixmap::new(img.width(), img.height())?;
    for (dst, p) in pm.pixels_mut().iter_mut().zip(img.pixels()) {
        let l = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0;
        let rgb = match tone {
            Tone::Colour => [p[0], p[1], p[2]],
            Tone::Grey => {
                let v = (punch(l) * 255.0) as u8;
                [v, v, v]
            }
            Tone::Duo(dark, light) => {
                let t = punch(l);
                [lerp(dark[0], light[0], t), lerp(dark[1], light[1], t), lerp(dark[2], light[2], t)]
            }
        };
        *dst = tiny_skia::ColorU8::from_rgba(rgb[0], rgb[1], rgb[2], 255).premultiply();
    }
    Some(pm)
}

fn decode(bytes: Option<&[u8]>) -> Option<DynamicImage> {
    image::load_from_memory(bytes?).ok()
}

/// The avatar blown up to cover `w` x `h`.
fn portrait(img: &DynamicImage, w: u32, h: u32, tone: &Tone) -> Option<Pixmap> {
    toned(&img.resize_to_fill(w, h, FilterType::Lanczos3).to_rgba8(), tone)
}

/// The same, soft as if shot out of focus: shrink hard, then stretch back.
fn blurred(img: &DynamicImage, w: u32, h: u32, tone: &Tone) -> Option<Pixmap> {
    let small = img.resize_to_fill((w / 24).max(1), (h / 24).max(1), FilterType::Triangle);
    toned(&small.resize_exact(w, h, FilterType::CatmullRom).to_rgba8(), tone)
}

fn average(img: &DynamicImage) -> [u8; 3] {
    let t = img.resize_exact(12, 12, FilterType::Triangle).to_rgb8();
    let mut sum = [0u32; 3];
    for p in t.pixels() {
        for k in 0..3 {
            sum[k] += p[k] as u32;
        }
    }
    let n = (t.width() * t.height()).max(1);
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

/// Portrait dissolving into `c`: untouched left of `x0`, solid `c` from `x1`.
/// Blended column by column on an eased curve, so the face holds and then
/// drops away instead of greying out evenly.
fn fade(px: &mut Pixmap, x0: f32, x1: f32, c: [u8; 3]) {
    fade_with(px, x0, x1, |_| c);
}

/// The same dissolve, into a texture instead of a flat colour.
fn fade_into(px: &mut Pixmap, x0: f32, x1: f32, under: &Pixmap) {
    let data = under.data();
    fade_with(px, x0, x1, |i| [data[i * 4], data[i * 4 + 1], data[i * 4 + 2]]);
}

fn fade_with(px: &mut Pixmap, x0: f32, x1: f32, target: impl Fn(usize) -> [u8; 3]) {
    let w = px.width() as usize;
    let ramp: Vec<f32> = (0..w)
        .map(|x| {
            let t = ((x as f32 - x0) / (x1 - x0)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        })
        .collect();
    for (i, chunk) in px.data_mut().chunks_exact_mut(4).enumerate() {
        let t = ramp[i % w];
        if t > 0.0 {
            let c = target(i);
            for k in 0..3 {
                chunk[k] = (chunk[k] as f32 * (1.0 - t) + c[k] as f32 * t).round() as u8;
            }
        }
    }
}

fn shade(px: &mut Pixmap, c: [u8; 3], alpha: u8) {
    if let Some(r) = Rect::from_xywh(0.0, 0.0, W, H) {
        px.fill_rect(r, &paint(c, alpha), Transform::identity(), None);
    }
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

/// Edges pulled into darkness.
fn vignette(px: &mut Pixmap, strength: u8) {
    vignette_tint(px, strength, [0, 0, 0]);
}

/// Edges pulled towards `c` - burnt paper, stained stone.
fn vignette_tint(px: &mut Pixmap, strength: u8, c: [u8; 3]) {
    let shader = RadialGradient::new(
        Point::from_xy(W / 2.0, H / 2.0),
        Point::from_xy(W / 2.0, H / 2.0),
        W * 0.72,
        vec![GradientStop::new(0.45, sk(c, 0)), GradientStop::new(1.0, sk(c, strength))],
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(shader), Some(r)) = (shader, Rect::from_xywh(0.0, 0.0, W, H)) {
        let mut p = Paint::default();
        p.shader = shader;
        px.fill_rect(r, &p, Transform::identity(), None);
    }
}

/// Film grain. Seeded from the quote so the same card always comes out the same.
fn grain(px: &mut Pixmap, amount: i32, seed: u32) {
    let mut s = seed | 1;
    let span = (2 * amount + 1) as u32;
    for chunk in px.data_mut().chunks_exact_mut(4) {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let n = (s % span) as i32 - amount;
        for k in 0..3 {
            chunk[k] = (chunk[k] as i32 + n).clamp(0, 255) as u8;
        }
    }
}

fn seed(q: &Quote) -> u32 {
    q.text.bytes().fold(2_166_136_261u32, |h, b| (h ^ b as u32).wrapping_mul(16_777_619))
}

fn draw_portrait(p: &mut Pen<'_>, q: &Quote, tone: &Tone, fallback: [u8; 3]) {
    match decode(q.avatar.as_deref()).and_then(|img| portrait(&img, 660, H as u32, tone)) {
        Some(pm) => p.px.draw_pixmap(-30, 0, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None),
        None => p.centered(&initial(&q.author), 300.0, H / 2.0 + 130.0, 360.0, Face::Serif, Weight::BOLD, fallback),
    }
}

/// Quote, rule, author and handle as one block centred between `x0` and `x1`.
#[allow(clippy::too_many_arguments)]
fn text_block(p: &mut Pen<'_>, q: &Quote, x0: f32, x1: f32, face: Face, caps: bool, ink: [u8; 3], author: [u8; 3], muted: [u8; 3], rule: [u8; 3], mark: [u8; 3]) {
    let width = x1 - x0;
    let cx = (x0 + x1) / 2.0;
    let big = if matches!(face, Face::Sans) { 60.0 } else { 54.0 };
    let text = p.paragraph(&q.text, face, Weight::NORMAL, width, 330.0, big, 24.0, Align::Center);
    let th = Pen::height(&text);
    // mark, quote, rule and byline, centred as one block
    let top0 = ((H - (70.0 + th + 136.0)) / 2.0 - 8.0).max(24.0);
    p.centered("“", cx, top0 + 92.0, 110.0, Face::Serif, Weight::BOLD, mark);
    let top = top0 + 70.0;
    p.draw(&text, x0, top, ink);
    let ry = top + th + 26.0;
    rect(&mut p.px, cx - 34.0, ry, 68.0, 2.0, rule);
    if caps {
        // an inscription: capitals, opened up
        let by = p.fit(&spaced(&q.author.to_uppercase()), 22.0, Face::Serif, Weight::SEMIBOLD, width);
        p.centered(&by, cx, ry + 46.0, 22.0, Face::Serif, Weight::SEMIBOLD, author);
    } else {
        let by = p.fit(&format!("— {}", q.author), 30.0, Face::SerifItalic, Weight::NORMAL, width);
        p.centered(&by, cx, ry + 50.0, 30.0, Face::SerifItalic, Weight::NORMAL, author);
    }
    let handle = p.fit(&format!("@{}", q.handle), 18.0, Face::Sans, Weight::MEDIUM, width);
    p.centered(&handle, cx, ry + 80.0, 18.0, Face::Sans, Weight::MEDIUM, muted);
}

/// Where and when, bottom right, small.
fn footer(p: &mut Pen<'_>, q: &Quote, color: [u8; 3]) {
    let mut parts: Vec<String> = Vec::new();
    if !q.channel.is_empty() {
        parts.push(q.channel.clone());
    }
    if !q.when.is_empty() {
        parts.push(q.when.clone());
    }
    parts.push("MLCI · GYAAN".to_string());
    let s = parts.join("  ·  ");
    let w = p.measure(&s, 15.0, Face::Sans, Weight::MEDIUM);
    p.line(&s, W - 28.0 - w, H - 24.0, 15.0, Face::Sans, Weight::MEDIUM, color);
}

fn noir(p: &mut Pen<'_>, q: &Quote) {
    p.px.fill(sk([0, 0, 0], 255));
    draw_portrait(p, q, &Tone::Grey, [60, 60, 60]);
    fade(&mut p.px, 250.0, 640.0, [0, 0, 0]);
    vignette(&mut p.px, 210);
    text_block(p, q, 640.0, 1165.0, Face::Sans, false, [245, 245, 245], [205, 205, 205], [130, 130, 130], [90, 90, 90], [95, 95, 95]);
    footer(p, q, [105, 105, 105]);
    grain(&mut p.px, 9, seed(q));
}

fn rang(p: &mut Pen<'_>, q: &Quote) {
    let avg = decode(q.avatar.as_deref()).map(|img| average(&img)).unwrap_or([90, 80, 120]);
    let (bg, accent) = (scale(avg, 0.16), brighten(avg));
    p.px.fill(sk(bg, 255));
    draw_portrait(p, q, &Tone::Colour, scale(avg, 0.5));
    fade(&mut p.px, 240.0, 640.0, bg);
    vignette(&mut p.px, 150);
    text_block(p, q, 640.0, 1165.0, Face::Sans, false, [250, 250, 250], accent, [175, 175, 180], accent, scale(accent, 0.55));
    footer(p, q, [150, 150, 155]);
    grain(&mut p.px, 5, seed(q));
}

fn spotlight(p: &mut Pen<'_>, q: &Quote) {
    let gold = [230, 190, 110];
    p.px.fill(sk([6, 6, 8], 255));
    if let Some(bg) = decode(q.avatar.as_deref()).and_then(|img| blurred(&img, W as u32, H as u32, &Tone::Grey)) {
        p.px.draw_pixmap(0, 0, bg.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    shade(&mut p.px, [0, 0, 0], 175);
    glow(&mut p.px, W / 2.0, 250.0, 520.0, [255, 244, 220], 46);
    vignette(&mut p.px, 235);
    p.avatar(q.avatar.as_deref(), W / 2.0, 108.0, 104.0, gold, [10, 10, 12], &q.author);
    let text = p.paragraph(&format!("“{}”", q.text), Face::SerifItalic, Weight::NORMAL, 940.0, 280.0, 56.0, 26.0, Align::Center);
    let th = Pen::height(&text);
    let top = 190.0 + (280.0 - th).max(0.0) / 2.0;
    p.draw(&text, W / 2.0 - 470.0, top, [250, 248, 242]);
    let ry = (top + th + 24.0).min(H - 124.0);
    rect(&mut p.px, W / 2.0 - 40.0, ry, 80.0, 2.0, gold);
    let name = p.fit(&q.author.to_uppercase(), 24.0, Face::Sans, Weight::SEMIBOLD, 900.0);
    p.centered(&name, W / 2.0, ry + 42.0, 24.0, Face::Sans, Weight::SEMIBOLD, gold);
    let handle = p.fit(&format!("@{}", q.handle), 17.0, Face::Sans, Weight::MEDIUM, 900.0);
    p.centered(&handle, W / 2.0, ry + 70.0, 17.0, Face::Sans, Weight::MEDIUM, [160, 156, 150]);
    footer(p, q, [120, 118, 112]);
    grain(&mut p.px, 8, seed(q));
}

fn duotone(p: &mut Pen<'_>, q: &Quote) {
    let (shadow, light, bg) = ([24, 8, 52], [255, 138, 96], [13, 5, 28]);
    p.px.fill(sk(bg, 255));
    draw_portrait(p, q, &Tone::Duo(shadow, light), [90, 40, 110]);
    fade(&mut p.px, 250.0, 640.0, bg);
    glow(&mut p.px, 1040.0, 90.0, 380.0, [255, 60, 150], 60);
    vignette(&mut p.px, 160);
    text_block(p, q, 640.0, 1165.0, Face::Sans, false, [255, 255, 255], [255, 120, 170], [190, 160, 200], light, [170, 70, 120]);
    footer(p, q, [150, 120, 160]);
    grain(&mut p.px, 7, seed(q));
}

fn spaced(s: &str) -> String {
    s.chars().map(String::from).collect::<Vec<_>>().join("\u{2009}")
}

fn clamp8(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}

fn hash2(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    h as f32 / u32::MAX as f32
}

/// Smooth value noise in 0..1.
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (xf, yf) = (x - xi as f32, y - yi as f32);
    let (u, v) = (xf * xf * (3.0 - 2.0 * xf), yf * yf * (3.0 - 2.0 * yf));
    let a = hash2(xi, yi, seed);
    let b = hash2(xi + 1, yi, seed);
    let c = hash2(xi, yi + 1, seed);
    let d = hash2(xi + 1, yi + 1, seed);
    a + (b - a) * u + (c - a) * v + (a - b - c + d) * u * v
}

/// Layered noise: broad shapes with finer detail on top.
fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for i in 0..5 {
        sum += amp * noise(x * freq, y * freq, seed.wrapping_add(i));
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / 0.96875
}

/// A full-card background painted pixel by pixel.
fn texture(colour_at: impl Fn(f32, f32) -> [u8; 3]) -> Option<Pixmap> {
    let w = W as u32;
    let mut pm = Pixmap::new(w, H as u32)?;
    for (i, dst) in pm.pixels_mut().iter_mut().enumerate() {
        let c = colour_at((i as u32 % w) as f32, (i as u32 / w) as f32);
        *dst = ColorU8::from_rgba(c[0], c[1], c[2], 255).premultiply();
    }
    Some(pm)
}

fn marble_at(x: f32, y: f32) -> [u8; 3] {
    let warp = fbm(x * 0.0035, y * 0.0035, 7);
    let vein = (1.0 - ((x * 0.0021 + y * 0.0013) * std::f32::consts::PI * 2.0 + warp * 6.0).sin().abs()).powf(7.0);
    let twist = fbm(x * 0.008, y * 0.008, 11);
    let fine = (1.0 - ((x * 0.006 - y * 0.003) * 6.28 + twist * 4.0).sin().abs()).powf(24.0);
    // the stone goes quiet behind the words
    let s = ((x - 520.0) / 160.0).clamp(0.0, 1.0);
    let calm = 1.0 - 0.65 * s * s * (3.0 - 2.0 * s);
    let g = 238.0 - fbm(x * 0.002, y * 0.002, 3) * 22.0 - (vein * 34.0 + fine * 12.0) * calm;
    [clamp8(g), clamp8(g - 2.0), clamp8(g - 6.0)]
}

/// Dark wood, for anything lying on a desk.
fn desk_colour(x: f32, y: f32) -> [f32; 3] {
    let wood = fbm(x * 0.004, y * 0.05, 79) * 18.0;
    [26.0 + wood, 17.0 + wood * 0.6, 11.0 + wood * 0.3]
}

/// Old parchment: mottled, spotted, fibrous.
fn sheet_colour(x: f32, y: f32) -> [f32; 3] {
    let mottle = (fbm(x * 0.004, y * 0.004, 91) - 0.4) * 0.8;
    let spots = (fbm(x * 0.012, y * 0.012, 93) - 0.56).max(0.0) * 2.6;
    let fibre = fbm(x * 0.08, y * 0.015, 95) * 0.12;
    let d = mottle + spots + fibre;
    [232.0 - d * 70.0, 212.0 - d * 80.0, 166.0 - d * 90.0]
}

/// An aged sheet with ragged, scorched edges, lying on a dark desk.
fn parchment_at(x: f32, y: f32) -> [u8; 3] {
    let edge = x.min(W - x).min(y).min(H - y) - 24.0 + (fbm(x * 0.018, y * 0.018, 77) - 0.5) * 34.0;
    if edge < 0.0 {
        let d = desk_colour(x, y);
        return [clamp8(d[0]), clamp8(d[1]), clamp8(d[2])];
    }
    let sheet = sheet_colour(x, y);
    // scorched towards the edge, nearly black right at it
    let burn = (edge / 42.0).clamp(0.0, 1.0).powf(0.55);
    let scorch = [58.0, 32.0, 14.0];
    [
        clamp8(scorch[0] + (sheet[0] - scorch[0]) * burn),
        clamp8(scorch[1] + (sheet[1] - scorch[1]) * burn),
        clamp8(scorch[2] + (sheet[2] - scorch[2]) * burn),
    ]
}

fn space_at(x: f32, y: f32) -> [u8; 3] {
    let t = (x / W) * 0.6 + (y / H) * 0.4;
    let dust = (fbm(x * 0.0028, y * 0.0028, 41) - 0.45).max(0.0) * 2.4;
    let gas = (fbm(x * 0.004 + 9.0, y * 0.004, 43) - 0.5).max(0.0) * 2.0;
    [
        clamp8(4.0 + 12.0 * t + dust * 70.0 + gas * 10.0),
        clamp8(6.0 + 4.0 * t + dust * 30.0 + gas * 50.0),
        clamp8(18.0 + 22.0 * t + dust * 110.0 + gas * 90.0),
    ]
}

fn rice_paper_at(x: f32, y: f32) -> [u8; 3] {
    let f = fbm(x * 0.03, y * 0.03, 61) * 10.0 + fbm(x * 0.004, y * 0.004, 63) * 8.0;
    [clamp8(246.0 - f), clamp8(242.0 - f), clamp8(234.0 - f * 1.1)]
}

fn dot(px: &mut Pixmap, x: f32, y: f32, r: f32, c: [u8; 3], a: u8) {
    if let Some(path) = PathBuilder::from_circle(x, y, r) {
        px.fill_path(&path, &paint(c, a), FillRule::Winding, Transform::identity(), None);
    }
}

/// A circular crop of the avatar in `tone`.
fn medallion(bytes: Option<&[u8]>, size: u32, tone: &Tone) -> Option<Pixmap> {
    let mut pm = portrait(&decode(bytes)?, size, size, tone)?;
    let mut mask = Mask::new(size, size)?;
    let s = size as f32;
    mask.fill_path(&PathBuilder::from_circle(s / 2.0, s / 2.0, s / 2.0)?, FillRule::Winding, true, Transform::identity());
    pm.apply_mask(&mask);
    Some(pm)
}

fn stars(px: &mut Pixmap, seed: u32, count: usize, clear: (f32, f32, f32, f32)) {
    let mut s = seed | 1;
    for _ in 0..count {
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s as f32 / u32::MAX as f32
        };
        let (x, y, b) = (next() * W, next() * H, next().powf(3.0));
        if x > clear.0 && x < clear.2 && y > clear.1 && y < clear.3 && b < 0.9 {
            continue;
        }
        if b > 0.8 {
            glow(px, x, y, 9.0 + b * 9.0, [200, 215, 255], 80);
        }
        dot(px, x, y, 0.5 + b * 1.5, [235, 240, 255], (60.0 + b * 195.0) as u8);
    }
}

/// The avatar as a planet: lit from the upper left, night side clipped to the disc.
fn planet(p: &mut Pen<'_>, q: &Quote, cx: f32, cy: f32, r: f32) {
    glow(&mut p.px, cx, cy, r * 1.6, [70, 130, 255], 80);
    match medallion(q.avatar.as_deref(), (2.0 * r) as u32, &Tone::Colour) {
        Some(pm) => p.px.draw_pixmap(
            (cx - r).round() as i32,
            (cy - r).round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        None => {
            dot(&mut p.px, cx, cy, r, [40, 52, 90], 255);
            p.centered(&initial(&q.author), cx, cy + r * 0.3, r * 0.8, Face::Sans, Weight::LIGHT, [200, 215, 255]);
        }
    }
    night_side(&mut p.px, cx, cy, r);
    if let Some(disc) = PathBuilder::from_circle(cx, cy, r) {
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        p.px.stroke_path(&disc, &paint([150, 190, 255], 110), &stroke, Transform::identity(), None);
    }
}

/// Darkens the half of the disc facing away from a light at its upper left.
fn night_side(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    let (w, h) = (px.width() as i32, px.height() as i32);
    let (lx, ly) = (cx - r * 0.45, cy - r * 0.5);
    let space = [3.0, 5.0, 18.0];
    let data = px.data_mut();
    for y in ((cy - r) as i32).max(0)..=((cy + r) as i32).min(h - 1) {
        for x in ((cx - r) as i32).max(0)..=((cx + r) as i32).min(w - 1) {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let d = ((x as f32 - lx).powi(2) + (y as f32 - ly).powi(2)).sqrt() / (r * 1.9);
            let t = ((d - 0.25) / 0.55).clamp(0.0, 1.0);
            let dark = t * t * (3.0 - 2.0 * t) * 0.9;
            let i = ((y * w + x) * 4) as usize;
            for k in 0..3 {
                data[i + k] = (data[i + k] as f32 * (1.0 - dark) + space[k] * dark) as u8;
            }
        }
    }
}

/// A single-stroke brush circle, thick in the middle, breaking up at the tail.
fn enso(px: &mut Pixmap, cx: f32, cy: f32, r: f32, seed: u32) {
    let n = 1100;
    for i in 0..n {
        let t = i as f32 / n as f32;
        if t > 0.72 && hash2(i as i32, 7, seed) < (t - 0.72) * 2.4 {
            continue;
        }
        let th = -1.25 + 5.7 * t;
        let rr = r + (noise(t * 5.0, 1.5, seed) - 0.5) * 12.0;
        let taper = if t > 0.8 { (1.0 - (t - 0.8) * 3.0).max(0.25) } else { 1.0 };
        let width = 30.0 * (0.4 + 0.6 * (std::f32::consts::PI * t).sin()) * taper;
        dot(px, cx + rr * th.cos(), cy + rr * th.sin(), width / 2.0, [26, 24, 22], 55);
    }
}

/// A red name seal, the kind stamped beside a signature.
fn seal(p: &mut Pen<'_>, x: f32, y: f32, size: f32, name: &str) {
    let (red, cream) = ([178, 38, 30], [246, 238, 226]);
    if let Some(sq) = rrect(x, y, size, size, 6.0) {
        p.px.fill_path(&sq, &paint(red, 235), FillRule::Winding, Transform::identity(), None);
    }
    if let Some(inner) = rrect(x + 5.0, y + 5.0, size - 10.0, size - 10.0, 4.0) {
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        p.px.stroke_path(&inner, &paint(cream, 220), &stroke, Transform::identity(), None);
    }
    p.centered(&initial(name), x + size / 2.0, y + size * 0.7, size * 0.52, Face::Serif, Weight::BOLD, cream);
}

fn marble(p: &mut Pen<'_>, q: &Quote) {
    let (gold, ink) = ([168, 132, 72], [44, 40, 38]);
    let Some(stone) = texture(marble_at) else { return };
    p.px.draw_pixmap(0, 0, stone.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    let bust = Tone::Duo([74, 70, 66], [240, 236, 230]);
    if let Some(pm) = decode(q.avatar.as_deref()).and_then(|img| portrait(&img, 660, H as u32, &bust)) {
        p.px.draw_pixmap(-30, 0, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    fade_into(&mut p.px, 250.0, 640.0, &stone);
    vignette_tint(&mut p.px, 110, [70, 62, 54]);
    if let Some(frame) = rrect(20.0, 20.0, W - 40.0, H - 40.0, 2.0) {
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        p.px.stroke_path(&frame, &paint(gold, 200), &stroke, Transform::identity(), None);
    }
    text_block(p, q, 640.0, 1150.0, Face::SerifItalic, true, ink, [138, 104, 52], [120, 114, 106], gold, gold);
    footer(p, q, [128, 120, 110]);
    grain(&mut p.px, 4, seed(q));
}

fn ordinal_suffix(n: u32) -> &'static str {
    match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    }
}

fn roman(mut n: u32) -> String {
    let mut out = String::new();
    for (value, numeral) in [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"),
        (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ] {
        while n >= value {
            out.push_str(numeral);
            n -= value;
        }
    }
    out
}

/// "11 Sep 2026" as a letter-writer of old would put it.
fn olden_date(q: &Quote) -> String {
    let month = |m: &str| -> String {
        match m {
            "Jan" => "January", "Feb" => "February", "Mar" => "March", "Apr" => "April",
            "May" => "May", "Jun" => "June", "Jul" => "July", "Aug" => "August",
            "Sep" => "September", "Oct" => "October", "Nov" => "November", "Dec" => "December",
            other => other,
        }
        .to_string()
    };
    let parts: Vec<&str> = q.when.split_whitespace().collect();
    let written = match parts.as_slice() {
        [d, m, y] => match (d.parse::<u32>(), y.parse::<u32>()) {
            (Ok(d), Ok(y)) => format!("Written this {}{} day of {}, in the year {}", d, ordinal_suffix(d), month(m), roman(y)),
            _ => q.when.clone(),
        },
        _ => q.when.clone(),
    };
    if q.channel.is_empty() {
        written
    } else {
        format!("{}, in {}", written, q.channel)
    }
}

/// A pen swash under the signature.
fn flourish(px: &mut Pixmap, x0: f32, x1: f32, y: f32, ink: [u8; 3]) {
    let n = ((x1 - x0) * 1.5).max(10.0) as i32;
    for i in 0..n {
        let t = i as f32 / n as f32;
        let x = x0 - 10.0 + (x1 - x0 + 30.0) * t;
        let yy = y + (t * std::f32::consts::PI * 1.5).sin() * 5.0 - t * 4.0;
        let width = 0.6 + 2.4 * (t * std::f32::consts::PI).sin();
        dot(px, x, yy, width / 2.0, ink, 160);
    }
}

/// Red sealing wax, pressed with the writer's face.
fn wax_seal(p: &mut Pen<'_>, q: &Quote, cx: f32, cy: f32, r: f32, seed: u32) {
    let (w, h) = (p.px.width() as i32, p.px.height() as i32);
    let reach = r * 1.25;
    {
        let data = p.px.data_mut();
        for y in ((cy - reach) as i32).max(0)..=((cy + reach) as i32).min(h - 1) {
            for x in ((cx - reach) as i32).max(0)..=((cx + reach) as i32).min(w - 1) {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let dist = (dx * dx + dy * dy).sqrt();
                let edge = r * (1.0 + (noise(dy.atan2(dx) * 1.6 + 10.0, 0.5, seed) - 0.5) * 0.18);
                let i = ((y * w + x) * 4) as usize;
                if dist > edge {
                    // a soft shadow just outside the wax
                    if dist < edge + 12.0 {
                        let a = (1.0 - (dist - edge) / 12.0) * 0.4;
                        for (k, s) in [30.0, 16.0, 8.0].into_iter().enumerate() {
                            data[i + k] = (data[i + k] as f32 * (1.0 - a) + s * a) as u8;
                        }
                    }
                    continue;
                }
                let rim = ((edge - dist) / 10.0).clamp(0.0, 1.0);
                let light = ((-dx - dy) / (1.4 * r)).clamp(-1.0, 1.0);
                let shade = 0.62 + 0.28 * light + 0.18 * rim;
                for (k, base) in [158.0, 26.0, 20.0].into_iter().enumerate() {
                    data[i + k] = clamp8(base * shade);
                }
            }
        }
    }
    let inner = (r * 1.26) as u32;
    if let Some(pm) = medallion(q.avatar.as_deref(), inner, &Tone::Duo([62, 8, 6], [236, 128, 96])) {
        let pressed = PixmapPaint { opacity: 0.92, ..PixmapPaint::default() };
        let half = inner as f32 / 2.0;
        p.px.draw_pixmap((cx - half).round() as i32, (cy - half).round() as i32, pm.as_ref(), &pressed, Transform::identity(), None);
    }
    if let Some(ring) = PathBuilder::from_circle(cx, cy, r * 0.66) {
        let stroke = Stroke { width: 4.0, ..Stroke::default() };
        p.px.stroke_path(&ring, &paint([92, 12, 10], 220), &stroke, Transform::identity(), None);
    }
    if let Some(ring) = PathBuilder::from_circle(cx - 1.5, cy - 1.5, r * 0.66 + 3.0) {
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        p.px.stroke_path(&ring, &paint([220, 110, 90], 120), &stroke, Transform::identity(), None);
    }
}

/// A letter in quill ink on old parchment, signed and sealed in wax.
fn pothi(p: &mut Pen<'_>, q: &Quote) {
    let (ink, sepia) = ([44, 26, 14], [112, 72, 38]);
    let sd = seed(q);
    if let Some(sheet) = texture(parchment_at) {
        p.px.draw_pixmap(0, 0, sheet.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    let (x0, x1) = (96.0, 820.0);
    let text = p.paragraph(&q.text, Face::Script, Weight::NORMAL, x1 - x0, 350.0, 62.0, 30.0, Align::Left);
    let th = Pen::height(&text);
    let top = ((H - (th + 130.0)) / 2.0 - 16.0).max(54.0);
    p.draw_ink(&text, x0, top, ink);
    let sig = p.fit(&format!("— {}", q.author), 46.0, Face::Script, Weight::NORMAL, 520.0);
    let sig_buf = p.layout(&sig, 46.0, Face::Script, Weight::NORMAL, None, None);
    let (line_y, sw) = sig_buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((46.0, 0.0));
    let sx = (x1 - sw).max(x0 + 140.0);
    let base = top + th + 64.0;
    p.draw_ink(&sig_buf, sx, base - line_y, ink);
    flourish(&mut p.px, sx, sx + sw, base + 14.0, ink);
    wax_seal(p, q, 1010.0, 290.0, 106.0, sd);
    let handle = p.fit(&format!("@{}", q.handle), 17.0, Face::SerifItalic, Weight::NORMAL, 260.0);
    p.centered(&handle, 1010.0, 446.0, 17.0, Face::SerifItalic, Weight::NORMAL, sepia);
    let dateline = p.fit(&olden_date(q), 17.0, Face::SerifItalic, Weight::NORMAL, 780.0);
    p.line(&dateline, x0, H - 46.0, 17.0, Face::SerifItalic, Weight::NORMAL, sepia);
    grain(&mut p.px, 5, sd);
}

fn rect_a(px: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 3], a: u8) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        px.fill_rect(r, &paint(c, a), Transform::identity(), None);
    }
}

/// A scroll with rolled ends, casting a shadow on a dark desk.
fn scroll_at(x: f32, y: f32) -> [u8; 3] {
    let (left, right) = (150.0, 1050.0);
    for (y0, y1) in [(56.0, 104.0), (526.0, 574.0)] {
        if y >= y0 && y <= y1 && x >= left - 22.0 && x <= right + 22.0 {
            let t = (y - y0) / (y1 - y0);
            let round = 0.42 + 0.58 * (std::f32::consts::PI * t).sin().powf(0.6);
            let end = ((x - (left - 22.0)).min(right + 22.0 - x) / 16.0).clamp(0.0, 1.0);
            let s = sheet_colour(x, y);
            let k = round * (0.5 + 0.5 * end);
            return [clamp8(s[0] * k), clamp8(s[1] * k), clamp8(s[2] * k)];
        }
    }
    if x >= left && x <= right && y > 100.0 && y < 530.0 {
        // the sheet curls into the rolls, so it darkens next to them
        let near = ((y - 104.0).min(526.0 - y) / 28.0).clamp(0.0, 1.0);
        let k = 0.74 + 0.26 * near;
        let s = sheet_colour(x, y);
        return [clamp8(s[0] * k), clamp8(s[1] * k), clamp8(s[2] * k)];
    }
    let d = desk_colour(x, y);
    let dx = (left - 22.0 - x).max(x - right - 22.0).max(0.0);
    let dy = (56.0 - y).max(y - 574.0).max(0.0);
    let shadow = (1.0 - (dx * dx + dy * dy).sqrt() / 36.0).clamp(0.0, 1.0) * 0.6;
    [clamp8(d[0] * (1.0 - shadow)), clamp8(d[1] * (1.0 - shadow)), clamp8(d[2] * (1.0 - shadow))]
}

/// Two red ribbon tails with swallowtail ends.
fn ribbon(px: &mut Pixmap, cx: f32, top: f32, len: f32) {
    for (dx, tilt, c) in [(-18.0, -26.0, [132, 18, 22]), (6.0, 30.0, [92, 10, 14])] {
        let mut pb = PathBuilder::new();
        pb.move_to(cx + dx, top);
        pb.line_to(cx + dx + 22.0, top);
        pb.line_to(cx + dx + 22.0 + tilt, top + len);
        pb.line_to(cx + dx + 11.0 + tilt, top + len - 14.0);
        pb.line_to(cx + dx + tilt, top + len);
        pb.close();
        if let Some(path) = pb.finish() {
            px.fill_path(&path, &paint(c, 255), FillRule::Winding, Transform::identity(), None);
        }
    }
}

fn scroll(p: &mut Pen<'_>, q: &Quote) {
    let (ink, sepia) = ([44, 26, 14], [112, 72, 38]);
    let sd = seed(q);
    if let Some(bg) = texture(scroll_at) {
        p.px.draw_pixmap(0, 0, bg.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    let date = p.fit(&olden_date(q), 16.0, Face::SerifItalic, Weight::NORMAL, 820.0);
    p.centered(&date, W / 2.0, 140.0, 16.0, Face::SerifItalic, Weight::NORMAL, sepia);
    let text = p.paragraph(&q.text, Face::Script, Weight::NORMAL, 760.0, 250.0, 58.0, 28.0, Align::Center);
    let th = Pen::height(&text);
    let top = 168.0 + (250.0 - th).max(0.0) / 2.0;
    p.draw_ink(&text, W / 2.0 - 380.0, top, ink);
    let sig = p.fit(&format!("— {}", q.author), 42.0, Face::Script, Weight::NORMAL, 520.0);
    let sig_buf = p.layout(&sig, 42.0, Face::Script, Weight::NORMAL, None, None);
    let (line_y, sw) = sig_buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((42.0, 0.0));
    let base = (top + th + 56.0).min(488.0);
    let sx = W / 2.0 - sw / 2.0;
    p.draw_ink(&sig_buf, sx, base - line_y, ink);
    flourish(&mut p.px, sx, sx + sw, base + 12.0, ink);
    let handle = p.fit(&format!("@{}", q.handle), 15.0, Face::SerifItalic, Weight::NORMAL, 400.0);
    p.centered(&handle, W / 2.0, (base + 38.0).min(514.0), 15.0, Face::SerifItalic, Weight::NORMAL, sepia);
    ribbon(&mut p.px, 1000.0, 520.0, 96.0);
    wax_seal(p, q, 1000.0, 510.0, 62.0, sd);
    grain(&mut p.px, 5, sd);
}

/// Yellowed letter paper with a deckled edge, on the desk.
fn letter_at(x: f32, y: f32) -> [u8; 3] {
    let edge = (x - 70.0).min(880.0 - x).min(y - 40.0).min(592.0 - y) + (noise(x * 0.25, y * 0.25, 111) - 0.5) * 5.0;
    if edge < 0.0 {
        let d = desk_colour(x, y);
        return [clamp8(d[0]), clamp8(d[1]), clamp8(d[2])];
    }
    let age = (fbm(x * 0.004, y * 0.004, 113) - 0.45) * 0.35 + fbm(x * 0.06, y * 0.015, 115) * 0.05;
    let fresh = (edge / 60.0).clamp(0.0, 1.0).powf(0.4);
    let base = [244.0 - age * 50.0, 236.0 - age * 56.0, 214.0 - age * 70.0];
    let old = [214.0, 190.0, 140.0];
    [
        clamp8(old[0] + (base[0] - old[0]) * fresh),
        clamp8(old[1] + (base[1] - old[1]) * fresh),
        clamp8(old[2] + (base[2] - old[2]) * fresh),
    ]
}

/// The ring a cup of chai leaves behind.
fn coffee_ring(px: &mut Pixmap, cx: f32, cy: f32, r: f32, seed: u32) {
    let (w, h) = (px.width() as i32, px.height() as i32);
    let data = px.data_mut();
    let reach = r + 14.0;
    for y in ((cy - reach) as i32).max(0)..=((cy + reach) as i32).min(h - 1) {
        for x in ((cx - reach) as i32).max(0)..=((cx + reach) as i32).min(w - 1) {
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let dist = (dx * dx + dy * dy).sqrt();
            let ang = dy.atan2(dx);
            let wobble = (noise(ang * 2.0 + 4.0, 1.0, seed) - 0.5) * 8.0;
            let ring = (-((dist - r - wobble) / 3.5).powi(2)).exp() * (0.55 + 0.45 * noise(ang * 3.0, 2.0, seed ^ 7));
            let fill = if dist < r + wobble { 0.07 } else { 0.0 };
            let a = (ring * 0.38 + fill).min(0.6);
            if a < 0.01 {
                continue;
            }
            let i = ((y * w + x) * 4) as usize;
            for (k, c) in [120.0, 78.0, 38.0].into_iter().enumerate() {
                data[i + k] = (data[i + k] as f32 * (1.0 - a) + c * a) as u8;
            }
        }
    }
}

/// The writer's face as a perforated postage stamp.
fn stamp(p: &mut Pen<'_>, q: &Quote, x: f32, y: f32, w: f32, h: f32) {
    rect(&mut p.px, x, y, w, h, [242, 236, 222]);
    let desk = [30, 20, 13];
    let mut t = 0.0;
    while t <= w {
        dot(&mut p.px, x + t, y, 5.0, desk, 255);
        dot(&mut p.px, x + t, y + h, 5.0, desk, 255);
        t += 14.0;
    }
    let mut t = 0.0;
    while t <= h {
        dot(&mut p.px, x, y + t, 5.0, desk, 255);
        dot(&mut p.px, x + w, y + t, 5.0, desk, 255);
        t += 14.0;
    }
    let inset = 14.0;
    let img_h = h - 62.0;
    if let Some(pm) = decode(q.avatar.as_deref()).and_then(|img| portrait(&img, (w - 2.0 * inset) as u32, img_h as u32, &Tone::Colour)) {
        let faded = PixmapPaint { opacity: 0.88, ..PixmapPaint::default() };
        p.px.draw_pixmap((x + inset) as i32, (y + inset) as i32, pm.as_ref(), &faded, Transform::identity(), None);
    }
    if let Some(frame) = rrect(x + inset, y + inset, w - 2.0 * inset, img_h, 0.5) {
        let stroke = Stroke { width: 1.5, ..Stroke::default() };
        p.px.stroke_path(&frame, &paint([120, 40, 40], 255), &stroke, Transform::identity(), None);
    }
    p.centered("MLCI POST", x + w / 2.0, y + h - 30.0, 15.0, Face::Serif, Weight::BOLD, [120, 36, 36]);
    p.centered("₹14", x + w / 2.0, y + h - 12.0, 13.0, Face::Serif, Weight::SEMIBOLD, [80, 70, 60]);
}

/// A round cancellation postmark, with waves running off to the right.
fn postmark(p: &mut Pen<'_>, cx: f32, cy: f32, r: f32, top: &str, bottom: &str) {
    let ink = [34, 40, 72];
    for (rr, width) in [(r, 3.0), (r - 9.0, 1.5)] {
        if let Some(c) = PathBuilder::from_circle(cx, cy, rr) {
            let stroke = Stroke { width, ..Stroke::default() };
            p.px.stroke_path(&c, &paint(ink, 190), &stroke, Transform::identity(), None);
        }
    }
    let a = p.fit(top, 12.0, Face::Sans, Weight::BOLD, 2.0 * r - 14.0);
    p.centered(&a, cx, cy + 3.0, 12.0, Face::Sans, Weight::BOLD, ink);
    let b = p.fit(bottom, 9.0, Face::Sans, Weight::SEMIBOLD, 2.0 * r - 18.0);
    p.centered(&b, cx, cy + 19.0, 9.0, Face::Sans, Weight::SEMIBOLD, ink);
    for k in 0..4 {
        let y0 = cy - 26.0 + 17.0 * k as f32;
        let mut x = cx + r + 4.0;
        while x < cx + r + 170.0 {
            dot(&mut p.px, x, y0 + ((x - cx) * 0.06).sin() * 5.0, 1.3, ink, 170);
            x += 1.5;
        }
    }
}

fn chitthi(p: &mut Pen<'_>, q: &Quote) {
    let ink = [30, 34, 62];
    let sd = seed(q);
    if let Some(bg) = texture(letter_at) {
        p.px.draw_pixmap(0, 0, bg.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    for k in 0..10 {
        rect_a(&mut p.px, 78.0, 128.0 + 44.0 * k as f32, 794.0, 1.0, [150, 172, 196], 150);
    }
    rect_a(&mut p.px, 150.0, 46.0, 1.5, 540.0, [196, 90, 90], 170);
    coffee_ring(&mut p.px, 720.0, 470.0, 58.0, sd);
    let greet = p.layout("Dear MLCI,", 34.0, Face::Script, Weight::NORMAL, None, None);
    let gy = greet.layout_runs().next().map(|r| r.line_y).unwrap_or(34.0);
    p.draw_ink(&greet, 170.0, 124.0 - gy, ink);
    // written along the ruled lines: fixed 44px line height, smaller hand for longer notes
    let mut size = 38.0;
    let text = loop {
        let buf = p.layout_with(&q.text, size, 44.0, Face::Script, Weight::NORMAL, Some(600.0), Some(Align::Left));
        if buf.layout_runs().count() <= 8 || size <= 24.0 {
            break buf;
        }
        size -= 2.0;
    };
    let first = text.layout_runs().next().map(|r| r.line_y).unwrap_or(30.0);
    p.draw_ink(&text, 170.0, 168.0 - first, ink);
    let lines = text.layout_runs().count().max(1) as f32;
    let last = 172.0 + 44.0 * (lines - 1.0);
    let sig_base = (last + 88.0).min(524.0) - 4.0;
    let sig = p.fit(&format!("— {}", q.author), 38.0, Face::Script, Weight::NORMAL, 420.0);
    let sig_buf = p.layout(&sig, 38.0, Face::Script, Weight::NORMAL, None, None);
    let (sly, sw) = sig_buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((38.0, 0.0));
    p.draw_ink(&sig_buf, 850.0 - sw, sig_base - sly, ink);
    stamp(p, q, 930.0, 70.0, 180.0, 212.0);
    // franked on the paper beside the stamp, the waves running across it
    postmark(p, 846.0, 132.0, 58.0, &q.when.to_uppercase(), &q.channel.to_uppercase());
    let handle = p.fit(&format!("@{}", q.handle), 16.0, Face::SerifItalic, Weight::NORMAL, 240.0);
    p.centered(&handle, 1020.0, 404.0, 16.0, Face::SerifItalic, Weight::NORMAL, [196, 182, 156]);
    grain(&mut p.px, 4, sd);
}

fn vellum_at(x: f32, y: f32) -> [u8; 3] {
    let m = (fbm(x * 0.003, y * 0.003, 101) - 0.45) * 0.5 + fbm(x * 0.07, y * 0.02, 103) * 0.06;
    [clamp8(240.0 - m * 40.0), clamp8(230.0 - m * 46.0), clamp8(204.0 - m * 56.0)]
}

fn diamond(px: &mut Pixmap, cx: f32, cy: f32, s: f32, c: [u8; 3]) {
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - s);
    pb.line_to(cx + s, cy);
    pb.line_to(cx, cy + s);
    pb.line_to(cx - s, cy);
    pb.close();
    if let Some(path) = pb.finish() {
        px.fill_path(&path, &paint(c, 255), FillRule::Winding, Transform::identity(), None);
    }
}

/// Gold double rule with red and blue diamonds between, gilded corners.
fn manuscript_border(px: &mut Pixmap) {
    let (gold, red, blue) = ([182, 142, 64], [150, 32, 30], [34, 62, 132]);
    for (inset, width) in [(22.0, 3.0), (46.0, 1.5)] {
        if let Some(frame) = rrect(inset, inset, W - 2.0 * inset, H - 2.0 * inset, 1.0) {
            let stroke = Stroke { width, ..Stroke::default() };
            px.stroke_path(&frame, &paint(gold, 255), &stroke, Transform::identity(), None);
        }
    }
    let band = 34.0;
    let (mut x, mut i) = (60.0, 0);
    while x < W - 50.0 {
        let c = if i % 2 == 0 { red } else { blue };
        diamond(px, x, band, 5.0, c);
        diamond(px, x, H - band, 5.0, c);
        x += 22.0;
        i += 1;
    }
    let (mut y, mut i) = (60.0, 0);
    while y < H - 50.0 {
        let c = if i % 2 == 0 { blue } else { red };
        diamond(px, band, y, 5.0, c);
        diamond(px, W - band, y, 5.0, c);
        y += 22.0;
        i += 1;
    }
    for (cx, cy) in [(band, band), (W - band, band), (band, H - band), (W - band, H - band)] {
        if let Some(sq) = rrect(cx - 13.0, cy - 13.0, 26.0, 26.0, 2.0) {
            px.fill_path(&sq, &paint(gold, 255), FillRule::Winding, Transform::identity(), None);
        }
        dot(px, cx, cy, 5.0, red, 255);
    }
}

/// The avatar in a bevelled gold ring, lit from the upper left.
fn gilded_frame(p: &mut Pen<'_>, q: &Quote, cx: f32, cy: f32, r: f32) {
    let (w, h) = (p.px.width() as i32, p.px.height() as i32);
    let (inner, outer) = (r + 5.0, r + 16.0);
    {
        let data = p.px.data_mut();
        for y in ((cy - outer) as i32 - 1).max(0)..=((cy + outer) as i32 + 1).min(h - 1) {
            for x in ((cx - outer) as i32 - 1).max(0)..=((cx + outer) as i32 + 1).min(w - 1) {
                let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                let dist = (dx * dx + dy * dy).sqrt();
                if dist > outer + 1.0 || dist < inner {
                    continue;
                }
                let across = ((dist - inner) / (outer - inner)).clamp(0.0, 1.0);
                let bevel = 0.7 + 0.3 * (std::f32::consts::PI * across).sin();
                let light = 0.75 + 0.35 * ((-dx - dy) / (1.4 * outer)).clamp(-1.0, 1.0);
                let aa = (outer - dist + 1.0).clamp(0.0, 1.0);
                let i = ((y * w + x) * 4) as usize;
                for (k, base) in [212.0, 168.0, 78.0].into_iter().enumerate() {
                    let gold = (base * bevel * light).clamp(0.0, 255.0);
                    data[i + k] = (data[i + k] as f32 * (1.0 - aa) + gold * aa) as u8;
                }
            }
        }
    }
    dot(&mut p.px, cx, cy, inner, [140, 30, 28], 255);
    dot(&mut p.px, cx, cy, r + 2.0, [236, 224, 196], 255);
    match medallion(q.avatar.as_deref(), (2.0 * r) as u32, &Tone::Colour) {
        Some(pm) => p.px.draw_pixmap(
            (cx - r).round() as i32,
            (cy - r).round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        None => p.centered(&initial(&q.author), cx, cy + r * 0.3, r * 0.9, Face::Serif, Weight::BOLD, [140, 30, 28]),
    }
}

fn illuminated(p: &mut Pen<'_>, q: &Quote) {
    let (ink, red, gold, blue) = ([40, 30, 24], [150, 32, 30], [190, 150, 70], [34, 62, 132]);
    let sd = seed(q);
    if let Some(bg) = texture(vellum_at) {
        p.px.draw_pixmap(0, 0, bg.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    vignette_tint(&mut p.px, 90, [110, 80, 40]);
    manuscript_border(&mut p.px);
    gilded_frame(p, q, 262.0, 318.0, 120.0);
    p.line(&spaced("LIBER GYAAN · CAPUT XIV"), 470.0, 104.0, 15.0, Face::Serif, Weight::SEMIBOLD, red);
    // the first letter gilded, the rest of the quote hanging beside it
    let mut chars = q.text.chars();
    let first = chars.next().unwrap_or(' ');
    let (cap, rest) = if first.is_ascii_alphanumeric() {
        (Some(first.to_ascii_uppercase().to_string()), chars.as_str().to_string())
    } else {
        (None, q.text.clone())
    };
    let top = 136.0;
    let tx = if cap.is_some() { 574.0 } else { 470.0 };
    if let Some(cap) = &cap {
        let s = 86.0;
        if let Some(b) = rrect(470.0, top, s, s, 3.0) {
            p.px.fill_path(&b, &paint(blue, 255), FillRule::Winding, Transform::identity(), None);
            let stroke = Stroke { width: 3.0, ..Stroke::default() };
            p.px.stroke_path(&b, &paint(gold, 255), &stroke, Transform::identity(), None);
        }
        for (dx, dy) in [(8.0, 8.0), (s - 8.0, 8.0), (8.0, s - 8.0), (s - 8.0, s - 8.0)] {
            dot(&mut p.px, 470.0 + dx, top + dy, 2.5, gold, 255);
        }
        p.centered(cap, 470.0 + s / 2.0, top + s * 0.76, s * 0.7, Face::Serif, Weight::BOLD, [236, 196, 110]);
    }
    let text = p.paragraph(rest.trim_start(), Face::Serif, Weight::NORMAL, 1130.0 - tx, 320.0, 42.0, 22.0, Align::Left);
    let th = Pen::height(&text);
    p.draw_ink(&text, tx, top + 2.0, ink);
    let base = (top + th.max(86.0) + 50.0).min(520.0);
    let by = p.fit(&format!("— {}", q.author), 28.0, Face::SerifItalic, Weight::NORMAL, 600.0);
    p.line(&by, 470.0, base, 28.0, Face::SerifItalic, Weight::NORMAL, red);
    let handle = p.fit(&format!("@{}", q.handle), 15.0, Face::Serif, Weight::NORMAL, 600.0);
    p.line(&handle, 470.0, base + 24.0, 15.0, Face::Serif, Weight::NORMAL, [120, 100, 80]);
    let date = p.fit(&olden_date(q), 15.0, Face::SerifItalic, Weight::NORMAL, 660.0);
    p.line(&date, 470.0, H - 70.0, 15.0, Face::SerifItalic, Weight::NORMAL, [130, 104, 70]);
    grain(&mut p.px, 4, sd);
}

fn cosmos(p: &mut Pen<'_>, q: &Quote) {
    if let Some(sky) = texture(space_at) {
        p.px.draw_pixmap(0, 0, sky.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    glow(&mut p.px, 880.0, 150.0, 420.0, [120, 60, 210], 45);
    stars(&mut p.px, seed(q), 300, (600.0, 60.0, 1180.0, 580.0));
    planet(p, q, 300.0, 315.0, 150.0);
    let (x0, x1) = (620.0, 1160.0);
    let cx = (x0 + x1) / 2.0;
    let text = p.paragraph(&q.text, Face::Sans, Weight::LIGHT, x1 - x0, 320.0, 52.0, 24.0, Align::Center);
    let th = Pen::height(&text);
    let top = ((H - (th + 110.0)) / 2.0 - 6.0).max(40.0);
    p.draw(&text, x0, top, [236, 240, 250]);
    let ry = top + th + 30.0;
    p.centered("✦", cx, ry + 10.0, 18.0, Face::Sans, Weight::NORMAL, [150, 175, 230]);
    let by = p.fit(&spaced(&q.author.to_uppercase()), 18.0, Face::Sans, Weight::MEDIUM, x1 - x0);
    p.centered(&by, cx, ry + 48.0, 18.0, Face::Sans, Weight::MEDIUM, [170, 190, 230]);
    let handle = p.fit(&format!("@{}", q.handle), 15.0, Face::Sans, Weight::NORMAL, x1 - x0);
    p.centered(&handle, cx, ry + 72.0, 15.0, Face::Sans, Weight::NORMAL, [110, 124, 160]);
    footer(p, q, [100, 112, 150]);
    grain(&mut p.px, 3, seed(q));
}

fn zen(p: &mut Pen<'_>, q: &Quote) {
    let (ink, soft) = ([38, 36, 32], [120, 114, 104]);
    if let Some(paper) = texture(rice_paper_at) {
        p.px.draw_pixmap(0, 0, paper.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    let (cx, cy) = (320.0, 315.0);
    if let Some(pm) = medallion(q.avatar.as_deref(), 210, &Tone::Grey) {
        let faded = PixmapPaint { opacity: 0.88, ..PixmapPaint::default() };
        p.px.draw_pixmap((cx - 105.0) as i32, (cy - 105.0) as i32, pm.as_ref(), &faded, Transform::identity(), None);
    }
    enso(&mut p.px, cx, cy, 168.0, seed(q));
    let (x0, x1) = (640.0, 1120.0);
    let text = p.paragraph(&q.text, Face::Serif, Weight::NORMAL, x1 - x0, 300.0, 46.0, 22.0, Align::Left);
    let th = Pen::height(&text);
    let top = ((H - (th + 104.0)) / 2.0).max(50.0);
    p.draw(&text, x0, top, ink);
    let ry = top + th + 30.0;
    rect(&mut p.px, x0, ry, 36.0, 2.0, [180, 170, 158]);
    let by = p.fit(&format!("— {}", q.author), 26.0, Face::SerifItalic, Weight::NORMAL, 380.0);
    let bw = p.line(&by, x0, ry + 44.0, 26.0, Face::SerifItalic, Weight::NORMAL, soft);
    let handle = p.fit(&format!("@{}", q.handle), 15.0, Face::Sans, Weight::NORMAL, 380.0);
    p.line(&handle, x0, ry + 68.0, 15.0, Face::Sans, Weight::NORMAL, [160, 152, 142]);
    // stamped just after the signature, as on a scroll
    seal(p, x0 + bw + 24.0, ry + 12.0, 50.0, &q.author);
    footer(p, q, [168, 160, 150]);
    grain(&mut p.px, 3, seed(q));
}

fn pick(fs: &FontSystem, names: &[&str]) -> String {
    for name in names {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

/// Draw `q` in `style` and return PNG bytes.
pub fn render(q: &Quote, style: Style, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let serif = pick(fs, &["Noto Serif", "Georgia", "Times New Roman"]);
    let sans = pick(fs, &["Montserrat", "Avenir Next", "Noto Sans"]);
    let script = pick(fs, &["Z003", "Apple Chancery", "Snell Roundhand", "Noto Serif"]);
    let mut pen = Pen {
        px: Pixmap::new(W as u32, H as u32)?,
        fs,
        cache: SwashCache::new(),
        serif,
        sans,
        script,
        faces: std::collections::HashMap::new(),
    };
    match style {
        Style::Noir => noir(&mut pen, q),
        Style::Rang => rang(&mut pen, q),
        Style::Spotlight => spotlight(&mut pen, q),
        Style::Duotone => duotone(&mut pen, q),
        Style::Marble => marble(&mut pen, q),
        Style::Pothi => pothi(&mut pen, q),
        Style::Scroll => scroll(&mut pen, q),
        Style::Chitthi => chitthi(&mut pen, q),
        Style::Illuminated => illuminated(&mut pen, q),
        Style::Cosmos => cosmos(&mut pen, q),
        Style::Zen => zen(&mut pen, q),
    }
    pen.px.encode_png().ok()
}
