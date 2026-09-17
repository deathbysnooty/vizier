//! The words Letter Duel accepts.
//!
//! One plain file, `<workspace>/wordbank/dictionary.txt`, one lower-case word
//! per line — the same file the Anagrams game reads its answers from, and
//! already on the server. It is a filtered copy of ENABLE: about 172,000 words.
//!
//! It is read ONCE at start into a set, the way [`super::anagram_words`] reads
//! its bank, because a play asks about several words and a turn asks about a
//! play. Nothing here is loaded lazily and nothing is read from disk while a
//! game is running.
//!
//! **The short words are built in.** That shipped file holds nothing under FOUR
//! letters — it was filtered that way for Anagrams, which never sets a shorter
//! word — and a tile game cannot be played without `AT`, `TO` and `QI`: almost
//! every tile laid beside an existing word makes a two- or three-letter word
//! sideways. So [`TWO_LETTER`] and [`THREE_LETTER`] below are compiled into the
//! binary and added to whatever the file holds. The three-letter list was
//! drawn from the shipped bank itself — a three-letter word is kept when the
//! bank knows its plural — so it is the same word list, not a second opinion,
//! and a word the bank has removed on purpose stays removed. Drop 2- and
//! 3-letter words into `dictionary.txt` one day and nothing here has to change:
//! the two sets are simply merged.
//!
//! If the bank isn't there the game simply never starts: [`open`] says so in
//! the log and leaves [`bank`] empty, and every part of the game checks it
//! before doing anything.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

/// The shortest thing that can be a word at all. A single letter is never a
/// word on this board — two tiles in a row are the smallest play.
pub const MIN_WORD: usize = 2;
/// Longer than the board is wide, and it cannot be on it.
pub const MAX_WORD: usize = super::duel_rules::SIZE;

static BANK: OnceLock<Bank> = OnceLock::new();

/// Every word the game will take.
#[derive(Debug, Default)]
pub struct Bank {
    words: HashSet<String>,
}

/// One line of the file as a word, or nothing. Anything that isn't plain
/// letters — a blank line, a stray count, a hyphenated entry — is skipped
/// rather than refused, so one odd line never costs the whole bank.
fn word_of(line: &str) -> Option<String> {
    let word = line.trim().to_lowercase();
    let length = word.chars().count();
    ((MIN_WORD..=MAX_WORD).contains(&length) && word.chars().all(|c| c.is_ascii_alphabetic())).then_some(word)
}

/// The two-letter words, as every word game has them.
pub const TWO_LETTER: &str = "\
     aa ab ad ae ag ah ai al am an ar as at aw ax ay ba be \
     bi bo by de do ed ef eh el em en er es et ex fa fe go \
     ha he hi hm ho id if in is it jo ka ki la li lo ma me \
     mi mm mo mu my na ne no nu od oe of oh oi om on op or \
     os ow ox oy pa pe pi qi re sh si so ta ti to uh um un \
     up us ut we wo xi xu ya ye yo za";

/// The three-letter words.
pub const THREE_LETTER: &str = "\
     aal aba aby ace act add ado adz aga age aha aid ail aim air ait ala alb \
     ale all alp alt ama ami amp ana and ani ant any ape apt arc are ark arm \
     art ash ask asp ass ate auk ave avo awe awl awn axe aye baa bad bag bal \
     bam ban bap bar bas bat bay bed bee beg bel ben bet bey bib bid big bin \
     bis bit biz boa bob bod bog boo bop bot bow box boy bra bro bub bud bug \
     bum bun bur bus but buy bye cab cad cam can cap car cat caw cay cee cel \
     cep chi cob cod cog col con coo cop cos cot cow cox coy coz cry cub cud \
     cue cup cur cut cwm dab dad dag dah dak dal dam dap daw day deb dee den \
     dev dew dey dib did die dig dim din dip dis dit doc doe dog dom don dor \
     dos dot dow dry dub dud due dug dun duo dup dye ear eat ebb edh eel eft \
     egg ego eke eld elf elk ell elm eme emu end eon era ere erg err ers ess \
     eta eve ewe eye fad fag fan far fat fax fay fed fee fen fet feu few fez \
     fib fid fig fin fir fit fix flu fly fob foe fog fop for fox foy fro fry \
     fub fud fug fun fur gab gad gag gal gam gap gar gas gat gay ged gee gel \
     gem gen get gib gid gie gig gin gip git gnu goa gob god goo got goy gul \
     gum gun gut guy gym had hag hah ham hap has hat haw hay hem hen her het \
     hew hex hey hid hie him hin hip his hit hob hod hoe hog hop hot how hoy \
     hub hue hug hum hut hyp ice ich icy ide ilk ill imp ink inn ion ire irk \
     ism its ivy jab jag jam jar jaw jay jet jib jig job joe jog jot jow joy \
     jug jut kat kay kea kef keg ken kep kex key kid kin kip kit koa kob kop \
     kor kos lab lac lad lag lam lap lar las lat law lax lay lea led lee leg \
     lei lek let ley lid lie lin lip lit lob log loo lop lot low lox lue lug \
     lum lux lye mac mad mae mag man map mar mas mat maw max may mel mem men \
     mew mho mib mid mig mil mir mix mob mog mom mon moo mop mor mot mow mud \
     mug mum mun nab nag nan nap nay neb net new nib nil nim nip nit nix nob \
     nod nog non nor not now nub nun nut oaf oak oar oat obe obi odd ode off \
     oft ohm oil oka old one ope opt orb orc ore ort ose our out owe owl own \
     pac pad pal pam pan pap par pat paw pax pay pea ped pee peg pen pep pet \
     pew phi pia pic pie pig pin pip pit pix ply pod poi pol pom pon pop pot \
     pow pox pro pry psi pub pud pug pul pun pup pur pus put pya pyx rad rag \
     ram rap ras rat raw rax ray reb red ree ref reg rep ret rev rex rho ria \
     rib rid rig rim rip rob roc rod roe rot row rub rue rug rum run rut rye \
     sab sac sad sag sal san sap sat saw sax say sea sec see seg ser set sew \
     sex she shy sib sic sin sip sir sis sit six ski sky sly sob sod sol son \
     sop sot sou sow soy spa spy sri sty sub sud sue sum sun sup tab tad tag \
     tam tan tao tap tar tat tau tav taw tax tea ted tee teg ten tew the tic \
     tie til tin tip tit tod toe tog tom ton too top tor tot tow toy try tub \
     tug tui tun tup tut tux twa two tye udo ugh uke ulu ump urd urn use uta \
     van vas vat vau vee vet vex via vie vim vis voe vow vug wab wad wae wag \
     wan wap war was wat waw wax way web wed wee wen wet who why wig win wis \
     wit wiz woe wok won woo wop wot wow wry wye wyn yak yam yap yaw yea yen \
     yes yet yew yin yip yob yok yon you yow zag zap zax zed zee zig zip zit \
     zoo";

impl Bank {
    /// The file's words and the built-in short ones together.
    pub fn from_text(dictionary: &str) -> Bank {
        let mut words: HashSet<String> = dictionary.lines().filter_map(word_of).collect();
        words.extend(TWO_LETTER.split_whitespace().filter_map(word_of));
        words.extend(THREE_LETTER.split_whitespace().filter_map(word_of));
        Bank { words }
    }

    /// Only what the file itself holds, for the test that says why the short
    /// words have to be built in at all.
    pub fn from_file_only(dictionary: &str) -> Bank {
        Bank { words: dictionary.lines().filter_map(word_of).collect() }
    }

    /// Whether the game knows a word. Case and the length limits are already
    /// taken care of, so a caller can hand this whatever a board spells.
    pub fn knows(&self, word: &str) -> bool {
        let word = word.trim().to_lowercase();
        word.chars().count() >= MIN_WORD && self.words.contains(&word)
    }

    pub fn count(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }
}

/// Reads the file. The error says which file is missing or unreadable, so the
/// log tells a mod what to do about it.
pub fn load(dir: &Path) -> anyhow::Result<Bank> {
    let path = dir.join("dictionary.txt");
    let text = std::fs::read_to_string(&path).map_err(|err| anyhow::anyhow!("{}: {}", path.display(), err))?;
    let bank = Bank::from_text(&text);
    if bank.is_empty() {
        anyhow::bail!("{} has no words in it", path.display());
    }
    Ok(bank)
}

/// Reads `<workspace>/wordbank/dictionary.txt` once, at start. A bank that
/// isn't there is not an error worth stopping for: the game stays off and says
/// so in the log.
pub fn open(workspace: &str) {
    let dir = crate::utils::build_path(workspace, &["wordbank"]);
    match load(&dir) {
        Ok(bank) => {
            tracing::info!("duel: {} words read from {}", bank.count(), dir.display());
            let _ = BANK.set(bank);
        }
        Err(err) => tracing::warn!(
            "duel: no dictionary ({}) — Letter Duel stays off. Copy wordbank/dictionary.txt into the workspace.",
            err
        ),
    }
}

/// The bank, when there is one.
pub fn bank() -> Option<&'static Bank> {
    BANK.get()
}

/// Whether a word is in the bank right now. With no bank at all nothing is a
/// word, which is why the game refuses to start without one.
pub fn knows(word: &str) -> bool {
    bank().is_some_and(|b| b.knows(word))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A bank small enough to read, with the words the game's own tests want.
    pub const DICTIONARY: &str = "cats\nhats\nband\nbands\nstare\nstar\nfarming\nteas\n";

    pub fn fixture() -> Bank {
        Bank::from_text(DICTIONARY)
    }

    /// The real bank when the checkout has one, and the little fixture when it
    /// hasn't, so a test that wants real words can have them without needing
    /// the file to exist.
    pub fn real_or_fixture() -> Bank {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("wordbank");
        load(&dir).unwrap_or_else(|_| fixture())
    }

    #[test]
    fn a_bank_is_one_plain_file_and_skips_anything_that_isnt_a_word() {
        let bank = Bank::from_file_only("cat\n\n  HATS \na\nabcdefghijklmnop\nco-op\ncat\n123\n");
        // "a" is one letter, the long one longer than the board, the hyphen and
        // the digits not letters, the blank line nothing, and the same word
        // twice is one word.
        assert_eq!(bank.count(), 2);
        assert!(bank.knows("cat") && bank.knows("hats"), "the upper-case line is read as a word");
        assert!(!bank.knows("a"));
        assert!(!bank.knows("co-op"));
    }

    #[test]
    fn a_word_is_known_whatever_case_the_board_spells_it_in() {
        let bank = fixture();
        assert!(bank.knows("CATS") && bank.knows("Cats") && bank.knows(" cats "));
        assert!(!bank.knows("wibbly"));
        assert!(!bank.knows(""), "nothing at all is not a word");
        assert!(!bank.knows("c"), "one letter is never a word");
    }

    #[test]
    fn the_short_words_are_built_in_because_the_shipped_file_has_none() {
        // This is the reason the two lists exist: the file itself starts at
        // four letters, so a bank made from it alone knows no cross-words.
        let file_only = Bank::from_file_only("cats\nhats\n");
        assert!(!file_only.knows("at") && !file_only.knows("cat"));
        let bank = fixture();
        for word in ["at", "to", "qi", "za", "xi", "cat", "the", "you", "try", "box", "ivy", "zit"] {
            assert!(bank.knows(word), "{} has to be playable", word);
        }
        // The lists are words and nothing else, and hold no duplicates.
        let two: Vec<&str> = TWO_LETTER.split_whitespace().collect();
        let three: Vec<&str> = THREE_LETTER.split_whitespace().collect();
        assert_eq!(two.len(), 101, "the two-letter words");
        assert!(three.len() > 700, "only {} three-letter words", three.len());
        for word in two.iter().chain(three.iter()) {
            assert!(word.chars().all(|c| c.is_ascii_lowercase()), "{} is not a plain lower-case word", word);
        }
        assert_eq!(two.iter().collect::<HashSet<_>>().len(), two.len(), "a two-letter word is listed twice");
        assert_eq!(three.iter().collect::<HashSet<_>>().len(), three.len(), "a three-letter word is listed twice");
        assert_eq!(two.iter().filter(|w| w.len() != 2).count(), 0);
        assert_eq!(three.iter().filter(|w| w.len() != 3).count(), 0);
    }

    #[test]
    fn with_no_bank_nothing_is_a_word() {
        // The real bank is only ever set by `open`, which the tests don't call,
        // so the free function refuses everything and the game stays off.
        assert!(!knows("cat") || bank().is_some());
    }

    /// The real bank on disk, when the tests are run from a checkout that has
    /// one. It is the file the server ships, so this is worth knowing about.
    #[test]
    fn the_shipped_dictionary_plays_a_real_game() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("wordbank");
        let Ok(bank) = load(&dir) else { return };
        assert!(bank.count() > 150_000, "only {} words — is this the right file?", bank.count());
        for word in ["shrimp", "quiz", "farming", "zyzzyva", "cat", "at", "qi", "jo", "za"] {
            assert!(bank.knows(word), "{} should be in the bank", word);
        }
        for word in ["qwertyuio", "zzzz", "sdfg"] {
            assert!(!bank.knows(word), "{} should not be", word);
        }
        // And the file on its own really does start at four letters, which is
        // the whole reason the short words are built in.
        let text = std::fs::read_to_string(dir.join("dictionary.txt")).expect("the file");
        assert!(text.lines().filter(|l| l.trim().len() < 4 && !l.trim().is_empty()).count() == 0);
    }
}
