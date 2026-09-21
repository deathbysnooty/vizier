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
//! ## Two scopes
//!
//! A round is either an **India** round or a **World** round, and the only
//! thing that changes between them is what the gate is. An India round is
//! gated by the STATE, and a town inside the right one is scored on distance.
//! A World round is gated by the COUNTRY and asks for nothing finer: the room
//! was asked to name the country, so naming the right one - or any town in it,
//! which says the same thing - is the whole answer. The World files are
//! `world.json` and `world_spots.json`, built by `build_world.py` and
//! `harvest_world.py`; without them the bank simply has no World rounds.
//!
//! If the India files are missing the game simply never starts: [`open`] says
//! so once in the log and leaves [`bank`] empty, and every part of the game
//! checks it before doing anything.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

/// The credit KartaView's licence asks for, wherever the photos appear.
pub const ATTRIBUTION: &str = "Street imagery from KartaView contributors and Grab (https://kartaview.org/), used under CC BY-SA 4.0.";

/// The formats this code knows how to read. A later one is not guessed at.
pub const PLACES_FORMAT: &str = "GEOPLACES1";
pub const SPOTS_FORMAT: &str = "GEOSPOTS1";
pub const WORLD_FORMAT: &str = "GEOWORLD1";

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

/// Which kind of round a photo belongs to, and so what its gate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    /// Gated by the state; a town inside it is scored on distance.
    India,
    /// Gated by the country, and nothing finer is asked for.
    World,
}

impl Scope {
    pub fn key(self) -> &'static str {
        match self {
            Scope::India => "india",
            Scope::World => "world",
        }
    }

    pub fn from_key(key: &str) -> Scope {
        if key == "world" { Scope::World } else { Scope::India }
    }
}

/// What a match plays, as the room votes for it at the break.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Pool {
    India,
    World,
    /// India and the world, a round of either drawn at random.
    #[default]
    Mix,
}

impl Pool {
    pub const ALL: [Pool; 3] = [Pool::India, Pool::World, Pool::Mix];

    pub fn key(self) -> &'static str {
        match self {
            Pool::India => "india",
            Pool::World => "world",
            Pool::Mix => "mix",
        }
    }

    pub fn from_key(key: &str) -> Pool {
        Pool::ALL.into_iter().find(|p| p.key() == key).unwrap_or_default()
    }

    /// What the button says.
    pub fn label(self) -> &'static str {
        match self {
            Pool::India => "🇮🇳 India",
            Pool::World => "🌍 World",
            Pool::Mix => "🎲 Mix",
        }
    }

    /// What the match is, said in a sentence.
    pub fn about(self) -> &'static str {
        match self {
            Pool::India => "India — name the state, or the town for more",
            Pool::World => "the World — name the country",
            Pool::Mix => "a Mix — India and the World, round by round",
        }
    }

    /// Which kinds of round the match draws from.
    pub fn scopes(self) -> &'static [Scope] {
        match self {
            Pool::India => &[Scope::India],
            Pool::World => &[Scope::World],
            Pool::Mix => &[Scope::India, Scope::World],
        }
    }
}

/// One country: the gate of a World round.
#[derive(Debug, Clone, Deserialize)]
pub struct Country {
    /// ISO 3166 alpha-2, which is what a World photo's region holds.
    pub code: String,
    pub name: String,
    /// Two letters: AS, EU, AF, NA, SA, OC, AN. What a World hint gives away.
    pub continent: String,
    pub keys: Vec<String>,
}

/// A town somewhere in the world, carrying its country. Naming one in a World
/// round names its country.
#[derive(Debug, Clone, Deserialize)]
pub struct WorldCity {
    pub name: String,
    pub country: String,
    pub pop: i64,
    pub keys: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WorldFile {
    format: String,
    countries: Vec<Country>,
    cities: Vec<WorldCity>,
}

/// One photo, and where on the ground it was taken.
#[derive(Debug, Clone)]
pub struct Spot {
    pub id: String,
    pub lat: f64,
    pub lon: f64,
    pub scope: Scope,
    /// The gate's answer: a state's name for an India photo, a country's code
    /// for a World one.
    pub region: String,
    /// The nearest town in the gazetteer, for the reveal. Not the answer — the
    /// answer is wherever the guess lands.
    pub city: Option<String>,
    pub city_km: Option<f64>,
    /// Where the picture is, relative to the bank folder.
    pub file: String,
    /// The contributor who took it, credited on the reveal. Mapillary's
    /// licence asks for the individual photographer by name, so this is a
    /// licence term and not a courtesy.
    pub by: String,
    /// Which archive it came from: `kartaview` (the default, for the photos
    /// banked before there were two) or `mapillary`.
    pub source: Option<String>,
    pub shot: String,
}

impl Spot {
    pub fn source(&self) -> &str {
        self.source.as_deref().unwrap_or("kartaview")
    }
}

/// A photo as the file holds it. The India file calls its gate `state` and
/// the World file calls it `country`; both become a [`Spot`]'s `region`.
#[derive(Debug, Deserialize)]
struct RawSpot {
    id: String,
    lat: f64,
    lon: f64,
    state: Option<String>,
    country: Option<String>,
    city: Option<String>,
    city_km: Option<f64>,
    file: String,
    by: String,
    source: Option<String>,
    shot: String,
}

impl RawSpot {
    fn into_spot(self, scope: Scope) -> Option<Spot> {
        let region = match scope {
            Scope::India => self.state?,
            Scope::World => self.country?,
        };
        Some(Spot {
            id: self.id,
            lat: self.lat,
            lon: self.lon,
            scope,
            region,
            city: self.city,
            city_km: self.city_km,
            file: self.file,
            by: self.by,
            source: self.source,
            shot: self.shot,
        })
    }
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
    /// The credit the photos' licences ask for. It travels with the bank
    /// rather than being written into the code, because which archives are in
    /// it is a property of the bank somebody built, not of this build.
    attribution: Option<String>,
    spots: Vec<RawSpot>,
}

/// How a typed name was read. A name that is both a state and a town appears
/// as both, and [`Bank::judge`] takes whichever scores better.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    State(usize),
    City(usize),
    Country(usize),
    WorldCity(usize),
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
    /// A World round's whole answer: the right country, named outright or by
    /// naming a town in it.
    Country,
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
            // The same as a state: both are the coarse answer that always
            // scores, and a World round asks for nothing finer.
            Verdict::Country => 2,
        }
    }

    /// How the reveal describes it.
    pub fn praise(self) -> &'static str {
        match self {
            Verdict::Wrong => "",
            Verdict::State => "right state",
            Verdict::Near(_) => "close",
            Verdict::Bullseye(_) => "bullseye",
            Verdict::Country => "right country",
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
    countries: Vec<Country>,
    world_cities: Vec<WorldCity>,
    spots: Vec<Spot>,
    attribution: String,
    state_keys: HashMap<String, usize>,
    city_keys: HashMap<String, usize>,
    country_keys: HashMap<String, usize>,
    world_city_keys: HashMap<String, usize>,
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

    /// The credit that has to go wherever the photos do.
    pub fn attribution(&self) -> &str {
        &self.attribution
    }

    /// The regions a round of this scope can be set in - states for India,
    /// countries for World - in play order. The picture supply is wildly
    /// uneven between them, so the game picks a region from THIS list and
    /// then a photo inside it: sampling photos directly would make Tamil Nadu
    /// a third of every India match and the game guessable without looking.
    pub fn playable(&self, scope: Scope) -> Vec<String> {
        let mut names: Vec<String> = self.spots.iter().filter(|s| s.scope == scope).map(|s| s.region.clone()).collect();
        names.sort();
        names.dedup();
        names
    }

    /// The India regions, for the places that only ever meant India.
    pub fn playable_states(&self) -> Vec<String> {
        self.playable(Scope::India)
    }

    pub fn count_in(&self, scope: Scope) -> usize {
        self.spots.iter().filter(|s| s.scope == scope).count()
    }

    pub fn spots_in(&self, scope: Scope, region: &str) -> Vec<&Spot> {
        self.spots.iter().filter(|s| s.scope == scope && s.region == region).collect()
    }

    /// How a photo's region is said out loud: a state's own name, or a
    /// country's name rather than its code.
    pub fn region_name(&self, spot: &Spot) -> String {
        match spot.scope {
            Scope::India => spot.region.clone(),
            Scope::World => self.countries.iter().find(|c| c.code == spot.region).map(|c| c.name.clone()).unwrap_or_else(|| spot.region.clone()),
        }
    }

    /// Where a photo lives on disk.
    pub fn path(&self, spot: &Spot) -> PathBuf {
        self.dir.join(&spot.file)
    }

    /// Every way a typed name can be read. Empty means the bank has never
    /// heard of it.
    pub fn readings(&self, folded: &str) -> Vec<Named> {
        self.readings_in(Scope::India, folded)
    }

    /// Every way a typed name can be read in a round of this scope. An India
    /// round knows states and Indian towns; a World round knows countries and
    /// the world's towns.
    pub fn readings_in(&self, scope: Scope, folded: &str) -> Vec<Named> {
        let mut out = Vec::new();
        match scope {
            Scope::India => {
                if let Some(&i) = self.state_keys.get(folded) {
                    out.push(Named::State(i));
                }
                if let Some(&i) = self.city_keys.get(folded) {
                    out.push(Named::City(i));
                }
            }
            Scope::World => {
                if let Some(&i) = self.country_keys.get(folded) {
                    out.push(Named::Country(i));
                }
                if let Some(&i) = self.world_city_keys.get(folded) {
                    out.push(Named::WorldCity(i));
                }
            }
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
        self.readings_in(spot.scope, folded)
            .into_iter()
            .map(|reading| self.judge_one(spot, reading))
            .max_by(|a, b| a.worth().cmp(&b.worth()))
            .unwrap_or(Verdict::Wrong)
    }

    fn judge_one(&self, spot: &Spot, reading: Named) -> Verdict {
        match reading {
            Named::State(i) => {
                if self.states[i].name == spot.region {
                    Verdict::State
                } else {
                    Verdict::Wrong
                }
            }
            // A World round asks for the country and nothing finer: the
            // country itself, or a town that is in it, both say it.
            Named::Country(i) => {
                if self.countries[i].code == spot.region {
                    Verdict::Country
                } else {
                    Verdict::Wrong
                }
            }
            Named::WorldCity(i) => {
                if self.world_cities[i].country == spot.region {
                    Verdict::Country
                } else {
                    Verdict::Wrong
                }
            }
            Named::City(i) => {
                let city = &self.cities[i];
                if city.state != spot.region {
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

    /// Who took the photo, as the reveal credits them.
    pub fn credit(&self, spot: &Spot) -> String {
        let archive = if spot.source() == "mapillary" { "Mapillary" } else { "KartaView" };
        format!("📷 {} · {}", spot.by, archive)
    }

    /// The answer as the reveal states it: the town, where the photo sits near
    /// one, and the state either way.
    pub fn answer_line(&self, spot: &Spot) -> String {
        let region = self.region_name(spot);
        match (spot.scope, &spot.city, spot.city_km) {
            // A World round asked for the country, so the country leads; the
            // town is just colour, said only when the photo sits near one.
            (Scope::World, Some(city), Some(km)) if km <= NEAR_KM => format!("**{}** — near {}", region, city),
            (Scope::World, _, _) => format!("**{}**", region),
            (Scope::India, Some(city), Some(km)) if km <= BULLSEYE_KM => format!("**{}**, {}", city, region),
            (Scope::India, Some(city), Some(km)) => format!("**{}** — {:.0} km from {}", region, km, city),
            (Scope::India, _, _) => format!("**{}**", region),
        }
    }

    /// What a hint gives away: the state's first letter and the part of the
    /// country it is in. Enough to narrow the list, not enough to hand it over.
    pub fn hint_line(&self, spot: &Spot) -> String {
        let name = self.region_name(spot);
        let letter = name.chars().next().unwrap_or('?').to_ascii_uppercase();
        match spot.scope {
            Scope::India => format!("Starts with **{}**, somewhere in the **{}**.", letter, region(spot.lat, spot.lon)),
            Scope::World => {
                let continent = self.countries.iter().find(|c| c.code == spot.region).map(|c| continent_name(&c.continent)).unwrap_or("world");
                format!("Starts with **{}**, somewhere in **{}**.", letter, continent)
            }
        }
    }
}

/// A continent code, said the way a hint says it.
pub fn continent_name(code: &str) -> &'static str {
    match code {
        "AS" => "Asia",
        "EU" => "Europe",
        "AF" => "Africa",
        "NA" => "North America",
        "SA" => "South America",
        "OC" => "Oceania",
        "AN" => "Antarctica",
        _ => "the world",
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
    let mut spots: Vec<Spot> =
        spots_file.spots.into_iter().filter_map(|s| s.into_spot(Scope::India)).filter(|s| dir.join(&s.file).exists()).collect();
    if spots.len() < before {
        tracing::warn!("geo: {} of {} photos named by the bank are not on disk", before - spots.len(), before);
    }
    if spots.is_empty() {
        anyhow::bail!("no photos on disk");
    }

    // The World files are optional: without them the bank simply has no World
    // rounds and the vote does not offer one.
    let (countries, world_cities, country_keys, world_city_keys, world_credit) = match load_world(dir) {
        Ok((world, raw, credit)) => {
            let n = raw.len();
            let world_spots: Vec<Spot> =
                raw.into_iter().filter_map(|s| s.into_spot(Scope::World)).filter(|s| dir.join(&s.file).exists()).collect();
            if world_spots.len() < n {
                tracing::warn!("geo: {} of {} World photos are not on disk", n - world_spots.len(), n);
            }
            spots.extend(world_spots);
            let mut country_keys = HashMap::new();
            for (i, c) in world.countries.iter().enumerate() {
                for key in &c.keys {
                    country_keys.insert(key.clone(), i);
                }
            }
            let mut world_city_keys: HashMap<String, usize> = HashMap::new();
            for (i, c) in world.cities.iter().enumerate() {
                for key in &c.keys {
                    match world_city_keys.get(key) {
                        Some(&held) if world.cities[held].pop >= c.pop => {}
                        _ => {
                            world_city_keys.insert(key.clone(), i);
                        }
                    }
                }
            }
            (world.countries, world.cities, country_keys, world_city_keys, credit)
        }
        Err(err) => {
            tracing::info!("geo: no World bank ({}) - India rounds only", err);
            (Vec::new(), Vec::new(), HashMap::new(), HashMap::new(), None)
        }
    };

    // The credit covers every archive whose photos are in play.
    let mut attribution = spots_file.attribution.unwrap_or_else(|| ATTRIBUTION.to_string());
    if let Some(extra) = world_credit.filter(|c| !attribution.contains(c.as_str())) {
        attribution = format!("{} {}", attribution, extra);
    }

    Ok(Bank {
        states: places.states,
        cities: places.cities,
        countries,
        world_cities,
        spots,
        attribution,
        state_keys,
        city_keys,
        country_keys,
        world_city_keys,
        dir: dir.to_path_buf(),
    })
}

/// The World gazetteer and its photos, when both are there.
fn load_world(dir: &Path) -> anyhow::Result<(WorldFile, Vec<RawSpot>, Option<String>)> {
    let world: WorldFile = serde_json::from_slice(&std::fs::read(dir.join("world.json"))?)?;
    if world.format != WORLD_FORMAT {
        anyhow::bail!("world.json is {}, this build reads {}", world.format, WORLD_FORMAT);
    }
    let spots: SpotsFile = serde_json::from_slice(&std::fs::read(dir.join("world_spots.json"))?)?;
    if spots.format != SPOTS_FORMAT {
        anyhow::bail!("world_spots.json is {}, this build reads {}", spots.format, SPOTS_FORMAT);
    }
    Ok((world, spots.spots, spots.attribution))
}

/// Reads `<workspace>/geobank/` once, at start. A bank that isn't there is not
/// an error: the game stays off and says why.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["geobank"]);
    match load(&dir) {
        Ok(bank) => {
            tracing::info!(
                "geo: {} India places across {} states, {} World places across {} countries, read from {}",
                bank.count_in(Scope::India),
                bank.playable(Scope::India).len(),
                bank.count_in(Scope::World),
                bank.playable(Scope::World).len(),
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
                id: "kv1".into(), lat: 17.390, lon: 78.490, scope: Scope::India, region: "Telangana".into(),
                city: Some("Hyderabad".into()), city_km: Some(0.6),
                file: "images/kv1.jpg".into(), by: "someone".into(), source: None, shot: "2021-03-08".into(),
            },
            Spot {
                id: "kv2".into(), lat: 26.913, lon: 75.803, scope: Scope::India, region: "Rajasthan".into(),
                city: Some("Jaipur".into()), city_km: Some(1.6),
                file: "images/kv2.jpg".into(), by: "someone".into(), source: None, shot: "2019-06-06".into(),
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
        let countries = vec![
            Country { code: "JP".into(), name: "Japan".into(), continent: "AS".into(), keys: vec!["japan".into()] },
            Country { code: "BR".into(), name: "Brazil".into(), continent: "SA".into(), keys: vec!["brazil".into()] },
            Country {
                code: "US".into(),
                name: "United States".into(),
                continent: "NA".into(),
                keys: vec!["united states".into(), "usa".into(), "america".into()],
            },
        ];
        let world_cities = vec![
            WorldCity { name: "Tokyo".into(), country: "JP".into(), pop: 9_733_276, keys: vec!["tokyo".into()] },
            WorldCity { name: "Yokohama".into(), country: "JP".into(), pop: 3_574_443, keys: vec!["yokohama".into()] },
            WorldCity { name: "Sao Paulo".into(), country: "BR".into(), pop: 12_400_232, keys: vec!["sao paulo".into()] },
        ];
        let mut spots = spots;
        spots.push(Spot {
            id: "mly1".into(), lat: 35.44, lon: 139.65, scope: Scope::World, region: "JP".into(),
            city: Some("Yokohama".into()), city_km: Some(3.1),
            file: "images/mly1.jpg".into(), by: "someone".into(), source: Some("mapillary".into()), shot: "2025-08-15".into(),
        });
        spots.push(Spot {
            id: "mly2".into(), lat: -23.55, lon: -46.63, scope: Scope::World, region: "BR".into(),
            city: Some("Sao Paulo".into()), city_km: Some(0.4),
            file: "images/mly2.jpg".into(), by: "someone".into(), source: Some("mapillary".into()), shot: "2020-09-05".into(),
        });
        let mut country_keys = HashMap::new();
        for (i, c) in countries.iter().enumerate() {
            for k in &c.keys {
                country_keys.insert(k.clone(), i);
            }
        }
        let mut world_city_keys = HashMap::new();
        for (i, c) in world_cities.iter().enumerate() {
            for k in &c.keys {
                world_city_keys.insert(k.clone(), i);
            }
        }
        Bank {
            states,
            cities,
            countries,
            world_cities,
            spots,
            attribution: ATTRIBUTION.to_string(),
            state_keys,
            city_keys,
            country_keys,
            world_city_keys,
            dir: PathBuf::new(),
        }
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
            id: "kv3".into(), lat: 28.650, lon: 77.220, scope: Scope::India, region: "Delhi".into(),
            city: Some("Delhi".into()), city_km: Some(0.3),
            file: "images/kv3.jpg".into(), by: "x".into(), source: None, shot: "2021-12-22".into(),
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
        assert_eq!(bank.playable(Scope::World), vec!["BR".to_string(), "JP".to_string()]);
    }

    fn world(bank: &Bank, code: &str) -> Spot {
        bank.spots().iter().find(|s| s.scope == Scope::World && s.region == code).expect("a World photo").clone()
    }

    /// A World round asks for the country and nothing finer.
    #[test]
    fn a_world_round_is_won_by_the_country() {
        let bank = fixture();
        let japan = world(&bank, "JP");
        assert_eq!(bank.judge(&japan, "japan"), Verdict::Country);
        assert_eq!(bank.judge(&japan, "brazil"), Verdict::Wrong);
    }

    /// Naming a town names its country - but earns no more than the country,
    /// because the room was asked for the country.
    #[test]
    fn a_town_in_a_world_round_counts_as_its_country() {
        let bank = fixture();
        let japan = world(&bank, "JP");
        assert_eq!(bank.judge(&japan, "tokyo"), Verdict::Country, "Tokyo is in Japan");
        assert_eq!(bank.judge(&japan, "yokohama"), Verdict::Country, "and no bullseye for being right on it");
        assert_eq!(bank.judge(&japan, "sao paulo"), Verdict::Wrong, "a town in another country");
    }

    /// The aliases people actually type.
    #[test]
    fn a_country_answers_to_what_people_call_it() {
        let bank = fixture();
        let us = Spot {
            id: "mly3".into(), lat: 34.05, lon: -118.24, scope: Scope::World, region: "US".into(),
            city: None, city_km: None, file: "images/mly3.jpg".into(), by: "x".into(), source: None, shot: "2017-04-30".into(),
        };
        for name in ["usa", "america", "united states"] {
            assert_eq!(bank.judge(&us, name), Verdict::Country, "{name}");
        }
    }

    /// The two scopes never answer for each other. An Indian state is not a
    /// country, and a country is not an Indian state.
    #[test]
    fn the_scopes_do_not_leak_into_each_other() {
        let bank = fixture();
        let japan = world(&bank, "JP");
        assert_eq!(bank.judge(&japan, "telangana"), Verdict::Wrong);
        assert_eq!(bank.judge(&bank.spots()[0], "japan"), Verdict::Wrong);
    }

    #[test]
    fn a_world_reveal_and_hint_name_the_country_not_its_code() {
        let bank = fixture();
        let japan = world(&bank, "JP");
        assert_eq!(bank.region_name(&japan), "Japan");
        assert_eq!(bank.answer_line(&japan), "**Japan** — near Yokohama");
        assert_eq!(bank.hint_line(&japan), "Starts with **J**, somewhere in **Asia**.");
    }

    /// The bank as it is actually shipped, when the photos are there. It is
    /// skipped rather than failed where they are not: `geobank/images/` is
    /// gitignored, so a fresh clone has the json and none of the pictures.
    #[test]
    fn the_shipped_bank_plays_if_it_is_there() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("geobank");
        let Ok(bank) = load(&dir) else { return };
        assert!(bank.count() >= 100, "only {} places", bank.count());

        let states = bank.playable_states();
        assert!(states.len() >= 5, "only {} states: {:?}", states.len(), states);
        for state in &states {
            // A state too thin to play would come up as often as a full one,
            // because rounds are drawn by state. harvest.py's MIN_SPOTS.
            let n = bank.spots_in(Scope::India, state).len();
            assert!(n >= 20, "{} has only {} places", state, n);
        }
        // And a country for a World round, where the floor is harvest_world.py's.
        for country in bank.playable(Scope::World) {
            let n = bank.spots_in(Scope::World, &country).len();
            assert!(n >= 12, "{} has only {} World places", country, n);
        }

        for spot in bank.spots() {
            // Every photo answers to its own state, or the round is
            // unwinnable. Not always with `State`: Delhi is a union territory
            // AND a city, so naming it can be a bullseye — which is the
            // two-readings rule doing its job on real data.
            let name = bank.region_name(spot);
            let named_state = bank.judge(spot, &fold(&name));
            assert!(named_state.won(), "{} does not answer to {}", spot.id, name);
            assert!(named_state.worth() >= Verdict::State.worth(), "{}: {} scored {:?}", spot.id, name, named_state);
            // And to the town it sits in, where the gazetteer put one nearby.
            if let (Scope::India, Some(city), Some(km)) = (spot.scope, &spot.city, spot.city_km)
                && km <= BULLSEYE_KM
            {
                let verdict = bank.judge(spot, &fold(city));
                assert!(verdict.worth() >= Verdict::Near(0.0).worth(), "{}: {} only scored {:?}", spot.id, city, verdict);
            }
            assert!(dir.join(&spot.file).exists(), "{} is not on disk", spot.file);
        }
    }

    #[test]
    fn distance_is_measured_on_the_ground() {
        // Hyderabad to Warangal is about 135 km.
        let km = haversine(17.385, 78.487, 17.978, 79.594);
        assert!((130.0..140.0).contains(&km), "{km}");
    }
}
