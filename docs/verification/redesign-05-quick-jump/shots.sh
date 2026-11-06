#!/bin/sh
# Capture the Quick jump palette (QJ-1 … QJ-5) from the real app.
#
# Seeds a scratch graph under target/redesign-05 (never the user's graph or config),
# drives the window with UTOPIA_SHOT_SCRIPT and writes the PNGs beside this file.
# Run it with the screen unlocked: a locked screen makes every window capture black.
#
#   docs/verification/redesign-05-quick-jump/shots.sh
#
# Then look at every PNG before using it (AGENTS.md, "Verifying a change").
set -eu
here=$(cd "$(dirname "$0")" && pwd)
cd "$here/../../.."   # repo root: `here` is docs/verification/<branch>`
out=target/redesign-05
mkdir -p "$out/config"

# The graph: a page, a person, a page a #tag made, and two days of notes. Re-importing
# into an existing graph is idempotent, so start clean.
rm -f "$out/g.utopia"
cargo run -q -p utopia-app -- import-roam "$out/g.utopia" "$here/seed.json"

script="
wait 1200; keys escape; wait 300;
keys cmd-k; wait 700; shot $here/01-empty.png;
type atl; wait 500; shot $here/02-typed.png;
keys cmd-a; type >; wait 600; shot $here/03-commands.png;
keys escape; wait 300;
keys cmd-k; type atlas; wait 500; keys enter; wait 900;
keys cmd-k; wait 700; shot $here/04-empty-recents.png;
keys cmd-a; type operations; wait 500; shot $here/05-tag-page.png;
keys escape; wait 300
"

UTOPIA_CONFIG_DIR="$PWD/$out/config" UTOPIA_SHOT_SCRIPT="$script" \
    cargo run -q -p utopia-app -- --graph "$out/g.utopia"

# Reviewer-sized PNGs, not full Retina captures.
for f in "$here"/0*.png; do
    sips -Z 1200 "$f" --out "$f" >/dev/null
done
echo "wrote $here/0*.png"
