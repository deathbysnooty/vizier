//! The themed month: one switch, one clock, one link, and the four houses
//! wearing a different set of names for as long as it runs.
//!
//! October is **Fire & Blood**. Nothing in here is specific to Westeros: the
//! names, the crests and the colours are all settings, so next month's theme is
//! a panel edit rather than a release. What IS specific is the shape of the
//! month, and that shape is the same every time:
//!
//! 1. **An egg week.** Everybody gets an egg. While it is unhatched only the
//!    games the egg is *hungry* for pay that member anything towards the month
//!    (see [`super::egg`]). Everything else runs exactly as it always did and
//!    keeps its own score - the month's points are the only thing being held
//!    back.
//! 2. **The hatch**, at [`hatch_at`]. Every egg opens at once, everyone is
//!    dealt a house so the four come out level on ACTIVITY rather than
//!    headcount (see [`super::hatch`]), and everyone gets a dragon.
//! 3. **The rest of the month**, where every game pays everybody again and the
//!    hungry games pay double - towards the same ceiling, so the bonus buys
//!    speed and never a higher score.
//!
//! WHY THE HOUSES ARE NOT REPLACED. The ledger stores a house KEY on every row
//! it has ever written, the Discord roles are real roles people are wearing, and
//! the channels are real channels. Renaming any of that would orphan every row
//! of September. So the four slots stay exactly as they are - `gryffindor`,
//! `slytherin`, `ravenclaw`, `hufflepuff` - and the theme is a coat of paint
//! read at the moment something is shown. Switch the month off and the paint
//! comes off with it; the points underneath never moved.
//!
//! Everything here is read at the moment it is needed, so the panel takes
//! effect without a restart, and every function that decides anything takes its
//! inputs rather than reading the clock, so the tests can hold time still.

use chrono::TimeZone;

use super::control;
use super::house::{HOUSES, House};

/// India is UTC+5:30 all year.
pub const IST_OFFSET: i64 = 5 * 3600 + 30 * 60;

/// The one switch for the whole themed month.
pub const KEY: &str = "VIZIER_MONTH";

/// True while the themed month is running. OFF by default: a server that never
/// turns it on plays exactly the month it played before.
pub fn running() -> bool {
    control::on("VIZIER_MONTH", false)
}

/// What the month is called, for the pages and the reveal.
pub fn title() -> String {
    control::var("VIZIER_MONTH_NAME").unwrap_or_else(|| "Fire & Blood".to_string())
}

// --- the clock ------------------------------------------------------------------

/// The default hatch: 8 October, noon, India time.
pub const HATCH_DEFAULT: &str = "2026-10-08 12:00";

/// Reads "YYYY-MM-DD HH:MM" as a moment in India time. `None` for anything that
/// isn't one, so a mistyped setting falls back rather than hatching at the epoch.
pub fn parse_ist(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    let naive = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M")
        .or_else(|_| chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M"))
        .ok()?;
    let ist = chrono::FixedOffset::east_opt(IST_OFFSET as i32)?;
    ist.from_local_datetime(&naive).single().map(|t| t.timestamp())
}

/// The moment every egg that has been waiting since the start opens.
pub fn hatch_at() -> i64 {
    control::var("VIZIER_MONTH_HATCH")
        .and_then(|raw| parse_ist(&raw))
        .or_else(|| parse_ist(HATCH_DEFAULT))
        .unwrap_or(0)
}

/// How long a LATE egg waits. Somebody who claims after the month has started
/// does not get a two-hour egg: theirs hatches this many days after they claim
/// it, so everyone gets the same week.
pub fn watch_days() -> i64 {
    control::number("VIZIER_MONTH_WATCH_DAYS", 7).clamp(1, 60) as i64
}

/// How long one egg's hunger lasts before it rotates.
pub fn craving_hours() -> i64 {
    control::number("VIZIER_MONTH_CRAVING_HOURS", 6).clamp(1, 24) as i64
}

/// How many games an egg is hungry for at once.
pub fn cravings() -> usize {
    control::number("VIZIER_MONTH_CRAVINGS", 2).clamp(1, 6) as usize
}

/// The hour the day's craving window is anchored to end at, India time.
///
/// The rotation's boundaries are shifted so one of them always lands here, so
/// "the double lasts until 8 pm" is a true sentence a member can plan around
/// rather than a rounded one. With the shipped six-hour rotation that puts the
/// boundaries at 2 am, 8 am, 2 pm and 8 pm.
pub fn double_hour() -> i64 {
    control::number("VIZIER_MONTH_DOUBLE_HOUR", 20).min(23) as i64
}

/// How long after the first right answer everybody else's right answer still
/// pays. The first one in still gets their bonus; this is the difference
/// between a game one fast person wins and a game everybody plays.
pub fn window_secs() -> i64 {
    control::number("VIZIER_MONTH_WINDOW_SECONDS", 90).clamp(0, 900) as i64
}

// --- the live page ----------------------------------------------------------------

/// The page the whole month is read on. Empty until a mod sets it.
pub fn live_url() -> Option<String> {
    let raw = control::var("VIZIER_LIVE_URL")?;
    let url = raw.trim().trim_end_matches('/').to_string();
    (url.starts_with("http://") || url.starts_with("https://")).then_some(url)
}

/// The one line every Cup reply ends with - the loud ones and the whispered
/// ones alike, because the whispered ones are the ones people read most.
///
/// Empty when no page is set, so nothing ever ends with a dangling "Live:".
pub fn live_line() -> String {
    match live_url() {
        Some(url) => format!("\n-# 🔴 Live: {}", url),
        None => String::new(),
    }
}

/// Puts the live line on the end of a reply. The one door, so "every Cup reply
/// ends with the link" is a thing one test can check rather than forty.
pub fn with_live(mut text: String) -> String {
    let line = live_line();
    if !line.is_empty() && !text.contains(line.trim_start_matches('\n')) {
        text.push_str(&line);
    }
    text
}

// --- the paint ----------------------------------------------------------------------

/// One house as this month shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Themed {
    /// The ledger key underneath, which never changes.
    pub key: &'static str,
    pub name: String,
    pub crest: String,
    pub colour: u32,
    /// The second colour. A banner needs two - a field and a hem - and so does
    /// a card, so the month names both rather than letting four callers each
    /// invent a different way of lightening the first.
    pub secondary: u32,
}

/// The shipped theme, in `HOUSES` order: Gryffindor's slot wears Stark,
/// Slytherin's wears Lannister, Ravenclaw's wears Targaryen, Hufflepuff's wears
/// the Night's Watch.
pub const THEME_NAMES: &str = "Stark,Lannister,Targaryen,Night's Watch";
pub const THEME_CRESTS: &str = "🐺,🦁,🐉,🗡️";
pub const THEME_COLOURS: &str = "6E7B8B,A8882B,8C1C1C,2B2F36";
pub const THEME_HEMS: &str = "C9D3DC,F0D98A,D96A6A,6E7681";

fn listed(raw: Option<String>, fallback: &str) -> Vec<String> {
    let text = raw.filter(|r| !r.trim().is_empty()).unwrap_or_else(|| fallback.to_string());
    text.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// A hex colour like `8C1C1C`, with or without a leading `#`.
fn hex(raw: &str) -> Option<u32> {
    u32::from_str_radix(raw.trim().trim_start_matches('#'), 16).ok().filter(|n| *n <= 0xFF_FFFF)
}

/// How one house is named, crested and coloured right now. With the month off
/// this is the house's own name, so every caller can use it unconditionally.
pub fn themed(house: &House) -> Themed {
    let second = |rgb: [u8; 3]| u32::from_be_bytes([0, rgb[0], rgb[1], rgb[2]]);
    let plain = Themed {
        key: house.key,
        name: house.name.to_string(),
        crest: house.crest.to_string(),
        colour: house.colour,
        secondary: second(house.colours.1),
    };
    if !running() {
        return plain;
    }
    let Some(slot) = HOUSES.iter().position(|h| h.key == house.key) else { return plain };
    let names = listed(control::var("VIZIER_MONTH_HOUSE_NAMES"), THEME_NAMES);
    let crests = listed(control::var("VIZIER_MONTH_HOUSE_CRESTS"), THEME_CRESTS);
    let colours = listed(control::var("VIZIER_MONTH_HOUSE_COLOURS"), THEME_COLOURS);
    let hems = listed(control::var("VIZIER_MONTH_HOUSE_HEMS"), THEME_HEMS);
    Themed {
        key: house.key,
        name: names.get(slot).cloned().unwrap_or(plain.name),
        crest: crests.get(slot).cloned().unwrap_or(plain.crest),
        colour: colours.get(slot).and_then(|c| hex(c)).unwrap_or(plain.colour),
        secondary: hems.get(slot).and_then(|c| hex(c)).unwrap_or(plain.secondary),
    }
}

/// The four, in `HOUSES` order.
pub fn themed_all() -> Vec<Themed> {
    HOUSES.iter().map(themed).collect()
}

/// What to call a house in a sentence.
pub fn name_of(house: &House) -> String {
    themed(house).name
}

/// A house by either name - its own, or the one it is wearing this month - so a
/// command option typed as "Stark" still finds Gryffindor's slot.
pub fn house_by_any_name(raw: &str) -> Option<&'static House> {
    if let Some(found) = super::house::house(raw) {
        return Some(found);
    }
    let wanted = raw.trim();
    HOUSES.iter().find(|h| themed(h).name.eq_ignore_ascii_case(wanted))
}

#[cfg(test)]
pub(super) mod testing {
    use std::sync::{Mutex, MutexGuard};

    /// Every test that moves a month setting takes this, so two of them can't
    /// run over each other, and everything it set is put back at the end.
    static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) struct Month {
        #[allow(dead_code)]
        guard: MutexGuard<'static, ()>,
        touched: Vec<String>,
    }

    impl Month {
        pub(crate) fn on() -> Month {
            let mut month = Month {
                guard: SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
                touched: Vec::new(),
            };
            month.set("VIZIER_MONTH", "on");
            month
        }

        pub(crate) fn off() -> Month {
            let mut month = Month {
                guard: SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()),
                touched: Vec::new(),
            };
            month.set("VIZIER_MONTH", "off");
            month
        }

        pub(crate) fn set(&mut self, key: &str, value: &str) {
            super::control::set_for_test(key, Some(value));
            self.touched.push(key.to_string());
        }
    }

    impl Drop for Month {
        fn drop(&mut self) {
            for key in std::mem::take(&mut self.touched) {
                super::control::set_for_test(&key, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::Month;
    use super::*;

    #[test]
    fn the_month_is_off_until_somebody_turns_it_on() {
        let mut month = Month::off();
        assert!(!running(), "off means off");
        month.set(KEY, "on");
        assert!(running());
        // A typo must never silently start a themed month.
        month.set(KEY, "perhaps");
        assert!(!running(), "an unreadable value falls back to the default, which is off");
    }

    #[test]
    fn the_hatch_is_read_in_india_time_and_a_bad_one_falls_back() {
        let mut month = Month::on();
        // 8 October 2026, noon India time.
        month.set("VIZIER_MONTH_HATCH", "2026-10-08 12:00");
        let at = hatch_at();
        assert_eq!(parse_ist("2026-10-08 12:00"), Some(at));
        // Noon in India is 06:30 UTC.
        let utc = chrono::DateTime::from_timestamp(at, 0).expect("a real moment");
        assert_eq!(utc.format("%Y-%m-%d %H:%M").to_string(), "2026-10-08 06:30");
        month.set("VIZIER_MONTH_HATCH", "the eighth of October, about lunchtime");
        assert_eq!(hatch_at(), parse_ist(HATCH_DEFAULT).unwrap(), "nonsense falls back to the shipped hatch");
    }

    #[test]
    fn the_live_link_is_only_a_link_and_lands_on_a_reply_once() {
        let mut month = Month::on();
        assert_eq!(live_url(), None, "nothing set, nothing said");
        assert_eq!(live_line(), "", "and no dangling line either");
        assert_eq!(with_live("🏆 the Cup".into()), "🏆 the Cup");
        month.set("VIZIER_LIVE_URL", "not a link at all");
        assert_eq!(live_url(), None, "a link has to be one");
        month.set("VIZIER_LIVE_URL", "https://mlci.example/live/");
        assert_eq!(live_url().as_deref(), Some("https://mlci.example/live"), "the trailing slash is dropped");
        let once = with_live("🏆 the Cup".into());
        assert!(once.contains("https://mlci.example/live"), "the reply ends with the page");
        assert_eq!(with_live(once.clone()), once, "and putting it on twice changes nothing");
    }

    #[test]
    fn the_houses_wear_the_months_names_and_keep_their_keys() {
        let month = Month::off();
        for h in HOUSES {
            let worn = themed(h);
            assert_eq!(worn.name, h.name, "with the month off a house is itself");
            assert_eq!(worn.colour, h.colour);
            let own = h.colours.1;
            assert_eq!(worn.secondary, u32::from_be_bytes([0, own[0], own[1], own[2]]), "and keeps both its colours");
        }
        drop(month);
        let _on = Month::on();
        let worn = themed_all();
        assert_eq!(
            worn.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["Stark", "Lannister", "Targaryen", "Night's Watch"]
        );
        assert_eq!(
            worn.iter().map(|t| t.key).collect::<Vec<_>>(),
            HOUSES.iter().map(|h| h.key).collect::<Vec<_>>(),
            "the ledger keys underneath never move - September's rows still belong to somebody"
        );
        assert_eq!(worn[0].crest, "🐺");
        assert_eq!(worn[2].colour, 0x8C1C1C);
        // Two colours, always: a banner has a field and a hem, and a caller
        // must never have to invent the second one for itself.
        assert_eq!(worn[2].secondary, 0xD96A6A);
        assert!(worn.iter().all(|t| t.colour != t.secondary));
        // Either name finds the same slot.
        assert_eq!(house_by_any_name("Stark").map(|h| h.key), Some("gryffindor"));
        assert_eq!(house_by_any_name("gryffindor").map(|h| h.key), Some("gryffindor"));
        assert!(house_by_any_name("Dorne").is_none());
    }

    #[test]
    fn a_half_written_theme_falls_back_slot_by_slot() {
        let mut month = Month::on();
        month.set("VIZIER_MONTH_HOUSE_NAMES", "Stark, Lannister");
        month.set("VIZIER_MONTH_HOUSE_COLOURS", "#112233,zzz");
        month.set("VIZIER_MONTH_HOUSE_HEMS", "445566");
        let worn = themed_all();
        assert_eq!(worn[1].name, "Lannister");
        assert_eq!(worn[2].name, HOUSES[2].name, "a slot the setting didn't reach keeps its own name");
        assert_eq!(worn[0].colour, 0x112233, "a # is allowed");
        assert_eq!(worn[1].colour, HOUSES[1].colour, "and an unreadable colour is ignored");
        assert_eq!(worn[0].secondary, 0x445566);
        let own = HOUSES[1].colours.1;
        assert_eq!(
            worn[1].secondary,
            u32::from_be_bytes([0, own[0], own[1], own[2]]),
            "a hem the setting didn't reach falls back to the house's own second colour"
        );
    }
}
