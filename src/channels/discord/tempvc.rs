//! Join-to-create and lifecycle for the temporary rooms recorded in tempvc.db.

mod access;
mod actions;
mod knock;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow};
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, ChannelType, Context, CreateChannel, GuildChannel, GuildId, HttpError, Member,
    PermissionOverwrite, PermissionOverwriteType, Permissions, RoleId, UserId, VoiceState,
};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

use super::control;
use super::tempvc_panel;
use super::tempvc_store::{self as store, Profile, Room};

pub(super) use actions::{can_control, execute};
pub(super) use knock::answer_knock;

const DEFAULT_NAME: &str = "{user}'s room";
const COOLDOWN: Duration = Duration::from_secs(10);
const RETRY: Duration = Duration::from_secs(15);

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

fn longest_present(
    humans: &[UserId],
    joined: &HashMap<UserId, Instant>,
    banned: &[u64],
) -> Option<UserId> {
    // The gateway does not report join times for people already present on boot.
    // Break ties stably by user ID when their original order is unknown.
    humans
        .iter()
        .copied()
        .filter(|user| banned.binary_search(&user.get()).is_err())
        .min_by_key(|user| (joined.get(user).is_none(), joined.get(user), *user))
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

fn forget_room(channel: ChannelId, state: &mut RoomState) -> Result<()> {
    store::delete(&*db()?, channel.get())?;
    state.cancel_lifecycle();
    if let Some(task) = state.rename.take() {
        task.abort();
    }
    ROOMS.lock().remove(&channel);
    Ok(())
}

async fn delete_room(ctx: &Context, room: Room, state: &mut RoomState, reason: &str) -> Result<()> {
    let channel = ChannelId::new(room.channel_id);
    match ctx.http.delete_channel(channel, Some(reason)).await {
        Ok(_) => (),
        Err(err) if unknown_channel(&err) => (),
        Err(err) => return Err(err.into()),
    }
    forget_room(channel, state)
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
    let Some(occupants) = occupants(ctx, guild, channel) else {
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
            Ok(()) => return,
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
) -> Result<()> {
    if !enabled() {
        return Ok(());
    }
    let Some(room) = lookup(guild, channel)? else {
        return Ok(());
    };
    let owner = UserId::new(room.owner_id);
    let current = occupants(ctx, guild, channel).context("guild voice state unavailable")?;
    if desired_job(&current, owner) != Some(job) {
        return Ok(());
    }
    match job {
        Job::Delete => delete_room(ctx, room, state, "Temp voice room empty").await,
        Job::Handoff => {
            let Some(ch) = fetch_voice(ctx, guild, channel).await? else {
                return forget_room(channel, state);
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
                if let Some(next) = longest_present(&current.humans, &state.joined, &bans) {
                    actions::give_ownership(ctx, &ch, room, next).await?;
                    actions::announce_owner(ctx, channel, next).await;
                } else {
                    for user in current.humans {
                        access::disconnect_if_present(ctx, guild, channel, user).await?;
                    }
                }
            }
            Ok(())
        }
    }
}

pub(super) fn on_cache_ready(ctx: &Context, guilds: Vec<GuildId>) {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    let start_watch = !WATCHING.swap(true, Ordering::Relaxed);
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
    let fetched = fetch_voice(ctx, guild, channel).await;
    if matches!(fetched, Ok(None)) {
        return forget_room(channel, &mut state);
    }
    reconcile(ctx, room, &mut state, &room_lock, None, Duration::ZERO);
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
        {
            tracing::warn!(%hub, %user, %err, "tempvc: hub join failed");
        }
    });
}

pub(super) fn on_channel_delete(channel: &GuildChannel) {
    let id = channel.id;
    let guild = channel.guild_id;
    if channel.kind != ChannelType::Voice || !matches!(lookup(guild, id), Ok(Some(_))) {
        return;
    }
    tokio::spawn(async move {
        let room_lock = room_state(id);
        let mut state = room_lock.lock().await;
        if matches!(lookup(guild, id), Ok(Some(_)))
            && let Err(err) = forget_room(id, &mut state)
        {
            tracing::warn!(%id, %err, "tempvc: could not remove deleted room");
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
    let Some(_creation) = CreationGuard::start(guild, user) else {
        return Ok(());
    };
    if find_owned(ctx, guild, user, Some(hub)).await?.is_none()
        && voice(ctx, guild, user) == Some(hub)
    {
        let options = RoomOptions {
            hub,
            name: None,
            limit: None,
            locked: None,
        };
        create_room(ctx, guild, &member, Some(hub), options).await?;
    }
    Ok(())
}

async fn find_owned(
    ctx: &Context,
    guild: GuildId,
    user: UserId,
    from_hub: Option<ChannelId>,
) -> Result<Option<ChannelId>> {
    loop {
        let Some(room) = store::owned_room(&*db()?, guild.get(), user.get())? else {
            return Ok(None);
        };
        let channel = ChannelId::new(room.channel_id);
        let room_lock = room_state(channel);
        let mut state = room_lock.lock().await;
        let Some(room) = lookup(guild, channel)?.filter(|room| room.owner_id == user.get()) else {
            continue;
        };
        if fetch_voice(ctx, guild, channel).await?.is_some() {
            if let Some(hub) = from_hub
                && voice(ctx, guild, user) == Some(hub)
            {
                guild.move_member(&ctx.http, user, channel).await?;
                if state
                    .lifecycle
                    .as_ref()
                    .is_some_and(|(job, _)| *job == Job::Delete)
                {
                    state.cancel_lifecycle();
                }
                let grace = Job::Delete.delay().max(Duration::from_secs(1));
                reconcile(ctx, room, &mut state, &room_lock, None, grace);
            }
            return Ok(Some(channel));
        }
        forget_room(channel, &mut state)?;
    }
}

struct RoomOptions<'a> {
    hub: ChannelId,
    name: Option<&'a str>,
    limit: Option<u32>,
    locked: Option<bool>,
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
) -> RoomDefaults<'a> {
    RoomDefaults {
        name: options
            .name
            .or(profile.name.as_deref())
            .unwrap_or(panel_name),
        limit: options.limit.or(profile.limit).unwrap_or(panel_limit),
        locked: options.locked.or(profile.locked),
        hidden: profile.hidden,
        bitrate: profile.bitrate,
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
    let Some(hub) = fetch_voice(ctx, guild, options.hub).await? else {
        return Err(user_error("Ye hub voice channel nahi mila."));
    };
    let (category, mut overwrites, staff_roles) = {
        let cached = ctx
            .cache
            .guild(guild)
            .context("guild channels unavailable")?;
        let configured = control::id("VIZIER_TEMPVC_CATEGORY")
            .filter(|id| *id != 0)
            .map(ChannelId::new);
        let category = [configured, hub.parent_id]
            .into_iter()
            .flatten()
            .filter_map(|id| cached.channels.get(&id))
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
    let profile = store::profile(&*db()?, guild.get(), owner.user.id.get())?.unwrap_or_default();
    let panel_name = control::var("VIZIER_TEMPVC_NAME");
    let defaults = room_defaults(
        &options,
        &profile,
        panel_name.as_deref().unwrap_or(DEFAULT_NAME),
        control::number("VIZIER_TEMPVC_LIMIT", 0).min(99) as u32,
    );
    let name = expand_name(
        defaults.name,
        owner.display_name(),
        store::count_for_hub(&*db()?, options.hub.get())? + 1,
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
    let edits = access::plan(ctx, guild, owner.user.id, None, &mut overwrites).await?;
    let mut builder = CreateChannel::new(name)
        .kind(ChannelType::Voice)
        .user_limit(defaults.limit)
        .permissions(overwrites)
        .audit_log_reason("Temp voice room");
    if let Some(bitrate) = defaults.bitrate {
        builder = builder.bitrate(bitrate.clamp(8_000, actions::bitrate_cap(ctx, guild)? * 1_000));
    }
    if let Some(category) = category {
        builder = builder.category(category);
    }
    let created = guild.create_channel(&ctx.http, builder).await?;
    let channel = created.id;
    let room = Room {
        channel_id: channel.get(),
        guild_id: guild.get(),
        hub_id: options.hub.get(),
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
    // List edits made while Discord created the channel could not yet see it.
    if let Err(err) = access::sync(
        ctx,
        &created,
        owner.user.id,
        created.permission_overwrites.clone(),
    )
    .await
    {
        if let Err(cleanup) =
            delete_room(ctx, room, &mut state, "Temp voice access setup failed").await
        {
            tracing::warn!(%channel, %cleanup, "tempvc: retrying cleanup after access setup failed");
            reconcile(ctx, room, &mut state, &room_lock, None, RETRY);
        }
        return Err(err);
    }
    if let Some(source) = source {
        let moved = if voice(ctx, guild, owner.user.id) != Some(source) {
            Err(user_error("Tu voice se nikal gaya. Phir se try kar."))
        } else {
            guild
                .move_member(&ctx.http, owner.user.id, channel)
                .await
                .map(|_| ())
                .map_err(Into::into)
        };
        if let Err(err) = moved {
            if let Err(cleanup) = delete_room(ctx, room, &mut state, "Temp voice move failed").await
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

    #[test]
    fn creation_applies_profile_fields_and_falls_back_individually() {
        let options = RoomOptions {
            hub: ChannelId::new(1),
            name: None,
            limit: None,
            locked: None,
        };
        assert_eq!(
            room_defaults(&options, &Profile::default(), DEFAULT_NAME, 12),
            RoomDefaults {
                name: DEFAULT_NAME,
                limit: 12,
                locked: None,
                hidden: None,
                bitrate: None,
            }
        );
        let profile = Profile {
            name: Some("{user} #{count}".into()),
            limit: Some(0),
            locked: Some(true),
            hidden: Some(true),
            bitrate: Some(64_000),
        };
        let resolved = room_defaults(&options, &profile, DEFAULT_NAME, 12);
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
                12
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
            room_defaults(&explicit, &profile, DEFAULT_NAME, 12),
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
    fn handoff_picks_longest_present_human_with_stable_ties() {
        let a = UserId::new(1);
        let b = UserId::new(2);
        let c = UserId::new(3);
        let now = Instant::now();
        let mut joined = HashMap::from([(a, now), (b, now - Duration::from_secs(10))]);
        assert_eq!(longest_present(&[c, a, b], &joined, &[]), Some(b));
        assert_eq!(longest_present(&[c, a, b], &joined, &[b.get()]), Some(a));
        assert_eq!(longest_present(&[c, a], &joined, &[]), Some(a));
        joined.insert(b, now);
        assert_eq!(longest_present(&[b, a], &joined, &[]), Some(a));
        assert_eq!(longest_present(&[b, a], &joined, &[a.get(), b.get()]), None);
        assert_eq!(longest_present(&[], &joined, &[]), None);
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
