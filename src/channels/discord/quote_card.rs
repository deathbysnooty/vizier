//! Quote cards: one message drawn as an image, in a handful of styles.
//!
//! Every style is the same data - the words, who said them, their avatar,
//! where and when - laid out differently. Quote text is wrapped and sized
//! down step by step until it fits its box, so a one-liner is big and bold and
//! a paragraph still reads.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style as FontStyle, SwashCache, Weight, Wrap,
};
use tiny_skia::{
    Color as SkColor, FillRule, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Point, RadialGradient, Rect,
    SpreadMode, Stroke, Transform,
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
    Classic,
    Akhbaar,
    Neon,
    Filmy,
}

impl Style {
    pub const ALL: [Style; 4] = [Style::Classic, Style::Akhbaar, Style::Neon, Style::Filmy];

    pub fn key(self) -> &'static str {
        match self {
            Style::Classic => "classic",
            Style::Akhbaar => "akhbaar",
            Style::Neon => "neon",
            Style::Filmy => "filmy",
        }
    }

    pub fn from_key(key: &str) -> Option<Style> {
        Style::ALL.into_iter().find(|s| s.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Style::Classic => "🖤 Classic",
            Style::Akhbaar => "📰 Akhbaar",
            Style::Neon => "🌆 Neon",
            Style::Filmy => "🎬 Filmy",
        }
    }
}

const W: f32 = 1200.0;
const H: f32 = 675.0;

#[derive(Clone, Copy)]
enum Face {
    Serif,
    SerifItalic,
    Sans,
}

struct Pen<'a> {
    px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    serif: String,
    sans: String,
}

impl Pen<'_> {
    fn layout(&mut self, text: &str, size: f32, face: Face, weight: Weight, width: Option<f32>, align: Option<Align>) -> Buffer {
        let fs = &mut *self.fs;
        let mut buf = Buffer::new(fs, Metrics::new(size, (size * 1.25).round()));
        buf.set_wrap(fs, Wrap::WordOrGlyph);
        buf.set_size(fs, width, None);
        let family = match face {
            Face::Sans => self.sans.as_str(),
            _ => self.serif.as_str(),
        };
        let mut attrs = Attrs::new().family(Family::Name(family)).weight(weight);
        if matches!(face, Face::SerifItalic) {
            attrs = attrs.style(FontStyle::Italic);
        }
        buf.set_text(fs, text, attrs, Shaping::Advanced);
        if align.is_some() {
            for line in buf.lines.iter_mut() {
                line.set_align(align);
            }
        }
        buf.shape_until_scroll(fs, false);
        buf
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
    fn avatar(&mut self, bytes: Option<&[u8]>, cx: f32, cy: f32, size: f32, ring: [u8; 3], gap: [u8; 3], grey: bool, name: &str) {
        let r = size / 2.0;
        fill_circle(&mut self.px, cx, cy, r + 8.0, ring);
        fill_circle(&mut self.px, cx, cy, r + 4.0, gap);
        match avatar_pixmap(bytes, size as u32, grey) {
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
                let initial = name
                    .chars()
                    .find(|c| c.is_ascii_alphanumeric())
                    .map(|c| c.to_ascii_uppercase().to_string())
                    .unwrap_or_else(|| "?".to_string());
                let fsz = size * 0.44;
                self.centered(&initial, cx, cy + fsz * 0.36, fsz, Face::Sans, Weight::BOLD, [240, 240, 240]);
            }
        }
    }

    /// Avatar beside name and details, the whole row centred on `cy`.
    #[allow(clippy::too_many_arguments)]
    fn byline_row(&mut self, q: &Quote, name: &str, cy: f32, ring: [u8; 3], gap: [u8; 3], name_color: [u8; 3], meta_color: [u8; 3]) {
        let name = self.fit(name, 30.0, Face::Sans, Weight::EXTRA_BOLD, 720.0);
        let meta = self.fit(&meta(q), 19.0, Face::Sans, Weight::MEDIUM, 720.0);
        let text_w = self.measure(&name, 30.0, Face::Sans, Weight::EXTRA_BOLD).max(self.measure(&meta, 19.0, Face::Sans, Weight::MEDIUM));
        let x0 = W / 2.0 - (88.0 + 20.0 + text_w) / 2.0;
        self.avatar(q.avatar.as_deref(), x0 + 44.0, cy, 72.0, ring, gap, false, &q.author);
        self.line(&name, x0 + 108.0, cy - 4.0, 30.0, Face::Sans, Weight::EXTRA_BOLD, name_color);
        self.line(&meta, x0 + 108.0, cy + 28.0, 19.0, Face::Sans, Weight::MEDIUM, meta_color);
    }
}

fn meta(q: &Quote) -> String {
    let mut parts = vec![format!("@{}", q.handle)];
    if !q.channel.is_empty() {
        parts.push(q.channel.clone());
    }
    if !q.when.is_empty() {
        parts.push(q.when.clone());
    }
    parts.join("  ·  ")
}

fn sk(c: [u8; 3], a: u8) -> SkColor {
    SkColor::from_rgba8(c[0], c[1], c[2], a)
}

fn rect(px: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, c: [u8; 3]) {
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        px.fill_rect(r, &paint(c, 255), Transform::identity(), None);
    }
}

fn gradient(px: &mut Pixmap, from: (f32, f32), to: (f32, f32), a: [u8; 3], b: [u8; 3]) {
    let shader = LinearGradient::new(
        Point::from_xy(from.0, from.1),
        Point::from_xy(to.0, to.1),
        vec![GradientStop::new(0.0, sk(a, 255)), GradientStop::new(1.0, sk(b, 255))],
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(shader), Some(r)) = (shader, Rect::from_xywh(0.0, 0.0, W, H)) {
        let mut p = Paint::default();
        p.shader = shader;
        px.fill_rect(r, &p, Transform::identity(), None);
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

fn classic(p: &mut Pen<'_>, q: &Quote) {
    gradient(&mut p.px, (0.0, 0.0), (W, H), [26, 23, 38], [10, 11, 15]);
    glow(&mut p.px, 1060.0, 70.0, 460.0, [167, 139, 250], 40);
    // The mark sits above the text box, not behind it: on a long quote the
    // first line otherwise runs straight through it.
    p.line("“", 62.0, 196.0, 200.0, Face::Serif, Weight::BOLD, [86, 72, 140]);
    let text = p.paragraph(&q.text, Face::SerifItalic, Weight::NORMAL, 960.0, 320.0, 62.0, 28.0, Align::Left);
    let top = 168.0 + (320.0 - Pen::height(&text)).max(0.0) / 2.0;
    p.draw(&text, 120.0, top, [244, 245, 247]);
    rect(&mut p.px, 120.0, 520.0, 960.0, 2.0, [52, 54, 66]);
    p.avatar(q.avatar.as_deref(), 160.0, 592.0, 76.0, [167, 139, 250], [16, 16, 22], false, &q.author);
    let name = p.fit(&q.author, 34.0, Face::Sans, Weight::BOLD, 600.0);
    p.line(&name, 222.0, 586.0, 34.0, Face::Sans, Weight::BOLD, [244, 245, 247]);
    let details = p.fit(&meta(q), 20.0, Face::Sans, Weight::MEDIUM, 600.0);
    p.line(&details, 222.0, 620.0, 20.0, Face::Sans, Weight::MEDIUM, [150, 154, 166]);
    let mark = "MLCI  ·  GYAAN";
    let w = p.measure(mark, 18.0, Face::Sans, Weight::SEMIBOLD);
    p.line(mark, 1080.0 - w, 620.0, 18.0, Face::Sans, Weight::SEMIBOLD, [120, 108, 160]);
}

fn akhbaar(p: &mut Pen<'_>, q: &Quote) {
    let (paper, ink, faded) = ([243, 236, 221], [34, 28, 22], [112, 100, 86]);
    p.px.fill(sk(paper, 255));
    glow(&mut p.px, W / 2.0, H / 2.0, 900.0, [243, 236, 221], 0);
    for (inset, width) in [(22.0, 3.0), (32.0, 1.0)] {
        if let Some(border) = rrect(inset, inset, W - 2.0 * inset, H - 2.0 * inset, 2.0) {
            let stroke = Stroke { width, ..Stroke::default() };
            p.px.stroke_path(&border, &paint(ink, 255), &stroke, Transform::identity(), None);
        }
    }
    p.centered("THE MLCI TIMES", W / 2.0, 104.0, 58.0, Face::Serif, Weight::BOLD, ink);
    rect(&mut p.px, 52.0, 122.0, W - 104.0, 3.0, ink);
    rect(&mut p.px, 52.0, 129.0, W - 104.0, 1.0, ink);
    let place = if q.channel.is_empty() { "MLCI".to_string() } else { q.channel.to_uppercase() };
    let dateline = p.fit(
        &format!("{}   ·   {}   ·   PRICE: EK CUTTING CHAI", q.when.to_uppercase(), place),
        16.0,
        Face::Serif,
        Weight::NORMAL,
        1040.0,
    );
    p.centered(&dateline, W / 2.0, 154.0, 16.0, Face::Serif, Weight::NORMAL, faded);
    rect(&mut p.px, 52.0, 168.0, W - 104.0, 1.0, ink);
    let text = p.paragraph(&format!("“{}”", q.text), Face::Serif, Weight::NORMAL, 940.0, 290.0, 54.0, 26.0, Align::Center);
    let top = 188.0 + (290.0 - Pen::height(&text)) / 2.0;
    p.draw(&text, W / 2.0 - 470.0, top, ink);
    p.avatar(q.avatar.as_deref(), W / 2.0, 526.0, 62.0, ink, paper, true, &q.author);
    let by = p.fit(&format!("— {}", q.author), 30.0, Face::SerifItalic, Weight::NORMAL, 900.0);
    p.centered(&by, W / 2.0, 602.0, 30.0, Face::SerifItalic, Weight::NORMAL, ink);
    let handle = p.fit(&format!("@{}", q.handle), 17.0, Face::Serif, Weight::NORMAL, 900.0);
    p.centered(&handle, W / 2.0, 628.0, 17.0, Face::Serif, Weight::NORMAL, faded);
}

fn neon(p: &mut Pen<'_>, q: &Quote) {
    gradient(&mut p.px, (0.0, 0.0), (W, H), [44, 14, 86], [6, 34, 64]);
    glow(&mut p.px, 110.0, 90.0, 400.0, [255, 64, 170], 80);
    glow(&mut p.px, 1100.0, 610.0, 440.0, [0, 220, 255], 70);
    let text = p.paragraph(&q.text, Face::Sans, Weight::EXTRA_BOLD, 1000.0, 350.0, 64.0, 28.0, Align::Center);
    let top = 70.0 + (350.0 - Pen::height(&text)) / 2.0;
    p.draw(&text, W / 2.0 - 500.0, top, [255, 255, 255]);
    let bar = LinearGradient::new(
        Point::from_xy(W / 2.0 - 90.0, 0.0),
        Point::from_xy(W / 2.0 + 90.0, 0.0),
        vec![GradientStop::new(0.0, sk([255, 64, 170], 255)), GradientStop::new(1.0, sk([0, 220, 255], 255))],
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(shader), Some(path)) = (bar, rrect(W / 2.0 - 90.0, 452.0, 180.0, 7.0, 3.5)) {
        let mut pt = Paint::default();
        pt.shader = shader;
        pt.anti_alias = true;
        p.px.fill_path(&path, &pt, FillRule::Winding, Transform::identity(), None);
    }
    p.byline_row(q, &q.author, 556.0, [255, 64, 170], [30, 20, 60], [255, 255, 255], [180, 200, 230]);
}

fn filmy(p: &mut Pen<'_>, q: &Quote) {
    let (yellow, red) = ([255, 212, 0], [236, 44, 52]);
    p.px.fill(sk([8, 6, 6], 255));
    glow(&mut p.px, W / 2.0, H / 2.0, 720.0, [150, 10, 20], 130);
    for y in [0.0, H - 46.0] {
        rect(&mut p.px, 0.0, y, W, 46.0, [18, 18, 18]);
        let mut x = 10.0;
        while x < W {
            if let Some(hole) = rrect(x, y + 15.0, 24.0, 16.0, 4.0) {
                p.px.fill_path(&hole, &paint([70, 70, 70], 255), FillRule::Winding, Transform::identity(), None);
            }
            x += 46.0;
        }
    }
    p.centered("🎬  EK FILMY DIALOGUE", W / 2.0, 98.0, 20.0, Face::Sans, Weight::BOLD, yellow);
    let text = p.paragraph(&q.text.to_uppercase(), Face::Sans, Weight::BLACK, 1020.0, 320.0, 66.0, 28.0, Align::Center);
    let top = 126.0 + (320.0 - Pen::height(&text)) / 2.0;
    p.draw(&text, W / 2.0 - 510.0, top, yellow);
    p.byline_row(q, &format!("— {}", q.author.to_uppercase()), 548.0, yellow, [20, 10, 10], red, [190, 178, 160]);
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
    let mut pen = Pen { px: Pixmap::new(W as u32, H as u32)?, fs, cache: SwashCache::new(), serif, sans };
    match style {
        Style::Classic => classic(&mut pen, q),
        Style::Akhbaar => akhbaar(&mut pen, q),
        Style::Neon => neon(&mut pen, q),
        Style::Filmy => filmy(&mut pen, q),
    }
    pen.px.encode_png().ok()
}
