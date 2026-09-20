#!/usr/bin/env python3
"""Pull street-level photos from Mapillary into geobank/, beside KartaView's.

KartaView gave the game 13 states. Mapillary reaches most of the rest —
Mumbai, Chennai, Ahmedabad, Kochi, Guwahati and Bhubaneswar all have imagery
there and none on KartaView — but it is a mixed bag where KartaView is
uniform. KartaView is dashcam footage and almost every frame faces the road;
Mapillary also holds phone photos taken on foot, and a photograph of the side
of a parked van is not a round anybody can win.

So this harvester is fussier than its sibling:

  Sequences, not snaps. A frame only counts if several photos from the SAME
  sequence sit in the same neighbourhood — that is what a drive looks like,
  and a one-off snapshot does not.

  Quality and shape. `quality_score` above QUALITY, flat images only: no
  panoramas, no fisheye, nothing that reads as a distorted bubble on a card.

Everything else matches `harvest.py`, whose rules this imports rather than
restates: one photo per ~330m cell, states capped so none can dominate, states
too thin to play dropped, and every photo's state taken from its nearest town.

Needs a free Mapillary token in `MAPILLARY_TOKEN`. It is used HERE, when the
bank is built, and never by the bot: nothing about it reaches the server.

Mapillary imagery is CC BY-SA 4.0 and the licence asks for the individual
photographer, so each spot keeps the contributor's username in `by`.
"""

import json
import os
import sys
import time
import urllib.parse
import urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from harvest import BANK, CELL, IMAGES, MIN_SPOTS, OUT, PLACES, TARGET_PER_STATE, dedupe, haversine

API = "https://graph.mapillary.com/images"

# The API refuses a box over 0.010 square degrees, so a look is ~5km a side.
BOX = 0.025
CITIES_PER_STATE = 14
QUALITY = 0.5
# How many frames of one sequence must sit in the same look before it reads as
# somebody driving rather than somebody standing.
RUN = 4

ATTRIBUTION = "Street imagery from Mapillary contributors, CC BY-SA 4.0."


def token() -> str:
    value = os.environ.get("MAPILLARY_TOKEN", "").strip()
    if not value:
        sys.exit("MAPILLARY_TOKEN is not set. Register an application at\n"
                 "https://www.mapillary.com/dashboard/developers and export its client token.")
    return value


def look(lat: float, lon: float, tok: str, tries: int = 3):
    """Every usable frame in one small box."""
    bbox = f"{lon - BOX:.4f},{lat - BOX:.4f},{lon + BOX:.4f},{lat + BOX:.4f}"
    fields = "id,computed_geometry,thumb_1024_url,captured_at,is_pano,camera_type,quality_score,sequence,creator"
    url = f"{API}?access_token={urllib.parse.quote(tok)}&bbox={bbox}&limit=200&fields={fields}"
    for attempt in range(tries):
        try:
            with urllib.request.urlopen(url, timeout=90) as r:
                return json.load(r).get("data", [])
        except Exception:
            time.sleep(1 + attempt * 2)
    return []


def usable(item) -> bool:
    if item.get("is_pano") or item.get("camera_type") not in (None, "perspective"):
        return False
    if (item.get("quality_score") or 0) < QUALITY:
        return False
    return bool(item.get("thumb_1024_url") and item.get("computed_geometry"))


def driven(items):
    """Only frames from a sequence with several photos here — a drive, not a
    snapshot. This is the filter that keeps parked vans off the cards."""
    runs = defaultdict(list)
    for item in items:
        runs[item.get("sequence")].append(item)
    return [i for run in runs.values() if len(run) >= RUN for i in run]


def main() -> int:
    tok = token()
    places = json.loads(PLACES.read_text(encoding="utf-8"))
    seen, cities = set(), []
    for c in places["cities"]:
        if c["id"] in seen:
            continue
        seen.add(c["id"])
        cities.append(c)

    by_state = defaultdict(list)
    for c in cities:
        by_state[c["state"]].append(c)
    for rows in by_state.values():
        rows.sort(key=lambda r: -r["pop"])

    def nearest_city(lat, lon):
        best, best_km = None, 1e9
        for c in cities:
            if abs(c["lat"] - lat) > 1.2 or abs(c["lon"] - lon) > 1.2:
                continue
            km = haversine(lat, lon, c["lat"], c["lon"])
            if km < best_km:
                best, best_km = c, km
        return best, best_km

    # What the bank already holds, so this only tops states up.
    held = defaultdict(list)
    if OUT.exists():
        for spot in json.loads(OUT.read_text(encoding="utf-8")).get("spots", []):
            if (BANK / spot["file"]).exists():
                held[spot["state"]].append(spot)
    print(f"{sum(len(v) for v in held.values())} photos already banked", flush=True)

    IMAGES.mkdir(exist_ok=True)
    fresh = []

    for state in sorted(by_state):
        have = held.get(state, [])
        want = max(0, TARGET_PER_STATE - len(have))
        if want == 0:
            continue
        probe = by_state[state][:CITIES_PER_STATE]
        with ThreadPoolExecutor(max_workers=5) as ex:
            hauls = list(ex.map(lambda c: look(c["lat"], c["lon"], tok), probe))

        cells = {(round(s["lat"] / CELL), round(s["lon"] / CELL)) for s in have}
        pools = []
        for items in hauls:
            good = driven([i for i in items if usable(i)])
            good.sort(key=lambda i: -(i.get("quality_score") or 0))
            if good:
                pools.append(good)

        taken = []
        while pools and len(taken) < want:
            for pool in list(pools):
                if not pool:
                    pools.remove(pool)
                    continue
                item = pool.pop(0)
                lon, lat = item["computed_geometry"]["coordinates"]
                cell = (round(lat / CELL), round(lon / CELL))
                if cell in cells:
                    continue
                cells.add(cell)
                taken.append((item, lat, lon))
                if len(taken) >= want:
                    break

        if not taken:
            print(f"  - {state}: nothing usable", flush=True)
            continue

        for item, lat, lon in taken:
            city, km = nearest_city(lat, lon)
            fresh.append({
                "id": f"mly{item['id']}",
                "lat": round(lat, 5),
                "lon": round(lon, 5),
                # The nearest town decides the state, so the answer the game
                # accepts and the answer it reveals can never disagree.
                "state": city["state"] if city else state,
                "city": city["name"] if city else None,
                "city_km": round(km, 1) if city else None,
                "file": f"images/mly{item['id']}.jpg",
                "url": item["thumb_1024_url"],
                "by": (item.get("creator") or {}).get("username") or "unknown",
                "source": "mapillary",
                "shot": time.strftime("%Y-%m-%d", time.gmtime((item.get("captured_at") or 0) / 1000)),
            })
        print(f"  {state}: {len(have)} kept + {len(taken)} new", flush=True)

    def fetch(spot):
        dest = IMAGES / Path(spot["file"]).name
        if dest.exists() and dest.stat().st_size > 10_000:
            return True
        try:
            with urllib.request.urlopen(spot["url"], timeout=90) as r:
                data = r.read()
            if len(data) < 10_000:
                return False
            dest.write_bytes(data)
            return True
        except Exception:
            return False

    print(f"\ndownloading {len(fresh)} photos ...", flush=True)
    with ThreadPoolExecutor(max_workers=8) as ex:
        ok = list(ex.map(fetch, fresh))
    fresh = [s for s, good in zip(fresh, ok) if good]
    for s in fresh:
        s.pop("url", None)

    bank = json.loads(OUT.read_text(encoding="utf-8")) if OUT.exists() else {"format": "GEOSPOTS1", "spots": []}
    spots = [s for s in bank["spots"] if (BANK / s["file"]).exists()]
    spots.extend(fresh)
    # One row per photo, however many times this has been run. Without this a
    # second pass appends the same frames again, and a duplicated photo is then
    # likelier to be drawn than any other.
    spots = dedupe(spots)

    counts = defaultdict(int)
    for s in spots:
        counts[s["state"]] += 1
    thin = {s for s, n in counts.items() if n < MIN_SPOTS}
    if thin:
        print("too thin to play, left out:", ", ".join(f"{s} ({counts[s]})" for s in sorted(thin)))
    spots = [s for s in spots if s["state"] not in thin]

    bank["format"] = "GEOSPOTS1"
    bank["source"] = "KartaView (https://kartaview.org/) and Mapillary (https://www.mapillary.com/), both CC BY-SA 4.0"
    bank["attribution"] = ("Street imagery from KartaView contributors and Grab, and from Mapillary contributors, "
                           "used under CC BY-SA 4.0.")
    bank["states"] = sorted({s["state"] for s in spots})
    bank["spots"] = spots
    OUT.write_text(json.dumps(bank, ensure_ascii=False, indent=1), encoding="utf-8")

    print(f"\n{len(spots)} places across {len(bank['states'])} states -> {OUT}")
    for state in bank["states"]:
        n = sum(1 for s in spots if s["state"] == state)
        mly = sum(1 for s in spots if s["state"] == state and s.get("source") == "mapillary")
        print(f"  {state:<42} {n:>3}  ({mly} from Mapillary)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
