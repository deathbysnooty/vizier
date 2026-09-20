# Movie bank: writer and checker guide

The movies here feed **Guess the Movie** on the MLCI Discord server (adults,
mostly Indian, chatty Hinglish banter, Harry Potter house theme). The bot puts
up ONE clue about a film — a set of tags, a line of dialogue, or a still from a
scene — and the **first person to type the title** wins. Everyone can type in
the channel: guessing *is* typing.

So every entry has to be a film a room of Indian adults would actually name, and
every clue has to be one the bot can judge automatically.

## Files

Authoring happens in `moviebank/ai/<category>.jsonl`, one JSON object per line,
UTF-8. `build.py` compiles those into `moviebank/movies.json`, which is the only
file the bot reads. Never edit `movies.json` by hand.

```json
{"id":"bw-modern-001","title":"Gangs of Wasseypur","year":2012,
 "industry":"bollywood","era":"modern","difficulty":"medium","tmdb":117691,
 "answers":["gangs of wasseypur","gow"],
 "tags":["revenge","guns","coal mafia","uttar pradesh","a butcher's family","three generations","told in two parts"],
 "dialogues":["Keh ke lunga"],
 "shots":[{"path":"/riwwiv2BMD9D6L3NM4sJjHCMVsm.jpg","note":"three men under an umbrella by a bus"}]}
```

- `id`: `<bw|hw>-<iconic|modern>-NNN`, unique, zero-padded.
- `industry`: `bollywood` (Hindi) or `hollywood` (English).
- `era`: `iconic` for the older ones, `modern` for roughly 2010 onward. Only
  genuinely famous films belong in `iconic` — if a 25-year-old wouldn't place
  it, it is not iconic, it is obscure.
- `difficulty`: `easy` | `medium` | `hard`, about 45/35/20 across a file. It
  describes how sharp the visible tags are, nothing else.
- `tmdb`: the film's TMDB id, so stills can be re-checked later.

## Answers

`answers` lists every spelling somebody might type, the plain title first. The
bot forgives a great deal on top of this — see **How a guess is judged** — so
the list is for *real alternatives*, not misspellings:

- short forms people genuinely use: `ddlj`, `k3g`, `gow`, `znmd`
- both transliterations where both are common: `kabhi khushi kabhie gham` and
  `kabhi khushi kabhi gham`
- the English and Hindi names of the same film, where both are used

Do **not** list an alias that is really a different film, and never one that is
a franchise stablemate: `dhoom` must not be an answer for *Dhoom 2*.

## Tags — the main clue type

Five tags go on the card, ordered **vague first, sharp last**. `!hint` reveals
the next one down the list, so write 6 to 8 and let the sharp ones wait.

> 🎬 **guns · revenge · coal mafia · uttar pradesh · a butcher's family**

1. **Concrete beats abstract.** `a water tank`, `a pencil trick`, `a spinning
   top` — one specific image each. `love, family, songs, bollywood` is not a
   clue, it is four hundred films.
2. **Vague to sharp.** The first tag should fit dozens of films. The last should
   fit almost nothing else.
3. **No title words**, in any language. The checker enforces this.
4. **No cast names**, and no character name that amounts to the title. Places,
   years and settings are fine and good: `uttar pradesh`, `1912`, `spain`.
5. **Franchise entries must separate.** The Dark Knight's tags must not fit
   Batman Begins. No two films in the bank may share four or more tags.
6. At most 44 characters a tag. A short phrase is fine; a sentence is not.

## Dialogues

One to three lines, each the sort of thing the room would recognise.

1. **Verbatim or not at all.** If you cannot recall the exact wording, leave it
   out — a half-remembered line is worse than no clue.
2. **No title words in the line.** "Run, Forrest, run" cannot be a clue for
   *Forrest Gump*, and "Hum teen guna lagaan denge" cannot be one for *Lagaan*.
   Either pick a different line or leave `dialogues` empty.
3. Roman script for Hindi, the way people type it. No Devanagari.
4. Nothing longer than 160 characters, and nothing that needs a paragraph of
   context to land.

## Stills

`shots` holds TMDB file paths **somebody has looked at**. Never a path picked at
random, and never the film's first backdrop just because it is first.

Use `python3 moviebank/tools/shots.py <tmdb-id>` to build a numbered contact
sheet of every backdrop a film has, then approve by eye. Three things to throw
out, all of which TMDB serves up next to real frames:

- **poster and key art** — cast collages, a hero posed against a colour, faces
  floating over an explosion;
- **cast lineups** — everyone facing the camera, nobody in a scene;
- **watermarked grabs** — streaming rips with a distributor logo burned in,
  which often spell out the title.

What you want is a frame from the film: people inside a scene, doing something,
looking at each other rather than at us. Two to four a film is plenty. `note`
says what you approved, so the next person can tell whether a path still points
at the same picture.

**A film with no usable still is normal.** Leave `shots` empty — older films
especially are all poster art on TMDB — and it simply plays tags and dialogue
rounds. Do not lower the bar to fill the field.

The stills stay on TMDB's servers; the bot only stores the path. Wherever they
appear, TMDB's credit goes with them, which is why the help card carries it.

## How a guess is judged

Worth knowing, because it decides how much slack the answers need. Both the
guess and every answer are reduced the same way (`tools/normalise.py`, mirrored
in `src/channels/discord/movie_bank.rs`):

- number words become digits, so `three idiots` is `3 idiots`;
- a leading `the`/`a`/`an` is dropped;
- everything but letters and digits goes, so `Spider-Man` is `spiderman`;
- transliteration is folded — `aa`→`a`, `ee`→`i`, `oo`→`u`, `w`→`v`, `y`→`i`,
  `ph`→`f`, `ksh`→`x`, and doubled letters collapse. This is what makes
  `dilwaale`, `dilwale`, `khushee` and `khushi` one string.

Then a guess wins if it matches an answer exactly, or is within about a fifth of
the title's length in edit distance (1 slip up to 5 letters, 2 to 10, 3 to 16,
4 to 24, 5 beyond) — **as long as no other film in the bank is just as near**.

Two things are never forgiven: **digits must match exactly**, so `don` can never
take *Don 2*; and a guess that lands equally near two films takes neither.

## Checker's job

Run `python3 moviebank/tools/check_bank.py -v` and fix everything it prints. It
fails the bank on a title leaking into its own clue, on two films close enough
that a forgiving guess could take the wrong one, and on two films sharing four
or more tags. `-v` also prints each film's nearest neighbour, which is where you
see trouble coming before it is a problem.

Then read the file yourself and, for each film:

- Guess it from the five visible tags **before** reading the title. If you
  cannot, or if you reach a different film that fits just as well, sharpen the
  tags.
- Check the facts behind the tags. A tag is a claim about the film.
- Check the dialogue lines are really from it, and really worded that way.
- Check the stills are frames, not posters, and that none has text in it.
- Check `era` is honest and the film is famous enough to be named by a room of
  adults who are not film buffs.
