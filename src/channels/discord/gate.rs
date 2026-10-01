//! Two standing posts the month needs, and nothing else.
//!
//! **The hourly post** (`VIZIER_MONTH_HOURLY_CHANNEL`). The hourly house
//! scoreboard was switched off with the Cup, and during the egg week it could
//! not come back as it was: nobody has a house yet, so four house totals would
//! be four noughts. So the hourly post is about PEOPLE during the egg week -
//! the top five by what they earned in the hour just gone, each with their
//! month's total beside it - and gains the four house standings above that list
//! from the hatch onwards. The hatch it reads is [`super::month::hatch_at`],
//! the same moment everything else in the month reads, never a date of its own.
//!
//! **The join / opt-out post** (`VIZIER_GAMES_GATE_CHANNEL`). Two buttons that
//! give and take the server games role. It is a toggle, so pressing the one you
//! already match says so politely instead of erroring, and the whole reason it
//! exists is that somebody who opts out is not tagged - which works because
//! every ping is a role mention and Discord resolves those by membership, so
//! taking the role off is the whole of opting out.
//!
//! TWO RULES BOTH POSTS FOLLOW, because the owner asked for both by name.
//!
//! *One message, edited.* Neither post is ever sent twice. The id is kept in
//! the egg store, and on boot the channel's recent messages are read and the
//! bot's own post is adopted rather than a fresh one being added beside it. A
//! quiet hour EDITS the post to say the hour was quiet; it never adds an empty
//! one.
//!
//! *The gate stays last.* When anything is posted after the gate it is deleted
//! and reposted at the bottom - the same approach `battle.rs` uses for its
//! lobby, counting what has landed underneath and moving once it is buried
//! rather than racing every message. The hourly post deliberately does NOT do
//! this: the owner was explicit that he does not want the score to be the
//! permanent last message in a channel.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, ChannelId, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditMessage, GetMessages, Message,
    MessageId,
};

use super::egg_store as store;
use super::month;
use super::points::{self as ledger};
use super::signup::{self};
use super::signup_store::Answer;

/// The channel the hourly post lives in.
const HOURLY_CHANNEL: u64 = 1_548_371_226_890_604_665;
/// The channel the two buttons live in.
const GATE_CHANNEL: u64 = 1_549_295_943_751_307_324;
/// Members named in the hourly post.
pub const TOP: usize = 5;
const HOUR: i64 = 3600;
/// Messages under the gate before it is moved back to the bottom.
const STICKY_AFTER: u32 = 1;
/// How often the gate looks to see whether chat has buried it.
const GATE_TICK: Duration = Duration::from_secs(5);
/// How far back a boot scan reads looking for a post to adopt.
const SCAN: u8 = 50;

// --- the custom ids -----------------------------------------------------------------
//
// Dispatch is by these and nothing else, so a gate posted before a restart
// still answers afterwards.

pub const ID_JOIN: &str = "gate:join";
pub const ID_LEAVE: &str = "gate:leave";

pub fn owns_component(id: &str) -> bool {
    id == ID_JOIN || id == ID_LEAVE
}

// --- the settings ---------------------------------------------------------------------

fn hourly_on() -> bool {
    super::control::on("VIZIER_MONTH_HOURLY", true)
}

fn hourly_channel() -> Option<ChannelId> {
    Some(ChannelId::new(super::control::id("VIZIER_MONTH_HOURLY_CHANNEL").unwrap_or(HOURLY_CHANNEL)))
}

fn gate_on() -> bool {
    super::control::on("VIZIER_GAMES_GATE", true)
}

fn gate_channel() -> Option<ChannelId> {
    Some(ChannelId::new(super::control::id("VIZIER_GAMES_GATE_CHANNEL").unwrap_or(GATE_CHANNEL)))
}

// --- the words ------------------------------------------------------------------------

/// The India hour a moment falls in, as "3 pm".
fn hour12(hour: i64) -> String {
    match hour {
        0 => "12 am".into(),
        12 => "12 pm".into(),
        h if h < 12 => format!("{} am", h),
        h => format!("{} pm", h - 12),
    }
}

/// What the post says. `end` is the moment the hour closed.
///
/// `rows` is (name, points earned in the hour, their month total), biggest
/// first and already trimmed. `houses` is the four standings - `None` during
/// the egg week, when nobody has a house to stand for.
pub fn hourly_text(end: i64, rows: &[(String, i64, i64)], houses: Option<&[(String, String, i64)]>) -> String {
    let from = hour12(ledger::ist_hour(end - HOUR));
    let to = hour12(ledger::ist_hour(end));
    // The window is spelled out rather than implied: "top 5" on its own is the
    // kind of line two people read two different ways.
    let window = format!("**{}–{} India time**", from, to);
    let mut text = match houses {
        Some(_) => format!("🏆 **The hour just gone** · points earned {}", window),
        None => format!("🥚 **The hour just gone** · points earned {}", window),
    };
    if let Some(houses) = houses {
        let mut ordered: Vec<&(String, String, i64)> = houses.iter().collect();
        ordered.sort_by(|a, b| b.2.cmp(&a.2).then(a.1.cmp(&b.1)));
        for (crest, name, points) in ordered {
            text.push_str(&format!("\n{} **{}** · {}", crest, name, points));
        }
        text.push_str("\n");
    }
    if rows.is_empty() {
        text.push_str(&format!(
            "\nNobody scored between {} and {} — a quiet hour. Nothing has been added to anybody's total.",
            from, to
        ));
        return month::with_live(text);
    }
    text.push_str(&format!("\n**Top {} this hour**", rows.len().min(TOP)));
    for (i, (name, hour, total)) in rows.iter().take(TOP).enumerate() {
        let rank = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            n => format!("`{:>2}.`", n + 1),
        };
        text.push_str(&format!(
            "\n{} **{}** · **+{}** this hour · {} this month",
            rank, name, hour, total
        ));
    }
    month::with_live(text)
}

/// The standing post over the two buttons.
pub fn gate_text(counts: (i64, i64)) -> String {
    let (in_count, _) = counts;
    format!(
        "🎮 **The games**\nPress **Join the games** and you'll be pinged when one opens — a raven, a melee, a quiz, any of it. \
         Press **Opt out** and you won't be pinged again. You can change your mind whenever you like; it is one role, on or off.\n\
         -# **{}** {} in. Opting out never touches your points, your house or your cards.",
        in_count,
        if in_count == 1 { "person is" } else { "people are" }
    )
}

pub fn gate_buttons() -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(ID_JOIN).label("Join the games").style(ButtonStyle::Success).emoji('🎮'),
        CreateButton::new(ID_LEAVE).label("Opt out").style(ButtonStyle::Secondary),
    ])
}

/// What one press is told. `changed` is false when they already matched, which
/// is a polite note rather than an error. `worn` is false when the role could
/// not actually be moved.
pub fn gate_reply(pressed: Answer, changed: bool, worn: bool) -> String {
    let mut text = match (pressed, changed) {
        (Answer::In, true) => "✅ You're in — you'll be pinged when a game opens.".to_string(),
        (Answer::In, false) => "✅ You're already in — you'll be pinged when a game opens. Nothing more to do.".to_string(),
        (Answer::Out, true) => "👋 You're out — no more pings.".to_string(),
        (Answer::Out, false) => "👋 You're already out — no more pings. Nothing more to do.".to_string(),
    };
    if pressed == Answer::In && !worn {
        text.push_str(signup::NO_ROLE_NOTE);
    }
    text
}

// --- the press ------------------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let pressed = match component.data.custom_id.as_str() {
        ID_JOIN => Answer::In,
        ID_LEAVE => Answer::Out,
        _ => return,
    };
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(db) = super::signup_store::db() else {
        let _ = component
            .create_response(&ctx.http, whisper("Something's wrong at my end — try again in a minute.".into()))
            .await;
        return;
    };
    let user = component.user.id.get();
    let name = component.user.global_name.clone().unwrap_or_else(|| component.user.name.clone());
    // The same door the sign-up buttons use: one place decides what a yes and a
    // no do to the role and to the record, so the two can never drift apart -
    // and so there is only ever one games role, whichever message was pressed.
    // Judged per press, exactly as the sign-up buttons judge it: a mod who
    // fills the setting in or drags the role down should not have to restart
    // the bot for the next press to work.
    let holder = match component.guild_id {
        Some(guild) => signup::role_state(ctx, guild)
            .await
            .role()
            .map(|role| signup::GuildRole { ctx, guild, role: serenity::all::RoleId::new(role) }),
        None => None,
    };
    let wardrobe: Option<&dyn signup::Wardrobe> = holder.as_ref().map(|h| h as &dyn signup::Wardrobe);
    let press = signup::press(db, wardrobe, user, &name, pressed, Utc::now().timestamp()).await;
    if let Some(note) = &press.note {
        tracing::warn!("gate: {}", note);
    }
    let worn = press.role_moved || (pressed == Answer::Out);
    let _ = component.create_response(&ctx.http, whisper(gate_reply(pressed, press.changed, worn))).await;
    // The count on the post follows, quietly and at most once in a while.
    if press.changed {
        refresh_gate(ctx.clone());
    }
}

// --- one message, found again -----------------------------------------------------------

/// Which message each post is, remembered across a restart.
fn remembered(key: &str) -> Option<MessageId> {
    let db = store::db()?;
    let conn = db.lock();
    store::meta_get(&conn, key).and_then(|v| v.parse().ok()).map(MessageId::new)
}

fn remember(key: &str, id: MessageId) {
    if let Some(db) = store::db() {
        let conn = db.lock();
        let _ = store::meta_set(&conn, key, &id.get().to_string());
    }
}

/// The post this feature already owns in `channel`: the remembered one if it is
/// still there, otherwise the bot's own newest message that looks like it.
///
/// `looks_like` is given the content so an hourly post is not mistaken for a
/// gate. Adopting rather than posting is what stops a restart leaving two.
async fn adopt(
    ctx: &Context,
    channel: ChannelId,
    key: &str,
    looks_like: impl Fn(&Message) -> bool,
) -> Option<Message> {
    if let Some(id) = remembered(key) {
        if let Ok(found) = channel.message(&ctx.http, id).await {
            return Some(found);
        }
    }
    let me = ctx.cache.current_user().id;
    let recent = channel.messages(&ctx.http, GetMessages::new().limit(SCAN)).await.ok()?;
    // Newest first, so the first match is the one to keep.
    let mut mine: Vec<Message> = recent.into_iter().filter(|m| m.author.id == me && looks_like(m)).collect();
    let keep = mine.first().cloned()?;
    remember(key, keep.id);
    // Anything older of ours is a leftover from before a restart. One message
    // only, the owner said, so the rest go.
    for stale in mine.drain(1..) {
        let _ = channel.delete_message(&ctx.http, stale.id).await;
    }
    Some(keep)
}

fn is_hourly(msg: &Message) -> bool {
    msg.content.contains("The hour just gone")
}

fn is_gate(msg: &Message) -> bool {
    msg.content.contains("**The games**")
}

// --- the hourly post ---------------------------------------------------------------------

/// The top five and, after the hatch, the four standings.
fn hourly_rows(ctx: &Context, end: i64) -> (Vec<(String, i64, i64)>, Option<Vec<(String, String, i64)>>) {
    let Some(db) = super::house::db() else { return (Vec::new(), None) };
    let month_start = ledger::month_start(end);
    let conn = db.lock();
    let top = ledger::top_server(&conn, end - HOUR, end, TOP).unwrap_or_default();
    let rows: Vec<(String, i64, i64)> = top
        .into_iter()
        .map(|(user, hour)| {
            let total: i64 =
                ledger::breakdown(&conn, user, month_start).map(|r| r.iter().map(|(_, n)| n).sum()).unwrap_or(0);
            let name = ctx
                .cache
                .user(serenity::all::UserId::new(user))
                .map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone()))
                .unwrap_or_else(|| "A member".to_string());
            (name, hour, total)
        })
        .collect();
    // The houses only once the eggs are open: before that they are four noughts
    // and a lie about who is winning.
    let houses = (end >= month::hatch_at()).then(|| {
        month::themed_all()
            .into_iter()
            .map(|worn| {
                let points = ledger::house_total(&conn, worn.key, month_start, i64::MAX).unwrap_or(0);
                (worn.crest, worn.name, points)
            })
            .collect()
    });
    (rows, houses)
}

/// Writes the hour into the one post, editing it in place.
async fn post_hour(ctx: &Context, end: i64) {
    if !hourly_on() || !month::running() {
        return;
    }
    let Some(channel) = hourly_channel() else { return };
    // Once per hour, whatever a restart does: the hour already written is kept
    // in the egg store beside the message id.
    if let Some(db) = store::db() {
        let conn = db.lock();
        if store::meta_get(&conn, "hourly_done").and_then(|v| v.parse::<i64>().ok()).is_some_and(|done| done >= end) {
            return;
        }
    }
    let (rows, houses) = hourly_rows(ctx, end);
    let text = hourly_text(end, &rows, houses.as_deref());
    let existing = adopt(ctx, channel, "hourly_message", is_hourly).await;
    match existing {
        // Edited, never reposted: a quiet hour rewrites the post to say so
        // rather than adding an empty one, and the score never becomes the
        // channel's permanent last message.
        Some(mut msg) => {
            if let Err(err) = msg.edit(&ctx.http, EditMessage::new().content(&text)).await {
                tracing::warn!("gate: the hourly post was not edited: {}", err);
                return;
            }
        }
        None => {
            match channel
                .send_message(&ctx.http, CreateMessage::new().content(&text).allowed_mentions(CreateAllowedMentions::new()))
                .await
            {
                Ok(posted) => remember("hourly_message", posted.id),
                Err(err) => {
                    tracing::warn!("gate: the hourly post was not sent: {}", err);
                    return;
                }
            }
        }
    }
    if let Some(db) = store::db() {
        let conn = db.lock();
        let _ = store::meta_set(&conn, "hourly_done", &end.to_string());
    }
}

// --- the gate, kept at the bottom ----------------------------------------------------------

/// How many messages have landed under the gate since it was last moved.
static BELOW: LazyLock<Mutex<HashMap<u64, u32>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// True while the gate is being posted, so its own message isn't counted as
/// having buried it.
static POSTING: AtomicBool = AtomicBool::new(false);

/// Counts chat under the gate. Called for every message the bot sees.
pub fn on_message(ctx: &Context, msg: &Message) {
    let Some(here) = gate_channel() else { return };
    if msg.channel_id != here || !gate_on() {
        return;
    }
    if POSTING.load(Ordering::SeqCst) || is_gate(msg) {
        return;
    }
    let _ = ctx;
    *BELOW.lock().entry(here.get()).or_insert(0) += 1;
}

/// Puts the gate back at the bottom when chat has buried it, and keeps its
/// count true. One message only: the old one is deleted after the new one is up,
/// never before, so a failed post can't leave the channel with no gate at all.
async fn keep_gate(ctx: &Context) {
    if !gate_on() {
        return;
    }
    let Some(channel) = gate_channel() else { return };
    let counts = super::signup_store::db().map(|db| super::signup_store::counts(&db.lock())).unwrap_or((0, 0));
    let text = gate_text(counts);
    let rows = vec![gate_buttons()];
    let existing = adopt(ctx, channel, "gate_message", is_gate).await;
    let buried = BELOW.lock().get(&channel.get()).copied().unwrap_or(0) >= STICKY_AFTER;
    POSTING.store(true, Ordering::SeqCst);
    match existing {
        Some(mut msg) if !buried => {
            if let Err(err) = msg.edit(&ctx.http, EditMessage::new().content(&text).components(rows)).await {
                tracing::debug!("gate: not edited: {}", err);
            }
        }
        existing => {
            // A message cannot be moved, only replaced.
            let fresh = CreateMessage::new().content(&text).components(rows).allowed_mentions(CreateAllowedMentions::new());
            match channel.send_message(&ctx.http, fresh).await {
                Ok(posted) => {
                    remember("gate_message", posted.id);
                    BELOW.lock().insert(channel.get(), 0);
                    if let Some(old) = existing {
                        let _ = channel.delete_message(&ctx.http, old.id).await;
                    }
                }
                Err(err) => tracing::warn!("gate: the join post was not sent: {}", err),
            }
        }
    }
    POSTING.store(false, Ordering::SeqCst);
}

/// Rewrites the gate's count soon, without waiting for the tick.
fn refresh_gate(ctx: Context) {
    tokio::spawn(async move {
        keep_gate(&ctx).await;
    });
}

// --- the jobs -----------------------------------------------------------------------------

/// The turn of the next India hour, plus a few seconds so the hour is closed.
fn next_hour(now: i64) -> i64 {
    let into = (now + month::IST_OFFSET).rem_euclid(HOUR);
    now - into + HOUR
}

pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hourly = ctx.clone();
    tokio::spawn(async move {
        // Let the cache settle, then write the hour that has just gone so a
        // restart mid-hour doesn't leave the post an hour stale.
        tokio::time::sleep(Duration::from_secs(20)).await;
        post_hour(&hourly, next_hour(Utc::now().timestamp()) - HOUR).await;
        loop {
            let now = Utc::now().timestamp();
            let next = next_hour(now);
            tokio::time::sleep(Duration::from_secs((next - now + 10).max(1) as u64)).await;
            post_hour(&hourly, next_hour(Utc::now().timestamp()) - HOUR).await;
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        loop {
            keep_gate(&ctx).await;
            tokio::time::sleep(GATE_TICK).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::super::month::testing::Month;
    use super::*;

    /// 2026-10-02, 4pm India time: the hour from 3 to 4.
    fn four_pm() -> i64 {
        month::parse_ist("2026-10-02 16:00").expect("a real moment")
    }

    fn rows() -> Vec<(String, i64, i64)> {
        vec![
            ("Zoya".into(), 9, 412),
            ("Kabir".into(), 6, 377),
            ("Ira".into(), 4, 120),
            ("Dev".into(), 2, 44),
            ("Noor".into(), 1, 8),
        ]
    }

    fn houses() -> Vec<(String, String, i64)> {
        vec![
            ("🐺".into(), "Stark".into(), 980),
            ("🦁".into(), "Lannister".into(), 1010),
            ("🐉".into(), "Targaryen".into(), 950),
            ("🗡️".into(), "Night's Watch".into(), 1002),
        ]
    }

    #[test]
    fn the_hour_is_spelled_out_so_nobody_has_to_guess_the_window() {
        let _month = Month::on();
        let text = hourly_text(four_pm(), &rows(), None);
        assert!(text.contains("3 pm–4 pm India time"), "the window must be on it: {}", text);
        assert!(text.contains("points earned"), "and what the number means");
        assert!(text.contains("Top 5 this hour"));
        for (name, hour, total) in rows() {
            assert!(text.contains(&name), "{} is missing", name);
            assert!(text.contains(&format!("+{}", hour)), "{}'s hour is missing", name);
            assert!(text.contains(&format!("{} this month", total)), "{}'s month total is missing", name);
        }
    }

    #[test]
    fn the_egg_week_shows_people_and_after_the_hatch_the_houses_go_above_them() {
        let mut month = Month::on();
        month.set("VIZIER_MONTH_HATCH", "2026-10-08 12:00");
        let week = hourly_text(four_pm(), &rows(), None);
        assert!(week.starts_with("🥚"), "the egg week's post is about people: {}", week);
        for (_, name, _) in houses() {
            assert!(!week.contains(&name), "{} has no business on an egg-week post", name);
        }
        let after = hourly_text(four_pm(), &rows(), Some(&houses()));
        assert!(after.starts_with("🏆"));
        // The standings sit ABOVE the five people, biggest first.
        let lannister = after.find("Lannister").expect("the leader");
        let watch = after.find("Night's Watch").expect("second");
        let zoya = after.find("Zoya").expect("the top scorer");
        assert!(lannister < watch, "the standings are ordered by points, not by name");
        assert!(watch < zoya, "the houses go above the top five");
        assert!(after.contains("Zoya"), "and the five are still there");
    }

    #[test]
    fn a_quiet_hour_says_so_rather_than_showing_an_empty_list() {
        let _month = Month::on();
        let quiet = hourly_text(four_pm(), &[], None);
        assert!(quiet.contains("Nobody scored between 3 pm and 4 pm"), "{}", quiet);
        assert!(quiet.contains("a quiet hour"));
        assert!(!quiet.contains("Top"), "there is no top five to show: {}", quiet);
        // And it still says the window and still carries the page.
        assert!(quiet.contains("3 pm–4 pm India time"));
        // A quiet hour after the hatch still shows the standings: the houses
        // have totals even in an hour when nobody scored.
        let after = hourly_text(four_pm(), &[], Some(&houses()));
        assert!(after.contains("Lannister") && after.contains("a quiet hour"));
    }

    #[test]
    fn fewer_than_five_scorers_shows_only_the_ones_who_scored() {
        let _month = Month::on();
        let two = &rows()[..2];
        let text = hourly_text(four_pm(), two, None);
        assert!(text.contains("Top 2 this hour"), "{}", text);
        assert!(text.contains("Zoya") && text.contains("Kabir"));
        assert!(!text.contains("Ira"), "nobody who didn't score is named");
        assert_eq!(text.matches("this month").count(), 2);
    }

    #[test]
    fn the_live_link_is_on_the_hourly_post() {
        let mut month = Month::on();
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live");
        for text in [hourly_text(four_pm(), &rows(), None), hourly_text(four_pm(), &[], Some(&houses()))] {
            assert!(text.contains("https://mlci.example/live"), "{}", text);
        }
    }

    #[test]
    fn every_hour_of_the_day_reads_as_a_time_of_day() {
        assert_eq!(hour12(0), "12 am");
        assert_eq!(hour12(1), "1 am");
        assert_eq!(hour12(11), "11 am");
        assert_eq!(hour12(12), "12 pm");
        assert_eq!(hour12(13), "1 pm");
        assert_eq!(hour12(23), "11 pm");
        // Midnight's hour reads as 11 pm–12 am, not as "23–24".
        let midnight = month::parse_ist("2026-10-03 00:00").unwrap();
        let _month = Month::on();
        assert!(hourly_text(midnight, &[], None).contains("11 pm–12 am"));
    }

    #[test]
    fn the_hourly_job_wakes_on_the_india_hour() {
        let at = month::parse_ist("2026-10-02 15:37").unwrap();
        let next = next_hour(at);
        assert_eq!(next, month::parse_ist("2026-10-02 16:00").unwrap());
        assert_eq!(next_hour(next), month::parse_ist("2026-10-02 17:00").unwrap(), "on the hour, the next one");
        assert_eq!(ledger::ist_hour(next - HOUR), 15, "the hour written is the one that closed");
    }

    // --- the gate -------------------------------------------------------------------

    #[test]
    fn the_two_buttons_are_a_toggle_and_say_so_politely_either_way() {
        assert_eq!(gate_reply(Answer::In, true, true), "✅ You're in — you'll be pinged when a game opens.");
        assert_eq!(gate_reply(Answer::Out, true, true), "👋 You're out — no more pings.");
        // Pressing the one you already match is a note, never an error.
        for (pressed, already) in [(Answer::In, gate_reply(Answer::In, false, true)), (Answer::Out, gate_reply(Answer::Out, false, true))] {
            assert!(already.contains("already"), "{:?} should say so: {}", pressed, already);
            assert!(already.contains("Nothing more to do"), "{}", already);
            assert!(!already.to_lowercase().contains("error") && !already.contains("wrong"));
        }
        // A yes the role couldn't be given for says the list has them anyway.
        let stuck = gate_reply(Answer::In, true, false);
        assert!(stuck.contains(signup::NO_ROLE_NOTE.trim()), "{}", stuck);
        // A no never needs that note: there is nothing to take off.
        assert!(!gate_reply(Answer::Out, true, false).contains(signup::NO_ROLE_NOTE.trim()));
    }

    #[test]
    fn the_gate_names_both_buttons_and_counts_who_is_in() {
        let ids = format!("{:?}", gate_buttons());
        assert!(ids.contains(ID_JOIN) && ids.contains(ID_LEAVE));
        assert!(ids.contains("Join the games") && ids.contains("Opt out"));
        assert!(owns_component(ID_JOIN) && owns_component(ID_LEAVE));
        assert!(!owns_component("signup:in"), "the sign-up buttons are somebody else's");
        let text = gate_text((53, 2));
        assert!(text.contains("**53** people are in"), "{}", text);
        assert!(text.contains("Join the games") && text.contains("Opt out"));
        // The one thing it must promise: opting out costs nothing else.
        assert!(text.contains("never touches your points"));
        assert_eq!(gate_text((1, 0)).contains("**1** person is in"), true);
    }

    #[test]
    fn the_gate_knows_its_own_post_and_the_hourly_one_apart() {
        let _month = Month::on();
        // The two markers must not match each other's text, or a restart would
        // adopt the wrong message and start editing the score into the gate.
        let gate = gate_text((5, 0));
        let hourly = hourly_text(four_pm(), &rows(), None);
        assert!(gate.contains("**The games**") && !gate.contains("The hour just gone"));
        assert!(hourly.contains("The hour just gone") && !hourly.contains("**The games**"));
    }
}
