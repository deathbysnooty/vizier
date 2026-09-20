"""The one way a title or a guess is reduced before anything is compared.

This mirrors `plain`, `fold` and `key` in `src/channels/discord/movie_bank.rs`
exactly. If you change one, change the other, or the checker will bless a bank
the bot cannot play.
"""

import re

NUMBERS = {
    "one": "1", "two": "2", "three": "3", "four": "4", "five": "5",
    "six": "6", "seven": "7", "eight": "8", "nine": "9", "ten": "10",
    # Roman numerals, which is how a sequel usually writes itself. Never the
    # single letters: "V for Vendetta" and "I Am Legend" are titles, not numbers.
    "ii": "2", "iii": "3", "iv": "4", "vi": "6", "vii": "7", "viii": "8",
    "ix": "9", "xi": "11", "xii": "12", "xiii": "13",
}
# Joining words a title can lose without becoming a different title.
STOPWORDS = {"of", "the", "a", "an", "and", "aur", "ki", "ke", "ka", "na", "in", "to"}


def numbers(text: str) -> str:
    """"three idiots" and "3 idiots" are the same film."""
    return re.sub(r"\b[a-z]+\b", lambda m: NUMBERS.get(m.group(0), m.group(0)), text.lower())


def plain(text: str) -> str:
    """Lower case, letters and digits only: "Spider-Man" -> "spiderman"."""
    return "".join(c for c in numbers(text) if c.isalnum())


def fold(p: str) -> str:
    """Transliteration folding, applied to a plain form.

    Hindi titles reach us spelled a dozen ways - dilwale/dilwaale,
    khushi/khushee, wasseypur/vaseypur - and none of those is a mistake worth
    losing a round over. Folding them onto one string does far more work here
    than edit distance ever could.
    """
    for was, now in (("ksh", "x"), ("ph", "f"), ("ee", "i"), ("oo", "u"), ("w", "v"), ("y", "i")):
        p = p.replace(was, now)
    out = []
    for c in p:
        if not out or out[-1] != c:
            out.append(c)
    return "".join(out)


def key(text: str) -> str:
    """What a title or a guess is actually compared as."""
    text = re.sub(r"^\s*(the|an|a)\s+", "", text.strip().lower())
    return fold(plain(text))


def words(text: str) -> list:
    """A title's words, the joining ones dropped: "gangs of wasseypur" is two."""
    return [fold(plain(w)) for w in re.split(r"[^\w]+", numbers(text).lower()) if w and w not in STOPWORDS]


def digits(text: str) -> str:
    """The digits in a string, in order. A sequel number is never forgiven."""
    return "".join(c for c in numbers(text) if c.isdigit())


def slack(length: int) -> int:
    """How many slips a title of this many letters forgives - about a fifth."""
    if length <= 5:
        return 1
    if length <= 10:
        return 2
    if length <= 16:
        return 3
    if length <= 24:
        return 4
    return 5


def distance(a: str, b: str) -> int:
    if a == b:
        return 0
    prev = list(range(len(b) + 1))
    for i, ca in enumerate(a, 1):
        row = [i]
        for j, cb in enumerate(b, 1):
            row.append(min(prev[j] + 1, row[j - 1] + 1, prev[j - 1] + (ca != cb)))
        prev = row
    return prev[-1]


def mask(text: str, title: str, leaked: list) -> str:
    """Blanks out the title words a clue gives away, leaving the rest alone.

    "An unlucky assassin boards a Japanese train..." becomes
    "An unlucky assassin boards a Japanese \u25ae\u25ae\u25ae\u25ae\u25ae...". The line reads as
    somebody wrote it; only the answer is gone.
    """
    out = text
    for word in leaked:
        for form in {word, word.capitalize(), word.upper()}:
            out = re.sub(rf"\b{re.escape(form)}\w*", lambda m: "\u25ae" * len(m.group(0)), out, flags=re.IGNORECASE)
    return out
