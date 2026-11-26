//! Text preparation and filtering for Discord features that read member messages.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

const MESSAGE_CHARS: usize = 280;

static USER_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<@[!&]?\d+>").expect("regex"));
static CHANNEL_MENTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<#\d+>").expect("regex"));
static CUSTOM_EMOJI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<a?:(\w+):\d+>").expect("regex"));
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\b(?:https?://|www\.)\S+").expect("regex"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("regex"));
/// `!play`, `/help`, `.fish`, `$bal`, `;;p`, `p!help`, `owo hunt`: a command
/// for some bot, not the member talking.
static COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:[!/.$%?;>~=+-]+[a-z]|[a-z]{1,3}!\w|owo\b|uwu\b|pls\s+(?:beg|fish|hunt|dig|daily)\b)").expect("regex"));

/// The words of a message as the model should read them - mentions and links
/// taken out, custom emoji by name - or `None` for a bot command, a one-word
/// reply or anything else too short to summarise.
pub fn usable(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || COMMAND.is_match(trimmed) {
        return None;
    }
    let t = USER_MENTION.replace_all(trimmed, "@someone");
    let t = CHANNEL_MENTION.replace_all(&t, "#channel");
    let t = CUSTOM_EMOJI.replace_all(&t, ":$1:");
    let t = URL.replace_all(&t, "");
    let t = SPACES.replace_all(t.trim(), " ").to_string();
    let words = t.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric) && !w.starts_with('@') && !w.starts_with(':')).count();
    let letters = t.chars().filter(|c| c.is_alphanumeric()).count();
    if words < 2 || (words < 3 && letters < 12) {
        return None;
    }
    Some(cut(&t, MESSAGE_CHARS))
}

/// Cut at a word, with an ellipsis.
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    let head = match head.rfind(' ') {
        Some(i) if i > max / 2 => head[..i].to_string(),
        _ => head,
    };
    format!("{}…", head.trim_end())
}

/// A rough token count for Hinglish in Roman script: about 3.5 characters a token.
pub fn estimate_tokens(text: &str) -> usize {
    (text.chars().count() * 2).div_ceil(7)
}

/// Sensitive areas filtered from member catchphrases and daily topics.
pub const BANNED: &[(&str, &str)] = &[
    (
        "health",
        r"health|healthy|ill|illness|sick|sickness|disease|doctor|doctors|hospital|medicine|medicines|medication|meds|therapy|therapist|depress\w*|anxiety|anxious|adhd|autis\w*|bipolar|mental|suicid\w*|self[- ]harm|trauma\w*|ptsd|diagnos\w*|disorder|surgery|cancer|diabet\w*|pregnan\w*|injur\w*|insomnia|lonely|loneliness|burn(?:ed|t)?[- ]out|stress(?:ed|ful)?|panic|grief|grieving",
    ),
    (
        "religion or caste",
        r"religio\w*|hindu\w*|muslim\w*|islam\w*|christian\w*|sikh\w*|jain|jains|buddhis\w*|atheis\w*|god|gods|allah|bhagwan|temple|mandir|mosque|masjid|church|gurudwara|namaz|pray\w*|puja|pooja|fasting|roza|ramadan|ramzan|eid|navratri|spiritual\w*|caste\w*|brahmin\w*|dalit\w*|rajput\w*|jaat|yadav|obc|upper[- ]caste|lower[- ]caste|hindutva",
    ),
    (
        "politics",
        r"politic\w*|election\w*|vote|votes|voting|voter\w*|bjp|congress|aap|modi|gandhi|kejriwal|government|govt|leftist\w*|left[- ]wing|right[- ]wing|rightist\w*|liberal\w*|conservative\w*|sanghi\w*|bhakt\w*|libtard\w*|rss|protest\w*|parliament|minister\w*|trump|biden|communis\w*|socialis\w*|nationalis\w*|party line",
    ),
    (
        "sexuality or gender identity",
        r"gay|lesbian\w*|bisexual\w*|queer|lgbt\w*|trans|transgender|non[- ]?binary|asexual|sexuality|sexual\w*|sex|gender\w*|pronoun\w*|closeted|coming out|came out",
    ),
    (
        "relationships",
        r"girlfriend\w*|boyfriend\w*|gf|bf|crush\w*|dating|relationship\w*|partner|partners|married|marriage|wife|husband|spouse|fianc\w*|ex|breakup\w*|broke up|single|situationship\w*|simp\w*|love life|flirt\w*|shaadi|bandi|seeing someone|romantic\w*|romance",
    ),
    (
        "family",
        r"family|families|mom|moms|mum|mother\w*|dad|dads|father\w*|parent|parents|sister\w*|brother\w*|sibling\w*|son|sons|daughter\w*|kid|kids|child|children|cousin\w*|uncle\w*|aunt\w*|aunty|auntie|grand\w*|mummy|papa|maa|didi|behen|nani|dadi|in[- ]laws?",
    ),
    (
        "where they live, study or work",
        r"lives?|living in|hometown|home town|city|village|neighbou?rhood|delhi|mumbai|bombay|bangalore|bengaluru|kolkata|calcutta|chennai|hyderabad|pune|jaipur|lucknow|noida|gurgaon|gurugram|ahmedabad|chandigarh|indore|bhopal|patna|kerala|punjab\w*|bihar\w*|gujarat\w*|rajasthan\w*|maharashtra\w*|karnataka|bengal\w*|assam\w*|goa|kashmir\w*|tamil nadu|telangana|odisha|jharkhand|haryana|uttar pradesh|canada|dubai|usa|abroad|school|schools|college|colleges|university|uni|classes|student|students|exam\w*|jee|neet|upsc|cbse|icse|iit\w*|nit|degree|semester|sem|hostel|campus|studies|studying|job|jobs|office|company|employer|boss|salary|intern\w*|career|profession\w*|colleague\w*|workplace|works as|works at|working at|coworker\w*",
    ),
    (
        "when they are online or their routine",
        r"online|offline|active at|night|nights|nightly|late[- ]night|midnight|morning\w*|evening\w*|afternoon\w*|\d{1,2}\s?(?:am|pm)|o'?clock|routine|schedule\w*|sleep\w*|asleep|wakes?|waking|woke|night owl|weekday\w*|weekend\w*|every day at|daily at|usually around|always around|hours of",
    ),
    (
        "unkind",
        r"stupid|dumb|idiot\w*|annoying|annoys|cringe\w*|toxic|rude|lazy|ugly|fat|loser\w*|creepy|obnoxious|arrogant|pathetic|attention[- ]seek\w*|irritating|immature|childish|spams?|spamming|clueless|whin\w*|brat\w*|jerk\w*",
    ),
];

static BANNED_RES: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    BANNED.iter().map(|(area, words)| (*area, Regex::new(&format!(r"(?i)\b(?:{})\b", words)).expect("banned words"))).collect()
});

static HARMLESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:(?:game|movie|quiz|trivia|karaoke|music)\s+nights?|online\s+(?:games?|gaming|chess|matches))\b").expect("regex"));

fn words_of(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(String::from)
        .collect()
}

/// Every run of `n` words in these texts.
pub fn shingles(texts: &[String], n: usize) -> HashSet<String> {
    let mut out = HashSet::new();
    for t in texts {
        let w = words_of(t);
        for window in w.windows(n) {
            out.insert(window.join(" "));
        }
    }
    out
}

/// The sensitive area matched by the text, if any.
pub fn sensitive_area(text: &str) -> Option<&'static str> {
    // Events like movie nights and online games do not reveal a routine.
    let checked = HARMLESS.replace_all(text, "event");
    BANNED_RES.iter().find_map(|(area, re)| re.is_match(&checked).then_some(*area))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_one_word_replies_and_links_are_not_read() {
        for skip in ["/help", "!play despacito", ".fish", "$bal", "p!help", "owo hunt", "lol", "haan", "ok bhai", "https://youtu.be/x", "<@123456789012345678>"] {
            assert!(usable(skip).is_none(), "{:?} should be skipped", skip);
        }
        assert_eq!(usable("<@123456789012345678> kal chess khelte hain yaar").as_deref(), Some("@someone kal chess khelte hain yaar"));
        assert!(usable("bhai ye anagram toh easy tha").is_some());
    }

    #[test]
    fn the_filter_throws_out_every_banned_area() {
        for (area, bullet) in [
            ("health", "Often talks about their anxiety before exams"),
            ("health", "Has been open about therapy"),
            ("religion or caste", "Shares memes about their religion"),
            ("religion or caste", "Proud of being a Rajput"),
            ("politics", "Argues about Modi and the BJP"),
            ("politics", "Strong political opinions"),
            ("sexuality or gender identity", "Openly gay and jokes about it"),
            ("sexuality or gender identity", "Talks about their pronouns"),
            ("relationships", "Keeps mentioning their girlfriend"),
            ("relationships", "Has a crush on someone in voice"),
            ("family", "Complains about their mom a lot"),
            ("family", "Has a younger brother who games"),
            ("where they live, study or work", "Lives in Pune and loves the rain"),
            ("where they live, study or work", "Preparing for JEE"),
            ("where they live, study or work", "Talks about their office job"),
            ("when they are online or their routine", "Always online late at night"),
            ("when they are online or their routine", "Shows up around 2am"),
            ("unkind", "Can be pretty annoying in voice"),
        ] {
            assert_eq!(sensitive_area(bullet), Some(area), "{:?}", bullet);
        }
        for fine in ["Hosts movie nights and plays online games", "Brings up cricket every match day", "Dry, deadpan humour with lots of puns", "Known for fast anagram solves", "Hypes everyone up in the chess channel"] {
            assert_eq!(sensitive_area(fine), None, "{:?}", fine);
        }
    }
}
