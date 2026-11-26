//! Persistent room access layered over inherited and room-specific permissions.

use std::collections::BTreeMap;

use serenity::all::{EditChannel, PermissionOverwrite};

use super::actions::{allow_bits, deny_bits, member_if_present, overwrite, set_overwrite};
use super::*;
use store::Access;

type Baselines = BTreeMap<UserId, (u64, u64)>;
type Lists = BTreeMap<UserId, Access>;
const ACCESS_BITS: Permissions = Permissions::VIEW_CHANNEL.union(Permissions::CONNECT);

#[derive(Default)]
pub(super) struct AccessEdits {
    added: Baselines,
    removed: Vec<UserId>,
    banned: Vec<UserId>,
}

impl AccessEdits {
    pub(super) fn save_added(&self, conn: &rusqlite::Connection, channel: ChannelId) -> Result<()> {
        for (user, baseline) in &self.added {
            store::set_room_access(conn, channel.get(), user.get(), Some(*baseline))?;
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
    for (&user, &(allow, deny)) in baselines {
        if !lists.contains_key(&user) {
            let mut restored = overwrite(overwrites, PermissionOverwriteType::Member(user));
            restored.allow =
                (restored.allow - ACCESS_BITS) | Permissions::from_bits_truncate(allow);
            restored.deny = (restored.deny - ACCESS_BITS) | Permissions::from_bits_truncate(deny);
            set_overwrite(overwrites, restored);
            edits.removed.push(user);
        }
    }
    for (&user, &access) in lists {
        let mut ow = overwrite(overwrites, PermissionOverwriteType::Member(user));
        if !baselines.contains_key(&user) {
            edits.added.insert(
                user,
                (
                    (ow.allow & ACCESS_BITS).bits(),
                    (ow.deny & ACCESS_BITS).bits(),
                ),
            );
        }
        match access {
            Access::Trusted => allow_bits(&mut ow, ACCESS_BITS),
            Access::Banned => {
                deny_bits(&mut ow, ACCESS_BITS);
                edits.banned.push(user);
            }
        }
        set_overwrite(overwrites, ow);
    }
    edits
}

pub(super) async fn plan(
    ctx: &Context,
    guild: GuildId,
    owner: UserId,
    channel: Option<ChannelId>,
    overwrites: &mut Vec<PermissionOverwrite>,
) -> Result<AccessEdits> {
    let (mut lists, baselines) = {
        let conn = db()?;
        let mut lists = Lists::new();
        for access in [Access::Trusted, Access::Banned] {
            for user in store::access_list(&conn, guild.get(), owner.get(), access)? {
                lists.insert(UserId::new(user), access);
            }
        }
        let baselines = channel
            .map(|id| store::room_access(&conn, id.get()))
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|(id, allow, deny)| (UserId::new(id), (allow, deny)))
            .collect();
        (lists, baselines)
    };
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
    Ok(apply_lists(overwrites, &lists, &baselines))
}

pub(super) async fn sync(
    ctx: &Context,
    channel: &GuildChannel,
    owner: UserId,
    mut overwrites: Vec<PermissionOverwrite>,
) -> Result<()> {
    let guild = channel.guild_id;
    let id = channel.id;
    let edits = plan(ctx, guild, owner, Some(id), &mut overwrites).await?;
    // Save the original bits before editing Discord. Recovery can retry the
    // edit without mistaking the managed permissions for the original ones.
    if !edits.added.is_empty() {
        let conn = db()?;
        let tx = conn.unchecked_transaction()?;
        edits.save_added(&tx, id)?;
        tx.commit()?;
    }
    if channel.permission_overwrites != overwrites {
        id.edit(&ctx.http, EditChannel::new().permissions(overwrites))
            .await?;
    }
    if !edits.removed.is_empty() {
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
            "List save ho gayi. Kuch rooms update nahi hue. Cmd phir se try kar.",
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
    if let Some((_, allow, deny)) = store::room_access(&conn, channel.get())?
        .into_iter()
        .find(|(id, _, _)| *id == user.get())
    {
        let bits = (bits & ACCESS_BITS).bits();
        store::set_room_access(
            &conn,
            channel.get(),
            user.get(),
            Some((allow | bits, deny & !bits)),
        )?;
    }
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
    fn list_overwrites_apply_both_bits_and_restore_only_managed_bits() {
        let trusted = UserId::new(1);
        let banned = UserId::new(2);
        let trust_kind = PermissionOverwriteType::Member(trusted);
        let ban_kind = PermissionOverwriteType::Member(banned);
        let inherited = PermissionOverwrite {
            allow: Permissions::SEND_MESSAGES,
            deny: Permissions::CONNECT,
            kind: trust_kind,
        };
        let mut overwrites = vec![inherited.clone()];
        let lists = Lists::from([(trusted, Access::Trusted), (banned, Access::Banned)]);
        let edits = apply_lists(&mut overwrites, &lists, &Baselines::new());
        assert_eq!(edits.banned, vec![banned]);
        assert_eq!(
            overwrite(&overwrites, trust_kind).allow,
            inherited.allow | ACCESS_BITS
        );
        assert!(overwrite(&overwrites, trust_kind).deny.is_empty());
        assert_eq!(overwrite(&overwrites, ban_kind).deny, ACCESS_BITS);

        let switched = Lists::from([(trusted, Access::Banned)]);
        let next = apply_lists(&mut overwrites, &switched, &edits.added);
        assert_eq!(next.banned, vec![trusted]);
        assert!(next.added.is_empty());
        assert_eq!(next.removed, vec![banned]);
        assert_eq!(overwrite(&overwrites, trust_kind).deny, ACCESS_BITS);
        assert_eq!(overwrites.len(), 1);

        let baselines = Baselines::from([(trusted, edits.added[&trusted])]);
        apply_lists(&mut overwrites, &Lists::new(), &baselines);
        assert_eq!(overwrites, vec![inherited]);
    }
}
