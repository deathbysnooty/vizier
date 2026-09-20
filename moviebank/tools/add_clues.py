#!/usr/bin/env python3
"""Folds hand-written tags and hint lines onto entries already in the bank.

    python3 moviebank/tools/add_clues.py clues.jsonl [--write]

Each line is `{"id": "...", "tags": [...], "hints": [...]}`. Tags replace what
the entry had (an imported entry has none); hint lines are added to the ones it
came with, because the pack's own line names the cast and a written one
describes the story - two different ways to ask.

Nothing is written without `--write`, and an id the bank has never heard of is
an error rather than a silent no-op: it means a typo in a file a person wrote.
"""

import json
import os
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(HERE, "tools"))
from normalise import key, words  # noqa: E402

COMMON = {"after", "another", "about", "before", "over", "under", "into", "from", "with", "this", "that", "there", "their", "what", "when", "where", "while", "would", "could", "should", "been", "have", "more", "most", "some", "such", "only", "also", "even", "just", "like", "very", "much", "many", "both", "each", "once", "other"}
COMMON_FOLDED = {words(w)[0] for w in COMMON if words(w)}


def leaks(text: str, title: str) -> list:
    said = set(words(text))
    return [w for w in words(title) if len(w) > 3 and w not in COMMON_FOLDED and w in said]


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    write = "--write" in sys.argv
    if not args:
        sys.exit(__doc__)
    patches = [json.loads(line) for line in open(args[0], encoding="utf-8") if line.strip()]

    files = {}
    for name in sorted(os.listdir(os.path.join(HERE, "ai"))):
        if name.endswith(".jsonl"):
            path = os.path.join(HERE, "ai", name)
            files[path] = [json.loads(line) for line in open(path, encoding="utf-8") if line.strip()]
    index = {e["id"]: (path, e) for path, entries in files.items() for e in entries}

    touched, problems = set(), []
    for patch in patches:
        found = index.get(patch["id"])
        if not found:
            problems.append(f"{patch['id']}: no entry with that id")
            continue
        path, entry = found
        title = entry["title"]
        for tag in patch.get("tags", []):
            said = leaks(tag, title)
            if said:
                problems.append(f"{patch['id']} ({title}): tag {tag!r} gives the title away ({', '.join(said)})")
        for line in patch.get("hints", []):
            said = leaks(line, title)
            if said:
                problems.append(f"{patch['id']} ({title}): hint {line!r} gives the title away ({', '.join(said)})")
        if patch.get("tags"):
            entry["tags"] = patch["tags"]
        for line in patch.get("hints", []):
            if line not in entry.setdefault("hints", []):
                entry["hints"].append(line)
        touched.add(path)

    for line in problems:
        print(f"  {line}")
    print(f"{len(patches)} patches, {len(problems)} problems")
    if problems or not write:
        sys.exit(1 if problems else 0)

    for path in touched:
        with open(path, "w", encoding="utf-8") as out:
            for entry in files[path]:
                out.write(json.dumps(entry, ensure_ascii=False) + "\n")
        print(f"updated {os.path.basename(path)}")


if __name__ == "__main__":
    main()
