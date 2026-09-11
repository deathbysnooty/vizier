"""Find questions whose answer is spelled out by another question.

    python3 giveaways.py [--only FILE_STEM ...] [--min-len 6]

For every typed answer (and its accepted forms) of at least --min-len
characters, looks for it as a whole phrase in the text, note or options of
every OTHER question in the bank. Multiple-choice answers are checked the same
way. Common words make noise, so hits are grouped by answer and printed with
both question ids for a human (or agent) to judge. --only limits the report to
hits where either side is in the named files.
"""

import json
import re
import sys
from collections import defaultdict
from pathlib import Path

BANK = Path(__file__).resolve().parent.parent
STOP = {
    "india", "indian", "water", "english", "hindi", "british", "america", "american", "china", "japan",
    "france", "england", "australia", "pakistan", "delhi", "mumbai", "kolkata", "chennai", "red", "blue",
    "green", "black", "white", "gold", "silver", "iron", "salt", "sugar", "milk", "tea", "rice", "one",
    "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "hundred", "first", "second",
}


def norm(text):
    return " " + re.sub(r"[^a-z0-9]+", " ", text.lower()).strip() + " "


def main():
    args = sys.argv[1:]
    min_len = int(args[args.index("--min-len") + 1]) if "--min-len" in args else 6
    only = set(args[args.index("--only") + 1:]) if "--only" in args else set()
    only = {o for o in only if not o.startswith("--")}

    rows = []
    for path in sorted(BANK.glob("*/*.jsonl")):
        if path.parent.name.startswith("."):
            continue
        for line in path.read_text().splitlines():
            if line.strip():
                q = json.loads(line)
                q["_file"] = path.stem
                rows.append(q)

    haystack = [(q, norm(" ".join([q["q"], q.get("note", ""), " ".join(q.get("options") or [])]))) for q in rows]
    hits = defaultdict(list)
    for q in rows:
        forms = {q["a"], *q.get("alt", [])}
        needles = {norm(f) for f in forms if len(f) >= min_len and f.lower() not in STOP}
        if not needles:
            continue
        for other, text in haystack:
            if other["id"] == q["id"]:
                continue
            if only and q["_file"] not in only and other["_file"] not in only:
                continue
            # An MCQ that lists this answer among its own options is not a giveaway of it.
            if other.get("kind") == "mcq" and other["a"] == q["a"]:
                continue
            for needle in needles:
                if needle in text:
                    hits[(q["id"], q["a"])].append((other["id"], needle.strip()))
                    break

    print(f"{len(hits)} answers appear in other questions")
    for (qid, answer), where in sorted(hits.items()):
        shown = ", ".join(f"{oid} ({needle})" for oid, needle in where[:6])
        more = f" +{len(where) - 6} more" if len(where) > 6 else ""
        print(f"  {qid} [{answer}] <- {shown}{more}")


if __name__ == "__main__":
    main()
