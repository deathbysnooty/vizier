//! Shared policy and room edits for slash commands, panels and automatic handoff.

use serenity::all::{CreateAllowedMentions, CreateMessage, EditChannel, PremiumTier};

use super::*;
use crate::channels::discord::tempvc_panel::{Action, Reply, RoomTarget};
use crate::channels::discord::tempvc_store::Rename;
use serenity::all::{Guild, PermissionOverwrite};

pub(super) const OWNER_ACCESS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::CONNECT)
    .union(Permissions::SPEAK)
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::READ_MESSAGE_HISTORY);
pub(super) const STAFF_ACCESS: Permissions = OWNER_ACCESS.union(Permissions::USE_SOUNDBOARD);
pub(super) const BOT_ACCESS: Permissions = OWNER_ACCESS.union(Permissions::EMBED_LINKS);

fn overwrite(
    overwrites: &[PermissionOverwrite],
    kind: PermissionOverwriteType,
) -> PermissionOverwrite {
    overwrites
        .iter()
        .find(|ow| ow.kind == kind)
        .cloned()
        .unwrap_or(PermissionOverwrite {
            kind,
            allow: Permissions::empty(),
            deny: Permissions::empty(),
        })
}

fn set_overwrite(overwrites: &mut Vec<PermissionOverwrite>, replacement: PermissionOverwrite) {
    overwrites.retain(|ow| ow.kind != replacement.kind);
    if !(replacement.allow | replacement.deny).is_empty() {
        overwrites.push(replacement);
    }
}

fn allow_bits(ow: &mut PermissionOverwrite, permissions: Permissions) {
    ow.allow.insert(permissions);
    ow.deny.remove(permissions);
}

fn deny_bits(ow: &mut PermissionOverwrite, permissions: Permissions) {
    ow.allow.remove(permissions);
    ow.deny.insert(permissions);
}

pub(super) fn grant(
    overwrites: &mut Vec<PermissionOverwrite>,
    kind: PermissionOverwriteType,
    bits: Permissions,
) {
    let mut ow = overwrite(overwrites, kind);
    allow_bits(&mut ow, bits);
    set_overwrite(overwrites, ow);
}

pub(super) fn restrict_roles(
    overwrites: &mut Vec<PermissionOverwrite>,
    guild: GuildId,
    bits: Permissions,
    staff: &[u64],
) -> Vec<u64> {
    // Discord applies role allows after @everyone denies. Preserve explicit
    // member access and staff, and remember ordinary role allows for reopening.
    let mut suppressed = Vec::new();
    for ow in overwrites.iter_mut() {
        if let PermissionOverwriteType::Role(role) = ow.kind
            && (role.get() == guild.get() || !staff.contains(&role.get()))
            && ow.allow.contains(bits)
        {
            ow.allow.remove(bits);
            suppressed.push(role.get());
        }
    }
    let mut everyone = overwrite(
        overwrites,
        PermissionOverwriteType::Role(RoleId::new(guild.get())),
    );
    deny_bits(&mut everyone, bits);
    set_overwrite(overwrites, everyone);
    suppressed
}

async fn write_overwrite(
    ctx: &Context,
    channel: ChannelId,
    ow: &PermissionOverwrite,
) -> Result<()> {
    if (ow.allow | ow.deny).is_empty() {
        channel.delete_permission(&ctx.http, ow.kind).await?;
    } else {
        channel.create_permission(&ctx.http, ow.clone()).await?;
    }
    Ok(())
}

pub(super) async fn give_ownership(
    ctx: &Context,
    channel: &GuildChannel,
    room: Room,
    new_owner: UserId,
) -> Result<()> {
    let new_before = overwrite(
        &channel.permission_overwrites,
        PermissionOverwriteType::Member(new_owner),
    );
    let old_before = overwrite(
        &channel.permission_overwrites,
        PermissionOverwriteType::Member(UserId::new(room.owner_id)),
    );
    let mut new_grant = new_before.clone();
    allow_bits(&mut new_grant, OWNER_ACCESS);
    let mut old_grant = old_before.clone();
    old_grant.allow.remove(OWNER_ACCESS);
    if let Some(category) = channel.parent_id
        && let Some(guild) = ctx.cache.guild(channel.guild_id)
        && let Some(category) = guild.channels.get(&category)
    {
        let inherited = overwrite(&category.permission_overwrites, old_before.kind);
        old_grant.allow.insert(inherited.allow & OWNER_ACCESS);
        old_grant.deny.insert(inherited.deny & OWNER_ACCESS);
    }

    if new_before != new_grant {
        write_overwrite(ctx, channel.id, &new_grant).await?;
    }
    if old_before != old_grant
        && let Err(err) = write_overwrite(ctx, channel.id, &old_grant).await
    {
        restore_overwrite(ctx, channel.id, &new_before, &new_grant).await;
        return Err(err);
    }
    let updated = db().and_then(|conn| {
        Ok(store::set_owner(
            &conn,
            room.channel_id,
            room.owner_id,
            new_owner.get(),
        )?)
    });
    match updated {
        Ok(true) => Ok(()),
        result => {
            restore_overwrite(ctx, channel.id, &old_before, &old_grant).await;
            restore_overwrite(ctx, channel.id, &new_before, &new_grant).await;
            result?;
            Err(anyhow!("room owner changed while transferring"))
        }
    }
}

async fn restore_overwrite(
    ctx: &Context,
    channel: ChannelId,
    before: &PermissionOverwrite,
    after: &PermissionOverwrite,
) {
    if before != after
        && let Err(err) = write_overwrite(ctx, channel, before).await
    {
        tracing::warn!(%channel, %err, "tempvc: could not restore permissions after a failed edit");
    }
}

pub(super) async fn announce_owner(ctx: &Context, channel: ChannelId, user: UserId) {
    let message = CreateMessage::new()
        .content(format!("👑 <@{user}> ab room ka owner hai."))
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = channel.send_message(&ctx.http, message).await {
        tracing::warn!(%channel, %err, "tempvc: could not announce ownership");
    }
}

fn staff(guild: &Guild, member: &Member, roles: &[u64]) -> bool {
    guild.member_permissions(member).administrator()
        || member.roles.iter().any(|role| roles.contains(&role.get()))
}

fn staff_roles() -> Vec<u64> {
    control::ids("VIZIER_TEMPVC_STAFF_ROLES")
        .into_iter()
        .filter(|id| *id != 0)
        .collect()
}

#[derive(Clone, Copy)]
struct Actor {
    id: UserId,
    staff: bool,
    voice: Option<ChannelId>,
}

fn actor(ctx: &Context, guild: GuildId, member: &Member) -> Result<Actor> {
    let guild = ctx
        .cache
        .guild(guild)
        .ok_or_else(|| user_error("Server load ho raha hai. Thodi der mein try kar."))?;
    let id = member.user.id;
    Ok(Actor {
        id,
        staff: staff(&guild, member, &staff_roles()),
        voice: guild
            .voice_states
            .get(&id)
            .and_then(|voice| voice.channel_id),
    })
}

fn resolve(actor: Actor, target: RoomTarget) -> Result<ChannelId> {
    match target {
        RoomTarget::Panel(channel) => Ok(channel),
        RoomTarget::Voice(Some(channel)) if actor.staff => Ok(channel),
        RoomTarget::Voice(Some(_)) => Err(user_error("Channel option sirf staff ke liye hai.")),
        RoomTarget::Voice(None) => actor
            .voice
            .ok_or_else(|| user_error("Pehle apne temp VC mein aa ja.")),
    }
}

fn controls(actor: Actor, owner: UserId) -> bool {
    actor.staff || actor.id == owner
}

fn can_claim(actor: Actor, room: ChannelId, owner: UserId, owner_inside: bool) -> bool {
    actor.id != owner && (actor.staff || (actor.voice == Some(room) && !owner_inside))
}

fn check_enabled() -> Result<()> {
    if enabled() {
        Ok(())
    } else {
        Err(user_error("Temp voice abhi off hai."))
    }
}

pub(in super::super) fn can_control(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    target: RoomTarget,
) -> Result<()> {
    check_enabled()?;
    let actor = actor(ctx, guild, member)?;
    let channel = resolve(actor, target)?;
    let room =
        lookup(guild, channel)?.ok_or_else(|| user_error("Ye Loduchand ka temp VC nahi hai."))?;
    if controls(actor, UserId::new(room.owner_id)) {
        Ok(())
    } else {
        Err(user_error("Sirf room owner ya staff ye kar sakte hain."))
    }
}

pub(in super::super) async fn execute(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    target: RoomTarget,
    action: Action,
) -> Result<Reply> {
    check_enabled()?;
    let actor = actor(ctx, guild, member)?;
    if let Action::Create {
        hub,
        name,
        limit,
        locked,
    } = action
    {
        let hub = select_hub(ctx, guild, hub, actor.voice)?;
        let _creation = CreationGuard::start(guild, actor.id)
            .ok_or_else(|| user_error("Room banane se pehle 10 second ruk ja."))?;
        if let Some(existing) = find_owned(ctx, guild, actor.id, None).await? {
            return Err(user_error(format!("Tera room pehle se hai <#{existing}>.")));
        }
        let options = RoomOptions {
            hub,
            name: name.as_deref(),
            limit,
            locked,
        };
        let channel = create_room(ctx, guild, member, actor.voice, options).await?;
        return Ok(Reply::Text(format!("🎙️ Tera room ready hai <#{channel}>.")));
    }

    let channel = resolve(actor, target)?;
    lookup(guild, channel)?.ok_or_else(|| user_error("Ye Loduchand ka temp VC nahi hai."))?;
    let room_lock = room_state(channel);
    let mut state = room_lock.lock().await;
    check_enabled()?;
    let actor = self::actor(ctx, guild, member)?;
    if resolve(actor, target)? != channel {
        return Err(user_error("Tu room badal chuka hai. Phir se try kar."));
    }
    let room = lookup(guild, channel)?.ok_or_else(|| user_error("Ye temp VC ab nahi hai."))?;
    let owner = UserId::new(room.owner_id);
    match action {
        Action::Info | Action::Claim => {}
        Action::Drag(_) => {
            if !actor.staff {
                return Err(user_error("Drag sirf staff kar sakte hain."));
            }
        }
        _ if !controls(actor, owner) => {
            return Err(user_error("Sirf room owner ya staff ye kar sakte hain."));
        }
        _ => {}
    }
    let Some(ch) = fetch_voice(ctx, guild, channel).await? else {
        forget_room(channel, &mut state)?;
        return Err(user_error("Ye temp VC ab nahi hai."));
    };

    let result = match action {
        Action::Create { .. } => unreachable!("create is handled before selecting a room"),
        Action::Lock(value) => toggle(ctx, &ch, Toggle::Connect, value).await?,
        Action::Hide(value) => toggle(ctx, &ch, Toggle::View, value).await?,
        Action::Chat(value) => {
            toggle(ctx, &ch, Toggle::Chat, value.map(|enabled| !enabled)).await?
        }
        Action::Soundboard(value) => {
            toggle(ctx, &ch, Toggle::Soundboard, value.map(|enabled| !enabled)).await?
        }
        Action::Rename(pattern) => {
            let name = expand_name(&pattern, member.display_name(), 1);
            rename(ctx, &ch, &mut state, &room_lock, name).await?
        }
        Action::Limit(limit) => {
            if limit > 99 {
                return Err(user_error("Limit 0 se 99 tak rakh. 0 matlab unlimited."));
            }
            channel
                .edit(&ctx.http, EditChannel::new().user_limit(limit))
                .await?;
            if limit == 0 {
                "Room limit hata di.".into()
            } else {
                format!("Room limit {limit} kar di.")
            }
        }
        Action::Bitrate(kbps) => {
            let cap = bitrate_cap(ctx, guild)?;
            let applied = kbps.clamp(8, cap);
            channel
                .edit(&ctx.http, EditChannel::new().bitrate(applied * 1_000))
                .await?;
            if kbps > cap {
                format!("Bitrate {applied} kbps kar di. Ye server ka max hai.")
            } else {
                format!("Bitrate {applied} kbps kar di.")
            }
        }
        Action::Region(region) => {
            let label = region.clone().unwrap_or_else(|| "auto".into());
            channel
                .edit(&ctx.http, EditChannel::new().voice_region(region))
                .await?;
            format!("Voice region {label} kar diya.")
        }
        Action::Permit(target)
        | Action::Deny(target)
        | Action::Kick(target)
        | Action::Disconnect(target)
        | Action::Reset(target)
        | Action::Drag(target) => {
            if matches!(action, Action::Reset(_)) && target == owner {
                return Err(user_error(
                    "Owner ki permissions reset nahi kar sakte. Pehle room transfer kar.",
                ));
            }
            checked_member(ctx, guild, member, actor.staff, owner, target).await?;
            member_action(ctx, &ch, target, &action).await?
        }
        Action::Claim => {
            let owner_inside = voice(ctx, guild, owner) == Some(channel);
            let actor = self::actor(ctx, guild, member)?;
            if !can_claim(actor, channel, owner, owner_inside) {
                return Err(user_error(if actor.id == owner {
                    "Tu already room owner hai."
                } else if owner_inside {
                    "Owner abhi room mein hai."
                } else {
                    "Claim karne ke liye pehle room join kar."
                }));
            }
            if owner_inside {
                checked_member(ctx, guild, member, actor.staff, owner, owner).await?;
            }
            give_ownership(ctx, &ch, room, actor.id).await?;
            state.cancel_handoff();
            reconcile(
                ctx,
                Room {
                    owner_id: actor.id.get(),
                    ..room
                },
                &mut state,
                &room_lock,
                None,
                Job::Delete.delay(),
            );
            "👑 Ab ye room tera hai.".into()
        }
        Action::Transfer(target) => {
            let target_member =
                checked_member(ctx, guild, member, actor.staff, owner, target).await?;
            if target_member.user.bot || target == owner {
                return Err(user_error("Is member ko room transfer nahi kar sakte."));
            }
            if !actor.staff && voice(ctx, guild, target) != Some(channel) {
                return Err(user_error("Naya owner room mein hona chahiye."));
            }
            give_ownership(ctx, &ch, room, target).await?;
            state.cancel_handoff();
            reconcile(
                ctx,
                Room {
                    owner_id: target.get(),
                    ..room
                },
                &mut state,
                &room_lock,
                None,
                Job::Delete.delay(),
            );
            format!("👑 <@{target}> ab room ka owner hai.")
        }
        Action::Delete => {
            delete_room(
                ctx,
                room,
                &mut state,
                "Temp voice room deleted by owner or staff",
            )
            .await?;
            "Room delete kar diya.".into()
        }
        Action::Panel => {
            tempvc_panel::post_panel(ctx, channel).await?;
            format!("Panel <#{channel}> ke chat mein bhej diya.")
        }
        Action::Info => {
            let members = occupants(ctx, guild, channel)
                .map(|occupants| occupants.all)
                .unwrap_or_default();
            return Ok(Reply::Info(tempvc_panel::info_embed(&ch, owner, &members)));
        }
    };
    Ok(Reply::Text(result))
}

fn select_hub(
    ctx: &Context,
    guild: GuildId,
    requested: Option<ChannelId>,
    current: Option<ChannelId>,
) -> Result<ChannelId> {
    let configured = control::ids("VIZIER_TEMPVC_HUBS");
    let guild = ctx
        .cache
        .guild(guild)
        .context("guild channels unavailable")?;
    let mut hubs: Vec<_> = guild
        .channels
        .values()
        .filter(|ch| ch.kind == ChannelType::Voice && configured.contains(&ch.id.get()))
        .map(|ch| ch.id)
        .collect();
    if let Some(hub) = requested {
        return if hubs.contains(&hub) {
            Ok(hub)
        } else {
            Err(user_error("Ye temp voice hub nahi hai."))
        };
    }
    if let Some(hub) = current.filter(|channel| hubs.contains(channel)) {
        return Ok(hub);
    }
    match hubs.len() {
        0 => Err(user_error("Abhi koi temp voice hub set nahi hai.")),
        1 => Ok(hubs.remove(0)),
        _ => Err(user_error("Kaunsa hub? Hub option se choose kar.")),
    }
}

fn bitrate_cap(ctx: &Context, id: GuildId) -> Result<u32> {
    let guild = ctx
        .cache
        .guild(id)
        .context("guild boost tier unavailable")?;
    Ok(
        if guild.premium_tier == PremiumTier::Tier3
            || guild.features.iter().any(|f| f == "VIP_REGIONS")
        {
            384
        } else {
            match guild.premium_tier {
                PremiumTier::Tier2 => 256,
                PremiumTier::Tier1 => 128,
                _ => 96,
            }
        },
    )
}

#[derive(Clone, Copy)]
enum Toggle {
    Connect,
    View,
    Chat,
    Soundboard,
}

impl Toggle {
    fn bits(self) -> Permissions {
        match self {
            Self::Connect => Permissions::CONNECT,
            Self::View => Permissions::VIEW_CHANNEL,
            Self::Chat => Permissions::SEND_MESSAGES,
            Self::Soundboard => Permissions::USE_SOUNDBOARD,
        }
    }

    fn message(self, restricted: bool) -> &'static str {
        match (self, restricted) {
            (Self::Connect, true) => "🔒 Room lock kar diya.",
            (Self::Connect, false) => "🔓 Room unlock kar diya.",
            (Self::View, true) => "🙈 Room hide kar diya.",
            (Self::View, false) => "👀 Room ab visible hai.",
            (Self::Chat, true) => "💬 Chat off kar di.",
            (Self::Chat, false) => "💬 Chat on kar di.",
            (Self::Soundboard, true) => "🔇 Soundboard off kar diya.",
            (Self::Soundboard, false) => "🔊 Soundboard on kar diya.",
        }
    }
}

async fn toggle(
    ctx: &Context,
    channel: &GuildChannel,
    toggle: Toggle,
    restrict: Option<bool>,
) -> Result<String> {
    let everyone = PermissionOverwriteType::Role(RoleId::new(channel.guild_id.get()));
    let bits = toggle.bits();
    let mut everyone_ow = overwrite(&channel.permission_overwrites, everyone);
    let restricted = restrict.unwrap_or(!everyone_ow.deny.contains(bits));
    let mut overwrites = channel.permission_overwrites.clone();
    let mut suppressed = store::load_suppressed_roles(&*db()?, channel.id.get(), bits.bits())?;
    if restricted {
        suppressed.extend(restrict_roles(
            &mut overwrites,
            channel.guild_id,
            bits,
            &staff_roles(),
        ));
        suppressed.sort_unstable();
        suppressed.dedup();
        // Persist first so a restart still has the grants it needs to restore.
        store::save_suppressed_roles(&*db()?, channel.id.get(), bits.bits(), &suppressed)?;
    } else {
        everyone_ow.deny.remove(bits);
        set_overwrite(&mut overwrites, everyone_ow);
        let guild = ctx
            .cache
            .guild(channel.guild_id)
            .context("guild roles unavailable")?;
        for role in suppressed {
            let kind = PermissionOverwriteType::Role(RoleId::new(role));
            if guild.roles.contains_key(&RoleId::new(role))
                && !overwrite(&overwrites, kind).deny.contains(bits)
            {
                grant(&mut overwrites, kind, bits);
            }
        }
    }
    overwrites.retain(|ow| !(ow.allow | ow.deny).is_empty());
    if channel.permission_overwrites != overwrites {
        channel
            .id
            .edit(&ctx.http, EditChannel::new().permissions(overwrites))
            .await?;
    }
    if !restricted {
        store::save_suppressed_roles(&*db()?, channel.id.get(), bits.bits(), &[])?;
    }
    Ok(toggle.message(restricted).into())
}

fn protected_target(
    actor_is_staff: bool,
    target_is_staff: bool,
    target_is_owner: bool,
    actor_outranks: bool,
) -> bool {
    if actor_is_staff {
        !actor_outranks
    } else {
        target_is_staff || target_is_owner
    }
}

async fn checked_member(
    ctx: &Context,
    guild: GuildId,
    actor: &Member,
    actor_is_staff: bool,
    owner: UserId,
    target: UserId,
) -> Result<Member> {
    if target == actor.user.id || target == ctx.cache.current_user().id {
        return Err(user_error("Khud par ya Loduchand par ye nahi kar sakte."));
    }
    let target_member = match guild.member(ctx, target).await {
        Ok(member) => member,
        Err(serenity::Error::Http(HttpError::UnsuccessfulRequest(response)))
            if response.error.code == 10007 =>
        {
            return Err(user_error("Ye member server mein nahi hai."));
        }
        Err(err) => return Err(err.into()),
    };
    let guild = ctx.cache.guild(guild).context("guild roles unavailable")?;
    let settings_roles = staff_roles();
    let actor_outranks = actor.user.id == guild.owner_id
        || (target != guild.owner_id
            && guild.member_highest_role(actor) > guild.member_highest_role(&target_member));
    if protected_target(
        actor_is_staff,
        staff(&guild, &target_member, &settings_roles),
        target == owner,
        actor_outranks,
    ) {
        return Err(user_error(if actor_is_staff {
            "Ye member tere role ke barabar ya upar hai."
        } else {
            "Staff ya room owner par ye nahi kar sakte."
        }));
    }
    Ok(target_member)
}

async fn member_action(
    ctx: &Context,
    channel: &GuildChannel,
    target: UserId,
    action: &Action,
) -> Result<String> {
    let guild = channel.guild_id;
    let id = channel.id;
    let kind = PermissionOverwriteType::Member(target);
    let before = overwrite(&channel.permission_overwrites, kind);
    let mut after = before.clone();
    let result = match action {
        Action::Permit(_) => {
            allow_bits(&mut after, Permissions::VIEW_CHANNEL | Permissions::CONNECT);
            format!("<@{target}> ab join kar sakta hai.")
        }
        Action::Deny(_) => {
            deny_bits(&mut after, Permissions::CONNECT);
            format!("<@{target}> ab is room mein join nahi kar sakta.")
        }
        Action::Kick(_) => {
            deny_bits(&mut after, Permissions::CONNECT);
            if after != before {
                write_overwrite(ctx, id, &after).await?;
            }
            if voice(ctx, guild, target) == Some(id)
                && let Err(err) = guild.disconnect_member(&ctx.http, target).await
            {
                restore_overwrite(ctx, id, &before, &after).await;
                return Err(err.into());
            }
            return Ok(format!("<@{target}> ko room se block kar diya."));
        }
        Action::Reset(_) => {
            after.allow = Permissions::empty();
            after.deny = Permissions::empty();
            format!("<@{target}> ki room permissions reset kar di.")
        }
        Action::Disconnect(_) => {
            if voice(ctx, guild, target) != Some(id) {
                return Err(user_error("Ye member is room mein nahi hai."));
            }
            guild.disconnect_member(&ctx.http, target).await?;
            return Ok(format!("<@{target}> ko disconnect kar diya."));
        }
        Action::Drag(_) => {
            let source = voice(ctx, guild, target)
                .ok_or_else(|| user_error("Ye member kisi VC mein nahi hai."))?;
            if source == id {
                return Err(user_error("Ye member already is VC mein hai."));
            }
            guild.move_member(&ctx.http, target, id).await?;
            return Ok(format!("<@{target}> ko <#{id}> mein move kar diya."));
        }
        _ => unreachable!("only member actions are routed here"),
    };
    if after != before {
        write_overwrite(ctx, id, &after).await?;
    }
    Ok(result)
}

// Use the same sliding window after a restart. Leave a second of headroom at the boundary.
const RENAME_WINDOW_MS: i64 = 601_000;

fn rename_delay(record: &Rename, now_ms: i64) -> Duration {
    let remaining = record.previous_ms.map_or(0, |previous| {
        previous
            .saturating_add(RENAME_WINDOW_MS)
            .saturating_sub(now_ms)
            .clamp(0, RENAME_WINDOW_MS)
    });
    Duration::from_millis(remaining as u64)
}

fn load_rename(channel: ChannelId) -> Result<Rename> {
    Ok(store::rename(&*db()?, channel.get())?.unwrap_or_default())
}

fn save_rename(channel: ChannelId, record: &Rename) -> Result<()> {
    Ok(store::save_rename(&*db()?, channel.get(), record)?)
}

async fn rename(
    ctx: &Context,
    channel: &GuildChannel,
    state: &mut RoomState,
    lock: &RoomLock,
    name: String,
) -> Result<String> {
    let id = channel.id;
    let mut record = load_rename(id)?;
    if channel.name == name {
        if record.pending.take().is_some() {
            save_rename(id, &record)?;
        }
        return Ok("Room ka naam already yahi hai.".into());
    }
    let delay = rename_delay(&record, chrono::Utc::now().timestamp_millis());
    record.pending = Some(name);
    save_rename(id, &record)?;
    if delay.is_zero() {
        apply_rename(ctx, id, record).await?;
        Ok("Room ka naam badal diya.".into())
    } else {
        start_rename(ctx, id, state, lock);
        Ok(format!(
            "Rename queue mein hai. Lagbhag {} second mein ho jayega.",
            delay.as_millis().div_ceil(1_000)
        ))
    }
}

async fn apply_rename(ctx: &Context, channel: ChannelId, mut record: Rename) -> Result<()> {
    let Some(name) = record.pending.clone() else {
        return Ok(());
    };
    // Reserve the edit before sending it, so a crash cannot reset the window.
    let before = record.clone();
    record.previous_ms = record.latest_ms;
    record.latest_ms = Some(chrono::Utc::now().timestamp_millis());
    save_rename(channel, &record)?;
    let result = channel.edit(&ctx.http, EditChannel::new().name(name)).await;
    if result.is_ok() {
        record.latest_ms = Some(chrono::Utc::now().timestamp_millis());
    } else {
        record = before;
    }
    record.pending = None;
    save_rename(channel, &record)?;
    result?;
    Ok(())
}

fn start_rename(ctx: &Context, channel: ChannelId, state: &mut RoomState, lock: &RoomLock) {
    if state.rename.is_none() {
        state.rename = Some(tokio::spawn(run_rename(ctx.clone(), channel, lock.clone())));
    }
}

pub(super) fn restore_rename(
    ctx: &Context,
    channel: ChannelId,
    state: &mut RoomState,
    lock: &RoomLock,
) -> Result<()> {
    if load_rename(channel)?.pending.is_some() {
        start_rename(ctx, channel, state, lock);
    }
    Ok(())
}

async fn run_rename(ctx: Context, channel: ChannelId, lock: RoomLock) {
    loop {
        let mut state = lock.lock().await;
        if !enabled() {
            state.rename.take();
            return;
        }
        match queued_edit(&ctx, channel).await {
            Ok(Some(delay)) => {
                drop(state);
                sleep(delay).await;
            }
            result => {
                if let Err(err) = result {
                    tracing::warn!(%channel, %err, "tempvc: queued rename failed");
                }
                state.rename.take();
                return;
            }
        }
    }
}

async fn queued_edit(ctx: &Context, channel: ChannelId) -> Result<Option<Duration>> {
    let record = load_rename(channel)?;
    if record.pending.is_none() {
        return Ok(None);
    }
    let delay = rename_delay(&record, chrono::Utc::now().timestamp_millis());
    if !delay.is_zero() {
        return Ok(Some(delay));
    }
    apply_rename(ctx, channel, record).await?;
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_actions_require_owner_or_staff_and_claim_requires_presence() {
        let room = ChannelId::new(1);
        let owner_id = UserId::new(2);
        let visitor = Actor {
            id: UserId::new(3),
            staff: false,
            voice: Some(room),
        };
        let owner = Actor {
            id: owner_id,
            ..visitor
        };
        let staff = Actor {
            staff: true,
            voice: None,
            ..visitor
        };
        assert!(controls(owner, owner_id));
        assert!(controls(staff, owner_id));
        assert!(!controls(visitor, owner_id));
        assert!(can_claim(visitor, room, owner_id, false));
        assert!(!can_claim(visitor, room, owner_id, true));
        assert!(!can_claim(
            Actor {
                voice: None,
                ..visitor
            },
            room,
            owner_id,
            false
        ));
        assert!(!can_claim(owner, room, owner_id, false));
        assert!(can_claim(staff, room, owner_id, true));
        assert!(resolve(visitor, RoomTarget::Voice(Some(room))).is_err());
        assert_eq!(resolve(staff, RoomTarget::Voice(Some(room))).unwrap(), room);
    }

    #[test]
    fn protects_staff_owner_and_equal_or_higher_staff_targets() {
        assert!(protected_target(false, true, false, true));
        assert!(protected_target(false, false, true, true));
        assert!(!protected_target(false, false, false, false));
        assert!(protected_target(true, true, false, false));
        assert!(!protected_target(true, true, true, true));
    }

    #[test]
    fn restrictions_suspend_ordinary_role_allows_and_keep_explicit_access() {
        let guild = GuildId::new(1);
        let everyone = PermissionOverwriteType::Role(guild.everyone_role());
        let ordinary = PermissionOverwriteType::Role(RoleId::new(2));
        let staff = PermissionOverwriteType::Role(RoleId::new(3));
        let permitted = PermissionOverwriteType::Member(UserId::new(4));
        let mut overwrites = Vec::new();
        for kind in [everyone, ordinary, staff, permitted] {
            grant(
                &mut overwrites,
                kind,
                Permissions::CONNECT | Permissions::VIEW_CHANNEL,
            );
        }

        assert_eq!(
            restrict_roles(&mut overwrites, guild, Permissions::CONNECT, &[3]),
            vec![1, 2]
        );
        assert!(
            overwrite(&overwrites, everyone)
                .deny
                .contains(Permissions::CONNECT)
        );
        assert_eq!(
            overwrite(&overwrites, ordinary).allow,
            Permissions::VIEW_CHANNEL
        );
        assert!(
            overwrite(&overwrites, staff)
                .allow
                .contains(Permissions::CONNECT)
        );
        assert!(
            overwrite(&overwrites, permitted)
                .allow
                .contains(Permissions::CONNECT)
        );
    }

    #[test]
    fn rename_windows_allow_two_edits_then_wait_until_oldest_expires() {
        assert_eq!(rename_delay(&Rename::default(), 12_000), Duration::ZERO);
        let first = Rename {
            latest_ms: Some(10_000),
            ..Rename::default()
        };
        assert_eq!(rename_delay(&first, 12_000), Duration::ZERO);
        let second = Rename {
            previous_ms: first.latest_ms,
            latest_ms: Some(12_000),
            pending: Some("last".into()),
        };
        assert_eq!(rename_delay(&second, 610_000), Duration::from_secs(1));
        assert_eq!(rename_delay(&second, 611_000), Duration::ZERO);
        assert_eq!(
            rename_delay(&second, 0),
            Duration::from_millis(RENAME_WINDOW_MS as u64)
        );
    }
}
