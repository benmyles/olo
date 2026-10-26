#!/bin/bash
# genicons.sh [names...] — generate missing icons from data/icons/manifest.txt
cd "$(dirname "$0")/.."
grep -v '^#' data/icons/manifest.txt | grep -v '^$' | while IFS='|' read -r name subject; do
  if [ $# -gt 0 ] && [[ ! " $* " =~ " $name " ]]; then continue; fi
  [ -f "data/icons/src/$name.png" ] && [ $# -eq 0 ] && continue
  printf '%s\0%s\0' "$name" "$subject"
done | xargs -0 -n2 -P6 python3 tools/genicon.py
