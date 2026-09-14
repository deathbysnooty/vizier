//! What the Chocolate Frog game keeps: the wizards on the cards, the riddle
//! bank, every drop, who tried what, and the cards people own.
//!
//! Everything that decides a catch happens in one SQLite transaction: whether
//! the frog is still open, how many tries someone has left, whether the answer
//! is right, the card's serial number and the collection bonus. The drop row is
//! only flipped to caught `WHERE status = 'open'`, so two right answers landing
//! together can never make two winners. Points are not here: they go through
//! the house ledger afterwards, with a key naming the drop, so a replay can't
//! score twice.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::control;

/// Tries each person gets on one frog.
pub const MAX_TRIES: i64 = 3;
/// The most wizards the panel can hold.
pub const MAX_WIZARDS: usize = 50;
/// Longest wizard name: "Catch <name>" has to fit Discord's 45-character modal title.
pub const MAX_NAME: usize = 38;

// --- rarity -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rarity {
    Common,
    Uncommon,
    Legendary,
}

impl Rarity {
    pub const ALL: [Rarity; 3] = [Rarity::Common, Rarity::Uncommon, Rarity::Legendary];

    pub fn key(self) -> &'static str {
        match self {
            Rarity::Common => "common",
            Rarity::Uncommon => "uncommon",
            Rarity::Legendary => "legendary",
        }
    }

    pub fn from_key(key: &str) -> Option<Rarity> {
        Self::ALL.into_iter().find(|r| r.key().eq_ignore_ascii_case(key.trim()))
    }

    pub fn name(self) -> &'static str {
        match self {
            Rarity::Common => "Common",
            Rarity::Uncommon => "Uncommon",
            Rarity::Legendary => "Legendary",
        }
    }

    /// Milk chocolate, dark chocolate, and the phoenix.
    pub fn emoji(self) -> &'static str {
        match self {
            Rarity::Common => "🥛",
            Rarity::Uncommon => "🍫",
            Rarity::Legendary => "🔥",
        }
    }

    pub fn colour(self) -> u32 {
        match self {
            // Caramel, cocoa, phoenix red-gold.
            Rarity::Common => 0xC68E54,
            Rarity::Uncommon => 0x7B4A2D,
            Rarity::Legendary => 0xE8572A,
        }
    }

    /// Which riddles a frog of this rarity asks.
    pub fn difficulty(self) -> &'static str {
        match self {
            Rarity::Common => "easy",
            Rarity::Uncommon => "medium",
            Rarity::Legendary => "hard",
        }
    }

    /// Points for catching one, read from the settings at the moment of the drop.
    pub fn points(self) -> i64 {
        let points = match self {
            Rarity::Common => control::number("VIZIER_POINTS_FROG_COMMON", 2),
            Rarity::Uncommon => control::number("VIZIER_POINTS_FROG_UNCOMMON", 4),
            Rarity::Legendary => control::number("VIZIER_POINTS_FROG_LEGENDARY", 10),
        };
        points.min(1000) as i64
    }

    /// How often a drop is this rarity, against the other three.
    pub fn weight(self) -> u64 {
        match self {
            Rarity::Common => control::number("VIZIER_FROG_WEIGHT_COMMON", 58),
            Rarity::Uncommon => control::number("VIZIER_FROG_WEIGHT_UNCOMMON", 35),
            Rarity::Legendary => control::number("VIZIER_FROG_WEIGHT_LEGENDARY", 7),
        }
    }
}

/// A rarity from a roll in `[0, 1)`, by weight. Rarities weighted 0 - or with no
/// wizard to show - never come up; `None` when nothing can.
pub fn choose_rarity(roll: f64, weights: &[(Rarity, u64)]) -> Option<Rarity> {
    let total: u64 = weights.iter().map(|(_, w)| *w).sum();
    if total == 0 {
        return None;
    }
    let mut target = ((roll.clamp(0.0, 1.0) * total as f64) as u64).min(total - 1);
    for (rarity, weight) in weights {
        if target < *weight {
            return Some(*rarity);
        }
        target -= *weight;
    }
    weights.iter().rev().find(|(_, w)| *w > 0).map(|(r, _)| *r)
}

/// "No. 0042", wider once the numbers need it.
pub fn serial_label(serial: i64) -> String {
    format!("No. {:04}", serial)
}

/// How a copy is named everywhere: "The Eternal Phoenix #3 · No. 0187".
pub fn card_label(name: &str, edition: i64, serial: i64) -> String {
    format!("{} #{} · {}", name, edition, serial_label(serial))
}

// --- the records ----------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Wizard {
    pub id: i64,
    pub slug: String,
    pub name: String,
    pub rarity: Rarity,
    /// A picture library id, or empty to use `frogcards/<slug>.*` if there is one.
    pub image: String,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Riddle {
    pub id: String,
    pub topic: String,
    pub difficulty: String,
    pub riddle: String,
    /// The canonical answer first.
    pub answers: Vec<String>,
    pub hint: String,
    pub lang: String,
    pub retired: bool,
    pub used: bool,
}

impl Riddle {
    pub fn canonical(&self) -> &str {
        self.answers.first().map(String::as_str).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Made, not yet posted: the message id is still to come.
    Pending,
    Open,
    Caught,
    Escaped,
}

impl Status {
    fn key(self) -> &'static str {
        match self {
            Status::Pending => "pending",
            Status::Open => "open",
            Status::Caught => "caught",
            Status::Escaped => "escaped",
        }
    }

    fn from_key(key: &str) -> Status {
        match key {
            "pending" => Status::Pending,
            "open" => Status::Open,
            "caught" => Status::Caught,
            _ => Status::Escaped,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Drop {
    pub id: i64,
    pub channel: u64,
    pub message: Option<u64>,
    pub wizard_id: i64,
    /// The wizard as they were when the frog dropped, so an edit later doesn't
    /// rewrite a card already in chat.
    pub wizard_name: String,
    pub slug: String,
    pub rarity: Rarity,
    pub points: i64,
    pub riddle_id: String,
    pub dropped_at: i64,
    pub closes_at: i64,
    pub status: Status,
    pub winner: Option<u64>,
    pub winner_name: String,
    pub answer_typed: String,
    pub solved_secs: Option<i64>,
    pub serial: Option<i64>,
    /// Which copy of this card it was: the 3rd Eternal Phoenix ever caught is #3.
    pub edition: Option<i64>,
    /// The message shows how it ended.
    pub finished: bool,
    /// The admin who dropped it from the panel, for a test drop.
    pub by: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Card {
    pub serial: i64,
    /// Copies of this card caught up to and including this one.
    pub edition: i64,
    pub user_id: u64,
    pub wizard_id: i64,
    pub wizard_name: String,
    pub rarity: Rarity,
    pub drop_id: i64,
    pub ts: i64,
}

// --- store ----------------------------------------------------------------------

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
static WORKSPACE: OnceLock<PathBuf> = OnceLock::new();

pub const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS wizards (
        id INTEGER PRIMARY KEY AUTOINCREMENT, slug TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
        rarity TEXT NOT NULL, image TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1,
        position INTEGER NOT NULL DEFAULT 0);
    CREATE TABLE IF NOT EXISTS riddles (
        id TEXT PRIMARY KEY, topic TEXT NOT NULL DEFAULT '', difficulty TEXT NOT NULL, riddle TEXT NOT NULL,
        answers TEXT NOT NULL, hint TEXT NOT NULL DEFAULT '', lang TEXT NOT NULL DEFAULT 'en',
        in_bank INTEGER NOT NULL DEFAULT 1, retired INTEGER NOT NULL DEFAULT 0,
        used INTEGER NOT NULL DEFAULT 0, times_used INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS riddles_pick ON riddles (difficulty, in_bank, retired, used);
    CREATE TABLE IF NOT EXISTS drops (
        id INTEGER PRIMARY KEY AUTOINCREMENT, channel_id INTEGER NOT NULL, message_id INTEGER,
        wizard_id INTEGER NOT NULL, wizard_name TEXT NOT NULL, slug TEXT NOT NULL, rarity TEXT NOT NULL,
        points INTEGER NOT NULL, riddle_id TEXT NOT NULL, dropped_at INTEGER NOT NULL, closes_at INTEGER NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending', winner_id INTEGER, winner_name TEXT NOT NULL DEFAULT '',
        answer_typed TEXT NOT NULL DEFAULT '', solved_secs INTEGER, serial INTEGER, edition INTEGER, caught_at INTEGER,
        finished INTEGER NOT NULL DEFAULT 0, by_user INTEGER);
    CREATE INDEX IF NOT EXISTS drops_status ON drops (status);
    CREATE TABLE IF NOT EXISTS attempts (
        drop_id INTEGER NOT NULL, user_id INTEGER NOT NULL, tries INTEGER NOT NULL, last_ts INTEGER NOT NULL,
        PRIMARY KEY (drop_id, user_id));
    CREATE TABLE IF NOT EXISTS cards (
        serial INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, wizard_id INTEGER NOT NULL,
        edition INTEGER NOT NULL, drop_id INTEGER NOT NULL UNIQUE, ts INTEGER NOT NULL,
        UNIQUE (wizard_id, edition));
    CREATE INDEX IF NOT EXISTS cards_user ON cards (user_id);
    CREATE INDEX IF NOT EXISTS cards_wizard ON cards (wizard_id);
    CREATE TABLE IF NOT EXISTS set_bonus (user_id INTEGER PRIMARY KEY, ts INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);";

/// The owner's ten cards, seeded once. The slugs are the picture file names in
/// `frogcards/`.
const STARTERS: &[(&str, &str, Rarity)] = &[
    ("hagrid", "Rubeus Hagrid", Rarity::Common),
    ("luna", "Luna Lovegood", Rarity::Common),
    ("hermione", "Hermione Granger", Rarity::Common),
    ("sirius", "Sirius Black", Rarity::Common),
    ("bloomweaver", "The Bloomweaver", Rarity::Common),
    ("flamel", "Nicolas Flamel", Rarity::Uncommon),
    ("merlin", "Merlin", Rarity::Uncommon),
    ("moonkeeper", "The Moonkeeper", Rarity::Uncommon),
    ("original-chocolate-frog", "The Original Chocolate Frog", Rarity::Uncommon),
    ("eternal-phoenix", "The Eternal Phoenix", Rarity::Legendary),
];

/// Schema, the starter wizards, and nothing left half-posted.
pub fn init(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(SCHEMA)?;
    if meta_get(conn, "wizards_seeded").is_none() {
        for (i, (slug, name, rarity)) in STARTERS.iter().enumerate() {
            conn.execute(
                "INSERT OR IGNORE INTO wizards (slug, name, rarity, position) VALUES (?1, ?2, ?3, ?4)",
                params![slug, name, rarity.key(), i as i64],
            )?;
        }
        meta_set(conn, "wizards_seeded", "1")?;
    }
    Ok(())
}

/// Opens `.runtime/frog.db` and reads the riddle bank into it.
pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let mut conn = Connection::open(dir.join("frog.db"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
    init(&conn)?;
    let bank = crate::utils::build_path(workspace, &["riddlebank", "ai"]);
    let (loaded, skipped) = import_riddles(&mut conn, &bank)?;
    let playable: i64 =
        conn.query_row("SELECT COUNT(*) FROM riddles WHERE in_bank = 1 AND retired = 0", [], |r| r.get(0))?;
    tracing::info!("frog: {} riddles read from the bank ({} skipped), {} in play", loaded, skipped, playable);
    let _ = WORKSPACE.set(PathBuf::from(workspace));
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

pub fn db() -> Option<&'static Mutex<Connection>> {
    DB.get()
}

pub fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

pub fn meta_set(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
}

// --- wizards --------------------------------------------------------------------

fn wizard_row(r: &rusqlite::Row) -> rusqlite::Result<Wizard> {
    Ok(Wizard {
        id: r.get(0)?,
        slug: r.get(1)?,
        name: r.get(2)?,
        rarity: Rarity::from_key(&r.get::<_, String>(3)?).unwrap_or(Rarity::Common),
        image: r.get(4)?,
        enabled: r.get::<_, i64>(5)? != 0,
    })
}

const WIZARD_COLUMNS: &str = "id, slug, name, rarity, image, enabled";

/// Every wizard: rarest last, then in the order they were added.
pub fn wizards(conn: &Connection) -> Vec<Wizard> {
    let sql = format!(
        "SELECT {} FROM wizards ORDER BY CASE rarity WHEN 'common' THEN 0 WHEN 'uncommon' THEN 1 ELSE 2 END, position, id",
        WIZARD_COLUMNS
    );
    conn.prepare(&sql).and_then(|mut s| s.query_map([], wizard_row)?.collect()).unwrap_or_default()
}

pub fn wizard(conn: &Connection, id: i64) -> Option<Wizard> {
    let sql = format!("SELECT {} FROM wizards WHERE id = ?1", WIZARD_COLUMNS);
    conn.query_row(&sql, params![id], wizard_row).optional().ok().flatten()
}

/// Lower-case ASCII words joined by hyphens: the `frogcards/` file name.
pub fn slugify(name: &str) -> String {
    let folded = super::frog_answer::normalise(name);
    let slug: String = folded.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let slug = slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    if slug.is_empty() { "wizard".to_string() } else { slug }
}

/// A wizard's details as the panel sends them, checked.
pub fn tidy_name(raw: &str) -> Result<String, String> {
    let name = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        return Err("Give the card a name.".into());
    }
    if name.chars().count() > MAX_NAME {
        return Err(format!("Card names can be at most {} characters.", MAX_NAME));
    }
    if name.contains(['<', '>', '@', '*', '_', '`', '|', '~']) {
        return Err("Use letters, spaces and simple punctuation in the name.".into());
    }
    Ok(name)
}

pub fn create_wizard(conn: &Connection, name: &str, rarity: Rarity, image: &str, enabled: bool) -> Result<Wizard, String> {
    let name = tidy_name(name)?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM wizards", [], |r| r.get(0)).map_err(|e| e.to_string())?;
    if count as usize >= MAX_WIZARDS {
        return Err(format!("There are already {} cards.", MAX_WIZARDS));
    }
    if wizards(conn).iter().any(|w| w.name.eq_ignore_ascii_case(&name)) {
        return Err(format!("There's already a card for {}.", name));
    }
    let base = slugify(&name);
    let mut slug = base.clone();
    let mut n = 2;
    while conn
        .query_row("SELECT 1 FROM wizards WHERE slug = ?1", params![slug], |_| Ok(()))
        .optional()
        .map_err(|e| e.to_string())?
        .is_some()
    {
        slug = format!("{}-{}", base, n);
        n += 1;
    }
    conn.execute(
        "INSERT INTO wizards (slug, name, rarity, image, enabled, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![slug, name, rarity.key(), image, enabled as i64, count],
    )
    .map_err(|e| e.to_string())?;
    wizard(conn, conn.last_insert_rowid()).ok_or_else(|| "The card vanished after saving.".to_string())
}

/// Renames, re-rarities, re-pictures or switches a wizard. The slug stays, so a
/// `frogcards/` file keeps working after a rename.
pub fn update_wizard(conn: &Connection, id: i64, name: &str, rarity: Rarity, image: &str, enabled: bool) -> Result<Wizard, String> {
    let name = tidy_name(name)?;
    if wizard(conn, id).is_none() {
        return Err("No such card.".into());
    }
    if wizards(conn).iter().any(|w| w.id != id && w.name.eq_ignore_ascii_case(&name)) {
        return Err(format!("There's already a card for {}.", name));
    }
    conn.execute(
        "UPDATE wizards SET name = ?2, rarity = ?3, image = ?4, enabled = ?5 WHERE id = ?1",
        params![id, name, rarity.key(), image, enabled as i64],
    )
    .map_err(|e| e.to_string())?;
    wizard(conn, id).ok_or_else(|| "No such card.".to_string())
}

/// A random enabled wizard of the rolled rarity. Rarities with nobody enabled
/// are left out of the roll.
pub fn pick_wizard(conn: &Connection, rarity_roll: f64, wizard_roll: f64) -> Option<Wizard> {
    let all: Vec<Wizard> = wizards(conn).into_iter().filter(|w| w.enabled).collect();
    let weights: Vec<(Rarity, u64)> = Rarity::ALL
        .into_iter()
        .map(|r| (r, if all.iter().any(|w| w.rarity == r) { r.weight() } else { 0 }))
        .collect();
    let rarity = choose_rarity(rarity_roll, &weights)?;
    let pool: Vec<&Wizard> = all.iter().filter(|w| w.rarity == rarity).collect();
    let index = ((wizard_roll.clamp(0.0, 1.0) * pool.len() as f64) as usize).min(pool.len().checked_sub(1)?);
    pool.get(index).map(|w| (*w).clone())
}

/// A wizard's card picture file, `{workspace}/frogcards/<slug>.(png|jpg|jpeg|webp)`.
pub fn card_file(workspace: &Path, slug: &str) -> Option<PathBuf> {
    if slug.is_empty() || !slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return None;
    }
    ["png", "jpg", "jpeg", "webp"]
        .iter()
        .map(|ext| workspace.join("frogcards").join(format!("{}.{}", slug, ext)))
        .find(|path| path.is_file())
}

/// Where a wizard's picture comes from: `media`, `file` or none.
pub fn image_source(image: &str, slug: &str) -> Option<&'static str> {
    if !image.is_empty() && control::media::info(image).is_some() {
        return Some("media");
    }
    WORKSPACE.get().and_then(|ws| card_file(ws, slug)).map(|_| "file")
}

/// A wizard's picture as bytes with its file extension: the library picture if
/// one is set, otherwise the `frogcards/` file. Reads control.db, so never call
/// it holding the frog lock's caller's control lock.
pub fn image_bytes(image: &str, slug: &str) -> Option<(Vec<u8>, &'static str)> {
    if !image.is_empty() {
        if let Some((info, bytes)) = control::media::get(image) {
            return Some((bytes, control::media::extension(&info.mime)));
        }
    }
    let path = WORKSPACE.get().and_then(|ws| card_file(ws, slug))?;
    let bytes = std::fs::read(&path).ok()?;
    let ext = match control::media::sniff(&bytes)? {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        _ => return None,
    };
    Some((bytes, ext))
}

/// The longest side of a card picture as it is posted: Discord shows it as a
/// small thumbnail, so the multi-megabyte original never needs to go up.
pub const THUMB_SIZE: u32 = 320;
/// A picture that can't be decoded is still sent as it is, if it is this small.
const PASS_THROUGH_BYTES: usize = 1024 * 1024;

static THUMBS: std::sync::LazyLock<Mutex<HashMap<String, (Vec<u8>, &'static str)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// A picture shrunk to fit [`THUMB_SIZE`]: JPEG when it is fully opaque, PNG when
/// it has see-through parts. `None` if it can't be decoded.
pub fn shrink(bytes: &[u8]) -> Option<(Vec<u8>, &'static str)> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = if img.width() > THUMB_SIZE || img.height() > THUMB_SIZE { img.thumbnail(THUMB_SIZE, THUMB_SIZE) } else { img };
    let see_through = small.color().has_alpha() && small.to_rgba8().pixels().any(|p| p.0[3] < 255);
    let mut out = std::io::Cursor::new(Vec::new());
    if see_through {
        small.write_to(&mut out, image::ImageFormat::Png).ok()?;
        Some((out.into_inner(), "png"))
    } else {
        image::DynamicImage::ImageRgb8(small.to_rgb8()).write_to(&mut out, image::ImageFormat::Jpeg).ok()?;
        Some((out.into_inner(), "jpg"))
    }
}

/// A wizard's picture ready to post: shrunk once, then kept in memory and in
/// `.runtime/frogthumbs/`, named after where the picture came from and when it
/// last changed. Decoding is slow, so call it off the async threads.
pub fn thumbnail(image: &str, slug: &str) -> Option<(Vec<u8>, &'static str)> {
    use sha2::{Digest, Sha256};
    let file = if image.is_empty() || control::media::info(image).is_none() { WORKSPACE.get().and_then(|ws| card_file(ws, slug)) } else { None };
    let identity = match &file {
        Some(path) => {
            let meta = std::fs::metadata(path).ok()?;
            let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            format!("file:{}:{}:{}", path.display(), meta.len(), modified)
        }
        None if !image.is_empty() => format!("media:{}", image),
        None => return None,
    };
    let key: String = Sha256::digest(format!("{}:{}", THUMB_SIZE, identity).as_bytes()).iter().take(10).map(|b| format!("{:02x}", b)).collect();
    if let Some(hit) = THUMBS.lock().get(&key) {
        return Some(hit.clone());
    }
    let dir = WORKSPACE.get().map(|ws| crate::utils::build_path(ws.to_str().unwrap_or("."), &[".runtime", "frogthumbs"]));
    if let Some(dir) = &dir {
        for ext in ["jpg", "png"] {
            if let Ok(bytes) = std::fs::read(dir.join(format!("{}.{}", key, ext))) {
                let found = (bytes, if ext == "jpg" { "jpg" } else { "png" });
                THUMBS.lock().insert(key, found.clone());
                return Some(found);
            }
        }
    }
    let (original, ext) = image_bytes(image, slug)?;
    let made = match shrink(&original) {
        Some(small) => {
            if let Some(dir) = &dir {
                let _ = std::fs::create_dir_all(dir);
                if let Err(err) = std::fs::write(dir.join(format!("{}.{}", key, small.1)), &small.0) {
                    tracing::warn!("frog: picture for {} not cached: {}", slug, err);
                }
            }
            small
        }
        None if original.len() <= PASS_THROUGH_BYTES => (original, ext),
        None => {
            tracing::warn!("frog: the picture for {} can't be read and is too big to post as it is", slug);
            return None;
        }
    };
    let mut cache = THUMBS.lock();
    if cache.len() > 200 {
        cache.clear();
    }
    cache.insert(key, made.clone());
    Some(made)
}

/// Copies owned per wizard.
pub fn copies(conn: &Connection) -> HashMap<i64, i64> {
    conn.prepare("SELECT wizard_id, COUNT(*) FROM cards GROUP BY wizard_id")
        .and_then(|mut s| s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect())
        .unwrap_or_default()
}

// --- riddles ----------------------------------------------------------------------

#[derive(Deserialize)]
struct BankRiddle {
    id: String,
    #[serde(default)]
    topic: String,
    difficulty: String,
    riddle: String,
    answers: Vec<String>,
    #[serde(default)]
    hint: Option<String>,
    #[serde(default)]
    lang: Option<String>,
    #[serde(default)]
    retired: bool,
}

fn jsonl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "jsonl"))
                .filter(|p| !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

/// Reads every `*.jsonl` in the bank folder into the riddles table: new ones
/// added, changed ones updated, the used flags and panel retirements kept.
/// Riddles that have left the bank stop being asked. Returns (loaded, skipped).
pub fn import_riddles(conn: &mut Connection, dir: &Path) -> anyhow::Result<(usize, usize)> {
    let files = jsonl_files(dir);
    if files.is_empty() {
        return Ok((0, 0));
    }
    let tx = conn.transaction()?;
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS frog_seen (id TEXT PRIMARY KEY); DELETE FROM temp.frog_seen;")?;
    let (mut loaded, mut skipped) = (0, 0);
    {
        let mut upsert = tx.prepare(
            "INSERT INTO riddles (id, topic, difficulty, riddle, answers, hint, lang, retired, in_bank)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)
             ON CONFLICT(id) DO UPDATE SET topic = excluded.topic, difficulty = excluded.difficulty,
                 riddle = excluded.riddle, answers = excluded.answers, hint = excluded.hint, lang = excluded.lang,
                 retired = MAX(riddles.retired, excluded.retired), in_bank = 1",
        )?;
        let mut seen = tx.prepare("INSERT OR IGNORE INTO temp.frog_seen (id) VALUES (?1)")?;
        for file in &files {
            for line in std::fs::read_to_string(file)?.lines().filter(|l| !l.trim().is_empty()) {
                let Ok(r) = serde_json::from_str::<BankRiddle>(line) else {
                    skipped += 1;
                    continue;
                };
                let answers: Vec<String> =
                    r.answers.iter().map(|a| a.trim().to_string()).filter(|a| !super::frog_answer::normalise(a).is_empty()).collect();
                let difficulty = r.difficulty.trim().to_ascii_lowercase();
                if r.id.trim().is_empty()
                    || r.riddle.trim().is_empty()
                    || answers.is_empty()
                    || !matches!(difficulty.as_str(), "easy" | "medium" | "hard")
                {
                    skipped += 1;
                    continue;
                }
                upsert.execute(params![
                    r.id.trim(),
                    r.topic,
                    difficulty,
                    r.riddle.trim(),
                    serde_json::to_string(&answers)?,
                    r.hint.unwrap_or_default(),
                    r.lang.unwrap_or_else(|| "en".into()),
                    r.retired as i64
                ])?;
                seen.execute(params![r.id.trim()])?;
                loaded += 1;
            }
        }
    }
    if loaded > 0 {
        tx.execute("UPDATE riddles SET in_bank = (id IN (SELECT id FROM temp.frog_seen))", [])?;
    }
    tx.commit()?;
    Ok((loaded, skipped))
}

fn riddle_row(r: &rusqlite::Row) -> rusqlite::Result<Riddle> {
    let answers: String = r.get(4)?;
    Ok(Riddle {
        id: r.get(0)?,
        topic: r.get(1)?,
        difficulty: r.get(2)?,
        riddle: r.get(3)?,
        answers: serde_json::from_str(&answers).unwrap_or_default(),
        hint: r.get(5)?,
        lang: r.get(6)?,
        retired: r.get::<_, i64>(7)? != 0,
        used: r.get::<_, i64>(8)? != 0,
    })
}

const RIDDLE_COLUMNS: &str = "id, topic, difficulty, riddle, answers, hint, lang, retired, used";

pub fn riddle(conn: &Connection, id: &str) -> Option<Riddle> {
    let sql = format!("SELECT {} FROM riddles WHERE id = ?1", RIDDLE_COLUMNS);
    conn.query_row(&sql, params![id], riddle_row).optional().ok().flatten()
}

/// The difficulties to try, nearest first.
fn nearest(difficulty: &str) -> [&'static str; 3] {
    match difficulty {
        "easy" => ["easy", "medium", "hard"],
        "medium" => ["medium", "easy", "hard"],
        _ => ["hard", "medium", "easy"],
    }
}

/// An unused riddle of the difficulty, marked used. When every riddle of it has
/// been asked the round starts over; a difficulty with none at all hands over
/// to the nearest one that has some.
pub fn pick_riddle(conn: &Connection, difficulty: &str, roll: f64) -> Option<Riddle> {
    for level in nearest(difficulty) {
        let playable = "difficulty = ?1 AND in_bank = 1 AND retired = 0";
        let total: i64 =
            conn.query_row(&format!("SELECT COUNT(*) FROM riddles WHERE {}", playable), params![level], |r| r.get(0)).ok()?;
        if total == 0 {
            continue;
        }
        let mut unused: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM riddles WHERE {} AND used = 0", playable), params![level], |r| r.get(0))
            .ok()?;
        if unused == 0 {
            tracing::info!("frog: every {} riddle has been asked, starting the {} again", level, total);
            conn.execute("UPDATE riddles SET used = 0 WHERE difficulty = ?1", params![level]).ok()?;
            unused = total;
        }
        let offset = ((roll.clamp(0.0, 1.0) * unused as f64) as i64).min(unused - 1);
        let sql = format!("SELECT {} FROM riddles WHERE {} AND used = 0 ORDER BY id LIMIT 1 OFFSET ?2", RIDDLE_COLUMNS, playable);
        let picked = conn.query_row(&sql, params![level, offset], riddle_row).optional().ok()??;
        conn.execute("UPDATE riddles SET used = 1, times_used = times_used + 1 WHERE id = ?1", params![picked.id]).ok()?;
        return Some(Riddle { used: true, ..picked });
    }
    None
}

/// Takes a riddle out of play, or puts it back.
pub fn set_retired(conn: &Connection, id: &str, retired: bool) -> rusqlite::Result<bool> {
    conn.execute("UPDATE riddles SET retired = ?2 WHERE id = ?1", params![id, retired as i64]).map(|n| n > 0)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BankStats {
    pub difficulty: &'static str,
    /// In the bank and not retired.
    pub playable: i64,
    pub unused: i64,
    pub retired: i64,
}

pub fn bank_stats(conn: &Connection) -> Vec<BankStats> {
    ["easy", "medium", "hard"]
        .into_iter()
        .map(|d| {
            let count = |cond: &str| -> i64 {
                conn.query_row(&format!("SELECT COUNT(*) FROM riddles WHERE difficulty = ?1 AND in_bank = 1 AND {}", cond), params![d], |r| r.get(0))
                    .unwrap_or(0)
            };
            BankStats { difficulty: d, playable: count("retired = 0"), unused: count("retired = 0 AND used = 0"), retired: count("retired = 1") }
        })
        .collect()
}

// --- drops ---------------------------------------------------------------------------

const DROP_COLUMNS: &str = "id, channel_id, message_id, wizard_id, wizard_name, slug, rarity, points, riddle_id, dropped_at, \
     closes_at, status, winner_id, winner_name, answer_typed, solved_secs, serial, finished, by_user, edition";

fn drop_row(r: &rusqlite::Row) -> rusqlite::Result<Drop> {
    Ok(Drop {
        id: r.get(0)?,
        channel: r.get::<_, i64>(1)? as u64,
        message: r.get::<_, Option<i64>>(2)?.map(|m| m as u64),
        wizard_id: r.get(3)?,
        wizard_name: r.get(4)?,
        slug: r.get(5)?,
        rarity: Rarity::from_key(&r.get::<_, String>(6)?).unwrap_or(Rarity::Common),
        points: r.get(7)?,
        riddle_id: r.get(8)?,
        dropped_at: r.get(9)?,
        closes_at: r.get(10)?,
        status: Status::from_key(&r.get::<_, String>(11)?),
        winner: r.get::<_, Option<i64>>(12)?.map(|u| u as u64),
        winner_name: r.get(13)?,
        answer_typed: r.get(14)?,
        solved_secs: r.get(15)?,
        serial: r.get(16)?,
        finished: r.get::<_, i64>(17)? != 0,
        by: r.get::<_, Option<i64>>(18)?.map(|u| u as u64),
        edition: r.get(19)?,
    })
}

pub fn get_drop(conn: &Connection, id: i64) -> Option<Drop> {
    let sql = format!("SELECT {} FROM drops WHERE id = ?1", DROP_COLUMNS);
    conn.query_row(&sql, params![id], drop_row).optional().ok().flatten()
}

fn drops_where(conn: &Connection, cond: &str, limit: i64) -> Vec<Drop> {
    let sql = format!("SELECT {} FROM drops WHERE {} ORDER BY id DESC LIMIT ?1", DROP_COLUMNS, cond);
    conn.prepare(&sql).and_then(|mut s| s.query_map(params![limit], drop_row)?.collect()).unwrap_or_default()
}

/// The newest drops that made it into chat.
pub fn recent_drops(conn: &Connection, limit: i64) -> Vec<Drop> {
    drops_where(conn, "status != 'pending'", limit)
}

pub fn open_drops(conn: &Connection) -> Vec<Drop> {
    drops_where(conn, "status = 'open'", 1000)
}

/// Whether a frog is still open in a channel.
pub fn channel_busy(conn: &Connection, channel: u64) -> bool {
    open_drops(conn).iter().any(|d| d.channel == channel)
}

/// Drops that ended but whose message doesn't show it yet.
pub fn unfinished(conn: &Connection) -> Vec<Drop> {
    drops_where(conn, "status IN ('caught', 'escaped') AND finished = 0 AND message_id IS NOT NULL", 100)
}

/// A frog about to be posted: its card, riddle and points fixed now. It only
/// becomes catchable once [`mark_open`] records the message.
pub fn start_drop(conn: &Connection, channel: u64, wizard: &Wizard, riddle: &Riddle, now: i64, by: Option<u64>) -> rusqlite::Result<Drop> {
    conn.execute(
        "INSERT INTO drops (channel_id, wizard_id, wizard_name, slug, rarity, points, riddle_id, dropped_at, closes_at, status, by_user)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, 'pending', ?9)",
        params![
            channel as i64,
            wizard.id,
            wizard.name,
            wizard.slug,
            wizard.rarity.key(),
            wizard.rarity.points(),
            riddle.id,
            now,
            by.map(|b| b as i64)
        ],
    )?;
    get_drop(conn, conn.last_insert_rowid()).ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// The message is in chat: the frog is open until `now + open_secs`.
pub fn mark_open(conn: &Connection, id: i64, message: u64, now: i64, open_secs: i64) -> rusqlite::Result<Option<Drop>> {
    conn.execute(
        "UPDATE drops SET message_id = ?2, status = 'open', dropped_at = ?3, closes_at = ?4 WHERE id = ?1 AND status = 'pending'",
        params![id, message as i64, now, now + open_secs],
    )?;
    Ok(get_drop(conn, id))
}

/// A frog Discord wouldn't take: forgotten, and its riddle back in the pile.
pub fn discard_pending(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    if let Some(d) = get_drop(conn, id).filter(|d| d.status == Status::Pending) {
        conn.execute("UPDATE riddles SET used = 0 WHERE id = ?1", params![d.riddle_id])?;
        conn.execute("DELETE FROM drops WHERE id = ?1", params![id])?;
    }
    Ok(())
}

/// Clears drops a restart caught between being made and posted.
pub fn discard_all_pending(conn: &Connection) -> rusqlite::Result<usize> {
    let ids: Vec<i64> = drops_where(conn, "status = 'pending'", 1000).into_iter().map(|d| d.id).collect();
    for id in &ids {
        discard_pending(conn, *id)?;
    }
    Ok(ids.len())
}

/// Closes an open frog nobody caught. `Some` only when this call closed it.
pub fn escape(conn: &Connection, id: i64) -> rusqlite::Result<Option<Drop>> {
    let changed = conn.execute("UPDATE drops SET status = 'escaped' WHERE id = ?1 AND status = 'open'", params![id])?;
    Ok(if changed == 1 { get_drop(conn, id) } else { None })
}

pub fn mark_finished(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE drops SET finished = 1 WHERE id = ?1", params![id]).map(|_| ())
}

pub fn tries_used(conn: &Connection, drop_id: i64, user: u64) -> i64 {
    conn.query_row("SELECT tries FROM attempts WHERE drop_id = ?1 AND user_id = ?2", params![drop_id, user as i64], |r| r.get(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// How a typed answer went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submit {
    /// No such frog.
    Missing,
    /// Somebody got there first.
    TooLate { winner_name: String },
    /// Nobody got it in time.
    Escaped,
    NoTries,
    Wrong { left: i64 },
    Won(Win),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Win {
    pub drop: Drop,
    pub serial: i64,
    pub edition: i64,
    pub canonical: String,
    /// This catch completed their collection for the first time.
    pub set_complete: bool,
}

/// Checks an answer to an open frog and, if it is right, hands over the card -
/// all in one immediate transaction.
pub fn submit(conn: &mut Connection, drop_id: i64, user: u64, user_name: &str, guess: &str, now: i64) -> rusqlite::Result<Submit> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(drop) = get_drop(&tx, drop_id) else {
        return Ok(Submit::Missing);
    };
    match drop.status {
        Status::Caught => return Ok(Submit::TooLate { winner_name: drop.winner_name }),
        Status::Escaped => return Ok(Submit::Escaped),
        Status::Pending => return Ok(Submit::Missing),
        Status::Open if now >= drop.closes_at => return Ok(Submit::Escaped),
        Status::Open => {}
    }
    let used = tries_used(&tx, drop_id, user);
    if used >= MAX_TRIES {
        return Ok(Submit::NoTries);
    }
    tx.execute(
        "INSERT INTO attempts (drop_id, user_id, tries, last_ts) VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(drop_id, user_id) DO UPDATE SET tries = tries + 1, last_ts = excluded.last_ts",
        params![drop_id, user as i64, now],
    )?;
    let Some(riddle) = riddle(&tx, &drop.riddle_id) else {
        tx.commit()?;
        return Ok(Submit::Wrong { left: MAX_TRIES - used - 1 });
    };
    if !super::frog_answer::is_correct(guess, &riddle.answers) {
        tx.commit()?;
        return Ok(Submit::Wrong { left: MAX_TRIES - used - 1 });
    }
    let serial: i64 = tx.query_row("SELECT COALESCE(MAX(serial), 0) + 1 FROM cards", [], |r| r.get(0))?;
    let edition: i64 =
        tx.query_row("SELECT COALESCE(MAX(edition), 0) + 1 FROM cards WHERE wizard_id = ?1", params![drop.wizard_id], |r| r.get(0))?;
    let typed: String = guess.trim().chars().take(60).collect();
    let name: String = user_name.chars().take(80).collect();
    let won = tx.execute(
        "UPDATE drops SET status = 'caught', winner_id = ?2, winner_name = ?3, answer_typed = ?4, solved_secs = ?5,
             serial = ?6, caught_at = ?7, edition = ?8
         WHERE id = ?1 AND status = 'open'",
        params![drop_id, user as i64, name, typed, (now - drop.dropped_at).max(0), serial, now, edition],
    )?;
    if won != 1 {
        return Ok(Submit::TooLate { winner_name: String::new() });
    }
    tx.execute(
        "INSERT INTO cards (serial, user_id, wizard_id, edition, drop_id, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![serial, user as i64, drop.wizard_id, edition, drop_id, now],
    )?;
    let enabled: i64 = tx.query_row("SELECT COUNT(*) FROM wizards WHERE enabled = 1", [], |r| r.get(0))?;
    let missing: i64 = tx.query_row(
        "SELECT COUNT(*) FROM wizards w WHERE w.enabled = 1
         AND NOT EXISTS (SELECT 1 FROM cards c WHERE c.user_id = ?1 AND c.wizard_id = w.id)",
        params![user as i64],
        |r| r.get(0),
    )?;
    let set_complete = enabled > 0
        && missing == 0
        && tx.execute("INSERT OR IGNORE INTO set_bonus (user_id, ts) VALUES (?1, ?2)", params![user as i64, now])? == 1;
    let drop = get_drop(&tx, drop_id).unwrap_or(drop);
    tx.commit()?;
    Ok(Submit::Won(Win { drop, serial, edition, canonical: riddle.canonical().to_string(), set_complete }))
}

// --- cards ---------------------------------------------------------------------------

fn card_row(r: &rusqlite::Row) -> rusqlite::Result<Card> {
    Ok(Card {
        serial: r.get(0)?,
        user_id: r.get::<_, i64>(1)? as u64,
        wizard_id: r.get(2)?,
        wizard_name: r.get(3)?,
        rarity: Rarity::from_key(&r.get::<_, String>(4)?).unwrap_or(Rarity::Common),
        drop_id: r.get(5)?,
        ts: r.get(6)?,
        edition: r.get(7)?,
    })
}

const CARD_SELECT: &str = "SELECT c.serial, c.user_id, c.wizard_id, COALESCE(w.name, d.wizard_name, '?'), COALESCE(w.rarity, d.rarity, 'common'),
         c.drop_id, c.ts, c.edition
     FROM cards c LEFT JOIN wizards w ON w.id = c.wizard_id LEFT JOIN drops d ON d.id = c.drop_id";

/// A member's cards, newest first.
pub fn cards_of(conn: &Connection, user: u64) -> Vec<Card> {
    conn.prepare(&format!("{} WHERE c.user_id = ?1 ORDER BY c.serial DESC", CARD_SELECT))
        .and_then(|mut s| s.query_map(params![user as i64], card_row)?.collect())
        .unwrap_or_default()
}

/// One copy by its number, with the drop it was won from.
pub fn card_by_serial(conn: &Connection, serial: i64) -> Option<(Card, Drop)> {
    let card = conn.query_row(&format!("{} WHERE c.serial = ?1", CARD_SELECT), params![serial], card_row).optional().ok()??;
    let drop = get_drop(conn, card.drop_id)?;
    Some((card, drop))
}

/// Who owns a card's copies: each owner with their (serial, edition) pairs,
/// most copies first.
pub fn owners_of(conn: &Connection, wizard_id: i64) -> Vec<(u64, Vec<(i64, i64)>)> {
    let rows: Vec<(i64, i64, i64)> = conn
        .prepare("SELECT user_id, serial, edition FROM cards WHERE wizard_id = ?1 ORDER BY serial")
        .and_then(|mut s| s.query_map(params![wizard_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect())
        .unwrap_or_default();
    let mut owners: Vec<(u64, Vec<(i64, i64)>)> = Vec::new();
    for (user, serial, edition) in rows {
        match owners.iter_mut().find(|(u, _)| *u == user as u64) {
            Some((_, copies)) => copies.push((serial, edition)),
            None => owners.push((user as u64, vec![(serial, edition)])),
        }
    }
    owners.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.1[0].cmp(&b.1[0])));
    owners
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub dropped: i64,
    pub caught: i64,
    pub escaped: i64,
    pub open: i64,
    pub cards: i64,
    pub collectors: i64,
    pub full_sets: i64,
}

pub fn totals(conn: &Connection) -> Totals {
    let count = |sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap_or(0) };
    Totals {
        dropped: count("SELECT COUNT(*) FROM drops WHERE status != 'pending'"),
        caught: count("SELECT COUNT(*) FROM drops WHERE status = 'caught'"),
        escaped: count("SELECT COUNT(*) FROM drops WHERE status = 'escaped'"),
        open: count("SELECT COUNT(*) FROM drops WHERE status = 'open'"),
        cards: count("SELECT COUNT(*) FROM cards"),
        collectors: count("SELECT COUNT(DISTINCT user_id) FROM cards"),
        full_sets: count("SELECT COUNT(*) FROM set_bonus"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn memory() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        init(&conn).expect("schema");
        conn
    }

    fn line(id: &str, difficulty: &str, answers: &[&str]) -> String {
        serde_json::json!({ "id": id, "topic": "t", "difficulty": difficulty, "riddle": format!("Riddle {id}?"), "answers": answers, "lang": "en" })
            .to_string()
    }

    pub fn bank(dir: &Path, lines: &[String]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("a.jsonl"), lines.join("\n")).unwrap();
    }

    pub fn riddle_for(conn: &mut Connection, id: &str, answers: &[&str]) -> Riddle {
        let dir = tempfile::tempdir().unwrap();
        let mut existing: Vec<String> = conn
            .prepare("SELECT id, difficulty, answers FROM riddles")
            .unwrap()
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .unwrap()
            .flatten()
            .map(|(i, d, a)| {
                let a: Vec<String> = serde_json::from_str(&a).unwrap();
                serde_json::json!({ "id": i, "difficulty": d, "riddle": "x?", "answers": a }).to_string()
            })
            .collect();
        existing.push(line(id, "easy", answers));
        bank(dir.path(), &existing);
        import_riddles(conn, dir.path()).unwrap();
        riddle(conn, id).unwrap()
    }

    fn open_frog(conn: &mut Connection, answers: &[&str], wizard_id: i64, now: i64) -> Drop {
        let riddle = riddle_for(conn, &format!("r-{}-{}", wizard_id, now), answers);
        let w = wizard(conn, wizard_id).unwrap();
        let d = start_drop(conn, 77, &w, &riddle, now, None).unwrap();
        mark_open(conn, d.id, 9_000 + d.id as u64, now, 300).unwrap().unwrap()
    }

    const NOW: i64 = 1_789_367_400;

    #[test]
    fn the_ten_cards_are_seeded_once_with_their_rarities() {
        let conn = memory();
        let all = wizards(&conn);
        assert_eq!(all.len(), 10);
        let count = |r: Rarity| all.iter().filter(|w| w.rarity == r).count();
        assert_eq!((count(Rarity::Common), count(Rarity::Uncommon), count(Rarity::Legendary)), (5, 4, 1));
        assert_eq!((all[0].name.as_str(), all[0].slug.as_str()), ("Rubeus Hagrid", "hagrid"));
        assert_eq!((all[9].name.as_str(), all[9].slug.as_str(), all[9].rarity), ("The Eternal Phoenix", "eternal-phoenix", Rarity::Legendary));
        assert!(all.iter().all(|w| w.enabled && w.image.is_empty()));
        // A second start doesn't re-seed a card the panel removed.
        conn.execute("DELETE FROM wizards WHERE slug = 'merlin'", []).unwrap();
        init(&conn).unwrap();
        assert_eq!(wizards(&conn).len(), 9);
        assert_eq!([Rarity::Common.points(), Rarity::Uncommon.points(), Rarity::Legendary.points()], [2, 4, 10]);
        assert_eq!([Rarity::Common.difficulty(), Rarity::Uncommon.difficulty(), Rarity::Legendary.difficulty()], ["easy", "medium", "hard"]);
        assert_eq!([Rarity::Common.emoji(), Rarity::Uncommon.emoji(), Rarity::Legendary.emoji()], ["🥛", "🍫", "🔥"]);
    }

    #[test]
    fn rarity_follows_the_weights() {
        let weights: Vec<(Rarity, u64)> = Rarity::ALL.into_iter().map(|r| (r, r.weight())).collect();
        assert_eq!(weights.iter().map(|w| w.1).collect::<Vec<_>>(), vec![58, 35, 7]);
        let mut counts: HashMap<Rarity, usize> = HashMap::new();
        for i in 0..10_000 {
            *counts.entry(choose_rarity(i as f64 / 10_000.0, &weights).unwrap()).or_default() += 1;
        }
        let near = |got: usize, want: usize| got.abs_diff(want) <= 2;
        assert!(near(counts[&Rarity::Common], 5800) && near(counts[&Rarity::Uncommon], 3500) && near(counts[&Rarity::Legendary], 700), "{counts:?}");
        assert_eq!(choose_rarity(1.0, &weights), Some(Rarity::Legendary));
        assert_eq!(choose_rarity(0.0, &weights), Some(Rarity::Common));
        // A zero weight never comes up; all zero is nothing.
        let no_uncommon = [(Rarity::Common, 1), (Rarity::Uncommon, 0), (Rarity::Legendary, 1)];
        for i in 0..100 {
            assert_ne!(choose_rarity(i as f64 / 100.0, &no_uncommon), Some(Rarity::Uncommon));
        }
        assert_eq!(choose_rarity(0.999, &no_uncommon), Some(Rarity::Legendary));
        assert_eq!(choose_rarity(0.5, &[(Rarity::Common, 0)]), None);
        assert_eq!(choose_rarity(0.5, &[]), None);
    }

    #[test]
    fn a_rarity_nobody_is_enabled_for_is_skipped() {
        let conn = memory();
        conn.execute("UPDATE wizards SET enabled = 0 WHERE rarity = 'legendary'", []).unwrap();
        for i in 0..200 {
            let w = pick_wizard(&conn, i as f64 / 200.0, 0.7).unwrap();
            assert_ne!(w.rarity, Rarity::Legendary);
            assert!(w.enabled);
        }
        // The very top of the roll, which would have been Legendary, lands on Uncommon.
        assert_eq!(pick_wizard(&conn, 0.999, 0.0).unwrap().rarity, Rarity::Uncommon);
        conn.execute("UPDATE wizards SET enabled = 0", []).unwrap();
        assert!(pick_wizard(&conn, 0.1, 0.1).is_none());
    }

    #[test]
    fn pictures_are_shrunk_for_posting() {
        let mut big = image::RgbImage::new(1254, 1254);
        let mut noise: u32 = 0x9e37_79b9;
        for p in big.pixels_mut() {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            *p = image::Rgb([noise as u8, (noise >> 8) as u8, (noise >> 16) as u8]);
        }
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(big).write_to(&mut png, image::ImageFormat::Png).unwrap();
        let png = png.into_inner();
        let (small, ext) = shrink(&png).unwrap();
        assert_eq!(ext, "jpg");
        assert!(small.len() < png.len() / 3, "{} vs {}", small.len(), png.len());
        let back = image::load_from_memory(&small).unwrap();
        assert_eq!((back.width(), back.height()), (THUMB_SIZE, THUMB_SIZE));
        // See-through corners stay see-through.
        let mut clear = image::RgbaImage::from_pixel(600, 300, image::Rgba([200, 100, 50, 255]));
        clear.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(clear).write_to(&mut out, image::ImageFormat::Png).unwrap();
        let (small, ext) = shrink(&out.into_inner()).unwrap();
        assert_eq!(ext, "png");
        let back = image::load_from_memory(&small).unwrap();
        assert_eq!((back.width(), back.height()), (THUMB_SIZE, 160));
        assert!(shrink(b"not a picture").is_none());
    }

    #[test]
    fn wizards_are_made_and_edited_with_checks() {
        let conn = memory();
        let dobby = create_wizard(&conn, "  Dobby   the Elf ", Rarity::Uncommon, "", true).unwrap();
        assert_eq!((dobby.name.as_str(), dobby.slug.as_str(), dobby.rarity), ("Dobby the Elf", "dobby-the-elf", Rarity::Uncommon));
        assert!(create_wizard(&conn, "dobby the elf", Rarity::Common, "", true).unwrap_err().contains("already a card"));
        assert!(create_wizard(&conn, "", Rarity::Common, "", true).is_err());
        assert!(create_wizard(&conn, &"x".repeat(MAX_NAME + 1), Rarity::Common, "", true).is_err());
        assert!(create_wizard(&conn, "<@123>", Rarity::Common, "", true).is_err());
        let renamed = update_wizard(&conn, dobby.id, "Dobby", Rarity::Legendary, "abcdef0123456789", false).unwrap();
        assert_eq!((renamed.name.as_str(), renamed.slug.as_str(), renamed.rarity, renamed.enabled), ("Dobby", "dobby-the-elf", Rarity::Legendary, false));
        assert!(update_wizard(&conn, dobby.id, "Merlin", Rarity::Common, "", true).is_err());
        assert!(update_wizard(&conn, 9999, "Nobody", Rarity::Common, "", true).is_err());
        assert_eq!(slugify("Minerva McGonagall"), "minerva-mcgonagall");
        assert_eq!(slugify("Gellért  Grindelwald!"), "gellert-grindelwald");
        assert_eq!(slugify("!!"), "wizard");
    }

    #[test]
    fn card_files_are_found_by_slug_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("frogcards")).unwrap();
        std::fs::write(dir.path().join("frogcards/luna-lovegood.webp"), b"RIFF").unwrap();
        assert_eq!(card_file(dir.path(), "luna-lovegood"), Some(dir.path().join("frogcards/luna-lovegood.webp")));
        assert_eq!(card_file(dir.path(), "merlin"), None);
        assert_eq!(card_file(dir.path(), "../frogcards/luna-lovegood"), None);
        assert_eq!(card_file(dir.path(), ""), None);
    }

    #[test]
    fn serials_read_as_card_numbers() {
        assert_eq!(serial_label(42), "No. 0042");
        assert_eq!(serial_label(1), "No. 0001");
        assert_eq!(serial_label(12345), "No. 12345");
    }

    #[test]
    fn the_bank_imports_idempotently_and_keeps_usage() {
        let mut conn = memory();
        let dir = tempfile::tempdir().unwrap();
        let mut lines = vec![
            line("e-1", "easy", &["keyboard", "computer keyboard"]),
            line("e-2", "Easy", &["candle"]),
            line("m-1", "medium", &["echo"]),
            line("h-1", "hard", &["shadow"]),
            "not json".to_string(),
            line("bad-1", "impossible", &["x"]),
            line("bad-2", "easy", &[]),
            line("bad-3", "easy", &["?!"]),
        ];
        bank(dir.path(), &lines);
        assert_eq!(import_riddles(&mut conn, dir.path()).unwrap(), (4, 4));
        assert_eq!(import_riddles(&mut conn, dir.path()).unwrap(), (4, 4));
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM riddles", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 4, "a second start adds nothing");
        assert_eq!(riddle(&conn, "e-2").unwrap().difficulty, "easy");
        assert_eq!(riddle(&conn, "e-1").unwrap().canonical(), "keyboard");

        // Usage and a panel retirement survive a re-import, and an edit lands.
        let picked = pick_riddle(&conn, "medium", 0.0).unwrap();
        assert_eq!(picked.id, "m-1");
        set_retired(&conn, "h-1", true).unwrap();
        lines[0] = line("e-1", "easy", &["keyboard", "keybaord"]);
        lines.push(serde_json::json!({ "id": "e-3", "difficulty": "easy", "riddle": "Gone?", "answers": ["x"], "retired": true }).to_string());
        bank(dir.path(), &lines);
        import_riddles(&mut conn, dir.path()).unwrap();
        assert!(riddle(&conn, "m-1").unwrap().used);
        assert!(riddle(&conn, "h-1").unwrap().retired, "a re-import doesn't un-retire");
        assert!(riddle(&conn, "e-3").unwrap().retired, "the bank can retire one too");
        assert_eq!(riddle(&conn, "e-1").unwrap().answers, vec!["keyboard", "keybaord"]);

        // One that left the bank stops being asked; an empty folder changes nothing.
        bank(dir.path(), &lines[1..].to_vec());
        import_riddles(&mut conn, dir.path()).unwrap();
        let stats = bank_stats(&conn);
        assert_eq!(stats[0], BankStats { difficulty: "easy", playable: 1, unused: 1, retired: 1 });
        assert_eq!(stats[2], BankStats { difficulty: "hard", playable: 0, unused: 0, retired: 1 });
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(import_riddles(&mut conn, empty.path()).unwrap(), (0, 0));
        assert_eq!(bank_stats(&conn)[0].playable, 1);
    }

    #[test]
    fn the_shipped_bank_loads_whole_and_every_answer_matches_itself() {
        let mut conn = memory();
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("riddlebank/ai");
        let (loaded, skipped) = import_riddles(&mut conn, &dir).unwrap();
        assert_eq!((loaded, skipped), (1000, 0));
        let stats = bank_stats(&conn);
        assert!(stats.iter().all(|s| s.playable > 100), "{stats:?}");
        let all: Vec<Riddle> = conn
            .prepare(&format!("SELECT {} FROM riddles", RIDDLE_COLUMNS))
            .unwrap()
            .query_map([], riddle_row)
            .unwrap()
            .flatten()
            .collect();
        for r in &all {
            assert!(r.riddle.chars().count() <= 300, "{} is long for the pop-up", r.id);
            for answer in &r.answers {
                assert!(super::super::frog_answer::is_correct(answer, &r.answers), "{}: {:?} doesn't match itself", r.id, answer);
            }
        }
    }

    #[test]
    fn riddles_are_used_up_then_start_over_and_borrow_from_the_nearest() {
        let mut conn = memory();
        let dir = tempfile::tempdir().unwrap();
        bank(dir.path(), &[line("e-1", "easy", &["a1"]), line("e-2", "easy", &["a2"]), line("e-3", "easy", &["a3"]), line("m-1", "medium", &["b"])]);
        import_riddles(&mut conn, dir.path()).unwrap();
        let mut seen = std::collections::HashSet::new();
        for roll in [0.9, 0.9, 0.9] {
            let r = pick_riddle(&conn, "easy", roll).unwrap();
            assert_eq!(r.difficulty, "easy");
            assert!(seen.insert(r.id.clone()), "{} came up twice before the rest", r.id);
        }
        assert_eq!(bank_stats(&conn)[0].unused, 0);
        // All three asked: they start over.
        let again = pick_riddle(&conn, "easy", 0.0).unwrap();
        assert_eq!(again.difficulty, "easy");
        assert_eq!(bank_stats(&conn)[0].unused, 2);
        // No hard riddles: the nearest, medium, steps in; then easy.
        assert_eq!(pick_riddle(&conn, "hard", 0.5).unwrap().id, "m-1");
        set_retired(&conn, "m-1", true).unwrap();
        assert_eq!(pick_riddle(&conn, "hard", 0.5).unwrap().difficulty, "easy");
        assert_eq!(pick_riddle(&conn, "medium", 0.5).unwrap().difficulty, "easy");
        let empty = memory();
        assert!(pick_riddle(&empty, "easy", 0.5).is_none());
    }

    #[test]
    fn a_wrong_answer_uses_a_try_and_three_is_the_limit() {
        let mut conn = memory();
        let d = open_frog(&mut conn, &["keyboard"], 3, NOW);
        assert_eq!(submit(&mut conn, d.id, 5, "Rohan", "piano", NOW + 5).unwrap(), Submit::Wrong { left: 2 });
        assert_eq!(submit(&mut conn, d.id, 5, "Rohan", "typewriter", NOW + 6).unwrap(), Submit::Wrong { left: 1 });
        assert_eq!(submit(&mut conn, d.id, 5, "Rohan", "mouse", NOW + 7).unwrap(), Submit::Wrong { left: 0 });
        assert_eq!(submit(&mut conn, d.id, 5, "Rohan", "keyboard", NOW + 8).unwrap(), Submit::NoTries, "even a right answer after three");
        assert_eq!(tries_used(&conn, d.id, 5), 3);
        // Someone else still has all three.
        assert_eq!(submit(&mut conn, d.id, 6, "Zoya", "piano", NOW + 9).unwrap(), Submit::Wrong { left: 2 });
        assert_eq!(get_drop(&conn, d.id).unwrap().status, Status::Open);
    }

    #[test]
    fn a_right_answer_takes_the_card_and_closes_the_frog() {
        let mut conn = memory();
        let d = open_frog(&mut conn, &["keyboard", "computer keyboard"], 3, NOW);
        assert_eq!(submit(&mut conn, d.id, 5, "Rohan", "piano", NOW + 10).unwrap(), Submit::Wrong { left: 2 });
        let Submit::Won(win) = submit(&mut conn, d.id, 6, "Aarav", "The Keybord", NOW + 48).unwrap() else { panic!("should win") };
        assert_eq!((win.serial, win.edition, win.canonical.as_str(), win.set_complete), (1, 1, "keyboard", false));
        assert_eq!(win.drop.edition, Some(1));
        assert_eq!(win.drop.status, Status::Caught);
        assert_eq!((win.drop.winner, win.drop.winner_name.as_str(), win.drop.solved_secs), (Some(6), "Aarav", Some(48)));
        assert_eq!(win.drop.answer_typed, "The Keybord");
        assert_eq!(win.drop.serial, Some(1));
        // After that: too late for everyone, the winner included.
        assert_eq!(submit(&mut conn, d.id, 5, "Rohan", "keyboard", NOW + 50).unwrap(), Submit::TooLate { winner_name: "Aarav".into() });
        assert_eq!(submit(&mut conn, d.id, 6, "Aarav", "keyboard", NOW + 50).unwrap(), Submit::TooLate { winner_name: "Aarav".into() });
        let cards = cards_of(&conn, 6);
        assert_eq!(cards.len(), 1);
        assert_eq!((cards[0].serial, cards[0].wizard_name.as_str(), cards[0].rarity), (1, "Hermione Granger", Rarity::Common));
        assert_eq!(submit(&mut conn, 999, 6, "Aarav", "x", NOW).unwrap(), Submit::Missing);
    }

    #[test]
    fn serials_count_up_across_every_card_and_editions_per_card() {
        let mut conn = memory();
        let mut numbers = Vec::new();
        for (i, wizard) in [1, 2, 1, 5, 1].into_iter().enumerate() {
            let d = open_frog(&mut conn, &["candle"], wizard, NOW + i as i64 * 1000);
            let Submit::Won(win) = submit(&mut conn, d.id, 40 + i as u64 % 2, "x", "candle", NOW + i as i64 * 1000 + 3).unwrap() else { panic!() };
            numbers.push((win.serial, win.edition));
        }
        assert_eq!(numbers, vec![(1, 1), (2, 1), (3, 2), (4, 1), (5, 3)]);
        assert_eq!(cards_of(&conn, 40).iter().map(|c| (c.serial, c.edition)).collect::<Vec<_>>(), vec![(5, 3), (3, 2), (1, 1)], "newest first");
        assert_eq!(owners_of(&conn, 1), vec![(40, vec![(1, 1), (3, 2), (5, 3)])]);
        let (card, drop) = card_by_serial(&conn, 3).unwrap();
        assert_eq!((card.wizard_name.as_str(), card.edition, drop.edition, drop.serial), ("Rubeus Hagrid", 2, Some(2), Some(3)));
        assert!(card_by_serial(&conn, 999).is_none());
        assert_eq!(card_label("The Eternal Phoenix", 3, 187), "The Eternal Phoenix #3 · No. 0187");
        assert_eq!(copies(&conn)[&1], 3);
        let t = totals(&conn);
        assert_eq!((t.dropped, t.caught, t.cards, t.collectors), (5, 5, 5, 2));
    }

    #[test]
    fn a_frog_past_its_time_or_escaped_takes_no_answers() {
        let mut conn = memory();
        let d = open_frog(&mut conn, &["echo"], 1, NOW);
        assert_eq!(submit(&mut conn, d.id, 5, "R", "echo", NOW + 300).unwrap(), Submit::Escaped, "the timer may not have fired yet");
        assert_eq!(tries_used(&conn, d.id, 5), 0, "a late answer doesn't use a try");
        let escaped = escape(&conn, d.id).unwrap().unwrap();
        assert_eq!(escaped.status, Status::Escaped);
        assert!(escape(&conn, d.id).unwrap().is_none(), "only closed once");
        assert_eq!(submit(&mut conn, d.id, 5, "R", "echo", NOW + 10).unwrap(), Submit::Escaped);
        // A caught frog can't escape.
        let d2 = open_frog(&mut conn, &["echo"], 1, NOW + 5000);
        assert!(matches!(submit(&mut conn, d2.id, 5, "R", "echo", NOW + 5001).unwrap(), Submit::Won(_)));
        assert!(escape(&conn, d2.id).unwrap().is_none());
        assert_eq!(unfinished(&conn).len(), 2);
        mark_finished(&conn, d.id).unwrap();
        assert_eq!(unfinished(&conn).len(), 1);
    }

    #[test]
    fn a_pending_frog_is_not_catchable_and_can_be_discarded() {
        let mut conn = memory();
        let picked = riddle_for(&mut conn, "p-1", &["echo"]);
        let w = wizard(&conn, 1).unwrap();
        let d = start_drop(&conn, 5, &w, &picked, NOW, Some(1001)).unwrap();
        conn.execute("UPDATE riddles SET used = 1 WHERE id = 'p-1'", []).unwrap();
        assert_eq!(d.status, Status::Pending);
        assert_eq!(d.by, Some(1001));
        assert_eq!(submit(&mut conn, d.id, 5, "R", "echo", NOW).unwrap(), Submit::Missing);
        assert!(recent_drops(&conn, 10).is_empty());
        assert_eq!(discard_all_pending(&conn).unwrap(), 1);
        assert!(get_drop(&conn, d.id).is_none());
        assert!(!riddle(&conn, "p-1").unwrap().used, "its riddle goes back in the pile");
    }

    #[test]
    fn exactly_one_winner_when_right_answers_race() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frog.db");
        let drop_id = {
            let mut conn = Connection::open(&path).unwrap();
            conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
            init(&conn).unwrap();
            open_frog(&mut conn, &["shadow"], 8, NOW).id
        };
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8u64)
            .map(|user| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut conn = Connection::open(&path).unwrap();
                    conn.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
                    barrier.wait();
                    submit(&mut conn, drop_id, 100 + user, &format!("p{user}"), "shadow", NOW + 20).unwrap()
                })
            })
            .collect();
        let results: Vec<Submit> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let winners = results.iter().filter(|r| matches!(r, Submit::Won(_))).count();
        assert_eq!(winners, 1, "{results:?}");
        assert!(results.iter().all(|r| matches!(r, Submit::Won(_) | Submit::TooLate { .. })));
        let conn = Connection::open(&path).unwrap();
        let cards: i64 = conn.query_row("SELECT COUNT(*) FROM cards", [], |r| r.get(0)).unwrap();
        assert_eq!(cards, 1);
    }

    #[test]
    fn the_collection_bonus_comes_once_when_the_set_is_first_complete() {
        let mut conn = memory();
        // Only three wizards in play, to keep it short.
        conn.execute("UPDATE wizards SET enabled = 0 WHERE id > 3", []).unwrap();
        let mut completes = Vec::new();
        for (i, wizard) in [1, 2, 2, 3, 1, 3].into_iter().enumerate() {
            let at = NOW + i as i64 * 1000;
            let d = open_frog(&mut conn, &["candle"], wizard, at);
            let Submit::Won(win) = submit(&mut conn, d.id, 7, "Kabir", "candle", at + 1).unwrap() else { panic!() };
            completes.push(win.set_complete);
        }
        assert_eq!(completes, vec![false, false, false, true, false, false]);
        // A new wizard: completing again later pays nothing more.
        conn.execute("UPDATE wizards SET enabled = 1 WHERE id = 4", []).unwrap();
        let d = open_frog(&mut conn, &["candle"], 4, NOW + 90_000);
        let Submit::Won(win) = submit(&mut conn, d.id, 7, "Kabir", "candle", NOW + 90_001).unwrap() else { panic!() };
        assert!(!win.set_complete);
        assert_eq!(totals(&conn).full_sets, 1);
    }
}
