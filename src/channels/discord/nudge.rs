//! The hourly nudge about someone who walked out.
//!
//! This was a scheduled agent task first, and it was a bad fit: every run cost
//! about 6.4k input tokens to produce one line, and the model was also asked to
//! count the hours, which it simply invented. Here the hours come from a
//! timestamp and the jokes come from a list, so the number is right, the cost is
//! nothing, and the only thing that changes hourly is which line comes up.
//!
//! Off unless `VIZIER_NUDGE_CHANNEL`, `VIZIER_NUDGE_USER` and
//! `VIZIER_NUDGE_SINCE` are all set. It stops itself the moment they rejoin.

use chrono::{DateTime, Utc};
use serenity::all::{ChannelId, Context, CreateAllowedMentions, CreateMessage, UserId};
use std::time::Duration;

/// Small offset past the hour, so the post lands after the hour has turned.
const PAST_THE_HOUR: Duration = Duration::from_secs(5);

struct Nudge {
    channel: ChannelId,
    user: UserId,
    name: String,
    since: DateTime<Utc>,
}

/// One per hour, in order, so the same joke can't land twice running.
const LINES: &[&str] = &[
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

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty())
}

fn config() -> Option<Nudge> {
    let since = env("VIZIER_NUDGE_SINCE")?;
    let since = DateTime::parse_from_rfc3339(&since)
        .map_err(|err| tracing::warn!("nudge: VIZIER_NUDGE_SINCE is not a date: {}", err))
        .ok()?
        .with_timezone(&Utc);
    Some(Nudge {
        channel: ChannelId::new(env("VIZIER_NUDGE_CHANNEL")?.parse().ok()?),
        user: UserId::new(env("VIZIER_NUDGE_USER")?.parse().ok()?),
        name: env("VIZIER_NUDGE_NAME").unwrap_or_else(|| "he".to_string()),
        since,
    })
}

/// Whole hours since they left, never negative however the clocks are set.
fn hours_since(since: DateTime<Utc>, now: DateTime<Utc>) -> i64 {
    (now - since).num_hours().max(0)
}

fn line_for(hours: i64) -> &'static str {
    LINES[(hours.rem_euclid(LINES.len() as i64)) as usize]
}

fn nudge_text(name: &str, hours: i64) -> String {
    format!("**{}** abhi tak wapas nahi aaya - **{} ghante** ho gaye.\n{}", name, hours, line_for(hours))
}

fn welcome_text(user: UserId, hours: i64) -> String {
    format!("🎉 <@{}> wapas aa gaya! Poore **{} ghante** lagaye. Sabne miss kiya, seriously.", user, hours)
}

/// Seconds until a few moments past the next hour.
fn until_next_hour(now: DateTime<Utc>) -> Duration {
    let past = now.timestamp() % 3600;
    Duration::from_secs((3600 - past) as u64) + PAST_THE_HOUR
}

pub fn spawn(ctx: Context) {
    if config().is_none() {
        return;
    }
    tokio::spawn(run(ctx));
}

async fn run(ctx: Context) {
    let Some(nudge) = config() else {
        return;
    };
    tracing::info!("nudge: waiting on {} in channel {}", nudge.name, nudge.channel);
    loop {
        tokio::time::sleep(until_next_hour(Utc::now())).await;
        let hours = hours_since(nudge.since, Utc::now());
        // Back in the server? Say so once and stop for good.
        let returned = match ctx.cache.guilds().first().copied() {
            Some(guild) => guild.member(&ctx.http, nudge.user).await.is_ok(),
            None => false,
        };
        let (text, mention) = if returned {
            (welcome_text(nudge.user, hours), true)
        } else {
            (nudge_text(&nudge.name, hours), false)
        };
        let mentions =
            if mention { CreateAllowedMentions::new().users(vec![nudge.user]) } else { CreateAllowedMentions::new() };
        if let Err(err) =
            nudge.channel.send_message(&ctx.http, CreateMessage::new().content(text).allowed_mentions(mentions)).await
        {
            tracing::warn!("nudge: not posted: {}", err);
        }
        if returned {
            tracing::info!("nudge: {} is back, stopping", nudge.name);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hours_are_counted_from_the_timestamp_not_guessed() {
        let left = DateTime::parse_from_rfc3339("2026-09-07T09:54:54Z").unwrap().with_timezone(&Utc);
        let now = DateTime::parse_from_rfc3339("2026-09-12T11:00:05Z").unwrap().with_timezone(&Utc);
        assert_eq!(hours_since(left, now), 121);
        // A clock behind the timestamp reads zero rather than a negative count.
        let earlier = DateTime::parse_from_rfc3339("2026-09-07T08:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(hours_since(left, earlier), 0);
        assert!(nudge_text("Lucky", 121).contains("121 ghante"));
    }

    #[test]
    fn the_lines_rotate_and_never_repeat_back_to_back() {
        assert!(LINES.len() > 6, "too few lines to feel like a rotation");
        for hours in 0..(LINES.len() as i64 * 3) {
            assert_ne!(line_for(hours), line_for(hours + 1), "same line twice at hour {}", hours);
        }
        // A full turn of the list comes back round to the start.
        assert_eq!(line_for(0), line_for(LINES.len() as i64));
        assert_eq!(line_for(-1), line_for(LINES.len() as i64 - 1), "negative hours still land in the list");
    }

    #[test]
    fn the_post_lands_just_after_the_hour() {
        let at = |text: &str| DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc);
        assert_eq!(until_next_hour(at("2026-09-12T10:59:00Z")), Duration::from_secs(60) + PAST_THE_HOUR);
        assert_eq!(until_next_hour(at("2026-09-12T10:00:00Z")), Duration::from_secs(3600) + PAST_THE_HOUR);
        assert_eq!(until_next_hour(at("2026-09-12T10:30:00Z")), Duration::from_secs(1800) + PAST_THE_HOUR);
    }
}
