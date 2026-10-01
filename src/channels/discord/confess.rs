//! Anonymous confessions: the two buttons on the newest card, the modals, a
//! mod's decision, and the numbered post that comes out of it.
//!
//! This replaces a third-party bot the server had been using, and it is built
//! to the flow the members already know:
//!
//! 1. **`/confess`** opens the form from anywhere, for anybody, and answers
//!    only the person who ran it. The two buttons — **Submit a confession**
//!    and **Submit a reply** — ride on the confession cards as well, on the
//!    newest card at any one time, where everybody's eye already is. Both
//!    doors open the same form through [`open_form`] and hand back a modal
//!    with the same custom id, so everything past the form is one path.
//!    `/confess` is the door that is always there: the buttons cannot exist
//!    until a confession does, and nothing else could have gone first.
//! 2. Nothing is posted publicly by submitting. The text goes to the review
//!    channel as an embed with the submitter on it — name, mention, id, how old
//!    the account is, when they joined, and how many of theirs have been
//!    approved and rejected before — and two buttons, **Approve** and
//!    **Reject**. Reject asks for an optional reason.
//! 3. Approving posts it in the confessions channel, anonymous and numbered,
//!    with the two buttons on it — and takes the buttons off the card before
//!    it, so the newest card is always the one to press. Rejecting posts
//!    nothing anywhere public, ever. Either way the log channel gets an entry
//!    naming the submitter and the mod who decided.
//! 4. A confession gets no thread when it is posted. A thread is opened on it
//!    only when its first approved reply arrives, and every later reply to it
//!    goes in the same thread. Most confessions never get a reply, and an
//!    empty thread on each one would litter the channel.
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
//! the words and nothing else: no name, no mention, no avatar, no footer. The
//! thread, when a reply opens one, is named after the confession and nobody
//! else. Mods see the submitter in review and in the log, which is how they
//! moderate, and `/whosent` answers it later — admin-only, private, and written
//! to the activity log every time.
//!
//! **The only messages this bot edits are ones it posted itself.** A card is
//! matched by a message id the bot wrote into its own store — never by author,
//! never by content — so the old bot's own messages, and the hundreds of
//! confessions already in that channel, are never touched. The bot deletes
//! nothing in there at all.
//!
//! **The record is written before Discord is asked for anything.** A mod has to
//! be able to answer "who sent #457" a month later, and a rejection has to be
//! provably unposted.

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::Connection;
use serenity::all::{
    ActionRowComponent, AutoArchiveDuration, ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction,
    CommandOptionType, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateButton,
    CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInputText, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, CreateModal, CreateThread, EditMessage, GuildId, Http,
    InputTextStyle, MessageId, ModalInteraction, UserId,
};

use super::confess_store::{self as store, Confession, Filter, Kind, New, Status};
use super::control;

// --- the custom ids ----------------------------------------------------------
//
// Dispatch is by these strings and nothing else, so a card posted before a
// restart still answers afterwards and a review embed from last week can still
// be approved.

pub const ID_NEW: &str = "confess:new";
pub const ID_REPLY: &str = "confess:reply";
pub const MODAL_NEW: &str = "confessform:new";
pub const MODAL_REPLY: &str = "confessform:reply";
/// `confess:ok:<number>` and `confess:no:<number>`.
pub const ID_APPROVE: &str = "confess:ok:";
pub const ID_REJECT: &str = "confess:no:";
/// `confessform:no:<number>` — the optional reason for a rejection.
pub const MODAL_REJECT: &str = "confessform:no:";

/// The field ids inside the modals.
const FIELD_TEXT: &str = "text";
const FIELD_NUMBER: &str = "number";
const FIELD_REASON: &str = "reason";

/// Discord's own ceiling on a message, less room for the heading.
pub const BODY_CEILING: usize = 1800;

/// Whether a custom id belongs to this feature.
pub fn owns_component(id: &str) -> bool {
    id == ID_NEW || id == ID_REPLY || id.starts_with(ID_APPROVE) || id.starts_with(ID_REJECT)
}

/// Whether a modal's custom id belongs to this feature.
pub fn owns_modal(id: &str) -> bool {
    id == MODAL_NEW || id == MODAL_REPLY || id.starts_with(MODAL_REJECT)
}

// --- settings ----------------------------------------------------------------

pub fn enabled() -> bool {
    control::on("VIZIER_CONFESS", true)
}

/// Where approved confessions are posted, and where the two submit buttons
/// ride on the newest card.
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

/// Whether a confession gets a thread when its first reply arrives. On: that
/// is how the server already reads them. Off: an approved reply goes in the
/// channel under its confession instead, and no threads are made.
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
/// clean too — the review embed and the log quote it, and so does the
/// Confessions page.
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
    /// A confession card: the same message, with the two buttons on it.
    /// Separate from `say_in_channel` because only this one carries components.
    async fn post_card(&self, channel: u64, text: &str) -> anyhow::Result<u64>;
    /// Puts the two buttons on an existing card, or takes them off.
    ///
    /// Only ever called with a message id this bot wrote into its own store.
    /// There is no delete on this trait at all: the bot removes nothing from
    /// the confessions channel, so the old bot's messages cannot be harmed
    /// even by a bug.
    async fn set_card_buttons(&self, channel: u64, message: u64, on: bool) -> anyhow::Result<()>;
}

/// Where an approved confession ended up.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Posted {
    /// 0 when the post itself failed.
    pub message: u64,
    /// The card the buttons were taken off, when there was one.
    pub buttons_off: Option<u64>,
    /// What the log should say, when something needs saying.
    pub notes: Vec<String>,
}

/// Posts a confession as a card with the two buttons on it, and takes the
/// buttons off whichever card had them before, so exactly one card in the
/// channel is pressable: the newest.
///
/// No thread is opened here. Most confessions never get a reply, and a thread
/// on every one would bury the channel — the thread is opened by the first
/// reply instead (see [`place_reply`]).
///
/// The new card goes up BEFORE the old one is edited, so a failure in the
/// middle leaves two pressable cards rather than none. `was_on` is the card
/// that had the buttons, out of the store and nowhere else.
pub async fn post_confession(poster: &dyn Poster, channel: u64, c: &Confession, was_on: Option<u64>) -> Posted {
    let mut out = Posted::default();
    match poster.post_card(channel, &confession_text(c)).await {
        Ok(id) => out.message = id,
        Err(err) => {
            out.notes.push(format!("#{} was approved but could not be posted: {}", c.number, err));
            return out;
        }
    }
    if let Some(old) = was_on.filter(|id| *id != out.message) {
        match poster.set_card_buttons(channel, old, false).await {
            Ok(()) => out.buttons_off = Some(old),
            // The old card has gone, or the bot cannot edit it. Harmless: two
            // cards show buttons and both press through to the same place.
            Err(err) => out.notes.push(format!(
                "the buttons were not taken off the card before #{} ({}) - it may still show them",
                c.number, err
            )),
        }
    }
    out
}

/// What moving the buttons onto a card did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ButtonsMove {
    /// The card they are on now, as (number, message id).
    pub on: Option<(i64, u64)>,
    /// True when the card they were meant to be on had gone.
    pub was_lost: bool,
    pub notes: Vec<String>,
}

/// Puts the buttons back on the newest surviving confession card.
///
/// Used when the card carrying them is deleted, and once at startup so a
/// restart cannot leave the channel with no way in. `newest` is the newest card
/// this bot has a message id for, out of its own store — so the buttons can
/// only ever land on a message the bot posted itself.
pub async fn move_buttons(poster: &dyn Poster, channel: u64, newest: Option<(i64, u64)>, lost: bool) -> ButtonsMove {
    let mut out = ButtonsMove { was_lost: lost, ..ButtonsMove::default() };
    let Some((number, message)) = newest else {
        out.notes.push(
            "there is no confession card left in the channel to put the buttons on - the next approved confession brings them back"
                .to_string(),
        );
        return out;
    };
    match poster.set_card_buttons(channel, message, true).await {
        Ok(()) => out.on = Some((number, message)),
        Err(err) => out.notes.push(format!("the buttons could not be put on #{} ({})", number, err)),
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
    want_thread: bool,
    minutes: u16,
) -> Placed {
    let mut out = Placed::default();
    if parent.thread_id != 0 && want_thread {
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

    // The first reply to this confession is what opens its thread: a confession
    // nobody answers never gets one.
    if parent.posted_message != 0 && want_thread {
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

    async fn post_card(&self, channel: u64, text: &str) -> anyhow::Result<u64> {
        let post = CreateMessage::new()
            .content(text)
            .components(vec![card_buttons()])
            .allowed_mentions(CreateAllowedMentions::new());
        Ok(ChannelId::new(channel).send_message(self.http, post).await?.id.get())
    }

    async fn set_card_buttons(&self, channel: u64, message: u64, on: bool) -> anyhow::Result<()> {
        let rows = if on { vec![card_buttons()] } else { vec![] };
        ChannelId::new(channel).edit_message(self.http, MessageId::new(message), EditMessage::new().components(rows)).await?;
        Ok(())
    }
}

/// The two buttons that ride on the newest confession card. There is no panel
/// message: these are the whole of the way in.
pub fn card_buttons() -> CreateActionRow {
    CreateActionRow::Buttons(vec![
        CreateButton::new(ID_NEW).label("Submit a confession").style(ButtonStyle::Primary),
        CreateButton::new(ID_REPLY).label("Submit a reply").style(ButtonStyle::Secondary),
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

/// The reply form. `prefill` fills the number box in for somebody who already
/// said which confession they meant — `/confess number:457` — so they only have
/// to type the reply. The box is still editable, and the number is checked when
/// the form comes back whatever was in it.
pub fn reply_modal(limits: &Limits, prefill: Option<i64>) -> CreateModal {
    let mut number = CreateInputText::new(InputTextStyle::Short, "Which confession? (its number)", FIELD_NUMBER)
        .placeholder("457")
        .max_length(12)
        .required(true);
    if let Some(n) = prefill.filter(|n| *n > 0) {
        number = number.value(n.to_string());
    }
    CreateModal::new(MODAL_REPLY, "Submit a reply").components(vec![
        CreateActionRow::InputText(number),
        CreateActionRow::InputText(
            CreateInputText::new(InputTextStyle::Paragraph, "Your reply", FIELD_TEXT)
                .min_length(limits.min.min(1024) as u16)
                .max_length(limits.max.min(4000) as u16)
                .required(true),
        ),
    ])
}

pub const OFF_SAID: &str = "Confessions are switched off right now.";
pub const BROKEN_SAID: &str = "Confessions aren't available right now. Tell a mod.";

/// The one gate to the submit form, used by both doors: the buttons on a
/// confession card and `/confess`. Either the box to open, or the words to
/// refuse with — and the same words either way, so which door somebody came
/// through is never visible in what they are told.
///
/// Everything past the form is one path too: both doors open a modal with the
/// same custom id, so the same handler, the same guards, the same numbering and
/// the same review queue serve both. There is no second implementation.
pub fn open_form(
    kind: Kind,
    user: u64,
    limits: &Limits,
    on: bool,
    store_open: bool,
    prefill: Option<i64>,
) -> Result<CreateModal, String> {
    if !on {
        return Err(OFF_SAID.to_string());
    }
    if !store_open {
        return Err(BROKEN_SAID.to_string());
    }
    if limits.blocked.contains(&user) {
        return Err(BLOCKED_SAID.to_string());
    }
    Ok(match kind {
        Kind::Confession => new_modal(limits),
        Kind::Reply => reply_modal(limits, prefill),
    })
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
    /// The card now carrying the two buttons, as (number, message id).
    pub buttons_on: Option<(i64, u64)>,
    /// The card the buttons were taken off.
    pub buttons_off: Option<u64>,
    pub notes: Vec<String>,
}

/// One decision, from the button to the posted message. Writes the decision
/// down first, so a rejection can never be posted by a retry and an approval
/// can never be posted twice; then posts an approved one and nothing else.
///
/// A confession is posted as a card with the two buttons on it, and the card
/// before it has them taken off. A reply goes INSIDE its confession's thread,
/// opening that thread if this is the first reply — so one post and at most one
/// thread per confession, with the conversation in one place.
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
            // A reply: into its confession's thread, which the first reply is
            // what opens. Nothing about the main channel changes, so the
            // buttons stay where they are.
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
                        let placed = place_reply(poster, here, &parent, &text, want_thread, minutes).await;
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
                    }
                }
            }
            // A confession: its own card, with the buttons, and the buttons
            // taken off the card before it. No thread until somebody replies.
            None => {
                let was_on = {
                    let conn = db.lock();
                    store::buttons_holder(&conn, here).map(|(message, _)| message)
                };
                let posted = post_confession(poster, here, &c, was_on).await;
                out.notes.extend(posted.notes.clone());
                out.buttons_off = posted.buttons_off;
                if posted.message != 0 {
                    let conn = db.lock();
                    let _ = store::set_posted(&conn, number, here, posted.message);
                    let _ = store::set_buttons_holder(&conn, here, posted.message, number, now);
                    c.posted_channel = here;
                    c.posted_message = posted.message;
                    out.posted = Some(posted.message);
                    out.buttons_on = Some((number, posted.message));
                }
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

    // The two submit buttons, through the same gate `/confess` goes through.
    if id == ID_NEW || id == ID_REPLY {
        let kind = if id == ID_NEW { Kind::Confession } else { Kind::Reply };
        let reply = match open_form(kind, component.user.id.get(), &limits(), enabled(), store::db().is_some(), None) {
            Ok(modal) => CreateInteractionResponse::Modal(modal),
            Err(why) => whisper(why),
        };
        let _ = component.create_response(&ctx.http, reply).await;
        return;
    }

    if !enabled() {
        let _ = component.create_response(&ctx.http, whisper(OFF_SAID)).await;
        return;
    }
    let Some(db) = db_or_whine() else {
        let _ = component.create_response(&ctx.http, whisper(BROKEN_SAID)).await;
        return;
    };

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
        let _ = modal.create_response(&ctx.http, whisper(BROKEN_SAID)).await;
        return;
    };

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
        let _ = modal.create_response(&ctx.http, whisper(OFF_SAID)).await;
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

/// Makes sure the two buttons are on the newest confession card, once, at
/// startup — so a restart can never leave the channel with no way in. An
/// ordinary boot with the newest card already carrying them is one edit that
/// changes nothing.
pub fn spawn(ctx: Context) {
    tokio::spawn(async move {
        if !enabled() {
            return;
        }
        let Some(here) = channel() else {
            tracing::info!("confess: no confessions channel is set, so the buttons were not placed");
            return;
        };
        let Some(db) = store::db() else {
            tracing::error!("confess: the store is not open, so the buttons would refuse every press");
            return;
        };
        let newest = {
            let conn = db.lock();
            store::newest_card(&conn, here)
        };
        let out = move_buttons(&Live { http: &ctx.http }, here, newest, false).await;
        for note in &out.notes {
            tracing::warn!("confess: {}", note);
        }
        match out.on {
            Some((number, _)) => tracing::info!("confess: the buttons are on #{}", number),
            None => tracing::info!("confess: no confession card to put the buttons on yet"),
        }
        if let Some((number, message)) = out.on {
            let conn = db.lock();
            let _ = store::set_buttons_holder(&conn, here, message, number, Utc::now().timestamp());
        }
    });
}

/// A deleted message in the confessions channel.
///
/// Only a message id this bot wrote down itself is recognised; anything else —
/// the old bot's cards, a member's message, one of the old confessions — is
/// not ours and is ignored. When the card carrying the buttons goes, they move
/// to the newest surviving card this bot posted.
pub fn on_delete(ctx: &Context, channel_id: ChannelId, message: MessageId) {
    if !enabled() {
        return;
    }
    let Some(here) = channel() else { return };
    if channel_id.get() != here {
        return;
    }
    let Some(db) = store::db() else { return };
    let (ours, carried) = {
        let conn = db.lock();
        let ours = store::card_at(&conn, here, message.get());
        let carried = store::buttons_holder(&conn, here).is_some_and(|(id, _)| id == message.get());
        (ours, carried)
    };
    let Some(number) = ours else { return };
    {
        let conn = db.lock();
        let _ = store::forget_posted(&conn, number);
        if carried {
            let _ = store::clear_buttons_holder(&conn, here);
        }
    }
    tracing::info!("confess: the card for #{} was deleted from the channel", number);
    if !carried {
        return;
    }
    // It was the pressable one, so the buttons have to move or there is no way
    // to submit anything.
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let Some(db) = store::db() else { return };
        let newest = {
            let conn = db.lock();
            store::newest_card(&conn, here)
        };
        let out = move_buttons(&Live { http: &ctx.http }, here, newest, true).await;
        for note in &out.notes {
            tracing::warn!("confess: {}", note);
        }
        if let Some((number, message)) = out.on {
            tracing::info!("confess: the buttons moved to #{}", number);
            let conn = db.lock();
            let _ = store::set_buttons_holder(&conn, here, message, number, Utc::now().timestamp());
        }
    });
}

// --- /confess ----------------------------------------------------------------

/// `/confess` — the door that is always there.
///
/// The buttons ride on the newest confession card, which means that until one
/// confession exists there is no button anywhere, and a mod cannot bootstrap
/// one either because approving needs a submission first. This command is the
/// way out of that, and it is the better door anyway: anybody can confess from
/// wherever they are rather than being seen typing in the confessions channel.
///
/// `number:` opens the reply form with that number filled in. Everyone may use
/// it, in any channel, and the reply only they can see.
pub fn confess_builder() -> CreateCommand {
    CreateCommand::new("confess")
        .description("send an anonymous confession - nothing is posted until a mod has read it")
        .add_option(
            CreateCommandOption::new(CommandOptionType::Integer, "number", "to reply to a confession instead: its number")
                .required(false)
                .min_int_value(1),
        )
}

pub async fn confess_command(ctx: &Context, command: &CommandInteraction) {
    let wanted = command
        .data
        .options
        .iter()
        .find(|o| o.name == "number")
        .and_then(|o| match o.value {
            CommandDataOptionValue::Integer(n) => Some(n),
            _ => None,
        })
        .filter(|n| *n > 0);
    let kind = if wanted.is_some() { Kind::Reply } else { Kind::Confession };
    // Exactly the gate the buttons go through, and the modal it hands back
    // carries the same custom id — so the form, the guards, the cooldown, the
    // numbering and the review queue are all the one path from here on.
    let reply = match open_form(kind, command.user.id.get(), &limits(), enabled(), store::db().is_some(), wanted) {
        Ok(modal) => CreateInteractionResponse::Modal(modal),
        Err(why) => whisper(why),
    };
    let _ = command.create_response(&ctx.http, reply).await;
}

// --- /whosent ----------------------------------------------------------------

/// `/whosent` — the mods' lookup, moved off the public card. Admin-only, the
/// answer is private, and every look is written to the activity log.
pub fn whosent_builder() -> CreateCommand {
    CreateCommand::new("whosent")
        .description("admin only: who sent a confession, privately - every look is logged")
        .add_option(
            CreateCommandOption::new(CommandOptionType::Integer, "number", "the confession's number")
                .required(true)
                .min_int_value(1),
        )
}

pub async fn whosent_command(ctx: &Context, command: &CommandInteraction) {
    let roles = command.member.as_ref().map(|m| m.roles.clone()).unwrap_or_default();
    if !is_mod(ctx, command.guild_id, command.user.id.get(), &roles).await {
        let _ = command.create_response(&ctx.http, whisper(WHO_NOT_MOD)).await;
        return;
    }
    let number = command
        .data
        .options
        .iter()
        .find(|o| o.name == "number")
        .and_then(|o| match o.value {
            CommandDataOptionValue::Integer(n) => Some(n),
            _ => None,
        })
        .unwrap_or(0);
    let found = store::db().and_then(|db| {
        let conn = db.lock();
        store::get(&conn, number).ok().flatten()
    });
    // Written down before the answer is shown, so a look is on the record even
    // if the reply never reaches them.
    let _ = control::log_change("confess:who", None, Some(&format!("Looked up who sent #{}", number)), command.user.id.get());
    let _ = command.create_response(&ctx.http, whisper(who_sent_words(number, found.as_ref()))).await;
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
    pub(super) struct Fake {
        pub(super) channel_posts: Mutex<Vec<(u64, String, Option<u64>)>>,
        thread_posts: Mutex<Vec<(u64, String)>>,
        opened: Mutex<Vec<(u64, u64, String, u16)>>,
        revived: Mutex<Vec<u64>>,
        /// Thread ids that refuse a post until they have been revived.
        archived: Mutex<Vec<u64>>,
        /// Thread ids that refuse a post whatever happens.
        dead: Mutex<Vec<u64>>,
        /// Every edit asked for: (channel, message, buttons on).
        edits: Mutex<Vec<(u64, u64, bool)>>,
        /// Which message ids are showing buttons: (message, on).
        buttons: Mutex<Vec<(u64, bool)>>,
        /// Message ids that are not there any more, so editing one fails.
        gone: Mutex<Vec<u64>>,
        no_threads: bool,
        no_revive: bool,
        /// The channel refuses every post.
        no_channel: bool,
        next_id: Mutex<u64>,
    }

    impl Fake {
        pub(super) fn new() -> Self {
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

        async fn post_card(&self, channel: u64, text: &str) -> anyhow::Result<u64> {
            if self.no_channel {
                return Err(anyhow::anyhow!("Missing Permissions"));
            }
            let id = self.id();
            self.channel_posts.lock().push((channel, text.to_string(), None));
            self.buttons.lock().push((id, true));
            self.edits.lock().push((channel, id, true));
            Ok(id)
        }

        async fn set_card_buttons(&self, channel: u64, message: u64, on: bool) -> anyhow::Result<()> {
            self.edits.lock().push((channel, message, on));
            if self.gone.lock().contains(&message) {
                return Err(anyhow::anyhow!("Unknown Message"));
            }
            self.buttons.lock().retain(|(id, _)| *id != message);
            self.buttons.lock().push((message, on));
            Ok(())
        }
    }

    impl Fake {
        /// Every message id that is showing the two buttons right now.
        fn pressable(&self) -> Vec<u64> {
            let mut out: Vec<u64> = self.buttons.lock().iter().filter(|(_, on)| *on).map(|(id, _)| *id).collect();
            out.sort_unstable();
            out
        }

        /// Somebody deleted that message in Discord: it is gone from the
        /// channel, so it shows no buttons and cannot be edited again.
        fn vanish(&self, message: u64) {
            self.gone.lock().push(message);
            self.buttons.lock().retain(|(id, _)| *id != message);
        }

        /// Message ids this fake was ever asked to edit.
        fn edited(&self) -> Vec<u64> {
            let mut out: Vec<u64> = self.edits.lock().iter().map(|(_, id, _)| *id).collect();
            out.sort_unstable();
            out.dedup();
            out
        }
    }

    #[tokio::test]
    async fn an_approved_confession_is_posted_as_a_card_with_the_buttons_on_it() {
        let fake = Fake::new();
        let c = confession(459, "I cheated at Wordle");
        let out = post_confession(&fake, 21, &c, None).await;
        assert_ne!(out.message, 0);
        assert_eq!(out.notes, Vec::<String>::new());
        assert_eq!(out.buttons_off, None, "there was no card before it");
        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1, "one post in the channel and no more");
        assert_eq!(posts[0].0, 21);
        assert_eq!(posts[0].1, "**Anonymous Confession (#459)**\n\nI cheated at Wordle");
        assert_eq!(fake.pressable(), vec![out.message], "and it is the pressable one");
        // No thread: that waits for the first reply.
        assert!(fake.opened.lock().is_empty(), "a confession opens no thread");
        assert!(fake.thread_posts.lock().is_empty());

        // The next one takes the buttons off it.
        let next = post_confession(&fake, 21, &confession(460, "me too"), Some(out.message)).await;
        assert_eq!(next.buttons_off, Some(out.message));
        assert_eq!(fake.pressable(), vec![next.message]);
    }

    /// The flow the owner asked for: one post and one thread per confession,
    /// with the replies inside it.
    #[tokio::test]
    async fn a_reply_lands_in_its_confessions_thread_and_makes_no_second_post() {
        let fake = Fake::new();
        let parent = Confession { thread_id: 7777, ..approved(457) };
        let text = reply_text(457, "A", "same here");
        let out = place_reply(&fake, 21, &parent, &text, true, 4320).await;
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
        let out = place_reply(&fake, 21, &parent, "a reply", true, 4320).await;
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
        let out = place_reply(&fake, 21, &parent, "a reply", true, 4320).await;
        let thread = out.thread.expect("a thread");
        assert_eq!(out.opened, Some(thread), "the new thread is handed back to be written down");
        assert!(!out.fell_back);
        let opened = fake.opened.lock().clone();
        assert_eq!((opened[0].0, opened[0].1, opened[0].2.as_str()), (21, parent.posted_message, "Anonymous Confession (#457)"));
        assert_eq!(*fake.thread_posts.lock(), vec![(thread, "a reply".to_string())]);
        assert!(fake.channel_posts.lock().is_empty());
    }

    /// Threads switched off in the settings: the reply goes in the channel
    /// under its confession and no thread is made at all.
    #[tokio::test]
    async fn with_threads_off_a_reply_goes_in_the_channel_and_makes_no_thread() {
        let fake = Fake::new();
        let parent = approved(457);
        let out = place_reply(&fake, 21, &parent, "a reply", false, 4320).await;
        assert!(out.fell_back);
        assert_eq!(out.thread, None);
        assert_eq!(out.opened, None);
        assert!(fake.opened.lock().is_empty(), "no thread was even attempted");
        assert!(fake.thread_posts.lock().is_empty());
        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].2, Some(parent.posted_message), "hanging off the confession");
    }

    /// Everything about threads failed. The reply still goes up, hanging off
    /// the confession, and the failure is in the log.
    #[tokio::test]
    async fn when_no_thread_can_be_used_the_reply_falls_back_to_the_channel() {
        let fake = Fake { no_threads: true, no_revive: true, ..Fake::new() };
        fake.dead.lock().push(7777);
        let parent = Confession { thread_id: 7777, ..approved(457) };
        let out = place_reply(&fake, 21, &parent, "a reply", true, 4320).await;
        assert!(out.fell_back, "it had to go in the channel");
        assert_eq!(out.thread, None);
        assert_ne!(out.message, 0, "but it was posted");
        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1);
        assert_eq!(posts[0].2, Some(parent.posted_message), "as a Discord reply to the confession");
        assert!(out.notes.iter().any(|n| n.contains("went in the channel rather than its thread")), "{:?}", out.notes);
        assert!(out.notes.iter().any(|n| n.contains("would not wake up")), "the reason is logged too: {:?}", out.notes);
    }

    /// There is no panel message: the two buttons ride on the cards, and there
    /// are only ever two of them.
    #[test]
    fn the_cards_carry_exactly_the_two_buttons() {
        let CreateActionRow::Buttons(row) = card_buttons() else { panic!("a row of buttons") };
        assert_eq!(row.len(), 2, "two buttons and nothing else");
        let drawn = serde_json::to_string(&row).unwrap();
        assert!(drawn.contains("Submit a confession") && drawn.contains("Submit a reply"), "{drawn}");
        assert!(drawn.contains(ID_NEW) && drawn.contains(ID_REPLY));
        // The mod lookup is a slash command now, never a button in public.
        assert!(!drawn.to_lowercase().contains("who sent"), "the mod lookup is not on a public card: {drawn}");
        assert!(!drawn.contains("confess:who"), "{drawn}");
        // And the card itself is still only a number and the words.
        let text = confession_text(&approved(459));
        assert!(!text.contains("Submit") && !text.contains("Moderators"), "no panel words on a card: {text}");
    }

    /// Dispatch is by the id alone, which is what makes a card posted before a
    /// restart still work after one.
    #[test]
    fn the_buttons_and_modals_are_recognised_by_their_ids_alone() {
        assert!(owns_component(ID_NEW) && owns_component(ID_REPLY));
        assert!(!owns_component("confess:who"), "the mod lookup button is gone");
        assert!(owns_component("confess:ok:459") && owns_component("confess:no:459"));
        assert!(!owns_component("signup:in") && !owns_component("confess") && !owns_component(""));
        assert!(owns_modal(MODAL_NEW) && owns_modal(MODAL_REPLY) && owns_modal("confessform:no:459"));
        assert!(!owns_modal("confessform:who"), "and so is its modal");
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

    // --- the two doors -------------------------------------------------------

    /// `/confess` and the button must be the same door. The proof is the custom
    /// id: both hand back a modal the one handler claims, so everything past
    /// the form — the guards, the cooldown, the numbering, the review queue —
    /// is one path with no second implementation.
    #[test]
    fn the_command_opens_exactly_the_box_the_button_does() {
        let limits = open_limits();
        let from_button = open_form(Kind::Confession, 11, &limits, true, true, None).unwrap();
        let from_command = open_form(Kind::Confession, 11, &limits, true, true, None).unwrap();
        let drawn = serde_json::to_value(&from_button).unwrap();
        assert_eq!(drawn, serde_json::to_value(&from_command).unwrap(), "the same box, down to the field ids");
        assert_eq!(drawn["custom_id"], MODAL_NEW);
        assert!(owns_modal(drawn["custom_id"].as_str().unwrap()), "and the one handler claims it");
        // Against the hand-built form, so a change to one cannot drift.
        assert_eq!(drawn, serde_json::to_value(new_modal(&limits)).unwrap());

        let reply = serde_json::to_value(open_form(Kind::Reply, 11, &limits, true, true, None).unwrap()).unwrap();
        assert_eq!(reply["custom_id"], MODAL_REPLY);
        assert!(owns_modal(reply["custom_id"].as_str().unwrap()));
        assert_eq!(reply, serde_json::to_value(reply_modal(&limits, None)).unwrap());
    }

    /// `/confess number:457` fills the number in, so they only type the reply.
    /// The box stays editable and the number is still checked on the way back.
    #[test]
    fn the_command_can_fill_the_confession_number_in() {
        let limits = open_limits();
        let prefilled = serde_json::to_value(open_form(Kind::Reply, 11, &limits, true, true, Some(457)).unwrap()).unwrap();
        let text = prefilled.to_string();
        assert!(text.contains("\"value\":\"457\""), "{text}");
        assert_eq!(prefilled["custom_id"], MODAL_REPLY, "still the same form, so still the same handler");
        // Without one, nothing is pre-typed.
        let blank = serde_json::to_value(open_form(Kind::Reply, 11, &limits, true, true, None).unwrap()).unwrap();
        assert!(!blank.to_string().contains("\"value\""), "{blank}");
        // A nonsense number is not written into the box.
        let silly = serde_json::to_value(open_form(Kind::Reply, 11, &limits, true, true, Some(0)).unwrap()).unwrap();
        assert!(!silly.to_string().contains("\"value\""));
    }

    /// Both doors are refused in the same words, so which one somebody used is
    /// never visible in what they are told.
    #[test]
    fn the_same_refusals_guard_both_doors() {
        let limits = Limits { blocked: vec![66], ..open_limits() };
        assert_eq!(open_form(Kind::Confession, 11, &limits, false, true, None).unwrap_err(), OFF_SAID);
        assert_eq!(open_form(Kind::Reply, 11, &limits, false, true, None).unwrap_err(), OFF_SAID);
        assert_eq!(open_form(Kind::Confession, 11, &limits, true, false, None).unwrap_err(), BROKEN_SAID);
        assert_eq!(open_form(Kind::Confession, 66, &limits, true, true, None).unwrap_err(), BLOCKED_SAID);
        assert_eq!(open_form(Kind::Reply, 66, &limits, true, true, Some(457)).unwrap_err(), BLOCKED_SAID);
        // Switched off beats blocked: nobody is told they are on a list by a
        // feature that is not even running.
        assert_eq!(open_form(Kind::Confession, 66, &limits, false, true, None).unwrap_err(), OFF_SAID);
        assert!(open_form(Kind::Confession, 11, &limits, true, true, None).is_ok());
    }

    /// The cold start, which is why this command exists: an empty channel, no
    /// confession card anywhere, so no button anywhere — and the first
    /// confession still goes in, is reviewed, is approved, and is the card the
    /// buttons then appear on.
    #[tokio::test]
    async fn the_very_first_confession_can_be_sent_with_no_cards_in_existence() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();

        // Nothing posted, nothing carrying buttons, nowhere to put them.
        assert_eq!(super::store::newest_card(&db.lock(), 21), None);
        assert_eq!(super::store::buttons_holder(&db.lock(), 21), None);
        let nowhere = move_buttons(&fake, 21, super::store::newest_card(&db.lock(), 21), false).await;
        assert_eq!(nowhere.on, None, "there is no button in the channel at all");
        assert!(fake.pressable().is_empty());

        // `/confess` opens the box anyway.
        let modal = open_form(Kind::Confession, 11, &limits, true, true, None).expect("the command still opens the form");
        assert_eq!(serde_json::to_value(&modal).unwrap()["custom_id"], MODAL_NEW);

        // And the submission goes through the one path, as if a button had been
        // pressed: the series starts where the setting says, and it waits on a
        // mod like any other.
        let (stored, _) = submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the very first one", 1000, 459).unwrap();
        assert_eq!((stored.number, stored.status), (459, Status::Pending));
        assert!(fake.channel_posts.lock().is_empty(), "still nothing public");

        // A mod approves it, and that card is where the buttons appear.
        let out = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await;
        let card = out.posted.expect("the first card");
        assert_eq!(out.buttons_on, Some((459, card)));
        assert_eq!(out.buttons_off, None, "there was no card before it to strip");
        assert_eq!(fake.pressable(), vec![card], "from here on there is a button in the channel");
        assert_eq!(super::store::newest_card(&db.lock(), 21), Some((459, card)));
    }

    /// A reply sent by command is validated exactly as one sent by the button:
    /// the number is checked against the store on the way back, whatever was
    /// pre-typed in the box.
    #[tokio::test]
    async fn a_reply_by_command_is_checked_the_same_way_as_one_by_button() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the confession", 1000, 459).unwrap();

        // Pre-typed or not, the form is the same and #459 is not public yet.
        for prefill in [None, Some(459)] {
            assert_eq!(
                serde_json::to_value(open_form(Kind::Reply, 12, &limits, true, true, prefill).unwrap()).unwrap()["custom_id"],
                MODAL_REPLY
            );
        }
        let too_early = submit(&db, &limits, 12, "Kabir", false, Kind::Reply, "459", "same here", 1100, 459).unwrap_err();
        assert_eq!(too_early, NOT_YET_SAID, "a confession nobody has approved cannot be replied to by either door");

        // Approved: the same submission now goes through, with its own number.
        settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await;
        let (reply, clean) = submit(&db, &limits, 12, "Kabir", false, Kind::Reply, "459", "same here", 3000, 459).unwrap();
        assert_eq!((reply.number, reply.answers), (460, Some(459)));
        assert_eq!(clean.answers, Some(459));

        // And every other way of typing the number is refused as before.
        assert_eq!(submit(&db, &limits, 13, "Ira", false, Kind::Reply, "", "x y z", 4000, 459).unwrap_err(), NO_NUMBER_SAID);
        assert_eq!(submit(&db, &limits, 13, "Ira", false, Kind::Reply, "abc", "x y z", 4000, 459).unwrap_err(), NO_NUMBER_SAID);
        assert_eq!(submit(&db, &limits, 13, "Ira", false, Kind::Reply, "999", "x y z", 4000, 459).unwrap_err(), NO_SUCH_SAID);
        assert_eq!(submit(&db, &limits, 13, "Ira", false, Kind::Reply, " #459 ", "x y z", 4000, 459).unwrap().0.answers, Some(459));
    }

    /// Whichever door a confession came through, what is written down and what
    /// goes up are the same: the command cannot become a way round a guard.
    #[tokio::test]
    async fn a_confession_by_command_and_one_by_button_are_indistinguishable() {
        let limits = Limits { cooldown: 600, ..open_limits() };
        let said = "I have never seen a single Star Wars film";

        // Two stores, one submission each, by the two doors. The door is not an
        // argument to anything past the form, which is the point.
        let by_button = sheet();
        let by_command = sheet();
        let one = submit(&by_button, &limits, 11, "Zoya", false, Kind::Confession, "", said, 1000, 459).unwrap().0;
        let two = submit(&by_command, &limits, 11, "Zoya", false, Kind::Confession, "", said, 1000, 459).unwrap().0;
        assert_eq!(one, two, "the same row, down to the number");

        // The cooldown counts the same, so `/confess` is no way round it.
        let again = submit(&by_command, &limits, 11, "Zoya", false, Kind::Confession, "", said, 1100, 459).unwrap_err();
        assert!(again.contains("9 minutes"), "{again}");
        // So does the length range, and so does ping stripping.
        assert!(submit(&by_command, &limits, 12, "Kabir", false, Kind::Confession, "", "hi", 2000, 459).unwrap_err().contains("at least"));
        let (_, clean) = submit(&by_command, &limits, 13, "Ira", false, Kind::Confession, "", "@everyone look at this", 2000, 459).unwrap();
        assert_eq!(clean.body, "everyone look at this");
        assert!(clean.pings_stripped);
        assert_eq!(
            submit(&by_command, &limits, 14, "Troll", false, Kind::Confession, "", "join discord.gg/abcd1234", 2000, 459).unwrap_err(),
            INVITE_SAID
        );
    }

    // --- the buttons, and which card carries them ----------------------------

    /// The whole of the new posting flow: three confessions approved in a row
    /// leave three cards in the channel and exactly one of them pressable — the
    /// newest. This is what replaced the panel the old bot buried.
    #[tokio::test]
    async fn only_the_newest_card_carries_the_buttons() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        let mut cards = Vec::new();
        for (i, body) in ["the first one", "the second one", "the third one"].iter().enumerate() {
            let n = 459 + i as i64;
            submit(&db, &limits, 11 + i as u64, "Zoya", false, Kind::Confession, "", body, 1000 + i as i64, 459).unwrap();
            let out = settle_with(&db, &fake, Some(21), n, Status::Approved, 7, "", true, 4320, 2000 + i as i64).await;
            let message = out.posted.expect("a card");
            assert_eq!(out.buttons_on, Some((n, message)), "#{} takes the buttons", n);
            if let Some(previous) = cards.last() {
                assert_eq!(out.buttons_off, Some(*previous), "and they come off the card before it");
            } else {
                assert_eq!(out.buttons_off, None, "there was no card before the first");
            }
            assert_eq!(out.thread, None, "no thread is opened when a confession posts");
            cards.push(message);
        }

        // Three cards in the channel, one pressable: the newest.
        assert_eq!(fake.channel_posts.lock().len(), 3);
        assert_eq!(fake.pressable(), vec![*cards.last().unwrap()]);
        assert!(fake.opened.lock().is_empty(), "and not one thread for three confessions");
        assert_eq!(
            super::store::buttons_holder(&db.lock(), 21),
            Some((*cards.last().unwrap(), 461)),
            "the store knows which card to take them off next time"
        );
        // Nothing was deleted: the bot has no way to delete in that channel.
        for id in fake.edited() {
            assert!(cards.contains(&id), "{} was edited and this bot never posted it", id);
        }
    }

    /// The rule that protects the channel: the bot only ever edits a message id
    /// out of its own store. The bot it replaces left its own cards and
    /// hundreds of confessions in there and none of them may be touched — and
    /// there is no delete on the trait at all.
    #[tokio::test]
    async fn the_bot_only_ever_edits_a_card_it_posted_itself() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        // The channel is full of the old bot's messages. None is in our store.
        for stranger in [500_001_u64, 500_002, 500_003] {
            assert_eq!(super::store::card_at(&db.lock(), 21, stranger), None);
        }
        let mut ours = Vec::new();
        for i in 0..4 {
            let n = 459 + i;
            submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "a confession", 1000 + i, 459).unwrap();
            ours.push(settle_with(&db, &fake, Some(21), n, Status::Approved, 7, "", true, 4320, 2000 + i).await.posted.unwrap());
        }
        let touched = fake.edited();
        assert!(!touched.is_empty());
        for id in &touched {
            assert!(ours.contains(id), "{} was touched and this bot never posted it", id);
            assert!(![500_001, 500_002, 500_003].contains(id));
        }
        // Four cards posted, three sets of buttons taken off: never more edits
        // than cards this bot owns.
        assert!(touched.len() <= ours.len());
    }

    /// The card carrying the buttons is deleted. They must move to the newest
    /// surviving card, or there is no way to submit anything.
    #[tokio::test]
    async fn when_the_pressable_card_is_deleted_the_buttons_move_down_one() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        let mut cards = Vec::new();
        for i in 0..3 {
            let n = 459 + i;
            submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "a confession", 1000 + i, 459).unwrap();
            cards.push(settle_with(&db, &fake, Some(21), n, Status::Approved, 7, "", true, 4320, 2000 + i).await.posted.unwrap());
        }
        // A mod deletes the newest card. The store is told, exactly as the
        // delete handler tells it.
        fake.vanish(cards[2]);
        super::store::forget_posted(&db.lock(), 461).unwrap();
        super::store::clear_buttons_holder(&db.lock(), 21).unwrap();
        let newest = super::store::newest_card(&db.lock(), 21);
        assert_eq!(newest, Some((460, cards[1])));

        let moved = move_buttons(&fake, 21, newest, true).await;
        assert_eq!(moved.on, Some((460, cards[1])));
        assert!(moved.was_lost && moved.notes.is_empty());
        assert_eq!(fake.pressable(), vec![cards[1]], "exactly one card is pressable again");

        // And every card gone: that is not an error, and the next approved
        // confession brings the buttons back.
        fake.vanish(cards[1]);
        fake.vanish(cards[0]);
        super::store::forget_posted(&db.lock(), 460).unwrap();
        super::store::forget_posted(&db.lock(), 459).unwrap();
        assert!(fake.pressable().is_empty(), "nothing in the channel to press");
        let nowhere = move_buttons(&fake, 21, super::store::newest_card(&db.lock(), 21), true).await;
        assert_eq!(nowhere.on, None);
        assert!(nowhere.notes[0].contains("next approved confession brings them back"), "{:?}", nowhere.notes);
        submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "a fresh one", 5000, 459).unwrap();
        let back = settle_with(&db, &fake, Some(21), 462, Status::Approved, 7, "", true, 4320, 6000).await;
        assert_eq!(back.buttons_on.map(|(n, _)| n), Some(462));
    }

    /// A card that cannot be edited any more — deleted in the moment between
    /// the new one going up and the edit going out — costs a line in the log
    /// and never the new card.
    #[tokio::test]
    async fn a_card_that_vanished_mid_edit_does_not_cost_the_new_one() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the first one", 1000, 459).unwrap();
        let first = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await.posted.unwrap();
        fake.vanish(first);

        submit(&db, &limits, 12, "Kabir", false, Kind::Confession, "", "the second one", 3000, 459).unwrap();
        let out = settle_with(&db, &fake, Some(21), 460, Status::Approved, 7, "", true, 4320, 4000).await;
        let second = out.posted.expect("the new card went up anyway");
        assert_eq!(out.buttons_on, Some((460, second)));
        assert_eq!(out.buttons_off, None, "there was nothing left to edit");
        assert!(out.notes[0].contains("may still show them"), "{:?}", out.notes);
        assert_eq!(super::store::buttons_holder(&db.lock(), 21), Some((second, 460)));
    }

    /// A channel that will not take the card leaves everything as it was: the
    /// card before it keeps its buttons, so there is still a way in.
    #[tokio::test]
    async fn a_card_that_cannot_be_posted_leaves_the_buttons_where_they_were() {
        let db = sheet();
        let fake = Fake::new();
        let limits = open_limits();
        submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", "the first one", 1000, 459).unwrap();
        let first = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await.posted.unwrap();

        let blocked = Fake { no_channel: true, ..Fake::new() };
        submit(&db, &limits, 12, "Kabir", false, Kind::Confession, "", "the second one", 3000, 459).unwrap();
        let out = settle_with(&db, &blocked, Some(21), 460, Status::Approved, 7, "", true, 4320, 4000).await;
        assert_eq!(out.posted, None);
        assert_eq!(out.buttons_on, None);
        assert_eq!(out.buttons_off, None, "the card that works is not stripped of its buttons");
        assert!(blocked.edits.lock().is_empty(), "nothing was edited at all");
        assert!(out.notes[0].contains("could not be posted"), "{:?}", out.notes);
        assert_eq!(super::store::buttons_holder(&db.lock(), 21), Some((first, 459)), "the store still points at the working card");
        assert_eq!(fake.pressable(), vec![first]);
    }

    // --- the whole flow, with Discord faked ----------------------------------

    pub(super) fn sheet() -> Mutex<Connection> {
        Mutex::new(super::store::open_memory().expect("store"))
    }

    /// No cooldown, so a test can send two things in a row without the clock
    /// being part of what it is checking.
    pub(super) fn open_limits() -> Limits {
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
        assert!(out.first);
        assert_eq!(out.notes, Vec::<String>::new());
        let posted = out.posted.expect("a posted message");
        assert_eq!(out.buttons_on, Some((459, posted)), "the card it posted is the pressable one");
        assert_eq!(out.thread, None, "and no thread until somebody replies");

        let posts = fake.channel_posts.lock().clone();
        assert_eq!(posts.len(), 1, "exactly one message in the channel");
        assert_eq!(posts[0].1, "**Anonymous Confession (#459)**\n\nI cheated at Wordle");
        assert!(!posts[0].1.contains("Zoya") && !posts[0].1.contains("11"), "{}", posts[0].1);
        assert_eq!(fake.pressable(), vec![posted], "the buttons ride on it");
        assert!(fake.opened.lock().is_empty());

        // And the store knows where it all went.
        let c = out.confession.expect("the row");
        assert_eq!((c.status, c.decided_by, c.posted_message, c.thread_id), (Status::Approved, 7, posted, 0));
        let back = super::store::get(&db.lock(), 459).unwrap().unwrap();
        assert_eq!((back.posted_message, back.posted_channel), (posted, 21));

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
        assert_eq!(out.buttons_on, None, "a rejection does not put a card in the channel");
        assert_eq!(out.buttons_off, None, "nor take the buttons off the card that has them");
        assert!(fake.channel_posts.lock().is_empty(), "nothing in the confessions channel");
        assert!(fake.thread_posts.lock().is_empty(), "nothing in a thread");
        assert!(fake.opened.lock().is_empty(), "and no thread opened");
        assert!(fake.edits.lock().is_empty(), "and not one message was touched");

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
        assert_eq!(fake.pressable().len(), 1, "one pressable card, not two");
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
        assert_eq!(parent.thread, None, "a confession nobody has answered has no thread");
        assert!(fake.opened.lock().is_empty());
        let card = parent.posted.expect("the card");
        let channel_posts_before = fake.channel_posts.lock().len();

        // Somebody replies to #459.
        let (reply, clean) = submit(&db, &limits, 12, "Kabir", false, Kind::Reply, "459", "same here", 3000, 459).unwrap();
        assert_eq!(reply.number, 460, "it takes the next number in the series");
        assert_eq!(reply.answers, Some(459));
        assert!(sent_words(&reply, &clean).contains("in the thread on #459"), "{}", sent_words(&reply, &clean));

        let out = settle_with(&db, &fake, Some(21), 460, Status::Approved, 7, "", true, 4320, 4000).await;
        let thread = out.thread.expect("the first reply is what opens the thread");
        assert_eq!(fake.opened.lock().len(), 1, "one thread, opened by the reply and not by the confession");
        assert_eq!(fake.opened.lock()[0].1, card, "on the confession's own card");
        assert_eq!(fake.opened.lock()[0].2, "Anonymous Confession (#459)");
        assert_eq!(out.buttons_on, None, "a reply never moves the buttons");
        assert_eq!(out.buttons_off, None);
        assert_eq!(fake.channel_posts.lock().len(), channel_posts_before, "no second main-channel message");
        assert_eq!(fake.pressable(), vec![card], "the confession's card is still the pressable one");
        let in_thread = fake.thread_posts.lock().clone();
        assert_eq!(in_thread.len(), 1);
        assert_eq!(in_thread[0].0, thread);
        assert_eq!(in_thread[0].1, "**Reply A to Confession (#459)**\n\nsame here");
        assert!(!in_thread[0].1.contains("Kabir") && !in_thread[0].1.contains("12"), "{}", in_thread[0].1);

        // A second reply is lettered B, goes in the same thread, and opens none.
        submit(&db, &limits, 13, "Ira", false, Kind::Reply, "#459", "and me", 5000, 459).unwrap();
        let second = settle_with(&db, &fake, Some(21), 461, Status::Approved, 7, "", true, 4320, 6000).await;
        assert_eq!(second.thread, Some(thread));
        assert_eq!(fake.opened.lock().len(), 1, "later replies open no further threads");
        let in_thread = fake.thread_posts.lock().clone();
        assert_eq!(in_thread.len(), 2);
        assert_eq!(in_thread[1].0, thread);
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

#[cfg(test)]
mod emoji_tests {
    use super::strip_pings;

    /// Emoji are the whole vocabulary of a confession channel. Only real pings
    /// are rewritten; a custom emoji is `<:name:id>` and must survive untouched.
    #[test]
    fn emoji_survive_the_ping_stripper() {
        for kept in [
            "i cried 😭😭 at 3am",
            "<:awwhellnaww:1516710980204232895> this whole month",
            "mixed 🥀 <:hehe:1516581530938511594> and <a:spin:123456789012345678>",
            "math: 3 < 5 > 1",
        ] {
            assert_eq!(strip_pings(kept), kept, "nothing here is a ping");
        }
        // And a ping next to an emoji still goes.
        assert_eq!(strip_pings("<@123> 😭"), "@member 😭");
    }

    /// The same, through the whole path a confession actually takes: the guards,
    /// the row in the store, and the words that go up in public. Nothing along
    /// the way may touch an emoji.
    #[tokio::test]
    async fn emoji_survive_all_the_way_to_the_posted_card() {
        use super::tests::{open_limits, sheet};
        use super::{Kind, confession_text, reply_text, settle_with, submit};
        use crate::channels::discord::confess_store::Status;

        let db = sheet();
        let limits = open_limits();
        let said = "i cried 😭😭 <:awwhellnaww:1516710980204232895> at 3am 🥀";
        let (stored, clean) = submit(&db, &limits, 11, "Zoya", false, Kind::Confession, "", said, 1000, 459).unwrap();
        assert_eq!(clean.body, said, "the guards left every emoji alone");
        assert!(!clean.pings_stripped, "an emoji is not a ping");
        assert_eq!(stored.body, said, "and so did the row in the store");
        assert!(confession_text(&stored).ends_with(said), "{}", confession_text(&stored));

        // Through an approval, into the card, and into a reply inside its thread.
        let fake = super::tests::Fake::new();
        let out = settle_with(&db, &fake, Some(21), 459, Status::Approved, 7, "", true, 4320, 2000).await;
        assert!(out.posted.is_some());
        let posted = fake.channel_posts.lock()[0].1.clone();
        assert!(posted.contains("😭😭") && posted.contains("<:awwhellnaww:1516710980204232895>") && posted.contains("🥀"), "{posted}");
        assert_eq!(reply_text(459, "A", said), format!("**Reply A to Confession (#459)**\n\n{}", said));
    }
}
