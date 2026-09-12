//! The four houses.
//!
//! Everyone on the server belongs to one of four houses, held as a Discord role
//! and a row in house.db. Nobody picks: the bot assigns. Members already here
//! are dealt out by the draft, balanced on how active they are, and anyone
//! arriving afterwards is sorted by the hat on the way in, with a card beside
//! their welcome. Mods stay out of it altogether, and they name a captain per
//! house.
//!
//! The row in house.db is what makes a returner keep their house: Discord
//! strips every role when someone leaves and hands none of them back.
//!
//! Sorting starts SHUT (`sorting_open`), so the commands and the roles ship
//! first and the server can position the roles and put the artwork on them
//! before anyone is sorted or anything is posted.
//!
//! Points come later: this module only decides who belongs where.

use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;

use chrono::{Datelike, Utc};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serenity::all::{
    ButtonStyle, ChannelId, ChannelType, CommandDataOptionValue, CommandInteraction, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateChannel, CreateCommandOption,
    CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    EditRole, GuildId, Member, PermissionOverwrite, PermissionOverwriteType, Permissions, RoleId, UserId,
};

use super::house_card::{self, Sorted};

/// The most one award may move a house, either way. Discord's option can't
/// carry a negative floor, so this is the guard against a mistyped award.
const MAX_AWARD: i64 = 100_000;

/// One captain role shared by all four houses - which house they captain is
/// already plain from the house role they wear.
const CAPTAIN_ROLE: &str = "House Captain";
const CAPTAIN_COLOUR: u32 = 0xE8B923;

pub struct House {
    pub key: &'static str,
    pub name: &'static str,
    pub crest: &'static str,
    /// Role colour, and the two card colours (primary, secondary).
    pub colour: u32,
    pub colours: ([u8; 3], [u8; 3]),
    /// What the hat says when it lands here.
    pub verdicts: &'static [&'static str],
}

pub const HOUSES: &[House] = &[
    House {
        key: "gryffindor",
        name: "Gryffindor",
        crest: "🦁",
        colour: 0x9B1B1B,
        colours: ([155, 27, 27], [232, 185, 35]),
        verdicts: &[
            "Brave to the point of trouble. GRYFFINDOR!",
            "First into the argument, last to back down. GRYFFINDOR!",
            "Big heart, no plan whatsoever. GRYFFINDOR!",
        ],
    },
    House {
        key: "slytherin",
        name: "Slytherin",
        crest: "🐍",
        colour: 0x1A6B4A,
        colours: ([26, 107, 74], [190, 195, 200]),
        verdicts: &[
            "Ambition, and the patience to use it. SLYTHERIN!",
            "Plans first, drama later. SLYTHERIN!",
            "Says nothing all day, then wins. SLYTHERIN!",
        ],
    },
    House {
        key: "ravenclaw",
        name: "Ravenclaw",
        crest: "🦅",
        colour: 0x1F4E8C,
        colours: ([31, 78, 140], [176, 122, 66]),
        verdicts: &[
            "A ready mind, and questions for everything. RAVENCLAW!",
            "Answers before the search engine does. RAVENCLAW!",
            "Talks less, knows far too much. RAVENCLAW!",
        ],
    },
    House {
        key: "hufflepuff",
        name: "Hufflepuff",
        crest: "🦡",
        colour: 0xC9A227,
        colours: ([201, 162, 39], [35, 35, 40]),
        verdicts: &[
            "Loyal, patient, and unafraid of the work. HUFFLEPUFF!",
            "Everyone's friend, everyone's backup. HUFFLEPUFF!",
            "No fights, just tea. HUFFLEPUFF!",
        ],
    },
];

pub fn house(key: &str) -> Option<&'static House> {
    HOUSES.iter().find(|h| h.key == key || h.name.eq_ignore_ascii_case(key))
}

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
/// One sorting at a time, so two arrivals can't race over the same rows.
static SORTING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("house.db"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS members (
             user_id INTEGER PRIMARY KEY, house TEXT NOT NULL, sorted_by TEXT NOT NULL, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS members_house ON members (house);
         CREATE TABLE IF NOT EXISTS awards (
             id INTEGER PRIMARY KEY AUTOINCREMENT, house TEXT NOT NULL, points INTEGER NOT NULL,
             reason TEXT NOT NULL, awarded_by INTEGER NOT NULL, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS awards_house_ts ON awards (house, ts);
         CREATE TABLE IF NOT EXISTS draft (
             user_id INTEGER PRIMARY KEY, house TEXT NOT NULL, seq INTEGER NOT NULL,
             done INTEGER NOT NULL DEFAULT 0);
         CREATE INDEX IF NOT EXISTS draft_todo ON draft (done, seq);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

fn meta_get(key: &str) -> Option<String> {
    let db = DB.get()?;
    db.lock().query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }
}

/// Whether the hat is open for business. It starts SHUT, so the commands and
/// the roles can be deployed - and the roles dragged up the list and given
/// their icons - before a single member is touched. Nothing sorts anyone and
/// nothing is posted until this is switched on.
fn sorting_open() -> bool {
    meta_get("sorting").as_deref() == Some("on")
}

fn meta_clear(key: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute("DELETE FROM meta WHERE key = ?1", params![key]);
    }
}

/// Which house someone is in, if they have been sorted.
pub fn house_of(user: u64) -> Option<&'static House> {
    let db = DB.get()?;
    let key: Option<String> = db
        .lock()
        .query_row("SELECT house FROM members WHERE user_id = ?1", params![user as i64], |r| r.get(0))
        .optional()
        .ok()
        .flatten();
    key.as_deref().and_then(house)
}

fn remember(user: u64, house: &House, sorted_by: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO members (user_id, house, sorted_by, ts) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(user_id) DO UPDATE SET house = excluded.house, sorted_by = excluded.sorted_by, ts = excluded.ts",
            params![user as i64, house.key, sorted_by, Utc::now().timestamp()],
        );
    }
}

/// How many members each house holds.
pub fn counts() -> HashMap<&'static str, i64> {
    let mut counts: HashMap<&'static str, i64> = HOUSES.iter().map(|h| (h.key, 0)).collect();
    if let Some(db) = DB.get() {
        let conn = db.lock();
        if let Ok(mut stmt) = conn.prepare("SELECT house, COUNT(*) FROM members GROUP BY house") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))) {
                for (key, n) in rows.flatten() {
                    if let Some(slot) = house(&key).and_then(|h| counts.get_mut(h.key)) {
                        *slot = n;
                    }
                }
            }
        }
    }
    counts
}

/// The hat's pick for a new arrival: whichever house is smallest, tossing a
/// coin between any that are tied.
///
/// Uniform random was the first cut, and it does not balance anything: three
/// arrivals in a row went to Ravenclaw, which is precisely what random does
/// and precisely what nobody wants to watch happen to houses that started
/// level. Smallest-first corrects drift the moment it appears, and stays
/// unguessable to the member, who cannot see the counts anyway.
fn hat_pick() -> &'static House {
    let counts = counts();
    let size = |house: &House| counts.get(house.key).copied().unwrap_or(0);
    let fewest = HOUSES.iter().map(size).min().unwrap_or(0);
    let tied: Vec<&'static House> = HOUSES.iter().filter(|house| size(house) == fewest).collect();
    tied.get(rand::random::<u32>() as usize % tied.len().max(1)).copied().unwrap_or(&HOUSES[0])
}

fn verdict(house: &'static House) -> String {
    house.verdicts[rand::random::<u32>() as usize % house.verdicts.len()].to_string()
}

// --- roles ------------------------------------------------------------------

/// Finds or creates a role by name, remembering its id - so renaming the role
/// on Discord doesn't make the bot build a second one beside it.
///
/// The role is made plain. Icons are attached by hand on the server, which is
/// also the only way to get artwork nobody has to argue with a program about.
async fn find_or_create(ctx: &Context, guild: GuildId, key: &str, name: &str, colour: u32) -> Option<RoleId> {
    let roles = guild.roles(&ctx.http).await.ok()?;
    if let Some(id) = meta_get(key).and_then(|v| v.parse::<u64>().ok()).map(RoleId::new) {
        if roles.contains_key(&id) {
            return Some(id);
        }
    }
    let id = match roles.values().find(|r| r.name.eq_ignore_ascii_case(name)) {
        Some(role) => role.id,
        None => {
            // Hoisted, so the member list groups by house - but NOT
            // mentionable: @Gryffindor would ping a quarter of the server.
            let builder = EditRole::new().name(name).colour(colour).hoist(true);
            guild.create_role(&ctx.http, builder).await.ok()?.id
        }
    };
    meta_set(key, &id.get().to_string());
    Some(id)
}

async fn role_for(ctx: &Context, guild: GuildId, house: &'static House) -> Option<RoleId> {
    find_or_create(ctx, guild, &format!("role_{}", house.key), house.name, house.colour).await
}

async fn captain_role(ctx: &Context, guild: GuildId) -> Option<RoleId> {
    find_or_create(ctx, guild, "role_captain", CAPTAIN_ROLE, CAPTAIN_COLOUR).await
}

/// Which house someone captains, if any.
fn captain_of(user: u64) -> Option<&'static House> {
    HOUSES
        .iter()
        .find(|h| meta_get(&format!("captain_{}", h.key)).and_then(|v| v.parse::<u64>().ok()) == Some(user))
}

/// Takes the captain role off someone who no longer captains anything.
async fn strip_captain_role(ctx: &Context, guild: GuildId, user: u64) {
    if captain_of(user).is_some() {
        return;
    }
    if let Some(role) = captain_role(ctx, guild).await {
        if let Ok(member) = guild.member(&ctx.http, UserId::new(user)).await {
            let _ = member.remove_role(&ctx.http, role).await;
        }
    }
}

/// Changing house costs you the captaincy of the house you left.
async fn drop_captaincy(ctx: &Context, guild: GuildId, user: u64, moving_to: &'static House) {
    let Some(was) = captain_of(user) else {
        return;
    };
    if was.key == moving_to.key {
        return;
    }
    meta_clear(&format!("captain_{}", was.key));
    strip_captain_role(ctx, guild, user).await;
}

/// Gives the house role and takes away the other three.
async fn wear_house(ctx: &Context, guild: GuildId, user: u64, house: &'static House) {
    let member = match guild.member(&ctx.http, UserId::new(user)).await {
        Ok(member) => member,
        Err(err) => {
            tracing::warn!("house: member {} not found: {}", user, err);
            return;
        }
    };
    for other in HOUSES.iter().filter(|h| h.key != house.key) {
        if let Some(role) = role_for(ctx, guild, other).await {
            if member.roles.contains(&role) {
                let _ = member.remove_role(&ctx.http, role).await;
            }
        }
    }
    if let Some(role) = role_for(ctx, guild, house).await {
        if let Err(err) = member.add_role(&ctx.http, role).await {
            tracing::warn!("house: {} role not given to {}: {}", house.name, user, err);
        }
    }
}

// --- the card ---------------------------------------------------------------

fn display(member: &Member) -> String {
    let name = member.display_name().to_string();
    if name.chars().count() > 24 { name.chars().take(23).collect::<String>() + "…" } else { name }
}

async fn avatar(member: &Member) -> Option<Vec<u8>> {
    let face = member.face().replace("size=1024", "size=256");
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().ok()?;
    client.get(face).send().await.ok()?.bytes().await.ok().map(|b| b.to_vec())
}

/// Drawing is CPU work, so it never runs on the gateway thread.
async fn card(sorted: Sorted) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || house_card::sorting_png(&sorted)).await.ok().flatten()
}

/// Sorts a new arrival, gives them the role, and announces it with the card.
async fn sort_member(ctx: &Context, guild: GuildId, member: &Member, channel: ChannelId) -> &'static House {
    let _one_at_a_time = SORTING.lock().await;
    let house = hat_pick();
    remember(member.user.id.get(), house, "hat");
    wear_house(ctx, guild, member.user.id.get(), house).await;
    announce(ctx, member, house, channel).await;
    house
}

/// Gives out the role and posts the card for a house already decided - by the
/// hat for an arrival, or by the draft for everyone who was already here.
async fn announce(ctx: &Context, member: &Member, house: &'static House, channel: ChannelId) {
    let user = member.user.id.get();
    let line = verdict(house);
    let sorted = Sorted {
        name: display(member),
        avatar: avatar(member).await,
        house: house.name.to_string(),
        crest: house.crest.to_string(),
        colours: house.colours,
        line: line.clone(),
    };
    let png = card(sorted).await;
    let mut message = CreateMessage::new()
        .content(format!("🎩 The hat has spoken: <@{}> joins **{} {}**!\n-# {}", user, house.crest, house.name, line));
    if let Some(png) = png {
        message = message.add_file(CreateAttachment::bytes(png, "sorted.png"));
    }
    if let Err(err) = channel.send_message(&ctx.http, message).await {
        tracing::warn!("house: sorting card not posted: {}", err);
    }
}

// --- commands ---------------------------------------------------------------

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

pub fn house_option(name: &str, about: &str) -> CreateCommandOption {
    let mut option = CreateCommandOption::new(serenity::all::CommandOptionType::String, name, about);
    for h in HOUSES {
        option = option.add_string_choice(format!("{} {}", h.crest, h.name), h.key);
    }
    option
}

/// `/houseroles` - mods only: create the four house roles and the captain
/// role, and put the crests on them.
///
/// The roles appear by themselves the first time someone is sorted, so this
/// isn't the only way in. It exists because the roles want making - and
/// dragging up the list - BEFORE anyone is sorted, which is the order the
/// server actually does it in. It also fixes up roles that already exist
/// without a crest, which creation alone can't.
pub async fn roles_command(ctx: &Context, command: &CommandInteraction) {
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.")).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can make the house roles.")).await;
        return;
    }
    // Five roles is more work than the three seconds Discord allows a reply.
    let thinking = CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true));
    let _ = command.create_response(&ctx.http, thinking).await;

    let mut lines = Vec::new();
    for house in HOUSES {
        match role_for(ctx, guild, house).await {
            Some(role) => lines.push(format!("{} <@&{}>", house.crest, role.get())),
            None => lines.push(format!("{} **{}** - could not be made", house.crest, house.name)),
        }
    }
    match captain_role(ctx, guild).await {
        Some(role) => lines.push(format!("🎖️ <@&{}>", role.get())),
        None => lines.push(format!("🎖️ **{}** - could not be made", CAPTAIN_ROLE)),
    }
    lines.push(
        "\n-# Add the icons by hand, then drag these above your decorative roles (still under me) - Discord shows \
         whichever icon and colour sits higher. Nobody is sorted until the hat is opened."
            .into(),
    );
    let reply = serenity::all::EditInteractionResponse::new().content(format!("**Houses**\n{}", lines.join("\n")));
    let _ = command.edit_response(&ctx.http, reply).await;
}

// --- common rooms -----------------------------------------------------------

/// The category the four common rooms live under.
const COMMON_ROOM_CATEGORY: &str = "The Houses";

/// Each house's own room: channel name and the topic that goes on it. Named
/// after where the houses actually live in the books.
const COMMON_ROOMS: [(&str, &str, &str); 4] = [
    ("gryffindor", "🦁│gryffindor-tower", "Gryffindor only. Behind the portrait of the Fat Lady."),
    ("slytherin", "🐍│slytherin-dungeons", "Slytherin only. Down past the dungeons, behind the bare stone wall."),
    ("ravenclaw", "🦅│ravenclaw-library", "Ravenclaw only. Up the spiral staircase, answer the riddle."),
    ("hufflepuff", "🦡│hufflepuff-kitchens", "Hufflepuff only. Tap the barrel in rhythm, by the kitchens."),
];

/// What a member of the house may do in their own room.
fn room_rights() -> Permissions {
    Permissions::VIEW_CHANNEL
        | Permissions::SEND_MESSAGES
        | Permissions::READ_MESSAGE_HISTORY
        | Permissions::ATTACH_FILES
        | Permissions::EMBED_LINKS
        | Permissions::ADD_REACTIONS
        | Permissions::USE_EXTERNAL_EMOJIS
}

/// Finds or creates the category the rooms sit under.
async fn common_room_category(ctx: &Context, guild: GuildId) -> Option<ChannelId> {
    let channels = guild.channels(&ctx.http).await.ok()?;
    if let Some(id) = meta_get("category").and_then(|v| v.parse::<u64>().ok()).map(ChannelId::new) {
        if channels.contains_key(&id) {
            return Some(id);
        }
    }
    let existing = channels
        .values()
        .find(|c| c.kind == ChannelType::Category && c.name.eq_ignore_ascii_case(COMMON_ROOM_CATEGORY))
        .map(|c| c.id);
    let id = match existing {
        Some(id) => id,
        None => {
            let builder = CreateChannel::new(COMMON_ROOM_CATEGORY).kind(ChannelType::Category);
            guild.create_channel(&ctx.http, builder).await.ok()?.id
        }
    };
    meta_set("category", &id.get().to_string());
    Some(id)
}

/// `/housechannels` - mods only: a private room per house.
///
/// Shut to everyone by default and opened only to the house's own role, to
/// every role that can moderate, and to the bot. Re-running adopts rooms that
/// already exist and resets their permissions, so it doubles as the repair.
pub async fn channels_command(ctx: &Context, command: &CommandInteraction) {
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.")).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can make the common rooms.")).await;
        return;
    }
    let thinking = CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true));
    let _ = command.create_response(&ctx.http, thinking).await;

    let roles = guild.roles(&ctx.http).await.unwrap_or_default();
    let powers = Permissions::ADMINISTRATOR
        | Permissions::MANAGE_GUILD
        | Permissions::MANAGE_ROLES
        | Permissions::MANAGE_MESSAGES
        | Permissions::KICK_MEMBERS
        | Permissions::BAN_MEMBERS
        | Permissions::MODERATE_MEMBERS;
    // Mods see every room. Managed roles are bots' own roles - skip those.
    let mod_roles: Vec<RoleId> = roles
        .values()
        .filter(|role| !role.managed && role.permissions.intersects(powers))
        .map(|role| role.id)
        .collect();
    let me = ctx.cache.current_user().id;
    let category = common_room_category(ctx, guild).await;
    let existing = guild.channels(&ctx.http).await.unwrap_or_default();

    let mut lines = Vec::new();
    for (key, name, topic) in COMMON_ROOMS {
        let Some(house) = house(key) else { continue };
        let Some(role) = role_for(ctx, guild, house).await else {
            lines.push(format!("{} **{}** - no role, so no room", house.crest, house.name));
            continue;
        };

        // Shut to the server, open to the house, its mods, and me.
        let mut overwrites = vec![
            PermissionOverwrite {
                allow: Permissions::empty(),
                deny: Permissions::VIEW_CHANNEL,
                kind: PermissionOverwriteType::Role(RoleId::new(guild.get())),
            },
            PermissionOverwrite {
                allow: room_rights(),
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Role(role),
            },
            PermissionOverwrite {
                allow: room_rights(),
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Member(me),
            },
        ];
        for id in &mod_roles {
            overwrites.push(PermissionOverwrite {
                allow: room_rights() | Permissions::MANAGE_MESSAGES,
                deny: Permissions::empty(),
                kind: PermissionOverwriteType::Role(*id),
            });
        }

        let known = meta_get(&format!("room_{}", key)).and_then(|v| v.parse::<u64>().ok()).map(ChannelId::new);
        let found = known
            .filter(|id| existing.contains_key(id))
            .or_else(|| existing.values().find(|c| c.name == name).map(|c| c.id));

        let id = match found {
            // Already there: just put the permissions back as they should be.
            Some(id) => {
                let mut edit = serenity::all::EditChannel::new().permissions(overwrites);
                if let Some(category) = category {
                    edit = edit.category(Some(category));
                }
                match id.edit(&ctx.http, edit).await {
                    Ok(_) => Some(id),
                    Err(err) => {
                        tracing::warn!("house: {} room not reset: {}", house.name, err);
                        Some(id)
                    }
                }
            }
            None => {
                let mut builder =
                    CreateChannel::new(name).kind(ChannelType::Text).topic(topic).permissions(overwrites);
                if let Some(category) = category {
                    builder = builder.category(category);
                }
                match guild.create_channel(&ctx.http, builder).await {
                    Ok(channel) => Some(channel.id),
                    Err(err) => {
                        tracing::warn!("house: {} room not made: {}", house.name, err);
                        None
                    }
                }
            }
        };
        match id {
            Some(id) => {
                meta_set(&format!("room_{}", key), &id.get().to_string());
                lines.push(format!("{} <#{}>", house.crest, id.get()));
            }
            None => lines.push(format!("{} **{}** - could not be made", house.crest, house.name)),
        }
    }

    let text = format!(
        "**Common rooms**\n{}\n-# Hidden from everyone but the house itself, {} mod role(s), and me. \
         Run this again any time to put the permissions back.",
        lines.join("\n"),
        mod_roles.len()
    );
    let _ = command.edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content(text)).await;
}

// --- the draft --------------------------------------------------------------

/// Between one member and the next. Discord tolerates about a message a
/// second into one channel, and each member is a role change plus a card.
const DRAFT_BEAT: Duration = Duration::from_millis(1200);
/// How often the running total is updated, in members.
const PROGRESS_EVERY: usize = 25;

/// Writes the plan down, replacing any previous one.
///
/// On disk rather than in memory so a restart half way through a twelve-minute
/// run picks up where it left off instead of sorting people twice.
fn store_plan(plan: &[(u64, &'static House)]) {
    if let Some(db) = DB.get() {
        let mut conn = db.lock();
        let Ok(tx) = conn.transaction() else {
            return;
        };
        let _ = tx.execute("DELETE FROM draft", []);
        for (seq, (user, house)) in plan.iter().enumerate() {
            let _ = tx.execute(
                "INSERT INTO draft (user_id, house, seq, done) VALUES (?1, ?2, ?3, 0)",
                params![*user as i64, house.key, seq as i64],
            );
        }
        let _ = tx.commit();
    }
}

/// The plan's remaining members, in draft order.
fn pending_plan() -> Vec<(u64, &'static House)> {
    let mut out = Vec::new();
    if let Some(db) = DB.get() {
        let conn = db.lock();
        if let Ok(mut stmt) = conn.prepare("SELECT user_id, house FROM draft WHERE done = 0 ORDER BY seq") {
            if let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, String>(1)?))) {
                for (user, key) in rows.flatten() {
                    if let Some(house) = house(&key) {
                        out.push((user, house));
                    }
                }
            }
        }
    }
    out
}

fn mark_drafted(user: u64) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute("UPDATE draft SET done = 1 WHERE user_id = ?1", params![user as i64]);
    }
}

/// How many of the plan are done, and how many there are.
fn plan_progress() -> (i64, i64) {
    let Some(db) = DB.get() else {
        return (0, 0);
    };
    let conn = db.lock();
    let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    (count("SELECT COUNT(*) FROM draft WHERE done = 1"), count("SELECT COUNT(*) FROM draft"))
}

/// Works out who goes where, without touching anybody.
///
/// Only ever plans members who have NO house yet, so running it a second time
/// tops up the people who were missed instead of re-sorting the whole server.
///
/// Returns the plan in draft order, plus how many were passed over: bots, mods,
/// and those already in a house.
async fn build_plan(ctx: &Context, guild: GuildId) -> (Vec<(u64, &'static House)>, usize, usize, usize) {
    // The role map is fetched ONCE: asking per member would be hundreds of
    // calls just to answer "is this a mod".
    let roles = guild.roles(&ctx.http).await.unwrap_or_default();
    let mut eligible: Vec<Member> = Vec::new();
    let (mut bots, mut mods, mut settled) = (0usize, 0usize, 0usize);
    let mut after: Option<UserId> = None;
    loop {
        let page = match guild.members(&ctx.http, Some(1000), after).await {
            Ok(page) => page,
            Err(err) => {
                tracing::warn!("house: member list failed: {}", err);
                break;
            }
        };
        for member in &page {
            if member.user.bot {
                bots += 1;
            } else if is_mod_with(&roles, member) {
                mods += 1;
            } else if house_of(member.user.id.get()).is_some() {
                // Already has a house - leave them alone. This is what makes a
                // second run a top-up rather than a re-sort of the server.
                settled += 1;
            } else {
                eligible.push(member.clone());
            }
        }
        if page.len() < 1000 {
            break;
        }
        after = page.last().map(|m| m.user.id);
    }

    // Everyone gets a place in the order, whether or not they have ever said
    // anything: the activity only decides WHERE in the order they land.
    let since = Utc::now().timestamp() - 60 * 24 * 3600;
    let measured = super::house_draft::activity(since);
    let mut people: HashMap<u64, super::house_draft::Activity> = HashMap::new();
    for member in &eligible {
        let id = member.user.id.get();
        people.insert(id, measured.get(&id).copied().unwrap_or_default());
    }

    let order = super::house_draft::ranked(&people);
    let mut held = counts();
    let plan: Vec<(u64, &'static House)> = if held.values().sum::<i64>() == 0 {
        // Nobody sorted yet: deal the whole server serpentine, which is what
        // makes the four houses come out evenly matched rather than merely
        // equal in size.
        let piles = super::house_draft::snake(&order, HOUSES.len());
        let mut house_of_user: HashMap<u64, &'static House> = HashMap::new();
        for (index, pile) in piles.iter().enumerate() {
            if let Some(house) = HOUSES.get(index) {
                for user in pile {
                    house_of_user.insert(*user, house);
                }
            }
        }
        // Back into draft order, so the run posts strongest first.
        order.iter().filter_map(|user| house_of_user.get(user).map(|house| (*user, *house))).collect()
    } else {
        // A top-up. Serpentine is wrong here - it deals as though the houses
        // were empty - so each newcomer goes to whichever house is behind,
        // strongest newcomer first.
        order
            .iter()
            .map(|user| {
                let house = HOUSES
                    .iter()
                    .min_by_key(|house| (held.get(house.key).copied().unwrap_or(0), house.key))
                    .unwrap_or(&HOUSES[0]);
                *held.entry(house.key).or_insert(0) += 1;
                (*user, house)
            })
            .collect()
    };
    (plan, bots, mods, settled)
}

fn draft_buttons() -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new("housedraft:go").label("Sort them").style(ButtonStyle::Success),
        CreateButton::new("housedraft:redraw").label("Redraw").style(ButtonStyle::Secondary),
        CreateButton::new("housedraft:cancel").label("Cancel").style(ButtonStyle::Secondary),
    ])]
}

/// The table for a plan: where everyone would land, and the proof it is even.
fn draft_preview(plan: &[(u64, &'static House)], bots: usize, mods: usize, settled: usize) -> CreateEmbed {
    let order: Vec<u64> = plan.iter().map(|(user, _)| *user).collect();
    let piles: Vec<Vec<u64>> = HOUSES
        .iter()
        .map(|house| plan.iter().filter(|(_, theirs)| theirs.key == house.key).map(|(user, _)| *user).collect())
        .collect();
    let weights = super::house_draft::weights(&order, &piles);

    let mut text = String::new();
    for (index, house) in HOUSES.iter().enumerate() {
        text.push_str(&format!(
            "{} **{}** — {} members · strength {}\n",
            house.crest,
            house.name,
            piles.get(index).map(|p| p.len()).unwrap_or(0),
            weights.get(index).copied().unwrap_or(0)
        ));
    }
    let spread = match (weights.iter().min(), weights.iter().max()) {
        (Some(low), Some(high)) => high - low,
        _ => 0,
    };
    let first: Vec<String> = plan.iter().take(6).map(|(user, house)| format!("{} <@{}>", house.crest, user)).collect();
    let already = if settled > 0 {
        format!("\n-# {} members already have a house and are left exactly as they are.", settled)
    } else {
        String::new()
    };
    text.push_str(&format!(
        "\n**{}** to sort · {} bots and {} mods passed over{}\n\
         -# Strength is the sum of draft positions - the closer those four numbers, the more even the houses. \
         Spread here is {}.\n-# First picks: {}\n-# Roughly {} minutes of posting, one card each.",
        plan.len(),
        bots,
        mods,
        already,
        spread,
        first.join(" · "),
        (plan.len() as f64 * DRAFT_BEAT.as_secs_f64() / 60.0).ceil() as u64
    ));
    CreateEmbed::new().title("🎩 The Sorting - nothing has happened yet").description(text).colour(0x9B1B1B)
}

/// `/housedraft` - mods only: work out the plan and show it. Sorts nobody.
pub async fn draft_command(ctx: &Context, command: &CommandInteraction) {
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.")).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can run the draft.")).await;
        return;
    }
    if meta_get("drafting").as_deref() == Some("on") {
        let (done, all) = plan_progress();
        let text = format!("A sorting is already running - {} of {} done.", done, all);
        let _ = command.create_response(&ctx.http, whisper(text)).await;
        return;
    }
    // Walking the roster and reading three databases takes longer than the
    // three seconds Discord gives a reply.
    let thinking = CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true));
    let _ = command.create_response(&ctx.http, thinking).await;

    let (plan, bots, mods, settled) = build_plan(ctx, guild).await;
    if plan.is_empty() {
        let text = if settled > 0 {
            format!("Nothing to do - all {} of them already have a house.", settled)
        } else {
            "Nobody to sort - the member list came back empty, or everyone is a bot or a mod.".to_string()
        };
        let _ = command.edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content(text)).await;
        return;
    }
    store_plan(&plan);
    let reply = serenity::all::EditInteractionResponse::new()
        .embed(draft_preview(&plan, bots, mods, settled))
        .components(draft_buttons());
    let _ = command.edit_response(&ctx.http, reply).await;
}

/// Sorts everyone in the stored plan, paced, and says so as it goes.
///
/// Spawned, never awaited by an interaction: this runs for minutes.
async fn run_draft(ctx: Context, guild: GuildId, channel: ChannelId) {
    meta_set("drafting", "on");
    // From here on an arrival gets the hat too, so nobody joining mid-sorting
    // is left houseless.
    meta_set("sorting", "on");

    let plan = pending_plan();
    let (already, total) = plan_progress();
    tracing::info!("house: sorting {} members into houses ({} already done)", plan.len(), already);

    let opening = format!("🎩 **The Sorting begins.** {} members to place.", plan.len());
    let mut progress = channel.send_message(&ctx.http, CreateMessage::new().content(opening)).await.ok();

    let mut placed = already as usize;
    for (user, house) in plan {
        if meta_get("drafting").as_deref() != Some("on") {
            tracing::info!("house: sorting stopped after {} members", placed);
            break;
        }
        match guild.member(&ctx.http, UserId::new(user)).await {
            Ok(member) => {
                remember(user, house, "draft");
                wear_house(&ctx, guild, user, house).await;
                announce(&ctx, &member, house, channel).await;
            }
            // Left between the plan and their turn: skip, don't stall.
            Err(err) => tracing::warn!("house: {} could not be sorted: {}", user, err),
        }
        mark_drafted(user);
        placed += 1;
        if placed % PROGRESS_EVERY == 0 {
            if let Some(message) = &mut progress {
                let text = format!("🎩 **The Sorting** — {} of {} placed.", placed, total);
                let edit = serenity::all::EditMessage::new().content(text);
                let _ = message.edit(&ctx.http, edit).await;
            }
        }
        tokio::time::sleep(DRAFT_BEAT).await;
    }

    meta_clear("drafting");
    let counts = counts();
    let mut tally = String::new();
    for house in HOUSES {
        tally.push_str(&format!("{} **{}** — {}\n", house.crest, house.name, counts.get(house.key).copied().unwrap_or(0)));
    }
    let finished = CreateEmbed::new()
        .title("🏰 The Sorting is done")
        .description(format!("{}\n-# {} members placed. `/houselist` to see a house.", tally, placed))
        .colour(0x9B1B1B);
    let _ = channel.send_message(&ctx.http, CreateMessage::new().embed(finished)).await;
    tracing::info!("house: sorting finished, {} placed", placed);
}

/// Picks a half-finished sorting back up after a restart.
pub fn resume_draft(ctx: &Context, guild: GuildId) {
    if meta_get("drafting").as_deref() != Some("on") {
        return;
    }
    let Some(channel) = cards_channel() else {
        tracing::warn!("house: a sorting was interrupted but no house channel is set");
        return;
    };
    let ctx = ctx.clone();
    tokio::spawn(async move { run_draft(ctx, guild, channel).await });
}

// --- points -----------------------------------------------------------------

/// Midnight on the 1st, India time: the month the table is counted over.
fn month_start() -> i64 {
    let ist = super::stats::ist();
    Utc::now()
        .with_timezone(&ist)
        .date_naive()
        .with_day(1)
        .and_then(|first| first.and_hms_opt(0, 0, 0))
        .and_then(|midnight| midnight.and_local_timezone(ist).single())
        .map(|t| t.timestamp())
        .unwrap_or(0)
}

/// Points per house since `since`, or all time for `None`.
///
/// Every award is its own row, so a month's table is a sum over a date range,
/// a mistake is undone by awarding the negative, and nothing has to be reset
/// when the month turns.
fn totals(since: Option<i64>) -> HashMap<&'static str, i64> {
    let mut out: HashMap<&'static str, i64> = HOUSES.iter().map(|h| (h.key, 0)).collect();
    if let Some(db) = DB.get() {
        let conn = db.lock();
        if let Ok(mut stmt) = conn.prepare("SELECT house, SUM(points) FROM awards WHERE ts >= ?1 GROUP BY house") {
            let rows = stmt.query_map(params![since.unwrap_or(0)], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            });
            if let Ok(rows) = rows {
                for (key, sum) in rows.flatten() {
                    if let Some(slot) = house(&key).and_then(|h| out.get_mut(h.key)) {
                        *slot = sum;
                    }
                }
            }
        }
    }
    out
}

/// Records an award and gives back the house's new total for the month.
fn award(house: &House, points: i64, reason: &str, by: u64) -> i64 {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO awards (house, points, reason, awarded_by, ts) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![house.key, points, reason, by as i64, Utc::now().timestamp()],
        );
    }
    totals(Some(month_start())).get(house.key).copied().unwrap_or(0)
}

/// `/housepoints <house> <points> [reason]` - mods only. A negative number
/// takes points away, which is also how a mistaken award is undone.
pub async fn points_command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can award points.")).await;
        return;
    }
    let mut chosen = None;
    let mut points = 0i64;
    let mut reason = String::new();
    for option in &command.data.options {
        match (&option.name[..], &option.value) {
            ("house", CommandDataOptionValue::String(key)) => chosen = house(key),
            ("points", CommandDataOptionValue::Integer(n)) => points = *n,
            ("reason", CommandDataOptionValue::String(text)) => reason = text.trim().to_string(),
            _ => {}
        }
    }
    let Some(house) = chosen else {
        let _ = command.create_response(&ctx.http, whisper("Which house? Use `/housepoints`.")).await;
        return;
    };
    if points == 0 {
        let _ = command.create_response(&ctx.http, whisper("Zero points would do nothing.")).await;
        return;
    }
    // Discord can't express a negative floor on the option, so the bound lives
    // here: a slip of the keyboard shouldn't put a house on a million points.
    if !(-MAX_AWARD..=MAX_AWARD).contains(&points) {
        let text = format!("That's more than {} points - award it in smaller pieces if you mean it.", MAX_AWARD);
        let _ = command.create_response(&ctx.http, whisper(text)).await;
        return;
    }
    let total = award(house, points, &reason, command.user.id.get());
    let headline = if points > 0 {
        format!("🏆 **+{}** to {} **{}**", points, house.crest, house.name)
    } else {
        format!("📉 **{}** from {} **{}**", points, house.crest, house.name)
    };
    let because = if reason.is_empty() { String::new() } else { format!("\n> {}", reason) };
    let text = format!(
        "{}{}\n-# {} now on **{}** points this month · awarded by <@{}>",
        headline,
        because,
        house.name,
        total,
        command.user.id.get()
    );
    let reply = CreateInteractionResponseMessage::new().content(text);
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

/// `/houses` - who holds what, and the captains.
pub async fn houses_command(ctx: &Context, command: &CommandInteraction) {
    let counts = counts();
    let points = totals(Some(month_start()));
    // The table reads as a table: whoever is ahead this month sits on top.
    let mut order: Vec<&House> = HOUSES.iter().collect();
    order.sort_by(|a, b| points.get(b.key).cmp(&points.get(a.key)).then(a.name.cmp(b.name)));

    let mut text = String::new();
    let scoring = points.values().any(|p| *p != 0);
    for (place, h) in order.iter().enumerate() {
        let captain = meta_get(&format!("captain_{}", h.key))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|id| format!(" · 🎖️ <@{}>", id))
            .unwrap_or_default();
        let lead = if place == 0 && scoring { " 👑" } else { "" };
        text.push_str(&format!(
            "{} **{}**{} — **{}** points · {} members{}\n",
            h.crest,
            h.name,
            lead,
            points.get(h.key).copied().unwrap_or(0),
            counts.get(h.key).copied().unwrap_or(0),
            captain
        ));
    }
    let total: i64 = counts.values().sum();
    text.push_str(&format!("\n-# {} members sorted · points counted from the 1st, India time", total));
    let embed = CreateEmbed::new()
        .title("🏰 The four houses")
        .description(text)
        .colour(0x9B1B1B)
        .footer(CreateEmbedFooter::new("Points start counting once the system is switched on"));
    let reply = CreateInteractionResponseMessage::new().embed(embed);
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

/// Members per page in `/houselist`. Thirty mentions is a page you can read
/// without scrolling, and ~190 members comes to seven pages.
const LIST_PAGE: usize = 30;

/// Everyone in a house, in the order they were sorted - which for the draft's
/// intake means strongest first.
fn members_of(key: &str) -> Vec<u64> {
    let mut out = Vec::new();
    if let Some(db) = DB.get() {
        let conn = db.lock();
        if let Ok(mut stmt) = conn.prepare("SELECT user_id FROM members WHERE house = ?1 ORDER BY ts, user_id") {
            if let Ok(rows) = stmt.query_map(params![key], |r| r.get::<_, i64>(0)) {
                out.extend(rows.flatten().map(|id| id as u64));
            }
        }
    }
    out
}

/// One page of a house's roll, and how many pages there are altogether.
fn list_page(house: &'static House, page: usize) -> (CreateEmbed, usize) {
    let members = members_of(house.key);
    let pages = members.len().div_ceil(LIST_PAGE).max(1);
    let page = page.min(pages - 1);
    let captain = meta_get(&format!("captain_{}", house.key)).and_then(|v| v.parse::<u64>().ok());

    let mut text = String::new();
    for (place, user) in members.iter().enumerate().skip(page * LIST_PAGE).take(LIST_PAGE) {
        let mark = if Some(*user) == captain { " 🎖️" } else { "" };
        text.push_str(&format!("{}. <@{}>{}\n", place + 1, user, mark));
    }
    if members.is_empty() {
        text.push_str("Nobody yet.");
    }
    let embed = CreateEmbed::new()
        .title(format!("{} {} - {} members", house.crest, house.name, members.len()))
        .description(text)
        .colour(house.colour)
        .footer(CreateEmbedFooter::new(format!("Page {} of {}", page + 1, pages)));
    (embed, pages)
}

/// Prev/Next for the roll. Ids carry the house and the page, so the buttons
/// keep working after a restart with nothing held in memory.
fn list_buttons(house: &'static House, page: usize, pages: usize) -> Vec<CreateActionRow> {
    if pages <= 1 {
        return Vec::new();
    }
    let back = CreateButton::new(format!("houselist:{}:{}", house.key, page.saturating_sub(1)))
        .label("Back")
        .style(ButtonStyle::Secondary)
        .disabled(page == 0);
    let next = CreateButton::new(format!("houselist:{}:{}", house.key, page + 1))
        .label("Next")
        .style(ButtonStyle::Secondary)
        .disabled(page + 1 >= pages);
    vec![CreateActionRow::Buttons(vec![back, next])]
}

/// `/houselist <house>` - who is in a house, privately, a page at a time.
pub async fn list_command(ctx: &Context, command: &CommandInteraction) {
    let chosen = command.data.options.iter().find_map(|o| match &o.value {
        CommandDataOptionValue::String(key) => house(key),
        _ => None,
    });
    let Some(house) = chosen else {
        let _ = command.create_response(&ctx.http, whisper("Which house? Use `/houselist`.")).await;
        return;
    };
    let (embed, pages) = list_page(house, 0);
    // Ephemeral, and with mentions switched off: a roll of 190 names must not
    // ping 190 people.
    let reply = CreateInteractionResponseMessage::new()
        .embed(embed)
        .components(list_buttons(house, 0, pages))
        .allowed_mentions(CreateAllowedMentions::new())
        .ephemeral(true);
    let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
}

/// Buttons under a house roll, and under the draft's preview.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let mut parts = component.data.custom_id.split(':');
    match parts.next() {
        Some("houselist") => list_component(ctx, component, parts).await,
        Some("housedraft") => draft_component(ctx, component, parts.next().unwrap_or("")).await,
        _ => {}
    }
}

/// Sort them / Redraw / Cancel on the draft preview.
async fn draft_component(ctx: &Context, component: &ComponentInteraction, action: &str) {
    let Some(guild) = component.guild_id else {
        return;
    };
    if !super::admin_ids().contains(&component.user.id.get()) {
        let reply = CreateInteractionResponseMessage::new().content("Only mods can do that.").ephemeral(true);
        let _ = component.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
        return;
    }
    match action {
        "go" => {
            let plan = pending_plan();
            if plan.is_empty() {
                let reply = CreateInteractionResponseMessage::new()
                    .content("Nothing left to sort - run `/housedraft` again for a fresh plan.")
                    .components(Vec::new());
                let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(reply)).await;
                return;
            }
            let channel = cards_channel().unwrap_or(component.channel_id);
            let text = format!(
                "🎩 Sorting **{}** members into houses, in <#{}>. Roughly {} minutes.\n\
                 -# Arrivals from now on get the hat as they join.",
                plan.len(),
                channel.get(),
                (plan.len() as f64 * DRAFT_BEAT.as_secs_f64() / 60.0).ceil() as u64
            );
            let reply = CreateInteractionResponseMessage::new().content(text).embeds(Vec::new()).components(Vec::new());
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(reply)).await;
            let ctx = ctx.clone();
            tokio::spawn(async move { run_draft(ctx, guild, channel).await });
        }
        "redraw" => {
            let thinking = CreateInteractionResponse::Acknowledge;
            let _ = component.create_response(&ctx.http, thinking).await;
            let (plan, bots, mods, settled) = build_plan(ctx, guild).await;
            if plan.is_empty() {
                return;
            }
            store_plan(&plan);
            let edit = serenity::all::EditInteractionResponse::new()
                .embed(draft_preview(&plan, bots, mods, settled))
                .components(draft_buttons());
            let _ = component.edit_response(&ctx.http, edit).await;
        }
        "cancel" => {
            if let Some(db) = DB.get() {
                let _ = db.lock().execute("DELETE FROM draft", []);
            }
            let reply = CreateInteractionResponseMessage::new()
                .content("Dropped. Nobody was sorted.")
                .embeds(Vec::new())
                .components(Vec::new());
            let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(reply)).await;
        }
        _ => {}
    }
}

/// The Back/Next buttons under a house roll.
async fn list_component<'a>(
    ctx: &Context,
    component: &ComponentInteraction,
    mut parts: impl Iterator<Item = &'a str>,
) {
    let Some(house) = parts.next().and_then(house) else {
        return;
    };
    let page = parts.next().and_then(|p| p.parse::<usize>().ok()).unwrap_or(0);
    let (embed, pages) = list_page(house, page);
    let reply = CreateInteractionResponseMessage::new()
        .embed(embed)
        .components(list_buttons(house, page.min(pages - 1), pages))
        .allowed_mentions(CreateAllowedMentions::new());
    let _ = component.create_response(&ctx.http, CreateInteractionResponse::UpdateMessage(reply)).await;
}

/// `/housecaptain @member` - mods only. The member keeps their own house.
pub async fn captain_command(ctx: &Context, command: &CommandInteraction) {
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.")).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can name a captain.")).await;
        return;
    }
    let target = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id.get()),
        _ => None,
    });
    let Some(target) = target else {
        let _ = command.create_response(&ctx.http, whisper("Who? Use `/housecaptain @member`.")).await;
        return;
    };
    let Some(house) = house_of(target) else {
        let _ = command
            .create_response(&ctx.http, whisper("They haven't been sorted yet - they need `/sortme` first."))
            .await;
        return;
    };
    // Set the new captain first, so the outgoing one is only stripped of the
    // role if they don't captain some other house.
    let outgoing = meta_get(&format!("captain_{}", house.key)).and_then(|v| v.parse::<u64>().ok());
    meta_set(&format!("captain_{}", house.key), &target.to_string());
    if let Some(outgoing) = outgoing.filter(|old| *old != target) {
        strip_captain_role(ctx, guild, outgoing).await;
    }
    match (captain_role(ctx, guild).await, guild.member(&ctx.http, UserId::new(target)).await) {
        (Some(role), Ok(member)) => {
            if let Err(err) = member.add_role(&ctx.http, role).await {
                tracing::warn!("house: captain role not given to {}: {}", target, err);
            }
        }
        _ => tracing::warn!("house: captain role not given to {}", target),
    }
    let _ = command
        .create_response(
            &ctx.http,
            CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(format!(
                "🎖️ <@{}> is now captain of **{} {}**.",
                target, house.crest, house.name
            ))),
        )
        .await;
}

/// `/sort @member house` - mods only: place or move someone by hand.
pub async fn sort_command(ctx: &Context, command: &CommandInteraction) {
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("This only works in a server.")).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can sort someone by hand.")).await;
        return;
    }
    let mut target = None;
    let mut chosen = None;
    for option in &command.data.options {
        match &option.value {
            CommandDataOptionValue::User(id) => target = Some(id.get()),
            CommandDataOptionValue::String(key) => chosen = house(key),
            _ => {}
        }
    }
    let (Some(target), Some(chosen)) = (target, chosen) else {
        let _ = command.create_response(&ctx.http, whisper("Use `/sort @member house`.")).await;
        return;
    };
    drop_captaincy(ctx, guild, target, chosen).await;
    remember(target, chosen, &format!("mod:{}", command.user.id.get()));
    wear_house(ctx, guild, target, chosen).await;
    let _ = command
        .create_response(
            &ctx.http,
            whisper(format!("Done - <@{}> is in **{} {}**.", target, chosen.crest, chosen.name)),
        )
        .await;
}

/// Where the sorting cards go. The houses channel if one is set, otherwise
/// wherever the caller was going to put it.
fn cards_channel() -> Option<ChannelId> {
    std::env::var("VIZIER_HOUSE_CHANNEL").ok()?.trim().parse().ok().map(ChannelId::new)
}

/// Mods stay out of the houses entirely (user's call): no house, no card, and
/// later no points - they are the ones running the competition. "Mod" means
/// the bot's own admin list, or any role that can moderate.
async fn is_mod(ctx: &Context, guild: GuildId, member: &Member) -> bool {
    match guild.roles(&ctx.http).await {
        Ok(roles) => is_mod_with(&roles, member),
        Err(_) => false,
    }
}

/// The same test against a role map fetched once. The draft walks hundreds of
/// members and must not ask Discord for the role list each time.
fn is_mod_with(roles: &HashMap<RoleId, serenity::all::Role>, member: &Member) -> bool {
    if super::admin_ids().contains(&member.user.id.get()) {
        return true;
    }
    let powers = Permissions::ADMINISTRATOR
        | Permissions::MANAGE_GUILD
        | Permissions::MANAGE_ROLES
        | Permissions::MANAGE_MESSAGES
        | Permissions::KICK_MEMBERS
        | Permissions::BAN_MEMBERS
        | Permissions::MODERATE_MEMBERS;
    member.roles.iter().any(|id| roles.get(id).is_some_and(|role| role.permissions.intersects(powers)))
}

/// A new arrival gets sorted straight away, card and all. Someone who left and
/// came back keeps the house they already had - their row outlives the leaving.
pub async fn on_join(ctx: &Context, member: &Member, welcome: ChannelId) {
    if member.user.bot {
        return;
    }
    // Someone coming back. Discord strips every role on the way out and hands
    // none of them back, so the remembered house has to be PUT BACK ON - the
    // row alone would leave them counted in a house they cannot see. Not a
    // sorting: no card, no fanfare, just their colours returned. Deliberately
    // not behind `sorting_open`, since restoring what someone already had is
    // never the thing we are holding back.
    if let Some(house) = house_of(member.user.id.get()) {
        wear_house(ctx, member.guild_id, member.user.id.get(), house).await;
        tracing::info!("house: {} came back, {} role restored", member.user.name, house.name);
        return;
    }
    if !sorting_open() {
        return;
    }
    if is_mod(ctx, member.guild_id, member).await {
        tracing::info!("house: {} is a mod, left unsorted", member.user.name);
        return;
    }
    // The card goes where the welcome just went. An arrival is a moment for
    // the room that greeted them; the houses channel is for the bulk sorting.
    let house = sort_member(ctx, member.guild_id, member, welcome).await;
    tracing::info!("house: sorted {} into {}", member.user.name, house.name);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn four_houses_each_with_a_crest_a_colour_and_verdicts() {
        assert_eq!(HOUSES.len(), 4);
        for h in HOUSES {
            assert!(!h.verdicts.is_empty(), "{} has no verdict", h.name);
            assert!(h.crest.chars().count() == 1, "{} crest should be one emoji", h.name);
            assert!(h.colour > 0, "{} has no colour", h.name);
            assert!(house(h.key).is_some() && house(h.name).is_some(), "{} not findable", h.name);
        }
        assert!(house("Hogwarts").is_none());
    }

    #[test]
    fn the_points_month_starts_at_midnight_on_the_first_india_time() {
        use chrono::Timelike;
        let ist = chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset");
        let start = chrono::DateTime::from_timestamp(month_start(), 0).expect("a real time").with_timezone(&ist);
        assert_eq!(start.day(), 1, "the month should start on the 1st");
        assert_eq!((start.hour(), start.minute(), start.second()), (0, 0, 0), "at midnight");
        assert!(month_start() <= Utc::now().timestamp(), "the month cannot start in the future");
    }

    #[test]
    fn the_hat_can_reach_every_house_and_every_verdict() {
        // With no database open every house reads as empty, so all four are
        // tied and the coin toss decides - which is what this exercises.
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for _ in 0..2_000 {
            *seen.entry(hat_pick().key).or_default() += 1;
            let house = hat_pick();
            assert!(house.verdicts.contains(&verdict(house).as_str()), "verdict came from another house");
        }
        assert_eq!(seen.len(), HOUSES.len(), "some house is unreachable: {:?}", seen);
        // Nothing like a strict balance check - it is a coin toss by design -
        // but a house that never comes up would be a modulo bug.
        assert!(seen.values().all(|n| *n > 200), "one house is starved: {:?}", seen);
    }
}
