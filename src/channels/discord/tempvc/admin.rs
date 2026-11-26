//! Staff commands and the private room manager.

use anyhow::{Context as _, Result};
use serenity::all::{
    ButtonStyle, ChannelId, ChannelType, CommandDataOption, CommandDataOptionValue,
    CommandOptionType, ComponentInteraction, ComponentInteractionDataKind, Context,
    CreateActionRow, CreateAllowedMentions, CreateButton, CreateCommandOption, CreateEmbed,
    CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage,
    CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse,
    GuildId, Member, Permissions, RoleId, UserId,
};

use super::super::tempvc_panel::{
    self, Action, ForceAction, Reply, RoomTarget, error_text, option_value,
};
use super::store::{HubTemplate, RolePerk};
use super::{actions, control, db, lookup, store, user_error};

pub(in super::super) const PREFIX: &str = "admin:";
const HUBS_KEY: &str = "VIZIER_TEMPVC_HUBS";
const PAGE_SIZE: usize = 8;

pub(in super::super) fn command() -> CreateCommandOption {
    use CommandOptionType::{Channel, Integer, Role, String, SubCommand, SubCommandGroup};

    let sub = |name, description| CreateCommandOption::new(SubCommand, name, description);
    let role = || CreateCommandOption::new(Role, "role", "Role to configure").required(true);
    let channel = || {
        CreateCommandOption::new(Channel, "channel", "Join-to-create voice channel")
            .channel_types(vec![ChannelType::Voice])
            .required(true)
    };
    let limit = |description| {
        CreateCommandOption::new(Integer, "limit", description)
            .min_int_value(0)
            .max_int_value(99)
    };
    let bitrate = |description| {
        CreateCommandOption::new(Integer, "bitrate", description)
            .min_int_value(8)
            .max_int_value(384)
    };
    CreateCommandOption::new(SubCommandGroup, "admin", "Staff controls for temp voice")
        .add_sub_option(sub("rooms", "List and manage temporary rooms"))
        .add_sub_option(
            sub("perk-set", "Raise room caps for a role")
                .add_sub_option(role())
                .add_sub_option(limit("Maximum member limit (0 = unlimited)").required(true))
                .add_sub_option(bitrate("Maximum bitrate in kbps").required(true)),
        )
        .add_sub_option(sub("perk-remove", "Remove a role's perks").add_sub_option(role()))
        .add_sub_option(sub("perk-list", "List temp voice role perks"))
        .add_sub_option(
            sub(
                "hub-set",
                "Enable a hub and customize its defaults",
            )
            .add_sub_option(channel())
            .add_sub_option(
                CreateCommandOption::new(
                    String,
                    "name_pattern",
                    "Room name with {user} and {count}",
                )
                .min_length(1)
                .max_length(100),
            )
            .add_sub_option(limit("Default room limit (0 = unlimited)"))
            .add_sub_option(bitrate("Default bitrate in kbps"))
            .add_sub_option(
                CreateCommandOption::new(Channel, "category", "Where to create rooms")
                    .channel_types(vec![ChannelType::Category]),
            )
            .add_sub_option(CreateCommandOption::new(
                Role,
                "required_role",
                "Role needed to use this hub",
            )),
        )
        .add_sub_option(
            sub("hub-remove", "Disable a hub and clear its overrides").add_sub_option(channel()),
        )
        .add_sub_option(sub("hub-list", "List hubs and their overrides"))
}

fn staff(ctx: &Context, guild: GuildId, member: &Member) -> Result<UserId> {
    let actor = actions::actor(ctx, guild, member)?;
    if !actor.staff {
        return Err(user_error("Only temp voice staff can use this."));
    }
    Ok(actor.id)
}

fn required(name: &str) -> anyhow::Error {
    user_error(format!("Fill in `{name}`."))
}

fn role(guild: GuildId, role: RoleId) -> Result<u64> {
    if role == guild.everyone_role() {
        return Err(user_error("Choose a member role other than @everyone."));
    }
    Ok(role.get())
}

fn number(
    options: &[CommandDataOption],
    name: &str,
    range: std::ops::RangeInclusive<u32>,
) -> Result<Option<u32>> {
    option_value(options, name)
        .map(|value| {
            value
                .as_i64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|n| range.contains(n))
                .ok_or_else(|| {
                    user_error(format!(
                        "Set `{name}` between {} and {}.",
                        range.start(),
                        range.end()
                    ))
                })
        })
        .transpose()
}

fn hub_template(guild: GuildId, options: &[CommandDataOption]) -> Result<HubTemplate> {
    let value = |name| option_value(options, name);
    let channel = value("channel")
        .and_then(CommandDataOptionValue::as_channel_id)
        .ok_or_else(|| required("channel"))?;
    let name_pattern = value("name_pattern")
        .and_then(CommandDataOptionValue::as_str)
        .map(str::trim);
    if name_pattern.is_some_and(|name| name.is_empty() || name.chars().count() > 100) {
        return Err(user_error("Use 1–100 characters for the room name pattern."));
    }
    Ok(HubTemplate {
        channel_id: channel.get(),
        name_pattern: name_pattern.map(str::to_owned),
        limit: number(options, "limit", 0..=99)?,
        bitrate: number(options, "bitrate", 8..=384)?.map(|kbps| kbps * 1_000),
        category_id: value("category")
            .and_then(CommandDataOptionValue::as_channel_id)
            .map(ChannelId::get),
        required_role_id: value("required_role")
            .and_then(CommandDataOptionValue::as_role_id)
            .map(|id| role(guild, id))
            .transpose()?,
    })
}

fn set_hub(ctx: &Context, guild: GuildId, actor: UserId, template: &HubTemplate) -> Result<String> {
    let channel = ChannelId::new(template.channel_id);
    {
        let cached = ctx
            .cache
            .guild(guild)
            .context("guild channels unavailable")?;
        if !cached
            .channels
            .get(&channel)
            .is_some_and(|ch| ch.kind == ChannelType::Voice)
        {
            return Err(user_error("Choose a voice channel in this server."));
        }
        if let Some(id) = template.category_id
            && !cached
                .channels
                .get(&ChannelId::new(id))
                .is_some_and(|ch| ch.kind == ChannelType::Category)
        {
            return Err(user_error("Choose a category in this server."));
        }
    }
    if lookup(guild, channel)?.is_some() {
        return Err(user_error("Choose a permanent voice channel for the hub."));
    }
    // Save the access template before exposing a newly enabled hub to joins.
    let conn = db()?;
    store::save_hub_template(&conn, guild.get(), template)?;
    if let Err(err) = control::edit_ids(HUBS_KEY, actor.get(), channel.get(), true) {
        tracing::warn!(%channel, %err, "tempvc: hub template saved but hub could not be enabled");
        return Ok(format!(
            "Saved the template for <#{channel}>. Turn on the hub from the panel."
        ));
    }
    Ok(format!(
        "<#{channel}> is ready. Blank fields use the defaults."
    ))
}

fn remove_hub(ctx: &Context, guild: GuildId, actor: UserId, channel: ChannelId) -> Result<String> {
    let exists_in_guild = ctx
        .cache
        .guild(guild)
        .is_some_and(|cached| cached.channels.contains_key(&channel));
    let conn = db()?;
    let template = store::hub_template(&conn, guild.get(), channel.get())?;
    if template.is_none() && !exists_in_guild {
        return Err(user_error("I couldn't find that voice hub in this server."));
    }
    let disabled = control::edit_ids(HUBS_KEY, actor.get(), channel.get(), false)?;
    let removed = store::remove_hub_template(&conn, guild.get(), channel.get())?;
    if disabled || removed {
        Ok(format!("Disabled the hub in <#{channel}>."))
    } else {
        Ok(format!("The hub in <#{channel}> is already off."))
    }
}

pub(in super::super) fn command_reply(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    options: &[CommandDataOption],
) -> Result<EditInteractionResponse> {
    let actor = staff(ctx, guild, member)?;
    let [sub] = options else {
        return Err(user_error("Choose a staff action."));
    };
    let CommandDataOptionValue::SubCommand(options) = &sub.value else {
        return Err(user_error("Choose a staff action."));
    };
    let id = |name| option_value(options, name);
    let perk_role = || {
        role(
            guild,
            id("role")
                .and_then(CommandDataOptionValue::as_role_id)
                .ok_or_else(|| required("role"))?,
        )
    };
    let text = match sub.name.as_str() {
        "rooms" => {
            actions::check_enabled()?;
            return list_reply(ctx, guild, actor, 0, View::Rooms);
        }
        "perk-list" => return list_reply(ctx, guild, actor, 0, View::Perks),
        "hub-list" => return list_reply(ctx, guild, actor, 0, View::Hubs),
        "perk-set" => {
            let role_id = perk_role()?;
            let limit = number(options, "limit", 0..=99)?.ok_or_else(|| required("limit"))?;
            let bitrate =
                number(options, "bitrate", 8..=384)?.ok_or_else(|| required("bitrate"))?;
            let perk = RolePerk {
                role_id,
                limit,
                bitrate: bitrate * 1_000,
            };
            store::save_role_perk(&*db()?, guild.get(), &perk)?;
            format!("Saved the perk for <@&{role_id}>.")
        }
        "perk-remove" => {
            let role_id = perk_role()?;
            if store::remove_role_perk(&*db()?, guild.get(), role_id)? {
                format!("Removed the perk for <@&{role_id}>.")
            } else {
                format!("<@&{role_id}> had no perk to remove.")
            }
        }
        "hub-set" => set_hub(ctx, guild, actor, &hub_template(guild, options)?)?,
        "hub-remove" => remove_hub(
            ctx,
            guild,
            actor,
            id("channel")
                .and_then(CommandDataOptionValue::as_channel_id)
                .ok_or_else(|| required("channel"))?,
        )?,
        _ => return Err(user_error("This staff action is unavailable.")),
    };
    Ok(edit().content(text))
}

fn edit() -> EditInteractionResponse {
    EditInteractionResponse::new().allowed_mentions(CreateAllowedMentions::new())
}

#[derive(Clone, Copy)]
enum View {
    Rooms,
    Hubs,
    Perks,
}

impl View {
    fn key(self) -> &'static str {
        match self {
            Self::Rooms => "rooms",
            Self::Hubs => "hubs",
            Self::Perks => "perks",
        }
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Browse,
    Transfer,
    ConfirmDelete,
}

struct Page {
    index: usize,
    total: usize,
}

impl Page {
    fn get<T>(all: &[T], requested: usize) -> (Self, &[T]) {
        let total = all.len().div_ceil(PAGE_SIZE).max(1);
        let index = requested.min(total - 1);
        let start = index * PAGE_SIZE;
        (
            Self { index, total },
            &all[start..all.len().min(start + PAGE_SIZE)],
        )
    }

    fn label(&self) -> String {
        format!("Page {} / {}", self.index + 1, self.total)
    }
}

fn custom_id(actor: UserId, action: &str, page: usize, channel: Option<ChannelId>) -> String {
    let channel = channel.map_or(0, ChannelId::get);
    format!(
        "{}{PREFIX}{actor}:{action}:{page}:{channel}",
        tempvc_panel::PREFIX
    )
}

fn button(id: String, label: &str, style: ButtonStyle) -> CreateButton {
    CreateButton::new(id).label(label).style(style)
}

fn navigation(actor: UserId, view: View, page: &Page) -> Option<CreateActionRow> {
    if page.total <= 1 {
        return None;
    }
    let make = |target, label| {
        button(
            custom_id(actor, view.key(), target, None),
            label,
            ButtonStyle::Secondary,
        )
    };
    Some(CreateActionRow::Buttons(vec![
        make(page.index.saturating_sub(1), "Previous").disabled(page.index == 0),
        make(page.index + 1, "Next").disabled(page.index + 1 == page.total),
    ]))
}

fn inline(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '`' => '’',
            c if c.is_control() => ' ',
            _ => c,
        })
        .collect()
}

struct RoomCard {
    id: ChannelId,
    name: String,
    owner: UserId,
    members: Vec<UserId>,
    state: &'static str,
}

impl RoomCard {
    fn row(&self) -> String {
        let mut row = format!(
            "`{}` <#{}> · owner <@{}> · {} in voice · {}",
            inline(&self.name),
            self.id,
            self.owner,
            self.members.len(),
            self.state
        );
        for member in self.members.iter().take(2) {
            row.push_str(&format!(" <@{member}>"));
        }
        row
    }

    fn members(&self) -> String {
        const MAX_SHOWN: usize = 100;
        let listed = self
            .members
            .iter()
            .take(MAX_SHOWN)
            .map(|id| format!("<@{id}>"))
            .collect::<Vec<_>>()
            .join(" ");
        match self.members.len() {
            0 => "The room is empty.".into(),
            count if count > MAX_SHOWN => format!("{listed} · +{}", count - MAX_SHOWN),
            _ => listed,
        }
    }
}

fn room_page(
    sorted_rooms: &[store::Room],
    requested: usize,
    selected: Option<ChannelId>,
) -> (Page, &[store::Room]) {
    let selected_index = selected.and_then(|channel| {
        sorted_rooms
            .binary_search_by_key(&channel.get(), |room| room.channel_id)
            .ok()
    });
    let page = selected_index.map_or(requested, |index| index / PAGE_SIZE);
    Page::get(sorted_rooms, page)
}

fn rooms_reply(
    ctx: &Context,
    guild: GuildId,
    actor: UserId,
    requested: usize,
    selected: Option<ChannelId>,
    mode: Mode,
    notice: &str,
) -> Result<EditInteractionResponse> {
    let mut records = store::guild_rooms(&*db()?, guild.get())?;
    records.sort_unstable_by_key(|room| room.channel_id);
    let (page, records) = room_page(&records, requested, selected);
    let rooms = {
        let cached = ctx
            .cache
            .guild(guild)
            .context("guild channels unavailable")?;
        records
            .iter()
            .map(|row| {
                let id = ChannelId::new(row.channel_id);
                let channel = cached.channels.get(&id);
                let mut members = cached
                    .voice_states
                    .values()
                    .filter(|state| state.channel_id == Some(id))
                    .map(|state| state.user_id)
                    .collect::<Vec<_>>();
                members.sort_unstable();
                let state = channel.map_or("Room unavailable", |ch| {
                    match (
                        actions::room_restricted(ch, Permissions::CONNECT),
                        actions::room_restricted(ch, Permissions::VIEW_CHANNEL),
                    ) {
                        (true, true) => "Locked · Hidden",
                        (true, false) => "Locked · Visible",
                        (false, true) => "Unlocked · Hidden",
                        (false, false) => "Unlocked · Visible",
                    }
                });
                RoomCard {
                    id,
                    name: channel.map_or_else(|| format!("Room {id}"), |ch| ch.name.clone()),
                    owner: UserId::new(row.owner_id),
                    members,
                    state,
                }
            })
            .collect::<Vec<_>>()
    };
    let mut description = rooms
        .iter()
        .map(RoomCard::row)
        .collect::<Vec<_>>()
        .join("\n");
    let current = selected.and_then(|id| rooms.iter().find(|room| room.id == id));
    let mut rows: Vec<_> = navigation(actor, View::Rooms, &page).into_iter().collect();
    if rooms.is_empty() {
        description.push_str("There are no temporary rooms right now.");
    } else {
        let options = rooms
            .iter()
            .map(|room| {
                CreateSelectMenuOption::new(&room.name, room.id.to_string())
                    .description(format!("{} in voice · {}", room.members.len(), room.state))
                    .default_selection(selected == Some(room.id))
            })
            .collect();
        let select = CreateSelectMenu::new(
            custom_id(actor, "select", page.index, None),
            CreateSelectMenuKind::String { options },
        )
        .placeholder("Choose a room")
        .min_values(1)
        .max_values(1);
        rows.push(CreateActionRow::SelectMenu(select));
    }
    let mut embeds = vec![
        CreateEmbed::new()
            .title(format!("Temp voice rooms · {}", page.label()))
            .description(description),
    ];
    if let Some(room) = current {
        embeds.push(
            CreateEmbed::new()
                .title("Selected room")
                .description(format!("<#{}>\n{}", room.id, room.members())),
        );
        room_controls(&mut rows, actor, page.index, room.id, mode);
    }
    let notice = if selected.is_some()
        && current.is_none()
        && (notice.is_empty() || !matches!(mode, Mode::Browse))
    {
        "This temporary room is gone. Choose another room."
    } else {
        notice
    };
    Ok(edit().content(notice).embeds(embeds).components(rows))
}

fn room_controls(
    rows: &mut Vec<CreateActionRow>,
    actor: UserId,
    page: usize,
    room: ChannelId,
    mode: Mode,
) {
    let id = |action| custom_id(actor, action, page, Some(room));
    let make = |action, label, style| button(id(action), label, style);
    let cancel = || make("back", "Cancel", ButtonStyle::Secondary);
    match mode {
        Mode::Browse => rows.push(CreateActionRow::Buttons(vec![
            make("ask-delete", "Force delete", ButtonStyle::Danger),
            make("ask-transfer", "Force transfer", ButtonStyle::Secondary),
            make("claim", "Claim for me", ButtonStyle::Primary),
        ])),
        Mode::ConfirmDelete => rows.push(CreateActionRow::Buttons(vec![
            make("delete", "Confirm delete", ButtonStyle::Danger),
            cancel(),
        ])),
        Mode::Transfer => {
            let picker = CreateSelectMenu::new(
                id("transfer"),
                CreateSelectMenuKind::User {
                    default_users: None,
                },
            )
            .placeholder("Choose a new owner")
            .min_values(1)
            .max_values(1);
            rows.push(CreateActionRow::SelectMenu(picker));
            rows.push(CreateActionRow::Buttons(vec![cancel()]));
        }
    }
}

fn list_reply(
    ctx: &Context,
    guild: GuildId,
    actor: UserId,
    requested: usize,
    view: View,
) -> Result<EditInteractionResponse> {
    let (title, empty, lines) = match view {
        View::Perks => {
            let perks = store::role_perks(&*db()?, guild.get())?;
            let lines = perks
                .iter()
                .map(|perk| {
                    let limit = if perk.limit == 0 {
                        "unlimited".into()
                    } else {
                        perk.limit.to_string()
                    };
                    format!(
                        "<@&{}> · limit {limit} · {} kbps",
                        perk.role_id,
                        perk.bitrate / 1_000
                    )
                })
                .collect::<Vec<_>>();
            (
                "Temp voice perks",
                "There are no role perks yet.",
                lines,
            )
        }
        View::Hubs => (
            "Temp voice hubs",
            "There are no voice hubs yet.",
            hub_lines(ctx, guild)?,
        ),
        View::Rooms => return rooms_reply(ctx, guild, actor, requested, None, Mode::Browse, ""),
    };
    let (page, lines) = Page::get(&lines, requested);
    let description = if lines.is_empty() {
        empty.into()
    } else {
        lines.join("\n")
    };
    let embed = CreateEmbed::new()
        .title(format!("{title} · {}", page.label()))
        .description(description);
    Ok(edit()
        .embed(embed)
        .components(navigation(actor, view, &page).into_iter().collect()))
}

fn hub_lines(ctx: &Context, guild: GuildId) -> Result<Vec<String>> {
    let mut templates = store::hub_templates(&*db()?, guild.get())?;
    let enabled = control::ids(HUBS_KEY);
    if let Some(cached) = ctx.cache.guild(guild) {
        for id in enabled.iter().copied().filter(|id| *id != 0) {
            if cached
                .channels
                .get(&ChannelId::new(id))
                .is_some_and(|ch| ch.kind == ChannelType::Voice)
                && !templates.iter().any(|template| template.channel_id == id)
            {
                templates.push(HubTemplate {
                    channel_id: id,
                    ..HubTemplate::default()
                });
            }
        }
    }
    templates.sort_unstable_by_key(|hub| hub.channel_id);
    Ok(templates
        .iter()
        .map(|hub| {
            let mut detail = Vec::new();
            if let Some(name) = &hub.name_pattern {
                detail.push(format!("name `{}`", inline(name)));
            }
            if let Some(limit) = hub.limit {
                detail.push(if limit == 0 {
                    "unlimited members".into()
                } else {
                    format!("limit {limit}")
                });
            }
            if let Some(bitrate) = hub.bitrate {
                detail.push(format!("{} kbps", bitrate / 1_000));
            }
            if let Some(category) = hub.category_id {
                detail.push(format!("category <#{category}>"));
            }
            if let Some(role) = hub.required_role_id {
                detail.push(format!("needs <@&{role}>"));
            }
            if detail.is_empty() {
                detail.push("defaults".into());
            }
            let status = if enabled.contains(&hub.channel_id) {
                "On"
            } else {
                "Off"
            };
            format!("<#{}> · {status} · {}", hub.channel_id, detail.join(" · "))
        })
        .collect())
}

struct Control<'a> {
    actor: UserId,
    action: &'a str,
    page: usize,
    channel: Option<ChannelId>,
}

impl<'a> Control<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        let mut parts = value.split(':');
        let actor = parts.next()?.parse::<u64>().ok().filter(|id| *id != 0)?;
        let action = parts.next()?;
        let page = parts.next()?.parse().ok()?;
        let channel = parts.next()?.parse::<u64>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            actor: UserId::new(actor),
            action,
            page,
            channel: (channel != 0).then(|| ChannelId::new(channel)),
        })
    }
}

pub(in super::super) async fn component(ctx: &Context, component: &ComponentInteraction, id: &str) {
    let authorized = (|| {
        let control = Control::parse(id).ok_or_else(|| user_error("This panel has expired."))?;
        let (guild, member) = component
            .guild_id
            .zip(component.member.as_ref())
            .ok_or_else(|| user_error("Use this in a server."))?;
        if staff(ctx, guild, member)? != control.actor {
            return Err(user_error("Use `/vc admin rooms` to open your own panel."));
        }
        Ok((guild, member, control))
    })();
    let (guild, member, control) = match authorized {
        Ok(authorized) => authorized,
        Err(err) => {
            let message = CreateInteractionResponseMessage::new()
                .content(error_text(err))
                .ephemeral(true)
                .allowed_mentions(CreateAllowedMentions::new());
            if let Err(err) = component
                .create_response(&ctx.http, CreateInteractionResponse::Message(message))
                .await
            {
                tracing::warn!(%err, "tempvc: could not respond to staff control");
            }
            return;
        }
    };
    if let Err(err) = component.defer(&ctx.http).await {
        tracing::warn!(%err, "tempvc: could not acknowledge staff control");
        return;
    }
    match component_reply(ctx, guild, member, control, &component.data.kind).await {
        Ok(reply) => {
            if let Err(err) = component.edit_response(&ctx.http, reply).await {
                tracing::warn!(%err, "tempvc: could not update staff control");
            }
        }
        Err(err) => {
            let reply = CreateInteractionResponseFollowup::new()
                .content(error_text(err))
                .ephemeral(true)
                .allowed_mentions(CreateAllowedMentions::new());
            if let Err(err) = component.create_followup(&ctx.http, reply).await {
                tracing::warn!(%err, "tempvc: could not report staff action result");
            }
        }
    }
}

async fn component_reply(
    ctx: &Context,
    guild: GuildId,
    member: &Member,
    control: Control<'_>,
    kind: &ComponentInteractionDataKind,
) -> Result<EditInteractionResponse> {
    let Control {
        actor,
        action,
        page,
        channel,
    } = control;
    let button = matches!(kind, ComponentInteractionDataKind::Button);
    match action {
        "perks" if button => return list_reply(ctx, guild, actor, page, View::Perks),
        "hubs" if button => return list_reply(ctx, guild, actor, page, View::Hubs),
        _ => actions::check_enabled()?,
    }
    let render =
        |channel, mode, notice| rooms_reply(ctx, guild, actor, page, channel, mode, notice);
    let force = match (action, kind) {
        ("rooms", _) if button => return list_reply(ctx, guild, actor, page, View::Rooms),
        ("select", ComponentInteractionDataKind::StringSelect { values }) if values.len() == 1 => {
            let selected = values
                .first()
                .and_then(|id| id.parse::<u64>().ok())
                .filter(|id| *id != 0)
                .map(ChannelId::new);
            return render(selected, Mode::Browse, "");
        }
        ("back", _) if button => return render(channel, Mode::Browse, ""),
        ("ask-delete", _) if button => {
            return render(
                channel,
                Mode::ConfirmDelete,
                "Delete this room?",
            );
        }
        ("ask-transfer", _) if button => {
            return render(channel, Mode::Transfer, "Who should own this room?");
        }
        ("delete", _) if button => ForceAction::Delete,
        ("claim", _) if button => ForceAction::Claim,
        ("transfer", ComponentInteractionDataKind::UserSelect { values }) if values.len() == 1 => {
            ForceAction::Transfer(values[0])
        }
        _ => return Err(user_error("This control is unavailable.")),
    };
    let room = channel.ok_or_else(|| user_error("Choose a room first."))?;
    let deleted = matches!(force, ForceAction::Delete);
    let reply = actions::execute(
        ctx,
        guild,
        member,
        RoomTarget::Voice(Some(room)),
        Action::Force(force),
    )
    .await?;
    let notice = match reply {
        Reply::Text(text) => text,
        Reply::Info(_) => String::new(),
    };
    render((!deleted).then_some(room), Mode::Browse, &notice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_pages_cover_all_rows_and_clamp_after_deletions() {
        let records = (1..=19).collect::<Vec<_>>();
        assert_eq!(Page::get(&records, 0).1, &records[..8]);
        assert_eq!(Page::get(&records, 1).1, &records[8..16]);
        assert_eq!(Page::get(&records, 2).1, &records[16..]);
        let (page, rows) = Page::get(&records[..8], usize::MAX);
        assert_eq!(page.label(), "Page 1 / 1");
        assert_eq!(rows, &records[..8]);
    }

    #[test]
    fn selected_room_stays_visible_when_deletions_shift_its_page() {
        let rooms = (1..=17)
            .map(|channel_id| store::Room {
                channel_id,
                guild_id: 1,
                hub_id: 1,
                owner_id: 1,
            })
            .collect::<Vec<_>>();
        let selected = Some(ChannelId::new(9));
        assert_eq!(room_page(&rooms, 1, selected).0.index, 1);
        let (page, shown) = room_page(&rooms[1..], 1, selected);
        assert_eq!(page.index, 0);
        assert!(shown.iter().any(|room| room.channel_id == 9));
    }

    #[test]
    fn staff_controls_bind_actor_page_and_room() {
        let id = custom_id(UserId::new(1), "transfer", 12, Some(ChannelId::new(2)));
        let control = Control::parse(id.strip_prefix("tvc:admin:").unwrap()).unwrap();
        assert_eq!(control.actor, UserId::new(1));
        assert_eq!(control.action, "transfer");
        assert_eq!(control.page, 12);
        assert_eq!(control.channel, Some(ChannelId::new(2)));
        assert!(Control::parse("0:transfer:12:2").is_none());
        assert!(Control::parse("1:transfer:12:2:extra").is_none());
    }

    #[test]
    fn dashboard_first_page_has_distinct_pager_ids() {
        let page = Page { index: 0, total: 2 };
        let row = navigation(UserId::new(1), View::Rooms, &page).unwrap();
        let row = serde_json::to_value(row).unwrap();
        let buttons = row["components"].as_array().unwrap();
        assert_eq!(buttons.len(), 2);
        assert_eq!(buttons[0]["disabled"], true);
        assert_ne!(buttons[0]["custom_id"], buttons[1]["custom_id"]);
    }
}
