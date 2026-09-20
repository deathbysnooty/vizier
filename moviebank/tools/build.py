#!/usr/bin/env python3
"""moviebank/ai/*.jsonl -> moviebank/movies.json, the one file the bot reads.

Run the checker first; this only builds what it is given.

    python3 moviebank/tools/build.py
"""

import glob
import json
import os
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FORMAT = "MOVIEBANK1"
ATTRIBUTION = (
    "Film stills and data from TMDB (https://www.themoviedb.org). "
    "This product uses the TMDB API but is not endorsed or certified by TMDB."
)


def load() -> list:
    movies = []
    for path in sorted(glob.glob(os.path.join(HERE, "ai", "*.jsonl"))):
        for n, line in enumerate(open(path, encoding="utf-8"), 1):
            line = line.strip()
            if not line:
                continue
            try:
                movies.append(json.loads(line))
            except json.JSONDecodeError as err:
                sys.exit(f"{os.path.basename(path)}:{n}: {err}")
    return movies


def main() -> None:
    movies = load()
    if not movies:
        sys.exit("no movies in moviebank/ai/ — nothing to build")
    out = os.path.join(HERE, "movies.json")
    bank = {"format": FORMAT, "attribution": ATTRIBUTION, "movies": movies}
    with open(out, "w", encoding="utf-8") as f:
        json.dump(bank, f, ensure_ascii=False, indent=1)
        f.write("\n")
    shots = sum(len(m.get("shots", [])) for m in movies)
    hints = sum(len(m.get("hints", [])) for m in movies)
    lines = sum(len(m.get("dialogues", [])) for m in movies)
    print(f"{len(movies)} movies, {shots} stills, {hints} hint lines, {lines} dialogue lines -> {os.path.relpath(out, os.getcwd())}")


if __name__ == "__main__":
    main()
