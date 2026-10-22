#!/bin/sh
# Renders the README's showcase images from src/ into PNGs beside this
# script, a dark and a light one each, at twice their size. Needs Google
# Chrome or Chromium; fetches Inter once into src/fonts (not kept in git).
# Pages named on the command line are rendered alone.
#
#   sh docs/assets/render.sh [page...]
set -eu
cd "$(dirname "$0")"
only=" $* "

chrome=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}
if [ ! -x "$chrome" ]; then
	chrome=$(command -v chromium || command -v chromium-browser || command -v google-chrome || true)
fi
[ -n "$chrome" ] || { echo "render.sh: no Chrome or Chromium found; set CHROME" >&2; exit 1; }

font=src/fonts/Inter.woff2
if [ ! -s "$font" ]; then
	mkdir -p src/fonts
	curl -fsSL "https://cdn.jsdelivr.net/fontsource/fonts/inter:vf@latest/latin-wght-normal.woff2" -o "$font"
fi

shot() { # page width height
	case $only in "  " | *" $1 "*) ;; *) return 0 ;; esac
	for theme in dark light; do
		"$chrome" --headless=new --disable-gpu --hide-scrollbars --allow-file-access-from-files \
			--force-device-scale-factor=2 --virtual-time-budget=3000 \
			--window-size="$2,$3" --screenshot="$1-$theme.png" \
			"file://$PWD/src/$1.html?$theme" >/dev/null 2>&1
		echo "$1-$theme.png"
	done
}

shot hero 1280 560
shot session 1280 500
shot cockpits 1280 648
shot architecture 1280 600
shot appearance 1280 664
