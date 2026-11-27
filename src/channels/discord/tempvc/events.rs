//! Persisted, guild-scoped scheduled temporary rooms.

use anyhow::{Context as _, Result};
use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};
use serenity::all::{
    ChannelId, ChannelType, CommandDataOption, CommandDataOptionValue, CommandOptionType, Context,
    CreateAllowedMentions, CreateCommandOption, CreateEmbed, CreateMessage, GuildId, HttpError,
    Member, Nonce, RoleId, UserId,
};

use super::{actions, caps, control, create_scheduled_room, db, enabled, store, user_error};
use crate::channels::discord::tempvc_panel::{Reply, option_value};

const LATE_WINDOW_SECONDS: i64 = 30 * 60;
const PAGE_SIZE: usize = 10;

pub(in super::super) fn event_command() -> CreateCommandOption {
    use CommandOptionType::{Integer, Role, String, SubCommand, SubCommandGroup};

    CreateCommandOption::new(SubCommandGroup, "event", "Schedule temporary voice rooms")
        .add_sub_option(
            CreateCommandOption::new(SubCommand, "schedule", "Open a room at a future time")
                .add_sub_option(
                    CreateCommandOption::new(
                        String,
                        "when",
                        "in 2h, 1d 30m, <t:UNIX>, or 2026-12-01 20:00 (IST)",
                    )
                    .max_length(64)
                    .required(true),
                )
                .add_sub_option(
                    CreateCommandOption::new(String, "name", "Name of the scheduled room")
                        .min_length(1)
                        .max_length(100)
                        .required(true),
                )
                .add_sub_option(
                    CreateCommandOption::new(Integer, "limit", "User limit (0 means unlimited)")
                        .min_int_value(0)
                        .max_int_value(99),
                )
                .add_sub_option(CreateCommandOption::new(
                    Role,
                    "role",
                    "Role to ping when the room opens (staff only)",
                )),
        )
        .add_sub_option(
            CreateCommandOption::new(SubCommand, "list", "See scheduled rooms and recent results")
                .add_sub_option(
                    CreateCommandOption::new(Integer, "page", "Which page to show")
                        .min_int_value(1),
                ),
        )
        .add_sub_option(
            CreateCommandOption::new(SubCommand, "cancel", "Cancel a scheduled room")
                .add_sub_option(
                    CreateCommandOption::new(Integer, "id", "Schedule ID from /vc event list")
                        .min_int_value(1)
                        .required(true),
                ),
        )
}

pub(in super::super) fn execute_event(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    options: &[CommandDataOption],
) -> Result<Reply> {
    actions::check_enabled()?;
    let actor = actions::actor(ctx, guild, member)?;
    let [sub] = options else {
        return Err(user_error("Choose a `/vc event` action."));
    };
    let CommandDataOptionValue::SubCommand(options) = &sub.value else {
        return Err(user_error("Choose a `/vc event` action."));
    };
    let value = |key| option_value(options, key);
    let int = |key| value(key).and_then(CommandDataOptionValue::as_i64);
    let string = |key| {
        value(key)
            .and_then(CommandDataOptionValue::as_str)
            .map(str::trim)
    };
    let creator = (!actor.staff).then_some(actor.id.get());
    match sub.name.as_str() {
        "schedule" => {
            let now = Utc::now().timestamp();
            let run_at = string("when").and_then(|when| parse_when(when, now))
                .ok_or_else(|| user_error("Try a time like `in 2h`, `1d 30m`, `<t:UNIX>`, or `2026-12-01 20:00` (IST)."))?;
            if run_at <= now {
                return Err(user_error("Choose a time in the future."));
            }
            let name = string("name")
                .filter(|name| !name.is_empty() && name.chars().count() <= 100)
                .ok_or_else(|| user_error("Use 1 to 100 characters for the room name."))?;
            let requested = int("limit").and_then(|limit| u32::try_from(limit).ok());
            let caps = caps::for_member(ctx, guild, member, actor.staff)?;
            let limit = requested.map(|limit| caps.limit(limit));
            let role = value("role").and_then(CommandDataOptionValue::as_role_id);
            if role.is_some() && !actor.staff {
                return Err(user_error("Only staff can ping a role."));
            }
            if let Some(role) = role
                && !ping_role_exists(ctx, guild, role)
            {
                return Err(user_error("Choose a server role other than @everyone."));
            }
            let hub = choose_hub(ctx, guild, member, None)?;
            let event = store::NewSchedule {
                guild_id: guild.get(),
                creator_id: actor.id.get(),
                run_at,
                name,
                limit,
                role_id: role.map(RoleId::get),
                hub_id: hub.map(ChannelId::get),
            };
            let id = store::add_schedule(&*db()?, &event)?;
            let mut reply = format!("📅 Room #{id} is scheduled for <t:{run_at}:F>.");
            if let Some(limit) = limit.filter(|&limit| Some(limit) != requested) {
                reply.push_str(&format!(" Your room limit is {limit}."));
            }
            Ok(Reply::Text(reply))
        }
        "list" => list_events(guild, creator, int("page")),
        "cancel" => {
            let id = int("id")
                .filter(|&id| id > 0)
                .ok_or_else(|| user_error("Get the schedule ID from `/vc event list`."))?;
            if !store::cancel_schedule(&*db()?, guild.get(), id, creator)? {
                return Err(user_error("You can't cancel this schedule."));
            }
            Ok(Reply::Text(format!("✅ Canceled schedule #{id}.")))
        }
        _ => Err(user_error("This `/vc event` action is unavailable.")),
    }
}

fn list_events(guild: GuildId, creator: Option<u64>, page: Option<i64>) -> Result<Reply> {
    let mut rows = store::list_schedules(&*db()?, guild.get())?;
    rows.retain(|event| creator.is_none_or(|user| event.creator_id == user));
    if rows.is_empty() {
        return Ok(Reply::Text("No rooms are scheduled.".into()));
    }
    let pages = rows.len().div_ceil(PAGE_SIZE);
    let page = page
        .and_then(|page| usize::try_from(page).ok())
        .unwrap_or(1)
        .clamp(1, pages);
    let lines = rows
        .chunks(PAGE_SIZE)
        .nth(page - 1)
        .unwrap_or_default()
        .iter()
        .map(|event| {
            let status = match event.status {
                store::ScheduleStatus::Pending => "Scheduled",
                store::ScheduleStatus::Open => "Started",
                store::ScheduleStatus::Missed => "Missed",
                store::ScheduleStatus::Cancelled => "Cancelled",
            };
            let name = event
                .name
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace('`', "'");
            format!(
                "`#{}` · {status} · `{name}` · <t:{}:F> · <@{}>",
                event.id, event.run_at, event.creator_id
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(Reply::info(
        CreateEmbed::new()
            .title(format!("📅 Temp voice events · {page}/{pages}"))
            .description(lines),
    ))
}

fn choose_hub(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    preferred: Option<u64>,
) -> Result<Option<ChannelId>> {
    let (mut hubs, has_category) = {
        let cached = ctx
            .cache
            .guild(guild)
            .context("guild channels unavailable")?;
        let hubs = control::ids("VIZIER_TEMPVC_HUBS")
            .into_iter()
            .filter(|&id| id != 0)
            .map(ChannelId::new)
            .filter(|id| {
                cached
                    .channels
                    .get(id)
                    .is_some_and(|channel| channel.kind == ChannelType::Voice)
            })
            .collect::<Vec<_>>();
        let has_category = control::id("VIZIER_TEMPVC_CATEGORY")
            .filter(|&id| id != 0)
            .and_then(|id| cached.channels.get(&ChannelId::new(id)))
            .is_some_and(|channel| channel.kind == ChannelType::Category);
        (hubs, has_category)
    };
    if hubs.is_empty() {
        return if has_category {
            Ok(None)
        } else {
            Err(user_error(
                "Ask staff to set up a temporary voice hub or category.",
            ))
        };
    }
    hubs.sort_by_key(|id| Some(id.get()) != preferred);
    let conn = db()?;
    for hub in hubs {
        let required = store::hub_template(&conn, guild.get(), hub.get())?
            .and_then(|hub| hub.required_role_id);
        if caps::has_required_role(member, required) {
            return Ok(Some(hub));
        }
    }
    Err(user_error(
        "You need a role that gives you access to a temporary voice hub.",
    ))
}

fn parse_when(input: &str, now: i64) -> Option<i64> {
    let input = input.trim();
    if let Some(stamp) = input
        .strip_prefix("<t:")
        .and_then(|stamp| stamp.strip_suffix('>'))
    {
        let digits = match stamp.split_once(':') {
            Some((digits, "t" | "T" | "d" | "D" | "f" | "F" | "R")) => digits,
            Some(_) => return None,
            None => stamp,
        };
        if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        return DateTime::from_timestamp(digits.parse().ok()?, 0).map(|time| time.timestamp());
    }
    for pattern in ["%Y-%m-%d %H:%M", "%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(local) = NaiveDateTime::parse_from_str(input, pattern) {
            return local
                .and_local_timezone(FixedOffset::east_opt(5 * 3600 + 30 * 60)?)
                .single()
                .map(|time| time.timestamp());
        }
    }
    let run_at = now.checked_add(parse_relative(input)?)?;
    DateTime::from_timestamp(run_at, 0).map(|time| time.timestamp())
}

fn parse_relative(input: &str) -> Option<i64> {
    let mut rest = input;
    if rest
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("in"))
    {
        rest = rest
            .get(2..)?
            .strip_prefix(char::is_whitespace)?
            .trim_start();
    }
    let mut seconds = 0_i64;
    let mut last_unit = i64::MAX;
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let (amount, tail) = rest.split_at(digits);
        let amount = amount.parse::<i64>().ok()?;
        let tail = tail.trim_start();
        let unit = match tail.as_bytes().first()?.to_ascii_lowercase() {
            b'd' => 86_400,
            b'h' => 3_600,
            b'm' => 60,
            _ => return None,
        };
        if unit >= last_unit {
            return None;
        }
        last_unit = unit;
        seconds = seconds.checked_add(amount.checked_mul(unit)?)?;
        rest = tail.get(1..)?.trim_start();
    }
    (seconds > 0).then_some(seconds)
}

#[derive(Debug, PartialEq, Eq)]
enum Due {
    Pending,
    Open,
    Missed,
}

fn due(run_at: i64, now: i64) -> Due {
    if now < run_at {
        Due::Pending
    } else if now.saturating_sub(run_at) > LATE_WINDOW_SECONDS {
        Due::Missed
    } else {
        Due::Open
    }
}

pub(super) async fn tick(ctx: &Context) {
    if !enabled() || store::db().is_none() {
        return;
    }
    for guild in ctx.cache.guilds() {
        if let Err(err) = tick_guild(ctx, guild).await {
            tracing::warn!(%guild, %err, "tempvc: scheduled room tick failed");
        }
    }
}

async fn tick_guild(ctx: &Context, guild: GuildId) -> Result<()> {
    let rows = store::due_schedules(&*db()?, guild.get(), Utc::now().timestamp())?;
    for mut event in rows {
        if !enabled() {
            return Ok(());
        }
        let id = event.id;
        let result = match due(event.run_at, Utc::now().timestamp()) {
            Due::Pending => continue,
            Due::Missed => mark_missed(guild, id),
            Due::Open => open_event(ctx, guild, &mut event).await,
        };
        if let Err(err) = result {
            tracing::warn!(%guild, %id, %err, "tempvc: scheduled room could not open");
        }
    }

    let cutoff = Utc::now().timestamp() - LATE_WINDOW_SECONDS;
    let unsent = store::unannounced_schedules(&*db()?, guild.get(), cutoff)?;
    for event in unsent {
        if !enabled() {
            return Ok(());
        }
        if let Err(err) = announce_event(ctx, guild, &event).await {
            tracing::warn!(%guild, id = event.id, %err, "tempvc: scheduled room announcement failed");
        }
    }
    Ok(())
}

fn mark_missed(guild: GuildId, id: i64) -> Result<()> {
    store::mark_schedule_missed(&*db()?, guild.get(), id)?;
    Ok(())
}

async fn open_event(ctx: &Context, guild: GuildId, event: &mut store::Schedule) -> Result<()> {
    let creator = UserId::new(event.creator_id);
    let Some(member) = actions::member_if_present(ctx, guild, creator).await? else {
        return mark_missed(guild, event.id);
    };
    if !enabled() {
        return Ok(());
    }
    event.hub_id = choose_hub(ctx, guild, &member, event.hub_id)?.map(ChannelId::get);
    let role = if actions::actor(ctx, guild, &member)?.staff {
        event_role(ctx, guild, event)
    } else {
        None
    };
    // If the owner is at their room limit, leave the schedule pending.
    // The next tick can open it if a room frees up within the late window.
    create_scheduled_room(ctx, guild, &member, event, role)
        .await
        .map(|_| ())
}

fn ping_role_exists(ctx: &Context, guild: GuildId, role: RoleId) -> bool {
    role != guild.everyone_role()
        && ctx
            .cache
            .guild(guild)
            .is_some_and(|cached| cached.roles.contains_key(&role))
}

fn event_role(ctx: &Context, guild: GuildId, event: &store::Schedule) -> Option<RoleId> {
    event
        .role_id
        .filter(|&role| role != 0)
        .map(RoleId::new)
        .filter(|&role| ping_role_exists(ctx, guild, role))
}

async fn announce_event(ctx: &Context, guild: GuildId, event: &store::Schedule) -> Result<()> {
    let channel = ChannelId::new(
        event
            .channel_id
            .context("scheduled room channel unavailable")?,
    );
    let role = if let Some(role) = event_role(ctx, guild, event) {
        match ctx
            .http
            .get_member(guild, UserId::new(event.creator_id))
            .await
        {
            Ok(member) => actions::actor(ctx, guild, &member)?.staff.then_some(role),
            Err(serenity::Error::Http(HttpError::UnsuccessfulRequest(response)))
                if response.error.code == 10007 =>
            {
                None
            }
            Err(err) => return Err(err.into()),
        }
    } else {
        None
    };
    channel
        .send_message(&ctx.http, announcement(event.id, role))
        .await?;
    store::mark_schedule_announced(&*db()?, guild.get(), event.id)?;
    Ok(())
}

fn announcement(id: i64, role: Option<RoleId>) -> CreateMessage {
    let mut content = format!("📅 Scheduled voice room #{id} is open. Come join!");
    if let Some(role) = role {
        content.push_str(&format!(" <@&{role}>"));
    }
    CreateMessage::new()
        .content(content)
        .allowed_mentions(CreateAllowedMentions::new().roles(role).replied_user(false))
        .nonce(Nonce::String(format!("vce{id:x}")))
        .enforce_nonce(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_schedule_times_are_ist() {
        assert_eq!(parse_when("2026-12-01 20:00", 0), Some(1_796_135_400));
        assert_eq!(parse_when("2026-12-02 00:30", 0), Some(1_796_151_600));
        assert_eq!(parse_when("2026-02-29 20:00", 0), None);
    }

    #[test]
    fn parses_relative_times_and_discord_timestamps() {
        assert_eq!(parse_when("in 2h", 10), Some(7_210));
        assert_eq!(parse_when("1d 30m", 10), Some(88_210));
        assert_eq!(parse_when("IN 1h 30m", 10), Some(5_410));
        assert_eq!(parse_when("<t:1796135400>", 0), Some(1_796_135_400));
        assert_eq!(parse_when("<t:1796135400:R>", 0), Some(1_796_135_400));
        for invalid in [
            "",
            "in",
            "0m",
            "1h 2h",
            "1x",
            "2hxyz",
            "<t:42:XX>",
            "<t:42:>",
            "<t:+42>",
            "9223372036854775807d",
        ] {
            assert_eq!(parse_when(invalid, 0), None, "{invalid}");
        }
    }

    #[test]
    fn opens_after_downtime_through_thirty_minutes_then_marks_missed() {
        let run_at = 2_000;
        assert_eq!(due(run_at, run_at - 1), Due::Pending);
        assert_eq!(due(run_at, run_at), Due::Open);
        assert_eq!(due(run_at, run_at + 1_800), Due::Open);
        assert_eq!(due(run_at, run_at + 1_801), Due::Missed);
    }

    #[test]
    fn announcement_can_ping_only_its_selected_role() {
        let message = serde_json::to_value(announcement(1, Some(RoleId::new(2)))).unwrap();
        assert_eq!(
            message["allowed_mentions"],
            serde_json::json!({
                "parse": [], "roles": ["2"], "users": [], "replied_user": false,
            })
        );
        assert_eq!(message["enforce_nonce"], true);
        assert!(message["content"].as_str().unwrap().ends_with("<@&2>"));

        let message = serde_json::to_value(announcement(1, None)).unwrap();
        assert_eq!(message["allowed_mentions"]["roles"], serde_json::json!([]));
        assert!(!message["content"].as_str().unwrap().contains("<@"));
    }
}
