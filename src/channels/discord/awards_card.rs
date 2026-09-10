//! The /awards card.
//!
//! Award cards in two columns under three section headings, each with the
//! winner's avatar, a headline number and a caption. Server names are full of
//! script letters and emoji, so text goes through cosmic-text, which falls
//! back to whichever installed font has each glyph and draws colour emoji.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use tiny_skia::{
    Color as SkColor, FillRule, GradientStop, LinearGradient, Mask, Paint, Path, PathBuilder, Pixmap,
    PixmapPaint, Point, Rect, SpreadMode, Stroke, Transform,
};

pub struct Award {
    pub label: String,
    pub emoji: String,
    pub winner: Option<String>,
    pub stat: String,
    pub sub: String,
    pub avatar: Option<Vec<u8>>,
    pub departed: bool,
}

pub struct Section {
    pub key: String,
    pub name: String,
    pub awards: Vec<Award>,
}

pub struct Card {
    pub title: String,
    pub period: String,
    pub chips: Vec<String>,
    pub sections: Vec<Section>,
    pub footer: String,
}

const W: f32 = 1600.0;
const M: f32 = 60.0;
const GAP: f32 = 28.0;
const CARD_H: f32 = 156.0;
const BG: [u8; 3] = [15, 16, 20];
const CARD: [u8; 3] = [30, 32, 38];
const INK: [u8; 3] = [244, 245, 247];
const MUTED: [u8; 3] = [148, 154, 166];
const LINE: [u8; 3] = [50, 53, 61];

fn colw() -> f32 {
    (W - 2.0 * M - GAP) / 2.0
}

fn section_style(key: &str) -> ([u8; 3], &'static str) {
    match key {
        "time" => ([167, 139, 250], "🌙"),
        "vc" => ([56, 189, 248], "🎧"),
        _ => ([251, 146, 60], "💬"),
    }
}

fn paint(c: [u8; 3], a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], a);
    p.anti_alias = true;
    p
}

fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let k = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn fill_circle(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3]) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        px.fill_path(&path, &paint(c, 255), FillRule::Winding, Transform::identity(), None);
    }
}

/// Blend a glyph fragment onto an opaque canvas.
fn blend_rect(px: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, c: Color) {
    let a = c.a() as u32;
    if a == 0 {
        return;
    }
    let (pw, ph) = (px.width() as i32, px.height() as i32);
    let data = px.data_mut();
    for yy in y.max(0)..(y + h as i32).min(ph) {
        for xx in x.max(0)..(x + w as i32).min(pw) {
            let i = ((yy * pw + xx) * 4) as usize;
            for (k, s) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
                let d = data[i + k] as u32;
                data[i + k] = ((s as u32 * a + d * (255 - a)) / 255) as u8;
            }
            data[i + 3] = 255;
        }
    }
}

/// A square, circle-cropped avatar; grey if the person has left.
fn avatar_pixmap(bytes: Option<&[u8]>, size: u32, departed: bool) -> Option<Pixmap> {
    let mut img = image::load_from_memory(bytes?).ok()?;
    if departed {
        img = image::DynamicImage::ImageLuma8(img.to_luma8());
    }
    let rgba = img.resize_to_fill(size, size, image::imageops::FilterType::Lanczos3).to_rgba8();
    let mut pm = Pixmap::new(size, size)?;
    for (dst, p) in pm.pixels_mut().iter_mut().zip(rgba.pixels()) {
        *dst = tiny_skia::ColorU8::from_rgba(p[0], p[1], p[2], p[3]).premultiply();
    }
    let mut mask = Mask::new(size, size)?;
    let s = size as f32;
    mask.fill_path(&PathBuilder::from_circle(s / 2.0, s / 2.0, s / 2.0)?, FillRule::Winding, true, Transform::identity());
    pm.apply_mask(&mask);
    Some(pm)
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

struct Ctx<'a> {
    px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    family: String,
}

impl Ctx<'_> {
    fn buffer(&mut self, text: &str, size: f32, weight: Weight) -> Buffer {
        let fs = &mut *self.fs;
        let mut buf = Buffer::new(fs, Metrics::new(size, size * 1.3));
        buf.set_size(fs, None, None);
        let attrs = Attrs::new().family(Family::Name(self.family.as_str())).weight(weight);
        buf.set_text(fs, text, attrs, Shaping::Advanced);
        buf.shape_until_scroll(fs, false);
        buf
    }

    fn measure(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        let buf = self.buffer(text, size, weight);
        let w = buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        w
    }

    /// Draw `text` with its baseline at `baseline`; returns the width drawn.
    fn text(&mut self, text: &str, x: f32, baseline: f32, size: f32, weight: Weight, color: [u8; 3]) -> f32 {
        let buf = self.buffer(text, size, weight);
        let (line_y, width) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
        let (ox, oy) = (x.round() as i32, (baseline - line_y).round() as i32);
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        buf.draw(fs, cache, Color::rgb(color[0], color[1], color[2]), |gx, gy, w, h, c| {
            blend_rect(px, ox + gx, oy + gy, w, h, c);
        });
        width
    }

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

    fn card(&mut self, x: f32, y: f32, w: f32, a: &Award, accent: [u8; 3]) {
        let h = CARD_H;
        let Some(shape) = rrect(x, y, w, h, 24.0) else { return };
        self.px.fill_path(&shape, &paint(CARD, 255), FillRule::Winding, Transform::identity(), None);
        // A faint wash of the section colour behind the number.
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(x + w * 0.59, y),
            Point::from_xy(x + w, y),
            vec![
                GradientStop::new(0.0, SkColor::from_rgba8(accent[0], accent[1], accent[2], 0)),
                GradientStop::new(1.0, SkColor::from_rgba8(accent[0], accent[1], accent[2], 58)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let mut p = Paint::default();
            p.shader = shader;
            p.anti_alias = true;
            self.px.fill_path(&shape, &p, FillRule::Winding, Transform::identity(), None);
        }
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        self.px.stroke_path(&shape, &paint(LINE, 255), &stroke, Transform::identity(), None);

        let won = a.winner.is_some();
        let (av, rw, gap) = (104.0_f32, 5.0_f32, 4.0_f32);
        let outer = av / 2.0 + rw + gap;
        let (cx, cy) = (x + 22.0 + outer, y + h / 2.0);
        fill_circle(&mut self.px, cx, cy, outer, if a.departed { MUTED } else { accent });
        fill_circle(&mut self.px, cx, cy, outer - rw, CARD);
        match avatar_pixmap(a.avatar.as_deref(), av as u32, a.departed) {
            Some(pm) => self.px.draw_pixmap(
                (cx - av / 2.0).round() as i32,
                (cy - av / 2.0).round() as i32,
                pm.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                None,
            ),
            None => {
                let dim = [
                    (accent[0] as f32 * 0.35) as u8,
                    (accent[1] as f32 * 0.35) as u8,
                    (accent[2] as f32 * 0.35) as u8,
                ];
                fill_circle(&mut self.px, cx, cy, av / 2.0, dim);
                let initial = a
                    .winner
                    .as_deref()
                    .and_then(|n| n.chars().find(|c| c.is_ascii_alphanumeric()))
                    .map(|c| c.to_ascii_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let iw = self.measure(&initial, 46.0, Weight::BOLD);
                self.text(&initial, cx - iw / 2.0, cy + 16.0, 46.0, Weight::BOLD, INK);
            }
        }

        let tx = x + 22.0 + 2.0 * outer + 22.0;
        let ssize = if a.stat.chars().count() <= 5 { 52.0 } else { 44.0 };
        let sw = self.measure(&a.stat, ssize, Weight::EXTRA_BOLD);
        self.text(&a.stat, x + w - 28.0 - sw, y + 96.0, ssize, Weight::EXTRA_BOLD, if won { accent } else { MUTED });
        let textmax = (x + w - 28.0 - sw - 18.0) - tx;

        let ew = self.text(&a.emoji, tx, y + 44.0, 22.0, Weight::NORMAL, INK);
        let label = self.fit(&a.label, 18.0, Weight::SEMIBOLD, textmax - ew - 8.0);
        self.text(&label, tx + ew + 8.0, y + 43.0, 18.0, Weight::SEMIBOLD, accent);
        let name = self.fit(a.winner.as_deref().unwrap_or("Koi nahi"), 32.0, Weight::BOLD, textmax);
        self.text(&name, tx, y + 90.0, 32.0, Weight::BOLD, if won { INK } else { MUTED });
        let sub = if a.departed { format!("left the server · {}", a.sub) } else { a.sub.clone() };
        let sub = self.fit(&sub, 18.0, Weight::MEDIUM, x + w - 28.0 - tx);
        self.text(&sub, tx, y + 126.0, 18.0, Weight::MEDIUM, MUTED);
    }
}

/// Draw the card and return it as PNG bytes.
pub fn render(card: &Card, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let family = pick_family(fs);
    let mut height = 300.0;
    for s in &card.sections {
        height += 26.0 + 58.0 + ((s.awards.len() + 1) / 2) as f32 * (CARD_H + GAP);
    }
    height += 18.0 + 80.0;

    let mut ctx = Ctx { px: Pixmap::new(W as u32, height as u32)?, fs, cache: SwashCache::new(), family };
    ctx.px.fill(SkColor::from_rgba8(BG[0], BG[1], BG[2], 255));

    // Purple-to-teal band that fades out rather than stopping at an edge.
    for row in 0..470 {
        let alpha = (255.0 * (1.0 - row as f32 / 470.0).powf(1.7)) as u8;
        if let Some(shader) = LinearGradient::new(
            Point::from_xy(0.0, 0.0),
            Point::from_xy(W, 0.0),
            vec![
                GradientStop::new(0.0, SkColor::from_rgba8(78, 54, 158, alpha)),
                GradientStop::new(1.0, SkColor::from_rgba8(16, 100, 146, alpha)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            let mut p = Paint::default();
            p.shader = shader;
            if let Some(r) = Rect::from_xywh(0.0, row as f32, W, 1.0) {
                ctx.px.fill_rect(r, &p, Transform::identity(), None);
            }
        }
    }

    let tw = ctx.text("🏆", M, 132.0, 76.0, Weight::NORMAL, INK);
    ctx.text(&card.title, M + tw + 22.0, 132.0, 80.0, Weight::EXTRA_BOLD, INK);
    ctx.text(&card.period, M + 2.0, 182.0, 23.0, Weight::SEMIBOLD, [208, 212, 222]);
    let mut cx = M;
    for chip in &card.chips {
        let cw = ctx.measure(chip, 20.0, Weight::SEMIBOLD) + 36.0;
        if let Some(pill) = rrect(cx, 214.0, cw, 40.0, 20.0) {
            ctx.px.fill_path(&pill, &paint([255, 255, 255], 30), FillRule::Winding, Transform::identity(), None);
            let stroke = Stroke { width: 2.0, ..Stroke::default() };
            ctx.px.stroke_path(&pill, &paint([255, 255, 255], 80), &stroke, Transform::identity(), None);
        }
        ctx.text(chip, cx + 18.0, 241.0, 20.0, Weight::SEMIBOLD, INK);
        cx += cw + 12.0;
    }

    let mut y = 300.0;
    for s in &card.sections {
        let (col, emoji) = section_style(&s.key);
        y += 26.0;
        let ew = ctx.text(emoji, M, y + 30.0, 30.0, Weight::NORMAL, INK);
        let nw = ctx.text(&s.name, M + ew + 14.0, y + 30.0, 30.0, Weight::EXTRA_BOLD, col);
        let lx = M + ew + 14.0 + nw + 22.0;
        if let Some(r) = Rect::from_xywh(lx, y + 18.0, W - M - lx, 2.0) {
            ctx.px.fill_rect(r, &paint(LINE, 255), Transform::identity(), None);
        }
        y += 58.0;
        let n = s.awards.len();
        for (i, a) in s.awards.iter().enumerate() {
            // An odd one out takes the whole row instead of leaving a hole.
            let wide = i == n - 1 && n % 2 == 1;
            let x = M + (i % 2) as f32 * (colw() + GAP);
            let cy = y + (i / 2) as f32 * (CARD_H + GAP);
            ctx.card(x, cy, if wide { W - 2.0 * M } else { colw() }, a, col);
        }
        y += ((n + 1) / 2) as f32 * (CARD_H + GAP);
    }

    y += 18.0;
    if let Some(r) = Rect::from_xywh(M, y, W - 2.0 * M, 2.0) {
        ctx.px.fill_rect(r, &paint(LINE, 255), Transform::identity(), None);
    }
    let fw = ctx.measure(&card.footer, 19.0, Weight::MEDIUM);
    ctx.text(&card.footer, (W - fw) / 2.0, y + 46.0, 19.0, Weight::MEDIUM, MUTED);
    ctx.px.encode_png().ok()
}
