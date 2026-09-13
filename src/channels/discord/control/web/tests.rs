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
        PEOPLE
            .iter()
            .filter(|p| q.is_empty() || p.1.to_lowercase().contains(&q) || p.2.contains(&q))
            .take(limit)
            .map(person)
            .collect()
    }

    async fn member(&self, id: u64) -> Option<MemberInfo> {
        PEOPLE.iter().find(|p| p.0 == id).map(person)
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
}

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

static STORE: OnceLock<tempfile::TempDir> = OnceLock::new();

/// Opens control.db once for the whole test binary.
pub fn store() {
    STORE.get_or_init(|| {
        let dir = tempfile::tempdir().expect("temp dir");
        super::super::open(dir.path().to_str().unwrap()).expect("control store");
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

    // Spread today's changes over the last few hours.
    DB.get()
        .unwrap()
        .lock()
        .execute(
            "UPDATE audit SET ts = ?1 - ((SELECT MAX(id) FROM audit) - id + 1) * 1500 WHERE ts >= ?2",
            rusqlite::params![now, now - 60],
        )
        .unwrap();

    let session = session_for(ADMIN);
    let signed_in = panel().layer(axum::middleware::map_request(move |mut req: axum::extract::Request| {
        let session = session.clone();
        async move {
            if !req.headers().contains_key("cookie") {
                req.headers_mut().insert("cookie", format!("mlci_panel={}", session).parse().unwrap());
            }
            req
        }
    }));
    let signed_in = signed_in.layer(axum::middleware::from_fn(ui_from_disk));
    let signed_out = panel().layer(axum::middleware::from_fn(ui_from_disk));
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
