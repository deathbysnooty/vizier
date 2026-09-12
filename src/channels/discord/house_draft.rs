//! The house draft: dealing everyone already here into the four houses.
//!
//! The houses feed a monthly points competition, so they have to start even in
//! STRENGTH, not just in headcount - four houses of 180 are no use if one of
//! them holds every quiz winner. So each member is measured on the four things
//! the points will actually come from, ranked, and then dealt out in
//! serpentine order.
//!
//! Two decisions worth keeping:
//!
//! Ranks, not raw numbers. The measures have no common unit, and any weighting
//! ("one voice hour = ten messages") would be a number I invented. A rank says
//! only "third busiest, seventh on quiz", which is all the dealing needs.
//!
//! Serpentine order (1234 4321 1234 ...), not round-robin. Dealing 1234 over
//! and over hands the first house the best of every group of four. Turning
//! back each round cancels that out exactly: over any eight picks each house
//! collects the same total of rank positions.

use std::collections::HashMap;

/// What one person brings to a house, before any of it becomes a rank.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Activity {
    /// Messages sent, outside the excluded channels.
    pub msgs: u64,
    /// Voice log entries - a proxy for time spent in rooms. The exact hours
    /// need the join/leave/switch pairing that /awards does, and for ORDERING
    /// people the count of entries is enough.
    pub voice: u64,
    /// Questions answered first in the quiz.
    pub quiz: u64,
    /// Fights and battles won in the arena.
    pub wins: u64,
}

impl Activity {
    /// True when we have never seen this person do anything. They still get a
    /// house; they just can't tip the balance of one.
    pub fn silent(&self) -> bool {
        self.msgs == 0 && self.voice == 0 && self.quiz == 0 && self.wins == 0
    }
}

/// Everyone in draft order, strongest first.
///
/// Each measure becomes that person's SHARE of the server's total for it, and
/// the four shares are added up.
///
/// Shares rather than rank positions, which is what this started as: ranking
/// punishes a specialist twice over. The quiz regular who never chats has zero
/// messages, so a rank puts them near the back of the field - it dealt one such
/// player 32nd of 41 - when they are precisely the member a house wants for a
/// points competition. A share only ever adds: hold a third of the quiz points
/// and that is a third of that measure, whatever you do elsewhere. And since
/// every measure's shares sum to 1 across the server, all four carry equal
/// weight without inventing an exchange rate between a message and a win.
///
/// Ties fall back to the user id, so the same data always deals the same draft
/// - one nobody can reproduce is one nobody can trust.
pub fn ranked(people: &HashMap<u64, Activity>) -> Vec<u64> {
    let measures: [fn(&Activity) -> u64; 4] = [|a| a.msgs, |a| a.voice, |a| a.quiz, |a| a.wins];
    let totals: Vec<f64> = measures.iter().map(|m| people.values().map(|a| m(a) as f64).sum()).collect();

    let mut scored: Vec<(u64, f64)> = people
        .iter()
        .map(|(user, activity)| {
            let score: f64 = measures
                .iter()
                .zip(&totals)
                // A measure nobody has done at all scores nothing for anyone.
                .map(|(measure, total)| if *total > 0.0 { measure(activity) as f64 / total } else { 0.0 })
                .sum();
            (*user, score)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(user, _)| user).collect()
}

/// Deals a ranked list into `houses` piles in serpentine order.
///
/// Returns one pile per house, in the order they were dealt.
pub fn snake(ranked: &[u64], houses: usize) -> Vec<Vec<u64>> {
    let mut piles = vec![Vec::new(); houses.max(1)];
    if houses == 0 {
        return piles;
    }
    for (position, user) in ranked.iter().enumerate() {
        let round = position / houses;
        let seat = position % houses;
        // Odd rounds run backwards, which is what keeps the piles even.
        let house = if round % 2 == 0 { seat } else { houses - 1 - seat };
        piles[house].push(*user);
    }
    piles
}

/// The sum of draft positions each pile received. Equal sums are the whole
/// point of dealing serpentine, so this is what a preview should show.
pub fn weights(ranked: &[u64], piles: &[Vec<u64>]) -> Vec<usize> {
    let position: HashMap<u64, usize> = ranked.iter().enumerate().map(|(i, u)| (*u, i)).collect();
    piles.iter().map(|pile| pile.iter().filter_map(|u| position.get(u)).sum()).collect()
}

/// Channels left out of the message count, from the same setting /awards uses.
fn excluded_channels() -> Vec<i64> {
    std::env::var("VIZIER_STATS_EXCLUDE_CHANNELS")
        .unwrap_or_default()
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|piece| piece.parse::<i64>().ok())
        .collect()
}

/// Reads what everyone has been doing since `since` (a unix time).
///
/// Only people who appear in one of the four sources show up here; the roster
/// decides who is actually drafted, and anyone missing counts as silent.
pub fn activity(since: i64) -> HashMap<u64, Activity> {
    let mut people: HashMap<u64, Activity> = HashMap::new();

    if let Some(db) = super::stats::db() {
        let conn = db.lock();
        let exclude = excluded_channels();
        let holes = exclude.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let day = chrono::DateTime::from_timestamp(since, 0)
            .unwrap_or_default()
            .with_timezone(&super::stats::ist())
            .format("%Y-%m-%d")
            .to_string();

        let sql = format!(
            "SELECT user_id, SUM(count) FROM msg_counts
             WHERE day >= ?1 AND channel_id NOT IN ({}) GROUP BY user_id",
            holes
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(day)];
        args.extend(exclude.into_iter().map(|id| Box::new(id) as Box<dyn rusqlite::ToSql>));
        if let Ok(mut stmt) = conn.prepare(&sql) {
            let bound = rusqlite::params_from_iter(args.iter().map(|a| a.as_ref()));
            if let Ok(rows) = stmt.query_map(bound, |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))) {
                for (user, msgs) in rows.flatten() {
                    people.entry(user).or_default().msgs = msgs;
                }
            }
        }

        let sql = "SELECT user_id, COUNT(*) FROM voice_events WHERE ts >= ?1 GROUP BY user_id";
        if let Ok(mut stmt) = conn.prepare(sql) {
            if let Ok(rows) =
                stmt.query_map([since], |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)))
            {
                for (user, voice) in rows.flatten() {
                    people.entry(user).or_default().voice = voice;
                }
            }
        }
    }

    for (user, quiz) in super::quiz::points_per_user(Some(since)) {
        people.entry(user).or_default().quiz = quiz;
    }
    for (user, wins) in super::battle::wins_per_user(Some(since)) {
        people.entry(user).or_default().wins = wins;
    }
    people
}

#[cfg(test)]
mod tests {
    use super::*;

    fn people(n: u64) -> HashMap<u64, Activity> {
        // A heavy tail, like a real server: a few people talk constantly.
        (1..=n)
            .map(|i| {
                let a = Activity {
                    msgs: 10_000 / i,
                    voice: 500 / i,
                    quiz: if i % 3 == 0 { 200 / i } else { 0 },
                    wins: if i % 7 == 0 { 20 / i } else { 0 },
                };
                (i, a)
            })
            .collect()
    }

    #[test]
    fn the_strongest_member_is_dealt_first_and_the_order_is_reproducible() {
        let mut people = people(50);
        people.insert(500, Activity { msgs: 99_999, voice: 9_999, quiz: 9_999, wins: 999 });
        let order = ranked(&people);
        assert_eq!(order.len(), 51);
        assert_eq!(order[0], 500, "the member ahead on all four measures should go first");
        assert_eq!(order, ranked(&people), "the same data must deal the same draft");
        // Nobody is dealt twice or dropped.
        let mut seen = order.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 51);
    }

    #[test]
    fn a_specialist_is_not_buried_behind_an_all_round_nobody() {
        // The reason this scores shares and not rank positions. Ranking dealt
        // the quiz-only player 32nd of 41, behind people who do a little of
        // nothing - and they are the member who will actually win house points.
        let mut people = people(40);
        people.insert(98, Activity { msgs: 0, voice: 0, quiz: 400, wins: 0 });
        people.insert(99, Activity { msgs: 3, voice: 1, quiz: 0, wins: 0 });
        let order = ranked(&people);
        let place = |user: u64| order.iter().position(|u| *u == user).expect("in the draft");
        assert!(place(98) < place(99), "specialist at {}, barely-there member at {}", place(98), place(99));
        assert!(place(98) < order.len() / 2, "specialist buried at {} of {}", place(98), order.len());
    }

    #[test]
    fn serpentine_dealing_gives_every_house_the_same_strength() {
        // The property that makes this fair: over any eight picks each house
        // collects the same total of draft positions, so with a multiple of
        // eight the four sums are EXACTLY equal.
        let order: Vec<u64> = (1..=400).collect();
        let piles = snake(&order, 4);
        let weights = weights(&order, &piles);
        assert_eq!(weights.iter().collect::<std::collections::HashSet<_>>().len(), 1, "uneven: {:?}", weights);

        // Round-robin, for contrast, hands the first house the best of every
        // four - this is what the serpentine order is avoiding.
        let flat: Vec<usize> = (0..4)
            .map(|h| order.iter().enumerate().filter(|(i, _)| i % 4 == h).map(|(i, _)| i).sum())
            .collect();
        assert!(flat[0] < flat[3], "round-robin should favour the first house");
    }

    #[test]
    fn the_piles_come_out_the_same_size_whatever_the_count() {
        for n in [0usize, 1, 3, 4, 7, 101, 399, 750] {
            let order: Vec<u64> = (1..=n as u64).collect();
            let piles = snake(&order, 4);
            let sizes: Vec<usize> = piles.iter().map(|p| p.len()).collect();
            let (small, big) = (sizes.iter().min().copied().unwrap(), sizes.iter().max().copied().unwrap());
            assert!(big - small <= 1, "{} members split {:?}", n, sizes);
            assert_eq!(sizes.iter().sum::<usize>(), n, "lost someone at n = {}", n);
        }
        // Whatever the count, the strongest four never land together: each
        // house opens with one of the first four picks, in order. (The position
        // sums only come out exactly equal on a full pair of rounds, which is
        // what the serpentine test covers.)
        let order: Vec<u64> = (1..=40).collect();
        let piles = snake(&order, 4);
        for (house, pile) in piles.iter().enumerate() {
            assert_eq!(pile.first(), order.get(house), "house {} should open with pick {}", house, house + 1);
        }
    }

    #[test]
    fn the_silent_are_still_dealt_a_house() {
        let mut people: HashMap<u64, Activity> = (1..=8).map(|i| (i, Activity::default())).collect();
        people.insert(1, Activity { msgs: 5, ..Activity::default() });
        assert!(people[&2].silent());
        let order = ranked(&people);
        assert_eq!(order.len(), 8);
        assert_eq!(order[0], 1, "the only active member goes first");
        let piles = snake(&order, 4);
        assert!(piles.iter().all(|p| p.len() == 2), "silent members still fill the houses");
    }
}
