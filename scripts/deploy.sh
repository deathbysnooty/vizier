#!/usr/bin/env bash
#
# Put a new build on the server and restart it.
#
# The binary is built by GitHub Actions (.github/workflows/build-linux.yml) so
# that a Linux binary never depends on anyone's laptop being awake. This script
# takes the artifact from a run of that workflow, installs it, restarts the
# service and brings the bot back out of /stop without anybody typing it in
# Discord.
#
#   scripts/deploy.sh                  # the newest successful build of master
#   scripts/deploy.sh 35515263828      # a particular run
#   scripts/deploy.sh --no-ui          # binary only, leave the panel files
#   scripts/deploy.sh --dry-run
#
# Defaults are the MLCI server; DEPLOY_HOST, DEPLOY_KEY, DEPLOY_RUNTIME and
# DEPLOY_AGENT override them.
#
# What it does NOT do: announce the change. That is a person's job - a post in
# the four house common rooms and an edit to the pinned game-updates message.
# The bot tells the game rooms itself (src/channels/discord/updates.rs): a word
# on the way down, and another once it is back.

set -euo pipefail

HOST="${DEPLOY_HOST:-root@37.27.180.72}"
KEY="${DEPLOY_KEY:-$HOME/.ssh/hetzner_mlci}"
RUNTIME="${DEPLOY_RUNTIME:-/root/vizier/.vizier/.runtime}"
AGENT="${DEPLOY_AGENT:-vizier}"
REPO="${DEPLOY_REPO:-deathbysnooty/vizier}"
ARTIFACT="vizier-linux-x86_64"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN=""
UI="1"
DRY=""

while [ $# -gt 0 ]; do
  case "$1" in
    --no-ui) UI=""; shift ;;
    --dry-run|-n) DRY="1"; shift ;;
    --host) HOST="$2"; shift 2 ;;
    --key) KEY="$2"; shift 2 ;;
    -h|--help) sed -n '2,22p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*) echo "deploy: unknown option $1" >&2; exit 2 ;;
    *) RUN="$1"; shift ;;
  esac
done

[ -f "$KEY" ] || { echo "deploy: no ssh key at $KEY" >&2; exit 1; }
command -v gh >/dev/null || { echo "deploy: needs the gh cli" >&2; exit 1; }

SSH=(ssh -i "$KEY" -o IdentitiesOnly=yes)

if [ -z "$RUN" ]; then
  RUN="$(gh run list -R "$REPO" --workflow build-linux.yml --status success --limit 1 --json databaseId -q '.[0].databaseId')"
  [ -n "$RUN" ] || { echo "deploy: no successful build to take" >&2; exit 1; }
fi

SHA="$(gh run view "$RUN" -R "$REPO" --json headSha -q .headSha)"
echo "deploy: run $RUN, commit ${SHA:0:8}"
if [ "$(git -C "$ROOT" rev-parse HEAD)" != "$SHA" ]; then
  echo "deploy: note - that is not the commit checked out here ($(git -C "$ROOT" rev-parse --short HEAD))"
fi

if [ -n "$DRY" ]; then
  echo "deploy: dry run, nothing sent"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
gh run download "$RUN" -R "$REPO" -n "$ARTIFACT" -D "$TMP"
BIN="$(find "$TMP" -type f -name vizier | head -1)"
[ -n "$BIN" ] || { echo "deploy: no binary in the artifact" >&2; exit 1; }
chmod +x "$BIN"
echo "deploy: $(du -h "$BIN" | cut -f1) to $HOST"

# Beside the running one first, then moved into place: a half-copied binary
# must never be what the service manager finds when it restarts.
scp -i "$KEY" -o IdentitiesOnly=yes "$BIN" "$HOST:/usr/local/bin/vizier.new"
"${SSH[@]}" "$HOST" "
  set -e
  mv /usr/local/bin/vizier.new /usr/local/bin/vizier
  chmod +x /usr/local/bin/vizier
  # The note the new process reads to know the restart was asked for, so it
  # tells the game rooms it is back.
  date +%s > '$RUNTIME/restarting'
  systemctl restart vizier
  # Out of /stop without anybody typing it in Discord. Admin-only mode, if it
  # is on, is left exactly as it was.
  sqlite3 '$RUNTIME/vizier.db' \"INSERT OR REPLACE INTO state (key, value) VALUES ('${AGENT}__paused', 'false');\"
  sleep 3
  systemctl is-active vizier
  /usr/local/bin/vizier --version
"

if [ -n "$UI" ]; then
  "$ROOT/scripts/push-ui.sh" >/dev/null
  echo "deploy: panel files pushed"
fi

echo "deploy: done. Now post the change in the four house common rooms and edit the game-updates message."
