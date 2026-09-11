"""Download the two free trivia databases into the quiz bank format.

    python3 fetch_apis.py opentdb     # Open Trivia DB, CC BY-SA 4.0
    python3 fetch_apis.py triviaapi   # The Trivia API, CC BY-NC 4.0 (free tier: multiple choice only)

Both write JSONL to quizbank/api/. Re-running replaces the file.
"""

import base64
import hashlib
import json
import re
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "api"
UA = {"User-Agent": "MLCI-quizbank/0.1 (Discord community quiz)"}


def get(url):
    for attempt in range(6):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=30) as r:
                return json.load(r)
        except (urllib.error.URLError, TimeoutError) as e:
            wait = 10 * (attempt + 1)
            print(f"  retry in {wait}s: {e}", flush=True)
            time.sleep(wait)
    raise RuntimeError(f"gave up on {url}")


def slug(text):
    text = text.split(":")[-1]
    return re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")


def india(*texts):
    return any(re.search(r"\bindia", t, re.I) for t in texts)


def opentdb():
    token = get("https://opentdb.com/api_token.php?command=request")["token"]
    b64 = lambda s: base64.b64decode(s).decode("utf-8")
    seen, rows, amount = set(), [], 50
    while True:
        data = get(f"https://opentdb.com/api.php?amount={amount}&type=multiple&encode=base64&token={token}")
        code = data["response_code"]
        if code == 5:
            time.sleep(6)
            continue
        if code == 1:
            if amount == 1:
                break
            amount = max(1, amount // 5)
            time.sleep(5.5)
            continue
        if code == 4:
            break
        if code != 0:
            raise RuntimeError(f"opentdb response code {code}")
        for item in data["results"]:
            q, a = b64(item["question"]), b64(item["correct_answer"])
            wrong = [b64(x) for x in item["incorrect_answers"]]
            key = hashlib.sha1(q.encode()).hexdigest()[:12]
            if key in seen or len(set([a] + wrong)) != 4:
                continue
            seen.add(key)
            rows.append({
                "id": f"otdb-{key}", "kind": "mcq", "q": q, "a": a, "alt": [],
                "options": [a] + wrong, "cat": slug(b64(item["category"])),
                "region": "india" if india(q, a) else "world",
                "diff": b64(item["difficulty"]), "note": "", "src": "opentdb",
            })
        print(f"  opentdb: {len(rows)} questions", flush=True)
        time.sleep(5.5)
    return rows


def triviaapi():
    # The free tier has no paging, only random draws, so keep drawing until
    # hardly anything new turns up. Polite pace: one request a second.
    seen, rows, stale = set(), [], 0
    while stale < 40 and len(rows) < 12000:
        batch = get("https://the-trivia-api.com/v2/questions?limit=50")
        new = 0
        for item in batch:
            if item["id"] in seen or item.get("isNiche") or item["type"] != "text_choice":
                continue
            seen.add(item["id"])
            a, wrong = item["correctAnswer"], item["incorrectAnswers"]
            if len(set([a] + wrong)) != 4:
                continue
            q = item["question"]["text"]
            new += 1
            rows.append({
                "id": f"tta-{item['id']}", "kind": "mcq", "q": q, "a": a, "alt": [],
                "options": [a] + wrong, "cat": slug(item["category"]),
                "region": "india" if "IN" in (item.get("regions") or []) or india(q, a) else "world",
                "diff": item.get("difficulty", "medium"), "note": "", "src": "triviaapi",
            })
        stale = stale + 1 if new < 3 else 0
        if len(seen) % 500 < 50:
            print(f"  triviaapi: {len(rows)} questions", flush=True)
        time.sleep(1)
    return rows


# Politics and celebrity questions go stale and start arguments; so does
# anything that only holds "currently".
SKIP_CATS = {"politics", "celebrities"}
DATED = re.compile(r"\b(current(ly)?|as of|latest|most recent|this year|right now|nowadays|still alive|recently)\b", re.I)


def keep(row):
    return row["cat"] not in SKIP_CATS and not DATED.search(row["q"])


if __name__ == "__main__":
    which = sys.argv[1]
    if which == "clean":
        for path in sorted(OUT.glob("*.jsonl")):
            rows = [json.loads(l) for l in path.read_text().splitlines() if l.strip()]
            kept = [r for r in rows if keep(r)]
            path.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in kept))
            print(f"{path.name}: kept {len(kept)} of {len(rows)}")
        sys.exit()
    rows = [r for r in {"opentdb": opentdb, "triviaapi": triviaapi}[which]() if keep(r)]
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"{which}.jsonl"
    path.write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))
    print(f"wrote {len(rows)} to {path}")
