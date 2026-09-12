#!/usr/bin/env python3
"""Turn kind:"text" rows in the quiz bank into kind:"mcq" rows with natural distractors.

    python3 to_mcq.py --preview            # write examples, change nothing
    python3 to_mcq.py --preview --out X    # preview to a specific file
    python3 to_mcq.py --write              # rewrite ai/*.jsonl and wikidata/*.jsonl in place

Only ai/ and wikidata/ are rewritten: api/ is already all MCQ (it is still read, as
vocabulary for the answer-type lexicon below).  Rows whose id is in dropped.json are
left exactly as they are, and so is any row that is already mcq.

Where the three wrong options come from, best source first:

  gen     shapes that are better invented than sampled - years, numbers, money,
          number+unit, dates, ordinals, single letters - and the closed classes
          (weekday, month, direction, finishing position) are built around the answer.
  tier 0  same file, same question template: either the same leading words, or the
          same question *pattern* ("... stand for?", "What number comes next", "Complete
          the proverb", "What am I?").  Template siblings that are already mcq also
          donate their wrong options - those were written for this exact question shape.
  tier 1  same file, same thing-being-asked-for (the noun after Which/What, or Who).
  tier 2  same file, same answer class (see Lexicon) and same shape.
  tier 3  other files, for acronyms and single letters only, where one topic file
          rarely holds three of them.

Two things stop a candidate that is merely the right *shape* from being the wrong
*sort of thing* ("Italy" next to "Gulzar", "Jaipur" next to "Kerala"):

  * Lexicon - every answer in the bank is typed by the question that asks for it
    ("Which city ..." types its answer as a city), and that typing carries over to
    every other row with the same answer string.  Two answers with known and different
    classes are never shown together.
  * question overlap - candidates whose own question shares vocabulary with this one
    rank first, which keeps distractors inside the same corner of the topic.

A candidate also has to survive every hard rule in `rejects()`.  Rows that cannot get
three good options stay typed questions and are listed in the report.
"""

import argparse
import json
import random
import re
import subprocess
import unicodedata
from collections import Counter, defaultdict
from pathlib import Path

BANK = Path(__file__).resolve().parent.parent
REWRITE = ("ai", "wikidata")          # api/ is already mcq
READ_ONLY = ("api",)                  # read for the lexicon, never written
MAX_OPTION_CHARS = 80

# ---------------------------------------------------------------- vocabularies

WEEKDAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
MONTHS = ["January", "February", "March", "April", "May", "June",
          "July", "August", "September", "October", "November", "December"]
MONTH_LEN = {"January": 31, "February": 28, "March": 31, "April": 30, "May": 31, "June": 30,
             "July": 31, "August": 31, "September": 30, "October": 31, "November": 30, "December": 31}
DIRECTIONS = ["North", "South", "East", "West", "North-east", "North-west", "South-east", "South-west"]
POSITIONS = ["First", "Second", "Third", "Fourth", "Fifth", "Last", "Second last"]
CLOSED_CLASSES = {"weekday": WEEKDAYS, "month": MONTHS, "direction": DIRECTIONS, "position": POSITIONS}
CLOSED_LOOKUP = {w.lower(): name for name, words in CLOSED_CLASSES.items() for w in words}

# lower-case unit words we are willing to read as "number + unit": "2 kg" is a weight,
# "2 States" is a film.
UNIT_WORDS = {
    "minute", "minutes", "hour", "hours", "second", "seconds", "day", "days", "week", "weeks",
    "month", "months", "year", "years", "km", "km/h", "kmph", "kmh", "mph", "m", "cm", "mm",
    "metre", "metres", "meter", "meters", "kg", "g", "gram", "grams", "litre", "litres", "liter",
    "liters", "ml", "degrees", "degree", "runs", "marks", "rupees", "paise", "crore", "crores",
    "lakh", "lakhs", "million", "billion", "percent", "miles", "mile", "feet", "foot", "inches", "inch",
}

STOPWORDS = {
    "the", "a", "an", "of", "in", "on", "at", "to", "for", "and", "or", "is", "are", "was", "were",
    "which", "what", "who", "whom", "whose", "when", "where", "how", "many", "much", "does", "do",
    "did", "this", "that", "these", "those", "it", "its", "s", "by", "with", "from", "as", "be",
    "has", "have", "had", "you", "your", "their", "his", "her", "name", "named", "called", "known",
    "but", "not", "they", "them", "he", "she", "we", "i", "me", "my", "one", "two", "first", "also",
}

# adjectives that sit between "which" and the noun we actually want
ADJECTIVES = {
    "indian", "famous", "legendary", "popular", "english", "hindi", "tamil", "telugu", "kannada",
    "malayalam", "marathi", "bengali", "punjabi", "australian", "american", "british", "us",
    "south", "north", "east", "west", "northeastern", "southern", "northern", "eastern", "western",
    "new", "old", "young", "great", "classic", "original", "real", "common", "traditional",
    "ancient", "modern", "tiny", "huge", "little", "big", "small", "main", "top", "best", "only",
    "same", "other", "future", "former", "current", "long", "short", "single", "double",
}

# the noun a question asks for -> the class of thing its answer is
CLASS_WORDS = {
    # real people, kept apart from the people inside a story
    "person": """who whom actor actress singer director player captain cricketer batsman bowler
        author writer poet scientist leader composer musician rapper
        artist coach manager comedian god goddess king queen emperor ruler president
        chef designer founder dancer politician wrestler boxer golfer batter keeper spinner
        pacer economist activist soldier general saint guru sultan nawab""",
    # people (and creatures) inside a story: a dwarf question must not draw film titles
    "character": """character dwarf resident villain hero heroine sidekick mascot protagonist
        wizard witch pokemon creature dragon elf goblin ninja superhero villager toon
        housemate contestant alchemist professor headmaster""",
    "role": "profession occupation job living trade career",
    "object": """object thing item device gadget weapon tool product goods gift toy vehicle
        instrument ingredient snack""",
    "business": "business shop store stall establishment",
    "organisation": "organisation organization company firm channel network studio label",
    "country": "country nation",
    "city": "city town village metro",
    "state": "state province",
    "place": """place region district site monument building temple fort palace lake river mountain
        peak island sea ocean desert park beach airport station stadium ground venue capital street
        road bridge cave waterfall valley continent""",
    "film": "film movie",
    "show": "show series sitcom serial drama programme channel",
    "song": "song track jingle album",
    "book": "book novel epic story comic",
    "sport": "game sport tournament league trophy cup award prize",
    "team": "team club franchise side squad band group company brand organisation party agency bank app",
    "animal": "animal bird fish insect creature pet breed",
    "food": "dish food snack sweet dessert curry bread spice fruit vegetable drink beverage grain pulse",
    "element": "element metal gas acid chemical compound mineral vitamin protein hormone enzyme material fabric",
    "language": "language tongue script",
    "colour": "colour color",
}
CLASS_OF_WORD = {w: cls for cls, words in CLASS_WORDS.items() for w in words.split()}

# classes that may stand beside each other.  A first-name question can take character
# names; a film title can take a show title; a city is NOT a generic "place", which is
# what put three north-eastern capitals next to Jaipur.
COMPATIBLE = [
    {"title", "film", "show", "song", "book", "sport"},
    {"person", "character", "name"},
]
# classes where an untyped option is not good enough: these are the ones that were
# trading answers across question shapes inside a single file
STRICT_CLASSES = {"character", "role", "object", "business", "organisation", "name",
                  "animal", "food", "element", "colour"}
# rows the owner has fixed by hand: never rebuild these
NEVER_TOUCH = {"ai-harry_potter-0038", "ai-anime_comics-0174"}

# three question shapes where a wrong-kind option hands the answer over
BLANK_RE = re.compile(r"___|\bcomplete the\b", re.I)
ACRONYM_RE = re.compile(r"[A-Z0-9&.]{2,6}")


def frame_of(row):
    """Which giveaway-prone shape this row is, if any.

    blank      - a line with a hole in it: the option must be able to finish the line
    expansion  - "what does DLC stand for": every option must expand an abbreviation
                 (the mirror shape, "which three letters mean X", is deliberately excluded)
    plainword  - a bare lower-case noun: the option must be the same category of thing
    """
    answer = row["a"]
    if BLANK_RE.search(row["q"]):
        return "blank"
    if pattern_of(row["q"]) == "stands_for" and not ACRONYM_RE.fullmatch(answer.strip()):
        return "expansion"
    if len(answer.split()) == 1 and answer[:1].islower():
        return "plainword"
    return None


def plain_word(text):
    return len(text.split()) == 1 and text[:1].islower()


# every "stands for" question contains the word "stand", so it tells us nothing about
# whether two abbreviations come from the same subject
FRAME_MARKERS = {"stand", "stands", "abbreviation", "acronym", "letter", "letters",
                 "short", "mean", "means", "full", "form", "code", "codes", "initials"}


# classes whose members are ordinary nouns, so a borrowed one may be lower-cased to sit
# beside a lower-case answer ("Jalebi" next to "samosa" is a tell)
COMMON_NOUNS = {"food", "animal", "object", "colour", "element"}


# single-work or single-fandom files: prefer distractors from the same work
FANDOM_FILES = {"disney", "harry_potter", "indian_tv_ott", "music_tv", "mcu_movies",
                "game_of_thrones", "pokemon", "anime_comics", "anime_gaming", "tv_shows",
                "world_tv", "video_games", "hollywood"}


def class_of(word):
    """Class of a single question word, tolerating a plural."""
    if word in CLASS_OF_WORD:
        return CLASS_OF_WORD[word]
    if len(word) > 3 and word.endswith("s") and word[:-1] in CLASS_OF_WORD:
        return CLASS_OF_WORD[word[:-1]]
    return None


def compatible(a, b):
    if a == b:
        return True
    return any(a in group and b in group for group in COMPATIBLE)


INDIAN_ZONES = {
    "rajasthan": "north", "delhi": "north", "punjab": "north", "haryana": "north",
    "himachal": "north", "uttarakhand": "north", "uttar pradesh": "north", "kashmir": "north",
    "jammu": "north", "ladakh": "north", "chandigarh": "north",
    "gujarat": "west", "maharashtra": "west", "goa": "west", "madhya pradesh": "west",
    "chhattisgarh": "west",
    "kerala": "south", "tamil nadu": "south", "karnataka": "south", "telangana": "south",
    "andhra pradesh": "south", "puducherry": "south",
    "bihar": "east", "jharkhand": "east", "odisha": "east", "west bengal": "east",
    "assam": "northeast", "meghalaya": "northeast", "manipur": "northeast",
    "mizoram": "northeast", "nagaland": "northeast", "tripura": "northeast",
    "sikkim": "northeast", "arunachal pradesh": "northeast",
}
FILE_ZONES = {"north_india": "north", "south_india": "south", "west_central": "west",
              "east_northeast": "northeast"}

# question patterns that group questions better than their leading words do
PATTERNS = [
    ("stands_for", re.compile(r"\bstands? for\b|\babbreviation\b|\bacronym\b|\bfull form\b")),
    ("sequence", re.compile(r"\bcomes next\b|\bmissing number\b|\bnext in the\b|\bin the same way\b")),
    ("proverb", re.compile(r"\bcomplete the\b")),
    ("whatami", re.compile(r"\bwhat am i\b|\bwho am i\b")),
    ("paheli", re.compile(r"^\s*paheli\b")),
    ("capital", re.compile(r"\bcapital of\b")),
    ("nickname", re.compile(r"\bnicknamed?\b|\bpopularly known as\b|\bnickname\b")),
    ("which_year", re.compile(r"\bin which year\b|\bwhich year\b")),
    ("directed", re.compile(r"\bwho directed\b|\bdirected the\b")),
    ("wrote", re.compile(r"\bwho wrote\b|\bwrote the\b")),
    ("iata", re.compile(r"\biata code\b")),
    ("dialling", re.compile(r"\bdialling code\b|\bcalling code\b")),
    ("symbol", re.compile(r"\bthe symbol\b|\bchemical symbol\b")),
    ("currency", re.compile(r"\bcurrency of\b")),
    ("clock", re.compile(r"\bclock'?s? hands?\b|\bangle between\b")),
    ("weekday", re.compile(r"\bday of the week\b")),
]

# files where another puzzle's answer is not a plausible answer to *this* puzzle
PUZZLE_FILES = {"brain_teasers", "logical_reasoning"}
RIDDLE_FILES = {"riddles"}
# shapes rare enough inside one topic file that we let them borrow from other files
CROSS_FILE_SHAPES = {"acronym", "letter"}
# question families that mean the same thing in every topic, so they can borrow too
UNIVERSAL_PATTERNS = {"stands_for", "capital", "which_year", "sequence"}

DIFF_RANK = {"easy": 0, "medium": 1, "hard": 2}
MIN_TIER2_OVERLAP = 0.0   # tier-2 candidates rank by question overlap; no hard floor

# ---------------------------------------------------------------- normalisation

LATIN = re.compile(r"[A-Za-z]")


def strip_accents(text):
    """Fold Latin accents, but leave Devanagari alone - its vowel signs are combining marks."""
    if not LATIN.search(text):
        return text
    out = []
    for ch in unicodedata.normalize("NFD", text):
        if unicodedata.combining(ch) and LATIN.search(unicodedata.normalize("NFC", ch) or "x"):
            continue
        out.append(ch)
    folded = unicodedata.normalize("NFC", "".join(out))
    return "".join(c for c in unicodedata.normalize("NFD", folded)
                   if not unicodedata.combining(c) or ord(c) > 0x0500)


def norm(text):
    """Lower-case, drop punctuation, drop a leading the/a/an, collapse spaces."""
    s = strip_accents(str(text)).lower()
    s = re.sub(r"[^\w\s]", " ", s, flags=re.UNICODE)
    s = re.sub(r"\s+", " ", s).strip()
    return re.sub(r"^(?:the|a|an)\s+", "", s)


def tokens(text):
    return norm(text).split()


def content_tokens(text):
    return [t for t in tokens(text) if t not in STOPWORDS]


def case_style(text):
    letters = [c for c in text if c.isalpha()]
    if not letters:
        return "none"
    if all(c.isupper() for c in letters):
        return "upper"
    if text[:1].islower():
        return "lower"
    return "title"


def question_language(q):
    """Roman-script Hindi riddles announce themselves; keep them away from English ones."""
    if re.search(r"[ऀ-ॿ]", q):
        return "hi"
    if re.match(r"\s*paheli\b", q.strip(), re.I):
        return "hi"
    return "en"

# ---------------------------------------------------------------- answer shape

NUM_RE = r"\d{1,3}(?:,\d{2,3})*(?:\.\d+)?|\d+(?:\.\d+)?"


def parse_number(text):
    return float(text.replace(",", ""))


def fmt_number(value, like):
    """Render `value` the way `like` was written (commas, decimals, Indian grouping)."""
    decimals = len(like.split(".")[1]) if "." in like else 0
    if decimals and abs(value - round(value)) > 1e-9:
        whole, frac = f"{round(value, decimals):.{decimals}f}".split(".")
    else:
        whole, frac = str(int(round(value))), ""
    if "," in like:
        groups = like.split(".")[0].split(",")
        if len(groups) > 1 and len(groups[-1]) == 3 and any(len(g) == 2 for g in groups[1:-1]):
            whole = indian_commas(whole)          # 1,00,000
        else:
            whole = f"{int(whole):,}"             # 100,000
    return whole + ("." + frac if frac else "")


def indian_commas(digits):
    if len(digits) <= 3:
        return digits
    head, tail = digits[:-3], digits[-3:]
    parts = []
    while len(head) > 2:
        parts.insert(0, head[-2:])
        head = head[:-2]
    if head:
        parts.insert(0, head)
    return ",".join(parts + [tail])


def shape(answer):
    """A coarse 'same sort of thing' key: (family, detail).  Detail is compared loosely."""
    s = answer.strip()
    low = s.lower()
    if low in CLOSED_LOOKUP:
        return (CLOSED_LOOKUP[low], None)
    if re.fullmatch(r"(?:1[4-9]\d\d|20[0-3]\d)", s):
        return ("year", s[:2])
    m = re.fullmatch(r"([₹$£€])\s?(" + NUM_RE + r")", s)
    if m:
        return ("money", m.group(1))
    if re.fullmatch(r"(?:" + NUM_RE + r")\s?%", s):
        return ("percent", None)
    if re.fullmatch(r"\d+(?:st|nd|rd|th)", s):
        return ("ordinal", None)
    if re.fullmatch(r"\d{1,2}\s+(?:" + "|".join(MONTHS) + r")", s):
        return ("date", "dm")
    if re.fullmatch(r"(?:" + "|".join(MONTHS) + r")\s+\d{1,2}", s):
        return ("date", "md")
    m = re.fullmatch(r"(" + NUM_RE + r")\s*([A-Za-z°/%][\w°/%.]*)", s)
    if m and (m.group(2).lower() in UNIT_WORDS or m.group(2) in ("%", "°C", "°F")):
        return ("unit", m.group(2).lower())
    if re.fullmatch(NUM_RE, s):
        digits = len(s.split(".")[0].replace(",", ""))
        return ("number", "dec" if "." in s else digits)
    if re.fullmatch(r"[A-Za-z]", s):
        return ("letter", None)
    if re.fullmatch(r"[A-Z][A-Z0-9]{1,5}", s):
        return ("acronym", len(s))
    return ("words", len(s.split()))


def shapes_match(a_shape, c_shape, a_text, c_text):
    fam, detail = a_shape
    cfam, cdetail = c_shape
    if fam != cfam:
        return False
    if fam == "year":
        if detail != cdetail:                       # never cross a century
            return False
        return 0 < abs(int(a_text) - int(c_text)) <= 15
    if fam == "number":
        if detail == "dec" or cdetail == "dec":
            return detail == cdetail
        try:
            av, cv = parse_number(a_text), parse_number(c_text)
        except ValueError:
            return False
        if av <= 20 or cv <= 20:
            return abs(av - cv) <= 12 and cv > 0
        return detail == cdetail                    # same order of magnitude
    if fam in ("unit", "money"):
        return detail == cdetail
    if fam == "acronym":
        return abs(detail - cdetail) <= 1
    if fam == "words":
        return abs(detail - cdetail) <= 1
    return True

# ---------------------------------------------------------------- what is asked

LEAD_SKIP = {"is", "was", "are", "were", "the", "a", "an", "of", "this", "that", "these",
             "did", "does", "do", "in", "his", "her", "their", "its"}
LEAD_CLAUSE = re.compile(r"^(?:in|on|at|during|for|from|among|under)\s+[^,]{2,40},\s*", re.I)


def template_sig(q, n):
    """Leading words, after dropping an 'In <show>,' style opener that groups everything."""
    return " ".join(tokens(LEAD_CLAUSE.sub("", q.strip()))[:n])


def pattern_of(q):
    low = norm(q)
    for name, rx in PATTERNS:
        if rx.search(low):
            return name
    return None


def asked_for(q):
    """The noun the question wants: 'who', 'city', 'year', 'how many', ..."""
    words = tokens(q)
    for i, w in enumerate(words):
        if w in ("who", "whom", "whose"):
            return "who"
        if w == "how" and i + 1 < len(words):
            return "how " + words[i + 1]
        if w in ("which", "what") and i + 1 < len(words):
            j = i + 1
            while j < len(words) and (words[j] in LEAD_SKIP or words[j] in ADJECTIVES):
                j += 1
            if j < len(words):
                return words[j]
    return None


def question_class(q):
    """What sort of thing the question wants.

    'Which Sholay villain ...' wants a character even though 'Sholay' comes first;
    "what does X sell" wants goods, not a person; "share which first name" wants a name.
    """
    words = tokens(q)
    text = " ".join(words)
    if re.search(r"\b(?:first|last|full|middle|sur) ?name\b", text):
        return "name"
    if re.search(r"\bwhat (?:sends|makes|opens|powers|stops|cures|kills|wakes)\b", text):
        return "object"                              # "what sends the dog to sleep?"
    if re.search(r"\bruns? .*\bfor a living\b", text):
        return "business"                            # what Sodhi runs is a garage, not a job
    if re.search(r"\bprofession\b|\boccupation\b|\bfor a living\b", text):
        return "role"
    m = re.search(r"\bname of (?:the |his |her |their )?(\w+)", text)
    if m and class_of(m.group(1)):
        return class_of(m.group(1))
    # "which group wears black cloaks" is not a question about clothing, so only the
    # selling verbs are read as asking for goods
    if re.search(r"\b(?:sells?|selling)\b", text) and not re.search(r"\bwho\b", text):
        return "object"
    for i, w in enumerate(words):
        if w in ("who", "whom", "whose"):
            return "person"
        if w in ("which", "what"):
            for nxt in words[i + 1:i + 11]:           # "the first Walt Disney ... film"
                if class_of(nxt):
                    return class_of(nxt)
        if w == "how" and i + 1 < len(words) and class_of(words[i + 1]):
            return class_of(words[i + 1])
    return None


CAP_PHRASE = re.compile(
    r"\b([A-Z][\w'’!.-]*(?:\s+(?:of|the|ka|ki|aur|and|in|de|di)\s+[A-Z][\w'’!.-]*|\s+[A-Z][\w'’!.-]*){1,4})")
PHRASE_SKIP = {"which", "what", "who", "whose", "when", "where", "how", "the", "a", "an",
               "in", "on", "at", "this", "that", "if", "before", "after", "during",
               "complete", "name", "his", "her", "their", "its", "and", "but", "for"}


def learn_works(rows):
    """Titles that several questions talk about, e.g. 'Taarak Mehta Ka Ooltah Chashmah'."""
    seen = defaultdict(set)
    for row in rows:
        for m in CAP_PHRASE.finditer(row["q"]):
            phrase = m.group(1).strip(" '’")
            parts = phrase.split()
            while parts and parts[0].lower() in PHRASE_SKIP:
                parts.pop(0)
            if len(parts) < 2:
                continue
            seen[" ".join(parts)].add(row["id"])
    return {p for p, ids in seen.items() if len(ids) >= 3}


def work_of(q, works):
    """The longest known title this question talks about."""
    best = None
    for phrase in works:
        if phrase in q and (best is None or len(phrase) > len(best)):
            best = phrase
    return best


def learn_places(rows, path_stems, skip_options=()):
    """Which Indian zone each place answer belongs to, from the questions around it.

    A state named in the question is strong evidence; the file a row lives in is weak
    evidence and counts only for that row's own answer.  The options of a row this tool
    wrote count for nothing - they are what put Kolkata in the north-east.
    """
    votes = defaultdict(Counter)
    for row, stem in zip(rows, path_stems):
        named = {INDIAN_ZONES[s] for s in INDIAN_ZONES if s in norm(row["q"])}
        strong = next(iter(named)) if len(named) == 1 else None
        weak = FILE_ZONES.get(stem)
        if strong:
            votes[norm(row["a"])][strong] += 3
        elif weak:
            votes[norm(row["a"])][weak] += 1
        if strong and row["id"] not in skip_options:
            for opt in row.get("options") or []:
                votes[norm(opt)][strong] += 2
    out = {}
    for key, counter in votes.items():
        zone, n = counter.most_common(1)[0]
        if n >= 2:                                    # one weak guess is not enough
            out[key] = zone
    return out


class Lexicon:
    """What sort of thing each answer string is, learnt from the questions that ask for it."""

    def __init__(self, rows, skip_options=()):
        votes = defaultdict(Counter)
        for row in rows:
            cls = question_class(row["q"])
            if not cls:
                continue
            # a title answer is unambiguous, so one question is enough to type it
            weight = 3 if cls in ("film", "show", "song", "book", "title") else 2
            votes[norm(row["a"])][cls] += weight     # the answer itself is the strong signal
            if row["id"] in skip_options:
                continue    # options this tool wrote are the noise we are here to fix,
                            # and learning from them taught the bank that Tangled is a character
            for opt in row.get("options") or []:     # its siblings are the same sort of thing
                if opt != row["a"]:
                    votes[norm(opt)][cls] += 1
        # one stray question must not type a string: a single "Who is the first Pokemon..."
        # was enough to call Pidgey a person and stand it next to Brock.
        self.votes = votes
        self.table = {}
        for key, counter in votes.items():
            cls, n = counter.most_common(1)[0]
            # one clean answer-vote (weight 2) is enough now that the options this tool
            # wrote are excluded; the majority test still throws out contested strings
            if n >= 2 and n >= 0.6 * sum(counter.values()):
                self.table[key] = cls

    def klass(self, text):
        return self.table.get(norm(text))

    def any_class(self, text):
        """Every class ever seen for this string, however weakly."""
        return set(self.votes.get(norm(text), ()))

    def conflict(self, a_text, c_text, a_cls=None):
        """True when both sides are typed and typed differently."""
        ca = a_cls or self.klass(a_text)
        cc = self.klass(c_text)
        return bool(ca and cc and not compatible(ca, cc))

# ---------------------------------------------------------------- generators


def gen_year(answer, rng):
    year = int(answer)
    # never offer this year or a future one as a wrong answer to a historical question
    century, cap = year // 100, min(2025, year + 12) if year <= 2025 else year + 5
    out = [str(year + d) for d in (-11, -7, -5, -3, -2, 2, 3, 4, 6, 8, 12, -14, 14)
           if (year + d) // 100 == century and 1400 <= year + d <= cap]
    rng.shuffle(out)
    return spread(out, answer, int)


def gen_number(answer, rng):
    value = parse_number(answer)
    outs = []
    if value <= 12 and value == int(value):
        outs = [value + d for d in (-3, -2, -1, 1, 2, 3, 4)]
    else:
        for pct in (0.08, 0.15, 0.25, 0.4):
            for sign in (1, -1):
                cand = value * (1 + sign * pct)
                step = 10 ** max(0, len(str(int(value))) - 2)
                outs.append(round(cand / step) * step if step > 1 else round(cand))
        if value <= 30:                              # a one-off near miss only for small counts
            outs += [value + 1, value - 1, value * 2]
    seen, clean = set(), []
    digits = len(answer.split(".")[0].replace(",", ""))
    for v in outs:
        if v <= 0 or v == value:
            continue
        text = fmt_number(v, answer)
        if text in seen or text == answer:
            continue
        if value > 20 and len(text.split(".")[0].replace(",", "")) != digits:
            continue
        seen.add(text)
        clean.append(text)
    rng.shuffle(clean)
    return spread(clean, answer, parse_number)


def gen_money(answer, rng):
    m = re.fullmatch(r"(?P<sym>[₹$£€])\s?(?P<num>" + NUM_RE + r")", answer.strip())
    if not m:
        return []
    return [f"{m.group('sym')}{t}" for t in gen_number(m.group("num"), rng)]


def gen_percent(answer, rng):
    m = re.fullmatch(r"(?P<num>" + NUM_RE + r")\s?%", answer.strip())
    if not m:
        return []
    value = parse_number(m.group("num"))
    outs = [fmt_number(value + d, m.group("num")) + "%"
            for d in (-40, -25, -20, -10, -5, 5, 10, 20, 25, 50) if 0 < value + d <= 100]
    rng.shuffle(outs)
    return spread(outs, answer, lambda s: parse_number(s[:-1]))


def gen_unit(answer, rng):
    m = re.fullmatch(r"(?P<num>" + NUM_RE + r")(?P<gap>\s*)(?P<unit>[A-Za-z°/%][\w°/%.]*)", answer.strip())
    if not m:
        return []
    outs = []
    for text in gen_number(m.group("num"), rng):
        if parse_number(text) == 1 and parse_number(m.group("num")) != 1:
            continue                                 # dodge "1 minutes"
        outs.append(f"{text}{m.group('gap')}{m.group('unit')}")
    return outs


def ordinal_suffix(n):
    if 10 <= n % 100 <= 20:
        return "th"
    return {1: "st", 2: "nd", 3: "rd"}.get(n % 10, "th")


def gen_ordinal(answer, rng):
    value = int(re.match(r"\d+", answer).group())
    outs = [f"{value + d}{ordinal_suffix(value + d)}" for d in (-5, -3, -2, -1, 1, 2, 3, 4, 6)
            if value + d > 0]
    rng.shuffle(outs)
    return spread(outs, answer, lambda s: int(re.match(r"\d+", s).group()))


def gen_date(answer, rng):
    if shape(answer)[1] == "dm":
        day, month = answer.split()
        build = lambda d, mo: f"{d} {mo}"
    else:
        month, day = answer.split()
        build = lambda d, mo: f"{mo} {d}"
    day = int(day)
    outs = [build(day + d, month) for d in (-9, -6, -4, -2, 2, 3, 5, 8, 11)
            if 1 <= day + d <= MONTH_LEN[month]]
    others = [m for m in MONTHS if m != month]
    rng.shuffle(others)
    outs += [build(day, mo) for mo in others[:3] if day <= MONTH_LEN[mo]]
    rng.shuffle(outs)
    return outs


def gen_letter(answer, rng):
    idx = ord(answer.upper()) - 65
    outs = [chr(65 + idx + d) if answer.isupper() else chr(97 + idx + d)
            for d in (-5, -3, -2, -1, 1, 2, 3, 5, 7) if 0 <= idx + d < 26]
    rng.shuffle(outs)
    return outs


def gen_closed(answer, rng):
    family = CLOSED_LOOKUP.get(answer.strip().lower())
    if not family:
        return []
    if family == "weekday":                          # neighbouring days read as real guesses
        i = [w.lower() for w in WEEKDAYS].index(answer.strip().lower())
        words = [WEEKDAYS[(i + d) % 7] for d in (1, 6, 2, 5, 3, 4)]
    else:
        words = [w for w in CLOSED_CLASSES[family] if w.lower() != answer.strip().lower()]
    rng.shuffle(words)
    return words


def spread(candidates, answer, value_of):
    """Interleave above and below the answer, so the answer is not the odd one out."""
    try:
        target = value_of(answer)
        below = [c for c in candidates if value_of(c) < target]
        above = [c for c in candidates if value_of(c) > target]
    except Exception:
        return candidates
    out, i = [], 0
    while below or above:
        pool = below if ((i % 2 == 0 and below) or not above) else above
        out.append(pool.pop(0))
        i += 1
    return out


GENERATORS = {
    "year": gen_year, "number": gen_number, "money": gen_money, "percent": gen_percent,
    "unit": gen_unit, "ordinal": gen_ordinal, "date": gen_date, "letter": gen_letter,
    "weekday": gen_closed, "month": gen_closed, "direction": gen_closed, "position": gen_closed,
}

# ---------------------------------------------------------------- bank loading


# Options give a riddle away, so these files keep whatever the writers chose and
# are never converted, in any mode.
NEVER_CONVERT = {"riddles", "brain_teasers"}


def bank_files(which=REWRITE):
    out = []
    for src in which:
        out += sorted(p for p in (BANK / src).glob("*.jsonl") if p.stem not in NEVER_CONVERT)
    return out


def read_rows(path):
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


def load_dropped():
    data = json.loads((BANK / "dropped.json").read_text())
    return {qid for ids in data.values() for qid in ids}, set(data.get("broken", []))

# ---------------------------------------------------------------- candidates


class Facts:
    """Everything we need to know about a row, worked out once."""
    __slots__ = ("row", "sig5", "sig4", "pattern", "asked", "cls", "ctoks", "lang", "stem",
                 "work", "zone", "frame")

    def __init__(self, row, lex, stem, works=(), places=None):
        self.row = row
        self.stem = stem
        self.work = work_of(row["q"], works)
        self.zone = (places or {}).get(norm(row["a"]))
        self.frame = frame_of(row)
        self.sig5 = template_sig(row["q"], 5)
        self.sig4 = template_sig(row["q"], 4)
        self.pattern = pattern_of(row["q"])
        self.asked = asked_for(row["q"])
        self.cls = question_class(row["q"]) or lex.klass(row["a"])
        self.ctoks = set(content_tokens(row["q"]))
        self.lang = question_language(row["q"])


class Candidate:
    __slots__ = ("text", "facts", "from_options")

    def __init__(self, text, facts, from_options):
        self.text = text
        self.facts = facts
        self.from_options = from_options


class Index:
    """Bank-wide shortlists, for the rows whose own file cannot fill three options."""

    def __init__(self):
        self.by_class = defaultdict(list)
        self.by_pattern = defaultdict(list)
        self.by_shape = defaultdict(list)
        self.by_work = defaultdict(list)

    def add(self, cand):
        if cand.facts.cls:
            self.by_class[cand.facts.cls].append(cand)
        if cand.facts.pattern in UNIVERSAL_PATTERNS:
            self.by_pattern[cand.facts.pattern].append(cand)
        fam = shape(cand.text)[0]
        if fam in CROSS_FILE_SHAPES:
            self.by_shape[fam].append(cand)
        if cand.facts.work:
            self.by_work[cand.facts.work].append(cand)

    def shortlist(self, me, a_shape):
        """Only things that are the same sort of thing, whichever file they live in."""
        out = []
        if me.work:
            out += self.by_work[me.work]         # the same show, even in another file
        if me.cls:
            out += self.by_class[me.cls]
        if me.pattern in UNIVERSAL_PATTERNS:
            out += self.by_pattern[me.pattern]
        if a_shape[0] in CROSS_FILE_SHAPES:
            out += self.by_shape[a_shape[0]]
        return out


def overlap(a, b):
    if not a or not b:
        return 0.0
    return len(a & b) / len(a | b)


def rejects(text, row, taken, qtokens, banned):
    """Every hard rule a wrong option has to pass."""
    text = text.strip()
    if not text or len(text) > MAX_OPTION_CHARS:
        return "length"
    if len(text) >= 3 and text.isalpha() and len(set(text.upper())) < 2:
        return "junk"                                 # 'IIII' from a roman-numeral puzzle
    key = norm(text)
    if not key or key in banned or key in taken:
        return "duplicate"                            # the answer, an alt, or an option we took
    answer_key = norm(row["a"])
    if key in answer_key or answer_key in key:        # 'Amitabh' beside 'Amitabh Bachchan'
        return "contains"
    ctoks = content_tokens(text)
    if ctoks and all(t in qtokens for t in ctoks):
        return "in question"
    if not ctoks and key in qtokens:
        return "in question"
    a_len, c_len = len(row["a"]), len(text)
    if c_len > max(1.7 * a_len + 6, a_len + 8) or c_len < min(0.45 * a_len - 2, a_len - 6):
        return "length"
    if abs(len(text.split()) - len(row["a"].split())) > 1:
        return "length"
    return None


def specific(sig):
    """'what is the capital' groups a real family; 'what is the name' groups everything."""
    return any(t not in STOPWORDS for t in sig.split())


def tier_of(me, other):
    if other.pattern and me.pattern == other.pattern:
        return 0
    if (me.sig5 == other.sig5 and specific(me.sig5)) or (me.sig4 == other.sig4 and specific(me.sig4)):
        return 0
    if me.asked and me.asked == other.asked:
        return 1
    if me.cls and other.cls and compatible(me.cls, other.cls):
        return 2
    if me.cls is None and other.cls is None:
        return 2
    return None


def score(cand, me, tier, a_shape, rng):
    """Lower is better."""
    s = tier * 1000.0
    s -= 120 * overlap(me.ctoks, cand.facts.ctoks)    # same corner of the topic
    if cand.from_options and tier == 0:
        s -= 60                                       # written as a sibling of this shape
    if case_style(cand.text) != case_style(me.row["a"]):
        s += 45
    s += 25 * abs(len(cand.text.split()) - len(me.row["a"].split()))
    s += 1.5 * abs(len(cand.text) - len(me.row["a"]))
    s += 30 * abs(DIFF_RANK.get(cand.facts.row.get("diff"), 1) - DIFF_RANK.get(me.row.get("diff"), 1))
    if cand.facts.lang != me.lang:
        s += 400
    if cand.facts.stem != me.stem:
        s += 150                                      # borrowing from another file
    if me.work and cand.facts.work == me.work:
        s -= 200                                      # the same show as this question
    if me.cls in ("city", "state", "place") and me.zone and cand.facts.zone:
        s += -120 if cand.facts.zone == me.zone else 150  # keep Indian places in one corner
    if a_shape[0] in ("number", "year", "unit", "money"):
        try:
            av = parse_number(re.search(NUM_RE, me.row["a"]).group())
            cv = parse_number(re.search(NUM_RE, cand.text).group())
            s += 20 * min(4.0, abs(av - cv) / max(1.0, abs(av)))
        except (AttributeError, ValueError):
            pass
    return s + rng.random() * 8


def build_options(me, ctx, rng):
    """Return (options, how) or (None, why-it-stayed-text)."""
    row = me.row
    answer = row["a"]
    a_shape = shape(answer)
    lex = ctx["lex"]
    banned = {norm(answer)} | {norm(x) for x in row.get("alt") or []}
    qtokens = set(tokens(row["q"]))
    picked, taken, how = [], set(), []

    def take(text, label):
        if rejects(text, row, taken, qtokens, banned):
            return
        picked.append(text)
        taken.add(norm(text))
        how.append(label)

    # a question that offers its own two choices cannot be padded out honestly
    if re.search(r"\bor\b", row["q"]) and re.search(
            r"\b" + re.escape(answer.lower()) + r"\b", row["q"].lower()):
        return None, "question offers its own alternatives ('X or Y')"

    generator = GENERATORS.get(a_shape[0])
    if generator:
        for text in generator(answer, rng):
            if len(picked) == 3:
                break
            take(text, "gen")

    def collect(pool, cross):
        out = []
        for cand in pool:
            other = cand.facts
            if other.row["id"] == row["id"]:
                continue
            if cross:
                if other.stem == me.stem or other.row["region"] != row["region"]:
                    continue
                if other.stem in PUZZLE_FILES or other.stem in RIDDLE_FILES:
                    continue                          # a teaser's answer is not a real thing
                # a universal family ("... stand for?") travels better than a same-shape
                # neighbour from this file, so let it compete before tier 2 does
                tier = 1 if me.pattern in UNIVERSAL_PATTERNS and other.pattern == me.pattern else 3
            else:
                tier = tier_of(me, other)
                if tier is None:
                    continue
            if not shapes_match(a_shape, shape(cand.text), answer, cand.text):
                continue
            if lex.conflict(answer, cand.text, me.cls):
                continue                              # a city is not a state, a film is not an actor
            cand_cls = lex.klass(cand.text) or other.cls
            if me.cls in STRICT_CLASSES and not (cand_cls and compatible(me.cls, cand_cls)):
                continue                              # a dwarf question takes only characters
            if me.stem in FANDOM_FILES and me.cls is None and cand_cls:
                continue                              # a thing must not draw people and places
            if (tier >= 2 or case_style(answer) == "lower") and case_style(cand.text) != case_style(answer):
                continue                              # 'kotwal' must not sit beside 'Ghanti'
            if me.stem in PUZZLE_FILES and tier >= 1 and not generator:
                continue                              # another teaser's answer is noise here
            if me.stem in RIDDLE_FILES and other.lang != me.lang:
                continue                              # keep pahelis away from English riddles
            pools = ctx["pools"]
            if me.frame == "blank":
                # only a real ending of another line in this same file can finish this one
                if (other.frame != "blank" or cand.from_options
                        or other.lang != me.lang or other.stem != me.stem):
                    continue
            elif me.frame == "expansion":
                # an abbreviation from the same subject beats one from across the bank:
                # ACP belongs beside other railway codes, not beside "Jan to Mar"
                local = pools["expansions_file"].get(me.stem, set())
                pool_ok = local if len(local) >= 4 else pools["expansions"]
                if norm(cand.text) not in pool_ok:
                    continue
                if len(cand.text.split()) != len(answer.split()):
                    continue                          # an expansion has the same shape
                if not ((me.ctoks - FRAME_MARKERS) & (other.ctoks - FRAME_MARKERS)):
                    continue                          # ... and comes from the same subject
            elif me.frame == "plainword":
                if (case_style(answer) == "lower" and cand_cls in COMMON_NOUNS
                        and len(cand.text.split()) == 1 and cand.text[:1].isupper()):
                    cand = Candidate(cand.text.lower(), cand.facts, cand.from_options)
                if not plain_word(cand.text) or other.frame == "blank":
                    continue                          # a slogan ending is not a category
                if me.cls and not (cand_cls and compatible(me.cls, cand_cls)):
                    continue                          # a snack question takes only snacks
                if not me.cls and tier >= 2:
                    continue                          # no category to match: same frame only
            if tier == 2 and overlap(me.ctoks, other.ctoks) < MIN_TIER2_OVERLAP:
                continue
            # a distractor the file has already leaned on is a tell: every Hindi proverb
            # was being finished with the same gaana / ghanti / damru
            penalty = 90 * ctx["used"][norm(cand.text)]
            out.append((score(cand, me, tier, a_shape, rng) + penalty, tier, cand))
        out.sort(key=lambda x: x[0])
        return out

    scored = collect(ctx["pool"], False)
    if me.stem in FANDOM_FILES and me.work:           # keep one show's answers together
        same = [x for x in scored if x[2].facts.work == me.work]
        same += collect([c for c in ctx["index"].by_work[me.work] if c.facts.stem != me.stem], True)
        same.sort(key=lambda x: x[0])
        # never leave the show for a distractor: a profession question answered with
        # "Yorkshire" is worse than a question the player has to type
        scored = same
    if me.zone and me.cls in ("city", "state", "place"):
        near = [x for x in scored if x[2].facts.zone == me.zone]
        if len(near) >= 3:
            scored = near                             # Jaipur draws other northern cities
    if me.pattern in UNIVERSAL_PATTERNS:              # let the family compete from any file
        scored = sorted(scored + collect(ctx["index"].shortlist(me, a_shape), True),
                        key=lambda x: x[0])
    for _, tier, cand in scored:
        if len(picked) == 3:
            break
        take(cand.text, f"t{tier}")

    if len(picked) < 3 and not (me.stem in FANDOM_FILES and me.work):
        for _, tier, cand in collect(ctx["index"].shortlist(me, a_shape), True):
            if len(picked) == 3:
                break
            take(cand.text, f"t{tier}")

    if len(picked) < 3:
        return None, why_failed(me, a_shape, generator)
    for text in picked:
        ctx["used"][norm(text)] += 1
    options = picked + [answer]
    rng.shuffle(options)                              # no fixed slot for the answer
    return options, "+".join(how)


def why_failed(me, a_shape, generator):
    answer = me.row["a"]
    if answer in set(re.findall(r"\b[A-Z][a-z]{2,}\b", me.row["q"])):
        return "answer is a name from its own puzzle (the only sensible options are the other names in it)"
    if me.stem in PUZZLE_FILES:
        return "one-off trick answer with no sibling of the same shape"
    if a_shape[0] == "acronym":
        return "no three comparable acronyms in the bank"
    if a_shape[0] == "words" and a_shape[1] >= 3:
        return "long phrase answer with no same-shape sibling"
    return f"fewer than three candidates of shape {a_shape[0]}"

# ---------------------------------------------------------------- driving


def tool_converted_ids():
    """Rows this tool turned from text into mcq, read from the last commit.

    The bank's own mcq rows have hand-written options and must never be rebuilt.
    """
    repo = BANK.parent
    out = set()
    for path in bank_files():
        rel = path.relative_to(repo)
        blob = subprocess.run(["git", "show", f"HEAD:{rel}"], cwd=repo,
                              capture_output=True, text=True)
        if blob.returncode:
            raise SystemExit(f"--repass needs the committed bank as a baseline ({rel} not in HEAD)")
        for line in blob.stdout.splitlines():
            if line.strip():
                was = json.loads(line)
                if was["kind"] == "text":
                    out.add(was["id"])
    return out


def suspect(row, me, lex, works_by_text, places):
    """Why a converted row needs rebuilding under the current rules, or None.

    Deliberately conservative.  An option that is simply untyped is left alone - most
    good distractors in the fandom files are untyped, and rebuilding them churns rows
    that are already fine.  A row is only rebuilt when there is positive evidence that
    an option is a *different sort of thing* than the question asks for: a class it has
    been seen with elsewhere, a different work, or a different part of the country.
    """
    for opt in [o for o in (row.get("options") or []) if o != row["a"]]:
        cls = lex.klass(opt)
        # only the settled class counts here: every stray vote ("Captain Haddock is a
        # place", "Getafix is an object") was flagging rows whose options were fine
        if me.cls and cls and not compatible(me.cls, cls):
            return f"{opt!r} is a {cls}, the question asks for a {me.cls}"
        if me.stem in FANDOM_FILES and me.work:
            elsewhere = works_by_text.get(norm(opt), set())
            if elsewhere and me.work not in elsewhere:
                return f"{opt!r} belongs to {sorted(elsewhere)[0]}, not {me.work}"
        if me.zone and me.cls in ("city", "state", "place"):
            zone = places.get(norm(opt))
            if zone and zone != me.zone:
                return f"{opt!r} is in the {zone}, but the answer is in the {me.zone}"
    return None


def suspect_shape(row, me, lex, pools):
    """Why one of the three giveaway-prone rows needs rebuilding, or None.

    These shapes are judged harder than the rest of the bank: an option that is not the
    same kind of thing hands the answer over, so "no option of the right kind" sends the
    row back to being typed rather than leaving a giveaway in place.
    """
    opts = [o for o in (row.get("options") or []) if o != row["a"]]
    all_endings = set().union(*pools["endings"].values()) if pools["endings"] else set()
    if me.frame == "blank":
        # a line is finished by other lines from its own file: the Cadbury slogan wants
        # the other Hindi slogans, not "Delicious" from an English one
        local = pools["endings_file"].get(me.stem, set())
        for opt in opts:
            if norm(opt) not in local:
                return f"{opt!r} does not finish another line in this file"
    elif me.frame == "expansion":
        local = pools["expansions_file"].get(me.stem, set())
        pool_ok = local if len(local) >= 4 else pools["expansions"]
        for opt in opts:
            if norm(opt) not in pool_ok:
                return f"{opt!r} does not expand an abbreviation from this topic"
            if len(opt.split()) != len(row["a"].split()):
                return f"{opt!r} is not shaped like the expansion {row['a']!r}"
            if not ((me.ctoks - FRAME_MARKERS)
                    & (pools["exp_ctoks"].get(norm(opt), set()) - FRAME_MARKERS)):
                return f"{opt!r} expands an abbreviation from a different subject"
    elif me.frame == "plainword":
        for opt in opts:
            if not plain_word(opt):
                return f"{opt!r} is not a plain one-word answer"
            if norm(opt) in all_endings:
                return f"{opt!r} is the end of a slogan, not a kind of thing"
            cls = lex.klass(opt)
            if me.cls and not (cls and compatible(me.cls, cls)):
                return f"{opt!r} is not a known {me.cls}"
            if not me.cls and me.asked and me.asked not in pools["asked_by"].get(norm(opt), ()):
                return f"{opt!r} answers a different kind of question"
    return None


def convert(preview_n=0, write=False, preview_path=None, seed=7, repass=False, shapes=False):
    dropped, broken = load_dropped()
    mine = tool_converted_ids() if (repass or shapes) else set()
    all_rows, all_stems = [], []
    for path in bank_files(REWRITE + READ_ONLY):
        rows = read_rows(path)
        all_rows += rows
        all_stems += [path.stem] * len(rows)
    lex = Lexicon(all_rows, skip_options=mine)
    works = learn_works(all_rows)
    places = learn_places(all_rows, all_stems, skip_options=mine)
    print(f"answer-type lexicon: {len(lex.table)} strings typed from {len(all_rows)} rows; "
          f"{len(works)} works, {len(places)} placed strings")

    works_by_text = defaultdict(set)
    for row in all_rows:
        work = work_of(row["q"], works)
        if work:
            for text in [row["a"]] + list(row.get("options") or []):
                works_by_text[norm(text)].add(work)

    # what can legitimately finish a line, expand an abbreviation, or answer a given
    # kind of question.  Options this tool wrote are excluded, as everywhere else.
    pools = {"endings": defaultdict(set), "endings_file": defaultdict(set),
             "expansions": set(), "expansions_file": defaultdict(set),
             "asked_by": defaultdict(set), "exp_ctoks": defaultdict(set)}
    for row, stem in zip(all_rows, all_stems):
        frame = frame_of(row)
        if frame == "blank":
            pools["endings"][question_language(row["q"])].add(norm(row["a"]))
            pools["endings_file"][stem].add(norm(row["a"]))
        elif frame == "expansion":
            pools["expansions"].add(norm(row["a"]))
            pools["expansions_file"][stem].add(norm(row["a"]))
            pools["exp_ctoks"][norm(row["a"])] |= set(content_tokens(row["q"]))
            if row["id"] not in mine:
                for opt in row.get("options") or []:
                    pools["expansions"].add(norm(opt))
                    pools["expansions_file"][stem].add(norm(opt))
                    pools["exp_ctoks"][norm(opt)] |= set(content_tokens(row["q"]))
        asked = asked_for(row["q"])
        if asked:
            pools["asked_by"][norm(row["a"])].add(asked)
    print(f"{sum(len(v) for v in pools['endings'].values())} line endings, "
          f"{len(pools['expansions'])} expansions")

    stats, failures, examples, how_counter = {}, [], defaultdict(list), Counter()
    rebuilt, index, loaded, used = [], Index(), [], Counter()

    for path in bank_files():
        rows = read_rows(path)
        facts = {r["id"]: Facts(r, lex, path.stem, works, places) for r in rows}
        loaded.append((path, rows, facts))

    reverted = set()
    if repass or shapes:                              # only rows this tool got wrong
        for path, rows, facts in loaded:
            for row in rows:
                if row["kind"] != "mcq" or row["id"] not in mine or row["id"] in NEVER_TOUCH:
                    continue
                me = facts[row["id"]]
                if shapes:
                    why = suspect_shape(row, me, lex, pools) if me.frame else None
                else:
                    why = suspect(row, me, lex, works_by_text, places)
                if why:
                    rebuilt.append((path, dict(row), why))
                    reverted.add(row["id"])
                    row["kind"], row["options"] = "text", []
        print(f"flagged {len(reverted)} converted rows to rebuild")

    with_pools = []                                   # pools reflect the reverted rows
    for path, rows, facts in loaded:
        pool = []
        for r in rows:
            if r["id"] in broken:
                continue                              # a known-wrong row is a bad source
            pool.append(Candidate(r["a"], facts[r["id"]], False))
            pool += [Candidate(o, facts[r["id"]], True) for o in (r.get("options") or []) if o != r["a"]]
        for cand in pool:
            index.add(cand)
        with_pools.append((path, rows, facts, pool))

    for path, rows, facts, pool in with_pools:
        ctx = {"pool": pool, "index": index, "lex": lex, "pools": pools, "used": used}
        done = skipped = 0
        for row in rows:
            if row["kind"] != "text" or row["id"] in dropped:
                continue
            rng = random.Random(f"{seed}:{row['id']}")
            options, how = build_options(facts[row["id"]], ctx, rng)
            if options is None:
                failures.append((path, row, how))
                skipped += 1
                continue
            done += 1
            how_counter[how.split("+")[0]] += 1
            examples[path].append((row, options, how))
            if write:
                row["kind"] = "mcq"
                row["options"] = options
        stats[path] = (done, skipped)
        if write and (done or any(r["id"] in reverted for r in rows)):
            path.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))

    if preview_n:
        write_preview(examples, preview_path, preview_n)
    report(stats, failures, how_counter, write)
    still_text = set()
    if repass:
        still_text = reverted & {r["id"] for _, r, _ in failures}
        print(f"\nrepass: {len(reverted)} flagged, {len(reverted) - len(still_text)} rebuilt, "
              f"{len(still_text)} moved back to typed")
        print("why they were flagged:")
        for reason, n in Counter(w.split("'")[-1].strip() or w for _, _, w in rebuilt).most_common(8):
            print(f"  {n:>4}  ...{reason}")
    return stats, failures, rebuilt, still_text


def write_preview(examples, path, want):
    """`want` examples spread round-robin over every file that converted anything."""
    picked, cursors = [], {p: 0 for p in examples}
    files = [p for p in examples if examples[p]]
    while len(picked) < want and files:
        for p in list(files):
            rows = examples[p]
            i = cursors[p] * max(1, len(rows) // 4)
            if i >= len(rows):
                files.remove(p)
                continue
            if len(picked) >= want:
                break
            picked.append((p, rows[i]))
            cursors[p] += 1
    picked.sort(key=lambda x: str(x[0]))
    out = [f"Preview: {len(picked)} converted rows, no files changed."]
    current = None
    for p, (row, options, how) in picked:
        rel = p.relative_to(BANK)
        if rel != current:
            out += [f"\n{'=' * 78}", str(rel), "=" * 78]
            current = rel
        out += [f"\n[{row['id']}]  ({how}, {row.get('diff')})", f"Q: {row['q']}"]
        out += [f"   {'>' if o == row['a'] else ' '} {o}" for o in options]
    Path(path).write_text("\n".join(out) + "\n")
    print(f"wrote {len(picked)} examples to {path}")


def report(stats, failures, how_counter, wrote):
    print(f"\n{'file':<40} {'converted':>9} {'left text':>10}")
    total_done = total_skip = 0
    for path, (done, skipped) in stats.items():
        if done or skipped:
            print(f"{str(path.relative_to(BANK)):<40} {done:>9} {skipped:>10}")
        total_done, total_skip = total_done + done, total_skip + skipped
    print(f"{'TOTAL':<40} {total_done:>9} {total_skip:>10}   ({'written' if wrote else 'preview only'})")
    print("\nwhere the first option came from:", dict(how_counter.most_common()))
    if failures:
        print("\nleft as text:")
        for reason, n in Counter(r for _, _, r in failures).most_common():
            print(f"  {n:>4}  {reason}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--preview", action="store_true", help="show examples, change nothing")
    ap.add_argument("--write", action="store_true", help="rewrite the jsonl files in place")
    ap.add_argument("--n", type=int, default=80, help="how many preview examples")
    ap.add_argument("--out", default="/tmp/mcq_preview.txt")
    ap.add_argument("--seed", type=int, default=7)
    ap.add_argument("--repass", action="store_true",
                    help="rebuild only the rows this tool converted badly, leaving good ones alone")
    ap.add_argument("--shapes", action="store_true",
                    help="rebuild only the fill-in-the-blank, stands-for and plain-word rows")
    args = ap.parse_args()
    if not (args.preview or args.write):
        ap.error("pass --preview or --write")
    convert(preview_n=args.n if args.preview else 0, write=args.write,
            preview_path=args.out, seed=args.seed, repass=args.repass, shapes=args.shapes)


if __name__ == "__main__":
    main()
