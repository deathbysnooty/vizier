//! The /fight and /battle cards.
//!
//! Two fighters facing off with their avatars, and the champion card at the end
//! of a battle royale. Drawing is CPU work: callers run it on a blocking thread.
//!
//! Everything is painted from paths and gradients: a lit floor, a glow behind
//! each fighter in their own colour, and the portraits standing over their own
//! shadows. Most people see these on a phone, so the shapes are big, the
//! contrast is high, and nothing is finer than about two pixels.
//!
//! Both cards take their fonts from the list `awards::fonts()` holds, so a
//! caller must not already be holding that lock when it calls in here.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Stretch, Style as FontStyle, SwashCache,
    Weight, Wrap,
};
use tiny_skia::{
    Color as SkColor, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint,
    PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader, SpreadMode, Stroke, StrokeDash, Transform,
};

use super::awards_card::{avatar_pixmap, blend_rect, fill_circle, paint, rrect};
pub use super::battle_theme::Theme;

/// One side of a fight. `avatar` is the raw bytes of the member's profile
/// picture, already downloaded; `None` draws a blank silhouette.
pub struct Fighter {
    pub name: String,
    pub avatar: Option<Vec<u8>>,
    /// Health left, 0..=max_hp.
    pub hp: u32,
    pub max_hp: u32,
    /// Their house, drawn as a crest badge on the portrait. `None` for anyone
    /// unsorted or stepped out.
    pub house: Option<&'static super::house::House>,
}

/// How far along a fight is when the card is drawn.
pub enum Outcome {
    /// Still going: both sides lit, no result shown.
    Open,
    /// Index 0 (left) or 1 (right) has won.
    Won(usize),
}

pub struct Fight<'a> {
    /// "Round 2 · quarter-final", or "Challenge" for a /fight.
    pub stage: String,
    pub left: &'a Fighter,
    pub right: &'a Fighter,
    /// The Hinglish line for this exchange.
    pub line: String,
    pub outcome: Outcome,
    /// The exchange just shown: which side was hit (0 left, 1 right) and the
    /// change to their HP - negative for damage, positive for a heal.
    pub hit: Option<(usize, i32)>,
    pub theme: Theme,
}

pub struct Champion<'a> {
    pub who: &'a Fighter,
    /// "12 warriors, 4 rounds, 1 champion".
    pub subtitle: String,
    pub line: String,
    pub theme: Theme,
}

const BG: [u8; 3] = [15, 16, 20];
const PANEL: [u8; 3] = [30, 32, 38];
const INK: [u8; 3] = [244, 245, 247];
const MUTED: [u8; 3] = [148, 154, 166];
const WARM: [u8; 3] = [251, 113, 133];
const COOL: [u8; 3] = [167, 139, 250];
const GOLD: [u8; 3] = [241, 196, 15];
const STAMP: [u8; 3] = [229, 72, 77];
/// Type that sits on gold or on any other lit fill.
const ON_LIGHT: [u8; 3] = [18, 19, 24];

/// Gold caught in light, top to bottom: highlight, face, shaded core, shine.
const METAL: [(f32, [u8; 3]); 5] = [
    (0.0, [255, 247, 214]),
    (0.30, [246, 206, 74]),
    (0.52, [176, 122, 10]),
    (0.66, [255, 230, 138]),
    (1.0, [201, 146, 14]),
];

/// The eight offsets that make a dark outline round a word.
const OUTLINE: [(f32, f32); 8] =
    [(-3.0, 0.0), (3.0, 0.0), (0.0, -3.0), (0.0, 3.0), (-2.0, -2.0), (2.0, -2.0), (-2.0, 2.0), (2.0, 2.0)];

/// Ring band, and the dark gap between it and the picture.
const RING: f32 = 9.0;
const GAP: f32 = 5.0;

// Fight card. The portraits sit high, with the name plate, the result pill and
// the line panel stacked under them.
const FIGHT_W: f32 = 1000.0;
const FIGHT_H: f32 = 490.0;
const AV: f32 = 210.0;
const FIGHT_CY: f32 = 143.0;
const LEFT_CX: f32 = 190.0;
const RIGHT_CX: f32 = 810.0;
const PLATE_Y: f32 = 276.0;
const PLATE_H: f32 = 44.0;
const BAR_W: f32 = 260.0;
const BAR_H: f32 = 22.0;
const BAR_Y: f32 = 328.0;
const PILL_Y: f32 = 360.0;
const PILL_H: f32 = 32.0;
const PANEL_X: f32 = 60.0;
const PANEL_Y: f32 = 404.0;
const PANEL_W: f32 = 880.0;
const PANEL_H: f32 = 72.0;
const PANEL_PAD: f32 = 36.0;

// Champion card.
const CHAMP_W: f32 = 1000.0;
const CHAMP_H: f32 = 580.0;
const CHAMP_AV: f32 = 300.0;
const CHAMP_CY: f32 = 232.0;
/// House crest badges, on the portrait's shoulder.
const FIGHT_BADGE_R: f32 = 36.0;
const CHAMP_BADGE_R: f32 = 50.0;

/// What a theme changes in colour and words. The scenery each world adds on
/// top is painted by `backdrop` and friends; Classic adds none, so its numbers
/// here are the card as it has always been.
struct Look {
    /// The floor, top to bottom. One stop is a flat fill.
    floor: &'static [(f32, [u8; 3])],
    /// Each side's glow and ring colour.
    left: [u8; 3],
    right: [u8; 3],
    /// The soft seam down the middle, and its bright core.
    seam: ([u8; 3], [u8; 3]),
    /// Words before the stage on the chip, and the champion's ribbon.
    prefix: Option<&'static str>,
    title: &'static str,
    /// The champion card: the deep light behind them, the pool under their
    /// name, the tints the rays cycle through and the confetti.
    halo: [u8; 3],
    pool: [u8; 3],
    rays: &'static [[u8; 3]],
    confetti: [[u8; 3]; 4],
}

fn look(theme: Theme) -> Look {
    let classic = Look {
        floor: &[(0.0, BG)],
        left: WARM,
        right: COOL,
        seam: ([196, 138, 214], [246, 228, 255]),
        prefix: None,
        title: "BATTLE CHAMPION",
        halo: [120, 86, 12],
        pool: [186, 132, 22],
        rays: &[GOLD],
        confetti: [GOLD, lift(GOLD, 0.55), [255, 244, 214], [255, 186, 120]],
    };
    match theme {
        Theme::Classic => classic,
        Theme::Pokemon => Look {
            floor: &[(0.0, [24, 50, 92]), (0.48, [15, 20, 32]), (1.0, [16, 42, 30])],
            left: [239, 68, 68],
            right: [59, 130, 246],
            seam: ([210, 226, 255], [255, 255, 255]),
            prefix: Some("POKÉMON BATTLE"),
            title: "POKÉMON CHAMPION",
            halo: [40, 70, 130],
            rays: &[[239, 68, 68], [250, 204, 21], [59, 130, 246]],
            confetti: [[239, 68, 68], [255, 255, 255], [59, 130, 246], [250, 204, 21]],
            ..classic
        },
        Theme::Wizard => Look {
            floor: &[(0.0, [12, 16, 42]), (0.58, [22, 14, 38]), (1.0, [50, 12, 28])],
            left: [232, 44, 56],
            right: [34, 197, 110],
            seam: ([214, 180, 110], [255, 238, 196]),
            prefix: Some("WIZARD DUEL"),
            title: "DUELLING CHAMPION",
            halo: [96, 70, 20],
            ..classic
        },
        Theme::Saiyan => Look {
            floor: &[(0.0, [84, 36, 6]), (0.55, [44, 18, 6]), (1.0, [20, 9, 5])],
            left: [255, 150, 30],
            right: [70, 150, 255],
            seam: ([255, 196, 110], [255, 244, 210]),
            prefix: Some("TOURNAMENT"),
            title: "STRONGEST UNDER THE HEAVENS",
            halo: [150, 76, 8],
            rays: &[[255, 170, 40]],
            confetti: [[255, 214, 90], [255, 150, 30], [255, 244, 200], [255, 120, 40]],
            ..classic
        },
        Theme::Wrestling => Look {
            floor: &[(0.0, [20, 18, 26]), (1.0, [8, 8, 11])],
            left: [226, 44, 44],
            right: [48, 100, 235],
            seam: ([200, 204, 222], [240, 242, 255]),
            prefix: Some("MAIN EVENT"),
            title: "UNDISPUTED CHAMPION",
            halo: [90, 84, 70],
            confetti: [GOLD, [230, 232, 240], [255, 244, 214], [200, 204, 214]],
            ..classic
        },
    }
}

/// PNG bytes of a fight card, or `None` if drawing failed.
pub fn fight_png(fight: &Fight) -> Option<Vec<u8>> {
    let mut fs = super::awards::fonts().lock();
    // cosmic-text panics rather than drawing with no fonts at all.
    if fs.db().len() == 0 {
        return None;
    }
    let mut pen = Pen::new(FIGHT_W, FIGHT_H, &mut fs)?;
    draw_fight(&mut pen, fight);
    pen.px.encode_png().ok()
}

/// PNG bytes of the champion card, or `None` if drawing failed.
pub fn champion_png(champion: &Champion) -> Option<Vec<u8>> {
    let mut fs = super::awards::fonts().lock();
    if fs.db().len() == 0 {
        return None;
    }
    let mut pen = Pen::new(CHAMP_W, CHAMP_H, &mut fs)?;
    draw_champion(&mut pen, champion);
    pen.px.encode_png().ok()
}

/// How a side of the fight is lit.
#[derive(Clone, Copy)]
enum Side {
    Lit,
    Winner,
    Loser,
}

/// An index that names neither side leaves the fight looking open rather than
/// dimming somebody at random.
fn sides(outcome: &Outcome) -> (Side, Side) {
    match outcome {
        Outcome::Won(0) => (Side::Winner, Side::Loser),
        Outcome::Won(1) => (Side::Loser, Side::Winner),
        _ => (Side::Lit, Side::Lit),
    }
}

fn draw_fight(pen: &mut Pen<'_>, f: &Fight) {
    let (left, right) = sides(&f.outcome);
    let look = look(f.theme);
    floor(&mut pen.px, FIGHT_W, FIGHT_H, look.floor);
    glow(&mut pen.px, LEFT_CX + 30.0, FIGHT_CY + 20.0, 430.0, look.left, 74);
    glow(&mut pen.px, RIGHT_CX - 30.0, FIGHT_CY + 20.0, 430.0, look.right, 74);
    // Widest band first: stacking them leaves the seam soft down both sides
    // instead of ending on an edge.
    for (half, alpha) in [(124.0, 7), (88.0, 7), (54.0, 8), (26.0, 9)] {
        seam(&mut pen.px, FIGHT_W, FIGHT_H, half, look.seam.0, alpha);
    }
    seam(&mut pen.px, FIGHT_W, FIGHT_H, 2.0, look.seam.1, 24);
    backdrop(&mut pen.px, f.theme, &look);
    vignette(&mut pen.px, FIGHT_W, FIGHT_H, 150);
    frame(&mut pen.px, FIGHT_W, FIGHT_H, f.theme);

    stage_chip(pen, &stage_text(f.theme, f.stage.trim()));
    fighter(pen, f.left, LEFT_CX, look.left, left, f.theme);
    fighter(pen, f.right, RIGHT_CX, look.right, right, f.theme);
    versus(pen, FIGHT_W / 2.0, FIGHT_CY);
    // Last, so the damage lands on top of everything it belongs to.
    if let Some((who, delta)) = f.hit {
        match who {
            0 => hit_number(pen, LEFT_CX, delta),
            1 => hit_number(pen, RIGHT_CX, delta),
            _ => {}
        }
    }

    let line = f.line.trim();
    if !line.is_empty() {
        pen.panel(PANEL_X, PANEL_Y, PANEL_W, PANEL_H, 22.0);
        let buf = pen.paragraph(line, 19.0, 25.0, PANEL_W - 2.0 * PANEL_PAD, 2);
        let top = PANEL_Y + (PANEL_H - Pen::height(&buf)) / 2.0;
        pen.draw(&buf, PANEL_X + PANEL_PAD, top, [219, 224, 234]);
    }
}

/// The chip's words: the theme's banner before the stage, or the banner alone
/// for a one-off challenge, where "CHALLENGE" would only repeat it.
fn stage_text(theme: Theme, stage: &str) -> String {
    match look(theme).prefix {
        None => stage.to_string(),
        Some(prefix) if stage.is_empty() || stage.eq_ignore_ascii_case("challenge") => prefix.to_string(),
        Some(prefix) => format!("{prefix} · {stage}"),
    }
}

/// Small, letter-spaced and outlined: "ROUND 2", "CHALLENGE".
fn stage_chip(pen: &mut Pen<'_>, stage: &str) {
    if stage.is_empty() {
        return;
    }
    let text = spaced(&stage.to_uppercase());
    let text = pen.fit(&text, 15.0, Weight::SEMIBOLD, 400.0);
    let chip = Chip {
        text: &text,
        size: 15.0,
        weight: Weight::SEMIBOLD,
        fill: ([255, 255, 255], 18),
        ink: [226, 229, 238],
        edge: Some(([255, 255, 255], 90)),
    };
    pen.chip(FIGHT_W / 2.0, 16.0, 34.0, chip);
}

/// One side: glow, shadow, ringed portrait, name plate and - once the fight is
/// decided - a winner's pill or an OUT stamp.
fn fighter(pen: &mut Pen<'_>, who: &Fighter, cx: f32, colour: [u8; 3], side: Side, theme: Theme) {
    let lost = matches!(side, Side::Loser);
    let outer = AV / 2.0 + GAP + RING;
    // The winner's ring turns to gold; the loser's goes to cold slate.
    let band = match side {
        Side::Winner => (lift(GOLD, 0.55), dim(GOLD, 0.62)),
        Side::Lit => (lift(colour, 0.42), dim(colour, 0.66)),
        Side::Loser => ([86, 91, 104], [44, 47, 56]),
    };
    if !lost {
        let halo = if matches!(side, Side::Winner) { 120 } else { 62 };
        glow(&mut pen.px, cx, FIGHT_CY, outer + 76.0, if lost { colour } else { band.0 }, halo);
    }
    // Saiyans flare up; whoever is beaten has nothing left to flare with.
    if theme == Theme::Saiyan && !lost {
        let ki = if matches!(side, Side::Winner) { GOLD } else { colour };
        aura(&mut pen.px, cx, FIGHT_CY, outer, ki, 1.0);
    }
    shadow(&mut pen.px, cx, FIGHT_CY + 16.0, outer);
    ring(&mut pen.px, cx, FIGHT_CY, outer, band);
    fill_circle(&mut pen.px, cx, FIGHT_CY, AV / 2.0 + GAP, [12, 13, 17]);
    pen.portrait(cx, FIGHT_CY, Portrait { bytes: who.avatar.as_deref(), d: AV, grey: lost });
    if lost {
        wash(&mut pen.px, cx, FIGHT_CY, AV / 2.0, [8, 9, 12], 142);
    }
    if matches!(side, Side::Winner) {
        crown(&mut pen.px, cx, FIGHT_CY - outer + 16.0, 82.0, 34.0);
    }
    if let Some(house) = who.house {
        // On the outer shoulder, away from the VS between the two.
        let d = outer * std::f32::consts::FRAC_1_SQRT_2;
        let bx = if cx < FIGHT_W / 2.0 { cx - d } else { cx + d };
        house_badge(pen, house, bx, FIGHT_CY + d, FIGHT_BADGE_R, lost);
    }

    let edge = match side {
        Side::Winner => GOLD,
        Side::Lit => colour,
        Side::Loser => [74, 79, 92],
    };
    pen.plate(cx, PLATE_Y, PLATE_H, &who.name, (edge, if lost { MUTED } else { INK }));
    // Both bars empty towards the middle, so they face each other.
    health_bar(pen, cx, who, side, cx < FIGHT_W / 2.0);
    match side {
        Side::Winner => {
            let pill = Chip {
                text: &spaced("WINNER"),
                size: 14.0,
                weight: Weight::EXTRA_BOLD,
                fill: (GOLD, 255),
                ink: ON_LIGHT,
                edge: None,
            };
            pen.chip(cx, PILL_Y, PILL_H, pill);
        }
        Side::Loser => stamp(pen, cx, FIGHT_CY + 6.0),
        Side::Lit => {}
    }
}

/// Draw into a transparent layer `size` big and set it down on the card at
/// (`cx`, `cy`), turned by `angle`. Only a whole pixmap can be rotated, so
/// anything at an angle has to be painted away from the card first.
fn on_layer(pen: &mut Pen<'_>, cx: f32, cy: f32, size: (f32, f32), angle: f32, draw: impl FnOnce(&mut Pen<'_>)) {
    let Some(mut layer) = Pixmap::new(size.0 as u32, size.1 as u32) else { return };
    std::mem::swap(&mut pen.px, &mut layer);
    draw(pen);
    std::mem::swap(&mut pen.px, &mut layer);
    let quality = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    let turn = Transform::from_rotate_at(angle, cx, cy);
    let (x, y) = ((cx - size.0 / 2.0).round() as i32, (cy - size.1 / 2.0).round() as i32);
    pen.px.draw_pixmap(x, y, layer.as_ref(), &quality, turn, None);
}

/// "OUT", struck across the loser at an angle.
fn stamp(pen: &mut Pen<'_>, cx: f32, cy: f32) {
    let (w, h) = (212.0, 78.0);
    on_layer(pen, cx, cy, (w, h), -13.0, |p: &mut Pen<'_>| {
        if let Some(shape) = rrect(4.0, 4.0, w - 8.0, h - 8.0, 14.0) {
            p.px.fill_path(&shape, &paint([10, 8, 10], 130), FillRule::Winding, Transform::identity(), None);
            let stroke = Stroke { width: 5.0, ..Stroke::default() };
            p.px.stroke_path(&shape, &paint(STAMP, 235), &stroke, Transform::identity(), None);
        }
        p.layer_text(&spaced("OUT"), w / 2.0, h / 2.0 + 13.0, 36.0, STAMP);
    });
}

/// What the last exchange cost, popping off the fighter it hit. This is the
/// card's punchline, so it is big, outlined and lit from behind.
fn hit_number(pen: &mut Pen<'_>, cx: f32, delta: i32) {
    let (text, colour, size) = match delta {
        0 => ("MISS".to_string(), [176, 182, 194], 40.0),
        d if d > 0 => (format!("+{d}"), [74, 222, 128], 56.0),
        d => (format!("-{}", d.abs()), [255, 96, 96], 56.0),
    };
    let left = cx < FIGHT_W / 2.0;
    let (x, angle) = if left { (cx + 94.0, -9.0) } else { (cx - 94.0, 9.0) };
    let layer = (300.0, 130.0);
    on_layer(pen, x, 74.0, layer, angle, |p: &mut Pen<'_>| {
        let (lx, ly) = (layer.0 / 2.0, layer.1 / 2.0);
        glow(&mut p.px, lx, ly, 104.0, colour, 104);
        let baseline = ly + size * 0.36;
        for (dx, dy) in OUTLINE {
            p.layer_text(&text, lx + dx, baseline + dy, size, [9, 8, 11]);
        }
        p.layer_text(&text, lx, baseline, size, colour);
    });
}

/// Green while there is plenty left, amber once it bites, red near the end.
fn health_colour(frac: f32) -> [u8; 3] {
    match frac {
        f if f > 0.60 => [46, 204, 113],
        f if f > 0.30 => [245, 166, 35],
        _ => [239, 68, 68],
    }
}

/// One fighter's health: a recessed track with a lit fill and the numbers on
/// top. `anchor_left` keeps the fill against the outer edge of the card, so
/// the bar drains towards the middle.
fn health_bar(pen: &mut Pen<'_>, cx: f32, who: &Fighter, side: Side, anchor_left: bool) {
    let (x, y, w, h) = (cx - BAR_W / 2.0, BAR_Y, BAR_W, BAR_H);
    let max = who.max_hp.max(1);
    let lost = matches!(side, Side::Loser);
    let hp = if lost { 0 } else { who.hp.min(max) };
    let frac = hp as f32 / max as f32;

    if let Some(track) = rrect(x, y, w, h, h / 2.0) {
        let shader = down(y, y + h, &[(0.0, [7, 8, 11]), (0.6, [23, 25, 31]), (1.0, [38, 41, 50])]);
        fill_shaded(&mut pen.px, &track, shader, [20, 22, 27]);
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        pen.px.stroke_path(&track, &paint([0, 0, 0], 140), &stroke, Transform::identity(), None);
    }
    if frac > 0.0 {
        // A sliver of health still shows a round cap, so an almost-dead bar
        // never reads as a drawing bug.
        let fw = (w * frac).max(h);
        let fx = if anchor_left { x } else { x + w - fw };
        let c = health_colour(frac);
        if let Some(fill) = rrect(fx, y, fw, h, h / 2.0) {
            let shader = down(y, y + h, &[(0.0, lift(c, 0.38)), (0.5, c), (1.0, dim(c, 0.62))]);
            fill_shaded(&mut pen.px, &fill, shader, c);
        }
        // A sheen along the top edge, so the fill looks lit rather than flat.
        if let Some(sheen) = rrect(fx + 4.0, y + 2.5, (fw - 8.0).max(2.0), h * 0.32, h * 0.16) {
            pen.px.fill_path(&sheen, &paint([255, 255, 255], 78), FillRule::Winding, Transform::identity(), None);
        }
    }
    let ink = if lost { [172, 178, 190] } else { [248, 249, 252] };
    pen.outlined(&format!("{hp}/{max}"), cx, y + h / 2.0 + 5.4, 15.0, ink);
}

/// The centrepiece: a clash of spikes, a pool of gold light, and the word
/// itself in metal.
fn versus(pen: &mut Pen<'_>, cx: f32, cy: f32) {
    burst(&mut pen.px, cx, cy, 158.0);
    glow(&mut pen.px, cx, cy, 122.0, [255, 208, 96], 78);
    pen.metal("VS", cx, cy + 36.0, 104.0);
}

fn draw_champion(pen: &mut Pen<'_>, c: &Champion) {
    let outer = CHAMP_AV / 2.0 + GAP + RING;
    let look = look(c.theme);
    floor(&mut pen.px, CHAMP_W, CHAMP_H, look.floor);
    glow(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, 430.0, look.halo, 150);
    champion_backdrop(&mut pen.px, c.theme, outer, &look);
    vignette(&mut pen.px, CHAMP_W, CHAMP_H, 170);
    frame(&mut pen.px, CHAMP_W, CHAMP_H, c.theme);
    // A warm pool of light under the name, so the type sits in the glow.
    glow(&mut pen.px, CHAMP_W / 2.0, 462.0, 330.0, look.pool, 78);

    glow(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, outer + 90.0, lift(GOLD, 0.3), 110);
    if c.theme == Theme::Saiyan {
        aura(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, outer, GOLD, 1.5);
    }
    shadow(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY + 20.0, outer);
    ring(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, outer, (lift(GOLD, 0.6), dim(GOLD, 0.58)));
    fill_circle(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, CHAMP_AV / 2.0 + GAP, [12, 13, 17]);
    let portrait = Portrait { bytes: c.who.avatar.as_deref(), d: CHAMP_AV, grey: false };
    pen.portrait(CHAMP_W / 2.0, CHAMP_CY, portrait);
    // After the ring, so the crown rests on it.
    crown(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY - outer + 8.0, 140.0, 60.0);
    if let Some(house) = c.who.house {
        // Low on the right of the ring, clear of the ribbon underneath.
        let angle = 30f32.to_radians();
        let (bx, by) = (CHAMP_W / 2.0 + outer * angle.cos(), CHAMP_CY + outer * angle.sin());
        house_badge(pen, house, bx, by, CHAMP_BADGE_R, false);
    }
    if c.theme == Theme::Wrestling {
        belt(pen);
        ribbon(pen, look.title, false);
    } else {
        ribbon(pen, look.title, true);
    }

    let name = pen.fit(&c.who.name, 42.0, Weight::EXTRA_BOLD, 820.0);
    pen.centered(&name, CHAMP_W / 2.0, 468.0, 42.0, Weight::EXTRA_BOLD, INK);
    let sub = c.subtitle.trim();
    if !sub.is_empty() {
        let sub = pen.fit(sub, 20.0, Weight::MEDIUM, 840.0);
        pen.centered(&sub, CHAMP_W / 2.0, 508.0, 20.0, Weight::MEDIUM, [208, 197, 166]);
    }
    let line = c.line.trim();
    if !line.is_empty() {
        let buf = pen.paragraph(line, 20.0, 25.0, 820.0, 2);
        pen.draw(&buf, (CHAMP_W - 820.0) / 2.0, 520.0, [186, 191, 202]);
    }
}

/// A house crest in a dark disc with a keyline in the house's second colour, the
/// same badge the sorting card wears. A beaten fighter's badge is dimmed with them.
fn house_badge(pen: &mut Pen<'_>, house: &super::house::House, cx: f32, cy: f32, r: f32, dimmed: bool) {
    wash(&mut pen.px, cx, cy + 4.0, r + 6.0, [0, 0, 0], 110);
    fill_circle(&mut pen.px, cx, cy, r, [16, 16, 21]);
    wash(&mut pen.px, cx, cy, r, [255, 255, 255], 14);
    if let Some(edge) = PathBuilder::from_circle(cx, cy, r - 1.25) {
        let keyline = if dimmed { [86, 91, 104] } else { lift(house.colours.1, 0.55) };
        let stroke = Stroke { width: 2.5, ..Stroke::default() };
        pen.px.stroke_path(&edge, &paint(keyline, 235), &stroke, Transform::identity(), None);
    }
    let side = (r * 1.5).round();
    if let Some(art) = super::house_card::crest_art(house.key, side as u32) {
        let corner = |centre: f32| (centre - side / 2.0).round() as i32;
        let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
        pen.px.draw_pixmap(corner(cx), corner(cy), art.as_ref(), &paint, Transform::identity(), None);
    }
    if dimmed {
        wash(&mut pen.px, cx, cy, r, [8, 9, 12], 120);
    }
}

/// A ribbon with two swallow-tailed ends, carrying the title. A long title
/// widens the plate before it is cut short; `tails` off leaves just the plate,
/// for a card that hangs it on something else.
fn ribbon(pen: &mut Pen<'_>, label: &str, tails: bool) {
    let label = spaced(label);
    let w = (pen.measure(&label, 21.0, Weight::EXTRA_BOLD) + 52.0).clamp(420.0, 600.0);
    let (x, y, h) = (CHAMP_W / 2.0 - w / 2.0, 370.0, 54.0);
    let ends: &[f32] = if tails { &[-1.0, 1.0] } else { &[] };
    for &side in ends {
        let start = if side < 0.0 { x + 24.0 } else { x + w - 24.0 };
        let (end, notch) = (start + side * 104.0, start + side * 74.0);
        let mut pb = PathBuilder::new();
        pb.move_to(start, y + 9.0);
        pb.line_to(end, y + 3.0);
        pb.line_to(notch, y + h / 2.0);
        pb.line_to(end, y + h - 3.0);
        pb.line_to(start, y + h - 9.0);
        pb.close();
        if let Some(tail) = pb.finish() {
            let shader = down(y, y + h, &[(0.0, dim(GOLD, 0.72)), (1.0, dim(GOLD, 0.42))]);
            fill_shaded(&mut pen.px, &tail, shader, dim(GOLD, 0.6));
        }
        // The fold where the tail passes behind the plate.
        let mut pb = PathBuilder::new();
        pb.move_to(start + side * 4.0, y + h - 9.0);
        pb.line_to(start + side * 30.0, y + h - 2.0);
        pb.line_to(start + side * 4.0, y + h + 9.0);
        pb.close();
        if let Some(fold) = pb.finish() {
            pen.px.fill_path(&fold, &paint(dim(GOLD, 0.34), 255), FillRule::Winding, Transform::identity(), None);
        }
    }
    let Some(plate) = rrect(x, y, w, h, 14.0) else { return };
    let face = [(0.0, [255, 238, 160]), (0.45, GOLD), (0.55, dim(GOLD, 0.78)), (1.0, [255, 226, 122])];
    let shader = down(y, y + h, &face);
    fill_shaded(&mut pen.px, &plate, shader, GOLD);
    let stroke = Stroke { width: 2.5, ..Stroke::default() };
    pen.px.stroke_path(&plate, &paint(dim(GOLD, 0.45), 255), &stroke, Transform::identity(), None);
    let text = pen.fit(&label, 21.0, Weight::EXTRA_BOLD, w - 52.0);
    pen.centered(&text, x + w / 2.0, y + 35.0, 21.0, Weight::EXTRA_BOLD, ON_LIGHT);
}

/// A five-point crown on a jewelled band.
fn crown(px: &mut Pixmap, cx: f32, base: f32, w: f32, h: f32) {
    let hw = w / 2.0;
    let peaks = [(-1.0, 0.60), (-0.5, 0.84), (0.0, 1.0), (0.5, 0.84), (1.0, 0.60)];
    let mut pb = PathBuilder::new();
    pb.move_to(cx - hw, base);
    pb.line_to(cx - hw, base - h * 0.60);
    for pair in peaks.windows(2) {
        let [(x0, _), (x1, y1)] = [pair[0], pair[1]];
        pb.quad_to(cx + (x0 + x1) / 2.0 * hw, base - h * 0.20, cx + x1 * hw, base - h * y1);
    }
    pb.line_to(cx + hw, base);
    pb.close();
    if let Some(path) = pb.finish() {
        let shader = down(base - h, base, &[(0.0, [255, 240, 168]), (0.55, GOLD), (1.0, dim(GOLD, 0.6))]);
        fill_shaded(px, &path, shader, GOLD);
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        px.stroke_path(&path, &paint(dim(GOLD, 0.38), 255), &stroke, Transform::identity(), None);
    }
    if let Some(band) = rrect(cx - hw - 5.0, base - 4.0, w + 10.0, h * 0.30, h * 0.15) {
        let shader = down(base - 4.0, base - 4.0 + h * 0.30, &[(0.0, [255, 236, 150]), (1.0, dim(GOLD, 0.55))]);
        fill_shaded(px, &band, shader, GOLD);
    }
    for (x, y) in peaks {
        fill_circle(px, cx + x * hw, base - h * y, h * 0.11, [255, 249, 214]);
    }
    fill_circle(px, cx, base + h * 0.09, h * 0.08, [214, 62, 82]);
}

/// Spokes of light fanning out from behind the champion, each in the next of
/// `tints`.
fn rays(px: &mut Pixmap, cx: f32, cy: f32, r0: f32, r1: f32, tints: &[[u8; 3]]) {
    let n = 18;
    for i in 0..n {
        let a = i as f32 * std::f32::consts::TAU / n as f32 + 0.18;
        let half = std::f32::consts::TAU / n as f32 * 0.28;
        let alpha = if i % 2 == 0 { 54 } else { 26 };
        let tint = tints.get(i % tints.len().max(1)).copied().unwrap_or(GOLD);
        let mut pb = PathBuilder::new();
        pb.move_to(cx + a.cos() * r0, cy + a.sin() * r0);
        pb.line_to(cx + (a - half).cos() * r1, cy + (a - half).sin() * r1);
        pb.line_to(cx + (a + half).cos() * r1, cy + (a + half).sin() * r1);
        pb.close();
        let Some(path) = pb.finish() else { continue };
        let stops = vec![
            GradientStop::new(0.0, sk(lift(tint, 0.3), alpha)),
            GradientStop::new(1.0, sk(tint, 0)),
        ];
        let shader = RadialGradient::new(
            Point::from_xy(cx, cy),
            Point::from_xy(cx, cy),
            r1,
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        );
        fill_shaded(px, &path, shader, tint);
    }
}

/// A fixed-seed xorshift: scattered things land in the same places every time,
/// so the same card twice over comes out the same.
fn scatter(seed: u32) -> impl FnMut() -> u32 {
    let mut s = seed;
    move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    }
}

/// Specks thrown over the card in the theme's colours.
fn confetti(px: &mut Pixmap, w: f32, h: f32, clear: (f32, f32, f32), palette: [[u8; 3]; 4]) {
    let mut next = scatter(0x9E37_79B9);
    for _ in 0..64 {
        let x = (next() % w as u32) as f32;
        let y = (next() % h as u32) as f32;
        let (dx, dy) = (x - clear.0, y - clear.1);
        // Clear of the portrait, and clear of the type along the bottom.
        if (dx * dx + dy * dy).sqrt() < clear.2 || y > h - 150.0 {
            continue;
        }
        let r = 1.5 + (next() % 20) as f32 / 10.0;
        let c = palette[(next() % palette.len() as u32) as usize];
        wash(px, x, y, r, c, 40 + (next() % 120) as u8);
    }
}

/// The floor: a flat fill, or a top-to-bottom wash through the theme's stops.
fn floor(px: &mut Pixmap, w: f32, h: f32, stops: &[(f32, [u8; 3])]) {
    let first = stops.first().map_or(BG, |s| s.1);
    px.fill(sk(first, 255));
    if stops.len() < 2 {
        return;
    }
    if let (Some(shader), Some(area)) = (down(0.0, h, stops), Rect::from_xywh(0.0, 0.0, w, h)) {
        let mut p = Paint::default();
        p.shader = shader;
        px.fill_rect(area, &p, Transform::identity(), None);
    }
}

/// The world behind a fight, laid over the floor and the glows. Everything
/// here sits behind the portraits and plates, and nothing busy reaches down to
/// the line panel.
fn backdrop(px: &mut Pixmap, theme: Theme, look: &Look) {
    let mid = FIGHT_W / 2.0;
    let edge = AV / 2.0 + GAP + RING;
    match theme {
        Theme::Classic => {}
        Theme::Pokemon => {
            poke_ball(px, mid, FIGHT_CY + 28.0, 140.0);
            for cx in [LEFT_CX, RIGHT_CX] {
                battle_field(px, cx, FIGHT_CY + 110.0);
            }
        }
        Theme::Wizard => {
            sparkles(px, FIGHT_W, 268.0, 90);
            spell_beam(px, (LEFT_CX + edge, RIGHT_CX - edge), FIGHT_CY, (look.left, look.right));
        }
        Theme::Saiyan => speed_lines(px, mid, FIGHT_CY, 150.0, 760.0),
        Theme::Wrestling => {
            crowd(px, FIGHT_W);
            ropes(px, FIGHT_W, [234.0, 292.0, 350.0]);
            for cx in [LEFT_CX, RIGHT_CX] {
                spotlight(px, cx + (mid - cx) * 0.4, (cx, FIGHT_CY + 140.0), 190.0, 1.0);
            }
        }
    }
}

/// The world behind the champion: rays and confetti in the theme's colours,
/// gold dust for the wizards, speed lines for the Saiyans, and a crowd under
/// crossing spotlights for the ring.
fn champion_backdrop(px: &mut Pixmap, theme: Theme, outer: f32, look: &Look) {
    let (cx, cy) = (CHAMP_W / 2.0, CHAMP_CY);
    match theme {
        Theme::Saiyan => speed_lines(px, cx, cy, outer + 30.0, 820.0),
        Theme::Wrestling => {
            crowd(px, CHAMP_W);
            // Two lamps from the rig, crossing on the champion.
            for lamp in [150.0, CHAMP_W - 150.0] {
                spotlight(px, lamp, (cx, CHAMP_H), 190.0, 2.0);
            }
        }
        _ => rays(px, cx, cy, outer + 10.0, 760.0, look.rays),
    }
    match theme {
        Theme::Wizard => sparkles(px, CHAMP_W, CHAMP_H - 150.0, 120),
        _ => confetti(px, CHAMP_W, CHAMP_H, (cx, cy, outer + 26.0), look.confetti),
    }
}

/// A theme's border. The wizards' cards get a double rule of gold, like the
/// edge of an old certificate, with a diamond in each corner; it sits inside
/// the stage chip and under the crown.
fn frame(px: &mut Pixmap, w: f32, h: f32, theme: Theme) {
    if theme != Theme::Wizard {
        return;
    }
    let gold = [214, 176, 98];
    for (inset, width, alpha, r) in [(6.0, 2.5, 220, 14.0), (12.0, 2.0, 130, 9.0)] {
        if let Some(rule) = rrect(inset, inset, w - 2.0 * inset, h - 2.0 * inset, r) {
            let stroke = Stroke { width, ..Stroke::default() };
            px.stroke_path(&rule, &paint(gold, alpha), &stroke, Transform::identity(), None);
        }
    }
    for (x, y) in [(20.0, 20.0), (w - 20.0, 20.0), (20.0, h - 20.0), (w - 20.0, h - 20.0)] {
        diamond(px, x, y, 6.0, gold, 230);
    }
}

fn diamond(px: &mut Pixmap, x: f32, y: f32, s: f32, c: [u8; 3], alpha: u8) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y - s);
    pb.line_to(x + s, y);
    pb.line_to(x, y + s);
    pb.line_to(x - s, y);
    pb.close();
    if let Some(path) = pb.finish() {
        px.fill_path(&path, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

/// A capture ball traced in faint light behind the VS: a red cap, a pale
/// base, the dark band across the middle and the button ring at its heart.
fn poke_ball(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    let (Some(disc), Some(mut mask)) = (PathBuilder::from_circle(cx, cy, r), Mask::new(px.width(), px.height()))
    else {
        return;
    };
    mask.fill_path(&disc, FillRule::Winding, true, Transform::identity());
    let across = |y: f32, h: f32| Rect::from_xywh(cx - r, y, 2.0 * r, h);
    for (area, c, alpha) in [
        (across(cy - r, r), [232, 52, 60], 58),
        (across(cy, r), [236, 240, 250], 22),
        (across(cy - r * 0.075, r * 0.15), [4, 5, 8], 190),
    ] {
        if let Some(area) = area {
            px.fill_rect(area, &paint(c, alpha), Transform::identity(), Some(&mask));
        }
    }
    let stroke = Stroke { width: 4.0, ..Stroke::default() };
    px.stroke_path(&disc, &paint([236, 240, 250], 40), &stroke, Transform::identity(), None);
    wash(px, cx, cy, r * 0.31, [4, 5, 8], 200);
    if let Some(button) = PathBuilder::from_circle(cx, cy, r * 0.19) {
        let stroke = Stroke { width: 6.0, ..Stroke::default() };
        px.stroke_path(&button, &paint([236, 240, 250], 80), &stroke, Transform::identity(), None);
    }
}

/// The pale oval of field a trainer's fighter stands on.
fn battle_field(px: &mut Pixmap, cx: f32, cy: f32) {
    let Some(oval) = Rect::from_xywh(cx - 172.0, cy - 30.0, 344.0, 60.0).and_then(PathBuilder::from_oval) else {
        return;
    };
    let shader = down(cy - 30.0, cy + 30.0, &[(0.0, [120, 196, 110]), (1.0, [52, 110, 60])]);
    let mut p = paint([86, 150, 84], 80);
    if let Some(shader) = shader {
        p.shader = shader;
        p.shader.apply_opacity(0.32);
    }
    px.fill_path(&oval, &p, FillRule::Winding, Transform::identity(), None);
    let stroke = Stroke { width: 3.0, ..Stroke::default() };
    px.stroke_path(&oval, &paint([190, 236, 176], 90), &stroke, Transform::identity(), None);
}

/// Gold dust with the odd four-pointed twinkle, kept above `bottom`.
fn sparkles(px: &mut Pixmap, w: f32, bottom: f32, n: usize) {
    let mut next = scatter(0x2545_F491);
    for i in 0..n {
        let x = (next() % w as u32) as f32;
        let y = (next() % bottom as u32) as f32;
        let alpha = 70 + (next() % 150) as u8;
        let c = if next() % 3 == 0 { [255, 246, 214] } else { [240, 200, 110] };
        // Inside the gold frame, and out of the chip's letters.
        if under_chip(x, y) || x < 28.0 || x > w - 28.0 || y < 28.0 {
            continue;
        }
        if i % 8 == 0 {
            let s = 6.0 + (next() % 6) as f32;
            glow(px, x, y, s * 1.8, c, 70);
            twinkle(px, x, y, s, c, 230);
        } else {
            wash(px, x, y, 1.0 + (next() % 12) as f32 / 10.0, c, alpha);
        }
    }
}

/// Whether a point falls where the stage chip sits, top centre, so specks of
/// scenery stay out of its letters.
fn under_chip(x: f32, y: f32) -> bool {
    y < 62.0 && (x - FIGHT_W / 2.0).abs() < 250.0
}

/// A four-pointed star with pinched sides.
fn twinkle(px: &mut Pixmap, x: f32, y: f32, s: f32, c: [u8; 3], alpha: u8) {
    let k = s * 0.16;
    let mut pb = PathBuilder::new();
    pb.move_to(x, y - s);
    pb.quad_to(x + k, y - k, x + s, y);
    pb.quad_to(x + k, y + k, x, y + s);
    pb.quad_to(x - k, y + k, x - s, y);
    pb.quad_to(x - k, y - k, x, y - s);
    pb.close();
    if let Some(path) = pb.finish() {
        px.fill_path(&path, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

/// Two spells meeting: a crackling bolt off each portrait in that side's
/// colour, flaring white-gold where they collide behind the VS.
fn spell_beam(px: &mut Pixmap, ends: (f32, f32), y: f32, colours: ([u8; 3], [u8; 3])) {
    let mid = (ends.0 + ends.1) / 2.0;
    let jag = [0.0, -9.0, 6.0, -4.0, 11.0, -7.0, 3.0, -10.0, 8.0, -3.0, 0.0];
    for (from, colour, flip) in [(ends.0, colours.0, 1.0), (ends.1, colours.1, -1.0)] {
        let mut pb = PathBuilder::new();
        pb.move_to(from, y);
        let n = (jag.len() - 1) as f32;
        for (i, dy) in jag.iter().enumerate().skip(1) {
            pb.line_to(from + (mid - from) * i as f32 / n, y + dy * flip);
        }
        let Some(bolt) = pb.finish() else { continue };
        for (width, c, alpha) in [(18.0, colour, 36), (8.0, colour, 110), (3.0, lift(colour, 0.7), 240)] {
            let stroke = Stroke { width, line_join: LineJoin::Round, line_cap: LineCap::Round, ..Stroke::default() };
            px.stroke_path(&bolt, &paint(c, alpha), &stroke, Transform::identity(), None);
        }
    }
    glow(px, mid, y, 80.0, [255, 240, 200], 170);
}

/// Speed lines bursting out of the clash, gone at the middle and strongest at
/// the card's edge, like a panel in a fight manga.
fn speed_lines(px: &mut Pixmap, cx: f32, cy: f32, r0: f32, r1: f32) {
    let mut next = scatter(0x6C8E_9CF5);
    let n = 60;
    let tint = [255, 236, 190];
    for i in 0..n {
        let a = (i as f32 + (next() % 100) as f32 / 100.0) * std::f32::consts::TAU / n as f32;
        let half = 0.004 + (next() % 10) as f32 * 0.0011;
        let start = r0 + (next() % 90) as f32;
        let mut pb = PathBuilder::new();
        pb.move_to(cx + a.cos() * start, cy + a.sin() * start);
        pb.line_to(cx + (a - half).cos() * r1, cy + (a - half).sin() * r1);
        pb.line_to(cx + (a + half).cos() * r1, cy + (a + half).sin() * r1);
        pb.close();
        let Some(path) = pb.finish() else { continue };
        let stops = vec![GradientStop::new(r0 / r1, sk(tint, 0)), GradientStop::new(0.8, sk(tint, 70))];
        let centre = Point::from_xy(cx, cy);
        let shader = RadialGradient::new(centre, centre, r1, stops, SpreadMode::Pad, Transform::identity());
        fill_shaded(px, &path, shader, tint);
    }
}

/// A flaring ki aura: curved tongues of light round the ring that sweep
/// upwards and burn tallest over the head, as a soft outer flame with a
/// brighter core. `scale` grows it.
fn aura(px: &mut Pixmap, cx: f32, cy: f32, r: f32, ki: [u8; 3], scale: f32) {
    let lengths = [0.62, 1.0, 0.74, 0.9, 0.56, 0.96, 0.7, 0.84];
    let far = r + 120.0 * scale;
    for (reach, alpha, n, turn) in [(1.0, 165u8, 18usize, 0.0), (0.6, 235, 14, 0.11)] {
        let step = std::f32::consts::TAU / n as f32;
        let at = |angle: f32, dist: f32| (cx + angle.cos() * dist, cy + angle.sin() * dist);
        let mut pb = PathBuilder::new();
        for i in 0..n {
            let (a0, a1) = (i as f32 * step + turn, (i + 1) as f32 * step + turn);
            let m = (a0 + a1) / 2.0;
            // 1 straight up, 0 straight down: flames rise.
            let rise = (1.0 - m.sin()) / 2.0;
            let len = (16.0 + 84.0 * rise * rise) * lengths[i % lengths.len()] * reach * scale;
            // Each tip leans from the ring's outward line towards straight up.
            let (nx, ny) = (m.cos(), m.sin() - 0.9);
            let norm = (nx * nx + ny * ny).sqrt().max(0.001);
            let root = at(m, r);
            let tip = (root.0 + nx / norm * len, root.1 + ny / norm * len);
            let (b0, b1) = (at(a0, r - 4.0), at(a1, r - 4.0));
            if i == 0 {
                pb.move_to(b0.0, b0.1);
            }
            // Bellied sides that pinch to a point, so each tongue reads as flame.
            let c0 = at(a0 + step * 0.15, r + len * 0.32);
            let c1 = at(a1 - step * 0.15, r + len * 0.2);
            pb.quad_to(c0.0, c0.1 - len * 0.18, tip.0, tip.1);
            pb.quad_to(c1.0, c1.1 - len * 0.08, b1.0, b1.1);
        }
        pb.close();
        let Some(path) = pb.finish() else { continue };
        let inner = r / far;
        let stops = vec![
            GradientStop::new(inner, sk(lift(ki, 0.65), alpha)),
            GradientStop::new(inner + (1.0 - inner) * 0.45, sk(ki, alpha / 2)),
            GradientStop::new(1.0, sk(ki, 0)),
        ];
        let centre = Point::from_xy(cx, cy);
        let shader = RadialGradient::new(centre, centre, far, stops, SpreadMode::Pad, Transform::identity());
        fill_shaded(px, &path, shader, ki);
    }
}

/// A crowd in the dark along the top: rows of heads and shoulders against the
/// arena haze, with the odd camera flash going off.
fn crowd(px: &mut Pixmap, w: f32) {
    glow(px, w * 0.25, 30.0, 250.0, [110, 96, 140], 26);
    glow(px, w * 0.75, 30.0, 250.0, [110, 96, 140], 26);
    let mut next = scatter(0x1B87_3593);
    for row in 0..3 {
        let y = 26.0 + row as f32 * 26.0;
        let shade = [16 - row * 3, 15 - row * 3, 21 - row * 4];
        let mut x = -12.0 + (next() % 20) as f32;
        while x < w + 20.0 {
            let r = 9.0 + (next() % 5) as f32;
            let dy = (next() % 8) as f32;
            wash(px, x, y + dy, r, shade, 215);
            let shoulders = Rect::from_xywh(x - r * 1.7, y + dy + r * 0.9, r * 3.4, r * 3.0);
            if let Some(body) = shoulders.and_then(PathBuilder::from_oval) {
                px.fill_path(&body, &paint(shade, 215), FillRule::Winding, Transform::identity(), None);
            }
            x += r * 2.5 + (next() % 10) as f32;
        }
    }
    for _ in 0..12 {
        let (x, y) = ((next() % w as u32) as f32, 18.0 + (next() % 80) as f32);
        if under_chip(x, y + 20.0) {
            continue;
        }
        glow(px, x, y, 12.0, [255, 255, 255], 100);
        wash(px, x, y, 1.8, [255, 255, 255], 210);
    }
}

/// A spotlight cone from a lamp above the card down to `(cx, floor_y)`, bright
/// at the lamp and gone by the floor. It is stacked from wide and faint to
/// narrow, so the beam has soft edges; `power` scales how strong it is.
fn spotlight(px: &mut Pixmap, lamp_x: f32, (cx, floor_y): (f32, f32), spread: f32, power: f32) {
    let tint = [255, 246, 222];
    for (widen, alpha) in [(1.25, 26.0), (1.0, 34.0), (0.72, 40.0)] {
        let alpha = (alpha * power).min(255.0) as u8;
        let mut pb = PathBuilder::new();
        pb.move_to(lamp_x - 14.0 * widen, -10.0);
        pb.line_to(lamp_x + 14.0 * widen, -10.0);
        pb.line_to(cx + spread * widen, floor_y);
        pb.line_to(cx - spread * widen, floor_y);
        pb.close();
        let Some(cone) = pb.finish() else { continue };
        let stops = vec![GradientStop::new(0.0, sk(tint, alpha)), GradientStop::new(1.0, sk(tint, 0))];
        let shader = LinearGradient::new(
            Point::from_xy(lamp_x, 0.0),
            Point::from_xy(cx, floor_y),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        );
        fill_shaded(px, &cone, shader, tint);
    }
}

/// Ring ropes in red, white and blue, sagging a little between two posts, with
/// a padded turnbuckle where each meets a post.
fn ropes(px: &mut Pixmap, w: f32, ys: [f32; 3]) {
    let colours = [[214, 36, 44], [236, 236, 242], [36, 78, 204]];
    let (x0, x1) = (22.0, w - 22.0);
    for x in [x0, x1] {
        if let Some(post) = rrect(x - 7.0, ys[0] - 30.0, 14.0, ys[2] - ys[0] + 60.0, 5.0) {
            px.fill_path(&post, &paint([48, 50, 58], 255), FillRule::Winding, Transform::identity(), None);
        }
    }
    for (y, c) in ys.into_iter().zip(colours) {
        let mut pb = PathBuilder::new();
        pb.move_to(x0, y);
        pb.quad_to(w / 2.0, y + 24.0, x1, y);
        let Some(rope) = pb.finish() else { continue };
        let line = |width: f32| Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
        px.stroke_path(&rope, &paint([0, 0, 0], 120), &line(9.0), Transform::from_translate(0.0, 4.0), None);
        px.stroke_path(&rope, &paint(dim(c, 0.9), 235), &line(6.0), Transform::identity(), None);
        px.stroke_path(&rope, &paint(lift(c, 0.5), 120), &line(2.0), Transform::from_translate(0.0, -1.5), None);
        for x in [x0, x1] {
            if let Some(pad) = rrect(x - 12.0, y - 17.0, 24.0, 34.0, 8.0) {
                let shader = down(y - 17.0, y + 17.0, &[(0.0, lift(c, 0.25)), (1.0, dim(c, 0.6))]);
                fill_shaded(px, &pad, shader, c);
            }
        }
    }
}

/// A title belt behind the champion's plate: a black leather strap with gold
/// stitching and a jewelled side plate either way.
fn belt(pen: &mut Pen<'_>) {
    let (x0, x1, y, h) = (150.0, 850.0, 380.0, 34.0);
    if let Some(strap) = rrect(x0, y, x1 - x0, h, 12.0) {
        let shader = down(y, y + h, &[(0.0, [52, 46, 46]), (0.5, [24, 21, 21]), (1.0, [10, 9, 9])]);
        fill_shaded(&mut pen.px, &strap, shader, [24, 21, 21]);
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        pen.px.stroke_path(&strap, &paint([0, 0, 0], 200), &stroke, Transform::identity(), None);
    }
    for sy in [y + 6.0, y + h - 6.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(x0 + 16.0, sy);
        pb.line_to(x1 - 16.0, sy);
        let Some(stitch) = pb.finish() else { continue };
        let stroke = Stroke { width: 2.0, dash: StrokeDash::new(vec![7.0, 6.0], 0.0), ..Stroke::default() };
        pen.px.stroke_path(&stitch, &paint(GOLD, 170), &stroke, Transform::identity(), None);
    }
    for (sx, gem) in [(222.0, [214, 40, 48]), (778.0, [40, 90, 220])] {
        let (pw, ph) = (62.0, h + 16.0);
        let Some(plate) = rrect(sx - pw / 2.0, y - 8.0, pw, ph, 10.0) else { continue };
        let shader = down(y - 8.0, y - 8.0 + ph, &METAL);
        fill_shaded(&mut pen.px, &plate, shader, GOLD);
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        pen.px.stroke_path(&plate, &paint(dim(GOLD, 0.4), 255), &stroke, Transform::identity(), None);
        fill_circle(&mut pen.px, sx, y + h / 2.0, 9.0, dim(GOLD, 0.4));
        fill_circle(&mut pen.px, sx, y + h / 2.0, 7.0, gem);
        wash(&mut pen.px, sx - 2.0, y + h / 2.0 - 2.5, 2.5, [255, 255, 255], 170);
    }
}

/// Where the two colours meet: a slanted seam, brightest across the middle.
fn seam(px: &mut Pixmap, w: f32, h: f32, half: f32, c: [u8; 3], alpha: u8) {
    let (tx, bx) = (w / 2.0 + 26.0, w / 2.0 - 26.0);
    let mut pb = PathBuilder::new();
    pb.move_to(tx - half, 0.0);
    pb.line_to(tx + half, 0.0);
    pb.line_to(bx + half, h);
    pb.line_to(bx - half, h);
    pb.close();
    let Some(path) = pb.finish() else { return };
    let stops = vec![
        GradientStop::new(0.0, sk(c, 0)),
        GradientStop::new(0.45, sk(c, alpha)),
        GradientStop::new(1.0, sk(c, 0)),
    ];
    let shader = LinearGradient::new(
        Point::from_xy(0.0, 0.0),
        Point::from_xy(0.0, h),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    fill_shaded(px, &path, shader, c);
}

/// A clash of spikes, longer ones alternating with short.
fn burst(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    let n = 12;
    for i in 0..n {
        let a = i as f32 * std::f32::consts::TAU / n as f32 + 0.26;
        let half = 0.06;
        let long = if i % 2 == 0 { r } else { r * 0.60 };
        let mut pb = PathBuilder::new();
        pb.move_to(cx + a.cos() * long, cy + a.sin() * long);
        pb.line_to(cx + (a - half).cos() * r * 0.26, cy + (a - half).sin() * r * 0.26);
        pb.line_to(cx + (a + half).cos() * r * 0.26, cy + (a + half).sin() * r * 0.26);
        pb.close();
        let Some(path) = pb.finish() else { continue };
        let stops = vec![
            GradientStop::new(0.0, sk([255, 226, 150], 120)),
            GradientStop::new(1.0, sk([255, 200, 90], 0)),
        ];
        let shader = RadialGradient::new(
            Point::from_xy(cx, cy),
            Point::from_xy(cx, cy),
            r,
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        );
        fill_shaded(px, &path, shader, [255, 214, 120]);
    }
}

/// The ring around a portrait, lit from the top left.
fn ring(px: &mut Pixmap, cx: f32, cy: f32, outer: f32, band: ([u8; 3], [u8; 3])) {
    let Some(path) = PathBuilder::from_circle(cx, cy, outer) else { return };
    let stops = vec![GradientStop::new(0.0, sk(band.0, 255)), GradientStop::new(1.0, sk(band.1, 255))];
    let shader = LinearGradient::new(
        Point::from_xy(cx - outer, cy - outer),
        Point::from_xy(cx + outer, cy + outer),
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    fill_shaded(px, &path, shader, band.0);
}

/// A soft shadow, faked as stacked discs, so a portrait stands over the floor
/// instead of being pasted onto it.
fn shadow(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    for i in 0..12 {
        let t = i as f32 / 12.0;
        wash(px, cx, cy + 14.0, r + 2.0 + t * 30.0, [0, 0, 0], (30.0 * (1.0 - t)) as u8);
    }
}

/// Edges pulled into darkness, so the middle of the card carries the eye.
fn vignette(px: &mut Pixmap, w: f32, h: f32, strength: u8) {
    let stops = vec![
        GradientStop::new(0.42, sk([0, 0, 0], 0)),
        GradientStop::new(1.0, sk([0, 0, 0], strength)),
    ];
    let shader = RadialGradient::new(
        Point::from_xy(w / 2.0, h / 2.0),
        Point::from_xy(w / 2.0, h / 2.0),
        w * 0.70,
        stops,
        SpreadMode::Pad,
        Transform::identity(),
    );
    if let (Some(shader), Some(area)) = (shader, Rect::from_xywh(0.0, 0.0, w, h)) {
        let mut p = Paint::default();
        p.shader = shader;
        px.fill_rect(area, &p, Transform::identity(), None);
    }
}

/// A neutral stand-in when somebody has no profile picture.
fn silhouette(px: &mut Pixmap, cx: f32, cy: f32, r: f32) {
    fill_circle(px, cx, cy, r, [52, 55, 64]);
    let (Some(mut mask), Some(disc)) = (Mask::new(px.width(), px.height()), PathBuilder::from_circle(cx, cy, r))
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

/// A translucent disc: confetti specks, and the wash over whoever lost.
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

/// Letters opened up, the way a small caps chip wants them.
fn spaced(s: &str) -> String {
    s.chars().map(String::from).collect::<Vec<_>>().join("\u{2009}")
}

fn pick_family(fs: &FontSystem) -> String {
    for name in ["Montserrat", "Avenir Next", "Noto Sans"] {
        if fs.db().faces().any(|f| f.families.iter().any(|(n, _)| n == name)) {
            return name.to_string();
        }
    }
    "sans-serif".to_string()
}

struct Chip<'t> {
    text: &'t str,
    size: f32,
    weight: Weight,
    fill: ([u8; 3], u8),
    ink: [u8; 3],
    edge: Option<([u8; 3], u8)>,
}

struct Portrait<'t> {
    bytes: Option<&'t [u8]>,
    d: f32,
    grey: bool,
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

    /// Small type with a dark outline, so it reads over a lit fill and over
    /// an empty track alike.
    fn outlined(&mut self, text: &str, cx: f32, baseline: f32, size: f32, color: [u8; 3]) {
        for (dx, dy) in [(-1.5, 0.0), (1.5, 0.0), (0.0, -1.5), (0.0, 1.5)] {
            self.centered(text, cx + dx, baseline + dy, size, Weight::EXTRA_BOLD, [8, 9, 12]);
        }
        self.centered(text, cx, baseline, size, Weight::EXTRA_BOLD, color);
    }

    /// One line onto a transparent layer. `blend_rect` forces its canvas
    /// opaque, which would box the letters in once the layer is turned.
    fn layer_text(&mut self, text: &str, cx: f32, baseline: f32, size: f32, color: [u8; 3]) {
        let buf = self.layout(text, size, size * 1.3, Weight::EXTRA_BOLD, None);
        let (line_y, w) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
        let (ox, oy) = ((cx - w / 2.0).round() as i32, (baseline - line_y).round() as i32);
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        buf.draw(fs, cache, Color::rgb(color[0], color[1], color[2]), |gx, gy, gw, gh, c| {
            blend_over(px, ox + gx, oy + gy, gw, gh, c);
        });
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

    /// Glyphs as an alpha mask, to paint something other than a flat colour
    /// through them.
    fn mask_of(&mut self, buf: &Buffer, x: f32, top: f32) -> Option<Mask> {
        let (pw, ph) = (self.px.width() as i32, self.px.height() as i32);
        let mut mask = Mask::new(self.px.width(), self.px.height())?;
        let (ox, oy) = (x.round() as i32, top.round() as i32);
        let (fs, cache) = (&mut *self.fs, &mut self.cache);
        let data = mask.data_mut();
        buf.draw(fs, cache, Color::rgb(255, 255, 255), |gx, gy, gw, gh, c| {
            for yy in (oy + gy).max(0)..(oy + gy + gh as i32).min(ph) {
                for xx in (ox + gx).max(0)..(ox + gx + gw as i32).min(pw) {
                    let i = (yy * pw + xx) as usize;
                    data[i] = data[i].max(c.a());
                }
            }
        });
        Some(mask)
    }

    /// A heavy word in gold: a dark outline first, then the metal poured
    /// through a mask of the glyphs so it shades from top to bottom.
    fn metal(&mut self, text: &str, cx: f32, baseline: f32, size: f32) {
        let buf = self.layout(text, size, size * 1.2, Weight::EXTRA_BOLD, None);
        let (line_y, w) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
        let (x, top) = (cx - w / 2.0, baseline - line_y);
        for (dx, dy) in OUTLINE {
            self.draw(&buf, x + dx, top + dy, [10, 9, 12]);
        }
        let Some(mask) = self.mask_of(&buf, x, top) else {
            self.draw(&buf, x, top, GOLD);
            return;
        };
        let shader = down(baseline - size * 0.74, baseline + size * 0.04, &METAL);
        let area = Rect::from_xywh(x - size * 0.4, top - size * 0.6, w + size * 0.8, size * 2.4);
        if let (Some(shader), Some(area)) = (shader, area) {
            let mut p = Paint::default();
            p.shader = shader;
            p.anti_alias = true;
            self.px.fill_rect(area, &p, Transform::identity(), Some(&mask));
        }
    }

    /// A pill with a word in it, centred on `cx`.
    fn chip(&mut self, cx: f32, y: f32, h: f32, c: Chip<'_>) {
        let w = self.measure(c.text, c.size, c.weight) + h;
        let Some(shape) = rrect(cx - w / 2.0, y, w, h, h / 2.0) else { return };
        self.px.fill_path(&shape, &paint(c.fill.0, c.fill.1), FillRule::Winding, Transform::identity(), None);
        if let Some((edge, alpha)) = c.edge {
            let stroke = Stroke { width: 2.0, ..Stroke::default() };
            self.px.stroke_path(&shape, &paint(edge, alpha), &stroke, Transform::identity(), None);
        }
        self.centered(c.text, cx, y + h / 2.0 + c.size * 0.36, c.size, c.weight, c.ink);
    }

    /// The name plate under a portrait: translucent, edged in the side's
    /// colour, with a long name cut short.
    fn plate(&mut self, cx: f32, y: f32, h: f32, name: &str, colours: ([u8; 3], [u8; 3])) {
        let name = self.fit(name.trim(), 26.0, Weight::BOLD, 272.0);
        let w = (self.measure(&name, 26.0, Weight::BOLD) + 52.0).clamp(180.0, 330.0);
        let Some(shape) = rrect(cx - w / 2.0, y, w, h, h / 2.0) else { return };
        self.px.fill_path(&shape, &paint([20, 21, 27], 214), FillRule::Winding, Transform::identity(), None);
        let stroke = Stroke { width: 2.5, ..Stroke::default() };
        self.px.stroke_path(&shape, &paint(colours.0, 235), &stroke, Transform::identity(), None);
        self.centered(&name, cx, y + h / 2.0 + 9.0, 26.0, Weight::BOLD, colours.1);
    }

    /// The slab the fight line sits on: translucent, so the glow behind it
    /// still shows, with a lit top edge to lift it off the floor.
    fn panel(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32) {
        let Some(shape) = rrect(x, y, w, h, r) else { return };
        let stops = vec![
            GradientStop::new(0.0, sk(lift(PANEL, 0.18), 222)),
            GradientStop::new(1.0, sk(dim(PANEL, 0.62), 238)),
        ];
        let shader = LinearGradient::new(
            Point::from_xy(0.0, y),
            Point::from_xy(0.0, y + h),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        );
        let mut p = paint(PANEL, 230);
        if let Some(shader) = shader {
            p.shader = shader;
        }
        p.anti_alias = true;
        self.px.fill_path(&shape, &p, FillRule::Winding, Transform::identity(), None);
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        self.px.stroke_path(&shape, &paint([64, 68, 80], 230), &stroke, Transform::identity(), None);
        if let Some(top) = rrect(x + r, y + 2.0, w - 2.0 * r, 2.0, 1.0) {
            self.px.fill_path(&top, &paint([255, 255, 255], 42), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// The picture itself, circle-cropped, or a silhouette when there is none.
    fn portrait(&mut self, cx: f32, cy: f32, p: Portrait<'_>) {
        let r = p.d / 2.0;
        match avatar_pixmap(p.bytes, p.d as u32, p.grey) {
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

/// Source-over onto a transparent canvas, in premultiplied form.
fn blend_over(px: &mut Pixmap, x: i32, y: i32, w: u32, h: u32, c: Color) {
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
                data[i + k] = ((s as u32 * a + data[i + k] as u32 * (255 - a)) / 255) as u8;
            }
            data[i + 3] = (a + data[i + 3] as u32 * (255 - a) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

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

    fn cast() -> (Fighter, Fighter) {
        (
            Fighter { name: "Rohit 🔥".to_string(), avatar: Some(fake_avatar([214, 96, 92])), hp: 68, max_hp: 100, house: super::super::house::house("gryffindor") },
            Fighter { name: "Meera".to_string(), avatar: Some(fake_avatar([112, 104, 220])), hp: 41, max_hp: 100, house: super::super::house::house("ravenclaw") },
        )
    }

    #[test]
    fn fight_card_renders_open_and_decided() {
        let (a, b) = cast();
        for (outcome, hit) in [(Outcome::Open, Some((1, -27))), (Outcome::Won(1), None)] {
            let fight = Fight {
                stage: "Round 2".to_string(),
                left: &a,
                right: &b,
                line: "Gaali nahi, bas ek chappal".to_string(),
                outcome,
                hit,
                theme: Theme::Classic,
            };
            let png = fight_png(&fight).expect("fight card");
            assert_eq!(&png[..4], &PNG_MAGIC);
        }
    }

    /// The awkward cases in one go: no picture, no health left, a heal, a
    /// miss, and a `hit` naming a side that does not exist.
    #[test]
    fn cards_survive_the_awkward_cases() {
        let a = Fighter { name: "Koi nahi".to_string(), avatar: None, hp: 0, max_hp: 0, house: None };
        let picture = Some(fake_avatar([90, 190, 160]));
        let b = Fighter { name: "ज़ैद".to_string(), avatar: picture, hp: 3, max_hp: 100, house: super::super::house::house("hufflepuff") };
        for hit in [None, Some((0, 9)), Some((1, 0)), Some((7, -5))] {
            let fight = Fight {
                stage: "Challenge".to_string(),
                left: &a,
                right: &b,
                line: String::new(),
                outcome: Outcome::Open,
                hit,
                theme: Theme::Classic,
            };
            assert_eq!(&fight_png(&fight).expect("fight card")[..4], &PNG_MAGIC);
        }
        let fight = Fight {
            stage: "Challenge".to_string(),
            left: &a,
            right: &b,
            line: String::new(),
            outcome: Outcome::Won(1),
            hit: None,
            theme: Theme::Classic,
        };
        assert_eq!(&fight_png(&fight).expect("fight card")[..4], &PNG_MAGIC);
        let champ =
            Champion { who: &a, subtitle: "12 warriors, 4 rounds, 1 champion".to_string(), line: String::new(), theme: Theme::Classic };
        assert_eq!(&champion_png(&champ).expect("champion card")[..4], &PNG_MAGIC);
    }

    #[test]
    fn champion_card_renders() {
        let (a, _) = cast();
        let champ = Champion {
            who: &a,
            subtitle: "12 warriors, 4 rounds, 1 champion".to_string(),
            line: "Rohit ne Meera ko block kar diya, aur crown utha liya".to_string(),
            theme: Theme::Classic,
        };
        let png = champion_png(&champ).expect("champion card");
        assert_eq!(&png[..4], &PNG_MAGIC);
    }

    #[test]
    fn themed_stage_reads_right() {
        assert_eq!(stage_text(Theme::Classic, "Round 2"), "Round 2");
        assert_eq!(stage_text(Theme::Pokemon, "Challenge"), "POKÉMON BATTLE");
        assert_eq!(stage_text(Theme::Wrestling, "Final"), "MAIN EVENT · Final");
    }

    #[test]
    fn every_theme_renders() {
        let (a, b) = cast();
        for theme in Theme::ALL {
            let fight = Fight {
                stage: "Final".to_string(),
                left: &a,
                right: &b,
                line: "Line".to_string(),
                outcome: Outcome::Won(0),
                hit: None,
                theme,
            };
            assert_eq!(&fight_png(&fight).expect("fight card")[..4], &PNG_MAGIC);
            let champ = Champion { who: &b, subtitle: String::new(), line: String::new(), theme };
            assert_eq!(&champion_png(&champ).expect("champion card")[..4], &PNG_MAGIC);
        }
    }

    /// Writes every theme's cards out to look at, as `fight_<key>.png`,
    /// `fight_result_<key>.png` and `champion_<key>.png`:
    /// `BATTLE_CARD_PREVIEW=/tmp cargo test battle_card -- --ignored`.
    #[test]
    #[ignore = "writes files; only useful when looking at the design"]
    fn preview() {
        let Ok(dir) = std::env::var("BATTLE_CARD_PREVIEW") else { return };
        let (a, b) = cast();
        let done = Fighter { name: "Meera".to_string(), avatar: b.avatar.clone(), hp: 0, max_hp: 100, house: b.house };
        for theme in Theme::ALL {
            // Classic keeps its old long stage, so it can be compared with earlier renders.
            let stage = if theme == Theme::Classic { "Round 2 · quarter-final" } else { "Quarter-final" };
            let mid = Fight {
                stage: stage.to_string(),
                left: &a,
                right: &b,
                line: "Rohit ne Meera ko block kar diya, aur bola: gaali nahi, bas ek chappal".to_string(),
                outcome: Outcome::Open,
                hit: Some((1, -27)),
                theme,
            };
            let over = Fight {
                stage: stage.to_string(),
                left: &a,
                right: &done,
                line: "Meera gir gayi, Rohit ne chappal hawa mein ghuma di".to_string(),
                outcome: Outcome::Won(0),
                hit: None,
                theme,
            };
            let champ = Champion {
                who: &a,
                subtitle: "12 warriors, 4 rounds, 1 champion".to_string(),
                line: "Sab ro rahe hain, Rohit chappal ghuma raha hai".to_string(),
                theme,
            };
            let key = theme.key();
            let cards = [
                (format!("fight_{key}.png"), fight_png(&mid)),
                (format!("fight_result_{key}.png"), fight_png(&over)),
                (format!("champion_{key}.png"), champion_png(&champ)),
            ];
            for (name, png) in cards {
                let bytes = png.expect("preview card");
                std::fs::write(std::path::Path::new(&dir).join(name), bytes).expect("writing the preview");
            }
        }
    }
}
