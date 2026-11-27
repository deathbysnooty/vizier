//! Join-to-create and lifecycle for the temporary rooms recorded in tempvc.db.

mod access;
mod actions;
pub(super) mod admin;
mod audit;
mod caps;
mod events;
mod knock;
mod usage;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Weak};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, ChannelType, Context, CreateChannel, GuildChannel, GuildId, HttpError, Member,
    PermissionOverwrite, PermissionOverwriteType, Permissions, RoleId, UserId, VoiceState,
};
use tokio::sync::{Mutex as AsyncMutex, Notify, OwnedMutexGuard};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

use super::control;
use super::tempvc_panel;
use super::tempvc_store::{self as store, Profile, Room};

pub(super) use actions::{can_control, execute};
pub(super) use events::{event_command, execute_event};
pub(super) use knock::answer_knock;
pub(super) use usage::{stats_command, top_command};

const DEFAULT_NAME: &str = "{user}'s room";
const COOLDOWN: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_secs(15);
const EVENT_GRACE: Duration = Duration::from_secs(30 * 60);

#[derive(Debug)]
pub(super) struct UserError(pub String);

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UserError {}

fn user_error(message: impl Into<String>) -> anyhow::Error {
    UserError(message.into()).into()
}

fn enabled() -> bool {
    control::on("VIZIER_TEMPVC", false)
}

static ENABLE_CHANGED: Notify = Notify::const_new();

pub(super) fn enabled_setting_changed() {
    ENABLE_CHANGED.notify_one();
}

fn db() -> Result<parking_lot::MutexGuard<'static, rusqlite::Connection>> {
    store::db()
        .map(Mutex::lock)
        .context("tempvc database is unavailable")
}

fn lookup(guild: GuildId, channel: ChannelId) -> Result<Option<Room>> {
    Ok(store::room(&*db()?, channel.get())?.filter(|room| room.guild_id == guild.get()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Job {
    Delete,
    Handoff,
}

impl Job {
    fn delay(self) -> Duration {
        let seconds = match self {
            Self::Delete => control::number("VIZIER_TEMPVC_GRACE_SECONDS", 20),
            Self::Handoff => control::number("VIZIER_TEMPVC_HANDOFF_SECONDS", 60),
        };
        Duration::from_secs(seconds.min(86_400))
    }
}

#[derive(Default)]
struct RoomState {
    joined: HashMap<UserId, Instant>,
    lifecycle: Option<(Job, JoinHandle<()>)>,
    rename: Option<JoinHandle<()>>,
    knocks: knock::Knocks,
}

impl RoomState {
    fn cancel_lifecycle(&mut self) {
        if let Some((_, task)) = self.lifecycle.take() {
            task.abort();
        }
    }

    fn cancel_handoff(&mut self) {
        if self
            .lifecycle
            .as_ref()
            .is_some_and(|(job, _)| *job == Job::Handoff)
        {
            self.cancel_lifecycle();
        }
    }
}

type RoomLock = Arc<AsyncMutex<RoomState>>;
static ROOMS: LazyLock<Mutex<HashMap<ChannelId, RoomLock>>> = LazyLock::new(Mutex::default);

fn room_state(channel: ChannelId) -> RoomLock {
    ROOMS.lock().entry(channel).or_default().clone()
}

fn voice(ctx: &Context, guild: GuildId, user: UserId) -> Option<ChannelId> {
    ctx.cache.guild(guild)?.voice_states.get(&user)?.channel_id
}

struct Occupants {
    all: Vec<UserId>,
    humans: Vec<UserId>,
}

fn occupants(ctx: &Context, guild: GuildId, channel: ChannelId) -> Option<Occupants> {
    let guild = ctx.cache.guild(guild)?;
    let mut occupants = Occupants {
        all: Vec::new(),
        humans: Vec::new(),
    };
    for (user, state) in &guild.voice_states {
        if state.channel_id == Some(channel) {
            occupants.all.push(*user);
            let member = state.member.as_ref().or_else(|| guild.members.get(user));
            if member.is_some_and(|member| !member.user.bot) {
                occupants.humans.push(*user);
            }
        }
    }
    Some(occupants)
}

fn desired_job(occupants: &Occupants, owner: UserId) -> Option<Job> {
    if occupants.all.is_empty() {
        Some(Job::Delete)
    } else if !occupants.all.contains(&owner) && !occupants.humans.is_empty() {
        Some(Job::Handoff)
    } else {
        None
    }
}

fn job_changed(current: Option<Job>, occupants: &Occupants, owner: UserId) -> Option<Option<Job>> {
    let wanted = desired_job(occupants, owner);
    (current != wanted).then_some(wanted)
}

fn handoff_candidates(
    humans: &[UserId],
    joined: &HashMap<UserId, Instant>,
    banned: &[u64],
) -> Vec<UserId> {
    // The gateway does not report join times for people already present on boot.
    // Break ties stably by user ID when their original order is unknown.
    let mut candidates = humans
        .iter()
        .copied()
        .filter(|user| banned.binary_search(&user.get()).is_err())
        .collect::<Vec<_>>();
    candidates.sort_unstable_by_key(|user| (joined.get(user).is_none(), joined.get(user), *user));
    candidates
}

fn unknown_channel(err: &serenity::Error) -> bool {
    matches!(err, serenity::Error::Http(HttpError::UnsuccessfulRequest(response)) if response.error.code == 10003)
}

async fn fetch_voice(
    ctx: &Context,
    guild: GuildId,
    channel: ChannelId,
) -> Result<Option<GuildChannel>> {
    match ctx.http.get_channel(channel).await {
        Ok(response) => Ok(response
            .guild()
            .filter(|ch| ch.guild_id == guild && ch.kind == ChannelType::Voice)),
        Err(err) if unknown_channel(&err) => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn remove_room(room: Room, state: &mut RoomState) -> Result<bool> {
    let channel = ChannelId::new(room.channel_id);
    let removed = store::delete(&*db()?, room.channel_id)?;
    usage::close(GuildId::new(room.guild_id), channel);
    state.cancel_lifecycle();
    if let Some(task) = state.rename.take() {
        task.abort();
    }
    ROOMS.lock().remove(&channel);
    Ok(removed)
}

fn forget_room(ctx: &Context, room: Room, state: &mut RoomState) -> Result<()> {
    log_deletion(ctx, room, state, None, false)
}

fn log_deletion(
    ctx: &Context,
    room: Room,
    state: &mut RoomState,
    actor: Option<UserId>,
    staff: bool,
) -> Result<()> {
    if remove_room(room, state)? {
        audit::post(
            ctx,
            GuildId::new(room.guild_id),
            if staff {
                audit::Action::StaffDelete
            } else {
                audit::Action::Deleted
            },
            actor,
            Some(ChannelId::new(room.channel_id)),
            Some(UserId::new(room.owner_id)),
        );
    }
    Ok(())
}

async fn delete_room(
    ctx: &Context,
    room: Room,
    state: &mut RoomState,
    reason: &str,
    actor: Option<UserId>,
    staff: bool,
) -> Result<()> {
    let channel = ChannelId::new(room.channel_id);
    match ctx.http.delete_channel(channel, Some(reason)).await {
        Ok(_) => (),
        Err(err) if unknown_channel(&err) => (),
        Err(err) => return Err(err.into()),
    }
    log_deletion(ctx, room, state, actor, staff)
}

fn scheduled_grace(room: Room) -> Result<Duration> {
    let opened = store::room_schedule_opened_at(&*db()?, room.guild_id, room.channel_id)?;
    let now = chrono::Utc::now().timestamp();
    Ok(opened.map_or(Duration::ZERO, |opened| {
        let remaining = opened
            .saturating_add(EVENT_GRACE.as_secs() as i64)
            .saturating_sub(now);
        Duration::from_secs(remaining.clamp(0, EVENT_GRACE.as_secs() as i64) as u64)
    }))
}

fn reconcile(
    ctx: &Context,
    room: Room,
    state: &mut RoomState,
    room_lock: &RoomLock,
    joined: Option<(UserId, Instant)>,
    empty_delay: Duration,
) {
    let guild = GuildId::new(room.guild_id);
    let channel = ChannelId::new(room.channel_id);
    let Some(occupants) = usage::observe(ctx, guild, channel) else {
        return;
    };
    let now = Instant::now();
    if let Some((user, _)) = joined
        && state
            .lifecycle
            .as_ref()
            .is_some_and(|(job, _)| *job == Job::Delete || user.get() == room.owner_id)
    {
        // Even a brief rejoin ends the previous empty or owner-away stretch.
        state.cancel_lifecycle();
    }
    state
        .joined
        .retain(|user, _| occupants.humans.contains(user));
    if let Some((user, when)) = joined.filter(|(user, _)| occupants.humans.contains(user)) {
        state.joined.insert(user, when);
    }
    for user in &occupants.humans {
        state.joined.entry(*user).or_insert(now);
    }
    let current = state.lifecycle.as_ref().map(|(job, _)| *job);
    if let Some(change) = job_changed(current, &occupants, UserId::new(room.owner_id)) {
        state.cancel_lifecycle();
        if let Some(job) = change {
            let delay = if job == Job::Delete {
                empty_delay
            } else {
                job.delay()
            };
            let delay = match scheduled_grace(room) {
                Ok(grace) => delay.max(grace),
                Err(err) => {
                    tracing::warn!(%channel, %err, "tempvc: could not read scheduled room grace");
                    delay
                }
            };
            let ctx = ctx.clone();
            let room_lock = room_lock.clone();
            let task = tokio::spawn(run_job(ctx, guild, channel, room_lock, job, delay));
            state.lifecycle = Some((job, task));
        }
    }
}

async fn run_job(
    ctx: Context,
    guild: GuildId,
    channel: ChannelId,
    room_lock: RoomLock,
    job: Job,
    delay: Duration,
) {
    sleep(delay).await;
    loop {
        let mut state = room_lock.lock().await;
        let handle = state.lifecycle.take();
        match finish_job(&ctx, guild, channel, &mut state, job).await {
            Ok(true) => return,
            Ok(false) => (),
            Err(err) => tracing::warn!(%channel, ?job, %err, "tempvc: room timer failed"),
        }
        state.lifecycle = handle;
        drop(state);
        sleep(RETRY).await;
    }
}

async fn finish_job(
    ctx: &Context,
    guild: GuildId,
    channel: ChannelId,
    state: &mut RoomState,
    job: Job,
) -> Result<bool> {
    if !enabled() {
        return Ok(true);
    }
    let Some(room) = lookup(guild, channel)? else {
        return Ok(true);
    };
    let owner = UserId::new(room.owner_id);
    let current = occupants(ctx, guild, channel).context("guild voice state unavailable")?;
    if desired_job(&current, owner) != Some(job) {
        return Ok(true);
    }
    match job {
        Job::Delete => delete_room(ctx, room, state, "Temp voice room empty", None, false).await?,
        Job::Handoff => {
            let Some(ch) = fetch_voice(ctx, guild, channel).await? else {
                forget_room(ctx, room, state)?;
                return Ok(true);
            };
            // Check again after the REST lookup, in case someone just returned.
            let current =
                occupants(ctx, guild, channel).context("guild voice state unavailable")?;
            if desired_job(&current, owner) == Some(Job::Handoff) {
                let bans = store::access_list(
                    &*db()?,
                    room.guild_id,
                    room.owner_id,
                    store::Access::Banned,
                )?;
                let candidates = handoff_candidates(&current.humans, &state.joined, &bans);
                if candidates.is_empty() {
                    for user in current.humans {
                        access::disconnect_if_present(ctx, guild, channel, user).await?;
                    }
                    return Ok(true);
                }
                for next in candidates {
                    let Some(member) = actions::member_if_present(ctx, guild, next).await? else {
                        continue;
                    };
                    let receiver = actions::actor(ctx, guild, &member)?;
                    if voice(ctx, guild, owner) == Some(channel) {
                        return Ok(true);
                    }
                    if receiver.voice != Some(channel)
                        || !actions::give_ownership(
                            ctx,
                            &ch,
                            room,
                            receiver,
                            actions::OwnershipChange::Handoff,
                        )
                        .await?
                    {
                        continue;
                    }
                    audit::post(
                        ctx,
                        guild,
                        audit::Action::Handoff,
                        None,
                        Some(channel),
                        Some(next),
                    );
                    actions::announce_owner(ctx, channel, next).await;
                    return Ok(true);
                }
                // Try again if everyone eligible is already at their room limit.
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(super) fn on_cache_ready(ctx: &Context, guilds: Vec<GuildId>) {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    let start_watch = !WATCHING.swap(true, Ordering::Relaxed);
    if start_watch {
        let ctx = ctx.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(15));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                if enabled() {
                    events::tick(&ctx).await;
                }
            }
        });
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(30));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                let result = if enabled() {
                    usage::flush()
                } else {
                    usage::clear()
                };
                if let Err(err) = result {
                    tracing::warn!(%err, "tempvc: usage flush failed");
                }
            }
        });
    }
    let ctx = ctx.clone();
    tokio::spawn(async move {
        if enabled() {
            recover(&ctx, guilds).await;
        }
        if start_watch {
            loop {
                ENABLE_CHANGED.notified().await;
                if enabled() {
                    recover(&ctx, ctx.cache.guilds()).await;
                } else if let Err(err) = usage::clear() {
                    tracing::warn!(%err, "tempvc: could not close active usage spans");
                }
            }
        }
    });
}

async fn recover(ctx: &Context, guilds: Vec<GuildId>) {
    for guild in guilds {
        if let Err(err) = recover_guild(ctx, guild).await {
            tracing::warn!(%guild, %err, "tempvc: recovery failed");
        }
    }
}

async fn recover_guild(ctx: &Context, guild: GuildId) -> Result<()> {
    let rooms = store::guild_rooms(&*db()?, guild.get())?;
    for row in rooms {
        let channel = ChannelId::new(row.channel_id);
        if let Err(err) = recover_room(ctx, guild, channel).await {
            tracing::warn!(%channel, %err, "tempvc: could not recover room");
        }
    }
    Ok(())
}

async fn recover_room(ctx: &Context, guild: GuildId, channel: ChannelId) -> Result<()> {
    let room_lock = room_state(channel);
    let mut state = room_lock.lock().await;
    let Some(room) = lookup(guild, channel)? else {
        return Ok(());
    };
    reconcile(ctx, room, &mut state, &room_lock, None, Duration::ZERO);
    let fetched = fetch_voice(ctx, guild, channel).await;
    if matches!(fetched, Ok(None)) {
        return forget_room(ctx, room, &mut state);
    }
    actions::restore_rename(ctx, channel, &mut state, &room_lock)?;
    if let Some(ch) = fetched? {
        access::sync(
            ctx,
            &ch,
            UserId::new(room.owner_id),
            ch.permission_overwrites.clone(),
        )
        .await?;
    }
    Ok(())
}

pub(super) fn on_member_join(ctx: &Context, guild: GuildId, user: UserId) {
    if !enabled() || store::db().is_none() {
        return;
    }
    let rooms = match db()
        .and_then(|conn| Ok(store::rooms_listing_member(&conn, guild.get(), user.get())?))
    {
        Ok(rooms) if rooms.is_empty() => return,
        Ok(rooms) => rooms,
        Err(err) => {
            tracing::warn!(%guild, %user, %err, "tempvc: could not look up returning member's access");
            return;
        }
    };
    let ctx = ctx.clone();
    tokio::spawn(async move {
        for room in rooms {
            let channel = ChannelId::new(room.channel_id);
            if let Err(err) =
                access::sync_owned_room(&ctx, guild, UserId::new(room.owner_id), channel).await
            {
                tracing::warn!(%channel, %err, "tempvc: could not sync returning member's access");
            }
        }
    });
}

/// Whether a member counts as temp voice staff (Discord admins and the
/// "Temp voice staff roles" setting). DragMe uses it so staff can answer any
/// request from anywhere.
pub(super) fn is_staff(ctx: &Context, guild: GuildId, member: &serenity::all::Member) -> bool {
    actions::actor(ctx, guild, member).map(|actor| actor.staff).unwrap_or(false)
}

pub(super) fn on_voice_state(ctx: &Context, old: Option<&VoiceState>, new: &VoiceState) {
    let old = old.and_then(|state| state.channel_id);
    let new_channel = new.channel_id;
    let Some(guild) = new.guild_id else { return };
    if old == new_channel || !enabled() || store::db().is_none() {
        return;
    }
    let ctx = ctx.clone();
    let user = new.user_id;
    let received = Instant::now();
    tokio::spawn(async move {
        for channel in [old, new_channel].into_iter().flatten() {
            let joined = (new_channel == Some(channel)).then_some((user, received));
            if let Err(err) = update_room(&ctx, guild, channel, joined).await {
                tracing::warn!(%channel, %err, "tempvc: could not update room presence");
            }
        }
        if let Some(hub) =
            new_channel.filter(|id| control::ids("VIZIER_TEMPVC_HUBS").contains(&id.get()))
            && let Err(err) = join_hub(&ctx, guild, hub, user).await
            && !err.is::<UserError>()
        {
            tracing::warn!(%hub, %user, %err, "tempvc: hub join failed");
        }
    });
}

pub(super) fn on_channel_delete(ctx: &Context, channel: &GuildChannel) {
    let id = channel.id;
    let guild = channel.guild_id;
    if channel.kind != ChannelType::Voice || !matches!(lookup(guild, id), Ok(Some(_))) {
        return;
    }
    queue_forget_channel(ctx, guild, id);
}

fn forget_channel(
    ctx: &Context,
    guild: GuildId,
    channel: ChannelId,
    state: &mut RoomState,
) -> Result<()> {
    if let Some(room) = lookup(guild, channel)? {
        forget_room(ctx, room, state)?;
    }
    Ok(())
}

fn queue_forget_channel(ctx: &Context, guild: GuildId, channel: ChannelId) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let room_lock = room_state(channel);
        let mut state = room_lock.lock().await;
        if let Err(err) = forget_channel(&ctx, guild, channel, &mut state) {
            tracing::warn!(%channel, %err, "tempvc: could not remove deleted room");
        }
    });
}

async fn update_room(
    ctx: &Context,
    guild: GuildId,
    channel: ChannelId,
    joined: Option<(UserId, Instant)>,
) -> Result<()> {
    if lookup(guild, channel)?.is_some() {
        let room_lock = room_state(channel);
        let mut state = room_lock.lock().await;
        if let Some(room) = lookup(guild, channel)? {
            let mut disconnected = Ok(());
            if let Some((user, _)) = joined
                && store::is_banned(&*db()?, room.guild_id, room.owner_id, user.get())?
            {
                disconnected = access::disconnect_if_present(ctx, guild, channel, user).await;
            }
            reconcile(
                ctx,
                room,
                &mut state,
                &room_lock,
                joined,
                Job::Delete.delay(),
            );
            disconnected?;
        }
    }
    Ok(())
}

struct Creation {
    started: Instant,
    in_progress: bool,
}

type Creator = (GuildId, UserId);
static CREATIONS: LazyLock<Mutex<HashMap<Creator, Creation>>> = LazyLock::new(Mutex::default);
static OWNERS: LazyLock<Mutex<HashMap<Creator, Weak<AsyncMutex<()>>>>> = LazyLock::new(Mutex::default);

// Serialize operations that add a room to the same owner's roster. Transfers
// already hold the source room lock, so counting must never wait for a room lock.
async fn owner_lock(guild: GuildId, user: UserId) -> OwnedMutexGuard<()> {
    let lock = {
        let mut owners = OWNERS.lock();
        owners.retain(|_, lock| lock.strong_count() > 0);
        let entry = owners.entry((guild, user)).or_default();
        entry.upgrade().unwrap_or_else(|| {
            let lock = Arc::new(AsyncMutex::new(()));
            *entry = Arc::downgrade(&lock);
            lock
        })
    };
    lock.lock_owned().await
}

struct CreationGuard(Creator);

impl CreationGuard {
    fn start(guild: GuildId, user: UserId) -> Option<Self> {
        let now = Instant::now();
        let mut creations = CREATIONS.lock();
        creations.retain(|_, creation| {
            creation.in_progress || now.duration_since(creation.started) < COOLDOWN
        });
        let key = (guild, user);
        if creations.contains_key(&key) {
            return None;
        }
        creations.insert(
            key,
            Creation {
                started: now,
                in_progress: true,
            },
        );
        Some(Self(key))
    }
}

impl Drop for CreationGuard {
    fn drop(&mut self) {
        if let Some(creation) = CREATIONS.lock().get_mut(&self.0) {
            creation.in_progress = false;
        }
    }
}

async fn join_hub(ctx: &Context, guild: GuildId, hub: ChannelId, user: UserId) -> Result<()> {
    if !enabled() || voice(ctx, guild, user) != Some(hub) {
        return Ok(());
    }
    let member = ctx
        .cache
        .guild(guild)
        .and_then(|guild| guild.members.get(&user).cloned());
    let Some(member) = member.filter(|member| !member.user.bot) else {
        return Ok(());
    };
    let limit = owned_room_limit(actions::actor(ctx, guild, &member)?.staff);
    loop {
        let ownership = owner_lock(guild, user).await;
        if !enabled() || voice(ctx, guild, user) != Some(hub) {
            return Ok(());
        }
        if let Some(limit) = limit {
            let rooms = live_owned_rooms(ctx, guild, user).await?;
            if let Some(room) = hub_fallback(limit, &rooms) {
                // The room may be deleted or transferred while we wait for its lock.
                drop(ownership);
                if return_to_room(ctx, guild, hub, user, room).await? {
                    return Ok(());
                }
                continue;
            }
        }
        if voice(ctx, guild, user) != Some(hub) {
            return Ok(());
        }
        let Some(_creation) = CreationGuard::start(guild, user) else {
            return Ok(());
        };
        let options = RoomOptions {
            hub: Some(hub),
            name: None,
            limit: None,
            locked: None,
            schedule_id: None,
            invite_role: None,
        };
        create_room(ctx, guild, &member, Some(hub), options).await?;
        return Ok(());
    }
}

fn owned_room_limit(is_staff: bool) -> Option<usize> {
    (!is_staff).then(|| control::number("VIZIER_TEMPVC_MAX_ROOMS", 3).clamp(1, 10) as usize)
}

fn room_limit_reached(limit: Option<usize>, rooms: &[Room]) -> bool {
    limit.is_some_and(|limit| rooms.len() >= limit)
}

fn hub_fallback(limit: usize, rooms: &[Room]) -> Option<Room> {
    rooms
        .first()
        .copied()
        .filter(|_| room_limit_reached(Some(limit), rooms))
}

async fn return_to_room(
    ctx: &Context,
    guild: GuildId,
    hub: ChannelId,
    user: UserId,
    room: Room,
) -> Result<bool> {
    let channel = ChannelId::new(room.channel_id);
    let room_lock = room_state(channel);
    let mut state = room_lock.lock().await;
    if !enabled() || voice(ctx, guild, user) != Some(hub) {
        return Ok(true);
    }
    if lookup(guild, channel)? != Some(room) {
        return Ok(false);
    }
    match guild.move_member(&ctx.http, user, channel).await {
        Ok(_) => (),
        Err(err) if unknown_channel(&err) => {
            forget_room(ctx, room, &mut state)?;
            return Ok(false);
        }
        Err(err) => return Err(err.into()),
    }
    if state
        .lifecycle
        .as_ref()
        .is_some_and(|(job, _)| *job == Job::Delete)
    {
        state.cancel_lifecycle();
    }
    let grace = Job::Delete.delay().max(Duration::from_secs(1));
    reconcile(ctx, room, &mut state, &room_lock, None, grace);
    Ok(true)
}

async fn live_owned_rooms(ctx: &Context, guild: GuildId, owner: UserId) -> Result<Vec<Room>> {
    let rooms = store::owned_rooms(&*db()?, guild.get(), owner.get())?;
    retain_live_rooms(rooms, |room| async move {
        let channel = ChannelId::new(room.channel_id);
        if fetch_voice(ctx, guild, channel).await?.is_some() {
            return Ok(lookup(guild, channel)? == Some(room));
        }
        let room_lock = room_state(channel);
        if let Ok(mut state) = room_lock.try_lock() {
            forget_channel(ctx, guild, channel, &mut state)?;
        } else {
            queue_forget_channel(ctx, guild, channel);
        }
        Ok(false)
    })
    .await
}

async fn retain_live_rooms<F>(
    rooms: Vec<Room>,
    mut is_live: impl FnMut(Room) -> F,
) -> Result<Vec<Room>>
where
    F: Future<Output = Result<bool>> + Send,
{
    let mut live = Vec::with_capacity(rooms.len());
    for room in rooms {
        if is_live(room).await? {
            live.push(room);
        }
    }
    Ok(live)
}

struct RoomOptions<'a> {
    hub: Option<ChannelId>,
    name: Option<&'a str>,
    limit: Option<u32>,
    locked: Option<bool>,
    schedule_id: Option<i64>,
    invite_role: Option<RoleId>,
}

#[derive(Debug, PartialEq, Eq)]
struct RoomDefaults<'a> {
    name: &'a str,
    limit: u32,
    locked: Option<bool>,
    hidden: Option<bool>,
    bitrate: Option<u32>,
}

fn room_defaults<'a>(
    options: &RoomOptions<'a>,
    profile: &'a Profile,
    panel_name: &'a str,
    panel_limit: u32,
    panel_bitrate: Option<u32>,
) -> RoomDefaults<'a> {
    RoomDefaults {
        name: options
            .name
            .or(profile.name.as_deref())
            .unwrap_or(panel_name),
        limit: options.limit.or(profile.limit).unwrap_or(panel_limit),
        locked: options.locked.or(profile.locked),
        hidden: profile.hidden,
        bitrate: profile.bitrate.or(panel_bitrate),
    }
}

fn creation_restrictions(
    overwrites: &mut Vec<PermissionOverwrite>,
    defaults: &RoomDefaults<'_>,
    guild: GuildId,
    staff: &[u64],
) -> Vec<(Permissions, Vec<u64>)> {
    let mut suppressed = Vec::new();
    for (state, bits) in [
        (defaults.locked, Permissions::CONNECT),
        (defaults.hidden, Permissions::VIEW_CHANNEL),
    ] {
        match state {
            Some(true) => suppressed.push((
                bits,
                actions::restrict_roles(overwrites, guild, bits, staff),
            )),
            Some(false) => actions::remove_restriction(overwrites, guild, bits),
            None => {}
        }
    }
    suppressed
}

async fn create_room(
    ctx: &Context,
    guild: GuildId,
    owner: &Member,
    source: Option<ChannelId>,
    options: RoomOptions<'_>,
) -> Result<ChannelId> {
    let hub_parent = if let Some(hub) = options.hub {
        fetch_voice(ctx, guild, hub)
            .await?
            .ok_or_else(|| user_error("I couldn't find that voice hub."))?
            .parent_id
    } else {
        None
    };
    let template = if let Some(hub) = options.hub {
        let conn = db()?;
        if !control::ids("VIZIER_TEMPVC_HUBS").contains(&hub.get()) {
            return Err(user_error("This temporary voice hub is no longer active."));
        }
        store::hub_template(&conn, guild.get(), hub.get())?
    } else {
        None
    };
    if !caps::has_required_role(
        owner,
        template.as_ref().and_then(|hub| hub.required_role_id),
    ) {
        audit::post(
            ctx,
            guild,
            audit::Action::HubDenied,
            Some(owner.user.id),
            options.hub,
            None,
        );
        return Err(user_error(
            "You need the required role to create a room in this hub.",
        ));
    }
    let (category, mut overwrites, staff_roles) = {
        let cached = ctx
            .cache
            .guild(guild)
            .context("guild channels unavailable")?;
        let category = [
            template.as_ref().and_then(|hub| hub.category_id),
            control::id("VIZIER_TEMPVC_CATEGORY"),
            hub_parent.map(ChannelId::get),
        ]
        .into_iter()
        .flatten()
        .filter(|id| *id != 0)
        .filter_map(|id| cached.channels.get(&ChannelId::new(id)))
        .find(|channel| channel.kind == ChannelType::Category);
        let staff_roles = control::ids("VIZIER_TEMPVC_STAFF_ROLES")
            .into_iter()
            .filter(|id| *id != 0 && *id != guild.get())
            .map(RoleId::new)
            .filter(|id| cached.roles.contains_key(id))
            .collect::<Vec<_>>();
        (
            category.map(|ch| ch.id),
            category.map_or_else(Vec::new, |ch| ch.permission_overwrites.clone()),
            staff_roles,
        )
    };
    if options.hub.is_none() && category.is_none() {
        return Err(user_error(
            "Set a voice category before creating a scheduled room.",
        ));
    }
    let profile = store::profile(&*db()?, guild.get(), owner.user.id.get())?.unwrap_or_default();
    let panel_name = control::var("VIZIER_TEMPVC_NAME");
    let defaults = room_defaults(
        &options,
        &profile,
        template
            .as_ref()
            .and_then(|hub| hub.name_pattern.as_deref())
            .or(panel_name.as_deref())
            .unwrap_or(DEFAULT_NAME),
        template
            .as_ref()
            .and_then(|hub| hub.limit)
            .unwrap_or_else(|| control::number("VIZIER_TEMPVC_LIMIT", 0).min(99) as u32),
        template.as_ref().and_then(|hub| hub.bitrate),
    );
    let actor = actions::actor(ctx, guild, owner)?;
    let caps = caps::for_member(ctx, guild, owner, actor.staff)?;
    // A guild identifier distinguishes a category-only scheduled room from a real hub.
    let hub_id = options.hub.map_or_else(|| guild.get(), ChannelId::get);
    let name = expand_name(
        defaults.name,
        owner.display_name(),
        store::count_for_hub(&*db()?, hub_id)? + 1,
    );
    for role in &staff_roles {
        actions::grant(
            &mut overwrites,
            PermissionOverwriteType::Role(*role),
            actions::STAFF_ACCESS,
        );
    }
    actions::grant(
        &mut overwrites,
        PermissionOverwriteType::Member(owner.user.id),
        actions::OWNER_ACCESS,
    );
    actions::grant(
        &mut overwrites,
        PermissionOverwriteType::Member(ctx.cache.current_user().id),
        actions::BOT_ACCESS,
    );
    let staff_roles = staff_roles
        .iter()
        .map(|role| role.get())
        .collect::<Vec<_>>();
    let suppressed = creation_restrictions(&mut overwrites, &defaults, guild, &staff_roles);
    if let Some(role) = options.invite_role {
        actions::grant(
            &mut overwrites,
            PermissionOverwriteType::Role(role),
            Permissions::VIEW_CHANNEL | Permissions::CONNECT,
        );
    }
    let edits = access::plan(ctx, guild, owner.user.id, None, &mut overwrites).await?;
    let mut builder = CreateChannel::new(name)
        .kind(ChannelType::Voice)
        .user_limit(caps.limit(defaults.limit))
        .permissions(overwrites)
        .audit_log_reason("Temp voice room");
    if let Some(bitrate) = defaults
        .bitrate
        .or_else(|| (caps.bitrate_kbps < 64).then_some(64_000))
    {
        builder = builder.bitrate(caps.bitrate(bitrate));
    }
    if let Some(category) = category {
        builder = builder.category(category);
    }
    let created = guild.create_channel(&ctx.http, builder).await?;
    let channel = created.id;
    let room = Room {
        channel_id: channel.get(),
        guild_id: guild.get(),
        hub_id,
        owner_id: owner.user.id.get(),
    };
    let room_lock = room_state(channel);
    let mut state = room_lock.lock().await;
    let inserted = db().and_then(|mut conn| {
        let transaction = conn.transaction()?;
        store::insert(&transaction, &room)?;
        for (bits, roles) in &suppressed {
            store::save_suppressed_roles(&transaction, channel.get(), bits.bits(), roles)?;
        }
        edits.save_added(&transaction, channel)?;
        let now = chrono::Utc::now();
        store::record_room_created(
            &transaction,
            room.guild_id,
            room.owner_id,
            now.timestamp_millis(),
        )?;
        if let Some(id) = options.schedule_id
            && !store::mark_schedule_open(
                &transaction,
                room.guild_id,
                id,
                room.channel_id,
                now.timestamp(),
            )?
        {
            return Err(anyhow!("schedule is no longer pending"));
        }
        transaction.commit()?;
        Ok(())
    });
    if let Err(err) = inserted {
        // This ID came from the create request above. Compensate for a failed insert.
        if let Err(cleanup) = ctx
            .http
            .delete_channel(channel, Some("Could not save temp voice room"))
            .await
        {
            tracing::error!(%channel, %cleanup, "tempvc: could not roll back room creation");
        }
        ROOMS.lock().remove(&channel);
        return Err(err);
    }
    audit::post(
        ctx,
        guild,
        audit::Action::Created,
        Some(owner.user.id),
        Some(channel),
        None,
    );
    // List edits made while Discord created the channel could not yet see it.
    if let Err(err) = access::sync(
        ctx,
        &created,
        owner.user.id,
        created.permission_overwrites.clone(),
    )
    .await
    {
        if options.schedule_id.is_some() {
            tracing::warn!(%channel, %err, "tempvc: could not refresh scheduled room access");
        } else {
            if let Err(cleanup) = delete_room(
                ctx,
                room,
                &mut state,
                "Temp voice access setup failed",
                None,
                false,
            )
            .await
            {
                tracing::warn!(%channel, %cleanup, "tempvc: retrying cleanup after access setup failed");
                reconcile(ctx, room, &mut state, &room_lock, None, RETRY);
            }
            return Err(err);
        }
    }
    if let Some(source) = source {
        let moved = if voice(ctx, guild, owner.user.id) != Some(source) {
            Err(user_error("You left voice. Join again and try again."))
        } else {
            guild
                .move_member(&ctx.http, owner.user.id, channel)
                .await
                .map(|_| ())
                .map_err(Into::into)
        };
        if let Err(err) = moved {
            if let Err(cleanup) =
                delete_room(ctx, room, &mut state, "Temp voice move failed", None, false).await
            {
                tracing::warn!(%channel, %cleanup, "tempvc: retrying cleanup after move failed");
                reconcile(ctx, room, &mut state, &room_lock, None, RETRY);
            }
            return Err(err);
        }
    }
    // Allow the gateway move event to reach the cache even with zero grace.
    let delay = if source.is_some() {
        Job::Delete.delay().max(Duration::from_secs(1))
    } else {
        Job::Delete.delay()
    };
    reconcile(ctx, room, &mut state, &room_lock, None, delay);
    if let Err(err) = tempvc_panel::post_panel(ctx, channel).await {
        tracing::warn!(%channel, %err, "tempvc: could not post room controls");
    }
    Ok(channel)
}

async fn create_scheduled_room(
    ctx: &Context,
    guild: GuildId,
    owner: &Member,
    event: &store::Schedule,
    role: Option<RoleId>,
) -> Result<Option<ChannelId>> {
    actions::check_enabled()?;
    let actor = actions::actor(ctx, guild, owner)?;
    let _ownership = owner_lock(guild, actor.id).await;
    if let Some(limit) = owned_room_limit(actor.staff)
        && room_limit_reached(Some(limit), &live_owned_rooms(ctx, guild, actor.id).await?)
    {
        return Ok(None);
    }
    create_room(
        ctx,
        guild,
        owner,
        None,
        RoomOptions {
            hub: event.hub_id.map(ChannelId::new),
            name: Some(&event.name),
            limit: event.limit,
            locked: None,
            schedule_id: Some(event.id),
            invite_role: role,
        },
    )
    .await
    .map(Some)
}

fn expand_name(pattern: &str, user: &str, count: usize) -> String {
    let count = count.to_string();
    let mut expanded = String::new();
    let mut rest = pattern;
    while let Some((before, token)) = rest.split_once('{') {
        expanded.push_str(before);
        if let Some(tail) = token.strip_prefix("user}") {
            expanded.push_str(user);
            rest = tail;
        } else if let Some(tail) = token.strip_prefix("count}") {
            expanded.push_str(&count);
            rest = tail;
        } else {
            expanded.push('{');
            rest = token;
        }
    }
    expanded.push_str(rest);
    let fallback = format!("{user}'s room");
    let name = expanded.trim();
    let name = if name.is_empty() {
        fallback.trim()
    } else {
        name
    };
    name.chars()
        .take(100)
        .collect::<String>()
        .trim_end()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room(channel_id: u64) -> Room {
        Room {
            channel_id,
            guild_id: 1,
            hub_id: 2,
            owner_id: 3,
        }
    }

    #[tokio::test]
    async fn gone_rooms_do_not_count_toward_the_member_limit() {
        let rooms = retain_live_rooms(vec![room(30), room(20), room(10)], |room| async move {
            Ok(room.channel_id != 30)
        })
        .await
        .unwrap();

        assert_eq!(rooms, [room(20), room(10)]);
        assert!(!room_limit_reached(Some(3), &rooms));
        assert!(room_limit_reached(Some(2), &rooms));
    }

    #[test]
    fn staff_are_exempt_from_the_member_limit() {
        let rooms = (10..=20).map(room).collect::<Vec<_>>();
        assert!(room_limit_reached(Some(10), &rooms));
        assert!(!room_limit_reached(owned_room_limit(true), &rooms));
    }

    #[tokio::test]
    async fn at_limit_hub_join_uses_the_newest_live_room_across_hubs() {
        let conn = store::connect(std::path::Path::new(":memory:")).unwrap();
        let newest = Room {
            hub_id: 4,
            ..room(20)
        };
        for row in [newest, room(30), room(10)] {
            store::insert(&conn, &row).unwrap();
        }
        let owned = store::owned_rooms(&conn, newest.guild_id, newest.owner_id).unwrap();
        let rooms = retain_live_rooms(owned, |room| async move { Ok(room.channel_id != 30) })
            .await
            .unwrap();

        assert_eq!(hub_fallback(2, &rooms), Some(newest));
        assert_eq!(hub_fallback(3, &rooms), None);
    }

    #[test]
    fn creation_applies_profile_fields_and_falls_back_individually() {
        let options = RoomOptions {
            hub: Some(ChannelId::new(1)),
            name: None,
            limit: None,
            locked: None,
            schedule_id: None,
            invite_role: None,
        };
        assert_eq!(
            room_defaults(
                &options,
                &Profile::default(),
                DEFAULT_NAME,
                12,
                Some(32_000)
            ),
            RoomDefaults {
                name: DEFAULT_NAME,
                limit: 12,
                locked: None,
                hidden: None,
                bitrate: Some(32_000),
            }
        );
        let profile = Profile {
            name: Some("{user} #{count}".into()),
            limit: Some(0),
            locked: Some(true),
            hidden: Some(true),
            bitrate: Some(64_000),
        };
        let resolved = room_defaults(&options, &profile, DEFAULT_NAME, 12, None);
        assert_eq!(
            resolved,
            RoomDefaults {
                name: "{user} #{count}",
                limit: 0,
                locked: Some(true),
                hidden: Some(true),
                bitrate: Some(64_000),
            }
        );
        assert_eq!(expand_name(resolved.name, "Asha", 3), "Asha #3");
        assert_eq!(
            room_defaults(
                &options,
                &Profile {
                    limit: Some(2),
                    ..Profile::default()
                },
                DEFAULT_NAME,
                12,
                None,
            ),
            RoomDefaults {
                name: DEFAULT_NAME,
                limit: 2,
                locked: None,
                hidden: None,
                bitrate: None,
            }
        );
        let explicit = RoomOptions {
            name: Some("Slash room"),
            limit: Some(7),
            locked: Some(false),
            ..options
        };
        assert_eq!(
            room_defaults(&explicit, &profile, DEFAULT_NAME, 12, None),
            RoomDefaults {
                name: "Slash room",
                limit: 7,
                locked: Some(false),
                hidden: Some(true),
                bitrate: Some(64_000),
            }
        );
    }

    #[test]
    fn explicit_off_overrides_inherited_deny_and_missing_flags_keep_it() {
        let guild = GuildId::new(1);
        let everyone = PermissionOverwriteType::Role(guild.everyone_role());
        let mut overwrites = vec![PermissionOverwrite {
            kind: everyone,
            allow: Permissions::empty(),
            deny: Permissions::CONNECT | Permissions::VIEW_CHANNEL,
        }];
        let defaults = RoomDefaults {
            name: DEFAULT_NAME,
            limit: 0,
            locked: Some(false),
            hidden: None,
            bitrate: None,
        };
        assert!(creation_restrictions(&mut overwrites, &defaults, guild, &[]).is_empty());
        assert_eq!(
            actions::overwrite(&overwrites, everyone).deny,
            Permissions::VIEW_CHANNEL
        );
    }

    #[test]
    fn expands_names_once_and_bounds_unicode() {
        assert_eq!(expand_name(DEFAULT_NAME, "Asha", 1), "Asha's room");
        assert_eq!(
            expand_name(" {count} {user} {other} ", "{count}", 12),
            "12 {count} {other}"
        );
        assert_eq!(expand_name(" ", "Asha", 1), "Asha's room");
        assert_eq!(
            expand_name("{user}", &"🎧".repeat(120), 1).chars().count(),
            100
        );
    }

    #[test]
    fn handoff_orders_eligible_humans_by_time_present_with_stable_ties() {
        let a = UserId::new(1);
        let b = UserId::new(2);
        let c = UserId::new(3);
        let now = Instant::now();
        let mut joined = HashMap::from([(a, now), (b, now - Duration::from_secs(10))]);
        assert_eq!(handoff_candidates(&[c, a, b], &joined, &[]), [b, a, c]);
        assert_eq!(handoff_candidates(&[c, a, b], &joined, &[b.get()]), [a, c]);
        assert_eq!(handoff_candidates(&[c, a], &joined, &[]), [a, c]);
        joined.insert(b, now);
        assert_eq!(handoff_candidates(&[b, a], &joined, &[]), [a, b]);
        assert!(handoff_candidates(&[b, a], &joined, &[a.get(), b.get()]).is_empty());
        assert!(handoff_candidates(&[], &joined, &[]).is_empty());
    }

    #[test]
    fn grace_is_started_once_and_cancelled_when_anyone_rejoins() {
        let owner = UserId::new(1);
        let member = UserId::new(2);
        let empty = Occupants {
            all: vec![],
            humans: vec![],
        };
        let bots_only = Occupants {
            all: vec![member],
            humans: vec![],
        };
        let owner_away = Occupants {
            all: vec![member],
            humans: vec![member],
        };
        let owner_here = Occupants {
            all: vec![owner, member],
            humans: vec![owner, member],
        };
        assert_eq!(job_changed(None, &empty, owner), Some(Some(Job::Delete)));
        assert_eq!(job_changed(Some(Job::Delete), &empty, owner), None);
        assert_eq!(
            job_changed(Some(Job::Delete), &bots_only, owner),
            Some(None)
        );
        assert_eq!(
            job_changed(Some(Job::Delete), &owner_away, owner),
            Some(Some(Job::Handoff))
        );
        assert_eq!(
            job_changed(Some(Job::Handoff), &owner_here, owner),
            Some(None)
        );
    }
}
