//! The card the arena gives away.
//!
//! Winning in the lists earns one card from the month's deck - the melee's
//! champion and a duel's winner alike - on top of the house points the arena
//! already pays. There is no second card system here: the card comes out of the
//! same store the frogs are caught from, under a key that can only ever pay
//! once, so a fight that is replayed or a message that is resent never mints a
//! second copy.
//!
//! **The lists pay better than a riddle.** A frog caught in chat is mostly a
//! common; a card won in the lists leans uncommon and rare, with legendary rare
//! enough to be worth remembering. Those weights live here, with the thing
//! doing the paying, rather than in the store: [`WEIGHTS`] is what the arena
//! asks for and [`wanted`] is what a roll means. The store still mints the
//! card, numbers it and keeps the key that can only pay once, so this is the
//! same card system the frogs use and not a second one.

use chrono::Utc;

use super::control;
use super::frog_store as store;

/// How rare a card the lists are willing to pay out. The arena is the hardest
/// thing in the month to win, so it pays better than a riddle does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Want {
    Common,
    Uncommon,
    Rare,
    Legendary,
}

/// How often the lists pay out each rarity, out of a hundred.
pub const WEIGHTS: [(Want, u64); 4] =
    [(Want::Common, 15), (Want::Uncommon, 45), (Want::Rare, 32), (Want::Legendary, 8)];

/// What the lists pay out, by a roll in 0..1. Common is the consolation rather
/// than the norm, uncommon is the norm, rare is a real result, and legendary is
/// something people will remember.
pub fn wanted(roll: f64) -> Want {
    let total: u64 = WEIGHTS.iter().map(|(_, w)| w).sum();
    let mut target = ((roll.clamp(0.0, 1.0) * total as f64) as u64).min(total - 1);
    for (want, weight) in WEIGHTS {
        if target < weight {
            return want;
        }
        target -= weight;
    }
    Want::Legendary
}

impl Want {
    /// The deck's own name for it.
    fn rarity(self) -> store::Rarity {
        match self {
            Want::Common => store::Rarity::Common,
            Want::Uncommon => store::Rarity::Uncommon,
            Want::Rare => store::Rarity::Rare,
            Want::Legendary => store::Rarity::Legendary,
        }
    }
}

/// A card at the arena's own weights, out of whatever the deck actually holds:
/// a rarity nothing is printed at is skipped rather than handing back nothing.
fn pick(conn: &rusqlite::Connection, rarity_roll: f64, card_roll: f64) -> Option<store::Wizard> {
    let all: Vec<store::Wizard> = store::wizards(conn).into_iter().filter(|w| w.enabled).collect();
    let weights: Vec<(store::Rarity, u64)> = WEIGHTS
        .iter()
        .map(|(want, weight)| {
            let rarity = want.rarity();
            (rarity, if all.iter().any(|w| w.rarity == rarity) { *weight } else { 0 })
        })
        .collect();
    let rarity = store::choose_rarity(rarity_roll, &weights)?;
    let pool: Vec<&store::Wizard> = all.iter().filter(|w| w.rarity == rarity).collect();
    let index = ((card_roll.clamp(0.0, 1.0) * pool.len() as f64) as usize).min(pool.len().checked_sub(1)?);
    pool.get(index).map(|w| (*w).clone())
}

/// One card won in the lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prize {
    /// The card's own name. Once the deck is the month's, this is the house.
    pub house: String,
    /// "No. 0042", the one number nobody else will ever hold.
    pub number: String,
    /// Which copy of that card this is.
    pub edition: i64,
    /// The rarity's own mark, for the message.
    pub emoji: &'static str,
}

/// The one line the result message carries. It says the house, it says the
/// number, and it says the thing that makes a serial worth having.
pub fn prize_line(prize: &Prize) -> String {
    format!(
        "{} Won a card from the deck: **{}** · {} (copy #{}) — and nobody else will ever hold that one.",
        prize.emoji, prize.house, prize.number, prize.edition
    )
}

/// The key a fight's prize is paid under. One key per fight per winner, so the
/// same fight can never mint two cards however often it is replayed.
pub fn prize_key(kind: &str, fight: i64, winner: u64) -> String {
    format!("arena:{}:{}:{}", kind, fight, winner)
}

/// Whether the arena gives cards away at all. The same switch that already
/// turns the arena's earned cards on, so this is not a new economy.
fn on() -> bool {
    control::on("VIZIER_FROG_ROYALE_CARDS", true)
}

/// Awards one card to the winner of a fight and returns it. `kind` is `melee`
/// or `duel`, `fight` is that fight's own id, and `roll` is a number in 0..1.
///
/// `None` when the arena's cards are switched off, the store is not open, or
/// the deck has nothing to give - the fight still pays its points either way.
pub fn award(kind: &str, fight: i64, winner: u64, roll: f64) -> Option<Prize> {
    if !on() {
        return None;
    }
    let key = prize_key(kind, fight, winner);
    let what = store::AwardFor {
        kind: "arena",
        battle_id: Some(fight),
        role: kind.to_string(),
        ..Default::default()
    };
    let db = store::db()?;
    let mut conn = db.lock();
    // A second roll off the first, so one number decides both the rarity and
    // which card of it: the caller only has to find one.
    let card_roll = (roll * 7.0).fract();
    let now = Utc::now().timestamp();
    match store::award_card_with(
        &mut conn,
        &key,
        winner,
        &format!("arena_{}", kind),
        &what,
        |c| pick(c, roll, card_roll),
        now,
    ) {
        Ok(Some(award)) => Some(Prize {
            house: award.card.wizard_name.clone(),
            number: store::serial_label(award.card.serial),
            edition: award.card.edition,
            emoji: award.card.rarity.emoji(),
        }),
        Ok(None) => None,
        Err(err) => {
            tracing::warn!("battle: card {} for {} not given: {}", key, winner, err);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The weighting the owner asked for: uncommon and rare carry it, common is
    /// the consolation, legendary is rare enough to mean something.
    #[test]
    fn the_lists_pay_towards_the_better_end_of_the_deck() {
        let n = 10_000;
        let mut seen = std::collections::HashMap::new();
        for i in 0..n {
            *seen.entry(wanted(i as f64 / n as f64)).or_insert(0) += 1;
        }
        let share = |w: Want| seen.get(&w).copied().unwrap_or(0) as f64 / n as f64;
        assert!((0.10..0.20).contains(&share(Want::Common)), "common {}", share(Want::Common));
        assert!((0.40..0.50).contains(&share(Want::Uncommon)), "uncommon {}", share(Want::Uncommon));
        assert!((0.27..0.37).contains(&share(Want::Rare)), "rare {}", share(Want::Rare));
        assert!((0.04..0.12).contains(&share(Want::Legendary)), "legendary {}", share(Want::Legendary));
        // Better than common far more often than not.
        assert!(share(Want::Uncommon) + share(Want::Rare) + share(Want::Legendary) > 0.8);
        // And rare beats common, which is the whole point of an arena card.
        assert!(share(Want::Rare) > share(Want::Common));
    }

    #[test]
    fn a_roll_outside_the_range_still_names_a_rarity() {
        assert_eq!(wanted(-5.0), Want::Common);
        assert_eq!(wanted(0.0), Want::Common);
        assert_eq!(wanted(1.0), Want::Legendary);
        assert_eq!(wanted(99.0), Want::Legendary);
        // Every rarity the deck has is one the lists can pay, and the weights
        // are the ones the owner asked for.
        assert_eq!(WEIGHTS.iter().map(|(_, w)| w).sum::<u64>(), 100);
        let names: Vec<store::Rarity> = WEIGHTS.iter().map(|(w, _)| w.rarity()).collect();
        assert_eq!(names, store::Rarity::ALL.to_vec(), "the deck and the lists must mean the same four");
    }

    /// One key per fight per winner: that is what stops a replayed fight, or a
    /// message that goes out twice, minting a second copy of a card.
    #[test]
    fn one_fight_pays_one_card_to_one_winner() {
        assert_eq!(prize_key("melee", 17, 9), "arena:melee:17:9");
        assert_eq!(prize_key("duel", 17, 9), "arena:duel:17:9");
        let same = prize_key("melee", 17, 9);
        assert_eq!(same, prize_key("melee", 17, 9), "the same fight is the same key");
        assert_ne!(same, prize_key("melee", 18, 9), "another fight is another key");
        assert_ne!(same, prize_key("melee", 17, 10), "another winner is another key");
    }

    /// The announcement says the house, the number, and the thing that makes a
    /// serial worth winning.
    #[test]
    fn the_line_names_the_house_the_number_and_what_makes_it_one_of_one() {
        let prize = Prize {
            house: "Targaryen".to_string(),
            number: "No. 0042".to_string(),
            edition: 3,
            emoji: "\u{1f525}",
        };
        let line = prize_line(&prize);
        assert!(line.contains("Targaryen"), "{line}");
        assert!(line.contains("No. 0042"), "{line}");
        assert!(line.contains("copy #3"), "{line}");
        assert!(line.contains("nobody else will ever hold that one"), "{line}");
        assert!(line.starts_with('\u{1f525}'), "{line}");
        // Nothing in it names a fight style: the arena has one world.
        let lower = line.to_lowercase();
        for word in ["classic", "pokemon", "style", "theme"] {
            assert!(!lower.contains(word), "{line}");
        }
    }

    /// Against a real deck: the lists hand out every rarity the deck has, lean
    /// to the better end of it, and a key pays exactly once however often it
    /// is asked.
    #[test]
    fn a_winner_gets_one_card_from_the_whole_deck() {
        let mut conn = store::tests::memory();
        let what = store::AwardFor { kind: "arena", battle_id: Some(1), role: "duel".into(), ..Default::default() };
        let mut seen: std::collections::HashMap<store::Rarity, usize> = std::collections::HashMap::new();
        let n = 400;
        for i in 0..n {
            let roll = i as f64 / n as f64;
            let key = prize_key("duel", i as i64, 42);
            let award = store::award_card_with(
                &mut conn,
                &key,
                42,
                "arena_duel",
                &what,
                |c| pick(c, roll, (roll * 7.0).fract()),
                1_700_000_000 + i as i64,
            )
            .expect("the deck answers")
            .expect("a card worth winning");
            assert!(award.fresh, "every fight is its own card");
            assert_eq!(award.card.origin, "arena_duel");
            assert_eq!(award.card.user_id, 42);
            assert!(award.card.serial > 0, "a card without a number is not a card");
            *seen.entry(award.card.rarity).or_default() += 1;
        }
        // Every rarity the deck actually prints turns up - a rarity nothing is
        // printed at is skipped rather than handing back nothing, so the test
        // asks the deck what it has rather than assuming four.
        let printed: std::collections::HashSet<store::Rarity> =
            store::wizards(&conn).into_iter().filter(|w| w.enabled).map(|w| w.rarity).collect();
        assert!(printed.len() >= 3, "a deck of {} rarities proves little", printed.len());
        for rarity in &printed {
            assert!(seen.contains_key(rarity), "the lists never paid a {:?}: {:?}", rarity, seen);
        }
        assert!(seen.keys().all(|r| printed.contains(r)), "the lists paid a card the deck does not print");
        // The common one is not the norm: the lists are the hardest thing in
        // the month to win, and legendary is still the rarest of them.
        let common = seen.get(&store::Rarity::Common).copied().unwrap_or(0);
        let better = n - common;
        assert!(better > common * 3, "{} of {} were better than common", better, n);
        let legendary = seen.get(&store::Rarity::Legendary).copied().unwrap_or(0);
        assert!(legendary * 3 < n, "legendary came up {} times in {}", legendary, n);

        // The same key again is the same card, not a second one.
        let before = store::totals(&conn).cards;
        let again = store::award_card_with(
            &mut conn,
            &prize_key("duel", 0, 42),
            42,
            "arena_duel",
            &what,
            |c| pick(c, 0.9, 0.1),
            1_700_000_000,
        )
        .expect("the deck answers")
        .expect("the card it already paid");
        assert!(!again.fresh, "a replayed fight must not mint a second copy");
        assert_eq!(store::totals(&conn).cards, before, "and the deck is no lighter for asking");

        // And the line that goes out with it says the three things it must.
        let prize = Prize {
            house: again.card.wizard_name.clone(),
            number: store::serial_label(again.card.serial),
            edition: again.card.edition,
            emoji: again.card.rarity.emoji(),
        };
        let line = prize_line(&prize);
        assert!(line.contains(&again.card.wizard_name), "{line}");
        assert!(line.contains(&store::serial_label(again.card.serial)), "{line}");
        assert!(line.contains("nobody else will ever hold that one"), "{line}");
    }

    /// With no store open there is no card, and the fight carries on - a prize
    /// that cannot be paid must never stop a result being posted.
    #[test]
    fn no_store_means_no_card_and_no_panic() {
        if store::db().is_some() {
            return;
        }
        assert_eq!(award("melee", 1, 2, 0.9), None);
        assert_eq!(award("duel", 1, 2, 0.1), None);
    }
}
