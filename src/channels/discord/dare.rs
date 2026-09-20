//! Truth or Dare, in its own channel (`VIZIER_DARE_CHANNEL`).
//!
//! The bot asks the ROOM, not a person. Nobody is picked, nobody is put on the
//! spot, and anyone who fancies a question answers it — several people can, and
//! usually do. That is the whole shape of the game, and it is what makes it
//! safe to leave running in a social channel.
//!
//! Like Guess the Movie and Letter Duel, ONE card is the channel's last message,
//! and the channel is one people can type in: answering IS typing. Nothing is
//! scored. No house points, no side tally, no board — nothing it asks can be
//! checked, so nothing it asks is paid for.
//!
//! Two vote counts run the whole thing:
//!
//! ```text
//!        ┌──────────── the card ────────────┐
//!        │  💬 Truth 1/2   ·   🔥 Dare 0/2  │   ← voting
//!        └──────────────────────────────────┘
//!                        │ a kind reaches 2
//!                        ▼
//!        ┌──────────────────────────────────┐
//!        │  💬 "the question"   ⏭️ Skip 1/3 │   ← open, anyone answers
//!        └──────────────────────────────────┘
//!                │                    │
//!      3 press skip            the room goes quiet
//!                └─────── back to voting ──────┘
//! ```
//!
//! A tally counts DISTINCT people, not presses ([`super::dare_store::tally`]),
//! so nobody carries a vote by leaning on a button. Both moves are a single
//! conditional `UPDATE`, so two votes landing in the same second put one
//! question up and three skips landing together end the round once.
//!
//! # A member's own question
//!
//! `✍️ Ask your own` opens a modal, and what somebody types goes up **with their
//! name on it**. That attribution is the moderation: an anonymous question is
//! something to hide behind, and a signed one is not. On top of it the text is
//! [screened](screen) — no mentions or `@everyone`, no invites, no links, length
//! capped, one per member per cooldown — and the room can bin it with the same
//! three votes as anything else.
//!
//! # The adult tier
//!
//! Tier 4 prompts exist and are truths only, and they need BOTH the spice
//! setting and Discord's own age-restricted flag on the channel
//! ([`adult_room`]). See [`super::dare_bank`].
//!
//! One task does all the posting ([`run`]). The handlers only write to the store
//! and set a flag, so nothing else in the game can put a card up.

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateCommand, CreateEmbed, CreateEmbedFooter, CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, CreateModal, EditMessage, GetMessages, InputTextStyle, Message, MessageId, ModalInteraction, ReactionType,
};

use super::control;
use super::dare_bank::{self as bank, Kind, Prompt};
use super::dare_store::{self as store, Status, Tally, Topic};
use super::rules_text::{self, DareRules};
use super::sudoku_gen::Rng;

const TICK: Duration = Duration::from_secs(1);
const HTTP_WAIT: Duration = Duration::from_secs(20);
const COLOUR: u32 = 0x9B59B6;
const TICK_MARK: &str = "✅";
const RECENT: usize = 10;

/// The channel the game plays in unless a mod moves it: 🎭 truth-or-dare.
pub const HOME_CHANNEL: u64 = 1_551_090_303_409_192_991;

/// What a member's own question may be, in characters.
pub const ASK_MIN: usize = 10;
pub const ASK_MAX: usize = 280;

// --- what the buttons are called -----------------------------------------------------------

pub const TRUTH_ID: &str = "daretruth";
pub const DARE_ID: &str = "daredare";
pub const SKIP_ID: &str = "dareskipvote";
pub const ASK_ID: &str = "dareask";
pub const HELP_ID: &str = "darehelp";
pub const ASK_MODAL: &str = "dareaskmodal";
const ASK_FIELD: &str = "question";

// --- settings -------------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_DARE", false)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_DARE_CHANNEL").or(Some(HOME_CHANNEL)).filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on())
}

/// How many people it takes to put a question up, and to be rid of one.
pub fn start_votes() -> i64 {
    control::number("VIZIER_DARE_START_VOTES", 2).clamp(1, 20) as i64
}

pub fn skip_votes() -> i64 {
    control::number("VIZIER_DARE_SKIP_VOTES", 3).clamp(1, 20) as i64
}

/// The highest tier the bank may reach into. Only ever half of it: the adult
/// tier also needs [`adult_room`], and the two are an AND.
pub fn spice() -> u8 {
    control::number("VIZIER_DARE_SPICE", bank::MIN_TIER as u64).clamp(bank::MIN_TIER as u64, bank::MAX_TIER as u64) as u8
}

/// Whether the game's channel is one Discord itself marks age-restricted.
///
/// Kept as a flag rather than read on demand because the help card and the
/// rules are built without a `Context` to ask. The task refreshes it every tick,
/// so ticking the box in Discord takes effect on the next question — and
/// unticking it does too.
static ADULT_ROOM: AtomicBool = AtomicBool::new(false);

pub fn adult_room() -> bool {
    ADULT_ROOM.load(Ordering::Relaxed)
}

/// Anything we cannot see counts as NOT age-restricted: the adult tier has to
/// be earned, never assumed.
fn note_adult_room(ctx: &Context, channel: Option<u64>) {
    let adult = channel
        .and_then(|channel| {
            ctx.cache
                .guilds()
                .iter()
                .find_map(|g| ctx.cache.guild(*g).and_then(|guild| guild.channels.get(&ChannelId::new(channel)).map(|c| c.nsfw)))
        })
        .unwrap_or(false);
    if ADULT_ROOM.swap(adult, Ordering::Relaxed) != adult {
        tracing::info!("dare: channel is {} age-restricted", if adult { "now" } else { "no longer" });
    }
}

/// How long a question may sit with nobody answering before the room has plainly
/// moved on.
fn idle_minutes() -> i64 {
    control::number("VIZIER_DARE_IDLE_MINUTES", 10).clamp(1, 1440) as i64
}

fn no_repeat_days() -> i64 {
    control::number("VIZIER_DARE_NO_REPEAT_DAYS", 14).min(365) as i64
}

/// How short a message may be and still count as an answer.
pub fn min_answer() -> usize {
    control::number("VIZIER_DARE_MIN_ANSWER", 15).clamp(1, 500) as usize
}

/// How long between one member's questions.
fn ask_cooldown_minutes() -> i64 {
    control::number("VIZIER_DARE_ASK_COOLDOWN", 10).min(1440) as i64
}

fn bump_messages() -> u64 {
    control::number("VIZIER_DARE_BUMP_MESSAGES", 6).max(1)
}

fn bump_seconds() -> i64 {
    control::number("VIZIER_DARE_BUMP_SECONDS", 45) as i64
}

pub fn dare_rules() -> DareRules {
    DareRules {
        channel: live_channel(),
        spice: spice(),
        adult: adult_room(),
        start_votes: start_votes(),
        skip_votes: skip_votes(),
        idle_minutes: idle_minutes(),
        min_answer: min_answer(),
        ask_cooldown: ask_cooldown_minutes(),
        truths: bank::count(Kind::Truth, spice(), adult_room()),
        dares: bank::count(Kind::Dare, spice(), adult_room()),
    }
}

// --- words -----------------------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

pub fn spice_words(tier: u8) -> &'static str {
    match tier {
        1 => "mild",
        2 => "sharper",
        3 => "bold",
        _ => "adult",
    }
}

/// "just now", "4 min", "2 h".
pub fn open_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

/// How a topic heads its card.
pub fn topic_words(topic: Topic, asked_by: Option<u64>) -> String {
    match (topic, asked_by) {
        (Topic::Truth, _) => "💬 Truth".to_string(),
        (Topic::Dare, _) => "🔥 Dare".to_string(),
        (Topic::Asked, Some(by)) => format!("✍️ Asked by <@{}>", by),
        (Topic::Asked, None) => "✍️ Asked".to_string(),
    }
}

/// The card while the room is deciding what it wants.
pub fn voting_text(t: Tally, need: i64, tier: u8, adult: bool) -> String {
    let mut text = String::from("# 🎭 Truth or Dare\n");
    text.push_str(&format!("What next? **{}** for either one puts it up.\n", plural(need, "vote", "votes")));
    text.push_str(&format!("💬 Truth **{}**/{} · 🔥 Dare **{}**/{}\n", t.truth, need, t.dare, need));
    text.push_str("Or **Ask your own** — your question, your name on it.\n");
    let tier = if adult && tier >= bank::ADULT_TIER { "adult" } else { spice_words(tier.min(bank::ADULT_TIER - 1)) };
    text.push_str(&format!("-# {} prompts · nobody is ever picked — anyone who wants to answers", tier));
    text
}

/// The card while a question is up.
pub fn question_text(topic: Topic, asked_by: Option<u64>, question: &str, skips: i64, need: i64, answers: i64, secs: i64) -> String {
    let mut text = format!("# {}\n", topic_words(topic, asked_by));
    text.push_str(&format!("> **{}**\n", question));
    text.push_str("Anyone can answer — just type below.\n");
    let mut notes = vec![format!("⏱️ up {}", open_words(secs))];
    notes.push(match answers {
        0 => "nobody yet".to_string(),
        n => format!("{} so far", plural(n, "answer", "answers")),
    });
    notes.push(format!("⏭️ {}/{} to skip", skips, need));
    text.push_str(&format!("-# {}", notes.join(" · ")));
    text
}

pub const CARD_FOOTER: &str = "Answer by typing · nobody is put on the spot · /darehelp";

/// The line when the room votes a question away. An expiry says nothing at all
/// — the room went quiet, and talking into a quiet room is how a bot becomes
/// noise.
pub fn skipped_text(topic: Topic, question: &str, by_mod: bool, votes: i64) -> String {
    let what = question.trim_end_matches(['.', '!', '?']);
    let kind = match topic {
        Topic::Asked => "that one",
        _ => "it",
    };
    match by_mod {
        true => format!("⏭️ A mod took {} down — *{}*. Voting's open again.", kind, what),
        false => format!("⏭️ Skipped — *{}*. {} had had enough. Voting's open again.", what, plural(votes, "person", "people")),
    }
}

// --- screening a member's own question ---------------------------------------------------------

/// What a member typed, cleaned up — or why it can't go up.
///
/// The bot is about to print this to the whole channel under its own name, so
/// the checks are deliberately blunt. Anything that pings, advertises or leads
/// somewhere else is refused outright rather than stripped and posted anyway:
/// silently changing what somebody wrote and then signing their name to it is
/// worse than telling them no.
pub fn screen(text: &str) -> Result<String, &'static str> {
    let cleaned = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = cleaned.chars().count();
    if count < ASK_MIN {
        return Err("That's too short to be a question — give it a few more words.");
    }
    if count > ASK_MAX {
        return Err("That's too long for a card. Trim it to a sentence or two.");
    }
    if super::automod::has_everyone(&cleaned) {
        return Err("No @everyone or @here, sorry.");
    }
    if cleaned.contains("<@") || cleaned.contains("<#") || cleaned.contains("<@&") {
        return Err("Questions can't mention anybody — ask the room, not one person.");
    }
    if super::automod::has_invite(&cleaned) {
        return Err("No invite links in a question.");
    }
    let lower = cleaned.to_lowercase();
    if lower.contains("http://") || lower.contains("https://") || lower.contains("www.") {
        return Err("No links in a question — just the words.");
    }
    if super::automod::is_wall(&cleaned, 120, 70) {
        return Err("That reads as a wall rather than a question.");
    }
    Ok(cleaned)
}

/// Whether a message counts as an answer. Anyone may answer; it just has to be
/// long enough to be one.
pub fn is_answer(content: &str, floor: usize) -> bool {
    content.trim().chars().count() >= floor
}

/// Whether the room has plainly moved on.
pub fn stale(since: i64, now: i64, idle_minutes: i64) -> bool {
    now - since >= idle_minutes.max(1) * 60
}

/// Which kind the vote has carried, if either has. A tie cannot happen — the
/// task looks after every vote, so one of them crosses first — but if the
/// thresholds are ever changed under a live round, truth takes it.
pub fn carried(t: Tally, need: i64) -> Option<Kind> {
    if t.truth >= need {
        return Some(Kind::Truth);
    }
    if t.dare >= need {
        return Some(Kind::Dare);
    }
    None
}

// --- shared state -----------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    card: Option<(u64, u64)>,
    orphans: Vec<(u64, u64)>,
    others_since_card: u64,
    last_bump_ms: i64,
    card_gone: bool,
    dirty: bool,
    ended: Option<i64>,
    admin_skip: Option<u64>,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

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

pub fn on_delete(channel: ChannelId, id: MessageId) {
    if Some(channel.get()) != channel_setting() {
        return;
    }
    let mut s = SHARED.lock();
    if s.card.map(|(_, m)| m) == Some(id.get()) {
        s.card_gone = true;
    }
}

// --- Discord helpers --------------------------------------------------------------------------

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
    match call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        Ok(()) => {}
        Err(err) if is_gone(&err) => {}
        Err(err) => {
            tracing::warn!("dare: card {} not deleted ({}) — trying again", message, err);
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

async fn say(ctx: &Context, channel: u64, text: String) {
    let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        tracing::warn!("dare: line not posted in {}: {}", channel, err);
    }
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), key, value);
    }
}

fn with_db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> Option<T> {
    store::db().map(|db| f(&db.lock()))
}

fn live_row() -> Option<store::Row> {
    with_db(store::live).flatten()
}

/// The words a round is actually showing, whoever wrote them.
fn question_of(row: &store::Row) -> Option<String> {
    match row.topic {
        Some(Topic::Asked) => row.text.clone(),
        _ => row.prompt_key.as_deref().and_then(bank::find).map(|p: Prompt| p.text.to_string()),
    }
}

// --- the card ---------------------------------------------------------------------------------

fn help_button() -> CreateButton {
    CreateButton::new(HELP_ID).label("❓ How to play").style(ButtonStyle::Secondary)
}

fn ask_button() -> CreateButton {
    CreateButton::new(ASK_ID).label("✍️ Ask your own").style(ButtonStyle::Secondary)
}

fn buttons(row: &store::Row, t: Tally, need: i64, skip_need: i64) -> Vec<CreateActionRow> {
    let row = match row.status {
        Status::Open => vec![
            CreateButton::new(SKIP_ID).label(format!("⏭️ Skip {}/{}", t.skip, skip_need)).style(ButtonStyle::Secondary),
            ask_button(),
            help_button(),
        ],
        _ => vec![
            CreateButton::new(TRUTH_ID).label(format!("💬 Truth {}/{}", t.truth, need)).style(ButtonStyle::Primary),
            CreateButton::new(DARE_ID).label(format!("🔥 Dare {}/{}", t.dare, need)).style(ButtonStyle::Danger),
            ask_button(),
            help_button(),
        ],
    };
    vec![CreateActionRow::Buttons(row)]
}

fn card_embed(row: &store::Row, t: Tally, now: i64) -> CreateEmbed {
    let description = match (row.status, row.topic) {
        (Status::Open, Some(topic)) => {
            let question = question_of(row).unwrap_or_else(|| "…".to_string());
            question_text(topic, row.asked_by, &question, t.skip, skip_votes(), row.answers, now - row.opened_ts.unwrap_or(row.posted_ts))
        }
        _ => voting_text(t, start_votes(), spice(), adult_room()),
    };
    CreateEmbed::new().description(description).colour(COLOUR).footer(CreateEmbedFooter::new(CARD_FOOTER))
}

/// Mentions never ping. A member's question carries their name so the room
/// knows whose it is, not so they get a notification every time the card moves.
fn card_message(row: &store::Row, t: Tally, now: i64) -> CreateMessage {
    CreateMessage::new()
        .embed(card_embed(row, t, now))
        .components(buttons(row, t, start_votes(), skip_votes()))
        .allowed_mentions(CreateAllowedMentions::new())
}

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
            tracing::warn!("dare: card not posted in {}: {}", channel, err);
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

async fn edit_card(ctx: &Context, row: &store::Row, t: Tally, now: i64) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let edit = EditMessage::new()
        .embed(card_embed(row, t, now))
        .components(buttons(row, t, start_votes(), skip_votes()))
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        if is_gone(&err) {
            SHARED.lock().card_gone = true;
        }
        tracing::warn!("dare: card not edited: {}", err);
    }
}

pub fn bump_due(others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> bool {
    others >= needed.max(1) && since_bump_ms >= every_secs.max(0) * 1_000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Nothing,
    Edit,
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

async fn tend_card(ctx: &Context, channel: u64, row: &store::Row, t: Tally, now: i64) -> Option<u64> {
    let (card, plan) = {
        let s = SHARED.lock();
        let since_bump = Utc::now().timestamp_millis() - s.last_bump_ms;
        let right = s.card.map(|(c, _)| c) == Some(channel);
        (s.card, card_plan(s.card.is_some(), right, s.card_gone, s.dirty, s.others_since_card, since_bump, bump_messages(), bump_seconds()))
    };
    match plan {
        CardAction::Nothing => None,
        CardAction::Edit => {
            edit_card(ctx, row, t, now).await;
            SHARED.lock().dirty = false;
            None
        }
        CardAction::Bump => {
            let gone = SHARED.lock().card_gone;
            let replace = card.is_some() && !gone;
            let posted = place_card(ctx, channel, card_message(row, t, now), replace, true).await;
            if let Some(id) = posted {
                let _ = with_db(|conn| store::set_message(conn, row.id, id));
            }
            posted
        }
    }
}

// --- the task ---------------------------------------------------------------------------------

pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting()) {
        (true, Some(c)) => tracing::info!("dare: playing in {} ({} prompts)", c, spice_words(spice())),
        (true, None) => tracing::info!("dare: on, but VIZIER_DARE_CHANNEL points nowhere it may play"),
        (false, _) => tracing::info!("dare: VIZIER_DARE is off"),
    }
    tokio::spawn(run(ctx));
}

async fn recover(ctx: &Context) -> Option<store::Row> {
    let channel = active_channel(ctx);
    let live = live_row();
    if let Some(c) = channel {
        let read = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await.is_ok();
        let mut s = SHARED.lock();
        s.others_since_card = 0;
        s.last_bump_ms = Utc::now().timestamp_millis();
        if !read {
            tracing::debug!("dare: couldn't read {} on waking", c);
        }
    }
    let old_card = with_db(|conn| store::meta_get(conn, "card"))
        .flatten()
        .as_deref()
        .and_then(|v| v.split_once(':'))
        .and_then(|(c, m)| Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?)));
    if let Some((c, m)) = old_card.filter(|(c, _)| Some(*c) == channel) {
        if exists(ctx, c, m).await {
            SHARED.lock().card = Some((c, m));
        }
    }
    match live {
        Some(row) if channel == Some(row.channel) => {
            tracing::info!("dare: round {} picked up where it was left", row.id);
            Some(row)
        }
        Some(moved) => {
            let _ = with_db(|conn| store::expire(conn, moved.id, Utc::now().timestamp()));
            tracing::info!("dare: round {} closed — the game moved", moved.id);
            None
        }
        None => None,
    }
}

async fn run(ctx: Context) {
    let mut live = recover(&ctx).await;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        note_adult_room(&ctx, channel);
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

        let mut ending = ended;
        if let (Some(by), Some(row)) = (admin_skip, live.as_ref()) {
            if with_db(|conn| store::skip(conn, row.id, Some(by), now)).and_then(Result::ok).unwrap_or(false) {
                tracing::info!("dare: round {} taken down by mod {}", row.id, by);
                ending = Some(row.id);
            }
        }

        // The vote counts are read HERE and nowhere else: the handlers only
        // record a press, so a question can never be put up twice.
        if ending.is_none() {
            if let Some(row) = live.as_ref() {
                let t = with_db(|conn| store::tally(conn, row.id)).unwrap_or_default();
                match row.status {
                    Status::Voting => {
                        if let Some(kind) = carried(t, start_votes()) {
                            open_question(row.id, kind, now);
                        }
                    }
                    Status::Open if t.skip >= skip_votes() => {
                        if with_db(|conn| store::skip(conn, row.id, None, now)).and_then(Result::ok).unwrap_or(false) {
                            tracing::info!("dare: round {} skipped by {} votes", row.id, t.skip);
                            ending = Some(row.id);
                        }
                    }
                    // Nobody has answered for long enough that the room has
                    // plainly moved on. This ends the round in SILENCE — see
                    // the note on `skipped_text`.
                    Status::Open if stale(row.touched_ts(), now, idle_minutes()) => {
                        if with_db(|conn| store::expire(conn, row.id, now)).and_then(Result::ok).unwrap_or(false) {
                            tracing::info!("dare: round {} ran quiet after {} min", row.id, idle_minutes());
                            ending = Some(row.id);
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(id) = ending {
            let row = with_db(|conn| store::get(conn, id)).flatten();
            if let Some(row) = row.filter(|r| r.status == Status::Skipped) {
                let question = question_of(&row).unwrap_or_default();
                let votes = with_db(|conn| store::tally(conn, row.id)).unwrap_or_default().skip;
                if let Some(topic) = row.topic {
                    say(&ctx, channel, skipped_text(topic, &question, row.ended_by.is_some(), votes)).await;
                }
            }
            live = None;
        }

        // The store is the truth.
        live = match live {
            Some(row) => with_db(|conn| store::get(conn, row.id)).flatten().filter(|r| r.status.live() && r.channel == channel),
            None => live_row().filter(|r| r.channel == channel),
        };
        if live.is_none() {
            live = with_db(|conn| store::start_vote(conn, channel, now)).and_then(Result::ok);
            if live.is_some() {
                SHARED.lock().dirty = true;
            }
        }
        let Some(row) = live.clone() else { continue };
        let t = with_db(|conn| store::tally(conn, row.id)).unwrap_or_default();
        if let Some(id) = tend_card(&ctx, channel, &row, t, now).await {
            live = Some(store::Row { message: Some(id), ..row });
        }
    }
}

/// Turns a round that has just carried its vote into the question itself.
fn open_question(round: i64, kind: Kind, now: i64) {
    let since = now - no_repeat_days() * 86_400;
    let seen = with_db(|conn| store::asked_since(conn, since)).unwrap_or_default();
    let mut rng = Rng::fresh();
    let Some(prompt) = bank::pick(kind, spice(), adult_room(), &seen, &mut rng) else {
        tracing::warn!("dare: the bank has no {} to ask", kind.key());
        return;
    };
    let topic = match kind {
        Kind::Truth => Topic::Truth,
        Kind::Dare => Topic::Dare,
    };
    if with_db(|conn| store::open_with(conn, round, topic, prompt.key, now)).and_then(Result::ok).unwrap_or(false) {
        tracing::info!("dare: round {} is a {} ({})", round, kind.key(), prompt.key);
        SHARED.lock().dirty = true;
    }
}

// --- what people type --------------------------------------------------------------------------

/// Every human message in the game's channel. While a question is up, anything
/// long enough counts as an answer — from anyone. Nothing is ever refused or
/// corrected in public: the room is a social one.
pub async fn on_message(ctx: &Context, msg: &Message) {
    let Some(channel) = live_channel() else { return };
    if msg.channel_id.get() != channel || msg.author.bot {
        return;
    }
    let Some(row) = live_row().filter(|r| r.status == Status::Open) else { return };
    if !is_answer(&msg.content, min_answer()) {
        return;
    }
    let now = Utc::now().timestamp();
    let first = row.answers == 0;
    if !with_db(|conn| store::note_answer(conn, row.id, msg.id.get(), now)).and_then(Result::ok).unwrap_or(false) {
        return;
    }
    // Only the FIRST answer is ticked. A ✅ on every reply in a busy channel is
    // a wall of green, and the card's own count says how many there have been.
    if first {
        if let Err(err) = call(msg.react(&ctx.http, ReactionType::Unicode(TICK_MARK.to_string()))).await {
            tracing::debug!("dare: tick not added to {}: {}", msg.id, err);
        }
    }
    SHARED.lock().dirty = true;
}

// --- the buttons ---------------------------------------------------------------------------------

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let message = CreateInteractionResponseMessage::new().content(text.into()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::debug!("dare: reply to {} not sent: {}", component.user.id, err);
    }
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.as_str();
    if id == HELP_ID {
        let message = CreateInteractionResponseMessage::new().embed(help_embed()).ephemeral(true);
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
        return;
    }
    if live_channel().is_none() {
        return whisper(ctx, component, OFF).await;
    }
    match id {
        TRUTH_ID => voted(ctx, component, "truth").await,
        DARE_ID => voted(ctx, component, "dare").await,
        SKIP_ID => voted(ctx, component, "skip").await,
        ASK_ID => ask_pressed(ctx, component).await,
        _ => {}
    }
}

/// A vote, or taking one back. Pressing the same button twice un-votes, so
/// somebody who changes their mind isn't stuck holding a question up.
async fn voted(ctx: &Context, component: &ComponentInteraction, choice: &str) {
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(row) = live_row() else {
        return whisper(ctx, component, "Nothing to vote on just now.").await;
    };
    let wanted_open = choice == "skip";
    if (row.status == Status::Open) != wanted_open {
        return whisper(ctx, component, "That's moved on — have another look at the card.").await;
    }
    let held = with_db(|conn| store::voted(conn, row.id, user, choice)).unwrap_or(false);
    let text = if held {
        let _ = with_db(|conn| store::unvote(conn, row.id, user, choice));
        "Vote taken back."
    } else {
        let _ = with_db(|conn| store::vote(conn, row.id, user, choice, now));
        match choice {
            "skip" => "⏭️ Noted — that's your vote to move on.",
            "dare" => "🔥 Noted.",
            _ => "💬 Noted.",
        }
    };
    SHARED.lock().dirty = true;
    whisper(ctx, component, text).await;
}

/// `✍️ Ask your own` — opens the modal, once the cooldown is clear and there is
/// somewhere for the question to go.
async fn ask_pressed(ctx: &Context, component: &ComponentInteraction) {
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(row) = live_row() else {
        return whisper(ctx, component, "Nothing's running just now.").await;
    };
    if row.status == Status::Open {
        return whisper(ctx, component, "There's a question up already — skip it first, then the floor's yours.").await;
    }
    if let Some(wait) = ask_wait(user, now) {
        return whisper(ctx, component, format!("You asked one recently — another in about {}.", open_words(wait))).await;
    }
    let modal = CreateModal::new(ASK_MODAL, "Ask the room").components(vec![CreateActionRow::InputText(
        CreateInputText::new(InputTextStyle::Paragraph, "Your question", ASK_FIELD)
            .placeholder("Asked to the whole channel, with your name on it")
            .min_length(ASK_MIN as u16)
            .max_length(ASK_MAX as u16)
            .required(true),
    )]);
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await {
        tracing::warn!("dare: ask modal not opened for {}: {}", user, err);
    }
}

/// How long this member still has to wait, if they do.
fn ask_wait(user: u64, now: i64) -> Option<i64> {
    let cooldown = ask_cooldown_minutes() * 60;
    if cooldown <= 0 {
        return None;
    }
    let last = with_db(|conn| store::last_asked_by(conn, user)).flatten()?;
    let left = last + cooldown - now;
    (left > 0).then_some(left)
}

async fn reply_modal(ctx: &Context, modal: &ModalInteraction, text: impl Into<String>) {
    let message =
        CreateInteractionResponseMessage::new().content(text.into()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = modal.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::debug!("dare: modal reply to {} not sent: {}", modal.user.id, err);
    }
}

/// Reads the one field out of a submitted modal, whatever serenity's shape for
/// it happens to be.
pub fn asked_words(data: &serde_json::Value) -> Option<String> {
    let rows = data.get("components")?.as_array()?;
    for row in rows {
        for field in row.get("components")?.as_array()? {
            if field.get("custom_id").and_then(|v| v.as_str()) == Some(ASK_FIELD) {
                return field.get("value").and_then(|v| v.as_str()).map(str::to_string);
            }
        }
    }
    None
}

pub async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    if modal.data.custom_id != ASK_MODAL {
        return;
    }
    let user = modal.user.id.get();
    let now = Utc::now().timestamp();
    if live_channel().is_none() {
        return reply_modal(ctx, modal, OFF).await;
    }
    let data = serde_json::to_value(&modal.data).unwrap_or(serde_json::Value::Null);
    let Some(raw) = asked_words(&data) else {
        return reply_modal(ctx, modal, "That didn't come through — try again.").await;
    };
    let question = match screen(&raw) {
        Ok(question) => question,
        Err(why) => return reply_modal(ctx, modal, why).await,
    };
    let Some(row) = live_row().filter(|r| r.status == Status::Voting) else {
        return reply_modal(ctx, modal, "Somebody got a question up while you were typing. Try again in a moment.").await;
    };
    // The cooldown is checked again here: the modal may have sat open a while.
    if let Some(wait) = ask_wait(user, now) {
        return reply_modal(ctx, modal, format!("You asked one recently — another in about {}.", open_words(wait))).await;
    }
    if !with_db(|conn| store::open_asked(conn, row.id, &question, user, now)).and_then(Result::ok).unwrap_or(false) {
        return reply_modal(ctx, modal, "That round moved on — try again.").await;
    }
    SHARED.lock().dirty = true;
    tracing::info!("dare: round {} is {}'s own question", row.id, user);
    reply_modal(ctx, modal, "✍️ It's up, with your name on it. Anyone can answer — and the room can skip it like any other.").await;
}

// --- the commands ----------------------------------------------------------------------------------

pub fn mine_builder() -> CreateCommand {
    CreateCommand::new("dare").description("truth or dare: what's up now and how the voting stands")
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("darehelp").description("how Truth or Dare works, from the settings as they are now")
}

pub fn skip_builder() -> CreateCommand {
    CreateCommand::new("dareskip").description("admin only: take the question that's up down")
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("darestop").description("admin only: switch Truth or Dare off and take the card down")
}

const OFF: &str = "Truth or Dare is switched off right now.";

async fn reply(ctx: &Context, command: &CommandInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message.ephemeral(true))).await {
        tracing::warn!("dare: reply to {} not sent: {}", command.user.id, err);
    }
}

pub fn mine_text(live: Option<&store::Row>, question: Option<&str>, t: Tally, channel: Option<u64>, now: i64) -> String {
    let Some(channel) = channel else { return OFF.to_string() };
    let mut text = format!("🎭 **Truth or Dare** is in <#{}>.\n", channel);
    match live.map(|r| (r.status, r.topic, r)) {
        Some((Status::Open, Some(topic), row)) => {
            text.push_str(&format!("**Up now:** {} — *{}*\n", topic_words(topic, row.asked_by), question.unwrap_or("…")));
            text.push_str(&format!(
                "{} so far · ⏭️ **{}**/{} voting to move on · up {}\n",
                plural(row.answers, "answer", "answers"),
                t.skip,
                skip_votes(),
                open_words(now - row.opened_ts.unwrap_or(row.posted_ts))
            ));
        }
        _ => {
            text.push_str(&format!("Nothing up — the room is voting. 💬 **{}**/{} · 🔥 **{}**/{}\n", t.truth, start_votes(), t.dare, start_votes()));
        }
    }
    text.push_str("-# Anyone can answer, and anyone can **Ask your own** — nobody is ever picked.");
    text
}

pub async fn mine_command(ctx: &Context, command: &CommandInteraction) {
    let live = live_row();
    let question = live.as_ref().and_then(question_of);
    let t = live.as_ref().and_then(|r| with_db(|conn| store::tally(conn, r.id))).unwrap_or_default();
    let text = mine_text(live.as_ref(), question.as_deref(), t, live_channel(), Utc::now().timestamp());
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

fn help_embed() -> CreateEmbed {
    CreateEmbed::new().title("🎭 Truth or Dare").description(rules_text::dare_help_text(&dare_rules())).colour(COLOUR)
}

pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    reply(ctx, command, CreateInteractionResponseMessage::new().embed(help_embed())).await;
}

pub async fn skip_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can take a question down.")).await;
    }
    if live_channel().is_none() {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content(OFF)).await;
    }
    let live = live_row().filter(|r| r.status == Status::Open).map(|r| r.id);
    let text = match live {
        Some(id) => {
            SHARED.lock().admin_skip = Some(user);
            tracing::info!("dare: /dareskip by {} (round {})", user, id);
            "⏭️ Taken down. Voting opens again in a moment."
        }
        None => "Nothing is up to take down.",
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can stop the game.")).await;
    }
    if let Some(row) = live_row() {
        let _ = with_db(|conn| store::skip(conn, row.id, Some(user), Utc::now().timestamp()));
    }
    let text = match control::set("VIZIER_DARE", Some("off"), user) {
        Ok(()) => {
            tracing::info!("dare: /darestop by {}", user);
            "🛑 Truth or Dare is off. The card comes down in a moment."
        }
        Err(err) => {
            tracing::warn!("dare: /darestop by {} failed: {}", user, err);
            "The setting wouldn't save — switch **Game on** off in the panel instead."
        }
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

pub fn recent_rounds(limit: usize) -> Vec<store::Row> {
    with_db(|conn| store::recent(conn, limit.min(RECENT))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(truth: i64, dare: i64, skip: i64) -> Tally {
        Tally { truth, dare, skip }
    }

    #[test]
    fn a_vote_carries_only_at_the_threshold() {
        assert_eq!(carried(tally(1, 1, 0), 2), None);
        assert_eq!(carried(tally(2, 1, 0), 2), Some(Kind::Truth));
        assert_eq!(carried(tally(0, 2, 0), 2), Some(Kind::Dare));
        assert_eq!(carried(tally(3, 3, 0), 2), Some(Kind::Truth), "a tie is broken, never left hanging");
        assert_eq!(carried(tally(0, 0, 9), 2), None, "skip votes never start anything");
    }

    #[test]
    fn the_voting_card_shows_where_the_room_has_got_to() {
        let text = voting_text(tally(1, 0, 0), 2, 1, false);
        assert!(text.contains("💬 Truth **1**/2"));
        assert!(text.contains("🔥 Dare **0**/2"));
        assert!(text.contains("Ask your own"));
        assert!(text.contains("nobody is ever picked"));
    }

    /// The card says what is actually in play, not what the setting asks for:
    /// spice 4 in an ordinary channel is not an adult game.
    #[test]
    fn the_card_never_claims_an_adult_game_it_cannot_run() {
        assert!(voting_text(tally(0, 0, 0), 2, bank::ADULT_TIER, false).contains("bold"));
        assert!(voting_text(tally(0, 0, 0), 2, bank::ADULT_TIER, true).contains("adult"));
        assert!(voting_text(tally(0, 0, 0), 2, 1, true).contains("mild"), "an adult room still obeys the setting");
    }

    #[test]
    fn a_question_card_carries_its_counts() {
        let text = question_text(Topic::Truth, None, "what keeps you up?", 1, 3, 2, 90);
        assert!(text.contains("💬 Truth"));
        assert!(text.contains("what keeps you up?"));
        assert!(text.contains("2 answers so far"));
        assert!(text.contains("⏭️ 1/3 to skip"));
        assert!(question_text(Topic::Dare, None, "q", 0, 3, 0, 0).contains("nobody yet"));
        assert!(question_text(Topic::Truth, None, "q", 0, 3, 1, 0).contains("1 answer so far"));
    }

    /// A member's question is always signed.
    #[test]
    fn an_asked_question_carries_its_asker() {
        assert_eq!(topic_words(Topic::Asked, Some(7)), "✍️ Asked by <@7>");
        assert!(question_text(Topic::Asked, Some(7), "go on then", 0, 3, 0, 0).contains("<@7>"));
    }

    #[test]
    fn a_skip_line_says_who_wanted_it_gone() {
        assert!(skipped_text(Topic::Truth, "the question.", false, 3).contains("3 people had had enough"));
        assert!(!skipped_text(Topic::Truth, "the question.", false, 3).contains(".*"), "the prompt's own stop is not doubled");
        assert!(skipped_text(Topic::Asked, "the question", true, 0).contains("A mod"));
    }

    #[test]
    fn anyone_can_answer_if_they_say_something_real() {
        assert!(is_answer("a proper honest answer", 15));
        assert!(!is_answer("lol", 15));
        assert!(!is_answer("              ", 15));
        assert!(is_answer("lol", 1));
    }

    #[test]
    fn a_question_runs_quiet_only_after_the_window() {
        assert!(!stale(0, 599, 10));
        assert!(stale(0, 600, 10));
        assert!(stale(0, 60, 0));
    }

    #[test]
    fn the_card_moves_for_the_right_reasons() {
        assert_eq!(card_plan(false, true, false, false, 0, 0, 6, 45), CardAction::Bump);
        assert_eq!(card_plan(true, false, false, false, 0, 0, 6, 45), CardAction::Bump);
        assert_eq!(card_plan(true, true, true, false, 0, 0, 6, 45), CardAction::Bump);
        assert_eq!(card_plan(true, true, false, false, 6, 45_000, 6, 45), CardAction::Bump);
        assert_eq!(card_plan(true, true, false, false, 6, 1_000, 6, 45), CardAction::Nothing);
        assert_eq!(card_plan(true, true, false, true, 0, 0, 6, 45), CardAction::Edit);
    }

    // --- screening -----------------------------------------------------------

    #[test]
    fn a_good_question_comes_back_tidied() {
        assert_eq!(screen("  what is   your\nfavourite film? ").expect("ok"), "what is your favourite film?");
    }

    #[test]
    fn a_question_may_not_ping_or_advertise_or_lead_away() {
        for bad in [
            "hey <@12345> what do you think",
            "what does @everyone think of this",
            "@here what's going on",
            "thoughts on discord.gg/abcdef",
            "have a look at https://example.com and say",
            "check out www.example.com for this",
            "what about <#987654321> then",
        ] {
            assert!(screen(bad).is_err(), "should have been refused: {}", bad);
        }
    }

    #[test]
    fn a_question_has_to_be_a_sensible_length() {
        assert!(screen("hi").is_err());
        // A real question that just fits, and the same one word too long.
        let long = "what is the single best thing ".repeat(9);
        let long = long.trim();
        assert_eq!(long.chars().count(), 269);
        assert!(screen(long).is_ok());
        assert!(screen(&format!("{} and also quite a lot more words on the end of it", long)).is_err());
        // Whitespace is collapsed BEFORE the length is judged, so padding a
        // short question out with spaces doesn't sneak it past.
        assert!(screen("hi        there        you").is_ok());
        assert!(screen("  hi  ").is_err());
    }

    /// Length alone isn't enough: a long question still has to read as one.
    #[test]
    fn a_wall_is_not_a_question() {
        assert!(screen(&"a".repeat(200)).is_err());
        assert!(screen(&"lol ".repeat(40)).is_err());
    }

    #[test]
    fn the_modal_field_is_read_back_out() {
        let data = serde_json::json!({
            "components": [{ "components": [{ "custom_id": ASK_FIELD, "value": "what did you dream about" }] }]
        });
        assert_eq!(asked_words(&data).as_deref(), Some("what did you dream about"));
        assert!(asked_words(&serde_json::json!({ "components": [] })).is_none());
        assert!(asked_words(&serde_json::Value::Null).is_none());
    }

    #[test]
    fn the_private_line_says_what_is_up_and_how_the_vote_stands() {
        assert_eq!(mine_text(None, None, Tally::default(), None, 100), OFF);
        let voting = mine_text(None, None, tally(1, 0, 0), Some(5), 100);
        assert!(voting.contains("the room is voting") && voting.contains("💬 **1**"));
    }

    #[test]
    fn spice_reads_as_words() {
        assert_eq!(spice_words(1), "mild");
        assert_eq!(spice_words(3), "bold");
        assert_eq!(spice_words(bank::ADULT_TIER), "adult");
    }

    #[test]
    fn the_adult_tier_is_off_until_the_channel_says_otherwise() {
        assert!(!adult_room(), "the flag starts shut");
    }
}
