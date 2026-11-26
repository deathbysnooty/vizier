//! Shared policy and room edits for slash commands, panels and automatic handoff.

use serenity::all::{CreateAllowedMentions, CreateEmbed, CreateMessage, EditChannel, PremiumTier};

use super::*;
use crate::channels::discord::tempvc_panel::{Action, ForceAction, MeAction, Reply, RoomTarget};
use crate::channels::discord::tempvc_store::{Access, Rename};
use serenity::all::{Guild, PermissionOverwrite};

pub(super) const OWNER_ACCESS: Permissions = Permissions::VIEW_CHANNEL
    .union(Permissions::CONNECT)
    .union(Permissions::SPEAK)
    .union(Permissions::SEND_MESSAGES)
    .union(Permissions::READ_MESSAGE_HISTORY);
pub(super) const STAFF_ACCESS: Permissions = OWNER_ACCESS.union(Permissions::USE_SOUNDBOARD);
pub(super) const BOT_ACCESS: Permissions = OWNER_ACCESS.union(Permissions::EMBED_LINKS);

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

pub(super) async fn give_ownership(
    ctx: &Context,
    channel: &GuildChannel,
    room: Room,
    new_owner: UserId,
) -> Result<()> {
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
        Some(channel.id),
        &mut overwrites,
    )
    .await?;
    grant(
        &mut overwrites,
        PermissionOverwriteType::Member(new_owner),
        OWNER_ACCESS,
    );
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
    Ok(())
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
pub(super) struct Actor {
    pub(super) id: UserId,
    pub(super) staff: bool,
    pub(super) voice: Option<ChannelId>,
}

pub(super) fn actor(ctx: &Context, guild: GuildId, member: &Member) -> Result<Actor> {
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
    let (action, force) = match action {
        Action::Force(force) => {
            if !actor.staff {
                return Err(user_error("Room manager sirf staff ke liye hai."));
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
        let hub = select_hub(ctx, guild, hub, actor.voice)?;
        let _creation = CreationGuard::start(guild, actor.id)
            .ok_or_else(|| user_error("Room banane se pehle 10 second ruk ja."))?;
        if let Some(existing) = find_owned(ctx, guild, actor.id, None).await? {
            return Err(user_error(format!("Tera room pehle se hai <#{existing}>.")));
        }
        let options = RoomOptions {
            hub: Some(hub),
            name: name.as_deref(),
            limit,
            locked,
            schedule_id: None,
            invite_role: None,
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
    if force && !actor.staff {
        return Err(user_error("Room manager sirf staff ke liye hai."));
    }
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
        forget_room(ctx, room, &mut state)?;
        return Err(user_error("Ye temp VC ab nahi hai."));
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
            let name = expand_name(&pattern, member.display_name(), 1);
            rename(ctx, &ch, &mut state, &room_lock, name, actor.id).await?
        }
        Action::Limit(limit) => {
            if limit > 99 {
                return Err(user_error("Limit 0 se 99 tak rakh. 0 matlab unlimited."));
            }
            let applied = caps::for_member(ctx, guild, member, actor.staff)?.limit(limit);
            channel
                .edit(&ctx.http, EditChannel::new().user_limit(applied))
                .await?;
            if applied == 0 {
                "Room limit hata di.".into()
            } else if applied != limit {
                format!("Room limit {applied} kar di. Ye tera max hai.")
            } else {
                format!("Room limit {applied} kar di.")
            }
        }
        Action::Bitrate(kbps) => {
            let cap = caps::for_member(ctx, guild, member, actor.staff)?.bitrate_kbps;
            let applied = kbps.clamp(8, cap);
            channel
                .edit(&ctx.http, EditChannel::new().bitrate(applied * 1_000))
                .await?;
            if kbps > cap {
                format!("Bitrate {applied} kbps kar di. Ye tera max hai.")
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
            if matches!(action, Action::Kick(_)) {
                save_kick_ban(&*db()?, room, target)?;
                log(audit::Action::Kick, Some(target));
                drop(state);
                access::sync_owner_rooms(ctx, guild, owner).await?;
                return Ok(Reply::Text(format!(
                    "<@{target}> ko owner ki ban list mein daal diya."
                )));
            }
            let result = member_action(ctx, &ch, owner, target, &action).await?;
            match action {
                Action::Drag(_) => log(audit::Action::Drag, Some(target)),
                Action::Deny(_) => log(audit::Action::Deny, Some(target)),
                _ => {}
            }
            result
        }
        Action::Claim => {
            if !force && store::is_banned(&*db()?, room.guild_id, room.owner_id, actor.id.get())? {
                return Err(user_error("Tu is room ki ban list mein hai."));
            }
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
            format!("👑 <@{target}> ab room ka owner hai.")
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
            let save_error = || user_error("Apne temp VC mein jaake settings save kar.");
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
            "Room ki settings save kar di. Naye rooms mein apply hongi."
        }
        MeAction::Set(mut profile) => {
            if profile == Profile::default() {
                return Err(user_error("Badalne ke liye kam se kam ek option de."));
            }
            let caps = caps::for_member(ctx, guild, member, actor.staff)?;
            profile.limit = profile.limit.map(|limit| caps.limit(limit));
            profile.bitrate = profile.bitrate.map(|bitrate| caps.bitrate(bitrate));
            store::save_profile(&*db()?, guild_id, user_id, &profile)?;
            "Tere room defaults update ho gaye."
        }
        MeAction::Profile => return profile_reply(guild_id, user_id),
        MeAction::Stats(target) => return usage::execute_stats(guild, target.unwrap_or(actor.id)),
        MeAction::Top => return usage::execute_top(guild),
        MeAction::Clear => {
            store::clear_profile(&*db()?, guild_id, user_id)?;
            "Saved defaults hata diye."
        }
        MeAction::Trust(user)
        | MeAction::Untrust(user)
        | MeAction::Ban(user)
        | MeAction::Unban(user) => {
            if user == actor.id || user == ctx.cache.current_user().id {
                return Err(user_error("Kisi aur member ko choose kar."));
            }
            let (access, add, message) = match action {
                MeAction::Trust(_) => (Access::Trusted, true, "trusted list mein daal diya."),
                MeAction::Untrust(_) => (Access::Trusted, false, "trusted list se hata diya."),
                MeAction::Ban(_) => (Access::Banned, true, "ban list mein daal diya."),
                MeAction::Unban(_) => (Access::Banned, false, "ban list se hata diya."),
                _ => unreachable!(),
            };
            if add {
                let target = if access == Access::Banned {
                    checked_member(ctx, guild, member, actor.staff, actor.id, user).await?
                } else {
                    member_if_present(ctx, guild, user)
                        .await?
                        .ok_or_else(|| user_error("Ye member server mein nahi hai."))?
                };
                if target.user.bot {
                    return Err(user_error("Kisi aur member ko choose kar."));
                }
            }
            store::set_access(&*db()?, guild_id, user_id, user.get(), access, add)?;
            if access == Access::Banned {
                let action = if add {
                    audit::Action::Ban
                } else {
                    audit::Action::Unban
                };
                audit::post(ctx, guild, action, Some(actor.id), None, Some(user));
            }
            access::sync_owner_rooms(ctx, guild, actor.id).await?;
            return Ok(Reply::Text(format!("<@{user}> ko {message}")));
        }
    };
    Ok(Reply::Text(text.into()))
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
                text.push_str(&format!("… aur {}", users.len() - index));
                break;
            }
            text.push_str(&mention);
            if index + 1 < users.len() {
                text.push_str(", ");
            }
        }
        if text.is_empty() {
            "Koi nahi".into()
        } else {
            text
        }
    }

    let conn = db()?;
    let profile = store::profile(&conn, guild, owner)?;
    let trusted = mentions(&store::access_list(&conn, guild, owner, Access::Trusted)?);
    let banned = mentions(&store::access_list(&conn, guild, owner, Access::Banned)?);
    let mut embed = CreateEmbed::new().title("🎙️ Tera room profile");
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
            .field("Naam", profile.name.as_deref().unwrap_or("Default"), true)
            .field("Limit", limit, true)
            .field("Bitrate", bitrate, true)
            .field("Lock", flag(profile.locked), true)
            .field("Hide", flag(profile.hidden), true);
    } else {
        embed = embed
            .description("Abhi defaults save nahi hain. `/vc me save` ya `/vc me set` use kar.");
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
    if matches!(toggle, Toggle::Connect | Toggle::View) {
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
        return Err(user_error("Khud par ya Loduchand par ye nahi kar sakte."));
    }
    let target_member = member_if_present(ctx, guild, target)
        .await?
        .ok_or_else(|| user_error("Ye member server mein nahi hai."))?;
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

fn save_kick_ban(conn: &rusqlite::Connection, room: Room, target: UserId) -> Result<()> {
    if target.get() == room.owner_id {
        return Err(user_error(
            "Room owner ko disconnect kar sakte ho. Ban ke liye pehle room transfer kar.",
        ));
    }
    store::set_access(
        conn,
        room.guild_id,
        room.owner_id,
        target.get(),
        Access::Banned,
        true,
    )?;
    Ok(())
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
    if matches!(
        action,
        Action::Permit(_) | Action::Drag(_) | Action::Reset(_)
    ) && store::is_banned(&*db()?, guild.get(), owner.get(), target.get())?
    {
        return Err(user_error(
            "Ye owner ki ban list mein hai. Owner `/vc me unban` use kar sakta hai.",
        ));
    }
    if matches!(action, Action::Deny(_) | Action::Reset(_))
        && store::access_list(&*db()?, guild.get(), owner.get(), Access::Trusted)?
            .contains(&target.get())
    {
        return Err(user_error(
            "Ye owner ki trusted list mein hai. Owner `/vc me untrust` use kar sakta hai.",
        ));
    }
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
    if matches!(action, Action::Permit(_)) {
        // Keep a room-specific permit if this member later leaves the trusted list.
        access::record_allow(id, target, Permissions::CONNECT | Permissions::VIEW_CHANNEL)?;
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
    actor: UserId,
) -> Result<String> {
    let id = channel.id;
    let mut record = load_rename(id)?;
    if channel.name == name {
        if record.pending.take().is_some() {
            record.actor_id = None;
            save_rename(id, &record)?;
        }
        return Ok("Room ka naam already yahi hai.".into());
    }
    let delay = rename_delay(&record, chrono::Utc::now().timestamp_millis());
    record.pending = Some(name);
    record.actor_id = Some(actor.get());
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
    fn kick_bans_for_the_room_owner_and_removes_trust() {
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

        save_kick_ban(&conn, room, target).unwrap();
        assert!(store::is_banned(&conn, room.guild_id, room.owner_id, target.get()).unwrap());
        assert!(!store::is_banned(&conn, room.guild_id, 6, target.get()).unwrap());
        assert!(
            store::access_list(&conn, room.guild_id, room.owner_id, Access::Trusted)
                .unwrap()
                .is_empty()
        );
        assert!(save_kick_ban(&conn, room, UserId::new(room.owner_id)).is_err());
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
