//! Invite tracking: which invite each new member joined through, and who made it.
//!
//! Discord never tells a bot which invite was used, so this does what every
//! invite tracker does. It keeps a snapshot of every guild invite with its use
//! count: read at startup, kept current from INVITE_CREATE and INVITE_DELETE,
//! and read again every so often in case an event went missing. When someone
//! joins, the invites are read again and compared with the snapshot:
//!
//! - exactly one invite's count went up: that one (**sure**);
//! - none went up, but a single-use invite (or one at its limit) has gone since:
//!   most likely that one (**likely**) - Discord deletes a used-up invite, and
//!   its INVITE_DELETE often arrives before the join does;
//! - the server's vanity link's count went up: **vanity**, and nobody made it;
//! - more than one went up (two people joined at once): **unsure**, with every
//!   invite it might have been;
//! - nothing changed at all: **unknown** - a bot's sign-in link, Server
//!   Discovery, or an invite the bot never saw.
//!
//! Joins are handled one at a time, and the snapshot is always refreshed after
//! each. When two joins land together, the first sees both counts go up and is
//! "unsure"; the second then sees nothing change, so it is given the same
//! candidates instead of being called unknown.
//!
//! Joins before tracking began are not known at all, and the panel and the
//! commands say so rather than guess.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};

use chrono::TimeZone;
use parking_lot::Mutex;
use rusqlite::Connection;
use serenity::all::{
    CommandDataOptionValue, CommandInteraction, Context, CreateAllowedMentions, CreateCommand, CreateCommandOption,
    CreateInteractionResponse, CreateInteractionResponseMessage, GuildId, Http, UserId,
};

use super::control;
use super::invites_store::{self as store, Candidate, How, Invite, Join, NewJoin, Vanity};

/// How long after it went an invite may still be the one a join used up.
pub const GONE_WINDOW_SECS: i64 = 600;
/// How long a second join may take the candidates an "unsure" one left over.
pub const PENDING_SECS: i64 = 120;

/// Switched on by default; the panel can turn it off.
pub fn enabled() -> bool {
    control::on("VIZIER_INVITES", true)
}

/// Minutes between full re-reads of the invite list.
fn resync_minutes() -> u64 {
    control::number("VIZIER_INVITES_RESYNC_MINUTES", 15).clamp(1, 24 * 60)
}

// --- what Discord is asked --------------------------------------------------------------

/// The two reads a join needs, behind a trait so tests never call Discord.
#[async_trait::async_trait]
pub trait InviteApi: Send + Sync {
    /// Every invite the guild has now.
    async fn invites(&self) -> anyhow::Result<Vec<Invite>>;
    /// The vanity link and its use count, or none when the server has none.
    async fn vanity(&self) -> anyhow::Result<Option<Vanity>>;
}

pub struct LiveApi {
    pub http: Arc<Http>,
    pub guild: GuildId,
}

#[async_trait::async_trait]
impl InviteApi for LiveApi {
    async fn invites(&self) -> anyhow::Result<Vec<Invite>> {
        let list = self.guild.invites(&self.http).await?;
        Ok(list
            .into_iter()
            .map(|i| Invite {
                code: i.code,
                inviter_id: i.inviter.as_ref().map(|u| u.id.get()),
                inviter_name: i.inviter.as_ref().map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone())).unwrap_or_default(),
                channel_id: i.channel.id.get(),
                channel_name: i.channel.name,
                uses: i.uses,
                max_uses: i.max_uses as u64,
                max_age: i.max_age as u64,
                temporary: i.temporary,
                created_ts: i.created_at.unix_timestamp(),
            })
            .collect())
    }

    /// Serenity's own `get_guild_vanity_url` keeps only the code and drops the
    /// use count, which is the whole point here, so the same route is fired
    /// directly (still through serenity's rate limiter).
    async fn vanity(&self) -> anyhow::Result<Option<Vanity>> {
        #[derive(serde::Deserialize)]
        struct Raw {
            #[serde(default)]
            code: Option<String>,
            #[serde(default)]
            uses: u64,
        }
        use serenity::http::{LightMethod, Request, Route};
        let req = Request::new(Route::GuildVanityUrl { guild_id: self.guild }, LightMethod::Get);
        match self.http.fire::<Raw>(req).await {
            Ok(raw) => Ok(raw.code.filter(|c| !c.is_empty()).map(|code| Vanity { code, uses: raw.uses })),
            // A server without the vanity feature is refused, which only means it has none.
            Err(serenity::Error::Http(serenity::all::HttpError::UnsuccessfulRequest(e))) if matches!(e.status_code.as_u16(), 403 | 404) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

// --- the comparison ----------------------------------------------------------------------------

/// Counts one "unsure" join left for the next one: two members who joined at
/// once show as two increments to the first, and nothing to the second.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    pub at: i64,
    pub left: u64,
    pub candidates: Vec<Candidate>,
}

/// One join's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attribution {
    pub how: How,
    /// The invite it was, when there is one answer.
    pub chosen: Option<Candidate>,
    /// Every invite it might have been, when unsure.
    pub candidates: Vec<Candidate>,
    /// Gone invites this join used up: never offered to a later join.
    pub claims: Vec<String>,
}

impl Attribution {
    fn one(how: How, c: Candidate) -> Self {
        Attribution { how, chosen: Some(c), candidates: Vec::new(), claims: Vec::new() }
    }
    fn unsure(candidates: Vec<Candidate>) -> Self {
        Attribution { how: How::Unsure, chosen: None, candidates, claims: Vec::new() }
    }
    pub fn unknown() -> Self {
        Attribution { how: How::Unknown, chosen: None, candidates: Vec::new(), claims: Vec::new() }
    }
}

fn candidate(i: &Invite) -> Candidate {
    Candidate { code: i.code.clone(), inviter_id: i.inviter_id, inviter_name: i.inviter_name.clone(), vanity: false }
}

/// Works out which invite a join used, from the snapshot `before`, the list
/// read just now `after`, invites that went recently and were not yet claimed
/// (`gone`), and the vanity counts either side. `pending` carries an unsure
/// join's leftover count to the next join.
pub fn attribute(
    before: &[Invite],
    after: &[Invite],
    gone: &[Invite],
    vanity_before: Option<&Vanity>,
    vanity_after: Option<&Vanity>,
    pending: &mut Option<Pending>,
    now: i64,
) -> Attribution {
    let prev: HashMap<&str, u64> = before.iter().map(|i| (i.code.as_str(), i.uses)).collect();
    // An invite made and used before its INVITE_CREATE arrived counts from nothing.
    let mut ups: Vec<(Candidate, u64)> = after
        .iter()
        .filter_map(|a| {
            let was = prev.get(a.code.as_str()).copied().unwrap_or(0);
            (a.uses > was).then(|| (candidate(a), a.uses - was))
        })
        .collect();
    if let (Some(b), Some(a)) = (vanity_before, vanity_after) {
        if a.uses > b.uses {
            ups.push((Candidate { code: a.code.clone(), inviter_id: None, inviter_name: String::new(), vanity: true }, a.uses - b.uses));
        }
    }

    if !ups.is_empty() {
        let total: u64 = ups.iter().map(|u| u.1).sum();
        let candidates: Vec<Candidate> = ups.iter().map(|u| u.0.clone()).collect();
        // Fresh counts replace any older leftover.
        *pending = (total > 1).then(|| Pending { at: now, left: total - 1, candidates: candidates.clone() });
        if candidates.len() == 1 {
            let c = candidates.into_iter().next().unwrap_or_else(|| ups[0].0.clone());
            let how = if c.vanity { How::Vanity } else { How::Sure };
            return Attribution::one(how, c);
        }
        return Attribution::unsure(candidates);
    }

    // Nothing went up: a single-use invite (or one on its last use) that vanished.
    let still: HashSet<&str> = after.iter().map(|i| i.code.as_str()).collect();
    let mut seen = HashSet::new();
    let vanished: Vec<&Invite> = before
        .iter()
        .filter(|b| !still.contains(b.code.as_str()))
        .chain(gone.iter().filter(|g| !still.contains(g.code.as_str())))
        .filter(|i| i.at_its_limit())
        .filter(|i| seen.insert(i.code.clone()))
        .collect();
    match vanished.len() {
        0 => {}
        1 => {
            let mut a = Attribution::one(How::Likely, candidate(vanished[0]));
            a.claims = vec![vanished[0].code.clone()];
            return a;
        }
        // Left unclaimed: another join is probably on its way for the rest.
        _ => return Attribution::unsure(vanished.into_iter().map(candidate).collect()),
    }

    // The second of two members who joined at once.
    if let Some(p) = pending.take() {
        if now - p.at <= PENDING_SECS && p.left > 0 {
            let candidates = p.candidates.clone();
            if p.left > 1 {
                *pending = Some(Pending { left: p.left - 1, ..p });
            }
            return if candidates.len() == 1 {
                let c = candidates.into_iter().next().unwrap_or_else(|| Candidate { code: String::new(), inviter_id: None, inviter_name: String::new(), vanity: false });
                let how = if c.vanity { How::Vanity } else { How::Likely };
                Attribution::one(how, c)
            } else {
                Attribution::unsure(candidates)
            };
        }
    }
    Attribution::unknown()
}

// --- keeping the snapshot, and recording joins ----------------------------------------------------

/// One join at a time, and the leftover an unsure join passes on.
pub struct Turn(tokio::sync::Mutex<Option<Pending>>);

impl Turn {
    pub fn new() -> Self {
        Turn(tokio::sync::Mutex::new(None))
    }
}

impl Default for Turn {
    fn default() -> Self {
        Self::new()
    }
}

static LIVE_TURN: LazyLock<Turn> = LazyLock::new(Turn::new);

fn invites_of(rows: Vec<store::Stored>) -> Vec<Invite> {
    rows.into_iter().map(|s| s.invite).collect()
}

/// Reads every invite and the vanity count and makes them the snapshot.
pub async fn sync(api: &dyn InviteApi, db: &Mutex<Connection>, turn: &Turn, now: i64) -> anyhow::Result<usize> {
    let _held = turn.0.lock().await;
    let live = api.invites().await?;
    let vanity = api.vanity().await;
    let conn = db.lock();
    store::replace_snapshot(&conn, &live, now)?;
    match vanity {
        Ok(Some(v)) => store::set_vanity(&conn, &v)?,
        Ok(None) => {}
        Err(e) => tracing::debug!("invites: vanity link not read: {}", e),
    }
    Ok(live.len())
}

/// An INVITE_CREATE: into the snapshot at once, so its first use is seen.
pub fn created(db: &Mutex<Connection>, invite: &Invite, now: i64) -> rusqlite::Result<()> {
    store::upsert(&db.lock(), invite, now)
}

/// An INVITE_DELETE: out of the snapshot, but kept, in case a join used it up.
pub fn deleted(db: &Mutex<Connection>, code: &str, now: i64) -> rusqlite::Result<()> {
    store::mark_gone(&db.lock(), code, now)
}

/// Works out and records which invite a member joined through, then refreshes
/// the snapshot. The database lock is never held across a Discord call.
pub async fn record_join(
    api: &dyn InviteApi,
    db: &Mutex<Connection>,
    turn: &Turn,
    member_id: u64,
    member_name: &str,
    now: i64,
) -> anyhow::Result<Join> {
    let mut pending = turn.0.lock().await;
    let (before, gone, vanity_before) = {
        let conn = db.lock();
        (invites_of(store::active(&conn)?), invites_of(store::recently_gone(&conn, now - GONE_WINDOW_SECS)?), store::vanity(&conn))
    };
    let fetched = api.invites().await;
    let vanity_after = match api.vanity().await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!("invites: vanity link not read: {}", e);
            None
        }
    };
    let (att, note) = match &fetched {
        Ok(after) => (attribute(&before, after, &gone, vanity_before.as_ref(), vanity_after.as_ref(), &mut pending, now), String::new()),
        Err(e) => {
            tracing::warn!("invites: couldn't read the invite list for a join: {}", e);
            (Attribution::unknown(), format!("the invite list couldn't be read: {}", e))
        }
    };
    let new = NewJoin {
        member_id,
        member_name: member_name.to_string(),
        joined_ts: now,
        how: att.how,
        code: att.chosen.as_ref().map(|c| c.code.clone()),
        inviter_id: att.chosen.as_ref().and_then(|c| c.inviter_id),
        inviter_name: att.chosen.as_ref().map(|c| c.inviter_name.clone()).unwrap_or_default(),
        candidates: att.candidates.clone(),
        note,
    };
    let conn = db.lock();
    let id = store::add_join(&conn, &new)?;
    if let Ok(after) = &fetched {
        store::replace_snapshot(&conn, after, now)?;
    }
    for code in &att.claims {
        store::claim(&conn, code)?;
    }
    if let Some(v) = &vanity_after {
        store::set_vanity(&conn, v)?;
    }
    Ok(Join { id, new })
}

// --- the gateway side -------------------------------------------------------------------------------

static GUILD: OnceLock<u64> = OnceLock::new();

fn live_api(ctx: &Context, guild: GuildId) -> LiveApi {
    LiveApi { http: ctx.http.clone(), guild }
}

/// On every cache_ready: read the snapshot (a reconnect may have missed
/// events), and once per process start the periodic re-read.
pub fn on_ready(ctx: &Context, guild: Option<GuildId>) {
    if !enabled() {
        return;
    }
    let Some(guild) = guild else { return };
    let Some(db) = store::db() else { return };
    let _ = GUILD.set(guild.get());
    let api = live_api(ctx, guild);
    tokio::spawn(async move {
        match sync(&api, db, &LIVE_TURN, chrono::Utc::now().timestamp()).await {
            Ok(n) => tracing::info!("invites: snapshot of {} invites taken", n),
            Err(e) => tracing::warn!("invites: couldn't read the invites at start: {}", e),
        }
    });
    static LOOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if LOOP.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let api = live_api(ctx, guild);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(resync_minutes() * 60)).await;
            if !enabled() {
                continue;
            }
            if let Err(e) = sync(&api, db, &LIVE_TURN, chrono::Utc::now().timestamp()).await {
                tracing::warn!("invites: periodic re-read failed: {}", e);
            }
        }
    });
}

fn ours(guild: Option<GuildId>) -> bool {
    match (guild, GUILD.get()) {
        (Some(g), Some(ours)) => g.get() == *ours,
        // Before the first snapshot any guild's invite is taken: there is one server.
        (Some(_), None) => true,
        (None, _) => false,
    }
}

pub fn on_create(ctx: &Context, e: &serenity::all::InviteCreateEvent) {
    if !enabled() || !ours(e.guild_id) {
        return;
    }
    let Some(db) = store::db() else { return };
    let channel_name = e
        .guild_id
        .and_then(|g| ctx.cache.guild(g).and_then(|g| g.channels.get(&e.channel_id).map(|c| c.name.clone())))
        .unwrap_or_default();
    let invite = Invite {
        code: e.code.clone(),
        inviter_id: e.inviter.as_ref().map(|u| u.id.get()),
        inviter_name: e.inviter.as_ref().map(|u| u.global_name.clone().unwrap_or_else(|| u.name.clone())).unwrap_or_default(),
        channel_id: e.channel_id.get(),
        channel_name,
        uses: e.uses,
        max_uses: e.max_uses as u64,
        max_age: e.max_age as u64,
        temporary: e.temporary,
        created_ts: e.created_at.unix_timestamp(),
    };
    if let Err(err) = created(db, &invite, chrono::Utc::now().timestamp()) {
        tracing::warn!("invites: couldn't keep a new invite: {}", err);
    }
}

pub fn on_delete(e: &serenity::all::InviteDeleteEvent) {
    if !enabled() || !ours(e.guild_id) {
        return;
    }
    let Some(db) = store::db() else { return };
    if let Err(err) = deleted(db, &e.code, chrono::Utc::now().timestamp()) {
        tracing::warn!("invites: couldn't mark an invite gone: {}", err);
    }
}

/// A member joined: find the invite in its own task, so the welcome never waits on it.
pub fn on_join(ctx: &Context, member: &serenity::all::Member) {
    if !enabled() || member.user.bot {
        return;
    }
    let Some(db) = store::db() else { return };
    let api = live_api(ctx, member.guild_id);
    let (id, name) = (member.user.id.get(), member.user.name.clone());
    tokio::spawn(async move {
        match record_join(&api, db, &LIVE_TURN, id, &name, chrono::Utc::now().timestamp()).await {
            Ok(j) => tracing::info!("invites: {} joined via {} ({})", name, j.new.code.as_deref().unwrap_or("-"), j.new.how.key()),
            Err(e) => tracing::warn!("invites: couldn't record how {} joined: {}", name, e),
        }
    });
}

// --- the words -----------------------------------------------------------------------------------------

fn ist() -> chrono::FixedOffset {
    chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset")
}

/// "21 Sep", or "21 Sep 2025" when it isn't this year. India time.
pub fn day_words(ts: i64, now: i64) -> String {
    let (Some(t), Some(n)) = (ist().timestamp_opt(ts, 0).single(), ist().timestamp_opt(now, 0).single()) else {
        return String::new();
    };
    if t.format("%Y").to_string() == n.format("%Y").to_string() {
        t.format("%-d %b").to_string()
    } else {
        t.format("%-d %b %Y").to_string()
    }
}

pub fn invite_url(code: &str) -> String {
    format!("discord.gg/{}", code)
}

/// Where a join came from, in one line. `who` names an inviter: "@Kabir" on the
/// panel, a mention in Discord. `joined_at` is Discord's join date, for a member
/// with no tracked join.
pub fn line(join: Option<&NewJoin>, started_ts: i64, joined_at: Option<i64>, now: i64, who: &dyn Fn(u64, &str) -> String) -> String {
    let creator = |id: Option<u64>, name: &str| match id {
        Some(id) => who(id, name),
        None => "nobody Discord names".to_string(),
    };
    let Some(j) = join else {
        return match joined_at {
            Some(at) if at >= started_ts => {
                format!("Joined {} — invite unknown: the bot didn't see this join", day_words(at, now))
            }
            Some(at) => format!(
                "Joined {} — before invite tracking began on {}, so not known. Discord's Server Settings → Members shows it.",
                day_words(at, now),
                day_words(started_ts, now)
            ),
            None => format!(
                "Not known: no join since invite tracking began on {}. Discord's Server Settings → Members shows it for anyone still here.",
                day_words(started_ts, now)
            ),
        };
    };
    let day = day_words(j.joined_ts, now);
    match j.how {
        How::Sure | How::Likely => {
            let code = j.code.as_deref().unwrap_or("?");
            let tail = if j.how == How::Likely { " (likely)" } else { "" };
            format!("Joined {} via {} — invite created by {}{}", day, invite_url(code), creator(j.inviter_id, &j.inviter_name), tail)
        }
        How::Vanity => format!("Joined {} via the server's vanity link", day),
        How::Unsure => {
            let links: Vec<String> =
                j.candidates.iter().map(|c| if c.vanity { "the vanity link".to_string() } else { invite_url(&c.code) }).collect();
            let mut creators: Vec<String> = Vec::new();
            for c in j.candidates.iter().filter(|c| !c.vanity) {
                let name = creator(c.inviter_id, &c.inviter_name);
                if !creators.contains(&name) {
                    creators.push(name);
                }
            }
            if creators.is_empty() {
                format!("Joined {} via {} (unsure)", day, links.join(" or "))
            } else {
                format!("Joined {} via {} — invite created by {} (unsure)", day, links.join(" or "), creators.join(" or "))
            }
        }
        How::Unknown => format!("Joined {} — invite unknown (a bot's sign-in link, Server Discovery, or a link the bot never saw)", day),
    }
}

// --- who brought whom -------------------------------------------------------------------------------------

/// One member an inviter brought in: their latest join through that inviter's invites.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Brought {
    pub member_id: u64,
    pub member_name: String,
    pub joined_ts: i64,
    pub code: String,
    pub how: How,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tally {
    pub inviter_id: u64,
    pub inviter_name: String,
    /// Newest first, each member once.
    pub members: Vec<Brought>,
}

/// Every inviter and the members their invites brought in. Only sure and
/// likely joins count: an unsure one is nobody's for certain.
pub fn tally(joins: &[Join]) -> Vec<Tally> {
    let mut out: Vec<Tally> = Vec::new();
    let mut sorted: Vec<&Join> = joins.iter().collect();
    sorted.sort_by(|a, b| b.new.joined_ts.cmp(&a.new.joined_ts).then(b.id.cmp(&a.id)));
    for j in sorted {
        let (Some(inviter), true) = (j.new.inviter_id, j.new.how.credits_inviter()) else { continue };
        let idx = match out.iter().position(|t| t.inviter_id == inviter) {
            Some(i) => i,
            None => {
                out.push(Tally { inviter_id: inviter, inviter_name: j.new.inviter_name.clone(), members: Vec::new() });
                out.len() - 1
            }
        };
        let t = &mut out[idx];
        if t.members.iter().any(|m| m.member_id == j.new.member_id) {
            continue;
        }
        t.members.push(Brought {
            member_id: j.new.member_id,
            member_name: j.new.member_name.clone(),
            joined_ts: j.new.joined_ts,
            code: j.new.code.clone().unwrap_or_default(),
            how: j.new.how,
        });
    }
    out
}

/// A moment in the join log, written with or without an offset (then UTC).
fn moment(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(t.timestamp());
    }
    raw.parse::<chrono::NaiveDateTime>().ok().map(|t| t.and_utc().timestamp())
}

/// Still in the server by the join log: they never left, or came back after
/// they last did. The same rule as the "Left the server" page.
pub fn still_here(leaves: u32, last_join: Option<&str>, last_leave: Option<&str>) -> bool {
    if leaves == 0 {
        return true;
    }
    let Some(left) = last_leave.and_then(moment) else { return true };
    matches!(last_join.and_then(moment), Some(back) if back >= left)
}

// --- the slash commands --------------------------------------------------------------------------------

pub fn invitedby_builder() -> CreateCommand {
    CreateCommand::new("invitedby")
        .description("mods: which invite a member joined through, and who made it")
        .add_option(CreateCommandOption::new(serenity::all::CommandOptionType::User, "member", "who to look up").required(true))
}

pub fn invites_builder() -> CreateCommand {
    CreateCommand::new("invites")
        .description("mods: how many people a member's invites brought in, and how many stayed")
        .add_option(CreateCommandOption::new(serenity::all::CommandOptionType::User, "member", "whose invites").required(true))
}

fn whisper(text: String) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(
        CreateInteractionResponseMessage::new().content(text).ephemeral(true).allowed_mentions(CreateAllowedMentions::new()),
    )
}

fn member_option(command: &CommandInteraction) -> Option<UserId> {
    command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) if o.name == "member" => Some(id),
        _ => None,
    })
}

/// Refuses anyone who isn't a mod, privately. True when they may go on.
async fn mods_only(ctx: &Context, command: &CommandInteraction) -> bool {
    if super::admin_ids().contains(&command.user.id.get()) {
        return true;
    }
    tracing::warn!("rejected /{} from non-admin {} ({})", command.data.name, command.user.name, command.user.id.get());
    let _ = command.create_response(&ctx.http, whisper("Only mods can use this.".into())).await;
    false
}

/// `/invitedby member:` - the same line as the panel's profile.
pub async fn invitedby_command(ctx: &Context, command: &CommandInteraction) {
    if !mods_only(ctx, command).await {
        return;
    }
    let Some(target) = member_option(command) else {
        let _ = command.create_response(&ctx.http, whisper("Pick a member.".into())).await;
        return;
    };
    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("Invite tracking isn't running (its store didn't open).".into())).await;
        return;
    };
    // The cache is read and let go before Discord is asked: never across an await.
    let cached = command.guild_id.and_then(|g| ctx.cache.guild(g).and_then(|guild| guild.members.get(&target).map(|m| m.joined_at)));
    let joined_at = match (cached, command.guild_id) {
        (Some(at), _) => at.map(|t| t.unix_timestamp()),
        (None, Some(g)) => g.member(&ctx.http, target).await.ok().and_then(|m| m.joined_at.map(|t| t.unix_timestamp())),
        (None, None) => None,
    };
    let (join, started) = {
        let conn = db.lock();
        (store::joins_of(&conn, target.get()).ok().and_then(|j| j.into_iter().next()), store::started_ts(&conn))
    };
    let now = chrono::Utc::now().timestamp();
    let text = format!("<@{}>: {}", target.get(), line(join.as_ref().map(|j| &j.new), started, joined_at, now, &|id, _| format!("<@{}>", id)));
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

/// `/invites member:` - how many their invites brought in and how many stayed.
pub async fn invites_command(ctx: &Context, command: &CommandInteraction, storage: &Arc<crate::storage::VizierStorage>) {
    if !mods_only(ctx, command).await {
        return;
    }
    let Some(target) = member_option(command) else {
        let _ = command.create_response(&ctx.http, whisper("Pick a member.".into())).await;
        return;
    };
    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("Invite tracking isn't running (its store didn't open).".into())).await;
        return;
    };
    let (joins, started, live) = {
        let conn = db.lock();
        let live = store::active(&conn).unwrap_or_default().into_iter().filter(|s| s.invite.inviter_id == Some(target.get())).count();
        (store::all_joins(&conn).unwrap_or_default(), store::started_ts(&conn), live)
    };
    let now = chrono::Utc::now().timestamp();
    let mine = tally(&joins).into_iter().find(|t| t.inviter_id == target.get());
    let brought = mine.map(|t| t.members).unwrap_or_default();
    let mut stayed = 0;
    for b in &brought {
        let log = super::joinlog_get(storage, b.member_id).await;
        if still_here(log.leaves, log.last_join.as_deref(), log.last_leave.as_deref()) {
            stayed += 1;
        }
    }
    let since = day_words(started, now);
    let text = if brought.is_empty() {
        format!("<@{}>'s invites have brought nobody in since tracking began on {}. They have {} live.", target.get(), since, plural(live as u64, "invite"))
    } else {
        format!(
            "<@{}>'s invites brought in {} since tracking began on {}: {} still here, {} gone. They have {} live.",
            target.get(),
            plural(brought.len() as u64, "member"),
            since,
            stayed,
            brought.len() - stayed,
            plural(live as u64, "invite")
        )
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

fn plural(n: u64, word: &str) -> String {
    if n == 1 { format!("1 {}", word) } else { format!("{} {}s", n, word) }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_000;

    pub fn inv(code: &str, by: u64, uses: u64, max_uses: u64) -> Invite {
        Invite {
            code: code.into(),
            inviter_id: Some(by),
            inviter_name: format!("user{}", by),
            channel_id: 21,
            channel_name: "general".into(),
            uses,
            max_uses,
            ..Default::default()
        }
    }

    /// Discord as the tests want it: set what the next read returns, and count calls.
    pub struct FakeApi {
        pub list: Mutex<anyhow::Result<Vec<Invite>>>,
        pub vanity: Mutex<Option<Vanity>>,
        pub calls: std::sync::atomic::AtomicUsize,
    }

    impl FakeApi {
        pub fn new(list: Vec<Invite>, vanity: Option<Vanity>) -> Self {
            FakeApi { list: Mutex::new(Ok(list)), vanity: Mutex::new(vanity), calls: Default::default() }
        }
        pub fn set(&self, list: Vec<Invite>) {
            *self.list.lock() = Ok(list);
        }
    }

    #[async_trait::async_trait]
    impl InviteApi for FakeApi {
        async fn invites(&self) -> anyhow::Result<Vec<Invite>> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            match &*self.list.lock() {
                Ok(v) => Ok(v.clone()),
                Err(e) => Err(anyhow::anyhow!(e.to_string())),
            }
        }
        async fn vanity(&self) -> anyhow::Result<Option<Vanity>> {
            Ok(self.vanity.lock().clone())
        }
    }

    fn db() -> Mutex<Connection> {
        Mutex::new(store::open_memory(NOW - 86_400).unwrap())
    }

    async fn setup(list: Vec<Invite>, vanity: Option<Vanity>) -> (FakeApi, Mutex<Connection>, Turn) {
        let api = FakeApi::new(list, vanity);
        let db = db();
        let turn = Turn::new();
        sync(&api, &db, &turn, NOW - 60).await.unwrap();
        (api, db, turn)
    }

    #[tokio::test]
    async fn exactly_one_count_going_up_is_sure() {
        let (api, db, turn) = setup(vec![inv("aaa", 7, 3, 0), inv("bbb", 8, 1, 0)], None).await;
        api.set(vec![inv("aaa", 7, 3, 0), inv("bbb", 8, 2, 0)]);
        let j = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!((j.new.how, j.new.code.as_deref(), j.new.inviter_id), (How::Sure, Some("bbb"), Some(8)));
        // The snapshot was refreshed: the next read with nothing new is not put on bbb again.
        let next = record_join(&api, &db, &turn, 51, "arjun", NOW + 400).await.unwrap();
        assert_eq!(next.new.how, How::Unknown);
        assert_eq!(store::active(&db.lock()).unwrap().iter().find(|s| s.invite.code == "bbb").unwrap().invite.uses, 2);
    }

    #[tokio::test]
    async fn a_single_use_invite_deleted_before_the_join_arrives_is_likely() {
        let (api, db, turn) = setup(vec![inv("once", 7, 0, 1), inv("open", 8, 5, 0)], None).await;
        // Discord deletes the used-up invite and says so before the member-add event.
        deleted(&db, "once", NOW - 1).unwrap();
        api.set(vec![inv("open", 8, 5, 0)]);
        let j = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!((j.new.how, j.new.code.as_deref(), j.new.inviter_id), (How::Likely, Some("once"), Some(7)));
        // Claimed: the next join isn't put down to it as well.
        let next = record_join(&api, &db, &turn, 51, "arjun", NOW + 1).await.unwrap();
        assert_eq!(next.new.how, How::Unknown);
    }

    #[tokio::test]
    async fn a_vanished_invite_with_uses_left_is_not_blamed() {
        // Deleted by a mod with 3 of 10 uses: nobody used it up.
        let (api, db, turn) = setup(vec![inv("ten", 7, 3, 10)], None).await;
        api.set(vec![]);
        let j = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!(j.new.how, How::Unknown);
    }

    #[tokio::test]
    async fn the_vanity_link_going_up_is_vanity_with_no_inviter() {
        let (api, db, turn) = setup(vec![inv("aaa", 7, 3, 0)], Some(Vanity { code: "mlci".into(), uses: 40 })).await;
        *api.vanity.lock() = Some(Vanity { code: "mlci".into(), uses: 41 });
        let j = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!((j.new.how, j.new.inviter_id), (How::Vanity, None));
        assert_eq!(store::vanity(&db.lock()).unwrap().uses, 41, "the count is kept for the next join");
    }

    #[tokio::test]
    async fn two_joining_at_once_are_both_unsure_with_the_candidates() {
        let (api, db, turn) = setup(vec![inv("aaa", 7, 3, 0), inv("bbb", 8, 1, 0), inv("ccc", 9, 0, 0)], None).await;
        api.set(vec![inv("aaa", 7, 4, 0), inv("bbb", 8, 2, 0), inv("ccc", 9, 0, 0)]);
        let first = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!(first.new.how, How::Unsure);
        assert_eq!(first.new.inviter_id, None, "nobody gets the credit");
        let codes: Vec<&str> = first.new.candidates.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes, vec!["aaa", "bbb"]);
        // The second sees nothing move, and takes the same two rather than "unknown".
        let second = record_join(&api, &db, &turn, 51, "arjun", NOW + 2).await.unwrap();
        assert_eq!((second.new.how, second.new.candidates.len()), (How::Unsure, 2));
        // A third has nothing left over.
        let third = record_join(&api, &db, &turn, 52, "dev", NOW + 3).await.unwrap();
        assert_eq!(third.new.how, How::Unknown);
        // Kept in the store with the candidates.
        let back = store::joins_of(&db.lock(), 50).unwrap();
        assert_eq!(back[0].new.candidates.len(), 2);
    }

    #[tokio::test]
    async fn nothing_changing_is_unknown_and_a_failed_read_says_why() {
        let (api, db, turn) = setup(vec![inv("aaa", 7, 3, 0)], None).await;
        let j = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!((j.new.how, j.new.code.clone()), (How::Unknown, None));
        *api.list.lock() = Err(anyhow::anyhow!("503"));
        let j = record_join(&api, &db, &turn, 51, "arjun", NOW + 1).await.unwrap();
        assert_eq!(j.new.how, How::Unknown);
        assert!(j.new.note.contains("couldn't be read"), "{}", j.new.note);
        // A failed read leaves the snapshot as it was.
        assert_eq!(store::active(&db.lock()).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn the_snapshot_stays_current_across_create_and_delete() {
        let (api, db, turn) = setup(vec![inv("aaa", 7, 3, 0)], None).await;
        // A new invite, then its first use: sure, from its create event.
        created(&db, &inv("new1", 9, 0, 0), NOW - 30).unwrap();
        api.set(vec![inv("aaa", 7, 3, 0), inv("new1", 9, 1, 0)]);
        let j = record_join(&api, &db, &turn, 50, "zoya", NOW).await.unwrap();
        assert_eq!((j.new.how, j.new.code.as_deref()), (How::Sure, Some("new1")));
        // An invite made and used before its create event arrived still counts from nothing.
        api.set(vec![inv("aaa", 7, 3, 0), inv("new1", 9, 1, 0), inv("fast", 8, 1, 0)]);
        let j = record_join(&api, &db, &turn, 51, "arjun", NOW + 5).await.unwrap();
        assert_eq!((j.new.how, j.new.code.as_deref()), (How::Sure, Some("fast")));
        // Deleted: out of the live snapshot, still on record.
        deleted(&db, "aaa", NOW + 10).unwrap();
        let live: Vec<String> = store::active(&db.lock()).unwrap().into_iter().map(|s| s.invite.code).collect();
        assert_eq!(live, vec!["fast", "new1"]);
        assert_eq!(store::all_invites(&db.lock()).unwrap().len(), 3);
        // A missed delete is caught by the periodic re-read.
        api.set(vec![inv("fast", 8, 1, 0)]);
        sync(&api, &db, &turn, NOW + 900).await.unwrap();
        let live: Vec<String> = store::active(&db.lock()).unwrap().into_iter().map(|s| s.invite.code).collect();
        assert_eq!(live, vec!["fast"]);
    }

    fn join(how: How, code: Option<&str>, by: Option<u64>) -> NewJoin {
        NewJoin {
            member_id: 50,
            member_name: "zoya".into(),
            joined_ts: NOW,
            how,
            code: code.map(String::from),
            inviter_id: by,
            inviter_name: "Kabir".into(),
            candidates: vec![],
            note: String::new(),
        }
    }

    #[test]
    fn the_line_says_how_sure_only_when_it_isnt() {
        let who = |id: u64, _: &str| if id == 7 { "@Kabir".to_string() } else { format!("@{}", id) };
        let started = NOW - 86_400;
        let sure = line(Some(&join(How::Sure, Some("abc123"), Some(7))), started, None, NOW, &who);
        assert_eq!(sure, format!("Joined {} via discord.gg/abc123 — invite created by @Kabir", day_words(NOW, NOW)));
        let likely = line(Some(&join(How::Likely, Some("abc123"), Some(7))), started, None, NOW, &who);
        assert!(likely.ends_with("invite created by @Kabir (likely)"), "{likely}");
        let vanity = line(Some(&join(How::Vanity, Some("mlci"), None)), started, None, NOW, &who);
        assert!(vanity.ends_with("via the server's vanity link"), "{vanity}");
        let mut unsure = join(How::Unsure, None, None);
        unsure.candidates = vec![
            Candidate { code: "a1".into(), inviter_id: Some(7), inviter_name: "Kabir".into(), vanity: false },
            Candidate { code: "b2".into(), inviter_id: Some(8), inviter_name: "Meera".into(), vanity: false },
        ];
        let unsure = line(Some(&unsure), started, None, NOW, &who);
        assert!(unsure.ends_with("via discord.gg/a1 or discord.gg/b2 — invite created by @Kabir or @8 (unsure)"), "{unsure}");
        let unknown = line(Some(&join(How::Unknown, None, None)), started, None, NOW, &who);
        assert!(unknown.contains("invite unknown"), "{unknown}");
    }

    #[test]
    fn a_join_before_tracking_began_is_not_known() {
        let who = |_: u64, _: &str| String::new();
        let started = NOW - 86_400;
        let before = line(None, started, Some(started - 400 * 86_400), NOW, &who);
        assert!(before.contains("before invite tracking began on"), "{before}");
        assert!(before.contains("not known") && before.contains("Server Settings → Members"), "{before}");
        assert!(!before.contains(" via "), "no guessing: {before}");
        // Joined since, but the bot has no record: unknown, not "before tracking".
        let missed = line(None, started, Some(NOW - 60), NOW, &who);
        assert!(missed.contains("didn't see this join"), "{missed}");
    }

    #[test]
    fn a_tally_counts_each_member_once_and_only_credited_joins() {
        let j = |id: i64, member: u64, how: How, by: Option<u64>, ts: i64| Join {
            id,
            new: NewJoin { member_id: member, joined_ts: ts, inviter_id: by, ..join(how, Some("x"), by) },
        };
        let joins = vec![
            j(1, 50, How::Sure, Some(7), NOW - 10),
            j(2, 51, How::Likely, Some(7), NOW - 5),
            j(3, 50, How::Sure, Some(7), NOW), // came back through Kabir again
            j(4, 52, How::Unsure, None, NOW),
            j(5, 53, How::Sure, Some(8), NOW),
        ];
        let t = tally(&joins);
        let kabir = t.iter().find(|t| t.inviter_id == 7).unwrap();
        assert_eq!(kabir.members.iter().map(|m| m.member_id).collect::<Vec<_>>(), vec![50, 51]);
        assert_eq!(kabir.members[0].joined_ts, NOW, "their latest join");
        assert_eq!(t.len(), 2, "an unsure join is nobody's");
    }

    #[test]
    fn still_here_follows_the_join_log() {
        assert!(still_here(0, Some("2026-09-01T00:00:00Z"), None));
        assert!(!still_here(1, Some("2026-09-01T00:00:00Z"), Some("2026-09-02T00:00:00Z")));
        assert!(still_here(1, Some("2026-09-03T00:00:00Z"), Some("2026-09-02T00:00:00Z")));
        assert!(!still_here(1, None, Some("2026-09-02T00:00:00")));
    }
}
