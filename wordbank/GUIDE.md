# Anagrams word bank

Two data files for the Anagrams game. The bot picks a word from `puzzles.txt`,
scrambles its letters into the channel, and accepts the first message that is a
word in `dictionary.txt` using **exactly** the same multiset of letters.

| file | lines | size | what it is |
|---|---|---|---|
| `puzzles.txt` | 5,787 | 42 KB | words the bot is allowed to **scramble** — common, everyday English, 4–9 letters |
| `dictionary.txt` | 171,607 | 1.7 MB | words the bot **accepts** as answers — every legitimate English word of 4+ letters |

Both files: plain UTF-8 (in practice pure ASCII), one lowercase `[a-z]+` word
per line, sorted ascending, no duplicates, LF line endings, single trailing
newline. Every word in `puzzles.txt` is also in `dictionary.txt` (checked).

Scoring bands the game uses (for reference): 4–5 letters = 1 pt, 6–7 = 2 pts,
8+ = 3 pts.

## Sources and licences

| source | used for | licence |
|---|---|---|
| [ENABLE 2k](https://raw.githubusercontent.com/dolph/dictionary/master/enable1.txt) (`dolph/dictionary`, 172,823 words) | the whole of `dictionary.txt` | **public domain** (ENABLE was released to the public domain by Alan Beale / M. Cooper; the repo is MIT) |
| [hermitdave/FrequencyWords](https://github.com/hermitdave/FrequencyWords) `2018/en/en_50k.txt` (OpenSubtitles 2018) | spoken-English frequency rank | CC-BY-SA 4.0 |
| [Norvig `count_1w.txt`](https://norvig.com/ngrams/) (Google Web Trillion Word Corpus unigrams, 333,333 words) | written/web frequency rank | free to use, attribution to Peter Norvig / Google ngrams |
| [dsojevic/profanity-list](https://github.com/dsojevic/profanity-list) `en.json` (434 entries, tagged `racial` / `lgbtq` / `sexual` / `general` / `religious` + severity 1–4) | primary slur + profanity filter | MIT |
| [LDNOOBW](https://github.com/LDNOOBW/List-of-Dirty-Naughty-Obscene-and-Otherwise-Bad-Words) `en` (403 entries) | extra profanity filter, `puzzles.txt` only | CC-BY-4.0 |
| [HurtLex EN 1.2](https://github.com/valeriobasile/hurtlex) (categories `ps`, `om`) | cross-check for slurs the other lists missed | CC-BY-NC-SA 4.0 — used only as a *reference while curating*, no HurtLex data is redistributed here |
| [dominictarr/random-name](https://github.com/dominictarr/random-name), [smashew/NameDatabases](https://github.com/smashew/NameDatabases) | flagging personal names for manual review | MIT / public data |

`dictionary.txt` is a filtered copy of ENABLE (public domain), so the shipped
files carry no copyleft obligation. The frequency and profanity lists were used
only as *selectors* during the build; none of their content is redistributed.

ENABLE is the standard choice for word games (it is the list behind Words With
Friends and most open Scrabble tooling): it is lowercase-only, contains **no
proper nouns, no abbreviations and no apostrophes/hyphens by construction**, so
the "drop anything capitalised-only / non-`a-z`" rule removed 0 entries — the
rule is still enforced in code so a different source list can be swapped in.

## Pipeline, with counts at every step

### `dictionary.txt`

| # | step | count |
|---|---|---|
| 1 | ENABLE raw entries | 172,823 |
| 2 | after Unicode fold + `[a-z]+` only + must not be capitalised in source | 172,823 (−0) |
| 3 | length ≥ 4 (3-letter words are never answers) | 171,755 |
| 4 | − slurs | −80 |
| 5 | − hard profanity | −68 |
| 6 | **`dictionary.txt`** | **171,607** |

### `puzzles.txt`

| # | step | count |
|---|---|---|
| 1 | `dictionary.txt` words 4–9 letters | 104,057 |
| 2 | frequency filter — common in **both** corpora (see below) | 6,051 |
| 3 | − any word on *any* profanity list, at any grade, plus regular inflections (puzzle list is stricter than the answer list) | −60 → 5,991 |
| 4 | − personal names / place names / brands (hand-reviewed, see below) | −183 → 5,808 |
| 5 | − chat-speak, interjections, archaisms (`gonna`, `psst`, `thee`, `hath`) | −13 → 5,795 |
| 6 | − palindromes (a scramble can come back as the word itself) | −8 → 5,787 |
| 7 | − fewer than 4 distinct letter arrangements (e.g. all-same-letter words) | −0 → 5,787 |
| 8 | **`puzzles.txt`** | **5,787** |

**The frequency filter.** A word is "common" only if it is frequent in *both* a
spoken corpus (OpenSubtitles top-50k) and a written corpus (Google web
unigrams). With `s` = OpenSubtitles rank and `n` = web rank, a word qualifies if

```
s ≤ 25,000  and  n ≤ 40,000  and  sqrt(s × n) < 9,000
```

Requiring both corpora is what keeps the list recognisable: the web corpus alone
floods the list with `keywords`, `thumbnail`, `shareware`, `inkjet`; the
subtitle corpus alone floods it with names and interjections. The geometric mean
lets a word that is very common in speech but middling on the web (`spoon`,
`kitten`, `elbow`, `carrot`, `biscuit`, `mango`) in, while keeping the total near
the target size. Nothing was hand-picked *into* the list — hand work only ever
removed words.

**Solvability.** Every puzzle word is itself a common word, so every puzzle has
at least one answer an ordinary player can reach — that is the real solvability
guarantee here. On top of that, palindromes and words with fewer than 4 distinct
arrangements are dropped so a scramble can't trivially come back as the answer.
The game code should still re-scramble if the shuffle equals the original.

**Multiple answers are a feature.** 2,226 of the 5,787 puzzle words (38.5%) share
their letter set with at least one other word in `dictionary.txt`, so more than
one answer is accepted; for 569 of them, two or more of those answers are
themselves common words (`least` / `steal` / `slate` / `stale` / `tales`).
The mean number of accepted answers per puzzle is 1.71, and the richest letter
set is `aeprs` with 11 accepted answers (`spare`, `spear`, `pares`, `parse`,
`pears`, `reaps`, `apers`, `apres`, `asper`, `prase`, `presa`).

Distribution of accepted answers per puzzle: 1 answer → 3,561 puzzles,
2 → 1,220, 3 → 538, 4 → 241, 5 → 134, 6 → 47, 7 → 33, 8 → 7.

## Offensive-word filtering

The bot never *says* a puzzle word's letters in order, but it does **accept**
dictionary words — so a slur in `dictionary.txt` means the bot would confirm a
slur as a winning answer. Two layers, both driven by published lists:

**Slurs (80 forms removed from `dictionary.txt`).** Everything
`dsojevic/profanity-list` tags `racial` or `lgbtq`, plus regular inflections of
those stems (a list that rejects `faggot` but accepts `faggots` is not filtered),
plus a hand-picked set of unambiguous slurs found in HurtLex `ps`/`om` but
missing from dsojevic (`boche`, `mongoloid`, `mick`, `nance`, `poove`, `wog`,
`faggotry`…). This deliberately removes some innocuous homographs — `chink` (a
gap), `dyke` (a levee), `coon`, `kraut`, `spick`, `pansy`, `sissy` — because
the cost of occasionally rejecting a valid anagram is far lower than the cost of
the bot congratulating someone for a slur.

*HurtLex was not applied wholesale*: its `ps` category tags `fool`, `simple`,
`mark`, `spade`, `savage`, `peasant` and its `om` category tags `gay`, `queer`,
`queen`, `fairy`, `chicken` — all of which must stay playable. Only the
unambiguous entries were taken.

*Documented exceptions, kept as valid answers*: `swastika` (a Hindu/Buddhist/Jain
religious symbol, not a slur) and `hadji` / `haji` / `hajji` (a respectful term
for someone who has performed the Hajj) — both are tagged `racial` by the
published list but are not slurs for this server's audience.

*Precautionary extra removal*: the `niggard` / `niggardly` family. Etymologically
unrelated to the slur, but a bot announcing it as a winner reads badly. The
`niggle` family is kept.

**Hard profanity (68 forms removed from `dictionary.txt`).** dsojevic severity 4
(the `fuck` / `cunt` / n-word / `rape` families) and severity 3 entries tagged
`sexual` or `general` that have no ordinary sense — explicit sex-act and
paraphilia vocabulary (`blowjob`, `fellatio`, `gangbang`, `bestiality`,
`necrophilia`, `sodomize`, `dildo`, …).

**Deliberately still accepted** (the spec says mild rude words are fine as
answers, and rejecting a real word players might type is its own failure):
`shit`, `crap`, `damn`, `hell`, `bitch`, `bastard`, `asshole`, `arse`, `piss`,
`pussy`, `whore`, `twat`, plus clinical and ordinary-sense words a naive filter
eats — `nude`, `nudity`, `orgasm`, `sexy`, `panties`, `clitoris`, `vibrator`,
`vibratory`, `ejaculate`, `intercourse`, `fingering`, `collared`, `snatch`,
`spunk`, `grope`, `poon`, `gay`, `queer`, `lesbian`, `spice`/`spicy` (which a
careless `spic` prefix rule would destroy).

**`puzzles.txt` is stricter.** No word on *any* of the three profanity lists at
*any* severity is ever scrambled, plus a short taste list (`piss`, `dicks`,
`asses`, `booty`, `bastards`, `suicide`, `abortion`, `racist`, `nazi`, …) — 60
words in total. The bot should never look like it is posting a crude word.

The exact removal list, with a `slur` / `hard-profanity` label per word, is
regenerated at `out/removed_from_dictionary.txt` by the build script.

## Proper nouns and names

ENABLE contains no proper nouns, but the frequency corpora do: lowercased names
(`jerry`, `peter`, `bailey`) and places (`japan`, `texas`, `paris`) are frequent,
and a few of them are also legitimate ENABLE common nouns (`japan` = a lacquer,
`harry` = to harass, `shea` = a tree). Automated detection does not work here —
a surname list flags 1,678 of the candidates, almost all of which are ordinary
words (`bell`, `cook`, `hill`, `walker`, `baker`), and no frequency-ratio
heuristic separates `jerry` from `bell`.

So the whole candidate list was read length by length and 183 words were
removed by hand: given names and surnames that read as names (`alan`, `carl`,
`harry`, `peter`, `murphy`, `gibson`, `chandler`, `einstein`…), place and
nationality words (`china`, `japan`, `texas`, `berlin`, `brazil`, `french`,
`pacific`, `shanghai`…), and brand/character words (`honda`, `yahoo`, `batman`,
`superman`). Words whose ordinary sense dominates were kept (`mark`, `bill`,
`frank`, `rose`, `amber`, `grace`, `hope`, `robin`, `iris`, `olive`, `pearl`,
`storm`, `porter`, `potter`, `walker`, `turkey`, `griffin`, `phoenix`). They all
remain valid *answers* either way — this only decides what gets scrambled.

## Length histogram of `puzzles.txt`

| letters | points | words | share |
|---|---|---|---|
| 4 | 1 | 890 | 15.4% |
| 5 | 1 | 1,152 | 19.9% |
| 6 | 2 | 1,197 | 20.7% |
| 7 | 2 | 1,123 | 19.4% |
| 8 | 3 | 843 | 14.6% |
| 9 | 3 | 582 | 10.1% |

By points band: 1 pt 2,042 (35.3%), 2 pts 2,320 (40.1%), 3 pts 1,425 (24.6%).

## Regenerating

The build scripts are not in the repo (they only exist to produce these two
files); they live in the scratchpad used for this task:

```
/private/tmp/claude-501/-Users-ejazahmad-papersearch/934cd6ae-d10f-4ee8-968d-87f174fbf871/scratchpad/wordbank_build/
  fetch.sh      curl every source listed above into this directory
  build.py      the pipeline (all thresholds are constants at the top)
  curation.py   the hand-reviewed exception lists, each one commented
  verify.py     the sanity checks
```

```bash
bash fetch.sh                       # download the sources
python3 build.py                    # writes out/dictionary.txt, out/puzzles.txt, out/stats.txt
python3 verify.py out               # all checks must pass
cp out/dictionary.txt out/puzzles.txt <repo>/wordbank/
```

To make the puzzle list bigger or smaller, change `SCORE_MAX` in `build.py`
(9,000 → 5,787 words; 12,000 → ~7,700; 7,000 → ~4,400). Everything else is
mechanical.

`verify.py` checks: LF endings, single trailing newline, pure ASCII, every line
`[a-z]+`, sorted, no duplicates, `puzzles ⊆ dictionary`, length bounds, no
palindromes, no all-one-letter words, and that no single length is more than 40%
of the puzzle list. It exits non-zero on any failure, so it is safe to run in CI.
