#!/usr/bin/env bash
# A remote player's one-time setup, in steps (F-KIT, issue #140). Idempotent: run a
# step again and it says what is done and changes nothing that is.
#
#   tools/client-setup.sh check    # read only: build, disk, Proton, ResFiles, jail probe
#   tools/client-setup.sh copy     # tq -> tq-offline, then your own prefix (offline-prefix.sh)
#   tools/client-setup.sh patch    # pin check, plan, apply, verify on tq-offline
#   tools/client-setup.sh refresh  # your launcher moved on to a newer served build:
#                                  # keep the old tq-offline as tq-<oldbuild>, then copy and patch again
#
# The client is not in the kit: you need a full EVE install (Steam or the
# launcher, with all resource files downloaded) at a build the server serves
# (the newest one is the target; `check` says which).
# `copy` writes one new directory, `tq-offline`, beside `tq` in your online
# prefix; it never changes `tq`. It refuses while Wine runs in that prefix.
#
# Paths: STEAM_ROOT (default ~/.local/share/Steam; the Ubuntu .deb's is
# ~/.steam/debian-installation, Flatpak's is under ~/.var/app), EVE_ONLINE_PREFIX
# (the Steam prefix your EVE lives in, default $STEAM_ROOT/steamapps/compatdata/8500;
# set it when your library is elsewhere), EVE_OFFLINE_PREFIX (where your own
# prefix goes), PROTON, as in tools/offline-prefix.sh. The launch scripts
# (tools/jail-rootless.sh) read the offline prefix from EVE_PREFIX, not
# EVE_OFFLINE_PREFIX: if you set the latter, set EVE_PREFIX to the same path.
#
# `copy` makes your own prefix with hard-linked resource files (about 10G) when
# they and the prefix are on one filesystem. `copy --full` makes a real copy
# instead (about 60G), for a prefix on another disk.
set -euo pipefail

REPO=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=tools/prefix-lib.sh
source "$REPO/tools/prefix-lib.sh"

say() { printf '%s\n' "$*"; }
die() { printf 'client-setup: %s\n' "$*" >&2; exit 1; }

(( EUID != 0 )) || die "run this as yourself, not as root"
: "${STEAM_ROOT:=$HOME/.local/share/Steam}"
: "${EVE_ONLINE_PREFIX:=$STEAM_ROOT/steamapps/compatdata/$EVE_ONLINE_PREFIX_ID}"
: "${EVE_OFFLINE_PREFIX_SET:=}"
[[ -z ${EVE_OFFLINE_PREFIX:-} ]] || EVE_OFFLINE_PREFIX_SET=1
: "${EVE_OFFLINE_PREFIX:=$(eve_offline_prefix "$HOME")}"
: "${PROTON:=$STEAM_ROOT/compatibilitytools.d/GE-Proton10-34/proton}"
EVE_REL=pfx/drive_c/CCP/EVE
ON=$EVE_ONLINE_PREFIX/$EVE_REL
OFF=$EVE_OFFLINE_PREFIX/$EVE_REL

# The pinned build: the `pinned` row of tools/builds.tsv. The served builds are
# the rows of the pinned build's line that are `pinned`, `equivalent` or
# `provisional`; the target is the highest of them (the build the server is
# ahead on or level with), and any served build can be played on.
pinned=$(awk -F'\t' '!/^#/ && NF == 5 && $3 == "pinned" { print $1; exit }' "$REPO/tools/builds.tsv" 2>/dev/null || true)
[[ -n $pinned ]] || die "tools/builds.tsv has no pinned build (is this the kit?)"
served=$(awk -F'\t' -v pin="$pinned" '
    !/^#/ && NF == 5 { if ($1 == pin) line = $2; row[$1] = $2 " " $3 }
    END { for (b in row) { split(row[b], r, " ");
        if (r[1] == line && (r[2] == "pinned" || r[2] == "equivalent" || r[2] == "provisional")) print b } }
' "$REPO/tools/builds.tsv" 2>/dev/null | sort -n | tr '\n' ' ')
target=$(printf '%s\n' $served | tail -1)
is_served() { local b; for b in $served; do [[ $b == "$1" ]] && return 0; done; return 1; }
# "pinned" for the pinned row, else "served": the word the messages use.
label_of() { [[ $1 == "$pinned" ]] && printf pinned || printf served; }
tlabel=$(label_of "$target")

ini_get() {   # FILE KEY
    sed -n "s/^[[:space:]]*$2[[:space:]]*=[[:space:]]*//p" "$1" 2>/dev/null | tr -d '\r' | head -1 || true
}

# Is a Wine or Proton process using this prefix? (prefix-lib.sh: eve_prefix_busy)
prefix_busy() { eve_prefix_busy "$1" >/dev/null; }

# resfileindex.txt and resfileindex_Windows.txt: rows are
# path,file,md5,size,compressed-size; the file is under ResFiles by its second
# field and has the first size. Every index must have rows, and every row's
# file must exist at that size. Prints "checked N missing M wrong-size S" (a
# few names go to stderr); exit 0 only when all is well.
res_rows() {   # CLIENT_DIR RESFILES_DIR
    python3 -I -c '
import os, sys
cd, res = sys.argv[1], sys.argv[2]
n = miss = bad = 0
ok = True
for name in ("resfileindex.txt", "resfileindex_Windows.txt"):
    rows = 0
    try:
        f = open(os.path.join(cd, name), encoding="utf-8", errors="replace")
    except OSError:
        print("  no " + name, file=sys.stderr)
        ok = False
        continue
    with f:
        for line in f:
            p = line.rstrip("\r\n").split(",")
            if len(p) < 4:
                continue
            rows += 1
            path = os.path.join(res, p[1])
            try:
                size = os.stat(path).st_size
            except OSError:
                miss += 1
                if miss <= 3:
                    print("  missing " + p[1], file=sys.stderr)
                continue
            if p[3].isdigit() and size != int(p[3]):
                bad += 1
                if bad <= 3:
                    print("  wrong size " + p[1], file=sys.stderr)
    n += rows
    if rows == 0:
        print("  " + name + " has no rows", file=sys.stderr)
        ok = False
print("checked %d missing %d wrong-size %d" % (n, miss, bad))
sys.exit(0 if ok and miss == 0 and bad == 0 else 1)
' "$1" "$2"
}

# The nearest directory that exists at or above PATH.
existing_dir() { local d=$1; while [[ ! -d $d && $d != / ]]; do d=$(dirname -- "$d"); done; printf '%s' "$d"; }
dev_of() { stat -c %d -- "$(existing_dir "$1")"; }
same_fs() { [[ $(dev_of "$1") == "$(dev_of "$2")" ]]; }

# The way forward for a copy that is not a served build. What it advises
# depends on what is left: tq-offline in your online prefix may be your only
# copy of an older build, so it is moved aside, never removed.
stale_help() {
    local tq_ok=0 off_ok=0 opt="" ob
    ob=$(ini_get "$ON/tq-offline/start.ini" build)
    is_served "$(ini_get "$ON/tq/start.ini" build)" && tq_ok=1
    if is_served "$ob" && "$REPO/tools/pin-client.sh" --path "$ON/tq-offline" >/dev/null 2>&1; then off_ok=1; fi
    same_fs "$ON/ResFiles" "$EVE_OFFLINE_PREFIX" && opt=" --link-resfiles"
    if (( off_ok )); then
        say "        Your tq-offline in the online prefix is a served build and matches its pin: do NOT remove it."
        say "        Make your own prefix again from it:"
        say "          tools/offline-prefix.sh --force$opt   # replaces your own prefix, which holds the bad copy"
        say "          tools/client-setup.sh patch"
    elif (( tq_ok )); then
        say "        Your install is a served build. One step does it all (it keeps the old copy as tq-<oldbuild>):"
        say "          tools/client-setup.sh refresh"
        say "        or by hand, keeping the old copy beside it (nothing is deleted):"
        [[ ! -e $ON/tq-offline ]] || say "          mv '$ON/tq-offline' '$ON/tq-${ob:-old}'"
        say "          tools/client-setup.sh copy                        # tq-offline again; it may stop at your old prefix"
        say "          tools/offline-prefix.sh --force$opt   # replaces your own prefix, which holds the old copy"
        say "          tools/client-setup.sh patch"
    else
        say "        Do NOT remove tq-offline: your install is not a build the server serves ($served), so a tq-offline"
        say "        that is, or may be, a served build is your only copy and cannot be downloaded again."
        say "        Update your launcher to the newest served build, or ask the owner."
    fi
}

# Run the ResFiles test, print its result; 0 when the install is complete.
# (shared by check and copy)
res_ok() {
    local out r=0
    out=$(res_rows "$ON/tq" "$ON/ResFiles" 2>&1) || r=1
    if (( r == 0 )); then say "  ok    ResFiles complete (${out##*$'\n'})"; return 0; fi
    say "  FAIL  ResFiles incomplete: ${out##*$'\n'}"
    [[ $out != *$'\n'* ]] || say "${out%$'\n'*}"
    say "        Let the launcher or Steam finish the full download, then run this again."
    return 1
}

step_check() {
    local rc=0 b ob free need_k=0 res_k=0 ahead="the server has not caught up with build"
    say "== client-setup check (pinned build $pinned, newest served build $target)"
    ob=$(ini_get "$ON/tq-offline/start.ini" build)
    [[ -n $ob ]] || ob=$(ini_get "$OFF/tq-offline/start.ini" build)
    if [[ -f $ON/tq/start.ini ]]; then
        b=$(ini_get "$ON/tq/start.ini" build)
        if is_served "$b"; then
            say "  ok    your install is build $b"
            [[ $b == "$target" ]] || say "  WARN  the newest served build is $target: let your launcher update, then run refresh"
            if [[ -n $ob && $ob != "$b" ]] && is_served "$ob"; then
                say "  FAIL  refresh: your tq-offline copy is build $ob, your install is $b: run tools/client-setup.sh refresh"
                rc=1
            fi
        elif is_served "$ob"; then
            say "  ok    your install moved on to build ${b:-unknown}, but your tq-offline copy is the served $ob: play on that one"
            [[ ! $b =~ ^[0-9]+$ ]] || (( b <= target )) \
                || say "        $ahead $b yet: wait, see the patch-day issue (do not refresh until it is served)."
        elif [[ $b =~ ^[0-9]+$ ]] && (( b > target )); then
            say "  FAIL  your install is build $b: $ahead $b yet: wait, see the patch-day issue."
            say "        You cannot play on this copy until the server serves $b."
            rc=1
        else
            say "  FAIL  your install is build ${b:-unknown}, the server serves $served."
            say "        You cannot play on this copy. Update your launcher to the newest served build ($target), then run:"
            say "          tools/client-setup.sh refresh"
            rc=1
        fi
    elif [[ -f $OFF/tq-offline/start.ini ]]; then
        say "  ok    no online install, but tq-offline exists (the copy step is done)"
    else
        say "  FAIL  no EVE install at $ON/tq (set STEAM_ROOT or EVE_ONLINE_PREFIX; Flatpak and the Ubuntu .deb keep Steam elsewhere)"
        rc=1
    fi
    if [[ -n $ob ]] && ! is_served "$ob"; then
        say "  FAIL  your tq-offline copy is build $ob, not the $tlabel $target (left from before a re-pin?)"
        stale_help
        rc=1
    fi
    if [[ -f $ON/tq/resfileindex.txt && -d $ON/ResFiles ]]; then
        res_ok || rc=1
    elif [[ -d $ON/tq ]]; then
        say "  FAIL  no ResFiles or resfileindex.txt: the install is not fully downloaded"; rc=1
    fi
    if [[ -d $ON/tq ]]; then
        need_k=$(du -sk -- "$ON/tq" | cut -f1)
        [[ ! -d $ON/ResFiles ]] || res_k=$(du -sk -- "$ON/ResFiles" | cut -f1)
        free=$(df -k --output=avail -- "$HOME" | tail -1 | tr -d ' ')
        if same_fs "$ON/ResFiles" "$EVE_OFFLINE_PREFIX"; then
            if (( free > need_k * 3 + 12582912 )); then say "  ok    disk: $(( free / 1048576 ))G free; ResFiles can be hard-linked (about 10G)"
            else say "  WARN  disk: $(( free / 1048576 ))G free; the copy needs about 10G"; fi
        else
            say "  WARN  your resource files and $EVE_OFFLINE_PREFIX are on different filesystems: they cannot be hard-linked."
            say "        Use: tools/client-setup.sh copy --full   (a real copy, about $(( (need_k + res_k) / 1048576 + 8 ))G; $(( free / 1048576 ))G free here)"
            (( free > (need_k + res_k) + 8388608 )) || { say "  FAIL  not enough room for a full copy"; rc=1; }
        fi
    fi
    if [[ -x $PROTON ]]; then say "  ok    Proton at $PROTON"
    else say "  FAIL  no Proton at $PROTON (GE-Proton10-34 under compatibilitytools.d, or set PROTON)"; rc=1; fi
    if "$REPO/tools/jail-rootless.sh" probe; then :; else rc=1; fi
    if (( rc == 0 )); then
        if [[ -n $ob && ( ! -f $ON/tq/start.ini || $ob == "$(ini_get "$ON/tq/start.ini" build)" ) ]]; then say "up to date: build $ob"
        else say "ready for: tools/client-setup.sh copy"; fi
    fi
    return "$rc"
}

COPY_PART=""
cleanup_copy() { [[ -z $COPY_PART ]] || rm -rf -- "$COPY_PART"; }

step_copy() {
    local full=0 refresh=0 a b ob need_k free
    for a in "$@"; do
        case $a in
            --full) full=1 ;;
            --refresh) refresh=1 ;;   # internal: `refresh` makes your own prefix again when it holds another build
            *) die "copy: unknown argument $a (only --full)" ;;
        esac
    done
    local part=$ON/tq-offline.partial
    ob=$(ini_get "$ON/tq-offline/start.ini" build)
    if [[ -f $ON/tq-offline/bin64/exefile.exe ]] && is_served "$ob"; then
        say "== tq-offline already exists in the online prefix (build $ob); not copying it again"
    else
        if [[ -e $ON/tq-offline && -n $ob ]] && ! is_served "$ob"; then
            say "  FAIL  $ON/tq-offline is build $ob, not the $tlabel $target"; stale_help; exit 1
        fi
        [[ -f $ON/tq/start.ini ]] || die "no install at $ON/tq (run check)"
        b=$(ini_get "$ON/tq/start.ini" build)
        is_served "$b" || die "your install is build ${b:-unknown}, not the $tlabel $target: you cannot play on it (run check)"
        [[ -f $ON/tq/resfileindex.txt && -d $ON/ResFiles ]] || die "no ResFiles or resfileindex.txt: the install is not fully downloaded (run check)"
        res_ok || exit 1
        if prefix_busy "$EVE_ONLINE_PREFIX"; then die "Wine is running in $EVE_ONLINE_PREFIX (pids $(eve_prefix_busy "$EVE_ONLINE_PREFIX")); close EVE and the launcher first"; fi
        [[ ! -e $ON/tq-offline ]] || die "$ON/tq-offline exists but is incomplete; remove it (rm -rf '$ON/tq-offline') and run again"
        need_k=$(du -sk -- "$ON/tq" | cut -f1)
        free=$(df -k --output=avail -- "$ON" | tail -1 | tr -d ' ')
        (( free > need_k + 1048576 )) || die "not enough room next to $ON/tq: it needs $(( need_k / 1048576 ))G and 1G more, $(( free / 1048576 ))G free"
        # A half copy from an earlier, stopped run is not a copy: remove it so the
        # new one does not land inside it.
        rm -rf -- "$part"
        COPY_PART=$part
        trap cleanup_copy EXIT
        trap 'exit 130' INT TERM
        say "== copying tq to tq-offline (a real copy; tq itself is not changed)"
        cp -aT --reflink=auto -- "$ON/tq" "$part"
        # The launcher or a Wine may have started while it copied.
        if prefix_busy "$EVE_ONLINE_PREFIX"; then die "Wine started in $EVE_ONLINE_PREFIX during the copy; nothing was kept. Close EVE and run again"; fi
        [[ $(ini_get "$ON/tq/start.ini" build) == "$b" && $(ini_get "$part/start.ini" build) == "$b" ]] \
            || die "the build changed during the copy; nothing was kept. Run check"
        [[ -f $part/bin64/exefile.exe ]] || die "the copy has no bin64/exefile.exe; nothing was kept"
        mv -T -- "$part" "$ON/tq-offline"
        COPY_PART=""
        trap - EXIT INT TERM
    fi
    local -a opts=(--link-resfiles)
    if (( full )); then opts=()
    elif ! same_fs "$ON/ResFiles" "$EVE_OFFLINE_PREFIX"; then
        die "your resource files and $EVE_OFFLINE_PREFIX are on different filesystems, so they cannot be hard-linked.
  Run: tools/client-setup.sh copy --full    (a real copy: about 60G; check says how much room there is)"
    fi
    if [[ -f $EVE_OFFLINE_PREFIX/eve-offline-prefix.txt ]]; then
        say "== your own prefix exists already: $EVE_OFFLINE_PREFIX"
        # Display only, so a rerun is safe: it changes nothing, and before the
        # patch step its cryptoPack line is expected to say MISS.
        "$REPO/tools/offline-prefix.sh" --check || say "  (a MISS before 'patch' is expected for cryptoPack; if the prefix itself is damaged: tools/offline-prefix.sh --force ${opts[*]})"
        [[ -f $OFF/tq-offline/bin64/exefile.exe ]] || { say "  FAIL  your own prefix has no tq-offline client; make it again: tools/offline-prefix.sh --force ${opts[*]}"; exit 1; }
        ob=$(ini_get "$OFF/tq-offline/start.ini" build)
        if (( refresh )) && [[ $ob != "$(ini_get "$ON/tq-offline/start.ini" build)" ]]; then
            say "== your own prefix holds tq-offline build ${ob:-unknown}; making it again from the refreshed copy"
            "$REPO/tools/offline-prefix.sh" --force "${opts[@]}"
        elif ! is_served "$ob"; then
            say "  FAIL  your own prefix holds tq-offline build ${ob:-unknown}, not the $tlabel $target"; stale_help; exit 1
        fi
    elif [[ -e $EVE_OFFLINE_PREFIX.partial ]]; then
        say "== a half-made prefix from an earlier run is at $EVE_OFFLINE_PREFIX.partial; making it again"
        "$REPO/tools/offline-prefix.sh" --force "${opts[@]}"
    else
        "$REPO/tools/offline-prefix.sh" "${opts[@]}"
    fi
    if [[ -n ${EVE_OFFLINE_PREFIX_SET:-} ]]; then say "note: also set EVE_PREFIX='$EVE_OFFLINE_PREFIX' when you launch (tools/jail-rootless.sh reads that name)"; fi
    say "next: tools/client-setup.sh patch"
}

step_patch() {
    local c=$OFF/tq-offline
    [[ -f $c/start.ini ]] || die "no $c (run: tools/client-setup.sh copy)"
    local cb
    cb=$(ini_get "$c/start.ini" build)
    is_served "$cb" \
        || die "this copy is build ${cb:-unknown}, not the $tlabel $target: you cannot play on it"
    # pin-client.sh accepts the stock files and the patcher's patched ones, so a
    # failure here is a real difference: a damaged or foreign copy.
    # Its failure is a diff of two hash lists, a few hundred lines on a wrong
    # copy: show the head of it, not a screenful.
    local pinout pinrc=0 n
    pinout=$("$REPO/tools/pin-client.sh" --path "$c" 2>&1) || pinrc=$?
    if (( pinrc == 2 )); then
        printf '%s\n' "$pinout" | head -n 12
        die "pin-client.sh could not run its check (exit 2: no tools/client-pin.sha256 or no client at $c). If the manifest is missing the kit is incomplete: unpack it again"
    elif (( pinrc )); then
        printf '%s\n' "$pinout" | head -n 12
        n=$(printf '%s\n' "$pinout" | wc -l)
        (( n <= 12 )) || say "  ... ($(( n - 12 )) more lines; tools/pin-client.sh --path '$c' prints them all)"
        say "client-setup: $c does not match the $(label_of "$cb") build $cb (pin-client.sh, above)." >&2
        say "        'copy' alone changes nothing while tq-offline is in place."
        stale_help
        exit 1
    fi
    printf '%s\n' "$pinout" | tail -n 3
    say "== plan"
    python3 -I "$REPO/tools/eve_patch.py" plan --client "$c"
    say "== apply"
    python3 -I "$REPO/tools/eve_patch.py" apply --client "$c"
    say "== verify"
    python3 -I "$REPO/tools/eve_patch.py" verify --client "$c"
    say "next: tools/account.sh login, then the tunnel (docs/guides/remote-clients.md)"
}

# Your launcher moved on to a newer served build. The old tq-offline is moved to
# tq-<oldbuild> (kept, a frozen copy you can still play on; delete it yourself),
# then copy and patch run again for the build the launcher installed.
step_refresh() {
    local tqb ob offb dest
    [[ -f $ON/tq/start.ini ]] || die "no install at $ON/tq (run check)"
    tqb=$(ini_get "$ON/tq/start.ini" build)
    if ! is_served "$tqb"; then
        if [[ $tqb =~ ^[0-9]+$ ]] && (( tqb > target )); then
            say "the server has not caught up with build $tqb yet: wait, see the patch-day issue. Nothing was changed."
            exit 1
        fi
        die "your install is build ${tqb:-unknown}, the server serves $served: update your launcher first. Nothing was changed."
    fi
    ob=$(ini_get "$ON/tq-offline/start.ini" build)
    offb=$(ini_get "$OFF/tq-offline/start.ini" build)
    if [[ $ob == "$tqb" && $offb == "$tqb" ]]; then
        say "up to date: build $tqb (tq-offline and your own prefix both hold it); nothing to refresh"
        return 0
    fi
    if [[ -e $ON/tq-offline && $ob != "$tqb" ]]; then
        say "== tq-offline in your online prefix is build ${ob:-unknown} (your install is $tqb)"
        if prefix_busy "$EVE_ONLINE_PREFIX"; then die "Wine is running in $EVE_ONLINE_PREFIX (pids $(eve_prefix_busy "$EVE_ONLINE_PREFIX")); close EVE and the launcher first"; fi
        dest=$ON/tq-${ob:-unknown}
        [[ ! -e $dest ]] || die "$dest exists already; rename or remove it yourself, then run refresh again"
        mv -T -- "$ON/tq-offline" "$dest"
        say "   kept as $dest (a frozen copy of build ${ob:-unknown}; delete it yourself when you no longer need it)"
    fi
    step_copy --refresh "$@"
    step_patch
}

case "${1:-}" in
    check) step_check ;;
    refresh) shift; step_refresh "$@" ;;
    copy)  shift; step_copy "$@" ;;
    patch) step_patch ;;
    -h|--help|'') sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; [[ -n ${1:-} ]] || exit 2 ;;
    *) die "unknown step $1 (check, copy, patch, refresh)" ;;
esac
