//! Fight themes for /fight and /battle: the lines each theme speaks and its key.
//!
//! Classic is the original Hinglish banter. The named themes speak English and
//! only borrow their world's moves and jokes; no official artwork is used anywhere.
//! Every theme fills every pool: `{a}` is the one acting, `{b}` the other, and in
//! FINISH `{w}` is the winner and `{l}` the loser.

/// Which world a fight is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Theme {
    #[default]
    Classic,
    Pokemon,
    Wizard,
    Saiyan,
    Wrestling,
}

impl Theme {
    pub const ALL: [Theme; 5] = [Theme::Classic, Theme::Pokemon, Theme::Wizard, Theme::Saiyan, Theme::Wrestling];

    /// The value stored in the command choice.
    pub fn key(self) -> &'static str {
        match self {
            Theme::Classic => "classic",
            Theme::Pokemon => "pokemon",
            Theme::Wizard => "harrypotter",
            Theme::Saiyan => "dbz",
            Theme::Wrestling => "wwe",
        }
    }

    pub fn from_key(key: &str) -> Option<Theme> {
        let key = key.trim().to_ascii_lowercase();
        Theme::ALL.into_iter().find(|t| t.key() == key)
    }

    /// The name members see in the command menu.
    pub fn label(self) -> &'static str {
        match self {
            Theme::Classic => "Classic",
            Theme::Pokemon => "Pokémon",
            Theme::Wizard => "Harry Potter",
            Theme::Saiyan => "Dragon Ball Z",
            Theme::Wrestling => "WWE",
        }
    }

    pub fn lines(self) -> &'static Lines {
        match self {
            Theme::Classic => &CLASSIC,
            Theme::Pokemon => &POKEMON,
            Theme::Wizard => &WIZARD,
            Theme::Saiyan => &SAIYAN,
            Theme::Wrestling => &WRESTLING,
        }
    }
}

/// Every line a fight can say, by what happened.
pub struct Lines {
    /// A plain hit.
    pub exchange: &'static [&'static str],
    pub crit: &'static [&'static str],
    pub miss: &'static [&'static str],
    pub heal: &'static [&'static str],
    /// Barely anything, purely for the joke.
    pub sip: &'static [&'static str],
    /// The swing comes back at whoever threw it.
    pub backfire: &'static [&'static str],
    /// {b} loses, {a} gains.
    pub drain: &'static [&'static str],
    /// Two in a row.
    pub double: &'static [&'static str],
    /// Both sides get hurt.
    pub chaos: &'static [&'static str],
    /// The biggest ordinary heal.
    pub snack: &'static [&'static str],
    /// Rare, a big heal.
    pub blessing: &'static [&'static str],
    /// Both sides heal.
    pub crowd: &'static [&'static str],
    /// The end of a fight: {w} won, {l} lost.
    pub finish: &'static [&'static str],
    /// A royale round with an odd fighter out.
    pub bye: &'static [&'static str],
    /// Under the royale champion's name.
    pub champion: &'static [&'static str],
}

// --- classic ------------------------------------------------------------------
// Roasts stay on the fight itself: nothing about looks, family, caste or faith.

// Roasts stay on the fight itself: nothing about looks, family, caste or faith.

const EXCHANGE: &[&str] = &[
    "{a} ne {b} ko chappal dikhayi 🩴",
    "{b} ne {a} ki DP screenshot kar li, blackmail ki taiyari",
    "{a} ne {b} ko ek dum se 'bhai sun' bola aur block kar diya",
    "{b} ne {a} pe pura jug paani daal diya",
    "{a} ne {b} ki keyboard ke keycaps nikaal diye",
    "{b} ne {a} ko VC mein ghaseet liya, mic mute karke",
    "{a} ne {b} ko 'seen' kar diya, reply nahi bheja",
    "{b} ne {a} ka WiFi router unplug kar diya",
    "{a} ne {b} pe tagda meme daga",
    "{b} ne {a} ko typing... typing... pe 10 minute rakha",
    "{a} ne {b} ki maggi bina namak ke bana di",
    "{b} ne {a} ko 'tera match to Jio pe hi atka hai' bola",
    "{a} ne {b} ka phone 1% battery pe chhod diya",
    "{b} ne {a} ki chai mein cheeni double kar di",
    "{a} ne {b} ko group se remove karke wapas add kiya, sirf dikhane ke liye",
    "{b} ne {a} ka last seen chhupa diya",
    "{a} ne {b} ko ludo mein teen baar chhakka maar ke hara diya",
    "{b} ne {a} ki playlist mein sirf sad songs bhar diye",
    "{a} ne {b} ka chair khinch liya, classic",
    "{b} ne {a} ko 'aur bata' bolke mool baat hi nahi batayi",
];

const FINISH: &[&str] = &[
    "{w} won. {l} is out 🏆",
    "{l} is down — {w} took it without breaking a sweat",
    "{w} wins, and {l} already has an excuse ready",
    "{l} made a dramatic exit, {w} waved them off",
    "{w} survives. {l} is out, and yes, someone screenshotted it",
    "Game over for {l}. {w} threw in a victory dance",
    "{w} landed the finisher: the silent treatment. {l} is done",
    "{l} gave up, {w} picked the chai back up",
    "{w} won. {l} moves to commentary",
    "{w} showed {l} the exit 🚪",
];

const CRIT: &[&str] = &[
    "{a} ne {b} pe poora combo chala diya, bina saans liye 💥",
    "{a} ka jhakaas headshot - {b} ka WiFi tak hil gaya",
    "{a} ne {b} ko ek hi taane mein udaa diya",
    "{a} ne {b} ki puri chat history nikal ke padh di 😳",
    "{a} ne {b} ko uske hi meme se maara",
];

const MISS: &[&str] = &[
    "{a} ne haath ghumaya... aur hawa mein reh gaya",
    "{a} ka taana miss, {b} ne duck kar liya",
    "{a} laga raha tha ki ye landega - nahi landa",
];

const HEAL: &[&str] = &[
    "{a} ne chai ka ghoont liya, thodi jaan wapas aayi ☕",
    "{a} ne maggi khaayi aur fresh ho gaya 🍜",
    "{a} ne Hanuman Chalisa laga di, thodi power aayi",
];

/// A sip, a biscuit, a deep breath: barely anything, purely for the joke.
const SIP: &[&str] = &[
    "{a} ne ek ghoont chai maari. Bas itna hi. ☕",
    "{a} ko kahin se ek Parle-G mil gaya 🍪",
    "{a} ne lamba saans liya aur collar theek kiya",
    "{a} ne apni DP badal di, confidence +1",
];

/// The swing comes back at whoever threw it.
const BACKFIRE: &[&str] = &[
    "{a} ne chappal feki, wapas aake khud ko lagi 🩴",
    "{a} apne hi jokes pe has ke gir gaya",
    "{a} ne block karne ki koshish ki, khud ko block kar liya",
    "{a} ka screenshot ulta pad gaya, apni hi baat pakdi gayi",
];

/// {b} loses, {a} gains: the fight's only real comeback move.
const DRAIN: &[&str] = &[
    "{a} ne {b} ki plate se samosa utha liya 🥟",
    "{a} ne {b} ka charger le liya - ab {a} full, {b} khali 🔌",
    "{a} ne {b} ki chai pi li, seedha energy transfer",
    "{a} ne {b} ka WiFi password chura liya",
];

/// Two in a row, before anyone can answer.
const DOUBLE: &[&str] = &[
    "{a} ne do baar maara - ek taana, ek screenshot 📸",
    "{a} ne back to back do meme daag diye",
    "{a} ne {b} ko group aur DM, dono mein sunaya",
    "{a} ne double chappal combo lagaya 🩴🩴",
];

/// Everyone in the frame suffers.
const CHAOS: &[&str] = &[
    "Beech mein aunty aa gayi, dono ko daant padi 👵",
    "Light chali gayi - dono andhere mein gir gaye 💡",
    "Dono ek hi kele ke chhilke pe phisal gaye 🍌",
    "Kisi ne dono ka naam mummy ko bata diya 😰",
];

/// A proper feed: the biggest ordinary heal.
const SNACK: &[&str] = &[
    "{a} ne garam samosa khaaya, shakti aa gayi 🥟",
    "{a} ne biryani ka dabba khol liya, ab mood set hai 🍛",
    "{a} ne do minute mein Maggi bana li 🍜",
    "{a} ne thanda Rooh Afza gatak liya 🥤",
    "{a} ne pani puri ka ek aur round maanga 🥣",
];

/// Rare, and worth it.
const BLESSING: &[&str] = &[
    "{a} ki mummy ne sar pe haath rakh diya - poori jaan wapas 🙏",
    "{a} ko kisi ne 'jeete raho beta' bol diya, full power up ✨",
    "{a} ne prasad kha liya, ab kaun rok sakta hai 🪔",
    "{a} ke papa ne kandha thapthapaya - motivation overload",
];

/// Both sides gain: the fight pauses for something nicer.
const CROWD: &[&str] = &[
    "Crowd ne dono ko cheer kar diya, dono ka mann bhar aaya 📣",
    "Kisi ne dono ko chai pila di, ladai thodi der ke liye band ☕",
    "Dono ne ek hi thali se kha liya, dosti ho gayi 🍽️",
    "Aunty ne dono ko laddoo pakda diya 🍬",
];

const BYE: &[&str] = &[
    "{a} had nobody to fight this round and went for chai ☕",
    "{a} gets a free pass to the next round",
    "{a}'s opponent never turned up — walkover",
];

const CHAMPION_LINE: &[&str] = &[
    "Took on the whole server and won 👑",
    "Last one standing, crown and all",
    "Today's champion. Everyone else is on commentary",
    "Undisputed. The rest can wait for the next battle",
];


const CLASSIC: Lines = Lines {
    exchange: EXCHANGE,
    crit: CRIT,
    miss: MISS,
    heal: HEAL,
    sip: SIP,
    backfire: BACKFIRE,
    drain: DRAIN,
    double: DOUBLE,
    chaos: CHAOS,
    snack: SNACK,
    blessing: BLESSING,
    crowd: CROWD,
    finish: FINISH,
    bye: BYE,
    champion: CHAMPION_LINE,
};

// --- themed (placeholders until written: they reuse classic) -------------------

const POKEMON: Lines = CLASSIC;
const WIZARD: Lines = CLASSIC;
const SAIYAN: Lines = CLASSIC;
const WRESTLING: Lines = CLASSIC;

#[cfg(test)]
mod tests {
    use super::*;

    fn pools(l: &Lines) -> [(&'static str, &'static [&'static str]); 15] {
        [
            ("exchange", l.exchange),
            ("crit", l.crit),
            ("miss", l.miss),
            ("heal", l.heal),
            ("sip", l.sip),
            ("backfire", l.backfire),
            ("drain", l.drain),
            ("double", l.double),
            ("chaos", l.chaos),
            ("snack", l.snack),
            ("blessing", l.blessing),
            ("crowd", l.crowd),
            ("finish", l.finish),
            ("bye", l.bye),
            ("champion", l.champion),
        ]
    }

    #[test]
    fn every_theme_fills_every_pool() {
        for theme in Theme::ALL {
            for (name, pool) in pools(theme.lines()) {
                assert!(!pool.is_empty(), "{:?} has no {} lines", theme, name);
                for line in pool {
                    assert!(!line.trim().is_empty(), "{:?} {} has a blank line", theme, name);
                }
            }
        }
    }

    #[test]
    fn keys_round_trip() {
        for theme in Theme::ALL {
            assert_eq!(Theme::from_key(theme.key()), Some(theme));
        }
        assert_eq!(Theme::from_key(" DBZ "), Some(Theme::Saiyan));
        assert_eq!(Theme::from_key("naruto"), None);
    }
}
