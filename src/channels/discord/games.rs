//! House points from the game bots that already run on the server.
//!
//! Koto, Anagram Bot and Cat Bot keep their own scores; this module only
//! watches what they post and pays the winner's house. Nothing here writes
//! points itself: every award goes through `house::award_person`, which refuses
//! the unsorted and the opted out, and the ledger applies the daily caps. Each
//! award carries a dedupe key naming the exact event, so an edit arriving twice,
//! a restart or a replay can never score again.
//!
//! Koto names its players in the card. The other two only print a username, so
//! the winner is taken from the channel itself: the human message that landed
//! just before the bot's line. That is why every human message is noted here,
//! in every channel, before anything else gets to look at it.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

use parking_lot::Mutex;
use regex::Regex;
use serenity::all::{ChannelId, Context, Embed, Message, MessageId, MessageUpdateEvent};

use super::points::Source;

pub const KOTO_BOT: u64 = 1_164_654_805_730_472_018;
pub const ANAGRAM_BOT: u64 = 888_013_540_705_832_961;
pub const CAT_BOT: u64 = 966_695_034_340_663_367;

const DISCORD_EPOCH_MS: u64 = 1_420_070_400_000;
/// Human messages remembered per channel. Only the last few seconds before a
/// solve matter; this is plenty for a busy channel.
const RING: usize = 30;
/// The winning guess lands in the same second as the bot's line. Anything older
/// than this is a message the bot never answered - most likely the real winner
/// was sent while we were offline - so nobody is paid rather than the wrong person.
const FRESH_MS: u64 = 20_000;

const KOTO_WIN: i64 = 3;
const KOTO_PLAYED: i64 = 1;
const ANAGRAM_WIN: i64 = 3;

#[derive(Clone, Debug)]
struct Seen {
    id: u64,
    user: u64,
    /// Username and global name, lowercased, for the cross-check against the
    /// name a bot prints.
    names: Vec<String>,
    /// Short messages only, trimmed and lowercased: a guess is one word.
    text: String,
}

static HUMANS: LazyLock<Mutex<HashMap<u64, VecDeque<Seen>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// The open anagram per channel, as its sorted letters.
static PUZZLES: LazyLock<Mutex<HashMap<u64, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Koto games already paid this run, so every later edit of a finished card is
/// skipped without a fetch or a trip to the ledger.
static KOTO_DONE: LazyLock<Mutex<HashSet<u64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Where Koto is played; only used to log edits whose author didn't come along.
const KOTO_CHANNEL: u64 = 1_543_493_815_644_590_180;

fn clip(text: &str) -> String {
    text.chars().take(400).collect()
}

/// A Koto card's shape for the log, until its format is confirmed from a live game.
fn describe_embeds(embeds: &[Embed]) -> String {
    let parts: Vec<String> = embeds
        .iter()
        .map(|e| {
            format!(
                "[title={:?} desc={:?} fields={:?} footer={:?}]",
                e.title.as_deref().map(clip),
                e.description.as_deref().map(clip),
                e.fields.iter().map(|f| format!("{}={}", clip(&f.name), clip(&f.value))).collect::<Vec<_>>(),
                e.footer.as_ref().map(|f| clip(&f.text))
            )
        })
        .collect();
    format!("embeds={} {}", embeds.len(), parts.join(" "))
}

fn snowflake_ms(id: u64) -> u64 {
    (id >> 22) + DISCORD_EPOCH_MS
}

/// Every message the bot sees, in every channel. Notes humans, and spots the
/// Koto, Anagram and Cat Bot lines that pay out. Never awaits and never
/// consumes the message: anything slow is spawned.
pub fn on_message(_ctx: &Context, msg: &Message) {
    if msg.guild_id.is_none() {
        return;
    }
    if !msg.author.bot {
        note_human(msg);
        return;
    }
    match msg.author.id.get() {
        KOTO_BOT => {
            tracing::info!(
                "games: koto post {} content={:?} components={} {}",
                msg.id,
                clip(&msg.content),
                msg.components.len(),
                describe_embeds(&msg.embeds)
            );
            // Normally Koto edits its card to the win, but a card could arrive
            // already finished.
            if let Ok(win) = parse_koto(&msg.embeds) {
                pay_koto(msg.channel_id, win);
            }
        }
        ANAGRAM_BOT => on_anagram(msg),
        CAT_BOT => on_cat(msg),
        _ => {}
    }
}

/// Koto announces a win by editing its game card, so edits are where Koto is
/// watched. Returns at once; any fetch and the awards run in their own task.
pub fn on_message_update(ctx: &Context, event: &MessageUpdateEvent) {
    // Most edits are people fixing typos. Drop them before any work.
    let author = event
        .author
        .as_ref()
        .map(|a| a.id.get())
        .or_else(|| ctx.cache.message(event.channel_id, event.id).map(|m| m.author.id.get()));
    if author.is_some_and(|a| a != KOTO_BOT) {
        return;
    }
    // An update can come without its embeds. Without them and without an
    // author there is nothing to go on; a card that is Koto's is fetched.
    let embeds = event.embeds.clone();
    if author == Some(KOTO_BOT) || event.channel_id.get() == KOTO_CHANNEL {
        tracing::info!(
            "games: koto edit {} author={:?} content={:?} {}",
            event.id,
            author,
            event.content.as_deref().map(clip),
            embeds.as_deref().map_or_else(|| "embeds=absent".to_string(), describe_embeds)
        );
    }
    if embeds.is_none() && author.is_none() {
        return;
    }
    if let Some(embeds) = &embeds {
        match parse_koto(embeds) {
            // Koto keeps editing a finished card; one payout per game.
            Ok(win) if !KOTO_DONE.lock().contains(&win.game) => {}
            _ => return,
        }
    }
    let ctx = ctx.clone();
    let (channel, id) = (event.channel_id, event.id);
    tokio::spawn(async move {
        let known_koto = author == Some(KOTO_BOT);
        let embeds = match embeds {
            // The author is confirmed and the card came with the event.
            Some(embeds) if known_koto => embeds,
            // Either the embeds are missing, or a solved-looking card came with
            // no author - anyone's link preview could carry a title, so check.
            _ => match channel.message(&ctx.http, id).await {
                Ok(m) if m.author.id.get() == KOTO_BOT => m.embeds,
                Ok(_) => return,
                Err(err) => {
                    tracing::warn!("games: could not fetch Koto card {}: {}", id, err);
                    return;
                }
            },
        };
        match parse_koto(&embeds) {
            Ok(win) => pay_koto(channel, win),
            Err(KotoSkip::NoRows) | Err(KotoSkip::NoGame) => {
                tracing::warn!("games: Koto card {} says solved but could not be read", id)
            }
            Err(KotoSkip::NotSolved) => {}
        }
    });
}

// --- the human tracker ----------------------------------------------------------

fn note_human(msg: &Message) {
    let text = msg.content.trim();
    let seen = Seen {
        id: msg.id.get(),
        user: msg.author.id.get(),
        names: std::iter::once(msg.author.name.to_lowercase())
            .chain(msg.author.global_name.as_deref().map(str::to_lowercase))
            .collect(),
        text: if text.chars().count() <= 40 { text.to_lowercase() } else { String::new() },
    };
    let mut humans = HUMANS.lock();
    let ring = humans.entry(msg.channel_id.get()).or_default();
    ring.push_back(seen);
    while ring.len() > RING {
        ring.pop_front();
    }
}

fn recent_humans(channel: ChannelId) -> Vec<Seen> {
    HUMANS.lock().get(&channel.get()).map(|r| r.iter().cloned().collect()).unwrap_or_default()
}

/// Who a bot's line is about: the newest fresh human message before it,
/// preferring one that looks like the winning input (a reply to it, or text
/// that `looks_right` accepts), since chatter can squeeze in between.
fn pick_solver<'a>(
    seen: &'a [Seen],
    line: u64,
    replied_to: Option<u64>,
    looks_right: impl Fn(&Seen) -> bool,
) -> Option<&'a Seen> {
    if let Some(target) = replied_to {
        if let Some(s) = seen.iter().find(|s| s.id == target && s.id < line) {
            return Some(s);
        }
    }
    let at = snowflake_ms(line);
    let fresh: Vec<&Seen> =
        seen.iter().filter(|s| s.id < line && at.saturating_sub(snowflake_ms(s.id)) <= FRESH_MS).collect();
    fresh
        .iter()
        .copied()
        .filter(|s| looks_right(s))
        .max_by_key(|s| s.id)
        .or_else(|| fresh.iter().copied().max_by_key(|s| s.id))
}

/// Everything a bot said in a message, content and embeds alike, since either
/// may carry the line.
fn all_text(msg: &Message) -> String {
    let mut out = msg.content.clone();
    for e in &msg.embeds {
        for part in [e.title.as_deref(), e.description.as_deref()].into_iter().flatten() {
            out.push('\n');
            out.push_str(part);
        }
    }
    out
}

/// Bots escape markdown in usernames (`baco\_only.`).
fn unescape(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut chars = name.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek().is_some_and(|n| n.is_ascii_punctuation()) {
            continue;
        }
        out.push(c);
    }
    out
}

fn check_name(game: &str, printed: &str, solver: &Seen) {
    let printed = unescape(printed).trim().to_lowercase();
    if !printed.is_empty() && !solver.names.iter().any(|n| *n == printed) {
        tracing::warn!(
            "games: {} line names {:?} but the message before it was {} ({:?}) - paying them anyway",
            game,
            printed,
            solver.user,
            solver.names
        );
    }
}

fn replied_to(msg: &Message) -> Option<u64> {
    msg.message_reference.as_ref().and_then(|r| r.message_id).map(MessageId::get)
}

/// The ledger calls are quick but take a lock; keep them off the event path.
fn pay(user: u64, source: Source, points: i64, reason: String, dedupe: String) {
    tokio::task::spawn_blocking(move || {
        match super::house::award_person(user, source, points, &reason, None, Some(dedupe.clone()), None) {
            Some((house, outcome)) => {
                tracing::info!("games: {} -> {} ({}): {:?}", dedupe, user, house.name, outcome)
            }
            None => tracing::info!("games: {} -> {} earns nothing (unsorted or opted out)", dedupe, user),
        }
    });
}

// --- Koto -------------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct KotoWin {
    game: u64,
    winner: u64,
    /// Everyone else who guessed, each once.
    others: Vec<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum KotoSkip {
    NotSolved,
    NoGame,
    NoRows,
}

static KOTO_GAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bkoto\s*#\s*(\d+)").expect("regex"));
static MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@!?(\d+)>").expect("regex"));
/// A guess row ends in the guesser and Koto's score for it: `<@123> +4`, maybe
/// dressed in markdown. Custom emoji tiles are `<:name:id>` and never match.
static KOTO_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@!?(\d+)>[\s*_`]*\(?\+\d").expect("regex"));

fn parse_koto(embeds: &[Embed]) -> Result<KotoWin, KotoSkip> {
    let mut title = String::new();
    let mut blocks: Vec<&str> = Vec::new();
    for e in embeds {
        if let Some(t) = e.title.as_deref() {
            title.push_str(t);
            title.push('\n');
        }
        blocks.extend(e.description.as_deref());
        for f in &e.fields {
            blocks.push(&f.name);
            blocks.push(&f.value);
        }
        blocks.extend(e.footer.as_ref().map(|f| f.text.as_str()));
    }
    parse_koto_parts(&title, &blocks)
}

/// The card as plain text: its title, then every text block in display order
/// (description, field names and values, footer).
fn parse_koto_parts(title: &str, blocks: &[&str]) -> Result<KotoWin, KotoSkip> {
    let solved = std::iter::once(title).chain(blocks.iter().copied()).any(|b| b.to_lowercase().contains("good job!"));
    if !solved {
        return Err(KotoSkip::NotSolved);
    }
    let game = std::iter::once(title)
        .chain(blocks.iter().copied())
        .find_map(|b| KOTO_GAME.captures(b).and_then(|c| c[1].parse::<u64>().ok()))
        .ok_or(KotoSkip::NoGame)?;

    let lines: Vec<&str> = blocks.iter().flat_map(|b| b.lines()).collect();
    let mut rows: Vec<u64> =
        lines.iter().filter_map(|l| KOTO_ROW.captures_iter(l).last()).filter_map(|c| c[1].parse().ok()).collect();
    if rows.is_empty() {
        // Should Koto ever drop the score column, a line naming exactly one
        // person is still a guess row. Lines naming several are not rows.
        rows = lines
            .iter()
            .filter_map(|l| {
                let ids: Vec<u64> = MENTION.captures_iter(l).filter_map(|c| c[1].parse().ok()).collect();
                if ids.len() == 1 { Some(ids[0]) } else { None }
            })
            .collect();
    }
    let winner = *rows.last().ok_or(KotoSkip::NoRows)?;
    let mut others = Vec::new();
    for id in rows {
        if id != winner && !others.contains(&id) {
            others.push(id);
        }
    }
    Ok(KotoWin { game, winner, others })
}

fn pay_koto(channel: ChannelId, win: KotoWin) {
    if !KOTO_DONE.lock().insert(win.game) {
        return;
    }
    tracing::info!(
        "games: Koto #{} solved in {} by {} with {} other player(s)",
        win.game,
        channel,
        win.winner,
        win.others.len()
    );
    pay(win.winner, Source::Koto, KOTO_WIN, format!("solved Koto #{}", win.game), koto_key(win.game, win.winner));
    for &user in &win.others {
        pay(user, Source::Koto, KOTO_PLAYED, format!("played Koto #{}", win.game), koto_key(win.game, user));
    }
    let mut done = KOTO_DONE.lock();
    if done.len() > 1000 {
        // Old games stay safe after this: the ledger still refuses their keys.
        done.clear();
        done.insert(win.game);
    }
}

fn koto_key(game: u64, user: u64) -> String {
    format!("koto:{}:{}", game, user)
}

// --- Anagram Bot ------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum AnagramLine {
    Puzzle(String),
    /// The name the bot printed.
    Solved(String),
    TimedOut,
}

static ANAGRAM_PUZZLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)guess the word:\s*\*\*\s*([a-z]+)\s*\*\*").expect("regex"));
static ANAGRAM_SOLVED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\*\*(.+?)\*\*\s*was correct and has been awarded\s*\*\*\s*\d+\s*\*\*\s*points").expect("regex")
});
static ANAGRAM_TIMEOUT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)the correct answer was\s*\*\*").expect("regex"));

fn parse_anagram(text: &str) -> Option<AnagramLine> {
    if let Some(c) = ANAGRAM_SOLVED.captures(text) {
        return Some(AnagramLine::Solved(c[1].trim_end_matches('!').trim().to_string()));
    }
    if ANAGRAM_TIMEOUT.is_match(text) {
        return Some(AnagramLine::TimedOut);
    }
    ANAGRAM_PUZZLE.captures(text).map(|c| AnagramLine::Puzzle(sorted_letters(&c[1])))
}

fn sorted_letters(word: &str) -> String {
    let mut letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect();
    letters.sort_unstable();
    letters.into_iter().collect()
}

fn on_anagram(msg: &Message) {
    let channel = msg.channel_id.get();
    match parse_anagram(&all_text(msg)) {
        Some(AnagramLine::Puzzle(letters)) => {
            PUZZLES.lock().insert(channel, letters);
        }
        Some(AnagramLine::TimedOut) => {
            PUZZLES.lock().remove(&channel);
        }
        Some(AnagramLine::Solved(name)) => {
            let letters = PUZZLES.lock().remove(&channel);
            let seen = recent_humans(msg.channel_id);
            // A guess that spells the puzzle's letters is the strongest sign of
            // who won; without a known puzzle, the newest human it is.
            let spells = |s: &Seen| letters.as_deref().is_some_and(|l| !l.is_empty() && sorted_letters(&s.text) == l);
            let Some(solver) = pick_solver(&seen, msg.id.get(), replied_to(msg), spells) else {
                tracing::warn!("games: anagram solved in {} but no fresh human message before it", msg.channel_id);
                return;
            };
            check_name("anagram", &name, solver);
            pay(
                solver.user,
                Source::Anagram,
                ANAGRAM_WIN,
                "solved an anagram".to_string(),
                format!("anagram:{}", msg.id.get()),
            );
        }
        None => {}
    }
}

// --- Cat Bot ----------------------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
struct Catch {
    name: String,
    /// The cat type as printed, e.g. "Gremlin". Empty when it could not be read.
    kind: String,
}

/// `name cought <:emoji:id> Type cat!`. The type is the word or two right before
/// ` cat`, after the emoji - never any earlier word, which is how a naive read
/// once took "this" for a type.
static CAT_CATCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?im)^\s*(.+?)\s+cought\s+(<a?:\w+:\d+>\s*)?[*_]*([a-z0-9]+(?: [a-z0-9]+)?)[*_]*\s+cat\b")
        .expect("regex")
});

fn parse_cat(text: &str) -> Option<Catch> {
    let c = CAT_CATCH.captures(text)?;
    let name = unescape(c[1].trim());
    // Cat Bot also talks about catches in general ("anyone who cought ..."),
    // which names nobody; only a line naming the catcher pays.
    let lower = name.to_lowercase();
    let general = ["anyone", "everyone", "everybody", "nobody", "no one", "someone", "whoever", "you"];
    if general.iter().any(|g| lower == *g || lower.starts_with(&format!("{} ", g))) {
        return None;
    }
    let kind = c[3].to_string();
    // Without the emoji the words before "cat" are less certain; a filler word
    // is not a type, and an unknown type still pays the minimum.
    let filler = ["this", "that", "a", "an", "the", "one", "some"];
    let kind = if c.get(2).is_none() && filler.contains(&kind.to_lowercase().as_str()) { String::new() } else { kind };
    Some(Catch { name, kind })
}

fn cat_points(kind: &str) -> i64 {
    match kind.trim().to_lowercase().as_str() {
        "rare" | "sus" | "rickroll" | "wild" => 2,
        "superior" | "mythic" | "legendary" => 3,
        // Fine, Nice, Good, Gremlin, and anything new or unreadable.
        _ => 1,
    }
}

fn on_cat(msg: &Message) {
    let Some(catch) = parse_cat(&all_text(msg)) else { return };
    let seen = recent_humans(msg.channel_id);
    let typed_cat = |s: &Seen| s.text == "cat";
    let Some(solver) = pick_solver(&seen, msg.id.get(), replied_to(msg), typed_cat) else {
        tracing::warn!("games: cat caught in {} but no fresh human message before it", msg.channel_id);
        return;
    };
    check_name("cat", &catch.name, solver);
    let label = if catch.kind.is_empty() { "a cat".to_string() } else { format!("a {} cat", catch.kind) };
    let key = format!("cat:{}", msg.id.get());
    pay(solver.user, Source::Cat, cat_points(&catch.kind), format!("caught {}", label), key);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALICE: u64 = 412_345_678_901_234_567;
    const BOB: u64 = 498_765_432_109_876_543;
    const CARA: u64 = 455_555_555_555_555_555;

    /// A message id sent `ms` after a fixed moment.
    fn id_at(ms: u64) -> u64 {
        (1_700_000_000_000 + ms) << 22
    }

    fn seen(id: u64, user: u64, text: &str) -> Seen {
        Seen { id, user, names: vec![format!("user{}", user % 100)], text: text.to_lowercase() }
    }

    fn tiles(word: &str) -> String {
        word.chars().map(|c| format!("<:y_{}:11{}>", c.to_ascii_lowercase(), c as u32)).collect()
    }

    #[test]
    fn a_cat_line_about_anyone_names_no_catcher() {
        assert!(parse_cat("anyone who cought a Fine cat gets a bonus").is_none());
        assert!(parse_cat("Everyone cought <:fine:123> Fine cat!").is_none());
        assert!(parse_cat("youngster\\_07 cought <:fine:123> Fine cat!").is_some(), "a name that only starts with 'you' is a person");
    }

    #[test]
    fn koto_solved_by_one_player_pays_only_the_winner() {
        let desc = format!(
            "1 {} <@{a}> +1 (+2)\n2 {} <@{a}> +1\n3 {} <@{a}> +2\n4 {} <@{a}> +2\n5 {} <@{a}> +3\n6 {} <@{a}> +4\n\n\
             Good job! Everyone who participated gets +2 points!",
            tiles("QWERTY"),
            tiles("PLANET"),
            tiles("MARKED"),
            tiles("MAILER"),
            tiles("MAILES"),
            tiles("MAILED"),
            a = ALICE
        );
        let win = parse_koto_parts("Koto #887", &[&desc]).unwrap();
        assert_eq!(win, KotoWin { game: 887, winner: ALICE, others: vec![] });
    }

    #[test]
    fn koto_multiplayer_pays_the_last_guesser_and_everyone_else_once() {
        let desc = format!(
            "1 QWERTY <@{b}> +1 (+2)\n2 PLANET <@!{c}> +1\n3 MARKED <@{b}> +2\n\
             4 MAILER <@{a}> +2\n5 MAILED <@!{c}> +4\nGood job! Everyone who participated gets +2 points!",
            a = ALICE,
            b = BOB,
            c = CARA
        );
        let win = parse_koto_parts("Koto #12", &[&desc]).unwrap();
        assert_eq!(win.winner, CARA);
        assert_eq!(win.others, vec![BOB, ALICE]);
    }

    #[test]
    fn koto_rows_in_fields_are_read_in_order() {
        let rows = format!("1 {} <@{}> **+1**\n2 {} <@{}> **+4**", tiles("CRANES"), BOB, tiles("BARMAN"), ALICE);
        let blocks = ["", "Guesses", &rows, "Good job! Everyone who participated gets +2 points!"];
        let win = parse_koto_parts("Koto #5", &blocks).unwrap();
        assert_eq!(win, KotoWin { game: 5, winner: ALICE, others: vec![BOB] });
    }

    #[test]
    fn koto_times_up_and_nine_misses_pay_nothing() {
        let desc = format!("1 QWERTY <@{}> +1\nTime's up! The correct word was **BARMAN**!", ALICE);
        assert_eq!(parse_koto_parts("Koto #888", &[&desc]), Err(KotoSkip::NotSolved));
        let nine: String = (1..=9).map(|i| format!("{} WRONGS <@{}> +0\n", i, BOB)).collect();
        assert_eq!(parse_koto_parts("Koto #889", &[&nine]), Err(KotoSkip::NotSolved));
    }

    #[test]
    fn koto_custom_emoji_tiles_are_never_players() {
        // Emoji ids are long numbers too; only <@...> is a person.
        let desc = format!("1 <:g_a:123456789012345678><a:y_b:223456789012345678> <@{}> +4\nGood job!", BOB);
        let win = parse_koto_parts("Koto #1", &[&desc]).unwrap();
        assert_eq!(win, KotoWin { game: 1, winner: BOB, others: vec![] });
    }

    #[test]
    fn koto_garbled_cards_are_refused_not_guessed() {
        let no_rows = ["Good job! Everyone who participated gets +2 points!"];
        assert_eq!(parse_koto_parts("Koto #3", &no_rows), Err(KotoSkip::NoRows));
        assert_eq!(parse_koto_parts("Wordle", &[&format!("1 X <@{}> +1\nGood job!", ALICE)]), Err(KotoSkip::NoGame));
        // A line naming two people is not a guess row, even without scores.
        let desc = format!("Players: <@{}> <@{}>\nGood job!", ALICE, BOB);
        assert_eq!(parse_koto_parts("Koto #4", &[&desc]), Err(KotoSkip::NoRows));
        // Rows without the score column still read, one person per line.
        let desc = format!("1 QWERTY <@{}>\n2 MAILED <@{}>\nGood job!", ALICE, BOB);
        assert_eq!(parse_koto_parts("Koto #6", &[&desc]).unwrap().winner, BOB);
    }

    #[test]
    fn koto_reads_serenity_embeds() {
        let mut e = Embed::default();
        e.title = Some("Koto #887".into());
        e.description = Some(format!("1 MAILED <@{}> +4\n\nGood job! Everyone who participated gets +2 points!", BOB));
        assert_eq!(parse_koto(&[e]).unwrap().winner, BOB);
    }

    #[test]
    fn anagram_lines_are_told_apart() {
        let puzzle = parse_anagram("Guess the word: **welbgo** in **240** seconds");
        assert_eq!(puzzle, Some(AnagramLine::Puzzle("beglow".into())));
        assert_eq!(parse_anagram("The correct answer was **reduction**"), Some(AnagramLine::TimedOut));
        let solve = "**zecorinthian!** was correct and has been awarded **145** Points, bringing them up to a total…";
        assert_eq!(parse_anagram(solve), Some(AnagramLine::Solved("zecorinthian".into())));
        assert_eq!(parse_anagram("was correct, apparently"), None);
    }

    #[test]
    fn the_solver_is_the_newest_fresh_human_preferring_the_right_answer() {
        let line = id_at(30_000);
        let ring = vec![
            seen(id_at(21_000), ALICE, "hello"),
            seen(id_at(29_000), BOB, "bowgle"),
            seen(id_at(29_500), CARA, "lol"),
        ];
        let letters = sorted_letters("welbgo");
        let spells = |s: &Seen| sorted_letters(&s.text) == letters;
        assert_eq!(pick_solver(&ring, line, None, spells).map(|s| s.user), Some(BOB));
        // With nothing that looks right, the newest human before the line.
        assert_eq!(pick_solver(&ring, line, None, |_| false).map(|s| s.user), Some(CARA));
        // A reply names the winner outright.
        assert_eq!(pick_solver(&ring, line, Some(id_at(21_000)), |_| false).map(|s| s.user), Some(ALICE));
        // Messages after the line, or long before it, are never the winner.
        let late = vec![seen(id_at(30_500), ALICE, "cat"), seen(id_at(30_000 - FRESH_MS - 1), BOB, "cat")];
        assert!(pick_solver(&late, line, None, |_| true).is_none());
    }

    #[test]
    fn cat_catches_read_the_catcher_and_the_type() {
        let line = "amittyagi0491 cought <:gremlincat_c:12345> Gremlin cat!!!!1!\nYou now have 10 cats of dat type!!!";
        let c = parse_cat(line).unwrap();
        assert_eq!(c, Catch { name: "amittyagi0491".into(), kind: "Gremlin".into() });
        assert_eq!(cat_points(&c.kind), 1);
        let c = parse_cat("baco\\_only. cought <a:legendarycat:99> Legendary cat!!!!1!").unwrap();
        assert_eq!(c.name, "baco_only.");
        assert_eq!(cat_points(&c.kind), 3);
        assert_eq!(cat_points(&parse_cat("x cought <:s:1> Sus cat!").unwrap().kind), 2);
        assert_eq!(cat_points(&parse_cat("x cought <:s:1> eGirl cat!").unwrap().kind), 1, "unknown types pay 1");
        // No emoji and a filler word: no type, the minimum.
        let c = parse_cat("x cought this cat!").unwrap();
        assert_eq!(c.kind, "");
        assert_eq!(cat_points(&c.kind), 1);
        // A spawn is not a catch.
        assert!(parse_cat("<:nicecat:123> Nice cat has appeared! Type \"cat\" to catch it!").is_none());
    }

    #[test]
    fn a_cat_goes_to_whoever_typed_cat_over_later_chatter() {
        let line = id_at(5_000);
        let ring = vec![seen(id_at(4_000), ALICE, "cat"), seen(id_at(4_600), BOB, "nooo")];
        assert_eq!(pick_solver(&ring, line, None, |s| s.text == "cat").map(|s| s.user), Some(ALICE));
    }

    #[test]
    fn escaped_names_are_unescaped() {
        assert_eq!(unescape("baco\\_only\\*"), "baco_only*");
        assert_eq!(unescape("plain\\name"), "plain\\name");
    }
}
