//! Biski cards: a message drawn the way Discord shows it, as a screenshot.
//!
//! Dark chat background, the message it replied to on a thin line above with
//! the little hook, then the message itself: avatar, name in the member's
//! colour, the time, the words wrapped to the width, and its first picture.
//! The height follows the content, so a one-liner is a short strip and a
//! paragraph with a photo is tall.

use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache, Weight};
use tiny_skia::{FillRule, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use super::awards_card::{avatar_pixmap, blend_rect, paint};

/// One message as it appears on the card.
pub struct Said {
    pub name: String,
    /// The member's role colour; `None` draws Discord's default near-white.
    pub colour: Option<[u8; 3]>,
    pub avatar: Option<Vec<u8>>,
    pub text: String,
    /// "19 Nov 2026, 3:14 pm", already in India time.
    pub when: String,
    /// The first picture attached, if any.
    pub picture: Option<Vec<u8>>,
}

pub struct Shot {
    pub replied_to: Option<Said>,
    pub said: Said,
}

const W: f32 = 1000.0;
const PAD: f32 = 32.0;
const AVATAR: f32 = 56.0;
const GUTTER: f32 = 20.0;
const NAME_SIZE: f32 = 22.0;
const TEXT_SIZE: f32 = 21.0;
const TEXT_LINE: f32 = 1.38;
const REPLY_SIZE: f32 = 17.0;
const REPLY_ROW: f32 = 34.0;
const PIC_MAX_W: f32 = 600.0;
const PIC_MAX_H: f32 = 420.0;

const BG: [u8; 3] = [49, 51, 56];
const NAME_DEFAULT: [u8; 3] = [242, 243, 245];
const TEXT: [u8; 3] = [219, 222, 225];
const MUTED: [u8; 3] = [148, 155, 164];
const HOOK: [u8; 3] = [78, 80, 88];

fn pick(fs: &FontSystem) -> String {
    for name in ["Noto Sans", "Montserrat", "Avenir Next"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

struct Pen<'a> {
    px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    family: String,
}

fn buffer(fs: &mut FontSystem, family: &str, text: &str, size: f32, line: f32, weight: Weight, width: Option<f32>) -> Buffer {
    let mut buf = Buffer::new(fs, Metrics::new(size, size * line));
    buf.set_size(fs, width, None);
    let attrs = Attrs::new().family(Family::Name(family)).weight(weight);
    buf.set_text(fs, text, attrs, Shaping::Advanced);
    buf.shape_until_scroll(fs, false);
    buf
}

fn height_of(buf: &Buffer, size: f32, line: f32) -> f32 {
    buf.layout_runs().count().max(1) as f32 * size * line
}

fn width_of(buf: &Buffer) -> f32 {
    buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
}

impl Pen<'_> {
    fn draw(&mut self, buf: &Buffer, x: f32, top: f32, colour: [u8; 3]) {
        let (ox, oy) = (x.round() as i32, top.round() as i32);
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        buf.draw(fs, cache, Color::rgb(colour[0], colour[1], colour[2]), |gx, gy, w, h, c| {
            blend_rect(px, ox + gx, oy + gy, w, h, c);
        });
    }

    /// One line of `text`, cut with an ellipsis to fit `max_w`; returns the width drawn.
    fn line(&mut self, text: &str, x: f32, top: f32, size: f32, weight: Weight, colour: [u8; 3], max_w: f32) -> f32 {
        let fitted = fit(self.fs, &self.family, text, size, weight, max_w);
        let buf = buffer(self.fs, &self.family, &fitted, size, 1.3, weight, None);
        let w = width_of(&buf);
        self.draw(&buf, x, top, colour);
        w
    }

    fn avatar(&mut self, bytes: Option<&[u8]>, x: f32, y: f32, size: f32) {
        match avatar_pixmap(bytes, size as u32, false) {
            Some(pm) => self.px.draw_pixmap(x as i32, y as i32, pm.as_ref(), &PixmapPaint::default(), Transform::identity(), None),
            None => {
                if let Some(c) = PathBuilder::from_circle(x + size / 2.0, y + size / 2.0, size / 2.0) {
                    self.px.fill_path(&c, &paint([88, 101, 242], 255), FillRule::Winding, Transform::identity(), None);
                }
            }
        }
    }
}

fn fit(fs: &mut FontSystem, family: &str, text: &str, size: f32, weight: Weight, max_w: f32) -> String {
    let one_line: String = text.split('\n').next().unwrap_or("").to_string();
    let cut = one_line.len() < text.trim_end().len();
    let measure = |fs: &mut FontSystem, t: &str| width_of(&buffer(fs, family, t, size, 1.3, weight, None));
    if !cut && measure(fs, &one_line) <= max_w {
        return one_line;
    }
    let mut chars: Vec<char> = one_line.chars().collect();
    loop {
        let t = format!("{}…", chars.iter().collect::<String>().trim_end());
        if chars.is_empty() || measure(fs, &t) <= max_w {
            return t;
        }
        // Long lines: drop a tenth at a time, then one by one near the end.
        let drop = (chars.len() / 10).max(1);
        chars.truncate(chars.len() - drop);
    }
}

/// A picture scaled to fit the box, as a pixmap.
fn picture(bytes: &[u8]) -> Option<Pixmap> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width() as f32, img.height() as f32);
    if w < 1.0 || h < 1.0 {
        return None;
    }
    let scale = (PIC_MAX_W / w).min(PIC_MAX_H / h).min(1.0);
    let (tw, th) = ((w * scale).round().max(1.0) as u32, (h * scale).round().max(1.0) as u32);
    let rgba = img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3).to_rgba8();
    let mut pm = Pixmap::new(tw, th)?;
    for (dst, p) in pm.pixels_mut().iter_mut().zip(rgba.pixels()) {
        *dst = tiny_skia::ColorU8::from_rgba(p[0], p[1], p[2], p[3]).premultiply();
    }
    Some(pm)
}

/// Draw `shot` and return PNG bytes.
pub fn render(shot: &Shot, fs: &mut FontSystem) -> Option<Vec<u8>> {
    let family = pick(fs);
    let x0 = PAD + AVATAR + GUTTER;
    let text_w = W - x0 - PAD;

    // Lay out first: the height depends on the words and the picture.
    let text = buffer(fs, &family, &shot.said.text, TEXT_SIZE, TEXT_LINE, Weight::NORMAL, Some(text_w));
    let text_h = if shot.said.text.trim().is_empty() { 0.0 } else { height_of(&text, TEXT_SIZE, TEXT_LINE) };
    let pic = shot.said.picture.as_deref().and_then(picture);
    let pic_h = pic.as_ref().map(|p| p.height() as f32 + 10.0).unwrap_or(0.0);
    let reply_h = if shot.replied_to.is_some() { REPLY_ROW } else { 0.0 };
    let body_h = (NAME_SIZE * 1.5 + text_h + pic_h).max(AVATAR);
    let h = PAD + reply_h + body_h + PAD;

    let mut pen = Pen { px: Pixmap::new(W as u32, h.ceil() as u32)?, fs, cache: SwashCache::new(), family };
    pen.px.fill(tiny_skia::Color::from_rgba8(BG[0], BG[1], BG[2], 255));

    let mut y = PAD;
    if let Some(r) = &shot.replied_to {
        // The hook: from above the main avatar's centre, up and right to the reply line.
        let cx = PAD + AVATAR / 2.0;
        let mid = y + REPLY_ROW / 2.0;
        let mut pb = PathBuilder::new();
        pb.move_to(cx, y + REPLY_ROW + 2.0);
        pb.line_to(cx, mid + 8.0);
        pb.quad_to(cx, mid, cx + 8.0, mid);
        pb.line_to(x0 - 6.0, mid);
        if let Some(path) = pb.finish() {
            let stroke = Stroke { width: 2.5, ..Stroke::default() };
            pen.px.stroke_path(&path, &paint(HOOK, 255), &stroke, Transform::identity(), None);
        }
        let small = 22.0;
        pen.avatar(r.avatar.as_deref(), x0, mid - small / 2.0, small);
        let mut x = x0 + small + 8.0;
        let top = mid - REPLY_SIZE * 0.68;
        let name_w = pen.line(&format!("@{}", r.name), x, top, REPLY_SIZE, Weight::SEMIBOLD, r.colour.unwrap_or(NAME_DEFAULT), 260.0);
        x += name_w + 8.0;
        let snippet = if r.text.trim().is_empty() {
            if r.picture.is_some() { "🖼️ GIF / picture".to_string() } else { "…".to_string() }
        } else {
            r.text.replace('\n', " ")
        };
        pen.line(&snippet, x, top, REPLY_SIZE, Weight::NORMAL, MUTED, W - PAD - x);
        y += REPLY_ROW;
    }

    let s = &shot.said;
    pen.avatar(s.avatar.as_deref(), PAD, y, AVATAR);
    let name_w = pen.line(&s.name, x0, y - 2.0, NAME_SIZE, Weight::SEMIBOLD, s.colour.unwrap_or(NAME_DEFAULT), text_w * 0.6);
    pen.line(&s.when, x0 + name_w + 12.0, y + 4.0, 15.0, Weight::NORMAL, MUTED, text_w * 0.4);
    let mut cy = y + NAME_SIZE * 1.5;
    if text_h > 0.0 {
        pen.draw(&text, x0, cy, TEXT);
        cy += text_h;
    }
    if let Some(p) = pic {
        pen.px.draw_pixmap(x0 as i32, (cy + 10.0) as i32, p.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    pen.px.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Said {
        Said { name: "Loduchand".into(), colour: Some([235, 69, 158]), avatar: None, text: text.into(), when: "19 Nov 2026, 3:14 pm".into(), picture: None }
    }

    fn size_of(png: &[u8]) -> (u32, u32) {
        let img = image::load_from_memory(png).unwrap();
        (img.width(), img.height())
    }

    #[test]
    fn a_reply_and_a_long_message_make_a_taller_card() {
        let mut fs = FontSystem::new();
        let short = render(&Shot { replied_to: None, said: said("hi") }, &mut fs).unwrap();
        let long = render(
            &Shot { replied_to: Some(said("what did you say")), said: said(&"bahut lamba message hai yaar ".repeat(30)) },
            &mut fs,
        )
        .unwrap();
        let (w1, h1) = size_of(&short);
        let (w2, h2) = size_of(&long);
        assert_eq!((w1, w2), (W as u32, W as u32));
        assert!(h2 > h1 + 60, "short {} long {}", h1, h2);
    }

    #[test]
    fn a_picture_is_drawn_below_and_kept_inside_its_box() {
        let mut big = image::RgbImage::new(2000, 1000);
        big.fill(200);
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(big).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let p = picture(&png).unwrap();
        assert!(p.width() as f32 <= PIC_MAX_W && p.height() as f32 <= PIC_MAX_H);
        let mut s = said("");
        s.picture = Some(png);
        let mut fs = FontSystem::new();
        let (_, h) = size_of(&render(&Shot { replied_to: None, said: s }, &mut fs).unwrap());
        assert!(h as f32 >= PAD * 2.0 + p.height() as f32);
    }
}

#[cfg(test)]
mod preview {
    use super::*;
    #[test]
    #[ignore]
    fn write_preview() {
        let out = std::env::var("BISKI_PREVIEW").unwrap();
        let mut pic = image::RgbImage::new(800, 500);
        for (x, y, p) in pic.enumerate_pixels_mut() { *p = image::Rgb([(x / 4) as u8, (y / 3) as u8, 180]); }
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(pic).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let shot = Shot {
            replied_to: Some(Said { name: "Tillu".into(), colour: Some([241, 196, 15]), avatar: None, text: "bhai tu har baat pe kalesh kyun karta hai, seriously, sab log thak gaye hai".into(), when: String::new(), picture: None }),
            said: Said { name: "Papi Snooty Booty".into(), colour: Some([235, 69, 158]), avatar: None, text: "kalesh main nahi karta, kalesh mujhe dhoondta hai 😌\nanyway ye dekho".into(), when: "19 Nov 2026, 3:14 pm".into(), picture: Some(png) },
        };
        let mut fs = FontSystem::new();
        std::fs::write(out, render(&shot, &mut fs).unwrap()).unwrap();
    }
}
