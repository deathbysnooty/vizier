//! `/housecards`: every Chocolate Frog card a house holds, and who holds it.
//!
//! Two ways of looking at the same pile. **By card** - the default - walks the
//! cards in the game's own order, says how many of each the house has and who
//! has them, and marks plainly the ones nobody has: knowing what the house
//! lacks is the point of the list. **By member** turns it round and lists the
//! collectors, most cards first, with what each is holding; members with none
//! are left out, since that is a roll call and `/houselist` already does it.
//!
//! The number that matters is at the top either way: how many full sets the
//! house could put together between them, which is the smallest count across
//! the cards in play and exactly what `/sellset` wants.
//!
//! A card counts for the house its holder is in *now*. Change house and your
//! cards go with you - nothing is stamped onto the card itself.
//!
//! Only cards still `owned` count. A copy handed in for a set is gone.
//!
//! Every view is squeezed to fit Discord before it leaves: holders per card,
//! cards per member and then whole collectors are dropped for a "+N more"
//! rather than letting a reply be refused. The wording is built from plain
//! slices so all of that can be tested without a database.

use serenity::all::{
    CommandDataOptionValue, CommandInteraction, CommandOptionType, Context, CreateAllowedMentions, CreateCommand,
    CreateCommandOption, CreateEmbed, CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseMessage,
};

use super::frog_store::{self as store, Card, Rarity, Wizard};
use super::house;

/// Discord's limit on an embed description - the whole list is one description.
pub const DESCRIPTION_LIMIT: usize = 4096;
/// And on an embed altogether, title and footer counted in.
pub const EMBED_LIMIT: usize = 6000;

/// Holders named under one card before the rest become "+N more".
const HOLDER_CAP: usize = 8;
/// Cards named beside one member before the rest become "+N more".
const CARD_CAP: usize = 8;

/// Why a card may be somewhere unexpected, said once under every list.
const FOOTER: &str = "Cards go with their owner: change house and your cards change house too";

/// Which way round the list is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    ByCard,
    ByMember,
}

impl View {
    fn from_key(key: &str) -> View {
        if key == "member" { View::ByMember } else { View::ByCard }
    }
}

fn s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// "#0042", the short form of a card's number next to a name.
fn number(serial: i64) -> String {
    format!("#{:04}", serial)
}

/// Legendary first, which is the order a collector reads their own cards in.
fn rarest_first(r: Rarity) -> usize {
    2 - Rarity::ALL.iter().position(|x| *x == r).unwrap_or(0)
}

// --- shared shaping ---------------------------------------------------------

/// Cuts a finished description to the budget rather than let Discord refuse it.
/// Nothing should ever reach this - the caps above are meant to get there first
/// - but a reply that is merely short is far better than a reply that is never
/// sent.
fn never_too_long(text: String, budget: usize) -> String {
    if text.chars().count() <= budget {
        return text;
    }
    let mut out: String = text.chars().take(budget.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Keeps as many lines as fit the budget, the rest becoming one `more(n)` line.
/// Lines are already in the order that matters, so this only ever drops from the
/// end.
fn keep_that_fit(lines: &[String], budget: usize, more: impl Fn(usize) -> String) -> Vec<String> {
    let width = |kept: &[String], tail: Option<&String>| -> usize {
        kept.iter().chain(tail).map(|l| l.chars().count() + 1).sum()
    };
    if width(lines, None) <= budget {
        return lines.to_vec();
    }
    for keep in (0..lines.len()).rev() {
        let tail = more(lines.len() - keep);
        if width(&lines[..keep], Some(&tail)) <= budget {
            let mut out = lines[..keep].to_vec();
            out.push(tail);
            return out;
        }
    }
    vec![more(lines.len())]
}

// --- by card ----------------------------------------------------------------

/// One card in the game and who in the house holds copies of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CardRow {
    /// "🔥 **The Eternal Phoenix** · Legendary — **2**", or the missing form.
    head: String,
    /// One per holder, most copies first: "<@111> #0007" or "<@111> ×2".
    holders: Vec<String>,
    /// How many holders are still named; trimmed when the list runs long.
    cap: usize,
}

impl CardRow {
    fn shown(&self) -> usize {
        self.cap.min(self.holders.len())
    }

    fn line(&self) -> String {
        let shown = self.shown();
        let mut text = self.head.clone();
        for holder in &self.holders[..shown] {
            text.push_str(" · ");
            text.push_str(holder);
        }
        if self.holders.len() > shown {
            text.push_str(&format!(" · +{} more", self.holders.len() - shown));
        }
        text
    }
}

/// Who holds a card: each holder with their copies' numbers, most copies first
/// and the earliest number breaking a tie.
fn holders_of(cards: &[Card], wizard_id: i64) -> Vec<(u64, Vec<i64>)> {
    let mut out: Vec<(u64, Vec<i64>)> = Vec::new();
    for card in cards.iter().filter(|c| c.wizard_id == wizard_id) {
        match out.iter_mut().find(|(user, _)| *user == card.user_id) {
            Some((_, serials)) => serials.push(card.serial),
            None => out.push((card.user_id, vec![card.serial])),
        }
    }
    for (_, serials) in out.iter_mut() {
        serials.sort_unstable();
    }
    out.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.1[0].cmp(&b.1[0])));
    out
}

/// The cards this list covers: everything in play, plus any retired card the
/// house still holds - the same rule `/frogs` uses, and the same order.
fn on_the_list<'a>(wizards: &'a [Wizard], cards: &[Card]) -> Vec<&'a Wizard> {
    wizards.iter().filter(|w| w.enabled || cards.iter().any(|c| c.wizard_id == w.id)).collect()
}

/// How many whole sets the house could make between them: the smallest number
/// of copies it has of any card in play. One card missing and the answer is 0,
/// however many of the rest are stacked up.
pub fn full_sets(wizards: &[Wizard], cards: &[Card]) -> usize {
    let in_play: Vec<&Wizard> = wizards.iter().filter(|w| w.enabled).collect();
    if in_play.is_empty() {
        return 0;
    }
    in_play.iter().map(|w| cards.iter().filter(|c| c.wizard_id == w.id).count()).min().unwrap_or(0)
}

fn collectors(cards: &[Card]) -> Vec<u64> {
    let mut out: Vec<u64> = cards.iter().map(|c| c.user_id).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The tally every view opens with.
fn tally(wizards: &[Wizard], cards: &[Card]) -> String {
    let on_list = on_the_list(wizards, cards);
    let in_play: Vec<&&Wizard> = on_list.iter().filter(|w| w.enabled).collect();
    let have = in_play.iter().filter(|w| cards.iter().any(|c| c.wizard_id == w.id)).count();
    let people = collectors(cards).len();
    let sets = full_sets(wizards, cards);
    format!(
        "**{} card{}** · **{} of {}** collected · **{}** collector{} · {}",
        cards.len(),
        s(cards.len()),
        have,
        in_play.len(),
        people,
        s(people),
        match sets {
            0 => "**no full set yet**".to_string(),
            n => format!("**{} full set{}** between them — `/sellset`", n, s(n)),
        }
    )
}

/// The card-by-card list: every card in the game, in the game's own order, with
/// who holds it - and nothing left out when nobody does.
pub fn by_card_text(wizards: &[Wizard], cards: &[Card], budget: usize) -> String {
    let header = tally(wizards, cards);
    let on_list = on_the_list(wizards, cards);
    if on_list.is_empty() {
        return never_too_long(format!("{}\nThere are no cards in play right now.", header), budget);
    }

    let mut rows: Vec<CardRow> = Vec::new();
    for wizard in &on_list {
        let holders = holders_of(cards, wizard.id);
        let copies: usize = holders.iter().map(|(_, serials)| serials.len()).sum();
        let retired = if wizard.enabled { "" } else { " *(retired)*" };
        let head = if copies == 0 {
            format!("{} **{}** · {}{} — ❌ **missing**", wizard.rarity.emoji(), wizard.name, wizard.rarity.name(), retired)
        } else {
            format!(
                "{} **{}** · {}{} — **{}**",
                wizard.rarity.emoji(),
                wizard.name,
                wizard.rarity.name(),
                retired,
                copies
            )
        };
        let holders = holders
            .into_iter()
            .map(|(user, serials)| match serials.len() {
                1 => format!("<@{}> {}", user, number(serials[0])),
                n => format!("<@{}> ×{}", user, n),
            })
            .collect();
        rows.push(CardRow { head, holders, cap: HOLDER_CAP });
    }

    // Squeeze the crowded cards first: the commonest card is the one carrying
    // the longest tail of holders, and the one nobody learns much from.
    loop {
        let text = assemble(&header, rows.iter().map(|r| r.line()));
        if text.chars().count() <= budget {
            return text;
        }
        let worst = rows.iter_mut().max_by_key(|r| (r.shown(), r.holders.len()));
        match worst {
            Some(row) if row.shown() > 0 => row.cap = row.shown() - 1,
            _ => break,
        }
    }
    never_too_long(assemble(&header, rows.iter().map(|r| r.line())), budget)
}

fn assemble(header: &str, lines: impl Iterator<Item = String>) -> String {
    let mut text = header.to_string();
    for line in lines {
        text.push('\n');
        text.push_str(&line);
    }
    text
}

// --- by member --------------------------------------------------------------

/// A collector's pile, most cards first.
fn piles(cards: &[Card]) -> Vec<(u64, Vec<Card>)> {
    let mut out: Vec<(u64, Vec<Card>)> = Vec::new();
    for card in cards {
        match out.iter_mut().find(|(user, _)| *user == card.user_id) {
            Some((_, held)) => held.push(card.clone()),
            None => out.push((card.user_id, vec![card.clone()])),
        }
    }
    for (_, held) in out.iter_mut() {
        held.sort_by(|a, b| rarest_first(a.rarity).cmp(&rarest_first(b.rarity)).then(a.serial.cmp(&b.serial)));
    }
    // Most cards first; the earliest number settles a tie, so the order never
    // wobbles between two people holding the same number of cards.
    out.sort_by(|a, b| {
        b.1.len().cmp(&a.1.len()).then(
            a.1.iter().map(|c| c.serial).min().unwrap_or(i64::MAX).cmp(&b.1.iter().map(|c| c.serial).min().unwrap_or(i64::MAX)),
        )
    });
    out
}

/// Whether one member holds a copy of every card in play, and so can `/sellset`
/// on their own without trading for anything.
fn whole_set(wizards: &[Wizard], held: &[Card]) -> bool {
    let in_play: Vec<&Wizard> = wizards.iter().filter(|w| w.enabled).collect();
    !in_play.is_empty() && in_play.iter().all(|w| held.iter().any(|c| c.wizard_id == w.id))
}

/// One member's cards, grouped so three copies of a card read "×3" rather than
/// eating three places in the list.
fn pile_line(wizards: &[Wizard], user: u64, held: &[Card]) -> String {
    let mut groups: Vec<(i64, Rarity, String, Vec<i64>)> = Vec::new();
    for card in held {
        match groups.iter_mut().find(|(id, _, _, _)| *id == card.wizard_id) {
            Some((_, _, _, serials)) => serials.push(card.serial),
            None => groups.push((card.wizard_id, card.rarity, card.wizard_name.clone(), vec![card.serial])),
        }
    }
    let shown = groups.len().min(CARD_CAP);
    let mark = if whole_set(wizards, held) { " 🏆" } else { "" };
    let mut text = format!("<@{}>{} — **{} card{}**", user, mark, held.len(), s(held.len()));
    for (_, rarity, name, serials) in &groups[..shown] {
        text.push_str(&format!(
            " · {} {}{}",
            rarity.emoji(),
            name,
            match serials.len() {
                1 => format!(" {}", number(serials[0])),
                n => format!(" ×{}", n),
            }
        ));
    }
    if groups.len() > shown {
        text.push_str(&format!(" · +{} more", groups.len() - shown));
    }
    text
}

/// The member-by-member list: who in the house is holding what. Members with no
/// cards are left out - this is a collectors' list, not a roll call.
pub fn by_member_text(wizards: &[Wizard], cards: &[Card], budget: usize) -> String {
    let piles = piles(cards);
    let ready = piles.iter().filter(|(_, held)| whole_set(wizards, held)).count();
    let mut header = format!(
        "**{} collector{}** · **{} card{}** between them",
        piles.len(),
        s(piles.len()),
        cards.len(),
        s(cards.len())
    );
    if ready > 0 {
        header.push_str(&format!(" · **{}** can `/sellset` alone 🏆", ready));
    }
    if piles.is_empty() {
        return never_too_long(format!("{}\nNobody in the house is holding a card yet.", header), budget);
    }

    let lines: Vec<String> = piles.iter().map(|(user, held)| pile_line(wizards, *user, held)).collect();
    let room = budget.saturating_sub(header.chars().count());
    let kept = keep_that_fit(&lines, room, |left| format!("-# +{} more collector{}", left, s(left)));
    never_too_long(assemble(&header, kept.into_iter()), budget)
}

// --- the command ------------------------------------------------------------

pub fn command() -> CreateCommand {
    CreateCommand::new("housecards")
        .description("which Chocolate Frog cards your house holds, and who holds them")
        .add_option(house::house_option("house", "which house (yours if left out; mods may name any)"))
        .add_option(
            CreateCommandOption::new(CommandOptionType::String, "view", "how to list them")
                .add_string_choice("By card - who holds each card (default)", "card")
                .add_string_choice("By member - what each collector holds", "member"),
        )
}

fn whisper(text: impl Into<String>) -> CreateInteractionResponse {
    CreateInteractionResponse::Message(CreateInteractionResponseMessage::new().content(text).ephemeral(true))
}

/// What to say to someone with no house of their own to look at.
pub fn no_house_text(is_mod: bool) -> String {
    if is_mod {
        "Mods aren't in a house, so there's no collection of your own here. Name one: `/housecards house:`.".to_string()
    } else {
        "You're not in a house, so there's no house collection to show. Your own cards are in `/frogs`.".to_string()
    }
}

/// `/housecards [house] [view]` - private, like `/houselist`.
pub async fn housecards_command(ctx: &Context, command: &CommandInteraction) {
    let asker = command.user.id.get();
    let mut chosen = None;
    let mut view = View::ByCard;
    for option in &command.data.options {
        match (&option.name[..], &option.value) {
            ("house", CommandDataOptionValue::String(key)) => chosen = house::house(key),
            ("view", CommandDataOptionValue::String(key)) => view = View::from_key(key),
            _ => {}
        }
    }
    let mod_here = match (command.guild_id, command.member.as_deref()) {
        (Some(guild), Some(member)) => house::is_mod(ctx, guild, member).await,
        _ => super::admin_ids().contains(&asker),
    };
    let own = house::house_of(asker);
    let target = match (chosen, mod_here) {
        (Some(house), true) => Some(house),
        // Naming someone else's house doesn't open it: members see their own.
        _ => own,
    };
    let Some(house) = target else {
        let _ = command.create_response(&ctx.http, whisper(no_house_text(mod_here))).await;
        return;
    };

    let members = house::members_of(house.key);
    let (wizards, cards) = match store::db() {
        Some(db) => {
            let conn = db.lock();
            (store::wizards(&conn), store::cards_of_users(&conn, &members))
        }
        None => (Vec::new(), Vec::new()),
    };

    let mut text = match view {
        View::ByCard => by_card_text(&wizards, &cards, DESCRIPTION_LIMIT),
        View::ByMember => by_member_text(&wizards, &cards, DESCRIPTION_LIMIT),
    };
    if chosen.is_some() && !mod_here && chosen.map(|h| h.key) != own.map(|h| h.key) {
        text = never_too_long(
            format!("{}\n-# Only mods can look inside another house - this is yours.", text),
            DESCRIPTION_LIMIT,
        );
    }
    let embed = CreateEmbed::new()
        .title(format!("{} {} · Chocolate Frog cards", house.crest, house.name))
        .description(text)
        .colour(house.colour)
        .footer(CreateEmbedFooter::new(FOOTER));
    // Ephemeral, and mentions switched off: a list of every collector in a house
    // must not ping a hundred people.
    let reply = CreateInteractionResponseMessage::new()
        .embed(embed)
        .allowed_mentions(CreateAllowedMentions::new())
        .ephemeral(true);
    if let Err(err) = command.create_response(&ctx.http, CreateInteractionResponse::Message(reply)).await {
        tracing::warn!("frog: /housecards for {} not shown: {}", house.key, err);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wizard(id: i64, name: &str, rarity: Rarity) -> Wizard {
        Wizard { id, slug: name.to_lowercase().replace(' ', "-"), name: name.to_string(), rarity, image: String::new(), enabled: true }
    }

    /// The ten cards of the game, in the order `/frogs` walks them.
    fn game() -> Vec<Wizard> {
        vec![
            wizard(1, "Moonstruck Mirela", Rarity::Common),
            wizard(2, "Luna Lovegood", Rarity::Common),
            wizard(3, "Hermione Granger", Rarity::Common),
            wizard(4, "Sirius Black", Rarity::Common),
            wizard(5, "The Bloomweaver", Rarity::Common),
            wizard(6, "Alchemist Ferro", Rarity::Uncommon),
            wizard(7, "Merlin", Rarity::Uncommon),
            wizard(8, "The Moonkeeper", Rarity::Uncommon),
            wizard(9, "The Original Chocolate Frog", Rarity::Uncommon),
            wizard(10, "Eternal Phoenix", Rarity::Legendary),
        ]
    }

    fn held(serial: i64, user: u64, wizard: &Wizard) -> Card {
        Card {
            serial,
            edition: serial,
            user_id: user,
            wizard_id: wizard.id,
            wizard_name: wizard.name.clone(),
            rarity: wizard.rarity,
            drop_id: Some(serial),
            ts: 0,
            origin: "caught".to_string(),
            original_owner: user,
            status: "owned".to_string(),
            spent_reason: String::new(),
            spent_ts: None,
        }
    }

    /// A pile built from (serial, user, wizard index) triples.
    fn pile(game: &[Wizard], rows: &[(i64, u64, usize)]) -> Vec<Card> {
        rows.iter().map(|(serial, user, w)| held(*serial, *user, &game[*w])).collect()
    }

    #[test]
    fn every_card_is_listed_in_the_games_own_order_and_the_missing_ones_are_marked() {
        let game = game();
        let cards = pile(&game, &[(7, 111, 9), (31, 222, 9), (4, 111, 0)]);
        let text = by_card_text(&game, &cards, DESCRIPTION_LIMIT);
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines.len(), 10, "all ten cards are listed:\n{}", text);
        let named: Vec<&Wizard> = game.iter().collect();
        for (line, wizard) in lines.iter().zip(named) {
            assert!(line.contains(&format!("**{}**", wizard.name)), "{:?} should be {}", line, wizard.name);
        }
        assert!(lines[0].starts_with("🥛 **Moonstruck Mirela** · Common — **1** · <@111> #0004"), "{}", lines[0]);
        assert!(lines[9].starts_with("🔥 **Eternal Phoenix** · Legendary — **2** · <@111> #0007 · <@222> #0031"), "{}", lines[9]);
        // The eight cards nobody holds are still on the list, plainly marked.
        assert_eq!(lines.iter().filter(|l| l.contains("❌ **missing**")).count(), 8, "{}", text);
        assert!(lines[1].ends_with("Luna Lovegood** · Common — ❌ **missing**"), "{}", lines[1]);
    }

    #[test]
    fn duplicates_are_counted_once_per_holder_and_serials_shown_for_singles() {
        let game = game();
        let cards = pile(&game, &[(1, 111, 0), (2, 111, 0), (3, 333, 0), (9, 444, 0), (12, 555, 0)]);
        let line = by_card_text(&game, &cards, DESCRIPTION_LIMIT).lines().nth(1).unwrap().to_string();
        assert_eq!(line, "🥛 **Moonstruck Mirela** · Common — **5** · <@111> ×2 · <@333> #0003 · <@444> #0009 · <@555> #0012");
    }

    #[test]
    fn a_missing_card_means_no_full_set_however_many_of_the_rest_there_are() {
        let game = game();
        // Nine of the ten, stacked three deep, and not one Phoenix.
        let mut rows = Vec::new();
        let mut serial = 1;
        for w in 0..9 {
            for _ in 0..3 {
                rows.push((serial, 111, w));
                serial += 1;
            }
        }
        let cards = pile(&game, &rows);
        assert_eq!(full_sets(&game, &cards), 0);
        assert!(by_card_text(&game, &cards, DESCRIPTION_LIMIT).starts_with("**27 cards** · **9 of 10** collected · **1** collector · **no full set yet**"));
    }

    #[test]
    fn the_full_sets_number_is_the_smallest_pile_across_the_ten_cards() {
        let game = game();
        let mut rows = Vec::new();
        let mut serial = 1;
        // Four of everything, but only two Phoenixes: two sets, spread over
        // three people, is the answer.
        for w in 0..9 {
            for copy in 0..4 {
                rows.push((serial, 111 + copy as u64, w));
                serial += 1;
            }
        }
        rows.push((serial, 111, 9));
        rows.push((serial + 1, 112, 9));
        let cards = pile(&game, &rows);
        assert_eq!(full_sets(&game, &cards), 2);
        let text = by_card_text(&game, &cards, DESCRIPTION_LIMIT);
        assert!(text.starts_with("**38 cards** · **10 of 10** collected · **4** collectors · **2 full sets** between them — `/sellset`"), "{}", text);
    }

    #[test]
    fn a_card_in_play_that_nobody_holds_still_gets_a_line_and_a_retired_one_only_when_held() {
        let game = game();
        let mut wizards = game.clone();
        wizards[1].enabled = false;
        // Nobody holds the retired Luna, so she is off the list altogether.
        let cards = pile(&game, &[(1, 111, 0)]);
        let text = by_card_text(&wizards, &cards, DESCRIPTION_LIMIT);
        assert_eq!(text.lines().count() - 1, 9, "{}", text);
        assert!(!text.contains("Luna"), "{}", text);
        assert!(text.starts_with("**1 card** · **1 of 9** collected"), "{}", text);
        // Once someone holds one, she is back on it and said to be retired.
        let cards = pile(&game, &[(1, 111, 0), (2, 111, 1)]);
        let text = by_card_text(&wizards, &cards, DESCRIPTION_LIMIT);
        assert!(text.contains("**Luna Lovegood** · Common *(retired)* — **1** · <@111> #0002"), "{}", text);
        assert!(text.starts_with("**2 cards** · **1 of 9** collected"), "{}", text);
    }

    #[test]
    fn the_holders_under_one_card_are_capped_with_a_count_of_the_rest() {
        let game = game();
        let rows: Vec<(i64, u64, usize)> = (0..20).map(|i| (i + 1, 100 + i as u64, 0)).collect();
        let cards = pile(&game, &rows);
        let line = by_card_text(&game, &cards, DESCRIPTION_LIMIT).lines().nth(1).unwrap().to_string();
        assert_eq!(line.matches("<@").count(), HOLDER_CAP, "{}", line);
        assert!(line.contains("— **20** ·"), "{}", line);
        assert!(line.ends_with("· +12 more"), "{}", line);
    }

    /// A house at the size the server actually runs at: 144 members, hundreds of
    /// cards, every card in the game held many times over.
    fn a_big_house() -> (Vec<Wizard>, Vec<Card>) {
        let game = game();
        let mut rows = Vec::new();
        let mut serial = 1;
        for member in 0..144u64 {
            for w in 0..10 {
                if (member as usize + w) % 3 == 0 {
                    rows.push((serial, 400_000_000_000_000_000 + member, w));
                    serial += 1;
                }
            }
        }
        let cards = pile(&game, &rows);
        assert!(cards.len() > 400, "{} cards", cards.len());
        (game, cards)
    }

    #[test]
    fn a_house_with_hundreds_of_cards_still_fits_what_discord_will_take() {
        let (game, cards) = a_big_house();
        for text in [by_card_text(&game, &cards, DESCRIPTION_LIMIT), by_member_text(&game, &cards, DESCRIPTION_LIMIT)] {
            let chars = text.chars().count();
            assert!(chars <= DESCRIPTION_LIMIT, "{} chars in a description", chars);
            // Title and footer ride along in the same embed.
            let whole = chars + FOOTER.chars().count() + "🦁 Hufflepuff · Chocolate Frog cards".chars().count();
            assert!(whole <= EMBED_LIMIT, "{} chars in an embed", whole);
        }
        // Every card still gets its line, however crowded the house is.
        assert_eq!(by_card_text(&game, &cards, DESCRIPTION_LIMIT).lines().count() - 1, 10);
    }

    #[test]
    fn a_crowded_list_gives_up_holders_on_the_commonest_cards_first() {
        let (game, cards) = a_big_house();
        // A budget far below what the list wants, to force the squeeze.
        let text = by_card_text(&game, &cards, 700);
        assert!(text.chars().count() <= 700, "{} chars", text.chars().count());
        assert_eq!(text.lines().count() - 1, 10, "every card keeps its line:\n{}", text);
        assert!(text.contains("more"), "the rest are counted:\n{}", text);
    }

    /// Puts a card straight into the store, so the reading side can be tested
    /// without playing a whole frog through.
    fn store_card(conn: &rusqlite::Connection, serial: i64, user: u64, wizard_id: i64, status: &str) {
        conn.execute(
            "INSERT INTO cards (serial, user_id, wizard_id, edition, drop_id, ts, origin, original_owner, status)
             VALUES (?1, ?2, ?3, ?4, NULL, 0, 'caught', ?2, ?5)",
            rusqlite::params![serial, user as i64, wizard_id, serial, status],
        )
        .expect("card");
    }

    #[test]
    fn only_cards_still_owned_and_only_the_houses_own_members_are_counted() {
        let conn = store::tests::memory();
        let wizards = store::wizards(&conn);
        assert_eq!(wizards.len(), 10, "the ten starters are seeded");
        // Two in the house hold a card each; one of the first member's copies
        // has been handed in for a set, and an outsider holds one too.
        store_card(&conn, 1, 111, wizards[0].id, "owned");
        store_card(&conn, 2, 111, wizards[9].id, "spent");
        store_card(&conn, 3, 222, wizards[5].id, "owned");
        store_card(&conn, 4, 999, wizards[9].id, "owned");

        let held = store::cards_of_users(&conn, &[111, 222]);
        assert_eq!(held.iter().map(|c| c.serial).collect::<Vec<_>>(), vec![1, 3], "spent copies and outsiders are out");
        let text = by_card_text(&wizards, &held, DESCRIPTION_LIMIT);
        assert!(text.starts_with("**2 cards** · **2 of 10** collected · **2** collectors · **no full set yet**"), "{}", text);
        // The Phoenix was spent, so the house has none - not "one, somewhere".
        assert!(text.contains("**The Eternal Phoenix** · Legendary — ❌ **missing**"), "{}", text);
        assert_eq!(full_sets(&wizards, &held), 0);
        assert!(store::cards_of_users(&conn, &[]).is_empty(), "a house with nobody in it holds nothing");
    }

    #[test]
    fn someone_with_no_house_is_told_why_and_a_mod_is_told_what_to_do_about_it() {
        let member = no_house_text(false);
        assert!(member.contains("not in a house"), "{}", member);
        assert!(member.contains("`/frogs`"), "{}", member);
        assert!(!member.contains("house:"), "a member is not sent to name a house: {}", member);
        let moderator = no_house_text(true);
        assert!(moderator.contains("`/housecards house:`"), "{}", moderator);
    }

    #[test]
    fn the_member_view_lists_collectors_most_cards_first_and_leaves_the_empty_handed_out() {
        let game = game();
        let cards = pile(&game, &[(7, 111, 9), (4, 111, 0), (11, 111, 0), (3, 222, 5), (9, 333, 1), (14, 333, 6)]);
        let text = by_member_text(&game, &cards, DESCRIPTION_LIMIT);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "**3 collectors** · **6 cards** between them");
        assert_eq!(lines[1], "<@111> — **3 cards** · 🔥 Eternal Phoenix #0007 · 🥛 Moonstruck Mirela ×2");
        assert_eq!(lines[2], "<@333> — **2 cards** · 🍫 Merlin #0014 · 🥛 Luna Lovegood #0009");
        assert_eq!(lines[3], "<@222> — **1 card** · 🍫 Alchemist Ferro #0003");
        assert_eq!(lines.len(), 4, "nobody card-less is listed:\n{}", text);
        assert!(!text.contains("🏆"), "nobody here holds a whole set:\n{}", text);
    }

    #[test]
    fn the_member_view_marks_whoever_could_sell_a_set_on_their_own() {
        let game = game();
        let rows: Vec<(i64, u64, usize)> = (0..10).map(|w| (w as i64 + 1, 111, w)).chain([(50, 222, 0)]).collect();
        let cards = pile(&game, &rows);
        let text = by_member_text(&game, &cards, DESCRIPTION_LIMIT);
        assert!(text.starts_with("**2 collectors** · **11 cards** between them · **1** can `/sellset` alone 🏆"), "{}", text);
        assert!(text.contains("<@111> 🏆 — **10 cards**"), "{}", text);
        assert!(text.contains("+2 more"), "a full set is capped at eight cards named:\n{}", text);
        assert!(!text.contains("<@222> 🏆"), "{}", text);
    }

    #[test]
    fn a_house_full_of_collectors_drops_whole_members_rather_than_overflow() {
        let (game, cards) = a_big_house();
        let text = by_member_text(&game, &cards, DESCRIPTION_LIMIT);
        assert!(text.chars().count() <= DESCRIPTION_LIMIT, "{} chars", text.chars().count());
        assert!(text.starts_with("**144 collectors** · "), "{}", text);
        let last = text.lines().last().unwrap();
        assert!(last.starts_with("-# +") && last.ends_with(" more collectors"), "{}", last);
        // The people shown are the ones holding the most.
        let shown = text.lines().count() - 2;
        assert!(shown > 3, "some collectors are still named: {}", shown);
    }

    #[test]
    fn an_empty_house_says_so_in_both_views() {
        let game = game();
        let by_card = by_card_text(&game, &[], DESCRIPTION_LIMIT);
        assert!(by_card.starts_with("**0 cards** · **0 of 10** collected · **0** collectors · **no full set yet**"), "{}", by_card);
        assert_eq!(by_card.matches("❌ **missing**").count(), 10, "{}", by_card);
        let by_member = by_member_text(&game, &[], DESCRIPTION_LIMIT);
        assert_eq!(by_member, "**0 collectors** · **0 cards** between them\nNobody in the house is holding a card yet.");
    }

    #[test]
    fn nothing_in_play_is_said_plainly_rather_than_left_blank() {
        assert!(by_card_text(&[], &[], DESCRIPTION_LIMIT).ends_with("There are no cards in play right now."));
    }

    #[test]
    fn the_view_option_reads_by_card_unless_a_member_view_is_asked_for() {
        assert_eq!(View::from_key("member"), View::ByMember);
        assert_eq!(View::from_key("card"), View::ByCard);
        assert_eq!(View::from_key(""), View::ByCard);
    }
}
