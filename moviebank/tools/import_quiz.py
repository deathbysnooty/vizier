#!/usr/bin/env python3
"""Folds a hand-made stills database into the bank.

    python3 moviebank/tools/import_quiz.py tools/source_quiz.sqlite

The source has `movies`, `stills` (two per film, each with a one-line hint) and
`accepted_answers`. Its images are already copied into `moviebank/images/`.

A source film is matched onto a bank film when they share ANY answer key, not
just the title: "Birdman or (The Unexpected Virtue of Ignorance)" is the
"Birdman" we already have, and matching on titles alone would quietly file the
same film twice - which the checker would then refuse, because two films that
reduce to one answer make one of them unwinnable.

Matched films gain the stills, the hint lines and any answer spellings they were
missing. Films the bank has never heard of are written to a new file with
everything except tags, which a person writes: a still and a hint are a clue,
but the tags are the game's own voice and are not in anybody's database.
"""

import json
import os
import re
import sqlite3
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "tools"))
from normalise import key, mask, words  # noqa: E402

COMMON_OK = {"after", "another", "about", "before", "over", "under", "into", "from", "with", "other"}


def unleak(hint: str, title: str) -> str:
    """A hint may not print its own answer. Masked rather than rewritten.

    The comparison happens on folded words - "Wakanda" and "vakanda" are one
    word to the matcher - but the masking has to run against the title as it is
    really spelled, or the blanking quietly misses the word it was called for.
    """
    hay = set(words(hint))
    raw = [w for w in re.split(r"[^\w]+", title) if w]
    leaked = [w for w in raw if len(w) > 3 and words(w) and words(w)[0] not in COMMON_OK and words(w)[0] in hay]
    return mask(hint, title, leaked) if leaked else hint

NEW_FILE = os.path.join(HERE, "ai", "hollywood_stills.jsonl")

# How far two release years may differ and still be the same film.
YEAR_SLACK = 1


def bank_files() -> dict:
    files = {}
    for name in sorted(os.listdir(os.path.join(HERE, "ai"))):
        if name.endswith(".jsonl"):
            path = os.path.join(HERE, "ai", name)
            files[path] = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
    return files


def keys_of(movie: dict) -> set:
    return {key(movie["title"])} | {key(a) for a in movie.get("answers", [])}


def main() -> None:
    source = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "tools", "source_quiz.sqlite")
    db = sqlite3.connect(source)
    files = bank_files()
    index = [(movie, keys_of(movie), path) for path, rows in files.items() for movie in rows]

    matched, added, fresh = 0, 0, []
    for mid, title, year, era in db.execute("SELECT id, title, year, era FROM movies ORDER BY id"):
        answers = [a for (a,) in db.execute("SELECT answer FROM accepted_answers WHERE movie_id = ? ORDER BY answer", (mid,))]
        shots = [
            {"file": f"images/{os.path.basename(f)}", "hint": unleak(hint, title), "note": f"still {order} of {title}"}
            for order, f, hint in db.execute(
                "SELECT reveal_order, image_file, hint FROM stills WHERE movie_id = ? ORDER BY reveal_order", (mid,)
            )
        ]
        for f in shots:
            if not os.path.exists(os.path.join(HERE, f["file"])):
                sys.exit(f"{f['file']} is not in moviebank/images/ — copy the images across first")
        source_keys = {key(title)} | {key(a) for a in answers}

        # A shared title is not a shared film. "Don" is a 2006 Hindi thriller
        # and a 2022 Tamil comedy; "Vikram Vedha" is a Tamil original and its
        # Hindi remake five years later. Matching on the name alone hangs one
        # film's pictures on another, which no checker downstream can catch
        # because the bank looks perfectly well formed afterwards.
        hit = next((m for m, mk, _ in index if mk & source_keys and abs(m.get("year", 0) - year) <= YEAR_SLACK), None)
        clash = next((m for m, mk, _ in index if mk & source_keys and abs(m.get("year", 0) - year) > YEAR_SLACK), None)
        if hit is None and clash is not None:
            print(f"  skipped {title} ({year}): shares a title with {clash['title']} ({clash['year']}) but is a different film")
        if hit is not None:
            matched += 1
            have = {key(a) for a in hit.get("answers", [])}
            hit.setdefault("answers", []).extend(a for a in answers if key(a) not in have and key(a) != key(hit["title"]))
            hit.setdefault("shots", []).extend(shots)
            added += len(shots)
            continue
        fresh.append({
            "id": f"hw-stills-{len(fresh) + 1:03d}",
            "title": title,
            "year": year,
            # The bank's two eras, by year: the source's own three don't map onto
            # them, and the year is the honest thing to read it from.
            "era": "modern" if year >= 2010 else "iconic",
            "industry": "hollywood",
            "difficulty": "medium",
            "answers": [title] + [a for a in answers if key(a) != key(title)],
            "tags": [],
            "dialogues": [],
            "shots": shots,
        })

    for path, rows in files.items():
        with open(path, "w", encoding="utf-8") as f:
            for row in rows:
                f.write(json.dumps(row, ensure_ascii=False) + "\n")
    if fresh:
        with open(NEW_FILE, "w", encoding="utf-8") as f:
            for row in fresh:
                f.write(json.dumps(row, ensure_ascii=False) + "\n")

    print(f"{matched} films matched and gained {added} stills")
    print(f"{len(fresh)} films new -> {os.path.relpath(NEW_FILE, os.getcwd())} (tags still to be written)")


if __name__ == "__main__":
    main()
