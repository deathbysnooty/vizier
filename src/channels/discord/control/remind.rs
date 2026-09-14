//! `/remind` and `/reminders`: members set and manage their own reminders
//! without going through the AI. The AI's reminder tools use the same store.

use chrono::Utc;
use serenity::all::{
    ButtonStyle, CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction, Context,
    CreateActionRow, CreateButton, CreateCommand, CreateCommandOption, CreateInteractionResponse,
    CreateInteractionResponseMessage,
};

use super::memos;

pub fn remind_builder() -> CreateCommand {
    CreateCommand::new("remind")
        .description("set a reminder - the bot pings you here when it's time")
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "when", "e.g. in 2 hours, 30m, at 9pm, tomorrow 9am, 2026-09-20 18:00")
                .required(true),
        )
        .add_option(CreateCommandOption::new(CommandOptionType::String, "what", "what to remind you about").required(true))
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "admins only: remind someone else"))
}

pub fn reminders_builder() -> CreateCommand {
    CreateCommand::new("reminders").description("see and cancel your reminders")
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new().content(text).ephemeral(true)
}

pub async fn remind_command(ctx: &Context, command: &CommandInteraction) {
    let reply = if !super::super::control::on("VIZIER_MEMBER_REMINDERS", true) {
        whisper("Reminders are switched off right now.")
    } else {
        let opt = |name: &str| {
            command.data.options.iter().find_map(|o| match (&o.value, o.name == name) {
                (CommandDataOptionValue::String(v), true) => Some(v.clone()),
                _ => None,
            })
        };
        let (when, what) = (opt("when").unwrap_or_default(), opt("what").unwrap_or_default());
        let asker = command.user.id.get();
        let target = command
            .data
            .options
            .iter()
            .find_map(|o| match o.value {
                CommandDataOptionValue::User(id) => Some(id.get()),
                _ => None,
            })
            .unwrap_or(asker);
        let now = Utc::now().timestamp();
        match memos::parse_when(&when, now) {
            None => whisper(format!(
                "I couldn't read \"{}\" as a time. Try `in 2 hours`, `30m`, `at 9pm`, `tomorrow 9am` or `2026-09-20 18:00` (India time).",
                when
            )),
            Some(due) => match memos::create(target, command.channel_id.get(), &what, due, "command", asker) {
                Ok(m) => CreateInteractionResponseMessage::new().content(if target == asker {
                    format!("⏰ Got it, <@{}> — I'll remind you **{}** (<t:{}:R>): {}", m.user_id, memos::describe(m.due_ts, now), m.due_ts, m.text)
                } else {
                    format!("⏰ Got it — I'll remind <@{}> **{}** (<t:{}:R>): {}", m.user_id, memos::describe(m.due_ts, now), m.due_ts, m.text)
                })
                .allowed_mentions(serenity::all::CreateAllowedMentions::new()),
                Err(err) => whisper(err),
            },
        }
    };
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

fn list_message(user: u64) -> CreateInteractionResponseMessage {
    let now = Utc::now().timestamp();
    let mine = memos::pending_for(user);
    if mine.is_empty() {
        return whisper("You have no reminders waiting. Set one with `/remind` or just ask me: \"@Loduchand remind me in 2 hours to…\"");
    }
    let lines: Vec<String> = mine
        .iter()
        .enumerate()
        .map(|(i, m)| format!("**{}.** {} (<t:{}:R>) · {}", i + 1, memos::describe(m.due_ts, now), m.due_ts, m.text))
        .collect();
    let buttons: Vec<CreateButton> = mine
        .iter()
        .take(20)
        .enumerate()
        .map(|(i, m)| CreateButton::new(format!("remindcancel:{}", m.id)).label(format!("Cancel {}", i + 1)).style(ButtonStyle::Secondary))
        .collect();
    let rows: Vec<CreateActionRow> = buttons.chunks(5).map(|c| CreateActionRow::Buttons(c.to_vec())).collect();
    whisper(format!("⏰ **Your reminders**\n{}", lines.join("\n"))).components(rows)
}

pub async fn reminders_command(ctx: &Context, command: &CommandInteraction) {
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(list_message(command.user.id.get()))).await;
}

/// A Cancel button from `/reminders`: only the owner's own reminders.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let Some(id) = component.data.custom_id.strip_prefix("remindcancel:").and_then(|v| v.parse::<i64>().ok()) else {
        return;
    };
    let user = component.user.id.get();
    let done = memos::cancel(id, Some(user));
    let mut message = list_message(user);
    if !done {
        message = message.content("That reminder was already sent or cancelled.");
    }
    let update = serenity::all::CreateInteractionResponse::UpdateMessage(message);
    let _ = component.create_response(&ctx.http, update).await;
}
