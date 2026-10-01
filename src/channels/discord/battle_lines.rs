//! What the arena says, in one voice: Westeros, spoken Hinglish.
//!
//! There is one world and one set of lines. There used to be a picker that
//! chose between seven of them; the tourney has no styles to pick between, so
//! there is nothing here but the lists, the melee and the people in them.
//!
//! Every pool is filled: `{a}` is the one acting, `{b}` the other, and in
//! FINISH `{w}` is the winner and `{l}` the loser. No official artwork or
//! copied text is used anywhere - these are the server's own jokes wearing a
//! cloak.

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
    /// A melee round with an odd fighter out.
    pub bye: &'static [&'static str],
    /// Under the melee champion's name.
    pub champion: &'static [&'static str],
}

/// The only voice the arena has.
pub fn lines() -> &'static Lines {
    &WESTEROS
}

// Roasts stay on the fight itself: nothing about looks, family, caste or faith.

const EXCHANGE: &[&str] = &[
    "{a} ne {b} ko dhaal pe aisa maara ki poore lists mein goonj gaya 🛡️",
    "{b} ne {a} ka helm tedha kar diya, ab kuch dikh hi nahi raha",
    "{a} ne {b} ko kehni maari aur bola 'yeh Winterfell ka tareeka hai'",
    "{b} ne {a} ki talwaar pe talwaar rakh di, chingari ud gayi ⚔️",
    "{a} ne {b} ko ghode se utaar diya, bina kuch bole",
    "{b} ne {a} ke paanv pe apna lohe wala boot rakh diya",
    "{a} ne {b} ka cloak kheench liya, saari hawa nikal gayi",
    "{b} ne {a} ko lists ke railing tak dhakel diya",
    "{a} ne {b} ke kandhe pe lance ka sira laga diya",
    "{b} ne {a} ko 'tu to Wall pe hi theek tha' bol diya",
    "{a} ne {b} ki dhaal ka crest ragad ke mita diya",
    "{b} ne {a} ko peeche se maara, maester bhi chup rahe",
    "{a} ne {b} ko reth mein ghasit diya, pura maidan dekh raha tha",
    "{b} ne {a} ka gauntlet nikal ke door feink diya",
    "{a} ne {b} ko ek hi vaar mein ghutne tak jhuka diya",
    "{b} ne {a} ki belt kaat di, armour latak gaya",
    "{a} ne {b} ko 'yeh tera naam gaane mein nahi aayega' bol diya 🎵",
    "{b} ne {a} ke helm pe itna thoka ki ghanti baj gayi",
];

const FINISH: &[&str] = &[
    "{w} jeeta. {l} ne yield bol diya 🏆",
    "{l} reth mein pada hai — {w} ne haath tak nahi jhaada",
    "{w} ki jeet, aur {l} ke paas bahaana pehle se taiyaar hai",
    "{l} ne natak karke exit liya, {w} ne haath hila diya",
    "{w} khada hai. {l} ab stands se dekhega",
    "Ghanti baj gayi {l} ke liye. {w} ne talwaar hawa mein ghumaayi",
    "{w} ne aakhri vaar kiya: bilkul chuppi. {l} khatam",
    "{l} ne haar maan li, {w} ne apna chalice wapas utha liya 🍷",
    "{w} jeet gaya. {l} ab commentary karega",
    "{w} ne {l} ko lists ka darwaaza dikha diya 🚪",
];

const CRIT: &[&str] = &[
    "{a} ne {b} pe aisa combo chalaya ki armour hi bikhar gaya 💥",
    "{a} ka Valyrian steel wala vaar — {b} ki dhaal do tukde 🗡️",
    "{a} ne {b} ko ek hi taane mein ghode se giraa diya",
    "{a} ne {b} ka visor khol ke seedha aankh mein dekh liya 😳",
    "{a} ne {b} ko uske hi gharaane ke naare se maara",
];

const MISS: &[&str] = &[
    "{a} ne lance ghumaayi... aur hawa mein hi reh gaya",
    "{a} ka vaar miss, {b} ne jhuk ke bacha liya",
    "{a} ko laga yeh landega — reth ke alawa kuch nahi laga",
];

const HEAL: &[&str] = &[
    "{a} ne maester ka kaadha pee liya, thodi jaan aayi 🧪",
    "{a} ne visor uthaya aur lambi saans li, dum wapas",
    "{a} ne Seven ke aage sar jhuka diya, thodi power aayi 🕯️",
];

/// A sip, a bite, a deep breath: barely anything, purely for the joke.
const SIP: &[&str] = &[
    "{a} ne ek ghoont sharaab maari. Bas itna hi 🍷",
    "{a} ko kahin se lemon cake mil gaya 🍰",
    "{a} ne belt kas li aur collar theek kiya",
    "{a} ne apna crest chamka diya, confidence +1",
];

/// The swing comes back at whoever threw it.
const BACKFIRE: &[&str] = &[
    "{a} ne gauntlet feka, wapas aake khud ke helm pe lagi 🪃",
    "{a} apni hi cloak mein ulajh ke gir gaya",
    "{a} ne lance uthaayi, apne hi ghode ko dara diya",
    "{a} ne naara lagaaya aur apni hi awaaz se dar gaya",
];

/// {b} loses, {a} gains: the fight's only real comeback move.
const DRAIN: &[&str] = &[
    "{a} ne {b} ki plate se roast le liya 🍗",
    "{a} ne {b} ki dhaal chheen li — ab {a} bacha hai, {b} khula 🛡️",
    "{a} ne {b} ka chalice khaali kar diya, seedha energy transfer 🍷",
    "{a} ne {b} ke ghode ki lagaam pakad li",
];

/// Two in a row, before anyone can answer.
const DOUBLE: &[&str] = &[
    "{a} ne do vaar kiye — ek dhaal pe, ek izzat pe ⚔️⚔️",
    "{a} ne back to back do baar ghode se utaara",
    "{a} ne {b} ko lists mein aur stands ke saamne, dono jagah suna diya",
    "{a} ne double lance combo laga diya",
];

/// Everyone in the frame suffers.
const CHAOS: &[&str] = &[
    "Septa beech mein aa gayi, dono ko daant padi 🕯️",
    "Mashaal bujh gayi — dono andhere mein gir gaye 🔥",
    "Dono ek hi geeli reth pe phisal gaye",
    "Kisi ne dono ki shikayat Lord ke paas kar di 📜",
];

/// A proper feed: the biggest ordinary heal.
const SNACK: &[&str] = &[
    "{a} ne garam pie khaayi, shakti aa gayi 🥧",
    "{a} ne roast ka pura dabba khol liya, ab mood set hai 🍖",
    "{a} ne shahad wala doodh gatak liya 🍯",
    "{a} ne thandi beer ek saans mein khatam ki 🍺",
    "{a} ne stew ka ek aur katora maanga 🥣",
];

/// Rare, and worth it.
const BLESSING: &[&str] = &[
    "{a} ke gharaane ne naam pukaar diya — poori jaan wapas 🛡️",
    "{a} ko kisi ne 'jeete raho' bol diya, full power up ✨",
    "{a} ne weirwood ke saamne sar jhuka liya, ab kaun rokega 🌲",
    "{a} ke Lord ne kandha thapthapaya — motivation overload",
];

/// Both sides gain: the fight pauses for something nicer.
const CROWD: &[&str] = &[
    "Stands ne dono ke liye taali bajaayi, dono ka mann bhar aaya 📣",
    "Kisi ne dono ko sharaab pila di, ladai thodi der ke liye band 🍷",
    "Dono ne ek hi thaali se kha liya, dosti ho gayi 🍽️",
    "Septa ne dono ko lemon cake pakda diya 🍰",
];

const BYE: &[&str] = &[
    "{a} ka is round mein koi saamna hi nahi tha, seedha pavilion chala gaya ⛺",
    "{a} ko agle round ka free pass mil gaya",
    "{a} ka challenger aaya hi nahi — walkover",
];

const CHAMPION_LINE: &[&str] = &[
    "Poore lists ko haraya aur taaj utha liya 👑",
    "Aakhri tak khada raha, taaj ke saath",
    "Aaj ka champion. Baaki sab stands mein hain",
    "Bina shak. Baaki agle melee ka intezaar karein",
];

const WESTEROS: Lines = Lines {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pools(l: &'static Lines) -> Vec<(&'static str, &'static [&'static str])> {
        vec![
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
    fn every_pool_is_filled() {
        for (name, pool) in pools(lines()) {
            assert!(!pool.is_empty(), "no {} lines", name);
            for line in pool {
                assert!(!line.trim().is_empty(), "{} has a blank line", name);
            }
        }
    }

    #[test]
    fn every_line_has_its_names_and_fits() {
        let l = lines();
        for (name, pool) in [("exchange", l.exchange), ("drain", l.drain)] {
            for line in pool {
                assert!(line.contains("{a}") && line.contains("{b}"), "{}: {}", name, line);
            }
        }
        for line in l.finish {
            assert!(line.contains("{w}") && line.contains("{l}"), "finish: {}", line);
        }
        for line in l.bye {
            assert!(line.contains("{a}"), "bye: {}", line);
        }
        for line in l.champion {
            assert!(!line.contains('{'), "champion has a placeholder: {}", line);
            assert!(line.chars().count() <= 60, "champion too long: {}", line);
        }
        for (name, pool) in pools(l) {
            for line in pool {
                assert!(line.chars().count() <= 110, "{} too long: {}", name, line);
            }
        }
    }

    /// The styles are gone: nothing the arena says may name one, or offer a
    /// pick between them. A line that mentions a "type" or a "style" would be
    /// the picker growing back in the copy.
    #[test]
    fn nothing_the_arena_says_names_a_style() {
        let banned = [
            "pokemon", "pokémon", "wizard", "hogwarts", "saiyan", "dragon ball", "wwe", "wrestling",
            "counter-strike", "cs2", "elden", "tarnished", "classic", "fight style", "fight type",
            "theme", " type", "types",
        ];
        for (name, pool) in pools(lines()) {
            for line in pool {
                let lower = line.to_lowercase();
                for word in banned {
                    assert!(!lower.contains(word), "{} line names a style ({}): {}", name, word, line);
                }
            }
        }
    }
}
