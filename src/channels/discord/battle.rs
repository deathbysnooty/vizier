//! /fight and /battle - the arena.
//!
//! `/fight @someone` works in any channel: the challenge card goes up in the
//! fight channel and the caller's channel gets a link to it. `/battle` is for
//! admins, opens a lobby for as many minutes as they ask for, pings the warrior
//! role, and then knocks the joiners out in pairs until one is left. Winners are
//! a coin toss - this is banter, not a ladder - and the last one standing wears
//! the champion role until the next battle.
//!
//! Fights live in memory; only results go to battle.db, so a restart loses an
//! open lobby but never the record of who won.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serenity::all::{
    ButtonStyle, ChannelId, CommandDataOptionValue, CommandInteraction, ComponentInteraction, Context,
    CreateActionRow, CreateAllowedMentions, CreateAttachment, CreateButton, CreateEmbed, CreateEmbedFooter,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage, EditAttachments, EditMessage, EditRole,
    GuildId, Member, MessageId, RoleId, UserId,
};

use super::battle_card::{self, Champion, Fight, Fighter, Outcome};

/// Fight channel, from `VIZIER_FIGHT_CHANNEL`; otherwise found by name.
const CHANNEL_NAME: &str = "fight-fight-fight";
/// Pinged when a battle opens. Created if the server has no such role.
const WARRIOR_ROLE: &str = "Warrior";
/// Worn by the last battle's winner.
const CHAMPION_ROLE: &str = "Battle Champion";
/// Health both fighters start a fight with.
const START_HP: i32 = 100;
/// Exchanges before the fight is called on health left, so nobody waits forever.
/// Blows land on either side at random, so a fight usually runs about eight.
const MAX_EXCHANGES: usize = 14;
/// Between exchanges of one fight. Short, because a battle is many fights.
const BEAT: Duration = Duration::from_secs(2);
/// Between two fights of the same round.
const FIGHT_GAP: Duration = Duration::from_secs(3);
/// Between rounds.
const ROUND_GAP: Duration = Duration::from_secs(6);
/// A challenge nobody answers expires.
const CHALLENGE_WAIT: Duration = Duration::from_secs(120);
/// Between two `/fight`s by the same member.
const FIGHT_COOLDOWN: Duration = Duration::from_secs(60);
/// Lobby length an admin may ask for.
const MIN_WAIT: i64 = 1;
const MAX_WAIT: i64 = 15;
/// Fewer joiners than this and the battle is called off.
const MIN_PLAYERS: usize = 4;
/// Discord takes a while over each card, so keep a battle under a few minutes.
const MAX_PLAYERS: usize = 32;

static DB: OnceLock<Mutex<Connection>> = OnceLock::new();
/// Open lobbies, by the lobby message id.
static LOBBIES: LazyLock<Mutex<HashMap<u64, Lobby>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Open challenges, by the challenge message id.
static CHALLENGES: LazyLock<Mutex<HashMap<u64, Challenge>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Channels with a battle or fight running, so two never overlap.
static BUSY: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Last `/fight` per member, for the cooldown.
static LAST_FIGHT: LazyLock<Mutex<HashMap<u64, std::time::Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

struct Lobby {
    joined: Vec<u64>,
    names: HashMap<u64, String>,
    open: bool,
    /// Kept so a join can redraw the lobby with its own countdown.
    ends: i64,
    minutes: i64,
}

struct Challenge {
    from: u64,
    to: u64,
    accepted: Option<bool>,
}

/// A fighter with everything the card needs.
#[derive(Clone)]
struct Warrior {
    id: u64,
    name: String,
    avatar: Option<Vec<u8>>,
}

impl Warrior {
    fn card(&self, hp: i32) -> Fighter {
        Fighter {
            name: self.name.clone(),
            avatar: self.avatar.clone(),
            hp: hp.max(0) as u32,
            max_hp: START_HP as u32,
        }
    }
}

// --- lines ------------------------------------------------------------------
// Roasts stay on the fight itself: nothing about looks, family, caste or faith.

const EXCHANGE: &[&str] = &[
    "{a} ne {b} ko chappal dikhayi 🩴",
    "{b} ne {a} ki DP screenshot kar li, blackmail ki taiyari",
    "{a} ne {b} ko ek dum se 'bhai sun' bola aur block kar diya",
    "{b} ne {a} pe pura jug paani daal diya",
    "{a} ne {b} ki keyboard ke keycaps nikaal diye",
    "{b} ne {a} ko VC mein ghaseet liya, mic mute karke",
    "{a} ne {b} ko 'seen' kar diya, reply nahi bheja",
    "{b} ne {a} ka WiFi router unplug kar diya",
    "{a} ne {b} pe tagda meme daga",
    "{b} ne {a} ko typing... typing... pe 10 minute rakha",
    "{a} ne {b} ki maggi bina namak ke bana di",
    "{b} ne {a} ko 'tera match to Jio pe hi atka hai' bola",
    "{a} ne {b} ka phone 1% battery pe chhod diya",
    "{b} ne {a} ki chai mein cheeni double kar di",
    "{a} ne {b} ko group se remove karke wapas add kiya, sirf dikhane ke liye",
    "{b} ne {a} ka last seen chhupa diya",
    "{a} ne {b} ko ludo mein teen baar chhakka maar ke hara diya",
    "{b} ne {a} ki playlist mein sirf sad songs bhar diye",
    "{a} ne {b} ka chair khinch liya, classic",
    "{b} ne {a} ko 'aur bata' bolke mool baat hi nahi batayi",
];

const FINISH: &[&str] = &[
    "{w} ne {l} ko bola 'ja beta ja, jee le apni zindagi' 🏆",
    "{l} out. {w} ne bina pasina bahaye jeet liya",
    "{w} ki jeet, {l} ka 'main next round mein aata hoon' wala excuse",
    "{l} ne dramatic exit liya, {w} ne wave karke bhej diya",
    "{w} bacha, {l} gaya - aur haan, screenshot le liya gaya hai",
    "{l} ka game over. {w} ne victory dance bhi kar liya",
    "{w} ne finishing move maara: silent treatment. {l} khatam",
    "{l} ne haar maan li, {w} ne chai ka cup uthaya",
    "{w} jeeta. {l} ab commentary karega",
    "{l} ko {w} ne exit ka darwaza dikha diya 🚪",
];

const CRIT: &[&str] = &[
    "{a} ne {b} pe poora combo chala diya, bina saans liye 💥",
    "{a} ka jhakaas headshot - {b} ka WiFi tak hil gaya",
    "{a} ne {b} ko ek hi taane mein udaa diya",
    "{a} ne {b} ki puri chat history nikal ke padh di 😳",
    "{a} ne {b} ko uske hi meme se maara",
];

const MISS: &[&str] = &[
    "{a} ne haath ghumaya... aur hawa mein reh gaya",
    "{a} ka taana miss, {b} ne duck kar liya",
    "{a} laga raha tha ki ye landega - nahi landa",
];

const HEAL: &[&str] = &[
    "{a} ne chai ka ghoont liya, thodi jaan wapas aayi ☕",
    "{a} ne maggi khaayi aur fresh ho gaya 🍜",
    "{a} ne Hanuman Chalisa laga di, thodi power aayi",
];

const BYE: &[&str] = &[
    "{a} ko is round mein koi mila hi nahi, chai peene chala gaya ☕",
    "{a} free pass le ke agle round mein, kismat wala hai",
    "{a} ka opponent aaya hi nahi, walkover",
];

const CHAMPION_LINE: &[&str] = &[
    "Poore server ko akele nipta diya 👑",
    "Sabko chappal dikha ke taj pehen liya",
    "Aaj ka don yahi hai. Baaki sab commentary box mein",
    "Undisputed. Baaki log next battle ka wait karein",
];

/// A tiny xorshift keeps rolls spread without dragging rand into here.
fn roll(seed: &mut u64, n: u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed % n.max(1)
}

fn pick<'a>(pool: &'a [&'a str], seed: &mut u64) -> &'a str {
    pool[roll(seed, pool.len() as u64) as usize]
}

/// What an exchange turned out to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Blow {
    Miss,
    Crit,
    Heal,
    Hit,
}

impl Blow {
    fn lines(self) -> &'static [&'static str] {
        match self {
            Blow::Miss => MISS,
            Blow::Crit => CRIT,
            Blow::Heal => HEAL,
            Blow::Hit => EXCHANGE,
        }
    }
}

/// One exchange: what happened, the change to health, and who it lands on.
/// Most swings hurt; a few miss, and now and then someone recovers.
fn exchange(seed: &mut u64, attacker: usize) -> (Blow, i32, usize) {
    let other = 1 - attacker;
    match roll(seed, 100) {
        0..=9 => (Blow::Miss, 0, other),
        10..=24 => (Blow::Crit, -(32 + roll(seed, 12) as i32), other),
        25..=33 => (Blow::Heal, 6 + roll(seed, 7) as i32, attacker),
        _ => (Blow::Hit, -(18 + roll(seed, 13) as i32), other),
    }
}

fn fill(line: &str, a: &str, b: &str) -> String {
    line.replace("{a}", a).replace("{b}", b).replace("{w}", a).replace("{l}", b)
}

// --- store ------------------------------------------------------------------

pub fn open(workspace: &str) -> anyhow::Result<()> {
    let dir = crate::utils::build_path(workspace, &[".runtime"]);
    std::fs::create_dir_all(&dir)?;
    let conn = Connection::open(dir.join("battle.db"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS wins (
             id INTEGER PRIMARY KEY AUTOINCREMENT, user_id INTEGER NOT NULL, kind TEXT NOT NULL,
             beat INTEGER NOT NULL DEFAULT 0, ts INTEGER NOT NULL);
         CREATE INDEX IF NOT EXISTS wins_user ON wins (user_id, kind);
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )?;
    let _ = DB.set(Mutex::new(conn));
    Ok(())
}

fn meta_get(key: &str) -> Option<String> {
    let db = DB.get()?;
    let conn = db.lock();
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0)).optional().ok().flatten()
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = DB.get() {
        let _ = db.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        );
    }
}

/// Records a win and returns how many of that kind the winner now has.
fn record_win(user: u64, kind: &str, beat: usize) -> i64 {
    let Some(db) = DB.get() else {
        return 0;
    };
    let conn = db.lock();
    let _ = conn.execute(
        "INSERT INTO wins (user_id, kind, beat, ts) VALUES (?1, ?2, ?3, ?4)",
        params![user as i64, kind, beat as i64, Utc::now().timestamp()],
    );
    conn.query_row("SELECT COUNT(*) FROM wins WHERE user_id = ?1 AND kind = ?2", params![user as i64, kind], |r| {
        r.get(0)
    })
    .unwrap_or(0)
}

// --- channel and roles ------------------------------------------------------

/// The arena channel: `VIZIER_FIGHT_CHANNEL`, else the channel named
/// `fight-fight-fight`, else wherever the command was used.
async fn arena(ctx: &Context, guild: GuildId, fallback: ChannelId) -> ChannelId {
    if let Some(id) = std::env::var("VIZIER_FIGHT_CHANNEL").ok().and_then(|v| v.trim().parse::<u64>().ok()) {
        return ChannelId::new(id);
    }
    if let Ok(channels) = guild.channels(&ctx.http).await {
        if let Some(found) = channels.values().find(|c| c.name == CHANNEL_NAME) {
            return found.id;
        }
    }
    fallback
}

/// Finds a role by name, creating it when the server has none.
async fn role_named(ctx: &Context, guild: GuildId, name: &str, colour: u32, key: &str) -> Option<RoleId> {
    let roles = guild.roles(&ctx.http).await.ok()?;
    if let Some(id) = meta_get(key).and_then(|v| v.parse::<u64>().ok()).map(RoleId::new) {
        if roles.contains_key(&id) {
            return Some(id);
        }
    }
    let wanted = name.to_lowercase();
    let id = match roles.values().find(|r| r.name.to_lowercase() == wanted) {
        Some(role) => role.id,
        None => {
            let builder = EditRole::new().name(name).colour(colour).hoist(false).mentionable(true);
            guild.create_role(&ctx.http, builder).await.ok()?.id
        }
    };
    meta_set(key, &id.get().to_string());
    Some(id)
}

async fn warrior_role(ctx: &Context, guild: GuildId) -> Option<RoleId> {
    role_named(ctx, guild, WARRIOR_ROLE, 0xE67E22, "warrior_role").await
}

/// Moves the champion role to the winner, taking it off whoever held it.
async fn crown(ctx: &Context, guild: GuildId, winner: u64) {
    let Some(role) = role_named(ctx, guild, CHAMPION_ROLE, 0xF1C40F, "champion_role").await else {
        return;
    };
    if let Some(previous) = meta_get("champion").and_then(|v| v.parse::<u64>().ok()) {
        if previous != winner {
            if let Ok(member) = guild.member(&ctx.http, UserId::new(previous)).await {
                let _ = member.remove_role(&ctx.http, role).await;
            }
        }
    }
    if let Ok(member) = guild.member(&ctx.http, UserId::new(winner)).await {
        if let Err(err) = member.add_role(&ctx.http, role).await {
            tracing::warn!("battle: champion role not given: {}", err);
            return;
        }
    }
    meta_set("champion", &winner.to_string());
}

// --- fighters ---------------------------------------------------------------

fn display(member: &Member) -> String {
    let name = member.display_name().to_string();
    if name.chars().count() > 22 { name.chars().take(21).collect::<String>() + "…" } else { name }
}

async fn warrior(ctx: &Context, guild: GuildId, user: u64) -> Option<Warrior> {
    let member = guild.member(&ctx.http, UserId::new(user)).await.ok()?;
    let face = member.face().replace("size=1024", "size=256");
    let avatar = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .ok()?
        .get(face)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()
        .map(|b| b.to_vec());
    Some(Warrior { id: user, name: display(&member), avatar })
}

/// Drawing a card is CPU work, so it never runs on the gateway thread.
async fn fight_card(
    stage: String,
    left: Fighter,
    right: Fighter,
    line: String,
    outcome: Outcome,
    hit: Option<(usize, i32)>,
) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        battle_card::fight_png(&Fight { stage, left: &left, right: &right, line, outcome, hit })
    })
    .await
    .ok()
    .flatten()
}

async fn champion_card(who: Fighter, subtitle: String, line: String) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || battle_card::champion_png(&Champion { who: &who, subtitle, line }))
        .await
        .ok()
        .flatten()
}

/// One fight: the card goes up, the exchanges land under it, then the result.
/// Returns the winner.
async fn play(ctx: &Context, channel: ChannelId, stage: &str, a: &Warrior, b: &Warrior, seed: &mut u64) -> Warrior {
    let mut hp = [START_HP; 2];
    let open =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), String::new(), Outcome::Open, None).await;
    let head = format!("**{}** · <@{}> vs <@{}>", stage, a.id, b.id);
    let mut log: Vec<String> = Vec::new();
    let mut text = head.clone();
    let mut message = {
        let mut msg = CreateMessage::new().content(&text).allowed_mentions(CreateAllowedMentions::new());
        if let Some(png) = open {
            msg = msg.add_file(CreateAttachment::bytes(png, "fight.png"));
        }
        match channel.send_message(&ctx.http, msg).await {
            Ok(m) => m,
            Err(err) => {
                tracing::warn!("battle: fight card not sent: {}", err);
                return if *seed % 2 == 0 { a.clone() } else { b.clone() };
            }
        }
    };

    // Trade blows until someone's health runs out. What lands is luck, not skill.
    let mut turns = 0;
    while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
        turns += 1;
        tokio::time::sleep(BEAT).await;
        let attacker = roll(seed, 2) as usize;
        let (x, y) = if attacker == 0 { (a, b) } else { (b, a) };
        let (blow, delta, target) = exchange(seed, attacker);
        hp[target] = (hp[target] + delta).clamp(0, START_HP);
        let line = fill(pick(blow.lines(), seed), &x.name, &y.name);
        let shown = match delta {
            0 => format!("{} · **miss**", line),
            d if d > 0 => format!("{} · **+{} HP**", line, d),
            d => format!("{} · **{} HP**", line, d),
        };
        log.push(shown.clone());
        text = fight_text(&head, &log, a, b, &hp);
        let card =
            fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), shown, Outcome::Open, Some((target, delta)))
                .await;
        let mut edit = EditMessage::new().content(&text);
        if let Some(png) = card {
            edit = edit.attachments(EditAttachments::new().add(CreateAttachment::bytes(png, "fight.png")));
        }
        let _ = message.edit(&ctx.http, edit).await;
    }

    let a_wins = hp[0] > hp[1] || (hp[0] == hp[1] && roll(seed, 2) == 0);
    let (winner, loser) = if a_wins { (a, b) } else { (b, a) };
    let finish = fill(pick(FINISH, seed), &winner.name, &loser.name);
    text.push_str(&format!("\n\n🏆 {}", finish));
    let side = if a_wins { 0 } else { 1 };
    let done =
        fight_card(stage.to_string(), a.card(hp[0]), b.card(hp[1]), finish, Outcome::Won(side), None).await;
    tokio::time::sleep(BEAT).await;
    let mut edit = EditMessage::new().content(&text);
    if let Some(png) = done {
        edit = edit.attachments(EditAttachments::new().add(CreateAttachment::bytes(png, "fight.png")));
    }
    let _ = message.edit(&ctx.http, edit).await;
    winner.clone()
}

/// The message under the card: the health line and the last few exchanges, so a
/// long fight never runs past Discord's message limit.
fn fight_text(head: &str, log: &[String], a: &Warrior, b: &Warrior, hp: &[i32; 2]) -> String {
    let recent = log.iter().rev().take(4).rev().cloned().collect::<Vec<_>>().join("\n");
    format!("{}\n❤️ **{}** {} — {} **{}**\n{}", head, a.name, hp[0].max(0), hp[1].max(0), b.name, recent)
}

// --- /fight -----------------------------------------------------------------

pub async fn fight_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("Ye server mein chalta hai.".into())).await;
        return;
    };
    let target = command.data.options.iter().find_map(|o| match o.value {
        CommandDataOptionValue::User(id) => Some(id),
        _ => None,
    });
    let Some(target) = target else {
        let _ = command.create_response(&ctx.http, whisper("Kisko challenge karna hai? `/fight @naam`".into())).await;
        return;
    };
    let me = command.user.id.get();
    let them = target.get();
    let refusal = if them == me {
        Some("Khud se ladega? Doctor se mil.".to_string())
    } else if ctx.cache.user(target).map(|u| u.bot).unwrap_or(false) {
        Some("Bot ko chhod, insaan dhoondh.".to_string())
    } else {
        LAST_FIGHT
            .lock()
            .get(&me)
            .and_then(|at| FIGHT_COOLDOWN.checked_sub(at.elapsed()))
            .filter(|left| !left.is_zero())
            .map(|left| format!("Thoda saans le. {}s baad phir challenge kar.", left.as_secs().max(1)))
    };
    if let Some(text) = refusal {
        let _ = command.create_response(&ctx.http, whisper(text)).await;
        return;
    }

    let here = command.channel_id;
    let arena = arena(ctx, guild, here).await;
    let _ = command.defer_ephemeral(&ctx.http).await;
    let (Some(a), Some(b)) = (warrior(ctx, guild, me).await, warrior(ctx, guild, them).await) else {
        let _ = command
            .edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content("Member nahi mila."))
            .await;
        return;
    };

    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let card = fight_card(
        "Challenge".into(),
        a.card(START_HP),
        b.card(START_HP),
        format!("{} vs {}", a.name, b.name),
        Outcome::Open,
        None,
    )
    .await;
    let content = format!(
        "⚔️ <@{}> ne <@{}> ko challenge kiya!\n<@{}>, accept karega ya bhaagega? {} mein challenge apne aap khatam.",
        me,
        them,
        them,
        "2 minute"
    );
    let mut msg = CreateMessage::new()
        .content(content)
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(them)]))
        .components(vec![CreateActionRow::Buttons(vec![
            CreateButton::new("fightyes:0").label("⚔️ Ladna hai").style(ButtonStyle::Success),
            CreateButton::new("fightno:0").label("🏃 Bhaag jaao").style(ButtonStyle::Secondary),
        ])]);
    if let Some(png) = card {
        msg = msg.add_file(CreateAttachment::bytes(png, "challenge.png"));
    }
    let Ok(posted) = arena.send_message(&ctx.http, msg).await else {
        let _ = command
            .edit_response(
                &ctx.http,
                serenity::all::EditInteractionResponse::new().content("Fight channel mein message nahi ja paya."),
            )
            .await;
        return;
    };

    // The buttons carry the message id, so the handler finds this challenge.
    let id = posted.id.get();
    let rows = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("fightyes:{}", id)).label("⚔️ Ladna hai").style(ButtonStyle::Success),
        CreateButton::new(format!("fightno:{}", id)).label("🏃 Bhaag jaao").style(ButtonStyle::Secondary),
    ])];
    let mut posted = posted;
    let _ = posted.edit(&ctx.http, EditMessage::new().components(rows)).await;
    CHALLENGES.lock().insert(id, Challenge { from: me, to: them, accepted: None });
    LAST_FIGHT.lock().insert(me, std::time::Instant::now());

    let link = posted.link();
    let note = if arena == here {
        format!("Challenge bhej diya: {}", link)
    } else {
        format!("⚔️ <@{}> ne <@{}> ko challenge kiya → {}", me, them, link)
    };
    let _ = command.edit_response(&ctx.http, serenity::all::EditInteractionResponse::new().content("Bhej diya.")).await;
    if arena != here {
        let _ =
            here.send_message(&ctx.http, CreateMessage::new().content(note).allowed_mentions(CreateAllowedMentions::new()))
                .await;
    }

    // Wait for the answer, then either fight or let the challenge lapse.
    let deadline = std::time::Instant::now() + CHALLENGE_WAIT;
    let answer = loop {
        if let Some(answer) = CHALLENGES.lock().get(&id).and_then(|c| c.accepted) {
            break Some(answer);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    CHALLENGES.lock().remove(&id);
    let _ = posted.edit(&ctx.http, EditMessage::new().components(vec![])).await;
    match answer {
        Some(true) => {
            if !BUSY.lock().insert(arena.get()) {
                let _ = arena.say(&ctx.http, "Ek fight already chal rahi hai, thodi der mein.").await;
                return;
            }
            let winner = play(ctx, arena, "Challenge", &a, &b, &mut seed).await;
            let wins = record_win(winner.id, "fight", 1);
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!("🏆 <@{}> jeet gaya! Total fight wins: **{}**", winner.id, wins))
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
            BUSY.lock().remove(&arena.get());
        }
        Some(false) => {
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!("🏃 <@{}> ne challenge se muh mod liya.", them))
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
        }
        None => {
            let _ = arena
                .send_message(
                    &ctx.http,
                    CreateMessage::new()
                        .content(format!("⌛ <@{}> ne challenge ignore kar diya. Fight cancel.", them))
                        .allowed_mentions(CreateAllowedMentions::new()),
                )
                .await;
        }
    }
}

// --- /battle ----------------------------------------------------------------

pub async fn battle_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("Ye server mein chalta hai.".into())).await;
        return;
    };
    if !super::admin_ids().contains(&command.user.id.get()) {
        let _ = command.create_response(&ctx.http, whisper("Battle sirf admin shuru kar sakta hai.".into())).await;
        return;
    }
    let minutes = command
        .data
        .options
        .iter()
        .find_map(|o| match o.value {
            CommandDataOptionValue::Integer(n) => Some(n),
            _ => None,
        })
        .unwrap_or(5)
        .clamp(MIN_WAIT, MAX_WAIT);

    let here = command.channel_id;
    let arena = arena(ctx, guild, here).await;
    if !BUSY.lock().insert(arena.get()) {
        let _ = command.create_response(&ctx.http, whisper("Ek battle already chal rahi hai.".into())).await;
        return;
    }
    let _ = command.create_response(&ctx.http, whisper(format!("Battle khol di, {} minute ka time.", minutes))).await;

    let ends = Utc::now().timestamp() + minutes * 60;
    let role = warrior_role(ctx, guild).await;
    let ping = role.map(|r| format!("<@&{}>", r)).unwrap_or_else(|| "Warriors".into());
    let embed = lobby_embed(&[], ends, minutes);
    let msg = CreateMessage::new()
        .content(format!("{} — battle royale khul gayi! Join karo 👇", ping))
        .allowed_mentions(CreateAllowedMentions::new().roles(role.into_iter().collect::<Vec<_>>()))
        .embed(embed)
        .components(lobby_buttons(0, true));
    let Ok(posted) = arena.send_message(&ctx.http, msg).await else {
        BUSY.lock().remove(&arena.get());
        return;
    };
    let lobby_id = posted.id.get();
    let rows = lobby_buttons(lobby_id, true);
    let mut posted = posted;
    let _ = posted.edit(&ctx.http, EditMessage::new().components(rows)).await;
    LOBBIES
        .lock()
        .insert(lobby_id, Lobby { joined: Vec::new(), names: HashMap::new(), open: true, ends, minutes });
    if arena != here {
        let _ = here
            .send_message(
                &ctx.http,
                CreateMessage::new().content(format!("⚔️ Battle royale yahan chal rahi hai → {}", posted.link())),
            )
            .await;
    }

    tokio::time::sleep(Duration::from_secs((minutes * 60) as u64)).await;
    let joined = {
        let mut lobbies = LOBBIES.lock();
        let lobby = lobbies.get_mut(&lobby_id);
        match lobby {
            Some(l) => {
                l.open = false;
                l.joined.clone()
            }
            None => Vec::new(),
        }
    };
    let _ = posted.edit(&ctx.http, EditMessage::new().components(lobby_buttons(lobby_id, false))).await;
    LOBBIES.lock().remove(&lobby_id);

    if joined.len() < MIN_PLAYERS {
        let _ = arena
            .say(&ctx.http, format!("Sirf {} log aaye. Battle cancel - agli baar {} chahiye.", joined.len(), MIN_PLAYERS))
            .await;
        BUSY.lock().remove(&arena.get());
        return;
    }
    run_battle(ctx, guild, arena, joined).await;
    BUSY.lock().remove(&arena.get());
}

fn lobby_buttons(id: u64, open: bool) -> Vec<CreateActionRow> {
    vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("battlejoin:{}", id))
            .label("⚔️ Join")
            .style(ButtonStyle::Success)
            .disabled(!open),
        CreateButton::new(format!("battlewarrior:{}", id))
            .label("🔔 Warrior role")
            .style(ButtonStyle::Secondary)
            .disabled(!open),
    ])]
}

fn lobby_embed(names: &[String], ends: i64, minutes: i64) -> CreateEmbed {
    let list = if names.is_empty() {
        "Abhi koi nahi. Pehla kaun?".to_string()
    } else {
        names.iter().map(|n| format!("• {}", n)).collect::<Vec<_>>().join("\n")
    };
    CreateEmbed::new()
        .title("⚔️ Battle Royale")
        .description(format!(
            "Join dabao aur ladne ke liye taiyar raho. Jeetne wale ko **{}** role milega.\n\n\
             ⏳ Band hoga <t:{}:R> ({} min)\n👥 **{}** joined (kam se kam {} chahiye)\n\n{}",
            CHAMPION_ROLE,
            ends,
            minutes,
            names.len(),
            MIN_PLAYERS,
            list
        ))
        .colour(0xE67E22)
        .footer(CreateEmbedFooter::new("Har fight ka result sikka uchhal ke - bas maza lo"))
}

/// Knockout rounds until one is left.
async fn run_battle(ctx: &Context, guild: GuildId, arena: ChannelId, joined: Vec<u64>) {
    let mut seed = Utc::now().timestamp_millis() as u64 | 1;
    let mut fighters: Vec<Warrior> = Vec::new();
    for id in joined.into_iter().take(MAX_PLAYERS) {
        if let Some(w) = warrior(ctx, guild, id).await {
            fighters.push(w);
        }
    }
    if fighters.len() < MIN_PLAYERS {
        let _ = arena.say(&ctx.http, "Itne log nahi mile. Battle cancel.").await;
        return;
    }
    let started = fighters.len();
    let mut round = 1;
    while fighters.len() > 1 {
        shuffle(&mut fighters, &mut seed);
        let stage = stage_name(fighters.len(), round);
        let _ = arena
            .say(&ctx.http, format!("**{}** — {} warriors bache hain.", stage, fighters.len()))
            .await;
        tokio::time::sleep(FIGHT_GAP).await;
        let mut next = Vec::new();
        let mut pairs = fighters.chunks(2);
        while let Some(pair) = pairs.next() {
            match pair {
                [a, b] => {
                    let winner = play(ctx, arena, &stage, a, b, &mut seed).await;
                    next.push(winner);
                    tokio::time::sleep(FIGHT_GAP).await;
                }
                [alone] => {
                    let line = fill(pick(BYE, &mut seed), &alone.name, "");
                    let _ = arena.say(&ctx.http, format!("☕ {}", line)).await;
                    next.push(alone.clone());
                }
                _ => {}
            }
        }
        fighters = next;
        round += 1;
        if fighters.len() > 1 {
            tokio::time::sleep(ROUND_GAP).await;
        }
    }

    let Some(champion) = fighters.into_iter().next() else {
        return;
    };
    let wins = record_win(champion.id, "battle", started.saturating_sub(1));
    crown(ctx, guild, champion.id).await;
    let subtitle = format!("{} warriors · {} rounds · 1 champion", started, round - 1);
    let line = pick(CHAMPION_LINE, &mut seed).to_string();
    let card = champion_card(champion.card(START_HP), subtitle, line).await;
    let mut msg = CreateMessage::new()
        .content(format!(
            "👑 <@{}> is the **{}**! Battle wins: **{}**",
            champion.id, CHAMPION_ROLE, wins
        ))
        .allowed_mentions(CreateAllowedMentions::new().users(vec![UserId::new(champion.id)]));
    if let Some(png) = card {
        msg = msg.add_file(CreateAttachment::bytes(png, "champion.png"));
    }
    let _ = arena.send_message(&ctx.http, msg).await;
}

fn stage_name(left: usize, round: u32) -> String {
    match left {
        2 => "Final".to_string(),
        3..=4 => "Semi-final".to_string(),
        5..=8 => "Quarter-final".to_string(),
        _ => format!("Round {}", round),
    }
}

fn shuffle(list: &mut [Warrior], seed: &mut u64) {
    for i in (1..list.len()).rev() {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        list.swap(i, (*seed % (i as u64 + 1)) as usize);
    }
}

// --- /warrior ---------------------------------------------------------------

pub async fn warrior_command(ctx: &Context, command: &CommandInteraction) {
    let whisper = |text: String| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Some(guild) = command.guild_id else {
        let _ = command.create_response(&ctx.http, whisper("Ye server mein chalta hai.".into())).await;
        return;
    };
    let text = match toggle_warrior(ctx, guild, command.user.id.get()).await {
        Some(true) => format!("🔔 {} role mil gaya. Ab har battle pe ping aayega.", WARRIOR_ROLE),
        Some(false) => format!("🔕 {} role hata diya. Ab ping nahi aayega.", WARRIOR_ROLE),
        None => "Role set nahi kar paya - bot ke paas Manage Roles nahi hai shayad.".to_string(),
    };
    let _ = command.create_response(&ctx.http, whisper(text)).await;
}

/// Adds the warrior role, or takes it away if they already have it.
async fn toggle_warrior(ctx: &Context, guild: GuildId, user: u64) -> Option<bool> {
    let role = warrior_role(ctx, guild).await?;
    let member = guild.member(&ctx.http, UserId::new(user)).await.ok()?;
    if member.roles.contains(&role) {
        member.remove_role(&ctx.http, role).await.ok()?;
        Some(false)
    } else {
        member.add_role(&ctx.http, role).await.ok()?;
        Some(true)
    }
}

// --- buttons ----------------------------------------------------------------

pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    let id = component.data.custom_id.clone();
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    if let Some(rest) = id.strip_prefix("battlejoin:") {
        join(ctx, component, rest).await;
    } else if id.starts_with("battlewarrior:") {
        let text = match component.guild_id {
            Some(guild) => match toggle_warrior(ctx, guild, component.user.id.get()).await {
                Some(true) => format!("🔔 {} role mil gaya.", WARRIOR_ROLE),
                Some(false) => format!("🔕 {} role hata diya.", WARRIOR_ROLE),
                None => "Role set nahi ho paya.".to_string(),
            },
            None => "Ye server mein chalta hai.".to_string(),
        };
        let _ = component.create_response(&ctx.http, whisper(&text)).await;
    } else if let Some(rest) = id.strip_prefix("fightyes:") {
        answer(ctx, component, rest, true).await;
    } else if let Some(rest) = id.strip_prefix("fightno:") {
        answer(ctx, component, rest, false).await;
    }
}

async fn join(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Ok(lobby_id) = rest.parse::<u64>() else {
        return;
    };
    let user = component.user.id.get();
    let name = component
        .member
        .as_ref()
        .map(|m| display(m))
        .unwrap_or_else(|| component.user.name.clone());
    let (result, names, ends, minutes) = {
        let mut lobbies = LOBBIES.lock();
        let roster = |lobby: &Lobby| -> Vec<String> {
            lobby.joined.iter().filter_map(|u| lobby.names.get(u).cloned()).collect()
        };
        match lobbies.get_mut(&lobby_id) {
            None => ("closed", Vec::new(), 0, 0),
            Some(lobby) if !lobby.open => ("closed", Vec::new(), lobby.ends, lobby.minutes),
            Some(lobby) if lobby.joined.contains(&user) => ("already", roster(lobby), lobby.ends, lobby.minutes),
            Some(lobby) if lobby.joined.len() >= MAX_PLAYERS => ("full", Vec::new(), lobby.ends, lobby.minutes),
            Some(lobby) => {
                lobby.joined.push(user);
                lobby.names.insert(user, name);
                ("joined", roster(lobby), lobby.ends, lobby.minutes)
            }
        }
    };
    match result {
        "joined" => {
            let _ = component.create_response(&ctx.http, whisper("⚔️ Tu andar hai. Taiyar reh.")).await;
            // Everyone should see the roster fill up, countdown untouched.
            let mut message = component.message.clone();
            let _ = message.edit(&ctx.http, EditMessage::new().embed(lobby_embed(&names, ends, minutes))).await;
        }
        "already" => {
            let _ = component.create_response(&ctx.http, whisper("Tu already andar hai.")).await;
        }
        "full" => {
            let _ = component.create_response(&ctx.http, whisper("Lobby full hai.")).await;
        }
        _ => {
            let _ = component.create_response(&ctx.http, whisper("Ye battle band ho chuki hai.")).await;
        }
    }
}

async fn answer(ctx: &Context, component: &ComponentInteraction, rest: &str, yes: bool) {
    let whisper = |text: &str| {
        CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
    };
    let Ok(id) = rest.parse::<u64>() else {
        return;
    };
    let user = component.user.id.get();
    let allowed = {
        let mut challenges = CHALLENGES.lock();
        match challenges.get_mut(&id) {
            Some(challenge) if challenge.to == user => {
                challenge.accepted = Some(yes);
                true
            }
            _ => false,
        }
    };
    if allowed {
        let text = if yes { "⚔️ Chalo, ho jaaye." } else { "🏃 Theek hai, bhaag ja." };
        let _ = component.create_response(&ctx.http, whisper(text)).await;
    } else {
        let _ = component.create_response(&ctx.http, whisper("Ye challenge tere liye nahi hai.")).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fill_both_names_and_stages_read_right() {
        let line = fill("{a} ne {b} ko chappal dikhayi", "Ravi", "Sneha");
        assert_eq!(line, "Ravi ne Sneha ko chappal dikhayi");
        assert_eq!(stage_name(2, 4), "Final");
        assert_eq!(stage_name(4, 3), "Semi-final");
        assert_eq!(stage_name(8, 2), "Quarter-final");
        assert_eq!(stage_name(16, 1), "Round 1");
    }

    #[test]
    fn every_line_has_placeholders_and_picks_spread() {
        for line in EXCHANGE {
            assert!(line.contains("{a}") && line.contains("{b}"), "{}", line);
        }
        for line in FINISH {
            assert!(line.contains("{w}") && line.contains("{l}"), "{}", line);
        }
        let mut seed = 12345u64;
        let picks: std::collections::HashSet<&str> = (0..200).map(|_| pick(EXCHANGE, &mut seed)).collect();
        assert!(picks.len() > EXCHANGE.len() / 2, "picks bunched up: {}", picks.len());
    }

    /// Play the exchange loop the way `play` does, without Discord in the way.
    fn simulate(seed: &mut u64) -> ([i32; 2], usize) {
        let mut hp = [START_HP; 2];
        let mut turns = 0;
        while hp[0] > 0 && hp[1] > 0 && turns < MAX_EXCHANGES {
            turns += 1;
            let attacker = roll(seed, 2) as usize;
            let (_, delta, target) = exchange(seed, attacker);
            hp[target] = (hp[target] + delta).clamp(0, START_HP);
        }
        (hp, turns)
    }

    #[test]
    fn health_runs_out_in_a_reasonable_number_of_exchanges() {
        let mut seed = 7u64;
        let (mut knockouts, mut total) = (0, 0);
        for _ in 0..500 {
            let (hp, turns) = simulate(&mut seed);
            assert!(hp.iter().all(|h| (0..=START_HP).contains(h)), "health left the bar: {:?}", hp);
            assert!(turns <= MAX_EXCHANGES);
            assert!(hp[0] > 0 || hp[1] > 0, "both fighters can't be out");
            if hp.contains(&0) {
                knockouts += 1;
            }
            total += turns;
        }
        // Most fights should end with a knockout rather than on health left.
        assert!(knockouts > 450, "only {} knockouts in 500 fights", knockouts);
        let average = total as f64 / 500.0;
        assert!((5.0..11.0).contains(&average), "fights average {} exchanges", average);
    }

    #[test]
    fn exchange_bands_stay_within_their_numbers() {
        let mut seed = 42u64;
        let (mut miss, mut crit, mut heal, mut hit) = (0, 0, 0, 0);
        for _ in 0..4000 {
            let attacker = roll(&mut seed, 2) as usize;
            let (blow, delta, target) = exchange(&mut seed, attacker);
            match blow {
                Blow::Miss => {
                    assert_eq!(delta, 0);
                    miss += 1;
                }
                Blow::Heal => {
                    assert!((6..=12).contains(&delta), "heal {}", delta);
                    assert_eq!(target, attacker, "a heal lands on the one who took it");
                    heal += 1;
                }
                Blow::Crit => {
                    assert!((-43..=-32).contains(&delta), "crit {}", delta);
                    crit += 1;
                }
                Blow::Hit => {
                    assert!((-30..=-18).contains(&delta), "hit {}", delta);
                    hit += 1;
                }
            }
            if delta <= 0 {
                assert_eq!(target, 1 - attacker, "damage lands on the other side");
            }
        }
        assert!(miss > 200 && crit > 400 && heal > 200 && hit > 2000, "{} {} {} {}", miss, crit, heal, hit);
    }

    #[test]
    fn fight_text_keeps_only_the_last_few_lines() {
        let warrior = |id: u64, name: &str| Warrior { id, name: name.to_string(), avatar: None };
        let (a, b) = (warrior(1, "Ravi"), warrior(2, "Sneha"));
        let log: Vec<String> = (1..=6).map(|i| format!("line {}", i)).collect();
        let text = fight_text("head", &log, &a, &b, &[62, 0]);
        assert!(text.contains("❤️ **Ravi** 62 — 0 **Sneha**"), "{}", text);
        assert!(text.contains("line 6") && text.contains("line 3"), "{}", text);
        assert!(!text.contains("line 2"), "{}", text);
    }

    #[test]
    fn shuffle_keeps_everyone_and_pairs_leave_one_out_when_odd() {
        let warriors = |n: usize| {
            (0..n)
                .map(|i| Warrior { id: i as u64, name: format!("w{}", i), avatar: None })
                .collect::<Vec<_>>()
        };
        let mut list = warriors(9);
        let mut seed = 99u64;
        shuffle(&mut list, &mut seed);
        let ids: std::collections::HashSet<u64> = list.iter().map(|w| w.id).collect();
        assert_eq!(ids.len(), 9);
        let chunks: Vec<usize> = list.chunks(2).map(|c| c.len()).collect();
        assert_eq!(chunks.iter().filter(|n| **n == 1).count(), 1, "odd rounds need exactly one bye");
    }
}
