# Puzzle bank: the chess pack for the puzzle game

The bot shows a chess position in a Discord channel and members type the move
that wins it. This directory holds the puzzles. Nothing here is generated at
runtime: the bot reads two files at startup (~520 KB together) and is ready.

Every puzzle in the pack has been replayed against a real chess engine's move
generator: the position is legal, every move in the line is legal in turn, and
the line ends in **checkmate or a plainly winning capture** — no "wins a pawn in
twelve moves if you know the endgame" puzzles. See *Verification* below.

## Source and attribution

The puzzles are the **Lichess puzzle database**
(`https://database.lichess.org/lichess_db_puzzle.csv.zst`, the 2026-09-09
release, 6,100,952 puzzles). Lichess releases its databases into the **public
domain under CC0 1.0**, so no permission is needed and no credit is legally
required — but the puzzles were mined from millions of real games and rated by
millions of solvers, and saying where they came from costs one line.

**Put this exact line in the game's help text:**

```
Puzzles from the Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0.
```

Each puzzle keeps its Lichess id, so a puzzle can always be linked back to
`https://lichess.org/training/<id>` — a good "see the full solution" link for
the end of a round.

## The one thing to get right: the FEN is *one move early*

This trips up everybody who uses this database, so read this section twice.

**The `fen` column is the position BEFORE the opponent's last move.** It is not
the position the member should be shown. The first move in `moves` is that
opponent move; playing it reaches the position the solver actually sees.

```
fen  ──play moves[0]──▶  the position you post to Discord  ──moves[1]──▶ ... 
(opponent to move)        (solver to move: THIS is the puzzle)
```

So:

- `moves[0]` is played **by the side to move in the FEN** — the opponent.
- The solver's colour is therefore the **opposite** of the FEN's side to move.
  A FEN ending in ` b ` means the *solver plays White*.
- `moves[1]`, `moves[3]`, ... are the solver's moves — what a member has to
  type to win the round.
- `moves[2]`, `moves[4]`, ... are the opponent's replies, which the bot plays
  automatically, and the line always **ends on a solver move**.
- Hence the move count is always even: `2 * (number of solver moves)`. In this
  pack that is `2` (one solver move) or `4` (two solver moves), nothing longer.

Why the database is built this way: the opponent's blunder is part of the story,
and animating it before handing over the board is what Lichess itself does. For
a Discord bot the practical upshot is: **always push `moves[0]` before you
render the board.** Render the FEN as given and the puzzle is wrong — a piece
sits on the wrong square and the "solution" looks like nonsense.

Moves are **UCI**: from-square then to-square (`e2e4`), with the promotion piece
appended in lowercase for a promotion (`e7e8q`, 125 puzzles here). Castling is
the *king's* move, two squares (`e1g1` = O-O, 20 in the pack); en passant is
just the pawn's diagonal move (`e5d6`, 3 in the pack). There is never any
punctuation: no `x`, no `+`, no `#`.

### Worked example

Line 0 of `puzzles.tsv` (118 bytes), tabs shown as `→`:

```
WqnOB→6k1/7p/p1P1b1p1/5p2/8/6P1/r5BP/2R3K1 b - - 0 29→e6c8 g2d5 g8g7 d5a2→800→easy→crushing endgame fork master short
```

Split it on tabs and you get the six columns:

| column | value |
| --- | --- |
| `id` | `WqnOB` → https://lichess.org/training/WqnOB |
| `fen` | `6k1/7p/p1P1b1p1/5p2/8/6P1/r5BP/2R3K1 b - - 0 29` |
| `moves` | `e6c8 g2d5 g8g7 d5a2` |
| `rating` | `800` |
| `band` | `easy` |
| `themes` | `crushing endgame fork master short` |

The FEN says `b`, so Black moves first — Black is the **opponent** and the
member is playing **White**.

**A. the FEN as stored** — not what you post:

```
 8 | . . . . . . k . |
 7 | . . . . . . . p |
 6 | p . P . b . p . |      Black to move (the opponent)
 5 | . . . . . p . . |
 4 | . . . . . . . . |
 3 | . . . . . . P . |
 2 | r . . . . . B P |
 1 | . . R . . . K . |
     a b c d e f g h
```

**B. after `moves[0]` = `e6c8` (Bc8)** — post *this* one:

```
 8 | . . b . . . k . |
 7 | . . . . . . . p |
 6 | p . P . . . p . |      White to move — the puzzle
 5 | . . . . . p . . |
 4 | . . . . . . . . |
 3 | . . . . . . P . |
 2 | r . . . . . B P |
 1 | . . R . . . K . |
     a b c d e f g h
```

Black has just retreated the bishop to c8, and it and the rook on a2 now stand
on the same diagonal with the king in between — the `fork` theme.

**C. the rest of the line:**

| move | UCI | SAN | who |
| --- | --- | --- | --- |
| 1 | `g2d5` | `Bd5+` | **solver** — the answer the member must type |
| 2 | `g8g7` | `Kg7` | opponent, played by the bot |
| 3 | `d5a2` | `Bxa2` | **solver** — wins the rook, end of line |

```
 8 | . . b . . . . . |
 7 | . . . . . . k p |
 6 | p . P . . . p . |      a rook up: the line is over
 5 | . . . . . p . . |
 4 | . . . . . . . . |
 3 | . . . . . . P . |
 2 | B . . . . . . P |
 1 | . . R . . . K . |
     a b c d e f g h
```

A one-solver-move puzzle looks the same, minus the last two rows:

```
4kYFv→r1bqkb1r/pp1pp1np/2n2p2/6NB/3p3B/4P3/PPP2PPP/RN1QK2R b KQkq - 1 9→g7h5 d1h5→802→easy→mate mateIn1 oneMove opening
```

Black plays `g7h5` (`Nxh5??`), you post the position, and the single winning
move is `d1h5` — `Qxh5#`.

## Files

| file | what it is |
| --- | --- |
| `puzzles.tsv` | all 4,000 puzzles, one per line, six tab-separated columns |
| `index.json` | 800 bytes of metadata: the band slices and the attribution line |
| `build.py` | the script that regenerates both from the Lichess database |

### `puzzles.tsv` — format `PZTSV1`

TSV, not a binary pack, because every field is already short ASCII text that the
bot passes straight to a chess crate. A FEN *is* a string; encoding it as bytes
would only mean decoding it again a millisecond later. The whole file is 517 KB
and 4,000 lines, so the "parser" is `split('\n')` then `split('\t')` — cheaper
than any binary format's bounds checks, and the file stays greppable when
someone reports a bad puzzle.

```
line := id \t fen \t moves \t rating \t band \t themes \n
```

| # | column | what it is |
| --- | --- | --- |
| 0 | `id` | Lichess puzzle id, 4-5 chars, `[A-Za-z0-9]`. Unique. Link: `https://lichess.org/training/<id>` |
| 1 | `fen` | standard 6-field FEN, **one move before the puzzle** (see above) |
| 2 | `moves` | space-separated UCI moves; `moves[0]` is the opponent's, the rest alternate solver/opponent; always 2 or 4 of them |
| 3 | `rating` | Lichess Glicko rating of the puzzle, 800-2099 |
| 4 | `band` | `easy`, `medium` or `hard` — derived from `rating`, and the same value for every line in a band's slice |
| 5 | `themes` | space-separated Lichess theme tags, 3-11 of them, e.g. `fork`, `mateIn2`, `backRankMate` |

Guarantees the reader may rely on:

- pure ASCII, Unix newlines, a trailing newline, no quoting, no escaping, no
  empty field; no field ever contains a tab or a newline
- exactly 4,000 lines; longest line 218 bytes
- sorted by band (`easy`, then `medium`, then `hard`) and by `rating` inside a
  band, so a band is one contiguous run of lines
- no two puzzles share a FEN

### `index.json`

(shown with the band objects on one line each; the file itself is indented)

```json
{
 "format": "PZTSV1",
 "pack": "puzzles.tsv",
 "source": "Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0",
 "attribution": "Puzzles from the Lichess puzzle database (https://database.lichess.org/#puzzles), CC0 1.0.",
 "columns": ["id", "fen", "moves", "rating", "band", "themes"],
 "puzzles": 4000,
 "bands": [
  {"band": "easy",   "line": 0,    "count": 1800, "offset": 0,      "bytes": 229576, "rating_min": 800,  "rating_max": 1299},
  {"band": "medium", "line": 1800, "count": 1400, "offset": 229576, "bytes": 183007, "rating_min": 1300, "rating_max": 1699},
  {"band": "hard",   "line": 3200, "count": 800,  "offset": 412583, "bytes": 104029, "rating_min": 1700, "rating_max": 2099}
 ]
}
```

- `line` / `count` — the band's slice of the line list, 0-based: `easy` is lines
  `0..1800`. This is all you need if you read the file into a `Vec<Puzzle>`.
- `offset` / `bytes` — the same slice as absolute byte positions in
  `puzzles.tsv`, for a reader that wants to `seek` one band instead of loading
  the file. `offset` always lands on the first byte of a line.
- `attribution` — the help-text line, so it is not duplicated in code.

`index.json` is small enough to hard-code if you would rather not pull in serde
for it, but keeping it means a rebuild that changes the counts does not need a
code change.

### Reading it in Rust

Half a megabyte: read it whole at startup and borrow out of it, the way
`guess_bank.rs` reads `doodles.bin`. No parser, no allocation per field.

```rust
pub struct Bank {
    raw: String,                 // puzzles.tsv, kept alive so rows can borrow it
}

pub struct Puzzle<'a> {
    pub id: &'a str,
    pub fen: &'a str,            // the position BEFORE the opponent's move
    pub moves: &'a str,          // space-separated UCI; moves[0] is the opponent's
    pub rating: u16,
    pub band: &'a str,
    pub themes: &'a str,
}

impl Bank {
    pub fn load(dir: &std::path::Path) -> anyhow::Result<Bank> {
        let path = dir.join("puzzles.tsv");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("{}: {}", path.display(), e))?;
        Ok(Bank { raw })
    }

    pub fn rows(&self) -> impl Iterator<Item = Puzzle<'_>> {
        self.raw.lines().filter(|l| !l.is_empty()).map(|line| {
            let mut c = line.split('\t');
            let (id, fen, moves, rating, band, themes) = (
                c.next().unwrap(), c.next().unwrap(), c.next().unwrap(),
                c.next().unwrap(), c.next().unwrap(), c.next().unwrap(),
            );
            Puzzle { id, fen, moves, rating: rating.parse().unwrap_or(0), band, themes }
        })
    }

    /// Lines `line .. line + count` from index.json — or just filter on `band`.
    pub fn band<'a>(&'a self, slice: &BandSlice) -> impl Iterator<Item = Puzzle<'a>> {
        self.rows().skip(slice.line).take(slice.count)
    }
}
```

Dealing a round, with the *one* rule that matters:

(`Board::from_fen` / `play_uci` stand in for whichever chess crate you pick —
with `shakmaty` they are `Fen::from_ascii(..)?.into_position(..)` and
`Position::play`.)

```rust
let p = pick_random(&bank, band);
let mut board = Board::from_fen(p.fen)?;
let mut it = p.moves.split(' ');

// 1. play the opponent's move FIRST — this is the position the members see
board.play_uci(it.next().unwrap())?;
post_board(&board);                       // render, then ask for the move

// 2. the member's answer must equal the next UCI move
let answer = it.next().unwrap();          // e.g. "g2d5"

// 3. if the line is four moves long, play the opponent's reply and ask again
if let Some(reply) = it.next() {
    board.play_uci(reply)?;
    post_board(&board);
    let answer2 = it.next().unwrap();
}
```

Two small kindnesses for the members, both on the bot's side rather than the
bank's:

- **accept SAN as well as UCI.** People type `Bd5+`, not `g2d5`. Parse the
  guess as SAN against the current position with the chess crate and compare the
  resulting move to the expected UCI; fall back to comparing the raw strings
  case-insensitively. Accepting only UCI will feel broken to anyone who plays.
- **an alternative mate is still a mate.** If the guess is not the stored move
  but delivers checkmate, count it. Lichess's line is one winning path, not the
  only one.

### Themes as hints

`themes` is what makes `!hint` easy: `fork` → "this one is a fork", `mateIn2` →
"mate in two", `backRankMate` → "the back rank is the weakness". The tags are
Lichess's own vocabulary (64 distinct tags appear in this pack). Some describe
the *puzzle*, some only its setting:

- **motif tags, the useful ones for hints** — `fork` (546), `pin` (345),
  `discoveredAttack` (284), `sacrifice` (202), `deflection` (177),
  `hangingPiece` (164), `skewer` (82), `attraction` (74), `trappedPiece` (89),
  `interference` (22), `xRayAttack` (15), `zugzwang` (1), and named mates:
  `backRankMate` (47), `smotheredMate` (34), `operaMate` (68), `arabianMate`
  (9), `bodenMate` (1), and a dozen more.
- **outcome tags** — `mate` (2015), `mateIn1` (1018), `mateIn2` (997),
  `crushing` (1021), `advantage` (964). `crushing` means "wins a lot of
  material", `advantage` "wins some".
- **length tags** — `oneMove` (1018) and `short` (2982): exactly the two line
  lengths in this pack, so `oneMove` is a reliable "one move wins it" hint.
- **setting tags, no hint value** — `opening` (349), `middlegame` (1921),
  `endgame` (1730), `master`, `masterVsMaster`, `superGM`, and the endgame kinds
  (`rookEndgame`, `queenEndgame`, ...). Every shipped puzzle has at least one
  tag that is *not* one of these, so there is always something to say.

A theme is never a spoiler in itself — it names the tactic, not the square.

## How the bank was built

Starting from the 2026-09-09 release, 6,100,952 puzzles. Each filter is applied
to what survived the previous one:

| step | filter | why | left |
| --- | --- | --- | --- |
| 0 | the whole database | | 6,100,952 |
| 1 | `800 <= rating < 2100` | below 800 is "take the free queen", above 2100 stops being fun in a chat window | 4,582,016 |
| 2 | line length 2 or 4 UCI moves | one or two solver moves; longer lines do not survive Discord's attention span | 3,151,378 |
| 3 | `ratingDeviation <= 80` | the rating has settled, so the band is meaningful | 2,032,685 |
| 4 | `popularity >= 90` | solvers' own up/down votes: only puzzles people liked | 1,191,588 |
| 5 | `nbPlays >= 500` | well-tested, not a puzzle five people have seen | 904,066 |
| 6 | `popularity >= 94`, `nbPlays >= 1000`, unique FEN | the tight pool the bank is drawn from (71 duplicate positions dropped) | 454,084 |
| 7 | stratified selection + replay validation | see below | **4,000** |

Step 7, per band: the quota is split between one-move and two-move puzzles, then
spread evenly over the band's 100-point rating buckets (so `easy` is not all
1290s), and drawn at random from each bucket with a fixed seed (`20260917`).
Each drawn puzzle then had to pass three more tests, or it was put back:

- **replay** — the FEN parses, every move is legal in turn, the sides alternate,
  the line ends on a solver move;
- **a visible payoff** — the line ends in checkmate *or* a material swing of at
  least 2 pawns for the solver, measured from the position the member is shown.
  This is what rejected 634 drawn puzzles: real Lichess puzzles whose point is a
  zugzwang, an unstoppable passed pawn or a pin that only pays off later. They
  are fine puzzles and terrible chat puzzles — nobody wants to be told they were
  wrong about a position that still looks equal.
- **a usable hint** — at least one theme that is not just `short`/`oneMove` or a
  phase/strength tag.

A cap of 3% of a band's quota per identical theme-set also applied, so no band is
500 copies of "short middlegame crushing" (597 skips).

### The bands

| band | rating | puzzles | share | one-move | two-move |
| --- | --- | --- | --- | --- | --- |
| `easy` | 800-1299 (mean 1055) | 1,800 | 45% | 630 | 1,170 |
| `medium` | 1300-1699 (mean 1497) | 1,400 | 35% | 308 | 1,092 |
| `hard` | 1700-2099 (mean 1895) | 800 | 20% | 80 | 720 |

Weighted toward the easy end on purpose: this is a chat game, and a puzzle
nobody in the channel can solve is a dead round. A Lichess rating of ~1000 is
"a club player sees it instantly, a casual player sees it in ten seconds", and
~1900 is "worth a minute of staring". The one-move share drops as the band gets
harder (35% / 22% / 10%): an easy round should often be over in one move, while
a hard round earns its two.

Overall the pack is 2,015 checkmates and 1,985 material wins; the material wins
swing between +2 and +14 pawns, median +5 — a rook or better, most of the time.

## Verification

`build.py` validates every puzzle as it is drawn, so the shipped file is
verified by construction; the check was then re-run from scratch over the
finished file with `python-chess` (an independent replay of the exact bytes in
`puzzles.tsv`, not of the build's in-memory rows):

```
rows: 4000
FULL BANK: 4000/4000 pass; 0 fail
  outcomes: {'mate': 2015, 'material win': 1985}
  material swings: min +2, median +5, max +14
  theme cross-check notes: none
RANDOM SAMPLE OF 40 (full move-by-move replay): 40/40
```

Each row passed: FEN parses; `moves[0]` legal in it; every later move legal in
turn and played by the expected side; line ends on a solver move; ends in mate or
a >= +2 swing. The theme cross-check additionally confirmed the semantics from
the other direction — every `mateIn1` puzzle really is mate after **one** solver
move (not one *ply*), every `mateIn2` mate after two, and `oneMove`/`short`
agree with the line lengths. If `moves[0]` were the solver's move instead of the
opponent's, those 1,018 `mateIn1` puzzles would all have failed.

## Regenerating

```sh
pip install chess                 # python-chess, for the replay validation
python3 puzzlebank/build.py       # ~6 min: streams 300 MB, writes puzzles.tsv + index.json
```

`build.py` pipes `curl | zstd -dc` straight into a line loop and keeps only the
~900k rows that survive the coarse filter, so the 300 MB download and the 1.5 GB
of CSV behind it never touch the disk. `--keep-pool` writes that survivor set to
`candidates.tsv` (~120 MB) if you want to re-select without downloading again
(`--from-pool candidates.tsv`); delete it afterwards.

The selection is deterministic for a given input: re-running against the same
release reproduces `puzzles.tsv` byte for byte. Lichess keeps adding puzzles, so
a rebuild against a later release will pick a different 4,000 — which is a fine
way to refresh the bank, but do it deliberately, not by accident, because member
"seen it already" tracking is keyed by puzzle id.

To change the mix, the knobs are at the top of `build.py`: `BANDS` (name, range,
quota), `ONE_MOVE_SHARE`, `RD_MAX`, `POP_MIN` / `PLAYS_MIN`, `MOVE_COUNTS`.
Raising `MOVE_COUNTS` to include 6 would add three-solver-move puzzles; the
format already handles them (the line still ends on a solver move), but the
reader's "play the reply, ask again" loop has to become a real loop.
