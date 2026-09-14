//! Posts the reminders made on the panel.
//!
//! One task wakes every half minute and looks at every reminder that is
//! switched on. Timing is worked out from the India clock and the reminder's
//! own `last_sent`, which is saved after every post, so a restart can neither
//! repeat a post nor pile up the ones it slept through: a slot is only posted
//! while it is fresh, and at most once.
//!
//! This replaces the hard-coded hourly nudge about someone who left. That one is
//! seeded here as an ordinary reminder, switched off, the first time the store
//! is empty.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, Context, CreateAllowedMentions, CreateAttachment, CreateEmbed, CreateEmbedFooter, CreateMessage, MessageId, UserId,
};

use super::posts;
use super::reminders::{self, Order, Reminder, Schedule};

const TICK: Duration = Duration::from_secs(30);
/// India is UTC+5:30 all year.
const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;
const DAY: i64 = 86_400;
/// How long after its moment a slot may still go out. Past this it is skipped
/// rather than posted late: a bot that was down at 10:00 doesn't post the
/// 10:00 line at 13:00, and a reminder switched on mid-hour waits for the hour.
const FRESH_FOR: i64 = 15 * 60;
/// How long one Discord call may take before the tick gives up on it.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Least time between two asks to Discord whether a member is back.
const MEMBER_CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
/// After a post fails, how long before that reminder tries again.
const RETRY_AFTER: Duration = Duration::from_secs(5 * 60);

/// When each reminder last asked Discord about its member, by reminder id.
static CHECKED: LazyLock<Mutex<HashMap<i64, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// When each reminder last failed to post, by reminder id.
static FAILED: LazyLock<Mutex<HashMap<i64, Instant>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

// --- timing -------------------------------------------------------------------

/// "HH:MM" as minutes after midnight.
fn minutes_of(time: &str) -> Option<i64> {
    let (h, m) = time.trim().split_once(':')?;
    let (h, m) = (h.trim().parse::<i64>().ok()?, m.trim().parse::<i64>().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Midnight India time at the start of the day `ts` falls on.
fn ist_midnight(ts: i64) -> i64 {
    (ts + IST_OFFSET).div_euclid(DAY) * DAY - IST_OFFSET
}

/// The most recent moment at or before `now` the schedule says to post.
fn latest_slot(schedule: &Schedule, now: i64) -> Option<i64> {
    match schedule {
        Schedule::Every { minutes } => {
            let step = (*minutes).max(1) as i64 * 60;
            // Lined up on the India clock: every 60 is on the hour, every 1440 at midnight.
            Some((now + IST_OFFSET).div_euclid(step) * step - IST_OFFSET)
        }
        Schedule::Daily { times } => {
            let midnight = ist_midnight(now);
            times
                .iter()
                .filter_map(|t| minutes_of(t))
                .map(|m| {
                    let at = midnight + m * 60;
                    if at > now { at - DAY } else { at }
                })
                .max()
        }
    }
}

/// Whether a reminder should post at `now`: its latest slot came after the last
/// post, and not so long ago that posting it now would be a back-fill.
fn is_due(schedule: &Schedule, last_sent: i64, now: i64) -> bool {
    let Some(slot) = latest_slot(schedule, now) else {
        return false;
    };
    let fresh = match schedule {
        Schedule::Every { minutes } => FRESH_FOR.min((*minutes).max(1) as i64 * 60),
        Schedule::Daily { .. } => FRESH_FOR,
    };
    slot > last_sent && now - slot < fresh
}

/// Whether `now` falls inside the India-time window. Both ends empty (or the
/// same) is any time; a window like 22:00-02:00 runs across midnight. An end
/// that doesn't read as "HH:MM" counts as empty.
fn in_active_window(from: &str, to: &str, now: i64) -> bool {
    let from = minutes_of(from).unwrap_or(0);
    let to = minutes_of(to).unwrap_or(24 * 60);
    let at = (now - ist_midnight(now)) / 60;
    if from == to || (from == 0 && to == 24 * 60) {
        true
    } else if from < to {
        (from..to).contains(&at)
    } else {
        at >= from || at < to
    }
}

fn moment(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text.trim()).ok().map(|t| t.timestamp())
}

/// Past its end date. No end date never ends; one that doesn't read is ignored.
fn is_over(ends: &str, now: i64) -> bool {
    moment(ends).is_some_and(|end| now >= end)
}

// --- the words ------------------------------------------------------------------

/// Which line goes out: in turn by how many have gone, or at random from `roll` in `[0, 1)`.
fn pick_line(lines: &[String], order: Order, sent_count: i64, roll: f64) -> Option<&str> {
    let lines: Vec<&str> = lines.iter().map(String::as_str).filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    let index = match order {
        Order::Rotate => sent_count.rem_euclid(lines.len() as i64) as usize,
        Order::Random => ((roll.clamp(0.0, 1.0) * lines.len() as f64) as usize).min(lines.len() - 1),
    };
    Some(lines[index])
}

/// Fills the placeholders. `{hours}` and `{days}` are whole ones since `since`,
/// never negative, and 0 when there is no such moment.
fn render(template: &str, name: &str, user: Option<u64>, since: &str, now: i64) -> String {
    let elapsed = moment(since).map_or(0, |at| (now - at).max(0));
    let mention = user.map_or_else(|| name.to_string(), |id| format!("<@{}>", id));
    template
        .replace("{name}", name)
        .replace("{mention}", &mention)
        .replace("{hours}", &(elapsed / 3600).to_string())
        .replace("{days}", &(elapsed / DAY).to_string())
}

// --- the loop -------------------------------------------------------------------------

/// Starts the scheduler. Safe to call on every `ready`: it only starts once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    seed();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            let now = Utc::now().timestamp();
            for reminder in reminders::list().into_iter().filter(|r| r.enabled) {
                tick(&ctx, reminder, now).await;
            }
            // Members' own reminders ("remind me in 2 hours…").
            if super::super::control::on("VIZIER_MEMBER_REMINDERS", true) {
                for memo in super::memos::due(now, 25) {
                    let channel = memo.channel_id.parse::<u64>().ok().filter(|c| *c != 0).map(ChannelId::new);
                    let user = memo.user_id.parse::<u64>().ok();
                    let result = match channel {
                        Some(channel) => post(&ctx, channel, super::memos::message_text(&memo), user).await,
                        None => Err("no channel".into()),
                    };
                    match result {
                        Ok(()) => {
                            tracing::info!("reminders: member reminder {} sent", memo.id);
                            super::memos::mark_sent(memo.id, now);
                        }
                        Err(err) => {
                            tracing::warn!("reminders: member reminder {} not sent: {}", memo.id, err);
                            super::memos::mark_failed_try(memo.id);
                        }
                    }
                }
            }
        }
    });
}

async fn tick(ctx: &Context, r: Reminder, now: i64) {
    if is_over(&r.ends, now) {
        tracing::info!("reminders: '{}' reached its end, switching it off", r.name);
        update(r.id, |fresh| fresh.enabled = false);
        return;
    }
    if !in_active_window(&r.active_from, &r.active_to, now) {
        return;
    }
    if FAILED.lock().get(&r.id).is_some_and(|at| at.elapsed() < RETRY_AFTER) {
        return;
    }
    let Some(channel) = r.channel_id.trim().parse::<u64>().ok().filter(|id| *id != 0).map(ChannelId::new) else {
        return;
    };
    let user = r.user_id.trim().parse::<u64>().ok().filter(|id| *id != 0);
    let name = display_name(ctx, &r, user);

    if r.stop_when_back {
        if let Some(id) = user {
            if is_back(ctx, r.id, id).await {
                let text = render(&r.welcome_line, &name, user, &r.since, now);
                if !text.trim().is_empty() && post(ctx, channel, text, user).await.is_err() {
                    FAILED.lock().insert(r.id, Instant::now());
                    return;
                }
                tracing::info!("reminders: {} is back, '{}' switched off", name, r.name);
                update(r.id, |fresh| {
                    fresh.enabled = false;
                    fresh.last_sent = now;
                });
                return;
            }
        }
    }

    if !is_due(&r.schedule, r.last_sent, now) {
        return;
    }
    let Some(prepared) = prepare(&r, &name, user, now).await else {
        // Waits like a failed post, so an empty one doesn't ask the AI every tick.
        FAILED.lock().insert(r.id, Instant::now());
        return;
    };
    match deliver(ctx, channel, &prepared, user).await {
        Ok(message) => {
            FAILED.lock().remove(&r.id);
            // Logged either way: a silent success is indistinguishable from a
            // thread that died, which cost an evening of guessing once already.
            tracing::info!("reminders: posted '{}' ({} so far)", r.name, r.sent_count + 1);
            if let Some(old) = posts::previous_to_delete(&r, message) {
                match tokio::time::timeout(HTTP_WAIT, channel.delete_message(&ctx.http, MessageId::new(old))).await {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => tracing::info!("reminders: '{}' previous post not deleted: {}", r.name, err),
                    Err(_) => tracing::info!("reminders: '{}' previous post not deleted in time", r.name),
                }
            }
            update(r.id, |fresh| posts::record_post(fresh, now, message, prepared.ai_text.as_deref()));
        }
        Err(err) => {
            FAILED.lock().insert(r.id, Instant::now());
            tracing::warn!("reminders: '{}' not posted: {}", r.name, err);
        }
    }
}

// --- richer posts ------------------------------------------------------------------------

/// How long the AI may take to write a post before the lines are used.
const AI_WAIT: Duration = Duration::from_secs(60);
/// How long a post with a picture may take to upload.
const UPLOAD_WAIT: Duration = Duration::from_secs(60);

/// A post worked out and ready to send.
pub(super) struct Prepared {
    pub outgoing: posts::Outgoing,
    pub bytes: Option<Vec<u8>>,
    pub reactions: Vec<String>,
    /// The AI's text, when it wrote this one.
    pub ai_text: Option<String>,
}

/// Works out what a reminder posts now: the AI's text or the next line, the
/// placeholders filled, the picture, plain or card. `None` when there is
/// nothing to send.
async fn prepare(r: &Reminder, name: &str, user: Option<u64>, now: i64) -> Option<Prepared> {
    let texts = r.lines.iter().map(String::as_str).chain([r.title.as_str(), r.footer.as_str(), r.ai_prompt.as_str()]);
    let table = if posts::needs_houses(texts) { posts::standings_now(now) } else { Vec::new() };
    let fill = |text: &str| {
        let mut roll = || rand::random::<f64>();
        render(&posts::fill_extras(text, now, &table, &mut roll), name, user, &r.since, now)
    };

    let mut reply = None;
    if !r.ai_prompt.trim().is_empty() {
        let prompt = posts::ai_prompt(&fill(&r.ai_prompt), &r.ai_recent, now);
        match tokio::time::timeout(AI_WAIT, super::web::ask_bot_model(prompt)).await {
            Ok(Ok(answer)) => reply = Some(answer),
            Ok(Err(err)) => tracing::warn!("reminders: '{}' AI call failed, using a line: {}", r.name, err),
            Err(_) => tracing::warn!("reminders: '{}' AI took too long, using a line", r.name),
        }
    }
    let line = pick_line(&r.lines, r.order, r.sent_count, rand::random::<f64>()).map(|line| fill(line));
    let (text, ai_text) = posts::choose_text(reply.as_deref(), line);
    if reply.is_some() && ai_text.is_none() {
        tracing::warn!("reminders: '{}' AI reply was not usable, using a line", r.name);
    }

    let picture = posts::pick_image(&r.images, r.image_order, r.sent_count, rand::random::<f64>()).and_then(|id| {
        let found = super::media::get(id);
        if found.is_none() {
            tracing::warn!("reminders: '{}' picture {} is gone, posting without it", r.name, id);
        }
        found
    });
    let attachment = picture.as_ref().map(|(info, _)| info.attachment_name());
    let outgoing = posts::compose(r.style, &text, &fill(&r.title), &r.colour, &fill(&r.footer), attachment.as_deref());
    if outgoing.is_empty() {
        tracing::warn!("reminders: '{}' had nothing to post", r.name);
        return None;
    }
    Some(Prepared { outgoing, bytes: picture.map(|(_, bytes)| bytes), reactions: r.reactions.clone(), ai_text })
}

/// Sends a prepared post and adds its reactions; gives back the message id.
async fn deliver(ctx: &Context, channel: ChannelId, p: &Prepared, user: Option<u64>) -> Result<u64, String> {
    // Only the member the reminder is about may be pinged, and only by {mention}.
    let mentions = CreateAllowedMentions::new().users(user.map(UserId::new).into_iter().collect::<Vec<_>>());
    let mut message = CreateMessage::new().allowed_mentions(mentions);
    if !p.outgoing.content.is_empty() {
        message = message.content(p.outgoing.content.clone());
    }
    if let Some(card) = &p.outgoing.card {
        let mut embed = CreateEmbed::new().colour(card.colour);
        if !card.title.is_empty() {
            embed = embed.title(card.title.clone());
        }
        if !card.description.is_empty() {
            embed = embed.description(card.description.clone());
        }
        if let Some(image) = &card.image {
            embed = embed.image(image.clone());
        }
        if !card.footer.is_empty() {
            embed = embed.footer(CreateEmbedFooter::new(card.footer.clone()));
        }
        message = message.embed(embed);
    }
    if let (Some(name), Some(bytes)) = (&p.outgoing.attachment, &p.bytes) {
        message = message.add_file(CreateAttachment::bytes(bytes.clone(), name.clone()));
    }
    let wait = if p.bytes.is_some() { UPLOAD_WAIT } else { HTTP_WAIT };
    let sent = match tokio::time::timeout(wait, channel.send_message(&ctx.http, message)).await {
        Ok(Ok(sent)) => sent,
        Ok(Err(err)) => return Err(err.to_string()),
        Err(_) => return Err(format!("no answer from Discord in {}s", wait.as_secs())),
    };
    for emoji in p.reactions.iter().filter_map(|e| super::autoreplies::reaction(e)) {
        if let Ok(Err(err)) = tokio::time::timeout(HTTP_WAIT, channel.create_reaction(&ctx.http, sent.id, emoji)).await {
            tracing::info!("reminders: a reaction was not added: {}", err);
        }
    }
    Ok(sent.id.get())
}

/// Posts a reminder once, now, as it would go out, without touching its count,
/// its last post or what the AI remembers: the panel's "Send a test now".
pub async fn send_test(ctx: &Context, r: &Reminder) -> Result<(), String> {
    let channel = r.channel_id.trim().parse::<u64>().ok().filter(|id| *id != 0).map(ChannelId::new).ok_or("Pick a channel first.")?;
    let user = r.user_id.trim().parse::<u64>().ok().filter(|id| *id != 0);
    let name = display_name(ctx, r, user);
    let now = Utc::now().timestamp();
    let prepared = prepare(r, &name, user, now).await.ok_or("There's nothing to send yet: add a line, a picture or an AI prompt.")?;
    deliver(ctx, channel, &prepared, user).await.map(|_| ())
}

/// Changes only what the scheduler owns, on the stored copy read afresh, so an
/// edit saved on the panel a moment ago is not overwritten.
fn update(id: i64, change: impl FnOnce(&mut Reminder)) {
    let Some(mut fresh) = reminders::get(id) else {
        return;
    };
    change(&mut fresh);
    if let Err(err) = reminders::save(&fresh, 0) {
        tracing::warn!("reminders: reminder {} not saved: {}", id, err);
    }
}

fn display_name(ctx: &Context, r: &Reminder, user: Option<u64>) -> String {
    if !r.user_name.trim().is_empty() {
        return r.user_name.trim().to_string();
    }
    let Some(id) = user else {
        return "someone".to_string();
    };
    ctx.cache
        .guilds()
        .first()
        .and_then(|g| ctx.cache.guild(*g).and_then(|g| g.members.get(&UserId::new(id)).map(|m| m.display_name().to_string())))
        .or_else(|| ctx.cache.user(UserId::new(id)).map(|u| u.display_name().to_string()))
        .unwrap_or_else(|| "someone".to_string())
}

/// Whether the member is in the server: the cache first, Discord itself at most
/// every few minutes, since the cache only knows who it has seen.
async fn is_back(ctx: &Context, reminder: i64, user: u64) -> bool {
    let Some(guild) = ctx.cache.guilds().first().copied() else {
        return false;
    };
    if ctx.cache.guild(guild).is_some_and(|g| g.members.contains_key(&UserId::new(user))) {
        return true;
    }
    {
        let mut checked = CHECKED.lock();
        if checked.get(&reminder).is_some_and(|at| at.elapsed() < MEMBER_CHECK_EVERY) {
            return false;
        }
        checked.insert(reminder, Instant::now());
    }
    matches!(tokio::time::timeout(HTTP_WAIT, guild.member(&ctx.http, UserId::new(user))).await, Ok(Ok(_)))
}

async fn post(ctx: &Context, channel: ChannelId, text: String, user: Option<u64>) -> Result<(), String> {
    // Only the member the reminder is about may be pinged, and only by {mention}.
    let mentions = CreateAllowedMentions::new().users(user.map(UserId::new).into_iter().collect::<Vec<_>>());
    let message = CreateMessage::new().content(text).allowed_mentions(mentions);
    match tokio::time::timeout(HTTP_WAIT, channel.send_message(&ctx.http, message)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!("no answer from Discord in {}s", HTTP_WAIT.as_secs())),
    }
}

// --- the old nudge ------------------------------------------------------------------

/// The jokes the hourly nudge rotated through, one per hour.
const NUDGE_LINES: &[&str] = &[
    "Lagta hai gaon chhod ke chala gaya.",
    "Uski kursi ab bhi khaali hai. 🪑",
    "Ab tak ek reply bhi nahi, bade aadmi ban gaye hain.",
    "Lagta hai WiFi ka bill nahi bhara.",
    "Kisi ne uska last seen dekha hai?",
    "Ghar wapsi ka muhurat nikalwa raha hai shayad.",
    "Hum toh chai leke baithe hain, aa jaaye bhai.",
    "Ek din aayega aur kahega 'kuch miss kiya?'",
    "Server suna suna lag raha hai. 🥲",
    "Shayad abhi bhi loading screen pe atka hai.",
    "Uske bina kalesh bhi thanda pad gaya.",
    "Maybe aeroplane mode on chhod diya.",
    "Itna time lag raha hai, lagta hai paidal aa raha hai.",
    "Bhai, block nahi kiya na humne? 👀",
];

/// The nudge that used to be built in, as a reminder, switched off.
fn old_nudge() -> Reminder {
    Reminder {
        id: 0,
        name: "Lucky hasn't come back".to_string(),
        enabled: false,
        channel_id: "1516492867642593443".to_string(),
        lines: NUDGE_LINES
            .iter()
            .map(|line| format!("**{{name}}** abhi tak wapas nahi aaya - **{{hours}} ghante** ho gaye.\n{}", line))
            .collect(),
        order: Order::Rotate,
        schedule: Schedule::Every { minutes: 60 },
        active_from: String::new(),
        active_to: String::new(),
        user_id: "459076776266694670".to_string(),
        user_name: "Lucky".to_string(),
        since: "2026-09-07T09:54:54Z".to_string(),
        stop_when_back: true,
        welcome_line: "🎉 {mention} wapas aa gaya! Poore **{hours} ghante** lagaye. Sabne miss kiya, seriously."
            .to_string(),
        ends: String::new(),
        last_sent: 0,
        sent_count: 0,
        ..Default::default()
    }
}

/// Puts the old nudge in the store, once: only while the reminders table has
/// never held a row, so deleting it on the panel doesn't bring it back.
fn seed() {
    let never_used = super::DB.get().is_some_and(|db| {
        let conn = db.lock();
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM reminders", [], |r| r.get(0)).unwrap_or(1);
        // AUTOINCREMENT keeps a counter that outlives deleted rows.
        let ever: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_sequence WHERE name = 'reminders'", [], |r| r.get(0))
            .unwrap_or(0);
        rows == 0 && ever == 0
    });
    if !never_used {
        return;
    }
    match reminders::save(&old_nudge(), 0) {
        Ok(id) => tracing::info!("reminders: the old nudge is reminder {}, switched off", id),
        Err(err) => tracing::warn!("reminders: the old nudge was not seeded: {}", err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-14 00:00 India time.
    const MIDNIGHT: i64 = 1_789_324_200;
    const HOUR: i64 = 3600;

    fn every(minutes: u32) -> Schedule {
        Schedule::Every { minutes }
    }

    fn daily(times: &[&str]) -> Schedule {
        Schedule::Daily { times: times.iter().map(|t| t.to_string()).collect() }
    }

    #[test]
    fn every_hour_is_on_the_india_hour_and_posts_once() {
        let noon = MIDNIGHT + 12 * HOUR;
        assert_eq!(latest_slot(&every(60), noon + 30), Some(noon));
        // Never sent: the hour posts as it turns.
        assert!(is_due(&every(60), 0, noon + 30));
        // Sent a few seconds past the hour: not again that hour, restart or not.
        assert!(!is_due(&every(60), noon + 30, noon + 60));
        assert!(!is_due(&every(60), noon + 30, noon + 59 * 60));
        // The next hour.
        assert!(is_due(&every(60), noon + 30, noon + HOUR + 5));
        // Every 90 minutes lines up on midnight India time: 00:00, 01:30, 03:00...
        assert_eq!(latest_slot(&every(90), MIDNIGHT + 2 * HOUR), Some(MIDNIGHT + 90 * 60));
        assert_eq!(latest_slot(&every(0), MIDNIGHT + 30), Some(MIDNIGHT), "zero minutes reads as one");
    }

    #[test]
    fn missed_slots_are_not_back_filled() {
        let noon = MIDNIGHT + 12 * HOUR;
        // Down from 10:00 to 12:20: the 12:00 slot is too stale, and nothing
        // from 10 or 11 is owed either.
        assert!(!is_due(&every(60), MIDNIGHT + 9 * HOUR, noon + 20 * 60));
        assert!(is_due(&every(60), MIDNIGHT + 9 * HOUR, noon + 14 * 60));
        // Switched on mid-hour with an old last_sent: waits for the next hour.
        assert!(!is_due(&every(60), 0, noon + 40 * 60));
        assert!(is_due(&every(60), 0, noon + HOUR));
        // Every 5 minutes: each slot once, whenever in its 5 minutes the tick lands.
        assert!(is_due(&every(5), 0, noon + 4 * 60));
        assert!(!is_due(&every(5), noon + 10, noon + 4 * 60));
        assert!(is_due(&every(5), noon + 10, noon + 5 * 60 + 1));
    }

    #[test]
    fn daily_times_post_once_each_and_the_latest_one_counts() {
        let s = daily(&["09:00", "21:30", "nonsense"]);
        let nine = MIDNIGHT + 9 * HOUR;
        let half_nine_pm = MIDNIGHT + 21 * HOUR + 30 * 60;
        assert!(!is_due(&s, 0, nine - 60));
        assert!(is_due(&s, 0, nine + 60));
        assert!(!is_due(&s, nine + 60, nine + 120));
        assert_eq!(latest_slot(&s, MIDNIGHT + 15 * HOUR), Some(nine));
        assert!(is_due(&s, nine + 60, half_nine_pm + 10));
        // Just after midnight the latest time is last night's 21:30.
        assert_eq!(latest_slot(&s, MIDNIGHT + 60), Some(half_nine_pm - DAY));
        assert_eq!(latest_slot(&daily(&[]), MIDNIGHT), None);
        assert!(!is_due(&daily(&["bad"]), 0, MIDNIGHT));
    }

    #[test]
    fn active_windows_work_across_midnight() {
        let at = |h: i64, m: i64| MIDNIGHT + h * HOUR + m * 60;
        assert!(in_active_window("", "", at(3, 0)));
        assert!(in_active_window("10:00", "10:00", at(3, 0)), "the same both ends is any time");
        assert!(in_active_window("10:00", "23:00", at(10, 0)));
        assert!(!in_active_window("10:00", "23:00", at(23, 0)));
        assert!(!in_active_window("10:00", "23:00", at(9, 59)));
        assert!(in_active_window("22:00", "02:00", at(23, 30)));
        assert!(in_active_window("22:00", "02:00", at(1, 59)));
        assert!(!in_active_window("22:00", "02:00", at(2, 0)));
        assert!(!in_active_window("22:00", "02:00", at(12, 0)));
        assert!(in_active_window("18:00", "", at(23, 59)), "an empty end is midnight");
        assert!(!in_active_window("", "06:00", at(7, 0)), "an empty start is midnight");
    }

    #[test]
    fn a_reminder_ends_on_its_end_date() {
        let end = "2026-09-14T10:00:00Z";
        let at = moment(end).unwrap();
        assert!(!is_over(end, at - 1));
        assert!(is_over(end, at));
        assert!(!is_over("", at));
        assert!(!is_over("soon", at));
    }

    #[test]
    fn lines_rotate_in_turn_or_pick_at_random() {
        let lines: Vec<String> = ["a", "", "b", "c"].iter().map(|s| s.to_string()).collect();
        let turn: Vec<&str> = (0..7).map(|n| pick_line(&lines, Order::Rotate, n, 0.0).unwrap()).collect();
        assert_eq!(turn, vec!["a", "b", "c", "a", "b", "c", "a"], "blank lines are skipped");
        assert_eq!(pick_line(&lines, Order::Random, 0, 0.0), Some("a"));
        assert_eq!(pick_line(&lines, Order::Random, 0, 0.999), Some("c"));
        assert_eq!(pick_line(&lines, Order::Random, 0, 1.0), Some("c"));
        assert_eq!(pick_line(&[], Order::Rotate, 3, 0.5), None);
    }

    #[test]
    fn placeholders_fill_from_the_timestamp() {
        let now = moment("2026-09-12T11:00:05Z").unwrap();
        let text = render("**{name}** - **{hours} ghante**, {days} din. {mention}", "Lucky", Some(42), "2026-09-07T09:54:54Z", now);
        assert_eq!(text, "**Lucky** - **121 ghante**, 5 din. <@42>");
        // A clock behind the moment reads zero, and no member means {mention} is the name.
        let early = moment("2026-09-07T08:00:00Z").unwrap();
        assert_eq!(render("{hours}/{days} {mention}", "x", None, "2026-09-07T09:54:54Z", early), "0/0 x");
        assert_eq!(render("{hours}", "x", None, "", now), "0");
    }

    #[test]
    fn the_old_nudge_reads_as_it_did() {
        let r = old_nudge();
        assert!(!r.enabled && r.stop_when_back && r.schedule == every(60));
        assert_eq!(r.lines.len(), NUDGE_LINES.len());
        let now = moment("2026-09-12T11:00:05Z").unwrap();
        let first = render(pick_line(&r.lines, r.order, 0, 0.0).unwrap(), &r.user_name, Some(1), &r.since, now);
        assert_eq!(first, "**Lucky** abhi tak wapas nahi aaya - **121 ghante** ho gaye.\nLagta hai gaon chhod ke chala gaya.");
        let welcome = render(&r.welcome_line, &r.user_name, Some(459076776266694670), &r.since, now);
        assert_eq!(welcome, "🎉 <@459076776266694670> wapas aa gaya! Poore **121 ghante** lagaye. Sabne miss kiya, seriously.");
    }
}
