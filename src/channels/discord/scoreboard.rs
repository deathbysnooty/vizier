//! The scoreboard card in the gaming updates channel: each House's points this
//! month with a bar, what each gained in the last hour, and a button that shows
//! whoever presses it the hour's top scorers. The card itself names no members;
//! names only appear in the private reply to the button.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::{Connection, params};
use serenity::all::{
    ButtonStyle, ChannelId, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    MessageId,
};

use super::house::{self, HOUSES, House};
use super::points::{self as ledger, Source};

const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;
const HOUR: i64 = 3600;
const BAR_WIDTH: usize = 12;
/// How many names the button lists.
const TOP: usize = 5;

fn channel() -> Option<ChannelId> {
    super::control::id("VIZIER_SCOREBOARD_CHANNEL").map(ChannelId::new)
}

fn ist_hour_floor(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(HOUR) * HOUR - IST_OFFSET
}

fn ist_hour(ts: i64) -> i64 {
    (ts + IST_OFFSET).rem_euclid(86_400) / HOUR
}

fn hour12(hour: i64) -> String {
    match hour.rem_euclid(24) {
        0 => "12am".into(),
        12 => "12pm".into(),
        h if h < 12 => format!("{}am", h),
        h => format!("{}pm", h - 12),
    }
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

/// A bar of `BAR_WIDTH` blocks, full for the leader.
fn bar(points: i64, best: i64) -> String {
    let filled = if best <= 0 { 0 } else { ((points.max(0) as f64 / best as f64) * BAR_WIDTH as f64).round() as usize };
    let filled = filled.min(BAR_WIDTH);
    format!("{}{}", "▰".repeat(filled), "▱".repeat(BAR_WIDTH - filled))
}

fn month_name(ts: i64) -> String {
    chrono::FixedOffset::east_opt(IST_OFFSET as i32)
        .and_then(|tz| tz.timestamp_opt(ts, 0).single())
        .map(|t| t.format("%B").to_string())
        .unwrap_or_default()
}

/// The card for the hour `[start, end)`, from month totals and the hour's gains.
pub fn card(end: i64, month: &HashMap<&'static str, i64>, hour: &HashMap<&'static str, i64>) -> CreateEmbed {
    let mut rows: Vec<&House> = HOUSES.iter().collect();
    rows.sort_by(|a, b| month.get(b.key).cmp(&month.get(a.key)).then(a.name.cmp(b.name)));
    let best = rows.first().and_then(|h| month.get(h.key)).copied().unwrap_or(0);
    let medals = ["🥇", "🥈", "🥉", "4️⃣"];
    let mut lines = Vec::new();
    for (i, h) in rows.iter().enumerate() {
        let points = month.get(h.key).copied().unwrap_or(0);
        let gained = hour.get(h.key).copied().unwrap_or(0);
        let gain = match gained {
            0 => String::new(),
            n if n > 0 => format!(" · **+{}** this hour", n),
            n => format!(" · **{}** this hour", n),
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
    let leader = rows.first().copied();
    let second = rows.get(1).and_then(|h| month.get(h.key)).copied().unwrap_or(0);
    let lead = match leader {
        Some(h) if best > second => format!("{} {} lead by **{}**", h.crest, h.name, thousands(best - second)),
        _ => "It's neck and neck at the top".to_string(),
    };
    let start = end - HOUR;
    CreateEmbed::new()
        .title(format!("🏆 House Cup · {}", month_name(end - 1)))
        .description(format!("{}\n\n{}", lead, lines.join("\n\n")))
        .colour(leader.map(|h| h.colour).unwrap_or(0xD4A73C))
        .footer(CreateEmbedFooter::new(format!(
            "Last hour: {} – {} India time · /today for your own points",
            hour12(ist_hour(start)),
            hour12(ist_hour(end))
        )))
}

pub fn button(end: i64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("scoreboard:top:{}", end)).label("Top scorers this hour").emoji('⭐').style(ButtonStyle::Primary),
    ])
}

/// Everyone's points in `[start, end)`: (user, house, total, sources), best first.
fn scorers(conn: &Connection, start: i64, end: i64) -> rusqlite::Result<Vec<(u64, &'static House, i64, Vec<(Source, i64)>)>> {
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
    let mut out: Vec<(u64, &'static House, i64, i64, Vec<(Source, i64)>)> =
        by_user.into_iter().filter(|(_, v)| v.1 > 0).map(|(u, (h, t, last, mut s))| {
            s.sort_by(|a, b| b.1.cmp(&a.1));
            (u, h, t, last, s)
        }).collect();
    // Most points; a tie goes to whoever finished scoring first.
    out.sort_by(|a, b| b.2.cmp(&a.2).then(a.3.cmp(&b.3)).then(a.0.cmp(&b.0)));
    Ok(out.into_iter().map(|(u, h, t, _, s)| (u, h, t, s)).collect())
}

/// The private reply to the button.
pub fn top_text(start: i64, end: i64, rows: &[(u64, &'static House, i64, Vec<(Source, i64)>)]) -> String {
    let when = format!("{} – {}", hour12(ist_hour(start)), hour12(ist_hour(end)));
    if rows.is_empty() {
        return format!("⭐ **Top scorers · {}**\nNobody scored in that hour.", when);
    }
    let medals = ["🥇", "🥈", "🥉"];
    let mut lines = vec![format!("⭐ **Top scorers · {} India time**", when)];
    for (i, (user, h, total, sources)) in rows.iter().take(TOP).enumerate() {
        let from: Vec<String> = sources.iter().take(3).map(|(s, n)| format!("{} {}", s.label(), n)).collect();
        lines.push(format!(
            "{} <@{}> {} — **{}** pts\n-# {}",
            medals.get(i).copied().unwrap_or("▫️"),
            user,
            h.crest,
            total,
            from.join(" · ")
        ));
    }
    lines.join("\n")
}

/// The button: the hour the card was posted for, not whatever hour it is now.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let end = component.data.custom_id.strip_prefix("scoreboard:top:").and_then(|v| v.parse::<i64>().ok());
    let text = match (end, house::db()) {
        (Some(end), Some(db)) => {
            let rows = scorers(&db.lock(), end - HOUR, end).unwrap_or_default();
            top_text(end - HOUR, end, &rows)
        }
        _ => "The scores for that hour couldn't be read.".to_string(),
    };
    let reply = CreateInteractionResponseMessage::new()
        .content(text)
        .ephemeral(true)
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

/// Posts the card for the hour that just ended, if it's due.
async fn post(ctx: &Context, end: i64) {
    if !super::control::on("VIZIER_SCOREBOARD", true) {
        return;
    }
    let Some(channel) = channel() else {
        return;
    };
    let every = super::control::number("VIZIER_SCOREBOARD_EVERY_HOURS", 1).clamp(1, 24) as i64;
    let first = super::control::number("VIZIER_SCOREBOARD_FIRST_HOUR", 10) as i64;
    let last = super::control::number("VIZIER_SCOREBOARD_LAST_HOUR", 23) as i64;
    let hour = ist_hour(end - HOUR);
    if !(first..=last).contains(&hour) || (hour - first).rem_euclid(every) != 0 {
        return;
    }
    if house::meta_get("scoreboard_last").and_then(|v| v.parse::<i64>().ok()).is_some_and(|done| done >= end) {
        return;
    }
    let (month, gained) = {
        let Some(db) = house::db() else {
            return;
        };
        let conn = db.lock();
        let month = ledger::house_totals(&conn, ledger::month_start(end - 1)).unwrap_or_default();
        let moved = ledger::by_source(&conn, end - HOUR, end).unwrap_or_default();
        let mut gained: HashMap<&'static str, i64> = HashMap::new();
        for ((h, _), n) in moved {
            *gained.entry(h).or_insert(0) += n;
        }
        (month, gained)
    };
    if super::control::on("VIZIER_SCOREBOARD_QUIET_SKIP", true) && gained.values().all(|n| *n == 0) {
        house::meta_set("scoreboard_last", &end.to_string());
        return;
    }
    let message = CreateMessage::new()
        .embed(card(end, &month, &gained))
        .components(vec![button(end)])
        .allowed_mentions(CreateAllowedMentions::new());
    match channel.send_message(&ctx.http, message).await {
        Ok(sent) => {
            if super::control::on("VIZIER_SCOREBOARD_REPLACE", true) {
                if let Some(old) = house::meta_get("scoreboard_message").and_then(|v| v.parse::<u64>().ok()) {
                    let _ = channel.delete_message(&ctx.http, MessageId::new(old)).await;
                }
            }
            house::meta_set("scoreboard_message", &sent.id.get().to_string());
            house::meta_set("scoreboard_last", &end.to_string());
        }
        Err(err) => tracing::warn!("scoreboard: not posted: {}", err),
    }
}

/// Wakes at the turn of each India hour.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        loop {
            let now = Utc::now().timestamp();
            let next = ist_hour_floor(now) + HOUR;
            tokio::time::sleep(Duration::from_secs((next - now + 10).max(1) as u64)).await;
            post(&ctx, ist_hour_floor(Utc::now().timestamp())).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-14 00:00 India time.
    const MIDNIGHT: i64 = 1_789_324_200;

    #[test]
    fn bars_and_numbers_read_well() {
        assert_eq!(bar(100, 100), "▰".repeat(12));
        assert_eq!(bar(50, 100), format!("{}{}", "▰".repeat(6), "▱".repeat(6)));
        assert_eq!(bar(0, 0), "▱".repeat(12));
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(999), "999");
        assert_eq!(hour12(ist_hour(MIDNIGHT + 19 * HOUR)), "7pm");
    }

    #[test]
    fn the_card_ranks_houses_and_shows_the_hour() {
        let month: HashMap<&'static str, i64> = [("ravenclaw", 663), ("gryffindor", 593), ("slytherin", 321), ("hufflepuff", 310)].into_iter().collect();
        let hour: HashMap<&'static str, i64> = [("slytherin", 12), ("ravenclaw", 4)].into_iter().collect();
        let json = serde_json::to_value(card(MIDNIGHT + 20 * HOUR, &month, &hour)).unwrap();
        let text = json["description"].as_str().unwrap();
        let ravenclaw = text.find("Ravenclaw").unwrap();
        let gryffindor = text.find("Gryffindor").unwrap();
        assert!(ravenclaw < gryffindor, "{}", text);
        assert!(text.contains("lead by **70**"), "{}", text);
        assert!(text.contains("**+12** this hour"), "{}", text);
        assert!(json["footer"]["text"].as_str().unwrap().contains("7pm – 8pm"));
        assert_eq!(json["title"], "🏆 House Cup · September");
    }

    #[test]
    fn the_top_list_names_the_best_first_with_their_sources() {
        let gryffindor = HOUSES.iter().find(|h| h.key == "gryffindor").unwrap();
        let rows = vec![(7, gryffindor, 9, vec![(Source::Quiz, 6), (Source::Chat, 3)])];
        let text = top_text(MIDNIGHT + 19 * HOUR, MIDNIGHT + 20 * HOUR, &rows);
        assert!(text.contains("🥇 <@7>") && text.contains("**9** pts") && text.contains("7pm – 8pm"), "{}", text);
        assert!(top_text(0, HOUR, &[]).contains("Nobody scored"));
    }
}
