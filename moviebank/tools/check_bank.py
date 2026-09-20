#!/usr/bin/env python3
"""Reads every line of the bank and refuses the ones the game could not play.

What it is really looking for is the three ways this bank goes wrong:

* a clue that gives the title away, in any language;
* two films near enough that a generous guess could take the wrong one —
  which is what makes it safe to forgive spelling everywhere else;
* a tag set so vague it fits half the bank.

    python3 moviebank/tools/check_bank.py
"""

import glob
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from normalise import STOPWORDS, digits, distance, key, slack, words  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
INDUSTRIES = {"bollywood", "hollywood"}
ERAS = {"iconic", "modern"}
KINDS = {"film", "series"}
DIFFICULTIES = {"easy", "medium", "hard"}
MAX_TAG = 44
MAX_DIALOGUE = 160
SHARED_TAGS = 4


# Words that carry no identity even when a title contains them: "One Battle
# After Another" is not given away by the word "after". Kept deliberately short
# - anything that could name a film stays out of it, so "room", "heat" and
# "split" are all still leaks.
COMMON = {
    "after", "another", "about", "before", "over", "under", "into", "from", "with", "this", "that",
    "these", "those", "than", "then", "there", "their", "they", "what", "when", "where", "which",
    "while", "would", "could", "should", "been", "have", "more", "most", "some", "such", "only",
    "also", "even", "just", "like", "very", "much", "many", "both", "each", "once", "other",
}


# Compared against folded words, so the list has to be folded too: "with"
# reduces to "vith", and an unfolded entry would never match anything.
COMMON_FOLDED = {words(w)[0] for w in COMMON if words(w)}


def leaks(text: str, title: str) -> list:
    """Title words that turn up in a clue. "Run, Forrest, run" is no clue."""
    hay = set(words(text))
    return [w for w in words(title) if len(w) > 3 and w not in COMMON_FOLDED and w in hay]


def collides(mine: list, theirs: list) -> str:
    """How close two films' answers are, in the terms the bot plays by.

    An answer wins on an exact fold, or on a slip within budget when no OTHER
    film is just as near. So:

    * `"same"` — two films reduce to the SAME string. One of them can never be
      won, whatever anybody types. That fails the bank.
    * `"near"` — inside each other's slip budget. Both are still winnable typed
      correctly; what they lose is forgiveness, because a typo near either one
      is a guess that fits two films and takes neither. Worth knowing, not worth
      refusing.
    * `""` — no relation.
    """
    near = ""
    for a in mine:
        for b in theirs:
            if digits(a) != digits(b):
                continue
            if a == b:
                return "same"
            if distance(a, b) <= slack(max(len(a), len(b))):
                near = "near"
    return near


TIGHT = []
SHARED = []


def check(movies: list) -> list:
    problems = []

    def bad(movie, msg):
        problems.append(f"{movie.get('id', '?')} ({movie.get('title', '?')}): {msg}")

    seen_ids = set()
    for m in movies:
        for field in ("id", "title", "year", "industry", "era", "answers", "tags"):
            if field not in m:
                bad(m, f"no {field}")
        if m.get("id") in seen_ids:
            bad(m, "that id is used twice")
        seen_ids.add(m.get("id"))
        if m.get("industry") not in INDUSTRIES:
            bad(m, f"industry {m.get('industry')!r} is not one of {sorted(INDUSTRIES)}")
        if m.get("era") not in ERAS:
            bad(m, f"era {m.get('era')!r} is not one of {sorted(ERAS)}")
        if m.get("kind", "film") not in KINDS:
            bad(m, f"kind {m.get('kind')!r} is not one of {sorted(KINDS)}")
        if m.get("difficulty", "easy") not in DIFFICULTIES:
            bad(m, f"difficulty {m.get('difficulty')!r} is not one of {sorted(DIFFICULTIES)}")
        if not isinstance(m.get("year"), int) or not 1900 < m["year"] < 2100:
            bad(m, f"year {m.get('year')!r}")

        title = m.get("title", "")
        answers = m.get("answers", [])
        if not answers:
            bad(m, "no answers at all")
        if title and key(title) not in [key(a) for a in answers]:
            bad(m, "the title itself is not in answers")

        tags = m.get("tags", [])
        # Tags are how a film asks without a picture, so a film with fewer than
        # two stills has to have them. One with two can already ask two ways -
        # either still - and is allowed in untagged, to be written up later.
        if tags and not 4 <= len(tags) <= 8:
            bad(m, f"{len(tags)} tags — it wants 4 to 8, vague first and sharp last")
        elif not tags and len(m.get("shots", [])) < 2:
            bad(m, "no tags, and not enough stills to ask without them")
        for tag in tags:
            if len(tag) > MAX_TAG:
                bad(m, f"tag {tag!r} is longer than {MAX_TAG} characters")
            found = leaks(tag, title)
            if found:
                bad(m, f"tag {tag!r} gives the title away ({', '.join(found)})")
        if len({key(t) for t in tags}) != len(tags):
            bad(m, "the same tag twice")

        for line in m.get("hints", []):
            if len(line) > MAX_DIALOGUE:
                bad(m, f"hint {line[:30]!r}... is longer than {MAX_DIALOGUE} characters")
            found = leaks(line, title)
            if found:
                bad(m, f"hint {line!r} gives the title away ({', '.join(found)})")

        for line in m.get("dialogues", []):
            if len(line) > MAX_DIALOGUE:
                bad(m, f"line {line[:30]!r}... is longer than {MAX_DIALOGUE} characters")
            found = leaks(line, title)
            if found:
                bad(m, f"line {line!r} gives the title away ({', '.join(found)})")

        for shot in m.get("shots", []):
            # A still is either one of TMDB's, named by its path on their CDN,
            # or one of ours, a file under moviebank/images. Ours has to really
            # be there: a missing image is a round nobody can answer.
            path, file = shot.get("path", ""), shot.get("file", "")
            if file:
                if not file.lower().endswith((".jpg", ".jpeg", ".png")):
                    bad(m, f"still {file!r} is not an image file")
                elif not os.path.exists(os.path.join(HERE, file)):
                    bad(m, f"still {file} is not on disk — copy the images into moviebank/")
            elif path:
                if not path.startswith("/") or not path.lower().endswith((".jpg", ".png")):
                    bad(m, f"still {path!r} is not a TMDB file path")
            else:
                bad(m, "a still with neither a file nor a path")
            if not shot.get("note") and not shot.get("hint"):
                bad(m, f"still {file or path} has neither a note nor a hint")
            hint = shot.get("hint", "")
            if len(hint) > MAX_DIALOGUE:
                bad(m, f"hint {hint[:30]!r}... is longer than {MAX_DIALOGUE} characters")
            if hint:
                found = leaks(hint, title)
                if found:
                    bad(m, f"hint {hint!r} gives the title away ({', '.join(found)})")

        if not tags and not m.get("dialogues") and not m.get("shots"):
            bad(m, "nothing to play it with")

    # Answers that could be forgiven onto the wrong film. This is the check that
    # lets the matcher be generous everywhere else.
    keyed = [(m, [key(a) for a in m.get("answers", [])]) for m in movies]
    for i, (mine, my_keys) in enumerate(keyed):
        for theirs, their_keys in keyed[i + 1:]:
            how = collides(my_keys, their_keys)
            if how == "same":
                # Two DIFFERENT films can share a title - Fighter (2024) and The
                # Fighter (2010) - and that is fine: an exact answer wins
                # outright, so whichever is on the card takes the round. What is
                # broken is the SAME film entered twice, because then the bank
                # holds two records for one answer and the no-repeat window,
                # the boards and the clue picking all treat them as strangers.
                same_film = mine.get("year") == theirs.get("year")
                if same_film:
                    bad(mine, f"is the same film as {theirs.get('title')!r}, entered twice — merge them")
                else:
                    SHARED.append(f"{mine.get('title')} ({mine.get('year')}) / {theirs.get('title')} ({theirs.get('year')}): one title, two films — both win when they are the one on the card")
            elif how == "near":
                TIGHT.append(f"{mine.get('title')} / {theirs.get('title')}: typed exactly they are fine, but a typo near either takes neither")

    # Tag sets that describe each other.
    tagged = [(m, {key(t) for t in m.get("tags", [])}) for m in movies]
    for i, (mine, my_tags) in enumerate(tagged):
        for theirs, their_tags in tagged[i + 1:]:
            shared = my_tags & their_tags
            if len(shared) >= SHARED_TAGS:
                bad(mine, f"shares {len(shared)} tags with {theirs.get('title')!r} — sharpen one of them")

    return problems


def nearest(movies: list) -> None:
    """The closest other film to each one, so fuzzy pairs are visible early."""
    keyed = [(m, [key(a) for a in m.get("answers", [])]) for m in movies]
    print("\nnearest neighbour by answer:")
    for mine, my_keys in keyed:
        best = None
        for theirs, their_keys in keyed:
            if theirs is mine:
                continue
            for a in my_keys:
                for b in their_keys:
                    d = distance(a, b)
                    if best is None or d < best[0]:
                        best = (d, theirs.get("title"))
        if best:
            flag = "  <-- close" if best[0] <= slack(len(my_keys[0])) + 1 else ""
            print(f"  {mine.get('title'):<34} {best[0]:>3} from {best[1]}{flag}")


def main() -> None:
    movies = []
    for path in sorted(glob.glob(os.path.join(HERE, "ai", "*.jsonl"))):
        for n, line in enumerate(open(path, encoding="utf-8"), 1):
            if line.strip():
                movies.append(json.loads(line))
    problems = check(movies)
    counts = {}
    for m in movies:
        counts[(m.get("industry"), m.get("era"))] = counts.get((m.get("industry"), m.get("era")), 0) + 1
    print(f"{len(movies)} movies")
    for (industry, era), n in sorted(counts.items()):
        print(f"  {industry:<10} {era:<7} {n}")
    print(f"  untagged (stills only) {sum(1 for m in movies if not m.get('tags'))}")
    print(f"  stills {sum(len(m.get('shots', [])) for m in movies)}"
          f" · hint lines {sum(len(m.get('hints', [])) for m in movies)}"
          f" · lines {sum(len(m.get('dialogues', [])) for m in movies)}"
          f" · no still {sum(1 for m in movies if not m.get('shots'))}")
    if SHARED:
        print(f"\n{len(SHARED)} titles shared by two different films (fine, and deliberate):")
        for t in SHARED:
            print(f"  {t}")
    if TIGHT:
        print(f"\n{len(TIGHT)} pairs too close to forgive a typo between (playable, just strict):")
        for t in TIGHT:
            print(f"  {t}")
    if "-v" in sys.argv:
        nearest(movies)
    if problems:
        print(f"\n{len(problems)} problems:")
        for p in problems:
            print(f"  {p}")
        sys.exit(1)
    print("\nbank is playable")


if __name__ == "__main__":
    main()
