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
        let mut commands = vec!["📖 `/frogs` your collection", "🔎 `/frogcard` look up any card by number"];
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
        let none = Rules { house_channel: None, quiz_channel: None, fight_channel: None, frogs_on: false, snitch_on: false, npat: NpatRules { channel: None, ..npat_defaults() }, ..defaults() };
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
            "📖 `/frogs` your collection · 🔎 `/frogcard` look up any card by number · 🔁 `/trades` your offers",
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
