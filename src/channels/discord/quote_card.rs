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

use super::awards_card::{avatar_pixmap, blend_rect, fill_circle, paint};

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
    Scroll,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Noir, Style::Rang, Style::Spotlight, Style::Scroll];

    pub fn key(self) -> &'static str {
        match self {
            Style::Noir => "noir",
            Style::Rang => "rang",
            Style::Spotlight => "spotlight",
            Style::Scroll => "scroll",
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
            Style::Scroll => "📜 Scroll",
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

// The scroll: a horizontal parchment between two vertical rolled ends, with
// turned brass knobs, burgundy silk ribbons, a sepia portrait on the left and
// the words in calligraphy on the right.

const ROLL_L: f32 = 80.0;
const ROLL_R: f32 = 1120.0;
const ROLL_HW: f32 = 34.0;
const ROLL_TOP: f32 = 92.0;
const ROLL_BOTTOM: f32 = 538.0;
const SHEET_TOP: f32 = 98.0;
const SHEET_BOTTOM: f32 = 532.0;

/// Warm, aged parchment: broad mottling, a few stains, fine fibres.
fn sheet_colour(x: f32, y: f32) -> [f32; 3] {
    let mottle = (fbm(x * 0.004, y * 0.004, 91) - 0.45) * 0.6;
    let stains = (fbm(x * 0.015, y * 0.015, 93) - 0.6).max(0.0) * 1.0;
    let fibre = (fbm(x * 0.09, y * 0.02, 95) - 0.5) * 0.18;
    let speck = (noise(x * 0.6, y * 0.6, 97) - 0.5) * 0.05;
    let d = mottle + stains + fibre + speck;
    [222.0 - d * 50.0, 186.0 - d * 60.0, 134.0 - d * 66.0]
}

/// Walnut desk, and the parchment lying on it between the rolls.
fn scroll_desk_at(x: f32, y: f32) -> [u8; 3] {
    let deckle = (fbm(x * 0.022, y * 0.022, 131) - 0.5) * 22.0;
    let (top, bottom) = (SHEET_TOP + deckle, SHEET_BOTTOM - deckle);
    if x >= ROLL_L + 16.0 && x <= ROLL_R - 16.0 && y >= top && y <= bottom {
        let s = sheet_colour(x, y);
        let edge = (y - top).min(bottom - y);
        let browned = (1.0 - (edge / 20.0).clamp(0.0, 1.0)).powf(1.4) * 0.26;
        let by_roll = (1.0 - ((x - (ROLL_L + ROLL_HW)).min((ROLL_R - ROLL_HW) - x) / 46.0).clamp(0.0, 1.0)) * 0.28;
        let k = 1.02 - browned - by_roll;
        return [clamp8(s[0] * k), clamp8(s[1] * k), clamp8(s[2] * k)];
    }
    let v = fbm(x * 0.0012, y * 0.06, 137) * 0.8 + (fbm(x * 0.008, y * 0.3, 139) - 0.5) * 0.4;
    let wood = [58.0 + v * 60.0, 32.0 + v * 34.0, 20.0 + v * 18.0];
    let dx = (ROLL_L - ROLL_HW - 6.0 - x).max(x - (ROLL_R + ROLL_HW + 6.0)).max(0.0);
    let dy = (SHEET_TOP - 60.0 - y).max(y - (SHEET_BOTTOM + 60.0)).max(0.0);
    let shadow = (1.0 - (dx * dx + dy * dy).sqrt() / 24.0).clamp(0.0, 1.0) * 0.4;
    [clamp8(wood[0] * (1.0 - shadow)), clamp8(wood[1] * (1.0 - shadow)), clamp8(wood[2] * (1.0 - shadow))]
}

/// A rolled end: parchment wound round a rod, shaded as a cylinder.
fn roll(px: &mut Pixmap, cx: f32, top: f32, bottom: f32, hw: f32) {
    let (w, h) = (px.width() as i32, px.height() as i32);
    let data = px.data_mut();
    for y in (top as i32).max(0)..=(bottom as i32).min(h - 1) {
        for x in ((cx - hw) as i32).max(0)..=((cx + hw) as i32).min(w - 1) {
            let t = (x as f32 - cx) / hw;
            if t.abs() > 1.0 {
                continue;
            }
            let round = (1.0 - t * t).sqrt();
            let light = 0.52 + 0.42 * round + 0.12 * (1.0 - (t + 0.35).abs() * 2.2).max(0.0);
            let s = sheet_colour(x as f32 * 0.4, y as f32 * 0.3);
            let i = ((y * w + x) * 4) as usize;
            data[i] = clamp8((s[0] + 24.0) * light);
            data[i + 1] = clamp8((s[1] + 26.0) * light);
            data[i + 2] = clamp8((s[2] + 30.0) * light);
        }
    }
}

/// A turned fitting - bronze knob or silk band - shaded as a lit cylinder.
/// `profile(y)` gives its half-width on each row, 0 where there is nothing.
fn turned(px: &mut Pixmap, cx: f32, y0: f32, y1: f32, base: [f32; 3], shine: f32, profile: impl Fn(f32) -> f32) {
    let (w, h) = (px.width() as i32, px.height() as i32);
    let data = px.data_mut();
    for y in (y0.floor() as i32).max(0)..=(y1.ceil() as i32).min(h - 1) {
        let hw = profile(y as f32);
        if hw <= 0.0 {
            continue;
        }
        for x in ((cx - hw - 1.0) as i32).max(0)..=((cx + hw + 1.0) as i32).min(w - 1) {
            let off = x as f32 - cx;
            let aa = (hw + 0.5 - off.abs()).clamp(0.0, 1.0);
            if aa <= 0.0 {
                continue;
            }
            let t = (off / hw).clamp(-1.0, 1.0);
            let round = (1.0 - t * t).sqrt();
            let spec = (1.0 - ((t + 0.38) * 3.2).abs()).max(0.0).powi(2);
            let k = 0.3 + 0.62 * round + shine * spec;
            let i = ((y * w + x) * 4) as usize;
            for c in 0..3 {
                let v = (base[c] * k).clamp(0.0, 255.0);
                data[i + c] = (data[i + c] as f32 * (1.0 - aa) + v * aa) as u8;
            }
        }
    }
}

/// Bronze knobs capping both ends of a roll: collar, rings, neck, knob, tip.
fn finials(px: &mut Pixmap, cx: f32, top: f32, bottom: f32, hw: f32) {
    let profile = move |d: f32| -> f32 {
        if d < 0.0 {
            0.0
        } else if d < 7.0 {
            hw + 5.0
        } else if d < 10.0 {
            hw + 1.0
        } else if d < 16.0 {
            hw + 4.0
        } else if d < 22.0 {
            hw * 0.45
        } else if d < 44.0 {
            let m = (d - 33.0) / 11.0;
            hw * 0.28 + hw * 0.42 * (1.0 - m * m).max(0.0).sqrt()
        } else if d < 50.0 {
            5.0
        } else if d < 58.0 {
            let m = (d - 54.0) / 4.0;
            (7.0 * (1.0 - m * m).max(0.0).sqrt()).max(2.0)
        } else {
            0.0
        }
    };
    let bronze = [164.0, 116.0, 50.0];
    turned(px, cx, top - 58.0, top, bronze, 0.5, |y| profile(top - y));
    turned(px, cx, bottom, bottom + 58.0, bronze, 0.5, |y| profile(y - bottom));
}

/// A burgundy silk band round the roll, knotted on the side facing the page.
fn ribbon_band(px: &mut Pixmap, cx: f32, y: f32, hw: f32, dir: f32) {
    turned(px, cx, y - 13.0, y + 13.0, [132.0, 22.0, 46.0], 0.35, |yy| if (yy - y).abs() <= 13.0 { hw + 3.0 } else { 0.0 });
    let kx = cx + dir * (hw + 2.0);
    dot(px, kx, y, 11.0, [104, 16, 36], 255);
    dot(px, kx - dir * 2.5, y - 3.0, 5.0, [168, 60, 84], 170);
}

/// A ribbon end hanging from its knot: mostly straight down, drifting a
/// little towards the page, with a forked tip. `dir` +1 drifts right.
fn ribbon_tail(px: &mut Pixmap, x: f32, y: f32, dir: f32, len: f32) {
    let at = |fx: f32, fy: f32| (x + dir * fx * len, y + fy * len);
    let mut pb = PathBuilder::new();
    let (sx, sy) = at(0.02, -0.02);
    pb.move_to(sx, sy);
    let ((a, b), (c, d), (e, f)) = (at(0.2, 0.25), at(0.3, 0.62), at(0.4, 1.0));
    pb.cubic_to(a, b, c, d, e, f);
    let (e, f) = at(0.3, 0.9);
    pb.line_to(e, f);
    let (e, f) = at(0.14, 1.0);
    pb.line_to(e, f);
    let ((a, b), (c, d), (e, f)) = (at(0.1, 0.62), at(-0.06, 0.3), at(-0.26, 0.04));
    pb.cubic_to(a, b, c, d, e, f);
    pb.close();
    if let Some(path) = pb.finish() {
        px.fill_path(&path, &paint([112, 18, 40], 255), FillRule::Winding, Transform::identity(), None);
    }
    let mut pb = PathBuilder::new();
    let (sx, sy) = at(0.04, 0.04);
    pb.move_to(sx, sy);
    let ((a, b), (c, d), (e, f)) = (at(0.16, 0.3), at(0.22, 0.6), at(0.3, 0.86));
    pb.cubic_to(a, b, c, d, e, f);
    let ((a, b), (c, d), (e, f)) = (at(0.14, 0.6), at(0.06, 0.3), at(-0.04, 0.08));
    pb.cubic_to(a, b, c, d, e, f);
    pb.close();
    if let Some(path) = pb.finish() {
        px.fill_path(&path, &paint([176, 66, 90], 100), FillRule::Winding, Transform::identity(), None);
    }
}

fn scroll(p: &mut Pen<'_>, q: &Quote) {
    let (ink, bronze, soft) = ([40, 22, 12], [122, 80, 34], [66, 42, 26]);
    if let Some(bg) = texture(scroll_desk_at) {
        p.px.draw_pixmap(0, 0, bg.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    for (cx, dir) in [(ROLL_L, 1.0), (ROLL_R, -1.0)] {
        roll(&mut p.px, cx, ROLL_TOP, ROLL_BOTTOM, ROLL_HW);
        finials(&mut p.px, cx, ROLL_TOP, ROLL_BOTTOM, ROLL_HW);
        for (band, len) in [(ROLL_TOP + 34.0, 84.0), (ROLL_BOTTOM - 34.0, 58.0)] {
            ribbon_band(&mut p.px, cx, band, ROLL_HW, dir);
            ribbon_tail(&mut p.px, cx + dir * (ROLL_HW + 2.0), band + 6.0, dir, len);
        }
    }

    let (ax, ay, r) = (300.0, 315.0, 134.0);
    for (rr, width, c) in [(r + 9.0, 6.0, [98, 62, 26]), (r + 15.5, 1.8, [160, 112, 52])] {
        if let Some(ring) = PathBuilder::from_circle(ax, ay, rr) {
            let stroke = Stroke { width, ..Stroke::default() };
            p.px.stroke_path(&ring, &paint(c, 255), &stroke, Transform::identity(), None);
        }
    }
    match medallion(q.avatar.as_deref(), (2.0 * r) as u32, &Tone::Duo([44, 28, 16], [240, 220, 180])) {
        Some(pm) => p.px.draw_pixmap(
            (ax - r).round() as i32,
            (ay - r).round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        None => p.centered(&initial(&q.author), ax, ay + r * 0.32, r * 0.9, Face::Serif, Weight::BOLD, bronze),
    }
    fleur(&mut p.px, ax, ay - r - 36.0, 44.0, bronze);
    fleur(&mut p.px, ax, ay + r + 36.0, 44.0, bronze);

    // A short quote stays on one big line, as a calligrapher would set it;
    // only a longer one wraps.
    let (cx, width) = (784.0, 500.0);
    let mut one_line = None;
    let mut size = 86.0;
    while size >= 54.0 {
        let buf = p.layout(&q.text, size, Face::Script, Weight::NORMAL, Some(width), Some(Align::Center));
        if buf.layout_runs().count() == 1 {
            one_line = Some(buf);
            break;
        }
        size -= 2.0;
    }
    let text = match one_line {
        Some(buf) => buf,
        None => p.paragraph(&q.text, Face::Script, Weight::NORMAL, width, 170.0, 54.0, 26.0, Align::Center),
    };
    let th = Pen::height(&text);
    let t0 = (315.0 - (th + 236.0) / 2.0).max(108.0);
    p.draw_ink(&text, cx - width / 2.0, t0, ink);
    let mut y = t0 + th + 20.0;
    fleur_rule(&mut p.px, cx, y + 12.0, 130.0, bronze);
    y += 44.0;
    let author = p.fit(&q.author, 44.0, Face::Serif, Weight::BOLD, width);
    p.centered(&author, cx, y + 36.0, 44.0, Face::Serif, Weight::BOLD, ink);
    y += 46.0;
    let handle = p.fit(&format!("@{}", q.handle), 24.0, Face::Serif, Weight::NORMAL, width);
    p.centered(&handle, cx, y + 28.0, 24.0, Face::Serif, Weight::NORMAL, soft);
    y += 56.0;
    fleur_rule(&mut p.px, cx, y + 12.0, 190.0, bronze);
    y += 38.0;
    let meta: Vec<String> = [q.channel.clone(), q.when.clone()].into_iter().filter(|s| !s.is_empty()).collect();
    let meta = p.fit(&meta.join(" · "), 22.0, Face::Serif, Weight::NORMAL, width);
    p.centered(&meta, cx, y + 24.0, 22.0, Face::Serif, Weight::NORMAL, soft);
    grain(&mut p.px, 4, seed(q));
}

/// A fleur-de-lis `size` px tall, centred on (cx, cy).
fn fleur(px: &mut Pixmap, cx: f32, cy: f32, size: f32, colour: [u8; 3]) {
    let at = |x: f32, y: f32| (cx + x * size, cy + y * size);
    let fill = |px: &mut Pixmap, pb: PathBuilder| {
        if let Some(path) = pb.finish() {
            px.fill_path(&path, &paint(colour, 255), FillRule::Winding, Transform::identity(), None);
        }
    };
    let mut pb = PathBuilder::new();
    let (x, y) = at(0.0, -0.5);
    pb.move_to(x, y);
    let ((a, b), (c, d), (e, f)) = (at(0.17, -0.3), at(0.13, -0.02), at(0.0, 0.1));
    pb.cubic_to(a, b, c, d, e, f);
    let ((a, b), (c, d), (e, f)) = (at(-0.13, -0.02), at(-0.17, -0.3), at(0.0, -0.5));
    pb.cubic_to(a, b, c, d, e, f);
    pb.close();
    fill(px, pb);
    for side in [1.0f32, -1.0] {
        let mut pb = PathBuilder::new();
        let (x, y) = at(0.03 * side, 0.06);
        pb.move_to(x, y);
        let ((a, b), (c, d), (e, f)) = (at(0.08 * side, -0.24), at(0.46 * side, -0.3), at(0.42 * side, 0.0));
        pb.cubic_to(a, b, c, d, e, f);
        let ((a, b), (c, d), (e, f)) = (at(0.4 * side, 0.13), at(0.26 * side, 0.12), at(0.27 * side, 0.02));
        pb.cubic_to(a, b, c, d, e, f);
        let ((a, b), (c, d), (e, f)) = (at(0.2 * side, -0.05), at(0.12 * side, 0.04), at(0.05 * side, 0.12));
        pb.cubic_to(a, b, c, d, e, f);
        pb.close();
        fill(px, pb);
    }
    let (x, y) = at(-0.22, 0.08);
    if let Some(r) = Rect::from_xywh(x, y, 0.44 * size, 0.07 * size) {
        px.fill_rect(r, &paint(colour, 255), Transform::identity(), None);
    }
    let mut pb = PathBuilder::new();
    let (x, y) = at(0.0, 0.15);
    pb.move_to(x, y);
    let ((a, b), (e, f)) = (at(0.1, 0.3), at(0.0, 0.5));
    pb.quad_to(a, b, e, f);
    let ((a, b), (e, f)) = (at(-0.1, 0.3), at(0.0, 0.15));
    pb.quad_to(a, b, e, f);
    pb.close();
    fill(px, pb);
}

/// A rule that tapers away on both sides of a small fleur-de-lis.
fn fleur_rule(px: &mut Pixmap, cx: f32, cy: f32, half: f32, colour: [u8; 3]) {
    for side in [1.0f32, -1.0] {
        let mut t = 0.0;
        while t <= 1.0 {
            let x = cx + side * (18.0 + (half - 18.0) * t);
            let width = 0.5 + 1.6 * (1.0 - t).powf(0.7);
            dot(px, x, cy, width / 2.0 + 0.2, colour, 235);
            t += 0.5 / half.max(1.0);
        }
    }
    fleur(px, cx, cy - 1.0, 26.0, colour);
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
        Style::Scroll => scroll(&mut pen, q),
    }
    pen.px.encode_png().ok()
}
