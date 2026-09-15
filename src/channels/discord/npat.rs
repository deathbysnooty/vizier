//! Name Place Animal Thing, in its own channel (`VIZIER_NPAT_CHANNEL`).
//!
//! Members can't type there: everything is the bot's cards, buttons and
//! pop-ups. A lobby card collects house members with "I'm in"; once enough are
//! in, from enough different houses, a short countdown starts a round. A round is a letter and a timer, and anyone
//! in a house presses "Submit answers" to type a Name, Place, Animal and Thing
//! in a private pop-up - as often as they like until the timer ends, the last
//! one counts. Then one model call judges every answer (see `npat_judge`):
//! unique answers score 10 and shared ones 5 on the results card, and the
//! round's top two win house points. The card has Challenge and Review buttons
//! for half an hour. With enough players the next round follows
//! after a short break; otherwise the lobby comes back.
//!
//! One task drives the game ([`run`]): the buttons only change shared state or
//! the database, and the task does the posting and editing, so two presses can
//! never post two rounds.

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
    GuildId, Message, MessageId, ModalInteraction, UserId,
};

use super::control;
use super::npat_judge::{self as judge, CATEGORIES, KEYS, Mark, Points, Prizes};
use super::npat_store::{self as store, GRACE_SECS, MUGGLE, Round, Status, Submit};
use super::points::{Outcome, Source};

/// How often the game task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// How long the model gets to judge a round before the letter-only check.
const JUDGE_WAIT: Duration = Duration::from_secs(20);
/// From enough players to the letter.
pub const COUNTDOWN_SECS: i64 = 10;
/// The live answer count is edited at most this often.
const COUNT_EDIT_SECS: i64 = 3;
/// Challenges and reviews stay open this long after the results.
pub const REVIEW_SECS: i64 = 30 * 60;
/// Messages under the active card before it is posted again below them.
const STICKY_AFTER: u32 = 3;
/// Players listed on a results card before "…and N more".
pub const RESULT_LINES: usize = 25;
/// Kept clear of Discord's 4096 characters for an embed description.
pub const DESCRIPTION_LIMIT: usize = 4000;
/// Players named on the lobby card.
const LOBBY_NAMES: usize = 40;
const COLOUR: u32 = 0xF2A93B;
const STOPPED_COLOUR: u32 = 0x4E5058;
pub const DEFAULT_LETTERS: &str = "ABCDEFGHIJKLMNOPRSTUVW";
/// A quick, cheap model on OpenRouter, where the bot's own model lives.
pub const DEFAULT_JUDGE_MODEL: &str = "google/gemini-2.5-flash-lite";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_NPAT", true)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_NPAT_CHANNEL").filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

fn min_players() -> usize {
    control::number("VIZIER_NPAT_MIN_PLAYERS", 5).clamp(1, 50) as usize
}

fn lobby_idle_secs() -> i64 {
    control::number("VIZIER_NPAT_LOBBY_MINUTES", 30).clamp(1, 1440) as i64 * 60
}

fn letters() -> String {
    control::var("VIZIER_NPAT_LETTERS").filter(|l| !judge::letter_pool(l).is_empty()).unwrap_or_else(|| DEFAULT_LETTERS.to_string())
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

fn min_houses() -> usize {
    control::number("VIZIER_NPAT_MIN_HOUSES", 2).clamp(1, 4) as usize
}

fn min_scored() -> usize {
    control::number("VIZIER_NPAT_MIN_SCORED", 3).min(50) as usize
}

/// What answers score in a round right now (game score, not house points).
pub fn points() -> Points {
    Points {
        unique: control::number("VIZIER_NPAT_SCORE_UNIQUE", 10).min(1000) as i64,
        shared: control::number("VIZIER_NPAT_SCORE_SHARED", 5).min(1000) as i64,
    }
}

/// House points for a round's 1st and 2nd right now.
pub fn prizes() -> Prizes {
    Prizes {
        first: control::number("VIZIER_POINTS_NPAT_1ST", 2).min(100) as i64,
        second: control::number("VIZIER_POINTS_NPAT_2ND", 1).min(100) as i64,
    }
}

/// The game's channel when the game is on: for the guide and /help.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on())
}

// --- the lobby ----------------------------------------------------------------------------

/// A player and the key of their house.
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

/// Whether these players can start a round: enough of them, from enough houses.
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

/// Players with their houses; anyone who can no longer play is dropped.
fn with_houses(users: impl IntoIterator<Item = u64>) -> Vec<Player> {
    users.into_iter().filter_map(|u| player_house(u).map(|h| (u, h))).collect()
}

/// How a player shows on the results card: a crest, or "🧙 Muggle".
pub fn badge(house: &str) -> String {
    if house == MUGGLE {
        return "🧙 Muggle".to_string();
    }
    super::house::house(house).map(|h| h.crest.to_string()).unwrap_or_default()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lobby {
    pub joined: Vec<Player>,
    /// When someone last joined or left.
    pub changed_at: i64,
    /// Set once enough are in: when the round starts.
    pub starts_at: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LobbyEvent {
    Nothing,
    CountdownStarted,
    CountdownCancelled,
    Start(Vec<u64>),
    /// Nobody joined or left for too long: the lobby emptied.
    Emptied,
}

impl Lobby {
    /// Joins or leaves; true when now in.
    pub fn toggle(&mut self, user: u64, house: &'static str, now: i64) -> bool {
        self.changed_at = now;
        if let Some(i) = self.joined.iter().position(|(u, _)| *u == user) {
            self.joined.remove(i);
            false
        } else {
            self.joined.push((user, house));
            true
        }
    }

    pub fn starting(&self) -> bool {
        self.starts_at.is_some()
    }

    /// What the clock does to the lobby.
    pub fn tick(&mut self, now: i64, min: usize, min_houses: usize, idle_secs: i64) -> LobbyEvent {
        let ready = enough_players(&self.joined, min, min_houses);
        if let Some(at) = self.starts_at {
            if !ready {
                self.starts_at = None;
                return LobbyEvent::CountdownCancelled;
            }
            if now >= at {
                self.starts_at = None;
                self.changed_at = now;
                return LobbyEvent::Start(std::mem::take(&mut self.joined).into_iter().map(|(u, _)| u).collect());
            }
            return LobbyEvent::Nothing;
        }
        if ready {
            self.starts_at = Some(now + COUNTDOWN_SECS);
            return LobbyEvent::CountdownStarted;
        }
        if !self.joined.is_empty() && now - self.changed_at >= idle_secs {
            self.joined.clear();
            self.changed_at = now;
            return LobbyEvent::Emptied;
        }
        LobbyEvent::Nothing
    }
}

#[derive(Default)]
struct Shared {
    lobby: Lobby,
    /// The lobby card needs editing.
    lobby_dirty: bool,
    /// (channel, message) of the lobby card in chat.
    lobby_card: Option<(u64, u64)>,
    /// Messages posted under the active card since it went up.
    below: u32,
    /// A mod asked to stop.
    stop: bool,
    /// Round in play, for the message counter.
    in_round: bool,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

// --- words -----------------------------------------------------------------------------------

/// Text that sits inside markdown without breaking it.
fn md(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '~' | '|' | '<' | '>' | '\\' | '#')).collect::<String>().trim().to_string()
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "🥇 +2 🥈 +1 house points", leaving out a place worth nothing.
fn prize_words(prizes: Prizes) -> String {
    let mut parts = Vec::new();
    if prizes.first > 0 {
        parts.push(format!("🥇 +{}", prizes.first));
    }
    if prizes.second > 0 {
        parts.push(format!("🥈 +{}", prizes.second));
    }
    if parts.is_empty() { "no house points".to_string() } else { format!("{} house points", parts.join(" ")) }
}

/// The judging rulebook in one line, for the lobby card and the guide.
pub const RULES_IN_SHORT: &str = "Real names · real places on a map · real living animals · real things you can touch, no brands · Hindi and Hinglish welcome";

/// The lobby card's text.
#[allow(clippy::too_many_arguments)]
pub fn lobby_text(joined: &[Player], min: usize, min_houses: usize, secs: i64, p: Points, prizes: Prizes, starting: bool) -> String {
    let (min, min_houses) = (min.max(1), min_houses.max(1));
    let houses_rule = if min_houses > 1 { format!(" from **{}+ houses**", min_houses) } else { String::new() };
    let mut crests: Vec<&str> = Vec::new();
    for (_, key) in joined.iter().filter(|(_, h)| *h != MUGGLE) {
        let crest = super::house::house(key).map(|h| h.crest).unwrap_or("🏠");
        if !crests.contains(&crest) {
            crests.push(crest);
        }
    }
    let houses = house_count(joined);
    let houses_words = if houses == 0 { String::new() } else { format!(" · {} {}", crests.join(""), plural(houses as i64, "house", "houses")) };
    let mut text = format!(
        "A letter pops up: find a **Name, Place, Animal and Thing** starting with it in **{} s**.\n\
         ✍️ Answers go in a private pop-up · unique **{}** · shared **{}** · top two win {}\n\
         ✋ Press **I'm in** · the round starts when **{}** are in{}\n\
         -# {}\n\n**{}/{} players{}**\n",
        secs,
        p.unique,
        p.shared,
        prize_words(prizes),
        plural(min as i64, "player", "players"),
        houses_rule,
        RULES_IN_SHORT,
        joined.len(),
        min,
        houses_words
    );
    if joined.is_empty() {
        text.push_str("Nobody yet. Be the first!");
    } else {
        let names: Vec<String> = joined.iter().take(LOBBY_NAMES).map(|(u, _)| format!("<@{}>", u)).collect();
        text.push_str(&names.join(" · "));
        if joined.len() > LOBBY_NAMES {
            text.push_str(&format!(" · and {} more", joined.len() - LOBBY_NAMES));
        }
    }
    if starting {
        text.push_str(&format!("\n\n⏳ **Starting in {} s…**", COUNTDOWN_SECS));
    } else if joined.len() >= min && houses < min_houses {
        text.push_str("\n\n⏳ Waiting for someone from another house");
    }
    text
}

fn lobby_embed(joined: &[Player], starting: bool) -> CreateEmbed {
    CreateEmbed::new()
        .title("🔤 Name · Place · Animal · Thing")
        .description(lobby_text(joined, min_players(), min_houses(), round_secs(), points(), prizes(), starting))
        .colour(COLOUR)
}

fn lobby_row() -> CreateActionRow {
    CreateActionRow::Buttons(vec![CreateButton::new("npatjoin").label("✋ I'm in").style(ButtonStyle::Primary)])
}

/// How a round card reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundState {
    Live,
    Judging,
    Judged,
    Stopped,
}

pub fn round_text(letter: char, ends_at: i64, answers: usize, state: RoundState) -> String {
    let head = format!("## Letter **{}**\n**Name · Place · Animal · Thing**, each starting with **{}**\n", letter, letter);
    let count = format!("📝 **{}** in", plural(answers as i64, "answer", "answers"));
    let tail = match state {
        RoundState::Live => format!("⏱️ Ends <t:{}:R>\n{}", ends_at, count),
        RoundState::Judging => format!("⏱️ Time's up · {} · judging…", count),
        RoundState::Judged => format!("⏱️ Time's up · {} · results below ⬇️", count),
        RoundState::Stopped => "🛑 Stopped by a mod · no points".to_string(),
    };
    head + &tail
}

fn round_embed(round: &Round, answers: usize, state: RoundState) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("🔤 Round {}", round.id))
        .description(round_text(round.letter, round.ends_at, answers, state))
        .colour(if state == RoundState::Stopped { STOPPED_COLOUR } else { COLOUR })
}

fn round_row(round_id: i64, open: bool) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("npatsubmit:{}", round_id)).label("✍️ Submit answers").style(ButtonStyle::Success).disabled(!open),
    ])
}

/// The answers pop-up, as the raw interaction response: four short boxes,
/// each optional, filled with what was sent before.
pub fn modal_json(round_id: i64, letter: char, previous: Option<&[String; 4]>) -> Value {
    let rows: Vec<Value> = (0..4)
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
    json!({ "type": 9, "data": { "custom_id": format!("npatans:{}", round_id), "title": format!("Letter {}", letter), "components": rows } })
}

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

/// One player's line on the results card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub user: u64,
    /// A crest, "🧙 Muggle", or empty.
    pub badge: String,
    pub answers: [String; 4],
    pub marks: [Mark; 4],
    /// The game score.
    pub score: i64,
    /// Position in the round, from 1.
    pub rank: usize,
    /// House points won (0 for everyone but the two best house members of a round that pays).
    pub prize: i64,
}

pub fn result_line(line: &Line) -> String {
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
    let mut medal = match (line.rank, line.score > 0) {
        (1, true) => " 🥇".to_string(),
        (2, true) => " 🥈".to_string(),
        _ => String::new(),
    };
    if line.prize > 0 {
        medal.push_str(&format!(" +{}", line.prize));
    }
    format!("<@{}>{} — {} · **{}**{}", line.user, crest, parts.join(" · "), line.score, medal)
}

/// The results card: (title, description, footer). Lines are expected best
/// first; at most [`RESULT_LINES`] are shown and never past the size limit.
#[allow(clippy::too_many_arguments)]
pub fn results_text(round_id: i64, letter: char, lines: &[Line], letter_only: bool, pays: bool, min_scored: usize, p: Points, prizes: Prizes) -> (String, String, String) {
    let title = format!("Round {} · Letter {}", round_id, letter);
    let mut notes = Vec::new();
    let winners: Vec<String> = lines.iter().filter(|l| l.prize > 0).map(|l| format!("<@{}> +{}", l.user, l.prize)).collect();
    if !winners.is_empty() {
        notes.push(format!("🏠 **House points:** {}", winners.join(" · ")));
    }
    if pays && lines.iter().any(|l| l.rank <= 2 && l.score > 0 && l.badge == "🧙 Muggle") {
        notes.push("-# 🧙 Muggles keep their place, but house points go to the two best house members".to_string());
    }
    if letter_only {
        notes.push("-# ⚠️ Checked by letter only: the judge couldn't be reached".to_string());
    }
    if !lines.is_empty() && !pays {
        notes.push(format!("-# Fewer than {} people played, so this round pays no house points", min_scored));
    }
    if !lines.is_empty() {
        notes.push("-# ⚖️ Think an answer was judged wrong? Press Challenge within 30 min".to_string());
    }
    let notes = notes.join("\n");
    let mut text = String::new();
    if lines.is_empty() {
        text.push_str("Nobody answered this round.");
    }
    // Room for "…and N more" and the notes.
    let reserve = notes.chars().count() + 40;
    let mut shown = 0;
    for line in lines.iter().take(RESULT_LINES) {
        let row = result_line(line);
        if text.chars().count() + row.chars().count() + 1 + reserve > DESCRIPTION_LIMIT {
            break;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&row);
        shown += 1;
    }
    if lines.len() > shown {
        text.push_str(&format!("\n…and {} more", lines.len() - shown));
    }
    if !notes.is_empty() {
        text.push('\n');
        text.push_str(&notes);
    }
    let footer = format!("✅ unique {} · 🟰 shared {} · {}", p.unique, p.shared, prize_words(prizes));
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
    CreateCommand::new("npatstop").description("admin only: stop the Name Place Animal Thing round, no points, back to the lobby")
}

// --- Discord helpers ------------------------------------------------------------------------

async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, serenity::Error> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(result) => result,
        Err(_) => Err(serenity::Error::Other("no answer from Discord in time")),
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

// --- the lobby card -----------------------------------------------------------------------------

async fn post_lobby(ctx: &Context, channel: u64) {
    let (joined, starting) = {
        let s = SHARED.lock();
        (s.lobby.joined.clone(), s.lobby.starting())
    };
    let message = CreateMessage::new().embed(lobby_embed(&joined, starting)).components(vec![lobby_row()]).allowed_mentions(CreateAllowedMentions::new());
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let old = {
                let mut s = SHARED.lock();
                s.below = 0;
                s.lobby_dirty = false;
                s.lobby_card.replace((channel, posted.id.get()))
            };
            if let Some(db) = store::db() {
                let _ = store::meta_set(&db.lock(), "lobby_card", &format!("{}:{}", channel, posted.id.get()));
            }
            if let Some((c, m)) = old {
                delete(ctx, c, m).await;
            }
        }
        Err(err) => tracing::warn!("npat: lobby card not posted in {}: {}", channel, err),
    }
}

async fn edit_lobby(ctx: &Context) {
    let (card, joined, starting) = {
        let mut s = SHARED.lock();
        s.lobby_dirty = false;
        (s.lobby_card, s.lobby.joined.clone(), s.lobby.starting())
    };
    let Some((channel, message)) = card else { return };
    let edit = EditMessage::new().embed(lobby_embed(&joined, starting)).components(vec![lobby_row()]).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("npat: lobby card not edited: {}", err);
    }
}

async fn remove_lobby(ctx: &Context) {
    let card = SHARED.lock().lobby_card.take();
    if let Some((c, m)) = card {
        delete(ctx, c, m).await;
    }
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), "lobby_card", "");
    }
}

/// Back to the lobby, with these players already in.
fn reset_lobby(prejoined: Vec<Player>, now: i64) {
    let mut s = SHARED.lock();
    s.lobby = Lobby { joined: prejoined, changed_at: now, starts_at: None };
    s.lobby_dirty = true;
    s.in_round = false;
}

// --- rounds ---------------------------------------------------------------------------------------

async fn start_round(ctx: &Context, channel: u64) -> Option<Round> {
    let now = Utc::now().timestamp();
    let round = {
        let db = store::db()?;
        let conn = db.lock();
        let recent = store::recent_letters(&conn, judge::RECENT_LETTERS);
        let letter = judge::pick_letter(&letters(), &recent, rand::random::<f64>())?;
        match store::start_round(&conn, channel, letter, now, round_secs()) {
            Ok(round) => round,
            Err(err) => {
                tracing::warn!("npat: round not started: {}", err);
                return None;
            }
        }
    };
    let message = CreateMessage::new()
        .embed(round_embed(&round, 0, RoundState::Live))
        .components(vec![round_row(round.id, true)])
        .allowed_mentions(CreateAllowedMentions::new());
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            if let Some(db) = store::db() {
                let _ = store::set_message(&db.lock(), round.id, posted.id.get());
            }
            {
                let mut s = SHARED.lock();
                s.below = 0;
                s.in_round = true;
            }
            tracing::info!("npat: round {} started in {} with the letter {}", round.id, channel, round.letter);
            Some(Round { message: Some(posted.id.get()), ..round })
        }
        Err(err) => {
            tracing::warn!("npat: round card not posted in {}: {}", channel, err);
            if let Some(db) = store::db() {
                let _ = store::stop_live(&db.lock(), 0);
            }
            None
        }
    }
}

async fn edit_round(ctx: &Context, round: &Round, answers: usize, state: RoundState) {
    let Some(message) = round.message else { return };
    let edit = EditMessage::new()
        .embed(round_embed(round, answers, state))
        .components(vec![round_row(round.id, state == RoundState::Live)])
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(round.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("npat: round {} card not edited: {}", round.id, err);
    }
}

/// Posts a live round's card again at the bottom and removes the old one.
async fn repost_round(ctx: &Context, round: &Round, answers: usize) -> Option<Round> {
    let message = CreateMessage::new()
        .embed(round_embed(round, answers, RoundState::Live))
        .components(vec![round_row(round.id, true)])
        .allowed_mentions(CreateAllowedMentions::new());
    let posted = call(ChannelId::new(round.channel).send_message(&ctx.http, message)).await.ok()?;
    if let Some(db) = store::db() {
        let _ = store::set_message(&db.lock(), round.id, posted.id.get());
    }
    SHARED.lock().below = 0;
    if let Some(old) = round.message {
        delete(ctx, round.channel, old).await;
    }
    Some(Round { message: Some(posted.id.get()), ..round.clone() })
}

/// Judges a round whose time is up, saves verdicts and scores, and pays. False
/// when the round was stopped (or already finished) meanwhile.
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
        let reply = match tokio::time::timeout(JUDGE_WAIT, control::web::ask_bot_model_with(prompt, judge_model())).await {
            Ok(Ok(reply)) => Some(reply),
            Ok(Err(err)) => {
                tracing::warn!("npat: round {} judge call failed: {}", round_id, err);
                None
            }
            Err(_) => {
                tracing::warn!("npat: round {} judge took over {}s", round_id, JUDGE_WAIT.as_secs());
                None
            }
        };
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
    let players = entries.len();
    let pays = judge::round_pays(players, min_scored());
    let now = Utc::now().timestamp();
    let finished = {
        let mut conn = db.lock();
        let saved = store::save_verdicts(&mut conn, round_id, &rows)
            .and_then(|_| {
                let scored = judge::score(&store::judged(&conn, round_id), points());
                store::save_scores(&mut conn, round_id, &scored, pays.then(prizes))
            })
            .and_then(|_| store::finish_judging(&conn, round_id, letter_only, pays, now));
        match saved {
            Ok(done) => done,
            Err(err) => {
                tracing::warn!("npat: round {} verdicts not saved: {}", round_id, err);
                false
            }
        }
    };
    if !finished {
        return false;
    }
    tracing::info!("npat: round {} judged: {} players, {} answers, letter only {}, pays {}", round_id, players, rows.len(), letter_only, pays);
    if pays {
        pay_out(round_id);
    } else if let Some(db) = store::db() {
        let _ = store::mark_paid_out(&db.lock(), round_id);
    }
    true
}

/// Pays a judged round's 1st and 2nd into the ledger. Safe to repeat: each row
/// has a key naming the round and the place.
fn pay_out(round_id: i64) {
    let Some(db) = store::db() else { return };
    let scores = store::scores(&db.lock(), round_id);
    for (user, row) in scores {
        if row.owed <= 0 || row.credited != 0 || !matches!(row.place, 1 | 2) {
            continue;
        }
        let reason = format!("Name Place Animal Thing: {} in round {}", if row.place == 1 { "1st" } else { "2nd" }, round_id);
        let granted = credit(user, row.owed, &reason, None, format!("npat:{}:{}", round_id, row.place));
        if granted != 0 {
            let _ = store::add_credit(&db.lock(), round_id, user, granted);
        }
    }
    let _ = store::mark_paid_out(&db.lock(), round_id);
}

/// A round's results, best first, with house crests.
fn result_lines(round_id: i64) -> Vec<Line> {
    let Some(db) = store::db() else { return Vec::new() };
    let (judged, scores, houses) = {
        let conn = db.lock();
        (store::judged(&conn, round_id), store::scores(&conn, round_id), store::houses(&conn, round_id))
    };
    judge::score(&judged, points())
        .iter()
        .map(|s| {
            let answers = judged
                .iter()
                .find(|e| e.user == s.user)
                .map(|e| std::array::from_fn(|i| e.answers[i].as_ref().map(|(a, _)| a.clone()).unwrap_or_default()));
            Line {
                user: s.user,
                badge: houses.get(&s.user).map(|h| badge(h)).unwrap_or_default(),
                answers: answers.unwrap_or_default(),
                marks: s.marks,
                score: s.score,
                rank: s.rank,
                prize: scores.get(&s.user).map(|r| r.owed).unwrap_or(0),
            }
        })
        .collect()
}

fn results_embed(round: &Round, lines: &[Line]) -> CreateEmbed {
    let (title, text, footer) = results_text(round.id, round.letter, lines, round.letter_only, round.pays, min_scored(), points(), prizes());
    CreateEmbed::new().title(title).description(text).colour(COLOUR).footer(CreateEmbedFooter::new(footer))
}

fn pending_challenges(round_id: i64) -> usize {
    store::db().map(|db| store::challenges(&db.lock(), round_id).iter().filter(|(_, _, done)| !done).count()).unwrap_or(0)
}

fn review_open(round: &Round, now: i64) -> bool {
    round.status == Status::Done && round.judged_at.is_some_and(|t| now - t <= REVIEW_SECS)
}

async fn post_results(ctx: &Context, round: &Round) {
    let lines = result_lines(round.id);
    let open = review_open(round, Utc::now().timestamp()) && !lines.is_empty();
    let message = CreateMessage::new()
        .embed(results_embed(round, &lines))
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
                    edit_results(&ctx, round_id).await;
                });
            }
        }
        Err(err) => tracing::warn!("npat: results for round {} not posted: {}", round.id, err),
    }
}

/// Redraws a results card: after a review, a challenge, or when its buttons close.
async fn edit_results(ctx: &Context, round_id: i64) {
    let Some(round) = store::db().and_then(|db| store::get_round(&db.lock(), round_id)) else { return };
    let Some(message) = round.results_message.filter(|m| *m != 0) else { return };
    let lines = result_lines(round_id);
    let open = review_open(&round, Utc::now().timestamp());
    let edit = EditMessage::new()
        .embed(results_embed(&round, &lines))
        .components(if lines.is_empty() { Vec::new() } else { results_rows(round_id, pending_challenges(round_id), open) })
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(round.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        tracing::warn!("npat: results card for round {} not edited: {}", round_id, err);
    }
}

// --- the game task --------------------------------------------------------------------------------

enum Phase {
    Lobby,
    Round(Round),
    Break { until: i64, line: Option<(u64, u64)> },
}

/// Starts the game task once. A round a restart left open is resumed or judged,
/// unpaid rounds are paid, and the lobby card is posted fresh.
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

async fn recover(ctx: &Context) -> Phase {
    let Some(db) = store::db() else { return Phase::Lobby };
    let (old_card, leftovers, live) = {
        let conn = db.lock();
        (store::meta_get(&conn, "lobby_card"), store::unfinished(&conn), store::live_round(&conn))
    };
    if let Some((c, m)) = old_card.as_deref().and_then(|v| v.split_once(':')).and_then(|(c, m)| Some((c.parse().ok()?, m.parse().ok()?))) {
        delete(ctx, c, m).await;
    }
    let now = Utc::now().timestamp();
    for round in leftovers {
        if round.pays && !round.paid_out {
            tracing::info!("npat: paying round {} that a restart left unpaid", round.id);
            pay_out(round.id);
        }
        if round.results_message.is_none() {
            if review_open(&round, now) {
                post_results(ctx, &round).await;
            } else {
                let _ = store::set_results_message(&db.lock(), round.id, 0);
            }
        }
    }
    reset_lobby(Vec::new(), now);
    match live {
        Some(round) => {
            tracing::info!("npat: resuming round {} (letter {})", round.id, round.letter);
            SHARED.lock().in_round = true;
            Phase::Round(round)
        }
        None => Phase::Lobby,
    }
}

async fn run(ctx: Context) {
    let mut phase = recover(&ctx).await;
    // (round, answers shown on its card, when)
    let mut shown: (i64, usize, i64) = (0, 0, 0);
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        let stop = std::mem::take(&mut SHARED.lock().stop);
        phase = match phase {
            Phase::Lobby => lobby_tick(&ctx, channel, stop, now).await,
            Phase::Round(round) => round_tick(&ctx, round, channel, stop, now, &mut shown).await,
            Phase::Break { until, line } => break_tick(&ctx, until, line, channel, stop, now).await,
        };
    }
}

async fn lobby_tick(ctx: &Context, channel: Option<u64>, stop: bool, now: i64) -> Phase {
    let Some(channel) = channel else {
        if SHARED.lock().lobby_card.is_some() {
            remove_lobby(ctx).await;
        }
        return Phase::Lobby;
    };
    if stop {
        let mut s = SHARED.lock();
        s.lobby.starts_at = None;
        s.lobby_dirty = true;
    }
    let card = SHARED.lock().lobby_card;
    if card.map(|(c, _)| c) != Some(channel) {
        post_lobby(ctx, channel).await;
        return Phase::Lobby;
    }
    let (event, dirty, buried) = {
        let mut s = SHARED.lock();
        let event = s.lobby.tick(now, min_players(), min_houses(), lobby_idle_secs());
        (event, s.lobby_dirty, s.below >= STICKY_AFTER)
    };
    match event {
        LobbyEvent::Start(players) => {
            remove_lobby(ctx).await;
            match start_round(ctx, channel).await {
                Some(round) => {
                    tracing::info!("npat: a lobby of {} started round {}", players.len(), round.id);
                    Phase::Round(round)
                }
                None => {
                    reset_lobby(with_houses(players), now);
                    Phase::Lobby
                }
            }
        }
        event => {
            if buried {
                post_lobby(ctx, channel).await;
            } else if dirty || event != LobbyEvent::Nothing {
                edit_lobby(ctx).await;
            }
            Phase::Lobby
        }
    }
}

async fn round_tick(ctx: &Context, round: Round, channel: Option<u64>, stop: bool, now: i64, shown: &mut (i64, usize, i64)) -> Phase {
    let (status, players) = match store::db() {
        Some(db) => {
            let conn = db.lock();
            let players: Vec<u64> = store::submissions(&conn, round.id).into_iter().map(|(u, _)| u).collect();
            (store::status_of(&conn, round.id).unwrap_or(Status::Stopped), players)
        }
        None => (Status::Stopped, Vec::new()),
    };
    let answers = players.len();
    if stop || status == Status::Stopped {
        edit_round(ctx, &round, answers, RoundState::Stopped).await;
        tracing::info!("npat: round {} stopped", round.id);
        // An empty lobby, so a stop doesn't roll straight into the next countdown.
        reset_lobby(Vec::new(), now);
        return Phase::Lobby;
    }
    if status == Status::Open && now <= round.ends_at + GRACE_SECS {
        if SHARED.lock().below >= STICKY_AFTER {
            let moved = repost_round(ctx, &round, answers).await;
            *shown = (round.id, answers, now);
            return Phase::Round(moved.unwrap_or(round));
        }
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
    match store::db().and_then(|db| store::get_round(&db.lock(), round.id)) {
        Some(done) if done.status == Status::Done => {
            edit_round(ctx, &done, answers, RoundState::Judged).await;
            if done.results_message.is_none() {
                post_results(ctx, &done).await;
            }
            SHARED.lock().in_round = false;
            after_round(ctx, &done, players, channel).await
        }
        // Stopped while judging: the next tick shows it.
        Some(other) => Phase::Round(other),
        None => {
            reset_lobby(Vec::new(), now);
            Phase::Lobby
        }
    }
}

async fn break_tick(ctx: &Context, until: i64, line: Option<(u64, u64)>, channel: Option<u64>, stop: bool, now: i64) -> Phase {
    let Some(channel) = channel.filter(|_| !stop) else {
        if let Some((c, m)) = line {
            delete(ctx, c, m).await;
        }
        reset_lobby(Vec::new(), now);
        return Phase::Lobby;
    };
    if now < until {
        return Phase::Break { until, line };
    }
    if let Some((c, m)) = line {
        delete(ctx, c, m).await;
    }
    match start_round(ctx, channel).await {
        Some(round) => Phase::Round(round),
        None => {
            reset_lobby(Vec::new(), now);
            Phase::Lobby
        }
    }
}

/// After a round's results: a break and the next round when enough played, from
/// enough houses; otherwise the lobby with this round's players already in.
async fn after_round(ctx: &Context, round: &Round, players: Vec<u64>, channel: Option<u64>) -> Phase {
    // Judging and posting took a while: the break starts now.
    let now = Utc::now().timestamp();
    let same_channel = channel == Some(round.channel);
    let players = with_houses(players);
    if same_channel && enough_players(&players, min_players(), min_houses()) {
        let wait = break_secs();
        let text = format!("⏭️ Next round in **{} s**", wait);
        let line = call(ChannelId::new(round.channel).send_message(&ctx.http, CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())))
            .await
            .ok()
            .map(|m| (round.channel, m.id.get()));
        return Phase::Break { until: now + wait, line };
    }
    reset_lobby(players, now);
    Phase::Lobby
}

/// Counts messages under the active card so it can be moved back to the bottom.
pub fn note_message(ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != channel_setting() || msg.author.id == ctx.cache.current_user().id {
        return;
    }
    SHARED.lock().below += 1;
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
    let update = {
        let mut s = SHARED.lock();
        let here = s.lobby_card.map(|(_, m)| m) == Some(component.message.id.get());
        if here && !s.in_round {
            s.lobby.toggle(user, house, now);
            Some((s.lobby.joined.clone(), s.lobby.starting()))
        } else {
            None
        }
    };
    let Some((joined, starting)) = update else {
        return whisper(ctx, component, "This lobby has closed. Look for the newest card in the channel.").await;
    };
    let message = CreateInteractionResponseMessage::new()
        .embed(lobby_embed(&joined, starting))
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
        return whisper(ctx, component, "⏱️ This round is over.").await;
    };
    if player_house(user).is_none() {
        return whisper(ctx, component, HOUSE_ONLY).await;
    }
    let modal = modal_json(round_id, round.letter, previous.as_ref());
    if let Err(err) = ctx.http.create_interaction_response(component.id, &component.token, &modal, Vec::new()).await {
        tracing::warn!("npat: answers pop-up for round {} not shown to {}: {}", round_id, user, err);
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
        Some(Ok(Submit::Closed | Submit::Missing)) | None => "⏱️ Too late — this round has ended",
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
        return whisper(ctx, component, "Challenges for this round have closed.").await;
    }
    let Some(options) = challengeable(round_id, component.user.id.get()) else {
        return whisper(ctx, component, "Only players of this round can challenge it.").await;
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
        return update(ctx, component, "Challenges for this round have closed.".into(), Vec::new()).await;
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
    edit_results(ctx, round_id).await;
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

const REVIEW_HELP: &str = "Pick an answer to flip between ✅ valid and ❌ not valid. Scores, 1st and 2nd, house points and the results card update straight away.";

async fn review_pressed(ctx: &Context, component: &ComponentInteraction, round_id: i64) {
    if !super::admin_ids().contains(&component.user.id.get()) {
        return whisper(ctx, component, "Only mods can review answers.").await;
    }
    let Some(round) = round_for_review(round_id) else {
        return whisper(ctx, component, "Reviews for this round have closed.").await;
    };
    let options = review_options(ctx, component.guild_id, round_id);
    let text = if options.is_empty() {
        format!("🛡️ **Round {} · Letter {}**\nNothing to review: no challenges and no answers judged not valid.", round.id, round.letter)
    } else {
        format!("🛡️ **Round {} · Letter {}**\n{}", round.id, round.letter, REVIEW_HELP)
    };
    let message = CreateInteractionResponseMessage::new()
        .content(text)
        .components(review_rows(options, round_id))
        .ephemeral(true)
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// Ledger changes a review calls for: (user, amount to ask for), from what each
/// player's place was owed before and after, for everyone whose place moved.
pub fn review_fixes(before: &std::collections::HashMap<u64, store::ScoreRow>, after: &std::collections::HashMap<u64, store::ScoreRow>) -> Vec<(u64, i64)> {
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
        return update(ctx, component, "Reviews for this round have closed.".into(), Vec::new()).await;
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
        let before = store::scores(&conn, round_id);
        match store::toggle_verdict(&conn, round_id, user, cat, by, now) {
            Ok(Some(verdict)) => {
                let judged = store::judged(&conn, round_id);
                let answer = judged.iter().find(|e| e.user == user).and_then(|e| e.answers[cat].as_ref().map(|(a, _)| a.clone())).unwrap_or_default();
                let scored = judge::score(&judged, points());
                match store::save_scores(&mut conn, round_id, &scored, round.pays.then(prizes)) {
                    Ok(()) => {
                        let after = store::scores(&conn, round_id);
                        let fixes: Vec<(u64, i64, i64)> = if round.pays {
                            review_fixes(&before, &after)
                                .into_iter()
                                .filter_map(|(u, amount)| store::next_fix(&conn, round_id, u).ok().map(|n| (u, amount, n)))
                                .collect()
                        } else {
                            Vec::new()
                        };
                        let moved = (before.get(&user).map(|r| r.score).unwrap_or(0), after.get(&user).map(|r| r.score).unwrap_or(0));
                        Some((verdict, answer, fixes, moved))
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
    let Some((verdict, answer, fixes, (old, new))) = outcome else {
        return update(ctx, component, "That answer couldn't be changed.".into(), Vec::new()).await;
    };
    let reason = format!("Name Place Animal Thing: round {} reviewed", round_id);
    for (u, amount, n) in &fixes {
        let granted = credit(*u, *amount, &reason, Some(by), format!("npat-fix:{}:{}:{}", round_id, u, n));
        if granted != 0 {
            let _ = store::add_credit(&db.lock(), round_id, *u, granted);
        }
    }
    tracing::info!("npat: {} made round {} {}:{} {} (fixes {:?})", by, round_id, user, cat, if verdict.valid { "valid" } else { "not valid" }, fixes);
    edit_results(ctx, round_id).await;
    let mut text = format!(
        "{} **{}** ({}, <@{}>) is now **{}** · their score {} → {}",
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
        text.push_str(&format!("\n🏅 1st and 2nd changed · house points {}", moves.join(" · ")));
    }
    if !round.pays {
        text.push_str("\n-# This round paid no house points, so only the card changed.");
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
    let stopped = store::db().and_then(|db| store::stop_live(&db.lock(), command.user.id.get()).ok().flatten());
    SHARED.lock().stop = true;
    let text = match stopped {
        Some(ref round) => format!("🛑 Round {} (letter {}) stopped. No points; back to the lobby.", round.id, round.letter),
        None => "No round was running. Any break or countdown is cancelled and the lobby is open.".to_string(),
    };
    tracing::info!("npat: /npatstop by {}: {:?}", command.user.id, stopped.map(|r| r.id));
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const P: Points = Points { unique: 10, shared: 5 };
    const PRIZES: Prizes = Prizes { first: 2, second: 1 };

    const G: &str = "gryffindor";
    const S: &str = "slytherin";

    #[test]
    fn the_lobby_counts_down_once_enough_are_in_from_enough_houses() {
        let mut lobby = Lobby::default();
        for user in 1..=4 {
            assert!(lobby.toggle(user, if user == 1 { S } else { G }, 100));
        }
        assert_eq!(lobby.tick(101, 5, 2, 1800), LobbyEvent::Nothing);
        assert!(!lobby.toggle(2, G, 102), "pressing again leaves");
        assert!(lobby.toggle(2, G, 103));
        assert!(lobby.toggle(5, G, 104));
        assert_eq!(lobby.tick(105, 5, 2, 1800), LobbyEvent::CountdownStarted);
        assert_eq!(lobby.starts_at, Some(105 + COUNTDOWN_SECS));
        assert_eq!(lobby.tick(106, 5, 2, 1800), LobbyEvent::Nothing);
        assert!(lobby.toggle(6, G, 107), "joining during the countdown is fine");
        assert_eq!(lobby.tick(105 + COUNTDOWN_SECS, 5, 2, 1800), LobbyEvent::Start(vec![1, 3, 4, 2, 5, 6]));
        assert!(lobby.joined.is_empty() && !lobby.starting());

        // Someone leaving below the minimum calls the countdown off.
        let mut lobby = Lobby::default();
        for user in 1..=5 {
            lobby.toggle(user, if user % 2 == 0 { S } else { G }, 0);
        }
        assert_eq!(lobby.tick(1, 5, 2, 1800), LobbyEvent::CountdownStarted);
        lobby.toggle(3, G, 2);
        assert_eq!(lobby.tick(3, 5, 2, 1800), LobbyEvent::CountdownCancelled);
        assert_eq!(lobby.tick(30, 5, 2, 1800), LobbyEvent::Nothing);

        // Idle too long: emptied.
        assert_eq!(lobby.tick(2 + 1799, 5, 2, 1800), LobbyEvent::Nothing);
        assert_eq!(lobby.tick(2 + 1800, 5, 2, 1800), LobbyEvent::Emptied);
        assert!(lobby.joined.is_empty());
        assert_eq!(Lobby::default().tick(99_999, 5, 2, 1800), LobbyEvent::Nothing, "an empty lobby has nothing to empty");

        // A minimum of zero still needs one player.
        let mut solo = Lobby::default();
        assert_eq!(solo.tick(0, 0, 0, 60), LobbyEvent::Nothing);
        solo.toggle(9, G, 0);
        assert_eq!(solo.tick(0, 0, 0, 60), LobbyEvent::CountdownStarted);
    }

    #[test]
    fn five_from_one_house_do_not_start_but_five_from_two_do() {
        let one_house: Vec<Player> = (1..=5).map(|u| (u, G)).collect();
        assert!(!enough_players(&one_house, 5, 2));
        assert!(enough_players(&one_house, 5, 1), "with the house rule at 1 they would");
        let mut lobby = Lobby { joined: one_house.clone(), ..Default::default() };
        for t in 0..20 {
            assert_eq!(lobby.tick(t, 5, 2, 1800), LobbyEvent::Nothing, "no countdown with one house");
        }
        let mut two_houses = one_house.clone();
        two_houses[4].1 = S;
        assert_eq!(house_count(&two_houses), 2);
        assert!(enough_players(&two_houses, 5, 2));
        assert!(!enough_players(&two_houses[..4], 5, 2), "4 players from 2 houses is still too few");
        assert!(!enough_players(&two_houses, 5, 3));
        // A player from another house joining starts the countdown; them leaving cancels it.
        assert!(lobby.toggle(6, S, 20));
        assert_eq!(lobby.tick(21, 5, 2, 1800), LobbyEvent::CountdownStarted);
        assert!(!lobby.toggle(6, S, 22));
        assert_eq!(lobby.tick(23, 5, 2, 1800), LobbyEvent::CountdownCancelled);
        assert_eq!(house_count(&[]), 0);

        // Muggles make up the numbers but not the houses.
        let with_muggle: Vec<Player> = vec![(1, G), (2, G), (3, S), (4, S), (5, MUGGLE)];
        assert_eq!(house_count(&with_muggle), 2);
        assert!(enough_players(&with_muggle, 5, 2), "4 house players from 2 houses + a Muggle start");
        let one_house_and_muggles: Vec<Player> = vec![(1, G), (2, G), (3, G), (4, MUGGLE), (5, MUGGLE)];
        assert!(!enough_players(&one_house_and_muggles, 5, 2), "a Muggle is not a second house");
        let mut lobby = Lobby::default();
        for (u, h) in with_muggle {
            lobby.toggle(u, h, 0);
        }
        assert_eq!(lobby.tick(1, 5, 2, 1800), LobbyEvent::CountdownStarted);
        let text = lobby_text(&lobby.joined, 5, 2, 45, P, PRIZES, true);
        assert!(text.contains("**5/5 players · 🦁🐍 2 houses**"), "{text}");
    }

    #[test]
    fn the_lobby_and_round_cards_read_as_agreed() {
        let text = lobby_text(&[(11, G), (22, S), (33, G), (44, G)], 5, 2, 45, P, PRIZES, false);
        assert!(text.starts_with("A letter pops up: find a **Name, Place, Animal and Thing** starting with it in **45 s**.\n"), "{text}");
        assert!(text.contains("✍️ Answers go in a private pop-up · unique **10** · shared **5** · top two win 🥇 +2 🥈 +1 house points\n"), "{text}");
        assert!(text.contains("✋ Press **I'm in** · the round starts when **5 players** are in from **2+ houses**\n-# Real names · real places on a map · real living animals · real things you can touch, no brands · Hindi and Hinglish welcome\n\n"), "{text}");
        assert!(text.ends_with("**4/5 players · 🦁🐍 2 houses**\n<@11> · <@22> · <@33> · <@44>"), "{text}");
        assert!(lobby_text(&[], 5, 2, 45, P, PRIZES, false).ends_with("**0/5 players**\nNobody yet. Be the first!"));
        let five: Vec<Player> = (1..=5).map(|u| (u, if u == 5 { S } else { G })).collect();
        assert!(lobby_text(&five, 5, 2, 45, P, PRIZES, true).ends_with("\n\n⏳ **Starting in 10 s…**"));
        let one_house: Vec<Player> = (1..=5).map(|u| (u, G)).collect();
        let waiting = lobby_text(&one_house, 5, 2, 45, P, PRIZES, false);
        assert!(waiting.contains("**5/5 players · 🦁 1 house**") && waiting.ends_with("\n\n⏳ Waiting for someone from another house"), "{waiting}");
        assert!(!lobby_text(&one_house, 5, 1, 45, P, PRIZES, false).contains("houses**"), "no house rule shown when one house is enough");
        let many: Vec<Player> = (1..=45).map(|u| (u, G)).collect();
        assert!(lobby_text(&many, 5, 2, 45, P, PRIZES, false).ends_with("<@40> · and 5 more\n\n⏳ Waiting for someone from another house"));
        assert!(lobby_text(&[], 5, 2, 45, P, Prizes { first: 3, second: 0 }, false).contains("top two win 🥇 +3 house points"));
        assert!(lobby_text(&[], 5, 2, 45, P, Prizes { first: 0, second: 0 }, false).contains("top two win no house points"));
        let row = serde_json::to_value(lobby_row()).unwrap();
        assert_eq!((row["components"][0]["custom_id"].as_str(), row["components"][0]["label"].as_str()), (Some("npatjoin"), Some("✋ I'm in")));

        assert_eq!(round_text('P', 1_789_367_445, 3, RoundState::Live), "## Letter **P**\n**Name · Place · Animal · Thing**, each starting with **P**\n⏱️ Ends <t:1789367445:R>\n📝 **3 answers** in");
        assert!(round_text('P', 0, 1, RoundState::Judging).ends_with("⏱️ Time's up · 📝 **1 answer** in · judging…"));
        assert!(round_text('P', 0, 1, RoundState::Judged).ends_with("results below ⬇️"));
        assert!(round_text('P', 0, 1, RoundState::Stopped).ends_with("🛑 Stopped by a mod · no points"));
        let open = serde_json::to_value(round_row(12, true)).unwrap();
        assert_eq!((open["components"][0]["custom_id"].as_str(), open["components"][0]["disabled"].as_bool()), (Some("npatsubmit:12"), Some(false)));
        assert_eq!(serde_json::to_value(round_row(12, false)).unwrap()["components"][0]["disabled"], true);
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

    fn line(user: u64, answer: &str, score: i64) -> Line {
        Line {
            user,
            badge: "🦁".into(),
            answers: [answer.to_string(), "Parrot".into(), "Pen".into(), "".into()],
            marks: [Mark::Unique, Mark::Shared, Mark::Invalid, Mark::Blank],
            score,
            rank: 5,
            prize: 0,
        }
    }

    #[test]
    fn a_results_line_reads_as_agreed() {
        assert_eq!(result_line(&line(1234, "Pune", 15)), "<@1234> 🦁 — Pune ✅ · Parrot 🟰 · Pen ❌ · — · **15**");
        assert_eq!(result_line(&Line { rank: 1, prize: 2, ..line(1, "Pune", 35) }), "<@1> 🦁 — Pune ✅ · Parrot 🟰 · Pen ❌ · — · **35** 🥇 +2");
        assert!(result_line(&Line { rank: 2, prize: 1, ..line(1, "Pune", 30) }).ends_with("**30** 🥈 +1"));
        assert!(result_line(&Line { rank: 1, prize: 0, ..line(1, "Pune", 30) }).ends_with("**30** 🥇"), "a round that pays nothing still shows the medal");
        assert!(result_line(&Line { rank: 3, prize: 1, ..line(1, "Pune", 20) }).ends_with("**20** +1"), "3rd behind a Muggle still wins house points");
        let muggle = Line { badge: badge(MUGGLE), rank: 1, ..line(7, "Pune", 40) };
        assert!(result_line(&muggle).starts_with("<@7> 🧙 Muggle — ") && result_line(&muggle).ends_with("**40** 🥇"), "{}", result_line(&muggle));
        assert_eq!(badge("gryffindor"), "🦁");
        assert_eq!(badge("nowhere"), "");
        let odd = Line { badge: String::new(), answers: ["**P**<@1>".into(), "".into(), "".into(), "".into()], marks: [Mark::Invalid, Mark::Blank, Mark::Blank, Mark::Blank], ..line(9, "", 0) };
        assert_eq!(result_line(&odd), "<@9> — P@1 ❌ · — · — · — · **0**", "markdown and mentions in answers are defused");
    }

    #[test]
    fn results_fit_discord_and_say_how_the_round_went() {
        let (title, text, footer) = results_text(12, 'P', &[Line { rank: 1, prize: 2, ..line(1, "Pune", 35) }, Line { rank: 2, prize: 1, ..line(2, "Patna", 30) }], false, true, 3, P, PRIZES);
        assert_eq!(title, "Round 12 · Letter P");
        assert!(text.starts_with("<@1> 🦁 — Pune ✅"), "{text}");
        assert!(text.contains("\n🏠 **House points:** <@1> +2 · <@2> +1\n"), "{text}");
        assert!(!text.contains("Muggles"), "{text}");
        assert!(text.ends_with("-# ⚖️ Think an answer was judged wrong? Press Challenge within 30 min"), "{text}");
        assert!(!text.contains("letter only") && !text.contains("pays no"), "{text}");
        assert_eq!(footer, "✅ unique 10 · 🟰 shared 5 · 🥇 +2 🥈 +1 house points");

        let (_, text, _) = results_text(12, 'P', &[line(1, "Pune", 3)], true, false, 3, P, PRIZES);
        assert!(text.contains("-# ⚠️ Checked by letter only: the judge couldn't be reached"), "{text}");
        assert!(text.contains("-# Fewer than 3 people played, so this round pays no house points"), "{text}");
        let (_, empty, _) = results_text(12, 'P', &[], false, false, 3, P, PRIZES);
        assert_eq!(empty, "Nobody answered this round.");

        let long = "W".repeat(40);
        let crowd: Vec<Line> = (0..60)
            .map(|i| Line { answers: [long.clone(), long.clone(), long.clone(), long.clone()], marks: [Mark::Shared; 4], rank: i as usize + 1, prize: 2 - i.min(2) as i64, ..line(1_000_000_000_000_000_000 + i, "", 20) })
            .collect();
        let (_, text, _) = results_text(99, 'W', &crowd, true, true, 3, P, PRIZES);
        assert!(text.chars().count() <= DESCRIPTION_LIMIT, "{} chars", text.chars().count());
        let shown = text.lines().filter(|l| l.starts_with("<@")).count();
        assert!(shown > 0 && shown <= RESULT_LINES, "{shown} lines");
        assert!(text.contains(&format!("…and {} more", 60 - shown)), "{text}");
        assert!(text.ends_with("within 30 min"), "notes survive the trim");
        let short: Vec<Line> = (0..26).map(|i| line(1_000_000_000_000_000_000 + i, "Pune", 10)).collect();
        let (_, few, _) = results_text(99, 'P', &short[..25], false, true, 3, P, PRIZES);
        assert_eq!(few.lines().filter(|l| l.starts_with("<@")).count(), 25, "25 short lines all fit");
        assert!(!few.contains("more"), "{few}");
        let (_, over, _) = results_text(99, 'P', &short, false, true, 3, P, PRIZES);
        assert!(over.contains("\n…and 1 more\n"), "never more than 25 lines: {over}");

        // A Muggle on top: they keep 1st, the house points line names who got them.
        let lines = vec![
            Line { badge: badge(MUGGLE), rank: 1, ..line(7, "Pune", 40) },
            Line { rank: 2, prize: 2, ..line(8, "Patna", 30) },
            Line { badge: "🐍".into(), rank: 3, prize: 1, ..line(9, "Puri", 20) },
        ];
        let (_, text, _) = results_text(13, 'P', &lines, false, true, 3, P, PRIZES);
        assert!(text.contains("🏠 **House points:** <@8> +2 · <@9> +1"), "{text}");
        assert!(text.contains("-# 🧙 Muggles keep their place, but house points go to the two best house members"), "{text}");
        let rows = serde_json::to_value(results_rows(12, 2, true)).unwrap();
        assert_eq!(rows[0]["components"][0]["custom_id"], "npatchal:12");
        assert_eq!(rows[0]["components"][1]["label"], "🛡️ Review · 2 challenged");
        assert_eq!(serde_json::to_value(results_rows(12, 0, false)).unwrap()[0]["components"][1]["disabled"], true);
    }

    #[test]
    fn a_review_moves_house_points_with_the_places() {
        let row = |owed, credited| store::ScoreRow { owed, credited, ..Default::default() };
        // 1 was 2nd and is now 1st; 2 was 1st and is now 2nd; 3 was 2nd (capped to 0) and drops out; 4 climbs into 2nd.
        let before: HashMap<u64, store::ScoreRow> = [(1, row(1, 1)), (2, row(2, 2)), (3, row(1, 0)), (4, row(0, 0)), (5, row(0, 0))].into_iter().collect();
        let after: HashMap<u64, store::ScoreRow> = [(1, row(2, 1)), (2, row(1, 2)), (3, row(0, 0)), (4, row(1, 0)), (5, row(0, 0))].into_iter().collect();
        assert_eq!(review_fixes(&before, &after), vec![(1, 1), (2, -1), (4, 1)], "3 was never credited, so nothing is taken back");
        assert!(review_fixes(&before, &before).is_empty());
    }
}
