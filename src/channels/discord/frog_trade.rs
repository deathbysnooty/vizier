//! Trading Chocolate Frog cards between members.
//!
//! `/trade member:` opens a private builder: pick cards to give and cards to ask
//! for, then send. The offer goes up in the channel for the other member to
//! accept or decline, and the sender can cancel it. Nothing is reserved while an
//! offer waits - a card can sit in several - so accepting checks, in one
//! transaction, that every card is still with the person it should be, moves
//! them all or none, and writes each move down. Trades never move house points.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction,
    ComponentInteractionDataKind, Context, CreateActionRow, CreateAllowedMentions, CreateButton, CreateCommand,
    CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, EditMessage, MessageId, UserId,
};

use super::control;
use super::frog_store::{self as store, Card, Rarity};

/// Most cards on one side of a trade.
pub const MAX_PER_SIDE: usize = 10;
/// Cards one select menu page lists: Discord's limit.
pub const PAGE: usize = 25;
/// How long a builder stays usable.
const DRAFT_SECS: i64 = 15 * 60;
const TRADE_COLOUR: u32 = 0x5865F2;
const DONE_COLOUR: u32 = 0x3BA55C;
const CLOSED_COLOUR: u32 = 0x4E5058;

fn trades_on() -> bool {
    control::on("VIZIER_FROGS", false) && control::on("VIZIER_TRADES", true)
}

fn expiry_secs() -> i64 {
    control::number("VIZIER_TRADE_EXPIRY_HOURS", 24).clamp(1, 24 * 7) as i64 * 3600
}

// --- the store -------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Trade {
    pub id: i64,
    pub from_user: u64,
    pub to_user: u64,
    pub from_name: String,
    pub to_name: String,
    pub guild_id: Option<u64>,
    pub channel_id: u64,
    pub message_id: Option<u64>,
    /// open, done, declined, cancelled, expired or failed.
    pub status: String,
    pub created_ts: i64,
    pub expires_ts: i64,
    pub closed_ts: Option<i64>,
    pub closed_by: Option<u64>,
    /// What the sender gives, by card number.
    pub give: Vec<i64>,
    /// What the sender asks for.
    pub ask: Vec<i64>,
}

const TRADE_COLUMNS: &str = "t.id, t.from_user, t.to_user, COALESCE(n.from_name, ''), COALESCE(n.to_name, ''), t.guild_id, t.channel_id, \
     t.message_id, t.status, t.created_ts, t.expires_ts, t.closed_ts, t.closed_by";

fn trade_row(conn: &Connection, r: &rusqlite::Row) -> rusqlite::Result<Trade> {
    let id: i64 = r.get(0)?;
    let items = |side: &str| -> Vec<i64> {
        conn.prepare("SELECT serial FROM trade_items WHERE trade_id = ?1 AND side = ?2 ORDER BY serial")
            .and_then(|mut s| s.query_map(params![id, side], |r| r.get(0))?.collect())
            .unwrap_or_default()
    };
    Ok(Trade {
        id,
        from_user: r.get::<_, i64>(1)? as u64,
        to_user: r.get::<_, i64>(2)? as u64,
        from_name: r.get(3)?,
        to_name: r.get(4)?,
        guild_id: r.get::<_, Option<i64>>(5)?.map(|g| g as u64),
        channel_id: r.get::<_, i64>(6)? as u64,
        message_id: r.get::<_, Option<i64>>(7)?.map(|m| m as u64),
        status: r.get(8)?,
        created_ts: r.get(9)?,
        expires_ts: r.get(10)?,
        closed_ts: r.get(11)?,
        closed_by: r.get::<_, Option<i64>>(12)?.map(|u| u as u64),
        give: items("give"),
        ask: items("ask"),
    })
}

fn trades_where(conn: &Connection, cond: &str, args: &[&dyn rusqlite::ToSql], limit: i64) -> Vec<Trade> {
    let sql = format!(
        "SELECT {} FROM trades t LEFT JOIN trade_names n ON n.trade_id = t.id WHERE {} ORDER BY t.id DESC LIMIT {}",
        TRADE_COLUMNS, cond, limit
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let ids: Vec<i64> = stmt.query_map(args, |r| r.get(0)).map(|rows| rows.flatten().collect()).unwrap_or_default();
    ids.into_iter().filter_map(|id| get(conn, id)).collect()
}

pub fn get(conn: &Connection, id: i64) -> Option<Trade> {
    let sql = format!("SELECT {} FROM trades t LEFT JOIN trade_names n ON n.trade_id = t.id WHERE t.id = ?1", TRADE_COLUMNS);
    conn.query_row(&sql, params![id], |r| trade_row(conn, r)).optional().ok().flatten()
}

/// A new open offer. The error says what's wrong with it.
#[allow(clippy::too_many_arguments)]
pub fn create(
    conn: &mut Connection,
    from: (u64, &str),
    to: (u64, &str),
    guild: Option<u64>,
    channel: u64,
    give: &[i64],
    ask: &[i64],
    now: i64,
    expiry: i64,
) -> Result<Trade, String> {
    if from.0 == to.0 {
        return Err("You can't trade with yourself.".into());
    }
    if give.is_empty() && ask.is_empty() {
        return Err("Pick at least one card to give or ask for.".into());
    }
    if give.len() > MAX_PER_SIDE || ask.len() > MAX_PER_SIDE {
        return Err(format!("At most {} cards on each side.", MAX_PER_SIDE));
    }
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute(
        "INSERT INTO trades (from_user, to_user, guild_id, channel_id, status, created_ts, expires_ts) VALUES (?1, ?2, ?3, ?4, 'open', ?5, ?6)",
        params![from.0 as i64, to.0 as i64, guild.map(|g| g as i64), channel as i64, now, now + expiry],
    )
    .map_err(|e| e.to_string())?;
    let id = tx.last_insert_rowid();
    tx.execute("INSERT INTO trade_names (trade_id, from_name, to_name) VALUES (?1, ?2, ?3)", params![id, from.1, to.1]).map_err(|e| e.to_string())?;
    for (side, list) in [("give", give), ("ask", ask)] {
        for serial in list {
            tx.execute("INSERT OR IGNORE INTO trade_items (trade_id, serial, side) VALUES (?1, ?2, ?3)", params![id, serial, side])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    get(conn, id).ok_or_else(|| "The offer vanished after saving.".into())
}

pub fn set_message(conn: &Connection, id: i64, message: u64) -> rusqlite::Result<()> {
    conn.execute("UPDATE trades SET message_id = ?2 WHERE id = ?1", params![id, message as i64]).map(|_| ())
}

/// How accepting went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accepted {
    Done(Trade),
    /// Already closed: nothing happens.
    NotOpen(Trade),
    Expired(Trade),
    /// Some card isn't where it should be; the offer is closed and nothing moved.
    /// Carries the card numbers that had moved.
    Failed(Trade, Vec<i64>),
    Missing,
}

/// Accepts an open offer: all the cards move in one transaction, or none do.
pub fn accept(conn: &mut Connection, id: i64, now: i64) -> rusqlite::Result<Accepted> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(trade) = get(&tx, id) else {
        return Ok(Accepted::Missing);
    };
    if trade.status != "open" {
        return Ok(Accepted::NotOpen(trade));
    }
    if now >= trade.expires_ts {
        tx.execute("UPDATE trades SET status = 'expired', closed_ts = ?2 WHERE id = ?1", params![id, now])?;
        let closed = get(&tx, id).unwrap_or(trade);
        tx.commit()?;
        return Ok(Accepted::Expired(closed));
    }
    let owner_of = |serial: i64| -> Option<u64> {
        tx.query_row("SELECT user_id FROM cards WHERE serial = ?1 AND status = 'owned'", params![serial], |r| r.get::<_, i64>(0))
            .optional()
            .ok()
            .flatten()
            .map(|u| u as u64)
    };
    let moved: Vec<i64> = trade
        .give
        .iter()
        .filter(|s| owner_of(**s) != Some(trade.from_user))
        .chain(trade.ask.iter().filter(|s| owner_of(**s) != Some(trade.to_user)))
        .copied()
        .collect();
    if !moved.is_empty() {
        tx.execute("UPDATE trades SET status = 'failed', closed_ts = ?2, closed_by = ?3 WHERE id = ?1", params![id, now, trade.to_user as i64])?;
        let closed = get(&tx, id).unwrap_or(trade);
        tx.commit()?;
        return Ok(Accepted::Failed(closed, moved));
    }
    for (list, from, to) in [(&trade.give, trade.from_user, trade.to_user), (&trade.ask, trade.to_user, trade.from_user)] {
        for serial in list {
            let changed = tx.execute(
                "UPDATE cards SET user_id = ?2 WHERE serial = ?1 AND user_id = ?3 AND status = 'owned'",
                params![serial, to as i64, from as i64],
            )?;
            if changed != 1 {
                // Checked above under the same lock; kept so a surprise rolls it all back.
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            tx.execute(
                "INSERT INTO transfers (serial, from_user, to_user, trade_id, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![serial, from as i64, to as i64, id, now],
            )?;
        }
    }
    tx.execute("UPDATE trades SET status = 'done', closed_ts = ?2, closed_by = ?3 WHERE id = ?1", params![id, now, trade.to_user as i64])?;
    let done = get(&tx, id).unwrap_or(trade);
    tx.commit()?;
    Ok(Accepted::Done(done))
}

/// Closes an open offer as declined, cancelled or expired. `Some` only when this
/// call closed it.
pub fn close(conn: &Connection, id: i64, status: &str, by: Option<u64>, now: i64) -> rusqlite::Result<Option<Trade>> {
    let changed = conn.execute(
        "UPDATE trades SET status = ?2, closed_ts = ?3, closed_by = ?4 WHERE id = ?1 AND status = 'open'",
        params![id, status, now, by.map(|b| b as i64)],
    )?;
    Ok(if changed == 1 { get(conn, id) } else { None })
}

/// Open offers whose time is up.
pub fn overdue(conn: &Connection, now: i64) -> Vec<Trade> {
    trades_where(conn, "t.status = 'open' AND t.expires_ts <= ?1", &[&now], 500)
}

/// Closed offers whose message doesn't show it yet.
pub fn unfinished(conn: &Connection) -> Vec<Trade> {
    trades_where(conn, "t.status != 'open' AND t.finished = 0 AND t.message_id IS NOT NULL", &[], 100)
}

pub fn mark_finished(conn: &Connection, id: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE trades SET finished = 1 WHERE id = ?1", params![id]).map(|_| ())
}

/// The newest trades, optionally one member's, optionally one status.
pub fn list(conn: &Connection, member: Option<u64>, status: Option<&str>, limit: i64) -> Vec<Trade> {
    match (member, status) {
        (Some(m), Some(st)) => trades_where(conn, "(t.from_user = ?1 OR t.to_user = ?1) AND t.status = ?2", &[&(m as i64), &st], limit),
        (Some(m), None) => trades_where(conn, "t.from_user = ?1 OR t.to_user = ?1", &[&(m as i64)], limit),
        (None, Some(st)) => trades_where(conn, "t.status = ?1", &[&st], limit),
        (None, None) => trades_where(conn, "1 = 1", &[], limit),
    }
}

// --- words and components ----------------------------------------------------------------

/// A card as a select option or a list line: "🍫 Merlin #4 · No. 0213".
pub fn card_line(card: &Card) -> String {
    let line = format!("{} {}", card.rarity.emoji(), store::card_label(&card.wizard_name, card.edition, card.serial));
    line.chars().take(100).collect()
}

/// Rarest first, then by number.
pub fn sort_for_trade(cards: &mut [Card]) {
    let rank = |r: Rarity| Rarity::ALL.iter().position(|x| *x == r).unwrap_or(0);
    cards.sort_by(|a, b| rank(b.rarity).cmp(&rank(a.rarity)).then(a.serial.cmp(&b.serial)));
}

pub fn pages(len: usize) -> usize {
    len.div_ceil(PAGE).max(1)
}

fn page_of(cards: &[Card], page: usize) -> &[Card] {
    let start = (page.min(pages(cards.len()) - 1)) * PAGE;
    &cards[start.min(cards.len())..(start + PAGE).min(cards.len())]
}

/// Folds one page's picks into a side's selection: the page's cards are
/// replaced by what was picked, capped at [`MAX_PER_SIDE`]. True when capped.
pub fn apply_picks(selected: &mut BTreeSet<i64>, page_cards: &[Card], picked: &[i64]) -> bool {
    for card in page_cards {
        selected.remove(&card.serial);
    }
    let mut capped = false;
    for serial in picked {
        if !page_cards.iter().any(|c| c.serial == *serial) {
            continue;
        }
        if selected.len() >= MAX_PER_SIDE {
            capped = true;
            break;
        }
        selected.insert(*serial);
    }
    capped
}

/// Which side of the builder: `g` the cards you give, `a` the ones you ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Give,
    Ask,
}

impl Side {
    fn key(self) -> &'static str {
        match self {
            Side::Give => "g",
            Side::Ask => "a",
        }
    }
    fn from_key(key: &str) -> Option<Side> {
        match key {
            "g" => Some(Side::Give),
            "a" => Some(Side::Ask),
            _ => None,
        }
    }
}

/// The select menu for one side's page.
pub fn select_menu(draft: &str, side: Side, cards: &[Card], page: usize, selected: &BTreeSet<i64>, whose: &str) -> CreateSelectMenu {
    let id = format!("tradesel:{}:{}:{}", draft, side.key(), page.min(pages(cards.len()) - 1));
    let shown = page_of(cards, page);
    if shown.is_empty() {
        let none = CreateSelectMenuOption::new("No cards", "none");
        let placeholder = match side {
            Side::Give => "You have no cards to give".to_string(),
            Side::Ask => format!("{} has no cards", whose),
        };
        return CreateSelectMenu::new(id, CreateSelectMenuKind::String { options: vec![none] }).placeholder(placeholder.chars().take(150).collect::<String>()).disabled(true);
    }
    let options: Vec<CreateSelectMenuOption> =
        shown.iter().map(|c| CreateSelectMenuOption::new(card_line(c), c.serial.to_string()).default_selection(selected.contains(&c.serial))).collect();
    let elsewhere = selected.iter().filter(|s| !shown.iter().any(|c| c.serial == **s)).count();
    let room = MAX_PER_SIDE.saturating_sub(elsewhere).min(shown.len()).max(1);
    let total = pages(cards.len());
    let mut placeholder = match side {
        Side::Give => "Cards you give".to_string(),
        Side::Ask => format!("Cards you ask {} for", whose),
    };
    if total > 1 {
        placeholder.push_str(&format!(" (page {} of {})", page.min(total - 1) + 1, total));
    }
    CreateSelectMenu::new(id, CreateSelectMenuKind::String { options })
        .min_values(0)
        .max_values(room as u8)
        .placeholder(placeholder.chars().take(150).collect::<String>())
}

fn lines_for(serials: &BTreeSet<i64>, cards: &[Card]) -> String {
    if serials.is_empty() {
        return "_nothing_".to_string();
    }
    serials.iter().map(|s| cards.iter().find(|c| c.serial == *s).map(card_line).unwrap_or_else(|| store::serial_label(*s))).collect::<Vec<_>>().join("\n")
}

fn list_lines(serials: &[i64], cards: &[Card]) -> String {
    if serials.is_empty() {
        return "_nothing_".to_string();
    }
    serials.iter().map(|s| cards.iter().find(|c| c.serial == *s).map(card_line).unwrap_or_else(|| store::serial_label(*s))).collect::<Vec<_>>().join("\n")
}

// --- the builder ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Draft {
    owner: u64,
    owner_name: String,
    target: u64,
    target_name: String,
    give: BTreeSet<i64>,
    ask: BTreeSet<i64>,
    give_page: usize,
    ask_page: usize,
    created: i64,
    note: Option<String>,
}

static DRAFTS: LazyLock<Mutex<HashMap<String, Draft>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn draft_id() -> String {
    rand::random::<[u8; 4]>().iter().map(|b| format!("{:02x}", b)).collect()
}

fn owned(user: u64) -> Vec<Card> {
    let mut cards = store::db().map(|db| store::cards_of(&db.lock(), user)).unwrap_or_default();
    sort_for_trade(&mut cards);
    cards
}

fn builder_message(id: &str, d: &Draft) -> CreateInteractionResponseMessage {
    let mine = owned(d.owner);
    let theirs = owned(d.target);
    let mut text = format!(
        "🔁 **Trade with <@{}>**\n\n**You give** ({}/{})\n{}\n\n**You ask for** ({}/{})\n{}\n\n-# Either side can be empty, not both · this draft closes in 15 min",
        d.target,
        d.give.len(),
        MAX_PER_SIDE,
        lines_for(&d.give, &mine),
        d.ask.len(),
        MAX_PER_SIDE,
        lines_for(&d.ask, &theirs)
    );
    if let Some(note) = &d.note {
        text.push_str(&format!("\n⚠️ {}", note));
    }
    let mut rows = vec![
        CreateActionRow::SelectMenu(select_menu(id, Side::Give, &mine, d.give_page, &d.give, &d.target_name)),
        CreateActionRow::SelectMenu(select_menu(id, Side::Ask, &theirs, d.ask_page, &d.ask, &d.target_name)),
    ];
    let mut paging = Vec::new();
    for (side, list, page, label) in [(Side::Give, &mine, d.give_page, "Yours"), (Side::Ask, &theirs, d.ask_page, "Theirs")] {
        let total = pages(list.len());
        if total > 1 {
            paging.push(
                CreateButton::new(format!("tradepg:{}:{}:{}:prev", id, side.key(), page.saturating_sub(1)))
                    .label(format!("◀ {}", label))
                    .style(ButtonStyle::Secondary)
                    .disabled(page == 0),
            );
            paging.push(
                CreateButton::new(format!("tradepg:{}:{}:{}:next", id, side.key(), (page + 1).min(total - 1)))
                    .label(format!("{} ▶", label))
                    .style(ButtonStyle::Secondary)
                    .disabled(page + 1 >= total),
            );
        }
    }
    if !paging.is_empty() {
        rows.push(CreateActionRow::Buttons(paging));
    }
    rows.push(CreateActionRow::Buttons(vec![
        CreateButton::new(format!("tradesend:{}", id)).label("Send offer").style(ButtonStyle::Success).disabled(d.give.is_empty() && d.ask.is_empty()),
        CreateButton::new(format!("tradedrop:{}", id)).label("Cancel").style(ButtonStyle::Secondary),
    ]));
    CreateInteractionResponseMessage::new().content(text).components(rows).ephemeral(true).allowed_mentions(CreateAllowedMentions::new())
}

pub fn command() -> CreateCommand {
    CreateCommand::new("trade")
        .description("offer Chocolate Frog cards to another member")
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "who to trade with").required(true))
}

pub fn trades_command_builder() -> CreateCommand {
    CreateCommand::new("trades").description("your open card trade offers and your latest trades")
}

fn house_member(user: u64) -> bool {
    if super::house::opted_out(user) {
        return false;
    }
    // Mods are in no house, but they may still play; the ledger refuses their points.
    super::house::house_of(user).is_some() || super::admin_ids().contains(&user)
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
}

pub async fn trade_command(ctx: &Context, command: &CommandInteraction) {
    let me = command.user.id.get();
    let target = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id),
        _ => None,
    });
    let reply = |text: &str| whisper(text.to_string());
    let Some(target) = target else {
        let _ = command.create_response(&ctx.http, reply("Pick who to trade with.")).await;
        return;
    };
    let problem = if !trades_on() {
        Some("Card trading is switched off right now.".to_string())
    } else if target.get() == me {
        Some("You can't trade with yourself.".to_string())
    } else if command.data.resolved.users.get(&target).is_some_and(|u| u.bot) {
        Some("Bots don't collect cards.".to_string())
    } else if !house_member(me) {
        Some("Only house members can trade cards.".to_string())
    } else if !house_member(target.get()) {
        Some("They're not in a house, so they can't trade cards.".to_string())
    } else {
        None
    };
    if let Some(text) = problem {
        let _ = command.create_response(&ctx.http, reply(&text)).await;
        return;
    }
    let owner_name = command.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| command.user.display_name().to_string());
    let target_name = command
        .data
        .resolved
        .members
        .get(&target)
        .and_then(|m| m.nick.clone())
        .or_else(|| command.data.resolved.users.get(&target).map(|u| u.display_name().to_string()))
        .unwrap_or_else(|| "them".to_string());
    let now = Utc::now().timestamp();
    let id = draft_id();
    let draft = Draft {
        owner: me,
        owner_name,
        target: target.get(),
        target_name,
        give: BTreeSet::new(),
        ask: BTreeSet::new(),
        give_page: 0,
        ask_page: 0,
        created: now,
        note: None,
    };
    let message = builder_message(&id, &draft);
    {
        let mut drafts = DRAFTS.lock();
        drafts.retain(|_, d| now - d.created < DRAFT_SECS);
        drafts.insert(id, draft);
    }
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("trade: builder for {} not shown: {}", me, err);
    }
}

/// `tradesel:<draft>:<side>:<page>` and `tradepg:<draft>:<side>:<page>:<dir>`.
fn parse_draft_id(custom_id: &str, prefix: &str) -> Option<(String, Side, usize)> {
    let mut parts = custom_id.strip_prefix(prefix)?.split(':');
    let id = parts.next()?.to_string();
    let side = Side::from_key(parts.next()?)?;
    let page = parts.next()?.parse().ok()?;
    Some((id, side, page))
}

async fn update(ctx: &Context, component: &ComponentInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await {
        tracing::warn!("trade: builder not updated: {}", err);
    }
}

fn gone() -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new().content("This trade draft has closed. Run /trade again.").components(Vec::new())
}

/// Every trade button and menu.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let user = component.user.id.get();
    if let Some((draft, side, page)) = parse_draft_id(&id, "tradesel:") {
        let picked: Vec<i64> = match &component.data.kind {
            ComponentInteractionDataKind::StringSelect { values } => values.iter().filter_map(|v| v.parse().ok()).collect(),
            _ => Vec::new(),
        };
        let current = DRAFTS.lock().get(&draft).cloned();
        let Some(mut d) = current.filter(|d| d.owner == user) else {
            return update(ctx, component, gone()).await;
        };
        let list = owned(if side == Side::Give { d.owner } else { d.target });
        let page_cards = page_of(&list, page).to_vec();
        let set = if side == Side::Give { &mut d.give } else { &mut d.ask };
        let capped = apply_picks(set, &page_cards, &picked);
        d.note = capped.then(|| format!("At most {} cards on each side.", MAX_PER_SIDE));
        let message = builder_message(&draft, &d);
        DRAFTS.lock().insert(draft, d);
        return update(ctx, component, message).await;
    }
    if let Some((draft, side, page)) = parse_draft_id(&id, "tradepg:") {
        let current = DRAFTS.lock().get(&draft).cloned();
        let Some(mut d) = current.filter(|d| d.owner == user) else {
            return update(ctx, component, gone()).await;
        };
        match side {
            Side::Give => d.give_page = page,
            Side::Ask => d.ask_page = page,
        }
        d.note = None;
        let message = builder_message(&draft, &d);
        DRAFTS.lock().insert(draft, d);
        return update(ctx, component, message).await;
    }
    if let Some(draft) = id.strip_prefix("tradedrop:") {
        DRAFTS.lock().remove(draft);
        return update(ctx, component, CreateInteractionResponseMessage::new().content("Trade cancelled. Nothing was sent.").components(Vec::new())).await;
    }
    if let Some(draft) = id.strip_prefix("tradesend:") {
        return send(ctx, component, draft).await;
    }
    let action = ["tradeacc:", "tradedec:", "tradecan:"].into_iter().find_map(|p| id.strip_prefix(p).map(|rest| (p, rest)));
    if let Some((prefix, rest)) = action {
        if let Ok(trade_id) = rest.parse::<i64>() {
            return respond(ctx, component, prefix, trade_id).await;
        }
    }
}

async fn send(ctx: &Context, component: &ComponentInteraction, draft_id: &str) {
    let user = component.user.id.get();
    let current = DRAFTS.lock().get(draft_id).cloned();
    let Some(d) = current.filter(|d| d.owner == user) else {
        return update(ctx, component, gone()).await;
    };
    if !trades_on() {
        return update(ctx, component, CreateInteractionResponseMessage::new().content("Card trading is switched off right now.").components(Vec::new())).await;
    }
    let now = Utc::now().timestamp();
    let give: Vec<i64> = d.give.iter().copied().collect();
    let ask: Vec<i64> = d.ask.iter().copied().collect();
    let created = match store::db() {
        Some(db) => {
            let mut conn = db.lock();
            create(
                &mut conn,
                (d.owner, &d.owner_name),
                (d.target, &d.target_name),
                component.guild_id.map(|g| g.get()),
                component.channel_id.get(),
                &give,
                &ask,
                now,
                expiry_secs(),
            )
        }
        None => Err("The card store isn't open.".to_string()),
    };
    let trade = match created {
        Ok(trade) => trade,
        Err(err) => {
            let mut d = d;
            d.note = Some(err);
            let message = builder_message(draft_id, &d);
            DRAFTS.lock().insert(draft_id.to_string(), d);
            return update(ctx, component, message).await;
        }
    };
    DRAFTS.lock().remove(draft_id);
    let cards = cards_for(&trade);
    let (content, embed, rows) = offer_view(&trade, &cards);
    let post = CreateMessage::new()
        .content(content)
        .embed(embed)
        .components(rows)
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(trade.to_user)]));
    match tokio::time::timeout(Duration::from_secs(20), component.channel_id.send_message(&ctx.http, post)).await {
        Ok(Ok(posted)) => {
            if let Some(db) = store::db() {
                let _ = set_message(&db.lock(), trade.id, posted.id.get());
            }
            tracing::info!("trade: {} offered trade {} to {}", trade.from_user, trade.id, trade.to_user);
            update(ctx, component, CreateInteractionResponseMessage::new().content(format!("Offer sent to <@{}>. It's open for {} h.", trade.to_user, expiry_secs() / 3600)).components(Vec::new())).await;
        }
        other => {
            let why = match other {
                Ok(Err(err)) => err.to_string(),
                _ => "Discord didn't answer".to_string(),
            };
            if let Some(db) = store::db() {
                let _ = close(&db.lock(), trade.id, "cancelled", None, now);
                let _ = mark_finished(&db.lock(), trade.id);
            }
            tracing::warn!("trade: offer {} not posted: {}", trade.id, why);
            update(ctx, component, CreateInteractionResponseMessage::new().content("I couldn't post the offer here. Try a channel where I can send messages.").components(Vec::new())).await;
        }
    }
}

fn cards_for(trade: &Trade) -> Vec<Card> {
    let serials: Vec<i64> = trade.give.iter().chain(trade.ask.iter()).copied().collect();
    store::db().map(|db| store::cards_by_serial(&db.lock(), &serials)).unwrap_or_default()
}

fn plain_name(name: &str, fallback: &str) -> String {
    let clean: String = name.chars().filter(|c| !matches!(c, '*' | '_' | '`' | '~' | '|' | '<' | '>' | '\\' | '@')).collect();
    let clean = clean.trim();
    if clean.is_empty() { fallback.to_string() } else { clean.chars().take(60).collect() }
}

/// The public offer message in any state: (content, embed, buttons).
pub fn offer_view(trade: &Trade, cards: &[Card]) -> (String, CreateEmbed, Vec<CreateActionRow>) {
    let a = plain_name(&trade.from_name, "They");
    let b = plain_name(&trade.to_name, "them");
    let give = list_lines(&trade.give, cards);
    let ask = list_lines(&trade.ask, cards);
    let (content, colour, gives, asks, line, button) = match trade.status.as_str() {
        "open" => (
            format!("🔁 Trade offer · <@{}> → <@{}>", trade.from_user, trade.to_user),
            TRADE_COLOUR,
            format!("{} gives", a),
            format!("{} asks for", a),
            format!("Expires <t:{}:R>", trade.expires_ts),
            None,
        ),
        "done" => (
            format!("✅ Trade done · <@{}> ⇄ <@{}>", trade.from_user, trade.to_user),
            DONE_COLOUR,
            format!("{} gave {}", a, b),
            format!("{} gave {}", b, a),
            "The cards have changed hands.".to_string(),
            Some("Trade done"),
        ),
        "declined" => (format!("❌ Trade declined · <@{}> → <@{}>", trade.from_user, trade.to_user), CLOSED_COLOUR, format!("{} offered", a), format!("{} asked for", a), format!("{} said no.", b), Some("Declined")),
        "cancelled" => (format!("🚫 Trade cancelled · <@{}> → <@{}>", trade.from_user, trade.to_user), CLOSED_COLOUR, format!("{} offered", a), format!("{} asked for", a), "The offer was withdrawn.".to_string(), Some("Cancelled")),
        "expired" => (format!("⌛ Trade expired · <@{}> → <@{}>", trade.from_user, trade.to_user), CLOSED_COLOUR, format!("{} offered", a), format!("{} asked for", a), "Nobody answered in time.".to_string(), Some("Expired")),
        _ => (format!("⚠️ Trade failed · <@{}> → <@{}>", trade.from_user, trade.to_user), CLOSED_COLOUR, format!("{} offered", a), format!("{} asked for", a), "Failed — cards changed hands before it was accepted. Nothing moved.".to_string(), Some("Failed")),
    };
    let embed = CreateEmbed::new()
        .colour(colour)
        .field(gives, give, true)
        .field(asks, ask, true)
        .description(line)
        .footer(CreateEmbedFooter::new(if trade.status == "open" {
            format!("Only {} can accept or decline · {} can cancel", b, a)
        } else {
            format!("Trade {}", trade.id)
        }));
    let rows = match button {
        None => vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("tradeacc:{}", trade.id)).label("Accept").style(ButtonStyle::Success),
            CreateButton::new(format!("tradedec:{}", trade.id)).label("Decline").style(ButtonStyle::Danger),
            CreateButton::new(format!("tradecan:{}", trade.id)).label("Cancel").style(ButtonStyle::Secondary),
        ])],
        Some(label) => vec![CreateActionRow::Buttons(vec![
            CreateButton::new(format!("tradeshut:{}", trade.id)).label(label).style(ButtonStyle::Secondary).disabled(true),
        ])],
    };
    (content, embed, rows)
}

/// Edits an offer's message to its state now.
async fn refresh(ctx: &Context, trade: &Trade) -> bool {
    let Some(message) = trade.message_id else {
        return false;
    };
    let cards = cards_for(trade);
    let (content, embed, rows) = offer_view(trade, &cards);
    let edit = EditMessage::new().content(content).embed(embed).components(rows).allowed_mentions(CreateAllowedMentions::new());
    let done = matches!(
        tokio::time::timeout(Duration::from_secs(20), ChannelId::new(trade.channel_id).edit_message(&ctx.http, MessageId::new(message), edit)).await,
        Ok(Ok(_))
    );
    if done && trade.status != "open" {
        if let Some(db) = store::db() {
            let _ = mark_finished(&db.lock(), trade.id);
        }
    }
    done
}

async fn respond(ctx: &Context, component: &ComponentInteraction, prefix: &str, trade_id: i64) {
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else {
        let _ = component.create_response(&ctx.http, whisper("The card store isn't open.")).await;
        return;
    };
    let Some(trade) = get(&db.lock(), trade_id) else {
        let _ = component.create_response(&ctx.http, whisper("That offer is gone.")).await;
        return;
    };
    let allowed = match prefix {
        "tradecan:" => user == trade.from_user,
        _ => user == trade.to_user,
    };
    if !allowed {
        let text = if prefix == "tradecan:" { "Only the member who made this offer can cancel it." } else { "This offer isn't for you." };
        let _ = component.create_response(&ctx.http, whisper(text)).await;
        return;
    }
    let (closed, note) = match prefix {
        "tradeacc:" => {
            let result = {
                let mut conn = db.lock();
                accept(&mut conn, trade_id, now)
            };
            match result {
                Ok(Accepted::Done(t)) => {
                    tracing::info!("trade: {} done ({} cards to {}, {} to {})", t.id, t.give.len(), t.to_user, t.ask.len(), t.from_user);
                    (Some(t), None)
                }
                Ok(Accepted::Failed(t, moved)) => {
                    let list = moved.iter().map(|s| store::serial_label(*s)).collect::<Vec<_>>().join(", ");
                    (Some(t), Some(format!("That trade can't happen: {} changed hands since the offer was made. Nothing moved.", list)))
                }
                Ok(Accepted::Expired(t)) => (Some(t), Some("That offer has expired.".to_string())),
                Ok(Accepted::NotOpen(t)) => (None, Some(format!("That offer is already {}.", t.status))),
                Ok(Accepted::Missing) => (None, Some("That offer is gone.".to_string())),
                Err(err) => {
                    tracing::warn!("trade: accepting {} failed: {}", trade_id, err);
                    (None, Some("Something went wrong. Nothing moved; try again.".to_string()))
                }
            }
        }
        other => {
            let status = if other == "tradedec:" { "declined" } else { "cancelled" };
            match close(&db.lock(), trade_id, status, Some(user), now) {
                Ok(Some(t)) => (Some(t), None),
                _ => (None, Some(format!("That offer is already {}.", trade.status))),
            }
        }
    };
    match (closed, note) {
        (Some(t), note) => {
            let cards = cards_for(&t);
            let (content, embed, rows) = offer_view(&t, &cards);
            let message = CreateInteractionResponseMessage::new().content(content).embed(embed).components(rows).allowed_mentions(CreateAllowedMentions::new());
            if component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(message)).await.is_ok() {
                let _ = mark_finished(&db.lock(), t.id);
            }
            if let Some(text) = note {
                let _ = component
                    .create_followup(&ctx.http, serenity::all::CreateInteractionResponseFollowup::new().content(text).ephemeral(true))
                    .await;
            }
        }
        (None, Some(text)) => {
            let _ = component.create_response(&ctx.http, whisper(text)).await;
        }
        (None, None) => {}
    }
}

/// Cancels an offer from the panel and updates its message.
pub async fn admin_cancel(ctx: &Context, trade_id: i64, by: u64) -> Result<Trade, String> {
    let db = store::db().ok_or("The card store isn't open.")?;
    let closed = close(&db.lock(), trade_id, "cancelled", Some(by), Utc::now().timestamp()).map_err(|e| e.to_string())?;
    let trade = closed.ok_or("That offer isn't open any more.")?;
    refresh(ctx, &trade).await;
    Ok(trade)
}

/// `/trades`: open offers both ways, and the latest done ones.
pub async fn trades_command(ctx: &Context, command: &CommandInteraction) {
    let me = command.user.id.get();
    let (open, done) = match store::db() {
        Some(db) => {
            let conn = db.lock();
            (list(&conn, Some(me), Some("open"), 50), list(&conn, Some(me), Some("done"), 10))
        }
        None => (Vec::new(), Vec::new()),
    };
    let text = trades_text(me, &open, &done);
    let embed = CreateEmbed::new().title("🔁 Your card trades").description(text).colour(TRADE_COLOUR);
    let message = CreateInteractionResponseMessage::new().embed(embed).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await;
}

fn jump(trade: &Trade) -> String {
    match (trade.guild_id, trade.message_id) {
        (Some(g), Some(m)) => format!(" · [open](https://discord.com/channels/{}/{}/{})", g, trade.channel_id, m),
        _ => String::new(),
    }
}

/// The /trades list for a member.
pub fn trades_text(me: u64, open: &[Trade], done: &[Trade]) -> String {
    let sent: Vec<&Trade> = open.iter().filter(|t| t.from_user == me).collect();
    let received: Vec<&Trade> = open.iter().filter(|t| t.to_user == me).collect();
    let count = |t: &Trade| format!("{} for {}", t.give.len(), t.ask.len());
    let mut text = String::from("**Waiting for you**\n");
    if received.is_empty() {
        text.push_str("_nothing_\n");
    }
    for t in &received {
        text.push_str(&format!("From <@{}> · {} cards · expires <t:{}:R>{}\n", t.from_user, count(t), t.expires_ts, jump(t)));
    }
    text.push_str("\n**You sent**\n");
    if sent.is_empty() {
        text.push_str("_nothing_\n");
    }
    for t in &sent {
        text.push_str(&format!("To <@{}> · {} cards · expires <t:{}:R>{}\n", t.to_user, count(t), t.expires_ts, jump(t)));
    }
    text.push_str("\n**Latest trades**\n");
    if done.is_empty() {
        text.push_str("_none yet_\n");
    }
    for t in done.iter().take(10) {
        let other = if t.from_user == me { t.to_user } else { t.from_user };
        let (gave, got) = if t.from_user == me { (t.give.len(), t.ask.len()) } else { (t.ask.len(), t.give.len()) };
        text.push_str(&format!("With <@{}> · gave {}, got {} · <t:{}:d>{}\n", other, gave, got, t.closed_ts.unwrap_or(t.created_ts), jump(t)));
    }
    text.trim_end().to_string()
}

/// Expires overdue offers and finishes messages a restart left behind.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let now = Utc::now().timestamp();
            let (expired, left) = match store::db() {
                Some(db) => {
                    let conn = db.lock();
                    let expired: Vec<Trade> = overdue(&conn, now).into_iter().filter_map(|t| close(&conn, t.id, "expired", None, now).ok().flatten()).collect();
                    (expired, unfinished(&conn))
                }
                None => (Vec::new(), Vec::new()),
            };
            for t in expired.iter().chain(left.iter().filter(|l| !expired.iter().any(|e| e.id == l.id))) {
                if !refresh(&ctx, t).await {
                    // A deleted message must not be retried for ever.
                    if let Some(db) = store::db() {
                        let _ = mark_finished(&db.lock(), t.id);
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::frog_store::{AwardFor, tests::memory};

    const NOW: i64 = 1_789_367_400;

    /// Gives `user` an earned card of the given card id, returning its serial.
    fn give_card(conn: &mut Connection, user: u64, key: &str) -> i64 {
        let what = AwardFor { kind: "royale", ..Default::default() };
        store::award_card(conn, key, user, "royale_champion", &what, (0.1, 0.3), NOW).unwrap().unwrap().card.serial
    }

    fn owner(conn: &Connection, serial: i64) -> u64 {
        conn.query_row("SELECT user_id FROM cards WHERE serial = ?1", params![serial], |r| r.get::<_, i64>(0)).unwrap() as u64
    }

    fn card(serial: i64, rarity: Rarity) -> Card {
        Card {
            serial,
            edition: serial,
            user_id: 1,
            wizard_id: 1,
            wizard_name: "The Original Chocolate Frog".into(),
            rarity,
            drop_id: None,
            ts: NOW,
            origin: "caught".into(),
            original_owner: 1,
            status: "owned".into(),
            spent_reason: String::new(),
            spent_ts: None,
        }
    }

    #[test]
    fn an_accepted_trade_moves_every_card_and_writes_it_down() {
        let mut conn = memory();
        let a1 = give_card(&mut conn, 1, "k1");
        let a2 = give_card(&mut conn, 1, "k2");
        let b1 = give_card(&mut conn, 2, "k3");
        let t = create(&mut conn, (1, "Aarav"), (2, "Zoya"), Some(9), 77, &[a1, a2], &[b1], NOW, 86_400).unwrap();
        assert_eq!((t.status.as_str(), t.give.clone(), t.ask.clone(), t.to_name.as_str()), ("open", vec![a1, a2], vec![b1], "Zoya"));
        let Accepted::Done(done) = accept(&mut conn, t.id, NOW + 60).unwrap() else { panic!("should be done") };
        assert_eq!((done.status.as_str(), done.closed_by), ("done", Some(2)));
        assert_eq!((owner(&conn, a1), owner(&conn, a2), owner(&conn, b1)), (2, 2, 1));
        assert_eq!(store::transfers_of(&conn, a1), vec![(1, 2, Some(t.id), NOW + 60)]);
        assert_eq!(store::transfers_of(&conn, b1), vec![(2, 1, Some(t.id), NOW + 60)]);
        let moved = store::cards_of(&conn, 2);
        assert!(moved.iter().all(|c| c.traded_in()), "both came by trade");
        assert_eq!(moved.iter().find(|c| c.serial == a1).unwrap().original_owner, 1);
        // Accepting again does nothing.
        assert!(matches!(accept(&mut conn, t.id, NOW + 61).unwrap(), Accepted::NotOpen(_)));
        assert_eq!(accept(&mut conn, 999, NOW).unwrap(), Accepted::Missing);
    }

    #[test]
    fn a_card_that_moved_meanwhile_fails_the_trade_and_nothing_moves() {
        let mut conn = memory();
        let a1 = give_card(&mut conn, 1, "k1");
        let a2 = give_card(&mut conn, 1, "k2");
        let c1 = give_card(&mut conn, 3, "k3");
        // The same card offered twice: to Zoya and to Rohan.
        let first = create(&mut conn, (1, "A"), (2, "Z"), None, 77, &[a1, a2], &[], NOW, 86_400).unwrap();
        let second = create(&mut conn, (1, "A"), (3, "R"), None, 77, &[a2], &[c1], NOW, 86_400).unwrap();
        assert!(matches!(accept(&mut conn, second.id, NOW + 10).unwrap(), Accepted::Done(_)));
        let Accepted::Failed(failed, moved) = accept(&mut conn, first.id, NOW + 20).unwrap() else { panic!("should fail") };
        assert_eq!((failed.status.as_str(), moved), ("failed", vec![a2]));
        assert_eq!(owner(&conn, a1), 1, "the card that was still there didn't move either");
        assert_eq!(owner(&conn, a2), 3);
        assert!(store::transfers_of(&conn, a1).is_empty());
        // A spent card can't be traded.
        conn.execute("UPDATE cards SET status = 'spent' WHERE serial = ?1", params![a1]).unwrap();
        let spent = create(&mut conn, (1, "A"), (2, "Z"), None, 77, &[a1], &[], NOW, 86_400).unwrap();
        assert!(matches!(accept(&mut conn, spent.id, NOW + 30).unwrap(), Accepted::Failed(_, _)));
    }

    #[test]
    fn trades_move_no_house_points() {
        // Trading only ever touches frog.db: the ledger lives in house.db and is
        // never opened here. Accepting on a store with no ledger at all works.
        let mut conn = memory();
        let a1 = give_card(&mut conn, 1, "k1");
        let t = create(&mut conn, (1, "A"), (2, "Z"), None, 77, &[a1], &[], NOW, 86_400).unwrap();
        assert!(matches!(accept(&mut conn, t.id, NOW).unwrap(), Accepted::Done(_)));
        let tables: Vec<String> = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'").unwrap().query_map([], |r| r.get(0)).unwrap().flatten().collect();
        assert!(!tables.iter().any(|t| t == "ledger"));
    }

    #[test]
    fn offers_are_checked_expire_and_close_once() {
        let mut conn = memory();
        let a1 = give_card(&mut conn, 1, "k1");
        assert!(create(&mut conn, (1, "A"), (1, "A"), None, 77, &[a1], &[], NOW, 60).unwrap_err().contains("yourself"));
        assert!(create(&mut conn, (1, "A"), (2, "Z"), None, 77, &[], &[], NOW, 60).unwrap_err().contains("at least one"));
        let many: Vec<i64> = (1..=11).collect();
        assert!(create(&mut conn, (1, "A"), (2, "Z"), None, 77, &many, &[], NOW, 60).unwrap_err().contains("At most 10"));
        let gift = create(&mut conn, (1, "A"), (2, "Z"), None, 77, &[a1], &[], NOW, 3600).unwrap();
        let request = create(&mut conn, (1, "A"), (2, "Z"), None, 77, &[], &[a1], NOW, 3600).unwrap();
        assert!(overdue(&conn, NOW + 3599).is_empty());
        assert_eq!(overdue(&conn, NOW + 3600).len(), 2);
        let Accepted::Expired(t) = accept(&mut conn, gift.id, NOW + 4000).unwrap() else { panic!() };
        assert_eq!(t.status, "expired");
        assert_eq!(owner(&conn, a1), 1);
        let closed = close(&conn, request.id, "declined", Some(2), NOW + 10).unwrap().unwrap();
        assert_eq!((closed.status.as_str(), closed.closed_by), ("declined", Some(2)));
        assert!(close(&conn, request.id, "cancelled", Some(1), NOW + 11).unwrap().is_none(), "closed once");
        set_message(&conn, request.id, 555).unwrap();
        assert_eq!(unfinished(&conn).iter().map(|t| t.id).collect::<Vec<_>>(), vec![request.id]);
        mark_finished(&conn, request.id).unwrap();
        assert!(unfinished(&conn).is_empty());
        assert_eq!(list(&conn, Some(2), None, 10).len(), 2);
        assert_eq!(list(&conn, Some(2), Some("declined"), 10).len(), 1);
        assert_eq!(list(&conn, Some(3), None, 10).len(), 0);
        assert_eq!(list(&conn, None, Some("expired"), 10).len(), 1);
    }

    #[test]
    fn select_menus_list_a_page_of_25_with_short_labels() {
        let mut cards: Vec<Card> = (1..=60).map(|i| card(i, if i % 7 == 0 { Rarity::Uncommon } else { Rarity::Common })).collect();
        cards.push(card(61, Rarity::Legendary));
        sort_for_trade(&mut cards);
        assert_eq!(cards[0].serial, 61, "rarest first");
        assert_eq!(cards[1].serial, 7);
        assert_eq!(pages(cards.len()), 3);
        let mut selected = BTreeSet::new();
        selected.insert(cards[30].serial);
        let menu = serde_json::to_value(select_menu("d1", Side::Give, &cards, 0, &selected, "Zoya")).unwrap();
        let options = menu["options"].as_array().unwrap();
        assert_eq!(options.len(), 25);
        assert!(options.iter().all(|o| o["label"].as_str().unwrap().chars().count() <= 100));
        assert_eq!(options[0]["label"], "🔥 The Original Chocolate Frog #61 · No. 0061");
        assert_eq!(menu["custom_id"], "tradesel:d1:g:0");
        assert_eq!((menu["min_values"].as_u64(), menu["max_values"].as_u64()), (Some(0), Some(9)), "one already picked on another page");
        assert!(menu["placeholder"].as_str().unwrap().ends_with("(page 1 of 3)"));
        let mut both = selected.clone();
        both.insert(cards[55].serial);
        let last = serde_json::to_value(select_menu("d1", Side::Ask, &cards, 7, &both, "Zoya")).unwrap();
        assert_eq!(last["custom_id"], "tradesel:d1:a:2", "a page past the end is the last");
        assert_eq!(last["options"].as_array().unwrap().len(), 11);
        assert_eq!(last["options"][5]["default"], true);
        let empty = serde_json::to_value(select_menu("d1", Side::Ask, &[], 0, &BTreeSet::new(), "Zoya")).unwrap();
        assert_eq!((empty["disabled"].as_bool(), empty["placeholder"].as_str()), (Some(true), Some("Zoya has no cards")));
        let long = Card { wizard_name: "x".repeat(200), ..card(1, Rarity::Common) };
        assert_eq!(card_line(&long).chars().count(), 100);
    }

    #[test]
    fn picks_replace_a_page_and_stop_at_ten() {
        let cards: Vec<Card> = (1..=30).map(|i| card(i, Rarity::Common)).collect();
        let page0 = page_of(&cards, 0);
        let page1 = page_of(&cards, 1);
        assert_eq!((page0.len(), page1.len()), (25, 5));
        let mut picked = BTreeSet::new();
        assert!(!apply_picks(&mut picked, page1, &[26, 27]));
        assert!(!apply_picks(&mut picked, page0, &[1, 2, 3]));
        assert_eq!(picked.iter().copied().collect::<Vec<_>>(), vec![1, 2, 3, 26, 27]);
        assert!(!apply_picks(&mut picked, page0, &[4]), "unticking on a page drops those cards");
        assert_eq!(picked.iter().copied().collect::<Vec<_>>(), vec![4, 26, 27]);
        assert!(apply_picks(&mut picked, page0, &(1..=20).collect::<Vec<_>>()), "capped");
        assert_eq!(picked.len(), MAX_PER_SIDE);
        assert!(!apply_picks(&mut picked, page0, &[99]), "a card not on the page is ignored");
        assert_eq!(picked.iter().copied().collect::<Vec<_>>(), vec![26, 27]);
    }

    #[test]
    fn the_offer_reads_right_in_every_state() {
        let mut trade = Trade {
            id: 5,
            from_user: 1,
            to_user: 2,
            from_name: "Aarav".into(),
            to_name: "Zoya".into(),
            guild_id: Some(9),
            channel_id: 77,
            message_id: Some(555),
            status: "open".into(),
            created_ts: NOW,
            expires_ts: NOW + 86_400,
            closed_ts: None,
            closed_by: None,
            give: vec![4],
            ask: vec![],
        };
        let cards = vec![Card { wizard_name: "Merlin".into(), edition: 4, serial: 4, ..card(4, Rarity::Uncommon) }];
        let (content, embed, rows) = offer_view(&trade, &cards);
        assert_eq!(content, "🔁 Trade offer · <@1> → <@2>");
        let embed = serde_json::to_value(embed).unwrap();
        assert_eq!(embed["fields"][0]["name"], "Aarav gives");
        assert_eq!(embed["fields"][0]["value"], "🍫 Merlin #4 · No. 0004");
        assert_eq!(embed["fields"][1]["value"], "_nothing_");
        assert_eq!(embed["description"], format!("Expires <t:{}:R>", NOW + 86_400));
        let rows = serde_json::to_value(rows).unwrap();
        let ids: Vec<&str> = rows[0]["components"].as_array().unwrap().iter().map(|b| b["custom_id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["tradeacc:5", "tradedec:5", "tradecan:5"]);
        for (status, head, label) in [
            ("done", "✅ Trade done · <@1> ⇄ <@2>", "Trade done"),
            ("declined", "❌ Trade declined", "Declined"),
            ("expired", "⌛ Trade expired", "Expired"),
            ("failed", "⚠️ Trade failed", "Failed"),
            ("cancelled", "🚫 Trade cancelled", "Cancelled"),
        ] {
            trade.status = status.into();
            let (content, embed, rows) = offer_view(&trade, &cards);
            assert!(content.starts_with(head), "{content}");
            let rows = serde_json::to_value(rows).unwrap();
            assert_eq!((rows[0]["components"][0]["label"].as_str(), rows[0]["components"][0]["disabled"].as_bool()), (Some(label), Some(true)));
            if status == "failed" {
                assert!(serde_json::to_value(embed).unwrap()["description"].as_str().unwrap().contains("cards changed hands"));
            }
        }
        let done = Trade { status: "done".into(), closed_ts: Some(NOW), ..trade.clone() };
        let open = Trade { status: "open".into(), ..trade };
        let text = trades_text(2, &[open.clone()], &[done]);
        assert!(text.contains(&format!("From <@1> · 1 for 0 cards · expires <t:{}:R> · [open](https://discord.com/channels/9/77/555)", NOW + 86_400)), "{text}");
        assert!(text.contains("**You sent**\n_nothing_"), "{text}");
        assert!(text.contains("With <@1> · gave 0, got 1"), "{text}");
        assert!(trades_text(1, &[open], &[]).contains("To <@2>"));
    }

    #[test]
    fn builder_ids_parse() {
        assert_eq!(parse_draft_id("tradesel:ab12cd34:g:2", "tradesel:"), Some(("ab12cd34".to_string(), Side::Give, 2)));
        assert_eq!(parse_draft_id("tradepg:ab12cd34:a:1:next", "tradepg:"), Some(("ab12cd34".to_string(), Side::Ask, 1)));
        assert_eq!(parse_draft_id("tradesel:ab:x:2", "tradesel:"), None);
        assert_eq!(parse_draft_id("tradeacc:5", "tradesel:"), None);
    }
}
