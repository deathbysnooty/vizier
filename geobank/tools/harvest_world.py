#!/usr/bin/env python3
"""Pull street-level photos from around the world into geobank/world_spots.json.

The sibling of `harvest_mapillary.py`, and it borrows that file's filters
wholesale: flat images only, a quality score above the floor, and a frame only
counts when several photos from the same sequence sit in the same
neighbourhood, which is what a drive looks like and a snapshot does not.

What differs is the unit. An India round is gated by its STATE, so that
harvester balances states; a World round is gated by its COUNTRY, so this one
balances countries. Each is capped at TARGET_PER_COUNTRY and any country that
cannot reach MIN_SPOTS is left out rather than shipped thin — a country with
four photographs would repeat itself the moment it came up.

A photo's country is the nearest town's, exactly as a photo's state is. It is
wrong only within a few kilometres of a border, and it keeps the answer the
game accepts and the answer it reveals in agreement.

## Why it reads vector tiles

Mapillary's `/images` search answers for India but returns HTTP 500 for Brazil,
the United States, Japan and South Africa - whatever the box size, the row
limit, the fields asked for or a date window. Those are among the most heavily
photographed places on Earth (a single San Francisco tile holds 130,000
frames) and the search times out trying to count them.

The vector tiles Mapillary's own map is drawn from do not have that problem,
and they carry something the search does not: a `foot` flag saying whether a
frame was taken walking. That is exactly the pedestrian-snapshot signal
`harvest_mapillary.py` has to approximate from sequence lengths, so here it is
read directly. The tile gives ids and flags; each chosen photo is then fetched
on its own for its picture, its position and its photographer.

Needs MAPILLARY_TOKEN, and the `mapbox-vector-tile` package to decode tiles -
put it in a virtualenv, it is a build tool and nothing the bot needs:

    python3 -m venv /tmp/geo-venv && /tmp/geo-venv/bin/pip install mapbox-vector-tile
    /tmp/geo-venv/bin/python geobank/tools/harvest_world.py --start=0 --limit=60 --budget=480
"""

import json
import os
import sys
import time
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import math
import urllib.parse
import urllib.request

import mapbox_vector_tile

from harvest import BANK, CELL, IMAGES, dedupe, haversine
from harvest_mapillary import QUALITY, RUN, token

TILES = "https://tiles.mapillary.com/maps/vtp/mly1_public/2/14/{x}/{y}?access_token={tok}"
GRAPH = "https://graph.mapillary.com/{id}?access_token={tok}&fields=computed_geometry,thumb_1024_url,creator,captured_at"
# The image layer only exists at zoom 14, a tile about 2.4 km across.
ZOOM = 14


def tile_of(lat, lon):
    n = 2 ** ZOOM
    x = int((lon + 180) / 360 * n)
    y = int((1 - math.log(math.tan(math.radians(lat)) + 1 / math.cos(math.radians(lat))) / math.pi) / 2 * n)
    return x, y


def point_of(x, y, px, py, extent):
    """A tile-local point, back to latitude and longitude."""
    n = 2 ** ZOOM
    lon = (x + px / extent) / n * 360 - 180
    lat = math.degrees(math.atan(math.sinh(math.pi * (1 - 2 * (y + py / extent) / n))))
    return lat, lon


def look_tile(lat, lon, tok, tries=3):
    """Every drivable frame in the tile around a town: flat, taken from a
    vehicle, above the quality floor, and part of a sequence long enough to be
    a drive. Returns (id, lat, lon, quality)."""
    x, y = tile_of(lat, lon)
    url = TILES.format(x=x, y=y, tok=urllib.parse.quote(tok))
    for attempt in range(tries):
        try:
            with urllib.request.urlopen(url, timeout=120) as r:
                body = r.read()
            break
        except Exception:
            time.sleep(2 + attempt * 3)
    else:
        return []
    layer = mapbox_vector_tile.decode(body, default_options={"y_coord_down": True}).get("image", {})
    extent = layer.get("extent", 4096)
    frames = []
    for f in layer.get("features", []):
        p = f.get("properties", {})
        if p.get("is_pano") or p.get("foot") or (p.get("quality_score") or 0) < QUALITY:
            continue
        coords = f.get("geometry", {}).get("coordinates")
        if not coords:
            continue
        flat, flon = point_of(x, y, coords[0], coords[1], extent)
        frames.append((p["id"], p.get("sequence_id"), flat, flon, p.get("quality_score") or 0))
    runs = defaultdict(int)
    for frame in frames:
        runs[frame[1]] += 1
    return [(i, la, lo, q) for i, s, la, lo, q in frames if runs[s] >= RUN]


def detail(image_id, tok):
    """The picture, true position and photographer of one frame."""
    try:
        with urllib.request.urlopen(GRAPH.format(id=image_id, tok=urllib.parse.quote(tok)), timeout=60) as r:
            return json.load(r)
    except Exception:
        return None

WORLD = BANK / "world.json"
OUT = BANK / "world_spots.json"

TARGET_PER_COUNTRY = 30
MIN_SPOTS = 12
# One tile holds far more frames than a country needs; more towns are for
# SPREAD, so the thirty photographs are not all one district. Tiles run to
# megabytes in the big cities, which is why this is four and not ten.
CITIES_PER_COUNTRY = 4

ATTRIBUTION = "Street imagery from Mapillary contributors, CC BY-SA 4.0."


def main() -> int:
    tok = token()
    world = json.loads(WORLD.read_text(encoding="utf-8"))
    countries = {c["code"]: c for c in world["countries"]}

    by_country = defaultdict(list)
    for c in world["cities"]:
        by_country[c["country"]].append(c)
    for rows in by_country.values():
        rows.sort(key=lambda r: -r["pop"])

    # Nearest town decides the country. Only towns within a couple of degrees
    # are worth measuring, which keeps this from being 34,000 comparisons a
    # photo.
    cities = world["cities"]

    def nearest_city(lat, lon):
        best, best_km = None, 1e9
        for c in cities:
            if abs(c["lat"] - lat) > 2.0 or abs(c["lon"] - lon) > 2.0:
                continue
            km = haversine(lat, lon, c["lat"], c["lon"])
            if km < best_km:
                best, best_km = c, km
        return best, best_km

    held = defaultdict(list)
    tried = set()
    if OUT.exists():
        banked = json.loads(OUT.read_text(encoding="utf-8"))
        tried = set(banked.get("tried", []))
        for spot in banked.get("spots", []):
            if (BANK / spot["file"]).exists():
                held[spot["country"]].append(spot)
    if held:
        print(f"{sum(len(v) for v in held.values())} photos already banked", flush=True)

    IMAGES.mkdir(exist_ok=True)

    def fetch(spot):
        dest = IMAGES / Path(spot["file"]).name
        if dest.exists() and dest.stat().st_size > 10_000:
            return True
        try:
            import urllib.request
            with urllib.request.urlopen(spot["url"], timeout=90) as r:
                data = r.read()
            if len(data) < 10_000:
                return False
            dest.write_bytes(data)
            return True
        except Exception:
            return False

    def save(new_spots, prune, done=None):
        """Merge into what is banked and write it out. Called after EVERY
        country, so a run cut short - and runs here do get cut short - keeps
        everything it finished instead of losing the lot."""
        bank = json.loads(OUT.read_text(encoding="utf-8")) if OUT.exists() else {"spots": []}
        spots = [s for s in bank.get("spots", []) if (BANK / s["file"]).exists()]
        spots.extend(new_spots)
        spots = dedupe(spots)
        if prune:
            counts = defaultdict(int)
            for s in spots:
                counts[s["country"]] += 1
            thin = {c for c, n in counts.items() if n < MIN_SPOTS}
            if thin:
                print(f"too thin to play, left out: {len(thin)} countries", flush=True)
            spots = [s for s in spots if s["country"] not in thin]
        codes = sorted({s["country"] for s in spots})
        seen = set(bank.get("tried", [])) | tried
        if done:
            seen.add(done)
            tried.add(done)
        OUT.write_text(json.dumps({
            "format": "GEOSPOTS1",
            "source": "Mapillary (https://www.mapillary.com/), CC BY-SA 4.0",
            "attribution": ATTRIBUTION,
            "countries": codes,
            "tried": sorted(seen),
            "spots": spots,
        }, ensure_ascii=False, indent=1), encoding="utf-8")
        return spots, codes
    # Biggest first, which is also roughly most-recognisable first. A room can
    # name Japan or Brazil from a street; nobody places Comoros, so the tail of
    # this list is not worth the harvest even where Mapillary has it.
    order = sorted(by_country, key=lambda code: -sum(c["pop"] for c in by_country[code][:3]))

    # Two hundred countries is more than one sitting: `--start N --limit M`
    # walks the list in slices, and each run merges into what is already
    # banked, so the slices add up.
    start = int(next((a.split("=")[1] for a in sys.argv if a.startswith("--start=")), 0))
    limit = int(next((a.split("=")[1] for a in sys.argv if a.startswith("--limit=")), len(order)))
    order = order[start:start + limit]
    only = next((a.split("=")[1] for a in sys.argv if a.startswith("--only=")), "")
    if only:
        order = [c for c in only.split(",") if c in by_country]
    print(f"countries {start}..{start + len(order)} of {len(by_country)}", flush=True)
    # Stop starting new countries once this many seconds have gone, so a run
    # ends cleanly instead of being cut off mid-country.
    budget = int(next((a.split("=")[1] for a in sys.argv if a.startswith("--budget=")), 10**9))
    began = time.time()

    for code in order:
        if time.time() - began > budget:
            print(f"  budget of {budget}s spent - run again to carry on", flush=True)
            break
        have = held.get(code, [])
        want = max(0, TARGET_PER_COUNTRY - len(have))
        # Every country is looked at once. A run here gets cut short, so the
        # same window is simply run again - and a country already tried, full
        # or hopeless, must cost nothing the second time round.
        if want == 0 or (code in tried and "--retry" not in sys.argv):
            continue
        probe = by_country[code][:CITIES_PER_COUNTRY]
        with ThreadPoolExecutor(max_workers=4) as ex:
            hauls = list(ex.map(lambda c: look_tile(c["lat"], c["lon"], tok), probe))

        cells = {(round(s["lat"] / CELL), round(s["lon"] / CELL)) for s in have}
        pools = []
        for frames in hauls:
            frames = sorted(frames, key=lambda f: -f[3])
            if frames:
                pools.append(frames)

        # Pick positions from the tiles first, spread one per cell and taken
        # from each town in turn; only then pay for a detail call on each.
        picked = []
        while pools and len(picked) < want:
            for pool in list(pools):
                if not pool:
                    pools.remove(pool)
                    continue
                image_id, lat, lon, _ = pool.pop(0)
                cell = (round(lat / CELL), round(lon / CELL))
                if cell in cells:
                    continue
                cells.add(cell)
                picked.append(image_id)
                if len(picked) >= want:
                    break

        with ThreadPoolExecutor(max_workers=8) as ex:
            details = list(ex.map(lambda i: detail(i, tok), picked))
        taken = []
        for image_id, d in zip(picked, details):
            if not d or not d.get("thumb_1024_url") or not d.get("computed_geometry"):
                continue
            lon, lat = d["computed_geometry"]["coordinates"]
            d["id"] = image_id
            taken.append((d, lat, lon))

        if not taken:
            save([], prune=False, done=code)
            print(f"  {countries[code]['name']:<34} nothing usable", flush=True)
            continue

        rows = []
        for item, lat, lon in taken:
            city, km = nearest_city(lat, lon)
            if city is None:
                continue
            rows.append({
                "id": f"mly{item['id']}",
                "lat": round(lat, 5),
                "lon": round(lon, 5),
                "country": city["country"],
                "city": city["name"],
                "city_km": round(km, 1),
                "file": f"images/mly{item['id']}.jpg",
                "url": item["thumb_1024_url"],
                "by": (item.get("creator") or {}).get("username") or "unknown",
                "source": "mapillary",
                "shot": time.strftime("%Y-%m-%d", time.gmtime((item.get("captured_at") or 0) / 1000)),
            })
        with ThreadPoolExecutor(max_workers=8) as ex:
            ok = list(ex.map(fetch, rows))
        rows = [s for s, good in zip(rows, ok) if good]
        for s in rows:
            s.pop("url", None)
        save(rows, prune=False, done=code)
        print(f"  {countries[code]['name']:<34} {len(have)} kept + {len(rows)} new", flush=True)

    spots, kept_codes = save([], prune="--final" in sys.argv)
    print(f"\n{len(spots)} places across {len(kept_codes)} countries -> {OUT}")
    for code in kept_codes:
        print(f"  {countries[code]['name']:<34} {sum(1 for s in spots if s['country'] == code):>3}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
