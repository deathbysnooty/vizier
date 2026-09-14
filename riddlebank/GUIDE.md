# Riddle bank: writer and checker guide

The riddles feed the Chocolate Frog game on the MLCI Discord server (Midlyf Crisis India: adults, mostly Indian, chatty, Hinglish banter, Harry Potter house theme). A frog appears in chat; members press a button, a private text box shows the riddle, and the **first correct typed answer** wins points. Each person gets **3 tries**. The bot checks answers automatically, so **every riddle must have one clear answer that people can type**.

## File format

One JSON object per line (JSONL), UTF-8, in `riddlebank/ai/<topic>.jsonl`:

```json
{"id":"objects-001","topic":"objects","difficulty":"easy","riddle":"I have keys but open no locks. I have space but no room. You can enter, but you can't go inside. What am I?","answers":["keyboard","computer keyboard"],"hint":"You're probably touching one right now.","lang":"en"}
```

- `id`: `<topic>-NNN`, unique, zero-padded.
- `difficulty`: `easy` | `medium` | `hard`. Mix per file: about 45% easy, 35% medium, 20% hard.
  - easy: most adults get it in under 30 seconds.
  - medium: needs a moment of thought or a pun.
  - hard: a good riddle-solver gets it within 2 minutes; still fair, never obscure trivia.
- `riddle`: at most 300 characters. Ends with a question such as "What am I?" or "What is it?".
- `answers`: the canonical answer FIRST, then every reasonable way someone might type the same correct thing: singular/plural, with/without "a", common spellings, Hindi/English names (e.g. `["jalebi","jilebi","jilapi"]`), short forms. 1–3 words each, lower case. Do NOT include answers that are a different thing.
- `hint`: optional, at most 100 characters, doesn't give the answer away.
- `lang`: `en` or `hinglish` (Hinglish = Roman-script Hindi-English mix, natural and friendly, the answer still typeable in Roman script).

## Rules

1. **One answer only.** If a smart player could defend a different answer ("a clock" vs "a watch"), either rewrite the clues until only one fits or add the other as an accepted answer when it is truly the same thing. Classic riddles with famous alternative answers must list them or be rewritten.
2. **Typeable answers.** 1–3 words. No numbers-as-words ambiguity unless both forms are listed (`["seven","7"]`). No answers that need punctuation.
3. **No trick answers that need an explanation** ("nothing", "your name") unless the riddle is a well-known classic and the answer is unambiguous; keep these under 5% of a file.
4. **Original wording.** Classic folk riddles are fine but reword them in your own words; don't copy riddles verbatim from books or websites. No song lyrics.
5. **Safe for a mixed adult community.** No sex, slurs, religion jokes, caste, politics, tragedies, real crimes, or jokes about body weight. Mild banter is fine.
6. **Harry Potter topic**: answers are objects, spells, creatures, places, characters, or Quidditch terms from the books/films; clues describe them in your own words, no quoted text. Answers must be spelled the standard way plus common misspellings (e.g. `["hedwig"]`, `["expelliarmus","expeliarmus"]`).
7. **No duplicates** within a file, and avoid the same answer appearing more than twice in a file.
8. **Hinglish** riddles: natural Roman-script Hinglish (e.g. "Main garam hoon, gol hoon, chai ke saath sabko pasand hoon. Kaun hoon main?"), never Devanagari. Answers accept both Hindi and English names where both are common.

## Checker's job

Read every line of the writer's file and for each riddle:
- Solve it yourself **before** looking at the answer. If you reach a different defensible answer, either add it to `answers` (if it's really the same thing) or rewrite the riddle so only the intended answer fits, or drop it.
- Check facts (especially Harry Potter, India, science, pop culture).
- Check the difficulty label is honest; relabel if not.
- Check `answers` covers common spellings and forms; remove any alias that is actually a different thing.
- Enforce every rule above; drop anything unsafe.
- Validate JSON and ids; keep ids stable (don't renumber).

Write the checked file back in place and write a short log to `riddlebank/reviews/<topic>.md`: kept / rewritten / relabelled / dropped counts and a line for each change.
