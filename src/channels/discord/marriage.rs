//! Marriages, adoptions and the gestures around them.
//!
//! Open to everybody on the server. It pays NOTHING and must never be able to:
//! the moment a marriage is worth a point, people marry for points and the
//! family tree stops meaning anything. Nothing here touches the ledger.
//!
//! CONSENT IS THE WHOLE DESIGN. A proposal is an offer with two buttons and
//! nothing happens until the other person presses one. A refusal can be made
//! permanent, so somebody being asked every evening can end it themselves.
//! Leaving, by contrast, is one-sided and immediate - needing permission to
//! leave is how people end up stuck in a joke they stopped finding funny.
use std::collections::HashMap;

use super::marriage_store::{self as store, Union};

/// The line a reply ends with: that member's own household on the page.
///
/// One door, like the Cup's: a command cannot forget the link, and the handle
/// means a link somebody pastes carries nothing about their account.
pub fn with_tree(user: u64, mut text: String) -> String {
    let Some(base) = super::month::live_url() else { return text };
    let root = base.strip_suffix("/live").unwrap_or(&base);
    let handle = super::control::web::housecup::handle(user);
    let line = format!("\n-# 🌳 Your family: {}/love/u/{}", root, handle);
    if !text.contains("/love/u/") {
        text.push_str(&line);
    }
    text
}

/// Where the cards go, `VIZIER_LOVE_CHANNEL`. The commands work anywhere; the
/// cards land in one room, so a busy channel is never buried in hearts.
pub fn channel() -> Option<u64> {
    super::control::id("VIZIER_LOVE_CHANNEL").filter(|id| *id != 0)
}

pub fn on() -> bool {
    super::control::on("VIZIER_LOVE", false)
}

/// How long before the same pair may marry again, `VIZIER_LOVE_REMARRY_HOURS`.
fn remarry_gap() -> i64 {
    super::control::number("VIZIER_LOVE_REMARRY_HOURS", 24).clamp(0, 720) as i64 * 3600
}

/// Between two gestures from one person, `VIZIER_LOVE_GESTURE_SECS`.
fn gesture_gap() -> i64 {
    super::control::number("VIZIER_LOVE_GESTURE_SECS", 20).clamp(0, 3600) as i64
}

/// Why a proposal cannot go ahead, in the words the asker is shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    Themselves,
    Already,
    TooSoon { hours: i64 },
    NotWanted,
    ABot,
}

impl Refusal {
    pub fn says(&self) -> String {
        match self {
            Refusal::Themselves => "You cannot marry yourself. Admirable self-regard, though.".into(),
            Refusal::Already => "You two are already married.".into(),
            Refusal::TooSoon { hours } => format!(
                "You two only just divorced. Give it {} hour{} before going round again.",
                hours,
                if *hours == 1 { "" } else { "s" }
            ),
            Refusal::NotWanted => "They have asked not to be proposed to again.".into(),
            Refusal::ABot => "Bots are not available. Loduchand is flattered.".into(),
        }
    }
}

/// Whether a proposal may be put to somebody.
///
/// `last_divorce` is when this pair last ended, if they ever did.
pub fn may_propose(
    asker: u64,
    target: u64,
    a_bot: bool,
    already: bool,
    refused: bool,
    last_divorce: Option<i64>,
    now: i64,
) -> Result<(), Refusal> {
    if asker == target {
        return Err(Refusal::Themselves);
    }
    if a_bot {
        return Err(Refusal::ABot);
    }
    if already {
        return Err(Refusal::Already);
    }
    if refused {
        return Err(Refusal::NotWanted);
    }
    if let Some(ended) = last_divorce {
        let gap = remarry_gap();
        if now - ended < gap {
            let left = ((gap - (now - ended)) as f64 / 3600.0).ceil() as i64;
            return Err(Refusal::TooSoon { hours: left.max(1) });
        }
    }
    Ok(())
}

/// Whether an adoption may go ahead: the same consent, plus the loop check.
pub fn may_adopt(parent: u64, child: u64, a_bot: bool, kin: &[(u64, u64)], refused: bool) -> Result<(), Refusal> {
    if parent == child || store::would_loop(kin, parent, child) {
        return Err(Refusal::Themselves);
    }
    if a_bot {
        return Err(Refusal::ABot);
    }
    if refused {
        return Err(Refusal::NotWanted);
    }
    Ok(())
}

/// Seconds left before somebody may make another gesture.
pub fn gesture_wait(last: Option<i64>, now: i64) -> i64 {
    match last {
        Some(then) => (gesture_gap() - (now - then)).max(0),
        None => 0,
    }
}

/// How long two people have been married, in the words a card uses.
pub fn how_long(since: i64, now: i64) -> String {
    let days = ((now - since).max(0)) / 86_400;
    match days {
        0 => "married today".to_string(),
        1 => "married yesterday".to_string(),
        d if d < 30 => format!("married {} days", d),
        d if d < 60 => "married a month".to_string(),
        d if d < 365 => format!("married {} months", d / 30),
        d => format!("married {} year{}", d / 365, if d / 365 == 1 { "" } else { "s" }),
    }
}

/// Which anniversaries are worth saying something about: every month, and then
/// the year. A card every single day would be noise nobody reads.
pub fn anniversary_today(since: i64, now: i64) -> Option<String> {
    let days = ((now - since).max(0)) / 86_400;
    if days == 0 || days % 30 != 0 {
        return None;
    }
    let months = days / 30;
    Some(match months {
        12 => "a year today".to_string(),
        m if m % 12 == 0 => format!("{} years today", m / 12),
        1 => "one month today".to_string(),
        m => format!("{} months today", m),
    })
}

/// Who a spouse has been replying to most, for `/love jealous`.
///
/// Reads the nightly ship sheet - the same numbers the ship percentage is built
/// from - so the card names somebody real rather than inventing a rival. The
/// asker themselves is skipped: "your spouse talks to you most" is not jealousy,
/// it is reassurance, and it is said as such.
pub fn rival(partners: &[(u64, u32)], asker: u64, spouse_replies: i64) -> Option<(u64, u32, u32)> {
    let (who, n) = partners.iter().filter(|(id, _)| *id != asker).max_by_key(|(_, n)| *n).copied()?;
    if n == 0 || spouse_replies <= 0 {
        return None;
    }
    let share = ((n as f64 / spouse_replies as f64) * 100.0).round().min(100.0) as u32;
    Some((who, n, share))
}

/// A family as the card and the page read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Family {
    pub spouses: Vec<Union>,
    pub parents: Vec<u64>,
    pub children: Vec<u64>,
    /// Their spouses' parents - in-laws, which is the joke that writes itself.
    pub in_laws: Vec<u64>,
}

/// Everyone a card should draw around one person.
pub fn family_of(conn: &rusqlite::Connection, user: u64) -> Family {
    let spouses = store::spouses(conn, user);
    let mut in_laws = Vec::new();
    for s in &spouses {
        for p in store::parents(conn, s.other) {
            if !in_laws.contains(&p) {
                in_laws.push(p);
            }
        }
    }
    Family { parents: store::parents(conn, user), children: store::children(conn, user), in_laws, spouses }
}

/// The households on the server, biggest first: every group of people joined by
/// a marriage or an adoption.
pub fn households(unions: &[(u64, u64, i64)], kin: &[(u64, u64)]) -> Vec<Vec<u64>> {
    let mut near: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut join = |a: u64, b: u64| {
        near.entry(a).or_default().push(b);
        near.entry(b).or_default().push(a);
    };
    for (a, b, _) in unions {
        join(*a, *b);
    }
    for (p, c) in kin {
        join(*p, *c);
    }
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<Vec<u64>> = Vec::new();
    let mut keys: Vec<u64> = near.keys().copied().collect();
    keys.sort_unstable();
    for start in keys {
        if seen.contains(&start) {
            continue;
        }
        let mut house = Vec::new();
        let mut stack = vec![start];
        while let Some(at) = stack.pop() {
            if !seen.insert(at) {
                continue;
            }
            house.push(at);
            stack.extend(near.get(&at).into_iter().flatten().copied());
        }
        house.sort_unstable();
        out.push(house);
    }
    out.sort_by_key(|h| std::cmp::Reverse(h.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn a_proposal_is_refused_for_the_reasons_a_member_would_try() {
        let now = 10 * DAY;
        assert_eq!(may_propose(1, 1, false, false, false, None, now), Err(Refusal::Themselves));
        assert_eq!(may_propose(1, 2, true, false, false, None, now), Err(Refusal::ABot));
        assert_eq!(may_propose(1, 2, false, true, false, None, now), Err(Refusal::Already));
        assert_eq!(may_propose(1, 2, false, false, true, None, now), Err(Refusal::NotWanted));
        // Divorced an hour ago: not yet.
        match may_propose(1, 2, false, false, false, Some(now - 3600), now) {
            Err(Refusal::TooSoon { hours }) => assert_eq!(hours, 23),
            other => panic!("should have been too soon: {:?}", other),
        }
        // Divorced last week: go ahead.
        assert_eq!(may_propose(1, 2, false, false, false, Some(now - 7 * DAY), now), Ok(()));
        assert!(may_propose(1, 2, false, false, false, None, now).is_ok());
    }

    #[test]
    fn an_adoption_cannot_close_a_loop() {
        let kin = vec![(1, 2), (2, 3)];
        assert_eq!(may_adopt(3, 1, false, &kin, false), Err(Refusal::Themselves));
        assert_eq!(may_adopt(1, 4, false, &kin, false), Ok(()));
    }

    #[test]
    fn how_long_reads_like_a_person_wrote_it() {
        let now = 400 * DAY;
        assert_eq!(how_long(now, now), "married today");
        assert_eq!(how_long(now - DAY, now), "married yesterday");
        assert_eq!(how_long(now - 4 * DAY, now), "married 4 days");
        assert_eq!(how_long(now - 45 * DAY, now), "married a month");
        assert_eq!(how_long(now - 200 * DAY, now), "married 6 months");
        assert_eq!(how_long(now - 400 * DAY, now), "married 1 year");
    }

    #[test]
    fn an_anniversary_is_monthly_and_then_yearly_and_otherwise_quiet() {
        let since = 0;
        assert_eq!(anniversary_today(since, 0), None, "the wedding day is not an anniversary");
        assert_eq!(anniversary_today(since, 29 * DAY), None, "and nor is every other day");
        assert_eq!(anniversary_today(since, 30 * DAY).as_deref(), Some("one month today"));
        assert_eq!(anniversary_today(since, 90 * DAY).as_deref(), Some("3 months today"));
        assert_eq!(anniversary_today(since, 360 * DAY).as_deref(), Some("a year today"));
        assert_eq!(anniversary_today(since, 720 * DAY).as_deref(), Some("2 years today"));
    }

    #[test]
    fn the_rival_is_somebody_real_and_never_the_asker() {
        // The spouse sent 100 replies: 60 to the asker, 30 to one other, 10 to another.
        let partners = vec![(1, 60), (7, 30), (9, 10)];
        let (who, n, share) = rival(&partners, 1, 100).expect("a rival");
        assert_eq!((who, n, share), (7, 30, 30), "the biggest that is not the asker");
        // Nobody else to name.
        assert_eq!(rival(&[(1, 60)], 1, 100), None);
        assert_eq!(rival(&[], 1, 100), None);
    }

    #[test]
    fn households_are_the_groups_the_page_draws() {
        // 1-2 married with a child 3; 4-5 married; 6 alone and in nothing.
        let unions = vec![(1, 2, 0), (4, 5, 0)];
        let kin = vec![(1, 3), (2, 3)];
        let houses = households(&unions, &kin);
        assert_eq!(houses.len(), 2, "6 is in no household at all");
        assert_eq!(houses[0], vec![1, 2, 3], "biggest first");
        assert_eq!(houses[1], vec![4, 5]);
    }
}
