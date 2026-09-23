//! `/roast`: the bot makes fun of a member, from what it can actually see -
//! their messages, their game records, their time in voice, how often AutoMod
//! eats them, who they are always replying to.
//!
//! It only ever posts in the roast channel (`VIZIER_ROAST_CHANNEL`). Run
//! anywhere else, the result still lands there and the place it was run in gets
//! a line with a link to it. Whoever the roast is about is pinged there, so
//! nobody finds out second-hand: the member and the one who asked for it.
//!
//! `/roast` is text only.
//!
//! The rules and the check every model answer has to pass live in
//! `roast_build.rs`, with the tests. `/noroast` is the opt-out, and it is a
//! different thing from `/forgetme`: that one is about member notes, and
//! neither touches the other.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::params;
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions,
    CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, UserId,
};

use super::control;
use super::notes_facts::Facts;
use super::notes_build::Said;
use super::roast_build::{self as build, Dossier, Logged, Made};

/// Where the command posts unless the panel says otherwise.
pub const DEFAULT_CHANNEL: u64 = 1_516_534_303_968_858_312;
/// How long one model call may take.
const MODEL_WAIT: Duration = Duration::from_secs(120);

const ROAST_COLOUR: u32 = 0xE0_4F_2A;

// --- settings ----------------------------------------------------------------------------------

pub fn roast_on() -> bool {
    control::on("VIZIER_ROAST", true)
}

/// The one channel the command posts in.
pub fn channel() -> u64 {
    control::id("VIZIER_ROAST_CHANNEL").unwrap_or(DEFAULT_CHANNEL)
}

/// Members who have put themselves out of reach of the command. Kept apart
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

/// How long a gathered dossier is reused before being read again. The model is
/// still asked every time, so the words are always new; this only saves
/// re-reading the databases in a burst. Zero switches the reuse off.
pub fn cache_window() -> Duration {
    Duration::from_secs(control::number("VIZIER_ROAST_CACHE_MINS", 10).clamp(0, 180) * 60)
}

// --- what has already been read -----------------------------------------------------------------

/// Gathered dossiers, in memory only: cleared on restart, swept whenever one is
/// written, and never bigger than this.
const GATHER_CACHE_MAX: usize = 512;

fn dossier_cache() -> &'static Mutex<HashMap<u64, (Instant, Dossier)>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, (Instant, Dossier)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn sweep<K: Eq + std::hash::Hash, V>(map: &mut HashMap<K, (Instant, V)>, now: Instant, window: Duration) {
    map.retain(|_, (at, _)| now.duration_since(*at) < window);
    if map.len() >= GATHER_CACHE_MAX {
        map.clear();
    }
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

/// Everything the bot knows about one member, ready for a prompt. Reused for
/// `cache_window()` so a burst of roasts doesn't read every database again;
/// the name is always taken fresh, since that is the cheap part.
async fn dossier(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64, name: &str) -> Dossier {
    let (window, at) = (cache_window(), Instant::now());
    if !window.is_zero() {
        if let Some((read_at, d)) = dossier_cache().lock().get(&user).cloned() {
            if at.duration_since(read_at) < window {
                tracing::debug!("roast: reusing what was read about {} {:?} ago", user, at.duration_since(read_at));
                return Dossier { name: name.to_string(), ..d };
            }
        }
    }
    let now = Utc::now().timestamp();
    let (channels, members, words) = super::notes::cache_names(ctx);
    let facts = super::notes::facts_for(ctx, storage, user, words).await;
    let cap = max_messages();
    let messages = tokio::task::spawn_blocking(move || load_logged(user, cap)).await.unwrap_or_default();
    let d = dossier_from(user, name, &facts, now, &channels, &members, messages);
    if !window.is_zero() {
        let mut cache = dossier_cache().lock();
        sweep(&mut cache, at, window);
        cache.insert(user, (at, d.clone()));
    }
    d
}

// --- the embed ----------------------------------------------------------------------------------

fn roast_embed(name: &str, text: &str, by: &str) -> CreateEmbed {
    CreateEmbed::new()
        .title(format!("🔥 {} ko roast kiya gaya", name))
        .description(text)
        .colour(ROAST_COLOUR)
        .footer(CreateEmbedFooter::new(format!("asked for by {} · it's a joke, chill · /noroast to stay out of these", by)))
}

/// Posts the roast in the roast channel and tells the place the command was run
/// in where it went. `Err` is a line to show the person, never a raw error.
async fn post(ctx: &Context, command: &CommandInteraction, embed: CreateEmbed, ping: &[u64]) -> Result<(), String> {
    let home = channel();
    let message = CreateMessage::new()
        .content(build::ping_line(ping))
        .embed(embed)
        .allowed_mentions(CreateAllowedMentions::new().users(ping.iter().copied().map(UserId::new).collect::<Vec<_>>()));
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

/// Where the answer to the command itself goes: quietly, unless the roast is
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
    // Their own words, so a quote of a catchphrase isn't mistaken for the bot
    // being cruel about a banned area.
    let said = d.messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>().join("\n") + "\n" + &d.phrases.join("\n");
    match build::make(prompt, |raw| build::check_roast_quoting(raw, &said), ask).await {
        Made::Ok { text, tries } => {
            tracing::info!("roast: roast of {} written in {} tr{}", target, tries, if tries == 1 { "y" } else { "ies" });
            // /roast is text only: nothing to draw for one person.
            // A roast pings the person it is about and the one who asked for it.
            if let Err(err) = post(ctx, command, roast_embed(&name, &text, &command.user.display_name().to_string()), &[caller, target]).await {
                give_up(ctx, command, &err).await;
            }
        }
        other => give_up(ctx, command, build::failure_message(&other)).await,
    }
}

// --- /noroast ---------------------------------------------------------------------------------------

pub fn noroast_builder() -> CreateCommand {
    CreateCommand::new("noroast").description("keep yourself out of /roast - run it again to come back in")
}

const OUT: &str = "Done - nobody can `/roast` you while you're out, and nothing of yours goes to the AI for it. \
    Run `/noroast` again to come back in.\n\
    -# This is separate from `/forgetme`: that one is about member notes, and it hasn't changed.";
const IN: &str = "You're back in - `/roast` can name you again. Brace yourself.\n\
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
        // And the command the owner asked for exists, with the option it needs.
        let roast = serde_json::to_value(roast_builder()).unwrap();
        assert_eq!(roast["name"], "roast");
        assert_eq!(roast["options"][0]["name"], "member");
        assert_eq!(roast["options"][0]["required"], true);
        assert!(OUT.contains("separate from `/forgetme`") && IN.contains("different opt-out"));
        // /noroast says what it actually does now: it keeps you out of /roast.
        assert!(OUT.contains("`/roast`") && IN.contains("`/roast`"));
        // The roast list lives in the panel's settings; the notes list in notes.db.
        assert!(optouts().is_empty(), "nothing saved, nobody out");
    }

    /// Reading is reused for a few minutes; the AI is not, so the words are
    /// always new. The window is a setting, and 0 switches the reuse off.
    #[test]
    fn what_was_read_is_reused_for_a_window_that_can_be_switched_off() {
        assert_eq!(cache_window(), Duration::from_secs(600), "ten minutes with nothing set");
        assert!(!cache_window().is_zero(), "reuse is on by default");
        assert!(dossier_cache().lock().is_empty(), "nothing is remembered across restarts");
        // The sweep drops what is stale and never grows past its cap.
        let mut map: HashMap<u64, (Instant, u8)> = HashMap::new();
        let now = Instant::now();
        map.insert(1, (now, 1));
        map.insert(2, (now - Duration::from_secs(3600), 2));
        sweep(&mut map, now, Duration::from_secs(600));
        assert_eq!(map.keys().copied().collect::<Vec<_>>(), vec![1]);
        let mut full: HashMap<u64, (Instant, u8)> = (0..GATHER_CACHE_MAX as u64).map(|i| (i, (now, 0))).collect();
        sweep(&mut full, now, Duration::from_secs(600));
        assert!(full.is_empty(), "a cache that fills up is emptied rather than left to grow");
    }

    #[test]
    fn the_embed_says_what_it_is_and_how_to_get_out() {
        let d = build::tests::arjun();
        let embed = roast_embed(&d.name, "61 games, 12 wins. bhai.", "riya");
        let json = serde_json::to_value(&embed).unwrap();
        assert_eq!(json["title"], "🔥 arjun ko roast kiya gaya");
        let footer = json["footer"]["text"].as_str().unwrap();
        assert!(footer.contains("asked for by riya") && footer.contains("it's a joke") && footer.contains("/noroast"), "{footer}");
    }
}
