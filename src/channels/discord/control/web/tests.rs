//! The panel's server against a fake Discord: sign-in, the guards on every
//! call, validation and the reminders round trip. `demo_server` (ignored) serves
//! the same fake with a signed-in session so the UI can be looked at.

use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::super::catalog::{Command, Kind, Section, Setting};
use super::*;

// --- the fake server --------------------------------------------------------------

pub const ADMIN: u64 = 1001;
pub const ADMIN_TWO: u64 = 1002;
pub const MEMBER: u64 = 1003;

pub struct FakeData;

fn avatar(name: &str, hue: u32) -> String {
    let letter = name.chars().next().unwrap_or('?');
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='64' height='64'><rect width='64' height='64' fill='hsl({},55%,45%)'/>\
         <text x='32' y='42' font-family='Arial' font-size='28' font-weight='700' fill='white' text-anchor='middle'>{}</text></svg>",
        hue, letter
    );
    format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg))
}

use base64::Engine as _;

const PEOPLE: &[(u64, &str, &str, u32)] = &[
    (ADMIN, "Kabir", "kabir.exe", 210),
    (ADMIN_TWO, "Meera", "meera_07", 330),
    (MEMBER, "Rohan", "rohanplays", 25),
    (1004, "Zoya", "zoya.z", 280),
    (1005, "Arjun", "arjun_k", 150),
    (1006, "Ishita", "ishita", 0),
    (1007, "Dev", "devdoesdev", 190),
    (1008, "Tanvi", "tanvi.t", 45),
    (1009, "Sameer", "sam_eer", 95),
];

fn person(p: &(u64, &str, &str, u32)) -> MemberInfo {
    MemberInfo { id: p.0.to_string(), name: p.1.into(), username: p.2.into(), avatar: avatar(p.1, p.3), bot: false }
}

fn ch(id: u64, name: &str, kind: ChannelKind, category: Option<(&str, i64)>, position: i64) -> ChannelInfo {
    ChannelInfo {
        id: id.to_string(),
        name: name.into(),
        kind,
        category: category.map(|c| c.0.to_string()),
        category_position: category.map(|c| c.1).unwrap_or(-1),
        position,
    }
}

#[async_trait::async_trait]
impl PanelData for FakeData {
    fn guild(&self) -> Option<GuildInfo> {
        Some(GuildInfo { id: "900".into(), name: "MLCI".into(), icon: Some(avatar("M", 260)), members: 1284 })
    }

    fn channels(&self) -> Vec<ChannelInfo> {
        use ChannelKind::*;
        let info = Some(("Information", 0));
        let chat = Some(("Chat", 1));
        let games = Some(("Games", 2));
        let voice = Some(("Voice", 3));
        vec![
            ch(10, "Information", Category, None, 0),
            ch(11, "welcome", Text, info, 0),
            ch(12, "rules", Text, info, 1),
            ch(13, "announcements", Text, info, 2),
            ch(20, "Chat", Category, None, 1),
            ch(21, "general", Text, chat, 0),
            ch(22, "memes", Text, chat, 1),
            ch(23, "desi-banter", Text, chat, 2),
            ch(24, "music", Text, chat, 3),
            ch(30, "Games", Category, None, 2),
            ch(31, "fight-fight-fight", Text, games, 0),
            ch(32, "quiz", Text, games, 1),
            ch(33, "houses", Text, games, 2),
            ch(34, "bot-log", Text, games, 3),
            ch(40, "Voice", Category, None, 3),
            ch(41, "Lounge", Voice, voice, 0),
            ch(42, "Gaming", Voice, voice, 1),
            ch(43, "AFK", Voice, voice, 2),
            ch(SAFE, "safe-corner", Text, chat, 4),
        ]
    }

    fn roles(&self) -> Vec<RoleInfo> {
        let role = |id: u64, name: &str, color: Option<&str>, position: i64, managed: bool| RoleInfo {
            id: id.to_string(),
            name: name.into(),
            color: color.map(String::from),
            position,
            managed,
        };
        vec![
            role(50, "Admin", Some("#e5534b"), 9, false),
            role(51, "Moderator", Some("#e0a43a"), 8, false),
            role(52, "Loduchand", Some("#8b93ff"), 7, true),
            role(53, "House Captain", Some("#c28b2c"), 6, false),
            role(54, "Gryffindor", Some("#9b1b1b"), 5, false),
            role(55, "Slytherin", Some("#1a6b4a"), 4, false),
            role(56, "Ravenclaw", Some("#1f4e8c"), 3, false),
            role(57, "Hufflepuff", Some("#d6a318"), 2, false),
            role(58, "Muggles", None, 1, false),
        ]
    }

    async fn search_members(&self, query: &str, limit: usize) -> Vec<MemberInfo> {
        let q = query.to_lowercase();
        let people = PEOPLE.iter().filter(|p| q.is_empty() || p.1.to_lowercase().contains(&q) || p.2.contains(&q)).map(person);
        let roster = (0..ROSTER.len() as u64).filter_map(|i| self.cached_member(2000 + i)).filter(|m| !q.is_empty() && m.name.to_lowercase().contains(&q));
        people.chain(roster).take(limit).collect()
    }

    async fn member(&self, id: u64) -> Option<MemberInfo> {
        self.cached_member(id)
    }

    fn admins(&self) -> Vec<u64> {
        vec![ADMIN, ADMIN_TWO]
    }

    fn restart(&self) {}

    fn emojis(&self) -> Vec<EmojiInfo> {
        ["pog", "kekw", "bhai_kya_kar_raha_hai", "doge_cool", "mlci_heart", "salute"]
            .iter()
            .enumerate()
            .map(|(i, name)| EmojiInfo {
                id: (912_345_678_901_234_000u64 + i as u64).to_string(),
                name: name.to_string(),
                animated: i == 1,
                url: avatar(name, (i as u32) * 57),
            })
            .collect()
    }

    fn cached_member(&self, id: u64) -> Option<MemberInfo> {
        if let Some(p) = PEOPLE.iter().find(|p| p.0 == id) {
            return Some(person(p));
        }
        let i = id.checked_sub(2000)? as usize;
        let name = ROSTER.get(i)?;
        Some(MemberInfo { id: id.to_string(), name: name.to_string(), username: name.to_lowercase(), avatar: avatar(name, (i as u32) * 37 % 360), bot: false })
    }

    fn house_cup(&self, period: super::houses::Period, now: i64) -> Option<super::houses::HouseCup> {
        let conn = fake_ledger(now).lock();
        let optouts = [2043u64].into_iter().collect();
        let captains = HOUSES_KEYS.iter().enumerate().map(|(i, k)| (*k, Some(2000 + i as u64))).collect();
        let counts = HOUSES_KEYS.iter().zip([312, 298, 287, 301]).map(|(k, n)| (*k, n)).collect();
        super::houses::read(&conn, period, now, &optouts, &captains, &counts).ok()
    }

    async fn agent_settings(&self) -> Option<super::agent::AgentSettings> {
        Some(FAKE_AGENT.lock().clone())
    }

    async fn save_agent_settings(&self, settings: &super::agent::AgentSettings) -> anyhow::Result<()> {
        *FAKE_AGENT.lock() = settings.clone();
        Ok(())
    }

    async fn member_detail(&self, id: u64) -> Option<super::members::MemberDetail> {
        let info = self.cached_member(id)?;
        let roles = match id % 4 {
            0 => vec!["54", "53"],
            1 => vec!["55", "51"],
            2 => vec!["56"],
            _ => vec!["57"],
        };
        let mut role_ids: Vec<String> = roles.into_iter().map(String::from).collect();
        if id == ADMIN || id == ADMIN_TWO {
            role_ids.push("50".into());
        }
        Some(super::members::MemberDetail {
            info,
            joined_at: Some(1_700_000_000 + (id as i64 % 97) * 86_400 * 3),
            created_at: 1_560_000_000 + (id as i64 % 53) * 86_400 * 11,
            role_ids,
        })
    }

    async fn member_stats(&self, id: u64, now: i64) -> super::members::MemberStats {
        use super::members::{JoinSummary, MemberStats};
        if self.cached_member(id).is_none() {
            return MemberStats { hours: vec![0; 24], ..Default::default() };
        }
        let (today_rows, month_rows, all_time) = {
            let conn = fake_ledger(now).lock();
            let sums = |since: i64| -> Vec<(String, i64)> {
                let mut stmt = conn
                    .prepare("SELECT source, SUM(points) FROM ledger WHERE user_id = ?1 AND ts >= ?2 GROUP BY source ORDER BY 2 DESC")
                    .unwrap();
                stmt.query_map(rusqlite::params![id as i64, since], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().flatten().collect()
            };
            let today = super::scorers::day_bounds(now, 0).0;
            (sums(today), sums(super::super::super::points::month_start(now)), sums(0).iter().map(|(_, n)| n).sum::<i64>())
        };
        let seed = id % 97;
        let house = if id >= 2000 { Some(HOUSES_KEYS[((id - 2000) % 4) as usize].to_string()) } else { Some(HOUSES_KEYS[(id % 4) as usize].to_string()) };
        let hours: Vec<i64> = (0..24).map(|h: i64| {
            let evening = (h - 21).abs().min((h + 3).abs());
            ((12 - evening.min(12)) * (3 + seed as i64 % 5) + if (10..14).contains(&h) { 9 } else { 0 }).max(0)
        }).collect();
        MemberStats {
            house,
            captain: id == 2000 || id == 2002,
            muggle: id == 2043,
            today: today_rows,
            month: month_rows,
            all_time: all_time + 140,
            house_rank: Some((1 + (seed % 7) as usize, 38)),
            weekly_this_week: 3,
            messages_today: 17 + seed as i64 % 9,
            chat_counted_today: 14 + seed as i64 % 9,
            messages_7d: 164 + seed as i64 * 2,
            messages_30d: 712 + seed as i64 * 9,
            top_channels: vec![(21, 318), (23, 201), (22, 96), (32, 61), (24, 36)],
            hours,
            voice_today_secs: 42 * 60,
            voice_points_today_secs: 42 * 60,
            voice_7d_secs: 385 * 60,
            quiz_all: 214,
            quiz_month: 37,
            fights: 58,
            wins: 34,
            crowns: 2,
            snitch_month: 5,
            joins: Some(JoinSummary {
                joins: 3,
                leaves: 2,
                first_join: Some("2023-02-11T18:04:00+00:00".into()),
                last_join: Some("2026-06-02T15:40:00+00:00".into()),
                last_leave: Some("2026-05-28T09:12:00+00:00".into()),
            }),
        }
    }

    async fn member_seen(&self, id: u64, now: i64) -> Vec<super::members::SeenMessage> {
        let lines = [
            ("chat", 21, "Loduchand bhai, who's winning the house cup this month? Ravenclaw again?"),
            ("silent_read", 23, "bro that last quiz question about Sholay was criminal, nobody got it"),
            ("silent_read", 21, "chai break, back in 10"),
            ("chat", 32, "can you give me a hint for the koto today? not the answer, just a hint"),
            ("silent_read", 22, "this meme is literally Dev every Monday 💀"),
            ("chat", 21, "roast Arjun's fantasy team please, he picked 4 bowlers"),
            ("silent_read", 23, "RCB will win this year, I'm not taking questions"),
        ];
        let _ = id;
        lines
            .iter()
            .enumerate()
            .map(|(i, (kind, ch, text))| super::members::SeenMessage {
                ts: now - 1300 - (i as i64) * 5_400,
                channel_id: Some(*ch),
                kind: kind.to_string(),
                text: text.to_string(),
            })
            .collect()
    }

    async fn memories(&self) -> Vec<super::members::MemoryEntry> {
        let now = chrono::Utc::now().timestamp();
        let m = |slug: &str, title: &str, content: &str, days: i64, tags: &[&str]| super::members::MemoryEntry {
            slug: slug.into(),
            title: title.into(),
            content: content.into(),
            ts: now - days * 86_400,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            keywords: vec![],
        };
        vec![
            m("people-sameer", "Sameer", "Sameer (DiscordId: 2012) is a Gryffindor who never misses quiz night. Supports RCB loudly and takes roasts about it well. Asked the bot to stop calling him 'Sam'.", 3, &["people"]),
            m("running-jokes", "Running jokes", "Dev and his Monday memes. Sameer's RCB prediction every April. Zoya disappears for a month every exam season.", 12, &["jokes"]),
            m("quiz-night", "Quiz night", "Every Friday at 9 PM in #quiz. Kabir hosts. Sameer and Yash usually top the board.", 20, &["events"]),
            m("snitch-rules", "Snitch rules", "Drops land in general, memes and desi-banter. Nobody may camp a channel.", 30, &["games"]),
        ]
    }

    async fn activity(&self, now: i64) -> Vec<super::super::profiles::Activity> {
        let conn = fake_ledger(now).lock();
        let mut points = std::collections::HashMap::new();
        let _ = super::profiles::ledger_points(&conn, now - 30 * 86_400, &mut |u, _, n| {
            points.insert(u, n);
        });
        (0..ROSTER.len() as u64)
            .map(|i| super::super::profiles::Activity {
                user_id: 2000 + i,
                messages: ((i * 37 + 11) % 97) as i64 * 9,
                voice_secs: ((i * 53 + 7) % 41) as i64 * 1_500,
                points: points.get(&(2000 + i)).copied().unwrap_or(0),
                house: Some(HOUSES_KEYS[(i % 4) as usize].to_string()),
                muggle: i == 43,
            })
            .collect()
    }

    async fn member_messages(&self, id: u64, now: i64) -> Vec<super::super::profiles::RawMessage> {
        let m = |ago: i64, channel: u64, dm: bool, text: &str| super::super::profiles::RawMessage {
            ts: now - ago,
            channel_id: Some(channel),
            parent_id: None,
            is_dm: dm,
            text: text.to_string(),
        };
        let chatter = [
            (21, "gm gm, chai ready? ☕"),
            (23, "RCB this year for sure, write it down"),
            (32, "koto today was brutal, took me 6 tries"),
            (22, "this meme is Dev every monday 💀"),
            (21, "Loduchand who is winning the house cup"),
            (24, "Arijit on loop again"),
            (32, "quiz night friday? I'm in"),
            (23, "bhai <@2010> your fantasy team has 4 bowlers https://fantasy.example.com/team/9"),
        ];
        let mut out: Vec<_> = (0..48).map(|i| {
            let (c, t) = chatter[i % chatter.len()];
            m(3_000 + i as i64 * 40_000, c, false, t)
        }).collect();
        out.push(m(5_000, SAFE, false, "SECRET-SAFE I've been struggling a lot lately"));
        out.push(m(6_000, 77, false, "SECRET-THREAD in a thread under safe corner"));
        out.push(m(7_000, 5_000, true, "SECRET-DM just between you and me"));
        out.push(m(8_000, 999, false, "SECRET-UNKNOWN somewhere private"));
        out.push(m(9_000, 21, false, "[replying to @Dev: \"SECRET-QUOTE someone else's words\"]\nlol same"));
        let _ = id;
        out
    }

    fn thread_parent(&self, channel: u64) -> Option<u64> {
        match channel {
            77 => Some(SAFE),
            78 => Some(21),
            _ => None,
        }
    }

    async fn search_window(&self, filter: super::search::Filter) -> anyhow::Result<super::search::Window> {
        let conn = fake_history().lock();
        Ok(super::search::read_window(&conn, "lodu", &filter)?)
    }

    fn member_house(&self, id: u64) -> Option<&'static super::super::super::house::House> {
        self.cached_member(id)?;
        let key = if id >= 2000 { HOUSES_KEYS[((id - 2000) % 4) as usize] } else { HOUSES_KEYS[(id % 4) as usize] };
        super::super::super::house::house(key)
    }

    fn duels(&self, since: i64) -> Vec<(u64, u64, i64)> {
        let now = chrono::Utc::now().timestamp();
        let pairs = [(2012u64, 2010u64, 9usize, 5usize), (2004, 2016, 6, 6), (2023, 2001, 7, 2), (2008, 2020, 3, 4), (2030, 2005, 4, 1)];
        let mut out = Vec::new();
        for (i, (a, b, wa, wb)) in pairs.iter().enumerate() {
            for k in 0..(*wa + *wb) {
                let ts = now - ((k * 7 + i * 3) as i64 % 28) * 86_400 - (k as i64 * 3_600);
                out.push(if k < *wa { (*a, *b, ts) } else { (*b, *a, ts) });
            }
        }
        out.into_iter().filter(|d| d.2 >= since).collect()
    }

    fn hour_counts(&self, _since_day: &str) -> Vec<(u64, i64, i64, i64)> {
        (0..ROSTER.len() as u64)
            .map(|i| {
                let total = 150 + ((i * 97) % 900) as i64;
                let night = total * ((i * 13) % 37) as i64 / 100;
                let early = total * ((i * 29) % 23) as i64 / 100;
                (2000 + i, night, early, total)
            })
            .collect()
    }

    async fn history_requests(
        &self,
        since: i64,
        progress: Arc<dyn Fn(usize) + Send + Sync>,
    ) -> anyhow::Result<Vec<super::super::insights::StoredRequest>> {
        use super::super::insights::StoredRequest;
        let base = since.max(chrono::Utc::now().timestamp() - 86_400);
        let r = |ts: i64, author: u64, id: u64, channel: u64, replied: Option<u64>| StoredRequest {
            ts: base + ts,
            author,
            message_id: id,
            channel_id: Some(channel),
            is_dm: false,
            replied_message_id: replied,
            mentions: vec![],
        };
        progress(5);
        Ok(vec![
            r(10, 3101, 910_001, 21, None),
            r(20, 3102, 910_002, 21, Some(910_001)),
            r(30, 3101, 910_003, 21, Some(910_002)),
            r(40, 3102, 910_004, SAFE, Some(910_003)),
            r(50, 3103, 910_005, 21, Some(999_999_999)),
        ])
    }

    async fn ask_model(&self, prompt: String) -> anyhow::Result<(String, String)> {
        use std::sync::atomic::Ordering;
        PROMPTS.lock().push(prompt);
        let call = MODEL_CALLS.fetch_add(1, Ordering::SeqCst);
        let delay = MODEL_DELAY_MS.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        let bad = match MODEL_MODE.load(Ordering::SeqCst) {
            0 => false,
            1 => {
                MODEL_MODE.store(0, Ordering::SeqCst);
                true
            }
            _ => true,
        };
        let _ = call;
        if bad {
            return Ok(("Sorry, I can't help with that.".into(), "fake/model-1".into()));
        }
        if PROMPTS.lock().last().is_some_and(|p| p.contains("writing one scheduled post")) {
            return Ok((
                "\"Chai or coffee, and what's your go-to order at the tapri? ☕ Bonus points if it's as dramatic as <@2010>'s. @everyone answer below!\"".into(),
                "fake/model-1".into(),
            ));
        }
        Ok((
            json!({
                "summary": "Shows up most evenings in #general and #desi-banter with cricket takes and quiz chatter. Plays Koto and quiz nights regularly, often teasing <@2010> about fantasy picks. Friendly, fast replies. See https://example.com",
                "interests": ["cricket (RCB)", "quiz nights", "Koto", "memes", "Arijit songs"],
                "style": "Short Hinglish messages, lots of 💀 and ☕, jokes more than arguments.",
                "games": "Regular in quiz and Koto; wins some fights, caught a few Snitches.",
                "vibe_with_bot": "Asks the bot for standings and hints; playful.",
                "suggested_tone": "light_roast",
                "tone_reason": "Enjoys banter and dishes it out himself.",
                "roast_material": ["six tries at Koto on a good day", "the annual RCB prediction"],
                "avoid": ["exam results"]
            })
            .to_string(),
            "fake/model-1".into(),
        ))
    }

    fn house_standings(&self, now: i64) -> Vec<super::super::posts::Standing> {
        let conn = fake_ledger(now).lock();
        let points = |f: fn(&rusqlite::Connection, i64) -> rusqlite::Result<std::collections::HashMap<&'static str, i64>>| f(&conn, super::super::super::points::month_start(now)).unwrap_or_default();
        super::super::posts::standings_from(&points(super::super::super::points::house_totals))
    }

    async fn send_test_post(&self, reminder: &Reminder) -> Result<(), String> {
        TEST_POSTS.lock().push(reminder.id);
        Ok(())
    }

    async fn join_summary(&self, id: u64) -> Option<super::members::JoinSummary> {
        use super::members::JoinSummary;
        let at = |t: &str| Some(t.to_string());
        match id {
            LUCKY => Some(JoinSummary { joins: 2, leaves: 2, first_join: at("2024-03-02T10:00:00+00:00"), last_join: at("2025-11-20T18:30:00+00:00"), last_leave: at("2026-09-07T09:54:54+00:00") }),
            1007 => Some(JoinSummary { joins: 6, leaves: 6, first_join: at("2023-01-05T12:00:00+00:00"), last_join: at("2026-09-11T20:10:00+00:00"), last_leave: at("2026-09-12T08:00:00+00:00") }),
            _ if self.cached_member(id).is_some() => Some(JoinSummary { joins: 3, leaves: 2, first_join: at("2023-02-11T18:04:00+00:00"), last_join: at("2026-06-02T15:40:00+00:00"), last_leave: at("2026-05-28T09:12:00+00:00") }),
            _ => None,
        }
    }

    async fn user(&self, id: u64) -> Option<MemberInfo> {
        OUTSIDERS.iter().find(|p| p.0 == id).map(person).or_else(|| self.cached_member(id))
    }

    async fn send_unpinged(&self, channel: u64, text: String) -> Result<(), String> {
        UNPINGED_POSTS.lock().push((channel, text));
        Ok(())
    }

    async fn cancel_trade(&self, id: i64, by: u64) -> Result<super::super::super::frog_trade::Trade, String> {
        let db = super::super::super::frog_store::db().ok_or("The frog store isn't open.")?;
        let closed = super::super::super::frog_trade::close(&db.lock(), id, "cancelled", Some(by), chrono::Utc::now().timestamp()).map_err(|e| e.to_string())?;
        closed.ok_or_else(|| "That offer isn't open any more.".to_string())
    }

    /// A frog opened straight in the store, as if Discord took the post.
    async fn drop_frog(&self, channel: u64, by: u64) -> Result<super::super::super::frog_store::Drop, String> {
        use super::super::super::frog_store as store;
        let db = store::db().ok_or("The frog store isn't open.")?;
        let conn = db.lock();
        if store::channel_busy(&conn, channel) {
            return Err("A frog is already hopping about in that channel.".into());
        }
        let wizard = store::pick_wizard(&conn, 0.1, 0.6).ok_or("No wizard is switched on.")?;
        let riddle = store::pick_riddle(&conn, wizard.rarity.difficulty(), 0.0).ok_or("The riddle bank is empty.")?;
        let now = chrono::Utc::now().timestamp();
        let pending = store::start_drop(&conn, channel, &wizard, &riddle, now, Some(by)).map_err(|e| e.to_string())?;
        store::mark_open(&conn, pending.id, 1_300_000_000_000_000_000 + pending.id as u64, now, 300)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "The frog vanished.".to_string())
    }

    fn scorers(&self, days_back: i64, now: i64) -> Option<Vec<super::scorers::ScorerData>> {
        let (start, end) = super::scorers::day_bounds(now, days_back);
        let conn = fake_ledger(now).lock();
        let ledger = super::scorers::read_ledger(&conn, start, end).ok()?;
        let members: std::collections::HashMap<u64, String> =
            (0..ROSTER.len() as u64).map(|i| (2000 + i, HOUSES_KEYS[(i % 4) as usize].to_string())).collect();
        let messages = (0..ROSTER.len() as u64).map(|i| (2000 + i, ((i * 7 + days_back as u64 * 3) % 31) as i64)).collect();
        let voice = (0..ROSTER.len() as u64).map(|i| (2000 + i, (((i * 13) % 75) * 60) as i64)).collect();
        let optouts = [2043u64].into_iter().collect();
        Some(super::scorers::assemble(ledger, &members, &messages, &voice, &optouts, 20, 3600))
    }
}

const SAFE: u64 = 1543162777642868736;

/// Lucky, whose special welcome is seeded, and the two who missed him: Discord
/// knows them, the fake server's member list doesn't.
const LUCKY: u64 = 459076776266694670;
const OUTSIDERS: &[(u64, &str, &str, u32)] = &[
    (LUCKY, "Lucky", "lucky.exe", 40),
    (302861753409208322, "Def Not Kohli", "defnotkohli", 205),
    (1453059765692272765, "MahoganyDesk", "mahoganydesk", 20),
];
static UNPINGED_POSTS: std::sync::LazyLock<parking_lot::Mutex<Vec<(u64, String)>>> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(Vec::new()));

/// How the fake model answers: 0 well, 1 badly once then well, 2 always badly.
static MODEL_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static MODEL_CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static MODEL_DELAY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static TEST_POSTS: std::sync::LazyLock<parking_lot::Mutex<Vec<i64>>> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(Vec::new()));
static PROMPTS: std::sync::LazyLock<parking_lot::Mutex<Vec<String>>> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(Vec::new()));

const HOUSES_KEYS: [&str; 4] = ["gryffindor", "slytherin", "ravenclaw", "hufflepuff"];

/// Members of the fake houses, ids 2000 and up; the member at `id % 4` picks the house.
const ROSTER: &[&str] = &[
    "Aarav", "Diya", "Kabir", "Meera", "Vihaan", "Anaya", "Rohan", "Zoya", "Arjun", "Ishita", "Dev", "Tanvi",
    "Sameer", "Riya", "Yash", "Nisha", "Aditya", "Pooja", "Karan", "Sana", "Nikhil", "Aisha", "Rahul", "Kavya",
    "Varun", "Myra", "Siddharth", "Neha", "Harsh", "Ira", "Kunal", "Mehak", "Parth", "Simran", "Aman", "Tara",
    "Om", "Jiya", "Vivek", "Anika", "Rudra", "Pari", "Laksh", "Muggle Mike",
];

static FAKE_AGENT: std::sync::LazyLock<parking_lot::Mutex<super::agent::AgentSettings>> = std::sync::LazyLock::new(|| {
    parking_lot::Mutex::new(super::agent::AgentSettings {
        name: "Loduchand".into(),
        description: "the resident bot of the MLCI Discord server".into(),
        system_prompt: "You are Loduchand, the bot of the MLCI server: a community of friends who chat about films, \
            games, cricket and life.\n\nSpeak like a friend in the group chat, not a customer-service agent. Keep \
            replies short unless someone asks for detail. Mix English and Hindi the way the members do.\n\nNever \
            share anyone's personal information, and stay out of arguments in #safe-corner."
            .into(),
        core: "# CORE\n\n## People\n- Kabir runs the quiz nights.\n- Meera prefers replies without emoji.\n\n\
            ## Running jokes\n- The Snitch always lands in #memes when nobody is around.\n"
            .into(),
        model: "openrouter/google/gemini-2.5-flash".into(),
        thinking_depth: 8,
        silent_read_initiative_chance: 0.04,
        max_tokens: Some(1200),
    })
});

/// A house ledger with about six weeks of made-up points, the busiest in the last day.
fn fake_ledger(now: i64) -> &'static parking_lot::Mutex<rusqlite::Connection> {
    static LEDGER: OnceLock<parking_lot::Mutex<rusqlite::Connection>> = OnceLock::new();
    LEDGER.get_or_init(|| {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::super::points::SCHEMA).unwrap();
        let mut seed: u64 = 0x5eed_cafe;
        let mut roll = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n
        };
        let sources: &[(&str, i64, &str, u64)] = &[
            ("chat", 1, "Daily chat points", 30),
            ("voice", 1, "Time in voice", 18),
            ("quiz", 3, "Quiz round: 3 right", 14),
            ("koto", 2, "Solved today's Koto", 6),
            ("anagram", 2, "Anagram: PLANET", 6),
            ("cat", 1, "Caught a cat", 5),
            ("arena", 3, "Won a 1v1", 8),
            ("royale", 10, "Battle royale champion", 1),
            ("snitch", 5, "Caught the Snitch", 6),
            ("golden_snitch", 25, "Caught the Golden Snitch", 1),
            ("weekly", 3, "A post in #safe-corner about a rough week", 3),
            ("mod", 15, "Meme contest winner", 2),
        ];
        let weights: u64 = sources.iter().map(|s| s.3).sum();
        for i in 0..1400u64 {
            // Most rows are old; the last 200 fall in the past day.
            let age = if i >= 1200 { (1400 - i) * 420 + roll(300) } else { 86_400 + roll(40 * 86_400) };
            let mut pick = roll(weights);
            let source = sources.iter().find(|s| if pick < s.3 { true } else { pick -= s.3; false }).unwrap();
            let user = 2000 + roll(ROSTER.len() as u64);
            // Gryffindor and Ravenclaw are a little busier this month.
            let house = if roll(10) < 2 { [0usize, 2][roll(2) as usize] } else { ((user - 2000) % 4) as usize };
            let user = if house as u64 == (user - 2000) % 4 { user } else { 2000 + (house as u64) + (roll(10) * 4) % 40 };
            let points = source.1 + roll(source.1 as u64 + 1) as i64;
            let ts = now - age as i64;
            conn.execute(
                "INSERT INTO ledger (user_id, house, source, points, reason, day, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    if source.0 == "mod" { None } else { Some(user as i64) },
                    HOUSES_KEYS[house],
                    source.0,
                    points,
                    source.2,
                    super::super::super::points::ist_day(ts),
                    ts
                ],
            )
            .unwrap();
        }
        parking_lot::Mutex::new(conn)
    })
}

/// A stored-history table like vizier.db's, with a month of member chatter and
/// the bot's answers, plus the rows the message-search tests look for.
fn fake_history() -> &'static parking_lot::Mutex<rusqlite::Connection> {
    static HISTORY: OnceLock<parking_lot::Mutex<rusqlite::Connection>> = OnceLock::new();
    HISTORY.get_or_init(|| {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_history (uid TEXT PRIMARY KEY, agent_id TEXT NOT NULL, channel TEXT NOT NULL, topic TEXT,
                 timestamp INTEGER NOT NULL, content_type TEXT NOT NULL, data TEXT NOT NULL);
             CREATE INDEX idx_sh_agent_time ON session_history(agent_id, timestamp);",
        )
        .unwrap();
        let now_ms = chrono::Utc::now().timestamp_millis();
        let mut n = 0u64;
        let mut add = |ago_ms: i64, column: &str, ctype: &str, who: (u64, &str), text: &str, meta: Value| {
            n += 1;
            let data = json!({ "uid": format!("u{n}"), "content": { ctype: {
                "timestamp": "2026-09-14T10:00:00Z",
                "user": format!("@{} (DiscordId: {})", who.1, who.0),
                "content": { if n % 5 == 0 { "chat" } else { "silent_read" }: text },
                "metadata": meta,
                "attachments": [],
            }}})
            .to_string();
            conn.execute(
                "INSERT INTO session_history VALUES (?1, 'lodu', ?2, NULL, ?3, ?4, ?5)",
                rusqlite::params![format!("u{n}"), column, now_ms - ago_ms, ctype, data],
            )
            .unwrap();
            // Another agent's copy never shows.
            if n % 50 == 0 {
                conn.execute("INSERT INTO session_history VALUES (?1, 'other', ?2, NULL, ?3, 'Request', ?4)", rusqlite::params![format!("o{n}"), column, now_ms - ago_ms, data]).unwrap();
            }
        };
        let meta = |channel: u64, id: Option<u64>, reply: Option<(&str, &str)>| {
            json!({
                "discord_channel_id": channel.to_string(), "is_dm": false, "message_id": id.map(|i| i.to_string()),
                "is_reply_message": reply.is_some(), "replied_message_author": reply.map(|r| r.0), "replied_message_content": reply.map(|r| r.1),
                "sent_at": "2026-09-14 10:00:00 UTC",
            })
        };
        let chatter: &[(u64, &str)] = &[
            (32, "koto today was brutal, took me 6 tries"),
            (21, "gm gm, chai ready? ☕"),
            (23, "RCB this year for sure, write it down"),
            (32, "who solved koto in 3?? teach me"),
            (22, "this meme is Dev every monday 💀"),
            (24, "Arijit on loop again, no regrets"),
            (32, "bhai koto ka answer mat batana abhi, I'm still on try 4"),
            (21, "Loduchand who is winning the house cup this month?"),
            (31, "anyone up for a 1v1 in the arena?"),
            (22, "the snitch just dropped in memes and I missed it AGAIN"),
            (21, "chai break, back in 10"),
            (32, "that Sholay question in the quiz was criminal, nobody got it"),
            (32, "koto streak 14 days, don't jinx it"),
            (21, "who's on voice tonight? lounge at 10"),
            (23, "my fantasy team has 4 bowlers, send help"),
            (32, "quiz night friday? I'm in, Kabir better go easy"),
        ];
        let replies = [("Zoya", "who's playing koto today?"), ("Dev", "RCB will choke again, watch"), ("Meera", "quiz starts in 5, get in")];
        for i in 0..520u64 {
            let (channel, text) = chatter[(i as usize * 7) % chatter.len()];
            let who = 2000 + (i * 13) % 43;
            let ago = 40 * 60_000 + i as i64 * 4_700_000 + ((i * 977) % 600_000) as i64;
            let reply = (i % 6 == 2).then(|| replies[(i as usize / 6) % replies.len()]);
            add(ago, &format!("discord__{channel}"), "Request", (who, ROSTER[(who - 2000) as usize]), text, meta(channel, Some(1_300_000_000_000_000_000 + i), reply));
            if i % 2 == 0 {
                add(ago - 4_000, &format!("discord__{channel}"), "Response", (0, "Loduchand"), "koto hint: think of a fruit 🍋", json!({}));
            }
        }
        // What the search tests look for.
        let day = 86_400_000i64;
        add(2 * 3_600_000, "discord__21", "Request", (2012, "Sameer"), "Anyone seen the ZEBRAFISH meme?", meta(21, Some(7001), Some(("Dev", "which meme are you on about"))));
        add(2 * 3_600_000 + 10, "discord__5000", "Request", (2007, "Zoya"), "zebrafish secret in a DM", json!({ "discord_channel_id": "5000", "is_dm": true, "message_id": "7002" }));
        add(2 * 3_600_000 + 20, &format!("discord__{SAFE}"), "Request", (2012, "Sameer"), "zebrafish SECRET-SAFE", meta(SAFE, Some(7003), None));
        add(2 * 3_600_000 + 30, "discord__77", "Request", (2012, "Sameer"), "zebrafish SECRET-THREAD", meta(77, Some(7004), None));
        add(2 * 3_600_000 + 40, "discord__999", "Request", (2012, "Sameer"), "zebrafish SECRET-UNKNOWN", meta(999, Some(7005), None));
        add(3 * day, "discord__22", "Request", (2010, "Dev"), "zebrafish again lol", json!({ "discord_channel_id": "22", "is_dm": false }));
        add(3 * day + 10, "discord__21", "Request", (MEMBER, "Rohan"), "[replying to @Dev: \"zebrafish SECRET-QUOTE?\"]\nyes", meta(21, Some(7007), Some(("Dev", "zebrafish SECRET-QUOTE?"))));
        add(3 * day + 20, "discord__21", "Request", (2004, "Vihaan"), "[Private notes from the server mods about people in this message. zebrafish SECRET-NOTES]\n- @Vihaan: fine\n[End of notes]\nhello there", meta(21, Some(7008), None));
        add(3 * day + 30, "discord__21", "Response", (0, "Loduchand"), "zebrafish SECRET-BOT", json!({}));
        add(4 * day, "discord__78", "Request", (2008, "Arjun"), "zebrafish in a thread under general", meta(78, Some(7010), None));
        add(4 * day + 10, "discord__21__koto", "Request", (2008, "Arjun"), "zebrafish with a topic", meta(21, Some(7011), None));
        add(5 * day, "discord__21", "Request", (20_120, "Imposter"), "zebrafish from someone else", meta(21, Some(7012), None));
        add(60 * day, "discord__21", "Request", (2012, "Sameer"), "zebrafish from long ago", meta(21, Some(7013), None));
        add(2 * day, "discord__23", "Request", (2023, "Kavya"), &format!("{} So yes, the zebrafish thing was real. {}",
            "Okay long story. We went to the aquarium on Saturday because the metro was shut, and the whole group decided it was a good idea to argue about which fish would win a 1v1 in the arena. Dev said shark, obviously, Zoya said octopus because of the eight arms, and Nikhil kept saying the tiny striped one in the corner tank.",
            "The guide told us they can regrow parts of their heart, which honestly is more than I can say for RCB fans every April. Anyway, if anyone wants to go again next weekend, I'm in, but Nikhil is not choosing the snacks this time."), meta(23, Some(7019), Some(("Nikhil", "wait what happened at the aquarium"))));
        add(1 * day, "discord__23", "Request", (2003, "Meera"), "ÉCLAIR au café, anyone?", meta(23, Some(7014), None));
        add(1 * day + 10, "discord__23", "Request", (2003, "Meera"), "this is 100% real", meta(23, Some(7015), None));
        add(1 * day + 20, "discord__23", "Request", (2003, "Meera"), "this is 1000 real", meta(23, Some(7016), None));
        add(1 * day + 30, "discord__23", "Request", (2003, "Meera"), "snake a_b case", meta(23, Some(7017), None));
        add(1 * day + 40, "discord__23", "Request", (2003, "Meera"), "snake aXb case", meta(23, Some(7018), None));
        for i in 0..7i64 {
            add(6 * day + i * 3_600_000, "discord__24", "Request", (2011, "Tanvi"), &format!("pagetoken number {}", i + 1), meta(24, Some(7100 + i as u64), None));
        }
        add(29 * day + 20 * 3_600_000, "discord__21", "Request", (2012, "Sameer"), "the oldtoken is here", meta(21, Some(7200), None));
        parking_lot::Mutex::new(conn)
    })
}

pub fn fake_catalog() -> Vec<Section> {
    let s = |key, label, help, kind, default, live| Setting { key, label, help, kind, default, live };
    let c = |name, who, usage, what| Command { name, who, usage, what };
    vec![
        Section {
            id: "arena",
            title: "Arena",
            icon: "⚔️",
            about: "1v1 fights with /fight and battle royales with /battle. Fighters pick a type, the bot tells the \
                    fight round by round, and the winner takes points for their house.",
            settings: vec![
                s("PANEL_TEST_ARENA", "Arena", "Switches fights and battle royales on or off for everyone.", Kind::Toggle, "on", true),
                s("VIZIER_FIGHT_CHANNEL", "Fight channel", "Where challenges and fights are posted. If empty, a channel named fight-fight-fight is used.", Kind::Channel, "", true),
                s("PANEL_TEST_FIGHT_POINTS", "Points for a win", "House points the winner of a 1v1 takes home.", Kind::Number { min: 0, max: 500, unit: "points" }, "10", true),
                s("PANEL_TEST_ROYALE_MAX", "Royale size limit", "The most fighters a battle royale takes before the bracket closes.", Kind::Number { min: 4, max: 64, unit: "fighters" }, "32", true),
                s("PANEL_TEST_FIGHT_STYLE", "Fight style", "How rounds are shown in the channel.", Kind::Choice { options: &[("cards", "Image cards"), ("text", "Plain text")] }, "cards", false),
            ],
            commands: vec![
                c("fight", "Everyone", "/fight @someone type:", "Challenge someone to a 1v1."),
                c("battle", "Admins", "/battle size:", "Open a battle royale and draw the bracket when it fills."),
                c("warrior", "Everyone", "/warrior", "Your fighter card: wins, losses and favourite type."),
                c("fightboard", "Everyone", "/fightboard", "The top fighters this season."),
                c("battle_stop", "Admins", "/battle_stop", "Stop the battle royale that's running."),
            ],
        },
        Section {
            id: "snitch",
            title: "Snitch",
            icon: "✨",
            about: "The Golden Snitch drops into a chat channel at random times. The first to catch it wins points \
                    for their house, so busy channels get lively.",
            settings: vec![
                s("PANEL_TEST_SNITCH", "Snitch drops", "Scheduled drops. /snitch still works for admins when this is off.", Kind::Toggle, "on", true),
                s("PANEL_TEST_SNITCH_CHANNELS", "Drop channels", "Where drops land and how often: weight 3 gets three times the drops of weight 1.", Kind::WeightedChannels, "", true),
                s("PANEL_TEST_SNITCH_PER_DAY", "Drops a day", "About how many drops to spread across a day.", Kind::Number { min: 0, max: 48, unit: "drops" }, "6", true),
                s("PANEL_TEST_SNITCH_QUIET_FROM", "Quiet from", "No drops from this time (India time)…", Kind::Time, "01:00", true),
                s("PANEL_TEST_SNITCH_QUIET_TO", "Quiet until", "…until this time.", Kind::Time, "08:00", true),
            ],
            commands: vec![c("snitch", "Admins", "/snitch", "Drop a Snitch right now in a random drop channel.")],
        },
        Section {
            id: "houses",
            title: "Houses",
            icon: "🏰",
            about: "Four houses compete all season. Chat, voice time, fights, quizzes and Snitch catches all add \
                    points; standings post every hour in the houses channel.",
            settings: vec![
                s("PANEL_TEST_HOUSE_CHANNEL", "Houses channel", "Where standings and sorting announcements go.", Kind::Channel, "", true),
                s("PANEL_TEST_MUGGLE_ROLE", "Muggles role", "Given to members who step out of the house cup.", Kind::Role, "", true),
                s("PANEL_TEST_HOUSE_CAPTAINS", "Captains", "Members who can give and take points with /housepoints.", Kind::Users, "", true),
                s("PANEL_TEST_CHAT_RATE", "Chat points", "Points a message earns, before the daily cap.", Kind::Decimal { min: 0.0, max: 5.0, unit: "per message" }, "0.2", true),
                s("PANEL_TEST_AFK", "AFK channel", "Time spent here doesn't count as voice time.", Kind::VoiceChannel, "", false),
            ],
            commands: vec![
                c("housepoints", "House captains", "/housepoints house: points: reason:", "Give or take points, with a reason everyone sees."),
                c("mypoints", "Everyone", "/mypoints", "Your points this season and where they came from."),
                c("housetop", "Everyone", "/housetop", "The top five of each house."),
                c("sort", "Admins", "/sort", "Sort everyone without a house into one."),
                c("captain", "Admins", "/captain house: @member", "Make someone captain of a house."),
            ],
        },
        Section {
            id: "quiz",
            title: "Quiz",
            icon: "🧠",
            about: "Timed trivia rounds in the quiz channel. Players vote on a genre, answer with buttons and the \
                    top three of each round get points.",
            settings: vec![
                s("PANEL_TEST_QUIZ", "Quiz", "Lets members start rounds with /quiz.", Kind::Toggle, "on", true),
                s("PANEL_TEST_QUIZ_CHANNEL", "Quiz channel", "The only channel rounds run in.", Kind::Channel, "", true),
                s("PANEL_TEST_QUIZ_TIMEOUT", "Time to answer", "How long each question stays open.", Kind::Number { min: 10, max: 120, unit: "seconds" }, "30", true),
                s("PANEL_TEST_QUIZ_DIFFICULTY", "Difficulty", "Which questions a round draws from.", Kind::Choice { options: &[("easy", "Easy"), ("mixed", "Mixed"), ("hard", "Hard")] }, "mixed", true),
                s("PANEL_TEST_QUIZ_FEEDS", "News feeds", "RSS feeds for current-affairs questions, comma-separated.", Kind::Text, "", false),
            ],
            commands: vec![
                c("quiz", "Everyone", "/quiz", "Start a round: vote a genre, then ten questions."),
                c("quizboard", "Everyone", "/quizboard", "The season's quiz leaderboard."),
                c("quiz_add", "Admins", "/quiz_add question: answer:", "Add a question to the bank."),
                c("quiz_stop", "Admins", "/quiz_stop", "End the running round."),
            ],
        },
        Section {
            id: "autoreplies",
            title: "Auto-responses",
            icon: "💬",
            about: "Rules made on the Auto-responses page that react to or answer messages containing set words.",
            settings: vec![s("VIZIER_AUTOREPLIES", "Auto-responses on", "Master switch for every auto-response rule.", Kind::Toggle, "on", true)],
            commands: vec![],
        },
    ]
}

/// The demo's catalog: the fake sections plus the real Chocolate Frogs one.
fn demo_catalog() -> Vec<Section> {
    let mut sections = fake_catalog();
    sections.extend(super::super::catalog::sections().into_iter().filter(|s| s.id == "frogs"));
    sections
}

fn demo_panel() -> Router {
    store();
    router(Panel::new(Arc::new(FakeData), demo_catalog))
}

static STORE: OnceLock<tempfile::TempDir> = OnceLock::new();

/// Opens control.db once for the whole test binary.
pub fn store() {
    STORE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temp dir");
        super::super::open(dir.path().to_str().unwrap()).expect("control store");
        // A small riddle bank for the Chocolate Frog page.
        let bank = dir.path().join("riddlebank/ai");
        std::fs::create_dir_all(&bank).unwrap();
        let riddles = [
            ("t-001", "easy", "I have keys but open no locks. What am I?", "keyboard"),
            ("t-002", "easy", "The more you take, the more you leave behind. What are they?", "footsteps"),
            ("t-003", "easy", "What has a neck but no head?", "bottle"),
            ("t-004", "medium", "I speak without a mouth and hear without ears. What am I?", "echo"),
            ("t-005", "hard", "The more of me there is, the less you see. What am I?", "darkness"),
        ];
        let lines: Vec<String> = riddles
            .iter()
            .map(|(id, d, r, a)| json!({ "id": id, "topic": "test", "difficulty": d, "riddle": r, "answers": [a], "lang": "en" }).to_string())
            .collect();
        std::fs::write(bank.join("test.jsonl"), lines.join("\n")).unwrap();
        super::super::super::frog_store::open(dir.path().to_str().unwrap()).expect("frog store");
        dir
    });
}

pub fn panel() -> Router {
    store();
    router(Panel::new(Arc::new(FakeData), fake_catalog))
}

pub fn session_for(user: u64) -> String {
    store();
    let link = super::super::create_link(user).expect("link");
    super::super::redeem_link(&link).expect("session").0
}

async fn call(app: &Router, method: &str, path: &str, session: Option<&str>, body: Option<Value>, header: bool) -> (StatusCode, Value, axum::http::HeaderMap) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(s) = session {
        req = req.header("cookie", format!("other=1; mlci_panel={}", s));
    }
    if header {
        req = req.header("x-panel", "1");
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let res = app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), 10 << 20).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, value, headers)
}

// --- sign-in -------------------------------------------------------------------------

#[tokio::test]
async fn login_swaps_a_link_for_a_session_once() {
    let app = panel();
    let link = super::super::create_link(ADMIN).unwrap();
    let (status, body, headers) = call(&app, "POST", "/api/login", None, Some(json!({ "token": link })), true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], "1001");
    assert_eq!(body["name"], "Kabir");
    let cookie = headers.get("set-cookie").unwrap().to_str().unwrap().to_string();
    for part in ["mlci_panel=", "HttpOnly", "Secure", "SameSite=Strict", "Path=/", "Max-Age=604800"] {
        assert!(cookie.contains(part), "cookie lacks {part}: {cookie}");
    }
    let session = cookie.trim_start_matches("mlci_panel=").split(';').next().unwrap().to_string();
    let (status, me, _) = call(&app, "GET", "/api/me", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(me["session_expires"].as_i64().unwrap() > chrono::Utc::now().timestamp());

    // The same link a second time.
    let (status, body, _) = call(&app, "POST", "/api/login", None, Some(json!({ "token": link })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("expired or was already used"));

    // Signing out ends the session.
    let (status, _, headers) = call(&app, "POST", "/api/logout", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(headers.get("set-cookie").unwrap().to_str().unwrap().contains("Max-Age=0"));
    let (status, _, _) = call(&app, "GET", "/api/me", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn bad_and_expired_links_are_refused() {
    let app = panel();
    let (status, _, _) = call(&app, "POST", "/api/login", None, Some(json!({ "token": "nonsense" })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "POST", "/api/login", None, Some(json!({ "nope": 1 })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let link = super::super::create_link(ADMIN).unwrap();
    super::super::DB
        .get()
        .unwrap()
        .lock()
        .execute("UPDATE links SET expires = 0 WHERE token_hash = ?1", rusqlite::params![super::super::hash(&link)])
        .unwrap();
    let (status, body, _) = call(&app, "POST", "/api/login", None, Some(json!({ "token": link })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("expired"));
}

#[tokio::test]
async fn a_non_admin_cannot_sign_in_or_use_a_session() {
    let app = panel();
    let link = super::super::create_link(MEMBER).unwrap();
    let (status, _, headers) = call(&app, "POST", "/api/login", None, Some(json!({ "token": link })), true).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(headers.get("set-cookie").is_none());

    let session = session_for(MEMBER);
    for path in ["/api/me", "/api/status", "/api/catalog", "/api/reminders", "/api/audit"] {
        let (status, _, _) = call(&app, "GET", path, Some(&session), None, false).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn login_is_rate_limited_per_address() {
    let app = panel();
    let mut limited = false;
    for _ in 0..12 {
        let req = Request::builder()
            .method("POST")
            .uri("/api/login")
            .header("x-panel", "1")
            .header("x-forwarded-for", "10.0.0.1, 203.0.113.9")
            .body(Body::from(json!({ "token": "wrong" }).to_string()))
            .unwrap();
        if app.clone().oneshot(req).await.unwrap().status() == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
        }
    }
    assert!(limited);
}

#[tokio::test]
async fn everything_needs_a_session() {
    let app = panel();
    for (method, path) in [
        ("GET", "/api/me"),
        ("GET", "/api/status"),
        ("GET", "/api/catalog"),
        ("GET", "/api/discord/channels"),
        ("GET", "/api/discord/members?q=ka"),
        ("GET", "/api/reminders"),
        ("GET", "/api/audit"),
        ("PUT", "/api/settings/PANEL_TEST_ARENA"),
        ("POST", "/api/restart"),
    ] {
        let (status, _, _) = call(&app, method, path, None, Some(json!({ "value": "off" })), true).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        let (status, _, _) = call(&app, method, path, Some("made-up"), Some(json!({ "value": "off" })), true).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path} with a made-up session");
    }
}

#[tokio::test]
async fn changes_need_the_panel_header() {
    let app = panel();
    let session = session_for(ADMIN);
    let (status, _, _) = call(&app, "PUT", "/api/settings/PANEL_TEST_FIGHT_POINTS", Some(&session), Some(json!({ "value": "20" })), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, "POST", "/api/restart", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, "POST", "/api/login", None, Some(json!({ "token": "x" })), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // Reading doesn't.
    let (status, _, _) = call(&app, "GET", "/api/catalog", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
}

// --- settings -------------------------------------------------------------------------

#[tokio::test]
async fn secrets_are_never_readable_or_writable() {
    let app = panel();
    let session = session_for(ADMIN);
    for key in ["DISCORD_TOKEN", "VIZIER_DISCORD_ADMIN_IDS", "OPENAI_API_KEY", "PATH", "HOME"] {
        let (status, _, _) = call(&app, "PUT", &format!("/api/settings/{key}"), Some(&session), Some(json!({ "value": "x" })), true).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{key}");
        assert_eq!(super::super::var(key).as_deref() == Some("x"), false);
    }
    let (_, catalog, _) = call(&app, "GET", "/api/catalog", Some(&session), None, false).await;
    let text = catalog.to_string();
    assert!(!text.contains("DISCORD_TOKEN") && !text.contains("\"HOME\"") && !text.contains("PATH\""));
    let home = std::env::var("HOME").unwrap_or_default();
    assert!(home.is_empty() || !text.contains(&home));
    // Unknown API paths are a JSON 404, not the page.
    let (status, body, _) = call(&app, "GET", "/api/settings/DISCORD_TOKEN", Some(&session), None, false).await;
    assert!(status == StatusCode::NOT_FOUND || status == StatusCode::METHOD_NOT_ALLOWED, "{status} {body}");
}

#[tokio::test]
async fn settings_are_checked_by_kind() {
    let app = panel();
    let session = session_for(ADMIN);
    let put = |key: &'static str, value: Value| {
        let app = app.clone();
        let session = session.clone();
        async move { call(&app, "PUT", &format!("/api/settings/{key}"), Some(&session), Some(json!({ "value": value })), true).await }
    };
    let bad = [
        ("PANEL_TEST_FIGHT_POINTS", json!("501")),
        ("PANEL_TEST_FIGHT_POINTS", json!("ten")),
        ("PANEL_TEST_FIGHT_POINTS", json!("")),
        ("PANEL_TEST_ARENA", json!("maybe")),
        ("PANEL_TEST_FIGHT_STYLE", json!("gif")),
        ("PANEL_TEST_SNITCH_QUIET_FROM", json!("25:00")),
        ("PANEL_TEST_SNITCH_QUIET_FROM", json!("9.30")),
        ("VIZIER_FIGHT_CHANNEL", json!("999")),
        ("VIZIER_FIGHT_CHANNEL", json!("41")),
        ("VIZIER_FIGHT_CHANNEL", json!("general")),
        ("PANEL_TEST_AFK", json!("21")),
        ("PANEL_TEST_MUGGLE_ROLE", json!("12345")),
        ("PANEL_TEST_HOUSE_CAPTAINS", json!("1001,4242")),
        ("PANEL_TEST_SNITCH_CHANNELS", json!("21:3,22:zero")),
        ("PANEL_TEST_SNITCH_CHANNELS", json!("21:0")),
        ("PANEL_TEST_SNITCH_CHANNELS", json!("21:1,21:2")),
        ("PANEL_TEST_CHAT_RATE", json!("5.5")),
        ("PANEL_TEST_CHAT_RATE", json!("NaN")),
        ("PANEL_TEST_ARENA", json!([1])),
    ];
    for (key, value) in bad {
        let (status, body, _) = put(key, value.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{key}={value} gave {body}");
        assert!(body["error"].is_string());
    }

    let good = [
        ("PANEL_TEST_FIGHT_POINTS", json!(" 25 "), "25"),
        ("PANEL_TEST_ARENA", json!("true"), "on"),
        ("PANEL_TEST_ARENA", json!(false), "off"),
        ("PANEL_TEST_SNITCH_QUIET_FROM", json!("7:05"), "07:05"),
        ("VIZIER_FIGHT_CHANNEL", json!("31"), "31"),
        ("PANEL_TEST_AFK", json!("43"), "43"),
        ("PANEL_TEST_MUGGLE_ROLE", json!("58"), "58"),
        ("PANEL_TEST_HOUSE_CAPTAINS", json!("1001, 1004,1001"), "1001,1004"),
        ("PANEL_TEST_SNITCH_CHANNELS", json!("21:3, 22"), "21:3,22:1"),
        ("PANEL_TEST_CHAT_RATE", json!("0.25"), "0.25"),
        ("PANEL_TEST_FIGHT_STYLE", json!("text"), "text"),
        ("PANEL_TEST_QUIZ_FEEDS", json!(""), ""),
    ];
    for (key, value, stored) in good {
        let (status, body, _) = put(key, value.clone()).await;
        assert_eq!(status, StatusCode::OK, "{key}={value} gave {body}");
        assert_eq!(body["origin"], "panel");
        assert_eq!(super::super::VALUES.read().get(key).map(String::as_str), Some(stored), "{key}");
    }

    // null goes back to the environment.
    let (status, body, _) = put("PANEL_TEST_FIGHT_POINTS", Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["origin"], "default");
    assert!(body["value"].is_null());

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=500", Some(&session), None, false).await;
    let entry = audit.as_array().unwrap().iter().find(|e| e["key"] == "PANEL_TEST_FIGHT_POINTS").unwrap();
    assert_eq!(entry["label"], "Points for a win");
    assert_eq!(entry["user_name"], "Kabir");
    assert_eq!(entry["section"]["id"], "arena");
}

#[tokio::test]
async fn status_and_catalog_describe_the_sections() {
    let app = panel();
    let session = session_for(ADMIN_TWO);
    let (status, body, headers) = call(&app, "GET", "/api/status", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("cache-control").unwrap(), "no-store");
    assert_eq!(body["bot"]["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(body["guild"]["name"], "MLCI");
    let sections = body["sections"].as_array().unwrap();
    assert_eq!(sections.len(), 5);
    assert!(sections.iter().find(|s| s["id"] == "houses").unwrap()["enabled"].is_null());
    assert!(sections.iter().find(|s| s["id"] == "quiz").unwrap()["enabled"].is_boolean());

    let (_, catalog, _) = call(&app, "GET", "/api/catalog", Some(&session), None, false).await;
    let first = &catalog["sections"][0]["settings"][0];
    assert_eq!(first["kind"]["type"], "toggle");
    assert!(first.get("env_value").is_some() && first.get("origin").is_some());

    let (_, channels, _) = call(&app, "GET", "/api/discord/channels", Some(&session), None, false).await;
    assert_eq!(channels[0]["name"], "Information");
    let (_, members, _) = call(&app, "GET", "/api/discord/members?q=ME", Some(&session), None, false).await;
    assert_eq!(members[0]["name"], "Meera");
    let (status, _, _) = call(&app, "GET", "/api/discord/members/4242", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn pages_carry_security_headers() {
    let app = panel();
    for path in ["/", "/login", "/assets/app.js", "/assets/app.css"] {
        let (status, _, headers) = call(&app, "GET", path, None, None, false).await;
        assert_eq!(status, StatusCode::OK, "{path}");
        assert!(headers.get("content-security-policy").unwrap().to_str().unwrap().starts_with("default-src 'self'"));
        assert_eq!(headers.get("x-frame-options").unwrap(), "DENY");
        assert_eq!(headers.get("referrer-policy").unwrap(), "no-referrer");
        assert!(headers.get("access-control-allow-origin").is_none());
    }
    let (status, _, _) = call(&app, "GET", "/.env", None, None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- reminders -------------------------------------------------------------------------

fn sample_reminder() -> Value {
    json!({
        "name": "Missing Zoya",
        "enabled": true,
        "channel_id": "21",
        "lines": ["{mention}, it's been {days} days.", "  ", "Come back {name}!"],
        "order": "rotate",
        "schedule": { "kind": "daily", "times": ["21:00", "9:00", "09:00"] },
        "active_from": "",
        "active_to": "",
        "user_id": "1004",
        "user_name": "Zoya",
        "since": "2026-09-01T10:00:00+05:30",
        "stop_when_back": true,
        "welcome_line": "She's back!",
        "ends": "",
        "sent_count": 999
    })
}

#[tokio::test]
async fn reminders_round_trip() {
    let app = panel();
    let session = session_for(ADMIN);
    let (status, created, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(sample_reminder()), true).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_i64().unwrap();
    assert!(id > 0);
    assert_eq!(created["lines"].as_array().unwrap().len(), 2);
    assert_eq!(created["schedule"]["times"], json!(["09:00", "21:00"]));
    assert_eq!(created["sent_count"], 0);

    let (_, list, _) = call(&app, "GET", "/api/reminders", Some(&session), None, false).await;
    assert!(list.as_array().unwrap().iter().any(|r| r["id"] == id));

    let mut edited = created.clone();
    edited["name"] = json!("Missing Zoya (weekly)");
    edited["schedule"] = json!({ "kind": "every", "minutes": 10080 });
    edited["active_from"] = json!("10:00");
    edited["active_to"] = json!("22:00");
    let (status, updated, _) = call(&app, "PUT", &format!("/api/reminders/{id}"), Some(&session), Some(edited), true).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["name"], "Missing Zoya (weekly)");
    assert_eq!(updated["schedule"]["minutes"], 10080);

    let (status, toggled, _) = call(&app, "POST", &format!("/api/reminders/{id}/toggle"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(toggled["enabled"], false);

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let mine: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == format!("reminder:{id}")).collect();
    assert_eq!(mine[0]["change"], "Switched off");
    assert_eq!(mine[0]["label"], "Reminder “Missing Zoya (weekly)”");
    assert!(mine[0]["new"].is_null(), "reminder bodies are summarised, not dumped");

    let (status, _, _) = call(&app, "DELETE", &format!("/api/reminders/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "GET", &format!("/api/reminders/{id}"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "DELETE", &format!("/api/reminders/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reminders_are_validated() {
    let app = panel();
    let session = session_for(ADMIN);
    let cases: Vec<(&str, Value)> = vec![
        ("name", json!("  ")),
        ("channel_id", json!("")),
        ("channel_id", json!("41")),
        ("channel_id", json!("777")),
        ("lines", json!(["", " "])),
        ("lines", json!(["x".repeat(1801)])),
        ("schedule", json!({ "kind": "every", "minutes": 4 })),
        ("schedule", json!({ "kind": "every", "minutes": 10081 })),
        ("schedule", json!({ "kind": "daily", "times": [] })),
        ("schedule", json!({ "kind": "daily", "times": ["24:00"] })),
        ("schedule", json!({ "kind": "weekly" })),
        ("active_from", json!("10:00")),
        ("since", json!("yesterday")),
        ("user_id", json!("")),
    ];
    for (field, value) in cases {
        let mut r = sample_reminder();
        r[field] = value.clone();
        let (status, body, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(r), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={value} gave {body}");
    }
    let (status, _, _) = call(&app, "PUT", "/api/reminders/987654", Some(&session), Some(sample_reminder()), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// --- auto-responses ------------------------------------------------------------------

fn sample_rule() -> Value {
    json!({
        "name": "Good morning",
        "enabled": true,
        "triggers": ["gm", " good morning ", "gm"],
        "match_mode": "whole_word",
        "case_sensitive": false,
        "channels": ["21", "22"],
        "exclude_channels": [],
        "replies": ["gm {user} ☀️", "  "],
        "as_reply": true,
        "reactions": ["☀️", "<:pog:912345678901234000>"],
        "chance": 100,
        "cooldown_secs": 60,
        "hits": 500
    })
}

#[tokio::test]
async fn autoreplies_round_trip() {
    let app = panel();
    let session = session_for(ADMIN);
    let (status, created, _) = call(&app, "POST", "/api/autoreplies", Some(&session), Some(sample_rule()), true).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["triggers"], json!(["gm", "good morning"]));
    assert_eq!(created["replies"], json!(["gm {user} ☀️"]));
    assert_eq!(created["hits"], 0, "counts can't be set from the page");

    let mut edited = created.clone();
    edited["match_mode"] = json!("exact");
    edited["hits"] = json!(99);
    let (status, updated, _) = call(&app, "PUT", &format!("/api/autoreplies/{id}"), Some(&session), Some(edited), true).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["match_mode"], "exact");
    assert_eq!(updated["hits"], 0);

    let (status, toggled, _) = call(&app, "POST", &format!("/api/autoreplies/{id}/toggle"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(toggled["enabled"], false);

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let mine: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == format!("autoreply:{id}")).collect();
    assert_eq!(mine[0]["change"], "Switched off");
    assert_eq!(mine[0]["label"], "Auto-response “Good morning”");
    assert_eq!(mine[0]["section"]["id"], "autoreplies");
    assert!(mine[0]["new"].is_null());

    let (status, _, _) = call(&app, "DELETE", &format!("/api/autoreplies/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "GET", &format!("/api/autoreplies/{id}"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "POST", "/api/autoreplies", None, Some(sample_rule()), true).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&app, "POST", "/api/autoreplies", Some(&session), Some(sample_rule()), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn autoreplies_are_validated() {
    let app = panel();
    let session = session_for(ADMIN);
    let cases: Vec<(&str, Value, &str)> = vec![
        ("name", json!(" "), "name"),
        ("triggers", json!(["", "  "]), "trigger"),
        ("replies", json!([]), ""),
        ("channels", json!(["41"]), "text channel"),
        ("channels", json!(["999"]), "no channel"),
        ("exclude_channels", json!(["21"]), "both"),
        ("match_mode", json!("pattern"), ""),
        ("reactions", json!(["fire"]), "emoji"),
        ("chance", json!(0), "Chance"),
        ("cooldown_secs", json!(700000), "7 days"),
        ("replies", json!(["x".repeat(1801)]), "1800"),
    ];
    for (field, value, needle) in cases {
        let mut r = sample_rule();
        r[field] = value.clone();
        if field == "replies" && value == json!([]) {
            r["reactions"] = json!([]);
        }
        if field == "match_mode" {
            r["triggers"] = json!(["(unclosed"]);
        }
        let (status, body, _) = call(&app, "POST", "/api/autoreplies", Some(&session), Some(r), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={value} gave {body}");
        assert!(body["error"].as_str().unwrap().contains(needle), "{field}: {body}");
    }
}

#[tokio::test]
async fn the_rule_tester_explains_itself() {
    let app = panel();
    let session = session_for(ADMIN);
    let try_it = |text: &'static str, channel: Option<&'static str>, rule: Value| {
        let app = app.clone();
        let session = session.clone();
        async move {
            let body = json!({ "rule": rule, "text": text, "channel_id": channel });
            call(&app, "POST", "/api/autoreplies/test", Some(&session), Some(body), true).await
        }
    };
    let (status, r, _) = try_it("GM guys", None, sample_rule()).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["fires"], true);
    assert_eq!(r["trigger"], "gm");
    let (_, r, _) = try_it("programming", None, sample_rule()).await;
    assert_eq!(r["fires"], false);
    let (_, r, _) = try_it("gm", Some("23"), sample_rule()).await;
    assert_eq!(r["fires"], false);
    assert!(r["why"].as_str().unwrap().contains("channel"));
    let mut off = sample_rule();
    off["enabled"] = json!(false);
    let (_, r, _) = try_it("gm", Some("21"), off).await;
    assert_eq!((r["fires"].clone(), r["matches"].clone()), (json!(false), json!(true)));
    let mut bad = sample_rule();
    bad["match_mode"] = json!("pattern");
    bad["triggers"] = json!(["("]);
    let (status, r, _) = try_it("gm", None, bad).await;
    assert_eq!(status, StatusCode::OK);
    assert!(r["why"].as_str().unwrap().contains("pattern"));
    let (_, emojis, _) = call(&app, "GET", "/api/discord/emojis", Some(&session), None, false).await;
    assert_eq!(emojis[0]["name"], "pog");
}

// --- house cup ----------------------------------------------------------------------------

#[tokio::test]
async fn house_cup_names_people_and_hides_weekly_reasons() {
    let app = panel();
    let session = session_for(ADMIN);
    let (status, cup, _) = call(&app, "GET", "/api/houses?period=month", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{cup}");
    let houses = cup["houses"].as_array().unwrap();
    assert_eq!(houses.len(), 4);
    let leader = houses.iter().find(|h| h["rank"] == 1).unwrap();
    assert_eq!(leader["gap"], 0);
    for h in houses {
        assert!(h["top"].as_array().unwrap().len() <= 10);
        assert!(h["top"].as_array().unwrap().iter().all(|t| t["id"] != "2043"), "Muggles aren't top scorers");
        assert!(h["captain"]["name"].is_string());
        assert!(h["colour"].as_str().unwrap().starts_with('#'));
    }
    let feed = cup["feed"].as_array().unwrap();
    assert_eq!(feed.len(), 50);
    for row in feed.iter().filter(|r| r["source"] == "weekly") {
        assert_eq!(row["reason"], "weekly posts award");
    }
    assert!(!cup.to_string().contains("rough week"), "weekly reasons never leave the server");
    for period in ["today", "week", "last_month", "all"] {
        let (status, _, _) = call(&app, "GET", &format!("/api/houses?period={period}"), Some(&session), None, false).await;
        assert_eq!(status, StatusCode::OK, "{period}");
    }
    let (status, _, _) = call(&app, "GET", "/api/houses?period=forever", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "GET", "/api/houses", None, None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// --- bot behaviour -------------------------------------------------------------------------

#[tokio::test]
async fn agent_settings_are_limited_checked_and_audited() {
    let app = panel();
    let session = session_for(ADMIN_TWO);
    let (status, got, _) = call(&app, "GET", "/api/agent", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    let keys: Vec<&String> = got["settings"].as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 8, "only the safe fields: {keys:?}");
    assert_eq!(got["live"]["system_prompt"], false);

    for bad in [json!({ "provider": "openai" }), json!({ "tools": {} }), json!({ "silent_read_initiative_chance": 2 }), json!([1])] {
        let (status, body, _) = call(&app, "PUT", "/api/agent", Some(&session), Some(bad.clone()), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad} gave {body}");
    }

    let core = got["settings"]["core"].as_str().unwrap().to_string();
    let (status, body, _) = call(
        &app,
        "PUT",
        "/api/agent",
        Some(&session),
        Some(json!({ "core": "# CORE\nsomething else", "base": { "core": "an older version" } })),
        true,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    let patch = json!({ "silent_read_initiative_chance": 0.1, "core": format!("{core}\n- Likes cricket."), "base": { "core": core } });
    let (status, body, _) = call(&app, "PUT", "/api/agent", Some(&session), Some(patch), true).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["restart_needed"], true);
    assert_eq!(body["changed"], json!(["core", "silent_read_initiative_chance"]));

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let chance = audit.as_array().unwrap().iter().find(|e| e["key"] == "agent:silent_read_initiative_chance").unwrap();
    assert_eq!(chance["new"], "10%");
    assert_eq!(chance["section"]["id"], "agent");
    let core_entry = audit.as_array().unwrap().iter().find(|e| e["key"] == "agent:core").unwrap();
    assert!(core_entry["change"].as_str().unwrap().contains("characters"));

    // Saving the same values again changes nothing.
    let (_, body, _) = call(&app, "PUT", "/api/agent", Some(&session), Some(json!({ "silent_read_initiative_chance": 0.1 })), true).await;
    assert_eq!(body["restart_needed"], false);
    let (status, _, _) = call(&app, "PUT", "/api/agent", Some(&session), Some(json!({ "name": "x" })), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

// --- members -------------------------------------------------------------------------

#[tokio::test]
async fn member_notes_round_trip_with_preview_and_audit() {
    let app = panel();
    let session = session_for(ADMIN);
    const ZOYA: &str = "/api/members/1004";
    let (status, _, _) = call(&app, "PUT", &format!("{ZOYA}/note"), Some(&session), Some(json!({ "tone": "roast", "notes": "RCB fan" })), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "changes need the panel header");
    let (status, _, _) = call(&app, "PUT", &format!("{ZOYA}/note"), None, Some(json!({ "tone": "roast" })), true).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    for (body, want) in [
        (json!({ "tone": "savage", "notes": "x" }), StatusCode::BAD_REQUEST),
        (json!({ "tone": "normal", "notes": "   " }), StatusCode::BAD_REQUEST),
        (json!({ "notes": "x".repeat(1001) }), StatusCode::BAD_REQUEST),
    ] {
        let (status, got, _) = call(&app, "PUT", &format!("{ZOYA}/note"), Some(&session), Some(body.clone()), true).await;
        assert_eq!(status, want, "{body} gave {got}");
    }
    let (status, _, _) = call(&app, "PUT", "/api/members/4242/note", Some(&session), Some(json!({ "notes": "who?" })), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, draft, _) = call(&app, "POST", &format!("{ZOYA}/note/preview"), Some(&session), Some(json!({ "tone": "light_roast", "notes": "Disappears in exam season." })), true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(draft["text"].as_str().unwrap().contains("- @Zoya (light friendly teasing"));
    let (_, empty, _) = call(&app, "GET", &format!("{ZOYA}/note/preview"), Some(&session), None, false).await;
    assert!(empty["text"].is_null(), "nothing saved yet");

    let (status, saved, _) = call(&app, "PUT", &format!("{ZOYA}/note"), Some(&session), Some(json!({ "tone": "roast", "notes": "RCB fan.\r\nTakes roasts well.", "use_in_replies": true })), true).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["note"]["name"], "Zoya");
    assert_eq!(saved["note"]["updated_by"], ADMIN.to_string());
    let (_, preview, _) = call(&app, "GET", &format!("{ZOYA}/note/preview"), Some(&session), None, false).await;
    let text = preview["text"].as_str().unwrap();
    assert!(text.contains("they enjoy being roasted") && text.contains("RCB fan. Takes roasts well.") && text.contains("never quote, reveal"));

    let (_, found, _) = call(&app, "GET", "/api/members?q=zoy", Some(&session), None, false).await;
    assert_eq!(found[0]["has_note"], true);
    let (_, listed, _) = call(&app, "GET", "/api/members/notes", Some(&session), None, false).await;
    assert!(listed.as_array().unwrap().iter().any(|n| n["user_id"] == "1004" && n["tone"] == "roast"));

    call(&app, "PUT", &format!("{ZOYA}/note"), Some(&session), Some(json!({ "tone": "roast", "notes": "RCB fan.\nTakes roasts well.", "use_in_replies": false })), true).await;
    let (_, preview, _) = call(&app, "GET", &format!("{ZOYA}/note/preview"), Some(&session), None, false).await;
    assert!(preview["text"].is_null(), "switched off: the AI gets nothing");

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let mine: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == "member:1004").collect();
    assert_eq!(mine[0]["label"], "Notes for @Zoya");
    assert_eq!(mine[0]["change"], "Switched off");
    assert_eq!(mine[0]["section"]["id"], "members");
    assert!(mine[0]["new"].is_null(), "note bodies are summarised, not dumped");

    let (status, _, _) = call(&app, "DELETE", &format!("{ZOYA}/note"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "DELETE", &format!("{ZOYA}/note"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn member_profiles_bring_everything_together() {
    let app = panel();
    let session = session_for(ADMIN);
    let (status, p, _) = call(&app, "GET", "/api/members/2012", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert_eq!(p["name"], "Sameer");
    assert_eq!(p["house"]["key"], "gryffindor");
    assert!(p["roles"].as_array().unwrap().iter().any(|r| r["name"] == "Gryffindor" && r["color"] == "#9b1b1b"));
    assert_eq!(p["activity"]["hours"].as_array().unwrap().len(), 24);
    assert_eq!(p["activity"]["top_channels"][0]["name"], "general");
    let chips = p["points"]["activities"].as_array().unwrap();
    assert_eq!(chips[0]["key"], "chat");
    let quiz = chips.iter().find(|c| c["key"] == "quiz").unwrap();
    assert_eq!(quiz["cap"], 6, "caps come from the live settings");
    assert!(p["tones"].as_array().unwrap().len() == 6);
    assert_eq!(p["joins"]["joins"], 3);

    let (_, seen, _) = call(&app, "GET", "/api/members/2012/seen", Some(&session), None, false).await;
    let first = &seen["messages"][0];
    assert_eq!(first["channel"], "general");
    assert!(first["kind"] == "chat" || first["kind"] == "silent_read");

    let (_, mem, _) = call(&app, "GET", "/api/members/2012/memories", Some(&session), None, false).await;
    let titles: Vec<&str> = mem["memories"].as_array().unwrap().iter().map(|m| m["title"].as_str().unwrap()).collect();
    assert!(titles.contains(&"Sameer") && titles.contains(&"Running jokes") && titles.contains(&"Quiz night"));
    assert!(!titles.contains(&"Snitch rules"));
    assert!(mem["memories"][0]["snippet"].as_str().unwrap().to_lowercase().contains("sameer"));

    let (status, _, _) = call(&app, "GET", "/api/members/424242", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "GET", "/api/members/abc", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "GET", "/api/members/2012", Some(&session_for(MEMBER)), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn scorers_show_caps_and_leave_out_muggles() {
    let app = panel();
    let session = session_for(ADMIN);
    let (status, today, _) = call(&app, "GET", "/api/houses/scorers?day=today", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{today}");
    let rows = today["rows"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= 300);
    assert!(rows.windows(2).all(|w| w[0]["total"].as_i64() >= w[1]["total"].as_i64()), "biggest total first");
    assert!(rows.iter().all(|r| r["id"] != "2043"), "Muggles are left out");
    for r in rows {
        for chip in r["activities"].as_array().unwrap() {
            if chip["kind"] == "capped" {
                if let (Some(pts), Some(cap)) = (chip["points"].as_i64(), chip["cap"].as_i64()) {
                    assert_eq!(chip["reached"].as_bool().unwrap(), pts >= cap);
                }
            }
        }
    }
    assert_eq!(today["chat_bar"], 20);
    let (_, g, _) = call(&app, "GET", "/api/houses/scorers?day=yesterday&house=gryffindor", Some(&session), None, false).await;
    assert!(g["rows"].as_array().unwrap().iter().all(|r| r["house"] == "gryffindor"));
    assert_eq!(g["which"], "yesterday");
    let (status, _, _) = call(&app, "GET", "/api/houses/scorers?day=tomorrow", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "GET", "/api/houses/scorers?house=durmstrang", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// --- member analyses -------------------------------------------------------------------

async fn wait_for_job(app: &Router, session: &str) -> Value {
    for _ in 0..400 {
        let (_, job, _) = call(app, "GET", "/api/profiles/job", Some(session), None, false).await;
        if job["job"]["running"] == false {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the job never finished");
}

/// The analysis job and the fake model are shared: tests using them take turns.
static JOB_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn member_analyses_end_to_end() {
    use std::sync::atomic::Ordering;
    let _turn = JOB_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    let app = panel();
    let session = session_for(ADMIN);
    const SAMEER: &str = "2012";

    // Most active: shares per category, overall is their mean.
    let (status, active, _) = call(&app, "GET", "/api/profiles/active?by=overall&limit=100", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{active}");
    assert_eq!(active["window_days"], 30);
    let rows = active["rows"].as_array().unwrap();
    assert!(rows.windows(2).all(|w| w[0]["score"].as_f64() >= w[1]["score"].as_f64()));
    let sum: f64 = rows.iter().map(|r| r["shares"]["chat"].as_f64().unwrap()).sum();
    assert!((sum - 1.0).abs() < 1e-6, "chat shares add up to the whole server");
    let r0 = &rows[0];
    let mean = (r0["shares"]["chat"].as_f64().unwrap() + r0["shares"]["voice"].as_f64().unwrap() + r0["shares"]["games"].as_f64().unwrap()) / 3.0;
    assert!((r0["score"].as_f64().unwrap() - mean).abs() < 1e-9);
    assert!(rows.iter().any(|r| r["muggle"] == true), "Muggles are listed and marked");
    let (_, by_voice, _) = call(&app, "GET", "/api/profiles/active?by=voice", Some(&session), None, false).await;
    let v = by_voice["rows"].as_array().unwrap();
    assert!(v.windows(2).all(|w| w[0]["voice_min"].as_i64() >= w[1]["voice_min"].as_i64()));
    let (status, _, _) = call(&app, "GET", "/api/profiles/active?by=karma", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // What would be sent: no safe-corner, thread, DM, unknown channel or quoted words.
    let (_, prompt, _) = call(&app, "GET", &format!("/api/profiles/{SAMEER}/prompt"), Some(&session), None, false).await;
    let text = prompt["prompt"].as_str().unwrap();
    assert!(!text.contains("SECRET"), "sensitive text reached the prompt");
    assert!(text.contains("NEVER infer") && text.contains("lol same") && text.contains("@Dev your fantasy team") && text.contains("[link]"));
    assert!(!text.contains("safe-corner"));
    assert_eq!(prompt["messages"], 49);

    // Analyse one member.
    MODEL_MODE.store(0, Ordering::SeqCst);
    let (status, started, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": [SAMEER], "force": true })), true).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    let job = wait_for_job(&app, &session).await;
    assert_eq!(job["job"]["items"][0]["status"], "done", "{job}");
    assert!(PROMPTS.lock().iter().all(|p| !p.contains("SECRET")));
    let (_, got, _) = call(&app, "GET", &format!("/api/profiles/{SAMEER}"), Some(&session), None, false).await;
    let p = &got["profile"];
    assert_eq!(p["status"], "draft");
    let summary = p["fields"]["summary"]["value"].as_str().unwrap();
    assert!(!summary.contains("<@") && !summary.contains("https"), "{summary}");
    assert_eq!(p["fields"]["suggested_tone"]["value"], "light_roast");
    assert_eq!(p["messages_analysed"], 49);

    // Skipped when fresh, unless forced.
    call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": [SAMEER] })), true).await;
    let job = wait_for_job(&app, &session).await;
    assert_eq!(job["job"]["items"][0]["status"], "skipped");

    // An unreadable answer is asked again once; twice fails.
    MODEL_MODE.store(1, Ordering::SeqCst);
    let before = MODEL_CALLS.load(Ordering::SeqCst);
    call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": ["2010"], "force": true })), true).await;
    let job = wait_for_job(&app, &session).await;
    assert_eq!(job["job"]["items"][0]["status"], "done");
    assert_eq!(MODEL_CALLS.load(Ordering::SeqCst) - before, 2);
    MODEL_MODE.store(2, Ordering::SeqCst);
    call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": ["2011"], "force": true })), true).await;
    let job = wait_for_job(&app, &session).await;
    assert_eq!(job["job"]["items"][0]["status"], "failed");
    MODEL_MODE.store(0, Ordering::SeqCst);

    // One job at a time, and it can be cancelled.
    MODEL_DELAY_MS.store(150, Ordering::SeqCst);
    let (status, _, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "top": 5, "force": true })), true).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "top": 2 })), true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = call(&app, "DELETE", "/api/profiles/job", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    let job = wait_for_job(&app, &session).await;
    MODEL_DELAY_MS.store(0, Ordering::SeqCst);
    assert!(job["job"]["items"].as_array().unwrap().iter().any(|i| i["status"] == "cancelled"), "{job}");
    let (status, _, _) = call(&app, "DELETE", "/api/profiles/job", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for bad in [json!({ "top": 51 }), json!({}), json!({ "user_ids": ["abc"] })] {
        let (status, _, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(bad), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Editing keeps the AI's original.
    let edit = json!({ "edits": { "summary": "Evening regular: cricket, quiz nights and Koto.", "avoid": ["exam results", "his job"] } });
    let (status, edited, _) = call(&app, "PUT", &format!("/api/profiles/{SAMEER}"), Some(&session), Some(edit), true).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    let f = &edited["profile"]["fields"]["summary"];
    assert_eq!(f["edited"], true);
    assert!(f["original"].as_str().unwrap().starts_with("Shows up most evenings"));
    for bad in [json!({ "edits": { "suggested_tone": "savage" } }), json!({ "edits": { "interests": vec!["x"; 7] } }), json!({ "edits": { "provider": "x" } })] {
        let (status, _, _) = call(&app, "PUT", &format!("/api/profiles/{SAMEER}"), Some(&session), Some(bad.clone()), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (_, reviewed, _) = call(&app, "PUT", &format!("/api/profiles/{SAMEER}"), Some(&session), Some(json!({ "status": "reviewed" })), true).await;
    assert_eq!(reviewed["profile"]["status"], "reviewed");

    // Adding to notes: preview, then write; refused when too long.
    super::super::members::delete(2012, ADMIN).ok();
    let pick = json!({ "fields": ["summary", "avoid"], "tone": true });
    let (_, preview, _) = call(&app, "POST", &format!("/api/profiles/{SAMEER}/apply/preview"), Some(&session), Some(pick.clone()), true).await;
    assert_eq!(preview["fits"], true);
    assert!(preview["note"].as_str().unwrap().starts_with("From the analysis:\nSummary: Evening regular"));
    assert!(preview["context_block"].as_str().unwrap().contains("light friendly teasing"));
    let (status, applied, _) = call(&app, "POST", &format!("/api/profiles/{SAMEER}/apply"), Some(&session), Some(pick.clone()), true).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let note = super::super::members::get(2012).unwrap();
    assert_eq!(note.tone, super::super::members::Tone::LightRoast);
    assert!(note.notes.contains("Avoid: exam results; his job"));
    let long = super::super::members::MemberNote { notes: "x".repeat(950), ..note.clone() };
    super::super::members::save(&long, ADMIN).unwrap();
    let (status, err, _) = call(&app, "POST", &format!("/api/profiles/{SAMEER}/apply"), Some(&session), Some(pick), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(err["error"].as_str().unwrap().contains("characters over"));
    let (status, _, _) = call(&app, "POST", &format!("/api/profiles/{SAMEER}/apply"), Some(&session), Some(json!({ "fields": ["provider"] })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The log names actions, not text.
    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let changes: Vec<String> = audit.as_array().unwrap().iter().filter(|e| e["key"] == "profile:2012").map(|e| e["change"].as_str().unwrap().to_string()).collect();
    for want in ["Generated", "Edited: Summary, Avoid", "Marked reviewed", "Added to notes: Summary, Avoid, Suggested tone"] {
        assert!(changes.iter().any(|c| c == want), "{want} missing from {changes:?}");
    }
    assert!(!audit.to_string().contains("Evening regular"), "no analysis text in the log");

    // Delete; auth and header like everything else.
    let (status, _, _) = call(&app, "DELETE", &format!("/api/profiles/{SAMEER}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    let (_, gone, _) = call(&app, "GET", &format!("/api/profiles/{SAMEER}"), Some(&session), None, false).await;
    assert!(gone["profile"].is_null());
    let (status, _, _) = call(&app, "GET", "/api/profiles/active", None, None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "top": 1 })), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, "GET", "/api/profiles/active", Some(&session_for(MEMBER)), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activity_tiers_and_auto_filled_notes() {
    use super::super::members::{self as notes, MemberNote, NoteSource, Tone};
    use super::super::profiles::{self, Tier};
    use std::sync::atomic::Ordering;
    let _turn = JOB_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    MODEL_MODE.store(0, Ordering::SeqCst);
    MODEL_DELAY_MS.store(0, Ordering::SeqCst);
    let app = panel();
    let session = session_for(ADMIN);
    for (key, value) in [
        ("VIZIER_ACTIVE_VERY_MESSAGES", "750"),
        ("VIZIER_ACTIVE_VERY_VOICE_MINUTES", "950"),
        ("VIZIER_ACTIVE_VERY_POINTS", "5000"),
        ("VIZIER_ACTIVE_FAIR_MESSAGES", "400"),
        ("VIZIER_ACTIVE_FAIR_VOICE_MINUTES", "700"),
        ("VIZIER_ACTIVE_FAIR_POINTS", "5000"),
    ] {
        super::super::set(key, Some(value), ADMIN).unwrap();
    }
    let t = profiles::thresholds();
    assert_eq!((t.very_messages, t.fair_voice_minutes), (750, 700), "tiers read the panel's settings");

    // Tiers in the ranking agree with the rule, and the filter keeps one tier.
    let rows = FakeData.activity(chrono::Utc::now().timestamp()).await;
    let expect = |tier: Tier| rows.iter().filter(|a| (a.messages > 0 || a.voice_secs > 0 || a.points > 0) && profiles::tier(a, &t) == tier).count();
    let (_, active, _) = call(&app, "GET", "/api/profiles/active?limit=100", Some(&session), None, false).await;
    assert_eq!(active["tiers"]["very"], expect(Tier::Very));
    assert_eq!(active["tiers"]["fair"], expect(Tier::Fair));
    assert_eq!(active["tiers"]["less"], expect(Tier::Less));
    assert_eq!(active["thresholds"]["very_messages"], 750);
    let (_, very, _) = call(&app, "GET", "/api/profiles/active?tier=very&limit=100", Some(&session), None, false).await;
    let very_ids: Vec<String> = very["rows"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap().to_string()).collect();
    assert_eq!(very_ids.len(), expect(Tier::Very));
    assert!(very["rows"].as_array().unwrap().iter().all(|r| r["tier"] == "very"));
    let (status, _, _) = call(&app, "GET", "/api/profiles/active?tier=super", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "tier": "everyone" })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A mod-written note on one very active member; nothing on the others.
    let (mod_id, auto_id, other_id) = (very_ids[0].parse::<u64>().unwrap(), very_ids[1].parse::<u64>().unwrap(), very_ids[2].parse::<u64>().unwrap());
    for id in [mod_id, auto_id, other_id] {
        let _ = notes::delete(id, ADMIN);
        let _ = profiles::delete(id, ADMIN);
    }
    let written = MemberNote { notes: "Written by a mod.".into(), tone: Tone::Gentle, ..MemberNote::blank(mod_id, "Mod Pick") };
    notes::save(&written, ADMIN).unwrap();

    let (status, started, _) = call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "tier": "very", "force": true, "fill_notes": true })), true).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{started}");
    assert_eq!(started["job"]["total"], very_ids.len());
    let job = wait_for_job(&app, &session).await;
    let item = |id: u64| job["job"]["items"].as_array().unwrap().iter().find(|i| i["user_id"] == id.to_string()).unwrap().clone();
    assert_eq!(item(mod_id)["note"], "kept", "a mod's note is never overwritten");
    assert_eq!(notes::get(mod_id).unwrap().notes, "Written by a mod.");
    assert_eq!(item(auto_id)["note"], "filled");
    let filled = notes::get(auto_id).unwrap();
    assert!(!filled.use_in_replies, "off until a mod reviews it");
    assert_eq!((filled.tone, filled.source, filled.reviewed, filled.updated_by.as_str()), (Tone::LightRoast, NoteSource::Analysis, false, "0"));
    assert!(filled.notes.chars().count() <= notes::MAX_NOTE_CHARS && filled.notes.contains("Interests: ") && !filled.notes.contains("<@"));
    assert!(notes::context_block(&[(auto_id, "x".into())]).is_none(), "the bot gets nothing from an unreviewed note");
    assert!(job["job"]["notes_filled"].as_u64().unwrap() >= 2);
    assert_eq!(job["job"]["notes_kept"], 1);

    // Re-analysing refills an untouched auto note; once a mod saves it, it stays theirs.
    call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": [auto_id.to_string()], "force": true, "fill_notes": true })), true).await;
    let job = wait_for_job(&app, &session).await;
    assert_eq!(job["job"]["items"][0]["note"], "refilled");
    let (status, saved, _) = call(&app, "PUT", &format!("/api/members/{auto_id}/note"), Some(&session), Some(json!({ "tone": "light_roast", "notes": "Edited by a mod.", "use_in_replies": false })), true).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!((saved["note"]["source"].as_str(), saved["note"]["reviewed"].as_bool()), (Some("edited"), Some(true)));
    call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": [auto_id.to_string()], "force": true, "fill_notes": true })), true).await;
    let job = wait_for_job(&app, &session).await;
    assert_eq!(job["job"]["items"][0]["note"], "kept");
    assert_eq!(notes::get(auto_id).unwrap().notes, "Edited by a mod.");
    // Skipped as fresh but a missing note is still filled from the saved analysis.
    notes::delete(other_id, ADMIN).unwrap();
    call(&app, "POST", "/api/profiles/analyse", Some(&session), Some(json!({ "user_ids": [other_id.to_string()], "fill_notes": true })), true).await;
    let job = wait_for_job(&app, &session).await;
    assert_eq!((job["job"]["items"][0]["status"].as_str(), job["job"]["items"][0]["note"].as_str()), (Some("skipped"), Some("filled")));

    // Review: to-review count, then switch on (one missing id is reported).
    let (_, status_before, _) = call(&app, "GET", "/api/status", Some(&session), None, false).await;
    let before = status_before["notes_to_review"].as_u64().unwrap();
    assert!(before >= 1);
    let (_, listed, _) = call(&app, "GET", "/api/members/notes", Some(&session), None, false).await;
    assert!(listed.as_array().unwrap().iter().any(|n| n["user_id"] == other_id.to_string() && n["awaits_review"] == true));
    let (status, enabled, _) = call(&app, "POST", "/api/members/notes/enable", Some(&session), Some(json!({ "user_ids": [other_id.to_string(), "4242"] })), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(enabled["enabled"], json!([other_id.to_string()]));
    assert_eq!(enabled["missing"], json!(["4242"]));
    let on = notes::get(other_id).unwrap();
    assert!(on.use_in_replies && on.reviewed && notes::context_block(&[(other_id, "x".into())]).is_some());
    let (_, status_after, _) = call(&app, "GET", "/api/status", Some(&session), None, false).await;
    assert_eq!(status_after["notes_to_review"].as_u64().unwrap(), before - 1);
    let (status, _, _) = call(&app, "POST", "/api/members/notes/enable", Some(&session), Some(json!({ "user_ids": [] })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The log shows the auto-fill as its own actor.
    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let mine: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == format!("member:{other_id}")).collect();
    assert!(mine.iter().any(|e| e["user_name"] == "Auto-fill (analysis)" && e["change"] == "Filled in from the analysis (off until reviewed)"));
    assert!(mine.iter().any(|e| e["change"] == "Reviewed and switched on" && e["user_name"] == "Kabir"));

    for key in ["VIZIER_ACTIVE_VERY_MESSAGES", "VIZIER_ACTIVE_VERY_VOICE_MINUTES", "VIZIER_ACTIVE_VERY_POINTS", "VIZIER_ACTIVE_FAIR_MESSAGES", "VIZIER_ACTIVE_FAIR_VOICE_MINUTES", "VIZIER_ACTIVE_FAIR_POINTS"] {
        super::super::set(key, None, ADMIN).unwrap();
    }
}

#[test]
fn tier_jobs_cap_at_three_hundred() {
    use super::super::profiles::Activity;
    let rows: Vec<Activity> =
        (0..400).map(|i| Activity { user_id: 10_000 + i, messages: 100_000, voice_secs: 0, points: 0, house: None, muggle: false }).collect();
    let picked = super::profiles::tier_members(rows, false, &|id| id == 10_001);
    assert_eq!(picked.len(), 300);
    assert!(!picked.contains(&10_001), "bots are left out");
}

#[test]
fn the_notes_preview_has_its_placeholders() {
    for text in ["Nothing yet — add notes or pick a tone, and this is what the bot will get.", "Switched off: the bot gets nothing for them right now."] {
        assert!(APP_JS.contains(text), "{text}");
    }
}

// --- insights ------------------------------------------------------------------------------

fn talk(ts: i64, from: u64, to: u64, channel: u64, id: u64, kind: super::super::insights::Kind) -> super::super::insights::Interaction {
    super::super::insights::Interaction { ts, channel_id: channel, from_user: from, to_user: to, message_id: id, replied_message_id: None, kind, source: "live" }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn insights_api_counts_pairs_and_shares_no_text() {
    use super::super::insights::{self, Kind};
    let app = panel();
    let session = session_for(ADMIN);
    let now = chrono::Utc::now().timestamp();
    let mut rows = Vec::new();
    // A four-reply back-and-forth between 3001 and 3002, then one more each way later.
    for (i, (from, to)) in [(3001, 3002), (3002, 3001), (3001, 3002), (3002, 3001)].iter().enumerate() {
        rows.push(talk(now - 7200 + i as i64 * 120, *from, *to, 21, 800_000 + i as u64, Kind::Reply));
    }
    rows.push(talk(now - 3000, 3001, 3002, 23, 800_010, Kind::Reply));
    rows.push(talk(now - 2000, 3002, 3001, 21, 800_011, Kind::Mention));
    // 3003 keeps replying to 3004, who answers once.
    for i in 0..16 {
        rows.push(talk(now - 50_000 + i * 900 + 3000, 3003, 3004, 22, 810_000 + i as u64, Kind::Reply));
    }
    rows.push(talk(now - 1000, 3004, 3003, 22, 810_100, Kind::Reply));
    insights::insert(&rows).unwrap();
    assert_eq!(insights::insert(&rows).unwrap(), 0, "the same message and target are recorded once");

    let (status, got, _) = call(&app, "GET", "/api/insights?period=7d", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{got}");
    let duo = got["duos"].as_array().unwrap().iter().find(|d| d["a"]["id"] == "3001").expect("the duo is listed");
    assert_eq!((duo["a_to_b"].as_u64(), duo["b_to_a"].as_u64(), duo["longest"]["len"].as_u64()), (Some(3), Some(2), Some(4)));
    let lopsided = got["one_sided"].as_array().unwrap().iter().find(|o| o["from"]["id"] == "3003").expect("one-sided pair");
    assert_eq!((lopsided["replies"].as_u64(), lopsided["back"].as_u64()), (Some(16), Some(1)));
    assert!(got["magnets"].as_array().unwrap().iter().any(|m| m["member"]["id"] == "3004" && m["people"] == 1));
    assert!(got["mentioned"].as_array().unwrap().iter().any(|m| m["member"]["id"] == "3001"));
    assert!(got["arena"].as_array().unwrap().len() >= 1);
    assert!(got["night_owls"].as_array().unwrap().len() <= 10);

    let (status, pair, _) = call(&app, "GET", "/api/insights/pair?a=3002&b=3001&period=all", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{pair}");
    assert_eq!(pair["a"]["id"], "3001", "pairs are ordered by id");
    assert_eq!(pair["recent"].as_array().unwrap().len(), 6);
    for e in pair["recent"].as_array().unwrap() {
        let keys: std::collections::BTreeSet<&str> = e.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["channel", "from", "kind", "to", "ts"].into_iter().collect(), "metadata only, never text");
    }
    assert!(pair["days"].as_array().unwrap().iter().map(|d| d["a_to_b"].as_u64().unwrap()).sum::<u64>() == 3);
    let (status, _, _) = call(&app, "GET", "/api/insights/pair?a=3001&b=3001", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "GET", "/api/insights/pair?a=x&b=3001", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, conn, _) = call(&app, "GET", "/api/members/3001/connections?period=30d", Some(&session), None, false).await;
    let first = &conn["partners"][0];
    assert_eq!(first["member"]["id"], "3002");
    assert_eq!((first["to_them"].as_u64(), first["from_them"].as_u64(), first["mentions_from_them"].as_u64()), (Some(3), Some(2), Some(1)));

    for bad in ["/api/insights?period=year", "/api/members/3001/connections?period=forever"] {
        let (status, _, _) = call(&app, "GET", bad, Some(&session), None, false).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _, _) = call(&app, "GET", "/api/insights", None, None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&app, "GET", "/api/insights", Some(&session_for(MEMBER)), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(&app, "POST", "/api/insights/rebuild", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "rebuilding needs the panel header");

    // Filling in from history: resolved replies only, #safe-corner left out.
    let (status, _, _) = call(&app, "POST", "/api/insights/rebuild", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let mut state = Value::Null;
    for _ in 0..200 {
        let (_, s, _) = call(&app, "GET", "/api/insights/rebuild", Some(&session), None, false).await;
        state = s;
        if state["rebuild"]["running"] == false {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(state["rebuild"]["error"], Value::Null, "{state}");
    assert_eq!(state["rebuild"]["found"], 2, "{state}");
    let history: Vec<_> = insights::rows_between(0, now + 200_000).into_iter().filter(|r| r.source == "history").collect();
    assert!(history.iter().any(|r| (r.from_user, r.to_user) == (3102, 3101)));
    assert!(history.iter().all(|r| r.channel_id != SAFE));
    assert!(insights::meta_get("backfill_done").is_some());
}

// --- members' reminders ----------------------------------------------------------------

#[tokio::test]
async fn member_reminders_list_create_cancel_and_audit() {
    use super::super::memos;
    let app = panel();
    let session = session_for(ADMIN);
    let make = |body: Value| {
        let app = app.clone();
        let session = session.clone();
        async move { call(&app, "POST", "/api/memos", Some(&session), Some(body), true).await }
    };

    // Problems read as something to fix.
    let good = json!({ "user_id": MEMBER.to_string(), "channel_id": "22", "when": "in 2 hours", "text": "call mum MEMO-SECRET" });
    for (field, value, says) in [
        ("when", json!("whenever"), "couldn't read “whenever”"),
        ("when", json!(""), "Say when"),
        ("when", json!("in 90 days"), "at most 60 days"),
        ("text", json!("  "), "What should the reminder say"),
        ("user_id", json!("4242"), "isn't in the server"),
        ("user_id", json!(""), "Pick the member"),
        ("channel_id", json!("41"), "not a text channel"),
        ("channel_id", json!(""), "Pick the channel"),
    ] {
        let mut body = good.clone();
        body[field] = value.clone();
        let (status, res, _) = make(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={value} gave {res}");
        assert!(res["error"].as_str().unwrap().contains(says), "{field}={value}: {res}");
    }
    let (status, _, _) = call(&app, "POST", "/api/memos", Some(&session), Some(good.clone()), false).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "needs the panel header");

    // An admin sets one for a member, and one for themself.
    let (status, created, _) = make(good.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["user"]["name"], "Rohan");
    assert_eq!(created["set_by"]["name"], "Kabir");
    assert_eq!((created["via"].as_str(), created["status"].as_str()), (Some("panel"), Some("pending")));
    assert_eq!(created["channel"]["name"], "memes");
    assert!(created["due_words"].as_str().unwrap().ends_with("IST"));
    let stored = memos::get(id).unwrap();
    assert_eq!((stored.set_by.as_str(), stored.via.as_str()), ("1001", "panel"));
    let (_, mine, _) = make(json!({ "user_id": ADMIN.to_string(), "channel_id": "21", "when": "tomorrow 9am", "text": "own one" })).await;
    assert!(mine["set_by"].is_null(), "no “set by” on your own: {mine}");

    // Ones members set somewhere private keep their text off the panel.
    let now = chrono::Utc::now().timestamp();
    let safe = memos::create(MEMBER, SAFE, "SAFE-MEMO private", now + 3600, "chat", 0).unwrap();
    let thread = memos::create(MEMBER, 77, "THREAD-MEMO private", now + 3600, "command", 0).unwrap();
    let dm = memos::create(MEMBER, 5_000, "DM-MEMO private", now + 3600, "chat", 0).unwrap();

    let (status, list, _) = call(&app, "GET", "/api/memos?status=pending", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["enabled"], true);
    let items = list["items"].as_array().unwrap();
    assert!(list["pending"].as_u64().unwrap() >= 5);
    let by_id = |id: i64| items.iter().find(|m| m["id"] == id).cloned().unwrap();
    assert_eq!(by_id(id)["text"], "call mum MEMO-SECRET");
    for private in [safe.id, thread.id, dm.id] {
        assert_eq!(by_id(private)["private"], true);
        assert!(by_id(private)["text"].is_null());
    }
    let text = list.to_string();
    assert!(!text.contains("SAFE-MEMO") && !text.contains("THREAD-MEMO") && !text.contains("DM-MEMO"));
    let (status, _, _) = call(&app, "GET", "/api/memos?status=later", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // The live "when" preview.
    let (_, when, _) = call(&app, "GET", "/api/memos/when?text=in%2030m", Some(&session), None, false).await;
    assert_eq!(when["ok"], true);
    assert!((1790..=1810).contains(&when["in_secs"].as_i64().unwrap()));
    assert!(when["words"].as_str().unwrap().contains(" at "));
    let (_, when, _) = call(&app, "GET", "/api/memos/when?text=someday", Some(&session), None, false).await;
    assert_eq!(when["ok"], false);
    assert!(when["error"].as_str().unwrap().contains("someday"));

    // Cancelling: once.
    let (status, cancelled, _) = call(&app, "POST", &format!("/api/memos/{id}/cancel"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!(cancelled["status"], "cancelled");
    let (status, again, _) = call(&app, "POST", &format!("/api/memos/{id}/cancel"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{again}");
    let (status, _, _) = call(&app, "POST", "/api/memos/999999/cancel", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, done, _) = call(&app, "GET", "/api/memos?status=done", Some(&session), None, false).await;
    assert!(done["items"].as_array().unwrap().iter().any(|m| m["id"] == id && m["status"] == "cancelled"));
    assert!(done["items"].as_array().unwrap().iter().all(|m| m["status"] != "pending"));

    // The log says who and when, never what.
    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let entries: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == format!("memo:{id}")).collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["change"], "Cancelled");
    assert!(entries[1]["change"].as_str().unwrap().starts_with("Created · due "));
    assert_eq!(entries[0]["label"], "Member reminder for @Rohan");
    assert_eq!(entries[0]["section"]["id"], "reminders");
    assert!(!audit.to_string().contains("MEMO-SECRET"));
    let raw = super::super::audit(1000);
    assert!(raw.iter().filter(|e| e.key.starts_with("memo:")).all(|e| !format!("{:?}{:?}", e.old, e.new).contains("MEMO-SECRET")));
}

// --- pictures and richer posts ------------------------------------------------------------

fn png(len: usize) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.resize(len, 7);
    b
}

async fn upload(app: &Router, session: &str, content_type: &str, filename: Option<&str>, body: Vec<u8>) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/media")
        .header("cookie", format!("mlci_panel={}", session))
        .header("x-panel", "1")
        .header("content-type", content_type);
    if let Some(name) = filename {
        req = req.header("x-filename", name);
    }
    let res = app.clone().oneshot(req.body(Body::from(body)).unwrap()).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into())))
}

#[tokio::test]
async fn pictures_are_checked_served_and_kept_while_used() {
    let app = panel();
    let session = session_for(ADMIN);

    // Raw bytes over the usual 256 KB body limit, with a name that needs tidying.
    let (status, pic, _) = {
        let (s, v) = upload(&app, &session, "image/png", Some("Good%20Morning%21.png"), png(1_500_000)).await;
        (s, v, ())
    };
    assert_eq!(status, StatusCode::CREATED, "{pic}");
    let id = pic["id"].as_str().unwrap().to_string();
    assert_eq!((pic["name"].as_str(), pic["mime"].as_str(), pic["size"].as_i64()), (Some("Good Morning.png"), Some("image/png"), Some(1_500_000)));

    // Multipart, and the file's real type wins over its name.
    let boundary = "XyZboundary";
    let mut form = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dance.png\"\r\nContent-Type: image/png\r\n\r\n").into_bytes();
    form.extend_from_slice(b"GIF89a-not-really-a-png");
    form.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let (status, gif) = upload(&app, &session, &format!("multipart/form-data; boundary={boundary}"), None, form).await;
    assert_eq!(status, StatusCode::CREATED, "{gif}");
    assert_eq!((gif["name"].as_str(), gif["mime"].as_str()), (Some("dance.gif"), Some("image/gif")));

    // What isn't a picture, or is too big.
    for (kind, name, body, code, says) in [
        ("image/svg+xml", "x.svg", b"<svg xmlns='http://www.w3.org/2000/svg'><script>alert(1)</script></svg>".to_vec(), StatusCode::BAD_REQUEST, "Only PNG"),
        ("image/png", "fake.png", b"just text pretending".to_vec(), StatusCode::BAD_REQUEST, "Only PNG"),
        ("image/jpeg", "empty.jpg", Vec::new(), StatusCode::BAD_REQUEST, "empty"),
        ("image/png", "huge.png", png(8 * 1024 * 1024 + 1), StatusCode::PAYLOAD_TOO_LARGE, "8 MB"),
    ] {
        let (status, res) = upload(&app, &session, kind, Some(name), body).await;
        assert_eq!(status, code, "{name}: {res}");
        assert!(res["error"].as_str().unwrap_or_default().contains(says), "{name}: {res}");
    }
    let (status, _) = upload(&app, &session_for(MEMBER), "image/png", Some("a.png"), png(100)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Listed, and served only to admins, with its real type.
    let (_, list, _) = call(&app, "GET", "/api/media", Some(&session), None, false).await;
    assert!(list["items"].as_array().unwrap().iter().any(|m| m["id"] == id && m["url"] == format!("/api/media/{id}")));
    let (status, body, headers) = call(&app, "GET", &format!("/api/media/{id}"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers.get("content-type").unwrap(), "image/png");
    assert!(headers.get("cache-control").unwrap().to_str().unwrap().starts_with("private"));
    assert_eq!(headers.get("x-content-type-options").unwrap(), "nosniff");
    assert!(body.as_str().map_or(true, |t| t.len() > 1000));
    let (status, _, _) = call(&app, "GET", &format!("/api/media/{id}"), None, None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    for bad in ["../../control.db", "ABCDEF0123456789", "0000000000000000"] {
        let (status, _, _) = call(&app, "GET", &format!("/api/media/{bad}"), Some(&session), None, false).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{bad}");
    }

    // A reminder that uses it keeps it from being deleted, and says which.
    let mut r = sample_reminder();
    r["name"] = json!("Morning picture");
    r["images"] = json!([id, id, gif["id"]]);
    r["stop_when_back"] = json!(false);
    let (status, saved, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(r.clone()), true).await;
    assert_eq!(status, StatusCode::CREATED, "{saved}");
    assert_eq!(saved["images"].as_array().unwrap().len(), 2, "duplicates dropped");
    let rid = saved["id"].as_i64().unwrap();
    let (status, refused, _) = call(&app, "DELETE", &format!("/api/media/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(refused["error"].as_str().unwrap().contains("“Morning picture”"), "{refused}");
    let (_, list, _) = call(&app, "GET", "/api/media", Some(&session), None, false).await;
    let listed = list["items"].as_array().unwrap().iter().find(|m| m["id"] == id).unwrap().clone();
    assert_eq!(listed["used_by"][0]["name"], "Morning picture");

    let mut without = saved.clone();
    without["images"] = json!([gif["id"]]);
    let (status, _, _) = call(&app, "PUT", &format!("/api/reminders/{rid}"), Some(&session), Some(without), true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "DELETE", &format!("/api/media/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = call(&app, "GET", &format!("/api/media/{id}"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A picture that's gone can't be picked.
    r["images"] = json!([id]);
    let (status, res, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(r), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(res["error"].as_str().unwrap().contains("no longer in the library"));

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let changes: Vec<&str> = audit.as_array().unwrap().iter().filter(|e| e["key"] == format!("media:{id}")).map(|e| e["change"].as_str().unwrap()).collect();
    assert_eq!(changes, vec!["Deleted", "Uploaded"]);
    call(&app, "DELETE", &format!("/api/reminders/{rid}"), Some(&session), None, true).await;
}

#[tokio::test]
async fn richer_reminders_are_checked_and_keep_the_schedulers_fields() {
    let app = panel();
    let session = session_for(ADMIN);
    let base = || {
        let mut r = sample_reminder();
        r["stop_when_back"] = json!(false);
        r
    };
    for (field, value, says) in [
        ("colour", json!("red"), "#8b93ff"),
        ("reactions", json!(["fire"]), "isn't an emoji"),
        ("reactions", json!(["😀", "😁", "😂", "🤣", "😃", "😄"]), "at most 5"),
        ("title", json!("t".repeat(257)), "256"),
        ("ai_prompt", json!("p".repeat(1001)), "1000"),
        ("style", json!("poster"), "missing something"),
        ("image_order", json!("sideways"), "missing something"),
    ] {
        let mut r = base();
        r[field] = value.clone();
        let (status, res, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(r), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={value}: {res}");
        assert!(res["error"].as_str().unwrap().contains(says), "{field}={value}: {res}");
    }

    // A card written by the AI needs no lines of its own.
    let mut r = base();
    r["lines"] = json!([]);
    r["style"] = json!("card");
    r["title"] = json!("  ❓ Question of the day  ");
    r["colour"] = json!("#E0A43A");
    r["reactions"] = json!(["👀", " <:pog:912345678901234000> ", "👀"]);
    r["ai_prompt"] = json!("A fun question");
    r["delete_previous"] = json!(true);
    r["last_message_id"] = json!("123");
    r["ai_recent"] = json!(["made up"]);
    let (status, card, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(r), true).await;
    assert_eq!(status, StatusCode::CREATED, "{card}");
    assert_eq!((card["style"].as_str(), card["title"].as_str(), card["colour"].as_str()), (Some("card"), Some("❓ Question of the day"), Some("#e0a43a")));
    assert_eq!(card["reactions"], json!(["👀", "<:pog:912345678901234000>"]));
    assert_eq!((card["last_message_id"].as_str(), card["ai_recent"].as_array().unwrap().len()), (Some(""), 0), "a new one starts clean");
    let id = card["id"].as_i64().unwrap();

    // What the scheduler keeps survives an edit from the page.
    let mut stored = super::super::reminders::get(id).unwrap();
    super::super::posts::record_post(&mut stored, 1_789_381_800, 555, Some("What's your comfort food?"));
    super::super::reminders::save(&stored, 0).unwrap();
    let mut edited = card.clone();
    edited["name"] = json!("QOTD");
    edited["last_message_id"] = json!("999");
    edited["ai_recent"] = json!([]);
    edited["sent_count"] = json!(0);
    let (status, updated, _) = call(&app, "PUT", &format!("/api/reminders/{id}"), Some(&session), Some(edited), true).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!((updated["name"].as_str(), updated["last_message_id"].as_str(), updated["sent_count"].as_i64()), (Some("QOTD"), Some("555"), Some(1)));
    assert_eq!(updated["ai_recent"], json!(["What's your comfort food?"]));

    // Old stored reminders, without any of the new fields, still read.
    {
        let conn = super::super::DB.get().unwrap().lock();
        let old = r#"{"name":"Old style","enabled":true,"channel_id":"21","lines":["hi"],"order":"rotate","schedule":{"kind":"every","minutes":60}}"#;
        conn.execute("INSERT INTO reminders (body, updated_by, updated_ts) VALUES (?1, 0, 0)", rusqlite::params![old]).unwrap();
    }
    let old = super::super::reminders::list().into_iter().find(|r| r.name == "Old style").unwrap();
    assert!(old.images.is_empty() && old.style == super::super::reminders::Style::Plain && !old.delete_previous);
    super::super::reminders::delete(old.id, ADMIN).unwrap();

    // "Send a test now" posts without counting, and not twice in a row.
    let (status, _, _) = call(&app, "POST", &format!("/api/reminders/{id}/test"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(TEST_POSTS.lock().contains(&id));
    let (status, _, _) = call(&app, "POST", &format!("/api/reminders/{id}/test"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let after = super::super::reminders::get(id).unwrap();
    assert_eq!((after.sent_count, after.last_sent, after.last_message_id.as_str()), (1, 1_789_381_800, "555"));
    let (status, _, _) = call(&app, "POST", "/api/reminders/987654/test", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "POST", &format!("/api/reminders/{id}/test"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Moving it to another channel forgets the post it would tidy away.
    let mut moved = updated.clone();
    moved["channel_id"] = json!("23");
    let (_, moved, _) = call(&app, "PUT", &format!("/api/reminders/{id}"), Some(&session), Some(moved), true).await;
    assert_eq!((moved["last_message_id"].as_str(), moved["sent_count"].as_i64()), (Some(""), Some(1)));
    call(&app, "DELETE", &format!("/api/reminders/{id}"), Some(&session), None, true).await;
}

#[tokio::test]
async fn templates_placeholders_and_the_ai_preview() {
    use std::sync::atomic::Ordering;
    let _turn = JOB_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    MODEL_MODE.store(0, Ordering::SeqCst);
    MODEL_DELAY_MS.store(0, Ordering::SeqCst);
    let app = panel();
    let session = session_for(ADMIN_TWO);

    // Every template saves as it is once it has a channel.
    let (status, templates, _) = call(&app, "GET", "/api/reminders/templates", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(templates.as_array().unwrap().len(), 5);
    for t in templates.as_array().unwrap() {
        let mut r = t["reminder"].clone();
        r["channel_id"] = json!("21");
        let (status, saved, _) = call(&app, "POST", "/api/reminders", Some(&session), Some(r), true).await;
        assert_eq!(status, StatusCode::CREATED, "{}: {saved}", t["key"]);
        call(&app, "DELETE", &format!("/api/reminders/{}", saved["id"]), Some(&session), None, true).await;
    }

    let (_, values, _) = call(&app, "GET", "/api/reminders/placeholders", Some(&session), None, false).await;
    assert_eq!(values["standings"].as_str().unwrap().matches(" · ").count(), 3, "{values}");
    assert!(!values["date"].as_str().unwrap().is_empty() && !values["leader"].as_str().unwrap().is_empty());

    // The preview fills the prompt, asks, and cleans the answer.
    let before = PROMPTS.lock().len();
    let (status, preview, _) = call(&app, "POST", "/api/reminders/ai-preview", Some(&session), Some(json!({ "prompt": "Cheer on {leader}: {standings}" })), true).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let text = preview["text"].as_str().unwrap();
    assert!(text.chars().count() <= 400 && !text.contains("<@"), "{text}");
    assert_eq!(preview["model"], "fake/model-1");
    let asked = PROMPTS.lock()[before..].join("\n");
    assert!(asked.contains(values["standings"].as_str().unwrap()) && !asked.contains("{standings}"), "{asked}");
    let (status, res, _) = call(&app, "POST", "/api/reminders/ai-preview", Some(&session), Some(json!({ "prompt": "  " })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{res}");
}

// --- special welcomes ----------------------------------------------------------------------

#[tokio::test]
async fn special_welcomes_round_trip() {
    use super::super::welcomes::{self, Mode};
    let app = panel();
    let session = session_for(ADMIN);
    super::super::set("VIZIER_WELCOME_CHANNEL", Some("11"), ADMIN).unwrap();
    const RIYA: &str = "712345678901234567";

    // Signed in, and changes from the panel's own page only.
    for (method, path) in [
        ("GET", "/api/welcomes"),
        ("POST", "/api/welcomes"),
        ("PUT", "/api/welcomes/1"),
        ("DELETE", "/api/welcomes/1"),
        ("POST", "/api/welcomes/preview"),
        ("POST", "/api/welcomes/1/test"),
        ("GET", "/api/welcomes/lookup?id=459076776266694670"),
    ] {
        let (status, _, _) = call(&app, method, path, None, Some(json!({ "lines": ["x"] })), true).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        if method != "GET" {
            let (status, _, _) = call(&app, method, path, Some(&session), Some(json!({ "lines": ["x"] })), false).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} without the panel header");
        }
    }
    let member_session = session_for(MEMBER);
    let (status, _, _) = call(&app, "GET", "/api/welcomes", Some(&member_session), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Lucky's welcome was seeded, waiting, with who missed him named.
    let (status, list, _) = call(&app, "GET", "/api/welcomes", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["enabled"], true);
    assert_eq!(list["welcome_channel"]["name"], "welcome");
    let lucky = list["items"].as_array().unwrap().iter().find(|w| w["user_id"] == LUCKY.to_string()).cloned().expect("seeded");
    let lucky_id = lucky["id"].as_i64().unwrap();
    assert_eq!((lucky["enabled"].as_bool(), lucky["mode"].as_str(), lucky["replace_normal"].as_bool()), (Some(true), Some("once"), Some(true)));
    assert_eq!((lucky["member"]["name"].as_str(), lucky["member"]["in_server"].as_bool()), (Some("Lucky"), Some(false)));
    assert_eq!(lucky["also"][0]["name"], "Def Not Kohli");
    assert_eq!(lucky["also"][1]["name"], "MahoganyDesk");
    assert_eq!((lucky["channel"]["id"].as_str(), lucky["channel"]["default"].as_bool()), (Some("1516492867642593443"), Some(false)));

    // Problems read as something to fix.
    let good = json!({
        "user_id": RIYA, "user_name": "Riya", "lines": ["{mention} is back after {away}!", "  "],
        "also_ping": ["1004"], "mode": "every", "replace_normal": false, "note": "keeps rejoining",
    });
    let make = |body: Value| {
        let app = app.clone();
        let session = session.clone();
        async move { call(&app, "POST", "/api/welcomes", Some(&session), Some(body), true).await }
    };
    for (field, value, says) in [
        ("user_id", json!("45907677626669467x"), "isn't a Discord ID"),
        ("user_id", json!("4242"), "isn't a Discord ID"),
        ("user_id", json!(""), "Pick the member"),
        ("lines", json!([]), "Write the message"),
        ("lines", json!(vec!["x"; 11]), "at most 10"),
        ("lines", json!(["y".repeat(1501)]), "longer than 1500"),
        ("also_ping", json!((0..11).map(|i| format!("30286175340920{:04}", i)).collect::<Vec<_>>()), "at most 10 other"),
        ("also_ping", json!(["99"]), "in Also ping isn't a Discord ID"),
        ("channel_id", json!("41"), "not a text channel"),
        ("channel_id", json!("777"), "no channel 777"),
        ("mode", json!("sometimes"), "missing something"),
        ("user_name", json!(""), "Type a name for them"),
    ] {
        let mut body = good.clone();
        body[field] = value.clone();
        let (status, res, _) = make(body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}={value} gave {res}");
        assert!(res["error"].as_str().unwrap().contains(says), "{field}={value}: {res}");
    }

    // Made for someone who isn't in the server, going to the welcome channel.
    let mut sneaky = good.clone();
    sneaky["fired_count"] = json!(9);
    sneaky["created_by"] = json!("1");
    let (status, created, _) = make(sneaky).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["id"].as_i64().unwrap();
    assert_eq!((created["member"]["name"].as_str(), created["member"]["known"].as_bool()), (Some("Riya"), Some(false)));
    assert_eq!(created["also"][0]["name"], "Zoya");
    assert_eq!((created["channel"]["id"].as_str(), created["channel"]["default"].as_bool()), (Some("11"), Some(true)));
    let stored = welcomes::get(id).unwrap();
    assert_eq!(stored.lines, vec!["{mention} is back after {away}!".to_string()]);
    assert_eq!((stored.fired_count, stored.created_by.as_str(), stored.mode, stored.enabled), (0, "1001", Mode::Every, true));
    let (status, dup, _) = make(good.clone()).await;
    assert_eq!(status, StatusCode::CONFLICT, "{dup}");
    assert!(dup["error"].as_str().unwrap().contains("already a welcome for Riya"));

    // Someone picked from the member list is named from it.
    let (status, rohan, _) = make(json!({ "user_id": MEMBER.to_string(), "lines": ["hi {name}"], "channel_id": "21" })).await;
    assert_eq!(status, StatusCode::CREATED, "{rohan}");
    assert_eq!((rohan["user_name"].as_str(), rohan["member"]["in_server"].as_bool()), (Some("Rohan"), Some(true)));
    assert_eq!((rohan["ping_member"].as_bool(), rohan["mode"].as_str(), rohan["replace_normal"].as_bool()), (Some(true), Some("once"), Some(true)));

    // Editing keeps what the bot counted.
    let now = chrono::Utc::now().timestamp();
    let fired = welcomes::record_fired(id, now, 555).unwrap();
    assert!(fired.enabled, "every time stays on");
    let mut edit = good.clone();
    edit["lines"] = json!(["Welcome back {name}, {n} time"]);
    edit["enabled"] = json!(false);
    edit["fired_count"] = json!(0);
    let (status, edited, _) = call(&app, "PUT", &format!("/api/welcomes/{id}"), Some(&session), Some(edit.clone()), true).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!((edited["fired_count"].as_i64(), edited["last_fired_message_id"].as_str(), edited["enabled"].as_bool()), (Some(1), Some("555"), Some(false)));
    assert_eq!(welcomes::get(id).unwrap().created_by, "1001");
    let (status, _, _) = call(&app, "PUT", "/api/welcomes/999999", Some(&session), Some(edit.clone()), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "PUT", "/api/welcomes/abc", Some(&session), Some(edit.clone()), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let mut steal = edit.clone();
    steal["user_id"] = json!(LUCKY.to_string());
    let (status, _, _) = call(&app, "PUT", &format!("/api/welcomes/{id}"), Some(&session), Some(steal), true).await;
    assert_eq!(status, StatusCode::CONFLICT, "one welcome per member");

    // The preview fills the words in from the join log and sends nothing.
    let posts_before = UNPINGED_POSTS.lock().len();
    let body = json!({
        "user_id": LUCKY.to_string(), "lines": ["{mention} {n} after {away}! <@302861753409208322> <@1004>", ""],
        "also_ping": ["302861753409208322"],
    });
    let (status, preview, _) = call(&app, "POST", "/api/welcomes/preview", Some(&session), Some(body), true).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let text = preview["items"][0].as_str().unwrap();
    assert!(text.starts_with("<@459076776266694670> 3rd after ") && text.ends_with("! <@302861753409208322> <@1004>"), "{text}");
    assert!(!text.contains("a while"), "{text}");
    assert_eq!(preview["items"][1], "");
    assert_eq!(preview["facts"]["from_log"], true);
    assert_eq!(preview["names"]["302861753409208322"], "Def Not Kohli");
    assert_eq!(preview["names"]["1004"], "Zoya");
    assert_eq!(preview["unpinged"], json!(["1004"]));
    let (_, quiet, _) = call(&app, "POST", "/api/welcomes/preview", Some(&session), Some(json!({ "user_name": "Someone", "lines": ["{mention} {n} {away}"], "ping_member": false })), true).await;
    assert_eq!(quiet["items"][0], "Someone 1st a while");
    assert_eq!(UNPINGED_POSTS.lock().len(), posts_before, "a preview posts nothing");
    assert_eq!(welcomes::get(lucky_id).unwrap().fired_count, 0);
    let (status, _, _) = call(&app, "POST", "/api/welcomes/preview", Some(&session), Some(json!({ "user_id": "abc", "lines": ["x"] })), true).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Looking a pasted ID up.
    let (status, found, _) = call(&app, "GET", &format!("/api/welcomes/lookup?id={LUCKY}"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!((found["name"].as_str(), found["in_server"].as_bool(), found["welcome_id"].as_i64()), (Some("Lucky"), Some(false), Some(lucky_id)));
    let (_, rohan_found, _) = call(&app, "GET", &format!("/api/welcomes/lookup?id={MEMBER}"), Some(&session), None, false).await;
    assert_eq!((rohan_found["name"].as_str(), rohan_found["in_server"].as_bool()), (Some("Rohan"), Some(true)));
    let (status, _, _) = call(&app, "GET", "/api/welcomes/lookup?id=712345678901234568", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for bad in ["12x", "4242", ""] {
        let (status, _, _) = call(&app, "GET", &format!("/api/welcomes/lookup?id={bad}"), Some(&session), None, false).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }

    // A test goes out marked, with no pings, and isn't counted.
    let (status, sent, _) = call(&app, "POST", &format!("/api/welcomes/{id}/test"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK, "{sent}");
    assert_eq!(UNPINGED_POSTS.lock().last().cloned(), Some((11, format!("(test) Welcome back Riya, 1st time"))));
    let (status, _, _) = call(&app, "POST", &format!("/api/welcomes/{id}/test"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(welcomes::get(id).unwrap().fired_count, 1);

    // At most fifty.
    let mut extra = Vec::new();
    while welcomes::list().len() < welcomes::MAX_RULES {
        let (status, res, _) = make(json!({ "user_id": format!("71234567890123{:04}", 5000 + extra.len()), "user_name": "x", "lines": ["hi"] })).await;
        assert_eq!(status, StatusCode::CREATED, "{res}");
        extra.push(res["id"].as_i64().unwrap());
    }
    let (status, full, _) = make(json!({ "user_id": "712345678901239999", "user_name": "x", "lines": ["hi"] })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(full["error"].as_str().unwrap().contains("already 50"), "{full}");
    for e in extra {
        let (status, _, _) = call(&app, "DELETE", &format!("/api/welcomes/{e}"), Some(&session), None, true).await;
        assert_eq!(status, StatusCode::OK);
    }

    // Deleting, and the log.
    let (status, _, _) = call(&app, "DELETE", &format!("/api/welcomes/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK);
    assert!(welcomes::get(id).is_none());
    let (status, _, _) = call(&app, "DELETE", &format!("/api/welcomes/{id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let entries: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == format!("welcome:{id}")).collect();
    let changes: Vec<&str> = entries.iter().map(|e| e["change"].as_str().unwrap()).collect();
    assert_eq!(changes, vec!["Deleted", "Switched off", "Created"]);
    assert_eq!(entries[0]["label"], "Special welcome for @Riya");
    assert_eq!(entries[0]["section"]["id"], "welcomes");
    assert_eq!(entries[0]["user_name"], "Kabir");
}

// --- chocolate frogs -----------------------------------------------------------------------

#[tokio::test]
async fn chocolate_frogs_round_trip() {
    use super::super::super::frog_store as store;
    let app = panel();
    let session = session_for(ADMIN);

    // Signed in, admins only, and changes from the panel's own page only.
    for (method, path) in [
        ("GET", "/api/frogs"),
        ("POST", "/api/frogs/wizards"),
        ("PUT", "/api/frogs/wizards/1"),
        ("GET", "/api/frogs/wizards/1/image"),
        ("GET", "/api/frogs/owners?wizard=1"),
        ("POST", "/api/frogs/riddles/t-001/retire"),
        ("POST", "/api/frogs/drop"),
    ] {
        let (status, _, _) = call(&app, method, path, None, Some(json!({})), true).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
        if method != "GET" {
            let (status, _, _) = call(&app, method, path, Some(&session), Some(json!({ "channel_id": "21" })), false).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path} without the panel header");
        }
    }
    let (status, _, _) = call(&app, "GET", "/api/frogs", Some(&session_for(MEMBER)), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // The overview: twelve wizards, four rarities, the bank.
    let (status, page, _) = call(&app, "GET", "/api/frogs", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["enabled"], false, "off until the owner switches it on");
    assert_eq!(page["wizards"].as_array().unwrap().len(), 10);
    assert_eq!((page["wizards"][1]["name"].as_str(), page["wizards"][1]["slug"].as_str()), (Some("Luna Lovegood"), Some("luna")));
    assert_eq!(page["wizards"][1]["image_url"], Value::Null);
    let rarities = page["rarities"].as_array().unwrap();
    assert_eq!(rarities.iter().map(|r| r["chance"].as_f64().unwrap()).collect::<Vec<_>>(), vec![58.0, 35.0, 7.0]);
    assert_eq!((rarities[2]["emoji"].as_str(), rarities[2]["points"].as_i64(), rarities[2]["colour"].as_str()), (Some("🔥"), Some(10), Some("#e8572a")));
    assert_eq!(page["bank"][0], json!({ "difficulty": "easy", "playable": 3, "unused": 3, "retired": 0 }));
    assert_eq!(page["modal_mode"], "text block");

    // Wizards: checked, made, edited, pictured.
    for (body, says) in [
        (json!({ "name": "Dobby", "rarity": "rare" }), "Pick Common, Uncommon or Legendary"),
        (json!({ "name": "", "rarity": "common" }), "Give the card a name"),
        (json!({ "name": "x".repeat(39), "rarity": "common" }), "at most 38"),
        (json!({ "name": "Merlin", "rarity": "common" }), "already a card"),
        (json!({ "name": "Dobby", "rarity": "common", "image": "0123456789abcdef" }), "isn't in the library"),
    ] {
        let (status, res, _) = call(&app, "POST", "/api/frogs/wizards", Some(&session), Some(body.clone()), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body} gave {res}");
        assert!(res["error"].as_str().unwrap().contains(says), "{body}: {res}");
    }
    let (status, dobby, _) = call(&app, "POST", "/api/frogs/wizards", Some(&session), Some(json!({ "name": "Dobby", "rarity": "uncommon" })), true).await;
    assert_eq!(status, StatusCode::CREATED, "{dobby}");
    let dobby_id = dobby["id"].as_i64().unwrap();
    assert_eq!((dobby["slug"].as_str(), dobby["enabled"].as_bool(), dobby["copies"].as_i64()), (Some("dobby"), Some(true), Some(0)));
    let (status, _, _) = call(&app, "GET", &format!("/api/frogs/wizards/{dobby_id}/image"), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "no picture yet");

    let (status, pic) = upload(&app, &session, "image/png", Some("dobby.png"), png(2048)).await;
    assert_eq!(status, StatusCode::CREATED, "{pic}");
    let pic_id = pic["id"].as_str().unwrap().to_string();
    let (status, edited, _) = call(&app, "PUT", &format!("/api/frogs/wizards/{dobby_id}"), Some(&session),
        Some(json!({ "name": "Dobby the Free Elf", "rarity": "legendary", "image": pic_id, "enabled": false })), true).await;
    assert_eq!(status, StatusCode::OK, "{edited}");
    assert_eq!((edited["name"].as_str(), edited["slug"].as_str(), edited["rarity"].as_str(), edited["enabled"].as_bool()), (Some("Dobby the Free Elf"), Some("dobby"), Some("legendary"), Some(false)));
    assert_eq!(edited["image_source"], "media");
    let (status, body, headers) = call(&app, "GET", edited["image_url"].as_str().unwrap(), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(matches!(headers["content-type"].to_str().unwrap(), "image/png" | "image/jpeg"), "{headers:?}");
    assert!(body.as_str().is_some_and(|b| !b.is_empty()), "the picture comes back");
    let (status, refused, _) = call(&app, "DELETE", &format!("/api/media/{pic_id}"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::CONFLICT, "a card's picture can't be deleted from under it");
    assert!(refused["error"].as_str().unwrap().contains("Dobby the Free Elf frog card"), "{refused}");
    let (status, _, _) = call(&app, "PUT", "/api/frogs/wizards/99999", Some(&session), Some(json!({ "name": "Nobody", "rarity": "common" })), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(&app, "PUT", "/api/frogs/wizards/abc", Some(&session), Some(json!({ "name": "Nobody", "rarity": "common" })), true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // A test drop: a text channel only, never #safe-corner, one frog at a time.
    for (channel, code, says) in [
        ("", StatusCode::BAD_REQUEST, "Pick a channel"),
        ("41", StatusCode::BAD_REQUEST, "not a text channel"),
        ("777", StatusCode::BAD_REQUEST, "no channel 777"),
        (&SAFE.to_string()[..], StatusCode::BAD_REQUEST, "safe-corner"),
    ] {
        let (status, res, _) = call(&app, "POST", "/api/frogs/drop", Some(&session), Some(json!({ "channel_id": channel })), true).await;
        assert_eq!(status, code, "{channel}: {res}");
        assert!(res["error"].as_str().unwrap().contains(says), "{channel}: {res}");
    }
    let (status, dropped, _) = call(&app, "POST", "/api/frogs/drop", Some(&session), Some(json!({ "channel_id": "21" })), true).await;
    assert_eq!(status, StatusCode::OK, "{dropped}");
    assert_eq!((dropped["open"].as_bool(), dropped["channel"]["name"].as_str()), (Some(true), Some("general")));
    let drop_id = dropped["id"].as_i64().unwrap();
    let (status, busy, _) = call(&app, "POST", "/api/frogs/drop", Some(&session), Some(json!({ "channel_id": "21" })), true).await;
    assert_eq!(status, StatusCode::CONFLICT, "{busy}");

    // Someone catches it.
    let won = {
        let db = store::db().unwrap();
        let mut conn = db.lock();
        let d = store::get_drop(&conn, drop_id).unwrap();
        let answer = store::riddle(&conn, &d.riddle_id).unwrap().canonical().to_string();
        store::submit(&mut conn, drop_id, 1004, "Zoya", &format!("the {answer}"), d.dropped_at + 31).unwrap()
    };
    assert!(matches!(won, store::Submit::Won(_)), "{won:?}");
    let (_, page, _) = call(&app, "GET", "/api/frogs", Some(&session), None, false).await;
    let row = page["drops"].as_array().unwrap().iter().find(|d| d["id"] == drop_id).cloned().expect("listed");
    assert_eq!((row["status"].as_str(), row["winner"]["name"].as_str(), row["solved_secs"].as_i64()), (Some("caught"), Some("Zoya"), Some(31)));
    assert_eq!(row["edition"], 1);
    assert_eq!((row["by"]["name"].as_str(), row["channel"]["name"].as_str()), (Some("Kabir"), Some("general")));
    let riddle_id = row["riddle"]["id"].as_str().unwrap().to_string();
    assert!(row["riddle"]["text"].as_str().unwrap().ends_with('?'));
    let wizard_id = row["wizard_id"].as_i64().unwrap();
    assert_eq!(page["totals"]["cards"], 1);

    // Owners, both ways.
    let (status, mine, _) = call(&app, "GET", "/api/frogs/owners?member=1004", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!((mine["member"]["name"].as_str(), mine["cards"].as_array().unwrap().len(), mine["collected"].as_i64(), mine["of"].as_i64()), (Some("Zoya"), 1, Some(1), Some(10)));
    assert_eq!(mine["cards"][0]["edition"], 1);
    let (_, owners, _) = call(&app, "GET", &format!("/api/frogs/owners?wizard={wizard_id}"), Some(&session), None, false).await;
    assert_eq!(owners["owners"][0]["member"]["name"], "Zoya");
    assert_eq!(owners["owners"][0]["serials"], json!([mine["cards"][0]["serial"]]));
    assert_eq!(owners["owners"][0]["copies"], json!([{ "serial": mine["cards"][0]["serial"], "edition": 1 }]));
    let (status, _, _) = call(&app, "GET", "/api/frogs/owners", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _, _) = call(&app, "GET", "/api/frogs/owners?member=abc", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Retiring the riddle, and putting it back.
    let (status, retired, _) = call(&app, "POST", &format!("/api/frogs/riddles/{riddle_id}/retire"), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK, "{retired}");
    assert_eq!(retired["retired"], true);
    let (_, back, _) = call(&app, "POST", &format!("/api/frogs/riddles/{riddle_id}/retire"), Some(&session), Some(json!({ "retired": false })), true).await;
    assert_eq!(back["retired"], false);
    let (status, _, _) = call(&app, "POST", "/api/frogs/riddles/nope/retire", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The activity log says what happened in words.
    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let frog_rows: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"].as_str().unwrap_or("").starts_with("frog:")).collect();
    let said: Vec<(String, String)> = frog_rows.iter().map(|e| (e["label"].as_str().unwrap().to_string(), e["change"].as_str().unwrap().to_string())).collect();
    assert_eq!(said[0], (format!("Riddle {riddle_id}"), "Back in play".to_string()));
    assert!(said[1].1.starts_with("Retired · answer "), "{said:?}");
    assert_eq!(said[2].0, "Test frog drop");
    assert!(said[2].1.starts_with("Dropped ") && said[2].1.ends_with(" in #general"), "{said:?}");
    assert_eq!(said[3], ("Frog card “Dobby the Free Elf”".to_string(), "Renamed from Dobby, now legendary, picture changed, switched off".to_string()));
    assert_eq!(said[4], ("Frog card “Dobby”".to_string(), "Added".to_string()));
    assert!(frog_rows.iter().all(|e| e["section"]["id"] == "frogs"));

    // Earned cards and trades.
    let (royale_card, gift) = {
        use super::super::super::frog_store::AwardFor;
        use super::super::super::frog_trade as trade;
        let db = store::db().unwrap();
        let mut conn = db.lock();
        let now = chrono::Utc::now().timestamp();
        let daily = AwardFor { kind: "daily_top", day: "2026-09-14".into(), activity: "quiz".into(), total: 12, ..Default::default() };
        store::award_card(&mut conn, "frogtop:2026-09-14:quiz", 1005, "daily_top:quiz", &daily, (0.2, 0.2), now).unwrap().unwrap();
        let royale = AwardFor { kind: "royale", battle_id: Some(77), role: "champion".into(), ..Default::default() };
        let card = store::award_card(&mut conn, "frogroyale:77:champion", 1004, "royale_champion", &royale, (0.9, 0.9), now).unwrap().unwrap().card;
        let gift = trade::create(&mut conn, (1004, "Zoya"), (1005, "Arjun"), Some(900), 21, &[card.serial], &[], now, 3600).unwrap();
        (card, gift)
    };
    let (status, rewards, _) = call(&app, "GET", "/api/frogs/rewards", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{rewards}");
    assert_eq!((rewards["daily"][0]["activity_label"].as_str(), rewards["daily"][0]["member"]["name"].as_str(), rewards["daily"][0]["total"].as_i64()), (Some("🧠 Quiz"), Some("Arjun"), Some(12)));
    assert_eq!((rewards["royale"][0]["role"].as_str(), rewards["royale"][0]["card"]["serial"].as_i64()), (Some("champion"), Some(royale_card.serial)));
    let (status, trades, _) = call(&app, "GET", "/api/frogs/trades?member=1005", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{trades}");
    assert_eq!((trades["items"][0]["status"].as_str(), trades["items"][0]["from"]["name"].as_str(), trades["items"][0]["give"][0]["serial"].as_i64()), (Some("open"), Some("Zoya"), Some(royale_card.serial)));
    assert_eq!(trades["counts"]["open"], 1);
    let (status, _, _) = call(&app, "GET", "/api/frogs/trades?status=weird", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, none, _) = call(&app, "GET", "/api/frogs/trades?member=1003", Some(&session), None, false).await;
    assert_eq!((status, none["items"].as_array().unwrap().len()), (StatusCode::OK, 0));
    let (_, mine, _) = call(&app, "GET", "/api/frogs/owners?member=1004", Some(&session), None, false).await;
    let earned = mine["cards"].as_array().unwrap().iter().find(|c| c["serial"] == royale_card.serial).cloned().unwrap();
    assert_eq!((earned["origin"].as_str(), earned["traded_in"].as_bool(), earned["transfers"].as_array().unwrap().len()), (Some("royale_champion"), Some(false), 0));
    for (method, path) in [("GET", "/api/frogs/rewards"), ("GET", "/api/frogs/trades"), ("POST", &format!("/api/frogs/trades/{}/cancel", gift.id)[..])] {
        let (status, _, _) = call(&app, method, path, None, None, true).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    let (status, _, _) = call(&app, "POST", &format!("/api/frogs/trades/{}/cancel", gift.id), Some(&session), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "cancelling needs the panel header");
    let (status, cancelled, _) = call(&app, "POST", &format!("/api/frogs/trades/{}/cancel", gift.id), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::OK, "{cancelled}");
    assert_eq!((cancelled["status"].as_str(), cancelled["closed_by"]["name"].as_str()), (Some("cancelled"), Some("Kabir")));
    let (status, _, _) = call(&app, "POST", &format!("/api/frogs/trades/{}/cancel", gift.id), Some(&session), None, true).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = call(&app, "POST", "/api/frogs/trades/99999/cancel", Some(&session), None, true).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // A full set sold: Arjun holds one of each card in play.
    {
        use super::super::super::frog_store::AwardFor;
        let db = store::db().unwrap();
        let mut conn = db.lock();
        let now = chrono::Utc::now().timestamp();
        let in_play: Vec<i64> = store::wizards(&conn).into_iter().filter(|w| w.enabled).map(|w| w.id).collect();
        let owned: Vec<i64> = store::cards_of(&conn, 1005).iter().map(|c| c.wizard_id).collect();
        for (i, w) in in_play.iter().enumerate() {
            if owned.contains(w) {
                continue;
            }
            store::award_card(&mut conn, &format!("test-set-{i}"), 1005, "royale_champion", &AwardFor { kind: "royale", ..Default::default() }, (0.0, 0.0), now).unwrap();
            conn.execute("UPDATE cards SET wizard_id = ?1 WHERE serial = (SELECT MAX(serial) FROM cards)", rusqlite::params![w]).unwrap();
        }
        let plan: Vec<i64> = store::sale_plan(&conn, 1005).unwrap().iter().map(|c| c.serial).collect();
        store::sell_set(&mut conn, 1005, &plan, 15, now).unwrap().unwrap();
    }
    let (status, sold, _) = call(&app, "GET", "/api/frogs/sales", Some(&session), None, false).await;
    assert_eq!(status, StatusCode::OK, "{sold}");
    assert_eq!((sold["items"][0]["member"]["name"].as_str(), sold["items"][0]["points"].as_i64(), sold["price"].as_i64()), (Some("Arjun"), Some(15), Some(35)));
    assert_eq!(sold["items"][0]["cards"].as_array().unwrap().len(), 10);
    let (status, _, _) = call(&app, "GET", "/api/frogs/sales", None, None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=5", Some(&session), None, false).await;
    let last = audit.as_array().unwrap().iter().find(|e| e["key"] == format!("frog:trade:{}", gift.id)).cloned().expect("logged");
    assert_eq!(last["change"], "Cancelled the offer from Zoya to Arjun");
}

// --- search messages ------------------------------------------------------------------------

async fn search_api(app: &Router, session: &str, query: &str) -> (StatusCode, Value) {
    let (status, body, _) = call(app, "GET", &format!("/api/messages/search?{}", query), Some(session), None, false).await;
    (status, body)
}

fn texts(body: &Value) -> Vec<String> {
    body["results"].as_array().unwrap().iter().map(|r| r["text"].as_str().unwrap().to_string()).collect()
}

/// Follows the cursor to the end: every result, and the last page.
async fn search_all(app: &Router, session: &str, query: &str) -> (Value, Value) {
    let mut all = Vec::new();
    let mut before = String::new();
    for _ in 0..30 {
        let (status, body) = search_api(app, session, &format!("{query}{before}")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        all.extend(body["results"].as_array().unwrap().iter().cloned());
        match body["next_before"].as_i64() {
            Some(b) => before = format!("&before={b}"),
            None => return (json!({ "results": all }), body),
        }
    }
    panic!("paging never ends for {query}");
}

#[tokio::test]
async fn message_search_finds_words_and_never_shows_dms_or_safe_corner() {
    let app = panel();
    let session = session_for(ADMIN);
    let (body, last) = search_all(&app, &session, "q=ZebraFish&days=all").await;
    let found = texts(&body);
    for want in ["Anyone seen the ZEBRAFISH meme?", "zebrafish again lol", "zebrafish in a thread under general", "zebrafish with a topic", "zebrafish from someone else", "zebrafish from long ago"] {
        assert!(found.iter().any(|t| t == want), "missing {want}: {found:?}");
    }
    for never in ["DM", "SECRET"] {
        assert!(!found.iter().any(|t| t.contains(never)), "{never} leaked: {found:?}");
    }
    assert!(!found.iter().any(|t| t == "hello there"), "words only in the mods' notes don't count");
    assert!(!found.iter().any(|t| t == "yes"), "words only in the quoted message don't count");
    assert_eq!(last["complete"], true);
    assert!(last["scanned_to"].is_null(), "all time has no start");

    // Newest first, with the member, house, channel, reply and jump link.
    let first = &body["results"][0];
    assert_eq!(first["text"], "Anyone seen the ZEBRAFISH meme?");
    assert_eq!(first["member"]["id"], "2012");
    assert_eq!(first["member"]["name"], "Sameer");
    assert!(first["member"]["avatar"].as_str().unwrap().starts_with("data:image"));
    assert_eq!(first["house"]["key"], "gryffindor");
    assert_eq!(first["channel"], json!({ "id": "21", "name": "general", "thread": false }));
    assert_eq!(first["reply_to"], json!({ "author": "Dev", "text": "which meme are you on about" }));
    assert_eq!(first["url"], "https://discord.com/channels/900/21/7001");
    let (start, end) = (first["match"][0].as_u64().unwrap() as usize, first["match"][1].as_u64().unwrap() as usize);
    assert_eq!(first["text"].as_str().unwrap().chars().skip(start).take(end - start).collect::<String>(), "ZEBRAFISH");
    let snip = &first["snippet"];
    let (ss, se) = (snip["start"].as_u64().unwrap() as usize, snip["end"].as_u64().unwrap() as usize);
    assert_eq!(snip["text"].as_str().unwrap().chars().skip(ss).take(se - ss).collect::<String>(), "ZEBRAFISH");
    let ts: Vec<i64> = body["results"].as_array().unwrap().iter().map(|r| r["ts_ms"].as_i64().unwrap()).collect();
    assert!(ts.windows(2).all(|w| w[0] >= w[1]), "newest first: {ts:?}");

    let by_text = |t: &str| body["results"].as_array().unwrap().iter().find(|r| r["text"] == t).cloned().unwrap();
    let thread = by_text("zebrafish in a thread under general");
    assert_eq!(thread["channel"], json!({ "id": "78", "name": "general", "thread": true }));
    assert_eq!(thread["url"], "https://discord.com/channels/900/78/7010");
    assert!(by_text("zebrafish again lol")["url"].is_null(), "no message id, no link");
    let stranger = by_text("zebrafish from someone else");
    assert_eq!(stranger["member"]["name"], "Imposter", "the stored name when the cache doesn't know them");
    assert!(stranger["house"].is_null());

    // The default period is 30 days.
    let (month, last) = search_all(&app, &session, "q=zebrafish").await;
    assert!(!texts(&month).iter().any(|t| t == "zebrafish from long ago"));
    assert_eq!(last["days"], "30");
    let start = last["scanned_to"].as_i64().unwrap();
    assert!((start - (chrono::Utc::now().timestamp() - 30 * 86_400)).abs() < 60, "searched back to the period's start");
    let (quarter, _) = search_all(&app, &session, "q=zebrafish&days=90").await;
    assert!(texts(&quarter).iter().any(|t| t == "zebrafish from long ago"));
    let (day, _) = search_all(&app, &session, "q=zebrafish&days=1").await;
    assert_eq!(texts(&day), vec!["Anyone seen the ZEBRAFISH meme?"]);
}

#[tokio::test]
async fn message_search_matches_case_in_any_script_and_escapes_wildcards() {
    let app = panel();
    let session = session_for(ADMIN_TWO);
    for q in ["%C3%A9clair", "CAF%C3%89", "%C3%89CLAIR%20AU"] {
        let (status, body) = search_api(&app, &session, &format!("q={q}&days=7")).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(texts(&body), vec!["ÉCLAIR au café, anyone?"], "{q}");
    }
    let (_, body) = search_api(&app, &session, "q=100%25&days=7").await;
    assert_eq!(texts(&body), vec!["this is 100% real"]);
    let (_, body) = search_api(&app, &session, "q=a_b&days=7").await;
    assert_eq!(texts(&body), vec!["snake a_b case"]);
    let (body, last) = search_all(&app, &session, "q=nothing-says-this&days=all").await;
    assert!(texts(&body).is_empty());
    assert_eq!(last["complete"], true);
}

#[tokio::test]
async fn message_search_filters_by_member_and_channel() {
    let app = panel();
    let session = session_for(ADMIN);
    let (body, last) = search_all(&app, &session, "q=zebrafish&days=all&member=2012").await;
    assert_eq!(texts(&body), vec!["Anyone seen the ZEBRAFISH meme?", "zebrafish from long ago"], "not 20120, never safe-corner");
    assert_eq!(last["member"], json!({ "id": "2012", "name": "Sameer" }));
    let (body, last) = search_all(&app, &session, "q=zebrafish&days=all&channel=21").await;
    let found = texts(&body);
    assert!(found.contains(&"zebrafish with a topic".to_string()), "a topic in the channel counts: {found:?}");
    assert!(found.contains(&"Anyone seen the ZEBRAFISH meme?".to_string()));
    assert!(!found.contains(&"zebrafish again lol".to_string()));
    assert!(body["results"].as_array().unwrap().iter().all(|r| r["channel"]["id"] == "21"));
    assert_eq!(last["channel"], json!({ "id": "21", "name": "general" }));
    let (status, body) = search_api(&app, &session, &format!("q=zebrafish&channel={SAFE}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body["error"].as_str().unwrap().contains("safe-corner"));
    let (body, _) = search_all(&app, &session, "q=zebrafish&days=all&channel=5000").await;
    assert!(texts(&body).is_empty(), "a DM channel gives nothing");
}

#[tokio::test]
async fn message_search_pages_with_a_cursor_and_a_scan_budget() {
    let app = panel();
    let session = session_for(ADMIN);
    let mut seen: Vec<String> = Vec::new();
    let mut before: Option<i64> = None;
    let mut pages = Vec::new();
    loop {
        let query = format!("q=pagetoken&days=all&limit=3{}", before.map(|b| format!("&before={b}")).unwrap_or_default());
        let (status, body) = search_api(&app, &session, &query).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["count"].as_u64().unwrap() <= 3);
        pages.push(body["count"].as_u64().unwrap());
        seen.extend(texts(&body));
        match body["next_before"].as_i64() {
            Some(b) => {
                assert!(body["scanned_to"].as_i64().unwrap() > 0);
                before = Some(b);
            }
            None => break,
        }
        assert!(pages.len() < 20, "paging never ends");
    }
    let want: Vec<String> = (1..=7).map(|i| format!("pagetoken number {i}")).collect();
    assert_eq!(seen, want, "every message once, newest first");
    assert_eq!(&pages[..2], &[3, 3]);

    // A word that sits past the scan budget: the first request hands back a cursor.
    let (_, first) = search_api(&app, &session, "q=oldtoken&days=all").await;
    assert_eq!(first["count"], 0);
    assert_eq!(first["complete"], false);
    let mut cursor = first["next_before"].as_i64().expect("a cursor");
    let mut found = false;
    for _ in 0..10 {
        let (_, next) = search_api(&app, &session, &format!("q=oldtoken&days=all&before={cursor}")).await;
        if next["count"] == 1 {
            assert_eq!(texts(&next), vec!["the oldtoken is here"]);
            found = true;
            break;
        }
        cursor = next["next_before"].as_i64().expect("more to read");
    }
    assert!(found);
}

#[tokio::test]
async fn message_search_checks_its_input_and_needs_an_admin() {
    let app = panel();
    let session = session_for(ADMIN);
    for bad in ["", "q=", "q=a", "q=%20a%20", &format!("q={}", "x".repeat(101)), "q=koto&days=5", "q=koto&member=abc", "q=koto&channel=x1", "q=koto&before=-4", "q=koto&limit=lots"] {
        let (status, body) = search_api(&app, &session, bad).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert!(body["error"].is_string());
    }
    let (status, body) = search_api(&app, &session, "q=koto&limit=1000").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["limit"], 100);
    assert!(body["count"].as_u64().unwrap() <= 100);
    let (_, body) = search_api(&app, &session, "q=koto").await;
    assert_eq!(body["limit"], 50);
    assert_eq!(body["count"], 50);
    assert!(body["next_before"].is_i64());

    let (status, _, _) = call(&app, "GET", "/api/messages/search?q=koto", None, None, false).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&app, "GET", "/api/messages/search?q=koto", Some(&session_for(MEMBER)), None, false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn message_searches_are_in_the_activity_log_once() {
    let app = panel();
    let session = session_for(ADMIN_TWO);
    let q = "q=Arijit%20on&member=2005&channel=24&days=7";
    let (status, _) = search_api(&app, &session, q).await;
    assert_eq!(status, StatusCode::OK);
    let (_, _) = search_api(&app, &session, q).await;
    let (_, audit, _) = call(&app, "GET", "/api/audit?limit=1000", Some(&session), None, false).await;
    let entries: Vec<&Value> = audit.as_array().unwrap().iter().filter(|e| e["key"] == "messages:search" && e["change"] == "“Arijit on” · @Anaya · #music · last 7 days").collect();
    assert_eq!(entries.len(), 1, "a repeat isn't logged again: {audit}");
    assert_eq!(entries[0]["label"], "Searched messages");
    assert_eq!(entries[0]["user_id"], ADMIN_TWO.to_string());
    assert_eq!(entries[0]["section"]["id"], "search");
}

// --- the demo ----------------------------------------------------------------------------

/// In the demo, the page and its assets come straight from disk, so a change to
/// the UI shows on reload without rebuilding.
async fn ui_from_disk(req: axum::extract::Request, next: axum::middleware::Next) -> axum::response::Response {
    let file = match req.uri().path() {
        "/" | "/login" => "index.html",
        "/assets/app.css" => "app.css",
        "/assets/app.js" => "app.js",
        _ => return next.run(req).await,
    };
    let res = next.run(req).await;
    let path = format!("{}/src/channels/discord/control/ui/{}", env!("CARGO_MANIFEST_DIR"), file);
    match std::fs::read(path) {
        Ok(bytes) => {
            let (parts, _) = res.into_parts();
            axum::response::Response::from_parts(parts, Body::from(bytes))
        }
        Err(_) => res,
    }
}

/// Serves the fake panel for a look in a browser:
/// `PANEL_DEMO_SECS=600 cargo test control::web::tests::demo_server -- --ignored --nocapture`
/// Port 8799 is signed in as Kabir; 8798 is signed out.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn demo_server() {
    use super::super::{DB, reminders::{Order, Reminder, Schedule}};
    store();
    // Safe here: this test runs alone, before anything reads the environment.
    unsafe {
        std::env::set_var("VIZIER_FIGHT_CHANNEL", "31");
        std::env::set_var("PANEL_TEST_QUIZ_CHANNEL", "32");
        std::env::set_var("PANEL_TEST_SNITCH_PER_DAY", "8");
        std::env::set_var("PANEL_TEST_QUIZ_FEEDS", "https://feeds.bbci.co.uk/news/rss.xml");
        std::env::set_var("PANEL_TEST_HOUSE_CHANNEL", "33");
        std::env::set_var("PANEL_TEST_SNITCH_CHANNELS", "21:1,22:1");
        std::env::set_var("VIZIER_WELCOME_CHANNEL", "11");
    }
    let now = chrono::Utc::now().timestamp();
    {
        // Older history first: the log lists by insertion.
        let conn = DB.get().unwrap().lock();
        let rows: &[(i64, u64, &str, Option<&str>, Option<&str>)] = &[
            (now - 50 * 3600, ADMIN, "PANEL_TEST_ARENA", Some("off"), Some("on")),
            (now - 30 * 3600, ADMIN_TWO, "PANEL_TEST_QUIZ_TIMEOUT", Some("20"), None),
            (now - 26 * 3600, ADMIN, "VIZIER_FIGHT_CHANNEL", None, Some("31")),
        ];
        for (ts, by, key, old, new) in rows {
            conn.execute(
                "INSERT INTO audit (ts, user_id, key, old, new) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![ts, *by as i64, key, old, new],
            )
            .unwrap();
        }
    }
    let set = |k: &str, v: &str, by: u64| super::super::set(k, Some(v), by).unwrap();
    set("PANEL_TEST_FIGHT_POINTS", "15", ADMIN_TWO);
    set("PANEL_TEST_SNITCH_CHANNELS", "21:5,22:3,23:2", ADMIN);
    set("PANEL_TEST_HOUSE_CAPTAINS", "1004,1005,1007", ADMIN);
    set("PANEL_TEST_MUGGLE_ROLE", "58", ADMIN_TWO);
    set("PANEL_TEST_QUIZ", "off", ADMIN);
    set("PANEL_TEST_QUIZ_DIFFICULTY", "hard", ADMIN_TWO);
    set("PANEL_TEST_FIGHT_STYLE", "cards", ADMIN);

    let base = Reminder {
        id: 0,
        name: String::new(),
        enabled: true,
        channel_id: "21".into(),
        lines: vec![],
        order: Order::Rotate,
        schedule: Schedule::Every { minutes: 60 },
        active_from: String::new(),
        active_to: String::new(),
        user_id: String::new(),
        user_name: String::new(),
        since: String::new(),
        stop_when_back: false,
        welcome_line: String::new(),
        ends: String::new(),
        last_sent: 0,
        sent_count: 0,
        ..Default::default()
    };
    let zoya = Reminder {
        name: "Where is Zoya?".into(),
        lines: vec![
            "{mention} has been gone for {days} days. The memes channel is not the same.".into(),
            "Day {days} without {name}. Somebody check on her.".into(),
            "{hours} hours and counting. {name}, come back!".into(),
        ],
        channel_id: "23".into(),
        schedule: Schedule::Daily { times: vec!["10:00".into(), "21:30".into()] },
        user_id: "1004".into(),
        user_name: "Zoya".into(),
        since: "2026-09-03T18:00:00+05:30".into(),
        stop_when_back: true,
        welcome_line: "{mention} is BACK. Normal service resumes.".into(),
        last_sent: now - 3 * 3600,
        sent_count: 21,
        ..base.clone()
    };
    let water = Reminder {
        name: "Hydration check".into(),
        lines: vec!["Drink some water, legends. 💧".into(), "Posture check. Shoulders down.".into()],
        order: Order::Random,
        schedule: Schedule::Every { minutes: 120 },
        active_from: "10:00".into(),
        active_to: "23:00".into(),
        enabled: false,
        last_sent: now - 26 * 3600,
        sent_count: 143,
        ..base.clone()
    };
    if super::super::reminders::list().is_empty() {
        super::super::reminders::save(&zoya, 0).unwrap();
        super::super::reminders::save(&water, 0).unwrap();
        let r = super::super::reminders::list()[0].clone();
        super::super::reminders::save(&r, ADMIN).unwrap();

        // Richer posts, with pictures from PANEL_DEMO_MEDIA_DIR when it's set.
        use super::super::reminders::{ImageOrder, Style};
        let mut pics: HashMap<String, String> = HashMap::new();
        if let Ok(dir) = std::env::var("PANEL_DEMO_MEDIA_DIR") {
            let mut files: Vec<_> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
            files.sort();
            for path in files {
                let name = path.file_name().unwrap().to_string_lossy().to_string();
                if let Ok(saved) = super::super::media::save(&name, &std::fs::read(&path).unwrap(), ADMIN_TWO) {
                    pics.insert(name, saved.id);
                }
            }
        }
        let pic = |name: &str| pics.get(name).cloned().into_iter().collect::<Vec<_>>();
        let morning = Reminder {
            name: "Good morning".into(),
            channel_id: "21".into(),
            lines: vec![
                "Good morning, legends! ☀️ **{day}** is here. Chai ready hai?".into(),
                "Suprabhat, {date}! Aaj ka plan kya hai? ☕".into(),
            ],
            schedule: Schedule::Daily { times: vec!["08:00".into()] },
            images: [pic("good-morning.png"), pic("chai-time.jpg")].concat(),
            image_order: ImageOrder::Rotate,
            reactions: vec!["☀️".into(), "<:mlci_heart:912345678901234004>".into()],
            last_sent: now - 5 * 3600,
            sent_count: 38,
            ..base.clone()
        };
        let cup = Reminder {
            name: "House Cup standings".into(),
            channel_id: "33".into(),
            style: Style::Card,
            title: "🏆 House Cup · {date}".into(),
            colour: "#c28b2c".into(),
            footer: "Points reset on the 1st · /houses for more".into(),
            lines: vec!["{standings}\n\n**{leader}** lead the month. Everyone else, it's not over yet!".into()],
            schedule: Schedule::Daily { times: vec!["22:00".into()] },
            images: pic("house-cup.png"),
            delete_previous: true,
            last_message_id: "1290000000000000001".into(),
            last_sent: now - 20 * 3600,
            sent_count: 12,
            ..base.clone()
        };
        let qotd = Reminder {
            name: "Question of the day".into(),
            channel_id: "22".into(),
            style: Style::Card,
            title: "❓ Question of the day".into(),
            colour: "#e0a43a".into(),
            footer: "Answer below 👇".into(),
            ai_prompt: "A fun, easy question for everyone to answer in chat: this-or-that, food, films, cricket or games. One question only.".into(),
            lines: vec!["Chai or coffee, and why is it chai?".into(), "What's one song you've had on repeat this week?".into()],
            order: Order::Random,
            schedule: Schedule::Daily { times: vec!["18:00".into()] },
            reactions: vec!["👀".into()],
            ai_recent: vec!["Would you rather give up biryani or pani puri forever?".into()],
            last_sent: now - 23 * 3600,
            sent_count: 9,
            ..base.clone()
        };
        let hydrate = Reminder {
            name: "Pani break".into(),
            channel_id: "23".into(),
            lines: vec!["💧 {random:Pani pee lo, doston|Hydration check|Sip sip, hooray} — it's {time}.".into()],
            schedule: Schedule::Every { minutes: 120 },
            active_from: "10:00".into(),
            active_to: "23:00".into(),
            images: [pic("pani-pee-lo.gif"), pic("stretch.webp")].concat(),
            image_order: ImageOrder::Random,
            delete_previous: true,
            last_sent: now - 3600,
            sent_count: 77,
            ..base.clone()
        };
        for r in [&morning, &cup, &qotd, &hydrate] {
            super::super::reminders::save(r, ADMIN).unwrap();
        }
    }

    // Members' own reminders: some waiting, some done.
    if super::super::memos::all(1).is_empty() {
        use super::super::memos;
        let set = |user: u64, channel: u64, text: &str, due: i64, via: &str, by: u64| {
            if by == 0 { memos::create(user, channel, text, due, via, 0).unwrap() } else { memos::create_by_admin(user, channel, text, due, by).unwrap() }
        };
        let conn_done = |id: i64, status: &str, created: i64, sent: i64| {
            DB.get()
                .unwrap()
                .lock()
                .execute(
                    "UPDATE member_reminders SET status = ?2, created_ts = ?3, due_ts = ?4, sent_ts = ?4 WHERE id = ?1",
                    rusqlite::params![id, status, created, sent],
                )
                .unwrap();
        };
        let a = set(2003, 21, "call mum before the quiz starts", now + 3 * 3600, "chat", 0);
        conn_done(a.id, "sent", now - 30 * 3600, now - 26 * 3600);
        let b = set(2011, 32, "check if the quiz leaderboard reset", now + 3600, "command", 0);
        conn_done(b.id, "cancelled", now - 9 * 3600, now - 4 * 3600);
        set(MEMBER, 21, "submit the fantasy team before the toss", now + 47 * 60, "chat", 0);
        set(2007, 22, "post the meme of the week", now + 5 * 3600 + 20 * 60, "command", 0);
        set(1004, 31, "rematch with Dev in the arena", now + 26 * 3600, "panel", ADMIN_TWO);
        set(2015, SAFE, "private note", now + 2 * 3600, "chat", 0);
        set(1005, 5_000, "take medicine", now + 8 * 3600, "chat", 0);
        set(2020, 23, "wish Tanvi happy birthday 🎂", now + 3 * 86_400, "panel", ADMIN);
    }
    if super::super::autoreplies::list().is_empty() {
        let rule = |v: Value| serde_json::from_value::<super::super::autoreplies::AutoReply>(v).unwrap();
        let gm = rule(json!({
            "name": "Good morning", "enabled": true, "triggers": ["gm", "good morning", "suprabhat"],
            "match_mode": "whole_word", "channels": ["21", "23"], "replies": ["gm {user} ☀️", "Good morning {name}! Chai ready hai?"],
            "reactions": ["☀️"], "chance": 60, "cooldown_secs": 300, "hits": 412, "last_hit": now - 2400
        }));
        let bruh = rule(json!({
            "name": "Bruh moment", "enabled": true, "triggers": ["bruh", "bhai kya"], "match_mode": "contains",
            "replies": [], "reactions": ["💀", "<:kekw:912345678901234001>"], "chance": 35, "cooldown_secs": 90,
            "exclude_channels": ["12", "13"], "hits": 1290, "last_hit": now - 300
        }));
        let quiz = rule(json!({
            "name": "When is quiz night", "enabled": false, "triggers": ["^when.*quiz"], "match_mode": "pattern",
            "channels": ["21"], "replies": ["Quiz night is every Friday at 9 PM in #quiz. Bring your A game, {name}."],
            "as_reply": true, "chance": 100, "cooldown_secs": 3600, "hits": 37, "last_hit": now - 5 * 86400
        }));
        for r in [&gm, &bruh, &quiz] {
            super::super::autoreplies::save(r, 0).unwrap();
        }
        let first = super::super::autoreplies::list()[1].clone();
        super::super::autoreplies::save(&first, ADMIN_TWO).unwrap();
        super::super::log_change("agent:silent_read_initiative_chance", Some("0.02"), Some("0.04"), ADMIN).unwrap();
    }

    if super::super::members::list().is_empty() {
        use super::super::members::{MemberNote, Tone};
        let note = |id: u64, name: &str, tone: Tone, notes: &str| MemberNote { tone, notes: notes.into(), ..MemberNote::blank(id, name) };
        super::super::members::save(&note(2012, "Sameer", Tone::Roast, "Loud RCB fan, takes roasts about it well. Never calls him Sam."), ADMIN).unwrap();
        super::super::members::save(&note(2003, "Meera", Tone::Brief, "Prefers short replies without emoji."), ADMIN_TWO).unwrap();
        super::super::members::save(&note(2007, "Zoya", Tone::Gentle, "Going through exams; keep it kind and don't bring up her marks."), ADMIN).unwrap();
    }

    // Special welcomes: Lucky's is seeded by the store (posting in a channel the
    // fake server has); one that has gone out on every join, and one switched off.
    {
        use super::super::welcomes::{self, Mode, Welcome};
        if let Some(mut lucky) = welcomes::list().into_iter().find(|w| w.user_id == LUCKY.to_string() && w.channel_id == "1516492867642593443") {
            lucky.channel_id = "23".into();
            lucky.created_ts = now - 2 * 86_400;
            welcomes::save(&lucky, 0).unwrap();
        }
        if welcomes::list().len() < 3 {
            let dev = Welcome {
                user_id: "1007".into(),
                user_name: "Dev".into(),
                lines: vec![
                    "{mention} is back. **{n}** time. Ek fight haar ke gaya tha, {hours} ghante mein wapas 😂".into(),
                    "Dekho kaun aaya! {mention}, entry number **{n}**. <@1005> ready rehna, rematch hoga.".into(),
                ],
                also_ping: vec!["1005".into()],
                mode: Mode::Every,
                replace_normal: false,
                fired_count: 4,
                last_fired_ts: now - 2 * 86_400 - 3 * 3600,
                last_fired_message_id: "1290000000000000555".into(),
                created_by: ADMIN.to_string(),
                created_ts: now - 40 * 86_400,
                note: "Leaves after every lost fight and is back the next day.".into(),
                ..Default::default()
            };
            let tanvi = Welcome {
                user_id: "1008".into(),
                user_name: "Tanvi".into(),
                channel_id: "21".into(),
                lines: vec!["Tanvi is back from exam jail! 📚➡️🎉 {mention}, <@1004> aur <@1006> ne tera spot sambhal ke rakha tha.".into()],
                also_ping: vec!["1004".into(), "1006".into()],
                enabled: false,
                created_by: ADMIN_TWO.to_string(),
                created_ts: now - 6 * 86_400,
                note: "Paused until her exams are over - she said she'd be back in October.".into(),
                ..Default::default()
            };
            welcomes::save(&dev, ADMIN).unwrap();
            welcomes::save(&tanvi, ADMIN_TWO).unwrap();
        }
    }

    if super::super::profiles::get(2012).is_none() {
        use super::super::profiles::{Analysis, Profile, Status};
        let now = chrono::Utc::now().timestamp();
        let profile = |id: u64, name: &str, status: Status, ai: Analysis, edits: Vec<(&str, Value)>| Profile {
            user_id: id.to_string(),
            name: name.into(),
            ai,
            edits: edits.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            status,
            stats: json!({}),
            messages_analysed: 212,
            chars_analysed: 11_480,
            window_days: 30,
            generated_ts: now - 5 * 3600,
            generated_by: ADMIN.to_string(),
            model: "openrouter/google/gemini-2.5-flash".into(),
            edited_ts: now - 3600,
            edited_by: ADMIN_TWO.to_string(),
        };
        let sameer = Analysis {
            summary: "Shows up most evenings in #general and #desi-banter, usually with cricket takes, fantasy-league trash talk and quiz-night chatter. Plays Koto and the Friday quiz regularly and jumps into Snitch drops fast. Friendly and quick to reply; starts more jokes than arguments.".into(),
            interests: vec!["Cricket, especially RCB".into(), "Friday quiz nights".into(), "Koto".into(), "Memes".into(), "Arijit Singh songs".into()],
            style: "Short Hinglish messages, often several in a row. Heavy on 💀 and ☕, light on punctuation.".into(),
            games: "Top-10 Gryffindor scorer: quiz and Snitch catches carry most of his points; wins a bit over half his fights.".into(),
            vibe_with_bot: "Asks the bot for standings, Koto hints and to roast friends' fantasy teams; takes jokes back well.".into(),
            suggested_tone: super::super::members::Tone::LightRoast,
            tone_reason: "Enjoys banter and dishes it out himself, but keep it about the game.".into(),
            roast_material: vec!["Needs six tries at Koto on a good day".into(), "Predicts an RCB title every single April".into()],
            avoid: vec!["Exam or work results".into()],
        };
        super::super::profiles::save(
            &profile(2012, "Sameer", Status::Draft, sameer, vec![("avoid", json!(["Exam or work results", "His fantasy team's actual rank"]))]),
            ADMIN,
            "generated",
            &[],
        )
        .unwrap();
        let meera = Analysis {
            summary: "A steady presence in #quiz and #general who mostly answers other people's questions. Rarely starts conversations but is reliable at quiz nights.".into(),
            interests: vec!["Quizzes".into(), "Books".into()],
            style: "Full sentences, no emoji, polite.".into(),
            games: "Quiz points almost every day; rarely fights.".into(),
            vibe_with_bot: "Uses the bot for quiz questions only.".into(),
            suggested_tone: super::super::members::Tone::Brief,
            tone_reason: "Prefers short, straightforward replies.".into(),
            roast_material: vec![],
            avoid: vec![],
        };
        super::super::profiles::save(&profile(2003, "Meera", Status::Reviewed, meera, vec![]), ADMIN_TWO, "reviewed", &[]).unwrap();
    }
    if super::super::members::get(2020).is_none() {
        use super::super::members::{MemberNote, NoteSource, Tone};
        let auto = |id: u64, name: &str, tone: Tone, text: &str| MemberNote {
            tone,
            notes: text.into(),
            use_in_replies: false,
            source: NoteSource::Analysis,
            reviewed: false,
            filled_ts: chrono::Utc::now().timestamp() - 2 * 3600,
            ..MemberNote::blank(id, name)
        };
        let fills = [
            (2020, "Nikhil", Tone::LightRoast, "Late-night regular in #desi-banter and voice; lives for Battle Royale and trash-talks after every win.\nInterests: fantasy cricket, royale, memes\nStyle: Hinglish, quick one-liners, lots of 😂\nPlays: arena and royale most days\nRoast angles: always \"one more round\" at 2 AM\nAvoid: exam results"),
            (2031, "Mehak", Tone::Gentle, "Mostly answers other people's questions in #general and quiz nights; rarely starts threads.\nInterests: books, quizzes\nStyle: full sentences, polite, no emoji\nPlays: quiz most days"),
            (2017, "Yash", Tone::Normal, "Quiz regular who posts the day's Koto score every morning.\nInterests: Koto, quizzes, F1\nPlays: quiz, Koto, the odd Snitch"),
        ];
        for (id, name, tone, text) in fills {
            super::super::members::save(&auto(id, name, tone, text), super::super::members::AUTO_FILL_BY).unwrap();
        }
    }
    if super::super::insights::meta_get("demo_seeded").is_none() {
        use super::super::insights::{self, Interaction, Kind};
        let now = chrono::Utc::now().timestamp();
        let mut seed: u64 = 0xdecaf;
        let mut roll = |n: u64| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 33) % n.max(1)
        };
        let channels = [21u64, 23, 22, 32, 24, 31];
        let mut rows: Vec<Interaction> = Vec::new();
        let mut id = 5_000_000u64;
        let mut push = |rows: &mut Vec<Interaction>, ts: i64, from: u64, to: u64, channel: u64, kind: Kind, id: &mut u64| {
            *id += 1;
            let source = if ts < now - 6 * 86_400 { "history" } else { "live" };
            rows.push(Interaction { ts, channel_id: channel, from_user: from, to_user: to, message_id: *id, replied_message_id: None, kind, source });
        };
        // Close pairs, with how often they talk and the channel they favour.
        let duos: [(u64, u64, u64, usize); 8] = [
            (2012, 2010, 23, 26), (2004, 2016, 21, 18), (2003, 2017, 32, 14), (2001, 2023, 21, 12),
            (2020, 2008, 31, 11), (2030, 2005, 22, 9), (2007, 2034, 24, 8), (2011, 2027, 23, 7),
        ];
        for day in 0..30i64 {
            for (a, b, ch, weight) in duos {
                let sessions = roll((weight / 9 + 2) as u64) as usize;
                for _ in 0..sessions {
                    let start = now - day * 86_400 - roll(80_000) as i64;
                    let len = 1 + roll(if a == 2012 { 11 } else { 6 }) as usize;
                    let (mut from, mut to) = if roll(2) == 0 { (a, b) } else { (b, a) };
                    let mut ts = start;
                    for _ in 0..len {
                        push(&mut rows, ts, from, to, ch, Kind::Reply, &mut id);
                        std::mem::swap(&mut from, &mut to);
                        ts += 30 + roll(400) as i64;
                    }
                }
            }
            // General chatter: anyone replying to anyone.
            for _ in 0..(40 + roll(30)) {
                let from = 2000 + roll(43);
                let to = 2000 + roll(43);
                if from != to {
                    let kind = if roll(5) == 0 { Kind::Mention } else { Kind::Reply };
                    push(&mut rows, now - day * 86_400 - roll(86_000) as i64, from, to, channels[roll(6) as usize], kind, &mut id);
                }
            }
            // Dev keeps replying to Zoya; she rarely answers.
            for _ in 0..(2 + roll(3)) {
                push(&mut rows, now - day * 86_400 - roll(80_000) as i64, 2010, 2007, 21, Kind::Reply, &mut id);
            }
            // Everyone pings Om.
            for _ in 0..roll(4) {
                push(&mut rows, now - day * 86_400 - roll(80_000) as i64, 2000 + roll(43), 2036, channels[roll(3) as usize], Kind::Mention, &mut id);
            }
        }
        // The month's great back-and-forth: Sunday night in #desi-banter.
        let sunday = now - 2 * 86_400 - 2 * 3_600;
        for i in 0..23 {
            let (from, to) = if i % 2 == 0 { (2012, 2010) } else { (2010, 2012) };
            push(&mut rows, sunday + i * 95, from, to, 23, Kind::Reply, &mut id);
        }
        // A new friendship this week.
        for i in 0..9 {
            let (from, to) = if i % 2 == 0 { (2041, 2019) } else { (2019, 2041) };
            push(&mut rows, now - 3 * 86_400 + i * 200, from, to, 24, Kind::Reply, &mut id);
        }
        rows.retain(|r| r.from_user != r.to_user && !(r.from_user == 2007 && r.to_user == 2010));
        insights::insert(&rows).unwrap();
        insights::meta_set("backfill_done", &(now - 6 * 86_400).to_string());
        insights::meta_set("demo_seeded", "1");
    }
    MODEL_DELAY_MS.store(2500, std::sync::atomic::Ordering::SeqCst);

    // Spread today's changes over the last few hours.
    DB.get()
        .unwrap()
        .lock()
        .execute(
            "UPDATE audit SET ts = ?1 - ((SELECT MAX(id) FROM audit) - id + 1) * 1500 WHERE ts >= ?2",
            rusqlite::params![now, now - 60],
        )
        .unwrap();


    // Chocolate Frogs: the real riddle bank, Luna's picture, three days of drops.
    {
        use super::super::super::frog_store::{self as store, Submit};
        let db = store::db().unwrap();
        let mut conn = db.lock();
        if store::meta_get(&conn, "demo_seeded").is_none() {
            let bank = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("riddlebank/ai");
            store::import_riddles(&mut conn, &bank).unwrap();
            // The owner's card pictures, as they would sit in {workspace}/frogcards.
            if let Ok(dir) = std::env::var("PANEL_DEMO_FROGCARDS") {
                let into = STORE.get().unwrap().path().join("frogcards");
                std::fs::create_dir_all(&into).unwrap();
                for entry in std::fs::read_dir(dir).unwrap().flatten() {
                    let _ = std::fs::copy(entry.path(), into.join(entry.file_name()));
                }
            }
            store::update_wizard(&conn, 6, "Nicolas Flamel", store::Rarity::Uncommon, "", false).unwrap();
            let mut seed: u64 = 0xf0_9f_90_b8;
            let mut roll = || {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (seed >> 11) as f64 / (1u64 << 53) as f64
            };
            let channels = [21u64, 21, 23, 22, 21, 23];
            for i in 0..34i64 {
                let at = now - (34 - i) * 2 * 3600 - (roll() * 1800.0) as i64;
                let wizard = store::pick_wizard(&conn, roll(), roll()).unwrap();
                let riddle = store::pick_riddle(&conn, wizard.rarity.difficulty(), roll()).unwrap();
                let channel = channels[(roll() * channels.len() as f64) as usize % channels.len()];
                let by = (i % 11 == 5).then_some(ADMIN);
                let d = store::start_drop(&conn, channel, &wizard, &riddle, at, by).unwrap();
                store::mark_open(&conn, d.id, 1_290_000_000_000_000_000 + d.id as u64, at, 300).unwrap();
                if roll() < 0.8 {
                    let who = if i % 4 == 0 { 2012 } else { 2000 + (roll() * 30.0) as u64 };
                    let name = ROSTER[(who - 2000) as usize];
                    if roll() < 0.5 {
                        let _ = store::submit(&mut conn, d.id, who + 1, "x", "definitely not it", at + 20);
                    }
                    let typed = riddle.answers[(roll() * riddle.answers.len() as f64) as usize % riddle.answers.len()].clone();
                    let res = store::submit(&mut conn, d.id, who, name, &typed, at + 12 + (roll() * 200.0) as i64).unwrap();
                    assert!(matches!(res, Submit::Won(_)), "{res:?}");
                } else {
                    store::escape(&conn, d.id).unwrap();
                }
                store::mark_finished(&conn, d.id).unwrap();
            }
            // One frog in chat right now.
            let wizard = store::wizard(&conn, 10).unwrap();
            let riddle = store::pick_riddle(&conn, "hard", 0.3).unwrap();
            let d = store::start_drop(&conn, 23, &wizard, &riddle, now - 70, None).unwrap();
            store::mark_open(&conn, d.id, 1_290_000_000_000_009_999, now - 70, 300).unwrap();
            store::set_retired(&conn, "wordplay-017", true).unwrap();
            // Earned cards: three mornings of tops and two royales.
            use super::super::super::frog_store::AwardFor;
            let acts = ["chat", "voice", "quiz", "koto", "wordle", "arena", "snitch", "frog"];
            for back in 1..=3i64 {
                let day = super::super::super::points::ist_day(now - back * 86_400);
                for (i, act) in acts.iter().enumerate() {
                    if (back as usize + i) % 5 == 4 {
                        continue;
                    }
                    let who = 2000 + ((back as u64 * 7 + i as u64 * 5) % 30);
                    let what = AwardFor { kind: "daily_top", day: day.clone(), activity: act.to_string(), total: 3 + ((i as i64 * 7 + back) % 11), ..Default::default() };
                    store::award_card(&mut conn, &format!("frogtop:{}:{}", day, act), who, &format!("daily_top:{}", act), &what, (roll(), roll()), now - back * 86_400 + 10 * 3600 + 86_400).unwrap();
                }
            }
            for (battle, champ, runner) in [(now - 30 * 3600, 2012u64, 2004u64), (now - 3 * 3600, 2007, 2012)] {
                for (role, user) in [("champion", champ), ("runner_up", runner)] {
                    let what = AwardFor { kind: "royale", battle_id: Some(battle), role: role.into(), ..Default::default() };
                    store::award_card(&mut conn, &format!("frogroyale:{}:{}", battle, role), user, &format!("royale_{}", role), &what, (roll(), roll()), battle).unwrap();
                }
            }
            // Trades in every state.
            use super::super::super::frog_trade as trade;
            let first = |user: u64, conn: &rusqlite::Connection| store::cards_of(conn, user).into_iter().map(|c| c.serial).collect::<Vec<_>>();
            let sameer = first(2012, &conn);
            let aarav = first(2000, &conn);
            let zoya = first(2007, &conn);
            let name = |u: u64| ROSTER[(u - 2000) as usize];
            if sameer.len() >= 3 && !zoya.is_empty() {
                let done = trade::create(&mut conn, (2012, name(2012)), (2007, name(2007)), Some(900), 21, &sameer[..2], &zoya[..1], now - 20 * 3600, 86_400).unwrap();
                trade::set_message(&conn, done.id, 1_290_000_000_000_100_001).unwrap();
                trade::accept(&mut conn, done.id, now - 19 * 3600).unwrap();
                let (z, s1) = (first(2007, &conn), first(2012, &conn));
                let open = trade::create(&mut conn, (2007, name(2007)), (2012, name(2012)), Some(900), 23, &z[..1], &s1[..1], now - 2 * 3600, 86_400).unwrap();
                trade::set_message(&conn, open.id, 1_290_000_000_000_100_002).unwrap();
            }
            let sameer_now = first(2012, &conn);
            if let (Some(a), Some(s2)) = (aarav.first(), sameer_now.last()) {
                let declined = trade::create(&mut conn, (2000, name(2000)), (2012, name(2012)), Some(900), 21, &[*a], &[*s2], now - 9 * 3600, 86_400).unwrap();
                trade::close(&conn, declined.id, "declined", Some(2012), now - 8 * 3600).unwrap();
                let gift = trade::create(&mut conn, (2000, name(2000)), (2004, name(2004)), Some(900), 22, &[*a], &[], now - 50 * 3600, 86_400).unwrap();
                trade::close(&conn, gift.id, "expired", None, now - 26 * 3600).unwrap();
            }
            // One full set sold by Siddharth.
            let seller = 2026u64;
            let in_play: Vec<i64> = store::wizards(&conn).into_iter().filter(|w| w.enabled).map(|w| w.id).collect();
            for w in in_play {
                if store::cards_of(&conn, seller).iter().any(|c| c.wizard_id == w) {
                    continue;
                }
                let what = AwardFor { kind: "royale", battle_id: Some(1), role: format!("demo-{w}"), ..Default::default() };
                store::award_card(&mut conn, &format!("demo-sale-{w}"), seller, "royale_champion", &what, (0.0, 0.0), now - 7 * 3600).unwrap();
                conn.execute(
                    "UPDATE cards SET edition = (SELECT COALESCE(MAX(edition), 0) + 1 FROM cards WHERE wizard_id = ?1), wizard_id = ?1 WHERE serial = (SELECT MAX(serial) FROM cards)",
                    rusqlite::params![w],
                )
                .unwrap();
                conn.execute("DELETE FROM awards WHERE key = ?1", rusqlite::params![format!("demo-sale-{w}")]).unwrap();
            }
            let plan: Vec<i64> = store::sale_plan(&conn, seller).unwrap().iter().map(|c| c.serial).collect();
            store::sell_set(&mut conn, seller, &plan, 35, now - 6 * 3600).unwrap().unwrap();
            store::meta_set(&conn, "demo_seeded", "1").unwrap();
        }
    }

    let session = session_for(ADMIN);
    let signed_in = demo_panel().layer(axum::middleware::map_request(move |mut req: axum::extract::Request| {
        let session = session.clone();
        async move {
            if !req.headers().contains_key("cookie") {
                req.headers_mut().insert("cookie", format!("mlci_panel={}", session).parse().unwrap());
            }
            req
        }
    }));
    let signed_in = signed_in.layer(axum::middleware::from_fn(ui_from_disk));
    let signed_out = demo_panel().layer(axum::middleware::from_fn(ui_from_disk));
    let secs: u64 = std::env::var("PANEL_DEMO_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    let a = tokio::net::TcpListener::bind("127.0.0.1:8799").await.unwrap();
    let b = tokio::net::TcpListener::bind("127.0.0.1:8798").await.unwrap();
    println!("demo panel: http://127.0.0.1:8799 (signed in), http://127.0.0.1:8798 (signed out) for {secs}s");
    let serve_a = axum::serve(a, signed_in.into_make_service_with_connect_info::<SocketAddr>());
    let serve_b = axum::serve(b, signed_out.into_make_service_with_connect_info::<SocketAddr>());
    tokio::select! {
        _ = async { serve_a.await } => {}
        _ = async { serve_b.await } => {}
        _ = tokio::time::sleep(std::time::Duration::from_secs(secs)) => {}
    }
}
