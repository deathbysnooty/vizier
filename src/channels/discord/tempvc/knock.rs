//! Short-lived requests to join locked temporary rooms.

use std::sync::atomic::AtomicU64;

use serenity::all::{CreateAllowedMentions, CreateMessage, MessageId};

use super::actions::{
    Actor, actor, allow_bits, check_enabled, controls, member_if_present, overwrite,
    room_restricted, write_overwrite,
};
use super::*;
use crate::channels::discord::tempvc_panel::Reply;

const KNOCK_COOLDOWN: Duration = Duration::from_secs(60);
const KNOCK_LIFETIME: Duration = Duration::from_secs(10 * 60);
const HIDDEN: &str = "Room hidden hai. Owner se `/vcinvite` ya `/vc permit` maang.";
const BANNED: &str = "Tu is room ki ban list mein hai.";
const EXPIRED: &str = "Ye knock ab active nahi hai.";

static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Knock {
    user: UserId,
    message: MessageId,
    posted: Instant,
}

impl Knock {
    fn active(self, now: Instant) -> bool {
        now.saturating_duration_since(self.posted) < KNOCK_LIFETIME
    }
}

#[derive(Default)]
pub(super) struct Knocks {
    recent: HashMap<UserId, Instant>,
    pending: HashMap<u64, Knock>,
}

impl Knocks {
    fn cooling_down(&self, user: UserId, now: Instant) -> bool {
        self.recent
            .get(&user)
            .is_some_and(|&posted| now.saturating_duration_since(posted) < KNOCK_COOLDOWN)
    }

    fn record(&mut self, id: u64, knock: Knock) {
        let now = knock.posted;
        self.recent
            .retain(|_, posted| now.saturating_duration_since(*posted) < KNOCK_COOLDOWN);
        self.pending.retain(|_, request| request.active(now));
        self.recent.insert(knock.user, now);
        self.pending.insert(id, knock);
    }

    fn answerable(
        &self,
        actor: Actor,
        owner: UserId,
        id: u64,
        message: MessageId,
        now: Instant,
    ) -> Result<Knock> {
        if !controls(actor, owner) {
            return Err(user_error("Sirf room owner ya staff jawab de sakte hain."));
        }
        self.pending
            .get(&id)
            .copied()
            .filter(|request| request.message == message && request.active(now))
            .ok_or_else(|| user_error(EXPIRED))
    }
}

fn request_allowed(locked: bool, hidden: bool, banned: bool) -> Result<()> {
    if banned {
        Err(user_error(BANNED))
    } else if hidden {
        Err(user_error(HIDDEN))
    } else if !locked {
        Err(user_error("Room unlocked hai. Seedha join kar le."))
    } else {
        Ok(())
    }
}

fn managed_room(guild: GuildId, channel: ChannelId) -> Result<Room> {
    lookup(guild, channel)?.ok_or_else(|| user_error("Ye Loduchand ka temp VC nahi hai."))
}

fn banned(room: Room, user: UserId) -> Result<bool> {
    Ok(store::is_banned(
        &*db()?,
        room.guild_id,
        room.owner_id,
        user.get(),
    )?)
}

fn visible_in_room(ctx: &Context, channel: &GuildChannel, member: &Member) -> Result<bool> {
    Ok(!room_restricted(channel, Permissions::VIEW_CHANNEL)
        && ctx
            .cache
            .guild(channel.guild_id)
            .context("guild roles unavailable")?
            .user_permissions_in(channel, member)
            .view_channel())
}

async fn target_voice(
    ctx: &Context,
    guild: GuildId,
    channel: ChannelId,
    state: &mut RoomState,
) -> Result<GuildChannel> {
    if let Some(voice) = fetch_voice(ctx, guild, channel).await? {
        return Ok(voice);
    }
    if let Some(room) = lookup(guild, channel)? {
        forget_room(ctx, room, state)?;
    }
    Err(user_error("Ye temp VC ab nahi hai."))
}

pub(super) async fn request(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    channel: ChannelId,
) -> Result<Reply> {
    managed_room(guild, channel)?;
    let lock = room_state(channel);
    let mut state = lock.lock().await;
    check_enabled()?;
    let room = managed_room(guild, channel)?;
    let user = member.user.id;
    let voice = target_voice(ctx, guild, channel, &mut state).await?;
    request_allowed(
        room_restricted(&voice, Permissions::CONNECT),
        !visible_in_room(ctx, &voice, member)?,
        banned(room, user)?,
    )?;
    if state.knocks.cooling_down(user, Instant::now()) {
        return Err(user_error("Abhi knock kiya tha. Owner ko ek minute de."));
    }

    let owner = UserId::new(room.owner_id);
    let id = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
    let message = channel
        .send_message(
            &ctx.http,
            CreateMessage::new()
                .content(format!(
                    "🚪 <@{owner}>, <@{user}> room join karna chahta hai."
                ))
                .allowed_mentions(CreateAllowedMentions::new().users([owner]))
                .components(tempvc_panel::knock_rows(id)),
        )
        .await?;
    state.knocks.record(
        id,
        Knock {
            user,
            message: message.id,
            posted: Instant::now(),
        },
    );
    Ok(Reply::Text(format!(
        "🚪 <#{channel}> ke chat mein knock bhej diya."
    )))
}

pub(in super::super) async fn answer_knock(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    prompt: (ChannelId, MessageId),
    request_id: u64,
    approved: bool,
) -> Result<String> {
    let (channel, message) = prompt;
    managed_room(guild, channel)?;
    let lock = room_state(channel);
    let mut state = lock.lock().await;
    check_enabled()?;
    let room = managed_room(guild, channel)?;
    let actor = actor(ctx, guild, member)?;
    let request = state.knocks.answerable(
        actor,
        UserId::new(room.owner_id),
        request_id,
        message,
        Instant::now(),
    )?;
    let user = request.user;
    if !approved {
        state.knocks.pending.remove(&request_id);
        return Ok(format!("⛔ <@{user}> ka knock deny kar diya."));
    }

    let voice_channel = target_voice(ctx, guild, channel, &mut state).await?;
    if banned(room, user)? {
        return Err(user_error(BANNED));
    }
    let requester = member_if_present(ctx, guild, user)
        .await?
        .ok_or_else(|| user_error("Ye member server mein nahi hai."))?;
    if !visible_in_room(ctx, &voice_channel, &requester)? {
        return Err(user_error(HIDDEN));
    }
    let before = overwrite(
        &voice_channel.permission_overwrites,
        PermissionOverwriteType::Member(user),
    );
    let mut permit = before.clone();
    allow_bits(&mut permit, Permissions::CONNECT);
    if permit != before {
        write_overwrite(ctx, channel, &permit).await?;
    }
    access::record_allow(channel, user, Permissions::CONNECT)?;
    state.knocks.pending.remove(&request_id);
    audit::post(
        ctx,
        guild,
        audit::Action::KnockApproved,
        Some(actor.id),
        Some(channel),
        Some(user),
    );

    let mut result = format!("✅ <@{user}> ka knock approve kar diya.");
    // Move Members lets the bot admit an approved user even at the room limit.
    if voice(ctx, guild, user).is_some_and(|source| source != channel) {
        match guild.move_member(&ctx.http, user, channel).await {
            Ok(_) => result.push_str(" Room mein move kar diya."),
            Err(err) => {
                tracing::warn!(%channel, %user, %err, "tempvc: could not move approved knocker");
                result.push_str(" Move nahi ho paaya. Ab khud join kar sakta hai.");
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: UserId = UserId::new(1);
    const GUEST: UserId = UserId::new(2);
    const MESSAGE: MessageId = MessageId::new(3);
    const ID: u64 = 4;

    fn knock_at(now: Instant) -> Knock {
        Knock {
            user: GUEST,
            message: MESSAGE,
            posted: now,
        }
    }

    #[test]
    fn only_a_visible_locked_room_allows_knocking_and_bans_take_priority() {
        assert!(request_allowed(true, false, false).is_ok());
        assert!(request_allowed(false, false, false).is_err());
        assert_eq!(
            request_allowed(true, true, false).unwrap_err().to_string(),
            HIDDEN
        );
        assert_eq!(
            request_allowed(true, false, true).unwrap_err().to_string(),
            BANNED
        );
    }

    #[test]
    fn cooldown_is_per_knocker_and_room_and_ends_at_one_minute() {
        let now = Instant::now();
        let mut knocks = Knocks::default();
        knocks.record(ID, knock_at(now));
        assert!(knocks.cooling_down(GUEST, now + Duration::from_secs(59)));
        assert!(!knocks.cooling_down(GUEST, now + KNOCK_COOLDOWN));
        assert!(!knocks.cooling_down(OWNER, now));
        assert!(!Knocks::default().cooling_down(GUEST, now));
    }

    #[test]
    fn only_the_current_owner_or_staff_can_answer_the_bound_message_until_expiry() {
        let now = Instant::now();
        let mut knocks = Knocks::default();
        knocks.record(ID, knock_at(now));
        let owner = Actor {
            id: OWNER,
            staff: false,
            voice: None,
        };
        let guest = Actor { id: GUEST, ..owner };
        let staff = Actor {
            id: UserId::new(5),
            staff: true,
            ..owner
        };

        assert!(knocks.answerable(owner, OWNER, ID, MESSAGE, now).is_ok());
        assert!(knocks.answerable(staff, OWNER, ID, MESSAGE, now).is_ok());
        assert!(knocks.answerable(guest, OWNER, ID, MESSAGE, now).is_err());
        assert!(knocks.answerable(owner, GUEST, ID, MESSAGE, now).is_err());
        assert!(
            knocks
                .answerable(owner, OWNER, ID, MessageId::new(6), now)
                .is_err()
        );
        assert!(
            knocks
                .answerable(owner, OWNER, ID, MESSAGE, now + KNOCK_LIFETIME)
                .is_err()
        );
    }
}
