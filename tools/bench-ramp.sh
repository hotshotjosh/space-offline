#!/usr/bin/env bash
# Ramp bots into ONE system and find where the server bends (docs/guides/bench-ramp.md).
#
#   tools/bench-ramp.sh                                  # 50 -> 300, +50 every 75 s, one grid
#   tools/bench-ramp.sh --start 10 --add 10 --max 30 --hold 60      # the small proof
#   tools/bench-ramp.sh --grid spread --mix miners=100
#   tools/bench-ramp.sh --stagger-ms 0                   # a login burst per step
#   tools/bench-ramp.sh --max 1000 --login-rate 2        # the login gate (#252), see below
#
# Options: --start N0, --add K, --max NMAX (bots), --hold SECS per step
# (default 75; one tick-health window is 10 s), --stagger-ms MS (gap between
# the logins of one batch, default 50 = 20 a second; 0: all at once),
# --grid same|spread (same: one belt, one fleet; spread: miners in fives, a
# belt each), --mix miners=100 (weights), --system NAME
# (default Amamake), --systems K (with --grid spread: the miners' fleets of five
# are seated round-robin in the K nearest low/null systems with a station, by
# jumps from --system; default 1), --brawl (eve-bots --brawl: fighters close on
# the nearest foe; a fight is --mix fighters=1,blues=1 on --grid same),
# --stats SECS (eve-bots [stats] lines, default 10, 0 off), --quiet (eve-bots
# --quiet: no act lines, so act_* read 0), --final-hold SECS (hold the last
# step this long, default --hold), --port N (default 26500; chat is N+1), --run NAME
# (default a timestamp; output in var/bench/NAME/), --no-build, --prep.
#
# --login-rate R (#252): measure the bots' login gate instead of a ramp. All --max
# bots start at once at Human pace with `eve-bots --login-rate R` (no --ramp);
# --start/--add/--stagger-ms are ignored. Writes logins.tsv (logins per second),
# login-windows.tsv (the tick p95 of each 10 s window, with its logins) and a
# "login gate" block in summary.txt; more than 5R+1 logins in any 5 s is flagged. The
# hold defaults to --max/R + 40 s (it must be at least that); not with --prep.
#
# --prep: run once per scenario size BEFORE the ramps. All NMAX bots log in
# at once on var/bench/base itself and buy their kits (dev market) and fit
# them, then it stops (at most 10 minutes); later runs copy that world, so
# their bots start working at once instead of shopping for their first
# minutes. Without it a ramp still works, but the early steps measure the
# shopping, not the work.
#
# Its own server and world: release binaries, `--dev` world copied from
# var/bench/base (the bots' accounts are made there once, then reused), a
# port that is not 26000, no client, no --bubble. Never touches var/dev.
# Bot passwords: ~/.config/eve-offline/bench-bots.tsv (mode 600).
# Output: var/bench/NAME/{steps.tsv,summary.txt,server.log,bots.log,roster.tsv}.
set -euo pipefail
# With --chat on, a bot holds about 2 sockets (game + chat), so 1,000 bots are ~2,000 in the
# bots process (and ~2,000 accepted in the server): lift the soft open-files limit to the hard one.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SRV=${EVE_SERVER_BIN:-eve-offline-server}   # the server binary; EVE_SERVER_BIN=eve-server for the plain core one
# A lane worktree has no out/; the main checkout (the git common dir's parent) does.
MAIN=$(cd "$(git -C "$REPO" rev-parse --git-common-dir)/.." && pwd)
OUT_ROOT=${EVE_BENCH_OUT:-$MAIN/out}
VAR=${EVE_BENCH_VAR:-$REPO/var/bench}
TOKENS=${EVE_BENCH_TOKENS:-$HOME/.config/eve-offline/bench-bots.tsv}
PORT=26500 BUILD=1 PREP=0 RUN=$(date +%Y%m%d-%H%M%S)
PY=(--start 50 --add 50 --max 300 --hold 75 --stagger-ms 50 --grid same --mix miners=100 --system Amamake --systems 1 --final-hold 0 --stats 10)
RATE=0 HOLD_SET=0
FLAGS=()   # words without a value: --brawl, --quiet
set_py() { local i; for ((i = 0; i < ${#PY[@]}; i += 2)); do [[ ${PY[i]} == "$1" ]] && { PY[i+1]=$2; return; }; done; }
while [[ $# -gt 0 ]]; do
    case "$1" in
        --login-rate) [[ $# -ge 2 ]] || { echo "bench-ramp.sh: $1 needs a value" >&2; exit 2; }; RATE=$2; shift 2 ;;
        --hold) [[ $# -ge 2 ]] || { echo "bench-ramp.sh: $1 needs a value" >&2; exit 2; }; HOLD_SET=1; set_py "$1" "$2"; shift 2 ;;
        --start|--add|--max|--stagger-ms|--grid|--mix|--system|--systems|--final-hold|--stats) [[ $# -ge 2 ]] || { echo "bench-ramp.sh: $1 needs a value" >&2; exit 2; }; set_py "$1" "$2"; shift 2 ;;
        --port) PORT=${2:?}; shift 2 ;;
        --run) RUN=${2:?}; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --prep) PREP=1; shift ;;
        --brawl|--quiet) FLAGS+=("$1"); shift ;;
        -h|--help) sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        *) echo "bench-ramp.sh: unknown argument $1 (see --help)" >&2; exit 2 ;;
    esac
done
[[ $RATE =~ ^[0-9]*\.?[0-9]+$ ]] || { echo "bench-ramp.sh: --login-rate: a number of logins a second" >&2; exit 2; }
if awk -v r="$RATE" 'BEGIN{exit !(r+0>0)}'; then
    if ((HOLD_SET == 0)); then   # every bot through the gate, plus the 10 s jitter and 30 s of work
        set_py --hold "$(awk -v n="$(printf '%s\n' "${PY[@]}" | awk '$0=="--max"{getline; print; exit}')" -v r="$RATE" 'BEGIN{printf "%d", n / r + 40}')"
    fi
    HOLDV=$(printf '%s\n' "${PY[@]}" | awk '$0=="--hold"{getline; print; exit}')
    MAXV=$(printf '%s\n' "${PY[@]}" | awk '$0=="--max"{getline; print; exit}')
    NEED=$(awk -v n="$MAXV" -v r="$RATE" 'BEGIN{printf "%d", int(n / r) + 40}')
    [[ $HOLDV =~ ^[0-9]+$ && $MAXV =~ ^[0-9]+$ ]] && ((HOLDV >= NEED)) || { echo "bench-ramp.sh: --login-rate $RATE with $MAXV bots needs --hold >= $NEED" >&2; exit 2; }
    ((PREP == 0)) || { echo "bench-ramp.sh: --login-rate and --prep do not go together" >&2; exit 2; }
    FLAGS+=(--login-rate "$RATE")
fi
MAXBOTS=$(printf '%s\n' "${PY[@]}" | awk '$0=="--max"{getline; print; exit}')
HARD=$(ulimit -Hn)
if [[ $HARD =~ ^[0-9]+$ && $MAXBOTS =~ ^[0-9]+$ ]] && ((HARD * 2 < MAXBOTS * 5)); then
    echo "bench-ramp.sh: WARNING: open-files hard limit $HARD is below 2.5 x --max ($MAXBOTS): about 2 sockets a bot (game + chat), logins will fail past ~$((HARD / 2)) bots; raise it (limits.conf / systemd LimitNOFILE)" >&2
fi
[[ $PORT =~ ^[0-9]+$ ]] && ((PORT != 26000 && PORT != 26001 && PORT != 26090)) || { echo "bench-ramp.sh: --port must be a number, not 26000/26001/26090" >&2; exit 2; }
[[ $RUN =~ ^[A-Za-z0-9._-]+$ ]] || { echo "bench-ramp.sh: --run: letters, digits, . _ -" >&2; exit 2; }
[[ -f $OUT_ROOT/content/MANIFEST.tsv ]] || { echo "bench-ramp.sh: no $OUT_ROOT/content (tools/serve.sh makes it)" >&2; exit 1; }
if ((BUILD)); then
    "$REPO/tools/dev.sh" cargo build --locked --release -p "$SRV" --bin "$SRV" -p eve-bot --bin eve-bots
fi
BIN=$REPO/server/target/release
[[ -x $BIN/$SRV && -x $BIN/eve-bots ]] || { echo "bench-ramp.sh: no release binaries in $BIN" >&2; exit 1; }
DATA=(--content "$OUT_ROOT/content" --authored "$REPO/content")
RUNDIR=$VAR/$RUN
[[ ! -e $RUNDIR ]] || { echo "bench-ramp.sh: $RUNDIR exists; pick another --run" >&2; exit 2; }
mkdir -p "$VAR/base" "$RUNDIR"
exec 9>"$VAR/bench.lock"
flock -n 9 || { echo "bench-ramp.sh: another bench-ramp holds $VAR/bench.lock" >&2; exit 1; }

# The bots' names for this scenario (the same roster the runner writes).
mapfile -t NAMES < <(python3 -I - "$REPO/tools" "${PY[@]}" <<'P'
import sys
sys.path.append(sys.argv[1])
import bench_ramp as b
a = dict(zip(sys.argv[2::2], sys.argv[3::2]))
_, names = b.roster(int(a["--max"]), b.parse_mix(a["--mix"]), a["--grid"])
print("\n".join(names))
P
)
echo "== bot accounts (${#NAMES[@]}; made once in $VAR/base, then reused)"
bash "$REPO/tools/bot-accounts.sh" --bin "$BIN" --world "$VAR/base/world.dat" --tokens "$TOKENS" \
    --logs "$VAR/base" --port 26590 --wait 1800 "${NAMES[@]}" -- --dev "${DATA[@]}" >"$VAR/base/accounts.log" 2>&1 \
    || { tail -n 20 "$VAR/base/accounts.log" >&2; echo "bench-ramp.sh: bot accounts failed" >&2; exit 1; }

if ((PREP)); then
    echo "== prep: all ${#NAMES[@]} bots on var/bench/base at once, until they are out of the station (max 10 min)"
    exec python3 -I "$REPO/tools/bench_ramp.py" --prep --bin "$BIN" --out "$RUNDIR" --world "$VAR/base/world.dat" \
        --tokens "$TOKENS" --content "$OUT_ROOT/content" --authored "$REPO/content" --port "$PORT" \
        --start "${#NAMES[@]}" --max "${#NAMES[@]}" --hold 600 --stagger-ms "$(printf '%s\n' "${PY[@]}" | awk '$0=="--stagger-ms"{getline; print; exit}')" \
        --grid "$(printf '%s\n' "${PY[@]}" | awk '$0=="--grid"{getline; print; exit}')" \
        --mix "$(printf '%s\n' "${PY[@]}" | awk '$0=="--mix"{getline; print; exit}')" \
        --system "$(printf '%s\n' "${PY[@]}" | awk '$0=="--system"{getline; print; exit}')" \
        --systems "$(printf '%s\n' "${PY[@]}" | awk '$0=="--systems"{getline; print; exit}')"
fi
# This run's world: a copy of the base (accounts only), so every run starts alike.
for f in "$VAR"/base/world.dat*; do
    case $f in *.lock|*.stop) continue ;; esac
    cp -r -- "$f" "$RUNDIR/$(basename "$f")"
done
echo "== ramp: ${PY[*]} on port $PORT, output $RUNDIR"
exec python3 -I "$REPO/tools/bench_ramp.py" --bin "$BIN" --out "$RUNDIR" --world "$RUNDIR/world.dat" \
    --tokens "$TOKENS" --content "$OUT_ROOT/content" --authored "$REPO/content" --port "$PORT" "${PY[@]}" ${FLAGS[@]+"${FLAGS[@]}"}
