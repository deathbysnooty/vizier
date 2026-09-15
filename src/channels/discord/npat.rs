//! Name Place Animal Thing, in its own channel (`VIZIER_NPAT_CHANNEL`).
//!
//! Members can't type there: everything is the bot's posts, buttons and
//! pop-ups. From the top: a rules post written from the live settings, the
//! results of earlier letters and games, and the game card, always last.
//!
//! The lobby is idle until someone presses "I'm in", which opens a join window.
//! Once enough players from enough houses are in, a short countdown starts a
//! GAME of several letters. Each letter is a round: anyone in a house (or a
//! Muggle) presses "Submit answers" and types a Name, Place, Animal and Thing
//! in a private pop-up, as often as they like until the timer ends. One model
//! call judges every answer (see `npat_judge`); unique answers score 10 and
//! shared ones 5, and the letter's results card shows the game scores so far,
//! with Challenge and Review buttons for half an hour. After a short break the
//! next letter comes. After the last letter (or a letter nobody answered) a
//! final card ranks the whole game and its best two house members win house
//! points.
//!
//! One task drives everything ([`run`]): the buttons only change shared state
//! or the database, and the task does the posting and editing, so two presses
//! can never post two rounds. The same task keeps the game card last: any
//! message that lands below it moves it down a few seconds later.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serde_json::{Value, json};
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, ComponentInteractionDataKind, Context, CreateActionRow,
    CreateAllowedMentions, CreateButton, CreateCommand, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, EditMessage,
    GetMessages, GuildId, Message, MessageId, ModalInteraction, UserId,
};

use super::control;
use super::npat_judge::{self as judge, CATEGORIES, KEYS, Mark, Points, Prizes};
use super::npat_store::{self as store, GRACE_SECS, GameStatus, MUGGLE, Round, Status, Submit};
use super::points::{Outcome, Source};
use super::rules_text::{self, NpatRules};

/// How often the game task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How often the rules post is checked against the settings, in ticks.
const RULES_EVERY_TICKS: u64 = 60;
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// How long the model gets to judge a round before the letter-only check.
const JUDGE_WAIT: Duration = Duration::from_secs(20);
/// The live answer count is edited at most this often.
const COUNT_EDIT_SECS: i64 = 3;
/// How long the game card waits after the last message below it before moving down.
pub const MOVE_DELAY_MS: i64 = 3_000;
/// The shortest countdown once enough players are in, even at the very end of the join window.
pub const START_FLOOR_SECS: i64 = 10;
/// How long "Not enough players this time" stays up before the lobby goes idle.
pub const MISSED_SECS: i64 = 15;
/// Challenges and reviews stay open this long after a letter's results.
pub const REVIEW_SECS: i64 = 30 * 60;
/// Players listed on a results card before "…and N more".
pub const RESULT_LINES: usize = 25;
/// Kept clear of Discord's 4096 characters for an embed description.
pub const DESCRIPTION_LIMIT: usize = 4000;
/// Players named on the lobby card.
const LOBBY_NAMES: usize = 40;
const COLOUR: u32 = 0xF2A93B;
const FINAL_COLOUR: u32 = 0xE8B923;
const STOPPED_COLOUR: u32 = 0x4E5058;
pub const DEFAULT_LETTERS: &str = "ABCDEFGHIJKLMNOPRSTUVW";
/// A quick, cheap model on OpenRouter, where the bot's own model lives.
pub const DEFAULT_JUDGE_MODEL: &str = "google/gemini-2.5-flash-lite";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_NPAT", true)
}

fn rules_on() -> bool {
    control::on("VIZIER_NPAT_RULES", true)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_NPAT_CHANNEL").filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

fn min_players() -> usize {
    control::number("VIZIER_NPAT_MIN_PLAYERS", 5).clamp(1, 50) as usize
}

fn min_houses() -> usize {
    control::number("VIZIER_NPAT_MIN_HOUSES", 2).clamp(1, 4) as usize
}

fn join_secs() -> i64 {
    control::number("VIZIER_NPAT_JOIN_SECONDS", 180).clamp(30, 1800) as i64
}

fn start_secs() -> i64 {
    control::number("VIZIER_NPAT_START_SECONDS", 30).clamp(5, 600) as i64
}

fn letters() -> String {
    control::var("VIZIER_NPAT_LETTERS").filter(|l| !judge::letter_pool(l).is_empty()).unwrap_or_else(|| DEFAULT_LETTERS.to_string())
}

fn letters_per_game() -> i64 {
    control::number("VIZIER_NPAT_LETTERS_PER_GAME", 5).clamp(1, 10) as i64
}

fn round_secs() -> i64 {
    control::number("VIZIER_NPAT_SECONDS", 45).clamp(15, 600) as i64
}

fn break_secs() -> i64 {
    control::number("VIZIER_NPAT_BREAK_SECONDS", 20).clamp(3, 600) as i64
}

/// The model that judges answers: a model name on the bot's own provider, or
/// `None` for the bot's usual model (the setting says "agent").
fn judge_model() -> Option<String> {
    match control::var("VIZIER_NPAT_AI_MODEL") {
        None => Some(DEFAULT_JUDGE_MODEL.to_string()),
        Some(v) if matches!(v.to_ascii_lowercase().as_str(), "agent" | "default" | "none") => None,
        Some(v) => Some(v),
    }
}

fn min_scored() -> usize {
    control::number("VIZIER_NPAT_MIN_SCORED", 3).min(50) as usize
}

/// What answers score in a letter right now (game score, not house points).
pub fn points() -> Points {
    Points {
        unique: control::number("VIZIER_NPAT_SCORE_UNIQUE", 10).min(1000) as i64,
        shared: control::number("VIZIER_NPAT_SCORE_SHARED", 5).min(1000) as i64,
    }
}

/// House points for a game's 1st and 2nd right now.
pub fn prizes() -> Prizes {
    Prizes {
        first: control::number("VIZIER_POINTS_NPAT_1ST", 2).min(100) as i64,
        second: control::number("VIZIER_POINTS_NPAT_2ND", 1).min(100) as i64,
    }
}

/// The game's channel when the game is on.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on())
}

fn daily_cap() -> Option<i64> {
    match Source::Npat.cap() {
        super::points::Cap::PerDay(n) => Some(n),
        _ => None,
    }
}

/// Every setting the rules post and the House Cup posts mention, as they are now.
pub fn npat_rules(cap: Option<i64>) -> NpatRules {
    let (p, prizes) = (points(), prizes());
    NpatRules {
        channel: live_channel(),
        min_players: min_players() as i64,
        min_houses: min_houses() as i64,
        join_secs: join_secs(),
        start_secs: start_secs(),
        letters: judge::letter_pool(&letters()).into_iter().collect(),
        letters_per_game: letters_per_game(),
        round_secs: round_secs(),
        break_secs: break_secs(),
        scores: [p.unique, p.shared],
        prizes: [prizes.first, prizes.second],
        cap,
        min_scored: min_scored() as i64,
    }
}

// --- players ----------------------------------------------------------------------------------

/// A player and the key of their house (or [`MUGGLE`]).
pub type Player = (u64, &'static str);

/// How many different houses the players come from. Muggles count as players
/// but not as a house.
pub fn house_count(players: &[Player]) -> usize {
    let mut seen: Vec<&str> = Vec::new();
    for (_, house) in players.iter().filter(|(_, h)| *h != MUGGLE) {
        if !seen.contains(house) {
            seen.push(house);
        }
    }
    seen.len()
}

/// Whether these players can start a game: enough of them, from enough houses.
pub fn enough_players(players: &[Player], min: usize, min_houses: usize) -> bool {
    players.len() >= min.max(1) && house_count(players) >= min_houses.max(1)
}

/// Who may play: a house member (their house's key) or a Muggle ([`MUGGLE`]).
/// `None` for someone who is neither.
fn player_house(user: u64) -> Option<&'static str> {
    if super::house::opted_out(user) {
        return Some(MUGGLE);
    }
    super::house::house_of(user).map(|h| h.key)
}

/// How a player shows on results: a crest, or "🧙 Muggle".
pub fn badge(house: &str) -> String {
    if house == MUGGLE {
        return "🧙 Muggle".to_string();
    }
    super::house::house(house).map(|h| h.crest.to_string()).unwrap_or_default()
}

// --- the lobby ----------------------------------------------------------------------------

/// Where the lobby is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LobbyState {
    /// Nobody has pressed I'm in.
    #[default]
    Idle,
    /// Someone pressed it: the game starts by `closes_at` if enough are in.
    Open { closes_at: i64 },
    /// Enough are in: the game starts at `starts_at`; more can still join.
    Starting { starts_at: i64, closes_at: i64 },
    /// The window ran out without enough players; idle again at `until`.
    Missed { until: i64 },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lobby {
    pub joined: Vec<Player>,
    pub state: LobbyState,
}

/// The lobby's settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub min: usize,
    pub min_houses: usize,
    pub join_secs: i64,
    pub start_secs: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LobbyEvent {
    Nothing,
    /// The first player opened the join window.
    Opened,
    /// Enough players: the countdown shortened.
    Enough,
    /// Someone left and it's no longer enough: back to the join window.
    NotEnoughAnymore,
    /// The window ended without enough players.
    Missed,
    /// Idle again: everyone left, or the "not enough" notice is done.
    BackToIdle,
    Start(Vec<u64>),
}

/// When the game starts once enough are in: `start_secs` from now, but never
/// later than the window would have closed, nor sooner than [`START_FLOOR_SECS`].
pub fn start_time(now: i64, closes_at: i64, start_secs: i64) -> i64 {
    (now + start_secs).min(closes_at.max(now + start_secs.min(START_FLOOR_SECS)))
}

impl Lobby {
    /// Joins or leaves; true when now in.
    pub fn toggle(&mut self, user: u64, house: &'static str) -> bool {
        if let Some(i) = self.joined.iter().position(|(u, _)| *u == user) {
            self.joined.remove(i);
            false
        } else {
            self.joined.push((user, house));
            true
        }
    }

    /// What a join or leave does straight away: opening the window, starting
    /// or calling off the countdown, going idle when everyone left. Never
    /// starts a game.
    pub fn settle(&mut self, now: i64, t: Timing) -> LobbyEvent {
        if self.joined.is_empty() {
            return match self.state {
                LobbyState::Open { .. } | LobbyState::Starting { .. } => {
                    self.state = LobbyState::Idle;
                    LobbyEvent::BackToIdle
                }
                _ => LobbyEvent::Nothing,
            };
        }
        let ready = enough_players(&self.joined, t.min, t.min_houses);
        match self.state {
            LobbyState::Idle | LobbyState::Missed { .. } => {
                let closes_at = now + t.join_secs;
                if ready {
                    self.state = LobbyState::Starting { starts_at: start_time(now, closes_at, t.start_secs), closes_at };
                    LobbyEvent::Enough
                } else {
                    self.state = LobbyState::Open { closes_at };
                    LobbyEvent::Opened
                }
            }
            LobbyState::Open { closes_at } if ready => {
                self.state = LobbyState::Starting { starts_at: start_time(now, closes_at, t.start_secs), closes_at };
                LobbyEvent::Enough
            }
            LobbyState::Starting { closes_at, .. } if !ready => {
                self.state = LobbyState::Open { closes_at };
                LobbyEvent::NotEnoughAnymore
            }
            _ => LobbyEvent::Nothing,
        }
    }

    /// What the clock does to the lobby.
    pub fn tick(&mut self, now: i64, t: Timing) -> LobbyEvent {
        let settled = self.settle(now, t);
        if settled != LobbyEvent::Nothing {
            return settled;
        }
        match self.state {
            LobbyState::Starting { starts_at, .. } if now >= starts_at => {
                self.state = LobbyState::Idle;
                LobbyEvent::Start(std::mem::take(&mut self.joined).into_iter().map(|(u, _)| u).collect())
            }
            LobbyState::Open { closes_at } if now >= closes_at => {
                self.joined.clear();
                self.state = LobbyState::Missed { until: now + MISSED_SECS };
                LobbyEvent::Missed
            }
            LobbyState::Missed { until } if now >= until => {
                self.state = LobbyState::Idle;
                LobbyEvent::BackToIdle
            }
            _ => LobbyEvent::Nothing,
        }
    }
}

// --- keeping the card last ------------------------------------------------------------------------

/// Whether the game card should move down now: something newer than it landed
/// in the channel (message ids grow with time) and the channel has been quiet
/// for `delay_ms` since. The card's own post is never newer than itself, so a
/// move can't set off another.
pub fn move_due(card: Option<u64>, latest: u64, last_seen_ms: i64, now_ms: i64, delay_ms: i64) -> bool {
    card.is_some_and(|c| latest > c) && now_ms - last_seen_ms >= delay_ms
}

#[derive(Default)]
struct Shared {
    lobby: Lobby,
    /// The lobby card needs editing.
    lobby_dirty: bool,
    /// Between games: I'm in works.
    in_lobby: bool,
    /// (channel, message) of the game card: the lobby, a letter, or the break line.
    card: Option<(u64, u64)>,
    /// The newest message id seen in the channel.
    latest: u64,
    /// When a message last landed in the channel.
    last_seen_ms: i64,
    /// The card was deleted by someone.
    card_gone: bool,
    /// The rules post, to notice it being deleted.
    rules_message: Option<u64>,
    rules_check: bool,
    /// A mod asked to stop.
    stop: bool,
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

/// A deleted game card is posted again; a deleted rules post too.
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

// --- words -----------------------------------------------------------------------------------

/// Text that sits inside markdown without breaking it.
fn md(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '~' | '|' | '<' | '>' | '\\' | '#')).collect::<String>().trim().to_string()
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "🥇 +2 🥈 +1 house points", leaving out a place worth nothing.
fn prize_words(prizes: [i64; 2]) -> String {
    let mut parts = Vec::new();
    if prizes[0] > 0 {
        parts.push(format!("🥇 +{}", prizes[0]));
    }
    if prizes[1] > 0 {
        parts.push(format!("🥈 +{}", prizes[1]));
    }
    if parts.is_empty() { "no house points".to_string() } else { format!("{} house points", parts.join(" ")) }
}

/// The judging rulebook in one line, for the lobby card.
pub const RULES_IN_SHORT: &str = "Real names · real places on a map · real living animals · real things you can touch, no brands · Hindi and Hinglish welcome";

/// "**5 players** from **2+ houses**", or just the players when one house is enough.
fn needs_words(n: &NpatRules) -> String {
    let players = format!("**{}**", plural(n.min_players, "player", "players"));
    if n.min_houses > 1 { format!("{} from **{}+ houses**", players, n.min_houses) } else { players }
}

/// The lobby card's text.
pub fn lobby_text(joined: &[Player], state: LobbyState, n: &NpatRules) -> String {
    let mut text = format!(
        "A game is **{}**. For each, find a **Name, Place, Animal and Thing** starting with it in **{} s**.\n\
         ✍️ Answers go in a private pop-up · unique **{}** · shared **{}** · the game's best two house members win {}\n\
         -# {} · 📜 full rules above\n\n",
        plural(n.letters_per_game, "letter", "letters"),
        n.round_secs,
        n.scores[0],
        n.scores[1],
        prize_words(n.prizes),
        RULES_IN_SHORT
    );
    let houses = house_count(joined) as i64;
    match state {
        LobbyState::Idle => text.push_str(&format!("▶️ Press **✋ I'm in** to start a game · needs {}", needs_words(n))),
        LobbyState::Open { closes_at } => {
            text.push_str(&format!("⏳ Game starts <t:{}:R> if {} are in", closes_at, needs_words(n)));
            if joined.len() as i64 >= n.min_players && houses < n.min_houses {
                text.push_str("\n-# Waiting for someone from another house");
            }
        }
        LobbyState::Starting { starts_at, .. } => {
            text.push_str(&format!("✅ **Enough players!** Starting <t:{}:R> — jump in now", starts_at))
        }
        LobbyState::Missed { .. } => text.push_str("😴 Not enough players this time. Press **✋ I'm in** to try again."),
    }
    if !joined.is_empty() {
        let mut crests: Vec<&str> = Vec::new();
        for (_, key) in joined.iter().filter(|(_, h)| *h != MUGGLE) {
            let crest = super::house::house(key).map(|h| h.crest).unwrap_or("🏠");
            if !crests.contains(&crest) {
                crests.push(crest);
            }
        }
        let houses_words = if houses == 0 || n.min_houses <= 1 { String::new() } else { format!(" · {} {}", crests.join(""), plural(houses, "house", "houses")) };
        text.push_str(&format!("\n\n**{}/{} players{}**\n", joined.len(), n.min_players, houses_words));
        let names: Vec<String> = joined.iter().take(LOBBY_NAMES).map(|(u, _)| format!("<@{}>", u)).collect();
        text.push_str(&names.join(" · "));
        if joined.len() > LOBBY_NAMES {
            text.push_str(&format!(" · and {} more", joined.len() - LOBBY_NAMES));
        }
    }
    text
}

fn lobby_embed(joined: &[Player], state: LobbyState) -> CreateEmbed {
    CreateEmbed::new().title("🔤 Name · Place · Animal · Thing").description(lobby_text(joined, state, &npat_rules(daily_cap()))).colour(COLOUR)
}

fn lobby_row() -> CreateActionRow {
    CreateActionRow::Buttons(vec![CreateButton::new("npatjoin").label("✋ I'm in").style(ButtonStyle::Primary)])
}

fn lobby_message() -> CreateMessage {
    let (joined, state) = {
        let mut s = SHARED.lock();
        s.lobby_dirty = false;
        (s.lobby.joined.clone(), s.lobby.state)
    };
    CreateMessage::new().embed(lobby_embed(&joined, state)).components(vec![lobby_row()]).allowed_mentions(CreateAllowedMentions::new())
}

/// How a letter's card reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundState {
    Live,
    Judging,
    Stopped,
}

pub fn round_title(game_id: i64, letter_no: i64, letters: i64) -> String {
    format!("🔤 Game {} · Letter {}/{}", game_id, letter_no, letters)
}

pub fn round_text(letter: char, ends_at: i64, answers: usize, state: RoundState) -> String {
    let head = format!("## Letter **{}**\n**Name · Place · Animal · Thing**, each starting with **{}**\n", letter, letter);
    let count = format!("📝 **{}** in", plural(answers as i64, "answer", "answers"));
    let tail = match state {
        RoundState::Live => format!("⏱️ Ends <t:{}:R>\n{}", ends_at, count),
        RoundState::Judging => format!("⏱️ Time's up · {} · judging…", count),
        RoundState::Stopped => "🛑 Stopped by a mod · no house points".to_string(),
    };
    head + &tail
}

fn round_embed(round: &Round, letters: i64, answers: usize, state: RoundState) -> CreateEmbed {
    CreateEmbed::new()
        .title(round_title(round.game_id, round.letter_no, letters))
        .description(round_text(round.letter, round.ends_at, answers, state))
        .colour(if state == RoundState::Stopped { STOPPED_COLOUR } else { COLOUR })
}

fn round_row(round_id: i64, open: bool) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("npatsubmit:{}", round_id)).label("✍️ Submit answers").style(ButtonStyle::Success).disabled(!open),
    ])
}

fn game_letters(game_id: i64) -> i64 {
    store::db().and_then(|db| store::get_game(&db.lock(), game_id)).map(|g| g.letters).unwrap_or_else(letters_per_game)
}

fn round_message(round: &Round, answers: usize, state: RoundState) -> CreateMessage {
    CreateMessage::new()
        .embed(round_embed(round, game_letters(round.game_id), answers, state))
        .components(vec![round_row(round.id, state == RoundState::Live)])
        .allowed_mentions(CreateAllowedMentions::new())
}

/// The line that is the game card between two letters.
pub fn break_text(next: i64, letters: i64, until: i64) -> String {
    format!("⏭️ Letter **{}/{}** starts <t:{}:R>", next, letters, until)
}

/// The answers pop-up, as the raw interaction response: four short boxes,
/// each optional, filled with what was sent before.
pub fn modal_json(round_id: i64, letter: char, previous: Option<&[String; 4]>) -> Value {
    modal_json_timed(round_id, letter, previous, None)
}

/// The pop-up with its countdown: a live timer line on top (Discord counts
/// it down by itself) and the seconds left when it opened in the title, which
/// still shows if the timer line isn't. `timer` is (ends_at, now).
pub fn modal_json_timed(round_id: i64, letter: char, previous: Option<&[String; 4]>, timer: Option<(i64, i64)>) -> Value {
    let mut rows: Vec<Value> = (0..4)
        .map(|i| {
            let mut input = json!({
                "type": 4, "custom_id": KEYS[i], "label": CATEGORIES[i], "style": 1,
                "required": false, "max_length": judge::MAX_ANSWER,
                "placeholder": format!("{} starting with {}", CATEGORIES[i].to_lowercase(), letter),
            });
            if let Some(value) = previous.map(|p| judge::tidy(&p[i])).filter(|v| !v.is_empty()) {
                input["value"] = json!(value);
            }
            json!({ "type": 1, "components": [input] })
        })
        .collect();
    let title = match timer {
        Some((ends_at, now)) => format!("Letter {} · {}s left", letter, (ends_at - now).max(0)),
        None => format!("Letter {}", letter),
    };
    // The live timer line is a newer kind of pop-up block that has crashed some
    // Discord apps, so it's off unless switched on; the title always has the time.
    let timer_line = super::control::on("VIZIER_NPAT_POPUP_TIMER", false);
    if let Some((ends_at, _)) = timer.filter(|_| timer_line && !PLAIN_MODAL.load(std::sync::atomic::Ordering::Relaxed)) {
        rows.insert(0, json!({ "type": 10, "content": format!("⏱️ **Time's up <t:{}:R>** · everything starting with **{}**", ends_at, letter) }));
    }
    json!({ "type": 9, "data": { "custom_id": format!("npatans:{}", round_id), "title": title, "components": rows } })
}

/// Set when Discord refuses the pop-up with a timer line, so later pop-ups skip it.
static PLAIN_MODAL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The four answers from a submitted pop-up's `data`, whether the boxes came
/// back inside action rows (`components`) or labels (`component`). A box that
/// isn't there is blank.
pub fn answers_from(data: &Value) -> [String; 4] {
    fn walk(v: &Value, key: &str) -> Option<String> {
        match v {
            Value::Object(map) => {
                if map.get("custom_id").and_then(Value::as_str) == Some(key) {
                    if let Some(value) = map.get("value").and_then(Value::as_str) {
                        return Some(value.to_string());
                    }
                }
                ["component", "components"].iter().filter_map(|k| map.get(*k)).find_map(|child| walk(child, key))
            }
            Value::Array(items) => items.iter().find_map(|child| walk(child, key)),
            _ => None,
        }
    }
    let root = data.get("components").unwrap_or(data);
    KEYS.map(|key| walk(root, key).map(|v| judge::tidy(&v)).unwrap_or_default())
}


/// One player's line on a letter's results card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LetterLine {
    pub user: u64,
    /// A crest, "🧙 Muggle", or empty.
    pub badge: String,
    pub answers: [String; 4],
    pub marks: [Mark; 4],
    /// This letter's score.
    pub score: i64,
    /// The game score so far, this letter included.
    pub game_total: i64,
}

pub fn letter_line(line: &LetterLine) -> String {
    let crest = if line.badge.is_empty() { String::new() } else { format!(" {}", line.badge) };
    let parts: Vec<String> = (0..4)
        .map(|i| match line.marks[i] {
            Mark::Blank => "—".to_string(),
            mark => {
                let text = md(&line.answers[i]);
                let text = if text.is_empty() { "?".to_string() } else { text };
                format!("{} {}", text, mark.emoji())
            }
        })
        .collect();
    format!("<@{}>{} — {} · **{}** · game **{}**", line.user, crest, parts.join(" · "), line.score, line.game_total)
}

/// Lines under a size limit: at most [`RESULT_LINES`], then "…and N more",
/// leaving `reserve` characters for what follows.
fn fit_lines(rows: &[String], reserve: usize) -> String {
    let mut text = String::new();
    let mut shown = 0;
    for row in rows.iter().take(RESULT_LINES) {
        if text.chars().count() + row.chars().count() + 1 + reserve + 40 > DESCRIPTION_LIMIT {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(row);
        shown += 1;
    }
    if rows.len() > shown {
        text.push_str(&format!("\n…and {} more", rows.len() - shown));
    }
    text
}

/// A letter's results card: (title, description, footer). Lines are expected
/// best game score first.
#[allow(clippy::too_many_arguments)]
pub fn letter_results_text(game_id: i64, letter_no: i64, letters: i64, letter: char, lines: &[LetterLine], letter_only: bool, p: Points) -> (String, String, String) {
    let title = format!("Letter {} · {}/{} · Game {}", letter, letter_no, letters, game_id);
    let mut notes = Vec::new();
    if letter_only {
        notes.push("-# ⚠️ Checked by letter only: the judge couldn't be reached".to_string());
    }
    if !lines.is_empty() {
        notes.push(format!("-# 🎯 Letter {}/{} · the last number is the game score so far · house points are paid when the game ends", letter_no, letters));
        notes.push("-# ⚖️ Think an answer was judged wrong? Press Challenge within 30 min".to_string());
    }
    let notes = notes.join("\n");
    let mut text = if lines.is_empty() {
        "Nobody answered this letter.".to_string()
    } else {
        let rows: Vec<String> = lines.iter().map(letter_line).collect();
        fit_lines(&rows, notes.chars().count())
    };
    if !notes.is_empty() {
        text.push('\n');
        text.push_str(&notes);
    }
    let footer = format!("✅ unique {} · 🟰 shared {} · ❌ 0", p.unique, p.shared);
    (title, text, footer)
}

/// One player's line on a game's final card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalLine {
    pub user: u64,
    pub badge: String,
    pub total: i64,
    /// Position in the game, from 1.
    pub rank: usize,
    /// House points won.
    pub prize: i64,
}

pub fn final_line(line: &FinalLine) -> String {
    let medal = match (line.rank, line.total > 0) {
        (1, true) => "🥇".to_string(),
        (2, true) => "🥈".to_string(),
        (n, _) => format!("{}.", n),
    };
    let crest = if line.badge.is_empty() { String::new() } else { format!(" {}", line.badge) };
    let prize = if line.prize > 0 { format!(" · 🏠 +{}", line.prize) } else { String::new() };
    format!("{} <@{}>{} — **{}**{}", medal, line.user, crest, line.total, prize)
}

/// A game's final card: (title, description, footer). Lines best first.
#[allow(clippy::too_many_arguments)]
pub fn final_results_text(game_id: i64, letters: &[char], lines: &[FinalLine], pays: bool, ended_early: bool, n: &NpatRules) -> (String, String, String) {
    let title = format!("🏁 Game {} · final results", game_id);
    let played: Vec<String> = letters.iter().map(|c| c.to_string()).collect();
    let mut head = format!("Letters: **{}**", if played.is_empty() { "—".to_string() } else { played.join(" · ") });
    if ended_early {
        head.push_str(&format!("\n-# Ended early, after letter {} of {}", played.len(), n.letters_per_game));
    }
    let mut notes = Vec::new();
    let winners: Vec<String> = lines.iter().filter(|l| l.prize > 0).map(|l| format!("<@{}> +{}", l.user, l.prize)).collect();
    if !winners.is_empty() {
        notes.push(format!("🏠 **House points:** {}", winners.join(" · ")));
    }
    if pays && lines.iter().any(|l| l.rank <= 2 && l.total > 0 && l.badge == "🧙 Muggle") {
        notes.push("-# 🧙 Muggles keep their place, but house points go to the two best house members".to_string());
    }
    if !lines.is_empty() && !pays {
        notes.push(format!("-# Fewer than {} played this game, so it pays no house points", plural(n.min_scored, "person", "people")));
    }
    if !lines.is_empty() {
        notes.push("-# ⚖️ Each letter's results can still be challenged for 30 min; places and house points update by themselves".to_string());
    }
    let notes = notes.join("\n");
    let body = if lines.is_empty() {
        "Nobody played this game.".to_string()
    } else {
        let rows: Vec<String> = lines.iter().map(final_line).collect();
        fit_lines(&rows, notes.chars().count() + head.chars().count())
    };
    let mut text = format!("{}\n\n{}", head, body);
    if !notes.is_empty() {
        text.push('\n');
        text.push_str(&notes);
    }
    let limit = match n.cap {
        Some(c) => format!("up to {} a day", c),
        None => "no daily limit".to_string(),
    };
    let footer = format!("{} · {}", prize_words(n.prizes), limit);
    (title, text, footer)
}

fn results_rows(round_id: i64, challenged: usize, open: bool) -> Vec<CreateActionRow> {
    let review = if challenged > 0 { format!("🛡️ Review · {} challenged", challenged) } else { "🛡️ Review".to_string() };
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("npatchal:{}", round_id)).label("⚖️ Challenge").style(ButtonStyle::Secondary).disabled(!open),
        CreateButton::new(format!("npatrev:{}", round_id)).label(review).style(ButtonStyle::Secondary).disabled(!open),
    ])]
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("npatstop").description("admin only: stop the Name Place Animal Thing game, no points, back to the lobby")
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
        serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(r)) => r.status_code.as_u16() == 404 || r.error.code == 10008,
        _ => false,
    }
}

/// Whether a message is still there; an error other than "unknown message"
/// counts as there, so a hiccup never causes a duplicate post.
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
    let name = ctx.cache.guilds().iter().find_map(|g| {
        ctx.cache.guild(*g).and_then(|guild| guild.channels.get(&ChannelId::new(channel)).map(|c| c.name.to_lowercase()))
    });
    name.is_some_and(|n| n.contains("safe-corner"))
}

/// The channel to play in right now, if the game is on.
fn active_channel(ctx: &Context) -> Option<u64> {
    live_channel().filter(|c| !is_safe_corner(ctx, *c))
}

async fn delete(ctx: &Context, channel: u64, message: u64) {
    if let Err(err) = call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        tracing::debug!("npat: message {} not deleted: {}", message, err);
    }
}

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

fn cached_name(ctx: &Context, guild: Option<GuildId>, user: u64) -> String {
    guild
        .and_then(|g| ctx.cache.guild(g).and_then(|g| g.members.get(&UserId::new(user)).map(|m| m.display_name().to_string())))
        .unwrap_or_else(|| format!("member {}", user))
}

/// Points already in the ledger under a key, for a replayed payout.
fn ledger_points(key: &str) -> Option<i64> {
    let db = super::house::db()?;
    db.lock().query_row("SELECT points FROM ledger WHERE dedupe = ?1", rusqlite::params![key], |r| r.get(0)).ok()
}

/// Writes to the ledger and returns what it credited.
fn credit(user: u64, amount: i64, reason: &str, by: Option<u64>, key: String) -> i64 {
    match super::house::award_person(user, Source::Npat, amount, reason, by, Some(key.clone()), None) {
        Some((_, Outcome::Granted(n))) => n,
        Some((_, Outcome::Duplicate)) => ledger_points(&key).unwrap_or(0),
        _ => 0,
    }
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), key, value);
    }
}

fn meta_get(key: &str) -> Option<String> {
    store::db().and_then(|db| store::meta_get(&db.lock(), key)).filter(|v| !v.is_empty())
}

// --- the game card ---------------------------------------------------------------------------

/// Posts a new game card and removes the old one. Returns the new message id.
async fn place_card(ctx: &Context, channel: u64, message: CreateMessage) -> Option<u64> {
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let id = posted.id.get();
            let old = {
                let mut s = SHARED.lock();
                s.latest = s.latest.max(id);
                s.card.replace((channel, id))
            };
            meta_set("card", &format!("{}:{}", channel, id));
            if let Some((c, m)) = old.filter(|(_, m)| *m != id) {
                delete(ctx, c, m).await;
            }
            Some(id)
        }
        Err(err) => {
            tracing::warn!("npat: game card not posted in {}: {}", channel, err);
            None
        }
    }
}

/// Takes the game card away without a new one, deleting it unless `keep`
/// (a stopped letter's card stays as history).
async fn drop_card(ctx: &Context, keep: bool) {
    let card = SHARED.lock().card.take();
    meta_set("card", "");
    if let (Some((c, m)), false) = (card, keep) {
        delete(ctx, c, m).await;
    }
}

async fn edit_lobby(ctx: &Context) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let (joined, state) = {
        let mut s = SHARED.lock();
        s.lobby_dirty = false;
        (s.lobby.joined.clone(), s.lobby.state)
    };
    let edit = EditMessage::new().embed(lobby_embed(&joined, state)).components(vec![lobby_row()]).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        if is_gone(&err) {
            SHARED.lock().card_gone = true;
        }
        tracing::warn!("npat: lobby card not edited: {}", err);
    }
}

/// Back to an idle lobby.
fn reset_lobby() {
    let mut s = SHARED.lock();
    s.lobby = Lobby::default();
    s.lobby_dirty = true;
}

// --- the rules post ------------------------------------------------------------------------------

/// Keeps the rules post matching the settings: posts it when missing, edits it
/// when the words changed, removes it when switched off or the channel moved.
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
    let n = npat_rules(daily_cap());
    let plain = rules_text::npat_rules_message(&n);
    let body = rules_text::npat_rules_text(&n);
    let hash = rules_text::digest(&[if plain.is_some() { "text" } else { "embed" }, rules_text::NPAT_RULES_TITLE, &body]);
    let current = old.filter(|_| old_channel == Some(channel));
    let present = match current {
        Some(id) => exists(ctx, channel, id).await,
        None => false,
    };
    let embed = || CreateEmbed::new().title(rules_text::NPAT_RULES_TITLE).description(body.clone()).colour(FINAL_COLOUR);
    if !present {
        let message = match &plain {
            Some(text) => CreateMessage::new().content(text.clone()),
            None => CreateMessage::new().embed(embed()),
        }
        .allowed_mentions(CreateAllowedMentions::new());
        match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
            Ok(posted) => {
                meta_set("rules_message", &posted.id.get().to_string());
                meta_set("rules_channel", &channel.to_string());
                meta_set("rules_hash", &hash);
                SHARED.lock().rules_message = Some(posted.id.get());
                tracing::info!("npat: rules post up in {}", channel);
            }
            Err(err) => tracing::warn!("npat: rules post not posted in {}: {}", channel, err),
        }
        return;
    }
    SHARED.lock().rules_message = current;
    if meta_get("rules_hash").as_deref() == Some(hash.as_str()) {
        return;
    }
    let Some(id) = current else { return };
    let edit = match &plain {
        Some(text) => EditMessage::new().content(text.clone()).embeds(Vec::new()),
        None => EditMessage::new().content("").embeds(vec![embed()]),
    }
    .allowed_mentions(CreateAllowedMentions::new());
    match call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(id), edit)).await {
        Ok(_) => {
            meta_set("rules_hash", &hash);
            tracing::info!("npat: rules post updated");
        }
        Err(err) => tracing::warn!("npat: rules post not edited: {}", err),
    }
}

// --- rounds ---------------------------------------------------------------------------------------

/// Starts letter `letter_no` of a game and makes its card the game card.
async fn start_round(ctx: &Context, game_id: i64, letter_no: i64, channel: u64) -> Option<Round> {
    let now = Utc::now().timestamp();
    let round = {
        let db = store::db()?;
        let conn = db.lock();
        let recent = store::recent_letters(&conn, judge::RECENT_LETTERS);
        let letter = judge::pick_letter(&letters(), &recent, rand::random::<f64>())?;
        match store::start_round(&conn, game_id, letter_no, channel, letter, now, round_secs()) {
            Ok(round) => round,
            Err(err) => {
                tracing::warn!("npat: letter {} of game {} not started: {}", letter_no, game_id, err);
                return None;
            }
        }
    };
    match place_card(ctx, channel, round_message(&round, 0, RoundState::Live)).await {
        Some(id) => {
            if let Some(db) = store::db() {
                let _ = store::set_message(&db.lock(), round.id, id);
            }
            tracing::info!("npat: game {} letter {} ({}) started in {}", game_id, letter_no, round.letter, channel);
            Some(Round { message: Some(id), ..round })
        }
        None => {
            if let Some(db) = store::db() {
                let conn = db.lock();
                let _ = conn.execute("UPDATE rounds SET status = 'stopped' WHERE id = ?1", rusqlite::params![round.id]);
            }
            None
        }
    }
}

async fn edit_round(ctx: &Context, round: &Round, answers: usize, state: RoundState) {
    let Some(message) = round.message else { return };
    let edit = EditMessage::new()
        .embed(round_embed(round, game_letters(round.game_id), answers, state))
        .components(vec![round_row(round.id, state == RoundState::Live)])
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(round.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("npat: round {} card not edited: {}", round.id, err);
    }
}

/// Judges a letter whose time is up and saves its verdicts and scores. False
/// when it was stopped (or already finished) meanwhile.
async fn judge_round(round_id: i64) -> bool {
    let Some(db) = store::db() else { return false };
    let (round, entries) = {
        let conn = db.lock();
        if !store::begin_judging(&conn, round_id).unwrap_or(false) {
            return false;
        }
        let Some(round) = store::get_round(&conn, round_id) else { return false };
        (round, store::submissions(&conn, round_id))
    };
    let answers: Vec<[String; 4]> = entries.iter().map(|(_, a)| a.clone()).collect();
    let (items, map) = judge::items(&answers);
    let cached = store::cached_for(&db.lock(), round.letter, &items);
    let fresh: Vec<judge::Item> = judge::uncached(&items, &cached).into_iter().cloned().collect();
    let parsed = if fresh.is_empty() {
        None
    } else {
        let prompt = judge::prompt(round.letter, &fresh);
        // A dropped connection to the model is common enough to be worth one retry.
        let mut reply = None;
        for attempt in 1..=2 {
            match tokio::time::timeout(JUDGE_WAIT, control::web::ask_bot_model_with(prompt.clone(), judge_model())).await {
                Ok(Ok(r)) => {
                    reply = Some(r);
                    break;
                }
                Ok(Err(err)) => tracing::warn!("npat: round {} judge call failed (try {}): {}", round_id, attempt, err),
                Err(_) => tracing::warn!("npat: round {} judge took over {}s (try {})", round_id, JUDGE_WAIT.as_secs(), attempt),
            }
            if attempt == 1 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        let parsed = reply.as_deref().and_then(|r| {
            let parsed = judge::parse_verdicts(r);
            if parsed.is_none() {
                tracing::warn!("npat: round {} judge reply unreadable: {}", round_id, r.chars().take(300).collect::<String>());
            }
            parsed
        });
        if let Some(p) = &parsed {
            let missing = fresh.iter().filter(|i| !p.contains_key(&i.id)).count();
            if missing > 0 {
                tracing::warn!("npat: round {} judge left out {} of {} answers; those were checked by letter", round_id, missing, fresh.len());
            }
        }
        parsed
    };
    let judge::Resolved { verdicts, letter_only, to_cache } = judge::resolve(round.letter, &items, &cached, parsed.as_ref());
    tracing::info!("npat: round {}: {} answers, {} remembered, {} asked, {} newly remembered", round_id, items.len(), cached.len(), fresh.len(), to_cache.len());
    {
        let conn = db.lock();
        let now = Utc::now().timestamp();
        for (cat, key, v) in &to_cache {
            if let Err(err) = store::cache_put(&conn, round.letter, *cat, key, v, "ai", now) {
                tracing::warn!("npat: verdict for {:?} not remembered: {}", key, err);
            }
        }
    }
    let mut rows = Vec::new();
    for ((user, typed), slots) in entries.iter().zip(&map) {
        for cat in 0..4 {
            if let Some(id) = slots[cat] {
                rows.push((*user, cat, judge::tidy(&typed[cat]), verdicts[id].clone()));
            }
        }
    }
    let now = Utc::now().timestamp();
    let mut conn = db.lock();
    let saved = store::save_verdicts(&mut conn, round_id, &rows)
        .and_then(|_| {
            let scored = judge::score(&store::judged(&conn, round_id), points());
            store::save_scores(&mut conn, round_id, &scored)
        })
        .and_then(|_| store::finish_judging(&conn, round_id, letter_only, now));
    match saved {
        Ok(done) => done,
        Err(err) => {
            tracing::warn!("npat: round {} verdicts not saved: {}", round_id, err);
            false
        }
    }
}

/// A letter's results lines: best game score so far first, then the letter's score.
fn letter_lines(round: &Round) -> Vec<LetterLine> {
    let Some(db) = store::db() else { return Vec::new() };
    let (judged, houses, so_far) = {
        let conn = db.lock();
        (store::judged(&conn, round.id), store::houses(&conn, round.id), store::letter_scores(&conn, round.game_id, round.letter_no))
    };
    let totals: HashMap<u64, i64> = judge::standings(&so_far).into_iter().map(|s| (s.user, s.total)).collect();
    let mut lines: Vec<LetterLine> = judge::score(&judged, points())
        .iter()
        .map(|s| {
            let answers = judged
                .iter()
                .find(|e| e.user == s.user)
                .map(|e| std::array::from_fn(|i| e.answers[i].as_ref().map(|(a, _)| a.clone()).unwrap_or_default()));
            LetterLine {
                user: s.user,
                badge: houses.get(&s.user).map(|h| badge(h)).unwrap_or_default(),
                answers: answers.unwrap_or_default(),
                marks: s.marks,
                score: s.score,
                game_total: totals.get(&s.user).copied().unwrap_or(s.score),
            }
        })
        .collect();
    lines.sort_by(|a, b| b.game_total.cmp(&a.game_total).then(b.score.cmp(&a.score)));
    lines
}

fn letter_results_embed(round: &Round, lines: &[LetterLine]) -> CreateEmbed {
    let (title, text, footer) = letter_results_text(round.game_id, round.letter_no, game_letters(round.game_id), round.letter, lines, round.letter_only, points());
    CreateEmbed::new().title(title).description(text).colour(COLOUR).footer(CreateEmbedFooter::new(footer))
}

fn pending_challenges(round_id: i64) -> usize {
    store::db().map(|db| store::challenges(&db.lock(), round_id).iter().filter(|(_, _, done)| !done).count()).unwrap_or(0)
}

fn review_open(round: &Round, now: i64) -> bool {
    round.status == Status::Done && round.judged_at.is_some_and(|t| now - t <= REVIEW_SECS)
}

async fn post_letter_results(ctx: &Context, round: &Round) {
    let lines = letter_lines(round);
    let open = review_open(round, Utc::now().timestamp()) && !lines.is_empty();
    let message = CreateMessage::new()
        .embed(letter_results_embed(round, &lines))
        .components(if lines.is_empty() { Vec::new() } else { results_rows(round.id, pending_challenges(round.id), open) })
        .allowed_mentions(CreateAllowedMentions::new());
    match call(ChannelId::new(round.channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            if let Some(db) = store::db() {
                let _ = store::set_results_message(&db.lock(), round.id, posted.id.get());
            }
            if open {
                let ctx = ctx.clone();
                let round_id = round.id;
                let wait = round.judged_at.map(|t| (t + REVIEW_SECS - Utc::now().timestamp()).max(0)).unwrap_or(0) as u64;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(wait + 1)).await;
                    edit_letter_results(&ctx, round_id).await;
                });
            }
        }
        Err(err) => tracing::warn!("npat: results for round {} not posted: {}", round.id, err),
    }
}

/// Redraws a letter's results card: after a review, a challenge, or when its buttons close.
async fn edit_letter_results(ctx: &Context, round_id: i64) {
    let Some(round) = store::db().and_then(|db| store::get_round(&db.lock(), round_id)) else { return };
    let Some(message) = round.results_message.filter(|m| *m != 0) else { return };
    let lines = letter_lines(&round);
    let open = review_open(&round, Utc::now().timestamp());
    let edit = EditMessage::new()
        .embed(letter_results_embed(&round, &lines))
        .components(if lines.is_empty() { Vec::new() } else { results_rows(round_id, pending_challenges(round_id), open) })
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(round.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("npat: results card for round {} not edited: {}", round_id, err);
    }
}

// --- games ---------------------------------------------------------------------------------------------

/// Adds a finished game up, pays its best two and posts its final card. `early`
/// when a letter nobody answered (or a switched-off game) cut it short.
async fn end_game(ctx: &Context, game_id: i64, early: bool) {
    let Some(db) = store::db() else { return };
    let now = Utc::now().timestamp();
    let finished = {
        let mut conn = db.lock();
        let Some(game) = store::get_game(&conn, game_id) else { return };
        let pays = judge::game_pays(store::game_players(&conn, game_id), min_scored());
        let table = judge::standings(&store::letter_scores(&conn, game_id, game.letters));
        match store::finish_game(&conn, game_id, pays, early, now).and_then(|done| {
            if done {
                store::save_game_scores(&mut conn, game_id, &table, pays.then(prizes)).map(|_| true)
            } else {
                Ok(false)
            }
        }) {
            Ok(done) => done,
            Err(err) => {
                tracing::warn!("npat: game {} not finished: {}", game_id, err);
                false
            }
        }
    };
    if !finished {
        return;
    }
    let game = store::get_game(&db.lock(), game_id);
    tracing::info!("npat: game {} over (early {}, pays {:?})", game_id, early, game.as_ref().map(|g| g.pays));
    if game.as_ref().is_some_and(|g| g.pays) {
        pay_game(game_id);
    } else {
        let _ = store::mark_game_paid_out(&db.lock(), game_id);
    }
    post_final(ctx, game_id).await;
}

/// Pays a finished game's 1st and 2nd into the ledger. Safe to repeat: each row
/// has a key naming the game and the place.
fn pay_game(game_id: i64) {
    let Some(db) = store::db() else { return };
    let rows = store::game_scores(&db.lock(), game_id);
    for (user, row) in rows {
        if row.owed <= 0 || row.credited != 0 || !matches!(row.place, 1 | 2) {
            continue;
        }
        let reason = format!("Name Place Animal Thing: {} in game {}", if row.place == 1 { "1st" } else { "2nd" }, game_id);
        let granted = credit(user, row.owed, &reason, None, format!("npat:{}:{}", game_id, row.place));
        if granted != 0 {
            let _ = store::add_game_credit(&db.lock(), game_id, user, granted);
        }
    }
    let _ = store::mark_game_paid_out(&db.lock(), game_id);
}

fn final_embed(game_id: i64) -> Option<(u64, Option<u64>, CreateEmbed)> {
    let db = store::db()?;
    let (game, rows, houses, rounds) = {
        let conn = db.lock();
        (store::get_game(&conn, game_id)?, store::game_scores(&conn, game_id), store::game_houses(&conn, game_id), store::game_rounds(&conn, game_id))
    };
    let mut lines: Vec<FinalLine> = rows
        .iter()
        .map(|(user, r)| FinalLine { user: *user, badge: houses.get(user).map(|h| badge(h)).unwrap_or_default(), total: r.total, rank: r.rank as usize, prize: r.owed })
        .collect();
    lines.sort_by_key(|l| (l.rank, l.user));
    let letters: Vec<char> = rounds.iter().filter(|r| r.status == Status::Done).map(|r| r.letter).collect();
    let n = NpatRules { letters_per_game: game.letters, ..npat_rules(daily_cap()) };
    let (title, text, footer) = final_results_text(game.id, &letters, &lines, game.pays, game.ended_early, &n);
    Some((game.channel, game.results_message, CreateEmbed::new().title(title).description(text).colour(FINAL_COLOUR).footer(CreateEmbedFooter::new(footer))))
}

async fn post_final(ctx: &Context, game_id: i64) {
    let Some((channel, _, embed)) = final_embed(game_id) else { return };
    let message = CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new());
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            if let Some(db) = store::db() {
                let _ = store::set_game_results_message(&db.lock(), game_id, posted.id.get());
            }
        }
        Err(err) => tracing::warn!("npat: final results for game {} not posted: {}", game_id, err),
    }
}

async fn edit_final(ctx: &Context, game_id: i64) {
    let Some((channel, Some(message), embed)) = final_embed(game_id) else { return };
    let edit = EditMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("npat: final results for game {} not edited: {}", game_id, err);
    }
}

// --- the game task --------------------------------------------------------------------------------

enum Phase {
    Lobby,
    Round(Round),
    Break { game_id: i64, next: i64, until: i64 },
}

/// Starts the game task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting()) {
        (true, Some(c)) => tracing::info!("npat: playing in {}", c),
        (true, None) => tracing::info!("npat: on, but VIZIER_NPAT_CHANNEL isn't set"),
        (false, _) => tracing::info!("npat: VIZIER_NPAT is off"),
    }
    tokio::spawn(run(ctx));
}

/// After a start: the rules post first, unfinished games paid and posted, a
/// letter a restart cut off resumed (or judged), or the lobby.
async fn recover(ctx: &Context) -> Phase {
    let channel = active_channel(ctx);
    sync_rules(ctx, channel).await;
    let Some(db) = store::db() else { return Phase::Lobby };
    let (old_card, leftovers, live, running) = {
        let conn = db.lock();
        (store::meta_get(&conn, "card"), store::unfinished_games(&conn), store::live_round(&conn), store::running_game(&conn))
    };
    for game in leftovers {
        if game.pays && !game.paid_out {
            tracing::info!("npat: paying game {} that a restart left unpaid", game.id);
            pay_game(game.id);
        }
        if game.results_message.is_none() {
            post_final(ctx, game.id).await;
        }
    }
    let old_card = old_card.as_deref().and_then(|v| v.split_once(':')).and_then(|(c, m)| Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?)));
    reset_lobby();
    if let Some(c) = channel {
        if let Ok(latest) = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await {
            let mut s = SHARED.lock();
            s.latest = latest.first().map(|m| m.id.get()).unwrap_or(0);
            s.last_seen_ms = 0;
        }
    }
    if let Some(round) = live.clone().filter(|r| running.as_ref().is_some_and(|g| g.id == r.game_id)) {
        tracing::info!("npat: resuming game {} letter {} ({})", round.game_id, round.letter_no, round.letter);
        if let Some((c, m)) = old_card.filter(|(_, m)| Some(*m) != round.message) {
            delete(ctx, c, m).await;
        }
        if let Some(id) = round.message {
            SHARED.lock().card = Some((round.channel, id));
        }
        return Phase::Round(round);
    }
    // A letter left open with no game around it (from before games existed) is closed.
    if let Some(stale) = live {
        let _ = db.lock().execute("UPDATE rounds SET status = 'stopped' WHERE id = ?1 AND status IN ('open', 'judging')", rusqlite::params![stale.id]);
    }
    if let Some((c, m)) = old_card {
        delete(ctx, c, m).await;
    }
    if let Some(game) = running {
        let rounds = store::game_rounds(&db.lock(), game.id);
        let last = rounds.iter().filter(|r| r.status == Status::Done).last().cloned();
        let answered = last.as_ref().is_some_and(|r| store::submission_count(&db.lock(), r.id) > 0);
        match last {
            Some(r) if answered && r.letter_no < game.letters && channel == Some(game.channel) => {
                tracing::info!("npat: resuming game {} before letter {}", game.id, r.letter_no + 1);
                return Phase::Break { game_id: game.id, next: r.letter_no + 1, until: Utc::now().timestamp() + break_secs() };
            }
            Some(r) => end_game(ctx, game.id, r.letter_no < game.letters).await,
            None => end_game(ctx, game.id, true).await,
        }
    }
    Phase::Lobby
}

async fn run(ctx: Context) {
    let mut phase = recover(&ctx).await;
    // (round, answers shown on its card, when)
    let mut shown: (i64, usize, i64) = (0, 0, 0);
    let mut ticks: u64 = 0;
    loop {
        tokio::time::sleep(TICK).await;
        ticks += 1;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        let (stop, rules_check) = {
            let mut s = SHARED.lock();
            (std::mem::take(&mut s.stop), std::mem::take(&mut s.rules_check))
        };
        if rules_check || ticks % RULES_EVERY_TICKS == 0 {
            sync_rules(&ctx, channel).await;
        }
        phase = match phase {
            Phase::Lobby => lobby_tick(&ctx, channel, stop, now).await,
            Phase::Round(round) => round_tick(&ctx, round, channel, stop, now, &mut shown).await,
            Phase::Break { game_id, next, until } => break_tick(&ctx, game_id, next, until, channel, stop, now).await,
        };
        SHARED.lock().in_lobby = matches!(phase, Phase::Lobby);
        keep_card_last(&ctx, &mut phase).await;
    }
}

/// Moves the game card below anything that landed under it, a few seconds after
/// the channel went quiet, and puts it back if someone deleted it.
async fn keep_card_last(ctx: &Context, phase: &mut Phase) {
    let (card, due, gone) = {
        let s = SHARED.lock();
        (s.card, move_due(s.card.map(|(_, m)| m), s.latest, s.last_seen_ms, Utc::now().timestamp_millis(), MOVE_DELAY_MS), s.card_gone)
    };
    let Some((channel, _)) = card else { return };
    if !due && !gone {
        return;
    }
    SHARED.lock().card_gone = false;
    match phase {
        Phase::Lobby => {
            place_card(ctx, channel, lobby_message()).await;
        }
        Phase::Round(round) => {
            let status = store::db().and_then(|db| store::status_of(&db.lock(), round.id));
            let answers = store::db().map(|db| store::submission_count(&db.lock(), round.id)).unwrap_or(0);
            let state = if status == Some(Status::Open) && Utc::now().timestamp() <= round.ends_at + GRACE_SECS { RoundState::Live } else { RoundState::Judging };
            if let Some(id) = place_card(ctx, channel, round_message(round, answers, state)).await {
                if let Some(db) = store::db() {
                    let _ = store::set_message(&db.lock(), round.id, id);
                }
                round.message = Some(id);
            }
        }
        Phase::Break { game_id, next, until } => {
            let text = break_text(*next, game_letters(*game_id), *until);
            place_card(ctx, channel, CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
        }
    }
}

fn timing() -> Timing {
    Timing { min: min_players(), min_houses: min_houses(), join_secs: join_secs(), start_secs: start_secs() }
}

async fn lobby_tick(ctx: &Context, channel: Option<u64>, stop: bool, now: i64) -> Phase {
    let Some(channel) = channel else {
        if SHARED.lock().card.is_some() {
            drop_card(ctx, false).await;
        }
        return Phase::Lobby;
    };
    if stop {
        reset_lobby();
    }
    let card = SHARED.lock().card;
    if card.map(|(c, _)| c) != Some(channel) {
        place_card(ctx, channel, lobby_message()).await;
        return Phase::Lobby;
    }
    let (event, dirty) = {
        let mut s = SHARED.lock();
        let event = s.lobby.tick(now, timing());
        (event, s.lobby_dirty)
    };
    match event {
        LobbyEvent::Start(players) => {
            let game = store::db().and_then(|db| store::start_game(&db.lock(), channel, letters_per_game(), now).ok());
            let Some(game) = game else {
                reset_lobby();
                return Phase::Lobby;
            };
            tracing::info!("npat: game {} started with {} players", game.id, players.len());
            match start_round(ctx, game.id, 1, channel).await {
                Some(round) => Phase::Round(round),
                None => {
                    if let Some(db) = store::db() {
                        let _ = store::stop_live(&db.lock(), 0);
                    }
                    reset_lobby();
                    Phase::Lobby
                }
            }
        }
        event => {
            if dirty || event != LobbyEvent::Nothing {
                edit_lobby(ctx).await;
            }
            Phase::Lobby
        }
    }
}

async fn round_tick(ctx: &Context, round: Round, channel: Option<u64>, stop: bool, now: i64, shown: &mut (i64, usize, i64)) -> Phase {
    let (status, answers) = match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::status_of(&conn, round.id).unwrap_or(Status::Stopped), store::submission_count(&conn, round.id))
        }
        None => (Status::Stopped, 0),
    };
    if stop || status == Status::Stopped {
        edit_round(ctx, &round, answers, RoundState::Stopped).await;
        drop_card(ctx, true).await;
        tracing::info!("npat: game {} stopped at letter {}", round.game_id, round.letter_no);
        reset_lobby();
        return Phase::Lobby;
    }
    if status == Status::Open && now <= round.ends_at + GRACE_SECS {
        if (shown.0 != round.id || shown.1 != answers) && now - shown.2 >= COUNT_EDIT_SECS && now < round.ends_at {
            edit_round(ctx, &round, answers, RoundState::Live).await;
            *shown = (round.id, answers, now);
        }
        return Phase::Round(round);
    }
    if status != Status::Done {
        edit_round(ctx, &round, answers, RoundState::Judging).await;
        judge_round(round.id).await;
    }
    let Some(done) = store::db().and_then(|db| store::get_round(&db.lock(), round.id)) else {
        reset_lobby();
        return Phase::Lobby;
    };
    match done.status {
        Status::Done => {}
        // Stopped while judging: the next tick shows it. Still judging after an error: try again.
        _ => return Phase::Round(Round { message: round.message, ..done }),
    }
    if done.results_message.is_none() {
        post_letter_results(ctx, &done).await;
    }
    // The results are the history now; the letter's card goes.
    drop_card(ctx, false).await;
    let game = store::db().and_then(|db| store::get_game(&db.lock(), done.game_id));
    let Some(game) = game.filter(|g| g.status == GameStatus::Running) else {
        reset_lobby();
        return Phase::Lobby;
    };
    let nobody = answers == 0;
    let last = done.letter_no >= game.letters;
    let moved = channel != Some(game.channel);
    if nobody || last || moved {
        end_game(ctx, game.id, !last).await;
        reset_lobby();
        return Phase::Lobby;
    }
    let until = Utc::now().timestamp() + break_secs();
    let next = done.letter_no + 1;
    place_card(ctx, game.channel, CreateMessage::new().content(break_text(next, game.letters, until)).allowed_mentions(CreateAllowedMentions::new())).await;
    Phase::Break { game_id: game.id, next, until }
}

async fn break_tick(ctx: &Context, game_id: i64, next: i64, until: i64, channel: Option<u64>, stop: bool, now: i64) -> Phase {
    let game = store::db().and_then(|db| store::get_game(&db.lock(), game_id));
    let Some(game) = game.filter(|g| g.status == GameStatus::Running && !stop) else {
        drop_card(ctx, false).await;
        reset_lobby();
        return Phase::Lobby;
    };
    if channel != Some(game.channel) {
        drop_card(ctx, false).await;
        end_game(ctx, game_id, true).await;
        reset_lobby();
        return Phase::Lobby;
    }
    if now < until {
        return Phase::Break { game_id, next, until };
    }
    match start_round(ctx, game_id, next, game.channel).await {
        Some(round) => Phase::Round(round),
        None => {
            drop_card(ctx, false).await;
            end_game(ctx, game_id, true).await;
            reset_lobby();
            Phase::Lobby
        }
    }
}

// --- buttons and the pop-up -----------------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    if id == "npatjoin" {
        join_pressed(ctx, component).await;
    } else if let Some(round) = id.strip_prefix("npatsubmit:").and_then(|r| r.parse().ok()) {
        submit_pressed(ctx, component, round).await;
    } else if let Some(round) = id.strip_prefix("npatchalpick:").and_then(|r| r.parse().ok()) {
        challenge_picked(ctx, component, round).await;
    } else if let Some(round) = id.strip_prefix("npatchal:").and_then(|r| r.parse().ok()) {
        challenge_pressed(ctx, component, round).await;
    } else if let Some(round) = id.strip_prefix("npatrevpick:").and_then(|r| r.parse().ok()) {
        review_picked(ctx, component, round).await;
    } else if let Some(round) = id.strip_prefix("npatrev:").and_then(|r| r.parse().ok()) {
        review_pressed(ctx, component, round).await;
    }
}

const HOUSE_ONLY: &str = "🏠 Join a house first — only house members and Muggles can play Name Place Animal Thing.";

async fn join_pressed(ctx: &Context, component: &ComponentInteraction) {
    if live_channel().is_none() {
        return whisper(ctx, component, "The game is switched off right now.").await;
    }
    let user = component.user.id.get();
    let Some(house) = player_house(user) else {
        return whisper(ctx, component, HOUSE_ONLY).await;
    };
    let now = Utc::now().timestamp();
    let t = timing();
    let update = {
        let mut s = SHARED.lock();
        if s.in_lobby {
            let joined = s.lobby.toggle(user, house);
            s.lobby.settle(now, t);
            let on_card = s.card.map(|(_, m)| m) == Some(component.message.id.get());
            if !on_card {
                s.lobby_dirty = true;
            }
            Some((joined, on_card, s.lobby.joined.clone(), s.lobby.state))
        } else {
            None
        }
    };
    let Some((joined, on_card, players, state)) = update else {
        return whisper(ctx, component, "A game is running — press ✍️ Submit answers on the letter card to play.").await;
    };
    if !on_card {
        return whisper(ctx, component, if joined { "✋ You're in." } else { "You left the lobby." }).await;
    }
    let message = CreateInteractionResponseMessage::new()
        .embed(lobby_embed(&players, state))
        .components(vec![lobby_row()])
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await {
        tracing::warn!("npat: lobby join by {} not shown: {}", user, err);
        SHARED.lock().lobby_dirty = true;
    }
}

async fn submit_pressed(ctx: &Context, component: &ComponentInteraction, round_id: i64) {
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let found = store::db().and_then(|db| {
        let conn = db.lock();
        let round = store::get_round(&conn, round_id)?;
        let previous = store::submission(&conn, round_id, user);
        Some((round, previous))
    });
    let Some((round, previous)) = found.filter(|(r, _)| r.status == Status::Open && now <= r.ends_at + GRACE_SECS) else {
        return whisper(ctx, component, "⏱️ This letter is over.").await;
    };
    if player_house(user).is_none() {
        return whisper(ctx, component, HOUSE_ONLY).await;
    }
    let modal = modal_json_timed(round_id, round.letter, previous.as_ref(), Some((round.ends_at, now)));
    match ctx.http.create_interaction_response(component.id, &component.token, &modal, Vec::new()).await {
        Ok(()) => {}
        Err(serenity::Error::Http(err))
            if err.status_code().is_some_and(|s| s.as_u16() == 400) && !PLAIN_MODAL.load(std::sync::atomic::Ordering::Relaxed) =>
        {
            tracing::warn!("npat: Discord refused the pop-up with a timer line ({}), leaving the line out from now on", err);
            PLAIN_MODAL.store(true, std::sync::atomic::Ordering::Relaxed);
            let plain = modal_json_timed(round_id, round.letter, previous.as_ref(), Some((round.ends_at, now)));
            if let Err(err) = ctx.http.create_interaction_response(component.id, &component.token, &plain, Vec::new()).await {
                tracing::warn!("npat: answers pop-up for round {} not shown to {}: {}", round_id, user, err);
            }
        }
        Err(err) => tracing::warn!("npat: answers pop-up for round {} not shown to {}: {}", round_id, user, err),
    }
}

async fn reply_modal(ctx: &Context, modal: &ModalInteraction, text: &str) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = modal.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("npat: reply to {} not sent: {}", modal.user.id, err);
    }
}

pub async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    let Some(round_id) = modal.data.custom_id.strip_prefix("npatans:").and_then(|r| r.parse::<i64>().ok()) else {
        return;
    };
    let user = modal.user.id.get();
    let Some(house) = player_house(user) else {
        return reply_modal(ctx, modal, HOUSE_ONLY).await;
    };
    let data = serde_json::to_value(&modal.data).unwrap_or(Value::Null);
    let answers = answers_from(&data);
    let result = store::db().map(|db| store::submit(&db.lock(), round_id, user, house, &answers, Utc::now().timestamp()));
    let text = match result {
        Some(Ok(Submit::Saved)) => "✅ Got it — you can change them until the timer ends",
        Some(Ok(Submit::Cleared)) => "🗑️ Every box was empty, so your answers were taken back",
        Some(Ok(Submit::Closed | Submit::Missing)) | None => "⏱️ Too late — this letter has ended",
        Some(Err(err)) => {
            tracing::warn!("npat: answers from {} for round {} not saved: {}", user, round_id, err);
            "Something went wrong saving that. Press Submit answers again."
        }
    };
    reply_modal(ctx, modal, text).await;
}

/// A player's answers that a challenge can ask about: not valid, or shared.
fn challengeable(round_id: i64, user: u64) -> Option<Vec<(usize, String, Mark)>> {
    let judged = store::db().map(|db| store::judged(&db.lock(), round_id))?;
    let scored = judge::score(&judged, points());
    let mine = scored.iter().find(|s| s.user == user)?;
    let entry = judged.iter().find(|e| e.user == user)?;
    Some(
        (0..4)
            .filter(|i| matches!(mine.marks[*i], Mark::Invalid | Mark::Shared))
            .map(|i| (i, entry.answers[i].as_ref().map(|(a, _)| a.clone()).unwrap_or_default(), mine.marks[i]))
            .collect(),
    )
}

fn round_for_review(round_id: i64) -> Option<Round> {
    store::db().and_then(|db| store::get_round(&db.lock(), round_id)).filter(|r| review_open(r, Utc::now().timestamp()))
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max { text.to_string() } else { text.chars().take(max - 1).collect::<String>() + "…" }
}

async fn challenge_pressed(ctx: &Context, component: &ComponentInteraction, round_id: i64) {
    if round_for_review(round_id).is_none() {
        return whisper(ctx, component, "Challenges for this letter have closed.").await;
    }
    let Some(options) = challengeable(round_id, component.user.id.get()) else {
        return whisper(ctx, component, "Only players of this letter can challenge it.").await;
    };
    if options.is_empty() {
        return whisper(ctx, component, "Nothing to challenge: every answer of yours was unique.").await;
    }
    let menu: Vec<CreateSelectMenuOption> = options
        .iter()
        .map(|(cat, answer, mark)| {
            let why = if *mark == Mark::Invalid { "❌ judged not valid" } else { "🟰 counted as shared" };
            CreateSelectMenuOption::new(clip(&format!("{}: {}", CATEGORIES[*cat], answer), 100), cat.to_string()).description(why)
        })
        .collect();
    let count = menu.len() as u8;
    let select = CreateSelectMenu::new(format!("npatchalpick:{}", round_id), CreateSelectMenuKind::String { options: menu })
        .placeholder("Which answers should a mod look at?")
        .min_values(1)
        .max_values(count);
    let message = CreateInteractionResponseMessage::new()
        .content("⚖️ Pick the answers you think were judged wrong. A mod will look at them; nothing is posted in the channel.")
        .components(vec![CreateActionRow::SelectMenu(select)])
        .ephemeral(true);
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

fn picked_values(component: &ComponentInteraction) -> Vec<String> {
    match &component.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.clone(),
        _ => Vec::new(),
    }
}

async fn update(ctx: &Context, component: &ComponentInteraction, text: String, rows: Vec<CreateActionRow>) {
    let message = CreateInteractionResponseMessage::new().content(text).components(rows).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await {
        tracing::warn!("npat: private message not updated: {}", err);
    }
}

async fn challenge_picked(ctx: &Context, component: &ComponentInteraction, round_id: i64) {
    let user = component.user.id.get();
    if round_for_review(round_id).is_none() {
        return update(ctx, component, "Challenges for this letter have closed.".into(), Vec::new()).await;
    }
    let allowed = challengeable(round_id, user).unwrap_or_default();
    let cats: Vec<usize> = picked_values(component).iter().filter_map(|v| v.parse().ok()).filter(|c| allowed.iter().any(|(a, _, _)| a == c)).collect();
    if cats.is_empty() {
        return update(ctx, component, "Nothing picked.".into(), Vec::new()).await;
    }
    let added = store::db().map(|db| store::add_challenges(&db.lock(), round_id, user, &cats, Utc::now().timestamp()));
    let names: Vec<String> = allowed.iter().filter(|(c, _, _)| cats.contains(c)).map(|(c, a, _)| format!("{} ({})", CATEGORIES[*c], md(a))).collect();
    tracing::info!("npat: {} challenged round {} {:?} ({:?} new)", user, round_id, cats, added);
    update(ctx, component, format!("⚖️ Sent to the mods: {}. They'll take a look.", names.join(", ")), Vec::new()).await;
    edit_letter_results(ctx, round_id).await;
}

/// What a mod can flip: challenged answers first, then every answer judged not valid.
fn review_options(ctx: &Context, guild: Option<GuildId>, round_id: i64) -> Vec<CreateSelectMenuOption> {
    let Some(db) = store::db() else { return Vec::new() };
    let (judged, challenges) = {
        let conn = db.lock();
        (store::judged(&conn, round_id), store::challenges(&conn, round_id))
    };
    let scored = judge::score(&judged, points());
    let mut picked: Vec<(u64, usize, bool)> = Vec::new();
    for (user, cat, resolved) in &challenges {
        if !resolved && !picked.iter().any(|(u, c, _)| u == user && c == cat) {
            picked.push((*user, *cat, true));
        }
    }
    for entry in &judged {
        for (cat, slot) in entry.answers.iter().enumerate() {
            if slot.as_ref().is_some_and(|(_, v)| !v.valid) && !picked.iter().any(|(u, c, _)| *u == entry.user && *c == cat) {
                picked.push((entry.user, cat, false));
            }
        }
    }
    picked
        .into_iter()
        .take(25)
        .filter_map(|(user, cat, challenged)| {
            let entry = judged.iter().find(|e| e.user == user)?;
            let (answer, _) = entry.answers[cat].as_ref()?;
            let mark = scored.iter().find(|s| s.user == user).map(|s| s.marks[cat]).unwrap_or(Mark::Blank);
            let now = match mark {
                Mark::Invalid => "❌ not valid → make it valid",
                Mark::Unique => "✅ unique → make it not valid",
                _ => "🟰 shared → make it not valid",
            };
            let flag = if challenged { "⚖️ " } else { "" };
            let label = clip(&format!("{}{}: {}", flag, CATEGORIES[cat], answer), 100);
            let description = clip(&format!("{} · {}", cached_name(ctx, guild, user), now), 100);
            Some(CreateSelectMenuOption::new(label, format!("{}:{}", user, cat)).description(description))
        })
        .collect()
}

fn review_rows(options: Vec<CreateSelectMenuOption>, round_id: i64) -> Vec<CreateActionRow> {
    if options.is_empty() {
        return Vec::new();
    }
    let select = CreateSelectMenu::new(format!("npatrevpick:{}", round_id), CreateSelectMenuKind::String { options })
        .placeholder("Pick an answer to flip");
    vec![CreateActionRow::SelectMenu(select)]
}

const REVIEW_HELP: &str = "Pick an answer to flip between ✅ valid and ❌ not valid. Scores, the game's 1st and 2nd, house points and the results cards update straight away.";

async fn review_pressed(ctx: &Context, component: &ComponentInteraction, round_id: i64) {
    if !super::admin_ids().contains(&component.user.id.get()) {
        return whisper(ctx, component, "Only mods can review answers.").await;
    }
    let Some(round) = round_for_review(round_id) else {
        return whisper(ctx, component, "Reviews for this letter have closed.").await;
    };
    let options = review_options(ctx, component.guild_id, round_id);
    let head = format!("🛡️ **Game {} · Letter {} ({}/{})**", round.game_id, round.letter, round.letter_no, game_letters(round.game_id));
    let text = if options.is_empty() { format!("{}\nNothing to review: no challenges and no answers judged not valid.", head) } else { format!("{}\n{}", head, REVIEW_HELP) };
    let message = CreateInteractionResponseMessage::new()
        .content(text)
        .components(review_rows(options, round_id))
        .ephemeral(true)
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// Ledger changes a review calls for: (user, amount to ask for), from what each
/// player's game place was owed before and after, for everyone whose place moved.
pub fn review_fixes(before: &HashMap<u64, store::GameScoreRow>, after: &HashMap<u64, store::GameScoreRow>) -> Vec<(u64, i64)> {
    let mut out: Vec<(u64, i64)> = after
        .iter()
        .filter_map(|(user, new)| {
            let old = before.get(user).copied().unwrap_or_default();
            let amount = judge::adjustment(old.owed, new.owed, old.credited);
            (amount != 0).then_some((*user, amount))
        })
        .collect();
    out.sort_unstable();
    out
}

async fn review_picked(ctx: &Context, component: &ComponentInteraction, round_id: i64) {
    let by = component.user.id.get();
    if !super::admin_ids().contains(&by) {
        return update(ctx, component, "Only mods can review answers.".into(), Vec::new()).await;
    }
    let Some(round) = round_for_review(round_id) else {
        return update(ctx, component, "Reviews for this letter have closed.".into(), Vec::new()).await;
    };
    let Some((user, cat)) = picked_values(component).first().and_then(|v| {
        let (u, c) = v.split_once(':')?;
        Some((u.parse::<u64>().ok()?, c.parse::<usize>().ok()?))
    }) else {
        return update(ctx, component, "Nothing picked.".into(), Vec::new()).await;
    };
    let Some(db) = store::db() else { return };
    let now = Utc::now().timestamp();
    let outcome = {
        let mut conn = db.lock();
        let before_letter = store::scores(&conn, round_id);
        match store::toggle_verdict(&conn, round_id, user, cat, by, now) {
            Ok(Some(verdict)) => {
                let judged = store::judged(&conn, round_id);
                let answer = judged.iter().find(|e| e.user == user).and_then(|e| e.answers[cat].as_ref().map(|(a, _)| a.clone())).unwrap_or_default();
                let scored = judge::score(&judged, points());
                let game = store::get_game(&conn, round.game_id);
                let result = store::save_scores(&mut conn, round_id, &scored).and_then(|_| match game.as_ref().filter(|g| g.status == GameStatus::Done) {
                    // A finished game is added up again; a running one adds up at its end.
                    Some(game) => {
                        let before = store::game_scores(&conn, game.id);
                        let table = judge::standings(&store::letter_scores(&conn, game.id, game.letters));
                        store::save_game_scores(&mut conn, game.id, &table, game.pays.then(prizes))?;
                        let after = store::game_scores(&conn, game.id);
                        let fixes: Vec<(u64, i64, i64)> = if game.pays {
                            review_fixes(&before, &after).into_iter().filter_map(|(u, amount)| store::next_game_fix(&conn, game.id, u).ok().map(|n| (u, amount, n))).collect()
                        } else {
                            Vec::new()
                        };
                        Ok(fixes)
                    }
                    None => Ok(Vec::new()),
                });
                match result {
                    Ok(fixes) => {
                        let after_letter = store::scores(&conn, round_id);
                        let moved = (before_letter.get(&user).map(|r| r.score).unwrap_or(0), after_letter.get(&user).map(|r| r.score).unwrap_or(0));
                        Some((verdict, answer, fixes, moved, game))
                    }
                    Err(err) => {
                        tracing::warn!("npat: rescoring round {} failed: {}", round_id, err);
                        None
                    }
                }
            }
            _ => None,
        }
    };
    let Some((verdict, answer, fixes, (old, new), game)) = outcome else {
        return update(ctx, component, "That answer couldn't be changed.".into(), Vec::new()).await;
    };
    let reason = format!("Name Place Animal Thing: game {} reviewed", round.game_id);
    for (u, amount, n) in &fixes {
        let granted = credit(*u, *amount, &reason, Some(by), format!("npat-fix:{}:{}:{}", round.game_id, u, n));
        if granted != 0 {
            let _ = store::add_game_credit(&db.lock(), round.game_id, *u, granted);
        }
    }
    tracing::info!("npat: {} made round {} {}:{} {} (fixes {:?})", by, round_id, user, cat, if verdict.valid { "valid" } else { "not valid" }, fixes);
    // This letter's card and every later one show game totals that just changed.
    let later: Vec<i64> =
        store::db().map(|db| store::game_rounds(&db.lock(), round.game_id).into_iter().filter(|r| r.letter_no >= round.letter_no).map(|r| r.id).collect()).unwrap_or_default();
    for id in later {
        edit_letter_results(ctx, id).await;
    }
    let done = game.as_ref().is_some_and(|g| g.status == GameStatus::Done);
    if done {
        edit_final(ctx, round.game_id).await;
    }
    let mut text = format!(
        "{} **{}** ({}, <@{}>) is now **{}** · their letter score {} → {}",
        if verdict.valid { "✅" } else { "❌" },
        md(&answer),
        CATEGORIES[cat],
        user,
        if verdict.valid { "valid" } else { "not valid" },
        old,
        new
    );
    if !fixes.is_empty() {
        let moves: Vec<String> = fixes.iter().map(|(u, amount, _)| format!("<@{}> {}{}", u, if *amount > 0 { "+" } else { "" }, amount)).collect();
        text.push_str(&format!("\n🏅 The game's 1st and 2nd changed · house points {}", moves.join(" · ")));
    } else if !done {
        text.push_str("\n-# The game is still going: house points are worked out when it ends.");
    } else if game.as_ref().is_some_and(|g| !g.pays) {
        text.push_str("\n-# This game paid no house points, so only the cards changed.");
    }
    text.push_str(&format!("\n\n{}", REVIEW_HELP));
    let options = review_options(ctx, component.guild_id, round_id);
    update(ctx, component, text, review_rows(options, round_id)).await;
}

/// `/npatstop` - admins only.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true));
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can stop the game.".into())).await;
        return;
    }
    let stopped = store::db().and_then(|db| store::stop_live(&db.lock(), command.user.id.get()).ok());
    SHARED.lock().stop = true;
    let text = match &stopped {
        Some((Some(round), _)) => format!("🛑 Game {} stopped at letter {} ({}). No house points; back to the lobby.", round.game_id, round.letter_no, round.letter),
        Some((None, Some(game))) => format!("🛑 Game {} stopped between letters. No house points; back to the lobby.", game.id),
        _ => "No game was running. The lobby is reset.".to_string(),
    };
    tracing::info!("npat: /npatstop by {}: {:?}", command.user.id, stopped.map(|(r, g)| (r.map(|r| r.id), g.map(|g| g.id))));
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Points = Points { unique: 10, shared: 5 };
    const G: &str = "gryffindor";
    const S: &str = "slytherin";
    const T: Timing = Timing { min: 5, min_houses: 2, join_secs: 180, start_secs: 30 };

    fn rules() -> NpatRules {
        rules_text::tests::npat_defaults()
    }

    #[test]
    fn the_first_join_opens_the_window_and_enough_players_shorten_it() {
        let mut lobby = Lobby::default();
        assert_eq!(lobby.tick(100, T), LobbyEvent::Nothing, "idle until someone presses");
        assert!(lobby.toggle(1, G));
        assert_eq!(lobby.settle(100, T), LobbyEvent::Opened);
        assert_eq!(lobby.state, LobbyState::Open { closes_at: 280 });
        for (u, h) in [(2, G), (3, G), (4, S)] {
            lobby.toggle(u, h);
            assert_eq!(lobby.tick(110, T), LobbyEvent::Nothing);
        }
        assert!(lobby.toggle(5, G));
        assert_eq!(lobby.tick(120, T), LobbyEvent::Enough);
        assert_eq!(lobby.state, LobbyState::Starting { starts_at: 150, closes_at: 280 }, "shortened to 30 s");
        assert!(lobby.toggle(6, S), "more can still jump in");
        assert_eq!(lobby.tick(149, T), LobbyEvent::Nothing);
        assert_eq!(lobby.tick(150, T), LobbyEvent::Start(vec![1, 2, 3, 4, 5, 6]));
        assert_eq!((lobby.joined.len(), lobby.state), (0, LobbyState::Idle));

        // Enough late in the window: the countdown never runs past the window, nor under 10 s.
        assert_eq!(start_time(270, 280, 30), 280);
        assert_eq!(start_time(278, 280, 30), 288);
        assert_eq!(start_time(100, 280, 5), 105, "a short countdown stays short");
    }

    #[test]
    fn someone_leaving_calls_off_the_countdown_and_everyone_leaving_goes_idle() {
        let mut lobby = Lobby::default();
        for (u, h) in [(1, G), (2, G), (3, S), (4, S), (5, G)] {
            lobby.toggle(u, h);
        }
        assert_eq!(lobby.tick(0, T), LobbyEvent::Enough, "a full lobby at once goes straight to the countdown");
        lobby.toggle(3, S);
        assert_eq!(lobby.tick(5, T), LobbyEvent::NotEnoughAnymore);
        assert_eq!(lobby.state, LobbyState::Open { closes_at: 180 }, "the original window stays");
        for u in [1, 2, 4, 5] {
            lobby.toggle(u, G);
        }
        assert_eq!(lobby.tick(6, T), LobbyEvent::BackToIdle);
        assert_eq!(lobby.state, LobbyState::Idle);
        assert_eq!(lobby.tick(7, T), LobbyEvent::Nothing);
    }

    #[test]
    fn a_window_without_enough_players_resets() {
        let mut lobby = Lobby::default();
        lobby.toggle(1, G);
        lobby.toggle(2, S);
        assert_eq!(lobby.tick(0, T), LobbyEvent::Opened);
        assert_eq!(lobby.tick(179, T), LobbyEvent::Nothing);
        assert_eq!(lobby.tick(180, T), LobbyEvent::Missed);
        assert_eq!((lobby.joined.len(), lobby.state), (0, LobbyState::Missed { until: 180 + MISSED_SECS }));
        assert_eq!(lobby.tick(181, T), LobbyEvent::Nothing, "the notice stays a moment");
        assert_eq!(lobby.tick(180 + MISSED_SECS, T), LobbyEvent::BackToIdle);
        // Pressing during the notice opens a fresh window.
        let mut again = Lobby { joined: Vec::new(), state: LobbyState::Missed { until: 500 } };
        again.toggle(9, G);
        assert_eq!(again.settle(400, T), LobbyEvent::Opened);
        assert_eq!(again.state, LobbyState::Open { closes_at: 580 });
    }

    #[test]
    fn five_from_one_house_do_not_start_but_five_from_two_do() {
        let one_house: Vec<Player> = (1..=5).map(|u| (u, G)).collect();
        assert!(!enough_players(&one_house, 5, 2));
        assert!(enough_players(&one_house, 5, 1), "with the house rule at 1 they would");
        let mut lobby = Lobby { joined: one_house.clone(), ..Default::default() };
        assert_eq!(lobby.tick(0, T), LobbyEvent::Opened);
        for t in 1..179 {
            assert_eq!(lobby.tick(t, T), LobbyEvent::Nothing, "no countdown with one house");
        }
        assert!(lobby.toggle(6, S));
        assert_eq!(lobby.tick(179, T), LobbyEvent::Enough);
        let mut two_houses = one_house.clone();
        two_houses[4].1 = S;
        assert_eq!(house_count(&two_houses), 2);
        assert!(enough_players(&two_houses, 5, 2));
        assert!(!enough_players(&two_houses[..4], 5, 2), "4 players from 2 houses is still too few");
        assert!(!enough_players(&two_houses, 5, 3));
        assert_eq!(house_count(&[]), 0);

        // Muggles make up the numbers but not the houses.
        let with_muggle: Vec<Player> = vec![(1, G), (2, G), (3, S), (4, S), (5, MUGGLE)];
        assert_eq!(house_count(&with_muggle), 2);
        assert!(enough_players(&with_muggle, 5, 2), "4 house players from 2 houses + a Muggle start");
        let one_house_and_muggles: Vec<Player> = vec![(1, G), (2, G), (3, G), (4, MUGGLE), (5, MUGGLE)];
        assert!(!enough_players(&one_house_and_muggles, 5, 2), "a Muggle is not a second house");
    }

    #[test]
    fn the_card_moves_after_a_quiet_moment_and_never_chases_itself() {
        // Nothing below the card.
        assert!(!move_due(Some(500), 500, 0, 10_000, MOVE_DELAY_MS));
        assert!(!move_due(Some(500), 400, 0, 10_000, MOVE_DELAY_MS), "an older message arriving late doesn't count");
        assert!(!move_due(None, 900, 0, 10_000, MOVE_DELAY_MS), "no card, nothing to move");
        // A message landed below: wait for the channel to go quiet.
        assert!(!move_due(Some(500), 600, 9_000, 10_000, MOVE_DELAY_MS));
        assert!(!move_due(Some(500), 700, 9_500, 12_499, MOVE_DELAY_MS), "each new message restarts the wait");
        assert!(move_due(Some(500), 700, 9_500, 12_500, MOVE_DELAY_MS));
        // The move posts the card again: its own id becomes both the card and the latest message.
        assert!(!move_due(Some(800), 800, 12_600, 20_000, MOVE_DELAY_MS), "the moved card is the newest message, no loop");
    }

    #[test]
    fn the_lobby_card_reads_as_agreed() {
        let n = rules();
        let idle = lobby_text(&[], LobbyState::Idle, &n);
        assert!(idle.starts_with("A game is **5 letters**. For each, find a **Name, Place, Animal and Thing** starting with it in **45 s**.\n"), "{idle}");
        assert!(idle.contains("✍️ Answers go in a private pop-up · unique **10** · shared **5** · the game's best two house members win 🥇 +2 🥈 +1 house points\n"), "{idle}");
        assert!(idle.contains("-# Real names · real places on a map · real living animals · real things you can touch, no brands · Hindi and Hinglish welcome · 📜 full rules above\n\n"), "{idle}");
        assert!(idle.ends_with("▶️ Press **✋ I'm in** to start a game · needs **5 players** from **2+ houses**"), "{idle}");

        let four = [(11, G), (22, S), (33, G), (44, G)];
        let open = lobby_text(&four, LobbyState::Open { closes_at: 1_789_367_580 }, &n);
        assert!(open.contains("⏳ Game starts <t:1789367580:R> if **5 players** from **2+ houses** are in\n\n**4/5 players · 🦁🐍 2 houses**\n<@11> · <@22> · <@33> · <@44>"), "{open}");
        let one_house: Vec<Player> = (1..=5).map(|u| (u, G)).collect();
        let waiting = lobby_text(&one_house, LobbyState::Open { closes_at: 9 }, &n);
        assert!(waiting.contains("are in\n-# Waiting for someone from another house\n\n**5/5 players · 🦁 1 house**"), "{waiting}");
        let starting = lobby_text(&four, LobbyState::Starting { starts_at: 1_789_367_430, closes_at: 9 }, &n);
        assert!(starting.contains("✅ **Enough players!** Starting <t:1789367430:R> — jump in now"), "{starting}");
        assert!(lobby_text(&[], LobbyState::Missed { until: 9 }, &n).ends_with("😴 Not enough players this time. Press **✋ I'm in** to try again."));
        let many: Vec<Player> = (1..=45).map(|u| (u, G)).collect();
        assert!(lobby_text(&many, LobbyState::Open { closes_at: 9 }, &n).ends_with("<@40> · and 5 more"));
        let solo = NpatRules { min_houses: 1, prizes: [3, 0], ..rules() };
        let text = lobby_text(&four, LobbyState::Open { closes_at: 9 }, &solo);
        assert!(text.contains("if **5 players** are in\n\n**4/5 players**\n"), "no house words when one house is enough: {text}");
        assert!(text.contains("win 🥇 +3 house points"), "{text}");
        let row = serde_json::to_value(lobby_row()).unwrap();
        assert_eq!((row["components"][0]["custom_id"].as_str(), row["components"][0]["label"].as_str()), (Some("npatjoin"), Some("✋ I'm in")));
    }

    #[test]
    fn the_letter_card_and_break_line_read_as_agreed() {
        assert_eq!(round_title(4, 3, 5), "🔤 Game 4 · Letter 3/5");
        assert_eq!(round_text('P', 1_789_367_445, 3, RoundState::Live), "## Letter **P**\n**Name · Place · Animal · Thing**, each starting with **P**\n⏱️ Ends <t:1789367445:R>\n📝 **3 answers** in");
        assert!(round_text('P', 0, 1, RoundState::Judging).ends_with("⏱️ Time's up · 📝 **1 answer** in · judging…"));
        assert!(round_text('P', 0, 1, RoundState::Stopped).ends_with("🛑 Stopped by a mod · no house points"));
        assert_eq!(break_text(3, 5, 1_789_367_500), "⏭️ Letter **3/5** starts <t:1789367500:R>");
        let open = serde_json::to_value(round_row(12, true)).unwrap();
        assert_eq!((open["components"][0]["custom_id"].as_str(), open["components"][0]["disabled"].as_bool()), (Some("npatsubmit:12"), Some(false)));
        assert_eq!(serde_json::to_value(round_row(12, false)).unwrap()["components"][0]["disabled"], true);
    }

    #[test]
    fn the_pop_up_shows_a_countdown() {
        let timed = modal_json_timed(12, 'P', None, Some((1_000_045, 1_000_007)));
        assert_eq!(timed["data"]["title"], "Letter P · 38s left");
        // The live timer line is off by default: only the four boxes.
        assert_eq!(timed["data"]["components"].as_array().unwrap().len(), 4);
        assert_eq!(modal_json_timed(12, 'P', None, Some((100, 200)))["data"]["title"], "Letter P · 0s left");
    }

    #[test]
    fn the_pop_up_has_four_optional_boxes_filled_with_the_last_answers() {
        let fresh = modal_json(12, 'P', None);
        assert_eq!(fresh["type"], 9);
        assert_eq!(fresh["data"]["custom_id"], "npatans:12");
        assert_eq!(fresh["data"]["title"], "Letter P");
        let rows = fresh["data"]["components"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        for (i, row) in rows.iter().enumerate() {
            let input = &row["components"][0];
            assert_eq!((row["type"].as_i64(), input["type"].as_i64(), input["style"].as_i64()), (Some(1), Some(4), Some(1)));
            assert_eq!(input["custom_id"], KEYS[i]);
            assert_eq!(input["label"], CATEGORIES[i]);
            assert_eq!((input["required"].as_bool(), input["max_length"].as_i64()), (Some(false), Some(40)));
            assert!(input.get("value").is_none(), "no empty values: Discord refuses them");
        }
        assert_eq!(rows[1]["components"][0]["placeholder"], "place starting with P");
        let before = ["Priya".to_string(), "".into(), " Parrot ".into(), "Pen".into()];
        let again = modal_json(12, 'P', Some(&before));
        let values: Vec<Option<&str>> = (0..4).map(|i| again["data"]["components"][i]["components"][0]["value"].as_str()).collect();
        assert_eq!(values, vec![Some("Priya"), None, Some("Parrot"), Some("Pen")]);
    }

    #[test]
    fn answers_are_read_from_either_submit_shape() {
        let rows = json!({
            "custom_id": "npatans:12",
            "components": [
                { "type": 1, "components": [{ "type": 4, "custom_id": "name", "value": " Priya " }] },
                { "type": 1, "components": [{ "type": 4, "custom_id": "place", "value": "" }] },
                { "type": 1, "components": [{ "type": 4, "custom_id": "animal", "value": "Parrot" }] },
                { "type": 1, "components": [{ "type": 4, "custom_id": "thing", "value": "x".repeat(60) }] }
            ]
        });
        let got = answers_from(&rows);
        assert_eq!(got[..3], ["Priya".to_string(), "".into(), "Parrot".into()]);
        assert_eq!(got[3].chars().count(), 40, "cut to the box's length");
        // Labels, as newer pop-ups come back, with a box missing.
        let labels = json!({
            "custom_id": "npatans:12",
            "components": [
                { "type": 18, "id": 1, "component": { "type": 4, "id": 2, "custom_id": "thing", "value": "Pen" } },
                { "type": 18, "id": 3, "component": { "type": 4, "id": 4, "custom_id": "name", "value": "Pooja" } }
            ]
        });
        assert_eq!(answers_from(&labels), ["Pooja".to_string(), "".into(), "".into(), "Pen".into()]);
        // What serenity makes of the rows round-trips through its own types.
        let parsed: serenity::all::ModalInteractionData = serde_json::from_value(rows.clone()).expect("serenity reads action rows");
        assert_eq!(answers_from(&serde_json::to_value(&parsed).unwrap())[0], "Priya");
        assert_eq!(answers_from(&Value::Null), [String::new(), String::new(), String::new(), String::new()]);
    }


    fn lline(user: u64, answer: &str, score: i64, game_total: i64) -> LetterLine {
        LetterLine {
            user,
            badge: "🦁".into(),
            answers: [answer.to_string(), "Parrot".into(), "Pen".into(), "".into()],
            marks: [Mark::Unique, Mark::Shared, Mark::Invalid, Mark::Blank],
            score,
            game_total,
        }
    }

    #[test]
    fn a_letter_results_card_shows_the_game_so_far_and_fits_discord() {
        assert_eq!(letter_line(&lline(1234, "Pune", 15, 60)), "<@1234> 🦁 — Pune ✅ · Parrot 🟰 · Pen ❌ · — · **15** · game **60**");
        let odd = LetterLine { badge: String::new(), answers: ["**P**<@1>".into(), "".into(), "".into(), "".into()], marks: [Mark::Invalid, Mark::Blank, Mark::Blank, Mark::Blank], ..lline(9, "", 0, 0) };
        assert_eq!(letter_line(&odd), "<@9> — P@1 ❌ · — · — · — · **0** · game **0**", "markdown and mentions in answers are defused");
        let muggle = LetterLine { badge: badge(MUGGLE), ..lline(7, "Pune", 40, 40) };
        assert!(letter_line(&muggle).starts_with("<@7> 🧙 Muggle — "));

        let (title, text, footer) = letter_results_text(4, 3, 5, 'P', &[lline(1, "Pune", 35, 90), lline(2, "Patna", 30, 60)], false, P);
        assert_eq!(title, "Letter P · 3/5 · Game 4");
        assert!(text.starts_with("<@1> 🦁 — Pune ✅"), "{text}");
        assert!(text.contains("\n-# 🎯 Letter 3/5 · the last number is the game score so far · house points are paid when the game ends"), "{text}");
        assert!(text.ends_with("-# ⚖️ Think an answer was judged wrong? Press Challenge within 30 min"), "{text}");
        assert!(!text.contains("House points:"), "no house points per letter");
        assert_eq!(footer, "✅ unique 10 · 🟰 shared 5 · ❌ 0");
        let (_, text, _) = letter_results_text(4, 1, 5, 'P', &[lline(1, "Pune", 3, 3)], true, P);
        assert!(text.contains("-# ⚠️ Checked by letter only: the judge couldn't be reached"), "{text}");
        assert_eq!(letter_results_text(4, 2, 5, 'M', &[], false, P).1, "Nobody answered this letter.");

        let long = "W".repeat(40);
        let crowd: Vec<LetterLine> = (0..60).map(|i| LetterLine { answers: [long.clone(), long.clone(), long.clone(), long.clone()], marks: [Mark::Shared; 4], ..lline(1_000_000_000_000_000_000 + i, "", 20, 200) }).collect();
        let (_, text, _) = letter_results_text(99, 5, 5, 'W', &crowd, true, P);
        assert!(text.chars().count() <= DESCRIPTION_LIMIT, "{} chars", text.chars().count());
        let shown = text.lines().filter(|l| l.starts_with("<@")).count();
        assert!(shown > 0 && shown <= RESULT_LINES, "{shown} lines");
        assert!(text.contains(&format!("…and {} more", 60 - shown)), "{text}");
        assert!(text.ends_with("within 30 min"), "notes survive the trim");
        let short: Vec<LetterLine> = (0..26).map(|i| lline(1_000_000_000_000_000_000 + i, "Pune", 10, 10)).collect();
        let (_, few, _) = letter_results_text(99, 1, 5, 'P', &short[..25], false, P);
        assert_eq!(few.lines().filter(|l| l.starts_with("<@")).count(), 25);
        assert!(letter_results_text(99, 1, 5, 'P', &short, false, P).1.contains("\n…and 1 more\n"));
        let rows = serde_json::to_value(results_rows(12, 2, true)).unwrap();
        assert_eq!(rows[0]["components"][0]["custom_id"], "npatchal:12");
        assert_eq!(rows[0]["components"][1]["label"], "🛡️ Review · 2 challenged");
    }

    #[test]
    fn the_final_card_ranks_the_game_and_names_who_won_house_points() {
        let n = rules();
        let fl = |user, badge: &str, total, rank, prize| FinalLine { user, badge: badge.into(), total, rank, prize };
        assert_eq!(final_line(&fl(1, "🐍", 145, 1, 2)), "🥇 <@1> 🐍 — **145** · 🏠 +2");
        assert_eq!(final_line(&fl(2, "🦁", 130, 2, 1)), "🥈 <@2> 🦁 — **130** · 🏠 +1");
        assert_eq!(final_line(&fl(3, "", 0, 3, 0)), "3. <@3> — **0**");
        // A Muggle wins the game; the house points pass to 2nd and 3rd.
        let lines = vec![fl(7, "🧙 Muggle", 160, 1, 0), fl(8, "🦁", 145, 2, 2), fl(9, "🐍", 120, 3, 1), fl(10, "🦅", 60, 4, 0)];
        let (title, text, footer) = final_results_text(4, &['P', 'M', 'S', 'K', 'T'], &lines, true, false, &n);
        assert_eq!(title, "🏁 Game 4 · final results");
        assert!(text.starts_with("Letters: **P · M · S · K · T**\n\n🥇 <@7> 🧙 Muggle — **160**\n🥈 <@8> 🦁 — **145** · 🏠 +2\n3. <@9> 🐍 — **120** · 🏠 +1\n4. <@10> 🦅 — **60**"), "{text}");
        assert!(text.contains("\n🏠 **House points:** <@8> +2 · <@9> +1\n-# 🧙 Muggles keep their place, but house points go to the two best house members"), "{text}");
        assert!(text.ends_with("places and house points update by themselves"), "{text}");
        assert_eq!(footer, "🥇 +2 🥈 +1 house points · up to 6 a day");
        let (_, early, footer) = final_results_text(5, &['P', 'M'], &lines[1..3], false, true, &NpatRules { cap: None, ..n.clone() });
        assert!(early.starts_with("Letters: **P · M**\n-# Ended early, after letter 2 of 5\n\n"), "{early}");
        assert!(early.contains("-# Fewer than 3 people played this game, so it pays no house points"), "{early}");
        assert_eq!(footer, "🥇 +2 🥈 +1 house points · no daily limit");
        assert!(final_results_text(6, &[], &[], false, true, &n).1.contains("Nobody played this game."));
        let crowd: Vec<FinalLine> = (0..80).map(|i| fl(1_000_000_000_000_000_000 + i, "🧙 Muggle", 999, i as usize + 1, 0)).collect();
        let (_, big, _) = final_results_text(7, &['A'; 10], &crowd, true, false, &n);
        assert!(big.chars().count() <= DESCRIPTION_LIMIT && big.contains("…and 55 more"), "{}", big.chars().count());
    }

    #[test]
    fn a_review_moves_house_points_with_the_places() {
        let row = |owed, credited| store::GameScoreRow { owed, credited, ..Default::default() };
        // 1 was 2nd and is now 1st; 2 was 1st and is now 2nd; 3 was 2nd (capped to 0) and drops out; 4 climbs into 2nd.
        let before: HashMap<u64, store::GameScoreRow> = [(1, row(1, 1)), (2, row(2, 2)), (3, row(1, 0)), (4, row(0, 0)), (5, row(0, 0))].into_iter().collect();
        let after: HashMap<u64, store::GameScoreRow> = [(1, row(2, 1)), (2, row(1, 2)), (3, row(0, 0)), (4, row(1, 0)), (5, row(0, 0))].into_iter().collect();
        assert_eq!(review_fixes(&before, &after), vec![(1, 1), (2, -1), (4, 1)], "3 was never credited, so nothing is taken back");
        assert!(review_fixes(&before, &before).is_empty());
    }
}
