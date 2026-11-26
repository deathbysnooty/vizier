# Operating the MLCI bot (Loduchand)

Read this before changing or shipping anything. `CLAUDE.md` (also `AGENTS.md`)
explains the code; this explains the **live deployment** that code runs in.

The Vizier binary runs one Discord bot for the MLCI server. Members and the
owner call it **Loduchand**. It is a live community with real members mid-game:
treat every restart, setting change and post as public.

## The one rule

**Production runs on a Hetzner VPS, never on a developer machine.** Do not run
`vizier run` anywhere else with the live bot token: the server would get two
bots answering everything. Develop with `cargo build`, `cargo test`,
`cargo clippy` only. `scripts/deploy.sh` holds the server address and key path
(the key is not in the repo; the owner provides it).

## Build and test

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"   # Rust comes from Homebrew rustup
cargo check
cargo test              # ~1,650 tests; run the area you touched, e.g. cargo test kalesh
cargo clippy
```

`build.rs` runs the WebUI build only if `webui/node_modules/` exists; otherwise
it needs a pre-built `webui/build/client/`. Run `just install` before WebUI work.

Commits are conventional (`feat:`, `fix:`, `doc:`, `chore:` ...), one logical
change each, committed to `master`.

## Ship (deploy) — ask the owner first

Deploying **restarts the live bot**. Get a clear yes from the owner for each
deploy; a yes to the code change is not a yes to the restart.

1. Commit and push to `master`.
2. Build: `gh workflow run build-linux.yml --ref master -R deathbysnooty/vizier`.
   Always pass `-R deathbysnooty/vizier` — without it `gh` resolves to the
   upstream repo. About 5 minutes. Find the run id with
   `gh run list -R deathbysnooty/vizier --workflow build-linux.yml --limit 1`,
   wait with `gh run watch <id> --exit-status`.
3. `scripts/deploy.sh <run-id>` installs that build, leaves the restart marker
   (so the bot tells live game rooms it is back), restarts the service, clears
   `/pause`, and pushes the panel files.
4. Verify: service active, no `ERROR`/`panicked` since boot, and the boot log
   has no "could not register the slash commands".

**Not everything needs a build:**
- Panel pages (`src/channels/discord/control/ui/*`): `scripts/push-ui.sh`, live
  within seconds, no restart.
- The live page (`control/live/index.html`, `liveart/`): `scripts/push-live.sh`.
- Bank content (`moviebank/`, `geobank/`, ...): rebuild the bank, rsync, **and
  restart** (banks are read once at boot).

**Art does not travel with the binary.** `arenaart/`, `liveart/` and the card
PNGs are gitignored and rsynced separately; forgetting them fails silently
(cards render as letters). Photo banks (`moviebank/images`, `geobank/images`)
are gitignored too.

## Things that will bite you

- **Slash-command ceiling.** Discord allows 100 global chat commands. Over
  the limit, the bulk registration fails and no new command appears. Look
  for the ERROR line at boot. In `mod.rs`, `MONTH_OFF` takes commands off
  during a themed month and `PAUSED_OFF` takes commands off while the Cup is
  paused. Right-click commands are counted separately.
- **Settings precedence.** `control::var` reads the saved panel value before the
  environment, so an env var can't override a stored setting. A value written
  straight into `.runtime/control.db` does **not** take effect until restart
  (the panel updates the in-memory cache; SQL doesn't).
- **JSON into the server's sqlite: write it through a file.** Build the SQL
  locally, `scp` it up, run `sqlite3 db < file.sql`, then read it back and
  parse it. Never splice JSON into a nested-quote `ssh` string. A broken
  `announce_news_done` parses as an empty list and the bot reposts every old
  release note, one a minute. Check `announce_news_done` after any write.
- **Discord API from a script** needs a `User-Agent` header or every call is a
  bare 403 that looks like a bad token.
- **macOS `pgrep -fc` lies** (no `-c`): it reports 0 while the job runs. Use
  `pgrep -fl`.
- **Chess and Letter Duel deliberately pay no House Cup points**, and Sudoku and
  chess-puzzles too. Don't "fix" that. The full table of who pays what, and the
  daily caps, is in `src/channels/discord/points.rs` (`Source::cap()`) and
  `house::award_person`.
- **Copy limits.** The welcome text is within ~20 characters of Discord's 2,000
  limit; release notes are capped at 1,200 (`NEWS_LIMIT`). Both are enforced by
  tests that only fail once the copy is written.

## Telling members

A change members can see (new game, scoring, setting) needs a release note:
add a `News` entry in `announce.rs`; `channels_for` picks the rooms. Mod-panel-
only changes and fixes members can't see do not. Edit the existing message in
the game-updates channel rather than posting a new one. **Check the target
channels still exist first** — several rooms from the old Fire & Blood month
were archived or deleted.

## Disk

The server disk filled on 17 Nov 2026. `uploads/` is now swept every 6 hours
(`src/file_manager/sweep.rs`): upload folders older than
`VIZIER_UPLOADS_KEEP_DAYS` (default 30; `0` keeps everything) are deleted
unless a memory points at them. Watch `df -h /` after anything that stores
files.

## Working style that has worked

- One change per commit; build and test locally before pushing.
- Report outcomes plainly: what shipped, what was verified, what was not.
- Never claim a feature works until it has been used on the real server; say
  "deployed, not yet tried on a real message" when that is the truth.
- If two people (or agents) edit this repo at once, use separate git worktrees
  and let one of them own `master`, building and deploying. Concurrent edits to
  `mod.rs` and `catalog.rs` clobber silently.
