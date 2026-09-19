//! `/help`, written from the same catalog the panel shows, so it can't fall
//! behind when a command is added or changed. Members see the commands they
//! can use; bot admins also see the admin ones.

use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateCommand, CreateCommandOption,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
};

use super::catalog::{self, Command, Section};

/// Discord's limit on one embed's description, kept clear of.
const DESCRIPTION_LIMIT: usize = 3900;
/// How much of a section's "about" the detail view shows.
const ABOUT_LIMIT: usize = 900;

fn admin_only(c: &Command) -> bool {
    c.who.to_ascii_lowercase().contains("admin")
}

/// Sections with at least one command the viewer may use, with just those commands.
fn visible(sections: Vec<Section>, is_admin: bool) -> Vec<Section> {
    sections
        .into_iter()
        .map(|mut s| {
            s.commands.retain(|c| is_admin || !admin_only(c));
            s
        })
        .filter(|s| !s.commands.is_empty())
        .collect()
}

pub fn builder() -> CreateCommand {
    let mut topic = CreateCommandOption::new(CommandOptionType::String, "topic", "details on one area");
    for s in catalog::sections().iter().filter(|s| !s.commands.is_empty()).take(25) {
        topic = topic.add_string_choice(format!("{} {}", s.icon, s.title), s.id);
    }
    CreateCommand::new("help").description("what the bot does and every command you can use").add_option(topic)
}

fn cut(text: &str, limit: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= limit {
        return text;
    }
    let mut out: String = text.chars().take(limit).collect();
    if let Some(end) = out.rfind(". ") {
        out.truncate(end + 1);
    } else {
        out.push('…');
    }
    out
}

/// The overview: a line per area with its commands.
fn overview(sections: &[Section], is_admin: bool) -> CreateEmbed {
    let mut text = String::from(
        "MLCI's own bot. **@mention me** in chat to talk - otherwise I just read along. \
         Houses, points, the Snitch, Chocolate Frogs, quiz, arena and more below.\n",
    );
    for s in sections {
        // Message menu entries (like Quote) aren't slash commands; they're
        // covered by the line at the end.
        let names: Vec<String> =
            s.commands.iter().filter(|c| c.name.starts_with(|ch: char| ch.is_ascii_lowercase())).map(|c| format!("`/{}`", c.name)).collect();
        if names.is_empty() {
            continue;
        }
        let line = format!("\n{} **{}** · {}", s.icon, s.title, names.join(" "));
        if text.chars().count() + line.chars().count() > DESCRIPTION_LIMIT {
            break;
        }
        text.push_str(&line);
    }
    text.push_str("\n\n**Without a command:** reply **ACCIO** to a Snitch card to catch it · press 🐸 **Catch it** on a Chocolate Frog and answer its riddle · press ✋ **I'm in** on the Name Place Animal Thing lobby, then ✍️ **Submit answers** · right-click a message → Apps → **Quote** for a quote card.");
    let footer = if is_admin {
        "/help topic: for details · admin commands included because you're a bot admin"
    } else {
        "/help topic: for details on any area · only you can see this"
    };
    CreateEmbed::new().title("🤖 Loduchand · help").description(text).colour(0x5865F2).footer(CreateEmbedFooter::new(footer))
}

/// One area in detail: what it is, then each command with how to use it.
fn detail(s: &Section) -> CreateEmbed {
    let mut text = cut(s.about, ABOUT_LIMIT);
    text.push('\n');
    for c in &s.commands {
        let tag = if admin_only(c) { " · *admins*" } else { "" };
        let line = format!("\n**{}**{}\n{}", c.usage, tag, cut(c.what, 300));
        if text.chars().count() + line.chars().count() > DESCRIPTION_LIMIT {
            break;
        }
        text.push_str(&line);
    }
    CreateEmbed::new()
        .title(format!("{} {}", s.icon, s.title))
        .description(text)
        .colour(0x5865F2)
        .footer(CreateEmbedFooter::new("/help for every area · only you can see this"))
}

pub async fn command(ctx: &Context, command: &CommandInteraction) {
    let is_admin = super::super::admin_ids().contains(&command.user.id.get());
    let sections = visible(catalog::sections(), is_admin);
    let topic = command.data.options.iter().find_map(|o| match (&o.value, o.name.as_str()) {
        (CommandDataOptionValue::String(v), "topic") => Some(v.clone()),
        _ => None,
    });
    let message = match topic.and_then(|t| sections.iter().find(|s| s.id == t)) {
        Some(section) => CreateInteractionResponseMessage::new().embed(detail(section)),
        None if command.data.options.is_empty() => CreateInteractionResponseMessage::new().embed(overview(&sections, is_admin)),
        None => CreateInteractionResponseMessage::new()
            .content("That area has no commands you can use. Here's everything:")
            .embed(overview(&sections, is_admin)),
    };
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(message.ephemeral(true))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn members_never_see_admin_commands() {
        for s in visible(catalog::sections(), false) {
            assert!(s.commands.iter().all(|c| !admin_only(c)), "{} shows an admin command", s.id);
        }
        let admin_total: usize = visible(catalog::sections(), true).iter().map(|s| s.commands.len()).sum();
        let member_total: usize = visible(catalog::sections(), false).iter().map(|s| s.commands.len()).sum();
        assert!(admin_total > member_total);
    }

    #[test]
    fn every_command_the_bot_registers_is_explained() {
        let names: Vec<&str> = catalog::sections().iter().flat_map(|s| s.commands.iter().map(|c| c.name)).collect();
        for needed in ["help", "today", "mypoints", "housetop", "fight", "battle", "quiz", "snitchdrop", "frogdrop", "frogs", "frogcard", "housecards", "trade", "trades", "sellset", "panel", "guiderefresh", "npatstop", "sudoku", "sudokutop", "sudokuhelp", "sudokunew", "chess", "chesshelp", "chessstop", "anagram", "anagramhelp", "anagramskip", "anagramstop", "guess", "guesshelp", "guessskip", "guessstop", "about", "forgetme"] {
            assert!(names.contains(&needed), "/{} is missing from the catalog", needed);
        }
    }

    #[test]
    fn long_text_is_cut_at_a_sentence() {
        assert_eq!(cut("Short.", 50), "Short.");
        let long = "One sentence here. Another sentence that runs on and on.";
        assert_eq!(cut(long, 30), "One sentence here.");
    }
}

