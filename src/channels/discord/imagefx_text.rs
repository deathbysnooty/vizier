//! Words on pictures: `caption`, `meme` and `ascii`, plus the hygiene that
//! keeps a caption from pinging the server.
//!
//! The type goes through cosmic-text, the same as every card in this bot, and
//! is blended straight into the `RgbaImage` rather than through tiny-skia -
//! there are no paths to draw here, only glyphs, and blending by hand is what
//! lets the text sit on a transparent PNG without turning the holes grey.
//!
//! **The face.** Meme text wants Impact, and Impact is Microsoft's; so are
//! Arial and Courier. None of them is bundled and none of them is asked for by
//! name. What is asked for is the free condensed-heavy family that stands in
//! for Impact everywhere else - Anton first, then Oswald and the rest - and if
//! a server has none of them, any installed family whose name reads condensed
//! or gothic, with the proprietary names filtered out, and failing that the
//! generic `sans-serif` the system resolves itself.

use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Stretch, Style as FontStyle, SwashCache, Weight};
use image::{Rgba, RgbaImage};

/// Free stand-ins for Impact, best first. Anton is the one the meme
/// generators settled on.
const HEAVY: &[&str] = &[
    "Anton",
    "Oswald",
    "Archivo Black",
    "Bebas Neue",
    "Fjalla One",
    "League Gothic",
    "Passion One",
    "Alfa Slab One",
    "Noto Sans Display",
    "Montserrat",
    "DejaVu Sans",
    "Liberation Sans Narrow",
];

/// Never these, however the search gets there: they are licensed fonts that
/// happen to be installed on some machines, and a bot that draws with them is
/// a bot that redistributes them.
const NEVER: &[&str] = &["impact", "arial", "courier", "helvetica", "times new roman", "georgia", "verdana", "tahoma"];

fn installed(fs: &FontSystem, names: &[&str]) -> Option<String> {
    names.iter().find(|name| fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == *name))).map(|n| n.to_string())
}

/// Any installed family whose name reads like one of these words - never one of
/// [`NEVER`].
fn family_like(fs: &FontSystem, words: &[&str]) -> Option<String> {
    fs.db().faces().flat_map(|f| f.families.iter().map(|(n, _)| n.clone())).find(|name| {
        let lower = name.to_lowercase();
        words.iter().any(|w| lower.contains(w)) && !NEVER.iter().any(|w| lower.contains(w))
    })
}

/// The face captions and memes are drawn in.
pub fn heavy_face(fs: &FontSystem) -> String {
    installed(fs, HEAVY)
        .or_else(|| family_like(fs, &["gothic", "condensed", "narrow", "black", "grotesk"]))
        .or_else(|| family_like(fs, &["sans"]))
        .unwrap_or_else(|| "sans-serif".to_string())
}

// --- keeping a caption quiet -------------------------------------------------------------------

/// Text safe to draw and safe to post: no mention markup, no `@everyone`, no
/// stray at-sign left to become one, and not long enough to fill a picture.
///
/// Pixels cannot ping anybody, but the same string travels in the message
/// beside the picture, and a caption that merely *reads* `@everyone` is a
/// caption someone will copy. Both go through here.
pub fn depinged(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            // <@123>, <@!123>, <@&123>, <#123>, <id:customize> - Discord's
            // mention markup. Swallow up to the closing bracket.
            let rest: String = chars.clone().take(24).collect();
            let is_mention = rest.starts_with('@') || rest.starts_with('#') || rest.starts_with("id:");
            if is_mention && rest.contains('>') {
                for c in chars.by_ref() {
                    if c == '>' {
                        break;
                    }
                }
                continue;
            }
        }
        // Every at-sign goes, which takes @everyone, @here and role names with
        // it and leaves the words readable.
        if c == '@' {
            continue;
        }
        out.push(c);
    }
    let out: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    out.chars().take(MAX_TEXT).collect()
}

/// The most characters a caption or meme line may carry.
pub const MAX_TEXT: usize = 200;

/// `meme` takes one string and splits it into the two lines on a `|`. With no
/// bar the whole thing goes on top, which is what people expect.
pub fn split_meme(text: &str) -> (String, String) {
    match text.split_once('|') {
        Some((top, bottom)) => (top.trim().to_string(), bottom.trim().to_string()),
        None => (text.trim().to_string(), String::new()),
    }
}

// --- the pen -----------------------------------------------------------------------------------

/// Source-over blend of one glyph fragment, on straight alpha, so a glyph can
/// land on a transparent pixel without dragging a grey box with it.
fn blend(img: &mut RgbaImage, x: i32, y: i32, w: u32, h: u32, c: Color) {
    let sa = c.a() as f32 / 255.0;
    if sa <= 0.0 {
        return;
    }
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let src = [c.r() as f32, c.g() as f32, c.b() as f32];
    for yy in y.max(0)..(y + h as i32).min(ih) {
        for xx in x.max(0)..(x + w as i32).min(iw) {
            let p = img.get_pixel_mut(xx as u32, yy as u32);
            let da = p.0[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            if oa <= 0.0 {
                *p = Rgba([0, 0, 0, 0]);
                continue;
            }
            for i in 0..3 {
                let d = p.0[i] as f32;
                p.0[i] = (((src[i] * sa) + d * da * (1.0 - sa)) / oa).clamp(0.0, 255.0) as u8;
            }
            p.0[3] = (oa * 255.0).clamp(0.0, 255.0) as u8;
        }
    }
}

struct Pen<'a> {
    fs: &'a mut FontSystem,
    cache: SwashCache,
    family: String,
    /// Every face the family actually has, so a weight it hasn't got snaps to
    /// one it has instead of dropping the words into the system fallback.
    faces: Vec<(FontStyle, Stretch, Weight)>,
}

impl<'a> Pen<'a> {
    fn new(fs: &'a mut FontSystem) -> Pen<'a> {
        let family = heavy_face(fs);
        let faces = fs
            .db()
            .faces()
            .filter(|f| f.families.iter().any(|(n, _)| *n == family))
            .map(|f| (f.style, f.stretch, f.weight))
            .collect();
        Pen { fs, cache: SwashCache::new(), family, faces }
    }

    fn snap(&self, weight: Weight) -> (FontStyle, Stretch, Weight) {
        let nearest = |upright: bool| {
            self.faces
                .iter()
                .filter(|f| !upright || f.0 == FontStyle::Normal)
                .min_by_key(|f| (f.2.0 as i32 - weight.0 as i32).abs())
                .copied()
        };
        nearest(true).or_else(|| nearest(false)).unwrap_or((FontStyle::Normal, Stretch::Normal, weight))
    }

    /// Wrapped text, with the width it actually took and the height it needs.
    /// Every line is centred in the box, which is what makes a caption that
    /// wraps look like a caption rather than like a paragraph.
    fn layout(&mut self, text: &str, size: f32, max_w: f32) -> (Buffer, usize, f32) {
        let (style, stretch, weight) = self.snap(Weight::BLACK);
        let family = self.family.clone();
        let metrics = Metrics::new(size, size * 1.12);
        let width = max_w.max(8.0);
        let fs = &mut *self.fs;
        let mut buf = Buffer::new(fs, metrics);
        buf.set_size(fs, Some(width), None);
        let attrs = Attrs::new().family(Family::Name(&family)).weight(weight).style(style).stretch(stretch);
        buf.set_text(fs, text, attrs, Shaping::Advanced);
        for line in buf.lines.iter_mut() {
            line.set_align(Some(Align::Center));
        }
        buf.shape_until_scroll(fs, false);
        let lines = buf.layout_runs().count().max(1);
        (buf, lines, lines as f32 * metrics.line_height)
    }

    /// The biggest type that fits.
    ///
    /// Two passes, because the two failures look different. First it comes down
    /// from `start` while the words wrap, but only so far: a short caption
    /// belongs on one line, and a long one belongs in readable type rather than
    /// squeezed onto one line at six pixels. Then, whatever it settled on, it
    /// comes down further while the block is taller than the space allows.
    fn fitted(&mut self, text: &str, start: f32, max_w: f32, max_h: f32) -> (Buffer, usize, f32) {
        let floor = (start * 0.45).max(11.0);
        let mut size = start.max(11.0);
        let mut best = self.layout(text, size, max_w);
        while best.1 > 1 && size > floor {
            size = (size * 0.88).max(floor);
            best = self.layout(text, size, max_w);
        }
        while best.2 > max_h && size > 9.0 {
            size = (size * 0.85).max(9.0);
            best = self.layout(text, size, max_w);
        }
        best
    }

    fn put(&mut self, buf: &Buffer, ox: i32, oy: i32, colour: [u8; 3], img: &mut RgbaImage) {
        let (fs, cache) = (&mut *self.fs, &mut self.cache);
        buf.draw(fs, cache, Color::rgb(colour[0], colour[1], colour[2]), |gx, gy, gw, gh, c| {
            blend(img, ox + gx, oy + gy, gw, gh, c);
        });
    }

    /// The same words behind themselves in black, so white type reads over any
    /// picture. This is what the outline on a meme actually is.
    fn outlined(&mut self, buf: &Buffer, ox: i32, oy: i32, ring: i32, img: &mut RgbaImage) {
        for dy in -ring..=ring {
            for dx in -ring..=ring {
                if dx * dx + dy * dy > ring * ring || (dx == 0 && dy == 0) {
                    continue;
                }
                self.put(buf, ox + dx, oy + dy, [0, 0, 0], img);
            }
        }
        self.put(buf, ox, oy, [255, 255, 255], img);
    }
}

/// Where the caption's white bar sits and how tall it is, worked out before
/// anything is drawn so the tests can check it without a font installed.
pub fn bar_height(img_h: u32, text_h: f32, pad: f32) -> u32 {
    let wanted = (text_h + pad * 2.0).ceil().max(8.0) as u32;
    // A bar may take the picture's own height at most, so a paragraph pasted in
    // as a caption cannot make a strip a mile high.
    wanted.min(img_h.max(24) * 2)
}

/// A white bar above the picture with the words in it, the way NotSoBot's
/// `caption` does it. Returns the picture unchanged when there is nothing to
/// say.
pub fn caption(img: &RgbaImage, text: &str) -> RgbaImage {
    let text = depinged(text);
    if text.is_empty() {
        return img.clone();
    }
    let (w, h) = (img.width().max(1), img.height().max(1));
    let pad = (w as f32 * 0.045).clamp(6.0, 48.0);
    let Some(mutex) = Some(super::awards::fonts()) else { return img.clone() };
    let mut fs = mutex.lock();
    if fs.db().len() == 0 {
        // No fonts at all: cosmic-text panics rather than drawing nothing.
        return img.clone();
    }
    let mut pen = Pen::new(&mut fs);
    let start = (w as f32 / 8.5).clamp(12.0, 130.0);
    let (buf, _lines, th) = pen.fitted(&text, start, w as f32 - pad * 2.0, h as f32 * 0.8);
    let bar = bar_height(h, th, pad);
    let mut out = RgbaImage::from_pixel(w, h + bar, Rgba([255, 255, 255, 255]));
    let top = ((bar as f32 - th) / 2.0).max(0.0).round() as i32;
    pen.put(&buf, pad.round() as i32, top, [8, 8, 8], &mut out);
    drop(buf);
    drop(pen);
    drop(fs);
    image::imageops::replace(&mut out, img, 0, i64::from(bar));
    out
}

/// Top and bottom text over the picture, white with a black outline.
pub fn meme(img: &RgbaImage, top: &str, bottom: &str) -> RgbaImage {
    let top = depinged(top).to_uppercase();
    let bottom = depinged(bottom).to_uppercase();
    if top.is_empty() && bottom.is_empty() {
        return img.clone();
    }
    let (w, h) = (img.width().max(1), img.height().max(1));
    let mut out = img.clone();
    let mutex = super::awards::fonts();
    let mut fs = mutex.lock();
    if fs.db().len() == 0 {
        return out;
    }
    let mut pen = Pen::new(&mut fs);
    let pad = (w as f32 * 0.04).clamp(4.0, 40.0);
    let size = (h as f32 / 7.0).clamp(11.0, 120.0);
    let ring = ((size / 16.0).round() as i32).clamp(1, 5);
    let band = h as f32 * 0.42;
    let left = pad.round() as i32;
    if !top.is_empty() {
        let (buf, _lines, _th) = pen.fitted(&top, size, w as f32 - pad * 2.0, band);
        pen.outlined(&buf, left, pad.round() as i32, ring, &mut out);
    }
    if !bottom.is_empty() {
        let (buf, _lines, th) = pen.fitted(&bottom, size, w as f32 - pad * 2.0, band);
        let oy = (h as f32 - th - pad).max(0.0).round() as i32;
        pen.outlined(&buf, left, oy, ring, &mut out);
    }
    out
}

// --- ascii -------------------------------------------------------------------------------------

/// Dark to light. The gaps matter more than the glyphs: this is what gives the
/// picture its tones in a monospaced code block.
const RAMP: &[u8] = b" .,:;irsXA253hMHGS#9B&@";

/// How many columns and rows fit, given Discord's message limit. Character
/// cells are about twice as tall as they are wide, which is the `/ 2`.
pub fn ascii_shape(w: u32, h: u32, cols: u32, max_chars: usize) -> (u32, u32) {
    let cols = cols.clamp(8, 80);
    let (w, h) = (w.max(1), h.max(1));
    let rows = (((h as f32 / w as f32) * cols as f32) / 2.0).round().clamp(1.0, 60.0) as u32;
    // Each row costs its columns plus a newline.
    let rows = rows.min((max_chars as u32 / (cols + 1)).max(1));
    (cols, rows)
}

/// The picture as text, fenced so Discord shows it monospaced. Never longer
/// than a message is allowed to be.
pub fn ascii(img: &RgbaImage, cols: u32) -> String {
    const FENCE: usize = 10;
    const LIMIT: usize = 1960;
    let (cols, rows) = ascii_shape(img.width(), img.height(), cols, LIMIT - FENCE);
    let small = image::imageops::resize(img, cols, rows, image::imageops::FilterType::Triangle);
    let mut out = String::with_capacity((cols as usize + 1) * rows as usize + FENCE);
    out.push_str("```\n");
    for y in 0..rows {
        for x in 0..cols {
            let p = small.get_pixel(x, y).0;
            // Transparent reads as empty, not as black: an avatar cut out of its
            // background should not come back as a solid block.
            let a = p[3] as f32 / 255.0;
            let l = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) * a / 255.0;
            let i = ((l * (RAMP.len() - 1) as f32).round() as usize).min(RAMP.len() - 1);
            out.push(RAMP[i] as char);
        }
        out.push('\n');
    }
    out.push_str("```");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| Rgba([(x * 5 % 256) as u8, (y * 9 % 256) as u8, 90, 255]))
    }

    #[test]
    fn a_caption_cannot_ping_anybody() {
        for (raw, banned) in [
            ("@everyone look at this", "@everyone"),
            ("@here quick", "@here"),
            ("hey <@1234567890> smile", "<@"),
            ("cc <@&987654321>", "<@&"),
            ("<@!42> you", "<@!"),
            ("meet in <#555>", "<#"),
            ("mail me at a@b.com", "@"),
        ] {
            let out = depinged(raw);
            assert!(!out.contains(banned), "{:?} still carries {:?}: {:?}", raw, banned, out);
            assert!(!out.contains('@'), "{:?} still has an at-sign: {:?}", raw, out);
            assert!(!out.contains("<@") && !out.contains("<#"), "{:?} still has markup: {:?}", raw, out);
        }
    }

    #[test]
    fn stripping_keeps_the_words() {
        assert_eq!(depinged("@everyone look at this"), "everyone look at this");
        assert_eq!(depinged("hey <@123> smile"), "hey smile");
        assert_eq!(depinged("  lots   of   space  "), "lots of space");
        assert_eq!(depinged("a < b and 3 > 2"), "a < b and 3 > 2", "plain angle brackets are not markup");
    }

    #[test]
    fn a_caption_cannot_be_endless() {
        let long = "ping ".repeat(400);
        assert!(depinged(&long).chars().count() <= MAX_TEXT);
    }

    #[test]
    fn meme_text_splits_on_the_bar() {
        assert_eq!(split_meme("top | bottom"), ("top".to_string(), "bottom".to_string()));
        assert_eq!(split_meme("just the top"), ("just the top".to_string(), String::new()));
        assert_eq!(split_meme("| only below"), (String::new(), "only below".to_string()));
    }

    /// The licensed faces are never asked for, by either route.
    #[test]
    fn no_proprietary_face_is_ever_named() {
        for name in HEAVY {
            let lower = name.to_lowercase();
            assert!(!NEVER.iter().any(|bad| lower.contains(bad)), "{} is on the wish list", name);
        }
        assert!(NEVER.contains(&"impact") && NEVER.contains(&"arial") && NEVER.contains(&"courier"));
    }

    #[test]
    fn a_caption_adds_a_bar_above_and_keeps_the_width() {
        let img = sample(200, 120);
        let out = caption(&img, "hello there");
        assert_eq!(out.width(), 200);
        assert!(out.height() > 120, "no bar was added");
        // The picture itself is still at the bottom, pixel for pixel.
        let bar = out.height() - 120;
        assert_eq!(out.get_pixel(40, bar + 10), img.get_pixel(40, 10));
        // And the bar is white.
        assert_eq!(out.get_pixel(1, 1).0, [255, 255, 255, 255]);
    }

    #[test]
    fn an_empty_caption_leaves_the_picture_alone() {
        let img = sample(40, 40);
        assert_eq!(caption(&img, "   "), img);
        assert_eq!(meme(&img, "", ""), img);
    }

    #[test]
    fn a_bar_is_never_taller_than_the_cap() {
        assert!(bar_height(100, 10_000.0, 10.0) <= 200);
        assert!(bar_height(10, 5.0, 2.0) >= 8);
    }

    #[test]
    fn meme_keeps_the_frame_and_marks_it() {
        let img = RgbaImage::from_pixel(200, 200, Rgba([60, 60, 60, 255]));
        let out = meme(&img, "top", "bottom");
        assert_eq!(out.dimensions(), (200, 200));
        // Something was drawn - unless the machine has no fonts at all, in
        // which case there is nothing to test.
        if super::super::awards::fonts().lock().db().len() > 0 {
            assert_ne!(out, img, "no words landed on the picture");
        }
    }

    #[test]
    fn a_caption_survives_a_one_pixel_picture() {
        let one = RgbaImage::from_pixel(1, 1, Rgba([0, 0, 0, 255]));
        assert_eq!(caption(&one, "hi").width(), 1);
        assert_eq!(meme(&one, "hi", "there").dimensions(), (1, 1));
    }

    #[test]
    fn ascii_fits_in_one_discord_message() {
        for (w, h) in [(1, 1), (800, 800), (800, 100), (100, 800), (3, 999)] {
            let out = ascii(&sample(w, h), 58);
            assert!(out.len() <= 2000, "{}x{} came out {} characters", w, h, out.len());
            assert!(out.starts_with("```\n") && out.ends_with("```"));
        }
    }

    #[test]
    fn ascii_has_the_tones_the_right_way_round() {
        let dark = ascii(&RgbaImage::from_pixel(60, 60, Rgba([0, 0, 0, 255])), 30);
        let light = ascii(&RgbaImage::from_pixel(60, 60, Rgba([255, 255, 255, 255])), 30);
        assert!(dark.contains("   "), "black should be mostly blank");
        assert!(light.contains("@@@"), "white should be the heaviest glyph");
    }

    #[test]
    fn ascii_never_asks_for_more_rows_than_fit() {
        let (cols, rows) = ascii_shape(100, 4000, 58, 600);
        assert!((cols + 1) * rows <= 600, "{}x{} does not fit", cols, rows);
        assert!(rows >= 1);
    }
}
