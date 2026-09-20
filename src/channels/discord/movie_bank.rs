//! The Guess the Movie bank: the films the bot asks about, the clues it puts
//! up, and what it takes as naming one.
//!
//! One file, `<workspace>/moviebank/movies.json`, read once at start the way
//! the doodle bank reads `drawbank/`. It is built from the JSONL files in
//! `moviebank/ai/` by `moviebank/tools/build.py`; `moviebank/GUIDE.md` is the
//! guide for whoever writes them.
//!
//! A film plays three kinds of clue, and a round puts up exactly one:
//!
//! * [`Clue::Tags`] — five words about it, vague first and sharp last:
//!   *revenge · guns · coal mafia · uttar pradesh · a butcher's family*. `!hint`
//!   reveals the next one down, which is why a film carries more tags than it
//!   shows.
//! * [`Clue::Hint`] — one line about the film in its own words: *a scientist
//!   leads a secret wartime project*. A film needs no picture to play one, which
//!   is what lets the half of the bank with no stills still ask three ways.
//! * [`Clue::Dialogue`] — a line the room would know.
//! * [`Clue::Shot`] — a still from a scene, hosted by TMDB and named here only
//!   by its path. A film with no still worth showing simply never draws one
//!   ([`Movie::kinds`]), and plays tags and dialogue instead.
//!
//! # Judging a guess
//!
//! Film titles are typed badly. They are long, half of them are transliterated
//! out of Hindi with no agreed spelling, and people guess them on phones — so
//! this bank forgives far more than the doodle bank does, and leans on one hard
//! guard rather than on being strict.
//!
//! Both a guess and every answer come down to the same string ([`key`]): number
//! words become digits, a leading "the" goes, punctuation goes, and the
//! spellings of a transliterated word are folded together, so `dilwaale`,
//! `dilwale` and `dilvale` are one word. After that a slip or two is forgiven by
//! length — about a fifth of the title — but never when another film in the
//! bank is just as near.
//!
//! Two things are never forgiven. **Digits must match exactly**, so `don` can
//! never take *Don 2* however near it looks; and a guess that lands equally near
//! two films takes neither. That guard is what makes the rest safe to be loose
//! with, and `moviebank/tools/check_bank.py` fails the bank if any two films
//! are close enough for it to matter.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use parking_lot::RwLock;

use serde::Deserialize;

use super::frog_answer::levenshtein;
use super::sudoku_gen::Rng;

/// The credit TMDB's terms ask for, wherever the stills or the data appear -
/// and TVmaze's, whose CC BY-SA metadata the television entries are built from.
/// `moviebank/CREDITS.md` names every series; this is the line that travels
/// with the game wherever its rules are told.
pub const ATTRIBUTION: &str =
    "Film stills and data from TMDB (https://www.themoviedb.org). This product uses the TMDB API but is not endorsed or certified by TMDB. TV series data from TVmaze (https://www.tvmaze.com), CC BY-SA.";

/// Where a still is actually fetched from. The bank stores only the path, so
/// nothing is downloaded, cached or served by us.
pub const IMAGE_BASE: &str = "https://image.tmdb.org/t/p/w1280";

/// The format `movies.json` announces. A file that says anything else is left
/// alone rather than half-read.
pub const FORMAT: &str = "MOVIEBANK1";

/// Longer than this and a message can't be a title at all.
const MAX_GUESS: usize = 96;

/// Joining words a title can lose without becoming a different title, so
/// "gangs wasseypur" still names *Gangs of Wasseypur*.
const JOINING: [&str; 12] = ["of", "the", "a", "an", "and", "aur", "ki", "ke", "ka", "na", "in", "to"];

/// The bank in play, and where it was read from so it can be read again.
///
/// The old copy is leaked on purpose when a new one is installed. `&'static` is
/// what lets a round hold on to a film without copying it, and every caller in
/// the game is written that way; a reload is something a mod does when a pack
/// of films lands, not something that runs, so a few megabytes left behind is
/// the cheaper half of that trade.
static BANK: RwLock<Option<&'static Bank>> = RwLock::new(None);
static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();

// --- reducing what was typed ------------------------------------------------------------------

/// Number words as digits, so "three idiots" and "3 idiots" are one film.
fn numbers(text: &str) -> String {
    // Roman numerals are in here because that is how a sequel usually writes
    // itself: Gladiator II is Gladiator 2, and the digit guard then keeps it
    // apart from Gladiator. NEVER the single letters — "V for Vendetta" and
    // "I Am Legend" are titles, not numbers.
    const WORDS: [(&str, &str); 20] = [
        ("one", "1"),
        ("two", "2"),
        ("three", "3"),
        ("four", "4"),
        ("five", "5"),
        ("six", "6"),
        ("seven", "7"),
        ("eight", "8"),
        ("nine", "9"),
        ("ten", "10"),
        ("ii", "2"),
        ("iii", "3"),
        ("iv", "4"),
        ("vi", "6"),
        ("vii", "7"),
        ("viii", "8"),
        ("ix", "9"),
        ("xi", "11"),
        ("xii", "12"),
        ("xiii", "13"),
    ];
    let mut out = String::with_capacity(text.len());
    for word in text.to_lowercase().split_inclusive(|c: char| !c.is_alphanumeric()) {
        let (letters, tail) = match word.chars().last().filter(|c| !c.is_alphanumeric()) {
            Some(c) => (&word[..word.len() - c.len_utf8()], &word[word.len() - c.len_utf8()..]),
            None => (word, ""),
        };
        match WORDS.iter().find(|(w, _)| *w == letters) {
            Some((_, digit)) => out.push_str(digit),
            None => out.push_str(letters),
        }
        out.push_str(tail);
    }
    out
}

/// Lower case, letters and digits only: "Spider-Man" comes to "spiderman".
pub fn plain(text: &str) -> String {
    numbers(text).chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Transliteration folding, on a plain form.
///
/// A Hindi title reaches us spelled a dozen ways and none of them is a mistake
/// anybody should lose a round over. Folding the spellings onto one string does
/// far more work here than edit distance could: it is what makes `khushee` and
/// `khushi`, or `wasseypur` and `vaseypur`, the same word before anything is
/// measured.
pub fn fold(plain: &str) -> String {
    let mut s = plain.to_string();
    for (was, now) in [("ksh", "x"), ("ph", "f"), ("ee", "i"), ("oo", "u"), ("w", "v"), ("y", "i")] {
        s = s.replace(was, now);
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if out.chars().last() != Some(c) {
            out.push(c);
        }
    }
    out
}

/// What a title or a guess is compared as.
pub fn key(text: &str) -> String {
    let trimmed = text.trim();
    let lower = trimmed.to_lowercase();
    let bare = ["the ", "an ", "a "].iter().find_map(|art| lower.strip_prefix(*art)).unwrap_or(&lower);
    fold(&plain(bare))
}

/// The digits in a string, in order. A sequel number is never forgiven, so this
/// is compared before anything else is measured.
pub fn digits(text: &str) -> String {
    numbers(text).chars().filter(char::is_ascii_digit).collect()
}

/// A title's words with the joining ones dropped, each folded: what lets
/// "gangs wasseypur" take *Gangs of Wasseypur*. Sorted, so word order doesn't
/// decide a round.
pub fn words(text: &str) -> Vec<String> {
    let lowered = numbers(text);
    let mut out: Vec<String> = lowered
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !JOINING.contains(&w.to_lowercase().as_str()))
        .map(|w| fold(&plain(w)))
        .filter(|w| !w.is_empty())
        .collect();
    out.sort();
    out
}

/// How many slips a title of this many letters forgives: about a fifth of it.
/// People type these on phones, and a long title typed nearly right is still
/// somebody who knew the film.
pub fn slack(letters: usize) -> usize {
    match letters {
        0..=5 => 1,
        6..=10 => 2,
        11..=16 => 3,
        17..=24 => 4,
        _ => 5,
    }
}

/// What someone typed, reduced every way a guess is judged by — or `None` when
/// the message can't be a guess at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Guess {
    pub key: String,
    pub words: Vec<String>,
    pub digits: String,
}

pub fn read_guess(text: &str) -> Option<Guess> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_GUESS {
        return None;
    }
    // `!hint` and `!skip` steer the round; they are never guesses.
    if trimmed.starts_with('!') || trimmed.starts_with('/') {
        return None;
    }
    let key = key(trimmed);
    (!key.is_empty()).then(|| Guess { key, words: words(trimmed), digits: digits(trimmed) })
}

// --- what a film is ----------------------------------------------------------------------------

/// Which of the two the film belongs to. The channel can be leaned one way or
/// the other without touching the bank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Industry {
    Bollywood,
    Hollywood,
}

impl Industry {
    pub fn label(self) -> &'static str {
        match self {
            Industry::Bollywood => "Hindi",
            Industry::Hollywood => "English",
        }
    }
}

/// A film, or a television series. The bank was films alone to begin with, so
/// `film` is what an entry means when it says nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Film,
    Series,
}

impl Kind {
    /// What to call one when the answer is known: "film", "show".
    pub fn word(self) -> &'static str {
        match self {
            Kind::Film => "film",
            Kind::Series => "show",
        }
    }
}

/// Roughly 2010 on is `Modern`; everything older has to have earned it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Era {
    Iconic,
    Modern,
}

/// A corner of the bank a match can be played in, chosen by the room before it
/// starts. `Mix` is everything, and what a match plays when nobody says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Pool {
    #[default]
    Mix,
    HindiFilms,
    EnglishFilms,
    HindiShows,
    EnglishShows,
}

impl Pool {
    pub const ALL: [Pool; 5] = [Pool::HindiFilms, Pool::EnglishFilms, Pool::HindiShows, Pool::EnglishShows, Pool::Mix];

    pub fn key(self) -> &'static str {
        match self {
            Pool::Mix => "mix",
            Pool::HindiFilms => "hindi_films",
            Pool::EnglishFilms => "english_films",
            Pool::HindiShows => "hindi_shows",
            Pool::EnglishShows => "english_shows",
        }
    }

    pub fn from_key(key: &str) -> Pool {
        Pool::ALL.into_iter().find(|p| p.key() == key).unwrap_or(Pool::Mix)
    }

    /// What the button says.
    pub fn label(self) -> &'static str {
        match self {
            Pool::Mix => "🎲 Mix",
            Pool::HindiFilms => "🎬 Hindi films",
            Pool::EnglishFilms => "🎞️ English films",
            Pool::HindiShows => "📺 Hindi shows",
            Pool::EnglishShows => "🍿 English shows",
        }
    }

    /// What the channel is told a match is playing.
    pub fn about(self) -> &'static str {
        match self {
            Pool::Mix => "anything in the bank",
            Pool::HindiFilms => "Hindi films",
            Pool::EnglishFilms => "English films",
            Pool::HindiShows => "Hindi TV shows",
            Pool::EnglishShows => "English TV shows",
        }
    }

    fn holds(self, movie: &Movie) -> bool {
        let (industry, kind) = match self {
            Pool::Mix => return true,
            Pool::HindiFilms => (Industry::Bollywood, Kind::Film),
            Pool::EnglishFilms => (Industry::Hollywood, Kind::Film),
            Pool::HindiShows => (Industry::Bollywood, Kind::Series),
            Pool::EnglishShows => (Industry::Hollywood, Kind::Series),
        };
        movie.industry == industry && movie.kind == kind
    }
}

/// What a round puts up. One to a round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Clue {
    /// Five words about the film, vague first and sharp last.
    Tags,
    /// One line about the film, in our own words.
    Hint,
    /// A line from it.
    Dialogue,
    /// A still from a scene.
    Shot,
}

impl Clue {
    pub const ALL: [Clue; 4] = [Clue::Tags, Clue::Hint, Clue::Dialogue, Clue::Shot];

    pub fn key(self) -> &'static str {
        match self {
            Clue::Tags => "tags",
            Clue::Hint => "hint",
            Clue::Dialogue => "dialogue",
            Clue::Shot => "shot",
        }
    }

    pub fn from_key(key: &str) -> Clue {
        Clue::ALL.into_iter().find(|c| c.key() == key).unwrap_or(Clue::Tags)
    }

    /// What the card calls it. The bank holds television as well as film now,
    /// and a card that asked "which film?" about an episode of one would be
    /// asking the wrong question - so every heading offers both, and which of
    /// the two it is stays part of the puzzle.
    pub fn heading(self) -> &'static str {
        match self {
            Clue::Tags => "Name the film or show",
            Clue::Hint => "Which film or show is this?",
            Clue::Dialogue => "Which film or show is this line from?",
            Clue::Shot => "Which film or show is this from?",
        }
    }
}

/// One approved still, from either of the two places a still can come from.
///
/// `file` is one of ours, a picture under the bank's own folder, which the bot
/// reads off disk and uploads with the card. `path` is one of TMDB's, named by
/// its place on their CDN and linked rather than copied. A still has one or the
/// other, never both.
///
/// `hint` is the one-line clue written for that particular still - what `!hint`
/// says out loud when it puts the second picture up. The note says what
/// somebody looked at and agreed to, so a still that quietly starts pointing at
/// something else can be spotted.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Shot {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub note: String,
}

impl Shot {
    /// Whether this one is a file of ours rather than a link to TMDB's.
    pub fn is_local(&self) -> bool {
        !self.file.is_empty()
    }

    /// Where TMDB serves it, for the stills that are theirs.
    pub fn url(&self) -> Option<String> {
        (!self.path.is_empty()).then(|| format!("{}{}", IMAGE_BASE, self.path))
    }

    /// The line written for this still, when it has one.
    pub fn hint(&self) -> Option<&str> {
        (!self.hint.is_empty()).then_some(self.hint.as_str())
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Movie {
    pub id: String,
    pub title: String,
    pub year: i64,
    pub industry: Industry,
    pub era: Era,
    /// Film unless it says otherwise - which is how 917 entries written before
    /// television existed here stay exactly as they are.
    #[serde(default)]
    pub kind: Kind,
    /// Every spelling that names it, the plain title first.
    pub answers: Vec<String>,
    /// Six to eight, vague first and sharp last. The card shows the first few
    /// and a hint reveals the next.
    #[serde(default)]
    pub tags: Vec<String>,
    /// One-line clues about the film, ours rather than anybody's dialogue. A
    /// film with no still worth showing still asks three ways because of these.
    #[serde(default)]
    pub hints: Vec<String>,
    #[serde(default)]
    pub dialogues: Vec<String>,
    #[serde(default)]
    pub shots: Vec<Shot>,
    /// The answers reduced to what a guess is compared against.
    #[serde(skip)]
    keys: Vec<String>,
    /// And their significant words, so a dropped "of" costs nobody a round.
    #[serde(skip)]
    word_sets: Vec<Vec<String>>,
}

impl Movie {
    /// The film's own key, which the store writes down so it doesn't come round
    /// again inside the no-repeat window.
    pub fn key(&self) -> String {
        key(&self.title)
    }

    /// The letter a hint gives away.
    pub fn first_letter(&self) -> char {
        self.title.chars().next().unwrap_or('?').to_ascii_uppercase()
    }

    /// The kinds of clue this film can actually play. A film with no still
    /// worth showing is normal — it plays the other two and nobody notices.
    pub fn kinds(&self) -> Vec<Clue> {
        let mut kinds = Vec::with_capacity(3);
        if !self.tags.is_empty() {
            kinds.push(Clue::Tags);
        }
        if !self.hints.is_empty() {
            kinds.push(Clue::Hint);
        }
        if !self.dialogues.is_empty() {
            kinds.push(Clue::Dialogue);
        }
        if !self.shots.is_empty() {
            kinds.push(Clue::Shot);
        }
        kinds
    }

    /// The tags on the card: the vaguest few, in the order they were written.
    pub fn shown_tags(&self, shown: usize) -> Vec<String> {
        self.tags.iter().take(shown.max(1)).cloned().collect()
    }

    /// The tag a hint reveals — the next one down, which is sharper than
    /// anything already up. `None` once they have all been shown.
    pub fn next_tag(&self, shown: usize) -> Option<&str> {
        self.tags.get(shown).map(String::as_str)
    }

    /// The sharpest tag the film has - the last one, written to fit almost
    /// nothing else. What a hint gives a round that has shown no tags at all.
    pub fn sharpest_tag(&self) -> Option<&str> {
        self.tags.last().map(String::as_str)
    }

    pub fn hint(&self, which: usize) -> Option<&str> {
        self.hints.get(which).map(String::as_str)
    }

    pub fn dialogue(&self, which: usize) -> Option<&str> {
        self.dialogues.get(which).map(String::as_str)
    }

    pub fn shot(&self, which: usize) -> Option<&Shot> {
        self.shots.get(which)
    }

    /// How many of a kind it has, for picking one nobody has seen lately.
    fn count_of(&self, clue: Clue) -> usize {
        match clue {
            Clue::Tags => 1,
            Clue::Hint => self.hints.len(),
            Clue::Dialogue => self.dialogues.len(),
            Clue::Shot => self.shots.len(),
        }
    }
}

/// What `movies.json` says.
#[derive(Deserialize)]
struct Manifest {
    format: String,
    movies: Vec<Movie>,
}

#[derive(Debug, Default)]
pub struct Bank {
    movies: Vec<Movie>,
    /// The folder the bank was read from. A still of ours is named relative to
    /// it, so the game can find the picture without knowing the workspace.
    dir: std::path::PathBuf,
}

impl Bank {
    pub fn from_json(text: &str) -> anyhow::Result<Bank> {
        let manifest: Manifest = serde_json::from_str(text).map_err(|err| anyhow::anyhow!("movies.json: {}", err))?;
        if manifest.format != FORMAT {
            anyhow::bail!("movies.json says format {}, which this reader doesn't know", manifest.format);
        }
        let mut movies = Vec::with_capacity(manifest.movies.len());
        for mut movie in manifest.movies {
            movie.keys = movie.answers.iter().map(|a| key(a)).filter(|a| !a.is_empty()).collect();
            movie.word_sets = movie.answers.iter().map(|a| words(a)).filter(|w| !w.is_empty()).collect();
            // A film nobody can name, or one with nothing to ask about, is left
            // out rather than put up as a round that can't be played.
            if movie.keys.is_empty() || movie.kinds().is_empty() {
                tracing::warn!("movie: {} left out of the bank — no answers or no clues", movie.title);
                continue;
            }
            movies.push(movie);
        }
        Ok(Bank { movies, dir: std::path::PathBuf::new() })
    }

    pub fn count(&self) -> usize {
        self.movies.len()
    }

    /// Where one of our own stills really lives. `None` for a TMDB one, which
    /// is linked and never on disk.
    pub fn shot_path(&self, shot: &Shot) -> Option<std::path::PathBuf> {
        shot.is_local().then(|| self.dir.join(&shot.file))
    }

    /// How many of them are television rather than film.
    pub fn series_count(&self) -> usize {
        self.movies.iter().filter(|m| m.kind == Kind::Series).count()
    }

    pub fn shot_count(&self) -> usize {
        self.movies.iter().map(|m| m.shots.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.movies.is_empty()
    }

    pub fn movie(&self, index: usize) -> Option<&Movie> {
        self.movies.get(index)
    }

    /// The film a round wrote down, found again by its key. A bank edited under
    /// a live round simply loses it, which the game reads as a round it can no
    /// longer judge.
    pub fn find(&self, key: &str) -> Option<usize> {
        self.movies.iter().position(|m| m.key() == key)
    }

    /// A film to ask about: one that hasn't been up lately, leaning the way the
    /// settings ask.
    ///
    /// `hindi` and `modern` are the shares out of a hundred. They are a lean and
    /// not a rule: when nothing fresh fits the corner they ask for, the pick
    /// widens rather than the channel sitting empty, and when every film has
    /// been used the window starts over.
    pub fn pick(&self, used: &HashSet<String>, hindi: u64, modern: u64, rng: &mut Rng) -> Option<usize> {
        self.pick_in(Pool::Mix, used, hindi, modern, rng)
    }

    /// The same, inside the corner a match voted for.
    ///
    /// A voted pool is a rule where the shares are only a lean: a match that
    /// asked for Hindi shows is never quietly handed an English film. When
    /// everything in that corner has been up lately the window starts over
    /// inside it, and only a corner with nothing in it at all widens - which
    /// can happen when the room votes for a pool the bank has yet to be given.
    pub fn pick_in(&self, pool: Pool, used: &HashSet<String>, hindi: u64, modern: u64, rng: &mut Rng) -> Option<usize> {
        if self.movies.is_empty() {
            return None;
        }
        let inside: Vec<usize> = (0..self.movies.len()).filter(|i| pool.holds(&self.movies[*i])).collect();
        let inside = if inside.is_empty() { (0..self.movies.len()).collect() } else { inside };
        let want_hindi = rng.below(100) < hindi.min(100) as usize;
        let want_modern = rng.below(100) < modern.min(100) as usize;
        let industry = if want_hindi { Industry::Bollywood } else { Industry::Hollywood };
        let era = if want_modern { Era::Modern } else { Era::Iconic };
        let fresh = |i: &usize| !used.contains(&self.movies[*i].key());
        let start = rng.below(inside.len());
        let order: Vec<usize> = (0..inside.len()).map(|step| inside[(start + step) % inside.len()]).collect();
        // The corner asked for, then either half of it, then anything fresh at
        // all, then — every film having been up lately — anything.
        let exact = order.iter().copied().find(|i| fresh(i) && self.movies[*i].industry == industry && self.movies[*i].era == era);
        let same_industry = || order.iter().copied().find(|i| fresh(i) && self.movies[*i].industry == industry);
        let any_fresh = || order.iter().copied().find(fresh);
        exact.or_else(same_industry).or_else(any_fresh).or_else(|| order.first().copied())
    }

    /// How many titles a pool holds, for the buttons: a corner with nothing in
    /// it is not offered to vote for.
    pub fn pool_count(&self, pool: Pool) -> usize {
        self.movies.iter().filter(|m| pool.holds(m)).count()
    }

    /// Which clue to put up: a kind first, so a film with six stills and one
    /// line doesn't spend every round on stills, then one of that kind nobody
    /// has been shown lately.
    pub fn pick_clue(&self, movie: usize, used: &HashSet<String>, rng: &mut Rng) -> Option<(Clue, usize)> {
        let film = self.movies.get(movie)?;
        let kinds = film.kinds();
        if kinds.is_empty() {
            return None;
        }
        let unused = |clue: Clue, which: usize| !used.contains(&clue_key(clue, which));
        let mut order = kinds.clone();
        rng.shuffle(&mut order);
        // A kind with something fresh in it, or failing that the first kind it
        // has: a film whose every clue has been seen still gets a round.
        let kind = order.iter().copied().find(|c| (0..film.count_of(*c)).any(|w| unused(*c, w))).unwrap_or(order[0]);
        let count = film.count_of(kind);
        let start = rng.below(count);
        let which = (0..count).map(|step| (start + step) % count).find(|w| unused(kind, *w)).unwrap_or(start);
        Some((kind, which))
    }

    /// Whether a typed guess names the film.
    ///
    /// An exact answer wins, however it was punctuated or transliterated. The
    /// same title with a joining word dropped wins. Beyond that a slip is
    /// forgiven by length — but only when the digits match exactly, and only
    /// when no OTHER film in the bank is just as near.
    pub fn wins(&self, movie: usize, guess: &str) -> bool {
        let Some(guess) = read_guess(guess) else { return false };
        let Some(mine) = self.movies.get(movie) else { return false };
        if mine.keys.iter().any(|k| *k == guess.key) {
            return true;
        }
        if !guess.words.is_empty() && mine.word_sets.iter().any(|set| *set == guess.words) {
            // Word for word, joining words aside. Still not a win if it says
            // that about two films at once.
            let elsewhere = self.movies.iter().enumerate().any(|(i, other)| i != movie && other.word_sets.iter().any(|s| *s == guess.words));
            if !elsewhere {
                return true;
            }
        }
        let Some(near) = mine.keys.iter().filter_map(|key| near_miss(&guess, key)).min() else { return false };
        !self
            .movies
            .iter()
            .enumerate()
            .any(|(i, other)| i != movie && other.keys.iter().any(|key| digits(key) == guess.digits && levenshtein(&guess.key, key) <= near))
    }
}

/// How the used-clues window names one clue.
pub fn clue_key(clue: Clue, which: usize) -> String {
    format!("{}:{}", clue.key(), which)
}

/// How far a guess is from one answer, when that is near enough to forgive.
/// `None` when the digits disagree: a sequel number is exact or nothing.
fn near_miss(guess: &Guess, key: &str) -> Option<usize> {
    if digits(key) != guess.digits {
        return None;
    }
    let allowed = slack(guess.key.chars().count().max(key.chars().count()));
    let distance = levenshtein(&guess.key, key);
    (distance > 0 && distance <= allowed).then_some(distance)
}

/// Reads the bank out of a folder.
pub fn load(dir: &Path) -> anyhow::Result<Bank> {
    let path = dir.join("movies.json");
    let text = std::fs::read_to_string(&path).map_err(|err| anyhow::anyhow!("{}: {}", path.display(), err))?;
    let mut bank = Bank::from_json(&text)?;
    bank.dir = dir.to_path_buf();
    // A still of ours is a file on disk, and the pictures are big enough that a
    // workspace can easily have the bank without them. A round that asked
    // "which film is this from?" over an empty space would be unanswerable, so
    // a still whose file isn't there is dropped here and the film simply asks
    // another way.
    let mut missing = 0;
    for movie in &mut bank.movies {
        let dir = &bank.dir;
        movie.shots.retain(|shot| {
            let there = !shot.is_local() || dir.join(&shot.file).exists();
            missing += usize::from(!there);
            there
        });
    }
    if missing > 0 {
        tracing::warn!("movie: {} stills are named in the bank but not on disk — those films will ask another way", missing);
    }
    if bank.is_empty() {
        anyhow::bail!("{} has no films in it", path.display());
    }
    Ok(bank)
}

/// Reads `<workspace>/moviebank/` once, at start. A bank that isn't there is
/// not an error worth stopping for: the game stays off and says so, once.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["moviebank"]);
    let _ = DIR.set(dir.clone());
    match load(&dir) {
        Ok(bank) => {
            tracing::info!("movie: {} films and {} stills read from {}", bank.count(), bank.shot_count(), dir.display());
            install(bank);
        }
        Err(err) => tracing::warn!(
            "movie: no film bank ({}) — the game stays off. Build moviebank/movies.json with moviebank/tools/build.py.",
            err
        ),
    }
}

fn install(bank: Bank) {
    *BANK.write() = Some(Box::leak(Box::new(bank)));
}

/// Reads the folder again and puts what it finds in play, for `/moviereload`.
///
/// Films, tags and dialogue all live in `movies.json`, which was read once at
/// boot - so until now, a new pack of films meant restarting the bot. Pictures
/// never did: a still is read off disk as the card goes up. Returns what is in
/// play afterwards, whether or not that is what was there before.
pub fn reload() -> anyhow::Result<(usize, usize)> {
    let dir = DIR.get().ok_or_else(|| anyhow::anyhow!("the bank was never opened"))?;
    let bank = load(dir)?;
    let counts = (bank.count(), bank.shot_count());
    tracing::info!("movie: bank read again — {} titles and {} stills", counts.0, counts.1);
    install(bank);
    Ok(counts)
}

/// The bank, when there is one.
pub fn bank() -> Option<&'static Bank> {
    *BANK.read()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A bank small enough to read, with every trap in it: a title in two
    /// transliterations, a sequel pair a digit apart, two films one letter
    /// apart, a film with no still, and a film with nothing but tags.
    pub fn fixture() -> Bank {
        let json = r#"{"format": "MOVIEBANK1", "movies": [
            {"id": "a1", "title": "Dilwale Dulhania Le Jayenge", "year": 1995, "industry": "bollywood", "era": "iconic",
             "answers": ["dilwale dulhania le jayenge", "ddlj"],
             "tags": ["romance", "train", "europe", "mustard fields", "a hand out of a moving train", "the last sharp one"],
             "dialogues": ["Bade bade deshon mein"],
             "shots": [{"path": "/one.jpg", "note": "a field"}, {"path": "/two.jpg", "note": "a train"}]},
            {"id": "a4", "title": "Heat", "year": 1995, "industry": "hollywood", "era": "iconic",
             "answers": ["heat"], "tags": ["a thief", "a detective", "a coffee shop", "a bank job"],
             "dialogues": [],
             "shots": [{"file": "images/h1.jpg", "hint": "A crew plans one last job.", "note": "first"},
                       {"file": "images/h2.jpg", "hint": "Two men meet over coffee.", "note": "second"}]},
            {"id": "a2", "title": "Don", "year": 2006, "industry": "bollywood", "era": "modern",
             "answers": ["don"], "tags": ["crime", "a lookalike", "malaysia", "a police plan"],
             "dialogues": ["Don ko pakadna"], "shots": []},
            {"id": "a3", "title": "Don 2", "year": 2011, "industry": "bollywood", "era": "modern",
             "answers": ["don 2"], "tags": ["berlin", "a heist", "a prison break", "a printing press"],
             "dialogues": [], "shots": [{"path": "/three.jpg", "note": "a vault"}]},
            {"id": "b1", "title": "Mouse", "year": 2015, "industry": "hollywood", "era": "modern",
             "answers": ["mouse"], "tags": ["small", "grey", "a kitchen", "a trap"], "dialogues": [], "shots": []},
            {"id": "b2", "title": "House", "year": 2016, "industry": "hollywood", "era": "modern",
             "answers": ["house"], "tags": ["bricks", "a door", "a garden", "a roof"], "dialogues": [], "shots": []},
            {"id": "b3", "title": "The Matrix", "year": 1999, "industry": "hollywood", "era": "iconic",
             "answers": ["the matrix", "matrix"], "tags": ["a hacker", "a simulation", "green code", "a red pill"],
             "dialogues": ["There is no spoon"], "shots": [{"path": "/four.jpg", "note": "a chair"}]}
        ]}"#;
        Bank::from_json(json).expect("a bank")
    }

    /// Words a title can contain without a clue that uses them giving anything
    /// away - "One Battle After Another" is not spoiled by "after". Mirrors the
    /// same list in `moviebank/tools/check_bank.py`.
    /// Already folded, because that is what they are compared against: "with"
    /// reduces to "vith", and the unfolded spelling would never match.
    const COMMON: [&str; 10] = ["after", "another", "about", "before", "over", "under", "into", "from", "vith", "other"];

    #[test]
    fn a_guess_comes_down_to_the_same_string_however_it_is_typed() {
        assert_eq!(plain("Spider-Man!"), "spiderman");
        assert_eq!(plain("three idiots"), "3idiots");
        assert_eq!(key("3 Idiots"), key("Three Idiots"));
        assert_eq!(key("The Matrix"), "matrix", "a leading article is dropped");
        assert_eq!(key("Matrix"), "matrix");
        // The whole point of folding: one Hindi title, spelled every way.
        let ddlj = key("Dilwale Dulhania Le Jayenge");
        for same in ["dilwaale dulhaniya le jayenge", "Dilvale Dulhania Le Jayenge", "DILWALE DULHANIA LE JAYENGE"] {
            assert_eq!(key(same), ddlj, "{:?} is the same title", same);
        }
        assert_eq!(key("Wasseypur"), key("Vaseypur"));
        assert_eq!(key("Khushee"), key("Khushi"));
        // Joining words don't decide a title, and word order doesn't either.
        assert_eq!(words("Gangs of Wasseypur"), words("wasseypur gangs"));
        assert_eq!(digits("Don 2"), "2");
        assert_eq!(digits("Don"), "");
        assert_eq!(digits("three idiots"), "3", "a number word is a digit");
        // A sequel writes its number in roman, and that is still a number.
        assert_eq!(key("Gladiator II"), key("gladiator 2"));
        assert_eq!(key("The Godfather Part II"), key("the godfather part 2"));
        assert_ne!(key("Gladiator II"), key("Gladiator"));
        assert_eq!(digits("Gladiator II"), "2");
        // But a single letter is a title, never a number.
        assert_eq!(digits("V for Vendetta"), "", "V is not five");
        assert_eq!(digits("I Am Legend"), "", "I is not one");
        assert_eq!(key("X-Men"), "xmen");
        // What can't be a guess at all.
        assert!(read_guess("!hint").is_none() && read_guess("/movie").is_none());
        assert!(read_guess("   ").is_none() && read_guess("🎬").is_none());
        assert!(read_guess(&"a".repeat(MAX_GUESS + 1)).is_none(), "an essay is not a guess");
    }

    #[test]
    fn a_title_wins_typed_badly_but_never_past_another_film() {
        let bank = fixture();
        let ddlj = bank.find(&key("Dilwale Dulhania Le Jayenge")).expect("ddlj");
        for yes in [
            "Dilwale Dulhania Le Jayenge",
            "dilwaale dulhaniya le jaenge",
            "DDLJ",
            "ddlj!",
            "dilwale dulhania le jayange",
            "dilwale dulhaniya jayenge",
        ] {
            assert!(bank.wins(ddlj, yes), "{:?} names it", yes);
        }
        for no in ["sholay", "dilwale", "", "🎬", "the matrix"] {
            assert!(!bank.wins(ddlj, no), "{:?} does not", no);
        }
        // A slip that lands as near one film as another is a guess, not an
        // answer: nobody takes a Mouse by nearly typing a House.
        let (mouse, house) = (bank.find("mouse").expect("mouse"), bank.find("house").expect("house"));
        assert!(!bank.wins(mouse, "youse") && !bank.wins(house, "youse"), "one letter from both");
        assert!(bank.wins(mouse, "Mouse.") && bank.wins(house, "HOUSE"), "and each is still its own title typed out");
    }

    /// Two different films can be called the same thing - Fighter (2024) and
    /// The Fighter (2010). An exact answer wins outright, before any of the
    /// near-miss machinery runs, so each of them is won by whoever names it
    /// while it is the film on the card. Nobody is asked to guess which.
    #[test]
    fn one_title_on_two_films_still_wins_whichever_is_up() {
        let json = r#"{"format": "MOVIEBANK1", "movies": [
            {"id": "a", "title": "Fighter", "year": 2024, "industry": "bollywood", "era": "modern",
             "answers": ["fighter"], "tags": ["a squadron", "kashmir", "a dogfight", "a captured pilot"]},
            {"id": "b", "title": "The Fighter", "year": 2010, "industry": "hollywood", "era": "modern",
             "answers": ["the fighter"], "tags": ["boxing", "lowell", "a half brother", "a comeback"]}
        ]}"#;
        let bank = Bank::from_json(json).expect("a bank");
        assert_eq!(bank.count(), 2, "both films are kept");
        assert_eq!(key("Fighter"), key("The Fighter"), "they really do reduce to one answer");
        for which in 0..2 {
            assert!(bank.wins(which, "fighter"), "typing it wins whichever is up");
            assert!(bank.wins(which, "The Fighter"), "however it is typed");
        }
        assert!(!bank.wins(0, "figter 2"), "and a sequel number is still exact or nothing");
    }

    #[test]
    fn a_sequel_number_is_exact_or_nothing() {
        let bank = fixture();
        let (don, don2) = (bank.find("don").expect("don"), bank.find("don2").expect("don 2"));
        assert!(bank.wins(don, "don") && bank.wins(don2, "don 2") && bank.wins(don2, "Don2"));
        assert!(!bank.wins(don2, "don"), "the first film is not the second");
        assert!(!bank.wins(don, "don 2"), "and the second is not the first");
        assert!(!bank.wins(don, "don 3"), "nor is one that doesn't exist");
    }

    #[test]
    fn a_film_is_picked_without_repeating_itself_and_leans_where_it_is_told() {
        let bank = fixture();
        let mut rng = Rng::seeded(7);
        // Everything but the Matrix has been up lately: that is what comes next.
        let used: HashSet<String> = ["dilvaledulhanialejaienge", "don", "don2", "mouse", "house", "heat"].iter().map(|k| k.to_string()).collect();
        for _ in 0..20 {
            assert_eq!(bank.pick(&used, 50, 50, &mut rng), bank.find("matrix"), "the only film not up lately");
        }
        // Once every film has been up the window starts over rather than the
        // channel sitting empty.
        let all: HashSet<String> = (0..bank.count()).map(|i| bank.movie(i).expect("a film").key()).collect();
        assert!(bank.pick(&all, 50, 50, &mut rng).is_some());
        // All Hindi and all English are both honoured when the bank has them.
        let fresh = HashSet::new();
        let industry = |hindi: u64, rng: &mut Rng| {
            let i = bank.pick(&fresh, hindi, 50, rng).expect("a film");
            bank.movie(i).expect("a film").industry
        };
        for _ in 0..30 {
            assert_eq!(industry(100, &mut rng), Industry::Bollywood);
            assert_eq!(industry(0, &mut rng), Industry::Hollywood);
        }
        assert_eq!(Bank::default().pick(&fresh, 50, 50, &mut rng), None);
    }

    #[test]
    fn a_clue_is_picked_by_kind_first_and_then_by_what_nobody_has_seen() {
        let bank = fixture();
        let mut rng = Rng::seeded(3);
        let ddlj = bank.find(&key("Dilwale Dulhania Le Jayenge")).expect("ddlj");
        // A film with two stills and one line still asks about the line: the
        // kind is chosen before the clue, so stills can't crowd it out.
        let mut kinds: HashSet<Clue> = HashSet::new();
        for _ in 0..60 {
            let (kind, _) = bank.pick_clue(ddlj, &HashSet::new(), &mut rng).expect("a clue");
            kinds.insert(kind);
        }
        assert_eq!(kinds.len(), 3, "all three kinds come up");
        // The still nobody has been shown is the one that goes up.
        let seen: HashSet<String> = HashSet::from([clue_key(Clue::Shot, 0)]);
        for _ in 0..20 {
            if let Some((Clue::Shot, which)) = bank.pick_clue(ddlj, &seen, &mut rng) {
                assert_eq!(which, 1, "the other still");
            }
        }
        // A film with nothing but tags plays tags; one with no still never
        // draws one.
        let mouse = bank.find("mouse").expect("mouse");
        assert_eq!(bank.movie(mouse).expect("a film").kinds(), vec![Clue::Tags]);
        for _ in 0..20 {
            assert_eq!(bank.pick_clue(mouse, &HashSet::new(), &mut rng), Some((Clue::Tags, 0)));
        }
        let don = bank.find("don").expect("don");
        assert!(!bank.movie(don).expect("a film").kinds().contains(&Clue::Shot));
    }

    #[test]
    fn the_card_shows_the_vague_tags_and_a_hint_reveals_the_next_one() {
        let bank = fixture();
        let ddlj = bank.movie(bank.find(&key("Dilwale Dulhania Le Jayenge")).expect("ddlj")).expect("a film");
        assert_eq!(ddlj.shown_tags(5), vec!["romance", "train", "europe", "mustard fields", "a hand out of a moving train"]);
        assert_eq!(ddlj.next_tag(5), Some("the last sharp one"), "the sharp one was held back");
        assert_eq!(ddlj.next_tag(6), None, "and there is nothing after it");
        // A round that showed no tags at all is hinted with the sharp end of
        // the list, not the vague one it would otherwise have started at.
        assert_eq!(ddlj.sharpest_tag(), Some("the last sharp one"));
        assert_ne!(ddlj.sharpest_tag(), ddlj.next_tag(0));
        assert_eq!(ddlj.first_letter(), 'D');
        // A film with four tags shows what it has and has nothing held back.
        let mouse = bank.movie(bank.find("mouse").expect("mouse")).expect("a film");
        assert_eq!(mouse.shown_tags(5).len(), 4);
        assert_eq!(mouse.next_tag(4), None);
    }

    #[test]
    fn a_still_is_either_one_of_ours_on_disk_or_one_of_tmdbs_by_link() {
        let bank = fixture();
        let heat = bank.movie(bank.find("heat").expect("heat")).expect("a film");
        let ours = heat.shot(0).expect("a still");
        assert!(ours.is_local() && ours.url().is_none(), "ours is a file, not a link");
        assert_eq!(ours.hint(), Some("A crew plans one last job."), "and it carries its own line");
        assert!(bank.shot_path(ours).is_some_and(|p| p.ends_with("images/h1.jpg")));
        let ddlj = bank.movie(bank.find("dilvaledulhanialejaienge").expect("ddlj")).expect("a film");
        let theirs = ddlj.shot(0).expect("a still");
        assert!(!theirs.is_local() && bank.shot_path(theirs).is_none(), "theirs is never on disk");
        assert_eq!(theirs.url().as_deref(), Some("https://image.tmdb.org/t/p/w1280/one.jpg"));
        assert_eq!(theirs.hint(), None, "and has no line of its own");
    }

    #[test]
    fn a_bank_that_doesnt_match_its_format_is_refused() {
        let good = r#"{"format": "MOVIEBANK1", "movies": [{"id": "x", "title": "Don", "year": 2006,
            "industry": "bollywood", "era": "modern", "answers": ["don"], "tags": ["crime", "a lookalike"]}]}"#;
        assert_eq!(Bank::from_json(good).expect("a bank").count(), 1);
        let later = r#"{"format": "MOVIEBANK9", "movies": []}"#;
        assert!(Bank::from_json(later).is_err(), "a format this reader doesn't know");
        assert!(Bank::from_json("{").is_err());
        // A film with no clues at all, or no answers, is left out rather than
        // put up as a round nobody could play.
        let empty = r#"{"format": "MOVIEBANK1", "movies": [{"id": "x", "title": "Don", "year": 2006,
            "industry": "bollywood", "era": "modern", "answers": ["don"]}]}"#;
        assert_eq!(Bank::from_json(empty).expect("a bank").count(), 0);
    }

    /// The pictures are big and the bank is small, so a workspace can easily
    /// end up with one and not the other. A still that isn't on disk has to go
    /// at load: otherwise the round asks "which film is this from?" over an
    /// empty space, and nobody can answer it.
    #[test]
    fn a_still_that_isnt_on_disk_is_dropped_rather_than_asked_about() {
        let dir = tempfile::tempdir().expect("a folder");
        std::fs::create_dir_all(dir.path().join("images")).expect("images");
        std::fs::write(dir.path().join("images/there.jpg"), [0xFF, 0xD8, 0xFF]).expect("a file");
        std::fs::write(
            dir.path().join("movies.json"),
            r#"{"format": "MOVIEBANK1", "movies": [
                {"id": "x", "title": "Heat", "year": 1995, "industry": "hollywood", "era": "iconic",
                 "answers": ["heat"], "tags": ["a thief", "a cop", "a diner", "a bank job"],
                 "shots": [{"file": "images/there.jpg", "note": "on disk"},
                           {"file": "images/gone.jpg", "note": "not on disk"},
                           {"path": "/tmdb.jpg", "note": "theirs, never on disk"}]}]}"#,
        )
        .expect("a bank");
        let bank = load(dir.path()).expect("a bank");
        let film = bank.movie(0).expect("a film");
        assert_eq!(film.shots.len(), 2, "the missing one went, the other two stayed");
        assert!(film.shots.iter().any(|s| s.file == "images/there.jpg"));
        assert!(film.shots.iter().any(|s| s.path == "/tmdb.jpg"), "a linked still is never looked for on disk");
        assert!(!film.shots.iter().any(|s| s.file == "images/gone.jpg"));
        // And the film still plays, because it has tags to ask with.
        assert!(film.kinds().contains(&Clue::Tags));
    }

    #[test]
    fn a_missing_film_bank_is_a_warning_and_not_a_crash() {
        let dir = tempfile::tempdir().expect("a folder");
        assert!(load(dir.path()).is_err(), "nothing there at all");
        std::fs::write(dir.path().join("movies.json"), r#"{"format": "MOVIEBANK1", "movies": []}"#).unwrap();
        assert!(load(dir.path()).is_err(), "a bank with no films in it is no bank");
        let empty = tempfile::tempdir().expect("a folder");
        open(&empty.path().display().to_string());
        assert!(bank().is_none());
    }

    #[test]
    fn a_voted_corner_is_a_rule_and_not_a_lean() {
        // Two of each corner, so a pick can only be right by honouring it.
        let json = r#"{"format": "MOVIEBANK1", "movies": [
            {"id": "hf1", "title": "Sholay", "year": 1975, "industry": "bollywood", "era": "iconic",
             "answers": ["sholay"], "tags": ["a water tank", "two friends", "a village", "a bandit"]},
            {"id": "hf2", "title": "Queen", "year": 2013, "industry": "bollywood", "era": "modern",
             "answers": ["queen"], "tags": ["a honeymoon alone", "paris", "amsterdam", "a called-off wedding"]},
            {"id": "ef1", "title": "Whiplash", "year": 2014, "industry": "hollywood", "era": "modern",
             "answers": ["whiplash"], "tags": ["a drummer", "a conservatory", "a thrown chair", "bleeding hands"]},
            {"id": "ef2", "title": "Alien", "year": 1979, "industry": "hollywood", "era": "iconic",
             "answers": ["alien"], "tags": ["a cargo ship", "an egg", "a cat", "a distress call"]},
            {"id": "hs1", "title": "Panchayat", "year": 2020, "industry": "bollywood", "era": "modern", "kind": "series",
             "answers": ["panchayat"], "tags": ["a village office", "a water tank", "an engineer", "a solar panel"]},
            {"id": "hs2", "title": "Kota Factory", "year": 2019, "industry": "bollywood", "era": "modern", "kind": "series",
             "answers": ["kota factory"], "tags": ["coaching classes", "black and white", "a hostel", "a physics teacher"]},
            {"id": "es1", "title": "Severance", "year": 2022, "industry": "hollywood", "era": "modern", "kind": "series",
             "answers": ["severance"], "tags": ["a lift", "two lives", "a green corridor", "a desk job"]},
            {"id": "es2", "title": "Fargo", "year": 2014, "industry": "hollywood", "era": "modern", "kind": "series",
             "answers": ["fargo"], "tags": ["snow", "a drifter", "a small town", "an insurance man"]}
        ]}"#;
        let bank = Bank::from_json(json).expect("a bank");
        let mut rng = Rng::seeded(11);
        // Every pool the sample can fill hands back only its own, whatever the
        // shares ask for - a hundred means "Hindi please" and nought means
        // "English please", and neither may override the corner.
        for pool in Pool::ALL {
            if pool == Pool::Mix || bank.pool_count(pool) == 0 {
                continue;
            }
            for shares in [(100, 100), (0, 0), (50, 65)] {
                for _ in 0..40 {
                    let which = bank.pick_in(pool, &HashSet::new(), shares.0, shares.1, &mut rng).expect("a film");
                    let film = bank.movie(which).expect("a film");
                    assert!(pool.holds(film), "{} came out of {}", film.title, pool.key());
                }
            }
        }
        // A corner the bank cannot fill widens rather than leaving the channel
        // with no round at all.
        let empty = Pool::ALL.into_iter().find(|p| bank.pool_count(*p) == 0);
        if let Some(empty) = empty {
            assert!(bank.pick_in(empty, &HashSet::new(), 50, 65, &mut rng).is_some(), "an empty corner left no film");
        }
    }

    /// The shipped bank when it is there - it is data, kept beside the code
    /// rather than in it, so this test passes quietly on a checkout without it.
    #[test]
    fn the_shipped_bank_plays_if_it_is_there() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("moviebank");
        let Ok(bank) = load(&dir) else { return };
        assert!(bank.count() >= 20, "only {} films", bank.count());
        // Television arrived in a pack of its own; a film says nothing about
        // its kind, so the default is what keeps the older entries right.
        if bank.series_count() > 0 {
            let series = bank.movies.iter().find(|m| m.kind == Kind::Series).expect("a series");
            assert!(!series.shots.is_empty(), "{} is a series with nothing to show", series.title);
            assert_eq!(series.kind.word(), "show");
            let films = bank.count() - bank.series_count();
            assert!(films > 0, "the bank went all television");
        }
        let mut rng = Rng::seeded(2026);
        let mut seen = HashSet::new();
        for _ in 0..200 {
            let which = bank.pick(&seen, 50, 60, &mut rng).expect("a film");
            let film = bank.movie(which).expect("a film");
            // Every film answers to its own title, and to each spelling it lists.
            for answer in &film.answers {
                assert!(bank.wins(which, answer), "{} doesn't answer to {:?}", film.title, answer);
            }
            assert!(!film.kinds().is_empty(), "{} has nothing to ask about", film.title);
            // Tagged films carry a proper set; a stills-only one carries none
            // yet and asks with its pictures until somebody writes them.
            assert!(film.tags.is_empty() || film.tags.len() >= 4, "{} has {} tags", film.title, film.tags.len());
            // No clue may name the film it is about.
            let title_words = words(&film.title);
            for tag in &film.tags {
                let leaked: Vec<&String> = title_words.iter().filter(|w| w.len() > 3 && words(tag).contains(w)).collect();
                assert!(leaked.is_empty(), "{}: tag {:?} gives it away", film.title, tag);
            }
            for line in film.dialogues.iter().chain(film.hints.iter()).chain(film.shots.iter().filter_map(Shot::hint).map(String::from).collect::<Vec<_>>().iter()) {
                let leaked: Vec<&String> =
                    title_words.iter().filter(|w| w.len() > 3 && !COMMON.contains(&w.as_str()) && words(line).contains(w)).collect();
                assert!(leaked.is_empty(), "{}: line {:?} gives it away", film.title, line);
            }
            // Every film can be asked at least two ways, so no round is ever
            // the same question twice over: two kinds of clue, or two stills,
            // which are two different pictures to ask with.
            assert!(film.kinds().len() >= 2 || film.shots.len() >= 2, "{} can only be asked one way", film.title);
            for shot in &film.shots {
                // Either one of ours on disk, or one of TMDB's linked by path.
                match shot.is_local() {
                    true => assert!(bank.shot_path(shot).is_some_and(|p| p.exists()), "{}: {} is not there", film.title, shot.file),
                    false => assert!(shot.path.starts_with('/') && shot.url().is_some_and(|u| u.starts_with(IMAGE_BASE))),
                }
                assert!(!shot.note.is_empty() || shot.hint().is_some(), "{}: a still with nothing said about it", film.title);
            }
            seen.insert(film.key());
        }
        // Both industries and both eras are really in there.
        let industries: HashSet<Industry> = (0..bank.count()).filter_map(|i| bank.movie(i)).map(|m| m.industry).collect();
        let eras: HashSet<Era> = (0..bank.count()).filter_map(|i| bank.movie(i)).map(|m| m.era).collect();
        assert_eq!(industries.len(), 2, "the bank leans entirely one way");
        assert_eq!(eras.len(), 2);
        // And the credit TMDB's terms ask for is the one we carry.
        assert!(ATTRIBUTION.contains("TMDB") && ATTRIBUTION.contains("not endorsed"));
    }
}
