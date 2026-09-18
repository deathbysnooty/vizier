//! The words for the House Cup posts in the scoreboard channel - the welcome,
//! the Snitch & Chocolate Frog cards post, the beginner's guide - the "How to
//! earn" button, and the rules post in the Name Place Animal Thing channel, all
//! written from the LIVE settings. [`Rules::live`]
//! reads everything once; every text builder below is a plain function of those
//! values, so a changed setting changes the text and the tests can pin values.

use sha2::{Digest, Sha256};

use super::frog_store::{MAX_TRIES, Rarity};
use super::house::{self, HOUSES};
use super::points::{Cap, Source};

/// Discord's limit on a plain message.
pub const MESSAGE_LIMIT: usize = 2000;
/// Discord's limit on one embed description.
pub const DESCRIPTION_LIMIT: usize = 4096;
/// Discord's limit on all the text in one message's embeds.
pub const EMBEDS_LIMIT: usize = 6000;

/// Every live value the posts mention. A limit of `None` means no limit.
#[derive(Clone, Debug)]
pub struct Rules {
    pub scoreboard_on: bool,
    pub house_channel: Option<u64>,
    pub quiz_channel: Option<u64>,
    pub fight_channel: Option<u64>,
    pub draw_minimum: i64,

    pub activity_on: bool,
    pub chat_tiers: Vec<i64>,
    pub chat_cap: Option<i64>,
    pub voice_minutes: i64,
    pub voice_cap: Option<i64>,
    pub voice_company: bool,
    /// Deafened time is left out of voice.
    pub voice_ignore_deaf: bool,

    pub quiz: [i64; 3],
    pub quiz_cap: Option<i64>,
    pub games_on: bool,
    pub koto_win: i64,
    pub koto_played: i64,
    pub koto_cap: Option<i64>,
    pub anagram: i64,
    pub anagram_cap: Option<i64>,
    /// Common, rare, top.
    pub cat: [i64; 3],
    pub cat_cap: Option<i64>,
    pub wordle_on: bool,
    /// In 1-2, 3, 4, 5-6, and the crown bonus.
    pub wordle: [i64; 5],
    pub arena_win: i64,
    pub arena_cap: Option<i64>,
    /// Champion, runner-up.
    pub royale: [i64; 2],
    pub battle_daily: bool,
    /// "HH:MM,HH:MM" as set.
    pub battle_times: String,

    pub snitch_on: bool,
    pub snitch_drops: (i64, i64),
    pub snitch_hours: (i64, i64),
    /// Bronze, silver, golden; each 1st, 2nd, 3rd.
    pub snitch_points: [[i64; 3]; 3],
    pub snitch_cap: Option<i64>,
    pub snitch_second_chance: bool,

    pub frogs_on: bool,
    pub frog_drops_max: i64,
    pub frog_open_minutes: i64,
    /// Common, uncommon, legendary.
    pub frog_points: [i64; 3],
    pub frog_cap: Option<i64>,
    /// Cards in play, when the card store could be read.
    pub cards: Option<usize>,
    pub set_bonus: i64,
    pub trades_on: bool,
    pub daily_top_cards: bool,
    pub royale_cards: bool,

    pub npat: NpatRules,
    pub sudoku: SudokuRules,
    pub chess: ChessRules,
    pub anagrams: AnagramRules,
    pub guess: GuessRules,
}

/// Guess the Word's live settings, for `/guesshelp` and the lines about it in
/// the House Cup posts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuessRules {
    /// The game's channel, only while the game is on and there is a bank.
    pub channel: Option<u64>,
    /// What naming a doodle pays.
    pub points: i64,
    pub cap: Option<i64>,
    /// How long a round nobody names stays up.
    pub idle_minutes: i64,
    /// How long a word - and each drawing of it - is held back.
    pub no_repeat_days: i64,
    /// Words in the bank, when there is one.
    pub words: Option<usize>,
}

/// The Anagrams game's live settings, for `/anagramhelp` and the lines about it
/// in the House Cup posts. Not to be confused with `anagram`/`anagram_cap`
/// above, which are the points paid for the Anagram Bot's own game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnagramRules {
    /// The game's channel, only while the game is on and there is a bank.
    pub channel: Option<u64>,
    /// What 4-5, 6-7 and 8+ letter words pay.
    pub points: [i64; 3],
    pub cap: Option<i64>,
    /// How long a round nobody answers stays up.
    pub idle_minutes: i64,
    /// How long the same letters are held back.
    pub no_repeat_days: i64,
    /// Words in the bank, when there is one.
    pub words: Option<usize>,
}

/// Sudoku's live settings, for its rules post, `/sudokuhelp` and the lines
/// about it in the House Cup posts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SudokuRules {
    /// The game's channel, only while the game is on.
    pub channel: Option<u64>,
    /// What easy, medium and hard are worth in SUDOKU points. Sudoku pays no
    /// house points at all, so there is no daily limit to describe.
    pub points: [i64; 3],
    /// What one hint takes off that puzzle, and how many a player may have.
    pub hint_cost: i64,
    pub max_hints: i64,
    /// Codes a player may send for one puzzle.
    pub max_tries: i64,
    /// How long an old puzzle's code is still checked.
    pub late_hours: i64,
    /// How often each level comes up.
    pub mix: [u32; 3],
    /// Whether the panel has an address, so the web page works.
    pub has_page: bool,
}

/// The chess puzzle's live settings, for `/puzzlehelp` and its card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PuzzleRules {
    /// The chess channel the puzzle lives in, only while it is on.
    pub channel: Option<u64>,
    /// House points for the FIRST person to crack one.
    pub first: i64,
    /// The daily limit on those. Nought - the default - means no house points
    /// are paid at all, and the help card says so rather than promising any.
    pub cap: i64,
    /// What easy, medium and hard are worth in PUZZLE points.
    pub band_points: [i64; 3],
    /// How long a solved puzzle stays up so others can still try it.
    pub next_minutes: i64,
    /// How long an untouched one stays up before its answer is shown.
    pub idle_minutes: i64,
    pub no_repeat_days: i64,
    /// How many puzzles the bank holds, when there is one.
    pub puzzles: Option<usize>,
    /// The line the pack asks to be credited with.
    pub attribution: String,
}

/// Name Place Animal Thing's live settings, for its rules post and the lines
/// about it in the House Cup posts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpatRules {
    /// The game's channel, only while the game is on.
    pub channel: Option<u64>,
    pub min_players: i64,
    pub min_houses: i64,
    /// How long the lobby stays open after the first I'm in.
    pub join_secs: i64,
    /// The countdown once enough players are in.
    pub start_secs: i64,
    /// The letters a round can use.
    pub letters: String,
    pub letters_per_game: i64,
    pub round_secs: i64,
    pub break_secs: i64,
    /// Game score for a unique and a shared answer.
    pub scores: [i64; 2],
    /// House points for a game's 1st and 2nd.
    pub prizes: [i64; 2],
    pub cap: Option<i64>,
    /// People who must play a game for it to pay.
    pub min_scored: i64,
}

/// Chess's live settings, for `/chesshelp`, its idle card and the House Cup posts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChessRules {
    /// The chess channel, only while chess is on.
    pub channel: Option<u64>,
    /// Hours per move in a casual game.
    pub casual_hours: i64,
    /// Seconds per move in a live game.
    pub live_seconds: i64,
    /// How many games one member may have running.
    pub max_games: i64,
    /// How many games the whole channel may have running.
    pub max_active: i64,
    /// Plies under which a resignation pays nothing.
    pub min_plies: i64,
    /// How long a finished game's replay is kept; 0 for no replays.
    pub replay_days: i64,
    /// Chess points for winning, and for each side of a draw. Chess moves no
    /// house points at all, so there is no cap to name.
    pub win: i64,
    pub draw: i64,
}

fn limit(cap: Cap) -> Option<i64> {
    match cap {
        Cap::PerDay(n) | Cap::PerWeekPerChannel(n) => Some(n),
        Cap::None => None,
    }
}

impl Rules {
    /// The settings as they are right now. Takes the card store's lock for a
    /// moment to count the cards in play, so never call it while holding that.
    pub fn live() -> Rules {
        use super::control;
        let pair = |a: u64, b: u64| (a.min(b) as i64, a.max(b) as i64);
        let frogs_on = control::on("VIZIER_FROGS", false);
        let cards = super::frog_store::db().map(|db| super::frog_store::wizards(&db.lock()).into_iter().filter(|w| w.enabled).count());
        Rules {
            scoreboard_on: control::on("VIZIER_SCOREBOARD", true) && control::id("VIZIER_SCOREBOARD_CHANNEL").is_some(),
            house_channel: control::id("VIZIER_HOUSE_CHANNEL"),
            quiz_channel: control::id("VIZIER_QUIZ_CHANNEL"),
            fight_channel: control::id("VIZIER_FIGHT_CHANNEL"),
            draw_minimum: super::points::draw_minimum(),

            activity_on: control::on("VIZIER_ACTIVITY_POINTS", true),
            chat_tiers: super::activity::chat_tier_bars(),
            chat_cap: limit(Source::Chat.cap()),
            voice_minutes: (super::activity::voice_bar_secs() / 60).max(1),
            voice_cap: limit(Source::Voice.cap()),
            voice_company: super::activity::voice_company_rule(),
            voice_ignore_deaf: super::activity::voice_deafened_rule(),

            quiz: [
                control::number("VIZIER_POINTS_QUIZ_1ST", 2) as i64,
                control::number("VIZIER_POINTS_QUIZ_2ND", 1) as i64,
                control::number("VIZIER_POINTS_QUIZ_3RD", 1) as i64,
            ],
            quiz_cap: limit(Source::Quiz.cap()),
            games_on: control::on("VIZIER_GAME_POINTS", true),
            koto_win: control::number("VIZIER_POINTS_KOTO_WIN", 3) as i64,
            koto_played: control::number("VIZIER_POINTS_KOTO_PLAYED", 1) as i64,
            koto_cap: limit(Source::Koto.cap()),
            anagram: control::number("VIZIER_POINTS_ANAGRAM", 3) as i64,
            anagram_cap: limit(Source::Anagram.cap()),
            cat: [
                control::number("VIZIER_POINTS_CAT_COMMON", 1) as i64,
                control::number("VIZIER_POINTS_CAT_RARE", 2) as i64,
                control::number("VIZIER_POINTS_CAT_TOP", 3) as i64,
            ],
            cat_cap: limit(Source::Cat.cap()),
            wordle_on: control::on("VIZIER_WORDLE_POINTS", true),
            wordle: [
                control::number("VIZIER_POINTS_WORDLE_1_2", 4) as i64,
                control::number("VIZIER_POINTS_WORDLE_3", 3) as i64,
                control::number("VIZIER_POINTS_WORDLE_4", 2) as i64,
                control::number("VIZIER_POINTS_WORDLE_5_6", 1) as i64,
                control::number("VIZIER_POINTS_WORDLE_CROWN", 1) as i64,
            ],
            arena_win: control::number("VIZIER_POINTS_ARENA_WIN", 1) as i64,
            arena_cap: limit(Source::Arena.cap()),
            royale: [
                control::number("VIZIER_POINTS_ROYALE_CHAMPION", 8) as i64,
                control::number("VIZIER_POINTS_ROYALE_RUNNER_UP", 3) as i64,
            ],
            battle_daily: control::on("VIZIER_BATTLE_DAILY", false),
            battle_times: control::var("VIZIER_BATTLE_DAILY_TIME").unwrap_or_else(|| "21:00".into()),

            snitch_on: control::on("VIZIER_SNITCH", true),
            snitch_drops: pair(control::number("VIZIER_SNITCH_DROPS_MIN", 3), control::number("VIZIER_SNITCH_DROPS_MAX", 4)),
            snitch_hours: (
                control::number("VIZIER_SNITCH_START_HOUR", 10).min(23) as i64,
                control::number("VIZIER_SNITCH_END_HOUR", 24).clamp(1, 24) as i64,
            ),
            snitch_points: [
                [
                    control::number("VIZIER_SNITCH_BRONZE_1ST", 2) as i64,
                    control::number("VIZIER_SNITCH_BRONZE_2ND", 1) as i64,
                    control::number("VIZIER_SNITCH_BRONZE_3RD", 1) as i64,
                ],
                [
                    control::number("VIZIER_SNITCH_SILVER_1ST", 3) as i64,
                    control::number("VIZIER_SNITCH_SILVER_2ND", 2) as i64,
                    control::number("VIZIER_SNITCH_SILVER_3RD", 1) as i64,
                ],
                [
                    control::number("VIZIER_SNITCH_GOLDEN_1ST", 6) as i64,
                    control::number("VIZIER_SNITCH_GOLDEN_2ND", 4) as i64,
                    control::number("VIZIER_SNITCH_GOLDEN_3RD", 2) as i64,
                ],
            ],
            snitch_cap: limit(Source::Snitch.cap()),
            snitch_second_chance: control::on("VIZIER_SNITCH_SECOND_CHANCE", true) && control::number("VIZIER_SNITCH_SECOND_CHANCES", 3) > 0,

            frogs_on,
            frog_drops_max: control::number("VIZIER_FROG_DROPS_MIN", 5).max(control::number("VIZIER_FROG_DROPS_MAX", 7)) as i64,
            frog_open_minutes: control::number("VIZIER_FROG_OPEN_MINUTES", 5).clamp(1, 60) as i64,
            frog_points: [Rarity::Common.points(), Rarity::Uncommon.points(), Rarity::Legendary.points()],
            frog_cap: limit(Source::Frog.cap()),
            cards,
            set_bonus: control::number("VIZIER_FROG_SET_BONUS", 35).min(1000) as i64,
            trades_on: frogs_on && control::on("VIZIER_TRADES", true),
            daily_top_cards: frogs_on && control::on("VIZIER_FROG_DAILY_TOP", true),
            royale_cards: frogs_on && control::on("VIZIER_FROG_ROYALE_CARDS", true),

            npat: super::npat::npat_rules(limit(Source::Npat.cap())),
            sudoku: super::sudoku::sudoku_rules(),
            chess: super::chess::chess_rules(),
            anagrams: super::anagram::anagram_rules(),
            guess: super::guess::guess_rules(),
        }
    }
}

// --- small words ----------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "(max 6)" or "(no limit)".
fn max_words(cap: Option<i64>) -> String {
    match cap {
        Some(n) => format!("(max {})", n),
        None => "(no limit)".to_string(),
    }
}

/// "20", "20 and 60", "20, 60 and 150".
fn and_list(items: &[String], and: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} {} {}", rest.join(", "), and, last),
    }
}

/// An India clock time in words: "9 pm", "3:30 pm", "noon", "midnight".
fn clock_words(minutes: i64) -> String {
    let (h, m) = (minutes.rem_euclid(1440) / 60, minutes.rem_euclid(1440) % 60);
    if m == 0 && h == 0 {
        return "midnight".into();
    }
    if m == 0 && h == 12 {
        return "noon".into();
    }
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    let half = if h < 12 { "am" } else { "pm" };
    if m == 0 { format!("{} {}", h12, half) } else { format!("{}:{:02} {}", h12, m, half) }
}

/// "15:00, 20:00" -> "3 pm & 8 pm", earliest first; anything unreadable is skipped.
pub fn times_words(raw: &str) -> String {
    let mut minutes: Vec<i64> = raw
        .split(',')
        .filter_map(|t| {
            let (h, m) = t.trim().split_once(':')?;
            let (h, m): (i64, i64) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
            ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
        })
        .collect();
    minutes.sort_unstable();
    minutes.dedup();
    let words: Vec<String> = minutes.into_iter().map(clock_words).collect();
    and_list(&words, "&")
}

fn channel(id: Option<u64>) -> Option<String> {
    id.filter(|id| *id != 0).map(|id| format!("<#{}>", id))
}

/// "**6–8 times a day**" or "**6 times a day**".
fn range_words(low: i64, high: i64) -> String {
    if low == high { format!("{} a day", plural(high, "time", "times")) } else { format!("{}–{} times a day", low, high) }
}

fn places(p: &[i64; 3]) -> String {
    format!("{} · {} · {}", p[0], p[1], p[2])
}

/// A stable fingerprint of a post's content, stored to tell when it changed.
pub fn digest(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize().iter().map(|b| format!("{:02x}", b)).collect()
}

// --- 1. the welcome --------------------------------------------------------------------

/// The plain-text welcome that sits first in the channel.
pub fn welcome_text(r: &Rules) -> String {
    let houses: Vec<String> = HOUSES.iter().map(|h| format!("{} **{}**", h.crest, h.name)).collect();
    let rooms: Vec<String> = house::common_room_names()
        .into_iter()
        .map(|(key, name)| format!("{} `{}`", house::house(key).map(|h| h.crest).unwrap_or("🏠"), name))
        .collect();
    let mut action = Vec::new();
    if let Some(c) = channel(r.house_channel) {
        action.push(format!("🏆 {} · points updates, sorting and house news", c));
    }
    if let Some(c) = channel(r.quiz_channel) {
        action.push(format!("🧠 {} · the quiz (vote a genre, answer first)", c));
    }
    if let Some(c) = channel(r.npat.channel) {
        action.push(format!(
            "🔤 {} · Name Place Animal Thing (games need {} from {})",
            c,
            plural(r.npat.min_players, "player", "players"),
            plural(r.npat.min_houses, "house", "houses")
        ));
    }
    if let Some(c) = channel(r.sudoku.channel) {
        action.push(format!("🔢 {} · a sudoku always waiting, first to solve it wins", c));
    }
    if let Some(c) = channel(r.chess.channel) {
        action.push(format!("♟️ {} · chess — press ⚔️ Challenge someone on the card; chess points, not house points", c));
    }
    if let Some(c) = channel(r.anagrams.channel) {
        action.push(format!("🔀 {} · anagrams — unscramble the letters and type the word", c));
    }
    if let Some(c) = channel(r.guess.channel) {
        action.push(format!("🎨 {} · guess the word — say what the doodle is", c));
    }
    if let Some(c) = channel(r.fight_channel) {
        let what = if r.battle_daily { "fights and the daily Battle Royale" } else { "fights and Battle Royales" };
        action.push(format!("⚔️ {} · {}", c, what));
    }
    match (r.snitch_on, r.frogs_on) {
        (true, true) => action.push("💬 Busy chats · where the 🪽 Golden Snitch and 🐸 Chocolate Frogs appear".into()),
        (true, false) => action.push("💬 Busy chats · where the 🪽 Golden Snitch appears".into()),
        (false, true) => action.push("💬 Busy chats · where the 🐸 Chocolate Frogs appear".into()),
        (false, false) => {}
    }

    let mut text = String::from("## 🏰 Welcome to the House Cup!\n\n");
    text.push_str(
        "Whether you're here to chat, climb the quiz board or throw hands in the arena, **everything you do on MLCI now \
         counts for your house**. This channel is your home base: the rules are right below, and the live scoreboard \
         always sits at the very bottom.\n\n",
    );
    text.push_str("**🎩 Your house**\n");
    text.push_str(&format!(
        "Every member is sorted into one of four houses: {}. Your house role shows next to your name. New here? The \
         Sorting Hat sorts you the moment you join.\n\n",
        and_list(&houses, "or")
    ));
    text.push_str("**🚪 Your common room**\n");
    text.push_str(&format!(
        "Each house has its **own private room** under the **{}** category in the channel list. **Only your house can \
         see yours**, so if you can see one, that's home:\n> {}\n\n",
        house::common_room_category_name(),
        rooms.join(" · ")
    ));
    text.push_str(
        "Use it to plan strategy, rally your housemates for the quiz, trade cards and hype up your team. Whatever's said \
         in the common room stays in the common room. 🤫\n\n",
    );
    if !action.is_empty() {
        text.push_str(&format!("**📍 Where the action is**\n{}\n\n", action.join("\n")));
    }
    text.push_str("**🤝 Play fair, play nice**\n");
    text.push_str(
        "Banter is welcome, bullying isn't. Points can't be bought, farmed or begged for, and mods can take away points \
         won unfairly. Want to sit it out? `/houseopt` makes you a Muggle, no hard feelings.\n\n",
    );
    text.push_str("Now scroll down, learn the ropes, and go win it for your house. ⬇️");
    text
}

// --- 2. the Snitch & Chocolate Frog cards post -----------------------------------------

/// Which how-to pictures go under the cards post, in order.
pub fn snitch_cards_images(r: &Rules) -> Vec<&'static str> {
    let mut out = Vec::new();
    if r.snitch_on {
        out.push("snitch");
    }
    if r.frogs_on {
        out.push("frog");
    }
    out
}

/// The Snitch & Chocolate Frog cards post, or `None` when both are switched off.
pub fn snitch_cards_text(r: &Rules) -> Option<String> {
    let (title, intro) = match (r.snitch_on, r.frogs_on) {
        (true, true) => ("✨ The Golden Snitch & Chocolate Frog Cards", "Two magical things fly around MLCI every day."),
        (true, false) => ("✨ The Golden Snitch", "The Golden Snitch flies around MLCI every day."),
        (false, true) => ("✨ Chocolate Frog Cards", "Chocolate Frogs hop around MLCI every day."),
        (false, false) => return None,
    };
    let mut text = format!("## {}\n\n{} Watch the busy chats: the fastest hands win.\n", title, intro);

    if r.snitch_on {
        let (start, end) = r.snitch_hours;
        let when = if start == 0 && end >= 24 {
            "day or night".to_string()
        } else {
            format!("between {} and {} India time", clock_words(start * 60), clock_words(end * 60))
        };
        let [bronze, silver, golden] = &r.snitch_points;
        text.push_str("\n**🪽 The Golden Snitch**\n");
        text.push_str(&format!(
            "A Snitch zooms into an active chat **{}**, {}.\n",
            range_words(r.snitch_drops.0, r.snitch_drops.1),
            when
        ));
        text.push_str("• **Reply** to the Snitch's message with just `ACCIO`\n");
        text.push_str("• The **first three** to catch it score for their house:\n");
        text.push_str(&format!(
            "> 🥉 Bronze **{}**  ·  🥈 Silver **{}**  ·  🥇 Gold **{}**\n",
            places(bronze),
            places(silver),
            places(golden)
        ));
        text.push_str(&match r.snitch_cap {
            None => "• No daily limit\n".to_string(),
            Some(n) => format!("• Up to {} a day (Gold has no limit)\n", plural(n, "point", "points")),
        });
        if r.snitch_second_chance {
            text.push_str("• Missed one? An uncaught Snitch often comes back for a second chance 👀\n");
        }
    }

    if r.frogs_on {
        let [common, uncommon, legendary] = r.frog_points;
        text.push_str("\n**🐸 Chocolate Frogs**\n");
        text.push_str(&format!(
            "A frog hops into a busy chat **up to {} a day**, carrying a collectible wizard card.\n",
            plural(r.frog_drops_max, "time", "times")
        ));
        text.push_str("• Press **🐸 Catch it** and a riddle pops up that **only you** can see\n");
        text.push_str(&format!("• You get **{}**, and small spelling slips are forgiven\n", plural(MAX_TRIES, "try", "tries")));
        text.push_str("• The **first correct answer** keeps the card and scores for the house:\n");
        text.push_str(&format!(
            "> 🥛 Common **{}**  ·  🍫 Uncommon **{}**  ·  🔥 The Eternal Phoenix **{}**\n",
            common, uncommon, legendary
        ));
        if let Some(n) = r.frog_cap {
            text.push_str(&format!("• Up to {} from frogs a day\n", plural(n, "point", "points")));
        }
        text.push_str(&format!(
            "• Nobody gets it in {}? It hops away and the answer is revealed\n",
            plural(r.frog_open_minutes, "minute", "minutes")
        ));

        text.push_str("\n**🃏 The cards**\n");
        let count = match r.cards {
            Some(n) => format!("There are **{}** to collect", plural(n as i64, "card", "cards")),
            None => "There are cards to collect".to_string(),
        };
        text.push_str(&format!("{}, and every copy has its own number forever, like *Luna Lovegood #3 · No. 0042*.\n", count));
        if r.daily_top_cards {
            text.push_str("• 🥇 **Top of the day:** yesterday's top player in each activity wins a bonus card every morning\n");
        }
        if r.royale_cards {
            text.push_str("• 👑 **Battle Royale:** the champion *and* the runner-up each win a bonus card\n");
        }
        if r.daily_top_cards || r.royale_cards {
            text.push_str("• *(Bonus cards give no house points. Points only come from catching frogs.)*\n");
        }
        if r.trades_on {
            text.push_str("• 🔁 `/trade` swap, gift or ask for cards with anyone\n");
        }
        text.push_str("• 🏠 `/housecards` what your house holds, by card or by member\n");
        let selling = r.set_bonus > 0;
        if selling {
            let all = r.cards.map(|n| format!("**all {}**", n)).unwrap_or_else(|| "**every card**".to_string());
            text.push_str(&format!(
                "• 🏆 `/sellset` hand in one of {} for **+{}** house points (you keep your catch points)\n",
                all, r.set_bonus
            ));
            if r.trades_on {
                text.push_str("• 🧠 **Tip:** pool cards to one housemate to finish a set faster\n");
            }
        }
        text.push_str(&format!(
            "\n**{}** Low numbers and rare cards may matter later. Something is coming. 👀\n",
            if selling { "…or keep them." } else { "Keep them." }
        ));
        let mut commands = vec![
            "📖 `/frogs` your collection",
            "🔎 `/frogcard` look up any card by number",
            "🏠 `/housecards` your house's cards",
        ];
        if r.trades_on {
            commands.push("🔁 `/trades` your offers");
        }
        text.push_str(&format!("\n{}", commands.join(" · ")));
    }
    Some(text.trim_end().to_string())
}

// --- 3. the beginner's guide -----------------------------------------------------------

/// One embed of the guide.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub title: String,
    pub body: String,
    pub colour: u32,
    pub footer: Option<String>,
}

pub const GUIDE_FOOTER: &str = "Updated automatically whenever the rules change";

fn chat_line(r: &Rules) -> Option<String> {
    let cap = r.chat_cap.map(|c| c.min(r.chat_tiers.len() as i64));
    if r.chat_tiers.is_empty() || cap == Some(0) {
        return None;
    }
    let tiers: Vec<String> = r.chat_tiers.iter().map(|t| t.to_string()).collect();
    let each = if r.chat_tiers.len() == 1 { "**1 point**" } else { "**1 point each**" };
    let word = if r.chat_tiers.len() == 1 && r.chat_tiers[0] == 1 { "message" } else { "messages" };
    Some(format!("💬 **Chat** — {} {} in a day → {} {}", and_list(&tiers, "and"), word, each, max_words(cap)))
}

/// "hour", "30 minutes", "2 hours".
fn voice_block(minutes: i64) -> String {
    match minutes {
        60 => "hour".into(),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{} minutes", m),
    }
}

fn voice_line(r: &Rules) -> Option<String> {
    if r.voice_cap == Some(0) {
        return None;
    }
    let company = if r.voice_company { " **with at least one other person**" } else { "" };
    Some(format!("🎙️ **Voice** — every full {} in VC{} → **1 point** {}", voice_block(r.voice_minutes), company, max_words(r.voice_cap)))
}

/// The small print under the voice line: what doesn't count.
fn voice_note(r: &Rules) -> Option<&'static str> {
    match (r.voice_company, r.voice_ignore_deaf) {
        (true, true) => Some("Alone in VC or only with a music bot doesn't count, and neither does deafened time."),
        (true, false) => Some("Alone in VC or only with a music bot doesn't count."),
        (false, true) => Some("Deafened time doesn't count."),
        (false, false) => None,
    }
}

fn cat_range(r: &Rules) -> String {
    let (lo, hi) = (r.cat.iter().min().copied().unwrap_or(0), r.cat.iter().max().copied().unwrap_or(0));
    if lo == hi { format!("**{}**", hi) } else { format!("**{}–{}**", lo, hi) }
}

fn game_lines(r: &Rules) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(quiz) = channel(r.quiz_channel).filter(|_| r.quiz.iter().any(|p| *p > 0)) {
        lines.push(format!("🧠 **Quiz** in {} — vote a genre, answer first; top 3 of each round get **{}** {}", quiz, places(&r.quiz), max_words(r.quiz_cap)));
    }
    if r.games_on {
        let mut koto = Vec::new();
        if r.koto_win > 0 {
            let played = if r.koto_played > 0 { format!(", **{}** for guesses that score", r.koto_played) } else { String::new() };
            koto.push(format!("🔤 **Koto** — solve **{}**{} {}", r.koto_win, played, max_words(r.koto_cap)));
        }
        if r.anagram > 0 {
            koto.push(format!("🔡 **Anagram** — solve **{}** {}", r.anagram, max_words(r.anagram_cap)));
        }
        if !koto.is_empty() {
            lines.push(koto.join(" · "));
        }
        if r.cat.iter().any(|p| *p > 0) {
            lines.push(format!("🐱 **Cat Bot** — catch a cat {} by rarity {}", cat_range(r), max_words(r.cat_cap)));
        }
    }
    if r.wordle_on && r.wordle[..4].iter().any(|p| *p > 0) {
        let crown = if r.wordle[4] > 0 { format!(", 👑 best of the day **+{}**", r.wordle[4]) } else { String::new() };
        lines.push(format!(
            "🟩 **Wordle** — solve in 1–2 **{}** · 3 **{}** · 4 **{}** · 5–6 **{}**{}",
            r.wordle[0], r.wordle[1], r.wordle[2], r.wordle[3], crown
        ));
    }
    if let Some(c) = channel(r.npat.channel).filter(|_| r.npat.prizes.iter().any(|p| *p > 0)) {
        lines.push(format!(
            "🔤 **Name Place Animal Thing** in {} — press I'm in; a game is {}, and its best two house members win 🥇 **+{}** · 🥈 **+{}** {}",
            c,
            plural(r.npat.letters_per_game, "letter", "letters"),
            r.npat.prizes[0],
            r.npat.prizes[1],
            max_words(r.npat.cap)
        ));
    }
    if let Some(c) = channel(r.sudoku.channel).filter(|_| r.sudoku.points.iter().any(|p| *p > 0)) {
        lines.push(format!(
            "🔢 **Sudoku** in {} — one is always waiting; the first correct code wins 🟢 **{}** · 🟡 **{}** · 🔴 **{}** **sudoku points**, the game's own score (no house points, no daily limit — `/sudokutop` is the board)",
            c, r.sudoku.points[0], r.sudoku.points[1], r.sudoku.points[2]
        ));
    }
    // Chess is on this list as a game, but it is no longer a way to earn house
    // points: it scores chess points of its own, which nothing limits.
    if let Some(c) = channel(r.chess.channel).filter(|_| r.chess.win > 0 || r.chess.draw > 0) {
        lines.push(format!(
            "♟️ **Chess** in {} — press ⚔️ Challenge someone (or `/chess @member`); **chess points**, not house points: win **+{}**, draw **+{}** each, no daily limit, `/chesstop`",
            c, r.chess.win, r.chess.draw
        ));
    }
    if let Some(c) = channel(r.anagrams.channel).filter(|_| r.anagrams.points.iter().any(|p| *p > 0)) {
        lines.push(format!(
            "🔀 **Anagrams** in {} — unscramble the letters and just type the word; any word using all of them counts, **{}** · **{}** · **{}** by length {}",
            c, r.anagrams.points[0], r.anagrams.points[1], r.anagrams.points[2], max_words(r.anagrams.cap)
        ));
    }
    if let Some(c) = channel(r.guess.channel).filter(|_| r.guess.points > 0) {
        lines.push(format!(
            "🎨 **Guess the Word** in {} — a doodle is always up; first to type what it is wins **{}** {}",
            c,
            r.guess.points,
            max_words(r.guess.cap)
        ));
    }
    if r.arena_win > 0 {
        lines.push(format!("⚔️ **1v1 fights** — `/fight` someone, win **{}** {}", r.arena_win, max_words(r.arena_cap)));
    }
    if r.royale.iter().any(|p| *p > 0) {
        let place = channel(r.fight_channel).map(|c| format!(" in {}", c)).unwrap_or_default();
        let times = times_words(&r.battle_times);
        let when = if r.battle_daily && !times.is_empty() {
            format!("daily at **{}**{}: press Join", times, place)
        } else {
            format!("when one opens{}, press Join", place)
        };
        lines.push(format!("👑 **Battle Royale** — {}; champion **+{}**, runner-up **+{}** (no limit)", when, r.royale[0], r.royale[1]));
    }
    lines
}

/// The guide's embeds, in order. `cards_post_above` says whether the Snitch &
/// cards post sits above it, so the intro can point there.
pub fn guide(r: &Rules, cards_post_above: bool) -> Vec<Panel> {
    let houses: Vec<String> = HOUSES.iter().map(|h| format!("{} **{}**", h.crest, h.name)).collect();
    let draw = if r.draw_minimum > 0 { format!(" (**{}+** points that month)", r.draw_minimum) } else { String::new() };
    let mut intro = format!(
        "Every member belongs to one of four houses: {}. Almost everything you do here earns points for your house.\n\n\
         🏆 The house with the most points at the end of the month wins the **House Cup**. Its captain and one lucky \
         active member{} win **Discord Nitro**.",
        and_list(&houses, "and"),
        draw
    );
    if cards_post_above {
        let what = match (r.snitch_on, r.frogs_on) {
            (true, true) => "Snitch and card rules are",
            (true, false) => "Snitch rules are",
            _ => "Card rules are",
        };
        intro.push_str(&format!("\n\n✨ {} in the post above ⬆️", what));
    }
    intro.push_str("\n-# Not sorted yet? The Sorting Hat sorts newcomers when they join. Don't want to play? `/houseopt` makes you a Muggle.");

    let mut panels = vec![Panel { title: "📖 How to play · House Cup for beginners".into(), body: intro, colour: 0xE8B923, footer: None }];
    let mut sections: Vec<(&str, &str, String, u32)> = Vec::new();

    if r.activity_on {
        let lines: Vec<String> = [chat_line(r), voice_line(r)].into_iter().flatten().collect();
        if !lines.is_empty() {
            let mut body = lines.join("\n");
            if let Some(note) = voice_note(r).filter(|_| voice_line(r).is_some()) {
                body.push_str(&format!("\n-# {}", note));
            }
            sections.push(("💬", "Just hang out", body, 0x3BA55C));
        }
    }
    let games = game_lines(r);
    if !games.is_empty() {
        sections.push(("🎮", "Play the games", format!("{}\n-# Limits reset at midnight India time.", games.join("\n")), 0x5865F2));
    }
    let frogs = if r.frogs_on { " · `/frogs` your cards" } else { "" };
    let buttons = if r.scoreboard_on { " · or press the buttons on the scoreboard below ⬇️" } else { "" };
    sections.push((
        "📊",
        "Check your progress",
        format!(
            "`/today` your points and limits today · `/mypoints` this month\n`/housetop` your house's top scorers{}\n`/help` every command{}",
            frogs, buttons
        ),
        0x99AAB5,
    ));
    for (i, (icon, name, body, colour)) in sections.into_iter().enumerate() {
        panels.push(Panel { title: format!("{} {} · {}", icon, i + 1, name), body, colour, footer: None });
    }
    for p in &mut panels {
        if p.body.chars().count() > DESCRIPTION_LIMIT {
            p.body = p.body.chars().take(DESCRIPTION_LIMIT - 1).collect::<String>() + "…";
        }
    }
    if let Some(last) = panels.last_mut() {
        last.footer = Some(GUIDE_FOOTER.into());
    }
    panels
}

/// Everything Discord counts towards a message's 6000-character embed limit.
pub fn embed_chars(panels: &[Panel]) -> usize {
    panels.iter().map(|p| p.title.chars().count() + p.body.chars().count() + p.footer.as_ref().map_or(0, |f| f.chars().count())).sum()
}

// --- the "How to earn" button ----------------------------------------------------------

/// The private reply to "How to earn": the guide in a few lines.
pub fn earn_text(r: &Rules) -> String {
    let mut lines = vec!["❓ **How to earn points**".to_string()];
    if r.activity_on {
        if !r.chat_tiers.is_empty() && r.chat_cap != Some(0) {
            let tiers: Vec<String> = r.chat_tiers.iter().map(|t| t.to_string()).collect();
            let cap = r.chat_cap.map(|c| c.min(r.chat_tiers.len() as i64));
            lines.push(format!("💬 Chat {} msgs → 1 each {}", tiers.join("·"), max_words(cap)));
        }
        if r.voice_cap != Some(0) {
            let company = if r.voice_company { " with others" } else { "" };
            let per = if r.voice_minutes == 60 { "hour".to_string() } else { format!("{} min", r.voice_minutes) };
            let deaf = if r.voice_ignore_deaf { " · deafened time doesn't count" } else { "" };
            lines.push(format!("🎙️ VC{} → 1/{} {}{}", company, per, max_words(r.voice_cap), deaf));
        }
    }
    let mut games = Vec::new();
    if r.quiz_channel.is_some() && r.quiz.iter().any(|p| *p > 0) {
        games.push(format!("🧠 Quiz podium {}", r.quiz.iter().map(|p| p.to_string()).collect::<Vec<_>>().join("·")));
    }
    if r.games_on {
        if r.koto_win > 0 {
            games.push(format!("🔤 Koto {}", r.koto_win));
        }
        if r.anagram > 0 {
            games.push(format!("🔡 Anagram {}", r.anagram));
        }
        if r.cat.iter().any(|p| *p > 0) {
            games.push(format!("🐱 Cats {}", cat_range(r).replace("**", "")));
        }
    }
    if !games.is_empty() {
        lines.push(games.join(" · "));
    }
    let mut more = Vec::new();
    if r.wordle_on && r.wordle[..4].iter().any(|p| *p > 0) {
        more.push(format!("🟩 Wordle up to {}", r.wordle[..4].iter().max().copied().unwrap_or(0) + r.wordle[4]));
    }
    if r.arena_win > 0 {
        more.push(format!("⚔️ 1v1 win {}", r.arena_win));
    }
    if r.npat.channel.is_some() && r.npat.prizes.iter().any(|p| *p > 0) {
        more.push(format!("🔤 NPAT game 1st +{} · 2nd +{} {}", r.npat.prizes[0], r.npat.prizes[1], max_words(r.npat.cap)));
    }
    // Neither sudoku nor chess is in this list. Each pays no house points at all
    // any more — each keeps its own score — so they drop out of "how to earn"
    // the same way the chat line does once its limit is nought. Their own lines
    // are below.
    if r.anagrams.channel.is_some() && r.anagrams.points.iter().any(|p| *p > 0) {
        more.push(format!(
            "🔀 Anagrams {}–{} first to type it {}",
            r.anagrams.points.iter().min().copied().unwrap_or(0),
            r.anagrams.points.iter().max().copied().unwrap_or(0),
            max_words(r.anagrams.cap)
        ));
    }
    if r.guess.channel.is_some() && r.guess.points > 0 {
        more.push(format!("🎨 Guess the Word {} first to name it {}", r.guess.points, max_words(r.guess.cap)));
    }
    if !more.is_empty() {
        lines.push(more.join(" · "));
    }
    let mut quick = Vec::new();
    if r.snitch_on {
        let lo = r.snitch_points[0].iter().filter(|p| **p > 0).min().copied().unwrap_or(0);
        let hi = r.snitch_points[2][0].max(r.snitch_points[1][0]).max(r.snitch_points[0][0]);
        let cap = match r.snitch_cap {
            None => String::new(),
            Some(n) => format!(" (max {}, Gold no limit)", n),
        };
        quick.push(format!("🪽 Snitch {}–{}{}", lo, hi, cap));
    }
    if r.frogs_on {
        let lo = r.frog_points.iter().min().copied().unwrap_or(0);
        let hi = r.frog_points.iter().max().copied().unwrap_or(0);
        quick.push(format!("🐸 Frogs {}–{} {}", lo, hi, max_words(r.frog_cap)));
    }
    if !quick.is_empty() {
        lines.push(quick.join(" · "));
    }
    if r.royale.iter().any(|p| *p > 0) {
        let times = times_words(&r.battle_times);
        let when = if r.battle_daily && !times.is_empty() { format!(" · daily {}", times) } else { String::new() };
        lines.push(format!("👑 Royale champion +{} · runner-up +{}{}", r.royale[0], r.royale[1], when));
    }
    if r.frogs_on && r.set_bonus > 0 {
        lines.push(format!("🏆 `/sellset` a full card set +{}", r.set_bonus));
    }
    // Said once, apart from the list, so nobody hunts for the sudoku line that
    // used to be in it.
    if r.sudoku.channel.is_some() && r.sudoku.points.iter().any(|p| *p > 0) {
        lines.push(format!(
            "-# 🔢 Sudoku pays **sudoku points** ({}–{} a puzzle, no limit), not house points · `/sudokutop`",
            r.sudoku.points.iter().min().copied().unwrap_or(0),
            r.sudoku.points.iter().max().copied().unwrap_or(0)
        ));
    }
    if r.chess.channel.is_some() && (r.chess.win > 0 || r.chess.draw > 0) {
        lines.push(format!(
            "-# ♟️ Chess pays **chess points** (win +{}, draw +{} each, no limit), not house points · `/chesstop`",
            r.chess.win, r.chess.draw
        ));
    }
    lines.push("-# Limits reset at midnight India time · the full guide sits above the scoreboard".into());
    lines.join("\n")
}

// --- the Name Place Animal Thing rules post -------------------------------------------

pub const NPAT_RULES_TITLE: &str = "📜 How Name · Place · Animal · Thing works";

/// "45 seconds", "3 minutes", "90 seconds".
fn duration_words(secs: i64) -> String {
    if secs >= 60 && secs % 60 == 0 { plural(secs / 60, "minute", "minutes") } else { plural(secs, "second", "seconds") }
}

/// The letters of A to Z a pool leaves out, in order.
pub fn skipped_letters(pool: &str) -> Vec<char> {
    let used: Vec<char> = pool.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_uppercase()).collect();
    ('A'..='Z').filter(|c| !used.contains(c)).collect()
}

/// Which letters come up, in a few words.
fn letters_words(pool: &str) -> String {
    let skipped = skipped_letters(pool);
    let used: Vec<String> = ('A'..='Z').filter(|c| !skipped.contains(c)).map(|c| c.to_string()).collect();
    let skipped: Vec<String> = skipped.iter().map(|c| c.to_string()).collect();
    match (skipped.len(), used.len()) {
        (0, _) => "Any letter from A to Z can come up.".to_string(),
        (_, 0) => "Any letter from A to Z can come up.".to_string(),
        (n, _) if n <= 13 => format!("{} never come{} up.", and_list(&skipped, "and"), if n == 1 { "s" } else { "" }),
        _ => format!("Only {} come up.", and_list(&used, "and")),
    }
}

/// The rules post's body, under [`NPAT_RULES_TITLE`]: everything a beginner
/// needs, from the live settings.
pub fn npat_rules_text(n: &NpatRules) -> String {
    let houses = if n.min_houses > 1 { format!(" from at least **{} different houses**", n.min_houses) } else { String::new() };
    let letters = plural(n.letters_per_game, "letter", "letters");
    let mut t = String::new();
    t.push_str("**✋ Joining a game**\n");
    t.push_str(&format!(
        "• Press **✋ I'm in** on the game card at the bottom (press again to leave). That opens the lobby for **{}**.\n",
        duration_words(n.join_secs)
    ));
    t.push_str(&format!(
        "• A game starts once **{}** are in{}. You still get **{}** after that to jump in.\n",
        plural(n.min_players, "player", "players"),
        houses,
        duration_words(n.start_secs)
    ));
    t.push_str("• Anyone in a house can play. 🧙 Muggles can play too: they count as players but not as a house, and never win house points. Not in a house? Join one first.\n");
    t.push_str("• Not enough players in time? The lobby resets, and anyone can press **I'm in** to try again.\n");

    t.push_str("\n**✍️ Playing**\n");
    t.push_str(&format!("• A game is **{}**. For each one you get **{}**. {}\n", letters, duration_words(n.round_secs), letters_words(&n.letters)));
    t.push_str("• Press **✍️ Submit answers** and fill in a **Name, Place, Animal and Thing** starting with that letter.\n");
    t.push_str("• Your answers are private, and you can change them until time's up. Came in late? Answer the letters that are left.\n");
    t.push_str("• Nobody types in this channel: everything happens with buttons.\n");

    t.push_str("\n**✅ What counts**\n");
    t.push_str("• **Name:** a real first name or surname (Priya, Patel), or a famous real person. No fictional characters (Pikachu, Harry Potter), no brands.\n");
    t.push_str("• **Place:** a real place on a map: a country, state, city, town, village or named landmark (Pune, Punjab, Paris). No made-up places.\n");
    t.push_str("• **Animal:** a real living animal, breeds included (Parrot, Pug). Nothing mythical or extinct (Pegasus, Pterodactyl), no plants.\n");
    t.push_str("• **Thing:** a real object you can touch or use, food and drink included (Pen, Pizza, Piano). No brand names (Pepsi, Parle-G), no feelings or ideas.\n");
    t.push_str("• Hindi and Hinglish count (Sher = Lion). Small typos are fine. One answer per box. \"The\", \"a\" and \"an\" at the start are ignored.\n");

    t.push_str("\n**🎯 Game score**\n");
    t.push_str(&format!("✅ unique **{}** · 🟰 shared **{}** · ❌ wrong or blank **0**\n", n.scores[0], n.scores[1]));
    t.push_str(&format!(
        "Shared means someone else gave the same answer, even in another spelling or language (Bombay = Mumbai). Scores add up over the {}.\n",
        letters
    ));

    t.push_str("\n**🏠 House points** (paid when the game ends)\n");
    t.push_str(&format!("• 🥇 The game's best house member gets **+{}**, 🥈 the next **+{}**.\n", n.prizes[0], n.prizes[1]));
    t.push_str("• Level scores go to whoever locked in their final score first.\n");
    let limit = match n.cap {
        Some(c) => format!("Up to **{}** a day each.", plural(c, "house point", "house points")),
        None => "No daily limit.".to_string(),
    };
    t.push_str(&format!("• Only when at least **{}** played the game. {}\n", plural(n.min_scored, "person", "people"), limit));
    t.push_str("• Muggles keep their place, but the points pass to the next house members.\n");

    t.push_str("\n**⚖️ Challenges**\n");
    t.push_str("Think an answer was judged wrong? Press **⚖️ Challenge** on that letter's results within 30 minutes. A mod decides with 🛡️ Review, and scores and house points are corrected automatically.\n");

    t.push_str("\n**🔁 Letters keep coming**\n");
    t.push_str(&format!(
        "The next letter comes **{}** after each letter's results, even if fewer people play. If nobody answers a letter, the game ends early. After the last letter the final results go up and the lobby opens again.",
        duration_words(n.break_secs)
    ));
    t
}

// --- the Sudoku rules post and `/sudokuhelp` ------------------------------------------

pub const SUDOKU_RULES_TITLE: &str = "🔢 How Sudoku works";

/// "🟢 Easy **2** · 🟡 Medium **4** · 🔴 Hard **6**".
fn sudoku_points_words(points: &[i64; 3]) -> String {
    format!("🟢 Easy **{}** · 🟡 Medium **{}** · 🔴 Hard **{}**", points[0], points[1], points[2])
}

fn sudoku_where(id: Option<u64>) -> String {
    match channel(id) {
        Some(c) => format!("in {}", c),
        None => "in its own channel".to_string(),
    }
}

/// What `/sudokuhelp` says: the whole game in one private message, from the
/// settings as they are now. Kept under one Discord message.
pub fn sudoku_help_text(s: &SudokuRules) -> String {
    let mut t = String::new();
    t.push_str(&format!(
        "**🔢 What it is**\nA sudoku is always waiting {}. Solve it before anyone else and it's yours.\n\n",
        sudoku_where(s.channel)
    ));
    t.push_str("**▶️ How to play**\n");
    if s.has_page {
        t.push_str("• Press **▶️ Play** on the puzzle card — the bot sends you a private link, just for you.\n");
        t.push_str("• Fill the grid on that page: tap a square, tap a number. **Notes**, **Undo**, **Check** and **Reset** are there to help, and your work is saved on that device.\n");
        t.push_str("• Press **Copy code**, come back here, press **📋 Submit code** and paste it.\n\n");
    } else {
        t.push_str("• Press **▶️ Play** on the puzzle card. The web page isn't set up yet, so the bot sends you the grid as text.\n");
        t.push_str("• Solve it wherever you like, then press **📋 Submit code** and type all **81 digits**, row by row.\n\n");
    }
    t.push_str(&format!(
        "**🧩 Sudoku points**\n{} — to the **first** correct code only. The moment one is solved the next puzzle appears.\n\
         • Sudoku pays **sudoku points**, this game's own score. They are **not** house points: solving a puzzle doesn't move the House Cup, and nothing here is capped.\n\
         • Everyone has them — houses or no houses, mods included.\n\
         • `/sudokutop` is the board for today or this month, `/sudoku` shows where you stand, and the day's top scorer is the one the 🐸 frog card goes to.\n\n",
        sudoku_points_words(&s.points)
    ));
    t.push_str(&format!(
        "**💡 Hints**\n**💡 Hint** shows you one square and takes **{}** off what that puzzle is worth to you, {} per puzzle. It's a button here, so the bot knows whose hint it was.\n\n",
        plural(s.hint_cost, "sudoku point", "sudoku points"),
        plural(s.max_hints, "hint", "hints")
    ));
    t.push_str(&format!(
        "**🕰️ Too late?**\nCodes still work for **{}** after a puzzle goes up. You'll be told whether your grid was right, but the sudoku points go to whoever was first. `/sudoku` lists the ones you haven't finished.\n\n",
        plural(s.late_hours, "hour", "hours")
    ));
    t.push_str("**🔒 Nothing to cheat**\nThe page never knows the answer — only the bot does, and it checks your code.\n\n");
    t.push_str("-# `/sudoku` your puzzles and links · `/sudokutop` the board · `/sudokuhelp` this · mods: `/sudokunew` for a fresh puzzle");
    t
}

/// The rules post that sits at the top of the sudoku channel: everything in
/// `/sudokuhelp`, and the small print about tries as well.
pub fn sudoku_rules_text(s: &SudokuRules) -> String {
    let mut t = sudoku_help_text(s);
    let mix = ["easy", "medium", "hard"];
    let total: u32 = s.mix.iter().sum();
    let shares: Vec<String> = mix
        .iter()
        .enumerate()
        .filter(|(i, _)| s.mix[*i] > 0)
        .map(|(i, name)| format!("{} {}%", name, (s.mix[i] as u64 * 100 / total.max(1) as u64)))
        .collect();
    let small = format!(
        "\n\n**📋 The small print**\n\
         • **{}** per puzzle, with a few seconds between them; a wrong code tells you HOW MANY squares are wrong, never which.\n\
         • How often each level comes up: {}.\n\
         • Nobody types in this channel — everything happens with the buttons on the card, which always sits at the bottom.",
        plural(s.max_tries, "try", "tries"),
        if shares.is_empty() { "as the panel sets it".to_string() } else { and_list(&shares, "and") }
    );
    t.push_str(&small);
    t
}

// --- the chess puzzle --------------------------------------------------------------------

pub const PUZZLE_RULES_TITLE: &str = "🧩 How the chess puzzle works";

/// What `/puzzlehelp` says, written from the settings as they are now.
///
/// Two things it must always say, whatever the settings: that an engine would
/// solve any of these instantly and the point is to do it yourself, and where
/// the puzzles came from - the bank's own attribution line, word for word.
pub fn puzzle_help_text(p: &PuzzleRules) -> String {
    let place = channel(p.channel).map(|c| format!(" in {}", c)).unwrap_or_else(|| " in the chess channel".to_string());
    let mut t = String::new();
    t.push_str(&format!(
        "**🧩 What it is**\nA real position from a real game is always waiting{}. One side has just blundered; \
         you have to find the move that wins it.\n\n",
        place
    ));

    t.push_str("**▶️ How to play**\n");
    t.push_str("• Press **🧩 Solve it** on the card. The bot hands you a private board of your own — tap a piece, tap where it goes.\n");
    t.push_str("• Play the whole winning line. The other side answers for itself between your moves.\n");
    t.push_str("• A wrong move is simply refused — *that's not it* — and you try again. Nothing is lost by guessing, and there is no limit on tries.\n");
    t.push_str("• If the position is a mate and you find a **different** mate, that counts: any move that mates is the right move.\n\n");

    t.push_str("**🏅 What it scores**\n");
    t.push_str(&format!(
        "• Everyone who solves a puzzle scores **puzzle points** — **{}** for an easy one, **{}** for a medium, **{}** for a hard. \
         They have no daily limit, they are **not** house points, and everyone has them: houses or no houses, mods included.\n",
        p.band_points[0], p.band_points[1], p.band_points[2]
    ));
    if p.cap > 0 && p.first > 0 {
        t.push_str(&format!(
            "• The **first** person to crack each puzzle also takes **{}**, up to **{}** a day. Nobody else earns house points from a puzzle.\n",
            plural(p.first, "house point", "house points"),
            plural(p.cap, "house point", "house points")
        ));
    } else {
        t.push_str("• Puzzles pay **no house points** at the moment: puzzle points are the whole of the score, and being first is for the glory. A mod can turn house points on from the panel.\n");
    }
    t.push_str("• `/puzzletop` is the board for today or this month, and `/puzzle` shows where you stand.\n\n");

    t.push_str("**⏱️ The pace**\n");
    t.push_str(&format!(
        "• Once somebody cracks it the puzzle stays up for another **{}**, so everyone else still gets their go.\n",
        plural(p.next_minutes, "minute", "minutes")
    ));
    t.push_str(&format!(
        "• One nobody solves is replaced after **{}**, with the answer shown on its card.\n",
        plural(p.idle_minutes, "minute", "minutes")
    ));
    if p.no_repeat_days > 0 {
        t.push_str(&format!("• The same puzzle doesn't come round again for **{}**.\n", plural(p.no_repeat_days, "day", "days")));
    }
    if let Some(n) = p.puzzles {
        t.push_str(&format!("• There are **{}** in the bank.\n", plural(n as i64, "puzzle", "puzzles")));
    }

    t.push_str("\n**🤖 About cheating**\n");
    t.push_str("• A chess engine would find any of these in a blink, and the bot has no way of knowing whether you used one. **The point is to find it yourself.** That is exactly why a puzzle is worth so little, and why the house points are capped — or off.\n");

    t.push_str(&format!("\n-# {}\n", p.attribution));
    t.push_str("-# `/puzzle` the one that's up · `/puzzletop` the board · `/puzzlehelp` this card · mods: `/puzzleskip` for a fresh one");
    t
}

// --- chess ------------------------------------------------------------------------------

/// "12 hours", "3 minutes", "45 seconds".
fn per_move_words(secs: i64) -> String {
    if secs >= 3600 && secs % 3600 == 0 {
        plural(secs / 3600, "hour", "hours")
    } else {
        duration_words(secs)
    }
}

/// What `/chesshelp` says, written from the live settings.
pub fn chess_help_text(c: &ChessRules) -> String {
    let place = channel(c.channel).map(|ch| format!(" in {}", ch)).unwrap_or_default();
    let mut t = String::new();
    t.push_str("**♟️ Starting a game**\n");
    t.push_str(&format!(
        "• Press **⚔️ Challenge someone** on the card{} — it is on the idle card and on every game card — then pick a \
         member and a pace from the menus and press **Send challenge**. Nobody types in that channel, so there is \
         nothing to type here either.\n",
        place
    ));
    t.push_str("• Or `/chess @member` from anywhere, which does exactly the same thing.\n");
    t.push_str(
        "• Either way the same challenge card goes up. They press **✅ Accept** (or either of you presses \
         **❌ Decline**); an offer nobody answers lapses after 30 minutes.\n",
    );
    t.push_str("• Colours are drawn at random when the challenge is accepted.\n");
    t.push_str(&format!(
        "• One open challenge per pair. You can have **{}** on the go, and the channel holds **{}** in all.\n",
        plural(c.max_games, "game", "games"),
        plural(c.max_active, "game", "games")
    ));
    t.push_str("• Every game keeps its own card, and the card of whichever game moved last sits at the bottom of the channel.\n");

    t.push_str("\n**🕰️ Time controls**\n");
    t.push_str(&format!("• **Casual** — **{}** for each move. Play over a day or two; the bot tags you when you have under two hours left.\n", per_move_words(c.casual_hours * 3600)));
    t.push_str(&format!("• **Live** — **{}** for each move. Play it out in one sitting.\n", per_move_words(c.live_seconds)));
    t.push_str("• Run out of time on your move and you lose the game. Pick the pace with `/chess @someone time:`.\n");

    t.push_str("\n**✍️ Making a move**\n");
    t.push_str("• **♟️ Open board** gives you a private link. Tap a piece, tap where it goes; legal moves light up and the page saves at once. The link is yours alone and stops working when the game ends.\n");
    t.push_str("• **✍️ Type move** takes `Nf3`, `exd5`, `O-O`, `e8=Q`, or just the two squares: `g1f3`, `e7e8q`.\n");
    t.push_str("• Anything wrong or unclear is refused with the reason, and it stays your move.\n");
    t.push_str("• Only the two players' buttons work; nobody types in the chess channel.\n");

    t.push_str("\n**👀 Watching**\n");
    t.push_str("• Every game card has a **👀 Watch** link. It opens a page anyone can read: the board, the clock and the moves as they happen. No sign-in, no way to move a piece, and nobody's private board link is on it.\n");
    if c.replay_days > 0 {
        t.push_str(&format!(
            "• When the game ends that same link becomes a **replay** you can step through move by move, kept for **{}**.\n",
            plural(c.replay_days, "day", "days")
        ));
    }

    t.push_str("\n**🏳️ Ending it**\n");
    t.push_str("• Normal chess: checkmate, stalemate, the same position three times, fifty moves with nothing taken, and too little material left to mate.\n");
    t.push_str("• **🏳️ Resign** gives the game up, and **🤝 Offer draw** asks the other side, who gets Accept or Play on.\n");
    t.push_str("• A game should be finished within a day or so; a mod can clear a stuck one.\n");

    t.push_str("\n**♟️ Chess points**\n");
    t.push_str(&format!("• Winner **+{}**, or **+{}** each for a draw.\n", c.win, c.draw));
    t.push_str("• Chess points are the game's own score. There is **no daily limit** on them, it makes no difference which house either of you is in, and mods and anyone not yet sorted have them too.\n");
    t.push_str("• Games of chess move **nothing** in the House Cup — the Cup is won elsewhere, and this board is chess's own.\n");
    t.push_str("• The same pair is scored for one game a day, so a rematch is for pride.\n");
    if c.min_plies > 0 {
        t.push_str(&format!(
            "• Give up in the first **{}** and nobody scores. Losing on time always scores for the winner, however short the game.\n",
            plural((c.min_plies + 1) / 2, "move", "moves")
        ));
    }
    t.push_str("• `/chesstop` shows the board for today or this month, and the day's top scorer is the one the frog card goes to.\n");

    t.push_str("\n**⌨️ Commands**\n");
    t.push_str("`/chess @someone time:` challenge · `/chess` your games, their board links and your chess points · `/chesstop` the chess points board · `/chesshelp` this card");
    t
}

// --- anagrams -----------------------------------------------------------------------------

pub const ANAGRAM_RULES_TITLE: &str = "🔀 How Anagrams works";

fn anagram_where(id: Option<u64>) -> String {
    match channel(id) {
        Some(c) => format!("in {}", c),
        None => "in its own channel (a mod has to set one)".to_string(),
    }
}

/// What `/anagramhelp` says: the whole game in one card, from the settings as
/// they are now.
pub fn anagram_help_text(a: &AnagramRules) -> String {
    let mut t = format!(
        "**🔀 What it is**\nThe bot shuffles a word's letters and puts them up {}. Be the first to type a word that uses **all** of them and your house scores.\n\n",
        anagram_where(a.channel)
    );
    t.push_str("**⌨️ How to play**\n");
    t.push_str("• Just type your answer in the channel — no buttons, no commands. Capitals, spaces around it and a `!` on the end are all forgiven.\n");
    t.push_str("• **Any** word that uses every letter counts, not only the one I scrambled: BEAST's letters are taken by `bates` and `tabes` just as happily.\n");
    t.push_str("• The first right answer wins, gets a ✅ on the message, and the next scramble goes up at once.\n");
    t.push_str("• A wrong guess is simply ignored — nobody is corrected in public, so guess away.\n\n");

    t.push_str("**💡 Stuck?**\n");
    t.push_str("• `!hint` gives away the **first letter** of the word I scrambled. One hint to a round, and it takes a point off what that round pays (never below one).\n");
    t.push_str("• `!skip` moves on to a new word, but only once a hint has been used. It pays nobody.\n");
    t.push_str(&format!("• A round nobody answers is replaced after **{}**, so the channel is never stuck on one word.\n\n", plural(a.idle_minutes, "minute", "minutes")));

    t.push_str("**🏠 House points**\n");
    t.push_str(&format!(
        "• **{}** for a 4–5 letter word, **{}** for 6–7, **{}** for 8 or more.\n",
        a.points[0], a.points[1], a.points[2]
    ));
    t.push_str(&format!("• {}\n", match a.cap {
        Some(n) => format!("Up to **{}** a day from anagrams.", plural(n, "house point", "house points")),
        None => "No daily limit from anagrams.".to_string(),
    }));
    t.push_str("• Muggles and anyone not yet sorted earn no house points, here as everywhere — mods are welcome to play, they just can't score for a house.\n");
    t.push_str("\n**🔀 Anagram points**\n");
    t.push_str("• Every solve also scores **anagram points**: what the round was worth, the hint taken off, with no daily limit at all. They keep counting once your house points are capped, and everyone has them — mods and Muggles included.\n");
    t.push_str("• `/anagramtop` shows the board for today or this month, and the day's top scorer is the one the frog card goes to.\n");
    if a.no_repeat_days > 0 {
        t.push_str(&format!("• The same set of letters doesn't come round again for **{}**.\n", plural(a.no_repeat_days, "day", "days")));
    }
    if let Some(words) = a.words {
        t.push_str(&format!("-# {} in the bank.\n", plural(words as i64, "word", "words")));
    }

    t.push_str("\n**⌨️ Commands**\n");
    t.push_str("`/anagram` the round that's up · `/anagramtop` the anagram points board · `/anagramhelp` this card · mods: `/anagramskip` for a fresh word, `/anagramstop` to switch it off");
    t
}

// --- guess the word -------------------------------------------------------------------------

pub const GUESS_RULES_TITLE: &str = "🎨 How Guess the Word works";

fn guess_where(id: Option<u64>) -> String {
    match channel(id) {
        Some(c) => format!("in {}", c),
        None => "in its own channel (a mod has to set one)".to_string(),
    }
}

/// What `/guesshelp` says: the whole game in one card, from the settings as
/// they are now. The doodles are somebody else's work, so the credit the
/// dataset's licence asks for goes out with every telling of the rules.
pub fn guess_help_text(g: &GuessRules) -> String {
    let mut t = format!(
        "**🎨 What it is**\nThe bot puts a hand-drawn doodle up {}. Be the first to type what it is and your house scores.\n\n",
        guess_where(g.channel)
    );
    t.push_str("**⌨️ How to play**\n");
    t.push_str("• Just type your guess in the channel — no buttons, no commands. Capitals, spaces and punctuation are all forgiven, so `Ice-Cream`, `ice cream` and `icecream` are one answer.\n");
    t.push_str("• Near enough counts on longer words: `gitar` takes a guitar. Short words have to be right.\n");
    t.push_str("• The first right guess wins, gets a ✅ on the message, and the next doodle goes up at once.\n");
    t.push_str("• A wrong guess is simply ignored — nobody is corrected in public, so guess away.\n\n");

    t.push_str("**💡 Stuck?**\n");
    t.push_str("• `!hint` puts a **second drawing of the same thing** up beside the first and gives away the word's **first letter**. One hint to a round, and it takes a point off what that round pays (never below one).\n");
    t.push_str("• `!skip` moves on to a new doodle, but only once a hint has been used. It pays nobody.\n");
    t.push_str(&format!("• A round nobody gets is replaced after **{}**, so the channel is never stuck on one picture.\n\n", plural(g.idle_minutes, "minute", "minutes")));

    t.push_str("**🏠 House points**\n");
    t.push_str(&format!("• **{}** for naming the doodle first.\n", plural(g.points, "house point", "house points")));
    t.push_str(&format!("• {}\n", match g.cap {
        Some(n) => format!("Up to **{}** a day from guessing.", plural(n, "house point", "house points")),
        None => "No daily limit from guessing.".to_string(),
    }));
    t.push_str("• Muggles and anyone not yet sorted earn no house points, here as everywhere — mods are welcome to play, they just can't score for a house.\n");
    t.push_str("\n**🎨 Guess points**\n");
    t.push_str("• Every doodle you name also scores **guess points**: what the round was worth, hint taken off, with no daily limit. They keep counting once your house points are capped, and everyone has them — mods and Muggles included.\n");
    t.push_str("• `/guesstop` shows the board for today or this month, and the day's top scorer takes the frog card.\n");
    if g.no_repeat_days > 0 {
        t.push_str(&format!("• Neither the same word nor the same drawing comes round again for **{}**.\n", plural(g.no_repeat_days, "day", "days")));
    }
    if let Some(words) = g.words {
        t.push_str(&format!("-# {} in the bank.\n", plural(words as i64, "word", "words")));
    }

    t.push_str("\n**⌨️ Commands**\n");
    t.push_str("`/guess` the doodle that's up · `/guesstop` the board · `/guesshelp` this card · mods: `/guessskip` for a fresh one, `/guessstop` to switch it off\n");
    t.push_str(&format!("-# {}", super::guess_bank::ATTRIBUTION));
    t
}

/// The rules post as it goes up: plain text under a heading when it fits one
/// message, otherwise `None` and it goes in an embed with the title.
pub fn npat_rules_message(n: &NpatRules) -> Option<String> {
    let text = format!("## {}\n\n{}", NPAT_RULES_TITLE, npat_rules_text(n));
    (text.chars().count() <= MESSAGE_LIMIT).then_some(text)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The shipped defaults, with every channel set.
    pub(crate) fn defaults() -> Rules {
        Rules {
            scoreboard_on: true,
            house_channel: Some(1548371226890604665),
            quiz_channel: Some(1547862192932528138),
            fight_channel: Some(1548160947766698074),
            draw_minimum: 10,
            activity_on: true,
            chat_tiers: vec![20, 60, 150],
            chat_cap: Some(3),
            voice_minutes: 60,
            voice_cap: Some(4),
            voice_company: true,
            voice_ignore_deaf: true,
            quiz: [2, 1, 1],
            quiz_cap: Some(6),
            games_on: true,
            koto_win: 3,
            koto_played: 1,
            koto_cap: Some(4),
            anagram: 3,
            anagram_cap: Some(6),
            cat: [1, 2, 3],
            cat_cap: Some(3),
            wordle_on: true,
            wordle: [4, 3, 2, 1, 1],
            arena_win: 1,
            arena_cap: Some(3),
            royale: [8, 3],
            battle_daily: true,
            battle_times: "20:00, 15:00".into(),
            snitch_on: true,
            snitch_drops: (6, 8),
            snitch_hours: (0, 24),
            snitch_points: [[2, 1, 1], [3, 2, 1], [6, 4, 2]],
            snitch_cap: None,
            snitch_second_chance: true,
            frogs_on: true,
            frog_drops_max: 10,
            frog_open_minutes: 5,
            frog_points: [2, 4, 10],
            frog_cap: None,
            cards: Some(10),
            set_bonus: 35,
            trades_on: true,
            daily_top_cards: true,
            royale_cards: true,
            npat: npat_defaults(),
            sudoku: sudoku_defaults(),
            chess: chess_defaults(),
            anagrams: anagram_defaults(),
            guess: guess_defaults(),
        }
    }

    pub(crate) fn guess_defaults() -> GuessRules {
        GuessRules {
            channel: Some(super::super::guess::HOME_CHANNEL),
            points: 2,
            cap: Some(10),
            idle_minutes: 15,
            no_repeat_days: 14,
            words: Some(240),
        }
    }

    pub(crate) fn anagram_defaults() -> AnagramRules {
        AnagramRules {
            channel: Some(1542764196901683231),
            points: [1, 2, 3],
            cap: Some(10),
            idle_minutes: 30,
            no_repeat_days: 30,
            words: Some(2500),
        }
    }

    pub(crate) fn sudoku_defaults() -> SudokuRules {
        SudokuRules {
            channel: Some(1544347052090327040),
            points: [2, 4, 6],
            hint_cost: 1,
            max_hints: 3,
            max_tries: 10,
            late_hours: 24,
            mix: [40, 40, 20],
            has_page: true,
        }
    }

    pub(crate) fn puzzle_defaults() -> PuzzleRules {
        PuzzleRules {
            channel: Some(1549625408004165682),
            first: 1,
            cap: 0,
            band_points: [1, 2, 3],
            next_minutes: 10,
            idle_minutes: 60,
            no_repeat_days: 60,
            puzzles: Some(4_000),
            attribution: super::super::puzzle_bank::ATTRIBUTION.to_string(),
        }
    }

    pub(crate) fn chess_defaults() -> ChessRules {
        ChessRules {
            channel: Some(1549625408004165682),
            casual_hours: 12,
            live_seconds: 180,
            max_games: 3,
            max_active: 8,
            min_plies: 10,
            replay_days: 30,
            win: 4,
            draw: 1,
        }
    }

    pub(crate) fn npat_defaults() -> NpatRules {
        NpatRules {
            channel: Some(1549000000000000001),
            min_players: 5,
            min_houses: 2,
            join_secs: 180,
            start_secs: 30,
            letters: "ABCDEFGHIJKLMNOPRSTUVW".into(),
            letters_per_game: 5,
            round_secs: 45,
            break_secs: 20,
            scores: [10, 5],
            prizes: [2, 1],
            cap: Some(6),
            min_scored: 3,
        }
    }

    /// Everything turned up as far as the panel allows.
    fn huge() -> Rules {
        Rules {
            house_channel: Some(u64::MAX),
            quiz_channel: Some(u64::MAX),
            fight_channel: Some(u64::MAX),
            draw_minimum: 1000,
            chat_tiers: vec![4999, 5000, 5001],
            voice_minutes: 1440,
            voice_cap: Some(24),
            quiz: [100, 100, 100],
            quiz_cap: Some(99),
            koto_win: 100,
            koto_played: 100,
            koto_cap: Some(99),
            anagram: 100,
            anagram_cap: Some(99),
            cat: [100, 100, 99],
            cat_cap: Some(99),
            wordle: [100, 100, 100, 100, 100],
            arena_win: 100,
            arena_cap: Some(99),
            royale: [1000, 1000],
            battle_times: (0..24).map(|h| format!("{:02}:30", h)).collect::<Vec<_>>().join(","),
            snitch_drops: (20, 20),
            snitch_hours: (10, 24),
            snitch_points: [[100; 3]; 3],
            snitch_cap: Some(99),
            frog_drops_max: 30,
            frog_open_minutes: 60,
            frog_points: [1000, 1000, 1000],
            frog_cap: Some(99),
            cards: Some(50),
            set_bonus: 1000,
            npat: NpatRules {
                channel: Some(u64::MAX),
                min_players: 50,
                min_houses: 4,
                join_secs: 1799,
                start_secs: 599,
                letters: "A".into(),
                letters_per_game: 10,
                round_secs: 599,
                break_secs: 599,
                scores: [1000, 1000],
                prizes: [100, 100],
                cap: Some(99),
                min_scored: 50,
            },
            sudoku: SudokuRules {
                channel: Some(u64::MAX),
                points: [100, 100, 100],
                hint_cost: 100,
                max_hints: 80,
                max_tries: 100,
                late_hours: 720,
                mix: [1000, 1000, 1000],
                has_page: true,
            },
            chess: ChessRules { casual_hours: 72, live_seconds: 3600, max_games: 20, max_active: 50, min_plies: 200, replay_days: 365, win: 100, draw: 100, ..chess_defaults() },
            anagrams: AnagramRules { channel: Some(u64::MAX), points: [100, 100, 100], cap: Some(99), idle_minutes: 1440, no_repeat_days: 365, words: Some(500_000) },
            guess: GuessRules { channel: Some(u64::MAX), points: 100, cap: Some(99), idle_minutes: 1440, no_repeat_days: 365, words: Some(500_000) },
            ..defaults()
        }
    }

    #[test]
    fn times_and_clock_words_read_naturally() {
        assert_eq!(times_words("20:00, 15:00"), "3 pm & 8 pm");
        assert_eq!(times_words("21:00"), "9 pm");
        assert_eq!(times_words("12:00,00:00,09:30,bad,25:00"), "midnight, 9:30 am & noon");
        assert_eq!(times_words(""), "");
    }

    #[test]
    fn the_welcome_uses_live_channels_and_drops_unset_ones() {
        let r = defaults();
        let text = welcome_text(&r);
        assert!(text.starts_with("## 🏰 Welcome to the House Cup!"), "{}", text);
        assert!(text.contains("🦁 **Gryffindor**, 🐍 **Slytherin**, 🦅 **Ravenclaw** or 🦡 **Hufflepuff**"), "{}", text);
        assert!(text.contains("under the **The Houses** category"), "{}", text);
        assert!(text.contains("> 🦁 `gryffindor-tower` · 🐍 `slytherin-dungeons` · 🦅 `ravenclaw-library` · 🦡 `hufflepuff-kitchens`"), "{}", text);
        assert!(text.contains("🏆 <#1548371226890604665> · points updates, sorting and house news"), "{}", text);
        assert!(text.contains("⚔️ <#1548160947766698074> · fights and the daily Battle Royale"), "{}", text);
        assert!(text.contains("where the 🪽 Golden Snitch and 🐸 Chocolate Frogs appear"), "{}", text);
        assert!(text.ends_with("go win it for your house. ⬇️"));
        assert!(text.chars().count() < MESSAGE_LIMIT, "{} chars", text.chars().count());
        assert!(welcome_text(&huge()).chars().count() < MESSAGE_LIMIT);

        let bare = Rules { quiz_channel: None, fight_channel: None, frogs_on: false, ..defaults() };
        let text = welcome_text(&bare);
        assert!(!text.contains("🧠 <#") && !text.contains("⚔️ <#"), "{}", text);
        assert!(text.contains("where the 🪽 Golden Snitch appears"), "{}", text);
        let none = Rules { house_channel: None, quiz_channel: None, fight_channel: None, frogs_on: false, snitch_on: false, npat: NpatRules { channel: None, ..npat_defaults() }, sudoku: SudokuRules { channel: None, ..sudoku_defaults() }, chess: ChessRules { channel: None, ..chess_defaults() }, anagrams: AnagramRules { channel: None, ..anagram_defaults() }, guess: GuessRules { channel: None, ..guess_defaults() }, ..defaults() };
        assert!(!welcome_text(&none).contains("Where the action is"));
    }

    #[test]
    fn the_cards_post_matches_the_draft_with_the_defaults() {
        let text = snitch_cards_text(&defaults()).unwrap();
        for part in [
            "## ✨ The Golden Snitch & Chocolate Frog Cards",
            "A Snitch zooms into an active chat **6–8 times a day**, day or night.",
            "> 🥉 Bronze **2 · 1 · 1**  ·  🥈 Silver **3 · 2 · 1**  ·  🥇 Gold **6 · 4 · 2**",
            "• No daily limit\n",
            "comes back for a second chance 👀",
            "**up to 10 times a day**",
            "• You get **3 tries**, and small spelling slips are forgiven",
            "> 🥛 Common **2**  ·  🍫 Uncommon **4**  ·  🔥 The Eternal Phoenix **10**",
            "• Nobody gets it in 5 minutes? It hops away",
            "There are **10 cards** to collect",
            "• 🏆 `/sellset` hand in one of **all 10** for **+35** house points (you keep your catch points)",
            "**…or keep them.** Low numbers",
            "• 🏠 `/housecards` what your house holds, by card or by member",
            "📖 `/frogs` your collection · 🔎 `/frogcard` look up any card by number · 🏠 `/housecards` your house's cards · 🔁 `/trades` your offers",
        ] {
            assert!(text.contains(part), "missing {:?} in\n{}", part, text);
        }
        assert!(text.chars().count() < MESSAGE_LIMIT, "{} chars", text.chars().count());
        let big = snitch_cards_text(&huge()).unwrap();
        assert!(big.chars().count() < MESSAGE_LIMIT, "{} chars with big numbers", big.chars().count());
        assert!(big.contains("• Up to 99 points a day (Gold has no limit)"), "{}", big);
        assert!(big.contains("between 10 am and midnight India time"), "{}", big);
        assert!(big.contains("**20 times a day**"), "{}", big);
        assert_eq!(snitch_cards_images(&defaults()), vec!["snitch", "frog"]);
    }

    #[test]
    fn the_cards_post_leaves_out_what_is_switched_off() {
        let no_frogs = Rules { frogs_on: false, trades_on: false, daily_top_cards: false, royale_cards: false, ..defaults() };
        let text = snitch_cards_text(&no_frogs).unwrap();
        assert!(text.starts_with("## ✨ The Golden Snitch\n"), "{}", text);
        assert!(!text.contains("Frog") && !text.contains("/sellset"), "{}", text);
        assert_eq!(snitch_cards_images(&no_frogs), vec!["snitch"]);

        let quiet = Rules { snitch_second_chance: false, daily_top_cards: false, royale_cards: false, trades_on: false, set_bonus: 0, ..defaults() };
        let text = snitch_cards_text(&quiet).unwrap();
        assert!(!text.contains("second chance") && !text.contains("Top of the day") && !text.contains("Bonus cards"), "{}", text);
        assert!(!text.contains("/trade") && !text.contains("/sellset") && !text.contains("Tip"), "{}", text);
        assert!(text.contains("**Keep them.**"), "{}", text);

        assert!(snitch_cards_text(&Rules { snitch_on: false, frogs_on: false, ..defaults() }).is_none());
    }

    #[test]
    fn the_guide_has_three_numbered_sections_and_stays_inside_discord_limits() {
        let panels = guide(&defaults(), true);
        let titles: Vec<&str> = panels.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["📖 How to play · House Cup for beginners", "💬 1 · Just hang out", "🎮 2 · Play the games", "📊 3 · Check your progress"]
        );
        assert!(panels[0].body.contains("Snitch and card rules are in the post above ⬆️"), "{}", panels[0].body);
        assert!(panels[0].body.contains("(**10+** points that month) win **Discord Nitro**"), "{}", panels[0].body);
        assert_eq!(panels.last().unwrap().footer.as_deref(), Some(GUIDE_FOOTER));
        assert!(panels[..panels.len() - 1].iter().all(|p| p.footer.is_none()));
        let hang = &panels[1].body;
        assert!(hang.contains("💬 **Chat** — 20, 60 and 150 messages in a day → **1 point each** (max 3)"), "{}", hang);
        assert!(hang.contains("🎙️ **Voice** — every full hour in VC **with at least one other person** → **1 point** (max 4)"), "{}", hang);
        assert!(hang.contains("-# Alone in VC or only with a music bot doesn't count, and neither does deafened time."), "{}", hang);
        let games = &panels[2].body;
        assert!(games.contains("🧠 **Quiz** in <#1547862192932528138> — vote a genre, answer first; top 3 of each round get **2 · 1 · 1** (max 6)"), "{}", games);
        assert!(games.contains("🐱 **Cat Bot** — catch a cat **1–3** by rarity (max 3)"), "{}", games);
        assert!(games.contains("🟩 **Wordle** — solve in 1–2 **4** · 3 **3** · 4 **2** · 5–6 **1**, 👑 best of the day **+1**"), "{}", games);
        assert!(games.contains("daily at **3 pm & 8 pm** in <#1548160947766698074>: press Join; champion **+8**, runner-up **+3** (no limit)"), "{}", games);
        assert!(games.contains("🔤 **Name Place Animal Thing** in <#1549000000000000001> — press I'm in; a game is 5 letters, and its best two house members win 🥇 **+2** · 🥈 **+1** (max 6)"), "{}", games);
        let no_npat = guide(&Rules { npat: NpatRules { channel: None, ..npat_defaults() }, ..defaults() }, true);
        assert!(!no_npat[2].body.contains("Name Place Animal Thing"), "left out while off or without a channel");
        assert!(panels[3].body.contains("`/frogs` your cards") && panels[3].body.contains("scoreboard below ⬇️"));
        assert!(panels.len() <= 10);
        for r in [defaults(), huge()] {
            let panels = guide(&r, true);
            assert!(embed_chars(&panels) < EMBEDS_LIMIT, "{} chars", embed_chars(&panels));
            assert!(panels.iter().all(|p| p.body.chars().count() <= DESCRIPTION_LIMIT && p.title.chars().count() <= 256));
        }
    }

    /// Sudoku is advertised as its own score now, never as house points: it
    /// pays the House Cup nothing, so it drops out of the "how to earn" list
    /// the way the chat line does once its limit is nought, and says what it
    /// does pay in a line of its own.
    #[test]
    fn the_guide_and_the_earn_button_carry_sudoku_from_its_live_settings() {
        let games = &guide(&defaults(), true)[2].body;
        assert!(
            games.contains(
                "🔢 **Sudoku** in <#1544347052090327040> — one is always waiting; the first correct code wins 🟢 **2** · 🟡 **4** · 🔴 **6** **sudoku points**, the game's own score (no house points, no daily limit — `/sudokutop` is the board)"
            ),
            "{}",
            games
        );
        let earn = earn_text(&defaults());
        // Not in the list of things that pay a house, and no limit is claimed.
        assert!(!earn.contains("🔢 Sudoku 2–6 first solver"), "{}", earn);
        assert!(!earn.contains("(max 20)"), "{}", earn);
        assert!(earn.contains("-# 🔢 Sudoku pays **sudoku points** (2–6 a puzzle, no limit), not house points · `/sudokutop`"), "{}", earn);
        // The list it left still reads as a list, with no gap where it was.
        let more = earn.lines().find(|l| l.contains("🔤 NPAT")).expect("the games line");
        assert!(!more.contains("Sudoku") && !more.contains(" ·  · ") && !more.ends_with(" · "), "{}", more);
        assert!(more.contains("🔤 NPAT game 1st +2 · 2nd +1 (max 6) · 🔀 Anagrams"), "{}", more);
        // Off, or with no channel set, it is left out of both.
        let off = Rules { sudoku: SudokuRules { channel: None, ..sudoku_defaults() }, ..defaults() };
        assert!(!guide(&off, true)[2].body.contains("Sudoku"));
        assert!(!earn_text(&off).contains("Sudoku"));
        let free = Rules { sudoku: SudokuRules { points: [0, 0, 0], ..sudoku_defaults() }, ..defaults() };
        assert!(!guide(&free, true)[2].body.contains("Sudoku"), "a game worth nothing isn't advertised");
        assert!(!earn_text(&free).contains("Sudoku"));
        // And nowhere in the whole guide, or the earn card, is sudoku a house-point source.
        let everything = guide(&defaults(), true).iter().map(|p| p.body.clone()).collect::<Vec<_>>().join("\n");
        for line in everything.lines().chain(earn.lines()).filter(|l| l.contains("Sudoku")) {
            let claims = line.to_lowercase().contains("house point") && !line.contains("no house points") && !line.contains("not house points");
            assert!(!claims, "sudoku is advertised as paying a house: {}", line);
        }
    }

    #[test]
    fn sudokuhelp_explains_the_game_from_the_settings_and_fits_one_message() {
        let text = sudoku_help_text(&sudoku_defaults());
        for part in [
            "always waiting in <#1544347052090327040>",
            "**▶️ Play** on the puzzle card",
            "**Copy code**",
            "📋 Submit code",
            "🟢 Easy **2** · 🟡 Medium **4** · 🔴 Hard **6**",
            "Sudoku pays **sudoku points**, this game's own score",
            "They are **not** house points",
            "`/sudokutop` is the board",
            "takes **1 sudoku point** off",
            "3 hints per puzzle",
            "**24 hours**",
            "never knows the answer",
            "`/sudoku`",
            "`/sudokunew`",
        ] {
            assert!(text.contains(part), "missing “{}” in:\n{}", part, text);
        }
        // Nothing in the card claims a house point or a daily limit.
        assert!(!text.contains("a day"), "sudoku has no daily limit to name:\n{}", text);
        for line in text.lines().filter(|l| l.to_lowercase().contains("house point")) {
            assert!(line.contains("**not** house points"), "{}", line);
        }
        assert!(text.chars().count() < MESSAGE_LIMIT, "{} chars", text.chars().count());
        // Every setting at its highest, and with no web page set up.
        let big = SudokuRules { has_page: false, ..huge().sudoku };
        let text = sudoku_help_text(&big);
        assert!(text.contains("81 digits") && text.contains("isn't set up yet"), "{}", text);
        assert!(text.contains("720 hours") && text.contains("80 hints"));
        assert!(text.chars().count() < MESSAGE_LIMIT, "{} chars at the maximum", text.chars().count());
        // No channel set at all still reads.
        let nowhere = SudokuRules { channel: None, hint_cost: 1, ..sudoku_defaults() };
        let text = sudoku_help_text(&nowhere);
        assert!(text.contains("in its own channel") && text.contains("nothing here is capped"), "{}", text);
    }

    #[test]
    fn anagramhelp_explains_a_game_played_by_typing_and_fits_one_card() {
        let text = anagram_help_text(&anagram_defaults());
        for part in [
            "puts them up in <#1542764196901683231>",
            "type your answer in the channel",
            "`bates` and `tabes`",
            "first right answer wins",
            "ignored",
            "`!hint` gives away the **first letter**",
            "`!skip` moves on",
            "**30 minutes**",
            "**1** for a 4–5 letter word, **2** for 6–7, **3** for 8 or more",
            "Up to **10 house points** a day",
            "doesn't come round again for **30 days**",
            "2500 words in the bank",
            "`/anagramskip`",
            "`/anagramstop`",
        ] {
            assert!(text.contains(part), "missing “{}” in:\n{}", part, text);
        }
        assert!(!text.contains("beast"), "the help never names a live word");
        assert!(text.chars().count() < MESSAGE_LIMIT, "{} chars", text.chars().count());
        // Every setting at its highest still fits an embed.
        assert!(anagram_help_text(&huge().anagrams).chars().count() < DESCRIPTION_LIMIT);
        // No channel, no limit, no bank read yet, and no no-repeat window.
        let bare = AnagramRules { channel: None, cap: None, words: None, no_repeat_days: 0, ..anagram_defaults() };
        let text = anagram_help_text(&bare);
        assert!(text.contains("in its own channel") && text.contains("No daily limit"), "{}", text);
        assert!(!text.contains("in the bank") && !text.contains("come round again"), "{}", text);
        assert!(ANAGRAM_RULES_TITLE.contains("Anagrams"));
    }

    #[test]
    fn guesshelp_explains_a_game_played_by_typing_and_credits_the_doodles() {
        let text = guess_help_text(&guess_defaults());
        for part in [
            "doodle up in <#1518233664016617582>",
            "type your guess in the channel",
            "`gitar` takes a guitar",
            "first right guess wins",
            "ignored",
            "second drawing of the same thing",
            "**first letter**",
            "`!skip` moves on",
            "**15 minutes**",
            "**2 house points** for naming the doodle first",
            "Up to **10 house points** a day",
            "comes round again for **14 days**",
            "240 words in the bank",
            "`/guessskip`",
            "`/guessstop`",
            "Quick, Draw!",
            "CC BY 4.0",
        ] {
            assert!(text.contains(part), "missing \u{201c}{}\u{201d} in:\n{}", part, text);
        }
        assert!(text.chars().count() < MESSAGE_LIMIT, "{} chars", text.chars().count());
        // Every setting at its highest still fits an embed.
        assert!(guess_help_text(&huge().guess).chars().count() < DESCRIPTION_LIMIT);
        // No channel, no limit, no bank read yet, and no no-repeat window.
        let bare = GuessRules { channel: None, cap: None, words: None, no_repeat_days: 0, ..guess_defaults() };
        let text = guess_help_text(&bare);
        assert!(text.contains("in its own channel") && text.contains("No daily limit"), "{}", text);
        assert!(!text.contains("in the bank") && !text.contains("comes round again"), "{}", text);
        assert!(GUESS_RULES_TITLE.contains("Guess the Word"));
    }

    #[test]
    fn the_guide_and_the_earn_button_carry_anagrams_from_its_live_settings() {
        let games = &guide(&defaults(), true)[2].body;
        assert!(
            games.contains("🔀 **Anagrams** in <#1542764196901683231> — unscramble the letters and just type the word; any word using all of them counts, **1** · **2** · **3** by length (max 10)"),
            "{}",
            games
        );
        assert!(earn_text(&defaults()).contains("🔀 Anagrams 1–3 first to type it (max 10)"), "{}", earn_text(&defaults()));
        // Off, or worth nothing: not advertised at all.
        assert!(
            games.contains("🎨 **Guess the Word** in <#1518233664016617582> — a doodle is always up; first to type what it is wins **2** (max 10)"),
            "{}",
            games
        );
        assert!(earn_text(&defaults()).contains("🎨 Guess the Word 2 first to name it (max 10)"), "{}", earn_text(&defaults()));
        let no_guess = Rules { guess: GuessRules { channel: None, ..guess_defaults() }, ..defaults() };
        assert!(!guide(&no_guess, true)[2].body.contains("Guess the Word") && !earn_text(&no_guess).contains("Guess the Word"));
        let off = Rules { anagrams: AnagramRules { channel: None, ..anagram_defaults() }, ..defaults() };
        assert!(!guide(&off, true)[2].body.contains("Anagrams") && !earn_text(&off).contains("Anagrams"));
        let free = Rules { anagrams: AnagramRules { points: [0, 0, 0], ..anagram_defaults() }, ..defaults() };
        assert!(!guide(&free, true)[2].body.contains("Anagrams"));
        // And the welcome points people at the channel.
        assert!(welcome_text(&defaults()).contains("🔀 <#1542764196901683231> · anagrams"));
    }

    #[test]
    fn the_sudoku_rules_post_adds_the_small_print_and_fits_an_embed() {
        let text = sudoku_rules_text(&sudoku_defaults());
        assert!(text.starts_with(&sudoku_help_text(&sudoku_defaults())), "the post is the help plus more");
        assert!(text.contains("**10 tries** per puzzle"), "{}", text);
        assert!(text.contains("easy 40%, medium 40% and hard 20%"), "{}", text);
        assert!(text.contains("never which"));
        assert!(SUDOKU_RULES_TITLE.contains("Sudoku"));
        for r in [sudoku_defaults(), huge().sudoku] {
            assert!(sudoku_rules_text(&r).chars().count() < DESCRIPTION_LIMIT);
        }
        // A mix with only one level in it.
        let one = SudokuRules { mix: [0, 0, 5], ..sudoku_defaults() };
        assert!(sudoku_rules_text(&one).contains("hard 100%"));
    }

    #[test]
    fn the_guide_renumbers_and_reads_no_limit_when_things_change() {
        let r = Rules { activity_on: false, quiz_channel: None, voice_cap: None, koto_cap: None, battle_daily: false, frogs_on: false, ..defaults() };
        let panels = guide(&r, false);
        assert_eq!(panels[1].title, "🎮 1 · Play the games");
        assert_eq!(panels[2].title, "📊 2 · Check your progress");
        assert!(!panels[0].body.contains("post above"));
        assert!(panels[1].body.contains("🔤 **Koto** — solve **3**, **1** for guesses that score (no limit)"), "{}", panels[1].body);
        assert!(panels[1].body.contains("when one opens in <#1548160947766698074>, press Join"), "{}", panels[1].body);
        assert!(!panels[1].body.contains("Quiz"));
        assert!(!panels[2].body.contains("/frogs"));

        let lonely = Rules { voice_company: false, voice_minutes: 30, voice_cap: None, ..defaults() };
        let hang = &guide(&lonely, true)[1].body;
        assert!(hang.contains("every full 30 minutes in VC → **1 point** (no limit)"), "{}", hang);
        assert!(!hang.contains("music bot"));
        assert!(hang.contains("-# Deafened time doesn't count."), "{}", hang);
        let hearing = Rules { voice_ignore_deaf: false, ..defaults() };
        let hang = &guide(&hearing, true)[1].body;
        assert!(hang.contains("-# Alone in VC or only with a music bot doesn't count.") && !hang.contains("deafened"), "{}", hang);
        let hang = &guide(&Rules { voice_ignore_deaf: false, ..lonely }, true)[1].body;
        assert!(!hang.contains("-#"), "{}", hang);
    }

    #[test]
    fn how_to_earn_is_short_and_live() {
        let text = earn_text(&defaults());
        assert!(text.contains("💬 Chat 20·60·150 msgs → 1 each (max 3)"), "{}", text);
        assert!(text.contains("🎙️ VC with others → 1/hour (max 4) · deafened time doesn't count"), "{}", text);
        assert!(!earn_text(&Rules { voice_ignore_deaf: false, ..defaults() }).contains("deafened"));
        assert!(text.contains("🧠 Quiz podium 2·1·1 · 🔤 Koto 3 · 🔡 Anagram 3 · 🐱 Cats 1–3"), "{}", text);
        assert!(text.contains("🪽 Snitch 1–6 · 🐸 Frogs 2–10 (no limit)"), "{}", text);
        assert!(text.contains("👑 Royale champion +8 · runner-up +3 · daily 3 pm & 8 pm"), "{}", text);
        assert!(text.contains("⚔️ 1v1 win 1 · 🔤 NPAT game 1st +2 · 2nd +1 (max 6)"), "{}", text);
        assert!(!earn_text(&Rules { npat: NpatRules { channel: None, ..npat_defaults() }, ..defaults() }).contains("NPAT"));
        assert!(earn_text(&huge()).chars().count() < MESSAGE_LIMIT);
        let off = earn_text(&Rules { snitch_on: false, frogs_on: false, games_on: false, ..defaults() });
        assert!(!off.contains("Snitch") && !off.contains("Frogs") && !off.contains("Koto"), "{}", off);
        assert!(earn_text(&Rules { snitch_cap: Some(6), ..defaults() }).contains("🪽 Snitch 1–6 (max 6, Gold no limit)"));
    }

    #[test]
    fn the_welcome_lists_name_place_animal_thing_only_with_its_channel() {
        let text = welcome_text(&defaults());
        assert!(text.contains("🔤 <#1549000000000000001> · Name Place Animal Thing (games need 5 players from 2 houses)"), "{}", text);
        let off = welcome_text(&Rules { npat: NpatRules { channel: None, ..npat_defaults() }, ..defaults() });
        assert!(!off.contains("Name Place Animal Thing"), "{}", off);
        assert!(welcome_text(&huge()).chars().count() < MESSAGE_LIMIT);
    }

    #[test]
    fn the_puzzle_help_card_reads_from_the_settings_and_always_credits_the_bank() {
        let p = puzzle_defaults();
        let text = puzzle_help_text(&p);
        assert!(text.contains("🧩 Solve it"), "the button comes first: {}", text);
        assert!(text.contains("<#1549625408004165682>"), "it names the chess channel: {}", text);
        assert!(text.contains("*that's not it*") && text.contains("try again"), "a wrong move is refused gently: {}", text);
        assert!(text.contains("any move that mates is the right move"), "the fairness rule is stated: {}", text);
        assert!(text.contains("**1** for an easy one, **2** for a medium, **3** for a hard"), "{}", text);
        assert!(text.contains("stays up for another **10 minutes**"), "{}", text);
        assert!(text.contains("replaced after **60 minutes**"), "{}", text);
        assert!(text.contains("doesn't come round again for **60 days**"), "{}", text);
        assert!(text.contains("**4000 puzzles** in the bank"), "{}", text);
        assert!(text.contains("`/puzzletop`") && text.contains("`/puzzleskip`"), "{}", text);
        // The two things it must say whatever the settings are.
        assert!(text.contains("The point is to find it yourself."), "the honesty about engines: {}", text);
        assert!(
            text.contains("Puzzles from the Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0."),
            "the bank's own attribution line, word for word: {}",
            text
        );
        assert!(text.chars().count() <= DESCRIPTION_LIMIT, "it has to fit an embed: {} characters", text.chars().count());

        // Shipped as it is — the limit at nought — it promises no house points
        // and never reads as "no points".
        assert!(text.contains("Puzzles pay **no house points** at the moment"), "{}", text);
        assert!(!text.contains("takes **1 house point**"), "{}", text);
        // With the limit turned up, the first solver's point is named.
        let paying = puzzle_help_text(&PuzzleRules { cap: 10, ..p.clone() });
        assert!(paying.contains("The **first** person to crack each puzzle also takes **1 house point**, up to **10 house points** a day"), "{}", paying);
        assert!(!paying.contains("pay **no house points**"), "{}", paying);
        // A first-solver value of nought is the same as the limit being off.
        assert!(puzzle_help_text(&PuzzleRules { cap: 10, first: 0, ..p.clone() }).contains("pay **no house points**"));
        // No bank, no channel, no no-repeat rule: the lines simply go.
        let bare = puzzle_help_text(&PuzzleRules { channel: None, puzzles: None, no_repeat_days: 0, ..p });
        assert!(!bare.contains("<#") && !bare.contains("in the bank") && !bare.contains("come round again"), "{}", bare);
        assert!(bare.contains("in the chess channel"), "{}", bare);
    }

    #[test]
    fn the_chess_help_card_reads_from_the_settings() {
        let c = chess_defaults();
        let text = chess_help_text(&c);
        assert!(text.contains("⚔️ Challenge someone"), "the button comes first: {}", text);
        assert!(text.find("⚔️ Challenge someone") < text.find("`/chess @member`"), "and is named before the command: {}", text);
        assert!(text.contains("<#1549625408004165682>"), "it names the channel: {}", text);
        assert!(text.contains("**12 hours**") && text.contains("**3 minutes**"), "both time controls: {}", text);
        assert!(text.contains("**3 games**") && text.contains("**8 games**"), "both limits: {}", text);
        assert!(text.contains("Winner **+4**, or **+1** each for a draw"), "{}", text);
        assert!(text.contains("no daily limit"), "chess points are uncapped: {}", text);
        assert!(text.contains("makes no difference which house"), "houses no longer decide anything: {}", text);
        assert!(!text.to_lowercase().contains("house point"), "chess pays none: {}", text);
        assert!(text.contains("one game a day"), "{}", text);
        assert!(text.contains("first **5 moves**"), "ten half-moves is five moves: {}", text);
        assert!(text.contains("`/chesstop`"), "{}", text);
        assert!(text.contains("`Nf3`") && text.contains("`g1f3`"), "both notations: {}", text);
        assert!(text.contains("/chesshelp"), "{}", text);
        assert!(text.contains("👀 Watch"), "anyone can follow a game: {}", text);
        assert!(text.contains("kept for **30 days**"), "{}", text);
        assert!(!chess_help_text(&ChessRules { replay_days: 0, ..c.clone() }).contains("replay"), "no replays, no promise of one");
        assert!(text.chars().count() <= DESCRIPTION_LIMIT, "it has to fit an embed: {} characters", text.chars().count());

        // Turned off, or with the rules loosened, the words follow.
        let off = chess_help_text(&ChessRules { channel: None, min_plies: 0, ..c.clone() });
        assert!(!off.contains("<#"), "no channel, no link: {}", off);
        assert!(!off.contains("Give up in the first"), "the quick-resign rule is off: {}", off);
        assert!(off.contains("no daily limit"), "{}", off);
        let live_only = chess_help_text(&ChessRules { live_seconds: 45, casual_hours: 1, ..c });
        assert!(live_only.contains("**45 seconds**") && live_only.contains("**1 hour**"), "{}", live_only);
    }

    #[test]
    fn the_guide_and_the_earn_lines_mention_chess_only_when_it_is_on() {
        let on = guide(&defaults(), true);
        let games = on.iter().find(|p| p.title.contains("Play the games")).expect("the games panel");
        assert!(games.body.contains("♟️ **Chess**"), "{}", games.body);
        assert!(games.body.contains("⚔️ Challenge someone"), "the guide names the button too: {}", games.body);
        assert!(games.body.contains("**chess points**, not house points"), "{}", games.body);
        let off = Rules { chess: ChessRules { channel: None, ..chess_defaults() }, ..defaults() };
        let quiet = guide(&off, true);
        let games = quiet.iter().find(|p| p.title.contains("Play the games")).expect("the games panel");
        assert!(!games.body.contains("Chess"), "left out while off or without a channel: {}", games.body);
        // Chess is off the house-points LIST altogether now: what is left is one
        // line apart from it, saying what it does pay instead.
        let earn = earn_text(&defaults());
        assert!(!earn.contains("♟️ Chess win +4 · draw"), "not a way to earn house points: {}", earn);
        assert!(earn.contains("-# ♟️ Chess pays **chess points** (win +4, draw +1 each, no limit), not house points · `/chesstop`"), "{}", earn);
        assert!(!earn_text(&off).contains("Chess"), "and nothing at all while it is off");
        assert!(welcome_text(&defaults()).contains("♟️ <#1549625408004165682>"));
        assert!(!welcome_text(&off).contains("♟️"));
        // Worth nothing is the same as switched off, as far as the guide goes.
        let free = Rules { chess: ChessRules { win: 0, draw: 0, ..chess_defaults() }, ..defaults() };
        let quiet = guide(&free, true);
        let games = quiet.iter().find(|p| p.title.contains("Play the games")).expect("the games panel");
        assert!(!games.body.contains("Chess"), "{}", games.body);
    }

    #[test]
    fn the_npat_rules_post_reads_from_the_settings() {
        let n = npat_defaults();
        let text = npat_rules_text(&n);
        for part in [
            "**✋ Joining a game**\n• Press **✋ I'm in** on the game card at the bottom (press again to leave). That opens the lobby for **3 minutes**.",
            "• A game starts once **5 players** are in from at least **2 different houses**. You still get **30 seconds** after that to jump in.",
            "🧙 Muggles can play too: they count as players but not as a house, and never win house points.",
            "• A game is **5 letters**. For each one you get **45 seconds**. Q, X, Y and Z never come up.",
            "fill in a **Name, Place, Animal and Thing** starting with that letter",
            "No fictional characters (Pikachu, Harry Potter), no brands.",
            "Nothing mythical or extinct (Pegasus, Pterodactyl), no plants.",
            "No brand names (Pepsi, Parle-G), no feelings or ideas.",
            "Hindi and Hinglish count (Sher = Lion). Small typos are fine. One answer per box.",
            "✅ unique **10** · 🟰 shared **5** · ❌ wrong or blank **0**",
            "Scores add up over the 5 letters.",
            "• 🥇 The game's best house member gets **+2**, 🥈 the next **+1**.",
            "• Only when at least **3 people** played the game. Up to **6 house points** a day each.",
            "within 30 minutes. A mod decides with 🛡️ Review",
            "The next letter comes **20 seconds** after each letter's results",
        ] {
            assert!(text.contains(part), "missing {:?} in\n{}", part, text);
        }
        assert!(!text.contains("<#"), "the post sits in the game's own channel");
        let text = npat_rules_text(&NpatRules { cap: None, min_houses: 1, join_secs: 90, letters: "abcxyz".into(), min_scored: 1, ..n.clone() });
        assert!(text.contains("No daily limit."), "{text}");
        assert!(text.contains("• A game starts once **5 players** are in. You still"), "one house needed: no house words: {text}");
        assert!(text.contains("That opens the lobby for **90 seconds**."), "{text}");
        assert!(text.contains("Only A, B, C, X, Y and Z come up."), "most letters skipped: say which are used: {text}");
        assert!(text.contains("at least **1 person** played"), "{text}");
        assert!(npat_rules_text(&NpatRules { letters: "ABCDEFGHIJKLMNOPQRSTUVWXYZ".into(), ..n.clone() }).contains("Any letter from A to Z can come up."));
        assert!(npat_rules_text(&NpatRules { letters: "ABCDEFGHIJKLMNOPQRSTUVWXY".into(), ..n.clone() }).contains("Z never comes up."));
        assert_eq!(skipped_letters("ABCDEFGHIJKLMNOPRSTUVW"), vec!['Q', 'X', 'Y', 'Z']);
        assert_eq!(duration_words(60), "1 minute");
        assert_eq!(duration_words(1799), "1799 seconds");
    }

    #[test]
    fn the_npat_rules_post_fits_discord() {
        for n in [npat_defaults(), huge().npat] {
            let body = npat_rules_text(&n);
            assert!(body.chars().count() <= DESCRIPTION_LIMIT, "{} chars", body.chars().count());
            assert!(NPAT_RULES_TITLE.chars().count() <= 256);
            if let Some(message) = npat_rules_message(&n) {
                assert!(message.chars().count() <= MESSAGE_LIMIT);
                assert!(message.starts_with("## 📜 How Name · Place · Animal · Thing works\n\n**✋ Joining a game**"));
            }
        }
    }

    #[test]
    fn digests_change_with_content() {
        assert_eq!(digest(&["a", "b"]), digest(&["a", "b"]));
        assert_ne!(digest(&["a", "b"]), digest(&["ab"]));
        assert_eq!(digest(&["x"]).len(), 64);
    }
}
