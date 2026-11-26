//! Discord interactions for temporary voice rooms. The room service owns all authorization.

use serenity::all::{
    ActionRowComponent, ButtonStyle, ChannelId, ChannelType, CommandDataOption,
    CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction,
    ComponentInteractionDataKind, Context, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateCommand, CreateCommandOption, CreateEmbed, CreateInputText, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateModal, CreateSelectMenu,
    CreateSelectMenuKind, EditInteractionResponse, EditMessage, GuildChannel, GuildId,
    InputTextStyle, InteractionContext, Member, ModalInteraction, PermissionOverwriteType,
    Permissions, UserId,
};

use super::tempvc::{self, UserError};
use super::tempvc_store::Profile;

pub(super) const COMMAND: &str = "vc";
pub(super) const PREFIX: &str = "tvc:";
const FIELD: &str = "value";
const KNOCK_PREFIX: &str = "knock:";

#[derive(Clone, Copy)]
pub(super) enum RoomTarget {
    Voice(Option<ChannelId>),
    Panel(ChannelId),
}

pub(super) enum Action {
    Create {
        hub: Option<ChannelId>,
        name: Option<String>,
        limit: Option<u32>,
        locked: Option<bool>,
    },
    Lock(Option<bool>),
    Hide(Option<bool>),
    Chat(Option<bool>),
    Soundboard(Option<bool>),
    Rename(String),
    Limit(u32),
    Bitrate(u32),
    Region(Option<String>),
    Permit(UserId),
    Deny(UserId),
    Kick(UserId),
    Disconnect(UserId),
    Reset(UserId),
    Claim,
    Transfer(UserId),
    Delete,
    Panel,
    Info,
    Drag(UserId),
    Knock(ChannelId),
    Me(MeAction),
}

pub(super) enum MeAction {
    Save,
    Set(Profile),
    Profile,
    Clear,
    Trust(UserId),
    Untrust(UserId),
    Ban(UserId),
    Unban(UserId),
}

pub(super) enum Reply {
    Text(String),
    Info(Box<CreateEmbed>),
}

impl Reply {
    pub(super) fn info(embed: CreateEmbed) -> Self {
        Self::Info(Box::new(embed))
    }
}

pub(super) fn command() -> CreateCommand {
    use CommandOptionType::{Boolean, Channel, Integer, String, SubCommand, SubCommandGroup, User};

    let channel = || {
        CreateCommandOption::new(Channel, "channel", "Temp room to manage (staff only)")
            .channel_types(vec![ChannelType::Voice])
    };
    let sub = |name, description| CreateCommandOption::new(SubCommand, name, description);
    let room = |name, description| sub(name, description).add_sub_option(channel());
    let with = |name, description, option: CreateCommandOption| {
        sub(name, description)
            .add_sub_option(option.required(true))
            .add_sub_option(channel())
    };
    let user = |description| CreateCommandOption::new(User, "user", description);
    let name = || {
        CreateCommandOption::new(String, "name", "Room name (supports {user} and {count})")
            .min_length(1)
            .max_length(100)
    };
    let limit = |key| {
        CreateCommandOption::new(Integer, key, "User limit (0 means unlimited)")
            .min_int_value(0)
            .max_int_value(99)
    };
    let bitrate = |key| {
        CreateCommandOption::new(Integer, key, "Bitrate in kbps, up to the server cap")
            .min_int_value(8)
            .max_int_value(384)
    };
    let state = |description| {
        CreateCommandOption::new(String, "state", description)
            .add_string_choice("on", "on")
            .add_string_choice("off", "off")
    };
    let list_action = |name, description| {
        sub(name, description).add_sub_option(user("Which member").required(true))
    };
    let me = CreateCommandOption::new(SubCommandGroup, "me", "Manage your room defaults and lists")
        .add_sub_option(sub("save", "Save settings from your current room"))
        .add_sub_option(
            sub("set", "Edit your saved defaults")
                .add_sub_option(name())
                .add_sub_option(limit("limit"))
                .add_sub_option(CreateCommandOption::new(Boolean, "locked", "Start locked"))
                .add_sub_option(CreateCommandOption::new(Boolean, "hidden", "Start hidden"))
                .add_sub_option(bitrate("bitrate")),
        )
        .add_sub_option(sub(
            "profile",
            "See your defaults, trusted friends and bans",
        ))
        .add_sub_option(sub("clear", "Clear your saved defaults"))
        .add_sub_option(list_action(
            "trust",
            "Let a friend always see and join your rooms",
        ))
        .add_sub_option(list_action(
            "untrust",
            "Remove a member from your trusted list",
        ))
        .add_sub_option(list_action("ban", "Block a member from your rooms"))
        .add_sub_option(list_action("unban", "Remove a member from your ban list"));

    CreateCommand::new(COMMAND)
        .description("Manage temporary voice rooms")
        .contexts(vec![InteractionContext::Guild])
        .set_options(vec![
            sub("create", "Create your own temp room")
                .add_sub_option(name())
                .add_sub_option(limit("limit"))
                .add_sub_option(CreateCommandOption::new(Boolean, "locked", "Start locked"))
                .add_sub_option(
                    CreateCommandOption::new(Channel, "hub", "Configured hub to create from")
                        .channel_types(vec![ChannelType::Voice]),
                ),
            with("lock", "Lock or unlock the room", state("Lock the room")),
            with("hide", "Hide or show the room", state("Hide the room")),
            with("rename", "Rename the room", name()),
            with("limit", "Set the user limit", limit("count")),
            with("bitrate", "Set the audio bitrate", bitrate("kbps")),
            with(
                "region",
                "Set the voice region",
                CreateCommandOption::new(String, "region", "Region ID, e.g. singapore or auto")
                    .min_length(1)
                    .max_length(32),
            ),
            with(
                "chat",
                "Turn room chat on or off",
                CreateCommandOption::new(Boolean, "enabled", "Allow messages in the room"),
            ),
            with(
                "soundboard",
                "Turn the soundboard on or off",
                CreateCommandOption::new(Boolean, "enabled", "Allow soundboard in the room"),
            ),
            with("permit", "Let a member see and join", user("Who to allow")),
            with("deny", "Block a member from joining", user("Who to block")),
            with(
                "reset",
                "Clear a member's room overrides",
                user("Whose permissions to clear"),
            ),
            with(
                "kick",
                "Disconnect a member and ban them from the owner's rooms",
                user("Who to kick"),
            ),
            with(
                "disconnect",
                "Disconnect a member from the room",
                user("Who to disconnect"),
            ),
            room("claim", "Claim a room after its owner leaves"),
            with(
                "transfer",
                "Give the room to someone else",
                user("New room owner"),
            ),
            room("delete", "Delete the room"),
            room("panel", "Repost the control panel in the room"),
            room("info", "Show room details"),
            with(
                "drag",
                "Staff: pull a member into the room",
                user("Who to move from voice"),
            ),
            sub("knock", "Ask to join a locked temp room").add_sub_option(
                CreateCommandOption::new(Channel, "channel", "Locked temp room to join")
                    .channel_types(vec![ChannelType::Voice])
                    .required(true),
            ),
            me,
        ])
}

fn option_value<'a>(
    options: &'a [CommandDataOption],
    name: &str,
) -> Option<&'a CommandDataOptionValue> {
    options
        .iter()
        .find(|option| option.name == name)
        .map(|option| &option.value)
}

fn user_option(options: &[CommandDataOption]) -> Result<UserId, UserError> {
    option_value(options, "user")
        .and_then(CommandDataOptionValue::as_user_id)
        .ok_or_else(|| UserError("Kisi member ko select kar.".into()))
}

fn me_request(options: &[CommandDataOption]) -> anyhow::Result<MeAction> {
    let [sub] = options else {
        return Err(UserError("Ek `/vc me` action select kar.".into()).into());
    };
    let CommandDataOptionValue::SubCommand(options) = &sub.value else {
        return Err(UserError("Ek `/vc me` action select kar.".into()).into());
    };
    let user = || user_option(options);
    Ok(match sub.name.as_str() {
        "save" => MeAction::Save,
        "set" => {
            let int = |name| {
                option_value(options, name)
                    .and_then(CommandDataOptionValue::as_i64)
                    .and_then(|value| u32::try_from(value).ok())
            };
            let flag = |name| option_value(options, name).and_then(CommandDataOptionValue::as_bool);
            let name = option_value(options, "name")
                .and_then(CommandDataOptionValue::as_str)
                .map(str::trim);
            if name == Some("") {
                return Err(UserError("Room ka naam khaali nahi ho sakta.".into()).into());
            }
            MeAction::Set(Profile {
                name: name.map(str::to_owned),
                limit: int("limit"),
                locked: flag("locked"),
                hidden: flag("hidden"),
                bitrate: int("bitrate").and_then(|kbps| kbps.checked_mul(1000)),
            })
        }
        "profile" => MeAction::Profile,
        "clear" => MeAction::Clear,
        "trust" => MeAction::Trust(user()?),
        "untrust" => MeAction::Untrust(user()?),
        "ban" => MeAction::Ban(user()?),
        "unban" => MeAction::Unban(user()?),
        _ => return Err(UserError("Ye `/vc me` action ab available nahi hai.".into()).into()),
    })
}

fn slash_request(options: &[CommandDataOption]) -> anyhow::Result<(RoomTarget, Action)> {
    let [sub] = options else {
        return Err(UserError("Ek `/vc` action select kar.".into()).into());
    };
    if sub.name == "me"
        && let CommandDataOptionValue::SubCommandGroup(options) = &sub.value
    {
        return Ok((RoomTarget::Voice(None), Action::Me(me_request(options)?)));
    }
    let CommandDataOptionValue::SubCommand(options) = &sub.value else {
        return Err(UserError("Ek `/vc` action select kar.".into()).into());
    };
    let value = |name| option_value(options, name);
    let channel = |name| value(name).and_then(CommandDataOptionValue::as_channel_id);
    let int = |name| {
        value(name)
            .and_then(CommandDataOptionValue::as_i64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let string = |name| {
        value(name)
            .and_then(CommandDataOptionValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let flag = |name| value(name).and_then(CommandDataOptionValue::as_bool);
    let user = || user_option(options);
    let required = |name| UserError(format!("`{name}` bhi bhar de."));
    let state = || match value("state").and_then(CommandDataOptionValue::as_str) {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err(UserError("`state` mein `on` ya `off` select kar.".into())),
    };
    let action = match sub.name.as_str() {
        "create" => Action::Create {
            hub: channel("hub"),
            name: string("name"),
            limit: int("limit"),
            locked: flag("locked"),
        },
        "lock" => Action::Lock(Some(state()?)),
        "hide" => Action::Hide(Some(state()?)),
        "rename" => Action::Rename(string("name").ok_or_else(|| required("name"))?),
        "limit" => Action::Limit(int("count").ok_or_else(|| required("count"))?),
        "bitrate" => Action::Bitrate(int("kbps").ok_or_else(|| required("kbps"))?),
        "chat" => Action::Chat(Some(flag("enabled").ok_or_else(|| required("enabled"))?)),
        "soundboard" => {
            Action::Soundboard(Some(flag("enabled").ok_or_else(|| required("enabled"))?))
        }
        "region" => Action::Region(
            string("region")
                .ok_or_else(|| required("region"))
                .map(|region| region.to_ascii_lowercase())
                .map(|region| (region != "auto").then_some(region))?,
        ),
        "permit" => Action::Permit(user()?),
        "deny" => Action::Deny(user()?),
        "reset" => Action::Reset(user()?),
        "kick" => Action::Kick(user()?),
        "disconnect" => Action::Disconnect(user()?),
        "claim" => Action::Claim,
        "transfer" => Action::Transfer(user()?),
        "delete" => Action::Delete,
        "panel" => Action::Panel,
        "info" => Action::Info,
        "drag" => Action::Drag(user()?),
        "knock" => Action::Knock(channel("channel").ok_or_else(|| required("channel"))?),
        _ => return Err(UserError("Ye `/vc` action ab available nahi hai.".into()).into()),
    };
    Ok((RoomTarget::Voice(channel("channel")), action))
}

fn actor(guild_id: Option<GuildId>, member: Option<&Member>) -> anyhow::Result<(GuildId, &Member)> {
    guild_id
        .zip(member)
        .ok_or_else(|| UserError("Ye sirf server mein chalta hai.".into()).into())
}

pub(super) async fn on_command(ctx: &Context, command: &CommandInteraction) {
    if let Err(err) = command.defer_ephemeral(&ctx.http).await {
        tracing::warn!(%err, "temp voice: could not defer command");
        return;
    }
    let result = async {
        let (guild, member) = actor(command.guild_id, command.member.as_deref())?;
        let (target, action) = slash_request(&command.data.options)?;
        tempvc::execute(ctx, guild, member, target, action).await
    }
    .await;
    if let Err(err) = command.edit_response(&ctx.http, edit_reply(result)).await {
        tracing::warn!(%err, "temp voice: could not respond to command");
    }
}

fn edit_reply(result: anyhow::Result<Reply>) -> EditInteractionResponse {
    let edit = EditInteractionResponse::new().allowed_mentions(CreateAllowedMentions::new());
    match result {
        Ok(Reply::Text(text)) => edit.content(text),
        Ok(Reply::Info(embed)) => edit.embed(*embed),
        Err(err) => edit.content(error_text(err)),
    }
}

fn error_text(err: anyhow::Error) -> String {
    if let Some(err) = err.downcast_ref::<UserError>() {
        return err.0.clone();
    }
    tracing::warn!(%err, "temp voice: action failed");
    "Ye action nahi ho paaya. Thodi der mein try kar.".into()
}

fn button(action: &str, label: &str, style: ButtonStyle) -> CreateButton {
    CreateButton::new(format!("{PREFIX}{action}"))
        .label(label)
        .style(style)
}

pub(super) fn knock_rows(request_id: u64) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        button(
            &format!("{KNOCK_PREFIX}approve:{request_id}"),
            "Approve",
            ButtonStyle::Success,
        ),
        button(
            &format!("{KNOCK_PREFIX}deny:{request_id}"),
            "Deny",
            ButtonStyle::Danger,
        ),
    ])]
}

fn panel_rows() -> Vec<CreateActionRow> {
    use ButtonStyle::{Danger, Primary, Secondary};

    vec![
        CreateActionRow::Buttons(vec![
            button("lock", "Lock / Unlock", Secondary),
            button("hide", "Hide / Unhide", Secondary),
            button("chat", "Chat on / off", Secondary),
            button("soundboard", "Soundboard on / off", Secondary),
        ]),
        CreateActionRow::Buttons(vec![
            button("rename", "Rename", Secondary),
            button("limit", "Limit", Secondary),
            button("bitrate", "Bitrate", Secondary),
        ]),
        CreateActionRow::Buttons(vec![
            button("kick", "Kick", Secondary),
            button("permit", "Permit", Secondary),
            button("transfer", "Transfer", Secondary),
            button("claim", "Claim", Primary),
            button("delete", "Delete", Danger),
        ]),
    ]
}

pub(super) async fn post_panel(ctx: &Context, channel: ChannelId) -> anyhow::Result<()> {
    let embed = CreateEmbed::new()
        .title("🎛️ Temp voice controls")
        .description("Owner aur staff room manage kar sakte hain. Owner chala gaya? Voice mein rehkar **Claim** daba.");
    channel
        .send_message(
            &ctx.http,
            CreateMessage::new().embed(embed).components(panel_rows()),
        )
        .await?;
    Ok(())
}

pub(super) fn info_embed(channel: &GuildChannel, owner: UserId, members: &[UserId]) -> CreateEmbed {
    let everyone = PermissionOverwriteType::Role(channel.guild_id.everyone_role());
    let deny = channel
        .permission_overwrites
        .iter()
        .find(|overwrite| overwrite.kind == everyone)
        .map_or_else(Permissions::empty, |overwrite| overwrite.deny);
    let yes_no = |yes| if yes { "Yes" } else { "No" };
    let on_off = |yes| if yes { "On" } else { "Off" };
    let limit = match channel.user_limit.unwrap_or_default() {
        0 => "Unlimited".into(),
        limit => limit.to_string(),
    };
    let bitrate = channel
        .bitrate
        .map_or_else(|| "—".into(), |bps| format!("{} kbps", bps / 1000));
    let occupancy = match members.len() {
        0 => "Abhi room khaali hai.".into(),
        count => format!("Voice mein abhi {count}"),
    };
    CreateEmbed::new()
        .title(&channel.name)
        .description(occupancy)
        .field("Owner", format!("<@{owner}>"), true)
        .field("Limit", limit, true)
        .field("Bitrate", bitrate, true)
        .field(
            "Region",
            channel.rtc_region.as_deref().unwrap_or("Automatic"),
            true,
        )
        .field("Locked", yes_no(deny.contains(Permissions::CONNECT)), true)
        .field(
            "Hidden",
            yes_no(deny.contains(Permissions::VIEW_CHANNEL)),
            true,
        )
        .field(
            "Chat",
            on_off(!deny.contains(Permissions::SEND_MESSAGES)),
            true,
        )
        .field(
            "Soundboard",
            on_off(!deny.contains(Permissions::USE_SOUNDBOARD)),
            true,
        )
}

fn ephemeral(text: impl Into<String>) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new()
        .content(text)
        .ephemeral(true)
        .allowed_mentions(CreateAllowedMentions::new())
}

fn modal(action: &str, title: &str, label: &str, max_length: u16) -> CreateInteractionResponse {
    CreateInteractionResponse::Modal(
        CreateModal::new(format!("{PREFIX}{action}"), title).components(vec![
            CreateActionRow::InputText(
                CreateInputText::new(InputTextStyle::Short, label, FIELD)
                    .max_length(max_length)
                    .required(true),
            ),
        ]),
    )
}

fn member_picker(action: &str, prompt: &str) -> CreateInteractionResponse {
    let picker = CreateSelectMenu::new(
        format!("{PREFIX}pick:{action}"),
        CreateSelectMenuKind::User {
            default_users: None,
        },
    )
    .placeholder("Member select kar")
    .min_values(1)
    .max_values(1);
    CreateInteractionResponse::Message(
        ephemeral(prompt).components(vec![CreateActionRow::SelectMenu(picker)]),
    )
}

fn panel_form(action: &str) -> Option<CreateInteractionResponse> {
    Some(match action {
        "rename" => modal("rename", "Room rename", "Naya naam", 100),
        "limit" => modal("limit", "Room limit", "Member limit (0 = unlimited)", 2),
        "bitrate" => modal("bitrate", "Room bitrate", "Bitrate in kbps", 3),
        "kick" => member_picker("kick", "Kisko kick karke owner ke rooms se ban karna hai?"),
        "permit" => member_picker("permit", "Kisko entry deni hai?"),
        "transfer" => member_picker("transfer", "Naya owner kaun?"),
        "delete" => CreateInteractionResponse::Message(
            ephemeral("Pakka, sabke liye room delete karna hai?").components(vec![
                CreateActionRow::Buttons(vec![button(
                    "delete:confirm",
                    "Delete room",
                    ButtonStyle::Danger,
                )]),
            ]),
        ),
        _ => return None,
    })
}

fn component_action(id: &str, kind: &ComponentInteractionDataKind) -> anyhow::Result<Action> {
    let picked = || match kind {
        ComponentInteractionDataKind::UserSelect { values } if values.len() == 1 => Ok(values[0]),
        _ => Err(UserError("Ek member select kar.".into())),
    };
    Ok(match id {
        "lock" => Action::Lock(None),
        "hide" => Action::Hide(None),
        "chat" => Action::Chat(None),
        "soundboard" => Action::Soundboard(None),
        "claim" => Action::Claim,
        "delete:confirm" => Action::Delete,
        "pick:kick" => Action::Kick(picked()?),
        "pick:permit" => Action::Permit(picked()?),
        "pick:transfer" => Action::Transfer(picked()?),
        _ => {
            return Err(UserError(
                "Ye button ab available nahi hai. `/vc panel` se naya panel laga.".into(),
            )
            .into());
        }
    })
}

fn knock_action(id: &str) -> Option<(u64, bool)> {
    let (answer, request) = id.split_once(':')?;
    let approved = match answer {
        "approve" => true,
        "deny" => false,
        _ => return None,
    };
    Some((request.parse().ok()?, approved))
}

async fn answer_knock(
    ctx: &Context,
    component: &ComponentInteraction,
    guild: GuildId,
    member: &Member,
    id: &str,
) -> anyhow::Result<Reply> {
    let (request_id, approved) = knock_action(id)
        .ok_or_else(|| UserError("Ye join request ab available nahi hai.".into()))?;
    let message = (component.channel_id, component.message.id);
    let text = tempvc::answer_knock(ctx, guild, member, message, request_id, approved).await?;
    let edit = EditMessage::new()
        .content(&text)
        .components(vec![])
        .allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = message.0.edit_message(&ctx.http, message.1, edit).await {
        tracing::warn!(%err, "temp voice: could not update knock prompt");
    }
    Ok(Reply::Text(text))
}

pub(super) async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let Some(id) = component.data.custom_id.strip_prefix(PREFIX) else {
        return;
    };
    let target = RoomTarget::Panel(component.channel_id);
    if let Some(form) = panel_form(id) {
        let result = match actor(component.guild_id, component.member.as_ref()) {
            Ok((guild, member)) => tempvc::can_control(ctx, guild, member, target),
            Err(err) => Err(err),
        };
        let response = match result {
            Ok(()) => form,
            Err(err) => CreateInteractionResponse::Message(ephemeral(error_text(err))),
        };
        if let Err(err) = component.create_response(&ctx.http, response).await {
            tracing::warn!(%err, "temp voice: could not open panel control");
        }
        return;
    }

    if let Err(err) = component.defer_ephemeral(&ctx.http).await {
        tracing::warn!(%err, "temp voice: could not defer panel action");
        return;
    }
    let result = async {
        let (guild, member) = actor(component.guild_id, component.member.as_ref())?;
        if let Some(request) = id.strip_prefix(KNOCK_PREFIX) {
            return answer_knock(ctx, component, guild, member, request).await;
        }
        let action = component_action(id, &component.data.kind)?;
        tempvc::execute(ctx, guild, member, target, action).await
    }
    .await;
    if let Err(err) = component.edit_response(&ctx.http, edit_reply(result)).await {
        tracing::warn!(%err, "temp voice: could not respond to panel action");
    }
}

fn modal_action(modal: &ModalInteraction) -> anyhow::Result<Action> {
    let value = modal
        .data
        .components
        .iter()
        .flat_map(|row| &row.components)
        .find_map(|field| match field {
            ActionRowComponent::InputText(input) if input.custom_id == FIELD => {
                input.value.as_deref()
            }
            _ => None,
        })
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| UserError("Kuch toh bhar de.".into()))?;
    let number = || {
        value
            .parse()
            .map_err(|_| UserError("Poora number daal.".into()))
    };
    match modal.data.custom_id.strip_prefix(PREFIX) {
        Some("rename") => Ok(Action::Rename(value.into())),
        Some("limit") => Ok(Action::Limit(number()?)),
        Some("bitrate") => Ok(Action::Bitrate(number()?)),
        _ => Err(UserError("Ye form ab available nahi hai.".into()).into()),
    }
}

pub(super) async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    if let Err(err) = modal.defer_ephemeral(&ctx.http).await {
        tracing::warn!(%err, "temp voice: could not defer room form");
        return;
    }
    let result = async {
        let (guild, member) = actor(modal.guild_id, modal.member.as_ref())?;
        let target = RoomTarget::Panel(modal.channel_id);
        tempvc::execute(ctx, guild, member, target, modal_action(modal)?).await
    }
    .await;
    if let Err(err) = modal.edit_response(&ctx.http, edit_reply(result)).await {
        tracing::warn!(%err, "temp voice: could not respond to room form");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn option_named<'a>(options: &'a [Value], name: &str) -> &'a Value {
        options
            .iter()
            .find(|option| option["name"] == name)
            .unwrap()
    }

    #[test]
    fn vc_is_guild_only_and_leaves_room_for_three_subcommand_groups() {
        let serialized = serde_json::to_value(command()).unwrap();
        assert_eq!(serialized["name"], "vc");
        assert_eq!(
            serialized["contexts"],
            serde_json::to_value([InteractionContext::Guild]).unwrap()
        );
        let options = serialized["options"].as_array().unwrap();
        assert!(options.len() <= 25 - 3);
        let names: Vec<_> = options
            .iter()
            .map(|option| option["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "create",
                "lock",
                "hide",
                "rename",
                "limit",
                "bitrate",
                "region",
                "chat",
                "soundboard",
                "permit",
                "deny",
                "reset",
                "kick",
                "disconnect",
                "claim",
                "transfer",
                "delete",
                "panel",
                "info",
                "drag",
                "knock",
                "me",
            ]
        );
        for option in options
            .iter()
            .filter(|option| !matches!(option["name"].as_str(), Some("create" | "knock" | "me")))
        {
            let children = option["options"].as_array().unwrap();
            let channel = children.last().unwrap();
            assert_eq!(channel["name"], "channel");
            assert_eq!(channel["required"], false);
        }

        for action in ["lock", "hide"] {
            let children = option_named(options, action)["options"].as_array().unwrap();
            let state = option_named(children, "state");
            assert_eq!(state["required"], true);
            assert_eq!(
                state["choices"],
                json!([{"name": "on", "value": "on"}, {"name": "off", "value": "off"}])
            );
        }
        let knock_channel = &option_named(options, "knock")["options"][0];
        assert_eq!(knock_channel["name"], "channel");
        assert_eq!(knock_channel["required"], true);
        assert_eq!(knock_channel["channel_types"], json!([ChannelType::Voice]));

        let me = option_named(options, "me");
        assert_eq!(me["type"], json!(CommandOptionType::SubCommandGroup));
        let children = me["options"].as_array().unwrap();
        let names: Vec<_> = children
            .iter()
            .map(|child| child["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "save", "set", "profile", "clear", "trust", "untrust", "ban", "unban"
            ]
        );
        let settings = option_named(children, "set")["options"].as_array().unwrap();
        assert_eq!(settings.len(), 5);
        for name in ["name", "limit", "locked", "hidden", "bitrate"] {
            assert_eq!(option_named(settings, name)["required"], false);
        }
        for action in ["trust", "untrust", "ban", "unban"] {
            let user = &option_named(children, action)["options"][0];
            assert_eq!(user["name"], "user");
            assert_eq!(user["required"], true);
        }
    }

    #[test]
    fn slash_request_preserves_explicit_off_and_zero_and_parses_knock_channel() {
        let options = |parsed| serde_json::from_value::<Vec<CommandDataOption>>(parsed).unwrap();
        let profile = options(json!([{
            "name": "me", "type": CommandOptionType::SubCommandGroup, "options": [{
                "name": "set", "type": CommandOptionType::SubCommand, "options": [
                    {"name": "name", "type": CommandOptionType::String, "value": " {user} {count} "},
                    {"name": "limit", "type": CommandOptionType::Integer, "value": 0},
                    {"name": "locked", "type": CommandOptionType::Boolean, "value": false},
                    {"name": "bitrate", "type": CommandOptionType::Integer, "value": 64}
                ]
            }]
        }]));
        let (_, Action::Me(MeAction::Set(profile))) = slash_request(&profile).unwrap() else {
            panic!("expected profile settings");
        };
        assert_eq!(profile.name.as_deref(), Some("{user} {count}"));
        assert_eq!(profile.limit, Some(0));
        assert_eq!(profile.locked, Some(false));
        assert_eq!(profile.hidden, None);
        assert_eq!(profile.bitrate, Some(64_000));

        let lock = options(json!([{
            "name": "lock", "type": CommandOptionType::SubCommand, "options": [
                {"name": "state", "type": CommandOptionType::String, "value": "off"}
            ]
        }]));
        assert!(matches!(
            slash_request(&lock),
            Ok((_, Action::Lock(Some(false))))
        ));

        let knock = options(json!([{
            "name": "knock", "type": CommandOptionType::SubCommand, "options": [
                {"name": "channel", "type": CommandOptionType::Channel, "value": "5"}
            ]
        }]));
        assert!(matches!(slash_request(&knock), Ok((_, Action::Knock(id))) if id.get() == 5));
    }

    #[test]
    fn knock_buttons_route_to_the_same_request() {
        let rows = serde_json::to_value(knock_rows(42)).unwrap();
        let buttons = rows[0]["components"].as_array().unwrap();
        assert_eq!(buttons.len(), 2);
        for (index, approved) in [true, false].into_iter().enumerate() {
            let id = buttons[index]["custom_id"]
                .as_str()
                .unwrap()
                .strip_prefix(PREFIX)
                .unwrap()
                .strip_prefix(KNOCK_PREFIX)
                .unwrap();
            assert_eq!(knock_action(id), Some((42, approved)));
        }
        assert_eq!(knock_action("approve:nope"), None);
    }

    #[test]
    fn all_room_controls_have_routable_persistent_ids() {
        let rows = serde_json::to_value(panel_rows()).unwrap();
        let rows = rows.as_array().unwrap();
        assert!(rows.len() <= 5);
        for row in rows {
            let buttons = row["components"].as_array().unwrap();
            assert!(buttons.len() <= 5);
            for button in buttons {
                let id = button["custom_id"]
                    .as_str()
                    .unwrap()
                    .strip_prefix(PREFIX)
                    .unwrap();
                assert!(
                    panel_form(id).is_some()
                        || component_action(id, &ComponentInteractionDataKind::Button).is_ok()
                );
            }
        }
    }
}
