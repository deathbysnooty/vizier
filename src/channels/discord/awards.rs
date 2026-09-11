//! /awards - who stands out, and how.
//!
//! Not a volume leaderboard: each award is a shape of behaviour - when
//! someone talks, where, how long they sit in voice. One award per person,
//! handed out in catalogue order, with ties always resolved the same way.
//!
//! Only current members win: awards are for people still here to be teased
//! about them. The weekly view is the last complete Monday-to-Sunday week in
//! India, so its winners hold still for seven days.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, NaiveDate};
use cosmic_text::FontSystem;
use parking_lot::Mutex;
use regex::Regex;
use rusqlite::Connection;
use serenity::all::{
    ChannelType, Context, CreateActionRow, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, GuildId,
    UserId,
};

use super::awards_card::{self, Award as CardAward, Card, Section};
use super::stats;

/// A "sitting" longer than this means a leave was never logged.
const MAX_SITTING: f64 = 12.0 * 3600.0;
const CACHE_FOR: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum View {
    Overall,
    Week,
}

impl View {
    pub fn from_value(value: &str) -> View {
        if value == "week" {
            View::Week
        } else {
            View::Overall
        }
    }

    fn value(self) -> &'static str {
        match self {
            View::Overall => "overall",
            View::Week => "week",
        }
    }
}

/// The dropdown under the card. Changing it redraws the one message for
/// everyone looking at it.
pub fn view_menu(current: View) -> CreateActionRow {
    let option = |label: &str, view: View| CreateSelectMenuOption::new(label, view.value()).default_selection(view == current);
    CreateActionRow::SelectMenu(
        CreateSelectMenu::new(
            "awards_view",
            CreateSelectMenuKind::String {
                options: vec![option("Overall - all time", View::Overall), option("Last week", View::Week)],
            },
        )
        .placeholder("Which awards?"),
    )
}

struct Def {
    key: &'static str,
    section: &'static str,
    label: &'static str,
    emoji: &'static str,
}

const CATALOGUE: &[Def] = &[
    Def { key: "night", section: "time", label: "NIGHT OWL", emoji: "🦉" },
    Def { key: "morning", section: "time", label: "SUBAH KA MURGA", emoji: "🐓" },
    Def { key: "office", section: "time", label: "OFFICE MEIN FREE", emoji: "💼" },
    Def { key: "weekend", section: "time", label: "WEEKEND WARRIOR", emoji: "🏖️" },
    Def { key: "vcshare", section: "vc", label: "VC KA KIDA", emoji: "🎙️" },
    Def { key: "nvc", section: "vc", label: "HAR MEHFIL MEIN", emoji: "🦘" },
    Def { key: "voicehrs", section: "vc", label: "SABSE ZYADA VC", emoji: "🔊" },
    Def { key: "voiceratio", section: "vc", label: "BOLTA HAI, LIKHTA NAHI", emoji: "🗣️" },
    Def { key: "avgsess", section: "vc", label: "LAMBI BAITHAK", emoji: "🛋️" },
    Def { key: "afkhrs", section: "vc", label: "KUMBHKARAN", emoji: "😴" },
    Def { key: "spread", section: "chat", label: "GHUMAKKAD", emoji: "🧭" },
    Def { key: "perday", section: "chat", label: "EK DIN MEIN PURA SAAL", emoji: "💣" },
    Def { key: "bestday", section: "chat", label: "EK HI DIN MEIN", emoji: "🔥" },
    Def { key: "days", section: "chat", label: "ROZ AATA HAI", emoji: "📅" },
    Def { key: "quiet", section: "chat", label: "GAYAB", emoji: "👻" },
    Def { key: "msgs", section: "chat", label: "BAKCHODI SAMRAT", emoji: "👑" },
];

const SECTIONS: &[(&str, &str)] = &[("time", "RAAT DIN KA HISAAB"), ("vc", "VC WALE"), ("chat", "CHAT KE SHER")];

#[derive(Clone)]
struct MemberInfo {
    name: String,
    face: String,
}

static AFK_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:\[\s*afk\s*\]|\(\s*afk\s*\)|afk\b)\s*|\s*(?:\[\s*afk\s*\]|\(\s*afk\s*\)|\bafk)\s*$")
        .expect("regex")
});

/// What people see in the server, minus a stale AFK tag.
pub(super) fn clean_name(name: &str) -> String {
    let cleaned = AFK_TAG.replace_all(name, "").trim().to_string();
    if cleaned.is_empty() {
        name.to_string()
    } else {
        cleaned
    }
}

async fn members(ctx: &Context, guild: GuildId) -> HashMap<u64, MemberInfo> {
    let mut out = HashMap::new();
    let mut after: Option<UserId> = None;
    loop {
        let page = match guild.members(&ctx.http, Some(1000), after).await {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!("awards: member list failed: {}", err);
                break;
            }
        };
        for m in &page {
            if !m.user.bot {
                out.insert(
                    m.user.id.get(),
                    MemberInfo { name: clean_name(m.display_name()), face: m.face().replace("size=1024", "size=256") },
                );
            }
        }
        if page.len() < 1000 {
            break;
        }
        after = page.last().map(|m| m.user.id);
    }
    out
}

#[derive(Default)]
struct Person {
    msgs: u64,
    vc_msgs: u64,
    hours: [u64; 24],
    dow: [u64; 7],
    days: HashMap<String, u64>,
    channels: HashSet<u64>,
    vc_channels: HashSet<u64>,
    voice_secs: f64,
    sittings: u64,
    afk_secs: f64,
    alltime_msgs: u64,
    last_active: Option<NaiveDate>,
}

struct Window {
    from_day: Option<String>,
    to_day: Option<String>,
    from_ts: Option<i64>,
    to_ts: Option<i64>,
}

fn load(
    conn: &Connection,
    win: &Window,
    exclude: &HashSet<u64>,
    vc_rooms: &HashSet<u64>,
    afk: Option<u64>,
) -> rusqlite::Result<HashMap<u64, Person>> {
    let mut people: HashMap<u64, Person> = HashMap::new();
    {
        let mut dates: HashMap<String, NaiveDate> = HashMap::new();
        let mut stmt = conn.prepare("SELECT user_id, channel_id, day, hour, count FROM msg_counts")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let channel = r.get::<_, i64>(1)? as u64;
            if exclude.contains(&channel) {
                continue;
            }
            let user = r.get::<_, i64>(0)? as u64;
            let day: String = r.get(2)?;
            let hour = r.get::<_, i64>(3)?.clamp(0, 23) as usize;
            let n = r.get::<_, i64>(4)? as u64;
            let date = match dates.get(&day) {
                Some(d) => *d,
                None => {
                    let Ok(d) = NaiveDate::parse_from_str(&day, "%Y-%m-%d") else { continue };
                    dates.insert(day.clone(), d);
                    d
                }
            };
            let p = people.entry(user).or_default();
            p.alltime_msgs += n;
            if p.last_active.is_none_or(|d| d < date) {
                p.last_active = Some(date);
            }
            if win.from_day.as_deref().is_some_and(|f| day.as_str() < f)
                || win.to_day.as_deref().is_some_and(|t| day.as_str() > t)
            {
                continue;
            }
            p.msgs += n;
            p.hours[hour] += n;
            p.dow[date.weekday().num_days_from_monday() as usize] += n;
            p.channels.insert(channel);
            if vc_rooms.contains(&channel) {
                p.vc_msgs += n;
                p.vc_channels.insert(channel);
            }
            *p.days.entry(day).or_insert(0) += n;
        }
    }
    {
        // Discord allows one voice room at a time, so any event closes what
        // was open. A sitting runs from arriving to leaving across switches;
        // AFK time is kept apart and excluded rooms count for nothing.
        let mut stmt =
            conn.prepare("SELECT user_id, action, channel_id, ts FROM voice_events ORDER BY user_id, ts, msg_id")?;
        let mut rows = stmt.query([])?;
        let mut current: Option<u64> = None;
        let mut open: Option<(i64, u64)> = None;
        let mut sit_real = 0.0;
        while let Some(r) = rows.next()? {
            let user = r.get::<_, i64>(0)? as u64;
            let action: String = r.get(1)?;
            let room = r.get::<_, i64>(2)? as u64;
            let ts: i64 = r.get(3)?;
            if current != Some(user) {
                current = Some(user);
                open = None;
                sit_real = 0.0;
            }
            if action == "left" && open.is_none() {
                continue;
            }
            let p = people.entry(user).or_default();
            if let Some((start, in_room)) = open {
                let d = (ts - start) as f64;
                let inside = win.from_ts.is_none_or(|f| start >= f) && win.to_ts.is_none_or(|t| start < t);
                if d <= MAX_SITTING && inside {
                    if Some(in_room) == afk {
                        p.afk_secs += d;
                    } else if !exclude.contains(&in_room) {
                        p.voice_secs += d;
                        sit_real += d;
                    }
                }
            }
            if action != "switched" && open.is_some() {
                if sit_real > 0.0 {
                    p.sittings += 1;
                }
                sit_real = 0.0;
            }
            if action != "left" {
                // Sitting in voice is being around, even without typing.
                if let Some(day) = DateTime::from_timestamp(ts, 0).map(|t| t.with_timezone(&stats::ist()).date_naive()) {
                    if p.last_active.is_none_or(|d| d < day) {
                        p.last_active = Some(day);
                    }
                }
            }
            open = if action == "left" { None } else { Some((ts, room)) };
        }
    }
    Ok(people)
}

fn eligible(key: &str, p: &Person, view: View) -> bool {
    let week = view == View::Week;
    match key {
        "afkhrs" => true,
        // Voice-only people must be able to win the voice awards, but someone
        // with nine messages must not win "most of their messages in VC".
        "voicehrs" | "voiceratio" | "avgsess" => p.voice_secs / 3600.0 >= if week { 5.0 } else { 50.0 },
        _ => p.msgs >= if week { 50 } else { 1000 },
    }
}

fn metric(key: &str, p: &Person, view: View, today: NaiveDate) -> f64 {
    let m = p.msgs.max(1) as f64;
    let span = |a: usize, b: usize| p.hours[a..b].iter().sum::<u64>() as f64;
    match key {
        "night" => span(0, 5) / m,
        "morning" => span(5, 9) / m,
        "office" => span(10, 18) / m,
        "weekend" => (p.dow[5] + p.dow[6]) as f64 / m,
        "vcshare" => p.vc_msgs as f64 / m,
        "nvc" => p.vc_channels.len() as f64,
        "voicehrs" => p.voice_secs / 3600.0,
        "voiceratio" => p.voice_secs / 3600.0 / m * 100.0,
        "avgsess" => {
            let enough = if view == View::Week { 5 } else { 30 };
            if p.sittings >= enough {
                p.voice_secs / p.sittings as f64 / 60.0
            } else {
                0.0
            }
        }
        "afkhrs" => p.afk_secs / 3600.0,
        "spread" => p.channels.len() as f64,
        "perday" => p.msgs as f64 / p.days.len().max(1) as f64,
        "bestday" => p.days.values().copied().max().unwrap_or(0) as f64,
        "days" => p.days.len() as f64,
        "quiet" => p.last_active.map(|d| (today - d).num_days() as f64).unwrap_or(0.0),
        _ => p.msgs as f64,
    }
}

/// The bar a winner must clear. Without one, an award nobody really has
/// goes to whoever sits on top of a column of zeroes.
fn floor(key: &str, view: View) -> f64 {
    let week = view == View::Week;
    match key {
        "night" => 0.25,
        "morning" => 0.15,
        "office" => 0.5,
        "weekend" => 0.45,
        "vcshare" => 0.6,
        "nvc" => if week { 4.0 } else { 8.0 },
        "voicehrs" => if week { 5.0 } else { 50.0 },
        "voiceratio" => 20.0,
        "avgsess" => if week { 60.0 } else { 90.0 },
        "afkhrs" => if week { 1.0 } else { 5.0 },
        "spread" => if week { 6.0 } else { 12.0 },
        "quiet" => 14.0,
        _ => f64::MIN_POSITIVE,
    }
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn short(n: u64) -> String {
    if n >= 10_000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        thousands(n)
    }
}

/// Headline number and caption for a winner.
fn present(key: &str, p: &Person, v: f64) -> (String, String) {
    let pct = || format!("{:.0}%", v * 100.0);
    let hours = |secs: f64| format!("{}h", thousands((secs / 3600.0).round() as u64));
    match key {
        "night" => (pct(), "of messages sent 12am–5am".into()),
        "morning" => (pct(), "of messages sent 5am–9am".into()),
        "office" => (pct(), "of messages in working hours".into()),
        "weekend" => (pct(), "of messages on Sat and Sun".into()),
        "vcshare" => (pct(), "of messages typed in VC chats".into()),
        "nvc" => ((v as u64).to_string(), "different VC rooms visited".into()),
        "voicehrs" => (hours(p.voice_secs), "spent in voice".into()),
        "voiceratio" => (hours(p.voice_secs), format!("in voice · only {} messages typed", thousands(p.msgs))),
        "avgsess" => (format!("{:.0}m", v), "average voice sitting".into()),
        "afkhrs" => (hours(p.afk_secs), "parked in Lost In The Void".into()),
        "spread" => ((v as u64).to_string(), "different channels posted in".into()),
        "perday" => (format!("{:.0}", v), "messages per active day".into()),
        "bestday" => (short(v as u64), "messages in a single day".into()),
        "days" => ((v as u64).to_string(), "different days active".into()),
        "quiet" => (format!("{}d", v as u64), format!("silent after {} messages", thousands(p.alltime_msgs))),
        _ => (short(p.msgs), "messages sent".into()),
    }
}

struct Pick {
    def: &'static Def,
    winner: Option<(u64, String, String)>,
}

fn pick_winners(people: &HashMap<u64, Person>, members: &HashMap<u64, MemberInfo>, view: View, today: NaiveDate) -> Vec<Pick> {
    let mut taken: HashSet<u64> = HashSet::new();
    let mut out = Vec::new();
    for def in CATALOGUE {
        // Nobody can be gone for two weeks inside one, and everyone who
        // turned up daily would tie at seven.
        if view == View::Week && matches!(def.key, "days" | "quiet") {
            continue;
        }
        let bar = floor(def.key, view);
        let best = people
            .iter()
            .filter(|&(id, p)| members.contains_key(id) && !taken.contains(id) && eligible(def.key, p, view))
            .map(|(id, p)| (*id, p, metric(def.key, p, view, today)))
            .filter(|(_, _, v)| *v >= bar)
            // Ties go to the more active person, then the lower id, so the
            // same data always crowns the same winner.
            .max_by(|a, b| {
                a.2.partial_cmp(&b.2)
                    .unwrap_or(Ordering::Equal)
                    .then(a.1.alltime_msgs.cmp(&b.1.alltime_msgs))
                    .then(a.1.voice_secs.partial_cmp(&b.1.voice_secs).unwrap_or(Ordering::Equal))
                    .then(b.0.cmp(&a.0))
            });
        let winner = best.map(|(id, p, v)| {
            let (stat, sub) = present(def.key, p, v);
            (id, stat, sub)
        });
        if let Some((id, _, _)) = &winner {
            taken.insert(*id);
        }
        out.push(Pick { def, winner });
    }
    out
}

pub(super) fn fonts() -> &'static Mutex<FontSystem> {
    static FONTS: OnceLock<Mutex<FontSystem>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let mut fs = FontSystem::new();
        // On Linux the font list comes from fontconfig's fonts.conf. Without
        // fontconfig installed that file is missing and the list is empty,
        // so look in the usual places directly.
        if fs.db().len() == 0 {
            for dir in ["/usr/share/fonts", "/usr/local/share/fonts"] {
                fs.db_mut().load_fonts_dir(dir);
            }
        }
        tracing::info!("awards: {} font faces available", fs.db().len());
        Mutex::new(fs)
    })
}

type Cached = (Instant, Arc<Vec<u8>>, String);

fn cache() -> &'static Mutex<HashMap<View, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<View, Cached>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn env_ids(key: &str) -> HashSet<u64> {
    std::env::var(key).unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect()
}

fn day_label(d: NaiveDate) -> String {
    d.format("%-d %b").to_string().to_uppercase()
}

/// The awards card as PNG, plus a note to post with it (import progress).
pub async fn card_png(ctx: &Context, guild: GuildId, view: View) -> Result<(Arc<Vec<u8>>, String), String> {
    if let Some((at, png, note)) = cache().lock().get(&view).cloned() {
        if at.elapsed() < CACHE_FOR {
            return Ok((png, note));
        }
    }
    let db = stats::db().ok_or("stats are not being recorded")?;
    let vc_rooms: HashSet<u64> = ctx
        .cache
        .guild(guild)
        .map(|g| g.channels.values().filter(|c| c.kind == ChannelType::Voice).map(|c| c.id.get()).collect())
        .unwrap_or_default();
    let exclude = env_ids("VIZIER_STATS_EXCLUDE_CHANNELS");
    let afk: Option<u64> = std::env::var("VIZIER_VOICE_AFK_CHANNEL").ok().and_then(|v| v.trim().parse().ok());
    let members = members(ctx, guild).await;
    if members.is_empty() {
        return Err("could not read the member list".into());
    }

    let today = chrono::Utc::now().with_timezone(&stats::ist()).date_naive();
    let this_monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64);
    let midnight = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .and_then(|t| t.and_local_timezone(stats::ist()).single())
            .map(|t| t.timestamp())
    };
    let (win, week) = match view {
        View::Overall => (Window { from_day: None, to_day: None, from_ts: None, to_ts: None }, None),
        View::Week => {
            let from = this_monday - chrono::Duration::days(7);
            let to = this_monday - chrono::Duration::days(1);
            let win = Window {
                from_day: Some(from.to_string()),
                to_day: Some(to.to_string()),
                from_ts: midnight(from),
                to_ts: midnight(this_monday),
            };
            (win, Some((from, to)))
        }
    };

    let roster = members.clone();
    let (picks, total_msgs, voice_hours, first_day) = tokio::task::spawn_blocking(move || -> rusqlite::Result<_> {
        let conn = db.lock();
        let people = load(&conn, &win, &exclude, &vc_rooms, afk)?;
        let first_day: Option<String> =
            conn.query_row("SELECT min(day) FROM msg_counts", [], |r| r.get::<_, Option<String>>(0)).ok().flatten();
        let total: u64 = people.values().map(|p| p.msgs).sum();
        let voice =
            people.iter().filter(|(id, _)| roster.contains_key(*id)).map(|(_, p)| p.voice_secs).sum::<f64>() / 3600.0;
        Ok((pick_winners(&people, &roster, view, today), total, voice, first_day))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let fetches = picks.iter().map(|pick| {
        let url = pick.winner.as_ref().and_then(|(id, _, _)| members.get(id)).map(|m| m.face.clone());
        let client = client.clone();
        async move {
            let resp = client.get(url?).send().await.ok()?;
            resp.bytes().await.ok().map(|b| b.to_vec())
        }
    });
    let avatars: Vec<Option<Vec<u8>>> = futures::future::join_all(fetches).await;

    let mut sections: Vec<Section> = SECTIONS
        .iter()
        .map(|(key, name)| Section { key: key.to_string(), name: name.to_string(), awards: Vec::new() })
        .collect();
    for (pick, avatar) in picks.into_iter().zip(avatars) {
        let Some(section) = sections.iter_mut().find(|s| s.key == pick.def.section) else { continue };
        let (winner, stat, sub, avatar) = match pick.winner {
            Some((id, stat, sub)) => (members.get(&id).map(|m| m.name.clone()), stat, sub, avatar),
            None => (None, "-".to_string(), "nobody cleared the bar".to_string(), None),
        };
        section.awards.push(CardAward {
            label: pick.def.label.to_string(),
            emoji: pick.def.emoji.to_string(),
            winner,
            stat,
            sub,
            avatar,
            departed: false,
        });
    }

    let period = match week {
        Some((from, to)) => format!("LAST WEEK  ·  {} – {}", day_label(from), to.format("%-d %b %Y").to_string().to_uppercase()),
        None => format!(
            "ALL TIME  ·  {} – {}",
            first_day.as_deref().and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).map(day_label).unwrap_or_default(),
            today.format("%-d %b %Y").to_string().to_uppercase()
        ),
    };
    let messages = if total_msgs >= 1000 { format!("{}K messages", total_msgs / 1000) } else { format!("{} messages", total_msgs) };
    let card = Card {
        title: "MLCI AWARDS".to_string(),
        period,
        chips: vec![
            messages,
            format!("{} voice hours", thousands(voice_hours.round() as u64)),
            format!("{} members", members.len()),
        ],
        sections,
        footer: match view {
            View::Week => "one award per person  ·  resets every Monday  ·  bots, mod and game rooms not counted".to_string(),
            View::Overall => "one award per person  ·  bots, #moderator-only and game rooms not counted".to_string(),
        },
    };
    let png = tokio::task::spawn_blocking(move || {
        let mut fs = fonts().lock();
        // cosmic-text panics rather than drawing with no fonts at all.
        if fs.db().len() == 0 {
            return Err("no fonts installed to draw with".to_string());
        }
        awards_card::render(&card, &mut fs).ok_or_else(|| "drawing the card failed".to_string())
    })
    .await
    .map_err(|e| e.to_string())??;

    let (done, total) = stats::history_progress();
    let mut note = String::new();
    if total > 0 && done < total {
        note.push_str(&format!("Purana hisaab abhi gin raha hoon ({}/{} channels) - numbers badlenge.\n", done, total));
    }
    if !stats::voice_caught_up() {
        note.push_str("VC ka purana record abhi padh raha hoon.\n");
    }
    let png = Arc::new(png);
    // Only a finished picture is worth holding on to.
    if note.is_empty() {
        cache().lock().insert(view, (Instant::now(), png.clone(), note.clone()));
    }
    Ok((png, note))
}
