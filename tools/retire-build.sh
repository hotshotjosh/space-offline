#!/usr/bin/env bash
# Retire an old client build: print the edits that drop it from the tree, delete its local copies.
#
#   tools/retire-build.sh OLD_BUILD [--prefix P] [--dry-run]
#
#   OLD_BUILD   the build number to retire (a row of tools/builds.tsv)
#   --prefix P  the Wine prefix root (holds pfx/drive_c/CCP/EVE); default the offline prefix.
#   --dry-run   run every check, print every step, delete nothing
#
# docs/guides/build-compat.md "Retiring a build" and docs/guides/tracking-tq.md 4.6. Retire a
# build only after the NEXT cut-over has passed its proof: until then the previous build is the
# rollback window. This tool lands nothing and edits no tracked file; it prints the edits
# (build-compat steps 1 to 3) for the maintainer to make and land through tools/land.sh.
#
# It deletes the local copies, and only these five, by their exact names:
#   <prefix>/pfx/drive_c/CCP/EVE/tq-<old>           the patched copy
#   <prefix>/pfx/drive_c/CCP/EVE/tq-<old>-control   the census control copy
#   out/pyc-<old>, out/content-<old>, out/new-build/<old>
#
# Refuses (exit 2), before it changes anything, when:
#   - OLD_BUILD is not a number, or is not a row of tools/builds.tsv;
#   - it is the `pinned` build, or the build in tq-offline;
#   - it is the rollback-window build: no registered build lies strictly between it and the
#     pinned one, so it is the previous build, and the next cut-over has not passed its proof
#     past it (a build newer than it, below the pinned one, must exist first);
#   - the prefix, or any path it would delete, is the online prefix (compatdata/8500) or inside
#     it, as written or with its symlinks resolved.
# A symlink among the five is removed as a link; what it points at is never touched.
#
# Exit: 0 done (or a clean dry run), 2 refused or usage, 1 a step failed.
# EVE_RETIRE_REPO points at another checkout (the tests).
set -euo pipefail

REPO=${EVE_RETIRE_REPO:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

die() { echo "retire-build: $*" >&2; exit 2; }
usage() { sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "${BASH_SOURCE[0]}"; }

OLD= PREFIX= DRY=0
while [[ $# -gt 0 ]]; do
    case $1 in
        --prefix) PREFIX=${2:?--prefix needs a directory}; shift ;;
        --dry-run) DRY=1 ;;
        -h|--help) usage; exit 0 ;;
        -*) usage >&2; exit 2 ;;
        *) [[ -z $OLD ]] || die "one OLD_BUILD only"; OLD=$1 ;;
    esac
    shift
done
[[ -n $OLD ]] || { usage >&2; exit 2; }
[[ $OLD =~ ^[0-9]+$ ]] || die "'$OLD' is not a build number"

if [[ -z $PREFIX ]]; then PREFIX=$(eve_offline_prefix "$HOME"); fi
EVE=$PREFIX/pfx/drive_c/CCP/EVE
eve_is_online_path "$PREFIX" && die "$PREFIX is the online prefix; refused"

REG=$REPO/tools/builds.tsv
[[ -f $REG ]] || die "no $REG"
row=$(awk -F'\t' -v b="$OLD" '!/^#/ && $1 == b {print; exit}' "$REG")
[[ -n $row ]] || die "$OLD is not a row of tools/builds.tsv"
status=$(printf '%s' "$row" | cut -f3)
[[ $status != pinned ]] || die "$OLD is the pinned build; never retire it (re-pinning moves the whole RE corpus: build-compat.md)"
PINNED=$(awk -F'\t' '!/^#/ && $3 == "pinned" {print $1; exit}' "$REG")
[[ $PINNED =~ ^[0-9]+$ ]] || die "tools/builds.tsv has no pinned build"
BETWEEN=$(awk -F'\t' -v o="$OLD" -v p="$PINNED" '!/^#/ && $1 ~ /^[0-9]+$/ && $1 + 0 > o + 0 && $1 + 0 < p + 0 {n++} END {print n + 0}' "$REG")
(( BETWEEN > 0 )) || die "$OLD is the rollback window: no registered build lies between it and the pinned build $PINNED (build-compat.md: retire only after the next cut-over has passed its proof)"

if [[ -f $EVE/tq-offline/start.ini ]]; then
    LIVE=$(sed -n 's/^build *= *//p' "$EVE/tq-offline/start.ini" | tr -d '\r' | head -n 1)
    [[ $LIVE != "$OLD" ]] || die "tq-offline is build $OLD: it is the live install"
fi

TARGETS=("$EVE/tq-$OLD" "$EVE/tq-$OLD-control" "$REPO/out/pyc-$OLD" "$REPO/out/content-$OLD" "$REPO/out/new-build/$OLD")
for t in "${TARGETS[@]}"; do
    eve_is_online_path "$t" && die "$t is the online prefix or inside it; refused"
done

echo "retire-build: $OLD (status $status; pinned is $PINNED)   prefix $PREFIX"
(( DRY )) && echo "retire-build: dry run, nothing is deleted"

cat <<TXT

Edits to make by hand, then land them (build-compat.md "Retiring a build"; this tool changes no tracked file):
  1. delete the row for $OLD in tools/builds.tsv and its entry in eve_marshal::build::BUILDS
     (server/crates/eve-marshal/src/build.rs) together; the table test checks both
  2. git rm tools/patches/$OLD.json
  3. keep findings/build-delta-*-$OLD.tsv: history; the gate skips a delta whose install is gone
TXT

echo
failed=0
for t in "${TARGETS[@]}"; do
    if [[ ! -e $t && ! -L $t ]]; then echo "== $t: not there, skipped"; continue; fi
    echo "== delete $t"
    printf '   + rm -rf --'; printf ' %q' "$t"; printf '\n'
    if (( ! DRY )); then
        rm -rf -- "$t" || { echo "retire-build: could not delete $t" >&2; failed=1; }
    fi
done
(( failed == 0 )) || exit 1
echo "retire-build: done$( (( DRY )) && echo ' (dry run)')"
