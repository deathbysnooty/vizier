//! Checking a typed answer against a riddle's accepted answers.
//!
//! People type on phones, in a hurry, in Hinglish. So both sides are brought to
//! one plain form first - lower case, accents folded, punctuation gone, a
//! leading "a"/"an"/"the"/"my" dropped - and compared with and without spaces.
//! After that a slip is forgiven on longer answers only: one letter on answers
//! of five to eight letters, two from nine up, none on short ones where a
//! single letter makes a different word. Anything with a digit must be exact.
//! Number words are never turned into digits: the bank lists both when both
//! should count.

/// Leading words that change nothing: "a samosa" is "samosa".
const FILLERS: &[&str] = &["a", "an", "the", "my"];

/// One accented Latin letter as its plain letter(s).
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'ĉ' | 'ċ' | 'č' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'ș' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ț' => "t",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        'þ' => "th",
        _ => return None,
    })
}

/// The plain form both sides are compared in: words separated by one space.
pub fn normalise(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.trim().to_lowercase().chars() {
        if let Some(plain) = fold_char(c) {
            out.push_str(plain);
        } else if matches!(c, '\'' | '’' | '‘' | '`' | '´') {
            // "hagrid's" is "hagrids", not "hagrid s".
        } else if c.is_alphanumeric() {
            out.push(c);
        } else {
            out.push(' ');
        }
    }
    let mut words: Vec<&str> = out.split_whitespace().collect();
    while words.len() > 1 && FILLERS.contains(&words[0]) {
        words.remove(0);
    }
    words.join(" ")
}

pub fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + usize::from(ca != cb));
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// How many slips an answer of this many letters forgives.
fn slack(letters: usize) -> usize {
    match letters {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

/// Whether a guess is one accepted answer, as the rules above say.
pub fn matches_one(guess: &str, answer: &str) -> bool {
    let (g, a) = (normalise(guess), normalise(answer));
    if g.is_empty() || a.is_empty() {
        return false;
    }
    if g == a {
        return true;
    }
    let (g, a) = (g.replace(' ', ""), a.replace(' ', ""));
    if g == a {
        return true;
    }
    if g.chars().chain(a.chars()).any(|c| c.is_ascii_digit()) {
        return false;
    }
    let allowed = slack(a.chars().count());
    allowed > 0 && levenshtein(&g, &a) <= allowed
}

/// Whether a guess is any of the accepted answers.
pub fn is_correct(guess: &str, answers: &[String]) -> bool {
    answers.iter().any(|answer| matches_one(guess, answer))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_plain_form_drops_case_accents_punctuation_and_fillers() {
        assert_eq!(normalise("  The Keyboard! "), "keyboard");
        assert_eq!(normalise("A  samosa"), "samosa");
        assert_eq!(normalise("an   Owl"), "owl");
        assert_eq!(normalise("My shadow"), "shadow");
        assert_eq!(normalise("the"), "the", "a filler on its own is still an answer");
        assert_eq!(normalise("Crème brûlée"), "creme brulee");
        assert_eq!(normalise("Hagrid's hut"), "hagrids hut");
        assert_eq!(normalise("pani-puri"), "pani puri");
        assert_eq!(normalise("Wingardium   Leviosa."), "wingardium leviosa");
        assert_eq!(normalise("Hogwarts’ Express"), "hogwarts express");
        assert_eq!(normalise("?!"), "");
    }

    #[test]
    fn exact_answers_count_in_any_case_with_or_without_fillers() {
        let answers = list(&["keyboard", "computer keyboard"]);
        for yes in ["keyboard", "KEYBOARD", " a keyboard ", "The keyboard.", "my keyboard", "Computer Keyboard", "computer-keyboard"] {
            assert!(is_correct(yes, &answers), "{yes:?} should count");
        }
    }

    #[test]
    fn spaces_do_not_matter() {
        let golgappa = list(&["pani puri", "panipuri", "golgappa", "gol gappa"]);
        assert!(is_correct("gol gappe", &golgappa), "one slip on a 8-letter answer");
        assert!(is_correct("golgappa", &golgappa));
        assert!(is_correct("Gol-Gappa", &golgappa));
        assert!(is_correct("pani  puri", &golgappa));
        assert!(is_correct("paanipuri", &golgappa), "one extra letter");
        assert!(is_correct("wingardiumleviosa", &list(&["wingardium leviosa"])));
    }

    #[test]
    fn typos_are_forgiven_by_length() {
        // 5 to 8 letters: one slip.
        let jalebi = list(&["jalebi"]);
        assert!(!is_correct("jalebee", &jalebi), "two slips on six letters is too many");
        assert!(is_correct("jalebo", &jalebi));
        assert!(is_correct("jlebi", &jalebi));
        assert!(!is_correct("jlbi", &jalebi));
        let keyboard = list(&["keyboard"]);
        assert!(!is_correct("keybaord", &keyboard), "a swap is two edits");
        assert!(is_correct("keyboad", &keyboard));
        assert!(!is_correct("keybo", &keyboard), "half a word is not the answer");
        // 9 and up: two slips.
        let expelliarmus = list(&["expelliarmus"]);
        assert!(is_correct("expeliarmus", &expelliarmus));
        assert!(is_correct("expeliarmos", &expelliarmus));
        assert!(!is_correct("expliarmos", &expelliarmus));
        assert!(is_correct("dumbeldore", &list(&["dumbledore"])), "a swap fits in two on long answers");
    }

    #[test]
    fn short_answers_must_be_exact() {
        let owl = list(&["owl"]);
        assert!(is_correct("owl", &owl));
        assert!(is_correct("An owl", &owl));
        assert!(!is_correct("owls", &owl));
        assert!(!is_correct("ow", &owl));
        let chai = list(&["chai", "tea"]);
        assert!(!is_correct("chat", &chai));
        assert!(!is_correct("teas", &chai));
        assert!(is_correct("TEA", &chai));
        // Five letters is the first length that forgives one.
        assert!(is_correct("brom", &list(&["broom"])));
        assert!(is_correct("brooom", &list(&["broom"])));
        assert!(!is_correct("bram", &list(&["bran"])), "four letters: no slips");
    }

    #[test]
    fn numbers_are_exact_and_never_turned_into_words() {
        let seven = list(&["seven", "7"]);
        assert!(is_correct("7", &seven));
        assert!(is_correct("Seven", &seven));
        assert!(is_correct("sevn", &seven), "the word form still forgives a slip");
        assert!(!is_correct("8", &seven));
        let only_word = list(&["seven"]);
        assert!(!is_correct("7", &only_word), "digits only count when the bank lists them");
        let platform = list(&["platform 9 3 4", "platform nine and three quarters"]);
        assert!(is_correct("Platform 9 3/4", &platform));
        assert!(!is_correct("platform 9 3 5", &platform), "a digit must match exactly");
        assert!(!is_correct("platform 8 3 4", &platform));
    }

    #[test]
    fn hinglish_aliases_count_as_listed() {
        let chai = list(&["chai", "tea", "masala chai", "adrak chai", "cha", "chaa", "chay", "cutting chai"]);
        for yes in ["Chai", "masala-chai", "adrak  chai", "cuttingchai", "chaa", "masala chae"] {
            assert!(is_correct(yes, &chai), "{yes:?} should count");
        }
        for no in ["coffee", "chaat", "lassi", ""] {
            assert!(!is_correct(no, &chai), "{no:?} should not count");
        }
        let jalebi = list(&["jalebi", "jalebis", "jilebi", "jilapi", "jalabi", "jilipi", "zulbia", "jaleebi"]);
        assert!(is_correct("Jalebii", &jalebi), "one slip from jalebi");
        assert!(is_correct("zalebi", &jalebi));
    }

    #[test]
    fn nothing_typed_is_never_right() {
        assert!(!is_correct("", &list(&["keyboard"])));
        assert!(!is_correct("   ", &list(&["keyboard"])));
        assert!(!is_correct("...", &list(&["keyboard"])));
        assert!(!is_correct("keyboard", &[]));
        assert!(!matches_one("keyboard", "  "));
    }

    #[test]
    fn levenshtein_counts_edits() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("keyboard", "keybaord"), 2);
        assert_eq!(levenshtein("chai", "chai"), 0);
    }
}
