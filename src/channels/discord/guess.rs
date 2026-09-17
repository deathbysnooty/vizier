//! Guess the Word, in its own channel (`VIZIER_GUESS_CHANNEL`).
//!
//! There is ALWAYS a doodle waiting. Its card is the last message in the
//! channel - and like anagrams, and unlike sudoku or chess, people CAN type
//! there: guessing IS typing. The bot takes a word from the doodle bank, picks
//! one of the drawings somebody really made of it, inks it out onto a sheet of
//! paper and asks what it is.
//!
//! Any spelling the bank accepts wins, and a slip or two is forgiven on longer
//! words, so `gitar` takes a guitar. The first correct guess gets a ✅ on the
//! message, a line naming the winner, and the next doodle at once. Everything
//! else typed in the channel is left alone: the room is a social one and a ❌ on
//! every stray message would be noise.
//!
//! Two words steer a round. `!hint` puts a SECOND drawing of the same thing up
//! beside the first and gives away the word's first letter, once per round, and
//! takes a point off what the round pays (never below one); `!skip` opens up
//! only after a hint, reveals the word and pays nothing. A round nobody touches
//! goes stale by itself after `VIZIER_GUESS_IDLE_MINUTES` - a picture is quicker
//! to give up on than a word - so the channel is never stuck overnight.
//!
//! One task does all the posting ([`run`]), the way anagrams does: the message
//! handler only writes to the database and leaves a note in the shared state, so
//! two guesses landing together can never post two rounds. The same task keeps
//! the card near the bottom and puts it back if someone deletes it - but the
//! room is chatty, so the card only follows the conversation down once chat has
//! genuinely buried it ([`card_plan`]).

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, CommandInteraction, Context, CreateAllowedMentions, CreateAttachment, CreateCommand, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditAttachments, EditMessage, GetMessages, Message, MessageId,
    ReactionType,
};

use super::control;
use super::guess_bank::{self as bank, ATTRIBUTION, Bank, Doodle};
use super::guess_draw;
use super::guess_store::{self as store, Status};
use super::points::{Cap, Outcome, Source};
use super::rules_text::{self, GuessRules};
use super::sudoku_gen::Rng;

/// How often the task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Rounds `/guess` and the day's list look back over.
const RECENT: usize = 20;

const COLOUR: u32 = 0xE67E22;

/// The channel the game plays in unless a mod moves it: ❓guess-the-word.
pub const HOME_CHANNEL: u64 = 1_518_233_664_016_617_582;

/// What someone types to ask for a second drawing, or to pass on a round.
pub const HINT_WORD: &str = "!hint";
pub const SKIP_WORD: &str = "!skip";

/// How long between two "the hint is already out" or "not yet" replies. The
/// channel is a social one: the same reminder five times over is noise.
pub const NAG_EVERY_MS: i64 = 30_000;

/// The tick mark the winning message gets.
const TICK_MARK: &str = "✅";
/// What a round's ledger row is called. Its own prefix, so a guess row can
/// never be taken for any other game's.
pub const LEDGER_KEY: &str = "guess:round:";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_GUESS", false)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_GUESS_CHANNEL").or(Some(HOME_CHANNEL)).filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

/// The game's channel when the game is on AND there are doodles to play with.
/// No bank, no game: the bot never puts up a picture it can't judge.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on() && bank::bank().is_some())
}

/// What naming a doodle pays.
pub fn round_points() -> i64 {
    control::number("VIZIER_POINTS_GUESS", 2).min(100) as i64
}

/// How long a round may sit untouched before the bot moves on by itself.
fn idle_minutes() -> i64 {
    control::number("VIZIER_GUESS_IDLE_MINUTES", 15).clamp(1, 1440) as i64
}

/// How long the same word - and any single drawing of it - is held back.
fn no_repeat_days() -> i64 {
    control::number("VIZIER_GUESS_NO_REPEAT_DAYS", 14).min(365) as i64
}

fn daily_cap() -> Option<i64> {
    match Source::Guess.cap() {
        Cap::PerDay(n) => Some(n),
        _ => None,
    }
}

/// How many messages from other people have to land under the card before it
/// follows the conversation down.
fn bump_messages() -> u64 {
    control::number("VIZIER_GUESS_BUMP_MESSAGES", 5).clamp(1, 100)
}

/// And how long between two of those moves, however busy the channel is.
fn bump_seconds() -> i64 {
    control::number("VIZIER_GUESS_BUMP_SECONDS", 120).clamp(10, 3600) as i64
}

/// Everything `/guesshelp` and the House Cup posts say about the game.
pub fn guess_rules() -> GuessRules {
    GuessRules {
        channel: live_channel(),
        points: round_points(),
        cap: daily_cap(),
        idle_minutes: idle_minutes(),
        no_repeat_days: no_repeat_days(),
        words: bank::bank().map(Bank::word_count),
    }
}

// --- words ---------------------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
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

/// How long a round has been up, in round words.
pub fn open_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

/// What a round pays once the hint has been out: a point less, never below one,
/// so a hinted round is always still worth playing.
pub fn worth_after_hint(points: i64, hinted: bool) -> i64 {
    if hinted { (points - 1).max(1) } else { points.max(0) }
}

pub fn card_title(round: i64) -> String {
    format!("🎨 Guess the Word · Round #{}", round)
}

/// Everything the live card says about a round right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    /// What it is worth now, the hint already taken off.
    pub points: i64,
    /// Who asked for the hint, and the letter it gave away.
    pub hint: Option<(u64, char)>,
    pub open_secs: i64,
}

pub fn card_text(live: &Live) -> String {
    let mut text = format!("# What is this?\nworth **{}** · first to name it wins", plural(live.points, "point", "points"));
    if let Some((who, letter)) = live.hint {
        text.push_str(&format!(
            "\n💡 another drawing of the same thing, and it starts with **{}** — <@{}> asked, so the round is worth a point less",
            letter, who
        ));
    }
    text.push_str(&format!("\n-# ⏱️ up {} · just type what you think it is", open_words(live.open_secs)));
    text
}

pub const CARD_FOOTER: &str = "Type your guess here · !hint for a second drawing and the first letter · !skip once a hint is out · /guesshelp";

/// What the channel is told when a round is won.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Won {
    pub round: i64,
    pub winner: u64,
    /// The winner's crest and house, when they are in one.
    pub badge: String,
    /// What they typed, and what the drawing was.
    pub guess: String,
    pub word: String,
    /// The HOUSE points the ledger paid: zero once the day's cap is full, and
    /// zero for anyone with no house.
    pub points: i64,
    /// The GUESS points the round was worth, hint taken off and no cap: what the
    /// solve scored in the game itself, whatever the ledger did with it.
    pub worth: i64,
    /// What it would have paid without the hint.
    pub full_points: i64,
    /// Whether the winner is in a house at all.
    pub housed: bool,
    /// Their guess points today, this round included.
    pub tally: i64,
    pub hinted: bool,
    pub seconds: i64,
    /// "Aarav 3 · Meera 2", the day so far.
    pub today: String,
}

/// A solve that paid no house points still won the round, and the line says so
/// as a win: the guess points are the score that always counts.
pub fn won_text(w: &Won) -> String {
    let badge = if w.badge.is_empty() { String::new() } else { format!(" {}", w.badge) };
    // Nothing paid is never "no points": the round was won and the guess points
    // are the winner's either way. The aside says why the ledger sat it out —
    // the day's house points are full, or there is no house to pay.
    let why = match (w.points, w.housed) {
        (0, true) => Some("that's your house points for today, but the guess points still count"),
        (0, false) => Some("no house to pay, but the guess points still count"),
        _ => None,
    };
    let (scored, today) = match why {
        Some(_) => (plural(w.worth, "guess point", "guess points"), format!("{} today", w.tally)),
        None => (w.points.to_string(), format!("{} guess points today", w.tally)),
    };
    let mut text = format!("✅ <@{}>{} had it: **{}** · **+{}** · {}", w.winner, badge, w.word.to_uppercase(), scored, today);
    let mut notes = vec![format!("round #{} in {}", w.round, spent_words(w.seconds))];
    if bank::plain(&w.guess) != bank::plain(&w.word) {
        notes.push(format!("they typed {}", w.guess));
    }
    if w.hinted {
        notes.push(format!("a hint was out, so {} instead of {}", w.worth, w.full_points));
    }
    if let Some(why) = why {
        notes.push(why.to_string());
    }
    if !w.today.is_empty() {
        notes.push(format!("today: {}", w.today));
    }
    text.push_str(&format!("\n-# {}", notes.join(" · ")));
    text
}

/// The line that goes up when a round ends without a winner.
pub fn ended_text(row: &store::Row, others: &[String]) -> String {
    let also: Vec<String> = others.iter().filter(|a| bank::plain(a) != bank::plain(&row.word)).take(3).cloned().collect();
    let also = if also.is_empty() { String::new() } else { format!(" ({} would also have counted)", also.join(", ")) };
    match (row.status, row.ended_by) {
        (Status::Skipped, Some(by)) => {
            format!("⏭️ <@{}> passed on round #{} — it was a **{}**{}. No points for that one; here's another.", by, row.id, row.word, also)
        }
        (Status::Skipped, None) => format!("⏭️ Round #{} skipped — it was a **{}**{}.", row.id, row.word, also),
        _ => format!("⏰ Nobody had round #{} — it was a **{}**{}. A new one is up.", row.id, row.word, also),
    }
}

/// What a hint says in the channel. It never names the thing, only its first
/// letter - the second drawing is on the card itself.
pub fn hint_text(row: &store::Row, by: u64, worth: i64) -> String {
    format!(
        "💡 <@{}> asked for the hint: round #{} gets a second drawing of the same thing, and it starts with **{}**. It is now worth **{}** — `!skip` moves on if it's still hopeless.",
        by,
        row.id,
        row.first_letter(),
        plural(worth, "point", "points")
    )
}

/// Whether the bot should say so again, or let it go: the same reminder over
/// and over would be exactly the noise this game keeps out of the channel.
pub fn nag_due(last_ms: i64, now_ms: i64) -> bool {
    now_ms - last_ms >= NAG_EVERY_MS
}

/// Why a `!skip` was turned down.
pub const SKIP_TOO_SOON: &str = "🚫 `!skip` opens up once somebody has used `!hint` on the round. Try the hint first.";

/// The day's winners as the cards show them: "Aarav 2 (+3) · Meera 1 (+2)".
/// The number in brackets is GUESS points, which is what the winner line beside
/// it counts too — house points are capped and would disagree with it.
pub fn today_line(solves: &[store::Solve], name: impl Fn(u64) -> String) -> String {
    let mut tally: Vec<(u64, i64, i64)> = Vec::new();
    for s in solves {
        match tally.iter_mut().find(|(u, _, _)| *u == s.user) {
            Some((_, wins, points)) => {
                *wins += 1;
                *points += s.worth;
            }
            None => tally.push((s.user, 1, s.worth)),
        }
    }
    tally.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    tally.iter().map(|(u, wins, points)| format!("{} {} (+{})", name(*u), wins, points)).collect::<Vec<_>>().join(" · ")
}

/// Where someone stands on a ranked board, counting from one. `None` when they
/// aren't on it at all.
pub fn place_of(rows: &[store::Tally], me: u64) -> Option<usize> {
    rows.iter().position(|t| t.user == me).map(|i| i + 1)
}

/// "3rd", the way a line reads it.
pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

/// One person's own line on a board: where they stand out of how many, what
/// they have and over how many rounds. Nothing at all when they haven't played
/// that stretch.
pub fn standing(rows: &[store::Tally], me: u64) -> Option<String> {
    let place = place_of(rows, me)?;
    let mine = rows.get(place - 1)?;
    Some(format!(
        "{} of {} · **{}** · {}",
        ordinal(place),
        rows.len(),
        plural(mine.points, "guess point", "guess points"),
        plural(mine.solves, "round", "rounds")
    ))
}

/// What `/guess` shows whoever ran it, the picture with it: the round that is
/// up, and their own guess points today and this month. Guess points are
/// uncapped, so this is the one tally that never quietly stops moving.
pub fn mine_text(live: Option<&store::Row>, today: &[store::Tally], month: &[store::Tally], me: u64, channel: Option<u64>) -> String {
    let mut lines = vec!["🎨 **Guess the Word**".to_string()];
    match live {
        Some(row) => {
            let worth = worth_after_hint(row.points, row.hinted());
            lines.push(format!("**Round #{}** · worth **{}** · the drawing is below.", row.id, plural(worth, "point", "points")));
            match row.hint_by {
                Some(by) => lines.push(format!("💡 <@{}> had the hint: it starts with **{}**, and there are two drawings.", by, row.first_letter())),
                None => lines.push("-# `!hint` in the channel adds a second drawing and gives away the first letter, once a round.".to_string()),
            }
        }
        None => lines.push("No round is up right now — the next one is on its way.".to_string()),
    }
    lines.push(match standing(today, me) {
        Some(mine) => format!("-# **You today:** {}", mine),
        None => "-# You haven't won one today. Type your guess straight into the channel.".to_string(),
    });
    if let Some(mine) = standing(month, me) {
        lines.push(format!("-# **This month:** {}", mine));
    }
    if let Some(c) = channel {
        lines.push(format!("-# The card lives in <#{}> · `/guesstop` for the board · `/guesshelp` explains the rest.", c));
    }
    lines.push(format!("-# {}", ATTRIBUTION));
    lines.join("\n")
}

/// How many names `/guesstop` lists.
const TOP_LIST: usize = 10;

/// What `/guesstop` says. `rows` is the whole board, already ranked; only the
/// first [`TOP_LIST`] are listed, and whoever asked gets their own line under
/// them when they didn't make it.
pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("🎨 **Guess points** · {}", period);
    if rows.is_empty() {
        text.push_str("\nNobody has named one yet. The card is waiting in the channel.");
        return text;
    }
    for (i, t) in rows.iter().take(TOP_LIST).enumerate() {
        let rank = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            n => format!("`{:>2}.`", n + 1),
        };
        let you = if t.user == me { " ← you" } else { "" };
        text.push_str(&format!("\n{} <@{}> **{}** · {}{}", rank, t.user, t.points, plural(t.solves, "round", "rounds"), you));
    }
    if place_of(rows, me).is_some_and(|p| p > TOP_LIST) {
        if let Some(mine) = standing(rows, me) {
            text.push_str(&format!("\n-# **You:** {}", mine));
        }
    }
    text.push_str("\n-# Guess points count every solve at its full value — the daily house-points limit never takes one away.");
    text
}

// --- reading what was typed --------------------------------------------------------------

/// What a message in the game's channel turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    Hint,
    Skip,
    /// A guess the bank accepts for the round's word.
    Guess(String),
    /// Anything else at all, which the channel never hears about.
    Nothing,
}

/// Reads a message against the round that is up. Wrong guesses and chatter come
/// back the same way - [`Typed::Nothing`] - because the channel is a social one
/// and only a right guess is ever answered.
pub fn read_message(text: &str, word: usize, bank: &Bank) -> Typed {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(HINT_WORD) {
        return Typed::Hint;
    }
    if trimmed.eq_ignore_ascii_case(SKIP_WORD) {
        return Typed::Skip;
    }
    if bank.wins(word, trimmed) {
        return Typed::Guess(trimmed.to_string());
    }
    Typed::Nothing
}

/// Whether a round has been sitting untouched long enough for the bot to move
/// on by itself.
pub fn stale(posted_ts: i64, now: i64, idle_minutes: i64) -> bool {
    now - posted_ts >= idle_minutes.max(1) * 60
}

// --- shared state ----------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    /// (channel, message) of the round card. The ONE card the channel has.
    card: Option<(u64, u64)>,
    /// Cards a delete failed on, tried again until they are gone, so a failed
    /// delete can never leave two cards standing.
    orphans: Vec<(u64, u64)>,
    /// Messages from other people that have landed under the card.
    others_since_card: u64,
    /// When the card last moved down.
    last_bump_ms: i64,
    /// Someone deleted the card.
    card_gone: bool,
    /// The card is out of date (a hint added a second drawing).
    dirty: bool,
    /// A round just ended: the task says how and puts the next one up.
    ended: Option<i64>,
    /// A mod asked for a new round, with no hint needed first.
    admin_skip: Option<u64>,
    /// When the bot last answered a `!hint` that was already out, or a `!skip`
    /// that was too soon.
    last_nag_ms: i64,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// Every message in the game's channel, the bot's own included. Only other
/// people's messages count towards burying the card: the bot's own winner lines
/// must never push its own card down.
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
/// off: two cards in the channel would be two puzzles as far as anyone reading
/// can tell, so it is tried again until it is gone.
async fn delete(ctx: &Context, channel: u64, message: u64) {
    match call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        Ok(()) => {}
        Err(err) if is_gone(&err) => {}
        Err(err) => {
            tracing::warn!("guess: card {} not deleted ({}) — trying again", message, err);
            let mut s = SHARED.lock();
            if !s.orphans.contains(&(channel, message)) {
                s.orphans.push((channel, message));
            }
        }
    }
}

/// Another go at the cards a delete failed on.
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
        tracing::warn!("guess: line not posted in {}: {}", channel, err);
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
/// their points. The same rule anagrams and the frog drops use.
fn may_play(user: u64) -> bool {
    if super::house::opted_out(user) {
        return false;
    }
    super::house::house_of(user).is_some() || super::admin_ids().contains(&user)
}

fn display_name(ctx: &Context, guild: Option<serenity::all::GuildId>, user: u64) -> String {
    guild
        .and_then(|g| ctx.cache.guild(g).and_then(|g| g.members.get(&serenity::all::UserId::new(user)).map(|m| m.display_name().to_string())))
        .unwrap_or_else(|| format!("member {}", user))
}

/// Claims the right to say one of the small refusals out loud, if it is far
/// enough behind the last one.
fn take_nag() -> bool {
    let now = Utc::now().timestamp_millis();
    let mut s = SHARED.lock();
    if !nag_due(s.last_nag_ms, now) {
        return false;
    }
    s.last_nag_ms = now;
    true
}

fn live_row() -> Option<store::Row> {
    store::db().and_then(|db| store::live(&db.lock()))
}

fn day_solves_now() -> Vec<store::Solve> {
    match store::db() {
        Some(db) => store::day_solves(&db.lock(), &super::points::ist_day(Utc::now().timestamp())),
        None => Vec::new(),
    }
}

/// The first and last day of the India month a day falls in. The store's days
/// are `YYYY-MM-DD`, which sorts as a date, so a month is just a pair of ends —
/// a 31st that some months don't have is simply never matched.
pub fn month_ends(day: &str) -> (String, String) {
    let month = day.get(..7).unwrap_or(day);
    (format!("{}-01", month), format!("{}-31", month))
}

/// Today's and this month's guess points, both boards ranked.
fn boards_now() -> (Vec<store::Tally>, Vec<store::Tally>) {
    let day = super::points::ist_day(Utc::now().timestamp());
    let (from, to) = month_ends(&day);
    match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::day_tally(&conn, &day), store::tally_between(&conn, &from, &to))
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// The drawings a round is showing, out of the bank. Empty when the bank has
/// been changed under a live round, which the card reads as nothing to draw.
fn doodles_of(row: &store::Row) -> Vec<Doodle> {
    let Some(bank) = bank::bank() else { return Vec::new() };
    let Some(word) = bank.find(&row.word_key) else { return Vec::new() };
    row.shown().into_iter().filter_map(|d| bank.doodle(word, d as usize)).collect()
}

// --- the card -------------------------------------------------------------------------------------

fn live_of(row: &store::Row, now: i64) -> Live {
    Live {
        points: worth_after_hint(row.points, row.hinted()),
        hint: row.hint_by.map(|by| (by, row.first_letter())),
        open_secs: now - row.posted_ts,
    }
}

fn card_embed(row: &store::Row, drawings: usize, now: i64) -> CreateEmbed {
    CreateEmbed::new()
        .title(card_title(row.id))
        .description(card_text(&live_of(row, now)))
        .colour(COLOUR)
        .image(format!("attachment://{}", guess_draw::file_name(row.id, drawings)))
        .footer(CreateEmbedFooter::new(CARD_FOOTER))
}

/// The picture for a round as it stands, with its file name.
async fn card_picture(row: &store::Row) -> Option<(CreateAttachment, usize)> {
    let doodles = doodles_of(row);
    let drawings = doodles.len();
    let png = guess_draw::png(row.id, doodles).await?;
    Some((CreateAttachment::bytes(png.to_vec(), guess_draw::file_name(row.id, drawings)), drawings))
}

/// Builds the message for a live round, picture and all.
async fn card_message(row: &store::Row, now: i64) -> Option<CreateMessage> {
    let (picture, drawings) = card_picture(row).await?;
    Some(CreateMessage::new().embed(card_embed(row, drawings, now)).allowed_mentions(CreateAllowedMentions::new()).add_file(picture))
}

/// Posts a card and remembers it. `replace` takes the old card away;
/// `moved` says this is the card coming DOWN to the bottom rather than a
/// fresh round starting, which is the only thing the bump cooldown paces.
/// A new round used to start that cooldown too, and since a round is often
/// solved inside it, the card could never follow the chat down at all.
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
            tracing::warn!("guess: card not posted in {}: {}", channel, err);
            None
        }
    }
}

async fn drop_card(ctx: &Context) {
    let card = SHARED.lock().card.take();
    meta_set("card", "");
    if let Some((c, m)) = card {
        delete(ctx, c, m).await;
    }
}

/// Takes away the card of the round that has just ended - and ONLY that one.
///
/// A guess closes its round in the store straight away, so the next round's
/// card can already be up by the time the win is finished and announced. A blind
/// `drop_card` would then delete the NEW card and leave the channel with no
/// picture at all - which is exactly what anagrams did to itself on round 16. A
/// card that belongs to some other round is left where it is.
async fn drop_card_of(ctx: &Context, ended: Option<u64>) {
    let card = {
        let mut s = SHARED.lock();
        match card_to_drop(s.card, ended) {
            Some(card) => {
                s.card = None;
                Some(card)
            }
            None => None,
        }
    };
    if let Some((c, m)) = card {
        meta_set("card", "");
        delete(ctx, c, m).await;
    }
}

/// Which card an ending round may take away: its own, and only if that is still
/// the card the channel is showing. `None` for the round whose card was never
/// recorded - guessing would cost the next round its card.
pub fn card_to_drop(current: Option<(u64, u64)>, ended: Option<u64>) -> Option<(u64, u64)> {
    match (current, ended) {
        (Some((channel, card)), Some(ended)) if card == ended => Some((channel, card)),
        _ => None,
    }
}

/// Sets a fresh round and puts its card up.
async fn post_round(ctx: &Context, channel: u64) -> Option<store::Row> {
    let bank = bank::bank()?;
    let now = Utc::now().timestamp();
    let db = store::db()?;
    let row = {
        let conn = db.lock();
        let since = now - no_repeat_days() * 86_400;
        let mut rng = Rng::fresh();
        let word = bank.pick(&store::words_since(&conn, since), &mut rng)?;
        let entry = bank.word(word)?;
        let key = entry.key();
        // Not only a word nobody has had lately, but a drawing of it nobody has
        // seen: the same picture twice is a round somebody simply remembers.
        let doodle = bank.pick_doodle(word, &store::doodles_since(&conn, &key, since), &mut rng)? as i64;
        match store::add_round(&conn, &entry.word, &key, doodle, round_points(), channel, now) {
            Ok(row) => row,
            Err(err) => {
                tracing::warn!("guess: round not saved: {}", err);
                return None;
            }
        }
    };
    let message = card_message(&row, now).await?;
    let posted = place_card(ctx, channel, message, false, false).await?;
    if let Some(db) = store::db() {
        let _ = store::set_message(&db.lock(), row.id, posted);
    }
    tracing::info!("guess: round {} up in {} ({}, drawing {}, worth {})", row.id, channel, row.word, row.doodle, row.points);
    Some(store::Row { message: Some(posted), ..row })
}

/// Redraws the live card where it is, picture and all: a hint changes both the
/// words and the drawing, so the old attachment goes with them.
async fn edit_card(ctx: &Context, row: &store::Row, now: i64) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let Some((picture, drawings)) = card_picture(row).await else { return };
    let edit = EditMessage::new()
        .embed(card_embed(row, drawings, now))
        .attachments(EditAttachments::new().add(picture))
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        if is_gone(&err) {
            SHARED.lock().card_gone = true;
        }
        tracing::warn!("guess: card not edited: {}", err);
    }
}

/// Whether chat has buried the card: enough messages from other people have
/// landed under it AND the card hasn't already moved in the last little while.
/// Both halves matter — a busy evening would otherwise have the card leapfrog
/// the conversation every few seconds.
pub fn bump_due(others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> bool {
    others >= needed.max(1) && since_bump_ms >= every_secs.max(0) * 1_000
}

/// What to do with the card this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Nothing,
    /// Redraw it where it is (a hint changed what it shows).
    Edit,
    /// Post it again at the bottom and take the old one away.
    Bump,
}

/// The ONE decision behind the card, so nothing else in the game posts one.
///
/// A missing card, a card in the wrong channel and a card somebody deleted all
/// come to the same thing: put one up. Chat burying it moves it down. Anything
/// else is at most a redraw in place — which is what keeps a chatty channel from
/// ending up with two cards in it.
#[allow(clippy::too_many_arguments)]
pub fn card_plan(has_card: bool, right_channel: bool, gone: bool, dirty: bool, others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> CardAction {
    if !has_card || !right_channel || gone {
        return CardAction::Bump;
    }
    if bump_due(others, since_bump_ms, needed, every_secs) {
        return CardAction::Bump;
    }
    if dirty {
        return CardAction::Edit;
    }
    CardAction::Nothing
}

/// Keeps the channel's one card where people can see it: redrawn in place when
/// its picture changes, moved to the bottom once chat has buried it, and put
/// back if someone deleted it. The only place in the game that posts a card for
/// a round that is already up.
async fn tend_card(ctx: &Context, channel: u64, row: &store::Row, now: i64) -> (CardAction, Option<u64>) {
    let (card, plan) = {
        let s = SHARED.lock();
        let since_bump = Utc::now().timestamp_millis() - s.last_bump_ms;
        let right = s.card.map(|(c, _)| c) == Some(channel);
        // A hint comes once to a round, so the second drawing goes up at once
        // rather than waiting on any redraw pace.
        (s.card, card_plan(s.card.is_some(), right, s.card_gone, s.dirty, s.others_since_card, since_bump, bump_messages(), bump_seconds()))
    };
    match plan {
        CardAction::Nothing => (CardAction::Nothing, None),
        CardAction::Edit => {
            edit_card(ctx, row, now).await;
            SHARED.lock().dirty = false;
            (CardAction::Edit, None)
        }
        CardAction::Bump => {
            let Some(message) = card_message(row, now).await else { return (CardAction::Nothing, None) };
            let gone = SHARED.lock().card_gone;
            let replace = card.is_some() && !gone;
            let posted = place_card(ctx, channel, message, replace, true).await;
            if let (Some(id), Some(db)) = (posted, store::db()) {
                let _ = store::set_message(&db.lock(), row.id, id);
            }
            (CardAction::Bump, posted)
        }
    }
}

// --- the task --------------------------------------------------------------------------------------

/// Starts the task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting(), bank::bank().is_some()) {
        (true, Some(c), true) => tracing::info!("guess: playing in {}", c),
        (true, Some(_), false) => tracing::warn!("guess: on, but there is no doodle bank — the game stays off"),
        (true, None, _) => tracing::info!("guess: on, but VIZIER_GUESS_CHANNEL isn't set"),
        (false, _, _) => tracing::info!("guess: VIZIER_GUESS is off"),
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
    // What was said while the bot was away doesn't count towards burying the
    // card: the channel is read from where it stands now.
    if let Some(c) = channel {
        let read = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await.is_ok();
        let mut s = SHARED.lock();
        s.others_since_card = 0;
        s.last_bump_ms = Utc::now().timestamp_millis();
        if !read {
            tracing::debug!("guess: couldn't read {} on waking", c);
        }
    }
    match live {
        Some(row) if channel == Some(row.channel) => {
            let still_there = match row.message {
                Some(id) => exists(ctx, row.channel, id).await,
                None => false,
            };
            if still_there {
                SHARED.lock().card = row.message.map(|m| (row.channel, m));
                tracing::info!("guess: round {} picked up where it was left", row.id);
                return Some(row);
            }
            if let Some((c, m)) = old_card.filter(|(_, m)| Some(*m) != row.message) {
                delete(ctx, c, m).await;
            }
            SHARED.lock().card = None;
            Some(row)
        }
        other => {
            if let Some((c, m)) = old_card {
                delete(ctx, c, m).await;
            }
            SHARED.lock().card = None;
            other
        }
    }
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

        // A mod pressed on: the round is closed here, and told about below like
        // any other ending.
        let mut ending = ended;
        if let (Some(by), Some(row)) = (admin_skip, live.as_ref()) {
            let done = store::db().map(|db| store::skip(&db.lock(), row.id, by, now).unwrap_or(false)).unwrap_or(false);
            if done {
                tracing::info!("guess: round {} skipped by mod {}", row.id, by);
                ending = Some(row.id);
            }
        }
        // Nobody guessed and nobody skipped: the bot moves on by itself, so the
        // channel is never stuck on one drawing overnight. A round whose word
        // the bank no longer has goes the same way - it can be neither drawn nor
        // judged, so it is closed rather than left up as a blank sheet.
        if ending.is_none() {
            let unknown = |r: &store::Row| bank::bank().is_some_and(|b| b.find(&r.word_key).is_none());
            if let Some(row) = live.as_ref().filter(|r| stale(r.posted_ts, now, idle_minutes()) || unknown(r)) {
                let done = store::db().map(|db| store::expire(&db.lock(), row.id, now).unwrap_or(false)).unwrap_or(false);
                if done {
                    match unknown(row) {
                        true => tracing::warn!("guess: round {} closed — the bank no longer has {}", row.id, row.word),
                        false => tracing::info!("guess: round {} went stale after {} min", row.id, idle_minutes()),
                    }
                    ending = Some(row.id);
                }
            }
        }

        if let Some(id) = ending {
            let row = store::db().and_then(|db| store::get(&db.lock(), id));
            let ended_card = row.as_ref().and_then(|r| r.message);
            if let Some(row) = row.filter(|r| r.status != Status::Open) {
                announce_end(&ctx, channel, &row).await;
            }
            drop_card_of(&ctx, ended_card).await;
            live = None;
        }

        // The store is the truth: a round claimed by a guess is no longer live.
        if let Some(row) = &live {
            let fresh = store::db().and_then(|db| store::get(&db.lock(), row.id));
            match fresh {
                Some(f) if f.status == Status::Open && f.channel == channel => live = Some(f),
                _ => live = None,
            }
        }
        if live.is_none() {
            let open = store::db().and_then(|db| store::live(&db.lock()));
            let existing = match open {
                Some(row) if row.channel == channel => Some(row),
                Some(stale) => {
                    if let Some(db) = store::db() {
                        let _ = store::expire(&db.lock(), stale.id, now);
                    }
                    tracing::info!("guess: round {} closed — the game moved to {}", stale.id, channel);
                    None
                }
                None => None,
            };
            live = match existing {
                Some(row) => {
                    // Taking a round over, its card is only ours to keep if it
                    // is still in the channel: a delete that crossed with the
                    // round starting would otherwise leave the game believing
                    // in a card nobody can see, and no picture would show up
                    // until the next bump.
                    let there = match row.message {
                        Some(id) => exists(&ctx, channel, id).await,
                        None => false,
                    };
                    let mut s = SHARED.lock();
                    s.card = row.message.filter(|_| there).map(|m| (channel, m));
                    s.card_gone = !there;
                    // Taking a card over: nothing has buried THIS one yet.
                    s.others_since_card = 0;
                    s.last_bump_ms = Utc::now().timestamp_millis();
                    drop(s);
                    Some(row)
                }
                None => post_round(&ctx, channel).await,
            };
        }
        let Some(row) = live.clone() else { continue };

        // One place decides what happens to the card, so a bump and a fresh
        // round can never both post one.
        let (_, posted) = tend_card(&ctx, channel, &row, now).await;
        if let Some(id) = posted {
            live = Some(store::Row { message: Some(id), ..row.clone() });
        }
    }
}

/// Says how a round ended: won, skipped or left to go stale.
async fn announce_end(ctx: &Context, channel: u64, row: &store::Row) {
    if row.status == Status::Solved {
        let Some(winner) = row.winner else { return };
        let guild = ctx.cache.guilds().first().copied();
        let solves = day_solves_now();
        let mine = solves.iter().find(|s| s.round == row.id && s.user == winner);
        let paid = mine.map(|s| s.points).unwrap_or(0);
        let worth = mine.map(|s| s.worth).unwrap_or_else(|| worth_after_hint(row.points, row.hinted()));
        let won = Won {
            round: row.id,
            winner,
            badge: badge_of(winner),
            guess: row.winning_guess.clone().unwrap_or_else(|| row.word.clone()),
            word: row.word.clone(),
            points: paid,
            worth,
            full_points: row.points,
            housed: super::house::house_of(winner).is_some(),
            tally: solves.iter().filter(|s| s.user == winner).map(|s| s.worth).sum(),
            hinted: row.hinted(),
            seconds: row.seconds.unwrap_or(0),
            today: today_line(&solves, |u| display_name(ctx, guild, u)),
        };
        say(ctx, channel, won_text(&won)).await;
        return;
    }
    let others = bank::bank()
        .and_then(|b| b.find(&row.word_key).and_then(|w| b.word(w)))
        .map(|w| w.answers.clone())
        .unwrap_or_default();
    say(ctx, channel, ended_text(row, &others)).await;
}

// --- what people type ------------------------------------------------------------------------

/// Every human message in the game's channel. A right guess wins the round, a
/// `!hint` or a `!skip` steers it, and everything else is left alone.
pub async fn on_message(ctx: &Context, msg: &Message) {
    let Some(channel) = live_channel() else { return };
    if msg.channel_id.get() != channel || msg.author.bot {
        return;
    }
    let Some(bank) = bank::bank() else { return };
    let Some(row) = live_row().filter(|r| r.status == Status::Open) else { return };
    let Some(word) = bank.find(&row.word_key) else { return };
    match read_message(&msg.content, word, bank) {
        Typed::Hint => hint_asked(ctx, msg, &row, word, channel).await,
        Typed::Skip => skip_asked(ctx, msg, &row, channel).await,
        Typed::Guess(guess) => guessed(ctx, msg, &row, &guess).await,
        Typed::Nothing => {}
    }
}

/// A guess that names the drawing. Only the first one counts; the rest are told
/// nothing, because they can see the winner's line for themselves.
async fn guessed(ctx: &Context, msg: &Message, row: &store::Row, guess: &str) {
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let won = {
        let conn = db.lock();
        store::claim(&conn, row.id, user, guess, now).unwrap_or(false)
    };
    if !won {
        return;
    }
    if let Err(err) = call(msg.react(&ctx.http, ReactionType::Unicode(TICK_MARK.to_string()))).await {
        tracing::debug!("guess: tick not added to {}: {}", msg.id, err);
    }
    let seconds = now - row.posted_ts;
    let worth = worth_after_hint(row.points, row.hinted());
    let granted = if may_play(user) {
        let reason = format!("Guess the Word: round {} ({})", row.id, row.word);
        match super::house::award_person(user, Source::Guess, worth, &reason, None, Some(format!("{}{}", LEDGER_KEY, row.id)), None) {
            Some((_, Outcome::Granted(n))) => n,
            _ => 0,
        }
    } else {
        0
    };
    // What the round was worth goes down for EVERY winner, cap or no cap, house
    // or no house: the house points are the ledger's business, the guess points
    // are the game's.
    {
        let conn = db.lock();
        let _ = store::add_solve(&conn, &super::points::ist_day(now), user, row.id, granted, worth, guess, seconds, now);
    }
    tracing::info!("guess: round {} won by {} with {:?} in {}s for {} house points ({} guess points)", row.id, user, guess, seconds, granted, worth);
    SHARED.lock().ended = Some(row.id);
}

/// `!hint`: once a round. A second drawing of the same thing goes onto the card,
/// and the word's first letter is said out loud. Who asked is written down — it
/// costs the round a point, so it has to be attributable.
async fn hint_asked(ctx: &Context, msg: &Message, row: &store::Row, word: usize, channel: u64) {
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let Some(bank) = bank::bank() else { return };
    let taken = {
        let conn = db.lock();
        // A drawing of this word nobody has been shown lately, and never the one
        // already on the card.
        let mut seen = store::doodles_since(&conn, &row.word_key, now - no_repeat_days() * 86_400);
        seen.insert(row.doodle);
        let second = bank.pick_doodle(word, &seen, &mut Rng::fresh()).map(|d| d as i64);
        match second.filter(|d| *d != row.doodle) {
            Some(second) => store::take_hint(&conn, row.id, user, second, now).unwrap_or(false),
            // A word with only the one drawing has no second one to show; the
            // hint still gives the letter away and still costs the round.
            None => store::take_hint(&conn, row.id, user, row.doodle, now).unwrap_or(false),
        }
    };
    if !taken {
        if !take_nag() {
            return;
        }
        let by = row.hint_by.map(|by| format!(" <@{}> asked for it", by)).unwrap_or_default();
        return say(
            ctx,
            channel,
            format!("💡 The hint is already out for round #{}:{} it starts with **{}**. `!skip` moves on.", row.id, by, row.first_letter()),
        )
        .await;
    }
    SHARED.lock().dirty = true;
    let worth = worth_after_hint(row.points, true);
    tracing::info!("guess: hint on round {} to {} (worth {} now)", row.id, user, worth);
    say(ctx, channel, hint_text(row, user, worth)).await;
}

/// `!skip`: only after a hint, and it pays nothing. The next drawing goes up at
/// once.
async fn skip_asked(ctx: &Context, msg: &Message, row: &store::Row, channel: u64) {
    if !row.hinted() {
        if take_nag() {
            say(ctx, channel, SKIP_TOO_SOON.to_string()).await;
        }
        return;
    }
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let skipped = {
        let conn = db.lock();
        store::skip(&conn, row.id, user, now).unwrap_or(false)
    };
    if !skipped {
        return;
    }
    tracing::info!("guess: round {} skipped by {}", row.id, user);
    SHARED.lock().ended = Some(row.id);
}

// --- commands -------------------------------------------------------------------------------

pub fn mine_builder() -> CreateCommand {
    CreateCommand::new("guess").description("the doodle that's up now, what it's worth and how you've done today")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("guesstop")
        .description("the guess points board, today or this month")
        .add_option(
            serenity::all::CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "which days to count")
                .add_string_choice("Today", "today")
                .add_string_choice("This month", "month"),
        )
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("guesshelp").description("how Guess the Word works, from the settings as they are now")
}

pub fn skip_builder() -> CreateCommand {
    CreateCommand::new("guessskip").description("admin only: drop the doodle that's up and draw a fresh one, no points")
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("guessstop").description("admin only: switch Guess the Word off and take the card down")
}

async fn reply(ctx: &Context, command: &CommandInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message.ephemeral(true))).await {
        tracing::warn!("guess: reply to {} not sent: {}", command.user.id, err);
    }
}

const OFF: &str = "Guess the Word is switched off right now.";

/// `/guess` — everyone, shown only to them, the picture with it.
pub async fn mine_command(ctx: &Context, command: &CommandInteraction) {
    let live = live_row();
    let (today, month) = boards_now();
    let text = mine_text(live.as_ref(), &today, &month, command.user.id.get(), live_channel());
    let mut message = CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Some((picture, _)) = match live.as_ref().filter(|r| r.status == Status::Open) {
        Some(row) => card_picture(row).await,
        None => None,
    } {
        message = message.add_file(picture);
    }
    reply(ctx, command, message).await;
}

/// `/guesstop [period]` — everyone, shown only to them, like `/housetop`.
pub async fn top_command(ctx: &Context, command: &CommandInteraction) {
    let month = command.data.options.iter().any(|o| {
        o.name == "period" && matches!(&o.value, serenity::all::CommandDataOptionValue::String(v) if v == "month")
    });
    let (today_rows, month_rows) = boards_now();
    let (rows, period) = if month {
        (month_rows, month_label(&super::points::ist_day(Utc::now().timestamp())))
    } else {
        (today_rows, "today".to_string())
    };
    let text = top_text(&period, &rows, command.user.id.get());
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

/// "September so far", the heading a month's board carries.
pub fn month_label(day: &str) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let month = day.get(5..7).and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m));
    match month {
        Some(m) => format!("{} so far", MONTHS[m - 1]),
        None => "this month".to_string(),
    }
}

/// The rules as a card, the same words for the command and anything else that
/// wants them.
fn help_embed() -> CreateEmbed {
    CreateEmbed::new().title(rules_text::GUESS_RULES_TITLE).description(rules_text::guess_help_text(&guess_rules())).colour(COLOUR)
}

/// `/guesshelp` — everyone.
pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    reply(ctx, command, CreateInteractionResponseMessage::new().embed(help_embed())).await;
}

/// `/guessskip` — admins only.
pub async fn skip_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can skip a round.")).await;
    }
    if live_channel().is_none() {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content(OFF)).await;
    }
    let live = live_row().map(|r| r.id);
    SHARED.lock().admin_skip = Some(user);
    tracing::info!("guess: /guessskip by {} (round {:?})", user, live);
    let text = match live {
        Some(id) => format!("⏭️ Round #{} dropped. A fresh doodle is on its way — nobody scores for that one.", id),
        None => "⏭️ A fresh doodle is on its way.".to_string(),
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

/// `/guessstop` — admins only. Switches the game off, which takes the card down
/// and stops new rounds; the round that was up is left where it is.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can stop the game.")).await;
    }
    let text = match control::set("VIZIER_GUESS", Some("off"), user) {
        Ok(()) => {
            tracing::info!("guess: /guessstop by {}", user);
            "🛑 Guess the Word is off. The card comes down in a moment; switch **Game on** back on in the panel to play again."
        }
        Err(err) => {
            tracing::warn!("guess: /guessstop by {} failed: {}", user, err);
            "The setting wouldn't save — switch **Game on** off in the panel instead."
        }
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

/// The last few rounds, for a summary or a panel page later on.
pub fn recent_rounds(limit: usize) -> Vec<store::Row> {
    store::db().map(|db| store::recent(&db.lock(), limit.min(RECENT))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::guess_bank::tests::fixture;
    use crate::channels::discord::guess_store::tests::{memory, put};

    /// Anagrams' round 16: round 17 went up, then round 16's win finished and
    /// took the new card away with it, leaving the channel with no picture at
    /// all. This game was born knowing better.
    #[test]
    fn an_ending_round_only_ever_takes_away_its_own_card() {
        let showing = Some((99, 1_000));
        // Its own card, still up: that one goes.
        assert_eq!(card_to_drop(showing, Some(1_000)), Some((99, 1_000)));
        // The next round's card is already up - leave it alone.
        assert_eq!(card_to_drop(showing, Some(900)), None);
        // A round whose card was never written down guesses at nothing.
        assert_eq!(card_to_drop(showing, None), None);
        assert_eq!(card_to_drop(None, Some(1_000)), None);
    }

    /// With no card of our own in the channel there is nothing to wait for: the
    /// next tick puts one up, which is what rescues a card that was deleted
    /// while its round was being handed over. The game checks with Discord that
    /// an adopted card is really there, and this is what it does when it isn't.
    #[test]
    fn a_missing_card_is_posted_again_at_once() {
        assert_eq!(card_plan(false, true, false, false, 0, 0, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, true, true, false, 0, 0, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, true, false, false, 0, 0, 5, 120), CardAction::Nothing);
    }

    #[test]
    fn a_hint_takes_a_point_off_but_a_round_is_never_worth_nothing() {
        assert_eq!(worth_after_hint(2, false), 2);
        assert_eq!(worth_after_hint(2, true), 1);
        assert_eq!(worth_after_hint(1, true), 1, "a one point round still pays one");
        assert_eq!(worth_after_hint(0, true), 1, "and never less than that");
        assert_eq!(worth_after_hint(0, false), 0);
        assert_eq!(worth_after_hint(5, true), 4, "another server's number holds too");
    }

    #[test]
    fn the_card_asks_what_it_is_and_says_what_the_round_is_worth() {
        let live = Live { points: 2, hint: None, open_secs: 380 };
        let text = card_text(&live);
        assert!(text.contains("# What is this?"), "{}", text);
        assert!(text.contains("worth **2 points** · first to name it wins"), "{}", text);
        assert!(text.contains("up 6 min") && text.contains("just type what you think it is"), "{}", text);
        assert_eq!(card_title(12), "🎨 Guess the Word · Round #12");
        // Once the hint is out the card says so, says who asked, and shows two
        // drawings.
        let hinted = Live { points: 1, hint: Some((42, 'G')), open_secs: 20 };
        let text = card_text(&hinted);
        assert!(text.contains("starts with **G**") && text.contains("<@42>"), "{}", text);
        assert!(text.contains("another drawing of the same thing"), "{}", text);
        assert!(text.contains("worth **1 point**") && text.contains("up just now"), "{}", text);
        // The card tells people how to play in a channel where they can type.
        assert!(CARD_FOOTER.contains("Type your guess") && CARD_FOOTER.contains(HINT_WORD) && CARD_FOOTER.contains(SKIP_WORD));
    }

    fn won() -> Won {
        Won {
            round: 12,
            winner: 42,
            badge: "🦅 Ravenclaw".into(),
            guess: "gitar".into(),
            word: "guitar".into(),
            points: 2,
            worth: 2,
            full_points: 2,
            housed: true,
            tally: 2,
            hinted: false,
            seconds: 47,
            today: "Aarav 2 (+4)".into(),
        }
    }

    #[test]
    fn the_winner_line_names_the_thing_and_the_points() {
        let won = Won { tally: 14, ..won() };
        let text = won_text(&won);
        assert_eq!(text.lines().next().unwrap(), "✅ <@42> 🦅 Ravenclaw had it: **GUITAR** · **+2** · 14 guess points today");
        assert!(text.contains("round #12 in 47 s") && text.contains("they typed gitar"), "{}", text);
        assert!(text.contains("today: Aarav 2 (+4)"), "{}", text);
        // The word typed properly needs no aside; a hinted round says what it cost.
        let plain = Won { guess: "Guitar!".into(), hinted: true, points: 1, worth: 1, full_points: 2, today: String::new(), ..won };
        let text = won_text(&plain);
        assert!(!text.contains("they typed"), "the same word, punctuated: {}", text);
        assert!(text.contains("a hint was out, so 1 instead of 2"), "{}", text);
    }

    #[test]
    fn a_solve_the_ledger_cant_pay_for_still_reads_as_a_win() {
        // The day's house points are full: nothing is paid, the round still
        // scored, and the line says which is which rather than "no points".
        let capped = Won { points: 0, worth: 2, full_points: 2, housed: true, tally: 12, ..won() };
        let text = won_text(&capped);
        assert_eq!(text.lines().next().unwrap(), "✅ <@42> 🦅 Ravenclaw had it: **GUITAR** · **+2 guess points** · 12 today");
        assert!(text.contains("that's your house points for today, but the guess points still count"), "{}", text);
        assert!(!text.contains("no points"), "a win is never reported as nothing: {}", text);
        // A mod or a Muggle, who has no house to pay into at all.
        let mod_win = Won { badge: String::new(), housed: false, ..capped.clone() };
        let text = won_text(&mod_win);
        assert!(text.starts_with("✅ <@42> had it: **GUITAR** · **+2 guess points** · 12 today"), "{}", text);
        assert!(text.contains("no house to pay, but the guess points still count"), "{}", text);
        // One point reads as one point.
        let one = Won { worth: 1, tally: 1, ..capped };
        assert!(won_text(&one).contains("**+1 guess point** · 1 today"), "{}", won_text(&one));
    }

    #[test]
    fn a_round_that_nobody_gets_says_what_it_was() {
        let conn = memory();
        let row = put(&conn, "police car", 3, 2, 100);
        let answers: Vec<String> = ["police car", "cop car", "squad car"].iter().map(|s| s.to_string()).collect();
        let stale = ended_text(&row, &answers);
        assert!(stale.starts_with("⏰ Nobody had round #"), "{}", stale);
        assert!(stale.contains("**police car**") && stale.contains("cop car, squad car would also have counted"), "{}", stale);
        // A player skipping says who did it.
        store::skip(&conn, row.id, 7, 200).unwrap();
        let skipped = ended_text(&store::get(&conn, row.id).unwrap(), &answers);
        assert!(skipped.contains("<@7> passed on round") && skipped.contains("No points"), "{}", skipped);
        // A word with only its own name reads cleanly.
        let lone = put(&conn, "guitar", 1, 2, 100);
        let text = ended_text(&lone, &["guitar".to_string()]);
        assert!(text.contains("**guitar**.") && !text.contains("would also"), "{}", text);
    }

    #[test]
    fn only_a_guess_that_names_the_drawing_is_even_looked_at() {
        let bank = fixture();
        let guitar = bank.find("guitar").expect("the guitar");
        assert_eq!(read_message("guitar", guitar, &bank), Typed::Guess("guitar".into()));
        assert_eq!(read_message("  Gitar! ", guitar, &bank), Typed::Guess("Gitar!".into()), "case, punctuation and a slip are forgiven");
        // Another word's answer, a part of a word, chatter and nothing at all.
        for quiet in ["police car", "car", "lol", "", "🎉", "is it a guitar or a banjo"] {
            assert_eq!(read_message(quiet, guitar, &bank), Typed::Nothing, "{:?} must be ignored in silence", quiet);
        }
        // The same guess against the round it really belongs to.
        let police = bank.find("policecar").expect("the police car");
        assert_eq!(read_message("cop car", police, &bank), Typed::Guess("cop car".into()));
        // And the two words that steer a round, however they are typed.
        assert_eq!(read_message("!hint", guitar, &bank), Typed::Hint);
        assert_eq!(read_message(" !HINT ", guitar, &bank), Typed::Hint);
        assert_eq!(read_message("!skip", guitar, &bank), Typed::Skip);
        assert_eq!(read_message("!skipping ahead", guitar, &bank), Typed::Nothing);
    }

    #[test]
    fn only_the_first_right_guess_wins_and_the_second_is_told_nothing() {
        let conn = memory();
        let bank = fixture();
        let row = put(&conn, "guitar", 0, 2, 1_000);
        let word = bank.find(&row.word_key).expect("the guitar");
        // Two people type at once; both messages read as guesses.
        assert_eq!(read_message("gitar", word, &bank), Typed::Guess("gitar".into()));
        assert_eq!(read_message("guitar", word, &bank), Typed::Guess("guitar".into()));
        assert!(store::claim(&conn, row.id, 11, "gitar", 1_030).unwrap());
        assert!(!store::claim(&conn, row.id, 22, "guitar", 1_030).unwrap(), "the second one wins nothing");
        let after = store::get(&conn, row.id).unwrap();
        assert_eq!((after.winner, after.winning_guess.as_deref()), (Some(11), Some("gitar")));
        assert_eq!(after.status, Status::Solved);
        // And the day's list has one winner in it.
        store::add_solve(&conn, "2026-09-17", 11, row.id, 2, 2, "gitar", 30, 1_030).unwrap();
        assert_eq!(store::day_solves(&conn, "2026-09-17").len(), 1);
    }

    #[test]
    fn a_hint_comes_once_with_a_second_drawing_and_a_skip_only_after_it() {
        let conn = memory();
        let row = put(&conn, "ice cream", 2, 2, 500);
        // No hint yet: a skip is turned down, and the round is untouched.
        assert!(!row.hinted());
        assert!(SKIP_TOO_SOON.contains("!hint"));
        assert_eq!(store::get(&conn, row.id).unwrap().status, Status::Open);
        // The hint: one only, it says who had it, and it puts a second drawing up.
        assert!(store::take_hint(&conn, row.id, 42, 9, 540).unwrap());
        assert!(!store::take_hint(&conn, row.id, 43, 11, 560).unwrap(), "one hint to a round");
        let hinted = store::get(&conn, row.id).unwrap();
        assert_eq!((hinted.hint_by, hinted.hint_doodle), (Some(42), Some(9)));
        assert_eq!(hinted.shown(), vec![2, 9]);
        let worth = worth_after_hint(hinted.points, true);
        assert_eq!(worth, 1, "a round pays two, one less after the hint");
        let text = hint_text(&hinted, 42, worth);
        assert!(text.contains("starts with **I**") && text.contains("worth **1 point**"), "{}", text);
        assert!(text.contains("second drawing"), "{}", text);
        assert!(!text.contains("ice cream"), "the hint never names the thing: {}", text);
        // Now a skip goes through, once, and pays nobody.
        assert!(hinted.hinted());
        assert!(store::skip(&conn, row.id, 43, 600).unwrap());
        assert!(!store::skip(&conn, row.id, 44, 610).unwrap());
        let after = store::get(&conn, row.id).unwrap();
        assert_eq!((after.status, after.ended_by), (Status::Skipped, Some(43)));
        assert!(store::day_solves(&conn, "2026-09-17").is_empty(), "a skip pays nothing");
    }

    #[test]
    fn a_round_nobody_touches_is_replaced_by_the_bot_itself() {
        let conn = memory();
        let row = put(&conn, "guitar", 0, 2, 1_000);
        let idle = 15;
        assert!(!stale(row.posted_ts, 1_000 + 14 * 60, idle));
        assert!(stale(row.posted_ts, 1_000 + 15 * 60, idle), "a quarter of an hour and it goes");
        assert!(stale(row.posted_ts, 1_000 + 3 * 3600, idle));
        // A shorter window, and a nonsense one that is treated as a minute.
        assert!(stale(row.posted_ts, 1_000 + 5 * 60, 5));
        assert!(stale(row.posted_ts, 1_000 + 60, 0));
        // The bot closes it and the next round is free to go up.
        assert!(store::expire(&conn, row.id, 1_000 + 15 * 60).unwrap());
        assert_eq!(store::get(&conn, row.id).unwrap().status, Status::Expired);
        assert!(store::live(&conn).is_none());
        // A round somebody won is never called stale afterwards.
        let other = put(&conn, "ice cream", 1, 2, 2_000);
        store::claim(&conn, other.id, 5, "icecream", 2_010).unwrap();
        assert!(!store::expire(&conn, other.id, 9_999).unwrap());
    }

    #[test]
    fn neither_the_word_nor_the_drawing_comes_round_again_inside_the_window() {
        let conn = memory();
        let bank = fixture();
        let day = 86_400;
        let now = 100 * day;
        // Every word but one has been drawn this fortnight.
        for (i, word) in ["guitar", "police car", "mouse", "house"].iter().enumerate() {
            put(&conn, word, i as i64, 2, now - (i as i64 + 1) * day);
        }
        let used = store::words_since(&conn, now - 14 * day);
        assert_eq!(used.len(), 4);
        let mut rng = Rng::seeded(11);
        for _ in 0..10 {
            let word = bank.pick(&used, &mut rng).expect("a word");
            assert_eq!(bank.word(word).map(|w| w.word.as_str()), Some("ice cream"), "the only word not drawn lately");
        }
        // And within a word, a drawing that has already been up is held back -
        // the one the card showed and the one a hint added alike.
        let again = put(&conn, "guitar", 0, 2, now);
        store::take_hint(&conn, again.id, 1, 1, now + 10).unwrap();
        let seen = store::doodles_since(&conn, "guitar", now - 14 * day);
        assert_eq!(seen, std::collections::HashSet::from([0, 1]));
        let guitar = bank.find("guitar").expect("the guitar");
        for _ in 0..10 {
            assert_eq!(bank.pick_doodle(guitar, &seen, &mut rng), Some(2), "the only drawing nobody has seen");
        }
    }

    fn tally(user: u64, points: i64, solves: i64, reached: i64) -> store::Tally {
        store::Tally { user, points, solves, reached }
    }

    #[test]
    fn the_days_winners_read_as_a_line() {
        // The day's line counts GUESS points, so the capped solve (paid 0, worth
        // 1) is in it exactly like the paid ones.
        let solves = vec![
            store::Solve { user: 1, round: 1, points: 2, worth: 2, guess: "gitar".into(), seconds: 30, ts: 10 },
            store::Solve { user: 2, round: 2, points: 2, worth: 2, guess: "cop car".into(), seconds: 90, ts: 20 },
            store::Solve { user: 1, round: 3, points: 0, worth: 1, guess: "icecream".into(), seconds: 40, ts: 30 },
        ];
        assert_eq!(today_line(&solves, |u| format!("P{}", u)), "P1 2 (+3) · P2 1 (+2)");
        assert_eq!(today_line(&[], |u| format!("P{}", u)), "");
        // And what /guess shows one of them.
        let conn = memory();
        let row = put(&conn, "guitar", 0, 2, 1_000);
        let today = vec![tally(2, 5, 2, 20), tally(1, 3, 2, 30)];
        let month = vec![tally(1, 40, 18, 90), tally(2, 9, 4, 20)];
        let text = mine_text(Some(&row), &today, &month, 1, Some(55));
        assert!(text.contains("**Round #1** · worth **2 points**"), "{}", text);
        assert!(text.contains("<#55>") && text.contains("**You today:** 2nd of 2 · **3 guess points** · 2 rounds"), "{}", text);
        assert!(text.contains("**This month:** 1st of 2 · **40 guess points** · 18 rounds"), "{}", text);
        assert!(!text.contains("guitar"), "the answer is never in it: {}", text);
        assert!(text.contains("`/guesstop`") && text.contains("`!hint`") && text.contains("Quick, Draw!"), "the doodles are credited: {}", text);
        // Somebody who hasn't played today but has this month, and one who never has.
        let some = mine_text(Some(&row), &today, &month, 2, None);
        assert!(some.contains("**You today:** 1st of 2 · **5 guess points**"), "{}", some);
        let none = mine_text(Some(&row), &today, &month, 99, None);
        assert!(none.contains("haven't won one today") && !none.contains("This month"), "{}", none);
        assert!(mine_text(None, &[], &[], 1, None).contains("No round is up right now"));
        // Once the hint is out, /guess carries it too.
        store::take_hint(&conn, row.id, 7, 2, 1_010).unwrap();
        let hinted = store::get(&conn, row.id).unwrap();
        assert!(mine_text(Some(&hinted), &[], &[], 1, None).contains("<@7> had the hint: it starts with **G**"));
    }

    #[test]
    fn the_guess_points_board_lists_ten_and_finds_the_asker_below_them() {
        let mut rows: Vec<store::Tally> = (1..=12).map(|i| tally(i, (20 - i) as i64, 3, 100 + i as i64)).collect();
        store::rank(&mut rows);
        let text = top_text("today", &rows, 12);
        assert!(text.starts_with("🎨 **Guess points** · today"), "{}", text);
        assert!(text.contains("\n🥇 <@1> **19** · 3 rounds"), "{}", text);
        assert!(text.contains("\n🥈 <@2> **18**") && text.contains("\n🥉 <@3> **17**"), "{}", text);
        assert!(text.contains("\n`10.` <@10> **10** · 3 rounds"), "{}", text);
        assert!(!text.contains("<@11>"), "only ten are listed: {}", text);
        // Outside the ten: their own line, and where they stand.
        assert!(text.contains("-# **You:** 12th of 12 · **8 guess points** · 3 rounds"), "{}", text);
        // Inside the ten: marked in place, with no line of their own.
        let inside = top_text("today", &rows, 3);
        assert!(inside.contains("🥉 <@3> **17** · 3 rounds ← you"), "{}", inside);
        assert!(!inside.contains("**You:**"), "{}", inside);
        // The board says what a guess point is, so nobody reads it as house points.
        assert!(text.contains("the daily house-points limit never takes one away"), "{}", text);
        assert!(top_text("September so far", &[], 1).contains("Nobody has named one yet"));
        // Ordering is the store's: points, then rounds, then who got there first.
        let mut close = vec![tally(1, 4, 1, 50), tally(2, 4, 2, 90), tally(3, 4, 2, 60)];
        store::rank(&mut close);
        assert_eq!(close.iter().map(|t| t.user).collect::<Vec<_>>(), vec![3, 2, 1]);
        assert_eq!((place_of(&close, 2), place_of(&close, 9)), (Some(2), None));
        assert_eq!([ordinal(1), ordinal(2), ordinal(3), ordinal(4), ordinal(11), ordinal(21)].join(" "), "1st 2nd 3rd 4th 11th 21st");
    }

    #[test]
    fn a_month_is_read_off_the_day_it_is_asked_on() {
        assert_eq!(month_ends("2026-09-17"), ("2026-09-01".to_string(), "2026-09-31".to_string()));
        assert_eq!(month_ends("2026-02-01"), ("2026-02-01".to_string(), "2026-02-31".to_string()));
        assert_eq!(month_label("2026-09-17"), "September so far");
        assert_eq!(month_label("2026-01-02"), "January so far");
        assert_eq!(month_label("nonsense"), "this month");
    }

    #[test]
    fn the_card_follows_the_chat_down_only_once_chat_has_really_buried_it() {
        let (needed, every) = (5, 120);
        let long_ago = 10 * 60 * 1_000;
        // A quiet channel, and a few messages: the card stays where it is.
        assert!(!bump_due(0, long_ago, needed, every));
        assert!(!bump_due(4, long_ago, needed, every), "four of the five it takes");
        // Enough messages, but the card only just moved: not again yet.
        assert!(!bump_due(9, 0, needed, every));
        assert!(!bump_due(9, every * 1_000 - 1, needed, every), "a second short of the window");
        // Both halves: down it goes.
        assert!(bump_due(5, every * 1_000, needed, every));
        assert!(bump_due(50, long_ago, needed, every));
        // A nonsense setting still asks for one message.
        assert!(!bump_due(0, long_ago, 0, every));
        assert!(bump_due(1, long_ago, 0, 0));
    }

    #[test]
    fn one_card_at_a_time_whatever_the_channel_is_doing() {
        let (needed, every) = (5, 120);
        let long_ago = 10 * 60 * 1_000;
        let plan = |has_card, right, gone, dirty, others, since| card_plan(has_card, right, gone, dirty, others, since, needed, every);
        // Nothing up yet, the game moved channel, or someone deleted it: post one.
        assert_eq!(plan(false, false, false, false, 0, long_ago), CardAction::Bump);
        assert_eq!(plan(true, false, false, false, 0, 0), CardAction::Bump, "a card in the old channel is not this channel's card");
        assert_eq!(plan(true, true, true, false, 0, 0), CardAction::Bump, "deleted, so back it goes");
        // A settled channel: nothing at all, which is what keeps the card single.
        assert_eq!(plan(true, true, false, false, 0, long_ago), CardAction::Nothing);
        assert_eq!(plan(true, true, false, false, 4, long_ago), CardAction::Nothing, "chatter under the threshold");
        assert_eq!(plan(true, true, false, false, 9, 1_000), CardAction::Nothing, "buried, but it only just moved");
        // A hint changed the picture: redrawn where it is, never posted again.
        assert_eq!(plan(true, true, false, true, 0, long_ago), CardAction::Edit);
        // Buried: one move, and the plan is one action, so a bump and a repost
        // can never both happen in a tick.
        assert_eq!(plan(true, true, false, true, 6, long_ago), CardAction::Bump, "a bump carries the new drawing with it");
        for others in [5, 20, 100] {
            assert_eq!(plan(true, true, false, false, others, long_ago), CardAction::Bump);
        }
    }

    #[test]
    fn the_same_small_refusal_is_not_repeated_over_and_over() {
        // Somebody hammering !hint or !skip is answered once, then left alone.
        assert!(nag_due(0, NAG_EVERY_MS));
        assert!(!nag_due(10_000, 10_000 + NAG_EVERY_MS - 1));
        assert!(nag_due(10_000, 10_000 + NAG_EVERY_MS));
    }

    #[test]
    fn with_no_doodle_bank_the_game_is_simply_off() {
        // Nothing sets the bank in a test run, so this is the missing-bank case:
        // the channel setting alone can never start the game.
        assert!(bank::bank().is_none());
        assert!(live_channel().is_none(), "no bank, no game — whatever the settings say");
        assert_eq!(guess_rules().words, None);
        assert!(live_row().is_none(), "and nothing to guess at");
        // The channel is the one the owner made for it until a mod moves it.
        assert_eq!(channel_setting(), Some(HOME_CHANNEL));
    }

    #[test]
    fn how_long_things_took_reads_naturally() {
        assert_eq!(spent_words(47), "47 s");
        assert_eq!(spent_words(461), "7 min 41 s");
        assert_eq!(spent_words(3_840), "1 h 04 min");
        assert_eq!(spent_words(-5), "0 s");
        assert_eq!(open_words(4), "just now");
        assert_eq!(open_words(380), "6 min");
        assert_eq!(open_words(200_000), "2 days");
    }
}
