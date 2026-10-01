#!/usr/bin/env bash
#
# Push the Hall of Dragons - the month's public page - to the server, with no
# rebuild and no restart.
#
# The page itself is built into the binary, so a release always has a complete
# one. Its art is NOT: the eggs, the cards, the crests, the baked textures and
# the models are about a dozen megabytes, which is no business of a binary. The
# bot serves all of it out of <runtime>/live/, and serves an index.html it finds
# there in place of the built-in one (src/channels/discord/control/live_ui.rs),
# picking a change up within a second or two.
#
# So a page-only change is this script, not a release.
#
#   scripts/push-live.sh                 # the page and all of its art
#   scripts/push-live.sh index.html      # just the page
#   LIVE_HOST=root@1.2.3.4 scripts/push-live.sh
#   scripts/push-live.sh --host root@1.2.3.4 --runtime /srv/vizier/.runtime
#
# The page's own files live in the checkout at src/channels/discord/control/live
# (index.html, which is the one built into the binary) and its art in liveart/,
# which is not in git - see liveart/README.md. The defaults are the MLCI server;
# anything can be overridden by an option or by the matching environment
# variable: LIVE_HOST, LIVE_RUNTIME, LIVE_KEY, LIVE_SRC, LIVE_ART.
#
# Nothing here is undone by a release: a later binary still carries its own copy
# of the page, and removing index.html from <runtime>/live goes back to it.

set -euo pipefail

HOST="${LIVE_HOST:-root@37.27.180.72}"
RUNTIME="${LIVE_RUNTIME:-/root/vizier/.vizier/.runtime}"
KEY="${LIVE_KEY:-$HOME/.ssh/hetzner_mlci}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="${LIVE_SRC:-$ROOT/src/channels/discord/control/live}"
ART="${LIVE_ART:-$ROOT/liveart}"
DRY=""

FILES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --runtime) RUNTIME="$2"; shift 2 ;;
    --key) KEY="$2"; shift 2 ;;
    --src) SRC="$2"; shift 2 ;;
    --art) ART="$2"; shift 2 ;;
    --dry-run|-n) DRY="1"; shift ;;
    -h|--help) sed -n '2,28p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "push-live: unknown option $1" >&2; exit 2 ;;
    *) FILES+=("$1"); shift ;;
  esac
done

[ -d "$SRC" ] || { echo "push-live: no live directory at $SRC" >&2; exit 1; }
[ -f "$KEY" ] || { echo "push-live: no ssh key at $KEY" >&2; exit 1; }

DEST="$RUNTIME/live"
SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes)

# Named files come from the checkout; named nothing means the page, plus the art
# directory if one was given.
if [ ${#FILES[@]} -eq 0 ]; then
  while IFS= read -r f; do FILES+=("$(basename "$f")"); done < <(find "$SRC" -maxdepth 1 -type f ! -name '.*' | sort)
fi

echo "push-live: $SRC -> $HOST:$DEST"
for f in "${FILES[@]}"; do
  [ -f "$SRC/$f" ] || { echo "push-live: no such page file: $f" >&2; exit 1; }
  printf '  %-18s %8s bytes\n' "$f" "$(wc -c < "$SRC/$f" | tr -d ' ')"
done
if [ -n "$ART" ] && [ ! -d "$ART" ]; then
  echo "push-live: no art at $ART; sending the page only (see liveart/README.md)" >&2
  ART=""
fi
if [ -n "$ART" ]; then
  printf '  %-18s %8s\n' "(art)" "$(du -sh "$ART" | cut -f1)"
fi

if [ -n "$DRY" ]; then
  echo "push-live: dry run, nothing sent"
  exit 0
fi

"${SSH[@]}" "$HOST" "mkdir -p '$DEST'"
for f in "${FILES[@]}"; do
  scp -q -i "$KEY" -o IdentitiesOnly=yes "$SRC/$f" "$HOST:$DEST/$f"
done
# The art keeps its own subdirectories (eggs/, cards/, guide/), which is how the
# page asks for it.
if [ -n "$ART" ]; then
  rsync -az --exclude 'index.html' --exclude '_src/' --exclude 'README.md' \
    -e "ssh -i $KEY -o IdentitiesOnly=yes" "$ART/" "$HOST:$DEST/"
fi

echo "push-live: done. The page picks it up within a second or two; no restart."
