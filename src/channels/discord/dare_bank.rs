//! The Truth or Dare bank: every prompt the bot asks, kept in code the way the
//! duel and anagram word lists are rather than in a file somebody can drop into
//! the workspace.
//!
//! That is deliberate. Unlike a film or a word, a truth-or-dare prompt cannot be
//! checked by the bot after the fact — whatever it asks, it asks a real person
//! in front of the whole channel, and there is no judging it afterwards. So the
//! list is reviewed once, here, and shipped.
//!
//! # Tiers
//!
//! Every prompt carries a tier, and the channel has a ceiling
//! (`VIZIER_DARE_SPICE`, 1 by default):
//!
//! * **1 — mild.** Anyone could answer it in front of anyone. Bad haircuts,
//!   useless skills, the song on repeat.
//! * **2 — sharper.** Embarrassing rather than exposing: the pettiest grudge,
//!   the worst gift, the money regret.
//! * **3 — bold.** Genuinely personal, and off by default. Regrets, fears, the
//!   apology never given.
//! * **4 — adult.** Grown-up questions: attraction, dating, the awkward bits.
//! * **5 — explicit.** Frank questions about sex. **Truths only**, like tier 4,
//!   and behind the same age gate.
//!
//! # The adult tier
//!
//! Tiers 4 and 5 are not simply tier 3 with the brakes off. Two hard rules hold
//! them, and both are enforced in code rather than left to whoever edits the
//! list:
//!
//! 1. **Truths only.** There are no adult dares at all, at either tier, and a
//!    test keeps it that way ([`tests::no_dare_is_ever_adult`]). A frank
//!    question is answered by typing a sentence about your own life; a frank
//!    *instruction* is something a person is told to go and do, which is a
//!    different thing entirely and not something this bot will ever ask for —
//!    no matter how the spice is set or how the room is flagged.
//! 2. **The channel decides, not the setting.** Turning the spice up is not
//!    enough on its own: the caller must also say the room is an age-restricted
//!    one, which [`super::dare`] takes from Discord's own flag on the channel
//!    rather than from anything a mod typed. In a channel Discord does not mark
//!    age-restricted, tier 4 does not exist however the setting is left.
//!
//! # What is never in here, at any tier
//!
//! No prompt names another member, asks about one, or asks the target to do
//! something TO one — that turns a game into a way of putting someone on the
//! spot through a third party. No prompt asks for a photo, a video, a document,
//! a contact, an address or anything else that leaves the channel, and that
//! holds hardest at tier 4, where a request for a picture is how an evening's
//! game becomes something far worse. Nothing goes near substances, self-harm or
//! anyone's family. A dare is always something that can be done by TYPING,
//! here, in the next few minutes: a dare nobody can verify is a dare nobody
//! should be asked for.
//!
//! The target can always [veto](super::dare) a prompt and be given another, and
//! nothing is lost by doing it — which is the real guard. The tiers only decide
//! how often a veto is needed.

use std::collections::HashSet;

use super::sudoku_gen::Rng;

/// Which of the two a prompt is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Truth,
    Dare,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Truth => "truth",
            Kind::Dare => "dare",
        }
    }

    pub fn from_key(key: &str) -> Option<Kind> {
        match key {
            "truth" => Some(Kind::Truth),
            "dare" => Some(Kind::Dare),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Truth => "Truth",
            Kind::Dare => "Dare",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Kind::Truth => "💬",
            Kind::Dare => "🔥",
        }
    }
}

/// One prompt. `key` is a stable slug and NOT a position in the list: the
/// no-repeat history is written in terms of it, so prompts can be added,
/// reordered or retired without quietly re-asking everybody everything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub key: &'static str,
    pub kind: Kind,
    pub tier: u8,
    pub text: &'static str,
}

/// The lowest and highest tier a channel may be set to.
pub const MIN_TIER: u8 = 1;
pub const MAX_TIER: u8 = 5;

/// The lowest tier that only ever plays in an age-restricted channel. Tier 4
/// and everything above it is a truth; see the module note.
pub const ADULT_TIER: u8 = 4;

/// The highest tier a dare may carry. Dares stay clean whatever the room is.
pub const MAX_DARE_TIER: u8 = 3;

const TRUTHS: &[(&str, u8, &str)] = &[
    // --- 1, mild -----------------------------------------------------------
    ("first_impression", 1, "What was your first impression of this server, honestly?"),
    ("comfort_show", 1, "What do you rewatch when nothing else will do?"),
    ("useless_skill", 1, "What's the most useless thing you're weirdly good at?"),
    ("worst_haircut", 1, "Describe the worst haircut you've ever had."),
    ("phone_wallpaper", 1, "What's your wallpaper right now, and why that one?"),
    ("odd_search", 1, "What's the last thing you searched that you'd have to explain?"),
    ("childhood_fear", 1, "What were you scared of as a kid that makes no sense now?"),
    ("unfunny", 1, "What's a joke everyone finds funny that you just don't?"),
    ("guilty_song", 1, "What song is on repeat that you'd never admit to?"),
    ("three_am", 1, "What do you actually do when you can't sleep?"),
    ("overrated_food", 1, "What food is massively overrated? Die on that hill."),
    ("first_username", 1, "What was your first ever username, and what were you thinking?"),
    ("teacher_lie", 1, "What's the biggest lie you told a teacher?"),
    ("year_free", 1, "If you had a free year, what would you learn?"),
    ("cringe_phase", 1, "What phase did you go through that you cringe at now?"),
    ("worst_cook", 1, "What's the worst thing you've ever cooked or eaten?"),
    // --- 2, sharper --------------------------------------------------------
    ("left_on_read", 2, "Whose message have you left on read the longest, and why?"),
    ("worst_gift", 2, "What's the worst gift you've given — and did they notice?"),
    ("petty_grudge", 2, "What's the pettiest grudge you're still holding?"),
    ("pretend_know", 2, "What do you pretend to understand but genuinely don't?"),
    ("caught_out", 2, "When were you last caught out in a lie? How did it go?"),
    ("worse_than_thought", 2, "What are you much worse at than people assume?"),
    ("never_sent", 2, "What's a message you typed out and never sent?"),
    ("quietly_jealous", 2, "What do you get quietly jealous about?"),
    ("last_cried", 2, "What last made you cry? A film counts."),
    ("worst_advice", 2, "What's the worst advice you've ever given someone?"),
    ("scrolled_far", 2, "How far back have you scrolled on someone's profile? Be honest."),
    ("money_regret", 2, "What's the dumbest thing you've spent real money on?"),
    ("blagged_it", 2, "What have you completely blagged your way through?"),
    ("reputation", 2, "What do you think people here get wrong about you?"),
    // --- 3, bold -----------------------------------------------------------
    ("misjudged", 3, "Who did you completely misjudge at first, and what changed?"),
    ("do_differently", 3, "What's the one thing you'd go back and do differently?"),
    ("unsent_apology", 3, "Who do you owe an apology to that you've never given?"),
    ("never_said", 3, "What's a fear you've never said out loud here?"),
    ("hard_truth", 3, "What's something true about you that took years to accept?"),
    ("friendship_end", 3, "What ended a friendship you thought would last?"),
    ("worst_habit", 3, "What habit would you hate someone to point out?"),
    ("quiet_pride", 3, "What are you proud of that you never get to mention?"),
    // --- 4, adult. Age-restricted channels only, and truths only -----------
    ("best_kiss", 4, "Describe the best kiss you've ever had."),
    ("turn_on", 4, "What's your biggest turn-on?"),
    ("turn_off", 4, "What's an instant turn-off, no matter how much you liked them?"),
    ("first_fancied", 4, "Who was the first person you properly fancied, and what did it do to you?"),
    ("worst_flirt", 4, "What's the worst attempt at flirting you've made — or had made at you?"),
    ("wrong_moment", 4, "When did you last have a thought you really shouldn't have, and where were you?"),
    ("nearly_caught", 4, "Have you ever nearly been caught at something? What happened?"),
    ("type_unpolite", 4, "Describe what you actually find attractive. Not the polite version."),
    ("the_list", 4, "Who's on your list, and what's the reason you'd never say out loud?"),
    ("slid_in", 4, "Have you ever slid into someone's DMs? How did that go for you?"),
    ("date_ended", 4, "What's the worst way a date has ever ended?"),
    ("lied_to_them", 4, "What have you lied about to someone you were seeing?"),
    ("best_feature", 4, "What part of yourself do you secretly think is your best?"),
    ("boldest_thing", 4, "What's the boldest thing you've done with somebody watching?"),
    ("most_forward", 4, "What's the most forward message you've ever sent? Did you regret it?"),
    ("tamer_than_guessed", 4, "What are you into that's far tamer than people would guess?"),
    ("fancied_online", 4, "Have you ever properly fancied someone you only knew online?"),
    ("longest_without", 4, "What's the longest you've gone without so much as a kiss?"),
    ("two_at_once", 4, "Have you ever wanted two people at the same time? How did that end?"),
    ("never_bring_up", 4, "What are you into that you'd never be the first to mention?"),
    ("walked_in", 4, "What have you walked in on that you wish you hadn't?"),
    ("worst_yes", 4, "Who was the worst idea you ever said yes to?"),
    ("line_that_works", 4, "What's the line that actually works on you?"),
    ("morning_after", 4, "What's the most awkward morning-after you've had?"),
    ("too_soon", 4, "How soon is too soon, honestly?"),
    ("only_after_a_drink", 4, "What would you only ever admit to after a drink?"),
    // --- 5, explicit. Age-restricted channels only, and truths only --------
    ("adventurous_place", 5, "What's the most adventurous place you've ever done it?"),
    ("never_told", 5, "What have you done that you've never told a single friend about?"),
    ("hard_no", 5, "What's your hard no, and has anyone ever asked for it anyway?"),
    ("never_asked_for", 5, "What do you actually want that you've never asked for out loud?"),
    ("loud_or_quiet", 5, "Loud or quiet? And is that a choice or not?"),
    ("the_best", 5, "What's the best you've ever had, and what made it that?"),
    ("never_in_person", 5, "What are you into that you'd never admit to someone's face?"),
    ("ever_faked", 5, "Have you ever faked it? How often, honestly?"),
    ("nearly_caught_at_it", 5, "Where's the riskiest place you've nearly been caught?"),
    ("time_of_day", 5, "Morning, night, or whenever it's going? Defend your answer."),
    ("type_in_practice", 5, "Your type on paper and your type in bed — how far apart are they?"),
    ("worst_youve_been", 5, "When were you genuinely bad at it, and do you know why?"),
    ("always_wanted", 5, "What's the one thing you've always wanted to try and still haven't?"),
    ("embarrassing_turn_on", 5, "What turn-on are you slightly embarrassed by?"),
    ("the_first", 5, "Your first time — how did it actually go?"),
    ("longest_shortest", 5, "What's the longest you've gone? And the shortest?"),
    ("ever_caught", 5, "Have you ever been properly caught? By whom?"),
    ("lights", 5, "Lights on or off, and tell the truth."),
    ("only_once", 5, "What have you done exactly once and never again?"),
    ("boldest_ask", 5, "What's the boldest thing you've ever asked somebody for?"),
    ("shouldnt_have", 5, "Have you ever wanted someone you absolutely shouldn't have?"),
    ("most_awkward", 5, "What's the most awkward thing that's ever happened mid-way?"),
    ("never_fails", 5, "What's the one thing that never fails on you?"),
    ("instant_regret", 5, "Have you ever sent something you regretted the second it went?"),
    ("own_red_flag", 5, "What's your biggest red flag in bed?"),
    ("still_think_about", 5, "Who do you still think about, and why that one?"),
    ("said_out_loud", 5, "What's the most unexpected thing you've ever said in the moment?"),
    ("would_never_repeat", 5, "What's something you tried once and would never repeat?"),
];

const DARES: &[(&str, u8, &str)] = &[
    // --- 1, mild -----------------------------------------------------------
    ("emoji_only", 1, "Reply in emoji only for your next five messages."),
    ("all_caps", 1, "TYPE EVERYTHING IN CAPS for the next ten minutes."),
    ("three_compliments", 1, "Give three people here a genuine compliment, right now."),
    ("worst_joke", 1, "Tell the worst joke you know."),
    ("chorus_memory", 1, "Type out the chorus of the last song you heard — from memory only."),
    ("last_five_apps", 1, "List the last five apps you opened, in order, no editing."),
    ("describe_badly", 1, "Describe your favourite film so badly nobody can guess it."),
    ("autocorrect", 1, "Type \"my plan for tomorrow is\" and let autocorrect finish it. Post what you get."),
    ("nature_doc", 1, "Write your next three messages as a nature documentary narration."),
    ("praise_rival", 1, "Say one genuinely nice thing about a house that isn't yours."),
    ("last_meal", 1, "Post the most recent thing you ate, in full unflattering detail."),
    ("no_e", 1, "Send your next two messages without using the letter E."),
    ("signature", 1, "Pick a ridiculous title for yourself and sign every message with it for ten minutes."),
    ("dramatic_read", 1, "Read your own last message back, as dramatically as typing allows."),
    ("weather_report", 1, "Deliver the current mood of this channel as a weather report."),
    // --- 2, sharper --------------------------------------------------------
    ("camera_roll", 2, "Describe the last photo in your camera roll. Don't show it — describe it."),
    ("last_sent", 2, "Post the last message you sent anyone, names taken out."),
    ("silent_judgement", 2, "Admit one thing you've silently judged in this server."),
    ("roast_self", 2, "Roast yourself. One sentence, no mercy."),
    ("honest_bio", 2, "Write a brutally honest one-line bio for yourself."),
    ("no_fear_message", 2, "Write the message you'd send if the reply couldn't hurt you. Post it here, not there."),
    ("indefensible", 2, "Share your most indefensible opinion and defend it for one message."),
    ("annoyed_voice", 2, "Do a written impression of how you sound when you're annoyed."),
    ("two_truths", 2, "Post two truths and a lie about yourself and let the channel guess."),
    ("worst_message", 2, "Quote the worst message you've ever sent in this server."),
    ("thirty_seconds", 2, "Say everything you think about Mondays in one unbroken message."),
    // --- 3, bold -----------------------------------------------------------
    ("own_flaw", 3, "Give your honest, unflattering opinion of your own biggest flaw."),
    ("the_apology", 3, "Write the apology you've been avoiding. You needn't say who it's for."),
    ("your_type", 3, "Describe your type in uncomfortable detail."),
    ("delete_moment", 3, "Tell us about the moment you'd most like to delete."),
    ("want_most", 3, "Say the thing you want most and never admit to wanting."),
    ("three_honest", 3, "Answer the next three questions anyone asks you completely honestly."),
];

fn build(rows: &'static [(&'static str, u8, &'static str)], kind: Kind) -> Vec<Prompt> {
    rows.iter().map(|(key, tier, text)| Prompt { key, kind, tier: *tier, text }).collect()
}

/// Every prompt of one kind, whatever the tier.
pub fn all(kind: Kind) -> Vec<Prompt> {
    match kind {
        Kind::Truth => build(TRUTHS, Kind::Truth),
        Kind::Dare => build(DARES, Kind::Dare),
    }
}

/// The ceiling that actually applies, once the room has had its say. Tier 4
/// needs BOTH the setting and an age-restricted channel; a dare never goes past
/// [`MAX_DARE_TIER`] whatever either of them says.
pub fn ceiling(kind: Kind, max_tier: u8, adult_room: bool) -> u8 {
    let wanted = max_tier.clamp(MIN_TIER, MAX_TIER);
    let allowed = if adult_room { MAX_TIER } else { ADULT_TIER - 1 };
    let for_kind = match kind {
        Kind::Dare => MAX_DARE_TIER,
        Kind::Truth => MAX_TIER,
    };
    wanted.min(allowed).min(for_kind)
}

/// How many prompts a ceiling actually opens up, for the help card and the
/// panel.
pub fn count(kind: Kind, max_tier: u8, adult_room: bool) -> usize {
    let ceiling = ceiling(kind, max_tier, adult_room);
    all(kind).into_iter().filter(|p| p.tier <= ceiling).count()
}

/// Finds a prompt by its slug, for reading a turn back out of the store.
pub fn find(key: &str) -> Option<Prompt> {
    all(Kind::Truth).into_iter().chain(all(Kind::Dare)).find(|p| p.key == key)
}

/// A prompt of the asked-for kind, at or under the ceiling that actually
/// applies ([`ceiling`] — the setting, the room, and the kind), that this person
/// hasn't had lately.
///
/// `seen` is that person's own recent history and nobody else's: two people can
/// be asked the same thing on the same evening, and the point of the window is
/// only that YOU are not asked the same thing twice. If the ceiling is low
/// enough that they have genuinely had them all, it repeats rather than
/// refusing — a tier-1 channel would otherwise run dry.
pub fn pick(kind: Kind, max_tier: u8, adult_room: bool, seen: &HashSet<String>, rng: &mut Rng) -> Option<Prompt> {
    let ceiling = ceiling(kind, max_tier, adult_room);
    let at_tier: Vec<Prompt> = all(kind).into_iter().filter(|p| p.tier <= ceiling).collect();
    if at_tier.is_empty() {
        return None;
    }
    let mut fresh: Vec<Prompt> = at_tier.iter().copied().filter(|p| !seen.contains(p.key)).collect();
    let mut pool = if fresh.is_empty() { at_tier } else { std::mem::take(&mut fresh) };
    rng.shuffle(&mut pool);
    pool.into_iter().next()
}

/// Either kind, for the 🎲 button: the coin is flipped here so the two lists
/// being different lengths can't tilt it.
pub fn flip(rng: &mut Rng) -> Kind {
    if rng.below(2) == 0 { Kind::Truth } else { Kind::Dare }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_unique_across_both_lists() {
        let mut seen = HashSet::new();
        for prompt in all(Kind::Truth).into_iter().chain(all(Kind::Dare)) {
            assert!(seen.insert(prompt.key), "duplicate slug {}", prompt.key);
        }
    }

    #[test]
    fn every_tier_is_one_we_ship() {
        for prompt in all(Kind::Truth).into_iter().chain(all(Kind::Dare)) {
            assert!((MIN_TIER..=MAX_TIER).contains(&prompt.tier), "{} has tier {}", prompt.key, prompt.tier);
        }
    }

    /// The default channel is a mild one, so tier 1 has to stand on its own.
    #[test]
    fn tier_one_alone_is_a_playable_game() {
        assert!(count(Kind::Truth, 1, false) >= 10);
        assert!(count(Kind::Dare, 1, false) >= 10);
    }

    #[test]
    fn a_ceiling_never_lets_a_higher_tier_through() {
        let seen = HashSet::new();
        for _ in 0..200 {
            let mut rng = Rng::fresh();
            let prompt = pick(Kind::Dare, 1, false, &seen, &mut rng).expect("a dare");
            assert_eq!(prompt.tier, 1);
        }
    }

    /// The first hard rule: a dare is never an adult one, so there is nothing
    /// at that tier for a dare to reach even in an age-restricted room.
    #[test]
    fn no_dare_is_ever_adult() {
        for prompt in all(Kind::Dare) {
            assert!(prompt.tier <= MAX_DARE_TIER, "{} is tier {}", prompt.key, prompt.tier);
        }
        let seen = HashSet::new();
        for _ in 0..300 {
            let mut rng = Rng::fresh();
            let prompt = pick(Kind::Dare, MAX_TIER, true, &seen, &mut rng).expect("a dare");
            assert!(prompt.tier <= MAX_DARE_TIER, "an adult room still got a dare at tier {}", prompt.tier);
        }
        assert_eq!(count(Kind::Dare, MAX_TIER, true), count(Kind::Dare, MAX_DARE_TIER, false));
    }

    /// Every tier from ADULT_TIER up is gated, not just the first of them —
    /// adding tier 5 must not have left a door open above the check.
    #[test]
    fn no_tier_above_the_gate_escapes_it() {
        let seen = HashSet::new();
        for _ in 0..400 {
            let mut rng = Rng::fresh();
            let prompt = pick(Kind::Truth, MAX_TIER, false, &seen, &mut rng).expect("a truth");
            assert!(prompt.tier < ADULT_TIER, "a tame room served tier {}", prompt.tier);
        }
        for tier in ADULT_TIER..=MAX_TIER {
            assert_eq!(ceiling(Kind::Truth, tier, false), ADULT_TIER - 1, "tier {} leaked into a tame room", tier);
            assert_eq!(ceiling(Kind::Truth, tier, true), tier);
            assert_eq!(ceiling(Kind::Dare, tier, true), MAX_DARE_TIER, "tier {} opened a dare", tier);
        }
        // And the explicit tier really is a further step, not a relabelling.
        assert!(count(Kind::Truth, MAX_TIER, true) > count(Kind::Truth, ADULT_TIER, true));
    }

    /// The second: turning the spice up is not enough on its own.
    #[test]
    fn the_adult_tier_needs_the_room_and_not_just_the_setting() {
        let seen = HashSet::new();
        for _ in 0..300 {
            let mut rng = Rng::fresh();
            let prompt = pick(Kind::Truth, MAX_TIER, false, &seen, &mut rng).expect("a truth");
            assert!(prompt.tier < ADULT_TIER, "a tame room served tier {}", prompt.tier);
        }
        assert_eq!(count(Kind::Truth, MAX_TIER, false), count(Kind::Truth, ADULT_TIER - 1, false));
        assert!(count(Kind::Truth, MAX_TIER, true) > count(Kind::Truth, MAX_TIER, false));
    }

    /// An age-restricted room does not drag a mild setting upwards either: the
    /// two are an AND, not an OR.
    #[test]
    fn an_adult_room_still_obeys_the_setting() {
        let seen = HashSet::new();
        for _ in 0..200 {
            let mut rng = Rng::fresh();
            assert_eq!(pick(Kind::Truth, 1, true, &seen, &mut rng).expect("a truth").tier, 1);
        }
        assert_eq!(ceiling(Kind::Truth, 2, true), 2);
        assert_eq!(ceiling(Kind::Truth, MAX_TIER, true), MAX_TIER);
        assert_eq!(ceiling(Kind::Truth, MAX_TIER, false), ADULT_TIER - 1);
        assert_eq!(ceiling(Kind::Dare, MAX_TIER, true), MAX_DARE_TIER);
    }

    #[test]
    fn what_someone_has_had_is_held_back() {
        let seen: HashSet<String> = all(Kind::Truth).into_iter().filter(|p| p.tier == 1).skip(1).map(|p| p.key.to_string()).collect();
        let only_left = all(Kind::Truth).into_iter().find(|p| p.tier == 1).expect("a truth").key;
        let mut rng = Rng::fresh();
        assert_eq!(pick(Kind::Truth, 1, false, &seen, &mut rng).expect("a truth").key, only_left);
    }

    /// Having had them all repeats rather than leaving the channel with nothing.
    #[test]
    fn a_dry_window_still_asks_something() {
        let seen: HashSet<String> = all(Kind::Dare).into_iter().map(|p| p.key.to_string()).collect();
        let mut rng = Rng::fresh();
        assert!(pick(Kind::Dare, MAX_TIER, true, &seen, &mut rng).is_some());
    }

    #[test]
    fn find_reads_a_turn_back() {
        let prompt = all(Kind::Dare).into_iter().next().expect("a dare");
        assert_eq!(find(prompt.key).expect("found").text, prompt.text);
        assert!(find("no_such_prompt").is_none());
    }

    /// No prompt may point at another member: that is how a game becomes a way
    /// of putting someone else on the spot.
    #[test]
    fn no_prompt_singles_anybody_out() {
        for prompt in all(Kind::Truth).into_iter().chain(all(Kind::Dare)) {
            assert!(!prompt.text.contains('@'), "{} mentions somebody", prompt.key);
        }
    }
}
