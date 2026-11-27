//! Shared policy and room edits for slash commands, panels and automatic handoff.

use serenity::all::{CreateAllowedMentions, CreateEmbed, CreateMessage, EditChannel, PremiumTier};

use super::*;
use crate::channels::discord::tempvc_panel::{
    Action, BulkUsers, ForceAction, MeAction, Reply, RoomTarget,
};
use crate::channels::discord::tempvc_store::{Access, Rename};
use serenity::all::{Guild, PermissionOverwrite};

pub(super) const OWNER_ACCESS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::CONNECT)
    .union(Permissions::SPEAK)
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::READ_MESSAGE_HISTORY);
pub(super) const STAFF_ACCESS: Permissions = OWNER_ACCESS.union(Permissions::USE_SOUNDBOARD);
pub(super) const BOT_ACCESS: Permissions = OWNER_ACCESS.union(Permissions::EMBED_LINKS);
pub(super) const PERMIT_ACCESS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::CONNECT)
    .union(Permissions::SEND_MESSAGES);

pub(super) fn overwrite(
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

pub(super) fn set_overwrite(
    overwrites: &mut Vec<PermissionOverwrite>,
    replacement: PermissionOverwrite,
) {
    let position = overwrites.iter().position(|ow| ow.kind == replacement.kind);
    let empty = (replacement.allow | replacement.deny).is_empty();
    match (position, empty) {
        (Some(index), true) => {
            overwrites.remove(index);
        }
        (Some(index), false) => overwrites[index] = replacement,
        (None, false) => overwrites.push(replacement),
        (None, true) => {}
    }
}

pub(super) fn allow_bits(ow: &mut PermissionOverwrite, permissions: Permissions) {
    ow.allow.insert(permissions);
    ow.deny.remove(permissions);
}

pub(super) fn deny_bits(ow: &mut PermissionOverwrite, permissions: Permissions) {
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

pub(super) fn remove_restriction(
    overwrites: &mut Vec<PermissionOverwrite>,
    guild: GuildId,
    bits: Permissions,
) {
    let mut everyone = overwrite(
        overwrites,
        PermissionOverwriteType::Role(guild.everyone_role()),
    );
    everyone.deny.remove(bits);
    set_overwrite(overwrites, everyone);
}

pub(super) async fn write_overwrite(
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

pub(super) enum OwnershipChange {
    Claim,
    Transfer { by_staff: bool },
    Handoff,
}

pub(super) async fn give_ownership(
    ctx: &Context,
    channel: &GuildChannel,
    room: Room,
    new_owner: Actor,
    change: OwnershipChange,
) -> Result<bool> {
    let Actor { id: new_owner, staff, .. } = new_owner;
    let owner_guard = owner_lock(channel.guild_id, new_owner).await;
    if let Some(limit) = owned_room_limit(staff)
        && room_limit_reached(
            Some(limit),
            &live_owned_rooms(ctx, channel.guild_id, new_owner).await?,
        )
    {
        return Ok(false);
    }

    let mut old_grant = overwrite(
        &channel.permission_overwrites,
        PermissionOverwriteType::Member(UserId::new(room.owner_id)),
    );
    old_grant.allow.remove(OWNER_ACCESS);
    if let Some(category) = channel.parent_id
        && let Some(guild) = ctx.cache.guild(channel.guild_id)
        && let Some(category) = guild.channels.get(&category)
    {
        let inherited = overwrite(&category.permission_overwrites, old_grant.kind);
        old_grant.allow.insert(inherited.allow & OWNER_ACCESS);
        old_grant.deny.insert(inherited.deny & OWNER_ACCESS);
    }

    let mut overwrites = channel.permission_overwrites.clone();
    set_overwrite(&mut overwrites, old_grant);
    let edits = access::plan(
        ctx,
        channel.guild_id,
        new_owner,
        Some(channel),
        &mut overwrites,
    )
    .await?;
    grant(
        &mut overwrites,
        PermissionOverwriteType::Member(new_owner),
        OWNER_ACCESS,
    );
    // Capacity and overwrite checks can await Discord. Confirm voice presence again.
    let receiver_inside = voice(ctx, channel.guild_id, new_owner) == Some(channel.id);
    let owner_inside =
        || voice(ctx, channel.guild_id, UserId::new(room.owner_id)) == Some(channel.id);
    match change {
        OwnershipChange::Handoff if !receiver_inside || owner_inside() => return Ok(false),
        OwnershipChange::Claim if !staff && (!receiver_inside || owner_inside()) => {
            return Err(user_error("The room's voice membership changed. Try again."));
        }
        OwnershipChange::Transfer { by_staff: false } if !receiver_inside => {
            return Err(user_error("The new owner must be in the room."));
        }
        _ => ()
    }
    let edited = if channel.permission_overwrites == overwrites {
        None
    } else {
        Some(
            channel
                .id
                .edit(&ctx.http, EditChannel::new().permissions(overwrites))
                .await?,
        )
    };
    let updated = db().and_then(|mut conn| {
        let tx = conn.transaction()?;
        if !store::set_owner(&tx, room.channel_id, room.owner_id, new_owner.get())? {
            return Err(anyhow!("room owner changed while transferring"));
        }
        edits.save_added(&tx, channel.id)?;
        edits.save_removed(&tx, channel.id)?;
        tx.commit()?;
        Ok(())
    });
    if let Err(err) = updated {
        if edited.is_some()
            && let Err(restore) = channel
                .id
                .edit(
                    &ctx.http,
                    EditChannel::new().permissions(channel.permission_overwrites.clone()),
                )
                .await
        {
            tracing::warn!(channel = %channel.id, %restore, "tempvc: could not restore permissions after a failed transfer");
        }
        return Err(err);
    }
    drop(owner_guard);
    // Re-read the new owner's lists now that list edits can find this room.
    let channel = edited.as_ref().unwrap_or(channel);
    if let Err(err) = access::sync(
        ctx,
        channel,
        new_owner,
        channel.permission_overwrites.clone(),
    )
    .await
    {
        tracing::warn!(channel = %channel.id, %err, "tempvc: could not sync access after transfer");
    }
    Ok(true)
}

pub(super) async fn announce_owner(ctx: &Context, channel: ChannelId, user: UserId) {
    let message = CreateMessage::new()
        .content(format!("👑 <@{user}> now owns this room."))
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = channel.send_message(&ctx.http, message).await {
        tracing::warn!(%channel, %err, "tempvc: could not announce ownership");
    }
}

fn staff(guild: &Guild, member: &Member, roles: &[u64]) -> bool {
    guild.member_permissions(member).administrator()
        || member.roles.iter().any(|role| roles.contains(&role.get()))
}

pub(super) fn staff_roles() -> Vec<u64> {
    control::ids("VIZIER_TEMPVC_STAFF_ROLES")
        .into_iter()
        .filter(|id| *id != 0)
        .collect()
}

#[derive(Clone, Copy)]
pub(super) struct Actor {
    pub(super) id: UserId,
    pub(super) staff: bool,
    pub(super) voice: Option<ChannelId>,
}

pub(super) fn actor(ctx: &Context, guild: GuildId, member: &Member) -> Result<Actor> {
    let guild = ctx
        .cache
        .guild(guild)
        .ok_or_else(|| user_error("The server is still loading. Try again in a moment."))?;
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
        RoomTarget::Voice(Some(_)) => Err(user_error("Only staff can use the channel option.")),
        RoomTarget::Voice(None) => actor
            .voice
            .ok_or_else(|| user_error("Join your temporary voice room first.")),
    }
}

pub(super) fn controls(actor: Actor, owner: UserId) -> bool {
    actor.staff || actor.id == owner
}

fn can_claim(actor: Actor, room: ChannelId, owner: UserId, owner_inside: bool) -> bool {
    actor.id != owner && (actor.staff || (actor.voice == Some(room) && !owner_inside))
}

pub(super) fn check_enabled() -> Result<()> {
    if enabled() {
        Ok(())
    } else {
        Err(user_error("Temporary voice rooms are turned off."))
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
    let room = lookup(guild, channel)?
        .ok_or_else(|| user_error("Loduchand doesn't manage this voice channel."))?;
    if controls(actor, UserId::new(room.owner_id)) {
        Ok(())
    } else {
        Err(user_error("Only the room owner or staff can do this."))
    }
}

fn check_create_capacity(limit: usize, rooms: &[Room]) -> Result<()> {
    if !room_limit_reached(Some(limit), rooms) {
        return Ok(());
    }

    // Leave room in Discord's 2,000-character reply for accounts that used to be staff.
    const MAX_ROOM_MENTIONS: usize = 70;
    let mut mentions = rooms
        .iter()
        .take(MAX_ROOM_MENTIONS)
        .map(|room| format!("<#{}>", room.channel_id))
        .collect::<Vec<_>>()
        .join(", ");
    if rooms.len() > MAX_ROOM_MENTIONS {
        mentions.push_str(&format!(" and {} more", rooms.len() - MAX_ROOM_MENTIONS));
    }
    Err(user_error(format!(
        "You've reached your temporary voice room limit of {limit}. You own {mentions}."
    )))
}

fn transfer_reply(transferred: bool, target: UserId) -> Result<String> {
    if transferred {
        Ok(format!("👑 <@{target}> now owns this room."))
    } else {
        Err(user_error(
            "This member has reached their temporary voice room limit.",
        ))
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
    let (action, force) = match action {
        Action::Force(force) => {
            if !actor.staff {
                return Err(user_error("Only staff can use the room manager."));
            }
            let action = match force {
                ForceAction::Claim => Action::Claim,
                ForceAction::Transfer(user) => Action::Transfer(user),
                ForceAction::Delete => Action::Delete,
            };
            (action, true)
        }
        action => (action, false),
    };
    match action {
        Action::Me(action) => return me(ctx, guild, member, actor, action).await,
        Action::Knock(channel) => return knock::request(ctx, guild, member, channel).await,
        _ => {}
    }
    if let Action::Create {
        hub,
        name,
        limit,
        locked,
    } = action
    {
        let _owner_guard = owner_lock(guild, actor.id).await;
        check_enabled()?;
        let actor = self::actor(ctx, guild, member)?;
        if let Some(limit) = owned_room_limit(actor.staff) {
            check_create_capacity(limit, &live_owned_rooms(ctx, guild, actor.id).await?)?;
        }
        let hub = select_hub(ctx, guild, hub, actor.voice)?;
        let _creation = CreationGuard::start(guild, actor.id)
            .ok_or_else(|| user_error("Wait 10 seconds before creating another room."))?;
        let options = RoomOptions {
            hub: Some(hub),
            name: name.as_deref(),
            limit,
            locked,
            schedule_id: None,
            invite_role: None,
        };
        let channel = create_room(ctx, guild, member, actor.voice, options).await?;
        return Ok(Reply::Text(format!(
            "🎙️ Your room is ready in <#{channel}>."
        )));
    }

    let channel = resolve(actor, target)?;
    lookup(guild, channel)?
        .ok_or_else(|| user_error("Loduchand doesn't manage this voice channel."))?;
    let room_lock = room_state(channel);
    let mut state = room_lock.lock().await;
    check_enabled()?;
    let actor = self::actor(ctx, guild, member)?;
    if resolve(actor, target)? != channel {
        return Err(user_error("You switched rooms. Try again."));
    }
    let room = lookup(guild, channel)?.ok_or_else(|| user_error("This temporary room is gone."))?;
    let owner = UserId::new(room.owner_id);
    if force && !actor.staff {
        return Err(user_error("Only staff can use the room manager."));
    }
    match action {
        Action::Info | Action::Claim => {}
        Action::Drag(_) => {
            if !actor.staff {
                return Err(user_error("Only staff can move members."));
            }
        }
        _ if !controls(actor, owner) => {
            return Err(user_error("Only the room owner or staff can do this."));
        }
        _ => {}
    }
    let Some(ch) = fetch_voice(ctx, guild, channel).await? else {
        forget_room(ctx, room, &mut state)?;
        return Err(user_error("This temporary room is gone."));
    };

    let log =
        |action, target| audit::post(ctx, guild, action, Some(actor.id), Some(channel), target);
    let staff_action = force || (actor.staff && actor.id != owner);
    let result = match action {
        Action::Create { .. } | Action::Me(_) | Action::Knock(_) | Action::Force(_) => {
            unreachable!("handled before selecting a room")
        }
        Action::Lock(value) | Action::Hide(value) => {
            let is_lock = matches!(action, Action::Lock(_));
            let toggle_type = if is_lock {
                Toggle::Connect
            } else {
                Toggle::View
            };
            let previous = room_restricted(&ch, toggle_type.bits());
            let restricted = value.unwrap_or(!previous);
            let result = toggle(ctx, &ch, owner, toggle_type, Some(restricted)).await?;
            if restricted != previous {
                log(
                    if is_lock {
                        audit::Action::Lock(restricted)
                    } else {
                        audit::Action::Hide(restricted)
                    },
                    None,
                );
            }
            result
        }
        Action::Chat(value) => {
            toggle(ctx, &ch, owner, Toggle::Chat, value.map(|enabled| !enabled)).await?
        }
        Action::Soundboard(value) => {
            toggle(
                ctx,
                &ch,
                owner,
                Toggle::Soundboard,
                value.map(|enabled| !enabled),
            )
            .await?
        }
        Action::Rename(pattern) => {
            let count = store::count_for_hub(&*db()?, room.hub_id)?;
            let name = expand_name(&pattern, member.display_name(), count);
            rename(ctx, &ch, &mut state, &room_lock, name, actor.id).await?
        }
        Action::Limit(limit) => {
            if limit > 99 {
                return Err(user_error(
                    "Use a limit from 0 to 99. Set it to 0 for no limit.",
                ));
            }
            let applied = caps::for_member(ctx, guild, member, actor.staff)?.limit(limit);
            channel
                .edit(&ctx.http, EditChannel::new().user_limit(applied))
                .await?;
            if applied == 0 {
                "Removed the room limit.".into()
            } else if applied != limit {
                format!("Set the room limit to your maximum of {applied}.")
            } else {
                format!("Set the room limit to {applied}.")
            }
        }
        Action::Bitrate(kbps) => {
            let cap = caps::for_member(ctx, guild, member, actor.staff)?.bitrate_kbps;
            let applied = kbps.clamp(8, cap);
            channel
                .edit(&ctx.http, EditChannel::new().bitrate(applied * 1_000))
                .await?;
            if kbps > cap {
                format!("Set the bitrate to your maximum of {applied} kbps.")
            } else {
                format!("Set the bitrate to {applied} kbps.")
            }
        }
        Action::Region(region) => {
            let label = region.clone().unwrap_or_else(|| "auto".into());
            channel
                .edit(&ctx.http, EditChannel::new().voice_region(region))
                .await?;
            format!("Set the voice region to {label}.")
        }
        Action::Permit(ref users) | Action::Deny(ref users) => {
            let permit = matches!(action, Action::Permit(_));
            return Ok(room_access(ctx, &ch, member, actor, owner, users, permit).await);
        }
        Action::Ban(users) => {
            let scope = ListScope {
                guild,
                owner,
                channel: Some(channel),
            };
            let result =
                change_owner_list(ctx, member, actor, scope, &users, ListChange::Ban).await?;
            drop(state);
            return Ok(sync_list_reply(ctx, scope, result, "Added to the owner's ban list").await);
        }
        Action::Kick(target)
        | Action::Disconnect(target)
        | Action::Reset(target)
        | Action::Drag(target) => {
            if matches!(action, Action::Reset(_)) && target == owner {
                return Err(user_error(
                    "Transfer the room before resetting the owner's permissions.",
                ));
            }
            checked_member(ctx, guild, member, actor.staff, owner, target).await?;
            let result = member_action(ctx, &ch, owner, target, &action).await?;
            match action {
                Action::Drag(_) => log(audit::Action::Drag, Some(target)),
                Action::Kick(_) => log(audit::Action::Kick, Some(target)),
                _ => {}
            }
            result
        }
        Action::Claim => {
            if !force && store::is_banned(&*db()?, room.guild_id, room.owner_id, actor.id.get())? {
                return Err(user_error("You're on this room owner's ban list."));
            }
            let owner_inside = voice(ctx, guild, owner) == Some(channel);
            let actor = self::actor(ctx, guild, member)?;
            if !can_claim(actor, channel, owner, owner_inside) {
                return Err(user_error(if actor.id == owner {
                    "You already own this room."
                } else if owner_inside {
                    "The owner is still in the room."
                } else {
                    "Join the room before claiming it."
                }));
            }
            if owner_inside {
                checked_member(ctx, guild, member, actor.staff, owner, owner).await?;
            }
            if !give_ownership(ctx, &ch, room, actor, OwnershipChange::Claim).await? {
                return Err(user_error("You've reached your temporary voice room limit."));
            }
            log(
                if staff_action {
                    audit::Action::StaffClaim
                } else {
                    audit::Action::Claim
                },
                Some(owner),
            );
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
            "👑 You now own this room.".into()
        }
        Action::Transfer(target) => {
            let target_member =
                checked_member(ctx, guild, member, actor.staff, owner, target).await?;
            if target_member.user.bot || target == owner {
                return Err(user_error("You can't transfer the room to this member."));
            }
            if !actor.staff && voice(ctx, guild, target) != Some(channel) {
                return Err(user_error("The new owner must be in the room."));
            }
            let receiver = self::actor(ctx, guild, &target_member)?;
            let transferred = give_ownership(
                ctx,
                &ch,
                room,
                receiver,
                OwnershipChange::Transfer {
                    by_staff: actor.staff,
                },
            )
            .await?;
            let reply = transfer_reply(transferred, target)?;
            log(
                if staff_action {
                    audit::Action::StaffTransfer
                } else {
                    audit::Action::Transfer
                },
                Some(target),
            );
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
            reply
        }
        Action::Delete => {
            delete_room(
                ctx,
                room,
                &mut state,
                "Temp voice room deleted by owner or staff",
                Some(actor.id),
                staff_action,
            )
            .await?;
            "Deleted the room.".into()
        }
        Action::Panel => {
            tempvc_panel::post_panel(ctx, channel).await?;
            format!("Posted the panel in the chat for <#{channel}>.")
        }
        Action::Info => {
            let members = occupants(ctx, guild, channel)
                .map(|occupants| occupants.all)
                .unwrap_or_default();
            return Ok(Reply::info(tempvc_panel::info_embed(&ch, owner, &members)));
        }
    };
    Ok(Reply::Text(result))
}

async fn me(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    actor: Actor,
    action: MeAction,
) -> Result<Reply> {
    let guild_id = guild.get();
    let user_id = actor.id.get();
    let text = match action {
        MeAction::Save => {
            let save_error = || user_error("Join your temporary voice room to save its settings.");
            let channel = actor.voice.ok_or_else(save_error)?;
            let room_lock = room_state(channel);
            let _state = room_lock.lock().await;
            if !lookup(guild, channel)?.is_some_and(|room| room.owner_id == user_id)
                || voice(ctx, guild, actor.id) != Some(channel)
            {
                return Err(save_error());
            }
            let ch = fetch_voice(ctx, guild, channel)
                .await?
                .ok_or_else(save_error)?;
            let caps = caps::for_member(ctx, guild, member, actor.staff)?;
            let profile = Profile {
                name: Some(ch.name.clone()),
                limit: Some(caps.limit(ch.user_limit.unwrap_or_default())),
                locked: Some(room_restricted(&ch, Permissions::CONNECT)),
                hidden: Some(room_restricted(&ch, Permissions::VIEW_CHANNEL)),
                bitrate: ch.bitrate.map(|bitrate| caps.bitrate(bitrate)),
            };
            store::save_profile(&*db()?, guild_id, user_id, &profile)?;
            "Saved your settings for new rooms."
        }
        MeAction::Set(mut profile) => {
            if profile == Profile::default() {
                return Err(user_error("Choose at least one setting to change."));
            }
            let caps = caps::for_member(ctx, guild, member, actor.staff)?;
            profile.limit = profile.limit.map(|limit| caps.limit(limit));
            profile.bitrate = profile.bitrate.map(|bitrate| caps.bitrate(bitrate));
            store::save_profile(&*db()?, guild_id, user_id, &profile)?;
            "Updated your room defaults."
        }
        MeAction::Profile => return profile_reply(guild_id, user_id),
        MeAction::Stats(target) => return usage::execute_stats(guild, target.unwrap_or(actor.id)),
        MeAction::Top => return usage::execute_top(guild),
        MeAction::Clear => {
            store::clear_profile(&*db()?, guild_id, user_id)?;
            "Cleared your saved defaults."
        }
        MeAction::Trust(ref users)
        | MeAction::Untrust(ref users)
        | MeAction::Ban(ref users)
        | MeAction::Unban(ref users) => {
            let (change, message) = match action {
                MeAction::Trust(_) => (ListChange::Trust, "Added to your trusted list"),
                MeAction::Untrust(_) => (ListChange::Untrust, "Removed from your trusted list"),
                MeAction::Ban(_) => (ListChange::Ban, "Added to your ban list"),
                MeAction::Unban(_) => (ListChange::Unban, "Removed from your ban list"),
                _ => unreachable!(),
            };
            let scope = ListScope {
                guild,
                owner: actor.id,
                channel: None,
            };
            let result = change_owner_list(ctx, member, actor, scope, users, change).await?;
            return Ok(sync_list_reply(ctx, scope, result, message).await);
        }
    };
    Ok(Reply::Text(text.into()))
}

#[derive(Default)]
struct BulkResult {
    applied: Vec<UserId>,
    skipped: Vec<(String, Vec<String>)>,
}

impl BulkResult {
    fn new(users: &BulkUsers) -> Self {
        let mut result = Self::default();
        for token in &users.invalid {
            result.skip(
                format!("`{}`", token.replace('`', "'")),
                "Use a member mention or ID.",
            );
        }
        result
    }

    fn skip(&mut self, label: String, reason: impl Into<String>) {
        let reason = reason.into();
        if let Some((_, labels)) = self.skipped.iter_mut().find(|(text, _)| *text == reason) {
            labels.push(label);
        } else {
            self.skipped.push((reason, vec![label]));
        }
    }

    fn skip_user(&mut self, user: UserId, error: anyhow::Error) {
        self.skip(format!("<@{user}>"), tempvc_panel::error_text(error));
    }

    fn reply(self, label: &str, note: Option<&str>) -> Reply {
        let mut lines = Vec::new();
        if self.applied.is_empty() {
            lines.push("No members were updated.".into());
        } else {
            let members = self
                .applied
                .iter()
                .map(|user| format!("<@{user}>"))
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(format!("**{label}**\n{members}"));
        }
        if !self.skipped.is_empty() {
            lines.push("**Skipped**".into());
            for (reason, users) in self.skipped {
                lines.push(format!("{} — {reason}", users.join(", ")));
            }
        }
        if let Some(note) = note {
            lines.push(note.into());
        }
        Reply::info(CreateEmbed::new().description(lines.join("\n")))
    }
}

#[derive(Clone, Copy)]
struct ListScope {
    guild: GuildId,
    owner: UserId,
    channel: Option<ChannelId>,
}

#[derive(Clone, Copy)]
enum ListChange {
    Trust,
    Untrust,
    Ban,
    Unban,
}

impl ListChange {
    fn update(self) -> (Access, bool, &'static str) {
        match self {
            Self::Trust => (Access::Trusted, true, "Already on the trusted list."),
            Self::Untrust => (Access::Trusted, false, "Not on the trusted list."),
            Self::Ban => (Access::Banned, true, "Already on the ban list."),
            Self::Unban => (Access::Banned, false, "Not on the ban list."),
        }
    }
}

fn save_owner_list(
    conn: &mut rusqlite::Connection,
    scope: ListScope,
    change: ListChange,
    users: &[UserId],
    result: &mut BulkResult,
) -> Result<()> {
    let (access, add, unchanged) = change.update();
    let guild = scope.guild.get();
    let owner = scope.owner.get();
    let tx = conn.transaction()?;
    let existing = store::access_list(&tx, guild, owner, access)?;
    for &user in users {
        if existing.contains(&user.get()) == add {
            result.skip(format!("<@{user}>"), unchanged);
            continue;
        }
        store::set_access(&tx, guild, owner, user.get(), access, add)?;
        result.applied.push(user);
    }
    tx.commit()?;
    Ok(())
}

async fn verify_list_target(
    ctx: &Context,
    member: &Member,
    actor: Actor,
    scope: ListScope,
    change: ListChange,
    user: UserId,
) -> Result<()> {
    if user == scope.owner {
        return Err(user_error("Choose someone other than the list owner."));
    }
    if user == actor.id || user == ctx.cache.current_user().id {
        return Err(user_error("You can't do this to yourself or Loduchand."));
    }
    let target = match change {
        ListChange::Trust => member_if_present(ctx, scope.guild, user)
            .await?
            .ok_or_else(|| user_error("This member is no longer in the server."))?,
        ListChange::Ban => {
            checked_member(ctx, scope.guild, member, actor.staff, scope.owner, user).await?
        }
        ListChange::Untrust | ListChange::Unban => return Ok(()),
    };
    if target.user.bot {
        return Err(user_error("Choose a member who isn't a bot."));
    }
    Ok(())
}

async fn change_owner_list(
    ctx: &Context,
    member: &Member,
    actor: Actor,
    scope: ListScope,
    users: &BulkUsers,
    change: ListChange,
) -> Result<(BulkResult, bool)> {
    let mut result = BulkResult::new(users);
    let mut valid = Vec::new();
    for &user in &users.users {
        match verify_list_target(ctx, member, actor, scope, change, user).await {
            Ok(()) => valid.push(user),
            Err(error) => result.skip_user(user, error),
        }
    }
    if valid.is_empty() {
        return Ok((result, false));
    }
    save_owner_list(&mut *db()?, scope, change, &valid, &mut result)?;
    if let Some(action) = match change {
        ListChange::Ban => Some(audit::Action::Ban),
        ListChange::Unban => Some(audit::Action::Unban),
        _ => None,
    } {
        for &user in &result.applied {
            audit::post(
                ctx,
                scope.guild,
                action,
                Some(actor.id),
                scope.channel,
                Some(user),
            );
        }
    }
    // Also sync unchanged entries so repeating a command can recover a partial Discord update.
    Ok((result, true))
}

async fn sync_list_reply(
    ctx: &Context,
    scope: ListScope,
    (result, sync): (BulkResult, bool),
    label: &str,
) -> Reply {
    let note = if sync {
        access::sync_owner_rooms(ctx, scope.guild, scope.owner)
            .await
            .err()
            .map(tempvc_panel::error_text)
    } else {
        None
    };
    result.reply(label, note.as_deref())
}

fn profile_reply(guild: u64, owner: u64) -> Result<Reply> {
    fn flag(value: Option<bool>) -> &'static str {
        match value {
            Some(true) => "On",
            Some(false) => "Off",
            None => "Default",
        }
    }
    fn mentions(users: &[u64]) -> String {
        // A Discord embed field holds 1,024 characters. Reserve room for the count.
        let mut text = String::new();
        for (index, user) in users.iter().enumerate() {
            let mention = format!("<@{user}>");
            if text.len() + mention.len() > 980 {
                text.push_str(&format!("… and {} more", users.len() - index));
                break;
            }
            text.push_str(&mention);
            if index + 1 < users.len() {
                text.push_str(", ");
            }
        }
        if text.is_empty() { "None".into() } else { text }
    }

    let conn = db()?;
    let profile = store::profile(&conn, guild, owner)?;
    let trusted = mentions(&store::access_list(&conn, guild, owner, Access::Trusted)?);
    let banned = mentions(&store::access_list(&conn, guild, owner, Access::Banned)?);
    let mut embed = CreateEmbed::new().title("🎙️ Your room profile");
    if let Some(profile) = profile {
        let limit = match profile.limit {
            Some(0) => "Unlimited".into(),
            Some(limit) => limit.to_string(),
            None => "Default".into(),
        };
        let bitrate = profile
            .bitrate
            .map_or_else(|| "Default".into(), |bps| format!("{} kbps", bps / 1_000));
        embed = embed
            .field("Name", profile.name.as_deref().unwrap_or("Default"), true)
            .field("Limit", limit, true)
            .field("Bitrate", bitrate, true)
            .field("Lock", flag(profile.locked), true)
            .field("Hide", flag(profile.hidden), true);
    } else {
        embed = embed.description("Use `/vc me save` or `/vc me set` to save your defaults.");
    }
    Ok(Reply::info(
        embed
            .field("Trusted friends", trusted, false)
            .field("Ban list", banned, false),
    ))
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
            Err(user_error("Choose a temporary voice hub."))
        };
    }
    if let Some(hub) = current.filter(|channel| hubs.contains(channel)) {
        return Ok(hub);
    }
    match hubs.len() {
        0 => Err(user_error("No temporary voice hubs are set up.")),
        1 => Ok(hubs.remove(0)),
        _ => Err(user_error("Choose a hub using the hub option.")),
    }
}

pub(super) fn bitrate_cap(ctx: &Context, id: GuildId) -> Result<u32> {
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
            (Self::Connect, true) => "🔒 Room locked.",
            (Self::Connect, false) => "🔓 Room unlocked.",
            (Self::View, true) => "🙈 Room hidden.",
            (Self::View, false) => "👀 Room is now visible.",
            (Self::Chat, true) => "💬 Chat is read-only for members without access.",
            (Self::Chat, false) => "💬 Chat is on.",
            (Self::Soundboard, true) => "🔇 Soundboard is off.",
            (Self::Soundboard, false) => "🔊 Soundboard is on.",
        }
    }
}

async fn toggle(
    ctx: &Context,
    channel: &GuildChannel,
    owner: UserId,
    toggle: Toggle,
    restrict: Option<bool>,
) -> Result<String> {
    let everyone = PermissionOverwriteType::Role(RoleId::new(channel.guild_id.get()));
    let bits = toggle.bits();
    let everyone_ow = overwrite(&channel.permission_overwrites, everyone);
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
        remove_restriction(&mut overwrites, channel.guild_id, bits);
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
    if matches!(toggle, Toggle::Connect | Toggle::View | Toggle::Chat) {
        access::sync(ctx, channel, owner, overwrites).await?;
    } else if channel.permission_overwrites != overwrites {
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

pub(super) fn room_restricted(channel: &GuildChannel, bits: Permissions) -> bool {
    overwrite(
        &channel.permission_overwrites,
        PermissionOverwriteType::Role(channel.guild_id.everyone_role()),
    )
    .deny
    .contains(bits)
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
        return Err(user_error("You can't do this to yourself or Loduchand."));
    }
    let target_member = member_if_present(ctx, guild, target)
        .await?
        .ok_or_else(|| user_error("This member is no longer in the server."))?;
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
            "This member's highest role is equal to or above yours."
        } else {
            "You can't do this to staff or the room owner."
        }));
    }
    Ok(target_member)
}

pub(super) async fn member_if_present(
    ctx: &Context,
    guild: GuildId,
    target: UserId,
) -> Result<Option<Member>> {
    match guild.member(ctx, target).await {
        Ok(member) => Ok(Some(member)),
        Err(serenity::Error::Http(HttpError::UnsuccessfulRequest(response)))
            if response.error.code == 10007 =>
        {
            Ok(None)
        }
        Err(err) => Err(err.into()),
    }
}

async fn room_access(
    ctx: &Context,
    channel: &GuildChannel,
    member: &Member,
    actor: Actor,
    owner: UserId,
    users: &BulkUsers,
    permit: bool,
) -> Reply {
    let mut result = BulkResult::new(users);
    for &user in &users.users {
        let updated = async {
            checked_member(ctx, channel.guild_id, member, actor.staff, owner, user).await?;
            apply_room_access(ctx, channel, owner, user, permit).await
        }
        .await;
        match updated {
            Ok(()) => {
                result.applied.push(user);
                if !permit {
                    audit::post(
                        ctx,
                        channel.guild_id,
                        audit::Action::Deny,
                        Some(actor.id),
                        Some(channel.id),
                        Some(user),
                    );
                }
            }
            Err(error) => result.skip_user(user, error),
        }
    }
    let label = if permit {
        "Allowed into this room"
    } else {
        "Blocked from joining this room"
    };
    result.reply(label, None)
}

fn check_room_list(
    guild: GuildId,
    owner: UserId,
    target: UserId,
    check_ban: bool,
    check_trust: bool,
) -> Result<()> {
    let conn = db()?;
    if check_ban && store::is_banned(&conn, guild.get(), owner.get(), target.get())? {
        return Err(user_error(
            "They're on the owner's ban list. The owner can use `/vc me unban`.",
        ));
    }
    if check_trust
        && store::access_list(&conn, guild.get(), owner.get(), Access::Trusted)?
            .contains(&target.get())
    {
        return Err(user_error(
            "They're trusted by the owner. The owner can use `/vc me untrust`.",
        ));
    }
    Ok(())
}

fn set_room_access(ow: &mut PermissionOverwrite, permit: bool) {
    if permit {
        allow_bits(ow, PERMIT_ACCESS);
    } else {
        deny_bits(ow, Permissions::CONNECT);
        ow.allow.remove(Permissions::SEND_MESSAGES);
    }
}

async fn apply_room_access(
    ctx: &Context,
    channel: &GuildChannel,
    owner: UserId,
    target: UserId,
    permit: bool,
) -> Result<()> {
    check_room_list(channel.guild_id, owner, target, permit, !permit)?;
    let id = channel.id;
    let kind = PermissionOverwriteType::Member(target);
    let before = overwrite(&channel.permission_overwrites, kind);
    let mut after = before.clone();
    set_room_access(&mut after, permit);
    if target == owner {
        allow_bits(&mut after, Permissions::SEND_MESSAGES);
    }
    if after != before {
        write_overwrite(ctx, id, &after).await?;
    }
    if permit {
        // Keep a room-specific permit if this member later leaves the trusted list.
        access::record_allow(id, target, PERMIT_ACCESS)?;
    } else {
        access::forget_permit(id, target)?;
    }
    Ok(())
}

async fn disconnect_once(
    current_room: Option<ChannelId>,
    room: ChannelId,
    target: UserId,
    disconnect: impl AsyncFnOnce(UserId) -> Result<()>,
) -> Result<String> {
    if current_room != Some(room) {
        return Err(user_error("This member isn't in the room."));
    }
    disconnect(target).await?;
    Ok(format!("Disconnected <@{target}>."))
}

async fn member_action(
    ctx: &Context,
    channel: &GuildChannel,
    owner: UserId,
    target: UserId,
    action: &Action,
) -> Result<String> {
    let guild = channel.guild_id;
    let id = channel.id;
    match action {
        Action::Reset(_) => {
            check_room_list(guild, owner, target, true, true)?;
            let kind = PermissionOverwriteType::Member(target);
            let before = overwrite(&channel.permission_overwrites, kind);
            if !(before.allow | before.deny).is_empty() {
                let after = PermissionOverwrite {
                    kind,
                    allow: Permissions::empty(),
                    deny: Permissions::empty(),
                };
                write_overwrite(ctx, id, &after).await?;
            }
            access::forget_permit(id, target)?;
            access::forget_suppressed_chat(id, target)?;
            Ok(format!("Reset room permissions for <@{target}>."))
        }
        Action::Kick(_) | Action::Disconnect(_) => {
            disconnect_once(voice(ctx, guild, target), id, target, async |target| {
                guild.disconnect_member(&ctx.http, target).await?;
                Ok(())
            })
            .await
        }
        Action::Drag(_) => {
            check_room_list(guild, owner, target, true, false)?;
            let source = voice(ctx, guild, target)
                .ok_or_else(|| user_error("This member isn't in a voice channel."))?;
            if source == id {
                return Err(user_error("This member is already in the room."));
            }
            guild.move_member(&ctx.http, target, id).await?;
            Ok(format!("Moved <@{target}> to <#{id}>."))
        }
        _ => unreachable!("only member actions are routed here"),
    }
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
    actor: UserId,
) -> Result<String> {
    let id = channel.id;
    let mut record = load_rename(id)?;
    if channel.name == name {
        if record.pending.take().is_some() {
            record.actor_id = None;
            save_rename(id, &record)?;
        }
        return Ok("The room already has this name.".into());
    }
    let delay = rename_delay(&record, chrono::Utc::now().timestamp_millis());
    record.pending = Some(name);
    record.actor_id = Some(actor.get());
    save_rename(id, &record)?;
    if delay.is_zero() {
        apply_rename(ctx, id, record).await?;
        Ok("Renamed the room.".into())
    } else {
        start_rename(ctx, id, state, lock);
        Ok(format!(
            "The room will be renamed in about {} seconds.",
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
    if let Ok(edited) = &result {
        record.latest_ms = Some(chrono::Utc::now().timestamp_millis());
        audit::post(
            ctx,
            edited.guild_id,
            audit::Action::Rename,
            record.actor_id.map(UserId::new),
            Some(channel),
            None,
        );
    } else {
        record = before;
    }
    record.pending = None;
    record.actor_id = None;
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

    fn owned_rooms() -> [Room; 3] {
        [30, 20, 10].map(|channel_id| Room {
            channel_id,
            guild_id: 1,
            hub_id: 2,
            owner_id: 3,
        })
    }

    #[test]
    fn slash_create_refuses_at_capacity_and_names_the_members_rooms() {
        let rooms = owned_rooms();
        assert!(check_create_capacity(3, &rooms[..2]).is_ok());
        let err = check_create_capacity(3, &rooms).unwrap_err();
        assert_eq!(
            tempvc_panel::error_text(err),
            "You've reached your temporary voice room limit of 3. You own <#30>, <#20>, <#10>."
        );
    }

    #[test]
    fn transfer_refuses_a_receiver_at_capacity() {
        let rooms = owned_rooms();
        let receiver = UserId::new(rooms[0].owner_id);
        assert!(transfer_reply(!room_limit_reached(Some(3), &rooms[..2]), receiver).is_ok());
        assert!(transfer_reply(!room_limit_reached(None, &rooms), receiver).is_ok());
        let err = transfer_reply(!room_limit_reached(Some(3), &rooms), receiver).unwrap_err();
        assert_eq!(
            tempvc_panel::error_text(err),
            "This member has reached their temporary voice room limit."
        );
    }

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

    #[tokio::test]
    async fn kick_disconnects_once_without_banning_or_removing_trust() {
        let dir = tempfile::tempdir().unwrap();
        let conn = store::connect(&dir.path().join("tempvc.db")).unwrap();
        let room = Room {
            guild_id: 1,
            channel_id: 2,
            hub_id: 3,
            owner_id: 4,
        };
        let target = UserId::new(5);
        store::set_access(
            &conn,
            room.guild_id,
            room.owner_id,
            target.get(),
            Access::Trusted,
            true,
        )
        .unwrap();

        let channel = ChannelId::new(room.channel_id);
        let mut disconnected = Vec::new();
        let reply = disconnect_once(Some(channel), channel, target, async |user| {
            disconnected.push(user);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(reply, "Disconnected <@5>.");
        let absent = disconnect_once(None, channel, target, async |user| {
            disconnected.push(user);
            Ok(())
        })
        .await;
        assert!(absent.is_err());
        assert_eq!(disconnected, [target]);
        assert!(!store::is_banned(&conn, room.guild_id, room.owner_id, target.get()).unwrap());
        assert_eq!(
            store::access_list(&conn, room.guild_id, room.owner_id, Access::Trusted).unwrap(),
            [target.get()]
        );
        assert!(
            store::room_access(&conn, room.channel_id)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn room_permit_grants_chat_and_deny_retracts_the_grant() {
        let kind = PermissionOverwriteType::Member(UserId::new(1));
        let mut ow = overwrite(&[], kind);
        ow.deny.insert(Permissions::SEND_MESSAGES);
        ow.allow.insert(Permissions::READ_MESSAGE_HISTORY);
        set_room_access(&mut ow, true);
        assert!(
            ow.allow
                .contains(PERMIT_ACCESS | Permissions::READ_MESSAGE_HISTORY)
        );
        assert!(!ow.deny.intersects(PERMIT_ACCESS));

        set_room_access(&mut ow, false);
        assert!(ow.deny.contains(Permissions::CONNECT));
        assert!(
            !ow.allow
                .intersects(Permissions::CONNECT | Permissions::SEND_MESSAGES)
        );
        assert!(
            ow.allow
                .contains(Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY)
        );
    }

    #[test]
    fn bulk_list_changes_apply_to_the_room_owner_and_report_skips() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = store::connect(&dir.path().join("tempvc.db")).unwrap();
        let scope = ListScope {
            guild: GuildId::new(1),
            owner: UserId::new(2),
            channel: Some(ChannelId::new(3)),
        };
        let first = UserId::new(4);
        let second = UserId::new(5);
        let members = [first, second];
        let mut result = BulkResult::default();
        save_owner_list(&mut conn, scope, ListChange::Trust, &members, &mut result).unwrap();
        assert_eq!(result.applied, members);
        save_owner_list(
            &mut conn,
            scope,
            ListChange::Untrust,
            &members,
            &mut BulkResult::default(),
        )
        .unwrap();
        assert!(
            store::access_list(&conn, 1, 2, Access::Trusted)
                .unwrap()
                .is_empty()
        );

        // Banning from a room uses the same persistent list as /vc me ban
        // and replaces trust for the affected members.
        save_owner_list(
            &mut conn,
            scope,
            ListChange::Trust,
            &members,
            &mut BulkResult::default(),
        )
        .unwrap();
        save_owner_list(
            &mut conn,
            scope,
            ListChange::Ban,
            &[first],
            &mut BulkResult::default(),
        )
        .unwrap();
        let mut result = BulkResult::new(&BulkUsers {
            users: members.to_vec(),
            invalid: vec!["@someone".into()],
        });
        save_owner_list(&mut conn, scope, ListChange::Ban, &members, &mut result).unwrap();
        assert_eq!(result.applied, [second]);
        assert_eq!(
            store::access_list(&conn, 1, 2, Access::Banned).unwrap(),
            [4, 5]
        );
        assert!(
            store::access_list(&conn, 1, 2, Access::Trusted)
                .unwrap()
                .is_empty()
        );
        assert!(
            store::access_list(&conn, 1, 6, Access::Banned)
                .unwrap()
                .is_empty()
        );
        let Reply::Info(embed) = result.reply("Added to your ban list", None) else {
            panic!("expected a summary embed");
        };
        assert_eq!(
            serde_json::to_value(embed).unwrap()["description"],
            "**Added to your ban list**\n<@5>\n**Skipped**\n`@someone` — Use a member mention or ID.\n<@4> — Already on the ban list."
        );

        save_owner_list(
            &mut conn,
            scope,
            ListChange::Unban,
            &members,
            &mut BulkResult::default(),
        )
        .unwrap();
        assert!(
            store::access_list(&conn, 1, 2, Access::Banned)
                .unwrap()
                .is_empty()
        );
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
            actor_id: None,
        };
        assert_eq!(rename_delay(&second, 610_000), Duration::from_secs(1));
        assert_eq!(rename_delay(&second, 611_000), Duration::ZERO);
        assert_eq!(
            rename_delay(&second, 0),
            Duration::from_millis(RENAME_WINDOW_MS as u64)
        );
    }
}
