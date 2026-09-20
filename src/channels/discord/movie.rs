//! Guess the Movie, in its own channel (`VIZIER_MOVIE_CHANNEL`).
//!
//! There is ALWAYS a film waiting. Its card is the last message in the channel -
//! and like anagrams and Guess the Word, and unlike sudoku or chess, people CAN
//! type there: guessing IS typing. The bot takes a film from the bank and asks
//! about it ONE way - five words about it, a line out of it, or a still from a
//! scene - and whoever names it first wins.
//!
//! Titles are typed badly, so the bank forgives a great deal: spelling,
//! transliteration, a dropped "of", a missing "The". What it never forgives is
//! a sequel number, or a guess that fits two films at once
//! ([`movie_bank`](super::movie_bank)). The first correct guess gets a ✅ on the
//! message, a line naming the winner, and the next film at once. Everything else
//! typed in the channel is left alone: the room is a social one and a ❌ on
//! every stray message would be noise.
//!
//! Two words steer a round. `!hint` reveals the NEXT tag down - the sharper one
//! the card held back - along with the title's first letter, its year and where
//! it is from, once per round, and takes a point off what the round pays (never
//! below one); `!skip` opens up only after a hint, names the film and pays
//! nothing. A round nobody touches goes stale by itself after
//! `VIZIER_MOVIE_IDLE_MINUTES`.
//!
//! One task does all the posting ([`run`]), the way Guess the Word does: the
//! message handler only writes to the database and leaves a note in the shared
//! state, so two guesses landing together can never post two rounds. The same
//! task keeps the card near the bottom and puts it back if someone deletes it -
//! but the room is chatty, so the card only follows the conversation down once
//! chat has genuinely buried it ([`card_plan`]).

use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;
use parking_lot::Mutex;
use serenity::all::{
    ButtonStyle, ChannelId, CommandInteraction, ComponentInteraction, Context, CreateActionRow, CreateAllowedMentions, CreateAttachment,
    CreateButton, CreateCommand, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
    EditAttachments, EditMessage, GetMessages, Message, MessageId, ReactionType,
};

use super::control;
use super::movie_bank::{self as bank, ATTRIBUTION, Bank, Clue, Movie, Pool, Shot};
use super::movie_store::{self as store, Status};
use super::points::{Cap, Outcome, Source};
use super::rules_text::{self, MovieRules};
use super::sudoku_gen::Rng;

/// How often the task looks at the clock.
const TICK: Duration = Duration::from_secs(1);
/// How long one Discord call may take.
const HTTP_WAIT: Duration = Duration::from_secs(20);
/// Rounds `/movie` and the day's list look back over.
const RECENT: usize = 20;

const COLOUR: u32 = 0xC0392B;

/// The channel the game plays in unless a mod moves it: 🎬 guess-the-movie.
pub const HOME_CHANNEL: u64 = 1_551_044_377_374_101_594;

/// What someone types to ask for a sharper tag, or to pass on a round.
/// The button that says somebody wants to play the next match.
pub const READY_ID: &str = "movieready:";
/// `moviepool:<match>:<pool>` — the vote for what the next match plays.
pub const POOL_ID: &str = "moviepool:";

pub const HINT_WORD: &str = "!hint";
pub const SKIP_WORD: &str = "!skip";

/// How long between two "the hint is already out" or "not yet" replies. The
/// channel is a social one: the same reminder five times over is noise.
pub const NAG_EVERY_MS: i64 = 30_000;

/// The tick mark the winning message gets.
const TICK_MARK: &str = "✅";
/// What a round's ledger row is called. Its own prefix, so a movie row can
/// never be taken for any other game's.
pub const LEDGER_KEY: &str = "movie:round:";

// --- settings -----------------------------------------------------------------------------

fn switched_on() -> bool {
    control::on("VIZIER_MOVIE", false)
}

fn channel_setting() -> Option<u64> {
    control::id("VIZIER_MOVIE_CHANNEL").or(Some(HOME_CHANNEL)).filter(|id| *id != 0 && *id != super::weekly::SAFE_CORNER)
}

/// The game's channel when the game is on AND there are films to play with.
/// No bank, no game: the bot never asks about a film it can't judge.
pub fn live_channel() -> Option<u64> {
    channel_setting().filter(|_| switched_on() && bank::bank().is_some())
}

/// What naming a film is worth in MOVIE points. House points are not paid per
/// film any more - they are the match prize, see [`win_points`].
pub fn round_points() -> i64 {
    control::number("VIZIER_POINTS_MOVIE", 3).min(100) as i64
}

/// How many films one match runs before it is scored.
pub fn match_films() -> i64 {
    control::number("VIZIER_MOVIE_MATCH_FILMS", 10).clamp(1, 200) as i64
}

/// The fewest people who must press Ready before a match may start.
pub fn min_players() -> usize {
    control::number("VIZIER_MOVIE_MIN_PLAYERS", 2).clamp(1, 20) as usize
}

/// The rest between matches. Nothing starts before it is up, however many are
/// ready - the break is there so people can arrive, not just so they can skip it.
pub fn break_minutes() -> i64 {
    control::number("VIZIER_MOVIE_BREAK_MINUTES", 2).clamp(0, 240) as i64
}

/// The match prize: house points for first and second.
pub fn win_points() -> i64 {
    control::number("VIZIER_POINTS_MOVIE_WIN", 5).min(100) as i64
}

pub fn second_points() -> i64 {
    control::number("VIZIER_POINTS_MOVIE_SECOND", 2).min(100) as i64
}

/// How long a round may sit untouched before the bot moves on by itself.
fn idle_minutes() -> i64 {
    control::number("VIZIER_MOVIE_IDLE_MINUTES", 20).clamp(1, 1440) as i64
}

/// How long the same film - and any single clue about it - is held back.
fn no_repeat_days() -> i64 {
    control::number("VIZIER_MOVIE_NO_REPEAT_DAYS", 30).min(365) as i64
}

/// How many tags the card shows. The rest are what a hint reveals.
pub fn tags_shown() -> i64 {
    control::number("VIZIER_MOVIE_TAGS_SHOWN", 5).clamp(3, 7) as i64
}

/// How far the channel leans Hindi, and how far it leans recent. Both are a
/// lean and not a rule - see [`Bank::pick`].
fn hindi_share() -> u64 {
    control::number("VIZIER_MOVIE_HINDI_SHARE", 50).min(100)
}

fn modern_share() -> u64 {
    control::number("VIZIER_MOVIE_MODERN_SHARE", 65).min(100)
}

fn daily_cap() -> Option<i64> {
    match Source::Movie.cap() {
        Cap::PerDay(n) => Some(n),
        _ => None,
    }
}

/// How many messages from other people have to land under the card before it
/// follows the conversation down.
fn bump_messages() -> u64 {
    control::number("VIZIER_MOVIE_BUMP_MESSAGES", 5).clamp(1, 100)
}

/// And how long between two of those moves, however busy the channel is.
fn bump_seconds() -> i64 {
    control::number("VIZIER_MOVIE_BUMP_SECONDS", 120).clamp(10, 3600) as i64
}

/// Everything `/moviehelp` and the House Cup posts say about the game.
pub fn movie_rules() -> MovieRules {
    MovieRules {
        channel: live_channel(),
        points: round_points(),
        cap: daily_cap(),
        idle_minutes: idle_minutes(),
        no_repeat_days: no_repeat_days(),
        tags_shown: tags_shown(),
        match_films: match_films(),
        min_players: min_players(),
        break_minutes: break_minutes(),
        win_points: win_points(),
        second_points: second_points(),
        films: bank::bank().map(Bank::count),
        shows: bank::bank().map(Bank::series_count),
        stills: bank::bank().map(Bank::shot_count),
    }
}

// --- words ---------------------------------------------------------------------------------

fn plural(n: i64, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// "48 s", "7 min 41 s", "1 h 04 min".
pub fn spent_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{} s", s),
        s if s < 3600 => format!("{} min {:02} s", s / 60, s % 60),
        s => format!("{} h {:02} min", s / 3600, (s % 3600) / 60),
    }
}

/// How long a round has been up, in round words.
pub fn open_words(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

/// What a round pays once the hint has been out: a point less, never below one,
/// so a hinted round is always still worth playing.
pub fn worth_after_hint(points: i64, hinted: bool) -> i64 {
    if hinted { (points - 1).max(1) } else { points.max(0) }
}

pub fn card_title(round: i64) -> String {
    format!("🎬 Guess the Movie · Round #{}", round)
}

/// The tags as the card writes them: "revenge · guns · coal mafia".
pub fn tag_line(tags: &[String]) -> String {
    tags.join(" · ")
}

/// What a hint gives away besides the letter: the year and where the film is
/// from, which narrow it without naming it.
pub fn placing(year: i64, industry: &str) -> String {
    format!("{}, {}", industry, year)
}

/// Everything the live card says about a round right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Live {
    pub clue: Clue,
    /// The tags on the card, the sharper one a hint added among them.
    pub tags: Vec<String>,
    /// The line, for a dialogue round.
    pub line: Option<String>,
    /// What the hint revealed, whatever kind of round it was: a sharper tag, or
    /// the line written for the second still.
    pub revealed: Option<String>,
    /// What it is worth now, the hint already taken off.
    pub points: i64,
    /// Who asked for the hint, the letter it gave away, and where the film is
    /// from.
    pub hint: Option<(u64, char, String)>,
    /// Hindi, English or Japanese, on every card. With Hindi films, Hindi
    /// television, English television and anime all in one bank, knowing which
    /// language the answer is in is the difference between a fair round and a
    /// shot in the dark.
    pub language: String,
    pub open_secs: i64,
}

pub fn card_text(live: &Live) -> String {
    let mut text = format!("# {}\n", live.clue.heading());
    match live.clue {
        Clue::Tags => text.push_str(&format!("🏷️ **{}**\n", tag_line(&live.tags))),
        Clue::Hint => text.push_str(&format!("🔎 *{}*\n", live.line.clone().unwrap_or_default())),
        Clue::Dialogue => {
            text.push_str(&format!("> *{}*\n", live.line.clone().unwrap_or_default()));
            // A hinted dialogue round has a tag to show as well, and it is the
            // only place it can go.
            if !live.tags.is_empty() {
                text.push_str(&format!("🏷️ **{}**\n", tag_line(&live.tags)));
            }
        }
        Clue::Shot => {
            if !live.tags.is_empty() {
                text.push_str(&format!("🏷️ **{}**\n", tag_line(&live.tags)));
            }
        }
    }
    if !live.language.is_empty() {
        text.push_str(&format!("🗣️ **{}**\n", live.language));
    }
    if let Some(revealed) = &live.revealed {
        text.push_str(&format!("💡 **{}**\n", revealed));
    }
    text.push_str(&format!("worth **{}** · first to name it wins", plural(live.points, "point", "points")));
    if let Some((who, letter, where_from)) = &live.hint {
        text.push_str(&format!(
            "\n💡 it starts with **{}** · {} — <@{}> asked, so the round is worth a point less",
            letter, where_from, who
        ));
    }
    text.push_str(&format!("\n-# ⏱️ up {} · just type the title", open_words(live.open_secs)));
    text
}

pub const CARD_FOOTER: &str = "Type the title here · !hint for a sharper clue and the first letter · !skip once a hint is out · /moviehelp";

/// What the channel is told when a round is won.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Won {
    pub round: i64,
    pub winner: u64,
    /// The winner's crest and house, when they are in one.
    pub badge: String,
    /// What they typed, and what the film was.
    pub guess: String,
    pub title: String,
    pub year: i64,
    /// The HOUSE points the ledger paid: zero once the day's cap is full, and
    /// zero for anyone with no house.
    pub points: i64,
    /// The MOVIE points the round was worth, hint taken off and no cap.
    pub worth: i64,
    /// What it would have paid without the hint.
    pub full_points: i64,
    /// Whether the winner is in a house at all.
    pub housed: bool,
    /// Their movie points today, this round included.
    pub tally: i64,
    pub hinted: bool,
    pub seconds: i64,
}

/// A solve that paid no house points still won the round, and the line says so
/// as a win: the movie points are the score that always counts.
pub fn won_text(w: &Won) -> String {
    let badge = if w.badge.is_empty() { String::new() } else { format!(" {}", w.badge) };
    let why = match (w.points, w.housed) {
        (0, true) => Some("that's your house points for today, but the movie points still count"),
        (0, false) => Some("no house to pay, but the movie points still count"),
        _ => None,
    };
    let (scored, today) = match why {
        Some(_) => (plural(w.worth, "movie point", "movie points"), format!("{} today", w.tally)),
        None => (w.points.to_string(), format!("{} movie points today", w.tally)),
    };
    let mut text = format!("✅ <@{}>{} had it: **{}** ({}) · **+{}** · {}", w.winner, badge, w.title, w.year, scored, today);
    let mut notes = vec![format!("round #{} in {}", w.round, spent_words(w.seconds))];
    if bank::key(&w.guess) != bank::key(&w.title) {
        notes.push(format!("they typed {}", w.guess));
    }
    if w.hinted {
        notes.push(format!("a hint was out, so {} instead of {}", w.worth, w.full_points));
    }
    if let Some(why) = why {
        notes.push(why.to_string());
    }
    text.push_str(&format!("\n-# {}", notes.join(" · ")));
    text
}

/// The line that goes up when a round ends without a winner.
pub fn ended_text(row: &store::Row, year: Option<i64>) -> String {
    let named = match year {
        Some(year) => format!("**{}** ({})", row.title, year),
        None => format!("**{}**", row.title),
    };
    match (row.status, row.ended_by) {
        (Status::Skipped, Some(by)) => {
            format!("⏭️ <@{}> passed on round #{} — it was {}. No points for that one; here's another.", by, row.id, named)
        }
        (Status::Skipped, None) => format!("⏭️ Round #{} skipped — it was {}.", row.id, named),
        _ => format!("⏰ Nobody had round #{} — it was {}. A new one is up.", row.id, named),
    }
}

/// What a hint says in the channel. It never names the film, only its first
/// letter and where it is from - the sharper tag is on the card itself.
pub fn hint_text(row: &store::Row, by: u64, worth: i64, clue: Option<&str>, second_shot: bool, placing: &str) -> String {
    let sharper = match (clue, second_shot) {
        (Some(clue), true) => format!("a second still — **{}** — and ", clue),
        (Some(clue), false) => format!("a sharper clue — **{}** — and ", clue),
        (None, true) => "a second still, and ".to_string(),
        (None, false) => "that ".to_string(),
    };
    format!(
        "💡 <@{}> asked for the hint: round #{} gets {}it starts with **{}** ({}). It is now worth **{}** — `!skip` moves on if it's still hopeless.",
        by,
        row.id,
        sharper,
        row.first_letter(),
        placing,
        plural(worth, "point", "points")
    )
}

/// Whether the bot should say so again, or let it go: the same reminder over
/// and over would be exactly the noise this game keeps out of the channel.
pub fn nag_due(last_ms: i64, now_ms: i64) -> bool {
    now_ms - last_ms >= NAG_EVERY_MS
}

/// Why a `!skip` was turned down.
pub const SKIP_TOO_SOON: &str = "🚫 `!skip` opens up once somebody has used `!hint` on the round. Try the hint first.";

/// Where someone stands on a ranked board, counting from one. `None` when they
/// aren't on it at all.
pub fn place_of(rows: &[store::Tally], me: u64) -> Option<usize> {
    rows.iter().position(|t| t.user == me).map(|i| i + 1)
}

/// "3rd", the way a line reads it.
pub fn ordinal(n: usize) -> String {
    let suffix = match (n % 10, n % 100) {
        (_, 11..=13) => "th",
        (1, _) => "st",
        (2, _) => "nd",
        (3, _) => "rd",
        _ => "th",
    };
    format!("{}{}", n, suffix)
}

/// One person's own line on a board: where they stand out of how many, what
/// they have and over how many rounds.
pub fn standing(rows: &[store::Tally], me: u64) -> Option<String> {
    let place = place_of(rows, me)?;
    let mine = rows.get(place - 1)?;
    Some(format!(
        "{} of {} · **{}** · {}",
        ordinal(place),
        rows.len(),
        plural(mine.points, "movie point", "movie points"),
        plural(mine.solves, "round", "rounds")
    ))
}

/// What `/movie` shows whoever ran it: the round that is up, and their own
/// movie points today and this month.
pub fn mine_text(live: Option<&store::Row>, today: &[store::Tally], month: &[store::Tally], me: u64, channel: Option<u64>) -> String {
    let mut lines = vec!["🎬 **Guess the Movie**".to_string()];
    match live {
        Some(row) => {
            let worth = worth_after_hint(row.points, row.hinted());
            lines.push(format!("**Round #{}** · worth **{}** · the clue is below.", row.id, plural(worth, "point", "points")));
            match row.hint_by {
                Some(by) => lines.push(format!("💡 <@{}> had the hint: it starts with **{}**.", by, row.first_letter())),
                None => lines.push("-# `!hint` in the channel reveals a sharper clue and gives away the first letter, once a round.".to_string()),
            }
        }
        None => lines.push("No round is up right now — the next one is on its way.".to_string()),
    }
    lines.push(match standing(today, me) {
        Some(mine) => format!("-# **You today:** {}", mine),
        None => "-# You haven't won one today. Type the title straight into the channel.".to_string(),
    });
    if let Some(mine) = standing(month, me) {
        lines.push(format!("-# **This month:** {}", mine));
    }
    if let Some(c) = channel {
        lines.push(format!("-# The card lives in <#{}> · `/movietop` for the board · `/moviehelp` explains the rest.", c));
    }
    lines.push(format!("-# {}", ATTRIBUTION));
    lines.join("\n")
}

/// How many names `/movietop` lists.
const TOP_LIST: usize = 10;

/// What `/movietop` says.
pub fn top_text(period: &str, rows: &[store::Tally], me: u64) -> String {
    let mut text = format!("🎬 **Movie points** · {}", period);
    if rows.is_empty() {
        text.push_str("\nNobody has named one yet. The card is waiting in the channel.");
        return text;
    }
    for (i, t) in rows.iter().take(TOP_LIST).enumerate() {
        let rank = match i {
            0 => "🥇".to_string(),
            1 => "🥈".to_string(),
            2 => "🥉".to_string(),
            n => format!("`{:>2}.`", n + 1),
        };
        let you = if t.user == me { " ← you" } else { "" };
        text.push_str(&format!("\n{} <@{}> **{}** · {}{}", rank, t.user, t.points, plural(t.solves, "round", "rounds"), you));
    }
    if place_of(rows, me).is_some_and(|p| p > TOP_LIST) {
        if let Some(mine) = standing(rows, me) {
            text.push_str(&format!("\n-# **You:** {}", mine));
        }
    }
    text.push_str("\n-# Movie points count every solve at its full value — the daily house-points limit never takes one away.");
    text
}

// --- reading what was typed --------------------------------------------------------------

/// What a message in the game's channel turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    Hint,
    Skip,
    /// A guess the bank accepts for the round's film.
    Guess(String),
    /// Anything else at all, which the channel never hears about.
    Nothing,
}

/// Reads a message against the round that is up. Wrong guesses and chatter come
/// back the same way - [`Typed::Nothing`] - because the channel is a social one
/// and only a right guess is ever answered.
pub fn read_message(text: &str, movie: usize, bank: &Bank) -> Typed {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case(HINT_WORD) {
        return Typed::Hint;
    }
    if trimmed.eq_ignore_ascii_case(SKIP_WORD) {
        return Typed::Skip;
    }
    if bank.wins(movie, trimmed) {
        return Typed::Guess(trimmed.to_string());
    }
    Typed::Nothing
}

/// Whether a round has been sitting untouched long enough for the bot to move
/// on by itself.
pub fn stale(posted_ts: i64, now: i64, idle_minutes: i64) -> bool {
    now - posted_ts >= idle_minutes.max(1) * 60
}

// --- matches --------------------------------------------------------------------------------

/// What a match pays each place. First is [`win_points`], second
/// [`second_points`], and everyone else nothing.
///
/// A TIE FOR FIRST pays every tied player the winner's share and skips second
/// altogether: splitting it would make a draw worth less than a win for no
/// reason anybody could see, and paying second to a third player behind two
/// joint winners reads as a mistake. A tie for SECOND pays them all the
/// runner-up share, for the same reason.
pub fn prize_table(scores: &[(u64, i64)], win: i64, second: i64) -> Vec<(u64, i64, i64)> {
    let Some((_, top)) = scores.first().copied() else { return Vec::new() };
    if top <= 0 {
        return Vec::new();
    }
    let winners: Vec<u64> = scores.iter().filter(|(_, n)| *n == top).map(|(u, _)| *u).collect();
    let mut out: Vec<(u64, i64, i64)> = winners.iter().map(|u| (*u, 1, win)).collect();
    // Joint winners take the second place with them: paying a runner-up behind
    // two people who both came first reads as a mistake to everyone looking at
    // it. One winner, and the next score down is second - however many hold it.
    if winners.len() == 1 {
        if let Some((_, runner)) = scores.iter().find(|(_, n)| *n < top).copied() {
            if runner > 0 && second > 0 {
                out.extend(scores.iter().filter(|(_, n)| *n == runner).map(|(u, _)| (*u, 2, second)));
            }
        }
    }
    out
}

/// Whether a match may start: enough people ready AND the break served out.
pub fn may_start(ready: usize, min: usize, votes: usize, now: i64, ready_from: i64) -> bool {
    ready >= min.max(1) && votes > 0 && now >= ready_from
}

fn plural_u(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

/// The card between matches.
pub fn break_text(ready: &[u64], min: usize, films: i64, votes: usize, now: i64, ready_from: i64, last: Option<&MatchResult>) -> String {
    let mut text = String::from("# 🎬 Guess the Movie\n");
    if let Some(last) = last {
        text.push_str(&format!("{}\n", result_line(last)));
    }
    text.push_str(&format!("Next match: **{} films**. Press **I'm ready** to play.\n", films));
    if ready.is_empty() {
        text.push_str(&format!("Nobody's ready yet — **{}** needed to start.\n", min));
    } else {
        let names = ready.iter().map(|id| format!("<@{}>", id)).collect::<Vec<_>>().join(" · ");
        text.push_str(&format!("**Ready:** {}\n", names));
        if ready.len() < min {
            text.push_str(&format!("{} more and it can start.\n", min - ready.len()));
        }
    }
    let left = ready_from - now;
    if left > 0 {
        text.push_str(&format!("Starting in **{}**", if left < 60 { format!("{}s", left) } else { format!("{} min", left.div_euclid(60) + 1) }));
        if ready.len() < min {
            text.push_str(&format!(" at the earliest, once {} are ready", min));
        }
        text.push('\n');
    } else if ready.len() >= min && votes > 0 {
        text.push_str("Starting now…\n");
    }
    if votes == 0 {
        text.push_str("Waiting on the vote — **one press below** and it can start.\n");
    }
    text.push_str(&format!("-# 🥇 **{}** house points · 🥈 **{}** · you can join a match already running", win_points(), second_points()));
    text
}

/// A match's final table, for the line that ends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchResult {
    pub id: i64,
    pub films: i64,
    /// (who, films named, place, house points the ledger actually paid).
    pub places: Vec<(u64, i64, i64, i64)>,
}

pub fn result_line(r: &MatchResult) -> String {
    if r.places.is_empty() {
        return format!("🎬 Match #{} ended with nobody on the board.", r.id);
    }
    let mut parts = Vec::new();
    for (who, films, place, paid) in &r.places {
        let medal = if *place == 1 { "🥇" } else { "🥈" };
        let pts = if *paid > 0 { format!("+{}", paid) } else { "+0".to_string() };
        parts.push(format!("{} <@{}> **{}** ({} {})", medal, who, pts, films, if *films == 1 { "film" } else { "films" }));
    }
    format!("🎬 **Match #{} done** — {}", r.id, parts.join(" · "))
}

pub fn scoreboard_line(scores: &[(u64, i64)], played: i64, films: i64) -> String {
    let mut text = format!("-# match {}/{}", played, films);
    for (i, (who, n)) in scores.iter().take(3).enumerate() {
        let medal = ["🥇", "🥈", "🥉"][i];
        text.push_str(&format!(" · {} <@{}> {}", medal, who, n));
    }
    text
}

// --- shared state ----------------------------------------------------------------------------

#[derive(Default)]
struct Shared {
    /// (channel, message) of the round card. The ONE card the channel has.
    card: Option<(u64, u64)>,
    /// Cards a delete failed on, tried again until they are gone.
    orphans: Vec<(u64, u64)>,
    /// Messages from other people that have landed under the card.
    others_since_card: u64,
    /// When the card last moved down.
    last_bump_ms: i64,
    /// Someone deleted the card.
    card_gone: bool,
    /// The card is out of date (a hint revealed a sharper tag).
    dirty: bool,
    /// A round just ended: the task says how and puts the next one up.
    ended: Option<i64>,
    /// A mod asked for a new round, with no hint needed first.
    admin_skip: Option<u64>,
    /// When the bot last answered a `!hint` that was already out, or a `!skip`
    /// that was too soon.
    last_nag_ms: i64,
    /// The break card as it was last drawn, so a countdown that has not changed
    /// in words does not cost an edit every second.
    break_shown: String,
    /// The match just scored, shown on the break card that follows it.
    last_result: Option<MatchResult>,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(|| Mutex::new(Shared::default()));

/// Every message in the game's channel, the bot's own included. Only other
/// people's messages count towards burying the card.
pub fn note_message(ctx: &Context, msg: &Message) {
    if Some(msg.channel_id.get()) != channel_setting() {
        return;
    }
    let mine = msg.author.id == ctx.cache.current_user().id;
    let mut s = SHARED.lock();
    if !mine && s.card.is_some_and(|(_, card)| msg.id.get() > card) {
        s.others_since_card += 1;
    }
}

/// A deleted card is posted again.
pub fn on_delete(channel: ChannelId, id: MessageId) {
    if Some(channel.get()) != channel_setting() {
        return;
    }
    let mut s = SHARED.lock();
    if s.card.map(|(_, m)| m) == Some(id.get()) {
        s.card_gone = true;
    }
}

// --- Discord helpers ---------------------------------------------------------------------------

async fn call<T>(fut: impl std::future::Future<Output = serenity::Result<T>>) -> Result<T, serenity::Error> {
    match tokio::time::timeout(HTTP_WAIT, fut).await {
        Ok(result) => result,
        Err(_) => Err(serenity::Error::Other("no answer from Discord in time")),
    }
}

fn is_gone(err: &serenity::Error) -> bool {
    match err {
        serenity::Error::Http(serenity::http::HttpError::UnsuccessfulRequest(r)) => r.status_code.as_u16() == 404 || r.error.code == 10008,
        _ => false,
    }
}

async fn exists(ctx: &Context, channel: u64, id: u64) -> bool {
    match call(ctx.http.get_message(ChannelId::new(channel), MessageId::new(id))).await {
        Ok(_) => true,
        Err(err) => !is_gone(&err),
    }
}

fn is_safe_corner(ctx: &Context, channel: u64) -> bool {
    if channel == super::weekly::SAFE_CORNER {
        return true;
    }
    let name = ctx
        .cache
        .guilds()
        .iter()
        .find_map(|g| ctx.cache.guild(*g).and_then(|guild| guild.channels.get(&ChannelId::new(channel)).map(|c| c.name.to_lowercase())));
    name.is_some_and(|n| n.contains("safe-corner"))
}

fn active_channel(ctx: &Context) -> Option<u64> {
    live_channel().filter(|c| !is_safe_corner(ctx, *c))
}

/// Takes a card away. A delete that fails is remembered rather than shrugged
/// off: two cards in the channel would be two rounds as far as anyone reading
/// can tell.
async fn delete(ctx: &Context, channel: u64, message: u64) {
    match call(ChannelId::new(channel).delete_message(&ctx.http, MessageId::new(message))).await {
        Ok(()) => {}
        Err(err) if is_gone(&err) => {}
        Err(err) => {
            tracing::warn!("movie: card {} not deleted ({}) — trying again", message, err);
            let mut s = SHARED.lock();
            if !s.orphans.contains(&(channel, message)) {
                s.orphans.push((channel, message));
            }
        }
    }
}

/// Another go at the cards a delete failed on.
async fn clear_orphans(ctx: &Context) {
    let waiting = std::mem::take(&mut SHARED.lock().orphans);
    for (channel, message) in waiting {
        delete(ctx, channel, message).await;
    }
}

/// A plain line in the channel, mentions never pinging anybody.
async fn say(ctx: &Context, channel: u64, text: String) {
    let message = CreateMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        tracing::warn!("movie: line not posted in {}: {}", channel, err);
    }
}

fn meta_set(key: &str, value: &str) {
    if let Some(db) = store::db() {
        let _ = store::meta_set(&db.lock(), key, value);
    }
}

/// The house crest and name of whoever won.
fn badge_of(user: u64) -> String {
    if super::house::opted_out(user) {
        return "🧙 Muggle".to_string();
    }
    super::house::house_of(user).map(|h| format!("{} {}", h.crest, h.name)).unwrap_or_default()
}

/// Mods are in no house, but they may still play; the ledger simply refuses
/// their points.
fn may_play(user: u64) -> bool {
    if super::house::opted_out(user) {
        return false;
    }
    super::house::house_of(user).is_some() || super::admin_ids().contains(&user)
}

/// Claims the right to say one of the small refusals out loud, if it is far
/// enough behind the last one.
fn take_nag() -> bool {
    let now = Utc::now().timestamp_millis();
    let mut s = SHARED.lock();
    if !nag_due(s.last_nag_ms, now) {
        return false;
    }
    s.last_nag_ms = now;
    true
}

fn with_db<T>(f: impl FnOnce(&rusqlite::Connection) -> T) -> Option<T> {
    store::db().map(|db| f(&db.lock()))
}

/// The match the channel is in, opening a fresh break when there is none.
fn match_now(channel: u64, now: i64) -> Option<store::Match> {
    let live = with_db(store::live_match).flatten();
    match live {
        Some(m) if m.channel == channel => Some(m),
        // The game moved channels: the old match is abandoned rather than
        // carried into a room it was not played in.
        Some(stale) => {
            let _ = with_db(|c| store::end_match(c, stale.id, now));
            None
        }
        None => None,
    }
    .or_else(|| {
        let from = now + break_minutes() * 60;
        let m = with_db(|c| store::open_match(c, match_films(), channel, now, from))?.ok()?;
        tracing::info!("movie: match {} open - {} films, break until {}", m.id, m.films, from);
        Some(m)
    })
}

fn ready_now(match_id: i64) -> Vec<u64> {
    with_db(|c| store::ready_list(c, match_id)).unwrap_or_default()
}

fn live_row() -> Option<store::Row> {
    store::db().and_then(|db| store::live(&db.lock()))
}

fn day_solves_now() -> Vec<store::Solve> {
    match store::db() {
        Some(db) => store::day_solves(&db.lock(), &super::points::ist_day(Utc::now().timestamp())),
        None => Vec::new(),
    }
}

/// The first and last day of the India month a day falls in.
pub fn month_ends(day: &str) -> (String, String) {
    let month = day.get(..7).unwrap_or(day);
    (format!("{}-01", month), format!("{}-31", month))
}

/// Today's and this month's movie points, both boards ranked.
fn boards_now() -> (Vec<store::Tally>, Vec<store::Tally>) {
    let day = super::points::ist_day(Utc::now().timestamp());
    let (from, to) = month_ends(&day);
    match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::day_tally(&conn, &day), store::tally_between(&conn, &from, &to))
        }
        None => (Vec::new(), Vec::new()),
    }
}

/// The film a round is about, out of the bank. `None` when the bank has been
/// changed under a live round, which the game reads as a round it can no longer
/// judge.
fn film_of(row: &store::Row) -> Option<&'static Movie> {
    let bank = bank::bank()?;
    bank.movie(bank.find(&row.movie_key)?)
}

// --- the card -------------------------------------------------------------------------------------

/// The tags a round is showing. Only a tags round has any: what a hint reveals
/// goes on its own line ([`Live::revealed`]), because on a stills round it is a
/// whole sentence and not a tag at all.
pub fn tags_of(row: &store::Row, film: Option<&Movie>) -> Vec<String> {
    let Some(film) = film else { return Vec::new() };
    match row.clue {
        Clue::Tags => film.shown_tags(row.tags_shown.max(1) as usize),
        _ => Vec::new(),
    }
}

/// The stills the card is showing: the one the round put up, and the second one
/// a hint added beside it. Empty for any round that isn't a stills round.
pub fn shots_of<'a>(row: &store::Row, film: Option<&'a Movie>) -> Vec<&'a Shot> {
    let Some(film) = film.filter(|_| row.clue == Clue::Shot) else { return Vec::new() };
    let mut out = Vec::new();
    if let Some(first) = film.shot(row.clue_index as usize) {
        out.push(first);
    }
    if let Some(second) = row.hint_shot.filter(|i| *i != row.clue_index).and_then(|i| film.shot(i as usize)) {
        out.push(second);
    }
    out
}

/// How tall the stills are drawn, and the gap between two of them.
const PICTURE_HEIGHT: u32 = 720;
const PICTURE_GAP: u32 = 10;
/// What a round's picture is called when it goes up.
fn picture_name(round: i64, shots: usize) -> String {
    format!("movie-{}-{}.jpg", round, shots)
}

/// One picture for the card: a still on its own, or two side by side once a
/// hint has put the second one up.
///
/// A single still goes up exactly as it is - re-encoding a photograph to say
/// nothing new about it only costs quality - and two are scaled to a common
/// height and set beside each other on a dark strip, the way the doodle game
/// puts a second drawing next to the first. Decoding happens off the async
/// runtime: these are full size frames, not thumbnails.
fn compose(paths: &[std::path::PathBuf]) -> Option<Vec<u8>> {
    match paths {
        [] => None,
        [only] => std::fs::read(only).ok(),
        many => {
            let mut scaled = Vec::with_capacity(many.len());
            for path in many {
                let picture = match image::ImageReader::open(path).ok()?.decode() {
                    Ok(picture) => picture,
                    Err(err) => {
                        tracing::warn!("movie: {} wouldn't decode: {}", path.display(), err);
                        return None;
                    }
                };
                let width = (picture.width() as f32 * PICTURE_HEIGHT as f32 / picture.height().max(1) as f32).round() as u32;
                scaled.push(picture.resize_exact(width.max(1), PICTURE_HEIGHT, image::imageops::FilterType::Triangle));
            }
            let total: u32 = scaled.iter().map(image::GenericImageView::width).sum::<u32>() + PICTURE_GAP * (scaled.len() as u32 - 1);
            let mut sheet = image::RgbImage::from_pixel(total, PICTURE_HEIGHT, image::Rgb([24, 24, 24]));
            let mut at = 0;
            for picture in &scaled {
                image::imageops::overlay(&mut sheet, &picture.to_rgb8(), at as i64, 0);
                at += picture.width() + PICTURE_GAP;
            }
            let mut out = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut std::io::Cursor::new(&mut out), 88)
                .encode_image(&sheet)
                .ok()?;
            Some(out)
        }
    }
}

/// The picture for a round as it stands, with its file name. `None` when the
/// round has no still of ours to show — a TMDB one is linked by the embed
/// instead, and a tags or dialogue round has no picture at all.
async fn card_picture(row: &store::Row) -> Option<(CreateAttachment, usize)> {
    let bank = bank::bank()?;
    let shots = shots_of(row, film_of(row));
    let paths: Vec<std::path::PathBuf> = shots.iter().filter_map(|shot| bank.shot_path(shot)).collect();
    if paths.is_empty() {
        return None;
    }
    let count = paths.len();
    let name = picture_name(row.id, count);
    let bytes = tokio::task::spawn_blocking(move || compose(&paths)).await.ok()??;
    Some((CreateAttachment::bytes(bytes, name), count))
}

fn live_of(row: &store::Row, film: Option<&Movie>, now: i64) -> Live {
    Live {
        clue: row.clue,
        tags: tags_of(row, film),
        line: match row.clue {
            Clue::Hint => film.and_then(|f| f.hint(row.clue_index as usize)),
            Clue::Dialogue => film.and_then(|f| f.dialogue(row.clue_index as usize)),
            _ => None,
        }
        .map(str::to_string),
        revealed: row.hint_clue.clone(),
        points: worth_after_hint(row.points, row.hinted()),
        hint: row.hint_by.map(|by| {
            let where_from = film.map(|f| placing(f.year, f.industry.label())).unwrap_or_default();
            (by, row.first_letter(), where_from)
        }),
        language: film.map(|f| f.language().to_string()).unwrap_or_default(),
        open_secs: now - row.posted_ts,
    }
}

/// `shots` is how many of our own stills are going up with the card, which is
/// what the attachment is named after. Nought means either a TMDB still, which
/// the embed links instead, or no picture at all.
fn card_embed(row: &store::Row, shots: usize, now: i64) -> CreateEmbed {
    let film = film_of(row);
    let mut embed = CreateEmbed::new()
        .title(card_title(row.id))
        .description(card_text(&live_of(row, film, now)))
        .colour(COLOUR)
        .footer(CreateEmbedFooter::new(CARD_FOOTER));
    if row.clue == Clue::Shot {
        if shots > 0 {
            embed = embed.image(format!("attachment://{}", picture_name(row.id, shots)));
        } else if let Some(url) = shots_of(row, film).first().and_then(|shot| shot.url()) {
            // One of TMDB's: linked and never copied, so nothing is downloaded
            // here and nothing is stored but the path the bank was built with.
            embed = embed.image(url);
        }
    }
    embed
}

/// Builds the message for a live round, picture and all.
async fn card_message(row: &store::Row, now: i64) -> CreateMessage {
    let picture = card_picture(row).await;
    let shots = picture.as_ref().map(|(_, n)| *n).unwrap_or(0);
    let mut message = CreateMessage::new().embed(card_embed(row, shots, now)).allowed_mentions(CreateAllowedMentions::new());
    if let Some((file, _)) = picture {
        message = message.add_file(file);
    }
    message
}

/// Posts a card and remembers it. `replace` takes the old card away;
/// `moved` says this is the card coming DOWN to the bottom rather than a fresh
/// round starting, which is the only thing the bump cooldown paces.
async fn place_card(ctx: &Context, channel: u64, message: CreateMessage, replace: bool, moved: bool) -> Option<u64> {
    match call(ChannelId::new(channel).send_message(&ctx.http, message)).await {
        Ok(posted) => {
            let id = posted.id.get();
            let old = {
                let mut s = SHARED.lock();
                s.dirty = false;
                s.card_gone = false;
                s.others_since_card = 0;
                if moved {
                    s.last_bump_ms = Utc::now().timestamp_millis();
                }
                s.card.replace((channel, id))
            };
            meta_set("card", &format!("{}:{}", channel, id));
            if let Some((c, m)) = old.filter(|(_, m)| *m != id && replace) {
                delete(ctx, c, m).await;
            }
            Some(id)
        }
        Err(err) => {
            tracing::warn!("movie: card not posted in {}: {}", channel, err);
            None
        }
    }
}

async fn drop_card(ctx: &Context) {
    let card = SHARED.lock().card.take();
    meta_set("card", "");
    if let Some((c, m)) = card {
        delete(ctx, c, m).await;
    }
}

/// Takes away the card of the round that has just ended - and ONLY that one. A
/// guess closes its round in the store straight away, so the next round's card
/// can already be up by the time the win is announced; a blind delete would
/// then take the NEW card away and leave the channel with nothing.
async fn drop_card_of(ctx: &Context, ended: Option<u64>) {
    let card = {
        let mut s = SHARED.lock();
        match card_to_drop(s.card, ended) {
            Some(card) => {
                s.card = None;
                Some(card)
            }
            None => None,
        }
    };
    if let Some((c, m)) = card {
        meta_set("card", "");
        delete(ctx, c, m).await;
    }
}

/// Which card an ending round may take away: its own, and only if that is still
/// the card the channel is showing.
pub fn card_to_drop(current: Option<(u64, u64)>, ended: Option<u64>) -> Option<(u64, u64)> {
    match (current, ended) {
        (Some((channel, card)), Some(ended)) if card == ended => Some((channel, card)),
        _ => None,
    }
}

/// Sets a fresh round and puts its card up.
async fn post_round(ctx: &Context, channel: u64, match_id: i64) -> Option<store::Row> {
    let bank = bank::bank()?;
    let now = Utc::now().timestamp();
    let db = store::db()?;
    let pool = with_db(|c| store::get_match(c, match_id)).flatten().map(|m| m.pool).unwrap_or_default();
    let row = {
        let conn = db.lock();
        let since = now - no_repeat_days() * 86_400;
        let mut rng = Rng::fresh();
        let used = store::movies_since(&conn, since);
        let which = bank.pick_in(pool, &used, hindi_share(), modern_share(), &mut rng)?;
        let film = bank.movie(which)?;
        let key = film.key();
        // Not only a film nobody has had lately, but a clue nobody has seen:
        // the same still twice is a round somebody simply remembers.
        let (clue, index) = bank.pick_clue(which, &store::clues_since(&conn, &key, since), &mut rng)?;
        match store::add_round(&conn, &film.title, &key, clue, index as i64, tags_shown(), round_points(), channel, now) {
            Ok(row) => row,
            Err(err) => {
                tracing::warn!("movie: round not saved: {}", err);
                return None;
            }
        }
    };
    // The round belongs to the match before its card goes up, so a crash
    // between the two can never leave a film nobody's score counts.
    let _ = with_db(|c| store::claim_round(c, match_id, row.id));
    let posted = place_card(ctx, channel, card_message(&row, now).await, false, false).await?;
    if let Some(db) = store::db() {
        let _ = store::set_message(&db.lock(), row.id, posted);
    }
    tracing::info!("movie: round {} up in {} ({}, {} {}, worth {})", row.id, channel, row.title, row.clue.key(), row.clue_index, row.points);
    Some(store::Row { message: Some(posted), ..row })
}

/// Redraws the live card where it is: a hint changes both the words and, on a
/// stills round, the picture, so the old attachment goes with them.
async fn edit_card(ctx: &Context, row: &store::Row, now: i64) {
    let Some((channel, message)) = SHARED.lock().card else { return };
    let picture = card_picture(row).await;
    let shots = picture.as_ref().map(|(_, n)| *n).unwrap_or(0);
    let mut edit = EditMessage::new().embed(card_embed(row, shots, now)).allowed_mentions(CreateAllowedMentions::new());
    if let Some((file, _)) = picture {
        edit = edit.attachments(EditAttachments::new().add(file));
    }
    if let Err(err) = call(ChannelId::new(channel).edit_message(&ctx.http, MessageId::new(message), edit)).await {
        if is_gone(&err) {
            SHARED.lock().card_gone = true;
        }
        tracing::warn!("movie: card not edited: {}", err);
    }
}

/// Whether chat has buried the card: enough messages from other people have
/// landed under it AND the card hasn't already moved in the last little while.
pub fn bump_due(others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> bool {
    others >= needed.max(1) && since_bump_ms >= every_secs.max(0) * 1_000
}

/// What to do with the card this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardAction {
    Nothing,
    /// Redraw it where it is (a hint changed what it shows).
    Edit,
    /// Post it again at the bottom and take the old one away.
    Bump,
}

/// The ONE decision behind the card, so nothing else in the game posts one.
#[allow(clippy::too_many_arguments)]
pub fn card_plan(has_card: bool, right_channel: bool, gone: bool, dirty: bool, others: u64, since_bump_ms: i64, needed: u64, every_secs: i64) -> CardAction {
    if !has_card || !right_channel || gone {
        return CardAction::Bump;
    }
    if bump_due(others, since_bump_ms, needed, every_secs) {
        return CardAction::Bump;
    }
    if dirty {
        return CardAction::Edit;
    }
    CardAction::Nothing
}

/// Keeps the channel's one card where people can see it.
async fn tend_card(ctx: &Context, channel: u64, row: &store::Row, now: i64) -> (CardAction, Option<u64>) {
    let (card, plan) = {
        let s = SHARED.lock();
        let since_bump = Utc::now().timestamp_millis() - s.last_bump_ms;
        let right = s.card.map(|(c, _)| c) == Some(channel);
        (s.card, card_plan(s.card.is_some(), right, s.card_gone, s.dirty, s.others_since_card, since_bump, bump_messages(), bump_seconds()))
    };
    match plan {
        CardAction::Nothing => (CardAction::Nothing, None),
        CardAction::Edit => {
            edit_card(ctx, row, now).await;
            SHARED.lock().dirty = false;
            (CardAction::Edit, None)
        }
        CardAction::Bump => {
            let gone = SHARED.lock().card_gone;
            let replace = card.is_some() && !gone;
            let posted = place_card(ctx, channel, card_message(row, now).await, replace, true).await;
            if let (Some(id), Some(db)) = (posted, store::db()) {
                let _ = store::set_message(&db.lock(), row.id, id);
            }
            (CardAction::Bump, posted)
        }
    }
}

// --- the ready button -------------------------------------------------------------------------

async fn whisper(ctx: &Context, component: &ComponentInteraction, text: impl Into<String>) {
    let message =
        CreateInteractionResponseMessage::new().content(text.into()).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    if let Err(err) = component.create_response(&ctx.http, CreateInteractionResponse::Message(message)).await {
        tracing::debug!("movie: reply to {} not sent: {}", component.user.id, err);
    }
}

/// `🎬 I'm ready`. Pressing it again takes the name off, so somebody who has to
/// go is not holding a match up.
pub async fn on_component(ctx: &Context, component: &ComponentInteraction) {
    if let Some(rest) = component.data.custom_id.strip_prefix(POOL_ID) {
        return on_pool_vote(ctx, component, rest).await;
    }
    let Some(match_id) = component.data.custom_id.strip_prefix(READY_ID).and_then(|v| v.parse::<i64>().ok()) else {
        return;
    };
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(m) = with_db(|c| store::get_match(c, match_id)).flatten() else {
        return whisper(ctx, component, "That match has moved on.").await;
    };
    if m.status != store::MatchStatus::Break {
        // Joining late needs no button: naming a film is joining.
        return whisper(ctx, component, "That match is already running — just type a title, you're in.").await;
    }
    let held = with_db(|c| store::is_ready(c, match_id, user)).unwrap_or(false);
    let text = if held {
        let _ = with_db(|c| store::unready(c, match_id, user));
        "Taken off the list.".to_string()
    } else {
        let _ = with_db(|c| store::mark_ready(c, match_id, user, now));
        let waiting = ready_now(match_id).len();
        let min = min_players();
        match waiting >= min {
            true => "🎬 You're in — it starts as soon as the break is up.".to_string(),
            false => format!("🎬 You're in. {} more to start.", min - waiting),
        }
    };
    SHARED.lock().break_shown.clear();
    whisper(ctx, component, text).await;
}

// --- what the next match plays ----------------------------------------------------------------

/// The corners worth voting for: Mix always, and any the bank can actually
/// fill. A room that votes for Hindi shows should get Hindi shows, so a corner
/// with nothing in it is not on the card at all.
pub fn pools_on_offer() -> Vec<Pool> {
    let Some(bank) = bank::bank() else { return vec![Pool::Mix] };
    Pool::ALL.into_iter().filter(|p| *p == Pool::Mix || bank.pool_count(*p) >= POOL_MINIMUM).collect()
}

/// How many titles a corner needs before the room may vote for it: a match is
/// ten rounds, and a corner thinner than that would repeat itself.
const POOL_MINIMUM: usize = 12;

/// Which corner won: most votes, a tie broken at random, the mix when nobody
/// voted at all. Given the tally rather than reading it, so the whole rule is
/// one function with nothing to mock.
pub fn winning_pool(tally: &[(Pool, usize)], roll: usize) -> Pool {
    let top = tally.iter().map(|(_, n)| *n).max().unwrap_or(0);
    if top == 0 {
        return Pool::Mix;
    }
    let tied: Vec<Pool> = tally.iter().filter(|(_, n)| *n == top).map(|(p, _)| *p).collect();
    tied[roll % tied.len()]
}

/// The buttons, each with its tally, the leader highlighted - the quiz's vote
/// card, in a game that already had a break to hold it.
fn pool_buttons(match_id: i64, tally: &[(Pool, usize)]) -> Vec<CreateActionRow> {
    let count = |pool: Pool| tally.iter().find(|(p, _)| *p == pool).map(|(_, n)| *n).unwrap_or(0);
    let offered = pools_on_offer();
    let top = offered.iter().map(|p| count(*p)).max().unwrap_or(0);
    let buttons: Vec<CreateButton> = offered
        .iter()
        .map(|pool| {
            let n = count(*pool);
            CreateButton::new(format!("{}{}:{}", POOL_ID, match_id, pool.key()))
                .label(if n > 0 { format!("{} · {}", pool.label(), n) } else { pool.label().to_string() })
                .style(if n > 0 && n == top { ButtonStyle::Primary } else { ButtonStyle::Secondary })
        })
        .collect();
    buttons.chunks(5).map(|row| CreateActionRow::Buttons(row.to_vec())).collect()
}

/// What the break card says about the vote as it stands.
pub fn vote_line(tally: &[(Pool, usize)]) -> String {
    let votes: usize = tally.iter().map(|(_, n)| n).sum();
    if votes == 0 {
        return "🗳️ **Vote for what the next match plays** — nobody has yet, so it's a mix of everything.".to_string();
    }
    let said: Vec<String> = tally.iter().filter(|(_, n)| *n > 0).map(|(p, n)| format!("{} {}", p.label(), n)).collect();
    format!("🗳️ **The vote so far:** {} · most wins, a tie is settled at random", said.join(" · "))
}

/// `moviepool:` — one press, one vote, and the same button again takes it back.
async fn on_pool_vote(ctx: &Context, component: &ComponentInteraction, rest: &str) {
    let Some((id, key)) = rest.split_once(':') else { return };
    let Some(match_id) = id.parse::<i64>().ok() else { return };
    let pool = Pool::from_key(key);
    let user = component.user.id.get();
    let now = Utc::now().timestamp();
    let Some(m) = with_db(|c| store::get_match(c, match_id)).flatten() else {
        return whisper(ctx, component, "That match has moved on.").await;
    };
    if m.status != store::MatchStatus::Break {
        return whisper(ctx, component, format!("That match is already running — it's playing {}.", m.pool.about())).await;
    }
    let kept = with_db(|c| store::cast_vote(c, match_id, user, pool, now)).and_then(Result::ok).unwrap_or(false);
    SHARED.lock().break_shown.clear();
    let text = if kept {
        format!("🗳️ Voted for **{}**. Press it again to take it back.", pool.label())
    } else {
        "🗳️ Vote taken back.".to_string()
    };
    whisper(ctx, component, text).await;
}

// --- the break, and scoring a match -----------------------------------------------------------

const BREAK_FOOTER: &str = "Press I'm ready · vote for what it plays · a match is 10 rounds · /moviehelp";

/// What the break card says and what it carries, worked out once.
///
/// Both the first posting and every refresh after it come through here. They
/// used to build their own: the refresh hand-rolled an embed and a single
/// I'm-ready button, so the moment a countdown ticked it quietly replaced the
/// card with one that had no vote on it at all.
fn break_body(m: &store::Match, ready: &[u64], last: Option<&MatchResult>, now: i64) -> (String, Vec<CreateActionRow>) {
    let tally = with_db(|c| store::vote_tally(c, m.id)).unwrap_or_default();
    let votes: usize = tally.iter().map(|(_, n)| n).sum();
    let mut text = break_text(ready, min_players(), m.films, votes, now, m.ready_from, last);
    text.push_str(&format!("\n\n{}", vote_line(&tally)));
    let mut rows = vec![CreateActionRow::Buttons(vec![
        CreateButton::new(format!("{}{}", READY_ID, m.id)).label("🎬 I'm ready").style(ButtonStyle::Success),
    ])];
    rows.extend(pool_buttons(m.id, &tally));
    (text, rows)
}

fn break_embed(text: &str) -> CreateEmbed {
    CreateEmbed::new().description(text).colour(COLOUR).footer(CreateEmbedFooter::new(BREAK_FOOTER))
}

fn break_message(m: &store::Match, ready: &[u64], last: Option<&MatchResult>, now: i64) -> CreateMessage {
    let (text, rows) = break_body(m, ready, last, now);
    CreateMessage::new().embed(break_embed(&text)).components(rows).allowed_mentions(CreateAllowedMentions::new())
}

/// Keeps the break card up. It is only redrawn when its WORDS change, so a
/// countdown that still reads "2 min" costs nothing.
async fn tend_break_card(ctx: &Context, channel: u64, m: &store::Match, now: i64) {
    let ready = ready_now(m.id);
    let last = SHARED.lock().last_result.clone();
    let (text, rows) = break_body(m, &ready, last.as_ref(), now);
    let (card, shown) = {
        let s = SHARED.lock();
        (s.card, s.break_shown.clone())
    };
    let right = card.map(|(c, _)| c) == Some(channel);
    if card.is_some() && right && shown == text && !SHARED.lock().card_gone {
        return;
    }
    let message = break_message(m, &ready, last.as_ref(), now);
    if card.is_some() && right && !SHARED.lock().card_gone {
        if let Some((c, id)) = card {
            let edit = EditMessage::new()
                .embed(break_embed(&text))
                .components(rows.clone())
                .allowed_mentions(CreateAllowedMentions::new());
            match call(ChannelId::new(c).edit_message(&ctx.http, MessageId::new(id), edit)).await {
                Ok(_) => {
                    SHARED.lock().break_shown = text;
                    return;
                }
                Err(err) => {
                    if is_gone(&err) {
                        SHARED.lock().card_gone = true;
                    }
                }
            }
        }
    }
    if place_card(ctx, channel, message, true, false).await.is_some() {
        SHARED.lock().break_shown = text;
    }
}

/// Scores a finished match and pays the prizes. Every payment carries a dedupe
/// key naming the match and the person, and the `prizes` row is written whether
/// the ledger paid or refused, so a retry can never pay twice.
async fn finish_match(ctx: &Context, channel: u64, m: &store::Match, now: i64) {
    if !with_db(|c| store::end_match(c, m.id, now)).and_then(Result::ok).unwrap_or(false) {
        return;
    }
    let scores = with_db(|c| store::match_scores(c, m.id)).unwrap_or_default();
    let mut places = Vec::new();
    for (user, place, points) in prize_table(&scores, win_points(), second_points()) {
        let films = scores.iter().find(|(u, _)| *u == user).map(|(_, n)| *n).unwrap_or(0);
        let paid = if may_play(user) {
            let reason = format!("Guess the Movie: match {}, place {}", m.id, place);
            let key = format!("{}match:{}:{}", LEDGER_KEY, m.id, user);
            match super::house::award_person(user, Source::Movie, points, &reason, None, Some(key), None) {
                Some((_, Outcome::Granted(n))) => n,
                _ => 0,
            }
        } else {
            0
        };
        let _ = with_db(|c| store::add_prize(c, m.id, user, place, films, points, paid, now));
        places.push((user, films, place, paid));
    }
    let result = MatchResult { id: m.id, films: m.films, places };
    tracing::info!("movie: match {} scored - {:?}", m.id, result.places);
    say(ctx, channel, result_line(&result)).await;
    {
        let mut sh = SHARED.lock();
        sh.last_result = Some(result);
        sh.break_shown.clear();
    }
    drop_card(ctx).await;
}

// --- the task --------------------------------------------------------------------------------------

/// Starts the task once.
pub fn spawn(ctx: Context) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    match (switched_on(), channel_setting(), bank::bank().is_some()) {
        (true, Some(c), true) => tracing::info!("movie: playing in {}", c),
        (true, Some(_), false) => tracing::warn!("movie: on, but there is no film bank — the game stays off"),
        (true, None, _) => tracing::info!("movie: on, but VIZIER_MOVIE_CHANNEL isn't set"),
        (false, _, _) => tracing::info!("movie: VIZIER_MOVIE is off"),
    }
    tokio::spawn(run(ctx));
}

/// After a start: pick up the card the last run left, and the round it was for.
async fn recover(ctx: &Context) -> Option<store::Row> {
    let channel = active_channel(ctx);
    let db = store::db()?;
    let (old_card, live) = {
        let conn = db.lock();
        (store::meta_get(&conn, "card"), store::live(&conn))
    };
    let old_card = old_card.as_deref().and_then(|v| v.split_once(':')).and_then(|(c, m)| Some((c.parse::<u64>().ok()?, m.parse::<u64>().ok()?)));
    // What was said while the bot was away doesn't count towards burying the
    // card: the channel is read from where it stands now.
    if let Some(c) = channel {
        let read = call(ChannelId::new(c).messages(&ctx.http, GetMessages::new().limit(1))).await.is_ok();
        let mut s = SHARED.lock();
        s.others_since_card = 0;
        s.last_bump_ms = Utc::now().timestamp_millis();
        if !read {
            tracing::debug!("movie: couldn't read {} on waking", c);
        }
    }
    match live {
        Some(row) if channel == Some(row.channel) => {
            let still_there = match row.message {
                Some(id) => exists(ctx, row.channel, id).await,
                None => false,
            };
            if still_there {
                SHARED.lock().card = row.message.map(|m| (row.channel, m));
                tracing::info!("movie: round {} picked up where it was left", row.id);
                return Some(row);
            }
            if let Some((c, m)) = old_card.filter(|(_, m)| Some(*m) != row.message) {
                delete(ctx, c, m).await;
            }
            SHARED.lock().card = None;
            Some(row)
        }
        other => {
            if let Some((c, m)) = old_card {
                delete(ctx, c, m).await;
            }
            SHARED.lock().card = None;
            other
        }
    }
}

async fn run(ctx: Context) {
    let mut live = recover(&ctx).await;
    loop {
        tokio::time::sleep(TICK).await;
        let now = Utc::now().timestamp();
        let channel = active_channel(&ctx);
        let (ended, admin_skip) = {
            let mut s = SHARED.lock();
            (s.ended.take(), s.admin_skip.take())
        };
        clear_orphans(&ctx).await;
        let Some(channel) = channel else {
            if SHARED.lock().card.is_some() {
                drop_card(&ctx).await;
            }
            live = None;
            continue;
        };

        // The match is the frame every round sits in: no match, no films. A
        // break is not a pause in a match - it IS the match, before it starts.
        let Some(m) = match_now(channel, now) else { continue };
        if m.status == store::MatchStatus::Break {
            if live.is_some() {
                drop_card(&ctx).await;
                live = None;
            }
            let ready = ready_now(m.id);
            let cast: usize = with_db(|c| store::vote_tally(c, m.id)).unwrap_or_default().iter().map(|(_, n)| n).sum();
            if may_start(ready.len(), min_players(), cast, now, m.ready_from) {
                if with_db(|c| store::start_match(c, m.id, now)).and_then(Result::ok).unwrap_or(false) {
                    // The vote is settled the moment the match starts, and
                    // written down: every round of it then asks the same
                    // corner, and a restart mid-match carries on in it.
                    let tally = with_db(|c| store::vote_tally(c, m.id)).unwrap_or_default();
                    let pool = winning_pool(&tally, Rng::fresh().below(64));
                    let _ = with_db(|c| store::set_pool(c, m.id, pool));
                    tracing::info!(
                        "movie: match {} started - {} films, {} ready, playing {}",
                        m.id,
                        m.films,
                        ready.len(),
                        pool.key()
                    );
                    SHARED.lock().break_shown.clear();
                    drop_card(&ctx).await;
                    let votes: usize = tally.iter().map(|(_, n)| n).sum();
                    let said = match (pool, votes) {
                        (Pool::Mix, 0) => "🎬 **Match on!** Nobody voted, so it's a mix of everything.".to_string(),
                        (pool, n) => format!(
                            "🎬 **Match on!** {} wins the vote with **{}** — this match is {}.",
                            pool.label(),
                            plural(n as i64, "vote", "votes"),
                            pool.about()
                        ),
                    };
                    let _ = crate::utils::discord::send_message(ctx.http.clone(), &ChannelId::new(channel), said).await;
                }
                continue;
            }
            tend_break_card(&ctx, channel, &m, now).await;
            continue;
        }
        // The last film of the match has been settled: score it and open the
        // next break.
        if m.played >= m.films && live.is_none() {
            finish_match(&ctx, channel, &m, now).await;
            continue;
        }

        // A mod pressed on: the round is closed here, and told about below like
        // any other ending.
        let mut ending = ended;
        if let (Some(by), Some(row)) = (admin_skip, live.as_ref()) {
            let done = store::db().map(|db| store::skip(&db.lock(), row.id, by, now).unwrap_or(false)).unwrap_or(false);
            if done {
                tracing::info!("movie: round {} skipped by mod {}", row.id, by);
                ending = Some(row.id);
            }
        }
        // Nobody guessed and nobody skipped: the bot moves on by itself. A round
        // whose film the bank no longer has goes the same way - it can be
        // neither shown nor judged.
        if ending.is_none() {
            let unknown = |r: &store::Row| bank::bank().is_some_and(|b| b.find(&r.movie_key).is_none());
            if let Some(row) = live.as_ref().filter(|r| stale(r.posted_ts, now, idle_minutes()) || unknown(r)) {
                let done = store::db().map(|db| store::expire(&db.lock(), row.id, now).unwrap_or(false)).unwrap_or(false);
                if done {
                    match unknown(row) {
                        true => tracing::warn!("movie: round {} closed — the bank no longer has {}", row.id, row.title),
                        false => tracing::info!("movie: round {} went stale after {} min", row.id, idle_minutes()),
                    }
                    ending = Some(row.id);
                }
            }
        }

        if let Some(id) = ending {
            let row = store::db().and_then(|db| store::get(&db.lock(), id));
            let ended_card = row.as_ref().and_then(|r| r.message);
            if let Some(row) = row.filter(|r| r.status != Status::Open) {
                announce_end(&ctx, channel, &row).await;
            }
            drop_card_of(&ctx, ended_card).await;
            live = None;
        }

        // The store is the truth: a round claimed by a guess is no longer live.
        if let Some(row) = &live {
            let fresh = store::db().and_then(|db| store::get(&db.lock(), row.id));
            match fresh {
                Some(f) if f.status == Status::Open && f.channel == channel => live = Some(f),
                _ => live = None,
            }
        }
        if live.is_none() {
            let open = store::db().and_then(|db| store::live(&db.lock()));
            let existing = match open {
                Some(row) if row.channel == channel => Some(row),
                Some(stale) => {
                    if let Some(db) = store::db() {
                        let _ = store::expire(&db.lock(), stale.id, now);
                    }
                    tracing::info!("movie: round {} closed — the game moved to {}", stale.id, channel);
                    None
                }
                None => None,
            };
            live = match existing {
                Some(row) => {
                    // Taking a round over, its card is only ours to keep if it
                    // is still in the channel.
                    let there = match row.message {
                        Some(id) => exists(&ctx, channel, id).await,
                        None => false,
                    };
                    let mut s = SHARED.lock();
                    s.card = row.message.filter(|_| there).map(|m| (channel, m));
                    s.card_gone = !there;
                    s.others_since_card = 0;
                    s.last_bump_ms = Utc::now().timestamp_millis();
                    drop(s);
                    Some(row)
                }
                None if m.played < m.films => post_round(&ctx, channel, m.id).await,
                None => None,
            };
        }
        let Some(row) = live.clone() else { continue };

        // One place decides what happens to the card, so a bump and a fresh
        // round can never both post one.
        let (_, posted) = tend_card(&ctx, channel, &row, now).await;
        if let Some(id) = posted {
            live = Some(store::Row { message: Some(id), ..row.clone() });
        }
    }
}

/// Says how a round ended: won, skipped or left to go stale.
async fn announce_end(ctx: &Context, channel: u64, row: &store::Row) {
    let film = film_of(row);
    if row.status == Status::Solved {
        let Some(winner) = row.winner else { return };
        let solves = day_solves_now();
        let mine = solves.iter().find(|s| s.round == row.id && s.user == winner);
        let paid = mine.map(|s| s.points).unwrap_or(0);
        let worth = mine.map(|s| s.worth).unwrap_or_else(|| worth_after_hint(row.points, row.hinted()));
        let won = Won {
            round: row.id,
            winner,
            badge: badge_of(winner),
            guess: row.winning_guess.clone().unwrap_or_else(|| row.title.clone()),
            title: row.title.clone(),
            year: film.map(|f| f.year).unwrap_or(0),
            points: paid,
            worth,
            full_points: row.points,
            housed: super::house::house_of(winner).is_some(),
            tally: solves.iter().filter(|s| s.user == winner).map(|s| s.worth).sum(),
            hinted: row.hinted(),
            seconds: row.seconds.unwrap_or(0),
        };
        say(ctx, channel, won_text(&won)).await;
        return;
    }
    say(ctx, channel, ended_text(row, film.map(|f| f.year))).await;
}

// --- what people type ------------------------------------------------------------------------

/// Every human message in the game's channel. A right guess wins the round, a
/// `!hint` or a `!skip` steers it, and everything else is left alone.
pub async fn on_message(ctx: &Context, msg: &Message) {
    let Some(channel) = live_channel() else { return };
    if msg.channel_id.get() != channel || msg.author.bot {
        return;
    }
    let Some(bank) = bank::bank() else { return };
    let Some(row) = live_row().filter(|r| r.status == Status::Open) else { return };
    let Some(movie) = bank.find(&row.movie_key) else { return };
    match read_message(&msg.content, movie, bank) {
        Typed::Hint => hint_asked(ctx, msg, &row, movie, channel).await,
        Typed::Skip => skip_asked(ctx, msg, &row, channel).await,
        Typed::Guess(guess) => guessed(ctx, msg, &row, &guess).await,
        Typed::Nothing => {}
    }
}

/// A guess that names the film. Only the first one counts; the rest are told
/// nothing, because they can see the winner's line for themselves.
async fn guessed(ctx: &Context, msg: &Message, row: &store::Row, guess: &str) {
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let won = {
        let conn = db.lock();
        store::claim(&conn, row.id, user, guess, now).unwrap_or(false)
    };
    if !won {
        return;
    }
    if let Err(err) = call(msg.react(&ctx.http, ReactionType::Unicode(TICK_MARK.to_string()))).await {
        tracing::debug!("movie: tick not added to {}: {}", msg.id, err);
    }
    let seconds = now - row.posted_ts;
    let worth = worth_after_hint(row.points, row.hinted());
    // A film pays NO house points on its own any more. House points are the
    // match prize - see `finish_match` - so twenty films cannot out-pay winning
    // the thing, and the daily limit means what it says. Movie points are
    // unchanged: per film, uncapped, everyone.
    let granted = 0;
    // What the round was worth goes down for EVERY winner, cap or no cap, house
    // or no house: the house points are the ledger's business, the movie points
    // are the game's.
    {
        let conn = db.lock();
        let _ = store::add_solve(&conn, &super::points::ist_day(now), user, row.id, granted, worth, guess, seconds, now);
    }
    tracing::info!("movie: round {} won by {} with {:?} in {}s for {} house points ({} movie points)", row.id, user, guess, seconds, granted, worth);
    SHARED.lock().ended = Some(row.id);
}

/// `!hint`: once a round. The next tag down goes onto the card - the sharper one
/// it held back - and the first letter, the year and the industry are said out
/// loud. Who asked is written down: it costs the round a point.
async fn hint_asked(ctx: &Context, msg: &Message, row: &store::Row, movie: usize, channel: u64) {
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let Some(bank) = bank::bank() else { return };
    let film = bank.movie(movie);
    // A tags round walks one step down its own list. A stills round puts the
    // film's OTHER still up beside the first and says the line written for it,
    // which is the clue that still was given. Anything else has shown no tags at
    // all, so the vaguest one would be no help: it gets the sharpest tag the
    // film has, the one written to fit almost nothing else.
    let second = match row.clue {
        Clue::Shot => film.and_then(|f| (0..f.shots.len()).find(|i| *i as i64 != row.clue_index)),
        _ => None,
    };
    let sharper = match row.clue {
        Clue::Tags => film.and_then(|f| f.next_tag(row.tags_shown.max(1) as usize)).map(str::to_string),
        // A still with no line of its own still counts as a hint; the sharpest
        // tag comes out instead so the round always gives something.
        Clue::Shot => second
            .and_then(|i| film.and_then(|f| f.shot(i)).and_then(Shot::hint))
            .or_else(|| film.and_then(Movie::sharpest_tag))
            .map(str::to_string),
        Clue::Hint | Clue::Dialogue => film.and_then(Movie::sharpest_tag).map(str::to_string),
    };
    let taken = {
        let conn = db.lock();
        store::take_hint(&conn, row.id, user, sharper.as_deref(), second.map(|i| i as i64), now).unwrap_or(false)
    };
    if !taken {
        if !take_nag() {
            return;
        }
        let by = row.hint_by.map(|by| format!(" <@{}> asked for it", by)).unwrap_or_default();
        return say(
            ctx,
            channel,
            format!("💡 The hint is already out for round #{}:{} it starts with **{}**. `!skip` moves on.", row.id, by, row.first_letter()),
        )
        .await;
    }
    SHARED.lock().dirty = true;
    let worth = worth_after_hint(row.points, true);
    let where_from = film.map(|f| placing(f.year, f.industry.label())).unwrap_or_default();
    tracing::info!("movie: hint on round {} to {} (worth {} now)", row.id, user, worth);
    say(ctx, channel, hint_text(row, user, worth, sharper.as_deref(), second.is_some(), &where_from)).await;
}

/// `!skip`: only after a hint, and it pays nothing. The next film goes up at
/// once.
async fn skip_asked(ctx: &Context, msg: &Message, row: &store::Row, channel: u64) {
    if !row.hinted() {
        if take_nag() {
            say(ctx, channel, SKIP_TOO_SOON.to_string()).await;
        }
        return;
    }
    let user = msg.author.id.get();
    let now = Utc::now().timestamp();
    let Some(db) = store::db() else { return };
    let skipped = {
        let conn = db.lock();
        store::skip(&conn, row.id, user, now).unwrap_or(false)
    };
    if !skipped {
        return;
    }
    tracing::info!("movie: round {} skipped by {}", row.id, user);
    SHARED.lock().ended = Some(row.id);
}

// --- commands -------------------------------------------------------------------------------

pub fn mine_builder() -> CreateCommand {
    CreateCommand::new("movie").description("the film that's up now, what it's worth and how you've done today")
}

pub fn top_builder() -> CreateCommand {
    CreateCommand::new("movietop")
        .description("the movie points board, today or this month")
        .add_option(
            serenity::all::CreateCommandOption::new(serenity::all::CommandOptionType::String, "period", "which days to count")
                .add_string_choice("Today", "today")
                .add_string_choice("This month", "month"),
        )
}

pub fn help_builder() -> CreateCommand {
    CreateCommand::new("moviehelp").description("how Guess the Movie works, from the settings as they are now")
}

pub fn skip_builder() -> CreateCommand {
    CreateCommand::new("movieskip").description("admin only: drop the film that's up and put a fresh one up, no points")
}

pub fn reload_builder() -> CreateCommand {
    CreateCommand::new("moviereload").description("admin only: read the film bank off disk again, without restarting")
}

pub fn stop_builder() -> CreateCommand {
    CreateCommand::new("moviestop").description("admin only: switch Guess the Movie off and take the card down")
}

async fn reply(ctx: &Context, command: &CommandInteraction, message: CreateInteractionResponseMessage) {
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(message.ephemeral(true))).await {
        tracing::warn!("movie: reply to {} not sent: {}", command.user.id, err);
    }
}

const OFF: &str = "Guess the Movie is switched off right now.";

/// `/movie` — everyone, shown only to them, the clue with it.
pub async fn mine_command(ctx: &Context, command: &CommandInteraction) {
    let live = live_row();
    let (today, month) = boards_now();
    let text = mine_text(live.as_ref(), &today, &month, command.user.id.get(), live_channel());
    let mut message = CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new());
    if let Some(row) = live.as_ref().filter(|r| r.status == Status::Open) {
        let picture = card_picture(row).await;
        let shots = picture.as_ref().map(|(_, n)| *n).unwrap_or(0);
        message = message.embed(card_embed(row, shots, Utc::now().timestamp()));
        if let Some((file, _)) = picture {
            message = message.add_file(file);
        }
    }
    reply(ctx, command, message).await;
}

/// `/movietop [period]` — everyone, shown only to them.
pub async fn top_command(ctx: &Context, command: &CommandInteraction) {
    let month = command.data.options.iter().any(|o| {
        o.name == "period" && matches!(&o.value, serenity::all::CommandDataOptionValue::String(v) if v == "month")
    });
    let (today_rows, month_rows) = boards_now();
    let (rows, period) = if month {
        (month_rows, month_label(&super::points::ist_day(Utc::now().timestamp())))
    } else {
        (today_rows, "today".to_string())
    };
    let text = top_text(&period, &rows, command.user.id.get());
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text).allowed_mentions(CreateAllowedMentions::new())).await;
}

/// "September so far", the heading a month's board carries.
pub fn month_label(day: &str) -> String {
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let month = day.get(5..7).and_then(|m| m.parse::<usize>().ok()).filter(|m| (1..=12).contains(m));
    match month {
        Some(m) => format!("{} so far", MONTHS[m - 1]),
        None => "this month".to_string(),
    }
}

/// The rules as a card, the same words for the command and anything else that
/// wants them.
fn help_embed() -> CreateEmbed {
    CreateEmbed::new().title(rules_text::MOVIE_RULES_TITLE).description(rules_text::movie_help_text(&movie_rules())).colour(COLOUR)
}

/// `/moviehelp` — everyone.
pub async fn help_command(ctx: &Context, command: &CommandInteraction) {
    reply(ctx, command, CreateInteractionResponseMessage::new().embed(help_embed())).await;
}

/// `/movieskip` — admins only.
pub async fn skip_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can skip a round.")).await;
    }
    if live_channel().is_none() {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content(OFF)).await;
    }
    let live = live_row().map(|r| r.id);
    SHARED.lock().admin_skip = Some(user);
    tracing::info!("movie: /movieskip by {} (round {:?})", user, live);
    let text = match live {
        Some(id) => format!("⏭️ Round #{} dropped. A fresh film is on its way — nobody scores for that one.", id),
        None => "⏭️ A fresh film is on its way.".to_string(),
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

/// `/moviereload` — admins only. A new pack of films is a file that lands in
/// the bank folder; until this existed, the only way to see it was a restart.
///
/// The round that is up is left exactly as it is. It was picked from the old
/// bank and is answered from the store, so nothing about it depends on what
/// this reads; the next round comes from whatever is now in play.
pub async fn reload_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can reload the bank.")).await;
    }
    let before = bank::bank().map(|b| (b.count(), b.shot_count()));
    let text = match bank::reload() {
        Ok((films, stills)) => {
            tracing::info!("movie: /moviereload by {} — {} titles, {} stills", user, films, stills);
            let was = before.map(|(f, s)| format!(" (was {} and {})", f, s)).unwrap_or_default();
            format!(
                "📚 Bank read again: **{}** titles and **{}** stills{}.\n-# The round that's up is untouched; the next one comes from this.",
                films, stills, was
            )
        }
        Err(err) => format!("Couldn't read the bank: {}\n-# Nothing changed — the bank that was in play still is.", err),
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

/// `/moviestop` — admins only. Switches the game off, which takes the card down
/// and stops new rounds; the round that was up is left where it is.
pub async fn stop_command(ctx: &Context, command: &CommandInteraction) {
    let user = command.user.id.get();
    if !super::admin_ids().contains(&user) {
        return reply(ctx, command, CreateInteractionResponseMessage::new().content("Only mods can stop the game.")).await;
    }
    let text = match control::set("VIZIER_MOVIE", Some("off"), user) {
        Ok(()) => {
            tracing::info!("movie: /moviestop by {}", user);
            "🛑 Guess the Movie is off. The card comes down in a moment; switch **Game on** back on in the panel to play again."
        }
        Err(err) => {
            tracing::warn!("movie: /moviestop by {} failed: {}", user, err);
            "The setting wouldn't save — switch **Game on** off in the panel instead."
        }
    };
    reply(ctx, command, CreateInteractionResponseMessage::new().content(text)).await;
}

/// The last few rounds, for a summary or a panel page later on.
pub fn recent_rounds(limit: usize) -> Vec<store::Row> {
    store::db().map(|db| store::recent(&db.lock(), limit.min(RECENT))).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_match_pays_first_and_second_and_nobody_else() {
        let scores = vec![(7, 5), (8, 3), (9, 1)];
        assert_eq!(prize_table(&scores, 5, 2), vec![(7, 1, 5), (8, 2, 2)]);
    }

    /// A tie for first pays BOTH the winner's share and skips second: splitting
    /// it would make a draw worth less than a win for no visible reason.
    #[test]
    fn a_tie_for_first_pays_both_and_skips_second() {
        let scores = vec![(7, 4), (8, 4), (9, 2)];
        assert_eq!(prize_table(&scores, 5, 2), vec![(7, 1, 5), (8, 1, 5)]);
    }

    #[test]
    fn a_tie_for_second_pays_them_all() {
        let scores = vec![(7, 6), (8, 2), (9, 2)];
        assert_eq!(prize_table(&scores, 5, 2), vec![(7, 1, 5), (8, 2, 2), (9, 2, 2)]);
    }

    /// Nobody named anything, or only one person played: no prize is invented.
    #[test]
    fn an_empty_match_pays_nothing() {
        assert!(prize_table(&[], 5, 2).is_empty());
        assert!(prize_table(&[(7, 0)], 5, 2).is_empty());
        assert_eq!(prize_table(&[(7, 3)], 5, 2), vec![(7, 1, 5)], "one player still wins what they won");
    }

    /// All three, never two: a full lobby still serves the break out, a
    /// served-out break still waits for people, and neither starts a match
    /// nobody has chosen a category for.
    #[test]
    fn a_match_needs_the_people_the_vote_and_the_clock() {
        assert!(!may_start(1, 2, 1, 100, 50), "not enough people");
        assert!(!may_start(2, 2, 1, 40, 50), "break not served");
        assert!(!may_start(9, 2, 0, 999, 50), "nobody voted");
        assert!(may_start(2, 2, 1, 50, 50), "all three met, on the second");
        assert!(may_start(9, 2, 4, 999, 50));
        assert!(may_start(1, 0, 1, 100, 50), "a nonsense minimum still starts");
    }

    #[test]
    fn the_break_card_says_what_is_missing() {
        assert!(break_text(&[], 2, 10, 1, 0, 0, None).contains("Nobody's ready"));
        assert!(break_text(&[7], 2, 10, 1, 0, 0, None).contains("1 more"));
        assert!(break_text(&[7, 8], 2, 10, 1, 0, 0, None).contains("Starting now"));
        assert!(break_text(&[7, 8], 2, 10, 1, 0, 90, None).contains("2 min"));
        assert!(break_text(&[7, 8], 2, 10, 1, 0, 30, None).contains("30s"));
        assert!(break_text(&[], 2, 10, 1, 0, 0, None).contains("10 films"));
        // Everyone ready and nobody voted: it says which of the two is missing,
        // and does not claim to be starting.
        let unvoted = break_text(&[7, 8], 2, 10, 0, 0, 0, None);
        assert!(unvoted.contains("Waiting on the vote"), "{}", unvoted);
        assert!(!unvoted.contains("Starting now"), "{}", unvoted);
    }

    #[test]
    fn the_result_line_names_the_places() {
        let r = MatchResult { id: 4, films: 10, places: vec![(7, 6, 1, 5), (8, 3, 2, 2)] };
        let line = result_line(&r);
        assert!(line.contains("Match #4"));
        assert!(line.contains("🥇 <@7> **+5** (6 films)"));
        assert!(line.contains("🥈 <@8> **+2** (3 films)"));
        // A capped-out winner still won; the line says +0 rather than lying.
        assert!(result_line(&MatchResult { id: 5, films: 10, places: vec![(7, 6, 1, 0)] }).contains("**+0**"));
        assert!(result_line(&MatchResult { id: 6, films: 10, places: vec![] }).contains("nobody on the board"));
    }

    #[test]
    fn the_scoreboard_shows_the_top_three() {
        let line = scoreboard_line(&[(7, 4), (8, 2), (9, 1), (10, 1)], 7, 10);
        assert!(line.contains("match 7/10"));
        assert!(line.contains("🥇 <@7> 4") && line.contains("🥉 <@9> 1"));
        assert!(!line.contains("<@10>"), "only three fit");
    }
    use crate::channels::discord::movie_bank::tests::fixture;
    use crate::channels::discord::movie_store::tests::{memory, put};

    /// Anagrams' round 16: round 17 went up, then round 16's win finished and
    /// took the new card away with it, leaving the channel with no picture at
    /// all. This game was born knowing better.
    #[test]
    fn an_ending_round_only_ever_takes_away_its_own_card() {
        let showing = Some((99, 1_000));
        assert_eq!(card_to_drop(showing, Some(1_000)), Some((99, 1_000)), "its own card, still up");
        assert_eq!(card_to_drop(showing, Some(900)), None, "the next round's card is already up");
        assert_eq!(card_to_drop(showing, None), None, "a round whose card was never written down guesses at nothing");
        assert_eq!(card_to_drop(None, Some(1_000)), None);
    }

    #[test]
    fn a_hint_takes_a_point_off_but_a_round_is_never_worth_nothing() {
        assert_eq!(worth_after_hint(3, false), 3);
        assert_eq!(worth_after_hint(3, true), 2);
        assert_eq!(worth_after_hint(1, true), 1, "a hinted round is still worth playing");
        assert_eq!(worth_after_hint(0, false), 0, "a game set to pay nothing pays nothing");
    }

    fn live(clue: Clue) -> Live {
        Live {
            clue,
            tags: vec!["revenge".into(), "guns".into(), "coal mafia".into()],
            line: Some("Keh ke lunga".into()),
            revealed: None,
            points: 3,
            hint: None,
            language: "Hindi".into(),
            open_secs: 130,
        }
    }

    #[test]
    fn every_drawing_of_the_break_card_carries_the_vote() {
        // The refresh used to hand-roll its own embed and one button, so a
        // countdown tick replaced the card with a voteless copy. Both drawings
        // come from break_body now, and this is what says so.
        let rows = pool_buttons(7, &[(Pool::HindiFilms, 1)]);
        assert!(!rows.is_empty(), "the vote has no buttons at all");
        // Six corners fit in two rows of five; whatever the bank can fill, the
        // mix is always one of them.
        assert!(rows.len() <= 2, "{} rows of buttons", rows.len());
        assert!(pools_on_offer().contains(&Pool::Mix));
    }

    #[test]
    fn the_vote_takes_the_most_and_settles_a_tie_at_random() {
        // Nobody voted: the match plays everything, which is what it always did.
        assert_eq!(winning_pool(&[], 0), Pool::Mix);
        assert_eq!(winning_pool(&[(Pool::HindiShows, 0)], 0), Pool::Mix);
        // A clear winner is the winner however the roll falls.
        let clear = [(Pool::HindiFilms, 3), (Pool::EnglishShows, 1)];
        for roll in 0..8 {
            assert_eq!(winning_pool(&clear, roll), Pool::HindiFilms);
        }
        // Two on the same count: both are reachable, and neither by anything
        // but the roll.
        let tied = [(Pool::HindiShows, 2), (Pool::EnglishFilms, 2), (Pool::Mix, 1)];
        assert_eq!(winning_pool(&tied, 0), Pool::HindiShows);
        assert_eq!(winning_pool(&tied, 1), Pool::EnglishFilms);
        assert_eq!(winning_pool(&tied, 2), Pool::HindiShows);
    }

    #[test]
    fn the_break_card_says_where_the_vote_stands() {
        assert!(vote_line(&[]).contains("nobody has yet"), "{}", vote_line(&[]));
        let line = vote_line(&[(Pool::HindiShows, 2), (Pool::Mix, 1)]);
        assert!(line.contains("📺 Hindi shows 2") && line.contains("🎲 Mix 1"), "{}", line);
        // A corner nobody picked is not listed as nought.
        assert!(!vote_line(&[(Pool::HindiShows, 2), (Pool::EnglishFilms, 0)]).contains("English films"));
    }

    #[test]
    fn the_card_asks_the_question_its_clue_asks() {
        // Tags: the words are the whole clue.
        let text = card_text(&live(Clue::Tags));
        assert!(text.contains("# Name the film or show"), "{}", text);
        assert!(text.contains("🗣️ **Hindi**"), "every card says which language the answer is in: {}", text);
        assert!(text.contains("🏷️ **revenge · guns · coal mafia**"), "{}", text);
        assert!(!text.contains("Keh ke lunga"), "a tags round never shows the line too");
        assert!(text.contains("worth **3 points**") && text.contains("up 2 min"), "{}", text);
        // A line, quoted, and no tags until a hint adds one.
        let mut dialogue = live(Clue::Dialogue);
        dialogue.tags.clear();
        let text = card_text(&dialogue);
        assert!(text.contains("# Which film or show is this line from?") && text.contains("> *Keh ke lunga*"), "{}", text);
        assert!(!text.contains("🏷️"), "{}", text);
        // A still asks its question and lets the picture do the rest.
        let mut shot = live(Clue::Shot);
        shot.tags.clear();
        let text = card_text(&shot);
        assert!(text.contains("# Which film or show is this from?") && !text.contains("🏷️"), "{}", text);
        // A hint says who asked, the letter and where the film is from, and the
        // card is worth a point less by then.
        let hinted = Live { points: 2, hint: Some((77, 'G', "Hindi, 2012".into())), ..live(Clue::Tags) };
        let text = card_text(&hinted);
        assert!(text.contains("it starts with **G** · Hindi, 2012"), "{}", text);
        assert!(text.contains("<@77> asked") && text.contains("worth **2 points**"), "{}", text);
    }

    #[test]
    fn a_hint_reveals_words_on_their_own_line_and_a_second_still_beside_the_first() {
        let conn = memory();
        let bank = fixture();
        let film = bank.movie(bank.find("dilvaledulhanialejaienge").expect("a film")).expect("a film");
        // A tags round shows its five. What the hint reveals goes on its own
        // line rather than being smuggled in among them.
        let mut row = put(&conn, "Dilwale Dulhania Le Jayenge", Clue::Tags, 0, 3, 100);
        assert_eq!(tags_of(&row, Some(film)).len(), 5);
        assert!(live_of(&row, Some(film), 200).revealed.is_none());
        row.hint_clue = Some("the last sharp one".into());
        assert_eq!(tags_of(&row, Some(film)).len(), 5, "the tags are unchanged");
        assert_eq!(live_of(&row, Some(film), 200).revealed.as_deref(), Some("the last sharp one"));
        assert!(card_text(&live_of(&row, Some(film), 200)).contains("💡 **the last sharp one**"));
        // A stills round shows one picture, and two once the hint has put the
        // film's other still up beside it.
        let mut shot = put(&conn, "Dilwale Dulhania Le Jayenge", Clue::Shot, 1, 3, 200);
        assert_eq!(shots_of(&shot, Some(film)).len(), 1);
        assert!(tags_of(&shot, Some(film)).is_empty(), "a stills round has no tags of its own");
        shot.hint_shot = Some(0);
        shot.hint_clue = Some("A field of yellow flowers.".into());
        let both = shots_of(&shot, Some(film));
        assert_eq!(both.len(), 2, "both stills are on the card now");
        assert_eq!(both[0].path, "/two.jpg", "the round's own still stays first");
        assert_eq!(both[1].path, "/one.jpg");
        // The same still twice is one still: a hint that found nothing new
        // never doubles the picture.
        shot.hint_shot = Some(1);
        assert_eq!(shots_of(&shot, Some(film)).len(), 1);
        // A round whose film the bank has lost shows nothing rather than guessing.
        assert!(tags_of(&row, None).is_empty() && shots_of(&shot, None).is_empty());
    }

    fn won() -> Won {
        Won {
            round: 12,
            winner: 5,
            badge: "🦁 Gryffindor".into(),
            guess: "gangs of wasseypur".into(),
            title: "Gangs of Wasseypur".into(),
            year: 2012,
            points: 3,
            worth: 3,
            full_points: 3,
            housed: true,
            tally: 9,
            hinted: false,
            seconds: 41,
        }
    }

    #[test]
    fn the_winner_line_names_the_film_and_the_points() {
        let text = won_text(&won());
        assert!(text.contains("✅ <@5> 🦁 Gryffindor had it: **Gangs of Wasseypur** (2012)"), "{}", text);
        assert!(text.contains("**+3**") && text.contains("9 movie points today"), "{}", text);
        assert!(text.contains("round #12 in 41 s"), "{}", text);
        assert!(!text.contains("they typed"), "a guess that matches the title isn't quoted back");
        // A title typed another way is quoted, so the room sees what counted.
        let text = won_text(&Won { guess: "gow".into(), ..won() });
        assert!(text.contains("they typed gow"), "{}", text);
        // A hinted round says what it cost.
        let text = won_text(&Won { hinted: true, points: 2, worth: 2, ..won() });
        assert!(text.contains("a hint was out, so 2 instead of 3"), "{}", text);
    }

    #[test]
    fn a_solve_the_ledger_cant_pay_for_still_reads_as_a_win() {
        // The day's house points are full: the round is still won, and the movie
        // points are still the winner's.
        let capped = won_text(&Won { points: 0, ..won() });
        assert!(capped.contains("✅") && capped.contains("**+3 movie points**"), "{}", capped);
        assert!(capped.contains("that's your house points for today, but the movie points still count"), "{}", capped);
        // And somebody with no house at all is told the same thing its own way.
        let mod_win = won_text(&Won { points: 0, housed: false, badge: String::new(), ..won() });
        assert!(mod_win.contains("no house to pay, but the movie points still count"), "{}", mod_win);
        assert!(!mod_win.contains("  "), "an empty badge leaves no double space: {}", mod_win);
    }

    #[test]
    fn a_round_that_nobody_gets_says_what_it_was() {
        let conn = memory();
        let row = put(&conn, "Andhadhun", Clue::Tags, 0, 3, 100);
        // Left to go stale by the bot.
        let stale = store::Row { status: Status::Expired, ..row.clone() };
        let text = ended_text(&stale, Some(2018));
        assert!(text.contains("Nobody had round #") && text.contains("**Andhadhun** (2018)"), "{}", text);
        // Skipped by a player, who is named.
        let skipped = store::Row { status: Status::Skipped, ended_by: Some(9), ..row.clone() };
        let text = ended_text(&skipped, Some(2018));
        assert!(text.contains("<@9> passed on round #") && text.contains("No points for that one"), "{}", text);
        // Skipped by a mod through the command, where nobody is named.
        let by_mod = store::Row { status: Status::Skipped, ended_by: None, ..row };
        assert!(ended_text(&by_mod, None).contains("skipped — it was **Andhadhun**."));
    }

    #[test]
    fn only_a_guess_that_names_the_film_is_even_looked_at() {
        let bank = fixture();
        let ddlj = bank.find("dilvaledulhanialejaienge").expect("a film");
        assert_eq!(read_message("!hint", ddlj, &bank), Typed::Hint);
        assert_eq!(read_message("  !SKIP ", ddlj, &bank), Typed::Skip);
        assert_eq!(read_message("ddlj", ddlj, &bank), Typed::Guess("ddlj".into()));
        assert_eq!(read_message("dilwaale dulhaniya le jaenge", ddlj, &bank), Typed::Guess("dilwaale dulhaniya le jaenge".into()));
        // Chatter, a wrong film and an empty line are all the same thing: nothing.
        for quiet in ["lol same", "the matrix", "", "🎬", "is it sholay?"] {
            assert_eq!(read_message(quiet, ddlj, &bank), Typed::Nothing, "{:?}", quiet);
        }
    }

    #[test]
    fn a_round_nobody_touches_is_replaced_by_the_bot_itself() {
        let posted = 1_000;
        assert!(!stale(posted, posted + 19 * 60, 20), "still inside the window");
        assert!(stale(posted, posted + 20 * 60, 20), "and out of it");
        // A setting of nought would be a round replaced the moment it went up.
        assert!(!stale(posted, posted + 30, 0));
    }

    fn tally(user: u64, points: i64, solves: i64, reached: i64) -> store::Tally {
        store::Tally { user, points, solves, reached }
    }

    #[test]
    fn the_movie_points_board_lists_ten_and_finds_the_asker_below_them() {
        let rows: Vec<store::Tally> = (1..=12).map(|n| tally(n, (20 - n) as i64, 2, n as i64 * 10)).collect();
        let text = top_text("today", &rows, 12);
        assert!(text.contains("🥇 <@1>") && text.contains("🥉 <@3>"), "{}", text);
        assert!(text.contains("`10.` <@10>"), "{}", text);
        assert!(!text.contains("<@11>"), "only ten are listed");
        assert!(text.contains("**You:** 12th of 12"), "the asker gets their own line: {}", text);
        // Somebody inside the ten is marked there rather than twice.
        let text = top_text("today", &rows, 2);
        assert!(text.contains("← you") && !text.contains("**You:**"), "{}", text);
        // An empty board says so instead of printing a heading over nothing.
        assert!(top_text("today", &[], 1).contains("Nobody has named one yet"));
        assert_eq!(standing(&rows, 99), None, "somebody who hasn't played has no line");
        assert_eq!(ordinal(1), "1st");
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(22), "22nd");
    }

    #[test]
    fn a_month_is_read_off_the_day_it_is_asked_on() {
        assert_eq!(month_ends("2026-09-20"), ("2026-09-01".to_string(), "2026-09-31".to_string()));
        assert_eq!(month_label("2026-09-20"), "September so far");
        assert_eq!(month_label("nonsense"), "this month");
    }

    #[test]
    fn the_card_follows_the_chat_down_only_once_chat_has_really_buried_it() {
        // Both halves have to be met: enough messages AND enough time.
        assert!(!bump_due(4, 999_000, 5, 120), "not enough has landed under it");
        assert!(!bump_due(9, 60_000, 5, 120), "it moved a moment ago");
        assert!(bump_due(5, 120_000, 5, 120));
    }

    #[test]
    fn one_card_at_a_time_whatever_the_channel_is_doing() {
        // No card, the wrong channel, or one somebody deleted: put one up.
        assert_eq!(card_plan(false, true, false, false, 0, 0, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, false, false, false, 0, 0, 5, 120), CardAction::Bump);
        assert_eq!(card_plan(true, true, true, false, 0, 0, 5, 120), CardAction::Bump);
        // Chat buried it: it follows the conversation down.
        assert_eq!(card_plan(true, true, false, false, 5, 200_000, 5, 120), CardAction::Bump);
        // A hint changed what it says: redraw it where it is, never a new one.
        assert_eq!(card_plan(true, true, false, true, 0, 0, 5, 120), CardAction::Edit);
        // And an untouched card in a quiet channel is left alone.
        assert_eq!(card_plan(true, true, false, false, 1, 200_000, 5, 120), CardAction::Nothing);
    }

    #[test]
    fn the_same_small_refusal_is_not_repeated_over_and_over() {
        assert!(nag_due(0, NAG_EVERY_MS), "far enough behind the last one");
        assert!(!nag_due(1_000, 1_000 + NAG_EVERY_MS - 1), "and not a moment sooner");
        assert!(SKIP_TOO_SOON.contains("!hint"), "the refusal says what to do instead");
    }

    #[test]
    fn with_no_film_bank_the_game_is_simply_off() {
        // Every way in checks the bank before it does anything, so a checkout
        // without one plays nothing rather than crashing.
        assert!(bank::bank().is_none(), "no bank is read in a test run");
        assert!(live_channel().is_none());
        assert!(movie_rules().films.is_none());
        assert!(live_row().is_none(), "and no store either");
        assert!(recent_rounds(5).is_empty());
    }

    /// The picture path, run against the real stills when they are there: two
    /// frames really do decode, scale and end up on one strip. A test that only
    /// checked the maths would have nothing to say about a corrupt jpeg.
    #[test]
    fn two_stills_are_composed_onto_one_strip_if_the_bank_is_there() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("moviebank");
        let Ok(bank) = bank::load(&dir) else { return };
        let Some(film) = (0..bank.count())
            .filter_map(|i| bank.movie(i))
            .find(|m| m.shots.len() >= 2 && m.shots.iter().all(|s| s.is_local()))
        else {
            return;
        };
        let paths: Vec<std::path::PathBuf> = film.shots.iter().take(2).filter_map(|s| bank.shot_path(s)).collect();
        assert_eq!(paths.len(), 2, "{} has two stills of ours", film.title);
        // One on its own goes up exactly as it is, bytes and all.
        let one = compose(&paths[..1]).expect("one still");
        assert_eq!(one, std::fs::read(&paths[0]).expect("the file"), "a single still is never re-encoded");
        // Two are drawn onto one strip: wider than tall, and a real jpeg.
        let both = compose(&paths).expect("two stills");
        let sheet = image::ImageReader::new(std::io::Cursor::new(&both))
            .with_guessed_format()
            .expect("a format")
            .decode()
            .expect("it decodes");
        assert_eq!(sheet.height(), PICTURE_HEIGHT);
        assert!(sheet.width() > sheet.height(), "two frames side by side are a wide picture");
        assert!(both.len() > 10_000, "and not an empty one");
        assert!(compose(&[]).is_none(), "no stills, no picture");
        assert_eq!(picture_name(7, 2), "movie-7-2.jpg");
    }

    #[test]
    fn how_long_things_took_reads_naturally() {
        assert_eq!(spent_words(41), "41 s");
        assert_eq!(spent_words(461), "7 min 41 s");
        assert_eq!(spent_words(3_840), "1 h 04 min");
        assert_eq!(spent_words(-5), "0 s");
        assert_eq!(open_words(30), "just now");
        assert_eq!(open_words(600), "10 min");
        assert_eq!(open_words(7_200), "2 h");
        assert_eq!(tag_line(&["a".into(), "b".into()]), "a · b");
        assert_eq!(placing(2012, "Hindi"), "Hindi, 2012");
    }
}

