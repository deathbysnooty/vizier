"""Build a review page of sample questions from every part of the bank.

    python3 preview.py OUT.html [--per 6]

Samples are seeded, so re-running on an unchanged bank gives the same page.
"""

import json
import random
import sys
from collections import Counter
from pathlib import Path

BANK = Path(__file__).resolve().parent.parent

LABELS = {
    "bollywood": "Bollywood films", "bollywood_buzz": "Bollywood gossip", "music_tv": "Music & Indian TV",
    "cricket": "Cricket", "ipl": "IPL", "history_civics": "History & civics", "geography": "Geography",
    "culture": "Food, festivals & culture", "sports_science_biz": "Sports, science & business",
    "desi_pop": "Desi pop culture", "north_india": "North India", "south_india": "South India",
    "east_northeast": "East & Northeast", "west_central": "West & Central", "regional_cinema": "Regional cinema",
    "hindi": "Hindi muhavare & language", "world_tv": "International TV shows",
    "opentdb": "Open Trivia DB", "triviaapi": "The Trivia API",
}
GROUPS = [
    ("Written for MLCI, fact-checked", "ai"),
    ("Built from Wikidata", "wikidata"),
    ("Downloaded trivia databases", "api"),
]


def label(path):
    if path.parent.name == "wikidata":
        return path.stem.replace("_", " ").capitalize()
    return LABELS.get(path.stem, path.stem.replace("_", " ").capitalize())


def sample(rows, n, rng):
    typed = [r for r in rows if r["kind"] == "text"]
    mcq = [r for r in rows if r["kind"] == "mcq"]
    rng.shuffle(typed)
    rng.shuffle(mcq)
    half = n // 2
    picked = typed[:half] + mcq[: n - min(half, len(typed))]
    picked += typed[half: half + n - len(picked)]
    return picked[:n]


def main():
    out = Path(sys.argv[1])
    per = int(sys.argv[sys.argv.index("--per") + 1]) if "--per" in sys.argv else 6
    rng = random.Random(11)
    groups, totals = [], Counter()
    for title, folder in GROUPS:
        files = []
        for path in sorted((BANK / folder).glob("*.jsonl")):
            rows = [json.loads(l) for l in path.read_text().splitlines() if l.strip()]
            if not rows:
                continue
            for r in rows:
                totals[(folder, r["region"])] += 1
            count = per if folder == "ai" else (8 if folder == "api" else 2)
            files.append({
                "name": label(path),
                "count": len(rows),
                "typed": sum(r["kind"] == "text" for r in rows),
                "samples": [{k: r.get(k) for k in ("id", "kind", "q", "a", "alt", "options", "cat", "region", "diff", "note")}
                            for r in sample(rows, count, rng)],
            })
        groups.append({"title": title, "files": files})
    data = {
        "groups": groups,
        "india": sum(v for (f, r), v in totals.items() if r == "india"),
        "world": sum(v for (f, r), v in totals.items() if r == "world"),
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(PAGE.replace("/*DATA*/null", json.dumps(data, ensure_ascii=False)))
    print(f"wrote {out} ({data['india'] + data['world']} questions in the bank)")


PAGE = r"""<title>Loduchand Quiz Bank</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Bricolage+Grotesque:opsz,wght@12..96,500;12..96,700&family=Source+Sans+3:wght@400;600&family=IBM+Plex+Mono:wght@400;500&display=swap">
<style>
:root {
  --ground: #f1f2f6; --surface: #ffffff; --ink: #1b1d2a; --muted: #5d6072; --line: #dcdee8;
  --accent: #5865f2; --accent-soft: #e6e8fd; --india: #d9771a; --india-soft: #fbeedd;
  --world: #17807f; --world-soft: #dcf0ef; --right: #1f8a4c; --right-soft: #e1f3e8;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --ground: #14151b; --surface: #1d1f28; --ink: #e7e8ef; --muted: #9a9db0; --line: #2d3040;
    --accent: #8d96f8; --accent-soft: #272b4a; --india: #f0a255; --india-soft: #3a2b1b;
    --world: #4fc2c0; --world-soft: #17312f; --right: #5bd08f; --right-soft: #183324;
  }
}
:root[data-theme="dark"] {
  --ground: #14151b; --surface: #1d1f28; --ink: #e7e8ef; --muted: #9a9db0; --line: #2d3040;
  --accent: #8d96f8; --accent-soft: #272b4a; --india: #f0a255; --india-soft: #3a2b1b;
  --world: #4fc2c0; --world-soft: #17312f; --right: #5bd08f; --right-soft: #183324;
}
* { box-sizing: border-box; }
body { background: var(--ground); color: var(--ink); font: 16px/1.55 "Source Sans 3", "Segoe UI", system-ui, sans-serif; }
main { max-width: 1120px; margin: 0 auto; padding: 40px 24px 80px; display: grid; gap: 48px; }
h1, h2, h3 { font-family: "Bricolage Grotesque", "Avenir Next", system-ui, sans-serif; text-wrap: balance; margin: 0; }
h1 { font-size: 2.4rem; font-weight: 700; letter-spacing: -0.02em; }
h2 { font-size: 1.45rem; font-weight: 700; }
h3 { font-size: 1.1rem; font-weight: 500; }
.lede { max-width: 65ch; color: var(--muted); margin: 8px 0 0; }
.mono { font-family: "IBM Plex Mono", ui-monospace, monospace; font-size: 0.85rem; }
header { display: grid; gap: 20px; }
.totals { display: flex; flex-wrap: wrap; gap: 12px; }
.total { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 10px 16px; display: grid; }
.total b { font-family: "Bricolage Grotesque", system-ui, sans-serif; font-size: 1.6rem; font-variant-numeric: tabular-nums; }
.total span { color: var(--muted); font-size: 0.85rem; letter-spacing: 0.04em; text-transform: uppercase; }
.filters { display: flex; flex-wrap: wrap; gap: 8px; }
.filters button { font: inherit; font-size: 0.9rem; border: 1px solid var(--line); background: var(--surface); color: var(--ink);
  border-radius: 999px; padding: 4px 14px; cursor: pointer; }
.filters button[aria-pressed="true"] { background: var(--accent); border-color: var(--accent); color: #fff; }
.filters button:focus-visible, summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
section.group { display: grid; gap: 28px; }
.group > h2 { border-bottom: 1px solid var(--line); padding-bottom: 8px; }
.topic { display: grid; gap: 12px; }
.topic-head { display: flex; flex-wrap: wrap; align-items: baseline; gap: 6px 14px; }
.topic-head .meta { color: var(--muted); font-size: 0.9rem; font-variant-numeric: tabular-nums; }
.cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(320px, 1fr)); gap: 12px; }
.card { background: var(--surface); border: 1px solid var(--line); border-radius: 10px; padding: 14px 16px; display: grid; gap: 10px; align-content: start; }
.card[hidden] { display: none; }
.chips { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
.chip { font-size: 0.75rem; letter-spacing: 0.04em; text-transform: uppercase; border-radius: 999px; padding: 1px 9px; font-weight: 600; }
.chip.india { background: var(--india-soft); color: var(--india); }
.chip.world { background: var(--world-soft); color: var(--world); }
.chip.kind { background: var(--accent-soft); color: var(--accent); }
.chip.diff { color: var(--muted); border: 1px solid var(--line); }
.q { font-weight: 600; font-size: 1.02rem; margin: 0; }
.options { list-style: none; padding: 0; margin: 0; display: grid; gap: 4px; }
.options li { border: 1px solid var(--line); border-radius: 7px; padding: 3px 10px; font-size: 0.95rem; }
.options li.right { background: var(--right-soft); border-color: var(--right); color: var(--ink); }
.options li.right::after { content: " ✓"; color: var(--right); font-weight: 600; }
.answer { display: grid; gap: 2px; font-size: 0.95rem; }
.answer b { color: var(--right); }
.answer .alt { color: var(--muted); font-size: 0.88rem; }
.note { color: var(--muted); font-size: 0.9rem; font-style: italic; margin: 0; }
.id { color: var(--muted); font-size: 0.75rem; }
@media (max-width: 560px) { h1 { font-size: 1.9rem; } main { padding: 28px 16px 60px; } }
</style>
<main>
  <header>
    <div>
      <h1>Loduchand Quiz Bank</h1>
      <p class="lede">Sample questions from every part of the bank that <span class="mono">/quiz</span> will draw from in #trivia-informational. Typed questions forgive small slips and common Hinglish spellings; MCQs give one click. Answers are shown so you can spot anything wrong before it goes live.</p>
    </div>
    <div class="totals" id="totals"></div>
    <div class="filters" role="group" aria-label="Filter samples">
      <button data-filter="all" aria-pressed="true">All</button>
      <button data-filter="india" aria-pressed="false">🇮🇳 India</button>
      <button data-filter="world" aria-pressed="false">🌍 World</button>
      <button data-filter="text" aria-pressed="false">Typed</button>
      <button data-filter="mcq" aria-pressed="false">Multiple choice</button>
    </div>
  </header>
  <div id="groups" style="display:grid;gap:48px"></div>
</main>
<script>
const DATA = /*DATA*/null;
const el = (tag, cls, text) => { const n = document.createElement(tag); if (cls) n.className = cls; if (text != null) n.textContent = text; return n; };
const fmt = (n) => n.toLocaleString("en-IN");
const totals = document.getElementById("totals");
const all = DATA.india + DATA.world;
for (const [value, name] of [[all, "Questions"], [DATA.india, "India"], [DATA.world, "World"]]) {
  const t = el("div", "total"); t.append(el("b", null, fmt(value)), el("span", null, name)); totals.append(t);
}
const groups = document.getElementById("groups");
for (const group of DATA.groups) {
  const section = el("section", "group"); section.append(el("h2", null, group.title));
  for (const file of group.files) {
    const topic = el("div", "topic");
    const head = el("div", "topic-head");
    head.append(el("h3", null, file.name), el("span", "meta", `${fmt(file.count)} questions · ${Math.round(100 * file.typed / file.count)}% typed`));
    const cards = el("div", "cards");
    for (const q of file.samples) {
      const card = el("article", "card"); card.dataset.region = q.region; card.dataset.kind = q.kind;
      const chips = el("div", "chips");
      chips.append(el("span", "chip " + q.region, q.region === "india" ? "India" : "World"),
                   el("span", "chip kind", q.kind === "mcq" ? "MCQ" : "Typed"));
      if (q.diff) chips.append(el("span", "chip diff", q.diff));
      card.append(chips, el("p", "q", q.q));
      if (q.kind === "mcq") {
        const list = el("ol", "options");
        for (const o of q.options) list.append(el("li", o === q.a ? "right" : "", o));
        card.append(list);
      } else {
        const ans = el("div", "answer");
        const line = el("span"); line.append("Answer: ", el("b", null, q.a)); ans.append(line);
        if (q.alt && q.alt.length) ans.append(el("span", "alt", "Also accepted: " + q.alt.join(", ")));
        card.append(ans);
      }
      if (q.note) card.append(el("p", "note", q.note));
      card.append(el("span", "id mono", q.id));
      cards.append(card);
    }
    topic.append(head, cards); section.append(topic);
  }
  groups.append(section);
}
const buttons = document.querySelectorAll(".filters button");
for (const b of buttons) b.addEventListener("click", () => {
  buttons.forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
  const f = b.dataset.filter;
  document.querySelectorAll(".card").forEach((c) => {
    c.hidden = !(f === "all" || c.dataset.region === f || c.dataset.kind === f);
  });
});
</script>
"""

if __name__ == "__main__":
    main()
