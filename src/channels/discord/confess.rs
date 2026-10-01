//! Anonymous confessions: the panel, the modals, a mod's decision, and the
//! numbered post that comes out of it.
//!
//! This replaces a third-party bot the server had been using, and it is built
//! to the flow the members already know:
//!
//! 1. A panel message sits in the confessions channel with **Submit a
//!    confession** and **Submit a reply** under it. Pressing either opens a
//!    modal: a confession is just the text, a reply is a confession number and
//!    the text.
//! 2. Nothing is posted publicly by submitting. The text goes to the review
//!    channel as an embed with the submitter on it — name, mention, id, how old
//!    the account is, when they joined, and how many of theirs have been
//!    approved and rejected before — and two buttons, **Approve** and
//!    **Reject**. Reject asks for an optional reason.
//! 3. Approving posts it in the confessions channel, anonymous and numbered,
//!    and opens a thread on that message with the same name, which is where the
//!    conversation about it happens — the channel itself stays readable.
//!    Rejecting posts nothing anywhere public, ever. Either way the log channel
//!    gets an entry naming the submitter and the mod who decided.
//! 4. The panel is moved back to the bottom after each post, because that is
//!    the bug that killed the old bot: its panel ended up fourteen messages
//!    deep and nobody could find it.
//!
//! Four things this is careful about, all of them because of what the feature
//! handles.
//!
//! **The text never reaches the AI.** The three channels are listed as
//! sensitive ([`sensitive_channels`]), which is the same list #safe-corner is
//! on, so the message log never records them — and the member notes, the daily
//! topic pass, the deep dive, the Kalesh pages and the weekly posts scan all
//! read the message log. The weekly scan can also be pointed at channels by
//! hand, so it filters these out of its own list too.
//!
//! **Anonymity is absolute in public.** The posted message carries a number and
//! the words and nothing else: no name, no mention, no avatar, no footer, no
//! thread. Mods see the submitter in review and in the log, which is how they
//! moderate, and the panel says plainly that they can.
//!
//! **The panel the repost deletes is only ever one this bot posted.** It is
//! matched by a message id this bot wrote into its own store — never by author,
//! never by content — so the old bot's panels, and every confession already in
//! that channel, are left exactly where they are.
//!
//! **The record is written before Discord is asked for anything.** A mod has to
//! be able to answer "who sent #457" a month later, and a rejection has to be
//! provably unposted.

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::Connection;
use serenity::all::{
    ActionRowComponent, AutoArchiveDuration, ButtonStyle, ChannelId, ComponentInteraction, Context, CreateActionRow,
    CreateAllowedMentions, CreateButton, CreateEmbed, CreateEmbedFooter, CreateInputText, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateModal, CreateThread, EditMessage, GuildId, Http,
    InputTextStyle, Message, MessageId, ModalInteraction, UserId,
};

use super::confess_store::{self as store, Confession, Filter, Kind, New, Status};
use super::control;

// --- the custom ids ----------------------------------------------------------
//
// Dispatch is by these strings and nothing else, so a panel posted before a
// restart still answers afterwards and a review embed from last week can still
// be approved.

pub const ID_NEW: &str = "confess:new";
pub const ID_REPLY: &str = "confess:reply";
pub const ID_WHO: &str = "confess:who";
pub const MODAL_NEW: &str = "confessform:new";
pub const MODAL_REPLY: &str = "confessform:reply";
pub const MODAL_WHO: &str = "confessform:who";
/// `confess:ok:<number>` and `confess:no:<number>`.
pub const ID_APPROVE: &str = "confess:ok:";
pub const ID_REJECT: &str = "confess:no:";
/// `confessform:no:<number>` — the optional reason for a rejection.
pub const MODAL_REJECT: &str = "confessform:no:";

/// The field ids inside the modals.
const FIELD_TEXT: &str = "text";
const FIELD_NUMBER: &str = "number";
const FIELD_REASON: &str = "reason";

/// At most one panel repost per channel per this long, so twenty confessions
/// approved in a row is a handful of reposts rather than twenty.
pub const PANEL_GAP: Duration = Duration::from_secs(15);

/// Discord's own ceiling on a message, less room for the heading.
pub const BODY_CEILING: usize = 1800;

/// Whether a custom id belongs to this feature.
pub fn owns_component(id: &str) -> bool {
    id == ID_NEW || id == ID_REPLY || id == ID_WHO || id.starts_with(ID_APPROVE) || id.starts_with(ID_REJECT)
}

/// Whether a modal's custom id belongs to this feature.
pub fn owns_modal(id: &str) -> bool {
    id == MODAL_NEW || id == MODAL_REPLY || id == MODAL_WHO || id.starts_with(MODAL_REJECT)
}

// --- settings ----------------------------------------------------------------

pub fn enabled() -> bool {
    control::on("VIZIER_CONFESS", true)
}

/// Where approved confessions are posted, and where the panel lives.
pub fn channel() -> Option<u64> {
    Some(control::number("VIZIER_CONFESS_CHANNEL", DEFAULT_CHANNEL)).filter(|id| *id != 0)
}

/// Where mods approve or reject first. Nothing is ever posted publicly without
/// a decision here.
pub fn review_channel() -> Option<u64> {
    Some(control::number("VIZIER_CONFESS_REVIEW_CHANNEL", DEFAULT_REVIEW)).filter(|id| *id != 0)
}

/// The audit trail: one entry per decision.
pub fn log_channel() -> Option<u64> {
    Some(control::number("VIZIER_CONFESS_LOG_CHANNEL", DEFAULT_LOG)).filter(|id| *id != 0)
}

/// Where the series starts when the store is empty: the old bot had reached
/// #458, so ours begins at #459 and runs alongside what is already there.
pub fn first_number() -> i64 {
    control::number("VIZIER_CONFESS_NEXT_NUMBER", 459).clamp(1, 1_000_000_000) as i64
}

pub const DEFAULT_CHANNEL: u64 = 1527318601126907924;
pub const DEFAULT_REVIEW: u64 = 1527321274064437528;
pub const DEFAULT_LOG: u64 = 1527320955763036180;

/// The three channels this feature owns. Listed as sensitive, which is how the
/// text is kept out of every AI path — see [`sensitive_channels`].
pub fn channels() -> Vec<u64> {
    let mut out: Vec<u64> = [channel(), review_channel(), log_channel()].into_iter().flatten().collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The channels no analysis may read, as the insights list wants them. The
/// public channel is on it because a confession is a confession wherever it
/// sits; the review and log channels are on it because they carry the text AND
/// the name together, which is the one pairing that must never be summarised.
///
/// This is the whole of the AI exclusion: `msglog::never_logged` reads this
/// list, so these channels are never recorded at all, and the member notes, the
/// topic pass, the deep dive and the Kalesh pages all read the message log.
pub fn sensitive_channels() -> Vec<u64> {
    channels()
}

/// Whether an approved confession gets a thread of its own for the
/// conversation. On: that is how the server already reads them.
pub fn threads_on() -> bool {
    control::on("VIZIER_CONFESS_THREADS", true)
}

/// How long a quiet thread waits before Discord archives it. Three days by
/// default, because a confession's thread is still going days later and a
/// shorter wait would archive it mid-conversation. Discord takes only 60, 1440,
/// 4320 or 10080 minutes, so anything else is rounded to the nearest one it
/// accepts rather than refused.
pub fn archive_minutes() -> u16 {
    nearest_archive(control::number("VIZIER_CONFESS_THREAD_ARCHIVE_MINUTES", 4320) as i64)
}

/// The nearest duration Discord actually accepts.
pub fn nearest_archive(minutes: i64) -> u16 {
    const ALLOWED: [i64; 4] = [60, 1440, 4320, 10080];
    ALLOWED.into_iter().min_by_key(|m| (m - minutes).abs()).unwrap_or(1440) as u16
}

pub fn archive_duration(minutes: u16) -> AutoArchiveDuration {
    match minutes {
        60 => AutoArchiveDuration::OneHour,
        4320 => AutoArchiveDuration::ThreeDays,
        10080 => AutoArchiveDuration::OneWeek,
        _ => AutoArchiveDuration::OneDay,
    }
}

pub fn limits() -> Limits {
    Limits {
        min: control::number("VIZIER_CONFESS_MIN_LENGTH", 10).clamp(1, 1000) as usize,
        max: (control::number("VIZIER_CONFESS_MAX_LENGTH", 1500).clamp(1, BODY_CEILING as u64) as usize),
        cooldown: control::number("VIZIER_CONFESS_COOLDOWN_MINUTES", 10).clamp(0, 10_080) as i64 * 60,
        blocked: control::ids("VIZIER_CONFESS_BLOCKED"),
    }
}

/// The guards one submission has to get past.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    /// The fewest characters a confession may be, once pings are stripped.
    pub min: usize,
    pub max: usize,
    /// Seconds a member must wait between submissions. 0 is no cooldown.
    pub cooldown: i64,
    /// Members who may not submit at all.
    pub blocked: Vec<u64>,
}

// --- cleaning the text -------------------------------------------------------

/// Takes the bite out of every ping. @everyone and @here lose their @, a
/// mention of a member or a role becomes a word. Nothing is deleted, because a
/// confession about somebody is still a confession: it just cannot ring their
/// phone.
///
/// The post also goes out with an empty allowed-mentions list, so even a form
/// of ping this misses cannot notify anybody. This exists so the *text* is
/// clean too — the review embed and the log quote it, and so does the panel
/// page.
pub fn strip_pings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '<' {
            // <@123>, <@!123>, <@&123> — a mention Discord would resolve.
            let rest: String = bytes[i..].iter().take(32).collect();
            if let Some(end) = rest.find('>') {
                let inner = &rest[1..end];
                let what = match inner.strip_prefix('@') {
                    Some(r) if r.starts_with('&') => r[1..].chars().all(|c| c.is_ascii_digit()).then_some("@role"),
                    Some(r) => {
                        let digits = r.strip_prefix('!').unwrap_or(r);
                        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some("@member")
                    }
                    None => None,
                };
                if let Some(word) = what {
                    out.push_str(word);
                    i += rest[..=end].chars().count();
                    continue;
                }
            }
        }
        if bytes[i] == '@' {
            let rest: String = bytes[i..].iter().take(9).collect::<String>().to_ascii_lowercase();
            if rest.starts_with("@everyone") {
                out.push_str("everyone");
                i += 9;
                continue;
            }
            if rest.starts_with("@here") {
                out.push_str("here");
                i += 5;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// One submission, cleaned and judged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clean {
    pub kind: Kind,
    /// The confession a reply answers.
    pub answers: Option<i64>,
    pub body: String,
    /// True when a ping was taken out, so the review embed can say so.
    pub pings_stripped: bool,
}

pub const BLOCKED_SAID: &str = "You can't submit confessions. If you think that's wrong, message a mod.";
pub const EMPTY_SAID: &str = "There was nothing in that but pings. Write something and try again.";
pub const INVITE_SAID: &str = "Invite links aren't allowed in a confession. Take it out and try again.";
pub const NO_NUMBER_SAID: &str = "Put the confession's number in the first box — just the digits, like 457.";
pub const NO_SUCH_SAID: &str = "There's no confession with that number here. Check it and try again.";
pub const NOT_YET_SAID: &str = "That confession hasn't been posted, so there's nothing to reply to yet.";

/// Everything one submission must get past, with no Discord and no clock of its
/// own, so every awkward case is a test rather than a thing to try on a live
/// server.
///
/// `last_ts` is when this member last submitted anything, `target` the
/// confession a reply says it answers. The order matters: who may submit at
/// all, then how often, then what they are replying to, then the words.
pub fn vet(
    limits: &Limits,
    user: u64,
    last_ts: Option<i64>,
    now: i64,
    kind: Kind,
    number_raw: &str,
    body_raw: &str,
    target: Option<&Confession>,
) -> Result<Clean, String> {
    if limits.blocked.contains(&user) {
        return Err(BLOCKED_SAID.to_string());
    }
    if limits.cooldown > 0 {
        if let Some(last) = last_ts {
            let left = limits.cooldown - (now - last);
            if left > 0 {
                return Err(format!("You've just sent one. Try again in {}.", wait_words(left)));
            }
        }
    }

    let answers = match kind {
        Kind::Confession => None,
        Kind::Reply => {
            let digits = number_raw.trim().trim_start_matches('#').trim();
            let Ok(number) = digits.parse::<i64>() else { return Err(NO_NUMBER_SAID.to_string()) };
            if number <= 0 {
                return Err(NO_NUMBER_SAID.to_string());
            }
            match target {
                None => return Err(NO_SUCH_SAID.to_string()),
                // A pending or rejected one is not public, and saying which it
                // is would leak a mod's decision to whoever typed the number.
                Some(c) if c.status != Status::Approved || c.posted_message == 0 => {
                    return Err(NOT_YET_SAID.to_string());
                }
                Some(c) if c.number != number => return Err(NO_SUCH_SAID.to_string()),
                Some(_) => Some(number),
            }
        }
    };

    if super::automod::has_invite(body_raw) {
        return Err(INVITE_SAID.to_string());
    }
    let body = strip_pings(body_raw).trim().to_string();
    let pings_stripped = body != body_raw.trim();
    if body.is_empty() {
        return Err(EMPTY_SAID.to_string());
    }
    let length = body.chars().count();
    if length < limits.min {
        return Err(format!(
            "That's {} characters and a confession has to be at least {}. Say a bit more.",
            length, limits.min
        ));
    }
    if length > limits.max {
        return Err(format!(
            "That's {} characters and the limit is {}. Shorten it by about {} and try again.",
            length,
            limits.max,
            length - limits.max
        ));
    }
    Ok(Clean { kind, answers, body, pings_stripped })
}

/// "3 minutes", "2 hours", "40 seconds" — the wait, as a mod would say it.
pub fn wait_words(seconds: i64) -> String {
    let plural = |n: i64, unit: &str| format!("{} {}{}", n, unit, if n == 1 { "" } else { "s" });
    match seconds {
        s if s < 60 => plural(s.max(1), "second"),
        s if s < 3600 => plural((s + 59) / 60, "minute"),
        s => plural((s + 3599) / 3600, "hour"),
    }
}

// --- what the public sees ----------------------------------------------------

/// The title a confession's own message carries, worded as the members already
/// read them. There is nothing about the submitter in it, and the thread takes
/// the same name, so there is nothing about them there either.
pub fn title(c: &Confession) -> String {
    format!("Anonymous Confession (#{})", c.number)
}

/// The thread's name: the message's own title, and Discord's 100-character cap.
pub fn thread_name(c: &Confession) -> String {
    title(c).chars().take(100).collect()
}

/// The whole public message for a confession: the title in bold, then the
/// words. Nothing about who sent it — not a name, not a mention, not a footer.
pub fn confession_text(c: &Confession) -> String {
    format!("**{}**\n\n{}", title(c), c.body)
}

/// A, B, ... Z, AA, AB — a reply's label inside its confession's thread, so one
/// can be referred to without anybody's name. `index` counts from 1.
pub fn reply_letter(index: i64) -> String {
    let mut n = index.max(1) - 1;
    let mut out = Vec::new();
    loop {
        out.push((b'A' + (n % 26) as u8) as char);
        n = n / 26;
        if n == 0 {
            break;
        }
        n -= 1;
    }
    out.iter().rev().collect()
}

/// An approved reply, as it reads inside the confession's thread: labelled so
/// members can point at it, and anonymous like everything else here. One post
/// per confession and one thread, with the replies inside it — not a second
/// main-channel message.
pub fn reply_text(target: i64, letter: &str, body: &str) -> String {
    format!("**Reply {} to Confession (#{})**\n\n{}", letter, target, body)
}

/// The public text for either kind, given a reply's letter when it is one.
pub fn public_text(c: &Confession, letter: Option<&str>) -> String {
    match (c.kind, c.answers) {
        (Kind::Reply, Some(n)) => reply_text(n, letter.unwrap_or("A"), &c.body),
        // A reply whose target went missing is never posted (see `vet`); if one
        // ever got this far it reads as its own confession rather than lying
        // about which number it answers.
        (Kind::Reply, None) | (Kind::Confession, _) => confession_text(c),
    }
}

/// Whether a thread still has to be opened on this confession. False once one
/// has been, so a second pass can never open a second thread.
pub fn needs_thread(c: &Confession) -> bool {
    c.kind == Kind::Confession && c.status == Status::Approved && c.posted_message != 0 && c.thread_id == 0
}

// --- posting it, and the thread it lives in ----------------------------------

/// Everything this feature asks of Discord once a mod has approved something.
/// Behind a trait so the awkward half — an archived thread, a deleted one, a
/// confession that predates this bot — is tested rather than tried live.
#[async_trait::async_trait]
pub trait Poster: Send + Sync {
    /// A message in the main channel, optionally as a Discord reply to another.
    async fn say_in_channel(&self, channel: u64, text: &str, reply_to: Option<u64>) -> anyhow::Result<u64>;
    /// A message inside a thread. An error means the thread could not be used.
    async fn say_in_thread(&self, thread: u64, text: &str) -> anyhow::Result<u64>;
    /// Unarchives a thread so it can be posted in again.
    async fn revive(&self, thread: u64) -> anyhow::Result<()>;
    /// Opens a thread on a message in the main channel.
    async fn open_thread(&self, channel: u64, message: u64, name: &str, minutes: u16) -> anyhow::Result<u64>;
    /// Deletes one message. Only ever called with a panel id this bot wrote
    /// into its own store — see [`repost_panel`].
    async fn delete(&self, channel: u64, message: u64) -> anyhow::Result<()>;
    /// The panel message, with its buttons. Separate from `say_in_channel`
    /// because only this one carries components.
    async fn post_panel(&self, channel: u64, text: &str) -> anyhow::Result<u64>;
}

/// Where an approved confession ended up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Posted {
    /// 0 when the post itself failed.
    pub message: u64,
    /// The thread opened on it, when one could be.
    pub thread: Option<u64>,
    /// What the log should say, when something needs saying.
    pub notes: Vec<String>,
}

/// Posts a confession and opens its thread. The post comes first and stands on
/// its own: a thread that cannot be opened costs a line in the log and never
/// the confession.
pub async fn post_confession(poster: &dyn Poster, channel: u64, c: &Confession, want_thread: bool, minutes: u16) -> Posted {
    let mut out = Posted::default();
    match poster.say_in_channel(channel, &confession_text(c), None).await {
        Ok(id) => out.message = id,
        Err(err) => {
            out.notes.push(format!("#{} was approved but could not be posted: {}", c.number, err));
            return out;
        }
    }
    if !want_thread {
        return out;
    }
    match poster.open_thread(channel, out.message, &thread_name(c), minutes).await {
        Ok(thread) => out.thread = Some(thread),
        Err(err) => out.notes.push(format!(
            "#{} is posted, but its thread could not be opened ({}) - the confession stands and the conversation will have to happen in the channel",
            c.number, err
        )),
    }
    out
}

/// Where an approved reply ended up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Placed {
    pub message: u64,
    /// The thread it went in, when it went in one.
    pub thread: Option<u64>,
    /// A thread opened here and now, to be written down.
    pub opened: Option<u64>,
    /// True when an archived thread had to be woken up first.
    pub revived: bool,
    /// True when no thread could be used and it went in the main channel.
    pub fell_back: bool,
    pub notes: Vec<String>,
}

/// Puts an approved reply inside its confession's thread.
///
/// Four tries, in the order that keeps the conversation where the members
/// expect it: the thread we know about; the same thread woken up, if it had
/// archived; a thread opened on the confession now, for one that predates this
/// bot or whose thread was deleted; and failing all of that, the main channel,
/// hanging off the confession so the pair is still obvious. Only the last one
/// is a failure, and it is logged as one.
pub async fn place_reply(
    poster: &dyn Poster,
    channel: u64,
    parent: &Confession,
    text: &str,
    minutes: u16,
) -> Placed {
    let mut out = Placed::default();
    if parent.thread_id != 0 {
        match poster.say_in_thread(parent.thread_id, text).await {
            Ok(id) => {
                out.message = id;
                out.thread = Some(parent.thread_id);
                return out;
            }
            Err(err) => {
                // Much the likeliest reason: the thread went quiet and Discord
                // archived it. Waking it up is cheap and keeps the conversation
                // in one place.
                match poster.revive(parent.thread_id).await {
                    Ok(()) => match poster.say_in_thread(parent.thread_id, text).await {
                        Ok(id) => {
                            out.message = id;
                            out.thread = Some(parent.thread_id);
                            out.revived = true;
                            return out;
                        }
                        Err(again) => out
                            .notes
                            .push(format!("the thread on #{} would not take a reply even after waking it: {}", parent.number, again)),
                    },
                    Err(why) => out.notes.push(format!(
                        "the thread on #{} would not take a reply ({}) and would not wake up ({})",
                        parent.number, err, why
                    )),
                }
            }
        }
    }

    if parent.posted_message != 0 {
        match poster.open_thread(channel, parent.posted_message, &thread_name(parent), minutes).await {
            Ok(thread) => match poster.say_in_thread(thread, text).await {
                Ok(id) => {
                    out.message = id;
                    out.thread = Some(thread);
                    out.opened = Some(thread);
                    return out;
                }
                Err(err) => {
                    out.opened = Some(thread);
                    out.notes.push(format!("a new thread was opened on #{} but would not take the reply: {}", parent.number, err));
                }
            },
            Err(err) => out.notes.push(format!("no thread could be opened on #{}: {}", parent.number, err)),
        }
    }

    // The fallback: in the channel, as a Discord reply to the confession, so a
    // reader can still see what it answers.
    let reply_to = (parent.posted_message != 0).then_some(parent.posted_message);
    match poster.say_in_channel(channel, text, reply_to).await {
        Ok(id) => {
            out.message = id;
            out.fell_back = true;
            out.notes.push(format!(
                "a reply to #{} went in the channel rather than its thread, because no thread could be used",
                parent.number
            ));
        }
        Err(err) => out.notes.push(format!("a reply to #{} could not be posted at all: {}", parent.number, err)),
    }
    out
}

/// The real Discord, behind the same trait.
pub struct Live<'a> {
    pub http: &'a Http,
}

#[async_trait::async_trait]
impl Poster for Live<'_> {
    async fn say_in_channel(&self, channel: u64, text: &str, reply_to: Option<u64>) -> anyhow::Result<u64> {
        let mut post = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
        if let Some(id) = reply_to {
            post = post.reference_message((ChannelId::new(channel), MessageId::new(id)));
        }
        Ok(ChannelId::new(channel).send_message(self.http, post).await?.id.get())
    }

    async fn say_in_thread(&self, thread: u64, text: &str) -> anyhow::Result<u64> {
        let post = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
        Ok(ChannelId::new(thread).send_message(self.http, post).await?.id.get())
    }

    async fn revive(&self, thread: u64) -> anyhow::Result<()> {
        ChannelId::new(thread).edit_thread(self.http, serenity::all::EditThread::new().archived(false)).await?;
        Ok(())
    }

    async fn open_thread(&self, channel: u64, message: u64, name: &str, minutes: u16) -> anyhow::Result<u64> {
        let thread = CreateThread::new(name.chars().take(100).collect::<String>())
            .auto_archive_duration(archive_duration(minutes));
        Ok(ChannelId::new(channel).create_thread_from_message(self.http, MessageId::new(message), thread).await?.id.get())
    }

    async fn delete(&self, channel: u64, message: u64) -> anyhow::Result<()> {
        ChannelId::new(channel).delete_message(self.http, MessageId::new(message)).await?;
        Ok(())
    }

    async fn post_panel(&self, channel: u64, text: &str) -> anyhow::Result<u64> {
        let post = CreateMessage::new()
            .content(text)
            .components(vec![panel_buttons()])
            .allowed_mentions(CreateAllowedMentions::new());
        Ok(ChannelId::new(channel).send_message(self.http, post).await?.id.get())
    }
}

/// The panel's words. It says plainly that mods see who submitted, because a
/// member deciding whether to trust this deserves to know that before they
/// type, not after.
pub const PANEL_TITLE: &str = "**Confessions**";
pub const PANEL_BODY: &str = "Press a button below to send something in. Nothing is posted until a mod has read it, \
    and what gets posted carries a number and your words — never your name.\n\n\
    Moderators do see who submitted, for moderation only, and every time one looks it is written down.\n\n\
    -# This panel is always the last message here, so you never have to scroll for it.";

pub fn panel_text() -> String {
    format!("{}\n\n{}", PANEL_TITLE, PANEL_BODY)
}

pub fn panel_buttons() -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(ID_NEW).label("Submit a confession").style(ButtonStyle::Primary),
        CreateButton::new(ID_REPLY).label("Submit a reply").style(ButtonStyle::Secondary),
        CreateButton::new(ID_WHO).label("Who sent it? (mods)").style(ButtonStyle::Secondary),
    ])
}

// --- the modals --------------------------------------------------------------

pub fn new_modal(limits: &Limits) -> CreateModal {
    CreateModal::new(MODAL_NEW, "Submit a confession").components(vec![CreateActionRow::InputText(
        CreateInputText::new(InputTextStyle::Paragraph, "Your confession", FIELD_TEXT)
            .placeholder("Nobody but the mods will ever know this was you.")
            .min_length(limits.min.min(1024) as u16)
            .max_length(limits.max.min(4000) as u16)
            .required(true),
    )])
}

pub fn reply_modal(limits: &Limits) -> CreateModal {
    CreateModal::new(MODAL_REPLY, "Submit a reply").components(vec![
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Short, "Which confession? (its number)", FIELD_NUMBER)
                .placeholder("457")
                .max_length(12)
                .required(true),
        ),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Paragraph, "Your reply", FIELD_TEXT)
                .min_length(limits.min.min(1024) as u16)
                .max_length(limits.max.min(4000) as u16)
                .required(true),
        ),
    ])
}

pub fn reject_modal(number: i64) -> CreateModal {
    CreateModal::new(format!("{}{}", MODAL_REJECT, number), format!("Reject #{}", number)).components(vec![
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Paragraph, "Why? (optional)", FIELD_REASON)
                .placeholder("Only mods ever see this.")
                .max_length(500)
                .required(false),
        ),
    ])
}

pub fn who_modal() -> CreateModal {
    CreateModal::new(MODAL_WHO, "Who sent it?").components(vec![CreateActionRow::InputText(
        CreateInputText::new(InputTextStyle::Short, "Which number?", FIELD_NUMBER)
            .placeholder("457")
            .max_length(12)
            .required(true),
    )])
}

/// One field out of a submitted modal.
pub fn field(modal: &ModalInteraction, id: &str) -> String {
    modal
        .data
        .components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|c| match c {
            ActionRowComponent::InputText(t) if t.custom_id == id => t.value.clone(),
            _ => None,
        })
        .unwrap_or_default()
}

// --- the review embed --------------------------------------------------------

/// What a mod is shown about the submitter. Everything here is theirs to see:
/// it is the whole reason the review channel exists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Submitter {
    pub id: u64,
    pub name: String,
    /// When the Discord account itself was made.
    pub account_ts: Option<i64>,
    /// When they joined this server.
    pub joined_ts: Option<i64>,
    pub approved: i64,
    pub rejected: i64,
    /// True when they can moderate. Said plainly, so a mod cannot approve
    /// their own submission without anybody noticing.
    pub is_mod: bool,
}

/// "2 years old", "3 days old" — an age in the plainest words that are true.
pub fn age_words(then: i64, now: i64) -> String {
    let days = (now - then).max(0) / 86_400;
    match days {
        0 => "today".to_string(),
        1 => "1 day".to_string(),
        d if d < 60 => format!("{} days", d),
        d if d < 730 => format!("{} months", d / 30),
        d => format!("{} years", d / 365),
    }
}

fn stamp(ts: Option<i64>, now: i64) -> String {
    match ts {
        Some(t) => format!("<t:{}:D> ({} ago)", t, age_words(t, now)),
        None => "not known".to_string(),
    }
}

/// The amber of something waiting on a person.
const REVIEW_COLOUR: u32 = 0xE8A33D;
const APPROVED_COLOUR: u32 = 0x3BA55D;
const REJECTED_COLOUR: u32 = 0xED4245;

/// The line that must be impossible to miss: this one is a mod's own.
pub const OWN_WARNING: &str = "⚠️ **The submitter is a moderator.** Get somebody else to decide this one.";

pub fn review_embed(c: &Confession, who: &Submitter, now: i64) -> CreateEmbed {
    let title = match (c.kind, c.answers) {
        (Kind::Reply, Some(n)) => format!("Confession Reply (#{}) · answering #{}", c.number, n),
        (Kind::Reply, None) => format!("Confession Reply (#{})", c.number),
        (Kind::Confession, _) => format!("Confession (#{})", c.number),
    };
    let mut description = String::new();
    if who.is_mod {
        description.push_str(OWN_WARNING);
        description.push_str("\n\n");
    }
    description.push_str(&c.body);
    let mut embed = CreateEmbed::new()
        .title(title)
        .description(description)
        .colour(REVIEW_COLOUR)
        .field("User", format!("{} (<@{}>)", if who.name.is_empty() { "unknown" } else { &who.name }, who.id), true)
        .field("ID", format!("`{}`", who.id), true)
        .field("Account made", stamp(who.account_ts, now), true)
        .field("Joined", stamp(who.joined_ts, now), true)
        .field("Approved before", who.approved.to_string(), true)
        .field("Rejected before", who.rejected.to_string(), true)
        .footer(CreateEmbedFooter::new(format!("#{} · nothing is public until somebody presses Approve", c.number)));
    if c.kind == Kind::Reply {
        if let Some(n) = c.answers {
            embed = embed.field("Answers", format!("Confession #{}", n), true);
        }
    }
    embed
}

pub fn review_buttons(number: i64) -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{}{}", ID_APPROVE, number)).label("Approve").style(ButtonStyle::Success),
        CreateButton::new(format!("{}{}", ID_REJECT, number)).label("Reject").style(ButtonStyle::Danger),
    ])
}

// --- the log entry -----------------------------------------------------------

/// One decision, in the shape the old bot used: the title says what happened
/// and to which number, and the fields are User, ID, Link and Approved By.
/// Mods see the submitter here; this channel is mods-only and is on the
/// sensitive list, so nothing in it ever reaches the AI.
pub fn log_embed(c: &Confession, decided_by_name: &str, guild: u64, link: Option<&str>) -> CreateEmbed {
    let approved = c.status == Status::Approved;
    let what = match (c.kind, approved) {
        (Kind::Confession, true) => format!("Confession Approved (#{})", c.number),
        (Kind::Confession, false) => format!("Confession Rejected (#{})", c.number),
        (Kind::Reply, true) => format!("Confession Reply Approved (#{})", c.number),
        (Kind::Reply, false) => format!("Confession Reply Rejected (#{})", c.number),
    };
    let link_value = link
        .map(String::from)
        .or_else(|| c.link(guild))
        .unwrap_or_else(|| if approved { "not posted".to_string() } else { "never posted".to_string() });
    let mut embed = CreateEmbed::new()
        .title(what)
        .description(c.body.clone())
        .colour(if approved { APPROVED_COLOUR } else { REJECTED_COLOUR })
        .field("User", format!("{} (<@{}>)", if c.user_name.is_empty() { "unknown" } else { &c.user_name }, c.user_id), true)
        .field("ID", format!("`{}`", c.user_id), true)
        .field("Link", link_value, false)
        .field(if approved { "Approved By" } else { "Rejected By" }, format!("{} (<@{}>)", decided_by_name, c.decided_by), true);
    if let Some(n) = c.answers {
        embed = embed.field("Answers", format!("Confession #{}", n), true);
    }
    if !approved && !c.reason.trim().is_empty() {
        embed = embed.field("Reason", c.reason.clone(), false);
    }
    embed
}

// --- moving the panel back to the bottom -------------------------------------

/// What a post should do about the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Move it now.
    Now,
    /// Move it in this many milliseconds.
    After(i64),
    /// Nothing: a move is already on its way and will do the work.
    Nothing,
}

/// One channel's repost budget. Posts arriving inside the gap fold into the one
/// repost already scheduled, so a burst of approvals is a handful of calls to
/// Discord rather than one each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ticker {
    last_ms: i64,
    pending: bool,
}

impl Ticker {
    pub fn posted(&mut self, now_ms: i64, gap_ms: i64) -> Plan {
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

    /// The scheduled repost has gone out.
    pub fn fired(&mut self, now_ms: i64) {
        self.last_ms = now_ms;
        self.pending = false;
    }
}

static TICKERS: LazyLock<Mutex<HashMap<u64, Ticker>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// What one move of the panel did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PanelMove {
    /// The panel that is now at the bottom. 0 when it could not be posted.
    pub posted: u64,
    /// The message that was deleted, when one was. Only ever a panel this bot
    /// wrote into its own store.
    pub deleted: Option<u64>,
    pub notes: Vec<String>,
}

/// Posts the panel at the bottom of the channel and takes the old one down.
///
/// **The old one is the message id this bot wrote into its own store for this
/// channel, and nothing else.** Not the newest message by the bot, not a
/// message that looks like a panel, not anything matched on author or content.
/// The third-party bot this replaces left its own panels and hundreds of
/// confessions in that channel, and none of them are visible here.
///
/// The new panel goes up BEFORE the old one comes down, so a failure in the
/// middle leaves two panels rather than none — a duplicate is untidy, no panel
/// at all is the bug that killed the old bot. A panel somebody deleted by hand
/// simply fails to delete, which costs a line in the log and nothing else.
pub async fn repost_panel(poster: &dyn Poster, db: &Mutex<Connection>, channel: u64, now: i64) -> PanelMove {
    let mut out = PanelMove::default();
    let old = {
        let conn = db.lock();
        store::panel_message(&conn, channel)
    };
    match poster.post_panel(channel, &panel_text()).await {
        Ok(id) => out.posted = id,
        Err(err) => {
            out.notes.push(format!("the panel was not posted in {} ({}) - the old one is left alone", channel, err));
            return out;
        }
    }
    {
        let conn = db.lock();
        if let Err(err) = store::set_panel(&conn, channel, out.posted, now) {
            out.notes.push(format!("the new panel {} was not written down: {}", out.posted, err));
        }
    }
    if let Some(old) = old.filter(|id| *id != out.posted) {
        match poster.delete(channel, old).await {
            Ok(()) => out.deleted = Some(old),
            Err(err) => out.notes.push(format!("the old panel {} was already gone ({})", old, err)),
        }
    }
    out
}

/// The same, against the live store and the real Discord.
async fn move_panel(http: &Http, channel: u64) {
    let Some(db) = store::db() else { return };
    let out = repost_panel(&Live { http }, db, channel, Utc::now().timestamp()).await;
    for note in &out.notes {
        tracing::warn!("confess: {}", note);
    }
}

/// Asks for the panel to be moved to the bottom, now or shortly.
pub fn bump_panel(ctx: &Context, channel: u64) {
    let plan = TICKERS.lock().entry(channel).or_default().posted(now_ms(), PANEL_GAP.as_millis() as i64);
    let http = ctx.http.clone();
    match plan {
        Plan::Nothing => {}
        Plan::Now => {
            tokio::spawn(async move { move_panel(&http, channel).await });
        }
        Plan::After(ms) => {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms.max(0) as u64)).await;
                TICKERS.lock().entry(channel).or_default().fired(now_ms());
                move_panel(&http, channel).await;
            });
        }
    }
}

/// Puts the panel up at startup if there isn't one, so a restart never leaves
/// the channel without its buttons. An existing panel is moved to the bottom
/// only if something has been said under it since.
pub fn spawn(ctx: Context) {
    tokio::spawn(async move {
        if !enabled() {
            return;
        }
        let Some(channel) = channel() else {
            tracing::info!("confess: no confessions channel is set, so the panel was not posted");
            return;
        };
        if store::db().is_none() {
            tracing::error!("confess: the store is not open, so the buttons would refuse every press - no panel posted");
            return;
        }
        move_panel(&ctx.http, channel).await;
    });
}

/// Anything new in the confessions channel that isn't the panel itself puts the
/// panel back at the bottom. This is what keeps it last between approvals —
/// somebody chatting in there, or the old bot posting, moves it down too.
pub fn on_message(ctx: &Context, msg: &Message) {
    if !enabled() || store::db().is_none() {
        return;
    }
    let Some(here) = channel() else { return };
    if msg.channel_id.get() != here {
        return;
    }
    // Our own panel landing must not start another repost, or it would never stop.
    let ours = store::db()
        .map(|db| {
            let conn = db.lock();
            store::panel_message(&conn, here) == Some(msg.id.get())
        })
        .unwrap_or(false);
    if ours {
        return;
    }
    bump_panel(ctx, here);
}

// --- who sent it -------------------------------------------------------------

pub const WHO_NOT_MOD: &str = "Only moderators can look that up.";
pub const WHO_NO_SUCH: &str = "Nothing here has that number.";

/// What a mod is told when they ask who sent one, and the line that goes in the
/// activity log. A look is always written down, whether or not it found
/// anything, because "did anybody look?" has to have an answer too.
pub fn who_sent_words(number: i64, found: Option<&Confession>) -> String {
    match found {
        None => WHO_NO_SUCH.to_string(),
        Some(c) => {
            let state = match c.status {
                Status::Pending => "still waiting on a mod".to_string(),
                Status::Approved => format!("approved by <@{}>", c.decided_by),
                Status::Rejected => format!("rejected by <@{}>", c.decided_by),
            };
            format!(
                "**#{}** was sent by **{}** (<@{}>, `{}`) <t:{}:R> — {}.\n\nThis look has been written to the activity log.",
                c.number,
                if c.user_name.is_empty() { "unknown" } else { &c.user_name },
                c.user_id,
                c.user_id,
                c.created_ts,
                state
            )
        }
    }
}

// --- the two seams the whole flow runs through -------------------------------

/// One submission, from the modal to the row in the store.
///
/// Everything the live handler does about a submission happens here: the guards
/// are checked against the store (the cooldown, and whether the confession a
/// reply names is actually public), and a submission that gets past them is
/// written down before anything is asked of Discord. The handler's only other
/// job is to send the review card.
pub fn submit(
    db: &Mutex<Connection>,
    limits: &Limits,
    user: u64,
    name: &str,
    by_mod: bool,
    kind: Kind,
    number_raw: &str,
    body_raw: &str,
    now: i64,
    seed: i64,
) -> Result<(Confession, Clean), String> {
    let wanted = number_raw.trim().trim_start_matches('#').trim().parse::<i64>().ok();
    let (last, target) = {
        let conn = db.lock();
        (store::last_from(&conn, user), wanted.and_then(|n| store::get(&conn, n).ok().flatten()))
    };
    let clean = vet(limits, user, last, now, kind, number_raw, body_raw, target.as_ref())?;
    let stored = {
        let mut conn = db.lock();
        store::add(
            &mut conn,
            &New {
                kind: clean.kind,
                answers: clean.answers,
                body: clean.body.clone(),
                user_id: user,
                user_name: name.to_string(),
                by_mod,
                created_ts: now,
            },
            seed,
        )
    };
    match stored {
        Ok(c) => Ok((c, clean)),
        Err(err) => {
            tracing::error!("confess: a submission from {} was not written down: {}", user, err);
            Err("Something went wrong saving that. Try again, or tell a mod.".into())
        }
    }
}

/// What the member who submitted is told, privately. It names the number their
/// confession will carry if it is approved — for a reply, the confession it
/// answers, because that is what its post is headed with.
pub fn sent_words(stored: &Confession, clean: &Clean) -> String {
    format!(
        "Sent to the mods. If they approve it, it goes up {} with no name on it.{}",
        match clean.answers {
            Some(target) => format!("in the thread on #{}", target),
            None => format!("as #{}", stored.number),
        },
        if clean.pings_stripped { " (The pings in it were taken out.)" } else { "" }
    )
}

/// What came of one decision.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settled {
    /// False when the decision had already been made by somebody else.
    pub first: bool,
    /// The submission as it stands after the decision.
    pub confession: Option<Confession>,
    /// The posted message, when one went up. Always `None` for a rejection.
    pub posted: Option<u64>,
    /// The thread it is in or was opened for it.
    pub thread: Option<u64>,
    /// True when the panel should be moved back to the bottom.
    pub bump: bool,
    pub notes: Vec<String>,
}

/// One decision, from the button to the posted message. Writes the decision
/// down first, so a rejection can never be posted by a retry and an approval
/// can never be posted twice; then posts an approved one and nothing else.
///
/// A confession gets its own main-channel post and a thread of the same name. A
/// reply goes INSIDE that confession's thread — one post and one thread per
/// confession, with the conversation in one place.
#[allow(clippy::too_many_arguments)]
pub async fn settle_with(
    db: &Mutex<Connection>,
    poster: &dyn Poster,
    here: Option<u64>,
    number: i64,
    status: Status,
    mod_id: u64,
    reason: &str,
    want_thread: bool,
    minutes: u16,
    now: i64,
) -> Settled {
    let mut out = Settled::default();
    out.first = {
        let conn = db.lock();
        store::decide(&conn, number, status, mod_id, now, reason).unwrap_or(false)
    };
    if !out.first {
        out.notes.push(format!("#{} had already been decided, so nothing more was done", number));
        return out;
    }
    let found = {
        let conn = db.lock();
        store::get(&conn, number).ok().flatten()
    };
    let Some(mut c) = found else { return out };

    if status == Status::Approved {
        let Some(here) = here else {
            out.notes.push(format!("#{} was approved with no confessions channel set, so it went nowhere", number));
            out.confession = Some(c);
            return out;
        };
        match c.answers {
            // A reply: into its confession's thread.
            Some(target) => {
                let parent = {
                    let conn = db.lock();
                    store::get(&conn, target).ok().flatten()
                };
                match parent {
                    None => out.notes.push(format!("#{} answers #{}, which is not on record", number, target)),
                    Some(parent) => {
                        let letter = {
                            let conn = db.lock();
                            // This one is approved already, so it is in the
                            // count: its own letter IS that count.
                            reply_letter(store::approved_replies_to(&conn, target).max(1))
                        };
                        let text = reply_text(target, &letter, &c.body);
                        let placed = place_reply(poster, here, &parent, &text, minutes).await;
                        out.notes.extend(placed.notes.clone());
                        if let Some(thread) = placed.opened {
                            let conn = db.lock();
                            let _ = store::set_thread(&conn, parent.number, thread);
                        }
                        if placed.message != 0 {
                            let lives_in = placed.thread.unwrap_or(here);
                            let conn = db.lock();
                            let _ = store::set_posted(&conn, number, lives_in, placed.message);
                            c.posted_channel = lives_in;
                            c.posted_message = placed.message;
                            out.posted = Some(placed.message);
                            out.thread = placed.thread;
                        }
                        // Only a reply that had to fall back into the main
                        // channel moves the panel: one inside a thread does not
                        // touch the channel at all.
                        out.bump = placed.fell_back;
                    }
                }
            }
            // A confession: its own post, and its own thread.
            None => {
                let posted = post_confession(poster, here, &c, want_thread, minutes).await;
                out.notes.extend(posted.notes.clone());
                if posted.message != 0 {
                    let conn = db.lock();
                    let _ = store::set_posted(&conn, number, here, posted.message);
                    c.posted_channel = here;
                    c.posted_message = posted.message;
                    out.posted = Some(posted.message);
                    if let Some(thread) = posted.thread {
                        let _ = store::set_thread(&conn, number, thread);
                        c.thread_id = thread;
                        out.thread = Some(thread);
                    }
                }
                out.bump = true;
            }
        }
    }
    out.confession = Some(c);
    out
}

// --- the live wiring ---------------------------------------------------------

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
}

/// Whether this member can moderate confessions: a bot admin, or anybody with
/// moderator powers in the server.
async fn is_mod(ctx: &Context, guild: Option<GuildId>, user: u64, roles: &[serenity::all::RoleId]) -> bool {
    if super::admin_ids().contains(&user) {
        return true;
    }
    let Some(guild) = guild else { return false };
    // The cache reference is dropped before the await below: holding one across
    // an await makes the whole handler future non-Send.
    let cached = ctx.cache.guild(guild).map(|g| super::house::has_mod_powers(&g.roles, roles));
    if let Some(answer) = cached {
        return answer;
    }
    // The cache has not got the guild: ask Discord rather than guess, since
    // guessing wrong either locks mods out or lets a member approve.
    match guild.roles(&ctx.http).await {
        Ok(map) => super::house::has_mod_powers(&map, roles),
        Err(_) => false,
    }
}

fn db_or_whine() -> Option<&'static Mutex<Connection>> {
    match store::db() {
        Some(db) => Some(db),
        None => {
            tracing::error!("confess: a press arrived with no store open, so it was refused");
            None
        }
    }
}

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    if !enabled() {
        let _ = component.create_response(&ctx.http, whisper("Confessions are switched off right now.")).await;
        return;
    }
    let Some(db) = db_or_whine() else {
        let _ = component.create_response(&ctx.http, whisper("Confessions aren't available right now. Tell a mod.")).await;
        return;
    };
    let limits = limits();

    if id == ID_NEW || id == ID_REPLY {
        if limits.blocked.contains(&component.user.id.get()) {
            let _ = component.create_response(&ctx.http, whisper(BLOCKED_SAID)).await;
            return;
        }
        let modal = if id == ID_NEW { new_modal(&limits) } else { reply_modal(&limits) };
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::Modal(modal)).await;
        return;
    }

    if id == ID_WHO {
        let roles = component.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
        if !is_mod(ctx, component.guild_id, component.user.id.get(), &roles).await {
            let _ = component.create_response(&ctx.http, whisper(WHO_NOT_MOD)).await;
            return;
        }
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::Modal(who_modal())).await;
        return;
    }

    // Approve or reject. Only a mod gets this far.
    let (approving, raw) = match (id.strip_prefix(ID_APPROVE), id.strip_prefix(ID_REJECT)) {
        (Some(n), _) => (true, n),
        (_, Some(n)) => (false, n),
        _ => return,
    };
    let Ok(number) = raw.parse::<i64>() else { return };
    let roles = component.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
    if !is_mod(ctx, component.guild_id, component.user.id.get(), &roles).await {
        let _ = component.create_response(&ctx.http, whisper("Only moderators can decide these.")).await;
        return;
    }
    let found = {
        let conn = db.lock();
        store::get(&conn, number).ok().flatten()
    };
    let Some(c) = found else {
        let _ = component.create_response(&ctx.http, whisper(WHO_NO_SUCH)).await;
        return;
    };
    if c.status != Status::Pending {
        let _ = component
            .create_response(&ctx.http, whisper(format!("#{} was already {} by somebody.", number, c.status.key())))
            .await;
        take_buttons_off(ctx, component).await;
        return;
    }

    if !approving {
        // The reason is optional, so the modal is the whole of the rejection.
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::Modal(reject_modal(number))).await;
        return;
    }

    let mod_id = component.user.id.get();
    let mod_name = component.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| component.user.name.clone());
    let _ = component.create_response(&ctx.http, whisper(format!("Approved #{}. Posting it now.", number))).await;
    take_buttons_off(ctx, component).await;
    settle(ctx, number, Status::Approved, mod_id, &mod_name, "").await;
}

/// Takes the Approve/Reject buttons off a review embed once it has been
/// decided, so nobody presses a dead button. Failing is harmless: the decision
/// is already in the store and a second press is refused there.
async fn take_buttons_off(ctx: &Context, component: &ComponentInteraction) {
    let edit = EditMessage::new().components(vec![]);
    if let Err(err) = component.channel_id.edit_message(&ctx.http, component.message.id, edit).await {
        tracing::debug!("confess: the review buttons were not removed: {}", err);
    }
}

pub async fn on_modal(ctx: &Context, modal: &ModalInteraction) {
    let id = modal.data.custom_id.clone();
    let Some(db) = db_or_whine() else {
        let _ = modal.create_response(&ctx.http, whisper("Confessions aren't available right now. Tell a mod.")).await;
        return;
    };

    if id == MODAL_WHO {
        let roles = modal.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
        if !is_mod(ctx, modal.guild_id, modal.user.id.get(), &roles).await {
            let _ = modal.create_response(&ctx.http, whisper(WHO_NOT_MOD)).await;
            return;
        }
        let asked = field(modal, FIELD_NUMBER);
        let number = asked.trim().trim_start_matches('#').trim().parse::<i64>().unwrap_or(0);
        let found = {
            let conn = db.lock();
            store::get(&conn, number).ok().flatten()
        };
        // Written down before the answer is shown, so a look is on the record
        // even if the reply never reaches them.
        let _ = control::log_change(
            "confess:who",
            None,
            Some(&format!("Looked up who sent #{}", if number > 0 { number.to_string() } else { asked.trim().to_string() })),
            modal.user.id.get(),
        );
        let _ = modal.create_response(&ctx.http, whisper(who_sent_words(number, found.as_ref()))).await;
        return;
    }

    if let Some(raw) = id.strip_prefix(MODAL_REJECT) {
        let Ok(number) = raw.parse::<i64>() else { return };
        let roles = modal.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
        if !is_mod(ctx, modal.guild_id, modal.user.id.get(), &roles).await {
            let _ = modal.create_response(&ctx.http, whisper("Only moderators can decide these.")).await;
            return;
        }
        let reason = field(modal, FIELD_REASON).trim().chars().take(500).collect::<String>();
        let mod_id = modal.user.id.get();
        let mod_name = modal.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| modal.user.name.clone());
        let _ = modal.create_response(&ctx.http, whisper(format!("Rejected #{}. Nothing was posted.", number))).await;
        // The review embed's buttons live on the message the modal came from.
        if let Some(message) = modal.message.as_ref() {
            let edit = EditMessage::new().components(vec![]);
            if let Err(err) = message.channel_id.edit_message(&ctx.http, message.id, edit).await {
                tracing::debug!("confess: the review buttons were not removed: {}", err);
            }
        }
        settle(ctx, number, Status::Rejected, mod_id, &mod_name, &reason).await;
        return;
    }

    if id != MODAL_NEW && id != MODAL_REPLY {
        return;
    }
    if !enabled() {
        let _ = modal.create_response(&ctx.http, whisper("Confessions are switched off right now.")).await;
        return;
    }
    let kind = if id == MODAL_NEW { Kind::Confession } else { Kind::Reply };
    let number_raw = field(modal, FIELD_NUMBER);
    let body_raw = field(modal, FIELD_TEXT);
    let user = modal.user.id.get();
    let name = modal.member.as_ref().map(|m| m.display_name().to_string()).unwrap_or_else(|| modal.user.name.clone());
    let limits = limits();
    let roles = modal.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
    let by_mod = is_mod(ctx, modal.guild_id, user, &roles).await;

    // The guards, and the row in the store — written down before Discord is
    // asked for anything, so a submission that got this far is findable
    // afterwards whatever happens next.
    let (stored, clean) = match submit(
        db,
        &limits,
        user,
        &name,
        by_mod,
        kind,
        &number_raw,
        &body_raw,
        Utc::now().timestamp(),
        first_number(),
    ) {
        Ok(pair) => pair,
        Err(why) => {
            let _ = modal.create_response(&ctx.http, whisper(why)).await;
            return;
        }
    };
    let _ = modal.create_response(&ctx.http, whisper(sent_words(&stored, &clean))).await;

    let Some(review) = review_channel() else {
        tracing::error!("confess: #{} has nowhere to be reviewed - no review channel is set", stored.number);
        return;
    };
    let (approved, rejected) = {
        let conn = db.lock();
        store::tally(&conn, user)
    };
    let who = Submitter {
        id: user,
        name,
        account_ts: Some(UserId::new(user).created_at().unix_timestamp()),
        joined_ts: modal.member.as_ref().and_then(|m| m.joined_at).map(|t| t.unix_timestamp()),
        approved,
        rejected,
        is_mod: by_mod,
    };
    let post = CreateMessage::new()
        .embed(review_embed(&stored, &who, Utc::now().timestamp()))
        .components(vec![review_buttons(stored.number)])
        .allowed_mentions(CreateAllowedMentions::new());
    match ChannelId::new(review).send_message(&ctx.http, post).await {
        Ok(message) => {
            let conn = db.lock();
            let _ = store::set_review_message(&conn, stored.number, message.id.get());
        }
        Err(err) => tracing::error!("confess: #{} was not sent for review: {}", stored.number, err),
    }
}

/// Writes the decision down, posts an approved one, and logs either way.
/// Everything awkward about this lives in [`settle_with`], which has no Discord
/// in it; this is the wiring.
async fn settle(ctx: &Context, number: i64, status: Status, mod_id: u64, mod_name: &str, reason: &str) {
    let Some(db) = store::db() else { return };
    let here = channel();
    let out = settle_with(
        db,
        &Live { http: &ctx.http },
        here,
        number,
        status,
        mod_id,
        reason,
        threads_on(),
        archive_minutes(),
        Utc::now().timestamp(),
    )
    .await;
    for note in &out.notes {
        tracing::warn!("confess: {}", note);
    }
    if !out.first {
        return;
    }
    let Some(c) = out.confession else { return };
    if out.bump {
        if let Some(here) = here {
            bump_panel(ctx, here);
        }
    }

    let guild = ctx.cache.guilds().first().map(|g| g.get()).unwrap_or(0);
    if let Some(log) = log_channel() {
        let post = CreateMessage::new()
            .embed(log_embed(&c, mod_name, guild, None))
            .allowed_mentions(CreateAllowedMentions::new());
        if let Err(err) = ChannelId::new(log).send_message(&ctx.http, post).await {
            tracing::error!("confess: the log entry for #{} was not posted: {}", number, err);
        }
    }
    let _ = control::log_change(
        if status == Status::Approved { "confess:approved" } else { "confess:rejected" },
        None,
        Some(&format!("#{}", number)),
        mod_id,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits { min: 10, max: 100, cooldown: 600, blocked: vec![66] }
    }

    fn confession(number: i64, body: &str) -> Confession {
        Confession { number, body: body.into(), user_id: 11, user_name: "Zoya".into(), created_ts: 1000, ..Confession::default() }
    }

    fn approved(number: i64) -> Confession {
        Confession {
            status: Status::Approved,
            decided_by: 7,
            posted_channel: 21,
            posted_message: 9000 + number as u64,
            ..confession(number, "the original")
        }
    }

    // --- the text ------------------------------------------------------------

    /// Nothing anybody types may ring a phone.
    #[test]
    fn every_kind_of_ping_loses_its_bite() {
        assert_eq!(strip_pings("@everyone look at this"), "everyone look at this");
        assert_eq!(strip_pings("@here now"), "here now");
        assert_eq!(strip_pings("@EVERYONE"), "everyone", "case does not save it");
        assert_eq!(strip_pings("it was <@123456789> all along"), "it was @member all along");
        assert_eq!(strip_pings("<@!123456789> did it"), "@member did it");
        assert_eq!(strip_pings("blame <@&987654321>"), "blame @role");
        assert_eq!(strip_pings("<@11> and <@&22> and @everyone"), "@member and @role and everyone");
        // An ordinary email or a stray < is left exactly as it was.
        assert_eq!(strip_pings("mail me at a@b.com"), "mail me at a@b.com");
        assert_eq!(strip_pings("1 < 2 and <not a tag>"), "1 < 2 and <not a tag>");
        assert_eq!(strip_pings("I <3 this"), "I <3 this");
        assert_eq!(strip_pings("नमस्ते @everyone 🙂"), "नमस्ते everyone 🙂", "multi-byte text survives");
    }

    #[test]
    fn a_confession_that_is_only_pings_is_refused_rather_than_posted_empty() {
        let out = vet(&limits(), 11, None, 5000, Kind::Confession, "", "@everyone", None);
        // "everyone" is 8 characters, under the minimum of 10.
        assert!(out.unwrap_err().contains("at least 10"));
        let out = vet(&limits(), 11, None, 5000, Kind::Confession, "", "<@1> <@&2>", None);
        assert!(out.is_ok(), "that leaves real words behind");
        let out = vet(&limits(), 11, None, 5000, Kind::Confession, "", "   ", None);
        assert_eq!(out.unwrap_err(), EMPTY_SAID);
    }

    #[test]
    fn the_length_range_is_enforced_on_the_cleaned_text() {
        let l = limits();
        assert!(vet(&l, 11, None, 5000, Kind::Confession, "", "too short", None).unwrap_err().contains("at least 10"));
        assert!(vet(&l, 11, None, 5000, Kind::Confession, "", "just enough!", None).is_ok());
        let long = "x".repeat(101);
        let err = vet(&l, 11, None, 5000, Kind::Confession, "", &long, None).unwrap_err();
        assert!(err.contains("101 characters") && err.contains("limit is 100") && err.contains("by about 1"), "{err}");
        assert!(vet(&l, 11, None, 5000, Kind::Confession, "", &"x".repeat(100), None).is_ok(), "exactly the limit is fine");
        // The ping markup is not counted: what is measured is what gets posted.
        let with_ping = format!("<@123456789012345678> {}", "x".repeat(95));
        assert!(vet(&l, 11, None, 5000, Kind::Confession, "", &with_ping, None).unwrap_err().contains("limit is 100"));
    }

    #[test]
    fn an_invite_link_is_refused_and_never_cleaned_away_quietly() {
        let l = limits();
        for bad in ["come to discord.gg/abcd1234 instead", "https://discord.com/invite/xyz99 lol", "DISCORD.GG/Hello1"] {
            assert_eq!(vet(&l, 11, None, 5000, Kind::Confession, "", bad, None).unwrap_err(), INVITE_SAID, "{bad}");
        }
        // Talking about Discord is not an invite.
        assert!(vet(&l, 11, None, 5000, Kind::Confession, "", "I hate discord.gg as a concept", None).is_ok());
    }

    #[test]
    fn a_blocked_member_is_refused_before_anything_else_happens() {
        let l = limits();
        // Blocked beats the cooldown, the length and the reply number alike.
        assert_eq!(vet(&l, 66, None, 5000, Kind::Confession, "", &"x".repeat(50), None).unwrap_err(), BLOCKED_SAID);
        assert_eq!(vet(&l, 66, None, 5000, Kind::Confession, "", "", None).unwrap_err(), BLOCKED_SAID);
        assert!(vet(&l, 11, None, 5000, Kind::Confession, "", &"x".repeat(50), None).is_ok(), "and nobody else is touched");
    }

    #[test]
    fn the_cooldown_counts_from_the_last_submission_and_says_how_long_is_left() {
        let l = limits();
        let body = "x".repeat(50);
        assert!(vet(&l, 11, Some(5000), 5000, Kind::Confession, "", &body, None).is_err(), "straight after one");
        let err = vet(&l, 11, Some(5000), 5100, Kind::Confession, "", &body, None).unwrap_err();
        assert!(err.contains("9 minutes"), "{err}");
        assert!(vet(&l, 11, Some(5000), 5600, Kind::Confession, "", &body, None).is_ok(), "the cooldown is up");
        assert!(vet(&l, 11, Some(5000), 9000, Kind::Confession, "", &body, None).is_ok());
        // A cooldown of nothing is no cooldown at all.
        let none = Limits { cooldown: 0, ..l };
        assert!(vet(&none, 11, Some(5000), 5000, Kind::Confession, "", &body, None).is_ok());
    }

    #[test]
    fn the_wait_is_said_in_whole_units() {
        assert_eq!(wait_words(1), "1 second");
        assert_eq!(wait_words(40), "40 seconds");
        assert_eq!(wait_words(60), "1 minute");
        assert_eq!(wait_words(540), "9 minutes");
        assert_eq!(wait_words(3600), "1 hour");
        assert_eq!(wait_words(7000), "2 hours");
        assert_eq!(wait_words(0), "1 second", "never \"0 seconds\"");
    }

    // --- replies -------------------------------------------------------------

    #[test]
    fn a_reply_has_to_name_a_confession_that_is_actually_public() {
        let l = limits();
        let body = "x".repeat(50);
        let target = approved(457);
        let ok = vet(&l, 11, None, 5000, Kind::Reply, "457", &body, Some(&target)).unwrap();
        assert_eq!(ok.answers, Some(457));
        assert_eq!(ok.kind, Kind::Reply);
        // A "#457" typed with the hash is the same thing.
        assert_eq!(vet(&l, 11, None, 5000, Kind::Reply, " #457 ", &body, Some(&target)).unwrap().answers, Some(457));

        assert_eq!(vet(&l, 11, None, 5000, Kind::Reply, "", &body, None).unwrap_err(), NO_NUMBER_SAID);
        assert_eq!(vet(&l, 11, None, 5000, Kind::Reply, "abc", &body, None).unwrap_err(), NO_NUMBER_SAID);
        assert_eq!(vet(&l, 11, None, 5000, Kind::Reply, "0", &body, None).unwrap_err(), NO_NUMBER_SAID);
        assert_eq!(vet(&l, 11, None, 5000, Kind::Reply, "999", &body, None).unwrap_err(), NO_SUCH_SAID);
    }

    /// A reply must not become a way of finding out what the mods refused.
    #[test]
    fn a_reply_to_something_unposted_says_the_same_thing_whatever_the_truth_is() {
        let l = limits();
        let body = "x".repeat(50);
        let pending = confession(457, "waiting");
        let rejected = Confession { status: Status::Rejected, decided_by: 7, ..confession(457, "refused") };
        let approved_but_gone = Confession { status: Status::Approved, posted_message: 0, ..confession(457, "lost") };
        for c in [&pending, &rejected, &approved_but_gone] {
            assert_eq!(vet(&l, 11, None, 5000, Kind::Reply, "457", &body, Some(c)).unwrap_err(), NOT_YET_SAID);
        }
    }

    // --- what the public sees ------------------------------------------------

    /// The one thing that must never go wrong: nothing about the submitter in
    /// the posted message.
    #[test]
    fn the_posted_message_is_a_number_and_the_words_and_nothing_else() {
        let c = Confession { user_name: "Zoya".into(), user_id: 1234, ..approved(459) };
        let text = confession_text(&c);
        assert_eq!(text, "**Anonymous Confession (#459)**\n\nthe original");
        assert!(!text.contains("Zoya") && !text.contains("1234") && !text.contains('@'), "{text}");
        assert_eq!(public_text(&c, None), text);
        // And the thread takes the same name, so there is no name there either.
        assert_eq!(thread_name(&c), "Anonymous Confession (#459)");
        assert!(!thread_name(&c).contains("Zoya"));
    }

    /// A reply lives inside its confession's thread, labelled so members can
    /// point at it, and with nothing about who sent it.
    #[test]
    fn a_reply_reads_as_a_labelled_anonymous_message_in_the_thread() {
        let reply = Confession { kind: Kind::Reply, answers: Some(457), user_name: "Zoya".into(), ..approved(460) };
        let text = public_text(&reply, Some("B"));
        assert_eq!(text, "**Reply B to Confession (#457)**\n\nthe original");
        assert!(!text.contains("Zoya") && !text.contains("460"), "its own number is for mods only: {text}");
        assert_eq!(reply_text(457, "A", "hello"), "**Reply A to Confession (#457)**\n\nhello");
    }

    #[test]
    fn replies_are_lettered_so_two_never_share_a_label() {
        assert_eq!(reply_letter(1), "A");
        assert_eq!(reply_letter(2), "B");
        assert_eq!(reply_letter(26), "Z");
        assert_eq!(reply_letter(27), "AA");
        assert_eq!(reply_letter(28), "AB");
        assert_eq!(reply_letter(52), "AZ");
        assert_eq!(reply_letter(53), "BA");
        assert_eq!(reply_letter(0), "A", "a count that came back as nothing is still the first reply");
        let mut seen = std::collections::HashSet::new();
        for i in 1..=500 {
            assert!(seen.insert(reply_letter(i)), "{} repeats a letter", i);
        }
    }

    #[test]
    fn a_thread_is_only_ever_opened_once_and_only_on_a_confession() {
        let c = approved(459);
        assert!(needs_thread(&c));
        assert!(!needs_thread(&Confession { thread_id: 7777, ..c.clone() }), "not a second time");
        assert!(!needs_thread(&Confession { posted_message: 0, ..c.clone() }), "nor on something unposted");
        assert!(!needs_thread(&confession(459, "waiting")), "nor on something still pending");
        let reply = Confession { kind: Kind::Reply, answers: Some(457), ..approved(460) };
        assert!(!needs_thread(&reply), "a reply never gets a thread of its own");
    }

    #[test]
    fn the_archive_wait_is_one_discord_actually_takes() {
        assert_eq!(nearest_archive(4320), 4320);
        assert_eq!(nearest_archive(1440), 1440);
        assert_eq!(nearest_archive(60), 60);
        assert_eq!(nearest_archive(10080), 10080);
        assert_eq!(nearest_archive(3000), 4320, "a number Discord refuses becomes the nearest it takes");
        assert_eq!(nearest_archive(0), 60);
        assert_eq!(nearest_archive(999_999), 10080);
        // Three days by default: a confession's thread is still going days on.
        assert!(matches!(archive_duration(4320), AutoArchiveDuration::ThreeDays));
        assert!(matches!(archive_duration(60), AutoArchiveDuration::OneHour));
        assert!(matches!(archive_duration(10080), AutoArchiveDuration::OneWeek));
        assert!(matches!(archive_duration(1440), AutoArchiveDuration::OneDay));
    }

    // --- posting, and the thread it lives in ---------------------------------

    /// A fake Discord that writes down every call and can be told to refuse
    /// whichever of them the test is about.
    #[derive(Default)]
    struct Fake {
        channel_posts: Mutex<Vec<(u64, String, Option<u64>)>>,
        thread_posts: Mutex<Vec<(u64, String)>>,
        opened: Mutex<Vec<(u64, u64, String, u16)>>,
        revived: Mutex<Vec<u64>>,
        /// Thread ids that refuse a post until they have been revived.
        archived: Mutex<Vec<u64>>,
        /// Thread ids that refuse a post whatever happens.
        dead: Mutex<Vec<u64>>,
        /// Every delete asked for: (channel, message).
        deleted: Mutex<Vec<(u64, u64)>>,
        /// Message ids that are not there any more, so a delete of one fails.
        gone: Mutex<Vec<u64>>,
        no_threads: bool,
        no_revive: bool,
        /// The channel refuses every post.
        no_channel: bool,
        next_id: Mutex<u64>,
    }

    impl Fake {
        fn new() -> Self {
            Fake { next_id: Mutex::new(1000), ..Fake::default() }
        }

        fn id(&self) -> u64 {
            let mut n = self.next_id.lock();
            *n += 1;
            *n
        }
    }

    #[async_trait::async_trait]
    impl Poster for Fake {
        async fn say_in_channel(&self, channel: u64, text: &str, reply_to: Option<u64>) -> anyhow::Result<u64> {
            if self.no_channel {
                return Err(anyhow::anyhow!("Missing Permissions"));
            }
            self.channel_posts.lock().push((channel, text.to_string(), reply_to));
            Ok(self.id())
        }

        async fn say_in_thread(&self, thread: u64, text: &str) -> anyhow::Result<u64> {
            if self.dead.lock().contains(&thread) || self.archived.lock().contains(&thread) {
                return Err(anyhow::anyhow!("Thread is archived"));
            }
            self.thread_posts.lock().push((thread, text.to_string()));
            Ok(self.id())
        }

        async fn revive(&self, thread: u64) -> anyhow::Result<()> {
            self.revived.lock().push(thread);
            if self.no_revive {
                return Err(anyhow::anyhow!("Missing Permissions"));
            }
            self.archived.lock().retain(|t| *t != thread);
            Ok(())
        }

        async fn open_thread(&self, channel: u64, message: u64, name: &str, minutes: u16) -> anyhow::Result<u64> {
            self.opened.lock().push((channel, message, name.to_string(), minutes));
            if self.no_threads {
                return Err(anyhow::anyhow!("Max active threads reached"));
            }
            Ok(self.id())
        }

        async fn delete(&self, channel: u64, message: u64) -> anyhow::Result<()> {
            self.deleted.lock().push((channel, message));
            if self.gone.lock().contains(&message) {
                return Err(anyhow::anyhow!("Unknown Message"));
            }
            Ok(())
        }

        async fn post_panel(&self, channel: u64, text: &str) -> anyhow::Result<u64> {
            self.say_in_channel(channel, text, None).await
        }
    }

    #[tokio::test]
    async fn an_approved_confession_is_posted_and_gets_a_thread_of_the_same_name() {
        let fake = Fake::new();
        let c = confession(459, "I cheated at Wordle");
        let out = post_confession(&fake, 21, &c, true, 4320).await;
        assert_ne!(out.message, 0);
        assert_eq!(out.notes, Vec::<String>::new());
        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1, "one post in the channel and no more");
        assert_eq!(posts[0].0, 21);
        assert_eq!(posts[0].1, "**Anonymous Confession (#459)**\n\nI cheated at Wordle");
        let opened = fake.opened.lock().clone();
        assert_eq!(opened.len(), 1, "one thread");
        assert_eq!((opened[0].0, opened[0].1), (21, out.message), "on the message that was just posted");
        assert_eq!(opened[0].2, "Anonymous Confession (#459)", "named exactly as the message is titled");
        assert_eq!(opened[0].3, 4320);
        assert!(out.thread.is_some(), "and its id comes back to be written down");
        // The text is not posted again inside the thread.
        assert!(fake.thread_posts.lock().is_empty(), "nothing is said inside the thread");
    }

    /// A thread that cannot be opened must never cost the confession.
    #[tokio::test]
    async fn a_thread_that_cannot_be_opened_leaves_the_confession_standing() {
        let fake = Fake { no_threads: true, ..Fake::new() };
        let out = post_confession(&fake, 21, &confession(459, "the text"), true, 4320).await;
        assert_ne!(out.message, 0, "the confession is posted");
        assert_eq!(out.thread, None);
        assert_eq!(out.notes.len(), 1);
        assert!(out.notes[0].contains("the confession stands"), "{:?}", out.notes);
        assert!(out.notes[0].contains("Max active threads"), "the reason is in the log: {:?}", out.notes);
        assert_eq!(fake.channel_posts.lock().len(), 1);
        // Threads switched off: the post goes up and nothing is attempted.
        let fake = Fake::new();
        let out = post_confession(&fake, 21, &confession(459, "the text"), false, 4320).await;
        assert_ne!(out.message, 0);
        assert!(fake.opened.lock().is_empty() && out.notes.is_empty());
    }

    /// The flow the owner asked for: one post and one thread per confession,
    /// with the replies inside it.
    #[tokio::test]
    async fn a_reply_lands_in_its_confessions_thread_and_makes_no_second_post() {
        let fake = Fake::new();
        let parent = Confession { thread_id: 7777, ..approved(457) };
        let text = reply_text(457, "A", "same here");
        let out = place_reply(&fake, 21, &parent, &text, 4320).await;
        assert_eq!(out.thread, Some(7777));
        assert!(!out.fell_back && !out.revived && out.opened.is_none());
        assert_eq!(out.notes, Vec::<String>::new());
        assert_eq!(*fake.thread_posts.lock(), vec![(7777, text.clone())]);
        assert!(fake.channel_posts.lock().is_empty(), "no second main-channel message for a reply");
        assert!(fake.opened.lock().is_empty(), "and no second thread");
    }

    /// Discord archives a quiet thread. A reply to it wakes it up rather than
    /// starting a new conversation somewhere else.
    #[tokio::test]
    async fn an_archived_thread_is_woken_up_and_the_reply_goes_in_it() {
        let fake = Fake::new();
        fake.archived.lock().push(7777);
        let parent = Confession { thread_id: 7777, ..approved(457) };
        let out = place_reply(&fake, 21, &parent, "a reply", 4320).await;
        assert_eq!(out.thread, Some(7777));
        assert!(out.revived, "it had to be woken");
        assert!(!out.fell_back && out.opened.is_none());
        assert_eq!(*fake.revived.lock(), vec![7777]);
        assert_eq!(fake.thread_posts.lock().len(), 1);
        assert!(fake.channel_posts.lock().is_empty());
    }

    /// A confession from before this bot, or one whose thread somebody deleted:
    /// a thread is opened on it now and written down.
    #[tokio::test]
    async fn a_confession_with_no_thread_gets_one_opened_for_the_reply() {
        let fake = Fake::new();
        let parent = approved(457);
        assert_eq!(parent.thread_id, 0);
        let out = place_reply(&fake, 21, &parent, "a reply", 4320).await;
        let thread = out.thread.expect("a thread");
        assert_eq!(out.opened, Some(thread), "the new thread is handed back to be written down");
        assert!(!out.fell_back);
        let opened = fake.opened.lock().clone();
        assert_eq!((opened[0].0, opened[0].1, opened[0].2.as_str()), (21, parent.posted_message, "Anonymous Confession (#457)"));
        assert_eq!(*fake.thread_posts.lock(), vec![(thread, "a reply".to_string())]);
        assert!(fake.channel_posts.lock().is_empty());
    }

    /// Everything about threads failed. The reply still goes up, hanging off
    /// the confession, and the failure is in the log.
    #[tokio::test]
    async fn when_no_thread_can_be_used_the_reply_falls_back_to_the_channel() {
        let fake = Fake { no_threads: true, no_revive: true, ..Fake::new() };
        fake.dead.lock().push(7777);
        let parent = Confession { thread_id: 7777, ..approved(457) };
        let out = place_reply(&fake, 21, &parent, "a reply", 4320).await;
        assert!(out.fell_back, "it had to go in the channel");
        assert_eq!(out.thread, None);
        assert_ne!(out.message, 0, "but it was posted");
        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].2, Some(parent.posted_message), "as a Discord reply to the confession");
        assert!(out.notes.iter().any(|n| n.contains("went in the channel rather than its thread")), "{:?}", out.notes);
        assert!(out.notes.iter().any(|n| n.contains("would not wake up")), "the reason is logged too: {:?}", out.notes);
    }

    #[test]
    fn the_panel_says_what_it_does_and_that_mods_can_look() {
        let text = panel_text();
        assert!(text.contains("never your name"), "{text}");
        assert!(text.contains("Moderators do see who submitted"), "a member deserves to know that before typing: {text}");
        assert!(text.contains("last message here"), "{text}");
        let CreateActionRow::Buttons(row) = panel_buttons() else { panic!("a row of buttons") };
        let drawn = serde_json::to_string(&row).unwrap();
        assert!(drawn.contains("Submit a confession") && drawn.contains("Submit a reply"), "{drawn}");
        assert!(drawn.contains(ID_NEW) && drawn.contains(ID_REPLY) && drawn.contains(ID_WHO));
    }

    /// Dispatch is by the id alone, which is what makes a panel posted before a
    /// restart still work after one.
    #[test]
    fn the_buttons_and_modals_are_recognised_by_their_ids_alone() {
        assert!(owns_component(ID_NEW) && owns_component(ID_REPLY) && owns_component(ID_WHO));
        assert!(owns_component("confess:ok:459") && owns_component("confess:no:459"));
        assert!(!owns_component("signup:in") && !owns_component("confess") && !owns_component(""));
        assert!(owns_modal(MODAL_NEW) && owns_modal(MODAL_REPLY) && owns_modal(MODAL_WHO) && owns_modal("confessform:no:459"));
        assert!(!owns_modal("lrmodal:abc") && !owns_modal(ID_NEW));
        // The ids themselves, so a rename has to be deliberate.
        assert_eq!((ID_NEW, ID_REPLY), ("confess:new", "confess:reply"));
        let CreateActionRow::Buttons(row) = review_buttons(459) else { panic!("a row of buttons") };
        let drawn = serde_json::to_string(&row).unwrap();
        assert!(drawn.contains("confess:ok:459") && drawn.contains("confess:no:459"), "{drawn}");
        assert!(drawn.contains("Approve") && drawn.contains("Reject"));
    }

    // --- review and log ------------------------------------------------------

    fn submitter() -> Submitter {
        Submitter {
            id: 1234,
            name: "Zoya".into(),
            account_ts: Some(5000 - 400 * 86_400),
            joined_ts: Some(5000 - 30 * 86_400),
            approved: 3,
            rejected: 1,
            is_mod: false,
        }
    }

    /// Mods see everything about the submitter here. This is how they moderate,
    /// so it is checked field by field.
    #[test]
    fn the_review_embed_shows_the_text_the_submitter_and_their_history() {
        let json = serde_json::to_value(review_embed(&confession(459, "I cheated at Wordle"), &submitter(), 5000)).unwrap();
        assert_eq!(json["title"], "Confession (#459)");
        assert_eq!(json["description"], "I cheated at Wordle");
        let fields: Vec<(String, String)> = json["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["name"].as_str().unwrap().to_string(), f["value"].as_str().unwrap().to_string()))
            .collect();
        let get = |name: &str| fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(get("User"), "Zoya (<@1234>)", "name and mention both");
        assert_eq!(get("ID"), "`1234`");
        assert!(get("Account made").contains("13 months"), "{:?}", get("Account made"));
        assert!(get("Joined").contains("30 days"), "{:?}", get("Joined"));
        assert_eq!(get("Approved before"), "3");
        assert_eq!(get("Rejected before"), "1");
        assert!(json["footer"]["text"].as_str().unwrap().contains("nothing is public until"));
    }

    /// A mod must not be able to wave their own through without anybody seeing.
    #[test]
    fn a_mods_own_submission_says_so_at_the_top_of_the_embed() {
        let who = Submitter { is_mod: true, ..submitter() };
        let json = serde_json::to_value(review_embed(&confession(459, "mine"), &who, 5000)).unwrap();
        let description = json["description"].as_str().unwrap();
        assert!(description.starts_with(OWN_WARNING), "{description}");
        assert!(description.contains("Get somebody else to decide"), "{description}");
        assert!(description.ends_with("mine"), "and the text is still there: {description}");
        // An ordinary member's has no warning at all.
        let plain = serde_json::to_value(review_embed(&confession(459, "mine"), &submitter(), 5000)).unwrap();
        assert!(!plain["description"].as_str().unwrap().contains("moderator"));
    }

    #[test]
    fn a_reply_in_review_names_its_own_number_and_the_one_it_answers() {
        let reply = Confession { kind: Kind::Reply, answers: Some(457), ..confession(460, "my answer") };
        let json = serde_json::to_value(review_embed(&reply, &submitter(), 5000)).unwrap();
        assert_eq!(json["title"], "Confession Reply (#460) · answering #457");
        let answers = json["fields"].as_array().unwrap().iter().any(|f| f["name"] == "Answers" && f["value"] == "Confession #457");
        assert!(answers, "{}", json["fields"]);
    }

    /// The log is the audit trail, in the shape the old bot used.
    #[test]
    fn an_approval_is_logged_with_the_user_the_id_the_link_and_the_mod() {
        let c = Confession { status: Status::Approved, decided_by: 7, decided_ts: 5000, ..approved(459) };
        let json = serde_json::to_value(log_embed(&c, "Kabir", 900, None)).unwrap();
        assert_eq!(json["title"], "Confession Approved (#459)");
        assert_eq!(json["description"], "the original");
        let fields: Vec<(String, String)> = json["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["name"].as_str().unwrap().to_string(), f["value"].as_str().unwrap().to_string()))
            .collect();
        let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["User", "ID", "Link", "Approved By"], "the old bot's four fields, in its order");
        assert_eq!(fields[0].1, "Zoya (<@11>)");
        assert_eq!(fields[1].1, "`11`");
        assert_eq!(fields[2].1, "https://discord.com/channels/900/21/9459", "a clickable link to the posted message");
        assert_eq!(fields[3].1, "Kabir (<@7>)");
    }

    /// A rejection is logged, says who refused it and why, and has no link
    /// because there is nothing public to link to.
    #[test]
    fn a_rejection_is_logged_and_has_no_link_because_nothing_was_posted() {
        let c = Confession {
            status: Status::Rejected,
            decided_by: 7,
            decided_ts: 5000,
            reason: "names somebody".into(),
            ..confession(459, "a name and a number")
        };
        let json = serde_json::to_value(log_embed(&c, "Kabir", 900, None)).unwrap();
        assert_eq!(json["title"], "Confession Rejected (#459)");
        let fields: Vec<(String, String)> = json["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (f["name"].as_str().unwrap().to_string(), f["value"].as_str().unwrap().to_string()))
            .collect();
        let get = |name: &str| fields.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(get("Link"), "never posted");
        assert_eq!(get("Rejected By"), "Kabir (<@7>)");
        assert_eq!(get("Reason"), "names somebody");
        assert!(get("Approved By").is_empty(), "it was not approved by anybody");
        assert_eq!(json["title"].as_str().unwrap().contains("Approved"), false);
    }

    #[test]
    fn an_age_reads_as_a_person_would_say_it() {
        let day = 86_400;
        assert_eq!(age_words(1000, 1000), "today");
        assert_eq!(age_words(1000, 1000 + day), "1 day");
        assert_eq!(age_words(1000, 1000 + 40 * day), "40 days");
        assert_eq!(age_words(1000, 1000 + 400 * day), "13 months");
        assert_eq!(age_words(1000, 1000 + 1000 * day), "2 years");
        assert_eq!(age_words(2000, 1000), "today", "a clock that went backwards is not a negative age");
    }

    // --- the panel staying last ----------------------------------------------

    /// The bug that killed the old bot was a panel nobody could find. The bug
    /// that would kill this one is hammering Discord instead, so a burst of
    /// approvals becomes a handful of reposts.
    #[test]
    fn a_burst_of_confessions_becomes_a_handful_of_reposts() {
        let gap = PANEL_GAP.as_millis() as i64;
        let mut ticker = Ticker::default();
        let mut reposts = 0;
        let mut due: Option<i64> = None;
        let start = 1_000_000;
        // Twenty confessions approved over a minute, three seconds apart.
        for step in 0..20 {
            let now = start + step * 3_000;
            if let Some(at) = due.filter(|at| *at <= now) {
                ticker.fired(at);
                reposts += 1;
                due = None;
            }
            match ticker.posted(now, gap) {
                Plan::Now => reposts += 1,
                Plan::After(ms) => due = Some(now + ms),
                Plan::Nothing => {}
            }
        }
        if due.is_some() {
            reposts += 1;
        }
        assert!(reposts <= 5, "a minute of approvals cost {} reposts, more than one per {:?}", reposts, PANEL_GAP);
        assert!(reposts >= 3, "but the panel must still reach the bottom: only {} reposts in a minute", reposts);
    }

    /// The one rule about this channel: the only message this bot ever deletes
    /// in it is its OWN panel, matched by a message id out of its own store.
    /// The bot this replaces left its panels and hundreds of confessions in
    /// there, and none of them may be touched.
    #[tokio::test]
    async fn the_repost_only_ever_deletes_a_panel_this_bot_wrote_down() {
        let db = sheet();
        let fake = Fake::new();
        // Nothing of ours in the channel yet, so nothing is deleted - however
        // many of the old bot's messages are sitting in there.
        let first = repost_panel(&fake, &db, 21, 1000).await;
        assert_ne!(first.posted, 0);
        assert_eq!(first.deleted, None, "not one message was deleted on the first pass");
        assert!(fake.deleted.lock().is_empty());
        assert_eq!(super::store::panel_message(&db.lock(), 21), Some(first.posted));

        // The next pass deletes exactly the panel the first pass wrote down.
        let second = repost_panel(&fake, &db, 21, 2000).await;
        assert_ne!(second.posted, first.posted);
        assert_eq!(second.deleted, Some(first.posted));
        assert_eq!(*fake.deleted.lock(), vec![(21, first.posted)], "one delete, and it is ours");
        assert_eq!(super::store::panel_message(&db.lock(), 21), Some(second.posted));
        assert_eq!(second.notes, Vec::<String>::new());

        // Ten more passes delete ten panels and nothing else: the count of
        // deletes never exceeds the count of panels this bot has posted.
        let mut posted = vec![first.posted, second.posted];
        for step in 0..10 {
            posted.push(repost_panel(&fake, &db, 21, 3000 + step).await.posted);
        }
        let deleted: Vec<u64> = fake.deleted.lock().iter().map(|(_, m)| *m).collect();
        assert_eq!(deleted.len(), 11, "one delete per repost after the first, and no more");
        for id in &deleted {
            assert!(posted.contains(id), "{} was deleted and this bot never posted it", id);
        }
        // And the one still at the bottom was never deleted.
        assert!(!deleted.contains(posted.last().unwrap()));
    }

    /// Somebody deleting the panel by hand must not stop the next one going up.
    #[tokio::test]
    async fn a_panel_deleted_by_hand_is_simply_replaced() {
        let db = sheet();
        let fake = Fake::new();
        let first = repost_panel(&fake, &db, 21, 1000).await;
        // A mod deletes it in Discord. The store still remembers the id.
        fake.gone.lock().push(first.posted);
        let second = repost_panel(&fake, &db, 21, 2000).await;
        assert_ne!(second.posted, 0, "a fresh panel went up anyway");
        assert_eq!(second.deleted, None, "there was nothing left to delete");
        assert!(second.notes[0].contains("already gone"), "{:?}", second.notes);
        assert_eq!(super::store::panel_message(&db.lock(), 21), Some(second.posted));
        // And the channel is not left panel-less: the next pass tidies up again.
        let third = repost_panel(&fake, &db, 21, 3000).await;
        assert_eq!(third.deleted, Some(second.posted));
    }

    /// A channel that will not take the panel leaves the old one where it is,
    /// rather than deleting it and leaving the channel with none.
    #[tokio::test]
    async fn a_panel_that_cannot_be_posted_leaves_the_old_one_alone() {
        let db = sheet();
        let fake = Fake::new();
        let first = repost_panel(&fake, &db, 21, 1000).await;
        let blocked = Fake { no_channel: true, ..Fake::new() };
        let out = repost_panel(&blocked, &db, 21, 2000).await;
        assert_eq!(out.posted, 0);
        assert_eq!(out.deleted, None, "the one that is there is not taken down");
        assert!(blocked.deleted.lock().is_empty());
        assert!(out.notes[0].contains("left alone"), "{:?}", out.notes);
        assert_eq!(super::store::panel_message(&db.lock(), 21), Some(first.posted), "the store still points at it");
    }

    #[test]
    fn the_first_post_moves_the_panel_at_once_and_the_next_one_waits() {
        let mut ticker = Ticker::default();
        assert_eq!(ticker.posted(100_000, 15_000), Plan::Now, "a quiet channel is tidied straight away");
        assert_eq!(ticker.posted(101_000, 15_000), Plan::After(14_000), "the next one waits out the gap");
        assert_eq!(ticker.posted(101_500, 15_000), Plan::Nothing, "and the ones after it fold into that move");
        assert_eq!(ticker.posted(114_900, 15_000), Plan::Nothing);
        ticker.fired(115_000);
        assert_eq!(ticker.posted(115_100, 15_000), Plan::After(14_900), "the gap starts again from the move that went out");
    }

    // --- the whole flow, with Discord faked ----------------------------------

    fn sheet() -> Mutex<Connection> {
        Mutex::new(super::store::open_memory().expect("store"))
    }

    /// No cooldown, so a test can send two things in a row without the clock
    /// being part of what it is checking.
    fn open_limits() -> Limits {
        Limits { min: 5, max: 500, cooldown: 0, blocked: vec![] }
    }

    /// The flow the members see, end to end: the modal is typed into, a card
    /// goes to the mods with the submitter on it, a mod approves, and a
    /// numbered anonymous post with a thread comes out. Nothing in between.
    #[tokio::test]
    async fn the_whole_flow_from_the_box_to_the_posted_confession() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();

        let (stored, clean) = submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "I cheated at Wordle", 1000, 459)
            .expect("it got past the guards");
        assert_eq!(stored.number, 459);
        assert_eq!(stored.status, Status::Pending);
        assert!(sent_words(&stored, &clean).contains("as #459"), "{}", sent_words(&stored, &clean));

        // What the mods are shown, before anything is public.
        let who = Submitter { id: 11, name: "Zoya".into(), approved: 0, rejected: 0, ..submitter() };
        let card = serde_json::to_value(review_embed(&stored, &who, 1000)).unwrap();
        assert_eq!(card["title"], "Confession (#459)");
        assert!(card.to_string().contains("Zoya"), "the mods see who it was");
        assert!(fake.channel_posts.lock().is_empty(), "and nothing is public yet");

        // A mod approves.
        let out = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await;
        assert!(out.first && out.bump);
        assert_eq!(out.notes, Vec::<String>::new());
        let posted = out.posted.expect("a posted message");
        let thread = out.thread.expect("a thread");

        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1, "exactly one message in the channel");
        assert_eq!(posts[0].1, "**Anonymous Confession (#459)**\n\nI cheated at Wordle");
        assert!(!posts[0].1.contains("Zoya") && !posts[0].1.contains("11"), "{}", posts[0].1);
        assert_eq!(fake.opened.lock()[0].2, "Anonymous Confession (#459)");

        // And the store knows where it all went.
        let c = out.confession.expect("the row");
        assert_eq!((c.status, c.decided_by, c.posted_message, c.thread_id), (Status::Approved, 7, posted, thread));
        let back = super::store::get(&db.lock(), 459).unwrap().unwrap();
        assert_eq!((back.posted_message, back.thread_id, back.posted_channel), (posted, thread, 21));

        // The log entry names the submitter and the mod. Mods only, by design.
        let log = serde_json::to_value(log_embed(&c, "Kabir", 900, None)).unwrap();
        assert_eq!(log["title"], "Confession Approved (#459)");
        assert!(log.to_string().contains("Zoya") && log.to_string().contains("Kabir"), "{log}");
    }

    /// A rejection must be provably unposted: nothing in the channel, nothing
    /// in a thread, nothing anywhere public, now or later.
    #[tokio::test]
    async fn a_rejection_posts_nothing_anywhere_and_is_still_logged() {
        let db = sheet();
        let fake = Fake::new();
        submit(&db, &open_limits(), 11, "Zoya", false, Kind::Confession, "", "somebody's phone number", 1000, 459).unwrap();

        let out = settle_with(&db, &fake, Some(21), 459, Status::Rejected, 7, "doxxing", true, 4320, 2000).await;
        assert!(out.first);
        assert_eq!(out.posted, None);
        assert_eq!(out.thread, None);
        assert!(!out.bump, "there is nothing to move the panel for");
        assert!(fake.channel_posts.lock().is_empty(), "nothing in the confessions channel");
        assert!(fake.thread_posts.lock().is_empty(), "nothing in a thread");
        assert!(fake.opened.lock().is_empty(), "and no thread opened");

        let c = out.confession.expect("the row");
        assert_eq!((c.status, c.decided_by, c.reason.as_str()), (Status::Rejected, 7, "doxxing"));
        assert_eq!(c.posted_message, 0);
        // The log says what happened; it is the only record there is.
        let log = serde_json::to_value(log_embed(&c, "Kabir", 900, None)).unwrap();
        assert_eq!(log["title"], "Confession Rejected (#459)");
        assert!(log.to_string().contains("never posted"), "{log}");

        // And it is still unposted after a second look.
        assert_eq!(super::store::get(&db.lock(), 459).unwrap().unwrap().posted_message, 0);
    }

    /// Two mods pressing Approve at the same moment must not post it twice.
    #[tokio::test]
    async fn the_second_mod_to_decide_changes_nothing_and_posts_nothing() {
        let db = sheet();
        let fake = Fake::new();
        submit(&db, &open_limits(), 11, "Zoya", false, Kind::Confession, "", "the text", 1000, 459).unwrap();
        let first = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await;
        assert!(first.first);
        let second = settle_with(&db, &fake, Some(21), 459, Status::Approved, 8, "", true, 4320, 2100).await;
        assert!(!second.first);
        assert_eq!(second.posted, None);
        assert_eq!(fake.channel_posts.lock().len(), 1, "one post, not two");
        assert_eq!(fake.opened.lock().len(), 1, "one thread, not two");
        assert!(second.notes[0].contains("already been decided"), "{:?}", second.notes);
        // A late rejection cannot unpost it either.
        let late = settle_with(&db, &fake, Some(21), 459, Status::Rejected, 9, "too late", true, 4320, 2200).await;
        assert!(!late.first);
        assert_eq!(super::store::get(&db.lock(), 459).unwrap().unwrap().status, Status::Approved);
    }

    /// The reply flow the owner asked for, end to end: one post and one thread
    /// per confession, with the reply inside it.
    #[tokio::test]
    async fn an_approved_reply_goes_into_its_confessions_thread_and_nowhere_else() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the confession", 1000, 459).unwrap();
        let parent = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await;
        let thread = parent.thread.expect("the confession's thread");
        let channel_posts_before = fake.channel_posts.lock().len();

        // Somebody replies to #459.
        let (reply, clean) = submit(&db, &limits, 12, "Kabir", false, Kind::Reply, "459", "same here", 3000, 459).unwrap();
        assert_eq!(reply.number, 460, "it takes the next number in the series");
        assert_eq!(reply.answers, Some(459));
        assert!(sent_words(&reply, &clean).contains("in the thread on #459"), "{}", sent_words(&reply, &clean));

        let out = settle_with(&db, &fake, Some(21), 460, Status::Approved, 7, "", true, 4320, 4000).await;
        assert_eq!(out.thread, Some(thread), "it went in the confession's own thread");
        assert!(!out.bump, "a reply inside a thread never moves the panel");
        assert_eq!(fake.channel_posts.lock().len(), channel_posts_before, "no second main-channel message");
        assert_eq!(fake.opened.lock().len(), 1, "and no second thread");
        let in_thread = fake.thread_posts.lock().clone();
        assert_eq!(in_thread.len(), 1);
        assert_eq!(in_thread[0].0, thread);
        assert_eq!(in_thread[0].1, "**Reply A to Confession (#459)**\n\nsame here");
        assert!(!in_thread[0].1.contains("Kabir") && !in_thread[0].1.contains("12"), "{}", in_thread[0].1);

        // A second reply is lettered B, so the two can be told apart.
        submit(&db, &limits, 13, "Ira", false, Kind::Reply, "#459", "and me", 5000, 459).unwrap();
        settle_with(&db, &fake, Some(21), 461, Status::Approved, 7, "", true, 4320, 6000).await;
        let in_thread = fake.thread_posts.lock().clone();
        assert_eq!(in_thread.len(), 2);
        assert!(in_thread[1].1.starts_with("**Reply B to Confession (#459)**"), "{}", in_thread[1].1);
    }

    /// The series has to carry on where it stopped, not restart at the seed.
    #[tokio::test]
    async fn the_numbering_climbs_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("confessions.db");
        let limits = open_limits();
        {
            let db = Mutex::new(Connection::open(&path).unwrap());
            db.lock().execute_batch(super::store::SCHEMA).unwrap();
            assert_eq!(submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the first one", 1000, 459).unwrap().0.number, 459);
            assert_eq!(submit(&db, &limits, 12, "Kabir", false, Kind::Confession, "", "the second one", 1100, 459).unwrap().0.number, 460);
        }
        // A different process, the same file, the same setting.
        let db = Mutex::new(Connection::open(&path).unwrap());
        db.lock().execute_batch(super::store::SCHEMA).unwrap();
        let (third, _) = submit(&db, &limits, 13, "Ira", false, Kind::Confession, "", "the third one", 2000, 459).unwrap();
        assert_eq!(third.number, 461, "not back at 459");
        // And everything before it is still there, with who sent it.
        let first = super::store::get(&db.lock(), 459).unwrap().unwrap();
        assert_eq!((first.user_id, first.user_name.as_str()), (11, "Zoya"));
    }

    /// The cooldown is read from the store, so it holds across a restart too.
    #[tokio::test]
    async fn the_cooldown_is_enforced_against_what_the_store_remembers() {
        let db = sheet();
        let limits = Limits { cooldown: 600, ..open_limits() };
        submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the first one", 1000, 459).unwrap();
        let err = submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the second one", 1100, 459).unwrap_err();
        assert!(err.contains("9 minutes"), "{err}");
        // Somebody else is not held up by it.
        assert!(submit(&db, &limits, 12, "Kabir", false, Kind::Confession, "", "mine own", 1100, 459).is_ok());
        // And the wait does run out.
        assert!(submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "later on", 1700, 459).is_ok());
        // Nothing refused was written down.
        assert_eq!(super::store::counts(&db.lock()), (3, 0, 0));
    }

    /// A blocked member gets no row at all: nothing to review, nothing to post.
    #[tokio::test]
    async fn a_blocked_member_leaves_no_row_behind() {
        let db = sheet();
        let limits = Limits { blocked: vec![66], ..open_limits() };
        assert_eq!(submit(&db, &limits, 66, "Troll", false, Kind::Confession, "", "the text", 1000, 459).unwrap_err(), BLOCKED_SAID);
        assert_eq!(super::store::counts(&db.lock()), (0, 0, 0));
        assert_eq!(super::store::next_number(&db.lock(), 459), 459, "and no number was burned");
    }

    // --- who sent it ---------------------------------------------------------

    #[test]
    fn a_mod_asking_who_sent_one_is_told_and_is_told_it_was_logged() {
        let c = Confession { status: Status::Approved, decided_by: 7, ..confession(457, "the text") };
        let words = who_sent_words(457, Some(&c));
        assert!(words.contains("#457") && words.contains("Zoya") && words.contains("<@11>") && words.contains("`11`"), "{words}");
        assert!(words.contains("approved by <@7>"), "{words}");
        assert!(words.contains("written to the activity log"), "a mod is told the look is on the record: {words}");
        assert_eq!(who_sent_words(999, None), WHO_NO_SUCH);
        let waiting = who_sent_words(457, Some(&confession(457, "x")));
        assert!(waiting.contains("still waiting on a mod"), "{waiting}");
    }

    // --- keeping it away from the AI -----------------------------------------

    /// Every path that reads what members said, checked at its own gate. If
    /// one of these ever goes green, a confession is in a prompt.
    #[test]
    fn the_confessions_channels_are_shut_out_of_every_ai_path() {
        use super::super::control::insights;
        use super::super::msglog;

        let never = msglog::never_logged();
        let sensitive = insights::sensitive_channels();
        for id in [DEFAULT_CHANNEL, DEFAULT_REVIEW, DEFAULT_LOG] {
            // The message log: nothing in these channels is recorded at all,
            // which is what blinds the Messages page, the deep dive, the Kalesh
            // pages and the daily topic pass in one go.
            assert!(never.contains(&id), "{} is still logged: {:?}", id, never);
            let place = msglog::Place { channel_id: id, parent_id: None, channel_name: "confessions".into() };
            assert!(msglog::excluded(&place, None, &never), "{} is not excluded from the log", id);
            assert_eq!(
                msglog::classify(true, false, false, !msglog::excluded(&place, None, &never)),
                msglog::Capture::Skip,
                "a message in {} must be skipped, not kept",
                id
            );
            // A thread inside one of them goes too: the parent is checked.
            let thread = msglog::Place { channel_id: 999_001, parent_id: Some(id), channel_name: "Anonymous Confession (#459)".into() };
            assert!(msglog::excluded(&thread, Some("confessions"), &never), "a thread in {} is not excluded", id);

            // The insights pass, which counts who talks to whom.
            assert!(!insights::countable(id, None, true, &sensitive), "{} is being counted", id);
            assert!(!insights::countable(999_001, Some(id), true, &sensitive), "a thread in {} is being counted", id);
            assert!(insights::rows_for(&seen_in(id, None), &sensitive).is_empty(), "{} made an interaction row", id);
            assert!(insights::rows_for(&seen_in(999_001, Some(id)), &sensitive).is_empty());

            // The member notes and the notes facts both filter on exactly this
            // list, the same way they filter #safe-corner.
            assert!(sensitive.contains(&id), "the notes builder reads this list: {:?}", sensitive);
        }

        // The weekly posts scan is the one AI path pointed at channels by hand,
        // so it drops them itself — even when the setting names one.
        let before = super::super::weekly::channels();
        for id in [DEFAULT_CHANNEL, DEFAULT_REVIEW, DEFAULT_LOG] {
            assert!(!before.contains(&id), "the weekly scan would read {}: {:?}", id, before);
        }
        control::set_for_test(
            "VIZIER_WEEKLY_CHANNELS",
            Some(&format!("{},{},{},{}", DEFAULT_CHANNEL, DEFAULT_REVIEW, DEFAULT_LOG, 777)),
        );
        let asked = super::super::weekly::channels();
        control::set_for_test("VIZIER_WEEKLY_CHANNELS", None);
        assert_eq!(asked, vec![777], "a confessions channel typed into the setting is dropped, not read");
    }

    fn seen_in(channel: u64, parent: Option<u64>) -> super::super::control::insights::Seen {
        super::super::control::insights::Seen {
            ts: 1000,
            in_server: true,
            channel_id: channel,
            parent_id: parent,
            known_channel: true,
            message_id: 1,
            author: 11,
            author_bot: false,
            replied: Some((2, 12, false)),
            mentions: vec![(13, false)],
        }
    }

    /// The whole safety claim in one test: all three channels are on the same
    /// list #safe-corner is on, which is what the message log and the member
    /// notes read.
    #[test]
    fn all_three_channels_are_listed_as_sensitive() {
        let listed = sensitive_channels();
        for id in [DEFAULT_CHANNEL, DEFAULT_REVIEW, DEFAULT_LOG] {
            assert!(listed.contains(&id), "{} is not on the sensitive list: {:?}", id, listed);
        }
        assert_eq!(listed.len(), 3);
        // And the insights list, which is the one every reader actually takes,
        // has them alongside #safe-corner.
        let everything = control::insights::sensitive_channels();
        assert!(everything.contains(&super::super::weekly::SAFE_CORNER), "#safe-corner is still on it");
        for id in [DEFAULT_CHANNEL, DEFAULT_REVIEW, DEFAULT_LOG] {
            assert!(everything.contains(&id), "{} must be excluded exactly as #safe-corner is: {:?}", id, everything);
        }
    }
}
