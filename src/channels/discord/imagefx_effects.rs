//! The pixel work: one function per effect, all of them pure.
//!
//! Every one of these takes an `RgbaImage` and hands back an `RgbaImage`, with
//! no clock, no settings and no Discord in sight, which is what makes them
//! testable one at a time. The frame handling, the caps and the encoding are
//! `imagefx_engine`'s job; `magik` is big enough to live in `imagefx_seam`.
//!
//! Two rules run through the lot:
//!
//! * **A one-pixel picture must not panic.** Every loop that looks at a
//!   neighbour clamps, every divisor is forced above zero, every target size is
//!   clamped to at least 1. The tests hammer 1x1 for exactly this reason.
//! * **Transparency survives.** Anything that resamples does it on
//!   premultiplied colour and unpremultiplies afterwards, so a transparent PNG
//!   comes back with clean edges instead of a grey halo.

use image::imageops::FilterType;
use image::{Rgba, RgbaImage};

// --- small helpers -----------------------------------------------------------------------------

fn luma_of(p: &Rgba<u8>) -> f32 {
    0.299 * p.0[0] as f32 + 0.587 * p.0[1] as f32 + 0.114 * p.0[2] as f32
}

const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

/// What a warp does about a sample that lands outside the picture.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outside {
    /// Nothing there - right for a turn, where the corner genuinely is empty.
    Clear,
    /// The nearest edge pixel, smeared outwards. Right for a swirl or a bulge,
    /// where a hole in the corner reads as a bug rather than an effect.
    Edge,
}

/// One pixel, sampled between four, on premultiplied colour so the edge of a
/// transparent region does not bleed.
fn sample(img: &RgbaImage, fx: f32, fy: f32) -> Rgba<u8> {
    sample_with(img, fx, fy, Outside::Clear)
}

fn sample_with(img: &RgbaImage, fx: f32, fy: f32, outside: Outside) -> Rgba<u8> {
    let (w, h) = (img.width() as i64, img.height() as i64);
    if !fx.is_finite() || !fy.is_finite() {
        return CLEAR;
    }
    let (fx, fy) = match outside {
        Outside::Edge => (fx.clamp(0.0, (w - 1) as f32), fy.clamp(0.0, (h - 1) as f32)),
        Outside::Clear => (fx, fy),
    };
    if fx < -0.5 || fy < -0.5 || fx > w as f32 - 0.5 || fy > h as f32 - 0.5 {
        return CLEAR;
    }
    let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let get = |x: i64, y: i64| {
        let x = x.clamp(0, w - 1) as u32;
        let y = y.clamp(0, h - 1) as u32;
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, p[3] as f32]
    };
    let (a, b, c, d) = (get(x0, y0), get(x0 + 1, y0), get(x0, y0 + 1), get(x0 + 1, y0 + 1));
    let mut out = [0f32; 4];
    for i in 0..4 {
        let top = a[i] + (b[i] - a[i]) * tx;
        let bottom = c[i] + (d[i] - c[i]) * tx;
        out[i] = top + (bottom - top) * ty;
    }
    let alpha = out[3].clamp(0.0, 255.0);
    if alpha < 0.5 {
        return CLEAR;
    }
    let un = 255.0 / alpha;
    Rgba([
        (out[0] * un).clamp(0.0, 255.0) as u8,
        (out[1] * un).clamp(0.0, 255.0) as u8,
        (out[2] * un).clamp(0.0, 255.0) as u8,
        alpha as u8,
    ])
}

/// Every output pixel from a function that says where in the input to look.
/// `at(x, y) -> (fx, fy)` in input coordinates.
fn warp(img: &RgbaImage, outside: Outside, at: impl Fn(f32, f32) -> (f32, f32)) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = at(x as f32 + 0.5, y as f32 + 0.5);
            out.put_pixel(x, y, sample_with(img, fx - 0.5, fy - 0.5, outside));
        }
    }
    out
}

/// A tiny deterministic generator. Glitch has to look random and be the same
/// every time, or a test can't say what it produced and a gif's frames can't
/// be made to ramp on purpose.
pub struct Roll(u64);

impl Roll {
    pub fn new(seed: u64) -> Roll {
        Roll(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// 0..n, and 0 when n is 0 so no caller has to check.
    pub fn upto(&mut self, n: u32) -> u32 {
        if n == 0 { 0 } else { (self.next() % u64::from(n)) as u32 }
    }

    /// -n..=n.
    pub fn about(&mut self, n: i32) -> i32 {
        if n <= 0 { 0 } else { self.upto(n as u32 * 2 + 1) as i32 - n }
    }
}

// --- colour ------------------------------------------------------------------------------------

pub fn invert(img: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        Rgba([255 - p[0], 255 - p[1], 255 - p[2], p[3]])
    })
}

pub fn grayscale(img: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        let g = luma_of(p).clamp(0.0, 255.0) as u8;
        Rgba([g, g, g, p.0[3]])
    })
}

fn to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (if h < 0.0 { h + 360.0 } else { h }, if max <= f32::EPSILON { 0.0 } else { d / max }, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    (r + m, g + m, b + m)
}

/// Turn the colour wheel. `degrees` is how far round.
pub fn hue(img: &RgbaImage, degrees: f32) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let (h, s, v) = to_hsv(p[0] as f32, p[1] as f32, p[2] as f32);
        let (r, g, b) = from_hsv(h + degrees, s, v);
        Rgba([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, p[3]])
    })
}

/// More colour, by pushing every channel away from its own grey.
pub fn saturate(img: &RgbaImage, by: f32) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        let g = luma_of(p);
        let ch = |i: usize| (g + (p.0[i] as f32 - g) * by).clamp(0.0, 255.0) as u8;
        Rgba([ch(0), ch(1), ch(2), p.0[3]])
    })
}

/// More contrast, by pushing every channel away from mid grey.
pub fn contrast(img: &RgbaImage, by: f32) -> RgbaImage {
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let ch = |i: usize| (128.0 + (p[i] as f32 - 128.0) * by).clamp(0.0, 255.0) as u8;
        Rgba([ch(0), ch(1), ch(2), p[3]])
    })
}

/// Discord's own two-and-a-half tones: dark blurple in the shadows, blurple
/// through the middle, white at the top.
pub fn blurple(img: &RgbaImage) -> RgbaImage {
    const STOPS: [[f32; 3]; 3] = [[35.0, 39.0, 92.0], [88.0, 101.0, 242.0], [255.0, 255.0, 255.0]];
    // A dark picture would otherwise land entirely in the bottom third of the
    // ramp and come out one flat blue. Stretch the brightness across the whole
    // range first, so the highlights actually reach white.
    let (lo, hi) = levels(img);
    let span = (hi - lo).max(1.0);
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y);
        let t = ((luma_of(p) - lo) / span).clamp(0.0, 1.0) * 2.0;
        let (lo, f) = if t < 1.0 { (0, t) } else { (1, t - 1.0) };
        let ch = |i: usize| (STOPS[lo][i] + (STOPS[lo + 1][i] - STOPS[lo][i]) * f).clamp(0.0, 255.0) as u8;
        Rgba([ch(0), ch(1), ch(2), p.0[3]])
    })
}

/// The darkest and lightest the picture actually gets, ignoring the very ends
/// so one stray white pixel cannot decide the whole range.
fn levels(img: &RgbaImage) -> (f32, f32) {
    let mut hist = [0u32; 256];
    let mut seen = 0u32;
    for p in img.pixels().filter(|p| p.0[3] > 8) {
        hist[(luma_of(p).clamp(0.0, 255.0) as usize).min(255)] += 1;
        seen += 1;
    }
    if seen == 0 {
        return (0.0, 255.0);
    }
    let edge = (seen / 100).max(1);
    let mut run = 0u32;
    let mut lo = 0usize;
    for (v, n) in hist.iter().enumerate() {
        run += n;
        if run >= edge {
            lo = v;
            break;
        }
    }
    let mut run = 0u32;
    let mut hi = 255usize;
    for (v, n) in hist.iter().enumerate().rev() {
        run += n;
        if run >= edge {
            hi = v;
            break;
        }
    }
    if hi <= lo { (0.0, 255.0) } else { (lo as f32, hi as f32) }
}

/// Unsharp masking, done by hand so it works on the alpha-bearing buffer the
/// rest of this module passes around.
pub fn sharpen(img: &RgbaImage, amount: f32) -> RgbaImage {
    let blurred = image::imageops::blur(img, 1.6);
    RgbaImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        let b = blurred.get_pixel(x, y).0;
        let ch = |i: usize| (p[i] as f32 + (p[i] as f32 - b[i] as f32) * amount).clamp(0.0, 255.0) as u8;
        Rgba([ch(0), ch(1), ch(2), p[3]])
    })
}

/// A JPEG round trip at a chosen quality: the whole point is the damage. A
/// failed encode or decode gives the picture back untouched rather than an
/// error, since this only ever makes something uglier.
pub fn crush(img: &RgbaImage, quality: u8) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
    }
    // JPEG has no alpha: lay the picture on black first so the transparent
    // parts come back as something rather than as noise.
    let flat = image::RgbImage::from_fn(w, h, |x, y| {
        let p = img.get_pixel(x, y).0;
        let a = p[3] as f32 / 255.0;
        image::Rgb([(p[0] as f32 * a) as u8, (p[1] as f32 * a) as u8, (p[2] as f32 * a) as u8])
    });
    let mut bytes: Vec<u8> = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality.clamp(1, 100));
    if enc.encode(flat.as_raw(), w, h, image::ExtendedColorType::Rgb8).is_err() {
        return img.clone();
    }
    match image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg) {
        Ok(back) => {
            let rgb = back.to_rgba8();
            // Keep the original alpha: crushing should not fill a hole in.
            RgbaImage::from_fn(w, h, |x, y| {
                let mut p = *rgb.get_pixel(x.min(rgb.width() - 1), y.min(rgb.height() - 1));
                p.0[3] = img.get_pixel(x, y).0[3];
                p
            })
        }
        Err(_) => img.clone(),
    }
}

/// Crushed, blown out and sharpened until it hurts. Strength decides how far.
pub fn deepfry(img: &RgbaImage, strength: u8) -> RgbaImage {
    let s = strength.clamp(1, 10) as f32 / 10.0;
    let out = saturate(img, 1.1 + 2.0 * s);
    let out = contrast(&out, 1.05 + 0.9 * s);
    let out = sharpen(&out, 0.4 + 1.8 * s);
    let out = crush(&out, (34.0 - 30.0 * s).round().max(1.0) as u8);
    let out = saturate(&out, 1.1 + 0.8 * s);
    let out = contrast(&out, 1.02 + 0.38 * s);
    crush(&out, (24.0 - 21.0 * s).round().max(1.0) as u8)
}

/// How hard a `jpeg` of this strength crushes. Ten is unreadable.
pub fn jpeg_quality(strength: u8) -> u8 {
    let s = strength.clamp(1, 10) as i32;
    // Five, the middle, has to be obviously crushed on sight: a picture that
    // has already been through a camera's JPEG barely shows quality 25.
    (22 - s * 2).clamp(1, 100) as u8
}

/// How many round trips a strength asks for. One pass at a low quality is
/// surprisingly tidy; the blocks and the ringing that people mean by "jpeg"
/// come from re-encoding an already-damaged picture, each pass compounding the
/// last.
pub fn jpeg_passes(strength: u8) -> usize {
    1 + strength.clamp(1, 10) as usize / 3
}

/// Crushed over and over.
pub fn crush_times(img: &RgbaImage, quality: u8, times: usize) -> RgbaImage {
    let mut out = img.clone();
    for _ in 0..times.max(1) {
        out = crush(&out, quality);
    }
    out
}

// --- shape -------------------------------------------------------------------------------------

/// Nearest-neighbour blocks. `block` is the size of one, in pixels.
pub fn pixelate(img: &RgbaImage, block: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let block = block.max(1);
    let small_w = (w / block).max(1);
    let small_h = (h / block).max(1);
    let small = image::imageops::resize(img, small_w, small_h, FilterType::Triangle);
    image::imageops::resize(&small, w, h, FilterType::Nearest)
}

/// The block size a strength asks for, on a picture this big.
pub fn pixel_block(w: u32, h: u32, strength: u8) -> u32 {
    let side = w.max(h).max(1) as f32;
    let s = strength.clamp(1, 10) as f32;
    ((side * (0.006 + 0.009 * s)).round() as u32).max(2)
}

/// Turn the middle and leave the rim alone.
pub fn swirl(img: &RgbaImage, strength: u8) -> RgbaImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r_max = (cx * cx + cy * cy).sqrt().max(1.0);
    let turn = strength.clamp(1, 10) as f32 * 0.55;
    warp(img, Outside::Edge, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        let a = dy.atan2(dx) + turn * (1.0 - r / r_max).max(0.0);
        (cx + r * a.cos(), cy + r * a.sin())
    })
}

/// Push the middle out (`out`) or suck it in. NotSoBot calls these explode and
/// implode.
pub fn bulge(img: &RgbaImage, strength: u8, out: bool) -> RgbaImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (cx, cy) = (w / 2.0, h / 2.0);
    let r_max = (cx * cx + cy * cy).sqrt().max(1.0);
    let s = strength.clamp(1, 10) as f32 / 10.0;
    // Above one the middle is magnified; below one it is pulled in.
    let power = if out { 1.0 + 1.6 * s } else { 1.0 / (1.0 + 1.6 * s) };
    warp(img, Outside::Edge, |x, y| {
        let (dx, dy) = (x - cx, y - cy);
        let r = (dx * dx + dy * dy).sqrt();
        if r < 0.001 {
            return (cx, cy);
        }
        let rr = r_max * (r / r_max).powf(power);
        (cx + dx / r * rr, cy + dy / r * rr)
    })
}

/// A ripple down the picture.
pub fn wave(img: &RgbaImage, strength: u8) -> RgbaImage {
    let h = img.height() as f32;
    let s = strength.clamp(1, 10) as f32;
    let amp = (img.width() as f32 * 0.012 * s).max(1.0);
    let len = (h / (1.0 + s * 0.35)).max(2.0);
    warp(img, Outside::Edge, |x, y| (x + amp * (y / len * std::f32::consts::TAU).sin(), y))
}

/// Torn scan lines, shifted channels and the odd stolen block. `seed` makes it
/// repeatable, which is what lets `glitchgif` ramp it frame by frame.
pub fn glitch(img: &RgbaImage, strength: u8, seed: u64) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return img.clone();
    }
    let s = strength.clamp(1, 10) as u32;
    let mut roll = Roll::new(seed);
    let mut out = img.clone();

    // Slices of rows slid sideways.
    let slices = (s * 2).max(2);
    for _ in 0..slices {
        let top = roll.upto(h);
        let tall = 1 + roll.upto((h / 12).max(1));
        let shift = roll.about((w as i32 * s as i32 / 40).max(1));
        for y in top..(top + tall).min(h) {
            for x in 0..w {
                let from = (x as i32 - shift).rem_euclid(w as i32) as u32;
                out.put_pixel(x, y, *img.get_pixel(from, y));
            }
        }
    }

    // Blocks lifted from somewhere else in the picture.
    for _ in 0..s {
        let bw = 1 + roll.upto((w / 4).max(1));
        let bh = 1 + roll.upto((h / 8).max(1));
        let (sx, sy) = (roll.upto(w), roll.upto(h));
        let (dx, dy) = (roll.upto(w), roll.upto(h));
        for y in 0..bh {
            for x in 0..bw {
                let (from_x, from_y) = ((sx + x).min(w - 1), (sy + y).min(h - 1));
                let (to_x, to_y) = (dx + x, dy + y);
                if to_x < w && to_y < h {
                    out.put_pixel(to_x, to_y, *img.get_pixel(from_x, from_y));
                }
            }
        }
    }

    // And the channels pulled apart, which is what makes it read as broken
    // rather than merely shuffled.
    let off = (w as i32 * s as i32 / 220).max(1);
    let shifted = out.clone();
    for y in 0..h {
        for x in 0..w {
            let r = shifted.get_pixel((x as i32 - off).clamp(0, w as i32 - 1) as u32, y).0[0];
            let b = shifted.get_pixel((x as i32 + off).clamp(0, w as i32 - 1) as u32, y).0[2];
            let p = out.get_pixel_mut(x, y);
            p.0[0] = r;
            p.0[2] = b;
        }
    }
    out
}

/// Sobel: the outline, white on black.
pub fn edges(img: &RgbaImage) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let l = |x: i64, y: i64| {
        let x = x.clamp(0, w as i64 - 1) as u32;
        let y = y.clamp(0, h as i64 - 1) as u32;
        luma_of(img.get_pixel(x, y))
    };
    RgbaImage::from_fn(w, h, |x, y| {
        let (x, y) = (x as i64, y as i64);
        let gx = -l(x - 1, y - 1) - 2.0 * l(x - 1, y) - l(x - 1, y + 1) + l(x + 1, y - 1) + 2.0 * l(x + 1, y) + l(x + 1, y + 1);
        let gy = -l(x - 1, y - 1) - 2.0 * l(x, y - 1) - l(x + 1, y - 1) + l(x - 1, y + 1) + 2.0 * l(x, y + 1) + l(x + 1, y + 1);
        let g = ((gx * gx + gy * gy).sqrt() * 0.9).clamp(0.0, 255.0) as u8;
        Rgba([g, g, g, 255])
    })
}

/// Squashed or stretched, by a factor on each side.
pub fn stretch(img: &RgbaImage, fx: f32, fy: f32) -> RgbaImage {
    let w = ((img.width() as f32 * fx).round() as u32).max(1);
    let h = ((img.height() as f32 * fy).round() as u32).max(1);
    image::imageops::resize(img, w, h, FilterType::Lanczos3)
}

/// Which half gets copied onto the other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Half {
    /// The left onto the right - NotSoBot's `haah`.
    Left,
    Right,
    Top,
    Bottom,
}

pub fn mirror(img: &RgbaImage, half: Half) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    RgbaImage::from_fn(w, h, |x, y| match half {
        Half::Left => *img.get_pixel(if x < w / 2 { x } else { (w - 1 - x).min(w - 1) }, y),
        Half::Right => *img.get_pixel(if x >= w / 2 { x } else { (w - 1 - x).min(w - 1) }, y),
        Half::Top => *img.get_pixel(x, if y < h / 2 { y } else { (h - 1 - y).min(h - 1) }),
        Half::Bottom => *img.get_pixel(x, if y >= h / 2 { y } else { (h - 1 - y).min(h - 1) }),
    })
}

/// Turned by any angle, on a canvas of the caller's choosing, with nothing
/// where the picture is not. A fixed canvas is what keeps a `spin` from
/// wobbling frame to frame.
pub fn rotate_on(img: &RgbaImage, degrees: f32, out_w: u32, out_h: u32) -> RgbaImage {
    let (w, h) = (img.width() as f32, img.height() as f32);
    let (ow, oh) = (out_w.max(1), out_h.max(1));
    let (cx, cy) = (w / 2.0, h / 2.0);
    let (ocx, ocy) = (ow as f32 / 2.0, oh as f32 / 2.0);
    let (s, c) = (-degrees.to_radians()).sin_cos();
    let mut out = RgbaImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let (dx, dy) = (x as f32 + 0.5 - ocx, y as f32 + 0.5 - ocy);
            let fx = cx + dx * c - dy * s;
            let fy = cy + dx * s + dy * c;
            out.put_pixel(x, y, sample(img, fx - 0.5, fy - 0.5));
        }
    }
    out
}

/// The canvas a turned picture needs so no corner is cut off.
pub fn turned_box(w: u32, h: u32, degrees: f32) -> (u32, u32) {
    let (s, c) = degrees.to_radians().sin_cos();
    // cos(90 degrees) in f32 is not zero but four hundred-millionths, which
    // rounds a quarter turn of a 30x10 picture up to 11x30 instead of 10x30.
    // Anything that small is zero.
    let snap = |v: f32| if v.abs() < 1e-4 { 0.0 } else { v.abs() };
    let (s, c) = (snap(s), snap(c));
    let bw = (w as f32 * c + h as f32 * s).ceil().max(1.0);
    let bh = (w as f32 * s + h as f32 * c).ceil().max(1.0);
    (bw as u32, bh as u32)
}

/// Turned, on a canvas just big enough.
pub fn rotate(img: &RgbaImage, degrees: f32) -> RgbaImage {
    let (bw, bh) = turned_box(img.width(), img.height(), degrees);
    rotate_on(img, degrees, bw, bh)
}

/// The middle of the picture, blown up to the size it was: one `zoom` frame.
pub fn zoomed(img: &RgbaImage, factor: f32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let f = factor.max(1.0);
    let cw = ((w as f32 / f).round() as u32).clamp(1, w);
    let ch = ((h as f32 / f).round() as u32).clamp(1, h);
    let crop = image::imageops::crop_imm(img, (w - cw) / 2, (h - ch) / 2, cw, ch).to_image();
    image::imageops::resize(&crop, w, h, FilterType::Lanczos3)
}

/// The picture nudged, with the edge smeared out rather than left empty: one
/// `shake` frame.
pub fn shifted(img: &RgbaImage, dx: i32, dy: i32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    RgbaImage::from_fn(w, h, |x, y| {
        let sx = (x as i32 - dx).clamp(0, w as i32 - 1) as u32;
        let sy = (y as i32 - dy).clamp(0, h as i32 - 1) as u32;
        *img.get_pixel(sx, sy)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_image(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([(x * 7 % 256) as u8, (y * 11 % 256) as u8, ((x + y) * 3 % 256) as u8, 255]))
    }

    /// The whole reason `sample` premultiplies: a hole must stay a hole and
    /// must not smear grey into the colour beside it.
    #[test]
    fn a_warp_keeps_a_transparent_hole_transparent() {
        let img = RgbaImage::from_fn(40, 40, |x, _| Rgba([220, 40, 40, if x < 20 { 0 } else { 255 }]));
        let out = swirl(&img, 3);
        assert!(out.pixels().any(|p| p.0[3] == 0), "the hole was filled in");
        for p in out.pixels().filter(|p| p.0[3] > 200) {
            assert!(p.0[0] > 120, "the colour was washed out by the hole beside it: {:?}", p.0);
        }
    }

    #[test]
    fn nothing_panics_on_a_single_pixel() {
        let one = RgbaImage::from_pixel(1, 1, Rgba([10, 200, 30, 255]));
        for out in [
            invert(&one),
            grayscale(&one),
            hue(&one, 120.0),
            blurple(&one),
            saturate(&one, 2.0),
            contrast(&one, 2.0),
            sharpen(&one, 2.0),
            crush(&one, 3),
            deepfry(&one, 10),
            pixelate(&one, 40),
            swirl(&one, 10),
            bulge(&one, 10, true),
            bulge(&one, 10, false),
            wave(&one, 10),
            glitch(&one, 10, 7),
            edges(&one),
            mirror(&one, Half::Left),
            zoomed(&one, 4.0),
            shifted(&one, 9, -9),
        ] {
            assert_eq!((out.width(), out.height()), (1, 1));
        }
        assert_eq!(stretch(&one, 2.0, 0.5).dimensions(), (2, 1));
        assert!(rotate(&one, 37.0).width() >= 1);
    }

    #[test]
    fn grayscale_has_no_colour_left_and_keeps_alpha() {
        let img = RgbaImage::from_fn(8, 8, |x, _| Rgba([255, 0, 0, if x == 0 { 0 } else { 255 }]));
        let out = grayscale(&img);
        assert!(out.pixels().all(|p| p.0[0] == p.0[1] && p.0[1] == p.0[2]));
        assert_eq!(out.get_pixel(0, 0).0[3], 0);
    }

    #[test]
    fn inverting_twice_is_the_original() {
        let img = sample_image(16, 12);
        assert_eq!(invert(&invert(&img)), img);
    }

    #[test]
    fn a_full_turn_of_the_hue_comes_back_to_itself() {
        let img = sample_image(12, 12);
        let out = hue(&img, 360.0);
        for (a, b) in img.pixels().zip(out.pixels()) {
            for i in 0..3 {
                assert!((a.0[i] as i32 - b.0[i] as i32).abs() <= 2, "{:?} vs {:?}", a.0, b.0);
            }
        }
    }

    #[test]
    fn blurple_is_blurple() {
        let img = RgbaImage::from_pixel(4, 4, Rgba([128, 128, 128, 255]));
        let p = blurple(&img).get_pixel(0, 0).0;
        assert!(p[2] > p[0] && p[2] > p[1], "the mid tone should be blurple, got {:?}", p);
    }

    /// Crushing has to visibly damage the picture, or the effect is a lie.
    #[test]
    fn crushing_actually_damages_the_picture() {
        let img = sample_image(64, 64);
        let out = crush(&img, 2);
        let moved = img.pixels().zip(out.pixels()).filter(|(a, b)| a.0[..3] != b.0[..3]).count();
        assert!(moved > 64 * 64 / 4, "quality 2 barely changed it: {}", moved);
        assert!(jpeg_quality(1) > jpeg_quality(10), "strength should mean more crushing");
    }

    /// What deepfrying does, measurably: it drives the channels to the ends of
    /// the scale. A gentle gradient goes in and a clipped, blown-out thing
    /// comes out, which is the whole joke.
    #[test]
    fn deepfrying_blows_the_channels_out_to_the_ends() {
        let img = RgbaImage::from_fn(48, 48, |x, y| {
            let v = (90 + (x + y) % 70) as u8;
            Rgba([v, v.saturating_sub(20), v / 2, 255])
        });
        let clipped = |i: &RgbaImage| i.pixels().flat_map(|p| p.0[..3].iter().copied()).filter(|v| *v < 8 || *v > 247).count();
        let before = clipped(&img);
        let after = clipped(&deepfry(&img, 10));
        assert!(after > before + 100, "deepfry left it tame: {} clipped channels, was {}", after, before);
        // And gently is gentler than hard.
        assert!(clipped(&deepfry(&img, 1)) < after, "strength should matter");
    }

    #[test]
    fn edges_finds_the_edge_and_nothing_else() {
        let img = RgbaImage::from_fn(32, 32, |x, _| if x < 16 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) });
        let out = edges(&img);
        assert!(out.get_pixel(15, 16).0[0] > 120, "the edge is not lit");
        assert!(out.get_pixel(4, 16).0[0] < 20, "the flat part is not dark");
    }

    #[test]
    fn a_mirror_makes_the_two_halves_match() {
        let img = sample_image(20, 20);
        let out = mirror(&img, Half::Left);
        for y in 0..20 {
            assert_eq!(out.get_pixel(3, y), out.get_pixel(16, y));
        }
        assert_eq!(mirror(&img, Half::Left).dimensions(), (20, 20));
        for half in [Half::Left, Half::Right, Half::Top, Half::Bottom] {
            assert_eq!(mirror(&img, half).dimensions(), (20, 20));
        }
    }

    #[test]
    fn the_four_mirrors_are_four_different_pictures() {
        let img = sample_image(24, 24);
        let all: Vec<RgbaImage> = [Half::Left, Half::Right, Half::Top, Half::Bottom].iter().map(|h| mirror(&img, *h)).collect();
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "mirrors {} and {} came out the same", i, j);
            }
        }
    }

    #[test]
    fn a_quarter_turn_swaps_the_sides() {
        let img = sample_image(30, 10);
        assert_eq!(turned_box(30, 10, 90.0), (10, 30));
        assert_eq!(rotate(&img, 90.0).dimensions(), (10, 30));
        assert_eq!(rotate(&img, 180.0).dimensions(), (30, 10));
    }

    #[test]
    fn the_same_seed_glitches_the_same_way_and_a_different_one_does_not() {
        let img = sample_image(48, 48);
        assert_eq!(glitch(&img, 5, 11), glitch(&img, 5, 11));
        assert_ne!(glitch(&img, 5, 11), glitch(&img, 5, 12));
    }

    #[test]
    fn pixelating_leaves_flat_blocks() {
        let img = sample_image(64, 64);
        let out = pixelate(&img, 16);
        assert_eq!(out.get_pixel(1, 1), out.get_pixel(6, 6), "the block is not flat");
        assert!(pixel_block(800, 800, 10) > pixel_block(800, 800, 1));
        assert!(pixel_block(1, 1, 5) >= 2);
    }

    #[test]
    fn zoom_and_shake_keep_the_frame_size() {
        let img = sample_image(40, 30);
        assert_eq!(zoomed(&img, 2.5).dimensions(), (40, 30));
        assert_eq!(shifted(&img, 5, -3).dimensions(), (40, 30));
        assert_ne!(zoomed(&img, 2.5), img);
    }

    #[test]
    fn stretching_is_the_size_asked_for() {
        let img = sample_image(50, 40);
        assert_eq!(stretch(&img, 2.0, 1.0).dimensions(), (100, 40));
        assert_eq!(stretch(&img, 1.0, 2.0).dimensions(), (50, 80));
        assert_eq!(stretch(&img, 0.001, 0.001).dimensions(), (1, 1));
    }

    /// Explode and implode must pull opposite ways, or one of them is broken.
    #[test]
    fn explode_and_implode_disagree() {
        let img = sample_image(40, 40);
        assert_ne!(bulge(&img, 8, true), bulge(&img, 8, false));
    }

    #[test]
    fn the_dice_stay_inside_their_bounds() {
        let mut roll = Roll::new(3);
        for _ in 0..200 {
            assert!(roll.upto(5) < 5);
            assert_eq!(roll.upto(0), 0);
            let a = roll.about(4);
            assert!((-4..=4).contains(&a));
            assert_eq!(roll.about(0), 0);
        }
    }
}
