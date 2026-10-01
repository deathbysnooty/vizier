//! The hatch: four houses dealt out so they come out LEVEL, and a dragon each.
//!
//! The old draft dealt people out so the four houses held the same NUMBER of
//! members. That is the wrong thing to make equal. Three of the server's
//! thousand-point players in one house decides the month in the first week,
//! however many quiet members are stacked against them. So the egg week is the
//! measurement, and the hatch is the deal: everybody's points for the week are
//! the weight, and the houses are filled to be equal in TOTAL ACTIVITY.
//!
//! The deal is the obvious greedy one, and it is the right one here: sort
//! everybody heaviest first, and give each in turn to whichever house is
//! lightest. Taking the biggest first is what keeps the error small - by the
//! time a house is being handed somebody worth four points, the gaps left to
//! close are of that size. The quiet members come last, and because a house
//! with the same total as another is broken apart by its HEADCOUNT, they land
//! one each around the four rather than all in the house that happened to be a
//! point behind. That is the "spread the quiet ones" rule, and it falls out of
//! the tie-break rather than needing a pass of its own.
//!
//! Nothing here reads the clock or the stores: [`split`] and [`name_dragons`]
//! are pure, which is what lets `/hatch dry:true` show the owner exactly the
//! split that running it for real would apply.

use std::collections::{HashMap, HashSet};

use super::house::HOUSES;

/// One person, dealt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dealt {
    pub user: u64,
    /// What they earned in the egg week - the weight they were dealt by.
    pub points: i64,
    /// The ledger key of the house they land in.
    pub house: &'static str,
    pub dragon: String,
}

/// How a split came out, for the reveal and for the dry run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    pub dealt: Vec<Dealt>,
    /// Per house, in `HOUSES` order: total points, and headcount.
    pub totals: Vec<(&'static str, i64, usize)>,
}

impl Split {
    /// The gap between the heaviest house and the lightest. The number the whole
    /// thing exists to keep small.
    pub fn gap(&self) -> i64 {
        let high = self.totals.iter().map(|(_, n, _)| *n).max().unwrap_or(0);
        let low = self.totals.iter().map(|(_, n, _)| *n).min().unwrap_or(0);
        high - low
    }

    /// The gap in headcount, which is allowed to be wide - that is the point.
    pub fn head_gap(&self) -> usize {
        let high = self.totals.iter().map(|(_, _, n)| *n).max().unwrap_or(0);
        let low = self.totals.iter().map(|(_, _, n)| *n).min().unwrap_or(0);
        high - low
    }
}

/// Deals everybody into the four houses, heaviest first, each to the lightest
/// house going.
///
/// `members` is (member, points for the week) in any order; the deal sorts it,
/// and ties are broken by the member id so two runs of the same week give the
/// same answer. Dragons are not named here - see [`name_dragons`].
pub fn split(members: &[(u64, i64)]) -> Split {
    let mut queue: Vec<(u64, i64)> = members.to_vec();
    // Heaviest first; level members in a stable order, never a wobbly one.
    queue.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut totals: Vec<(&'static str, i64, usize)> = HOUSES.iter().map(|h| (h.key, 0i64, 0usize)).collect();
    let mut dealt = Vec::with_capacity(queue.len());
    for (user, points) in queue {
        // The lightest house: by points, then by headcount - which is what
        // spreads the quiet members instead of clumping them - then by the
        // house's own order, so nothing is ever decided by chance.
        let pick = (0..totals.len())
            .min_by(|a, b| {
                totals[*a].1.cmp(&totals[*b].1).then(totals[*a].2.cmp(&totals[*b].2)).then(a.cmp(b))
            })
            .unwrap_or(0);
        totals[pick].1 += points;
        totals[pick].2 += 1;
        dealt.push(Dealt { user, points, house: totals[pick].0, dragon: String::new() });
    }
    Split { dealt, totals }
}

// --- the dragons --------------------------------------------------------------------

/// Valyrian-sounding names. Long enough that a server several times this one's
/// size still gets one each, and if it ever runs out a numeral is added rather
/// than a name being shared.
pub const DRAGONS: &[&str] = &[
    "Vhagaryx", "Meleyra", "Syraxor", "Caraxon", "Vermithas", "Drogaryn", "Balerys", "Sunfyrax", "Tessaryon",
    "Morghul", "Aemyra", "Valaryon", "Daemyx", "Rhaenyx", "Vhaelor", "Seasmyra", "Grey Ghost", "Silverwyng",
    "Shrykos", "Zaldrizar", "Qoherys", "Terrax", "Arrax", "Tyraxes", "Moondancer", "Stormcloud", "Vermax",
    "Sheepstealer", "Cannibal", "Nettlewing", "Urrathon", "Maegaryx", "Jaehaelor", "Viserax", "Aerion",
    "Gaelithox", "Hraegar", "Ormund", "Belaerys", "Draekath", "Elaenyx", "Haegor", "Illyrax", "Jorakar",
    "Kaelithon", "Lysarax", "Mordrax", "Naelyra", "Obarys", "Pyrathos", "Quenthrax", "Raelithar", "Sorax",
    "Thessaly", "Ulvaryn", "Vaelithor", "Wraethyx", "Xandrys", "Yvaraxes", "Zhaedar", "Aethryx", "Brynaxes",
    "Corvaryn", "Dryxaros", "Emberyx", "Falyrion", "Ghaestar", "Helaenyx", "Ithrax", "Jovaryn", "Kythrax",
    "Lornaxes", "Mythraen", "Nyxaros", "Orthaeryx", "Pellaxes", "Qyburnax", "Rhogaryn", "Saelithyx", "Torrhaxes",
    "Umbraryn", "Veltharyx", "Wynnaros", "Xyrathon", "Ysolde", "Zephyrax", "Astaryx", "Belgaryn", "Caelithar",
    "Dorynax",
];

/// Gives each of `users` a dragon name that nobody else on the server has.
///
/// `taken` is every name already handed out, so a member who joins after the
/// hatch can be named later without colliding with anyone. The order of `users`
/// decides who gets what, so the same hatch run twice names the same dragons.
pub fn name_dragons(users: &[u64], taken: &HashSet<String>) -> HashMap<u64, String> {
    let mut used: HashSet<String> = taken.clone();
    let mut out = HashMap::with_capacity(users.len());
    let mut next = 0usize;
    for user in users {
        let name = loop {
            // Past the end of the list, the list starts again with a numeral:
            // "Vhagaryx II", then "Vhagaryx III". Nobody shares a name.
            let round = next / DRAGONS.len();
            let base = DRAGONS[next % DRAGONS.len()];
            next += 1;
            let candidate =
                if round == 0 { base.to_string() } else { format!("{} {}", base, numeral(round + 1)) };
            if used.insert(candidate.clone()) {
                break candidate;
            }
        };
        out.insert(*user, name);
    }
    out
}

/// Roman numerals, as far as a dragon name will ever need them.
fn numeral(n: usize) -> String {
    const PARTS: [(usize, &str); 7] =
        [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L")];
    const SMALL: [(usize, &str); 6] = [(40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut left = n;
    let mut out = String::new();
    for (value, sign) in PARTS.into_iter().chain(SMALL) {
        while left >= value {
            out.push_str(sign);
            left -= value;
        }
    }
    out
}

/// The whole hatch in one call: the deal and the names together.
pub fn deal(members: &[(u64, i64)], taken: &HashSet<String>) -> Split {
    let mut split = split(members);
    let order: Vec<u64> = split.dealt.iter().map(|d| d.user).collect();
    let names = name_dragons(&order, taken);
    for one in &mut split.dealt {
        one.dragon = names.get(&one.user).cloned().unwrap_or_default();
    }
    split
}

// --- the reveal -----------------------------------------------------------------------

/// What the hatch posts, or what a dry run shows instead of posting it.
pub fn reveal_text(split: &Split, dry: bool) -> String {
    let head = if dry {
        "🧪 **Dry run — nothing has been written.** This is how the hatch would fall:".to_string()
    } else {
        format!("🔥 **The eggs are open.** {} dragons, four houses.", split.dealt.len())
    };
    let mut text = head;
    let mut ordered: Vec<&(&'static str, i64, usize)> = split.totals.iter().collect();
    ordered.sort_by(|a, b| b.1.cmp(&a.1));
    for (key, points, count) in ordered {
        let worn = super::house::house(key).map(|h| super::month::themed(h));
        let (crest, name) = match &worn {
            Some(t) => (t.crest.as_str(), t.name.as_str()),
            None => ("🏳️", *key),
        };
        text.push_str(&format!("\n{} **{}** · {} members · **{}** from the week", crest, name, count, points));
    }
    text.push_str(&format!(
        "\n-# Dealt by the week's activity, not by headcount: the four started within **{}** point{} of each other.",
        split.gap(),
        if split.gap() == 1 { "" } else { "s" }
    ));
    super::month::with_live(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// September's real shape: a few people in the thousands, a middle, and a
    /// long tail of members who played once or twice. 53 of them, as the
    /// sign-up sheet has.
    fn september() -> Vec<(u64, i64)> {
        let heavy = [3140i64, 2870, 2410, 1990, 1655];
        let middle = [980i64, 910, 840, 790, 720, 680, 640, 605, 560, 520, 480, 455, 410, 380, 355];
        let tail = [
            140i64, 125, 118, 96, 90, 84, 77, 70, 66, 61, 55, 48, 44, 40, 36, 31, 28, 24, 20, 18, 15, 12, 10, 8, 6, 5,
            4, 3, 2, 1, 1, 0, 0,
        ];
        heavy.iter().chain(&middle).chain(&tail).enumerate().map(|(i, n)| (100 + i as u64, *n)).collect()
    }

    #[test]
    fn the_four_houses_come_out_level_on_activity_not_on_headcount() {
        let members = september();
        let total: i64 = members.iter().map(|(_, n)| n).sum();
        let split = split(&members);
        assert_eq!(split.dealt.len(), members.len(), "everybody is dealt, once");
        assert_eq!(split.totals.iter().map(|(_, n, _)| n).sum::<i64>(), total, "and no points are invented or lost");
        // The thing it is for: the four start within a rounding error of each
        // other, on a spread where the top player alone outweighs the whole tail.
        assert!(split.gap() <= 50, "the houses started {} points apart - that is a decided month", split.gap());
        let share = total / 4;
        for (key, points, count) in &split.totals {
            assert!((points - share).abs() <= 50, "{} is {} against a fair share of {}", key, points, share);
            assert!(*count > 0, "{} got nobody", key);
        }
        // Headcount is NOT equal, and is not supposed to be - but nor is it
        // absurd: the quiet members are spread, not stacked in one house.
        assert!(split.head_gap() <= 4, "the quiet members were clumped: {:?}", split.totals);
    }

    #[test]
    fn the_same_week_always_deals_the_same_way() {
        let members = september();
        let once = split(&members);
        let mut shuffled = members.clone();
        shuffled.reverse();
        assert_eq!(split(&shuffled), once, "the order it arrives in changes nothing");
        assert_eq!(split(&members), once, "and running it twice changes nothing");
    }

    #[test]
    fn the_heaviest_players_are_not_all_in_one_house() {
        let split = split(&september());
        let top: Vec<&'static str> = split.dealt.iter().take(4).map(|d| d.house).collect();
        let distinct: HashSet<&&str> = top.iter().collect();
        assert_eq!(distinct.len(), 4, "the top four must land one in each house, not two together: {:?}", top);
    }

    #[test]
    fn the_awkward_sizes_all_deal() {
        assert_eq!(split(&[]).dealt.len(), 0);
        assert_eq!(split(&[]).gap(), 0);
        let one = split(&[(1, 500)]);
        assert_eq!(one.dealt.len(), 1);
        assert_eq!(one.totals.iter().filter(|(_, _, n)| *n > 0).count(), 1);
        // Everybody at nought: four houses, as near equal in headcount as it goes.
        let quiet: Vec<(u64, i64)> = (0..13u64).map(|u| (u, 0)).collect();
        let split = split(&quiet);
        assert_eq!(split.gap(), 0);
        assert!(split.head_gap() <= 1, "with nothing to weigh, the headcount evens out: {:?}", split.totals);
    }

    #[test]
    fn every_dragon_is_a_name_of_its_own() {
        let users: Vec<u64> = (0..53).collect();
        let names = name_dragons(&users, &HashSet::new());
        assert_eq!(names.len(), 53);
        let distinct: HashSet<&String> = names.values().collect();
        assert_eq!(distinct.len(), 53, "two riders had the same dragon");
        assert!(names.values().all(|n| !n.trim().is_empty()));
        // A later arrival can't be given a name already in the sky.
        let taken: HashSet<String> = names.values().cloned().collect();
        let more = name_dragons(&[900, 901], &taken);
        assert!(more.values().all(|n| !taken.contains(n)), "a latecomer took somebody's dragon");
    }

    #[test]
    fn running_out_of_names_adds_a_numeral_rather_than_sharing_one() {
        let users: Vec<u64> = (0..(DRAGONS.len() as u64 * 2 + 5)).collect();
        let names = name_dragons(&users, &HashSet::new());
        let distinct: HashSet<&String> = names.values().collect();
        assert_eq!(distinct.len(), users.len(), "even past the end of the list, nobody shares");
        assert!(names.values().any(|n| n.ends_with(" II")), "the second time round is numbered: {:?}", distinct.len());
        assert_eq!(numeral(2), "II");
        assert_eq!(numeral(4), "IV");
        assert_eq!(numeral(9), "IX");
    }

    #[test]
    fn deal_puts_a_dragon_on_everybody_it_dealt() {
        let split = deal(&september(), &HashSet::new());
        assert!(split.dealt.iter().all(|d| !d.dragon.is_empty()));
        let distinct: HashSet<&String> = split.dealt.iter().map(|d| &d.dragon).collect();
        assert_eq!(distinct.len(), split.dealt.len());
        assert_eq!(deal(&september(), &HashSet::new()), split, "and the same week deals the same dragons");
    }

    #[test]
    fn the_reveal_names_the_four_and_a_dry_run_says_it_wrote_nothing() {
        // Held off, so the reveal is read in the houses' own names rather than
        // whatever paint another test happens to have on at the time.
        let _month = super::super::month::testing::Month::off();
        let split = deal(&september(), &HashSet::new());
        let real = reveal_text(&split, false);
        assert!(real.contains("The eggs are open"));
        assert!(real.contains("53 dragons"));
        let dry = reveal_text(&split, true);
        assert!(dry.contains("nothing has been written"), "{}", dry);
        for (key, _, _) in &split.totals {
            assert!(real.contains(super::super::house::house(key).expect("a real house").name), "{} missing", key);
        }
    }
}

// --- `/hatch` --------------------------------------------------------------------------

use chrono::Utc;
use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions, CreateCommand,
    CreateCommandOption, CreateInteractionResponse, CreateInteractionResponseMessage, CreateMessage,
};

use super::egg_store::{self as store};
use super::month;
use super::points::{self as ledger};

pub fn builder() -> CreateCommand {
    CreateCommand::new("hatch")
        .description("admin only: open every egg, deal the houses and post the reveal")
        .add_option(
            CreateCommandOption::new(CommandOptionType::Boolean, "dry", "show the split without applying it")
                .required(false),
        )
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

/// What a member weighed in the egg week: everything they earned since their own
/// egg was claimed, or since the month began, whichever is later.
fn weights(eggs: &[store::Egg], now: i64) -> Vec<(u64, i64)> {
    let Some(db) = super::house::db() else { return eggs.iter().map(|e| (e.user, 0)).collect() };
    let conn = db.lock();
    let month_start = ledger::month_start(now);
    eggs.iter()
        .map(|egg| {
            let since = egg.claimed_ts.max(month_start);
            let earned: i64 =
                ledger::breakdown(&conn, egg.user, since).map(|rows| rows.iter().map(|(_, n)| n).sum()).unwrap_or(0);
            let pooled: i64 = ledger::pool_breakdown(&conn, egg.user, since).iter().map(|(_, n)| n).sum();
            (egg.user, (earned + pooled).max(0))
        })
        .collect()
}

/// The refusal when it has already been run. A hatch is once a month: running it
/// twice would re-deal everybody's house out from under them.
pub const ALREADY: &str =
    "🔥 The eggs have already hatched. Nothing has been changed - re-running it would deal every house out from \
     under everybody. `/hatch dry:true` still shows what the split looked like.";

pub async fn command(ctx: &Context, command: &CommandInteraction) {
    let by = command.user.id.get();
    if !super::admin_ids().contains(&by) {
        let _ = command.create_response(&ctx.http, whisper("Only mods can hatch the eggs.")).await;
        return;
    }
    if !month::running() {
        let _ = command.create_response(&ctx.http, whisper(super::egg::MONTH_OFF)).await;
        return;
    }
    let dry = command
        .data
        .options
        .iter()
        .find_map(|o| match (&o.value, o.name == "dry") {
            (CommandDataOptionValue::Boolean(v), true) => Some(*v),
            _ => None,
        })
        .unwrap_or(false);

    let Some(db) = store::db() else {
        let _ = command.create_response(&ctx.http, whisper("The egg store isn't open.")).await;
        return;
    };
    let now = Utc::now().timestamp();
    let (already, eggs, taken) = {
        let conn = db.lock();
        (store::hatched_at(&conn), store::unhatched(&conn), store::dragons(&conn))
    };
    if already.is_some() && !dry {
        let _ = command.create_response(&ctx.http, whisper(month::with_live(ALREADY.to_string()))).await;
        return;
    }
    if eggs.is_empty() {
        let _ = command
            .create_response(&ctx.http, whisper("There are no unopened eggs. Nobody has claimed one yet."))
            .await;
        return;
    }

    let split = deal(&weights(&eggs, now), &taken);
    if dry {
        let _ = command.create_response(&ctx.http, whisper(reveal_text(&split, true))).await;
        return;
    }

    // Written down BEFORE anything is posted or any role is touched: a house and
    // a dragon that only exist in a Discord message would be lost on a restart.
    {
        let conn = db.lock();
        for one in &split.dealt {
            match store::hatch(&conn, one.user, one.house, &one.dragon, now) {
                Ok(true) => {}
                Ok(false) => tracing::warn!("hatch: {}'s egg was already open, left as it was", one.user),
                Err(err) => tracing::error!("hatch: {} not written ({}) - they keep their old house", one.user, err),
            }
        }
        let _ = store::mark_hatched(&conn, now);
    }
    for one in &split.dealt {
        if let Some(house) = super::house::house(one.house) {
            super::house::place(one.user, house, "hatch");
        }
    }
    let text = reveal_text(&split, false);
    let _ = command
        .create_response(
            ctx,
            CreateInteractionResponse::Message(
                CreateInteractionResponseMessage::new().content(&text).allowed_mentions(CreateAllowedMentions::new()),
            ),
        )
        .await;
    // The roles come last and are allowed to fail one by one: the store is the
    // truth, and `/houseroles` can put any that didn't take right afterwards.
    if let Some(guild) = command.guild_id {
        for one in &split.dealt {
            if let Some(house) = super::house::house(one.house) {
                super::house::wear(ctx, guild, one.user, house).await;
            }
        }
    }
    if let Some(channel) = super::control::id("VIZIER_HOUSE_CHANNEL") {
        let channel = serenity::all::ChannelId::new(channel);
        let _ = channel
            .send_message(&ctx.http, CreateMessage::new().content(&text).allowed_mentions(CreateAllowedMentions::new()))
            .await;
    }
    tracing::info!("hatch: {} eggs opened by {}, houses within {} points", split.dealt.len(), by, split.gap());
}
