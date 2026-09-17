//! Sudoku, in its own channel (`VIZIER_SUDOKU_CHANNEL`).
//!
//! There is ALWAYS a puzzle waiting. Its card is the last message in the
//! channel — members can't type there — and carries a picture of the grid and
//! four buttons: ▶️ Play, 📋 Submit code, 💡 Hint and 📊 Today's solvers. Play
//! hands out a private link to a page that plays the puzzle in a browser; the
//! page never sees the answer, it only turns a finished grid into a short code.
//! The first person to paste a correct code wins the puzzle's points, the card
//! turns into a "Solved!" card, and the next puzzle goes up at once.
//!
//! Sudoku keeps its OWN score. A solve is worth SUDOKU POINTS — the puzzle's
//! value with that player's hints taken off — and nothing else: no house points,
//! no daily limit, and the same score for everyone, houses or no houses. The
//! House Cup was being moved by puzzles a solver app had done, so the game was
//! taken out of it; every house point earned from sudoku before that stays
//! exactly where it is.
//!
//! Someone who was still working when the channel moved on isn't cut off: a
//! code carries the number of the puzzle it belongs to, so codes for the last
//! day's puzzles are still checked and still told "yes, that's right" — they
//! just count as a finish rather than a win, and score nothing.
//!
//! One task does all the posting ([`run`]), the way Name Place Animal Thing
//! does: the buttons only touch the database and some shared state, so two
//! presses landing together can never post two puzzles. The same task keeps the
//! card last in the channel and puts it back if someone deletes it.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{Value, json};
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions,
    CreateAttachment, CreateButton, CreateCommand, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditAttachments, EditMessage, GetMessages, Message, MessageId,
    ModalInteraction,
};

use super::control;
use super::points::Source;
use super::rules_text::{self, SudokuRules};
use super::sudoku_card;
use super::sudoku_code::{self, CodeError};
use super::sudoku_gen::{self as puzzles, Grid, Level, Puzzle};
use super::sudoku_store::{self as store, Kind, Status};

/// How often the task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How often the rules post is checked against the settings, in ticks.
const RULES_EVERY_TICKS: u64 = 60;
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// How long the card waits after the last message below it before moving down.
pub const MOVE_DELAY_MS: i64 = 3_000;
/// The card is redrawn at most this often while people join.
const EDIT_EVERY_SECS: i64 = 15;
/// How long between two wrong codes from the same person.
pub const COOLDOWN_SECS: i64 = 10;
/// Old puzzles kept checkable however long ago they were posted.
pub const KEEP_CHECKABLE: usize = 10;
/// Puzzles listed by `/sudoku`.
const MINE_LINES: usize = 8;

const COLOUR: u32 = 0x5865F2;
const SOLVED_COLOUR: u32 = 0x3BA55C;

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_SUDOKU", false)
}

fn rules_on() -> bool {
    control::on("VIZIER_SUDOKU_RULES", true)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_SUDOKU_CHANNEL").filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

/// The game's channel when the game is on.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on())
}

fn mix() -> [u32; 3] {
    puzzles::parse_mix(&control::var("VIZIER_SUDOKU_MIX").unwrap_or_else(|| puzzles::DEFAULT_MIX.to_string()))
}

/// What solving a puzzle of each level is worth right now.
pub fn level_points() -> [i64; 3] {
    [
        control::number("VIZIER_POINTS_SUDOKU_EASY", 2).min(100) as i64,
        control::number("VIZIER_POINTS_SUDOKU_MEDIUM", 4).min(100) as i64,
        control::number("VIZIER_POINTS_SUDOKU_HARD", 6).min(100) as i64,
    ]
}

fn points_for(level: Level) -> i64 {
    level_points()[level as usize]
}

fn hint_cost() -> i64 {
    control::number("VIZIER_SUDOKU_HINT_COST", 1).min(100) as i64
}

fn max_hints() -> i64 {
    control::number("VIZIER_SUDOKU_MAX_HINTS", 3).min(80) as i64
}

fn max_tries() -> i64 {
    control::number("VIZIER_SUDOKU_MAX_TRIES", 10).clamp(1, 100) as i64
}

fn late_hours() -> i64 {
    control::number("VIZIER_SUDOKU_LATE_HOURS", 24).clamp(1, 720) as i64
}

/// The page's address, when the panel has one. Read through the panel so there
/// is one place that knows where the bot lives.
fn page_base() -> Option<String> {
    control::web::panel_url()
}

/// The link a player is sent to.
pub fn page_link(base: &str, puzzle_id: i64) -> String {
    format!("{}/sudoku/{}", base.trim_end_matches('/'), puzzle_id)
}

/// Every setting the rules post, the guide and `/sudokuhelp` mention.
pub fn sudoku_rules() -> SudokuRules {
    SudokuRules {
        channel: live_channel(),
        points: level_points(),
        hint_cost: hint_cost(),
        max_hints: max_hints(),
        max_tries: max_tries(),
        late_hours: late_hours(),
        mix: mix(),
        has_page: page_base().is_some(),
    }
}

// --- words ---------------------------------------------------------------------------------

/// "48 s", "7 min 41 s", "1 h 04 min".
pub fn spent_words(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        s if s < 60 => format!("{} s", s),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// How long a puzzle has been up, in round words: "just now", "6 min", "2 h".
pub fn open_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "4 sudoku points", "1 sudoku point".
pub fn score_words(n: i64) -> String {
    plural(n, "sudoku point", "sudoku points")
}

/// What a puzzle is worth to someone who has taken hints, never below nothing.
pub fn worth_after_hints(points: i64, hints: i64, cost: i64) -> i64 {
    (points - hints.max(0) * cost.max(0)).max(0)
}

/// What a correct grid scores in SUDOKU points: a win, less what that player's
/// hints cost them; a finish is worth nothing at all, because somebody else had
/// already solved it. No cap of any kind touches this — sudoku points are the
/// game's own score and the House Cup is not in it.
pub fn payout(kind: Kind, points: i64, hints: i64, cost: i64) -> i64 {
    match kind {
        Kind::Win => worth_after_hints(points, hints, cost),
        Kind::Finish => 0,
    }
}

pub fn card_title(puzzle_id: i64) -> String {
    format!("🔢 Sudoku · Puzzle #{}", puzzle_id)
}

/// Everything the live card says about a puzzle right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Live {
    pub level: Level,
    pub points: i64,
    pub playing: i64,
    pub open_secs: i64,
    pub hint_cost: i64,
}

pub fn card_text(live: &Live) -> String {
    let playing = match live.playing {
        0 => "🧑‍💻 **nobody playing yet** — be first".to_string(),
        n => format!("🧑‍💻 **{}** playing", plural(n, "person", "people")),
    };
    let hint = if live.hint_cost > 0 {
        format!(" · 💡 a hint costs **{}**", score_words(live.hint_cost))
    } else {
        " · 💡 hints are free".to_string()
    };
    format!(
        "{} **{}** · worth **{}** · first to solve it wins\n{} · ⏱️ open **{}**{}",
        live.level.emoji(),
        live.level.name(),
        score_words(live.points),
        playing,
        open_words(live.open_secs),
        hint
    )
}

pub const CARD_FOOTER: &str = "As soon as someone solves it, the next puzzle appears · press ❓ How to play for the rules";

/// Everything the card says once a puzzle is solved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Solved {
    pub puzzle_id: i64,
    pub level: Level,
    pub winner: u64,
    /// The winner's crest and house, when they are in one. It says who they
    /// play for, not what the solve paid them: sudoku pays no house points.
    pub badge: String,
    pub seconds: i64,
    /// The SUDOKU points the solve scored, hints already taken off.
    pub points: i64,
    /// Hints the winner took, and what they cost them.
    pub hints: i64,
    pub full_points: i64,
    /// Hints everyone took on this puzzle.
    pub all_hints: i64,
    /// People who had started it, the winner not counted.
    pub others: i64,
    /// The winner's sudoku points today, this puzzle included.
    pub tally: i64,
    /// "Aarav 3 · Meera 2", the day so far.
    pub today: String,
}

pub fn solved_title(puzzle_id: i64, level: Level) -> String {
    format!("🎉 Solved! Puzzle #{} · {} {}", puzzle_id, level.emoji(), level.name())
}

/// The solved card's words. Sudoku points are the only score here — the House
/// Cup is not mentioned, because the game no longer moves it.
pub fn solved_text(s: &Solved) -> String {
    let badge = if s.badge.is_empty() { String::new() } else { format!(" {}", s.badge) };
    let scored = match s.points {
        0 => "**no sudoku points left after hints**".to_string(),
        n => format!("**+{}**", score_words(n)),
    };
    let mut text = format!("🧩 **solved by <@{}>**{} · {} · {} today", s.winner, badge, scored, s.tally);
    let mut notes = vec![format!("{} {} in {}", s.level.emoji(), s.level.name(), spent_words(s.seconds))];
    notes.push(match s.hints {
        0 => "no hints used".to_string(),
        n if s.points < s.full_points => format!("{} used, {} off", plural(n, "hint", "hints"), s.full_points - s.points),
        n => plural(n, "hint", "hints") + " used",
    });
    if s.all_hints > s.hints {
        notes.push(format!("{} in all", plural(s.all_hints, "hint", "hints")));
    }
    if s.others > 0 {
        notes.push(format!("{} were also playing", plural(s.others, "other", "others")));
    }
    text.push_str(&format!("\n-# {}", notes.join(" · ")));
    text
}

/// The day's solvers as the cards and the button show them: wins first, then
/// the people who finished a puzzle somebody else had already won. The number
/// in brackets is SUDOKU points, the same score the solved card names.
pub fn today_lines(solves: &[store::Solve], name: impl Fn(u64) -> String) -> (Vec<String>, Vec<String>) {
    let mut wins: Vec<(u64, i64, i64)> = Vec::new();
    let mut finishes: Vec<u64> = Vec::new();
    for s in solves {
        match s.kind {
            Kind::Win => match wins.iter_mut().find(|(u, _, _)| *u == s.user) {
                Some((_, count, points)) => {
                    *count += 1;
                    *points += s.worth;
                }
                None => wins.push((s.user, 1, s.worth)),
            },
            Kind::Finish => {
                if !finishes.contains(&s.user) {
                    finishes.push(s.user);
                }
            }
        }
    }
    wins.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)));
    let won: Vec<String> = wins.iter().map(|(u, count, points)| format!("{} {} (+{})", name(*u), count, points)).collect();
    let also: Vec<String> = finishes.into_iter().filter(|u| !wins.iter().any(|(w, _, _)| w == u)).map(name).collect();
    (won, also)
}

/// One line per solve for the 📊 button: who, which puzzle, how long, what it
/// scored.
pub fn solver_line(s: &store::Solve) -> String {
    let tail = match s.kind {
        Kind::Win => format!("**+{}**", s.worth),
        Kind::Finish => "finished after the win, so nothing scored".to_string(),
    };
    format!("<@{}> · #{} {} {} · {} · {}", s.user, s.puzzle, s.level.emoji(), s.level.name(), spent_words(s.seconds), tail)
}

// --- the card -------------------------------------------------------------------------------

/// The card's buttons. Members can't type in this channel - the message box
/// itself is denied them, so a slash command is not even an option there - so
/// everything the game can do has to be reachable by pressing something,
/// the rules included.
pub fn card_rows() -> Vec<CreateActionRow> {
    vec![
        CreateActionRow::Buttons(vec![
            CreateButton::new("sudokuplay").label("▶️ Play").style(ButtonStyle::Success),
            CreateButton::new("sudokucode").label("📋 Submit code").style(ButtonStyle::Primary),
            CreateButton::new("sudokuhint").label("💡 Hint").style(ButtonStyle::Secondary),
            CreateButton::new("sudokutoday").label("📊 Today's solvers").style(ButtonStyle::Secondary),
        ]),
        CreateActionRow::Buttons(vec![help_button()]),
    ]
}

/// The button that explains the game, for a channel where nobody can type
/// `/sudokuhelp`.
pub fn help_button() -> CreateButton {
    CreateButton::new(HELP_ID).label("❓ How to play").style(ButtonStyle::Secondary)
}

/// The button that opens the rules.
pub const HELP_ID: &str = "sudokuhelp";

fn live_of(row: &store::Row, playing: i64, now: i64) -> Live {
    Live { level: row.level, points: row.points, playing, open_secs: now - row.posted_ts, hint_cost: hint_cost() }
}

fn card_embed(row: &store::Row, playing: i64, now: i64) -> CreateEmbed {
    CreateEmbed::new()
        .title(card_title(row.id))
        .description(card_text(&live_of(row, playing, now)))
        .colour(row.level.colour())
        .image(format!("attachment://{}", sudoku_card::file_name(row.id)))
        .footer(CreateEmbedFooter::new(CARD_FOOTER))
}

// --- shared state ----------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    /// (channel, message) of the puzzle card.
    card: Option<(u64, u64)>,
    /// The newest message id seen in the channel.
    latest: u64,
    /// When a message last landed in the channel.
    last_seen_ms: i64,
    /// Someone deleted the card.
    card_gone: bool,
    /// The card's words are out of date (a new player, a hint).
    dirty: bool,
    /// A puzzle was just won: the task turns its card into the Solved card.
    solved: Option<i64>,
    /// A mod asked for a new puzzle.
    skip: bool,
    rules_message: Option<u64>,
    rules_check: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// Every message in the game's channel, the bot's own included.
pub fn note_message(_ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != channel_setting() {
        return;
    }
    let mut s = SHARED.lock();
    s.latest = s.latest.max(msg.id.get());
    s.last_seen_ms = Utc::now().timestamp_millis();
}

/// A deleted card is posted again; a deleted rules post too.
pub fn on_delete(channel: ChannelId, id: MessageId) {
    if Some(channel.get()) != channel_setting() {
        return;
    }
    let mut s = SHARED.lock();
    if s.card.map(|(_, m)| m) == Some(id.get()) {
        s.card_gone = true;
    }
    if s.rules_message == Some(id.get()) {
        s.rules_check = true;
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

async fn delete(ctx: &Context, channel: u64, message: u64) {
    if let Err(err) = call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        tracing::debug!("sudoku: message {} not deleted: {}", message, err);
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

fn meta_get(key: &str) -> Option<String> {
    store::db().and_then(|db| store::meta_get(&db.lock(), key)).filter(|v| !v.is_empty())
}

/// The house crest and name of whoever won, for the Solved card.
fn badge_of(user: u64) -> String {
    if super::house::opted_out(user) {
        return "🧙 Muggle".to_string();
    }
    super::house::house_of(user).map(|h| format!("{} {}", h.crest, h.name)).unwrap_or_default()
}

fn display_name(ctx: &Context, guild: Option<serenity::all::GuildId>, user: u64) -> String {
    guild
        .and_then(|g| ctx.cache.guild(g).and_then(|g| g.members.get(&serenity::all::UserId::new(user)).map(|m| m.display_name().to_string())))
        .unwrap_or_else(|| format!("member {}", user))
}

// --- posting -------------------------------------------------------------------------------------

/// Posts a card and remembers it. `replace` takes the old card away (a solved
/// card stays, as the channel's history).
async fn place_card(ctx: &Context, channel: u64, message: CreateMessage, replace: bool) -> Option<u64> {
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let id = posted.id.get();
            let old = {
                let mut s = SHARED.lock();
                s.latest = s.latest.max(id);
                s.dirty = false;
                s.card.replace((channel, id))
            };
            meta_set("card", &format!("{}:{}", channel, id));
            if let Some((c, m)) = old.filter(|(_, m)| *m != id && replace) {
                delete(ctx, c, m).await;
            }
            Some(id)
        }
        Err(err) => {
            tracing::warn!("sudoku: card not posted in {}: {}", channel, err);
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

/// Builds the message for a live puzzle, picture and all.
async fn card_message(row: &store::Row, playing: i64, now: i64) -> CreateMessage {
    let message = CreateMessage::new().embed(card_embed(row, playing, now)).components(card_rows()).allowed_mentions(CreateAllowedMentions::new());
    match sudoku_card::png(row.id, row.givens).await {
        Some(png) => message.add_file(CreateAttachment::bytes(png.to_vec(), sudoku_card::file_name(row.id))),
        None => message,
    }
}

/// Makes a puzzle and puts it up. The making is real work, so it happens off
/// the gateway thread.
async fn post_puzzle(ctx: &Context, channel: u64) -> Option<store::Row> {
    let weights = mix();
    let level = puzzles::pick_level(weights, rand::random::<u64>());
    let points = points_for(level);
    let puzzle: Puzzle = tokio::task::spawn_blocking(move || puzzles::generate(level, &mut puzzles::Rng::fresh())).await.ok()?;
    let now = Utc::now().timestamp();
    let row = {
        let db = store::db()?;
        let conn = db.lock();
        match store::add_puzzle(&conn, &puzzle, points, channel, now) {
            Ok(row) => row,
            Err(err) => {
                tracing::warn!("sudoku: puzzle not saved: {}", err);
                return None;
            }
        }
    };
    let message = card_message(&row, 0, now).await;
    let posted = place_card(ctx, channel, message, false).await?;
    if let Some(db) = store::db() {
        let _ = store::set_message(&db.lock(), row.id, posted);
    }
    tracing::info!("sudoku: puzzle {} ({:?}, {} givens) up in {}", row.id, level, puzzles::CELLS - puzzle.blanks(), channel);
    Some(store::Row { message: Some(posted), ..row })
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
        tracing::warn!("sudoku: card not edited: {}", err);
    }
}

/// The day's solvers, shortest form, for a card footer.
fn today_words(ctx: &Context, guild: Option<serenity::all::GuildId>) -> String {
    let Some(db) = store::db() else { return String::new() };
    let solves = store::day_solves(&db.lock(), &super::points::ist_day(Utc::now().timestamp()));
    let (won, _) = today_lines(&solves, |u| display_name(ctx, guild, u));
    won.join(" · ")
}

/// Turns a won puzzle's own card into the Solved card: same message, so it
/// keeps its place in the channel, with the picture and the buttons taken off.
async fn show_solved(ctx: &Context, row: &store::Row, guild: Option<serenity::all::GuildId>) {
    let Some(winner) = row.winner else { return };
    let (hints, all_hints, others, points, tally) = {
        let Some(db) = store::db() else { return };
        let conn = db.lock();
        let player = store::player(&conn, row.id, winner).unwrap_or_default();
        let day = store::day_solves(&conn, &super::points::ist_day(row.solved_ts.unwrap_or_else(|| Utc::now().timestamp())));
        let scored = day.iter().find(|s| s.puzzle == row.id && s.user == winner).map(|s| s.worth).unwrap_or(row.points);
        let tally: i64 = day.iter().filter(|s| s.user == winner).map(|s| s.worth).sum();
        (player.hints, store::hints_used(&conn, row.id), (store::playing(&conn, row.id) - 1).max(0), scored, tally.max(scored))
    };
    let solved = Solved {
        puzzle_id: row.id,
        level: row.level,
        winner,
        badge: badge_of(winner),
        seconds: row.seconds.unwrap_or(0),
        points,
        hints,
        full_points: row.points,
        all_hints,
        others,
        tally,
        today: today_words(ctx, guild),
    };
    let embed = CreateEmbed::new()
        .title(solved_title(row.id, row.level))
        .description(solved_text(&solved))
        .colour(SOLVED_COLOUR)
        .footer(CreateEmbedFooter::new(CARD_FOOTER));
    let Some(message) = row.message else { return };
    let edit = EditMessage::new()
        .embed(embed)
        .components(Vec::new())
        .attachments(EditAttachments::new())
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(row.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("sudoku: solved card for {} not shown: {}", row.id, err);
    }
}

// --- the rules post --------------------------------------------------------------------------------

async fn sync_rules(ctx: &Context, channel: Option<u64>) {
    let wanted = channel.filter(|_| rules_on());
    let old = meta_get("rules_message").and_then(|v| v.parse::<u64>().ok());
    let old_channel = meta_get("rules_channel").and_then(|v| v.parse::<u64>().ok());
    if let (Some(id), Some(c)) = (old, old_channel) {
        if Some(c) != wanted {
            delete(ctx, c, id).await;
            meta_set("rules_message", "");
            meta_set("rules_hash", "");
            SHARED.lock().rules_message = None;
        }
    }
    let Some(channel) = wanted else { return };
    let body = rules_text::sudoku_rules_text(&sudoku_rules());
    let hash = rules_text::digest(&[rules_text::SUDOKU_RULES_TITLE, &body]);
    let current = old.filter(|_| old_channel == Some(channel));
    let present = match current {
        Some(id) => exists(ctx, channel, id).await,
        None => false,
    };
    let embed = || CreateEmbed::new().title(rules_text::SUDOKU_RULES_TITLE).description(body.clone()).colour(COLOUR);
    if !present {
        let message = CreateMessage::new().embed(embed()).allowed_mentions(CreateAllowedMentions::new());
        match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
            Ok(posted) => {
                meta_set("rules_message", &posted.id.get().to_string());
                meta_set("rules_channel", &channel.to_string());
                meta_set("rules_hash", &hash);
                SHARED.lock().rules_message = Some(posted.id.get());
                tracing::info!("sudoku: rules post up in {}", channel);
            }
            Err(err) => tracing::warn!("sudoku: rules post not posted in {}: {}", channel, err),
        }
        return;
    }
    SHARED.lock().rules_message = current;
    if meta_get("rules_hash").as_deref() == Some(hash.as_str()) {
        return;
    }
    let Some(id) = current else { return };
    let edit = EditMessage::new().embeds(vec![embed()]).allowed_mentions(CreateAllowedMentions::new());
    match call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(id), edit)).await {
        Ok(_) => meta_set("rules_hash", &hash),
        Err(err) => tracing::warn!("sudoku: rules post not edited: {}", err),
    }
}

// --- the task --------------------------------------------------------------------------------------

/// Starts the task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting()) {
        (true, Some(c)) => tracing::info!("sudoku: playing in {}", c),
        (true, None) => tracing::info!("sudoku: on, but VIZIER_SUDOKU_CHANNEL isn't set"),
        (false, _) => tracing::info!("sudoku: VIZIER_SUDOKU is off"),
    }
    tokio::spawn(run(ctx));
}

/// After a start: pick up the card the last run left, and the puzzle it was for.
async fn recover(ctx: &Context) -> Option<store::Row> {
    let channel = active_channel(ctx);
    sync_rules(ctx, channel).await;
    let db = store::db()?;
    let (old_card, live) = {
        let conn = db.lock();
        (store::meta_get(&conn, "card"), store::live(&conn))
    };
    let old_card = old_card.as_deref().and_then(|v| v.split_once(':')).and_then(|(c, m)| Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?)));
    if let Some(c) = channel {
        if let Ok(latest) = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await {
            let mut s = SHARED.lock();
            s.latest = latest.first().map(|m| m.id.get()).unwrap_or(0);
            s.last_seen_ms = 0;
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
                tracing::info!("sudoku: puzzle {} picked up where it was left", row.id);
                return Some(row);
            }
            // The card is gone: the puzzle stays, its card goes back up.
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
    let mut ticks: u64 = 0;
    let mut shown: (i64, i64, i64) = (0, -1, 0);
    loop {
        tokio::time::sleep(TICK).await;
        ticks += 1;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        let (solved, skip, rules_check, dirty) = {
            let mut s = SHARED.lock();
            (s.solved.take(), std::mem::take(&mut s.skip), std::mem::take(&mut s.rules_check), s.dirty)
        };
        if rules_check || ticks % RULES_EVERY_TICKS == 0 {
            sync_rules(&ctx, channel).await;
        }
        let Some(channel) = channel else {
            if SHARED.lock().card.is_some() {
                drop_card(&ctx).await;
            }
            live = None;
            continue;
        };

        // A puzzle was just won, or a mod skipped one: show how it ended, then
        // put the next one up straight away.
        if let Some(id) = solved {
            let row = store::db().and_then(|db| store::get(&db.lock(), id));
            if let Some(row) = row.filter(|r| r.status == Status::Solved) {
                show_solved(&ctx, &row, ctx.cache.guilds().first().copied()).await;
                SHARED.lock().card = None;
            }
            live = None;
        }
        if skip {
            if let Some(row) = live.as_ref() {
                let done = store::db().map(|db| store::skip(&db.lock(), row.id, now).unwrap_or(false)).unwrap_or(false);
                if done {
                    tracing::info!("sudoku: puzzle {} skipped by a mod", row.id);
                }
            }
            drop_card(&ctx).await;
            live = None;
        }

        // The store is the truth: a puzzle claimed by a code is no longer live.
        if let Some(row) = &live {
            let fresh = store::db().and_then(|db| store::get(&db.lock(), row.id));
            match fresh {
                Some(f) if f.status == Status::Open && f.channel == channel => live = Some(f),
                _ => live = None,
            }
        }
        if live.is_none() {
            let open = store::db().and_then(|db| store::live(&db.lock()));
            // A puzzle left open in a channel the game has moved away from is
            // closed, so it stops looking live to `/sudoku` and the page.
            let existing = match open {
                Some(row) if row.channel == channel => Some(row),
                Some(stale) => {
                    if let Some(db) = store::db() {
                        let _ = store::skip(&db.lock(), stale.id, now);
                    }
                    tracing::info!("sudoku: puzzle {} closed — the game moved to {}", stale.id, channel);
                    None
                }
                None => None,
            };
            live = match existing {
                Some(row) => {
                    SHARED.lock().card = row.message.map(|m| (channel, m));
                    Some(row)
                }
                None => post_puzzle(&ctx, channel).await,
            };
            shown = (0, -1, 0);
        }
        let Some(row) = live.clone() else { continue };

        // Somewhere else to be, or no card at all: put one up.
        let card = SHARED.lock().card;
        if card.map(|(c, _)| c) != Some(channel) {
            let playing = store::db().map(|db| store::playing(&db.lock(), row.id)).unwrap_or(0);
            let message = card_message(&row, playing, now).await;
            if let Some(id) = place_card(&ctx, channel, message, card.is_some()).await {
                if let Some(db) = store::db() {
                    let _ = store::set_message(&db.lock(), row.id, id);
                }
                live = Some(store::Row { message: Some(id), ..row.clone() });
            }
            continue;
        }

        let playing = store::db().map(|db| store::playing(&db.lock(), row.id)).unwrap_or(0);
        let changed = shown.0 != row.id || shown.1 != playing;
        if (changed || dirty) && now - shown.2 >= EDIT_EVERY_SECS {
            edit_card(&ctx, &row, playing, now).await;
            SHARED.lock().dirty = false;
            shown = (row.id, playing, now);
        }
        keep_card_last(&ctx, &row, playing, now).await;
    }
}

/// Whether the puzzle card should move down now: something newer than it
/// landed in the channel and the channel has been quiet since. The rule is
/// Name Place Animal Thing's, shared rather than copied, and the card's own
/// post is never newer than itself, so a move can't set off another.
pub fn move_due(card: Option<u64>, latest: u64, last_seen_ms: i64, now_ms: i64) -> bool {
    super::npat::move_due(card, latest, last_seen_ms, now_ms, MOVE_DELAY_MS)
}

/// Moves the card below anything that landed under it — an admin's message,
/// the bot's own Solved card — a few seconds after the channel went quiet, and
/// puts it back if someone deleted it.
async fn keep_card_last(ctx: &Context, row: &store::Row, playing: i64, now: i64) {
    let (card, due, gone) = {
        let s = SHARED.lock();
        (s.card, move_due(s.card.map(|(_, m)| m), s.latest, s.last_seen_ms, Utc::now().timestamp_millis()), s.card_gone)
    };
    let Some((channel, _)) = card else { return };
    if !due && !gone {
        return;
    }
    SHARED.lock().card_gone = false;
    let message = card_message(row, playing, now).await;
    if let Some(id) = place_card(ctx, channel, message, !gone).await {
        if let Some(db) = store::db() {
            let _ = store::set_message(&db.lock(), row.id, id);
        }
    }
}

// --- reading a submission ----------------------------------------------------------------------------

/// What a pasted line turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Submission {
    /// A code, and the puzzle it names.
    Code(i64, Grid),
    /// 81 digits typed straight in, for the puzzle whose card they pressed.
    Digits(Grid),
    Bad(CodeError),
}

/// Reads what someone pasted: a code for any puzzle, or the 81 digits of a
/// grid for the puzzle they are looking at. `puzzle_of` finds a puzzle's
/// givens, so a code for a puzzle from yesterday can still be read.
pub fn read_submission(text: &str, puzzle_of: impl Fn(i64) -> Option<Grid>) -> Submission {
    let tidy: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    // 81 digits: the grid typed out, row by row, for the puzzle in front of them.
    if tidy.chars().count() == puzzles::CELLS && tidy.chars().all(|c| c.is_ascii_digit()) {
        return match puzzles::grid_from_str(&tidy) {
            Some(grid) => Submission::Digits(grid),
            None => Submission::Bad(CodeError::NotACode),
        };
    }
    let Some(id) = sudoku_code::puzzle_id_of(&tidy) else {
        return Submission::Bad(CodeError::NotACode);
    };
    let Some(givens) = puzzle_of(id) else {
        // A code that reads fine but names a puzzle we no longer check.
        return Submission::Bad(CodeError::OtherPuzzle(id));
    };
    match sudoku_code::decode(&tidy, id, &givens) {
        Ok(grid) => Submission::Code(id, grid),
        // The number matched the puzzle we looked up, so anything left is damage.
        Err(_) => Submission::Bad(CodeError::Damaged),
    }
}

/// Why a try wasn't looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Too soon after the last wrong try.
    Cooldown(i64),
    /// Out of tries on this puzzle.
    NoTriesLeft(i64),
}

/// Whether a player may try this puzzle again now.
pub fn may_try(player: Option<&store::Player>, max: i64, cooldown: i64, now: i64) -> Result<(), Refusal> {
    let Some(p) = player else { return Ok(()) };
    if p.tries >= max {
        return Err(Refusal::NoTriesLeft(max));
    }
    let wait = p.last_try_ts + cooldown - now;
    if p.last_try_ts > 0 && wait > 0 {
        return Err(Refusal::Cooldown(wait));
    }
    Ok(())
}

/// What a checked grid came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Right,
    /// How many squares are still empty.
    Unfinished(usize),
    /// How many filled squares are wrong — never which.
    Wrong(usize),
}

pub fn check(attempt: &Grid, solution: &Grid) -> Verdict {
    let (wrong, blank) = puzzles::wrong_squares(attempt, solution);
    match (blank, wrong) {
        (0, 0) => Verdict::Right,
        (0, n) => Verdict::Wrong(n),
        (n, _) => Verdict::Unfinished(n),
    }
}

pub fn wrong_words(verdict: Verdict, tries_left: i64) -> String {
    let left = match tries_left {
        n if n <= 0 => String::new(),
        1 => " · **one try left**".to_string(),
        n => format!(" · {} tries left", n),
    };
    match verdict {
        Verdict::Unfinished(n) => format!("⏳ Not finished yet — **{}** still empty. Fill every square, then press **Copy code** again.", plural(n as i64, "square is", "squares are")),
        Verdict::Wrong(n) => format!("❌ **{}** wrong (I won't say which). Press 🔍 **Check** on the page: it marks any number that clashes in its row, column or box.{}", plural(n as i64, "square is", "squares are"), left),
        Verdict::Right => "✅ Right!".to_string(),
    }
}

// --- buttons ------------------------------------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    match component.data.custom_id.as_str() {
        "sudokuplay" => play_pressed(ctx, component).await,
        "sudokucode" => code_pressed(ctx, component).await,
        "sudokuhint" => hint_pressed(ctx, component).await,
        "sudokutoday" => today_pressed(ctx, component).await,
        HELP_ID => help_pressed(ctx, component).await,
        _ => {}
    }
}

/// The puzzle that is up now.
fn live_row() -> Option<store::Row> {
    store::db().and_then(|db| store::live(&db.lock()))
}

/// Puzzles whose codes are still worth checking.
fn checkable() -> Vec<store::Row> {
    let now = Utc::now().timestamp();
    store::db().map(|db| store::checkable(&db.lock(), now - late_hours() * 3600, KEEP_CHECKABLE)).unwrap_or_default()
}

const OFF: &str = "Sudoku is switched off right now.";

/// What ▶️ Play sends back: the link and how to use it, or the way to play
/// without a page at all.
pub fn play_text(puzzle_id: i64, level: Level, points: i64, link: Option<&str>, givens: &Grid) -> String {
    match link {
        Some(link) => format!(
            "▶️ **Puzzle #{}** · {} {} · worth **{}**\n{}\n\
             -# Tap a square, tap a number. **Notes**, **Undo**, **Check** and **Reset** are there to help, and your work is saved on that device.\n\
             When every square is filled, press **Copy code** on the page, come back here, press 📋 **Submit code** and paste it.\n\
             -# The page never knows the answer — the bot checks it. `/sudoku` finds this link again, and ❓ How to play on the card explains the rest.",
            puzzle_id,
            level.emoji(),
            level.name(),
            score_words(points),
            link
        ),
        None => format!(
            "▶️ **Puzzle #{}** · {} {} · worth **{}**\n\
             ⚠️ The web page isn't set up yet (a mod needs to set **VIZIER_PANEL_URL** in the panel), so here it is as text:\n```\n{}\n```\n\
             Solve it anywhere you like, then press 📋 **Submit code** and type all **81 digits**, row by row, left to right — 0 for a square you haven't filled.",
            puzzle_id,
            level.emoji(),
            level.name(),
            score_words(points),
            text_grid(givens)
        ),
    }
}

/// The grid as text, for when there is no page: columns 1–9 across the top,
/// rows a–i down the side.
pub fn text_grid(givens: &Grid) -> String {
    let mut out = String::from("   1 2 3  4 5 6  7 8 9\n");
    for row in 0..9 {
        if row % 3 == 0 && row > 0 {
            out.push_str("   ------+------+------\n");
        }
        out.push((b'a' + row as u8) as char);
        out.push(' ');
        for col in 0..9 {
            if col % 3 == 0 {
                out.push(' ');
            }
            let digit = givens[row * 9 + col];
            out.push(if digit == 0 { '.' } else { char::from(b'0' + digit) });
            out.push(' ');
        }
        out.push('\n');
    }
    out
}

async fn play_pressed(ctx: &Context, component: &ComponentInteraction) {
    if live_channel().is_none() {
        return whisper(ctx, component, OFF).await;
    }
    let Some(row) = live_row() else {
        return whisper(ctx, component, "The next puzzle is on its way — try again in a moment.").await;
    };
    let now = Utc::now().timestamp();
    let user = component.user.id.get();
    let joined = store::db().map(|db| store::start_playing(&db.lock(), row.id, user, now).unwrap_or(false)).unwrap_or(false);
    if joined {
        SHARED.lock().dirty = true;
    }
    let link = page_base().map(|base| page_link(&base, row.id));
    whisper(ctx, component, play_text(row.id, row.level, row.points, link.as_deref(), &row.givens)).await;
}

/// The pop-up that takes a code: one plain box, nothing clever — the newer
/// pop-up blocks crash the Discord iPhone app.
pub fn modal_json(puzzle_id: i64) -> Value {
    json!({
        "type": 9,
        "data": {
            "custom_id": format!("sudokuans:{}", puzzle_id),
            "title": format!("Puzzle #{} · your answer", puzzle_id),
            "components": [{
                "type": 1,
                "components": [{
                    "type": 4, "custom_id": "code", "label": "Paste your code", "style": 1,
                    "required": true, "max_length": 200, "min_length": 3,
                    "placeholder": "S123-ABCDE-… (or all 81 digits)",
                }],
            }],
        }
    })
}

/// The one box out of a submitted pop-up, however Discord wrapped it.
pub fn code_from(data: &Value) -> String {
    fn walk(v: &Value) -> Option<String> {
        match v {
            Value::Object(map) => {
                if map.get("custom_id").and_then(Value::as_str) == Some("code") {
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
    walk(data.get("components").unwrap_or(data)).unwrap_or_default().trim().to_string()
}

async fn open_modal(ctx: &Context, component: &ComponentInteraction, puzzle_id: i64) {
    let modal = modal_json(puzzle_id);
    if let Err(err) = ctx.http.create_interaction_response(component.id, &component.token, &modal, Vec::new()).await {
        tracing::warn!("sudoku: code pop-up not shown to {}: {}", component.user.id, err);
    }
}

async fn code_pressed(ctx: &Context, component: &ComponentInteraction) {
    let live = live_row().map(|r| r.id).or_else(|| checkable().first().map(|r| r.id));
    match live {
        Some(id) => open_modal(ctx, component, id).await,
        None => whisper(ctx, component, "There's no puzzle to answer yet.").await,
    }
}

async fn hint_pressed(ctx: &Context, component: &ComponentInteraction) {
    if live_channel().is_none() {
        return whisper(ctx, component, OFF).await;
    }
    let Some(row) = live_row() else {
        return whisper(ctx, component, "The next puzzle is on its way — try again in a moment.").await;
    };
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let (max, cost) = (max_hints(), hint_cost());
    let Some(db) = store::db() else { return whisper(ctx, component, "Hints aren't working just now.").await };
    let had = {
        let conn = db.lock();
        store::player(&conn, row.id, user).unwrap_or_default()
    };
    if had.hints >= max {
        return whisper(
            ctx,
            component,
            format!("💡 You've had all **{}** hints for puzzle #{}. The rest is yours to work out.", max, row.id),
        )
        .await;
    }
    let Some(cell) = pick_hint(&row.givens, &had.hint_cells, rand::random::<u64>()) else {
        return whisper(ctx, component, "There's nothing left to give away on this one.").await;
    };
    let hints = {
        let conn = db.lock();
        store::add_hint(&conn, row.id, user, cell, now).unwrap_or(had.hints + 1)
    };
    SHARED.lock().dirty = true;
    let worth = worth_after_hints(row.points, hints, cost);
    whisper(
        ctx,
        component,
        format!(
            "💡 **{} is {}** · hint **{}/{}** on puzzle #{}\n-# This puzzle is now worth **{}** to you. Only you can see this.",
            puzzles::cell_name(cell),
            row.solution[cell],
            hints,
            max,
            row.id,
            score_words(worth)
        ),
    )
    .await;
}

/// A square to give away: one the puzzle left blank and this player hasn't
/// already been given.
pub fn pick_hint(givens: &Grid, already: &[usize], roll: u64) -> Option<usize> {
    let left: Vec<usize> = (0..puzzles::CELLS).filter(|c| givens[*c] == 0 && !already.contains(c)).collect();
    if left.is_empty() {
        return None;
    }
    Some(left[(roll % left.len() as u64) as usize])
}

async fn today_pressed(ctx: &Context, component: &ComponentInteraction) {
    let user = component.user.id.get();
    let solves = match store::db() {
        Some(db) => store::day_solves(&db.lock(), &super::points::ist_day(Utc::now().timestamp())),
        None => Vec::new(),
    };
    whisper(ctx, component, today_text(&solves, user)).await;
}

/// The private list behind 📊 Today's solvers.
pub fn today_text(solves: &[store::Solve], me: u64) -> String {
    if solves.is_empty() {
        return "📊 **Today's solvers**\nNobody has solved one yet today — the puzzle above is going begging.".to_string();
    }
    let mut lines = vec!["📊 **Today's solvers**".to_string()];
    let shown: Vec<&store::Solve> = solves.iter().rev().take(15).collect();
    for s in shown.iter().rev() {
        lines.push(solver_line(s));
    }
    if solves.len() > 15 {
        lines.push(format!("-# …and {} earlier today", solves.len() - 15));
    }
    let mine: Vec<&store::Solve> = solves.iter().filter(|s| s.user == me).collect();
    let wins = mine.iter().filter(|s| s.kind == Kind::Win).count();
    let points: i64 = mine.iter().map(|s| s.worth).sum();
    lines.push(match (mine.len(), wins) {
        (0, _) => "-# You haven't solved one today. Press ▶️ Play on the card above.".to_string(),
        (all, won) => format!("-# **You:** {} today · {} won · **{}**", plural(all as i64, "puzzle", "puzzles"), won, score_words(points)),
    });
    lines.push("-# Sudoku keeps its own score: sudoku points, no daily limit, and everyone has them. `/sudokutop` is the board.".to_string());
    lines.join("\n")
}

// --- the pop-up ---------------------------------------------------------------------------------------

async fn reply_modal(ctx: &Context, modal: &ModalInteraction, text: String) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = modal.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("sudoku: reply to {} not sent: {}", modal.user.id, err);
    }
}

pub async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    let Some(card_puzzle) = modal.data.custom_id.strip_prefix("sudokuans:").and_then(|r| r.parse::<i64>().ok()) else {
        return;
    };
    let data = serde_json::to_value(&modal.data).unwrap_or(Value::Null);
    let text = code_from(&data);
    let user = modal.user.id.get();
    let now = Utc::now().timestamp();
    let rows = checkable();
    let live = live_row();
    let read = read_submission(&text, |id| rows.iter().find(|r| r.id == id).map(|r| r.givens));
    let (puzzle_id, grid) = match read {
        Submission::Code(id, grid) => (id, grid),
        Submission::Digits(grid) => (card_puzzle, grid),
        Submission::Bad(CodeError::OtherPuzzle(id)) if !rows.iter().any(|r| r.id == id) => {
            return reply_modal(
                ctx,
                modal,
                format!(
                    "🕰️ **Puzzle #{}** is too old to check — codes work for **{}** after a puzzle goes up. The one on the card above is waiting though.",
                    id,
                    plural(late_hours(), "hour", "hours")
                ),
            )
            .await;
        }
        Submission::Bad(err) => {
            let live_id = live.as_ref().map(|r| r.id).unwrap_or(card_puzzle);
            return reply_modal(ctx, modal, err.words(live_id)).await;
        }
    };
    let Some(row) = rows.iter().find(|r| r.id == puzzle_id).cloned().or_else(|| store::db().and_then(|db| store::get(&db.lock(), puzzle_id))) else {
        return reply_modal(ctx, modal, format!("I can't find puzzle #{} any more.", puzzle_id)).await;
    };
    let Some(db) = store::db() else { return reply_modal(ctx, modal, "Sudoku isn't working just now.".to_string()).await };

    // Someone who has already been counted on this puzzle is simply told so.
    let (already, player) = {
        let conn = db.lock();
        (store::solved_by(&conn, row.id, user), store::player(&conn, row.id, user))
    };
    if let Some(kind) = already {
        let words = match kind {
            Kind::Win => format!("🏅 You already won puzzle #{}.", row.id),
            Kind::Finish => format!("✅ You already finished puzzle #{} — it was won by someone else.", row.id),
        };
        return reply_modal(ctx, modal, words).await;
    }

    if let Err(refusal) = may_try(player.as_ref(), max_tries(), COOLDOWN_SECS, now) {
        let words = match refusal {
            Refusal::Cooldown(wait) => format!("⏳ Give it **{} s** — then paste it again.", wait),
            Refusal::NoTriesLeft(max) => {
                format!("🚫 That's all **{}** tries on puzzle #{}. 💡 Hint still works, and the next puzzle is a fresh start.", max, row.id)
            }
        };
        return reply_modal(ctx, modal, words).await;
    }

    match check(&grid, &row.solution) {
        Verdict::Right => {
            let outcome = finish(&row, user, now);
            reply_modal(ctx, modal, outcome.words).await;
            if outcome.won {
                SHARED.lock().solved = Some(row.id);
            }
        }
        verdict => {
            let tries = {
                let conn = db.lock();
                store::add_try(&conn, row.id, user, now).unwrap_or(0)
            };
            let head = if row.id != card_puzzle || live.as_ref().map(|r| r.id) != Some(row.id) {
                format!("**Puzzle #{}** · ", row.id)
            } else {
                String::new()
            };
            reply_modal(ctx, modal, format!("{}{}", head, wrong_words(verdict, max_tries() - tries))).await;
        }
    }
}

/// What a correct grid came to.
struct Landed {
    won: bool,
    words: String,
}

/// Records a correct grid: a win if this player got there first, otherwise a
/// finish, which is written down but pays nothing.
fn finish(row: &store::Row, user: u64, now: i64) -> Landed {
    let Some(db) = store::db() else { return Landed { won: false, words: "Sudoku isn't working just now.".into() } };
    let day = super::points::ist_day(now);
    let won = {
        let conn = db.lock();
        store::claim(&conn, row.id, user, now).unwrap_or(false)
    };
    if !won {
        // A finish: written down so it shows in the day's list, and that is all.
        // It scores nothing, because the puzzle was already won — see `payout`.
        let seconds = now - row.posted_ts;
        let winner = {
            let conn = db.lock();
            let _ = store::add_solve(&conn, &day, user, row.id, 0, 0, Kind::Finish, row.level, seconds, now);
            store::get(&conn, row.id).and_then(|r| r.winner)
        };
        let words = match winner {
            Some(w) => format!(
                "✅ **Correct!** Puzzle #{} in **{}** — <@{}> got there first, so the sudoku points went to them.\n-# It still counts as a finish. The puzzle on the card above is up for grabs.",
                row.id,
                spent_words(seconds),
                w
            ),
            None => format!(
                "✅ **Correct!** Puzzle #{} in **{}** — that one was already closed, so there was nothing left to score.",
                row.id,
                spent_words(seconds)
            ),
        };
        return Landed { won: false, words };
    }
    let (seconds, hints) = {
        let conn = db.lock();
        (
            store::get(&conn, row.id).and_then(|r| r.seconds).unwrap_or(now - row.posted_ts),
            store::player(&conn, row.id, user).map(|p| p.hints).unwrap_or(0),
        )
    };
    let worth = payout(Kind::Win, row.points, hints, hint_cost());
    // The ledger is still told, and still told NOTHING: a zero row, written by
    // the one door that turns mods, Muggles and the unsorted away. It moves no
    // house points and no total — it is the receipt that says this person played
    // sudoku today, which is what keeps the daily 🐸 card in the right hands and
    // what stops a replayed puzzle ever paying if the game is ever put back.
    let reason = format!("Sudoku: puzzle {} ({}) — sudoku points, no house points", row.id, row.level.name().to_lowercase());
    let _ = super::house::award_person(user, Source::Sudoku, 0, &reason, None, Some(format!("sudoku:{}", row.id)), None);
    let tally = {
        let conn = db.lock();
        let _ = store::add_solve(&conn, &day, user, row.id, 0, worth, Kind::Win, row.level, seconds, now);
        store::day_solves(&conn, &day).iter().filter(|s| s.user == user).map(|s| s.worth).sum::<i64>()
    };
    tracing::info!("sudoku: puzzle {} won by {} in {}s for {} sudoku points", row.id, user, seconds, worth);
    let aside = match worth {
        0 => "\n-# Your hints used up this puzzle's sudoku points, but the solve still counts.".to_string(),
        _ => format!("\n-# **{}** today. `/sudokutop` is the board.", score_words(tally.max(worth))),
    };
    Landed {
        won: true,
        words: format!("🎉 **You solved puzzle #{}** in **{}** · **+{}**{}", row.id, spent_words(seconds), score_words(worth), aside),
    }
}

// --- commands -------------------------------------------------------------------------------------------

pub fn new_builder() -> CreateCommand {
    CreateCommand::new("sudokunew").description("admin only: skip the sudoku that's up and post a fresh one, nobody scoring for it")
}

pub fn mine_builder() -> CreateCommand {
    CreateCommand::new("sudoku").description("your sudoku links: the puzzle that's up now and any you haven't finished")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("sudokutop")
        .description("the sudoku points board, today or this month")
        .add_option(
            serenity::all::CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "which days to count")
                .add_string_choice("Today", "today")
                .add_string_choice("This month", "month"),
        )
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("sudokuhelp").description("how the sudoku game works, from the settings as they are now")
}

async fn reply_command(ctx: &Context, command: &CommandInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message.ephemeral(true))).await {
        tracing::warn!("sudoku: reply to {} not sent: {}", command.user.id, err);
    }
}

/// `/sudokunew` — admins only.
pub async fn new_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        return reply_command(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can skip a puzzle.")).await;
    }
    if live_channel().is_none() {
        return reply_command(ctx, command, CreateInteractionResponseMessage::new().content(OFF)).await;
    }
    SHARED.lock().skip = true;
    let live = live_row().map(|r| r.id);
    tracing::info!("sudoku: /sudokunew by {} (puzzle {:?})", command.user.id, live);
    let text = match live {
        Some(id) => format!("⏭️ Puzzle #{} skipped. A fresh one is on its way — nobody scores for the skipped one.", id),
        None => "⏭️ A fresh puzzle is on its way.".to_string(),
    };
    reply_command(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
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
/// they have and over how many puzzles. Nothing at all when they haven't won one
/// in that stretch.
pub fn standing(rows: &[store::Tally], me: u64) -> Option<String> {
    let place = place_of(rows, me)?;
    let mine = rows.get(place - 1)?;
    Some(format!("{} of {} · **{}** · {}", ordinal(place), rows.len(), score_words(mine.points), plural(mine.solves, "puzzle", "puzzles")))
}

/// The first and last day of the India month a day falls in. The store's days
/// are `YYYY-MM-DD`, which sorts as a date, so a month is just a pair of ends —
/// a 31st that some months don't have is simply never matched.
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

/// Today's and this month's sudoku points, both boards ranked.
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

/// How many names `/sudokutop` lists.
const TOP_LIST: usize = 10;

/// What `/sudokutop` says. `rows` is the whole board, already ranked; only the
/// first [`TOP_LIST`] are listed, and whoever asked gets their own line under
/// them when they didn't make it.
pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("🧩 **Sudoku points** · {}", period);
    if rows.is_empty() {
        text.push_str("\nNobody has solved one yet. The puzzle is waiting in the channel.");
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
        text.push_str(&format!("\n{} <@{}> **{}** · {}{}", rank, t.user, t.points, plural(t.solves, "puzzle", "puzzles"), you));
    }
    if place_of(rows, me).is_some_and(|p| p > TOP_LIST) {
        if let Some(mine) = standing(rows, me) {
            text.push_str(&format!("\n-# **You:** {}", mine));
        }
    }
    text.push_str("\n-# Sudoku points are this game's own score — no daily limit, and everyone has them. They don't move the House Cup.");
    text
}

/// The lines `/sudoku` shows: the live puzzle, where the asker stands today and
/// this month, then the puzzles they started and never finished, newest first.
pub fn mine_text(
    live: Option<&store::Row>,
    today: &[store::Tally],
    month: &[store::Tally],
    me: u64,
    mine: &[(store::Row, bool)],
    base: Option<&str>,
    channel: Option<u64>,
) -> String {
    let link = |id: i64| match base {
        Some(b) => format!(" · [play]({})", page_link(b, id)),
        None => String::new(),
    };
    let mut lines = vec!["🔢 **Your sudoku**".to_string()];
    match live {
        Some(row) => lines.push(format!(
            "**Up now: #{}** {} {} · worth **{}**{}",
            row.id,
            row.level.emoji(),
            row.level.name(),
            score_words(row.points),
            link(row.id)
        )),
        None => lines.push("No puzzle is up right now.".to_string()),
    }
    lines.push(match standing(today, me) {
        Some(yours) => format!("-# **You today:** {}", yours),
        None => "-# You haven't won one today. Press ▶️ Play on the card and see.".to_string(),
    });
    if let Some(yours) = standing(month, me) {
        lines.push(format!("-# **This month:** {}", yours));
    }
    let open: Vec<&(store::Row, bool)> = mine.iter().filter(|(_, done)| !done).filter(|(r, _)| live.map(|l| l.id) != Some(r.id)).collect();
    if open.is_empty() {
        lines.push("-# Nothing else of yours is half finished.".to_string());
    } else {
        lines.push("\n**Still open for you**".to_string());
        for (row, _) in open.iter().take(MINE_LINES) {
            let ending = match row.status {
                Status::Open => "still up for grabs".to_string(),
                Status::Solved => format!("won by <@{}>", row.winner.unwrap_or(0)),
                Status::Skipped => "skipped by a mod".to_string(),
            };
            lines.push(format!("#{} {} {} · {}{}", row.id, row.level.emoji(), row.level.name(), ending, link(row.id)));
        }
        if open.len() > MINE_LINES {
            lines.push(format!("-# …and {} more", open.len() - MINE_LINES));
        }
        lines.push("-# Finish one and paste its code below: you'll be told whether it was right, but the sudoku points went to whoever was first.".to_string());
    }
    if base.is_none() {
        lines.push("-# The web page isn't set up yet — a mod needs to set **VIZIER_PANEL_URL** in the panel.".to_string());
    }
    if let Some(c) = channel {
        lines.push(format!(
            "-# The puzzle card lives in <#{}> · `/sudokutop` for the board · press ❓ How to play on the card for the rules.",
            c
        ));
    }
    lines.join("\n")
}

/// `/sudoku` — everyone.
pub async fn mine_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let live = live_row();
    let rows = checkable();
    let mine: Vec<(store::Row, bool)> = match store::db() {
        Some(db) => {
            let conn = db.lock();
            rows.iter()
                .filter(|r| store::player(&conn, r.id, user).is_some())
                .map(|r| (r.clone(), store::solved_by(&conn, r.id, user).is_some()))
                .collect()
        }
        None => Vec::new(),
    };
    let base = page_base();
    let (today, month) = boards_now();
    let text = mine_text(live.as_ref(), &today, &month, user, &mine, base.as_deref(), live_channel());
    let button = live.as_ref().map(|r| r.id).or_else(|| rows.first().map(|r| r.id));
    let mut message = CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if button.is_some() {
        message = message.components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new("sudokucode").label("📋 Submit code").style(ButtonStyle::Primary),
        ])]);
    }
    reply_command(ctx, command, message).await;
}

/// `/sudokutop [period]` — everyone, shown only to them, like `/housetop`.
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
    reply_command(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

/// The rules as a card, for both the command and the ❓ How to play button:
/// one set of words, written from the settings as they are now.
fn help_embed() -> CreateEmbed {
    CreateEmbed::new()
        .title(rules_text::SUDOKU_RULES_TITLE)
        .description(rules_text::sudoku_help_text(&sudoku_rules()))
        .colour(COLOUR)
}

/// `/sudokuhelp` — everyone.
pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    reply_command(ctx, command, CreateInteractionResponseMessage::new().embed(help_embed())).await;
}

/// ❓ How to play, from the puzzle card. The same words the command gives,
/// shown only to whoever pressed it.
async fn help_pressed(ctx: &Context, component: &ComponentInteraction) {
    let message = CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true);
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("sudoku: the rules weren't shown to {}: {}", component.user.id, err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_puzzle_card_can_be_played_and_explained_without_typing_a_word() {
        // Members can't type in that channel - the message box itself is denied
        // them, so /sudokuhelp cannot be run there - which is why the rules are
        // a button like everything else.
        let rows = serde_json::to_value(card_rows()).expect("rows");
        let rows = rows.as_array().expect("two rows");
        assert_eq!(rows.len(), 2);
        let ids = |row: &serde_json::Value| -> Vec<String> {
            row["components"].as_array().expect("buttons").iter().filter_map(|b| b["custom_id"].as_str().map(String::from)).collect()
        };
        assert_eq!(ids(&rows[0]), vec!["sudokuplay", "sudokucode", "sudokuhint", "sudokutoday"], "the four it had are untouched");
        assert!(rows[0]["components"].as_array().unwrap().len() <= 5, "Discord allows five to a row");
        assert_eq!(ids(&rows[1]), vec![HELP_ID]);
        assert_eq!(rows[1]["components"][0]["label"], "❓ How to play");
        // And the card points at the button rather than at a command nobody
        // in that channel can run.
        assert!(CARD_FOOTER.contains("❓ How to play"), "{}", CARD_FOOTER);
        assert!(!CARD_FOOTER.contains("/sudokuhelp"), "{}", CARD_FOOTER);
    }
    use crate::channels::discord::sudoku_gen::{Rng, generate};
    use crate::channels::discord::sudoku_store::tests::{memory, put};

    fn solve_row(conn: &rusqlite::Connection, level: Level, points: i64, at: i64) -> store::Row {
        put(conn, level, points, at)
    }

    #[test]
    fn the_card_says_the_level_the_points_and_who_is_playing() {
        let text = card_text(&Live { level: Level::Medium, points: 4, playing: 5, open_secs: 380, hint_cost: 1 });
        assert!(text.contains("🟡 **Medium**"), "{}", text);
        assert!(text.contains("worth **4 sudoku points**") && text.contains("first to solve it wins"));
        assert!(text.contains("**5 people** playing") && text.contains("open **6 min**"));
        assert!(text.contains("a hint costs **1 sudoku point**"));
        assert_eq!(card_title(128), "🔢 Sudoku · Puzzle #128");
        // Nobody yet, and free hints.
        let empty = card_text(&Live { level: Level::Hard, points: 6, playing: 0, open_secs: 4, hint_cost: 0 });
        assert!(empty.contains("nobody playing yet") && empty.contains("open **just now**"), "{}", empty);
        assert!(empty.contains("hints are free"));
        assert!(CARD_FOOTER.contains("the next puzzle appears"));
    }

    fn solved() -> Solved {
        Solved {
            puzzle_id: 128,
            level: Level::Medium,
            winner: 42,
            badge: "🦅 Ravenclaw".into(),
            seconds: 461,
            points: 4,
            hints: 0,
            full_points: 4,
            all_hints: 0,
            others: 4,
            tally: 26,
            today: "Aarav 3 (+9) · Meera 2 (+6)".into(),
        }
    }

    #[test]
    fn the_solved_card_names_the_winner_the_puzzle_and_the_sudoku_points() {
        let solved = solved();
        assert_eq!(solved_title(128, Level::Medium), "🎉 Solved! Puzzle #128 · 🟡 Medium");
        let text = solved_text(&solved);
        assert_eq!(
            text.lines().next().unwrap(),
            "🧩 **solved by <@42>** 🦅 Ravenclaw · **+4 sudoku points** · 26 today"
        );
        // The difficulty, how long it took and who was playing are all still there.
        assert!(text.contains("🟡 Medium in 7 min 41 s"), "{}", text);
        assert!(text.contains("no hints used") && text.contains("4 others were also playing"), "{}", text);
        // Hints eat into the sudoku points and are said so.
        let hinted = Solved { hints: 2, points: 2, all_hints: 3, others: 0, tally: 2, ..solved.clone() };
        let text = solved_text(&hinted);
        assert!(text.contains("**+2 sudoku points** · 2 today"), "{}", text);
        assert!(text.contains("2 hints used, 2 off") && text.contains("3 hints in all"), "{}", text);
        // One point reads as one point, and hints that ate the lot say so.
        assert!(solved_text(&Solved { points: 1, tally: 1, ..solved.clone() }).contains("**+1 sudoku point** · 1 today"));
        let spent = Solved { points: 0, hints: 4, tally: 6, ..solved.clone() };
        assert!(solved_text(&spent).contains("**no sudoku points left after hints** · 6 today"), "{}", solved_text(&spent));
        // A mod or a Muggle, with no crest, wins the same way.
        let no_house = Solved { badge: String::new(), ..solved };
        assert!(solved_text(&no_house).starts_with("🧩 **solved by <@42>** · **+4 sudoku points** · 26 today"), "{}", solved_text(&no_house));
    }

    #[test]
    fn hints_take_points_off_but_never_below_nothing() {
        assert_eq!(worth_after_hints(4, 0, 1), 4);
        assert_eq!(worth_after_hints(4, 3, 1), 1);
        assert_eq!(worth_after_hints(2, 3, 1), 0, "never a negative score");
        assert_eq!(worth_after_hints(6, 2, 3), 0);
        assert_eq!(worth_after_hints(6, 1, 0), 6, "free hints cost nothing");
    }

    #[test]
    fn a_hint_gives_away_a_blank_square_and_never_the_same_one_twice() {
        let p = generate(Level::Easy, &mut Rng::seeded(5));
        let mut given_out = Vec::new();
        for roll in 0..20u64 {
            let Some(cell) = pick_hint(&p.givens, &given_out, roll * 7919) else { break };
            assert_eq!(p.givens[cell], 0, "a hint gave away a square that was already shown");
            assert!(!given_out.contains(&cell));
            given_out.push(cell);
        }
        assert!(given_out.len() >= 20 || given_out.len() == p.blanks());
        // Nothing left to give.
        let all: Vec<usize> = (0..puzzles::CELLS).filter(|c| p.givens[*c] == 0).collect();
        assert_eq!(pick_hint(&p.givens, &all, 3), None);
    }

    #[test]
    fn a_pasted_line_is_read_as_a_code_or_as_81_digits() {
        let conn = memory();
        let row = solve_row(&conn, Level::Medium, 4, 100);
        let code = sudoku_code::encode(row.id, &row.givens, &row.solution);
        let find = |id: i64| store::get(&conn, id).map(|r| r.givens);
        assert_eq!(read_submission(&code, find), Submission::Code(row.id, row.solution));
        let digits = puzzles::grid_to_str(&row.solution);
        assert_eq!(read_submission(&digits, find), Submission::Digits(row.solution));
        // A code for a puzzle we don't keep, and plain nonsense.
        assert_eq!(read_submission("S9999-ABCDE", find), Submission::Bad(CodeError::OtherPuzzle(9999)));
        assert_eq!(read_submission("hello there", find), Submission::Bad(CodeError::NotACode));
        // A code for a puzzle we DO keep, with a character changed on the way.
        let mut letters: Vec<char> = code.chars().collect();
        let at = letters.len() / 2;
        letters[at] = if letters[at] == 'A' { 'B' } else { 'A' };
        let broken: String = letters.into_iter().collect();
        assert_eq!(read_submission(&broken, find), Submission::Bad(CodeError::Damaged));
    }

    #[test]
    fn a_grid_is_checked_without_saying_which_squares_are_wrong() {
        let p = generate(Level::Medium, &mut Rng::seeded(17));
        assert_eq!(check(&p.solution, &p.solution), Verdict::Right);
        let mut wrong = p.solution;
        let blank = (0..puzzles::CELLS).find(|c| p.givens[*c] == 0).unwrap();
        wrong[blank] = if p.solution[blank] == 9 { 1 } else { p.solution[blank] + 1 };
        assert_eq!(check(&wrong, &p.solution), Verdict::Wrong(1));
        let words = wrong_words(Verdict::Wrong(1), 9);
        assert!(words.contains("**1 square is** wrong") && words.contains("9 tries left"), "{}", words);
        assert!(!words.contains(&puzzles::cell_name(blank)), "the reply must not say where");
        let mut unfinished = p.solution;
        unfinished[blank] = 0;
        assert_eq!(check(&unfinished, &p.solution), Verdict::Unfinished(1));
        assert!(wrong_words(Verdict::Unfinished(1), 5).contains("Not finished yet"));
        assert!(wrong_words(Verdict::Wrong(3), 1).contains("one try left"));
    }

    #[test]
    fn tries_are_limited_and_spaced_out() {
        let none: Option<store::Player> = None;
        assert_eq!(may_try(none.as_ref(), 10, 10, 1_000), Ok(()));
        let fresh = store::Player { tries: 1, last_try_ts: 995, ..Default::default() };
        assert_eq!(may_try(Some(&fresh), 10, 10, 1_000), Err(Refusal::Cooldown(5)));
        assert_eq!(may_try(Some(&fresh), 10, 10, 1_005), Ok(()));
        let spent = store::Player { tries: 10, last_try_ts: 100, ..Default::default() };
        assert_eq!(may_try(Some(&spent), 10, 10, 9_999), Err(Refusal::NoTriesLeft(10)));
        // A player who has never tried isn't held back by the cooldown.
        let played = store::Player { tries: 0, last_try_ts: 0, ..Default::default() };
        assert_eq!(may_try(Some(&played), 10, 10, 1), Ok(()));
    }

    #[test]
    fn todays_solvers_split_wins_from_finishes() {
        // The ledger pays nothing for any of these now: `points` is nought and
        // the sudoku points are the whole of the score.
        let solves = vec![
            store::Solve { user: 1, puzzle: 1, points: 0, worth: 4, kind: Kind::Win, level: Level::Medium, seconds: 120, ts: 10 },
            store::Solve { user: 2, puzzle: 1, points: 0, worth: 0, kind: Kind::Finish, level: Level::Medium, seconds: 400, ts: 20 },
            store::Solve { user: 1, puzzle: 2, points: 0, worth: 2, kind: Kind::Win, level: Level::Easy, seconds: 90, ts: 30 },
            store::Solve { user: 3, puzzle: 3, points: 0, worth: 6, kind: Kind::Win, level: Level::Hard, seconds: 800, ts: 40 },
        ];
        let name = |u: u64| format!("P{}", u);
        let (won, also) = today_lines(&solves, name);
        assert_eq!(won, vec!["P1 2 (+6)", "P3 1 (+6)"]);
        assert_eq!(also, vec!["P2"]);
        let text = today_text(&solves, 2);
        assert!(text.contains("<@2> · #1 🟡 Medium · 6 min 40 s · finished after the win, so nothing scored"), "{}", text);
        assert!(text.contains("**You:** 1 puzzle today · 0 won · **0 sudoku points**"), "{}", text);
        assert!(text.contains("<@1> · #2 🟢 Easy · 1 min 30 s · **+2**"), "{}", text);
        assert!(today_text(&[], 1).contains("Nobody has solved one yet today"));
        assert!(today_text(&solves, 99).contains("You haven't solved one today"));
        assert!(today_text(&solves, 1).contains("**You:** 2 puzzles today · 2 won · **6 sudoku points**"), "{}", today_text(&solves, 1));
    }

    #[test]
    fn the_play_message_carries_the_link_or_a_way_to_play_without_one() {
        let p = generate(Level::Easy, &mut Rng::seeded(2));
        let link = page_link("https://panel.example/", 128);
        assert_eq!(link, "https://panel.example/sudoku/128");
        let with = play_text(128, Level::Easy, 2, Some(&link), &p.givens);
        assert!(with.contains(&link) && with.contains("Copy code") && with.contains("Submit code"), "{}", with);
        assert!(with.contains("never knows the answer"));
        let without = play_text(128, Level::Easy, 2, None, &p.givens);
        assert!(without.contains("VIZIER_PANEL_URL") && without.contains("81 digits"), "{}", without);
        // The text grid shows the givens and hides the blanks.
        let grid = text_grid(&p.givens);
        assert_eq!(grid.lines().count(), 12, "nine rows, a header and two rules");
        assert!(grid.contains('.'), "blanks are dots");
    }

    #[test]
    fn the_pop_up_is_one_plain_box() {
        let modal = modal_json(128);
        assert_eq!(modal["data"]["custom_id"], "sudokuans:128");
        let rows = modal["data"]["components"].as_array().expect("rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["type"], 1, "an action row, never a text display (it crashes the iPhone app)");
        let input = &rows[0]["components"][0];
        assert_eq!((input["type"].as_i64(), input["custom_id"].as_str()), (Some(4), Some("code")));
        // And it reads back whichever shape Discord sends.
        let sent = json!({ "components": [{ "type": 1, "components": [{ "type": 4, "custom_id": "code", "value": " S128-ABC " }] }] });
        assert_eq!(code_from(&sent), "S128-ABC");
        let labelled = json!({ "components": [{ "type": 18, "component": { "type": 4, "custom_id": "code", "value": "S1-XY" } }] });
        assert_eq!(code_from(&labelled), "S1-XY");
        assert_eq!(code_from(&json!({})), "");
    }

    #[test]
    fn my_puzzles_lists_the_live_one_and_anything_half_finished() {
        let conn = memory();
        let live = solve_row(&conn, Level::Medium, 4, 1_000);
        let older = solve_row(&conn, Level::Hard, 6, 900);
        let done = solve_row(&conn, Level::Easy, 2, 800);
        store::claim(&conn, older.id, 7, 950).unwrap();
        let older = store::get(&conn, older.id).unwrap();
        let mine = vec![(older.clone(), false), (done.clone(), true)];
        let today = vec![tally(2, 10, 3, 20), tally(1, 6, 2, 30)];
        let month = vec![tally(1, 84, 21, 90), tally(2, 18, 5, 20)];
        let text = mine_text(Some(&live), &today, &month, 1, &mine, Some("https://p.example"), Some(55));
        assert!(text.contains(&format!("**Up now: #{}**", live.id)), "{}", text);
        assert!(text.contains("worth **4 sudoku points**"), "{}", text);
        assert!(text.contains(&format!("https://p.example/sudoku/{}", older.id)));
        assert!(text.contains("won by <@7>"));
        assert!(!text.contains(&format!("#{} 🟢", done.id)), "a puzzle already finished isn't listed again");
        assert!(text.contains("<#55>") && text.contains("`/sudokutop`"));
        // Where the asker stands, today and this month.
        assert!(text.contains("**You today:** 2nd of 2 · **6 sudoku points** · 2 puzzles"), "{}", text);
        assert!(text.contains("**This month:** 1st of 2 · **84 sudoku points** · 21 puzzles"), "{}", text);
        // Somebody who hasn't won one today but has this month, and one who never has.
        let some = mine_text(Some(&live), &today, &month, 2, &[], None, None);
        assert!(some.contains("**You today:** 1st of 2 · **10 sudoku points**"), "{}", some);
        let none = mine_text(Some(&live), &today, &month, 99, &[], None, None);
        assert!(none.contains("haven't won one today") && !none.contains("This month"), "{}", none);
        // Nothing of your own, and no page set up.
        let bare = mine_text(Some(&live), &[], &[], 1, &[], None, None);
        assert!(bare.contains("Nothing else of yours is half finished") && bare.contains("VIZIER_PANEL_URL"), "{}", bare);
        assert!(mine_text(None, &[], &[], 1, &[], None, None).contains("No puzzle is up right now"));
    }

    fn tally(user: u64, points: i64, solves: i64, reached: i64) -> store::Tally {
        store::Tally { user, points, solves, reached }
    }

    #[test]
    fn the_sudoku_points_board_lists_ten_and_finds_the_asker_below_them() {
        let mut rows: Vec<store::Tally> = (1..=12).map(|i| tally(i, (40 - 2 * i) as i64, 3, 100 + i as i64)).collect();
        store::rank(&mut rows);
        let text = top_text("today", &rows, 12);
        assert!(text.starts_with("🧩 **Sudoku points** · today"), "{}", text);
        assert!(text.contains("\n🥇 <@1> **38** · 3 puzzles"), "{}", text);
        assert!(text.contains("\n🥈 <@2> **36**") && text.contains("\n🥉 <@3> **34**"), "{}", text);
        assert!(text.contains("\n`10.` <@10> **20** · 3 puzzles"), "{}", text);
        assert!(!text.contains("<@11>"), "only ten are listed: {}", text);
        // Outside the ten: their own line, and where they stand.
        assert!(text.contains("-# **You:** 12th of 12 · **16 sudoku points** · 3 puzzles"), "{}", text);
        // Inside the ten: marked in place, with no line of their own.
        let inside = top_text("today", &rows, 3);
        assert!(inside.contains("🥉 <@3> **34** · 3 puzzles ← you"), "{}", inside);
        assert!(!inside.contains("**You:**"), "{}", inside);
        // The board says what a sudoku point is, so nobody reads it as a house point.
        assert!(text.contains("no daily limit, and everyone has them. They don't move the House Cup."), "{}", text);
        assert!(top_text("September so far", &[], 1).contains("Nobody has solved one yet"));
        // Ordering is the store's: points, then puzzles, then who got there first.
        let mut close = vec![tally(1, 6, 1, 50), tally(2, 6, 2, 90), tally(3, 6, 2, 60)];
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
    fn the_card_moves_back_to_the_bottom_after_anything_lands_below_it() {
        // Nothing below the card.
        assert!(!move_due(Some(500), 500, 0, 10_000));
        assert!(!move_due(Some(500), 400, 0, 10_000), "an older message arriving late doesn't count");
        assert!(!move_due(None, 900, 0, 10_000), "no card, nothing to move");
        // A mod says something below it: the card waits for quiet, then moves.
        assert!(!move_due(Some(500), 900, 9_000, 9_000 + MOVE_DELAY_MS - 1));
        assert!(move_due(Some(500), 900, 9_000, 9_000 + MOVE_DELAY_MS));
        // And the move itself can never set off another: the new card is the
        // newest message there is.
        assert!(!move_due(Some(901), 901, 9_000, 999_999));
    }

    #[test]
    fn a_finish_is_worth_nothing_and_a_win_is_worth_its_points_less_the_hints() {
        assert_eq!(payout(Kind::Win, 6, 0, 1), 6);
        assert_eq!(payout(Kind::Win, 6, 2, 1), 4);
        assert_eq!(payout(Kind::Win, 2, 5, 1), 0);
        for hints in 0..4 {
            for points in [0, 2, 4, 6, 100] {
                assert_eq!(payout(Kind::Finish, points, hints, 1), 0, "a finish must never pay");
            }
        }
    }

    #[test]
    fn a_late_solver_is_told_they_were_right_and_the_puzzle_stays_won() {
        let conn = memory();
        let row = solve_row(&conn, Level::Medium, 4, 1_000);
        // The winner takes it; the second correct code is a finish.
        assert!(store::claim(&conn, row.id, 11, 1_120).unwrap());
        store::add_solve(&conn, "2026-09-16", 11, row.id, 0, 4, Kind::Win, row.level, 120, 1_120).unwrap();
        assert!(!store::claim(&conn, row.id, 22, 1_724).unwrap(), "the puzzle is already won");
        store::add_solve(&conn, "2026-09-16", 22, row.id, 0, 0, Kind::Finish, row.level, 724, 1_724).unwrap();
        let after = store::get(&conn, row.id).unwrap();
        assert_eq!((after.winner, after.seconds), (Some(11), Some(120)), "a late finish never moves the win");
        let day = store::day_solves(&conn, "2026-09-16");
        assert_eq!(day.iter().filter(|s| s.kind == Kind::Finish).count(), 1);
        assert_eq!(day.iter().map(|s| s.worth).sum::<i64>(), 4, "only the win scored");
        assert_eq!(day.iter().map(|s| s.points).sum::<i64>(), 0, "and the House Cup was paid nothing");
        // And the words the late solver sees.
        let words = format!(
            "✅ **Correct!** Puzzle #{} in **{}** — <@{}> got there first, so the sudoku points went to them.",
            row.id,
            spent_words(724),
            11
        );
        assert!(words.contains("12 min 04 s") && words.contains("sudoku points went to them"), "{}", words);
    }

    #[test]
    fn a_late_wrong_code_is_answered_the_same_way_and_counts_against_that_puzzle() {
        let conn = memory();
        let row = solve_row(&conn, Level::Easy, 2, 1_000);
        store::claim(&conn, row.id, 11, 1_100).unwrap();
        // Someone still working sends a wrong grid for the puzzle that has moved on.
        let mut attempt = row.solution;
        let blank = (0..puzzles::CELLS).find(|c| row.givens[*c] == 0).unwrap();
        attempt[blank] = if row.solution[blank] == 9 { 1 } else { row.solution[blank] + 1 };
        assert_eq!(check(&attempt, &row.solution), Verdict::Wrong(1));
        assert_eq!(store::add_try(&conn, row.id, 22, 2_000).unwrap(), 1);
        assert_eq!(store::add_try(&conn, row.id, 22, 2_010).unwrap(), 2, "tries count against that puzzle");
        let player = store::player(&conn, row.id, 22).unwrap();
        assert_eq!(may_try(Some(&player), 10, COOLDOWN_SECS, 2_015), Err(Refusal::Cooldown(5)));
        assert_eq!(may_try(Some(&player), 2, COOLDOWN_SECS, 9_999), Err(Refusal::NoTriesLeft(2)));
        // The live puzzle is untouched by any of it.
        let live = solve_row(&conn, Level::Hard, 6, 2_100);
        assert_eq!(store::player(&conn, live.id, 22), None);
        assert_eq!(store::live(&conn).map(|r| r.id), Some(live.id));
    }

    #[test]
    fn a_code_for_a_puzzle_past_the_window_is_turned_away_kindly() {
        let conn = memory();
        let day = 86_400;
        let old = solve_row(&conn, Level::Easy, 2, 1_000);
        let fresh = solve_row(&conn, Level::Medium, 4, 1_000 + 3 * day);
        let now = 1_000 + 3 * day + 60;
        // Inside the window, or among the last few: still checked.
        let keep = store::checkable(&conn, now - day, 10);
        assert!(keep.iter().any(|r| r.id == old.id));
        // Outside both: the code reads fine but there is nothing to check it against.
        let narrow = store::checkable(&conn, now - day, 1);
        assert!(!narrow.iter().any(|r| r.id == old.id));
        let code = sudoku_code::encode(old.id, &old.givens, &old.solution);
        let read = read_submission(&code, |id| narrow.iter().find(|r| r.id == id).map(|r| r.givens));
        assert_eq!(read, Submission::Bad(CodeError::OtherPuzzle(old.id)));
        // The live puzzle's own code still works.
        let live = sudoku_code::encode(fresh.id, &fresh.givens, &fresh.solution);
        assert_eq!(
            read_submission(&live, |id| narrow.iter().find(|r| r.id == id).map(|r| r.givens)),
            Submission::Code(fresh.id, fresh.solution)
        );
    }

    /// Writes the channel as Discord would show it — the live card, the Solved
    /// card and the next puzzle, with the real words and the real picture — so
    /// the flow can be looked at without a server:
    ///
    /// ```sh
    /// SUDOKU_SHOT_DIR=/tmp/sudoku-page cargo test discord_flow_to_disk -- --ignored
    /// ```
    #[test]
    #[ignore]
    fn discord_flow_to_disk() {
        use base64::Engine as _;
        let dir = std::env::var("SUDOKU_SHOT_DIR").unwrap_or_else(|_| "/tmp/sudoku-page".to_string());
        let conn = memory();
        let live = solve_row(&conn, Level::Medium, 4, 0);
        let next = solve_row(&conn, Level::Hard, 6, 0);
        let picture = |givens: &puzzles::Grid| {
            let mut fs = crate::channels::discord::awards::fonts().lock();
            let png = crate::channels::discord::sudoku_card::render(givens, &mut fs).expect("a picture");
            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png))
        };
        // The bot's own markdown, as Discord draws it.
        let markdown = |text: &str| {
            let mut out = String::new();
            for line in text.lines() {
                let small = line.starts_with("-# ");
                let line = line.trim_start_matches("-# ");
                let mut html = String::new();
                let mut bold = false;
                let mut chars = line.chars().peekable();
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        html.push_str(if bold { "</b>" } else { "<b>" });
                        bold = !bold;
                    } else if c == '<' {
                        html.push_str("&lt;");
                    } else if c == '>' {
                        html.push_str("&gt;");
                    } else {
                        html.push(c);
                    }
                }
                let html = html.replace("&lt;@42&gt;", "<span class=\"ping\">@Aarav</span>");
                out.push_str(&format!("<div class=\"{}\">{}</div>", if small { "small" } else { "line" }, html));
            }
            out
        };
        let card = |title: &str, body: String, footer: &str, colour: u32, image: Option<String>, buttons: &[(&str, &str)]| {
            let picture = image.map(|src| format!("<img src=\"{}\" alt=\"the grid\">", src)).unwrap_or_default();
            let buttons: String = buttons
                .iter()
                .map(|(style, label)| format!("<button class=\"btn {}\">{}</button>", style, label))
                .collect();
            format!(
                "<div class=\"msg\"><div class=\"avatar\">🔢</div><div class=\"body\"><div class=\"who\">Loduchand \
                 <span class=\"app\">APP</span> <span class=\"when\">Today at 9:02 PM</span></div>\
                 <div class=\"embed\" style=\"border-left-color:#{:06X}\"><div class=\"title\">{}</div>{}{}\
                 <div class=\"foot\">{}</div></div><div class=\"row\">{}</div></div></div>",
                colour,
                title,
                markdown(&body),
                picture,
                footer,
                buttons
            )
        };
        let live_card = card(
            &card_title(128),
            card_text(&Live { level: live.level, points: 4, playing: 5, open_secs: 374, hint_cost: 1 }),
            CARD_FOOTER,
            live.level.colour(),
            Some(picture(&live.givens)),
            &[("green", "▶️ Play"), ("blue", "📋 Submit code"), ("grey", "💡 Hint"), ("grey", "📊 Today's solvers")],
        );
        let solved = Solved {
            puzzle_id: 128,
            level: Level::Medium,
            winner: 42,
            badge: "🦅 Ravenclaw".into(),
            seconds: 461,
            points: 4,
            hints: 0,
            full_points: 4,
            all_hints: 2,
            others: 4,
            tally: 26,
            today: "Aarav 3 (+9) · Meera 2 (+6) · Rohan 1 (+2)".into(),
        };
        let solved_card = card(
            &solved_title(128, Level::Medium),
            solved_text(&solved),
            CARD_FOOTER,
            SOLVED_COLOUR,
            None,
            &[],
        );
        let next_card = card(
            &card_title(129),
            card_text(&Live { level: Level::Hard, points: 6, playing: 0, open_secs: 3, hint_cost: 1 }),
            CARD_FOOTER,
            Level::Hard.colour(),
            Some(picture(&next.givens)),
            &[("green", "▶️ Play"), ("blue", "📋 Submit code"), ("grey", "💡 Hint"), ("grey", "📊 Today's solvers")],
        );
        let whisper = |title: &str, body: String| {
            format!("<div class=\"msg only-you\"><div class=\"avatar\">🔢</div><div class=\"body\"><div class=\"who\">{}</div>{}\
                     <div class=\"small\">👁️ Only you can see this</div></div></div>", title, markdown(&body))
        };
        let play = whisper("Loduchand · in reply to ▶️ Play", play_text(129, Level::Hard, 6, Some("https://panel.mlci.example/sudoku/129"), &next.givens));
        let hint = whisper(
            "Loduchand · in reply to 💡 Hint",
            format!(
                "💡 **{} is {}** · hint **1/3** on puzzle #129\n-# This puzzle is now worth **5 sudoku points** to you. Only you can see this.",
                puzzles::cell_name(40),
                next.solution[40]
            ),
        );
        let solves = vec![
            store::Solve { user: 42, puzzle: 128, points: 0, worth: 4, kind: Kind::Win, level: Level::Medium, seconds: 461, ts: 10 },
            store::Solve { user: 43, puzzle: 128, points: 0, worth: 0, kind: Kind::Finish, level: Level::Medium, seconds: 903, ts: 20 },
        ];
        let today = whisper("Loduchand · in reply to 📊 Today's solvers", today_text(&solves, 43));
        let style = "body{background:#313338;color:#dbdee1;font:15px/1.4 'Helvetica Neue',Arial,sans-serif;margin:0;padding:24px;}\
             h2{color:#f2f3f5;font-size:15px;margin:26px 0 8px;text-transform:uppercase;letter-spacing:.04em;}\
             .chan{max-width:720px;background:#313338;border:1px solid #232428;border-radius:10px;overflow:hidden;}\
             .chanhead{background:#2b2d31;padding:10px 16px;color:#f2f3f5;font-weight:600;}\
             .msg{display:flex;gap:12px;padding:10px 16px;}\
             .only-you{background:#2b2d31;}\
             .avatar{width:38px;height:38px;border-radius:50%;background:#5865f2;display:grid;place-items:center;font-size:18px;flex:none;}\
             .who{color:#f2f3f5;font-weight:600;margin-bottom:3px;}\
             .app{background:#5865f2;color:#fff;font-size:10px;border-radius:3px;padding:1px 4px;vertical-align:2px;}\
             .when{color:#949ba4;font-size:12px;font-weight:400;}\
             .embed{background:#2b2d31;border-left:4px solid #5865f2;border-radius:4px;padding:10px 14px;max-width:460px;}\
             .title{color:#f2f3f5;font-weight:700;margin-bottom:6px;}\
             .line{margin:2px 0;}.small{color:#949ba4;font-size:13px;margin:2px 0;}\
             .embed img{display:block;margin:10px 0 4px;border-radius:6px;width:320px;max-width:100%;}\
             .foot{color:#949ba4;font-size:12px;margin-top:6px;}\
             .row{margin-top:8px;display:flex;gap:8px;flex-wrap:wrap;}\
             .btn{border:0;border-radius:4px;padding:8px 14px;font-size:14px;color:#fff;font-family:inherit;}\
             .green{background:#248046;}.blue{background:#5865f2;}.grey{background:#4e5058;}\
             .ping{background:rgba(88,101,242,.3);color:#c9cdfb;border-radius:3px;padding:0 2px;}";
        let page = format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Sudoku in #sudoku</title><style>{}</style>\
             <h2>1 · the channel, the card always last</h2><div class=\"chan\"><div class=\"chanhead\"># sudoku</div>{}</div>\
             <h2>2 · press Play</h2><div class=\"chan\"><div class=\"chanhead\"># sudoku</div>{}{}</div>\
             <h2>3 · solved → the next puzzle at once</h2><div class=\"chan\"><div class=\"chanhead\"># sudoku</div>{}{}</div>\
             <h2>4 · today's solvers</h2><div class=\"chan\"><div class=\"chanhead\"># sudoku</div>{}</div>",
            style, live_card, play, hint, solved_card, next_card, today
        );
        std::fs::create_dir_all(&dir).expect("a place to write");
        std::fs::write(std::path::Path::new(&dir).join("discord.html"), page).unwrap();
        println!("discord flow written to {}/discord.html", dir);
    }

    /// The one rule the whole change comes down to: nothing the game says to a
    /// player mentions house points. Not "+4 points", not "no points", not
    /// "that's your house points for today" — sudoku pays sudoku points, and the
    /// House Cup is not in it.
    #[test]
    fn nothing_the_game_says_to_a_player_mentions_house_points() {
        let p = generate(Level::Medium, &mut Rng::seeded(11));
        let solves = vec![
            store::Solve { user: 1, puzzle: 1, points: 0, worth: 4, kind: Kind::Win, level: Level::Medium, seconds: 120, ts: 10 },
            store::Solve { user: 2, puzzle: 1, points: 0, worth: 0, kind: Kind::Finish, level: Level::Medium, seconds: 400, ts: 20 },
        ];
        let conn = memory();
        let live = solve_row(&conn, Level::Medium, 4, 1_000);
        let board = vec![tally(1, 26, 7, 40), tally(2, 4, 1, 20)];
        let solved_spent = Solved { points: 0, hints: 4, tally: 6, ..solved() };
        let said: Vec<String> = vec![
            card_text(&Live { level: Level::Medium, points: 4, playing: 5, open_secs: 380, hint_cost: 1 }),
            CARD_FOOTER.to_string(),
            solved_title(128, Level::Medium),
            solved_text(&solved()),
            solved_text(&solved_spent),
            today_text(&solves, 1),
            today_text(&solves, 99),
            today_text(&[], 1),
            solver_line(&solves[0]),
            solver_line(&solves[1]),
            play_text(128, Level::Easy, 2, Some("https://p.example/sudoku/128"), &p.givens),
            play_text(128, Level::Easy, 2, None, &p.givens),
            mine_text(Some(&live), &board, &board, 1, &[(live.clone(), false)], Some("https://p.example"), Some(55)),
            mine_text(None, &[], &[], 99, &[], None, None),
            top_text("today", &board, 1),
            top_text("September so far", &[], 1),
            wrong_words(Verdict::Wrong(3), 2),
            wrong_words(Verdict::Unfinished(1), 0),
            OFF.to_string(),
            score_words(1),
            score_words(4),
        ];
        for text in &said {
            let low = text.to_lowercase();
            assert!(!low.contains("house point"), "sudoku still talks about house points:\n{}", text);
            assert!(!low.contains("house cup") || low.contains("don't move the house cup"), "{}", text);
            assert!(!text.contains("no points"), "a solve is never reported as \"no points\":\n{}", text);
        }
        // And what it DOES say is named: sudoku points, everywhere points are named.
        for text in said.iter().filter(|t| t.to_lowercase().contains("point")) {
            assert!(text.to_lowercase().contains("sudoku point"), "points named without saying whose:\n{}", text);
        }
        assert_eq!((score_words(1), score_words(0), score_words(4)), ("1 sudoku point".into(), "0 sudoku points".into(), "4 sudoku points".into()));
    }

    #[test]
    fn how_long_things_took_reads_naturally() {
        assert_eq!(spent_words(48), "48 s");
        assert_eq!(spent_words(461), "7 min 41 s");
        assert_eq!(spent_words(724), "12 min 04 s");
        assert_eq!(spent_words(3_840), "1 h 04 min");
        assert_eq!(spent_words(-5), "0 s");
        assert_eq!(open_words(4), "just now");
        assert_eq!(open_words(380), "6 min");
        assert_eq!(open_words(7_200), "2 h");
        assert_eq!(open_words(200_000), "2 days");
    }
}
