#!/bin/bash
# Copy the newest achievement definitions the EVE client downloaded from CCP's CDN into
# out/cdn/achievements/, where the `achievements` content section reads them. Run it on the
# host (WSL), not in tools/analysis.sh: the container sees the repo as /work but not the
# Windows user profile.
#
# The client writes the file only after a character has ENTERED THE GAME on Tranquility
# (character selection is not enough):
#   %LOCALAPPDATA%\CCP\EVE\<install>_tranquility\cache\Storage\achievements\data.bin
#
# usage: tools/fetch-cdn-achievements.sh [CACHE_DIR]
#   CACHE_DIR  a ...\cache\Storage\achievements directory (default: the newest version under
#              /mnt/c/Users/*/AppData/Local/CCP/EVE/*_tranquility/)
# The file is CCP's: it stays in out/ (gitignored) and goes nowhere near git.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
dest="$here/../out/cdn/achievements"

if [ $# -ge 1 ]; then
    src=$1
else
    src=""
    best=""
    for d in /mnt/c/Users/*/AppData/Local/CCP/EVE/*_tranquility/cache/Storage/achievements; do
        [ -f "$d/data.bin" ] || continue
        v=$(cat "$d/version.txt" 2>/dev/null || echo 0.0.0)
        if [ -z "$best" ] || [ "$(printf '%s\n%s\n' "$best" "$v" | sort -V | tail -1)" = "$v" ]; then
            best=$v
            src=$d
        fi
    done
    if [ -z "$src" ]; then
        echo "fetch-cdn-achievements: no Tranquility cache with achievements/data.bin under" >&2
        echo "  /mnt/c/Users/*/AppData/Local/CCP/EVE/*_tranquility/ -- enter the game once on" >&2
        echo "  Tranquility (character selection is not enough), or pass the directory" >&2
        exit 1
    fi
fi

[ -f "$src/data.bin" ] || { echo "fetch-cdn-achievements: no data.bin in $src" >&2; exit 1; }
if [ "$(dd if="$src/data.bin" bs=1 skip=4 count=4 2>/dev/null)" != "ACHV" ]; then
    echo "fetch-cdn-achievements: $src/data.bin is not an achievements file (no ACHV)" >&2
    exit 1
fi
mkdir -p "$dest"
cp "$src/data.bin" "$dest/data.bin"
if [ -f "$src/version.txt" ]; then cp "$src/version.txt" "$dest/version.txt"; fi
echo "fetch-cdn-achievements: $(cat "$dest/version.txt" 2>/dev/null || echo '?') from $src -> out/cdn/achievements/"
