#!/usr/bin/env python3
"""Folds a quiz-database pack into the bank: films AND television.

    python3 moviebank/tools/import_pack.py ~/Downloads/bollywood-and-tv-quiz-expansion [--write]

The pack is `quiz_database.json` plus an `images/` folder. Each record carries a
title, a year, a language, two stills and a one-line hint for each of them.

Three things happen here that are worth knowing:

* A record whose answers already match a film in the bank is MERGED into it -
  the stills and any missing spellings are added, and no second entry appears.
  Two records for one answer would make one of them unwinnable.
* A record the bank has never seen becomes a new entry with NO tags. Tags are
  the game's own voice and are written by a person; a pair of stills and their
  hints is enough to play with in the meantime.
* Television is marked `kind: "series"`. Everything else in the bank is a film
  and says nothing, so the field is absent on all 917 of them.

Nothing is written without `--write`.
"""

import json
import os
import re
import shutil
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "tools"))
from normalise import key, mask, words  # noqa: E402

# Words a hint may share with a title without giving it away.
COMMON_OK = {"after", "another", "about", "before", "over", "under", "into", "from", "with", "other", "this", "that"}
# How far two years may differ and still be the same title.
YEAR_SLACK = 1
MAX_LINE = 160


def unleak(hint: str, title: str) -> str:
    """A hint may not print its own answer. Masked, not rewritten."""
    hay = set(words(hint))
    raw = [w for w in re.split(r"[^\w]+", title) if w]
    leaked = [w for w in raw if len(w) > 3 and words(w) and words(w)[0] not in COMMON_OK and words(w)[0] in hay]
    return mask(hint, title, leaked) if leaked else hint


def bank_files() -> dict:
    files = {}
    for name in sorted(os.listdir(os.path.join(HERE, "ai"))):
        if name.endswith(".jsonl"):
            path = os.path.join(HERE, "ai", name)
            files[path] = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
    return files


def keys_of(entry: dict) -> set:
    return {key(a) for a in entry.get("answers", [])} | {key(entry.get("title", ""))}


def answers_of(record: dict) -> list:
    """The spellings worth keeping: the title, and any real alternative.

    "Island City (2016)" is not an alternative, it is the title with the year
    stuck on - and the matcher's digit guard treats a number as a sequel, so
    letting one in would teach it that 2016 matters.
    """
    out = []
    for raw in [record["title"], *record.get("accepted_answers", [])]:
        cleaned = re.sub(r"\s*\(\d{4}\)\s*$", "", raw).strip()
        if cleaned and key(cleaned) not in [key(a) for a in out]:
            out.append(cleaned)
    return out


def industry_of(record: dict) -> str:
    """Hindi is bollywood, everything else is hollywood - the bank has two."""
    languages = record.get("languages") or [record.get("language") or ""]
    return "bollywood" if any(l.lower().startswith("hindi") for l in languages) else "hollywood"


def shots_of(record: dict) -> list:
    shots = []
    for n, still in enumerate(record.get("stills", []), 1):
        file = still.get("image_file", "")
        if not file:
            continue
        hint = unleak((still.get("hint") or "").strip(), record["title"])[:MAX_LINE]
        what = "episode" if record.get("content_type") == "tv_series" else "still"
        shots.append({"file": "images/" + os.path.basename(file), "hint": hint,
                      "note": f"{what} {n} of {record['title']}"})
    return shots


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    write = "--write" in sys.argv
    if not args:
        sys.exit(__doc__)
    pack = os.path.abspath(os.path.expanduser(args[0]))
    data = json.load(open(os.path.join(pack, "quiz_database.json"), encoding="utf-8"))
    records = data["entries"]

    files = bank_files()
    index = {}
    for path, entries in files.items():
        for entry in entries:
            for k in keys_of(entry):
                index.setdefault(k, []).append((path, entry))

    merged, fresh, skipped = 0, [], []
    for record in records:
        title, year = record["title"], record["year"]
        series = record.get("content_type") == "tv_series"
        answers = answers_of(record)
        shots = shots_of(record)
        if not shots:
            skipped.append(f"{title}: no stills")
            continue

        # Already in the bank? Only a film can match a film, and only within a
        # year of it: Don (1978) and Don (2006) are two films, not one.
        hit = None
        for k in {key(a) for a in answers}:
            for path, entry in index.get(k, []):
                same_kind = (entry.get("kind") == "series") == series
                if same_kind and abs(entry.get("year", 0) - year) <= YEAR_SLACK:
                    hit = (path, entry)
                    break
            if hit:
                break

        if hit:
            path, entry = hit
            have = {os.path.basename(s.get("file", "")) for s in entry.get("shots", [])}
            added = [s for s in shots if os.path.basename(s["file"]) not in have]
            if added:
                entry.setdefault("shots", []).extend(added)
            for spelling in answers:
                if key(spelling) not in [key(a) for a in entry.get("answers", [])]:
                    entry.setdefault("answers", []).append(spelling.lower())
            merged += 1
            continue

        # The hint that is NOT tied to a still goes on the entry itself, so the
        # bot can ask about it in words alone as well as with a picture.
        lines = [unleak((s.get("hint") or "").strip(), title)[:MAX_LINE] for s in record.get("stills", [])]
        hints = [lines[-1]] if lines else []
        fresh.append({
            "id": f"pk3-{record['id']}",
            "title": title,
            "year": year,
            "industry": industry_of(record),
            "era": "modern" if year >= 2010 else "iconic",
            "difficulty": "medium",
            "kind": "series" if series else "film",
            "answers": [a.lower() for a in answers],
            "tags": [],
            "dialogues": [],
            "hints": hints,
            "shots": shots,
            "category": record.get("category", ""),
        })

    by_file = {}
    for entry in fresh:
        where = {"indian_tv": "tv_indian", "english_tv": "tv_english"}.get(entry.pop("category", ""), "bollywood_pack3")
        by_file.setdefault(where, []).append(entry)

    print(f"pack: {len(records)} records")
    print(f"  merged into films already here: {merged}")
    for where, entries in sorted(by_file.items()):
        films = sum(1 for e in entries if e["kind"] == "film")
        print(f"  {where}.jsonl: {len(entries)} new ({films} films, {len(entries) - films} series)")
    for line in skipped:
        print(f"  skipped {line}")

    if not write:
        print("\nnothing written - pass --write")
        return

    # New entries go into the same picture of the bank the merges were made in,
    # and then every file is written once. Appending first and rewriting after
    # truncates the appends back out again - which is exactly what happened the
    # first time a pack landed in files that already existed.
    for where, entries in by_file.items():
        path = os.path.join(HERE, "ai", where + ".jsonl")
        files.setdefault(path, []).extend(entries)
        print(f"wrote {len(entries)} to ai/{where}.jsonl")
    for path, entries in files.items():
        with open(path, "w", encoding="utf-8") as out:
            for entry in entries:
                out.write(json.dumps(entry, ensure_ascii=False) + "\n")
        written = sum(1 for line in open(path, encoding="utf-8") if line.strip())
        if written != len(entries):
            sys.exit(f"{os.path.basename(path)}: wrote {written} of {len(entries)} entries")

    images = os.path.join(HERE, "images")
    os.makedirs(images, exist_ok=True)
    copied = 0
    for record in records:
        for still in record.get("stills", []):
            name = os.path.basename(still.get("image_file", ""))
            source = os.path.join(pack, "images", name)
            if name and os.path.exists(source) and not os.path.exists(os.path.join(images, name)):
                shutil.copy2(source, os.path.join(images, name))
                copied += 1
    print(f"copied {copied} images into moviebank/images/")


if __name__ == "__main__":
    main()
