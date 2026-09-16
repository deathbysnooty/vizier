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
                setting("VIZIER_CAP_CHAT", "Chat points a day", "Most chat points one person can earn in a day (one per message tier reached).", number(0, 3, "points"), "3"),
                setting("VIZIER_CAP_VOICE", "Voice points a day", "Most voice points one person can earn in a day (one per full hour, or whatever the voice block is set to).", number(0, 24, "points"), "4"),
                setting("VIZIER_CAP_QUIZ", "Quiz points a day", "Most points one person can earn from quiz rounds in a day. 100 means no limit.", number(0, 100, "points"), "6"),
                setting("VIZIER_CAP_KOTO", "Koto points a day", "Most points one person can earn from Koto in a day. 100 means no limit.", number(0, 100, "points"), "4"),
                setting("VIZIER_CAP_ANAGRAM", "Anagram points a day", "Most points one person can earn from Anagram Bot in a day. 100 means no limit.", number(0, 100, "points"), "6"),
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
                setting("VIZIER_CHAT_DAY_MESSAGES", "1st chat point at", "Messages in one India day that earn the first chat point.", number(1, 5000, "messages"), "20"),
                setting("VIZIER_CHAT_TIER2_MESSAGES", "2nd chat point at", "Messages in the day for the second chat point. Set it at or below the first to switch this tier off.", number(1, 5000, "messages"), "60"),
                setting("VIZIER_CHAT_TIER3_MESSAGES", "3rd chat point at", "Messages in the day for the third chat point. Set it at or below the second to switch this tier off.", number(1, 5000, "messages"), "150"),
                setting("VIZIER_VOICE_DAY_MINUTES", "Minutes per voice point", "Each full block of this many minutes in voice in a day earns a voice point, up to the daily limit.", number(10, 1440, "minutes"), "60"),
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
            id: "msglog",
            title: "Deleted messages",
            icon: "🗑️",
            about: "A log of deleted and edited messages, shown only on the panel's Deleted messages page (nothing is \
                    posted in Discord). The bot keeps a short-lived copy of every member message it sees in the server - \
                    text channels, threads and voice channel chats, temporary rooms included - with its pictures, so \
                    that when a message is deleted its text and pictures can still be shown, and when one is edited the \
                    text before and after. Never #safe-corner or its threads, never DMs, never bots. Discord doesn't \
                    tell bots who deleted a message, so the log can't say. Every look at the log is in the activity log.",
            settings: vec![
                toggle(
                    "VIZIER_MSGLOG",
                    "Log deleted and edited messages",
                    "Keep copies of new messages and log deletions and edits. Off stops copying new messages and logging; \
                     what's already logged stays until it's cleared.",
                ),
                setting(
                    "VIZIER_MSGLOG_KEEP_DAYS",
                    "Keep copies for",
                    "How long the copy of a message nobody deleted (its text and pictures) is kept before it's cleared. \
                     A message deleted after this shows in the log without its text.",
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
