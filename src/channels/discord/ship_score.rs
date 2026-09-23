//! What `/ship` actually works the percentage out from.
//!
//! The owner's point: a raw count of replies is a poor signal. Two people with
//! 200 replies between them, out of thousands each, are barely a pair; two who
//! reply to almost nobody else are. So every relationship signal here is a
//! **share** of each person's own total, and what counts is the **smaller** of
//! the two shares - the one who is putting in less decides how much of a pair
//! they are. The gap between the two shares is a penalty of its own.
//!
//! Nothing here reads a database, a model or Discord: it takes two nightly
//! sheets (`ship_sheet.rs`), reduced to a [`Side`] each, plus the handful of
//! things that only exist between them ([`Between`]), and returns a number. Every
//! knob is a constant at the top, so a test can state a scenario in words -
//! "500 replies each way, both of them talk to nobody else" - and assert the
//! band it lands in.
//!
//! The shape of the answer:
//!
//! ```text
//! percent = base  +  Σ  weight · confidence · (component - base)   -  one-sidedness
//! ```
//!
//! where `base` is the pair's own number from their two ids. A component the
//! bot has no evidence for has confidence 0, and its weight flows back to the
//! base - so two members nobody has ever seen do anything score their own
//! number and nothing else, exactly as `/ship` behaved before any of this.

use std::collections::HashMap;

// --- the weights ---------------------------------------------------------------------------------
//
// Relationship signals about half, similarity about a third, the pair's own
// base number the rest. These three must add up to 1.

/// How much of each other's replies they get - the biggest single signal.
pub const W_ATTENTION: f64 = 0.26;
/// Time in the same voice room, as a share of each one's own voice time.
pub const W_VOICE: f64 = 0.10;
/// What has actually happened: back-and-forths, and fights.
pub const W_HISTORY: f64 = 0.14;
/// Relationship in all.
pub const W_RELATIONSHIP: f64 = W_ATTENTION + W_VOICE + W_HISTORY;

/// They are online at the same hours.
pub const W_HOURS: f64 = 0.11;
/// They live in the same channels.
pub const W_CHANNELS: f64 = 0.12;
/// They play the same games.
pub const W_GAMES: f64 = 0.09;
/// Similarity in all.
pub const W_SIMILARITY: f64 = W_HOURS + W_CHANNELS + W_GAMES;

/// The pair's own number, from their two ids.
pub const W_BASE: f64 = 1.0 - W_RELATIONSHIP - W_SIMILARITY;

// --- the curves ----------------------------------------------------------------------------------

/// A share of this much reads as half of the signal. A fifth of everything you
/// say going to one person is already a lot.
pub const ATTENTION_HALF: f64 = 0.15;
/// How much either of them has to have replied before the share means anything.
/// Both denominators have to be real: two people who have each sent two replies
/// in their lives tell us nothing, however they split.
pub const ATTENTION_K: f64 = 60.0;

/// A fifth of your voice time spent with one person reads as half the signal.
pub const VOICE_HALF: f64 = 0.20;
/// Minutes of each one's own voice time before the share means anything.
pub const VOICE_K: f64 = 120.0;

/// Back-and-forth stretches, and fights, before what happened means anything.
pub const HISTORY_K: f64 = 3.0;
/// Stretches at which the "they talk" half of it is half full.
pub const STRETCH_HALF: f64 = 6.0;
/// Fights at which the "they fight" half of it is half full.
pub const FIGHT_HALF: f64 = 3.0;
/// How far back-and-forth can lift what happened, and how far fights sink it.
/// Fights count for more: a fight is a rarer, louder thing than a chat.
pub const HISTORY_UP: f64 = 0.40;
pub const HISTORY_DOWN: f64 = 0.90;

/// Messages each of them has to have on record before two habits can be
/// compared at all.
pub const SIMILAR_K: f64 = 200.0;
/// Two members of the same server overlap a fair amount by accident, so the
/// overlap is rescaled: this much is nothing, and this much more is everything.
pub const HOURS_FLOOR: f64 = 0.45;
pub const HOURS_SPAN: f64 = 0.45;
pub const CHANNELS_FLOOR: f64 = 0.15;
pub const CHANNELS_SPAN: f64 = 0.60;

/// The most points one of them doing nearly all the replying can take off.
pub const ONE_SIDED_MAX: f64 = 10.0;
/// The gap in shares at which it stops being lopsided and starts being a crush.
pub const ONE_SIDED_GAP: f64 = 0.60;
/// How sure we have to be before the ceiling is put on at all.
pub const ONE_SIDED_SURE: f64 = 0.40;
/// A crush can't read as a great pairing however much else they have in common.
pub const ONE_SIDED_CEIL: u8 = 55;

/// The number only ever moves off its base in steps of this, so it drifts when
/// something real changed rather than jittering on every reply.
pub const STEP: f64 = 3.0;

/// Never a suspiciously round nothing or everything.
pub const MIN_PERCENT: u8 = 1;
pub const MAX_PERCENT: u8 = 99;

// --- what one side of a pair looks like -----------------------------------------------------------

/// One member, as the score reads them: their own totals (the denominators),
/// what of those totals went to the other one (the numerators), and the shape of
/// their habits. All of it comes off their nightly sheet, so both numbers in a
/// share were counted the same way on the same pass.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Side {
    /// Every reply they sent, to anyone.
    pub replies_sent: i64,
    /// How many of those went to the other half of this pair.
    pub replies_to_them: i64,
    /// Every minute they spent in voice, with anyone.
    pub vc_minutes: i64,
    /// How many of those were with the other half of this pair.
    pub vc_with_them: i64,
    /// Messages on record, for how sure we can be about their habits.
    pub messages: i64,
    /// When they are about, by hour of the India day.
    pub hours: [u32; 24],
    /// Messages per channel.
    pub channels: Vec<(u64, u32)>,
    /// The games they play.
    pub games: Vec<String>,
}

impl Side {
    /// The share of their replies that go to the other one, never more than all
    /// of them - the two counts can come off sheets built minutes apart.
    pub fn attention(&self) -> f64 {
        share(self.replies_to_them, self.replies_sent)
    }

    /// The share of their voice time spent with the other one.
    pub fn voice(&self) -> f64 {
        share(self.vc_with_them, self.vc_minutes)
    }
}

/// The things that only exist between two people and aren't on either sheet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Between {
    /// Stretches of back-and-forth the kalesh reader found in the period.
    pub stretches: usize,
    /// Fights the kalesh detector called with both of them in it.
    pub fights: usize,
}

fn share(part: i64, whole: i64) -> f64 {
    if whole <= 0 || part <= 0 { 0.0 } else { (part as f64 / whole as f64).min(1.0) }
}

/// A count read as a signal between 0 and 1, half full at `half`. Saturating, so
/// an absurd count lands just short of 1 rather than running away.
pub fn sat(x: f64, half: f64) -> f64 {
    if x <= 0.0 || !x.is_finite() { 0.0 } else { x / (x + half) }
}

/// How sure we are of a share, from the size of the sample behind it. Nothing
/// known is nothing said: the weight flows to the pair's own base instead.
pub fn sure(n: f64, k: f64) -> f64 {
    if n <= 0.0 || !n.is_finite() { 0.0 } else { (n / (n + k)).clamp(0.0, 1.0) }
}

/// A raw overlap rescaled: `floor` of it is worth nothing, `floor + span` is
/// worth everything.
fn rescale(overlap: f64, floor: f64, span: f64) -> f64 {
    ((overlap - floor) / span).clamp(0.0, 1.0)
}

// --- how alike two habits are ------------------------------------------------------------------

/// How much two lists of counts overlap once each is read as shares: 1 when they
/// are spent the same way, 0 when they have nothing in common. Symmetric.
pub fn overlap(a: &[(u64, u32)], b: &[(u64, u32)]) -> f64 {
    let total = |list: &[(u64, u32)]| list.iter().map(|(_, n)| *n as f64).sum::<f64>();
    let (ta, tb) = (total(a), total(b));
    if ta <= 0.0 || tb <= 0.0 {
        return 0.0;
    }
    let mut mine: HashMap<u64, f64> = HashMap::new();
    for (k, n) in a {
        *mine.entry(*k).or_insert(0.0) += *n as f64 / ta;
    }
    let mut theirs: HashMap<u64, f64> = HashMap::new();
    for (k, n) in b {
        *theirs.entry(*k).or_insert(0.0) += *n as f64 / tb;
    }
    mine.iter().map(|(k, share)| share.min(*theirs.get(k).unwrap_or(&0.0))).sum::<f64>().clamp(0.0, 1.0)
}

/// The same thing for the 24-slot histogram of when they are about.
pub fn hours_overlap(a: &[u32; 24], b: &[u32; 24]) -> f64 {
    let list = |h: &[u32; 24]| h.iter().enumerate().map(|(i, n)| (i as u64, *n)).collect::<Vec<_>>();
    overlap(&list(a), &list(b))
}

/// How many of the games they play are the same ones, out of all the games
/// either of them plays.
pub fn games_overlap(a: &[String], b: &[String]) -> f64 {
    let key = |g: &String| g.trim().to_lowercase();
    let mine: std::collections::BTreeSet<String> = a.iter().map(key).filter(|g| !g.is_empty()).collect();
    let theirs: std::collections::BTreeSet<String> = b.iter().map(key).filter(|g| !g.is_empty()).collect();
    let union = mine.union(&theirs).count();
    if union == 0 { 0.0 } else { mine.intersection(&theirs).count() as f64 / union as f64 }
}

// --- the parts of one score ------------------------------------------------------------------------

/// One thing that moved the number, what it read, and how sure the bot was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Part {
    /// In plain English, for the prompt and the log.
    pub what: &'static str,
    /// The signal itself, 0 to 1.
    pub value: f64,
    /// How sure the bot is of it, 0 to 1.
    pub sure: f64,
    /// Its nominal share of the answer.
    pub weight: f64,
}

impl Part {
    /// How many points it moved the pair off their own base.
    pub fn points(&self, base: f64) -> f64 {
        self.weight * self.sure * (self.value * 100.0 - base)
    }
}

/// A worked-out score, with everything that went into it.
#[derive(Clone, Debug, PartialEq)]
pub struct Score {
    pub percent: u8,
    /// The pair's own number, before anything they did.
    pub base: i32,
    pub parts: Vec<Part>,
    /// Points taken off for one of them doing nearly all the replying.
    pub one_sided: f64,
    /// Whether the ceiling on a one-sided pair was actually put on.
    pub capped: bool,
}

impl Score {
    /// How far what they do carried them off their own number, in the end.
    pub fn moved(&self) -> i32 {
        self.percent as i32 - self.base
    }

    /// The parts worth telling anyone about, biggest first, as (what, points).
    /// Anything under a point is noise and is left out.
    pub fn told(&self) -> Vec<(&'static str, i32)> {
        let base = self.base as f64;
        let mut out: Vec<(&'static str, i32)> = self
            .parts
            .iter()
            .map(|p| (p.what, p.points(base).round() as i32))
            .filter(|(_, by)| *by != 0)
            .collect();
        if self.one_sided.round() as i32 != 0 {
            out.push((ONE_SIDED, -(self.one_sided.round() as i32)));
        }
        out.sort_by_key(|(_, by)| -by.abs());
        out
    }
}

pub const ATTENTION: &str = "the share of their replies that go to each other";
pub const VOICE: &str = "time in voice together, out of each one's own voice time";
pub const HISTORY: &str = "back-and-forth between them, against the fights";
pub const HOURS: &str = "they are around at the same hours";
pub const CHANNELS: &str = "they live in the same channels";
pub const GAMES: &str = "the games they both play";
pub const ONE_SIDED: &str = "one of them does nearly all the replying";

/// The pair's own number, from their two ids and nothing else: the same every
/// time, whichever way round they are named, and nobody can reroll it.
pub fn base(a: u64, b: u64) -> i32 {
    let (lo, hi) = (a.min(b), a.max(b));
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in lo.to_le_bytes().iter().chain(hi.to_le_bytes().iter()) {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % 101) as i32
}

/// Every part of the answer, before the base is folded in. Pure, symmetric, and
/// the same list whichever way round the pair is given.
pub fn parts(a: &Side, b: &Side, t: &Between) -> Vec<Part> {
    // How much of each other's attention they actually get. The smaller share
    // is the signal: the one putting in less decides how much of a pair it is.
    let (sa, sb) = (a.attention(), b.attention());
    let attention = Part {
        what: ATTENTION,
        value: sat(sa.min(sb), ATTENTION_HALF),
        // Both denominators have to be real before a share means anything.
        sure: sure(a.replies_sent.min(b.replies_sent) as f64, ATTENTION_K),
        weight: W_ATTENTION,
    };
    let (va, vb) = (a.voice(), b.voice());
    let voice = Part {
        what: VOICE,
        value: sat(va.min(vb), VOICE_HALF),
        sure: sure(a.vc_minutes.min(b.vc_minutes) as f64, VOICE_K),
        weight: W_VOICE,
    };
    let happened = (HISTORY_UP * sat(t.stretches as f64, STRETCH_HALF) - HISTORY_DOWN * sat(t.fights as f64, FIGHT_HALF) + 0.5).clamp(0.0, 1.0);
    let history = Part {
        what: HISTORY,
        value: happened,
        sure: sure(t.stretches.saturating_add(t.fights) as f64, HISTORY_K),
        weight: W_HISTORY,
    };
    // Two habits can only be compared when there are enough of both to compare.
    let alike = sure(a.messages.min(b.messages) as f64, SIMILAR_K);
    let hours = Part {
        what: HOURS,
        value: rescale(hours_overlap(&a.hours, &b.hours), HOURS_FLOOR, HOURS_SPAN),
        sure: alike,
        weight: W_HOURS,
    };
    let channels = Part {
        what: CHANNELS,
        value: rescale(overlap(&a.channels, &b.channels), CHANNELS_FLOOR, CHANNELS_SPAN),
        sure: alike,
        weight: W_CHANNELS,
    };
    let games = Part {
        what: GAMES,
        value: games_overlap(&a.games, &b.games),
        // Nobody plays anything: nothing to say either way.
        sure: if a.games.is_empty() && b.games.is_empty() { 0.0 } else { 1.0 },
        weight: W_GAMES,
    };
    vec![attention, voice, history, hours, channels, games]
}

/// How lopsided the replying is, 0 when even and 1 when all of it is one way.
pub fn gap(a: &Side, b: &Side) -> f64 {
    let (sa, sb) = (a.attention(), b.attention());
    let hi = sa.max(sb);
    if hi <= 0.0 { 0.0 } else { ((hi - sa.min(sb)) / hi).clamp(0.0, 1.0) }
}

/// The whole thing: the pair's own number moved by what the two of them do, in
/// chunky steps, guarded at both ends. Identical whichever way round it is
/// asked, because every signal it uses is.
pub fn score(a_id: u64, b_id: u64, a: &Side, b: &Side, t: &Between) -> Score {
    let base = base(a_id, b_id);
    let parts = parts(a, b, t);
    let moved: f64 = parts.iter().map(|p| p.points(base as f64)).sum();
    // One of them doing nearly all the replying: a penalty the size of the gap,
    // and only as far as we are sure of the shares behind it.
    let lopsided = gap(a, b);
    let sure_of_it = sure(a.replies_sent.min(b.replies_sent) as f64, ATTENTION_K);
    let one_sided = ONE_SIDED_MAX * lopsided * sure_of_it;
    // Only whole steps off the base, so the number holds still until something
    // real changed.
    let delta = moved - one_sided;
    let stepped = (delta / STEP).round() * STEP;
    let mut percent = (base as f64 + stepped).round().clamp(MIN_PERCENT as f64, MAX_PERCENT as f64) as u8;
    let capped = lopsided >= ONE_SIDED_GAP && sure_of_it >= ONE_SIDED_SURE && percent > ONE_SIDED_CEIL;
    if capped {
        percent = ONE_SIDED_CEIL;
    }
    Score { percent, base, parts, one_sided, capped }
}

/// In plain English, for `/ship`'s own help, the panel and the report: how the
/// number is worked out, so nobody thinks it is a random roll.
pub const WHAT_MOVES_IT: &str = "The percentage is not a random roll and it is not a count of anything. Each pair has a \
number of their own, worked out from their two member ids, that never changes and nobody can reroll; about a sixth of \
the answer stays that number, and the rest is what the two of them actually do. Relationship is about half of it and \
how alike they are about a third. What matters is SHARES, not totals: how much of everything you say goes to that one \
person, and how much of everything they say comes back - the smaller of the two is what counts, so two people with 200 \
replies out of thousands each are less of a pair than two who barely reply to anyone else. It goes up when they give \
each other a real share of their replies, spend a real share of their voice time together, keep going back and forth, \
are around at the same hours, live in the same channels and play the same games. It goes down when they ignore each \
other while talking to everyone else, when the kalesh detector has called fights between them, and when one of them \
does nearly all the replying - a one-sided crush is capped and can never read high. A small sample is pulled back \
towards their own number, so two replies each way proves nothing, and anything the bot has no evidence for simply \
leaves the pair's own number showing. It moves only in steps of 3, so it drifts when something real changed rather \
than jittering. Nothing else is read into: the bot never guesses at personalities, interests or what anyone meant.";

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A pair whose own number happens to land dead in the middle, so a band
    /// either side of it means something. Every scenario below is scored for
    /// these two, and [`it_reads_the_same_whichever_way_round_the_pair_is_named`]
    /// takes the same scenarios round every other base there is.
    const A: u64 = 700;
    const B: u64 = 917;

    /// An even day: most of the talking in the evening, a couple of channels.
    fn hours_evening() -> [u32; 24] {
        let mut h = [0u32; 24];
        for (i, slot) in h.iter_mut().enumerate() {
            *slot = if (18..=23).contains(&i) { 200 } else { 5 };
        }
        h
    }

    /// Somebody who is about at all hours rather than one end of the day.
    fn hours_all_day() -> [u32; 24] {
        [60; 24]
    }

    fn hours_morning() -> [u32; 24] {
        let mut h = [0u32; 24];
        for (i, slot) in h.iter_mut().enumerate() {
            *slot = if (6..=11).contains(&i) { 200 } else { 5 };
        }
        h
    }

    /// Two people who reply to nobody but each other, sit in voice together and
    /// never miss a beat.
    fn inseparable() -> (Side, Side, Between) {
        let side = |games: Vec<String>| Side {
            replies_sent: 500,
            replies_to_them: 500,
            vc_minutes: 1_200,
            vc_with_them: 1_100,
            messages: 4_000,
            hours: hours_evening(),
            channels: vec![(1, 3_000), (2, 1_000)],
            games,
        };
        (side(vec!["Chess".into()]), side(vec!["Chess".into()]), Between { stretches: 40, fights: 0 })
    }

    /// Two loud members with thousands of replies each and none for each other.
    fn strangers() -> (Side, Side, Between) {
        let a = Side {
            replies_sent: 3_000,
            replies_to_them: 0,
            vc_minutes: 900,
            vc_with_them: 0,
            messages: 20_000,
            hours: hours_evening(),
            channels: vec![(1, 19_000), (9, 1_000)],
            games: vec!["Chess".into()],
        };
        let b = Side {
            replies_sent: 2_400,
            replies_to_them: 0,
            vc_minutes: 700,
            vc_with_them: 0,
            messages: 15_000,
            hours: hours_morning(),
            channels: vec![(7, 14_000), (8, 1_000)],
            games: vec!["Sudoku".into()],
        };
        (a, b, Between::default())
    }

    /// One of them lives in the other's replies; the other barely notices.
    fn crush() -> (Side, Side, Between) {
        let a = Side {
            replies_sent: 500,
            replies_to_them: 400,
            vc_minutes: 300,
            vc_with_them: 60,
            messages: 3_000,
            hours: hours_evening(),
            channels: vec![(1, 3_000)],
            games: vec!["Chess".into()],
        };
        let b = Side {
            replies_sent: 3_000,
            replies_to_them: 5,
            vc_minutes: 900,
            vc_with_them: 60,
            messages: 18_000,
            hours: hours_evening(),
            channels: vec![(1, 18_000)],
            games: vec!["Chess".into()],
        };
        (a, b, Between { stretches: 12, fights: 0 })
    }

    /// Two busy people who happen to share one channel: a fiftieth of each
    /// one's replying goes to the other, and the rest of their days look
    /// nothing alike.
    fn same_channel() -> (Side, Side, Between) {
        let side = |replies: i64, hours: [u32; 24], mine: u64| Side {
            replies_sent: replies,
            replies_to_them: replies / 50,
            vc_minutes: 0,
            vc_with_them: 0,
            messages: 9_000,
            hours,
            channels: vec![(1, 4_000), (mine, 5_000)],
            games: vec![],
        };
        (side(2_000, hours_evening(), 3), side(1_800, hours_all_day(), 4), Between { stretches: 2, fights: 0 })
    }

    /// The same two people as [`same_channel`], reply for reply - except that
    /// every time they do get going, the kalesh detector calls it.
    fn only_fight() -> (Side, Side, Between) {
        let (a, b, _) = same_channel();
        (a, b, Between { stretches: 2, fights: 7 })
    }

    /// Two replies each way and nothing else on record at all.
    fn tiny() -> (Side, Side, Between) {
        let side = || Side {
            replies_sent: 2,
            replies_to_them: 2,
            vc_minutes: 0,
            vc_with_them: 0,
            messages: 9,
            hours: hours_evening(),
            channels: vec![(1, 9)],
            games: vec![],
        };
        (side(), side(), Between { stretches: 1, fights: 0 })
    }

    fn percent(pair: (Side, Side, Between)) -> u8 {
        score(A, B, &pair.0, &pair.1, &pair.2).percent
    }

    #[test]
    fn the_weights_are_about_half_relationship_a_third_alike_and_the_rest_their_own_number() {
        assert!((W_RELATIONSHIP - 0.50).abs() < 0.01, "relationship is {}", W_RELATIONSHIP);
        assert!((W_SIMILARITY - 0.33).abs() < 0.02, "similarity is {}", W_SIMILARITY);
        assert!((W_BASE - 0.17).abs() < 0.02, "the pair's own number is {}", W_BASE);
        assert!((W_RELATIONSHIP + W_SIMILARITY + W_BASE - 1.0).abs() < 1e-9, "the weights have to add up");
        assert!(W_ATTENTION > W_VOICE && W_ATTENTION > W_HISTORY, "attention is the biggest single signal");
    }

    /// The scenarios the owner named, and the band each lands in.
    #[test]
    fn each_scenario_lands_where_it_should() {
        let base = base(A, B);
        let (close, apart, crushing, sharing, fighting, small) =
            (percent(inseparable()), percent(strangers()), percent(crush()), percent(same_channel()), percent(only_fight()), percent(tiny()));
        println!(
            "base {base}% · inseparable {close}% · strangers {apart}% · crush {crushing}% · same channel {sharing}% · only fight {fighting}% · tiny sample {small}%"
        );
        assert!((70..=99).contains(&close), "an inseparable pair read {close}%");
        assert!((1..=30).contains(&apart), "two strangers read {apart}%");
        assert!(crushing <= ONE_SIDED_CEIL && crushing < close, "a one-sided crush read {crushing}%");
        assert!((25..=50).contains(&sharing), "two chatty people sharing a channel read {sharing}%");
        assert!(fighting + 3 < sharing, "a pair who only fight ({fighting}%) must read well below the same two not fighting ({sharing}%)");
        // A tiny sample is pulled back to the pair's own number: two replies each
        // way must never read as a great romance.
        assert!((small as i32 - base).abs() <= 6, "two replies each way read {small}% against a base of {base}%");
        assert!(small < close, "two replies each way ({small}%) must not read like 500 ({close}%)");
        // And the order of them is the point of the whole thing.
        assert!(close > sharing && sharing > apart, "{close} > {sharing} > {apart}");
    }

    /// The owner's actual complaint: totals lie, shares don't.
    #[test]
    fn it_is_shares_not_totals() {
        let loud = |to_them: i64| Side { replies_sent: 4_000, replies_to_them: to_them, messages: 20_000, hours: hours_evening(), channels: vec![(1, 20_000)], ..Default::default() };
        let quiet = |to_them: i64| Side { replies_sent: 220, replies_to_them: to_them, messages: 1_500, hours: hours_evening(), channels: vec![(1, 1_500)], ..Default::default() };
        let t = Between { stretches: 10, fights: 0 };
        // 200 replies between two people who each send thousands.
        let big_totals = score(A, B, &loud(100), &loud(100), &t).percent;
        // 200 replies between two people who barely reply to anyone else.
        let big_shares = score(A, B, &quiet(100), &quiet(100), &t).percent;
        println!("the same 200 replies: out of thousands each {big_totals}%, out of a couple of hundred each {big_shares}%");
        assert!(big_shares > big_totals + 10, "{big_shares}% vs {big_totals}% - the share has to be what counts");
        // And piling on totals without moving the share changes almost nothing.
        let ten_times = score(A, B, &loud(1_000), &loud(1_000), &t).percent;
        assert!(ten_times > big_totals, "ten times the replies, ten times the share: {ten_times}% vs {big_totals}%");
    }

    #[test]
    fn a_one_sided_pair_is_docked_and_capped() {
        let (a, b, t) = crush();
        let s = score(A, B, &a, &b, &t);
        assert!(gap(&a, &b) > ONE_SIDED_GAP, "the gap is {}", gap(&a, &b));
        assert!(s.one_sided > 0.0, "a crush is docked");
        assert!(s.told().iter().any(|(what, by)| *what == ONE_SIDED && *by < 0), "{:?}", s.told());
        // The same two, with the quiet one giving back the same share: higher.
        let mutual = Side { replies_sent: 500, replies_to_them: 400, ..b.clone() };
        let even = score(A, B, &a, &mutual, &t);
        assert!(even.percent > s.percent, "mutual {}% vs one-sided {}%", even.percent, s.percent);
        assert_eq!(even.one_sided.round(), 0.0, "even replying is not lopsided");
        // A crush can never read high, whatever else the two have in common.
        let devoted = Side { replies_sent: 500, replies_to_them: 500, vc_minutes: 600, vc_with_them: 600, ..a.clone() };
        let ignored = Side { replies_sent: 4_000, replies_to_them: 6, vc_minutes: 4_000, vc_with_them: 600, ..b.clone() };
        let one_way = score(A, B, &devoted, &ignored, &Between { stretches: 50, fights: 0 });
        assert!(one_way.percent <= ONE_SIDED_CEIL, "a crush read {}%", one_way.percent);
    }

    #[test]
    fn someone_with_no_voice_time_at_all_is_simply_not_asked_about_it() {
        let (a, b, t) = same_channel();
        assert_eq!((a.vc_minutes, b.vc_minutes), (0, 0));
        let s = score(A, B, &a, &b, &t);
        let voice = s.parts.iter().find(|p| p.what == VOICE).expect("the voice part is still listed");
        assert_eq!(voice.sure, 0.0, "nothing known, nothing said");
        assert_eq!(voice.points(s.base as f64), 0.0, "and so it moves the number not at all");
        assert!(!s.told().iter().any(|(what, _)| *what == VOICE), "and it isn't mentioned");
        // One of them having voice time and the other none is still nothing known.
        let alone = Side { vc_minutes: 5_000, ..a.clone() };
        assert_eq!(score(A, B, &alone, &b, &t).percent, s.percent, "one person's voice time on its own says nothing");
        assert!(s.percent >= MIN_PERCENT && s.percent <= MAX_PERCENT);
    }

    #[test]
    fn nothing_known_at_all_leaves_the_pairs_own_number_showing() {
        let empty = Side::default();
        let s = score(A, B, &empty, &empty, &Between::default());
        assert_eq!(s.percent as i32, base(A, B).clamp(MIN_PERCENT as i32, MAX_PERCENT as i32), "no evidence, no movement");
        assert!(s.told().is_empty(), "and nothing to say about it: {:?}", s.told());
        assert!(s.parts.iter().all(|p| p.sure == 0.0 || p.what == GAMES));
    }

    #[test]
    fn it_reads_the_same_whichever_way_round_the_pair_is_named() {
        for pair in [inseparable(), strangers(), crush(), same_channel(), only_fight(), tiny()] {
            let (a, b, t) = pair;
            for (x, y) in [(A, B), (1, 2), (999_999_999_999_999_999, 4), (7, 7)] {
                assert_eq!(score(x, y, &a, &b, &t).percent, score(y, x, &b, &a, &t).percent, "{x} and {y}");
                assert_eq!(base(x, y), base(y, x));
            }
            // And asking fifty times is asking once.
            let first = score(A, B, &a, &b, &t).percent;
            for _ in 0..50 {
                assert_eq!(score(B, A, &b, &a, &t).percent, first);
            }
        }
    }

    #[test]
    fn it_is_never_a_round_nothing_or_everything_and_never_jitters() {
        let mad = Side {
            replies_sent: i64::MAX,
            replies_to_them: i64::MAX,
            vc_minutes: i64::MAX,
            vc_with_them: i64::MAX,
            messages: i64::MAX,
            hours: [u32::MAX; 24],
            channels: vec![(1, u32::MAX)],
            games: vec!["Chess".into()],
        };
        let worst = Side { replies_to_them: 0, vc_with_them: 0, games: vec![], ..mad.clone() };
        for (a, b, t) in [
            (mad.clone(), mad.clone(), Between { stretches: usize::MAX, fights: 0 }),
            (worst.clone(), worst.clone(), Between { stretches: 0, fights: usize::MAX }),
            (mad.clone(), worst.clone(), Between { stretches: usize::MAX, fights: usize::MAX }),
        ] {
            for (x, y) in [(A, B), (1, 2), (5, 900)] {
                let p = score(x, y, &a, &b, &t).percent;
                assert!((MIN_PERCENT..=MAX_PERCENT).contains(&p), "{p}% from {x}/{y}");
            }
        }
        // Chunky steps: a handful more replies changes nothing at all.
        let (a, b, t) = inseparable();
        let first = score(A, B, &a, &b, &t).percent;
        for extra in 1..=4 {
            let nudged = Side { replies_to_them: a.replies_to_them - extra, ..a.clone() };
            assert_eq!(score(A, B, &nudged, &b, &t).percent, first, "{extra} replies fewer moved it");
        }
        // Different pairs don't all land on the same number.
        let spread: std::collections::HashSet<u8> = (1..40u64).map(|i| score(i, i * 7 + 3, &a, &b, &t).percent).collect();
        assert!(spread.len() > 10, "only {} distinct scores in 39 pairs", spread.len());
    }

    #[test]
    fn two_habits_are_compared_as_shares_of_each_persons_own_day() {
        assert!((hours_overlap(&hours_evening(), &hours_evening()) - 1.0).abs() < 1e-9, "the same day overlaps itself entirely");
        assert!(hours_overlap(&hours_evening(), &hours_morning()) < 0.2, "opposite days barely overlap");
        assert_eq!(hours_overlap(&[0; 24], &hours_evening()), 0.0, "nothing on record, no overlap");
        // A busy channel and a quiet one overlap by their shares, not their counts.
        assert!((overlap(&[(1, 10)], &[(1, 10_000)]) - 1.0).abs() < 1e-9, "both spend all of their time in the same place");
        assert_eq!(overlap(&[(1, 10)], &[(2, 10)]), 0.0);
        assert!((overlap(&[(1, 50), (2, 50)], &[(1, 100)]) - 0.5).abs() < 1e-9);
        assert_eq!(overlap(&[], &[(1, 5)]), 0.0);
        assert_eq!(games_overlap(&["Chess".into()], &["chess".into()]), 1.0, "the same game spelled two ways");
        assert_eq!(games_overlap(&["Chess".into()], &["Sudoku".into()]), 0.0);
        assert!((games_overlap(&["Chess".into(), "Sudoku".into()], &["Chess".into()]) - 0.5).abs() < 1e-9);
        assert_eq!(games_overlap(&[], &[]), 0.0, "nobody plays anything");
        // Both curves behave at the edges.
        assert_eq!(sat(0.0, 0.15), 0.0);
        assert_eq!(sure(0.0, 60.0), 0.0);
        assert!(sat(f64::INFINITY, 0.15) == 0.0 || sat(f64::MAX, 0.15) < 1.0);
        assert!((0.0..=1.0).contains(&sure(f64::MAX, 60.0)));
    }

    #[test]
    fn what_it_says_about_itself_is_true_of_what_it_does() {
        for must in [
            "SHARES, not totals",
            "smaller of the two",
            "never changes",
            "member ids",
            "kalesh",
            "one-sided",
            "capped",
            "steps of 3",
            "never guesses",
        ] {
            assert!(WHAT_MOVES_IT.contains(must), "the explanation lacks {must:?}");
        }
        assert_eq!(STEP, 3.0, "the explanation says steps of 3");
        // The breakdown names the parts in plain words, biggest first.
        let (a, b, t) = inseparable();
        assert_eq!(base(A, B), 50, "the scenarios are scored against a middling base");
        let told = score(A, B, &a, &b, &t).told();
        assert!(told.iter().any(|(what, by)| *what == ATTENTION && *by > 0), "{told:?}");
        assert!(told.windows(2).all(|w| w[0].1.abs() >= w[1].1.abs()), "biggest first: {told:?}");
        let (a, b, t) = only_fight();
        let told = score(A, B, &a, &b, &t).told();
        assert!(told.iter().any(|(what, by)| *what == HISTORY && *by < 0), "fights pull it down: {told:?}");
    }
}
