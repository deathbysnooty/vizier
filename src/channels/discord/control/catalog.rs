//! What the panel shows: every feature, its settings and its commands, with the
//! words an admin needs to use them. The panel draws itself from this, so a new
//! setting only has to be described here.

use serde::Serialize;

/// How a setting is edited.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Kind {
    /// On or off, stored as "on"/"off".
    Toggle,
    /// One text channel id.
    Channel,
    /// Comma-separated text channel ids.
    Channels,
    /// `id:weight` pairs, comma-separated (e.g. how often the Snitch picks a channel).
    WeightedChannels,
    /// A voice channel id.
    VoiceChannel,
    Role,
    /// Comma-separated member ids.
    Users,
    Number { min: i64, max: i64, unit: &'static str },
    Decimal { min: f64, max: f64, unit: &'static str },
    Text,
    /// A time of day in India time, "HH:MM".
    Time,
    /// One of fixed values: (value, label).
    Choice { options: &'static [(&'static str, &'static str)] },
}

#[derive(Clone, Debug, Serialize)]
pub struct Setting {
    /// The stored key; the same name the environment variable had.
    pub key: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    /// What applies when neither the panel nor the environment sets it.
    pub default: &'static str,
    /// False when a change only takes effect after the bot restarts.
    pub live: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Command {
    /// Without the slash.
    pub name: &'static str,
    /// "Everyone", "Admins", "House captains".
    pub who: &'static str,
    /// e.g. "/fight @someone type:"
    pub usage: &'static str,
    pub what: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct Section {
    pub id: &'static str,
    pub title: &'static str,
    pub icon: &'static str,
    /// What the feature does, in a few sentences.
    pub about: &'static str,
    pub settings: Vec<Setting>,
    pub commands: Vec<Command>,
}

// --- building blocks ----------------------------------------------------------

/// A setting that takes effect at once.
fn setting(key: &'static str, label: &'static str, help: &'static str, kind: Kind, default: &'static str) -> Setting {
    Setting { key, label, help, kind, default, live: true }
}

/// A setting only read when the bot starts.
fn at_start(key: &'static str, label: &'static str, help: &'static str, kind: Kind, default: &'static str) -> Setting {
    Setting { live: false, ..setting(key, label, help, kind, default) }
}

fn toggle(key: &'static str, label: &'static str, help: &'static str) -> Setting {
    setting(key, label, help, Kind::Toggle, "on")
}

fn number(min: i64, max: i64, unit: &'static str) -> Kind {
    Kind::Number { min, max, unit }
}

fn command(name: &'static str, who: &'static str, usage: &'static str, what: &'static str) -> Command {
    Command { name, who, usage, what }
}

const EVERYONE: &str = "Everyone";
const ADMINS: &str = "Bot admins";
const ROUNDS: &[(&str, &str)] = &[("semi", "Semi-finals"), ("quarter", "Quarter-finals"), ("r16", "Round of 16")];
const DAYS: &[(&str, &str)] = &[
    ("mon", "Monday"),
    ("tue", "Tuesday"),
    ("wed", "Wednesday"),
    ("thu", "Thursday"),
    ("fri", "Friday"),
    ("sat", "Saturday"),
    ("sun", "Sunday"),
];

// --- the sections ------------------------------------------------------------------

/// Every section, in the order the panel lists them.
pub fn sections() -> Vec<Section> {
    vec![
        Section {
            id: "bot",
            title: "Bot & channels",
            icon: "🤖",
            about: "Loduchand reads the channels on its allowlist and answers when someone @mentions it there; \
                    everything else in those channels is read quietly and remembered. DMs are answered for bot \
                    admins only. The games, points and letters below work in every channel whatever this list \
                    says. Who counts as a bot admin is set in the server's environment file, not here.",
            settings: vec![setting(
                "VIZIER_DISCORD_CHANNELS",
                "Chat channels",
                "Channels the bot chats in and counts messages from for /awards and chat points. Empty means every \
                 channel it can see. A newly added channel's older history is only counted after a restart.",
                Kind::Channels,
                "",
            )],
            commands: vec![
                command("help", EVERYONE, "/help", "Explains what the bot does and lists the chat commands. Only the asker sees it."),
                command("new", EVERYONE, "/new", "Starts a fresh conversation in this channel; the bot forgets the current thread."),
                command(
                    "session",
                    EVERYONE,
                    "/session topic_id:",
                    "Lists this channel's past conversations, or switches to one when a topic id is given (DEFAULT for the main one).",
                ),
                command("abort", EVERYONE, "/abort", "Stops the bot mid-answer when it is taking too long."),
                command("checkpoint", EVERYONE, "/checkpoint", "Saves a summary of the conversation so far and carries it into a clean context."),
                command("lobotomy", EVERYONE, "/lobotomy", "Clears the conversation without keeping a summary: a clean break."),
                command("thinking", EVERYONE, "/thinking", "Turns showing the bot's thinking in this channel on or off."),
                command("tool_calls", EVERYONE, "/tool_calls", "Turns showing the bot's background actions in this channel on or off."),
            ],
        },
        Section {
            id: "admin",
            title: "Admin controls",
            icon: "🛡️",
            about: "Switches for the bot as a whole. /stop silences it everywhere (slash commands still work so it \
                    can be brought back); admin-only mode keeps it reading and recording but answering nobody except \
                    admins. The quiz, arena, quotes and letters carry on in admin-only mode because they never reach \
                    the AI.",
            settings: vec![],
            commands: vec![
                command("stop", ADMINS, "/stop", "Pauses the bot everywhere until /resume: it reads, stores and answers nothing. The quiz holds its place."),
                command("resume", ADMINS, "/resume", "Brings the bot back after /stop."),
                command("adminonly", ADMINS, "/adminonly", "Turns admin-only mode on or off: the bot answers admins and quietly reads everyone else."),
                command("ping", EVERYONE, "/ping", "Replies Pong, to check the bot is alive."),
            ],
        },
        Section {
            id: "houses",
            title: "Houses",
            icon: "🏰",
            about: "Everyone is sorted into one of four houses (Gryffindor, Hufflepuff, Ravenclaw, Slytherin) and earns \
                    points for it from the games, the Snitch, the quiz, the arena, chat and voice. The house with the \
                    most points in a month wins: its captain and one random member with enough points get Nitro. \
                    Existing members were placed by the /housedraft; newcomers are sorted by the hat as they join \
                    (the card goes under the welcome message), and anyone who leaves and comes back keeps their house. \
                    Mods stay out of the houses. Members can step out with /houseopt and become Muggles. A bot admin \
                    replying `points 10 for something` to a message gives that member's house the points.",
            settings: vec![
                setting(
                    "VIZIER_HOUSE_CHANNEL",
                    "Houses channel",
                    "Where the hourly points summary and the draft's sorting cards go. Empty turns the summary off.",
                    Kind::Channel,
                    "",
                ),
                setting(
                    "VIZIER_MUGGLE_ROLE",
                    "Muggles role",
                    "The role worn by members who stepped out with /houseopt. Empty means no role is given.",
                    Kind::Role,
                    "",
                ),
                toggle(
                    "VIZIER_HOUSE_SORT_NEW",
                    "Sort newcomers",
                    "Sort new members into a house as they join. Off leaves them unsorted until a mod uses /sort; returners still get their house back.",
                ),
            ],
            commands: vec![
                command("houses", EVERYONE, "/houses", "The four houses with this month's points, member counts and captains."),
                command("houselist", EVERYONE, "/houselist house:", "Who is in a house, a page at a time. Only the asker sees it."),
                command(
                    "housetop",
                    EVERYONE,
                    "/housetop house: period:",
                    "A house's top 10 scorers this month, last month or all time (your own house if none is picked). Only the asker sees it.",
                ),
                command(
                    "today",
                    EVERYONE,
                    "/today member:",
                    "Your points today, with chat and voice progress and which daily game limits are maxed out; add a member of your own house to see theirs (admins can check anyone). Private.",
                ),
                command("mypoints", EVERYONE, "/mypoints", "Your points this month, where they came from, and how far you are from the Nitro draw. Private."),
                command(
                    "houseopt",
                    EVERYONE,
                    "/houseopt",
                    "Steps out of the houses (house role removed, Muggles role given, no points), or back into your old house.",
                ),
                command(
                    "housepoints",
                    ADMINS,
                    "/housepoints house: points: reason:",
                    "Gives a house points, or takes them away with a negative number. Posted publicly.",
                ),
                command("housecaptain", ADMINS, "/housecaptain who:", "Makes a member captain of the house they are in, replacing the old captain."),
                command("sort", ADMINS, "/sort who: house:", "Puts a member into a house by hand, or moves them."),
                command(
                    "housedraft",
                    ADMINS,
                    "/housedraft",
                    "Works out an even split of everyone not yet in a house and shows the plan with Sort / Redraw / Cancel buttons. Sorting also turns on newcomer sorting.",
                ),
                command("houseroles", ADMINS, "/houseroles", "Creates the four house roles with their crests, or fixes them."),
                command("housechannels", ADMINS, "/housechannels", "Creates a private common room channel for each house."),
                command(
                    "housedraw",
                    ADMINS,
                    "/housedraw redraw:",
                    "Last month's winning house and its random Nitro winner. The result is remembered; redraw picks again and says so.",
                ),
            ],
        },
        Section {
            id: "points",
            title: "Points & limits",
            icon: "🏆",
            about: "Every house point is written to one ledger, which applies the limits below at the moment of \
                    writing: a game only says what someone earned, and the ledger decides how much of it still fits \
                    under today's limit. Limits count per person per India day (midnight to midnight). Battle royales, \
                    the Golden Snitch and points given by mods have no limit. What each game pays is set in its own \
                    section.",
            settings: vec![
                setting("VIZIER_CAP_CHAT", "Chat points a day", "Most points one person can earn from chat days in a day.", number(0, 100, "points"), "1"),
                setting("VIZIER_CAP_VOICE", "Voice points a day", "Most points one person can earn from voice days in a day.", number(0, 100, "points"), "1"),
                setting("VIZIER_CAP_QUIZ", "Quiz points a day", "Most points one person can earn from quiz rounds in a day.", number(0, 100, "points"), "6"),
                setting("VIZIER_CAP_KOTO", "Koto points a day", "Most points one person can earn from Koto in a day.", number(0, 100, "points"), "4"),
                setting("VIZIER_CAP_ANAGRAM", "Anagram points a day", "Most points one person can earn from Anagram Bot in a day.", number(0, 100, "points"), "6"),
                setting("VIZIER_CAP_CAT", "Cat Bot points a day", "Most points one person can earn from Cat Bot catches in a day.", number(0, 100, "points"), "3"),
                setting("VIZIER_CAP_ARENA", "Arena points a day", "Most points one person can earn from 1v1 fights in a day.", number(0, 100, "points"), "3"),
                setting(
                    "VIZIER_CAP_SNITCH",
                    "Snitch points a day",
                    "Most points one person can earn from Bronze and Silver Snitches in a day. The Golden Snitch is never limited.",
                    number(0, 100, "points"),
                    "6",
                ),
                setting(
                    "VIZIER_CAP_WEEKLY",
                    "Weekly scan points",
                    "Most points one person can earn from the weekly scan per channel per week (Monday to Sunday).",
                    number(0, 100, "points"),
                    "3",
                ),
                setting(
                    "VIZIER_DRAW_MINIMUM",
                    "Nitro draw minimum",
                    "Points a member needs in the month to be in the winning house's random Nitro draw.",
                    number(0, 1000, "points"),
                    "10",
                ),
            ],
            commands: vec![],
        },
        Section {
            id: "snitch",
            title: "Snitch",
            icon: "✨",
            about: "A few times a day a Snitch card flies into a busy chat channel. The first three people to reply to \
                    it with exactly `accio` score points for their house; after the catch window the card turns grey \
                    and nothing counts. Drops only land in a channel someone spoke in recently, at random India times \
                    inside the drop hours, spaced apart. Most are Bronze, some Silver and a rare Golden one pays the \
                    most and ignores the daily limit. A Snitch nobody catches gets a second chance a little later. \
                    Misses get no reply, so a busy catch doesn't flood the channel. A day's plan is made once, so \
                    changes to drops, hours and gap apply from the next day's plan.",
            settings: vec![
                toggle("VIZIER_SNITCH", "Scheduled drops", "Drop Snitches on their own through the day. Off stops new drops; /snitchdrop still works."),
                setting(
                    "VIZIER_SNITCH_CHANNELS",
                    "Drop channels",
                    "Channels a Snitch can drop into, each with a weight for how often (3 and 1 means three drops in four go to the first). The first is the home channel a drop falls back to when its pick is quiet. Empty means no scheduled drops.",
                    Kind::WeightedChannels,
                    "",
                ),
                setting("VIZIER_SNITCH_DROPS_MIN", "Fewest drops a day", "The least scheduled drops in a day; each day picks at random between this and the most.", number(0, 20, "drops"), "3"),
                setting("VIZIER_SNITCH_DROPS_MAX", "Most drops a day", "The most scheduled drops in a day.", number(0, 20, "drops"), "4"),
                setting("VIZIER_SNITCH_START_HOUR", "Drops from", "India hour the drop hours start (10 is 10:00).", number(0, 23, "hour"), "10"),
                setting(
                    "VIZIER_SNITCH_END_HOUR",
                    "Drops until",
                    "India hour the drop hours end (24 is midnight). The last drop lands one catch window before it, so its points count on the same day.",
                    number(1, 24, "hour"),
                    "24",
                ),
                setting("VIZIER_SNITCH_GAP_MINUTES", "Gap between drops", "The least time between two scheduled drops.", number(1, 1440, "minutes"), "120"),
                setting("VIZIER_SNITCH_CATCH_SECS", "Catch window", "How long a Snitch can be caught before it flies away. A card already in the air keeps its own window.", number(10, 3600, "seconds"), "120"),
                setting("VIZIER_SNITCH_QUIET_MINUTES", "Quiet after", "A channel counts as busy if someone spoke in it this recently; drops only go to busy channels.", number(1, 120, "minutes"), "5"),
                toggle("VIZIER_SNITCH_SECOND_CHANCE", "Second chances", "Send another Snitch a little after one flies away uncaught."),
                setting("VIZIER_SNITCH_SECOND_CHANCE_MIN_MINUTES", "Second chance after (least)", "The soonest a second-chance Snitch comes after an uncaught one flew.", number(1, 720, "minutes"), "20"),
                setting("VIZIER_SNITCH_SECOND_CHANCE_MAX_MINUTES", "Second chance after (most)", "The latest a second-chance Snitch comes after an uncaught one flew.", number(1, 720, "minutes"), "40"),
                setting("VIZIER_SNITCH_SECOND_CHANCES", "Second chances a day", "How many second chances a day can have.", number(0, 20, "drops"), "3"),
                setting("VIZIER_SNITCH_GOLDEN_ONE_IN", "Golden: 1 in", "About one drop in this many is Golden. 0 means never.", number(0, 1000, "drops"), "12"),
                setting("VIZIER_SNITCH_SILVER_ONE_IN", "Silver: 1 in", "About one drop in this many is Silver. 0 means never; the rest are Bronze.", number(0, 1000, "drops"), "4"),
                setting("VIZIER_SNITCH_BRONZE_1ST", "Bronze 1st", "Points for the first catcher of a Bronze Snitch.", number(0, 100, "points"), "2"),
                setting("VIZIER_SNITCH_BRONZE_2ND", "Bronze 2nd", "Points for the second catcher of a Bronze Snitch.", number(0, 100, "points"), "1"),
                setting("VIZIER_SNITCH_BRONZE_3RD", "Bronze 3rd", "Points for the third catcher of a Bronze Snitch.", number(0, 100, "points"), "1"),
                setting("VIZIER_SNITCH_SILVER_1ST", "Silver 1st", "Points for the first catcher of a Silver Snitch.", number(0, 100, "points"), "3"),
                setting("VIZIER_SNITCH_SILVER_2ND", "Silver 2nd", "Points for the second catcher of a Silver Snitch.", number(0, 100, "points"), "2"),
                setting("VIZIER_SNITCH_SILVER_3RD", "Silver 3rd", "Points for the third catcher of a Silver Snitch.", number(0, 100, "points"), "1"),
                setting("VIZIER_SNITCH_GOLDEN_1ST", "Golden 1st", "Points for the first catcher of a Golden Snitch (never limited).", number(0, 100, "points"), "6"),
                setting("VIZIER_SNITCH_GOLDEN_2ND", "Golden 2nd", "Points for the second catcher of a Golden Snitch.", number(0, 100, "points"), "4"),
                setting("VIZIER_SNITCH_GOLDEN_3RD", "Golden 3rd", "Points for the third catcher of a Golden Snitch.", number(0, 100, "points"), "2"),
            ],
            commands: vec![command(
                "snitchdrop",
                ADMINS,
                "/snitchdrop type:",
                "Drops a Snitch (Bronze, Silver, Golden or random) in this channel right now. It scores like any other but doesn't use up a scheduled drop.",
            )],
        },
        Section {
            id: "summary",
            title: "Hourly summary",
            icon: "📊",
            about: "At the turn of each India hour the bot posts which houses gained or lost points in the hour just \
                    gone and from what, with this month's standings underneath. It posts in the houses channel \
                    (set under Houses), only when points actually moved, and only for the hours below.",
            settings: vec![
                toggle("VIZIER_HOUSE_SUMMARY", "Hourly summary", "Post the hourly points summary."),
                setting(
                    "VIZIER_HOUSE_SUMMARY_FIRST_HOUR",
                    "First hour",
                    "The first India hour summarised: 10 covers 10:00-11:00 and is posted at 11:00.",
                    number(0, 23, "hour"),
                    "10",
                ),
                setting(
                    "VIZIER_HOUSE_SUMMARY_LAST_HOUR",
                    "Last hour",
                    "The last India hour summarised: 23 covers 23:00-midnight and is posted at midnight.",
                    number(0, 23, "hour"),
                    "23",
                ),
            ],
            commands: vec![],
        },
        Section {
            id: "weekly",
            title: "Weekly scan",
            icon: "📝",
            about: "Once a week the bot reads the last seven days of the discussion channels, has the AI pick out \
                    genuinely valuable posts (names are hidden from it), checks those picks a second time against \
                    the quoted messages, and DMs what survives to the reviewers. Nothing is awarded or posted until a \
                    reviewer approves lines with the buttons in that DM; approved points go quietly into the ledger, \
                    limited per channel per week. A missed run (the bot was down) still happens within the catch-up \
                    time.",
            settings: vec![
                toggle("VIZIER_WEEKLY", "Scheduled scan", "Run the scan every week on its own. Off stops it; /weeklyscan still works."),
                setting(
                    "VIZIER_WEEKLY_CHANNELS",
                    "Channels read",
                    "The discussion channels scanned. Empty means the eight picked by the owner.",
                    Kind::Channels,
                    "",
                ),
                setting(
                    "VIZIER_WEEKLY_REVIEWERS",
                    "Reviewers",
                    "Who gets the approval DM and may press its buttons. Empty means the quiz reviewers, or the bot admins if those are empty too.",
                    Kind::Users,
                    "",
                ),
                setting("VIZIER_WEEKLY_DAY", "Day", "The day of the week the scan runs, India time.", Kind::Choice { options: DAYS }, "sun"),
                setting("VIZIER_WEEKLY_TIME", "Time", "The India time the scan runs; it covers the seven days before it.", Kind::Time, "20:00"),
                setting(
                    "VIZIER_WEEKLY_CATCH_UP_HOURS",
                    "Catch-up time",
                    "How long after its time a missed scan may still run. Past this the week is skipped.",
                    number(0, 144, "hours"),
                    "36",
                ),
            ],
            commands: vec![command(
                "weeklyscan",
                ADMINS,
                "/weeklyscan",
                "Runs the scan over the last 7 days now; results go to the reviewers by DM for approval.",
            )],
        },
        Section {
            id: "games",
            title: "Game bots",
            icon: "🎮",
            about: "Koto, Anagram Bot and Cat Bot already run on the server and keep their own scores. The bot watches \
                    what they post and pays the winner's house: Koto's solver and everyone else who guessed, the \
                    anagram solver, and whoever caught a cat (rarer cats pay more). Members just play those games as \
                    usual. Each game, restart or edit can only pay once, and the daily limits apply.",
            settings: vec![
                toggle("VIZIER_GAME_POINTS", "Pay for game wins", "Give house points for Koto, Anagram Bot and Cat Bot wins. Off pays nothing; the games themselves carry on."),
                setting("VIZIER_POINTS_KOTO_WIN", "Koto solved", "Points for solving a Koto game.", number(0, 100, "points"), "3"),
                setting("VIZIER_POINTS_KOTO_PLAYED", "Koto played", "Points for each other player who guessed in a solved Koto game.", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_ANAGRAM", "Anagram solved", "Points for solving an Anagram Bot puzzle.", number(0, 100, "points"), "3"),
                setting("VIZIER_POINTS_CAT_COMMON", "Common cat", "Points for catching a Fine, Nice, Good, Gremlin or unknown cat.", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_CAT_RARE", "Rare cat", "Points for catching a Rare, Sus, Rickroll or Wild cat.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_CAT_TOP", "Top cat", "Points for catching a Superior, Mythic or Legendary cat.", number(0, 100, "points"), "3"),
            ],
            commands: vec![],
        },
        Section {
            id: "activity",
            title: "Chat & voice",
            icon: "🎙️",
            about: "Turning up earns a point: someone who sends enough messages in an India day, or spends long enough \
                    in voice, gets their house a point for each, once a day. The counts are the same ones behind \
                    /awards, checked every quarter hour. Voice time comes from Dyno's voice log channel, so private \
                    rooms count too; time in the AFK room or an excluded channel never does.",
            settings: vec![
                toggle("VIZIER_ACTIVITY_POINTS", "Chat and voice points", "Give the daily chat and voice points. Off pays nothing; a day earned meanwhile is paid if it's switched back on before the next day ends."),
                setting("VIZIER_CHAT_DAY_MESSAGES", "Messages for a chat day", "Messages in one India day that earn the chat point.", number(1, 1000, "messages"), "20"),
                setting("VIZIER_VOICE_DAY_MINUTES", "Minutes for a voice day", "Minutes in voice in one India day that earn the voice point.", number(1, 1440, "minutes"), "60"),
                at_start(
                    "VIZIER_VOICE_LOG_CHANNEL",
                    "Voice log channel",
                    "Dyno's voice log, where joins and leaves are read from. Empty means voice time is not recorded.",
                    Kind::Channel,
                    "",
                ),
                setting(
                    "VIZIER_VOICE_AFK_CHANNEL",
                    "AFK room",
                    "The voice channel whose time never counts, for points or /awards. Empty means none.",
                    Kind::VoiceChannel,
                    "",
                ),
                setting(
                    "VIZIER_STATS_EXCLUDE_CHANNELS",
                    "Excluded channels",
                    "Text or voice channels left out of chat points, voice points, /awards and the house draft's activity count. Empty leaves nothing out.",
                    Kind::Channels,
                    "",
                ),
            ],
            commands: vec![],
        },
        Section {
            id: "quiz",
            title: "Quiz",
            icon: "🧠",
            about: "A never-ending quiz in its own channel, started with /quiz. One question at a time; the first right \
                    answer scores (small spelling slips are fine), and multiple choice gives each person one pick. \
                    Players type `!hint` for help and `!skip` to vote past a question (one admin's skip is enough). \
                    Questions come in rounds: each round's top scorer is crowned that genre's champion, the top three \
                    earn house points, then everyone votes on the next round's genre. Members send questions with \
                    /quizadd and reviewers approve them by DM. Every Monday the bot can also make questions from the \
                    week's Bollywood headlines, again approved by DM first. It keeps running across restarts until \
                    /quizstop.",
            settings: vec![
                setting(
                    "VIZIER_QUIZ_CHANNEL",
                    "Quiz channel",
                    "The only channel the quiz runs in; everything said there belongs to the quiz. Change it while the quiz is stopped. Empty means no quiz.",
                    Kind::Channel,
                    "",
                ),
                setting(
                    "VIZIER_QUIZ_REVIEWERS",
                    "Reviewers",
                    "Who approves member and news questions by DM. Empty means the bot admins.",
                    Kind::Users,
                    "",
                ),
                setting("VIZIER_QUIZ_ROUND_QUESTIONS", "Questions a round", "Questions per round before the scores are settled and the genre vote.", number(1, 1000, "questions"), "20"),
                setting("VIZIER_QUIZ_SKIPS", "Skip votes needed", "Different members typing !skip it takes to pass a question.", number(1, 50, "votes"), "3"),
                setting("VIZIER_QUIZ_WRONG_WAIT_SECS", "Wait after a wrong pick", "How long someone waits to pick again after a wrong multiple-choice answer.", number(0, 600, "seconds"), "20"),
                setting("VIZIER_QUIZ_VOTE_SECS", "Genre vote time", "How long the genre vote stays open between rounds.", number(10, 600, "seconds"), "60"),
                setting("VIZIER_QUIZ_MAX_PENDING", "Questions waiting per member", "How many /quizadd questions one member may have waiting for approval.", number(0, 100, "questions"), "5"),
                setting(
                    "VIZIER_QUIZ_INDIA_SHARE",
                    "India share",
                    "The share of mixed-round questions from the India pool, from 0 (none) to 1 (all).",
                    Kind::Decimal { min: 0.0, max: 1.0, unit: "" },
                    "0.7",
                ),
                setting("VIZIER_POINTS_QUIZ_1ST", "Round winner", "House points for the top scorer of a round.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_QUIZ_2ND", "Round second", "House points for second place in a round.", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_QUIZ_3RD", "Round third", "House points for third place in a round.", number(0, 100, "points"), "1"),
                toggle("VIZIER_QUIZ_NEWS", "Weekly news questions", "Make questions from the week's entertainment news every Monday. Off stops it; /quiznews still works."),
                setting("VIZIER_QUIZ_NEWS_HOUR", "News questions at", "The India hour on Monday the news questions are made.", number(0, 23, "hour"), "11"),
                setting(
                    "VIZIER_QUIZ_NEWS_FEEDS",
                    "News feeds",
                    "RSS feed addresses to read headlines from, comma-separated. Empty means the built-in Bollywood feeds.",
                    Kind::Text,
                    "",
                ),
            ],
            commands: vec![
                command("quiz", EVERYONE, "/quiz", "Starts the quiz, in the quiz channel only."),
                command("quizleaderboard", EVERYONE, "/quizleaderboard", "Top quiz scorers, all time or this week, and the genre champions."),
                command(
                    "quizadd",
                    EVERYONE,
                    "/quizadd question: answer: also: wrong1: wrong2: wrong3:",
                    "Sends in a question. Give three wrong options for multiple choice, none for a typed one; `also` lists other answers that count. A reviewer approves it first.",
                ),
                command("quizstop", ADMINS, "/quizstop", "Stops the quiz; it stays off, restarts included, until someone runs /quiz."),
                command("quiznews", ADMINS, "/quiznews", "Makes this week's news questions now and DMs them to the reviewers."),
            ],
        },
        Section {
            id: "arena",
            title: "Arena",
            icon: "⚔️",
            about: "1v1 fights and battle royales, just for fun. /fight challenges someone; if they accept, both pick \
                    △ ○ □ ✕ once at the start and the winner of that clash wins the fight, which then plays out. \
                    Winning a 1v1 earns a house point (only the first fight between the same two people each day \
                    counts). /battle opens a lobby, pings the Warrior role, and knocks the joiners out on a bracket \
                    until one champion is left, who wears the Battle Champion role and wins house points, as does the \
                    runner-up. Early rounds of a big battle are quick rounds posted as a list; later rounds are fought \
                    out in full. A daily battle can open by itself at a set time, tagging the houses.",
            settings: vec![
                setting(
                    "VIZIER_FIGHT_CHANNEL",
                    "Fight channel",
                    "Where challenges, fights and battles are posted. Empty means a channel named fight-fight-fight, or wherever the command was used.",
                    Kind::Channel,
                    "",
                ),
                setting(
                    "VIZIER_BATTLE_DAILY",
                    "Daily battle royale",
                    "Open a battle royale by itself every day at the time below, exactly as /battle does. Needs the fight channel set.",
                    Kind::Toggle,
                    "off",
                ),
                setting(
                    "VIZIER_BATTLE_DAILY_TIME",
                    "Daily battle time",
                    "India time the daily lobby opens. If a fight is running then, it opens as soon as the arena is free (up to 30 minutes late).",
                    Kind::Time,
                    "21:00",
                ),
                setting("VIZIER_BATTLE_DAILY_MINUTES", "Daily lobby length", "How long the daily lobby stays open for joining.", number(1, 60, "minutes"), "10"),
                setting(
                    "VIZIER_BATTLE_DAILY_THEME",
                    "Daily battle style",
                    "The fight type for the daily battle, or a random one each day.",
                    Kind::Choice {
                        options: &[
                            ("random", "Random each day"),
                            ("classic", "Classic"),
                            ("pokemon", "Pokémon"),
                            ("harrypotter", "Harry Potter"),
                            ("dbz", "Dragon Ball Z"),
                            ("wwe", "WWE"),
                            ("cs2", "Counter-Strike 2"),
                            ("eldenring", "Elden Ring"),
                        ],
                    },
                    "classic",
                ),
                setting(
                    "VIZIER_BATTLE_DAILY_PING",
                    "Daily battle tags",
                    "Who the daily lobby tags. Houses tags all four house roles (the bot needs permission to mention roles); Warriors tags the Warrior role.",
                    Kind::Choice {
                        options: &[("houses", "All 4 houses"), ("warriors", "Warrior role"), ("everyone", "@everyone"), ("none", "Nobody")],
                    },
                    "houses",
                ),
                setting("VIZIER_FIGHT_COOLDOWN_SECS", "Challenge cooldown", "How long a member waits between two /fight challenges.", number(0, 3600, "seconds"), "60"),
                setting("VIZIER_FIGHT_EXPIRY_SECS", "Challenge expiry", "How long a challenge waits for an answer before it is cancelled.", number(10, 3600, "seconds"), "120"),
                setting("VIZIER_FIGHT_PICK_SECS", "Move pick time", "How long both fighters have to pick a move each turn before the bot picks for them.", number(3, 120, "seconds"), "15"),
                setting("VIZIER_POINTS_ARENA_WIN", "1v1 win", "House points for winning a 1v1 challenge.", number(0, 100, "points"), "1"),
                setting("VIZIER_BATTLE_MIN_PLAYERS", "Fewest fighters", "A battle with fewer joiners than this is called off.", number(2, 64, "players"), "4"),
                setting(
                    "VIZIER_BATTLE_FULL_FIGHTS_FROM",
                    "Full fights from",
                    "The round from which battle fights are played out in full; bigger rounds before it are quick rounds posted as a list.",
                    Kind::Choice { options: ROUNDS },
                    "quarter",
                ),
                setting("VIZIER_BATTLE_LOBBY_MINUTES", "Lobby time", "How long a battle lobby stays open when /battle is run without minutes.", number(1, 60, "minutes"), "5"),
                at_start(
                    "VIZIER_BATTLE_LOBBY_MAX_MINUTES",
                    "Longest lobby",
                    "The most minutes /battle accepts. Lowering it works at once; Discord's own limit on the option only moves after a restart.",
                    number(1, 60, "minutes"),
                    "15",
                ),
                setting("VIZIER_BATTLE_LOBBY_NAMES", "Names in the lobby", "Joiners listed on the lobby card before it just says how many more.", number(1, 200, "names"), "40"),
                setting("VIZIER_POINTS_ROYALE_CHAMPION", "Battle champion", "House points for winning a battle royale (never limited).", number(0, 1000, "points"), "8"),
                setting("VIZIER_POINTS_ROYALE_RUNNER_UP", "Battle runner-up", "House points for losing the battle royale final.", number(0, 1000, "points"), "3"),
            ],
            commands: vec![
                command("fight", EVERYONE, "/fight who: type:", "Challenges someone to a 1v1 in the fight channel, in an optional fight style."),
                command("battle", ADMINS, "/battle minutes: type:", "Opens a battle royale lobby for that many minutes and pings the Warrior role."),
                command("warrior", EVERYONE, "/warrior", "Gets or drops the Warrior role, which is pinged when a battle opens."),
                command("fightboard", EVERYONE, "/fightboard", "Who has won the most fights, with battle crowns, and your own record."),
                command("battlestop", ADMINS, "/battlestop", "Clears a battle or fight that got stuck, so /battle works again."),
            ],
        },
        Section {
            id: "kalesh",
            title: "Kalesh detector",
            icon: "🍿",
            about: "Watches chosen channels for a real argument. First a free check on the shape of the chat - a fast \
                    burst from only a few people, full of replies - and only when that trips, the AI is asked \
                    whether it is a genuine fight or just loud banter. If it says fight, the bot pings the kalesh \
                    role with a playful line, then waits out the cooldown before it can ping again in that channel. \
                    It works even while the bot is in admin-only mode.",
            settings: vec![
                toggle("VIZIER_KALESH", "Kalesh detector", "Watch for fights and ping the role. Off stops it entirely."),
                setting("VIZIER_KALESH_CHANNELS", "Watched channels", "Channels watched for fights. Empty means none.", Kind::Channels, ""),
                setting("VIZIER_KALESH_ROLE_ID", "Kalesh role", "The role pinged when a fight is found. Empty means nobody is pinged.", Kind::Role, ""),
                setting("VIZIER_KALESH_WINDOW_SECS", "Burst window", "How far back the burst of messages is looked at.", number(10, 3600, "seconds"), "90"),
                setting("VIZIER_KALESH_MIN_MSGS", "Messages in a burst", "Messages within the window before it looks like a fight.", number(2, 80, "messages"), "10"),
                setting("VIZIER_KALESH_MAX_AUTHORS", "Most people", "More people than this talking in the window reads as a crowd, not a fight.", number(2, 50, "people"), "4"),
                setting("VIZIER_KALESH_MIN_REPLIES", "Replies needed", "Replies to other messages within the window before it looks like a fight.", number(0, 80, "replies"), "4"),
                setting("VIZIER_KALESH_COOLDOWN_SECS", "Cooldown", "After a check in a channel, how long before it can check (and ping) there again.", number(0, 86400, "seconds"), "900"),
                setting(
                    "VIZIER_KALESH_MODEL",
                    "AI model",
                    "The OpenRouter model that judges fight or banter. Empty means google/gemini-2.5-flash-lite.",
                    Kind::Text,
                    "google/gemini-2.5-flash-lite",
                ),
            ],
            commands: vec![],
        },
        Section {
            id: "letters",
            title: "Anonymous letters",
            icon: "📬",
            about: "Members send each other anonymous letters with /letter. A notice goes up in the letters channel \
                    naming only the recipient; only they can open it, once, and they can reply anonymously, which \
                    carries on in a thread under the notice. Once everything in an exchange is read, the notice and \
                    its thread are deleted after a pause. Members can refuse letters with /nochitthi. No AI is \
                    involved, and admins can look up who sent a letter.",
            settings: vec![
                toggle("VIZIER_LETTERS", "Anonymous letters", "Allow sending and replying to letters. Off makes /letter say letters aren't available; letters already sent can still be opened."),
                setting("VIZIER_LETTERS_CHANNEL", "Letters channel", "Where letter notices are posted. Empty means letters are off.", Kind::Channel, ""),
                setting("VIZIER_LETTERS_PER_HOUR", "Letters an hour", "How many letters and replies one member can send in an hour.", number(1, 1_000_000, "letters"), "5"),
                setting(
                    "VIZIER_LETTER_LOG_CHANNEL",
                    "Moderator log",
                    "A private channel where every letter is logged with both names. Keep it mods-only. Empty means no log.",
                    Kind::Channel,
                    "",
                ),
                setting("VIZIER_LETTER_CLEANUP_SECS", "Clean-up delay", "How long after the last letter in an exchange is opened before its notice and thread are deleted.", number(0, 86400, "seconds"), "120"),
            ],
            commands: vec![
                command("letter", EVERYONE, "/letter recipient: message:", "Sends someone an anonymous letter of up to 1500 characters."),
                command("letterbox", EVERYONE, "/letterbox", "Your unopened letters with buttons to open them, and recent read ones to reply to. Private."),
                command("nochitthi", EVERYONE, "/nochitthi", "Stops or resumes anonymous letters coming to you."),
                command("letter_trace", ADMINS, "/letter_trace letter_id:", "Shows who sent a letter, to whom, when, and what it said. Private, and logged."),
            ],
        },
        Section {
            id: "welcome",
            title: "Welcome & join history",
            icon: "👋",
            about: "When someone joins, the bot greets them in the welcome channel - with a ribbing count for anyone \
                    who keeps leaving and coming back - and the house sorting card follows underneath. Every join \
                    and leave is recorded quietly (leaving is never announced), and accounts that belong to one person \
                    can be merged so their history adds up.",
            settings: vec![
                toggle("VIZIER_WELCOME", "Welcome messages", "Greet people who join. Off posts no greeting; newcomers are still sorted into a house there."),
                setting(
                    "VIZIER_WELCOME_CHANNEL",
                    "Welcome channel",
                    "Where greetings and newcomers' sorting cards go. Empty means no greeting and no newcomer sorting.",
                    Kind::Channel,
                    "",
                ),
            ],
            commands: vec![
                command("rejoinstats", EVERYONE, "/rejoinstats", "The top 15 people who keep leaving and coming back, with their counts."),
                command("samebanda", ADMINS, "/samebanda purana: naya:", "Merges an old account's join history into the account the person uses now."),
            ],
        },
        Section {
            id: "quotes",
            title: "Quotes & awards",
            icon: "💬",
            about: "Anyone can turn a message into a quote card: right-click it and choose Apps → Quote, or reply to it \
                    with `@Loduchand quote this`. The quoter picks a style and presses Save, and the card goes to the \
                    quotes channel (one left unsaved is saved after 15 minutes). /awards draws the server's awards \
                    card - most messages, most voice time and more - for all time or last week.",
            settings: vec![setting(
                "VIZIER_QUOTES_CHANNEL",
                "Quotes channel",
                "Where saved quote cards are posted. Empty means the channel with gyaan in its name.",
                Kind::Channel,
                "",
            )],
            commands: vec![
                command("Quote", EVERYONE, "Right-click a message → Apps → Quote", "Makes a quote card of the message to style and save."),
                command("awards", EVERYONE, "/awards", "The server's awards card, all time, with a menu to switch to last week."),
            ],
        },
        Section {
            id: "reminders",
            title: "Reminders",
            icon: "⏰",
            about: "Messages the bot posts on a schedule, made on the Reminders page: every so many minutes (lined up on \
                    the India clock, so every 60 is on the hour) or at set India times each day, optionally only \
                    between two times. Each has one or more lines, posted in turn or at random, with {name}, \
                    {mention}, {hours} and {days} filled in. A reminder about a member can stop itself when they come \
                    back to the server, posting a welcome line. A slot is posted once, only while it's fresh - after \
                    downtime the bot doesn't catch up on missed ones - and a reminder with an end date switches itself \
                    off after it.",
            settings: vec![],
            commands: vec![],
        },
        Section {
            id: "members",
            title: "Member notes",
            icon: "🗒️",
            about: "Private notes the mods keep about members on the Members page, with a tone for each (gentle, \
                    light teasing, roast freely, respectful, brief). Whenever the bot answers a message, the notes for \
                    its author, anyone mentioned in it and whoever it replies to are handed to the AI with an \
                    instruction never to reveal them, so the bot treats people the way the mods intend. Notes are \
                    never used on messages the bot only reads quietly.",
            settings: vec![
                toggle(
                    "VIZIER_MEMBER_NOTES",
                    "Use member notes",
                    "Give the AI the mods' notes and tones when it answers. Off keeps the notes on the panel but the bot stops using them.",
                ),
                // Activity tiers on the Members page, over the last 30 days. Reaching any one bar is enough.
                setting("VIZIER_ACTIVE_VERY_MESSAGES", "Very active: messages", "Messages in 30 days that make someone very active on their own. 600 is about 20 a day, a chat point every day.", number(1, 100_000, "messages"), "600"),
                setting("VIZIER_ACTIVE_VERY_VOICE_MINUTES", "Very active: voice minutes", "Minutes in voice in 30 days that make someone very active. 600 is 10 hours.", number(1, 43_200, "minutes"), "600"),
                setting("VIZIER_ACTIVE_VERY_POINTS", "Very active: game points", "Game and activity points in 30 days (mods' and weekly awards not counted) that make someone very active.", number(1, 10_000, "points"), "60"),
                setting("VIZIER_ACTIVE_FAIR_MESSAGES", "Fairly active: messages", "Messages in 30 days that make someone fairly active. 150 is about 5 a day.", number(1, 100_000, "messages"), "150"),
                setting("VIZIER_ACTIVE_FAIR_VOICE_MINUTES", "Fairly active: voice minutes", "Minutes in voice in 30 days that make someone fairly active. 180 is 3 hours.", number(1, 43_200, "minutes"), "180"),
                setting("VIZIER_ACTIVE_FAIR_POINTS", "Fairly active: game points", "Game and activity points in 30 days that make someone fairly active.", number(1, 10_000, "points"), "20"),
            ],
            commands: vec![],
        },
        Section {
            id: "autoreplies",
            title: "Auto-responses",
            icon: "💬",
            about: "Rules made on the Auto-responses page that react to or answer messages containing set words or \
                    phrases: anywhere in the message, as a whole word, the exact message, how it starts, or a pattern. \
                    Each rule can add emoji reactions and/or reply with one of several messages picked at random \
                    ({user} mentions the author, {name} is their name), work in chosen channels only, wait a cooldown \
                    before firing again in the same channel, and fire only a percentage of the time. Bots are ignored, \
                    as are quiz answers and Snitch catches.",
            settings: vec![toggle(
                "VIZIER_AUTOREPLIES",
                "Auto-responses on",
                "Master switch for every auto-response rule. Off stops them all at once without deleting any.",
            )],
            commands: vec![],
        },
    ]
}

/// Every key the catalog describes.
pub fn keys() -> Vec<&'static str> {
    sections().iter().flat_map(|s| s.settings.iter().map(|x| x.key)).collect()
}

/// The description of one key, if the catalog has it.
pub fn find(key: &str) -> Option<Setting> {
    sections().into_iter().flat_map(|s| s.settings).find(|s| s.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_described() {
        let mut seen = std::collections::HashSet::new();
        for section in sections() {
            assert!(!section.about.trim().is_empty(), "{} has no description", section.id);
            for s in &section.settings {
                assert!(seen.insert(s.key), "{} is listed twice", s.key);
                assert!(!s.label.trim().is_empty() && !s.help.trim().is_empty(), "{} needs a label and help", s.key);
            }
            for c in &section.commands {
                assert!(!c.who.trim().is_empty() && !c.usage.trim().is_empty() && !c.what.trim().is_empty(), "/{} needs describing", c.name);
            }
        }
    }

    /// Secrets never become panel settings: they would be shown and stored.
    #[test]
    fn no_secret_is_a_setting() {
        for key in keys() {
            assert!(!key.contains("TOKEN") && !key.contains("API_KEY") && key != "VIZIER_DISCORD_ADMIN_IDS", "{} is a secret", key);
        }
    }

    /// Every source file under the discord module, read at test time.
    fn sources(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("discord sources").flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap_or_default()));
            }
        }
    }

    #[test]
    fn every_setting_read_is_described_and_every_described_setting_is_read() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/channels/discord");
        let mut files = Vec::new();
        sources(&root, &mut files);
        let read = regex::Regex::new(r#"control::(?:var|id|ids|number|float|on)\(\s*"(VIZIER_[A-Z0-9_]+)""#).unwrap();
        let mut used = std::collections::BTreeSet::new();
        for (path, text) in &files {
            if path.ends_with("catalog.rs") {
                continue;
            }
            used.extend(read.captures_iter(text).map(|c| c[1].to_string()));
        }
        assert!(used.len() > 50, "the scan found only {} settings - has the reading style changed?", used.len());
        let described: std::collections::BTreeSet<String> = keys().into_iter().map(String::from).collect();
        let missing: Vec<&String> = used.difference(&described).collect();
        assert!(missing.is_empty(), "read but not in the catalog: {:?}", missing);
        let unread: Vec<&String> = described.difference(&used).collect();
        assert!(unread.is_empty(), "in the catalog but never read: {:?}", unread);
    }
}
