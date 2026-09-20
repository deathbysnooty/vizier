#!/usr/bin/env python3
"""Pull street-level photos from KartaView into geobank/.

KartaView is community dashcam footage, so it pools where a few people drove:
thick in Tamil Nadu and Telangana, thin elsewhere, absent in most of the north
and west. Two rules keep that from becoming the game's problem.

  Spread. One photo per ~330m cell, and cities inside a state are taken in
  turn, so a state is not fifty frames of the same junction.

  A floor. A state that cannot reach MIN_SPOTS distinct places is left out of
  the bank entirely rather than shipped as a token round nobody can win.

Photos are taken at `lth` size (1280x720, ~300KB) - big enough to read a
signboard, small enough to post. Re-running skips what is already downloaded.
"""

import json
import math
import random
import sys
import time
import urllib.parse
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
BANK = HERE.parent
IMAGES = BANK / "images"
OUT = BANK / "spots.json"
PLACES = BANK / "places.json"

API = "https://api.openstreetcam.org/1.0/list/nearby-photos/"
CDN = "https://kartaview.org/"

TARGET_PER_STATE = 60      # enough that a state does not repeat for weeks
MIN_SPOTS = 20             # below this a state is not playable, so it is cut
CITIES_PER_STATE = 18      # how many towns in a state to look around
RADIUS = 5000              # metres, the largest the API answers reliably
CELL = 0.003               # ~330m: one spot per cell, so shots are not neighbours
DAY_FROM, DAY_TO = 6, 18   # keep daylight shots; night frames are unguessable

ATTRIBUTION = "Street imagery from KartaView contributors and Grab, CC BY-SA 4.0."


def dedupe(spots):
    """One row per photo, keeping the first. Both harvesters merge into the
    same file and either can be run twice, so without this the bank grows
    copies and a duplicated frame becomes likelier to be drawn than the rest."""
    seen, out = set(), []
    for spot in spots:
        if spot["id"] in seen:
            continue
        seen.add(spot["id"])
        out.append(spot)
    return out


def haversine(a_lat, a_lon, b_lat, b_lon):
    r = 6371.0
    p1, p2 = math.radians(a_lat), math.radians(b_lat)
    dp, dl = p2 - p1, math.radians(b_lon - a_lon)
    h = math.sin(dp / 2) ** 2 + math.cos(p1) * math.cos(p2) * math.sin(dl / 2) ** 2
    return 2 * r * math.asin(math.sqrt(h))


def nearby(lat, lon, tries=3):
    body = urllib.parse.urlencode({"lat": lat, "lng": lon, "radius": RADIUS}).encode()
    for attempt in range(tries):
        try:
            req = urllib.request.Request(API, data=body)
            with urllib.request.urlopen(req, timeout=60) as r:
                return json.load(r).get("currentPageItems", [])
        except Exception:
            time.sleep(1 + attempt * 2)
    return []


def daylight(item):
    stamp = (item.get("shot_date") or item.get("date_added") or "").strip()
    if len(stamp) < 13:
        return True
    try:
        return DAY_FROM <= int(stamp[11:13]) < DAY_TO
    except ValueError:
        return True


def relabel():
    """Re-derive every banked photo's state from its nearest town, and re-apply
    the MIN_SPOTS floor. Needs no network: the coordinates are already here."""
    places = json.loads(PLACES.read_text(encoding="utf-8"))
    seen, cities = set(), []
    for c in places["cities"]:
        if c["id"] in seen:
            continue
        seen.add(c["id"])
        cities.append(c)

    bank = json.loads(OUT.read_text(encoding="utf-8"))
    before = len(bank["spots"])
    bank["spots"] = dedupe(bank["spots"])
    if len(bank["spots"]) < before:
        print(f"dropped {before - len(bank['spots'])} duplicate rows")
    moved = 0
    for spot in bank["spots"]:
        best, best_km = None, 1e9
        for c in cities:
            km = haversine(spot["lat"], spot["lon"], c["lat"], c["lon"])
            if km < best_km:
                best, best_km = c, km
        if best is None:
            continue
        if best["state"] != spot["state"]:
            print(f"  {spot['id']}: {spot['state']} -> {best['state']} ({best['name']}, {best_km:.1f} km)")
            moved += 1
        spot["state"] = best["state"]
        spot["city"] = best["name"]
        spot["city_km"] = round(best_km, 1)

    counts = {}
    for spot in bank["spots"]:
        counts[spot["state"]] = counts.get(spot["state"], 0) + 1
    thin = {s for s, n in counts.items() if n < MIN_SPOTS}
    if thin:
        print("too thin to play, dropped:", ", ".join(f"{s} ({counts[s]})" for s in sorted(thin)))
    bank["spots"] = [s for s in bank["spots"] if s["state"] not in thin]
    bank["states"] = sorted({s["state"] for s in bank["spots"]})
    OUT.write_text(json.dumps(bank, ensure_ascii=False, indent=1), encoding="utf-8")

    # Photos the bank no longer points at - from a state that fell below the
    # floor, or a duplicate - are build output nothing will read again, and
    # they would otherwise ride along in every rsync.
    wanted = {Path(s["file"]).name for s in bank["spots"]}
    orphans = [f for f in IMAGES.iterdir() if f.is_file() and f.name not in wanted]
    for f in orphans:
        f.unlink()
    if orphans:
        print(f"removed {len(orphans)} photos the bank no longer names")

    print(f"{moved} photos moved state; {len(bank['spots'])} places across {len(bank['states'])} states")
    return 0


def main():
    places = json.loads(PLACES.read_text(encoding="utf-8"))
    by_state = {}
    seen_ids = set()
    for c in places["cities"]:
        if c["id"] in seen_ids:
            continue
        seen_ids.add(c["id"])
        by_state.setdefault(c["state"], []).append(c)
    for rows in by_state.values():
        rows.sort(key=lambda r: -r["pop"])

    all_cities = [c for rows in by_state.values() for c in rows]

    def nearest_city(lat, lon):
        best, best_km = None, 1e9
        for c in all_cities:
            if abs(c["lat"] - lat) > 1.2 or abs(c["lon"] - lon) > 1.2:
                continue
            km = haversine(lat, lon, c["lat"], c["lon"])
            if km < best_km:
                best, best_km = c, km
        return best, best_km

    IMAGES.mkdir(exist_ok=True)

    # A download that failed last time should not cost the bank a state. What
    # is already on disk is kept, and each state is only topped up to target,
    # so re-running fills the gaps instead of starting over.
    held = {}
    if OUT.exists():
        for spot in json.loads(OUT.read_text(encoding="utf-8")).get("spots", []):
            if (BANK / spot["file"]).exists():
                held.setdefault(spot["state"], []).append(spot)
        kept = sum(len(v) for v in held.values())
        if kept:
            print(f"keeping {kept} photos already on disk", flush=True)

    spots, skipped = [], []

    for state, cities in sorted(by_state.items()):
        probe = cities[:CITIES_PER_STATE]
        with ThreadPoolExecutor(max_workers=6) as ex:
            hauls = list(ex.map(lambda c: nearby(c["lat"], c["lon"]), probe))

        # Take from each town in turn, so the biggest one cannot fill the state.
        pools = []
        for city, items in zip(probe, hauls):
            good = [i for i in items if daylight(i) and i.get("lth_name")]
            random.shuffle(good)
            if good:
                pools.append(good)

        have = held.get(state, [])
        # Photos already banked own their cells, so a top-up does not land a
        # second frame of a junction the bank already has.
        cells = {(round(s["lat"] / CELL), round(s["lon"] / CELL)) for s in have}
        want = max(0, TARGET_PER_STATE - len(have))
        taken = []
        while pools and len(taken) < want:
            for pool in list(pools):
                if not pool:
                    pools.remove(pool)
                    continue
                item = pool.pop()
                lat, lon = float(item["lat"]), float(item["lng"])
                cell = (round(lat / CELL), round(lon / CELL))
                if cell in cells:
                    continue
                cells.add(cell)
                taken.append(item)
                if len(taken) >= want:
                    break

        if len(have) + len(taken) < MIN_SPOTS:
            skipped.append((state, len(have) + len(taken)))
            print(f"  - {state}: only {len(have) + len(taken)} places, left out", flush=True)
            continue

        spots.extend(have)

        for item in taken:
            lat, lon = float(item["lat"]), float(item["lng"])
            city, km = nearest_city(lat, lon)
            # The state comes from the NEAREST TOWN, not from whichever town we
            # happened to look around. Puducherry is an enclave inside Tamil
            # Nadu: a frame found 4 km from a Puducherry town can stand in Tamil
            # Nadu, and labelling it Puducherry would have the reveal name a
            # town the game then refuses as an answer. Nearest-town keeps the
            # answer and the reveal in agreement, always.
            spots.append({
                "id": f"kv{item['id']}",
                "lat": round(lat, 5),
                "lon": round(lon, 5),
                "state": city["state"] if city else state,
                "city": city["name"] if city else None,
                "city_km": round(km, 1) if city else None,
                "file": f"images/kv{item['id']}.jpg",
                "url": CDN + item["lth_name"],
                "by": item.get("username") or "unknown",
                "shot": (item.get("shot_date") or "")[:10],
            })
        print(f"  {state}: {len(have)} kept + {len(taken)} new", flush=True)

    def fetch(spot):
        if "url" not in spot:
            return True
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

    print(f"\ndownloading {len(spots)} photos ...", flush=True)
    with ThreadPoolExecutor(max_workers=8) as ex:
        ok = list(ex.map(fetch, spots))
    spots = [s for s, good in zip(spots, ok) if good]
    for s in spots:
        s.pop("url", None)

    states = sorted({s["state"] for s in spots})
    OUT.write_text(json.dumps({
        "format": "GEOSPOTS1",
        "source": "KartaView (https://kartaview.org/), CC BY-SA 4.0",
        "attribution": ATTRIBUTION,
        "states": states,
        "spots": spots,
    }, ensure_ascii=False, indent=1), encoding="utf-8")

    print(f"\n{len(spots)} places across {len(states)} states -> {OUT}")
    for state in states:
        print(f"  {state:<22} {sum(1 for s in spots if s['state'] == state):>3}")
    if skipped:
        print("left out:", ", ".join(f"{s} ({n})" for s, n in skipped))
    return 0


if __name__ == "__main__":
    # --relabel re-derives states from the coordinates already banked; the
    # plain run goes back to KartaView for more photos.
    raise SystemExit(relabel() if "--relabel" in sys.argv else main())
