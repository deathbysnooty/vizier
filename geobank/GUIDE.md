# Place bank: how Geo's photos and answers are built

Geo is GeoGuessr for India: the bot posts a street photo and the room types
where it was taken. Two files feed it, and the bot reads both once at boot.

## Files

```
geobank/
  places.json     the answers  — 36 states/UTs + 2,928 towns   (tracked in git)
  spots.json      the photos   — where each one was taken       (tracked in git)
  images/         the photos themselves, ~1280x720 JPEGs        (NOT in git)
  tools/build.py      places.json, from the GeoNames India dump
  tools/harvest.py    spots.json + images/, from KartaView
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
```

`harvest.py` **merges**: photos already on disk are kept and each state is only
topped up to `TARGET_PER_STATE`, so re-running fills gaps rather than starting
over. Run it twice — the first pass always loses some downloads to timeouts.

## Deploying it

No `cargo build` is needed for bank content, but the bot reads the bank once at
boot, so the server needs the files and a restart:

```sh
rsync -a geobank/places.json geobank/spots.json geobank/images/ \
  root@37.27.180.72:/root/vizier/.vizier/geobank/
```

## What the photos are

Real dashcam frames contributed to [KartaView](https://kartaview.org/), CC
BY-SA 4.0, credited wherever they appear (`geo_bank::ATTRIBUTION`). Nobody
curated them for beauty: they are roads, shopfronts, signboards, traffic and
the occasional windscreen wiper. That is the point — the clues are the things a
photo of a real street happens to contain.

It also means the coverage is wherever somebody drove with a dashcam, which is
**13 states** and not 36. Tamil Nadu and Telangana alone are most of the raw
supply.

**This is why the game draws a state first and a photo second.** Sampling
photos directly would make "Tamil Nadu" a winning guess without looking at the
screen. `harvest.py` caps each state at `TARGET_PER_STATE` and drops any state
that cannot reach `MIN_SPOTS`, and `geo::pick` then draws evenly among the
states that survived.

To widen the map beyond 13 states, the next source is Mapillary: broader Indian
coverage, but it needs a free API token, so it is a decision for the owner
rather than something the harvester can do on its own.

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
