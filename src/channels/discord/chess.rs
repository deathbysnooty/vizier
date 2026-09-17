//! Chess in its own channel (`VIZIER_CHESS_CHANNEL`).
//!
//! Members can't type there: everything is the bot's cards, buttons and
//! pop-ups. `/chess @someone` posts a challenge; when it is accepted the
//! colours are drawn, a game is made and its card goes up with the board drawn
//! on it. A move comes either from the private board page - a link only that
//! player ever sees - or from the ✍️ Type move pop-up, and either way the same
//! code reads and checks it.
//!
//! One card is the channel's ACTIVE card and is always its last message: the
//! newest running game, or, when nothing is running, the idle line. Anything
//! that lands below it moves it down a few seconds later, the same way the
//! Name Place Animal Thing card works. A game that is not the newest keeps its
//! card where it is; making a move brings it back to the bottom, because its
//! board has changed anyway and a new picture has to be posted.
//!
//! CHESS POINTS go to the winner, or to both on a draw. They are the game's own
//! score and no longer move the House Cup at all: there is no daily limit on
//! them, no house needs to be behind them, and mods and Muggles keep theirs like
//! everyone else. `/chesstop` is the board. A pair is scored for one game a day,
//! and a game given up in the first few moves scores nothing, so two friends
//! can't farm the board by resigning at move two.
//!
//! One task drives the clock ([`run`]): the buttons and the web page only write
//! to the database, and the task posts, edits and pays, so two presses can
//! never make two result cards.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{Value, json};
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, ComponentInteraction,
    ComponentInteractionDataKind, Context, CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton,
    CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu, CreateSelectMenuKind, EditAttachments,
    EditMessage, GetMessages, Message, MessageId, ModalInteraction, UserId,
};
use shakmaty::{Color, Position};

use super::chess_board::{self, View};
use super::chess_rules::{self as rules, Ending, MoveError, Replay, Result_, TimeControl};
use super::chess_store::{self as store, ChallengeStatus, Game};
use super::control;
use super::points::Source;
use super::rules_text::ChessRules;

/// How often the game task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// How long the active card waits after the last message below it before moving down.
pub const MOVE_DELAY_MS: i64 = 3_000;
/// How long a challenge stands before it lapses.
pub const CHALLENGE_SECS: i64 = 30 * 60;
/// A casual game nudges the player to move when this little is left.
pub const NUDGE_SECS: i64 = 2 * 3600;
/// How often the active card is redrawn so its clock moves: live games tick fast.
const LIVE_REFRESH_SECS: i64 = 30;
const CASUAL_REFRESH_SECS: i64 = 300;
/// How often the bot writes down that it is still here. A restart reads the
/// last one to work out how long the clocks ran unwatched.
const HEARTBEAT_SECS: i64 = 30;
/// Downtime shorter than this is not worth giving back.
pub const MIN_DOWNTIME_SECS: i64 = 10;
/// Downtime longer than this is not believed: a machine that was off for days
/// should not hand every game most of a week.
pub const MAX_DOWNTIME_SECS: i64 = 6 * 3600;
/// The meta row a deploy script reads: when the earliest live game needs a move.
pub const LIVE_UNTIL_KEY: &str = "chess_live_move_until";
/// Characters the move list may use on a card.
const MOVE_LIST_WIDTH: usize = 52;
const MOVE_LIST_LINES: usize = 6;

const COLOUR: u32 = 0xB58863;
const CHALLENGE_COLOUR: u32 = 0xD9A441;
const RESULT_COLOUR: u32 = 0xE8B923;
const IDLE_COLOUR: u32 = 0x4E5058;

const BOARD_FILE: &str = "board.png";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_CHESS", false)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_CHESS_CHANNEL").filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

/// The channel to play in right now, if chess is on.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on())
}

fn casual_hours() -> i64 {
    control::number("VIZIER_CHESS_CASUAL_HOURS", 12).clamp(1, 72) as i64
}

fn live_seconds() -> i64 {
    control::number("VIZIER_CHESS_LIVE_SECONDS", 180).clamp(30, 3600) as i64
}

fn max_games() -> i64 {
    control::number("VIZIER_CHESS_MAX_GAMES", 3).clamp(1, 20) as i64
}

/// How many games the whole channel may have running at once.
fn max_active() -> i64 {
    control::number("VIZIER_CHESS_MAX_ACTIVE", 8).clamp(1, 50) as i64
}

/// How long a finished game's replay stays readable. Zero turns replays off.
fn replay_days() -> i64 {
    control::number("VIZIER_CHESS_REPLAY_DAYS", 30).min(365) as i64
}

/// How long after the bot wakes up before anyone can lose on time. A player
/// must have a real chance to move, not be flagged the moment the bot returns.
fn grace_secs() -> i64 {
    control::number("VIZIER_CHESS_GRACE_SECONDS", 60).min(3600) as i64
}

fn min_plies() -> i64 {
    control::number("VIZIER_CHESS_MIN_MOVES", 10).min(200) as i64
}

fn win_points() -> i64 {
    control::number("VIZIER_POINTS_CHESS_WIN", 4).min(100) as i64
}

fn draw_points() -> i64 {
    control::number("VIZIER_POINTS_CHESS_DRAW", 1).min(100) as i64
}

/// How long a side has per move under a time control, right now.
pub fn per_move_secs(time: TimeControl) -> i64 {
    match time {
        TimeControl::Casual => casual_hours() * 3600,
        TimeControl::Live => live_seconds(),
    }
}

/// Every setting the guide and the help card mention, as they are now.
pub fn chess_rules() -> ChessRules {
    ChessRules {
        channel: live_channel(),
        casual_hours: casual_hours(),
        live_seconds: live_seconds(),
        max_games: max_games(),
        max_active: max_active(),
        replay_days: replay_days(),
        min_plies: min_plies(),
        win: win_points(),
        draw: draw_points(),
    }
}

/// The guide's view of chess, read from the settings.
pub fn live_rules() -> ChessRules {
    chess_rules()
}

// --- being away ---------------------------------------------------------------------------

/// How much time a restart owes the clocks: the gap since the bot last wrote
/// that it was here. A blink is not worth giving back, and a gap of days is not
/// believed at all - the machine was off, not the game - so both ends are
/// ignored and the caller says so in the log.
pub fn forgiveness(last_seen: Option<i64>, now: i64, min: i64, max: i64) -> Option<i64> {
    let gap = now - last_seen?;
    (gap >= min && gap <= max).then_some(gap)
}

/// Whether a clock may be judged yet: not within `grace` of the bot waking up.
pub fn may_flag(now: i64, woke_at: i64, grace: i64) -> bool {
    now - woke_at >= grace
}

/// When the bot came up, so the grace window can be worked out.
static WOKE_AT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

// --- shared state ------------------------------------------------------------------------

/// What the channel's last message should be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bottom {
    /// Nothing is running.
    #[default]
    Idle,
    /// The newest running game's card.
    Game(i64),
}

#[derive(Default)]
struct Shared {
    /// (channel, message) of the active card, and what it shows.
    card: Option<(u64, u64)>,
    showing: Bottom,
    /// The newest message id seen in the channel.
    latest: u64,
    /// When a message last landed in the channel.
    last_seen_ms: i64,
    /// The active card was deleted by someone.
    card_gone: bool,
    /// When the active card was last redrawn, so its clock keeps moving.
    refreshed_at: i64,
    /// A game whose card the task should post or repost at once.
    dirty: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// Every message in the chess channel, the bot's own included.
pub fn note_message(_ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != channel_setting() {
        return;
    }
    // The chess PUZZLE lives in this channel too and keeps its own card at the
    // bottom. Without this the two would chase each other down the channel for
    // ever — chess moves below the puzzle card, the puzzle moves below that —
    // so the puzzle's card is the one message chess does not follow.
    if super::puzzle::owns_message(msg.id.get()) {
        return;
    }
    let mut s = SHARED.lock();
    s.latest = s.latest.max(msg.id.get());
    s.last_seen_ms = Utc::now().timestamp_millis();
}

/// A deleted active card is posted again.
pub fn on_delete(channel: ChannelId, id: MessageId) {
    if Some(channel.get()) != channel_setting() {
        return;
    }
    let mut s = SHARED.lock();
    if s.card.map(|(_, m)| m) == Some(id.get()) {
        s.card_gone = true;
    }
}

/// Whether the active card should move down now: something newer than it landed
/// in the channel (message ids grow with time) and the channel has been quiet
/// for `delay_ms` since. The card's own post is never newer than itself, so a
/// move can't set off another. This is the same rule the Name Place Animal
/// Thing card uses, and it is repeated here rather than shared so one game's
/// card can never move the other's.
pub fn move_due(card: Option<u64>, latest: u64, last_seen_ms: i64, now_ms: i64, delay_ms: i64) -> bool {
    card.is_some_and(|c| latest > c) && now_ms - last_seen_ms >= delay_ms
}

// --- small words -------------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// A house crest for a member, or a hat for someone in no house.
pub fn crest(user: u64) -> &'static str {
    super::house::house_of(user).map(|h| h.crest).unwrap_or("🧙")
}

/// The house a member counts as being in for the points. Empty for a Muggle or
/// anyone the Sorting Hat hasn't reached, who earns nothing anywhere.
fn house_key(user: u64) -> String {
    if super::house::opted_out(user) {
        return String::new();
    }
    super::house::house_of(user).map(|h| h.key.to_string()).unwrap_or_default()
}

/// "⬜" for white and "⬛" for black.
fn side_square(white: bool) -> &'static str {
    if white { "⬜" } else { "⬛" }
}

/// The time control in a few words: "Casual · 12 h per move".
pub fn control_words(time: TimeControl, per_move: i64) -> String {
    match time {
        TimeControl::Casual => format!("Casual · {} per move", rules::clock_words(per_move)),
        TimeControl::Live => format!("Live · {} per move", rules::clock_words(per_move)),
    }
}

/// What a game is worth, in one line. Chess points, not house points: the House
/// Cup does not move for a game of chess, so which house either player is in
/// makes no difference to it and nothing is capped.
pub fn worth_words(win: i64, draw: i64) -> String {
    if win <= 0 && draw <= 0 {
        return "Played for the fun of it — this one scores nothing.".to_string();
    }
    format!("Chess points · winner **+{}** · draw **+{}** each · no daily limit · `/chesstop`", win, draw)
}

// --- the challenge card --------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn challenge_text(
    challenger: u64,
    opponent: u64,
    time: TimeControl,
    per_move: i64,
    win: i64,
    draw: i64,
    expires_at: i64,
) -> String {
    format!(
        "<@{}> {} challenges <@{}> {}\n🕰️ {}\n-# {}\n-# Offer expires <t:{}:R> — either of you can decline",
        challenger,
        crest(challenger),
        opponent,
        crest(opponent),
        control_words(time, per_move),
        worth_words(win, draw),
        expires_at
    )
}

fn challenge_embed(c: &store::Challenge) -> CreateEmbed {
    let time = TimeControl::from_key(&c.time_control);
    CreateEmbed::new()
        .title("♟️ Chess challenge")
        .description(challenge_text(
            c.challenger,
            c.opponent,
            time,
            per_move_secs(time),
            win_points(),
            draw_points(),
            c.expires_at,
        ))
        .colour(CHALLENGE_COLOUR)
}

fn challenge_rows(id: i64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("chessyes:{}", id)).label("✅ Accept").style(ButtonStyle::Success),
        CreateButton::new(format!("chessno:{}", id)).label("❌ Decline").style(ButtonStyle::Secondary),
        // Being challenged is where a beginner meets this game, and it is the
        // one moment they most want the rules. They can't type to ask for them.
        help_button(),
    ])]
}

/// How a closed challenge reads once its buttons are gone.
pub fn challenge_closed_text(c: &store::Challenge, game_id: Option<i64>) -> String {
    match c.status {
        ChallengeStatus::Accepted => match game_id {
            Some(id) => format!("✅ <@{}> accepted — Game #{} is on.", c.opponent, id),
            None => format!("✅ <@{}> accepted.", c.opponent),
        },
        ChallengeStatus::Declined => format!("❌ <@{}> declined the challenge.", c.opponent),
        ChallengeStatus::Expired => format!("⌛ <@{}> never answered — the challenge has lapsed.", c.opponent),
        ChallengeStatus::Open => challenge_text(
            c.challenger,
            c.opponent,
            TimeControl::from_key(&c.time_control),
            per_move_secs(TimeControl::from_key(&c.time_control)),
            win_points(),
            draw_points(),
            c.expires_at,
        ),
    }
}

// --- the game card --------------------------------------------------------------------------

/// A game card's title. When more than one game is going the count says so, so
/// nobody thinks the card at the bottom is the only game on the board.
pub fn game_title(game_id: i64, running: usize) -> String {
    if running > 1 {
        format!("♟️ Game #{} · {} games running", game_id, running)
    } else {
        format!("♟️ Game #{}", game_id)
    }
}

/// The line that says whose move it is and how long they have.
pub fn turn_line(game: &Game, secs_left: i64) -> String {
    let to_move = game.to_move();
    format!(
        "{} <@{}> {} to move · ⏱️ **{}** left",
        side_square(game.is_white(to_move)),
        to_move,
        crest(to_move),
        rules::clock_words(secs_left)
    )
}

/// The game card's whole description.
pub fn game_text(game: &Game, secs_left: i64, now: i64, win: i64, draw: i64) -> String {
    let time = TimeControl::from_key(&game.time_control);
    let mut text = format!(
        "{} <@{}> {} **vs** {} <@{}> {}\n{}\n-# {} · move {} · started {} ago\n-# {}",
        side_square(true),
        game.white,
        crest(game.white),
        side_square(false),
        game.black,
        crest(game.black),
        turn_line(game, secs_left),
        control_words(time, game.per_move_secs),
        game.moves.len() / 2 + 1,
        rules::span_words(now - game.started_at),
        worth_words(win, draw)
    );
    if game.given_back > 0 {
        text.push_str(&format!("\n-# ⏸️ {} given back after an update — nobody loses time to a restart.", rules::span_words(game.given_back)));
    }
    if let Some(who) = game.draw_offer {
        text.push_str(&format!("\n🤝 <@{}> has offered a draw.", who));
    }
    text.push_str(&format!("\n\n```\n{}\n```", rules::move_list(&game.moves, MOVE_LIST_WIDTH, MOVE_LIST_LINES)));
    text
}

fn game_embed(game: &Game, running: usize, now: i64) -> CreateEmbed {
    let left = rules::seconds_left(game.last_move_ts, game.per_move_secs, now);
    CreateEmbed::new()
        .title(game_title(game.id, running))
        .description(game_text(game, left, now, win_points(), draw_points()))
        .colour(COLOUR)
        .image(format!("attachment://{}", BOARD_FILE))
        .footer(CreateEmbedFooter::new("Open the board for a tap-to-move page, or type your move"))
}

/// How many games are running right now, for the cards' own headline.
fn running_count() -> usize {
    with_db(|conn| store::running_games(conn).len()).unwrap_or(0)
}

/// The button that opens the pop-up picker. Nobody can type in the chess
/// channel, so this is how a challenge starts without a slash command: it is on
/// the idle card AND on every game card, so somebody watching a game can start
/// one of their own without going anywhere.
pub fn challenge_button() -> CreateButton {
    CreateButton::new(PICK_ID).label("⚔️ Challenge someone").style(ButtonStyle::Secondary)
}

/// The button that explains the game. Members can't type in the chess channel -
/// the message box itself is denied them, so `/chesshelp` cannot be run there at
/// all - which is why the rules have to be a button like everything else.
pub fn help_button() -> CreateButton {
    CreateButton::new(HELP_ID).label("❓ How to play").style(ButtonStyle::Secondary)
}

fn game_rows(game_id: i64) -> Vec<CreateActionRow> {
    let mut buttons = vec![
        CreateButton::new(format!("chessopen:{}", game_id)).label("♟️ Open board").style(ButtonStyle::Success),
        CreateButton::new(format!("chesstype:{}", game_id)).label("✍️ Type move").style(ButtonStyle::Primary),
        CreateButton::new(format!("chessresign:{}", game_id)).label("🏳️ Resign").style(ButtonStyle::Secondary),
        CreateButton::new(format!("chessdraw:{}", game_id)).label("🤝 Offer draw").style(ButtonStyle::Secondary),
    ];
    // Anyone may watch, so this one is a plain link rather than a button only
    // the two players' presses reach.
    if let Some(link) = watch_link(game_id) {
        buttons.push(CreateButton::new_link(link).label("👀 Watch"));
    }
    // A row of its own: the first is full, and this one is for everybody else
    // in the channel rather than the two playing.
    vec![CreateActionRow::Buttons(buttons), CreateActionRow::Buttons(vec![challenge_button(), help_button()])]
}

/// What the board picture for a game should show.
pub fn view_of(game: &Game, flipped: bool) -> View {
    let replay = Replay::from_sans(&game.moves).unwrap_or_default();
    let last = rules::last_move_squares(&game.moves).unwrap_or_default();
    let check = if replay.position.is_check() {
        replay
            .position
            .board()
            .king_of(replay.position.turn())
            .map(|sq| sq.to_string())
            .unwrap_or_default()
    } else {
        String::new()
    };
    View { placement: game.fen.clone(), last, check, flipped }
}

async fn board_attachment(game: &Game, flipped: bool) -> Option<CreateAttachment> {
    let view = view_of(game, flipped);
    let png = tokio::task::spawn_blocking(move || chess_board::board_png(&view)).await.ok().flatten()?;
    Some(CreateAttachment::bytes(png.to_vec(), BOARD_FILE))
}

// --- the idle card -----------------------------------------------------------------------------

pub fn idle_text(rules: &ChessRules) -> String {
    let mut text = String::from(
        "♟️ **No game running** — press **⚔️ Challenge someone** below, or use `/chess @member`.\n\
         Pick your pace: **casual** for a move every few hours, or **live** to play it out now.",
    );
    if rules.win > 0 {
        text.push_str(&format!(
            "\n-# Chess points: winner **+{}** · draw **+{}** each, with no daily limit — `/chesstop` is the board",
            rules.win, rules.draw
        ));
    }
    text.push_str("\n-# Press **❓ How to play** for the rules.");
    text
}

fn idle_embed() -> CreateEmbed {
    CreateEmbed::new().title("♟️ Chess").description(idle_text(&live_rules())).colour(IDLE_COLOUR)
}

/// The idle card's buttons: start a game, or find out how. Nothing else is
/// reachable in that channel, so these two are all there is.
pub fn idle_rows() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![challenge_button(), help_button()])]
}

fn idle_message() -> CreateMessage {
    CreateMessage::new().embed(idle_embed()).components(idle_rows()).allowed_mentions(CreateAllowedMentions::new())
}

// --- the result card ---------------------------------------------------------------------------

/// How a finished game is announced, in three or four words.
pub fn result_headline(result: &str) -> &'static str {
    match result {
        "checkmate" => "🏁 Checkmate",
        "resign" => "🏳️ Resignation",
        "timeout" => "⌛ Out of time",
        "draw" => "🤝 Draw agreed",
        "stalemate" => "🤝 Stalemate",
        "threefold" => "🤝 Draw by repetition",
        "fifty" => "🤝 Draw by the fifty-move rule",
        "material" => "🤝 Draw — neither side can mate",
        "cancelled" => "🛑 Cancelled by a mod",
        _ => "🏁 Game over",
    }
}

/// Whether a finished game's replay is still readable: `days` of 0 turns
/// replays off altogether, and after that many days a game ages out.
pub fn replay_open(finished_at: Option<i64>, now: i64, days: i64) -> bool {
    if days <= 0 {
        return false;
    }
    match finished_at {
        None => true,
        Some(at) => now - at <= days * 86_400,
    }
}

/// How long a finished game's replay stays readable, as the settings have it.
pub fn replay_window_days() -> i64 {
    replay_days()
}

/// What a result card says. `scored` is the CHESS points the game was worth to
/// white and to black - the game's own score, with no limit over it and nothing
/// to do with the House Cup, which chess no longer moves at all.
#[allow(clippy::too_many_arguments)]
pub fn result_text(
    game: &Game,
    result: &str,
    winner: Option<u64>,
    plies: usize,
    lasted: i64,
    scored: (i64, i64),
    why_nothing: &str,
    replay: Option<&str>,
) -> (String, String) {
    let title = format!("{} · Game #{}", result_headline(result), game.id);
    let moves = plural((plies as i64 + 1) / 2, "move", "moves");
    let mut body = match (winner, result) {
        (Some(who), "timeout") => {
            format!("<@{}> {} beat <@{}> {} on time, after **{}**", who, crest(who), game.other(who), crest(game.other(who)), moves)
        }
        (Some(who), "resign") => format!(
            "<@{}> {} won — <@{}> {} resigned after **{}**",
            who,
            crest(who),
            game.other(who),
            crest(game.other(who)),
            moves
        ),
        (Some(who), _) => {
            format!("<@{}> {} beat <@{}> {} in **{}**", who, crest(who), game.other(who), crest(game.other(who)), moves)
        }
        (None, "cancelled") => format!("<@{}> and <@{}> — the game was stopped after **{}**", game.white, game.black, moves),
        (None, _) => format!("<@{}> {} and <@{}> {} drew after **{}**", game.white, crest(game.white), game.black, crest(game.black), moves),
    };
    let awarded: Vec<String> = [(game.white, scored.0), (game.black, scored.1)]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(user, n)| format!("<@{}> **+{}**", user, n))
        .collect();
    if awarded.is_empty() {
        let why = if why_nothing.is_empty() { "no chess points this time".to_string() } else { format!("no chess points: {}", why_nothing) };
        body.push_str(&format!("\n♟️ {}", why));
    } else {
        body.push_str(&format!("\n♟️ Chess points: {} · `/chesstop` for the board", awarded.join(" · ")));
    }
    let mut tail = format!("The game lasted {}", rules::span_words(lasted));
    if let Some(link) = replay {
        tail.push_str(&format!(" · [replay the moves]({})", link));
    }
    body.push_str(&format!("\n-# {}", tail));
    (title, body)
}

// --- Discord helpers ----------------------------------------------------------------------------

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

async fn delete(ctx: &Context, channel: u64, message: u64) {
    if let Err(err) = call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        tracing::debug!("chess: message {} not deleted: {}", message, err);
    }
}

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

fn whisper_command(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
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

fn with_db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> Option<T> {
    let db = store::db()?;
    let conn = db.lock();
    Some(f(&conn))
}

fn meta_set(key: &str, value: &str) {
    let _ = with_db(|conn| store::meta_set(conn, key, value));
}

fn meta_get(key: &str) -> Option<String> {
    with_db(|conn| store::meta_get(conn, key)).flatten().filter(|v| !v.is_empty())
}

// --- the active card ------------------------------------------------------------------------------

/// Which of the running games should hold the bottom of the channel: the one
/// that moved most recently, or - between two that moved in the same second -
/// the one that started later. With none running, the idle line.
pub fn newest_of(games: &[Game]) -> Bottom {
    match games.iter().max_by_key(|g| (g.last_move_ts, g.id)) {
        Some(game) => Bottom::Game(game.id),
        None => Bottom::Idle,
    }
}

/// What should be at the bottom of the channel now.
fn bottom_now() -> Bottom {
    with_db(|conn| newest_of(&store::running_games(conn))).unwrap_or(Bottom::Idle)
}

/// Builds the message for whatever should be at the bottom.
async fn bottom_message(bottom: Bottom, now: i64) -> Option<CreateMessage> {
    match bottom {
        Bottom::Idle => Some(idle_message()),
        Bottom::Game(id) => {
            let game = with_db(|conn| store::get_game(conn, id)).flatten()?;
            let mut message =
                CreateMessage::new().embed(game_embed(&game, running_count(), now)).components(game_rows(game.id)).allowed_mentions(CreateAllowedMentions::new());
            if let Some(file) = board_attachment(&game, false).await {
                message = message.add_file(file);
            }
            Some(message)
        }
    }
}

/// Posts the active card afresh and takes the old one away - both the card that
/// was last in the channel and, when the new card is a game's, whatever that
/// game's own older card was, so a game coming back to the bottom never leaves
/// a second board behind.
async fn place_card(ctx: &Context, channel: u64, bottom: Bottom, message: CreateMessage) -> Option<u64> {
    let theirs = match bottom {
        Bottom::Game(game) => with_db(|conn| store::get_game(conn, game)).flatten().and_then(|g| g.message.map(|m| (g.channel, m))),
        Bottom::Idle => None,
    };
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let id = posted.id.get();
            let old = {
                let mut s = SHARED.lock();
                s.latest = s.latest.max(id);
                s.showing = bottom;
                s.refreshed_at = Utc::now().timestamp();
                s.card.replace((channel, id))
            };
            meta_set("card", &format!("{}:{}", channel, id));
            if let Bottom::Game(game) = bottom {
                let _ = with_db(|conn| store::set_message(conn, game, Some(id)));
            }
            let mut going: Vec<(u64, u64)> = Vec::new();
            for slot in [old, theirs].into_iter().flatten() {
                if slot.1 != id && !going.contains(&slot) {
                    going.push(slot);
                }
            }
            for (c, m) in going {
                delete(ctx, c, m).await;
            }
            Some(id)
        }
        Err(err) => {
            tracing::warn!("chess: card not posted in {}: {}", channel, err);
            None
        }
    }
}

/// Takes the active card away without a new one.
async fn drop_card(ctx: &Context) {
    let card = SHARED.lock().card.take();
    meta_set("card", "");
    if let Some((c, m)) = card {
        delete(ctx, c, m).await;
    }
}

/// Redraws the active card in place, for a clock that has moved on.
async fn refresh_card(ctx: &Context, now: i64) {
    let (card, showing) = {
        let s = SHARED.lock();
        (s.card, s.showing)
    };
    let (Some((channel, message)), Bottom::Game(id)) = (card, showing) else { return };
    let Some(game) = with_db(|conn| store::get_game(conn, id)).flatten().filter(|g| g.running()) else { return };
    let mut edit = EditMessage::new().embed(game_embed(&game, running_count(), now)).components(game_rows(game.id)).allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = board_attachment(&game, false).await {
        edit = edit.attachments(EditAttachments::new().add(file));
    }
    match call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        Ok(_) => SHARED.lock().refreshed_at = now,
        Err(err) => {
            if is_gone(&err) {
                SHARED.lock().card_gone = true;
            }
            tracing::warn!("chess: card for game {} not redrawn: {}", id, err);
        }
    }
}

/// Redraws a game's card where it stands, for a game that is not at the bottom.
async fn edit_game_card(ctx: &Context, game: &Game, now: i64) {
    let Some(message) = game.message else { return };
    let mut edit = EditMessage::new().embed(game_embed(game, running_count(), now)).components(game_rows(game.id)).allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = board_attachment(game, false).await {
        edit = edit.attachments(EditAttachments::new().add(file));
    }
    if let Err(err) = call(ChannelId::new(game.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("chess: card for game {} not edited: {}", game.id, err);
    }
}

/// Everything a change to a game needs: the card back at the bottom, drawn afresh.
fn nudge_task() {
    SHARED.lock().dirty = true;
}

// --- challenges --------------------------------------------------------------------------------

/// Why a challenge can't be sent now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Off,
    Self_,
    Bot,
    AlreadyAsked,
    TooManyYours(i64),
    TooManyTheirs(i64),
    /// The channel itself is full, whoever is playing.
    ChannelFull(i64),
}

impl Refusal {
    pub fn words(&self, them: u64) -> String {
        match self {
            Refusal::Off => "Chess is switched off right now.".to_string(),
            Refusal::Self_ => "You can't play yourself. Pick someone else.".to_string(),
            Refusal::Bot => "Pick a human, not a bot.".to_string(),
            Refusal::AlreadyAsked => format!("You and <@{}> already have a challenge waiting. Answer that one first.", them),
            Refusal::TooManyYours(max) => {
                format!("You already have **{}** going. Finish one before starting another.", plural(*max, "game", "games"))
            }
            Refusal::TooManyTheirs(max) => {
                format!("<@{}> already has **{}** going. Give them a moment.", them, plural(*max, "game", "games"))
            }
            Refusal::ChannelFull(max) => format!(
                "The board is full: **{}** are already running here. Start one when somebody finishes.",
                plural(*max, "game", "games")
            ),
        }
    }
}

/// Whether a challenge may be sent, from what the database says. The friendly
/// refusals are tried in the order that tells the challenger the most useful
/// thing first: what they did wrong, then their own limit, then the other
/// player's, then the channel's.
#[allow(clippy::too_many_arguments)]
pub fn may_challenge(
    me: u64,
    them: u64,
    them_is_bot: bool,
    mine: usize,
    theirs: usize,
    running: usize,
    already: bool,
    max: i64,
    max_active: i64,
) -> Result<(), Refusal> {
    if me == them {
        return Err(Refusal::Self_);
    }
    if them_is_bot {
        return Err(Refusal::Bot);
    }
    if already {
        return Err(Refusal::AlreadyAsked);
    }
    if mine as i64 >= max {
        return Err(Refusal::TooManyYours(max));
    }
    if theirs as i64 >= max {
        return Err(Refusal::TooManyTheirs(max));
    }
    if running as i64 >= max_active {
        return Err(Refusal::ChannelFull(max_active));
    }
    Ok(())
}

// --- challenging with a button ----------------------------------------------------------------

/// The button that opens the picker, and the prefixes its own parts carry.
pub const PICK_ID: &str = "chesspick";
/// The button that opens the rules, on the idle card and on every game card.
pub const HELP_ID: &str = "chesshelp";
pub const PICK_WHO: &str = "chesswho:";
/// The pace buttons carry the pace in the id itself: `chesstime:<draft>:live`.
pub const PICK_TIME: &str = "chesstime:";
pub const PICK_SEND: &str = "chesssend:";
pub const PICK_DROP: &str = "chesscancel:";

/// How long a half-filled picker is remembered. It only holds who was chosen
/// and at what pace, and it is thrown away the moment the challenge goes out.
pub const DRAFT_SECS: i64 = 15 * 60;
/// Drafts kept at once, so a jammed button cannot fill the machine's memory.
pub const DRAFT_MAX: usize = 500;
const DRAFT_ALPHABET: &[u8] = b"bcdfghjkmnpqrstvwxyz23456789";
pub const DRAFT_ID_LEN: usize = 8;

/// A challenge half made: who is making it, who they have picked so far, and
/// the pace. It lives in memory only - there is nothing here worth a database
/// row, and a restart losing one costs a press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draft {
    pub by: u64,
    pub target: Option<u64>,
    pub time: TimeControl,
    pub made_at: i64,
}

static DRAFTS: LazyLock<Mutex<HashMap<String, Draft>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Forgets drafts older than `life`, and the oldest of them if somebody has
/// been leaning on the button. Returns how many were thrown away.
pub fn sweep_drafts(drafts: &mut HashMap<String, Draft>, now: i64, life: i64, max: usize) -> usize {
    let before = drafts.len();
    drafts.retain(|_, d| now - d.made_at < life);
    while drafts.len() > max {
        let Some(oldest) = drafts.iter().min_by_key(|(id, d)| (d.made_at, (*id).clone())).map(|(id, _)| id.clone()) else {
            break;
        };
        drafts.remove(&oldest);
    }
    before - drafts.len()
}

/// A short random name for a draft, from the caller's own random numbers. No
/// vowels, so no word can turn up in one by accident.
pub fn draft_id(mut roll: impl FnMut() -> f64) -> String {
    (0..DRAFT_ID_LEN)
        .map(|_| {
            let n = (roll().clamp(0.0, 0.999_999) * DRAFT_ALPHABET.len() as f64) as usize;
            DRAFT_ALPHABET[n.min(DRAFT_ALPHABET.len() - 1)] as char
        })
        .collect()
}

/// Starts a draft and returns its name.
fn open_draft(by: u64, now: i64) -> String {
    let id = draft_id(|| rand::random::<f64>());
    let mut drafts = DRAFTS.lock();
    sweep_drafts(&mut drafts, now, DRAFT_SECS, DRAFT_MAX);
    drafts.insert(id.clone(), Draft { by, target: None, time: TimeControl::Casual, made_at: now });
    id
}

/// A draft, if it is still there and belongs to the person pressing.
fn my_draft(id: &str, by: u64, now: i64) -> Option<Draft> {
    let mut drafts = DRAFTS.lock();
    sweep_drafts(&mut drafts, now, DRAFT_SECS, DRAFT_MAX);
    drafts.get(id).copied().filter(|d| d.by == by)
}

/// Changes a draft in place; `None` when it has been forgotten or is not theirs.
fn edit_draft(id: &str, by: u64, now: i64, change: impl FnOnce(&mut Draft)) -> Option<Draft> {
    let mut drafts = DRAFTS.lock();
    sweep_drafts(&mut drafts, now, DRAFT_SECS, DRAFT_MAX);
    let draft = drafts.get_mut(id).filter(|d| d.by == by)?;
    change(draft);
    Some(*draft)
}

const DRAFT_GONE: &str = "That picker has timed out. Press ⚔️ Challenge someone again.";

/// What the picker says while it is being filled in. `problem` is the reason
/// the chosen member can't be challenged, and `note` something worth knowing
/// that is not a refusal - both are worked out before this is called, so the
/// words can be read on their own.
pub fn picker_text(chosen: Option<u64>, time: TimeControl, per_move: i64, problem: Option<&str>, note: Option<&str>) -> String {
    let mut text = String::from("⚔️ **Challenge someone to chess**\nPick a member, choose the pace, then press **Send challenge**.\n");
    match chosen {
        None => text.push_str("\n👤 Nobody picked yet."),
        Some(them) => text.push_str(&format!("\n👤 <@{}> {}", them, crest(them))),
    }
    text.push_str(&format!("\n🕰️ {}", control_words(time, per_move)));
    if let Some(problem) = problem {
        text.push_str(&format!("\n\n🚫 {}", problem));
    } else if let Some(note) = note {
        text.push_str(&format!("\n\n{}", note));
    }
    text.push_str("\n-# Only you can see this, and it forgets itself after 15 minutes.");
    text
}

/// Something worth telling the challenger that is not a reason to refuse.
///
/// There is nothing left to warn about: chess scores CHESS points, which owe
/// nothing to the House Cup, so a game inside one house and a game with someone
/// who has no house are worth exactly what any other game is. The picker keeps
/// the slot because a future reason to warn would go here.
pub fn picker_note(_them: u64, _my_house: &str, _their_house: &str) -> Option<String> {
    None
}

/// The picker's three rows: who, how fast, and send. Send stays dead until a
/// member has been chosen and nothing is in the way.
pub fn picker_rows(id: &str, chosen: Option<u64>, time: TimeControl, ready: bool, casual: i64, live: i64) -> Vec<CreateActionRow> {
    let who = CreateSelectMenu::new(
        format!("{}{}", PICK_WHO, id),
        CreateSelectMenuKind::User { default_users: chosen.map(|u| vec![UserId::new(u)]) },
    )
    .placeholder("Who do you want to play?")
    .min_values(1)
    .max_values(1);
    // The pace is two buttons rather than a menu: a menu is one more thing to
    // open, and a button says which one is chosen just by being lit.
    let pace = |what: TimeControl, secs: i64, label: &str| {
        CreateButton::new(format!("{}{}:{}", PICK_TIME, id, what.key()))
            .label(format!("{} {} · {}", label, what.label(), rules::clock_words(secs)))
            .style(if time == what { ButtonStyle::Primary } else { ButtonStyle::Secondary })
    };
    vec![
        CreateActionRow::SelectMenu(who),
        CreateActionRow::Buttons(vec![
            pace(TimeControl::Casual, casual, "🕰️"),
            pace(TimeControl::Live, live, "⚡"),
        ]),
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{}{}", PICK_SEND, id))
                .label("⚔️ Send challenge")
                .style(ButtonStyle::Success)
                .disabled(!ready),
            CreateButton::new(format!("{}{}", PICK_DROP, id)).label("Cancel").style(ButtonStyle::Secondary),
        ]),
    ]
}

/// Whether the member a picker has landed on can be challenged, and what to say
/// if not. The same checks `/chess` runs, so neither way in is easier than the
/// other.
fn picker_problem(ctx: &Context, me: u64, them: u64, them_is_bot: bool) -> Option<String> {
    if active_channel(ctx).is_none() {
        return Some(Refusal::Off.words(them));
    }
    let now = Utc::now().timestamp();
    let checks = with_db(|conn| {
        (
            store::games_of(conn, me).len(),
            store::games_of(conn, them).len(),
            store::running_games(conn).len(),
            store::open_between(conn, me, them, now).is_some(),
        )
    });
    let Some((mine, theirs, running, already)) = checks else {
        return Some("Chess isn't ready yet — try again in a moment.".to_string());
    };
    may_challenge(me, them, them_is_bot, mine, theirs, running, already, max_games(), max_active())
        .err()
        .map(|refusal| refusal.words(them))
}

/// The whole picker as a fresh ephemeral reply or an update to the one already
/// on screen.
fn picker_message(ctx: &Context, id: &str, draft: &Draft) -> (String, Vec<CreateActionRow>) {
    let problem = draft.target.and_then(|them| picker_problem(ctx, draft.by, them, is_bot(ctx, them)));
    let note = draft
        .target
        .filter(|_| problem.is_none())
        .and_then(|them| picker_note(them, &house_key(draft.by), &house_key(them)));
    let ready = draft.target.is_some() && problem.is_none();
    let text = picker_text(draft.target, draft.time, per_move_secs(draft.time), problem.as_deref(), note.as_deref());
    (text, picker_rows(id, draft.target, draft.time, ready, per_move_secs(TimeControl::Casual), per_move_secs(TimeControl::Live)))
}

fn is_bot(ctx: &Context, user: u64) -> bool {
    ctx.cache.user(UserId::new(user)).map(|u| u.bot).unwrap_or(false)
}

async fn show_picker(ctx: &Context, component: &ComponentInteraction, id: &str, draft: &Draft, fresh: bool) {
    let (text, rows) = picker_message(ctx, id, draft);
    let message = CreateInteractionResponseMessage::new()
        .content(text)
        .components(rows)
        .ephemeral(true)
        .allowed_mentions(CreateAllowedMentions::new());
    let response =
        if fresh { CreateInteractionResponse::Message(message) } else { CreateInteractionResponse::UpdateMessage(message) };
    if let Err(err) = component.create_response(&ctx.http, response).await {
        tracing::warn!("chess: the challenge picker wasn't shown to {}: {}", component.user.id, err);
    }
}

/// ⚔️ Challenge someone, from the idle card or any game card.
async fn start_picker(ctx: &Context, component: &ComponentInteraction) {
    if active_channel(ctx).is_none() {
        return whisper(ctx, component, Refusal::Off.words(0)).await;
    }
    let now = Utc::now().timestamp();
    let id = open_draft(component.user.id.get(), now);
    let draft = Draft { by: component.user.id.get(), target: None, time: TimeControl::Casual, made_at: now };
    show_picker(ctx, component, &id, &draft, true).await;
}

/// The member chosen in the user menu.
fn picked_user(component: &ComponentInteraction) -> Option<u64> {
    match &component.data.kind {
        ComponentInteractionDataKind::UserSelect { values } => values.first().map(|u| u.get()),
        _ => None,
    }
}

/// A pace button's id, split into the draft it belongs to and the pace it sets.
pub fn pace_pressed(custom_id: &str) -> Option<(&str, TimeControl)> {
    let rest = custom_id.strip_prefix(PICK_TIME)?;
    let (draft, pace) = rest.rsplit_once(':')?;
    (!draft.is_empty() && matches!(pace, "casual" | "live")).then(|| (draft, TimeControl::from_key(pace)))
}

async fn picker_chose(ctx: &Context, component: &ComponentInteraction, id: &str) {
    let by = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(them) = picked_user(component) else { return };
    let Some(draft) = edit_draft(id, by, now, |d| d.target = Some(them)) else {
        return whisper(ctx, component, DRAFT_GONE).await;
    };
    show_picker(ctx, component, id, &draft, false).await;
}

async fn picker_paced(ctx: &Context, component: &ComponentInteraction, id: &str, time: TimeControl) {
    let by = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(draft) = edit_draft(id, by, now, |d| d.time = time) else {
        return whisper(ctx, component, DRAFT_GONE).await;
    };
    show_picker(ctx, component, id, &draft, false).await;
}

/// ❓ How to play, from the idle card or any game card: the same words
/// `/chesshelp` gives, shown only to whoever pressed it.
async fn show_help(ctx: &Context, component: &ComponentInteraction) {
    let message = CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true);
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("chess: the rules weren't shown to {}: {}", component.user.id, err);
    }
}

async fn picker_cancelled(ctx: &Context, component: &ComponentInteraction, id: &str) {
    DRAFTS.lock().remove(id);
    let message = CreateInteractionResponseMessage::new()
        .content("Nothing sent.")
        .components(Vec::new())
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await;
}

async fn picker_sent(ctx: &Context, component: &ComponentInteraction, id: &str) {
    let by = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(draft) = my_draft(id, by, now) else {
        return whisper(ctx, component, DRAFT_GONE).await;
    };
    let Some(them) = draft.target else {
        return show_picker(ctx, component, id, &draft, false).await;
    };
    // Everything is checked again here: a picker can sit open while the other
    // player starts games of their own.
    let text = match send_challenge(ctx, by, them, is_bot(ctx, them), draft.time).await {
        Ok(sent) => {
            DRAFTS.lock().remove(id);
            sent
        }
        Err(why) => why,
    };
    let message = CreateInteractionResponseMessage::new()
        .content(text)
        .components(Vec::new())
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await;
}

// --- commands -------------------------------------------------------------------------------------

pub fn command() -> CreateCommand {
    CreateCommand::new("chess")
        .description("challenge a member to a game of chess - or, with nobody named, see your games")
        .add_option(CreateCommandOption::new(serenity::all::CommandOptionType::User, "member", "who to play"))
        .add_option(
            CreateCommandOption::new(serenity::all::CommandOptionType::String, "time", "how long each move may take")
                .add_string_choice("Casual - hours per move", "casual")
                .add_string_choice("Live - minutes per move", "live"),
        )
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("chesshelp").description("how chess works here: challenges, moves, points and time controls")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("chesstop")
        .description("the chess points board, today or this month")
        .add_option(
            CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "which days to count")
                .add_string_choice("Today", "today")
                .add_string_choice("This month", "month"),
        )
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("chessstop")
        .description("admin only: cancel a stuck chess game, with no chess points")
        .add_option(
            CreateCommandOption::new(serenity::all::CommandOptionType::Integer, "game", "the game's number")
                .required(true),
        )
}

/// Puts a challenge up in the chess channel and says what to tell the person
/// who asked for it. The ONE door both `/chess` and the ⚔️ Challenge someone
/// button go through, so the card that lands is the same card either way, and
/// neither way in is easier than the other.
async fn send_challenge(ctx: &Context, me: u64, them: u64, them_is_bot: bool, time: TimeControl) -> Result<String, String> {
    let Some(channel) = active_channel(ctx) else {
        return Err(Refusal::Off.words(them));
    };
    let now = Utc::now().timestamp();
    let checks = with_db(|conn| {
        (
            store::games_of(conn, me).len(),
            store::games_of(conn, them).len(),
            store::running_games(conn).len(),
            store::open_between(conn, me, them, now).is_some(),
        )
    });
    let Some((mine, theirs, running, already)) = checks else {
        return Err("Chess isn't ready yet — try again in a moment.".to_string());
    };
    if let Err(refusal) = may_challenge(me, them, them_is_bot, mine, theirs, running, already, max_games(), max_active()) {
        return Err(refusal.words(them));
    }
    let made = with_db(|conn| store::create_challenge(conn, channel, me, them, time.key(), now, now + CHALLENGE_SECS))
        .and_then(|r| r.ok());
    let Some(challenge) = made else {
        return Err("Couldn't put that challenge up. Try again.".to_string());
    };
    let message = CreateMessage::new()
        .embed(challenge_embed(&challenge))
        .components(challenge_rows(challenge.id))
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(them)]));
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let _ = with_db(|conn| store::set_challenge_message(conn, challenge.id, posted.id.get()));
            {
                let mut s = SHARED.lock();
                s.latest = s.latest.max(posted.id.get());
                s.dirty = true;
            }
            Ok(format!("♟️ Challenge sent in <#{}> — {}", channel, posted.link()))
        }
        Err(err) => {
            tracing::warn!("chess: challenge {} not posted: {}", challenge.id, err);
            let _ = with_db(|conn| store::close_challenge(conn, challenge.id, ChallengeStatus::Expired));
            Err("Couldn't post in the chess channel.".to_string())
        }
    }
}

/// `/chess` - everyone.
pub async fn command_handler(ctx: &Context, command: &CommandInteraction) {
    let me = command.user.id.get();
    let target = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    });
    let time = command
        .data
        .options
        .iter()
        .find(|o| o.name == "time")
        .and_then(|o| o.value.as_str())
        .map(TimeControl::from_key)
        .unwrap_or(TimeControl::Casual);
    if active_channel(ctx).is_none() {
        let _ = command.create_response(&ctx.http, whisper_command(Refusal::Off.words(0))).await;
        return;
    }
    let Some(them) = target else {
        let _ = command.create_response(&ctx.http, whisper_command(my_games_text(me, command.guild_id.map(|g| g.get())))).await;
        return;
    };
    let text = match send_challenge(ctx, me, them, is_bot(ctx, them), time).await {
        Ok(sent) => sent,
        Err(why) => why,
    };
    let _ = command.create_response(&ctx.http, whisper_command(text)).await;
}

/// One line of the ephemeral list `/chess` gives when nobody is named: who it
/// is against, which colour you are, whose move it is with the clock, and the
/// two links - your own board page and a jump to the game's card in the channel.
pub fn my_game_line(game: &Game, user: u64, secs_left: i64, board: Option<&str>, jump: Option<&str>) -> String {
    let them = game.other(user);
    let mine = game.to_move() == user;
    let whose = if mine {
        format!("**your move** · ⏱️ {} left", rules::clock_words(secs_left))
    } else {
        format!("waiting for <@{}> · ⏱️ {} left on their clock", them, rules::clock_words(secs_left))
    };
    let mut line = format!(
        "**#{}** vs <@{}> {} · {} you're {} · {}",
        game.id,
        them,
        crest(them),
        side_square(game.is_white(user)),
        if game.is_white(user) { "white" } else { "black" },
        whose
    );
    let links: Vec<String> = [board.map(|b| format!("[open your board]({})", b)), jump.map(|j| format!("[jump to the card]({})", j))]
        .into_iter()
        .flatten()
        .collect();
    if !links.is_empty() {
        line.push_str(&format!("\n-# {}", links.join(" · ")));
    }
    line
}

/// The whole ephemeral list, newest game last, with the asker's own chess
/// points under it.
pub fn my_games_text(user: u64, guild: Option<u64>) -> String {
    let (today, month) = boards_now();
    my_games_lines(user, guild, &today, &month)
}

/// The same list with the two boards already read, so a test can see it.
pub fn my_games_lines(user: u64, guild: Option<u64>, today: &[store::Tally], month: &[store::Tally]) -> String {
    let games = with_db(|conn| store::games_of(conn, user)).unwrap_or_default();
    let now = Utc::now().timestamp();
    let mut lines = match games.is_empty() {
        true => vec!["♟️ **Chess** — you have no games running. `/chess @someone` starts one.".to_string()],
        false => vec![format!("♟️ **Your games** ({} running)", games.len())],
    };
    for game in &games {
        let left = rules::seconds_left(game.last_move_ts, game.per_move_secs, now);
        let board = board_link(game.id, user);
        let jump = jump_link(guild, game);
        lines.push(my_game_line(game, user, left, board.as_deref(), jump.as_deref()));
    }
    if !games.is_empty() {
        lines.push("-# Only you can see these board links — each one works for your side of its own game alone.".to_string());
    }
    lines.push(match standing(today, user) {
        Some(mine) => format!("-# **You today:** {}", mine),
        None => "-# You haven't finished a game today. Chess points come from playing one out.".to_string(),
    });
    if let Some(mine) = standing(month, user) {
        lines.push(format!("-# **This month:** {}", mine));
    }
    lines.push("-# `/chesstop` for the board · `/chesshelp` explains the rest.".to_string());
    lines.join("\n")
}

// --- the chess points board -------------------------------------------------------------

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
/// they have and over how many games. Nothing at all when they haven't finished
/// a game in that stretch.
pub fn standing(rows: &[store::Tally], me: u64) -> Option<String> {
    let place = place_of(rows, me)?;
    let mine = rows.get(place - 1)?;
    Some(format!(
        "{} of {} · **{}** · {}",
        ordinal(place),
        rows.len(),
        plural(mine.points, "chess point", "chess points"),
        plural(mine.games, "game", "games")
    ))
}

/// How many names `/chesstop` lists.
const TOP_LIST: usize = 10;

/// What `/chesstop` says. `rows` is the whole board, already ranked; only the
/// first [`TOP_LIST`] are listed, and whoever asked gets their own line under
/// them when they didn't make it.
pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("♟️ **Chess points** · {}", period);
    if rows.is_empty() {
        text.push_str("\nNobody has finished a game yet. `/chess @someone` starts one.");
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
        text.push_str(&format!("\n{} <@{}> **{}** · {}{}", rank, t.user, t.points, plural(t.games, "game", "games"), you));
    }
    if place_of(rows, me).is_some_and(|p| p > TOP_LIST) {
        if let Some(mine) = standing(rows, me) {
            text.push_str(&format!("\n-# **You:** {}", mine));
        }
    }
    text.push_str("\n-# Chess points are the game's own score: a win or a draw at what it was worth, with no daily limit at all, and everyone has them.");
    text
}

/// The first moment of the India day a moment falls in, and the first moment of
/// the day after it.
pub fn day_bounds(now: i64) -> (i64, i64) {
    let start = super::points::ist_day_start(&super::points::ist_day(now)).unwrap_or(0);
    (start, start + 86_400)
}

/// The first moment of the India month a moment falls in, and the first moment
/// of the month after it.
pub fn month_bounds(now: i64) -> (i64, i64) {
    let day = super::points::ist_day(now);
    let (year, month) = (
        day.get(..4).and_then(|y| y.parse::<i32>().ok()).unwrap_or(1970),
        day.get(5..7).and_then(|m| m.parse::<u32>().ok()).unwrap_or(1),
    );
    let next = if month == 12 { format!("{}-01-01", year + 1) } else { format!("{}-{:02}-01", year, month + 1) };
    let start = super::points::ist_day_start(&format!("{}-{:02}-01", year, month)).unwrap_or(0);
    (start, super::points::ist_day_start(&next).unwrap_or(start))
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

/// Today's and this month's chess points, both boards ranked.
fn boards_now() -> (Vec<store::Tally>, Vec<store::Tally>) {
    let now = Utc::now().timestamp();
    let (day, month) = (day_bounds(now), month_bounds(now));
    match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::tally_between(&conn, day.0, day.1), store::tally_between(&conn, month.0, month.1))
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// The address of a game's card in the channel, so `/chess` can point at it.
pub fn jump_link(guild: Option<u64>, game: &Game) -> Option<String> {
    Some(format!("https://discord.com/channels/{}/{}/{}", guild?, game.channel, game.message?))
}

/// The public address anyone can watch a game at - and read its replay from
/// once it is over. No link of a player's is in it: it is only the game's
/// number, which the card shows anyway.
pub fn watch_link(game_id: i64) -> Option<String> {
    Some(format!("{}/chess/watch/{}", control::web::panel_url()?, game_id))
}

/// The private board address for one player in one game.
pub fn board_link(game_id: i64, user: u64) -> Option<String> {
    let base = control::web::panel_url()?;
    let now = Utc::now().timestamp();
    let token = with_db(|conn| store::token_for(conn, game_id, user, now, || rand::random::<f64>())).flatten()?;
    Some(format!("{}/chess/{}/{}", base, game_id, token))
}

/// The rules as a card, for both `/chesshelp` and the ❓ How to play button:
/// one set of words, written from the settings as they are now.
fn help_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("♟️ How chess works here")
        .description(super::rules_text::chess_help_text(&live_rules()))
        .colour(COLOUR)
}

/// `/chesstop [period]` - everyone, shown only to them, like `/housetop`.
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
    let message =
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// `/chesshelp` - everyone, shown only to them.
pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    let message = CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// `/chessstop <game>` - admins only.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper_command("Only mods can stop a game.")).await;
        return;
    }
    let id = command.data.options.iter().find_map(|o| o.value.as_i64()).unwrap_or(0);
    let Some(game) = with_db(|conn| store::get_game(conn, id)).flatten() else {
        let _ = command.create_response(&ctx.http, whisper_command(format!("There is no game #{}.", id))).await;
        return;
    };
    if !game.running() {
        let _ = command.create_response(&ctx.http, whisper_command(format!("Game #{} has already finished.", id))).await;
        return;
    }
    let _ = command.create_response(&ctx.http, whisper_command(format!("🛑 Game #{} cancelled. No points.", id))).await;
    tracing::info!("chess: /chessstop {} by {}", id, command.user.id);
    finish(id, "cancelled", None, false);
}

// --- buttons and the pop-up -------------------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let rest = |prefix: &str| id.strip_prefix(prefix).and_then(|r| r.parse::<i64>().ok());
    let draft = |prefix: &str| id.strip_prefix(prefix).filter(|d| !d.is_empty() && d.len() <= 32);
    if id == PICK_ID {
        start_picker(ctx, component).await;
    } else if let Some(draft) = draft(PICK_WHO) {
        picker_chose(ctx, component, &draft.to_string()).await;
    } else if let Some((draft, time)) = pace_pressed(&id) {
        picker_paced(ctx, component, &draft.to_string(), time).await;
    } else if id == HELP_ID {
        show_help(ctx, component).await;
    } else if let Some(draft) = draft(PICK_SEND) {
        picker_sent(ctx, component, &draft.to_string()).await;
    } else if let Some(draft) = draft(PICK_DROP) {
        picker_cancelled(ctx, component, &draft.to_string()).await;
    } else if let Some(challenge) = rest("chessyes:") {
        answer_challenge(ctx, component, challenge, true).await;
    } else if let Some(challenge) = rest("chessno:") {
        answer_challenge(ctx, component, challenge, false).await;
    } else if let Some(game) = rest("chessopen:") {
        open_board(ctx, component, game).await;
    } else if let Some(game) = rest("chesstype:") {
        type_move(ctx, component, game).await;
    } else if let Some(game) = rest("chessresignok:") {
        resign(ctx, component, game).await;
    } else if let Some(game) = rest("chessresign:") {
        ask_resign(ctx, component, game).await;
    } else if let Some(game) = rest("chessdrawyes:") {
        answer_draw(ctx, component, game, true).await;
    } else if let Some(game) = rest("chessdrawno:") {
        answer_draw(ctx, component, game, false).await;
    } else if let Some(game) = rest("chessdraw:") {
        offer_draw(ctx, component, game).await;
    }
}

const NOT_YOURS: &str = "This isn't your game.";

/// The game behind a button, if the person pressing is one of its players.
fn my_game(component: &ComponentInteraction, game_id: i64) -> Result<Game, &'static str> {
    let user = component.user.id.get();
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else {
        return Err("That game is gone.");
    };
    if !game.has(user) {
        return Err(NOT_YOURS);
    }
    if !game.running() {
        return Err("That game has finished.");
    }
    Ok(game)
}

async fn answer_challenge(ctx: &Context, component: &ComponentInteraction, id: i64, accept: bool) {
    let user = component.user.id.get();
    let Some(challenge) = with_db(|conn| store::get_challenge(conn, id)).flatten() else {
        return whisper(ctx, component, "That challenge is gone.").await;
    };
    if user != challenge.opponent && !(user == challenge.challenger && !accept) {
        return whisper(
            ctx,
            component,
            if accept { format!("Only <@{}> can accept this one.", challenge.opponent) } else { NOT_YOURS.to_string() },
        )
        .await;
    }
    if challenge.status != ChallengeStatus::Open {
        return whisper(ctx, component, "That challenge has already been answered.").await;
    }
    let status = if accept { ChallengeStatus::Accepted } else { ChallengeStatus::Declined };
    let closed = with_db(|conn| store::close_challenge(conn, id, status)).and_then(|r| r.ok()).unwrap_or(false);
    if !closed {
        return whisper(ctx, component, "That challenge has already been answered.").await;
    }
    let game = if accept { start_game(ctx, &challenge).await } else { None };
    let text = challenge_closed_text(&store::Challenge { status, ..challenge.clone() }, game.map(|g| g.id));
    let update = CreateInteractionResponseMessage::new()
        .embed(CreateEmbed::new().title("♟️ Chess challenge").description(text).colour(if accept { COLOUR } else { IDLE_COLOUR }))
        .components(Vec::new())
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
    nudge_task();
}

/// Makes the game a challenge was accepted into and puts its card up.
async fn start_game(ctx: &Context, challenge: &store::Challenge) -> Option<Game> {
    let (white, black) = rules::assign_colours(challenge.challenger, challenge.opponent, rand::random::<f64>());
    let time = TimeControl::from_key(&challenge.time_control);
    let now = Utc::now().timestamp();
    let fen = Replay::new().fen();
    let game = with_db(|conn| {
        store::start_game(
            conn,
            challenge.channel,
            white,
            black,
            &house_key(white),
            &house_key(black),
            &fen,
            time.key(),
            per_move_secs(time),
            now,
            &super::points::ist_day(now),
        )
    })
    .and_then(|r| r.ok())?;
    let _ = with_db(|conn| store::set_challenge_game(conn, challenge.id, game.id));
    tracing::info!("chess: game {} started, white {} black {} ({})", game.id, white, black, time.key());
    if let Some(message) = bottom_message(Bottom::Game(game.id), now).await {
        place_card(ctx, game.channel, Bottom::Game(game.id), message).await;
    }
    Some(game)
}

async fn open_board(ctx: &Context, component: &ComponentInteraction, game_id: i64) {
    let user = component.user.id.get();
    match my_game(component, game_id) {
        Err(text) => whisper(ctx, component, text).await,
        Ok(game) => match board_link(game.id, user) {
            None => {
                whisper(ctx, component, "The board page has no address yet — a mod needs to set the panel's web address.")
                    .await
            }
            Some(link) => {
                let text = format!(
                    "♟️ **Your board · Game #{}**\nTap a piece, tap where it goes. The page saves after every move.\n-# 🔒 Only you can see this link — it works for your side alone.",
                    game.id
                );
                let message = CreateInteractionResponseMessage::new()
                    .content(text)
                    .components(vec![CreateActionRow::Buttons(vec![CreateButton::new_link(link).label("Open my board")])])
                    .ephemeral(true)
                    .allowed_mentions(CreateAllowedMentions::new());
                let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
            }
        },
    }
}

/// The type-a-move pop-up, as raw JSON. Never a Text Display block: that newer
/// kind of pop-up block crashes the Discord iPhone app.
pub fn modal_json(game_id: i64, to_move: bool) -> Value {
    let placeholder = if to_move { "e4 · Nf3 · O-O · exd5 · e8=Q · g1f3" } else { "it isn't your move yet" };
    json!({
        "type": 9,
        "data": {
            "custom_id": format!("chessmove:{}", game_id),
            "title": format!("Your move - game #{}", game_id),
            "components": [{
                "type": 1,
                "components": [{
                    "type": 4, "custom_id": "move", "label": "Your move", "style": 1,
                    "required": true, "max_length": 12, "min_length": 2,
                    "placeholder": placeholder,
                }],
            }],
        }
    })
}

/// The move typed into a pop-up, whether the box came back inside an action row
/// or a label.
pub fn move_from(data: &Value) -> String {
    fn walk(v: &Value) -> Option<String> {
        match v {
            Value::Object(map) => {
                if map.get("custom_id").and_then(Value::as_str) == Some("move") {
                    if let Some(value) = map.get("value").and_then(Value::as_str) {
                        return Some(value.to_string());
                    }
                }
                ["component", "components"].iter().filter_map(|k| map.get(*k)).find_map(walk)
            }
            Value::Array(items) => items.iter().find_map(walk),
            _ => None,
        }
    }
    let root = data.get("components").unwrap_or(data);
    walk(root).map(|v| rules::tidy(&v)).unwrap_or_default()
}

async fn type_move(ctx: &Context, component: &ComponentInteraction, game_id: i64) {
    let user = component.user.id.get();
    match my_game(component, game_id) {
        Err(text) => whisper(ctx, component, text).await,
        Ok(game) => {
            let modal = modal_json(game_id, game.to_move() == user);
            if let Err(err) = ctx.http.create_interaction_response(component.id, &component.token, &modal, Vec::new()).await {
                tracing::warn!("chess: move pop-up for game {} not shown to {}: {}", game_id, user, err);
            }
        }
    }
}

pub async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    let Some(game_id) = modal.data.custom_id.strip_prefix("chessmove:").and_then(|r| r.parse::<i64>().ok()) else {
        return;
    };
    let user = modal.user.id.get();
    let data = serde_json::to_value(&modal.data).unwrap_or(Value::Null);
    let typed = move_from(&data);
    let text = match play(user, game_id, &typed) {
        Ok(played) => played.words,
        Err(why) => why,
    };
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = modal.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("chess: reply to {} not sent: {}", user, err);
    }
    nudge_task();
}

// --- making a move ---------------------------------------------------------------------------

/// What a move did.
pub struct Played {
    pub game_id: i64,
    pub san: String,
    /// What to tell the player who made it.
    pub words: String,
    /// Set when the move ended the game.
    pub ending: Option<Ending>,
}

/// Reads a typed move, checks it and writes it. The one door both the pop-up
/// and the web page go through, so neither can play a move the other couldn't.
pub fn play(user: u64, game_id: i64, typed: &str) -> Result<Played, String> {
    let db = store::db().ok_or_else(|| "Chess isn't ready yet — try again in a moment.".to_string())?;
    let now = Utc::now().timestamp();
    let conn = db.lock();
    let game = store::get_game(&conn, game_id).ok_or_else(|| "That game is gone.".to_string())?;
    if !game.has(user) {
        return Err(NOT_YOURS.to_string());
    }
    if !game.running() {
        return Err("That game has finished.".to_string());
    }
    if game.to_move() != user {
        return Err("⏳ It isn't your move yet.".to_string());
    }
    if rules::flagged(game.last_move_ts, game.per_move_secs, now) {
        return Err("⌛ Your time ran out on this one — the result is on its way.".to_string());
    }
    let mut replay = Replay::from_sans(&game.moves).map_err(|why| {
        tracing::error!("chess: game {} has an unreadable move list: {}", game_id, why);
        "Something is wrong with this game's move list. A mod can stop it with /chessstop.".to_string()
    })?;
    let m = rules::read_move(&replay.position, typed).map_err(|e: MoveError| e.words(typed))?;
    replay.push(m);
    let san = replay.sans.last().cloned().unwrap_or_default();
    let fen = replay.fen();
    let landed = store::play_move(&conn, game_id, &game.moves, &san, &fen, now).unwrap_or(false);
    if !landed {
        return Err("That move crossed with another. Look at the board and try again.".to_string());
    }
    // The database lock goes before anything else is touched: this is the one
    // place that holds it while it has work left to do, and the task's own lock
    // must never be taken under it.
    drop(conn);
    nudge_task();
    let ending = replay.ending();
    let words = match ending {
        Some(Ending::Checkmate { .. }) => format!("♟️ **{}** — checkmate. Game over!", san),
        Some(Ending::Stalemate) => format!("♟️ **{}** — stalemate, so it's a draw.", san),
        Some(Ending::Threefold) => format!("♟️ **{}** — that position for the third time: a draw.", san),
        Some(Ending::FiftyMove) => format!("♟️ **{}** — fifty moves with nothing taken: a draw.", san),
        Some(Ending::InsufficientMaterial) => format!("♟️ **{}** — neither side can mate now, so it's a draw.", san),
        None if replay.position.is_check() => format!("✅ **{}** played — check!", san),
        None => format!("✅ **{}** played.", san),
    };
    Ok(Played { game_id, san, words, ending })
}

// --- resigning and draws -----------------------------------------------------------------------

async fn ask_resign(ctx: &Context, component: &ComponentInteraction, game_id: i64) {
    match my_game(component, game_id) {
        Err(text) => whisper(ctx, component, text).await,
        Ok(game) => {
            let short = game.moves.len() < min_plies() as usize;
            let mut text = format!("🏳️ Really give up game #{}?", game.id);
            if short {
                text.push_str(&format!(
                    "\n-# It's only move {} — a game given up this early scores no chess points for either of you.",
                    game.moves.len() / 2 + 1
                ));
            }
            let message = CreateInteractionResponseMessage::new()
                .content(text)
                .components(vec![CreateActionRow::Buttons(vec![
                    CreateButton::new(format!("chessresignok:{}", game.id)).label("🏳️ Yes, resign").style(ButtonStyle::Danger),
                ])])
                .ephemeral(true)
                .allowed_mentions(CreateAllowedMentions::new());
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
        }
    }
}

async fn resign(ctx: &Context, component: &ComponentInteraction, game_id: i64) {
    let user = component.user.id.get();
    match my_game(component, game_id) {
        Err(text) => whisper(ctx, component, text).await,
        Ok(game) => {
            whisper(ctx, component, format!("🏳️ You resigned game #{}.", game.id)).await;
            tracing::info!("chess: {} resigned game {}", user, game.id);
            finish(game.id, "resign", Some(game.other(user)), true);
        }
    }
}

async fn offer_draw(ctx: &Context, component: &ComponentInteraction, game_id: i64) {
    let user = component.user.id.get();
    match my_game(component, game_id) {
        Err(text) => whisper(ctx, component, text).await,
        Ok(game) => {
            match offer_or_accept_draw(&game, user) {
                DrawMove::Agreed => whisper(ctx, component, "🤝 They offered first, so that's a draw.").await,
                DrawMove::Offered => {
                    whisper(ctx, component, format!("🤝 Draw offered in game #{}. It's up to them now.", game.id)).await
                }
                DrawMove::AlreadyOpen => {
                    whisper(ctx, component, "There's already a draw offer waiting on this game.").await
                }
            }
        }
    }
}

async fn answer_draw(ctx: &Context, component: &ComponentInteraction, game_id: i64, accept: bool) {
    let user = component.user.id.get();
    let game = match my_game(component, game_id) {
        Err(text) => return whisper(ctx, component, text).await,
        Ok(game) => game,
    };
    let Some(offered_by) = game.draw_offer else {
        return whisper(ctx, component, "That draw offer is no longer open.").await;
    };
    if offered_by == user {
        return whisper(ctx, component, "You're the one offering — it's their answer, not yours.").await;
    }
    let taken = with_db(|conn| store::take_draw_offer(conn, game.id, offered_by)).and_then(|r| r.ok()).unwrap_or(false);
    if !taken {
        return whisper(ctx, component, "That draw offer is no longer open.").await;
    }
    let text = if accept { format!("🤝 <@{}> accepted the draw in game #{}.", user, game.id) } else { format!("⚔️ <@{}> would rather play on — game #{} continues.", user, game.id) };
    let update = CreateInteractionResponseMessage::new().content(text).components(Vec::new()).allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(update)).await;
    if accept {
        finish(game.id, "draw", None, false);
    } else {
        nudge_task();
    }
}

/// What pressing 🤝 Offer draw did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrawMove {
    /// The other side had offered first, so pressing it agreed the draw.
    Agreed,
    /// A fresh offer is now waiting for the other side.
    Offered,
    /// This player's own offer is already waiting.
    AlreadyOpen,
}

/// Offers a draw, or takes the other side's offer. The one door the button and
/// the board page both go through.
pub fn offer_or_accept_draw(game: &Game, user: u64) -> DrawMove {
    if game.draw_offer == Some(game.other(user)) {
        let taken = with_db(|conn| store::take_draw_offer(conn, game.id, game.other(user))).and_then(|r| r.ok()).unwrap_or(false);
        if taken {
            finish(game.id, "draw", None, false);
            return DrawMove::Agreed;
        }
    }
    let opened = with_db(|conn| store::offer_draw(conn, game.id, user)).and_then(|r| r.ok()).unwrap_or(false);
    if opened {
        nudge_task();
        DrawMove::Offered
    } else {
        DrawMove::AlreadyOpen
    }
}

// --- ending a game -------------------------------------------------------------------------------

/// Ends a game in the database and throws its board links away. The task pays
/// it and posts the result card a moment later, so a button, a typed move and
/// the board page all end a game the same way and only one result can ever be
/// posted. Safe to call twice: only the first call finds the game running.
pub fn finish(game_id: i64, result: &str, winner: Option<u64>, by_resignation: bool) -> bool {
    let now = Utc::now().timestamp();
    let finished = with_db(|conn| store::finish_game(conn, game_id, result, winner, by_resignation, now))
        .and_then(|r| r.ok())
        .unwrap_or(false);
    if !finished {
        return false;
    }
    let _ = with_db(|conn| store::drop_tokens(conn, game_id));
    tracing::info!("chess: game {} ended ({}, winner {:?})", game_id, result, winner);
    nudge_task();
    true
}

/// Scores and announces every game that has ended but not yet been settled.
async fn settle_finished(ctx: &Context) {
    for game in with_db(store::unfinished_business).unwrap_or_default() {
        score_game(game.id);
        post_result(ctx, game.id).await;
    }
}

/// Writes what a finished game was worth to each player in CHESS points.
///
/// Nothing here touches the House Cup any more: a game of chess moves no house
/// points at all, so there is no cap to bite, no house to be in and no ledger
/// row to write. What it does write is the game's own score, which no limit ever
/// takes away. Safe to repeat: only the first call finds the game unsettled.
///
/// The one game that scores nothing on purpose is a rematch — a pair is scored
/// for one game a day, so two friends can't sit and trade wins into an uncapped
/// board.
fn score_game(game_id: i64) {
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else { return };
    if game.paid_out {
        return;
    }
    let result = match (game.result.as_deref(), game.winner) {
        (Some("cancelled"), _) => Result_::Cancelled,
        (_, Some(winner)) => Result_::Win { winner: if winner == game.white { Color::White } else { Color::Black } },
        (_, None) => Result_::Draw,
    };
    let mut payout =
        rules::payout(result, game.by_resignation, game.moves.len(), min_plies() as usize, win_points(), draw_points());
    let day = super::points::ist_day(game.finished_at.unwrap_or(game.started_at));
    if payout.pays() {
        let first_today = with_db(|conn| store::claim_pair_day(conn, &day, game.white, game.black, game.id))
            .and_then(|r| r.ok())
            .unwrap_or(true);
        if !first_today {
            payout = rules::Payout { why_nothing: "you two have already scored from chess today", ..rules::Payout::NOTHING };
        }
    }
    // The ledger is still told, and still told NOTHING: a zero row for each
    // player, written by the one door that turns mods, Muggles and the unsorted
    // away. It moves no house points and no total — it is the receipt that says
    // these two played chess today, which is what keeps the daily 🐸 card in the
    // right hands.
    let reason = format!("chess: game {} — chess points, no house points", game.id);
    for user in [game.white, game.black] {
        let key = format!("chess:{}:{}", game.id, user);
        let _ = super::house::award_person(user, Source::Chess, 0, &reason, None, Some(key), None);
    }
    tracing::info!("chess: game {} scored {} / {} chess points", game.id, payout.white, payout.black);
    let _ = with_db(|conn| store::record_payout(conn, game.id, payout.white, payout.black, payout.why_nothing));
}

async fn post_result(ctx: &Context, game_id: i64) {
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else { return };
    let now = Utc::now().timestamp();
    let (title, body) = result_text(
        &game,
        game.result.as_deref().unwrap_or("done"),
        game.winner,
        game.moves.len(),
        game.finished_at.unwrap_or(now) - game.started_at,
        (game.worth_white, game.worth_black),
        &game.why_nothing,
        watch_link(game.id).filter(|_| replay_days() > 0).as_deref(),
    );
    let mut message = CreateMessage::new()
        .embed(
            CreateEmbed::new()
                .title(title)
                .description(body)
                .colour(RESULT_COLOUR)
                .image(format!("attachment://{}", BOARD_FILE)),
        )
        .allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = board_attachment(&game, false).await {
        message = message.add_file(file);
    }
    // The game's own card goes: the result is the history now.
    let was_bottom = SHARED.lock().showing == Bottom::Game(game.id);
    if was_bottom {
        drop_card(ctx).await;
    } else if let Some(id) = game.message {
        delete(ctx, game.channel, id).await;
    }
    let _ = with_db(|conn| store::set_message(conn, game.id, None));
    if let Err(err) = call(ChannelId::new(game.channel).send_message(&ctx.http, message)).await {
        tracing::warn!("chess: result for game {} not posted: {}", game.id, err);
    }
}

// --- nudges ------------------------------------------------------------------------------------

/// The line a casual game posts when someone's clock is running low.
pub fn nudge_text(game_id: i64, user: u64, secs_left: i64) -> String {
    format!("⏰ <@{}> — **{}** left on your move in game #{}.", user, rules::clock_words(secs_left), game_id)
}

/// Whether a casual game should nudge the side to move now: a casual game, under
/// the threshold, still running, and that side hasn't been nudged since their
/// last move.
pub fn nudge_due(time: TimeControl, secs_left: i64, already: bool, threshold: i64) -> bool {
    time == TimeControl::Casual && !already && secs_left > 0 && secs_left <= threshold
}

async fn maybe_nudge(ctx: &Context, game: &Game, now: i64) {
    let time = TimeControl::from_key(&game.time_control);
    let left = rules::seconds_left(game.last_move_ts, game.per_move_secs, now);
    let to_move = game.to_move();
    let white = game.is_white(to_move);
    let already = if white { game.nudged_white } else { game.nudged_black };
    if !nudge_due(time, left, already, NUDGE_SECS) {
        return;
    }
    if !with_db(|conn| store::mark_nudged(conn, game.id, white)).and_then(|r| r.ok()).unwrap_or(false) {
        return;
    }
    let message = CreateMessage::new()
        .content(nudge_text(game.id, to_move, left))
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(to_move)]));
    let _ = call(ChannelId::new(game.channel).send_message(&ctx.http, message)).await;
    nudge_task();
}

// --- the game task --------------------------------------------------------------------------------

/// Starts the chess task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting()) {
        (true, Some(c)) => tracing::info!("chess: playing in {}", c),
        (true, None) => tracing::info!("chess: on, but VIZIER_CHESS_CHANNEL isn't set"),
        (false, _) => tracing::info!("chess: VIZIER_CHESS is off"),
    }
    tokio::spawn(run(ctx));
}

/// After a start: games a restart left unpaid are paid and announced, and the
/// channel's newest message is noted so the active card isn't moved at once.
async fn recover(ctx: &Context) {
    let now = Utc::now().timestamp();
    WOKE_AT.store(now, Ordering::SeqCst);
    let last_seen = meta_get("heartbeat").and_then(|v| v.parse::<i64>().ok());
    match forgiveness(last_seen, now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS) {
        Some(gap) => {
            let given = with_db(|conn| store::give_time_back(conn, gap)).and_then(|r| r.ok()).unwrap_or(0);
            tracing::info!("chess: down for {}s, so {} running games got that time back", gap, given);
        }
        None => match last_seen.map(|seen| now - seen) {
            None => tracing::info!("chess: no heartbeat to compare against, so no time was given back"),
            Some(gap) if gap < MIN_DOWNTIME_SECS => tracing::info!("chess: away {}s, too short to be worth giving back", gap),
            Some(gap) => tracing::warn!(
                "chess: the last heartbeat was {}s ago, more than the {}s that is believable, so no time was given back",
                gap,
                MAX_DOWNTIME_SECS
            ),
        },
    }
    settle_finished(ctx).await;
    let old_card = meta_get("card").and_then(|v| {
        let (c, m) = v.split_once(':')?;
        Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?))
    });
    if let Some(c) = active_channel(ctx) {
        if let Ok(latest) = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await {
            let mut s = SHARED.lock();
            s.latest = latest.first().map(|m| m.id.get()).unwrap_or(0);
            s.last_seen_ms = 0;
        }
    }
    if let Some((c, m)) = old_card {
        delete(ctx, c, m).await;
        meta_set("card", "");
    }
    SHARED.lock().dirty = true;
}

async fn run(ctx: Context) {
    recover(&ctx).await;
    // Positions already looked at for an ending, as (game, how many moves it
    // had), so a quiet game is not replayed every second.
    let mut checked: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();
    let mut last_beat = 0i64;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        expire_challenges(&ctx, now).await;
        let running = with_db(store::running_games).unwrap_or_default();
        beat(now, &mut last_beat);
        finish_ended_games(&running, &mut checked);
        time_out_games(&running, now, WOKE_AT.load(Ordering::SeqCst), grace_secs());
        settle_finished(&ctx).await;
        let Some(channel) = channel else {
            if SHARED.lock().card.is_some() {
                drop_card(&ctx).await;
            }
            continue;
        };
        for game in &running {
            announce_draw_offer(&ctx, game).await;
            maybe_nudge(&ctx, game, now).await;
        }
        keep_card_last(&ctx, channel, now).await;
    }
}

/// Games the position itself has ended: mate, stalemate, a third repetition,
/// fifty quiet moves, or too little material. A move made in a pop-up or on the
/// board page only writes the move; this is what turns it into a result.
fn finish_ended_games(running: &[Game], checked: &mut std::collections::HashMap<i64, usize>) {
    checked.retain(|id, _| running.iter().any(|g| g.id == *id));
    for game in running {
        if checked.get(&game.id) == Some(&game.moves.len()) {
            continue;
        }
        checked.insert(game.id, game.moves.len());
        let Some(ending) = Replay::from_sans(&game.moves).ok().and_then(|r| r.ending()) else { continue };
        let winner = ending.winner().map(|c| if c == Color::White { game.white } else { game.black });
        finish(game.id, ending.key(), winner, false);
    }
}

/// Tells the channel about a draw offer nobody has answered yet, once.
async fn announce_draw_offer(ctx: &Context, game: &Game) {
    let Some(from) = game.draw_offer.filter(|_| !game.draw_announced) else { return };
    if !with_db(|conn| store::announce_draw(conn, game.id)).and_then(|r| r.ok()).unwrap_or(false) {
        return;
    }
    let them = game.other(from);
    let message = CreateMessage::new()
        .content(format!("🤝 <@{}> offers a draw in **game #{}** — <@{}>, your call.", from, game.id, them))
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("chessdrawyes:{}", game.id)).label("🤝 Accept draw").style(ButtonStyle::Success),
            CreateButton::new(format!("chessdrawno:{}", game.id)).label("⚔️ Play on").style(ButtonStyle::Secondary),
        ])])
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(them)]));
    let _ = call(ChannelId::new(game.channel).send_message(&ctx.http, message)).await;
    nudge_task();
}

/// Challenges nobody answered in time.
async fn expire_challenges(ctx: &Context, now: i64) {
    for challenge in with_db(store::open_challenges).unwrap_or_default() {
        if challenge.expires_at > now {
            continue;
        }
        if !with_db(|conn| store::close_challenge(conn, challenge.id, ChallengeStatus::Expired)).and_then(|r| r.ok()).unwrap_or(false) {
            continue;
        }
        let Some(message) = challenge.message else { continue };
        let closed = store::Challenge { status: ChallengeStatus::Expired, ..challenge.clone() };
        let edit = EditMessage::new()
            .embed(CreateEmbed::new().title("♟️ Chess challenge").description(challenge_closed_text(&closed, None)).colour(IDLE_COLOUR))
            .components(Vec::new())
            .allowed_mentions(CreateAllowedMentions::new());
        let _ = call(ChannelId::new(challenge.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await;
    }
}

/// Writes down that the bot is still here, and when the earliest live game
/// needs a move, so a deploy script can wait rather than cost somebody a game.
fn beat(now: i64, last_beat: &mut i64) {
    if now - *last_beat < HEARTBEAT_SECS {
        return;
    }
    *last_beat = now;
    meta_set("heartbeat", &now.to_string());
    let until = with_db(store::live_move_until).flatten();
    meta_set(LIVE_UNTIL_KEY, &until.map(|t| t.to_string()).unwrap_or_default());
}

/// Games whose side to move has run out of time. Nothing is flagged in the
/// first moments after the bot wakes: the clocks have just been put right and a
/// player has to have a chance to see them.
fn time_out_games(running: &[Game], now: i64, woke_at: i64, grace: i64) {
    if !may_flag(now, woke_at, grace) {
        return;
    }
    for game in running {
        if !rules::flagged(game.last_move_ts, game.per_move_secs, now) {
            continue;
        }
        let loser = game.to_move();
        tracing::info!("chess: game {} timed out on {}", game.id, loser);
        finish(game.id, "timeout", Some(game.other(loser)), false);
    }
}

/// What the task should do with the channel's cards this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    /// Leave every card where it is.
    Nothing,
    /// Redraw the card at the bottom without moving it: its own game changed,
    /// or its clock has moved on.
    EditBottom,
    /// Post the card that should be last afresh at the bottom. When the card
    /// there belonged to a DIFFERENT game, that game keeps its own card where
    /// it stands and is redrawn in place first, so an older game never loses
    /// its board or its buttons.
    MoveToBottom,
}

/// The one decision behind several games at once.
///
/// Every game has a card of its own and every card's buttons carry its game's
/// number, so an older card goes on working where it is. Only ONE card is at
/// the bottom of the channel at a time: the game that moved most recently. A
/// move in that same game is an edit in place - it is already last - while a
/// move in another game, or any message landing below, is what moves a card.
/// Nothing else shuffles the channel.
#[allow(clippy::too_many_arguments)]
pub fn card_plan(has_card: bool, right_channel: bool, showing: Bottom, wanted: Bottom, due: bool, gone: bool, dirty: bool, stale: bool) -> CardAction {
    if !has_card || !right_channel || gone || showing != wanted || due {
        return CardAction::MoveToBottom;
    }
    if dirty || stale {
        return CardAction::EditBottom;
    }
    CardAction::Nothing
}

/// Moves the active card below anything that landed under it, puts it back if
/// someone deleted it, hands the bottom over when a different game became the
/// newest, and redraws it now and then so its clock keeps moving.
async fn keep_card_last(ctx: &Context, channel: u64, now: i64) {
    let wanted = bottom_now();
    let (card, showing, due, gone, dirty, refreshed) = {
        let mut s = SHARED.lock();
        let due = move_due(s.card.map(|(_, m)| m), s.latest, s.last_seen_ms, Utc::now().timestamp_millis(), MOVE_DELAY_MS);
        (s.card, s.showing, due, std::mem::take(&mut s.card_gone), std::mem::take(&mut s.dirty), s.refreshed_at)
    };
    let right_channel = card.map(|(c, _)| c) == Some(channel);
    let refresh_every = match wanted {
        Bottom::Game(id) => match with_db(|conn| store::get_game(conn, id)).flatten().map(|g| TimeControl::from_key(&g.time_control)) {
            Some(TimeControl::Live) => LIVE_REFRESH_SECS,
            _ => CASUAL_REFRESH_SECS,
        },
        Bottom::Idle => i64::MAX,
    };
    let stale = refresh_every != i64::MAX && now - refreshed >= refresh_every;
    match card_plan(card.is_some(), right_channel, showing, wanted, due, gone, dirty, stale) {
        CardAction::Nothing => {}
        CardAction::EditBottom => refresh_card(ctx, now).await,
        CardAction::MoveToBottom => {
            // The game that was last, if it is a different one, keeps its card
            // where it stands: it is redrawn there and the bottom is let go of,
            // so posting the new card does not take the old game's away.
            if showing != wanted {
                if let Bottom::Game(old) = showing {
                    let kept = SHARED.lock().card.map(|(_, m)| m);
                    let still_running = with_db(|conn| store::get_game(conn, old)).flatten().is_some_and(|g| g.running());
                    if still_running {
                        let _ = with_db(|conn| store::set_message(conn, old, kept));
                        if let Some(game) = with_db(|conn| store::get_game(conn, old)).flatten() {
                            edit_game_card(ctx, &game, now).await;
                        }
                        SHARED.lock().card = None;
                    }
                }
            }
            if let Some(message) = bottom_message(wanted, now).await {
                place_card(ctx, channel, wanted, message).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn game(moves: &[&str]) -> Game {
        let replay = Replay::from_sans(moves).expect("legal");
        Game {
            id: 42,
            channel: 5,
            white: 111,
            black: 222,
            white_house: "ravenclaw".into(),
            black_house: "gryffindor".into(),
            fen: replay.fen(),
            moves: replay.sans.clone(),
            status: "running".into(),
            time_control: "casual".into(),
            per_move_secs: 43_200,
            started_at: 1_000,
            last_move_ts: 1_000,
            message: None,
            result: None,
            winner: None,
            by_resignation: false,
            finished_at: None,
            points_white: 0,
            points_black: 0,
            worth_white: 0,
            worth_black: 0,
            paid_out: false,
            why_nothing: String::new(),
            draw_offer: None,
            draw_announced: false,
            nudged_white: false,
            nudged_black: false,
            given_back: 0,
        }
    }

    /// Writes the three cards out as a rough Discord mock-up when
    /// `CHESS_SHOTS` names a directory, so the wording and the board can be
    /// looked at with human eyes. Draws nothing otherwise.
    #[test]
    fn the_cards_can_be_written_out_to_be_looked_at() {
        let Ok(dir) = std::env::var("CHESS_SHOTS") else { return };
        std::fs::create_dir_all(&dir).expect("a place to put them");
        use base64::Engine as _;
        let board = |g: &Game| {
            let png = chess_board::board_png(&view_of(g, false)).expect("a board");
            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&*png))
        };
        let live = game(&["e4", "e5", "Nf3", "Nc6", "Bc4", "Nf6", "d3", "Bc5"]);
        let mate = game(&["e4", "e5", "Bc4", "Nc6", "Qh5", "Nf6", "Qxf7"]);
        let challenge = challenge_text(111, 222, TimeControl::Casual, 43_200, 4, 1, 1_790_000_000);
        let playing = game_text(&live, 42_120, live.started_at + 1_200, 4, 1);
        let (result_title, result_body) =
            result_text(&mate, "checkmate", Some(111), 7, 22_320, (4, 0), "", Some("https://panel.example/chess/watch/42"));
        let same = Game { black_house: "ravenclaw".into(), ..game(&["d4", "d5"]) };
        let same_text = game_text(&same, 41_000, same.started_at + 60, 4, 1);
        let idle = idle_text(&super::super::rules_text::tests::chess_defaults());
        let playing_title = game_title(42, 3);
        let same_title = game_title(43, 3);
        let cards = [
            ("♟️ Chess challenge", challenge, String::new(), vec!["✅ Accept", "❌ Decline"]),
            (playing_title.as_str(), playing, board(&live), vec!["♟️ Open board", "✍️ Type move", "🏳️ Resign", "🤝 Offer draw"]),
            (result_title.as_str(), result_body, board(&mate), Vec::new()),
            (same_title.as_str(), same_text, String::new(), vec!["♟️ Open board", "✍️ Type move", "🏳️ Resign", "🤝 Offer draw"]),
            ("♟️ Chess", idle, String::new(), Vec::new()),
        ];
        let mut html = String::from(
            "<!doctype html><meta charset=utf-8><title>Chess cards</title><style>\n\
             body{background:#313338;color:#dbdee1;font:15px/1.4 system-ui,sans-serif;margin:0;padding:22px;}\n\
             .wrap{max-width:660px;margin:0 auto;}\n\
             .card{border-left:4px solid #b58863;background:#2b2d31;border-radius:5px;padding:12px 14px;margin:0 0 6px;}\n\
             .card h3{margin:0 0 6px;font-size:16px;color:#f2f3f5;}\n\
             .card p{margin:0;white-space:pre-wrap;}\n\
             .card pre{background:#1e1f22;border-radius:4px;padding:8px 10px;margin:8px 0 0;font:13px/1.5 ui-monospace,monospace;overflow-x:auto;}\n\
             .card img{display:block;margin-top:10px;max-width:390px;width:100%;border-radius:6px;}\n\
             .row{display:flex;gap:8px;flex-wrap:wrap;margin:0 0 22px;}\n\
             .btn{background:#4e5058;color:#fff;border-radius:4px;padding:8px 13px;font-size:14px;font-weight:600;}\n\
             .mention{color:#c9cdfb;background:#3c4270;border-radius:3px;padding:0 2px;}\n\
             small{color:#949ba4;}\n\
             </style><div class=wrap>",
        );
        for (title, body, picture, buttons) in cards {
            let mut text = body.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            // Turn the mentions and the small lines into something that reads.
            for (id, name) in [(111, "@Aarav"), (222, "@Meera")] {
                text = text.replace(&format!("&lt;@{}&gt;", id), &format!("<span class=mention>{}</span>", name));
            }
            text = text.replace("-# ", "");
            let (text, block) = match text.split_once("```") {
                Some((head, rest)) => (head.to_string(), format!("<pre>{}</pre>", rest.trim_matches('`').trim())),
                None => (text, String::new()),
            };
            html.push_str(&format!("<div class=card><h3>{}</h3><p>{}</p>{}", title, text, block));
            if !picture.is_empty() {
                html.push_str(&format!("<img src=\"{}\" alt=board>", picture));
            }
            html.push_str("</div><div class=row>");
            for b in buttons {
                html.push_str(&format!("<span class=btn>{}</span>", b));
            }
            html.push_str("</div>");
        }
        html.push_str("</div>");
        std::fs::write(format!("{}/cards.html", dir), html).expect("written");
    }

    #[test]
    fn the_game_card_names_both_sides_and_whose_move_it_is() {
        let g = game(&["e4", "e5", "Nf3", "Nc6"]);
        let text = game_text(&g, 42_120, 2_200, 4, 1);
        assert!(text.contains("<@111>") && text.contains("<@222>"));
        assert!(text.contains("⬜ <@111>"), "white is to move on an even move list: {}", text);
        assert!(text.contains("11 h 42 m"), "{}", text);
        assert!(text.contains("move 3"), "{}", text);
        assert!(text.contains("1. e4 e5  2. Nf3 Nc6"), "{}", text);
        assert!(text.contains("Chess points"), "{}", text);
        assert!(text.contains("winner **+4**") && text.contains("draw **+1**"));

        let after = game(&["e4", "e5", "Nf3"]);
        assert!(game_text(&after, 100, 2_200, 4, 1).contains("⬛ <@222>"), "black to move after three plies");
    }

    #[test]
    fn a_game_inside_one_house_is_worth_exactly_what_any_other_game_is() {
        // It used to be worth nothing, because it moved nothing between houses.
        // Chess points aren't the House Cup's, so it scores like everything else.
        let g = Game { black_house: "ravenclaw".into(), ..game(&["e4"]) };
        assert!(g.same_house());
        let text = game_text(&g, 100, 2_000, 4, 1);
        assert!(text.contains("winner **+4**") && text.contains("no daily limit"), "{}", text);
        assert!(!text.contains("just for the fun of it"), "{}", text);
        assert!(worth_words(4, 1).contains("winner **+4**"));
        assert!(worth_words(4, 1).contains("Chess points"));
        // Only a game set to nothing at all says so.
        assert!(worth_words(0, 0).contains("scores nothing"));
    }

    #[test]
    fn no_card_of_a_chess_game_talks_about_house_points() {
        let g = game(&["e4", "e5"]);
        let idle = idle_text(&super::super::rules_text::tests::chess_defaults());
        let (_, result) = result_text(&g, "checkmate", Some(111), 7, 500, (4, 0), "", None);
        let (_, nothing) = result_text(&g, "cancelled", None, 7, 500, (0, 0), "the game was cancelled", None);
        let boards = top_text("today", &[store::Tally { user: 111, points: 4, games: 1, reached: 9 }], 111);
        for text in [
            game_text(&g, 100, 2_000, 4, 1),
            challenge_text(111, 222, TimeControl::Casual, 43_200, 4, 1, 9_999),
            idle,
            result,
            nothing,
            boards,
            worth_words(4, 1),
            worth_words(0, 0),
        ] {
            let flat = text.to_lowercase();
            assert!(!flat.contains("house point"), "still talks about house points: {}", text);
            assert!(!flat.contains("house cup"), "still talks about the House Cup: {}", text);
            assert!(!flat.contains("no points"), "a game is never reported as nothing: {}", text);
        }
    }

    #[test]
    fn an_open_draw_offer_shows_on_the_card() {
        let g = Game { draw_offer: Some(222), ..game(&["e4", "e5"]) };
        assert!(game_text(&g, 100, 2_000, 4, 1).contains("<@222> has offered a draw"));
    }

    #[test]
    fn the_challenge_card_says_who_what_and_until_when() {
        let text = challenge_text(111, 222, TimeControl::Casual, 43_200, 4, 1, 9_999);
        assert!(text.contains("<@111>") && text.contains("<@222>"));
        assert!(text.contains("Casual · 12 h 00 m per move"), "{}", text);
        assert!(text.contains("<t:9999:R>"), "Discord counts the offer down itself: {}", text);
        let live = challenge_text(1, 2, TimeControl::Live, 180, 4, 1, 5);
        assert!(live.contains("Live · 3 m 00 s per move"), "{}", live);
        assert!(live.contains("Chess points"), "{}", live);
    }

    #[test]
    fn a_closed_challenge_says_how_it_closed() {
        let c = store::Challenge {
            id: 1,
            channel: 5,
            message: None,
            challenger: 111,
            opponent: 222,
            time_control: "casual".into(),
            created_at: 0,
            expires_at: 100,
            status: ChallengeStatus::Accepted,
            game_id: None,
        };
        assert!(challenge_closed_text(&c, Some(42)).contains("Game #42"));
        assert!(challenge_closed_text(&store::Challenge { status: ChallengeStatus::Declined, ..c.clone() }, None).contains("declined"));
        assert!(challenge_closed_text(&store::Challenge { status: ChallengeStatus::Expired, ..c.clone() }, None).contains("lapsed"));
    }

    // --- challenging with the button ---------------------------------------------

    /// The picker's components as Discord will actually be sent them.
    fn rows_json(chosen: Option<u64>, time: TimeControl, ready: bool) -> Value {
        serde_json::to_value(picker_rows("bcdfgh23", chosen, time, ready, 43_200, 180)).expect("rows")
    }

    /// Writes the picker's components out when `CHESS_SHOTS` names a
    /// directory, so the shape Discord will be sent can be read with human
    /// eyes. Writes nothing otherwise.
    #[test]
    fn the_picker_can_be_written_out_to_be_looked_at() {
        let Ok(dir) = std::env::var("CHESS_SHOTS") else { return };
        std::fs::create_dir_all(&dir).expect("a place to put it");
        let shown = json!({
            "empty": {
                "text": picker_text(None, TimeControl::Casual, 43_200, None, None),
                "components": rows_json(None, TimeControl::Casual, false),
            },
            "ready": {
                "text": picker_text(Some(222), TimeControl::Live, 180, None, Some("🏠 You're both 🦅 Ravenclaw — a game inside one house is just for the fun of it, and pays nothing.")),
                "components": rows_json(Some(222), TimeControl::Live, true),
            },
            "refused": {
                "text": picker_text(Some(222), TimeControl::Casual, 43_200, Some(&Refusal::TooManyTheirs(3).words(222)), None),
                "components": rows_json(Some(222), TimeControl::Casual, false),
            },
        });
        std::fs::write(format!("{}/picker.json", dir), serde_json::to_string_pretty(&shown).expect("json")).expect("written");
    }

    #[test]
    fn the_picker_asks_with_menus_and_never_with_the_keyboard() {
        let rows = rows_json(None, TimeControl::Casual, false);
        let rows = rows.as_array().expect("three rows");
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert_eq!(row["type"], 1, "every one is an action row");
        }
        // Who: a USER select, so there is nothing to type and nothing to spell.
        let who = &rows[0]["components"][0];
        assert_eq!(who["type"], 5, "a user select, not a text box");
        assert_eq!(who["custom_id"], "chesswho:bcdfgh23");
        assert_eq!(who["min_values"], 1);
        assert_eq!(who["max_values"], 1);
        // The pace: two buttons, the chosen one lit. No menu to open, and
        // nothing to type.
        let pace = rows[1]["components"].as_array().expect("two paces");
        assert_eq!(pace.len(), 2);
        assert_eq!(pace[0]["custom_id"], "chesstime:bcdfgh23:casual");
        assert_eq!(pace[0]["label"], "🕰️ Casual · 12 h 00 m");
        assert_eq!(pace[0]["style"], 1, "the chosen pace is lit");
        assert_eq!(pace[1]["custom_id"], "chesstime:bcdfgh23:live");
        assert_eq!(pace[1]["label"], "⚡ Live · 3 m 00 s");
        assert_eq!(pace[1]["style"], 2);
        let live = rows_json(None, TimeControl::Live, false);
        assert_eq!(live[1]["components"][0]["style"], 2, "and the other one is not");
        assert_eq!(live[1]["components"][1]["style"], 1);
        // Nothing anywhere in the picker is a text input: a text input is
        // component type 4, and there is not one in any of the three rows.
        for rows in [rows_json(None, TimeControl::Casual, false), rows_json(Some(222), TimeControl::Live, true)] {
            for row in rows.as_array().unwrap() {
                for part in row["components"].as_array().unwrap() {
                    assert_ne!(part["type"], 4, "no typing at any step: {}", part);
                }
            }
        }
    }

    #[test]
    fn send_is_dead_until_somebody_has_been_picked() {
        let idle = rows_json(None, TimeControl::Casual, false);
        let send = &idle[2]["components"][0];
        assert_eq!(send["custom_id"], "chesssend:bcdfgh23");
        assert_eq!(send["disabled"], true, "nothing to send yet");
        let ready = rows_json(Some(222), TimeControl::Live, true);
        assert_eq!(ready[2]["components"][0]["disabled"], false);
        // Picked, but something is in the way: still dead.
        assert_eq!(rows_json(Some(222), TimeControl::Live, false)[2]["components"][0]["disabled"], true);
        // Cancel is always there, and the choice made so far is remembered.
        assert_eq!(ready[2]["components"][1]["custom_id"], "chesscancel:bcdfgh23");
        assert_eq!(ready[2]["components"][1]["disabled"], false, "cancel is always live");
        assert_eq!(ready[1]["components"][1]["style"], 1, "live stays picked");
        assert_eq!(ready[0]["components"][0]["default_values"][0]["id"], 222, "and so does the member");
    }

    #[test]
    fn the_picker_says_what_has_been_chosen_and_what_is_in_the_way() {
        let empty = picker_text(None, TimeControl::Casual, 43_200, None, None);
        assert!(empty.contains("Nobody picked yet"), "{}", empty);
        assert!(empty.contains("Casual · 12 h 00 m per move"), "{}", empty);
        assert!(empty.contains("forgets itself after 15 minutes"), "{}", empty);

        let chosen = picker_text(Some(222), TimeControl::Live, 180, None, None);
        assert!(chosen.contains("<@222>"), "{}", chosen);
        assert!(chosen.contains("Live · 3 m 00 s per move"), "{}", chosen);

        // A refusal is shown in the picker, never posted in the channel.
        let refused = picker_text(Some(222), TimeControl::Casual, 43_200, Some(&Refusal::Bot.words(222)), None);
        assert!(refused.contains("🚫"), "{}", refused);
        assert!(refused.contains("Pick a human"), "{}", refused);
        // A note is only shown when there is nothing to refuse.
        let noted = picker_text(Some(222), TimeControl::Casual, 43_200, None, Some("🏠 same house"));
        assert!(noted.contains("🏠 same house"), "{}", noted);
        let both = picker_text(Some(222), TimeControl::Casual, 43_200, Some("no"), Some("🏠 same house"));
        assert!(!both.contains("🏠 same house"), "the refusal is the thing that matters: {}", both);
    }

    #[test]
    fn the_picker_no_longer_warns_about_houses_because_houses_no_longer_matter() {
        // Every one of these used to be a warning that the game would pay
        // nothing. Chess points don't care which house anyone is in, so there is
        // nothing left to say.
        for (mine, theirs) in [("ravenclaw", "ravenclaw"), ("ravenclaw", ""), ("", "gryffindor"), ("ravenclaw", "gryffindor")] {
            assert_eq!(picker_note(222, mine, theirs), None, "{} vs {}", mine, theirs);
        }
    }

    #[test]
    fn a_half_filled_picker_is_forgotten_after_a_quarter_of_an_hour() {
        let mut drafts = HashMap::new();
        let draft = |made_at| Draft { by: 111, target: None, time: TimeControl::Casual, made_at };
        drafts.insert("aaa".to_string(), draft(1_000));
        drafts.insert("bbb".to_string(), draft(1_000 + DRAFT_SECS - 1));
        assert_eq!(sweep_drafts(&mut drafts, 1_000 + DRAFT_SECS - 1, DRAFT_SECS, DRAFT_MAX), 0, "both still young");
        assert_eq!(sweep_drafts(&mut drafts, 1_000 + DRAFT_SECS, DRAFT_SECS, DRAFT_MAX), 1, "the first has timed out");
        assert!(drafts.contains_key("bbb") && !drafts.contains_key("aaa"));
        assert_eq!(sweep_drafts(&mut drafts, 1_000 + 2 * DRAFT_SECS, DRAFT_SECS, DRAFT_MAX), 1);
        assert!(drafts.is_empty());

        // Somebody leaning on the button loses their oldest, not the machine's memory.
        for n in 0..10 {
            drafts.insert(format!("id{}", n), draft(1_000 + n));
        }
        assert_eq!(sweep_drafts(&mut drafts, 1_005, DRAFT_SECS, 4), 6);
        assert_eq!(drafts.len(), 4);
        assert!(drafts.contains_key("id9") && !drafts.contains_key("id0"), "the newest are the ones kept");
    }

    #[test]
    fn a_drafts_name_is_short_random_and_has_no_vowels_in_it() {
        let mut n = 0u64;
        let mut roll = || {
            n = n.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (n >> 11) as f64 / (1u64 << 53) as f64
        };
        let one = draft_id(&mut roll);
        let two = draft_id(&mut roll);
        assert_eq!(one.len(), DRAFT_ID_LEN);
        assert_ne!(one, two);
        assert!(one.bytes().all(|b| DRAFT_ALPHABET.contains(&b)), "{}", one);
        assert!(!one.chars().any(|c| "aeiou".contains(c)), "{}", one);
        // It has to fit inside a custom id with its prefix, well under Discord's 100.
        assert!(PICK_WHO.len() + DRAFT_ID_LEN < 100);
        assert_eq!(draft_id(|| 0.0), "b".repeat(DRAFT_ID_LEN), "the bottom of the range is in range");
        assert_eq!(draft_id(|| 1.0), "9".repeat(DRAFT_ID_LEN), "and so is the top");
    }

    #[test]
    fn a_pace_button_says_which_draft_and_which_pace_it_is() {
        assert_eq!(pace_pressed("chesstime:bcdfgh23:casual"), Some(("bcdfgh23", TimeControl::Casual)));
        assert_eq!(pace_pressed("chesstime:bcdfgh23:live"), Some(("bcdfgh23", TimeControl::Live)));
        assert_eq!(pace_pressed("chesstime:bcdfgh23:blitz"), None, "only the two paces there are");
        assert_eq!(pace_pressed("chesstime::live"), None, "a draft has to be named");
        assert_eq!(pace_pressed("chesstime:bcdfgh23"), None);
        assert_eq!(pace_pressed("chesstype:42"), None, "not the pop-up button, which reads alike");
        assert_eq!(pace_pressed("chesstime:"), None);
    }

    #[test]
    fn the_button_that_opens_the_picker_is_on_both_cards() {
        // Nobody can type in that channel - the message box itself is denied
        // them - so both cards have to offer a way to START a game and a way to
        // find out HOW, with no slash command anywhere in it.
        let idle = serde_json::to_value(idle_rows()).expect("rows");
        let game = serde_json::to_value(game_rows(42)).expect("rows");
        let ids = |row: &Value| -> Vec<String> {
            row["components"].as_array().expect("buttons").iter().filter_map(|b| b["custom_id"].as_str().map(String::from)).collect()
        };
        assert_eq!(ids(&idle[0]), vec![PICK_ID, HELP_ID]);
        assert_eq!(idle[0]["components"][0]["label"], "⚔️ Challenge someone");
        assert_eq!(idle[0]["components"][1]["label"], "❓ How to play");

        // A game card keeps its own four buttons and puts these in a row of
        // their own, because a row holds five and the Watch link may be a fifth.
        let rows = game.as_array().expect("two rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(ids(&rows[0]), vec!["chessopen:42", "chesstype:42", "chessresign:42", "chessdraw:42"]);
        assert!(rows[0]["components"].as_array().unwrap().len() <= 5, "Discord allows five to a row");
        assert_eq!(ids(&rows[1]), vec![PICK_ID, HELP_ID], "someone watching can start their own, and read the rules");

        // And the card a beginner meets first offers the rules beside the answer.
        let challenge = serde_json::to_value(challenge_rows(7)).expect("rows");
        assert_eq!(ids(&challenge[0]), vec!["chessyes:7", "chessno:7", HELP_ID]);
    }

    #[test]
    fn both_ways_in_are_refused_for_exactly_the_same_reasons() {
        // `may_challenge` is the only judge, and the picker and the command
        // both ask it; this pins the shape so neither can quietly grow a rule
        // the other doesn't have.
        for (mine, theirs, running, already, bot, expected) in [
            (0usize, 0usize, 0usize, false, true, Some(Refusal::Bot)),
            (0, 0, 0, true, false, Some(Refusal::AlreadyAsked)),
            (3, 0, 3, false, false, Some(Refusal::TooManyYours(3))),
            (0, 3, 3, false, false, Some(Refusal::TooManyTheirs(3))),
            (0, 0, 8, false, false, Some(Refusal::ChannelFull(8))),
            (0, 0, 0, false, false, None),
        ] {
            assert_eq!(may_challenge(111, 222, bot, mine, theirs, running, already, 3, 8).err(), expected);
        }
        assert_eq!(may_challenge(111, 111, false, 0, 0, 0, false, 3, 8).err(), Some(Refusal::Self_), "not yourself");
        // And every refusal reads as a sentence a member can act on.
        for refusal in [Refusal::Off, Refusal::Self_, Refusal::Bot, Refusal::AlreadyAsked, Refusal::TooManyYours(3), Refusal::TooManyTheirs(3), Refusal::ChannelFull(8)] {
            let words = refusal.words(222);
            assert!(words.ends_with('.') && words.chars().count() > 20, "{:?}: {}", refusal, words);
        }
    }

    #[test]
    fn a_challenge_is_refused_for_the_reasons_it_should_be() {
        // (mine, theirs, running, already) against limits of 3 each and 8 in the channel.
        let ask = |mine, theirs, running, already, bot| may_challenge(1, 2, bot, mine, theirs, running, already, 3, 8);
        assert_eq!(may_challenge(1, 1, false, 0, 0, 0, false, 3, 8), Err(Refusal::Self_));
        assert_eq!(ask(0, 0, 0, false, true), Err(Refusal::Bot));
        assert_eq!(ask(0, 0, 0, true, false), Err(Refusal::AlreadyAsked));
        assert_eq!(ask(3, 0, 3, false, false), Err(Refusal::TooManyYours(3)));
        assert_eq!(ask(2, 3, 5, false, false), Err(Refusal::TooManyTheirs(3)));
        assert_eq!(ask(2, 2, 2, false, false), Ok(()));
        // Both well under their own limit, but the channel is full.
        assert_eq!(ask(1, 1, 8, false, false), Err(Refusal::ChannelFull(8)));
        assert_eq!(ask(1, 1, 7, false, false), Ok(()), "one place left");
        assert!(Refusal::TooManyTheirs(3).words(2).contains("<@2>"));
        assert!(Refusal::AlreadyAsked.words(2).contains("already have a challenge"));
        assert!(Refusal::ChannelFull(8).words(2).contains("**8 games**"));
    }

    #[test]
    fn the_bottom_goes_to_the_game_that_moved_last() {
        assert_eq!(newest_of(&[]), Bottom::Idle, "nothing running: the idle card");
        let one = Game { id: 1, last_move_ts: 500, ..game(&["e4"]) };
        let two = Game { id: 2, last_move_ts: 400, ..game(&["d4"]) };
        let three = Game { id: 3, last_move_ts: 500, ..game(&["c4"]) };
        assert_eq!(newest_of(&[one.clone(), two.clone()]), Bottom::Game(1));
        // Game 2 moves: the bottom is handed over, and game 1's card stays put.
        let two_moved = Game { last_move_ts: 900, ..two.clone() };
        assert_eq!(newest_of(&[one.clone(), two_moved.clone()]), Bottom::Game(2));
        assert_eq!(card_plan(true, true, Bottom::Game(1), Bottom::Game(2), false, false, true, false), CardAction::MoveToBottom);
        // Game 1 answers: it takes the bottom back.
        let one_moved = Game { last_move_ts: 950, ..one.clone() };
        assert_eq!(newest_of(&[one_moved, two_moved]), Bottom::Game(1));
        // Two moves in the same second: the later game wins, so the answer is stable.
        assert_eq!(newest_of(&[one, three]), Bottom::Game(3));
    }

    #[test]
    fn the_bottom_card_follows_the_latest_move_and_older_games_stay_put() {
        use CardAction::*;
        let (a, b) = (Bottom::Game(1), Bottom::Game(2));
        // A quiet tick with the right card already last: nothing moves.
        assert_eq!(card_plan(true, true, a, a, false, false, false, false), Nothing);
        // A move in the game that is already last is an edit in place.
        assert_eq!(card_plan(true, true, a, a, false, false, true, false), EditBottom);
        // So is a clock that has moved on.
        assert_eq!(card_plan(true, true, a, a, false, false, false, true), EditBottom);
        // A move in ANOTHER game hands the bottom over; game 1 keeps its card.
        assert_eq!(card_plan(true, true, a, b, false, false, true, false), MoveToBottom);
        assert_eq!(card_plan(true, true, a, b, false, false, false, false), MoveToBottom);
        // Something landed below it, or someone deleted it, or it is elsewhere.
        assert_eq!(card_plan(true, true, a, a, true, false, false, false), MoveToBottom);
        assert_eq!(card_plan(true, true, a, a, false, true, false, false), MoveToBottom);
        assert_eq!(card_plan(true, false, a, a, false, false, false, false), MoveToBottom);
        assert_eq!(card_plan(false, true, a, a, false, false, false, false), MoveToBottom);
        // The last game finishing puts the idle card up in its place.
        assert_eq!(card_plan(true, true, a, Bottom::Idle, false, false, false, false), MoveToBottom);
        assert_eq!(card_plan(true, true, Bottom::Idle, Bottom::Idle, false, false, false, false), Nothing);
    }

    #[test]
    fn the_your_games_list_shows_whose_move_the_clock_and_both_links() {
        let mut waiting = game(&["e4"]);
        waiting.message = Some(777);
        let jump = jump_link(Some(99), &waiting);
        assert_eq!(jump.as_deref(), Some("https://discord.com/channels/99/5/777"));
        assert_eq!(jump_link(None, &waiting), None, "no server, no jump");
        assert_eq!(jump_link(Some(99), &game(&["e4"])), None, "no card yet, no jump");

        // White has moved, so white is waiting and black is on the clock.
        let white = my_game_line(&waiting, 111, 40_000, Some("https://panel/chess/42/tok"), jump.as_deref());
        assert!(white.contains("**#42**") && white.contains("<@222>"), "{}", white);
        assert!(white.contains("you're white"), "{}", white);
        assert!(white.contains("waiting for <@222>"), "{}", white);
        assert!(white.contains("11 h 06 m"), "{}", white);
        assert!(white.contains("[open your board](https://panel/chess/42/tok)"), "{}", white);
        assert!(white.contains("[jump to the card](https://discord.com/channels/99/5/777)"), "{}", white);

        let black = my_game_line(&waiting, 222, 40_000, None, None);
        assert!(black.contains("you're black") && black.contains("**your move**"), "{}", black);
        assert!(!black.contains("open your board"), "no link when the panel has no address: {}", black);
    }

    #[test]
    fn a_card_says_how_many_games_are_running() {
        assert_eq!(game_title(42, 1), "♟️ Game #42");
        assert_eq!(game_title(42, 0), "♟️ Game #42");
        assert_eq!(game_title(42, 3), "♟️ Game #42 · 3 games running");
    }

    #[test]
    fn every_way_a_game_can_end_reads_properly() {
        let g = game(&["f3", "e5", "g4", "Qh4"]);
        let replay = "https://panel/chess/watch/42";
        let (title, body) = result_text(&g, "checkmate", Some(222), 4, 22_320, (0, 4), "", Some(replay));
        assert_eq!(title, "🏁 Checkmate · Game #42");
        assert!(body.contains("<@222>") && body.contains("beat <@111>"), "{}", body);
        assert!(body.contains("in **2 moves**"), "{}", body);
        assert!(body.contains("Chess points: <@222> **+4**"), "{}", body);
        assert!(body.contains("lasted 6 h 12 m"), "{}", body);
        assert!(body.contains("[replay the moves](https://panel/chess/watch/42)"), "{}", body);

        let (title, body) = result_text(&g, "resign", Some(111), 30, 60, (0, 0), "it was given up too early to count", None);
        assert!(title.starts_with("🏳️ Resignation"));
        assert!(body.contains("resigned"), "{}", body);
        assert!(body.contains("no chess points: it was given up too early"), "{}", body);
        assert!(!body.contains("replay"), "no replay line when replays are off: {}", body);

        let (title, body) = result_text(&g, "timeout", Some(111), 9, 90_000, (4, 0), "", None);
        assert!(title.starts_with("⌛ Out of time"));
        assert!(body.contains("on time"), "{}", body);
        assert!(body.contains("**5 moves**"), "nine plies is five moves: {}", body);

        let (title, body) = result_text(&g, "draw", None, 60, 600, (1, 1), "", None);
        assert!(title.starts_with("🤝 Draw agreed"));
        assert!(body.contains("drew after **30 moves**"), "{}", body);
        assert!(body.contains("<@111> **+1**") && body.contains("<@222> **+1**"), "{}", body);

        for (key, head) in [("stalemate", "🤝 Stalemate"), ("threefold", "🤝 Draw by repetition"), ("fifty", "🤝 Draw by the fifty-move rule"), ("material", "🤝 Draw — neither side can mate"), ("cancelled", "🛑 Cancelled by a mod")] {
            assert!(result_text(&g, key, None, 10, 10, (0, 0), "", None).0.starts_with(head), "{} should read as {}", key, head);
        }
    }

    #[test]
    fn a_move_is_only_played_by_the_player_whose_turn_it_is() {
        // Without a database open, every path refuses rather than panicking.
        assert!(play(111, 42, "e4").is_err());
    }

    #[test]
    fn the_pop_up_has_one_plain_box_and_never_a_text_display() {
        let modal = modal_json(42, true);
        assert_eq!(modal["type"], 9);
        assert_eq!(modal["data"]["custom_id"], "chessmove:42");
        let rows = modal["data"]["components"].as_array().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["type"], 1, "an action row, never a type 10 Text Display");
        let input = &rows[0]["components"][0];
        assert_eq!(input["type"], 4);
        assert_eq!(input["custom_id"], "move");
        assert!(input["placeholder"].as_str().unwrap().contains("Nf3"));
        assert!(modal_json(42, false)["data"]["components"][0]["components"][0]["placeholder"]
            .as_str()
            .unwrap()
            .contains("isn't your move"));
    }

    #[test]
    fn a_typed_move_is_found_however_discord_wraps_it() {
        let rows = json!({"components": [{"type": 1, "components": [{"type": 4, "custom_id": "move", "value": " Nf3 "}]}]});
        assert_eq!(move_from(&rows), "Nf3");
        let labels = json!({"components": [{"type": 18, "component": {"type": 4, "custom_id": "move", "value": "e2e4"}}]});
        assert_eq!(move_from(&labels), "e2e4");
        assert_eq!(move_from(&json!({"components": []})), "");
    }

    #[test]
    fn the_board_picture_follows_the_position_the_last_move_and_the_check() {
        let g = game(&["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6"]);
        let view = view_of(&g, false);
        assert_eq!(view.placement, g.fen);
        assert_eq!(view.last, "g8f6", "the black knight's move is lit");
        assert_eq!(view.check, "");
        assert!(!view.flipped);
        assert!(view_of(&g, true).flipped, "black sees it the other way round");

        let mate = game(&["e4", "e5", "Qh5", "Nc6", "Bc4", "Nf6", "Qxf7"]);
        assert_eq!(view_of(&mate, false).check, "e8", "the mated king's square is lit");
        assert_eq!(view_of(&mate, false).last, "h5f7");
        let fresh = game(&[]);
        assert_eq!(view_of(&fresh, false).last, "", "nothing is lit before the first move");
    }

    #[test]
    fn a_restart_gives_the_clocks_back_the_time_it_cost_them() {
        let now = 1_000_000i64;
        // A normal restart: a couple of minutes, handed straight back.
        assert_eq!(forgiveness(Some(now - 120), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), Some(120));
        assert_eq!(forgiveness(Some(now - MIN_DOWNTIME_SECS), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), Some(MIN_DOWNTIME_SECS));
        assert_eq!(forgiveness(Some(now - MAX_DOWNTIME_SECS), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), Some(MAX_DOWNTIME_SECS));
        // A blink between two ticks is not worth the bookkeeping.
        assert_eq!(forgiveness(Some(now - 3), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None);
        // Days off is a machine that was switched off, not a game that was paused.
        assert_eq!(forgiveness(Some(now - 3 * 86_400), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None);
        assert_eq!(forgiveness(Some(now - MAX_DOWNTIME_SECS - 1), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None);
        // A first ever start has nothing to compare against.
        assert_eq!(forgiveness(None, now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None);
        // A clock from the future (the machine's own was wrong) is not believed.
        assert_eq!(forgiveness(Some(now + 500), now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None);
    }

    #[test]
    fn nobody_is_flagged_in_the_moments_after_the_bot_wakes() {
        let woke = 5_000i64;
        assert!(!may_flag(woke, woke, 60), "not the instant it comes up");
        assert!(!may_flag(woke + 59, woke, 60));
        assert!(may_flag(woke + 60, woke, 60));
        assert!(may_flag(woke + 600, woke, 60));
        // With the grace turned off, the clock is judged at once.
        assert!(may_flag(woke, woke, 0));
    }

    #[test]
    fn a_game_given_time_back_says_so_until_it_is_played_on() {
        let given = Game { given_back: 135, ..game(&["e4", "e5"]) };
        let text = game_text(&given, 43_000, 2_000, 4, 1);
        assert!(text.contains("⏸️ 2 m given back after an update"), "{}", text);
        let quiet = game(&["e4", "e5"]);
        assert!(!game_text(&quiet, 43_000, 2_000, 4, 1).contains("given back"));
    }

    #[test]
    fn a_casual_game_nudges_once_and_a_live_one_never() {
        assert!(nudge_due(TimeControl::Casual, 6_000, false, NUDGE_SECS));
        assert!(!nudge_due(TimeControl::Casual, 6_000, true, NUDGE_SECS), "only once per move");
        assert!(!nudge_due(TimeControl::Casual, 9_000, false, NUDGE_SECS), "still plenty of time");
        assert!(!nudge_due(TimeControl::Casual, 0, false, NUDGE_SECS), "already out of time: that is a result, not a nudge");
        assert!(!nudge_due(TimeControl::Live, 60, false, NUDGE_SECS), "live games move too fast to nudge");
        assert!(nudge_text(42, 111, 5_400).contains("<@111>"));
        assert!(nudge_text(42, 111, 5_400).contains("game #42"));
    }

    #[test]
    fn the_active_card_only_moves_once_the_channel_has_gone_quiet() {
        // Nothing below it: it stays where it is.
        assert!(!move_due(Some(100), 100, 0, 10_000, MOVE_DELAY_MS));
        // Something below it, but the talking is still going on.
        assert!(!move_due(Some(100), 101, 9_000, 10_000, MOVE_DELAY_MS));
        assert!(move_due(Some(100), 101, 6_000, 10_000, MOVE_DELAY_MS));
        // No card at all: nothing to move.
        assert!(!move_due(None, 101, 0, 10_000, MOVE_DELAY_MS));
    }

    #[test]
    fn the_idle_card_points_at_the_command_and_the_points() {
        let text = idle_text(&ChessRules {
            channel: Some(5),
            casual_hours: 12,
            live_seconds: 180,
            max_games: 3,
            max_active: 8,
            min_plies: 10,
            replay_days: 30,
            win: 4,
            draw: 1,
        });
        assert!(text.contains("No game running"), "{}", text);
        assert!(text.contains("⚔️ Challenge someone"), "the button comes first: {}", text);
        assert!(text.contains("`/chess @member`"), "and the command still works: {}", text);
        assert!(text.find("⚔️").unwrap() < text.find("/chess").unwrap(), "the button is named first: {}", text);
        assert!(text.contains("+4") && text.contains("+1") && text.contains("no daily limit"), "{}", text);
        assert!(text.contains("❓ How to play"), "and the rules are a button too: {}", text);
    }

    #[test]
    fn the_clocks_on_a_card_come_from_the_settings() {
        assert_eq!(control_words(TimeControl::Casual, 43_200), "Casual · 12 h 00 m per move");
        assert_eq!(control_words(TimeControl::Live, 180), "Live · 3 m 00 s per move");
        assert_eq!(control_words(TimeControl::Live, 45), "Live · 45 s per move");
    }
}
