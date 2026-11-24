#!/usr/bin/env bash
# Soak the server with bots and no client: eve-server plus N bots for a
# fixed time, then judge the run: panics, the server's exit, memory, the
# world-turn hold, and the calls nobody answered.
#
#   tools/soak.sh                       # 40 miners + 10 escorts, 20 minutes
#   tools/soak.sh --miners 5 --escorts 2 --secs 120
#   tools/soak.sh --release             # judge the build players run
#   tools/soak.sh --rss-budget 1        # a synthetic threshold: must fail
#   tools/soak.sh --no-build            # use the binaries as they are
#   tools/soak.sh --catch-all           # the pre-ACC2 catch-all server
#   tools/soak.sh --builders 5 --ratters 5   # add economy roles (E1)
#   tools/soak.sh --closed --ratters 5       # the closed economy (E8, E10b)
#   tools/soak.sh --closed --society --secs 300 --miners 10 --escorts 5
#   tools/soak.sh --closed --society --time-scale 24 --secs 3600   # a game day an hour
#   tools/soak.sh --hunters 10 --miners 5 --escorts 0 --secs 600  # a PvP roster (BP-SOAK)
#   tools/soak.sh --society --society-pvp --secs 600               # the pirate career, no roster
#   tools/soak.sh --dictors 2 --hics 2 --hunters 4 --roster-home "A LowSec System" --secs 900
#                                                                  # a PvP roster with bubblers (#136)
#
# Options: --miners N, --escorts N, --builders N (--loop builder, named
# BuilderN), --ratters N (--loop sites, named RatterN), --secs N, --release (default debug, so
# the index debug_asserts keep firing unattended), --rss-budget MB (0 = no
# budget), --rss-growth MB, --hold-ms MS, --no-build, --no-chat,
# --catch-all (--world-accounts, the default, is still accepted), --closed,
# --society, --day-secs N, --time-scale N, --difficulty LEVEL, --society-wars,
# --hunters N, --society-pvp, --dictors N, --hics N, --roster-home SYSTEM, --foes N,
# --kit-drill smartbomb|mjd, --hunter-status VALUE.
#
# **PvP (BP-SOAK, #12).** --hunters N adds N bots named HunterN that play
# `--loop hunter` (with --society the minds pick their roles instead), and
# implies --society-pvp. --society-pvp alone passes `--society-pvp` to eve-bots
# (the opt-in that #90 waits on): with --society the pirate career plays the
# hunts, with no roster. Both need the content bundle for the systems' security
# and the hulls' classes (out/content; the soak notes when it is missing).
# Before the verdict line the soak prints tools/pvp-report.py's one PASS/FAIL
# line per behaviour, reported, never judged. --hunters alone cannot make the
# probes, the field or the bubble: hunters fly the escort's hulls, and only a
# bot flying an interdictor or a heavy interdictor takes the Interdict role
# (driver.rs `sync_doctrine`, `Ground::is_bubbler`).
#
# **Bubblers (#136).** --dictors N (a Sabre each) and --hics N (a Devoter each)
# add bots named DictorN and HicN that play `--loop hunter` in those hulls, and
# --hunters N still adds HunterN in Rifters; --foes N (default 4) adds FoeN, Rifters
# on the other side to fight. This is the DEV-MODE roster (docs/guides/dev-mode.md):
# the server runs `--dev` (a dev seller of every item at 100 ISK, a billion ISK
# per character) and eve-bots gets `--roster FILE --roster-home SYSTEM`; any of
# --dictors/--hics/--foes selects this roster. Each
# bot BUYS its hull, modules and rounds the closed economy's way; what is
# GRANTED (GM /createitem, camp.rs gm_lines) is the camper's own part: the
# dictor's launcher, probes and mobile disruptor, the HIC's field generator,
# and the tackle (scrambler, webifier) of the pvp seats. SYSTEM (a
# null-sec or wormhole system name or id) is required, as is the content bundle,
# and it is checked against out/content/universe/systems.tsv (security <= 0): bubbles
# need zero-sec or wormhole ground (hunt.rs `Ground::allows_bubbles`), and a low-sec
# home made dictors that never camp (2026-11-04 soak), so a low-sec home is refused
# (the check is skipped with a note when the bundle's systems.tsv is missing),
# and the roster replaces --miners/--escorts/--builders/--ratters (all set to 0)
# and cannot go with --society or --catch-all. [A] a bot flying a Sabre or a
# Devoter takes the Interdict role by its hull (`sync_doctrine`); falsified by
# a run whose DictorN log no `kit` act line and no `[camp]` line.
# --kit-drill smartbomb|mjd sets EVE_BOTS_KIT_DRILL for eve-bots inside the
# container (dev.sh passes no environment): smartbomb goes with bubblers (the
# foes board a Tempest), mjd puts both sides in Hurricanes and is refused with
# --dictors/--hics. --hunter-status VALUE passes `--dev-status VALUE` to eve-bots
# (#29): each seat of the Hunter group (not Dictor/Hic/Foe) sends `/secstatus VALUE` once at its dev setup, to make
# law-aware roaming matter. bots-threads.tsv (seconds, thread name, wchan,
# state; one row per thread per sample) shows where a stalled bot's thread waits.
# [A] a bot's thread is named after the bot (driver.rs); falsified by a
# bots-threads.tsv whose thread names are all the process name.
# [A] a hunter of a world-accounts soak gets its hull and skills like any
# role; falsified by a soak whose hunters all say MISSING. A gang roster
# (roamgang, a group camping a gate) needs the dev roster (`eve-bots --roster`
# against `eve-server --dev`), not this soak.
#
# **Wars (R-REL-5).** --society-wars (needs --society) passes `--society-wars`
# to eve-bots: bot alliances declare wars and mutual-war invites on the groups
# their relation marks as enemies, and their members hunt war targets in high
# sec while it lasts (a group of humans needs --hostile, not given here).
#
# **Difficulty (DOC-8).** --difficulty easy|normal|hard|brutal|0..4 is passed
# through to eve-bots (default: its own, normal). It scales the bots'
# behaviour only (content/bots/difficulty.tsv); the server is the same.
#
# **Compressed time (lane clock).** --time-scale N (1..60) passes
# `--time-scale N` to the server, whose slow game timers (industry, PI,
# skills, the daily rollover, respawns, order and contract expiry) then run
# N times faster while physics stays real time (docs/lanes/clock.md). With
# --society and no --day-secs, the bots' day is 86400/N seconds, so their
# session timetable runs at the server's pace: a 24-hour economic day in
# 24/N hours.
#
# **Society mode (SOC-A2).** --society runs eve-bots with --society: the
# counts only name the bots (MinerN, EscortN, ...), and each bot's mind picks
# its role; no --loop is passed. The minds persist in server/target/soak/state
# (--state; wiped with the rest at the start). By default every bot stays
# logged on (--sessions always), so a short soak loads the server as before;
# --day-secs N passes daily sessions with a day of N seconds, so bots log off
# and on inside the soak. bots.log carries the [soc] lines.
#
# **The closed economy (E8, E10b, R-ECON gate G3).** --closed runs eve-bots
# with --closed: no GM setup at all, every bot gets its kit the player's way
# (eve_bot::acquire). It passes no --bootstrap, because day 0 needs none:
# a capsule bot boards a rookie ship from CreateNewbieShip, buys its skill
# book from its school station's NPC ask with the starting wallet, mines
# with the civilian miner or fights with the civilian gun, and sells its
# loot into the NPC loot bids. [A] that the soak's characters start at a
# school station with a wallet; falsified by a closed soak whose bots all
# say MISSING for a skillbook (then pass --bootstrap by hand: it is the
# only GM setup a closed bot accepts).
#
# **Accounts.** By default (BOTMARK) the soak server runs the way
# tools/up.sh and tools/serve.sh do: tools/bot-accounts.sh first signs every
# bot up on a one-shot server (127.0.0.1:26090, ceiling player, sign-up on),
# grants it gm and marks it a bot offline, and the soak server then runs
# --world-accounts with the bots' passwords in a tokens file in the
# container's /tmp. The bot mark makes every bot's character a bot, so the
# status file counts them as bots, not humans. --catch-all runs the server
# as before ACC2 (loopback, every bot the GM, the roster from --character
# with the `,bot` mark). Both passed 5 miners + 2 escorts for 180 s with the
# same RSS and hold (docs/lanes/botmark.md), so the thresholds below, which
# were measured on catch-all, stand for both.
#
# Runs inside tools/dev.sh's container: no network (loopback only), the repo
# read-only, and nothing that needs root. That is why it works unattended,
# where up.sh and client-ci.sh need `sudo -v`. The content bundle in
# out/content (or the loose extracts) and the authored tables in content/
# are passed when present. Logs, rss.tsv and the world file go to
# server/target/soak/, which is wiped at the start of each run.
#
# **Measurement (E1, R-ECON §1.1).** Besides rss.tsv (the server), the
# soak writes bots-rss.tsv: the eve-bots process sampled every SAMPLE_SECS
# (seconds, VmRSS kB, threads, CPU seconds so far), and prints its peak, per
# bot, and its mean CPU. It also sums the server's `[save]` lines (bytes,
# render under the lock and write off it apart, µs per KB) and the `labels=` field of `[turns]`
# (each watched call's hold max over the run). These are reported, never
# judged: the gates that use them are R-ECON §3's G0-G4.
#
# **Summary (S10).** tools/soak_summary.py (unit-tested in
# tools/test_soak_summary.py) reads the logs and prints the tick work and
# lock-held p95/max (over the server's `tick health` windows), lateness,
# UserErrors by key, bot "homing Recovery" docks and FAILED/refused acts by
# kind, and the log deny-list (panic, UNANSWERED, ERROR/error:, TAINTED); it
# also writes summary.tsv (metric, key, value). Once the bots stop, the
# server's last `[live]` line after the log-off hold is saved to offline.txt:
# pilots over 0 means a bot was left online. --allow-unanswered N tolerates N UNANSWERED lines
# a log (the real dev world logs a few; default 0).
#
# The verdict fails on (each prints one `soak: FAIL` line naming it):
#   - a panic in either log, or a bot run that exited non-zero;
#   - the server dying before the bots finished, or not exiting 0 on its
#     stop file (lane shutdown: it is stopped cleanly, through
#     world.dat.stop, and SIGTERM after 60 s is itself a failure);
#   - peak VmRSS over --rss-budget;
#   - VmRSS growth over --rss-growth after the last bot login (the sample
#     where every bot's character has entered the game);
#   - a `[turns]` line whose hold_max is over --hold-ms (tools/doctrine-report.py G10
#     is the p99.9 rule; the server runs with --turn-sched, and env.tsv holds the
#     cgroup throttle / memory pressure / majflt samples, #248);
#   - a deny-list hit or a pilot still online after the bots stopped (S10).
# The `[turns]` contract (CP15/CA13): one line per window starting `[turns]`
# with a `hold_max=<ms>` field, e.g. `[turns] ... hold_max=12.5ms ...`.
# Until that line exists its absence is a note, not a failure
# (TURNS_REQUIRED).
#
# Exit status: 0 = pass; 1 = a verdict failure; 2 = usage.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The knobs. Each [A] is a guess with its falsifier beside it; tune here,
# never in a Rust test (owner rule: no [A] in a test).
# [A] V4 P11: about 1.46 GB at 50 bots after §67, plus 50%. Falsified by a
#     clean full-size release soak peaking above it (V3 §3 measured 640 MB).
RSS_BUDGET_MB=2200
# [A] V3 R4: 100 MB growth after the last login. V3 §3 saw +6 MB in 4 min
#     with 50 bots holding (ore stacks). Falsified by a long clean soak that
#     grows past it with no leak behind it.
RSS_GROWTH_MB=100
# [A] V3 R4: a world-turn hold over 100 ms in release. Debug held 157-196 ms
#     for the same turns (V3 §2.1), so debug gets HOLD_MS_DEBUG [A], about
#     twice that. Falsified by a release soak with no slow turn over it.
HOLD_MS=100 HOLD_MS_DEBUG=400
# 1 since CP15 prints `[turns]` every report window.
TURNS_REQUIRED=1
SAMPLE_SECS=10

MINERS=40 ESCORTS=10 BUILDERS=0 RATTERS=0 SECS=1200 BUILD=1 CHAT=1 PROFILE=debug HOLD_SET="" ACCOUNTS=1 CLOSED=0
ALLOW_UNANSWERED=""
SOCIETY=0 DAY_SECS="" TIME_SCALE="" DIFFICULTY="" SOCIETY_WARS=0
HUNTERS=0 SOCIETY_PVP=0 DICTORS=0 HICS=0 FOES=4 FOES_SET=0 ROSTER_HOME=""
KIT_DRILL="" HUNTER_STATUS=""
while [[ $# -gt 0 ]]; do
    case "$1" in
        --miners)  MINERS="$2"; shift 2 ;;
        --escorts) ESCORTS="$2"; shift 2 ;;
        --builders) BUILDERS="$2"; shift 2 ;;
        --ratters) RATTERS="$2"; shift 2 ;;
        --secs)    SECS="$2"; shift 2 ;;
        --release) PROFILE=release; shift ;;
        --rss-budget) RSS_BUDGET_MB="$2"; shift 2 ;;
        --rss-growth) RSS_GROWTH_MB="$2"; shift 2 ;;
        --hold-ms) HOLD_SET="$2"; shift 2 ;;
        --no-build) BUILD=0; shift ;;
        --no-chat) CHAT=0; shift ;;
        --world-accounts) ACCOUNTS=1; shift ;;
        --catch-all) ACCOUNTS=0; shift ;;
        --closed) CLOSED=1; shift ;;
        --society) SOCIETY=1; shift ;;
        --day-secs) DAY_SECS="$2"; shift 2 ;;
        --time-scale) TIME_SCALE="$2"; shift 2 ;;
        --allow-unanswered) ALLOW_UNANSWERED="$2"; shift 2 ;;
        --difficulty) DIFFICULTY="$2"; shift 2 ;;
        --society-wars) SOCIETY_WARS=1; shift ;;
        --hunters) HUNTERS="$2"; shift 2 ;;
        --society-pvp) SOCIETY_PVP=1; shift ;;
        --dictors) DICTORS="$2"; shift 2 ;;
        --hics) HICS="$2"; shift 2 ;;
        --foes) FOES="$2"; FOES_SET=1; shift 2 ;;
        --roster-home) ROSTER_HOME="$2"; shift 2 ;;
        --kit-drill) KIT_DRILL="$2"; shift 2 ;;
        --hunter-status) HUNTER_STATUS="$2"; shift 2 ;;
        -h|--help) sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p}' "$0"; exit 0 ;;
        *) echo "soak.sh: unknown argument $1" >&2; exit 2 ;;
    esac
done
if [[ -n $DIFFICULTY ]] && ! [[ $DIFFICULTY =~ ^(easy|normal|hard|brutal|[0-4])$ ]]; then echo "soak.sh: --difficulty wants easy, normal, hard, brutal or 0..4" >&2; exit 2; fi
if ((SOCIETY_WARS)) && ((!SOCIETY)); then echo "soak.sh: --society-wars needs --society" >&2; exit 2; fi
if [[ -n $DAY_SECS ]] && ((!SOCIETY)); then echo "soak.sh: --day-secs needs --society" >&2; exit 2; fi
for n in "$MINERS" "$ESCORTS" "$BUILDERS" "$RATTERS" "$HUNTERS" "$DICTORS" "$HICS" "$FOES" "$SECS" "$RSS_BUDGET_MB" "$RSS_GROWTH_MB" ${HOLD_SET:+"$HOLD_SET"} ${DAY_SECS:+"$DAY_SECS"} ${TIME_SCALE:+"$TIME_SCALE"}; do
    [[ "$n" =~ ^[0-9]+$ ]] || { echo "soak.sh: not a whole number: $n" >&2; exit 2; }
done
if [[ -n $TIME_SCALE ]]; then
    ((TIME_SCALE >= 1 && TIME_SCALE <= 60)) || { echo "soak.sh: --time-scale must be 1..60" >&2; exit 2; }
    # The bots' day at the server's pace, unless --day-secs says otherwise.
    if ((SOCIETY)) && [[ -z $DAY_SECS ]]; then DAY_SECS=$((86400 / TIME_SCALE)); fi
fi
if [[ -n "$HOLD_SET" ]]; then HOLD_LIMIT=$HOLD_SET
elif [[ $PROFILE == release ]]; then HOLD_LIMIT=$HOLD_MS
else HOLD_LIMIT=$HOLD_MS_DEBUG; fi
((HUNTERS > 0)) && SOCIETY_PVP=1
# #136: the dev roster with bubblers replaces the role counts.
DEVROSTER=0
if [[ -n $KIT_DRILL ]] && ! [[ $KIT_DRILL =~ ^(smartbomb|mjd)$ ]]; then echo "soak.sh: --kit-drill wants smartbomb or mjd" >&2; exit 2; fi
if [[ -n $HUNTER_STATUS ]] && ! [[ $HUNTER_STATUS =~ ^-?[0-9]+(\.[0-9]+)?$ ]]; then echo "soak.sh: --hunter-status wants a number such as -3" >&2; exit 2; fi
if [[ -n $HUNTER_STATUS ]] && ! awk -v s="$HUNTER_STATUS" 'BEGIN{exit !(s + 0 >= -10 && s + 0 <= 10)}'; then echo "soak.sh: --hunter-status must be from -10 to 10" >&2; exit 2; fi
if [[ $KIT_DRILL == mjd ]] && ((DICTORS + HICS > 0)); then
    echo "soak.sh: --kit-drill mjd puts both sides in Hurricanes (pvp_kit.rs drill_hull); it cannot go with --dictors/--hics" >&2; exit 2
fi
if ((DICTORS + HICS > 0 || FOES_SET)); then
    DEVROSTER=1 SOCIETY_PVP=1
    ((!SOCIETY)) || { echo "soak.sh: --dictors/--hics cannot go with --society" >&2; exit 2; }
    ((ACCOUNTS)) || { echo "soak.sh: --dictors/--hics need world accounts, not --catch-all" >&2; exit 2; }
    [[ -n $ROSTER_HOME ]] || { echo "soak.sh: --dictors/--hics/--foes need --roster-home SYSTEM (a null-sec or wormhole system name or id)" >&2; exit 2; }
    [[ -f "$REPO/out/content/MANIFEST.tsv" ]] || { echo "soak.sh: --dictors/--hics/--foes need out/content (tools/up.sh makes it)" >&2; exit 2; }
    # A3: bubbles need zero-sec or wormhole ground (hunt.rs Ground::allows_bubbles), so
    # a low-sec home makes bubblers that never camp (checked only with --dictors/--hics;
    # a hunters-and-foes roster just needs the system to exist: the #29 run wants low-sec). Name col 18, id col 1, security col 6.
    systems="$REPO/out/content/universe/systems.tsv"
    if [[ -f $systems ]]; then
        # Names compare case-insensitively.
        homesec=$(awk -F'\t' -v h="$ROSTER_HOME" 'BEGIN{h = tolower(h)} !/^#/ && (tolower($18) == h || $1 == h) {print $6; exit}' "$systems")
        if [[ -z $homesec ]]; then
            echo "soak.sh: --roster-home $ROSTER_HOME is not in $systems" >&2; exit 2
        elif ((DICTORS + HICS > 0)) && ! awk -v s="$homesec" 'BEGIN{exit !(s + 0 <= 0.0)}'; then
            echo "soak.sh: --roster-home $ROSTER_HOME has security $homesec; bubbles need a null-sec or wormhole home (hunt.rs Ground::allows_bubbles)" >&2; exit 2
        fi
    else
        echo "soak: note: no $systems: --roster-home security not checked (it must be null-sec or wormhole)" >&2
    fi
    MINERS=0 ESCORTS=0 BUILDERS=0 RATTERS=0
    BOTS=$((HUNTERS + DICTORS + HICS + FOES))
else
    # #136: these only shape the dev roster; refuse them rather than ignore them.
    [[ -z $ROSTER_HOME ]] || { echo "soak.sh: --roster-home only goes with --dictors/--hics/--foes" >&2; exit 2; }
    [[ -z $KIT_DRILL ]] || { echo "soak.sh: --kit-drill needs the dev roster (--dictors/--hics/--foes)" >&2; exit 2; }
    [[ -z $HUNTER_STATUS ]] || { echo "soak.sh: --hunter-status needs the dev roster (--dictors/--hics/--foes)" >&2; exit 2; }
    BOTS=$((MINERS + ESCORTS + BUILDERS + RATTERS + HUNTERS))
fi

if ((BUILD)); then
    build=(cargo build --locked --bins)
    [[ $PROFILE == release ]] && build+=(--release)
    "$REPO/tools/dev.sh" "${build[@]}"
fi

# The content bundle when there is one (tools/up.sh makes it), else the
# loose extracts, as /repo paths inside the container, only if they exist.
data=()
if [[ -f "$REPO/out/content/MANIFEST.tsv" ]]; then
    data+=(--content /repo/out/content)
else
  for pair in celestials:celestials-all.tsv belt-rocks:belt-rocks-all.tsv \
            planets:pi-statics.tsv typedata:typedata.tsv skills:skills.tsv; do
    flag="${pair%%:*}" file="${pair#*:}"
    [[ -f "$REPO/out/$file" ]] && data+=("--$flag" "/repo/out/$file")
  done
fi
# Our own tables, as the server is run for players (plan CA2).
[[ -d "$REPO/content" ]] && data+=(--authored /repo/content)

roster=() names=()
for ((i = 1; i <= MINERS; i++)); do roster+=(--character "Miner$i,bot"); names+=("Miner$i"); done
for ((i = 1; i <= ESCORTS; i++)); do roster+=(--character "Escort$i,bot"); names+=("Escort$i"); done
for ((i = 1; i <= BUILDERS; i++)); do roster+=(--character "Builder$i,bot"); names+=("Builder$i"); done
for ((i = 1; i <= RATTERS; i++)); do roster+=(--character "Ratter$i,bot"); names+=("Ratter$i"); done
for ((i = 1; i <= HUNTERS; i++)); do roster+=(--character "Hunter$i,bot"); names+=("Hunter$i"); done
# #136: the dev roster's names are <group>1..N, as eve_bot::dev expands them.
if ((DEVROSTER)); then
    roster=() names=()
    for pair in Hunter:$HUNTERS Dictor:$DICTORS Hic:$HICS Foe:$FOES; do
        for ((i = 1; i <= ${pair#*:}; i++)); do names+=("${pair%%:*}$i"); done
    done
    data+=(--dev)
fi
# World accounts: no roster; the names go to bot-accounts.sh instead.
((ACCOUNTS)) && roster=(--world-accounts)
((ACCOUNTS)) || names=()
chatflag=()
((CHAT)) && chatflag=(--chat)
# The server's clock (lane clock); rides with the chat flag into the
# server's arguments.
[[ -n $TIME_SCALE ]] && chatflag+=(--time-scale "$TIME_SCALE")
botloops=()
((MINERS > 0)) && botloops+=(--loop mine --prefix Miner --count "$MINERS")
((ESCORTS > 0)) && botloops+=(--loop escort --prefix Escort --count "$ESCORTS")
((BUILDERS > 0)) && botloops+=(--loop builder --prefix Builder --count "$BUILDERS")
# [A] a "ratter" is the site runner (R-ECON §1.2 lists them together); a
#     dedicated belt-ratter role would replace this. Falsified by an E-row
#     adding such a role to eve-bot's ROLES.
((RATTERS > 0)) && botloops+=(--loop sites --prefix Ratter --count "$RATTERS")
# BP-SOAK: the hunter role (eve-bots refuses it without --society-pvp).
((HUNTERS > 0)) && botloops+=(--loop hunter --prefix Hunter --count "$HUNTERS")
# #136: the dev roster (group side role count skills ship), bought hulls.
if ((DEVROSTER)); then
    botloops=(--roster /repo/server/target/soak/roster.tsv --roster-home "$ROSTER_HOME")
fi
# Society mode: the same names, no --loop (the minds pick the roles).
if ((SOCIETY)); then
    botloops=()
    for pair in Miner:$MINERS Escort:$ESCORTS Builder:$BUILDERS Ratter:$RATTERS Hunter:$HUNTERS; do
        if ((${pair#*:} > 0)); then botloops+=(--prefix "${pair%%:*}" --count "${pair#*:}"); fi
    done
    botloops+=(--society --state /repo/server/target/soak/state)
    if [[ -n $DAY_SECS ]]; then botloops+=(--sessions daily --day-secs "$DAY_SECS")
    else botloops+=(--sessions always); fi
fi
botchat=(--chat off)
((CHAT)) && botchat=(--chat on)
# The bots read the bundle's NPC profiles (CP46), when there is a bundle.
botdata=()
[[ -f "$REPO/out/content/MANIFEST.tsv" ]] && botdata=(--content /repo/out/content)
# The closed economy: no GM setup, no --bootstrap (see the header).
((CLOSED)) && botdata+=(--closed)
# DOC-8: the bots' difficulty, when asked for.
[[ -n $DIFFICULTY ]] && botdata+=(--difficulty "$DIFFICULTY")
# R-REL-5: bot wars, when asked for.
((SOCIETY_WARS)) && botdata+=(--society-wars)
# BP-SOAK: bots hunt pilots in low sec, null sec and wormhole space.
((SOCIETY_PVP)) && botdata+=(--society-pvp)
# C2 (#29): the hunters' security status, set once at their dev setup (/secstatus).
[[ -n $HUNTER_STATUS ]] && botdata+=(--dev-status "$HUNTER_STATUS")
if ((SOCIETY_PVP)) && [[ ! -f "$REPO/out/content/MANIFEST.tsv" ]]; then
    echo "soak: note: --society-pvp without out/content: the bots lack the systems' security and the hulls' classes" >&2
fi

out="$REPO/server/target/soak"
rm -rf "$out"
mkdir -p "$out"
if ((DEVROSTER)); then
    {
        echo "# written by tools/soak.sh (#136): bubblers and the bots they fight"
        ((HUNTERS > 0)) && printf 'Hunter\tred\thunter\t%s\trole\tRifter\n' "$HUNTERS"
        ((DICTORS > 0)) && printf 'Dictor\tred\thunter\t%s\trole\tSabre\n' "$DICTORS"
        ((HICS > 0)) && printf 'Hic\tred\thunter\t%s\trole\tDevoter\n' "$HICS"
        ((FOES > 0)) && printf 'Foe\tFOES\thunter\t%s\trole\tRifter\n' "$FOES"
        true
    } >"$out/roster.tsv"
fi

# The time limit is the soak plus build-free startup slack. The sampler
# lives in the container because that is where the server's pid is known.
# rss.tsv: seconds since start, VmRSS in kB, characters entered so far.
# bots-rss.tsv: seconds, the eve-bots VmRSS in kB, its threads, and its
# CPU seconds so far (utime + stime over CLK_TCK).
EVE_DEV_TIMEOUT=$((SECS + 600 + 900 * ACCOUNTS)) "$REPO/tools/dev.sh" bash -c '
    set -u
    out=/repo/server/target/soak
    bin=/repo/server/target/$1; shift
    secs=$1; every=$2; kit=$3; shift 3
    # A5: dev.sh passes no environment, so the drill is set here.
    [[ -n $kit ]] && export EVE_BOTS_KIT_DRILL=$kit
    nnames=$1; shift
    names=("${@:1:$nnames}"); shift "$nnames"
    nserver=$1; shift
    server=("${@:1:$nserver}"); shift "$nserver"
    tokens=()
    if ((nnames)); then
        # Accounts first, on their own one-shot server (the data args only).
        data=()
        skip=0
        for a in "${server[@]}"; do
            if ((skip)); then skip=0; continue; fi
            [[ $a == --time-scale ]] && { skip=1; continue; }
            [[ $a == --world-accounts || $a == --chat ]] || data+=("$a")
        done
        bash /repo/tools/bot-accounts.sh --bin "$bin" --world "$out/world.dat" --tokens /tmp/soak-bots.tsv \
            --logs "$out" --wait 900 "${names[@]}" -- "${data[@]}" >"$out/accounts.log" 2>&1 \
            || { echo "bots exit accounts" >"$out/bots.exit"; echo "server exit none" >"$out/server.exit"
                 tail -n 20 "$out/accounts.log"; exit 0; }
        tokens=(--tokens /tmp/soak-bots.tsv)
    fi
    # #94: the economy rows for tools/econ-report.py; set after the accounts
    # step so its one-shot server writes none. dev.sh passes no environment.
    export EVE_ECON_TSV="$out/econ.tsv"
    "$bin/${EVE_SERVER_BIN:-eve-offline-server}" --bind 127.0.0.1:26000 --world "$out/world.dat" --status "$out/status.tsv" --turn-sched "${server[@]}" \
        >"$out/server.log" 2>&1 &
    spid=$!
    sample() {
        local kb
        kb=$(awk "/^VmRSS:/{print \$2}" "/proc/$spid/status" 2>/dev/null) || return 0
        [[ -n $kb ]] || return 0
        printf "%s\t%s\t%s\n" "$SECONDS" "$kb" \
            "$(grep -c "entering the game" "$out/server.log" 2>/dev/null)" >>"$out/rss.tsv"
        sample_env
    }
    # #248: what the box did to the server, from read-only files. env.tsv:
    # seconds, cgroup cpu throttle (periods, usec), memory pressure
    # some/full avg10, the server pid majflt. A file that is absent reads "-".
    sample_env() {
        local cg=/sys/fs/cgroup nth tus ps pf mf
        nth=$(awk "/^nr_throttled /{print \$2}" "$cg/cpu.stat" 2>/dev/null) || nth=
        tus=$(awk "/^throttled_usec /{print \$2}" "$cg/cpu.stat" 2>/dev/null) || tus=
        ps=$(sed -n "s/^some .*avg10=\([0-9.]*\).*/\1/p" "$cg/memory.pressure" 2>/dev/null) || ps=
        pf=$(sed -n "s/^full .*avg10=\([0-9.]*\).*/\1/p" "$cg/memory.pressure" 2>/dev/null) || pf=
        mf=$(awk "{print \$12}" "/proc/$spid/stat" 2>/dev/null) || mf=
        printf "%s\t%s\t%s\t%s\t%s\t%s\n" "$SECONDS" "${nth:--}" "${tus:--}" "${ps:--}" "${pf:--}" "${mf:--}" >>"$out/env.tsv"
    }
    hz=$(getconf CLK_TCK 2>/dev/null || echo 100)
    sample_bots() {
        local kb th cpu
        kb=$(awk "/^VmRSS:/{print \$2}" "/proc/$bpid/status" 2>/dev/null) || return 0
        th=$(awk "/^Threads:/{print \$2}" "/proc/$bpid/status" 2>/dev/null) || return 0
        cpu=$(awk -v hz="$hz" "{printf \"%.2f\", (\$14 + \$15) / hz}" "/proc/$bpid/stat" 2>/dev/null) || return 0
        [[ -n $kb && -n $cpu ]] || return 0
        printf "%s\t%s\t%s\t%s\n" "$SECONDS" "$kb" "${th:-0}" "$cpu" >>"$out/bots-rss.tsv"
        # A6: every thread (named after its bot): where it waits (wchan) and its
        # scheduler state (R running, S sleeping, D disk wait); the syscall file
        # needs ptrace rights, so it is not read.
        local t comm wchan sc
        for t in /proc/$bpid/task/*; do
            [[ -d $t ]] || continue
            comm=$(cat "$t/comm" 2>/dev/null) || continue
            wchan=$(cat "$t/wchan" 2>/dev/null || echo "?")
            sc=$(awk "/^State:/{print \$2; exit}" "$t/status" 2>/dev/null || echo "?")
            printf "%s\t%s\t%s\t%s\n" "$SECONDS" "$comm" "${wchan:-0}" "${sc:-?}" >>"$out/bots-threads.tsv"
        done
    }
    printf "secs\trss_kb\tlogins\n" >"$out/rss.tsv"
    printf "secs\tnr_throttled\tthrottled_usec\tmem_some_avg10\tmem_full_avg10\tmajflt\n" >"$out/env.tsv"
    printf "secs\trss_kb\tthreads\tcpu_s\n" >"$out/bots-rss.tsv"
    printf "secs\tthread\twchan\tstate\n" >"$out/bots-threads.tsv"
    SECONDS=0
    ( while kill -0 "$spid" 2>/dev/null; do sample; sleep "$every"; done ) &
    sampler=$!
    for _ in $(seq 120); do
        grep -q "listening" "$out/server.log" 2>/dev/null && break
        kill -0 "$spid" 2>/dev/null || break
        sleep 1
    done
    "$bin/eve-bots" --connect 127.0.0.1:26000 --run "$secs" "${tokens[@]}" "$@" >"$out/bots.log" 2>&1 &
    bpid=$!
    ( while kill -0 "$bpid" 2>/dev/null; do sample_bots; sleep "$every"; done ) &
    bsampler=$!
    wait "$bpid"
    echo "bots exit $?" >"$out/bots.exit"
    # Cleanup check (S10): every bot offline once the bots stopped. A bot
    # that exits in space stays in the log-off hold for at least a minute
    # (crimewatch LOGOFF_MINIMUM), and up to 15 on a combat timer, so wait up
    # to 16 min for a `[live]` line with no pilots; the last one is the verdict.
    for _ in $(seq 480); do
        grep "^\[live\]" "$out/server.log" 2>/dev/null | tail -1 | grep -q "pilots 0 " && break
        kill -0 "$spid" 2>/dev/null || break
        sleep 2
    done
    grep "^\[live\]" "$out/server.log" 2>/dev/null | tail -1 >"$out/offline.txt"
    kill "$sampler" "$bsampler" 2>/dev/null; wait "$sampler" "$bsampler" 2>/dev/null
    sample
    if kill -0 "$spid" 2>/dev/null; then
        # The clean stop: the stop file, then SIGTERM only as a fallback.
        . /repo/tools/stop-server.sh
        if stop_server "$spid" "$out/world.dat.stop" 60; then
            wait "$spid"; echo "server exit $?" >"$out/server.exit"
        else
            wait "$spid"; echo "server killed $?" >"$out/server.exit"
        fi
    else
        wait "$spid"
        echo "server died $?" >"$out/server.exit"
    fi
' soak "$PROFILE" "$SECS" "$SAMPLE_SECS" "$KIT_DRILL" "${#names[@]}" ${names[@]+"${names[@]}"} \
    "$((${#data[@]} + ${#roster[@]} + ${#chatflag[@]}))" \
    "${data[@]}" "${roster[@]}" "${chatflag[@]}" "${botloops[@]}" "${botchat[@]}" \
    "${botdata[@]}" || true

fails=()
fail() { echo "soak: FAIL $*"; fails+=("$1"); }
echo "soak: $PROFILE$( ((CLOSED)) && echo ' closed')$( ((SOCIETY)) && echo ' society')${TIME_SCALE:+ time-scale $TIME_SCALE}${DAY_SECS:+ day-secs $DAY_SECS}, $MINERS miners + $ESCORTS escorts + $BUILDERS builders + $RATTERS ratters$( ((SOCIETY_PVP)) && echo " + $HUNTERS hunters, pvp"), $SECS s;" \
    "$(cat "$out/bots.exit" 2>/dev/null || echo 'bots: no exit recorded');" \
    "$(cat "$out/server.exit" 2>/dev/null || echo 'server: no exit recorded')"

for log in server bots; do
    f="$out/$log.log"
    [[ -f "$f" ]] || { fail "$log-log missing"; continue; }
    panics=$(grep -c "panicked" "$f" || true)
    errors=$(grep -v "ended: read error: Connection reset by peer" "$f" | grep -c "ERROR\|error:" || true)
    unans=$(grep -c "UNANSWERED" "$f" || true)
    echo "soak: $log.log $(wc -l <"$f") lines, $panics panics, $errors error lines, $unans unanswered"
    ((panics == 0)) || fail "panic: $panics in $log.log"
done
grep -q "bots exit 0" "$out/bots.exit" 2>/dev/null \
    || fail "bots: $(cat "$out/bots.exit" 2>/dev/null || echo 'no exit recorded')"

# The server is stopped through its stop file and must exit 0, having
# written its world (lane shutdown). SIGTERM is the fallback, and needing it
# is a failure; so is any other status, or dying first.
read -r _ how code 2>/dev/null <"$out/server.exit" || how=none code=?
case "$how:$code" in
    exit:0) ;;
    died:*) fail "server-exit: died before the bots finished, status $code" ;;
    exit:*) fail "server-exit: status $code after its stop file (137 = SIGKILL, likely OOM)" ;;
    killed:*) fail "server-exit: ignored its stop file for 60 s; SIGTERM, status $code" ;;
    *) fail "server-exit: none recorded" ;;
esac
grep -q "\[shutdown\] world written" "$out/server.log" 2>/dev/null \
    || [[ $how != exit ]] || fail "server-exit: exit 0 without its '[shutdown] world written' line"

# Memory. Peak over the run; growth from the sample where the last bot
# login was seen (or the most logins ever seen, said so) to the peak after it.
if [[ $(wc -l <"$out/rss.tsv" 2>/dev/null || echo 0) -gt 1 ]]; then
    read -r peak first base logins last samples < <(awk -F'\t' -v want="$BOTS" '
        NR == 1 { next }
        { n++; kb = $2; if (kb > peak) peak = kb; if (n == 1) first = kb
          if ($3 > maxl) { maxl = $3; at = n }
          if (!full && $3 >= want) { full = n }
          rss[n] = kb; last = kb }
        END { from = full ? full : at; if (!from) from = 1
              post = 0; for (i = from; i <= n; i++) if (rss[i] > post) post = rss[i]
              printf "%d %d %d %d %d %d\n", peak/1024, first/1024, rss[from]/1024,
                     maxl, (post - rss[from])/1024, n }' "$out/rss.tsv")
    echo "soak: RSS peak $peak MB (first $first MB, at last login $base MB," \
        "growth after it $last MB; $logins/$BOTS logins; $samples samples in rss.tsv)"
    ((logins >= BOTS)) || echo "soak: note: only $logins of $BOTS bots logged in; growth is from the most seen"
    if ((RSS_BUDGET_MB > 0 && peak > RSS_BUDGET_MB)); then
        fail "RSS: peak $peak MB over the --rss-budget of $RSS_BUDGET_MB MB"
    fi
    ((last <= RSS_GROWTH_MB)) \
        || fail "RSS: grew $last MB after the last login, over --rss-growth $RSS_GROWTH_MB MB"
else
    fail "RSS: no VmRSS sample in rss.tsv"
fi

# The world-turn hold, once the server prints `[turns]`.
turns=$(grep -c '^\[turns\]' "$out/server.log" 2>/dev/null || true)
if ((turns > 0)); then
    hold=$(grep '^\[turns\]' "$out/server.log" \
        | sed -n 's/.*hold_max=\([0-9.]*\).*/\1/p' | sort -g | tail -1)
    if [[ -z $hold ]]; then
        fail "turns: $turns [turns] lines but no hold_max=<ms> field"
    else
        echo "soak: [turns] $turns lines, hold max ${hold} ms (limit $HOLD_LIMIT ms, $PROFILE)"
        awk -v h="$hold" -v l="$HOLD_LIMIT" 'BEGIN { exit !(h > l) }' \
            && fail "turns: hold max $hold ms over --hold-ms $HOLD_LIMIT"
    fi
elif ((TURNS_REQUIRED)); then
    fail "turns: no [turns] line in server.log"
else
    echo "soak: note: no [turns] line yet (CP15); the hold is not judged"
fi

# #248: the box's own pressure over the soak (cgroup cpu throttle, memory
# pressure, server majflt), reported, not judged: it tells a world-turn hold
# that was the server's from one that was the container's. `-` = not readable.
if [[ $(wc -l <"$out/env.tsv" 2>/dev/null || echo 0) -gt 1 ]]; then
    awk -F'\t' '
        NR == 1 { next }
        { n++; if ($2 != "-") { if (f2 == "") f2 = $2; l2 = $2 }
          if ($3 != "-") { if (f3 == "") f3 = $3; l3 = $3 }
          if ($4 != "-" && $4 + 0 > ps) ps = $4 + 0
          if ($5 != "-" && $5 + 0 > pf) pf = $5 + 0
          if ($6 != "-") { if (f6 == "") f6 = $6; l6 = $6 } }
        END {
            printf "soak: box: %d samples; cpu throttled %s periods / %s ms; memory pressure avg10 max some %s full %s; server majflt +%s\n", n,
                f2 == "" ? "?" : l2 - f2, f3 == "" ? "?" : (l3 - f3) / 1000, ps + 0, pf + 0, f6 == "" ? "?" : l6 - f6
        }' "$out/env.tsv"
fi

# The bot process (R-ECON B3): reported, not judged.
if [[ $(wc -l <"$out/bots-rss.tsv" 2>/dev/null || echo 0) -gt 1 ]]; then
    awk -F'\t' -v bots="$BOTS" '
        NR == 1 { next }
        { n++; if ($2 > peak) peak = $2; if ($3 > th) th = $3
          if (n == 1) { t0 = $1; c0 = $4 } t1 = $1; c1 = $4 }
        END { span = t1 - t0; pct = span > 0 ? 100 * (c1 - c0) / span : 0
              printf "soak: eve-bots RSS peak %d MB (%d KB a bot), %d threads, CPU %.1f s, %.1f%% of a core over %d s (%d samples)\n",
                     peak / 1024, bots ? peak / bots : 0, th, c1, pct, span, n }' "$out/bots-rss.tsv"
else
    echo "soak: note: no eve-bots sample in bots-rss.tsv"
fi

# The saves, split (E1 `[save]`): the worst window and the run's cost per KB.
if grep -q '^\[save\]' "$out/server.log" 2>/dev/null; then
    grep '^\[save\]' "$out/server.log" | awk '
        { for (i = 2; i <= NF; i++) { split($i, kv, "="); v[kv[1]] = kv[2] + 0 }
          saves += v["saves"]; split_n += v["split"]; writes += v["writes"]; coal += v["coalesced"]
          if (v["bytes_max"] > bytes) bytes = v["bytes_max"]
          if (v["snapshot_max"] > snap) snap = v["snapshot_max"]
          if (v["render_max"] > render) render = v["render_max"]
          if (v["write_max"] > write) write = v["write_max"]
          ru += v["render_us_per_kb"] * v["split"]; wu += v["write_us_per_kb"] * v["writes"] }
        END { printf "soak: [save] %d saves (%d split), %d written (%d coalesced), bytes max %d, max snapshot %.1f ms render %.1f ms (under the lock) write %.1f ms (off it), mean render %.1f us/KB write %.1f us/KB\n",
                     saves, split_n, writes, coal, bytes, snap, render, write,
                     split_n ? ru / split_n : 0, writes ? wu / writes : 0 }'
else
    echo "soak: note: no [save] line in server.log"
fi

# The watched calls' holds (E1 `labels=`), the run's max per label.
labels=$(grep '^\[turns\]' "$out/server.log" 2>/dev/null \
    | sed -n 's/.* labels=\([^ ]*\).*/\1/p' | tr ',' '\n' | grep -v '^-\?$' \
    | awk -F: '{ n[$1] += $2; ms = $3 + 0; if (ms > m[$1]) m[$1] = ms }
               END { for (k in n) printf "%s %d calls, hold max %.1f ms\n", k, n[k], m[k] }' \
    | sort || true)
if [[ -n $labels ]]; then
    echo "soak: watched holds:"
    printf '%s\n' "$labels" | sed 's/^/soak:   /'
fi

# What nobody answered, most frequent first: the next packet's to-do list.
top=$( { grep -ho '\[svc\] UNANSWERED .*' "$out/server.log" 2>/dev/null \
            | sed 's/^\[svc\] UNANSWERED //; s/ -- answering None.*//'
         grep -ho 'UNANSWERED  *[^ ]*' "$out/bots.log" 2>/dev/null \
            | sed 's/^UNANSWERED  */bots: /'; } \
       | sort | uniq -c | sort -rn | head -10 || true)
if [[ -n $top ]]; then
    echo "soak: top UNANSWERED:"
    printf '%s\n' "$top" | sed 's/^/soak:   /'
fi

# Society mode: the last population line and the switches seen (reported,
# never judged: gate GA judges a 6 h run).
if ((SOCIETY)); then
    soc=$(grep '^\[soc\] bots ' "$out/bots.log" 2>/dev/null | tail -1 || true)
    switches=$(grep -c '^\[soc\] .* decision switch ' "$out/bots.log" 2>/dev/null || true)
    sessions=$(grep -c ' session .*log off' "$out/bots.log" 2>/dev/null || true)
    echo "soak: society: ${soc:-no [soc] summary line}; $switches switches, $sessions log-offs"
fi

# S10: tick p95/max, lateness, UserErrors by key, the log deny-list, bot
# docks and act failures, and the cleanup check; summary.tsv beside the logs.
# A deny-list hit or a bot left online is a verdict failure.
summary_status=0
python3 -I "$REPO/tools/soak_summary.py" "$out" ${ALLOW_UNANSWERED:+--allow "UNANSWERED=$ALLOW_UNANSWERED"} >"$out/summary.txt" || summary_status=$?
grep -v '^soak: FAIL' "$out/summary.txt" || true
while IFS= read -r l; do fail "${l#soak: FAIL }"; done < <(grep '^soak: FAIL' "$out/summary.txt" || true)
((summary_status == 0 || ${#fails[@]})) || fail "summary: soak_summary.py status $summary_status"

# BP-SOAK: the PvP behaviours, one line each (reported, never judged).
if ((SOCIETY_PVP)); then
    echo "soak: pvp behaviours (tools/pvp-report.py):"
    python3 -I "$REPO/tools/pvp-report.py" "$out/bots.log" | sed 's/^/soak:   /' || true
fi

# #94, #21, #23, #2: the society, doctrine and economy reports, saved beside the
# logs (reported, never judged here; judge with tools/econ-report.py --judge).
if ((SOCIETY)); then
    python3 -I "$REPO/tools/society-report.py" "$out/bots.log" "$out/server.log" >"$out/society.txt" 2>&1 || true
    python3 -I "$REPO/tools/doctrine-report.py" "$out/bots.log" "$out/server.log" >"$out/doctrine.txt" 2>&1 || true
    echo "soak: society.txt and doctrine.txt in $out (tools/society-report.py, tools/doctrine-report.py)"
fi
if [[ -s "$out/econ.tsv" ]]; then
    python3 -I "$REPO/tools/econ-report.py" "$out/econ.tsv" >"$out/econ.txt" 2>&1 || true
    echo "soak: econ.txt in $out (tools/econ-report.py; --judge for the #94 checks)"
fi

# Soak D: the money identity (#156), and the economy judges (--judges: #155 #158 #159 #142 #144 #2 #23).
python3 -I "$REPO/tools/econ-report.py" --identity "$out" >"$out/identity.txt" 2>&1 || true
echo "soak: identity.txt in $out (tools/econ-report.py --identity: the #156 money identity)"
bot_args=(--judges --scale "${TIME_SCALE:-1}"); ((CLOSED)) && bot_args+=(--closed)
[[ -d "$REPO/out/content" ]] && bot_args+=(--content "$REPO/out/content")
python3 -I "$REPO/tools/bot-econ-report.py" "$out" "${bot_args[@]}" >"$out/bot-econ.txt" 2>&1 || true
echo "soak: bot-econ.txt in $out (tools/bot-econ-report.py --judges: the #2, #23, #142, #155, #158 and #159 lines, never gating)"

echo "soak: logs, rss.tsv, bots-rss.tsv and bots-threads.tsv in $out"
if ((${#fails[@]})); then
    echo "soak: verdict FAIL (${fails[*]})"
    exit 1
fi
echo "soak: verdict PASS"
