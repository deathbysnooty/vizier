//! Geo — GeoGuessr for India, in its own channel (`VIZIER_GEO_CHANNEL`).
//!
//! A match is a run of places. The bot puts up ONE street photo taken
//! somewhere in India and asks where it is; whoever says first, and nearest,
//! takes the round. Like anagrams, Guess the Word and Guess the Movie, and
//! unlike sudoku or chess, people CAN type in the channel: guessing IS typing.
//!
//! ## What counts as an answer
//!
//! A place name at whatever scale you happen to know. Naming the **state** is
//! the safe two points. Naming a **town near the photo** is worth four, and a
//! town on top of it five — and the town carries its state, so nobody has to
//! know both. The gap between shouting `Telangana` at once and holding on to
//! risk `Warangal` is the whole decision the game asks for.
//!
//! The state is a gate, not a tiebreak: **a guess in the wrong state scores
//! nothing**, however near the kilometres come out
//! ([`geo_bank`](super::geo_bank)). A town at the far end of the RIGHT state is
//! a wrong town but a right state, and still earns the state's two.
//!
//! The first correct guess ends the round, so the channel stays a race and
//! open chat stays fair — once somebody has it, there is nothing left to copy.
//!
//! ## Why a state is drawn before a photo
//!
//! The picture supply is wildly uneven: KartaView is community dashcam
//! footage, and nearly two thirds of the Indian frames are Tamil Nadu and
//! Telangana. Sampling photos directly would make "Tamil Nadu" a winning guess
//! without looking at the screen. So a round draws a STATE first, evenly, and
//! only then a photo inside it ([`pick`]).
//!
//! ## Steering a round
//!
//! `!hint` gives the state's first letter and the quarter of the country it is
//! in, once per round, and takes a point off what the round pays (never below
//! one). `!skip` opens up only after a hint. A round nobody touches goes stale
//! by itself after `VIZIER_GEO_IDLE_MINUTES`.
//!
//! One task does all the posting ([`run`]), the way Guess the Movie does: the
//! message handler only writes to the database and leaves a note in the shared
//! state, so two guesses landing together can never post two rounds.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateAttachment,
    CreateButton, CreateCommand, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    EditAttachments, EditMessage, GetMessages, Message, MessageId, ReactionType,
};

use super::control;
use super::geo_bank::{self as bank, ATTRIBUTION, Bank, Spot, Verdict};
use super::geo_store::{self as store, Status};
use super::points::{Cap, Outcome, Source};
use super::rules_text::{self, GeoRules};
use super::sudoku_gen::Rng;

/// How often the task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Rounds `/geo` and the day's list look back over.
const RECENT: usize = 20;

const COLOUR: u32 = 0x1ABC9C;
const TICK_MARK: &str = "✅";
const CARD_FOOTER: &str = "Type a state or a town · !hint · !skip";
const LEDGER_KEY: &str = "geo:";

/// The button that says somebody wants to play the next match.
pub const READY_ID: &str = "geoready:";

pub const HINT_WORD: &str = "!hint";
pub const SKIP_WORD: &str = "!skip";

/// The channel the game plays in unless a mod moves it: 🗺️ geo-guess.
/// Zero until the channel exists on the server, so the game stays off until a
/// mod points `VIZIER_GEO_CHANNEL` at a real room.
pub const HOME_CHANNEL: u64 = 0;

// --- settings --------------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_GEO", false)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_GEO_CHANNEL").or(Some(HOME_CHANNEL)).filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

/// The game's channel when the game is on AND there are places to play with.
/// No bank, no game: the bot never shows a photo it can't judge.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on() && bank::bank().is_some())
}

/// How many places one match runs before it is scored. Five is the format the
/// game is named after.
pub fn match_rounds() -> i64 {
    control::number("VIZIER_GEO_MATCH_ROUNDS", 5).clamp(1, 50) as i64
}

/// The fewest people who must press Ready before a match may start.
pub fn min_players() -> usize {
    control::number("VIZIER_GEO_MIN_PLAYERS", 2).clamp(1, 20) as usize
}

/// The rest between matches. Nothing starts before it is up, however many are
/// ready — the break is there so people can arrive, not just so they can skip it.
pub fn break_minutes() -> i64 {
    control::number("VIZIER_GEO_BREAK_MINUTES", 2).clamp(0, 240) as i64
}

/// The match prize: house points for first and second, the way Guess the Movie
/// pays. A single place pays no house points at all.
pub fn win_points() -> i64 {
    control::number("VIZIER_POINTS_GEO_WIN", 5).min(100) as i64
}

pub fn second_points() -> i64 {
    control::number("VIZIER_POINTS_GEO_SECOND", 2).min(100) as i64
}

/// How long a round may sit untouched before the bot moves on by itself.
fn idle_minutes() -> i64 {
    control::number("VIZIER_GEO_IDLE_MINUTES", 10).clamp(1, 1440) as i64
}

/// How long the same photo is held back.
fn no_repeat_days() -> i64 {
    control::number("VIZIER_GEO_NO_REPEAT_DAYS", 30).min(365) as i64
}

fn daily_cap() -> Option<i64> {
    match Source::Geo.cap() {
        Cap::PerDay(n) => Some(n),
        _ => None,
    }
}

/// How many messages from other people have to land under the card before it
/// follows the conversation down.
fn bump_messages() -> u64 {
    control::number("VIZIER_GEO_BUMP_MESSAGES", 5).clamp(1, 100)
}

/// And how long between two of those moves, however busy the channel is.
fn bump_seconds() -> i64 {
    control::number("VIZIER_GEO_BUMP_SECONDS", 120).clamp(10, 3600) as i64
}

/// Everything `/geohelp` and the House Cup posts say about the game.
pub fn geo_rules() -> GeoRules {
    let bank = bank::bank();
    GeoRules {
        channel: live_channel(),
        cap: daily_cap(),
        idle_minutes: idle_minutes(),
        no_repeat_days: no_repeat_days(),
        match_rounds: match_rounds(),
        min_players: min_players(),
        break_minutes: break_minutes(),
        win_points: win_points(),
        second_points: second_points(),
        places: bank.map(Bank::count),
        states: bank.map(|b| b.playable_states().len()),
    }
}

// --- words -----------------------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

fn plural_u(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "48 s", "7 min 41 s", "1 h 04 min".
pub fn spent_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{} s", s),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// What a round pays once a hint has been taken off it — never below one, so a
/// hinted round is still worth taking.
pub fn worth_after_hint(points: i64, hinted: bool) -> i64 {
    if hinted { (points - 1).max(1) } else { points }
}

/// How near a guess landed, in words the card can show.
pub fn near_words(verdict: Verdict) -> String {
    match verdict {
        Verdict::Wrong => String::new(),
        Verdict::State => "the right state".to_string(),
        Verdict::Near(km) => format!("{:.0} km away", km),
        Verdict::Bullseye(km) if km < 1.0 => "right on it".to_string(),
        Verdict::Bullseye(km) => format!("{:.0} km away", km),
    }
}

pub fn card_title(round: i64) -> String {
    format!("🗺️ Where in India is this? · round {}", round)
}

/// The line under the photo: what it is worth, and the hint if one is out.
pub fn card_text(worth: i64, hint: Option<(u64, String)>, open_secs: i64) -> String {
    let mut text = String::new();
    text.push_str("Name the **state** for 2, or the **town** for up to 5.\n");
    if let Some((by, line)) = hint {
        text.push_str(&format!("💡 <@{}> asked for a hint: {}\n", by, line));
    }
    text.push_str(&format!("-# Worth **{}** · up {}", plural(worth, "point", "points"), spent_words(open_secs)));
    text
}

/// The line when somebody takes a round.
pub fn won_text(user: u64, badge: &str, guess: &str, verdict: Verdict, answer: &str, worth: i64, secs: i64) -> String {
    let near = near_words(verdict);
    let praise = verdict.praise();
    format!(
        "✅ <@{}> {} — **{}** ({}, {}) in {}. It was {}. **+{}**",
        user,
        if badge.is_empty() { "takes it".to_string() } else { format!("· {}", badge) },
        guess,
        praise,
        near,
        spent_words(secs),
        answer,
        worth
    )
}

/// The line when a round ends with nobody taking it.
pub fn ended_text(row: &store::Row, answer: &str) -> String {
    match row.status {
        Status::Skipped => format!("⏭️ Passed. It was {}.", answer),
        _ => format!("🕰️ Nobody placed it. It was {}.", answer),
    }
}

/// How long between two of the small refusals — a `!hint` already out, a
/// `!skip` too soon. The channel is social; the same reminder five times is
/// noise.
pub fn nag_due(last_ms: i64, now_ms: i64) -> bool {
    now_ms - last_ms >= 30_000
}

/// Whether a round has been sitting untouched long enough for the bot to move
/// on by itself.
pub fn stale(posted_ts: i64, now: i64, idle_minutes: i64) -> bool {
    now - posted_ts >= idle_minutes.max(1) * 60
}

// --- picking a place -------------------------------------------------------------------------

/// Draws the next place: a STATE first, evenly, and only then a photo inside
/// it.
///
/// Drawing the photo directly would hand the game to whoever noticed that most
/// of the bank is Tamil Nadu. Drawing the state first makes every state as
/// likely as every other, whether it holds thirty thousand frames or sixty.
///
/// States already played in this match are held back, and so are photos seen
/// recently — but both give way rather than fail: a match longer than the bank
/// is wide will repeat a state before it posts nothing.
pub fn pick<'a>(bank: &'a Bank, used_spots: &HashSet<String>, states_played: &[String], rng: &mut Rng) -> Option<&'a Spot> {
    let all = bank.playable_states();
    if all.is_empty() {
        return None;
    }
    let fresh: Vec<&String> = all.iter().filter(|s| !states_played.contains(s)).collect();
    let pool: Vec<&String> = if fresh.is_empty() { all.iter().collect() } else { fresh };
    let state = pool[rng.below(pool.len())];

    let spots = bank.spots_in(state);
    if spots.is_empty() {
        return None;
    }
    let unseen: Vec<&&Spot> = spots.iter().filter(|s| !used_spots.contains(&s.id)).collect();
    if unseen.is_empty() {
        return Some(spots[rng.below(spots.len())]);
    }
    Some(unseen[rng.below(unseen.len())])
}

// --- matches ---------------------------------------------------------------------------------

/// What a match pays each place. First is [`win_points`], second
/// [`second_points`], and everyone else nothing.
///
/// A TIE FOR FIRST pays every tied player the winner's share and skips second
/// altogether: splitting it would make a draw worth less than a win for no
/// reason anybody could see, and paying second to a third player behind two
/// joint winners reads as a mistake. A tie for SECOND pays them all the
/// runner-up share, for the same reason. The rule Guess the Movie settled on.
pub fn prize_table(scores: &[(u64, i64)], win: i64, second: i64) -> Vec<(u64, i64, i64)> {
    let Some((_, top)) = scores.first().copied() else { return Vec::new() };
    if top <= 0 {
        return Vec::new();
    }
    let winners: Vec<u64> = scores.iter().filter(|(_, n)| *n == top).map(|(u, _)| *u).collect();
    let mut out: Vec<(u64, i64, i64)> = winners.iter().map(|u| (*u, 1, win)).collect();
    if winners.len() == 1 {
        if let Some((_, runner)) = scores.iter().find(|(_, n)| *n < top).copied() {
            if runner > 0 && second > 0 {
                out.extend(scores.iter().filter(|(_, n)| *n == runner).map(|(u, _)| (*u, 2, second)));
            }
        }
    }
    out
}

/// Whether a match may start: enough people ready AND the break served out.
pub fn may_start(ready: usize, min: usize, now: i64, ready_from: i64) -> bool {
    ready >= min.max(1) && now >= ready_from
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MatchResult {
    pub id: i64,
    pub rounds: i64,
    /// (who, their score, their place, house points actually paid)
    pub places: Vec<(u64, i64, i64, i64)>,
}

pub fn result_line(r: &MatchResult) -> String {
    if r.places.is_empty() {
        return "🗺️ Match over — nobody placed a single one.".to_string();
    }
    let mut text = String::from("🗺️ **Match over.** ");
    let mut parts = Vec::new();
    for (user, score, place, paid) in &r.places {
        let medal = if *place == 1 { "🥇" } else { "🥈" };
        let pay = if *paid > 0 { format!(", +{} house points", paid) } else { String::new() };
        parts.push(format!("{} <@{}> — {}{}", medal, user, plural(*score, "point", "points"), pay));
    }
    text.push_str(&parts.join(" · "));
    text
}

pub fn scoreboard_line(scores: &[(u64, i64)], played: i64, rounds: i64) -> String {
    if scores.is_empty() {
        return format!("-# {} of {} · nobody on the board yet", played, rounds);
    }
    let board = scores.iter().take(3).map(|(u, n)| format!("<@{}> {}", u, n)).collect::<Vec<_>>().join(" · ");
    format!("-# {} of {} · {}", played, rounds, board)
}

/// The card between matches.
pub fn break_text(ready: &[u64], min: usize, rounds: i64, now: i64, ready_from: i64, last: Option<&MatchResult>) -> String {
    let mut text = String::from("# 🗺️ Geo\n");
    if let Some(last) = last {
        text.push_str(&format!("{}\n", result_line(last)));
    }
    text.push_str(&format!("Next match: **{}**, anywhere in India. Press **I'm ready** to play.\n", plural(rounds, "place", "places")));
    if ready.is_empty() {
        text.push_str(&format!("Nobody's ready yet — **{}** needed to start.\n", min));
    } else {
        let who = ready.iter().map(|u| format!("<@{}>", u)).collect::<Vec<_>>().join(" · ");
        text.push_str(&format!("Ready: {} ({} of {})\n", who, ready.len(), min.max(1)));
    }
    let wait = ready_from - now;
    if wait > 0 {
        text.push_str(&format!("-# Starting in {}", spent_words(wait)));
    } else if ready.len() < min.max(1) {
        text.push_str(&format!("-# Waiting for {}", plural_u(min.max(1) - ready.len(), "more player", "more players")));
    } else {
        text.push_str("-# Starting now");
    }
    text
}

// --- shared state ----------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    /// (channel, message) of the round card. The ONE card the channel has.
    card: Option<(u64, u64)>,
    /// Cards a delete failed on, tried again until they are gone.
    orphans: Vec<(u64, u64)>,
    /// Messages from other people that have landed under the card.
    others_since_card: u64,
    /// When the card last moved down.
    last_bump_ms: i64,
    /// Someone deleted the card.
    card_gone: bool,
    /// The card is out of date (a hint went onto it).
    dirty: bool,
    /// A round just ended: the task says how and puts the next one up.
    ended: Option<i64>,
    /// A mod asked for a new round, with no hint needed first.
    admin_skip: Option<u64>,
    /// When the bot last answered a `!hint` already out, or a `!skip` too soon.
    last_nag_ms: i64,
    /// The break card as it was last drawn, so a countdown that has not changed
    /// in words does not cost an edit every second.
    break_shown: String,
    /// The match just scored, shown on the break card that follows it.
    last_result: Option<MatchResult>,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// Every message in the game's channel, the bot's own included. Only other
/// people's messages count towards burying the card.
pub fn note_message(ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != channel_setting() {
        return;
    }
    let mine = msg.author.id == ctx.cache.current_user().id;
    let mut s = SHARED.lock();
    if !mine && s.card.is_some_and(|(_, card)| msg.id.get() > card) {
        s.others_since_card += 1;
    }
}

/// A deleted card is posted again.
pub fn on_delete(channel: ChannelId, id: MessageId) {
    if Some(channel.get()) != channel_setting() {
        return;
    }
    let mut s = SHARED.lock();
    if s.card.map(|(_, m)| m) == Some(id.get()) {
        s.card_gone = true;
    }
}

// --- Discord helpers ---------------------------------------------------------------------------

async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, serenity::Error> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(result) => result,
        Err(_) => Err(serenity::Error::Other("no answer from Discord in time")),
    }
}

fn is_gone(err: &serenity::Error) -> bool {
    match err {
        serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(r)) => r.status_code.as_u16() == 404 || r.error.code == 10008,
        _ => false,
    }
}

async fn exists(ctx: &Context, channel: u64, id: u64) -> bool {
    match call(ctx.http.get_message(ChannelId::new(channel), MessageId::new(id))).await {
        Ok(_) => true,
        Err(err) => !is_gone(&err),
    }
}

fn is_safe_corner(ctx: &Context, channel: u64) -> bool {
    if channel == super::weekly::SAFE_CORNER {
        return true;
    }
    let name = ctx
        .cache
        .guilds()
        .iter()
        .find_map(|g| ctx.cache.guild(*g).and_then(|guild| guild.channels.get(&ChannelId::new(channel)).map(|c| c.name.to_lowercase())));
    name.is_some_and(|n| n.contains("safe-corner"))
}

fn active_channel(ctx: &Context) -> Option<u64> {
    live_channel().filter(|c| !is_safe_corner(ctx, *c))
}

/// Takes a card away. A delete that fails is remembered rather than shrugged
/// off: two cards in the channel would be two rounds as far as anyone reading
/// can tell.
async fn delete(ctx: &Context, channel: u64, message: u64) {
    match call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        Ok(()) => {}
        Err(err) if is_gone(&err) => {}
        Err(err) => {
            tracing::warn!("geo: card {} not deleted ({}) — trying again", message, err);
            let mut s = SHARED.lock();
            if !s.orphans.contains(&(channel, message)) {
                s.orphans.push((channel, message));
            }
        }
    }
}

async fn clear_orphans(ctx: &Context) {
    let waiting = std::mem::take(&mut SHARED.lock().orphans);
    for (channel, message) in waiting {
        delete(ctx, channel, message).await;
    }
}

/// A plain line in the channel, mentions never pinging anybody.
async fn say(ctx: &Context, channel: u64, text: String) {
    let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        tracing::warn!("geo: line not posted in {}: {}", channel, err);
    }
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), key, value);
    }
}

/// The house crest and name of whoever won.
fn badge_of(user: u64) -> String {
    if super::house::opted_out(user) {
        return "🧙 Muggle".to_string();
    }
    super::house::house_of(user).map(|h| format!("{} {}", h.crest, h.name)).unwrap_or_default()
}

/// Mods are in no house, but they may still play; the ledger simply refuses
/// their points.
fn may_play(user: u64) -> bool {
    if super::house::opted_out(user) {
        return false;
    }
    super::house::house_of(user).is_some() || super::admin_ids().contains(&user)
}

fn take_nag() -> bool {
    let now = Utc::now().timestamp_millis();
    let mut s = SHARED.lock();
    if !nag_due(s.last_nag_ms, now) {
        return false;
    }
    s.last_nag_ms = now;
    true
}

fn with_db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> Option<T> {
    store::db().map(|db| f(&db.lock()))
}

/// The match the channel is in, opening a fresh break when there is none.
fn match_now(channel: u64, now: i64) -> Option<store::Match> {
    let live = with_db(store::live_match).flatten();
    match live {
        Some(m) if m.channel == channel => Some(m),
        // The game moved channels: the old match is abandoned rather than
        // carried into a room it was not played in.
        Some(stale) => {
            let _ = with_db(|c| store::end_match(c, stale.id, now));
            None
        }
        None => None,
    }
    .or_else(|| {
        let from = now + break_minutes() * 60;
        let m = with_db(|c| store::open_match(c, match_rounds(), channel, now, from))?.ok()?;
        tracing::info!("geo: match {} open - {} places, break until {}", m.id, m.rounds, from);
        Some(m)
    })
}

fn ready_now(match_id: i64) -> Vec<u64> {
    with_db(|c| store::ready_list(c, match_id)).unwrap_or_default()
}

fn live_row() -> Option<store::Row> {
    with_db(store::live).flatten()
}

fn day_solves_now() -> Vec<store::Solve> {
    let day = super::points::ist_day(Utc::now().timestamp());
    with_db(|c| store::day_solves(c, &day)).unwrap_or_default()
}

pub fn month_ends(day: &str) -> (String, String) {
    let month = day.get(0..7).unwrap_or(day);
    (format!("{}-01", month), format!("{}-31", month))
}

fn boards_now() -> (Vec<store::Tally>, Vec<store::Tally>) {
    let day = super::points::ist_day(Utc::now().timestamp());
    let (from, to) = month_ends(&day);
    let today = with_db(|c| store::tally_between(c, &day, &day)).unwrap_or_default();
    let month = with_db(|c| store::tally_between(c, &from, &to)).unwrap_or_default();
    (today, month)
}

/// The photo a round was set at, when the bank still holds it.
fn spot_of(row: &store::Row) -> Option<&'static Spot> {
    bank::bank()?.spots().iter().find(|s| s.id == row.spot_id)
}

/// How the answer is stated when a round ends. The bank's own wording where
/// the photo is still in it; the state alone once it has been rebuilt away.
fn answer_of(row: &store::Row) -> String {
    match (bank::bank(), spot_of(row)) {
        (Some(bank), Some(spot)) => bank.answer_line(spot),
        _ => format!("**{}**", row.state),
    }
}

// --- the card --------------------------------------------------------------------------------

fn picture_name(round: i64) -> String {
    format!("geo-{}.jpg", round)
}

/// The photo for a round. Sent exactly as it sits on disk: these are already
/// 1280x720, and re-encoding a photograph to say nothing new about it only
/// costs the signboards people are trying to read.
async fn card_picture(row: &store::Row) -> Option<CreateAttachment> {
    let bank = bank::bank()?;
    let path = bank.path(spot_of(row)?);
    let name = picture_name(row.id);
    let bytes = tokio::task::spawn_blocking(move || std::fs::read(path).ok()).await.ok()??;
    Some(CreateAttachment::bytes(bytes, name))
}

fn hint_shown(row: &store::Row) -> Option<(u64, String)> {
    let by = row.hint_by?;
    let line = spot_of(row).and_then(|spot| bank::bank().map(|b| b.hint_line(spot)))?;
    Some((by, line))
}

fn card_embed(row: &store::Row, has_picture: bool, now: i64) -> CreateEmbed {
    let worth = worth_after_hint(Verdict::Bullseye(0.0).worth(), row.hinted());
    let mut embed = CreateEmbed::new()
        .title(card_title(row.id))
        .description(card_text(worth, hint_shown(row), now - row.posted_ts))
        .colour(COLOUR)
        .footer(CreateEmbedFooter::new(CARD_FOOTER));
    if has_picture {
        embed = embed.image(format!("attachment://{}", picture_name(row.id)));
    }
    embed
}

async fn card_message(row: &store::Row, now: i64) -> CreateMessage {
    let picture = card_picture(row).await;
    let mut message = CreateMessage::new().embed(card_embed(row, picture.is_some(), now)).allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = picture {
        message = message.add_file(file);
    }
    message
}

/// Posts a card and remembers it.
async fn place_card(ctx: &Context, channel: u64, message: CreateMessage, replace: bool, moved: bool) -> Option<u64> {
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let id = posted.id.get();
            let old = {
                let mut s = SHARED.lock();
                s.dirty = false;
                s.card_gone = false;
                s.others_since_card = 0;
                if moved {
                    s.last_bump_ms = Utc::now().timestamp_millis();
                }
                s.card.replace((channel, id))
            };
            meta_set("card", &format!("{}:{}", channel, id));
            if let Some((c, m)) = old.filter(|(_, m)| *m != id && replace) {
                delete(ctx, c, m).await;
            }
            Some(id)
        }
        Err(err) => {
            tracing::warn!("geo: card not posted in {}: {}", channel, err);
            None
        }
    }
}

async fn drop_card(ctx: &Context) {
    let card = SHARED.lock().card.take();
    if let Some((channel, message)) = card {
        delete(ctx, channel, message).await;
    }
    meta_set("card", "");
}

async fn post_round(ctx: &Context, channel: u64, match_id: i64) -> Option<store::Row> {
    let bank = bank::bank()?;
    let now = Utc::now().timestamp();
    let since = now - no_repeat_days() * 86_400;
    let used = with_db(|c| store::spots_since(c, since)).unwrap_or_default();
    let played = with_db(|c| store::states_in_match(c, match_id)).unwrap_or_default();
    let mut rng = Rng::fresh();
    let spot = pick(bank, &used, &played, &mut rng)?;
    let row = with_db(|c| store::add_round(c, &spot.id, &spot.state, spot.lat, spot.lon, channel, now))?.ok()?;
    let _ = with_db(|c| store::claim_round(c, match_id, row.id));
    let message = card_message(&row, now).await;
    let posted = place_card(ctx, channel, message, true, false).await?;
    let _ = with_db(|c| store::set_message(c, row.id, posted));
    tracing::info!("geo: round {} up in {} — {} ({})", row.id, channel, spot.state, spot.id);
    store::db().map(|db| store::get(&db.lock(), row.id)).flatten()
}

async fn edit_card(ctx: &Context, row: &store::Row, now: i64) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let picture = card_picture(row).await;
    let mut edit = EditMessage::new().embed(card_embed(row, picture.is_some(), now)).allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = picture {
        edit = edit.attachments(EditAttachments::new().add(file));
    }
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        if is_gone(&err) {
            SHARED.lock().card_gone = true;
        }
        tracing::warn!("geo: card not edited: {}", err);
    } else {
        SHARED.lock().dirty = false;
    }
}

/// Whether the card should follow the conversation down: enough has been said
/// under it, and the last move is far enough behind.
pub fn bump_due(others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> bool {
    others >= needed.max(1) && since_bump_ms >= every_secs.max(1) * 1000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Nothing,
    Post,
    Move,
    Edit,
}

pub fn card_plan(has_card: bool, right_channel: bool, gone: bool, dirty: bool, others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> CardAction {
    if !has_card || gone || !right_channel {
        return CardAction::Post;
    }
    if bump_due(others, since_bump_ms, needed, every_secs) {
        return CardAction::Move;
    }
    if dirty {
        return CardAction::Edit;
    }
    CardAction::Nothing
}

async fn tend_card(ctx: &Context, channel: u64, row: &store::Row, now: i64) {
    let (card, gone, dirty, others, last_bump) = {
        let s = SHARED.lock();
        (s.card, s.card_gone, s.dirty, s.others_since_card, s.last_bump_ms)
    };
    let since_bump = Utc::now().timestamp_millis() - last_bump;
    let right_channel = card.is_some_and(|(c, _)| c == channel);
    let plan = card_plan(card.is_some(), right_channel, gone, dirty, others, since_bump, bump_messages(), bump_seconds());
    match plan {
        CardAction::Nothing => {}
        CardAction::Edit => edit_card(ctx, row, now).await,
        CardAction::Post | CardAction::Move => {
            // A card that is only being moved is checked first: if it is still
            // there and nothing has actually buried it, moving would be churn.
            if plan == CardAction::Post
                && let Some((c, m)) = card
                && exists(ctx, c, m).await
                && right_channel
                && !gone
            {
                return;
            }
            let message = card_message(row, now).await;
            if let Some(id) = place_card(ctx, channel, message, true, plan == CardAction::Move).await {
                let _ = with_db(|c| store::set_message(c, row.id, id));
            }
        }
    }
}

// --- the break card --------------------------------------------------------------------------

fn break_message(m: &store::Match, ready: &[u64], last: Option<&MatchResult>, now: i64) -> CreateMessage {
    let text = break_text(ready, min_players(), m.rounds, now, m.ready_from, last);
    let button = CreateButton::new(format!("{}{}", READY_ID, m.id)).label("I'm ready").style(ButtonStyle::Success).emoji('🗺');
    CreateMessage::new().content(text).components(vec![CreateActionRow::Buttons(vec![button])]).allowed_mentions(CreateAllowedMentions::new())
}

async fn tend_break_card(ctx: &Context, channel: u64, m: &store::Match, now: i64) {
    let ready = ready_now(m.id);
    let last = SHARED.lock().last_result.clone();
    let text = break_text(&ready, min_players(), m.rounds, now, m.ready_from, last.as_ref());
    let (card, shown, gone) = {
        let s = SHARED.lock();
        (s.card, s.break_shown.clone(), s.card_gone)
    };
    let right_channel = card.is_some_and(|(c, _)| c == channel);
    if card.is_none() || gone || !right_channel {
        let message = break_message(m, &ready, last.as_ref(), now);
        if place_card(ctx, channel, message, true, false).await.is_some() {
            SHARED.lock().break_shown = text;
        }
        return;
    }
    // The countdown only costs an edit when its WORDS change, not every second.
    if text == shown {
        return;
    }
    let Some((c, message)) = card else { return };
    let button = CreateButton::new(format!("{}{}", READY_ID, m.id)).label("I'm ready").style(ButtonStyle::Success).emoji('🗺');
    let edit = EditMessage::new().content(&text).components(vec![CreateActionRow::Buttons(vec![button])]);
    if let Err(err) = call(ChannelId::new(c).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::debug!("geo: break card not edited: {}", err);
    } else {
        SHARED.lock().break_shown = text;
    }
}

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let reply = CreateInteractionResponseMessage::new().content(text).ephemeral(true);
    if let Err(err) = call(component.create_response(&ctx.http, CreateInteractionResponse::Message(reply))).await {
        tracing::debug!("geo: ready not answered: {}", err);
    }
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let Some(match_id) = component.data.custom_id.strip_prefix(READY_ID).and_then(|v| v.parse::<i64>().ok()) else {
        return;
    };
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(m) = with_db(|c| store::get_match(c, match_id)).flatten() else {
        whisper(ctx, component, "That match is over.").await;
        return;
    };
    if m.status != store::MatchStatus::Break {
        whisper(ctx, component, "That match has already started.").await;
        return;
    }
    let held = with_db(|c| store::is_ready(c, match_id, user)).unwrap_or(false);
    if held {
        let _ = with_db(|c| store::unready(c, match_id, user));
        whisper(ctx, component, "Taken off the list.").await;
    } else {
        let _ = with_db(|c| store::mark_ready(c, match_id, user, now));
        let waiting = ready_now(match_id).len();
        whisper(ctx, component, format!("You're in — {} of {} ready.", waiting, min_players())).await;
    }
    SHARED.lock().break_shown.clear();
}

async fn finish_match(ctx: &Context, channel: u64, m: &store::Match, now: i64) {
    if !with_db(|c| store::end_match(c, m.id, now)).and_then(Result::ok).unwrap_or(false) {
        return;
    }
    let scores = with_db(|c| store::match_scores(c, m.id)).unwrap_or_default();
    let mut places = Vec::new();
    for (user, place, points) in prize_table(&scores, win_points(), second_points()) {
        let score = scores.iter().find(|(u, _)| *u == user).map(|(_, n)| *n).unwrap_or(0);
        let paid = if may_play(user) {
            let reason = format!("Geo: match {}, place {}", m.id, place);
            let key = format!("{}match:{}:{}", LEDGER_KEY, m.id, user);
            match super::house::award_person(user, Source::Geo, points, &reason, None, Some(key), None) {
                Some((_, Outcome::Granted(n))) => n,
                _ => 0,
            }
        } else {
            0
        };
        let _ = with_db(|c| store::add_prize(c, m.id, user, place, score, points, paid, now));
        places.push((user, score, place, paid));
    }
    let result = MatchResult { id: m.id, rounds: m.rounds, places };
    tracing::info!("geo: match {} scored - {:?}", m.id, result.places);
    say(ctx, channel, result_line(&result)).await;
    {
        let mut s = SHARED.lock();
        s.last_result = Some(result);
        s.break_shown.clear();
    }
    drop_card(ctx).await;
}

// --- the task --------------------------------------------------------------------------------

/// Starts the task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting(), bank::bank().is_some()) {
        (true, Some(c), true) => tracing::info!("geo: playing in {}", c),
        (true, Some(_), false) => tracing::warn!("geo: on, but there is no place bank — the game stays off"),
        (true, None, _) => tracing::info!("geo: on, but VIZIER_GEO_CHANNEL isn't set"),
        (false, _, _) => tracing::info!("geo: VIZIER_GEO is off"),
    }
    tokio::spawn(run(ctx));
}

/// After a start: pick up the card the last run left, and the round it was for.
async fn recover(ctx: &Context) -> Option<store::Row> {
    let channel = active_channel(ctx);
    let db = store::db()?;
    let (old_card, live) = {
        let conn = db.lock();
        (store::meta_get(&conn, "card"), store::live(&conn))
    };
    let old_card = old_card.as_deref().and_then(|v| v.split_once(':')).and_then(|(c, m)| Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?)));
    if let Some((c, m)) = old_card {
        // What was said while the bot was away doesn't count towards burying
        // the card: the channel is read from where it stands now.
        let buried = match call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().after(MessageId::new(m)).limit(100))).await {
            Ok(after) => after.iter().filter(|msg| msg.author.id != ctx.cache.current_user().id).count() as u64,
            Err(_) => 0,
        };
        let mut s = SHARED.lock();
        s.card = Some((c, m));
        s.others_since_card = buried;
        s.last_bump_ms = Utc::now().timestamp_millis();
    }
    if channel.is_none() {
        return None;
    }
    live
}

async fn run(ctx: Context) {
    let mut live = recover(&ctx).await;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        let (ended, admin_skip) = {
            let mut s = SHARED.lock();
            (s.ended.take(), s.admin_skip.take())
        };
        clear_orphans(&ctx).await;
        let Some(channel) = channel else {
            if SHARED.lock().card.is_some() {
                drop_card(&ctx).await;
            }
            live = None;
            continue;
        };

        // The match is the frame every round sits in: no match, no places.
        let Some(m) = match_now(channel, now) else { continue };
        if m.status == store::MatchStatus::Break {
            if live.is_some() {
                drop_card(&ctx).await;
                live = None;
            }
            let ready = ready_now(m.id);
            if may_start(ready.len(), min_players(), now, m.ready_from) {
                if with_db(|c| store::start_match(c, m.id, now)).and_then(Result::ok).unwrap_or(false) {
                    tracing::info!("geo: match {} started - {} places, {} ready", m.id, m.rounds, ready.len());
                    SHARED.lock().break_shown.clear();
                    drop_card(&ctx).await;
                }
                continue;
            }
            tend_break_card(&ctx, channel, &m, now).await;
            continue;
        }

        // The last place of the match has been settled: score it and open the
        // next break.
        if m.played >= m.rounds && live.is_none() {
            finish_match(&ctx, channel, &m, now).await;
            continue;
        }

        // A mod pressed on: the round is closed here, and told about below like
        // any other ending.
        let mut ending = ended;
        if let (Some(by), Some(row)) = (admin_skip, live.as_ref()) {
            let done = with_db(|c| store::skip(c, row.id, by, now)).and_then(Result::ok).unwrap_or(false);
            if done {
                tracing::info!("geo: round {} skipped by mod {}", row.id, by);
                ending = Some(row.id);
            }
        }

        // A round nobody has touched for long enough is given up on.
        if ending.is_none()
            && let Some(row) = live.as_ref().filter(|r| r.status == Status::Open)
            && stale(row.posted_ts, now, idle_minutes())
            && with_db(|c| store::expire(c, row.id, now)).and_then(Result::ok).unwrap_or(false)
        {
            tracing::info!("geo: round {} went stale", row.id);
            ending = Some(row.id);
        }

        if let Some(id) = ending {
            let row = with_db(|c| store::get(c, id)).flatten();
            if let Some(row) = row {
                announce_end(&ctx, channel, &row).await;
            }
            live = None;
            continue;
        }

        match live.as_ref() {
            Some(row) if row.status == Status::Open => tend_card(&ctx, channel, row, now).await,
            _ => live = post_round(&ctx, channel, m.id).await,
        }
    }
}

/// Says how a round ended, and takes its card away.
async fn announce_end(ctx: &Context, channel: u64, row: &store::Row) {
    let answer = answer_of(row);
    let line = match (row.status, row.winner) {
        (Status::Solved, Some(user)) => {
            let verdict = match (row.verdict.as_deref(), row.km) {
                (Some("bullseye"), Some(km)) => Verdict::Bullseye(km),
                (Some("near"), Some(km)) => Verdict::Near(km),
                _ => Verdict::State,
            };
            won_text(
                user,
                &badge_of(user),
                row.winning_guess.as_deref().unwrap_or(""),
                verdict,
                &answer,
                row.won,
                row.seconds.unwrap_or(0),
            )
        }
        _ => ended_text(row, &answer),
    };
    let scores = row.match_id.and_then(|id| with_db(|c| store::match_scores(c, id)));
    let played = row.match_id.and_then(|id| with_db(|c| store::get_match(c, id)).flatten());
    let mut text = line;
    if let (Some(scores), Some(m)) = (scores, played) {
        text.push('\n');
        text.push_str(&scoreboard_line(&scores, m.played, m.rounds));
    }
    say(ctx, channel, text).await;
    drop_card(ctx).await;
}

// --- what people type ------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum Typed {
    Hint,
    Skip,
    Guess(String),
    Nothing,
}

/// What a message in the channel amounts to. A name the bank has never heard
/// of is `Nothing`, not a wrong guess: the room is a chatty one, and the bot
/// does not react to conversation.
pub fn read_message(text: &str, spot: &Spot, bank: &Bank) -> Typed {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(HINT_WORD) {
        return Typed::Hint;
    }
    if trimmed.eq_ignore_ascii_case(SKIP_WORD) {
        return Typed::Skip;
    }
    let Some(folded) = bank::tidy(trimmed) else { return Typed::Nothing };
    if bank.judge(spot, &folded).won() {
        return Typed::Guess(folded);
    }
    Typed::Nothing
}

pub async fn on_message(ctx: &Context, msg: &Message) {
    let Some(channel) = live_channel() else { return };
    if msg.channel_id.get() != channel || msg.author.bot {
        return;
    }
    let Some(bank) = bank::bank() else { return };
    let Some(row) = live_row().filter(|r| r.status == Status::Open) else { return };
    let Some(spot) = spot_of(&row) else { return };
    match read_message(&msg.content, spot, bank) {
        Typed::Hint => hint_asked(ctx, msg, &row, channel).await,
        Typed::Skip => skip_asked(ctx, msg, &row, channel).await,
        Typed::Guess(guess) => guessed(ctx, msg, &row, spot, bank, &guess).await,
        Typed::Nothing => {}
    }
}

/// A guess that places the photo. Only the first one counts; the rest are told
/// nothing, because they can see the winner's line for themselves.
async fn guessed(ctx: &Context, msg: &Message, row: &store::Row, spot: &Spot, bank: &Bank, guess: &str) {
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let verdict = bank.judge(spot, guess);
    if !verdict.won() {
        return;
    }
    let worth = worth_after_hint(verdict.worth(), row.hinted());
    let (key, km) = match verdict {
        Verdict::Bullseye(km) => ("bullseye", Some(km)),
        Verdict::Near(km) => ("near", Some(km)),
        _ => ("state", None),
    };
    let won = {
        let conn = db.lock();
        store::claim(&conn, row.id, user, guess, key, km, worth, now).unwrap_or(false)
    };
    if !won {
        return;
    }
    if let Err(err) = call(msg.react(&ctx.http, ReactionType::Unicode(TICK_MARK.to_string()))).await {
        tracing::debug!("geo: tick not added to {}: {}", msg.id, err);
    }
    let seconds = now - row.posted_ts;
    // A place pays NO house points on its own: house points are the match
    // prize, see `finish_match`, so a long match cannot out-pay winning the
    // thing and the daily limit means what it says. Geo points are the game's
    // own score - per round, uncapped, everyone, mods and Muggles included.
    {
        let conn = db.lock();
        let _ = store::add_solve(&conn, &super::points::ist_day(now), user, row.id, worth, key, km, guess, seconds, now);
    }
    tracing::info!("geo: round {} won by {} with {:?} ({}) in {}s for {} geo points", row.id, user, guess, key, seconds, worth);
    SHARED.lock().ended = Some(row.id);
}

/// `!hint`: once a round. The state's first letter and the quarter of the
/// country go onto the card, and it costs the round a point.
async fn hint_asked(ctx: &Context, msg: &Message, row: &store::Row, channel: u64) {
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let taken = with_db(|c| store::take_hint(c, row.id, user, now)).and_then(Result::ok).unwrap_or(false);
    if !taken {
        if take_nag() {
            say(ctx, channel, "💡 The hint is already out.".to_string()).await;
        }
        return;
    }
    let line = spot_of(row).and_then(|spot| bank::bank().map(|b| b.hint_line(spot))).unwrap_or_default();
    let worth = worth_after_hint(Verdict::Bullseye(0.0).worth(), true);
    SHARED.lock().dirty = true;
    tracing::info!("geo: round {} hinted by {}", row.id, user);
    say(ctx, channel, format!("💡 <@{}> asked. {} The round is now worth **{}**.", user, line, worth)).await;
}

/// `!skip`: only after a hint, so a round cannot be thrown away before anyone
/// has really looked at it.
async fn skip_asked(ctx: &Context, msg: &Message, row: &store::Row, channel: u64) {
    if !row.hinted() {
        if take_nag() {
            say(ctx, channel, "⏭️ Not yet — take the hint first.".to_string()).await;
        }
        return;
    }
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    if with_db(|c| store::skip(c, row.id, user, now)).and_then(Result::ok).unwrap_or(false) {
        tracing::info!("geo: round {} passed by {}", row.id, user);
        SHARED.lock().ended = Some(row.id);
    }
}

// --- the boards ------------------------------------------------------------------------------

pub fn place_of(rows: &[store::Tally], me: u64) -> Option<usize> {
    rows.iter().position(|r| r.user == me).map(|i| i + 1)
}

pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (1, 11) | (2, 12) | (3, 13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

pub fn mine_text(live: Option<&store::Row>, today: &[store::Tally], month: &[store::Tally], me: u64) -> String {
    let mut text = String::from("# 🗺️ Geo\n");
    match live {
        Some(row) => text.push_str(&format!("A place is up — round {}.\n", row.id)),
        None => text.push_str("Nothing up right now.\n"),
    }
    let mine_today = today.iter().find(|r| r.user == me);
    let mine_month = month.iter().find(|r| r.user == me);
    match mine_today {
        Some(t) => text.push_str(&format!(
            "Today: **{}** from {}, {} bullseye{}.\n",
            plural(t.points, "point", "points"),
            plural(t.solves, "place", "places"),
            t.bullseyes,
            if t.bullseyes == 1 { "" } else { "s" }
        )),
        None => text.push_str("Today: nothing yet.\n"),
    }
    if let Some(t) = mine_month {
        let place = place_of(month, me).map(|p| format!(" — {} this month", ordinal(p))).unwrap_or_default();
        text.push_str(&format!("This month: **{}**{}.\n", plural(t.points, "point", "points"), place));
    }
    text
}

pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("# 🗺️ Geo — {}\n", period);
    if rows.is_empty() {
        text.push_str("Nobody has placed one yet.\n");
        return text;
    }
    for (i, row) in rows.iter().take(10).enumerate() {
        let mark = if row.user == me { " ←" } else { "" };
        text.push_str(&format!(
            "{}. <@{}> — **{}** from {}{}\n",
            i + 1,
            row.user,
            plural(row.points, "point", "points"),
            plural(row.solves, "place", "places"),
            mark
        ));
    }
    if place_of(rows, me).is_some_and(|p| p > 10) {
        text.push_str(&format!("-# You're {}.\n", ordinal(place_of(rows, me).unwrap_or(0))));
    }
    text
}

pub fn month_label(day: &str) -> String {
    day.get(0..7).unwrap_or(day).to_string()
}

// --- commands --------------------------------------------------------------------------------

pub fn mine_builder() -> CreateCommand {
    CreateCommand::new("geo").description("The place that's up, and how you're doing")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("geotop").description("The Geo board").add_option(
        serenity::all::CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "today or this month")
            .add_string_choice("today", "today")
            .add_string_choice("month", "month")
            .required(false),
    )
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("geohelp").description("How Geo works")
}

pub fn skip_builder() -> CreateCommand {
    CreateCommand::new("geoskip").description("Mods: move on to the next place")
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("geostop").description("Mods: switch Geo off")
}

async fn reply(ctx: &Context, command: &CommandInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = call(command.create_response(&ctx.http, CreateInteractionResponse::Message(message))).await {
        tracing::debug!("geo: command not answered: {}", err);
    }
}

pub async fn mine_command(ctx: &Context, command: &CommandInteraction) {
    let me = command.user.id.get();
    let (today, month) = boards_now();
    let live = live_row();
    let text = mine_text(live.as_ref(), &today, &month, me);
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new())).await;
}

pub async fn top_command(ctx: &Context, command: &CommandInteraction) {
    let me = command.user.id.get();
    let wants_month = command.data.options.first().and_then(|o| o.value.as_str().map(|v| v == "month")).unwrap_or(false);
    let (today, month) = boards_now();
    let day = super::points::ist_day(Utc::now().timestamp());
    let (period, rows) = if wants_month { (month_label(&day), month) } else { ("today".to_string(), today) };
    let text = top_text(&period, &rows, me);
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

fn help_embed() -> CreateEmbed {
    CreateEmbed::new().title("🗺️ Geo").description(rules_text::geo_help_text(&geo_rules())).colour(COLOUR)
}

pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    reply(ctx, command, CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true)).await;
}

pub async fn skip_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        reply(ctx, command, CreateInteractionResponseMessage::new().content("Mods only.").ephemeral(true)).await;
        return;
    }
    if live_row().is_none() {
        reply(ctx, command, CreateInteractionResponseMessage::new().content("Nothing is up.").ephemeral(true)).await;
        return;
    }
    SHARED.lock().admin_skip = Some(user);
    reply(ctx, command, CreateInteractionResponseMessage::new().content("Moving on.").ephemeral(true)).await;
}

pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        reply(ctx, command, CreateInteractionResponseMessage::new().content("Mods only.").ephemeral(true)).await;
        return;
    }
    let text = match control::set("VIZIER_GEO", Some("off"), user) {
        Ok(()) => {
            tracing::info!("geo: /geostop by {}", user);
            "🛑 Geo is off. The card comes down in a moment; switch **Game on** back on in the panel to play again."
        }
        Err(err) => {
            tracing::warn!("geo: /geostop by {} failed: {}", user, err);
            "The setting wouldn't save — switch **Game on** off in the panel instead."
        }
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).ephemeral(true)).await;
}

/// The last few rounds, for the panel.
pub fn recent_rounds(limit: usize) -> Vec<store::Row> {
    with_db(|c| store::recent(c, limit.min(RECENT))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::geo_bank::tests::fixture;

    #[test]
    fn a_hinted_round_is_still_worth_taking() {
        assert_eq!(worth_after_hint(5, false), 5);
        assert_eq!(worth_after_hint(5, true), 4);
        assert_eq!(worth_after_hint(2, true), 1);
        assert_eq!(worth_after_hint(1, true), 1);
    }

    /// The bank leans hard towards a couple of states. Drawing the state first
    /// is what stops "Tamil Nadu" being a winning guess without looking.
    #[test]
    fn every_state_comes_up_however_lopsided_the_bank_is() {
        let bank = fixture();
        let mut seen = std::collections::HashMap::new();
        let mut rng = Rng::seeded(7);
        for _ in 0..400 {
            let spot = pick(&bank, &HashSet::new(), &[], &mut rng).expect("a place");
            *seen.entry(spot.state.clone()).or_insert(0) += 1;
        }
        assert_eq!(seen.len(), 2, "both states should come up: {seen:?}");
        for (state, n) in &seen {
            assert!((120..280).contains(n), "{state} came up {n} times in 400");
        }
    }

    #[test]
    fn a_state_already_played_this_match_is_held_back() {
        let bank = fixture();
        let mut rng = Rng::seeded(3);
        for _ in 0..20 {
            let spot = pick(&bank, &HashSet::new(), &["Telangana".to_string()], &mut rng).expect("a place");
            assert_eq!(spot.state, "Rajasthan");
        }
    }

    /// A match longer than the bank is wide has to repeat rather than stop.
    #[test]
    fn holding_states_back_gives_way_before_the_game_does() {
        let bank = fixture();
        let mut rng = Rng::seeded(5);
        let played = vec!["Telangana".to_string(), "Rajasthan".to_string()];
        assert!(pick(&bank, &HashSet::new(), &played, &mut rng).is_some());
    }

    #[test]
    fn a_photo_seen_recently_is_passed_over() {
        let bank = fixture();
        let mut rng = Rng::seeded(11);
        let used: HashSet<String> = ["kv1".to_string()].into_iter().collect();
        for _ in 0..20 {
            let spot = pick(&bank, &used, &["Rajasthan".to_string()], &mut rng).expect("a place");
            assert_eq!(spot.id, "kv1", "only kv1 is in Telangana, so it comes back rather than nothing");
        }
    }

    #[test]
    fn a_match_pays_first_and_second_and_nobody_else() {
        let scores = vec![(1u64, 14i64), (2, 9), (3, 4)];
        assert_eq!(prize_table(&scores, 5, 2), vec![(1, 1, 5), (2, 2, 2)]);
    }

    /// Joint winners take second place with them.
    #[test]
    fn a_tie_for_first_pays_both_and_skips_second() {
        let scores = vec![(1u64, 12i64), (2, 12), (3, 5)];
        assert_eq!(prize_table(&scores, 5, 2), vec![(1, 1, 5), (2, 1, 5)]);
    }

    #[test]
    fn an_empty_match_pays_nothing() {
        assert!(prize_table(&[], 5, 2).is_empty());
        assert!(prize_table(&[(1, 0)], 5, 2).is_empty());
    }

    #[test]
    fn a_match_needs_the_people_and_the_clock() {
        assert!(!may_start(1, 2, 100, 100), "not enough people");
        assert!(!may_start(2, 2, 90, 100), "break not served out");
        assert!(may_start(2, 2, 100, 100));
    }

    #[test]
    fn steering_words_are_read_before_guesses() {
        let bank = fixture();
        let spot = &bank.spots()[0];
        assert_eq!(read_message("!hint", spot, &bank), Typed::Hint);
        assert_eq!(read_message("!SKIP", spot, &bank), Typed::Skip);
        assert_eq!(read_message("Telangana", spot, &bank), Typed::Guess("telangana".to_string()));
        assert_eq!(read_message("what a lovely street", spot, &bank), Typed::Nothing);
        // Wrong state: nothing happens, the same as any other chat.
        assert_eq!(read_message("Rajasthan", spot, &bank), Typed::Nothing);
    }

    #[test]
    fn the_card_is_posted_moved_and_edited_when_it_should_be() {
        assert_eq!(card_plan(false, true, false, false, 0, 0, 5, 120), CardAction::Post);
        assert_eq!(card_plan(true, true, true, false, 0, 0, 5, 120), CardAction::Post);
        assert_eq!(card_plan(true, false, false, false, 0, 0, 5, 120), CardAction::Post);
        assert_eq!(card_plan(true, true, false, false, 9, 200_000, 5, 120), CardAction::Move);
        assert_eq!(card_plan(true, true, false, true, 0, 0, 5, 120), CardAction::Edit);
        assert_eq!(card_plan(true, true, false, false, 0, 0, 5, 120), CardAction::Nothing);
    }

    /// A busy channel must not make the card jump every few seconds.
    #[test]
    fn the_card_waits_out_its_cooldown_however_busy_the_room_is() {
        assert!(!bump_due(50, 1_000, 5, 120));
        assert!(bump_due(50, 200_000, 5, 120));
        assert!(!bump_due(1, 200_000, 5, 120));
    }

    #[test]
    fn near_words_say_how_close_it_was() {
        assert_eq!(near_words(Verdict::State), "the right state");
        assert_eq!(near_words(Verdict::Near(41.2)), "41 km away");
        assert_eq!(near_words(Verdict::Bullseye(0.4)), "right on it");
    }
}
