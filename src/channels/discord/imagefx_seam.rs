//! Seam carving: the machinery behind `magik`.
//!
//! Content-aware resizing. A *seam* is a connected path of pixels from the top
//! of the picture to the bottom, one per row, each within a column of the one
//! above. Remove the seam that crosses the least detail and the picture gets a
//! pixel narrower without squashing anything that matters; do it four hundred
//! times and the picture is half as wide, with the flat parts eaten and the
//! busy parts crowded together. Grow it back by *inserting* the same kind of
//! seams and the crowding stays. That is the whole melty look.
//!
//! Written here rather than taken from a crate: the one maintained Rust seam
//! carver (`seamcarving`) is LGPL-3.0-or-later, which is not a licence this
//! binary can take on, and the algorithm is a hundred lines.
//!
//! Everything is bounded. The caller hands in a small working image (see
//! `imagefx_engine::MAGIK_SIDE`), the number of seams is capped at half the
//! side, and [`too_much`] refuses a job whose pixel-passes would run away -
//! the caller then falls back to a plain resize, which is a worse picture but
//! never a hung thread.

use image::RgbaImage;
use image::imageops::FilterType;

/// The most pixel-passes one carve may cost (width x height x seams). A
/// 350x350 picture losing 175 seams is about 21 million, which takes a few
/// tens of milliseconds; a hundred times that would take minutes.
const MAX_PASSES: u64 = 120_000_000;

/// Whether carving this many seams off this picture is more work than we allow.
pub fn too_much(w: u32, h: u32, seams: u32) -> bool {
    u64::from(w) * u64::from(h) * u64::from(seams) > MAX_PASSES
}

/// A picture as a flat buffer, with the column each pixel started life in.
/// `orig` is what makes insertion possible: after a run of removals it still
/// says which of the *original* columns each surviving pixel came from.
struct Plane {
    w: usize,
    h: usize,
    px: Vec<[u8; 4]>,
    orig: Vec<u32>,
}

impl Plane {
    fn from_image(img: &RgbaImage) -> Plane {
        let (w, h) = (img.width() as usize, img.height() as usize);
        let px = img.pixels().map(|p| p.0).collect();
        let orig = (0..h).flat_map(|_| 0..w as u32).collect();
        Plane { w, h, px, orig }
    }

    fn to_image(&self) -> RgbaImage {
        let mut out = RgbaImage::new(self.w as u32, self.h as u32);
        for (dst, src) in out.pixels_mut().zip(self.px.iter()) {
            *dst = image::Rgba(*src);
        }
        out
    }

    fn at(&self, x: usize, y: usize) -> [u8; 4] {
        self.px[y * self.w + x]
    }

    /// Rows become columns. Carving the height is carving the width of this.
    fn transposed(&self) -> Plane {
        let (w, h) = (self.h, self.w);
        let mut px = vec![[0u8; 4]; w * h];
        let mut orig = vec![0u32; w * h];
        for y in 0..h {
            for x in 0..w {
                // (x, y) of the transpose is (y, x) of the original.
                px[y * w + x] = self.px[x * self.w + y];
                orig[y * w + x] = x as u32;
            }
        }
        Plane { w, h, px, orig }
    }
}

fn luma(p: [u8; 4]) -> f32 {
    // Premultiplied by alpha so a transparent hole counts as flat rather than
    // as whatever colour happens to be sitting under the zero.
    let a = p[3] as f32 / 255.0;
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) * a
}

/// How much detail sits at each pixel: the gradient magnitude of the
/// brightness, with the edges of the picture reading their own value back.
fn energy(p: &Plane) -> Vec<f32> {
    let (w, h) = (p.w, p.h);
    let mut lum = vec![0f32; w * h];
    for (i, px) in p.px.iter().enumerate() {
        lum[i] = luma(*px);
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            let l = |xx: usize, yy: usize| lum[yy.min(h - 1) * w + xx.min(w - 1)];
            let (xl, xr) = (x.saturating_sub(1), (x + 1).min(w - 1));
            let (yu, yd) = (y.saturating_sub(1), (y + 1).min(h - 1));
            let dx = l(xr, y) - l(xl, y);
            let dy = l(x, yd) - l(x, yu);
            out[y * w + x] = (dx * dx + dy * dy).sqrt();
        }
    }
    out
}

/// The cheapest top-to-bottom seam, as one column per row.
fn seam(p: &Plane, e: &[f32]) -> Vec<usize> {
    let (w, h) = (p.w, p.h);
    if w == 0 || h == 0 {
        return Vec::new();
    }
    // cost[y][x]: the cheapest way down to (x, y). from[y][x]: which of the
    // three pixels above it came through.
    let mut cost = vec![0f32; w * h];
    let mut from = vec![0u8; w * h];
    cost[..w].copy_from_slice(&e[..w]);
    for y in 1..h {
        for x in 0..w {
            let mut best = cost[(y - 1) * w + x];
            let mut pick = 1u8;
            if x > 0 && cost[(y - 1) * w + x - 1] < best {
                best = cost[(y - 1) * w + x - 1];
                pick = 0;
            }
            if x + 1 < w && cost[(y - 1) * w + x + 1] < best {
                best = cost[(y - 1) * w + x + 1];
                pick = 2;
            }
            cost[y * w + x] = e[y * w + x] + best;
            from[y * w + x] = pick;
        }
    }
    let last = (h - 1) * w;
    let mut x = (0..w).min_by(|a, b| cost[last + a].total_cmp(&cost[last + b])).unwrap_or(0);
    let mut path = vec![0usize; h];
    for y in (0..h).rev() {
        path[y] = x;
        if y > 0 {
            x = match from[y * w + x] {
                0 => x - 1,
                2 => x + 1,
                _ => x,
            };
        }
    }
    path
}

/// Takes the seam out, one pixel narrower, and hands back which original
/// column it took from each row.
fn remove(p: &mut Plane, path: &[usize]) -> Vec<u32> {
    let (w, h) = (p.w, p.h);
    let mut px = Vec::with_capacity((w - 1) * h);
    let mut orig = Vec::with_capacity((w - 1) * h);
    let mut taken = Vec::with_capacity(h);
    for y in 0..h {
        let cut = path[y].min(w - 1);
        taken.push(p.orig[y * w + cut]);
        for x in 0..w {
            if x != cut {
                px.push(p.px[y * w + x]);
                orig.push(p.orig[y * w + x]);
            }
        }
    }
    p.w = w - 1;
    p.px = px;
    p.orig = orig;
    taken
}

/// `n` seams off the width, cheapest first.
fn shrink_width(p: &mut Plane, n: usize) {
    for _ in 0..n {
        if p.w <= 1 {
            return;
        }
        let e = energy(p);
        let s = seam(p, &e);
        remove(p, &s);
    }
}

/// `n` seams into the width. Which seams to widen is found by carving them
/// *out* of a copy: the cheapest n seams of the original are exactly the ones
/// n rounds of removal would take, and `orig` says where they were. Each is
/// then doubled in place, the new pixel being the average of its neighbours,
/// so the picture grows where it was already crowded.
fn grow_width(p: &mut Plane, n: usize) {
    if n == 0 || p.w == 0 || p.h == 0 {
        return;
    }
    let (w, h) = (p.w, p.h);
    let mut copy = Plane { w, h, px: p.px.clone(), orig: p.orig.clone() };
    // Every removal takes one pixel from every row, so after n rounds each row
    // has exactly n columns marked - which is why the output is rectangular.
    let mut dup = vec![false; w * h];
    for _ in 0..n {
        if copy.w <= 1 {
            break;
        }
        let e = energy(&copy);
        let s = seam(&copy, &e);
        for (y, col) in remove(&mut copy, &s).into_iter().enumerate() {
            dup[y * w + col as usize] = true;
        }
    }
    let mut px = Vec::with_capacity((w + n) * h);
    let mut orig = Vec::with_capacity((w + n) * h);
    let mut out_w = 0usize;
    for y in 0..h {
        let mut wrote = 0usize;
        for x in 0..w {
            let here = p.px[y * w + x];
            px.push(here);
            orig.push(p.orig[y * w + x]);
            wrote += 1;
            if dup[y * w + x] {
                let next = p.px[y * w + (x + 1).min(w - 1)];
                let mean = [0, 1, 2, 3].map(|i| ((here[i] as u16 + next[i] as u16) / 2) as u8);
                px.push(mean);
                orig.push(p.orig[y * w + x]);
                wrote += 1;
            }
        }
        // A row that somehow came out short or long is padded or cut, so the
        // buffer stays a rectangle whatever the seams did.
        out_w = out_w.max(wrote);
    }
    if out_w * h != px.len() {
        // Uneven rows: rebuild at the widest row, repeating the last pixel.
        let mut even = Vec::with_capacity(out_w * h);
        let mut even_orig = Vec::with_capacity(out_w * h);
        let mut i = 0usize;
        for _ in 0..h {
            let start = i;
            let mut wrote = 0usize;
            while wrote < out_w && i < px.len() {
                even.push(px[i]);
                even_orig.push(orig[i]);
                i += 1;
                wrote += 1;
            }
            while wrote < out_w {
                even.push(*even.last().unwrap_or(&px[start.min(px.len() - 1)]));
                even_orig.push(0);
                wrote += 1;
            }
        }
        px = even;
        orig = even_orig;
    }
    p.w = out_w;
    p.px = px;
    p.orig = orig;
}

/// Carve the picture down to `to_w` x `to_h`. Growing in either direction is
/// left to [`grow`]; this only takes away.
pub fn shrink(img: &RgbaImage, to_w: u32, to_h: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let to_w = to_w.clamp(1, w);
    let to_h = to_h.clamp(1, h);
    if to_w == w && to_h == h {
        return img.clone();
    }
    let mut p = Plane::from_image(img);
    shrink_width(&mut p, (w - to_w) as usize);
    if to_h < h {
        let mut t = p.transposed();
        shrink_width(&mut t, (h - to_h) as usize);
        p = t.transposed();
    }
    p.to_image()
}

/// Grow the picture out to `to_w` x `to_h` by inserting seams.
pub fn grow(img: &RgbaImage, to_w: u32, to_h: u32) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let to_w = to_w.max(w);
    let to_h = to_h.max(h);
    if to_w == w && to_h == h {
        return img.clone();
    }
    let mut p = Plane::from_image(img);
    grow_width(&mut p, (to_w - w) as usize);
    if to_h > h {
        let mut t = p.transposed();
        grow_width(&mut t, (to_h - h) as usize);
        p = t.transposed();
    }
    p.to_image()
}

/// How far down `magik` carves, by strength. Ten is NotSoBot's 50%; one barely
/// touches it.
pub fn shrink_to(strength: u8) -> f32 {
    let s = strength.clamp(1, 10) as f32;
    // Five, the middle, lands on NotSoBot's half - which is what makes the
    // picture melt rather than merely settle.
    0.72 - (s - 1.0) * 0.049
}

/// The signature effect: carve away, grow back, and stretch what is left to
/// the size it started at. The distortion is all in the carving; the final
/// resize only puts the frame back to the size the caller asked for.
///
/// Returns the picture unchanged in the pathological cases - a picture too
/// small to have seams, or a job [`too_much`] would refuse - so the caller
/// never has to special-case them.
pub fn magik(img: &RgbaImage, strength: u8) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    if w < 8 || h < 8 {
        return img.clone();
    }
    let f = shrink_to(strength);
    let sw = ((w as f32 * f).round() as u32).clamp(4, w);
    let sh = ((h as f32 * f).round() as u32).clamp(4, h);
    if too_much(w, h, (w - sw) + (h - sh)) {
        return img.clone();
    }
    let small = shrink(img, sw, sh);
    // Then out again by half as much again, the way liquid_rescale 150% does.
    let gw = (sw as f32 * 1.5).round() as u32;
    let gh = (sh as f32 * 1.5).round() as u32;
    let grown = if too_much(sw, sh, (gw - sw) + (gh - sh)) { small } else { grow(&small, gw, gh) };
    if grown.width() == w && grown.height() == h {
        grown
    } else {
        image::imageops::resize(&grown, w, h, FilterType::Lanczos3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with a bright stripe down one side: the flat half is what the
    /// carver should eat, so the stripe survives.
    fn striped(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, _| {
            if x > w * 3 / 4 { image::Rgba([255, 240, 20, 255]) } else { image::Rgba([20, 20, 30, 255]) }
        })
    }

    #[test]
    fn carving_narrows_the_picture_to_exactly_what_was_asked() {
        let img = striped(60, 40);
        let out = shrink(&img, 30, 20);
        assert_eq!((out.width(), out.height()), (30, 20));
    }

    #[test]
    fn growing_widens_the_picture_to_exactly_what_was_asked() {
        let img = striped(40, 30);
        let out = grow(&img, 60, 45);
        assert_eq!((out.width(), out.height()), (60, 45));
    }

    /// The point of the thing: detail is kept and flatness is eaten. Count the
    /// bright pixels before and after - carving half the width away should
    /// keep most of the stripe, where a plain squash would lose half of it.
    #[test]
    fn carving_keeps_the_detail_and_eats_the_flat_part() {
        let img = striped(80, 40);
        let bright = |i: &RgbaImage| i.pixels().filter(|p| p.0[0] > 200).count();
        let before = bright(&img);
        let after = bright(&shrink(&img, 40, 40));
        assert!(after * 2 > before, "carving lost the stripe: {} of {}", after, before);
    }

    #[test]
    fn magik_gives_back_the_size_it_was_given() {
        for (w, h) in [(1, 1), (3, 9), (8, 8), (64, 40), (40, 64)] {
            let out = magik(&striped(w, h), 5);
            assert_eq!((out.width(), out.height()), (w, h), "{}x{}", w, h);
        }
    }

    /// Something must actually change, or the effect is a no-op with extra steps.
    #[test]
    fn magik_changes_the_picture() {
        let img = RgbaImage::from_fn(64, 64, |x, y| image::Rgba([(x * 4) as u8, (y * 4) as u8, 128, 255]));
        let out = magik(&img, 8);
        let moved = img.pixels().zip(out.pixels()).filter(|(a, b)| a.0 != b.0).count();
        assert!(moved > 64 * 64 / 10, "magik barely touched it: {} pixels", moved);
    }

    #[test]
    fn a_runaway_job_is_refused_rather_than_attempted() {
        assert!(!too_much(350, 350, 175));
        assert!(too_much(4000, 4000, 2000));
    }

    #[test]
    fn stronger_carves_further() {
        assert!(shrink_to(1) > shrink_to(5) && shrink_to(5) > shrink_to(10));
        // Ten carves away nearly three quarters of each side; one barely bites.
        assert!((0.25..0.32).contains(&shrink_to(10)), "ten carves to {}", shrink_to(10));
        assert!((0.65..0.80).contains(&shrink_to(1)), "one carves to {}", shrink_to(1));
        assert!((0.48..0.56).contains(&shrink_to(5)), "five should be about NotSoBot's half, got {}", shrink_to(5));
    }

    #[test]
    fn transparency_survives_the_carve() {
        let img = RgbaImage::from_fn(40, 40, |x, _| image::Rgba([200, 30, 30, if x < 20 { 0 } else { 255 }]));
        let out = shrink(&img, 30, 40);
        assert!(out.pixels().any(|p| p.0[3] == 0), "the hole was filled in");
        assert!(out.pixels().any(|p| p.0[3] == 255), "the solid half went missing");
    }
}
