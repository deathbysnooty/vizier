//! What the arena says, in one voice: Westeros in plain English.
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
    "{a} hit {b}'s shield so hard it rang across the arena 🛡️",
    "{b} knocked {a}'s helm sideways, blocking their view",
    "{a} elbowed {b} and said, 'That's how we do it in Winterfell'",
    "{b}'s sword met {a}'s, and sparks flew ⚔️",
    "{a} knocked {b} off the horse without a word",
    "{b} planted an iron boot on {a}'s foot",
    "{a} yanked {b}'s cloak and left them gasping",
    "{b} drove {a} back against the arena rail",
    "{a} clipped {b}'s shoulder with the tip of a lance",
    "{b} told {a}, 'You should've stayed at the Wall'",
    "{a} scraped the crest right off {b}'s shield",
    "{b} struck {a} from behind. Even the maester stayed quiet",
    "{a} dragged {b} through the sand with the whole arena watching",
    "{b} pulled off {a}'s gauntlet and flung it away",
    "{a} brought {b} to one knee with a single blow",
    "{b} cut {a}'s belt, and the armour hung loose",
    "{a} told {b}, 'The bards won't be singing your name' 🎵",
    "{b} struck {a}'s helm so hard it rang like a bell",
];

const FINISH: &[&str] = &[
    "{w} wins. {l} yields 🏆",
    "{l} is down in the sand. {w} barely broke a sweat",
    "{w} wins, and {l} already has an excuse ready",
    "{l} made a dramatic exit. {w} waved goodbye",
    "{w} is still standing. {l} can watch from the stands",
    "The bell rang for {l}. {w} swung a sword in the air",
    "{w} landed the final blow. The arena went quiet. {l} was done",
    "{l} admitted defeat. {w} picked up the chalice again 🍷",
    "{w} wins. {l} can do the commentary now",
    "{w} showed {l} the arena door 🚪",
];

const CRIT: &[&str] = &[
    "{a} landed a combo that tore {b}'s armour apart 💥",
    "{a}'s Valyrian steel split {b}'s shield in two 🗡️",
    "{a} knocked {b} off the horse with a single taunt",
    "{a} flipped up {b}'s visor and looked them right in the eye 😳",
    "{a} hit {b} with their own house motto",
];

const MISS: &[&str] = &[
    "{a} swung a lance... and hit thin air",
    "{a} missed as {b} ducked",
    "{a} thought that would land. It only hit sand",
];

const HEAL: &[&str] = &[
    "{a} drank the maester's brew and got some strength back 🧪",
    "{a} lifted the visor, took a deep breath and got a second wind",
    "{a} bowed to the Seven and found a little more strength 🕯️",
];

/// A sip, a bite, a deep breath: barely anything, purely for the joke.
const SIP: &[&str] = &[
    "{a} took one sip of wine. That's it 🍷",
    "{a} found a lemon cake somewhere 🍰",
    "{a} tightened their belt and straightened their collar",
    "{a} polished the crest, confidence +1",
];

/// The swing comes back at whoever threw it.
const BACKFIRE: &[&str] = &[
    "{a} threw a gauntlet. It bounced back and hit their helm 🪃",
    "{a} tripped over their own cloak",
    "{a} raised a lance and scared their own horse",
    "{a} shouted a battle cry and startled themself",
];

/// {b} loses, {a} gains: the fight's only real comeback move.
const DRAIN: &[&str] = &[
    "{a} stole the roast off {b}'s plate 🍗",
    "{a} took {b}'s shield and left them wide open 🛡️",
    "{a} drained {b}'s chalice for a quick energy boost 🍷",
    "{a} grabbed the reins of {b}'s horse",
];

/// Two in a row, before anyone can answer.
const DOUBLE: &[&str] = &[
    "{a} struck twice — once at a shield, once at an opponent's pride ⚔️⚔️",
    "{a} knocked their opponent off the horse twice in a row",
    "{a} roasted {b} twice, once in the ring and once for the crowd",
    "{a} landed a double lance combo",
];

/// Everyone in the frame suffers.
const CHAOS: &[&str] = &[
    "A septa stepped in and scolded them both 🕯️",
    "The torch went out — both fell in the dark 🔥",
    "Both slipped on the same patch of wet sand",
    "Someone reported them both to the lord 📜",
];

/// A proper feed: the biggest ordinary heal.
const SNACK: &[&str] = &[
    "{a} ate a hot pie and got their strength back 🥧",
    "{a} opened a whole box of roast meat. That's more like it 🍖",
    "{a} gulped down some milk with honey 🍯",
    "{a} finished a cold beer in one gulp 🍺",
    "{a} asked for another bowl of stew 🥣",
];

/// Rare, and worth it.
const BLESSING: &[&str] = &[
    "{a}'s house chanted their name — full strength restored 🛡️",
    "Someone blessed {a} with a long life. Full power restored ✨",
    "{a} bowed before a weirwood. Who can stop them now? 🌲",
    "{a}'s lord gave them a pat on the shoulder. Instant morale boost",
];

/// Both sides gain: the fight pauses for something nicer.
const CROWD: &[&str] = &[
    "The crowd cheered for both fighters. They were both touched 📣",
    "Someone poured them both wine. The fight can wait 🍷",
    "They shared a plate of food and made friends 🍽️",
    "A septa handed them both lemon cake 🍰",
];

const BYE: &[&str] = &[
    "{a} had no opponent this round and went straight to the pavilion ⛺",
    "{a} got a free pass to the next round",
    "{a}'s challenger never showed up — walkover",
];

const CHAMPION_LINE: &[&str] = &[
    "Beat the whole field and claimed the crown 👑",
    "Still standing at the end, crown in hand",
    "Today's champion. Everyone else is in the stands",
    "A clear win. Everyone else can wait for the next melee",
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
