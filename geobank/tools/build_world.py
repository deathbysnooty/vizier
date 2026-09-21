#!/usr/bin/env python3
"""Compile the GeoNames world dump into geobank/world.json.

This is `build.py`'s sibling: that one builds the answers for an India round,
where the gate is the STATE, and this one builds the answers for a World round,
where the gate is the COUNTRY.

  countries  the 250-odd countries and territories, each with every spelling
             somebody might type — `USA`, `UK`, `UAE`, `Holland`, `Burma`
  cities     every town above 15,000 people, worldwide, each carrying its
             country

The cities are here for the same two jobs they do in the India bank: deciding
which country a photograph stands in (the nearest town's), and letting somebody
who knows the actual town say so.

Usage:
    curl -sL -o cities15000.zip https://download.geonames.org/export/dump/cities15000.zip
    curl -sL -o countryInfo.txt https://download.geonames.org/export/dump/countryInfo.txt
    unzip -q cities15000.zip
    python3 geobank/tools/build_world.py cities15000.txt countryInfo.txt
"""

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OUT = HERE.parent / "world.json"

sys.path.insert(0, str(HERE))
from build import fold  # the same folding the India bank uses, or keys would not match

POP_FLOOR = 15_000

# What people actually type, against the name GeoNames holds. Everything here
# is a name a room would really use - not a translation exercise. The official
# name is added automatically, so these are only the extras.
ALIASES = {
    "US": {"usa", "u s a", "united states", "united states of america", "america", "the us", "states"},
    "GB": {"uk", "u k", "britain", "great britain", "united kingdom", "england", "scotland", "wales"},
    "AE": {"uae", "u a e", "emirates", "dubai country", "united arab emirates"},
    "KR": {"south korea", "korea", "republic of korea", "s korea"},
    "KP": {"north korea", "dprk", "n korea"},
    "RU": {"russia", "russian federation"},
    "CZ": {"czech republic", "czechia"},
    "NL": {"holland", "the netherlands", "netherlands"},
    "MM": {"burma", "myanmar"},
    "TR": {"turkey", "turkiye", "türkiye"},
    "CI": {"ivory coast", "cote d ivoire", "côte d'ivoire"},
    "CD": {"drc", "congo kinshasa", "democratic republic of the congo", "dr congo"},
    "CG": {"congo brazzaville", "republic of the congo"},
    "VA": {"vatican", "vatican city", "holy see"},
    "SZ": {"swaziland", "eswatini"},
    "MK": {"macedonia", "north macedonia"},
    "TL": {"east timor", "timor leste"},
    "CV": {"cape verde", "cabo verde"},
    "LA": {"laos", "lao pdr"},
    "SY": {"syria"},
    "IR": {"iran", "persia"},
    "VN": {"vietnam", "viet nam"},
    "TZ": {"tanzania"},
    "BO": {"bolivia"},
    "VE": {"venezuela"},
    "MD": {"moldova"},
    "BN": {"brunei"},
    "PS": {"palestine", "palestinian territories"},
    "TW": {"taiwan"},
    "HK": {"hong kong"},
    "MO": {"macau", "macao"},
    "GM": {"gambia", "the gambia"},
    "BS": {"bahamas", "the bahamas"},
    "DO": {"dominican republic"},
    "CF": {"central african republic", "car"},
    "GB-WLS": set(),
}


def main(cities_path: Path, country_path: Path) -> int:
    countries = {}
    for line in country_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        f = line.split("\t")
        if len(f) < 9 or not f[0]:
            continue
        code, iso3, name, continent = f[0], f[1], f[4], f[8]
        keys = {fold(name)} | {fold(a) for a in ALIASES.get(code, set())}
        countries[code] = {
            "code": code,
            "iso3": iso3,
            "name": name,
            "continent": continent,
            "keys": sorted(k for k in keys if k),
            # filled in from the cities below: the mean of its towns, which is
            # a better centre of gravity for a guessing game than a landmass
            # centroid that can sit in the sea.
            "lat": None,
            "lon": None,
        }

    cities = []
    sums = {}
    with cities_path.open(encoding="utf-8") as fh:
        for line in fh:
            f = line.rstrip("\n").split("\t")
            if len(f) < 15:
                continue
            name, ascii_name = f[1], f[2]
            lat, lon, fclass, code = float(f[4]), float(f[5]), f[6], f[8]
            pop = int(f[14] or 0)
            if fclass != "P" or pop < POP_FLOOR or code not in countries:
                continue
            keys = {fold(n) for n in (name, ascii_name)}
            cities.append({
                "name": ascii_name or name,
                "country": code,
                "lat": round(lat, 5),
                "lon": round(lon, 5),
                "pop": pop,
                "keys": sorted(k for k in keys if k),
            })
            got = sums.setdefault(code, [0.0, 0.0, 0])
            got[0] += lat
            got[1] += lon
            got[2] += 1

    for code, (lat, lon, n) in sums.items():
        countries[code]["lat"] = round(lat / n, 5)
        countries[code]["lon"] = round(lon / n, 5)

    # A country with no town above the floor cannot be a round's answer, and
    # has no centre to measure from either.
    playable = [c for c in countries.values() if c["lat"] is not None]

    # A name that would answer for two countries answers for neither: the round
    # would be unwinnable for whoever typed it and meant the other one.
    seen, clashes = {}, set()
    for c in playable:
        for k in c["keys"]:
            if k in seen and seen[k] != c["code"]:
                clashes.add(k)
            seen[k] = c["code"]
    if clashes:
        for c in playable:
            c["keys"] = [k for k in c["keys"] if k not in clashes]
        print(f"  dropped {len(clashes)} names that fit two countries: {sorted(clashes)[:6]}")

    OUT.write_text(json.dumps({
        "format": "GEOWORLD1",
        "source": "GeoNames (CC BY 4.0), https://download.geonames.org/export/dump/",
        "attribution": "Place names and coordinates from GeoNames, CC BY 4.0.",
        "pop_floor": POP_FLOOR,
        "countries": sorted(playable, key=lambda c: c["code"]),
        "cities": sorted(cities, key=lambda c: -c["pop"]),
    }, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"{len(playable)} countries, {len(cities)} towns -> {OUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(Path(sys.argv[1]), Path(sys.argv[2])))
