//! The duel and melee cards: Westeros, in two seasons.
//!
//! Two fighters facing off across the lists, and the champion card at the end
//! of a melee. Drawing is CPU work: callers run it on a blocking thread.
//!
//! **Two seasons, one composition.** Before the hatch nobody has a house, so
//! every fighter is shown as a dragon egg over an ember-and-old-gold banner and
//! no house name, crest or colour appears anywhere. After the hatch the same
//! card wears the member's own face over their house's banner, with the house's
//! mark beside their name. Nothing moves between the two: only the subject and
//! the palette change, so it reads as the same game in two seasons.
//!
//! Everything is painted from paths and gradients: cold stone underfoot,
//! torchlight on each side, banners hanging behind the fighters and snow coming
//! down over the lot. Most people see these on a phone, so the shapes are big,
//! the contrast is high, and nothing is finer than about two pixels.
//!
//! Nothing here picks between worlds and there is nothing to pick: the arena
//! has one world. Both cards take their fonts from the list `awards::fonts()`
//! holds, so a caller must not already be holding that lock when it calls here.

use cosmic_text::{
    Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Stretch, Style as FontStyle, SwashCache,
    Weight, Wrap,
};
use tiny_skia::{
    Color as SkColor, FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Mask, Paint,
    PathBuilder, Pixmap, PixmapPaint, Point, RadialGradient, Rect, Shader, SpreadMode, Stroke, Transform,
};

use super::awards_card::{avatar_pixmap, blend_rect, fill_circle, paint, rrect};
use super::battle_art::{self, Art};

/// Which season the arena is in. The egg week comes first, and the hatch is
/// what moves it on; `battle::hatched` is the one place that decides.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Season {
    /// Before the hatch: there are no houses, so every fighter is an unhatched
    /// egg and no house name, mark or colour is drawn.
    #[default]
    Eggs,
    /// After the hatch: fighters wear their house's banner and mark.
    Houses,
}

impl Season {
    /// Whether house paint may be drawn at all. The egg week has none, even for
    /// a fighter who somehow carries one.
    pub fn houses(self) -> bool {
        self == Season::Houses
    }
}

/// A house as this month paints it. The renderer is handed the name, the mark
/// and the colours rather than reading them off a `House`, so the month can
/// rename and recolour the four without the cards knowing anything about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HouseLook {
    /// The ledger key underneath, for looking up artwork.
    pub key: &'static str,
    /// Which painted crest and dragon go with this slot - `stark`, `watch` -
    /// or empty for a slot with no painting, which wears its initial instead.
    pub art: &'static str,
    /// "Stark", as this month calls it.
    pub name: String,
    /// The crest as Discord shows it, for the rosters and lists in chat. Never
    /// drawn on a card: a painted card uses [`HouseLook::initial`], so a month
    /// that renames the four is never shown somebody else's artwork.
    pub crest: String,
    /// One letter for the mark beside the name, e.g. "S".
    pub initial: String,
    /// Banner colours: the field, and the stripe and keyline on it.
    pub colours: ([u8; 3], [u8; 3]),
}

/// How far along an egg is: 1 cold, 5 splitting open.
pub const EGG_STAGES: u8 = 5;

/// One side of a fight. `avatar` is the raw bytes of the member's profile
/// picture, already downloaded; `None` draws a blank silhouette. In the egg
/// week neither the picture nor the house is drawn.
pub struct Fighter {
    pub name: String,
    pub avatar: Option<Vec<u8>>,
    /// Health left, 0..=max_hp.
    pub hp: u32,
    pub max_hp: u32,
    /// Their house as the month paints it, drawn as a banner and a mark.
    /// `None` for anyone unsorted, stepped out, or joined after the draft -
    /// they stay neutral rather than being given one.
    pub house: Option<HouseLook>,
    /// How far along their egg is, 1..=[`EGG_STAGES`]. Only drawn before the
    /// hatch; after it the member's own face takes the egg's place.
    pub stage: u8,
}

/// How far along a fight is when the card is drawn.
pub enum Outcome {
    /// Still going: both sides lit, no result shown.
    Open,
    /// Index 0 (left) or 1 (right) has won.
    Won(usize),
}

pub struct Fight<'a> {
    /// "The Last Eight", or "Challenge" for a duel.
    pub stage: String,
    pub left: &'a Fighter,
    pub right: &'a Fighter,
    /// The Hinglish line for this exchange.
    pub line: String,
    pub outcome: Outcome,
    /// The exchange just shown: which side was hit (0 left, 1 right) and the
    /// change to their HP - negative for damage, positive for a heal.
    pub hit: Option<(usize, i32)>,
    pub season: Season,
}

pub struct Champion<'a> {
    pub who: &'a Fighter,
    /// "12 in the lists · 4 rounds · House Stark".
    pub subtitle: String,
    pub line: String,
    pub season: Season,
}

pub(super) const BG: [u8; 3] = [15, 16, 20];
const PANEL: [u8; 3] = [30, 32, 38];
pub(super) const INK: [u8; 3] = [244, 245, 247];
pub(super) const MUTED: [u8; 3] = [148, 154, 166];
/// Torchlight on the left of the lists, cold steel on the right. Neither is a
/// house colour: a house only ever shows on its own banner and mark.
const TORCH: [u8; 3] = [214, 138, 58];
const STEEL: [u8; 3] = [106, 142, 186];
pub(super) const GOLD: [u8; 3] = [241, 196, 15];
const STAMP: [u8; 3] = [229, 72, 77];
/// Type that sits on gold or on any other lit fill.
const ON_LIGHT: [u8; 3] = [18, 19, 24];
/// The iron a card is framed and railed in.
const IRON: [u8; 3] = [116, 124, 138];
/// The egg week's whole palette: soot, ember, old gold. Nothing else.
const SOOT: [u8; 3] = [18, 16, 19];
const EMBER: [u8; 3] = [255, 134, 38];
const OLD_GOLD: [u8; 3] = [198, 158, 82];
/// A banner for a fighter with no house, in both seasons.
const NEUTRAL: ([u8; 3], [u8; 3]) = ([46, 49, 58], [120, 128, 142]);

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
/// The banner hanging behind a fighter: wider than the portrait, so its edges
/// and its swallow tail show round them.
const BANNER_W: f32 = 344.0;
const BANNER_TOP: f32 = 4.0;
const BANNER_BODY: f32 = 348.0;
const BANNER_TAIL: f32 = 394.0;

// Champion card.
const CHAMP_W: f32 = 1000.0;
const CHAMP_H: f32 = 580.0;
const CHAMP_AV: f32 = 300.0;
const CHAMP_CY: f32 = 232.0;
const CHAMP_BANNER_W: f32 = 540.0;
const CHAMP_BANNER_BODY: f32 = 330.0;
const CHAMP_BANNER_TAIL: f32 = 378.0;
/// House marks, on the portrait's shoulder.
const FIGHT_BADGE_R: f32 = 36.0;
const CHAMP_BADGE_R: f32 = 50.0;

/// The one look the arena has. There used to be seven of these behind a picker;
/// the lists are the lists, so there is one.
pub(super) struct Look {
    /// The floor, top to bottom.
    pub(super) floor: &'static [(f32, [u8; 3])],
    /// Each side's glow and ring colour.
    pub(super) left: [u8; 3],
    pub(super) right: [u8; 3],
    /// The soft seam down the middle, and its bright core.
    pub(super) seam: ([u8; 3], [u8; 3]),
    /// The deep light behind the champion, the pool under their name, and the
    /// specks thrown over the card.
    pub(super) halo: [u8; 3],
    pub(super) pool: [u8; 3],
    pub(super) rays: &'static [[u8; 3]],
    pub(super) confetti: [[u8; 3]; 4],
}

pub(super) fn look() -> Look {
    Look {
        floor: &[(0.0, [23, 27, 35]), (0.55, [14, 16, 22]), (1.0, [9, 10, 14])],
        left: TORCH,
        right: STEEL,
        seam: ([176, 190, 210], [232, 240, 252]),
        halo: [120, 86, 12],
        pool: [186, 132, 22],
        rays: &[GOLD],
        confetti: [GOLD, lift(GOLD, 0.55), [255, 244, 214], [255, 186, 120]],
    }
}

/// Words before the round on the stage chip, and the chip on its own for a
/// one-off duel, where the round would only repeat it.
const MELEE_PREFIX: &str = "THE MELEE";
const DUEL_CHIP: &str = "TRIAL BY COMBAT";
/// What the champion's ribbon says before the hatch, when there is no house to
/// name. After it, the house names itself.
const EGG_TITLE: &str = "CHAMPION OF THE LISTS";

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

/// The banner colours a fighter hangs behind them. Before the hatch that is the
/// egg week's own soot-and-ember cloth for everybody; after it, their house's,
/// or plain iron for anyone without one.
fn cloth(who: &Fighter, season: Season) -> ([u8; 3], [u8; 3]) {
    match (season.houses(), who.house.as_ref()) {
        (false, _) => (SOOT, OLD_GOLD),
        (true, Some(house)) => house.colours,
        (true, None) => NEUTRAL,
    }
}

fn draw_fight(pen: &mut Pen<'_>, f: &Fight) {
    let (left, right) = sides(&f.outcome);
    let look = look();
    floor(&mut pen.px, FIGHT_W, FIGHT_H, look.floor);
    // A fight that is over is a result card - the one people screenshot - so
    // it gets the painted hall behind it and the painted border round it. One
    // still being fought keeps the plain ground, so the two read apart at a
    // glance in a channel.
    let decided = matches!(f.outcome, Outcome::Won(_));
    let painted = decided && hall(&mut pen.px, Art::Throne, FIGHT_W, FIGHT_H, 170);
    glow(&mut pen.px, LEFT_CX + 30.0, FIGHT_CY + 20.0, 430.0, look.left, 74);
    glow(&mut pen.px, RIGHT_CX - 30.0, FIGHT_CY + 20.0, 430.0, look.right, 74);
    // Widest band first: stacking them leaves the seam soft down both sides
    // instead of ending on an edge.
    for (half, alpha) in [(124.0, 7), (88.0, 7), (54.0, 8), (26.0, 9)] {
        seam(&mut pen.px, FIGHT_W, FIGHT_H, half, look.seam.0, alpha);
    }
    seam(&mut pen.px, FIGHT_W, FIGHT_H, 2.0, look.seam.1, 24);
    // The lists themselves: banners behind each fighter, torches either side,
    // the rail they fight over and the snow coming down on all of it.
    for (cx, who) in [(LEFT_CX, f.left), (RIGHT_CX, f.right)] {
        banner(pen, cx, BANNER_W, (BANNER_TOP, BANNER_BODY, BANNER_TAIL), cloth(who, f.season));
    }
    torch(&mut pen.px, 44.0, (LEFT_CX - 40.0, PLATE_Y + 40.0), 170.0);
    torch(&mut pen.px, FIGHT_W - 44.0, (RIGHT_CX + 40.0, PLATE_Y + 40.0), 170.0);
    rail(&mut pen.px, FIGHT_W, PLATE_Y - 18.0);
    mist(&mut pen.px, FIGHT_W, FIGHT_H);
    snow(&mut pen.px, FIGHT_W, PANEL_Y - 10.0, 70, f.season);
    vignette(&mut pen.px, FIGHT_W, FIGHT_H, 150);
    if !(painted && painted_frame(&mut pen.px, Art::Frame("rare"), FIGHT_W, FIGHT_H, 38.0)) {
        frame(&mut pen.px, FIGHT_W, FIGHT_H);
    }

    stage_chip(pen, &stage_text(f.stage.trim()));
    fighter(pen, f.left, LEFT_CX, look.left, left, f.season);
    fighter(pen, f.right, RIGHT_CX, look.right, right, f.season);
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

/// The chip's words: the melee before the round, or the duel's own words for a
/// one-off challenge, where "CHALLENGE" would only repeat it.
fn stage_text(stage: &str) -> String {
    if stage.is_empty() || stage.eq_ignore_ascii_case("challenge") {
        return DUEL_CHIP.to_string();
    }
    format!("{MELEE_PREFIX} · {stage}")
}

/// What the winner's pill says: their house, once there is one to name.
fn winner_pill(who: &Fighter, season: Season) -> String {
    match (season.houses(), who.house.as_ref()) {
        (true, Some(house)) => format!("WINNER · HOUSE {}", house.name.to_uppercase()),
        _ => "WINNER".to_string(),
    }
}

/// Small, letter-spaced and outlined: "THE MELEE · THE LAST EIGHT".
fn stage_chip(pen: &mut Pen<'_>, stage: &str) {
    if stage.is_empty() {
        return;
    }
    let text = spaced(&stage.to_uppercase());
    let text = pen.fit(&text, 15.0, Weight::SEMIBOLD, 420.0);
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

/// One side: glow, shadow, the ringed subject - a face after the hatch, an egg
/// before it - the name plate and, once the fight is decided, a winner's pill
/// or a YIELD stamp.
fn fighter(pen: &mut Pen<'_>, who: &Fighter, cx: f32, colour: [u8; 3], side: Side, season: Season) {
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
        glow(&mut pen.px, cx, FIGHT_CY, outer + 76.0, band.0, halo);
    }
    shadow(&mut pen.px, cx, FIGHT_CY + 16.0, outer);
    ring(&mut pen.px, cx, FIGHT_CY, outer, band);
    fill_circle(&mut pen.px, cx, FIGHT_CY, AV / 2.0 + GAP, [12, 13, 17]);
    subject(pen, who, cx, FIGHT_CY, AV, season, lost);
    if lost {
        wash(&mut pen.px, cx, FIGHT_CY, AV / 2.0, [8, 9, 12], 142);
    }
    if matches!(side, Side::Winner) {
        crown(&mut pen.px, cx, FIGHT_CY - outer + 16.0, 82.0, 34.0);
    }
    if let (true, Some(house)) = (season.houses(), who.house.as_ref()) {
        // On the outer shoulder, away from the VS between the two.
        let d = outer * std::f32::consts::FRAC_1_SQRT_2;
        let bx = if cx < FIGHT_W / 2.0 { cx - d } else { cx + d };
        house_mark(pen, house, bx, FIGHT_CY + d, FIGHT_BADGE_R, lost);
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
            let text = spaced(&winner_pill(who, season));
            let pill = Chip {
                text: &text,
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

/// What stands inside the ring: the member's own face after the hatch, their
/// dragon egg before it.
fn subject(pen: &mut Pen<'_>, who: &Fighter, cx: f32, cy: f32, d: f32, season: Season, lost: bool) {
    if season.houses() {
        pen.portrait(cx, cy, Portrait { bytes: who.avatar.as_deref(), d, grey: lost });
        return;
    }
    // The egg sits in its own pool of soot, so it reads as a thing in the dark
    // rather than a shape cut out of the floor.
    fill_circle(&mut pen.px, cx, cy, d / 2.0, SOOT);
    egg(pen, cx, cy, d, who.stage, lost);
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

/// "YIELD", struck across the loser at an angle.
fn stamp(pen: &mut Pen<'_>, cx: f32, cy: f32) {
    let (w, h) = (236.0, 78.0);
    on_layer(pen, cx, cy, (w, h), -13.0, |p: &mut Pen<'_>| {
        if let Some(shape) = rrect(4.0, 4.0, w - 8.0, h - 8.0, 14.0) {
            p.px.fill_path(&shape, &paint([10, 8, 10], 130), FillRule::Winding, Transform::identity(), None);
            let stroke = Stroke { width: 5.0, ..Stroke::default() };
            p.px.stroke_path(&shape, &paint(STAMP, 235), &stroke, Transform::identity(), None);
        }
        p.layer_text(&spaced("YIELD"), w / 2.0, h / 2.0 + 13.0, 34.0, STAMP);
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

/// What the champion's ribbon says: the house that won, once there is one.
fn champion_title(who: &Fighter, season: Season) -> String {
    match (season.houses(), who.house.as_ref()) {
        (true, Some(house)) => format!("{} CHAMPION", house.name.to_uppercase()),
        (true, None) => "CHAMPION OF THE LISTS".to_string(),
        (false, _) => EGG_TITLE.to_string(),
    }
}

fn draw_champion(pen: &mut Pen<'_>, c: &Champion) {
    let outer = CHAMP_AV / 2.0 + GAP + RING;
    let look = look();
    floor(&mut pen.px, CHAMP_W, CHAMP_H, look.floor);
    // The throne room behind them, or the old burst of rays when the month's
    // paintings have not been copied in.
    let painted = hall(&mut pen.px, Art::Throne, CHAMP_W, CHAMP_H, 150);
    glow(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, 430.0, look.halo, if painted { 90 } else { 150 });
    if !painted {
        rays(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, outer + 10.0, 760.0, look.rays);
    }
    // Without a painted hall the banner is what the champion stands against;
    // with one, the hall is, and a banner over it is a layer too many.
    if !painted {
        banner(
            pen,
            CHAMP_W / 2.0,
            CHAMP_BANNER_W,
            (BANNER_TOP, CHAMP_BANNER_BODY, CHAMP_BANNER_TAIL),
            cloth(c.who, c.season),
        );
    }
    for lamp in [120.0, CHAMP_W - 120.0] {
        torch(&mut pen.px, lamp, (CHAMP_W / 2.0, CHAMP_H - 120.0), 210.0);
    }
    snow(&mut pen.px, CHAMP_W, CHAMP_H - 150.0, 90, c.season);
    confetti(&mut pen.px, CHAMP_W, CHAMP_H, (CHAMP_W / 2.0, CHAMP_CY, outer + 26.0), champion_specks(c.season, &look));
    vignette(&mut pen.px, CHAMP_W, CHAMP_H, 170);
    // The painted border, or the iron rule when there is none.
    if !painted_frame(&mut pen.px, Art::Frame("rare"), CHAMP_W, CHAMP_H, 46.0) {
        frame(&mut pen.px, CHAMP_W, CHAMP_H);
    }
    // A warm pool of light under the name, so the type sits in the glow.
    glow(&mut pen.px, CHAMP_W / 2.0, 462.0, 330.0, look.pool, 78);

    glow(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, outer + 90.0, lift(GOLD, 0.3), 110);
    shadow(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY + 20.0, outer);
    ring(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, outer, (lift(GOLD, 0.6), dim(GOLD, 0.58)));
    fill_circle(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY, CHAMP_AV / 2.0 + GAP, [12, 13, 17]);
    subject(pen, c.who, CHAMP_W / 2.0, CHAMP_CY, CHAMP_AV, c.season, false);
    // After the ring, so the crown rests on it.
    crown(&mut pen.px, CHAMP_W / 2.0, CHAMP_CY - outer + 8.0, 140.0, 60.0);
    if let (true, Some(house)) = (c.season.houses(), c.who.house.as_ref()) {
        // Low on the right of the ring, clear of the ribbon underneath.
        let angle = 30f32.to_radians();
        let (bx, by) = (CHAMP_W / 2.0 + outer * angle.cos(), CHAMP_CY + outer * angle.sin());
        house_mark(pen, house, bx, by, CHAMP_BADGE_R, false);
    }
    ribbon(pen, &champion_title(c.who, c.season));

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

/// What falls over the champion: gold in both seasons, warmed to ember before
/// the hatch so nothing on an egg-week card is any colour but soot, ember and
/// old gold.
fn champion_specks(season: Season, look: &Look) -> [[u8; 3]; 4] {
    match season {
        Season::Houses => look.confetti,
        Season::Eggs => [EMBER, OLD_GOLD, lift(EMBER, 0.45), dim(OLD_GOLD, 0.7)],
    }
}

// --- the lists --------------------------------------------------------------

/// A banner hanging behind a fighter: an iron rail at the top, a dyed field
/// with a stripe down each edge, and a swallow tail at the foot. The portrait
/// covers the middle, so what reads is the colour either side of it and the
/// tail below.
///
/// ART SEAM: if a painted banner ever arrives per house, drop it in and let
/// [`banner_art`] return it - the field is replaced and the rail, stripes and
/// tail stay exactly where they are.
fn banner(pen: &mut Pen<'_>, cx: f32, w: f32, ends: (f32, f32, f32), colours: ([u8; 3], [u8; 3])) {
    let (top, body, tail) = ends;
    let (field, trim) = colours;
    let x = cx - w / 2.0;
    // The cloth: the field, with the swallow tail cut into the bottom edge.
    let mut pb = PathBuilder::new();
    pb.move_to(x, top);
    pb.line_to(x + w, top);
    pb.line_to(x + w, body);
    pb.line_to(cx, tail);
    pb.line_to(x, body);
    pb.close();
    let Some(cloth) = pb.finish() else { return };
    let shader = down(top, tail, &[(0.0, lift(field, 0.26)), (0.4, field), (1.0, dim(field, 0.42))]);
    fill_shaded(&mut pen.px, &cloth, shader, field);
    // The weave and the hem are painted through a mask the size of the card,
    // which a banner has to be worth: a narrow one is a sliver of colour and
    // the detail would not show.
    let Some(mut mask) = (w >= 200.0).then(|| Mask::new(pen.px.width(), pen.px.height())).flatten() else {
        let stroke = Stroke { width: 3.0, line_join: LineJoin::Round, ..Stroke::default() };
        pen.px.stroke_path(&cloth, &paint(lift(trim, 0.2), 215), &stroke, Transform::identity(), None);
        return;
    };
    mask.fill_path(&cloth, FillRule::Winding, true, Transform::identity());
    // Woven, so the cloth is cloth and not a painted rectangle: faint vertical
    // threads, and the long fold either side of where the shield will hang.
    let mut weave = PathBuilder::new();
    let mut tx = x + 6.0;
    while tx < x + w {
        weave.move_to(tx, top);
        weave.line_to(tx, tail);
        tx += 12.0;
    }
    if let Some(path) = weave.finish() {
        let stroke = Stroke { width: 1.0, ..Stroke::default() };
        pen.px.stroke_path(&path, &paint([0, 0, 0], 26), &stroke, Transform::identity(), Some(&mask));
    }
    for (at, width, alpha) in [(-0.34f32, 16.0, 40), (0.34, 16.0, 26)] {
        if let Some(fold) = Rect::from_xywh(cx + at * w - width / 2.0, top, width, tail - top) {
            pen.px.fill_rect(fold, &paint([0, 0, 0], alpha), Transform::identity(), Some(&mask));
        }
    }
    // The border in the house's second colour, run round the whole cloth so it
    // reads as a hem rather than as two poles behind the portrait.
    let mut hem = PathBuilder::new();
    hem.move_to(x + 13.0, top);
    hem.line_to(x + 13.0, body - 5.0);
    hem.line_to(cx, tail - 13.0);
    hem.line_to(x + w - 13.0, body - 5.0);
    hem.line_to(x + w - 13.0, top);
    if let Some(path) = hem.finish() {
        let stroke = Stroke { width: 7.0, line_join: LineJoin::Round, ..Stroke::default() };
        pen.px.stroke_path(&path, &paint(trim, 170), &stroke, Transform::identity(), Some(&mask));
    }
    // The cut edges themselves, picked out so the tail has a shape.
    let mut edge = PathBuilder::new();
    edge.move_to(x, body);
    edge.line_to(cx, tail);
    edge.line_to(x + w, body);
    if let Some(path) = edge.finish() {
        let stroke = Stroke { width: 3.0, line_join: LineJoin::Round, ..Stroke::default() };
        pen.px.stroke_path(&path, &paint(lift(trim, 0.2), 215), &stroke, Transform::identity(), None);
    }
    // The rail it hangs from, with a ring at each end.
    if let Some(bar) = rrect(x - 20.0, top + 2.0, w + 40.0, 10.0, 5.0) {
        let shader = down(top + 2.0, top + 12.0, &[(0.0, lift(IRON, 0.45)), (1.0, dim(IRON, 0.35))]);
        fill_shaded(&mut pen.px, &bar, shader, IRON);
    }
    for end in [x - 12.0, x + w + 12.0] {
        fill_circle(&mut pen.px, end, top + 7.0, 7.5, dim(IRON, 0.55));
        fill_circle(&mut pen.px, end, top + 7.0, 4.0, [10, 11, 14]);
    }
}

/// A painted banner for a house, if one has been supplied. Nothing is shipped
/// yet, so every banner is drawn; put `banners/<key>.png` beside the crests and
/// add it to this table and it is used instead of the dyed field.
#[allow(dead_code)]
fn banner_art(_key: &str) -> Option<&'static [u8]> {
    const BANNERS: [(&str, &[u8]); 0] = [];
    BANNERS.iter().find(|(k, _)| *k == _key).map(|(_, bytes)| *bytes)
}

/// A torch on the wall at `lamp_x`, throwing its light down onto the lists.
fn torch(px: &mut Pixmap, lamp_x: f32, (cx, floor_y): (f32, f32), spread: f32) {
    let tint = [255, 206, 142];
    for (widen, alpha) in [(1.25, 14.0), (1.0, 18.0), (0.72, 22.0)] {
        let mut pb = PathBuilder::new();
        pb.move_to(lamp_x - 12.0 * widen, -10.0);
        pb.line_to(lamp_x + 12.0 * widen, -10.0);
        pb.line_to(cx + spread * widen, floor_y);
        pb.line_to(cx - spread * widen, floor_y);
        pb.close();
        let Some(cone) = pb.finish() else { continue };
        let stops = vec![GradientStop::new(0.0, sk(tint, alpha as u8)), GradientStop::new(1.0, sk(tint, 0))];
        let shader = LinearGradient::new(
            Point::from_xy(lamp_x, 0.0),
            Point::from_xy(cx, floor_y),
            stops,
            SpreadMode::Pad,
            Transform::identity(),
        );
        fill_shaded(px, &cone, shader, tint);
    }
    glow(px, lamp_x, 10.0, 90.0, [255, 170, 70], 90);
}

/// The rail down the middle of the lists, faint and low behind the fighters.
fn rail(px: &mut Pixmap, w: f32, y: f32) {
    for (dy, width, alpha) in [(0.0, 7.0, 60), (14.0, 4.0, 36)] {
        let mut pb = PathBuilder::new();
        pb.move_to(-10.0, y + dy);
        pb.line_to(w + 10.0, y + dy);
        let Some(line) = pb.finish() else { continue };
        let stroke = Stroke { width, line_cap: LineCap::Round, ..Stroke::default() };
        px.stroke_path(&line, &paint(IRON, alpha), &stroke, Transform::identity(), None);
    }
    let mut posts = PathBuilder::new();
    let mut x = 60.0;
    while x < w {
        posts.move_to(x, y - 16.0);
        posts.line_to(x, y + 22.0);
        x += 112.0;
    }
    if let Some(path) = posts.finish() {
        let stroke = Stroke { width: 5.0, line_cap: LineCap::Round, ..Stroke::default() };
        px.stroke_path(&path, &paint(IRON, 40), &stroke, Transform::identity(), None);
    }
}

/// Snow coming down over the card, kept above `bottom` and out of the chip's
/// letters. In the egg week a few of the flakes are ash and ember instead.
fn snow(px: &mut Pixmap, w: f32, bottom: f32, n: usize, season: Season) {
    let mut next = scatter(0x7F4A_7C15);
    for i in 0..n {
        let (x, y) = ((next() % w as u32) as f32, (next() % bottom.max(1.0) as u32) as f32);
        let r = 1.0 + (next() % 18) as f32 / 10.0;
        if under_chip(x, y) {
            continue;
        }
        let flake = [226, 234, 244];
        let c = match (season, i % 6) {
            (Season::Eggs, 0) => EMBER,
            (Season::Eggs, 1) => OLD_GOLD,
            _ => flake,
        };
        if c != flake {
            glow(px, x, y, r * 5.0, c, 70);
        }
        wash(px, x, y, r, c, 110 + (next() % 110) as u8);
    }
}

/// Cold fog lying along the bottom of the card, as flattened pools.
fn mist(px: &mut Pixmap, w: f32, h: f32) {
    let tint = [150, 158, 170];
    let pools = [(0.12, 0.78, 300.0, 54), (0.52, 0.88, 380.0, 44), (0.9, 0.76, 300.0, 54), (0.3, 0.98, 320.0, 42)];
    for (fx, fy, r, alpha) in pools {
        let (cx, cy) = (w * fx, h * fy);
        let stops = vec![GradientStop::new(0.0, sk(tint, alpha)), GradientStop::new(1.0, sk(tint, 0))];
        let squash = Transform::from_row(1.0, 0.0, 0.0, 0.28, cx, cy);
        let shader = RadialGradient::new(Point::zero(), Point::zero(), r, stops, SpreadMode::Pad, squash);
        if let (Some(shader), Some(area)) = (shader, Rect::from_xywh(cx - r, cy - r * 0.28, 2.0 * r, r * 0.56)) {
            let mut p = Paint::default();
            p.shader = shader;
            px.fill_rect(area, &p, Transform::identity(), None);
        }
    }
}

/// A painted hall behind the card, darkened well back so the type still reads.
/// `strength` is how much black goes over it: a card with a lot of words on it
/// wants more. Returns whether there was a painting at all.
fn hall(px: &mut Pixmap, art: Art, w: f32, h: f32, strength: u8) -> bool {
    let Some(painted) = battle_art::cover(art, w as u32, h as u32) else {
        return false;
    };
    let how = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    px.draw_pixmap(0, 0, painted.as_ref().as_ref(), &how, Transform::identity(), None);
    // Pulled down hard. These are the cards people screenshot and the words on
    // them are the point; the hall is there to be felt, not read.
    if let Some(area) = Rect::from_xywh(0.0, 0.0, w, h) {
        px.fill_rect(area, &paint([6, 6, 9], strength), Transform::identity(), None);
    }
    true
}

/// A painted border round the card, fitted as a nine-slice so its corners keep
/// their own shape. `None` when the art is not there, and the caller falls back
/// to the iron rule.
fn painted_frame(px: &mut Pixmap, art: Art, w: f32, h: f32, band: f32) -> bool {
    let Some(border) = battle_art::nine_slice(art, w as u32, h as u32, 0.20, band.round() as u32) else {
        return false;
    };
    let how = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
    px.draw_pixmap(0, 0, border.as_ref().as_ref(), &how, Transform::identity(), None);
    true
}

/// The card's border: a double rule of cold iron with a stud in each corner. It
/// sits inside the stage chip and under the crown.
fn frame(px: &mut Pixmap, w: f32, h: f32) {
    for (inset, width, alpha, r) in [(6.0, 2.5, 150, 14.0), (12.0, 2.0, 80, 9.0)] {
        if let Some(rule) = rrect(inset, inset, w - 2.0 * inset, h - 2.0 * inset, r) {
            let stroke = Stroke { width, ..Stroke::default() };
            px.stroke_path(&rule, &paint(IRON, alpha), &stroke, Transform::identity(), None);
        }
    }
    for (x, y) in [(20.0, 20.0), (w - 20.0, 20.0), (20.0, h - 20.0), (w - 20.0, h - 20.0)] {
        diamond(px, x, y, 6.0, lift(IRON, 0.4), 200);
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

// --- the scroll card --------------------------------------------------------

// A scroll: the two fighters along the top, the score between them, and the
// puzzle itself drawn on parchment underneath. The puzzle is ON the card
// because a modal is text only - a picture nobody can see is no puzzle at all.
const SCROLL_W: f32 = 1000.0;
const SCROLL_H: f32 = 620.0;
/// The two fighters' discs, top left and top right.
const SCROLL_AV: f32 = 104.0;
const SCROLL_FACE_CY: f32 = 84.0;
const SCROLL_LEFT_CX: f32 = 86.0;
const SCROLL_RIGHT_CX: f32 = SCROLL_W - SCROLL_LEFT_CX;
/// The parchment.
const VELLUM_X: f32 = 62.0;
const VELLUM_Y: f32 = 158.0;
const VELLUM_W: f32 = SCROLL_W - 2.0 * VELLUM_X;
const VELLUM_H: f32 = 396.0;
/// Where the puzzle is drawn, inside the parchment and under the prompt.
/// Clear of a prompt that runs to two lines.
const PANEL_TOP: f32 = 262.0;
const PANEL_BOTTOM: f32 = 530.0;
/// Parchment, and the ink on it.
const VELLUM: [u8; 3] = [228, 214, 184];
const VELLUM_EDGE: [u8; 3] = [150, 132, 102];
const INK_DARK: [u8; 3] = [42, 34, 26];

pub struct Scroll<'a> {
    pub left: &'a Fighter,
    pub right: &'a Fighter,
    /// "Scroll 2 of 3".
    pub number: String,
    /// Scrolls won so far, left then right.
    pub score: [u32; 2],
    /// The question, printed at the head of the parchment.
    pub prompt: &'a str,
    /// What to draw under it. An empty spec leaves the parchment to the words.
    pub spec: &'a super::battle_scroll::Spec,
    pub season: Season,
    /// How long the scroll stands before it burns.
    pub seconds: u64,
}

/// PNG bytes of a scroll card, or `None` if drawing failed.
pub fn scroll_png(scroll: &Scroll) -> Option<Vec<u8>> {
    let mut fs = super::awards::fonts().lock();
    if fs.db().len() == 0 {
        return None;
    }
    let mut pen = Pen::new(SCROLL_W, SCROLL_H, &mut fs)?;
    draw_scroll(&mut pen, scroll);
    pen.px.encode_png().ok()
}

fn draw_scroll(pen: &mut Pen<'_>, s: &Scroll) {
    let look = look();
    floor(&mut pen.px, SCROLL_W, SCROLL_H, look.floor);
    glow(&mut pen.px, SCROLL_LEFT_CX + 40.0, SCROLL_FACE_CY, 300.0, look.left, 70);
    glow(&mut pen.px, SCROLL_RIGHT_CX - 40.0, SCROLL_FACE_CY, 300.0, look.right, 70);
    // The same banners the fight card hangs, cut short: only the heads show
    // above the parchment, which is all there is room for.
    for (cx, who) in [(SCROLL_LEFT_CX, s.left), (SCROLL_RIGHT_CX, s.right)] {
        banner(pen, cx, 186.0, (2.0, 112.0, 142.0), cloth(who, s.season));
    }
    torch(&mut pen.px, 36.0, (SCROLL_LEFT_CX, VELLUM_Y), 120.0);
    torch(&mut pen.px, SCROLL_W - 36.0, (SCROLL_RIGHT_CX, VELLUM_Y), 120.0);
    snow(&mut pen.px, SCROLL_W, VELLUM_Y - 8.0, 44, s.season);
    vignette(&mut pen.px, SCROLL_W, SCROLL_H, 150);
    frame(&mut pen.px, SCROLL_W, SCROLL_H);

    // The two fighters and the score between them.
    scroll_fighter(pen, s.left, SCROLL_LEFT_CX, look.left, s.season);
    scroll_fighter(pen, s.right, SCROLL_RIGHT_CX, look.right, s.season);
    scoreline(pen, s);

    vellum(pen);
    let prompt = pen.paragraph(s.prompt.trim(), 27.0, 34.0, VELLUM_W - 96.0, 2);
    let top = VELLUM_Y + 26.0;
    pen.draw(&prompt, VELLUM_X + 48.0, top, INK_DARK);
    puzzle(pen, s.spec);

    let foot = format!(
        "First to read it lands the blow \u{00b7} the scroll burns in {}s",
        s.seconds
    );
    pen.centered(&foot, SCROLL_W / 2.0, SCROLL_H - 30.0, 19.0, Weight::MEDIUM, [186, 191, 202]);
}

/// One fighter on a scroll card: the ringed subject, their name and their
/// health, small enough to leave the parchment the room.
fn scroll_fighter(pen: &mut Pen<'_>, who: &Fighter, cx: f32, colour: [u8; 3], season: Season) {
    let outer = SCROLL_AV / 2.0 + 4.0 + 6.0;
    let band = (lift(colour, 0.42), dim(colour, 0.66));
    glow(&mut pen.px, cx, SCROLL_FACE_CY, outer + 48.0, band.0, 58);
    shadow(&mut pen.px, cx, SCROLL_FACE_CY + 10.0, outer);
    ring(&mut pen.px, cx, SCROLL_FACE_CY, outer, band);
    fill_circle(&mut pen.px, cx, SCROLL_FACE_CY, SCROLL_AV / 2.0 + 4.0, [12, 13, 17]);
    subject(pen, who, cx, SCROLL_FACE_CY, SCROLL_AV, season, false);
    if let (true, Some(house)) = (season.houses(), who.house.as_ref()) {
        // Inboard here, unlike the fight card: these discs sit in the corners
        // of the card and the outer shoulder would hang off the edge.
        let d = outer * std::f32::consts::FRAC_1_SQRT_2;
        let bx = if cx < SCROLL_W / 2.0 { cx + d } else { cx - d };
        house_mark(pen, house, bx, SCROLL_FACE_CY + d, 22.0, false);
    }
    // The name and the bar sit inboard of the disc, where there is room.
    let inboard = if cx < SCROLL_W / 2.0 { 1.0 } else { -1.0 };
    let plate_cx = cx + inboard * (outer + 108.0);
    pen.plate(plate_cx, SCROLL_FACE_CY - 34.0, 38.0, &who.name, (colour, INK));
    let max = who.max_hp.max(1);
    let hp = who.hp.min(max);
    let (bw, bh) = (196.0, 18.0);
    let (bx, by) = (plate_cx - bw / 2.0, SCROLL_FACE_CY + 16.0);
    if let Some(track) = rrect(bx, by, bw, bh, bh / 2.0) {
        pen.px.fill_path(&track, &paint([16, 17, 22], 235), FillRule::Winding, Transform::identity(), None);
    }
    let frac = hp as f32 / max as f32;
    if frac > 0.0 {
        let fw = (bw * frac).max(bh);
        let fx = if inboard > 0.0 { bx } else { bx + bw - fw };
        let c = health_colour(frac);
        if let Some(fill) = rrect(fx, by, fw, bh, bh / 2.0) {
            let shader = down(by, by + bh, &[(0.0, lift(c, 0.38)), (1.0, dim(c, 0.62))]);
            fill_shaded(&mut pen.px, &fill, shader, c);
        }
    }
    pen.outlined(&format!("{hp}/{max}"), plate_cx, by + bh / 2.0 + 4.8, 13.0, [248, 249, 252]);
}

/// The scroll's number and the score, between the two fighters.
fn scoreline(pen: &mut Pen<'_>, s: &Scroll) {
    let cx = SCROLL_W / 2.0;
    let number = pen.fit(&spaced(&s.number.to_uppercase()), 15.0, Weight::SEMIBOLD, 300.0);
    let chip = Chip {
        text: &number,
        size: 15.0,
        weight: Weight::SEMIBOLD,
        fill: ([255, 255, 255], 18),
        ink: [226, 229, 238],
        edge: Some(([255, 255, 255], 90)),
    };
    pen.chip(cx, 16.0, 34.0, chip);
    pen.metal(&format!("{} \u{2013} {}", s.score[0], s.score[1]), cx, 104.0, 54.0);
    pen.centered(&spaced("SCROLLS WON"), cx, 136.0, 12.0, Weight::SEMIBOLD, MUTED);
}

/// The parchment: an aged sheet with real fibre and staining in it, hung
/// between two turned rollers. Everything on it has to stay readable at a
/// glance - that is the whole game - so the texture is quiet and the ink is
/// dark.
fn vellum(pen: &mut Pen<'_>) {
    let Some(sheet) = rrect(VELLUM_X, VELLUM_Y, VELLUM_W, VELLUM_H, 10.0) else { return };
    let shader = down(
        VELLUM_Y,
        VELLUM_Y + VELLUM_H,
        &[(0.0, lift(VELLUM, 0.35)), (0.4, VELLUM), (1.0, dim(VELLUM, 0.88))],
    );
    fill_shaded(&mut pen.px, &sheet, shader, VELLUM);
    // Everything that follows is kept inside the sheet.
    let Some(mut mask) = Mask::new(pen.px.width(), pen.px.height()) else { return };
    mask.fill_path(&sheet, FillRule::Winding, true, Transform::identity());

    // Westeros itself, so faint it is felt rather than seen. The map is the
    // month's own painting; at this strength it never fights the puzzle.
    if let Some(art) = battle_art::cover(Art::Map, VELLUM_W as u32, VELLUM_H as u32) {
        let how = PixmapPaint { quality: FilterQuality::Bilinear, opacity: 0.10, ..PixmapPaint::default() };
        let at = Transform::from_translate(VELLUM_X, VELLUM_Y);
        pen.px.draw_pixmap(0, 0, art.as_ref().as_ref(), &how, at, None);
    }

    let mut next = scatter(0x5BD1_2C77);
    // The fibres: long, nearly flat strokes lying the way the sheet was pressed.
    let mut fibres = PathBuilder::new();
    for _ in 0..70 {
        let x = VELLUM_X + (next() % VELLUM_W as u32) as f32;
        let y = VELLUM_Y + (next() % VELLUM_H as u32) as f32;
        let len = 30.0 + (next() % 150) as f32;
        fibres.move_to(x, y);
        fibres.quad_to(x + len / 2.0, y + ((next() % 5) as f32 - 2.0), x + len, y);
    }
    if let Some(path) = fibres.finish() {
        let stroke = Stroke { width: 1.0, ..Stroke::default() };
        pen.px.stroke_path(&path, &paint([150, 128, 92], 26), &stroke, Transform::identity(), Some(&mask));
    }
    // Age: broad soft stains, heavier towards the edges where a sheet is handled.
    for _ in 0..26 {
        let x = VELLUM_X + (next() % VELLUM_W as u32) as f32;
        let y = VELLUM_Y + (next() % VELLUM_H as u32) as f32;
        let r = 10.0 + (next() % 54) as f32;
        glow(&mut pen.px, x, y, r, [148, 120, 76], 12 + (next() % 10) as u8);
    }
    // The sheet's own edge and the shadow it casts into its curl.
    let stroke = Stroke { width: 3.0, ..Stroke::default() };
    pen.px.stroke_path(&sheet, &paint(VELLUM_EDGE, 220), &stroke, Transform::identity(), None);
    for (y, h) in [(VELLUM_Y, 16.0), (VELLUM_Y + VELLUM_H - 16.0, 16.0)] {
        if let Some(area) = Rect::from_xywh(VELLUM_X, y, VELLUM_W, h) {
            pen.px.fill_rect(area, &paint([92, 74, 48], 26), Transform::identity(), Some(&mask));
        }
    }
    roller(pen, VELLUM_Y - 11.0);
    roller(pen, VELLUM_Y + VELLUM_H - 13.0);
}

/// A turned wooden roller across the sheet, with a cap at each end: the thing
/// a scroll is actually wound on, rather than a rounded bar.
fn roller(pen: &mut Pen<'_>, y: f32) {
    let (x, w, h) = (VELLUM_X - 26.0, VELLUM_W + 52.0, 24.0);
    let wood = [(0.0, [122, 86, 52]), (0.34, [86, 58, 34]), (0.62, [58, 38, 22]), (1.0, [34, 22, 13])];
    if let Some(barrel) = rrect(x, y, w, h, h / 2.0) {
        let shader = down(y, y + h, &wood);
        fill_shaded(&mut pen.px, &barrel, shader, [86, 58, 34]);
        let stroke = Stroke { width: 2.0, ..Stroke::default() };
        pen.px.stroke_path(&barrel, &paint([26, 17, 10], 215), &stroke, Transform::identity(), None);
    }
    // The grain along it, and the light down its top.
    if let Some(sheen) = rrect(x + 10.0, y + 3.5, w - 20.0, 4.0, 2.0) {
        pen.px.fill_path(&sheen, &paint([214, 176, 126], 60), FillRule::Winding, Transform::identity(), None);
    }
    // The turned caps, and the collar inside each one.
    for (at, side) in [(x + 16.0, -1.0f32), (x + w - 16.0, 1.0)] {
        if let Some(collar) = rrect(at - 5.0, y - 3.0, 10.0, h + 6.0, 4.0) {
            let shader = down(y - 3.0, y + h + 3.0, &[(0.0, [150, 112, 62]), (1.0, [62, 42, 24])]);
            fill_shaded(&mut pen.px, &collar, shader, [96, 68, 38]);
        }
        let cap = at + side * 12.0;
        fill_circle(&mut pen.px, cap, y + h / 2.0, h * 0.62, [40, 27, 16]);
        fill_circle(&mut pen.px, cap, y + h / 2.0, h * 0.50, [118, 84, 48]);
        fill_circle(&mut pen.px, cap, y + h / 2.0, h * 0.30, [72, 50, 29]);
        wash(&mut pen.px, cap - h * 0.14, y + h / 2.0 - h * 0.16, h * 0.12, [226, 194, 148], 90);
    }
}

/// The puzzle itself, laid out inside the parchment: the placed shapes, the
/// scorched number on a shield, or the two hosts.
fn puzzle(pen: &mut Pen<'_>, spec: &super::battle_scroll::Spec) {
    let (top, bottom) = (PANEL_TOP, PANEL_BOTTOM);
    let (x0, x1) = (VELLUM_X + 36.0, VELLUM_X + VELLUM_W - 36.0);
    if let Some((left, right)) = spec.armies {
        hosts(pen, (x0, top, x1, bottom), left, right);
        return;
    }
    if !spec.digits.is_empty() {
        scorched(pen, (x0 + x1) / 2.0, (top + bottom) / 2.0, bottom - top, &spec.digits);
        return;
    }
    // One size for every shape on a card, so nothing is bigger than its
    // neighbour by accident: the row's width decides it.
    let n = spec.items.len().max(1) as f32;
    let side = (((x1 - x0) / n) * 0.80).min((bottom - top) * 0.46);
    for item in &spec.items {
        let cx = x0 + item.x * (x1 - x0);
        let cy = top + item.y * (bottom - top);
        glyph(pen, item, cx, cy, side * item.scale);
    }
}

/// One shape, drawn to fit a box of `side`, outlined so a pale dye still reads
/// on the parchment.
fn glyph(pen: &mut Pen<'_>, item: &super::battle_scroll::Item, cx: f32, cy: f32, side: f32) {
    use super::battle_scroll::Glyph;
    let s = side / 2.0;
    let turn = match (item.flip, item.upside) {
        (true, _) => Transform::from_scale(-1.0, 1.0).post_translate(2.0 * cx, 0.0),
        (_, true) => Transform::from_scale(1.0, -1.0).post_translate(0.0, 2.0 * cy),
        _ => Transform::identity(),
    };
    let mut pb = PathBuilder::new();
    let mut extra: Vec<(tiny_skia::Path, [u8; 3], bool)> = Vec::new();
    match item.glyph {
        Glyph::Sword => {
            // A blade, a crossguard and a grip. A snapped one is drawn in two
            // pieces with the break between them.
            let gap = if item.lit { 0.0 } else { s * 0.22 };
            pb.move_to(cx, cy - s);
            pb.line_to(cx + s * 0.16, cy - s * 0.74);
            pb.line_to(cx + s * 0.16, cy - gap);
            pb.line_to(cx - s * 0.16, cy - gap);
            pb.line_to(cx - s * 0.16, cy - s * 0.74);
            pb.close();
            pb.move_to(cx - s * 0.16, cy + gap);
            pb.line_to(cx + s * 0.16, cy + gap);
            pb.line_to(cx + s * 0.16, cy + s * 0.44);
            pb.line_to(cx - s * 0.16, cy + s * 0.44);
            pb.close();
            pb.move_to(cx - s * 0.52, cy + s * 0.44);
            pb.line_to(cx + s * 0.52, cy + s * 0.44);
            pb.line_to(cx + s * 0.52, cy + s * 0.60);
            pb.line_to(cx - s * 0.52, cy + s * 0.60);
            pb.close();
            pb.move_to(cx - s * 0.10, cy + s * 0.60);
            pb.line_to(cx + s * 0.10, cy + s * 0.60);
            pb.line_to(cx + s * 0.10, cy + s);
            pb.line_to(cx - s * 0.10, cy + s);
            pb.close();
        }
        Glyph::Raven => {
            // Perched and facing right: a body, a head with a beak, a tail
            // wedge out the back and one wing laid over the body.
            pb.move_to(cx - s * 0.80, cy + s * 0.10);
            pb.cubic_to(cx - s * 0.55, cy - s * 0.35, cx - s * 0.05, cy - s * 0.50, cx + s * 0.30, cy - s * 0.30);
            pb.cubic_to(cx + s * 0.52, cy - s * 0.18, cx + s * 0.56, cy + s * 0.10, cx + s * 0.40, cy + s * 0.34);
            pb.cubic_to(cx + s * 0.18, cy + s * 0.56, cx - s * 0.35, cy + s * 0.56, cx - s * 0.80, cy + s * 0.10);
            pb.close();
            // The tail, out behind.
            pb.move_to(cx - s * 0.70, cy - s * 0.02);
            pb.line_to(cx - s * 1.05, cy + s * 0.46);
            pb.line_to(cx - s * 0.58, cy + s * 0.40);
            pb.close();
            if let Some(head) = PathBuilder::from_circle(cx + s * 0.44, cy - s * 0.46, s * 0.27) {
                extra.push((head, item.colour, false));
            }
            let mut beak = PathBuilder::new();
            beak.move_to(cx + s * 0.64, cy - s * 0.58);
            beak.line_to(cx + s * 1.02, cy - s * 0.44);
            beak.line_to(cx + s * 0.64, cy - s * 0.30);
            beak.close();
            if let Some(path) = beak.finish() {
                extra.push((path, item.colour, false));
            }
            let mut wing = PathBuilder::new();
            wing.move_to(cx - s * 0.40, cy - s * 0.10);
            wing.quad_to(cx - s * 0.02, cy + s * 0.44, cx + s * 0.22, cy - s * 0.06);
            wing.quad_to(cx - s * 0.08, cy - s * 0.02, cx - s * 0.40, cy - s * 0.10);
            wing.close();
            if let Some(path) = wing.finish() {
                extra.push((path, dim(item.colour, 0.72), false));
            }
        }
        Glyph::Brazier => {
            // A bowl on a stand; a lit one carries a flame.
            pb.move_to(cx - s * 0.52, cy + s * 0.04);
            pb.line_to(cx + s * 0.52, cy + s * 0.04);
            pb.line_to(cx + s * 0.30, cy + s * 0.46);
            pb.line_to(cx - s * 0.30, cy + s * 0.46);
            pb.close();
            pb.move_to(cx - s * 0.10, cy + s * 0.46);
            pb.line_to(cx + s * 0.10, cy + s * 0.46);
            pb.line_to(cx + s * 0.10, cy + s * 0.84);
            pb.line_to(cx - s * 0.10, cy + s * 0.84);
            pb.close();
            pb.move_to(cx - s * 0.40, cy + s * 0.84);
            pb.line_to(cx + s * 0.40, cy + s * 0.84);
            pb.line_to(cx + s * 0.40, cy + s);
            pb.line_to(cx - s * 0.40, cy + s);
            pb.close();
            if item.lit {
                let mut flame = PathBuilder::new();
                flame.move_to(cx, cy - s);
                flame.cubic_to(cx + s * 0.40, cy - s * 0.56, cx + s * 0.30, cy - s * 0.08, cx, cy - s * 0.02);
                flame.cubic_to(cx - s * 0.30, cy - s * 0.08, cx - s * 0.40, cy - s * 0.56, cx, cy - s);
                flame.close();
                if let Some(path) = flame.finish() {
                    extra.push((path, [232, 126, 40], true));
                }
            }
        }
        Glyph::Crown => {
            // A band along the bottom with five points rising off it, so the
            // dips never cut below the band and it reads as a crown.
            let x = |f: f32| cx + f * s * 0.86;
            let y = |f: f32| cy + f * s;
            pb.move_to(x(-1.0), y(0.62));
            pb.line_to(x(1.0), y(0.62));
            pb.line_to(x(1.0), y(0.16));
            for (px, dip) in [(1.0f32, 0.645f32), (0.43, 0.215), (0.0, -0.215), (-0.43, -0.645), (-1.0, -1.0)] {
                let peak = if px.abs() > 0.9 { -0.40 } else if px == 0.0 { -0.86 } else { -0.74 };
                pb.line_to(x(px), y(peak));
                if dip > -1.0 {
                    pb.line_to(x(dip), y(0.06));
                }
            }
            pb.line_to(x(-1.0), y(0.16));
            pb.close();
            for jewel in [-0.5f32, 0.0, 0.5] {
                if let Some(path) = PathBuilder::from_circle(x(jewel), y(0.40), s * 0.10) {
                    extra.push((path, dim(item.colour, 0.55), false));
                }
            }
        }
        Glyph::Shield => {
            pb.move_to(cx - s * 0.72, cy - s * 0.78);
            pb.line_to(cx + s * 0.72, cy - s * 0.78);
            pb.line_to(cx + s * 0.72, cy + s * 0.12);
            pb.cubic_to(cx + s * 0.70, cy + s * 0.64, cx + s * 0.30, cy + s * 0.90, cx, cy + s);
            pb.cubic_to(cx - s * 0.30, cy + s * 0.90, cx - s * 0.70, cy + s * 0.64, cx - s * 0.72, cy + s * 0.12);
            pb.close();
        }
        Glyph::Dragon => {
            // Facing right: a long tail out to the left, a chest, a neck and a
            // snout. Which way it faces is the whole point of one puzzle, so
            // the silhouette is as lopsided as it can be.
            pb.move_to(cx - s * 1.00, cy + s * 0.70);
            pb.cubic_to(cx - s * 0.60, cy + s * 0.62, cx - s * 0.30, cy + s * 0.46, cx - s * 0.05, cy + s * 0.22);
            pb.cubic_to(cx + s * 0.18, cy, cx + s * 0.26, cy - s * 0.22, cx + s * 0.48, cy - s * 0.38);
            pb.cubic_to(cx + s * 0.62, cy - s * 0.50, cx + s * 0.74, cy - s * 0.56, cx + s * 0.86, cy - s * 0.52);
            pb.line_to(cx + s * 1.02, cy - s * 0.64);
            pb.line_to(cx + s * 1.02, cy - s * 0.32);
            pb.cubic_to(cx + s * 0.80, cy - s * 0.24, cx + s * 0.72, cy - s * 0.08, cx + s * 0.60, cy + s * 0.16);
            pb.cubic_to(cx + s * 0.44, cy + s * 0.46, cx + s * 0.10, cy + s * 0.66, cx - s * 0.40, cy + s * 0.80);
            pb.cubic_to(cx - s * 0.66, cy + s * 0.86, cx - s * 0.88, cy + s * 0.80, cx - s * 1.00, cy + s * 0.70);
            pb.close();
            // One wing, up off the back, in its own shade so the neck and the
            // snout are not swallowed by it.
            let mut wing = PathBuilder::new();
            wing.move_to(cx - s * 0.05, cy + s * 0.14);
            wing.line_to(cx - s * 0.46, cy - s * 0.86);
            wing.cubic_to(cx - s * 0.10, cy - s * 0.70, cx + s * 0.14, cy - s * 0.50, cx + s * 0.26, cy - s * 0.30);
            wing.close();
            if let Some(path) = wing.finish() {
                extra.push((path, dim(item.colour, 0.66), false));
            }
            // Spines standing off the back, so it is a dragon and not a long
            // bird. They sit on the line the back actually runs along, which
            // climbs from the tail to the neck.
            let mut spines = PathBuilder::new();
            for (at, back) in [(-0.66f32, 0.53f32), (-0.41, 0.40), (-0.16, 0.27)] {
                spines.move_to(cx + (at - 0.09) * s, cy + back * s);
                spines.line_to(cx + at * s, cy + (back - 0.30) * s);
                spines.line_to(cx + (at + 0.09) * s, cy + (back - 0.06) * s);
                spines.close();
            }
            if let Some(path) = spines.finish() {
                extra.push((path, lift(item.colour, 0.3), false));
            }
            // An eye, so the head is plainly a head and plainly pointing.
            if let Some(eye) = PathBuilder::from_circle(cx + s * 0.80, cy - s * 0.48, s * 0.07) {
                extra.push((eye, lift(item.colour, 0.85), false));
            }
        }
        Glyph::Banner => {
            pb.move_to(cx - s * 0.56, cy - s * 0.86);
            pb.line_to(cx + s * 0.56, cy - s * 0.86);
            pb.line_to(cx + s * 0.56, cy + s * 0.50);
            pb.line_to(cx, cy + s);
            pb.line_to(cx - s * 0.56, cy + s * 0.50);
            pb.close();
            let mut pole = PathBuilder::new();
            pole.move_to(cx - s * 0.78, cy - s * 0.94);
            pole.line_to(cx + s * 0.78, cy - s * 0.94);
            pole.line_to(cx + s * 0.78, cy - s * 0.80);
            pole.line_to(cx - s * 0.78, cy - s * 0.80);
            pole.close();
            if let Some(path) = pole.finish() {
                extra.push((path, [96, 102, 114], false));
            }
        }
        Glyph::Sigil => {
            // A disc with two or three heads looking out over the top of it.
            if let Some(disc) = PathBuilder::from_circle(cx, cy + s * 0.22, s * 0.60) {
                extra.push((disc, item.colour, false));
            }
            let heads = item.heads.clamp(2, 3);
            let spread = if heads == 3 { [-0.62f32, 0.0, 0.62] } else { [-0.34, 0.34, 0.0] };
            for k in 0..heads as usize {
                let hx = cx + spread[k] * s * 0.78;
                let hy = cy - s * 0.52 + (spread[k].abs() * s * 0.20);
                if let Some(head) = PathBuilder::from_circle(hx, hy, s * 0.24) {
                    extra.push((head, item.colour, false));
                }
                let mut snout = PathBuilder::new();
                snout.move_to(hx - s * 0.08, hy - s * 0.22);
                snout.line_to(hx + s * 0.30, hy - s * 0.34);
                snout.line_to(hx + s * 0.06, hy - s * 0.02);
                snout.close();
                if let Some(path) = snout.finish() {
                    extra.push((path, item.colour, false));
                }
            }
        }
        Glyph::Eye => {
            pb.move_to(cx - s * 0.92, cy);
            pb.quad_to(cx, cy - s * 0.70, cx + s * 0.92, cy);
            pb.quad_to(cx, cy + s * 0.70, cx - s * 0.92, cy);
            pb.close();
            let mut slit = PathBuilder::new();
            slit.move_to(cx, cy - s * 0.42);
            slit.quad_to(cx + s * 0.16, cy, cx, cy + s * 0.42);
            slit.quad_to(cx - s * 0.16, cy, cx, cy - s * 0.42);
            slit.close();
            if let Some(path) = slit.finish() {
                extra.push((path, INK_DARK, false));
            }
        }
    }
    let outline = Stroke { width: (side * 0.045).clamp(1.5, 3.0), line_join: LineJoin::Round, ..Stroke::default() };
    if let Some(path) = pb.finish() {
        pen.px.fill_path(&path, &paint(item.colour, 255), FillRule::Winding, turn, None);
        pen.px.stroke_path(&path, &paint(INK_DARK, 215), &outline, turn, None);
    }
    for (path, colour, glowing) in extra {
        pen.px.fill_path(&path, &paint(colour, 255), FillRule::Winding, turn, None);
        pen.px.stroke_path(&path, &paint(INK_DARK, 215), &outline, turn, None);
        if glowing {
            let bounds = path.bounds();
            glow(&mut pen.px, bounds.x() + bounds.width() / 2.0, bounds.y() + bounds.height() / 2.0, side * 0.5, [255, 160, 60], 70);
        }
    }
}

/// A number scorched into a shield, big enough to read at a glance.
fn scorched(pen: &mut Pen<'_>, cx: f32, cy: f32, height: f32, digits: &str) {
    let s = height / 2.0;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - s * 0.80, cy - s * 0.92);
    pb.line_to(cx + s * 0.80, cy - s * 0.92);
    pb.line_to(cx + s * 0.80, cy + s * 0.10);
    pb.cubic_to(cx + s * 0.78, cy + s * 0.66, cx + s * 0.32, cy + s * 0.92, cx, cy + s);
    pb.cubic_to(cx - s * 0.32, cy + s * 0.92, cx - s * 0.78, cy + s * 0.66, cx - s * 0.80, cy + s * 0.10);
    pb.close();
    if let Some(path) = pb.finish() {
        let shader = down(cy - s, cy + s, &[(0.0, [122, 106, 84]), (0.5, [94, 80, 62]), (1.0, [62, 52, 40])]);
        fill_shaded(&mut pen.px, &path, shader, [94, 80, 62]);
        let stroke = Stroke { width: 4.0, ..Stroke::default() };
        pen.px.stroke_path(&path, &paint([40, 33, 25], 235), &stroke, Transform::identity(), None);
    }
    let size = (height * 0.44).min(128.0);
    glow(&mut pen.px, cx, cy, height * 0.42, [255, 150, 50], 110);
    for (dx, dy) in [(-2.0f32, 0.0f32), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
        pen.centered(digits, cx + dx, cy + size * 0.34 + dy, size, Weight::EXTRA_BOLD, [28, 20, 14]);
    }
    pen.centered(digits, cx, cy + size * 0.34, size, Weight::EXTRA_BOLD, [255, 206, 140]);
}

/// Two hosts drawn as blocks of figures, with the divide between them.
fn hosts(pen: &mut Pen<'_>, area: (f32, f32, f32, f32), left: usize, right: usize) {
    let (x0, top, x1, bottom) = area;
    let mid = (x0 + x1) / 2.0;
    let mut line = PathBuilder::new();
    line.move_to(mid, top - 4.0);
    line.line_to(mid, bottom + 4.0);
    if let Some(path) = line.finish() {
        let stroke = Stroke { width: 3.0, ..Stroke::default() };
        pen.px.stroke_path(&path, &paint(VELLUM_EDGE, 200), &stroke, Transform::identity(), None);
    }
    for (side, n) in [(0usize, left), (1, right)] {
        let (lo, hi) = if side == 0 { (x0, mid - 26.0) } else { (mid + 26.0, x1) };
        let per_row = n.min(4).max(1);
        let rows = n.div_ceil(per_row);
        let step = ((hi - lo) / per_row as f32).min(72.0);
        let r = (step * 0.20).min((bottom - top) / (rows as f32 * 3.4));
        for i in 0..n {
            let (col, row) = (i % per_row, i / per_row);
            let cx = (lo + hi) / 2.0 + (col as f32 - (per_row as f32 - 1.0) / 2.0) * step;
            let cy = (top + bottom) / 2.0 + (row as f32 - (rows as f32 - 1.0) / 2.0) * r * 3.4;
            fill_circle(&mut pen.px, cx, cy - r * 1.4, r * 0.82, INK_DARK);
            let body = Rect::from_xywh(cx - r * 1.4, cy - r * 0.5, r * 2.8, r * 2.6).and_then(PathBuilder::from_oval);
            if let Some(path) = body {
                pen.px.fill_path(&path, &paint(INK_DARK, 255), FillRule::Winding, Transform::identity(), None);
            }
        }
        let label = if side == 0 { "LEFT" } else { "RIGHT" };
        pen.centered(&spaced(label), (lo + hi) / 2.0, bottom + 2.0, 14.0, Weight::EXTRA_BOLD, [96, 82, 60]);
    }
}

// --- the egg week -----------------------------------------------------------

/// A dragon egg, drawn to fit a circle of diameter `d`. `stage` runs 1..=5 and
/// decides nothing but how warm and how broken it is: 1 is cold stone, 2 has a
/// light inside it, 3 is finely cracked, 4 is cracked to the molten, and 5 is
/// splitting open. A beaten fighter's egg goes cold whatever its stage.
///
/// ART SEAM: `egg-1.png` … `egg-5.png` would go through [`egg_art`]; nothing is
/// shipped yet, so every egg is painted.
pub(super) fn egg(pen: &mut Pen<'_>, cx: f32, cy: f32, d: f32, stage: u8, cold: bool) {
    let stage = stage.clamp(1, EGG_STAGES);
    let heat = if cold { 0 } else { stage };
    // The painting, when the month's art has been copied in. It is a square
    // picture with its own transparency, so it drops straight into the ring.
    let side = d.round().max(8.0) as u32;
    if let Some(art) = battle_art::round(Art::Egg(stage), side) {
        let corner = |centre: f32| (centre - side as f32 / 2.0).round() as i32;
        let how = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
        if !cold {
            // Warmer the further through the week it is: the crack is the
            // calendar's, the fire behind it is the card's.
            glow(&mut pen.px, cx, cy, d * 0.46, EMBER, 30 + stage * 14);
        }
        pen.px.draw_pixmap(corner(cx), corner(cy), art.as_ref().as_ref(), &how, Transform::identity(), None);
        if cold {
            wash(&mut pen.px, cx, cy, d / 2.0, [8, 9, 12], 120);
        }
        return;
    }
    let (hw, hh) = (d * 0.32, d * 0.41);
    // The shell: narrower and rounded at the crown, full and round at the foot.
    let mut pb = PathBuilder::new();
    pb.move_to(cx, cy - hh);
    pb.cubic_to(cx + hw * 0.52, cy - hh * 0.98, cx + hw * 0.96, cy - hh * 0.30, cx + hw, cy + hh * 0.26);
    pb.cubic_to(cx + hw, cy + hh * 0.80, cx + hw * 0.62, cy + hh, cx, cy + hh);
    pb.cubic_to(cx - hw * 0.62, cy + hh, cx - hw, cy + hh * 0.80, cx - hw, cy + hh * 0.26);
    pb.cubic_to(cx - hw * 0.96, cy - hh * 0.30, cx - hw * 0.52, cy - hh * 0.98, cx, cy - hh);
    pb.close();
    let Some(shell) = pb.finish() else { return };
    // Scales and cracks need a mask the size of the whole card, which is fine
    // once on a fight card and ruinous thirty-two times on a bracket. An egg
    // drawn this small has no room for either, so it is drawn plain.
    let detail = d >= 70.0;
    // A light inside, growing with the stage, thrown before the shell so it
    // reads as coming through it rather than sitting on top.
    if heat >= 2 {
        let lit = 40 + heat as u8 * 26;
        glow(&mut pen.px, cx, cy + hh * 0.2, d * 0.52, EMBER, lit);
    }
    let body: &[(f32, [u8; 3])] = match heat {
        0 => &[(0.0, [62, 65, 73]), (0.55, [38, 40, 46]), (1.0, [20, 21, 25])],
        1 => &[(0.0, [68, 62, 62]), (0.55, [42, 38, 40]), (1.0, [22, 20, 22])],
        2 => &[(0.0, [84, 64, 52]), (0.5, [50, 38, 36]), (1.0, [26, 22, 23])],
        _ => &[(0.0, [96, 68, 48]), (0.5, [54, 38, 34]), (1.0, SOOT)],
    };
    let shader = down(cy - hh, cy + hh, body);
    fill_shaded(&mut pen.px, &shell, shader, SOOT);
    if !detail {
        // Small: the shell, its edge, and a hint of fire if there is any.
        if heat >= 3 {
            glow(&mut pen.px, cx, cy, d * 0.34, EMBER, 60 + heat as u8 * 24);
        }
        let stroke = Stroke { width: (d * 0.03).max(1.2), ..Stroke::default() };
        let edge = if heat >= 3 { dim(OLD_GOLD, 0.9) } else { [104, 110, 122] };
        pen.px.stroke_path(&shell, &paint(edge, 235), &stroke, Transform::identity(), None);
        return;
    }
    let Some(mut mask) = Mask::new(pen.px.width(), pen.px.height()) else { return };
    mask.fill_path(&shell, FillRule::Winding, true, Transform::identity());
    // Scales: overlapping arcs in rows down the shell.
    let mut scales = PathBuilder::new();
    let rows = 9;
    for row in 0..rows {
        let t = row as f32 / (rows - 1) as f32;
        let y = cy - hh * 0.86 + t * hh * 1.7;
        let span = hw * (0.45 + 0.95 * (t * std::f32::consts::PI).sin());
        let step = d * 0.085;
        let shift = if row % 2 == 0 { 0.0 } else { step / 2.0 };
        let mut x = cx - span + shift;
        while x <= cx + span {
            scales.move_to(x - step * 0.5, y);
            scales.quad_to(x, y + step * 0.62, x + step * 0.5, y);
            x += step;
        }
    }
    if let Some(path) = scales.finish() {
        let stroke = Stroke { width: 2.0, line_cap: LineCap::Round, ..Stroke::default() };
        let ink = if heat >= 3 { lift(OLD_GOLD, 0.1) } else { [128, 132, 142] };
        let alpha = if heat >= 3 { 110 } else { 95 };
        pen.px.stroke_path(&path, &paint(ink, alpha), &stroke, Transform::identity(), Some(&mask));
    }
    // Cracks, from stage three on: more of them, brighter, and at stage five
    // one of them opens all the way down.
    if heat >= 3 {
        let veins: &[&[(f32, f32)]] = &[
            &[(0.0, -0.78), (0.14, -0.42), (-0.06, -0.06), (0.2, 0.3), (0.06, 0.72)],
            &[(-0.52, -0.2), (-0.26, -0.02), (-0.34, 0.26), (-0.1, 0.52)],
            &[(0.5, 0.0), (0.26, 0.2), (0.4, 0.46), (0.18, 0.74)],
            &[(-0.3, -0.56), (-0.04, -0.4), (-0.18, -0.2)],
        ];
        let how_many = match heat {
            3 => 2,
            4 => 3,
            _ => 4,
        };
        let mut pb = PathBuilder::new();
        for vein in veins.iter().take(how_many) {
            for (i, (fx, fy)) in vein.iter().enumerate() {
                let (x, y) = (cx + fx * hw, cy + fy * hh);
                if i == 0 {
                    pb.move_to(x, y);
                } else {
                    pb.line_to(x, y);
                }
            }
        }
        if let Some(path) = pb.finish() {
            let wide = match heat {
                3 => 3.0,
                4 => 5.0,
                _ => 7.0,
            };
            for (width, c, alpha) in [
                (wide * 2.6, EMBER, 60),
                (wide, EMBER, 190),
                (wide * 0.42, [255, 236, 196], 240),
            ] {
                let stroke =
                    Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
                pen.px.stroke_path(&path, &paint(c, alpha), &stroke, Transform::identity(), Some(&mask));
            }
        }
    }
    // Stage five: the shell is coming apart, so the light gets out.
    if heat >= EGG_STAGES {
        glow(&mut pen.px, cx, cy, d * 0.34, [255, 214, 140], 150);
        for (dx, dy, len) in [(-0.1f32, -0.8f32, 0.5f32), (0.26, -0.5, 0.42)] {
            let mut pb = PathBuilder::new();
            pb.move_to(cx + dx * hw, cy + dy * hh);
            pb.line_to(cx + (dx + 0.18) * hw, cy + (dy + len) * hh);
            if let Some(path) = pb.finish() {
                let stroke = Stroke { width: 9.0, line_cap: LineCap::Round, ..Stroke::default() };
                pen.px.stroke_path(&path, &paint([255, 246, 220], 230), &stroke, Transform::identity(), Some(&mask));
            }
        }
    }
    // The shell's own edge, and a highlight off its shoulder.
    let stroke = Stroke { width: 3.0, ..Stroke::default() };
    let edge = if heat >= 3 { dim(OLD_GOLD, 0.9) } else { [104, 110, 122] };
    pen.px.stroke_path(&shell, &paint(edge, 235), &stroke, Transform::identity(), None);
    wash(&mut pen.px, cx - hw * 0.36, cy - hh * 0.46, d * 0.07, [255, 255, 255], 44);
}

/// A painted egg for a stage, if one has been supplied. Nothing is shipped yet,
/// so every egg is drawn; put `eggs/egg-<stage>.png` beside the crests and add
/// it to this table and it is used instead.
#[allow(dead_code)]
fn egg_art(stage: u8) -> Option<&'static [u8]> {
    const EGGS: [(u8, &[u8]); 0] = [];
    EGGS.iter().find(|(s, _)| *s == stage).map(|(_, bytes)| *bytes)
}

// --- house marks ------------------------------------------------------------

/// A house's mark: its painted crest in a dark disc with a keyline in the
/// house's second colour. Without the painting - a checkout or a server that
/// has not been given the art - it is the house's initial instead, which is
/// what this drew before the paintings existed. A beaten fighter's mark is
/// dimmed with them.
pub(super) fn house_mark(pen: &mut Pen<'_>, house: &HouseLook, cx: f32, cy: f32, r: f32, dimmed: bool) {
    wash(&mut pen.px, cx, cy + 4.0, r + 6.0, [0, 0, 0], 110);
    fill_circle(&mut pen.px, cx, cy, r, [16, 16, 21]);
    let field = if dimmed { [48, 50, 58] } else { house.colours.0 };
    fill_circle(&mut pen.px, cx, cy, r - 5.0, field);
    wash(&mut pen.px, cx, cy - r * 0.3, r * 0.74, [255, 255, 255], 16);
    if let Some(edge) = PathBuilder::from_circle(cx, cy, r - 1.25) {
        let keyline = if dimmed { [86, 91, 104] } else { lift(house.colours.1, 0.55) };
        let stroke = Stroke { width: 2.5, ..Stroke::default() };
        pen.px.stroke_path(&edge, &paint(keyline, 235), &stroke, Transform::identity(), None);
    }
    // The painted crest, sized to sit inside the keyline.
    let side = (r * 1.72).round().max(8.0) as u32;
    if let Some(art) = (!house.art.is_empty()).then(|| battle_art::sized(Art::Crest(house.art), side, side)).flatten() {
        let corner = |centre: f32| (centre - side as f32 / 2.0).round() as i32;
        let how = PixmapPaint { quality: FilterQuality::Bicubic, opacity: if dimmed { 0.45 } else { 1.0 }, ..PixmapPaint::default() };
        pen.px.draw_pixmap(corner(cx), corner(cy), art.as_ref().as_ref(), &how, Transform::identity(), None);
        if dimmed {
            wash(&mut pen.px, cx, cy, r - 5.0, [8, 9, 12], 90);
        }
        return;
    }
    let letter: String = house.initial.chars().take(2).collect::<String>().to_uppercase();
    let size = r * 0.95;
    let ink = if dimmed { [148, 154, 166] } else { lift(house.colours.1, 0.45) };
    pen.centered(&letter, cx, cy + size * 0.36, size, Weight::EXTRA_BOLD, ink);
}

/// A ribbon with two swallow-tailed ends, carrying the champion's title.
fn ribbon(pen: &mut Pen<'_>, label: &str) {
    let label = spaced(label);
    let w = (pen.measure(&label, 21.0, Weight::EXTRA_BOLD) + 52.0).clamp(420.0, 600.0);
    let (x, y, h) = (CHAMP_W / 2.0 - w / 2.0, 370.0, 54.0);
    for &side in &[-1.0f32, 1.0] {
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
pub(super) fn crown(px: &mut Pixmap, cx: f32, base: f32, w: f32, h: f32) {
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

/// Specks thrown over the card.
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

/// The floor: a flat fill, or a top-to-bottom wash through the stops.
pub(super) fn floor(px: &mut Pixmap, w: f32, h: f32, stops: &[(f32, [u8; 3])]) {
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

/// Whether a point falls where the stage chip sits, top centre, so specks of
/// scenery stay out of its letters.
fn under_chip(x: f32, y: f32) -> bool {
    y < 62.0 && (x - FIGHT_W / 2.0).abs() < 250.0
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
pub(super) fn vignette(px: &mut Pixmap, w: f32, h: f32, strength: u8) {
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

pub(super) fn sk(c: [u8; 3], a: u8) -> SkColor {
    SkColor::from_rgba8(c[0], c[1], c[2], a)
}

/// A top-to-bottom gradient between `y0` and `y1`.
pub(super) fn down(y0: f32, y1: f32, stops: &[(f32, [u8; 3])]) -> Option<Shader<'static>> {
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
pub(super) fn fill_shaded(px: &mut Pixmap, path: &tiny_skia::Path, shader: Option<Shader<'_>>, c: [u8; 3]) {
    let mut p = paint(c, 255);
    if let Some(shader) = shader {
        p.shader = shader;
    }
    px.fill_path(path, &p, FillRule::Winding, Transform::identity(), None);
}

/// A soft pool of colour, fading out to nothing at radius `r`.
pub(super) fn glow(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3], alpha: u8) {
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

/// A translucent disc: specks of snow, and the wash over whoever lost.
pub(super) fn wash(px: &mut Pixmap, cx: f32, cy: f32, r: f32, c: [u8; 3], alpha: u8) {
    if let Some(path) = PathBuilder::from_circle(cx, cy, r) {
        px.fill_path(&path, &paint(c, alpha), FillRule::Winding, Transform::identity(), None);
    }
}

/// The same hue pulled `t` of the way towards white.
pub(super) fn lift(c: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 + (255.0 - c[i] as f32) * t) as u8)
}

/// The same hue at `t` of its brightness.
pub(super) fn dim(c: [u8; 3], t: f32) -> [u8; 3] {
    [0, 1, 2].map(|i| (c[i] as f32 * t) as u8)
}

/// Letters opened up, the way a small caps chip wants them.
pub(super) fn spaced(s: &str) -> String {
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

pub(super) struct Pen<'a> {
    pub(super) px: Pixmap,
    fs: &'a mut FontSystem,
    cache: SwashCache,
    family: String,
    /// Every face `family` has, to snap a wanted weight onto a real one.
    faces: Vec<(FontStyle, Stretch, Weight)>,
}

impl<'a> Pen<'a> {
    pub(super) fn new(w: f32, h: f32, fs: &'a mut FontSystem) -> Option<Pen<'a>> {
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

    pub(super) fn layout(&mut self, text: &str, size: f32, line_h: f32, weight: Weight, width: Option<f32>) -> Buffer {
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

    pub(super) fn draw(&mut self, buf: &Buffer, x: f32, top: f32, color: [u8; 3]) {
        let (fs, cache, px) = (&mut *self.fs, &mut self.cache, &mut self.px);
        let (ox, oy) = (x.round() as i32, top.round() as i32);
        buf.draw(fs, cache, Color::rgb(color[0], color[1], color[2]), |gx, gy, w, h, c| {
            blend_rect(px, ox + gx, oy + gy, w, h, c);
        });
    }

    pub(super) fn measure(&mut self, text: &str, size: f32, weight: Weight) -> f32 {
        let buf = self.layout(text, size, size * 1.3, weight, None);
        buf.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// One line centred on `cx`, with its baseline at `baseline`.
    pub(super) fn centered(&mut self, text: &str, cx: f32, baseline: f32, size: f32, weight: Weight, color: [u8; 3]) {
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

    pub(super) fn fit(&mut self, text: &str, size: f32, weight: Weight, max_w: f32) -> String {
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
    ///
    /// The longest prefix that fits is found by halving rather than by dropping
    /// one character at a time. Shaping a long line is slow, and the old loop
    /// shaped it once per character dropped - two hundred shapings for a line
    /// somebody pasted, with the font lock held the whole way. This is at most
    /// a dozen, and it cannot spin: the empty prefix always fits.
    fn paragraph(&mut self, text: &str, size: f32, line_h: f32, width: f32, max_lines: usize) -> Buffer {
        // Nobody reads past this, and it bounds the work below.
        let all: Vec<char> = text.chars().take(240).collect();
        let whole: String = all.iter().collect();
        let buf = self.layout(&whole, size, line_h, Weight::MEDIUM, Some(width));
        if buf.layout_runs().count() <= max_lines {
            return buf;
        }
        // `low` always fits and `high` never does, so the answer is between.
        let (mut low, mut high) = (0usize, all.len());
        while low + 1 < high {
            let mid = (low + high) / 2;
            let body = clipped(&all, mid);
            let buf = self.layout(&body, size, line_h, Weight::MEDIUM, Some(width));
            if buf.layout_runs().count() <= max_lines {
                low = mid;
            } else {
                high = mid;
            }
        }
        let body = clipped(&all, low);
        self.layout(&body, size, line_h, Weight::MEDIUM, Some(width))
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

/// The first `n` characters, with an ellipsis after them.
fn clipped(all: &[char], n: usize) -> String {
    format!("{}\u{2026}", all[..n.min(all.len())].iter().collect::<String>().trim_end())
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
    pub(super) fn fake_avatar(tint: [u8; 3]) -> Vec<u8> {
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

    /// The month's paint for a house, as `battle.rs` hands it over.
    pub(super) fn look_of(key: &'static str, name: &str, initial: &str, c: ([u8; 3], [u8; 3])) -> HouseLook {
        // The art slug the real `house_look` derives, so a preview wears the
        // right crest rather than everybody wearing the first one.
        let art = match key {
            "gryffindor" => "stark",
            "slytherin" => "lannister",
            "ravenclaw" => "targaryen",
            _ => "watch",
        };
        HouseLook {
            key,
            art,
            name: name.to_string(),
            crest: "\u{1f6e1}\u{fe0f}".to_string(),
            initial: initial.to_string(),
            colours: c,
        }
    }

    fn stark() -> HouseLook {
        look_of("gryffindor", "Stark", "S", ([110, 123, 139], [226, 232, 240]))
    }

    fn targaryen() -> HouseLook {
        look_of("ravenclaw", "Targaryen", "T", ([44, 38, 44], [168, 34, 38]))
    }

    fn cast() -> (Fighter, Fighter) {
        (
            Fighter {
                name: "Rohit 🔥".to_string(),
                avatar: Some(fake_avatar([214, 96, 92])),
                hp: 68,
                max_hp: 100,
                house: Some(stark()),
                stage: 4,
            },
            Fighter {
                name: "Meera".to_string(),
                avatar: Some(fake_avatar([112, 104, 220])),
                hp: 41,
                max_hp: 100,
                house: Some(targaryen()),
                stage: 2,
            },
        )
    }

    #[test]
    fn fight_card_renders_open_and_decided() {
        let (a, b) = cast();
        for (outcome, hit) in [(Outcome::Open, Some((1, -27))), (Outcome::Won(1), None)] {
            let fight = Fight {
                stage: "The Last Eight".to_string(),
                left: &a,
                right: &b,
                line: "Gaali nahi, bas ek dhaal".to_string(),
                outcome,
                hit,
                season: Season::Houses,
            };
            let png = fight_png(&fight).expect("fight card");
            assert_eq!(&png[..4], &PNG_MAGIC);
        }
    }

    /// The awkward cases in one go: no picture, no health left, a heal, a
    /// miss, and a `hit` naming a side that does not exist.
    #[test]
    fn cards_survive_the_awkward_cases() {
        let a = Fighter { name: "Koi nahi".to_string(), avatar: None, hp: 0, max_hp: 0, house: None, stage: 0 };
        let picture = Some(fake_avatar([90, 190, 160]));
        let b = Fighter {
            name: "ज़ैद".to_string(),
            avatar: picture,
            hp: 3,
            max_hp: 100,
            house: Some(look_of("hufflepuff", "Night's Watch", "W", ([46, 51, 60], [138, 190, 222]))),
            stage: 9,
        };
        for season in [Season::Eggs, Season::Houses] {
            for hit in [None, Some((7, -5))] {
                let fight = Fight {
                    stage: "Challenge".to_string(),
                    left: &a,
                    right: &b,
                    line: String::new(),
                    outcome: Outcome::Open,
                    hit,
                    season,
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
                season,
            };
            assert_eq!(&fight_png(&fight).expect("fight card")[..4], &PNG_MAGIC);
            let champ = Champion { who: &a, subtitle: "12 in the lists".to_string(), line: String::new(), season };
            assert_eq!(&champion_png(&champ).expect("champion card")[..4], &PNG_MAGIC);
        }
    }

    #[test]
    fn champion_card_renders() {
        let (a, _) = cast();
        let champ = Champion {
            who: &a,
            subtitle: "12 in the lists · 4 rounds · House Stark".to_string(),
            line: "Rohit ne Meera ko reth mein gira diya, aur taaj utha liya".to_string(),
            season: Season::Houses,
        };
        let png = champion_png(&champ).expect("champion card");
        assert_eq!(&png[..4], &PNG_MAGIC);
    }

    #[test]
    fn both_seasons_render_every_card() {
        let (a, b) = cast();
        for season in [Season::Eggs, Season::Houses] {
            let fight = Fight {
                stage: "The Final Tilt".to_string(),
                left: &a,
                right: &b,
                line: "Line".to_string(),
                outcome: Outcome::Won(0),
                hit: None,
                season,
            };
            assert_eq!(&fight_png(&fight).expect("fight card")[..4], &PNG_MAGIC);
            let champ = Champion { who: &b, subtitle: String::new(), line: String::new(), season };
            assert_eq!(&champion_png(&champ).expect("champion card")[..4], &PNG_MAGIC);
        }
    }

    /// Every egg stage draws, including the ones outside the range.
    #[test]
    fn every_egg_stage_draws() {
        let (a, b) = cast();
        for stage in 0..=EGG_STAGES + 2 {
            let who = Fighter { name: "Egg".into(), avatar: None, hp: 50, max_hp: 100, house: None, stage };
            let fight = Fight {
                stage: "The Melee".to_string(),
                left: &who,
                right: &a,
                line: String::new(),
                outcome: Outcome::Open,
                hit: None,
                season: Season::Eggs,
            };
            assert_eq!(&fight_png(&fight).expect("fight card")[..4], &PNG_MAGIC);
        }
        let champ = Champion { who: &b, subtitle: String::new(), line: String::new(), season: Season::Eggs };
        assert_eq!(&champion_png(&champ).expect("champion card")[..4], &PNG_MAGIC);
    }

    #[test]
    fn the_stage_chip_says_the_melee_or_the_duel() {
        assert_eq!(stage_text("The Last Eight"), "THE MELEE · The Last Eight");
        assert_eq!(stage_text("The Final Tilt"), "THE MELEE · The Final Tilt");
        // A duel has one fight, so repeating "challenge" would say nothing.
        assert_eq!(stage_text("Challenge"), DUEL_CHIP);
        assert_eq!(stage_text(""), DUEL_CHIP);
    }

    /// Nothing the card writes may name a house before the hatch, and after it
    /// the winner's house is named on both the fight card and the champion's.
    #[test]
    fn houses_are_named_only_after_the_hatch() {
        let (a, _) = cast();
        let nobody = Fighter { name: "Nobody".into(), avatar: None, hp: 9, max_hp: 100, house: None, stage: 3 };
        // The egg week: the fighter carries a house and it is still not said.
        assert_eq!(winner_pill(&a, Season::Eggs), "WINNER");
        assert_eq!(champion_title(&a, Season::Eggs), EGG_TITLE);
        for text in [winner_pill(&a, Season::Eggs), champion_title(&a, Season::Eggs)] {
            assert!(!text.to_lowercase().contains("stark"), "the egg week named a house: {}", text);
            assert!(!text.to_lowercase().contains("house"), "the egg week named a house: {}", text);
        }
        // After the hatch, the house is named in both places.
        assert_eq!(winner_pill(&a, Season::Houses), "WINNER · HOUSE STARK");
        assert_eq!(champion_title(&a, Season::Houses), "STARK CHAMPION");
        // Somebody with no house stays neutral in either season rather than
        // being handed one.
        assert_eq!(winner_pill(&nobody, Season::Houses), "WINNER");
        assert_eq!(champion_title(&nobody, Season::Houses), "CHAMPION OF THE LISTS");
        assert_eq!(cloth(&nobody, Season::Houses), NEUTRAL);
        assert_eq!(cloth(&nobody, Season::Eggs), (SOOT, OLD_GOLD));
        // And in the egg week even a sorted fighter's banner is the egg week's.
        assert_eq!(cloth(&a, Season::Eggs), (SOOT, OLD_GOLD));
        assert_eq!(cloth(&a, Season::Houses), stark().colours);
    }

    /// The egg week's palette is soot, ember and old gold and nothing else -
    /// no house colour may leak into a card drawn before the hatch.
    #[test]
    fn the_egg_week_has_no_house_colour_in_it() {
        let (a, _) = cast();
        let specks = champion_specks(Season::Eggs, &look());
        for c in specks {
            assert_ne!(c, stark().colours.0, "a house colour in the egg week");
            assert_ne!(c, targaryen().colours.0, "a house colour in the egg week");
        }
        assert_eq!(cloth(&a, Season::Eggs), (SOOT, OLD_GOLD));
        assert!(Season::Houses.houses() && !Season::Eggs.houses());
        assert_eq!(Season::default(), Season::Eggs, "the month opens on the egg week");
    }

    /// Every puzzle draws, and both seasons draw. Drawing a card is slow, so
    /// the templates take turns at the two dresses rather than every one of
    /// them being drawn twice; the dress is tested on its own above.
    #[test]
    fn the_scroll_card_draws_every_puzzle_in_both_seasons() {
        use super::super::battle_scroll::{Rng, TEMPLATES};
        let (a, b) = cast();
        let mut seasons = std::collections::HashSet::new();
        for (n, template) in TEMPLATES.iter().enumerate() {
            let season = if n % 2 == 0 { Season::Eggs } else { Season::Houses };
            let mut rng = Rng::new(template.kind.len() as u64 * 7 + 11);
            // The riddle bank is not open in a test, so that template has
            // nothing to draw; every other one does.
            let Some(puzzle) = (template.make)(&mut rng) else { continue };
            let scroll = Scroll {
                left: &a,
                right: &b,
                number: "Scroll 2 of 3".to_string(),
                score: [1, 0],
                prompt: &puzzle.prompt,
                spec: &puzzle.spec,
                season,
                seconds: 25,
            };
            let png = scroll_png(&scroll).expect("scroll card");
            assert_eq!(&png[..4], &PNG_MAGIC, "{} in {:?}", template.kind, season);
            seasons.insert(season);
        }
        assert_eq!(seasons.len(), 2, "both dresses have to have been drawn");
    }

    /// The awkward scrolls: nobody sorted, no pictures, a blank prompt, an
    /// empty spec, and a score nobody could reach.
    #[test]
    fn the_scroll_card_survives_the_awkward_cases() {
        let bare = Fighter { name: String::new(), avatar: None, hp: 0, max_hp: 0, house: None, stage: 0 };
        let spec = super::super::battle_scroll::Spec::default();
        for season in [Season::Eggs, Season::Houses] {
            let long = "Which of these banners is the one that hangs upside down, counting from the left?";
            for (prompt, score, seconds) in [("", [0, 0], 0u64), (long, [9, 9], 999)] {
                let scroll = Scroll {
                    left: &bare,
                    right: &bare,
                    number: String::new(),
                    score,
                    prompt,
                    spec: &spec,
                    season,
                    seconds,
                };
                assert_eq!(&scroll_png(&scroll).expect("scroll card")[..4], &PNG_MAGIC);
            }
        }
    }

    /// Writes both seasons' cards out to look at:
    /// `BATTLE_CARD_PREVIEW=/tmp cargo test battle_card -- --ignored`.
    #[test]
    #[ignore = "writes files; only useful when looking at the design"]
    fn preview() {
        let Ok(dir) = std::env::var("BATTLE_CARD_PREVIEW") else { return };
        let (a, b) = cast();
        let done = Fighter {
            name: "Meera".to_string(),
            avatar: b.avatar.clone(),
            hp: 0,
            max_hp: 100,
            house: b.house.clone(),
            stage: 2,
        };
        for (season, key) in [(Season::Eggs, "eggs"), (Season::Houses, "houses")] {
            let duel = Fight {
                stage: "Challenge".to_string(),
                left: &a,
                right: &b,
                line: "Rohit ne Meera ko dhaal pe aisa maara ki poore lists mein goonj gaya".to_string(),
                outcome: Outcome::Open,
                hit: Some((1, -27)),
                season,
            };
            let melee = Fight {
                stage: "The Last Eight".to_string(),
                left: &a,
                right: &b,
                line: "Meera ne Rohit ka helm tedha kar diya, ab kuch dikh hi nahi raha".to_string(),
                outcome: Outcome::Open,
                hit: None,
                season,
            };
            let over = Fight {
                stage: "The Final Tilt".to_string(),
                left: &a,
                right: &done,
                line: "Meera gir gayi, Rohit ne talwaar hawa mein ghuma di".to_string(),
                outcome: Outcome::Won(0),
                hit: None,
                season,
            };
            let champ = Champion {
                who: &a,
                subtitle: if season == Season::Eggs {
                    "12 in the lists · 4 rounds · 1 champion".to_string()
                } else {
                    "12 in the lists · 4 rounds · House Stark".to_string()
                },
                line: "Sab dekh rahe hain, Rohit taaj ghuma raha hai".to_string(),
                season,
            };
            let cards = [
                (format!("duel_{key}.png"), fight_png(&duel)),
                (format!("melee_{key}.png"), fight_png(&melee)),
                (format!("result_{key}.png"), fight_png(&over)),
                (format!("champion_{key}.png"), champion_png(&champ)),
            ];
            for (name, png) in cards {
                let bytes = png.expect("preview card");
                std::fs::write(std::path::Path::new(&dir).join(name), bytes).expect("writing the preview");
            }
            // A scroll in this dress, one per template, so every puzzle can be
            // looked at: `scroll_<template>_<dress>.png`.
            use super::super::battle_scroll::{Rng, TEMPLATES};
            for (n, template) in TEMPLATES.iter().enumerate() {
                let mut rng = Rng::new(n as u64 * 2_654_435_761 + 7);
                let Some(puzzle) = (template.make)(&mut rng) else { continue };
                let scroll = Scroll {
                    left: &a,
                    right: &b,
                    number: "Scroll 2 of 3".to_string(),
                    score: [1, 0],
                    prompt: &puzzle.prompt,
                    spec: &puzzle.spec,
                    season,
                    seconds: 25,
                };
                let bytes = scroll_png(&scroll).expect("preview scroll");
                let name = format!("scroll_{}_{key}.png", template.kind);
                std::fs::write(std::path::Path::new(&dir).join(name), bytes).expect("writing the preview");
            }
        }
    }
}
