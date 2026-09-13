//! Where the houses stand: the hourly summary, `/mypoints`, and the monthly
//! Nitro draw. Nothing here awards points - it only reads the ledger.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, Context, CreateAllowedMentions, CreateCommand,
    CreateCommandOption, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
};

use super::house::{self, HOUSES, House};
use super::points::{self as ledger, DrawResult, Source};

/// India is 5h30 ahead of UTC, so its hours start at half past a UTC hour.
const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;

/// The hourly summary covers hours starting 10:00 through 23:00 India time: the
/// 10-11 summary goes out at 11:00, the last one at midnight. Nothing overnight.
const FIRST_HOUR: i64 = 10;
const LAST_HOUR: i64 = 23;

fn houses_channel() -> Option<ChannelId> {
    std::env::var("VIZIER_HOUSE_CHANNEL").ok()?.trim().parse().ok().map(ChannelId::new)
}

/// The start of the India hour a moment falls in.
fn ist_hour_floor(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(3600) * 3600 - IST_OFFSET
}

/// The India hour of the day (0-23) a moment falls in.
fn ist_hour(ts: i64) -> i64 {
    (ts + IST_OFFSET).rem_euclid(86_400) / 3600
}

/// "7pm", "12am", "12pm".
fn hour12(hour: i64) -> String {
    match hour.rem_euclid(24) {
        0 => "12am".into(),
        12 => "12pm".into(),
        h if h < 12 => format!("{}am", h),
        h => format!("{}pm", h - 12),
    }
}

fn signed(n: i64) -> String {
    if n >= 0 { format!("+{}", n) } else { n.to_string() }
}

/// The summary message for one hour, or `None` if no house's points moved.
///
/// House-level only - no names - so it reads as a scoreboard, not a shout-out.
fn summary_text(
    start_hour: i64,
    moved: &HashMap<(&'static str, Source), i64>,
    month: &HashMap<&'static str, i64>,
) -> Option<String> {
    let mut gains: Vec<(&'static House, i64, Vec<(Source, i64)>)> = HOUSES
        .iter()
        .filter_map(|h| {
            let mut sources: Vec<(Source, i64)> =
                Source::ALL.iter().filter_map(|s| moved.get(&(h.key, *s)).map(|n| (*s, *n))).collect();
            if sources.is_empty() {
                return None;
            }
            sources.sort_by(|a, b| b.1.cmp(&a.1));
            Some((h, sources.iter().map(|(_, n)| n).sum(), sources))
        })
        .collect();
    if gains.is_empty() {
        return None;
    }
    gains.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.name.cmp(b.0.name)));

    let mut text = format!("📊 **House points · {} – {}**\n", hour12(start_hour), hour12(start_hour + 1));
    for (h, total, sources) in gains {
        let parts: Vec<String> = sources.iter().map(|(s, n)| format!("{} {}", s.label(), signed(*n))).collect();
        text.push_str(&format!("{} **{} {}** — {}\n", h.crest, h.name, signed(total), parts.join(" · ")));
    }
    let mut standing: Vec<&House> = HOUSES.iter().collect();
    standing.sort_by(|a, b| month.get(b.key).cmp(&month.get(a.key)).then(a.name.cmp(b.name)));
    let table: Vec<String> =
        standing.iter().map(|h| format!("{} {}", h.crest, month.get(h.key).copied().unwrap_or(0))).collect();
    text.push_str(&format!("-# This month: {}", table.join(" · ")));
    Some(text)
}

/// Posts the hourly summary at the turn of each India hour, stacking a new
/// message each time (the owner's call) and only when points actually moved.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let now = Utc::now().timestamp();
            let next = ist_hour_floor(now) + 3600;
            // A few seconds past the hour, so the last minute's points are in.
            tokio::time::sleep(Duration::from_secs((next - now + 5).max(1) as u64)).await;
            let end = ist_hour_floor(Utc::now().timestamp());
            post_hour(&ctx, end - 3600, end).await;
        }
    });
}

async fn post_hour(ctx: &Context, start: i64, end: i64) {
    let hour = ist_hour(start);
    if !(FIRST_HOUR..=LAST_HOUR).contains(&hour) {
        return;
    }
    // Once per hour even across a restart: remember the last hour posted.
    if house::meta_get("summary_last").and_then(|v| v.parse::<i64>().ok()).is_some_and(|last| last >= end) {
        return;
    }
    let Some(channel) = houses_channel() else {
        return;
    };
    let text = {
        let Some(db) = house::db() else {
            return;
        };
        let conn = db.lock();
        let moved = ledger::by_source(&conn, start, end).unwrap_or_default();
        let month = ledger::house_totals(&conn, ledger::month_start(end - 1)).unwrap_or_default();
        summary_text(hour, &moved, &month)
    };
    let Some(text) = text else {
        return;
    };
    let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    match channel.send_message(&ctx.http, message).await {
        Ok(_) => house::meta_set("summary_last", &end.to_string()),
        Err(err) => tracing::warn!("standings: hourly summary not posted: {}", err),
    }
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

pub fn mypoints_builder() -> CreateCommand {
    CreateCommand::new("mypoints").description("your house points this month, and where they came from")
}

/// What `/mypoints` says for someone in a house.
fn mypoints_text(h: &House, breakdown: &[(Source, i64)]) -> String {
    let total: i64 = breakdown.iter().map(|(_, n)| n).sum();
    let mut text = format!("{} **{}** · this month you've earned **{}** points", h.crest, h.name, total);
    if !breakdown.is_empty() {
        let parts: Vec<String> = breakdown.iter().map(|(s, n)| format!("{} {}", s.label(), n)).collect();
        text.push_str(&format!("\n{}", parts.join(" · ")));
    }
    let short = ledger::DRAW_MINIMUM - total;
    text.push_str(&if short > 0 {
        format!("\n-# {} more to be in this month's Nitro draw.", short)
    } else {
        "\n-# You're in this month's Nitro draw if your house wins.".into()
    });
    text
}

/// `/mypoints` - private to whoever asks.
pub async fn mypoints_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let text = if house::opted_out(user) {
        "You've stepped out of the houses, so you're not earning points. Run `/houseopt` to step back in.".to_string()
    } else if let Some(h) = house::house_of(user) {
        let since = ledger::month_start(Utc::now().timestamp());
        let breakdown = house::db().and_then(|db| ledger::breakdown(&db.lock(), user, since).ok()).unwrap_or_default();
        mypoints_text(h, &breakdown)
    } else {
        "You're not in a house yet.".to_string()
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

pub fn draw_builder() -> CreateCommand {
    CreateCommand::new("housedraw")
        .description("admin only: last month's winning house and its random Nitro winner")
        .add_option(CreateCommandOption::new(
            serenity::all::CommandOptionType::Boolean,
            "redraw",
            "draw the random winner again (it's recorded that you did)",
        ))
}

/// The draw, written up for the mod who ran it.
fn draw_text(month_label: &str, result: &DrawResult) -> String {
    match result {
        DrawResult::Empty => format!("No house earned any points in {}.", month_label),
        DrawResult::Tie(keys) => {
            let names: Vec<String> =
                keys.iter().filter_map(|k| house::house(k)).map(|h| format!("{} {}", h.crest, h.name)).collect();
            format!("**{}** ended in a tie at the top: {}. That one's a call for the mods.", month_label, names.join(", "))
        }
        DrawResult::Winner { house: key, points, captain, member, pool } => {
            let h = house::house(key);
            let name = h.map(|h| format!("{} {}", h.crest, h.name)).unwrap_or_else(|| key.to_string());
            let captain = captain.map(|c| format!("<@{}>", c)).unwrap_or_else(|| "no captain named".into());
            let member = match member {
                Some(m) => format!("<@{}> (drawn from {} eligible)", m, pool),
                None => "nobody - no one else in the house reached 10 points".into(),
            };
            format!(
                "🏆 **{}** won **{}** with **{}** points.\n🎁 Nitro to the captain: {}\n🎲 Nitro to a random member: {}",
                month_label, name, points, captain, member
            )
        }
    }
}

/// `/housedraw` - mods only. Draws once per month and remembers the result, so
/// running it again shows the same winner. A redraw is possible, but it says so.
pub async fn draw_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can run the draw.")).await;
        return;
    }
    let redraw = command.data.options.iter().any(|o| matches!(o.value, CommandDataOptionValue::Boolean(true)));
    let now = Utc::now().timestamp();
    let this_month = ledger::month_start(now);
    let last_month = ledger::month_start(this_month - 1);
    let day = ledger::ist_day(last_month);
    let month_key = day.get(..7).unwrap_or(&day).to_string();
    let label = month_label(&day);
    let saved_key = format!("draw_{}", month_key);

    if !redraw {
        if let Some(saved) = house::meta_get(&saved_key) {
            let text = format!("{}\n-# Already drawn. Use `redraw` to draw the random winner again.", saved);
            let _ = command.create_response(&ctx.http, whisper(text)).await;
            return;
        }
    }

    // Read these BEFORE taking the database lock: both read the database too, and
    // the lock can't be taken twice by the same caller.
    let optouts = house::optout_set();
    let captains: HashMap<&'static str, Option<u64>> = HOUSES.iter().map(|h| (h.key, house::captain_id(h.key))).collect();
    let result = {
        let Some(db) = house::db() else {
            let _ = command.create_response(&ctx.http, whisper("The points database isn't open.")).await;
            return;
        };
        let conn = db.lock();
        ledger::draw(
            &conn,
            last_month,
            this_month,
            |h| captains.get(h.key).copied().flatten(),
            |user| !optouts.contains(&user),
            rand::random::<u64>(),
        )
    };
    let mut text = match result {
        Ok(result) => draw_text(&label, &result),
        Err(err) => {
            tracing::warn!("standings: draw failed: {}", err);
            let _ = command.create_response(&ctx.http, whisper("The draw failed - try again.")).await;
            return;
        }
    };
    if redraw {
        text.push_str(&format!("\n-# Redrawn by <@{}>.", command.user.id.get()));
    }
    house::meta_set(&saved_key, &text);
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

/// "2026-08-01" -> "August 2026".
fn month_label(day: &str) -> String {
    const NAMES: [&str; 12] = [
        "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
        "December",
    ];
    let mut parts = day.split('-');
    match (parts.next(), parts.next().and_then(|m| m.parse::<usize>().ok())) {
        (Some(year), Some(month)) if (1..=12).contains(&month) => format!("{} {}", NAMES[month - 1], year),
        _ => day.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn india_hours_start_at_half_past_a_utc_hour() {
        // 2026-09-14 13:45 UTC is 19:15 India time, in the 7pm hour.
        let ts = 1_789_393_500;
        assert_eq!(ist_hour(ts), 19);
        assert_eq!(ist_hour(ist_hour_floor(ts)), 19);
        assert_eq!((ts - ist_hour_floor(ts)), 15 * 60);
        assert_eq!(hour12(0), "12am");
        assert_eq!(hour12(12), "12pm");
        assert_eq!(hour12(19), "7pm");
        assert_eq!(hour12(24), "12am");
    }

    #[test]
    fn a_quiet_hour_posts_nothing_and_a_busy_one_leads_with_the_biggest_gain() {
        let month: HashMap<&'static str, i64> = [("gryffindor", 100), ("ravenclaw", 140)].into_iter().collect();
        assert_eq!(summary_text(19, &HashMap::new(), &month), None);

        let moved: HashMap<(&'static str, Source), i64> =
            [(("gryffindor", Source::Quiz), 4), (("ravenclaw", Source::Snitch), 6), (("ravenclaw", Source::Chat), 3)]
                .into_iter()
                .collect();
        let text = summary_text(19, &moved, &month).expect("points moved");
        assert!(text.contains("7pm – 8pm"));
        let raven = text.find("Ravenclaw +9").expect("ravenclaw line");
        let gryff = text.find("Gryffindor +4").expect("gryffindor line");
        assert!(raven < gryff, "the bigger gain comes first");
        assert!(!text.contains("<@"), "the summary names no one");
        assert!(text.contains("This month:"));
    }

    #[test]
    fn a_deduction_shows_as_a_minus() {
        let moved: HashMap<(&'static str, Source), i64> = [(("slytherin", Source::Mod), -5)].into_iter().collect();
        let text = summary_text(12, &moved, &HashMap::new()).expect("points moved");
        assert!(text.contains("Slytherin -5"), "{}", text);
    }

    #[test]
    fn mypoints_says_how_far_off_the_draw_someone_is() {
        let h = &HOUSES[0];
        let short = mypoints_text(h, &[(Source::Quiz, 4), (Source::Chat, 2)]);
        assert!(short.contains("**6** points") && short.contains("4 more"), "{}", short);
        let enough = mypoints_text(h, &[(Source::Koto, 12)]);
        assert!(enough.contains("in this month's Nitro draw"), "{}", enough);
    }

    #[test]
    fn the_draw_writeup_covers_every_outcome() {
        assert!(draw_text("August 2026", &DrawResult::Empty).contains("No house"));
        assert!(draw_text("August 2026", &DrawResult::Tie(vec!["gryffindor", "slytherin"])).contains("tie"));
        let won = draw_text(
            "August 2026",
            &DrawResult::Winner { house: "ravenclaw", points: 1624, captain: Some(1), member: Some(2), pool: 31 },
        );
        assert!(won.contains("Ravenclaw") && won.contains("<@1>") && won.contains("<@2>") && won.contains("31"));
        let nobody = draw_text(
            "August 2026",
            &DrawResult::Winner { house: "ravenclaw", points: 5, captain: None, member: None, pool: 0 },
        );
        assert!(nobody.contains("no captain") && nobody.contains("nobody"));
        assert_eq!(month_label("2026-08-01"), "August 2026");
    }
}
