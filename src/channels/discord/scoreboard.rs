//! The House Cup channel (`VIZIER_SCOREBOARD_CHANNEL`), kept in a fixed order:
//!
//! 1. the welcome (plain text),
//! 2. the Snitch & Chocolate Frog cards post (plain text with two how-to pictures),
//! 3. the beginner's guide (embeds),
//! 4. the scoreboard card, always the last message.
//!
//! The first three are written from the live settings (see `rules_text`) and
//! edited in place when a setting changes their words; one that goes missing is
//! posted again, with everything below it, so the order holds. The card is
//! posted every hour and moved back to the bottom whenever anything else lands
//! in the channel. The card names no members; names only appear in the private
//! replies to its buttons.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use chrono::{Datelike, NaiveDate, TimeZone, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, params};
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions,
    CreateAttachment, CreateButton, CreateCommand, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, EditMessage, GetMessages, Message, MessageId,
};

use super::house::{self, HOUSES, House};
use super::points::{self as ledger, Source};
use super::rules_text::{self, Panel, Rules};

const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;
const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
const BAR_WIDTH: usize = 12;
/// How many names "Top this hour" and "My house" list.
const TOP: usize = 5;
/// How many names "Top today" lists.
const TOP_TODAY: usize = 10;
/// Posts landing within this long of each other move the card down once.
const BUMP_DELAY_MS: i64 = 5_000;
/// How often the posts above the card are checked against the settings.
const POLL_SECS: u64 = 60;

static SNITCH_HOW: &[u8] = include_bytes!("../../../assets/howto/snitch-how.png");
static FROG_HOW: &[u8] = include_bytes!("../../../assets/howto/frog-how.png");

fn channel() -> Option<ChannelId> {
    super::control::id("VIZIER_SCOREBOARD_CHANNEL").filter(|id| *id != 0).map(ChannelId::new)
}

fn ist_hour_floor(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(HOUR) * HOUR - IST_OFFSET
}

fn ist_hour(ts: i64) -> i64 {
    (ts + IST_OFFSET).rem_euclid(DAY) / HOUR
}

/// Midnight India time on the day a moment falls on.
fn ist_day_start(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(DAY) * DAY - IST_OFFSET
}

fn hour12(hour: i64) -> String {
    match hour.rem_euclid(24) {
        0 => "12am".into(),
        12 => "12pm".into(),
        h if h < 12 => format!("{}am", h),
        h => format!("{}pm", h - 12),
    }
}

fn ist(ts: i64) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::FixedOffset::east_opt(IST_OFFSET as i32).and_then(|tz| tz.timestamp_opt(ts, 0).single())
}

/// "8:00 pm".
fn clock(ts: i64) -> String {
    ist(ts).map(|t| t.format("%-I:%M %P").to_string()).unwrap_or_default()
}

fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 { format!("-{}", out) } else { out }
}

fn signed(n: i64) -> String {
    if n >= 0 { format!("+{}", n) } else { n.to_string() }
}

/// "1st", "2nd", "3rd", "11th", "22nd".
fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

/// A bar of `BAR_WIDTH` blocks, full for the leader.
fn bar(points: i64, best: i64) -> String {
    let filled = if best <= 0 { 0 } else { ((points.max(0) as f64 / best as f64) * BAR_WIDTH as f64).round() as usize };
    let filled = filled.min(BAR_WIDTH);
    format!("{}{}", "█".repeat(filled), "░".repeat(BAR_WIDTH - filled))
}

fn month_name(ts: i64) -> String {
    ist(ts).map(|t| t.format("%B").to_string()).unwrap_or_default()
}

/// Whole days left in the India month after the day `ts` falls on: 0 on the last day.
fn days_left(ts: i64) -> i64 {
    let Some(date) = ist(ts).map(|t| t.date_naive()) else {
        return 0;
    };
    let (y, m) = if date.month() == 12 { (date.year() + 1, 1) } else { (date.year(), date.month() + 1) };
    NaiveDate::from_ymd_opt(y, m, 1).map(|first| (first - date).num_days() - 1).unwrap_or(0)
}

fn days_words(left: i64) -> String {
    match left {
        n if n <= 0 => "last day of the month".into(),
        1 => "1 day to go".into(),
        n => format!("{} days to go", n),
    }
}

/// When the next scheduled card goes up after `now`: the end of the next hour
/// inside `[first, last]` that lines up with `every`.
fn next_post(now: i64, first: i64, last: i64, every: i64) -> Option<i64> {
    let every = every.clamp(1, 24);
    (1..=48).map(|k| ist_hour_floor(now) + k * HOUR).find(|end| {
        let hour = ist_hour(end - HOUR);
        (first..=last).contains(&hour) && (hour - first).rem_euclid(every) == 0
    })
}

// --- the card ---------------------------------------------------------------------------

/// Everything the card shows for the hour ending at `end`.
pub struct CardData {
    pub end: i64,
    pub month: HashMap<&'static str, i64>,
    pub hour: HashMap<&'static str, i64>,
    pub today: HashMap<&'static str, i64>,
    /// The hour's top sources across all houses, biggest first.
    pub sources: Vec<(Source, i64)>,
    pub next: Option<i64>,
}

fn ranked(month: &HashMap<&'static str, i64>) -> Vec<&'static House> {
    let mut rows: Vec<&'static House> = HOUSES.iter().collect();
    rows.sort_by(|a, b| month.get(b.key).cmp(&month.get(a.key)).then(a.name.cmp(b.name)));
    rows
}

/// The three biggest sources of points, all houses together.
fn top_sources(moved: &HashMap<(&'static str, Source), i64>) -> Vec<(Source, i64)> {
    let mut sums: Vec<(Source, i64)> =
        Source::ALL.iter().map(|s| (*s, moved.iter().filter(|((_, src), _)| src == s).map(|(_, n)| n).sum::<i64>())).filter(|(_, n)| *n > 0).collect();
    sums.sort_by(|a, b| b.1.cmp(&a.1));
    sums.truncate(3);
    sums
}

fn hottest(rows: &[&'static House], hour: &HashMap<&'static str, i64>) -> String {
    let best = rows.iter().map(|h| (*h, hour.get(h.key).copied().unwrap_or(0))).filter(|(_, n)| *n > 0).max_by(|a, b| a.1.cmp(&b.1).then(std::cmp::Ordering::Greater));
    match best {
        Some((h, n)) => format!("{} {} +{}", h.crest, h.name, n),
        None => "Quiet hour".into(),
    }
}

pub fn card(d: &CardData) -> CreateEmbed {
    let rows = ranked(&d.month);
    let best = rows.first().and_then(|h| d.month.get(h.key)).copied().unwrap_or(0);
    let medals = ["🥇", "🥈", "🥉", "4️⃣"];
    let mut lines = Vec::new();
    for (i, h) in rows.iter().enumerate() {
        let points = d.month.get(h.key).copied().unwrap_or(0);
        let gain = match d.hour.get(h.key).copied().unwrap_or(0) {
            0 => String::new(),
            n => format!(" · **{}** this hour", signed(n)),
        };
        lines.push(format!(
            "{} {} **{}** — **{}** pts{}\n`{}`",
            medals.get(i).copied().unwrap_or("•"),
            h.crest,
            h.name,
            thousands(points),
            gain,
            bar(points, best)
        ));
    }
    let second = rows.get(1).and_then(|h| d.month.get(h.key)).copied().unwrap_or(0);
    let left = days_words(days_left(d.end - 1));
    let lead = match rows.first() {
        Some(h) if best > second => format!("{} **{}** lead by **{}** · {}", h.crest, h.name, thousands(best - second), left),
        _ => format!("It's neck and neck at the top · {}", left),
    };
    let today: Vec<String> = rows.iter().map(|h| format!("{} {}", h.crest, thousands(d.today.get(h.key).copied().unwrap_or(0)))).collect();
    let sources = if d.sources.is_empty() {
        "Nothing this hour".to_string()
    } else {
        d.sources.iter().map(|(s, n)| format!("{} {}", s.label(), n)).collect::<Vec<_>>().join(" · ")
    };
    let next = d.next.map(|t| format!("<t:{}:R> (<t:{}:t>)", t, t)).unwrap_or_else(|| "Not scheduled".into());
    let spacer = ("\u{200b}", "\u{200b}", true);
    CreateEmbed::new()
        .title(format!("🏆 House Cup · {}", month_name(d.end - 1)))
        .description(format!("{}\n\n{}", lead, lines.join("\n\n")))
        .colour(rows.first().filter(|_| best > second).map(|h| h.colour).unwrap_or(0xD4A73C))
        .fields(vec![
            ("🔥 Hottest this hour", hottest(&rows, &d.hour), true),
            ("📅 Today so far", today.join(" · "), true),
            (spacer.0, spacer.1.to_string(), spacer.2),
            ("⚡ Last hour's points from", sources, true),
            ("⏰ Next update", next, true),
            (spacer.0, spacer.1.to_string(), spacer.2),
        ])
        .footer(CreateEmbedFooter::new(format!("Updated {} India time · always the last message here", clock(d.end))))
}

pub fn buttons(end: i64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("scoreboard:hour:{}", end)).label("Top this hour").emoji('⭐').style(ButtonStyle::Primary),
        CreateButton::new(format!("scoreboard:today:{}", end)).label("Top today").emoji('📅').style(ButtonStyle::Secondary),
        CreateButton::new("scoreboard:house").label("My house").emoji('🏠').style(ButtonStyle::Secondary),
        CreateButton::new("scoreboard:me").label("My points").emoji('📊').style(ButtonStyle::Secondary),
        CreateButton::new("scoreboard:earn").label("How to earn").emoji('❓').style(ButtonStyle::Secondary),
    ])
}

/// The card's figures, read in one go under the ledger lock.
fn card_data(conn: &Connection, end: i64) -> CardData {
    let month = ledger::house_totals(conn, ledger::month_start(end - 1)).unwrap_or_default();
    let moved = ledger::by_source(conn, end - HOUR, end).unwrap_or_default();
    let mut hour: HashMap<&'static str, i64> = HashMap::new();
    for ((h, _), n) in &moved {
        *hour.entry(h).or_insert(0) += n;
    }
    let day = ist_day_start(end - 1);
    let today = HOUSES.iter().map(|h| (h.key, ledger::house_total(conn, h.key, day, end).unwrap_or(0))).collect();
    CardData { end, month, hour, today, sources: top_sources(&moved), next: None }
}

fn schedule() -> (i64, i64, i64) {
    let every = super::control::number("VIZIER_SCOREBOARD_EVERY_HOURS", 1).clamp(1, 24) as i64;
    let first = super::control::number("VIZIER_SCOREBOARD_FIRST_HOUR", 10) as i64;
    let last = super::control::number("VIZIER_SCOREBOARD_LAST_HOUR", 23) as i64;
    (first, last, every)
}

// --- the buttons ------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum Press {
    Hour(i64),
    Today(i64),
    House,
    Me,
    Earn,
}

fn parse_press(id: &str) -> Option<Press> {
    let rest = id.strip_prefix("scoreboard:")?;
    let (kind, arg) = rest.split_once(':').map_or((rest, None), |(k, a)| (k, Some(a)));
    let end = || arg.and_then(|a| a.parse::<i64>().ok());
    match kind {
        // "top" is the button on cards posted before the others existed.
        "hour" | "top" => end().map(Press::Hour),
        "today" => end().map(Press::Today),
        "house" => Some(Press::House),
        "me" => Some(Press::Me),
        "earn" => Some(Press::Earn),
        _ => None,
    }
}

type Scorer = (u64, &'static House, i64, Vec<(Source, i64)>);

/// Everyone's points in `[start, end)`: (user, house, total, sources), best first.
fn scorers(conn: &Connection, start: i64, end: i64) -> rusqlite::Result<Vec<Scorer>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, house, source, SUM(points), MAX(ts) FROM ledger
         WHERE user_id IS NOT NULL AND ts >= ?1 AND ts < ?2 GROUP BY user_id, house, source",
    )?;
    let rows = stmt.query_map(params![start, end], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?))
    })?;
    let mut by_user: HashMap<u64, (&'static House, i64, i64, Vec<(Source, i64)>)> = HashMap::new();
    for (user, house_key, source, sum, last) in rows.flatten() {
        let (Some(h), Some(s)) = (HOUSES.iter().find(|h| h.key == house_key), Source::from_key(&source)) else {
            continue;
        };
        let entry = by_user.entry(user).or_insert((h, 0, 0, Vec::new()));
        entry.1 += sum;
        entry.2 = entry.2.max(last);
        if sum != 0 {
            entry.3.push((s, sum));
        }
    }
    let mut out: Vec<(u64, &'static House, i64, i64, Vec<(Source, i64)>)> = by_user
        .into_iter()
        .filter(|(_, v)| v.1 > 0)
        .map(|(u, (h, t, last, mut s))| {
            s.sort_by(|a, b| b.1.cmp(&a.1));
            (u, h, t, last, s)
        })
        .collect();
    // Most points; a tie goes to whoever finished scoring first.
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.3.cmp(&b.3)).then(a.0.cmp(&b.0)));
    Ok(out.into_iter().map(|(u, h, t, _, s)| (u, h, t, s)).collect())
}

fn medal(i: usize) -> String {
    match i {
        0 => "🥇".into(),
        1 => "🥈".into(),
        2 => "🥉".into(),
        n => format!("`{:>2}.`", n + 1),
    }
}

/// "Top this hour".
pub fn top_text(start: i64, end: i64, rows: &[Scorer]) -> String {
    let when = format!("{} – {}", hour12(ist_hour(start)), hour12(ist_hour(end)));
    if rows.is_empty() {
        return format!("⭐ **Top scorers · {}**\nNobody scored in that hour.", when);
    }
    let mut lines = vec![format!("⭐ **Top scorers · {} India time**", when)];
    for (i, (user, h, total, sources)) in rows.iter().take(TOP).enumerate() {
        let from: Vec<String> = sources.iter().take(3).map(|(s, n)| format!("{} {}", s.label(), n)).collect();
        let rank = if i < 3 { medal(i) } else { "▫️".into() };
        lines.push(format!("{} <@{}> {} — **{}** pts\n-# {}", rank, user, h.crest, total, from.join(" · ")));
    }
    lines.join("\n")
}

/// "Top today": the day's top ten, and where the presser stands.
fn today_top_text(label: &str, is_today: bool, rows: &[(u64, &'static House, i64)], presser: u64) -> String {
    let mut lines = vec![format!("📅 **Top scorers · {}**", label)];
    if rows.is_empty() {
        lines.push("Nobody has scored yet.".into());
    }
    for (i, (user, h, total)) in rows.iter().take(TOP_TODAY).enumerate() {
        lines.push(format!("{} <@{}> {} **{}**", medal(i), user, h.crest, total));
    }
    let when = if is_today { "today" } else { "that day" };
    lines.push(match rows.iter().position(|r| r.0 == presser) {
        None => format!("-# You haven't scored {} yet", when),
        Some(0) => match rows.get(1) {
            Some(next) if next.2 < rows[0].2 => format!("-# You: 1st with {} pts — {} ahead of 2nd", rows[0].2, rows[0].2 - next.2),
            Some(_) => format!("-# You: 1st with {} pts — level with 2nd", rows[0].2),
            None => format!("-# You: 1st with {} pts", rows[0].2),
        },
        Some(i) => {
            let (mine, above) = (rows[i].2, rows[i - 1].2);
            if above > mine {
                format!("-# You: {} with {} pts — {} behind {}", ordinal(i + 1), mine, above - mine, ordinal(i))
            } else {
                format!("-# You: {} with {} pts — level with {}", ordinal(i + 1), mine, ordinal(i))
            }
        }
    });
    lines.join("\n")
}

/// "My house". `standings` is every house best first; `members` the house's
/// scorers this month, best first, Muggles already left out.
fn my_house_text(
    h: &'static House,
    standings: &[(&'static House, i64)],
    today: i64,
    hour: i64,
    members: &[(u64, i64)],
    captain: Option<u64>,
    presser: u64,
) -> String {
    let pos = standings.iter().position(|(x, _)| x.key == h.key).unwrap_or(0);
    let mine = standings.get(pos).map_or(0, |s| s.1);
    let place = if pos == 0 {
        match standings.get(1) {
            Some((_, second)) if mine > *second => format!("leading by **{}**", thousands(mine - second)),
            _ => "level at the top".to_string(),
        }
    } else {
        let (leader, top) = standings[0];
        if top > mine {
            format!("**{}** behind {} {}", thousands(top - mine), leader.crest, leader.name)
        } else {
            format!("level with {} {}", leader.crest, leader.name)
        }
    };
    let mut lines = vec![
        format!("{} **{}** · {} · {}", h.crest, h.name, ordinal(pos + 1), place),
        format!("Today **{}** · this hour **{}**", signed(today), signed(hour)),
        "**Top this month**".to_string(),
    ];
    if members.is_empty() {
        lines.push("Nobody has scored yet.".into());
    } else {
        let top: Vec<String> = members
            .iter()
            .take(TOP)
            .map(|(user, points)| format!("{}<@{}> {}", if Some(*user) == captain { "👑 " } else { "" }, user, points))
            .collect();
        lines.push(top.join(" · "));
    }
    lines.push(match members.iter().position(|(u, _)| *u == presser) {
        Some(i) => format!("-# You: {} in {} with {} pts this month", ordinal(i + 1), h.name, members[i].1),
        None => format!("-# You haven't scored for {} this month yet", h.name),
    });
    lines.join("\n")
}

fn no_house_text(stepped_out: bool) -> String {
    if stepped_out {
        "You've stepped out of the houses, so you're a Muggle for now 🧹 Run `/houseopt` to step back into your house.".into()
    } else {
        "You're not in a house yet 🎩 The Sorting Hat sorts newcomers when they join - if it missed you, ask a mod. \
         (Muggles who stepped out with `/houseopt` can step back in the same way.)"
            .into()
    }
}

fn hour_reply(end: i64) -> String {
    let out = house::optout_set();
    let Some(db) = house::db() else {
        return "The scores couldn't be read.".into();
    };
    let rows: Vec<Scorer> = scorers(&db.lock(), end - HOUR, end).unwrap_or_default().into_iter().filter(|r| !out.contains(&r.0)).collect();
    top_text(end - HOUR, end, &rows)
}

fn today_reply(end: i64, presser: u64, now: i64) -> String {
    let out = house::optout_set();
    let day = ist_day_start(end - 1);
    let Some(db) = house::db() else {
        return "The scores couldn't be read.".into();
    };
    let rows: Vec<(u64, &'static House, i64)> = scorers(&db.lock(), day, day + DAY)
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !out.contains(&r.0))
        .map(|(u, h, t, _)| (u, h, t))
        .collect();
    let is_today = day == ist_day_start(now);
    let label = if is_today { "today".to_string() } else { ist(day).map(|t| t.format("%a %-d %b").to_string()).unwrap_or_default() };
    today_top_text(&label, is_today, &rows, presser)
}

fn house_reply(presser: u64, now: i64) -> String {
    // Every house-database read comes before the ledger lock below.
    let stepped_out = house::opted_out(presser);
    let home = house::house_of(presser);
    let Some(h) = home.filter(|_| !stepped_out) else {
        return no_house_text(stepped_out);
    };
    let out = house::optout_set();
    let captain = house::captain_id(h.key);
    let Some(db) = house::db() else {
        return "The scores couldn't be read.".into();
    };
    let conn = db.lock();
    let month = ledger::house_totals(&conn, ledger::month_start(now)).unwrap_or_default();
    let standings: Vec<(&'static House, i64)> = ranked(&month).into_iter().map(|x| (x, month.get(x.key).copied().unwrap_or(0))).collect();
    let today = ledger::house_total(&conn, h.key, ist_day_start(now), now + 1).unwrap_or(0);
    let hour_end = ist_hour_floor(now);
    let hour = ledger::house_total(&conn, h.key, hour_end - HOUR, hour_end).unwrap_or(0);
    let members: Vec<(u64, i64)> =
        ledger::top_members(&conn, h.key, ledger::month_start(now), i64::MAX).unwrap_or_default().into_iter().filter(|(u, _)| !out.contains(u)).collect();
    drop(conn);
    my_house_text(h, &standings, today, hour, &members, captain, presser)
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let presser = component.user.id.get();
    let now = Utc::now().timestamp();
    let text = match parse_press(&component.data.custom_id) {
        Some(Press::Hour(end)) => hour_reply(end),
        Some(Press::Today(end)) => today_reply(end, presser, now),
        Some(Press::House) => house_reply(presser, now),
        Some(Press::Me) => super::standings::today_for(presser, None),
        Some(Press::Earn) => rules_text::earn_text(&Rules::live()),
        None => "That button doesn't do anything any more.".into(),
    };
    let reply = CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

// --- keeping the channel in order ---------------------------------------------------------

/// The posts above the card, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Welcome,
    Cards,
    Guide,
}

impl Slot {
    const ALL: [Slot; 3] = [Slot::Welcome, Slot::Cards, Slot::Guide];

    fn key(self) -> &'static str {
        match self {
            Slot::Welcome => "welcome",
            Slot::Cards => "snitchcards",
            Slot::Guide => "guide",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Slot::Welcome => "the welcome",
            Slot::Cards => "the Snitch & cards post",
            Slot::Guide => "the guide",
        }
    }
}

enum Body {
    Text(String),
    Embeds(Vec<Panel>),
}

/// One post as it should be now.
struct Content {
    body: Body,
    images: Vec<&'static str>,
    hash: String,
}

fn panel_embed(p: &Panel) -> CreateEmbed {
    let mut e = CreateEmbed::new().description(p.body.clone()).colour(p.colour);
    if !p.title.is_empty() {
        e = e.title(p.title.clone());
    }
    if let Some(f) = &p.footer {
        e = e.footer(CreateEmbedFooter::new(f.clone()));
    }
    e
}

/// Plain text when it fits a message, otherwise one embed.
fn text_body(text: String, colour: u32) -> Body {
    if text.chars().count() <= rules_text::MESSAGE_LIMIT {
        Body::Text(text)
    } else {
        let body: String = text.chars().take(rules_text::DESCRIPTION_LIMIT).collect();
        Body::Embeds(vec![Panel { title: String::new(), body, colour, footer: None }])
    }
}

fn content(body: Body, images: Vec<&'static str>) -> Content {
    let flat = match &body {
        Body::Text(t) => format!("text\n{}", t),
        Body::Embeds(panels) => panels
            .iter()
            .map(|p| format!("{}\n{}\n{:06x}\n{}", p.title, p.body, p.colour, p.footer.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
            .join("\n---\n"),
    };
    let hash = rules_text::digest(&[&flat, &images.join(",")]);
    Content { body, images, hash }
}

/// What each slot should hold now; `None` for a post that's switched off.
fn contents(r: &Rules, welcome_on: bool, cards_on: bool, guide_on: bool) -> [Option<Content>; 3] {
    let cards = rules_text::snitch_cards_text(r).filter(|_| cards_on);
    let cards_above = cards.is_some();
    [
        welcome_on.then(|| content(text_body(rules_text::welcome_text(r), 0xE8B923), Vec::new())),
        cards.map(|t| content(text_body(t, 0xF1C40F), rules_text::snitch_cards_images(r))),
        guide_on.then(|| content(Body::Embeds(rules_text::guide(r, cards_above)), Vec::new())),
    ]
}

/// What to do with one post above the card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Keep,
    Edit,
    Delete,
    Post,
    /// Delete the old one and post it again lower down, to restore the order.
    Repost,
}

#[derive(Clone, Copy, Debug)]
struct PartState {
    wanted: bool,
    present: bool,
    changed: bool,
}

/// Works out each post's step, top to bottom, and whether the card must be
/// posted again. Once one post has to go up anew, every post below it goes up
/// again too, so the order holds.
fn plan(parts: &[PartState], force: bool) -> (Vec<Step>, bool) {
    let mut reflow = force;
    let steps = parts
        .iter()
        .map(|p| {
            if !p.wanted {
                return if p.present { Step::Delete } else { Step::Keep };
            }
            if reflow || !p.present {
                reflow = true;
                return if p.present { Step::Repost } else { Step::Post };
            }
            if p.changed { Step::Edit } else { Step::Keep }
        })
        .collect();
    (steps, reflow)
}

/// Whether a message that just landed in the channel should move the card down:
/// everything does, except the bot's own layout posts.
fn should_bump(id: u64, from_me: bool, own: &VecDeque<u64>, posting: bool) -> bool {
    !(from_me && (posting || own.contains(&id)))
}

/// How much longer to wait before moving the card, or `None` when it's time.
fn debounce_wait(now_ms: i64, last_ms: i64, delay_ms: i64) -> Option<u64> {
    let due = last_ms + delay_ms;
    (now_ms < due).then(|| (due - now_ms) as u64)
}

/// One layout change at a time: the hourly post, a move to the bottom, the
/// settings check and /guiderefresh all queue here.
static LAYOUT: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
/// Set while the bot is posting layout messages, whose ids aren't known yet.
static POSTING: AtomicBool = AtomicBool::new(false);
/// Layout messages the bot posted, newest last, so their own events are ignored.
static OWN: LazyLock<Mutex<VecDeque<u64>>> = LazyLock::new(|| Mutex::new(VecDeque::new()));
/// The layout messages live in the channel now, to notice one being deleted.
static CURRENT: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// A layout post was deleted: check the posts really exist on the next pass.
static CHECK_POSTS: AtomicBool = AtomicBool::new(false);
static LAST_SEEN_MS: AtomicI64 = AtomicI64::new(0);
static WAITING: AtomicBool = AtomicBool::new(false);

fn remember_own(id: u64) {
    let mut own = OWN.lock();
    own.push_back(id);
    while own.len() > 32 {
        own.pop_front();
    }
    drop(own);
    CURRENT.lock().insert(id);
}

fn forget(id: u64) {
    CURRENT.lock().remove(&id);
}

fn meta_id(key: &str) -> Option<u64> {
    house::meta_get(key).and_then(|v| v.parse::<u64>().ok()).filter(|id| *id != 0)
}

fn is_gone(err: &serenity::Error) -> bool {
    match err {
        serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(r)) => r.status_code.as_u16() == 404 || r.error.code == 10008,
        _ => false,
    }
}

/// Whether a message is still there; an error other than "unknown message"
/// counts as there, so a hiccup never causes a duplicate post.
async fn exists(ctx: &Context, channel: ChannelId, id: u64) -> bool {
    match ctx.http.get_message(channel, MessageId::new(id)).await {
        Ok(_) => true,
        Err(err) => !is_gone(&err),
    }
}

fn image(key: &str) -> Option<CreateAttachment> {
    match key {
        "snitch" => Some(CreateAttachment::bytes(SNITCH_HOW, "snitch-how.png")),
        "frog" => Some(CreateAttachment::bytes(FROG_HOW, "frog-how.png")),
        _ => None,
    }
}

async fn send_post(ctx: &Context, channel: ChannelId, c: &Content) -> serenity::Result<Message> {
    let mut m = CreateMessage::new().allowed_mentions(CreateAllowedMentions::new());
    m = match &c.body {
        Body::Text(t) => m.content(t.clone()),
        Body::Embeds(panels) => m.embeds(panels.iter().map(panel_embed).collect()),
    };
    for file in c.images.iter().filter_map(|k| image(k)) {
        m = m.add_file(file);
    }
    channel.send_message(&ctx.http, m).await
}

/// Edits a post in place. Its pictures stay unless the set of pictures changed.
async fn edit_post(ctx: &Context, channel: ChannelId, id: u64, c: &Content, images_changed: bool) -> serenity::Result<Message> {
    let mut e = EditMessage::new().allowed_mentions(CreateAllowedMentions::new());
    e = match &c.body {
        Body::Text(t) => e.content(t.clone()).embeds(Vec::new()),
        Body::Embeds(panels) => e.content("").embeds(panels.iter().map(panel_embed).collect()),
    };
    if images_changed {
        e = e.remove_all_attachments();
        for file in c.images.iter().filter_map(|k| image(k)) {
            e = e.new_attachment(file);
        }
    }
    channel.edit_message(&ctx.http, MessageId::new(id), e).await
}

/// Posts the card for the hour ending at `end` at the bottom of the channel.
/// Call holding `LAYOUT` with `POSTING` set. `advance` marks the hour as posted
/// (the hourly post); a move to the bottom leaves that alone.
async fn send_card(ctx: &Context, channel: ChannelId, end: i64, delete_old: bool, advance: bool) -> bool {
    let (first, last, every) = schedule();
    let old = meta_id("scoreboard_message");
    let data = {
        let Some(db) = house::db() else {
            return false;
        };
        let conn = db.lock();
        card_data(&conn, end)
    };
    let data = CardData { next: next_post(Utc::now().timestamp(), first, last, every), ..data };
    let message = CreateMessage::new().embed(card(&data)).components(vec![buttons(end)]).allowed_mentions(CreateAllowedMentions::new());
    match channel.send_message(&ctx.http, message).await {
        Ok(sent) => {
            remember_own(sent.id.get());
            house::meta_set("scoreboard_message", &sent.id.get().to_string());
            if advance {
                house::meta_set("scoreboard_last", &end.to_string());
            }
            if let Some(old) = old.filter(|_| delete_old) {
                forget(old);
                let _ = channel.delete_message(&ctx.http, MessageId::new(old)).await;
            }
            true
        }
        Err(err) => {
            tracing::warn!("scoreboard: card not posted: {}", err);
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// After a start: check every post really exists.
    Startup,
    /// The minute check: edit posts whose words changed, post missing ones.
    Poll,
    /// Something landed in the channel: move the card to the bottom.
    Bump,
    /// /guiderefresh: post everything again, in order.
    Refresh,
}

/// Brings the channel into order. Returns what was done, for /guiderefresh.
async fn arrange(ctx: &Context, mode: Mode) -> Result<String, String> {
    let Some(channel) = channel() else {
        return Err("No scoreboard channel is set (VIZIER_SCOREBOARD_CHANNEL).".into());
    };
    let _guard = LAYOUT.lock().await;
    let rules = Rules::live();
    let wanted = contents(
        &rules,
        super::control::on("VIZIER_SCOREBOARD_WELCOME", true),
        super::control::on("VIZIER_SCOREBOARD_SNITCH_CARDS", true),
        super::control::on("VIZIER_SCOREBOARD_GUIDE", true),
    );
    let card_on = super::control::on("VIZIER_SCOREBOARD", true);

    // A new channel starts from nothing; the old channel's posts are removed.
    let here = channel.get().to_string();
    let moved = house::meta_get("scoreboard_channel").is_some_and(|c| c != here);
    let mut ids: Vec<Option<u64>> = Slot::ALL.iter().map(|s| meta_id(&format!("{}_message", s.key()))).collect();
    let mut card_id = meta_id("scoreboard_message");
    if moved {
        if let Some(old) = house::meta_get("scoreboard_channel").and_then(|c| c.parse::<u64>().ok()).filter(|c| *c != 0).map(ChannelId::new) {
            for id in ids.iter().flatten().chain(card_id.iter()) {
                forget(*id);
                let _ = old.delete_message(&ctx.http, MessageId::new(*id)).await;
            }
        }
        ids = vec![None; 3];
        card_id = None;
        house::meta_set("scoreboard_message", "");
    }
    let hashes: Vec<Option<String>> = Slot::ALL.iter().map(|s| house::meta_get(&format!("{}_hash", s.key()))).collect();
    let images: Vec<String> = Slot::ALL.iter().map(|s| house::meta_get(&format!("{}_images", s.key())).unwrap_or_default()).collect();
    {
        let mut current = CURRENT.lock();
        current.extend(ids.iter().flatten().chain(card_id.iter()).copied());
    }

    let verify = mode == Mode::Startup || CHECK_POSTS.swap(false, Ordering::SeqCst);
    let mut present: Vec<bool> = ids.iter().map(|id| id.is_some()).collect();
    if verify && mode != Mode::Refresh {
        for (i, id) in ids.iter().enumerate() {
            if let Some(id) = id {
                present[i] = exists(ctx, channel, *id).await;
            }
        }
    }
    let states: Vec<PartState> = (0..3)
        .map(|i| PartState {
            wanted: wanted[i].is_some(),
            present: present[i],
            changed: wanted[i].as_ref().is_some_and(|c| hashes[i].as_deref() != Some(c.hash.as_str())),
        })
        .collect();
    let (steps, mut card_again) = plan(&states, mode == Mode::Refresh);
    if card_on && !card_again {
        card_again = match (mode, card_id) {
            (_, None) => true,
            (Mode::Startup, Some(id)) => !exists(ctx, channel, id).await,
            (Mode::Bump, Some(id)) => match channel.messages(&ctx.http, GetMessages::new().limit(1)).await {
                Ok(latest) => latest.first().map(|m| m.id.get()) != Some(id),
                Err(_) => true,
            },
            _ => false,
        };
    }
    let card_again = card_again && card_on;
    if steps.iter().all(|s| *s == Step::Keep) && !card_again {
        house::meta_set("scoreboard_channel", &here);
        return Ok("Everything in the channel is already up to date.".into());
    }

    POSTING.store(true, Ordering::SeqCst);
    let mut done: Vec<String> = Vec::new();
    // Removals first, so nothing old is left sitting between the new posts.
    for (i, step) in steps.iter().enumerate() {
        if matches!(step, Step::Delete | Step::Repost) {
            if let Some(id) = ids[i] {
                forget(id);
                let _ = channel.delete_message(&ctx.http, MessageId::new(id)).await;
            }
            if *step == Step::Delete {
                let key = Slot::ALL[i].key();
                house::meta_set(&format!("{}_message", key), "");
                house::meta_set(&format!("{}_hash", key), "");
                done.push(format!("removed {}", Slot::ALL[i].name()));
            }
        }
    }
    for (i, step) in steps.iter().enumerate() {
        let slot = Slot::ALL[i];
        let Some(c) = wanted[i].as_ref() else { continue };
        let key = slot.key();
        match step {
            Step::Edit => {
                let Some(id) = ids[i] else { continue };
                match edit_post(ctx, channel, id, c, images[i] != c.images.join(",")).await {
                    Ok(_) => {
                        house::meta_set(&format!("{}_hash", key), &c.hash);
                        house::meta_set(&format!("{}_images", key), &c.images.join(","));
                        done.push(format!("updated {}", slot.name()));
                    }
                    Err(err) if is_gone(&err) => {
                        // Deleted meanwhile: the next pass posts it again, in order.
                        CHECK_POSTS.store(true, Ordering::SeqCst);
                        bump(ctx.clone());
                    }
                    Err(err) => tracing::warn!("scoreboard: {} not edited: {}", slot.name(), err),
                }
            }
            Step::Post | Step::Repost => match send_post(ctx, channel, c).await {
                Ok(sent) => {
                    if let Some(old) = ids[i] {
                        forget(old);
                    }
                    remember_own(sent.id.get());
                    house::meta_set(&format!("{}_message", key), &sent.id.get().to_string());
                    house::meta_set(&format!("{}_hash", key), &c.hash);
                    house::meta_set(&format!("{}_images", key), &c.images.join(","));
                    done.push(format!("posted {}", slot.name()));
                }
                Err(err) => tracing::warn!("scoreboard: {} not posted: {}", slot.name(), err),
            },
            Step::Keep | Step::Delete => {}
        }
    }
    if card_again && send_card(ctx, channel, ist_hour_floor(Utc::now().timestamp()), true, false).await {
        done.push("posted the scoreboard".into());
    }
    POSTING.store(false, Ordering::SeqCst);
    house::meta_set("scoreboard_channel", &here);
    if !done.is_empty() {
        tracing::info!("scoreboard: {:?} in {}: {}", mode, channel, done.join(", "));
    }
    Ok(if done.is_empty() { "Nothing could be posted - check the bot's permissions in that channel.".into() } else { format!("Done in <#{}>: {}.", channel, done.join(", ")) })
}

/// Moves the card to the bottom a few seconds after the last post lands.
fn bump(ctx: Context) {
    LAST_SEEN_MS.store(Utc::now().timestamp_millis(), Ordering::SeqCst);
    if WAITING.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        while let Some(ms) = debounce_wait(Utc::now().timestamp_millis(), LAST_SEEN_MS.load(Ordering::SeqCst), BUMP_DELAY_MS) {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        // Anything landing from here on starts its own wait; this pass still
        // covers whatever came before it.
        WAITING.store(false, Ordering::SeqCst);
        if let Err(err) = arrange(&ctx, Mode::Bump).await {
            tracing::debug!("scoreboard: not moved: {}", err);
        }
    });
}

/// Every message the bot sees, bots' included: anything new in the scoreboard
/// channel that isn't the bot's own layout post moves the card back down.
pub fn on_message(ctx: &Context, msg: &Message) {
    let Some(here) = channel() else { return };
    if msg.channel_id != here || !super::control::on("VIZIER_SCOREBOARD", true) {
        return;
    }
    let from_me = msg.author.id == ctx.cache.current_user().id;
    if should_bump(msg.id.get(), from_me, &OWN.lock(), POSTING.load(Ordering::SeqCst)) {
        bump(ctx.clone());
    }
}

/// A deleted layout post is put back (and the card moved below it).
pub fn on_delete(ctx: &Context, channel_id: ChannelId, id: MessageId) {
    if channel() != Some(channel_id) || !CURRENT.lock().contains(&id.get()) {
        return;
    }
    forget(id.get());
    CHECK_POSTS.store(true, Ordering::SeqCst);
    bump(ctx.clone());
}

/// Posts the card for the hour that just ended, if it's due.
async fn post(ctx: &Context, end: i64) {
    if !super::control::on("VIZIER_SCOREBOARD", true) {
        return;
    }
    let Some(channel) = channel() else {
        return;
    };
    let (first, last, every) = schedule();
    let hour = ist_hour(end - HOUR);
    if !(first..=last).contains(&hour) || (hour - first).rem_euclid(every) != 0 {
        return;
    }
    if house::meta_get("scoreboard_last").and_then(|v| v.parse::<i64>().ok()).is_some_and(|done| done >= end) {
        return;
    }
    let quiet = {
        let Some(db) = house::db() else {
            return;
        };
        let conn = db.lock();
        ledger::by_source(&conn, end - HOUR, end).unwrap_or_default().values().all(|n| *n == 0)
    };
    if super::control::on("VIZIER_SCOREBOARD_QUIET_SKIP", true) && quiet {
        house::meta_set("scoreboard_last", &end.to_string());
        return;
    }
    let replace = super::control::on("VIZIER_SCOREBOARD_REPLACE", true);
    let _guard = LAYOUT.lock().await;
    POSTING.store(true, Ordering::SeqCst);
    send_card(ctx, channel, end, replace, true).await;
    POSTING.store(false, Ordering::SeqCst);
}

/// Wakes at the turn of each India hour for the card, and every minute to keep
/// the posts above it matching the settings.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hourly = ctx.clone();
    tokio::spawn(async move {
        loop {
            let now = Utc::now().timestamp();
            let next = ist_hour_floor(now) + HOUR;
            tokio::time::sleep(Duration::from_secs((next - now + 10).max(1) as u64)).await;
            post(&hourly, ist_hour_floor(Utc::now().timestamp())).await;
        }
    });
    tokio::spawn(async move {
        // Let the cache settle after connecting.
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut mode = Mode::Startup;
        loop {
            match arrange(&ctx, mode).await {
                Ok(_) => mode = Mode::Poll,
                // No channel yet: check properly once one is set.
                Err(_) => mode = Mode::Startup,
            }
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        }
    });
}

pub fn refresh_builder() -> CreateCommand {
    CreateCommand::new("guiderefresh").description("admin only: post the House Cup welcome, rules and scoreboard again, in order")
}

/// `/guiderefresh`: every post in the scoreboard channel goes up again, in order.
pub async fn refresh_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let reply = CreateInteractionResponseMessage::new().content("Only bot admins can do that.").ephemeral(true);
        let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
        return;
    }
    let _ = command.defer_ephemeral(&ctx.http).await;
    let text = arrange(ctx, Mode::Refresh).await.unwrap_or_else(|e| e);
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(text)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-14 00:00 India time.
    const MIDNIGHT: i64 = 1_789_324_200;

    fn h(key: &str) -> &'static House {
        HOUSES.iter().find(|h| h.key == key).unwrap()
    }

    fn data(end: i64) -> CardData {
        CardData {
            end,
            month: [("ravenclaw", 663), ("gryffindor", 593), ("slytherin", 321), ("hufflepuff", 310)].into_iter().collect(),
            hour: [("slytherin", 12), ("ravenclaw", 4), ("gryffindor", 9)].into_iter().collect(),
            today: [("ravenclaw", 84), ("gryffindor", 71), ("slytherin", 66), ("hufflepuff", 40)].into_iter().collect(),
            sources: vec![(Source::Quiz, 11), (Source::Frog, 8), (Source::Chat, 5)],
            next: Some(end + HOUR),
        }
    }

    #[test]
    fn bars_and_numbers_read_well() {
        assert_eq!(bar(100, 100), "█".repeat(12));
        assert_eq!(bar(50, 100), format!("{}{}", "█".repeat(6), "░".repeat(6)));
        assert_eq!(bar(0, 0), "░".repeat(12));
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(hour12(ist_hour(MIDNIGHT + 19 * HOUR)), "7pm");
        assert_eq!(clock(MIDNIGHT + 20 * HOUR), "8:00 pm");
        assert_eq!([1, 2, 3, 4, 11, 12, 13, 21, 22, 103].map(ordinal), ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "103rd"]);
    }

    #[test]
    fn days_left_counts_the_india_month() {
        // 15 September: fifteen days after today.
        assert_eq!(days_left(MIDNIGHT + DAY + 12 * HOUR), 15);
        // 30 September, 23:59 India time, is the last day; a minute later is 1 October.
        let oct1 = MIDNIGHT + 17 * DAY;
        assert_eq!(days_left(oct1 - 60), 0);
        assert_eq!(days_left(oct1), 30);
        assert_eq!(days_words(0), "last day of the month");
        assert_eq!(days_words(1), "1 day to go");
        assert_eq!(days_left(1_798_655_400 + 3600), 0, "31 December is the last day");
        assert_eq!(days_left(1_798_655_400 + DAY), 30, "and 1 January starts a new month");
    }

    #[test]
    fn the_card_ranks_houses_and_fills_its_fields() {
        let end = MIDNIGHT + DAY + 20 * HOUR; // 15 Sep, 8pm
        let json = serde_json::to_value(card(&data(end))).unwrap();
        let text = json["description"].as_str().unwrap();
        assert!(text.find("Ravenclaw").unwrap() < text.find("Gryffindor").unwrap(), "{}", text);
        assert!(text.starts_with("🦅 **Ravenclaw** lead by **70** · 15 days to go"), "{}", text);
        assert!(text.contains("**+12** this hour"), "{}", text);
        assert!(!text.contains("Hufflepuff** — **310** pts ·"), "no gain, no gain line: {}", text);
        assert_eq!(json["title"], "🏆 House Cup · September");
        assert_eq!(json["color"], h("ravenclaw").colour);
        let fields = json["fields"].as_array().unwrap();
        let field = |name: &str| fields.iter().find(|f| f["name"] == name).unwrap()["value"].as_str().unwrap().to_string();
        assert_eq!(field("🔥 Hottest this hour"), "🐍 Slytherin +12");
        assert_eq!(field("📅 Today so far"), "🦅 84 · 🦁 71 · 🐍 66 · 🦡 40");
        assert_eq!(field("⚡ Last hour's points from"), "🧠 Quiz 11 · 🐸 Chocolate Frog 8 · 💬 Chat 5");
        assert_eq!(field("⏰ Next update"), format!("<t:{0}:R> (<t:{0}:t>)", end + HOUR));
        assert!(fields.iter().filter(|f| f["inline"] == true).count() == 6);
        assert_eq!(json["footer"]["text"], "Updated 8:00 pm India time · always the last message here");

        let quiet = CardData { hour: HashMap::new(), sources: Vec::new(), month: [("ravenclaw", 5), ("gryffindor", 5)].into_iter().collect(), ..data(MIDNIGHT + 17 * DAY - HOUR) };
        let json = serde_json::to_value(card(&quiet)).unwrap();
        assert!(json["description"].as_str().unwrap().starts_with("It's neck and neck at the top · last day of the month"), "{}", json["description"]);
        let fields = json["fields"].as_array().unwrap();
        assert_eq!(fields[0]["value"], "Quiet hour");
        assert_eq!(fields[3]["value"], "Nothing this hour");
    }

    #[test]
    fn the_hours_sources_add_up_across_houses() {
        let moved: HashMap<(&'static str, Source), i64> =
            [(("ravenclaw", Source::Quiz), 6), (("slytherin", Source::Quiz), 5), (("ravenclaw", Source::Chat), 5), (("gryffindor", Source::Frog), 8), (("hufflepuff", Source::Koto), 2), (("slytherin", Source::Mod), -20)]
                .into_iter()
                .collect();
        assert_eq!(top_sources(&moved), vec![(Source::Quiz, 11), (Source::Frog, 8), (Source::Chat, 5)]);
    }

    #[test]
    fn the_next_update_follows_the_schedule() {
        let at = |hour: i64, min: i64| MIDNIGHT + hour * HOUR + min * 60;
        assert_eq!(next_post(at(19, 20), 10, 23, 1), Some(at(20, 0)));
        // Every 2 hours from 10: covers 10, 12, ... so posts at 11, 13, ... 21, 23.
        assert_eq!(next_post(at(19, 20), 10, 23, 2), Some(at(21, 0)));
        // After the last hour, the next is tomorrow's first.
        assert_eq!(next_post(at(23, 30), 10, 22, 1), Some(at(35, 0)));
        assert_eq!(next_post(at(1, 0), 5, 3, 1), None);
    }

    #[test]
    fn button_ids_carry_the_cards_hour() {
        assert_eq!(parse_press("scoreboard:hour:1789400000"), Some(Press::Hour(1_789_400_000)));
        assert_eq!(parse_press("scoreboard:top:17"), Some(Press::Hour(17)), "old cards still work");
        assert_eq!(parse_press("scoreboard:today:18"), Some(Press::Today(18)));
        assert_eq!(parse_press("scoreboard:house"), Some(Press::House));
        assert_eq!(parse_press("scoreboard:me"), Some(Press::Me));
        assert_eq!(parse_press("scoreboard:earn"), Some(Press::Earn));
        assert_eq!(parse_press("scoreboard:today:x"), None);
        assert_eq!(parse_press("scoreboard:hour"), None);
        assert_eq!(parse_press("quiz:hour:1"), None);
        let json = serde_json::to_value(buttons(42)).unwrap();
        let ids: Vec<&str> = json["components"].as_array().unwrap().iter().map(|b| b["custom_id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["scoreboard:hour:42", "scoreboard:today:42", "scoreboard:house", "scoreboard:me", "scoreboard:earn"]);
        for id in ids {
            assert!(parse_press(id).is_some(), "{}", id);
        }
    }

    #[test]
    fn the_top_list_names_the_best_first_with_their_sources() {
        let rows = vec![(7, h("gryffindor"), 9, vec![(Source::Quiz, 6), (Source::Chat, 3)])];
        let text = top_text(MIDNIGHT + 19 * HOUR, MIDNIGHT + 20 * HOUR, &rows);
        assert!(text.contains("🥇 <@7>") && text.contains("**9** pts") && text.contains("7pm – 8pm"), "{}", text);
        assert!(top_text(0, HOUR, &[]).contains("Nobody scored"));
    }

    #[test]
    fn top_today_ranks_and_places_the_presser() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(ledger::SCHEMA).unwrap();
        let day = MIDNIGHT;
        let put = |user: u64, key: &str, points: i64, ts: i64| {
            conn.execute(
                "INSERT INTO ledger (user_id, house, source, points, day, ts) VALUES (?1, ?2, 'mod', ?3, '2026-09-14', ?4)",
                params![user as i64, key, points, ts],
            )
            .unwrap();
        };
        put(1, "gryffindor", 10, day + HOUR);
        put(2, "slytherin", 10, day + 2 * HOUR); // same points, finished later
        put(3, "ravenclaw", 15, day + 3 * HOUR);
        put(4, "hufflepuff", 4, day + 4 * HOUR);
        put(5, "hufflepuff", 50, day - HOUR); // yesterday
        let rows: Vec<(u64, &'static House, i64)> = scorers(&conn, day, day + DAY).unwrap().into_iter().map(|(u, h, t, _)| (u, h, t)).collect();
        assert_eq!(rows.iter().map(|r| r.0).collect::<Vec<_>>(), vec![3, 1, 2, 4]);

        let text = today_top_text("today", true, &rows, 4);
        assert!(text.starts_with("📅 **Top scorers · today**\n🥇 <@3> 🦅 **15**\n🥈 <@1> 🦁 **10**"), "{}", text);
        assert!(text.contains("` 4.` <@4> 🦡 **4**"), "{}", text);
        assert!(text.ends_with("-# You: 4th with 4 pts — 6 behind 3rd"), "{}", text);
        assert!(today_top_text("today", true, &rows, 2).ends_with("-# You: 3rd with 10 pts — level with 2nd"));
        assert!(today_top_text("today", true, &rows, 3).ends_with("-# You: 1st with 15 pts — 5 ahead of 2nd"));
        assert!(today_top_text("today", true, &rows, 99).ends_with("-# You haven't scored today yet"));
        assert!(today_top_text("Sun 13 Sep", false, &[], 99).contains("Nobody has scored yet.\n-# You haven't scored that day yet"));
        let many: Vec<(u64, &'static House, i64)> = (1..=14).map(|i| (i, h("slytherin"), 100 - i as i64)).collect();
        let text = today_top_text("today", true, &many, 12);
        assert!(text.contains("`10.` <@10>") && !text.contains("<@11>"), "{}", text);
        assert!(text.ends_with("-# You: 12th with 88 pts — 1 behind 11th"), "{}", text);
    }

    #[test]
    fn my_house_shows_the_race_the_day_and_the_members() {
        let standings = vec![(h("ravenclaw"), 663), (h("gryffindor"), 593), (h("slytherin"), 321), (h("hufflepuff"), 310)];
        let members = vec![(11, 142), (12, 118), (13, 97), (14, 80), (15, 64), (16, 58)];
        let text = my_house_text(h("gryffindor"), &standings, 71, 9, &members, Some(11), 16);
        assert!(text.starts_with("🦁 **Gryffindor** · 2nd · **70** behind 🦅 Ravenclaw\nToday **+71** · this hour **+9**\n**Top this month**"), "{}", text);
        assert!(text.contains("👑 <@11> 142 · <@12> 118 · <@13> 97 · <@14> 80 · <@15> 64"), "{}", text);
        assert!(!text.contains("<@16> 58"), "only the top five are listed: {}", text);
        assert!(text.ends_with("-# You: 6th in Gryffindor with 58 pts this month"), "{}", text);

        let lead = my_house_text(h("ravenclaw"), &standings, 0, -2, &[], None, 1);
        assert!(lead.contains("🦅 **Ravenclaw** · 1st · leading by **70**"), "{}", lead);
        assert!(lead.contains("Today **+0** · this hour **-2**") && lead.contains("Nobody has scored yet."), "{}", lead);
        assert!(lead.ends_with("-# You haven't scored for Ravenclaw this month yet"));
        assert!(no_house_text(true).contains("Muggle") && no_house_text(false).contains("Sorting Hat"));
    }

    #[test]
    fn a_missing_post_takes_everything_below_it_along() {
        let s = |wanted, present, changed| PartState { wanted, present, changed };
        // All there, nothing changed.
        assert_eq!(plan(&[s(true, true, false), s(true, true, false), s(true, true, false)], false), (vec![Step::Keep; 3], false));
        // Words changed: edited in place, the card stays.
        assert_eq!(plan(&[s(true, true, true), s(true, true, false), s(true, true, true)], false), (vec![Step::Edit, Step::Keep, Step::Edit], false));
        // The cards post was deleted: it and the guide go up again, then the card.
        assert_eq!(plan(&[s(true, true, true), s(true, false, false), s(true, true, true)], false), (vec![Step::Edit, Step::Post, Step::Repost], true));
        // The welcome switched on later: everything below it moves down.
        assert_eq!(plan(&[s(true, false, true), s(true, true, false), s(true, true, false)], false), (vec![Step::Post, Step::Repost, Step::Repost], true));
        // Switched off: removed, and the order below is unaffected.
        assert_eq!(plan(&[s(false, true, false), s(false, false, false), s(true, true, false)], false), (vec![Step::Delete, Step::Keep, Step::Keep], false));
        // /guiderefresh: everything again.
        assert_eq!(plan(&[s(true, true, false), s(false, true, false), s(true, false, false)], true), (vec![Step::Repost, Step::Delete, Step::Post], true));
    }

    #[test]
    fn only_other_messages_move_the_card_and_bursts_move_it_once() {
        let own: VecDeque<u64> = [10, 11].into_iter().collect();
        assert!(should_bump(5, false, &own, false), "an admin's post");
        assert!(should_bump(10, false, &own, true), "someone else's message is never the bot's own");
        assert!(should_bump(12, true, &own, false), "another bot post, like a frog drop or a lead card");
        assert!(!should_bump(10, true, &own, false), "the card itself");
        assert!(!should_bump(13, true, &own, true), "a layout post whose id isn't known yet");

        assert_eq!(debounce_wait(1_000, 1_000, 5_000), Some(5_000));
        assert_eq!(debounce_wait(4_000, 1_000, 5_000), Some(2_000));
        // A second post at 4s pushes the move back to 9s.
        assert_eq!(debounce_wait(6_000, 4_000, 5_000), Some(3_000));
        assert_eq!(debounce_wait(9_000, 4_000, 5_000), None);
    }

    #[test]
    fn posts_fall_back_to_an_embed_and_hash_their_pictures() {
        assert!(matches!(text_body("short".into(), 0), Body::Text(_)));
        match text_body("x".repeat(2500), 7) {
            Body::Embeds(p) => assert!(p.len() == 1 && p[0].title.is_empty() && p[0].body.len() == 2500 && p[0].colour == 7),
            Body::Text(_) => panic!("too long for a message"),
        }
        let a = content(Body::Text("same".into()), vec!["snitch", "frog"]);
        let b = content(Body::Text("same".into()), vec!["snitch"]);
        assert_ne!(a.hash, b.hash, "switching frogs off changes the pictures, so the post changes");
        let r = rules_text::tests::defaults();
        let posts = contents(&r, true, true, true);
        assert!(posts.iter().all(|p| p.is_some()));
        assert!(matches!(posts[0].as_ref().unwrap().body, Body::Text(_)));
        assert_eq!(posts[1].as_ref().unwrap().images, vec!["snitch", "frog"]);
        match &posts[2].as_ref().unwrap().body {
            Body::Embeds(p) => assert!(p[0].body.contains("post above")),
            Body::Text(_) => panic!("the guide is embeds"),
        }
        let no_cards = contents(&r, false, false, true);
        assert!(no_cards[0].is_none() && no_cards[1].is_none());
        match &no_cards[2].as_ref().unwrap().body {
            Body::Embeds(p) => assert!(!p[0].body.contains("post above")),
            Body::Text(_) => panic!("the guide is embeds"),
        }
        assert!(!SNITCH_HOW.is_empty() && FROG_HOW.starts_with(b"\x89PNG"));
    }
}
