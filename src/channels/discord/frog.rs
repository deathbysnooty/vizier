//! The Chocolate Frog: a small card that hops into a busy channel a few times a
//! day with a wizard on it and a riddle behind a button. The first house member
//! to type the right answer keeps the card - a numbered collectable - and scores
//! the frog's points for their house.
//!
//! Many people can press "Catch it" at once. Each gets the riddle in a private
//! pop-up and three tries; wrong answers are only ever shown to whoever typed
//! them. The first right answer wins, decided in one database transaction (see
//! `frog_store`), and the card in chat changes to say who caught it. Nobody in
//! five minutes and the frog hops away, showing the answer but never the riddle.
//!
//! Drops follow the Snitch's pattern - a plan per India day, spaced apart, only
//! into a channel someone spoke in lately - and stay clear of the Snitch's own
//! drops. The whole game is off until `VIZIER_FROGS` is switched on; every
//! number is a `VIZIER_FROG_…` setting read when it is needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    EditAttachments, EditMessage, MessageId, ModalInteraction, ReactionType,
};

use super::control;
use super::frog_store::{self as store, Card, Drop, MAX_TRIES, Rarity, Status, Submit, Wizard};
use super::house::House;
use super::points::{Outcome, Source};

/// How often the scheduler wakes to see whether a drop is due.
const TICK: Duration = Duration::from_secs(30);
/// When a drop is due but every channel is quiet, look again after this long.
const RETRY_AFTER: i64 = 3 * 60;
/// India is UTC+5:30 all year.
const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;
const DAY: i64 = 86_400;
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Waits before each attempt at editing a card to how its frog ended.
const EDIT_RETRIES: [u64; 3] = [0, 10, 60];
const ESCAPED_COLOUR: u32 = 0x4E5058;
/// Cards /frogs lists before "…and N more".
const CARD_LINES: usize = 15;

// --- settings -------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_FROGS", false)
}

fn open_secs() -> i64 {
    control::number("VIZIER_FROG_OPEN_MINUTES", 5).clamp(1, 60) as i64 * 60
}

fn min_gap() -> i64 {
    control::number("VIZIER_FROG_GAP_MINUTES", 90).max(1) as i64 * 60
}

fn window_start() -> i64 {
    control::number("VIZIER_FROG_START_HOUR", 0).min(23) as i64 * 3600
}

/// The last drop lands one open window before the end hour, so it is over by then.
fn window_end() -> i64 {
    control::number("VIZIER_FROG_END_HOUR", 24).clamp(1, 24) as i64 * 3600 - open_secs()
}

fn drops_per_day() -> (usize, usize) {
    let low = control::number("VIZIER_FROG_DROPS_MIN", 5) as usize;
    let high = control::number("VIZIER_FROG_DROPS_MAX", 7) as usize;
    (low.min(high), low.max(high))
}

fn quiet_after() -> i64 {
    control::number("VIZIER_FROG_QUIET_MINUTES", 5).max(1) as i64 * 60
}

fn snitch_gap() -> i64 {
    control::number("VIZIER_FROG_SNITCH_GAP_MINUTES", 20) as i64 * 60
}

/// Where frogs drop: their own channels, or the Snitch's when none are set -
/// never #safe-corner. `None` means nowhere.
fn channels() -> Option<Vec<(u64, u32)>> {
    let own = control::var("VIZIER_FROG_CHANNELS").map(|raw| super::snitch::parse_weighted(&raw)).filter(|l| !l.is_empty());
    let list: Vec<(u64, u32)> =
        own.or_else(super::snitch::channels)?.into_iter().filter(|(id, _)| *id != super::weekly::SAFE_CORNER).collect();
    (!list.is_empty()).then_some(list)
}

// --- the day's plan -------------------------------------------------------------

fn ist_midnight(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(DAY) * DAY - IST_OFFSET
}

fn in_window(ts: i64) -> bool {
    let into_day = ts - ist_midnight(ts);
    (window_start()..=window_end()).contains(&into_day)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Plan {
    day: String,
    times: Vec<i64>,
    done: usize,
    last_drop: Option<i64>,
}

/// `want` times between `start` and `end`, at least `gap` apart; fewer if they
/// don't fit.
fn spread(start: i64, end: i64, want: usize, gap: i64, mut roll: impl FnMut() -> f64) -> Vec<i64> {
    if end < start {
        return Vec::new();
    }
    let fits = 1 + ((end - start) / gap) as usize;
    let n = want.min(fits);
    let slack = (end - start) - gap * n.saturating_sub(1) as i64;
    let mut offsets: Vec<i64> = (0..n).map(|_| (roll().clamp(0.0, 1.0) * slack as f64) as i64).collect();
    offsets.sort_unstable();
    offsets.iter().enumerate().map(|(i, offset)| start + (*offset).min(slack) + gap * i as i64).collect()
}

/// Today's plan: the stored one if it is today's, otherwise a fresh one over
/// what is left of the day.
fn plan_for(now: i64, stored: Option<Plan>, mut roll: impl FnMut() -> f64) -> Plan {
    let day = super::points::ist_day(now);
    let last_drop = stored.as_ref().and_then(|plan| plan.last_drop);
    if let Some(plan) = stored.filter(|plan| plan.day == day) {
        return plan;
    }
    let midnight = ist_midnight(now);
    let (low, high) = drops_per_day();
    let want = (low + (roll().clamp(0.0, 1.0) * (high - low + 1) as f64) as usize).min(high);
    let times = spread((midnight + window_start()).max(now), midnight + window_end(), want, min_gap(), roll);
    Plan { day, times, done: 0, last_drop }
}

/// Whether the next drop should go now: its slot has come, it is inside the drop
/// hours, and the gap since the last one is kept.
fn due(plan: &Plan, now: i64) -> bool {
    let Some(next) = plan.times.get(plan.done) else {
        return false;
    };
    now >= *next && in_window(now) && plan.last_drop.is_none_or(|last| now - last >= min_gap())
}

/// Whether a frog may drop at `now` without crowding the Snitch: not within `gap`
/// after its last drop, nor within `gap` of its next planned one. A planned
/// Snitch long overdue (quiet chat) doesn't hold frogs up.
fn clear_of_snitch(now: i64, last: Option<i64>, next: Option<i64>, gap: i64) -> bool {
    last.is_none_or(|l| now - l >= gap) && next.is_none_or(|n| (n - now).abs() >= gap)
}

fn load_plan(conn: &rusqlite::Connection) -> Option<Plan> {
    let day = store::meta_get(conn, "plan_day")?;
    let times = store::meta_get(conn, "plan_times").unwrap_or_default().split(',').filter_map(|t| t.trim().parse().ok()).collect();
    let done = store::meta_get(conn, "plan_done").and_then(|d| d.parse().ok()).unwrap_or(0);
    let last_drop = store::meta_get(conn, "plan_last").and_then(|d| d.parse().ok());
    Some(Plan { day, times, done, last_drop })
}

fn save_plan(conn: &rusqlite::Connection, plan: &Plan) -> rusqlite::Result<()> {
    let times = plan.times.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(",");
    store::meta_set(conn, "plan_day", &plan.day)?;
    store::meta_set(conn, "plan_times", &times)?;
    store::meta_set(conn, "plan_done", &plan.done.to_string())?;
    store::meta_set(conn, "plan_last", &plan.last_drop.map(|t| t.to_string()).unwrap_or_default())
}

fn persist(plan: &Plan) {
    if let Some(db) = store::db() {
        if let Err(err) = save_plan(&db.lock(), plan) {
            tracing::warn!("frog: plan for {} not saved: {}", plan.day, err);
        }
    }
}

// --- words ------------------------------------------------------------------------

fn points_words(n: i64) -> String {
    format!("{} point{}", n, if n == 1 { "" } else { "s" })
}

fn tries_words(n: i64) -> String {
    format!("{} tr{} left", n, if n == 1 { "y" } else { "ies" })
}

fn minutes_words(secs: i64) -> String {
    match secs / 60 {
        1 => "1 min".to_string(),
        m => format!("{} min", m),
    }
}

/// "14 Sep", India time.
fn day_month(ts: i64) -> String {
    chrono::FixedOffset::east_opt(IST_OFFSET as i32)
        .and_then(|ist| ist.timestamp_opt(ts, 0).single())
        .map(|t| t.format("%-d %b").to_string())
        .unwrap_or_default()
}

/// Text that goes inside `**bold**` or a code span without breaking it.
fn plain(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '~' | '|' | '<' | '>' | '\\')).collect::<String>().trim().to_string()
}

fn attachment_name(slug: &str, ext: &str) -> String {
    format!("frog-{}.{}", slug, ext)
}

fn live_embed(d: &Drop, thumb: Option<&str>) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title("🐸 A Chocolate Frog hopped in!")
        .description(format!(
            "**{}** · {} {} · **{}**\n-# First correct answer keeps the card · {} tries · {}",
            d.wizard_name,
            d.rarity.emoji(),
            d.rarity.name(),
            points_words(d.points),
            MAX_TRIES,
            minutes_words(d.closes_at - d.dropped_at)
        ))
        .colour(d.rarity.colour());
    if let Some(file) = thumb {
        embed = embed.thumbnail(format!("attachment://{}", file));
    }
    embed
}

fn caught_embed(d: &Drop, house: Option<&House>, canonical: &str, thumb: Option<&str>) -> CreateEmbed {
    let winner = d.winner.map(|u| format!("<@{}>", u)).unwrap_or_else(|| "Someone".into());
    let house = house.map(|h| format!(" ({} {})", h.crest, h.name)).unwrap_or_default();
    let card = match (d.serial, d.edition) {
        (Some(serial), Some(edition)) => store::card_label(&d.wizard_name, edition, serial),
        (Some(serial), None) => store::serial_label(serial),
        _ => d.wizard_name.clone(),
    };
    let solved = d.solved_secs.map(|s| format!(" · solved in {} s", s)).unwrap_or_default();
    let mut embed = CreateEmbed::new()
        .title(format!("🎉 Caught! {}", d.wizard_name))
        .description(format!(
            "{}{} · **+{}** · **{}**\n-# Answer was **{}**{}",
            winner,
            house,
            points_words(d.points),
            card,
            plain(canonical),
            solved
        ))
        .colour(d.rarity.colour());
    if let Some(file) = thumb {
        embed = embed.thumbnail(format!("attachment://{}", file));
    }
    embed
}

fn escaped_embed(d: &Drop, canonical: &str, thumb: Option<&str>) -> CreateEmbed {
    let mut embed = CreateEmbed::new()
        .title("💨 The frog hopped away")
        .description(format!("**{}** · nobody solved it\n-# Answer: **{}**", d.wizard_name, plain(canonical)))
        .colour(ESCAPED_COLOUR);
    if let Some(file) = thumb {
        embed = embed.thumbnail(format!("attachment://{}", file));
    }
    embed
}

fn frog_emoji() -> ReactionType {
    ReactionType::Unicode("🐸".into())
}

fn catch_button(drop_id: i64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("frogcatch:{}", drop_id)).label("Catch it").emoji(frog_emoji()).style(ButtonStyle::Success),
    ])
}

fn done_button(drop_id: i64, label: String, with_frog: bool) -> CreateActionRow {
    let mut button = CreateButton::new(format!("frogdone:{}", drop_id)).label(label).style(ButtonStyle::Secondary).disabled(true);
    if with_frog {
        button = button.emoji(frog_emoji());
    }
    CreateActionRow::Buttons(vec![button])
}

/// A label fits Discord's 80 characters.
fn caught_label(name: &str) -> String {
    let label = format!("Caught by {}", name.trim());
    label.chars().take(80).collect()
}

/// "Catch <wizard>", cut to Discord's 45-character modal title.
fn modal_title(wizard: &str) -> String {
    let title = format!("Catch {}", wizard);
    if title.chars().count() <= 45 { title } else { title.chars().take(44).collect::<String>() + "…" }
}

/// The riddle pop-up, as the raw interaction response. The riddle is a text
/// block above the answer box. `legacy` puts it in a read-only-in-spirit
/// paragraph box instead, for when Discord refuses the text block.
fn modal_json(d: &Drop, riddle: &str, tries_left: i64, legacy: bool) -> Value {
    let hint = format!("{} · close spellings count", tries_words(tries_left));
    let answer = json!({
        "type": 4, "custom_id": "answer", "label": "Your answer", "style": 1,
        "min_length": 1, "max_length": 60, "required": true, "placeholder": hint,
    });
    let components = if legacy {
        json!([
            { "type": 1, "components": [{
                "type": 4, "custom_id": "riddle", "label": "Riddle (just read this)", "style": 2,
                "value": riddle.chars().take(4000).collect::<String>(), "required": false, "max_length": 4000,
            }]},
            { "type": 1, "components": [answer] },
        ])
    } else {
        json!([
            { "type": 10, "content": format!("**Riddle**\n{}\n-# {}", riddle, hint) },
            { "type": 1, "components": [answer] },
        ])
    };
    json!({ "type": 9, "data": { "custom_id": format!("frogans:{}", d.id), "title": modal_title(&d.wizard_name), "components": components } })
}

/// The typed answer from a submitted pop-up's `data`, wherever Discord put it:
/// inside an action row (`components`) or inside a label (`component`).
fn answer_from(data: &Value) -> Option<String> {
    fn walk(v: &Value) -> Option<String> {
        match v {
            Value::Object(map) => {
                if map.get("custom_id").and_then(Value::as_str) == Some("answer") {
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
    walk(data.get("components").unwrap_or(data))
}

fn wrong_text(left: i64) -> String {
    if left > 0 { format!("❌ Not quite. {}", tries_words(left)) } else { "❌ Not quite. No tries left on this frog.".to_string() }
}

fn too_late_text(winner: &str) -> String {
    if winner.trim().is_empty() {
        "Too late — someone already caught this frog".to_string()
    } else {
        format!("Too late — {} already caught this frog", plain(winner))
    }
}

const HOPPED: &str = "This frog hopped away";
const HOUSE_ONLY: &str = "Only house members can catch frogs";
const OUT_OF_TRIES: &str = "You've used your 3 tries on this frog";

/// What the winner is told. `granted` is what the ledger actually paid.
fn won_text(canonical: &str, d: &Drop, serial: i64, edition: i64, granted: Option<i64>) -> String {
    let points = match granted {
        Some(n) if n > 0 => format!("+{}", points_words(n)),
        Some(_) => "no points (today's frog limit is reached)".to_string(),
        None => "no points".to_string(),
    };
    format!("✅ It's **{}**! **{}** is yours · {}", plain(canonical), store::card_label(&d.wizard_name, edition, serial), points)
}

fn my_cards_button() -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new("frogmine").label("My cards").emoji(ReactionType::Unicode("📖".into())).style(ButtonStyle::Secondary),
    ])
}

// --- /frogs and /frogcard ------------------------------------------------------------

/// One page of someone's collection.
#[derive(Debug, PartialEq, Eq)]
struct CollectionPage {
    title: String,
    text: String,
    footer: String,
    page: usize,
    pages: usize,
}

/// The collection: the checklist of every card in play, then the copies owned,
/// newest first, [`CARD_LINES`] to a page.
#[allow(clippy::too_many_arguments)]
fn frogs_text(name: &str, cards: &[Card], wizards: &[Wizard], frog_points: i64, house: Option<&House>, dropped: i64, page: usize) -> CollectionPage {
    let title = format!("{}'s Chocolate Frog cards", name).chars().take(250).collect();
    let shown: Vec<&Wizard> = wizards.iter().filter(|w| w.enabled || cards.iter().any(|c| c.wizard_id == w.id)).collect();
    let owned = |w: &Wizard| cards.iter().filter(|c| c.wizard_id == w.id).count();
    let collected = shown.iter().filter(|w| w.enabled && owned(w) > 0).count();
    let enabled = shown.iter().filter(|w| w.enabled).count();
    let house = house.map(|h| format!(" · {} {}", h.crest, h.name)).unwrap_or_default();
    let full = enabled > 0 && collected == enabled;
    let badge = if full { " · Full set ready to sell — /sellset" } else { "" };
    let mut text = format!(
        "**{} of {} collected**{} · **{}** frog point{}{}\n",
        collected,
        enabled,
        badge,
        frog_points,
        if frog_points == 1 { "" } else { "s" },
        house
    );
    for rarity in Rarity::ALL {
        let row: Vec<String> = shown
            .iter()
            .filter(|w| w.rarity == rarity)
            .map(|w| match owned(w) {
                0 => format!("▫️ {}", w.name),
                n => format!("✅ {} ×{}", w.name, n),
            })
            .collect();
        if !row.is_empty() {
            text.push_str(&format!("{} {}\n", rarity.emoji(), row.join(" · ")));
        }
    }
    let pages = cards.len().div_ceil(CARD_LINES).max(1);
    let page = page.min(pages - 1);
    text.push_str(&format!("\n**{} card{}**", cards.len(), if cards.len() == 1 { "" } else { "s" }));
    if pages > 1 {
        text.push_str(&format!(" · page {} of {}", page + 1, pages));
    }
    text.push('\n');
    if cards.is_empty() {
        text.push_str("No cards yet. Press 🐸 **Catch it** when a frog hops in.\n");
    }
    for card in cards.iter().skip(page * CARD_LINES).take(CARD_LINES) {
        let mark = if card.traded_in() { " 🔁" } else if card.earned() { " 🏅" } else { "" };
        text.push_str(&format!("{} {} · {}{}\n", card.rarity.emoji(), store::card_label(&card.wizard_name, card.edition, card.serial), day_month(card.ts), mark));
    }
    if cards.iter().any(|c| c.traded_in() || c.earned()) {
        text.push_str("-# 🏅 earned by playing · 🔁 came by trade\n");
    }
    let footer = format!("{} frog{} dropped so far", dropped, if dropped == 1 { "" } else { "s" });
    CollectionPage { title, text: text.trim_end().to_string(), footer, page, pages }
}

/// The rarest card someone owns, newest first among equals: their collection's picture.
fn rarest(cards: &[Card]) -> Option<&Card> {
    let rank = |r: Rarity| Rarity::ALL.iter().position(|x| *x == r).unwrap_or(0);
    cards.iter().max_by(|a, b| rank(a.rarity).cmp(&rank(b.rarity)).then(a.serial.cmp(&b.serial)))
}

fn page_buttons(owner: u64, page: usize, pages: usize) -> Vec<CreateActionRow> {
    if pages <= 1 {
        return Vec::new();
    }
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("frogpage:{}:{}:prev", owner, page.saturating_sub(1)))
            .emoji(ReactionType::Unicode("◀️".into()))
            .style(ButtonStyle::Secondary)
            .disabled(page == 0),
        CreateButton::new(format!("frogpage:{}:{}:next", owner, (page + 1).min(pages - 1)))
            .emoji(ReactionType::Unicode("▶️".into()))
            .style(ButtonStyle::Secondary)
            .disabled(page + 1 >= pages),
    ])]
}

/// `frogpage:<owner>:<page>:<prev|next>`.
fn parse_page(custom_id: &str) -> Option<(u64, usize)> {
    let mut parts = custom_id.strip_prefix("frogpage:")?.split(':');
    let owner = parts.next()?.parse().ok()?;
    let page = parts.next()?.parse().ok()?;
    Some((owner, page))
}

pub fn command() -> CreateCommand {
    CreateCommand::new("frogs")
        .description("your Chocolate Frog cards - or someone else's")
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "whose cards to show (you if left out)"))
}

/// The `/frogdrop` command, to register.
pub fn drop_command_builder() -> CreateCommand {
    let mut kind = CreateCommandOption::new(CommandOptionType::String, "rarity", "which kind of frog (random by the usual odds if left out)");
    for r in Rarity::ALL {
        kind = kind.add_string_choice(r.name(), r.key());
    }
    CreateCommand::new("frogdrop").description("admin only: drop a Chocolate Frog right here, right now").add_option(kind)
}

/// `/frogdrop [rarity]` - admins only. Drops a frog in the channel it is run in,
/// straight away, whether or not scheduled drops are on. It is a real frog under
/// the same rules and doesn't use up a scheduled drop.
pub async fn drop_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if command.guild_id.is_none() {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.".into())).await;
        return;
    }
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can drop a frog.".into())).await;
        return;
    }
    let rarity = command.data.options.iter().find_map(|o| match (&o.name[..], &o.value) {
        ("rarity", CommandDataOptionValue::String(key)) => Rarity::from_key(key),
        _ => None,
    });
    let channel = command.channel_id.get();
    if store::db().is_some_and(|db| store::channel_busy(&db.lock(), channel)) {
        let text = "A frog is already hopping about here. Wait for it to be caught or escape.".to_string();
        let _ = command.create_response(&ctx.http, whisper(text)).await;
        return;
    }
    let text = match rarity {
        Some(r) => format!("Releasing {} {} frog.", r.emoji(), r.name()),
        None => "Releasing a frog.".to_string(),
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
    if let Err(err) = release(ctx, channel, Some(command.user.id.get()), rarity).await {
        let _ = command
            .create_followup(
                &ctx.http,
                serenity::all::CreateInteractionResponseFollowup::new().content(err).ephemeral(true),
            )
            .await;
    }
}

pub fn card_command() -> CreateCommand {
    CreateCommand::new("frogcard").description("look at one Chocolate Frog card by its number").add_option(
        CreateCommandOption::new(CommandOptionType::Integer, "number", "the card's No., like 42 for No. 0042").required(true).min_int_value(1),
    )
}

fn frog_points_of(user: u64) -> i64 {
    super::house::db()
        .and_then(|db| {
            db.lock()
                .query_row(
                    "SELECT COALESCE(SUM(points), 0) FROM ledger WHERE user_id = ?1 AND source = ?2",
                    rusqlite::params![user as i64, Source::Frog.key()],
                    |r| r.get::<_, i64>(0),
                )
                .ok()
        })
        .unwrap_or(0)
}

/// A member's name as the server shows it, from the cache or Discord.
async fn member_name(ctx: &Context, guild: Option<serenity::all::GuildId>, user: u64) -> String {
    let id = serenity::all::UserId::new(user);
    if let Some(guild) = guild {
        let cached = ctx.cache.guild(guild).and_then(|g| g.members.get(&id).map(|m| m.display_name().to_string()));
        if let Some(name) = cached {
            return name;
        }
        if let Ok(Ok(m)) = tokio::time::timeout(Duration::from_secs(5), guild.member(&ctx.http, id)).await {
            return m.display_name().to_string();
        }
    }
    match tokio::time::timeout(Duration::from_secs(5), id.to_user(ctx)).await {
        Ok(Ok(u)) => u.display_name().to_string(),
        _ => "That member".to_string(),
    }
}

/// Someone's collection as a private message, with the rarest card's picture.
async fn collection_message(owner: u64, name: &str, page: usize) -> CreateInteractionResponseMessage {
    let (cards, wizards, dropped) = match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::cards_of(&conn, owner), store::wizards(&conn), store::totals(&conn).dropped)
        }
        None => (Vec::new(), Vec::new(), 0),
    };
    let points = frog_points_of(owner);
    let house = super::house::house_of(owner);
    let view = frogs_text(&plain(name), &cards, &wizards, points, house, dropped, page);
    let top = rarest(&cards).cloned();
    let mut embed = CreateEmbed::new()
        .title(view.title)
        .description(view.text)
        .colour(top.as_ref().map(|c| c.rarity.colour()).unwrap_or(0x7A4A2A))
        .footer(CreateEmbedFooter::new(view.footer));
    let mut files = Vec::new();
    if let Some(card) = top {
        let found = store::db().and_then(|db| store::wizard(&db.lock(), card.wizard_id));
        if let Some(w) = found {
            let slug = w.slug.clone();
            if let Ok(Some((bytes, ext))) = tokio::task::spawn_blocking(move || store::thumbnail(&w.image, &w.slug)).await {
                let file = attachment_name(&slug, ext);
                embed = embed.thumbnail(format!("attachment://{}", file));
                files.push(CreateAttachment::bytes(bytes, file));
            }
        }
    }
    CreateInteractionResponseMessage::new()
        .embed(embed)
        .files(files)
        .components(page_buttons(owner, view.page, view.pages))
        .ephemeral(true)
        .allowed_mentions(CreateAllowedMentions::new())
}

pub async fn frogs_command(ctx: &Context, command: &CommandInteraction) {
    let picked = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id),
        _ => None,
    });
    let user = picked.unwrap_or(command.user.id);
    let name = if picked.is_some() {
        let nick = command.data.resolved.members.get(&user).and_then(|m| m.nick.clone());
        let global = command.data.resolved.users.get(&user).map(|u| u.display_name().to_string());
        nick.or(global).unwrap_or_else(|| "That member".to_string())
    } else {
        command.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| command.user.display_name().to_string())
    };
    let message = collection_message(user.get(), &name, 0).await;
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("frog: /frogs for {} not shown: {}", user, err);
    }
}

/// The text of one card: (title, description).
/// How a card came to exist, for /frogcard.
fn origin_words(card: &Card, drop: Option<&Drop>) -> String {
    let when = day_month(card.ts);
    let who = format!("<@{}>", card.original_owner);
    match card.origin.as_str() {
        "caught" => match drop {
            Some(d) => format!("Caught by {} on {} in <#{}>", who, when, d.channel),
            None => format!("Caught by {} on {}", who, when),
        },
        "royale_champion" => format!("Earned by {} on {} for winning a battle royale", who, when),
        "royale_runner_up" => format!("Earned by {} on {} as a battle royale runner-up", who, when),
        other => match other.strip_prefix("daily_top:") {
            Some(activity) => format!("Earned by {} on {} as the day's top in {}", who, when, super::frog_rewards::activity_label(activity)),
            None => format!("Earned by {} on {}", who, when),
        },
    }
}

/// The text of one card: (title, description).
fn card_text(card: &Card, drop: Option<&Drop>, canonical: &str, transfers: usize) -> (String, String) {
    let title = format!("{} {}", card.rarity.emoji(), store::card_label(&card.wizard_name, card.edition, card.serial));
    // Only a catch paid points; an earned card shows none.
    let points = drop.map(|d| format!(" · **{}**", points_words(d.points))).unwrap_or_default();
    let holder = if card.status == "spent" {
        format!("Sold in a full set by <@{}> on {}", card.user_id, day_month(card.spent_ts.unwrap_or(card.ts)))
    } else {
        format!("Owned by <@{}>", card.user_id)
    };
    let mut text = format!("{} **{}**{}\n{}\n{}", card.rarity.emoji(), card.rarity.name(), points, holder, origin_words(card, drop));
    if transfers > 0 {
        text.push_str(&format!(" · traded {}×", transfers));
    }
    if let (Some(d), false) = (drop, canonical.is_empty()) {
        text.push_str(&format!("\nWon with the answer **{}**", plain(canonical)));
        if let Some(secs) = d.solved_secs {
            text.push_str(&format!(" · solved in {} s", secs));
        }
    }
    (title, text)
}

pub async fn frogcard_command(ctx: &Context, command: &CommandInteraction) {
    let number = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::Integer(n) => Some(n),
        _ => None,
    });
    let found = number.and_then(|n| {
        let db = store::db()?;
        let conn = db.lock();
        let (card, drop) = store::card_by_serial(&conn, n)?;
        let canonical = drop.as_ref().and_then(|d| store::riddle(&conn, &d.riddle_id)).map(|r| r.canonical().to_string()).unwrap_or_default();
        let wizard = store::wizard(&conn, card.wizard_id);
        let moves = store::transfers_of(&conn, card.serial).len();
        Some((card, drop, canonical, wizard, moves))
    });
    let message = match found {
        None => CreateInteractionResponseMessage::new().content(format!("No card {} yet", store::serial_label(number.unwrap_or(0).max(0)))),
        Some((card, drop, canonical, wizard, moves)) => {
            let (title, text) = card_text(&card, drop.as_ref(), &canonical, moves);
            let mut embed = CreateEmbed::new().title(title).description(text).colour(card.rarity.colour());
            let mut message = CreateInteractionResponseMessage::new();
            if let Some(w) = wizard {
                let slug = w.slug.clone();
                if let Ok(Some((bytes, ext))) = tokio::task::spawn_blocking(move || store::thumbnail(&w.image, &w.slug)).await {
                    let file = attachment_name(&slug, ext);
                    embed = embed.image(format!("attachment://{}", file));
                    message = message.add_file(CreateAttachment::bytes(bytes, file));
                }
            }
            message.embed(embed)
        }
    };
    let message = message.ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("frog: /frogcard not shown: {}", err);
    }
}

// --- Discord -----------------------------------------------------------------------

async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, serenity::Error> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(result) => result,
        Err(_) => Err(serenity::Error::Other("no answer from Discord in time")),
    }
}

/// The riddle pop-up couldn't be shown in the text-block form, so the old form is used.
static LEGACY_MODAL: AtomicBool = AtomicBool::new(false);
static MODE_LOGGED: AtomicBool = AtomicBool::new(false);

/// Which pop-up this run uses: "text block" or "legacy".
pub fn modal_mode() -> &'static str {
    if LEGACY_MODAL.load(Ordering::Relaxed) { "legacy" } else { "text block" }
}

fn house_member(user: u64) -> bool {
    super::house::house_of(user).is_some() && !super::house::opted_out(user)
}

/// A wizard's picture ready to attach - shrunk, never the full-size original -
/// and its file name.
async fn picture(d: &Drop) -> Option<(CreateAttachment, String)> {
    let image = store::db().and_then(|db| store::wizard(&db.lock(), d.wizard_id)).map(|w| w.image).unwrap_or_default();
    let slug = d.slug.clone();
    let (bytes, ext) = tokio::task::spawn_blocking(move || store::thumbnail(&image, &slug)).await.ok()??;
    let file = attachment_name(&d.slug, ext);
    Some((CreateAttachment::bytes(bytes, file.clone()), file))
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

/// Posts a frog in a channel and opens it. The error says why not.
async fn release(ctx: &Context, channel: u64, by: Option<u64>, rarity: Option<Rarity>) -> Result<Drop, String> {
    if is_safe_corner(ctx, channel) {
        return Err("Frogs never drop in #safe-corner.".into());
    }
    let db = store::db().ok_or("The frog store isn't open.")?;
    let now = Utc::now().timestamp();
    let pending = {
        let conn = db.lock();
        let wizard = store::pick_wizard_of(&conn, rarity, rand::random::<f64>(), rand::random::<f64>())
            .ok_or("No card of that kind is switched on, so there's nothing to drop.")?;
        let riddle = store::pick_riddle(&conn, wizard.rarity.difficulty(), rand::random::<f64>())
            .ok_or("The riddle bank is empty: copy riddlebank/ into the workspace.")?;
        store::start_drop(&conn, channel, &wizard, &riddle, now, by).map_err(|e| e.to_string())?
    };
    let preview = Drop { closes_at: pending.dropped_at + open_secs(), ..pending.clone() };
    let pic = picture(&preview).await;
    let mut message = CreateMessage::new()
        .embed(live_embed(&preview, pic.as_ref().map(|p| p.1.as_str())))
        .components(vec![catch_button(pending.id)])
        .allowed_mentions(CreateAllowedMentions::new());
    if let Some((file, _)) = pic {
        message = message.add_file(file);
    }
    let posted = match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => posted,
        Err(err) => {
            let _ = store::discard_pending(&db.lock(), pending.id);
            tracing::warn!("frog: {} not dropped in {}: {}", pending.wizard_name, channel, err);
            return Err(format!("Discord wouldn't take the frog: {}", err));
        }
    };
    let opened = store::mark_open(&db.lock(), pending.id, posted.id.get(), Utc::now().timestamp(), open_secs());
    let drop = match opened {
        Ok(Some(d)) => d,
        other => {
            tracing::warn!("frog: drop {} posted but not opened: {:?}", pending.id, other.err());
            return Err("The frog was posted but couldn't be opened.".into());
        }
    };
    arm(ctx.clone(), drop.id, drop.closes_at);
    tracing::info!(
        "frog: {} {} dropped in {} (drop {}, riddle {}){}",
        drop.rarity.key(),
        drop.wizard_name,
        channel,
        drop.id,
        drop.riddle_id,
        by.map(|b| format!(", asked for by {}", b)).unwrap_or_default()
    );
    Ok(drop)
}

/// A drop asked for from the panel: straight away, in that channel, whether or
/// not scheduled drops are on. It doesn't use up a scheduled drop.
pub async fn drop_now(ctx: &Context, channel: u64, by: u64) -> Result<Drop, String> {
    let busy = store::db().is_some_and(|db| store::channel_busy(&db.lock(), channel));
    if busy {
        return Err("A frog is already hopping about in that channel. Wait for it to be caught or escape.".into());
    }
    release(ctx, channel, Some(by), None).await
}

/// Lets the frog escape when its time is up.
fn arm(ctx: Context, drop_id: i64, closes_at: i64) {
    tokio::spawn(async move {
        let wait = (closes_at - Utc::now().timestamp()).max(0) as u64;
        tokio::time::sleep(Duration::from_secs(wait)).await;
        let escaped = store::db().and_then(|db| store::escape(&db.lock(), drop_id).ok().flatten());
        if let Some(d) = escaped {
            tracing::info!("frog: {} (drop {}) hopped away uncaught", d.wizard_name, d.id);
            finish(&ctx, &d, false).await;
        }
    });
}

/// Edits a frog's card to how it ended. A failed edit is retried a couple of
/// times, then left for the next start, unless this already is that start.
async fn finish(ctx: &Context, d: &Drop, give_up_after: bool) {
    let Some(message) = d.message else {
        return;
    };
    let canonical = store::db().and_then(|db| store::riddle(&db.lock(), &d.riddle_id)).map(|r| r.canonical().to_string()).unwrap_or_default();
    let house = d.winner.and_then(super::house::house_of);
    let mut edited = false;
    for wait in EDIT_RETRIES {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        let pic = picture(d).await;
        let thumb = pic.as_ref().map(|p| p.1.clone());
        let (embed, row) = match d.status {
            Status::Caught => (caught_embed(d, house, &canonical, thumb.as_deref()), done_button(d.id, caught_label(&d.winner_name), true)),
            _ => (escaped_embed(d, &canonical, thumb.as_deref()), done_button(d.id, "Escaped".into(), false)),
        };
        let attachments = match pic {
            Some((file, _)) => EditAttachments::new().add(file),
            None => EditAttachments::new(),
        };
        let edit = EditMessage::new()
            .embed(embed)
            .components(vec![row])
            .attachments(attachments)
            .allowed_mentions(CreateAllowedMentions::new());
        match call(ChannelId::new(d.channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
            Ok(_) => {
                edited = true;
                break;
            }
            Err(err) => tracing::warn!("frog: card for drop {} not updated yet: {}", d.id, err),
        }
    }
    if edited || give_up_after {
        if let Some(db) = store::db() {
            let _ = store::mark_finished(&db.lock(), d.id);
        }
    }
}

async fn whisper_component(ctx: &Context, component: &ComponentInteraction, text: &str) {
    let message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

/// The game's buttons: catching, paging a collection, and "My cards".
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.as_str();
    if id == "frogmine" {
        let user = component.user.id.get();
        let name = component.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| component.user.display_name().to_string());
        let message = collection_message(user, &name, 0).await;
        if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
            tracing::warn!("frog: cards for {} not shown: {}", user, err);
        }
        return;
    }
    if let Some((owner, page)) = parse_page(id) {
        let name = if owner == component.user.id.get() {
            component.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| component.user.display_name().to_string())
        } else {
            member_name(ctx, component.guild_id, owner).await
        };
        let message = collection_message(owner, &name, page).await;
        if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await {
            tracing::warn!("frog: card page not turned: {}", err);
        }
        return;
    }
    catch_pressed(ctx, component).await;
}

/// "Catch it", on the card or under a wrong answer.
async fn catch_pressed(ctx: &Context, component: &ComponentInteraction) {
    let Some(drop_id) = component.data.custom_id.strip_prefix("frogcatch:").and_then(|id| id.parse::<i64>().ok()) else {
        return;
    };
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let found = store::db().and_then(|db| {
        let conn = db.lock();
        let d = store::get_drop(&conn, drop_id)?;
        let riddle = store::riddle(&conn, &d.riddle_id)?;
        let used = store::tries_used(&conn, drop_id, user);
        Some((d, riddle, used))
    });
    let Some((d, riddle, used)) = found else {
        whisper_component(ctx, component, HOPPED).await;
        return;
    };
    match d.status {
        Status::Caught => return whisper_component(ctx, component, &too_late_text(&d.winner_name)).await,
        Status::Open if now < d.closes_at => {}
        _ => return whisper_component(ctx, component, HOPPED).await,
    }
    if !house_member(user) {
        return whisper_component(ctx, component, HOUSE_ONLY).await;
    }
    if used >= MAX_TRIES {
        return whisper_component(ctx, component, OUT_OF_TRIES).await;
    }
    let left = MAX_TRIES - used;
    let legacy = LEGACY_MODAL.load(Ordering::Relaxed);
    let sent = ctx.http.create_interaction_response(component.id, &component.token, &modal_json(&d, &riddle.riddle, left, legacy), Vec::new()).await;
    match sent {
        Ok(()) => {
            if !MODE_LOGGED.swap(true, Ordering::Relaxed) {
                tracing::info!("frog: riddle pop-ups use the {} form", modal_mode());
            }
        }
        Err(serenity::Error::Http(err)) if !legacy && err.status_code().is_some_and(|s| s.as_u16() == 400) => {
            tracing::warn!("frog: Discord refused the text-block pop-up ({}), using the legacy form from now on", err);
            LEGACY_MODAL.store(true, Ordering::Relaxed);
            MODE_LOGGED.store(true, Ordering::Relaxed);
            let retry = ctx.http.create_interaction_response(component.id, &component.token, &modal_json(&d, &riddle.riddle, left, true), Vec::new()).await;
            if let Err(err) = retry {
                tracing::warn!("frog: legacy pop-up for drop {} failed too: {}", drop_id, err);
            }
        }
        Err(err) => tracing::warn!("frog: pop-up for drop {} not shown to {}: {}", drop_id, user, err),
    }
}

async fn reply_modal(ctx: &Context, modal: &ModalInteraction, text: String, row: Option<CreateActionRow>) {
    let mut message = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Some(row) = row {
        message = message.components(vec![row]);
    }
    if let Err(err) = modal.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("frog: answer reply to {} not sent: {}", modal.user.id, err);
    }
}

/// A submitted answer.
pub async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    let Some(drop_id) = modal.data.custom_id.strip_prefix("frogans:").and_then(|id| id.parse::<i64>().ok()) else {
        return;
    };
    let user = modal.user.id.get();
    let data = serde_json::to_value(&modal.data).unwrap_or(Value::Null);
    let guess = answer_from(&data).unwrap_or_default();
    if !house_member(user) {
        return reply_modal(ctx, modal, HOUSE_ONLY.into(), None).await;
    }
    let name = modal.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| modal.user.display_name().to_string());
    let Some(db) = store::db() else {
        return reply_modal(ctx, modal, HOPPED.into(), None).await;
    };
    let result = {
        let mut conn = db.lock();
        store::submit(&mut conn, drop_id, user, &name, &guess, Utc::now().timestamp())
    };
    let win = match result {
        Err(err) => {
            tracing::warn!("frog: answer from {} on drop {} not checked: {}", user, drop_id, err);
            return reply_modal(ctx, modal, "Something went wrong checking that. Press Catch it again.".into(), None).await;
        }
        Ok(Submit::Missing | Submit::Escaped) => return reply_modal(ctx, modal, HOPPED.into(), None).await,
        Ok(Submit::TooLate { winner_name }) => return reply_modal(ctx, modal, too_late_text(&winner_name), None).await,
        Ok(Submit::NoTries) => return reply_modal(ctx, modal, OUT_OF_TRIES.into(), None).await,
        Ok(Submit::Wrong { left }) => {
            let again = (left > 0).then(|| {
                CreateActionRow::Buttons(vec![
                    CreateButton::new(format!("frogcatch:{}", drop_id)).label("Try again").emoji(frog_emoji()).style(ButtonStyle::Success),
                ])
            });
            return reply_modal(ctx, modal, wrong_text(left), again).await;
        }
        Ok(Submit::Won(win)) => win,
    };
    let d = &win.drop;
    let reason = format!("Chocolate Frog: {}", d.wizard_name);
    let paid = super::house::award_person(user, Source::Frog, d.points, &reason, None, Some(format!("frog:{}", d.id)), None);
    let granted = paid.as_ref().map(|(_, outcome)| match outcome {
        Outcome::Granted(n) => *n,
        _ => 0,
    });
    let text = won_text(&win.canonical, d, win.serial, win.edition, granted);
    tracing::info!(
        "frog: {} caught {} (drop {}, card {}) in {}s, paid {:?}",
        user,
        d.wizard_name,
        d.id,
        win.serial,
        d.solved_secs.unwrap_or(0),
        granted
    );
    reply_modal(ctx, modal, text, Some(my_cards_button())).await;
    finish(ctx, d, false).await;
}

/// Closes frogs a restart left open and finishes cards it left unedited, then
/// runs the day's drops. Safe to call on every `ready`.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let leftovers = store::db().map(|db| {
        let conn = db.lock();
        let _ = store::discard_all_pending(&conn);
        for d in store::open_drops(&conn) {
            let _ = store::escape(&conn, d.id);
        }
        store::unfinished(&conn)
    });
    for d in leftovers.unwrap_or_default() {
        let ctx = ctx.clone();
        tokio::spawn(async move { finish(&ctx, &d, true).await });
    }
    match (switched_on(), channels()) {
        (true, Some(list)) => tracing::info!("frog: dropping into {:?}", list),
        (true, None) => tracing::info!("frog: switched on but no channels to drop into"),
        (false, _) => tracing::info!("frog: VIZIER_FROGS is off, no scheduled drops until it is switched on"),
    }
    // Earned cards each morning, and offers that run out of time.
    super::frog_rewards::spawn_daily(ctx.clone());
    super::frog_trade::spawn(ctx.clone());
    tokio::spawn(schedule(ctx));
}

async fn schedule(ctx: Context) {
    let mut plan: Option<Plan> = store::db().and_then(|db| load_plan(&db.lock()));
    let mut retry_at = 0i64;
    loop {
        tokio::time::sleep(TICK).await;
        let Some(channels) = channels().filter(|_| switched_on()) else {
            continue;
        };
        let now = Utc::now().timestamp();
        let before = plan.as_ref().map(|p| p.day.clone());
        let stored = plan.take();
        let current = plan.insert(plan_for(now, stored, rand::random::<f64>));
        if before.as_deref() != Some(current.day.as_str()) {
            tracing::info!("frog: {} drops planned for {}", current.times.len(), current.day);
            persist(current);
        }
        if now < retry_at || !due(current, now) {
            continue;
        }
        let (last, next) = super::snitch::drop_times();
        if !clear_of_snitch(now, last, next, snitch_gap()) {
            retry_at = now + 60;
            continue;
        }
        let busy: Vec<u64> = store::db().map(|db| store::open_drops(&db.lock()).iter().map(|d| d.channel).collect()).unwrap_or_default();
        let quiet = quiet_after();
        let awake = |id: u64| !busy.contains(&id) && super::snitch::last_spoke(id).is_some_and(|seen| now - seen <= quiet);
        let Some(channel) = super::snitch::choose_channel(&channels, rand::random::<f64>(), awake) else {
            retry_at = now + RETRY_AFTER;
            continue;
        };
        match release(&ctx, channel, None, None).await {
            Ok(d) => {
                current.done += 1;
                current.last_drop = Some(d.dropped_at);
                persist(current);
            }
            Err(_) => retry_at = now + RETRY_AFTER,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::house::HOUSES;
    use super::*;

    /// 2026-09-14 00:00 India time.
    const MIDNIGHT: i64 = 1_789_324_200;
    const HOUR: i64 = 3600;

    fn xorshift(seed: u64) -> impl FnMut() -> f64 {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn sample_drop(status: Status) -> Drop {
        Drop {
            id: 42,
            channel: 7,
            message: Some(900),
            wizard_id: 3,
            wizard_name: "Luna Lovegood".into(),
            slug: "luna-lovegood".into(),
            rarity: Rarity::Common,
            points: 1,
            riddle_id: "objects-001".into(),
            dropped_at: MIDNIGHT,
            closes_at: MIDNIGHT + 300,
            status,
            winner: Some(1234),
            winner_name: "Aarav".into(),
            answer_typed: "keybord".into(),
            solved_secs: Some(48),
            serial: Some(42),
            edition: Some(3),
            finished: false,
            by: None,
        }
    }

    #[test]
    fn a_day_gets_five_to_seven_drops_spaced_out() {
        let mut sizes = std::collections::HashMap::new();
        for seed in 1..400u64 {
            let plan = plan_for(MIDNIGHT + 60, None, xorshift(seed));
            let n = plan.times.len();
            assert!((5..=7).contains(&n), "seed {} planned {}", seed, n);
            *sizes.entry(n).or_insert(0) += 1;
            for t in &plan.times {
                assert!(in_window(*t));
                assert!(*t + 300 <= MIDNIGHT + DAY, "a frog must be over by midnight");
            }
            for pair in plan.times.windows(2) {
                assert!(pair[1] - pair[0] >= 90 * 60, "seed {}: {}s apart", seed, pair[1] - pair[0]);
            }
        }
        assert_eq!(sizes.len(), 3, "five, six and seven all come up: {:?}", sizes);
        // Late in the day there's only room for what fits.
        let late = plan_for(MIDNIGHT + 21 * HOUR, None, || 0.99);
        assert_eq!(late.times.len(), 2);
        assert!(late.times.iter().all(|t| *t >= MIDNIGHT + 21 * HOUR));
    }

    #[test]
    fn the_plan_keeps_the_gap_and_survives_the_day() {
        let mut plan = Plan { day: "2026-09-14".into(), times: vec![MIDNIGHT + 10 * HOUR, MIDNIGHT + 11 * HOUR], done: 0, last_drop: None };
        assert!(!due(&plan, MIDNIGHT + 9 * HOUR));
        assert!(due(&plan, MIDNIGHT + 10 * HOUR));
        plan.done = 1;
        plan.last_drop = Some(MIDNIGHT + 10 * HOUR + 20 * 60);
        assert!(!due(&plan, MIDNIGHT + 11 * HOUR + 30 * 60), "only 70 minutes after the last");
        assert!(due(&plan, MIDNIGHT + 11 * HOUR + 50 * 60));
        assert_eq!(plan_for(MIDNIGHT + 12 * HOUR, Some(plan.clone()), || 0.5), plan);
        let tomorrow = plan_for(MIDNIGHT + DAY + 60, Some(plan), || 0.5);
        assert_eq!((tomorrow.day.as_str(), tomorrow.done, tomorrow.last_drop), ("2026-09-15", 0, Some(MIDNIGHT + 10 * HOUR + 20 * 60)));

        let conn = store::tests::memory();
        assert_eq!(load_plan(&conn), None);
        save_plan(&conn, &tomorrow).unwrap();
        assert_eq!(load_plan(&conn), Some(tomorrow));
    }

    #[test]
    fn frogs_keep_clear_of_the_snitch() {
        let gap = 20 * 60;
        let now = MIDNIGHT + 12 * HOUR;
        assert!(clear_of_snitch(now, None, None, gap));
        assert!(!clear_of_snitch(now, Some(now - 5 * 60), None, gap), "a Snitch just dropped");
        assert!(clear_of_snitch(now, Some(now - 20 * 60), None, gap));
        assert!(!clear_of_snitch(now, None, Some(now + 10 * 60), gap), "one is about to drop");
        assert!(clear_of_snitch(now, None, Some(now + 25 * 60), gap));
        assert!(!clear_of_snitch(now, None, Some(now - 5 * 60), gap), "one is due right now");
        assert!(clear_of_snitch(now, None, Some(now - 3 * HOUR), gap), "a long-overdue Snitch doesn't block frogs");
        assert!(clear_of_snitch(now, Some(now - 60), None, 0), "a gap of 0 switches the rule off");
    }

    #[test]
    fn the_drop_card_reads_as_agreed() {
        let d = sample_drop(Status::Open);
        let live = serde_json::to_value(live_embed(&d, Some("frog-luna-lovegood.webp"))).unwrap();
        assert_eq!(live["title"], "🐸 A Chocolate Frog hopped in!");
        assert_eq!(live["description"], "**Luna Lovegood** · 🥛 Common · **1 point**\n-# First correct answer keeps the card · 3 tries · 5 min");
        assert_eq!(live["color"], 0xC68E54);
        assert_eq!(live["thumbnail"]["url"], "attachment://frog-luna-lovegood.webp");
        let rare = Drop { rarity: Rarity::Legendary, points: 10, wizard_name: "The Eternal Phoenix".into(), ..d.clone() };
        let bare = serde_json::to_value(live_embed(&rare, None)).unwrap();
        assert!(bare["description"].as_str().unwrap().starts_with("**The Eternal Phoenix** · 🔥 Legendary · **10 points**"));
        assert!(bare.get("thumbnail").is_none(), "no picture, no thumbnail");
        let button = serde_json::to_value(catch_button(42)).unwrap();
        assert_eq!(button["components"][0]["custom_id"], "frogcatch:42");
        assert_eq!(button["components"][0]["label"], "Catch it");
        assert_eq!(button["components"][0]["style"], 3);
    }

    #[test]
    fn the_caught_and_escaped_cards_read_as_agreed() {
        let d = sample_drop(Status::Caught);
        let caught = serde_json::to_value(caught_embed(&d, Some(&HOUSES[2]), "keyboard", Some("frog-luna-lovegood.webp"))).unwrap();
        assert_eq!(caught["title"], "🎉 Caught! Luna Lovegood");
        assert_eq!(caught["description"], format!("<@1234> ({} {}) · **+1 point** · **Luna Lovegood #3 · No. 0042**\n-# Answer was **keyboard** · solved in 48 s", HOUSES[2].crest, HOUSES[2].name));
        assert_eq!(caught["thumbnail"]["url"], "attachment://frog-luna-lovegood.webp");
        let row = serde_json::to_value(done_button(42, caught_label("Aarav"), true)).unwrap();
        assert_eq!((row["components"][0]["label"].as_str(), row["components"][0]["disabled"].as_bool()), (Some("Caught by Aarav"), Some(true)));

        let escaped = serde_json::to_value(escaped_embed(&sample_drop(Status::Escaped), "keyboard", None)).unwrap();
        assert_eq!(escaped["title"], "💨 The frog hopped away");
        assert_eq!(escaped["description"], "**Luna Lovegood** · nobody solved it\n-# Answer: **keyboard**");
        assert_eq!(escaped["color"], 0x4E5058);
        let text = escaped.to_string();
        assert!(!text.contains("Riddle") && !text.contains("I have keys"), "the riddle is never shown publicly");
        let row = serde_json::to_value(done_button(42, "Escaped".into(), false)).unwrap();
        assert_eq!((row["components"][0]["label"].as_str(), row["components"][0]["disabled"].as_bool()), (Some("Escaped"), Some(true)));
        // Markdown in an answer can't break the card.
        assert!(serde_json::to_value(escaped_embed(&d, "**bold** _x_ `y`", None)).unwrap()["description"].as_str().unwrap().ends_with("**bold x y**"));
    }

    #[test]
    fn the_private_replies_read_as_agreed() {
        assert_eq!(wrong_text(2), "❌ Not quite. 2 tries left");
        assert_eq!(wrong_text(1), "❌ Not quite. 1 try left");
        assert_eq!(wrong_text(0), "❌ Not quite. No tries left on this frog.");
        assert_eq!(too_late_text("Aarav"), "Too late — Aarav already caught this frog");
        assert_eq!(too_late_text(""), "Too late — someone already caught this frog");
        let d = sample_drop(Status::Caught);
        assert_eq!(won_text("keyboard", &d, 42, 3, Some(2)), "✅ It's **keyboard**! **Luna Lovegood #3 · No. 0042** is yours · +2 points");
        assert!(won_text("keyboard", &d, 42, 3, Some(0)).ends_with("no points (today's frog limit is reached)"));
        assert!(won_text("keyboard", &d, 7, 1, None).ends_with("**Luna Lovegood #1 · No. 0007** is yours · no points"));
        let button = serde_json::to_value(my_cards_button()).unwrap();
        assert_eq!((button["components"][0]["custom_id"].as_str(), button["components"][0]["label"].as_str(), button["components"][0]["style"].as_i64()), (Some("frogmine"), Some("My cards"), Some(2)));
    }

    #[test]
    fn the_riddle_pop_up_is_built_in_both_forms() {
        let d = sample_drop(Status::Open);
        let riddle = "I have keys but open no locks. What am I?";
        let new = modal_json(&d, riddle, 3, false);
        assert_eq!(new["type"], 9);
        assert_eq!(new["data"]["custom_id"], "frogans:42");
        assert_eq!(new["data"]["title"], "Catch Luna Lovegood");
        let parts = new["data"]["components"].as_array().unwrap();
        assert_eq!(parts[0]["type"], 10);
        assert_eq!(parts[0]["content"], format!("**Riddle**\n{}\n-# 3 tries left · close spellings count", riddle));
        assert_eq!(parts[1]["type"], 1);
        let answer = &parts[1]["components"][0];
        assert_eq!((answer["custom_id"].as_str(), answer["label"].as_str(), answer["style"].as_i64()), (Some("answer"), Some("Your answer"), Some(1)));
        assert_eq!(answer["placeholder"], "3 tries left · close spellings count");
        assert_eq!(answer["max_length"], 60);

        let old = modal_json(&d, riddle, 1, true);
        let rows = old["data"]["components"].as_array().unwrap();
        assert!(rows.iter().all(|r| r["type"] == 1), "legacy pop-ups are only action rows");
        assert_eq!(rows[0]["components"][0]["label"], "Riddle (just read this)");
        assert_eq!(rows[0]["components"][0]["value"], riddle);
        assert_eq!(rows[0]["components"][0]["style"], 2);
        assert_eq!(rows[1]["components"][0]["placeholder"], "1 try left · close spellings count");

        let long = Drop { wizard_name: "A Very Long Wizard Name That Goes On".into(), ..d };
        assert!(modal_title(&long.wizard_name).chars().count() <= 45);
        assert_eq!(modal_title("Merlin"), "Catch Merlin");
    }

    #[test]
    fn the_answer_is_read_from_either_submit_shape() {
        // Legacy action rows, as Discord sends them back.
        let legacy = json!({
            "custom_id": "frogans:42",
            "components": [
                { "type": 1, "id": 1, "components": [{ "type": 4, "id": 2, "custom_id": "riddle", "value": "I have keys..." }] },
                { "type": 1, "id": 3, "components": [{ "type": 4, "id": 4, "custom_id": "answer", "value": "keyboard" }] }
            ]
        });
        assert_eq!(answer_from(&legacy).as_deref(), Some("keyboard"));
        // A text block, then the answer box inside a label.
        let labelled = json!({
            "custom_id": "frogans:42",
            "components": [
                { "type": 10, "id": 1 },
                { "type": 18, "id": 2, "component": { "type": 4, "id": 3, "custom_id": "answer", "value": " The Keyboard " } }
            ]
        });
        assert_eq!(answer_from(&labelled).as_deref(), Some(" The Keyboard "));
        // A text block and an action row, the form this bot sends.
        let mixed = json!({ "custom_id": "frogans:1", "components": [{ "type": 10, "id": 1 }, { "type": 1, "components": [{ "type": 4, "custom_id": "answer", "value": "echo" }] }] });
        assert_eq!(answer_from(&mixed).as_deref(), Some("echo"));
        // What serenity makes of the mixed form round-trips through its own types.
        let parsed: serenity::all::ModalInteractionData = serde_json::from_value(mixed.clone()).expect("serenity reads the mixed form");
        assert_eq!(answer_from(&serde_json::to_value(&parsed).unwrap()).as_deref(), Some("echo"));
        // Nothing to find.
        assert_eq!(answer_from(&json!({ "components": [{ "type": 10, "id": 1 }] })), None);
        assert_eq!(answer_from(&json!({ "components": [{ "type": 1, "components": [{ "type": 4, "custom_id": "riddle", "value": "x" }] }] })), None);
        assert_eq!(answer_from(&Value::Null), None);
    }

    #[test]
    fn the_collection_reads_newest_first_with_every_card_a_page_at_a_time() {
        let conn = store::tests::memory();
        let wizards = store::wizards(&conn);
        let card = |serial: i64, wizard_id: i64, edition: i64| {
            let w = wizards.iter().find(|w| w.id == wizard_id).unwrap();
            Card { serial, edition, user_id: 1, wizard_id, wizard_name: w.name.clone(), rarity: w.rarity, drop_id: Some(serial), ts: MIDNIGHT + 12 * HOUR, origin: "caught".into(), original_owner: 1, status: "owned".into(), spent_reason: String::new(), spent_ts: None }
        };
        let mut cards: Vec<Card> = (1..=20).map(|s| card(s, if s % 2 == 0 { 3 } else { 1 }, (s + 1) / 2)).collect();
        cards.push(card(187, 10, 3));
        cards.reverse();
        let view = frogs_text("Aarav", &cards, &wizards, 25, Some(&HOUSES[2]), 48, 0);
        assert_eq!(view.title, "Aarav's Chocolate Frog cards");
        let text = &view.text;
        assert!(text.starts_with(&format!("**3 of 10 collected** · **25** frog points · {} {}", HOUSES[2].crest, HOUSES[2].name)), "{text}");
        assert!(text.contains("🥛 ✅ Rubeus Hagrid ×10 · ▫️ Luna Lovegood · ✅ Hermione Granger ×10 · ▫️ Sirius Black · ▫️ The Bloomweaver"), "{text}");
        assert!(text.contains("🍫 ▫️ Nicolas Flamel · ▫️ Merlin · ▫️ The Moonkeeper · ▫️ The Original Chocolate Frog"), "{text}");
        assert!(text.contains("🔥 ✅ The Eternal Phoenix ×1"), "{text}");
        assert!(text.contains("**21 cards** · page 1 of 2"), "{text}");
        let lines: Vec<&str> = text.lines().filter(|l| l.contains(" · No. ")).collect();
        assert_eq!(lines.len(), 15);
        assert_eq!(lines[0], "🔥 The Eternal Phoenix #3 · No. 0187 · 14 Sep");
        assert_eq!((view.page, view.pages, view.footer.as_str()), (0, 2, "48 frogs dropped so far"));
        let last = frogs_text("Aarav", &cards, &wizards, 25, None, 48, 9);
        assert_eq!(last.page, 1, "a page past the end shows the last");
        assert_eq!(last.text.lines().filter(|l| l.contains(" · No. ")).count(), 6);
        assert!(last.text.ends_with("🥛 Rubeus Hagrid #1 · No. 0001 · 14 Sep"), "{}", last.text);
        assert_eq!(rarest(&cards).map(|c| c.serial), Some(187));
        assert_eq!(rarest(&cards[1..]).map(|c| c.serial), Some(20), "newest of the commons");

        let empty = frogs_text("Zoya", &[], &wizards, 0, None, 1, 0);
        assert!(empty.text.starts_with("**0 of 10 collected** · **0** frog points\n"), "{}", empty.text);
        assert!(empty.text.contains("No cards yet"));
        assert_eq!((empty.pages, empty.footer.as_str()), (1, "1 frog dropped so far"));
        assert!(rarest(&[]).is_none());
    }

    #[test]
    fn collection_pages_turn_with_buttons() {
        assert!(page_buttons(5, 0, 1).is_empty());
        let rows = serde_json::to_value(page_buttons(5, 0, 3)).unwrap();
        let buttons = &rows[0]["components"];
        assert_eq!((buttons[0]["custom_id"].as_str(), buttons[0]["disabled"].as_bool()), (Some("frogpage:5:0:prev"), Some(true)));
        assert_eq!((buttons[1]["custom_id"].as_str(), buttons[1]["disabled"].as_bool()), (Some("frogpage:5:1:next"), Some(false)));
        let rows = serde_json::to_value(page_buttons(5, 2, 3)).unwrap();
        assert_eq!(rows[0]["components"][1]["disabled"], true);
        assert_eq!(parse_page("frogpage:1234:2:next"), Some((1234, 2)));
        assert_eq!(parse_page("frogpage:x:2:next"), None);
        assert_eq!(parse_page("frogcatch:3"), None);
    }

    #[test]
    fn one_card_reads_with_its_owner_and_answer() {
        let d = sample_drop(Status::Caught);
        let card = Card { serial: 187, edition: 3, user_id: 1234, wizard_id: 10, wizard_name: "The Eternal Phoenix".into(), rarity: Rarity::Legendary, drop_id: Some(42), ts: MIDNIGHT + 12 * HOUR, origin: "caught".into(), original_owner: 99, status: "owned".into(), spent_reason: String::new(), spent_ts: None };
        let drop = Drop { points: 10, rarity: Rarity::Legendary, ..d };
        let (title, text) = card_text(&card, Some(&drop), "phoenix", 2);
        assert_eq!(title, "🔥 The Eternal Phoenix #3 · No. 0187");
        assert_eq!(text, "🔥 **Legendary** · **10 points**\nOwned by <@1234>\nCaught by <@99> on 14 Sep in <#7> · traded 2×\nWon with the answer **phoenix** · solved in 48 s");
        let earned = Card { rarity: Rarity::Uncommon, wizard_name: "Merlin".into(), drop_id: None, origin: "daily_top:snitch".into(), original_owner: 1234, ..card.clone() };
        let (_, text) = card_text(&earned, None, "", 0);
        assert_eq!(text, "🍫 **Uncommon**\nOwned by <@1234>\nEarned by <@1234> on 14 Sep as the day's top in 🪽 Snitch");
        let royale = Card { origin: "royale_runner_up".into(), ..earned };
        assert!(card_text(&royale, None, "", 1).1.ends_with("as a battle royale runner-up · traded 1×"));
        let sold = Card { status: "spent".into(), spent_reason: "sold full set #3".into(), spent_ts: Some(MIDNIGHT + 30 * HOUR), ..royale };
        assert!(card_text(&sold, None, "", 1).1.contains("\nSold in a full set by <@1234> on 15 Sep\n"), "{}", card_text(&sold, None, "", 1).1);
    }

    #[test]
    fn a_disabled_card_only_shows_for_someone_who_owns_one() {
        let conn = store::tests::memory();
        conn.execute("UPDATE wizards SET enabled = 0 WHERE id = 2", []).unwrap();
        let wizards = store::wizards(&conn);
        let none = frogs_text("A", &[], &wizards, 0, None, 0, 0).text;
        assert!(!none.contains("Luna") && none.contains("0 of 9 collected"), "{none}");
        let owned = Card { serial: 1, edition: 1, user_id: 1, wizard_id: 2, wizard_name: "Luna Lovegood".into(), rarity: Rarity::Common, drop_id: None, ts: MIDNIGHT, origin: "royale_champion".into(), original_owner: 5, status: "owned".into(), spent_reason: String::new(), spent_ts: None };
        let some = frogs_text("A", &[owned.clone()], &wizards, 1, None, 1, 0).text;
        assert!(some.contains("✅ Luna Lovegood ×1") && some.contains("0 of 9 collected"), "{some}");
        assert!(some.contains("🥛 Luna Lovegood #1 · No. 0001 · 14 Sep 🔁") && some.ends_with("-# 🏅 earned by playing · 🔁 came by trade"), "{some}");
        // A full set of any origin is ready to sell.
        let all: Vec<Card> = wizards.iter().filter(|w| w.enabled).enumerate().map(|(i, w)| Card { serial: i as i64 + 1, wizard_id: w.id, wizard_name: w.name.clone(), rarity: w.rarity, ..owned.clone() }).collect();
        assert!(frogs_text("A", &all, &wizards, 1, None, 1, 0).text.starts_with("**9 of 9 collected** · Full set ready to sell — /sellset · **1** frog point"));
        assert!(frogs_text("A", &[], &wizards, 1, None, 1, 0).text.starts_with("**0 of 9 collected** · **1** frog point"));
    }

    #[test]
    fn small_words() {
        assert_eq!(points_words(1), "1 point");
        assert_eq!(points_words(4), "4 points");
        assert_eq!(tries_words(3), "3 tries left");
        assert_eq!(minutes_words(300), "5 min");
        assert_eq!(day_month(MIDNIGHT + 12 * HOUR), "14 Sep");
        assert_eq!(attachment_name("merlin", "png"), "frog-merlin.png");
        assert_eq!(caught_label(&"x".repeat(100)).chars().count(), 80);
    }
}
