//! The egg week: who is hungry for what, and what that does to a point.
//!
//! Everybody on the sign-up sheet wakes up on the first morning of the month
//! with a dragon egg. It is hungry, and it is picky: at any moment it wants
//! exactly two of the seven games and nothing else. Play one of those and it
//! eats; play any of the others and you have had a perfectly good game that
//! paid the month nothing. The hunger moves every few hours, and by the end of
//! the first day every egg has wanted every game at least once.
//!
//! WHY NOT RANDOM. A random pick per call is three bad things at once: it can't
//! be shown in advance (so `/egg` could never say "and at six it wants the
//! quiz"), it can't be checked afterwards (so "my answer should have counted"
//! has no answer), and it clumps - half the server can end up hungry for the
//! same game while nobody is playing the other six. So the hunger is a *seat*
//! and a *slot* put through one line of arithmetic:
//!
//! ```text
//! game[j] = (seat + slot * cravings + j) mod games        for j in 0..cravings
//! ```
//!
//! Seats are handed out in order of claiming, one each, and never reused. Two
//! things fall straight out of that line, and both are tested:
//!
//! * **Everybody covers everything.** The window walks forward by `cravings`
//!   each slot, and 2 and 7 share no factor, so after four slots - one day - a
//!   member has been hungry for all seven. Over a week, four times over.
//! * **Every game has a crowd.** At any one slot the seats are spread evenly
//!   around the seven games, so each game has between a seventh and two
//!   sevenths of the server hungry for it. Nobody is ever playing alone.
//!
//! What it does to a point, in one table:
//!
//! | when | hungry game | anything else |
//! |---|---|---|
//! | egg week | pays as usual | pays the month nothing |
//! | after the hatch | pays **double** | pays as usual |
//!
//! The double goes towards the same daily ceiling as everything else (see
//! [`super::points::Group`]), so it buys speed and never a bigger number. And
//! none of this touches a game's OWN score: every board, every streak and every
//! `/anagramtop` is unchanged all month.

use chrono::Utc;
use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateCommand, CreateCommandOption,
    CreateInteractionResponse, CreateInteractionResponseMessage,
};

use super::egg_store::{self as store, Egg};
use super::month;
use super::points::{self as ledger, Source};
use super::raven;

/// The seven games the month is played with, in the order the rotation walks
/// them. The quick three and the thinking four, interleaved, so two games that
/// are hungry at the same time are rarely both the sitting-down kind.
pub const GAMES: [Source; 7] =
    [Source::Anagram, Source::Quiz, Source::Cat, Source::Koto, Source::Guess, Source::Geo, Source::Movie];

/// Whether a source is one of the month's games at all.
pub fn is_game(source: Source) -> bool {
    GAMES.contains(&source)
}

// --- the clock ---------------------------------------------------------------------

/// How long one slot lasts, in seconds.
fn slot_secs() -> i64 {
    month::craving_hours() * 3600
}

/// How far the slot boundaries are shifted so that one of them always falls on
/// the anchor hour.
///
/// Without this the boundaries land wherever the epoch puts them, and the
/// evening's double ends at some arbitrary minute. With the anchor at 8 pm and
/// six-hour slots the boundaries are 2 am, 8 am, 2 pm and 8 pm India time, so
/// "the double lasts until 8 pm" is a true sentence rather than a rounded one.
fn anchor() -> i64 {
    let period = slot_secs();
    (month::double_hour() * 3600).rem_euclid(period)
}

/// Which slot a moment falls in. Counted from the India epoch, shifted to the
/// anchor hour, so a slot has the same number for everybody and a changed hatch
/// date never shuffles the rotation underneath people.
pub fn slot_of(ts: i64) -> i64 {
    (ts + month::IST_OFFSET - anchor()).div_euclid(slot_secs())
}

/// When a slot gives way to the next.
pub fn slot_ends(slot: i64) -> i64 {
    (slot + 1) * slot_secs() + anchor() - month::IST_OFFSET
}

/// How many slots there are in a watch.
pub fn slots_in_watch() -> i64 {
    (month::watch_days() * 86_400) / slot_secs()
}

// --- the rotation ---------------------------------------------------------------

/// The hunger of one seat in one slot. Pure, total, and the whole of the
/// assignment: everything else in this module only looks it up or writes it
/// down.
pub fn pick(seat: i64, slot: i64, cravings: usize) -> Vec<Source> {
    let games = GAMES.len() as i64;
    let want = cravings.clamp(1, GAMES.len()) as i64;
    let start = (seat + slot.wrapping_mul(want)).rem_euclid(games);
    (0..want).map(|j| GAMES[((start + j) % games) as usize]).collect()
}

/// The hunger as it stands for one member, written down the first time it is
/// asked for so it can be checked afterwards. A mod's override for the slot
/// beats everything.
pub fn craving_of(conn: &rusqlite::Connection, egg: &Egg, at: i64) -> Vec<Source> {
    let slot = slot_of(at);
    let from_keys = |keys: Vec<String>| -> Vec<Source> { keys.iter().filter_map(|k| Source::from_key(k)).collect() };
    if let Some(forced) = store::override_for(conn, slot) {
        let games = from_keys(forced);
        if !games.is_empty() {
            return games;
        }
    }
    if let Some(written) = store::craving(conn, egg.user, slot) {
        let games = from_keys(written);
        if !games.is_empty() {
            return games;
        }
    }
    let games = pick(egg.seat, slot, month::cravings());
    let keys: Vec<&str> = games.iter().map(|s| s.key()).collect();
    let _ = store::set_craving(conn, egg.user, slot, &keys);
    games
}

/// Everyone's hunger in one slot, counted per game: what `/craving` shows a mod
/// and what the balance test checks.
pub fn spread(seats: &[i64], slot: i64, cravings: usize) -> std::collections::BTreeMap<&'static str, usize> {
    let mut out: std::collections::BTreeMap<&'static str, usize> = GAMES.iter().map(|g| (g.key(), 0)).collect();
    for seat in seats {
        for game in pick(*seat, slot, cravings) {
            *out.entry(game.key()).or_insert(0) += 1;
        }
    }
    out
}

// --- what a point is worth --------------------------------------------------------

/// What one source pays one member right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pay {
    /// The month pays nothing. The game still ran and still kept its own score.
    Nothing,
    Once,
    Double,
}

impl Pay {
    /// The multiplier, or `None` when nothing is paid at all.
    pub fn times(self) -> Option<i64> {
        match self {
            Pay::Nothing => None,
            Pay::Once => Some(1),
            Pay::Double => Some(2),
        }
    }
}

/// The whole decision, with everything handed in so a test can ask it anything.
///
/// Four deliberate rules. A mod's own award is not a game and is never held
/// back. A member with no egg at all - anybody without the server games role,
/// or anybody at all while the month is off - is paid exactly as they were
/// before, because the month is something you join, not something done to you.
/// A member who has opted out earns the month nothing at all until they come
/// back. And a deduction is never multiplied, which is handled by the caller.
pub fn decide(month_on: bool, egg: Option<&Egg>, hungry: &[Source], source: Source, now: i64) -> Pay {
    if !month_on || source == Source::Mod {
        return Pay::Once;
    }
    let Some(egg) = egg else { return Pay::Once };
    // Opted out means out: nothing they do counts towards the month or towards
    // any house while they are away, and their dragon does not grow. Their own
    // game scores - chess points, duel points, every board - carry on as
    // normal, because those are not the month. Nothing is lost: the moment they
    // come back they resume exactly where they left off.
    if egg.frozen() {
        return Pay::Nothing;
    }
    let wanted = hungry.contains(&source);
    if egg.waiting(now) {
        if wanted { Pay::Once } else { Pay::Nothing }
    } else if wanted {
        Pay::Double
    } else {
        Pay::Once
    }
}

/// The door the ledger asks through: what this member's next point from this
/// source is multiplied by, or `None` for "pay nothing at all".
///
/// Reads the egg store. Never called while the egg store's lock is held.
pub fn multiplier(user: u64, source: Source, at: i64) -> Option<i64> {
    if !month::running() {
        return Some(1);
    }
    let Some(db) = store::db() else { return Some(1) };
    let conn = db.lock();
    let egg = store::egg(&conn, user).ok().flatten();
    let hungry = egg.as_ref().map(|e| craving_of(&conn, e, at)).unwrap_or_default();
    decide(true, egg.as_ref(), &hungry, source, at).times()
}

// --- the five stages ---------------------------------------------------------------

/// What the egg looks like, by how much it has been fed.
pub const STAGES: [(&str, &str); 5] = [
    ("🥚", "Cold and still"),
    ("🥚", "Warm to the touch"),
    ("🪺", "Shifting in the nest"),
    ("🔥", "Veined with fire"),
    ("🐣", "Cracking"),
];

/// What the dragon looks like, by how much it has grown. Same five steps as the
/// egg's stages, so a member's number means the same thing all month.
pub const SIZES: [(&str, &str); 5] = [
    ("🐲", "Hatchling"),
    ("🐲", "Fledgling"),
    ("🐉", "Winged"),
    ("🐉", "Great"),
    ("🔥", "Dreadful"),
];

/// The shipped thresholds: the points at which each stage begins.
pub const STAGE_POINTS: &str = "0,25,60,120,200";

fn thresholds() -> Vec<i64> {
    let raw = super::control::var("VIZIER_MONTH_EGG_STAGES").unwrap_or_else(|| STAGE_POINTS.to_string());
    let mut steps: Vec<i64> = raw.split(',').filter_map(|p| p.trim().parse::<i64>().ok()).collect();
    if steps.len() != STAGES.len() {
        steps = STAGE_POINTS.split(',').filter_map(|p| p.parse().ok()).collect();
    }
    steps.sort_unstable();
    steps
}

/// Which stage a number of points is at, 0 to 4, and how much more there is to
/// the next one.
pub fn stage(points: i64) -> (usize, Option<i64>) {
    let steps = thresholds();
    let at = steps.iter().rposition(|step| points >= *step).unwrap_or(0);
    let next = steps.get(at + 1).map(|step| (step - points).max(0));
    (at, next)
}

// --- the words ---------------------------------------------------------------------

fn hours_words(secs: i64) -> String {
    let secs = secs.max(0);
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3600;
    let minutes = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{} min", minutes.max(1))
    }
}

fn games_words(games: &[Source]) -> String {
    if games.is_empty() {
        return "nothing at all".to_string();
    }
    games.iter().map(|g| g.label().to_string()).collect::<Vec<_>>().join(" and ")
}

/// Everything `/dragon` says, whichever half of the month it is asked in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub name: String,
    /// Empty before the hatch.
    pub dragon: String,
    /// The themed house name, empty before the hatch.
    pub house: String,
    /// Points this month: the egg's warmth, the dragon's size.
    pub points: i64,
    /// Where they stand on the server, if they have scored.
    pub rank: Option<usize>,
    pub hungry: Vec<Source>,
    /// When the double on those games runs out.
    pub double_until: i64,
    pub hatch_at: i64,
    pub hatched: bool,
    /// Per shared limit: label, used today, the ceiling.
    pub caps: Vec<(&'static str, i64, i64)>,
    pub cards: usize,
    pub cards_in_play: usize,
    pub frozen: bool,
}

/// The one reply `/dragon` and `/egg` both give.
///
/// Before the hatch it is about the egg - its stage, its warmth, the days left.
/// After it, the same numbers read as a dragon. Either way it ends with what
/// they crave, how long the double lasts, what is left under today's limits,
/// how many of the twelve cards they hold, and the live page.
pub fn card_text(card: &Card, now: i64) -> String {
    let mut text = if card.hatched {
        let (at, _) = stage(card.points);
        let (icon, size) = SIZES[at.min(SIZES.len() - 1)];
        let crown = match card.rank {
            Some(1) => " 👑",
            Some(2) | Some(3) => " 🥈",
            _ => "",
        };
        let dragon = if card.dragon.is_empty() { "an unnamed dragon" } else { card.dragon.as_str() };
        format!(
            "{} **{}**{} — {}'s dragon, of **{}**.\n*{}* · **{}** point{} this month{}",
            icon,
            dragon,
            crown,
            card.name,
            if card.house.is_empty() { "no house yet" } else { card.house.as_str() },
            size,
            card.points,
            if card.points == 1 { "" } else { "s" },
            card.rank.map(|r| format!(" · **#{}** on the server", r)).unwrap_or_default()
        )
    } else {
        let (at, next) = stage(card.points);
        let (icon, label) = STAGES[at];
        let days = ((card.hatch_at - now).max(0) + 86_399) / 86_400;
        let mut head = format!(
            "{} **{}'s egg** · *{}* (stage {} of {})\nWarmth **{}** point{}{} · hatches in **{}**",
            icon,
            card.name,
            label,
            at + 1,
            STAGES.len(),
            card.points,
            if card.points == 1 { "" } else { "s" },
            card.rank.map(|r| format!(" · **#{}** on the server", r)).unwrap_or_default(),
            if days <= 1 { "less than a day".to_string() } else { format!("{} days", days) }
        );
        if let Some(more) = next {
            head.push_str(&format!("\n-# {} more warmth to the next stage.", more));
        }
        head
    };

    text.push_str(&format!(
        "\n**Craving** {} · double points until {}",
        games_words(&card.hungry),
        clock12(card.double_until)
    ));
    for (label, used, limit) in &card.caps {
        let left = (limit - used).max(0);
        text.push_str(&format!(
            "\n{}: {}/{} today{}",
            label,
            used,
            limit,
            if left == 0 { " · maxed".to_string() } else { format!(" · {} left", left) }
        ));
    }
    text.push_str(&format!("\n🃏 **{}** of {} cards", card.cards, card.cards_in_play));
    if card.frozen {
        text.push_str(
            "\n-# You're opted out, so nothing you play is counting towards the month. Press **Join the games** to pick up \
             exactly where you left off — same egg, same warmth, same house, same cards.",
        );
    }
    month::with_live(text)
}

/// A time of day, India time, as "8 pm".
fn clock12(ts: i64) -> String {
    let hour = ledger::ist_hour(ts);
    let minutes = ((ts + month::IST_OFFSET).rem_euclid(3600)) / 60;
    let (h, suffix) = match hour {
        0 => (12, "am"),
        12 => (12, "pm"),
        h if h < 12 => (h, "am"),
        h => (h - 12, "pm"),
    };
    if minutes == 0 { format!("{} {}", h, suffix) } else { format!("{}:{:02} {}", h, minutes, suffix) }
}

/// What `/egg` says while the egg is still an egg.
pub fn waiting_text(name: &str, points: i64, hungry: &[Source], changes_at: i64, hatch_at: i64, now: i64) -> String {
    let (at, next) = stage(points);
    let (icon, label) = STAGES[at];
    let mut text = format!(
        "{} **{}'s egg** · {} *(stage {} of {})*\nHungry for **{}** — for the next {}.\nFed **{}** point{} so far.",
        icon,
        name,
        label,
        at + 1,
        STAGES.len(),
        games_words(hungry),
        hours_words(changes_at - now),
        points,
        if points == 1 { "" } else { "s" }
    );
    if let Some(more) = next {
        text.push_str(&format!("\n-# {} more to the next stage.", more));
    }
    text.push_str(&format!("\n-# It hatches in {}. Only what it is hungry for feeds it until then.", hours_words(hatch_at - now)));
    month::with_live(text)
}

/// What `/egg` says once the egg has opened: the dragon, not the egg.
pub fn hatched_text(name: &str, dragon: &str, house: &str, points: i64, hungry: &[Source], changes_at: i64, now: i64) -> String {
    let text = format!(
        "🐉 **{}** — {}'s dragon, of **{}**.\nCraving **{}** for the next {} · **double** points there.\n**{}** point{} this month.",
        dragon,
        name,
        house,
        games_words(hungry),
        hours_words(changes_at - now),
        points,
        if points == 1 { "" } else { "s" }
    );
    month::with_live(text)
}

/// The channel the Join button lives in, so the nudge can point at it.
fn gate_channel() -> u64 {
    super::control::id("VIZIER_GAMES_GATE_CHANNEL").unwrap_or(1_549_295_943_751_307_324)
}

/// What `/dragon` says to somebody who hasn't got one: a nudge towards the
/// button, never an error. They have done nothing wrong - the month is
/// something you join.
pub fn no_egg_text() -> String {
    month::with_live(format!(
        "🥚 You haven't got an egg yet — the month belongs to the **server games** role.\nPress **Join the games** in <#{}> \
         and an egg is yours on the spot. You can do it any day of the month; the button never closes.\n-# Every game still \
         works for you in the meantime, and still keeps your own score on its own board.",
        gate_channel()
    ))
}

/// What `/egg` says while the month isn't running.
pub const MONTH_OFF: &str = "There's no themed month running right now. The games all still pay their own points.";

/// What `/livepoints` says: the page, and enough of the numbers to be worth
/// reading on its own.
pub fn livepoints_text(name: &str, house: Option<&str>, points: i64, rank: Option<usize>, left: &[(&'static str, i64, i64)]) -> String {
    let mut text = format!("📊 **{}** · **{}** point{} this month", name, points, if points == 1 { "" } else { "s" });
    if let Some(house) = house {
        text.push_str(&format!(" for **{}**", house));
    }
    if let Some(rank) = rank {
        text.push_str(&format!("\nRanked **#{}** on the server.", rank));
    }
    for (label, used, cap) in left {
        let room = (cap - used).max(0);
        text.push_str(&format!(
            "\n{}: {}/{} today{}",
            label,
            used,
            cap,
            if room == 0 { " · maxed".to_string() } else { format!(" · {} left", room) }
        ));
    }
    month::with_live(text)
}

// --- the commands ------------------------------------------------------------------

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

/// `/dragon` — the one command the whole month is read through.
pub fn dragon_builder() -> CreateCommand {
    CreateCommand::new("dragon")
        .description("your egg, then your dragon: what it craves, its size, your rank and today's limits")
}

/// `/egg` — the same command under the name people will type in the first week.
/// Registered as a command of its own rather than a Discord alias, because
/// Discord has no aliases, and handled by the same function.
pub fn egg_builder() -> CreateCommand {
    CreateCommand::new("egg").description("the same as /dragon: your egg while it is an egg, your dragon after")
}

/// Everything `/egg` needs, read in one pass so the reply is one lock each.
fn egg_state(user: u64, now: i64) -> Option<(Egg, Vec<Source>, i64)> {
    let db = store::db()?;
    let conn = db.lock();
    let egg = store::egg(&conn, user).ok().flatten()?;
    let hungry = craving_of(&conn, &egg, now);
    Some((egg, hungry, slot_ends(slot_of(now))))
}

/// Points this member has earned towards the month: since their egg was
/// claimed, or since the month began, whichever is later.
fn month_points(user: u64, since: i64) -> i64 {
    let Some(db) = super::house::db() else { return 0 };
    let conn = db.lock();
    let ledger_points: i64 =
        ledger::breakdown(&conn, user, since).map(|rows| rows.iter().map(|(_, n)| n).sum()).unwrap_or(0);
    let pool: i64 = ledger::pool_breakdown(&conn, user, since).iter().map(|(_, n)| n).sum();
    ledger_points + pool
}

/// `/dragon` and `/egg`, which are one command. Whispered, so a channel is
/// never cluttered by fifty people checking their own egg.
pub async fn egg_command(ctx: &Context, command: &CommandInteraction) {
    if !month::running() {
        let _ = command.create_response(&ctx.http, whisper(MONTH_OFF)).await;
        return;
    }
    let user = command.user.id.get();
    let now = Utc::now().timestamp();
    let name = command.user.global_name.clone().unwrap_or_else(|| command.user.name.clone());
    let Some((egg, hungry, changes_at)) = egg_state(user, now) else {
        let _ = command.create_response(&ctx.http, whisper(no_egg_text())).await;
        return;
    };
    let since = egg.claimed_ts.max(ledger::month_start(now));
    let points = month_points(user, since);
    let rank = super::house::db().and_then(|db| {
        let conn = db.lock();
        ledger::server_rank(&conn, user, ledger::month_start(now), i64::MAX).ok().flatten()
    });
    let caps: Vec<(&'static str, i64, i64)> = ledger::Group::ALL
        .into_iter()
        .map(|group| {
            let used: i64 = group.sources().iter().map(|s| super::house::earned_on(user, *s, now)).sum();
            (group.label(), used, group.limit())
        })
        .collect();
    let (cards, in_play) = match super::frog_store::db() {
        Some(db) => {
            let conn = db.lock();
            let mine = super::frog_store::cards_of(&conn, user).len();
            (mine, super::frog_store::wizards(&conn).into_iter().filter(|w| w.enabled).count())
        }
        None => (0, 0),
    };
    let card = Card {
        name,
        dragon: egg.dragon.clone(),
        house: super::house::house(&egg.house).map(month::name_of).unwrap_or_default(),
        points,
        rank,
        hungry,
        double_until: changes_at,
        hatch_at: egg.hatch_ts,
        hatched: !egg.waiting(now),
        caps,
        cards,
        cards_in_play: in_play.max(raven::DECK.len()),
        frozen: egg.frozen(),
    };
    let _ = command.create_response(&ctx.http, whisper(card_text(&card, now))).await;
}

pub fn craving_builder() -> CreateCommand {
    CreateCommand::new("craving")
        .description("admin only: what the eggs are hungry for, or make them hungry for something else")
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "games", "comma-separated game keys, or `clear`")
                .required(false),
        )
}

/// What `/craving` shows: the slot, how long it has left, and how many members
/// are hungry for each game right now.
pub fn craving_text(slot: i64, ends_at: i64, now: i64, counts: &std::collections::BTreeMap<&'static str, usize>, forced: bool) -> String {
    let mut text = format!("🍖 **The eggs are hungry** · slot {} · {} left", slot, hours_words(ends_at - now));
    if forced {
        text.push_str("\n-# A mod has overridden this slot.");
    }
    for game in GAMES {
        let n = counts.get(game.key()).copied().unwrap_or(0);
        text.push_str(&format!("\n{} · **{}** egg{}", game.label(), n, if n == 1 { "" } else { "s" }));
    }
    text.push_str("\n-# `/craving games:anagram,quiz` makes every egg want those two. `/craving games:clear` puts the rotation back.");
    month::with_live(text)
}

pub async fn craving_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can look at the cravings.")).await;
        return;
    }
    if !month::running() {
        let _ = command.create_response(&ctx.http, whisper(MONTH_OFF)).await;
        return;
    }
    let asked = command.data.options.iter().find_map(|o| match (&o.value, o.name == "games") {
        (CommandDataOptionValue::String(v), true) => Some(v.clone()),
        _ => None,
    });
    let now = Utc::now().timestamp();
    let slot = slot_of(now);
    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("The egg store isn't open.")).await;
        return;
    };
    let text = {
        let conn = db.lock();
        if let Some(raw) = &asked {
            let wanted: Vec<&str> = if raw.trim().eq_ignore_ascii_case("clear") {
                Vec::new()
            } else {
                raw.split(',').map(|p| p.trim()).filter(|p| Source::from_key(p).is_some_and(is_game)).collect()
            };
            if !raw.trim().eq_ignore_ascii_case("clear") && wanted.is_empty() {
                let keys: Vec<&str> = GAMES.iter().map(|g| g.key()).collect();
                drop(conn);
                let _ = command
                    .create_response(&ctx.http, whisper(format!("I don't know those games. Pick from: {}", keys.join(", "))))
                    .await;
                return;
            }
            let _ = store::set_override(&conn, slot, &wanted, user, now);
        }
        let seats: Vec<i64> = store::all(&conn).into_iter().map(|e| e.seat).collect();
        let forced = store::override_for(&conn, slot);
        let counts = match &forced {
            Some(keys) => {
                let games: Vec<Source> = keys.iter().filter_map(|k| Source::from_key(k)).collect();
                let mut out: std::collections::BTreeMap<&'static str, usize> =
                    GAMES.iter().map(|g| (g.key(), 0)).collect();
                for game in games {
                    out.insert(game.key(), seats.len());
                }
                out
            }
            None => spread(&seats, slot, month::cravings()),
        };
        craving_text(slot, slot_ends(slot), now, &counts, forced.is_some())
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

pub fn livepoints_builder() -> CreateCommand {
    CreateCommand::new("livepoints").description("the live page, your points, your rank and what's left today")
}

pub async fn livepoints_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let now = Utc::now().timestamp();
    let name = command.user.global_name.clone().unwrap_or_else(|| command.user.name.clone());
    let since = ledger::month_start(now);
    let points = month_points(user, since);
    let house = super::house::house_of(user).map(month::name_of);
    let mut left: Vec<(&'static str, i64, i64)> = Vec::new();
    if month::running() {
        for group in ledger::Group::ALL {
            let used: i64 = group.sources().iter().map(|s| super::house::earned_on(user, *s, now)).sum();
            left.push((group.label(), used, group.limit()));
        }
    }
    let rank = super::house::db().and_then(|db| {
        let conn = db.lock();
        ledger::server_rank(&conn, user, since, i64::MAX).ok().flatten()
    });
    let text = livepoints_text(&name, house.as_deref(), points, rank, &left);
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

pub fn mycards_builder() -> CreateCommand {
    CreateCommand::new("mycards").description("the cards you hold, with their serial numbers")
}

/// What `/mycards` says. `cards` is already in the order the page shows them.
pub fn mycards_text(name: &str, cards: &[(String, &'static str, i64)]) -> String {
    if cards.is_empty() {
        return month::with_live(format!(
            "🃏 **{}** · no cards yet. Capture a raven when one lands and the card is yours, serial number and all.",
            name
        ));
    }
    let mut text = format!("🃏 **{}** · **{}** card{}", name, cards.len(), if cards.len() == 1 { "" } else { "s" });
    for (card_name, rarity, serial) in cards.iter().take(25) {
        text.push_str(&format!("\n{} · *{}* · {}", card_name, rarity, super::frog_store::serial_label(*serial)));
    }
    if cards.len() > 25 {
        text.push_str(&format!("\n-# …and {} more.", cards.len() - 25));
    }
    month::with_live(text)
}

pub async fn mycards_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let name = command.user.global_name.clone().unwrap_or_else(|| command.user.name.clone());
    let cards: Vec<(String, &'static str, i64)> = match super::frog_store::db() {
        Some(db) => {
            let conn = db.lock();
            super::frog_store::cards_of(&conn, user)
                .into_iter()
                .map(|c| (c.wizard_name, c.rarity.name(), c.serial))
                .collect()
        }
        None => Vec::new(),
    };
    let _ = command.create_response(&ctx.http, whisper(mycards_text(&name, &cards))).await;
}

// --- handing the eggs out ----------------------------------------------------------

/// When an egg claimed at `now` opens.
///
/// Everybody who was already on the sheet shares the month's one hatch. Anybody
/// who turns up after it gets their own week, so a member who joins on the 20th
/// still plays an egg week rather than an egg minute.
pub fn hatch_for(now: i64, month_hatch: i64, watch_days: i64) -> i64 {
    if now < month_hatch { month_hatch } else { now + watch_days * 86_400 }
}

/// Gives an egg to everybody who holds the server games role and hasn't got one.
///
/// THE ROLE IS THE WHOLE OF IT. An egg belongs to the role and nothing else: no
/// role, no egg, no dragon, no house. A member without it plays every game
/// exactly as before and keeps every one of their own game scores - they are
/// simply not in the month. Which also means a member who already had the role
/// from last month's sign-up already has an egg and is never asked to press
/// anything again.
///
/// `holders` is (member, display name) for everybody wearing the role, read
/// from the gateway cache by the caller. Returns how many eggs were new.
/// Idempotent: claiming is a no-op for anybody who already has one, so this can
/// run every few minutes forever.
pub fn hand_out(holders: &[(u64, String)], now: i64) -> usize {
    if !month::running() {
        return 0;
    }
    let Some(eggs) = store::db() else { return 0 };
    let hatch_at = month::hatch_at();
    let days = month::watch_days();
    let conn = eggs.lock();
    let mut fresh = 0;
    for (user, name) in holders {
        let had = store::egg(&conn, *user).ok().flatten().is_some();
        if store::claim(&conn, *user, name, hatch_for(now, hatch_at, days), now).is_ok() && !had {
            fresh += 1;
        }
    }
    fresh
}

/// How long is left of the toggle's cooldown, in seconds, or 0 when they may
/// press now. Pure, so the wording can be tested without a clock.
pub fn cooldown_left(last: Option<i64>, now: i64, window: i64) -> i64 {
    match last {
        Some(at) => (at + window - now).clamp(0, window),
        None => 0,
    }
}

/// The lightest house by total activity: where a late arrival is put, so that
/// joining on the 25th keeps the four level instead of tipping them.
///
/// `totals` is (house key, points, headcount). Ties go to the smaller house and
/// then to the house's own order, never to chance.
pub fn lightest(totals: &[(&'static str, i64, usize)]) -> Option<&'static str> {
    totals
        .iter()
        .enumerate()
        .min_by(|(i, a), (j, b)| a.1.cmp(&b.1).then(a.2.cmp(&b.2)).then(i.cmp(j)))
        .map(|(_, one)| one.0)
}

/// The four houses as they stand this month: key, points, headcount.
fn house_weights(now: i64) -> Vec<(&'static str, i64, usize)> {
    let counts = super::house::counts();
    let Some(db) = super::house::db() else {
        return super::house::HOUSES.iter().map(|h| (h.key, 0, 0)).collect();
    };
    let conn = db.lock();
    let since = ledger::month_start(now);
    super::house::HOUSES
        .iter()
        .map(|h| {
            let points = ledger::house_total(&conn, h.key, since, i64::MAX).unwrap_or(0);
            (h.key, points, counts.get(h.key).copied().unwrap_or(0).max(0) as usize)
        })
        .collect()
}

/// What pressing "Join the games" does about the month, after the role itself
/// has been given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Joined {
    /// They already had an egg and are back in it, exactly as they left it.
    Resumed { hatched: bool },
    /// A fresh egg, hatching at this moment.
    Egg { hatches_at: i64 },
    /// Joined after the hatch, so their egg opened on the spot.
    Hatched { house: &'static str, dragon: String },
    /// The month isn't running, or the store isn't open: nothing to do.
    Nothing,
}

/// Claims an egg for somebody who has just pressed Join, at any point in the
/// month, and never closes.
///
/// Three cases, in the order they are decided.
///
/// 1. **They have been here before.** Their egg is thawed and nothing else is
///    touched. If they had already hatched they go back to the SAME house -
///    their house is theirs for the month, and re-sorting a returner would turn
///    opting out into shopping for whichever house is winning.
/// 2. **New, before the hatch.** A fresh egg on the shared hatch.
/// 3. **New, after the hatch.** The egg opens on the spot and they are put in
///    whichever house is lightest by activity, so a late arrival keeps the four
///    level.
pub fn join(user: u64, name: &str, now: i64) -> Joined {
    if !month::running() {
        return Joined::Nothing;
    }
    let Some(db) = store::db() else { return Joined::Nothing };
    let existing = {
        let conn = db.lock();
        store::egg(&conn, user).ok().flatten()
    };
    if let Some(had) = existing {
        {
            let conn = db.lock();
            let _ = store::thaw(&conn, user);
        }
        // Back to the house they left, by name, so a returner is never re-sorted.
        if let Some(house) = super::house::house(&had.house) {
            super::house::place(user, house, "rejoined");
        }
        return Joined::Resumed { hatched: had.open() };
    }
    let hatches_at = hatch_for(now, month::hatch_at(), month::watch_days());
    let fresh = {
        let conn = db.lock();
        store::claim(&conn, user, name, hatches_at, now).ok()
    };
    if fresh.is_none() {
        return Joined::Nothing;
    }
    // Before the hatch: an egg like everybody else's.
    if now < month::hatch_at() {
        return Joined::Egg { hatches_at };
    }
    // After it: there is no reveal left to wait for, so it opens now.
    let weights = house_weights(now);
    let Some(key) = lightest(&weights) else { return Joined::Egg { hatches_at } };
    let taken = {
        let conn = db.lock();
        store::dragons(&conn)
    };
    let names = super::hatch::name_dragons(&[user], &taken);
    let dragon = names.get(&user).cloned().unwrap_or_default();
    {
        let conn = db.lock();
        let _ = store::hatch(&conn, user, key, &dragon, now);
    }
    if let Some(house) = super::house::house(key) {
        super::house::place(user, house, "late hatch");
    }
    Joined::Hatched { house: key, dragon }
}

/// What opting out does about the month: freeze, never delete.
pub fn leave(user: u64) -> bool {
    let Some(db) = store::db() else { return false };
    let conn = db.lock();
    store::freeze(&conn, user, Utc::now().timestamp()).unwrap_or(false)
}

/// How often the job looks for somebody new to give an egg to.
const HAND_OUT_EVERY: std::time::Duration = std::time::Duration::from_secs(600);

/// The job: eggs for the sheet, for as long as the month runs.
pub fn spawn(ctx: serenity::all::Context) {
    tokio::spawn(async move {
        loop {
            if month::running() {
                let fresh = hand_out(&role_holders(&ctx), Utc::now().timestamp());
                if fresh > 0 {
                    tracing::info!("egg: {} new egg{} handed out to the games role", fresh, if fresh == 1 { "" } else { "s" });
                }
            }
            tokio::time::sleep(HAND_OUT_EVERY).await;
        }
    });
}

/// Everybody wearing the server games role, from the gateway cache. The role IS
/// the membership, so this is read fresh rather than from any list of our own:
/// a mod who hands the role out by hand has handed out an egg.
fn role_holders(ctx: &serenity::all::Context) -> Vec<(u64, String)> {
    let Some(role) = super::signup::role_id().map(serenity::all::RoleId::new) else { return Vec::new() };
    let Some(guild) = ctx.cache.guilds().first().copied() else { return Vec::new() };
    let Some(cached) = ctx.cache.guild(guild) else { return Vec::new() };
    cached
        .members
        .values()
        .filter(|m| !m.user.bot && m.roles.contains(&role))
        .map(|m| (m.user.id.get(), m.display_name().to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::super::month::testing::Month;
    use super::*;

    /// Whatever the server's size, the seats are 0..n.
    fn seats(n: i64) -> Vec<i64> {
        (0..n).collect()
    }

    #[test]
    fn a_seat_wants_every_game_inside_one_day_and_the_week_over_and_over() {
        let per_slot = 2;
        let slots_a_day = 24 / 6;
        for seat in 0..53 {
            let day: BTreeSet<&str> =
                (0..slots_a_day).flat_map(|s| pick(seat, s, per_slot)).map(|g| g.key()).collect();
            assert_eq!(day.len(), GAMES.len(), "seat {} missed a game on day one: {:?}", seat, day);
            let week: BTreeSet<&str> =
                (0..slots_a_day * 7).flat_map(|s| pick(seat, s, per_slot)).map(|g| g.key()).collect();
            assert_eq!(week.len(), GAMES.len(), "seat {} missed a game across the week", seat);
        }
    }

    #[test]
    fn the_same_seat_and_slot_always_give_the_same_two_games() {
        for seat in 0..40 {
            for slot in [-3i64, 0, 1, 7, 40_000, 1_234_567] {
                let once = pick(seat, slot, 2);
                assert_eq!(once.len(), 2, "two games, always");
                assert_eq!(once[0], once[0], "same call, same answer");
                assert_eq!(pick(seat, slot, 2), once, "and again");
                assert_ne!(once[0], once[1], "and never the same game twice over");
            }
        }
    }

    /// The crowd test: nobody is ever the only person playing a game.
    #[test]
    fn every_game_has_a_crowd_in_every_slot() {
        for members in [8i64, 20, 53, 120] {
            let seats = seats(members);
            for slot in 0..28 {
                let counts = spread(&seats, slot, 2);
                let smallest = counts.values().copied().min().unwrap_or(0);
                let biggest = counts.values().copied().max().unwrap_or(0);
                let total: usize = counts.values().sum();
                assert_eq!(total, members as usize * 2, "every member is hungry for exactly two");
                // Each seat lights two ADJACENT games, and the seats themselves
                // are spread evenly round the seven, so two games can differ by
                // at most one seat each - never by more than the craving count.
                assert!(biggest - smallest <= 2, "slot {} with {} members is lopsided: {:?}", slot, members, counts);
                let share = members as usize * 2 / GAMES.len();
                assert!(smallest + 1 >= share, "a game was left nearly empty: {:?}", counts);
            }
        }
    }

    #[test]
    fn a_bigger_craving_count_still_covers_everything() {
        for want in 1..=4usize {
            let covered: BTreeSet<&str> = (0..GAMES.len() as i64 * 2).flat_map(|s| pick(5, s, want)).map(|g| g.key()).collect();
            assert_eq!(covered.len(), GAMES.len(), "{} at a time should still reach every game", want);
            assert_eq!(pick(5, 0, want).len(), want);
        }
        assert_eq!(pick(0, 0, 99).len(), GAMES.len(), "it can never want more games than there are");
    }

    #[test]
    fn slots_follow_the_india_clock_and_do_not_overlap() {
        let mut month = Month::on();
        month.set("VIZIER_MONTH_CRAVING_HOURS", "6");
        month.set("VIZIER_MONTH_DOUBLE_HOUR", "20");
        // 2026-10-02 11:00 India time.
        let at = month::parse_ist("2026-10-02 11:00").unwrap();
        let slot = slot_of(at);
        assert_eq!(slot_of(at + 60), slot, "a minute later is the same slot");
        let ends = slot_ends(slot);
        assert_eq!(slot_of(ends), slot + 1, "the end of one slot is the start of the next");
        assert_eq!(ends - slot_ends(slot - 1), 6 * 3600, "a slot is as long as the setting says");
        assert_eq!(slots_in_watch(), 28, "a seven-day watch in six-hour slots");
        // The anchor is the point of it: with the double set to 8 pm, one
        // boundary lands on 8 pm, so "double points until 8 pm" is a true
        // sentence rather than a rounded one.
        assert_eq!(ends, month::parse_ist("2026-10-02 14:00").unwrap(), "11 am is in the 8 am - 2 pm slot");
        let evening = slot_of(month::parse_ist("2026-10-02 19:30").unwrap());
        assert_eq!(slot_ends(evening), month::parse_ist("2026-10-02 20:00").unwrap(), "the evening's double ends at 8 pm");
        // Moving the anchor moves every boundary with it.
        month.set("VIZIER_MONTH_DOUBLE_HOUR", "21");
        let evening = slot_of(month::parse_ist("2026-10-02 19:30").unwrap());
        assert_eq!(slot_ends(evening), month::parse_ist("2026-10-02 21:00").unwrap());
    }

    // --- what a point is worth --------------------------------------------------

    fn an_egg(hatch_at: i64) -> Egg {
        Egg {
            user: 1,
            seat: 0,
            name: "Zoya".into(),
            claimed_ts: 0,
            hatch_ts: hatch_at,
            house: String::new(),
            dragon: String::new(),
            hatched_ts: None,
            frozen_ts: None,
        }
    }

    #[test]
    fn during_the_egg_week_only_the_hungry_games_pay() {
        let egg = an_egg(1_000);
        let hungry = vec![Source::Anagram, Source::Quiz];
        for game in GAMES {
            let pay = decide(true, Some(&egg), &hungry, game, 500);
            if hungry.contains(&game) {
                assert_eq!(pay, Pay::Once, "{} is hungry, so it pays", game.key());
            } else {
                assert_eq!(pay, Pay::Nothing, "{} is not, so it pays the month nothing", game.key());
            }
        }
        // Everything that isn't one of the seven pays nothing either - it runs,
        // it is fun, it keeps its own score, and the month doesn't count it.
        for other in [Source::Chat, Source::Sudoku, Source::Frog, Source::Chess] {
            assert_eq!(decide(true, Some(&egg), &hungry, other, 500), Pay::Nothing);
        }
        // Except a mod's own award, which is not a game at all.
        assert_eq!(decide(true, Some(&egg), &hungry, Source::Mod, 500), Pay::Once);
    }

    #[test]
    fn after_the_hatch_everything_pays_and_the_hungry_pay_double() {
        let egg = an_egg(1_000);
        let hungry = vec![Source::Anagram, Source::Quiz];
        for game in GAMES {
            let pay = decide(true, Some(&egg), &hungry, game, 1_001);
            assert_eq!(pay, if hungry.contains(&game) { Pay::Double } else { Pay::Once }, "{}", game.key());
        }
        for other in [Source::Chat, Source::Sudoku, Source::Frog] {
            assert_eq!(decide(true, Some(&egg), &hungry, other, 1_001), Pay::Once);
        }
        assert_eq!((Pay::Nothing.times(), Pay::Once.times(), Pay::Double.times()), (None, Some(1), Some(2)));
    }

    #[test]
    fn an_early_claim_shares_the_months_hatch_and_a_late_one_gets_its_own_week() {
        let hatch = 1_000_000i64;
        assert_eq!(hatch_for(hatch - 1, hatch, 7), hatch, "before the hatch, everybody shares it");
        assert_eq!(hatch_for(hatch, hatch, 7), hatch + 7 * 86_400, "from the hatch on, a full week each");
        assert_eq!(hatch_for(hatch + 86_400, hatch, 7), hatch + 86_400 + 7 * 86_400);
    }

    /// Out means out: nothing they play counts towards the month or towards any
    /// house - and nothing of theirs is lost by it.
    #[test]
    fn an_opted_out_member_earns_the_month_nothing_at_all() {
        let out = Egg { frozen_ts: Some(900), ..an_egg(1_000) };
        let hungry = vec![Source::Anagram, Source::Quiz];
        // Not during the egg week...
        for game in GAMES {
            assert_eq!(decide(true, Some(&out), &hungry, game, 500), Pay::Nothing, "{} while out", game.key());
        }
        // ...and not after the hatch either, hungry or otherwise.
        let hatched = Egg { hatched_ts: Some(1_000), ..out.clone() };
        for game in GAMES {
            assert_eq!(decide(true, Some(&hatched), &hungry, game, 2_000), Pay::Nothing, "{} while out", game.key());
        }
        for other in [Source::Chat, Source::Sudoku, Source::Frog] {
            assert_eq!(decide(true, Some(&hatched), &hungry, other, 2_000), Pay::Nothing);
        }
        // A mod's own award is still not a game, so it still lands.
        assert_eq!(decide(true, Some(&out), &hungry, Source::Mod, 500), Pay::Once);
        // And the moment they come back the very same call pays again.
        let back = Egg { frozen_ts: None, ..hatched };
        assert_eq!(decide(true, Some(&back), &hungry, Source::Anagram, 2_000), Pay::Double);
    }

    #[test]
    fn the_lightest_house_is_where_a_late_arrival_goes() {
        let four = [("gryffindor", 900i64, 12usize), ("slytherin", 400, 9), ("ravenclaw", 870, 11), ("hufflepuff", 400, 7)];
        // Level on points: the smaller house takes them, so joining late keeps
        // the headcount sane as well as the totals.
        assert_eq!(lightest(&four), Some("hufflepuff"));
        let clear = [("gryffindor", 900i64, 12usize), ("slytherin", 10, 9), ("ravenclaw", 870, 11), ("hufflepuff", 400, 7)];
        assert_eq!(lightest(&clear), Some("slytherin"));
        // Level all round: the house order, never chance.
        let level: Vec<(&'static str, i64, usize)> =
            super::super::house::HOUSES.iter().map(|h| (h.key, 0i64, 0usize)).collect();
        assert_eq!(lightest(&level), Some(super::super::house::HOUSES[0].key));
        assert_eq!(lightest(&[]), None);
    }

    #[test]
    fn the_toggle_has_an_hour_on_it_and_says_how_long_is_left() {
        let hour = 3_600;
        assert_eq!(cooldown_left(None, 1_000, hour), 0, "a first press is never refused");
        assert_eq!(cooldown_left(Some(1_000), 1_000, hour), hour, "straight after, the whole hour");
        assert_eq!(cooldown_left(Some(1_000), 1_000 + 600, hour), hour - 600);
        assert_eq!(cooldown_left(Some(1_000), 1_000 + hour, hour), 0, "an hour later, free again");
        assert_eq!(cooldown_left(Some(1_000), 1_000 + hour * 5, hour), 0);
        // A clock that has gone backwards can never lock somebody out for longer
        // than the window itself.
        assert_eq!(cooldown_left(Some(10_000), 1_000, hour), hour);
        assert_eq!(cooldown_left(Some(1_000), 2_000, 0), 0, "and no window means no wait");
    }

    #[test]
    fn with_the_month_off_or_no_egg_nothing_at_all_changes() {
        let egg = an_egg(1_000);
        for game in GAMES {
            assert_eq!(decide(false, Some(&egg), &[], game, 500), Pay::Once, "month off: {} pays as it always did", game.key());
            assert_eq!(decide(true, None, &[], game, 500), Pay::Once, "no egg: {} pays as it always did", game.key());
        }
    }

    // --- the stages ---------------------------------------------------------------

    #[test]
    fn the_egg_climbs_five_stages_and_stops_at_the_top() {
        let _month = Month::on();
        assert_eq!(stage(0).0, 0);
        assert_eq!(stage(24).0, 0);
        assert_eq!(stage(25).0, 1);
        assert_eq!(stage(119).0, 2);
        assert_eq!(stage(120).0, 3);
        assert_eq!(stage(200).0, 4);
        assert_eq!(stage(10_000), (4, None), "the last stage is the last stage");
        assert_eq!(stage(0).1, Some(25), "and it says how far the next one is");
        assert_eq!(STAGES.len(), 5);
    }

    #[test]
    fn a_broken_stage_setting_falls_back_to_the_shipped_one() {
        let mut month = Month::on();
        month.set("VIZIER_MONTH_EGG_STAGES", "10,20");
        assert_eq!(stage(25).0, 1, "too few steps: the shipped ones apply");
        month.set("VIZIER_MONTH_EGG_STAGES", "0,10,20,30,40");
        assert_eq!(stage(25).0, 2, "and a good one is used");
    }

    // --- the words ------------------------------------------------------------------

    #[test]
    fn egg_says_the_egg_before_the_hatch_and_the_dragon_after() {
        let mut month = Month::on();
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live");
        let before = waiting_text("Zoya", 30, &[Source::Anagram, Source::Quiz], 3_600, 86_400, 0);
        assert!(before.contains("stage 2 of 5"), "the stage is on it: {}", before);
        assert!(before.contains("Anagram") && before.contains("Quiz"), "what it wants is on it");
        assert!(before.contains("1h 0m"), "and when that changes");
        assert!(before.contains("1d 0h"), "and the time to the hatch");
        assert!(before.contains("Fed **30** point"), "and what it has been fed");
        assert!(before.contains("https://mlci.example/live"), "and the page, always");
        assert!(!before.contains("🐉"), "there is no dragon yet");

        let after = hatched_text("Zoya", "Vhagaryx", "Stark", 400, &[Source::Geo], 7_200, 0);
        assert!(after.contains("Vhagaryx") && after.contains("Stark"), "the dragon and the house: {}", after);
        assert!(after.contains("double"), "and that the craving pays double");
        assert!(!after.contains("hatches in"), "nothing about hatching any more");
        assert!(after.contains("https://mlci.example/live"));
        assert!(no_egg_text().contains("https://mlci.example/live"));
    }

    fn a_card() -> Card {
        Card {
            name: "Zoya".into(),
            dragon: String::new(),
            house: String::new(),
            points: 30,
            rank: Some(4),
            hungry: vec![Source::Anagram, Source::Quiz],
            double_until: month::parse_ist("2026-10-02 20:00").unwrap(),
            hatch_at: month::parse_ist("2026-10-08 12:00").unwrap(),
            hatched: false,
            caps: vec![("Quick games", 12, 20), ("Thinking games", 30, 30)],
            cards: 3,
            cards_in_play: 12,
            frozen: false,
        }
    }

    #[test]
    fn dragon_shows_the_egg_first_and_the_dragon_after_and_always_the_same_tail() {
        let mut month = Month::on();
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live");
        let now = month::parse_ist("2026-10-02 11:00").unwrap();
        let egg = card_text(&a_card(), now);
        assert!(egg.contains("Zoya's egg") && egg.contains("stage 2 of 5"), "{}", egg);
        assert!(egg.contains("Warmth **30** points"), "{}", egg);
        assert!(egg.contains("hatches in **7 days**"), "the days to the hatch: {}", egg);
        assert!(egg.contains("#4 on the server") || egg.contains("**#4**"), "their rank: {}", egg);
        assert!(!egg.contains("🐉"), "there is no dragon yet");

        let dragon = card_text(
            &Card { hatched: true, dragon: "Vhagaryx".into(), house: "Stark".into(), points: 420, rank: Some(2), ..a_card() },
            now,
        );
        assert!(dragon.contains("Vhagaryx") && dragon.contains("Stark"), "{}", dragon);
        assert!(dragon.contains("Great") || dragon.contains("Dreadful"), "the dragon's size: {}", dragon);
        assert!(dragon.contains("🥈"), "second on the server wears something: {}", dragon);
        assert!(card_text(&Card { hatched: true, rank: Some(1), ..a_card() }, now).contains("👑"), "first wears the crown");
        assert!(!card_text(&Card { hatched: true, rank: Some(9), ..a_card() }, now).contains("👑"));
        assert!(!dragon.contains("hatches in"), "nothing about hatching any more");

        // The tail is the same either way, and it is most of the use of it.
        for text in [&egg, &dragon] {
            assert!(text.contains("Anagram") && text.contains("Quiz"), "what it craves: {}", text);
            assert!(text.contains("double points until 8 pm"), "and how long the double lasts: {}", text);
            assert!(text.contains("Quick games: 12/20") && text.contains("8 left"), "{}", text);
            assert!(text.contains("Thinking games: 30/30") && text.contains("maxed"), "{}", text);
            assert!(text.contains("**3** of 12 cards"), "the cards: {}", text);
            assert!(text.contains("https://mlci.example/live"), "and the page, always: {}", text);
        }
    }

    #[test]
    fn a_member_who_is_out_is_told_so_and_told_nothing_is_lost() {
        let _month = Month::on();
        let now = month::parse_ist("2026-10-02 11:00").unwrap();
        let text = card_text(&Card { frozen: true, ..a_card() }, now);
        assert!(text.contains("opted out"), "{}", text);
        assert!(text.contains("Join the games"), "and where to come back: {}", text);
        assert!(text.contains("exactly where you left off"), "{}", text);
        assert!(!card_text(&a_card(), now).contains("opted out"));
    }

    #[test]
    fn somebody_without_an_egg_is_nudged_at_the_button_not_scolded() {
        let mut month = Month::on();
        month.set("VIZIER_GAMES_GATE_CHANNEL", "1549295943751307324");
        let text = no_egg_text();
        assert!(text.contains("<#1549295943751307324>"), "it points at the channel: {}", text);
        assert!(text.contains("Join the games"));
        assert!(text.contains("any day of the month") && text.contains("never closes"), "{}", text);
        assert!(text.contains("server games"), "and says what the egg belongs to");
        // Nothing in it reads as a telling-off.
        for scold in ["error", "cannot", "not allowed", "too late"] {
            assert!(!text.to_lowercase().contains(scold), "{:?} has no business in a nudge: {}", scold, text);
        }
    }

    #[test]
    fn a_time_of_day_reads_as_one() {
        assert_eq!(clock12(month::parse_ist("2026-10-02 20:00").unwrap()), "8 pm");
        assert_eq!(clock12(month::parse_ist("2026-10-02 08:00").unwrap()), "8 am");
        assert_eq!(clock12(month::parse_ist("2026-10-02 00:00").unwrap()), "12 am");
        assert_eq!(clock12(month::parse_ist("2026-10-02 12:30").unwrap()), "12:30 pm");
        assert_eq!(clock12(month::parse_ist("2026-10-02 14:05").unwrap()), "2:05 pm");
    }

    #[test]
    fn livepoints_says_the_numbers_as_well_as_the_link() {
        let mut month = Month::on();
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live");
        let text = livepoints_text("Zoya", Some("Stark"), 412, Some(3), &[("Quick games", 12, 20), ("Thinking games", 30, 30)]);
        assert!(text.contains("**412** point") && text.contains("Stark") && text.contains("#3"), "{}", text);
        assert!(text.contains("12/20") && text.contains("8 left"), "what is left today: {}", text);
        assert!(text.contains("30/30") && text.contains("maxed"));
        assert!(text.contains("https://mlci.example/live"));
    }

    #[test]
    fn mycards_lists_serials_and_says_so_when_there_are_none() {
        let mut month = Month::on();
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live");
        let empty = mycards_text("Zoya", &[]);
        assert!(empty.contains("no cards yet") && empty.contains("https://mlci.example/live"));
        let some = mycards_text("Zoya", &[("House Stark".to_string(), "Common", 42)]);
        assert!(some.contains("House Stark") && some.contains("No. 0042"), "{}", some);
    }

    #[test]
    fn craving_shows_every_game_and_says_when_a_mod_has_forced_it() {
        let mut month = Month::on();
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live");
        let counts = spread(&seats(53), 4, 2);
        let text = craving_text(4, 3_600, 0, &counts, false);
        for game in GAMES {
            assert!(text.contains(game.label()), "{} is missing from {}", game.key(), text);
        }
        assert!(!text.contains("overridden"));
        assert!(craving_text(4, 3_600, 0, &counts, true).contains("overridden"));
        assert!(text.contains("https://mlci.example/live"));
    }

    // --- the store and the rotation together -------------------------------------

    #[test]
    fn a_written_hunger_stands_even_when_the_setting_changes_under_it() {
        let mut month = Month::on();
        month.set("VIZIER_MONTH_CRAVINGS", "2");
        let conn = store::memory();
        let egg = store::claim(&conn, 7, "Zoya", 10_000, 0).unwrap();
        let at = 1_000_000;
        let first = craving_of(&conn, &egg, at);
        assert_eq!(first.len(), 2);
        // The owner widens the craving mid-week. The slot already judged stays
        // as it was judged; the NEXT slot takes the new number.
        month.set("VIZIER_MONTH_CRAVINGS", "3");
        assert_eq!(craving_of(&conn, &egg, at), first, "what counted at the time still counts");
        let next = craving_of(&conn, &egg, slot_ends(slot_of(at)) + 60);
        assert_eq!(next.len(), 3, "and the new slot is wider");
    }

    #[test]
    fn a_mods_override_beats_the_rotation_for_that_slot_only() {
        let _month = Month::on();
        let conn = store::memory();
        let egg = store::claim(&conn, 7, "Zoya", 10_000, 0).unwrap();
        let at = 1_000_000;
        let slot = slot_of(at);
        let rolled = craving_of(&conn, &egg, at);
        store::set_override(&conn, slot, &["movie"], 1, at).unwrap();
        assert_eq!(craving_of(&conn, &egg, at), vec![Source::Movie], "the mod's word goes");
        let later = craving_of(&conn, &egg, slot_ends(slot) + 60);
        assert_ne!(later, vec![Source::Movie], "and only for that slot");
        store::set_override(&conn, slot, &[], 1, at).unwrap();
        assert_eq!(craving_of(&conn, &egg, at), rolled, "clearing it puts the rotation back");
    }
}
