//! Letter Duel: a tile game in its own channel (`VIZIER_DUEL_CHANNEL`).
//!
//! Two to four people share a board. `/duel`, or the ⚔️ button on the idle
//! card, puts a LOBBY up with a Join button and a countdown the bot edits onto
//! the card every few seconds — Discord's own relative timestamps do not
//! refresh where anybody is looking, so the seconds are written out in words
//! and rewritten as they run down. The lobby starts early when it fills and is
//! called off, with nothing lost, if too few turn up.
//!
//! After that it is ordinary Scrabble, and the rules live in
//! [`super::duel_rules`]: a fifteen-by-fifteen board with the standard premium
//! squares, the standard hundred tiles, seven on a rack, the first word over
//! the middle, every word made — sideways ones included — in the dictionary,
//! fifty for using all seven, blanks worth nothing, exchanges while the bag is
//! full enough, and the game over when somebody uses their last tile or
//! everybody passes twice.
//!
//! A player's own tiles are theirs alone: a private page at `/duel/<game>/<token>`
//! shows one rack and one rack only, exactly the way the chess board page shows
//! one side. The page adds up what a play would score before it is committed
//! and says in plain words why an illegal one is refused — and the server then
//! checks the whole thing again, because the page is convenience and never
//! evidence.
//!
//! ONE card is the channel's last message: the lobby, the game, or the idle
//! line. It is redrawn in place when its own words change, and posted again at
//! the bottom once chat has buried it — paced by how long since the card
//! actually MOVED, not by how long since a card was posted, so a game whose
//! board changes every few seconds still follows the conversation down.
//!
//! Points: the winner, the runner-up, and one to everyone who played to the
//! end, so turning up pays. A game with only TWO people in it pays the winner
//! the runner-up's share and nobody else anything at all, so two friends can't
//! farm each other. Alongside that each player keeps an uncapped **duel points**
//! tally, the way anagrams and Guess the Word do, so mods and members in no
//! house score something too.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{Value, json};
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions,
    CreateAttachment, CreateButton, CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditAttachments, EditMessage,
    GetMessages, Message, MessageId, UserId,
};

use super::chess_rules::{clock_words, span_words};
use super::control;
use super::duel_board::{self, View};
use super::duel_rules::{self as rules, Placement, Refusal};
use super::duel_store::{self as store, Game, Lobby, LobbyStatus, Player, Written};
use super::duel_words;
use super::points::{Outcome, Source};
use super::sudoku_gen::Rng;

/// How often the game task looks at the clocks.
const TICK: Duration = Duration::from_secs(1);
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// How often a lobby card is rewritten so its countdown moves.
const COUNTDOWN_EDIT_SECS: i64 = 5;
/// How often a running game's card is redrawn so its clock moves.
const CLOCK_EDIT_SECS: i64 = 15;
/// How often the bot writes down that it is still here. A restart reads the
/// last one to work out how long the clocks ran unwatched.
const HEARTBEAT_SECS: i64 = 30;
/// Downtime shorter than this is not worth giving back.
pub const MIN_DOWNTIME_SECS: i64 = 10;
/// Downtime longer than this is not believed: a machine that was off for days
/// should not hand every game most of a week.
pub const MAX_DOWNTIME_SECS: i64 = 6 * 3600;
/// How long after the bot wakes up before anybody can lose a turn to the clock.
/// A player must have a real chance to move, not be passed the moment the bot
/// comes back.
pub const GRACE_SECS: i64 = 30;
/// Turns somebody may let run out before they are taken out of the game.
pub const MAX_MISSES: i64 = 3;
/// How many names a lobby card lists.
const LOBBY_NAMES: usize = 8;

const COLOUR: u32 = 0x2E7D52;
const LOBBY_COLOUR: u32 = 0xD9A441;
const RESULT_COLOUR: u32 = 0xE8B923;
const IDLE_COLOUR: u32 = 0x4E5058;

const BOARD_FILE: &str = "duel.png";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_DUEL", false)
}

/// The channel the game was made for, when the setting is empty.
pub const HOME_CHANNEL: u64 = 1_550_088_683_414_225_006;

fn channel_setting() -> Option<u64> {
    let set = control::id("VIZIER_DUEL_CHANNEL").filter(|id| *id != 0).unwrap_or(HOME_CHANNEL);
    Some(set).filter(|id| *id != super::weekly::SAFE_CORNER)
}

/// The channel to play in right now, if the game is on AND there are words to
/// play with. No dictionary means no game, whatever the toggle says.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on() && duel_words::bank().is_some())
}

fn lobby_secs() -> i64 {
    control::number("VIZIER_DUEL_LOBBY_SECS", 120).clamp(15, 1800) as i64
}

fn min_players() -> usize {
    control::number("VIZIER_DUEL_MIN", 2).clamp(2, 4) as usize
}

fn max_players() -> usize {
    (control::number("VIZIER_DUEL_MAX", 4).clamp(2, 4) as usize).max(min_players())
}

fn turn_secs() -> i64 {
    control::number("VIZIER_DUEL_TURN_SECS", 240).clamp(20, 3600) as i64
}

fn win_points() -> i64 {
    control::number("VIZIER_POINTS_DUEL_WIN", 4).min(100) as i64
}

fn second_points() -> i64 {
    control::number("VIZIER_POINTS_DUEL_SECOND", 2).min(100) as i64
}

fn played_points() -> i64 {
    control::number("VIZIER_POINTS_DUEL_PLAYED", 1).min(100) as i64
}

fn bump_messages() -> u64 {
    control::number("VIZIER_DUEL_BUMP_MESSAGES", 5).clamp(1, 100)
}

fn bump_seconds() -> i64 {
    control::number("VIZIER_DUEL_BUMP_SECONDS", 60).clamp(10, 3600) as i64
}

fn daily_cap() -> Option<i64> {
    match Source::Duel.cap() {
        super::points::Cap::PerDay(n) => Some(n),
        _ => None,
    }
}

/// Everything the cards and the help say about what a game is worth, as the
/// settings have it now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prizes {
    pub win: i64,
    pub second: i64,
    pub played: i64,
    pub cap: Option<i64>,
}

pub fn prizes() -> Prizes {
    Prizes { win: win_points(), second: second_points(), played: played_points(), cap: daily_cap() }
}

// --- shared state ------------------------------------------------------------------------

/// What the channel's last message should be.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Bottom {
    /// Nothing is happening.
    #[default]
    Idle,
    /// A lobby is filling up.
    Lobby(i64),
    /// A game is being played.
    Game(i64),
}

#[derive(Default)]
struct Shared {
    /// (channel, message) of the card, and what it shows.
    card: Option<(u64, u64)>,
    showing: Bottom,
    /// Messages from anybody else that have landed under the card.
    others_since_card: u64,
    /// When the card last MOVED, which is what the cooldown paces.
    last_bump_ms: i64,
    /// The card was deleted by somebody.
    card_gone: bool,
    /// Something changed and the card's words are out of date.
    dirty: bool,
    /// When the card was last redrawn, so its clock keeps moving.
    refreshed_at: i64,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// When the bot came up, so the grace window can be worked out.
static WOKE_AT: AtomicI64 = AtomicI64::new(0);

/// Every message in the duel channel that isn't the bot's own card.
pub fn note_message(ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != channel_setting() {
        return;
    }
    let mine = msg.author.id == ctx.cache.current_user().id;
    let mut s = SHARED.lock();
    if !mine && s.card.map(|(_, m)| m) != Some(msg.id.get()) {
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

fn nudge_task() {
    SHARED.lock().dirty = true;
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

// --- small words -------------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// A house crest for a member, or a hat for somebody in no house.
pub fn crest(user: u64) -> &'static str {
    super::house::house_of(user).map(|h| h.crest).unwrap_or("🧙")
}

/// The house a member counts as being in for the points. Empty for a Muggle,
/// a mod with no house, or anyone the Sorting Hat hasn't reached — they play
/// and they score duel points, but they move nothing between houses.
fn house_key(user: u64) -> String {
    if super::house::opted_out(user) {
        return String::new();
    }
    super::house::house_of(user).map(|h| h.key.to_string()).unwrap_or_default()
}

/// Time left as a card shows it: "2:40" above a minute, "35 s" below. Written
/// out rather than left to Discord, whose relative timestamps do not tick.
pub fn left_words(until: i64, now: i64) -> String {
    let left = (until - now).max(0);
    if left >= 60 { format!("{}:{:02}", left / 60, left % 60) } else { format!("{} s", left) }
}

/// What the prizes come to, in one line.
pub fn worth_words(p: Prizes) -> String {
    let limit = match p.cap {
        Some(n) => format!(" · max **{}** a day", n),
        None => String::new(),
    };
    format!(
        "🏆 Winner **+{}** · runner-up **+{}** · **+{}** to everyone who plays to the end{}\n\
         -# Three or more players for the full prizes — a two-player game pays the winner **+{}** and nothing else, \
         so two friends can't farm each other. Duel points are kept for everybody, whatever your house.",
        p.win,
        p.second,
        p.played,
        limit,
        p.win.min(p.second)
    )
}

// --- the lobby card -------------------------------------------------------------------------

pub fn lobby_title() -> String {
    "⚔️ Letter Duel — taking seats".to_string()
}

/// What a lobby card says while it is filling up.
pub fn lobby_text(seats: &[u64], closes_at: i64, now: i64, min: usize, max: usize, p: Prizes) -> String {
    let mut text = format!(
        "A game of tiles is starting. Press **⚔️ Join** to take a seat — **{}** to **{}** players.\n\
         ⏳ Starts in **{}** · **{}/{}** in",
        min,
        max,
        left_words(closes_at, now),
        seats.len(),
        max
    );
    if seats.len() < min {
        text.push_str(&format!(" · needs **{}**", min));
    } else {
        text.push_str(" · enough to play");
    }
    text.push('\n');
    if seats.is_empty() {
        text.push_str("\nNobody yet. Who's first?\n");
    } else {
        let names: Vec<String> = seats.iter().take(LOBBY_NAMES).map(|u| format!("{} <@{}>", crest(*u), u)).collect();
        text.push('\n');
        text.push_str(&names.join(" · "));
        text.push('\n');
    }
    text.push_str(&format!("\n{}", worth_words(p)));
    text
}

/// How a lobby reads once it has closed.
pub fn lobby_closed_text(seats: usize, min: usize, game: Option<i64>) -> String {
    match game {
        Some(id) => format!("✅ **{}** took a seat — Game #{} is on.", plural(seats as i64, "player", "players"), id),
        None => format!(
            "😴 Only **{}** joined and **{}** are needed, so this one is off. Press **⚔️ Start a duel** to try again — nothing is lost.",
            plural(seats as i64, "player", "players"),
            min
        ),
    }
}

fn lobby_embed(lobby: &Lobby, now: i64) -> CreateEmbed {
    let seats: Vec<u64> = lobby.seats.iter().map(|s| s.user).collect();
    CreateEmbed::new()
        .title(lobby_title())
        .description(lobby_text(&seats, lobby.closes_at, now, min_players(), max_players(), prizes()))
        .colour(LOBBY_COLOUR)
        .footer(CreateEmbedFooter::new("Your tiles are yours alone — you get a private board page when it starts"))
}

fn lobby_rows(lobby: i64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("dueljoin:{}", lobby)).label("⚔️ Join").style(ButtonStyle::Success),
        CreateButton::new(format!("duelleavelobby:{}", lobby)).label("🚪 Leave").style(ButtonStyle::Secondary),
        help_button(),
    ])]
}

// --- the game card ---------------------------------------------------------------------------

pub fn game_title(game_id: i64) -> String {
    format!("🔠 Letter Duel · Game #{}", game_id)
}

/// The line that says what just happened.
pub fn last_line(game: &Game, turn: Option<&store::Turn>) -> String {
    let Some(turn) = turn else {
        return "The board is empty — the first word has to cross the middle ★.".to_string();
    };
    let who = game.player(turn.seat).map(|p| p.user).unwrap_or(0);
    match turn.kind.as_str() {
        "word" => format!("📝 <@{}> {} played **{}** for **{}**", who, crest(who), turn.word, plural(turn.score, "point", "points")),
        "pass" => format!("➖ <@{}> {} passed", who, crest(who)),
        "exchange" => format!("♻️ <@{}> {} swapped **{}** for fresh ones", who, crest(who), plural(turn.score.max(0), "tile", "tiles")),
        "missed" => format!("⌛ <@{}> {} ran out of time — that turn is a pass", who, crest(who)),
        "dropped" => format!("🚪 <@{}> {} has left the game", who, crest(who)),
        _ => String::new(),
    }
}

/// Everybody's score, best first, with the player to move marked.
pub fn scores_line(game: &Game) -> String {
    let mut players: Vec<&Player> = game.players.iter().collect();
    players.sort_by(|a, b| b.total().cmp(&a.total()).then(a.seat.cmp(&b.seat)));
    players
        .iter()
        .map(|p| {
            let mark = if game.turn == p.seat && !p.dropped { "▶️ " } else { "" };
            let out = if p.dropped { " *(left)*" } else { "" };
            format!("{}{} <@{}> **{}**{}", mark, crest(p.user), p.user, p.total(), out)
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The whole game card.
pub fn game_text(game: &Game, turn: Option<&store::Turn>, secs_left: i64, now: i64) -> String {
    let mut text = format!("{}\n{}\n", last_line(game, turn), scores_line(game));
    match game.to_move() {
        Some(who) => text.push_str(&format!(
            "\n🎲 <@{}> {} to play · ⏱️ **{}** left\n",
            who,
            crest(who),
            clock_words(secs_left)
        )),
        None => text.push_str("\n🏁 Nobody left to play — the game is ending.\n"),
    }
    text.push_str(&format!(
        "-# {} left in the bag · turn {} · started {} ago",
        plural(game.bag_left() as i64, "tile", "tiles"),
        game.turns + 1,
        span_words(now - game.started_at)
    ));
    if game.given_back > 0 {
        text.push_str(&format!(
            "\n-# ⏸️ {} given back after an update — nobody loses a turn to a restart.",
            span_words(game.given_back)
        ));
    }
    text
}

fn game_embed(game: &Game, turn: Option<&store::Turn>, now: i64) -> CreateEmbed {
    let left = seconds_left(game, now);
    CreateEmbed::new()
        .title(game_title(game.id))
        .description(game_text(game, turn, left, now))
        .colour(COLOUR)
        .image(format!("attachment://{}", BOARD_FILE))
        .footer(CreateEmbedFooter::new("Open your rack for a tap-to-place board — only you ever see your tiles"))
}

fn game_rows(game_id: i64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("duelopenrack:{}", game_id)).label("🔤 Open my rack").style(ButtonStyle::Success),
        CreateButton::new(format!("duelpass:{}", game_id)).label("➖ Pass").style(ButtonStyle::Secondary),
        CreateButton::new(format!("duelquit:{}", game_id)).label("🚪 Leave").style(ButtonStyle::Secondary),
        help_button(),
    ])]
}

/// What the picture on a game's card should show.
pub fn view_of(game: &Game, turn: Option<&store::Turn>) -> View {
    let fresh = turn.filter(|t| t.kind == "word").map(|t| t.covered()).unwrap_or_default();
    View::new(&game.board, &fresh)
}

async fn board_attachment(view: View) -> Option<CreateAttachment> {
    let png = tokio::task::spawn_blocking(move || duel_board::board_png(&view)).await.ok().flatten()?;
    Some(CreateAttachment::bytes(png.to_vec(), BOARD_FILE))
}

// --- the idle card -----------------------------------------------------------------------------

pub fn idle_text(min: usize, max: usize, turn: i64, p: Prizes) -> String {
    format!(
        "🔠 **No game running** — press **⚔️ Start a duel** below, or use `/duel`.\n\
         **{}** to **{}** players share one board: make words with your seven tiles, and the premium squares \
         are worth having. **{}** a turn.\n\n{}\n-# Press **❓ How to play** for the rules · `/dueltop` for the board.",
        min,
        max,
        clock_words(turn),
        worth_words(p)
    )
}

fn idle_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("🔠 Letter Duel")
        .description(idle_text(min_players(), max_players(), turn_secs(), prizes()))
        .colour(IDLE_COLOUR)
}

pub fn idle_rows() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(OPEN_ID).label("⚔️ Start a duel").style(ButtonStyle::Primary),
        help_button(),
    ])]
}

pub const OPEN_ID: &str = "duelopen";
pub const HELP_ID: &str = "duelhelp";

pub fn help_button() -> CreateButton {
    CreateButton::new(HELP_ID).label("❓ How to play").style(ButtonStyle::Secondary)
}

// --- the result card ---------------------------------------------------------------------------

/// How a finished game is announced.
pub fn result_headline(result: &str) -> &'static str {
    match result {
        "played out" => "🏁 Last tile down",
        "passed out" => "🏁 Everybody passed",
        "walkover" => "🏁 Everybody else left",
        "cancelled" => "🛑 Cancelled by a mod",
        _ => "🏁 Game over",
    }
}

/// A place as a medal, or nothing for the rest.
fn medal(place: usize) -> &'static str {
    match place {
        1 => "🥇",
        2 => "🥈",
        3 => "🥉",
        _ => "▫️",
    }
}

/// One player's line on the result card: where they came, what they finished
/// on, and what the leftover tiles did to it.
pub fn result_line(p: &Player, place: usize) -> String {
    let mut line = format!("{} <@{}> {} **{}**", medal(place), p.user, crest(p.user), p.total());
    if p.adjust != 0 {
        line.push_str(&format!(" -# ({} from the tiles left over)", if p.adjust > 0 { format!("+{}", p.adjust) } else { p.adjust.to_string() }));
    }
    if p.dropped {
        line.push_str(" *(left the game)*");
    }
    line
}

/// The whole result card. `places` is each player's finishing place, in seat
/// order, which the caller has already worked out from the totals.
pub fn result_text(game: &Game, places: &[usize], lasted: i64, why_nothing: &str) -> (String, String) {
    let title = format!("{} · Game #{}", result_headline(game.result.as_deref().unwrap_or("done")), game.id);
    let mut ranked: Vec<(usize, &Player)> =
        game.players.iter().enumerate().map(|(i, p)| (places.get(i).copied().unwrap_or(9), p)).collect();
    ranked.sort_by_key(|(place, p)| (*place, p.seat));
    let mut body: String = ranked.iter().map(|(place, p)| result_line(p, *place)).collect::<Vec<_>>().join("\n");

    let paid: Vec<String> =
        game.players.iter().filter(|p| p.points > 0).map(|p| format!("<@{}> **+{}**", p.user, p.points)).collect();
    if paid.is_empty() {
        let why = if why_nothing.is_empty() { "no House Cup points this time".to_string() } else { format!("no House Cup points: {}", why_nothing) };
        body.push_str(&format!("\n\n🏠 {}", why));
    } else {
        body.push_str(&format!("\n\n🏠 House points: {}", paid.join(" · ")));
    }
    let worth: Vec<String> =
        game.players.iter().filter(|p| p.worth > 0).map(|p| format!("<@{}> **+{}**", p.user, p.worth)).collect();
    if !worth.is_empty() {
        body.push_str(&format!("\n🔠 Duel points: {} -# (no daily limit · `/dueltop`)", worth.join(" · ")));
    }
    body.push_str(&format!("\n-# {} · the game lasted {}", plural(game.turns, "turn", "turns"), span_words(lasted)));
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
        tracing::debug!("duel: message {} not deleted: {}", message, err);
    }
}

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let message =
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
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
    let name = ctx.cache.guilds().iter().find_map(|g| {
        ctx.cache.guild(*g).and_then(|guild| guild.channels.get(&ChannelId::new(channel)).map(|c| c.name.to_lowercase()))
    });
    name.is_some_and(|n| n.contains("safe-corner"))
}

fn active_channel(ctx: &Context) -> Option<u64> {
    live_channel().filter(|c| !is_safe_corner(ctx, *c))
}

// --- the card ------------------------------------------------------------------------------------

/// What should be at the bottom of the channel now: the game if one is running,
/// else the lobby if one is open, else the idle line.
fn bottom_now(channel: u64) -> Bottom {
    with_db(|conn| match store::live_game(conn, channel) {
        Some(game) => Bottom::Game(game.id),
        None => match store::live_lobby(conn, channel) {
            Some(lobby) => Bottom::Lobby(lobby.id),
            None => Bottom::Idle,
        },
    })
    .unwrap_or(Bottom::Idle)
}

/// The embed, the buttons and the picture for whatever should be at the bottom.
async fn bottom_parts(bottom: Bottom, now: i64) -> Option<(CreateEmbed, Vec<CreateActionRow>, Option<CreateAttachment>)> {
    match bottom {
        Bottom::Idle => Some((idle_embed(), idle_rows(), None)),
        Bottom::Lobby(id) => {
            let lobby = with_db(|conn| store::get_lobby(conn, id)).flatten()?;
            Some((lobby_embed(&lobby, now), lobby_rows(lobby.id), None))
        }
        Bottom::Game(id) => {
            let (game, turn) = with_db(|conn| (store::get_game(conn, id), store::last_turn(conn, id)))?;
            let game = game?;
            let file = board_attachment(view_of(&game, turn.as_ref())).await;
            Some((game_embed(&game, turn.as_ref(), now), game_rows(game.id), file))
        }
    }
}

/// Posts the card afresh and takes the old one away. `moved` says this is the
/// card coming DOWN to the bottom rather than a new thing going up, which is
/// the only thing the cooldown paces — a game whose board changes every few
/// seconds would otherwise never be allowed to follow the chat down.
async fn place_card(ctx: &Context, channel: u64, bottom: Bottom, message: CreateMessage, moved: bool) -> Option<u64> {
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let id = posted.id.get();
            let old = {
                let mut s = SHARED.lock();
                s.dirty = false;
                s.card_gone = false;
                s.others_since_card = 0;
                s.showing = bottom;
                s.refreshed_at = Utc::now().timestamp();
                if moved {
                    s.last_bump_ms = Utc::now().timestamp_millis();
                }
                s.card.replace((channel, id))
            };
            meta_set("card", &format!("{}:{}", channel, id));
            if let Some(game) = match bottom {
                Bottom::Game(g) => Some(g),
                _ => None,
            } {
                let _ = with_db(|conn| store::set_message(conn, game, Some(id)));
            }
            if let Bottom::Lobby(l) = bottom {
                let _ = with_db(|conn| store::set_lobby_message(conn, l, Some(id)));
            }
            if let Some((c, m)) = old.filter(|(_, m)| *m != id) {
                delete(ctx, c, m).await;
            }
            Some(id)
        }
        Err(err) => {
            tracing::warn!("duel: card not posted in {}: {}", channel, err);
            None
        }
    }
}

/// Takes the card away without a new one.
async fn drop_card(ctx: &Context) {
    let card = SHARED.lock().card.take();
    meta_set("card", "");
    if let Some((c, m)) = card {
        delete(ctx, c, m).await;
    }
}

/// Which card a thing that has ended may take away: its OWN, and only if that
/// is still the card the channel is showing.
///
/// A game's result closes it in the store straight away, so the next card can
/// already be up by the time the result is posted. A blind `drop_card` then
/// deleted the NEW card and left the channel with nothing at all — the very
/// thing that went wrong on the anagrams game, and the reason this is spelled
/// out rather than assumed.
pub fn card_to_drop(current: Option<(u64, u64)>, ended: Option<u64>) -> Option<(u64, u64)> {
    match (current, ended) {
        (Some((channel, card)), Some(ended)) if card == ended => Some((channel, card)),
        _ => None,
    }
}

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

/// Redraws the card where it stands.
async fn edit_card(ctx: &Context, bottom: Bottom, now: i64) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let Some((embed, rows, file)) = bottom_parts(bottom, now).await else { return };
    let mut edit = EditMessage::new().embed(embed).components(rows).allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = file {
        edit = edit.attachments(EditAttachments::new().add(file));
    }
    match call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        Ok(_) => {
            let mut s = SHARED.lock();
            s.refreshed_at = now;
            s.dirty = false;
        }
        Err(err) => {
            if is_gone(&err) {
                SHARED.lock().card_gone = true;
            }
            tracing::warn!("duel: card not redrawn: {}", err);
        }
    }
}

/// Whether chat has buried the card: enough messages from other people have
/// landed under it AND the card hasn't already moved in the last little while.
pub fn bump_due(others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> bool {
    others >= needed.max(1) && since_bump_ms >= every_secs.max(0) * 1_000
}

/// What to do with the card this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Nothing,
    /// Redraw it where it is.
    Edit,
    /// Post it again at the bottom and take the old one away.
    Bump,
}

/// The ONE decision behind the card, so nothing else in the game posts one.
///
/// A missing card, a card in the wrong channel, a card somebody deleted and a
/// card showing the wrong thing all come to the same thing: put one up. Chat
/// burying it moves it down. Anything else is at most a redraw in place, which
/// is what keeps a chatty channel from ending up with two cards in it.
#[allow(clippy::too_many_arguments)]
pub fn card_plan(
    has_card: bool,
    right_channel: bool,
    gone: bool,
    showing: Bottom,
    wanted: Bottom,
    dirty: bool,
    stale: bool,
    others: u64,
    since_bump_ms: i64,
    needed: u64,
    every_secs: i64,
) -> CardAction {
    if !has_card || !right_channel || gone || showing != wanted {
        return CardAction::Bump;
    }
    if bump_due(others, since_bump_ms, needed, every_secs) {
        return CardAction::Bump;
    }
    if dirty || stale {
        return CardAction::Edit;
    }
    CardAction::Nothing
}

/// How often the thing at the bottom wants redrawing so its countdown moves.
fn refresh_every(bottom: Bottom) -> i64 {
    match bottom {
        Bottom::Lobby(_) => COUNTDOWN_EDIT_SECS,
        Bottom::Game(_) => CLOCK_EDIT_SECS,
        Bottom::Idle => i64::MAX,
    }
}

/// Keeps the channel's one card where people can see it. The only place in the
/// game that posts or edits a card.
async fn tend_card(ctx: &Context, channel: u64, now: i64) {
    let wanted = bottom_now(channel);
    let (card, showing, gone, dirty, others, last_bump, refreshed) = {
        let mut s = SHARED.lock();
        (s.card, s.showing, std::mem::take(&mut s.card_gone), s.dirty, s.others_since_card, s.last_bump_ms, s.refreshed_at)
    };
    let right_channel = card.map(|(c, _)| c) == Some(channel);
    let every = refresh_every(wanted);
    let stale = every != i64::MAX && now - refreshed >= every;
    let since_bump = Utc::now().timestamp_millis() - last_bump;
    match card_plan(
        card.is_some(),
        right_channel,
        gone,
        showing,
        wanted,
        dirty,
        stale,
        others,
        since_bump,
        bump_messages(),
        bump_seconds(),
    ) {
        CardAction::Nothing => {}
        CardAction::Edit => edit_card(ctx, wanted, now).await,
        CardAction::Bump => {
            let moved = card.is_some() && right_channel && showing == wanted;
            let Some((embed, rows, file)) = bottom_parts(wanted, now).await else { return };
            let mut message =
                CreateMessage::new().embed(embed).components(rows).allowed_mentions(CreateAllowedMentions::new());
            if let Some(file) = file {
                message = message.add_file(file);
            }
            place_card(ctx, channel, wanted, message, moved).await;
        }
    }
}

// --- opening a lobby -------------------------------------------------------------------------

/// Why a lobby can't be opened now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Off,
    NoWords,
    LobbyOpen,
    GameRunning,
}

impl Refused {
    pub fn words(self) -> &'static str {
        match self {
            Refused::Off => "Letter Duel is switched off right now.",
            Refused::NoWords => "Letter Duel has no dictionary to play with — a mod needs to put `wordbank/dictionary.txt` in the workspace.",
            Refused::LobbyOpen => "A lobby is already open — press **⚔️ Join** on the card instead.",
            Refused::GameRunning => "A game is already being played. Wait for it to finish, and the next lobby goes up after.",
        }
    }
}

/// Whether a lobby may be opened, from what the database says.
pub fn may_open(on: bool, words: bool, lobby: bool, game: bool) -> Result<(), Refused> {
    if !on {
        return Err(Refused::Off);
    }
    if !words {
        return Err(Refused::NoWords);
    }
    if game {
        return Err(Refused::GameRunning);
    }
    if lobby {
        return Err(Refused::LobbyOpen);
    }
    Ok(())
}

/// Opens a lobby and says what to tell whoever asked for one. The ONE door
/// both `/duel` and the ⚔️ button go through.
async fn open_lobby(ctx: &Context, by: u64) -> Result<String, String> {
    let Some(channel) = active_channel(ctx) else {
        return Err(if duel_words::bank().is_none() && switched_on() { Refused::NoWords } else { Refused::Off }.words().to_string());
    };
    let now = Utc::now().timestamp();
    let state = with_db(|conn| (store::live_lobby(conn, channel).is_some(), store::live_game(conn, channel).is_some()));
    let Some((lobby, game)) = state else {
        return Err("Letter Duel isn't ready yet — try again in a moment.".to_string());
    };
    if let Err(why) = may_open(true, true, lobby, game) {
        return Err(why.words().to_string());
    }
    let made = with_db(|conn| store::open_lobby(conn, channel, by, now, now + lobby_secs())).and_then(|r| r.ok());
    let Some(lobby) = made else {
        return Err("Couldn't open a lobby. Try again.".to_string());
    };
    // Whoever asked for the game is in it — they shouldn't have to press Join
    // on their own lobby.
    let _ = with_db(|conn| store::join_lobby(conn, lobby.id, by, &house_key(by), now, max_players()));
    tracing::info!("duel: lobby {} open in {} for {}s", lobby.id, channel, lobby_secs());
    nudge_task();
    Ok(format!("⚔️ Lobby open in <#{}> — you're in. It starts in {}.", channel, clock_words(lobby_secs())))
}

/// Deals a lobby into a game: seats shuffled so the same person doesn't always
/// go first, a full bag, and seven tiles each.
pub fn deal(seats: &[(u64, String)], bag: String, rng: &mut Rng) -> (Vec<(u64, String, String)>, String) {
    let mut order: Vec<(u64, String)> = seats.to_vec();
    rng.shuffle(&mut order);
    let mut bag = bag;
    let mut out = Vec::new();
    for (user, house) in order {
        let (rack, left) = rules::draw(&bag, rules::RACK);
        bag = left;
        out.push((user, house, rack));
    }
    (out, bag)
}

/// Turns a lobby into a game, or calls it off. Only the first caller finds the
/// lobby open, so a full lobby and a countdown running out can never both start
/// one.
async fn close_lobby(ctx: &Context, lobby: &Lobby) {
    let now = Utc::now().timestamp();
    let seats: Vec<(u64, String)> = lobby.seats.iter().map(|s| (s.user, s.house.clone())).collect();
    if seats.len() < min_players() {
        if with_db(|conn| store::close_lobby(conn, lobby.id, LobbyStatus::CalledOff, None)).and_then(|r| r.ok()).unwrap_or(false) {
            tracing::info!("duel: lobby {} called off with {} in", lobby.id, seats.len());
            let text = lobby_closed_text(seats.len(), min_players(), None);
            let message = CreateMessage::new()
                .embed(CreateEmbed::new().title(lobby_title()).description(text).colour(IDLE_COLOUR))
                .allowed_mentions(CreateAllowedMentions::new());
            drop_card_of(ctx, lobby.message).await;
            let _ = call(ChannelId::new(lobby.channel).send_message(&ctx.http, message)).await;
            nudge_task();
        }
        return;
    }
    let mut rng = Rng::fresh();
    let (dealt, bag) = deal(&seats, rules::fresh_bag(&mut rng), &mut rng);
    let made = with_db(|conn| {
        if !store::close_lobby(conn, lobby.id, LobbyStatus::Started, None).unwrap_or(false) {
            return None;
        }
        let game = store::start_game(
            conn,
            lobby.channel,
            &dealt,
            &rules::empty_board(),
            &bag,
            turn_secs(),
            now,
            &super::points::ist_day(now),
        )
        .ok()?;
        let _ = store::set_lobby_game(conn, lobby.id, game.id);
        Some(game)
    })
    .flatten();
    let Some(game) = made else { return };
    tracing::info!("duel: game {} started with {} players", game.id, game.players.len());
    // The lobby card goes: the game's card is what the channel wants now.
    drop_card_of(ctx, lobby.message).await;
    let players: Vec<String> = game.players.iter().map(|p| format!("<@{}>", p.user)).collect();
    let message = CreateMessage::new()
        .content(format!(
            "{} — {}. Press **🔤 Open my rack** on the card below for your tiles.",
            lobby_closed_text(game.players.len(), min_players(), Some(game.id)),
            players.join(" · ")
        ))
        .allowed_mentions(CreateAllowedMentions::new().users(game.players.iter().map(|p| UserId::new(p.user)).collect::<Vec<_>>()));
    let _ = call(ChannelId::new(game.channel).send_message(&ctx.http, message)).await;
    nudge_task();
}

// --- taking a turn -----------------------------------------------------------------------------

/// How long the player to move has left.
pub fn seconds_left(game: &Game, now: i64) -> i64 {
    (game.turn_started_at + game.turn_secs - now).max(0)
}

/// Whether the player to move has run out of time.
pub fn out_of_time(game: &Game, now: i64) -> bool {
    now >= game.turn_started_at + game.turn_secs
}

/// Whether a clock may be judged yet: not within the grace of the bot waking.
pub fn may_flag(now: i64, woke_at: i64, grace: i64) -> bool {
    now - woke_at >= grace
}

const NOT_YOURS: &str = "You're not in this game.";
const NOT_READY: &str = "Letter Duel isn't ready yet — try again in a moment.";

/// What a turn did.
#[derive(Clone, Debug)]
pub struct Took {
    pub game_id: i64,
    /// What to tell the player who took it.
    pub words: String,
}

/// Everything a turn needs before it can be taken: the game, the seat, and the
/// player, with every refusal already made.
fn my_turn(conn: &rusqlite::Connection, user: u64, game_id: i64, now: i64) -> Result<(Game, usize), String> {
    let game = store::get_game(conn, game_id).ok_or_else(|| "That game is gone.".to_string())?;
    if !game.running() {
        return Err("That game has finished.".to_string());
    }
    let seat = game.seat_of(user).ok_or_else(|| NOT_YOURS.to_string())?;
    if game.player(seat).is_some_and(|p| p.dropped) {
        return Err("You've left this game.".to_string());
    }
    if game.turn != seat {
        return Err("⏳ It isn't your turn yet.".to_string());
    }
    if out_of_time(&game, now) {
        return Err("⌛ Your time ran out on this one — the turn is passing.".to_string());
    }
    Ok((game, seat))
}

/// Plays tiles. The ONE door the web page and anything else go through, so
/// nothing can play a word the other couldn't.
pub fn play(user: u64, game_id: i64, placements: &[Placement]) -> Result<Took, String> {
    let bank = duel_words::bank().ok_or_else(|| NOT_READY.to_string())?;
    let db = store::db().ok_or_else(|| NOT_READY.to_string())?;
    let now = Utc::now().timestamp();
    let landed = {
        let conn = db.lock();
        let (game, seat) = my_turn(&conn, user, game_id, now)?;
        let rack = game.player(seat).map(|p| p.rack.clone()).unwrap_or_default();
        let played = rules::check(&game.board, &rack, placements, |w| bank.knows(w)).map_err(|why: Refusal| why.words())?;
        // The tiles that go down are replaced from the bag, as many as there
        // are to be had.
        let (drawn, bag) = rules::draw(&game.bag, placements.len());
        let fresh = format!("{}{}", played.rack_left, drawn);
        let next = game.next_seat(seat).unwrap_or(seat);
        let squares = played.covered.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(",");
        let written = store::take_turn(
            &conn,
            Written {
                game: game.id,
                expect: game.turns,
                seat,
                kind: "word",
                word: &played.headline(),
                score: played.score,
                squares,
                board: &played.board,
                bag: &bag,
                rack: &fresh,
                next,
                passes: 0,
                now,
            },
        )
        .unwrap_or(false);
        if !written {
            return Err("That play crossed with another. Look at the board and try again.".to_string());
        }
        let bonus = if played.bingo { " — **all seven tiles**, fifty on top!" } else { "" };
        // A play that made several words says what each of them was worth; one
        // that made only the one has already said so.
        let made = if played.words.len() > 1 {
            format!("\n-# {}", played.words.iter().map(|w| format!("{} {}", w.word, w.score)).collect::<Vec<_>>().join(" · "))
        } else {
            String::new()
        };
        Took {
            game_id,
            words: format!("✅ **{}** for **{}**{}{}", played.headline(), played.score, bonus, made),
        }
    };
    // The database lock goes before anything else is touched.
    settle_if_over(game_id);
    nudge_task();
    Ok(landed)
}

/// Passes. An exchange and a missed turn go through the same counter, because
/// the game ends when nothing has scored for two turns each.
pub fn pass(user: u64, game_id: i64) -> Result<Took, String> {
    let db = store::db().ok_or_else(|| NOT_READY.to_string())?;
    let now = Utc::now().timestamp();
    {
        let conn = db.lock();
        let (game, seat) = my_turn(&conn, user, game_id, now)?;
        if !write_pass(&conn, &game, seat, "pass", now) {
            return Err("That crossed with another turn. Look at the board and try again.".to_string());
        }
    }
    settle_if_over(game_id);
    nudge_task();
    Ok(Took { game_id, words: "➖ You passed. It's the next player's turn.".to_string() })
}

/// Writes a pass, a missed turn or the scoreless half of an exchange.
fn write_pass(conn: &rusqlite::Connection, game: &Game, seat: usize, kind: &str, now: i64) -> bool {
    let rack = game.player(seat).map(|p| p.rack.clone()).unwrap_or_default();
    let next = game.next_seat(seat).unwrap_or(seat);
    store::take_turn(
        conn,
        Written {
            game: game.id,
            expect: game.turns,
            seat,
            kind,
            word: "",
            score: 0,
            squares: String::new(),
            board: &game.board,
            bag: &game.bag,
            rack: &rack,
            next,
            passes: game.passes + 1,
            now,
        },
    )
    .unwrap_or(false)
}

/// How many tiles must still be in the bag for an exchange to be allowed.
pub const EXCHANGE_FLOOR: usize = rules::RACK;

/// Swaps tiles for fresh ones, while the bag is full enough.
pub fn exchange(user: u64, game_id: i64, tiles: &str) -> Result<Took, String> {
    let db = store::db().ok_or_else(|| NOT_READY.to_string())?;
    let now = Utc::now().timestamp();
    let count;
    {
        let conn = db.lock();
        let (game, seat) = my_turn(&conn, user, game_id, now)?;
        if game.bag_left() < EXCHANGE_FLOOR {
            return Err(format!(
                "There are only {} left in the bag, and swapping needs {}. Play something or pass.",
                plural(game.bag_left() as i64, "tile", "tiles"),
                EXCHANGE_FLOOR
            ));
        }
        let rack = game.player(seat).map(|p| p.rack.clone()).unwrap_or_default();
        let wanted: Vec<char> = tiles.chars().collect();
        if wanted.is_empty() {
            return Err("Pick the tiles you want to swap first.".to_string());
        }
        if wanted.len() > rules::RACK {
            return Err("You can't swap more tiles than a rack holds.".to_string());
        }
        // The tiles have to be on the rack, and each one only once.
        let mut left: Vec<char> = rack.chars().collect();
        for tile in &wanted {
            match left.iter().position(|c| c == tile) {
                Some(i) => {
                    left.remove(i);
                }
                None => return Err(Refusal::NotYours(*tile).words()),
            }
        }
        let mut rng = Rng::fresh();
        let (drawn, rest) = rules::draw(&game.bag, wanted.len());
        let bag = rules::put_back(&rest, &wanted.iter().collect::<String>(), &mut rng);
        let fresh = format!("{}{}", left.into_iter().collect::<String>(), drawn);
        let next = game.next_seat(seat).unwrap_or(seat);
        let written = store::take_turn(
            &conn,
            Written {
                game: game.id,
                expect: game.turns,
                seat,
                kind: "exchange",
                word: "",
                score: wanted.len() as i64,
                squares: String::new(),
                board: &game.board,
                bag: &bag,
                rack: &fresh,
                next,
                passes: game.passes + 1,
                now,
            },
        )
        .unwrap_or(false);
        if !written {
            return Err("That crossed with another turn. Look at the board and try again.".to_string());
        }
        count = wanted.len();
    }
    settle_if_over(game_id);
    nudge_task();
    Ok(Took { game_id, words: format!("♻️ Swapped **{}** for fresh ones.", plural(count as i64, "tile", "tiles")) })
}

/// Leaves a game. The tiles go back in the bag so the rest are not short.
pub fn leave(user: u64, game_id: i64) -> Result<Took, String> {
    let db = store::db().ok_or_else(|| NOT_READY.to_string())?;
    let now = Utc::now().timestamp();
    {
        let conn = db.lock();
        let game = store::get_game(&conn, game_id).ok_or_else(|| "That game is gone.".to_string())?;
        if !game.running() {
            return Err("That game has finished.".to_string());
        }
        let seat = game.seat_of(user).ok_or_else(|| NOT_YOURS.to_string())?;
        drop_seat(&conn, &game, seat, now)?;
    }
    settle_if_over(game_id);
    nudge_task();
    Ok(Took { game_id, words: "🚪 You've left the game. Your tiles went back in the bag.".to_string() })
}

/// Takes a seat out of a game and moves the turn on if it was theirs.
fn drop_seat(conn: &rusqlite::Connection, game: &Game, seat: usize, now: i64) -> Result<(), String> {
    let rack = game.player(seat).map(|p| p.rack.clone()).unwrap_or_default();
    let mut rng = Rng::fresh();
    let bag = rules::put_back(&game.bag, &rack, &mut rng);
    if !store::drop_player(conn, game.id, seat, &bag).unwrap_or(false) {
        return Err("You've already left this game.".to_string());
    }
    if game.turn == seat {
        // Read it back so the seat that just went is stepped over.
        if let Some(after) = store::get_game(conn, game.id) {
            if let Some(next) = after.next_seat(seat) {
                let _ = store::hand_over(conn, game.id, next, now);
            }
        }
    }
    Ok(())
}

// --- ending a game ---------------------------------------------------------------------------

/// Whether a game is over, and why. Read from the position alone, so it gives
/// the same answer whoever asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Over {
    /// Somebody used their last tile with the bag empty.
    WentOut(usize),
    /// Everybody passed twice in a row.
    PassedOut,
    /// Fewer than two people are left.
    Walkover,
}

impl Over {
    pub fn key(self) -> &'static str {
        match self {
            Over::WentOut(_) => "played out",
            Over::PassedOut => "passed out",
            Over::Walkover => "walkover",
        }
    }

    pub fn went_out(self) -> Option<usize> {
        match self {
            Over::WentOut(seat) => Some(seat),
            _ => None,
        }
    }
}

/// Whether a position has ended. `playing` is how many people are still in.
pub fn ending(bag_left: usize, racks: &[(usize, String, bool)], passes: i64, players: usize) -> Option<Over> {
    let playing: Vec<&(usize, String, bool)> = racks.iter().filter(|(_, _, dropped)| !dropped).collect();
    if playing.len() < 2 {
        return Some(Over::Walkover);
    }
    if bag_left == 0 {
        if let Some((seat, _, _)) = playing.iter().find(|(_, rack, _)| rack.is_empty()) {
            return Some(Over::WentOut(*seat));
        }
    }
    if rules::passed_out(passes, players) {
        return Some(Over::PassedOut);
    }
    None
}

/// Ends a game if the position says it is over. Safe to call after every turn:
/// only the first call finds it running.
fn settle_if_over(game_id: i64) -> bool {
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else { return false };
    if !game.running() {
        return false;
    }
    let racks: Vec<(usize, String, bool)> = game.players.iter().map(|p| (p.seat, p.rack.clone(), p.dropped)).collect();
    let Some(over) = ending(game.bag_left(), &racks, game.passes, game.players.len()) else { return false };
    finish(game_id, over.key(), over.went_out())
}

/// Ends a game in the database and throws its board links away. The task pays
/// it and posts the result a moment later, so every way of ending a game ends
/// it the same way and only one result can ever be posted.
pub fn finish(game_id: i64, result: &str, went_out: Option<usize>) -> bool {
    let now = Utc::now().timestamp();
    let finished =
        with_db(|conn| store::finish_game(conn, game_id, result, went_out, now)).and_then(|r| r.ok()).unwrap_or(false);
    if !finished {
        return false;
    }
    let _ = with_db(|conn| store::drop_tokens(conn, game_id));
    tracing::info!("duel: game {} ended ({}, went out {:?})", game_id, result, went_out);
    nudge_task();
    true
}

/// Pays and announces every game that has ended but not been settled.
async fn settle_finished(ctx: &Context) {
    for game in with_db(store::unfinished_business).unwrap_or_default() {
        pay_game(game.id);
        post_result(ctx, game.id).await;
    }
}

/// Writes a finished game's points to the ledger, and what each player was
/// worth before the daily limit — their duel points, which nothing caps.
/// Safe to repeat: every ledger row carries a key naming the game and the
/// player, and the game is marked settled at the end.
fn pay_game(game_id: i64) {
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else { return };
    if game.paid_out {
        return;
    }
    // The tiles left over first: they change who won.
    let racks: Vec<&str> = game.players.iter().map(|p| p.rack.as_str()).collect();
    let went_out = game.went_out.filter(|_| game.result.as_deref() == Some("played out"));
    let cancelled = game.result.as_deref() == Some("cancelled");
    let sums = if cancelled { vec![0; game.players.len()] } else { rules::adjustments(&racks, went_out) };
    for (p, adjust) in game.players.iter().zip(&sums) {
        let _ = with_db(|conn| store::set_adjust(conn, game.id, p.seat, *adjust));
    }
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else { return };
    let totals: Vec<i64> = game.players.iter().map(|p| p.total()).collect();
    let places = rules::placings(&totals);
    let prizes = prizes();
    let mut paid_anything = false;
    let mut refused = false;
    for (i, p) in game.players.iter().enumerate() {
        let worth = if cancelled {
            0
        } else {
            rules::prize(places[i], !p.dropped, game.players.len(), prizes.win, prizes.second, prizes.played)
        };
        let mut points = 0;
        if worth > 0 {
            let key = format!("duel:{}:{}", game.id, p.user);
            let reason = format!("letter duel: game {}", game.id);
            match super::house::award_person(p.user, Source::Duel, worth, &reason, None, Some(key.clone()), None) {
                Some((_, Outcome::Granted(n))) => points = n,
                Some((_, Outcome::Duplicate)) => points = ledger_points(&key).unwrap_or(0),
                _ => refused = true,
            }
            if points > 0 {
                paid_anything = true;
            }
        }
        let _ = with_db(|conn| store::record_points(conn, game.id, p.seat, points, worth));
    }
    let why = if cancelled {
        "a mod stopped the game"
    } else if game.players.len() < 3 {
        "a two-player game only pays its winner, so nobody can farm points"
    } else if !paid_anything && refused {
        "nobody here can earn house points from this one right now"
    } else if !paid_anything {
        "nobody finished in a paying place"
    } else {
        ""
    };
    let _ = with_db(|conn| store::record_payout(conn, game.id, why));
}

/// Points already in the ledger under a key, for a payment played again.
fn ledger_points(key: &str) -> Option<i64> {
    let db = super::house::db()?;
    let points = db.lock().query_row("SELECT points FROM ledger WHERE dedupe = ?1", rusqlite::params![key], |r| r.get(0)).ok();
    points
}

async fn post_result(ctx: &Context, game_id: i64) {
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else { return };
    let now = Utc::now().timestamp();
    let totals: Vec<i64> = game.players.iter().map(|p| p.total()).collect();
    let places = rules::placings(&totals);
    let (title, body) = result_text(&game, &places, game.finished_at.unwrap_or(now) - game.started_at, &game.why_nothing);
    let mut message = CreateMessage::new()
        .embed(CreateEmbed::new().title(title).description(body).colour(RESULT_COLOUR).image(format!("attachment://{}", BOARD_FILE)))
        .allowed_mentions(CreateAllowedMentions::new());
    if let Some(file) = board_attachment(View::new(&game.board, &[])).await {
        message = message.add_file(file);
    }
    // The game's own card goes — and ONLY if it is still the card on screen,
    // so a lobby that is already up is never taken down by an old result.
    drop_card_of(ctx, game.message).await;
    let _ = with_db(|conn| store::set_message(conn, game.id, None));
    if let Err(err) = call(ChannelId::new(game.channel).send_message(&ctx.http, message)).await {
        tracing::warn!("duel: result for game {} not posted: {}", game.id, err);
    }
    nudge_task();
}

// --- the clock -------------------------------------------------------------------------------

/// Passes the turn of anybody whose clock has run out, and takes out anybody
/// who has let it happen [`MAX_MISSES`] times.
pub fn run_clocks(now: i64, woke_at: i64) {
    if !may_flag(now, woke_at, GRACE_SECS) {
        return;
    }
    for game in with_db(store::running_games).unwrap_or_default() {
        if !out_of_time(&game, now) {
            continue;
        }
        let seat = game.turn;
        let Some(player) = game.player(seat).filter(|p| !p.dropped) else {
            // Nobody is sitting there; move on so the game cannot stick.
            if let Some(next) = game.next_seat(seat) {
                let _ = with_db(|conn| store::hand_over(conn, game.id, next, now));
            } else {
                finish(game.id, "walkover", None);
            }
            continue;
        };
        let passed = with_db(|conn| write_pass(conn, &game, seat, "missed", now)).unwrap_or(false);
        if !passed {
            continue;
        }
        let misses = with_db(|conn| store::miss_turn(conn, game.id, seat)).and_then(|r| r.ok()).unwrap_or(0);
        tracing::info!("duel: game {} — {} missed a turn ({})", game.id, player.user, misses);
        if misses >= MAX_MISSES {
            let Some(after) = with_db(|conn| store::get_game(conn, game.id)).flatten() else { continue };
            let _ = with_db(|conn| drop_seat(conn, &after, seat, now));
            tracing::info!("duel: game {} — {} dropped after {} missed turns", game.id, player.user, misses);
        }
        settle_if_over(game.id);
        nudge_task();
    }
}

// --- commands -------------------------------------------------------------------------------------

pub fn command() -> CreateCommand {
    CreateCommand::new("duel").description("open a Letter Duel lobby - a game of tiles for two to four players")
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("duelhelp").description("how Letter Duel works: the board, the words, the clock and the points")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("dueltop")
        .description("the duel points board, today or this month")
        .add_option(
            CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "which days to count")
                .add_string_choice("Today", "today")
                .add_string_choice("This month", "month"),
        )
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("duelstop").description("admin only: stop the Letter Duel game or lobby that is running, with no points")
}

/// `/duel` — everyone.
pub async fn command_handler(ctx: &Context, command: &CommandInteraction) {
    let text = match open_lobby(ctx, command.user.id.get()).await {
        Ok(said) => said,
        Err(why) => why,
    };
    let _ = command.create_response(&ctx.http, whisper_command(text)).await;
}

/// The rules as a card, for both `/duelhelp` and the ❓ button.
fn help_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title("🔠 How Letter Duel works")
        .description(help_text(min_players(), max_players(), lobby_secs(), turn_secs(), prizes()))
        .colour(COLOUR)
}

/// The rules in words, written from the settings as they are now.
pub fn help_text(min: usize, max: usize, lobby: i64, turn: i64, p: Prizes) -> String {
    format!(
        "**Getting in.** `/duel`, or **⚔️ Start a duel** on the card, puts a lobby up for **{}**. Press **⚔️ Join** \
         to take a seat — **{}** to **{}** players. It starts early when it fills, and is called off with nothing \
         lost if too few turn up.\n\n\
         **Your tiles.** Press **🔤 Open my rack** for a page of your own: your seven tiles along the bottom, tap a \
         tile then a square to put it down, tap it again to pick it up. The page adds up what the play would score \
         BEFORE you commit it, and says in plain words if it can't be played. Nobody else ever sees your rack.\n\n\
         **The words.** The first word has to cross the middle ★. After that every play touches what is already \
         there, runs in one row or one column with no gaps, and EVERY word it makes — sideways ones too — has to be \
         in the dictionary.\n\n\
         **The scoring.** Premium squares multiply the letter first (**DL**, **TL**) and then the whole word \
         (**DW**, **TW**), and each one only counts the turn it is covered. Using all **seven** tiles in one play is \
         **+50**. A blank can be any letter and is worth nothing for ever after.\n\n\
         **Your turn.** **{}** each. Play, **swap** tiles while there are still {} in the bag, or **pass**. A turn \
         you let run out is a pass, and **{}** missed turns take you out of the game.\n\n\
         **The end.** The bag empties and somebody puts their last tile down: they gain what everybody else is still \
         holding, and everybody else loses theirs. Or everybody passes twice in a row, and each player loses what \
         they are holding.\n\n{}\n-# `/dueltop` is the duel points board — those have no daily limit, and everybody \
         scores them whatever their house.",
        clock_words(lobby),
        min,
        max,
        clock_words(turn),
        EXCHANGE_FLOOR,
        MAX_MISSES,
        worth_words(p)
    )
}

/// `/duelhelp` — everyone, shown only to them.
pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    let message =
        CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// How many names `/dueltop` lists.
const TOP_LIST: usize = 10;

/// Where somebody stands on a ranked board, counting from one.
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

/// What `/dueltop` says.
pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("🔠 **Duel points** · {}", period);
    if rows.is_empty() {
        text.push_str("\nNobody has finished a game yet. `/duel` starts one.");
        return text;
    }
    for (i, row) in rows.iter().take(TOP_LIST).enumerate() {
        text.push_str(&format!(
            "\n{} <@{}> {} **{}** · {} · best **{}**",
            medal(i + 1),
            row.user,
            crest(row.user),
            plural(row.points, "point", "points"),
            plural(row.games, "game", "games"),
            row.best
        ));
    }
    match place_of(rows, me) {
        Some(place) if place > TOP_LIST => {
            let mine = &rows[place - 1];
            text.push_str(&format!(
                "\n-# **You:** {} of {} · {} · {}",
                ordinal(place),
                rows.len(),
                plural(mine.points, "point", "points"),
                plural(mine.games, "game", "games")
            ));
        }
        None => text.push_str("\n-# You haven't finished a game in this stretch yet."),
        _ => {}
    }
    text.push_str("\n-# Duel points have no daily limit, and everyone scores them — mods and Muggles too.");
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

/// `/dueltop [period]` — everyone, shown only to them.
pub async fn top_command(ctx: &Context, command: &CommandInteraction) {
    let month = command.data.options.iter().any(|o| {
        o.name == "period" && matches!(&o.value, serenity::all::CommandDataOptionValue::String(v) if v == "month")
    });
    let day = super::points::ist_day(Utc::now().timestamp());
    let (rows, period) = match month {
        true => {
            let (from, to) = month_ends(&day);
            (with_db(|conn| store::tally_between(conn, &from, &to)).unwrap_or_default(), month_label(&day))
        }
        false => (with_db(|conn| store::day_tally(conn, &day)).unwrap_or_default(), "today".to_string()),
    };
    let text = top_text(&period, &rows, command.user.id.get());
    let _ = command.create_response(&ctx.http, whisper_command(text)).await;
}

/// `/duelstop` — admins only.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper_command("Only mods can stop a duel.")).await;
        return;
    }
    let Some(channel) = channel_setting() else {
        let _ = command.create_response(&ctx.http, whisper_command("Letter Duel has no channel set.")).await;
        return;
    };
    let what = with_db(|conn| (store::live_game(conn, channel).map(|g| g.id), store::live_lobby(conn, channel).map(|l| l.id)));
    let Some((game, lobby)) = what else {
        let _ = command.create_response(&ctx.http, whisper_command(NOT_READY)).await;
        return;
    };
    let text = match (game, lobby) {
        (Some(id), _) => {
            finish(id, "cancelled", None);
            tracing::info!("duel: /duelstop cancelled game {} (by {})", id, command.user.id);
            format!("🛑 Game #{} stopped. No points for anyone.", id)
        }
        (None, Some(id)) => {
            let _ = with_db(|conn| store::close_lobby(conn, id, LobbyStatus::CalledOff, None));
            nudge_task();
            tracing::info!("duel: /duelstop closed lobby {} (by {})", id, command.user.id);
            "🛑 Lobby closed. Nobody has lost anything.".to_string()
        }
        (None, None) => "Nothing is running right now.".to_string(),
    };
    let _ = command.create_response(&ctx.http, whisper_command(text)).await;
}

// --- buttons -------------------------------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let rest = |prefix: &str| id.strip_prefix(prefix).and_then(|r| r.parse::<i64>().ok());
    if id == OPEN_ID {
        let text = match open_lobby(ctx, component.user.id.get()).await {
            Ok(said) => said,
            Err(why) => why,
        };
        whisper(ctx, component, text).await;
    } else if id == HELP_ID {
        let message = CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true);
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
    } else if let Some(lobby) = rest("dueljoin:") {
        join_pressed(ctx, component, lobby).await;
    } else if let Some(lobby) = rest("duelleavelobby:") {
        leave_lobby_pressed(ctx, component, lobby).await;
    } else if let Some(game) = rest("duelopenrack:") {
        open_rack(ctx, component, game).await;
    } else if let Some(game) = rest("duelpass:") {
        let text = match pass(component.user.id.get(), game) {
            Ok(took) => took.words,
            Err(why) => why,
        };
        whisper(ctx, component, text).await;
    } else if let Some(game) = rest("duelquit:") {
        let text = match leave(component.user.id.get(), game) {
            Ok(took) => took.words,
            Err(why) => why,
        };
        whisper(ctx, component, text).await;
    }
}

async fn join_pressed(ctx: &Context, component: &ComponentInteraction, lobby_id: i64) {
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(lobby) = with_db(|conn| store::get_lobby(conn, lobby_id)).flatten() else {
        return whisper(ctx, component, "That lobby is gone.").await;
    };
    if lobby.status != LobbyStatus::Open {
        return whisper(ctx, component, "That lobby has already closed.").await;
    }
    if lobby.has(user) {
        return whisper(ctx, component, "You're already in — sit tight.").await;
    }
    let joined =
        with_db(|conn| store::join_lobby(conn, lobby_id, user, &house_key(user), now, max_players())).and_then(|r| r.ok()).unwrap_or(false);
    let text = if joined {
        nudge_task();
        format!("⚔️ You're in. **{}/{}** seats taken.", lobby.players() + 1, max_players())
    } else {
        "That lobby is full. The next one goes up when this game finishes.".to_string()
    };
    whisper(ctx, component, text).await;
}

async fn leave_lobby_pressed(ctx: &Context, component: &ComponentInteraction, lobby_id: i64) {
    let user = component.user.id.get();
    let left = with_db(|conn| store::leave_lobby(conn, lobby_id, user)).and_then(|r| r.ok()).unwrap_or(false);
    if left {
        nudge_task();
    }
    let text = if left { "🚪 You've given your seat up." } else { "You weren't in that lobby." };
    whisper(ctx, component, text).await;
}

/// The private board address for one player in one game.
pub fn rack_link(game_id: i64, user: u64) -> Option<String> {
    let base = control::web::panel_url()?;
    let now = Utc::now().timestamp();
    let token = with_db(|conn| store::token_for(conn, game_id, user, now, || rand::random::<f64>())).flatten()?;
    Some(format!("{}/duel/{}/{}", base, game_id, token))
}

async fn open_rack(ctx: &Context, component: &ComponentInteraction, game_id: i64) {
    let user = component.user.id.get();
    let Some(game) = with_db(|conn| store::get_game(conn, game_id)).flatten() else {
        return whisper(ctx, component, "That game is gone.").await;
    };
    if !game.has(user) {
        return whisper(ctx, component, NOT_YOURS).await;
    }
    if !game.running() {
        return whisper(ctx, component, "That game has finished.").await;
    }
    match rack_link(game.id, user) {
        None => {
            whisper(ctx, component, "The board page has no address yet — a mod needs to set the panel's web address.").await
        }
        Some(link) => {
            let text = format!(
                "🔤 **Your rack · Game #{}**\nTap a tile, tap a square. The page shows what the play would score before you commit it.\n-# 🔒 Only you can see this link — it works for your tiles alone.",
                game.id
            );
            let message = CreateInteractionResponseMessage::new()
                .content(text)
                .components(vec![CreateActionRow::Buttons(vec![CreateButton::new_link(link).label("Open my rack")])])
                .ephemeral(true)
                .allowed_mentions(CreateAllowedMentions::new());
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
        }
    }
}

// --- the task --------------------------------------------------------------------------------------

/// Starts the duel task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting(), duel_words::bank().is_some()) {
        (true, Some(c), true) => tracing::info!("duel: playing in {}", c),
        (true, Some(_), false) => tracing::warn!("duel: on, but there is no wordbank/dictionary.txt to play with"),
        (true, None, _) => tracing::info!("duel: on, but VIZIER_DUEL_CHANNEL points at the safe corner"),
        (false, _, _) => tracing::info!("duel: VIZIER_DUEL is off"),
    }
    tokio::spawn(run(ctx));
}

/// How much time a restart owes the clocks: the gap since the bot last wrote
/// that it was here. A blink is not worth giving back, and a gap of days is not
/// believed at all.
pub fn forgiveness(last_seen: Option<i64>, now: i64, min: i64, max: i64) -> Option<i64> {
    let gap = now - last_seen?;
    (gap >= min && gap <= max).then_some(gap)
}

async fn recover(ctx: &Context) {
    let now = Utc::now().timestamp();
    WOKE_AT.store(now, Ordering::SeqCst);
    let last_seen = meta_get("heartbeat").and_then(|v| v.parse::<i64>().ok());
    match forgiveness(last_seen, now, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS) {
        Some(gap) => {
            let games = with_db(|conn| store::give_time_back(conn, gap)).and_then(|r| r.ok()).unwrap_or(0);
            let lobbies = with_db(|conn| store::give_lobbies_time_back(conn, gap)).and_then(|r| r.ok()).unwrap_or(0);
            tracing::info!("duel: down for {}s, so {} games and {} lobbies got that time back", gap, games, lobbies);
        }
        None => match last_seen.map(|seen| now - seen) {
            None => tracing::info!("duel: no heartbeat to compare against, so no time was given back"),
            Some(gap) if gap < MIN_DOWNTIME_SECS => tracing::info!("duel: away {}s, too short to be worth giving back", gap),
            Some(gap) => tracing::warn!("duel: the last heartbeat was {}s ago, more than is believable, so no time was given back", gap),
        },
    }
    settle_finished(ctx).await;
    let old_card = meta_get("card").and_then(|v| {
        let (c, m) = v.split_once(':')?;
        Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?))
    });
    if let Some(c) = active_channel(ctx) {
        // Read the channel so the card isn't moved the moment the bot is back.
        let _ = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await;
    }
    if let Some((c, m)) = old_card {
        delete(ctx, c, m).await;
        meta_set("card", "");
    }
    {
        let mut s = SHARED.lock();
        s.card = None;
        s.others_since_card = 0;
        s.last_bump_ms = Utc::now().timestamp_millis();
        s.dirty = true;
    }
}

async fn run(ctx: Context) {
    recover(&ctx).await;
    let mut last_beat = 0i64;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        beat(now, &mut last_beat);
        run_clocks(now, WOKE_AT.load(Ordering::SeqCst));
        settle_finished(&ctx).await;
        let Some(channel) = active_channel(&ctx) else {
            if SHARED.lock().card.is_some() {
                drop_card(&ctx).await;
            }
            continue;
        };
        // A lobby that has filled up or run out of time becomes a game.
        for lobby in with_db(store::open_lobbies).unwrap_or_default() {
            if lobby.players() >= max_players() || now >= lobby.closes_at {
                close_lobby(&ctx, &lobby).await;
            }
        }
        tend_card(&ctx, channel, now).await;
    }
}

/// Writes down that the bot is still here.
fn beat(now: i64, last_beat: &mut i64) {
    if now - *last_beat < HEARTBEAT_SECS {
        return;
    }
    *last_beat = now;
    meta_set("heartbeat", &now.to_string());
}

// --- what the web page is told ----------------------------------------------------------------

/// Everything the private page draws, for ONE player. Nobody else's rack is in
/// here, and neither is anything else of theirs but their name.
pub fn state_json(game: &Game, me: u64, name: impl Fn(u64) -> String, now: i64) -> Value {
    // Someone with no seat in this game is shown the board and nothing else.
    // This used to fall back to seat 0, which handed them the first player's
    // rack and told them it was their turn whenever it was seat 0's.
    let seat = game.seat_of(me);
    let mine = seat.and_then(|s| game.player(s));
    let turn = with_db(|conn| store::last_turn(conn, game.id)).flatten();
    let seats: Vec<Value> = game
        .players
        .iter()
        .map(|p| {
            json!({
                "seat": p.seat,
                "name": name(p.user),
                "crest": crest(p.user),
                "score": p.total(),
                "tiles": p.rack.chars().count(),
                "you": p.user == me,
                "to_play": game.turn == p.seat && !p.dropped,
                "dropped": p.dropped,
                "misses": p.misses,
            })
        })
        .collect();
    let premiums: Vec<&str> = (0..rules::SQUARES).map(|at| rules::premium(at).key()).collect();
    json!({
        "game": game.id,
        "board": game.board,
        "premiums": premiums,
        "size": rules::SIZE,
        "centre": rules::CENTRE,
        "rack": mine.map(|p| p.rack.clone()).unwrap_or_default(),
        "your_turn": seat == Some(game.turn) && mine.is_some_and(|p| !p.dropped),
        "dropped": mine.is_some_and(|p| p.dropped),
        "seats": seats,
        "bag": game.bag_left(),
        "can_exchange": game.bag_left() >= EXCHANGE_FLOOR,
        "exchange_floor": EXCHANGE_FLOOR,
        "seconds_left": seconds_left(game, now),
        "clock": clock_words(seconds_left(game, now)),
        "turn_secs": game.turn_secs,
        "turns": game.turns,
        "last": turn.as_ref().map(|t| json!({
            "who": game.player(t.seat).map(|p| name(p.user)).unwrap_or_default(),
            "kind": t.kind,
            "word": t.word,
            "score": t.score,
            "squares": t.covered(),
        })),
        "misses_allowed": MAX_MISSES,
        "running": game.running(),
    })
}

/// What a play WOULD score, without playing it — what the page shows before
/// anybody commits. The same code that plays it, so the number on the page and
/// the number on the card are the same number.
pub fn preview(game: &Game, me: u64, placements: &[Placement]) -> Value {
    let Some(bank) = duel_words::bank() else {
        return json!({"ok": false, "error": NOT_READY});
    };
    let seat = match game.seat_of(me) {
        Some(seat) => seat,
        None => return json!({"ok": false, "error": NOT_YOURS}),
    };
    let rack = game.player(seat).map(|p| p.rack.clone()).unwrap_or_default();
    match rules::check(&game.board, &rack, placements, |w| bank.knows(w)) {
        Err(why) => json!({"ok": false, "error": why.words()}),
        Ok(play) => json!({
            "ok": true,
            "score": play.score,
            "bingo": play.bingo,
            "words": play.words.iter().map(|w| json!({"word": w.word, "score": w.score})).collect::<Vec<_>>(),
        }),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::channels::discord::duel_store::tests::memory;

    pub fn prizes_fixture() -> Prizes {
        Prizes { win: 4, second: 2, played: 1, cap: Some(8) }
    }

    fn player(seat: usize, user: u64, score: i64) -> Player {
        Player {
            seat,
            user,
            house: "ravenclaw".into(),
            rack: "ABC".into(),
            score,
            adjust: 0,
            misses: 0,
            dropped: false,
            points: 0,
            worth: 0,
        }
    }

    fn game(players: Vec<Player>) -> Game {
        Game {
            id: 7,
            channel: 5,
            message: None,
            status: "running".into(),
            board: rules::empty_board(),
            bag: "A".repeat(30),
            turn: 0,
            passes: 0,
            turns: 4,
            turn_secs: 90,
            started_at: 1_000,
            turn_started_at: 1_200,
            finished_at: None,
            result: None,
            went_out: None,
            paid_out: false,
            why_nothing: String::new(),
            given_back: 0,
            day: "2026-09-17".into(),
            players,
        }
    }

    // --- the cards ----------------------------------------------------------------

    #[test]
    fn a_lobby_card_counts_the_seats_and_writes_the_countdown_out() {
        let text = lobby_text(&[111, 222], 1_400, 1_295, 2, 4, prizes_fixture());
        assert!(text.contains("**1:45**"), "the seconds are written out, not left to Discord: {}", text);
        assert!(text.contains("**2/4** in"), "{}", text);
        assert!(text.contains("enough to play"), "{}", text);
        assert!(text.contains("<@111>") && text.contains("<@222>"));
        assert!(text.contains("Winner **+4**") && text.contains("runner-up **+2**"));
        assert!(text.contains("max **8** a day"));

        let empty = lobby_text(&[], 1_320, 1_295, 2, 4, prizes_fixture());
        assert!(empty.contains("**25 s**"), "under a minute it counts in seconds: {}", empty);
        assert!(empty.contains("needs **2**") && empty.contains("Nobody yet"));
        // A countdown that has run out never goes negative.
        assert!(lobby_text(&[], 1_000, 9_999, 2, 4, prizes_fixture()).contains("**0 s**"));
    }

    #[test]
    fn a_closed_lobby_says_what_happened_and_that_nothing_was_lost() {
        assert!(lobby_closed_text(3, 2, Some(12)).contains("Game #12"));
        let off = lobby_closed_text(1, 2, None);
        assert!(off.contains("1 player") && off.contains("nothing is lost"), "{}", off);
    }

    #[test]
    fn the_game_card_says_what_was_played_the_scores_and_whose_turn_it_is() {
        let g = game(vec![player(0, 111, 48), player(1, 222, 62)]);
        let turn = store::Turn { seat: 1, kind: "word".into(), word: "QUARTZ".into(), score: 48, squares: "112,113".into(), ts: 1_200 };
        let text = game_text(&g, Some(&turn), 72, 1_260);
        assert!(text.contains("<@222>") && text.contains("**QUARTZ**") && text.contains("48 points"), "{}", text);
        assert!(text.contains("▶️"), "the player to move is marked: {}", text);
        assert!(text.contains("<@111> **48**") && text.contains("<@222> **62**"), "{}", text);
        assert!(text.contains("🎲 <@111>"), "the first seat is to play: {}", text);
        assert!(text.contains("1 m 12 s left") || text.contains("**1 m 12 s**"), "{}", text);
        assert!(text.contains("30 tiles left in the bag"), "{}", text);
        assert!(text.contains("turn 5"), "{}", text);

        // An empty board says what the first word has to do.
        let fresh = game(vec![player(0, 111, 0), player(1, 222, 0)]);
        assert!(game_text(&fresh, None, 90, 1_200).contains("cross the middle"));
    }

    #[test]
    fn every_kind_of_turn_reads_as_a_sentence() {
        let g = game(vec![player(0, 111, 0), player(1, 222, 0)]);
        let line = |kind: &str, word: &str, score: i64| {
            last_line(&g, Some(&store::Turn { seat: 0, kind: kind.into(), word: word.into(), score, squares: String::new(), ts: 0 }))
        };
        assert!(line("word", "CAT", 10).contains("**CAT**"));
        assert!(line("pass", "", 0).contains("passed"));
        assert!(line("exchange", "", 3).contains("swapped **3 tiles**"));
        assert!(line("missed", "", 0).contains("ran out of time"));
        assert!(line("dropped", "", 0).contains("has left the game"));
        assert!(last_line(&g, None).contains("first word"));
    }

    #[test]
    fn a_player_who_left_is_shown_as_having_left() {
        let mut g = game(vec![player(0, 111, 30), player(1, 222, 10)]);
        g.players[1].dropped = true;
        let line = scores_line(&g);
        assert!(line.contains("<@222> **10** *(left)*"), "{}", line);
        assert!(line.starts_with("▶️"), "the player to move is still marked: {}", line);
    }

    #[test]
    fn the_idle_card_invites_people_in() {
        let text = idle_text(2, 4, 90, prizes_fixture());
        assert!(text.contains("No game running") && text.contains("⚔️ Start a duel"));
        assert!(text.contains("**2** to **4** players"));
        assert!(text.contains("**1 m 30 s** a turn"), "{}", text);
    }

    // --- the result card ------------------------------------------------------------

    #[test]
    fn the_result_card_ranks_everyone_and_says_what_was_paid() {
        let mut g = game(vec![player(0, 111, 240), player(1, 222, 201), player(2, 333, 180)]);
        g.status = "over".into();
        g.result = Some("played out".into());
        g.players[0].adjust = 12;
        g.players[0].points = 4;
        g.players[0].worth = 4;
        g.players[1].adjust = -7;
        g.players[1].points = 2;
        g.players[1].worth = 2;
        g.players[2].adjust = -5;
        g.players[2].points = 0;
        g.players[2].worth = 1;
        let totals: Vec<i64> = g.players.iter().map(|p| p.total()).collect();
        let places = rules::placings(&totals);
        let (title, body) = result_text(&g, &places, 900, "");
        assert!(title.contains("Last tile down") && title.contains("#7"));
        assert!(body.contains("🥇 <@111>") && body.contains("**252**"), "{}", body);
        assert!(body.contains("🥈 <@222>") && body.contains("🥉 <@333>"), "{}", body);
        assert!(body.contains("+12 from the tiles left over"), "{}", body);
        assert!(body.contains("House points: <@111> **+4** · <@222> **+2**"), "{}", body);
        assert!(body.contains("Duel points:") && body.contains("<@333> **+1**"), "the uncapped tally is shown: {}", body);
    }

    #[test]
    fn a_game_that_paid_nothing_says_why() {
        let mut g = game(vec![player(0, 111, 100), player(1, 222, 90)]);
        g.status = "over".into();
        g.result = Some("passed out".into());
        g.players[0].worth = 2;
        let totals: Vec<i64> = g.players.iter().map(|p| p.total()).collect();
        let (_, body) = result_text(&g, &rules::placings(&totals), 300, "a two-player game only pays its winner, so nobody can farm points");
        assert!(body.contains("no House Cup points: a two-player game only pays its winner"), "{}", body);
        assert!(body.contains("Duel points: <@111> **+2**"), "{}", body);
    }

    // --- the clock -------------------------------------------------------------------

    #[test]
    fn a_turn_runs_out_when_its_time_is_up_and_not_before() {
        let g = game(vec![player(0, 111, 0), player(1, 222, 0)]);
        assert_eq!(seconds_left(&g, 1_200), 90);
        assert_eq!(seconds_left(&g, 1_260), 30);
        assert_eq!(seconds_left(&g, 9_999), 0, "it never runs backwards");
        assert!(!out_of_time(&g, 1_289));
        assert!(out_of_time(&g, 1_290));
        // Nothing is judged in the moments after the bot wakes up.
        assert!(!may_flag(1_000, 1_000, GRACE_SECS));
        assert!(!may_flag(1_029, 1_000, GRACE_SECS));
        assert!(may_flag(1_030, 1_000, GRACE_SECS));
    }

    #[test]
    fn a_restart_gives_back_what_it_was_away_and_never_more() {
        assert_eq!(forgiveness(Some(1_000), 1_600, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), Some(600));
        assert_eq!(forgiveness(Some(1_000), 1_005, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None, "a blink is not worth it");
        assert_eq!(forgiveness(Some(1_000), 1_000 + 7 * 3600, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None, "days off are not believed");
        assert_eq!(forgiveness(None, 1_600, MIN_DOWNTIME_SECS, MAX_DOWNTIME_SECS), None);
    }

    // --- how a game ends ---------------------------------------------------------------

    #[test]
    fn a_game_ends_when_somebody_goes_out_with_the_bag_empty() {
        let racks = |a: &str, b: &str| vec![(0, a.to_string(), false), (1, b.to_string(), false)];
        assert_eq!(ending(0, &racks("", "QI"), 0, 2), Some(Over::WentOut(0)));
        assert_eq!(ending(0, &racks("AB", ""), 0, 2), Some(Over::WentOut(1)));
        assert_eq!(ending(3, &racks("", "QI"), 0, 2), None, "not while there are still tiles to draw");
        assert_eq!(ending(0, &racks("AB", "QI"), 0, 2), None);
        assert_eq!(Over::WentOut(1).key(), "played out");
        assert_eq!(Over::WentOut(1).went_out(), Some(1));
    }

    #[test]
    fn a_game_ends_when_everybody_has_passed_twice() {
        let racks = vec![(0, "AB".to_string(), false), (1, "CD".to_string(), false)];
        assert_eq!(ending(30, &racks, 3, 2), None);
        assert_eq!(ending(30, &racks, 4, 2), Some(Over::PassedOut));
        let four: Vec<(usize, String, bool)> = (0..4).map(|s| (s, "AB".to_string(), false)).collect();
        assert_eq!(ending(30, &four, 7, 4), None, "four players need eight");
        assert_eq!(ending(30, &four, 8, 4), Some(Over::PassedOut));
        assert_eq!(Over::PassedOut.went_out(), None);
    }

    #[test]
    fn a_game_ends_when_everybody_else_has_left() {
        let racks = vec![(0, "AB".to_string(), false), (1, "CD".to_string(), true), (2, "EF".to_string(), true)];
        assert_eq!(ending(30, &racks, 0, 3), Some(Over::Walkover));
        let two_left = vec![(0, "AB".to_string(), false), (1, "CD".to_string(), false), (2, "EF".to_string(), true)];
        assert_eq!(ending(30, &two_left, 0, 3), None, "two is still a game");
    }

    // --- the card's one decision ---------------------------------------------------------

    #[test]
    fn a_missing_wrong_or_deleted_card_is_put_back_and_nothing_else_posts_one() {
        let plan = |has, right, gone, showing, wanted, dirty, stale, others, since| {
            card_plan(has, right, gone, showing, wanted, dirty, stale, others, since, 5, 60)
        };
        let idle = Bottom::Idle;
        assert_eq!(plan(false, true, false, idle, idle, false, false, 0, 0), CardAction::Bump, "no card at all");
        assert_eq!(plan(true, false, false, idle, idle, false, false, 0, 0), CardAction::Bump, "a card in the wrong channel");
        assert_eq!(plan(true, true, true, idle, idle, false, false, 0, 0), CardAction::Bump, "somebody deleted it");
        assert_eq!(plan(true, true, false, idle, Bottom::Game(1), false, false, 0, 0), CardAction::Bump, "it shows the wrong thing");
        assert_eq!(plan(true, true, false, idle, idle, true, false, 0, 0), CardAction::Edit, "its words changed");
        assert_eq!(plan(true, true, false, idle, idle, false, true, 0, 0), CardAction::Edit, "its clock has moved");
        assert_eq!(plan(true, true, false, idle, idle, false, false, 0, 0), CardAction::Nothing);
    }

    #[test]
    fn the_card_follows_the_chat_down_but_no_faster_than_the_cooldown() {
        // Both halves have to be met: enough messages AND enough time since the
        // card last MOVED. Pacing by moves rather than by every card posted is
        // the whole point — a game whose board changes every few seconds would
        // otherwise never be allowed to follow the conversation down.
        assert!(!bump_due(4, 999_000, 5, 60), "not enough messages yet");
        assert!(!bump_due(5, 30_000, 5, 60), "moved too recently");
        assert!(bump_due(5, 60_000, 5, 60));
        assert!(bump_due(9, 61_000, 5, 60));
        // A cooldown of zero means the message count alone decides.
        assert!(bump_due(5, 0, 5, 0));
        // And a count of zero still needs one message.
        assert!(!bump_due(0, 99_000, 0, 60));

        let game = Bottom::Game(3);
        assert_eq!(
            card_plan(true, true, false, game, game, false, false, 5, 60_000, 5, 60),
            CardAction::Bump,
            "chat has buried it"
        );
        assert_eq!(
            card_plan(true, true, false, game, game, true, false, 5, 10_000, 5, 60),
            CardAction::Edit,
            "buried but moved a moment ago: redraw it where it is"
        );
    }

    #[test]
    fn a_round_that_ends_never_takes_the_next_cards_message_away() {
        // The rule copied from the anagrams game, where a blind delete once
        // took the NEW card down and left the channel with nothing.
        let showing = Some((99u64, 1_000u64));
        assert_eq!(card_to_drop(showing, Some(1_000)), Some((99, 1_000)), "its own card, still on screen");
        assert_eq!(card_to_drop(showing, Some(900)), None, "an older card is not the one showing");
        assert_eq!(card_to_drop(showing, None), None, "a card that was never recorded takes nothing");
        assert_eq!(card_to_drop(None, Some(1_000)), None, "no card at all");
    }

    // --- opening a lobby ---------------------------------------------------------------

    #[test]
    fn a_lobby_only_opens_when_there_is_room_for_one() {
        assert_eq!(may_open(true, true, false, false), Ok(()));
        assert_eq!(may_open(false, true, false, false), Err(Refused::Off));
        assert_eq!(may_open(true, false, false, false), Err(Refused::NoWords));
        assert_eq!(may_open(true, true, true, false), Err(Refused::LobbyOpen));
        assert_eq!(may_open(true, true, false, true), Err(Refused::GameRunning));
        assert_eq!(may_open(true, true, true, true), Err(Refused::GameRunning), "the game is the more useful thing to say");
        for refusal in [Refused::Off, Refused::NoWords, Refused::LobbyOpen, Refused::GameRunning] {
            assert!(!refusal.words().is_empty());
        }
    }

    #[test]
    fn dealing_gives_everyone_seven_tiles_and_shuffles_who_goes_first() {
        let seats = vec![(1u64, "ravenclaw".to_string()), (2, String::new()), (3, "gryffindor".to_string())];
        let mut rng = Rng::seeded(11);
        let bag = rules::fresh_bag(&mut rng);
        let (dealt, left) = deal(&seats, bag.clone(), &mut rng);
        assert_eq!(dealt.len(), 3);
        for (_, _, rack) in &dealt {
            assert_eq!(rack.chars().count(), rules::RACK);
        }
        assert_eq!(left.chars().count(), 100 - 3 * rules::RACK);
        // Every tile is still accounted for.
        let mut all: Vec<char> = left.chars().collect();
        for (_, _, rack) in &dealt {
            all.extend(rack.chars());
        }
        all.sort_unstable();
        let mut original: Vec<char> = bag.chars().collect();
        original.sort_unstable();
        assert_eq!(all, original);
        // Somebody other than the first joiner leads at least sometimes.
        let firsts: Vec<u64> = (0..12)
            .map(|seed| {
                let mut rng = Rng::seeded(seed);
                deal(&seats, rules::fresh_bag(&mut rng), &mut rng).0[0].0
            })
            .collect();
        assert!(firsts.iter().any(|u| *u != 1), "the same person always went first: {:?}", firsts);
    }

    // --- the points board ---------------------------------------------------------------

    #[test]
    fn the_duel_points_board_lists_the_top_and_your_own_line() {
        let rows: Vec<store::Tally> =
            (1..=12).map(|i| store::Tally { user: i, points: (20 - i) as i64, games: 3, best: 100 + i as i64 }).collect();
        let text = top_text("today", &rows, 12);
        assert!(text.contains("🥇 <@1>") && text.contains("🥈 <@2>") && text.contains("🥉 <@3>"));
        assert!(text.contains("<@10>") && !text.contains("<@11>"), "only the top ten are listed: {}", text);
        assert!(text.contains("**You:** 12th of 12"), "{}", text);
        assert!(text.contains("no daily limit"));
        assert_eq!(place_of(&rows, 3), Some(3));
        assert_eq!(place_of(&rows, 99), None);
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
        assert!(top_text("today", &[], 1).contains("Nobody has finished a game yet"));
        // Somebody in the top ten gets no second line about themselves.
        assert!(!top_text("today", &rows, 2).contains("**You:**"));
    }

    #[test]
    fn a_month_is_a_pair_of_days_and_reads_as_a_month() {
        assert_eq!(month_ends("2026-09-17"), ("2026-09-01".into(), "2026-09-31".into()));
        assert_eq!(month_label("2026-09-17"), "September so far");
        assert_eq!(month_label("nonsense"), "this month");
    }

    // --- what the page is told -----------------------------------------------------------

    #[test]
    fn the_page_is_told_one_rack_and_only_one() {
        let mut g = game(vec![player(0, 111, 12), player(1, 222, 30)]);
        g.players[0].rack = "QUARTZS".into();
        g.players[1].rack = "SECRETS".into();
        let json = state_json(&g, 111, |u| format!("member {}", u), 1_260);
        assert_eq!(json["rack"], "QUARTZS");
        assert_eq!(json["your_turn"], true);
        let text = json.to_string();
        assert!(!text.contains("SECRETS"), "the other player's tiles must never be sent: {}", text);
        assert_eq!(json["seats"][1]["tiles"], 7, "only how many they hold");
        assert_eq!(json["seats"][1]["name"], "member 222");
        assert_eq!(json["bag"], 30);
        assert_eq!(json["can_exchange"], true);
        assert_eq!(json["seconds_left"], 30);
        assert_eq!(json["premiums"].as_array().map(|a| a.len()), Some(rules::SQUARES));
        assert_eq!(json["premiums"][rules::CENTRE], "centre");
        assert_eq!(json["premiums"][0], "tw");

        // The player who isn't to move is told so.
        let theirs = state_json(&g, 222, |u| format!("member {}", u), 1_260);
        assert_eq!(theirs["rack"], "SECRETS");
        assert_eq!(theirs["your_turn"], false);
        assert!(!theirs.to_string().contains("QUARTZS"));
    }

    #[test]
    fn a_short_bag_stops_anybody_swapping() {
        let mut g = game(vec![player(0, 111, 0), player(1, 222, 0)]);
        g.bag = "ABCDEF".into();
        let json = state_json(&g, 111, |u| u.to_string(), 1_200);
        assert_eq!(json["can_exchange"], false, "six tiles is not enough to swap");
        assert_eq!(json["exchange_floor"], 7);
        g.bag = "ABCDEFG".into();
        assert_eq!(state_json(&g, 111, |u| u.to_string(), 1_200)["can_exchange"], true);
    }

    // --- a whole game --------------------------------------------------------------------

    /// Plays a short game from the lobby to the result card, through the real
    /// store and the real rules, and checks what everyone finished on.
    #[test]
    fn a_whole_short_game_can_be_played_out() {
        let bank = crate::channels::discord::duel_words::tests::real_or_fixture();
        if !bank.knows("quartz") {
            return; // no dictionary in this checkout
        }
        let conn = memory();
        let square = |name: &str| {
            let col = name.as_bytes()[0] - b'A';
            let row: usize = name[1..].parse().expect("a row");
            (row - 1) * rules::SIZE + col as usize
        };
        // Two players, dealt the tiles the game below wants, and a bag with
        // just enough in it that it empties partway through.
        let seats = [
            (111u64, "ravenclaw".to_string(), "QUARTZS".to_string()),
            (222u64, "gryffindor".to_string(), "ANTEDIR".to_string()),
        ];
        let game = store::start_game(&conn, 5, &seats, &rules::empty_board(), "EIOPLMN", 90, 1_000, "2026-09-17").expect("a game");
        let known = |w: &str| bank.knows(w);

        // Seat 0 plays QUARTZ across the middle — six tiles, and six more come
        // out of the bag.
        let tiles: Vec<Placement> = "QUARTZ"
            .chars()
            .enumerate()
            .map(|(i, c)| Placement { at: square("F8") + i, letter: c, blank: false })
            .collect();
        let play = rules::check(&game.board, "QUARTZS", &tiles, known).expect("QUARTZ");
        // Q 10 + U 1 + A 1 + R 1 + T 1 + Z 10 = 24, and the star doubles it.
        assert_eq!(play.score, 48);
        let (drawn, bag) = rules::draw(&game.bag, tiles.len());
        assert!(
            store::take_turn(
                &conn,
                Written {
                    game: game.id,
                    expect: 0,
                    seat: 0,
                    kind: "word",
                    word: &play.headline(),
                    score: play.score,
                    squares: play.covered.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(","),
                    board: &play.board,
                    bag: &bag,
                    rack: &format!("{}{}", play.rack_left, drawn),
                    next: 1,
                    passes: 0,
                    now: 1_010,
                }
            )
            .unwrap()
        );
        let mid = store::get_game(&conn, game.id).expect("still there");
        assert_eq!(mid.player(0).map(|p| p.score), Some(48));
        assert_eq!(mid.player(0).map(|p| p.rack.as_str()), Some("SEIOPLM"), "one left over and six drawn");
        assert_eq!(mid.bag_left(), 1);

        // Seat 1 hangs ANTE down off the A.
        let tiles: Vec<Placement> = "NTE"
            .chars()
            .enumerate()
            .map(|(i, c)| Placement { at: square("H9") + i * rules::SIZE, letter: c, blank: false })
            .collect();
        let play = rules::check(&mid.board, "ANTEDIR", &tiles, known).expect("ANTE");
        assert_eq!(play.headline(), "ANTE");
        let (drawn, bag) = rules::draw(&mid.bag, tiles.len());
        assert_eq!(drawn.chars().count(), 1, "the bag had one tile left");
        assert!(
            store::take_turn(
                &conn,
                Written {
                    game: game.id,
                    expect: 1,
                    seat: 1,
                    kind: "word",
                    word: &play.headline(),
                    score: play.score,
                    squares: play.covered.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(","),
                    board: &play.board,
                    bag: &bag,
                    rack: &format!("{}{}", play.rack_left, drawn),
                    next: 0,
                    passes: 0,
                    now: 1_020,
                }
            )
            .unwrap()
        );
        let after = store::get_game(&conn, game.id).expect("still there");
        assert_eq!(after.bag_left(), 0, "the bag is empty now");
        assert_eq!(after.player(1).map(|p| p.rack.chars().count()), Some(5));

        // Seat 0 puts their last tiles down: SLIP hanging off nothing would be
        // adrift, so they pass, and so does seat 1, twice each.
        let mut expect = 2;
        for (seat, next, now) in [(0usize, 1usize, 1_030i64), (1, 0, 1_040), (0, 1, 1_050), (1, 0, 1_060)] {
            let g = store::get_game(&conn, game.id).expect("still there");
            assert!(
                store::take_turn(
                    &conn,
                    Written {
                        game: game.id,
                        expect,
                        seat,
                        kind: "pass",
                        word: "",
                        score: 0,
                        squares: String::new(),
                        board: &g.board,
                        bag: &g.bag,
                        rack: &g.player(seat).map(|p| p.rack.clone()).unwrap_or_default(),
                        next,
                        passes: g.passes + 1,
                        now,
                    }
                )
                .unwrap()
            );
            expect += 1;
        }
        let g = store::get_game(&conn, game.id).expect("still there");
        assert_eq!(g.passes, 4);
        let racks: Vec<(usize, String, bool)> = g.players.iter().map(|p| (p.seat, p.rack.clone(), p.dropped)).collect();
        assert_eq!(ending(g.bag_left(), &racks, g.passes, g.players.len()), Some(Over::PassedOut));
        assert!(store::finish_game(&conn, g.id, "passed out", None, 1_070).unwrap());

        // Everybody loses what they are still holding.
        let g = store::get_game(&conn, game.id).expect("still there");
        let racks: Vec<&str> = g.players.iter().map(|p| p.rack.as_str()).collect();
        let sums = rules::adjustments(&racks, None);
        assert!(sums.iter().all(|s| *s <= 0), "nobody gains when a game merely peters out: {:?}", sums);
        for (p, adjust) in g.players.iter().zip(&sums) {
            store::set_adjust(&conn, g.id, p.seat, *adjust).unwrap();
        }
        let g = store::get_game(&conn, game.id).expect("still there");
        let totals: Vec<i64> = g.players.iter().map(|p| p.total()).collect();
        let places = rules::placings(&totals);
        assert_eq!(places[0], 1, "48 for QUARTZ was always going to win it");

        // Two players, so the winner gets the runner-up's share and the loser
        // nothing at all: two friends cannot farm each other.
        let p = prizes_fixture();
        let worth: Vec<i64> = (0..2).map(|i| rules::prize(places[i], true, g.players.len(), p.win, p.second, p.played)).collect();
        assert_eq!(worth, vec![2, 0]);
        for (i, player) in g.players.iter().enumerate() {
            store::record_points(&conn, g.id, player.seat, worth[i], worth[i]).unwrap();
        }
        store::record_payout(&conn, g.id, "a two-player game only pays its winner, so nobody can farm points").unwrap();

        // And the whole thing reads as a result card.
        let g = store::get_game(&conn, game.id).expect("still there");
        let (title, body) = result_text(&g, &places, 70, &g.why_nothing);
        assert!(title.contains("Everybody passed"));
        assert!(body.contains("🥇 <@111>"), "{}", body);
        assert!(body.contains("Duel points: <@111> **+2**"), "{}", body);
        assert_eq!(store::day_tally(&conn, "2026-09-17").first().map(|t| (t.user, t.points)), Some((111, 2)));
    }
}
