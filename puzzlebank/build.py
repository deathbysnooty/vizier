#!/usr/bin/env python3
"""Rebuild the Discord puzzle bank from the Lichess puzzle database (CC0).

    pip install chess
    python3 build.py                 # streams the database, writes puzzles.tsv + index.json
    python3 build.py --keep-pool     # also keeps candidates.tsv (the ~900k survivor rows)

Nothing large is ever written to disk: the 300 MB .zst is decompressed on the fly
by `zstd -dc` and only the rows that survive the coarse filter are held in memory.
Requires `curl` and `zstd` on PATH, and python-chess for the replay validation.

The selection is deterministic for a given input database (fixed SEED), but the
Lichess database itself grows, so a rebuild months later yields a different bank.
"""
import argparse, collections, json, os, random, subprocess, sys
import chess

URL = "https://database.lichess.org/lichess_db_puzzle.csv.zst"
HERE = os.path.dirname(os.path.abspath(__file__))
SEED = 20260917

# --- coarse filter (applied while streaming) ---------------------------------
RATING_LO, RATING_HI = 800, 2100     # the three bands together
MOVE_COUNTS = (2, 4)                 # 1 or 2 solver moves
RD_MAX = 80                          # rating deviation: a settled rating
POP_MIN_STREAM, PLAYS_MIN_STREAM = 90, 500
# --- fine filter (applied to the survivors) ----------------------------------
POP_MIN, PLAYS_MIN = 94, 1000

BANDS = [("easy", 800, 1300, 1800), ("medium", 1300, 1700, 1400), ("hard", 1700, 2100, 800)]
ONE_MOVE_SHARE = {"easy": 0.35, "medium": 0.22, "hard": 0.10}   # share of one-solver-move puzzles
BUCKET = 100                                                    # rating stratification width

VAL = {chess.PAWN: 1, chess.KNIGHT: 3, chess.BISHOP: 3, chess.ROOK: 5, chess.QUEEN: 9}
# themes that only describe phase or length: a puzzle whose themes are all from
# this set gives the bot nothing to say in a hint, so it is not shipped.
DESCRIPTIVE = {"short", "oneMove", "veryLong", "long", "opening", "middlegame",
               "endgame", "master", "masterVsMaster", "superGM", "equality"}


def material(board, color):
    return sum(VAL[pt] * len(board.pieces(pt, color)) for pt in VAL)


def validate(fen, moves, themes):
    """Replay the puzzle exactly the way the bot will.

    True iff the FEN parses, the first move (the opponent's) is legal in it,
    every later move is legal in turn with the sides alternating, the line ends
    on a solver move, and that ending is checkmate or a >= 2 pawn material swing
    in the solver's favour measured from the position the solver is shown.
    """
    if not set(themes.split()) - DESCRIPTIVE:
        return False
    try:
        board = chess.Board(fen)
        mvs = moves.split()
        opponent, solver = board.turn, not board.turn
        first = chess.Move.from_uci(mvs[0])
        if first not in board.legal_moves:
            return False
        board.push(first)
        start = material(board, solver) - material(board, opponent)
        for i, u in enumerate(mvs[1:], 1):
            m = chess.Move.from_uci(u)
            if m not in board.legal_moves or board.turn != (solver if i % 2 else opponent):
                return False
            board.push(m)
        if board.turn != opponent:
            return False
        if board.is_checkmate():
            return True
        return (material(board, solver) - material(board, opponent)) - start >= 2
    except Exception:
        return False


def stream_pool(keep_pool):
    """curl | zstd -dc | coarse filter. Returns the survivor rows and the counts."""
    cmd = f"curl -sS {URL} | zstd -dc --long=27"
    proc = subprocess.Popen(cmd, shell=True, stdout=subprocess.PIPE, text=True, bufsize=1 << 20)
    counts = collections.Counter()
    rows, keep = [], open(os.path.join(HERE, "candidates.tsv"), "w") if keep_pool else None
    for n, line in enumerate(proc.stdout):
        if n == 0:
            continue                      # CSV header
        # No field can contain a comma (FEN, UCI moves, space-separated themes, URL).
        p = line.rstrip("\n").split(",")
        if len(p) < 8:
            continue
        counts["total"] += 1
        pid, fen, moves, rating, rd, pop, plays, themes = p[0], p[1], p[2], int(p[3]), int(p[4]), int(p[5]), int(p[6]), p[7]
        if not (RATING_LO <= rating < RATING_HI):
            continue
        counts["rating"] += 1
        if len(moves.split()) not in MOVE_COUNTS:
            continue
        counts["length"] += 1
        if rd > RD_MAX:
            continue
        counts["rd"] += 1
        if pop < POP_MIN_STREAM:
            continue
        counts["popularity"] += 1
        if plays < PLAYS_MIN_STREAM:
            continue
        counts["plays"] += 1
        row = (pid, fen, moves, rating, pop, plays, themes)
        rows.append(row)
        if keep:
            keep.write("\t".join(map(str, row)) + "\n")
    proc.stdout.close()
    if proc.wait() != 0:
        sys.exit("download/decompress failed")
    if keep:
        keep.close()
    return rows, counts


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--keep-pool", action="store_true")
    ap.add_argument("--from-pool", help="read a previously kept candidates.tsv instead of streaming")
    args = ap.parse_args()

    if args.from_pool:
        rows, counts = [], collections.Counter()
        for line in open(args.from_pool):
            p = line.rstrip("\n").split("\t")
            rows.append((p[0], p[1], p[2], int(p[3]), int(p[5]), int(p[6]), p[7]))
        counts["plays"] = len(rows)
    else:
        rows, counts = stream_pool(args.keep_pool)
    print("streamed rows:", counts["total"])
    for k in ("rating", "length", "rd", "popularity", "plays"):
        print(f"  after {k:11s}: {counts[k]}")

    # fine filter + de-duplicate positions
    pool, seen, dup = collections.defaultdict(list), set(), 0
    for r in rows:
        if r[4] < POP_MIN or r[5] < PLAYS_MIN:
            continue
        if r[1] in seen:
            dup += 1
            continue
        seen.add(r[1])
        band = next(b for b, lo, hi, _ in BANDS if lo <= r[3] < hi)
        pool[(band, len(r[2].split()), r[3] // BUCKET)].append(r)
    tight = sum(len(v) for v in pool.values())
    print(f"  after pop>={POP_MIN}, plays>={PLAYS_MIN}, unique FEN: {tight} (dropped {dup} duplicate FENs)")

    rng, selected, rejected, cap_skips = random.Random(SEED), [], collections.Counter(), 0
    for band, lo, hi, quota in BANDS:
        buckets = list(range(lo // BUCKET, hi // BUCKET))
        sig_count, sig_cap, band_sel = collections.Counter(), max(8, int(quota * 0.03)), []
        q_one = round(quota * ONE_MOVE_SHARE[band])
        for nmoves, sub_quota in ((2, q_one), (4, quota - q_one)):
            per = [sub_quota // len(buckets)] * len(buckets)
            for i in range(sub_quota - sum(per)):
                per[i] += 1
            deficit = 0
            for i, b in enumerate(buckets):
                cand = pool[(band, nmoves, b)][:]
                rng.shuffle(cand)
                want, got = per[i] + deficit, 0
                for r in cand:
                    if got >= want:
                        break
                    if sig_count[r[6]] >= sig_cap:
                        cap_skips += 1
                        continue
                    if not validate(r[1], r[2], r[6]):
                        rejected[band] += 1
                        continue
                    sig_count[r[6]] += 1
                    band_sel.append((band, r))
                    got += 1
                deficit = want - got          # carry any shortfall to the next bucket
            if deficit:
                print(f"  ! {band}/{nmoves}-move short by {deficit}")
        selected.extend(band_sel)
        one = sum(1 for _, r in band_sel if len(r[2].split()) == 2)
        print(f"{band:6s} {lo}-{hi-1}: {len(band_sel)} puzzles ({one} one-move, {len(band_sel)-one} two-move)")
    print("replay-validation rejects while picking:", dict(rejected), "total", sum(rejected.values()))
    print("theme-signature cap skips:", cap_skips)

    order = {"easy": 0, "medium": 1, "hard": 2}
    selected.sort(key=lambda x: (order[x[0]], x[1][3], x[1][0]))

    out, index, cur, n_in_band, last = os.path.join(HERE, "puzzles.tsv"), [], None, 0, 0
    with open(out, "w", newline="\n") as f:
        for band, r in selected:
            if band != cur:
                if cur is not None:
                    index[-1].update(count=n_in_band, bytes=f.tell() - index[-1]["offset"], rating_max=last)
                index.append({"band": band, "line": 0, "offset": f.tell(), "rating_min": r[3]})
                cur, n_in_band = band, 0
            f.write("\t".join([r[0], r[1], r[2], str(r[3]), band, r[6]]) + "\n")
            n_in_band, last = n_in_band + 1, r[3]
        index[-1].update(count=n_in_band, bytes=f.tell() - index[-1]["offset"], rating_max=last)
    line = 0
    for e in index:
        e["line"], line = line, line + e["count"]

    with open(os.path.join(HERE, "index.json"), "w") as f:
        json.dump({
            "format": "PZTSV1",
            "pack": "puzzles.tsv",
            "source": "Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0",
            "attribution": "Puzzles from the Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0.",
            "columns": ["id", "fen", "moves", "rating", "band", "themes"],
            "puzzles": len(selected),
            "bands": [{"band": e["band"], "line": e["line"], "count": e["count"],
                       "offset": e["offset"], "bytes": e["bytes"],
                       "rating_min": e["rating_min"], "rating_max": e["rating_max"]} for e in index],
        }, f, indent=1)
        f.write("\n")
    print("wrote", out, os.path.getsize(out), "bytes;", len(selected), "puzzles")


if __name__ == "__main__":
    main()
