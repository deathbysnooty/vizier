//! Two small pieces of memory the `/image` command needs: the last picture
//! posted in the channel, and how often people are allowed to ask.
//!
//! **The last picture.** The biggest thing NotSoBot gets right is that you can
//! run an effect with no argument at all and it picks up whatever was just
//! posted. Reading the channel's history on every call would be a request to
//! Discord per command and a rate limit of its own, so instead every message
//! that lands in the image channel leaves its picture's address here, in a
//! short list per channel with a time on it. Looking one up costs a lock and a
//! hash.
//!
//! **The bucket.** Every effect shares one allowance, per channel and per
//! member, which is how NotSoBot does it: a burst of `/image magik` is one
//! person hogging a CPU, whichever effect they name. A refusal is never public,
//! and a *fast* refusal is not even words - see [`Verdict`].
//!
//! Both stores take the time as an argument, so the tests can move the clock
//! without sleeping, and both are swept rather than left to grow.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// How long a remembered picture is still offered as "the last one".
pub const REMEMBER_FOR: Duration = Duration::from_secs(3 * 3600);
/// How many pictures are kept per channel, newest first.
const KEEP_PER_CHANNEL: usize = 8;
/// How many channels are remembered at all before the oldest are swept.
const KEEP_CHANNELS: usize = 64;
/// A refusal this soon after the member's last try is not worth words.
pub const QUIET: Duration = Duration::from_secs(1);
/// The emoji a quiet refusal leaves instead of a message.
pub const SLOW_DOWN: &str = "🐢";

// --- the last picture in the channel -----------------------------------------------------------

#[derive(Default)]
pub struct Recent {
    per_channel: HashMap<u64, VecDeque<(Instant, String)>>,
}

impl Recent {
    /// Put a picture at the front of a channel's list.
    pub fn note(&mut self, now: Instant, channel: u64, url: String) {
        if url.is_empty() {
            return;
        }
        if self.per_channel.len() > KEEP_CHANNELS && !self.per_channel.contains_key(&channel) {
            self.per_channel.retain(|_, list| list.front().is_some_and(|(at, _)| now.duration_since(*at) < REMEMBER_FOR));
            if self.per_channel.len() > KEEP_CHANNELS {
                self.per_channel.clear();
            }
        }
        let list = self.per_channel.entry(channel).or_default();
        // The same picture posted twice moves up rather than appearing twice.
        list.retain(|(_, u)| *u != url);
        list.push_front((now, url));
        while list.len() > KEEP_PER_CHANNEL {
            list.pop_back();
        }
    }

    /// The newest picture still inside the window.
    pub fn latest(&self, now: Instant, channel: u64) -> Option<String> {
        self.per_channel
            .get(&channel)?
            .iter()
            .find(|(at, _)| now.duration_since(*at) < REMEMBER_FOR)
            .map(|(_, u)| u.clone())
    }

    /// What is remembered for a channel, newest first - for the tests, and for
    /// a future "the one before that".
    pub fn all(&self, channel: u64) -> Vec<String> {
        self.per_channel.get(&channel).map(|l| l.iter().map(|(_, u)| u.clone()).collect()).unwrap_or_default()
    }
}

/// Whether an attachment is a picture the toolkit can read. Discord's
/// `content_type` is usually right and occasionally missing, so the filename
/// gets a say too - and neither is trusted further than choosing what to try.
pub fn looks_like_an_image(filename: &str, content_type: Option<&str>) -> bool {
    if let Some(kind) = content_type {
        let kind = kind.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
        if let Some(rest) = kind.strip_prefix("image/") {
            return matches!(rest, "png" | "jpeg" | "jpg" | "webp" | "gif");
        }
        if !kind.is_empty() && kind != "application/octet-stream" {
            return false;
        }
    }
    let lower = filename.to_ascii_lowercase();
    // Take the name off any query string a CDN has hung on the end.
    let lower = lower.split(['?', '#']).next().unwrap_or(&lower);
    [".png", ".jpg", ".jpeg", ".webp", ".gif"].iter().any(|ext| lower.ends_with(ext))
}

fn store() -> &'static Mutex<Recent> {
    static STORE: OnceLock<Mutex<Recent>> = OnceLock::new();
    STORE.get_or_init(Mutex::default)
}

/// Remember a picture posted in a channel.
pub fn note_image(channel: u64, url: String) {
    store().lock().note(Instant::now(), channel, url);
}

/// The last picture posted in a channel, if one is still remembered.
pub fn last_image(channel: u64) -> Option<String> {
    store().lock().latest(Instant::now(), channel)
}

// --- the bucket --------------------------------------------------------------------------------

/// What to do about a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Go,
    /// Over the limit, and asked again too soon to deserve words: leave the
    /// emoji and say nothing. This is what keeps a channel from filling with
    /// "slow down" notices, which is the failure mode of every rate limit that
    /// answers in words.
    Quiet(&'static str),
    /// Over the limit, said once, quietly, to the member only.
    Tell(String),
}

#[derive(Clone, Copy, Debug)]
pub struct Rules {
    pub per_member: u32,
    pub per_channel: u32,
    pub window: Duration,
}

impl Default for Rules {
    fn default() -> Rules {
        Rules { per_member: 4, per_channel: 12, window: Duration::from_secs(60) }
    }
}

#[derive(Default)]
pub struct Buckets {
    channel: HashMap<u64, Vec<Instant>>,
    member: HashMap<u64, Vec<Instant>>,
    /// When each member last asked for anything, and when they were last told
    /// no in words.
    tried: HashMap<u64, Instant>,
    told: HashMap<u64, Instant>,
}

fn prune(list: &mut Vec<Instant>, now: Instant, window: Duration) {
    list.retain(|at| now.duration_since(*at) < window);
}

impl Buckets {
    /// Whether this member may run an effect in this channel now, and what to
    /// say if not. Counts the attempt only when it is allowed, so a refusal
    /// does not push the member further into the hole.
    pub fn ask(&mut self, now: Instant, channel: u64, member: u64, rules: &Rules) -> Verdict {
        let window = rules.window.max(Duration::from_secs(1));
        if self.channel.len() + self.member.len() > 1024 {
            self.channel.retain(|_, l| l.iter().any(|at| now.duration_since(*at) < window));
            self.member.retain(|_, l| l.iter().any(|at| now.duration_since(*at) < window));
            self.tried.retain(|_, at| now.duration_since(*at) < window);
            self.told.retain(|_, at| now.duration_since(*at) < window);
        }
        let last_try = self.tried.insert(member, now);
        let mine = self.member.entry(member).or_default();
        prune(mine, now, window);
        let mine_now = mine.len() as u32;
        let theirs = self.channel.entry(channel).or_default();
        prune(theirs, now, window);
        let theirs_now = theirs.len() as u32;

        let over = mine_now >= rules.per_member.max(1) || theirs_now >= rules.per_channel.max(1);
        if !over {
            self.member.entry(member).or_default().push(now);
            self.channel.entry(channel).or_default().push(now);
            return Verdict::Go;
        }
        // Rapid-fire, or already told in this window: the emoji, not a message.
        let rapid = last_try.is_some_and(|at| now.duration_since(at) < QUIET);
        let already = self.told.get(&member).is_some_and(|at| now.duration_since(*at) < window);
        if rapid || already {
            return Verdict::Quiet(SLOW_DOWN);
        }
        self.told.insert(member, now);
        let secs = window.as_secs().max(1);
        Verdict::Tell(if mine_now >= rules.per_member.max(1) {
            format!("You've had {} of those in the last {}s - give it a moment.", mine_now, secs)
        } else {
            format!("This channel has had {} of those in the last {}s - give it a moment.", theirs_now, secs)
        })
    }
}

fn buckets() -> &'static Mutex<Buckets> {
    static BUCKETS: OnceLock<Mutex<Buckets>> = OnceLock::new();
    BUCKETS.get_or_init(Mutex::default)
}

/// The live bucket.
pub fn ask(channel: u64, member: u64, rules: &Rules) -> Verdict {
    buckets().lock().ask(Instant::now(), channel, member, rules)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAN: u64 = 1_516_534_303_968_858_312;

    #[test]
    fn the_last_picture_posted_is_the_one_offered() {
        let t = Instant::now();
        let mut r = Recent::default();
        assert_eq!(r.latest(t, CHAN), None, "nothing posted yet");
        r.note(t, CHAN, "https://cdn.example/one.png".into());
        r.note(t + Duration::from_secs(1), CHAN, "https://cdn.example/two.png".into());
        assert_eq!(r.latest(t + Duration::from_secs(2), CHAN).as_deref(), Some("https://cdn.example/two.png"));
    }

    #[test]
    fn one_channel_never_answers_for_another() {
        let t = Instant::now();
        let mut r = Recent::default();
        r.note(t, CHAN, "https://cdn.example/here.png".into());
        assert_eq!(r.latest(t, 999), None);
    }

    #[test]
    fn a_picture_is_forgotten_once_it_is_stale() {
        let t = Instant::now();
        let mut r = Recent::default();
        r.note(t, CHAN, "https://cdn.example/old.png".into());
        assert!(r.latest(t + REMEMBER_FOR - Duration::from_secs(1), CHAN).is_some());
        assert_eq!(r.latest(t + REMEMBER_FOR + Duration::from_secs(1), CHAN), None);
    }

    #[test]
    fn the_list_is_bounded_and_holds_no_duplicates() {
        let t = Instant::now();
        let mut r = Recent::default();
        for i in 0..40 {
            r.note(t + Duration::from_secs(i), CHAN, format!("https://cdn.example/{i}.png"));
        }
        assert_eq!(r.all(CHAN).len(), KEEP_PER_CHANNEL);
        assert_eq!(r.latest(t + Duration::from_secs(40), CHAN).as_deref(), Some("https://cdn.example/39.png"));
        // The same address twice is one entry, moved to the front.
        let mut r = Recent::default();
        r.note(t, CHAN, "a".into());
        r.note(t, CHAN, "b".into());
        r.note(t, CHAN, "a".into());
        assert_eq!(r.all(CHAN), vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn many_channels_do_not_grow_the_store_without_end() {
        let t = Instant::now();
        let mut r = Recent::default();
        for i in 0..500u64 {
            r.note(t, i, format!("https://cdn.example/{i}.png"));
        }
        assert!(r.per_channel.len() <= KEEP_CHANNELS + 1, "{} channels kept", r.per_channel.len());
    }

    #[test]
    fn what_counts_as_a_picture() {
        for (name, kind) in [
            ("cat.png", Some("image/png")),
            ("cat.JPG", Some("image/jpeg")),
            ("cat.webp", Some("image/webp")),
            ("loop.gif", Some("image/gif")),
            ("cat.png", None),
            ("cat.jpeg?ex=123&is=456", None),
            ("blob", Some("image/png")),
            ("cat.png", Some("application/octet-stream")),
        ] {
            assert!(looks_like_an_image(name, kind), "{} / {:?} was not taken for a picture", name, kind);
        }
        for (name, kind) in [
            ("notes.txt", Some("text/plain")),
            ("clip.mp4", Some("video/mp4")),
            ("song.mp3", Some("audio/mpeg")),
            ("thing.svg", Some("image/svg+xml")),
            ("sheet.pdf", Some("application/pdf")),
            ("archive.zip", None),
            ("no-extension", None),
        ] {
            assert!(!looks_like_an_image(name, kind), "{} / {:?} was taken for a picture", name, kind);
        }
    }

    // --- the bucket ---

    fn rules() -> Rules {
        Rules { per_member: 3, per_channel: 5, window: Duration::from_secs(60) }
    }

    #[test]
    fn a_member_gets_their_allowance_and_then_is_stopped() {
        let t = Instant::now();
        let mut b = Buckets::default();
        for i in 0..3 {
            assert_eq!(b.ask(t + Duration::from_secs(i * 5), CHAN, 7, &rules()), Verdict::Go, "try {}", i);
        }
        let next = b.ask(t + Duration::from_secs(20), CHAN, 7, &rules());
        assert!(matches!(next, Verdict::Tell(_)), "the fourth should be refused, got {:?}", next);
    }

    /// The point of the thing: a burst gets the emoji, not four messages.
    #[test]
    fn a_burst_is_answered_with_an_emoji_and_nothing_else() {
        let t = Instant::now();
        let mut b = Buckets::default();
        for i in 0..3 {
            assert_eq!(b.ask(t + Duration::from_millis(i * 100), CHAN, 7, &rules()), Verdict::Go);
        }
        // The fourth, hard on the heels of the third: no words at all.
        assert_eq!(b.ask(t + Duration::from_millis(350), CHAN, 7, &rules()), Verdict::Quiet(SLOW_DOWN));
        // And the fifth, and the sixth.
        for i in 4..10 {
            assert_eq!(b.ask(t + Duration::from_millis(350 + i * 50), CHAN, 7, &rules()), Verdict::Quiet(SLOW_DOWN));
        }
    }

    /// Words once, then never again inside the window, however long the gaps.
    #[test]
    fn the_words_are_said_once_and_then_the_emoji_takes_over() {
        let t = Instant::now();
        let mut b = Buckets::default();
        for i in 0..3 {
            b.ask(t + Duration::from_secs(i), CHAN, 7, &rules());
        }
        let first = b.ask(t + Duration::from_secs(10), CHAN, 7, &rules());
        assert!(matches!(first, Verdict::Tell(_)), "the first refusal should explain itself");
        let second = b.ask(t + Duration::from_secs(20), CHAN, 7, &rules());
        assert_eq!(second, Verdict::Quiet(SLOW_DOWN), "the second should not spam");
    }

    #[test]
    fn the_allowance_comes_back_once_the_window_has_passed() {
        let t = Instant::now();
        let mut b = Buckets::default();
        for i in 0..3 {
            b.ask(t + Duration::from_secs(i), CHAN, 7, &rules());
        }
        assert!(matches!(b.ask(t + Duration::from_secs(5), CHAN, 7, &rules()), Verdict::Tell(_)));
        assert_eq!(b.ask(t + Duration::from_secs(120), CHAN, 7, &rules()), Verdict::Go);
    }

    /// One busy channel, several people: the channel's own allowance stops it
    /// even though nobody has used up their own.
    #[test]
    fn the_channel_has_an_allowance_of_its_own() {
        let t = Instant::now();
        let mut b = Buckets::default();
        for who in 0..5u64 {
            assert_eq!(b.ask(t + Duration::from_secs(who * 2), CHAN, who, &rules()), Verdict::Go, "member {}", who);
        }
        let sixth = b.ask(t + Duration::from_secs(12), CHAN, 99, &rules());
        assert!(matches!(sixth, Verdict::Tell(said) if said.contains("channel")), "the channel cap should say so");
        // A different channel is unaffected.
        assert_eq!(b.ask(t + Duration::from_secs(12), 4242, 99, &rules()), Verdict::Go);
    }

    #[test]
    fn a_refusal_does_not_cost_the_member_their_next_turn() {
        let t = Instant::now();
        let mut b = Buckets::default();
        let one = Rules { per_member: 1, per_channel: 99, window: Duration::from_secs(10) };
        assert_eq!(b.ask(t, CHAN, 7, &one), Verdict::Go);
        // Refused twice, then the window passes: one Go, not none.
        assert!(matches!(b.ask(t + Duration::from_secs(2), CHAN, 7, &one), Verdict::Tell(_)));
        assert_eq!(b.ask(t + Duration::from_secs(3), CHAN, 7, &one), Verdict::Quiet(SLOW_DOWN));
        assert_eq!(b.ask(t + Duration::from_secs(20), CHAN, 7, &one), Verdict::Go);
    }

    #[test]
    fn a_nonsense_rule_still_lets_somebody_through_eventually() {
        let t = Instant::now();
        let mut b = Buckets::default();
        let zero = Rules { per_member: 0, per_channel: 0, window: Duration::ZERO };
        // Zero is read as one, so the first ask works and the second does not.
        assert_eq!(b.ask(t, CHAN, 7, &zero), Verdict::Go);
        assert!(!matches!(b.ask(t + Duration::from_millis(10), CHAN, 7, &zero), Verdict::Go));
    }
}
