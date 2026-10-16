//! A household, drawn.
//!
//! Three rows and one rule: parents above, the married row in the middle,
//! children below. Everyone is a real profile picture where there is one and
//! their initial where there is not, which is the same fallback every other
//! card in this bot uses.
//!
//! WHY NOT THE WHOLE SERVER. A card is one fixed rectangle. Twenty households
//! drawn small enough to fit makes every face a smudge and every line a
//! scribble - which is what the page is for, with its zoom and its dragging.
//! So the card draws ONE household properly and the numbers sit beside it.

use cosmic_text::{FontSystem, Weight};
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use super::awards_card::{avatar_pixmap, fill_circle};
use super::roast_card::{clip, faces, initial, Ink};

const W: f32 = 1100.0;
const H: f32 = 760.0;

const INK: [u8; 3] = [243, 236, 246];
const DIM: [u8; 3] = [160, 140, 174];
const FAINT: [u8; 3] = [110, 95, 124];
const LOVE: [u8; 3] = [224, 114, 159];
const BLOOD: [u8; 3] = [79, 127, 109];

/// One person on the card.
#[derive(Clone, Debug, Default)]
pub struct Face {
    pub name: String,
    pub avatar: Option<Vec<u8>>,
    /// "since 12 Oct", or empty.
    pub note: String,
}

/// A household as the card draws it.
#[derive(Clone, Debug, Default)]
pub struct House {
    /// Whose card it is - drawn biggest, in the middle of the married row.
    pub who: Face,
    pub spouses: Vec<Face>,
    pub parents: Vec<Face>,
    pub children: Vec<Face>,
    /// The line under the title: "3 generations · 8 people".
    pub note: String,
}

fn ground(px: &mut Pixmap) {
    // A warm dark, brightest behind the married row, so the eye lands there.
    let (w, h) = (px.width(), px.height());
    let pixels = px.pixels_mut();
    for y in 0..h {
        let t = y as f32 / h as f32;
        let lift = (1.0 - (t - 0.42).abs() * 1.8).clamp(0.0, 1.0);
        let c = tiny_skia::ColorU8::from_rgba(
            (13.0 + 28.0 * lift) as u8,
            (10.0 + 14.0 * lift) as u8,
            (18.0 + 30.0 * lift) as u8,
            255,
        )
        .premultiply();
        for x in 0..w {
            if let Some(p) = pixels.get_mut((y * w + x) as usize) {
                *p = c;
            }
        }
    }
}

/// A face in a ring, with its name under it.
fn portrait(ink: &mut Ink, sans: &str, f: &Face, cx: f32, cy: f32, r: f32, ring: [u8; 3]) {
    fill_circle(&mut ink.px, cx, cy + 4.0, r + 5.0, [8, 6, 11]);
    fill_circle(&mut ink.px, cx, cy, r + 3.0, ring);
    let side = (r * 2.0).round() as u32;
    match avatar_pixmap(f.avatar.as_deref(), side, false) {
        Some(pm) => ink.px.draw_pixmap(
            (cx - r).round() as i32,
            (cy - r).round() as i32,
            pm.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        ),
        None => {
            fill_circle(&mut ink.px, cx, cy, r, [58, 47, 68]);
            ink.centred(&initial(&f.name), sans, cx, cy + r * 0.36, r * 1.05, Weight::BOLD, INK);
        }
    }
    ink.centred(&clip(&f.name, 16), sans, cx, cy + r + 26.0, 19.0, Weight::MEDIUM, INK);
    if !f.note.is_empty() {
        ink.centred(&clip(&f.note, 22), sans, cx, cy + r + 46.0, 14.0, Weight::NORMAL, DIM);
    }
}

/// A row of faces, spread evenly and centred.
fn row(ink: &mut Ink, sans: &str, people: &[Face], cy: f32, r: f32, ring: [u8; 3]) {
    if people.is_empty() {
        return;
    }
    let gap = (r * 2.0 + 44.0).min((W - 120.0) / people.len() as f32);
    let span = gap * (people.len() - 1) as f32;
    for (i, f) in people.iter().enumerate() {
        portrait(ink, sans, f, W / 2.0 - span / 2.0 + gap * i as f32, cy, r, ring);
    }
}

fn line(px: &mut Pixmap, x1: f32, y1: f32, x2: f32, y2: f32, c: [u8; 3]) {
    let mut pb = tiny_skia::PathBuilder::new();
    pb.move_to(x1, y1);
    pb.line_to(x2, y2);
    if let Some(path) = pb.finish() {
        let stroke = tiny_skia::Stroke { width: 2.0, ..tiny_skia::Stroke::default() };
        px.stroke_path(&path, &super::awards_card::paint(c, 190), &stroke, Transform::identity(), None);
    }
}

/// The card, or `None` where no fonts are installed.
pub fn house_png(fs: &mut FontSystem, house: &House) -> Option<Vec<u8>> {
    let (serif, sans, _) = faces(fs);
    let mut ink = Ink::new(fs, &[&serif, &sans])?;
    ink.px = Pixmap::new(W as u32, H as u32)?;
    ground(&mut ink.px);

    let title = format!("The House of {}", clip(&house.who.name, 18));
    ink.centred(&title, &serif, W / 2.0, 62.0, 42.0, Weight::BOLD, INK);
    if !house.note.is_empty() {
        ink.centred(&house.note, &sans, W / 2.0, 92.0, 17.0, Weight::NORMAL, DIM);
    }

    // Parents above, joined down to the middle.
    if !house.parents.is_empty() {
        ink.centred("PARENTS", &sans, W / 2.0, 142.0, 12.0, Weight::MEDIUM, FAINT);
        row(&mut ink, &sans, &house.parents, 196.0, 42.0, [111, 95, 125]);
        line(&mut ink.px, W / 2.0, 266.0, W / 2.0, 306.0, [74, 58, 84]);
    }

    // The married row: whoever the card is for, biggest, with their spouses
    // beside them and a heart between each pair.
    let mut middle: Vec<Face> = vec![house.who.clone()];
    middle.extend(house.spouses.iter().cloned());
    ink.centred(
        if house.spouses.is_empty() { "UNMARRIED" } else { "MARRIED" },
        &sans,
        W / 2.0,
        330.0,
        12.0,
        Weight::MEDIUM,
        FAINT,
    );
    row(&mut ink, &sans, &middle, 400.0, 58.0, LOVE);
    if !house.spouses.is_empty() {
        let gap = (58.0 * 2.0 + 44.0f32).min((W - 120.0) / middle.len() as f32);
        let span = gap * (middle.len() - 1) as f32;
        for i in 0..middle.len().saturating_sub(1) {
            let x = W / 2.0 - span / 2.0 + gap * i as f32 + gap / 2.0;
            ink.centred("\u{2665}", &sans, x, 408.0, 26.0, Weight::BOLD, LOVE);
        }
    }

    // Children below. The line starts UNDER the married row's "since" notes -
    // drawn from the portraits it would run straight through the words.
    if !house.children.is_empty() {
        line(&mut ink.px, W / 2.0, 520.0, W / 2.0, 552.0, [40, 70, 60]);
        ink.centred("CHILDREN", &sans, W / 2.0, 578.0, 12.0, Weight::MEDIUM, FAINT);
        row(&mut ink, &sans, &house.children, 636.0, 38.0, BLOOD);
    }

    // Clear of the children's names, which sit at their circle plus 26.
    ink.centred("/family · /marry · /love adopt", &sans, W / 2.0, H - 18.0, 14.0, Weight::NORMAL, FAINT);
    ink.px.encode_png().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(name: &str) -> Face {
        Face { name: name.into(), avatar: None, note: String::new() }
    }

    #[test]
    fn a_household_draws_with_no_pictures_at_all() {
        let mut fs = super::super::awards::fonts().lock();
        if fs.db().len() == 0 {
            return; // no fonts on this machine; the renderer refuses, which is its own test
        }
        let house = House {
            who: face("Billori"),
            spouses: vec![face("Kaju"), face("Maddy")],
            parents: vec![face("Sailor")],
            children: vec![face("tulo"), face("Izumi")],
            note: "3 generations · 6 people".into(),
        };
        let png = house_png(&mut fs, &house).expect("a card");
        assert!(png.len() > 1000, "a real png came out");
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn somebody_alone_still_gets_a_card() {
        let mut fs = super::super::awards::fonts().lock();
        if fs.db().len() == 0 {
            return;
        }
        let house = House { who: face("Nobody"), note: "just you, for now".into(), ..House::default() };
        assert!(house_png(&mut fs, &house).is_some(), "an empty family is still a card");
    }

    /// `MARRIAGE_CARD_PREVIEW=/tmp cargo test marriage_card -- --ignored`
    #[test]
    #[ignore = "writes a file; only useful when looking at the design"]
    fn preview() {
        let Ok(dir) = std::env::var("MARRIAGE_CARD_PREVIEW") else { return };
        let mut fs = super::super::awards::fonts().lock();
        let mut with = |name: &str, note: &str| Face {
            name: name.into(),
            avatar: std::fs::read(format!("{}/{}.png", dir, name)).ok(),
            note: note.into(),
        };
        let house = House {
            who: with("Billori", ""),
            spouses: vec![with("Kaju", "since 12 Oct"), with("Maddy", "since 14 Oct")],
            parents: vec![with("Sailor", ""), with("chet", "")],
            children: vec![with("tulo", ""), with("Izumi", ""), with("blossom", "")],
            note: "3 generations · 8 people".into(),
        };
        let png = house_png(&mut fs, &house).expect("a card");
        std::fs::write(std::path::Path::new(&dir).join("family.png"), png).expect("write");
    }

    #[test]
    fn a_very_long_name_is_cut_rather_than_run_off_the_card() {
        let long = "a".repeat(80);
        assert!(clip(&long, 16).chars().count() <= 17, "names are clipped before they are drawn");
    }
}
