# Place bank: how Geo's photos and answers are built

Geo is GeoGuessr for India and the World: the bot posts a street photo and the
room types where it was taken. Before each match the room votes India, World
or Mix. An India round is gated by the **state**; a World round asks for the
**country** and nothing finer. The bot reads every file here once at boot.

## Files

```
geobank/
  places.json       India answers — 36 states/UTs + 2,928 towns        (tracked)
  spots.json        India photos  — where each was taken, its state    (tracked)
  world.json        World answers — 210 countries + 34,101 towns       (tracked)
  world_spots.json  World photos  — where each was taken, its country  (tracked)
  images/           every photo, both banks, ~1024-1280px JPEGs        (NOT in git)
  images/.cropped   which photos have had their dashcam overlay cut off
  tools/build.py             places.json, from the GeoNames India dump
  tools/harvest.py           spots.json + images/, from KartaView
  tools/harvest_mapillary.py the same, from Mapillary, for the states KartaView misses
  tools/build_world.py       world.json, from GeoNames cities15000 + countryInfo
  tools/harvest_world.py     world_spots.json, from Mapillary's vector tiles
  tools/crop_overlays.py     cuts the dashcam overlay off every photo - RUN LAST
```

`images/` is gitignored for the same reason `moviebank/images/` is: it runs to
hundreds of megabytes. Unlike the movie bank, though, Geo **cannot** fall back
to anything — a round with no photo is not a round — so a workspace without the
images keeps the game switched off and says so in the log.

## Building it

```sh
# the answers: 15 MB download, a few seconds
curl -sL -o IN.zip https://download.geonames.org/export/dump/IN.zip
curl -sL -o admin1.txt https://download.geonames.org/export/dump/admin1CodesASCII.txt
unzip -q IN.zip
python3 geobank/tools/build.py IN.txt admin1.txt

# the photos: ~40 minutes, mostly waiting on the API
python3 geobank/tools/harvest.py

# and the states KartaView misses, which needs a free Mapillary token
export MAPILLARY_TOKEN='MLY|...'
python3 geobank/tools/harvest_mapillary.py
```

The Mapillary token is used **here**, when the bank is built, and never by the
bot: nothing about it goes into the config, the repo or the server. Register an
application at <https://www.mapillary.com/dashboard/developers> with READ scope
only and copy its client token.

`harvest.py` **merges**: photos already on disk are kept and each state is only
topped up to `TARGET_PER_STATE`, so re-running fills gaps rather than starting
over. Run it twice — the first pass always loses some downloads to timeouts.

## The World bank

```sh
curl -sL -o cities15000.zip https://download.geonames.org/export/dump/cities15000.zip
curl -sL -o countryInfo.txt https://download.geonames.org/export/dump/countryInfo.txt
unzip -q cities15000.zip
python3 geobank/tools/build_world.py cities15000.txt countryInfo.txt

python3 -m venv /tmp/geo-venv && /tmp/geo-venv/bin/pip install mapbox-vector-tile pillow
/tmp/geo-venv/bin/python geobank/tools/harvest_world.py --start=0 --limit=75 --budget=500
/tmp/geo-venv/bin/python geobank/tools/harvest_world.py --start=999 --final
```

`harvest_world.py` reads Mapillary's **vector tiles**, not its `/images`
search. The search returns HTTP 500 for Brazil, the US, Japan and South Africa
whatever you ask it - those are among the most photographed places on Earth
and it times out counting them. The tiles work everywhere, and they carry a
`foot` flag saying whether a frame was taken walking, which is the
parked-van filter done properly.

It saves after **every** country and remembers which it has tried, so a run
cut short loses nothing and the same command simply carries on. `--budget`
stops it starting new countries after that many seconds. `--final` applies
the 12-photo floor once every slice has had its turn.

It takes the 75 most populous countries, which is roughly the most
recognisable. That leaves out some a room in India would want - Nepal, Sri
Lanka, Bhutan - and add them with `--only=NP,LK,BT --retry`.

## Cutting the overlays off

Almost every photo came off a dashcam, and dashcams print on the picture: the
time, the speed, the camera model - and some print the **latitude and
longitude**. One of the first World photos read `N 35.439922 E 139.646057` in
its corner, which is the answer.

`crop_overlays.py` cuts the same thin strip off the bottom (9%) and top (5%) of
every photo, where the overlays sit. A detector for the text finds about one
photo in fifty, but a detector that misses one leaks the answer, so this cuts
them all; what is lost is dashboard, wiper and sky. It is idempotent - each
photo is cut once, recorded in `images/.cropped` - so run it after every
harvest and before every rsync.

## Deploying it

No `cargo build` is needed for bank content, but the bot reads the bank once at
boot, so the server needs the files and a restart:

```sh
rsync -az geobank/places.json geobank/spots.json geobank/world.json geobank/world_spots.json \
  geobank/images root@37.27.180.72:/root/vizier/.vizier/geobank/
```

macOS ships rsync 2.6.9, which rejects `--info=`; stick to `-az`. Cropping
rewrites every photo, so the first rsync after it re-sends the lot.

## What the photos are

Street-level photographs contributed by the public to two archives, both CC
BY-SA 4.0, credited wherever they appear. Nobody curated them for beauty: they
are roads, shopfronts, signboards, traffic and the occasional windscreen wiper.
That is the point — the clues are the things a photo of a real street happens
to contain.

**[KartaView](https://kartaview.org/)** is dashcam footage, and almost every
frame faces the road. Uniform and very playable, but it pools where a few
people drove: 13 states on its own, and Tamil Nadu and Telangana alone are most
of its raw supply. Needs no token.

**[Mapillary](https://www.mapillary.com/)** reaches most of the rest — Mumbai,
Chennai, Ahmedabad, Kochi, Guwahati and Bhubaneswar all have imagery there and
none on KartaView — but it is a mixed bag, because it also holds phone photos
taken on foot. A photograph of the side of a parked van is not a round anybody
can win, so `harvest_mapillary.py` is fussier than its sibling: flat images
only, `quality_score` above 0.5, and a frame only counts when several photos
from the **same sequence** sit in the same neighbourhood, which is what a drive
looks like and a snapshot does not.

Mapillary's licence asks for the individual photographer, so every spot keeps
its contributor in `by` and the reveal credits them.

**This is why the game draws a state first and a photo second.** Sampling
photos directly would make "Tamil Nadu" a winning guess without looking at the
screen. Both harvesters cap each state at `TARGET_PER_STATE` and drop any state
that cannot reach `MIN_SPOTS`, and `geo::pick` then draws evenly among the
states that survived.

Run KartaView first and Mapillary second: the Mapillary pass only tops up
states that are short, so the better imagery wins wherever it exists.

## How a guess is judged

`places.json` holds every spelling of every place, already folded to a plain
form: lower case, no punctuation, no accents. `geo_bank::fold` has to agree with
`fold()` in `build.py`, because one folds the keys and the other folds what
somebody typed. **Change one and you must change the other.**

- The **state** is worth 2. A town within 60 km is 4; within 15 km, 5.
- Naming a town carries its state, so nobody has to know both.
- The state is a **gate**: a town in the wrong state scores nothing, whatever
  the kilometres say. A town at the far end of the right state still earns 2.
- Where a name reads two ways — Delhi is a UT and a city — the reading that
  scores better for that photo wins.
- Where two towns share a name, the bigger by population owns it, decided once
  when `places.json` is built.

## Tuning the harvest

| Knob | What it does |
|---|---|
| `TARGET_PER_STATE` | photos to hold per state (60) — the evenness cap |
| `MIN_SPOTS` | below this a state is left out rather than shipped thin (20) |
| `CITIES_PER_STATE` | how many towns in a state to look around (18) |
| `CELL` | one photo per ~330 m, so a state is not one junction over and over |
| `DAY_FROM` / `DAY_TO` | daylight only; night frames are unguessable |

Known rough edge: there is no sharpness check, so a few frames are motion
blurred. If that bites, filter on file size or a Laplacian variance before the
download step.
