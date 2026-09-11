"""Generate quiz questions from Wikidata for the MLCI Discord quiz bot.

    python3 wikidata_gen.py                  # all templates, spot checks, validation
    python3 wikidata_gen.py --only state_capital,airport_code
    python3 wikidata_gen.py --sample 12      # print 12 random questions per template
    python3 wikidata_gen.py --refresh        # ignore the SPARQL cache and re-query

Writes one JSONL file per template to quizbank/wikidata/. Raw SPARQL results are
cached in quizbank/wikidata/.cache/ (keyed by query hash), so re-runs are offline.

Every answer has to be right and unambiguous, because the first person to type it
scores. The rule throughout is: when the data is not clean, skip the item.
Standard library only.
"""

import argparse
import hashlib
import http.client
import json
import random
import re
import sys
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "wikidata"
CACHE = OUT / ".cache"
ENDPOINT = "https://query.wikidata.org/sparql"
UA = "MLCI-quizbank/0.1 (Discord community quiz)"
THIS_YEAR = 2026
REFRESH = False

PREFIXES = """PREFIX wd: <http://www.wikidata.org/entity/>
PREFIX wdt: <http://www.wikidata.org/prop/direct/>
PREFIX p: <http://www.wikidata.org/prop/>
PREFIX ps: <http://www.wikidata.org/prop/statement/>
PREFIX pq: <http://www.wikidata.org/prop/qualifier/>
PREFIX wikibase: <http://wikiba.se/ontology#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX skos: <http://www.w3.org/2004/02/skos/core#>
PREFIX schema: <http://schema.org/>
"""

KEYS = ["id", "kind", "q", "a", "alt", "options", "cat", "region", "diff", "note", "src"]

# ---------------------------------------------------------------------------
# SPARQL access: one query at a time, cached, with backoff
# ---------------------------------------------------------------------------

_last_live = [0.0]


def sparql(name, body):
    query = PREFIXES + body
    key = hashlib.sha1(query.encode()).hexdigest()[:12]
    path = CACHE / f"{name}-{key}.json"
    if path.exists() and not REFRESH:
        return json.loads(path.read_text())
    data = urllib.parse.urlencode({"query": query, "format": "json"}).encode()
    headers = {"User-Agent": UA, "Accept": "application/sparql-results+json"}
    for attempt in range(8):
        gap = time.time() - _last_live[0]
        if gap < 1.5:
            time.sleep(1.5 - gap)
        try:
            req = urllib.request.Request(ENDPOINT, data=data, headers=headers)
            with urllib.request.urlopen(req, timeout=120) as r:
                res = json.load(r)
            _last_live[0] = time.time()
            break
        except urllib.error.HTTPError as e:
            _last_live[0] = time.time()
            if e.code not in (429, 500, 502, 503, 504):
                raise RuntimeError(f"SPARQL {name}: HTTP {e.code}\n{e.read()[:500]!r}") from e
            retry_after = e.headers.get("Retry-After", "")
            wait = int(retry_after) if retry_after.isdigit() else min(240, 10 * 2 ** attempt)
            print(f"  [{name}] HTTP {e.code}, retry in {wait}s", flush=True)
            time.sleep(wait)
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.HTTPException) as e:
            _last_live[0] = time.time()
            wait = min(240, 10 * 2 ** attempt)
            print(f"  [{name}] {e!r}, retry in {wait}s", flush=True)
            time.sleep(wait)
    else:
        raise RuntimeError(f"SPARQL {name}: gave up")

    def simplify(b):
        v = b["value"]
        if b["type"] == "uri" and v.startswith("http://www.wikidata.org/entity/"):
            return v.rsplit("/", 1)[1]
        return v

    rows = [{k: simplify(v) for k, v in b.items()} for b in res["results"]["bindings"]]
    CACHE.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(rows, ensure_ascii=False))
    print(f"  [{name}] {len(rows)} rows (live)", flush=True)
    return rows


def values(qids):
    return " ".join(f"wd:{q}" for q in qids)


def batched(name, template, qids, size=200):
    """Run template (with {values}) over sorted QID chunks; concatenate rows."""
    qids = sorted(set(qids), key=lambda q: int(q[1:]))
    rows = []
    for i in range(0, len(qids), size):
        rows += sparql(f"{name}-{i // size}", template.replace("{values}", values(qids[i:i + size])))
    return rows


def qnum(q):
    return int(q[1:])


# ---------------------------------------------------------------------------
# Entity info: English label, English aliases, sitelinks
# ---------------------------------------------------------------------------

_info = {}


def info(qids):
    missing = [q for q in set(qids) if q not in _info]
    if missing:
        rows = batched("info", """
SELECT ?x ?label ?llang ?alias ?sl WHERE {
  VALUES ?x { {values} }
  OPTIONAL { ?x rdfs:label ?label FILTER(LANG(?label) IN ("en", "mul")) BIND(LANG(?label) AS ?llang) }
  OPTIONAL { ?x skos:altLabel ?alias FILTER(LANG(?alias) IN ("en", "mul")) }
  OPTIONAL { ?x wikibase:sitelinks ?sl }
}""", missing)
        # Wikidata now stores many names (people, the euro) only under the
        # language-neutral "mul" code; an explicit English label wins when present.
        for q in missing:
            _info[q] = {"label": None, "aliases": [], "sl": 0, "_lang": None}
        for r in rows:
            d = _info[r["x"]]
            if "label" in r and (d["_lang"] != "en"):
                d["label"], d["_lang"] = r["label"], r.get("llang")
            if "alias" in r and r["alias"] not in d["aliases"]:
                d["aliases"].append(r["alias"])
            if "sl" in r:
                d["sl"] = int(r["sl"])
        for q in missing:
            _info[q]["aliases"].sort()
    return {q: _info[q] for q in qids}


# ---------------------------------------------------------------------------
# Text helpers
# ---------------------------------------------------------------------------

def norm(s):
    s = unicodedata.normalize("NFKD", s)
    s = "".join(c for c in s if not unicodedata.combining(c))
    s = re.sub(r"[^0-9a-z]+", " ", s.casefold())
    return s.strip()


def clean_label(s):
    if not s:
        return None
    s = s.strip()
    s = re.sub(r"\s*\([^()]*\)$", "", s)          # "Devdas (2002 film)" -> "Devdas"
    s = re.sub(r"\s+(district|metropolitan area|metropolitan region)$", "", s, flags=re.I)
    if not s or "(" in s or ")" in s or re.fullmatch(r"Q\d+", s):
        return None
    return s[0].upper() + s[1:] if s[0].islower() else s


ABBREV_OK = {"USA", "US", "UK", "UAE"}


def latin(s):
    return all(ord(c) < 0x250 or c in "’‘–—" for c in s)


def is_code(s):
    if s in ABBREV_OK:
        return False
    if re.search(r"\d", s):
        return True
    if len(re.sub(r"[\s.]", "", s)) <= 3:          # "Kol", "Hyd", "Fe"
        return True
    if re.fullmatch(r"[A-Z.]{2,5}", s):
        return True
    if re.fullmatch(r"[A-Z]{2}-[A-Z0-9]+", s):
        return True
    return False


# Words that mark an alias as a nickname/epithet rather than a name
# ("Pink City", "Queen of the Deccan", "Capital Of Gujarat", "Land of the Rising Sun").
NICK_WORDS = {"city", "cities", "town", "land", "capital", "centre", "center", "valley", "queen", "king",
              "gateway", "paradise", "abode", "garden", "heaven", "jewel", "pearl", "pearls", "manchester",
              "oxford", "venice", "paris", "silicon", "constantinople", "golden", "pink", "blue", "sun",
              "whispering", "beautiful", "hills", "lakes", "joy", "palaces", "nawabs", "leather", "cotton",
              "banyan", "summer", "winter", "nation", "father", "mother", "dream", "girl", "mint", "seat",
              "heart", "province", "municipality", "area", "community", "urban", "administration",
              "communist", "smoke"}


def clean_aliases(aliases, answer, mode=None, drop=()):
    """English aliases a player could fairly type.

    mode "place": strip " district" suffixes, drop epithets.  mode "person": keep
    only aliases that share a name token with the label (drops "Dream Girl").
    drop: normalised aliases to discard (e.g. the name of a different place).
    """
    out, seen = [], {norm(answer)} | {norm(d) for d in drop}
    label_tokens = set(norm(answer).split())
    for al in aliases:
        al = al.strip()
        if mode == "place":
            al = re.sub(r"\s+(district|metropolitan area|metropolitan region)$", "", al, flags=re.I)
        if not al or "(" in al or ")" in al or "," in al or re.fullmatch(r"Q\d+", al):
            continue
        if len(al.split()) > 5 or is_code(al) or not latin(al):
            continue
        n = norm(al)
        if not n or n in seen:
            continue
        toks = set(n.split())
        if mode in ("place", "country") and (toks - label_tokens) & NICK_WORDS:
            continue
        if mode == "person" and not ({t for t in toks if len(t) >= 3} & {t for t in label_tokens if len(t) >= 3}):
            continue
        seen.add(n)
        out.append(al)
    return out


def alias_clashes(name, pairs):
    """{(answer_qid, norm alias)} where a DIFFERENT geographic item in the same
    country has that alias as its English label, e.g. "Patan" listed as an alias
    of Patna while Patan is a separate city in Gujarat."""
    pairs = {(q, a.strip()) for q, a in pairs if a and a.strip() and latin(a)}
    labels = sorted({a for _, a in pairs})
    holders = defaultdict(set)          # label -> {(item, country)}
    for i in range(0, len(labels), 100):
        chunk = labels[i:i + 100]
        for r in sparql(f"{name}-{i // 100}", """
SELECT ?lbl ?o ?c WHERE {
  VALUES ?lbl { %s }
  ?o rdfs:label ?lbl .
  ?o wikibase:sitelinks ?sl . FILTER(?sl >= 10)
  ?o wdt:P625 [] .
  OPTIONAL { ?o wdt:P17 ?c }
}""" % " ".join(sparql_str(t) for t in chunk)):
            holders[r["lbl"]].add((r["o"], r.get("c")))
    home = defaultdict(set)
    for r in batched(f"{name}-p17", "SELECT ?x ?c WHERE { VALUES ?x { {values} } ?x wdt:P17 ?c . }",
                     {q for q, _ in pairs}):
        home[r["x"]].add(r["c"])
    return {(q, norm(a)) for q, a in pairs
            if any(o != q and c in home[q] for o, c in holders.get(a, ()))}


def enwiki_titles(name, qids):
    """qid -> English Wikipedia article title with the disambiguator removed.
    Films and books are known by their article title ("Kabhi Khushi Kabhie Gham...")
    more reliably than by their Wikidata label (sometimes a translation)."""
    out = {}
    for r in batched(name, """
SELECT ?x ?t WHERE {
  VALUES ?x { {values} }
  ?art schema:about ?x ; schema:isPartOf <https://en.wikipedia.org/> ; schema:name ?t .
}""", qids):
        out[r["x"]] = clean_label(r["t"])
    return out


def appears(needle, hay):
    """Whole-word, accent/case-insensitive containment."""
    n, h = norm(needle), norm(hay)
    return bool(n) and f" {n} " in f" {h} "


def words(s):
    return len(s.split())


# ---------------------------------------------------------------------------
# Question assembly
# ---------------------------------------------------------------------------

class Skip(Exception):
    pass


def make(tpl, idpart, q, a, names, cat, region, sl, pool=None, kind="text", rng=None, mode=None, drop=()):
    """Build a row. names = all accepted names of the answer (label + aliases).

    kind "text" falls back to mcq when the answer is longer than 4 words.
    pool = ordered candidate distractor strings (preference order already applied).
    """
    rid = f"wd-{tpl}-{idpart}"
    alt = clean_aliases(names, a, mode=mode, drop=drop)
    if kind == "text" and words(a) > 4:
        kind = "mcq"
    # Only names a player could actually score with count as a leak; junk
    # aliases like "Hyd" are already dropped and must not veto "IATA code HYD".
    # For mcq only the option text itself matters.
    for n in [a] + (alt if kind == "text" else []):
        if appears(n, q):
            raise Skip(f"answer leaks into question: {n!r} in {q!r}")
    if kind == "text":
        options = []
    else:
        if pool is None:
            raise Skip("no distractor pool")
        bad = {norm(a)} | {norm(n) for n in names}
        options, seen = [a], set(bad)
        for cand in pool:
            c = norm(cand)
            if not c or c in seen:
                continue
            seen.add(c)
            options.append(cand)
            if len(options) == 4:
                break
        if len(options) < 4:
            raise Skip("not enough distractors")
        (rng or random.Random(rid)).shuffle(options)
        alt = []
    return {"id": rid, "kind": kind, "q": q, "a": a, "alt": alt, "options": options,
            "cat": cat, "region": region, "diff": None, "note": "", "src": "wikidata",
            "_sl": sl}


def ordered(rng, *groups):
    """Concatenate candidate groups, each shuffled; earlier groups preferred."""
    out = []
    for g in groups:
        g = list(g)
        rng.shuffle(g)
        out += g
    return out


def assign_diff(rows):
    rows.sort(key=lambda r: (-r["_sl"], r["id"]))
    n = len(rows)
    for i, r in enumerate(rows):
        r["diff"] = "easy" if i < n / 3 else ("medium" if i < 2 * n / 3 else "hard")
    rows.sort(key=lambda r: r["id"])
    return rows


class Log:
    def __init__(self, tpl):
        self.tpl, self.skips = tpl, defaultdict(list)

    def skip(self, why, what):
        self.skips[why].append(what)

    def report(self):
        for why, items in sorted(self.skips.items(), key=lambda kv: -len(kv[1])):
            shown = ", ".join(items[:8]) + (" ..." if len(items) > 8 else "")
            print(f"    skip {len(items):4d}  {why}: {shown}")


# ---------------------------------------------------------------------------
# India: states and union territories
# ---------------------------------------------------------------------------

STATE, UT = "Q12443800", "Q467745"
LABEL_OVERRIDE = {"Q9357528": "Delhi"}    # "National Capital Territory of Delhi"

# Capital is disputed, split, or has an official second capital: never ask.
DISPUTED_STATE_CAPITAL = {
    "Q66278313": "Jammu and Kashmir (Srinagar summer / Jammu winter)",
    "Q200667": "Ladakh (Leh and Kargil)",
    "Q1177": "Himachal Pradesh (Dharamshala winter capital)",
    "Q1499": "Uttarakhand (Gairsain summer capital)",
    "Q1159": "Andhra Pradesh (three-capitals dispute)",
    "Q1191": "Maharashtra (Nagpur is the official second capital)",
}
# Wikidata lists a 'winter capital' that only hosts a legislature session; the
# capital is not in doubt. Applied only if Wikidata still lists this capital.
STATE_CAPITAL_OVERRIDE = {"Q1185": "Q1355"}   # Karnataka -> Bengaluru (Belagavi = session seat)

_states = None


def india_states():
    """Current states/UTs: qid -> {label, kind, sl, capitals, neighbours}."""
    global _states
    if _states is not None:
        return _states
    rows = sparql("in_states", """
SELECT ?s ?cls WHERE {
  VALUES ?cls { wd:Q12443800 wd:Q467745 }
  ?s p:P31 ?ist . ?ist ps:P31 ?cls .
  FILTER NOT EXISTS { ?ist pq:P582 [] }
  FILTER NOT EXISTS { ?ist wikibase:rank wikibase:DeprecatedRank }
  FILTER NOT EXISTS { ?s wdt:P576 [] }
}""")
    kinds = {}
    for r in rows:
        kinds.setdefault(r["s"], set()).add(r["cls"])
    meta = info(kinds)
    st = {}
    for q, ks in kinds.items():
        if len(ks) != 1 or not meta[q]["label"]:
            continue
        st[q] = {"label": LABEL_OVERRIDE.get(q, meta[q]["label"]),
                 "kind": "state" if STATE in ks else "union territory",
                 "sl": meta[q]["sl"], "aliases": meta[q]["aliases"],
                 "capitals": set(), "neighbours": set()}
    for r in sparql("in_state_capitals", """
SELECT ?s ?cap WHERE {
  VALUES ?s { %s }
  ?s p:P36 ?st . ?st ps:P36 ?cap .
  FILTER NOT EXISTS { ?st pq:P582 [] }
  FILTER NOT EXISTS { ?st wikibase:rank wikibase:DeprecatedRank }
}""" % values(st)):
        st[r["s"]]["capitals"].add(r["cap"])
    for r in sparql("in_state_borders", """
SELECT ?s ?n WHERE { VALUES ?s { %s } ?s wdt:P47 ?n . }""" % values(st)):
        if r["n"] in st:
            st[r["s"]]["neighbours"].add(r["n"])
    _states = st
    return st


def resolve_states(name, qids):
    """qid -> set of current state/UT qids reachable through P131*."""
    st = india_states()
    out = {q: set() for q in qids}
    tmpl = """
SELECT ?x ?st WHERE {
  VALUES ?x { {values} }
  VALUES ?st { %s }
  ?x wdt:P131* ?st .
}""" % values(st)
    for r in batched(name, tmpl, qids, size=100):
        out[r["x"]].add(r["st"])
    return out


def state_pool(rng, answer_state, neighbours=True):
    """Distractor state names of the same kind (state/UT).

    neighbours=True: up to 2 bordering states first (plausible for point places).
    neighbours=False: bordering states are never offered (for parks/reserves that
    may continue across a border, e.g. Pench in MP and Maharashtra)."""
    st = india_states()
    kind = st[answer_state]["kind"]
    same = [q for q in st if q != answer_state and st[q]["kind"] == kind]
    nb = [q for q in same if q in st[answer_state]["neighbours"]]
    if not neighbours:
        return [st[q]["label"] for q in ordered(rng, [q for q in same if q not in nb])]
    rng.shuffle(nb)
    rest = [q for q in same if q not in nb[:2]]
    return [st[q]["label"] for q in ordered(rng, nb[:2]) + ordered(rng, rest)]


def state_names(q):
    s = india_states()[q]
    return [s["label"]] + s["aliases"]


# ---------------------------------------------------------------------------
# Templates: INDIA
# ---------------------------------------------------------------------------

def t_state_capital(log):
    st = india_states()
    tpl = "state_capital"
    caps = info({c for s in st.values() for c in s["capitals"]})
    cap_of = defaultdict(set)
    for q, s in st.items():
        for c in s["capitals"]:
            cap_of[c].add(q)
    clashes = alias_clashes("state_capital_alias_clash",
                            [(c, al) for c in caps for al in caps[c]["aliases"]])
    rows = []
    for q, s in sorted(st.items(), key=lambda kv: qnum(kv[0])):
        if q in DISPUTED_STATE_CAPITAL:
            log.skip("disputed/multiple capitals (hand list)", s["label"])
            continue
        capset = s["capitals"]
        if q in STATE_CAPITAL_OVERRIDE and STATE_CAPITAL_OVERRIDE[q] in capset:
            capset = {STATE_CAPITAL_OVERRIDE[q]}
        if len(capset) != 1:
            log.skip("≠1 current capital", f"{s['label']}={len(capset)}")
            continue
        c = next(iter(capset))
        clab = clean_label(caps[c]["label"])
        if not clab:
            log.skip("no usable capital label", s["label"])
            continue
        if any(appears(n, clab) for n in state_names(q)) or appears(clab, s["label"]):
            log.skip("state name inside capital name", f"{s['label']}→{clab}")
            continue
        names = [clab] + caps[c]["aliases"]
        if any(appears(n, s["label"]) for n in names):
            log.skip("capital alias inside state name", f"{s['label']}→{clab}")
            continue
        rng = random.Random(f"{tpl}-{q}")
        try:
            rows.append(make(tpl, f"{q}", f"What is the capital of the Indian {s['kind']} of {s['label']}?",
                             clab, names, "geography", "india", s["sl"], mode="place",
                             drop=[al for al in caps[c]["aliases"] if (c, norm(al)) in clashes]))
        except Skip as e:
            log.skip(str(e).split(":")[0], s["label"])
        if len(cap_of[c]) != 1:
            log.skip("reverse: capital of >1 state/UT", clab)
            continue
        try:
            rows.append(make(tpl, f"{c}-{q}", f"{clab} is the capital of which Indian {s['kind']}?",
                             s["label"], state_names(q), "geography", "india", caps[c]["sl"],
                             pool=state_pool(rng, q), kind="mcq", rng=rng))
        except Skip as e:
            log.skip("reverse: " + str(e).split(":")[0], clab)
    return rows


def _place_state_template(tpl, log, subjects, question, cat, exclude_labels=(), extra_states=None,
                          neighbours=True):
    """subjects: qid -> sitelinks. question: fn(label, kind) -> text.
    extra_states: qid -> states of its parts (a serial site spans all of them)."""
    st = india_states()
    meta = info(subjects)
    res = resolve_states(f"{tpl}_p131", subjects)
    for q, extra in (extra_states or {}).items():
        if q in res:
            res[q] = res[q] | extra
    by_label = defaultdict(set)
    for q in subjects:
        lab = clean_label(meta[q]["label"])
        if lab:
            by_label[norm(lab)].add(q)
    rows = []
    for q in sorted(subjects, key=qnum):
        lab = clean_label(meta[q]["label"])
        if not lab:
            log.skip("no English label", q)
            continue
        if len(by_label[norm(lab)]) > 1:
            log.skip("label shared by several items", lab)
            continue
        if norm(lab) in exclude_labels:
            log.skip("already used elsewhere", lab)
            continue
        sts = res[q]
        if len(sts) != 1:
            log.skip(f"resolves to {len(sts)} states", lab)
            continue
        s = next(iter(sts))
        rng = random.Random(f"{tpl}-{q}")
        try:
            rows.append(make(tpl, q, question(lab, st[s]["kind"]), st[s]["label"], state_names(s),
                             cat, "india", subjects[q], pool=state_pool(rng, s, neighbours), kind="mcq",
                             rng=rng))
        except Skip as e:
            log.skip(str(e).split(":")[0], lab)
    return rows


def all_capital_labels():
    st = india_states()
    caps = info({c for s in st.values() for c in s["capitals"]})
    return {norm(clean_label(caps[c]["label"]) or "") for c in caps} - {""}, set(caps)


HERITAGE_QUERY = """
SELECT ?x ?sl WHERE {
  ?x wdt:P1435 wd:Q9259 ; wdt:P17 wd:Q668 ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 12)
}"""


def t_heritage_state(log):
    subjects = {r["x"]: int(r["sl"]) for r in sparql("heritage", HERITAGE_QUERY)}
    # Transnational serial sites (Le Corbusier's works) are not "in an Indian state".
    # Only present-day countries count: Red Fort also lists the Mughal Empire.
    today = set(countries()) - {"Q668"}
    abroad = {r["x"] for r in batched("heritage_countries", """
SELECT ?x ?c WHERE { VALUES ?x { {values} } ?x wdt:P17 ?c . FILTER(?c != wd:Q668) }""", subjects)
              if r["c"] in today}
    for q in abroad:
        log.skip("transnational site", q)
        del subjects[q]
    cap_labels, cap_qids = all_capital_labels()
    for q in list(subjects):
        if q in cap_qids:
            log.skip("site is a state capital city", q)
            del subjects[q]
    # Parts of serial sites: their states count too (Maratha Military Landscapes
    # includes Gingee Fort in Tamil Nadu), and a site that is part of another
    # listed site (Qutb Minar / Qutb complex) keeps only the better-known one.
    parts = defaultdict(set)
    for r in batched("heritage_parts", """
SELECT ?x ?part WHERE { VALUES ?x { {values} } { ?x wdt:P527 ?part } UNION { ?part wdt:P361 ?x } }""", subjects):
        parts[r["x"]].add(r["part"])
    part_states = resolve_states("heritage_part_p131", {p for ps in parts.values() for p in ps})
    extra = {q: set().union(*(part_states[p] for p in ps)) for q, ps in parts.items()}
    for q, ps in parts.items():
        for p in ps & set(subjects):
            if q in subjects and p in subjects:
                loser = q if subjects[q] < subjects[p] else p
                log.skip("part of / contains another listed site", loser)
                del subjects[loser]
    return _place_state_template(
        "heritage_state", log, subjects,
        lambda lab, kind: f"In which Indian {kind} is the World Heritage Site {lab}?", "geography",
        extra_states=extra)


def t_park_state(log):
    rows = sparql("parks", """
SELECT DISTINCT ?x ?sl WHERE {
  VALUES ?cls { wd:Q46169 wd:Q5533772 }
  ?x wdt:P31 ?cls ; wdt:P17 wd:Q668 ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 8)
  FILTER NOT EXISTS { ?x wdt:P576 [] }
}""")
    subjects = {r["x"]: int(r["sl"]) for r in rows}
    her = {r["x"] for r in sparql("heritage", HERITAGE_QUERY)}
    for q in list(subjects):
        if q in her:
            log.skip("already in heritage_state", q)
            del subjects[q]
    return _place_state_template(
        "park_state", log, subjects, lambda lab, kind: f"In which Indian {kind} is {lab}?", "wildlife",
        neighbours=False)


# Items Wikidata lists as separate places served that a player treats as one.
SAME_PLACE = {"Q987": "Q1353",      # New Delhi -> Delhi (alt keeps "New Delhi")
              "Q955990": "Q1352"}   # Chennai metropolitan area -> Chennai


def t_airport_code(log):
    tpl = "airport_code"
    rows = sparql("airports", """
SELECT ?x ?code ?sl ?v WHERE {
  ?x wdt:P238 ?code ; wdt:P17 wd:Q668 ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 10)
  FILTER EXISTS { ?x wdt:P31/wdt:P279* wd:Q1248784 }
  FILTER NOT EXISTS { ?x wdt:P576 [] }
  FILTER NOT EXISTS { ?x wdt:P3999 [] }
  OPTIONAL { ?x wdt:P931 ?v }
}""")
    ap = {}
    for r in rows:
        d = ap.setdefault(r["x"], {"codes": set(), "sl": int(r["sl"]), "served": set()})
        d["codes"].add(r["code"])
        if "v" in r:
            d["served"].add(r["v"])
    codes = sorted({c for d in ap.values() for c in d["codes"]})
    holders = defaultdict(set)
    for r in sparql("airport_code_holders", """
SELECT ?y ?code WHERE {
  VALUES ?code { %s }
  ?y wdt:P238 ?code .
  FILTER EXISTS { ?y wdt:P31/wdt:P279* wd:Q1248784 }
  FILTER NOT EXISTS { ?y wdt:P576 [] }
  FILTER NOT EXISTS { ?y wdt:P3999 [] }
}""" % " ".join(json.dumps(c) for c in codes)):
        holders[r["code"]].add(r["y"])
    served = {v for d in ap.values() for v in d["served"]}
    st = india_states()
    klass = defaultdict(set)
    for r in batched("airport_served_class", """
SELECT DISTINCT ?v ?k WHERE {
  VALUES ?v { {values} }
  VALUES ?k { wd:Q486972 wd:Q56061 }
  ?v wdt:P31/wdt:P279* ?k .
}""", served):
        klass[r["v"]].add(r["k"])
    settlements = {v for v in served if "Q486972" in klass[v]}
    admins = {v for v in served if "Q56061" in klass[v]} - settlements
    inside = defaultdict(set)
    for r in batched("airport_served_p131", """
SELECT ?v ?anc WHERE { VALUES ?v { {values} } ?v wdt:P131+ ?anc . }""", served):
        inside[r["v"]].add(r["anc"])
    res = resolve_states(f"{tpl}_p131", set(ap) | served | set(SAME_PLACE.values()))
    meta = info(set(ap) | served | set(SAME_PLACE.values()))

    cands = []
    for x, d in sorted(ap.items(), key=lambda kv: qnum(kv[0])):
        alab = meta[x]["label"] or x
        if len(d["codes"]) != 1:
            log.skip("airport has ≠1 IATA code", alab)
            continue
        code = next(iter(d["codes"]))
        if not re.fullmatch(r"[A-Z]{3}", code):
            log.skip("malformed IATA code", f"{alab} {code}")
            continue
        if len(holders[code]) != 1:
            log.skip("IATA code held by several items", code)
            continue
        raw = {v for v in d["served"] if v not in st}          # a state (Goa) is not a city
        towns = {SAME_PLACE.get(v, v) for v in raw if v in settlements}
        # Prefer the city over its district; drop neighbourhoods inside another served city.
        vs = towns or {v for v in raw if v in admins}
        vs = {v for v in vs if not (inside[v] & (vs - {v}))}
        if len(vs) != 1:
            log.skip(f"serves {len(vs)} places", f"{code} {alab}")
            continue
        city = next(iter(vs))
        clab = clean_label(meta[city]["label"])
        if not clab:
            log.skip("no usable city label", code)
            continue
        if len(res[x]) != 1 or res[x] != res[city]:
            log.skip("airport and city in different states", f"{code}→{clab}")
            continue
        if meta[city]["sl"] < 15:
            log.skip("served place too obscure (<15 sitelinks)", f"{code}→{clab}")
            continue
        extra = [meta[k]["label"] for k, v in SAME_PLACE.items() if v == city]
        names = [clab] + meta[city]["aliases"] + extra
        named_after_city = any(appears(n, alab) for n in names)
        if not named_after_city and not (d["sl"] >= 20 and meta[city]["sl"] >= 30):
            log.skip("minor airport not named after its city", f"{code} {alab}→{clab}")
            continue
        cands.append((x, code, city, clab, names, d["sl"]))
    clashes = alias_clashes("airport_alias_clash", [(c[2], al) for c in cands for al in meta[c[2]]["aliases"]])
    pool = sorted({c[3] for c in cands})
    rows = []
    for x, code, city, clab, names, sl in cands:
        rng = random.Random(f"{tpl}-{x}")
        try:
            rows.append(make(tpl, x, f"Which city's airport has the IATA code {code}?", clab, names,
                             "geography", "india", sl, pool=ordered(rng, pool), rng=rng, mode="place",
                             drop=[al for al in meta[city]["aliases"] if (city, norm(al)) in clashes]))
        except Skip as e:
            log.skip(str(e).split(":")[0], code)
    return rows


_films = None


def hindi_films():
    global _films
    if _films is not None:
        return _films
    rows = sparql("hindi_films", """
SELECT ?f ?sl ?d ?date WHERE {
  ?f wdt:P31 wd:Q11424 ; wdt:P364 wd:Q1568 ; wdt:P495 wd:Q668 ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 15)
  OPTIONAL { ?f wdt:P57 ?d }
  OPTIONAL { ?f p:P577 ?pst . ?pst ps:P577 ?date .
             FILTER NOT EXISTS { ?pst wikibase:rank wikibase:DeprecatedRank } }
}""")
    films = {}
    for r in rows:
        f = films.setdefault(r["f"], {"sl": int(r["sl"]), "directors": set(), "years": set()})
        if "d" in r:
            f["directors"].add(r["d"])
        m = re.match(r"^(\d{4})-", r.get("date", ""))
        if m:
            f["years"].add(int(m.group(1)))
    for r in batched("hindi_film_lang", """
SELECT ?f ?lang ?ctry WHERE { VALUES ?f { {values} } { ?f wdt:P364 ?lang } UNION { ?f wdt:P495 ?ctry } }""", films):
        f = films[r["f"]]
        if "lang" in r:
            f.setdefault("langs", set()).add(r["lang"])
        if "ctry" in r:
            f.setdefault("countries", set()).add(r["ctry"])
    titles = enwiki_titles("hindi_film_enwiki", films)
    for q, f in films.items():
        # Only the English Wikipedia title: Wikidata labels are sometimes translations
        # ("Sometimes Happiness Sometimes Sadness..." for Kabhi Khushi Kabhie Gham...).
        f["label"] = titles.get(q)
        langs, ctry = f.get("langs", set()), f.get("countries", set())
        other = langs - HINDI_LANGS - {ENGLISH}
        f["not_hindi"] = ("also in " + ",".join(sorted(other))) if other else (
            "English-language co-production" if ENGLISH in langs and ctry - {"Q668"} else "")
    # Every Indian film (any popularity) carrying the same title: Devdas 1936/1955/2002.
    namesakes = defaultdict(set)     # norm title -> {(film, earliest year or None)}
    tl = sorted({f["label"] for f in films.values() if f["label"]})
    for i in range(0, len(tl), 80):
        yrs = defaultdict(set)
        for r in sparql(f"hindi_film_namesakes-{i // 80}", """
SELECT ?lbl ?o ?d WHERE {
  VALUES ?lbl { %s }
  ?o rdfs:label ?lbl ; wdt:P31 wd:Q11424 ; wdt:P495 wd:Q668 .
  OPTIONAL { ?o wdt:P577 ?d }
}""" % " ".join(sparql_str(t) for t in tl[i:i + 80])):
            m = re.match(r"^(\d{4})-", r.get("d", ""))
            yrs[(norm(r["lbl"]), r["o"])].add(int(m.group(1)) if m else None)
        for (t, o), ys in yrs.items():
            known = [y for y in ys if y]
            namesakes[t].add((o, min(known) if known else None))
    for q, f in films.items():
        others = {(o, y) for o, y in namesakes[norm(f["label"])] if o != q} if f["label"] else set()
        f["namesakes"] = others
    _films = films
    return films


HINDI_LANGS = {"Q1568", "Q1617", "Q11051"}   # Hindi, Urdu, Hindustani
ENGLISH = "Q1860"


def t_hindi_film_director(log):
    tpl = "hindi_film_director"
    films = hindi_films()
    dmeta = info({d for f in films.values() for d in f["directors"]})
    key_count = defaultdict(int)
    for f in films.values():
        if f["label"] and f["years"]:
            key_count[(norm(f["label"]), min(f["years"]))] += 1
    usable = {}
    for q, f in sorted(films.items(), key=lambda kv: qnum(kv[0])):
        if not f["label"]:
            log.skip("no usable English Wikipedia title", q)
            continue
        if f["not_hindi"]:
            log.skip("not a Hindi-language film", f"{f['label']} ({f['not_hindi']})")
            continue
        if len(f["directors"]) != 1:
            log.skip(f"≠1 director", f"{f['label']}={len(f['directors'])}")
            continue
        if not f["years"]:
            log.skip("no release year", f["label"])
            continue
        if key_count[(norm(f["label"]), min(f["years"]))] > 1 or any(
                y is None or y == min(f["years"]) for _, y in f["namesakes"]):
            log.skip("another Indian film with this title in the same (or unknown) year", f["label"])
            continue
        d = next(iter(f["directors"]))
        if not clean_label(dmeta[d]["label"]):
            log.skip("director has no English label", f["label"])
            continue
        usable[q] = d
    dyears = defaultdict(set)
    for q, d in usable.items():
        dyears[d].add(min(films[q]["years"]))
    rows = []
    for q, d in usable.items():
        f = films[q]
        year = min(f["years"])
        dlab = clean_label(dmeta[d]["label"])
        rng = random.Random(f"{tpl}-{q}")
        near = [clean_label(dmeta[o]["label"]) for o in dyears if o != d and any(abs(y - year) <= 10 for y in dyears[o])]
        far = [clean_label(dmeta[o]["label"]) for o in dyears if o != d]
        kind = "text" if dmeta[d]["sl"] >= 10 else "mcq"
        try:
            rows.append(make(tpl, q, f"Who directed the Hindi film {f['label']} ({year})?", dlab,
                             [dlab] + dmeta[d]["aliases"], "bollywood", "india", f["sl"],
                             pool=ordered(rng, near, far), kind=kind, rng=rng, mode="person"))
        except Skip as e:
            log.skip(str(e).split(":")[0], f["label"])
    return rows


def t_hindi_film_year(log):
    tpl = "hindi_film_year"
    films = hindi_films()
    labels = defaultdict(int)
    for f in films.values():
        if f["sl"] >= 25 and f["label"]:
            labels[norm(f["label"])] += 1
    rows = []
    for q, f in sorted(films.items(), key=lambda kv: qnum(kv[0])):
        if f["sl"] < 25:
            continue
        if not f["label"]:
            log.skip("no usable English Wikipedia title", q)
            continue
        if f["not_hindi"]:
            log.skip("not a Hindi-language film", f"{f['label']} ({f['not_hindi']})")
            continue
        if not f["years"]:
            log.skip("no release year", f["label"])
            continue
        if labels[norm(f["label"])] > 1 or f["namesakes"]:
            log.skip("title shared with another Indian film", f["label"])
            continue
        ys = sorted(f["years"])
        if len(ys) > 1 and ys[1] - ys[0] <= 1:
            log.skip("release dates straddle adjacent years", f"{f['label']} {ys}")
            continue
        year = ys[0]
        rng = random.Random(f"{tpl}-{q}")
        cands = [y for y in range(year - 8, year + 9) if y not in f["years"] and y <= THIS_YEAR]
        try:
            rows.append(make(tpl, q, f"In which year was the Hindi film {f['label']} released?", str(year),
                             [], "bollywood", "india", f["sl"], pool=ordered(rng, map(str, cands)),
                             kind="mcq", rng=rng))
        except Skip as e:
            log.skip(str(e).split(":")[0], f["label"])
    return rows


def sparql_str(s):
    """The string as both an English and a language-neutral ("mul") label."""
    lit = json.dumps(s, ensure_ascii=False)
    return f"{lit}@en {lit}@mul"


MAX_PER_AUTHOR = 4


def t_indian_book_author(log):
    tpl = "indian_book_author"
    rows = sparql("books", """
SELECT DISTINCT ?w ?sl WHERE {
  VALUES ?cit { wd:Q668 wd:Q129286 }
  ?au wdt:P27 ?cit .
  ?w wdt:P50 ?au ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 8)
  FILTER EXISTS { ?w wdt:P31/wdt:P279* wd:Q7725634 }
}""")
    works = {r["w"]: int(r["sl"]) for r in rows}
    detail = defaultdict(lambda: {"authors": set(), "strings": 0, "edition": False, "religious": False})
    for r in batched("book_detail", """
SELECT ?w ?a ?s ?ed ?rel WHERE {
  VALUES ?w { {values} }
  { ?w wdt:P50 ?a } UNION { ?w wdt:P2093 ?s } UNION { ?w wdt:P629 ?ed }
  UNION { ?w wdt:P31/wdt:P279* wd:Q179461 . BIND(1 AS ?rel) }
  UNION { ?w wdt:P136/wdt:P279* wd:Q179461 . BIND(1 AS ?rel) }
}""", works):
        d = detail[r["w"]]
        if "a" in r:
            d["authors"].add(r["a"])
        if "s" in r:
            d["strings"] += 1
        if "ed" in r:
            d["edition"] = True
        if "rel" in r:
            d["religious"] = True
    authors = {a for d in detail.values() for a in d["authors"]}
    born, citizen, theologian = defaultdict(set), defaultdict(set), set()
    for r in batched("book_author_born", """
SELECT ?a ?b ?cit ?theo WHERE {
  VALUES ?a { {values} }
  { ?a wdt:P569 ?b } UNION { ?a wdt:P27 ?cit }
  UNION { ?a wdt:P106/wdt:P279* wd:Q1234713 . BIND(1 AS ?theo) }
}""", authors):
        m = re.match(r"^(\d{4})-", r.get("b", ""))
        if m:
            born[r["a"]].add(int(m.group(1)))
        if "cit" in r:
            citizen[r["a"]].add(r["cit"])
        if "theo" in r:
            theologian.add(r["a"])
    wmeta, ameta = info(works), info(authors)
    titles = enwiki_titles("book_enwiki", works)

    cands = []
    for w in sorted(works, key=qnum):
        title = titles.get(w)
        d = detail[w]
        if not title:
            log.skip("no usable English Wikipedia title", clean_label(wmeta[w]["label"]) or w)
            continue
        if d["edition"]:
            log.skip("edition/translation item", title)
            continue
        if d["religious"]:
            log.skip("religious text", title)
            continue
        if len(d["authors"]) != 1 or d["strings"]:
            log.skip("≠1 author", title)
            continue
        a = next(iter(d["authors"]))
        alab = clean_label(ameta[a]["label"])
        if not alab:
            log.skip("author has no English label", title)
            continue
        if not born[a] or min(born[a]) < 1800:
            log.skip("author not a modern Indian writer (no birth date ≥1800)", f"{title}/{alab}")
            continue
        if citizen[a] & {"Q843", "Q902"} or not citizen[a] & {"Q668", "Q129286"}:
            log.skip("author is Pakistani/Bangladeshi or not Indian", f"{title}/{alab}")
            continue
        # Obscure theological works (fatwa collections, polemics) slip in as "literary
        # work"; widely known figures who are also tagged theologian (Ambedkar) stay.
        if a in theologian and ameta[a]["sl"] < 100:
            log.skip("theological work by a lesser-known theologian", f"{title}/{alab}")
            continue
        names = [alab] + ameta[a]["aliases"]
        if norm(title) == norm(alab) or any(appears(n, title) for n in names) or appears(title, alab):
            log.skip("title contains / equals author name", title)
            continue
        cands.append((w, title, a, alab, names))

    # A title that another work (by a different author) also carries is ambiguous.
    clash = set()
    titles = sorted({c[1] for c in cands})
    for i in range(0, len(titles), 80):
        chunk = titles[i:i + 80]
        for r in sparql(f"book_title_clash-{i // 80}", """
SELECT ?lbl ?o ?a ?ed WHERE {
  VALUES ?lbl { %s }
  ?o rdfs:label ?lbl ; wdt:P50 ?a .
  OPTIONAL { ?o wdt:P629 ?ed }
}""" % " ".join(sparql_str(t) for t in chunk)):
            clash.add((norm(r["lbl"]), r["o"], r["a"], r.get("ed")))
    by_title = defaultdict(lambda: defaultdict(lambda: {"authors": set(), "editions_of": set()}))
    for t, o, au, ed in clash:
        if not re.fullmatch(r"Q\d+", au):     # "unknown value" author (blank node)
            continue
        by_title[t][o]["authors"].add(au)
        if ed:
            by_title[t][o]["editions_of"].add(ed)
    pool = sorted({c[3] for c in cands})
    per_author = defaultdict(int)
    rows = []
    for w, title, a, alab, names in sorted(cands, key=lambda c: (-works[c[0]], qnum(c[0]))):
        if per_author[a] >= MAX_PER_AUTHOR:
            log.skip(f"author already has {MAX_PER_AUTHOR} questions", f"{title}/{alab}")
            continue
        # Another work of this title is a real clash only if it is not an edition or
        # translation of this work and our author is not among its authors.
        others = [o for o, d in by_title[norm(title)].items()
                  if o != w and not d["editions_of"] and d["authors"] and a not in d["authors"]]
        if others:
            log.skip("another work with this title has a different author", title)
            continue
        rng = random.Random(f"{tpl}-{w}")
        try:
            rows.append(make(tpl, w, f"Who wrote {title}?", alab, names, "literature", "india", works[w],
                             pool=ordered(rng, pool), rng=rng, mode="person"))
            per_author[a] += 1
        except Skip as e:
            log.skip(str(e).split(":")[0], title)
    return rows


# Mouths that are seas/bays/lakes (terminal). A river flowing into a western body
# never gets an eastern body as the right answer and vice versa, so distractors
# are drawn from the other coast; nested bodies (Gulf of Khambhat inside the
# Arabian Sea) can then never be offered against each other.
WEST_WATER = {"arabian sea", "gulf of khambhat", "gulf of kutch", "rann of kutch", "vembanad",
              "vembanad lake", "laccadive sea", "lakshadweep sea"}
EAST_WATER = {"bay of bengal", "andaman sea", "palk bay", "palk strait", "gulf of mannar", "chilika lake"}
EAST_EXTRA = ["Bay of Bengal", "Andaman Sea", "Palk Bay", "Gulf of Mannar"]
WEST_EXTRA = ["Arabian Sea", "Gulf of Khambhat", "Gulf of Kutch"]
# The Ganges, Brahmaputra and Meghna share one delta: none may be a distractor
# for another's tributaries (the Brahmaputra "flows into" all three, by name).
GBM_DELTA = {"ganges", "padma river", "padma", "meghna river", "meghna", "hooghly river", "hooghly",
             "brahmaputra river", "brahmaputra", "jamuna river", "bhagirathi river"}


def t_river_mouth(log):
    tpl = "river_mouth"
    rows = sparql("rivers", """
SELECT ?r ?sl ?m WHERE {
  ?r wdt:P31 wd:Q4022 ; wdt:P17 wd:Q668 ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 20)
  OPTIONAL { ?r wdt:P403 ?m }
}""")
    rivers = {}
    for r in rows:
        d = rivers.setdefault(r["r"], {"sl": int(r["sl"]), "mouths": set()})
        if "m" in r:
            d["mouths"].add(r["m"])
    nodes = set(rivers) | {m for d in rivers.values() for m in d["mouths"]}
    down = defaultdict(set)   # node -> immediate P403 targets, for the downstream walk
    frontier, walked = set(nodes), set()
    for depth in range(8):    # level by level: a single P403* path query times out
        frontier -= walked
        if not frontier:
            break
        walked |= frontier
        nxt = set()
        for r in batched(f"river_down{depth}", """
SELECT ?x ?y WHERE { VALUES ?x { {values} } ?x wdt:P403 ?y . }""", frontier):
            down[r["x"]].add(r["y"])
            nxt.add(r["y"])
        frontier = nxt
    meta = info(nodes | {y for ys in down.values() for y in ys})
    lab = {q: clean_label(meta[q]["label"]) for q in meta}

    def basin(q, seen=()):
        """(coast, root river) of q by walking P403 to the sea; None if unclear."""
        n = norm(lab.get(q) or "")
        if n in WEST_WATER:
            return ("west", None)
        if n in EAST_WATER:
            return ("east", None)
        nxt = down.get(q, set())
        if len(nxt) != 1 or q in seen:
            return None
        b = basin(next(iter(nxt)), seen + (q,))
        if not b:
            return None
        root = b[1] or q
        return (b[0], "gbm" if norm(lab.get(root) or "") in GBM_DELTA else root)

    def river_phrase(label):
        return f"the {label}" if re.search(r"\briver$", label, re.I) else f"the {label} river"

    rows = []
    for q in sorted(rivers, key=qnum):
        d = rivers[q]
        rl = lab.get(q)
        if not rl:
            log.skip("no English label", q)
            continue
        if len(d["mouths"]) != 1:
            log.skip("≠1 mouth", f"{rl}={len(d['mouths'])}")
            continue
        m = next(iter(d["mouths"]))
        ml = lab.get(m)
        b = basin(q)
        if not ml or not b:
            log.skip("mouth chain unclear", rl)
            continue
        coast, root = b
        rng = random.Random(f"{tpl}-{q}")
        terminal = norm(ml) in WEST_WATER | EAST_WATER
        if terminal:
            question = f"Into which body of water does {river_phrase(rl)} flow?"
            other = [t for t in ([lab[x] for x in meta if lab.get(x) and norm(lab[x]) in (EAST_WATER if coast == "west" else WEST_WATER)]
                                 + (EAST_EXTRA if coast == "west" else WEST_EXTRA))]
            pool = ordered(rng, sorted(set(other)))
        else:
            question = f"Into which river does {river_phrase(rl)} flow?"
            pool = []
            for o in sorted(nodes, key=qnum):
                ol = lab.get(o)
                if not ol or o in (q, m) or norm(ol) in WEST_WATER | EAST_WATER:
                    continue
                ob = basin(o)
                if not ob or ob[0] == coast:   # other coast only: cannot share a river system
                    continue
                pool.append(ol)
            pool = ordered(rng, sorted(set(pool)))
        try:
            rows.append(make(tpl, q, question, ml, [ml] + meta[m]["aliases"], "geography", "india", d["sl"],
                             pool=pool, kind="mcq", rng=rng))
        except Skip as e:
            log.skip(str(e).split(":")[0], rl)
    return rows


def t_city_state(log):
    tpl = "city_state"
    rows = sparql("cities", """
SELECT DISTINCT ?c ?sl WHERE {
  ?c wdt:P17 wd:Q668 ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 30)
  FILTER EXISTS { ?c wdt:P31/wdt:P279* wd:Q515 }
  FILTER NOT EXISTS { ?c wdt:P576 [] }
}""")
    st = india_states()
    subjects = {r["c"]: int(r["sl"]) for r in rows if r["c"] not in st}
    cap_labels, cap_qids = all_capital_labels()
    meta = info(subjects)
    res = resolve_states("city_state_p131", subjects)
    for q in list(subjects):
        lab = clean_label(meta[q]["label"])
        if q in cap_qids or (lab and norm(lab) in cap_labels):
            log.skip("state capital", lab or q)
            del subjects[q]
    # Another Indian place with the same English name in a different state?
    labels = sorted({clean_label(meta[q]["label"]) for q in subjects if clean_label(meta[q]["label"])})
    others = defaultdict(set)
    for i in range(0, len(labels), 60):
        chunk = labels[i:i + 60]
        for r in sparql(f"city_label_clash-{i // 60}", """
SELECT ?lbl ?o ?st WHERE {
  VALUES ?lbl { %s }
  VALUES ?st { %s }
  ?o rdfs:label ?lbl ; wdt:P17 wd:Q668 ; wikibase:sitelinks ?osl .
  FILTER(?osl >= 5)
  ?o wdt:P131* ?st .
}""" % (" ".join(sparql_str(t) for t in chunk), values(st))):
            others[norm(r["lbl"])].add((r["o"], r["st"]))
    kept = {}
    for q in sorted(subjects, key=qnum):
        lab = clean_label(meta[q]["label"])
        if lab and len(res[q]) == 1 and any(o != q and ost not in res[q] for o, ost in others[norm(lab)]):
            log.skip("another Indian place of this name is in a different state", lab)
            continue
        kept[q] = subjects[q]
    return _place_state_template(tpl, log, kept, lambda lab, kind: f"In which Indian {kind} is the city of {lab}?",
                                 "geography")


# ---------------------------------------------------------------------------
# Templates: WORLD
# ---------------------------------------------------------------------------

# Capital itself is internationally contested: never ask.
DISPUTED_COUNTRY_CAPITAL = {
    "Q801": "Israel (Jerusalem status)",
    "Q219060": "State of Palestine",
    # Capital recently moved; the old one is still what most players would type.
    "Q983": "Equatorial Guinea (Malabo -> Ciudad de la Paz)",
    "Q967": "Burundi (Bujumbura -> Gitega)",
}
# Statehood itself is politically contested: never a question, answer or distractor.
EXCLUDED_COUNTRIES = {"Q865": "Taiwan"}
COUNTRY_Q_NAME = {"Q230": "Georgia (the country)"}
# Only the realm items carry "sovereign state" + UN membership on Wikidata; use the
# everyday name (the realm label stays an accepted alias).
COUNTRY_LABEL = {"Q756617": "Denmark", "Q29999": "Netherlands", "Q148": "China"}

_THE = re.compile(r"(United |Republic |Democratic Republic|Federated |Central African|Dominican Republic|"
                  r"Czech Republic|People's |Philippines|Netherlands|Maldives|Solomon Islands|"
                  r"Marshall Islands|Comoros|Bahamas|Gambia|Vatican|Seychelles)")


def the(name):
    """'United Kingdom' -> 'the United Kingdom' inside a sentence."""
    if name.startswith("The "):
        return "the " + name[4:]
    return "the " + name if _THE.match(name) else name

_countries = None


def countries():
    global _countries
    if _countries is not None:
        return _countries
    rows = sparql("countries", """
SELECT ?c ?sl WHERE {
  ?c p:P31 ?ist . ?ist ps:P31 wd:Q3624078 .
  FILTER NOT EXISTS { ?ist pq:P582 [] }
  FILTER NOT EXISTS { ?ist wikibase:rank wikibase:DeprecatedRank }
  FILTER NOT EXISTS { ?c wdt:P576 [] }
  # UN members plus the two observer states; drops stray historical kingdoms
  # that are still typed "sovereign state" (Serendip, Unyasa, ...).
  FILTER(EXISTS { ?c wdt:P463 wd:Q1065 } || ?c IN (wd:Q237, wd:Q219060))
  ?c wikibase:sitelinks ?sl .
}""")
    cs = {r["c"]: {"sl": int(r["sl"])} for r in rows if r["c"] not in EXCLUDED_COUNTRIES}
    meta = info(cs)
    for q in list(cs):
        lab = clean_label(meta[q]["label"])
        if not lab:
            del cs[q]
            continue
        aliases = list(meta[q]["aliases"])
        if q in COUNTRY_LABEL:
            aliases.append(lab)
            lab = COUNTRY_LABEL[q]
        cs[q].update(label=lab, aliases=aliases, capitals=set(), currencies=set(),
                     codes=set(), continents=set())
    for prop, field in (("P36", "capitals"), ("P38", "currencies")):
        for r in sparql(f"country_{field}", """
SELECT ?c ?v WHERE {
  VALUES ?c { %s }
  ?c p:%s ?st . ?st ps:%s ?v .
  FILTER NOT EXISTS { ?st pq:P582 [] }
  FILTER NOT EXISTS { ?st wikibase:rank wikibase:DeprecatedRank }
}""" % (values(cs), prop, prop)):
            cs[r["c"]][field].add(r["v"])
    for r in sparql("country_codes_continents", """
SELECT ?c ?code ?cont WHERE {
  VALUES ?c { %s }
  { ?c wdt:P474 ?code } UNION { ?c wdt:P30 ?cont }
}""" % values(cs)):
        if "code" in r:
            cs[r["c"]]["codes"].add(r["code"].strip())
        if "cont" in r:
            cs[r["c"]]["continents"].add(r["cont"])
    # "Kingdom of Denmark" / "Kingdom of the Netherlands" duplicate Denmark and
    # the Netherlands (same capital): keep only the better-known item.
    by_cap = defaultdict(list)
    for q, d in cs.items():
        for c in d["capitals"]:
            by_cap[c].append(q)
    for c, qs in by_cap.items():
        if len(qs) > 1:
            keep = max(qs, key=lambda q: cs[q]["sl"])
            for q in qs:
                if q != keep and q in cs:
                    print(f"    drop duplicate country item {cs[q]['label']} (same capital as {cs[keep]['label']})")
                    del cs[q]
    _countries = cs
    return cs


def country_pool(rng, q, cs):
    same = [o for o in cs if o != q and cs[o]["continents"] & cs[q]["continents"]]
    rest = [o for o in cs if o != q and o not in same]
    return [cs[o]["label"] for o in ordered(rng, same) + ordered(rng, rest)]


def t_country_capital(log):
    tpl = "country_capital"
    cs = countries()
    caps = info({c for d in cs.values() for c in d["capitals"]})
    cap_of = defaultdict(set)
    for q, d in cs.items():
        for c in d["capitals"]:
            cap_of[c].add(q)
    cap_pool = sorted({clean_label(caps[c]["label"]) for c in caps if clean_label(caps[c]["label"])})
    clashes = alias_clashes("country_capital_alias_clash", [(c, al) for c in caps for al in caps[c]["aliases"]])
    rows = []
    for q, d in sorted(cs.items(), key=lambda kv: qnum(kv[0])):
        if q in DISPUTED_COUNTRY_CAPITAL:
            log.skip("contested capital (hand list)", d["label"])
            continue
        if len(d["capitals"]) != 1:
            log.skip("≠1 current capital", f"{d['label']}={len(d['capitals'])}")
            continue
        c = next(iter(d["capitals"]))
        clab = clean_label(caps[c]["label"])
        if not clab:
            log.skip("no usable capital label", d["label"])
            continue
        cnames = [d["label"]] + d["aliases"]
        capnames = [clab] + caps[c]["aliases"]
        if any(appears(n, clab) for n in cnames) or any(appears(n, d["label"]) for n in capnames):
            log.skip("country and capital share a name", f"{d['label']}/{clab}")
            continue
        qname = COUNTRY_Q_NAME.get(q, the(d["label"]))
        rng = random.Random(f"{tpl}-{q}")
        try:
            rows.append(make(tpl, q, f"What is the capital of {qname}?", clab, capnames, "geography", "world",
                             d["sl"], pool=ordered(rng, cap_pool), rng=rng, mode="place",
                             drop=[al for al in caps[c]["aliases"] if (c, norm(al)) in clashes]))
        except Skip as e:
            log.skip(str(e).split(":")[0], d["label"])
        if len(cap_of[c]) != 1:
            log.skip("reverse: capital of >1 country", clab)
            continue
        try:
            rows.append(make(tpl, f"{c}-{q}", f"{clab} is the capital of which country?", d["label"], cnames,
                             "geography", "world", caps[c]["sl"], pool=country_pool(rng, q, cs), rng=rng,
                             mode="country"))
        except Skip as e:
            log.skip("reverse: " + str(e).split(":")[0], clab)
    return rows


MAX_PER_CURRENCY = 5


def t_country_currency(log):
    tpl = "country_currency"
    cs = countries()
    cur = info({c for d in cs.values() for c in d["currencies"]})
    # English Wikipedia title ("Japanese yen") over the bare Wikidata label ("yen").
    cur_title = enwiki_titles("currency_enwiki", cur)
    for c in cur:
        if cur_title.get(c):
            cur[c] = dict(cur[c], aliases=cur[c]["aliases"] + [cur[c]["label"] or ""], label=cur_title[c])
    by_cur = defaultdict(list)
    for q, d in cs.items():
        if len(d["currencies"]) != 1:
            log.skip("≠1 current currency", f"{d['label']}={len(d['currencies'])}")
            continue
        by_cur[next(iter(d["currencies"]))].append(q)
    cur_cont = defaultdict(set)
    for c, qs in by_cur.items():
        for q in qs:
            cur_cont[c] |= cs[q]["continents"]
    rows = []
    for c, qs in sorted(by_cur.items(), key=lambda kv: qnum(kv[0])):
        clab = clean_label(cur[c]["label"])
        if not clab:
            log.skip("currency has no English label", c)
            continue
        qs.sort(key=lambda q: -cs[q]["sl"])
        for q in qs[MAX_PER_CURRENCY:]:
            log.skip(f"currency used by >{MAX_PER_CURRENCY} countries (kept top {MAX_PER_CURRENCY})",
                     f"{cs[q]['label']}/{clab}")
        for q in qs[:MAX_PER_CURRENCY]:
            d = cs[q]
            rng = random.Random(f"{tpl}-{q}")
            same = [o for o in by_cur if o != c and cur_cont[o] & d["continents"]]
            rest = [o for o in by_cur if o != c and o not in same]
            pool = [clean_label(cur[o]["label"]) for o in ordered(rng, same) + ordered(rng, rest)
                    if clean_label(cur[o]["label"])]
            try:
                rows.append(make(tpl, q, f"What is the currency of {COUNTRY_Q_NAME.get(q, the(d['label']))}?", clab,
                                 [clab] + cur[c]["aliases"], "geography", "world", d["sl"], pool=pool,
                                 kind="mcq", rng=rng))
            except Skip as e:
                log.skip(str(e).split(":")[0], d["label"])
    return rows


def t_element_symbol(log):
    tpl = "element_symbol"
    rows = sparql("elements", """
SELECT ?e ?n ?sym ?sl WHERE {
  ?e wdt:P31 wd:Q11344 ; wdt:P1086 ?n ; wdt:P246 ?sym ; wikibase:sitelinks ?sl .
  FILTER(?n >= 1 && ?n <= 92)
}""")
    els = {}
    for r in rows:
        d = els.setdefault(r["e"], {"n": set(), "sym": set(), "sl": int(r["sl"])})
        d["n"].add(int(float(r["n"])))
        d["sym"].add(r["sym"])
    meta = info(els)
    pool = sorted({clean_label(meta[e]["label"]) for e in els if clean_label(meta[e]["label"])})
    seen_n, seen_sym = defaultdict(list), defaultdict(list)
    for e, d in els.items():
        for n in d["n"]:
            seen_n[n].append(e)
        for s in d["sym"]:
            seen_sym[s].append(e)
    out = []
    for e in sorted(els, key=lambda q: min(els[q]["n"])):
        d = els[e]
        lab = clean_label(meta[e]["label"])
        if not lab or len(d["n"]) != 1 or len(d["sym"]) != 1:
            log.skip("unclear label/number/symbol", e)
            continue
        sym = next(iter(d["sym"]))
        if len(seen_sym[sym]) != 1 or len(seen_n[next(iter(d["n"]))]) != 1:
            log.skip("symbol or number shared by several items", sym)
            continue
        rng = random.Random(f"{tpl}-{e}")
        names = [lab] + [a for a in meta[e]["aliases"] if norm(a) != norm(sym)]
        try:
            out.append(make(tpl, e, f"Which chemical element has the symbol {sym}?", lab, names, "science",
                            "world", d["sl"], pool=ordered(rng, pool), rng=rng))
        except Skip as e2:
            log.skip(str(e2).split(":")[0], lab)
    return out


def t_calling_code(log):
    tpl = "calling_code"
    cs = countries()
    codes = defaultdict(set)
    for q, d in cs.items():
        for c in d["codes"]:
            codes[c].add(q)
    # Also count dependent territories and partially recognised states that share
    # a sovereign state's code (e.g. +44 Jersey/Guernsey, +7 Kazakhstan).
    all_codes = sorted(codes)
    sharers = defaultdict(set)
    for r in sparql("calling_code_holders", """
SELECT ?x ?code WHERE {
  VALUES ?code { %s }
  ?x wdt:P474 ?code ; wikibase:sitelinks ?sl .
  FILTER(?sl >= 20)
  FILTER NOT EXISTS { ?x wdt:P576 [] }
  VALUES ?k { wd:Q6256 wd:Q161243 wd:Q46395 wd:Q185086 wd:Q3624078 wd:Q15634554 wd:Q10711424 wd:Q1048835 }
  ?x wdt:P31 ?k .
}""" % " ".join(json.dumps(c) for c in all_codes)):
        sharers[r["code"]].add(r["x"])
    pool = [cs[q]["label"] for q in cs]
    rows = []
    for q, d in sorted(cs.items(), key=lambda kv: qnum(kv[0])):
        if len(d["codes"]) != 1:
            log.skip("≠1 calling code", f"{d['label']}={len(d['codes'])}")
            continue
        code = next(iter(d["codes"]))
        if not re.fullmatch(r"\+\d{1,4}( \d{1,4})?", code):
            log.skip("malformed code", f"{d['label']} {code!r}")
            continue
        base = code.split()[0]
        holders = codes[code] | sharers[code]
        if len(holders) != 1:
            log.skip("code shared by several countries/territories", f"{code} {d['label']}")
            continue
        if " " not in code and any(c.startswith(code + " ") for c in codes):
            log.skip("prefix of other countries' codes", code)
            continue
        rng = random.Random(f"{tpl}-{q}")
        try:
            rows.append(make(tpl, q, f"Which country has the international dialling code {code}?", d["label"],
                             [d["label"]] + d["aliases"], "geography", "world", d["sl"],
                             pool=country_pool(rng, q, cs), rng=rng, mode="country"))
        except Skip as e:
            log.skip(str(e).split(":")[0], d["label"])
    return rows


TEMPLATES = {
    "state_capital": t_state_capital,
    "heritage_state": t_heritage_state,
    "park_state": t_park_state,
    "airport_code": t_airport_code,
    "hindi_film_director": t_hindi_film_director,
    "hindi_film_year": t_hindi_film_year,
    "indian_book_author": t_indian_book_author,
    "river_mouth": t_river_mouth,
    "city_state": t_city_state,
    "country_capital": t_country_capital,
    "country_currency": t_country_currency,
    "element_symbol": t_element_symbol,
    "calling_code": t_calling_code,
}

# ---------------------------------------------------------------------------
# Verification
# ---------------------------------------------------------------------------

# (templates to search, question must contain, accepted answers, alt must contain)
SPOT_CHECKS = [
    (["state_capital"], "Indian state of Karnataka?", {"Bengaluru"}, "Bangalore"),
    (["state_capital"], "Indian state of Rajasthan?", {"Jaipur"}, None),
    (["heritage_state", "park_state"], "Taj Mahal", {"Uttar Pradesh"}, None),
    (["heritage_state", "park_state"], "Kaziranga", {"Assam"}, None),
    (["airport_code"], "IATA code BOM?", {"Mumbai"}, "Bombay"),
    (["airport_code"], "IATA code DEL?", {"Delhi", "New Delhi"}, None),
    (["hindi_film_director"], "Sholay (1975)", {"Ramesh Sippy"}, None),
    (["hindi_film_year"], "Dilwale Dulhania Le Jayenge", {"1995"}, None),
    (["indian_book_author"], "The God of Small Things", {"Arundhati Roy"}, None),
    (["river_mouth"], "the Ganges river", {"Bay of Bengal"}, None),
    (["country_capital"], "capital of Japan?", {"Tokyo"}, None),
    (["element_symbol"], "symbol Fe?", {"Iron"}, None),
    (["calling_code"], "code +81?", {"Japan"}, None),
    (["country_currency"], "currency of Japan?", {"Japanese yen"}, None),
    (["country_capital"], "capital of India?", {"New Delhi"}, None),
    (["state_capital"], "Indian state of Tamil Nadu?", {"Chennai"}, "Madras"),
]


def spot_check(out):
    failures = []
    for tpls, needle, answers, alt in SPOT_CHECKS:
        if not all(t in out for t in tpls):
            continue
        hits = [r for t in tpls for r in out[t] if needle in r["q"]]
        if not hits:
            failures.append(f"MISSING  {tpls} {needle!r}")
            continue
        for r in hits:
            ok = r["a"] in answers and (alt is None or alt in r["alt"])
            print(f"  {'PASS' if ok else 'FAIL'}  {r['q']}  ->  {r['a']}"
                  + (f"  (alt has {alt!r})" if alt and ok else ""))
            if not ok:
                failures.append(f"WRONG    {r['q']} -> {r['a']} alt={r['alt']} (want {answers}, alt {alt})")
    return failures


def validate(out):
    errors, ids = [], {}
    for tpl, rows in out.items():
        for r in rows:
            where = f"{tpl}:{r.get('id')}"
            if list(r.keys()) != KEYS:
                errors.append(f"{where}: keys {list(r.keys())}")
                continue
            if r["id"] in ids:
                errors.append(f"{where}: duplicate id (also in {ids[r['id']]})")
            ids[r["id"]] = tpl
            if not r["id"].startswith(f"wd-{tpl}-"):
                errors.append(f"{where}: id prefix")
            if r["region"] not in ("india", "world") or r["diff"] not in ("easy", "medium", "hard"):
                errors.append(f"{where}: region/diff")
            if r["src"] != "wikidata" or r["note"] != "":
                errors.append(f"{where}: src/note")
            if not all(isinstance(r[k], str) and r[k].strip() for k in ("q", "a", "cat")):
                errors.append(f"{where}: empty q/a/cat")
            if any(appears(x, r["q"]) for x in [r["a"]] + r["alt"]):
                errors.append(f"{where}: answer appears in question")
            if r["kind"] == "text":
                if r["options"] != []:
                    errors.append(f"{where}: text with options")
                if words(r["a"]) > 4:
                    errors.append(f"{where}: text answer > 4 words")
                if any(norm(x) == norm(r["a"]) for x in r["alt"]) or len({norm(x) for x in r["alt"]}) != len(r["alt"]):
                    errors.append(f"{where}: alt duplicates")
                for x in r["alt"]:
                    if "(" in x or len(x.split()) > 5 or is_code(x) or re.fullmatch(r"Q\d+", x):
                        errors.append(f"{where}: bad alt {x!r}")
            elif r["kind"] == "mcq":
                if r["alt"] != []:
                    errors.append(f"{where}: mcq with alt")
                o = r["options"]
                if len(o) != 4 or len({norm(x) for x in o}) != 4 or r["a"] not in o:
                    errors.append(f"{where}: options {o}")
            else:
                errors.append(f"{where}: kind {r['kind']}")
    return errors


# ---------------------------------------------------------------------------

def main():
    global REFRESH
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--only", help="comma-separated template names")
    ap.add_argument("--sample", type=int, default=0, help="print N random questions per template")
    ap.add_argument("--seed", type=int, default=None, help="seed for --sample")
    ap.add_argument("--refresh", action="store_true", help="ignore cached SPARQL results")
    args = ap.parse_args()
    REFRESH = args.refresh
    names = args.only.split(",") if args.only else list(TEMPLATES)
    for n in names:
        if n not in TEMPLATES:
            sys.exit(f"unknown template {n!r}; choose from {', '.join(TEMPLATES)}")
    OUT.mkdir(parents=True, exist_ok=True)
    CACHE.mkdir(parents=True, exist_ok=True)

    out = {}
    for n in names:
        print(f"== {n}", flush=True)
        log = Log(n)
        rows = assign_diff(TEMPLATES[n](log))
        for r in rows:
            del r["_sl"]
        out[n] = rows
        kinds = defaultdict(int)
        for r in rows:
            kinds[r["kind"]] += 1
        print(f"   {len(rows)} questions  (text {kinds['text']}, mcq {kinds['mcq']})")
        log.report()
        with open(OUT / f"{n}.jsonl", "w", encoding="utf-8") as fh:
            for r in rows:
                fh.write(json.dumps(r, ensure_ascii=False) + "\n")

    if args.sample:
        rng = random.Random(args.seed)
        for n, rows in out.items():
            print(f"\n--- sample: {n}")
            for r in rng.sample(rows, min(args.sample, len(rows))):
                extra = f"  alt={r['alt']}" if r["kind"] == "text" else f"  opts={r['options']}"
                print(f"  [{r['diff'][0]}] {r['q']}  ->  {r['a']}{extra}")

    print("\n== spot checks")
    failures = spot_check(out)
    print("\n== validation")
    errors = validate(out)
    total = sum(len(r) for r in out.values())
    print(f"  {total} questions in {len(out)} files, {len(errors)} format errors")
    for e in errors[:40]:
        print("  ERROR", e)
    for f in failures:
        print("  SPOT-CHECK", f)
    if errors or failures:
        sys.exit(1)


if __name__ == "__main__":
    main()
