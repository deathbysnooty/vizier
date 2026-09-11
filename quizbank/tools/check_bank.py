"""Validate the quiz bank the way the bot will read it, and report what's in it.

    python3 check_bank.py            # report only
    python3 check_bank.py --dedupe   # also drop exact duplicate questions (keeps the first source in PRIORITY order)

Mirrors Question::is_valid in src/channels/discord/quiz.rs: a question the bot
would skip is reported here as invalid.
"""

import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

BANK = Path(__file__).resolve().parent.parent
# When the same question turns up twice, keep the most trusted copy.
PRIORITY = ["wikidata", "ai", "opentdb", "triviaapi"]
KEYS = {"id", "kind", "q", "a", "alt", "options", "cat", "region", "diff", "note"}


def files():
    return sorted(p for p in BANK.rglob("*.jsonl") if not any(part.startswith(".") for part in p.relative_to(BANK).parts))


def norm(text):
    return re.sub(r"[^a-z0-9]+", "", text.lower())


def problems(q):
    out = []
    missing = KEYS - q.keys()
    if missing:
        out.append(f"missing {sorted(missing)}")
    if not str(q.get("id", "")).strip() or not str(q.get("q", "")).strip() or not str(q.get("a", "")).strip():
        out.append("empty id/q/a")
    if len(q.get("q", "")) > 1000:
        out.append("question too long")
    kind, options = q.get("kind"), q.get("options") or []
    if kind == "mcq":
        if len(options) != 4 or len(set(options)) != 4 or q.get("a") not in options:
            out.append("mcq needs 4 unique options including the answer")
    elif kind == "text":
        if options:
            out.append("text question has options")
    else:
        out.append(f"bad kind {kind!r}")
    if q.get("region") not in ("india", "world"):
        out.append(f"bad region {q.get('region')!r}")
    return out


def source(path):
    rel = path.relative_to(BANK).parts[0]
    return {"api": path.stem}.get(rel, rel)


def main():
    dedupe = "--dedupe" in sys.argv
    seen_ids, by_text = {}, defaultdict(list)
    counts = Counter()
    invalid = []
    rows_by_file = {}
    for path in files():
        rows = []
        for n, line in enumerate(path.read_text().splitlines(), 1):
            if not line.strip():
                continue
            try:
                q = json.loads(line)
            except json.JSONDecodeError as e:
                invalid.append(f"{path.name}:{n} bad json ({e})")
                continue
            issues = problems(q)
            if q.get("id") in seen_ids:
                issues.append(f"duplicate id (also in {seen_ids[q['id']]})")
            seen_ids.setdefault(q.get("id"), path.name)
            if issues:
                invalid.append(f"{path.name}:{n} {q.get('id')}: {'; '.join(issues)}")
            rows.append(q)
            src = source(path)
            counts[(src, q.get("region"), q.get("kind"))] += 1
            by_text[norm(q.get("q", ""))].append((PRIORITY.index(src) if src in PRIORITY else 99, path, q["id"]))
        rows_by_file[path] = rows

    print(f"{'source':<12} {'region':<7} {'text':>6} {'mcq':>6}")
    for src in sorted({s for s, _, _ in counts}):
        for region in ("india", "world"):
            t, m = counts[(src, region, "text")], counts[(src, region, "mcq")]
            if t or m:
                print(f"{src:<12} {region:<7} {t:>6} {m:>6}")
    total = sum(counts.values())
    india = sum(v for (s, r, k), v in counts.items() if r == "india")
    print(f"total {total}  (india {india}, world {total - india})")

    dupes = {k: v for k, v in by_text.items() if len(v) > 1}
    print(f"\nduplicate question texts: {len(dupes)}")
    for k, v in list(dupes.items())[:8]:
        print("  ", [f"{p.name}:{i}" for _, p, i in v])
    print(f"invalid: {len(invalid)}")
    for line in invalid[:20]:
        print("  ", line)

    if dedupe and dupes:
        drop = set()
        for v in dupes.values():
            for _, _, qid in sorted(v, key=lambda x: x[0])[1:]:
                drop.add(qid)
        for path, rows in rows_by_file.items():
            kept = [q for q in rows if q["id"] not in drop]
            if len(kept) != len(rows):
                path.write_text("".join(json.dumps(q, ensure_ascii=False) + "\n" for q in kept))
                print(f"  {path.name}: dropped {len(rows) - len(kept)} duplicates")


if __name__ == "__main__":
    main()
