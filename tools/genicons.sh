#!/bin/bash
# genicons.sh [--theme ID[,ID…]] [names...] — generate missing icons from
# data/icons/manifest.txt, for the classic set or for each theme named
# (data/themes/ID). JOBS (default 6) requests run at once.
cd "$(dirname "$0")/.."
themes=""
if [ "$1" = "--theme" ]; then themes="$2"; shift 2; fi
IFS=',' read -ra themes <<< "${themes:-,}"
for theme in "${themes[@]}"; do
  src="data/icons/src"
  [ -n "$theme" ] && src="data/themes/$theme/src"
  grep -v '^#' data/icons/manifest.txt | grep -v '^$' | while IFS='|' read -r name subject; do
    if [ $# -gt 0 ] && [[ ! " $* " =~ " $name " ]]; then continue; fi
    [ -f "$src/$name.png" ] && [ $# -eq 0 ] && continue
    printf '%s\0%s\0%s\0' "${theme:---no-theme}" "$name" "$subject"
  done
done | xargs -0 -n3 -P"${JOBS:-6}" sh -c 'if [ "$0" = --no-theme ]; then exec python3 tools/genicon.py "$1" "$2"; else exec python3 tools/genicon.py --theme "$0" "$1" "$2"; fi'
