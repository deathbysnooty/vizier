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
    /// Counter-Strike 2.
    Tactical,
    /// Elden Ring.
    Tarnished,
}

impl Theme {
    pub const ALL: [Theme; 7] = [
        Theme::Classic,
        Theme::Pokemon,
        Theme::Wizard,
        Theme::Saiyan,
        Theme::Wrestling,
        Theme::Tactical,
        Theme::Tarnished,
    ];

    /// The value stored in the command choice.
    pub fn key(self) -> &'static str {
        match self {
            Theme::Classic => "classic",
            Theme::Pokemon => "pokemon",
            Theme::Wizard => "harrypotter",
            Theme::Saiyan => "dbz",
            Theme::Wrestling => "wwe",
            Theme::Tactical => "cs2",
            Theme::Tarnished => "eldenring",
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
            Theme::Tactical => "Counter-Strike 2",
            Theme::Tarnished => "Elden Ring",
        }
    }

    pub fn lines(self) -> &'static Lines {
        match self {
            Theme::Classic => &CLASSIC,
            Theme::Pokemon => &POKEMON,
            Theme::Wizard => &WIZARD,
            Theme::Saiyan => &SAIYAN,
            Theme::Wrestling => &WRESTLING,
            Theme::Tactical => &TACTICAL,
            Theme::Tarnished => &TARNISHED,
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

// --- pokemon ------------------------------------------------------------------
// Moves, berries, Nurse Joy and Team Rocket. English, and only about the battle.

const POKEMON_EXCHANGE: &[&str] = &[
    "{a} used Tackle! {b} definitely felt that one",
    "{a} used Quick Attack and {b} never saw it coming ⚡",
    "{b} took a Water Gun straight to the face from {a} 💦",
    "{a} used Scratch. {b} is filing a complaint",
    "{a} threw a Razor Leaf at {b}. It's super effective! 🍃",
    "{b} got hit by {a}'s Ember and is now lightly toasted 🔥",
    "{a} used Headbutt. {b} is seeing Pidgeys",
    "{a} used Vine Whip on {b}, like an angry garden hose",
    "{b} walked right into {a}'s Thunder Shock ⚡",
    "{a} used Bite! {b} did not agree to that",
    "A wild {a} appeared and hit {b} with Pound",
    "{a} used Rock Throw. {b} caught it the hard way 🪨",
    "{a} used Double Slap on {b}. Only one slap landed",
    "{b} got hit by {a}'s Gust and lost their trainer cap 🧢",
    "{a} used Peck on {b}, over and over, very rudely",
    "Karate Chop from {a}! {b} is not happy about it",
    "{a} used Confusion. {b} forgot which way the fight is 🌀",
    "{b} got sprayed by {a}'s Bubble. Tiny, but it stings 🫧",
    "{a} used Mud-Slap. {b} now needs a bath",
    "{a} used Leer, then just bonked {b} anyway",
];

const POKEMON_CRIT: &[&str] = &[
    "{a} used Hyper Beam! {b} is recharging their dignity 💥",
    "Critical hit! {a}'s Thunderbolt lit {b} up ⚡",
    "{a} used Earthquake. {b} and half the gym felt it",
    "It's super effective! {a}'s Flamethrower toasted {b} 🔥",
    "{a} used Mega Punch. {b} flew all the way back to Pallet Town",
    "{a} used Body Slam and landed on {b} like a sleepy Snorlax",
];

const POKEMON_MISS: &[&str] = &[
    "{a} used Splash. But nothing happened 🐟",
    "{a}'s attack missed! {b} just stepped to the left",
    "{a} used Hypnosis. {b} stayed wide awake",
    "{a} threw a Poké Ball at {b}. You can't catch a trainer",
];

const POKEMON_HEAL: &[&str] = &[
    "{a} drank a Potion. Tastes like grape, heals a little 🧪",
    "{a} munched an Oran Berry 🫐",
    "{a} used Rest and napped for exactly one turn 💤",
    "{a} used Recover and patched up a few HP",
];

/// Barely anything, purely for the joke.
const POKEMON_SIP: &[&str] = &[
    "{a} used Growl. Morale went up, HP barely did",
    "{a} found half a berry on the route floor and ate it anyway",
    "{a} used Splash to stay hydrated. +1 HP 💧",
    "{a} checked the Pokédex for tips. Very little help",
];

/// The move comes back at whoever used it.
const POKEMON_BACKFIRE: &[&str] = &[
    "{a} used Self-Destruct. Bold choice 💥",
    "{a} is confused and hit themselves in confusion 🌀",
    "{a} used Take Down and took the recoil damage",
    "{a}'s Poké Ball bounced off the wall and bonked {a}",
];

/// {b} loses, {a} gains.
const POKEMON_DRAIN: &[&str] = &[
    "{a} used Absorb and sipped some HP right out of {b} 🌿",
    "{a} used Leech Seed. {b} is now paying for {a}'s health",
    "{a} used Giga Drain. {b} feels lighter, {a} feels great",
    "{a} used Thief and ate {b}'s Sitrus Berry 🍋",
];

/// Two in a row, before anyone can answer.
const POKEMON_DOUBLE: &[&str] = &[
    "{a} used Double Kick! {b} got booted twice",
    "{a} used Fury Swipes on {b}. Hit 2 times!",
    "{a} used Bonemerang. It hit {b} going and coming back 🦴",
    "{a} used Double Team, and both copies hit {b}",
];

/// Everyone in the frame suffers.
const POKEMON_CHAOS: &[&str] = &[
    "Team Rocket crashed the battle and everyone blasted off 🚀",
    "A wild Snorlax rolled over onto both trainers 💤",
    "{a} and {b} both stepped on a Voltorb. It used Explosion",
    "Somebody used Surf indoors. Everyone is soaked 🌊",
];

/// The biggest ordinary heal.
const POKEMON_SNACK: &[&str] = &[
    "{a} drank a Super Potion. Much better 🧪",
    "{a} ate a Sitrus Berry in one bite 🍋",
    "{a} gulped down a bottle of Moomoo Milk 🥛",
    "{a} used Milk Drink and feels refreshed",
    "{a} bought a Lemonade from the Celadon vending machine 🥤",
];

/// Rare, and worth it.
const POKEMON_BLESSING: &[&str] = &[
    "Nurse Joy healed {a} back to full health 💖",
    "{a} used a Max Potion. Good as new 🧪",
    "A Chansey waddled over and healed {a} with Soft-Boiled 🥚",
    "{a} ate a Rare Candy and grew a level on the spot 🍬",
];

/// Both sides gain: the battle pauses for something nicer.
const POKEMON_CROWD: &[&str] = &[
    "Both trainers stopped by the Pokémon Center for a quick heal 🏥",
    "A friendly Chansey handed out eggs to both sides",
    "{a} and {b} shared a picnic sandwich. Truce, for one turn 🥪",
    "A Wigglytuff sang, and both sides napped a little 🎵",
];

const POKEMON_FINISH: &[&str] = &[
    "{l} fainted! {w} wins the badge 🏅",
    "{l} is out of usable Pokémon. {w} takes the win",
    "{w} wins! {l} blacked out and ran to Nurse Joy",
    "{l} is blasting off agaaain! {w} is the winner 🚀",
    "{w} beat {l} and pocketed the prize money 💰",
    "{l} tried to run. Can't escape! {w} wins",
    "{w} is the new Gym Leader. {l} is back on Route 1",
    "{l} flopped like a Magikarp on land. {w} wins 🐟",
    "Gotcha! {w} caught the win, and {l} is off to the Pokémon Center",
];

const POKEMON_BYE: &[&str] = &[
    "{a} walked through tall grass all round. No wild opponent appeared",
    "A Snorlax is blocking the road, so {a} gets a free pass 💤",
    "{a}'s opponent is still stuck in the Poké Ball. Walkover",
];

const POKEMON_CHAMPION: &[&str] = &[
    "Beat the Elite Four and the whole server 🏆",
    "Pokémon League Champion of the server",
    "Gotta beat 'em all. Did.",
    "Hall of Fame, first try",
];

const POKEMON: Lines = Lines {
    exchange: POKEMON_EXCHANGE,
    crit: POKEMON_CRIT,
    miss: POKEMON_MISS,
    heal: POKEMON_HEAL,
    sip: POKEMON_SIP,
    backfire: POKEMON_BACKFIRE,
    drain: POKEMON_DRAIN,
    double: POKEMON_DOUBLE,
    chaos: POKEMON_CHAOS,
    snack: POKEMON_SNACK,
    blessing: POKEMON_BLESSING,
    crowd: POKEMON_CROWD,
    finish: POKEMON_FINISH,
    bye: POKEMON_BYE,
    champion: POKEMON_CHAMPION,
};

// --- harry potter -------------------------------------------------------------
// Spells, house points, detention and sweets from Honeydukes. Duels, not grudges.

const WIZARD_EXCHANGE: &[&str] = &[
    "{a} cast Stupefy and {b} bounced off the wall ✨",
    "{a} hit {b} with a Jelly-Legs Jinx. Wobble wobble",
    "{b} got caught by {a}'s Rictusempra and can't stop giggling",
    "{a} used Wingardium Leviosa on {b}, then let go 🪶",
    "{a} hit {b} with Flipendo. Lovely tumble",
    "{b} got Expelliarmus'd by {a}. Wand gone, pride gone 🪄",
    "{a} set a bag of Cornish pixies loose on {b}",
    "{a} cast Tarantallegra and {b} is dancing against their will 💃",
    "{a} took ten points from {b}'s house for no reason at all",
    "{b} walked straight into {a}'s Stinging Hex",
    "{a} tossed a Dungbomb at {b}'s feet. The whole corridor suffers",
    "Levicorpus! {a} left {b} dangling upside down",
    "{b} opened a Howler from {a}. Ears still ringing 📨",
    "{a} hit {b} with the Leg-Locker Curse. Hop along, {b}",
    "{a} set {b}'s robes smoking with Incendio 🔥",
    "{a} cast Aguamenti right at {b} 💧",
    "Peeves dropped a water balloon on {b}, and {a} took the credit",
    "{a} sent a Knockback Jinx flying into {b}",
    "{a} flicked a Chocolate Frog card at {b}. Surprisingly sharp",
];

const WIZARD_CRIT: &[&str] = &[
    "{a} cast Petrificus Totalus. {b} fell over like a plank 🪵",
    "{a} used Confringo and {b} flew across the Great Hall 💥",
    "{a} cast Reducto and {b} is picking up the pieces",
    "A Bludger out of nowhere, aimed by {a}, knocked {b} flat",
    "{a} got Snape to give {b} detention. Brutal",
    "{a} set a hippogriff on {b}. {b} forgot to bow",
];

const WIZARD_MISS: &[&str] = &[
    "{a} said Wingardium Levio-SAR. Nothing happened 🪶",
    "{a} waved the wand the wrong way round. Sparks, no spell",
    "{a}'s Stupefy hit a suit of armour instead of {b}",
    "{a} tried to Apparate behind {b} and landed in the lake",
];

const WIZARD_HEAL: &[&str] = &[
    "{a} bit a Chocolate Frog before it hopped away 🐸",
    "{a} sipped a warm Butterbeer by the fire",
    "{a} cast Episkey and patched up a bruise",
    "{a} had a Pepperup Potion. Steam out of the ears ♨️",
];

/// Barely anything, purely for the joke.
const WIZARD_SIP: &[&str] = &[
    "{a} ate a Bertie Bott's bean. Earwax flavour. +1 HP",
    "{a} found a stale Pumpkin Pasty in a robe pocket",
    "{a} earned one point for their house. Just the one",
    "{a} fixed their glasses with Oculus Reparo. Slightly better 👓",
];

/// The spell comes back at whoever cast it.
const WIZARD_BACKFIRE: &[&str] = &[
    "{a}'s wand backfired, just like Ron's broken one 🪄",
    "{a} held the wand backwards and Stupefied themselves",
    "{a} wandered too close to the Whomping Willow 🌳",
    "{a} forgot the common room password and got locked out in the cold",
];

/// {b} loses, {a} gains.
const WIZARD_DRAIN: &[&str] = &[
    "{a} cast Accio and {b}'s Chocolate Frog hopped over to {a} 🐸",
    "{a} Accio'd {b}'s Butterbeer right out of their hand",
    "{a} traded {b} a Bertie Bott's bean for a whole Pumpkin Pasty",
    "{a} copied {b}'s Potions homework and got {b}'s marks",
];

/// Two in a row, before anyone can answer.
const WIZARD_DOUBLE: &[&str] = &[
    "{a} cast Stupefy, then once more for luck. {b} felt both",
    "{a} fired Flipendo and Rictusempra at {b}, back to back",
    "Fred and George would be proud: {a} pranked {b} twice at once",
    "{a} dual-wielded wands at {b}. Ollivander would not approve 🪄",
];

/// Everyone in the frame suffers.
const WIZARD_CHAOS: &[&str] = &[
    "Dementors swooped past and the room went cold for everyone 🥶",
    "Peeves dumped a bucket of ink on both duellists",
    "The Whomping Willow woke up and swatted both of them 🌳",
    "The staircase moved mid-duel and dropped both on the wrong floor",
];

/// The biggest ordinary heal.
const WIZARD_SNACK: &[&str] = &[
    "{a} raided the Honeydukes stash 🍬",
    "{a} ate a whole Hogwarts feast in two minutes flat 🍗",
    "{a} downed a big mug of Butterbeer, foam and all",
    "{a} polished off an entire Cauldron Cake 🎂",
    "{a} snuck into the kitchens and the house-elves served treacle tart",
];

/// Rare, and worth it.
const WIZARD_BLESSING: &[&str] = &[
    "Fawkes flew in and cried on {a}'s wounds. Fully patched up 🔥",
    "Madam Pomfrey fixed {a} up in a flash. Back to the duel",
    "{a} took a lucky sip of Felix Felicis. Everything is going right ✨",
    "Dumbledore awarded {a} fifty points for sheer nerve 🏆",
];

/// Both sides gain: the duel pauses for something nicer.
const WIZARD_CROWD: &[&str] = &[
    "Hagrid handed out rock cakes. Hard, but it's the thought that counts",
    "Mrs Weasley knitted a jumper for both duellists 🧶",
    "The house-elves brought hot chocolate to both sides ☕",
    "Madam Pomfrey marched in and made both of them eat chocolate 🍫",
];

const WIZARD_FINISH: &[&str] = &[
    "{l} is Petrified. {w} wins the duel 🏆",
    "{w} wins! {l} is off to the Hospital Wing",
    "{w} disarmed {l} for good. Fifty points to {w}'s house",
    "{l} lost and got detention with Snape anyway. {w} wins",
    "{w} sent {l} splashing into the Black Lake 🌊",
    "{l} is stuck upside down under Levicorpus. {w} takes it",
    "Duel over: {w} wins, {l} is sulking in the common room",
    "{w} won. {l} is polishing trophies in detention",
    "{l} forgot the counter-jinx. {w} wins the House Cup",
];

const WIZARD_BYE: &[&str] = &[
    "{a}'s opponent got lost on the moving staircases. Free pass",
    "{a} waited all round, but the other duellist never left the library",
    "{a} gets a bye. The other duellist forgot the password 🚪",
];

const WIZARD_CHAMPION: &[&str] = &[
    "Triwizard Champion of the whole server 🏆",
    "House Cup winner, no points taken",
    "Top of the class, and the Duelling Club",
    "Undefeated. Even Snape clapped, once",
];

const WIZARD: Lines = Lines {
    exchange: WIZARD_EXCHANGE,
    crit: WIZARD_CRIT,
    miss: WIZARD_MISS,
    heal: WIZARD_HEAL,
    sip: WIZARD_SIP,
    backfire: WIZARD_BACKFIRE,
    drain: WIZARD_DRAIN,
    double: WIZARD_DOUBLE,
    chaos: WIZARD_CHAOS,
    snack: WIZARD_SNACK,
    blessing: WIZARD_BLESSING,
    crowd: WIZARD_CROWD,
    finish: WIZARD_FINISH,
    bye: WIZARD_BYE,
    champion: WIZARD_CHAMPION,
};

// --- dragon ball z ------------------------------------------------------------
// Ki blasts, Senzu Beans and a lot of screaming. Nobody stays down for long.

const SAIYAN_EXCHANGE: &[&str] = &[
    "{a} fired a ki blast. {b} felt that one",
    "{a} and {b} traded punches too fast for the camera",
    "{a} hit {b} with a Destructo Disc. Close shave 💿",
    "{b} got knocked through three mountains by {a} ⛰️",
    "{a} flashed a Solar Flare, then bopped {b} while they blinked",
    "{a} Instant Transmissioned behind {b}. Nothing personal",
    "{a} hit {b} with a Masenko 🌟",
    "{b} blocked, but {a}'s kick still sent them into a crater",
    "{a} fired a Dodon Ray at {b}. Tiny beam, real sting",
    "Afterimage trick! {a} was behind {b} the whole time",
    "{a} used the Tri-Beam on {b} and screamed about it",
    "{a} threw a Galick Gun at {b}. Very loud, very purple 💜",
    "{a} and {b} clashed so hard the whole planet shook",
    "{a} dropped out of the clouds with a flying kick on {b} ☁️",
    "{a} hit {b} with a rapid ki barrage. Smoke everywhere",
    "{a} asked 'is that all?' and swatted {b} across the arena",
    "{b} got punched into a rock, and {a} is already powering up again",
    "{a} fired a Special Beam Cannon and {b} only half dodged",
    "{a} flicked {b} with one finger. Disrespectful",
];

const SAIYAN_CRIT: &[&str] = &[
    "KA-ME-HA-ME-HAAA! {a} blasted {b} off the map 💥",
    "{a} went Super Saiyan and folded {b} with one punch ⚡",
    "{a} used the Big Bang Attack. {b} is now a crater",
    "{a} fired a Final Flash and {b} vanished into the light",
    "{a}'s power level is over 9000! {b} should have checked the scouter",
    "{a} went Kaioken x10 and {b} ate every single punch 🔴",
];

const SAIYAN_MISS: &[&str] = &[
    "{a} charged a blast for three episodes. {b} walked off",
    "{a}'s Kamehameha went straight into space 🌌",
    "{a} punched an afterimage. The real {b} was somewhere else",
    "{a}'s scouter exploded, and so did the aim",
];

const SAIYAN_HEAL: &[&str] = &[
    "{a} ate a bowl of rice in three seconds 🍚",
    "{a} meditated on a cliff and got a bit of ki back",
    "{a} got a quick patch-up from Dende 🌱",
    "{a} powered up with a long yell. Slightly better",
];

/// Barely anything, purely for the joke.
const SAIYAN_SIP: &[&str] = &[
    "{a} screamed for a full minute. Power level +1",
    "{a} did one push-up under 10x gravity. Tough, but one",
    "{a} checked the scouter. Info +1, health +1",
    "{a} found a crumb of Senzu Bean on the floor",
];

/// The attack comes back at whoever threw it.
const SAIYAN_BACKFIRE: &[&str] = &[
    "{a}'s Destructo Disc boomeranged right back 💿",
    "{a} pushed the Kaioken too far and paid for it 🔴",
    "{a} got caught mid-transformation and is now in the Yamcha pose",
    "{a}'s Spirit Bomb came down on {a} instead",
];

/// {b} loses, {a} gains.
const SAIYAN_DRAIN: &[&str] = &[
    "{a} absorbed {b}'s ki, Cell-style. Rude",
    "{a} caught {b}'s ki blast and soaked it right up",
    "{a} pocketed {b}'s Senzu Bean and ate it 🫘",
    "{a} pulled the energy out of {b} like Android 19 🔋",
];

/// Two in a row, before anyone can answer.
const SAIYAN_DOUBLE: &[&str] = &[
    "{a} hit {b} with a double Kamehameha, one from each hand",
    "{a} used the Multi-Form technique and both copies punched {b}",
    "{a} Instant Transmissioned twice and hit {b} from both sides",
    "{a} fired a Masenko, then followed up with a kick to {b} 🌟",
];

/// Everyone in the frame suffers.
const SAIYAN_CHAOS: &[&str] = &[
    "Krillin flew into the middle and the blast hit everyone, mostly Krillin",
    "Frieza blew up the planet. Both fighters are floating in space 🪐",
    "Majin Buu showed up bored and bopped both fighters 🍬",
    "Gravity jumped to 100x and flattened both fighters",
];

/// The biggest ordinary heal.
const SAIYAN_SNACK: &[&str] = &[
    "{a} ate 40 bowls of ramen between rounds 🍜",
    "{a} got a Senzu Bean and felt a lot better 🫘",
    "{a} had a full feast at Capsule Corp 🍗",
    "{a} rested in the Hyperbolic Time Chamber. A year inside, a minute out",
    "{a} took a hot spring break on King Kai's planet ♨️",
];

/// Rare, and worth it.
const SAIYAN_BLESSING: &[&str] = &[
    "Korin tossed {a} a whole Senzu Bean. Full power 🫘",
    "Shenron granted {a}'s wish: full health 🐉",
    "{a} got a hidden-potential boost from the Elder Kai ✨",
    "Dende healed {a} right back to full 🌱",
];

/// Both sides gain: the fight pauses for something nicer.
const SAIYAN_CROWD: &[&str] = &[
    "Everyone raised their hands for the Spirit Bomb. Both fighters felt it 🙌",
    "Chi-Chi called a lunch break. Nobody argues with Chi-Chi 🍚",
    "Mr. Satan claimed the credit, and the crowd cheered both fighters",
    "Korin handed both fighters a Senzu Bean. Fair is fair 🫘",
];

const SAIYAN_FINISH: &[&str] = &[
    "{l} is lying in the Yamcha pose. {w} wins",
    "{w} charged up a Kamehameha and {l} is done 💥",
    "{w}'s power level was simply higher. {l} is out",
    "{l} will be wished back with the Dragon Balls. {w} wins 🐉",
    "{w} wins. {l} just got Krillin'd",
    "{w} went Super Saiyan and {l} never recovered ⚡",
    "{l} flew off shouting about a rematch. {w} takes it",
    "{w} wins, and {l} is off to train with King Kai",
    "{l} is out of ki. {w} wins, no filler episode needed",
];

const SAIYAN_BYE: &[&str] = &[
    "{a} trained with King Kai all round. Nobody showed up",
    "{a}'s opponent is still charging up. Free pass 🌀",
    "{a} spent this round in the Hyperbolic Time Chamber. Walkover",
];

const SAIYAN_CHAMPION: &[&str] = &[
    "Power level: over 9000 ⚡",
    "Strongest in the universe (well, the server)",
    "Won the whole Tournament of Power 🏆",
    "Beat everyone. Didn't even need a Spirit Bomb",
];

const SAIYAN: Lines = Lines {
    exchange: SAIYAN_EXCHANGE,
    crit: SAIYAN_CRIT,
    miss: SAIYAN_MISS,
    heal: SAIYAN_HEAL,
    sip: SAIYAN_SIP,
    backfire: SAIYAN_BACKFIRE,
    drain: SAIYAN_DRAIN,
    double: SAIYAN_DOUBLE,
    chaos: SAIYAN_CHAOS,
    snack: SAIYAN_SNACK,
    blessing: SAIYAN_BLESSING,
    crowd: SAIYAN_CROWD,
    finish: SAIYAN_FINISH,
    bye: SAIYAN_BYE,
    champion: SAIYAN_CHAMPION,
};

// --- wwe ----------------------------------------------------------------------
// Ring moves, steel chairs and a commentator losing it. Kayfabe only.

const WRESTLING_EXCHANGE: &[&str] = &[
    "{a} hit {b} with a snap suplex 💪",
    "{a} whipped {b} into the ropes and clotheslined them on the rebound",
    "{b} ate a big boot from {a}",
    "{a} dropkicked {b} right off the apron",
    "{a} slammed {b} onto the announce table. Commentators scatter",
    "DDT! {a} planted {b} in the middle of the ring",
    "{b} got caught in {a}'s spinebuster",
    "{a} chopped {b} so loud the whole crowd went WOOO 🗣️",
    "{a} hit {b} with a steel chair. The ref saw nothing 🪑",
    "{a} hit a flying crossbody on {b} from the top rope",
    "{b} got speared by {a}. The crowd is on its feet",
    "{a} hit {b} with a Stone Cold Stunner",
    "{a} locked {b} in a sleeper hold. The ref checks the arm",
    "{a} hit {b} with a running bulldog",
    "{a} powerbombed {b}. The ring is still bouncing",
    "{b} rolled right into a German suplex from {a}",
    "The crowd chanted ONE MORE TIME, so {a} suplexed {b} again",
    "{a} tossed {b} over the top rope",
    "{a} hit {b} with a Zig Zag. Textbook",
];

const WRESTLING_CRIT: &[&str] = &[
    "RKO outta nowhere! {a} dropped {b} 🐍",
    "{a} chokeslammed {b} right through the mat",
    "BAH GAWD! {a} just launched {b} off the top of the cage!",
    "{a} hit the 619 on {b} and the arena erupted",
    "{a} dropped the People's Elbow on {b}. Electrifying ⚡",
    "{a} hit a top-rope elbow drop on {b} through a table",
];

const WRESTLING_MISS: &[&str] = &[
    "{a} went for the elbow drop and {b} rolled away",
    "{a} did the 'You can't see me' taunt. {b} saw it and moved",
    "{a} climbed the top rope, waved to the crowd and forgot to jump",
    "{a} went for the pin. {b} kicked out at one",
];

const WRESTLING_HEAL: &[&str] = &[
    "{a} rolled out of the ring and grabbed an ice pack 🧊",
    "{a} took a breather on the apron while the ref counted",
    "{a} drank a protein shake at ringside 🥤",
    "{a} fed off the crowd chants and got back up",
];

/// Barely anything, purely for the joke.
const WRESTLING_SIP: &[&str] = &[
    "{a} flexed for the camera. HP +1 💪",
    "{a} adjusted the kneepads. That's it",
    "{a} did one push-up in the corner and posed",
    "{a} grabbed the mic and said absolutely nothing 🎤",
];

/// The move comes back at whoever threw it.
const WRESTLING_BACKFIRE: &[&str] = &[
    "{a} went for a moonsault, missed everything and landed flat",
    "{a} swung the steel chair, it bounced off the ropes and hit {a} 🪑",
    "{a} tried to cash in the briefcase and dropped it on their own foot 💼",
    "{a} dove through the ropes and crashed into the announce table",
];

/// {b} loses, {a} gains.
const WRESTLING_DRAIN: &[&str] = &[
    "{a} stole {b}'s entrance music and the crowd's love with it 🎵",
    "{a} drank {b}'s protein shake at ringside 🥤",
    "{a} took {b}'s title belt for a victory lap",
    "{a} got the crowd chanting against {b} and fed on it",
];

/// Two in a row, before anyone can answer.
const WRESTLING_DOUBLE: &[&str] = &[
    "{a} hit {b} with back-to-back suplexes",
    "{a} hit the 619, then a frog splash on {b}",
    "{a} made the tag, and the tag team both hit {b} at once",
    "{a} gave {b} two chokeslams, one for each side of the ring",
];

/// Everyone in the frame suffers.
const WRESTLING_CHAOS: &[&str] = &[
    "Ref bump! Nobody's counting, and both wrestlers crashed into each other",
    "Someone's entrance music hit mid-match, and both got ambushed 🎵",
    "{a} and {b} both went through the announce table. BAH GAWD!",
    "The lights went out. When they came back, both were down 💡",
];

/// The biggest ordinary heal.
const WRESTLING_SNACK: &[&str] = &[
    "{a} chugged two protein shakes at ringside 🥤",
    "{a} got patched up by the trainer backstage",
    "{a} sat in an ice bath during the commercial break 🧊",
    "{a} rolled outside and raided the catering table 🍕",
    "{a} got a second wind from the entrance video replay",
];

/// Rare, and worth it.
const WRESTLING_BLESSING: &[&str] = &[
    "{a}'s entrance music hit again and the arena went wild 🎶",
    "{a} hulked up and no-sold everything ✨",
    "The whole crowd chanted {a}'s name. Fully fired up 📣",
    "{a} cashed in the Money in the Bank briefcase for full health 💼",
];

/// Both sides gain: the match pauses for something nicer.
const WRESTLING_CROWD: &[&str] = &[
    "Commercial break! Both wrestlers grab some water 📺",
    "The crowd chanted THIS IS AWESOME and both got a boost",
    "Both shook hands for the code of honor. Respect heals",
    "The GM called a timeout and handed out ice packs 🧊",
];

const WRESTLING_FINISH: &[&str] = &[
    "1... 2... 3! {w} pins {l} 🔔",
    "{l} tapped out. {w} wins",
    "{w} hit the finisher and {l} stayed down. BAH GAWD!",
    "{w} wins with an RKO outta nowhere. {l} never saw it 🐍",
    "{l} got counted out arguing with the ref. {w} wins",
    "{w} is the new champion. {l} is back on the pre-show",
    "{l} is headed to catering. {w} takes the belt 🏆",
    "{w} dropped the People's Elbow and {l} is done",
    "{w} won. {l} is already demanding a rematch on Raw",
    "{w}'s music hits. {l} is face down on the mat",
];

const WRESTLING_BYE: &[&str] = &[
    "{a} cut a promo in an empty ring. Nobody came out",
    "{a}'s opponent got stuck in traffic. Win by forfeit",
    "{a} waited for entrance music that never hit. Free pass 🎵",
];

const WRESTLING_CHAMPION: &[&str] = &[
    "Undisputed champion of the server 🏆",
    "Holds every belt in the building",
    "Main-evented and won. BAH GAWD!",
    "Last one standing in the Royal Rumble 👑",
];

const WRESTLING: Lines = Lines {
    exchange: WRESTLING_EXCHANGE,
    crit: WRESTLING_CRIT,
    miss: WRESTLING_MISS,
    heal: WRESTLING_HEAL,
    sip: WRESTLING_SIP,
    backfire: WRESTLING_BACKFIRE,
    drain: WRESTLING_DRAIN,
    double: WRESTLING_DOUBLE,
    chaos: WRESTLING_CHAOS,
    snack: WRESTLING_SNACK,
    blessing: WRESTLING_BLESSING,
    crowd: WRESTLING_CROWD,
    finish: WRESTLING_FINISH,
    bye: WRESTLING_BYE,
    champion: WRESTLING_CHAMPION,
};

// --- counter-strike 2 and elden ring (placeholders until written: they reuse classic) ---

const TACTICAL: Lines = CLASSIC;
const TARNISHED: Lines = CLASSIC;

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
    fn every_line_has_its_names_and_fits() {
        for theme in Theme::ALL {
            let l = theme.lines();
            for (name, pool) in [("exchange", l.exchange), ("drain", l.drain)] {
                for line in pool {
                    assert!(line.contains("{a}") && line.contains("{b}"), "{:?} {}: {}", theme, name, line);
                }
            }
            for line in l.finish {
                assert!(line.contains("{w}") && line.contains("{l}"), "{:?} finish: {}", theme, line);
            }
            for line in l.bye {
                assert!(line.contains("{a}"), "{:?} bye: {}", theme, line);
            }
            for line in l.champion {
                assert!(!line.contains('{'), "{:?} champion has a placeholder: {}", theme, line);
                assert!(line.chars().count() <= 60, "{:?} champion too long: {}", theme, line);
            }
            for (name, pool) in pools(l) {
                for line in pool {
                    assert!(line.chars().count() <= 110, "{:?} {} too long: {}", theme, name, line);
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
