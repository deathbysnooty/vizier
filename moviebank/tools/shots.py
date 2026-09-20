#!/usr/bin/env python3
"""A contact sheet of every backdrop a film has, so stills can be approved by eye.

    export TMDB_API_KEY=...            # themoviedb.org -> settings -> API
    python3 moviebank/tools/shots.py 117691

Writes `<id>-sheet.jpg` and prints the file path under each numbered tile. Look
at the sheet, pick the frames that are really frames, and put those paths in the
film's `shots` with a note saying what you approved. TMDB serves poster art,
cast lineups and watermarked streaming rips in the same list as real stills —
see GUIDE.md. The key is only ever needed here: the bot stores the paths and
never calls the API.
"""

import json
import os
import sys
import urllib.request

API = "https://api.themoviedb.org/3/movie/{id}/images?api_key={key}"
IMAGE = "https://image.tmdb.org/t/p/w300{path}"
COLS = 5
W, H, PAD = 330, 186, 6


def backdrops(movie_id: str, key: str) -> list:
    with urllib.request.urlopen(API.format(id=movie_id, key=key)) as r:
        data = json.load(r)
    shots = data.get("backdrops", [])
    # Textless first: a backdrop tagged with a language usually has the title on
    # it. It is a weak signal - it says nothing about poster art - so the rest
    # are kept and simply come after.
    shots.sort(key=lambda b: (b.get("iso_639_1") is not None, -b.get("vote_count", 0)))
    return shots


def sheet(movie_id: str, shots: list) -> str:
    from PIL import Image, ImageDraw

    rows = (len(shots) + COLS - 1) // COLS
    out = Image.new("RGB", (COLS * (W + PAD) + PAD, rows * (H + PAD) + PAD), (24, 24, 24))
    draw = ImageDraw.Draw(out)
    for n, shot in enumerate(shots):
        with urllib.request.urlopen(IMAGE.format(path=shot["file_path"])) as r:
            tmp = f"/tmp/tmdb-{shot['file_path'].strip('/')}"
            open(tmp, "wb").write(r.read())
        tile = Image.open(tmp).convert("RGB").resize((W, H))
        x, y = PAD + (n % COLS) * (W + PAD), PAD + (n // COLS) * (H + PAD)
        out.paste(tile, (x, y))
        draw.text((x + 5, y + 4), str(n + 1), fill=(255, 255, 0))
        os.remove(tmp)
    path = f"{movie_id}-sheet.jpg"
    out.save(path, quality=88)
    return path


def main() -> None:
    key = os.environ.get("TMDB_API_KEY") or os.environ.get("VIZIER_TMDB_KEY")
    if not key:
        sys.exit("set TMDB_API_KEY first — themoviedb.org, settings, API, it is free")
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for movie_id in sys.argv[1:]:
        shots = backdrops(movie_id, key)
        if not shots:
            print(f"{movie_id}: no backdrops at all — this one plays tags and dialogue only")
            continue
        print(f"{movie_id}: {len(shots)} backdrops -> {sheet(movie_id, shots)}")
        for n, shot in enumerate(shots, 1):
            text = "" if shot.get("iso_639_1") is None else f"  (tagged {shot['iso_639_1']} — probably has the title on it)"
            print(f"  {n:>3}. {shot['file_path']}{text}")


if __name__ == "__main__":
    main()
