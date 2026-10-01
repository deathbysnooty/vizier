//! Ravens: one drop mechanic for the whole month, and the twelve cards it
//! carries.
//!
//! The server had two things falling out of the sky - a Snitch worth points and
//! a Chocolate Frog worth a card - and nobody could ever say which one they
//! were chasing. The month replaces both with one: **a raven lands carrying a
//! sealed scroll.** Anyone can press *Capture*. Everyone who does gets the same
//! riddle privately, and the first right answer takes the scroll, the card and
//! the points.
//!
//! That is, line for line, what the Chocolate Frog already did, which is the
//! whole reason it is the mechanic that survived: the drop planner, the private
//! modal, the three tries, the one-transaction catch that cannot make two
//! winners, the serial numbers, the trading - all of it is reused exactly as it
//! stands in [`super::frog`] and [`super::frog_store`]. Nothing here
//! reimplements any of it. What this module is, is the **deck** and the
//! **conversion**.
//!
//! THE CONVERSION, AND WHY IT IS DONE THIS WAY. People own cards. Some have
//! owned No. 0001 since the first week, and a card's serial number is the only
//! thing about it that can't be earned again. So the twelve Westeros cards are
//! not new rows: ten of them are the ten wizards RENAMED IN PLACE, keeping the
//! same `wizards.id`. Every row in `cards` points at that id, so every serial,
//! every owner, every edition number and every trade in the history book comes
//! across untouched - a member who held *The Eternal Phoenix* No. 0187 now
//! holds *The Faceless Man* No. 0187. Two cards have no wizard to inherit from
//! and are inserted fresh. Anything in the table that is not in the deck is
//! switched off rather than deleted: it stops dropping, and the people who own
//! one keep it.
//!
//! The art is referenced by the slug: `frogcards/card-stark.png` and so on. The
//! files are dropped in later; until then a card posts without a picture, which
//! is exactly what an unillustrated wizard already did.

use rusqlite::{Connection, OptionalExtension, params};

use super::frog_store::{self as store, Rarity};

/// One card in the deck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Also the art file's name: `frogcards/<slug>.png`.
    pub slug: &'static str,
    pub name: &'static str,
    pub rarity: Rarity,
    /// The old wizard whose row - and therefore whose serials and owners - this
    /// card takes over. `None` for the two that are new.
    pub from: Option<&'static str>,
}

/// Twelve cards: four of the great houses common, four more uncommon, two rare,
/// and two that almost nobody will ever hold.
pub const DECK: [Entry; 12] = [
    Entry { slug: "card-stark", name: "House Stark", rarity: Rarity::Common, from: Some("hagrid") },
    Entry { slug: "card-lannister", name: "House Lannister", rarity: Rarity::Common, from: Some("luna") },
    Entry { slug: "card-targaryen", name: "House Targaryen", rarity: Rarity::Common, from: Some("hermione") },
    Entry { slug: "card-watch", name: "The Night's Watch", rarity: Rarity::Common, from: Some("sirius") },
    Entry { slug: "card-baratheon", name: "House Baratheon", rarity: Rarity::Uncommon, from: Some("flamel") },
    Entry { slug: "card-greyjoy", name: "House Greyjoy", rarity: Rarity::Uncommon, from: Some("merlin") },
    Entry { slug: "card-tyrell", name: "House Tyrell", rarity: Rarity::Uncommon, from: Some("moonkeeper") },
    Entry {
        slug: "card-martell",
        name: "House Martell",
        rarity: Rarity::Uncommon,
        from: Some("original-chocolate-frog"),
    },
    Entry { slug: "card-tully", name: "House Tully", rarity: Rarity::Rare, from: Some("bloomweaver") },
    Entry { slug: "card-arryn", name: "House Arryn", rarity: Rarity::Rare, from: None },
    Entry { slug: "card-faceless", name: "The Faceless Man", rarity: Rarity::Legendary, from: Some("eternal-phoenix") },
    Entry { slug: "card-nightking", name: "The Night King", rarity: Rarity::Legendary, from: None },
];

/// The deck by slug.
pub fn entry(slug: &str) -> Option<&'static Entry> {
    DECK.iter().find(|e| e.slug == slug)
}

/// What one run of the conversion did, for the log and for the tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Converted {
    /// Wizards renamed in place, keeping their id - and so their cards.
    pub renamed: usize,
    /// Cards with nothing to inherit from, inserted fresh.
    pub added: usize,
    /// Cards that were already right: a second run does nothing.
    pub already: usize,
    /// Old cards switched off: they stop dropping, their owners keep them.
    pub retired: usize,
}

impl Converted {
    pub fn did_nothing(&self) -> bool {
        self.renamed == 0 && self.added == 0 && self.retired == 0
    }
}

fn wizard_id_of(conn: &Connection, slug: &str) -> Option<i64> {
    conn.query_row("SELECT id FROM wizards WHERE slug = ?1", params![slug], |r| r.get(0)).optional().ok().flatten()
}

/// Turns whatever deck is in the store into the twelve Westeros cards.
///
/// Safe to run on every start: it is written so that the second run reports
/// `already` for all twelve and changes nothing. Nothing is deleted, ever - the
/// one destructive-looking thing it does is set `enabled = 0` on a card that is
/// no longer in the deck, which stops it dropping and leaves every copy of it in
/// its owner's hands.
pub fn convert(conn: &Connection) -> rusqlite::Result<Converted> {
    let mut done = Converted::default();
    for (position, card) in DECK.iter().enumerate() {
        if let Some(id) = wizard_id_of(conn, card.slug) {
            // Already converted. Keep its name, rarity and position true to the
            // deck - a panel edit of a card's rarity is allowed to be undone by
            // the deck, because the deck is what the points table is written
            // against - but never touch its picture or whether it is on.
            conn.execute(
                "UPDATE wizards SET name = ?2, rarity = ?3, position = ?4 WHERE id = ?1",
                params![id, card.name, card.rarity.key(), position as i64],
            )?;
            done.already += 1;
            continue;
        }
        match card.from.and_then(|old| wizard_id_of(conn, old)) {
            // The whole point: the SAME row, so every serial and every owner in
            // `cards` comes across without being touched at all.
            Some(id) => {
                conn.execute(
                    "UPDATE wizards SET slug = ?2, name = ?3, rarity = ?4, image = '', position = ?5, enabled = 1
                     WHERE id = ?1",
                    params![id, card.slug, card.name, card.rarity.key(), position as i64],
                )?;
                done.renamed += 1;
            }
            None => {
                conn.execute(
                    "INSERT INTO wizards (slug, name, rarity, image, enabled, position) VALUES (?1, ?2, ?3, '', 1, ?4)",
                    params![card.slug, card.name, card.rarity.key(), position as i64],
                )?;
                done.added += 1;
            }
        }
    }
    // Anything left over is not in the deck. Off, not gone.
    let slugs: Vec<String> = DECK.iter().map(|e| e.slug.to_string()).collect();
    let marks = vec!["?"; slugs.len()].join(", ");
    let sql = format!("UPDATE wizards SET enabled = 0 WHERE enabled = 1 AND slug NOT IN ({})", marks);
    let mut stmt = conn.prepare(&sql)?;
    for (i, slug) in slugs.iter().enumerate() {
        stmt.raw_bind_parameter(i + 1, slug.as_str())?;
    }
    done.retired = stmt.raw_execute()?;
    store::meta_set(conn, "westeros_deck", "1")?;
    Ok(done)
}

/// Whether the deck has been converted already.
pub fn converted(conn: &Connection) -> bool {
    store::meta_get(conn, "westeros_deck").is_some()
}

/// Runs the conversion once, when the month is on, and says so in the log.
///
/// Called at startup. With the month off nothing happens at all, so a server
/// that never turns the month on never has its deck touched.
pub fn migrate() {
    if !super::month::running() {
        return;
    }
    let Some(db) = store::db() else { return };
    let conn = db.lock();
    match convert(&conn) {
        Ok(done) if done.did_nothing() => {
            tracing::info!("raven: the deck is already the twelve Westeros cards");
        }
        Ok(done) => tracing::info!(
            "raven: the deck is now Westeros - {} cards renamed in place (serials and owners kept), {} added, {} old cards switched off",
            done.renamed,
            done.added,
            done.retired
        ),
        Err(err) => tracing::error!("raven: the deck was not converted ({}) - the old cards are still in play", err),
    }
}

// --- the words ------------------------------------------------------------------------

/// What lands in the channel.
pub const LANDS: &str = "A raven lands, carrying a sealed scroll.";

/// The button on it.
pub const CAPTURE: &str = "Capture the raven";

/// What the deck is worth, as a line for the rules and the panel.
pub fn deck_line() -> String {
    let one = |r: Rarity| {
        let n = DECK.iter().filter(|e| e.rarity == r).count();
        format!("{} {} ×{} — **{}**", r.emoji(), r.name(), n, r.points())
    };
    Rarity::ALL.into_iter().map(one).collect::<Vec<_>>().join(" · ")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// A frog store exactly as a live one looks before the month: the ten
    /// starter wizards, and cards in people's hands.
    fn before() -> Connection {
        let conn = Connection::open_in_memory().expect("memory db");
        store::init(&conn).expect("frog schema");
        conn
    }

    fn give(conn: &Connection, slug: &str, serial: i64, edition: i64, owner: u64) {
        let id = wizard_id_of(conn, slug).unwrap_or_else(|| panic!("no wizard {}", slug));
        conn.execute(
            "INSERT INTO cards (serial, user_id, wizard_id, edition, drop_id, ts, origin, original_owner, status)
             VALUES (?1, ?2, ?3, ?4, NULL, 1, 'caught', ?2, 'owned')",
            params![serial, owner as i64, id, edition],
        )
        .expect("a card");
    }

    fn holdings(conn: &Connection) -> Vec<(i64, u64, String)> {
        conn.prepare(
            "SELECT c.serial, c.user_id, w.name FROM cards c JOIN wizards w ON w.id = c.wizard_id ORDER BY c.serial",
        )
        .and_then(|mut s| {
            s.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get::<_, String>(2)?)))?.collect()
        })
        .unwrap_or_default()
    }

    #[test]
    fn the_deck_is_twelve_cards_with_no_two_the_same() {
        assert_eq!(DECK.len(), 12);
        let slugs: BTreeSet<&str> = DECK.iter().map(|e| e.slug).collect();
        assert_eq!(slugs.len(), 12, "two cards share a slug - and therefore a picture");
        let names: BTreeSet<&str> = DECK.iter().map(|e| e.name).collect();
        assert_eq!(names.len(), 12);
        let from: Vec<&str> = DECK.iter().filter_map(|e| e.from).collect();
        assert_eq!(from.iter().collect::<BTreeSet<_>>().len(), from.len(), "two cards claim the same old wizard");
        let count = |r: Rarity| DECK.iter().filter(|e| e.rarity == r).count();
        assert_eq!(
            (count(Rarity::Common), count(Rarity::Uncommon), count(Rarity::Rare), count(Rarity::Legendary)),
            (4, 4, 2, 2),
            "four, four, two and two"
        );
        // Every slug is a usable file name, because it IS the file name.
        for card in &DECK {
            assert!(
                card.slug.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{} would not find frogcards/{}.png",
                card.slug,
                card.slug
            );
            assert!(card.slug.starts_with("card-"));
        }
    }

    #[test]
    fn the_rarer_the_card_the_more_it_pays_and_the_harder_the_riddle() {
        let points: Vec<i64> = Rarity::ALL.into_iter().map(|r| r.points()).collect();
        assert_eq!(points, vec![2, 5, 10, 25], "the month's deck: 2, 5, 10, 25");
        assert!(points.windows(2).all(|w| w[0] < w[1]), "rarer must always be worth more");
        let weights: Vec<u64> = Rarity::ALL.into_iter().map(|r| r.weight()).collect();
        assert!(weights.windows(2).all(|w| w[0] > w[1]), "rarer must always drop less often: {:?}", weights);
        assert_eq!(Rarity::Common.difficulty(), "easy");
        assert_eq!(Rarity::Uncommon.difficulty(), "medium");
        assert_eq!(Rarity::Rare.difficulty(), "hard", "a rare card is guarded by a hard riddle");
        assert_eq!(Rarity::Legendary.difficulty(), "hard");
    }

    /// The one that matters: nobody loses a serial number.
    #[test]
    fn every_existing_card_converts_across_with_its_serial_and_its_owner() {
        let conn = before();
        // Three people's collections, as they stood the night before.
        give(&conn, "eternal-phoenix", 187, 3, 1234);
        give(&conn, "hagrid", 1, 1, 99);
        give(&conn, "merlin", 42, 7, 99);
        give(&conn, "bloomweaver", 7, 2, 500);
        let serials_before = holdings(&conn);
        assert_eq!(serials_before.len(), 4);

        let done = convert(&conn).unwrap();
        assert_eq!(done.renamed, 10, "all ten wizards were renamed in place");
        assert_eq!(done.added, 2, "and the two with nothing to inherit were added");
        assert_eq!(done.retired, 0, "nothing was left over to switch off");

        let after = holdings(&conn);
        assert_eq!(after.len(), 4, "not one card was lost");
        assert_eq!(
            after.iter().map(|(serial, owner, _)| (*serial, *owner)).collect::<Vec<_>>(),
            serials_before.iter().map(|(serial, owner, _)| (*serial, *owner)).collect::<Vec<_>>(),
            "every serial is still in the same hands"
        );
        // And each one is now the card that took its row over.
        let named = |serial: i64| after.iter().find(|(s, _, _)| *s == serial).map(|(_, _, n)| n.clone()).unwrap();
        assert_eq!(named(187), "The Faceless Man", "the phoenix's No. 0187 is the Faceless Man's No. 0187");
        assert_eq!(named(1), "House Stark");
        assert_eq!(named(42), "House Greyjoy");
        assert_eq!(named(7), "House Tully");
        // The editions came across too: the third phoenix is the third Faceless Man.
        let edition: i64 =
            conn.query_row("SELECT edition FROM cards WHERE serial = 187", [], |r| r.get(0)).unwrap();
        assert_eq!(edition, 3);
    }

    #[test]
    fn the_whole_deck_is_in_play_afterwards_and_nothing_else_is() {
        let conn = before();
        // A card somebody added by hand from the panel, which is not in the deck.
        store::create_wizard(&conn, "Dornish Spear", Rarity::Uncommon, "", true).unwrap();
        let done = convert(&conn).unwrap();
        assert_eq!(done.retired, 1, "the hand-made card was switched off");
        let live: Vec<(String, Rarity)> =
            store::wizards(&conn).into_iter().filter(|w| w.enabled).map(|w| (w.slug, w.rarity)).collect();
        assert_eq!(live.len(), 12, "twelve cards in play, no more and no fewer");
        for card in &DECK {
            assert!(live.iter().any(|(slug, r)| slug == card.slug && *r == card.rarity), "{} is not in play", card.slug);
        }
        // Off, not gone: whoever owned a spear still owns it.
        let spear: i64 = conn
            .query_row("SELECT COUNT(*) FROM wizards WHERE name = 'Dornish Spear'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(spear, 1, "nothing is ever deleted");
        // Rarest last, as the collection lists them.
        let order: Vec<Rarity> = store::wizards(&conn).into_iter().filter(|w| w.enabled).map(|w| w.rarity).collect();
        let rank = |r: Rarity| Rarity::ALL.iter().position(|x| *x == r).unwrap_or(0);
        assert!(order.windows(2).all(|w| rank(w[0]) <= rank(w[1])), "the deck should read commonest first");
    }

    #[test]
    fn converting_twice_changes_nothing() {
        let conn = before();
        give(&conn, "hagrid", 1, 1, 99);
        convert(&conn).unwrap();
        let after_once = holdings(&conn);
        let again = convert(&conn).unwrap();
        assert_eq!(again.already, 12, "all twelve were already right");
        assert!(again.did_nothing(), "a second run must be a no-op: {:?}", again);
        assert_eq!(holdings(&conn), after_once);
        assert!(converted(&conn));
    }

    #[test]
    fn a_server_with_no_cards_at_all_still_gets_the_deck() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(store::SCHEMA).unwrap();
        let done = convert(&conn).unwrap();
        assert_eq!((done.renamed, done.added), (0, 12), "nothing to inherit, so all twelve are new");
        assert_eq!(store::wizards(&conn).len(), 12);
    }

    /// The capture path, end to end, on the converted deck: a raven lands, three
    /// people try, one of them is right, and the card they take has a serial.
    #[test]
    fn a_raven_is_captured_by_the_first_right_answer_and_nobody_else() {
        let mut conn = before();
        convert(&conn).unwrap();
        conn.execute(
            "INSERT INTO riddles (id, topic, difficulty, riddle, answers, hint) VALUES
             ('r1', 'test', 'hard', 'What has a bite and no teeth?', '[\"winter\"]', '')",
            [],
        )
        .unwrap();
        let card = store::wizards(&conn).into_iter().find(|w| w.slug == "card-faceless").expect("the legendary");
        let riddle = store::riddle(&conn, "r1").expect("the riddle");
        let drop = store::start_drop(&conn, 555, &card, &riddle, 1_000, None).unwrap();
        store::mark_open(&conn, drop.id, 777, 1_000, 300).unwrap();

        // Three capture it. Two are wrong, one is right, and then it is gone.
        assert_eq!(store::submit(&mut conn, drop.id, 1, "Zoya", "summer", 1_010).unwrap(), store::Submit::Wrong { left: 2 });
        let won = store::submit(&mut conn, drop.id, 2, "Kabir", "Winter", 1_020).unwrap();
        let store::Submit::Won(win) = won else { panic!("the right answer should win: {:?}", won) };
        assert_eq!(win.drop.winner, Some(2));
        assert_eq!(win.canonical, "winter");
        assert!(win.serial > 0 && win.edition > 0, "the card is numbered");
        assert_eq!(
            store::submit(&mut conn, drop.id, 3, "Ira", "winter", 1_030).unwrap(),
            store::Submit::TooLate { winner_name: "Kabir".into() },
            "second right answer is too late - one scroll, one captor"
        );
        // Even the member who was wrong first can't come back and take it.
        assert_eq!(
            store::submit(&mut conn, drop.id, 1, "Zoya", "winter", 1_040).unwrap(),
            store::Submit::TooLate { winner_name: "Kabir".into() }
        );
        // And it is in their hands, by name and by number.
        let theirs = store::cards_of(&conn, 2);
        assert_eq!(theirs.len(), 1);
        assert_eq!((theirs[0].wizard_name.as_str(), theirs[0].rarity), ("The Faceless Man", Rarity::Legendary));
        assert_eq!(theirs[0].serial, win.serial);
    }

    #[test]
    fn the_deck_line_names_all_four_rarities() {
        let line = deck_line();
        for rarity in Rarity::ALL {
            assert!(line.contains(rarity.name()), "{} is missing from {}", rarity.name(), line);
        }
        assert!(line.contains("×4") && line.contains("×2"));
    }
}
