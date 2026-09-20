//! The Geo bank: the street photos the bot puts up, and the places it takes as
//! answers.
//!
//! Two files in `<workspace>/geobank/`, read once at start the way the movie
//! bank reads `moviebank/`:
//!
//! * `places.json` — the 28 states and 8 union territories, and every town
//!   above twenty thousand people, each with the state it sits in and every
//!   spelling somebody might type. Built from GeoNames by `tools/build.py`.
//! * `spots.json` — the photos: where each one was taken, and which file holds
//!   it. Harvested from KartaView by `tools/harvest.py`, which is why
//!   [`ATTRIBUTION`] goes wherever the pictures do.
//!
//! ## How a guess is judged
//!
//! A guess is a PLACE NAME, whatever scale the person happens to know. Naming
//! the state is the coarse answer; naming the town is the sharp one, and it
//! carries the state with it, because the bank knows which state every town is
//! in. Nobody has to know the town, and nobody who does goes unrewarded.
//!
//! The state is a gate, not a tiebreak: **a guess in the wrong state scores
//! nothing**, however close the kilometres happen to come out. A name in the
//! RIGHT state is then scored on how near it lands — [`BULLSEYE_KM`],
//! [`NEAR_KM`], or the state's own credit for a town at the far end of it.
//!
//! Where a name reads two ways the bank takes the reading that helps the
//! player. Delhi is a union territory and a city; whichever scores better for
//! the photo in hand is the one that counts.
//!
//! Where two towns share a name the bigger one owns it, decided once when
//! `places.json` is built — so `Hyderabad` is the one in Telangana, and
//! somebody who means the other one is out of luck the way they would be in any
//! room.
//!
//! If either file is missing the game simply never starts: [`open`] says so once
//! in the log and leaves [`bank`] empty, and every part of the game checks it
//! before doing anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// The credit KartaView's licence asks for, wherever the photos appear.
pub const ATTRIBUTION: &str = "Street imagery from KartaView contributors and Grab (https://kartaview.org/), used under CC BY-SA 4.0.";

/// The formats this code knows how to read. A later one is not guessed at.
pub const PLACES_FORMAT: &str = "GEOPLACES1";
pub const SPOTS_FORMAT: &str = "GEOSPOTS1";

/// Naming a town this near the photo is a bullseye.
pub const BULLSEYE_KM: f64 = 15.0;
/// Naming a town this near is close enough to be worth more than the state.
pub const NEAR_KM: f64 = 60.0;

/// Longer than this and a message can't be a place name at all.
const MAX_GUESS: usize = 48;

static BANK: OnceLock<Bank> = OnceLock::new();

/// One state or union territory: the coarse answer, and the gate every guess
/// has to pass.
#[derive(Debug, Clone, Deserialize)]
pub struct StatePlace {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// Every spelling that counts, already folded. Orissa takes Odisha.
    pub keys: Vec<String>,
}

/// One town: the sharp answer, and the state it hands over for free.
#[derive(Debug, Clone, Deserialize)]
pub struct City {
    pub name: String,
    pub state: String,
    pub lat: f64,
    pub lon: f64,
    pub pop: i64,
    pub keys: Vec<String>,
}

/// One photo, and where on the ground it was taken.
#[derive(Debug, Clone, Deserialize)]
pub struct Spot {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    pub state: String,
    /// The nearest town in the gazetteer, for the reveal. Not the answer — the
    /// answer is wherever the guess lands.
    pub city: Option<String>,
    pub city_km: Option<f64>,
    /// Where the picture is, relative to the bank folder.
    pub file: String,
    /// The KartaView contributor who drove it, credited on the reveal.
    pub by: String,
    pub shot: String,
}

#[derive(Debug, Deserialize)]
struct PlacesFile {
    format: String,
    states: Vec<StatePlace>,
    cities: Vec<City>,
}

#[derive(Debug, Deserialize)]
struct SpotsFile {
    format: String,
    spots: Vec<Spot>,
}

/// How a typed name was read. A name that is both a state and a town appears
/// as both, and [`Bank::judge`] takes whichever scores better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    State(usize),
    City(usize),
}

/// What a guess was worth.
///
/// `Wrong` covers both a name in the wrong state and a name the bank has never
/// heard of; the game treats neither as an event, because the channel is a
/// social one and reacting to every stray message would be noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Verdict {
    Wrong,
    /// The right state, but the town named is at the far end of it.
    State,
    /// The right state and a town within [`NEAR_KM`].
    Near(f64),
    /// The right state and a town within [`BULLSEYE_KM`].
    Bullseye(f64),
}

impl Verdict {
    pub fn won(self) -> bool {
        !matches!(self, Verdict::Wrong)
    }

    /// What the round pays before a hint is taken off it. The gap between
    /// shouting the state and risking the town is the whole decision the game
    /// asks for, so it is a wide one.
    pub fn worth(self) -> i64 {
        match self {
            Verdict::Wrong => 0,
            Verdict::State => 2,
            Verdict::Near(_) => 4,
            Verdict::Bullseye(_) => 5,
        }
    }

    /// How the reveal describes it.
    pub fn praise(self) -> &'static str {
        match self {
            Verdict::Wrong => "",
            Verdict::State => "right state",
            Verdict::Near(_) => "close",
            Verdict::Bullseye(_) => "bullseye",
        }
    }
}

/// The plain form a name is looked up in: lower case, no punctuation, single
/// spaces. It has to agree with `fold()` in `tools/build.py`, which folds the
/// keys this is compared against.
///
/// Accents are dropped for the Latin range people can actually type. The
/// gazetteer's own diacritics were folded away when it was built, so the only
/// text reaching this is what somebody typed on a phone.
pub fn fold(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for raw in text.trim().chars() {
        let c = deaccent(raw);
        if c == '&' {
            if !space {
                out.push(' ');
            }
            out.push_str("and ");
            space = true;
            continue;
        }
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            space = false;
        } else if !space {
            out.push(' ');
            space = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

/// The accented Latin letters a place name might arrive with, flattened. Any
/// other non-ASCII character falls through and is treated as a separator.
fn deaccent(c: char) -> char {
    match c {
        'à'..='å' | 'À'..='Å' | 'ā' | 'Ā' | 'ă' | 'ǎ' => 'a',
        'è'..='ë' | 'È'..='Ë' | 'ē' | 'Ē' | 'ĕ' => 'e',
        'ì'..='ï' | 'Ì'..='Ï' | 'ī' | 'Ī' | 'ĭ' => 'i',
        'ò'..='ö' | 'Ò'..='Ö' | 'ō' | 'Ō' | 'ŏ' => 'o',
        'ù'..='ü' | 'Ù'..='Ü' | 'ū' | 'Ū' | 'ŭ' => 'u',
        'ñ' | 'Ñ' | 'ṇ' | 'Ṇ' | 'ń' => 'n',
        'ç' | 'Ç' | 'ć' => 'c',
        'ś' | 'Ś' | 'ṣ' | 'Ṣ' | 'š' => 's',
        'ṭ' | 'Ṭ' | 'ť' => 't',
        'ḍ' | 'Ḍ' | 'ḏ' => 'd',
        'ṛ' | 'Ṛ' | 'ṝ' => 'r',
        'ḷ' | 'Ḷ' => 'l',
        'ḥ' | 'Ḥ' => 'h',
        'ṃ' | 'Ṃ' => 'm',
        'ġ' | 'ǧ' => 'g',
        'ý' | 'ÿ' => 'y',
        'ž' | 'ź' => 'z',
        other => other,
    }
}

/// What someone typed, as something to look up — or `None` when the message
/// can't be a place name at all.
pub fn tidy(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_GUESS {
        return None;
    }
    // `!hint` and `!skip` steer the round; they are never guesses.
    if trimmed.starts_with('!') || trimmed.starts_with('/') {
        return None;
    }
    let folded = fold(trimmed);
    (!folded.is_empty()).then_some(folded)
}

/// How far apart two points on the ground are, in kilometres.
pub fn haversine(a_lat: f64, a_lon: f64, b_lat: f64, b_lon: f64) -> f64 {
    const R: f64 = 6371.0;
    let (p1, p2) = (a_lat.to_radians(), b_lat.to_radians());
    let dp = p2 - p1;
    let dl = (b_lon - a_lon).to_radians();
    let h = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * R * h.sqrt().asin()
}

#[derive(Debug)]
pub struct Bank {
    states: Vec<StatePlace>,
    cities: Vec<City>,
    spots: Vec<Spot>,
    state_keys: HashMap<String, usize>,
    city_keys: HashMap<String, usize>,
    /// Where the pictures are, remembered from the load so the game can find
    /// one without knowing the workspace.
    dir: PathBuf,
}

impl Bank {
    pub fn spots(&self) -> &[Spot] {
        &self.spots
    }

    pub fn count(&self) -> usize {
        self.spots.len()
    }

    /// The states the bank can actually set a round in, in play order. The
    /// picture supply is wildly uneven between them, so the game picks a state
    /// from THIS list and then a photo inside it: sampling photos directly
    /// would make Tamil Nadu a third of every match and the game guessable
    /// without looking.
    pub fn playable_states(&self) -> Vec<String> {
        let mut names: Vec<String> = self.spots.iter().map(|s| s.state.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    pub fn spots_in(&self, state: &str) -> Vec<&Spot> {
        self.spots.iter().filter(|s| s.state == state).collect()
    }

    /// Where a photo lives on disk.
    pub fn path(&self, spot: &Spot) -> PathBuf {
        self.dir.join(&spot.file)
    }

    /// Every way a typed name can be read. Empty means the bank has never
    /// heard of it.
    pub fn readings(&self, folded: &str) -> Vec<Named> {
        let mut out = Vec::new();
        if let Some(&i) = self.state_keys.get(folded) {
            out.push(Named::State(i));
        }
        if let Some(&i) = self.city_keys.get(folded) {
            out.push(Named::City(i));
        }
        out
    }

    pub fn state(&self, i: usize) -> &StatePlace {
        &self.states[i]
    }

    pub fn city(&self, i: usize) -> &City {
        &self.cities[i]
    }

    /// What a typed name is worth against this photo.
    ///
    /// The state is a gate: a name belonging to any other state scores nothing,
    /// however near the kilometres come out. Past the gate, a town is scored on
    /// distance, and a town at the far end of a big state still earns the
    /// state's own credit — naming Jaisalmer for a photo in Jaipur is a wrong
    /// town but a right state, and the game says so.
    pub fn judge(&self, spot: &Spot, folded: &str) -> Verdict {
        self.readings(folded)
            .into_iter()
            .map(|reading| self.judge_one(spot, reading))
            .max_by(|a, b| a.worth().cmp(&b.worth()))
            .unwrap_or(Verdict::Wrong)
    }

    fn judge_one(&self, spot: &Spot, reading: Named) -> Verdict {
        match reading {
            Named::State(i) => {
                if self.states[i].name == spot.state {
                    Verdict::State
                } else {
                    Verdict::Wrong
                }
            }
            Named::City(i) => {
                let city = &self.cities[i];
                if city.state != spot.state {
                    return Verdict::Wrong;
                }
                let km = haversine(city.lat, city.lon, spot.lat, spot.lon);
                if km <= BULLSEYE_KM {
                    Verdict::Bullseye(km)
                } else if km <= NEAR_KM {
                    Verdict::Near(km)
                } else {
                    Verdict::State
                }
            }
        }
    }

    /// The answer as the reveal states it: the town, where the photo sits near
    /// one, and the state either way.
    pub fn answer_line(&self, spot: &Spot) -> String {
        match (&spot.city, spot.city_km) {
            (Some(city), Some(km)) if km <= BULLSEYE_KM => format!("**{}**, {}", city, spot.state),
            (Some(city), Some(km)) => format!("**{}** — {:.0} km from {}", spot.state, km, city),
            _ => format!("**{}**", spot.state),
        }
    }

    /// What a hint gives away: the state's first letter and the part of the
    /// country it is in. Enough to narrow the list, not enough to hand it over.
    pub fn hint_line(&self, spot: &Spot) -> String {
        let letter = spot.state.chars().next().unwrap_or('?').to_ascii_uppercase();
        format!("Starts with **{}**, somewhere in the **{}**.", letter, region(spot.lat, spot.lon))
    }
}

/// The rough quarter of the country a point falls in. Coarse on purpose: a
/// hint should shorten the list of states, not pick one.
pub fn region(lat: f64, lon: f64) -> &'static str {
    let north = lat >= 23.5;
    match (north, lon >= 79.0) {
        (true, false) => "north-west",
        (true, true) => "north-east",
        (false, false) => "south-west",
        (false, true) => "south-east",
    }
}

/// Reads both files out of a folder. Pictures the bank names but the folder
/// does not hold are dropped, so a workspace with the json and no images
/// starts empty rather than posting rounds nobody can see.
pub fn load(dir: &Path) -> anyhow::Result<Bank> {
    let places: PlacesFile = serde_json::from_slice(&std::fs::read(dir.join("places.json"))?)?;
    if places.format != PLACES_FORMAT {
        anyhow::bail!("places.json is {}, this build reads {}", places.format, PLACES_FORMAT);
    }
    let spots_file: SpotsFile = serde_json::from_slice(&std::fs::read(dir.join("spots.json"))?)?;
    if spots_file.format != SPOTS_FORMAT {
        anyhow::bail!("spots.json is {}, this build reads {}", spots_file.format, SPOTS_FORMAT);
    }

    let mut state_keys = HashMap::new();
    for (i, state) in places.states.iter().enumerate() {
        for key in &state.keys {
            state_keys.insert(key.clone(), i);
        }
    }
    // The bigger town owns a shared name. build.py already decided that, but a
    // hand-edited bank might not have, so the rule is enforced here too.
    let mut city_keys: HashMap<String, usize> = HashMap::new();
    for (i, city) in places.cities.iter().enumerate() {
        for key in &city.keys {
            match city_keys.get(key) {
                Some(&held) if places.cities[held].pop >= city.pop => {}
                _ => {
                    city_keys.insert(key.clone(), i);
                }
            }
        }
    }

    let before = spots_file.spots.len();
    let spots: Vec<Spot> = spots_file.spots.into_iter().filter(|s| dir.join(&s.file).exists()).collect();
    if spots.len() < before {
        tracing::warn!("geo: {} of {} photos named by the bank are not on disk", before - spots.len(), before);
    }
    if spots.is_empty() {
        anyhow::bail!("no photos on disk");
    }

    Ok(Bank {
        states: places.states,
        cities: places.cities,
        spots,
        state_keys,
        city_keys,
        dir: dir.to_path_buf(),
    })
}

/// Reads `<workspace>/geobank/` once, at start. A bank that isn't there is not
/// an error: the game stays off and says why.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["geobank"]);
    match load(&dir) {
        Ok(bank) => {
            tracing::info!(
                "geo: {} places across {} states read from {}",
                bank.count(),
                bank.playable_states().len(),
                dir.display()
            );
            let _ = BANK.set(bank);
        }
        Err(err) => tracing::warn!(
            "geo: no place bank ({}) — the game stays off. Build geobank/ with geobank/tools/build.py and harvest.py.",
            err
        ),
    }
}

/// The bank, when there is one.
pub fn bank() -> Option<&'static Bank> {
    BANK.get()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A bank small enough to read, with the traps in it: a name that is both a
    /// state and a city, two towns sharing a name where one is far bigger, and
    /// two towns in the same state at opposite ends of it.
    pub fn fixture() -> Bank {
        let states = vec![
            StatePlace { name: "Telangana".into(), lat: 17.8, lon: 79.0, keys: vec!["telangana".into()] },
            StatePlace { name: "Delhi".into(), lat: 28.6, lon: 77.2, keys: vec!["delhi".into(), "new delhi".into()] },
            StatePlace { name: "Rajasthan".into(), lat: 26.9, lon: 74.0, keys: vec!["rajasthan".into()] },
        ];
        let cities = vec![
            City { name: "Hyderabad".into(), state: "Telangana".into(), lat: 17.385, lon: 78.487, pop: 6_993_262, keys: vec!["hyderabad".into()] },
            City { name: "Warangal".into(), state: "Telangana".into(), lat: 17.978, lon: 79.594, pop: 704_570, keys: vec!["warangal".into()] },
            City { name: "Delhi".into(), state: "Delhi".into(), lat: 28.651, lon: 77.217, pop: 10_927_986, keys: vec!["delhi".into(), "new delhi".into()] },
            City { name: "Jaipur".into(), state: "Rajasthan".into(), lat: 26.912, lon: 75.787, pop: 2_711_758, keys: vec!["jaipur".into()] },
            City { name: "Jaisalmer".into(), state: "Rajasthan".into(), lat: 26.915, lon: 70.908, pop: 78_000, keys: vec!["jaisalmer".into()] },
        ];
        let spots = vec![
            Spot {
                id: "kv1".into(), lat: 17.390, lon: 78.490, state: "Telangana".into(),
                city: Some("Hyderabad".into()), city_km: Some(0.6),
                file: "images/kv1.jpg".into(), by: "someone".into(), shot: "2021-03-08".into(),
            },
            Spot {
                id: "kv2".into(), lat: 26.913, lon: 75.803, state: "Rajasthan".into(),
                city: Some("Jaipur".into()), city_km: Some(1.6),
                file: "images/kv2.jpg".into(), by: "someone".into(), shot: "2019-06-06".into(),
            },
        ];
        let mut state_keys = HashMap::new();
        for (i, s) in states.iter().enumerate() {
            for k in &s.keys {
                state_keys.insert(k.clone(), i);
            }
        }
        let mut city_keys = HashMap::new();
        for (i, c) in cities.iter().enumerate() {
            for k in &c.keys {
                city_keys.insert(k.clone(), i);
            }
        }
        Bank { states, cities, spots, state_keys, city_keys, dir: PathBuf::new() }
    }

    #[test]
    fn folding_matches_what_people_type() {
        assert_eq!(fold("  Tamil  Nadu "), "tamil nadu");
        assert_eq!(fold("Jammu & Kashmir"), "jammu and kashmir");
        assert_eq!(fold("J&K"), "j and k");
        assert_eq!(fold("Jammu&Kashmir"), "jammu and kashmir");
        assert_eq!(fold("Puduchérry!"), "puducherry");
        assert_eq!(fold("Bengalūru"), "bengaluru");
        assert_eq!(fold("---"), "");
    }

    #[test]
    fn a_state_is_the_coarse_answer() {
        let bank = fixture();
        let spot = &bank.spots()[0];
        assert_eq!(bank.judge(spot, "telangana"), Verdict::State);
        assert_eq!(bank.judge(spot, "telangana").worth(), 2);
    }

    #[test]
    fn naming_the_town_beats_naming_the_state() {
        let bank = fixture();
        let spot = &bank.spots()[0];
        let verdict = bank.judge(spot, "hyderabad");
        assert!(matches!(verdict, Verdict::Bullseye(_)), "{verdict:?}");
        assert!(verdict.worth() > bank.judge(spot, "telangana").worth());
    }

    /// The whole point of the state gate: kilometres never rescue a guess that
    /// named the wrong state.
    #[test]
    fn the_wrong_state_scores_nothing() {
        let bank = fixture();
        let spot = &bank.spots()[1]; // Jaipur, Rajasthan
        assert_eq!(bank.judge(spot, "delhi"), Verdict::Wrong);
        assert_eq!(bank.judge(spot, "hyderabad"), Verdict::Wrong);
    }

    /// A big state is a big place. Naming its far corner is a wrong town but a
    /// right state, and still earns the state's credit.
    #[test]
    fn the_far_end_of_a_state_still_earns_the_state() {
        let bank = fixture();
        let spot = &bank.spots()[1]; // Jaipur
        assert_eq!(bank.judge(spot, "jaisalmer"), Verdict::State);
    }

    /// Delhi is a union territory and a city. The reading that pays more is the
    /// one that counts.
    #[test]
    fn a_name_that_reads_two_ways_takes_the_better_one() {
        let bank = fixture();
        let spot = Spot {
            id: "kv3".into(), lat: 28.650, lon: 77.220, state: "Delhi".into(),
            city: Some("Delhi".into()), city_km: Some(0.3),
            file: "images/kv3.jpg".into(), by: "x".into(), shot: "2021-12-22".into(),
        };
        assert_eq!(bank.readings("delhi").len(), 2);
        assert!(matches!(bank.judge(&spot, "delhi"), Verdict::Bullseye(_)));
    }

    #[test]
    fn a_name_the_bank_never_heard_of_is_not_a_guess() {
        let bank = fixture();
        assert_eq!(bank.judge(&bank.spots()[0], "banana"), Verdict::Wrong);
        assert!(bank.readings("banana").is_empty());
    }

    #[test]
    fn steering_words_are_never_guesses() {
        assert_eq!(tidy("!hint"), None);
        assert_eq!(tidy("/geo"), None);
        assert_eq!(tidy(""), None);
        assert_eq!(tidy("Tamil Nadu").as_deref(), Some("tamil nadu"));
    }

    #[test]
    fn states_are_listed_once_each_in_play_order() {
        let bank = fixture();
        assert_eq!(bank.playable_states(), vec!["Rajasthan".to_string(), "Telangana".to_string()]);
    }

    #[test]
    fn distance_is_measured_on_the_ground() {
        // Hyderabad to Warangal is about 135 km.
        let km = haversine(17.385, 78.487, 17.978, 79.594);
        assert!((130.0..140.0).contains(&km), "{km}");
    }
}
