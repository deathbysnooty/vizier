//! The chess puzzle: one position always waiting in the chess channel, solved
//! on a private web page.
//!
//! A card sits in the chess channel with the board drawn on it and a **🧩 Solve
//! it** button. The button hands whoever pressed it a private link of their own,
//! and on that page they play the winning line move by move: a wrong move is
//! refused with "that's not it" and they may try again, and the puzzle is solved
//! only when the whole line is played. Nobody types in that channel, here as
//! everywhere in it.
//!
//! THE FIRST person to crack each puzzle takes a house point
//! (`VIZIER_POINTS_PUZZLE`, up to `VIZIER_CAP_PUZZLE` a day — nought by default,
//! which means no house points are paid at all until the owner turns them on).
//! Everybody who solves it, first or not, scores PUZZLE POINTS: the game's own
//! score, which nothing caps and which everyone has, mods and Muggles included.
//! `/puzzletop` is the board.
//!
//! The pace: once somebody cracks it the puzzle stays up for
//! `VIZIER_PUZZLE_NEXT_MINUTES` so others can still try, and a puzzle nobody
//! touches is replaced after `VIZIER_PUZZLE_IDLE_MINUTES` with its answer shown.
//! The same puzzle doesn't come round again for `VIZIER_PUZZLE_NO_REPEAT_DAYS`.
//!
//! An engine solves any of these instantly, and the card and the help card both
//! say so: the point is to find it yourself. That is why the house point is
//! capped and why the game ships with it switched off.
//!
//! One task does all the posting ([`run`]), the way the other card games do: the
//! button and the web page only write to the database, so two presses can never
//! make two cards.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions,
    CreateAttachment, CreateButton, CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage, GetMessages, Message,
    MessageId,
};

use super::chess_board::{self, View};
use super::control;
use super::points::{Outcome, Source};
use super::puzzle_bank as bank;
use super::puzzle_rules as rules;
use super::puzzle_store::{self as store, Status};
use super::sudoku_gen::Rng;

const TICK: Duration = Duration::from_secs(5);
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// How often the card may be redrawn in place, however much changes.
const EDIT_EVERY_SECS: i64 = 15;
const COLOUR: u32 = 0x9B59B6;
const SOLVED_COLOUR: u32 = 0x3BA55C;
const BOARD_FILE: &str = "puzzle.png";

/// The button people press to get their own board.
pub const SOLVE_ID: &str = "puzzlesolve";
pub const HELP_ID: &str = "puzzlehelp";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_PUZZLE", false)
}

/// The puzzle lives in the chess channel: it is a chess puzzle, and the people
/// who want it are already there. It has no channel setting of its own.
pub fn live_channel() -> Option<u64> {
    super::chess::live_channel()
}

/// House points for the FIRST person to crack a puzzle. Nobody else ever earns
/// house points from one.
fn first_points() -> i64 {
    control::number("VIZIER_POINTS_PUZZLE", 1).min(100) as i64
}

/// The daily limit on those house points, as the panel has it. Nought — the
/// default — means no house points are paid at all.
pub fn daily_cap() -> i64 {
    control::number("VIZIER_CAP_PUZZLE", 0).min(100) as i64
}

/// Whether the puzzle pays house points at all right now.
pub fn pays_house_points() -> bool {
    daily_cap() > 0 && first_points() > 0
}

/// How long a solved puzzle stays up so other people can still try it.
fn next_minutes() -> i64 {
    control::number("VIZIER_PUZZLE_NEXT_MINUTES", 10).clamp(1, 1440) as i64
}

/// How long a puzzle nobody has solved stays up before it is replaced.
fn idle_minutes() -> i64 {
    control::number("VIZIER_PUZZLE_IDLE_MINUTES", 60).clamp(5, 10080) as i64
}

fn no_repeat_days() -> i64 {
    control::number("VIZIER_PUZZLE_NO_REPEAT_DAYS", 60).min(3650) as i64
}

/// How many messages from other people have to land under the card before it
/// follows the conversation down.
fn bump_messages() -> u64 {
    control::number("VIZIER_PUZZLE_BUMP_MESSAGES", 5).clamp(1, 100)
}

/// And how long between two of those moves, however busy the channel is.
fn bump_seconds() -> i64 {
    control::number("VIZIER_PUZZLE_BUMP_SECONDS", 120).clamp(10, 3600) as i64
}

/// Everything `/puzzlehelp` says about the game, from the settings as they are.
pub fn puzzle_rules() -> super::rules_text::PuzzleRules {
    super::rules_text::PuzzleRules {
        channel: live_channel(),
        first: first_points(),
        cap: daily_cap(),
        band_points: BAND_POINTS,
        next_minutes: next_minutes(),
        idle_minutes: idle_minutes(),
        no_repeat_days: no_repeat_days(),
        puzzles: bank::bank().map(bank::Bank::count),
        attribution: bank::bank().map(|b| b.attribution().to_string()).unwrap_or_else(|| bank::ATTRIBUTION.to_string()),
    }
}

// --- what a puzzle is worth -----------------------------------------------------------

/// Puzzle points by band: easy, medium, hard. A harder puzzle is worth more,
/// which is the whole of the difference between them.
pub const BAND_POINTS: [i64; 3] = [1, 2, 3];

/// What one puzzle is worth in puzzle points, from its band. An unknown band
/// reads as the easiest, which is the safe way to be wrong.
pub fn band_points(band: &str) -> i64 {
    match band {
        "hard" => BAND_POINTS[2],
        "medium" => BAND_POINTS[1],
        _ => BAND_POINTS[0],
    }
}

/// The band as a card names it.
pub fn band_words(band: &str) -> &'static str {
    match band {
        "hard" => "🔴 Hard",
        "medium" => "🟡 Medium",
        _ => "🟢 Easy",
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "47 s", "2 m 10 s" — how long somebody took.
pub fn spent_words(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{} s", s.max(0)),
        s if s < 3600 => format!("{} m {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} m", s / 3600, (s % 3600) / 60),
    }
}

// --- the card ---------------------------------------------------------------------------

/// Everything the live card says, worked out before any of it is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub puzzle_id: i64,
    /// The bank's own id, for the link to the whole puzzle.
    pub bank_id: String,
    pub band: String,
    pub rating: i64,
    /// Whether the solver plays white.
    pub solver_is_white: bool,
    /// The opponent's move that set it up, as a person reads it.
    pub setup_san: String,
    /// How many moves the solver has to find.
    pub solver_moves: usize,
    /// The puzzle points on offer to everyone who solves it.
    pub worth: i64,
    /// The house points the FIRST solver takes — nought while the daily limit
    /// is nought, which is how the game ships.
    pub first_point: i64,
    /// Who cracked it first, if anybody has.
    pub first_solver: Option<u64>,
    /// How long they took.
    pub first_seconds: i64,
    /// How many people have solved it since.
    pub since_first: i64,
    /// How many have opened its page.
    pub playing: i64,
    /// Seconds until the next puzzle goes up, once this one is solved.
    pub next_in: Option<i64>,
}

pub fn card_title(puzzle_id: i64) -> String {
    format!("🧩 Chess puzzle #{}", puzzle_id)
}

/// The footer every live card carries. It says the quiet part out loud: a
/// computer finds these instantly, and doing that is not playing.
pub const CARD_FOOTER: &str =
    "An engine would find this in a blink — the point is to find it yourself · ❓ How to play for the rules";

pub fn card_text(live: &Live) -> String {
    let side = if live.solver_is_white { "White" } else { "Black" };
    let them = if live.solver_is_white { "Black" } else { "White" };
    let mut text = format!(
        "**{} to play.** {} has just played **{}**.\nFind the winning line — **{}** · {} ({})",
        side,
        them,
        live.setup_san,
        plural(live.solver_moves as i64, "move", "moves"),
        band_words(&live.band),
        live.rating
    );
    text.push_str("\n🧩 **Solve it** opens a private board of your own: tap a piece, tap where it goes.");
    match live.first_solver {
        Some(who) => {
            text.push_str(&format!("\n\n🥇 <@{}> cracked it first, in **{}**", who, spent_words(live.first_seconds)));
            text.push_str(&match live.since_first {
                0 => " — nobody else has yet.".to_string(),
                n => format!(" · **{}** have solved it since.", plural(n, "person", "people")),
            });
            if let Some(secs) = live.next_in {
                text.push_str(&format!("\n-# The next puzzle goes up in **{}** — there is still time to try this one.", spent_words(secs)));
            }
        }
        None => {
            text.push_str(&match live.playing {
                0 => "\n\n-# Nobody has cracked it yet.".to_string(),
                n => format!("\n\n-# Nobody has cracked it yet · **{}** on it right now.", plural(n, "person", "people")),
            });
        }
    }
    text.push_str(&format!("\n-# {}", worth_words(live)));
    text
}

/// The one line that says what a puzzle is worth. With house points switched off
/// — which is how this ships — it names only the puzzle points, because that is
/// then the whole of the score; it never says "no points", and being first is
/// still worth being first.
pub fn worth_words(live: &Live) -> String {
    let mine = plural(live.worth, "puzzle point", "puzzle points");
    if live.first_point > 0 {
        return format!(
            "First to solve takes **+{}** · everyone who solves scores **{}**, with no daily limit · `/puzzletop`",
            plural(live.first_point, "house point", "house points"),
            mine
        );
    }
    format!("Everyone who solves scores **{}**, with no daily limit — being first is for the glory · `/puzzletop`", mine)
}

/// What a puzzle's card becomes once the puzzle has made way for the next one:
/// the answer, and who found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub puzzle_id: i64,
    pub bank_id: String,
    pub band: String,
    pub solver_is_white: bool,
    /// The whole line, in the notation a person reads: "Bd5+ Kg7 Bxa2".
    pub line_san: String,
    pub first_solver: Option<u64>,
    pub first_seconds: i64,
    /// How many solved it in all, the first one included.
    pub solvers: i64,
    /// Whether a mod took it down.
    pub skipped: bool,
}

pub fn ended_title(e: &Ended) -> String {
    match (e.skipped, e.first_solver) {
        (true, _) => format!("🧩 Puzzle #{} — dropped", e.puzzle_id),
        (_, Some(_)) => format!("🧩 Puzzle #{} — solved", e.puzzle_id),
        (_, None) => format!("🧩 Puzzle #{} — nobody found it", e.puzzle_id),
    }
}

pub fn ended_text(e: &Ended) -> String {
    let side = if e.solver_is_white { "White" } else { "Black" };
    let mut text = match (e.skipped, e.first_solver) {
        (true, _) => "A mod dropped this one and set a fresh puzzle.".to_string(),
        (_, Some(who)) => {
            let others = (e.solvers - 1).max(0);
            let rest = match others {
                0 => String::new(),
                n => format!(" · **{}** solved it after them", plural(n, "person", "people")),
            };
            format!("🥇 <@{}> cracked it first, in **{}**{}", who, spent_words(e.first_seconds), rest)
        }
        (_, None) => "Nobody found this one in time.".to_string(),
    };
    text.push_str(&format!("\n{} to play — the line was **{}**.", side, e.line_san));
    text.push_str(&format!("\n-# {} · the whole puzzle: {}", band_words(&e.band), bank::lichess_link(&e.bank_id)));
    text
}

pub fn card_rows() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(SOLVE_ID).label("🧩 Solve it").style(ButtonStyle::Primary),
        CreateButton::new(HELP_ID).label("❓ How to play").style(ButtonStyle::Secondary),
    ])]
}

fn live_of(row: &store::Row, playing: i64, now: i64) -> Live {
    let setup_san = row.setup_san.clone();
    Live {
        puzzle_id: row.id,
        bank_id: row.bank_id.clone(),
        band: row.band.clone(),
        rating: row.rating,
        solver_is_white: row.solver_is_white(),
        setup_san,
        solver_moves: row.solver_moves(),
        worth: band_points(&row.band),
        first_point: if pays_house_points() { first_points() } else { 0 },
        first_solver: row.first_solver,
        first_seconds: row.first_ts.map(|t| t - row.posted_ts).unwrap_or(0),
        since_first: (row.solvers - 1).max(0),
        playing,
        next_in: row.ends_at.map(|at| (at - now).max(0)),
    }
}

/// The whole solution in the notation a person reads.
pub fn line_san(fen: &str, line: &[String]) -> String {
    let Some(mut pos) = rules::position_of(fen) else { return line.join(" ") };
    let mut out = Vec::new();
    for uci in line {
        match rules::san_of(&pos, uci) {
            Some(san) => {
                out.push(san);
                match rules::play(&pos, uci) {
                    Some(next) => pos = next,
                    None => break,
                }
            }
            None => {
                out.push(uci.clone());
                break;
            }
        }
    }
    out.join(" ")
}

fn ended_of(row: &store::Row) -> Ended {
    Ended {
        puzzle_id: row.id,
        bank_id: row.bank_id.clone(),
        band: row.band.clone(),
        solver_is_white: row.solver_is_white(),
        line_san: line_san(&row.fen, &row.line),
        first_solver: row.first_solver,
        first_seconds: row.first_ts.map(|t| t - row.posted_ts).unwrap_or(0),
        solvers: row.solvers,
        skipped: row.ended_why == "skipped",
    }
}

fn view_of(row: &store::Row) -> View {
    // UCI writes castling as the king's own two-square move, so the first four
    // characters of the setting-up move are always the pair to highlight.
    let last = row.setup.get(..4).unwrap_or_default().to_string();
    let check = rules::position_of(&row.fen).map(|p| rules::check_square(&p)).unwrap_or_default();
    // Drawn from the solver's own side: it is their move.
    View { placement: row.fen.clone(), last, check, flipped: !row.solver_is_white() }
}

async fn board_attachment(row: &store::Row) -> Option<CreateAttachment> {
    let view = view_of(row);
    let png = tokio::task::spawn_blocking(move || chess_board::board_png(&view)).await.ok().flatten()?;
    Some(CreateAttachment::bytes(png.to_vec(), BOARD_FILE))
}

fn card_embed(row: &store::Row, playing: i64, now: i64) -> CreateEmbed {
    CreateEmbed::new()
        .title(card_title(row.id))
        .description(card_text(&live_of(row, playing, now)))
        .colour(COLOUR)
        .image(format!("attachment://{}", BOARD_FILE))
        .footer(CreateEmbedFooter::new(CARD_FOOTER))
}

// --- shared state -------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    /// (channel, message) of the puzzle card. The ONE card the puzzle has.
    card: Option<(u64, u64)>,
    /// Messages from other people that have landed under the card.
    others_since_card: u64,
    /// When the card last moved down.
    last_bump_ms: i64,
    /// Someone deleted the card.
    card_gone: bool,
    /// The card's words are out of date (somebody solved it, somebody opened it).
    dirty: bool,
    /// A puzzle was just cracked for the first time: the task redraws its card.
    cracked: Option<i64>,
    /// A mod asked for a fresh puzzle.
    skip: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// Whether a message is the puzzle's own card.
///
/// The chess game lives in the same channel and keeps its own card at the
/// bottom. Without this the two would chase each other down the channel for
/// ever: chess would move below the puzzle card, the puzzle would move below
/// that, and so on. Chess asks this before counting a message, so the puzzle's
/// card is the one thing it does not follow.
pub fn owns_message(id: u64) -> bool {
    SHARED.lock().card.is_some_and(|(_, card)| card == id)
}

/// Every message in the chess channel, the bot's own included. Only OTHER
/// people's messages count towards burying the card: the bot's own chess cards
/// would otherwise move the puzzle for no reason.
pub fn note_message(ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != live_channel() {
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
    if Some(channel.get()) != live_channel() {
        return;
    }
    let mut s = SHARED.lock();
    if s.card.map(|(_, m)| m) == Some(id.get()) {
        s.card_gone = true;
    }
}

// --- Discord helpers ------------------------------------------------------------------------

async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, serenity::Error> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(result) => result,
        Err(_) => Err(serenity::Error::Other("no answer from Discord in time")),
    }
}

fn is_gone(err: &serenity::Error) -> bool {
    match err {
        serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(r)) => {
            r.status_code.as_u16() == 404 || r.error.code == 10008
        }
        _ => false,
    }
}

async fn exists(ctx: &Context, channel: u64, id: u64) -> bool {
    call(ChannelId::new(channel).message(&ctx.http, MessageId::new(id))).await.is_ok()
}

async fn delete(ctx: &Context, channel: u64, message: u64) {
    if let Err(err) = call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        tracing::debug!("puzzle: message {} not deleted: {}", message, err);
    }
}

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), key, value);
    }
}

/// The channel the puzzle may run in right now: on, with a bank, and a chess
/// channel to live in.
fn active_channel() -> Option<u64> {
    if !switched_on() || bank::bank().is_none_or(bank::Bank::is_empty) {
        return None;
    }
    live_channel()
}

// --- posting -----------------------------------------------------------------------------

/// Posts the card and remembers it. `moved` says this was a real MOVE of the
/// card rather than a first posting, which is what paces the next one: counting
/// every card posted would have a busy channel bumping on every redraw.
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
            tracing::warn!("puzzle: card not posted in {}: {}", channel, err);
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

/// Which card an ending puzzle may take away: its own, and only if that is
/// still the card the channel is showing. `None` for a puzzle whose card was
/// never recorded — guessing would cost the next puzzle its card.
pub fn card_to_drop(current: Option<(u64, u64)>, ended: Option<u64>) -> Option<(u64, u64)> {
    match (current, ended) {
        (Some((channel, card)), Some(ended)) if card == ended => Some((channel, card)),
        _ => None,
    }
}

async fn card_message(row: &store::Row, playing: i64, now: i64) -> CreateMessage {
    let message =
        CreateMessage::new().embed(card_embed(row, playing, now)).components(card_rows()).allowed_mentions(CreateAllowedMentions::new());
    match board_attachment(row).await {
        Some(file) => message.add_file(file),
        None => message,
    }
}

/// Redraws the live card where it is.
async fn edit_card(ctx: &Context, row: &store::Row, playing: i64, now: i64) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let edit = EditMessage::new()
        .embed(card_embed(row, playing, now))
        .components(card_rows())
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        if is_gone(&err) {
            SHARED.lock().card_gone = true;
        }
        tracing::debug!("puzzle: card not edited: {}", err);
    }
}

/// Turns a finished puzzle's own card into the answer card: same message, so it
/// keeps its place in the channel, with the buttons taken off.
async fn show_ended(ctx: &Context, row: &store::Row) {
    let card = {
        let mut s = SHARED.lock();
        match card_to_drop(s.card, row.message) {
            Some(card) => {
                s.card = None;
                Some(card)
            }
            None => None,
        }
    };
    let Some((channel, message)) = card else { return };
    meta_set("card", "");
    let ended = ended_of(row);
    let edit = EditMessage::new()
        .embed(
            CreateEmbed::new()
                .title(ended_title(&ended))
                .description(ended_text(&ended))
                .colour(SOLVED_COLOUR)
                .image(format!("attachment://{}", BOARD_FILE)),
        )
        .components(Vec::new())
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::debug!("puzzle: answer card not shown: {}", err);
    }
}

/// Sets a fresh puzzle and puts its card up.
async fn post_puzzle(ctx: &Context, channel: u64) -> Option<store::Row> {
    let bank = bank::bank()?;
    let now = Utc::now().timestamp();
    let db = store::db()?;
    let row = {
        let conn = db.lock();
        let used = store::set_since(&conn, now - no_repeat_days() * 86_400);
        let mut rng = Rng::fresh();
        // A puzzle whose position doesn't hold up is passed over rather than put
        // in front of anybody: the pack is verified, but a bad row must never
        // reach the channel as a nonsense board.
        let mut chosen = None;
        for _ in 0..20 {
            let puzzle = bank.pick(&used, &mut rng)?;
            if let Some(open) = rules::open_with(&puzzle.fen, &puzzle.setup) {
                chosen = Some((puzzle, open));
                break;
            }
            tracing::warn!("puzzle: {} does not set up and was passed over", puzzle.id);
        }
        let (puzzle, open) = chosen?;
        match store::add(
            &conn,
            &puzzle.id,
            &open.fen,
            &puzzle.setup,
            &open.setup_san,
            &puzzle.line,
            open.solver_is_white,
            puzzle.rating as i64,
            &puzzle.band,
            &puzzle.themes,
            channel,
            now,
        ) {
            Ok(row) => row,
            Err(err) => {
                tracing::warn!("puzzle: not saved: {}", err);
                return None;
            }
        }
    };
    let message = card_message(&row, 0, now).await;
    let posted = place_card(ctx, channel, message, false, false).await?;
    if let Some(db) = store::db() {
        let _ = store::set_message(&db.lock(), row.id, Some(posted));
    }
    tracing::info!("puzzle: {} ({}, {}) up in {}", row.id, row.bank_id, row.band, channel);
    Some(store::Row { message: Some(posted), ..row })
}

// --- the card's one decision ---------------------------------------------------------------

/// Whether chat has buried the card: enough messages from other people have
/// landed under it AND the card hasn't already moved in the last little while.
pub fn bump_due(others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> bool {
    others >= needed.max(1) && since_bump_ms >= every_secs.max(0) * 1_000
}

/// What to do with the card this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Nothing,
    /// Redraw it where it is (somebody solved it, somebody opened it).
    Edit,
    /// Post it again at the bottom and take the old one away.
    Bump,
}

/// The ONE decision behind the card, so nothing else in the game posts one.
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

/// Whether the puzzle that is up has had its time: solved and past its window,
/// or untouched for the idle window.
pub fn due_to_go(first_solved: bool, ends_at: Option<i64>, posted_ts: i64, idle_secs: i64, now: i64) -> bool {
    match (first_solved, ends_at) {
        (true, Some(at)) => now >= at,
        // Solved but with no window written (an old row): give it the idle one.
        (true, None) => now - posted_ts >= idle_secs,
        (false, _) => now - posted_ts >= idle_secs,
    }
}

async fn tend_card(ctx: &Context, channel: u64, row: &store::Row, playing: i64, now: i64, last_edit: i64) -> (CardAction, i64) {
    let plan = {
        let s = SHARED.lock();
        let since_bump = Utc::now().timestamp_millis() - s.last_bump_ms;
        let right = s.card.map(|(c, _)| c) == Some(channel);
        let dirty = s.dirty && now - last_edit >= EDIT_EVERY_SECS;
        card_plan(s.card.is_some(), right, s.card_gone, dirty, s.others_since_card, since_bump, bump_messages(), bump_seconds())
    };
    match plan {
        CardAction::Nothing => (CardAction::Nothing, last_edit),
        CardAction::Edit => {
            edit_card(ctx, row, playing, now).await;
            SHARED.lock().dirty = false;
            (CardAction::Edit, now)
        }
        CardAction::Bump => {
            let (card, gone) = {
                let s = SHARED.lock();
                (s.card, s.card_gone)
            };
            let replace = card.is_some() && !gone;
            let message = card_message(row, playing, now).await;
            if let Some(id) = place_card(ctx, channel, message, replace, true).await {
                if let Some(db) = store::db() {
                    let _ = store::set_message(&db.lock(), row.id, Some(id));
                }
            }
            (CardAction::Bump, now)
        }
    }
}

// --- the task ------------------------------------------------------------------------------

pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), live_channel(), bank::bank().map(bank::Bank::count)) {
        (false, _, _) => tracing::info!("puzzle: VIZIER_PUZZLE is off"),
        (true, None, _) => tracing::info!("puzzle: on, but chess has no channel to live in"),
        (true, _, None) => tracing::info!("puzzle: on, but there is no puzzle bank — the rest of chess is unaffected"),
        (true, Some(c), Some(n)) => tracing::info!("puzzle: {} puzzles, playing in {}", n, c),
    }
    tokio::spawn(run(ctx));
}

/// After a start: pick up the card the last run left, and the puzzle it was for.
async fn recover(ctx: &Context) -> Option<store::Row> {
    let channel = active_channel();
    let db = store::db()?;
    let (old_card, live) = {
        let conn = db.lock();
        (store::meta_get(&conn, "card"), store::live(&conn))
    };
    let old_card =
        old_card.as_deref().and_then(|v| v.split_once(':')).and_then(|(c, m)| Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?)));
    if let Some(c) = channel {
        let read = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await.is_ok();
        let mut s = SHARED.lock();
        s.others_since_card = 0;
        s.last_bump_ms = Utc::now().timestamp_millis();
        if !read {
            tracing::debug!("puzzle: couldn't read {} on waking", c);
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
                tracing::info!("puzzle: {} picked up where it was left", row.id);
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
    let mut last_edit = 0i64;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let channel = active_channel();
        let (cracked, skip) = {
            let mut s = SHARED.lock();
            (s.cracked.take(), std::mem::take(&mut s.skip))
        };
        if cracked.is_some() {
            SHARED.lock().dirty = true;
        }

        let Some(channel) = channel else {
            if SHARED.lock().card.is_some() {
                drop_card(&ctx).await;
            }
            live = None;
            continue;
        };

        // The store is the truth: a puzzle taken down elsewhere is not live.
        if let Some(row) = &live {
            live = store::db().and_then(|db| store::get(&db.lock(), row.id)).filter(|r| r.open() && r.channel == channel);
        }

        // A mod's skip, and a puzzle that has had its time, come to the same
        // thing: close it, show the answer on its own card, and set another.
        if let Some(row) = live.clone() {
            let why = if skip {
                Some("skipped")
            } else if due_to_go(row.first_solver.is_some(), row.ends_at, row.posted_ts, idle_minutes() * 60, now) {
                Some(if row.first_solver.is_some() { "solved" } else { "idle" })
            } else {
                None
            };
            if let Some(why) = why {
                let closed = store::db().map(|db| store::close(&db.lock(), row.id, why, now).unwrap_or(false)).unwrap_or(false);
                if closed {
                    let ended = store::db().and_then(|db| store::get(&db.lock(), row.id)).unwrap_or(row);
                    tracing::info!("puzzle: {} closed ({}), {} solved it", ended.id, why, ended.solvers);
                    show_ended(&ctx, &ended).await;
                }
                live = None;
                last_edit = 0;
            }
        }

        if live.is_none() {
            let open = store::db().and_then(|db| store::live(&db.lock()));
            live = match open {
                Some(row) if row.channel == channel => {
                    SHARED.lock().card = row.message.map(|m| (channel, m));
                    Some(row)
                }
                Some(stale) => {
                    if let Some(db) = store::db() {
                        let _ = store::close(&db.lock(), stale.id, "moved", now);
                    }
                    post_puzzle(&ctx, channel).await
                }
                None => post_puzzle(&ctx, channel).await,
            };
            last_edit = 0;
        }
        let Some(row) = live.clone() else { continue };
        let playing = store::db().map(|db| store::playing(&db.lock(), row.id)).unwrap_or(0);
        let (_, edited) = tend_card(&ctx, channel, &row, playing, now, last_edit).await;
        last_edit = edited;
    }
}

// --- playing it ------------------------------------------------------------------------------

/// What became of one move somebody played on their page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attempt {
    /// Whether the move was accepted.
    pub ok: bool,
    /// Whether that finished the puzzle.
    pub solved: bool,
    /// Whether they were the first to finish it.
    pub first: bool,
    /// The move they played, as a person reads it.
    pub san: String,
    /// The opponent's reply, if the line had one.
    pub reply: String,
    /// The position after both, as a FEN.
    pub fen: String,
    /// The squares of the last move played, for the picture.
    pub last: String,
    pub check: String,
    /// How many of the solver's moves are behind them, and how many there are.
    pub done: i64,
    pub total: i64,
    /// What to tell them.
    pub words: String,
    /// House points paid, and puzzle points scored.
    pub points: i64,
    pub worth: i64,
    /// Their puzzle points today, this solve included.
    pub tally: i64,
}

/// Plays one move for one person. The ONE door the page goes through, so a page
/// can be as wrong as it likes without a bad move ever landing.
///
/// A wrong move does NOT end the attempt: it is counted, and they may try again
/// from the same position. Solving means playing the whole line — or mating,
/// which finishes it wherever in the line they are.
pub fn attempt(user: u64, puzzle_id: i64, typed: &str) -> Option<Attempt> {
    let db = store::db()?;
    let now = Utc::now().timestamp();
    let (row, player) = {
        let conn = db.lock();
        let row = store::get(&conn, puzzle_id).filter(store::Row::open)?;
        let player = store::begin(&conn, puzzle_id, user, now).ok()?;
        (row, player)
    };
    let total = row.solver_moves() as i64;
    if player.solved {
        return Some(Attempt {
            ok: false,
            solved: true,
            done: player.done,
            total,
            fen: at_move(&row, player.done as usize).unwrap_or_else(|| row.fen.clone()),
            words: "You have already solved this one.".to_string(),
            ..Default::default()
        });
    }
    let done = player.done as usize;
    let fen = at_move(&row, done)?;
    let pos = rules::position_of(&fen)?;
    let expected = row.solver_move(done)?;
    let (verdict, after) = rules::judge(&pos, typed, expected);
    let Some(after) = after else {
        let conn = db.lock();
        let _ = store::wrong(&conn, puzzle_id, user, now);
        return Some(Attempt {
            ok: false,
            done: player.done,
            total,
            fen,
            check: rules::check_square(&pos),
            words: rules::NOT_IT.to_string(),
            ..Default::default()
        });
    };
    let san = rules::san_of(&pos, typed).unwrap_or_else(|| typed.to_string());
    let last = rules::squares_of(&pos, typed).unwrap_or_default();
    // A mate ends it wherever in the line it came, so the whole line being
    // played and a mate found early are one and the same thing here.
    let finished = verdict == rules::Verdict::Mate || done + 1 >= total as usize;
    let reply_uci = if finished { None } else { row.reply_to(done) };
    let (pos_now, reply_san, last_now) = match reply_uci.and_then(|uci| {
        let san = rules::san_of(&after, uci)?;
        let squares = rules::squares_of(&after, uci)?;
        Some((rules::play(&after, uci)?, san, squares))
    }) {
        Some((pos, san, squares)) => (pos, san, squares),
        None => (after, String::new(), last.clone()),
    };
    let stepped = {
        let conn = db.lock();
        store::step(&conn, puzzle_id, user, player.done, finished, now).unwrap_or(false)
    };
    if !stepped {
        // Two presses landed together; the other one is the one that counted.
        return Some(Attempt {
            ok: false,
            done: player.done,
            total,
            fen,
            words: "That move crossed with another — the board has already moved on.".to_string(),
            ..Default::default()
        });
    }
    let mut out = Attempt {
        ok: true,
        solved: finished,
        first: false,
        san,
        reply: reply_san,
        fen: rules::fen_of(&pos_now),
        last: last_now,
        check: rules::check_square(&pos_now),
        done: player.done + 1,
        total,
        words: String::new(),
        points: 0,
        worth: 0,
        tally: 0,
    };
    if !finished {
        out.words = if out.reply.is_empty() {
            format!("**{}** — right. Keep going.", out.san)
        } else {
            format!("**{}** — right. They answered **{}**.", out.san, out.reply)
        };
        SHARED.lock().dirty = true;
        return Some(out);
    }
    let scored = score(&row, user, player.started_ts.max(row.posted_ts), now);
    out.first = scored.0;
    out.points = scored.1;
    out.worth = scored.2;
    out.tally = scored.3;
    out.words = solved_words(&out);
    Some(out)
}

/// What a page says when the last move lands. With house points switched off it
/// names only the puzzle points, because that is then the whole of the score —
/// and being first is still being first.
pub fn solved_words(a: &Attempt) -> String {
    let mut text = format!("🎉 **{}** — solved it!", a.san);
    if a.first {
        text.push_str(" You were the **first** to crack this one.");
    }
    text.push_str(&format!(" **+{}**", plural(a.worth, "puzzle point", "puzzle points")));
    if a.points > 0 {
        text.push_str(&format!(" · **+{}** for being first", plural(a.points, "house point", "house points")));
    }
    text.push_str(&format!(" · {} today.", plural(a.tally, "puzzle point", "puzzle points")));
    text
}

/// The position after `done` of the solver's moves have been played, with the
/// opponent's replies in between.
fn at_move(row: &store::Row, done: usize) -> Option<String> {
    let mut pos = rules::position_of(&row.fen)?;
    for uci in row.line.iter().take(done * 2) {
        pos = rules::play(&pos, uci)?;
    }
    Some(rules::fen_of(&pos))
}

/// Writes a solve: the first solver's house point if there is one to pay, and
/// the puzzle points everybody gets. Returns (first, house points, puzzle
/// points, their puzzle points today).
fn score(row: &store::Row, user: u64, started: i64, now: i64) -> (bool, i64, i64, i64) {
    let Some(db) = store::db() else { return (false, 0, 0, 0) };
    let worth = band_points(&row.band);
    let ends_at = now + next_minutes() * 60;
    // The claim is one statement under one lock, and never across an await: the
    // database decides the race, not whoever the task happens to look at first.
    let first = {
        let conn = db.lock();
        store::claim_first(&conn, row.id, user, now, ends_at).unwrap_or(false)
    };
    let mut points = 0;
    if first && pays_house_points() {
        let reason = format!("chess puzzle {} ({}) — first to solve it", row.id, row.bank_id);
        let key = format!("puzzle:{}:{}", row.id, user);
        match super::house::award_person(user, Source::Puzzle, first_points(), &reason, None, Some(key), None) {
            Some((_, Outcome::Granted(n))) => points = n,
            _ => points = 0,
        }
    }
    let day = super::points::ist_day(now);
    let tally = {
        let conn = db.lock();
        let _ = store::count_solver(&conn, row.id);
        let _ = store::add_solve(&conn, &day, user, row.id, points, worth, first, (now - started).max(0), now);
        store::day_solves(&conn, &day).iter().filter(|s| s.user == user).map(|s| s.worth).sum::<i64>()
    };
    if first {
        SHARED.lock().cracked = Some(row.id);
    }
    SHARED.lock().dirty = true;
    tracing::info!(
        "puzzle: {} solved by {}{} for {} puzzle points ({} house points)",
        row.id,
        user,
        if first { " (first)" } else { "" },
        worth,
        points
    );
    (first, points, worth, tally.max(worth))
}

// --- the buttons ----------------------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    match component.data.custom_id.as_str() {
        SOLVE_ID => solve_pressed(ctx, component).await,
        HELP_ID => {
            let message =
                CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
        }
        _ => {}
    }
}

fn live_row() -> Option<store::Row> {
    store::db().and_then(|db| store::live(&db.lock()))
}

const OFF: &str = "The chess puzzle is switched off right now.";

/// The private address of one person's board for one puzzle.
pub fn page_link(puzzle_id: i64, user: u64) -> Option<String> {
    let base = control::web::panel_url()?;
    let db = store::db()?;
    let now = Utc::now().timestamp();
    let token = {
        let conn = db.lock();
        store::token_for(&conn, puzzle_id, user, now, || rand::random::<f64>())
    }?;
    Some(format!("{}/puzzle/{}/{}", base, puzzle_id, token))
}

/// What the 🧩 button answers with: their own link, and where they are in it.
pub fn solve_text(row: &store::Row, player: Option<&store::Player>, link: Option<&str>) -> String {
    let side = if row.solver_is_white() { "White" } else { "Black" };
    let mut text = format!("🧩 **Puzzle #{}** · {} to play · **{}**", row.id, side, plural(row.solver_moves() as i64, "move", "moves"));
    match player {
        Some(p) if p.solved => text.push_str("\nYou have already solved this one — the board is there to look at again."),
        Some(p) if p.done > 0 => text.push_str(&format!("\nYou are **{}** into the line. Carry on where you left off.", plural(p.done, "move", "moves"))),
        _ => text.push_str("\nTap a piece, then tap where it goes. A wrong move is just refused — try as often as you like."),
    }
    match link {
        Some(url) => text.push_str(&format!("\n**[Open your board]({})** — the link is yours alone and stops working when the puzzle is replaced.", url)),
        None => text.push_str("\n-# The board page needs the panel's web address to be set; a mod can do that."),
    }
    text.push_str("\n-# An engine would find this in a blink. The point is to find it yourself.");
    text
}

async fn solve_pressed(ctx: &Context, component: &ComponentInteraction) {
    let Some(row) = live_row().filter(store::Row::open) else {
        whisper(ctx, component, OFF).await;
        return;
    };
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let player = store::db().and_then(|db| {
        let conn = db.lock();
        store::begin(&conn, row.id, user, now).ok()
    });
    let link = page_link(row.id, user);
    SHARED.lock().dirty = true;
    whisper(ctx, component, solve_text(&row, player.as_ref(), link.as_deref())).await;
}

// --- the board ---------------------------------------------------------------------------------

/// Where someone stands on a ranked board, counting from one.
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

/// One person's own line on a board.
pub fn standing(rows: &[store::Tally], me: u64) -> Option<String> {
    let place = place_of(rows, me)?;
    let mine = rows.get(place - 1)?;
    let firsts = match mine.firsts {
        0 => String::new(),
        n => format!(" · {} first", n),
    };
    Some(format!(
        "{} of {} · **{}** · {}{}",
        ordinal(place),
        rows.len(),
        plural(mine.points, "puzzle point", "puzzle points"),
        plural(mine.solves, "puzzle", "puzzles"),
        firsts
    ))
}

/// How many names `/puzzletop` lists.
const TOP_LIST: usize = 10;

pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("🧩 **Puzzle points** · {}", period);
    if rows.is_empty() {
        text.push_str("\nNobody has solved one yet. The card is waiting in the chess channel.");
        return text;
    }
    for (i, t) in rows.iter().take(TOP_LIST).enumerate() {
        let rank = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            n => format!("`{:>2}.`", n + 1),
        };
        let firsts = match t.firsts {
            0 => String::new(),
            n => format!(" · {} first", n),
        };
        let you = if t.user == me { " ← you" } else { "" };
        text.push_str(&format!("\n{} <@{}> **{}** · {}{}{}", rank, t.user, t.points, plural(t.solves, "puzzle", "puzzles"), firsts, you));
    }
    if place_of(rows, me).is_some_and(|p| p > TOP_LIST) {
        if let Some(mine) = standing(rows, me) {
            text.push_str(&format!("\n-# **You:** {}", mine));
        }
    }
    text.push_str("\n-# Puzzle points count every solve, first or not, with no daily limit — and everyone has them.");
    text
}

/// The first and last day of the India month a day falls in.
pub fn month_ends(day: &str) -> (String, String) {
    let month = day.get(..7).unwrap_or(day);
    (format!("{}-01", month), format!("{}-31", month))
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

/// What `/puzzle` shows whoever ran it.
pub fn mine_text(live: Option<&store::Row>, today: &[store::Tally], month: &[store::Tally], me: u64, channel: Option<u64>) -> String {
    let mut lines = vec!["🧩 **Chess puzzle**".to_string()];
    match live {
        Some(row) => {
            let side = if row.solver_is_white() { "White" } else { "Black" };
            lines.push(format!(
                "**Puzzle #{}** · {} to play · **{}** · {} ({})",
                row.id,
                side,
                plural(row.solver_moves() as i64, "move", "moves"),
                band_words(&row.band),
                row.rating
            ));
            match row.first_solver {
                Some(who) => lines.push(format!("🥇 <@{}> cracked it first · **{}** have solved it", who, plural(row.solvers, "person", "people"))),
                None => lines.push("Nobody has cracked it yet.".to_string()),
            }
        }
        None => lines.push("No puzzle is up right now — the next one is on its way.".to_string()),
    }
    lines.push(match standing(today, me) {
        Some(mine) => format!("-# **You today:** {}", mine),
        None => "-# You haven't solved one today. Press 🧩 Solve it on the card.".to_string(),
    });
    if let Some(mine) = standing(month, me) {
        lines.push(format!("-# **This month:** {}", mine));
    }
    if let Some(c) = channel {
        lines.push(format!("-# The card lives in <#{}> · `/puzzletop` for the board · `/puzzlehelp` explains the rest.", c));
    }
    lines.join("\n")
}

// --- commands ------------------------------------------------------------------------------------

pub fn mine_builder() -> CreateCommand {
    CreateCommand::new("puzzle").description("the chess puzzle that's up now, and where you stand")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("puzzletop").description("the puzzle points board, today or this month").add_option(
        CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "which days to count")
            .add_string_choice("Today", "today")
            .add_string_choice("This month", "month"),
    )
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("puzzlehelp").description("how the chess puzzle works, from the settings as they are now")
}

pub fn skip_builder() -> CreateCommand {
    CreateCommand::new("puzzleskip").description("admin only: drop the puzzle that's up and set a fresh one")
}

/// `/puzzle` — everyone, shown only to them.
pub async fn mine_command(ctx: &Context, command: &CommandInteraction) {
    let (today, month) = boards_now();
    let text = mine_text(live_row().as_ref(), &today, &month, command.user.id.get(), live_channel());
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

/// `/puzzletop [period]` — everyone, shown only to them.
pub async fn top_command(ctx: &Context, command: &CommandInteraction) {
    let month = command
        .data
        .options
        .iter()
        .any(|o| o.name == "period" && matches!(&o.value, serenity::all::CommandDataOptionValue::String(v) if v == "month"));
    let (today_rows, month_rows) = boards_now();
    let (rows, period) = if month {
        (month_rows, month_label(&super::points::ist_day(Utc::now().timestamp())))
    } else {
        (today_rows, "today".to_string())
    };
    let text = top_text(&period, &rows, command.user.id.get());
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

fn help_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title(super::rules_text::PUZZLE_RULES_TITLE)
        .description(super::rules_text::puzzle_help_text(&puzzle_rules()))
        .colour(COLOUR)
}

/// `/puzzlehelp` — everyone, shown only to them.
pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    let message =
        CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// `/puzzleskip` — admins only.
pub async fn skip_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can drop the puzzle.")).await;
        return;
    }
    if live_row().is_none() {
        reply(ctx, command, CreateInteractionResponseMessage::new().content("No puzzle is up right now.")).await;
        return;
    }
    SHARED.lock().skip = true;
    reply(ctx, command, CreateInteractionResponseMessage::new().content("🧩 Dropped — a fresh puzzle is on its way.")).await;
}

async fn reply(ctx: &Context, command: &CommandInteraction, message: CreateInteractionResponseMessage) {
    let message = message.ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::puzzle_store::tests::memory;

    fn live() -> Live {
        Live {
            puzzle_id: 12,
            bank_id: "WqnOB".into(),
            band: "easy".into(),
            rating: 800,
            solver_is_white: true,
            setup_san: "Bc8".into(),
            solver_moves: 2,
            worth: 1,
            first_point: 1,
            first_solver: None,
            first_seconds: 0,
            since_first: 0,
            playing: 0,
            next_in: None,
        }
    }

    #[test]
    fn the_card_says_whose_move_it_is_and_what_was_just_played() {
        let text = card_text(&live());
        assert!(text.starts_with("**White to play.** Black has just played **Bc8**."), "{}", text);
        assert!(text.contains("Find the winning line — **2 moves** · 🟢 Easy (800)"), "{}", text);
        assert!(text.contains("🧩 **Solve it**"), "{}", text);
        assert!(text.contains("Nobody has cracked it yet."), "{}", text);
        assert_eq!(card_title(12), "🧩 Chess puzzle #12");
        // Black's puzzle reads from black's side.
        let black = card_text(&Live { solver_is_white: false, setup_san: "f8=Q".into(), ..live() });
        assert!(black.starts_with("**Black to play.** White has just played **f8=Q**."), "{}", black);
        // The footer is where the honesty lives.
        assert!(CARD_FOOTER.contains("engine") && CARD_FOOTER.contains("find it yourself"), "{}", CARD_FOOTER);
    }

    #[test]
    fn the_card_names_the_first_solver_and_counts_everyone_after_them() {
        let solved = Live { first_solver: Some(111), first_seconds: 47, since_first: 0, next_in: Some(420), ..live() };
        let text = card_text(&solved);
        assert!(text.contains("🥇 <@111> cracked it first, in **47 s** — nobody else has yet."), "{}", text);
        assert!(text.contains("The next puzzle goes up in **7 m 00 s**"), "{}", text);
        let more = card_text(&Live { since_first: 4, ..solved.clone() });
        assert!(more.contains("**4 people** have solved it since."), "{}", more);
        let one = card_text(&Live { since_first: 1, ..solved });
        assert!(one.contains("**1 person** have solved it since.") || one.contains("**1 person**"), "{}", one);
        // And how many are on it before anyone cracks it.
        assert!(card_text(&Live { playing: 3, ..live() }).contains("**3 people** on it right now"));
    }

    #[test]
    fn with_house_points_switched_off_the_card_names_only_the_puzzle_points() {
        // How the game ships: VIZIER_CAP_PUZZLE is nought, so no house point is
        // paid. Nothing may read as "+1 point" or "no points"; the first solver
        // is still the first solver.
        let off = Live { first_point: 0, ..live() };
        let text = card_text(&off);
        assert!(text.contains("Everyone who solves scores **1 puzzle point**, with no daily limit — being first is for the glory"), "{}", text);
        assert!(!text.contains("house point"), "{}", text);
        assert!(!text.to_lowercase().contains("no points"), "{}", text);
        // Switched on, the first solver's point is named and the rest is the same.
        let on = card_text(&Live { first_point: 1, worth: 3, ..live() });
        assert!(on.contains("First to solve takes **+1 house point** · everyone who solves scores **3 puzzle points**"), "{}", on);
    }

    #[test]
    fn with_house_points_off_nothing_a_player_reads_mentions_them_or_reads_as_nothing() {
        // The state the game ships in. Every word a member can see is walked
        // here: the card before and after a solve, the answer card, the board,
        // the button's reply, `/puzzle`, the solved line, and the help card.
        let conn = memory();
        let row = super::super::puzzle_store::tests::put(&conn, "4kYFv", 1_000);
        let tally = store::Tally { user: 1, points: 3, solves: 2, firsts: 1, reached: 9 };
        let off = Live { first_point: 0, ..live() };
        let ended = Ended {
            puzzle_id: 12,
            bank_id: "WqnOB".into(),
            band: "easy".into(),
            solver_is_white: true,
            line_san: "Bd5+ Kg7 Bxa2".into(),
            first_solver: Some(111),
            first_seconds: 47,
            solvers: 3,
            skipped: false,
        };
        let help = super::super::rules_text::puzzle_help_text(&super::super::rules_text::tests::puzzle_defaults());
        let solved = Attempt { ok: true, solved: true, first: true, san: "Qxh5#".into(), worth: 1, tally: 1, ..Default::default() };
        for text in [
            card_text(&off),
            card_text(&Live { first_solver: Some(111), first_seconds: 47, since_first: 2, next_in: Some(300), ..off }),
            ended_text(&ended),
            ended_text(&Ended { first_solver: None, solvers: 0, ..ended.clone() }),
            top_text("today", &[tally.clone()], 1),
            top_text("today", &[], 1),
            mine_text(Some(&row), &[tally.clone()], &[tally], 1, Some(55)),
            mine_text(None, &[], &[], 1, None),
            solve_text(&row, None, Some("https://panel/puzzle/1/abc")),
            solved_words(&solved),
            solved_words(&Attempt { first: false, ..solved }),
            CARD_FOOTER.to_string(),
        ] {
            let flat = text.to_lowercase();
            assert!(!flat.contains("house point"), "house points are off: {}", text);
            assert!(!flat.contains("house cup"), "the puzzle is nothing to do with it: {}", text);
            assert!(!flat.contains("no points"), "a solve is never reported as nothing: {}", text);
        }
        // The help card is the one place that may name house points at all, and
        // only to say the puzzle does not pay them. It still never reads as
        // "no points", and it still never invokes the House Cup.
        let flat = help.to_lowercase();
        assert!(flat.contains("pay **no house points**"), "{}", help);
        assert!(!flat.contains("no points") && !flat.contains("house cup"), "{}", help);
    }

    #[test]
    fn the_answer_card_shows_the_line_and_who_found_it() {
        let ended = Ended {
            puzzle_id: 12,
            bank_id: "WqnOB".into(),
            band: "easy".into(),
            solver_is_white: true,
            line_san: "Bd5+ Kg7 Bxa2".into(),
            first_solver: Some(111),
            first_seconds: 47,
            solvers: 3,
            skipped: false,
        };
        assert_eq!(ended_title(&ended), "🧩 Puzzle #12 — solved");
        let text = ended_text(&ended);
        assert!(text.contains("🥇 <@111> cracked it first, in **47 s** · **2 people** solved it after them"), "{}", text);
        assert!(text.contains("White to play — the line was **Bd5+ Kg7 Bxa2**."), "{}", text);
        assert!(text.contains("https://lichess.org/training/WqnOB"), "{}", text);

        let nobody = Ended { first_solver: None, solvers: 0, ..ended.clone() };
        assert_eq!(ended_title(&nobody), "🧩 Puzzle #12 — nobody found it");
        assert!(ended_text(&nobody).contains("Nobody found this one in time."));
        let dropped = Ended { skipped: true, ..ended };
        assert_eq!(ended_title(&dropped), "🧩 Puzzle #12 — dropped");
        assert!(ended_text(&dropped).contains("A mod dropped this one"));
    }

    #[test]
    fn the_line_reads_as_moves_a_person_would_write() {
        let p = super::super::puzzle_bank::tests::fixture().get("WqnOB").expect("a puzzle").clone();
        let open = rules::open_with(&p.fen, &p.setup).expect("the opening");
        assert_eq!(line_san(&open.fen, &p.line), "Bd5+ Kg7 Bxa2");
        // Nonsense falls back to the plain moves rather than panicking.
        assert_eq!(line_san("rubbish", &p.line), p.line.join(" "));
    }

    #[test]
    fn a_puzzle_has_its_time_and_then_makes_way() {
        let hour = 3_600;
        // Nobody has solved it: the idle window decides.
        assert!(!due_to_go(false, None, 1_000, hour, 1_000 + hour - 1));
        assert!(due_to_go(false, None, 1_000, hour, 1_000 + hour));
        // Somebody has: the window written when they cracked it decides, and it
        // is deliberately SHORTER, so others still get their go.
        assert!(!due_to_go(true, Some(1_600), 1_000, hour, 1_599));
        assert!(due_to_go(true, Some(1_600), 1_000, hour, 1_600));
        // A solved row from before the window existed falls back to the idle one.
        assert!(!due_to_go(true, None, 1_000, hour, 1_500));
        assert!(due_to_go(true, None, 1_000, hour, 1_000 + hour));
    }

    #[test]
    fn a_card_is_taken_away_only_by_what_owns_it_and_only_while_it_is_up() {
        // The pitfall this game inherited: a blind drop took away the card of
        // the puzzle that had just gone UP, leaving the channel empty.
        assert_eq!(card_to_drop(Some((7, 55)), Some(55)), Some((7, 55)), "its own card, still showing");
        assert_eq!(card_to_drop(Some((7, 56)), Some(55)), None, "the card belongs to the next puzzle now");
        assert_eq!(card_to_drop(None, Some(55)), None, "no card at all");
        assert_eq!(card_to_drop(Some((7, 55)), None), None, "a puzzle whose card was never recorded takes none");
    }

    #[test]
    fn the_card_moves_for_chat_and_not_for_every_card_the_bot_posts() {
        // Bumping is paced by real MOVES of the card, not by every card posted:
        // a redraw must not reset the clock the next move is measured against.
        assert!(!bump_due(4, 999_000, 5, 120), "not enough has landed under it");
        assert!(!bump_due(5, 119_000, 5, 120), "it moved a moment ago");
        assert!(bump_due(5, 120_000, 5, 120));
        assert!(bump_due(50, 120_000, 5, 120));

        // No card, the wrong channel, or a deleted one: put one up.
        assert_eq!(card_plan(false, false, false, false, 0, 0, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, false, false, false, 0, 0, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, true, true, false, 0, 0, 5, 120), CardAction::Bump);
        // Buried: move it. Only out of date: redraw it where it is.
        assert_eq!(card_plan(true, true, false, false, 5, 130_000, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, true, false, true, 0, 0, 5, 120), CardAction::Edit);
        assert_eq!(card_plan(true, true, false, false, 0, 0, 5, 120), CardAction::Nothing);
    }

    #[test]
    fn what_a_puzzle_is_worth_comes_from_its_band() {
        assert_eq!((band_points("easy"), band_points("medium"), band_points("hard")), (1, 2, 3));
        assert_eq!(band_points("nonsense"), 1, "an unknown band reads as the easiest");
        assert_eq!((band_words("easy"), band_words("medium"), band_words("hard")), ("🟢 Easy", "🟡 Medium", "🔴 Hard"));
        assert_eq!(spent_words(47), "47 s");
        assert_eq!(spent_words(130), "2 m 10 s");
        assert_eq!(spent_words(7_320), "2 h 02 m");
    }

    #[test]
    fn the_board_lists_ten_and_finds_the_asker_below_them() {
        let tally = |user: u64, points: i64, solves: i64, firsts: i64| store::Tally { user, points, solves, firsts, reached: 100 + user as i64 };
        let mut rows: Vec<store::Tally> = (1..=12).map(|i| tally(i, (20 - i) as i64, 3, 1)).collect();
        store::rank(&mut rows);
        let text = top_text("today", &rows, 12);
        assert!(text.starts_with("🧩 **Puzzle points** · today"), "{}", text);
        assert!(text.contains("\n🥇 <@1> **19** · 3 puzzles · 1 first"), "{}", text);
        assert!(text.contains("\n`10.` <@10> **10**"), "{}", text);
        assert!(!text.contains("<@11>"), "only ten are listed: {}", text);
        assert!(text.contains("-# **You:** 12th of 12 · **8 puzzle points** · 3 puzzles · 1 first"), "{}", text);
        let inside = top_text("today", &rows, 3);
        assert!(inside.contains("🥉 <@3> **17** · 3 puzzles · 1 first ← you"), "{}", inside);
        assert!(!inside.contains("**You:**"), "{}", inside);
        assert!(text.contains("no daily limit"), "{}", text);
        assert!(top_text("September so far", &[], 1).contains("Nobody has solved one yet"));
        assert_eq!([ordinal(1), ordinal(2), ordinal(3), ordinal(11)].join(" "), "1st 2nd 3rd 11th");
        assert_eq!(month_ends("2026-09-17"), ("2026-09-01".to_string(), "2026-09-31".to_string()));
        assert_eq!(month_label("2026-09-17"), "September so far");
        assert_eq!(month_label("nonsense"), "this month");
    }

    #[test]
    fn what_slash_puzzle_shows_the_asker() {
        let conn = memory();
        let row = super::super::puzzle_store::tests::put(&conn, "4kYFv", 1_000);
        let tally = |user: u64, points: i64, solves: i64, firsts: i64| store::Tally { user, points, solves, firsts, reached: 100 };
        let today = vec![tally(2, 5, 3, 1), tally(1, 3, 2, 0)];
        let month = vec![tally(1, 40, 18, 5), tally(2, 9, 4, 1)];
        let text = mine_text(Some(&row), &today, &month, 1, Some(55));
        assert!(text.contains("**Puzzle #1** · White to play · **1 move** · 🟢 Easy (802)"), "{}", text);
        assert!(text.contains("Nobody has cracked it yet."), "{}", text);
        assert!(text.contains("**You today:** 2nd of 2 · **3 puzzle points** · 2 puzzles"), "{}", text);
        assert!(text.contains("**This month:** 1st of 2 · **40 puzzle points** · 18 puzzles · 5 first"), "{}", text);
        assert!(text.contains("<#55>") && text.contains("`/puzzletop`"), "{}", text);
        let none = mine_text(Some(&row), &today, &month, 99, None);
        assert!(none.contains("You haven't solved one today") && !none.contains("This month"), "{}", none);
        assert!(mine_text(None, &[], &[], 1, None).contains("No puzzle is up right now"));
    }

    #[test]
    fn the_button_hands_over_a_link_and_says_where_they_are_in_the_line() {
        let conn = memory();
        let row = super::super::puzzle_store::tests::put(&conn, "WqnOB", 1_000);
        let fresh = solve_text(&row, None, Some("https://panel/puzzle/1/abc"));
        assert!(fresh.contains("🧩 **Puzzle #1** · White to play · **1 move**"), "{}", fresh);
        assert!(fresh.contains("Tap a piece, then tap where it goes"), "{}", fresh);
        assert!(fresh.contains("[Open your board](https://panel/puzzle/1/abc)"), "{}", fresh);
        assert!(fresh.contains("yours alone"), "{}", fresh);
        assert!(fresh.contains("An engine would find this in a blink"), "{}", fresh);
        let midway = solve_text(&row, Some(&store::Player { user: 5, done: 1, ..Default::default() }), Some("x"));
        assert!(midway.contains("**1 move** into the line"), "{}", midway);
        let done = solve_text(&row, Some(&store::Player { user: 5, done: 2, solved: true, ..Default::default() }), Some("x"));
        assert!(done.contains("already solved this one"), "{}", done);
        // No panel address: said plainly rather than a dead link.
        assert!(solve_text(&row, None, None).contains("needs the panel's web address"));
    }

    #[test]
    fn a_solved_line_reads_the_same_whether_or_not_a_house_point_was_paid() {
        let base = Attempt { ok: true, solved: true, san: "Bxa2".into(), worth: 2, tally: 6, ..Default::default() };
        // House points off — how it ships. The first solver is still first.
        let first_off = solved_words(&Attempt { first: true, points: 0, ..base.clone() });
        assert_eq!(first_off, "🎉 **Bxa2** — solved it! You were the **first** to crack this one. **+2 puzzle points** · 6 puzzle points today.");
        assert!(!first_off.contains("house point") && !first_off.contains("no points"));
        // House points on: the point is named beside the puzzle points.
        let first_on = solved_words(&Attempt { first: true, points: 1, ..base.clone() });
        assert!(first_on.contains("**+2 puzzle points** · **+1 house point** for being first · 6 puzzle points today."), "{}", first_on);
        // A later solver scores puzzle points and is not called first.
        let later = solved_words(&Attempt { first: false, points: 0, ..base });
        assert!(!later.contains("first"), "{}", later);
        assert!(later.contains("**+2 puzzle points** · 6 puzzle points today."), "{}", later);
    }
}
