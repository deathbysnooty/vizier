//! Time spent in temporary rooms, with periodic checkpoints in tempvc.db.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, CommandOptionType, Context, CreateCommandOption, CreateEmbed, GuildId, UserId,
};

use super::{Occupants, db, enabled, occupants, store};
use crate::channels::discord::tempvc_panel::Reply;

#[derive(Clone, Copy)]
struct Sample {
    tick: Instant,
    wall_ms: i64,
}

impl Sample {
    fn now() -> Self {
        Self {
            tick: Instant::now(),
            wall_ms: Utc::now().timestamp_millis(),
        }
    }
}

struct Session {
    channel: ChannelId,
    cursor: Sample,
}

type MemberKey = (GuildId, UserId);

#[derive(Debug, PartialEq, Eq)]
struct Span {
    member: MemberKey,
    start_ms: i64,
    end_ms: i64,
}

impl Session {
    fn checkpoint(&mut self, member: MemberKey, now: Sample, pending: &mut Vec<Span>) {
        let elapsed = now.tick.saturating_duration_since(self.cursor.tick);
        let elapsed_ms = elapsed.as_millis() as u64;
        if elapsed_ms == 0 {
            return;
        }
        let end_ms = self.cursor.wall_ms + elapsed_ms as i64;
        pending.push(Span {
            member,
            start_ms: self.cursor.wall_ms,
            end_ms,
        });
        self.cursor = Sample {
            tick: self.cursor.tick + Duration::from_millis(elapsed_ms),
            wall_ms: end_ms,
        };
    }
}

#[derive(Default)]
struct Tracker {
    active: HashMap<MemberKey, Session>,
    pending: Vec<Span>,
}

impl Tracker {
    fn presence(&mut self, guild: GuildId, channel: ChannelId, humans: &[UserId], now: Sample) {
        self.active.retain(|&member, session| {
            let left =
                member.0 == guild && session.channel == channel && !humans.contains(&member.1);
            if left {
                session.checkpoint(member, now, &mut self.pending);
            }
            !left
        });
        for &user in humans {
            let member = (guild, user);
            let session = Session {
                channel,
                cursor: now,
            };
            match self.active.entry(member) {
                Entry::Vacant(entry) => {
                    entry.insert(session);
                }
                Entry::Occupied(mut entry) if entry.get().channel != channel => {
                    entry.get_mut().checkpoint(member, now, &mut self.pending);
                    entry.insert(session);
                }
                Entry::Occupied(_) => (),
            }
        }
    }

    fn checkpoint(&mut self, now: Sample) {
        for (&member, session) in &mut self.active {
            session.checkpoint(member, now, &mut self.pending);
        }
    }

    fn save(&mut self) -> Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let conn = db()?;
        let tx = conn.unchecked_transaction()?;
        for span in &self.pending {
            let (guild, user) = span.member;
            store::add_voice_time(&tx, guild.get(), user.get(), span.start_ms, span.end_ms)?;
        }
        tx.commit()?;
        self.pending.clear();
        Ok(())
    }
}

static TRACKER: LazyLock<Mutex<Tracker>> = LazyLock::new(Mutex::default);

pub(super) fn observe(ctx: &Context, guild: GuildId, channel: ChannelId) -> Option<Occupants> {
    let mut tracker = TRACKER.lock();
    // Get the cache snapshot under this lock so a concurrent room move cannot
    // apply an older snapshot after the member's new session has started.
    let occupants = occupants(ctx, guild, channel)?;
    if enabled() {
        record_presence(&mut tracker, guild, channel, &occupants.humans);
    }
    Some(occupants)
}

fn record_presence(tracker: &mut Tracker, guild: GuildId, channel: ChannelId, humans: &[UserId]) {
    tracker.presence(guild, channel, humans, Sample::now());
    if let Err(err) = tracker.save() {
        tracing::warn!(%guild, %channel, %err, "tempvc: could not save voice presence");
    }
}

pub(super) fn close(guild: GuildId, channel: ChannelId) {
    record_presence(&mut TRACKER.lock(), guild, channel, &[]);
}

pub(super) fn clear() -> Result<()> {
    let mut tracker = TRACKER.lock();
    tracker.checkpoint(Sample::now());
    tracker.active.clear();
    tracker.save()
}

pub(super) fn flush() -> Result<()> {
    let mut tracker = TRACKER.lock();
    tracker.checkpoint(Sample::now());
    tracker.save()
}

pub(in super::super) fn stats_command() -> CreateCommandOption {
    CreateCommandOption::new(
        CommandOptionType::SubCommand,
        "stats",
        "See temp voice time and rooms created",
    )
    .add_sub_option(CreateCommandOption::new(
        CommandOptionType::User,
        "member",
        "Whose stats to show (default: you)",
    ))
}

pub(in super::super) fn top_command() -> CreateCommandOption {
    CreateCommandOption::new(
        CommandOptionType::SubCommand,
        "top",
        "See the top 10 by temp voice time",
    )
}

pub(super) fn execute_stats(guild: GuildId, member: UserId) -> Result<Reply> {
    flush()?;
    let month = store::month_key(Utc::now().timestamp_millis())?;
    let conn = db()?;
    let total = store::stats(&conn, guild.get(), member.get(), None)?;
    let monthly = store::stats(&conn, guild.get(), member.get(), Some(month))?;
    Ok(Reply::info(
        CreateEmbed::new()
            .title("🎧 Temp voice stats")
            .description(format!("<@{member}>"))
            .field("This month (IST)", stats_text(monthly), false)
            .field("All time", stats_text(total), false),
    ))
}

pub(super) fn execute_top(guild: GuildId) -> Result<Reply> {
    flush()?;
    let month = store::month_key(Utc::now().timestamp_millis())?;
    let conn = db()?;
    let total = store::top_stats(&conn, guild.get(), None)?;
    let monthly = store::top_stats(&conn, guild.get(), Some(month))?;
    Ok(Reply::info(
        CreateEmbed::new()
            .title("🎧 Temp voice top 10")
            .field("This month (IST)", top_text(&monthly), false)
            .field("All time", top_text(&total), false),
    ))
}

fn stats_text(stats: store::VoiceStats) -> String {
    format!(
        "**{}** in temp VCs · **{}** rooms created",
        duration(stats.time_ms),
        stats.rooms_created
    )
}

fn top_text(rows: &[(u64, store::VoiceStats)]) -> String {
    if rows.is_empty() {
        return "No voice time recorded yet.".into();
    }
    rows.iter()
        .enumerate()
        .map(|(index, (user, stats))| {
            format!("{}. <@{user}> · {}", index + 1, duration(stats.time_ms))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    let hours = seconds / 3_600;
    let minutes = seconds / 60 % 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_then_move_and_leave_charge_each_millisecond_once() {
        let guild = GuildId::new(1);
        let user = UserId::new(2);
        let first = ChannelId::new(3);
        let second = ChannelId::new(4);
        let start = Instant::now();
        let sample = |ms| Sample {
            tick: start + Duration::from_millis(ms),
            wall_ms: 1_000 + ms as i64,
        };
        let mut tracker = Tracker::default();
        tracker.presence(guild, first, &[user], sample(0));
        tracker.checkpoint(sample(1_000));
        tracker.presence(guild, second, &[user], sample(2_000));
        tracker.presence(guild, first, &[], sample(2_000));
        tracker.presence(guild, second, &[], sample(3_000));
        tracker.checkpoint(sample(4_000));

        assert!(tracker.active.is_empty());
        assert_eq!(tracker.pending.len(), 3);
        for (index, span) in tracker.pending.iter().enumerate() {
            assert_eq!(span.member, (guild, user));
            assert_eq!(span.start_ms, 1_000 + index as i64 * 1_000);
            assert_eq!(span.end_ms - span.start_ms, 1_000);
        }
    }
}
