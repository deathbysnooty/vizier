//! Telling everyone what changed, without anyone having to write a post.
//!
//! Two things are announced, both to the channels in `VIZIER_ANNOUNCE_CHANNELS`
//! (the four house common rooms and the houses channel):
//!
//! 1. **Settings changes.** Every minute the live settings are read into the
//!    same [`Rules`] snapshot the House Cup posts are written from, and the
//!    parts a player would care about - points, limits, frequencies, on/off -
//!    are compared with the ones last announced ([`FIELDS`]). A run of edits on
//!    the panel becomes ONE message: nothing goes out until the settings have
//!    sat still for `VIZIER_ANNOUNCE_DELAY_MINUTES`, and never more often than
//!    `VIZIER_ANNOUNCE_MIN_GAP_MINUTES`.
//! 2. **Release notes.** [`NEWS`] is a hand-written list; an entry whose id
//!    isn't in `announce_news_done` yet is posted, oldest first, one a poll, and
//!    then recorded. Adding a new game means adding one entry here.
//!
//! The words in the scoreboard channel are not this module's job: `scoreboard`
//! already edits the guide, the welcome and the rules posts in place when the
//! settings change, from the very same [`Rules`]. This adds the "here's what
//! changed" nudge in the rooms people actually read.
//!
//! What is left out on purpose: channels, hours of the day, timers, and
//! anything only the panel cares about. Changing where the Snitch drops is not
//! news; changing what it pays is.
//!
//! Nothing is ever pinged, and every post is written down (announcement hash +
//! channel) so a restart in the middle can't say the same thing twice.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serenity::all::{
    ChannelId, CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions,
    CreateCommand, CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse,
    CreateInteractionResponseMessage, CreateMessage, EditInteractionResponse,
};

use super::control;
use super::house;
use super::rules_text::{self, Rules};

/// How often the settings are looked at, in step with the scoreboard's own poll.
const POLL_SECS: u64 = 60;
/// Quiet needed after the last edit before a batch goes out, in minutes.
const DELAY_DEFAULT: u64 = 10;
/// The least time between two automatic announcements, in minutes.
const GAP_DEFAULT: u64 = 30;
/// House Cup gold.
const GOLD: u32 = 0xE8B923;
/// Discord's embed title limit.
const TITLE_LIMIT: usize = 256;
/// Kept clear of Discord's 4096 for an embed description.
const BODY_LIMIT: usize = 3900;
/// How long a hand-written release note may be.
pub const NEWS_LIMIT: usize = 1200;
/// Extra tries a channel that failed gets, on later polls.
const MAX_RETRIES: u32 = 2;
/// How many "posted this there already" marks are kept.
const KEEP_MARKS: usize = 400;

/// The settings as they were when they were last announced.
const ANNOUNCED: &str = "announce_rules";
/// The settings as the last poll saw them, and when they last moved: the batch.
const SEEN: &str = "announce_rules_seen";
const SEEN_TS: &str = "announce_rules_seen_ts";
/// When the last automatic announcement went out.
const LAST_TS: &str = "announce_last_ts";
/// The ids of the release notes already told.
const NEWS_DONE: &str = "announce_news_done";
/// "<hash>:<channel>" for every post that landed.
const MARKS: &str = "announce_posted";
/// The one announcement still owed to a channel that failed.
const RETRY: &str = "announce_retry";

// --- what counts as news ---------------------------------------------------------------

/// The values behind one line, in the order its words read them.
type Read = fn(&Rules) -> Vec<String>;
/// The line to post, from the values as they were and as they are.
type Say = fn(&[String], &[String]) -> String;

/// One thing worth telling everyone about.
///
/// Adding a game's points is one entry - say, when the sudoku settings reach
/// [`Rules`]:
///
/// ```ignore
/// Field {
///     key: "sudoku",
///     read: |r| vec![num(r.sudoku), cap(r.sudoku_cap)],
///     say: |o, n| {
///         format!("🔢 **Sudoku** now pays {} for a solve{}", shift(num_at(n, 0), num_at(o, 0)), cap_clause(cap_at(n, 1), cap_at(o, 1)))
///     },
///     under: Some("sudoku_on"),
/// }
/// ```
///
/// A key the stored snapshot doesn't have yet (a setting that didn't exist when
/// it was written) is taken on quietly, so a new field never announces itself.
struct Field {
    /// Stable: it is the key in the stored snapshot. Renaming one costs a
    /// silent adoption, never a wrong announcement.
    key: &'static str,
    read: Read,
    say: Say,
    /// A switch whose own line already covers this one: when that switch moved
    /// in the same batch, this line is left out. Switching the frogs off should
    /// say so once, not five times.
    under: Option<&'static str>,
}

/// Every line, in the order they are posted in.
const FIELDS: &[Field] = &[
    Field {
        key: "activity",
        read: |r| vec![yes(r.activity_on)],
        say: |_, n| switch("💬 **Chat and voice points**", "are", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "chat",
        read: |r| vec![r.chat_tiers.iter().map(i64::to_string).collect::<Vec<_>>().join(","), cap(r.chat_cap)],
        say: |o, n| {
            // A cap of nought is not "up to 0 a day": chatting simply stops
            // paying, and the note has to read like a decision, not a number.
            if cap_at(n, 1) == Some(0) {
                return "💬 **Chat** no longer pays points — messages are still counted, they just don't earn.".to_string();
            }
            format!(
                "💬 **Chat** now pays 1 point at {}{}",
                shift_in(format!("{} messages in a day", tiers(n)), tiers(n), tiers(o)),
                cap_clause(cap_at(n, 1), cap_at(o, 1))
            )
        },
        under: Some("activity"),
    },
    Field {
        key: "voice",
        read: |r| vec![num(r.voice_minutes), cap(r.voice_cap)],
        say: |o, n| {
            let (new, old) = (format!("{} min", num_at(n, 0)), format!("{} min", num_at(o, 0)));
            format!(
                "🎙️ **Voice** now pays {}{}",
                shift_in(format!("1 point per {}", new), &new, &old),
                cap_clause(cap_at(n, 1), cap_at(o, 1))
            )
        },
        under: Some("activity"),
    },
    Field {
        key: "quiz",
        read: |r| vec![num(r.quiz[0]), num(r.quiz[1]), num(r.quiz[2]), cap(r.quiz_cap)],
        say: |o, n| {
            format!(
                "🧠 **Quiz** now pays {} to a round's top three{}",
                shift(row(n, 0, 3), row(o, 0, 3)),
                cap_clause(cap_at(n, 3), cap_at(o, 3))
            )
        },
        under: None,
    },
    Field {
        key: "games",
        read: |r| vec![yes(r.games_on)],
        say: |_, n| switch("🎮 **Word game points** (Koto, Anagram, Cat Bot)", "are", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "koto",
        read: |r| vec![num(r.koto_win), num(r.koto_played), cap(r.koto_cap)],
        say: |o, n| {
            format!(
                "🔤 **Koto** now pays {} for a solve and {} for each guess that scores{}",
                shift(num_at(n, 0), num_at(o, 0)),
                shift(num_at(n, 1), num_at(o, 1)),
                cap_clause(cap_at(n, 2), cap_at(o, 2))
            )
        },
        under: Some("games"),
    },
    Field {
        key: "anagram",
        read: |r| vec![num(r.anagram), cap(r.anagram_cap)],
        say: |o, n| {
            format!(
                "🔡 **Anagram** now pays {} for a solve{}",
                shift(num_at(n, 0), num_at(o, 0)),
                cap_clause(cap_at(n, 1), cap_at(o, 1))
            )
        },
        under: Some("games"),
    },
    Field {
        key: "cat",
        read: |r| vec![num(r.cat[0]), num(r.cat[1]), num(r.cat[2]), cap(r.cat_cap)],
        say: |o, n| {
            format!(
                "🐱 **Cat Bot** now pays {} for a common · rare · top cat{}",
                shift(row(n, 0, 3), row(o, 0, 3)),
                cap_clause(cap_at(n, 3), cap_at(o, 3))
            )
        },
        under: Some("games"),
    },
    Field {
        key: "wordle_on",
        read: |r| vec![yes(r.wordle_on)],
        say: |_, n| switch("🟩 **Wordle points**", "are", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "wordle",
        read: |r| r.wordle.iter().map(|p| num(*p)).collect(),
        say: |o, n| {
            format!(
                "🟩 **Wordle** now pays {} for a solve in 1–2 · 3 · 4 · 5–6 guesses, and {} for the day's best",
                shift(row(n, 0, 4), row(o, 0, 4)),
                shift(format!("+{}", num_at(n, 4)), format!("+{}", num_at(o, 4)))
            )
        },
        under: Some("wordle_on"),
    },
    Field {
        key: "arena",
        read: |r| vec![num(r.arena_win), cap(r.arena_cap)],
        say: |o, n| {
            format!(
                "⚔️ **1v1 fights** now pay {} for a win{}",
                shift(num_at(n, 0), num_at(o, 0)),
                cap_clause(cap_at(n, 1), cap_at(o, 1))
            )
        },
        under: None,
    },
    Field {
        key: "royale",
        read: |r| vec![num(r.royale[0]), num(r.royale[1])],
        say: |o, n| {
            format!(
                "👑 **Battle Royale** now pays {} to the champion and {} to the runner-up",
                shift(num_at(n, 0), num_at(o, 0)),
                shift(num_at(n, 1), num_at(o, 1))
            )
        },
        under: None,
    },
    Field {
        key: "battle_daily",
        read: |r| vec![yes(r.battle_daily)],
        say: |_, n| switch("⚔️ **The daily Battle Royale**", "is", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "snitch_on",
        read: |r| vec![yes(r.snitch_on)],
        say: |_, n| switch("🪽 **The Golden Snitch**", "is", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "snitch_drops",
        read: |r| vec![num(r.snitch_drops.0), num(r.snitch_drops.1)],
        say: |o, n| {
            format!(
                "🪽 **The Golden Snitch** now appears {}",
                shift(times(num_at(n, 0), num_at(n, 1)), times(num_at(o, 0), num_at(o, 1)))
            )
        },
        under: Some("snitch_on"),
    },
    Field {
        key: "snitch_points",
        read: |r| {
            let mut v: Vec<String> = r.snitch_points.iter().flatten().map(|p| num(*p)).collect();
            v.push(cap(r.snitch_cap));
            v
        },
        say: |o, n| {
            format!(
                "🪽 **Catching the Snitch** now pays bronze {} · silver {} · golden {} for 1st · 2nd · 3rd{}",
                shift(row(n, 0, 3), row(o, 0, 3)),
                shift(row(n, 3, 3), row(o, 3, 3)),
                shift(row(n, 6, 3), row(o, 6, 3)),
                cap_clause(cap_at(n, 9), cap_at(o, 9))
            )
        },
        under: Some("snitch_on"),
    },
    Field {
        key: "frogs",
        read: |r| vec![yes(r.frogs_on)],
        say: |_, n| switch("🐸 **Chocolate Frogs**", "are", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "frog_drops",
        read: |r| vec![num(r.frog_drops_max)],
        say: |o, n| {
            format!(
                "🐸 **Chocolate Frogs** now drop up to {}",
                shift(format!("{} a day", num_at(n, 0)), format!("{} a day", num_at(o, 0)))
            )
        },
        under: Some("frogs"),
    },
    Field {
        key: "frog_points",
        read: |r| vec![num(r.frog_points[0]), num(r.frog_points[1]), num(r.frog_points[2]), cap(r.frog_cap)],
        say: |o, n| {
            format!(
                "🐸 **Frog cards** now pay {} for a common · uncommon · legendary card{}",
                shift(row(n, 0, 3), row(o, 0, 3)),
                cap_clause(cap_at(n, 3), cap_at(o, 3))
            )
        },
        under: Some("frogs"),
    },
    Field {
        key: "frog_set",
        read: |r| vec![num(r.set_bonus)],
        say: |o, n| match num_at(n, 0) {
            0 => format!("🏆 **A full card set** can't be sold for points any more (it paid **{}**)", num_at(o, 0)),
            _ => format!("🏆 **A full card set** now sells for {}", shift(num_at(n, 0), num_at(o, 0))),
        },
        under: Some("frogs"),
    },
    Field {
        key: "trades",
        read: |r| vec![yes(r.trades_on)],
        say: |_, n| switch("🔄 **Card trading**", "is", on_at(n, 0)),
        under: Some("frogs"),
    },
    Field {
        key: "daily_top_card",
        read: |r| vec![yes(r.daily_top_cards)],
        say: |_, n| switch("🃏 **A card for the day's top scorer**", "is", on_at(n, 0)),
        under: Some("frogs"),
    },
    Field {
        key: "royale_cards",
        read: |r| vec![yes(r.royale_cards)],
        say: |_, n| switch("🃏 **Cards for the Battle Royale winners**", "are", on_at(n, 0)),
        under: Some("frogs"),
    },
    Field {
        key: "npat_on",
        read: |r| vec![yes(r.npat.channel.is_some())],
        say: |_, n| switch("🔤 **Name Place Animal Thing**", "is", on_at(n, 0)),
        under: None,
    },
    Field {
        key: "npat_players",
        read: |r| vec![num(r.npat.min_players), num(r.npat.min_houses), num(r.npat.min_scored)],
        say: |o, n| {
            format!(
                "🔤 **Name Place Animal Thing** games now need {} from {}, and {} of them must score for the game to pay",
                shift(plural(num_at(n, 0), "player", "players"), plural(num_at(o, 0), "player", "players")),
                shift(plural(num_at(n, 1), "house", "houses"), plural(num_at(o, 1), "house", "houses")),
                shift(num_at(n, 2), num_at(o, 2))
            )
        },
        under: Some("npat_on"),
    },
    Field {
        key: "npat_letters",
        read: |r| vec![num(r.npat.letters_per_game), num(r.npat.letters.chars().count() as i64)],
        say: |o, n| {
            format!(
                "🔤 **Name Place Animal Thing** games are now {} long, drawn from a pool of {}",
                shift(plural(num_at(n, 0), "letter", "letters"), plural(num_at(o, 0), "letter", "letters")),
                shift(plural(num_at(n, 1), "letter", "letters"), plural(num_at(o, 1), "letter", "letters"))
            )
        },
        under: Some("npat_on"),
    },
    Field {
        key: "npat_scores",
        read: |r| vec![num(r.npat.scores[0]), num(r.npat.scores[1])],
        say: |o, n| {
            format!(
                "🔤 **Name Place Animal Thing** answers now score {} when nobody else has them and {} when they're shared",
                shift(num_at(n, 0), num_at(o, 0)),
                shift(num_at(n, 1), num_at(o, 1))
            )
        },
        under: Some("npat_on"),
    },
    Field {
        key: "npat_prizes",
        read: |r| vec![num(r.npat.prizes[0]), num(r.npat.prizes[1]), cap(r.npat.cap)],
        say: |o, n| {
            format!(
                "🔤 **Name Place Animal Thing** now gives a game's best two 🥇 {} · 🥈 {}{}",
                shift(format!("+{}", num_at(n, 0)), format!("+{}", num_at(o, 0))),
                shift(format!("+{}", num_at(n, 1)), format!("+{}", num_at(o, 1))),
                cap_clause(cap_at(n, 2), cap_at(o, 2))
            )
        },
        under: Some("npat_on"),
    },
    Field {
        key: "sudoku",
        read: |r| vec![num(r.sudoku.points[0]), num(r.sudoku.points[1]), num(r.sudoku.points[2])],
        say: |o, n| {
            // There is no cap clause here on purpose. Sudoku pays SUDOKU points,
            // its own uncapped score, and no house points at all, so the note has
            // to name the currency — and say, every time, that what was earned
            // back when it did pay is still there.
            if row(n, 0, 3) == "0 · 0 · 0" {
                return "🔢 **Sudoku** no longer scores at all — the puzzles stay, they just stop counting. The sudoku points already earned stay where they are."
                    .to_string();
            }
            format!(
                "🔢 **Sudoku** now pays {} sudoku points for an easy · medium · hard puzzle — sudoku points, not house points, so nothing here moves the House Cup. What anyone has already earned stays exactly where it is. `/sudokutop` is the board.",
                shift(row(n, 0, 3), row(o, 0, 3))
            )
        },
        under: None,
    },
];

// --- writing the values down ------------------------------------------------------------

fn num(n: i64) -> String {
    n.to_string()
}

/// A limit, or "-" for no limit.
fn cap(c: Option<i64>) -> String {
    c.map(|n| n.to_string()).unwrap_or_else(|| "-".into())
}

fn yes(on: bool) -> String {
    if on { "on".into() } else { "off".into() }
}

/// The number at `i`, 0 when it isn't there.
fn num_at(v: &[String], i: usize) -> i64 {
    v.get(i).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// The limit at `i`: anything unreadable, "-" included, means no limit.
fn cap_at(v: &[String], i: usize) -> Option<i64> {
    v.get(i).and_then(|s| s.parse().ok())
}

fn on_at(v: &[String], i: usize) -> bool {
    v.get(i).is_some_and(|s| s == "on")
}

// --- putting the words together ----------------------------------------------------------

/// "**30 min** (was 60 min)" - the value now, and the old one when it moved.
fn shift(new: impl fmt::Display, old: impl fmt::Display) -> String {
    let (new, old) = (new.to_string(), old.to_string());
    shift_in(&new, &new, &old)
}

/// The same for a phrase where only one part moved: the phrase in bold, and
/// just that part repeated after it - "**1 point per 30 min** (was 60 min)".
fn shift_in(phrase: impl fmt::Display, new: impl fmt::Display, old: impl fmt::Display) -> String {
    let (new, old) = (new.to_string(), old.to_string());
    if new == old { format!("**{}**", phrase) } else { format!("**{}** (was {})", phrase, old) }
}

/// "· up to **6 a day**", saying it when the limit itself moved.
fn cap_clause(new: Option<i64>, old: Option<i64>) -> String {
    match (new, old) {
        (Some(n), Some(o)) => format!(" · up to {}", shift(format!("{} a day", n), format!("{} a day", o))),
        (Some(n), None) => format!(" · up to **{} a day** (there was no limit)", n),
        (None, Some(o)) => format!(" · with **no daily limit** any more (it was {} a day)", o),
        (None, None) => " · no daily limit".into(),
    }
}

/// "2 · 1 · 1": `len` numbers from `at`.
fn row(v: &[String], at: usize, len: usize) -> String {
    (at..at + len).map(|i| num_at(v, i).to_string()).collect::<Vec<_>>().join(" · ")
}

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "6–8 times a day", or "3 times a day" when the two are the same.
fn times(low: i64, high: i64) -> String {
    if low == high { format!("{} a day", plural(high, "time", "times")) } else { format!("{}–{} times a day", low, high) }
}

/// "20, 60 and 150" from the stored comma list.
fn tiers(v: &[String]) -> String {
    let items: Vec<&str> = v.first().map(|s| s.split(',').filter(|t| !t.is_empty()).collect()).unwrap_or_default();
    match items.as_slice() {
        [] => "no".into(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} and {}", rest.join(", "), last),
    }
}

/// "🐸 **Chocolate Frogs** are now **ON**".
fn switch(what: &str, is: &str, on: bool) -> String {
    format!("{} {} now **{}**", what, is, if on { "ON" } else { "OFF" })
}

// --- the difference between two days ------------------------------------------------------

/// Every value the announcements watch, by field key.
pub type Snapshot = BTreeMap<String, Vec<String>>;

/// The settings as they are now, ready to be compared or stored.
pub fn snapshot(r: &Rules) -> Snapshot {
    FIELDS.iter().map(|f| (f.key.to_string(), (f.read)(r))).collect()
}

/// Whether one field really moved. A field the old snapshot never had, or one
/// whose shape has changed since, counts as unmoved: it is taken on quietly
/// rather than announced as something it isn't.
fn moved(old: &Snapshot, new: &Snapshot, key: &str) -> bool {
    match (old.get(key), new.get(key)) {
        (Some(o), Some(n)) => o.len() == n.len() && o != n,
        _ => false,
    }
}

/// What to tell everyone, one line per thing that changed, in [`FIELDS`] order.
/// Empty when nothing a player would notice is different.
pub fn changes(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let moved: HashSet<&str> = FIELDS.iter().map(|f| f.key).filter(|k| moved(old, new, k)).collect();
    FIELDS
        .iter()
        .filter(|f| moved.contains(f.key))
        // A switch that moved says it all; its own settings' lines would only repeat it.
        .filter(|f| f.under.is_none_or(|switch| !moved.contains(switch)))
        .filter_map(|f| Some((f.say)(old.get(f.key)?, new.get(f.key)?)))
        .collect()
}

// --- release notes --------------------------------------------------------------------------

/// One hand-written announcement about something new. Add an entry and the bot
/// posts it, once, everywhere - `id` is what it remembers it by, so never
/// change one after it has gone out. Keep `body` under [`NEWS_LIMIT`]
/// characters; Discord markdown is fine.
///
/// ```ignore
/// News { id: "sudoku-2026-09", title: "🔢 Sudoku is here", body: "A grid a day in <#…>; solve it for points." }
/// ```
pub struct News {
    pub id: &'static str,
    pub title: &'static str,
    pub body: &'static str,
}

pub const NEWS: &[News] = &[News {
    id: "geo-2026-09",
    title: "\u{1F5FA}\u{FE0F} Geo is here \u{2014} GeoGuessr, but only India",
    body: "A photo of a street somewhere in India goes up in {geo}, and you say where it is.\n\n           \u{2022} Real dashcam photos \u{2014} no landmark picked out for you. A road, the shops on it, the signboards. Read them.\n           \u{2022} Type a place. The **state** is worth **2**, a town **within 60 km** is **4**, a town **within 15 km** is **5**.\n           \u{2022} You never have to know the town: the state alone always scores, and a town hands you its state free. `Kerala` is safe, `Kochi` is greedy.\n           \u{2022} The state is a **gate** \u{2014} a town in the WRONG state scores nothing, however near it looks.\n           \u{2022} Old names and bad spelling both work: `Bombay`, `Benares`, `Gurgaon`.\n           \u{2022} **Matches of 5 places.** Most points wins: **5 house points**, **2** for second, up to **15 a day**. One photo alone pays none. Press **I'm ready** to play.\n           \u{2022} **`!hint`** gives the state's first letter for a point off; **`!skip`** works after a hint.\n\n           It covers **22 states** so far \u{2014} wherever people have driven with a camera. Every state in it comes up as often as every other, so guessing Tamil Nadu every time will not work.\n\n           `/geohelp` explains the lot, `/geotop` is the board.",
}, News {
    id: "koto-scoring-guesses-2026-09",
    title: "🔤 Koto now pays for every guess that scores",
    body: "Koto points have changed, and they're kinder.\n\n\
           • Each guess that **scores** earns its points - a guess worth **+0** earns nothing, so blind guessing \
           doesn't pay any more.\n\
           • A game nobody solves still pays out: your scoring guesses count even when the board runs out of guesses.\n\
           • Solving the word still pays the solve on top.\n\n\
           Nothing you have to do differently - play as you were, and the points follow the guesses that actually helped.",
}, News {
    id: "sudoku-2026-09",
    title: "🔢 Sudoku is here",
    body: "A sudoku is now always waiting in {sudoku}, and solving it first scores for your house.\n\n• Press **▶️ Play** on the puzzle card and the bot gives you a private link to fill the grid in - tap a \
           square, tap a number.\n• When it's full, press **Copy code** on the page, come back and press **📋 Submit code**.\n• The first correct code wins: 🟢 Easy **2** · 🟡 Medium **4** · 🔴 Hard **6** points. A new puzzle appears \
           the moment one is solved.\n• Stuck? **💡 Hint** fills one square and costs a point off that puzzle. Beaten to it? Your code still \
           works for a day, so you can see whether you had it right.\n\n`/sudokuhelp` explains the lot, `/sudoku` finds your puzzles again.",
}, News {
    id: "chess-2026-09",
    title: "♟️ Chess is here",
    body: "Challenge anyone to a game of chess in {chess}.\n\n• `/chess @someone` sends a challenge; they press **Accept**. Pick **casual** (hours per move) or \
           **live** (play it out there and then).\n• Press **♟️ Open board** for your own board - tap a piece, tap where it goes - or **✍️ Type move** \
           if you prefer `e4`, `Nf3`, `O-O`.\n• The card in the channel shows the board after every move, so everyone can follow along, and **👀 Watch** \
           opens a live board for anyone. Finished games can be replayed.\n• Winning scores **chess points** — the game's own score, which has no daily limit and is nothing to do \
           with the House Cup. Whichever houses you are both in.\n\n`/chesshelp` explains everything, `/chess` on its own lists your games, `/chesstop` is the board.",
}, News {
    id: "anagrams-2026-09",
    title: "🔀 Anagrams is here",
    body: "A scrambled word is now always waiting in {anagram} — and this one you play by **typing**.\n\n• The letters go up spaced out: **T S A B E**. Work out the word and just type it in the channel. No \
           buttons, no commands.\n• **Any** word that uses all the letters counts, not only the one I scrambled — those letters are taken \
           by `beast`, `bates` and `tabes` alike.\n• First right answer gets a ✅ and the house points: **1** for 4–5 letters, **2** for 6–7, **3** for 8 or \
           more (up to 10 a day). The next word goes up straight away.\n• That same number is your **anagram points** as well, and those have no daily limit — they keep counting \
           after your house points stop for the day, everyone scores them, and the most of them takes the day's 🐸 frog card for anagrams.\n• Wrong guesses are ignored, so guess as much as you like. Stuck? **`!hint`** gives away the first \
           letter (a point off the round), and **`!skip`** moves on once a hint is out.\n\n`/anagramhelp` explains the lot, `/anagram` shows the round that's up, `/anagramtop` is the anagram points board.",
}, News {
    id: "guess-the-word-2026-09",
    title: "🎨 Guess the Word is here",
    body: "There's a doodle waiting in {guess}, and all you have to do is say what it is.\n\n• Somebody really drew it — every picture comes from Google's Quick, Draw! dataset, so they're quick, wobbly and \
           human.\n• Just type your guess in the channel. Capitals, spaces and hyphens don't matter (`Ice-Cream`, `ice cream` \
           and `icecream` are all the same answer), and a spelling slip is forgiven on longer words — `gitar` wins a \
           guitar.\n• First right guess gets a ✅ and **2** house points (up to 10 a day), and the next doodle goes up \
           straight away.\n• Those 2 are your **guess points** as well, and those have no daily limit — they keep counting after your \
           house points stop for the day, everyone scores them, and the most of them takes the day's 🐸 frog card for this game.\n• Stuck? **`!hint`** puts a **second drawing of the same thing** up and gives away the first letter (a point \
           off the round), and **`!skip`** moves on once a hint is out.\n\n`/guesshelp` explains the lot, `/guess` shows the doodle that's up, `/guesstop` is the guess points board.",
}, News {
    id: "chat-points-off-2026-09",
    title: "💬 Chatting no longer earns points",
    body: "From now on, **text messages do not earn house points**. People were spamming to hit the message count \u{2014}            \"6 msgs, 5 to go\" \u{2014} and that is not what this server is for.\n\n\u{2022} Everything else pays exactly as before: the games, voice, the 🪄 Snitch, 🐸 frogs, the arena, all of it.\n\u{2022} Your messages are **still counted** \u{2014} the panel, the most-active lists and the day's 🐸 frog card for chat all work the same.\n\u{2022} Spam is now taken down automatically, and the mods can see what was removed.\n\nTalk because you want to talk. Play the games for points.",
}, News {
    id: "letter-duel-2026-09",
    title: "🔠 Letter Duel is here",
    body: "A board, a bag of tiles and up to four of you — Letter Duel is now in {duel}.\n\n• `/duel`, or **⚔️ Start a duel** on the card, opens a lobby. Anyone can press **⚔️ Join**; it starts \
           early when the seats fill, and is called off with nothing lost if too few turn up.\n• Press **🔤 Open my rack** for a board of your own. Tap a tile, tap a square. Nobody else sees your \
           tiles, and the page tells you what a play scores **before** you commit it.\n• Proper rules: first word over the middle ★, every word made — sideways ones too — has to be real, \
           premium squares double and triple, all seven tiles is **+50**, blanks score nothing. Swap or pass if \
           you're stuck. **Four minutes** a turn; three missed turns and you're out.\n• **Duel points** for everyone who plays, with no daily limit — the winner most, then the runner-up, then \
           anyone who sees it through. `/dueltop` is that board.\n\n`/duelhelp` explains the lot.",
}, News {
    id: "sudoku-solver-points-back-2026-09",
    title: "⚖️ Sudoku: some points have been taken back",
    body: "A few sudoku puzzles have been solved in times that are not humanly possible. A medium puzzle has about fifty            empty squares, and some were handed in **14 to 19 seconds** after the page was first opened \u{2014} faster than the            digits can be typed, never mind worked out. A solver app does that. A person doesn\u{2019}t.\n\n\u{2022} **66 points have been taken back**, from the members and from their house.\n\u{2022} This is checked, not guessed: the bot records when each player opened a puzzle and when their code arrived.\n\u{2022} Nobody is named here, one of them owned up when asked, and solves that merely look quick have been left            alone \u{2014} being good at sudoku is not cheating.\n\nUse a solver for fun if you like. Just don\u{2019}t hand the code in for points.",
}, News {
    id: "sudoku-sudoku-points-2026-09",
    title: "🧩 Sudoku keeps its own score now",
    body: "Sudoku in {sudoku} has stopped paying house points. The puzzles, the difficulties, the hints, the codes and \
           the late window are all exactly as they were — what changes is what a solve is worth: **sudoku points**, \
           the game's own score, which doesn't move the House Cup.\n\n• A solve scores the puzzle's value with your own hints taken off, and **nothing is capped** — a good day \
           keeps counting all the way.\n• **Everyone** has sudoku points, houses or no houses. Mods and Muggles score too.\n• **The house points already earned stay exactly where they are.** Nothing is taken back by this.\n• `/sudokutop` is the new board, today or this month; `/sudoku` shows where you stand; the day's top \
           scorer still gets the 🐸 frog card.\n\nWhy: a grid handed to a solver app was paying a house the same as a grid worked out by a person. Sudoku is \
           still here to play — it just doesn't decide the House Cup any more.",
}, News {
    id: "guess-the-movie-matches-2026-09",
    title: "🎬 Guess the Movie — now played in matches of 10",
    body: "There is always a film waiting in {movie} \u{2014} and it is now played in **matches**.\n\n\u{2022} A match is **10 films**. Whoever names the most of them **wins the match**.\n\u{2022} **5 house points** to the winner, **2** to the runner-up, up to **15 a day**. A single film pays no house points on its own any more \u{2014} the match is the thing worth playing for.\n\u{2022} Joint winners both take the 5, and no runner-up is paid.\n\u{2022} Between matches there is a short break. Press **I'm ready** on the card and it starts once **2** of you are. Turning up late is fine \u{2014} naming a film IS joining.\n\u{2022} **Movie points** are unchanged: every film you name scores them, no daily limit, and mods and the unsorted score them too. `/movietop` is that board.\n\u{2022} Playing is the same \u{2014} the bot asks ONE way each round and you just type the title. Spelling is forgiven, a **sequel number** never is. **`!hint`**, then **`!skip`**.\n\n`/moviehelp` explains the lot.",
}, News {
    id: "letter-duel-correction-2026-09",
    title: "🔠 Letter Duel \u{2014} two corrections",
    body: "The note about Letter Duel had two things wrong in it.\n\n\u{2022} A turn is **four minutes**, not ninety seconds. The very first game showed ninety seconds isn\u{2019}t long            enough to open your rack, read seven tiles and find a word \u{2014} nobody managed a single move.\n\u{2022} It pays **duel points only** for now, not house points. Win, come second or just see the game through            and you score duel points, with no daily limit; `/dueltop` is the board. House points may follow once the            game has been played in properly.\n\nEverything else in that note stands: the lobby, your own tile page, the real rules, and **+50** for using all seven tiles.",
}];

/// Puts the live channel mentions into a note: `{sudoku}`, `{chess}`,
/// `{anagram}`, `{guess}`, `{npat}`, `{duel}`, `{movie}`, `{geo}`
/// and `{scoreboard}` become `<#id>`, or a plain name when that channel isn't
/// set, so a note never shows a broken link.
pub fn fill_channels(body: &str) -> String {
    let mention = |key: &str, name: &str| match super::control::id(key).filter(|id| *id != 0) {
        Some(id) => format!("<#{}>", id),
        None => format!("**{}**", name),
    };
    body.replace("{sudoku}", &mention("VIZIER_SUDOKU_CHANNEL", "the sudoku channel"))
        .replace("{chess}", &mention("VIZIER_CHESS_CHANNEL", "the chess channel"))
        .replace("{anagram}", &mention("VIZIER_ANAGRAM_CHANNEL", "the anagrams channel"))
        .replace("{guess}", &mention("VIZIER_GUESS_CHANNEL", "the guess-the-word channel"))
        .replace("{npat}", &mention("VIZIER_NPAT_CHANNEL", "the word-game channel"))
        .replace("{duel}", &mention("VIZIER_DUEL_CHANNEL", "the Letter Duel channel"))
        .replace("{movie}", &mention("VIZIER_MOVIE_CHANNEL", "the guess-the-movie channel"))
        .replace("{geo}", &mention("VIZIER_GEO_CHANNEL", "the geo channel"))
        .replace("{scoreboard}", &mention("VIZIER_SCOREBOARD_CHANNEL", "the game updates channel"))
}

/// The oldest note nobody has been told about yet.
pub fn next_news(done: &[String]) -> Option<&'static News> {
    NEWS.iter().find(|n| !done.iter().any(|d| d == n.id))
}

// --- when a batch is ready ------------------------------------------------------------------

/// Whether a batch that last moved at `changed_at` may go out now: the settings
/// have been quiet long enough, and the last announcement is far enough behind.
pub fn due(now: i64, changed_at: i64, last_post: Option<i64>, delay_mins: i64, gap_mins: i64) -> bool {
    now.saturating_sub(changed_at) >= delay_mins.max(0) * 60
        && last_post.is_none_or(|t| now.saturating_sub(t) >= gap_mins.max(0) * 60)
}

// --- posting ----------------------------------------------------------------------------------

/// One announcement: what it says, and the hash that stops it being said twice.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Post {
    title: String,
    body: String,
    hash: String,
}

impl Post {
    fn new(title: &str, body: &str) -> Post {
        let title = cut(title, TITLE_LIMIT);
        let body = cut(body, BODY_LIMIT);
        let hash = rules_text::digest(&[&title, &body]);
        Post { title, body, hash }
    }
}

/// Cut to fit Discord's limit, on a line break where there is one.
fn cut(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    if let Some(end) = out.rfind('\n').filter(|at| *at > limit / 2) {
        out.truncate(end);
    }
    out.push('…');
    out
}

/// The description of a "what's new" card, with as many lines as fit.
pub fn description(lines: &[String]) -> String {
    let mut out = String::new();
    let mut left = 0;
    for line in lines {
        let next = if out.is_empty() { line.clone() } else { format!("\n{}", line) };
        if out.chars().count() + next.chars().count() > BODY_LIMIT - 40 {
            left += 1;
            continue;
        }
        out.push_str(&next);
    }
    if left > 0 {
        out.push_str(&format!("\n…and {} more.", left));
    }
    out
}

/// "House Cup · see the full rules in #house-cup", when there is such a channel.
fn footer() -> Option<String> {
    control::id("VIZIER_SCOREBOARD_CHANNEL")
        .filter(|id| *id != 0)
        .map(|id| format!("House Cup · see the full rules in <#{}>", id))
}

fn message(p: &Post) -> CreateMessage {
    let mut embed = CreateEmbed::new().title(&p.title).description(&p.body).colour(GOLD);
    if let Some(footer) = footer() {
        embed = embed.footer(CreateEmbedFooter::new(footer));
    }
    // Never ping: these go to every common room at once.
    CreateMessage::new().embed(embed).allowed_mentions(CreateAllowedMentions::new())
}

/// "this went to that channel".
fn mark(hash: &str, channel: u64) -> String {
    format!("{}:{}", hash, channel)
}

/// The channels still owed this announcement, in order. A restart in the middle
/// of a round of posting picks up exactly where it stopped.
fn pending(channels: &[u64], done: &[String], hash: &str) -> Vec<u64> {
    channels.iter().copied().filter(|id| !done.iter().any(|d| *d == mark(hash, *id))).collect()
}

/// What to keep after a try: the channels that still wouldn't take it, and how
/// many times it has been tried in all (the first go counts as one). Nothing
/// left to do, or out of tries, and the announcement is let go of.
fn after_try(failed: Vec<u64>, attempts: u32) -> Option<(Vec<u64>, u32)> {
    if failed.is_empty() || attempts > MAX_RETRIES { None } else { Some((failed, attempts)) }
}

fn list(key: &str) -> Vec<String> {
    house::meta_get(key).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save<T: Serialize>(key: &str, value: &T) {
    match serde_json::to_string(value) {
        Ok(text) => house::meta_set(key, &text),
        Err(err) => tracing::warn!("announce: {} not stored: {}", key, err),
    }
}

fn stamp(key: &str) -> Option<i64> {
    house::meta_get(key).and_then(|v| v.parse().ok())
}

/// Posts one announcement everywhere it is still owed, and gives back the
/// channels that wouldn't take it. Each channel is written down the moment it
/// lands, so a failure - or a restart - never costs a repeat somewhere else.
async fn send(ctx: &Context, channels: &[u64], p: &Post) -> Vec<u64> {
    let mut done = list(MARKS);
    let mut failed = Vec::new();
    for id in pending(channels, &done, &p.hash) {
        match ChannelId::new(id).send_message(&ctx.http, message(p)).await {
            Ok(_) => {
                done.push(mark(&p.hash, id));
                if done.len() > KEEP_MARKS {
                    done.drain(..done.len() - KEEP_MARKS);
                }
                save(MARKS, &done);
            }
            Err(err) => {
                tracing::warn!("announce: \"{}\" not posted in {}: {}", p.title, id, err);
                failed.push(id);
            }
        }
    }
    failed
}

/// One line for the panel's activity log.
fn logged(key: &str, what: &str, by: u64) {
    if let Err(err) = control::log_change(key, None, Some(what), by) {
        tracing::warn!("announce: not written to the activity log: {}", err);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Retry {
    post: Post,
    channels: Vec<u64>,
    /// Tries so far, the first one included.
    attempts: u32,
}

/// Posts `p` and keeps hold of any channel that failed, for the next poll.
/// `true` when something was actually said somewhere.
async fn deliver(ctx: &Context, channels: &[u64], p: &Post, before: u32) -> bool {
    let owed = pending(channels, &list(MARKS), &p.hash).len();
    let failed = send(ctx, channels, p).await;
    let sent = owed - failed.len().min(owed);
    match after_try(failed, before + 1) {
        Some((channels, attempts)) => save(RETRY, &Retry { post: p.clone(), channels, attempts }),
        None => house::meta_set(RETRY, ""),
    }
    sent > 0
}

// --- the poll -----------------------------------------------------------------------------------

fn channels() -> Vec<u64> {
    control::ids("VIZIER_ANNOUNCE_CHANNELS").into_iter().filter(|id| *id != 0).collect()
}

/// Where a note actually goes.
///
/// `VIZIER_ANNOUNCE_CHANNELS` when it is set, and otherwise what this module
/// has always claimed in its own first paragraph: the four house common rooms,
/// found by name, and the houses channel. That setting being empty used to mean
/// announcements went NOWHERE, silently, which is a poor way to say "not
/// configured" - every release note since the feature was written sat unposted.
fn channels_for(ctx: &Context) -> Vec<u64> {
    let set = channels();
    if !set.is_empty() {
        return set;
    }
    let mut out = house::common_room_ids(ctx);
    if let Some(houses) = control::id("VIZIER_HOUSE_CHANNEL").filter(|id| *id != 0) {
        if !out.contains(&houses) {
            out.push(houses);
        }
    }
    out
}

/// A channel that failed last time gets another go before anything else.
async fn retry(ctx: &Context) -> bool {
    let Some(waiting) = house::meta_get(RETRY).filter(|t| !t.is_empty()).and_then(|t| serde_json::from_str::<Retry>(&t).ok())
    else {
        return false;
    };
    let channels = waiting.channels.clone();
    if waiting.attempts > MAX_RETRIES {
        tracing::warn!("announce: giving up on \"{}\" for {:?}", waiting.post.title, channels);
        house::meta_set(RETRY, "");
        return false;
    }
    deliver(ctx, &channels, &waiting.post, waiting.attempts).await;
    true
}

/// The next release note, if one is waiting.
async fn news(ctx: &Context, channels: &[u64]) -> bool {
    let mut done = list(NEWS_DONE);
    let Some(entry) = next_news(&done) else {
        return false;
    };
    let post = Post::new(entry.title, &fill_channels(entry.body));
    let said = deliver(ctx, channels, &post, 0).await;
    // Recorded either way: a note nobody could be told is still yesterday's news.
    done.push(entry.id.to_string());
    save(NEWS_DONE, &done);
    if said {
        house::meta_set(LAST_TS, &Utc::now().timestamp().to_string());
        logged("announce:news", &format!("{} · {} channels", entry.title, channels.len()), 0);
    }
    true
}

/// Takes the settings on as the ones everyone has been told about, without
/// saying anything. Writes nothing when they are already the stored ones, so an
/// idle bot leaves the store alone.
fn adopt(live: &Snapshot, now: i64) {
    if house::meta_get(ANNOUNCED).and_then(|t| serde_json::from_str::<Snapshot>(&t).ok()).as_ref() == Some(live) {
        return;
    }
    save(ANNOUNCED, live);
    save(SEEN, live);
    house::meta_set(SEEN_TS, &now.to_string());
}

/// The settings, compared with the last ones announced.
async fn settings(ctx: &Context, channels: &[u64], now: i64) -> bool {
    let live = snapshot(&Rules::live());
    let Some(announced) = house::meta_get(ANNOUNCED).and_then(|t| serde_json::from_str::<Snapshot>(&t).ok()) else {
        // Nothing to compare with: today's settings become the baseline.
        adopt(&live, now);
        return false;
    };
    let lines = changes(&announced, &live);
    if lines.is_empty() {
        // Nothing worth saying - a setting put back the way it was, or only
        // channels and timers moved. Catch the snapshot up quietly.
        adopt(&live, now);
        return false;
    }
    let seen = house::meta_get(SEEN).and_then(|t| serde_json::from_str::<Snapshot>(&t).ok());
    if seen.as_ref() != Some(&live) {
        // Still being edited: start the quiet period again.
        save(SEEN, &live);
        house::meta_set(SEEN_TS, &now.to_string());
        return false;
    }
    let delay = control::number("VIZIER_ANNOUNCE_DELAY_MINUTES", DELAY_DEFAULT) as i64;
    let gap = control::number("VIZIER_ANNOUNCE_MIN_GAP_MINUTES", GAP_DEFAULT) as i64;
    if !due(now, stamp(SEEN_TS).unwrap_or(now), stamp(LAST_TS), delay, gap) {
        return false;
    }
    let post = Post::new("📣 What's new", &description(&lines));
    let said = deliver(ctx, channels, &post, 0).await;
    // Said or not, these changes have had their turn: whatever wouldn't take it
    // is the retry's business now, and a channel nobody can post in must never
    // hold the same list up for announcing again and again.
    save(ANNOUNCED, &live);
    house::meta_set(LAST_TS, &now.to_string());
    if said {
        logged(
            "announce:settings",
            &format!("{} · {} channels", plural(lines.len() as i64, "change", "changes"), channels.len()),
            0,
        );
    }
    tracing::info!("announce: {} change(s) for {} channel(s), posted: {}", lines.len(), channels.len(), said);
    true
}

/// One pass: a channel still owed something, then a release note, then the
/// settings - at most one announcement a poll.
pub async fn poll(ctx: &Context) {
    if house::db().is_none() {
        return;
    }
    let now = Utc::now().timestamp();
    let channels = channels_for(ctx);
    if !control::on("VIZIER_ANNOUNCE", true) || channels.is_empty() {
        // Switched off, or nowhere to say it: keep the snapshot fresh so that
        // switching it on doesn't announce months of history at once. Release
        // notes are left waiting - they are worth telling late.
        adopt(&snapshot(&Rules::live()), now);
        return;
    }
    if retry(ctx).await || news(ctx, &channels).await {
        return;
    }
    settings(ctx, &channels, now).await;
}

/// Watches the settings from a minute after the bot connects.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        // Let the cache and the stores settle first.
        tokio::time::sleep(Duration::from_secs(20)).await;
        loop {
            poll(&ctx).await;
            tokio::time::sleep(Duration::from_secs(POLL_SECS)).await;
        }
    });
}

// --- /announce ------------------------------------------------------------------------------------

pub fn builder() -> CreateCommand {
    CreateCommand::new("announce")
        .description("admin only: post an announcement in the common rooms and the houses channel")
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "text", "what to tell everyone")
                .required(true)
                .max_length(NEWS_LIMIT as u16),
        )
}

/// `/announce`: the same card, in the same places, with your own words.
pub async fn command(ctx: &Context, command: &CommandInteraction) {
    if !super::admin_ids().contains(&command.user.id.get()) {
        let reply = CreateInteractionResponseMessage::new().content("Only bot admins can do that.").ephemeral(true);
        let _ = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await;
        return;
    }
    let text = command
        .data
        .options
        .iter()
        .find_map(|o| match (&o.value, o.name.as_str()) {
            (CommandDataOptionValue::String(v), "text") => Some(v.trim().to_string()),
            _ => None,
        })
        .unwrap_or_default();
    let _ = command.defer_ephemeral(&ctx.http).await;
    let channels = channels_for(ctx);
    let reply = if text.is_empty() {
        "There was nothing to say.".to_string()
    } else if channels.is_empty() {
        "No announcement channels are set, and the four common rooms could not be found by name either - add them on the panel under Announcements.".to_string()
    } else {
        let post = Post::new("📣 Announcement", &text);
        let failed = send(ctx, &channels, &post).await;
        logged("announce:custom", &cut(&text, 200), command.user.id.get());
        let landed = channels.len() - failed.len();
        match failed.as_slice() {
            [] => format!("Posted in {} channels.", landed),
            some => format!(
                "Posted in {} of {} channels. These wouldn't take it: {}",
                landed,
                channels.len(),
                some.iter().map(|id| format!("<#{}>", id)).collect::<Vec<_>>().join(", ")
            ),
        }
    };
    let _ = command.edit_response(&ctx.http, EditInteractionResponse::new().content(reply)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::discord::rules_text::tests::defaults;

    fn lines(edit: impl Fn(&mut Rules)) -> Vec<String> {
        let before = defaults();
        let mut after = before.clone();
        edit(&mut after);
        changes(&snapshot(&before), &snapshot(&after))
    }

    fn one(edit: impl Fn(&mut Rules)) -> String {
        let said = lines(edit);
        assert_eq!(said.len(), 1, "expected one line, got {:?}", said);
        said[0].clone()
    }

    #[test]
    fn nothing_changed_says_nothing() {
        assert!(lines(|_| {}).is_empty());
        // Things a player never sees are not news.
        assert!(lines(|r| r.quiz_channel = Some(42)).is_empty());
        assert!(lines(|r| r.snitch_hours = (9, 21)).is_empty());
        assert!(lines(|r| r.battle_times = "07:00".into()).is_empty());
        assert!(lines(|r| r.npat.round_secs = 90).is_empty());
        assert!(lines(|r| r.frog_open_minutes = 9).is_empty());
    }

    #[test]
    fn voice_says_both_halves_and_only_names_what_moved() {
        assert_eq!(
            one(|r| {
                r.voice_minutes = 30;
                r.voice_cap = Some(10);
            }),
            "🎙️ **Voice** now pays **1 point per 30 min** (was 60 min) · up to **10 a day** (was 4 a day)"
        );
        // Only the limit moved: the rate is stated, but not as a change.
        assert_eq!(
            one(|r| r.voice_cap = Some(10)),
            "🎙️ **Voice** now pays **1 point per 60 min** · up to **10 a day** (was 4 a day)"
        );
    }

    #[test]
    fn limits_coming_and_going_read_as_sentences() {
        assert!(one(|r| r.anagram_cap = None).contains("**no daily limit** any more (it was 6 a day)"));
        assert!(one(|r| r.snitch_cap = Some(5)).contains("up to **5 a day** (there was no limit)"));
        assert!(one(|r| r.frog_points = [3, 4, 10]).ends_with("· no daily limit"));
    }

    #[test]
    fn every_kind_of_field_has_words() {
        assert_eq!(
            one(|r| r.chat_tiers = vec![10, 20]),
            "💬 **Chat** now pays 1 point at **10 and 20 messages in a day** (was 20, 60 and 150) · up to **3 a day**"
        );
        assert!(one(|r| r.quiz = [3, 2, 1]).starts_with("🧠 **Quiz** now pays **3 · 2 · 1** (was 2 · 1 · 1)"));
        assert!(one(|r| r.koto_played = 2).contains("**2** (was 1) for each guess that scores"));
        assert!(one(|r| r.cat = [2, 3, 4]).contains("**2 · 3 · 4** (was 1 · 2 · 3)"));
        assert!(one(|r| r.wordle = [5, 3, 2, 1, 2]).contains("**5 · 3 · 2 · 1** (was 4 · 3 · 2 · 1)"));
        assert!(one(|r| r.wordle = [4, 3, 2, 1, 3]).contains("**+3** (was +1) for the day's best"));
        assert!(one(|r| r.arena_win = 2).starts_with("⚔️ **1v1 fights** now pay **2** (was 1)"));
        assert_eq!(
            one(|r| r.royale = [10, 3]),
            "👑 **Battle Royale** now pays **10** (was 8) to the champion and **3** to the runner-up"
        );
        assert_eq!(
            one(|r| r.snitch_drops = (3, 3)),
            "🪽 **The Golden Snitch** now appears **3 times a day** (was 6–8 times a day)"
        );
        assert!(one(|r| r.snitch_points[2] = [8, 4, 2]).contains("golden **8 · 4 · 2** (was 6 · 4 · 2)"));
        assert!(one(|r| r.frog_drops_max = 12).contains("drop up to **12 a day** (was 10 a day)"));
        assert_eq!(one(|r| r.set_bonus = 50), "🏆 **A full card set** now sells for **50** (was 35)");
        assert_eq!(one(|r| r.set_bonus = 0), "🏆 **A full card set** can't be sold for points any more (it paid **35**)");
        assert!(one(|r| r.npat.min_players = 4).contains("need **4 players** (was 5 players) from **2 houses**"));
        assert!(one(|r| r.npat.letters_per_game = 6).contains("now **6 letters** (was 5 letters) long"));
        assert!(one(|r| r.npat.scores = [12, 5]).contains("score **12** (was 10) when nobody else has them"));
        assert!(one(|r| r.npat.prizes = [3, 1]).contains("🥇 **+3** (was +2) · 🥈 **+1**"));
    }

    #[test]
    fn switches_say_on_and_off() {
        assert_eq!(one(|r| r.frogs_on = false), "🐸 **Chocolate Frogs** are now **OFF**");
        assert_eq!(
            one(|r| {
                r.frogs_on = true;
                r.snitch_on = false
            }),
            "🪽 **The Golden Snitch** is now **OFF**"
        );
        assert_eq!(one(|r| r.games_on = false), "🎮 **Word game points** (Koto, Anagram, Cat Bot) are now **OFF**");
        assert_eq!(one(|r| r.wordle_on = false), "🟩 **Wordle points** are now **OFF**");
        assert_eq!(one(|r| r.activity_on = false), "💬 **Chat and voice points** are now **OFF**");
        assert_eq!(one(|r| r.battle_daily = false), "⚔️ **The daily Battle Royale** is now **OFF**");
        assert_eq!(one(|r| r.trades_on = false), "🔄 **Card trading** is now **OFF**");
        assert_eq!(one(|r| r.npat.channel = None), "🔤 **Name Place Animal Thing** is now **OFF**");
        assert_eq!(one(|r| r.daily_top_cards = false), "🃏 **A card for the day's top scorer** is now **OFF**");
    }

    /// Switching a game off moves everything under it; that is one piece of news.
    #[test]
    fn a_switch_covers_its_own_settings() {
        let said = lines(|r| {
            r.frogs_on = false;
            r.trades_on = false;
            r.daily_top_cards = false;
            r.royale_cards = false;
            r.frog_points = [0, 0, 0];
        });
        assert_eq!(said, vec!["🐸 **Chocolate Frogs** are now **OFF**"]);
    }

    #[test]
    fn a_run_of_edits_becomes_one_list_in_order() {
        let said = lines(|r| {
            r.voice_minutes = 30;
            r.koto_win = 5;
            r.frogs_on = false;
            r.npat.prizes = [3, 2];
        });
        assert_eq!(said.len(), 4);
        assert!(said[0].starts_with("🎙️"), "{:?}", said);
        assert!(said[1].starts_with("🔤 **Koto**"), "{:?}", said);
        assert!(said[2].starts_with("🐸"), "{:?}", said);
        assert!(said[3].starts_with("🔤 **Name Place"), "{:?}", said);
    }

    /// A field the stored snapshot never had - a game added since - is taken on
    /// quietly, and so is one whose shape has changed.
    #[test]
    fn new_and_reshaped_fields_are_adopted_quietly() {
        let now = snapshot(&defaults());
        let mut old = now.clone();
        old.remove("koto");
        old.insert("cat".into(), vec!["1".into()]);
        assert!(changes(&old, &now).is_empty());
        // And a field that is gone from the code doesn't stop the rest.
        let mut stale = now.clone();
        stale.insert("sudoku".into(), vec!["3".into()]);
        stale.insert("arena".into(), vec!["9".into(), "3".into()]);
        assert_eq!(changes(&stale, &now).len(), 1);
    }

    #[test]
    fn a_batch_waits_for_quiet_and_keeps_its_distance() {
        let (min, delay, gap) = (60, 10, 30);
        // Still being edited two minutes ago: not yet.
        assert!(!due(1_000 * min, 998 * min, None, delay, gap));
        assert!(due(1_000 * min, 990 * min, None, delay, gap));
        // Quiet long enough, but something was announced ten minutes ago.
        assert!(!due(1_000 * min, 980 * min, Some(990 * min), delay, gap));
        assert!(due(1_000 * min, 980 * min, Some(970 * min), delay, gap));
        // No wait at all is allowed, and a clock that jumped backwards never posts early.
        assert!(due(1_000 * min, 1_000 * min, None, 0, 0));
        assert!(!due(1_000 * min, 1_001 * min, None, delay, gap));
    }

    #[test]
    fn a_release_note_is_posted_once() {
        assert_eq!(fill_channels("play in {sudoku}"), "play in **the sudoku channel**", "no channel set in tests");
        assert_eq!(fill_channels("type it in {anagram}"), "type it in **the anagrams channel**");
        assert_eq!(fill_channels("draw in {guess}"), "draw in **the guess-the-word channel**");
        // Every note's channel marks are ones fill_channels knows, or a note
        // would go out with `{anagram}` in it.
        for note in NEWS {
            let filled = fill_channels(note.body);
            assert!(!filled.contains('{'), "{} still has a channel mark in it:\n{}", note.id, filled);
            assert!(note.body.chars().count() <= NEWS_LIMIT, "{} is too long", note.id);
        }
        let first = next_news(&[]).expect("a seeded note");
        assert_eq!(first.id, NEWS[0].id);
        let done: Vec<String> = NEWS.iter().map(|n| n.id.to_string()).collect();
        assert!(next_news(&done).is_none());
        // Ids are stable and unique, or a note would go out twice.
        let mut ids: Vec<&str> = NEWS.iter().map(|n| n.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), NEWS.len());
    }

    #[test]
    fn a_release_note_never_shows_its_own_escapes() {
        // `"\\u{1F3AC}"` is a backslash, a u and some braces - not a clapperboard.
        // One went out reading "\u{1F3AC} Guess the Movie \u{2014} now played in
        // matches of 10", in front of the whole server.
        for entry in NEWS {
            for (what, text) in [("title", entry.title), ("body", entry.body)] {
                assert!(!text.contains("\\u{"), "{}'s {} shows an escape: {}", entry.id, what, text);
            }
        }
    }

    #[test]
    fn release_notes_fit_in_an_embed() {
        for entry in NEWS {
            assert!(entry.body.chars().count() <= NEWS_LIMIT, "{} is too long", entry.id);
            assert!(entry.title.chars().count() <= TITLE_LIMIT, "{} has a long title", entry.id);
            assert!(!entry.title.is_empty() && !entry.body.is_empty(), "{} is empty", entry.id);
            let post = Post::new(entry.title, entry.body);
            assert_eq!(post.body, entry.body, "{} was cut", entry.id);
        }
    }

    #[test]
    fn a_long_list_of_changes_is_cut_to_fit() {
        let long: Vec<String> = (0..400).map(|i| format!("🎙️ line number {} about a setting that moved", i)).collect();
        let text = description(&long);
        assert!(text.chars().count() <= BODY_LIMIT);
        assert!(text.contains("…and "), "{}", text);
        let post = Post::new("📣 What's new", &text);
        assert!(post.body.chars().count() <= BODY_LIMIT && post.title.chars().count() <= TITLE_LIMIT);
        // A short list is left exactly as it is.
        let short = vec!["one".to_string(), "two".to_string()];
        assert_eq!(description(&short), "one\ntwo");
    }

    #[test]
    fn nothing_in_an_announcement_can_ping() {
        let post = Post::new("📣 What's new", &description(&lines(|r| r.voice_minutes = 30)));
        let json = serde_json::to_value(message(&post)).expect("a message");
        let mentions = json.get("allowed_mentions").expect("allowed mentions are set");
        for what in ["parse", "users", "roles"] {
            let list = mentions.get(what).and_then(|v| v.as_array()).cloned().unwrap_or_default();
            assert!(list.is_empty(), "{} would ping: {:?}", what, list);
        }
        for entry in NEWS {
            assert!(!entry.body.contains("@everyone") && !entry.body.contains("@here"), "{} pings", entry.id);
        }
    }

    #[test]
    fn a_post_is_only_owed_to_the_channels_that_missed_it() {
        let channels = vec![1, 2, 3, 4, 5];
        let hash = "abc";
        assert_eq!(pending(&channels, &[], hash), channels);
        // Two landed before a restart; only the other three are owed.
        let done = vec![mark(hash, 1), mark(hash, 2), mark("other", 3)];
        assert_eq!(pending(&channels, &done, hash), vec![3, 4, 5]);
        let all: Vec<String> = channels.iter().map(|id| mark(hash, *id)).collect();
        assert!(pending(&channels, &all, hash).is_empty());
    }

    #[test]
    fn a_channel_that_fails_is_retried_twice_and_then_left() {
        // The others still got it; only the two that failed come back.
        assert_eq!(after_try(vec![3, 5], 1), Some((vec![3, 5], 1)));
        // Everything landed: nothing is held on to.
        assert_eq!(after_try(vec![], 1), None);
        // The first go, then two retries, and it's let go of.
        let mut attempts = 1;
        let mut held = after_try(vec![3], attempts);
        let mut retries = 0;
        while let Some((failed, done)) = held {
            retries += 1;
            attempts = done + 1;
            held = after_try(failed, attempts);
        }
        assert_eq!(retries, MAX_RETRIES);
    }

    #[test]
    fn the_snapshot_survives_a_trip_through_the_store() {
        let now = snapshot(&defaults());
        let text = serde_json::to_string(&now).expect("json");
        let back: Snapshot = serde_json::from_str(&text).expect("json back");
        assert_eq!(now, back);
        assert!(changes(&back, &now).is_empty());
        assert_eq!(back.len(), FIELDS.len());
    }
}

