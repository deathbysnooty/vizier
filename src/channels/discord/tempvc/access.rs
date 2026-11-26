//! Persistent room access layered over inherited and room-specific permissions.

use std::collections::{BTreeMap, BTreeSet};

use serenity::all::{EditChannel, PermissionOverwrite};

use super::actions::{
    PERMIT_ACCESS, allow_bits, deny_bits, grant, member_if_present, overwrite, restrict_roles,
    set_overwrite, staff_roles,
};
use super::*;
use store::{Access, RoomAccessBaseline};

type Baselines = BTreeMap<UserId, RoomAccessBaseline>;
type Lists = BTreeMap<UserId, Access>;
const ENTRY_BITS: Permissions = Permissions::VIEW_CHANNEL.union(Permissions::CONNECT);

#[derive(Default)]
pub(super) struct AccessEdits {
    added: Baselines,
    removed: Vec<UserId>,
    banned: Vec<UserId>,
    suppress_chat: Vec<UserId>,
    clear_suppressed_chat: Vec<UserId>,
    suppress_role_chat: Vec<u64>,
}

impl AccessEdits {
    pub(super) fn save_added(&self, conn: &rusqlite::Connection, channel: ChannelId) -> Result<()> {
        for (user, baseline) in &self.added {
            store::set_room_access(conn, channel.get(), user.get(), Some(*baseline))?;
        }
        for user in &self.suppress_chat {
            store::set_member_chat_suppressed(conn, channel.get(), user.get(), true)?;
        }
        if !self.suppress_role_chat.is_empty() {
            let chat = Permissions::SEND_MESSAGES.bits();
            let mut roles = store::load_suppressed_roles(conn, channel.get(), chat)?;
            roles.extend_from_slice(&self.suppress_role_chat);
            roles.sort_unstable();
            roles.dedup();
            store::save_suppressed_roles(conn, channel.get(), chat, &roles)?;
        }
        Ok(())
    }

    pub(super) fn save_removed(
        &self,
        conn: &rusqlite::Connection,
        channel: ChannelId,
    ) -> Result<()> {
        for user in &self.removed {
            store::set_room_access(conn, channel.get(), user.get(), None)?;
        }
        for user in &self.clear_suppressed_chat {
            store::set_member_chat_suppressed(conn, channel.get(), user.get(), false)?;
        }
        Ok(())
    }

    pub(super) async fn disconnect_banned(
        &self,
        ctx: &Context,
        guild: GuildId,
        channel: ChannelId,
    ) -> Result<()> {
        let mut failed = None;
        for user in &self.banned {
            if let Err(err) = disconnect_if_present(ctx, guild, channel, *user).await {
                tracing::warn!(%channel, %user, %err, "tempvc: could not disconnect banned member");
                failed = Some(err);
            }
        }
        failed.map_or(Ok(()), Err)
    }
}

fn apply_lists(
    overwrites: &mut Vec<PermissionOverwrite>,
    lists: &Lists,
    baselines: &Baselines,
) -> AccessEdits {
    let mut edits = AccessEdits::default();
    for (&user, baseline) in baselines {
        if !lists.contains_key(&user) {
            let mut restored = overwrite(overwrites, PermissionOverwriteType::Member(user));
            let bits = if baseline.tracks_chat { PERMIT_ACCESS } else { ENTRY_BITS };
            restored.allow = (restored.allow - bits) | Permissions::from_bits_truncate(baseline.allow);
            restored.deny = (restored.deny - bits) | Permissions::from_bits_truncate(baseline.deny);
            set_overwrite(overwrites, restored);
            edits.removed.push(user);
        }
    }
    for (&user, &access) in lists {
        let mut ow = overwrite(overwrites, PermissionOverwriteType::Member(user));
        match baselines.get(&user) {
            None => {
                edits.added.insert(user, RoomAccessBaseline {
                    allow: (ow.allow & PERMIT_ACCESS).bits(),
                    deny: (ow.deny & PERMIT_ACCESS).bits(),
                    tracks_chat: true,
                });
            }
            Some(baseline) if !baseline.tracks_chat => {
                let chat = Permissions::SEND_MESSAGES;
                edits.added.insert(user, RoomAccessBaseline {
                    allow: baseline.allow | (ow.allow & chat).bits(),
                    deny: baseline.deny | (ow.deny & chat).bits(),
                    tracks_chat: true,
                });
            }
            Some(_) => {}
        }
        match access {
            Access::Trusted => allow_bits(&mut ow, PERMIT_ACCESS),
            Access::Banned => {
                deny_bits(&mut ow, PERMIT_ACCESS);
                edits.banned.push(user);
            }
        }
        set_overwrite(overwrites, ow);
    }
    edits
}

fn grant_room_permit_chat(
    overwrites: &mut [PermissionOverwrite],
    exempt: &mut BTreeSet<UserId>,
    inherited: Option<&[PermissionOverwrite]>,
) {
    let Some(inherited) = inherited else { return };
    for ow in overwrites {
        let PermissionOverwriteType::Member(user) = ow.kind else {
            continue;
        };
        // Earlier room permits granted both entry bits. Inherited access and
        // knocks (CONNECT only) do not identify a legacy room permit.
        if ow.allow.contains(ENTRY_BITS)
            && !overwrite(inherited, ow.kind).allow.contains(ENTRY_BITS)
            && !ow.deny.contains(Permissions::SEND_MESSAGES)
        {
            allow_bits(ow, Permissions::SEND_MESSAGES);
            exempt.insert(user);
        }
    }
}

fn restrict_member_chat(
    overwrites: &mut Vec<PermissionOverwrite>,
    exempt: &BTreeSet<UserId>,
) -> BTreeSet<UserId> {
    let mut suppressed = BTreeSet::new();
    for ow in overwrites.iter_mut() {
        if let PermissionOverwriteType::Member(user) = ow.kind
            && ow.allow.contains(Permissions::SEND_MESSAGES)
            && !exempt.contains(&user)
        {
            ow.allow.remove(Permissions::SEND_MESSAGES);
            suppressed.insert(user);
        }
    }
    overwrites.retain(|ow| !(ow.allow | ow.deny).is_empty());
    suppressed
}

pub(super) async fn plan(
    ctx: &Context,
    guild: GuildId,
    owner: UserId,
    channel: Option<&GuildChannel>,
    overwrites: &mut Vec<PermissionOverwrite>,
) -> Result<AccessEdits> {
    let (mut lists, baselines, suppressed_chat, permits) = {
        let conn = db()?;
        let mut lists = Lists::new();
        for access in [Access::Trusted, Access::Banned] {
            for user in store::access_list(&conn, guild.get(), owner.get(), access)? {
                lists.insert(UserId::new(user), access);
            }
        }
        let baselines = channel
            .map(|ch| store::room_access(&conn, ch.id.get()))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|(id, baseline)| (UserId::new(id), baseline))
            .collect();
        let suppressed_chat = channel
            .map(|ch| store::room_suppressed_chat(&conn, ch.id.get()))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(UserId::new)
            .collect::<BTreeSet<_>>();
        let permits = channel
            .map(|ch| store::room_permits(&conn, ch.id.get()))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(UserId::new)
            .collect::<BTreeSet<_>>();
        (lists, baselines, suppressed_chat, permits)
    };
    // Restore the underlying chat grants before computing access-list changes.
    // This keeps a suppressed grant in the baseline if a member is later trusted or banned.
    for &user in &suppressed_chat {
        let mut ow = overwrite(overwrites, PermissionOverwriteType::Member(user));
        if !ow.deny.contains(Permissions::SEND_MESSAGES)
            && (!(ow.allow | ow.deny).is_empty()
                || member_if_present(ctx, guild, user).await?.is_some())
        {
            allow_bits(&mut ow, Permissions::SEND_MESSAGES);
            set_overwrite(overwrites, ow);
        }
    }
    let mut departed = Vec::new();
    for &user in lists.keys() {
        let existing = overwrites
            .iter()
            .any(|ow| ow.kind == PermissionOverwriteType::Member(user));
        if !existing && member_if_present(ctx, guild, user).await?.is_none() {
            departed.push(user);
        }
    }
    for user in departed {
        lists.remove(&user);
    }
    let mut edits = apply_lists(overwrites, &lists, &baselines);
    let mut exempt = BTreeSet::from([owner, ctx.cache.current_user().id]);
    exempt.extend(lists.iter().filter_map(|(&user, &access)| {
        (access == Access::Trusted).then_some(user)
    }));
    for user in permits {
        if lists.get(&user) == Some(&Access::Banned) {
            continue;
        }
        let kind = PermissionOverwriteType::Member(user);
        if overwrites.iter().any(|ow| ow.kind == kind) || member_if_present(ctx, guild, user).await?.is_some() {
            grant(overwrites, kind, PERMIT_ACCESS);
            exempt.insert(user);
        }
    }
    let inherited = channel.map(|ch| {
        ch.parent_id
            .and_then(|id| {
                let cached = ctx.cache.guild(guild)?;
                Some(cached.channels.get(&id)?.permission_overwrites.clone())
            })
            .unwrap_or_default()
    });
    grant_room_permit_chat(overwrites, &mut exempt, inherited.as_deref());
    let everyone = PermissionOverwriteType::Role(guild.everyone_role());
    let suppressed = if overwrite(overwrites, everyone).deny.contains(Permissions::SEND_MESSAGES) {
        let staff = staff_roles();
        edits.suppress_role_chat = restrict_roles(overwrites, guild, Permissions::SEND_MESSAGES, &staff);
        grant(
            overwrites,
            PermissionOverwriteType::Member(owner),
            Permissions::SEND_MESSAGES,
        );
        if let Some(cached) = ctx.cache.guild(guild) {
            for role in staff.into_iter().map(RoleId::new) {
                if role != guild.everyone_role() && cached.roles.contains_key(&role) {
                    grant(overwrites, PermissionOverwriteType::Role(role), Permissions::SEND_MESSAGES);
                }
            }
        }
        restrict_member_chat(overwrites, &exempt)
    } else {
        BTreeSet::new()
    };
    edits.suppress_chat = suppressed.difference(&suppressed_chat).copied().collect();
    edits.clear_suppressed_chat = suppressed_chat.difference(&suppressed).copied().collect();
    Ok(edits)
}

pub(super) async fn sync(
    ctx: &Context,
    channel: &GuildChannel,
    owner: UserId,
    mut overwrites: Vec<PermissionOverwrite>,
) -> Result<()> {
    let guild = channel.guild_id;
    let id = channel.id;
    let edits = plan(ctx, guild, owner, Some(channel), &mut overwrites).await?;
    // Save the original bits before editing Discord. Recovery can retry the
    // edit without mistaking the managed permissions for the original ones.
    if !edits.added.is_empty() || !edits.suppress_chat.is_empty() || !edits.suppress_role_chat.is_empty() {
        let conn = db()?;
        let tx = conn.unchecked_transaction()?;
        edits.save_added(&tx, id)?;
        tx.commit()?;
    }
    if channel.permission_overwrites != overwrites {
        id.edit(&ctx.http, EditChannel::new().permissions(overwrites))
            .await?;
    }
    if !edits.removed.is_empty() || !edits.clear_suppressed_chat.is_empty() {
        let conn = db()?;
        let tx = conn.unchecked_transaction()?;
        edits.save_removed(&tx, id)?;
        tx.commit()?;
    }
    edits.disconnect_banned(ctx, guild, id).await
}

pub(super) async fn sync_owner_rooms(ctx: &Context, guild: GuildId, owner: UserId) -> Result<()> {
    let rooms = store::owned_rooms(&*db()?, guild.get(), owner.get())?;
    let mut failed = false;
    for room in rooms {
        let channel = ChannelId::new(room.channel_id);
        if let Err(err) = sync_owned_room(ctx, guild, owner, channel).await {
            tracing::warn!(%channel, %err, "tempvc: could not sync saved room access");
            failed = true;
        }
    }
    if failed {
        return Err(user_error(
            "Your list was saved. Some rooms could not be updated. Try the command again.",
        ));
    }
    Ok(())
}

pub(super) async fn sync_owned_room(
    ctx: &Context,
    guild: GuildId,
    owner: UserId,
    id: ChannelId,
) -> Result<()> {
    let room_lock = room_state(id);
    let mut state = room_lock.lock().await;
    let Some(room) = lookup(guild, id)?.filter(|room| room.owner_id == owner.get()) else {
        return Ok(());
    };
    let Some(channel) = fetch_voice(ctx, guild, id).await? else {
        return forget_room(ctx, room, &mut state);
    };
    sync(ctx, &channel, owner, channel.permission_overwrites.clone()).await
}

pub(super) fn record_allow(channel: ChannelId, user: UserId, bits: Permissions) -> Result<()> {
    let conn = db()?;
    let tx = conn.unchecked_transaction()?;
    if bits.contains(PERMIT_ACCESS) {
        store::set_room_permit(&tx, channel.get(), user.get(), true)?;
    }
    if let Some((_, mut baseline)) = store::room_access(&tx, channel.get())?
        .into_iter()
        .find(|(id, _)| *id == user.get())
    {
        baseline.tracks_chat |= bits.contains(Permissions::SEND_MESSAGES);
        let bits = (bits & PERMIT_ACCESS).bits();
        baseline.allow |= bits;
        baseline.deny &= !bits;
        store::set_room_access(&tx, channel.get(), user.get(), Some(baseline))?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn forget_permit(channel: ChannelId, user: UserId) -> Result<()> {
    store::set_room_permit(&*db()?, channel.get(), user.get(), false)?;
    Ok(())
}

pub(super) fn forget_suppressed_chat(channel: ChannelId, user: UserId) -> Result<()> {
    store::set_member_chat_suppressed(&*db()?, channel.get(), user.get(), false)?;
    Ok(())
}

pub(super) async fn disconnect_if_present(
    ctx: &Context,
    guild: GuildId,
    channel: ChannelId,
    user: UserId,
) -> Result<()> {
    if voice(ctx, guild, user) == Some(channel) {
        guild.disconnect_member(&ctx.http, user).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_overwrites_grant_chat_to_trusted_members_and_restore_only_managed_bits() {
        let trusted = UserId::new(1);
        let banned = UserId::new(2);
        let trust_kind = PermissionOverwriteType::Member(trusted);
        let ban_kind = PermissionOverwriteType::Member(banned);
        let inherited = PermissionOverwrite {
            allow: Permissions::SPEAK,
            deny: Permissions::CONNECT,
            kind: trust_kind,
        };
        let mut overwrites = vec![inherited.clone()];
        let lists = Lists::from([(trusted, Access::Trusted), (banned, Access::Banned)]);
        let edits = apply_lists(&mut overwrites, &lists, &Baselines::new());
        assert_eq!(edits.banned, vec![banned]);
        assert_eq!(
            overwrite(&overwrites, trust_kind).allow,
            inherited.allow | PERMIT_ACCESS
        );
        assert!(overwrite(&overwrites, trust_kind).deny.is_empty());
        assert_eq!(overwrite(&overwrites, ban_kind).deny, PERMIT_ACCESS);

        let switched = Lists::from([(trusted, Access::Banned)]);
        let next = apply_lists(&mut overwrites, &switched, &edits.added);
        assert_eq!(next.banned, vec![trusted]);
        assert!(next.added.is_empty());
        assert_eq!(next.removed, vec![banned]);
        assert_eq!(overwrite(&overwrites, trust_kind).deny, PERMIT_ACCESS);
        assert_eq!(overwrites.len(), 1);

        let baselines = Baselines::from([(trusted, edits.added[&trusted])]);
        apply_lists(&mut overwrites, &Lists::new(), &baselines);
        assert_eq!(overwrites, vec![inherited]);
    }

    #[test]
    fn old_access_baseline_keeps_existing_chat_permissions() {
        let user = UserId::new(1);
        let kind = PermissionOverwriteType::Member(user);
        let mut overwrites = vec![PermissionOverwrite {
            allow: ENTRY_BITS,
            deny: Permissions::SEND_MESSAGES,
            kind,
        }];
        let legacy = Baselines::from([(
            user,
            RoomAccessBaseline {
                allow: 0,
                deny: 0,
                tracks_chat: false,
            },
        )]);
        let lists = Lists::from([(user, Access::Trusted)]);
        let edits = apply_lists(&mut overwrites, &lists, &legacy);
        assert_eq!(overwrite(&overwrites, kind).allow, PERMIT_ACCESS);
        assert!(edits.added[&user].tracks_chat);

        apply_lists(&mut overwrites, &Lists::new(), &edits.added);
        let restored = overwrite(&overwrites, kind);
        assert!(restored.allow.is_empty());
        assert_eq!(restored.deny, Permissions::SEND_MESSAGES);
    }

    #[test]
    fn chat_off_keeps_owner_staff_permit_and_trust_grants() {
        let guild = GuildId::new(1);
        let everyone = PermissionOverwriteType::Role(guild.everyone_role());
        let staff = PermissionOverwriteType::Role(RoleId::new(2));
        let ordinary = PermissionOverwriteType::Role(RoleId::new(3));
        let owner = UserId::new(4);
        let permitted = UserId::new(5);
        let trusted = UserId::new(6);
        let inherited_member = UserId::new(7);
        let denied_member = UserId::new(8);
        let member = PermissionOverwriteType::Member;
        let chat = Permissions::SEND_MESSAGES;
        let read = Permissions::VIEW_CHANNEL | Permissions::READ_MESSAGE_HISTORY;
        let mut overwrites = Vec::new();
        grant(&mut overwrites, everyone, read | chat);
        grant(&mut overwrites, staff, chat);
        grant(&mut overwrites, ordinary, chat);
        grant(&mut overwrites, member(owner), PERMIT_ACCESS);
        grant(&mut overwrites, member(permitted), ENTRY_BITS);
        grant(&mut overwrites, member(inherited_member), PERMIT_ACCESS);
        grant(&mut overwrites, member(denied_member), ENTRY_BITS);
        let mut denied = overwrite(&overwrites, member(denied_member));
        deny_bits(&mut denied, chat);
        set_overwrite(&mut overwrites, denied);
        let inherited = vec![overwrite(&overwrites, member(inherited_member))];

        let lists = Lists::from([(trusted, Access::Trusted)]);
        apply_lists(&mut overwrites, &lists, &Baselines::new());
        let mut exempt = BTreeSet::from([owner, trusted]);
        grant_room_permit_chat(&mut overwrites, &mut exempt, Some(&inherited));
        restrict_roles(&mut overwrites, guild, chat, &[2]);
        let suppressed = restrict_member_chat(&mut overwrites, &exempt);

        assert_eq!(overwrite(&overwrites, everyone).allow, read);
        assert_eq!(overwrite(&overwrites, everyone).deny, chat);
        assert!(!overwrite(&overwrites, ordinary).allow.contains(chat));
        for kind in [staff, member(owner), member(permitted), member(trusted)] {
            assert!(overwrite(&overwrites, kind).allow.contains(chat));
        }
        assert_eq!(suppressed, BTreeSet::from([inherited_member]));
        assert_eq!(overwrite(&overwrites, member(inherited_member)).allow, ENTRY_BITS);
        assert_eq!(overwrite(&overwrites, member(denied_member)).deny, chat);
    }
}
