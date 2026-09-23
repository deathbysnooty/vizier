//! `/roast` and `/ship`: the bot makes fun of a member, or reads two of them
//! together, from what it can actually see - their messages, their game
//! records, their time in voice, how often AutoMod eats them, who they are
//! always replying to.
//!
//! Both only ever post in the roast channel (`VIZIER_ROAST_CHANNEL`). Run
//! anywhere else, the card still lands there and the place it was run in gets a
//! line with a link to it. The person who ran it and the person it is about are
//! both pinged where it lands, so nobody finds out second-hand.
//!
//! The rules and the check every model answer has to pass live in
//! `roast_build.rs`, with the tests. `/noroast` is the opt-out, and it is a
//! different thing from `/forgetme`: that one is about member notes, and
//! neither touches the other.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use chrono::Utc;
use rusqlite::params;
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions, CreateCommand,
    CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
    CreateMessage, EditInteractionResponse, UserId,
};

use super::control;
use super::kalesh;
use super::kalesh_store;
use super::notes_facts::Facts;
use super::notes_build::Said;
use super::roast_build::{self as build, Dossier, Logged, Made, Together};

/// Where both commands post unless the panel says otherwise.
pub const DEFAULT_CHANNEL: u64 = 1_516_534_303_968_858_312;
/// How long one model call may take.
const MODEL_WAIT: Duration = Duration::from_secs(120);
/// How far back `/ship` looks for the two of them talking.
const SHIP_PERIOD_MS: i64 = 60 * 86_400_000;
/// Their own messages one `/ship` reads at most, the way kalesh caps a search.
const SHIP_ROWS: usize = 12_000;
/// Things they said at each other shown to the model.
const SHIP_SAMPLE: usize = 14;
/// One of those, at most, in characters.
const SHIP_SAMPLE_CHARS: usize = 180;
/// Fights looked through for the two of them.
const DETECTIONS: usize = 5_000;

const ROAST_COLOUR: u32 = 0xE0_4F_2A;
const SHIP_COLOUR: u32 = 0xE7_54_80;

// --- settings ----------------------------------------------------------------------------------

pub fn roast_on() -> bool {
    control::on("VIZIER_ROAST", true)
}

pub fn ship_on() -> bool {
    control::on("VIZIER_SHIP", true)
}

/// The one channel both commands post in.
pub fn channel() -> u64 {
    control::id("VIZIER_ROAST_CHANNEL").unwrap_or(DEFAULT_CHANNEL)
}

/// Members who have put themselves out of reach of both commands. Kept apart
/// from the member-notes opt-out on purpose: `/forgetme` is about notes.
pub fn optouts() -> Vec<u64> {
    control::ids("VIZIER_ROAST_OPTOUTS")
}

/// A model on the bot's own provider; empty means the bot's usual one.
pub fn model_name() -> Option<String> {
    control::var("VIZIER_ROAST_MODEL")
}

/// Their own messages one roast reads at most, before the token budget trims
/// it further.
pub fn max_messages() -> usize {
    control::number("VIZIER_ROAST_MAX_MESSAGES", 250).clamp(20, 3_000) as usize
}

// --- the model ------------------------------------------------------------------------------------

/// One call to the roast model. The retry that matters for a dropped
/// connection is inside the model itself (`never_sent`); this one waits, and
/// `build::make` asks a second time if what comes back is no good.
async fn ask(prompt: String) -> anyhow::Result<String> {
    match tokio::time::timeout(MODEL_WAIT, control::web::ask_bot_model_with(prompt, model_name())).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("the model took over {}s", MODEL_WAIT.as_secs())),
    }
}

// --- reading the record ------------------------------------------------------------------------------

/// A member's own messages from the log, newest first, never #safe-corner.
/// Blocking.
fn load_logged(user: u64, cap: usize) -> Vec<Said> {
    let sensitive = control::insights::sensitive_channels();
    let mut rows: Vec<Logged> = Vec::new();
    if let Some(reader) = super::msglog::reader() {
        let conn = reader.conn.lock();
        if let Ok(mut stmt) =
            conn.prepare("SELECT content, created_ts, channel_id, parent_id FROM recent WHERE author_id = ?1 ORDER BY message_id DESC LIMIT ?2")
        {
            if let Ok(found) = stmt.query_map(params![user as i64, (cap * 4) as i64], |r| {
                Ok(Logged {
                    text: r.get::<_, String>(0)?,
                    ts_ms: r.get::<_, i64>(1)?,
                    channel: r.get::<_, i64>(2)? as u64,
                    parent: r.get::<_, Option<i64>>(3)?.map(|p| p as u64),
                })
            }) {
                rows.extend(found.flatten());
            }
        }
    }
    build::keep_messages(&rows, &sensitive, cap)
}

/// How many of their messages AutoMod blocked, and how many were deleted.
fn gone_counts(user: u64) -> (i64, i64) {
    let Some(reader) = super::msglog::reader() else { return (0, 0) };
    let conn = reader.conn.lock();
    let count = |sql: &str| conn.query_row(sql, params![user as i64], |r| r.get::<_, i64>(0)).unwrap_or(0);
    (count("SELECT COUNT(*) FROM blocked WHERE author_id = ?1"), count("SELECT COUNT(*) FROM deleted WHERE author_id = ?1"))
}

fn dossier_from(id: u64, name: &str, f: &Facts, now: i64, channels: &HashMap<u64, String>, members: &HashMap<u64, String>, messages: Vec<Said>) -> Dossier {
    let (blocked, deleted) = gone_counts(id);
    Dossier {
        id,
        name: name.to_string(),
        days_here: f.member_since.map(|since| ((now - since).max(0)) / 86_400),
        messages_all: f.messages_all,
        messages_month: f.messages_month,
        top_channels: f.top_channels.iter().filter_map(|c| channels.get(c).cloned()).collect(),
        games: f.games.iter().map(|g| (g.name.to_string(), g.detail.clone())).collect(),
        house: f.house.map(String::from),
        points_month: f.points_month,
        house_place: f.house_place,
        frog_cards: f.frog_cards,
        voice_month_mins: f.voice_month_secs / 60,
        automod_blocked: blocked,
        deleted,
        emoji: f.emoji.clone(),
        phrases: f.phrases.clone(),
        partners: f.partners.iter().filter_map(|p| members.get(p).cloned()).collect(),
        messages: build::read_sample(&messages),
    }
}

/// Everything the bot knows about one member, ready for a prompt.
async fn dossier(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64, name: &str) -> Dossier {
    let now = Utc::now().timestamp();
    let (channels, members, words) = super::notes::cache_names(ctx);
    let facts = super::notes::facts_for(ctx, storage, user, words).await;
    let cap = max_messages();
    let messages = tokio::task::spawn_blocking(move || load_logged(user, cap)).await.unwrap_or_default();
    dossier_from(user, name, &facts, now, &channels, &members, messages)
}

/// How two members behave around each other, from the message log and the
/// kalesh record. Blocking.
fn together_of(a: u64, b: u64, now_ms: i64, channels: &HashMap<u64, String>) -> Together {
    let mut out = Together::default();
    if let Some(reader) = super::msglog::reader() {
        let conn = reader.conn.lock();
        let rows = kalesh::authors_between(&conn, &[a, b], now_ms - SHIP_PERIOD_MS, now_ms, None, SHIP_ROWS).unwrap_or_default();
        let mut theirs: HashMap<u64, HashSet<u64>> = HashMap::new();
        for r in &rows {
            theirs.entry(r.channel_id).or_default().insert(r.author_id);
        }
        let mut shared: Vec<u64> = theirs.iter().filter(|(_, who)| who.contains(&a) && who.contains(&b)).map(|(c, _)| *c).collect();
        shared.sort_unstable();
        out.shared_channels = shared.iter().filter_map(|c| channels.get(c).cloned()).take(6).collect();
        let stretches = kalesh::find_stretches(&rows, &[a, b]);
        out.stretches = stretches.len();
        out.replies_ab = stretches.iter().map(|s| s.replies_from(0, 1)).sum();
        out.replies_ba = stretches.iter().map(|s| s.replies_from(1, 0)).sum();
        out.mentions = stretches.iter().map(|s| s.mentions).sum();
        // What they actually said at each other, newest stretches first.
        let mut sample: Vec<(String, String)> = Vec::new();
        for s in stretches.iter().take(4) {
            let in_it: Vec<_> = rows
                .iter()
                .filter(|r| r.channel_id == s.channel_id && r.created_ms >= s.start_ms && r.created_ms <= s.end_ms)
                .collect();
            for line in kalesh::exchange(&in_it.iter().map(|r| (*r).clone()).collect::<Vec<_>>(), &[a, b]) {
                if sample.len() >= SHIP_SAMPLE {
                    break;
                }
                let text = super::notes_build::cut(&line.row.content.split_whitespace().collect::<Vec<_>>().join(" "), SHIP_SAMPLE_CHARS);
                if text.chars().filter(|c| c.is_alphanumeric()).count() >= 4 {
                    sample.push((line.row.author_name.clone(), text));
                }
            }
        }
        out.sample = sample;
    }
    if let Some(db) = kalesh_store::db() {
        let found = kalesh_store::detections(&db.lock(), DETECTIONS).unwrap_or_default();
        out.fights = found.iter().filter(|d| d.participants.iter().any(|p| p.id == a) && d.participants.iter().any(|p| p.id == b)).count();
    }
    out
}

// --- the cards -----------------------------------------------------------------------------------

fn roast_embed(name: &str, text: &str, by: &str) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("🔥 {} ko roast kiya gaya", name))
        .description(text)
        .colour(ROAST_COLOUR)
        .footer(CreateEmbedFooter::new(format!("asked for by {} · it's a joke, chill · /noroast to stay out of these", by)))
}

fn ship_embed(a: &Dossier, b: &Dossier, ship: &str, percent: u8, verdict: &str, by: &str) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("💘 {}", ship))
        .description(format!(
            "**{}** × **{}**\n\n`{}` **{}%**\n\n{}",
            a.name,
            b.name,
            build::bar(percent),
            percent,
            verdict
        ))
        .colour(SHIP_COLOUR)
        .footer(CreateEmbedFooter::new(format!("shipped by {} · a bot's joke, nothing more · /noroast to stay out of these", by)))
}

/// Posts the card in the roast channel and tells the place the command was run
/// in where it went. `Err` is a line to show the person, never a raw error.
async fn post(ctx: &Context, command: &CommandInteraction, embed: CreateEmbed, targets: &[u64]) -> Result<(), String> {
    let caller = command.user.id.get();
    let home = channel();
    let message = CreateMessage::new()
        .content(build::ping_line(caller, targets))
        .embed(embed)
        .allowed_mentions(CreateAllowedMentions::new().users(std::iter::once(caller).chain(targets.iter().copied()).map(UserId::new).collect::<Vec<_>>()));
    let posted = ChannelId::new(home).send_message(&ctx.http, message).await.map_err(|err| {
        tracing::warn!("roast: couldn't post in {}: {}", home, err);
        format!("I couldn't post in <#{}>. Check I'm allowed to talk there.", home)
    })?;
    let used_in = command.channel_id.get();
    let note = match (build::needs_redirect_note(used_in, home), command.guild_id) {
        (true, Some(guild)) => build::posted_elsewhere(home, &build::message_link(guild.get(), home, posted.id.get())),
        // A DM: no link to give, so just say where it went.
        (true, None) => format!("Posted in <#{}>", home),
        (false, _) => "Ho gaya 🔥".to_string(),
    };
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(note).allowed_mentions(CreateAllowedMentions::new())).await;
    Ok(())
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()))
}

/// The named member, if the command names one.
fn picked(command: &CommandInteraction, name: &str) -> Option<u64> {
    command.data.options.iter().find(|o| o.name == name).and_then(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    })
}

fn is_bot(command: &CommandInteraction, user: u64) -> bool {
    command.data.resolved.users.get(&UserId::new(user)).is_some_and(|u| u.bot)
}

fn name_of(ctx: &Context, command: &CommandInteraction, user: u64) -> String {
    let fallback = command
        .data
        .resolved
        .members
        .get(&UserId::new(user))
        .and_then(|m| m.nick.clone())
        .or_else(|| command.data.resolved.users.get(&UserId::new(user)).map(|u| u.display_name().to_string()))
        .unwrap_or_else(|| if user == command.user.id.get() { command.user.display_name().to_string() } else { "that member".into() });
    super::notes::display_name(ctx, user, &fallback)
}

/// Where the answer to the command itself goes: quietly, unless the card is
/// landing somewhere else and the channel should be told where.
async fn open(ctx: &Context, command: &CommandInteraction) -> bool {
    if build::needs_redirect_note(command.channel_id.get(), channel()) {
        command.defer(&ctx.http).await.is_ok()
    } else {
        command.defer_ephemeral(&ctx.http).await.is_ok()
    }
}

async fn give_up(ctx: &Context, command: &CommandInteraction, text: &str) {
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

// --- /roast -----------------------------------------------------------------------------------------

pub fn roast_builder() -> CreateCommand {
    CreateCommand::new("roast").description("the bot roasts a member, from what it has actually seen them do").add_option(
        CreateCommandOption::new(CommandOptionType::User, "member", "kiska roast karna hai").required(true),
    )
}

pub async fn roast_command(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, command: &CommandInteraction) {
    if !roast_on() {
        let _ = command.create_response(&ctx.http, whisper("`/roast` is switched off right now.")).await;
        return;
    }
    let caller = command.user.id.get();
    let Some(target) = picked(command, "member") else {
        let _ = command.create_response(&ctx.http, whisper("Name someone: `/roast @member`.")).await;
        return;
    };
    if is_bot(command, target) {
        let _ = command.create_response(&ctx.http, whisper("Bots don't get roasted. Pick a human.")).await;
        return;
    }
    if let Some(out) = build::blocked(&optouts(), &[caller, target]) {
        let _ = command.create_response(&ctx.http, whisper(build::opted_out_message(out, caller, &name_of(ctx, command, out)))).await;
        return;
    }
    let name = name_of(ctx, command, target);
    if !open(ctx, command).await {
        return;
    }
    let d = dossier(ctx, storage, target, &name).await;
    let prompt = build::roast_prompt(&d);
    tracing::info!("roast: {} asked for a roast of {} ({} messages, about {} tokens)", caller, target, d.messages.len(), build::prompt_tokens(&prompt));
    match build::make(prompt, build::check_roast, ask).await {
        Made::Ok { text, tries } => {
            tracing::info!("roast: roast of {} written in {} tr{}", target, tries, if tries == 1 { "y" } else { "ies" });
            if let Err(err) = post(ctx, command, roast_embed(&name, &text, &command.user.display_name().to_string()), &[target]).await {
                give_up(ctx, command, &err).await;
            }
        }
        other => give_up(ctx, command, build::failure_message(&other)).await,
    }
}

// --- /ship ------------------------------------------------------------------------------------------

pub fn ship_builder() -> CreateCommand {
    CreateCommand::new("ship")
        .description("ship two members and see the damage - leave the second empty and it's you")
        .add_option(CreateCommandOption::new(CommandOptionType::User, "member", "pehla banda").required(true))
        .add_option(CreateCommandOption::new(CommandOptionType::User, "with", "doosra banda - khaali chhoda to tum ho").required(false))
}

pub async fn ship_command(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, command: &CommandInteraction) {
    if !ship_on() {
        let _ = command.create_response(&ctx.http, whisper("`/ship` is switched off right now.")).await;
        return;
    }
    let caller = command.user.id.get();
    let Some(first) = picked(command, "member") else {
        let _ = command.create_response(&ctx.http, whisper("Name someone: `/ship @member` or `/ship @a @b`.")).await;
        return;
    };
    // Only one named: it's them and whoever asked.
    let (a, b) = match picked(command, "with") {
        Some(second) => (first, second),
        None => (caller, first),
    };
    if a == b {
        let _ = command.create_response(&ctx.http, whisper("You can't ship someone with themselves. Well - you can, but the answer is 100% and it's sad.")).await;
        return;
    }
    if is_bot(command, a) || is_bot(command, b) {
        let _ = command.create_response(&ctx.http, whisper("Bots are not shippable. Pick two humans.")).await;
        return;
    }
    if let Some(out) = build::blocked(&optouts(), &[caller, a, b]) {
        let _ = command.create_response(&ctx.http, whisper(build::opted_out_message(out, caller, &name_of(ctx, command, out)))).await;
        return;
    }
    let (name_a, name_b) = (name_of(ctx, command, a), name_of(ctx, command, b));
    if !open(ctx, command).await {
        return;
    }
    let da = dossier(ctx, storage, a, &name_a).await;
    let db = dossier(ctx, storage, b, &name_b).await;
    let channels = super::notes::cache_names(ctx).0;
    let now_ms = Utc::now().timestamp_millis();
    let mut t = tokio::task::spawn_blocking(move || together_of(a, b, now_ms, &channels)).await.unwrap_or_default();
    t.shared_games = da.games.iter().map(|(g, _)| g.clone()).filter(|g| db.games.iter().any(|(o, _)| o == g)).collect();
    let percent = build::ship_percent(a, b);
    let ship = build::ship_name(&name_a, &name_b);
    let prompt = build::ship_prompt(&da, &db, &t, percent, &ship);
    tracing::info!("roast: {} shipped {} and {} ({}%, about {} tokens)", caller, a, b, percent, build::prompt_tokens(&prompt));
    match build::make(prompt, build::check_ship, ask).await {
        Made::Ok { text, .. } => {
            let by = command.user.display_name().to_string();
            if let Err(err) = post(ctx, command, ship_embed(&da, &db, &ship, percent, &text, &by), &[a, b]).await {
                give_up(ctx, command, &err).await;
            }
        }
        other => give_up(ctx, command, build::failure_message(&other)).await,
    }
}

// --- /noroast ---------------------------------------------------------------------------------------

pub fn noroast_builder() -> CreateCommand {
    CreateCommand::new("noroast").description("keep yourself out of /roast and /ship - run it again to come back in")
}

const OUT: &str = "Done - nobody can `/roast` or `/ship` you while you're out, and nothing of yours goes to the AI for \
    either. Run `/noroast` again to come back in.\n\
    -# This is separate from `/forgetme`: that one is about member notes, and it hasn't changed.";
const IN: &str = "You're back in - `/roast` and `/ship` can name you again. Brace yourself.\n\
    -# `/forgetme` is untouched: it's a different opt-out, about member notes.";

/// Flips a member's opt-out on the panel's list; true when they are now out.
pub fn toggle_optout(user: u64) -> anyhow::Result<bool> {
    let mut ids = optouts();
    let now_out = if ids.contains(&user) {
        ids.retain(|id| *id != user);
        false
    } else {
        ids.push(user);
        true
    };
    let value = ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
    control::set("VIZIER_ROAST_OPTOUTS", (!value.is_empty()).then_some(value.as_str()), user)?;
    Ok(now_out)
}

pub async fn noroast_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    let text = match toggle_optout(user) {
        Ok(true) => OUT,
        Ok(false) => IN,
        Err(err) => {
            tracing::warn!("roast: /noroast for {} failed: {}", user, err);
            "That didn't work - try again in a minute."
        }
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one channel the owner asked for is what applies with nothing set.
    #[test]
    fn the_roast_channel_defaults_to_the_owners_channel() {
        assert_eq!(DEFAULT_CHANNEL, 1_516_534_303_968_858_312);
        assert_eq!(channel(), DEFAULT_CHANNEL, "no setting saved: the owner's channel");
        assert!(!build::needs_redirect_note(DEFAULT_CHANNEL, channel()));
        assert!(build::needs_redirect_note(999, channel()));
    }

    /// The two opt-outs are different lists, in different places, with
    /// different commands: nobody loses their notes opt-out by running /noroast.
    #[test]
    fn the_roast_opt_out_is_not_the_notes_opt_out() {
        let name_of = |c: &CreateCommand| serde_json::to_value(c).unwrap()["name"].as_str().unwrap_or_default().to_string();
        assert_eq!(name_of(&noroast_builder()), "noroast");
        assert_eq!(name_of(&super::super::notes::forgetme_builder()), "forgetme");
        // And the two commands the owner asked for exist, with the options they need.
        let roast = serde_json::to_value(roast_builder()).unwrap();
        assert_eq!(roast["name"], "roast");
        assert_eq!(roast["options"][0]["name"], "member");
        assert_eq!(roast["options"][0]["required"], true);
        let ship = serde_json::to_value(ship_builder()).unwrap();
        assert_eq!(ship["name"], "ship");
        assert_eq!(ship["options"][1]["name"], "with");
        assert_eq!(ship["options"][1]["required"], false, "the second member is optional: it's the caller when left out");
        assert!(OUT.contains("separate from `/forgetme`") && IN.contains("different opt-out"));
        // The roast list lives in the panel's settings; the notes list in notes.db.
        assert!(optouts().is_empty(), "nothing saved, nobody out");
    }

    #[test]
    fn the_cards_say_what_they_are_and_how_to_get_out() {
        let d = build::tests::arjun();
        let embed = roast_embed(&d.name, "61 games, 12 wins. bhai.", "riya");
        let json = serde_json::to_value(&embed).unwrap();
        assert_eq!(json["title"], "🔥 arjun ko roast kiya gaya");
        let footer = json["footer"]["text"].as_str().unwrap();
        assert!(footer.contains("asked for by riya") && footer.contains("it's a joke") && footer.contains("/noroast"), "{footer}");
        let ship = ship_embed(&d, &build::tests::riya(), "Arjiya", 41, "they fight in #chess and call it love", "dev");
        let json = serde_json::to_value(&ship).unwrap();
        assert_eq!(json["title"], "💘 Arjiya");
        let body = json["description"].as_str().unwrap();
        assert!(body.contains("**arjun** × **riya**") && body.contains("41%") && body.contains("█"), "{body}");
        assert!(json["footer"]["text"].as_str().unwrap().contains("a bot's joke"));
    }
}
