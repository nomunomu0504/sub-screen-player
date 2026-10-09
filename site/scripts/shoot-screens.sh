#!/bin/sh
# Draws each screen of the gallery (public/screens/<name>/) with its example figures into
# src/assets/gallery/<name>.png at the panel's size, for the gallery page. Uses the headless
# Chrome that `ssp web --install` downloaded, or the program in $CHROME.
#
#   sh scripts/shoot-screens.sh            # every screen
#   sh scripts/shoot-screens.sh system     # one
set -eu
cd "$(dirname "$0")/.."
chrome=${CHROME:-}
if [ -z "$chrome" ]; then
	for c in "$HOME"/Library/Application\ Support/sub-screen-player/chrome/*/chrome-headless-shell-*/chrome-headless-shell \
		"$HOME"/.local/share/sub-screen-player/chrome/*/chrome-headless-shell-*/chrome-headless-shell; do
		[ -x "$c" ] && chrome=$c
	done
fi
[ -n "$chrome" ] || { echo "no headless Chrome: run \`ssp web --install\` or set CHROME" >&2; exit 1; }
mkdir -p src/assets/gallery
names=${*:-$(ls public/screens)}
for name in $names; do
	page="$PWD/public/screens/$name/index.html"
	[ -f "$page" ] || { echo "no screen $name" >&2; exit 1; }
	# The example figures move every second; a few seconds fill the graphs.
	"$chrome" --headless --hide-scrollbars --window-size=1920,462 --virtual-time-budget=12000 \
		--screenshot="$PWD/src/assets/gallery/$name.png" "file://$page?clean" > /dev/null 2>&1
	echo "src/assets/gallery/$name.png"
done
