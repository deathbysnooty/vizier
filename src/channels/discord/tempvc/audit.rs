//! Best-effort Discord audit messages containing only action names and IDs.

use serenity::all::{CreateAllowedMentions, CreateMessage};

use super::*;

#[derive(Clone, Copy)]
pub(super) enum Action {
    Created,
    Deleted,
    StaffDelete,
    Kick,
    Ban,
    Deny,
    Unban,
    Drag,
    Claim,
    StaffClaim,
    Transfer,
    StaffTransfer,
    Handoff,
    Rename,
    Lock(bool),
    Hide(bool),
    KnockApproved,
    HubDenied,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Created => "Room created",
            Self::Deleted => "Room deleted",
            Self::StaffDelete => "Staff force-deleted room",
            Self::Kick => "Member kicked from the room",
            Self::Ban => "Member banned",
            Self::Deny => "Member denied room access",
            Self::Unban => "Member unbanned",
            Self::Drag => "Staff moved member",
            Self::Claim => "Room claimed",
            Self::StaffClaim => "Staff claimed room",
            Self::Transfer => "Room transferred",
            Self::StaffTransfer => "Staff force-transferred room",
            Self::Handoff => "Room handed off automatically",
            Self::Rename => "Room renamed",
            Self::Lock(true) => "Room locked",
            Self::Lock(false) => "Room unlocked",
            Self::Hide(true) => "Room hidden",
            Self::Hide(false) => "Room shown",
            Self::KnockApproved => "Knock approved",
            Self::HubDenied => "Hub join blocked (required role)",
        }
    }
}

fn line(
    action: Action,
    actor: Option<UserId>,
    channel: Option<ChannelId>,
    target: Option<UserId>,
) -> String {
    let mut line = action.label().to_owned();
    if let Some(channel) = channel {
        line.push_str(&format!(" · channel `{channel}`"));
    }
    if let Some(actor) = actor {
        line.push_str(&format!(" · by `{actor}`"));
    }
    if let Some(target) = target {
        line.push_str(&format!(" · member `{target}`"));
    }
    line
}

pub(super) fn post(
    ctx: &Context,
    guild: GuildId,
    action: Action,
    actor: Option<UserId>,
    channel: Option<ChannelId>,
    target: Option<UserId>,
) {
    let Some(destination) = control::id("VIZIER_TEMPVC_LOG_CHANNEL")
        .filter(|id| *id != 0)
        .map(ChannelId::new)
    else {
        return;
    };
    if !ctx.cache.guild(guild).is_some_and(|guild| {
        guild
            .channels
            .get(&destination)
            .is_some_and(|channel| matches!(channel.kind, ChannelType::Text | ChannelType::News))
    }) {
        return;
    }
    let message = CreateMessage::new()
        .content(line(action, actor, channel, target))
        .allowed_mentions(CreateAllowedMentions::new());
    let http = ctx.http.clone();
    tokio::spawn(async move {
        if let Err(err) = destination.send_message(&http, message).await {
            tracing::warn!(%destination, %err, "tempvc: could not post audit log");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_uses_one_line_and_plain_ids() {
        let text = line(
            Action::StaffTransfer,
            Some(UserId::new(2)),
            Some(ChannelId::new(3)),
            Some(UserId::new(4)),
        );
        assert_eq!(
            text,
            "Staff force-transferred room · channel `3` · by `2` · member `4`"
        );
        assert!(!text.contains(['@', '<', '\n']));
        let json = serde_json::to_value(CreateAllowedMentions::new()).unwrap();
        assert_eq!(json["parse"], serde_json::json!([]));
    }
}
