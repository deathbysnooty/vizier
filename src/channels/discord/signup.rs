//! The sign-up button: one message the owner words themselves, two buttons
//! under it, and a list of who is in.
//!
//! `/signup` posts the message into a channel with **Yes, I'm in** and **No,
//! I'm out** under it, and up to three posters alongside. Pressing yes gives the
//! member the *server games* role (`VIZIER_GAMES_ROLE`) and writes them down;
//! pressing no takes the role off again and is otherwise a quiet no. Either way
//! only the member who pressed sees the answer.
//!
//! Three things this is careful about.
//!
//! **The record comes first.** Next month's eggs are handed out from the list on
//! the panel, so a press is written to `signups.db` whether or not the role
//! could be given. An empty setting, a role that has been deleted, or a role
//! sitting above the bot's own in the role list all mean the member is still on
//! the sheet — the log says plainly what went wrong, the panel warns about it,
//! and the startup check says so before anybody has pressed anything.
//!
//! **Nothing is remembered in memory.** The buttons are dispatched by their
//! custom id, and the words to rewrite the message with come back out of the
//! store, so the message posted before a restart keeps working after one.
//!
//! **The live count is batched.** A rush of clicks would otherwise be a rush of
//! message edits, which Discord rate-limits hard. Each message gets at most one
//! edit every [`EDIT_GAP`], and the edit that does go out reads the count fresh,
//! so the number on the message is never more than a few seconds stale.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::Connection;
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateCommand, CreateCommandOption,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, EditMessage, GuildId,
    Http, MessageId, RoleId, UserId,
};

use super::control;
use super::signup_store::{self as store, Answer, Post};

/// The setting that holds the role given to everyone who says yes.
pub const ROLE_KEY: &str = "VIZIER_GAMES_ROLE";

/// The two buttons. Dispatch is by these strings and nothing else, so a message
/// posted before a restart still answers afterwards.
pub const ID_IN: &str = "signup:in";
pub const ID_OUT: &str = "signup:out";

/// At most one edit of the live count per message per this long.
pub const EDIT_GAP: Duration = Duration::from_secs(5);

/// Posters the announcement may carry.
pub const MAX_IMAGES: usize = 3;

/// Discord's limit on a message, less room for the count line the bot adds.
const TEXT_ROOM: usize = 1900;

/// The most a poster may weigh before the bot gives up fetching it.
const IMAGE_CAP: usize = 8 << 20;

/// Whether a custom id belongs to this feature.
pub fn owns_component(id: &str) -> bool {
    id == ID_IN || id == ID_OUT
}

// --- the role ----------------------------------------------------------------

/// The role id in the setting, when one is set. Written out in full rather than
/// through [`ROLE_KEY`] so the catalog's scan of what is read can see it.
pub fn role_id() -> Option<u64> {
    control::id("VIZIER_GAMES_ROLE")
}

/// What the bot needs to know about one role to judge whether it can hand it out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoleFacts {
    pub id: u64,
    pub name: String,
    /// Where it sits in the role list. Higher is higher.
    pub position: i64,
    /// True for a role Discord looks after itself — a bot's own, or an
    /// integration's. Nobody can be given one of those, the bot included.
    pub managed: bool,
}

/// Whether the bot can give and take the sign-up role, and if not, why not.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Verdict {
    /// No role id in the setting. Presses are still written down.
    Unset,
    /// The setting names a role this server hasn't got — renamed, deleted, or a
    /// number typed wrong.
    Missing { id: String },
    /// A role Discord manages itself, which nobody can be given.
    Managed { id: String, name: String },
    /// Above the bot's own highest role, so Discord refuses every attempt.
    TooHigh { id: String, name: String, position: i64, bot_position: i64 },
    /// The bot can give it and take it.
    Fine { id: String, name: String },
}

impl Verdict {
    /// True when a press can actually put the role on somebody.
    pub fn usable(&self) -> bool {
        matches!(self, Verdict::Fine { .. })
    }

    /// The one line that goes in the log and on the panel. `None` when all is well.
    pub fn warning(&self) -> Option<String> {
        match self {
            Verdict::Fine { .. } => None,
            Verdict::Unset => Some(format!(
                "{} is empty, so nobody who presses Yes gets a role — they are written down and a mod has to hand the role out.",
                ROLE_KEY
            )),
            Verdict::Missing { id } => Some(format!(
                "{} is set to {}, and this server has no such role. Presses are written down but no role is given.",
                ROLE_KEY, id
            )),
            Verdict::Managed { id, name } => Some(format!(
                "{} points at \"{}\" ({}), which Discord manages itself — it cannot be given to anybody. Presses are written down only.",
                ROLE_KEY, name, id
            )),
            Verdict::TooHigh { id, name, position, bot_position } => Some(format!(
                "\"{}\" ({}) sits at position {} and the bot's own highest role is at {}. Discord will refuse every attempt: drag the bot's role above \"{}\" in Server Settings › Roles. Presses are written down meanwhile.",
                name, id, position, bot_position, name
            )),
        }
    }

    /// The role, when there is a usable one.
    pub fn role(&self) -> Option<u64> {
        match self {
            Verdict::Fine { id, .. } => id.parse().ok(),
            _ => None,
        }
    }
}

/// Judges the setting against the server's roles. Pure, so every awkward case
/// is a test rather than a thing to try on a live server.
///
/// A tie in position counts as too high: Discord's own check is strictly
/// greater, so two roles at the same position cannot manage one another.
pub fn verdict(setting: Option<u64>, roles: &[RoleFacts], bot_top: Option<i64>) -> Verdict {
    let Some(want) = setting else { return Verdict::Unset };
    let Some(role) = roles.iter().find(|r| r.id == want) else { return Verdict::Missing { id: want.to_string() } };
    if role.managed {
        return Verdict::Managed { id: want.to_string(), name: role.name.clone() };
    }
    // No idea where the bot stands is treated as "can't": saying so and being
    // wrong costs a line in the log, the other way round loses sign-ups.
    let top = bot_top.unwrap_or(i64::MIN);
    if top <= role.position {
        return Verdict::TooHigh { id: want.to_string(), name: role.name.clone(), position: role.position, bot_position: top };
    }
    Verdict::Fine { id: want.to_string(), name: role.name.clone() }
}

/// The same judgement, against the live server.
pub async fn role_state(ctx: &Context, guild: GuildId) -> Verdict {
    let setting = role_id();
    let Ok(roles) = guild.roles(&ctx.http).await else {
        // Nothing can be said about a role list that wouldn't load, and the
        // startup check runs again next boot.
        return setting.map(|id| Verdict::Missing { id: id.to_string() }).unwrap_or(Verdict::Unset);
    };
    let facts: Vec<RoleFacts> = roles
        .values()
        .map(|r| RoleFacts { id: r.id.get(), name: r.name.clone(), position: r.position as i64, managed: r.managed })
        .collect();
    let me = ctx.cache.current_user().id;
    let bot_top = match guild.member(&ctx.http, me).await {
        Ok(member) => member.roles.iter().filter_map(|id| facts.iter().find(|f| f.id == id.get())).map(|f| f.position).max(),
        Err(_) => None,
    };
    verdict(setting, &facts, bot_top)
}

/// Says at startup whether the role can be handed out, so a misconfiguration is
/// in the log before the first member presses anything rather than after.
pub fn check_at_startup(ctx: &Context, guild: GuildId) {
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let state = role_state(&ctx, guild).await;
        match state.warning() {
            None => tracing::info!("signup: the games role can be given and taken"),
            Some(why) => tracing::warn!("signup: {}", why),
        }
    });
}

/// Puts the games role on somebody or takes it off. Behind a trait so the button
/// can be tested without Discord.
#[async_trait::async_trait]
pub trait Wardrobe: Send + Sync {
    /// `Ok(true)` when the member's roles actually moved.
    async fn wear(&self, user: u64, on: bool) -> anyhow::Result<bool>;
}

/// The real one: one role in one guild.
pub struct GuildRole<'a> {
    pub ctx: &'a Context,
    pub guild: GuildId,
    pub role: RoleId,
}

#[async_trait::async_trait]
impl Wardrobe for GuildRole<'_> {
    async fn wear(&self, user: u64, on: bool) -> anyhow::Result<bool> {
        let member = self.guild.member(&self.ctx.http, UserId::new(user)).await?;
        match (on, member.roles.contains(&self.role)) {
            (true, false) => member.add_role(&self.ctx.http, self.role).await?,
            (false, true) => member.remove_role(&self.ctx.http, self.role).await?,
            // Already how it should be: nothing to ask Discord for.
            _ => return Ok(false),
        }
        Ok(true)
    }
}

// --- what a press means ------------------------------------------------------

/// What came of one press.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Press {
    /// What only the member who pressed sees.
    pub text: String,
    /// What the log should say, when something needs saying.
    pub note: Option<String>,
    /// In, out — after the press.
    pub counts: (i64, i64),
    /// True when the record moved, so the live count is worth rewriting.
    pub changed: bool,
    /// True when the role went on or came off.
    pub role_moved: bool,
}

pub const IN_REPLY: &str = "You're in — see you next month.";
pub const IN_AGAIN: &str = "You're already in — nothing more to do. See you next month.";
pub const OUT_REPLY: &str = "Noted — you're out. Nothing else happens.";
pub const OUT_AGAIN: &str = "You're already out — nothing more to do.";
/// What a yes is told when the role could not be put on them.
pub const NO_ROLE_NOTE: &str = " Your name is on the list; a mod still has to give you the role.";

/// One press, from the record to the reply. `wardrobe` is `None` when there is
/// no role the bot can give — the press is written down all the same, which is
/// the whole reason the list exists.
///
/// The connection is locked in short bursts and never across an await.
pub async fn press(
    db: &Mutex<Connection>,
    wardrobe: Option<&dyn Wardrobe>,
    user: u64,
    name: &str,
    pressed: Answer,
    now: i64,
) -> Press {
    let before = {
        let conn = db.lock();
        store::answer_of(&conn, user).ok().flatten()
    };
    let already = before.as_ref().is_some_and(|e| e.answer == pressed);
    let wants = pressed == Answer::In;
    let mut worn = before.as_ref().is_some_and(|e| e.worn);
    let mut note = None;
    let mut role_moved = false;

    // Somebody pressing the same button twice, with the role already the way it
    // should be, is not asked of Discord a second time. Somebody recorded while
    // the setting was empty IS retried, so filling the setting in heals the list.
    if !(already && worn == wants) {
        match wardrobe {
            Some(w) => match w.wear(user, wants).await {
                Ok(moved) => {
                    role_moved = moved;
                    worn = wants;
                }
                Err(err) => {
                    worn = false;
                    note = Some(format!("the games role could not be {} {}: {}", if wants { "given to" } else { "taken off" }, user, err));
                }
            },
            None => {
                worn = false;
                // Only a yes is worth a line: there is nothing to take off a no.
                if wants {
                    note = Some(format!("{} said yes and was written down, but there is no games role the bot can give", user));
                }
            }
        }
    }

    let counts = {
        let conn = db.lock();
        if let Err(err) = store::record(&conn, user, name, pressed, worn, now) {
            // The list is the deliverable, so a write that failed is loud.
            tracing::error!("signup: {}'s answer was not written down: {}", user, err);
        }
        store::counts(&conn)
    };

    let mut text = match (pressed, already) {
        (Answer::In, false) => IN_REPLY.to_string(),
        (Answer::In, true) => IN_AGAIN.to_string(),
        (Answer::Out, false) => OUT_REPLY.to_string(),
        (Answer::Out, true) => OUT_AGAIN.to_string(),
    };
    if wants && !worn {
        text.push_str(NO_ROLE_NOTE);
    }
    Press { text, note, counts, changed: !already, role_moved }
}

// --- the live count ----------------------------------------------------------

/// What a click should do about the number on the message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Rewrite it now.
    Now,
    /// Rewrite it in this many milliseconds.
    After(i64),
    /// Nothing: a rewrite is already on its way and will read the fresh count.
    Nothing,
}

/// One message's edit budget. Clicks arriving inside the gap fold into the one
/// edit already scheduled, so a hundred presses in a minute is a handful of
/// edits rather than a hundred.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ticker {
    last_ms: i64,
    pending: bool,
}

impl Ticker {
    pub fn click(&mut self, now_ms: i64, gap_ms: i64) -> Plan {
        if self.pending {
            return Plan::Nothing;
        }
        if now_ms.saturating_sub(self.last_ms) >= gap_ms {
            self.last_ms = now_ms;
            return Plan::Now;
        }
        self.pending = true;
        Plan::After(gap_ms - (now_ms - self.last_ms))
    }

    /// The scheduled edit has gone out.
    pub fn fired(&mut self, now_ms: i64) {
        self.last_ms = now_ms;
        self.pending = false;
    }
}

static TICKERS: LazyLock<Mutex<HashMap<u64, Ticker>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// The count as it reads on the message.
pub fn count_line(counts: (i64, i64)) -> String {
    match counts {
        (0, 0) => "*Nobody has answered yet.*".to_string(),
        (yes, 0) => format!("**{} in**", yes),
        (yes, no) => format!("**{} in** · {} out", yes, no),
    }
}

/// The whole message: the owner's words, then the count on its own last line.
pub fn message_text(title: &str, body: &str, counts: (i64, i64)) -> String {
    let head = if title.trim().is_empty() { String::new() } else { format!("**{}**\n\n", title.trim()) };
    format!("{}{}\n\n{}", head, body.trim(), count_line(counts))
}

/// The two buttons, with the count nowhere in them — the number lives in the
/// text, so a press never has to redraw the buttons.
pub fn buttons() -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(ID_IN).label("Yes, I'm in").style(ButtonStyle::Success),
        CreateButton::new(ID_OUT).label("No, I'm out").style(ButtonStyle::Secondary),
    ])
}

/// Rewrites one message's count from the store. Knows nothing about what this
/// process has posted, which is what makes it work after a restart.
async fn rewrite(http: &Http, channel: u64, message: u64) {
    let Some(db) = store::db() else { return };
    let (found, counts) = {
        let conn = db.lock();
        (store::post(&conn, message).ok().flatten(), store::counts(&conn))
    };
    let Some(post) = found else {
        tracing::warn!("signup: message {} is not in the store, so its count was left alone", message);
        return;
    };
    let text = message_text(&post.title, &post.body, counts);
    let edit = EditMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = ChannelId::new(channel).edit_message(http, MessageId::new(message), edit).await {
        tracing::warn!("signup: the count on message {} was not updated: {}", message, err);
    }
}

/// Asks for the count on one message to be brought up to date, now or shortly.
fn refresh(ctx: &Context, channel: u64, message: u64) {
    let plan = TICKERS.lock().entry(message).or_default().click(now_ms(), EDIT_GAP.as_millis() as i64);
    match plan {
        Plan::Nothing => {}
        Plan::Now => {
            let http = ctx.http.clone();
            tokio::spawn(async move { rewrite(&http, channel, message).await });
        }
        Plan::After(ms) => {
            let http = ctx.http.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms.max(0) as u64)).await;
                TICKERS.lock().entry(message).or_default().fired(now_ms());
                rewrite(&http, channel, message).await;
            });
        }
    }
}

// --- the buttons -------------------------------------------------------------

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.as_str();
    let Some(pressed) = (match id {
        ID_IN => Some(Answer::In),
        ID_OUT => Some(Answer::Out),
        _ => None,
    }) else {
        return;
    };
    let Some(db) = store::db() else {
        let _ = component.create_response(&ctx.http, whisper("The sign-up sheet isn't available right now. Tell a mod.")).await;
        tracing::error!("signup: a press arrived with no store open, so it was lost");
        return;
    };
    let Some(guild) = component.guild_id else {
        let _ = component.create_response(&ctx.http, whisper("This only works in the server.")).await;
        return;
    };
    let user = component.user.id.get();
    let name = component.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| component.user.name.clone());

    // Judged per press rather than cached: a mod who fills the setting in or
    // drags the role down should not have to restart the bot for it to take.
    let state = role_state(ctx, guild).await;
    let role = state.role().map(RoleId::new);
    let holder = role.map(|role| GuildRole { ctx, guild, role });
    let wardrobe: Option<&dyn Wardrobe> = holder.as_ref().map(|h| h as &dyn Wardrobe);

    let result = press(db, wardrobe, user, &name, pressed, Utc::now().timestamp()).await;
    if let Some(why) = result.note {
        tracing::warn!("signup: {}", why);
        if let Some(extra) = state.warning() {
            tracing::warn!("signup: {}", extra);
        }
    }
    let _ = component.create_response(&ctx.http, whisper(result.text)).await;

    // Only a press that moved the sheet is worth an edit.
    if result.changed {
        refresh(ctx, component.channel_id.get(), component.message.id.get());
    }
}

// --- /signup -----------------------------------------------------------------

/// A file put on the command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub url: String,
    pub filename: String,
    pub content_type: Option<String>,
}

/// What `/signup` is about to post.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub title: String,
    pub body: String,
    /// The posters, in the order they were given.
    pub images: Vec<Attached>,
}

/// A slash command option is one line, so `\n` typed in the box becomes a real
/// line break — otherwise a body could never have paragraphs.
fn unescape(raw: &str) -> String {
    raw.replace("\\n", "\n").replace("\r\n", "\n").trim().to_string()
}

/// Works out what to post, and refuses in words rather than posting something
/// wrong. Pure: everything awkward about the command is tested here.
pub fn compose(title: &str, body: &str, attached: &[Attached]) -> Result<Draft, String> {
    let title = unescape(title);
    let body = unescape(body);
    if title.is_empty() {
        return Err("The title was empty, so nothing was posted.".into());
    }
    if body.is_empty() {
        return Err("The body was empty, so nothing was posted.".into());
    }
    if title.chars().count() + body.chars().count() > TEXT_ROOM {
        return Err(format!(
            "That's {} characters and Discord takes about {} in one message. Shorten it and try again.",
            title.chars().count() + body.chars().count(),
            TEXT_ROOM
        ));
    }
    let bad: Vec<String> = attached
        .iter()
        .filter(|a| !super::imagefx_recent::looks_like_an_image(&a.filename, a.content_type.as_deref()))
        .map(|a| a.filename.clone())
        .collect();
    if !bad.is_empty() {
        return Err(format!("These aren't pictures, so nothing was posted: {}. PNG, JPG, WebP or GIF only.", bad.join(", ")));
    }
    if attached.len() > MAX_IMAGES {
        return Err(format!("{} posters is more than the {} this takes. Drop one and try again.", attached.len(), MAX_IMAGES));
    }
    Ok(Draft { title, body, images: attached.to_vec() })
}

pub fn builder() -> CreateCommand {
    CreateCommand::new("signup")
        .description("admin only: post a sign-up message with Yes/No buttons that hand out the server games role")
        .add_option(
            CreateCommandOption::new(CommandOptionType::Channel, "channel", "where to post it")
                .required(true)
                .channel_types(vec![serenity::all::ChannelType::Text, serenity::all::ChannelType::News]),
        )
        .add_option(CreateCommandOption::new(CommandOptionType::String, "title", "the bold first line").required(true).max_length(200))
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "body", "what it says - type \\n where you want a new line")
                .required(true)
                .max_length(1800),
        )
        .add_option(CreateCommandOption::new(CommandOptionType::Attachment, "poster", "a picture to post with it"))
        .add_option(CreateCommandOption::new(CommandOptionType::Attachment, "poster2", "a second picture"))
        .add_option(CreateCommandOption::new(CommandOptionType::Attachment, "poster3", "a third picture"))
}

fn option<'a>(command: &'a CommandInteraction, name: &str) -> Option<&'a CommandDataOptionValue> {
    command.data.options.iter().find(|o| o.name == name).map(|o| &o.value)
}

fn text_option(command: &CommandInteraction, name: &str) -> String {
    match option(command, name) {
        Some(CommandDataOptionValue::String(v)) => v.clone(),
        _ => String::new(),
    }
}

/// The posters put on the command, in the order the options are declared.
fn attachments(command: &CommandInteraction) -> Vec<Attached> {
    ["poster", "poster2", "poster3"]
        .iter()
        .filter_map(|name| match option(command, name) {
            Some(CommandDataOptionValue::Attachment(id)) => command.data.resolved.attachments.get(id).map(|a| Attached {
                url: a.url.clone(),
                filename: a.filename.clone(),
                content_type: a.content_type.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// `/signup` — the owner's own words, in the channel they name, with the buttons.
pub async fn command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only bot admins can do that.")).await;
        return;
    }
    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("The sign-up store isn't open, so the buttons would not work. Restart the bot.")).await;
        return;
    };
    let Some(channel) = (match option(command, "channel") {
        Some(CommandDataOptionValue::Channel(id)) => Some(id.get()),
        _ => None,
    }) else {
        let _ = command.create_response(&ctx.http, whisper("Name the channel to post in.")).await;
        return;
    };
    let draft = match compose(&text_option(command, "title"), &text_option(command, "body"), &attachments(command)) {
        Ok(draft) => draft,
        Err(why) => {
            let _ = command.create_response(&ctx.http, whisper(why)).await;
            return;
        }
    };

    // Fetching the posters takes seconds, so the reply is deferred first.
    let _ = command.defer_ephemeral(&ctx.http).await;

    let mut files = Vec::new();
    let mut lost = Vec::new();
    for image in &draft.images {
        match super::imagefx_fetch::get(&image.url, IMAGE_CAP, Duration::from_secs(20)).await {
            Ok(bytes) => files.push(CreateAttachment::bytes(bytes, image.filename.clone())),
            Err(err) => {
                tracing::warn!("signup: the poster {} was not fetched: {}", image.filename, err.plainly());
                lost.push(image.filename.clone());
            }
        }
    }

    let counts = {
        let conn = db.lock();
        store::counts(&conn)
    };
    let mut post = CreateMessage::new()
        .content(message_text(&draft.title, &draft.body, counts))
        .components(vec![buttons()])
        .allowed_mentions(CreateAllowedMentions::new());
    for file in files {
        post = post.add_file(file);
    }

    let sent = match ChannelId::new(channel).send_message(&ctx.http, post).await {
        Ok(message) => message,
        Err(err) => {
            let _ = command
                .edit_response(
                    &ctx.http,
                    EditInteractionResponse::new().content(format!("<#{}> wouldn't take it: {}", channel, err)),
                )
                .await;
            return;
        }
    };

    {
        let conn = db.lock();
        if let Err(err) = store::add_post(
            &conn,
            &Post {
                message_id: sent.id.get(),
                channel_id: channel,
                posted_by: command.user.id.get(),
                posted_ts: Utc::now().timestamp(),
                title: draft.title.clone(),
                body: draft.body.clone(),
                images: draft.images.len() as i64,
            },
        ) {
            // The buttons still work; only the live count would stop moving.
            tracing::error!("signup: message {} was not written to the store: {}", sent.id.get(), err);
        }
    }
    let _ = control::log_change("signup:post", None, Some(&draft.title), command.user.id.get());

    let mut reply = format!("Posted in <#{}>. {}", channel, sent.link());
    if !lost.is_empty() {
        reply.push_str(&format!("\nThese posters wouldn't download, so they aren't on it: {}.", lost.join(", ")));
    }
    if let Some(warning) = role_state(ctx, command.guild_id.unwrap_or_default()).await.warning() {
        reply.push_str(&format!("\n⚠️ {}", warning));
    }
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(reply)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake role list: three roles, the bot wearing the middle one.
    fn roles() -> Vec<RoleFacts> {
        vec![
            RoleFacts { id: 1, name: "@everyone".into(), position: 0, managed: false },
            RoleFacts { id: 2, name: "server games".into(), position: 3, managed: false },
            RoleFacts { id: 3, name: "Loduchand".into(), position: 7, managed: true },
            RoleFacts { id: 4, name: "Admin".into(), position: 20, managed: false },
        ]
    }

    // --- the role -------------------------------------------------------------

    #[test]
    fn a_role_below_the_bot_can_be_handed_out() {
        let v = verdict(Some(2), &roles(), Some(7));
        assert_eq!(v, Verdict::Fine { id: "2".into(), name: "server games".into() });
        assert!(v.usable());
        assert_eq!(v.warning(), None);
        assert_eq!(v.role(), Some(2));
    }

    #[test]
    fn an_empty_setting_is_said_plainly_and_is_never_an_error() {
        let v = verdict(None, &roles(), Some(7));
        assert_eq!(v, Verdict::Unset);
        assert!(!v.usable() && v.role().is_none());
        let why = v.warning().expect("a warning");
        assert!(why.contains(ROLE_KEY), "{why}");
        assert!(why.contains("written down"), "the log says the press is still recorded: {why}");
    }

    #[test]
    fn a_setting_pointing_at_no_role_says_so() {
        let v = verdict(Some(999), &roles(), Some(7));
        assert_eq!(v, Verdict::Missing { id: "999".into() });
        assert!(v.warning().unwrap().contains("999"));
    }

    /// The case that quietly breaks role bots: the role sits above the bot's own.
    #[test]
    fn a_role_the_bot_cannot_manage_is_named_with_the_fix() {
        let v = verdict(Some(4), &roles(), Some(7));
        assert_eq!(v, Verdict::TooHigh { id: "4".into(), name: "Admin".into(), position: 20, bot_position: 7 });
        assert!(!v.usable());
        let why = v.warning().unwrap();
        assert!(why.contains("Server Settings"), "the log says what to do about it: {why}");
        assert!(why.contains("20") && why.contains("7"), "and both positions: {why}");
        // A tie is not manageable either: Discord's own check is strictly greater.
        assert!(matches!(verdict(Some(2), &roles(), Some(3)), Verdict::TooHigh { .. }));
        // And not knowing where the bot stands is treated as "can't".
        assert!(matches!(verdict(Some(2), &roles(), None), Verdict::TooHigh { .. }));
    }

    #[test]
    fn a_role_discord_manages_can_never_be_given() {
        let v = verdict(Some(3), &roles(), Some(30));
        assert_eq!(v, Verdict::Managed { id: "3".into(), name: "Loduchand".into() });
        assert!(!v.usable());
    }

    // --- pressing -------------------------------------------------------------

    /// A wardrobe that writes down what it was asked, and can be told to fail.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<(u64, bool)>>,
        moved: AtomicUsize,
        fail: bool,
    }

    #[async_trait::async_trait]
    impl Wardrobe for Fake {
        async fn wear(&self, user: u64, on: bool) -> anyhow::Result<bool> {
            self.calls.lock().push((user, on));
            if self.fail {
                return Err(anyhow::anyhow!("Missing Permissions"));
            }
            self.moved.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        }
    }

    fn sheet() -> Mutex<Connection> {
        Mutex::new(store::open_memory().expect("store"))
    }

    #[tokio::test]
    async fn yes_gives_the_role_and_writes_them_down() {
        let db = sheet();
        let fake = Fake::default();
        let out = press(&db, Some(&fake), 11, "Zoya", Answer::In, 500).await;
        assert_eq!(out.text, IN_REPLY);
        assert_eq!(out.counts, (1, 0));
        assert!(out.changed && out.role_moved && out.note.is_none());
        assert_eq!(*fake.calls.lock(), vec![(11, true)]);
        let e = store::answer_of(&db.lock(), 11).unwrap().unwrap();
        assert_eq!((e.answer, e.name.as_str(), e.worn, e.first_ts), (Answer::In, "Zoya", true, 500));
    }

    #[tokio::test]
    async fn no_takes_the_role_off_again() {
        let db = sheet();
        let fake = Fake::default();
        press(&db, Some(&fake), 11, "Zoya", Answer::In, 500).await;
        let out = press(&db, Some(&fake), 11, "Zoya", Answer::Out, 900).await;
        assert_eq!(out.text, OUT_REPLY);
        assert_eq!(out.counts, (0, 1));
        assert!(out.changed);
        assert_eq!(*fake.calls.lock(), vec![(11, true), (11, false)], "taken off, not left on");
        let e = store::answer_of(&db.lock(), 11).unwrap().unwrap();
        assert_eq!((e.answer, e.worn, e.first_ts, e.ts), (Answer::Out, false, 500, 900), "the first answer's day stands");
    }

    /// Pressing the same button twice is a sentence, not an error, and Discord is
    /// not asked a second time.
    #[tokio::test]
    async fn the_same_button_twice_is_told_so_and_asks_discord_once() {
        let db = sheet();
        let fake = Fake::default();
        press(&db, Some(&fake), 11, "Zoya", Answer::In, 500).await;
        let again = press(&db, Some(&fake), 11, "Zoya", Answer::In, 600).await;
        assert_eq!(again.text, IN_AGAIN);
        assert!(!again.changed, "the count did not move, so the message is not edited");
        assert_eq!(again.counts, (1, 0));
        assert_eq!(fake.calls.lock().len(), 1, "the role was not asked for twice");

        press(&db, Some(&fake), 12, "Kabir", Answer::Out, 700).await;
        let out_again = press(&db, Some(&fake), 12, "Kabir", Answer::Out, 800).await;
        assert_eq!(out_again.text, OUT_AGAIN);
        assert!(!out_again.changed);
        assert_eq!(out_again.counts, (1, 1));
    }

    /// No role setting: the press still counts, and the log says why.
    #[tokio::test]
    async fn with_no_role_the_press_is_still_recorded_and_the_log_says_so() {
        let db = sheet();
        let out = press(&db, None, 11, "Zoya", Answer::In, 500).await;
        assert!(out.text.starts_with(IN_REPLY), "{}", out.text);
        assert!(out.text.contains("a mod still has to give you the role"), "{}", out.text);
        assert_eq!(out.counts, (1, 0));
        assert!(out.changed && !out.role_moved);
        let note = out.note.expect("a line for the log");
        assert!(note.contains("written down") && note.contains("no games role"), "{note}");
        let e = store::answer_of(&db.lock(), 11).unwrap().unwrap();
        assert_eq!((e.answer, e.worn), (Answer::In, false), "on the list, without the role");

        // A no needs no role and says nothing about one.
        let out = press(&db, None, 12, "Kabir", Answer::Out, 600).await;
        assert_eq!(out.text, OUT_REPLY);
        assert!(out.note.is_none());
    }

    /// A role the bot can't manage: Discord refuses, and the sheet keeps them.
    #[tokio::test]
    async fn a_refused_role_is_logged_and_the_member_is_kept_on_the_list() {
        let db = sheet();
        let fake = Fake { fail: true, ..Fake::default() };
        let out = press(&db, Some(&fake), 11, "Zoya", Answer::In, 500).await;
        assert!(out.text.starts_with(IN_REPLY) && out.text.contains("a mod still has to"), "{}", out.text);
        assert_eq!(out.counts, (1, 0), "the count moves whether or not the role did");
        assert!(!out.role_moved);
        assert!(out.note.unwrap().contains("Missing Permissions"));
        assert!(!store::answer_of(&db.lock(), 11).unwrap().unwrap().worn);
        assert_eq!(store::unworn(&db.lock()), 1, "and the panel has something to warn about");
    }

    /// Filling the setting in later heals a member recorded without the role:
    /// their next press is tried again rather than skipped as a double-click.
    #[tokio::test]
    async fn a_member_recorded_without_the_role_is_retried_once_it_works() {
        let db = sheet();
        press(&db, None, 11, "Zoya", Answer::In, 500).await;
        let fake = Fake::default();
        let out = press(&db, Some(&fake), 11, "Zoya", Answer::In, 900).await;
        assert_eq!(out.text, IN_AGAIN, "they are told they were already in");
        assert_eq!(*fake.calls.lock(), vec![(11, true)], "but the role is tried again");
        assert!(store::answer_of(&db.lock(), 11).unwrap().unwrap().worn);
        assert_eq!(store::unworn(&db.lock()), 0);
    }

    // --- the live count -------------------------------------------------------

    #[test]
    fn the_count_reads_as_a_sentence_at_every_size() {
        assert_eq!(count_line((0, 0)), "*Nobody has answered yet.*");
        assert_eq!(count_line((1, 0)), "**1 in**");
        assert_eq!(count_line((142, 0)), "**142 in**");
        assert_eq!(count_line((142, 3)), "**142 in** · 3 out");
        assert_eq!(count_line((0, 2)), "**0 in** · 2 out");
        let text = message_text("Next month", "the mods have an idea — would you be up for it?", (142, 3));
        assert_eq!(text, "**Next month**\n\nthe mods have an idea — would you be up for it?\n\n**142 in** · 3 out");
        // Rewriting with a new count changes only the last line.
        let later = message_text("Next month", "the mods have an idea — would you be up for it?", (143, 3));
        assert_eq!(text.rsplit_once('\n').unwrap().0, later.rsplit_once('\n').unwrap().0);
    }

    /// A rush of clicks becomes a handful of edits, not one each.
    #[test]
    fn many_clicks_become_few_edits() {
        let gap = EDIT_GAP.as_millis() as i64;
        let mut ticker = Ticker::default();
        let mut edits = 0;
        // 200 clicks over 20 seconds, 100ms apart. Each scheduled edit fires
        // when its wait is up, exactly as the spawned task does.
        let mut due: Option<i64> = None;
        let start = 1_000_000;
        for step in 0..200 {
            let now = start + step * 100;
            if let Some(at) = due.filter(|at| *at <= now) {
                ticker.fired(at);
                edits += 1;
                due = None;
            }
            match ticker.click(now, gap) {
                Plan::Now => edits += 1,
                Plan::After(ms) => due = Some(now + ms),
                Plan::Nothing => {}
            }
        }
        if let Some(at) = due {
            ticker.fired(at);
            edits += 1;
        }
        assert!(edits <= 6, "20 seconds of clicking cost {} edits, which is more than one per {:?}", edits, EDIT_GAP);
        assert!(edits >= 4, "but the count must still move: only {} edits in 20 seconds", edits);
    }

    #[test]
    fn the_first_click_edits_at_once_and_the_next_one_waits() {
        let mut ticker = Ticker::default();
        assert_eq!(ticker.click(10_000, 5_000), Plan::Now, "a quiet message is edited straight away");
        assert_eq!(ticker.click(11_000, 5_000), Plan::After(4_000), "the next one waits out the gap");
        assert_eq!(ticker.click(11_500, 5_000), Plan::Nothing, "and the ones after it fold into that edit");
        assert_eq!(ticker.click(14_900, 5_000), Plan::Nothing);
        ticker.fired(15_000);
        assert_eq!(ticker.click(15_100, 5_000), Plan::After(4_900), "the gap starts again from the edit that went out");
    }

    // --- /signup --------------------------------------------------------------

    fn png(name: &str) -> Attached {
        Attached { url: format!("https://cdn.discordapp.com/{}", name), filename: name.into(), content_type: Some("image/png".into()) }
    }

    #[test]
    fn the_command_takes_a_title_a_body_and_up_to_three_posters() {
        let draft = compose("Next month", "line one\\nline two", &[png("a.png"), png("b.jpg")]).unwrap();
        assert_eq!(draft.title, "Next month");
        assert_eq!(draft.body, "line one\nline two", "a typed \\n becomes a real line break");
        assert_eq!(draft.images.len(), 2);
        // Three is the limit, four is refused in words.
        assert!(compose("T", "B", &[png("a.png"), png("b.png"), png("c.png")]).is_ok());
        let err = compose("T", "B", &[png("a.png"), png("b.png"), png("c.png"), png("d.png")]).unwrap_err();
        assert!(err.contains("more than the 3"), "{err}");
    }

    #[test]
    fn the_command_refuses_rather_than_posting_something_wrong() {
        assert!(compose("", "B", &[]).unwrap_err().contains("title was empty"));
        assert!(compose("  ", "B", &[]).unwrap_err().contains("title was empty"));
        assert!(compose("T", "   ", &[]).unwrap_err().contains("body was empty"));
        let long = "x".repeat(2000);
        assert!(compose("T", &long, &[]).unwrap_err().contains("Shorten it"));
        // A file that isn't a picture is named, and nothing is posted.
        let pdf = Attached { url: "https://cdn/x.pdf".into(), filename: "poster.pdf".into(), content_type: Some("application/pdf".into()) };
        let err = compose("T", "B", &[png("a.png"), pdf]).unwrap_err();
        assert!(err.contains("poster.pdf") && err.contains("PNG"), "{err}");
    }

    /// The buttons are dispatched by these ids and nothing else, which is what
    /// makes yesterday's message work after a restart.
    #[test]
    fn the_buttons_are_recognised_by_their_ids_alone() {
        assert!(owns_component(ID_IN) && owns_component(ID_OUT));
        assert!(!owns_component("signup") && !owns_component("trade:1") && !owns_component(""));
        assert_eq!((ID_IN, ID_OUT), ("signup:in", "signup:out"));
        // And the two labels the owner asked for are on them.
        let CreateActionRow::Buttons(row) = buttons() else { panic!("a row of buttons") };
        let drawn = serde_json::to_string(&row).unwrap();
        assert!(drawn.contains("Yes, I'm in") && drawn.contains("No, I'm out"), "{drawn}");
        assert!(drawn.contains(ID_IN) && drawn.contains(ID_OUT));
    }
}
