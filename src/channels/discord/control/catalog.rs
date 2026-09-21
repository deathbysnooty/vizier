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
    /// Several India times of day, "HH:MM,HH:MM".
    Times,
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
/// Captains and mods both - and not the word "admin", so /help still shows it
/// to the members who are captains.
const CAPTAINS: &str = "House captains and mods";
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
            settings: vec![
                setting(
                    "VIZIER_DISCORD_CHANNELS",
                    "Chat channels",
                    "Channels the bot chats in and counts messages from for /awards and chat points. Empty means every \
                     channel it can see. A newly added channel's older history is only counted after a restart.",
                    Kind::Channels,
                    "",
                ),
                setting(
                    "VIZIER_RESTART_NOTICES",
                    "Announce restarts",
                    "Post \"the bot is being updated\" in every running game's channel when the bot restarts, and \
                     \"back up\" when it returns. Off: restarts go by without a word.",
                    Kind::Toggle,
                    "off",
                ),
            ],
            commands: vec![
                command("help", EVERYONE, "/help topic:", "Every command you can use, by area; pick a topic for the details. Only the asker sees it."),
            ],
        },
        Section {
            id: "admin",
            title: "Admin controls",
            icon: "🛡️",
            about: "Switches for the bot as a whole. /stop silences it everywhere (slash commands still work so it \
                    can be brought back); admin commands are hidden from members without Manage Server.",
            settings: vec![],
            commands: vec![
                command("stop", ADMINS, "/stop", "Pauses the bot everywhere until /resume: it reads, stores and answers nothing. The quiz holds its place."),
                command("resume", ADMINS, "/resume", "Brings the bot back after /stop."),
                command("panel", ADMINS, "/panel", "Sends you a private one-time link to the web control panel (valid 10 minutes, keeps you signed in for 7 days)."),
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
                toggle(
                    "VIZIER_HOUSE_WAIT_CARD",
                    "Tell newcomers a mod will sort them",
                    "With Sort newcomers off, post a card under the welcome saying the house games exist, where the rules are, and that a mod will give them a house.",
                ),
                setting(
                    "VIZIER_HOUSE_WAIT_TEXT",
                    "Newcomer card text",
                    "What that card says. {mention} pings them, {name} is their name and {games} becomes the scoreboard channel. Empty uses the bot's own wording.",
                    Kind::Text,
                    "",
                ),
                toggle(
                    "VIZIER_HOUSE_PING",
                    "House rallies",
                    "Let a house captain tag their own house with /houseping. Off refuses every rally, captains included.",
                ),
                setting(
                    "VIZIER_HOUSE_PING_HOURS",
                    "Hours between rallies",
                    "How long a house waits between /houseping rallies. Counted per house, not per person, and remembered across restarts.",
                    number(1, 168, "hours"),
                    "6",
                ),
                setting(
                    "VIZIER_HOUSE_PING_MAX_CHARS",
                    "Longest rally",
                    "How long a /houseping message may be. A rally is a shout, not an essay; anything longer is refused.",
                    number(20, 1000, "characters"),
                    "300",
                ),
                toggle(
                    "VIZIER_HOUSECUP",
                    "Scoreboard page",
                    "The public House Cup web page and the /housecup command that hands out its link. Off closes the page for everyone, link or no link; the link itself does not change and works again the moment this goes back on.",
                ),
                setting(
                    "VIZIER_HOUSECUP_CACHE_SECS",
                    "Scoreboard refresh",
                    "How long the scoreboard page serves one set of figures before working them out again. Everyone reading shares the same pass over the points, so a hundred people watching cost the same as one.",
                    number(1, 300, "seconds"),
                    "10",
                ),
            ],
            commands: vec![
                command("houses", EVERYONE, "/houses", "The four houses with this month's points, member counts and captains."),
                command("houselist", EVERYONE, "/houselist house:", "Who is in a house, a page at a time. Only the asker sees it."),
                command(
                    "houseping",
                    CAPTAINS,
                    "/houseping message: house:",
                    "Tags your own house with a short message, in the channel you run it in. The house roles can't be \
                     mentioned by hand, so the bot does it for the captain: one house at a time, once every few hours, \
                     and with any other ping stripped out of the words. Mods may rally any house by naming one.",
                ),
                command(
                    "housecup",
                    EVERYONE,
                    "/housecup",
                    "Posts the link to the live scoreboard page: the four houses' points this month with bars, every \
                     house's top ten scorers and the Chocolate Frog cards each one holds, updating itself while it is \
                     open. The address carries a long random token so it can't be guessed, and it never changes.",
                ),
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
                command(
                    "modpoints",
                    ADMINS,
                    "/modpoints",
                    "The points a mod has earned from the games and not yet given away. Mods are in no house, so what they \
                    win is held for them instead of going to a house. Private.",
                ),
                command(
                    "modgive",
                    ADMINS,
                    "/modgive house: points:",
                    "Gives a house the points the mod is holding - all of them, or as many as asked for. Posted publicly, \
                    so the Cup can be checked.",
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
                setting("VIZIER_CAP_CHAT", "Chat points a day", "Most chat points one person can earn in a day (one per message tier reached).", number(0, 3, "points"), "3"),
                setting("VIZIER_CAP_VOICE", "Voice points a day", "Most voice points one person can earn in a day. The hours climb (see Voice hour ladder), so this is what stops a marathon.", number(0, 100, "points"), "20"),
                setting("VIZIER_CAP_QUIZ", "Quiz points a day", "Most points one person can earn from quiz rounds in a day. 100 means no limit.", number(0, 100, "points"), "6"),
                setting("VIZIER_CAP_KOTO", "Koto points a day", "Most points one person can earn from Koto in a day. 100 means no limit.", number(0, 100, "points"), "4"),
                setting("VIZIER_CAP_GUESS", "Guess the Word points a day", "Most points one person can earn from Guess the Word in a day. 100 means no limit.", number(0, 100, "points"), "10"),
                setting("VIZIER_CAP_ANAGRAM", "Anagram points a day", "Most points one person can earn from anagrams in a day — the bot's own Anagrams game and Anagram Bot together, since they are one kind of word game. 100 means no limit.", number(0, 100, "points"), "10"),
                setting("VIZIER_CAP_CAT", "Cat Bot points a day", "Most points one person can earn from Cat Bot catches in a day. 100 means no limit.", number(0, 100, "points"), "3"),
                setting("VIZIER_CAP_ARENA", "Arena points a day", "Most points one person can earn from 1v1 fights in a day. 100 means no limit.", number(0, 100, "points"), "3"),
                setting(
                    "VIZIER_CAP_SNITCH",
                    "Snitch points a day",
                    "Most points one person can earn from Bronze and Silver Snitches in a day; 100 means no limit. The Golden Snitch is never limited.",
                    number(0, 100, "points"),
                    "6",
                ),
                setting("VIZIER_CAP_FROG", "Chocolate Frog points a day", "Most points one person can earn from Chocolate Frogs in a day, the collection bonus included. 100 means no limit.", number(0, 100, "points"), "100"),
                setting("VIZIER_CAP_NPAT", "Name Place Animal Thing points a day", "Most house points one person can win from Name Place Animal Thing rounds in a day (1st and 2nd places, review fixes included). 100 means no limit.", number(0, 100, "points"), "6"),
                setting("VIZIER_CAP_SUDOKU", "Sudoku house-points limit (no longer used)", "Left from when sudoku paid house points. It does not any more - it keeps its own score, sudoku points, which has no limit - so this no longer bites on anything. It is kept because the ledger still holds the sudoku house points people earned before the change, and those are untouched.", number(0, 100, "points"), "20"),
                setting("VIZIER_CAP_CHESS", "Chess house-points limit (no longer used)", "Left from when chess paid house points. It does not any more - it keeps its own score, chess points, which has no limit - so this no longer bites on anything. It is kept because the ledger still holds the chess house points people earned before the change, and those are untouched.", number(0, 100, "points"), "8"),
                setting("VIZIER_CAP_PUZZLE", "Chess puzzle points a day", "Most house points one person can win from chess puzzles in a day. Only the FIRST person to crack each puzzle earns one at all. Nought - the default - means chess puzzles pay no house points whatever: an engine solves any of them instantly, so this is deliberately off until you decide otherwise. Puzzle points themselves are never capped and are not affected by this. 100 means no limit.", number(0, 100, "points"), "0"),
                setting("VIZIER_CAP_DUEL", "Letter Duel points a day", "Most house points one person can win from Letter Duel in a day. Duel points themselves are never capped. 100 means no limit.", number(0, 100, "points"), "8"),
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
            id: "frogs",
            title: "Chocolate Frogs",
            icon: "🐸",
            about: "A few times a day a Chocolate Frog card hops into a busy chat channel. Anyone \
                    in a house can press Catch it to see a riddle in a private pop-up and type the answer; each person \
                    gets three tries, and close spellings count. The first right answer keeps the card, a numbered \
                    collectable, and scores the frog's points for their house. Nobody right in time and the frog hops \
                    away, showing the answer but never the riddle. Uncommon cards ask harder riddles and pay more, the \
                    Legendary Eternal Phoenix hardest and most, and a full set of cards can be sold for points with /sellset. Drops only land where someone spoke recently and keep \
                    clear of Snitch drops. The cards, recent drops, card owners and the riddle bank are managed on \
                    this page. A day's plan is made once, so changes to drops, hours and gap apply from the next day's \
                    plan.",
            settings: vec![
                setting("VIZIER_FROGS", "Scheduled drops", "Drop Chocolate Frogs on their own through the day. Off by default; the Drop a frog now button still works when it's off.", Kind::Toggle, "off"),
                setting(
                    "VIZIER_FROG_CHANNELS",
                    "Drop channels",
                    "Channels a frog can drop into, each with a weight for how often. The first is the home channel a drop falls back to when its pick is quiet. Empty means the Snitch's drop channels. Frogs never drop in #safe-corner.",
                    Kind::WeightedChannels,
                    "",
                ),
                setting("VIZIER_FROG_DROPS_MIN", "Fewest drops a day", "The least scheduled frogs in a day; each day picks at random between this and the most.", number(0, 30, "drops"), "5"),
                setting("VIZIER_FROG_DROPS_MAX", "Most drops a day", "The most scheduled frogs in a day.", number(0, 30, "drops"), "7"),
                setting("VIZIER_FROG_START_HOUR", "Drops from", "India hour the drop hours start (0 is midnight).", number(0, 23, "hour"), "0"),
                setting("VIZIER_FROG_END_HOUR", "Drops until", "India hour the drop hours end (24 is midnight). The last frog drops early enough to be over by then.", number(1, 24, "hour"), "24"),
                setting("VIZIER_FROG_GAP_MINUTES", "Gap between drops", "The least time between two scheduled frogs.", number(1, 1440, "minutes"), "90"),
                setting("VIZIER_FROG_SNITCH_GAP_MINUTES", "Gap from a Snitch", "A frog never drops this close to a Snitch drop, before or after. 0 switches this off.", number(0, 240, "minutes"), "20"),
                setting("VIZIER_FROG_POPUP_TEXT_BLOCK", "Riddle as a text block", "Show the riddle as plain text above the answer box. Off by default because that newer pop-up block crashes the Discord iPhone app; off puts the riddle in a read-only-style box instead.", Kind::Toggle, "off"),
                setting("VIZIER_FROG_OPEN_MINUTES", "Time to catch", "How long a frog stays open before it hops away. A frog already in chat keeps its own time.", number(1, 60, "minutes"), "5"),
                setting("VIZIER_FROG_QUIET_MINUTES", "Quiet after", "A channel counts as busy if someone spoke in it this recently; frogs only drop into busy channels.", number(1, 120, "minutes"), "5"),
                setting("VIZIER_FROG_WEIGHT_COMMON", "Common: how often", "How often a drop is Common (easy riddle), weighed against the other rarities. With the defaults 58, 35 and 7 that's 58 drops in 100. 0 means never.", number(0, 1000, "weight"), "58"),
                setting("VIZIER_FROG_WEIGHT_UNCOMMON", "Uncommon: how often", "How often a drop is Uncommon (medium riddle).", number(0, 1000, "weight"), "35"),
                setting("VIZIER_FROG_WEIGHT_LEGENDARY", "Legendary: how often", "How often a drop is Legendary (hard riddle): the Eternal Phoenix. 7 in 100 is about three a week at six drops a day.", number(0, 1000, "weight"), "7"),
                setting("VIZIER_POINTS_FROG_COMMON", "Common points", "Points for catching a Common frog.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_FROG_UNCOMMON", "Uncommon points", "Points for catching an Uncommon frog.", number(0, 100, "points"), "4"),
                setting("VIZIER_POINTS_FROG_LEGENDARY", "Legendary points", "Points for catching a Legendary frog.", number(0, 100, "points"), "10"),
                toggle("VIZIER_FROG_DAILY_TOP", "Top of the day cards", "Each morning, give the day before's top scorer in every game (chat, voice, quiz, Koto, anagram, cats, Wordle, arena, Snitch, frogs, Name Place Animal Thing game wins) a random Common or Uncommon card (no house points: only catching frogs pays those). Only while scheduled frog drops are on."),
                setting("VIZIER_FROG_DAILY_TOP_TIME", "Top of the day time", "India time the top of the day cards go out, for the day before. After Wordle's morning results is best. Missed while the bot was down, they go out when it's back.", Kind::Time, "10:00"),
                setting("VIZIER_FROG_DAILY_TOP_CHANNEL", "Top of the day channel", "Where the morning summary of who won which card is posted. Empty uses the houses channel.", Kind::Channel, ""),
                toggle("VIZIER_FROG_ROYALE_CARDS", "Battle royale cards", "Give a battle royale's champion and runner-up a random Common or Uncommon card each (no house points). Only while scheduled frog drops are on."),
                setting("VIZIER_FROG_ROYALE_MIN_PLAYERS", "Royale cards from", "A royale needs at least this many fighters for its cards, so tiny royales can't be farmed.", number(2, 64, "fighters"), "6"),
                toggle("VIZIER_TRADES", "Card trading", "Let house members trade cards with /trade. Trades move cards only, never points. Only while scheduled frog drops are on."),
                setting("VIZIER_TRADE_EXPIRY_HOURS", "Trade offers last", "How long a trade offer waits for an answer before it expires.", number(1, 168, "hours"), "24"),
                setting("VIZIER_FROG_SET_BONUS", "Full set price", "House points a member gets for handing in one copy of every card in play with /sellset. The copies are spent; each sale needs a whole new set.", number(0, 1000, "points"), "35"),
            ],
            commands: vec![
                command(
                    "frogs",
                    EVERYONE,
                    "/frogs member:",
                    "Your Chocolate Frog cards, or anyone's: which cards are collected, frog points, and every copy owned, like The Eternal Phoenix #3 · No. 0187. Only you see it.",
                ),
                command(
                    "housecards",
                    EVERYONE,
                    "/housecards house: view:",
                    "What a whole house holds. By card (the default): every card in the game in the same order as \
                     /frogs, how many of each the house has, who has them with their numbers, and which cards nobody \
                     has at all - plus how many full sets the house could make between them, which is what /sellset \
                     wants. By member: each collector, most cards first, and what they hold, with 🏆 on anyone \
                     holding a whole set alone. Members see their own house; mods may name any. A card counts for \
                     the house its holder is in now, so cards move house when their owner does. Only the asker sees it.",
                ),
                command(
                    "trade",
                    EVERYONE,
                    "/trade member:",
                    "Offer cards to another house member and ask for some of theirs. They accept or decline in the channel; cards only move if everyone still has them.",
                ),
                command("trades", EVERYONE, "/trades", "Your open trade offers, both ways, and your latest trades. Only you see it."),
                command(
                    "sellset",
                    EVERYONE,
                    "/sellset",
                    "Hand in one copy of every card in play for house points. Your highest-numbered copies go unless you pick others; the copies are spent.",
                ),
                command(
                    "frogdrop",
                    ADMINS,
                    "/frogdrop rarity:",
                    "Drops a Chocolate Frog (Common, Uncommon, Legendary or random by the usual odds) in this channel right now, even while scheduled drops are off. It plays like any other frog but doesn't use up a scheduled drop.",
                ),
                command(
                    "frogcard",
                    EVERYONE,
                    "/frogcard number:",
                    "One card by its No.: which card and copy it is, who owns it, when it was caught and the answer that won it. Only you see it.",
                ),
            ],
        },
        Section {
            id: "npat",
            title: "Name Place Animal Thing",
            icon: "🔤",
            about: "The classic game in its own channel (Game channel below), played only with the bot's posts, buttons and \
                    pop-ups: members can't type there, so deny Send Messages for @everyone and let the bot send, embed, \
                    read history and manage messages. From the top the channel holds a rules post written from these \
                    settings, the results of earlier letters and games, and the game card, which always moves back to \
                    the bottom. The lobby is idle until someone presses I'm in; that opens a join window, and once enough \
                    players from enough different houses are in, a short countdown starts a game. Muggles can play and \
                    count as players but not as a house. A game is several letters: for each, everyone presses Submit \
                    answers and types a Name, Place, Animal and Thing in a private pop-up. One AI call judges every \
                    answer by a fixed rulebook (real names, real places on a map, real living animals, real touchable \
                    things, no brands, Hindi and Hinglish welcome) and merges spellings and languages; answers it has \
                    judged before are remembered. If the AI can't be reached, answers are checked by their first letter \
                    only. Unique answers score 10 and shared ones 5, added up over the game. When the game ends, its two \
                    best house members win house points, up to the daily limit set under Points & limits (a Muggle can \
                    top a game but never earns them). Players can challenge their own answers for 30 minutes and mods \
                    press Review to flip an answer, which rescores the game and fixes the house points. Never runs in \
                    #safe-corner.",
            settings: vec![
                toggle("VIZIER_NPAT", "Game on", "Run the Name Place Animal Thing lobby and games in its channel. Off stops new games; a letter already running still finishes."),
                setting("VIZIER_NPAT_CHANNEL", "Game channel", "The channel the game lives in. Empty means the game is off. Never #safe-corner.", Kind::Channel, ""),
                toggle("VIZIER_NPAT_RULES", "Rules post", "Keep a detailed How it works post at the top of the game channel, written from these settings and edited by itself when they change. Put back if deleted."),
                setting("VIZIER_NPAT_MIN_PLAYERS", "Players to start", "How many must press I'm in before a game starts.", number(1, 50, "players"), "5"),
                setting("VIZIER_NPAT_MIN_HOUSES", "Houses to start", "Those players must come from at least this many different houses (Muggles don't count as a house).", number(1, 4, "houses"), "2"),
                setting("VIZIER_NPAT_JOIN_SECONDS", "Join window", "How long the lobby stays open after the first I'm in. Not enough players by then and it resets.", number(30, 1800, "seconds"), "180"),
                setting("VIZIER_NPAT_START_SECONDS", "Countdown once enough", "Once enough players are in, the game starts this soon (never later than the join window), so more can still jump in.", number(5, 600, "seconds"), "30"),
                setting("VIZIER_NPAT_LETTERS_PER_GAME", "Letters per game", "How many letters one game has. House points are paid at the end of the game.", number(1, 10, "letters"), "5"),
                setting("VIZIER_NPAT_POPUP_TIMER", "Live timer inside the pop-up", "Show a live countdown line inside the answers pop-up. Off by default: that newer pop-up block crashes the Discord iPhone app. The pop-up title always shows the seconds left.", Kind::Toggle, "off"),
                setting("VIZIER_NPAT_SECONDS", "Time per letter", "How long players have to send their answers for each letter. A few seconds of grace after the timer catch last-second pop-ups.", number(15, 600, "seconds"), "45"),
                setting("VIZIER_NPAT_BREAK_SECONDS", "Break between letters", "The pause after a letter's results before the next letter starts.", number(3, 600, "seconds"), "20"),
                setting("VIZIER_NPAT_LETTERS", "Letters", "The letters a game can use; each letter avoids the last five used. Hard letters like Q, X, Y and Z are left out by default.", Kind::Text, "ABCDEFGHIJKLMNOPRSTUVW"),
                setting("VIZIER_NPAT_SCORE_UNIQUE", "Score: unique answer", "Game score for a valid answer nobody else gave in that box. Shown on the results; not house points.", number(0, 1000, "score"), "10"),
                setting("VIZIER_NPAT_SCORE_SHARED", "Score: shared answer", "Game score for a valid answer someone else also gave. Not house points.", number(0, 1000, "score"), "5"),
                setting("VIZIER_POINTS_NPAT_1ST", "1st place house points", "House points for the best house member of a game. Equal totals go to whoever reached theirs first.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_NPAT_2ND", "2nd place house points", "House points for the second best house member of a game.", number(0, 100, "points"), "1"),
                setting("VIZIER_NPAT_MIN_SCORED", "Players for house points", "A game only pays house points if at least this many different people answered at least one of its letters.", number(1, 50, "players"), "3"),
                setting("VIZIER_NPAT_AI_MODEL", "Judge model", "The AI model that judges answers, on the bot's own provider (OpenRouter). Type agent to use the bot's usual model. Empty uses the default, a quick and cheap one.", Kind::Text, "google/gemini-2.5-flash-lite"),
            ],
            commands: vec![
                command(
                    "I'm in · Submit answers",
                    EVERYONE,
                    "Buttons in the Name Place Animal Thing channel",
                    "Press I'm in on the game card to join the lobby, then Submit answers on each letter to type your Name, Place, Animal and Thing in a private pop-up. The rules post at the top of the channel explains everything, including the house points and the daily limit.",
                ),
                command(
                    "npatstop",
                    ADMINS,
                    "/npatstop",
                    "Stops the Name Place Animal Thing game that is running (or the countdown) with no house points, and puts the lobby back.",
                ),
            ],
        },
        Section {
            id: "sudoku",
            title: "Sudoku",
            icon: "🔢",
            about: "A sudoku is always waiting in its own channel (Game channel below), and the first person to solve \
                    it wins the puzzle's SUDOKU POINTS - the game's own score, which is not house points and does not move the House \
                    Cup. Members can't type there, so deny Send Messages for @everyone and let the \
                    bot send, embed, attach files, read history and manage messages. The card sits at the bottom with \
                    a picture of the grid and four buttons: Play, Submit code, Hint and Today's solvers. Play sends \
                    that member a private link to a web page (the panel's own address plus /sudoku/<number>) where \
                    they fill the grid in a browser, with pencil marks, Undo, Check and Reset; the page never learns \
                    the answer, it only turns a finished grid into a short code. Pasting a correct code into Submit \
                    code wins the puzzle: the card turns into a Solved card and the next puzzle goes up at once, at a \
                    new difficulty. Puzzles are made by the bot itself, always with exactly one answer. Someone who \
                    was still working when the channel moved on can still paste their code: they are told whether it \
                    was right and it counts as a finish, but the points went to whoever was first. Never runs in \
                    #safe-corner.",
            settings: vec![
                setting("VIZIER_SUDOKU", "Game on", "Run the sudoku game in its channel. Off takes the card down and stops new puzzles; the one that was up is kept, and comes back when you switch it on again.", Kind::Toggle, "off"),
                setting("VIZIER_SUDOKU_CHANNEL", "Game channel", "The channel the puzzle card lives in. Empty means the game is off. Never #safe-corner.", Kind::Channel, ""),
                toggle("VIZIER_SUDOKU_RULES", "Rules post", "Keep a How it works post at the top of the sudoku channel, written from these settings and edited by itself when they change. Put back if deleted."),
                setting("VIZIER_SUDOKU_MIX", "Difficulty mix", "How often each difficulty comes up, as name:weight pairs. The weights are shares, not percentages: easy:40,medium:40,hard:20 means easy and medium twice as often as hard. Easy puzzles start with 36-40 squares filled in, medium 30-34 and hard 26-29.", Kind::Text, "easy:40,medium:40,hard:20"),
                setting("VIZIER_POINTS_SUDOKU_EASY", "Easy puzzle points", "Sudoku points for the first correct code on an easy puzzle. Sudoku points are the game's own score, not house points.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_SUDOKU_MEDIUM", "Medium puzzle points", "Sudoku points for the first correct code on a medium puzzle. Sudoku points are the game's own score, not house points.", number(0, 100, "points"), "4"),
                setting("VIZIER_POINTS_SUDOKU_HARD", "Hard puzzle points", "Sudoku points for the first correct code on a hard puzzle. Sudoku points are the game's own score, not house points.", number(0, 100, "points"), "6"),
                setting("VIZIER_SUDOKU_HINT_COST", "What a hint costs", "How many sudoku points one hint takes off that puzzle for the person who asked for it. Their score never goes below nothing, and it only affects them.", number(0, 100, "points"), "1"),
                setting("VIZIER_SUDOKU_MAX_HINTS", "Hints per puzzle", "How many squares one person may have given away on the same puzzle.", number(0, 80, "hints"), "3"),
                setting("VIZIER_SUDOKU_MAX_TRIES", "Codes per puzzle", "How many codes one person may send for the same puzzle before it is closed to them. A wrong code says how many squares are wrong, never which, and there are ten seconds between tries.", number(1, 100, "tries"), "10"),
                setting("VIZIER_SUDOKU_LATE_HOURS", "Late codes accepted for", "How long after a puzzle goes up its code is still checked, so someone who was still solving is told whether they were right. It scores nothing: the sudoku points went to whoever was first. The last ten puzzles are always checked, however old.", number(1, 720, "hours"), "24"),
            ],
            commands: vec![
                command(
                    "Play · Submit code · Hint",
                    EVERYONE,
                    "Buttons on the sudoku card",
                    "Play sends you a private link to the puzzle's page, Submit code takes the code you copied from it, Hint gives away one square (and takes a sudoku point off that puzzle for you), and Today's solvers lists the day's wins and finishes.",
                ),
                command("sudoku", EVERYONE, "/sudoku", "Your sudoku links: the puzzle that is up now and any from the last day that you started and never finished, each with its own link, and a Submit code button."),
                command("sudokutop", EVERYONE, "/sudokutop", "The sudoku points board, today or this month: the top 10 and, if you are not on it, your own line. Sudoku points are the game's own score - each puzzle at what it was worth with that solver's hints taken off, and no daily limit - and they are NOT house points: sudoku does not move the House Cup. Everyone has them, mods and Muggles included. Only you see it."),
                command("sudokuhelp", EVERYONE, "/sudokuhelp", "How the sudoku game works, written from the settings as they are right now: how to play, what each difficulty is worth in sudoku points, what a hint costs and how long a late code is still checked. Only you see it."),
                command("sudokunew", ADMINS, "/sudokunew", "Skips the puzzle that is up, so nobody scores for it, and posts a fresh one at once."),
            ],
        },
        Section {
            id: "anagrams",
            title: "Anagrams",
            icon: "🔀",
            about: "A scrambled word is always waiting in its own channel (Game channel below), and the first person to \
                    unscramble it wins house points. Unlike sudoku and chess, members DO type here: answering is \
                    typing, so leave Send Messages on for @everyone and let the bot send, embed, read history, add \
                    reactions and manage messages (and Manage Channels if you want the channel topic used as a \
                    header). The bot takes a word from the word bank, shuffles its letters into an arrangement that \
                    gives nothing away, and puts the letters up spaced out and in capitals. ANY word in the \
                    dictionary that uses ALL of the letters wins - the letters of BEAST are taken by beast, bates and \
                    tabes alike - so nobody is ever told their perfectly good word was the wrong one. Case and the \
                    punctuation around a word are forgiven; a message that isn't one word is not a guess. The first \
                    right answer gets a ✅ on the message, a line naming the winner, and the next scramble at once. \
                    Wrong guesses are ignored in silence, because a ❌ on every stray message in a chatty channel \
                    would be noise. Typing !hint gives away the first letter, once a round, and takes a point off \
                    what the round pays; !skip opens up only after a hint and pays nobody. A round nobody answers is \
                    replaced by the bot itself, so the channel is never stuck on one word overnight, and the same \
                    letters don't come round again for a month. The words come from wordbank/puzzles.txt and \
                    wordbank/dictionary.txt in the bot's workspace, read at start: with no word bank there the game \
                    simply stays off and says so in the log. Never runs in #safe-corner.",
            settings: vec![
                setting("VIZIER_ANAGRAM", "Game on", "Run the anagrams game in its channel. Off takes the card down and stops new rounds; the round that was up is left as it is. The game also stays off, whatever this says, when there is no word bank to play with.", Kind::Toggle, "off"),
                setting("VIZIER_ANAGRAM_CHANNEL", "Game channel", "The channel the scramble card lives in. Members need to be able to type there - that is how answers are given. Empty means the game is off. Never #safe-corner.", Kind::Channel, ""),
                setting("VIZIER_POINTS_ANAGRAM_SHORT", "Short word points", "House points for the first right answer to a 4 or 5 letter word.", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_ANAGRAM_MEDIUM", "Middling word points", "House points for the first right answer to a 6 or 7 letter word.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_ANAGRAM_LONG", "Long word points", "House points for the first right answer to a word of 8 letters or more.", number(0, 100, "points"), "3"),
                setting("VIZIER_ANAGRAM_IDLE_MINUTES", "Replace a round after", "How long a round nobody answers and nobody skips stays up before the bot reveals the word and sets a new one by itself.", number(1, 1440, "minutes"), "30"),
                setting("VIZIER_ANAGRAM_NO_REPEAT_DAYS", "Don't repeat letters for", "How long the same set of letters is held back before it can be set again. It goes by the letters, not the word, so LISTEN and SILENT are the same round.", number(0, 365, "days"), "30"),
                setting("VIZIER_ANAGRAM_BUMP_MESSAGES", "Messages before the card moves", "How many messages from other people have to land under the card before it is posted again at the bottom and the old copy deleted. The channel is a chatty one, so the card follows the conversation down rather than jumping after every message.", number(1, 100, "messages"), "5"),
                setting("VIZIER_ANAGRAM_BUMP_SECONDS", "Wait between moves", "The shortest time between two of those moves, however busy the channel gets. Both this and the message count have to be met.", number(10, 3600, "seconds"), "120"),
                toggle("VIZIER_ANAGRAM_TOPIC", "Letters in the channel topic", "Write the round into the channel's topic, so the letters sit at the top of the channel as a header that never scrolls away. Needs Manage Channels for the bot in that channel; Discord only allows a couple of topic edits every ten minutes, so it is best effort and a round never waits for it."),
            ],
            commands: vec![
                command("anagram", EVERYONE, "/anagram", "The round that is up now: its letters, how long the word is, what it is worth and whether the hint has gone. Only you see it."),
                command("anagramtop", EVERYONE, "/anagramtop", "The anagram points board, today or this month: the top 10 and, if you are not on it, your own line. Anagram points are the game's own score - every solve at what the round was worth, hint taken off but no daily limit - so they keep counting after the house-points limit is full, and mods and Muggles have them too. Only you see it."),
                command("anagramhelp", EVERYONE, "/anagramhelp", "How the anagrams game works, written from the settings as they are right now: how to answer, what each length pays, and what !hint and !skip do. Only you see it."),
                command("anagramskip", ADMINS, "/anagramskip", "Drops the round that is up, with no points for anyone, reveals the word and sets a fresh one at once. Unlike !skip, no hint is needed first."),
                command("anagramstop", ADMINS, "/anagramstop", "Switches the anagrams game off: the card comes down, the channel topic is cleared and no new rounds are set. Switch Game on back on to play again."),
            ],
        },
        Section {
            id: "guess",
            title: "Guess the Word",
            icon: "🎨",
            about: "A hand-drawn doodle is always waiting in its own channel (Game channel below), and the first person to \
                    say what it is wins house points. Members DO type here - guessing is typing - so leave Send Messages \
                    on for @everyone and let the bot send, embed, attach files, read history, add reactions and manage \
                    messages. The bot inks out one of the drawings from the doodle bank and asks what it is; members type \
                    their guess straight into the channel. Capitals, spaces and punctuation are forgiven, so Ice-Cream, \
                    ice cream and icecream are one answer, and a spelling slip is forgiven on longer words - gitar takes a \
                    guitar - but never as far as a different word in the bank, so car can't take a police car. The first \
                    right guess gets a ✅ on the message, a line naming the winner, and the next doodle at once. Wrong \
                    guesses are ignored in silence, because a ❌ on every stray message in a chatty channel would be \
                    noise. Typing !hint puts a SECOND drawing of the same thing up beside the first and gives away the \
                    word's first letter, once a round, and takes a point off what the round pays; !skip opens up only \
                    after a hint and pays nobody. A round nobody gets is replaced by the bot itself, and neither a word \
                    nor any single drawing of it comes round again inside the no-repeat window. The doodles come from \
                    drawbank/words.json and drawbank/doodles.bin in the bot's workspace, read at start: with no bank \
                    there the game simply stays off and says so in the log. The drawings are real ones out of Google's \
                    Quick, Draw! dataset, used under CC BY 4.0, and the game credits them wherever they appear. Never \
                    runs in #safe-corner.",
            settings: vec![
                setting("VIZIER_GUESS", "Game on", "Run Guess the Word in its channel. Off takes the card down and stops new rounds; the round that was up is left as it is. The game also stays off, whatever this says, when there is no doodle bank to play with.", Kind::Toggle, "off"),
                setting("VIZIER_GUESS_CHANNEL", "Game channel", "The channel the doodle card lives in. Members need to be able to type there - that is how guesses are given. Empty means ❓guess-the-word, the channel the game was made for. Never #safe-corner.", Kind::Channel, "1518233664016617582"),
                setting("VIZIER_POINTS_GUESS", "Points for naming it", "House points for the first person to say what the doodle is. A hint takes one off, never below one.", number(0, 100, "points"), "2"),
                setting("VIZIER_GUESS_IDLE_MINUTES", "Replace a round after", "How long a doodle nobody names and nobody skips stays up before the bot says what it was and draws a new one by itself. A picture is quicker to give up on than a word, so this is shorter than the anagrams one.", number(1, 1440, "minutes"), "15"),
                setting("VIZIER_GUESS_NO_REPEAT_DAYS", "Don't repeat a word for", "How long a word is held back before it can be drawn again - and, inside that, how long each particular drawing of it is held back, so the same picture is never shown twice.", number(0, 365, "days"), "14"),
                setting("VIZIER_GUESS_BUMP_MESSAGES", "Messages before the card moves", "How many messages from other people have to land under the card before it is posted again at the bottom and the old copy deleted. The channel is a chatty one, so the card follows the conversation down rather than jumping after every message.", number(1, 100, "messages"), "5"),
                setting("VIZIER_GUESS_BUMP_SECONDS", "Wait between moves", "The shortest time between two of those moves, however busy the channel gets. Both this and the message count have to be met.", number(10, 3600, "seconds"), "120"),
            ],
            commands: vec![
                command("guess", EVERYONE, "/guess", "The doodle that is up now, sent privately with the picture: what it is worth, whether the hint has gone, and how you have done today. Only you see it."),
                command("guesstop", EVERYONE, "/guesstop", "The guess points board, today or this month: the top 10 and, if you are not on it, your own line. Guess points are the game's own score - every solve at what the round was worth, hint taken off but no daily limit - so they keep counting after the house-points limit is full, and mods and Muggles have them too. Only you see it."),
                command("guesshelp", EVERYONE, "/guesshelp", "How Guess the Word works, written from the settings as they are right now: how to guess, what a round pays, and what !hint and !skip do. Only you see it."),
                command("guessskip", ADMINS, "/guessskip", "Drops the doodle that is up, with no points for anyone, says what it was and draws a fresh one at once. Unlike !skip, no hint is needed first."),
                command("guessstop", ADMINS, "/guessstop", "Switches Guess the Word off: the card comes down and no new doodles go up. Switch Game on back on to play again."),
            ],
        },
        Section {
            id: "movie",
            title: "Guess the Movie",
            icon: "🎬",
            about: "A film is always waiting in its own channel (Game channel below), and the first person to type its title \
                    wins house points. The bot asks about it ONE way each round - five tags about it (revenge, guns, coal \
                    mafia, uttar pradesh, a butcher's family), a line of dialogue out of it, or a still from a scene - and \
                    members type the title straight into the channel, so leave Send Messages on for @everyone and let the \
                    bot send, embed, read history, add reactions and manage messages. Spelling is forgiven generously, and \
                    so is transliteration: dilwaale dulhaniya le jayenge takes Dilwale Dulhania Le Jayenge, and so does \
                    ddlj. Two things are never forgiven - a sequel number has to be exactly right, so don cannot take Don \
                    2, and a guess that fits two films at once takes neither. The first right guess gets a ✅, a line \
                    naming the winner and the film, and the next one at once; wrong guesses are ignored in silence. Typing \
                    !hint reveals the sharper tag the card held back, plus the title's first letter, its year and whether \
                    it is Hindi or English, once a round, and takes a point off what the round pays; !skip opens up only \
                    after a hint and pays nobody. Neither a film nor any single clue about it comes round again inside the \
                    no-repeat window. The films come from moviebank/movies.json in the bot's workspace, read at start: \
                    with no bank there the game simply stays off and says so in the log. The stills stay on TMDB's own \
                    servers and the bank holds only their paths - every one of them was looked at by hand before it went \
                    in, and a film with no still worth showing simply plays tags and dialogue. Never runs in #safe-corner.",
            settings: vec![
                setting("VIZIER_MOVIE", "Game on", "Run Guess the Movie in its channel. Off takes the card down and stops new rounds; the round that was up is left as it is. The game also stays off, whatever this says, when there is no film bank to play with.", Kind::Toggle, "off"),
                setting("VIZIER_MOVIE_CHANNEL", "Game channel", "The channel the film card lives in. Members need to be able to type there - that is how guesses are given. Empty means 🎬 guess-the-movie, the channel the game was made for. Never #safe-corner.", Kind::Channel, "1551044377374101594"),
                setting("VIZIER_POINTS_MOVIE", "Movie points for naming it", "MOVIE points for the first person to type the title - the game's own uncapped score, not house points. A hint takes one off, never below one. House points come from winning a match, below.", number(0, 100, "points"), "3"),
                setting("VIZIER_MOVIE_MATCH_FILMS", "Films in a match", "How many films the game plays before it scores the match and takes a break. Whoever named the most wins it.", number(1, 200, "films"), "10"),
                setting("VIZIER_MOVIE_MIN_PLAYERS", "People needed to start", "How many have to press I'm ready before a match can begin. Joining a match already running needs no button - naming a film is joining.", number(1, 20, "people"), "2"),
                setting("VIZIER_MOVIE_BREAK_MINUTES", "Break between matches", "The rest after a match is scored. Nothing starts before it is up however many are ready, so people arriving late can still get in.", number(0, 240, "minutes"), "2"),
                setting("VIZIER_POINTS_MOVIE_WIN", "House points for winning a match", "What the winner of a match earns. Joint winners each take this and no runner-up is paid.", number(0, 100, "points"), "5"),
                setting("VIZIER_POINTS_MOVIE_SECOND", "House points for the runner-up", "What second place earns, when there is a single winner ahead of them.", number(0, 100, "points"), "2"),
                setting("VIZIER_CAP_MOVIE", "Daily limit", "The most house points one person can earn from this game in a day - that is, from winning matches. The movie points the game keeps alongside are never capped, so the board goes on counting after this is full. 100 or more means no limit.", number(0, 100, "points a day"), "15"),
                setting("VIZIER_MOVIE_TAGS_SHOWN", "Tags on the card", "How many tags a tags round puts up. They are written vague first and sharp last, so showing fewer makes the game harder; a hint reveals the next one down whatever this is set to.", number(3, 7, "tags"), "5"),
                setting("VIZIER_MOVIE_HINDI_SHARE", "Lean Hindi", "Out of a hundred rounds, how many should be Hindi films rather than English ones. It is a lean and not a rule: when nothing fresh fits, the bot widens rather than leaving the channel empty.", number(0, 100, "%"), "50"),
                setting("VIZIER_MOVIE_MODERN_SHARE", "Lean recent", "Out of a hundred rounds, how many should be recent films rather than the older classics. Same lean, same widening.", number(0, 100, "%"), "65"),
                setting("VIZIER_MOVIE_IDLE_MINUTES", "Replace a round after", "How long a film nobody names and nobody skips stays up before the bot says what it was and puts a new one up by itself.", number(1, 1440, "minutes"), "20"),
                setting("VIZIER_MOVIE_NO_REPEAT_DAYS", "Don't repeat a film for", "How long a film is held back before it can come round again - and, inside that, how long each particular still and each particular line is held back, so the same picture is never shown twice.", number(0, 365, "days"), "30"),
                setting("VIZIER_MOVIE_BUMP_MESSAGES", "Messages before the card moves", "How many messages from other people have to land under the card before it is posted again at the bottom and the old copy deleted.", number(1, 100, "messages"), "5"),
                setting("VIZIER_MOVIE_BUMP_SECONDS", "Wait between moves", "The shortest time between two of those moves, however busy the channel gets. Both this and the message count have to be met.", number(10, 3600, "seconds"), "120"),
            ],
            commands: vec![
                command("movie", EVERYONE, "/movie", "The film that is up now, sent privately with its clue: what it is worth, whether the hint has gone, and how you have done today. Only you see it."),
                command("movietop", EVERYONE, "/movietop", "The movie points board, today or this month: the top 10 and, if you are not on it, your own line. Movie points are the game's own score - every solve at what the round was worth, hint taken off but no daily limit - so they keep counting after the house-points limit is full, and mods and Muggles have them too. Only you see it."),
                command("moviehelp", EVERYONE, "/moviehelp", "How Guess the Movie works, written from the settings as they are right now: how to guess, what a round pays, and what !hint and !skip do. Only you see it."),
                command("movieskip", ADMINS, "/movieskip", "Drops the film that is up, with no points for anyone, says what it was and puts a fresh one up at once. Unlike !skip, no hint is needed first."),
                command("moviestop", ADMINS, "/moviestop", "Switches Guess the Movie off: the card comes down and no new films go up. Switch Game on back on to play again."),
                command("moviereload", ADMINS, "/moviereload", "Reads the film bank off disk again, so a new pack of films or TV shows is in play without restarting the bot. The round that is up is left alone; the next one comes from what was just read."),
            ],
        },
        Section {
            id: "geo",
            title: "Geo",
            icon: "🗺️",
            about: "GeoGuessr for India and the World. Before each match the room VOTES - India, World or Mix - and \
                    pressing a button is both the vote and the seat, the way Guess the Movie works. A World round asks \
                    for the COUNTRY and nothing finer: naming it pays 2, and naming a town in it counts as the country. \
                    An India round is the original game. A street photo taken somewhere in the country goes up in its own channel \
                    (Game channel below) and the first person to say WHERE takes the round, so leave Send Messages on \
                    for @everyone and let the bot send, embed, read history, add reactions and manage messages. A guess \
                    is a place name at whatever scale the person knows: the state is worth 2, a town within 60 km of the \
                    photo 4, and a town within 15 km 5. Naming the town gives its state for free, so nobody has to know \
                    both - but the state is a GATE, and a town in the wrong state scores nothing however near the \
                    kilometres look. A town at the far end of the right state is a wrong town and a right state, and \
                    still earns the 2. Old names work (Bombay takes Mumbai, Benares takes Varanasi, Gurgaon takes \
                    Gurugram), spelling and punctuation are forgiven, and where two towns share a name the bigger one \
                    wins it. Typing !hint gives the state's first letter and the quarter of the country it is in, once a \
                    round, and takes a point off; !skip opens up only after a hint and pays nobody. The game runs in \
                    matches and pays house points for WINNING one, not per photo. The photos come from geobank/ in the \
                    bot's workspace, read at start: with no bank there the game stays off and says so in the log. They \
                    are street photos contributed by the public to KartaView and Mapillary, so the bank only covers \
                    the states people have actually driven - twenty-two of them today, and the game draws a STATE \
                    first and a photo second so that every state it does hold comes up as often as every other. \
                    Never runs in #safe-corner.",
            settings: vec![
                setting("VIZIER_GEO", "Game on", "Run Geo in its channel. Off takes the card down and stops new rounds; the round that was up is left as it is. The game also stays off, whatever this says, when there is no place bank to play with.", Kind::Toggle, "off"),
                setting("VIZIER_GEO_CHANNEL", "Game channel", "The channel the photo card lives in. Members need to be able to type there - that is how guesses are given. Empty means the game stays off until you point this at a room. Never #safe-corner.", Kind::Channel, ""),
                setting("VIZIER_GEO_MATCH_ROUNDS", "Places in a match", "How many photos the game plays before it scores the match and takes a break. Whoever scored the most across them wins it, so one bullseye beats two states.", number(1, 50, "places"), "5"),
                setting("VIZIER_GEO_MIN_PLAYERS", "People needed to start", "How many have to press I'm ready before a match can begin. Joining a match already running needs no button - placing a photo is joining.", number(1, 20, "people"), "2"),
                setting("VIZIER_GEO_BREAK_MINUTES", "Break between matches", "The rest after a match is scored. Nothing starts before it is up however many are ready, so people arriving late can still get in.", number(0, 240, "minutes"), "2"),
                setting("VIZIER_POINTS_GEO_WIN", "House points for winning a match", "What the winner of a match earns. Joint winners each take this and no runner-up is paid.", number(0, 100, "points"), "5"),
                setting("VIZIER_POINTS_GEO_SECOND", "House points for the runner-up", "What second place earns, when there is a single winner ahead of them.", number(0, 100, "points"), "2"),
                setting("VIZIER_CAP_GEO", "Daily limit", "The most house points one person can earn from this game in a day - that is, from winning matches. The geo points the game keeps alongside are never capped, so the board goes on counting after this is full. 100 or more means no limit.", number(0, 100, "points a day"), "15"),
                setting("VIZIER_GEO_IDLE_MINUTES", "Replace a round after", "How long a photo nobody places and nobody skips stays up before the bot says where it was and puts a new one up by itself.", number(1, 1440, "minutes"), "10"),
                setting("VIZIER_GEO_NO_REPEAT_DAYS", "Don't repeat a photo for", "How long a photo is held back before it can come round again.", number(0, 365, "days"), "30"),
                setting("VIZIER_GEO_BUMP_MESSAGES", "Messages before the card moves", "How many messages from other people have to land under the card before it is posted again at the bottom and the old copy deleted.", number(1, 100, "messages"), "5"),
                setting("VIZIER_GEO_BUMP_SECONDS", "Wait between moves", "The shortest time between two of those moves, however busy the channel gets. Both this and the message count have to be met.", number(10, 3600, "seconds"), "120"),
            ],
            commands: vec![
                command("geo", EVERYONE, "/geo", "The place that is up now, sent privately: whether the hint has gone, and how you have done today. Only you see it."),
                command("geotop", EVERYONE, "/geotop", "The geo points board, today or this month: the top 10 and, if you are not on it, your own line. Geo points are the game's own score - every round at what it was worth, hint taken off but no daily limit - so they keep counting after the house-points limit is full, and mods and Muggles have them too."),
                command("geohelp", EVERYONE, "/geohelp", "How Geo works, written from the settings as they are right now: what each kind of answer pays, and what !hint and !skip do. Only you see it."),
                command("geoskip", ADMINS, "/geoskip", "Drops the photo that is up, with no points for anyone, says where it was and puts a fresh one up at once. Unlike !skip, no hint is needed first."),
                command("geostop", ADMINS, "/geostop", "Switches Geo off: the card comes down and no new photos go up. Switch Game on back on to play again."),
            ],
        },
        Section {
            id: "chess",
            title: "Chess",
            icon: "♟️",
            about: "Chess in its own channel (Game channel below), played only with the bot's cards, buttons and \
                    pop-ups: members can't type there, so deny Send Messages for @everyone and let the bot send, \
                    embed, attach files, read history and manage messages. Anyone presses the \
                    ⚔️ Challenge someone button - it sits on the idle card and on every game card - and picks a member \
                    and a pace from menus, with no typing anywhere; `/chess @member` does the same thing from \
                    outside the channel. Either way a challenge card goes up; the challenged member presses Accept, colours are drawn at random, and the game card goes \
                    up with the board drawn on it. Moves come either from a private web page - a link each player \
                    gets to their own side, which needs the panel's web address to be set - or from the Type move \
                    pop-up, which takes both `Nf3` and `g1f3`. Full chess: castling, en passant, promotion, \
                    checkmate, stalemate, threefold repetition, the fifty-move rule, too little material, draw \
                    offers, resignation, and losing on the clock. One card is always the channel's last message, \
                    and it moves back to the bottom when anything lands below it: several games can run side by side, each with its own card that keeps working where it is, and the card of whichever game moved last is the one at the bottom. Anyone can follow a game at its 👀 Watch address, a page that needs no sign-in, shows no private link and cannot move a piece; when the game ends the same address becomes a replay to step through. A restart is safe: the bot notes that it is alive every half minute, and on waking it hands every running game back the time it was away, mentions that on the card, and judges no clock at all for the grace below. Chess pays NO house points: it keeps chess \
                    points of its own, which nothing limits at all. The winner takes them, or both sides on a draw, \
                    whichever houses the two are in, and mods and anyone not yet sorted score them like everybody \
                    else; `/chesstop` is the board. The same pair is scored for one game a day, and a game given up \
                    in the first few moves scores nothing at all. Never runs in #safe-corner.",
            settings: vec![
                setting("VIZIER_CHESS", "Chess on", "Run chess in its channel. Off stops new challenges and games; games already running keep their cards until they finish.", Kind::Toggle, "off"),
                setting("VIZIER_CHESS_CHANNEL", "Game channel", "The channel chess lives in. Empty means chess is off. Never #safe-corner.", Kind::Channel, ""),
                setting("VIZIER_CHESS_CASUAL_HOURS", "Casual: hours per move", "How long each move may take in a casual game, meant to be played over a day or two. The player is tagged once when under two hours are left.", number(1, 72, "hours"), "12"),
                setting("VIZIER_CHESS_LIVE_SECONDS", "Live: seconds per move", "How long each move may take in a live game, meant to be played out in one sitting.", number(30, 3600, "seconds"), "180"),
                setting("VIZIER_CHESS_MAX_GAMES", "Games at once", "How many games one member may have running. A challenge to or from someone already at the limit is refused.", number(1, 20, "games"), "3"),
                setting("VIZIER_CHESS_MAX_ACTIVE", "Games in the channel", "How many games the whole channel may have running at once, whoever is playing. A new challenge is refused politely once it is reached.", number(1, 50, "games"), "8"),
                setting("VIZIER_CHESS_REPLAY_DAYS", "Keep replays for", "How long a finished game stays readable at its Watch address, where anyone can step through the moves. 0 turns replays off and takes the replay link off the result card.", number(0, 365, "days"), "30"),
                setting("VIZIER_CHESS_GRACE_SECONDS", "Grace after a restart", "Nobody can lose a game on time within this long of the bot starting up. Time the bot was away is handed back to every running game anyway, so a restart never costs anybody a game.", number(0, 3600, "seconds"), "60"),
                setting("VIZIER_CHESS_MIN_MOVES", "Half-moves before a resignation pays", "A game given up before this many half-moves have been played scores for nobody, so two friends can't farm chess points by resigning at once. Losing on time always scores for the winner. 0 turns the rule off.", number(0, 200, "half-moves"), "10"),
                setting("VIZIER_POINTS_CHESS_WIN", "Chess points for winning", "Chess points for the winner of a game. Chess points are the game's own score, never capped and never part of the House Cup.", number(0, 100, "points"), "4"),
                setting("VIZIER_POINTS_CHESS_DRAW", "Chess points for a draw", "Chess points for each player when a game is drawn. Chess points are the game's own score, never capped and never part of the House Cup.", number(0, 100, "points"), "1"),
            ],
            commands: vec![
                command(
                    "chess",
                    EVERYONE,
                    "/chess member: time:",
                    "Challenges a member to chess in the chess channel, casual (hours per move) or live (minutes per move). With nobody named it shows your running games and your private board links instead.",
                ),
                command(
                    "chesstop",
                    EVERYONE,
                    "/chesstop",
                    "The chess points board, today or this month: the top 10 and, if you are not on it, your own line. Chess points are the game's own score - a win or a draw at what it was worth, with no daily limit - and they are not house points, so mods and Muggles have them too. Only you see it.",
                ),
                command(
                    "chesshelp",
                    EVERYONE,
                    "/chesshelp",
                    "Explains chess from these settings: challenges, the two time controls, the board page and typed moves, resigning and draws, and what a game is worth. Only you see it.",
                ),
                command(
                    "chessstop",
                    ADMINS,
                    "/chessstop game:",
                    "Cancels a chess game that is stuck, by its number. No chess points for either player, and the card is replaced with a cancelled result.",
                ),
            ],
        },
        Section {
            id: "puzzle",
            title: "Chess puzzle",
            icon: "🧩",
            about: "A real position from a real game, always waiting in the CHESS channel (the puzzle has no channel of \
                    its own - it goes wherever chess is). One side has just blundered; members press 🧩 Solve it, get a \
                    private board page of their own, and play the winning line move by move. A wrong move is refused - \
                    \"that's not it\" - and they try again, as often as they like; the puzzle is solved when the whole \
                    line is played, or when they find a mate of their own, because any move that mates is the right \
                    move. The FIRST person to crack each puzzle takes a house point, if house points are switched on \
                    for this at all - by default they are NOT, because a chess engine would find any of these \
                    instantly and the bot cannot tell whether one was used. Everybody who solves it, first or not, \
                    scores PUZZLE POINTS: this game's own score, which nothing limits and everyone has, mods and \
                    Muggles included. `/puzzletop` is the board. Once somebody cracks it the puzzle stays up a while \
                    longer so the rest still get their go; one nobody solves is replaced with its answer shown. The \
                    puzzles come from the Lichess puzzle database (CC0), shipped in `puzzlebank/`: no bank, no \
                    puzzle, and the rest of chess is unaffected. The card lives below chess's own and never runs in \
                    #safe-corner.",
            settings: vec![
                setting("VIZIER_PUZZLE", "Puzzle on", "Run the chess puzzle in the chess channel. Off takes the card down and sets no more puzzles; chess itself is unaffected either way.", Kind::Toggle, "off"),
                setting("VIZIER_POINTS_PUZZLE", "House points for cracking it first", "House points for the FIRST person to solve each puzzle. Nobody else ever earns house points from one. The daily limit above decides whether any of this is paid at all - it is nought by default, which means none is.", number(0, 100, "points"), "1"),
                setting("VIZIER_PUZZLE_NEXT_MINUTES", "Stays up after the first solve", "How long a cracked puzzle stays up before the next one takes its place, so everybody else still gets their go at it.", number(1, 1440, "minutes"), "10"),
                setting("VIZIER_PUZZLE_IDLE_MINUTES", "Replace an unsolved one after", "How long a puzzle nobody has solved stays up. When it goes, its card shows the answer.", number(5, 10080, "minutes"), "60"),
                setting("VIZIER_PUZZLE_NO_REPEAT_DAYS", "Don't repeat a puzzle for", "How long before the same puzzle may be set again. 0 turns the rule off.", number(0, 3650, "days"), "60"),
                setting("VIZIER_PUZZLE_BUMP_MESSAGES", "Messages before the card moves", "How many messages from other people have to land under the puzzle card before it follows the conversation down. The bot's own messages never count.", number(1, 100, "messages"), "5"),
                setting("VIZIER_PUZZLE_BUMP_SECONDS", "Wait between moves of the card", "The shortest time between two moves of the puzzle card, however busy the channel is.", number(10, 3600, "seconds"), "120"),
            ],
            commands: vec![
                command("puzzle", EVERYONE, "/puzzle", "The puzzle that is up now: whose move it is, how long the line is, who cracked it first, and your own puzzle points today and this month. Only you see it."),
                command("puzzletop", EVERYONE, "/puzzletop", "The puzzle points board, today or this month: the top 10 and, if you are not on it, your own line. Puzzle points are the game's own score - every solve, first or not, with no daily limit - so mods and Muggles have them too. Only you see it."),
                command("puzzlehelp", EVERYONE, "/puzzlehelp", "How the chess puzzle works, written from the settings as they are right now: the private board, what a solve scores, the pace, and where the puzzles come from. Only you see it."),
                command("puzzleskip", ADMINS, "/puzzleskip", "Drops the puzzle that is up, shows its answer on its card and sets a fresh one at once."),
            ],
        },
        Section {
            id: "duel",
            title: "Letter Duel",
            icon: "🔠",
            about: "A tile game for two to four people in its own channel (Game channel below): a fifteen-by-fifteen \
                    board with the usual premium squares, the usual hundred tiles, and seven on a rack. `/duel`, or \
                    the ⚔️ Start a duel button on the card, puts a LOBBY up with a Join button and a countdown the bot \
                    rewrites every few seconds — Discord's own relative timestamps do not tick where anyone is \
                    looking, so the seconds are written out. The lobby starts early when it fills and is called off, \
                    with nothing lost, below the minimum. Each player then gets a private page of their own, the way \
                    chess does: their rack along the bottom, tap a tile then a square, tap it again to pick it back \
                    up, and the page adds up what the play would score BEFORE it is committed and refuses an illegal \
                    one in plain words. The server checks everything again, so the page is never trusted. Full rules: \
                    the first word crosses the middle, tiles in one line with no gaps, every word made — sideways \
                    ones too — has to be in the dictionary, premium squares multiply the letter and then the word and \
                    are spent after use, all seven tiles is +50, blanks are any letter and score nothing, and a \
                    player may swap tiles while the bag is full enough or simply pass. It ends when the bag is empty \
                    and somebody puts their last tile down — they gain what everyone else is holding and the rest \
                    lose theirs — or when everybody passes twice. After every turn the one card in the channel is \
                    redrawn with the board as a picture, the word just played, the scores and whose turn it is; it \
                    follows the conversation down when chat buries it. **The words come from wordbank/dictionary.txt \
                    in the bot's workspace**, read once at start: with no bank there the game simply stays off and \
                    says so in the log. That file starts at four letters, so the two- and three-letter words a board \
                    needs are built into the bot itself. A restart is safe: the bot notes that it is alive every half \
                    minute and hands every running game and open lobby back the time it was away. Never runs in \
                    #safe-corner.",
            settings: vec![
                setting("VIZIER_DUEL", "Game on", "Run Letter Duel in its channel. Off stops new lobbies; a game already running keeps its card until it finishes. The game also stays off, whatever this says, when there is no dictionary to play with.", Kind::Toggle, "off"),
                setting("VIZIER_DUEL_CHANNEL", "Game channel", "The channel Letter Duel lives in. The bot needs Send Messages, Attach Files, Embed Links, Read Message History and Manage Messages there. Empty means the channel the game was made for. Never #safe-corner.", Kind::Channel, "1550088683414225006"),
                setting("VIZIER_DUEL_LOBBY_SECS", "Lobby stays open for", "How long a lobby takes seats before it starts. It starts early the moment it is full, and is called off if too few have joined by the end.", number(15, 1800, "seconds"), "120"),
                setting("VIZIER_DUEL_MIN", "Fewest players", "How many have to join for a game to start at all. Below this the lobby is called off and nobody loses anything.", number(2, 4, "players"), "2"),
                setting("VIZIER_DUEL_MAX", "Most players", "How many seats a game has. The lobby starts the moment they are all taken.", number(2, 4, "players"), "4"),
                setting("VIZIER_DUEL_TURN_SECS", "Seconds per turn", "How long each player has to play, swap or pass. A turn that runs out is an automatic pass, and three missed turns take that player out of the game.", number(20, 3600, "seconds"), "240"),
                setting("VIZIER_DUEL_BUMP_MESSAGES", "Messages before the card moves", "How many messages from other people have to land under the card before it is posted again at the bottom and the old copy deleted.", number(1, 100, "messages"), "5"),
                setting("VIZIER_DUEL_BUMP_SECONDS", "Wait between moves", "The shortest time between two of those moves, counted from when the card last MOVED rather than from every redraw, so a game whose board changes each turn still follows the conversation down.", number(10, 3600, "seconds"), "60"),
                setting("VIZIER_POINTS_DUEL_WIN", "House points for winning", "House points for the winner of a game with three or more players. A two-player game pays its winner the runner-up's share instead, and the loser nothing, so two friends can't farm each other.", number(0, 100, "points"), "4"),
                setting("VIZIER_POINTS_DUEL_SECOND", "House points for second", "House points for the runner-up, in a game with three or more players.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_DUEL_PLAYED", "House points for playing to the end", "House points for everybody else who was still in the game when it finished, so turning up pays. Somebody who left, or was dropped for missing turns, gets nothing.", number(0, 100, "points"), "1"),
            ],
            commands: vec![
                command("duel", EVERYONE, "/duel", "Opens a Letter Duel lobby in the game channel and takes a seat in it. Refused politely while a lobby or a game is already running."),
                command("dueltop", EVERYONE, "/dueltop", "The duel points board, today or this month: the top 10 and, if you are not on it, your own line. Duel points are the game's own score — what each finish was worth before the daily house-points limit — so they keep counting after that limit is full, and mods and Muggles have them too. Only you see it."),
                command("duelhelp", EVERYONE, "/duelhelp", "How Letter Duel works, written from the settings as they are right now: the lobby, the board page, the words, the scoring, the clock and the points. Only you see it."),
                command("duelstop", ADMINS, "/duelstop", "Stops the Letter Duel game that is running, or closes the lobby that is open. No points for anyone, and nobody loses anything from a closed lobby."),
            ],
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
                toggle("VIZIER_SCOREBOARD", "Scoreboard card", "Post the House Cup scoreboard card (each house's points this month with bars, the last hour's gains, today's totals, and buttons for top scorers, your house, your points and how to earn) in the scoreboard channel. It is always kept as the last message there: anything posted in the channel moves it back to the bottom a few seconds later."),
                setting("VIZIER_SCOREBOARD_CHANNEL", "Scoreboard channel", "Where the scoreboard card and the welcome, Snitch & cards and guide posts above it go, e.g. a gaming updates channel. The bot needs Send Messages, Attach Files, Embed Links, Read Message History and Manage Messages there. Empty turns them all off.", Kind::Channel, ""),
                toggle("VIZIER_SCOREBOARD_WELCOME", "Welcome post", "Keep a welcome message first in the scoreboard channel: the houses, the common rooms and where the action is, with live channel links. Edited by itself when a setting it mentions changes."),
                toggle("VIZIER_SCOREBOARD_SNITCH_CARDS", "Snitch & cards post", "Keep a post on the Golden Snitch and Chocolate Frog cards, with two how-to pictures, under the welcome. Written from the live drop, points and card settings and edited by itself when they change; the frog and card parts are left out while frog drops are off."),
                toggle("VIZIER_SCOREBOARD_GEO", "Geo post", "Keep a post about Geo above the scoreboard card: where the game is, what naming a state or a town is worth, and what winning a match pays. It comes down by itself while Geo is off or has no channel. Geo has a post of its own because the welcome message is already at Discord's length limit."),
                toggle("VIZIER_SCOREBOARD_GUIDE", "Beginner's guide", "Keep the How to play guide above the scoreboard card: chat and voice points, the games and their limits, and the commands to check progress, all written from the live settings and edited by itself when they change."),
                setting("VIZIER_SCOREBOARD_EVERY_HOURS", "Scoreboard every", "How often the scoreboard card is posted, counted from the first hour below.", number(1, 24, "hours"), "1"),
                setting("VIZIER_SCOREBOARD_FIRST_HOUR", "Scoreboard first hour", "The first India hour the scoreboard covers: 10 covers 10:00-11:00 and posts at 11:00.", number(0, 23, "hour"), "10"),
                setting("VIZIER_SCOREBOARD_LAST_HOUR", "Scoreboard last hour", "The last India hour the scoreboard covers: 23 posts at midnight.", number(0, 23, "hour"), "23"),
                toggle("VIZIER_SCOREBOARD_REPLACE", "Scoreboard replaces itself", "Delete the previous scoreboard card when a new one goes up, so the channel shows only the latest. Off stacks them."),
                toggle("VIZIER_SCOREBOARD_QUIET_SKIP", "Skip quiet hours", "Don't post the scoreboard for an hour in which no house's points moved."),
                toggle("VIZIER_LEAD_CARD", "Lead change card", "Post a card in the houses channel when a different house takes the lead in this month's House Cup."),
                setting("VIZIER_LEAD_CARD_GAP_MINUTES", "Lead card cooldown", "Least time between two lead cards, so a close race doesn't flood the channel.", number(0, 1440, "minutes"), "30"),
                setting("VIZIER_LEAD_CARD_MIN_POINTS", "Lead card from", "No lead card until the leading house has at least this many points in the month (quiet early days).", number(0, 10000, "points"), "20"),
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
            commands: vec![command(
                "guiderefresh",
                ADMINS,
                "/guiderefresh",
                "Posts the scoreboard channel's welcome, Snitch & cards post, guide and scoreboard card again, in that order, removing the old copies. Only you see the reply.",
            )],
        },
        Section {
            id: "daily",
            title: "Daily posts",
            icon: "🗞️",
            about: "Each topic channel gets a long card at its times (11:00 and 19:00 India time unless changed): a story, news piece, poem \
                    or quote. Stories and news are written by the AI only from a real source - a Wikipedia article or a \
                    news article from a feed - which is linked on the card, and a second AI call checks the draft against \
                    that source; a draft that still claims something the source doesn't say after one correction is \
                    dropped and the slot tries again later. Poems and quotes come from a hand-picked list with the \
                    author named. Nothing is repeated until a list runs out. A post missed while the bot was down still \
                    goes out within the catch-up time. The first time the bot starts with a topic that has never \
                    posted, that topic posts once straight away. /dailypost previews any topic privately, or posts it now.",
            settings: {
                let mut settings = vec![
                    toggle("VIZIER_DAILY", "Daily posts on", "Off stops every topic's scheduled posts; /dailypost still works."),
                    setting(
                        "VIZIER_DAILY_MODEL",
                        "Model that writes them",
                        "A model on the bot's own provider for writing and fact-checking the posts. \"agent\" uses the bot's usual model.",
                        Kind::Text,
                        super::super::daily::DEFAULT_MODEL,
                    ),
                    setting(
                        "VIZIER_DAILY_CATCH_UP_HOURS",
                        "Catch-up time",
                        "How long after its time a missed post may still go out. Past this, that post is skipped.",
                        number(0, 23, "hours"),
                        "3",
                    ),
                ];
                for desk in super::super::daily::DESKS {
                    settings.push(toggle(desk.on_key, desk.title, desk.about));
                    settings.push(setting(desk.channel_key, desk.channel_label, "Where this topic posts.", Kind::Channel, desk.channel));
                    settings.push(setting(
                        desk.times_key,
                        desk.times_label,
                        "India times it posts each day, e.g. 09:00,20:00. One time means once a day.",
                        Kind::Times,
                        desk.times,
                    ));
                }
                settings
            },
            commands: vec![command(
                "dailypost",
                ADMINS,
                "/dailypost desk: post:",
                "Writes a topic's post now. Without post:true only you see a preview and nothing is marked used; with \
                 post:true it goes into the topic's channel and counts as the slot that is due.",
            )],
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
                toggle("VIZIER_WORDLE_POINTS", "Wordle points", "Pay house points from the Wordle app's daily results post (the day before's scores)."),
                setting("VIZIER_POINTS_WORDLE_1_2", "Wordle in 1-2", "Points for solving Wordle in one or two guesses.", number(0, 100, "points"), "4"),
                setting("VIZIER_POINTS_WORDLE_3", "Wordle in 3", "Points for solving Wordle in three guesses.", number(0, 100, "points"), "3"),
                setting("VIZIER_POINTS_WORDLE_4", "Wordle in 4", "Points for solving Wordle in four guesses.", number(0, 100, "points"), "2"),
                setting("VIZIER_POINTS_WORDLE_5_6", "Wordle in 5-6", "Points for solving Wordle in five or six guesses.", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_WORDLE_CROWN", "Wordle crown bonus", "Extra points for the day's best Wordle score (the 👑 in the results).", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_KOTO_WIN", "Koto solved", "Points for the person who solves a Koto game.", number(0, 100, "points"), "3"),
                setting("VIZIER_POINTS_KOTO_PLAYED", "Koto played", "Points for each other player whose guesses scored at least one point in a Koto game, solved or not. Guesses that score +0 earn nothing.", number(0, 100, "points"), "1"),
                setting("VIZIER_POINTS_ANAGRAM", "Anagram Bot solved", "Points for solving one of the third-party Anagram Bot's puzzles. The bot's own Anagrams game (its own section) pays by word length instead, and is never paid for twice: while that game is running in a channel, an Anagram Bot solve there pays nothing. Both share the daily anagram limit.", number(0, 100, "points"), "3"),
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
                setting("VIZIER_CHAT_DAY_MESSAGES", "1st chat point at", "Messages in one India day that earn the first chat point.", number(1, 5000, "messages"), "20"),
                setting("VIZIER_CHAT_TIER2_MESSAGES", "2nd chat point at", "Messages in the day for the second chat point. Set it at or below the first to switch this tier off.", number(1, 5000, "messages"), "60"),
                setting("VIZIER_CHAT_TIER3_MESSAGES", "3rd chat point at", "Messages in the day for the third chat point. Set it at or below the second to switch this tier off.", number(1, 5000, "messages"), "150"),
                setting("VIZIER_VOICE_DAY_MINUTES", "Minutes per voice point", "Each full block of this many minutes in voice in a day earns a voice point, up to the daily limit.", number(10, 1440, "minutes"), "60"),
                setting(
                    "VIZIER_VOICE_HOUR_POINTS",
                    "Voice hour ladder",
                    "What each hour in voice pays, in order, separated by commas. \"1,2,3,4\" means the first hour \
                     pays 1, the second 2, the third 3 and the fourth 4. An hour past the end of the list pays what \
                     the last one paid, and the daily limit decides where it stops.",
                    Kind::Text,
                    "1,2,3,4",
                ),
                toggle(
                    "VIZIER_VOICE_NEEDS_COMPANY",
                    "Voice needs company",
                    "Only count voice time spent with at least one other person in the room (bots don't count). Off counts time alone too.",
                ),
                toggle(
                    "VIZIER_VOICE_IGNORE_DEAFENED",
                    "Deafened time doesn't count",
                    "Time a person spends deafened (by themselves or by a moderator) earns them no voice time, and they don't count as company for anyone else while deafened. Muted still counts. Needs the bot's own voice-state events, so only time since this was added is known.",
                ),
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
                setting("VIZIER_STATS_VOICE_CHATS", "Count voice channel chats", "Count messages typed in any voice channel's text chat, temporary rooms included, towards chat stats and chat points, even though those channels aren't on the bot's channel list. The bot never replies or stores the text there. Excluded channels still don't count.", Kind::Toggle, "on"),
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
                setting("VIZIER_QUIZ_RESUME_MAX_MINUTES", "Pick a round back up within", "After a restart the quiz carries on the round it was in - same scores, same count, the question that was on screen asked again. A round older than this is quietly abandoned and a fresh one starts instead.", number(1, 1440, "minutes"), "15"),
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
                toggle("VIZIER_QUIZ_FREAKY", "Freaky (18+) genre", "Offer the 🌶️ Freaky (18+) genre in the vote: sex ed, kinks, dating and weird sex facts. Only ever offered when Discord has the quiz channel marked age-restricted, and its questions never appear in the mix."),
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
                    "Daily battle times",
                    "India times a lobby opens each day - add as many as you like, one battle each. If a fight is running at a time, that lobby opens as soon as the arena is free (up to 30 minutes late).",
                    Kind::Times,
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
                    It works even while the bot is in admin-only mode. Every fight it calls is listed on the panel's \
                    Kalesh page, where a mod can also look up two members and ask for a neutral summary.",
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
                setting(
                    "VIZIER_KALESH_SUMMARY_MODEL",
                    "Summary model",
                    "The model that writes a fight's summary on the Kalesh page, on the bot's own provider. Empty means the bot's main model: a mod may act on it, so quality matters more than cost.",
                    Kind::Text,
                    "",
                ),
                setting(
                    "VIZIER_KALESH_SUMMARY_MAX_MESSAGES",
                    "Messages per summary",
                    "Most messages the summary model is shown. A longer stretch is trimmed to how it started, its busiest part and how it ended, and the summary says so.",
                    number(20, 5000, "messages"),
                    "400",
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
                    can be merged so their history adds up. Admins can also set a special welcome for one member on the \
                    panel's Welcomes page: a message of their own that posts the moment that member joins, for someone \
                    the server is waiting on to come back.",
            settings: vec![
                toggle("VIZIER_WELCOME", "Welcome messages", "Greet people who join. Off posts no greeting; newcomers are still sorted into a house there."),
                toggle(
                    "VIZIER_SPECIAL_WELCOMES",
                    "Special welcomes",
                    "Post the special welcomes set for particular members on the Welcomes page when they join. These go out even with the usual welcome messages off. Off keeps them saved but posts none.",
                ),
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
            settings: vec![toggle(
                "VIZIER_MEMBER_REMINDERS",
                "Members' own reminders",
                "Let members set their own reminders by asking the bot (\"@Loduchand remind me in 2 hours to…\") or with /remind. Off stops new ones and pauses sending.",
            )],
            commands: vec![
                command(
                    "remind",
                    EVERYONE,
                    "/remind when: what:",
                    "Set yourself a reminder (in 2 hours, 30m, at 9pm, tomorrow 9am, 2026-09-20 18:00 - India time); the bot pings you in that channel. You can also just ask the bot in chat. Up to 20 waiting, 60 days ahead.",
                ),
                command("reminders", EVERYONE, "/reminders", "See your waiting reminders and cancel any of them. Private."),
            ],
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
            id: "about",
            title: "About members",
            icon: "🪪",
            about: "A few bullet points about a member, answered at once and at no cost: no model is asked when \
                    someone asks. First the facts, read from the bot's own records when asked - member since, messages \
                    this month and all time, their three main channels, the games they play most and how they do there, \
                    house and points this month with their place in the house, frog cards, voice time this month, who \
                    they talk with most, and a favourite emoji and phrase. Then \"what they're like\": four to six \
                    bullets a cheap model wrote ahead of time from a sample of their own messages (never #safe-corner), \
                    and stored. It never mentions health, religion or caste, politics, sexuality or gender, \
                    relationships, family, where they live, study or work, or when they are online - the model is told \
                    so and every bullet is checked again afterwards, along with anything quoted word for word or \
                    unkind. A member can see only their own; mods and admins can look anyone up, and every such lookup \
                    is written to the activity log. Asking in chat (@Loduchand about @someone, tell me about @someone, \
                    who is @someone) answers in the asker's DMs. The first build starts when this is switched on and \
                    runs once a day, within the cap, until everyone who qualifies is covered; after that a weekly run \
                    rebuilds only members with enough new messages. Every run's model calls and tokens are logged and \
                    shown on each member's page, where a mod can also rebuild or clear their notes.",
            settings: vec![
                setting(
                    "VIZIER_NOTES",
                    "Member notes on",
                    "Switches on /about, the chat ask and the builds. Off: /about says it's off, the chat ask goes to \
                     the AI as before and nothing is built. /forgetme works either way.",
                    Kind::Toggle,
                    "off",
                ),
                setting(
                    "VIZIER_NOTES_MODEL",
                    "Model that writes them",
                    "A model on the bot's own provider for writing \"what they're like\" - the same cheap one Name \
                     Place Animal Thing judges with unless changed. \"agent\" uses the bot's usual model.",
                    Kind::Text,
                    "google/gemini-2.5-flash-lite",
                ),
                setting(
                    "VIZIER_NOTES_MIN_MESSAGES",
                    "Least messages to write about",
                    "Someone with fewer usable messages than this (one-word replies and bot commands don't count) is \
                     never sent to the model; their answer says there isn't enough to go on yet.",
                    number(20, 10_000, "messages"),
                    "100",
                ),
                setting(
                    "VIZIER_NOTES_NEW_MESSAGES",
                    "New messages for a rebuild",
                    "The weekly run rebuilds someone's notes only once they have written at least this many messages \
                     since the last build.",
                    number(1, 10_000, "messages"),
                    "50",
                ),
                setting(
                    "VIZIER_NOTES_MAX_PER_RUN",
                    "Most builds per run",
                    "The cap on model calls in one run, first build and weekly alike. Anyone left over waits for the \
                     next run. 0 stops the builds without switching the rest off.",
                    number(0, 1000, "members"),
                    "30",
                ),
                setting(
                    "VIZIER_NOTES_READ_BUDGET",
                    "Messages read per member",
                    "How much of a member's own writing the model reads, in tokens: about half from their newest \
                     messages and the rest spread over their older ones. The whole call costs this plus about 550 for the instructions and 150 for the answer.",
                    number(500, 30_000, "tokens"),
                    "4000",
                ),
            ],
            commands: vec![
                command(
                    "about",
                    EVERYONE,
                    "/about member:",
                    "Your own notes: the facts and what you're like. Only mods and admins can name someone else, and \
                     that lookup is logged. Only you see the answer.",
                ),
                command(
                    "forgetme",
                    EVERYONE,
                    "/forgetme",
                    "Deletes your notes and keeps you out of every future build. Facts already public elsewhere \
                     (points, cards, games) can still show. Run it again, or press the button, to opt back in.",
                ),
            ],
        },
        Section {
            id: "msglog",
            title: "Message history",
            icon: "🗄️",
            about: "What the server said, kept for the panel only - nothing is posted in Discord. The bot copies every \
                    member message it sees anywhere in the server (text channels, threads, voice channel chats, \
                    temporary rooms) with its pictures, and two pages read it: Messages, where you open a member and \
                    see what they have said across every channel, and Deleted messages, where a deleted message's text \
                    and pictures and an edited one's before and after are shown. Three clocks, because the three \
                    things are kept for different reasons: text is the server's memory, so a year; pictures are only \
                    here so a message deleted soon after it was posted can still be shown, and they are what costs \
                    disk, so a week; the deleted and edited log is a moderation record, so a month. Past the picture \
                    clock a copy keeps every word and loses its pictures. Never #safe-corner or its threads, never a \
                    channel on the skip list, never DMs, never bots. Discord doesn't tell bots who deleted a message, \
                    so the log can't say. Every look at either page is in the activity log.",
            settings: vec![
                toggle(
                    "VIZIER_MSGLOG",
                    "Keep what the server says",
                    "Keep copies of new messages and log deletions and edits. Off stops copying new messages and logging; \
                     what's already kept stays until it's cleared.",
                ),
                setting(
                    "VIZIER_MSGLOG_TEXT_DAYS",
                    "Keep what was said for",
                    "How long the text of a message nobody deleted is kept, and so how far back the Messages page can \
                     look. At about 24,000 messages a day a year takes roughly 3.5 GB on the server. A message deleted \
                     after this shows in the deleted log without its text.",
                    number(7, 3650, "days"),
                    "365",
                ),
                setting(
                    "VIZIER_MSGLOG_SKIP_CHANNELS",
                    "Channels never kept",
                    "Channels whose messages are never copied at all, on top of #safe-corner, which is always left out. \
                     Threads inside a skipped channel are skipped too. Anything already kept from a channel added here \
                     stops being shown at once.",
                    Kind::Channels,
                    "",
                ),
                setting(
                    "VIZIER_MSGLOG_KEEP_DAYS",
                    "Keep pictures for",
                    "How long a message's saved pictures are kept. After this the copy keeps its text and loses its \
                     pictures, so a message deleted later shows its words but not what was attached.",
                    number(1, 90, "days"),
                    "7",
                ),
                setting(
                    "VIZIER_MSGLOG_LOG_DAYS",
                    "Keep the log for",
                    "How long deleted and edited messages stay in the log, pictures included, before they're cleared.",
                    number(1, 365, "days"),
                    "30",
                ),
                setting(
                    "VIZIER_MSGLOG_MAX_IMAGE_MB",
                    "Largest picture saved",
                    "Pictures bigger than this aren't downloaded; the log shows just their name. At most four pictures \
                     are saved from one message, and other files are never downloaded.",
                    number(1, 25, "MB"),
                    "8",
                ),
                setting(
                    "VIZIER_MSGLOG_MAX_GB",
                    "Picture storage limit",
                    "When saved pictures take up more than this on the server, new pictures stop being downloaded (text \
                     is still kept) until old ones are cleared.",
                    number(1, 500, "GB"),
                    "5",
                ),
            ],
            commands: vec![],
        },
        Section {
            id: "automod",
            title: "Moderation",
            icon: "🚨",
            about: "Two quite different things, and it matters which is which.\n\n\
                    SPAM is objective - the same message over and over, twenty messages in four seconds, a mass \
                    ping, an @everyone from someone who isn't a mod, an invite to another server, a wall of one \
                    repeated character. That is counted, not guessed at, so those messages are deleted and one \
                    entry goes to the moderation log saying who, which rule, where, what it said and a link to \
                    the conversation. A whole flood is one entry, not twenty.\n\n\
                    AI-WRITTEN TEXT is not objective, and this is the important part: there is NO reliable way to \
                    tell whether a person or a machine wrote something. Every detector gets it wrong often, and it \
                    gets it wrong hardest on people writing English as a second language - which is most of this \
                    server. Careful, formal, correctly punctuated English written by someone who learned it at \
                    school reads exactly like what a detector calls AI. So a suspicion here NEVER deletes anything \
                    and never punishes anybody, and there is no setting that changes that: it posts a flag in the \
                    log with the reasons in plain words, and a moderator decides with the buttons on it. Every time \
                    a moderator presses Not AI that is counted, and the Moderation page shows how often they have - \
                    which is the only honest measure of whether this half is worth keeping.\n\n\
                    Only messages long enough to judge are scored at all, on free signals first (document-like \
                    layout in chat, em dashes, essay vocabulary, unnaturally even sentences, and how far the \
                    message is from how that member themselves normally writes). A cheap model is asked for a \
                    second opinion only when that score is already high, never twice about the same message, and \
                    never more than a set number of times an hour.\n\n\
                    Nothing here ever touches moderators, bot admins, bots, DMs, #safe-corner, or the game \
                    channels - anagram, guess-the-word, word-chain, sudoku, chess and Name Place Animal Thing - \
                    where people fire the same short word off all day and a flood rule would eat the game. Those \
                    are left alone whatever the exempt list says.",
            settings: vec![
                setting(
                    "VIZIER_AUTOMOD",
                    "Moderation on",
                    "The master switch for both halves below. Off by default, so nothing starts watching until you \
                     say so. While it is on, the bot also quietly learns how each member normally writes, which is \
                     what the AI half compares against - so it is worth switching this on a while before that one.",
                    Kind::Toggle,
                    "off",
                ),
                setting(
                    "VIZIER_AUTOMOD_SPAM",
                    "Delete spam",
                    "The objective rules below: repeats, floods, mass pings, invites and walls. These DO delete the \
                     messages they catch and log what they did. Off by default.",
                    Kind::Toggle,
                    "off",
                ),
                setting(
                    "VIZIER_AUTOMOD_AI",
                    "Flag possibly AI-written messages",
                    "Put long messages that might have been written by an AI in front of a moderator. This NEVER \
                     deletes anything and never punishes anybody, whatever the score - only a moderator pressing \
                     the button on the flag deletes. Off by default.",
                    Kind::Toggle,
                    "off",
                ),
                setting(
                    "VIZIER_AUTOMOD_LOG_CHANNEL",
                    "Moderation log",
                    "Where spam removals and AI flags are posted. Keep it mods-only: it quotes what people wrote. \
                     Empty means nothing is posted anywhere, and only the Moderation page shows what happened.",
                    Kind::Channel,
                    "1516779799865987101",
                ),
                setting(
                    "VIZIER_AUTOMOD_EXEMPT_CHANNELS",
                    "More channels to leave alone",
                    "Extra channels no rule ever acts in. The game channels (anagram, guess-the-word, word-chain, \
                     sudoku, chess, Name Place Animal Thing) and #safe-corner are always left alone and do not need \
                     listing - this only adds to them. The quiz channel is worth adding if members type a lot of \
                     repeated short answers there.",
                    Kind::Channels,
                    "",
                ),
                setting(
                    "VIZIER_AUTOMOD_REPEAT_COUNT",
                    "Repeats before it's spam",
                    "How many near-identical messages from one member inside the window below count as spam - the \
                     classic posting of the same thing in ten channels. All of them are deleted, not just the last. \
                     Messages under 12 characters never count, because \"haan\", \"same\" and \"lol\" repeat \
                     perfectly innocently. 0 switches this rule off.",
                    number(0, 50, "messages"),
                    "3",
                ),
                setting(
                    "VIZIER_AUTOMOD_REPEAT_WINDOW_SECS",
                    "Repeat window",
                    "How far back the repeat rule looks. Repeats spread wider apart than this are not counted \
                     together.",
                    number(5, 3600, "seconds"),
                    "60",
                ),
                setting(
                    "VIZIER_AUTOMOD_BURST_COUNT",
                    "Messages before it's a flood",
                    "How many messages from one member inside the burst window count as a flood, whatever they say. \
                     Keep it high enough that a fast typer having an argument is never caught - someone excited \
                     can fire off six \"lol\"s in a few seconds and must not lose them. 0 switches this rule off.",
                    number(0, 100, "messages"),
                    "8",
                ),
                setting(
                    "VIZIER_AUTOMOD_BURST_SECS",
                    "Flood window",
                    "The few seconds the flood rule counts over. Eight messages in five seconds is a paste or a \
                     script, not typing.",
                    number(1, 600, "seconds"),
                    "5",
                ),
                setting(
                    "VIZIER_AUTOMOD_MAX_MENTIONS",
                    "Mentions before it's a mass ping",
                    "How many different members and roles one message may ping before it is deleted. 0 switches this \
                     rule off.",
                    number(0, 50, "mentions"),
                    "6",
                ),
                toggle(
                    "VIZIER_AUTOMOD_EVERYONE",
                    "Delete @everyone from non-mods",
                    "Delete a message from someone who isn't a moderator that says @everyone or @here. Whether \
                     Discord actually let the ping through doesn't matter: typing it is the same act.",
                ),
                toggle(
                    "VIZIER_AUTOMOD_INVITES",
                    "Delete invites to other servers",
                    "Delete discord.gg and discord.com/invite links posted by anyone who isn't a moderator. Talking \
                     about invites without posting one is left alone.",
                ),
                setting(
                    "VIZIER_AUTOMOD_WALL_CHARS",
                    "Wall length",
                    "How long a message has to be before it can count as a wall of one repeated character or word. \
                     An ordinary long message is never a wall - it is the repetition that makes one. 0 switches this \
                     rule off.",
                    number(0, 4000, "characters"),
                    "400",
                ),
                setting(
                    "VIZIER_AUTOMOD_WALL_PCT",
                    "Wall sameness",
                    "How much of that message has to be the same character (or the same short word) for it to count \
                     as a wall.",
                    number(10, 100, "%"),
                    "70",
                ),
                setting(
                    "VIZIER_AUTOMOD_QUIET_SECS",
                    "One log entry per member every",
                    "After an entry about a member, messages caught in the next stretch are still deleted but don't \
                     each get their own post - so one flood is one entry in the log instead of twenty.",
                    number(0, 3600, "seconds"),
                    "60",
                ),
                setting(
                    "VIZIER_AUTOMOD_AI_MIN_CHARS",
                    "Shortest message judged",
                    "Messages shorter than this are never scored for AI writing at all. Short text carries no \
                     evidence either way, and pretending otherwise is how detectors end up accusing people who \
                     simply write briefly.",
                    number(120, 4000, "characters"),
                    "280",
                ),
                setting(
                    "VIZIER_AUTOMOD_AI_THRESHOLD",
                    "Flag at",
                    "How high the free score has to be before a message is put in front of a moderator, from 0 to 1. \
                     At least two different kinds of signal must agree as well, so one em dash never flags anybody. \
                     Lower means more flags and more of them wrong; the Moderation page shows how often moderators \
                     said so.",
                    Kind::Decimal { min: 0.2, max: 1.0, unit: "" },
                    "0.62",
                ),
                setting(
                    "VIZIER_AUTOMOD_MODEL",
                    "Second-opinion model",
                    "The model asked to look again at a message that already crossed the threshold, on the bot's own \
                     provider. It is given the message, samples of that member's own normal writing, and a rubric \
                     telling it plainly that second-language English is not evidence and short text cannot be \
                     judged. Type agent to use the bot's usual model, or none to skip the second opinion entirely.",
                    Kind::Text,
                    "google/gemini-2.5-flash-lite",
                ),
                setting(
                    "VIZIER_AUTOMOD_AI_MAX_HOUR",
                    "Second opinions an hour",
                    "The most times an hour the model is asked, so a bad day can't run up a bill. Past it, flags \
                     still go up with the free signals alone and say the model wasn't asked. The same message is \
                     never sent twice.",
                    number(0, 10_000, "calls"),
                    "30",
                ),
                setting(
                    "VIZIER_AUTOMOD_KEEP_DAYS",
                    "Keep flagged text for",
                    "How long the words of a flagged message are kept. After this the text is cleared but the row \
                     stays, so what the feature did - and how often a moderator said it was wrong - is never lost.",
                    number(1, 365, "days"),
                    "30",
                ),
            ],
            commands: vec![command(
                "🗑️ Delete it · ✅ Not AI · 👤 This member's flags",
                ADMINS,
                "Buttons under a flag in the moderation log",
                "Delete it removes the message and records that a human decided to; Not AI dismisses the flag and \
                 records that this feature got it wrong, which is counted on the Moderation page; This member's \
                 flags shows their last ten, privately. Only moderators can press them.",
            )],
        },
        Section {
            id: "announce",
            title: "Announcements",
            icon: "📣",
            about: "The bot tells everyone what changed, so nobody has to write a post. Whenever a game is switched \
                    on or off, or points, limits or how often something happens are edited here, it waits for the \
                    changes to stop and then posts one gold “📣 What's new” card in each channel below, listing \
                    exactly what moved and what it was before - a run of edits becomes one message, never one per \
                    click. Notes about a brand-new game or feature are written into the bot and posted the same way, \
                    once each. Where a player looks things up doesn't change: the welcome, the guide and the rules in \
                    the House Cup channel are still edited in place to match. Nothing is ever pinged, and a restart \
                    in the middle can't say the same thing twice.",
            settings: vec![
                toggle(
                    "VIZIER_ANNOUNCE",
                    "Announcements on",
                    "Off stops both the automatic “what's new” cards and the notes about new features. The settings \
                     are still followed - only the telling stops, and changes made while it's off are never announced \
                     later.",
                ),
                setting(
                    "VIZIER_ANNOUNCE_CHANNELS",
                    "Where to announce",
                    "Every channel a “what's new” card goes to. The five meant here are the four house common rooms \
                     and the houses channel: 1548413022744350732, 1548413025470390332, 1548413028117258321, \
                     1548413030738436117, 1548371226890604665. Empty means nothing is announced anywhere. A channel \
                     the bot can't post in is warned about in the log and tried twice more, then left.",
                    Kind::Channels,
                    "",
                ),
                setting(
                    "VIZIER_ANNOUNCE_DELAY_MINUTES",
                    "Wait for quiet",
                    "How long the settings must sit still before the card goes out, so a session of edits on this \
                     panel becomes one message instead of a dozen. Higher means later but tidier.",
                    number(1, 240, "minutes"),
                    "10",
                ),
                setting(
                    "VIZIER_ANNOUNCE_MIN_GAP_MINUTES",
                    "Least gap between announcements",
                    "No automatic announcement goes out within this long of the last one, whatever else changes in \
                     between - the changes simply wait and go together.",
                    number(0, 1440, "minutes"),
                    "30",
                ),
            ],
            commands: vec![command(
                "announce",
                ADMINS,
                "/announce text:",
                "Posts your own message as a gold card in the same channels, in your own words. Only you see the \
                 reply, which says which channels took it. It's written to the activity log with your name.",
            )],
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
        // A game with one setting per topic keeps the keys in a table and reads
        // them through a field: `on_key: "VIZIER_DAILY_BOOKS"` declared, then
        // `control::on(desk.on_key, ..)`. A key declared that way counts as read
        // only when that very field is passed to a reader in the same file, so
        // a key that is declared and never read still fails.
        let field_read = regex::Regex::new(r#"control::(?:var|id|ids|number|float|on)\(\s*[a-z_]+\.([a-z_]+_key)\b"#).unwrap();
        let field_decl = regex::Regex::new(r#"\b([a-z_]+_key):\s*"(VIZIER_[A-Z0-9_]+)""#).unwrap();
        let mut used = std::collections::BTreeSet::new();
        for (path, text) in &files {
            if path.ends_with("catalog.rs") {
                continue;
            }
            used.extend(read.captures_iter(text).map(|c| c[1].to_string()));
            let fields: std::collections::HashSet<String> = field_read.captures_iter(text).map(|c| c[1].to_string()).collect();
            used.extend(field_decl.captures_iter(text).filter(|c| fields.contains(&c[1])).map(|c| c[2].to_string()));
        }
        assert!(used.len() > 50, "the scan found only {} settings - has the reading style changed?", used.len());
        let described: std::collections::BTreeSet<String> = keys().into_iter().map(String::from).collect();
        let missing: Vec<&String> = used.difference(&described).collect();
        assert!(missing.is_empty(), "read but not in the catalog: {:?}", missing);
        let unread: Vec<&String> = described.difference(&used).collect();
        assert!(unread.is_empty(), "in the catalog but never read: {:?}", unread);
    }
}
