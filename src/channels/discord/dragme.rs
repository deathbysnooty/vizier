use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType,
    CommandType, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateCommand, CreateCommandOption, CreateInteractionResponse, CreateInteractionResponseFollowup,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, EditMessage, GuildId,
    Http, InteractionContext, Mentionable, MessageId, UserId, VoiceState,
};
use tokio::time::{Instant, sleep_until};

use crate::error::VizierError;

pub const COMMAND: &str = "dragme";
pub const INVITE_COMMAND: &str = "Invite to my VC";
pub const INVITE_SLASH_COMMAND: &str = "vcinvite";

const DRAG_PREFIX: &str = "dragme:";
const INVITE_PREFIX: &str = "vcinv:";
const EXPIRED: &str = "⌛ expired";
const INVITER_LEFT: &str = "⚠️ Invite cancel ho gaya. Invite bhejne wala VC se nikal gaya.";
const REQUESTER_LEFT: &str = "⚠️ Request bhejne wala VC se nikal gaya.";
const JOIN_VOICE_FIRST: &str = "Pehle kisi bhi VC mein aa ja, phir Join dabana";
const DRAG_NOT_ALLOWED: &str = "Sirf us VC ke log approve kar sakte hain.";
const INVITE_NOT_ALLOWED: &str = "Sirf jisko invite bheja hai, woh jawab de sakta hai.";
const MOVE_FAILED: &str = "⚠️ Move nahi ho paaya.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Drag,
    Invite,
}

impl Kind {
    fn lifetime(self) -> Duration {
        Duration::from_secs(match self {
            Self::Drag => 10 * 60,
            Self::Invite => 30 * 60,
        })
    }
}

#[derive(Clone, Copy)]
enum Action {
    Accept,
    Decline,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Close(&'static str),
    Move(UserId),
    Decline,
}

#[derive(Clone, Copy)]
struct Request {
    kind: Kind,
    guild: GuildId,
    initiator: UserId,
    member: UserId,
    target: ChannelId,
    message_channel: ChannelId,
    message: Option<MessageId>,
    expires_at: Instant,
}

impl Request {
    fn expired(self, now: Instant) -> bool {
        now >= self.expires_at
    }

    fn content(self) -> String {
        let initiator = self.initiator.mention();
        let member = self.member.mention();
        match self.kind {
            Kind::Drag => format!("🎙️ {initiator} {member} ke VC mein aana chahta hai. Andar wale batao?"),
            Kind::Invite => format!("🎙️ {member}, {initiator} ne tujhe {} mein bulaya hai!", self.target.mention()),
        }
    }

    fn buttons(self, id: u64) -> CreateActionRow {
        let (prefix, accept, decline) = match self.kind {
            Kind::Drag => (DRAG_PREFIX, "Approve", "Deny"),
            Kind::Invite => (INVITE_PREFIX, "Join", "No thanks"),
        };
        CreateActionRow::Buttons(vec![
            CreateButton::new(format!("{prefix}{id}:yes")).label(accept).style(ButtonStyle::Success),
            CreateButton::new(format!("{prefix}{id}:no")).label(decline).style(ButtonStyle::Secondary),
        ])
    }

    fn decide(
        self,
        action: Action,
        presser: UserId,
        presser_voice: Option<ChannelId>,
        initiator_voice: Option<ChannelId>,
    ) -> Result<Decision, &'static str> {
        match self.kind {
            Kind::Drag if presser_voice != Some(self.target) => return Err(DRAG_NOT_ALLOWED),
            Kind::Invite if presser != self.member => return Err(INVITE_NOT_ALLOWED),
            Kind::Invite if initiator_voice != Some(self.target) => return Ok(Decision::Close(INVITER_LEFT)),
            _ => {}
        }
        match (self.kind, action) {
            (_, Action::Decline) => Ok(Decision::Decline),
            (Kind::Drag, Action::Accept) if initiator_voice.is_none() => Ok(Decision::Close(REQUESTER_LEFT)),
            (Kind::Drag, Action::Accept) => Ok(Decision::Move(self.initiator)),
            (Kind::Invite, Action::Accept) if presser_voice.is_none() => Err(JOIN_VOICE_FIRST),
            (Kind::Invite, Action::Accept) => Ok(Decision::Move(self.member)),
        }
    }
}

#[derive(Default)]
struct Pending {
    requests: HashMap<u64, Request>,
}

impl Pending {
    fn insert(&mut self, id: u64, request: Request, now: Instant) -> bool {
        if request.kind == Kind::Drag && self.requests.values().any(|other| {
            other.kind == Kind::Drag
                && other.guild == request.guild
                && other.initiator == request.initiator
                && other.target == request.target
                && (other.message.is_none() || !other.expired(now))
        }) {
            return false;
        }
        self.requests.insert(id, request);
        true
    }

    fn claim(&mut self, id: u64, now: Instant) -> Option<Request> {
        self.requests.get(&id).filter(|request| !request.expired(now))?;
        self.requests.remove(&id)
    }

    fn expire(&mut self, id: u64, now: Instant) -> Option<Request> {
        self.requests.get(&id).filter(|request| request.expired(now))?;
        self.requests.remove(&id)
    }

    fn inviter_left(&mut self, guild: GuildId, inviter: UserId, old_channel: ChannelId) -> Vec<Request> {
        let mut canceled = Vec::new();
        self.requests.retain(|_, request| {
            let void = request.kind == Kind::Invite
                && request.guild == guild
                && request.initiator == inviter
                && request.target == old_channel;
            if void {
                canceled.push(*request);
            }
            !void
        });
        canceled
    }
}

static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(|| Mutex::new(Pending::default()));

pub fn command() -> CreateCommand {
    CreateCommand::new(COMMAND)
        .description("Ask to be moved into someone's voice channel")
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Someone in that VC").required(true))
        .contexts(vec![InteractionContext::Guild])
}

pub fn invite_command() -> CreateCommand {
    CreateCommand::new(INVITE_COMMAND)
        .kind(CommandType::User)
        .contexts(vec![InteractionContext::Guild])
}

pub fn invite_slash_command() -> CreateCommand {
    CreateCommand::new(INVITE_SLASH_COMMAND)
        .description("Invite someone into your voice channel")
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "Who to invite").required(true))
        .contexts(vec![InteractionContext::Guild])
}

pub fn owns_component(id: &str) -> bool {
    id.starts_with(DRAG_PREFIX) || id.starts_with(INVITE_PREFIX)
}

fn button(id: &str) -> Option<(Kind, u64, Action)> {
    let (kind, rest) = match id.strip_prefix(DRAG_PREFIX) {
        Some(rest) => (Kind::Drag, rest),
        None => (Kind::Invite, id.strip_prefix(INVITE_PREFIX)?),
    };
    let (id, answer) = rest.split_once(':')?;
    let action = match answer {
        "yes" => Action::Accept,
        "no" => Action::Decline,
        _ => return None,
    };
    Some((kind, id.parse().ok()?, action))
}

fn voice_channels(ctx: &Context, guild: GuildId, first: UserId, second: UserId) -> (Option<ChannelId>, Option<ChannelId>) {
    let Some(guild) = ctx.cache.guild(guild) else { return (None, None) };
    let channel = |user| guild.voice_states.get(&user).and_then(|state| state.channel_id);
    (channel(first), channel(second))
}

fn request_from_command(ctx: &Context, command: &CommandInteraction) -> Result<Request, &'static str> {
    let guild = command.guild_id.ok_or("Ye sirf server mein chalta hai.")?;
    let initiator = command.user.id;
    let kind = if command.data.name == COMMAND { Kind::Drag } else { Kind::Invite };
    let member = if command.data.name == INVITE_COMMAND {
        command.data.target_id.map(|id| id.to_user_id())
    } else {
        command.data.options.iter().find_map(|option| match option.value {
            CommandDataOptionValue::User(id) if option.name == "member" => Some(id),
            _ => None,
        })
    }.ok_or("Kisi member ko select kar.")?;
    let (own_voice, member_voice) = voice_channels(ctx, guild, initiator, member);
    let (target, message_channel) = match kind {
        Kind::Drag => {
            let own = own_voice.ok_or("Pehle kisi bhi VC mein aa ja, phir /dragme kar.")?;
            let target = member_voice.ok_or("Woh abhi kisi VC mein nahi hai.")?;
            if own == target {
                return Err("Tu already usi VC mein hai!");
            }
            (target, target)
        }
        Kind::Invite => (
            own_voice.ok_or("Pehle kisi bhi VC mein aa ja, phir invite bhej.")?,
            command.channel_id,
        ),
    };
    Ok(Request {
        kind,
        guild,
        initiator,
        member,
        target,
        message_channel,
        message: None,
        expires_at: Instant::now() + kind.lifetime(),
    })
}

pub async fn on_command(ctx: &Context, command: &CommandInteraction) {
    if let Err(err) = command.defer_ephemeral(&ctx.http).await {
        tracing::warn!("voice request: could not defer command: {err}");
        return;
    }
    let reply = match request_from_command(ctx, command) {
        Ok(request) => match post_request(ctx, command.id.get(), request).await {
            Ok(reply) => reply,
            Err(err) => {
                tracing::warn!("{err}");
                "Message nahi bhej paaya. Thodi der mein try kar."
            }
        },
        Err(reply) => reply,
    };
    if let Err(err) = command.edit_response(&ctx.http, EditInteractionResponse::new().content(reply)).await {
        tracing::warn!("voice request: could not confirm command: {err}");
    }
}

async fn post_request(ctx: &Context, id: u64, mut request: Request) -> crate::Result<&'static str> {
    // Reserve the request before sending so two commands cannot both post it.
    if !PENDING.lock().insert(id, request, Instant::now()) {
        return Ok("Is VC ki request pehle se pending hai.");
    }
    let builder = CreateMessage::new()
        .content(request.content())
        .allowed_mentions(CreateAllowedMentions::new().users([request.member]))
        .components(vec![request.buttons(id)]);
    let message = match request.message_channel.send_message(&ctx.http, builder).await {
        Ok(message) => message,
        Err(err) => {
            PENDING.lock().requests.remove(&id);
            return Err(VizierError(format!("voice request: could not post message: {err}")));
        }
    };
    request.message = Some(message.id);
    request.expires_at = Instant::now() + request.kind.lifetime();
    let posted = {
        let mut pending = PENDING.lock();
        pending.requests.get_mut(&id).map(|entry| *entry = request).is_some()
    };
    if !posted {
        // A voice-state update may cancel an invite while the message is being sent.
        close(&ctx.http, request, INVITER_LEFT).await;
        return Ok(INVITER_LEFT);
    }

    let http = ctx.http.clone();
    tokio::spawn(async move {
        sleep_until(request.expires_at).await;
        let expired = PENDING.lock().expire(id, Instant::now());
        if let Some(expired) = expired {
            close(&http, expired, EXPIRED).await;
        }
    });
    Ok(match request.kind {
        Kind::Drag => "Us VC ke chat mein request bhej di. Ab unke jawab ka wait kar.",
        Kind::Invite => "Invite bhej diya!",
    })
}

pub async fn on_voice_state(ctx: &Context, old: Option<&VoiceState>, new: &VoiceState) {
    let (Some(guild), Some(old_channel)) = (new.guild_id, old.and_then(|state| state.channel_id)) else { return };
    if new.channel_id == Some(old_channel) {
        return;
    }
    let canceled = PENDING.lock().inviter_left(guild, new.user_id, old_channel);
    for request in canceled {
        let outcome = if request.expired(Instant::now()) { EXPIRED } else { INVITER_LEFT };
        close(&ctx.http, request, outcome).await;
    }
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let Some((kind, id, action)) = button(&component.data.custom_id) else {
        reply(ctx, component, EXPIRED).await;
        return;
    };
    let request = PENDING.lock().requests.get(&id).copied().filter(|request| {
        request.kind == kind
            && Some(request.guild) == component.guild_id
            && request.message_channel == component.channel_id
            && !request.expired(Instant::now())
    });
    let Some(request) = request else {
        reply(ctx, component, EXPIRED).await;
        return;
    };
    match request.message {
        None => {
            reply(ctx, component, "Ek second ruk, phir button dabana.").await;
            return;
        }
        Some(id) if id != component.message.id => {
            reply(ctx, component, EXPIRED).await;
            return;
        }
        Some(_) => {}
    }

    let presser = component.user.id;
    let (presser_voice, initiator_voice) = voice_channels(ctx, request.guild, presser, request.initiator);
    let decision = match request.decide(action, presser, presser_voice, initiator_voice) {
        Ok(decision) => decision,
        Err(text) => {
            reply(ctx, component, text).await;
            return;
        }
    };
    // Claim before any Discord I/O. A second click or the timer cannot decide this request.
    let claimed = PENDING.lock().claim(id, Instant::now());
    let Some(request) = claimed else {
        reply(ctx, component, EXPIRED).await;
        return;
    };
    let actor = presser.mention();
    let outcome = match decision {
        Decision::Move(user) => {
            let source = match kind {
                Kind::Drag => initiator_voice,
                Kind::Invite => presser_voice,
            };
            let move_member = async {
                if source != Some(request.target) {
                    request.guild.move_member(&ctx.http, user, request.target).await?;
                }
                Ok(())
            };
            // Start the move without waiting on the acknowledgment's HTTP round trip.
            let ((), moved) = tokio::join!(acknowledge(ctx, component), move_member);
            if let Err(err) = moved {
                let outcome = report_move_failure(ctx, component, request, &err).await;
                close(&ctx.http, request, outcome).await;
                return;
            }
            match kind {
                Kind::Drag => format!("✅ approved by {actor}"),
                Kind::Invite => format!("✅ {actor} joined"),
            }
        }
        Decision::Decline => {
            acknowledge(ctx, component).await;
            match kind {
                Kind::Drag => format!("❌ denied by {actor}"),
                Kind::Invite => format!("❌ declined by {actor}"),
            }
        }
        Decision::Close(text) => {
            reply(ctx, component, text).await;
            text.to_owned()
        }
    };
    close(&ctx.http, request, &outcome).await;
}

async fn acknowledge(ctx: &Context, component: &ComponentInteraction) {
    if let Err(err) = component.defer(&ctx.http).await {
        tracing::warn!("voice request: could not acknowledge button: {err}");
    }
}

async fn report_move_failure(
    ctx: &Context,
    component: &ComponentInteraction,
    request: Request,
    err: &serenity::Error,
) -> &'static str {
    let text = match err {
        serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(response))
            if response.status_code.as_u16() == 403 || response.error.code == 50013 =>
        {
            "Loduchand ko Move Members permission chahiye."
        }
        _ if request.kind == Kind::Drag
            && voice_channels(ctx, request.guild, request.initiator, request.member).0.is_none() =>
        {
            REQUESTER_LEFT
        }
        _ => MOVE_FAILED,
    };
    tracing::warn!("voice request: could not move member: {err}");
    let note = CreateInteractionResponseFollowup::new().ephemeral(true).content(text);
    if let Err(err) = component.create_followup(&ctx.http, note).await {
        tracing::warn!("voice request: could not report move failure: {err}");
    }
    text
}

async fn reply(ctx: &Context, component: &ComponentInteraction, text: &str) {
    let message = CreateInteractionResponseMessage::new().ephemeral(true).content(text);
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::warn!("voice request: could not answer button: {err}");
    }
}

async fn close(http: &Http, request: Request, outcome: &str) {
    let Some(message) = request.message else { return };
    let edit = EditMessage::new()
        .content(format!("{}\n\n{outcome}", request.content()))
        .allowed_mentions(CreateAllowedMentions::new())
        .components(vec![]);
    if let Err(err) = request.message_channel.edit_message(http, message, edit).await {
        tracing::warn!("voice request: could not close message: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: Kind, now: Instant) -> Request {
        Request {
            kind,
            guild: GuildId::new(1),
            initiator: UserId::new(2),
            member: UserId::new(3),
            target: ChannelId::new(4),
            message_channel: ChannelId::new(5),
            message: Some(MessageId::new(6)),
            expires_at: now + kind.lifetime(),
        }
    }

    #[test]
    fn voice_invites_have_slash_and_user_menu_commands() {
        let slash = serde_json::to_value(invite_slash_command()).unwrap();
        assert_eq!(slash["name"], "vcinvite");
        assert_eq!(slash["description"], "Invite someone into your voice channel");
        let options = slash["options"].as_array().unwrap();
        assert_eq!(options.len(), 1);
        assert_eq!(options[0]["name"], "member");
        assert_eq!(options[0]["type"], serde_json::to_value(CommandOptionType::User).unwrap());
        assert_eq!(options[0]["required"], true);

        let menu = serde_json::to_value(invite_command()).unwrap();
        assert_eq!(menu["name"], "Invite to my VC");
        assert_eq!(menu["type"], serde_json::to_value(CommandType::User).unwrap());
        let guild_only = serde_json::to_value([InteractionContext::Guild]).unwrap();
        assert_eq!(slash["contexts"], guild_only);
        assert_eq!(menu["contexts"], guild_only);
    }

    #[test]
    fn any_current_target_vc_member_can_decide_a_drag_request() {
        let request = request(Kind::Drag, Instant::now());
        let other_member = UserId::new(7);
        let target = Some(request.target);
        let source = Some(ChannelId::new(8));

        assert_eq!(
            request.decide(Action::Accept, other_member, target, source),
            Ok(Decision::Move(request.initiator)),
        );
        assert_eq!(request.decide(Action::Decline, other_member, target, source), Ok(Decision::Decline));
        assert_eq!(request.decide(Action::Accept, request.member, source, source), Err(DRAG_NOT_ALLOWED));
        assert_eq!(request.decide(Action::Decline, other_member, None, source), Err(DRAG_NOT_ALLOWED));
        assert_eq!(request.decide(Action::Accept, other_member, target, None), Ok(Decision::Close(REQUESTER_LEFT)));
    }

    #[test]
    fn invites_require_the_recipient_and_check_voice_membership() {
        let request = request(Kind::Invite, Instant::now());
        let target = Some(request.target);
        let other_vc = Some(ChannelId::new(8));

        assert_eq!(request.decide(Action::Decline, UserId::new(7), target, target), Err(INVITE_NOT_ALLOWED));
        assert_eq!(request.decide(Action::Accept, request.member, None, target), Err(JOIN_VOICE_FIRST));
        assert_eq!(request.decide(Action::Decline, request.member, None, target), Ok(Decision::Decline));
        assert_eq!(request.decide(Action::Accept, request.member, other_vc, target), Ok(Decision::Move(request.member)));
        assert_eq!(request.decide(Action::Accept, request.member, other_vc, None), Ok(Decision::Close(INVITER_LEFT)));
        assert_eq!(request.decide(Action::Decline, request.member, target, other_vc), Ok(Decision::Close(INVITER_LEFT)));
    }

    #[test]
    fn a_request_has_one_winner_and_expires_at_its_deadline() {
        let now = Instant::now();
        let mut pending = Pending::default();
        for (kind, seconds) in [(Kind::Drag, 600), (Kind::Invite, 1800)] {
            let request = request(kind, now);
            let deadline = now + Duration::from_secs(seconds);
            let just_before = deadline - Duration::from_millis(1);
            assert!(pending.insert(1, request, now));
            assert!(pending.expire(1, just_before).is_none());
            assert!(pending.claim(1, just_before).is_some());
            assert!(pending.claim(1, just_before).is_none());
            assert!(pending.expire(1, deadline).is_none());

            assert!(pending.insert(2, request, now));
            assert!(pending.claim(2, deadline).is_none());
            assert!(pending.expire(2, deadline).is_some());
            assert!(pending.expire(2, deadline).is_none());
        }
    }

    #[test]
    fn drag_duplicates_are_scoped_to_the_requester_and_destination() {
        let now = Instant::now();
        let mut pending = Pending::default();
        let request = request(Kind::Drag, now);
        assert!(pending.insert(1, request, now));
        assert!(!pending.insert(2, Request { member: UserId::new(7), ..request }, now));
        assert!(pending.insert(3, Request { initiator: UserId::new(7), ..request }, now));
        assert!(pending.insert(4, Request { target: ChannelId::new(8), ..request }, now));
        let replacement = Request { expires_at: request.expires_at + Kind::Drag.lifetime(), ..request };
        assert!(pending.insert(5, replacement, request.expires_at));
        assert!(pending.expire(1, request.expires_at).is_some());
        assert!(pending.claim(5, request.expires_at).is_some());
    }

    #[test]
    fn leaving_the_destination_voids_the_inviters_pending_invite() {
        let now = Instant::now();
        let mut pending = Pending::default();
        let invite = request(Kind::Invite, now);
        assert!(pending.insert(1, invite, now));
        assert!(pending.insert(2, Request { initiator: UserId::new(7), ..invite }, now));
        assert!(pending.insert(3, request(Kind::Drag, now), now));

        assert!(pending.inviter_left(invite.guild, invite.initiator, ChannelId::new(8)).is_empty());
        assert_eq!(pending.inviter_left(invite.guild, invite.initiator, invite.target).len(), 1);
        assert!(pending.claim(1, now).is_none());
        assert!(pending.claim(2, now).is_some());
        assert!(pending.claim(3, now).is_some());
    }
}
