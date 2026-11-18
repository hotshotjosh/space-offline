#!/usr/bin/env bash
# Bring a new client build dir into the repo's build registry, step by step.
# R-NEWBUILD §3 (NB-1, NB-2). Read-only on the install; never launches the
# client; the analysis container has no network.
#
#   tools/new-build.sh DIR [--prefix P] [--pinned NAME] [--go-on] [--data-done] [--report] [--dry-run]
#   tools/new-build.sh --gate-check        # NB-1: re-verify committed deltas
#
#   DIR        install dir: a name under the prefix (tq-new) or a path
#   --prefix   the EVE/ dir holding the installs (default: the offline
#              prefix copy, never the online one; or $EVE_PREFIX)
#   --pinned   the pinned install's dir name beside DIR (default tq-offline)
#   --go-on    continue past a `differs` verdict (it is still reported)
#   --data-done  step 7 (static data) has been done by hand; do not stop there
#   --report   after the steps, run tools/port-surface.sh, bundle-diff.py,
#              port-recheck.py, client-ci/fci-surface.py and content-report.py
#              (those present; the answer shapes come from port-surface's G7), then
#              eve_patch_author.py locate and build-triage.py, and print
#              their exit codes and the tier (T0-T3)
#   --dry-run  print what each step would run, run nothing
#
# Steps (docs/guides/build-compat.md): 1 the dir is an install; 2 hash manifest
# (tools/pin-client.sh, into out/); 3 delta against the PINNED build;
# 4 `differs` reading, preceded by the triage of the delta alone (4a: the tier,
# tools/build-triage.py exit 20-23 = T0-T3); 5 loader layout (fsd.CFSD_BUILD_LAYOUTS) and expected
# counts; 6 registry row; 7 data; 8 patch manifest and the hosts check;
# 9 gate. Layout and registry come before the data: content.py refuses a
# build that has neither.
# A step that needs a human decision prints exactly what to do and the
# script exits 10. Exit: 0 all done, 1 a step failed, 2 usage, 10 stopped.
#
# `--gate-check` is what `tools/gate.sh --env-checks` runs (not the landing gate): for every committed
# findings/build-delta-<old>-<new>.tsv whose two installs both exist on
# this machine it runs build-delta.py --check; it skips with a note when
# they do not. Every delta is checked, not only those against the pinned
# build: after a re-pin the older deltas stay as history and must still
# reproduce. Exit 0 ok/skipped, 1 a delta no longer reproduces.
# Test hooks: EVE_ANALYSIS (default tools/analysis.sh), EVE_PREFIX.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

analysis=${EVE_ANALYSIS:-tools/analysis.sh}
registry=tools/builds.tsv

usage() { sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; }
die() { echo "new-build: $*" >&2; exit 2; }

prefixes() {
    if [[ -n ${EVE_PREFIX:-} ]]; then echo "$EVE_PREFIX"; return; fi
    echo "$HOME/.local/share/eve-offline/prefix/pfx/drive_c/CCP/EVE"
    # Never the online prefix (compatdata/8500): a typo'd DIR must not fall
    # back into it. Pass --prefix or EVE_PREFIX for any other place.
}
ini_get() { sed -n "s/^$2 *= *//p" "$1/start.ini" 2>/dev/null | head -1 | tr -d '\r '; }
# find_install BUILD: print the first install dir whose start.ini says BUILD.
find_install() {
    local p d
    while read -r p; do
        for d in "$p"/*/; do
            d=${d%/}
            [[ -f $d/start.ini && $(ini_get "$d" build) == "$1" ]] && { echo "$d"; return 0; }
        done
    done < <(prefixes)
    return 1
}
pinned_build() { awk -F'\t' '!/^#/ && $3=="pinned" {print $1; exit}' "$registry"; }

if [[ ${1:-} == --gate-check ]]; then
    [[ -f $registry ]] || { echo "   build deltas: no $registry; skipped"; exit 0; }
    shopt -s nullglob
    files=(findings/build-delta-*-*.tsv)
    ((${#files[@]})) || { echo "   build deltas: none committed; skipped"; exit 0; }
    for f in "${files[@]}"; do
        pair=${f##*/build-delta-}; pair=${pair%.tsv}
        old=${pair%%-*}; new=${pair#*-}
        # a delta is old->new; the old side is the baseline it was taken against
        od=$(find_install "$old") || { echo "   $f: build $old not installed here; skipped"; continue; }
        nd=$(find_install "$new") || { echo "   $f: build $new not installed here; skipped"; continue; }
        if [[ $(dirname "$od") != "$(dirname "$nd")" ]]; then
            echo "   $f: installs are in different prefixes; skipped"; continue
        fi
        rc=0
        EVE_CLIENT_DIR=$(basename "$nd") EVE_CLIENT=$nd "$analysis" python3 -I tools/build-delta.py \
            "/client-root/$(basename "$od")" "/client-root/$(basename "$nd")" --check "$f" || rc=$?
        # 0 equivalent and 3 differs both mean the committed rows reproduce.
        if ((rc != 0 && rc != 3)); then
            echo "gate: build delta $f no longer reproduces (exit $rc); the install changed" >&2
            exit 1
        fi
        echo "   $f: reproduces (exit $rc)"
    done
    exit 0
fi

dir=""; prefix=""; pinned_name=tq-offline; goon=0; dry=0; datadone=0; report=0
while (($#)); do
    case "$1" in
        --prefix) [[ $# -ge 2 ]] || die "--prefix needs a value"; prefix=$2; shift ;;
        --pinned) [[ $# -ge 2 ]] || die "--pinned needs a value"; pinned_name=$2; shift ;;
        --go-on) goon=1 ;;
        --data-done) datadone=1 ;;
        --dry-run) dry=1 ;;
        --report) report=1 ;;
        -h|--help) usage; exit 0 ;;
        -*) die "unknown argument $1" ;;
        *) [[ -z $dir ]] || die "one DIR only"; dir=$1 ;;
    esac
    shift
done
[[ -n $dir ]] || { usage >&2; exit 2; }

# Resolve DIR: a path as given, else a name under the first prefix holding it.
if [[ $dir != */* && ! -d $dir ]]; then
    found=""
    if [[ -n $prefix ]]; then [[ -d $prefix/$dir ]] && found=$prefix/$dir
    else while read -r p; do [[ -d $p/$dir ]] && { found=$p/$dir; break; }; done < <(prefixes); fi
    [[ -n $found ]] && dir=$found
fi
dir=${dir%/}
run() { if ((dry)); then echo "   (dry-run) $*"; else "$@"; fi; }
stop() { echo; echo "new-build: STOP at step $1: needs you"; shift; printf '%s\n' "$@"
         echo "new-build: then run this command again."; exit 10; }

echo "== step 1: is $dir an install"
[[ -f $dir/start.ini ]] || die "$dir has no start.ini; not a client install"
build=$(ini_get "$dir" build); line=$(ini_get "$dir" codename)
[[ $build =~ ^[0-9]+$ ]] || die "no numeric build in $dir/start.ini"
parent=$(cd "$dir/.." && pwd); name=$(basename "$dir")
pin=$(pinned_build); [[ -n $pin ]] || die "no pinned build in $registry"
echo "   build $build, codename ${line:-?}, pinned build is $pin"
[[ $build != "$pin" ]] || die "$name is the pinned build itself"
[[ -f $parent/$pinned_name/start.ini ]] || die "pinned install $parent/$pinned_name not found (use --pinned)"
[[ $(ini_get "$parent/$pinned_name" build) == "$pin" ]] || die "$pinned_name is not build $pin"

echo "== step 2: hash manifest (read-only) into out/new-build/$build/"
run mkdir -p "out/new-build/$build"
run tools/pin-client.sh --write --path "$dir" --manifest "out/new-build/$build/pin.sha256"

echo "== step 3: delta against the pinned build $pin"
delta=findings/build-delta-$pin-$build.tsv
rc=0
if ((dry)); then
    echo "   (dry-run) EVE_CLIENT_DIR=$name EVE_CLIENT=$dir $analysis python3 -I tools/build-delta.py /client-root/$pinned_name /client-root/$name --out /work/$delta"
else
    EVE_CLIENT_DIR=$name EVE_CLIENT=$dir "$analysis" python3 -I tools/build-delta.py \
        "/client-root/$pinned_name" "/client-root/$name" --out "/work/$delta" || rc=$?
fi
# triage OUTDIR [ARGS...]: run tools/build-triage.py in the analysis container, writing to OUTDIR,
# and print the tier. Exit 20-23 = T0-T3; anything else is a failure, never a pass. An absent tool
# is named. Its inputs (surface, recheck, shapes, bundle-diff) are looked for under OUTDIR only, so
# a delta-only OUTDIR can never pick up an older run's results.
triage() {
    local tool=tools/build-triage.py trc=0 od=$1 tout nskip
    shift
    if [[ ! -f $tool ]]; then echo "   triage: $tool not present"; return 0; fi
    tout=$("$analysis" python3 -I "$tool" "$build" --pinned "$pin" --delta "$delta" --out "$od" "$@" 2>&1) || trc=$?
    nskip=$(grep -c '^build-triage: skipped' <<<"$tout" || true)
    if ((trc >= 20 && trc <= 23)); then
        echo "   triage: T$((trc - 20)) (exit $trc) ($nskip inputs skipped), see $od/triage.md"
    else
        echo "   triage: FAILED (exit $trc): is $delta a format-2 delta?"
    fi
}
if ((!dry)) && [[ -f $delta ]] && ((rc == 0 || rc == 3)); then
    echo "== step 4a: triage of the delta alone, into out/new-build/$build/triage-delta (the full one follows --report)"
    triage "out/new-build/$build/triage-delta"
fi
case $rc in
    0) echo "   equivalent: $delta written; commit it" ;;
    3) echo "   DIFFERS: read the reason rows in $delta"
       ((goon)) || stop 4 "Read the 'verdict' row and the reason rows in $delta." \
         "Protocol file: re-diff blue.dll/_destiny.dll with tools/dis.py, recheck marshal opcodes." \
         "Call surface: compare findings/client-calls.tsv, client-callable.tsv, push-events.tsv, session-keys.tsv, then implement the calls (NB-5)." \
         "Schema: tools/pyd_schema.py vs docs/content/schema.md. Static data: step 7 below (NB-7)." \
         "To go on to the registry anyway (status stays unverified): add --go-on." ;;
    *) echo "new-build: delta failed (exit $rc)" >&2; exit 1 ;;
esac

echo "== step 5: loader layout and expected counts"
# `content.py`'s schema_guard raises for a build with no CFSD_BUILD_LAYOUTS
# entry, and content-lib.sh refuses an unregistered build, so both come
# BEFORE the data (step 7), not after it.
fsd=tools/fsd.py
if ! awk -v b="$build" '/^CFSD_BUILD_LAYOUTS = \{/ {on=1; next} on && /^\}/ {exit} on && $1 == b ":" {f=1} END {exit !f}' "$fsd" 2>/dev/null; then
    # What to paste: decided by the `schema` rows of the delta (class column).
    nschema=0; ndiff=0; nskip=0; nverdict=0; difftables=""; loaderwarn=""
    if [[ -f $delta ]]; then
        nverdict=$(awk -F'\t' '$1=="verdict" {n++} END {print n+0}' "$delta")
        nschema=$(awk -F'\t' '$1=="schema" {n++} END {print n+0}' "$delta")
        nskip=$(awk -F'\t' '$1=="schema" && $3=="skipped" {n++} END {print n+0}' "$delta")
        ndiff=$(awk -F'\t' '$1=="schema" && $3!="identical" && $3!="skipped" {n++} END {print n+0}' "$delta")
        difftables=$(awk -F'\t' '$1=="schema" && $3!="identical" && $3!="skipped" {printf "%s%s", (n++ ? "," : ""), $2}' "$delta")
        # reasons "loader <file> added|removed": a loader appeared or went, whatever the schema rows say
        loaderwarn=$(awk -F'\t' '$1=="verdict" {m=split($6, r, "; "); for (i=1; i<=m; i++) if (r[i] ~ /^loader .* (added|removed)$/) printf "%s%s", (n++ ? ", " : ""), r[i]}' "$delta")
    fi
    if ((nverdict == 0)); then
        layout_help=(
          "$delta is missing or has no verdict row (not written yet, or the delta stopped early), so no layout line can be chosen."
          "Rerun after the delta: no differing schema row gives '    $build: (None, CFSD_PRESENCE_FIXES),';"
          "otherwise the differing tables go to pyd_schema.py tsv --tables.")
    elif ((nskip > 0)); then
        layout_help=(
          "$nskip schema rows were not checked; rerun the delta in the analysis image ($delta has them as 'skipped').")
    elif ((ndiff > 0)); then
        layout_help=(
          "$ndiff of $nschema schema rows of $delta are not identical. Regenerate only those loaders:"
          "  tools/analysis.sh python3 tools/pyd_schema.py tsv --client /client --tables $difftables --out findings/pyd-schemas-$build.tsv"
          "Never overwrite the hand-corrected dungeons messageTextID row. Recheck every presence-bit fix by hand,"
          "then add one line to CFSD_BUILD_LAYOUTS in $fsd: $build: (\"pyd-schemas-$build.tsv\", <its fixes>),")
    else
        if ((nschema > 0)); then why="All $nschema schema rows of $delta are identical."
        else why="$delta has a verdict row and no schema rows: no loader .pyd changed."; fi
        layout_help=(
          "$why Add this line to CFSD_BUILD_LAYOUTS in $fsd, after the last entry:"
          "    $build: (None, CFSD_PRESENCE_FIXES),"
          "with a comment citing $delta.")
    fi
    if [[ -n $loaderwarn ]]; then
        layout_help+=("WARNING: the verdict names a loader added or removed ($loaderwarn): check the schema rows and presence fixes by hand before trusting the line above.")
    fi
    stop 5 "$build has no entry in fsd.CFSD_BUILD_LAYOUTS ($fsd): content.py refuses it." \
      "${layout_help[@]}" \
      "Counts: content_types.EXPECT_BY_BUILD and content_dungeons.EXPECT_BY_BUILD hold per-build row counts;" \
      "until a row is added, content.py --check NOTES an UNVERIFIED count instead of failing. After step 7" \
      "read those notes, then add the rows."
fi
echo "   build $build has a loader layout"

echo "== step 6: registry"
# Rust: the build with _ thousands separators; the line constant is the codename (V24.01 -> V24_01).
rust_build=$(echo "$build" | sed ':a;s/\([0-9]\)\([0-9]\{3\}\)\($\|_\)/\1_\2\3/;ta')
rust_line=$(echo "${line:-V??.??}" | tr . _)
rust_entry="    BuildEntry {
        build: $rust_build,
        line: &$rust_line,
        status: Status::Unverified,
        data_hash: None,
    },"
if ! awk -F'\t' -v b="$build" '!/^#/ && $1==b {f=1} END {exit !f}' "$registry"; then
    stop 6 "Add this row to $registry (status unverified until the human check):" \
      "  $build	${line:-V??.??}	unverified	-	build-delta $(date +%F): see $delta" \
      "and this entry in server/crates/eve-marshal/src/build.rs BUILDS, BEFORE the REFERENCE entry (the test the_table_matches_tools_builds_tsv checks both):" \
      "$rust_entry" \
      "A new release line (new codename) also needs a new ReleaseLine and handshake identity." \
      "Set status equivalent only when step 3 said equivalent AND the human check passed."
fi
echo "   build $build is registered"

echo "== step 7: data"
echo "   only if step 3 said static data differs. Paste these (full extraction, then its check):"
echo "   EVE_CLIENT=$dir tools/analysis.sh python3 -I /work/tools/content.py --client /client --resfiles /client-root/ResFiles --out /work/out/content-$build --build $build"
echo "   EVE_CLIENT=$dir tools/analysis.sh python3 -I /work/tools/content.py --client /client --resfiles /client-root/ResFiles --out /work/out/content-$build --build $build --check"
echo "   NEVER --sections: into a fresh --out it leaves a one-section bundle; port day needs the full extraction."
echo "   (a count change prints UNVERIFIED notes until EXPECT_BY_BUILD has the build.) Then the bundle diff:"
echo "   tools/analysis.sh python3 -I tools/bundle-diff.py out/content out/content-$build --uses --out out/new-build/$build/bundle-diff.txt"
# A `static data` reason row (resfileindex or bin64/staticdata) means the
# content bundle must be re-extracted before the build is trusted.
if ((!dry)) && [[ -f $delta ]] && grep '^verdict.*differs' "$delta" | grep -q 'static data' && ((!datadone)); then
    stop 7 "$delta has 'static data' reasons: the content data changed." \
      "Re-extract and compare the bundle (docs/guides/build-compat.md, docs/content/policy.md)," \
      "tools/analysis.sh python3 tools/content.py ... --build $build, then --check --against OLDBUNDLE." \
      "When that is done, run this command again with --data-done."
fi

echo "== step 8: patch manifest and hosts"
echo "   CONTROL COPY first: the census needs a STOCK copy of $name, made before any patch:"
echo "   apply --tier B --only B0,P0 on a copy named $name-control (tools/census.sh reads the control name); once $name is patched there is no stock copy short of revert."
if [[ ! -f tools/patches/$build.json ]]; then
    stop 8 "Locate the recipes in the new build and merge them into the pinned manifest:" \
      "  tools/analysis.sh python3 -I tools/eve_patch_author.py locate /client --merge tools/patches/$pin.json > tools/patches/$build.json" \
      "It also rehashes B0 (blue.dll) and P0 (start.ini) from the new files; it refuses (exit 2) if a B0 site window moved, a real relocation for tools/client-patch-manifest.py." \
      "Then change build, identify.start_ini_build and the comment; read the diff against $pin.json" \
      "(a refusal names the rule that did not fit its slot; B3 and the code.ccp members are the likely ones)." \
      "Verify: tools/analysis.sh python3 -I tools/eve_patch_author.py check /client --manifest tools/patches/$build.json" \
      "        and python3 -I tools/test_eve_patch.py"
fi
echo "   tools/patches/$build.json exists"
hrc=0
if ((dry)); then
    echo "   (dry-run) EVE_CLIENT_DIR=$name EVE_CLIENT=$dir $analysis python3 -I tools/eve_patch_author.py hosts /client"
else
    EVE_CLIENT_DIR=$name EVE_CLIENT=$dir "$analysis" python3 -I tools/eve_patch_author.py hosts /client || hrc=$?
    ((hrc == 0)) || stop 8 "The hosts check exited $hrc: a host not in out/census/known-hosts.txt (or no list)." \
      "Read each NEW line. An unreviewed outbound host means the jail and the patcher need a look before any client runs." \
      "Once reviewed, add it to the list (eve_patch_author.py hosts /client --write-known rewrites the whole list)."
fi

if ((report)); then
    echo "== report: one page"
    echo "   delta: $(grep '^verdict' "$delta" 2>/dev/null | cut -f3,6 | head -1 | tr '\t' ' ')"
    # T2-T4 (docs: r-port-1109 section 4). A tool that is not in the tree yet is named, not faked.
    oldpyc=${EVE_REPORT_OLD_PYC:-out/pyc}; newpyc=${EVE_REPORT_NEW_PYC:-out/new-build/$build/pyc}
    oldb=${EVE_REPORT_OLD_BUNDLE:-out/content}; newb=${EVE_REPORT_NEW_BUNDLE:-out/content-$build}
    rep() {  # NAME TOOL ARGS...
        local label=$1 tool=$2 rc=0; shift 2
        # A .py tool is run through the container, so it need not be executable (most are mode 644).
        if [[ $tool == *.py ]]; then [[ -f $tool ]] || { echo "   $label: $tool not present"; return; }
        elif [[ ! -x $tool ]]; then echo "   $label: $tool not present"; return; fi
        # A Python tool runs in the analysis container, like every other one here.
        if [[ $tool == *.py ]]; then "$analysis" python3 -I "$tool" "$@" >/dev/null 2>&1 || rc=$?
        else "$tool" "$@" >/dev/null 2>&1 || rc=$?; fi
        echo "   $label: exit $rc ($tool $*)"
    }
    if ((dry)); then echo "   (dry-run) surface, bundle, recheck, shapes, fci-surface, content-report and triage are not run"
    else
        if [[ -d $oldpyc && -d $newpyc ]]; then rep surface tools/port-surface.sh "$oldpyc" "$newpyc"
        else echo "   surface: $oldpyc or $newpyc missing"; fi
        if [[ -d $oldb && -d $newb ]]; then rep bundle tools/bundle-diff.py "$oldb" "$newb" --uses --out "out/new-build/$build/bundle-diff.txt"
        else echo "   bundle: $oldb or $newb missing"; fi
        recheck=("$oldpyc" "$newpyc")
        [[ -d $oldb && -d $newb ]] && recheck+=(--old-bundle "$oldb" --new-bundle "$newb")
        if [[ -f tools/port-recheck.py && ! ( -d $oldpyc && -d $newpyc ) ]]; then echo "   recheck: $oldpyc or $newpyc missing"
        else rep recheck tools/port-recheck.py "${recheck[@]}"; fi
        # N1. The answer shapes were run once, as port-surface's step G7 (surface/shapes/): read, do not rerun.
        shp=out/new-build/$build/surface/shapes/shapes.tsv
        if [[ -f $shp ]]; then echo "   shapes: from G7, $(awk -F'\t' '$2 ~ /^SERVED/ {n++} END {print n+0}' "$shp") SERVED row(s) ($shp)"
        else echo "   shapes: no $shp (port-surface did not run or G7 failed)"; fi
        if [[ -d $oldpyc && -d $newpyc ]]; then rep fci-surface tools/client-ci/fci-surface.py "$oldpyc" "$newpyc"
        else echo "   fci-surface: $oldpyc or $newpyc missing"; fi
        if [[ -d $oldb && -d $newb ]]; then rep content-report tools/content-report.py "$oldb" "$newb" --uses
        else echo "   content-report: $oldb or $newb missing"; fi
        # The patch rule check: locate refuses with eve_patch_author.EXIT_REFUSED (4); triage reads it as T3.
        targs=()
        if [[ -f tools/eve_patch_author.py && -f tools/patches/$pin.json ]]; then
            lrc=0
            EVE_CLIENT_DIR=$name EVE_CLIENT=$dir "$analysis" python3 -I tools/eve_patch_author.py locate /client \
                --merge "tools/patches/$pin.json" > "out/new-build/$build/locate.json" 2>/dev/null || lrc=$?
            echo "   locate: exit $lrc (tools/eve_patch_author.py locate; 4 = a rule refused)"
            targs+=(--locate-exit "$lrc")
        else echo "   locate: tools/eve_patch_author.py or tools/patches/$pin.json not present"; fi
        echo "   tier (all inputs present are used; the others are named skipped in triage.md):"
        triage "out/new-build/$build" ${targs[@]+"${targs[@]}"}
    fi
fi

echo "== step 9: gate"
echo "   EVE_MAX_JOBS=6 tools/dev.sh cargo test --locked --workspace"
echo "   EVE_MAX_JOBS=6 tools/dev.sh cargo clippy --locked --workspace --all-targets -- -D warnings"
echo "   then tools/land.sh --head"
echo "new-build: all steps done for build $build"
