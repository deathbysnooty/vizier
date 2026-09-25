//! `/ship`: the bot reads two members together, from what it can actually see -
//! their game records, their time in voice, how often AutoMod eats them, who
//! they are always replying to, and what the two of them have said at each
//! other.
//!
//! It only ever posts in the roast channel (`VIZIER_ROAST_CHANNEL`). Run
//! anywhere else, the result still lands there and the place it was run in gets
//! a line with a link to it. The two being shipped are pinged there, so nobody
//! finds out second-hand - whoever asked is named in the footer and doesn't
//! need telling about their own command.
//!
//! `/ship` posts a drawn card (`roast_card.rs`): both avatars, the ship name,
//! the score, a bar coloured by it, one counted fact about the pair, and which
//! way the score has moved since they were last shipped. That one fact is
//! always something anyone in the channel could have noticed - a count of
//! replies, time in voice, a shared channel, or the plain absence of any of it.
//!
//! The score itself is worked out in `ship_score.rs`, from shares rather than
//! totals: how much of each other's replying and voice time the two of them
//! actually get, how alike their hours, channels and games are, and their
//! back-and-forths against their kalesh - all of it against each person's own
//! nightly sheet of counts (`ship_sheet.rs`). What is left is the pair's own
//! number from their two ids, and a pair the bot knows nothing about is nothing
//! but that number. `ship_score::WHAT_MOVES_IT` says the whole of it in English.
//!
//! The rules and the check every model answer has to pass live in
//! `roast_build.rs`, with the tests. `/noroast` is the opt-out, and it is a
//! different thing from `/forgetme`: that one is about member notes, and
//! neither touches the other.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use rusqlite::params;
use parking_lot::Mutex;
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions,
    CreateAttachment, CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse, User, UserId,
};

use super::control;
use super::kalesh;
use super::kalesh_store;
use super::notes_facts::Facts;
use super::roast_card::{self, Face};
use super::roast_build::{self as build, Dossier, Made, Together};

/// Where `/ship` posts unless the panel says otherwise.
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
/// One profile picture, at most, and how long one try at it may take.
const AVATAR_BYTES: usize = 2 * 1024 * 1024;
const AVATAR_WAIT: Duration = Duration::from_secs(4);
/// Both pictures together, at most. Whatever hasn't arrived by then is drawn
/// as an initial instead: the card never holds the command up.
const AVATAR_BUDGET: Duration = Duration::from_secs(6);
/// How long a picture, and a failed try at one, are remembered.
const AVATAR_TTL: Duration = Duration::from_secs(6 * 3600);
const AVATAR_MISS_TTL: Duration = Duration::from_secs(600);
const AVATAR_CACHE_MAX: usize = 256;

// --- settings ----------------------------------------------------------------------------------

pub fn ship_on() -> bool {
    control::on("VIZIER_SHIP", true)
}

/// The one channel `/ship` posts in.
pub fn channel() -> u64 {
    control::id("VIZIER_ROAST_CHANNEL").unwrap_or(DEFAULT_CHANNEL)
}

/// Members who have put themselves out of reach of `/ship`. Kept apart from the
/// member-notes opt-out on purpose: `/forgetme` is about notes.
pub fn optouts() -> Vec<u64> {
    control::ids("VIZIER_ROAST_OPTOUTS")
}

/// A model on the bot's own provider; empty means the bot's usual one.
pub fn model_name() -> Option<String> {
    control::var("VIZIER_ROAST_MODEL")
}

/// How long a gathered dossier, and a pair's interaction summary, are reused
/// before being read again. The model is still asked every time, so the words
/// are always new; this only saves re-reading the databases in a burst. Zero
/// switches the reuse off.
pub fn cache_window() -> Duration {
    Duration::from_secs(control::number("VIZIER_ROAST_CACHE_MINS", 10).clamp(0, 180) * 60)
}

// --- what has already been read -----------------------------------------------------------------

/// Gathered dossiers and pair summaries, in memory only: cleared on restart,
/// swept whenever one is written, and never bigger than this.
const GATHER_CACHE_MAX: usize = 512;

fn dossier_cache() -> &'static Mutex<HashMap<u64, (Instant, Dossier)>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, (Instant, Dossier)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn pair_cache() -> &'static Mutex<HashMap<(u64, u64), (Instant, Together)>> {
    static CACHE: OnceLock<Mutex<HashMap<(u64, u64), (Instant, Together)>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn sweep<K: Eq + std::hash::Hash, V>(map: &mut HashMap<K, (Instant, V)>, now: Instant, window: Duration) {
    map.retain(|_, (at, _)| now.duration_since(*at) < window);
    if map.len() >= GATHER_CACHE_MAX {
        map.clear();
    }
}

/// The same summary read from the other side: only the two reply counts are
/// one-directional, everything else is the same either way round.
fn flipped(t: &Together) -> Together {
    Together { replies_ab: t.replies_ba, replies_ba: t.replies_ab, ..t.clone() }
}

/// What this pair scored last time, as a line for the card, and this score
/// written down in its place. `None` when they have never been shipped, when
/// the number hasn't moved, or when the store isn't open.
fn remember_score(a: u64, b: u64, percent: u8) -> Option<String> {
    let db = super::roast_store::db()?;
    let now = Utc::now().timestamp();
    let conn = db.lock();
    let moved = build::movement(percent, super::roast_store::last(&conn, a, b), now);
    if let Err(err) = super::roast_store::record(&conn, a, b, percent, now) {
        tracing::warn!("roast: the score for {} and {} wasn't written down: {}", a, b, err);
    }
    moved
}

// --- the model ------------------------------------------------------------------------------------

/// One call to the ship model. The retry that matters for a dropped
/// connection is inside the model itself (`never_sent`); this one waits, and
/// `build::make` asks a second time if what comes back is no good.
async fn ask(prompt: String) -> anyhow::Result<String> {
    match tokio::time::timeout(MODEL_WAIT, control::web::ask_bot_model_with(prompt, model_name())).await {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!("the model took over {}s", MODEL_WAIT.as_secs())),
    }
}

// --- reading the record ------------------------------------------------------------------------------

/// How many of their messages AutoMod blocked, and how many were deleted.
fn gone_counts(user: u64) -> (i64, i64) {
    let Some(reader) = super::msglog::reader() else { return (0, 0) };
    let conn = reader.conn.lock();
    let count = |sql: &str| conn.query_row(sql, params![user as i64], |r| r.get::<_, i64>(0)).unwrap_or(0);
    (count("SELECT COUNT(*) FROM blocked WHERE author_id = ?1"), count("SELECT COUNT(*) FROM deleted WHERE author_id = ?1"))
}

fn dossier_from(id: u64, name: &str, f: &Facts, now: i64, channels: &HashMap<u64, String>, members: &HashMap<u64, String>) -> Dossier {
    let (blocked, deleted) = gone_counts(id);
    Dossier {
        id,
        name: name.to_string(),
        // Off their role on the server, never out of the model's head.
        pronouns: super::pronouns::of(id),
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
    }
}

/// Everything the bot knows about one member, ready for a prompt. Reused for
/// `cache_window()` so a burst of ships doesn't read every database again;
/// the name is always taken fresh, since that is the cheap part.
async fn dossier(ctx: &Context, storage: &std::sync::Arc<crate::storage::VizierStorage>, user: u64, name: &str) -> Dossier {
    let (window, at) = (cache_window(), Instant::now());
    if !window.is_zero() {
        if let Some((read_at, d)) = dossier_cache().lock().get(&user).cloned() {
            if at.duration_since(read_at) < window {
                tracing::debug!("roast: reusing what was read about {} {:?} ago", user, at.duration_since(read_at));
                // The name and the pronouns are both cheap and both live.
                return Dossier { name: name.to_string(), pronouns: super::pronouns::of(user), ..d };
            }
        }
    }
    let now = Utc::now().timestamp();
    let (channels, members, words) = super::notes::cache_names(ctx);
    let facts = super::notes::facts_for(ctx, storage, user, words).await;
    let d = dossier_from(user, name, &facts, now, &channels, &members);
    if !window.is_zero() {
        let mut cache = dossier_cache().lock();
        sweep(&mut cache, at, window);
        cache.insert(user, (at, d.clone()));
    }
    d
}

/// How the two of them are around each other, read once and reused for
/// `cache_window()`. Always gathered for the lower id first, so asking the
/// pair the other way round costs nothing.
async fn pair_together(a: u64, b: u64, channels: HashMap<u64, String>) -> Together {
    let (lo, hi) = (a.min(b), a.max(b));
    let (window, at) = (cache_window(), Instant::now());
    let facing = |t: Together| if a == lo { t } else { flipped(&t) };
    if !window.is_zero() {
        if let Some((read_at, t)) = pair_cache().lock().get(&(lo, hi)).cloned() {
            if at.duration_since(read_at) < window {
                return facing(t);
            }
        }
    }
    let now_ms = Utc::now().timestamp_millis();
    let t = tokio::task::spawn_blocking(move || together_of(lo, hi, now_ms, &channels)).await.unwrap_or_default();
    if !window.is_zero() {
        let mut cache = pair_cache().lock();
        sweep(&mut cache, at, window);
        cache.insert((lo, hi), (at, t.clone()));
    }
    facing(t)
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
    // Time in the same voice room, counted exactly the way voice points are.
    if let Some(db) = super::stats::db() {
        let now = now_ms / 1000;
        let since = now - SHIP_PERIOD_MS / 1000;
        match super::activity::voice_pairs(&db.lock(), since, now, now) {
            Ok(pairs) => {
                let (lo, hi) = (a.min(b), a.max(b));
                out.vc_minutes = pairs.iter().find(|p| p.a == lo && p.b == hi).map(|p| p.together / 60).unwrap_or(0);
            }
            Err(err) => tracing::warn!("roast: couldn't read voice time for {} and {}: {}", a, b, err),
        }
    }
    out
}

// --- profile pictures -------------------------------------------------------------------------------

/// Pictures already downloaded, so a second `/ship` in the same hour doesn't
/// fetch them again. Kept the way the panel keeps its member cache: a map with
/// a time on every entry, swept when it grows. A failed fetch is remembered
/// too, briefly, so a member with a broken avatar isn't retried every time.
type Cached = (Instant, Option<Arc<Vec<u8>>>);

fn avatar_cache() -> &'static Mutex<HashMap<String, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Cached>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

fn ttl(bytes: &Option<Arc<Vec<u8>>>) -> Duration {
    if bytes.is_some() { AVATAR_TTL } else { AVATAR_MISS_TTL }
}

/// One picture off Discord's CDN, capped in size and in time.
async fn download(url: &str) -> Option<Arc<Vec<u8>>> {
    let client = reqwest::Client::builder().timeout(AVATAR_WAIT).build().ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    if resp.content_length().is_some_and(|n| n > AVATAR_BYTES as u64) {
        tracing::warn!("roast: a profile picture was too big to draw ({:?} bytes)", resp.content_length());
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    if bytes.len() > AVATAR_BYTES {
        return None;
    }
    Some(Arc::new(bytes.to_vec()))
}

async fn fetch_avatar(url: String) -> Option<Arc<Vec<u8>>> {
    let now = Instant::now();
    if let Some((at, bytes)) = avatar_cache().lock().get(&url).cloned() {
        if now.duration_since(at) < ttl(&bytes) {
            return bytes;
        }
    }
    let got = download(&url).await;
    let mut cache = avatar_cache().lock();
    if cache.len() >= AVATAR_CACHE_MAX {
        cache.retain(|_, (at, bytes)| now.duration_since(*at) < ttl(bytes));
        if cache.len() >= AVATAR_CACHE_MAX {
            cache.clear();
        }
    }
    cache.insert(url, (now, got.clone()));
    got
}

/// Discord's own picture for an account that has none of its own.
fn default_face(user: u64) -> String {
    format!("https://cdn.discordapp.com/embed/avatars/{}.png", (user >> 22) % 6)
}

fn add_face(out: &mut Vec<String>, url: String) {
    if !out.contains(&url) {
        out.push(url);
    }
}

/// Every picture worth trying for this member, best first, ending with
/// Discord's default so there is always something to fall back to.
fn face_urls(ctx: &Context, command: &CommandInteraction, user: u64) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let small = |u: &User| u.face().replace("size=1024", "size=256");
    if command.user.id.get() == user {
        add_face(&mut out, small(&command.user));
    }
    if let Some(u) = command.data.resolved.users.get(&UserId::new(user)) {
        add_face(&mut out, small(u));
    }
    for guild in ctx.cache.guilds() {
        if let Some(url) = ctx.cache.guild(guild).and_then(|g| g.members.get(&UserId::new(user)).map(|m| m.face().replace("size=1024", "size=256"))) {
            add_face(&mut out, url);
        }
    }
    add_face(&mut out, default_face(user));
    out
}

/// The first of those that comes back. `None` means the card draws an initial.
async fn avatar_of(urls: Vec<String>) -> Option<Vec<u8>> {
    for url in urls {
        if let Some(bytes) = fetch_avatar(url).await {
            return Some(bytes.as_ref().clone());
        }
    }
    None
}

// --- the card ------------------------------------------------------------------------------------

/// The ship card's message. With a picture the number lives in the picture;
/// without one - when drawing failed - it is written out instead, exactly as
/// it used to be.
fn ship_embed(a: &Dossier, b: &Dossier, ship: &str, percent: u8, verdict: &str, by: &str, with_card: bool) -> CreateEmbed {
    let embed = CreateEmbed::new()
        .title(format!("💘 {}", ship))
        .description(format!("{}\n\n{}", build::score_line(&a.name, &b.name, percent, with_card), verdict))
        .colour(roast_card::heat(percent).into_colour())
        .footer(CreateEmbedFooter::new(format!("shipped by {} · a bot's joke, nothing more · /noroast to stay out of these", by)));
    if with_card { embed.image(format!("attachment://{}", roast_card::FILE)) } else { embed }
}

/// The score's own colour for the embed's stripe, so the card and the bar down
/// the side agree.
trait AsColour {
    fn into_colour(self) -> u32;
}

impl AsColour for [u8; 3] {
    fn into_colour(self) -> u32 {
        ((self[0] as u32) << 16) | ((self[1] as u32) << 8) | self[2] as u32
    }
}

/// Posts the card in the roast channel and tells the place the command was run
/// in where it went. `Err` is a line to show the person, never a raw error.
async fn post(ctx: &Context, command: &CommandInteraction, embed: CreateEmbed, ping: &[u64], card: Option<Vec<u8>>) -> Result<(), String> {
    let home = channel();
    let mut message = CreateMessage::new()
        .content(build::ping_line(ping))
        .embed(embed)
        .allowed_mentions(CreateAllowedMentions::new().users(ping.iter().copied().map(UserId::new).collect::<Vec<_>>()));
    if let Some(png) = card {
        message = message.add_file(CreateAttachment::bytes(png, roast_card::FILE));
    }
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

// --- /ship ------------------------------------------------------------------------------------------

pub fn ship_builder() -> CreateCommand {
    CreateCommand::new("ship")
        // Discord allows 100 characters here, and this is the only help most
        // people will read: say plainly that it is shares, not a count and not
        // a roll. The whole of it is in /help and on the panel.
        .description("ship two members - the share of each other's replies and time, not who talks most")
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
    let mut t = pair_together(a, b, channels).await;
    t.shared_games = da.games.iter().map(|(g, _)| g.clone()).filter(|g| db.games.iter().any(|(o, _)| o == g)).collect();
    // The shares come off the two nightly sheets, so both halves of every share
    // were counted the same way on the same pass. A member with no sheet reads
    // as nothing known, and their pair's own number simply shows through.
    let (side_a, side_b, _, _) = super::ship_sheet::sides(a, b);
    let scored = build::ship_score(a, b, &side_a, &side_b, &t.between());
    let percent = scored.percent;
    let ship = build::ship_name(&name_a, &name_b);
    let prompt = build::ship_prompt(&da, &db, &t, &scored, &ship);
    // What they scored last time, so the card can say which way it has moved.
    // Remembered now: the score is already decided, whatever the model does.
    let moved = remember_score(a, b, percent);
    tracing::info!(
        "roast: {} shipped {} and {} ({}% = base {} {:+}{}; {:?}), {}, about {} tokens",
        caller,
        a,
        b,
        percent,
        scored.base,
        scored.moved(),
        if scored.capped { ", capped as one-sided" } else { "" },
        scored.told(),
        moved.as_deref().unwrap_or("no change"),
        build::prompt_tokens(&prompt)
    );
    // The pictures are fetched while the model writes, so they cost nothing the
    // command wasn't already waiting for, and whatever hasn't come by then is
    // simply not drawn.
    let (faces_a, faces_b) = (face_urls(ctx, command, a), face_urls(ctx, command, b));
    let (made, avatars) = tokio::join!(
        build::make(prompt, build::check_ship, ask),
        tokio::time::timeout(AVATAR_BUDGET, futures::future::join(avatar_of(faces_a), avatar_of(faces_b))),
    );
    let Made::Ok { text, .. } = made else {
        give_up(ctx, command, build::failure_message(&made)).await;
        return;
    };
    let (avatar_a, avatar_b) = avatars.unwrap_or_else(|_| {
        tracing::info!("roast: the profile pictures took too long; the card draws initials instead");
        (None, None)
    });
    let card = roast_card::Card {
        ship: ship.clone(),
        percent,
        left: Face { name: da.name.clone(), avatar: avatar_a },
        right: Face { name: db.name.clone(), avatar: avatar_b },
        line: build::headline(&t, SHIP_PERIOD_MS / 86_400_000),
        moved,
    };
    // Drawing is CPU work, and a picture that won't draw must not cost the
    // command: `spawn_blocking` also catches a panic in the renderer, and the
    // message then goes out as text, with the number written in.
    let png = tokio::task::spawn_blocking(move || roast_card::png(&card)).await.unwrap_or_else(|err| {
        tracing::warn!("roast: the ship card didn't draw: {}", err);
        None
    });
    if png.is_none() {
        tracing::warn!("roast: no ship card for {} and {}; posting the text version", a, b);
    }
    let by = command.user.display_name().to_string();
    let embed = ship_embed(&da, &db, &ship, percent, &text, &by, png.is_some());
    // A ship pings the two being shipped, and only them: whoever asked is named
    // in the footer, and doesn't need telling about their own command.
    if let Err(err) = post(ctx, command, embed, &[a, b], png).await {
        give_up(ctx, command, &err).await;
    }
}

// --- /noroast ---------------------------------------------------------------------------------------

pub fn noroast_builder() -> CreateCommand {
    CreateCommand::new("noroast").description("keep yourself out of /ship - run it again to come back in")
}

const OUT: &str = "Done - nobody can `/ship` you while you're out, and nothing of yours goes to the AI for it. \
    Run `/noroast` again to come back in.\n\
    -# This is separate from `/forgetme`: that one is about member notes, and it hasn't changed.";
const IN: &str = "You're back in - `/ship` can name you again. Brace yourself.\n\
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
        // And the command the owner asked for exists, with the options it needs.
        let ship = serde_json::to_value(ship_builder()).unwrap();
        assert_eq!(ship["name"], "ship");
        assert_eq!(ship["options"][0]["name"], "member");
        assert_eq!(ship["options"][0]["required"], true);
        assert_eq!(ship["options"][1]["name"], "with");
        assert_eq!(ship["options"][1]["required"], false, "the second member is optional: it's the caller when left out");
        // /noroast now keeps you out of /ship, and says so.
        assert!(OUT.contains("`/ship`") && !OUT.contains("`/roast`"), "{OUT}");
        assert!(IN.contains("`/ship`") && !IN.contains("`/roast`"), "{IN}");
        assert!(OUT.contains("separate from `/forgetme`") && IN.contains("different opt-out"));
        // The opt-out list lives in the panel's settings; the notes list in notes.db.
        assert!(optouts().is_empty(), "nothing saved, nobody out");
    }

    /// Reading is reused for a few minutes; the AI is not, so the words are
    /// always new. The window is a setting, and 0 switches the reuse off.
    #[test]
    fn what_was_read_is_reused_for_a_window_that_can_be_switched_off() {
        assert_eq!(cache_window(), Duration::from_secs(600), "ten minutes with nothing set");
        assert!(!cache_window().is_zero(), "reuse is on by default");
        assert!(dossier_cache().lock().is_empty() && pair_cache().lock().is_empty(), "nothing is remembered across restarts");
        // A pair read one way round answers the other way round too, with only
        // the two reply counts swapped.
        let t = build::tests::together();
        let back = flipped(&t);
        assert_eq!((back.replies_ab, back.replies_ba), (t.replies_ba, t.replies_ab));
        assert_eq!((back.fights, back.mentions, back.stretches, &back.shared_channels), (t.fights, t.mentions, t.stretches, &t.shared_channels));
        assert_eq!(flipped(&back), t, "and back again");
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
    fn the_card_says_what_it_is_and_how_to_get_out() {
        let d = build::tests::arjun();
        // With a card, the number lives in the picture and the message just
        // carries the verdict.
        let verdict = "they argue in #chess and call it a hobby";
        let with_card = serde_json::to_value(ship_embed(&d, &build::tests::riya(), "Arjya", 41, verdict, "dev", true)).unwrap();
        assert_eq!(with_card["title"], "💘 Arjya");
        assert_eq!(with_card["image"]["url"], format!("attachment://{}", roast_card::FILE));
        let body = with_card["description"].as_str().unwrap();
        assert!(body.contains("**arjun** × **riya**") && body.contains(verdict), "{body}");
        assert!(!body.contains('%') && !body.contains('█'), "the picture carries the number: {body}");
        assert_eq!(with_card["color"], roast_card::heat(41).into_colour(), "the stripe matches the bar");
        assert!(with_card["footer"]["text"].as_str().unwrap().contains("a bot's joke"));
        // Drawing failed: the same message, with the score written back in and
        // no attachment named.
        let text_only = serde_json::to_value(ship_embed(&d, &build::tests::riya(), "Arjya", 41, verdict, "dev", false)).unwrap();
        assert!(text_only["image"].is_null(), "nothing is attached, so nothing is pointed at");
        let body = text_only["description"].as_str().unwrap();
        assert!(body.contains("**41%**") && body.contains("█") && body.contains(verdict), "{body}");
    }
}
