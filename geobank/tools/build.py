#!/usr/bin/env python3
"""Compile the GeoNames India dump into geobank/places.json.

The bot reads places.json and nothing else. Two kinds of row live in it:

  state  - the 28 states and 8 union territories, the coarse answer
  city   - populated places above POP_FLOOR, the fine answer

A guess is judged by looking the typed name up here. A city carries the state
it sits in, so naming the town credits the state for free. Where two places
share a name the bigger one wins, which is why every row keeps its population.
"""

import json
import re
import sys
import unicodedata
from pathlib import Path

POP_FLOOR = 20_000

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "places.json"


def fold(name: str) -> str:
    """Normalise a name the way a guess will be normalised: no diacritics, no
    punctuation, single spaces, lowercase. 'Bengalūru' and 'bengaluru' fold the
    same, so the bank never has to list both."""
    flat = unicodedata.normalize("NFKD", name)
    flat = "".join(c for c in flat if not unicodedata.combining(c))
    flat = flat.lower().replace("&", " and ")
    flat = re.sub(r"[^a-z0-9 ]+", " ", flat)
    return re.sub(r"\s+", " ", flat).strip()


def load_admin1(path: Path) -> dict:
    """admin1 code -> (state name, geonameid)."""
    states = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        parts = line.split("\t")
        if len(parts) < 4 or not parts[0].startswith("IN."):
            continue
        states[parts[0].split(".", 1)[1]] = (parts[1], int(parts[3]))
    return states


def main(dump: Path, admin1: Path) -> int:
    states = load_admin1(admin1)
    centroids = {}
    cities = {}

    with dump.open(encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if len(f) < 15:
                continue
            gid, name, ascii_name, alt = int(f[0]), f[1], f[2], f[3]
            lat, lon, fclass, admin = float(f[4]), float(f[5]), f[6], f[10]
            pop = int(f[14] or 0)

            if gid in {g for _, g in states.values()}:
                centroids[gid] = (lat, lon)

            if fclass != "P" or pop < POP_FLOOR or admin not in states:
                continue

            state_name = states[admin][0]
            # Every spelling anyone might type: the display name, the ascii
            # form, and GeoNames' own alternates where they look like a name
            # rather than a code or a script we don't accept.
            names = {name, ascii_name}
            for a in alt.split(","):
                a = a.strip()
                if 2 < len(a) < 40 and re.fullmatch(r"[A-Za-z0-9 .'\-]+", a):
                    names.add(a)
            keys = {fold(n) for n in names if fold(n)}

            row = {
                "id": gid,
                "name": ascii_name or name,
                "state": state_name,
                "lat": round(lat, 5),
                "lon": round(lon, 5),
                "pop": pop,
                "keys": sorted(keys),
            }
            # Same name, two towns: the bigger one owns the name.
            for k in keys:
                held = cities.get(k)
                if held is None or pop > held["pop"]:
                    cities[k] = row

    by_id = {}
    for row in cities.values():
        by_id[row["id"]] = row

    state_rows = []
    for code, (name, gid) in sorted(states.items()):
        lat, lon = centroids.get(gid, (None, None))
        if lat is None:
            print(f"  ! no centroid for {name}", file=sys.stderr)
            continue
        state_rows.append({
            "code": code,
            "name": name,
            "lat": round(lat, 5),
            "lon": round(lon, 5),
            "keys": sorted({fold(name)} | EXTRA_STATE_KEYS.get(name, set())),
        })

    out = {
        "format": "GEOPLACES1",
        "source": "GeoNames (CC BY 4.0), https://download.geonames.org/export/dump/",
        "attribution": "Place names and coordinates from GeoNames, CC BY 4.0.",
        "pop_floor": POP_FLOOR,
        "states": state_rows,
        "cities": sorted(by_id.values(), key=lambda r: -r["pop"]),
    }
    OUT.write_text(json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"{len(state_rows)} states/UTs, {len(by_id)} cities -> {OUT}")
    return 0


# Names people really type that GeoNames does not carry as the admin1 name.
EXTRA_STATE_KEYS = {
    "Delhi": {"new delhi", "ncr", "nct", "national capital territory"},
    "Odisha": {"orissa"},
    "Puducherry": {"pondicherry", "pondy"},
    "Uttarakhand": {"uttaranchal"},
    "Andaman and Nicobar": {"andaman", "nicobar", "andamans"},
    "Jammu and Kashmir": {"kashmir", "jammu", "j and k", "jk"},
    "Dadra and Nagar Haveli and Daman and Diu": {"daman", "diu", "dadra", "nagar haveli"},
    "Tamil Nadu": {"tamilnadu"},
    "Andhra Pradesh": {"andhra"},
    "Himachal Pradesh": {"himachal"},
    "Madhya Pradesh": {"madhya pradesh", "mp"},
    "Arunachal Pradesh": {"arunachal"},
    "West Bengal": {"bengal"},
}

if __name__ == "__main__":
    raise SystemExit(main(Path(sys.argv[1]), Path(sys.argv[2])))
