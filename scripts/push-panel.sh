#!/usr/bin/env bash
# Put the panel's own files on the server, without building or restarting.
#
# The bot reads these from <workspace>/.runtime/ui and notices a new copy on
# its own, so a panel-only change - a page, a chart, wording, styling - is live
# as soon as this finishes. Anything that changes the bot itself still needs a
# build and a restart; this only carries what the browser loads.
set -euo pipefail
HOST=${VIZIER_HOST:-root@37.27.180.72}
KEY=${VIZIER_KEY:-$HOME/.ssh/hetzner_mlci}
UI=/root/vizier/.vizier/.runtime/ui
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$HERE/src/channels/discord/control/ui"

cd "$SRC"
FILES=(*.js *.css *.html *.svg)
echo "pushing ${#FILES[@]} panel files to $HOST"
scp -q -i "$KEY" -o IdentitiesOnly=yes "${FILES[@]}" "$HOST:$UI/"

# What the server now has, so a push can be checked at a glance.
ssh -i "$KEY" -o IdentitiesOnly=yes "$HOST" "
  cd '$UI'
  md5sum app.js | cut -c1-12
  ls -la app.js | awk '{print \$5, \$6, \$7, \$8}'
"
echo "live - reload the panel (hard refresh if it looks the same)"
