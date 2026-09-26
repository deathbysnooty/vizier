//! The engine: bytes in, bytes out.
//!
//! One picture arrives as a blob of PNG, JPEG, WEBP or GIF; one effect is named;
//! a PNG, a GIF or a block of text comes back. Everything between - the frame
//! handling, the caps, the time budget, the encoding and the shrinking that
//! makes a result fit Discord's upload limit - lives here. No Discord types, no
//! settings lookups, no clock beyond a stopwatch: this module is a function,
//! and the tests treat it as one.
//!
//! **Where the caps are.** Nothing is trusted. The picture is scaled down to
//! [`Limits::max_side`] before any work happens (NotSoBot's 800x800 rule), an
//! animated input is read to [`Limits::max_frames`] frames and no further, an
//! animated *frame* is worked at the smaller [`Limits::anim_side`] because sixty
//! of them at full size is a hundred megabytes and a hung thread, and
//! [`Limits::budget`] stops a multi-frame job that is taking too long. `magik`
//! has a working size of its own ([`MAGIK_SIDE`]): seam carving costs width x
//! height x seams, so it is done small and stretched back.
//!
//! Callers run [`run`] on a blocking thread. It is CPU-bound from end to end.

use std::io::Cursor;
use std::time::{Duration, Instant};

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::imageops::FilterType;
use image::{AnimationDecoder, ImageFormat, RgbaImage};

use super::imagefx_effects as fx;
use super::imagefx_seam as seam;
use super::imagefx_text as words;

/// Seam carving is done at this size however big the picture is, and the result
/// stretched back. Carving costs width x height x seams; 350 keeps `magik`
/// under a tenth of a second and loses nothing anybody can see once the frame
/// is back to 800.
pub const MAGIK_SIDE: u32 = 350;

/// How many frames an effect makes out of a still picture, and how long each
/// one shows for. Fourteen at 70ms is a one-second loop.
pub const MADE_FRAMES: usize = 14;
const MADE_DELAY: u16 = 70;

/// GIF will not show a frame for less than two hundredths of a second; asking
/// for less makes players run the animation at their own speed instead.
const MIN_DELAY: u16 = 20;

/// What the caller allows itself to spend. Every field is a setting; see the
/// `Image toolkit` section of the catalog.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// The longest side a still picture is worked at.
    pub max_side: u32,
    /// The longest side an animated frame is worked at.
    pub anim_side: u32,
    /// The most frames read out of an animated input, or made from a still.
    pub max_frames: usize,
    /// How long the whole job may take before it gives up.
    pub budget: Duration,
    /// The most the result may weigh when it is handed to Discord.
    pub upload_cap: usize,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits { max_side: 800, anim_side: 256, max_frames: 60, budget: Duration::from_secs(20), upload_cap: 8 * 1024 * 1024 }
    }
}

/// Why a job did not produce a picture. Every one of these is shown to whoever
/// asked, in these words, so they say what to do differently.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The bytes are not a picture this can read.
    NotAnImage,
    /// The picture is bigger than the caps allow even after scaling.
    TooBig,
    /// The work ran past its budget with nothing to show.
    TooSlow,
    /// `caption` and `meme` need something to say.
    NeedsText,
    /// Something in the middle failed; the detail is for the log.
    Broke(String),
}

impl Trouble {
    /// The sentence the member reads.
    pub fn plainly(&self) -> String {
        match self {
            Trouble::NotAnImage => "That isn't a picture I can read - PNG, JPEG, WEBP or GIF only.".into(),
            Trouble::TooBig => "That picture is too big for me to work on. Try a smaller one.".into(),
            Trouble::TooSlow => "That took too long, so I stopped. Try a smaller picture, or a shorter GIF.".into(),
            Trouble::NeedsText => "That one needs some words - put them in `text`.".into(),
            Trouble::Broke(_) => "That didn't work. Try again, or try another picture.".into(),
        }
    }
}

/// One frame and how long it shows for.
#[derive(Clone)]
pub struct Frame {
    pub img: RgbaImage,
    pub delay: u16,
}

/// What comes back.
pub enum Out {
    Png(Vec<u8>),
    Gif(Vec<u8>),
    /// `ascii`: a fenced code block, not a file.
    Text(String),
}

impl Out {
    pub fn filename(&self, effect: Effect) -> String {
        match self {
            Out::Png(_) => format!("{}.png", effect.key()),
            Out::Gif(_) => format!("{}.gif", effect.key()),
            Out::Text(_) => format!("{}.txt", effect.key()),
        }
    }

    /// How much this weighs where it matters - nothing, for text.
    pub fn weight(&self) -> usize {
        match self {
            Out::Png(b) | Out::Gif(b) => b.len(),
            Out::Text(_) => 0,
        }
    }
}

// --- the effects -------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Magik,
    MagikGif,
    Deepfry,
    Jpeg,
    Pixelate,
    Swirl,
    Explode,
    Implode,
    Wave,
    Glitch,
    GlitchGif,
    Edges,
    Wide,
    Tall,
    MirrorLeft,
    MirrorRight,
    MirrorTop,
    MirrorBottom,
    Flip,
    Flop,
    Rotate,
    Invert,
    Grayscale,
    Hue,
    Blurple,
    Caption,
    Meme,
    Ascii,
    Spin,
    Shake,
    Zoom,
}

use Effect::*;

/// Every effect, in the order the picker offers them: the famous ones first,
/// then by group.
pub const ALL: &[Effect] = &[
    Magik,
    Deepfry,
    Caption,
    Meme,
    Jpeg,
    Pixelate,
    Glitch,
    Swirl,
    Explode,
    Implode,
    Wave,
    Edges,
    Wide,
    Tall,
    MirrorLeft,
    MirrorRight,
    MirrorTop,
    MirrorBottom,
    Flip,
    Flop,
    Rotate,
    Invert,
    Grayscale,
    Hue,
    Blurple,
    Ascii,
    Spin,
    Shake,
    Zoom,
    MagikGif,
    GlitchGif,
];

impl Effect {
    pub fn key(self) -> &'static str {
        match self {
            Magik => "magik",
            MagikGif => "magikgif",
            Deepfry => "deepfry",
            Jpeg => "jpeg",
            Pixelate => "pixelate",
            Swirl => "swirl",
            Explode => "explode",
            Implode => "implode",
            Wave => "wave",
            Glitch => "glitch",
            GlitchGif => "glitchgif",
            Edges => "edges",
            Wide => "wide",
            Tall => "tall",
            MirrorLeft => "mirror",
            MirrorRight => "mirror-right",
            MirrorTop => "mirror-top",
            MirrorBottom => "mirror-bottom",
            Flip => "flip",
            Flop => "flop",
            Rotate => "rotate",
            Invert => "invert",
            Grayscale => "grayscale",
            Hue => "hue",
            Blurple => "blurple",
            Caption => "caption",
            Meme => "meme",
            Ascii => "ascii",
            Spin => "spin",
            Shake => "shake",
            Zoom => "zoom",
        }
    }

    /// What the picker shows. Short, because Discord gives it one line.
    pub fn label(self) -> &'static str {
        match self {
            Magik => "Magik - the melty one",
            MagikGif => "Magik gif - melting, frame by frame",
            Deepfry => "Deepfry - crushed, blown out, sharpened",
            Jpeg => "JPEG - just the crushing",
            Pixelate => "Pixelate - blocks",
            Swirl => "Swirl - turn the middle",
            Explode => "Explode - push the middle out",
            Implode => "Implode - suck the middle in",
            Wave => "Wave - a ripple down it",
            Glitch => "Glitch - torn scan lines",
            GlitchGif => "Glitch gif - breaking, frame by frame",
            Edges => "Edges - the outline only",
            Wide => "Wide - stretched sideways",
            Tall => "Tall - stretched upwards",
            MirrorLeft => "Mirror - left half onto the right",
            MirrorRight => "Mirror right - right half onto the left",
            MirrorTop => "Mirror top - top half onto the bottom",
            MirrorBottom => "Mirror bottom - bottom half onto the top",
            Flip => "Flip - upside down",
            Flop => "Flop - back to front",
            Rotate => "Rotate - turn it (strength x 45 degrees)",
            Invert => "Invert - the negative",
            Grayscale => "Grayscale - no colour",
            Hue => "Hue - shift the colours round",
            Blurple => "Blurple - in Discord's own colours",
            Caption => "Caption - a white bar above with your words",
            Meme => "Meme - words on top and bottom (split on |)",
            Ascii => "Ascii - the picture as text",
            Spin => "Spin - a gif that turns",
            Shake => "Shake - a gif that judders",
            Zoom => "Zoom - a gif that punches in",
        }
    }

    pub fn group(self) -> &'static str {
        match self {
            Magik | MagikGif => "Melt",
            Deepfry | Jpeg | Pixelate => "Ruin",
            Swirl | Explode | Implode | Wave | Glitch | GlitchGif => "Warp",
            Edges | Invert | Grayscale | Hue | Blurple => "Colour",
            Wide | Tall | MirrorLeft | MirrorRight | MirrorTop | MirrorBottom | Flip | Flop | Rotate => "Shape",
            Caption | Meme | Ascii => "Words",
            Spin | Shake | Zoom => "Move",
        }
    }

    pub fn from_key(key: &str) -> Option<Effect> {
        let key = key.trim().to_ascii_lowercase();
        ALL.iter().copied().find(|e| e.key() == key)
    }

    /// Nothing to draw without words.
    pub fn wants_text(self) -> bool {
        matches!(self, Caption | Meme)
    }

    /// Makes an animation out of a still picture.
    pub fn makes_frames(self) -> bool {
        matches!(self, Spin | Shake | Zoom | MagikGif | GlitchGif)
    }

    /// Where the strength knob sits when nobody moves it. `rotate` is the odd
    /// one: two is a quarter turn, which is what people mean by rotate.
    pub fn default_strength(self) -> u8 {
        match self {
            Rotate => 2,
            // Half-crushed does not read as crushed; seven does.
            Jpeg => 7,
            _ => 5,
        }
    }
}

/// What the caller asks for.
#[derive(Clone, Debug)]
pub struct Job {
    pub effect: Effect,
    /// For `caption` and `meme`. Already stripped of pings by the caller, and
    /// stripped again here so the engine is safe on its own.
    pub text: Option<String>,
    pub strength: Option<u8>,
}

impl Job {
    pub fn new(effect: Effect) -> Job {
        Job { effect, text: None, strength: None }
    }

    fn strength(&self) -> u8 {
        self.strength.unwrap_or_else(|| self.effect.default_strength()).clamp(1, 10)
    }
}

// --- reading the picture -----------------------------------------------------------------------

/// Only these four go in, whatever a server says a file is.
pub fn readable(format: ImageFormat) -> bool {
    matches!(format, ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif)
}

/// The format these bytes really are, by their own magic numbers.
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    image::guess_format(bytes).ok().filter(|f| readable(*f))
}

/// Shrink to fit inside a square of `side`. Never enlarges.
pub fn fit_into(img: &RgbaImage, side: u32) -> RgbaImage {
    let side = side.max(1);
    let (w, h) = (img.width().max(1), img.height().max(1));
    if w <= side && h <= side {
        return img.clone();
    }
    let f = side as f32 / w.max(h) as f32;
    let nw = ((w as f32 * f).round() as u32).max(1);
    let nh = ((h as f32 * f).round() as u32).max(1);
    image::imageops::resize(img, nw, nh, FilterType::Lanczos3)
}

fn collect_frames<'a>(
    frames: image::Frames<'a>,
    max: usize,
    side: u32,
) -> Result<Vec<Frame>, Trouble> {
    let mut out: Vec<Frame> = Vec::new();
    // Read to the cap and stop. Truncating rather than sampling keeps the
    // memory bounded without having to know how long the animation is first;
    // a two-hundred-frame GIF comes back as its first sixty.
    for frame in frames.take(max.max(1)) {
        let frame = frame.map_err(|e| Trouble::Broke(format!("frame: {e}")))?;
        let (num, den) = frame.delay().numer_denom_ms();
        let ms = if den == 0 { MADE_DELAY as u32 } else { num / den.max(1) };
        out.push(Frame { img: fit_into(&frame.into_buffer(), side), delay: (ms as u16).max(MIN_DELAY) });
    }
    if out.is_empty() { Err(Trouble::NotAnImage) } else { Ok(out) }
}

/// The picture, as frames, already scaled to the working size. The flag says
/// whether it arrived animated.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<(Vec<Frame>, bool), Trouble> {
    let format = sniff(bytes).ok_or(Trouble::NotAnImage)?;
    match format {
        ImageFormat::Gif => {
            let dec = GifDecoder::new(Cursor::new(bytes)).map_err(|_| Trouble::NotAnImage)?;
            let frames = collect_frames(dec.into_frames(), limits.max_frames, limits.anim_side)?;
            let animated = frames.len() > 1;
            // A one-frame GIF is a still picture, and deserves the full size.
            if animated {
                Ok((frames, true))
            } else {
                let one = image::load_from_memory(bytes).map_err(|_| Trouble::NotAnImage)?.to_rgba8();
                Ok((vec![Frame { img: fit_into(&one, limits.max_side), delay: MADE_DELAY }], false))
            }
        }
        ImageFormat::WebP => {
            let dec = WebPDecoder::new(Cursor::new(bytes)).map_err(|_| Trouble::NotAnImage)?;
            if dec.has_animation() {
                let frames = collect_frames(dec.into_frames(), limits.max_frames, limits.anim_side)?;
                let animated = frames.len() > 1;
                return Ok((frames, animated));
            }
            let one = image::load_from_memory_with_format(bytes, ImageFormat::WebP).map_err(|_| Trouble::NotAnImage)?.to_rgba8();
            Ok((vec![Frame { img: fit_into(&one, limits.max_side), delay: MADE_DELAY }], false))
        }
        _ => {
            let one = image::load_from_memory_with_format(bytes, format).map_err(|_| Trouble::NotAnImage)?.to_rgba8();
            if one.width() == 0 || one.height() == 0 {
                return Err(Trouble::NotAnImage);
            }
            Ok((vec![Frame { img: fit_into(&one, limits.max_side), delay: MADE_DELAY }], false))
        }
    }
}

// --- doing the work ----------------------------------------------------------------------------

/// `magik` at its own working size, stretched back to the frame it came from.
fn magik_frame(img: &RgbaImage, strength: u8) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let small = fit_into(img, MAGIK_SIDE);
    let melted = seam::magik(&small, strength);
    if melted.width() == w && melted.height() == h {
        melted
    } else {
        image::imageops::resize(&melted, w.max(1), h.max(1), FilterType::Lanczos3)
    }
}

/// One frame through one effect. `step` is which frame of an animation this is,
/// which is what lets `spin` turn a little further each time and `glitch` break
/// differently.
fn one(img: &RgbaImage, job: &Job, step: usize, of: usize, limits: &Limits) -> Result<RgbaImage, Trouble> {
    let s = job.strength();
    let text = job.text.as_deref().unwrap_or("");
    let ramp = if of <= 1 { 0.0 } else { step as f32 / of as f32 };
    let out = match job.effect {
        Magik => magik_frame(img, s),
        // Ramped from barely-touched to well past where the knob sits, so the
        // loop actually goes somewhere. Cumulative melting would look better
        // still and costs one carve per frame per frame, which it is not worth.
        MagikGif => magik_frame(img, (1.0 + (s.saturating_add(5).min(10) as f32 - 1.0) * ramp).round().clamp(1.0, 10.0) as u8),
        Deepfry => fx::deepfry(img, s),
        Jpeg => fx::crush_times(img, fx::jpeg_quality(s), fx::jpeg_passes(s)),
        Pixelate => fx::pixelate(img, fx::pixel_block(img.width(), img.height(), s)),
        Swirl => fx::swirl(img, s),
        Explode => fx::bulge(img, s, true),
        Implode => fx::bulge(img, s, false),
        Wave => fx::wave(img, s),
        Glitch => fx::glitch(img, s, 0x5eed),
        GlitchGif => fx::glitch(img, s, 0x5eed + step as u64 * 7919),
        Edges => fx::edges(img),
        Wide => fit_into(&fx::stretch(img, 1.0 + 0.2 * s as f32, 1.0), limits.max_side),
        Tall => fit_into(&fx::stretch(img, 1.0, 1.0 + 0.2 * s as f32), limits.max_side),
        MirrorLeft => fx::mirror(img, fx::Half::Left),
        MirrorRight => fx::mirror(img, fx::Half::Right),
        MirrorTop => fx::mirror(img, fx::Half::Top),
        MirrorBottom => fx::mirror(img, fx::Half::Bottom),
        Flip => image::imageops::flip_vertical(img),
        Flop => image::imageops::flip_horizontal(img),
        Rotate => fx::rotate(img, s as f32 * 45.0),
        Invert => fx::invert(img),
        Grayscale => fx::grayscale(img),
        // Thirty-two, not thirty-six: ten would be a whole turn of the wheel
        // and would hand the picture back unchanged.
        Hue => fx::hue(img, s as f32 * 32.0),
        Blurple => fx::blurple(img),
        Caption => {
            if text.trim().is_empty() {
                return Err(Trouble::NeedsText);
            }
            words::caption(img, text)
        }
        Meme => {
            if text.trim().is_empty() {
                return Err(Trouble::NeedsText);
            }
            let (top, bottom) = words::split_meme(text);
            words::meme(img, &top, &bottom)
        }
        // Handled before we get here.
        Ascii => img.clone(),
        Spin => {
            // One canvas for every frame, big enough for the worst angle, or the
            // picture would breathe as it turned.
            let (bw, bh) = fx::turned_box(img.width(), img.height(), 45.0);
            let side = bw.max(bh);
            fx::rotate_on(img, ramp * 360.0, side, side)
        }
        Shake => {
            // Round a small ellipse twice per loop, which reads as a judder
            // rather than a drift, and comes back exactly where it started.
            let reach = ((img.width().min(img.height()) as f32 * 0.022 * s as f32).round() as i32).max(2);
            let angle = ramp * std::f32::consts::TAU * 2.0;
            fx::shifted(img, (angle.cos() * reach as f32).round() as i32, (angle.sin() * reach as f32 * 0.7).round() as i32)
        }
        Zoom => {
            // In and back out again, so the loop does not jump.
            let there = if ramp <= 0.5 { ramp * 2.0 } else { (1.0 - ramp) * 2.0 };
            fx::zoomed(img, 1.0 + 0.12 * s as f32 * there)
        }
    };
    Ok(out)
}

/// Effect applied, frames and all, within the budget.
pub fn run(bytes: &[u8], job: &Job, limits: &Limits) -> Result<Out, Trouble> {
    let started = Instant::now();
    if job.effect.wants_text() && job.text.as_deref().unwrap_or("").trim().is_empty() {
        return Err(Trouble::NeedsText);
    }
    let (frames, animated) = decode(bytes, limits)?;

    if job.effect == Ascii {
        let first = &frames.first().ok_or(Trouble::NotAnImage)?.img;
        return Ok(Out::Text(words::ascii(first, 58)));
    }

    // Three shapes of job. An animated input goes through frame by frame. A
    // still picture with an animating effect becomes frames. Anything else is
    // one frame in, one frame out.
    let (input, count) = if animated {
        (frames, 0)
    } else if job.effect.makes_frames() {
        let side = limits.anim_side;
        let one = Frame { img: fit_into(&frames[0].img, side), delay: MADE_DELAY };
        let count = MADE_FRAMES.min(limits.max_frames.max(2));
        (vec![one], count)
    } else {
        (frames, 1)
    };

    let mut out: Vec<Frame> = Vec::new();
    if count == 0 {
        // Animated in, animated out: one pass per frame it arrived with.
        let of = input.len();
        for (i, frame) in input.iter().enumerate() {
            if !out.is_empty() && started.elapsed() > limits.budget {
                tracing::warn!("image: {} ran out of time after {} of {} frames", job.effect.key(), out.len(), of);
                break;
            }
            out.push(Frame { img: one(&frame.img, job, i, of, limits)?, delay: frame.delay });
        }
    } else if count == 1 {
        out.push(Frame { img: one(&input[0].img, job, 0, 1, limits)?, delay: input[0].delay });
    } else {
        for i in 0..count {
            if !out.is_empty() && started.elapsed() > limits.budget {
                tracing::warn!("image: {} ran out of time after {} of {} frames", job.effect.key(), out.len(), count);
                break;
            }
            out.push(Frame { img: one(&input[0].img, job, i, count, limits)?, delay: MADE_DELAY });
        }
    }
    if out.is_empty() {
        return Err(Trouble::TooSlow);
    }
    // One frame left of what should have been an animation is a still, and a
    // still is better as a PNG.
    encode_fitting(out, limits)
}

// --- writing it out ----------------------------------------------------------------------------

fn encode_png(img: &RgbaImage) -> Result<Vec<u8>, Trouble> {
    let mut bytes: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(img.clone())
        .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
        .map_err(|e| Trouble::Broke(format!("png: {e}")))?;
    Ok(bytes)
}

/// Written against the `gif` crate rather than `image`'s wrapper for one
/// reason: the disposal method. A `spin` frame has transparent corners, and
/// with GIF's default "leave the last frame where it is" the earlier frames
/// show through those corners and the animation smears into a pile. `image`
/// does not expose the field; the crate underneath it does, and it is the same
/// crate `image` already depends on.
fn encode_gif(frames: &[Frame]) -> Result<Vec<u8>, Trouble> {
    let first = frames.first().ok_or_else(|| Trouble::Broke("no frames".into()))?;
    let (w, h) = (first.img.width(), first.img.height());
    if w == 0 || h == 0 || w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
        return Err(Trouble::Broke(format!("gif size {w}x{h}")));
    }
    let mut bytes: Vec<u8> = Vec::new();
    {
        let mut enc = gif::Encoder::new(&mut bytes, w as u16, h as u16, &[]).map_err(|e| Trouble::Broke(format!("gif: {e}")))?;
        enc.set_repeat(gif::Repeat::Infinite).map_err(|e| Trouble::Broke(format!("gif repeat: {e}")))?;
        for f in frames {
            if f.img.dimensions() != (w, h) {
                return Err(Trouble::Broke("frames of different sizes".into()));
            }
            let mut raw = f.img.clone().into_raw();
            // Speed 20 of 30: the quantiser is the slow part of a long GIF, and
            // nobody studies the palette of a shaking avatar.
            let mut frame = gif::Frame::from_rgba_speed(w as u16, h as u16, &mut raw, 20);
            // GIF counts in hundredths of a second, and will not show a frame
            // for less than two of them.
            frame.delay = (f.delay.max(MIN_DELAY) / 10).max(2);
            frame.dispose = gif::DisposalMethod::Background;
            enc.write_frame(&frame).map_err(|e| Trouble::Broke(format!("gif frame: {e}")))?;
        }
    }
    Ok(bytes)
}

/// Every other frame, with the delays added up so the animation still runs at
/// the same speed - the cheapest way to halve a GIF that is too heavy.
fn thinned(frames: &[Frame]) -> Vec<Frame> {
    let mut out: Vec<Frame> = Vec::new();
    for (i, f) in frames.iter().enumerate() {
        if i % 2 == 0 {
            out.push(f.clone());
        } else if let Some(last) = out.last_mut() {
            last.delay = last.delay.saturating_add(f.delay).min(1000);
        }
    }
    out
}

fn scaled(frames: &[Frame], by: f32) -> Vec<Frame> {
    frames
        .iter()
        .map(|f| {
            let w = ((f.img.width() as f32 * by).round() as u32).max(1);
            let h = ((f.img.height() as f32 * by).round() as u32).max(1);
            Frame { img: image::imageops::resize(&f.img, w, h, FilterType::Lanczos3), delay: f.delay }
        })
        .collect()
}

/// Encode, and if it is too heavy for Discord, make it lighter until it isn't:
/// frames first (nobody misses every other frame of a shake), then size. Never
/// fails for being big unless there is nothing left to take away.
pub fn encode_fitting(frames: Vec<Frame>, limits: &Limits) -> Result<Out, Trouble> {
    let cap = limits.upload_cap.max(64 * 1024);
    let mut frames = frames;
    for round in 0..10 {
        let bytes = if frames.len() == 1 { encode_png(&frames[0].img)? } else { encode_gif(&frames)? };
        if bytes.len() <= cap {
            return Ok(if frames.len() == 1 { Out::Png(bytes) } else { Out::Gif(bytes) });
        }
        tracing::info!(
            "image: {} bytes over the {} cap on round {} ({} frames, {}x{})",
            bytes.len(),
            cap,
            round,
            frames.len(),
            frames[0].img.width(),
            frames[0].img.height()
        );
        let side = frames[0].img.width().max(frames[0].img.height());
        if frames.len() > 4 {
            frames = thinned(&frames);
        } else if side > 48 {
            frames = scaled(&frames, 0.7);
        } else {
            return Err(Trouble::TooBig);
        }
    }
    Err(Trouble::TooBig)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn sample(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_fn(w, h, |x, y| {
            image::Rgba([((x * 3 + y) % 256) as u8, ((y * 5) % 256) as u8, ((x ^ y) % 256) as u8, 255])
        })
    }

    fn png_of(img: &RgbaImage) -> Vec<u8> {
        encode_png(img).expect("png")
    }

    fn gif_of(n: usize, w: u32, h: u32) -> Vec<u8> {
        let frames: Vec<Frame> = (0..n)
            .map(|i| Frame { img: RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x + i as u32) as u8, y as u8, 60, 255])), delay: 40 })
            .collect();
        encode_gif(&frames).expect("gif")
    }

    #[test]
    fn every_effect_has_a_key_a_label_and_a_group_and_no_two_share_a_key() {
        let mut seen = std::collections::HashSet::new();
        for e in ALL {
            assert!(seen.insert(e.key()), "{} is listed twice", e.key());
            assert!(!e.label().trim().is_empty() && !e.group().trim().is_empty(), "{} needs describing", e.key());
            assert!(e.label().len() <= 100, "{} has a label Discord will not take", e.key());
            assert_eq!(Effect::from_key(e.key()), Some(*e));
            assert_eq!(Effect::from_key(&e.key().to_uppercase()), Some(*e));
        }
        assert_eq!(Effect::from_key("nonsense"), None);
        // The list the brief asked for, all present.
        for want in [
            "magik", "deepfry", "jpeg", "pixelate", "swirl", "explode", "implode", "wave", "glitch", "edges", "wide",
            "tall", "mirror", "mirror-right", "mirror-top", "mirror-bottom", "flip", "flop", "rotate", "invert",
            "grayscale", "hue", "blurple", "caption", "meme", "ascii", "spin", "shake", "zoom", "magikgif", "glitchgif",
        ] {
            assert!(Effect::from_key(want).is_some(), "{} is missing", want);
        }
    }

    #[test]
    fn a_png_goes_in_and_a_png_comes_out_at_the_size_it_went_in() {
        let img = sample(200, 150);
        let bytes = png_of(&img);
        let out = run(&bytes, &Job::new(Grayscale), &Limits::default()).expect("grayscale");
        let Out::Png(png) = out else { panic!("expected a png") };
        let back = image::load_from_memory(&png).expect("valid png").to_rgba8();
        assert_eq!(back.dimensions(), (200, 150));
    }

    /// Every effect, on one ordinary picture: a real image comes back, of a
    /// size that makes sense, and nothing panics on the way.
    #[test]
    fn every_effect_produces_a_real_picture() {
        let img = sample(160, 120);
        let bytes = png_of(&img);
        let limits = Limits { max_frames: 6, ..Limits::default() };
        for e in ALL {
            let mut job = Job::new(*e);
            if e.wants_text() {
                job.text = Some("top | bottom".into());
            }
            let out = run(&bytes, &job, &limits).unwrap_or_else(|t| panic!("{}: {:?}", e.key(), t));
            match out {
                Out::Text(t) => assert!(t.starts_with("```"), "{} gave odd text", e.key()),
                Out::Png(b) => {
                    let back = image::load_from_memory(&b).unwrap_or_else(|_| panic!("{} gave a bad png", e.key()));
                    assert!(back.width() > 0 && back.height() > 0, "{} gave nothing", e.key());
                    assert!(!e.makes_frames(), "{} should have animated", e.key());
                }
                Out::Gif(b) => {
                    let dec = GifDecoder::new(Cursor::new(&b)).unwrap_or_else(|_| panic!("{} gave a bad gif", e.key()));
                    let n = dec.into_frames().count();
                    assert!(n > 1, "{} gave a one-frame gif", e.key());
                }
            }
        }
    }

    /// The sizes that break things: one pixel, one enormous picture, no colour,
    /// a hole in the middle.
    #[test]
    fn the_awkward_pictures_all_survive() {
        let one = png_of(&RgbaImage::from_pixel(1, 1, image::Rgba([200, 10, 10, 255])));
        let huge = png_of(&sample(2400, 1800));
        let grey = {
            let g = image::GrayImage::from_fn(90, 70, |x, y| image::Luma([((x + y) % 256) as u8]));
            let mut b = Vec::new();
            image::DynamicImage::ImageLuma8(g).write_to(&mut Cursor::new(&mut b), ImageFormat::Png).expect("grey png");
            b
        };
        // A solid disc on nothing at all: the transparent case that matters.
        let clear = png_of(&RgbaImage::from_fn(80, 80, |x, y| {
            let (dx, dy) = (x as f32 - 40.0, y as f32 - 40.0);
            let inside = dx * dx + dy * dy < 30.0 * 30.0;
            image::Rgba([40, 180, 220, if inside { 255 } else { 0 }])
        }));
        let limits = Limits { max_frames: 4, ..Limits::default() };
        for (what, bytes) in [("1x1", &one), ("huge", &huge), ("grey", &grey), ("transparent", &clear)] {
            for e in ALL {
                let mut job = Job::new(*e);
                if e.wants_text() {
                    job.text = Some("a | b".into());
                }
                let out = run(bytes, &job, &limits).unwrap_or_else(|t| panic!("{} on {}: {:?}", e.key(), what, t));
                assert!(out.weight() < limits.upload_cap, "{} on {} came out too heavy", e.key(), what);
            }
        }
    }

    #[test]
    fn a_huge_picture_is_scaled_down_before_anything_touches_it() {
        let limits = Limits { max_side: 200, ..Limits::default() };
        let (frames, animated) = decode(&png_of(&sample(2000, 1000)), &limits).expect("decode");
        assert!(!animated);
        assert_eq!(frames[0].img.dimensions(), (200, 100));
    }

    #[test]
    fn a_single_frame_gif_is_treated_as_a_still() {
        let (frames, animated) = decode(&gif_of(1, 60, 40), &Limits::default()).expect("decode");
        assert!(!animated, "one frame is not an animation");
        assert_eq!(frames.len(), 1);
        let out = run(&gif_of(1, 60, 40), &Job::new(Invert), &Limits::default()).expect("invert");
        assert!(matches!(out, Out::Png(_)), "a still should come back as a png");
    }

    #[test]
    fn a_two_hundred_frame_gif_is_cut_to_the_cap_and_still_works() {
        let bytes = gif_of(200, 48, 48);
        let limits = Limits { max_frames: 12, ..Limits::default() };
        let (frames, animated) = decode(&bytes, &limits).expect("decode");
        assert!(animated);
        assert_eq!(frames.len(), 12, "the frame cap was not applied");
        let out = run(&bytes, &Job::new(Invert), &limits).expect("invert a gif");
        let Out::Gif(gif) = out else { panic!("a gif in should be a gif out") };
        let n = GifDecoder::new(Cursor::new(&gif)).expect("valid gif").into_frames().count();
        assert!(n > 1 && n <= 12, "{} frames came back", n);
    }

    #[test]
    fn an_animated_gif_stays_animated_through_every_still_effect() {
        let bytes = gif_of(4, 40, 40);
        let limits = Limits { max_frames: 8, ..Limits::default() };
        for e in [Magik, Deepfry, Swirl, Invert, Blurple, Edges, Pixelate] {
            let out = run(&bytes, &Job::new(e), &limits).unwrap_or_else(|t| panic!("{}: {:?}", e.key(), t));
            assert!(matches!(out, Out::Gif(_)), "{} flattened the animation", e.key());
        }
    }

    #[test]
    fn rubbish_is_refused_rather_than_guessed_at() {
        for junk in [&b"not a picture at all"[..], &[][..], &b"<html><body>hi</body></html>"[..]] {
            assert_eq!(run(junk, &Job::new(Invert), &Limits::default()).err(), Some(Trouble::NotAnImage), "junk was accepted");
        }
    }

    #[test]
    fn caption_and_meme_say_so_when_there_are_no_words() {
        let bytes = png_of(&sample(60, 60));
        for e in [Caption, Meme] {
            assert_eq!(run(&bytes, &Job::new(e), &Limits::default()).err(), Some(Trouble::NeedsText));
            let blank = Job { effect: e, text: Some("   ".into()), strength: None };
            assert_eq!(run(&bytes, &blank, &Limits::default()).err(), Some(Trouble::NeedsText));
        }
        assert!(!Trouble::NeedsText.plainly().is_empty());
    }

    /// A result always fits the cap: frames go first, then size.
    #[test]
    fn the_result_is_made_to_fit_discords_limit() {
        // Noise, which no encoder can squash, at a cap far below what it needs.
        let noisy = |i: usize| {
            RgbaImage::from_fn(300, 300, |x, y| {
                let v = ((x * 7919 + y * 104_729 + i as u32 * 31) % 251) as u8;
                image::Rgba([v, v.wrapping_mul(3), v.wrapping_mul(7), 255])
            })
        };
        let frames: Vec<Frame> = (0..16).map(|i| Frame { img: noisy(i), delay: 60 }).collect();
        let limits = Limits { upload_cap: 64 * 1024, ..Limits::default() };
        let out = encode_fitting(frames, &limits).expect("something should fit");
        assert!(out.weight() <= limits.upload_cap, "{} bytes over a {} cap", out.weight(), limits.upload_cap);
        // And the same for a single heavy still.
        let out = encode_fitting(vec![Frame { img: noisy(0), delay: 60 }], &limits).expect("a still should fit too");
        assert!(out.weight() <= limits.upload_cap);
    }

    #[test]
    fn thinning_keeps_the_running_time() {
        let frames: Vec<Frame> = (0..8).map(|_| Frame { img: sample(4, 4), delay: 50 }).collect();
        let thin = thinned(&frames);
        assert_eq!(thin.len(), 4);
        assert_eq!(thin.iter().map(|f| u32::from(f.delay)).sum::<u32>(), 400);
    }

    #[test]
    fn a_job_that_runs_out_of_time_still_hands_back_what_it_finished() {
        let bytes = gif_of(40, 200, 200);
        // A budget of nothing: the first frame is always done, then it stops.
        let limits = Limits { budget: Duration::ZERO, max_frames: 40, ..Limits::default() };
        let out = run(&bytes, &Job::new(Magik), &limits).expect("one frame at least");
        assert!(matches!(out, Out::Png(_)), "one frame of a gif is a still");
    }

    #[test]
    fn strength_is_clamped_and_rotate_starts_at_a_quarter_turn() {
        assert_eq!(Job { effect: Magik, text: None, strength: Some(0) }.strength(), 1);
        assert_eq!(Job { effect: Magik, text: None, strength: Some(99) }.strength(), 10);
        assert_eq!(Job::new(Rotate).strength(), 2);
        assert_eq!(Job::new(Magik).strength(), 5);
    }

    #[test]
    fn only_the_four_formats_are_read() {
        assert!(readable(ImageFormat::Png) && readable(ImageFormat::Jpeg));
        assert!(readable(ImageFormat::WebP) && readable(ImageFormat::Gif));
        assert!(!readable(ImageFormat::Bmp) && !readable(ImageFormat::Tiff));
        assert_eq!(sniff(b"GIF89a....."), Some(ImageFormat::Gif));
        assert_eq!(sniff(b"nope"), None);
    }

    #[test]
    fn rotate_turns_by_the_strength_it_was_given() {
        let bytes = png_of(&sample(80, 40));
        let out = run(&bytes, &Job { effect: Rotate, text: None, strength: Some(2) }, &Limits::default()).expect("rotate");
        let Out::Png(png) = out else { panic!("png") };
        let back = image::load_from_memory(&png).expect("valid").to_rgba8();
        assert_eq!(back.dimensions(), (40, 80), "a quarter turn should swap the sides");
    }

    #[test]
    fn wide_and_tall_stay_inside_the_size_cap() {
        let limits = Limits { max_side: 120, ..Limits::default() };
        for (e, bytes) in [(Wide, png_of(&sample(120, 120))), (Tall, png_of(&sample(120, 120)))] {
            let out = run(&bytes, &Job::new(e), &limits).expect("stretch");
            let Out::Png(png) = out else { panic!("png") };
            let back = image::load_from_memory(&png).expect("valid").to_rgba8();
            assert!(back.width() <= 120 && back.height() <= 120, "{} came out {:?}", e.key(), back.dimensions());
            assert_ne!(back.width(), back.height(), "{} did not change the shape", e.key());
        }
    }
}

/// Rendering a sample of every effect to look at.
///
/// Not part of the suite - a picture cannot assert anything about itself, and
/// the only useful check is a person looking at the sheet. Run it by hand:
///
/// ```sh
/// IMAGEFX_SHEET=/some/dir cargo test --bin vizier imagefx_engine::sheet -- --ignored --nocapture
/// ```
///
/// It writes one file per effect plus `contact-sheet.png`, which is every
/// effect at once with its name under it, and prints how long each one took.
#[cfg(test)]
mod sheet {
    use super::tests::*;
    use super::*;

    /// A frame from a film in the movie bank: a face, skin tones, hair, a soft
    /// background and a hard edge - everything the effects are judged on. A
    /// synthetic gradient would make every one of them look fine.
    fn source() -> RgbaImage {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("moviebank/images/m276_2.jpg");
        let img = image::open(&path).expect("the sample frame").to_rgba8();
        // A square around the face, at the size an avatar arrives as.
        let side = img.height();
        let left = (img.width() as f32 * 0.52).round() as u32;
        let crop = image::imageops::crop_imm(&img, left.min(img.width() - side), 0, side, side).to_image();
        // 512 square: exactly what a Discord avatar arrives as, so the times
        // this test prints are the times a real command takes.
        image::imageops::resize(&crop, 512, 512, FilterType::Lanczos3)
    }

    /// One result, letterboxed into a square tile on a dark ground, with its
    /// name in a bar above it.
    fn tile(img: &RgbaImage, name: &str, side: u32) -> RgbaImage {
        let fitted = fit_into(img, side);
        let mut cell = RgbaImage::from_pixel(side, side, image::Rgba([22, 23, 28, 255]));
        let (ox, oy) = ((side - fitted.width()) / 2, (side - fitted.height()) / 2);
        // Over a dark ground, so transparency shows as the dark rather than as
        // white - which is how Discord shows it too.
        image::imageops::overlay(&mut cell, &fitted, i64::from(ox), i64::from(oy));
        words::caption(&cell, name)
    }

    #[test]
    #[ignore = "writes pictures for a person to look at"]
    fn contact_sheet() {
        let dir = std::path::PathBuf::from(std::env::var("IMAGEFX_SHEET").unwrap_or_else(|_| "/tmp/imagefx".into()));
        std::fs::create_dir_all(&dir).expect("somewhere to write");
        let src = source();
        let mut bytes: Vec<u8> = Vec::new();
        image::DynamicImage::ImageRgba8(src.clone())
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("source png");
        std::fs::write(dir.join("000-source.png"), &bytes).expect("write source");

        let limits = Limits::default();
        let mut tiles: Vec<RgbaImage> = vec![tile(&src, "the original", 240)];
        println!("\n{:<14} {:>8}  {:>10}  {}", "effect", "ms", "bytes", "out");
        println!("{}", "-".repeat(52));
        for e in ALL {
            let mut job = Job::new(*e);
            if e.wants_text() {
                job.text = Some(match e {
                    Meme => "one does not simply | run imagemagick".into(),
                    _ => "loduchand ke dil se".into(),
                });
            }
            let began = Instant::now();
            let out = run(&bytes, &job, &limits).unwrap_or_else(|t| panic!("{}: {:?}", e.key(), t));
            let took = began.elapsed().as_millis();
            let (kind, weight, shown) = match &out {
                Out::Png(b) => {
                    std::fs::write(dir.join(format!("{}.png", e.key())), b).expect("write");
                    ("png", b.len(), image::load_from_memory(b).expect("png").to_rgba8())
                }
                Out::Gif(b) => {
                    std::fs::write(dir.join(format!("{}.gif", e.key())), b).expect("write");
                    let first = GifDecoder::new(Cursor::new(b))
                        .expect("gif")
                        .into_frames()
                        .next()
                        .expect("a frame")
                        .expect("a frame")
                        .into_buffer();
                    ("gif", b.len(), first)
                }
                Out::Text(t) => {
                    std::fs::write(dir.join(format!("{}.txt", e.key())), t).expect("write");
                    println!("{:<14} {:>8}  {:>10}  text", e.key(), took, t.len());
                    // The ascii, rendered small, so it has a tile like the rest.
                    let small = fit_into(&src, 120);
                    tiles.push(tile(&fx::grayscale(&fx::pixelate(&small, 4)), "ascii (see the .txt)", 240));
                    continue;
                }
            };
            println!("{:<14} {:>8}  {:>10}  {}", e.key(), took, weight, kind);
            tiles.push(tile(&shown, e.key(), 240));
        }

        // Everything at once, six across.
        const ACROSS: u32 = 6;
        const PAD: u32 = 10;
        let cell_w = tiles.iter().map(|t| t.width()).max().unwrap_or(240);
        let cell_h = tiles.iter().map(|t| t.height()).max().unwrap_or(280);
        let rows = tiles.len().div_ceil(ACROSS as usize) as u32;
        let mut board = RgbaImage::from_pixel(
            ACROSS * (cell_w + PAD) + PAD,
            rows * (cell_h + PAD) + PAD,
            image::Rgba([13, 14, 17, 255]),
        );
        for (i, t) in tiles.iter().enumerate() {
            let (col, row) = (i as u32 % ACROSS, i as u32 / ACROSS);
            let x = PAD + col * (cell_w + PAD);
            let y = PAD + row * (cell_h + PAD);
            image::imageops::overlay(&mut board, t, i64::from(x), i64::from(y));
        }
        let path = dir.join("contact-sheet.png");
        board.save(&path).expect("write the sheet");
        println!("\ncontact sheet: {}", path.display());

        // The strength knob, on the effects where it decides how the thing
        // reads: one row each, one to ten across.
        let mut rows: Vec<RgbaImage> = Vec::new();
        for e in [Magik, Deepfry, Jpeg, Pixelate, Swirl, Explode, Implode, Wave, Glitch, Hue] {
            let mut cells: Vec<RgbaImage> = Vec::new();
            for strength in 1..=10u8 {
                let job = Job { effect: e, text: None, strength: Some(strength) };
                let out = run(&bytes, &job, &limits).unwrap_or_else(|t| panic!("{} at {}: {:?}", e.key(), strength, t));
                let Out::Png(b) = out else { panic!("{} should be a still", e.key()) };
                let shown = image::load_from_memory(&b).expect("png").to_rgba8();
                cells.push(tile(&shown, &format!("{} {}", e.key(), strength), 150));
            }
            let (cw, ch) = (cells[0].width(), cells.iter().map(|c| c.height()).max().unwrap_or(180));
            let mut row = RgbaImage::from_pixel(10 * (cw + 6) + 6, ch + 12, image::Rgba([13, 14, 17, 255]));
            for (i, c) in cells.iter().enumerate() {
                image::imageops::overlay(&mut row, c, i64::from(6 + i as u32 * (cw + 6)), 6);
            }
            rows.push(row);
        }
        let width = rows.iter().map(|r| r.width()).max().unwrap_or(1);
        let height: u32 = rows.iter().map(|r| r.height()).sum();
        let mut ladder = RgbaImage::from_pixel(width, height, image::Rgba([13, 14, 17, 255]));
        let mut y = 0i64;
        for r in &rows {
            image::imageops::overlay(&mut ladder, r, 0, y);
            y += i64::from(r.height());
        }
        let path = dir.join("strengths.png");
        ladder.save(&path).expect("write the ladder");
        println!("strength ladder: {}", path.display());

        // The animations, laid out flat: a gif cannot be judged from its first
        // frame, and its first frame is the one that looks least like anything.
        let mut strips: Vec<RgbaImage> = Vec::new();
        for e in [Spin, Shake, Zoom, MagikGif, GlitchGif] {
            let out = run(&bytes, &Job::new(e), &limits).unwrap_or_else(|t| panic!("{}: {:?}", e.key(), t));
            let Out::Gif(b) = out else { panic!("{} should animate", e.key()) };
            let frames: Vec<RgbaImage> = GifDecoder::new(Cursor::new(&b))
                .expect("gif")
                .into_frames()
                .map(|f| f.expect("frame").into_buffer())
                .collect();
            let step = (frames.len() / 7).max(1);
            let cells: Vec<RgbaImage> = frames
                .iter()
                .step_by(step)
                .take(7)
                .enumerate()
                .map(|(i, f)| tile(f, &format!("{} f{}", e.key(), i * step), 150))
                .collect();
            let (cw, ch) = (cells[0].width(), cells.iter().map(|c| c.height()).max().unwrap_or(180));
            let mut strip = RgbaImage::from_pixel(7 * (cw + 6) + 6, ch + 12, image::Rgba([13, 14, 17, 255]));
            for (i, c) in cells.iter().enumerate() {
                image::imageops::overlay(&mut strip, c, i64::from(6 + i as u32 * (cw + 6)), 6);
            }
            strips.push(strip);
        }
        let width = strips.iter().map(|r| r.width()).max().unwrap_or(1);
        let height: u32 = strips.iter().map(|r| r.height()).sum();
        let mut reel = RgbaImage::from_pixel(width, height, image::Rgba([13, 14, 17, 255]));
        let mut y = 0i64;
        for r in &strips {
            image::imageops::overlay(&mut reel, r, 0, y);
            y += i64::from(r.height());
        }
        let path = dir.join("frames.png");
        reel.save(&path).expect("write the reel");
        println!("animation frames: {}", path.display());

        // A picture with real holes in it - a house crest - through the effects
        // most likely to smear grey into them or fill them in.
        let crest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/channels/discord/crests/ravenclaw.png");
        let crest_bytes = std::fs::read(&crest).expect("a crest");
        let mut cells: Vec<RgbaImage> = Vec::new();
        {
            let shown = image::load_from_memory(&crest_bytes).expect("crest").to_rgba8();
            cells.push(tile(&shown, "crest (as it is)", 150));
        }
        for e in [Magik, Swirl, Explode, Implode, Wave, Rotate, Edges, Blurple, Caption, Pixelate, Spin] {
            let mut job = Job::new(e);
            if e.wants_text() {
                job.text = Some("ravenclaw".into());
            }
            let out = run(&crest_bytes, &job, &limits).unwrap_or_else(|t| panic!("{} on the crest: {:?}", e.key(), t));
            let shown = match &out {
                Out::Png(b) => image::load_from_memory(b).expect("png").to_rgba8(),
                Out::Gif(b) => GifDecoder::new(Cursor::new(b))
                    .expect("gif")
                    .into_frames()
                    .nth(3)
                    .expect("a frame")
                    .expect("a frame")
                    .into_buffer(),
                Out::Text(_) => continue,
            };
            cells.push(tile(&shown, e.key(), 150));
        }
        let (cw, ch) = (cells[0].width(), cells.iter().map(|c| c.height()).max().unwrap_or(180));
        let across = 6u32;
        let rows_n = (cells.len() as u32).div_ceil(across);
        let mut holes = RgbaImage::from_pixel(across * (cw + 6) + 6, rows_n * (ch + 6) + 6, image::Rgba([13, 14, 17, 255]));
        for (i, c) in cells.iter().enumerate() {
            let (col, row) = (i as u32 % across, i as u32 / across);
            image::imageops::overlay(&mut holes, c, i64::from(6 + col * (cw + 6)), i64::from(6 + row * (ch + 6)));
        }
        let path = dir.join("transparency.png");
        holes.save(&path).expect("write the transparency sheet");
        println!("transparency: {}\n", path.display());
    }
}
