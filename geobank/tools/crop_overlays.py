#!/usr/bin/env python3
"""Cut the dashcam overlay off every photo in geobank/images/.

Almost every photo in the bank came off a dashcam, and dashcams print on the
picture: a timestamp, the speed, the camera model - and some print the
latitude and longitude. One of the first World photos read
`N 35.439922 E 139.646057` in its bottom corner; anybody could paste that into
a map and have the answer before reading a single signboard.

A detector for that text finds about one photo in fifty, but a detector that
misses one leaks the answer, so this does not try to be clever. Every photo
loses the same thin strip off the bottom and the top, which is where dashcams
put their overlays. What is lost is dashboard, wiper and sky: nothing anyone
was going to place a street by.

It is idempotent - a photo is cut once - because cutting twice would eat into
the street. The names of the photos already cut are kept in
`images/.cropped`, beside the photos, so the record travels with them.

Needs Pillow; put it in a virtualenv, it is a build tool:

    /tmp/geo-venv/bin/pip install pillow
    /tmp/geo-venv/bin/python geobank/tools/crop_overlays.py

Run it after every harvest, before the bank is rsynced.
"""

import sys
from pathlib import Path

from PIL import Image

IMAGES = Path(__file__).resolve().parent.parent / "images"
DONE = IMAGES / ".cropped"

# Measured on the bank: the overlays sit in the bottom 2-6% of the frame, the
# odd camera prints along the top. A margin over each.
TOP = 0.05
BOTTOM = 0.09


def main() -> int:
    done = set(DONE.read_text().split()) if DONE.exists() else set()
    todo = [f for f in sorted(IMAGES.glob("*.jpg")) if f.name not in done]
    cut = 0
    for f in todo:
        try:
            im = Image.open(f)
            im.load()
        except Exception as err:
            print(f"  ! {f.name}: {err}", file=sys.stderr)
            continue
        w, h = im.size
        box = (0, int(h * TOP), w, h - int(h * BOTTOM))
        im.crop(box).convert("RGB").save(f, "JPEG", quality=90, optimize=True)
        done.add(f.name)
        cut += 1
        # Written as it goes: a run cut short must not cut the same photos
        # again next time.
        if cut % 100 == 0:
            DONE.write_text("\n".join(sorted(done)) + "\n")
    DONE.write_text("\n".join(sorted(done)) + "\n")
    print(f"cut {cut} photos; {len(done)} done in all")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
