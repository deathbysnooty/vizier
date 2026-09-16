#!/usr/bin/env bash
#
# Push the panel's pages to the server, with no rebuild and no restart.
#
# The bot serves every file in <runtime>/ui/ in place of the copy built into
# the binary (src/channels/discord/control/ui.rs), and picks a change up within
# a second or two. So a panel-only change is this script, not a release.
#
#   scripts/push-ui.sh                 # every panel file
#   scripts/push-ui.sh app.js app.css  # just these
#   UI_HOST=root@1.2.3.4 scripts/push-ui.sh
#   scripts/push-ui.sh --host root@1.2.3.4 --runtime /srv/vizier/.runtime
#
# The defaults are the MLCI server. Anything can be overridden by an option or
# by the matching environment variable: UI_HOST, UI_RUNTIME, UI_KEY, UI_SRC.
#
# Nothing here is undone by a release: a later binary still carries its own
# copies, and removing a file from <runtime>/ui/ goes back to them.

set -euo pipefail

HOST="${UI_HOST:-root@37.27.180.72}"
RUNTIME="${UI_RUNTIME:-/root/vizier/.vizier/.runtime}"
KEY="${UI_KEY:-$HOME/.ssh/hetzner_mlci}"
SRC="${UI_SRC:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/src/channels/discord/control/ui}"
DRY=""

FILES=()
while [ $# -gt 0 ]; do
  case "$1" in
    --host) HOST="$2"; shift 2 ;;
    --runtime) RUNTIME="$2"; shift 2 ;;
    --key) KEY="$2"; shift 2 ;;
    --src) SRC="$2"; shift 2 ;;
    --dry-run|-n) DRY="1"; shift ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "push-ui: unknown option $1" >&2; exit 2 ;;
    *) FILES+=("$1"); shift ;;
  esac
done

[ -d "$SRC" ] || { echo "push-ui: no ui directory at $SRC" >&2; exit 1; }
[ -f "$KEY" ] || { echo "push-ui: no ssh key at $KEY" >&2; exit 1; }

DEST="$RUNTIME/ui"
SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes)

# Nothing named: everything the panel can serve.
if [ ${#FILES[@]} -eq 0 ]; then
  while IFS= read -r f; do FILES+=("$(basename "$f")"); done < <(find "$SRC" -maxdepth 1 -type f ! -name '.*' | sort)
fi

echo "push-ui: $SRC -> $HOST:$DEST"
for f in "${FILES[@]}"; do
  [ -f "$SRC/$f" ] || { echo "push-ui: no such panel file: $f" >&2; exit 1; }
  printf '  %-18s %8s bytes\n' "$f" "$(wc -c < "$SRC/$f" | tr -d ' ')"
done

if [ -n "$DRY" ]; then
  echo "push-ui: dry run, nothing sent"
  exit 0
fi

"${SSH[@]}" "$HOST" "mkdir -p '$DEST'"

if command -v rsync >/dev/null 2>&1; then
  # --checksum, not times: only what actually changed goes over, and a file
  # that is already identical is left alone.
  ( cd "$SRC" && rsync -av --checksum --progress -e "ssh -i '$KEY' -o IdentitiesOnly=yes" "${FILES[@]}" "$HOST:$DEST/" )
else
  ( cd "$SRC" && scp -i "$KEY" -o IdentitiesOnly=yes "${FILES[@]}" "$HOST:$DEST/" )
fi

echo
echo "push-ui: on the server now:"
"${SSH[@]}" "$HOST" "ls -l '$DEST'"
echo
echo "push-ui: done. Reload the panel — the bot picks the change up within a second or two."
echo "push-ui: the log line to look for after a restart is: panel: serving N ui files from disk"
