//! The battle royale bracket: the whole draw as one picture, posted before each
//! round and once more with the champion. `battle.rs` owns the tournament; this
//! only draws what it is handed.
//!
//! The draw is split down the middle: the first half of every round climbs in
//! from the left, the second half from the right, and the two meet at the final
//! in the centre. Colours, floor and glows come from the fight cards' theme
//! look, so a Pokémon bracket sits on the same ground as a Pokémon fight.

use cosmic_text::Weight;
use tiny_skia::{FillRule, FilterQuality, LineCap, PathBuilder, Pixmap, PixmapPaint, Stroke, StrokeDash, Transform};

use super::awards_card::{avatar_pixmap, fill_circle, paint, rrect};
use super::battle_card::{
    crown, dim, down, fill_shaded, floor, glow, lift, look, spaced, vignette, Pen, BG, GOLD, INK, MUTED,
};
use super::battle_theme::Theme;
use super::house::House;

/// Someone in the draw.
pub struct Entrant {
    pub name: String,
    /// Raw profile picture bytes, already downloaded; `None` draws a blank.
    pub avatar: Option<Vec<u8>>,
    pub house: Option<&'static House>,
}

/// One match in the draw. `a` and `b` index `Bracket::entrants`; `None` is a
/// slot nobody has reached yet, or the empty side of a free pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub a: Option<usize>,
    pub b: Option<usize>,
    /// 0 if `a` won, 1 if `b` won.
    pub winner: Option<usize>,
    /// The winner's health left at the end of the fight.
    pub hp: Option<i32>,
    /// A first-round free pass: `a` goes through without a fight.
    pub bye: bool,
    /// Being fought right now.
    pub live: bool,
}

pub struct Bracket<'a> {
    pub entrants: &'a [Entrant],
    /// Every round, first to last; round `r` holds `2^(rounds - 1 - r)` matches in
    /// draw order, so matches `2j` and `2j + 1` feed match `j` of the next round.
    /// The final is the last round, with one match.
    pub rounds: &'a [Vec<Slot>],
    /// "16 warriors · Quarter-finals".
    pub subtitle: String,
    pub theme: Theme,
}

/// What a round is called on the bracket, by how many matches it has.
pub fn round_title(matches: usize) -> String {
    match matches {
        1 => "FINAL".to_string(),
        2 => "SEMI-FINALS".to_string(),
        4 => "QUARTER-FINALS".to_string(),
        n => format!("ROUND OF {}", n * 2),
    }
}

/// PNG bytes of the bracket, or `None` if drawing failed. Draws of 4, 8, 16
/// and 32 (two to five rounds) are laid out; anything else is `None`.
pub fn bracket_png(bracket: &Bracket) -> Option<Vec<u8>> {
    let lay = Layout::for_rounds(bracket.rounds.len())?;
    let mut fs = super::awards::fonts().lock();
    // cosmic-text panics rather than drawing with no fonts at all.
    if fs.db().len() == 0 {
        return None;
    }
    let mut pen = Pen::new(lay.w, lay.h, &mut fs)?;
    draw(&mut pen, bracket, &lay);
    pen.px.encode_png().ok()
}

/// Half the space between the final's two slots when they sit side by side,
/// with the VS in it.
const FINAL_GAP: f32 = 18.0;
const SLOT_FILL: [u8; 3] = [38, 42, 51];
const EMPTY_FILL: [u8; 3] = [28, 31, 38];
const EDGE: [u8; 3] = [139, 147, 167];
const EMPTY_EDGE: [u8; 3] = [64, 68, 80];
const WIRE: [u8; 3] = [74, 79, 92];
const LIVE: [u8; 3] = [239, 68, 68];
const LABEL: [u8; 3] = [139, 147, 167];
const LOSER_INK: [u8; 3] = [154, 161, 176];
const FAINT: [u8; 3] = [107, 114, 130];
/// Initials for anyone without a picture, by where they sit in the draw.
const INITIAL_FILLS: [[u8; 3]; 8] = [
    [231, 111, 81],
    [42, 157, 143],
    [233, 196, 106],
    [123, 108, 246],
    [72, 202, 228],
    [255, 126, 182],
    [144, 190, 109],
    [205, 180, 219],
];

/// Where everything goes for a draw of a given depth. The widths follow from
/// the columns, so a small draw gets a small card instead of a sea of floor.
struct Layout {
    rounds: usize,
    w: f32,
    h: f32,
    /// Outer margin, slot size and the step from one column to the next.
    margin: f32,
    slot_w: f32,
    slot_h: f32,
    pitch: f32,
    final_w: f32,
    /// Top of the draw area, the height each first-round match gets, and how
    /// far a first-round match's two slots sit from its middle.
    top: f32,
    match_h: f32,
    pair: f32,
    /// Name type size.
    name: f32,
    /// The 32-draw has no room for the final's slots side by side, so they
    /// stack in the centre column instead.
    stacked: bool,
}

impl Layout {
    fn for_rounds(rounds: usize) -> Option<Layout> {
        // (margin, slot_w, slot_h, column gap, final_w, top, match_h, pair, name)
        let (margin, slot_w, slot_h, gap, final_w, top, match_h, pair, name) = match rounds {
            2 => (24.0, 196.0, 38.0, 18.0, 170.0, 150.0, 190.0, 23.0, 15.0),
            3 => (20.0, 190.0, 38.0, 20.0, 164.0, 160.0, 160.0, 23.0, 15.0),
            4 => (18.0, 184.0, 38.0, 16.0, 156.0, 160.0, 155.0, 23.0, 15.0),
            5 => (14.0, 168.0, 32.0, 16.0, 160.0, 152.0, 84.0, 19.0, 14.0),
            _ => return None,
        };
        let stacked = rounds == 5;
        let columns = (rounds - 1) as f32;
        let side = margin + (columns - 1.0) * (slot_w + gap) + slot_w;
        let w = if stacked { 2.0 * (side + 36.0) + final_w } else { 2.0 * (side + 40.0 + FINAL_GAP + final_w) };
        let firsts = (1usize << (rounds - 2)) as f32;
        let mut lay = Layout {
            rounds,
            w: w.round(),
            h: 0.0,
            margin,
            slot_w,
            slot_h,
            pitch: slot_w + gap,
            final_w,
            top,
            match_h,
            pair,
            name,
            stacked,
        };
        // Room for the champion line under the final, then the legend.
        let low = (top + firsts * match_h).max(lay.final_y() + lay.final_off() + slot_h / 2.0 + 56.0);
        lay.h = (low + 52.0).round();
        Some(lay)
    }

    /// The columns either side of the final.
    fn columns(&self) -> usize {
        self.rounds - 1
    }

    /// Left edge of column `r` on `side` (0 left, 1 right).
    fn column_x(&self, r: usize, side: usize) -> f32 {
        let x = self.margin + r as f32 * self.pitch;
        if side == 0 {
            x
        } else {
            self.w - x - self.slot_w
        }
    }

    /// Middle of local match `j` in column `r`: halfway between the two
    /// matches that feed it.
    fn match_y(&self, r: usize, j: usize) -> f32 {
        self.top + (j as f32 + 0.5) * self.match_h * (1u32 << r) as f32
    }

    /// Distance from a match's middle to either of its slots.
    fn slot_off(&self, r: usize) -> f32 {
        if r == 0 {
            self.pair
        } else {
            self.match_h * (1u32 << (r - 1)) as f32 / 2.0
        }
    }

    /// The round names' baseline: a little above the highest slot.
    fn label_y(&self) -> f32 {
        self.match_y(0, 0) - self.pair - self.slot_h / 2.0 - 22.0
    }

    fn final_y(&self) -> f32 {
        self.top + (1usize << (self.rounds - 2)) as f32 * self.match_h / 2.0
    }

    /// How far each final slot sits above or below the final's middle.
    fn final_off(&self) -> f32 {
        if self.stacked {
            self.slot_h / 2.0 + 11.0
        } else {
            0.0
        }
    }

    /// Where final slot `k` sits: its left edge and middle.
    fn final_slot(&self, k: usize) -> (f32, f32) {
        let (mid, fy) = (self.w / 2.0, self.final_y());
        if self.stacked {
            let dy = if k == 0 { -self.final_off() } else { self.final_off() };
            (mid - self.final_w / 2.0, fy + dy)
        } else if k == 0 {
            (mid - FINAL_GAP - self.final_w, fy)
        } else {
            (mid + FINAL_GAP, fy)
        }
    }
}

/// How a slot is shown.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Winner,
    Loser,
    Live,
    /// Somebody is here and waiting for the fight.
    Waiting,
    /// Nobody has got here yet.
    Empty,
    /// The empty side of a free pass: no box, just the words, so the one
    /// going through keeps the whole slot for their name.
    FreePass,
}

struct View {
    who: Option<usize>,
    mark: Mark,
    hp: Option<i32>,
}

/// Both sides of a match as they should be drawn.
fn views(m: &Slot, entrants: usize) -> [View; 2] {
    let known = |i: Option<usize>| i.filter(|&i| i < entrants);
    let one = |k: usize| {
        let who = known(if k == 0 { m.a } else { m.b });
        if m.bye {
            let mark = match (k, who) {
                (0, Some(_)) => Mark::Winner,
                (0, None) => Mark::Empty,
                _ => Mark::FreePass,
            };
            return View { who: if k == 0 { who } else { None }, mark, hp: None };
        }
        let mark = match (who, m.winner) {
            (None, _) => Mark::Empty,
            (Some(_), Some(w)) if w == k => Mark::Winner,
            (Some(_), Some(w)) if w < 2 => Mark::Loser,
            (Some(_), _) if m.live => Mark::Live,
            _ => Mark::Waiting,
        };
        let hp = if mark == Mark::Winner { m.hp } else { None };
        View { who, mark, hp }
    };
    [one(0), one(1)]
}

/// Whether a match's result is in, and whether it carries `k` onwards.
fn carries(m: &Slot, k: usize) -> bool {
    if m.bye {
        k == 0
    } else {
        m.winner == Some(k)
    }
}

fn decided(m: &Slot) -> bool {
    m.bye || matches!(m.winner, Some(0 | 1))
}

/// Pictures decoded once each: every entrant's avatar at slot size, and each
/// house crest that appears.
struct Art {
    avatars: Vec<Option<Pixmap>>,
    crests: Vec<(&'static str, Option<Pixmap>)>,
}

impl Art {
    fn new(entrants: &[Entrant], avatar: u32, crest: u32) -> Art {
        let avatars = entrants.iter().map(|e| avatar_pixmap(e.avatar.as_deref(), avatar, false)).collect();
        let mut crests: Vec<(&'static str, Option<Pixmap>)> = Vec::new();
        for house in entrants.iter().filter_map(|e| e.house) {
            if !crests.iter().any(|(k, _)| *k == house.key) {
                crests.push((house.key, super::house_card::crest_art(house.key, crest)));
            }
        }
        Art { avatars, crests }
    }

    fn crest(&self, house: &House) -> Option<&Pixmap> {
        self.crests.iter().find(|(k, _)| *k == house.key).and_then(|(_, p)| p.as_ref())
    }
}

fn draw(pen: &mut Pen<'_>, b: &Bracket, lay: &Layout) {
    let look = look(b.theme);
    let (w, h) = (lay.w, lay.h);
    floor(&mut pen.px, w, h, look.floor);
    glow(&mut pen.px, w * 0.10, h * 0.5, w * 0.45, look.left, 20);
    glow(&mut pen.px, w * 0.90, h * 0.5, w * 0.45, look.right, 22);
    glow(&mut pen.px, w * 0.5, lay.final_y(), w * 0.42, GOLD, 26);
    vignette(&mut pen.px, w, h, 110);

    header(pen, b, lay, (look.left, look.right));
    let art = Art::new(b.entrants, (lay.slot_h * 0.64).round() as u32, (lay.slot_h * 0.63).round() as u32);
    let ground = |y: f32| ground_at(look.floor, y / h);

    // Wires first, so the slots sit over their ends.
    for r in 0..lay.columns() {
        for (j, m) in round(b, r).iter().enumerate() {
            wires(pen, lay, r, j, m, b.entrants.len());
        }
    }
    for r in 0..lay.columns() {
        let count = 1usize << (lay.rounds - 1 - r);
        for (j, m) in round(b, r).iter().enumerate().take(count) {
            let (side, local) = if j < count / 2 { (0, j) } else { (1, j - count / 2) };
            let x = lay.column_x(r, side);
            let (mid, off) = (lay.match_y(r, local), lay.slot_off(r));
            for (k, view) in views(m, b.entrants.len()).iter().enumerate() {
                let y = if k == 0 { mid - off } else { mid + off };
                slot(pen, &art, b.entrants, view, (x, y, lay.slot_w), lay, ground(y));
            }
        }
        for side in [0, 1] {
            let x = lay.column_x(r, side) + lay.slot_w / 2.0;
            let label = spaced(&round_title(1 << (lay.rounds - 1 - r)));
            pen.centered(&label, x, lay.label_y(), 13.0, Weight::EXTRA_BOLD, LABEL);
        }
    }
    final_block(pen, b, lay, &art, ground(lay.final_y()));

    let legend = "Gold = winner · HP left after the fight · crest = house";
    pen.centered(legend, w / 2.0, h - 22.0, 14.0, Weight::MEDIUM, FAINT);
}

/// The floor's colour a fraction `t` of the way down, for veiling a slot in
/// the same colour as the ground behind it.
fn ground_at(stops: &[(f32, [u8; 3])], t: f32) -> [u8; 3] {
    let Some(first) = stops.first() else { return BG };
    let mut prev = *first;
    for &(at, c) in stops {
        if t <= at {
            let span = (at - prev.0).max(0.001);
            let f = ((t - prev.0) / span).clamp(0.0, 1.0);
            return [0, 1, 2].map(|i| (prev.1[i] as f32 + (c[i] as f32 - prev.1[i] as f32) * f) as u8);
        }
        prev = (at, c);
    }
    prev.1
}

/// A round's matches, or an empty list for a round that was not handed over.
fn round<'a>(b: &'a Bracket, r: usize) -> &'a [Slot] {
    b.rounds.get(r).map_or(&[], |v| v.as_slice())
}

/// The title with crossed swords before it, and the subtitle under it. Any
/// part of the subtitle from a "●" on is news, and is picked out in red.
fn header(pen: &mut Pen<'_>, b: &Bracket, lay: &Layout, tints: ([u8; 3], [u8; 3])) {
    let title = "BATTLE ROYALE";
    let (size, icon) = (38.0, 46.0);
    let tw = pen.measure(title, size, Weight::EXTRA_BOLD);
    let left = lay.w / 2.0 - (tw + icon + 14.0) / 2.0;
    swords(&mut pen.px, left + icon / 2.0, 50.0, icon, tints);
    text_left(pen, title, left + icon + 14.0, 64.0, size, Weight::EXTRA_BOLD, INK);

    let sub = b.subtitle.trim();
    if sub.is_empty() {
        return;
    }
    let sub = pen.fit(sub, 17.0, Weight::MEDIUM, lay.w - 80.0);
    let (calm, news) = match sub.find('●') {
        Some(i) => sub.split_at(i),
        None => (sub.as_str(), ""),
    };
    let cw = pen.measure(calm, 17.0, Weight::MEDIUM);
    let nw = if news.is_empty() { 0.0 } else { pen.measure(news, 17.0, Weight::BOLD) };
    let x = lay.w / 2.0 - (cw + nw) / 2.0;
    text_left(pen, calm, x, 99.0, 17.0, Weight::MEDIUM, [154, 161, 176]);
    if !news.is_empty() {
        text_left(pen, news, x + cw, 99.0, 17.0, Weight::BOLD, LIVE);
    }
}

/// One line with its left edge at `x` and its baseline at `baseline`; returns
/// how wide it came out.
fn text_left(pen: &mut Pen<'_>, text: &str, x: f32, baseline: f32, size: f32, weight: Weight, c: [u8; 3]) -> f32 {
    let buf = pen.layout(text, size, size * 1.3, weight, None);
    let (line_y, w) = buf.layout_runs().next().map(|r| (r.line_y, r.line_w)).unwrap_or((size, 0.0));
    pen.draw(&buf, x, baseline - line_y, c);
    w
}

/// Bracket elbows from a match's two slots in column `r` to the slot it feeds
/// next: gold along the way a winner went, grey everywhere else.
fn wires(pen: &mut Pen<'_>, lay: &Layout, r: usize, j: usize, m: &Slot, entrants: usize) {
    let count = 1usize << (lay.rounds - 1 - r);
    if j >= count {
        return;
    }
    let (side, local) = if j < count / 2 { (0, j) } else { (1, j - count / 2) };
    let dir = if side == 0 { 1.0 } else { -1.0 };
    let x = lay.column_x(r, side);
    let out = if side == 0 { x + lay.slot_w } else { x };
    let (mid, off) = (lay.match_y(r, local), lay.slot_off(r));
    let last = r + 1 == lay.columns();
    // Where the winner's line ends: the next column's slot, or the final's.
    let (into, into_y) = if !last {
        let next = lay.column_x(r + 1, side);
        (if side == 0 { next } else { next + lay.slot_w }, mid)
    } else {
        let (fx, fy) = lay.final_slot(side);
        (if side == 0 { fx } else { fx + lay.final_w }, fy)
    };
    let elbow = if last { (out + into) / 2.0 } else { out + dir * 12.0 };
    let done = decided(m);
    let present = views(m, entrants);
    for k in [0, 1] {
        if present[k].mark == Mark::FreePass {
            continue;
        }
        let y = if k == 0 { mid - off } else { mid + off };
        let colour = if done && carries(m, k) { GOLD } else { WIRE };
        polyline(&mut pen.px, &[(out, y), (elbow, y), (elbow, mid)], colour);
    }
    let colour = if done { GOLD } else { WIRE };
    polyline(&mut pen.px, &[(elbow, mid), (elbow, into_y), (into, into_y)], colour);
}

fn polyline(px: &mut Pixmap, points: &[(f32, f32)], c: [u8; 3]) {
    let mut pb = PathBuilder::new();
    for (i, (x, y)) in points.iter().enumerate() {
        if i == 0 {
            pb.move_to(*x, *y);
        } else {
            pb.line_to(*x, *y);
        }
    }
    if let Some(path) = pb.finish() {
        let stroke = Stroke { width: 2.0, line_cap: LineCap::Square, ..Stroke::default() };
        px.stroke_path(&path, &paint(c, 255), &stroke, Transform::identity(), None);
    }
}

/// One slot: a rounded box with the fighter's picture, name and crest, and on
/// the right whatever the result says - health left, LIVE or a free pass.
/// `at` is the left edge, the middle and the width.
fn slot(
    pen: &mut Pen<'_>,
    art: &Art,
    entrants: &[Entrant],
    v: &View,
    at: (f32, f32, f32),
    lay: &Layout,
    ground: [u8; 3],
) {
    let (x, cy, w) = at;
    let h = lay.slot_h;
    let Some(shape) = rrect(x, cy - h / 2.0, w, h, 9.0) else { return };
    let (fill, edge, width) = match v.mark {
        Mark::Winner => (SLOT_FILL, GOLD, 2.4),
        Mark::Live => (SLOT_FILL, LIVE, 2.4),
        Mark::Waiting => (SLOT_FILL, EDGE, 1.6),
        Mark::Loser => (EMPTY_FILL, EMPTY_EDGE, 1.6),
        Mark::Empty => (EMPTY_FILL, EMPTY_EDGE, 1.6),
        Mark::FreePass => {
            let size = (h * 0.30).round();
            let text = spaced("FREE PASS");
            pen.centered(&text, x + w / 2.0, cy + size * 0.36, size, Weight::EXTRA_BOLD, dim(GOLD, 0.62));
            return;
        }
    };
    pen.px.fill_path(&shape, &paint(fill, 255), FillRule::Winding, Transform::identity(), None);
    let dash = if v.mark == Mark::Empty { StrokeDash::new(vec![5.0, 5.0], 0.0) } else { None };
    let stroke = Stroke { width, dash, ..Stroke::default() };
    pen.px.stroke_path(&shape, &paint(edge, 255), &stroke, Transform::identity(), None);

    let Some(who) = v.who.and_then(|i| entrants.get(i).map(|e| (i, e))) else {
        let size = (h * 0.34).round();
        pen.centered(&spaced("TBD"), x + w / 2.0, cy + size * 0.36, size, Weight::BOLD, [93, 99, 115]);
        return;
    };
    let (index, entrant) = who;
    let pad = h * 0.18;
    let r = h * 0.32;
    avatar(pen, art, index, entrant, (x + pad + r, cy, r));

    // Right to left: crest, then the result, then whatever width is left
    // for the name.
    let mut right = x + w - 8.0;
    if let Some(house) = entrant.house {
        if let Some(crest) = art.crest(house) {
            let side = crest.width() as f32;
            let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..PixmapPaint::default() };
            let (px, py) = ((right - side).round() as i32, (cy - side / 2.0).round() as i32);
            pen.px.draw_pixmap(px, py, crest.as_ref(), &paint, Transform::identity(), None);
            right -= side + 6.0;
        }
    }
    let small = (h * 0.31).round().max(11.0);
    if v.mark == Mark::Live {
        right -= tag(pen, "LIVE", right, cy, small, (LIVE, [255, 255, 255])) + 6.0;
    } else if let Some(hp) = v.hp {
        let text = format!("{hp} HP");
        let tw = pen.measure(&text, small, Weight::BOLD);
        text_left(pen, &text, right - tw, cy + small * 0.36, small, Weight::BOLD, GOLD);
        right -= tw + 8.0;
    }

    let name_x = x + pad + 2.0 * r + 7.0;
    let (weight, ink) = match v.mark {
        Mark::Winner => (Weight::EXTRA_BOLD, [255, 255, 255]),
        Mark::Loser => (Weight::SEMIBOLD, LOSER_INK),
        _ => (Weight::SEMIBOLD, [233, 235, 240]),
    };
    // A long name steps down a size before it loses any letters.
    let room = (right - name_x).max(12.0);
    let full = entrant.name.trim();
    let size = if pen.measure(full, lay.name, weight) > room { (lay.name - 2.0).max(13.0) } else { lay.name };
    let name = pen.fit(full, size, weight, room);
    let nw = text_left(pen, &name, name_x, cy + size * 0.36, size, weight, ink);
    if v.mark == Mark::Loser {
        polyline(&mut pen.px, &[(name_x, cy + 0.5), (name_x + nw, cy + 0.5)], LOSER_INK);
        // Knocked back to about half strength, box and all.
        if let Some(veil) = rrect(x - 1.5, cy - h / 2.0 - 1.5, w + 3.0, h + 3.0, 10.0) {
            pen.px.fill_path(&veil, &paint(ground, 120), FillRule::Winding, Transform::identity(), None);
        }
    }
}

/// The small round picture at a slot's left, or a coloured initial for anyone
/// without one. `at` is the centre and radius.
fn avatar(pen: &mut Pen<'_>, art: &Art, index: usize, entrant: &Entrant, at: (f32, f32, f32)) {
    let (cx, cy, r) = at;
    if let Some(Some(pic)) = art.avatars.get(index) {
        let side = pic.width() as f32;
        let (px, py) = ((cx - side / 2.0).round() as i32, (cy - side / 2.0).round() as i32);
        pen.px.draw_pixmap(px, py, pic.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        return;
    }
    fill_circle(&mut pen.px, cx, cy, r, INITIAL_FILLS[index % INITIAL_FILLS.len()]);
    let first = entrant.name.chars().find(|c| c.is_alphanumeric());
    let initial = first.map_or("?".to_string(), |c| c.to_uppercase().to_string());
    let size = (r * 1.05).round();
    pen.centered(&initial, cx, cy + size * 0.36, size, Weight::EXTRA_BOLD, [16, 19, 26]);
}

/// A small pill ending at `right`, returning its width.
fn tag(pen: &mut Pen<'_>, text: &str, right: f32, cy: f32, size: f32, colours: ([u8; 3], [u8; 3])) -> f32 {
    let tw = pen.measure(text, size, Weight::EXTRA_BOLD);
    let (w, h) = (tw + 12.0, size + 7.0);
    if let Some(pill) = rrect(right - w, cy - h / 2.0, w, h, 4.0) {
        pen.px.fill_path(&pill, &paint(colours.0, 255), FillRule::Winding, Transform::identity(), None);
    }
    text_left(pen, text, right - w + 6.0, cy + size * 0.36, size, Weight::EXTRA_BOLD, colours.1);
    w
}

/// The final in the centre: a trophy and "FINAL" over its two slots, "VS"
/// between them and the champion, once there is one, underneath.
fn final_block(pen: &mut Pen<'_>, b: &Bracket, lay: &Layout, art: &Art, ground: [u8; 3]) {
    let m = round(b, lay.rounds - 1).first().cloned().unwrap_or_default();
    let mid = lay.w / 2.0;
    let fy = lay.final_y();
    let top = fy - lay.final_off() - lay.slot_h / 2.0;
    let bottom = fy + lay.final_off() + lay.slot_h / 2.0;

    trophy(&mut pen.px, mid, top - 44.0, 46.0);
    pen.centered(&spaced("FINAL"), mid, top - 16.0, 16.0, Weight::EXTRA_BOLD, GOLD);
    for (k, view) in views(&m, b.entrants.len()).iter().enumerate() {
        let (x, y) = lay.final_slot(k);
        slot(pen, art, b.entrants, view, (x, y, lay.final_w), lay, ground);
    }
    pen.centered("VS", mid, fy + 4.7, 13.0, Weight::EXTRA_BOLD, GOLD);

    let champion = match m.winner {
        Some(0) => m.a,
        Some(1) => m.b,
        _ => None,
    }
    .and_then(|i| b.entrants.get(i));
    let (line, ink) = match champion {
        Some(e) => (format!("Champion: {}", e.name.trim()), GOLD),
        None => ("Champion: TBD".to_string(), [154, 161, 176]),
    };
    let size = 16.0;
    let line = pen.fit(&line, size, Weight::BOLD, lay.w * 0.3);
    let lw = pen.measure(&line, size, Weight::BOLD);
    let baseline = bottom + 34.0;
    let x = mid - (lw + 38.0) / 2.0;
    crown(&mut pen.px, x + 14.0, baseline - 2.0, 28.0, 19.0);
    text_left(pen, &line, x + 38.0, baseline, size, Weight::BOLD, if champion.is_some() { ink } else { MUTED });
}

/// A plain cup on a plinth, standing with its foot at `base`.
fn trophy(px: &mut Pixmap, cx: f32, base: f32, h: f32) {
    let top = base - h;
    let metal = [(0.0, [255, 240, 168]), (0.45, GOLD), (1.0, dim(GOLD, 0.6))];
    let handle = Stroke { width: h * 0.07, line_cap: LineCap::Round, ..Stroke::default() };
    for side in [-1.0, 1.0] {
        let mut pb = PathBuilder::new();
        pb.move_to(cx + side * h * 0.36, top + h * 0.10);
        let x = |f: f32| cx + side * h * f;
        let y = |f: f32| top + h * f;
        pb.cubic_to(x(0.62), y(0.06), x(0.60), y(0.42), x(0.22), y(0.50));
        if let Some(path) = pb.finish() {
            px.stroke_path(&path, &paint(dim(GOLD, 0.82), 255), &handle, Transform::identity(), None);
        }
    }
    let mut pb = PathBuilder::new();
    pb.move_to(cx - h * 0.40, top);
    pb.line_to(cx + h * 0.40, top);
    pb.cubic_to(cx + h * 0.40, top + h * 0.45, cx + h * 0.20, top + h * 0.62, cx, top + h * 0.64);
    pb.cubic_to(cx - h * 0.20, top + h * 0.62, cx - h * 0.40, top + h * 0.45, cx - h * 0.40, top);
    pb.close();
    if let Some(bowl) = pb.finish() {
        fill_shaded(px, &bowl, down(top, top + h * 0.64, &metal), GOLD);
    }
    if let Some(stem) = rrect(cx - h * 0.06, top + h * 0.60, h * 0.12, h * 0.22, 2.0) {
        fill_shaded(px, &stem, down(top + h * 0.6, base, &metal), GOLD);
    }
    if let Some(plinth) = rrect(cx - h * 0.30, base - h * 0.16, h * 0.60, h * 0.16, 3.0) {
        fill_shaded(px, &plinth, down(base - h * 0.16, base, &metal), GOLD);
    }
    if let Some(shine) = rrect(cx - h * 0.26, top + h * 0.08, h * 0.08, h * 0.30, h * 0.04) {
        px.fill_path(&shine, &paint([255, 255, 255], 110), FillRule::Winding, Transform::identity(), None);
    }
}

/// Two swords crossed, hilts down, guards in the two sides' colours.
fn swords(px: &mut Pixmap, cx: f32, cy: f32, size: f32, guards: ([u8; 3], [u8; 3])) {
    let s = size / 2.0;
    for (angle, guard) in [(-40.0, guards.0), (40.0, guards.1)] {
        let at = Transform::from_rotate(angle).post_translate(cx, cy);
        let mut blade = PathBuilder::new();
        blade.move_to(0.0, -s);
        blade.line_to(s * 0.12, -s * 0.78);
        blade.line_to(s * 0.12, s * 0.42);
        blade.line_to(-s * 0.12, s * 0.42);
        blade.line_to(-s * 0.12, -s * 0.78);
        blade.close();
        if let Some(path) = blade.finish() {
            px.fill_path(&path, &paint([214, 220, 232], 255), FillRule::Winding, at, None);
        }
        if let Some(cross) = rrect(-s * 0.34, s * 0.42, s * 0.68, s * 0.12, s * 0.05) {
            px.fill_path(&cross, &paint(lift(guard, 0.15), 255), FillRule::Winding, at, None);
        }
        if let Some(grip) = rrect(-s * 0.06, s * 0.54, s * 0.12, s * 0.32, s * 0.04) {
            px.fill_path(&grip, &paint(dim(guard, 0.55), 255), FillRule::Winding, at, None);
        }
        if let Some(pommel) = PathBuilder::from_circle(0.0, s * 0.92, s * 0.09) {
            px.fill_path(&pommel, &paint(lift(guard, 0.15), 255), FillRule::Winding, at, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG_MAGIC: [u8; 4] = [0x89, b'P', b'N', b'G'];

    const NAMES: [&str; 32] = [
        "Rohit", "Meera", "Kabir", "Zoya", "Raizelᴰᴺ💫", "Tanya", "Vikram", "Ishaan", "Noor", "Sam", "Ananya", "Dev",
        "Riya", "Kunal", "Aisha", "Honoré de Balzac", "Arjun", "Priya", "Yash", "Sana", "Kian", "Tara", "Omar", "Leela",
        "Neel", "Maya", "Reza", "Ira", "Dhruv", "Esha", "Farhan", "Gia",
    ];
    const HOUSES: [&str; 4] = ["gryffindor", "ravenclaw", "slytherin", "hufflepuff"];

    /// A picture to stand in for a download: a tinted gradient with a head
    /// and shoulders on it.
    fn fake_avatar(tint: [u8; 3]) -> Vec<u8> {
        let n = 96.0_f32;
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

    /// Every third entrant has no picture and every seventh no house, so the
    /// fallbacks get drawn too.
    fn entrants(n: usize) -> Vec<Entrant> {
        (0..n)
            .map(|i| Entrant {
                name: NAMES[i % NAMES.len()].to_string(),
                avatar: (i % 3 != 2).then(|| fake_avatar(INITIAL_FILLS[i % INITIAL_FILLS.len()])),
                house: (i % 7 != 6).then(|| super::super::house::house(HOUSES[i % 4])).flatten(),
            })
            .collect()
    }

    #[derive(Clone, Copy, PartialEq)]
    enum Play {
        Done,
        Live,
        Open,
    }

    /// A draw of `size` slots in its first round, with free passes on the
    /// first-round matches in `byes`, played out as `play(round, match)` says.
    /// Returns the entrants and rounds.
    fn tournament(size: usize, byes: &[usize], play: impl Fn(usize, usize) -> Play) -> (Vec<Entrant>, Vec<Vec<Slot>>) {
        let rounds_n = size.trailing_zeros() as usize;
        let people = size - byes.len();
        let mut next = 0;
        let mut first = Vec::new();
        for j in 0..size / 2 {
            let a = Some(next);
            next += 1;
            let b = if byes.contains(&j) {
                None
            } else {
                next += 1;
                Some(next - 1)
            };
            first.push(Slot { a, b, ..Slot::default() });
        }
        let mut rounds = vec![first];
        for r in 0..rounds_n {
            for (j, m) in rounds[r].iter_mut().enumerate() {
                if m.b.is_none() && byes.contains(&j) && r == 0 {
                    m.bye = true;
                    m.winner = Some(0);
                    continue;
                }
                if m.a.is_none() || m.b.is_none() {
                    continue;
                }
                match play(r, j) {
                    Play::Done => {
                        m.winner = Some((j + r) % 2);
                        m.hp = Some(3 + ((j * 37 + r * 11) % 90) as i32);
                    }
                    Play::Live => m.live = true,
                    Play::Open => {}
                }
            }
            if r + 1 == rounds_n {
                break;
            }
            let fed: Vec<Slot> = rounds[r]
                .chunks(2)
                .map(|pair| {
                    let won = |m: &Slot| match m.winner {
                        Some(0) => m.a,
                        Some(1) => m.b,
                        _ => None,
                    };
                    Slot { a: won(&pair[0]), b: won(&pair[1]), ..Slot::default() }
                })
                .collect();
            rounds.push(fed);
        }
        (entrants(people), rounds)
    }

    fn render(entrants: &[Entrant], rounds: &[Vec<Slot>], subtitle: &str, theme: Theme) -> Vec<u8> {
        let bracket = Bracket { entrants, rounds, subtitle: subtitle.to_string(), theme };
        bracket_png(&bracket).expect("bracket")
    }

    /// The draws the preview writes out, by file name.
    fn scenes() -> Vec<(String, Vec<Entrant>, Vec<Vec<Slot>>, String, Theme)> {
        let mut out = Vec::new();
        let (e, r) = tournament(4, &[], |r, _| if r == 0 { Play::Done } else { Play::Live });
        out.push(("bracket_4.png".into(), e, r, "4 warriors · Final · ● Rohit vs Zoya fighting now".into(), Theme::Classic));
        let (e, r) = tournament(8, &[1, 2], |_, _| Play::Open);
        out.push(("bracket_8_start.png".into(), e, r, "6 warriors · Quarter-finals".into(), Theme::Classic));
        let (e, r) = tournament(16, &[], |r, j| match (r, j) {
            (0, _) | (1, 0) | (1, 3) => Play::Done,
            (1, 2) => Play::Live,
            _ => Play::Open,
        });
        out.push(("bracket_16_mid.png".into(), e, r, "16 warriors · Quarter-finals · ● Noor vs Dev fighting now".into(), Theme::Classic));
        let (e, r) = tournament(32, &[3, 9, 14], |r, j| match (r, j) {
            (0, _) | (1, 0..=3) | (1, 4) | (1, 6) => Play::Done,
            (1, 5) => Play::Live,
            _ => Play::Open,
        });
        out.push(("bracket_32_mid.png".into(), e, r, "29 warriors · Round of 16 · ● Yash vs Tara fighting now".into(), Theme::Classic));
        let (e, r) = tournament(32, &[], |_, _| Play::Open);
        out.push(("bracket_32_start.png".into(), e, r, "32 warriors · Round of 32".into(), Theme::Classic));
        let (e, r) = tournament(32, &[], |r, _| if r == 0 { Play::Done } else { Play::Open });
        out.push(("bracket_32_round2.png".into(), e, r, "32 warriors · Round of 16".into(), Theme::Classic));
        for theme in [Theme::Classic, Theme::Pokemon, Theme::Tarnished] {
            let (e, r) = tournament(16, &[5], |_, _| Play::Done);
            let name = format!("bracket_16_done_{}.png", theme.key());
            out.push((name, e, r, "15 warriors · 4 rounds · 1 champion".into(), theme));
        }
        out
    }

    #[test]
    fn every_size_and_state_renders() {
        for (_, e, r, sub, theme) in scenes() {
            assert_eq!(&render(&e, &r, &sub, theme)[..4], &PNG_MAGIC);
        }
        let (e, r) = tournament(8, &[0], |r, _| if r == 0 { Play::Done } else { Play::Live });
        assert_eq!(&render(&e, &r, "", Theme::Tactical)[..4], &PNG_MAGIC);
    }

    #[test]
    fn odd_input_does_not_panic() {
        let e = entrants(3);
        // Indexes past the entrants, a round missing its matches, and a
        // winner index that names nobody.
        let rounds = vec![
            vec![Slot { a: Some(0), b: Some(9), winner: Some(1), hp: Some(4), ..Slot::default() }, Slot::default()],
            vec![Slot { a: Some(2), b: None, winner: Some(5), ..Slot::default() }],
        ];
        assert_eq!(&render(&e, &rounds, "x", Theme::Wizard)[..4], &PNG_MAGIC);
        // One round, or none, is not a draw this lays out.
        for rounds in [&rounds[..1], &[]] {
            let bracket = Bracket { entrants: &e, rounds, subtitle: String::new(), theme: Theme::Classic };
            assert!(bracket_png(&bracket).is_none());
        }
    }

    #[test]
    fn layouts_fit_their_canvas() {
        for rounds in 2..=5 {
            let lay = Layout::for_rounds(rounds).expect("layout");
            assert!(lay.w <= 1700.0 && lay.h <= 1400.0, "{rounds} rounds: {}x{}", lay.w, lay.h);
            let side_edge = lay.column_x(lay.columns() - 1, 0) + lay.slot_w;
            assert!(side_edge + 24.0 <= lay.final_slot(0).0, "{rounds} rounds: final crowds the draw");
        }
    }

    /// Writes the preview draws out to look at:
    /// `BRACKET_PREVIEW=/tmp cargo test battle_bracket -- --ignored`.
    #[test]
    #[ignore = "writes files; only useful when looking at the design"]
    fn preview() {
        let Ok(dir) = std::env::var("BRACKET_PREVIEW") else { return };
        for (name, e, r, sub, theme) in scenes() {
            let png = render(&e, &r, &sub, theme);
            std::fs::write(std::path::Path::new(&dir).join(name), png).expect("writing the preview");
        }
    }
}
