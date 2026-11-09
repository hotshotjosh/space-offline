#!/usr/bin/env bash
# Pre-push gate. There is no CI runner: if this script does not run a check,
# nothing does. Cheapest checks first; stops at the first failure.
#
#   tools/gate.sh                     # the full gate; the only form that
#                                     # authorises a push
#   tools/gate.sh --dirty             # gate what is on disk, tree as it is
#   tools/gate.sh --only CRATE        # scope fmt/clippy/test to one package
#   tools/gate.sh --without C1,C2     # run in a COPY with those crates removed
#
# **A docs-only push skips the build.** When everything since the base branch is
# Markdown outside tools/ and server/, or under private/, only the docs tests run:
# test_docs, test_status_page, test_plan_owners, test_client_kit,
# test_fci_scenarios and test_public_export (rule 17 of docs/lanes/README.md: a hand edit of the
# generated status page fails the gate). Any other path in the range gets the
# full gate.
# **A tools-only push skips it too.** Markdown, private/ and tools/*.py or tools/*.sh
# scripts (at least one) run the allowlist, build deltas, every tools test and bash -n;
# gate.sh, land.sh, dev.sh, analysis.sh, check_deps.py, status-guard.py, test-parallel.sh,
# gate_reds.py and tools/hooks/ keep the full gate.
#
# **Each stage prints `   took Ns`** after its last line, so a slow gate can be
# read off its log. The `== ` lines are what land.sh and pre-push grep; the
# timing lines are indented and never change them.
#
# **Why the last three exist** (`docs/port-tracker.md` §29.6). The clean-tree
# check is right for a push and impossible while several research lanes are
# writing in the tree, so a coordinator used to copy the tree by hand, strip
# the lanes' half-written crates, and run the other four checks in the copy.
# `--without` is that workaround, automated: `members = ["crates/*"]` means one
# crate with a `Cargo.toml` and no `main.rs` fails the whole workspace
# manifest, and that blocks every other lane's build.
#
# **A partial run cannot authorise a push, by construction.** Only the full
# gate prints `== gate passed`, which is the exact line `tools/land.sh` greps
# for; every scoped form prints `== partial gate passed` instead. So there is
# no flag combination that makes `land.sh` push on an incomplete check -- the
# §54.6 lesson applied forward rather than after the fact.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

dirty=0
only=""
without=""
print_jobs=0
while (($#)); do
    case "$1" in
        --dirty) dirty=1 ;;
        --only) only=$2; dirty=1; shift ;;
        --without) without=$2; dirty=1; shift ;;
        --print-test-jobs) print_jobs=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d;s/^# \{0,1\}//'; exit 0 ;;
        *) echo "gate: unknown argument $1" >&2; exit 2 ;;
    esac
    shift
done

# `--without` re-execs the copy's own gate, so everything below runs once, in
# the tree it is actually checking. `tools/dev.sh` derives the mount from its
# own path, so the copy's checks see the copy.
if [[ -n $without ]]; then
    work=$(mktemp -d -t eve-gate-XXXXXX)
    trap 'rm -rf "$work"' EXIT
    echo "== copying the tree to $work (excluding: $without)"
    tar -cf - --exclude=server/target --exclude=.git --exclude=out . | (cd "$work" && tar -xf -)
    IFS=, read -ra drop <<<"$without"
    for c in "${drop[@]}"; do
        c=${c// /}
        [[ -n $c ]] || continue
        if [[ -d $work/server/crates/$c ]]; then
            rm -rf "$work/server/crates/$c"
            echo "   removed crate $c"
        else
            echo "gate: no such crate: $c" >&2; exit 2
        fi
    done
    exec "$work/tools/gate.sh" --dirty
fi

# Stage timing: `took` closes a stage, measured from the previous call.
stage_t0=$SECONDS
took() {
    echo "   took $((SECONDS - stage_t0))s"
    stage_t0=$SECONDS
}

# **Python tests run up to 8 at once** (r-gate report section 4, step B). Each
# script writes its output and exit code to a file in a temp dir; the report
# prints in the order given, and the gate fails after all have finished,
# naming every script that failed. Isolated interpreter (`-I`), as before. The
# tests use temp dirs, a private mount namespace (test_jail) or ephemeral
# state only, with no fixed port or shared file, so they can run side by side.
# A named script that does not exist is a failure, never a skip.
run_python_tests() {
    local pydir t i=0 failed=() s
    pydir=$(mktemp -d -t eve-gate-py-XXXXXX)
    for t in "$@"; do
        while (($(jobs -rp | wc -l) >= 8)); do wait -n || true; done
        (
            s=0
            python3 -I "$t" -q >"$pydir/$i.out" 2>&1 || s=$?
            echo "$s" >"$pydir/$i.rc"
        ) &
        i=$((i + 1))
    done
    wait
    i=0
    for t in "$@"; do
        s=$(cat "$pydir/$i.rc" 2>/dev/null || echo 1)
        if ((s != 0)); then
            cat "$pydir/$i.out" 2>/dev/null || true
            failed+=("$t")
        else
            echo "   $t: $(tail -1 "$pydir/$i.out")"
        fi
        i=$((i + 1))
    done
    rm -rf "$pydir"
    if ((${#failed[@]})); then
        echo "gate: ${#failed[@]} python test(s) failed:" >&2
        for t in "${failed[@]}"; do echo "gate: $t failed" >&2; done
        exit 1
    fi
}

# **The progress table moves with the code** (docs-autopilot, 2026-11-03). A
# change under server/, content/ or tools/client-ci/scenarios/ must change
# docs/status/progress.tsv, or say `Status: none (<reason>)` in the head commit
# message; `--audit` then fails on a row the newest regress verdicts contradict,
# and on a scenario no row cites. Without var/ the audit skips with a note.
# The branch a change is measured from: the remote's default (HEAD), else main, else
# master, of `upstream` if that remote exists (a fork), then of `origin`. A clone of the
# public repo has main; the maintainer tree has master; both have origin/HEAD.
default_base() {
    local r
    for r in refs/remotes/upstream/HEAD refs/remotes/upstream/main refs/remotes/upstream/master \
             refs/remotes/origin/HEAD refs/remotes/origin/main refs/remotes/origin/master; do
        git rev-parse -q --verify "$r" && return 0
    done
    return 1
}

status_checks() {
    echo "== feature status"
    # Each check runs only where its tool exists (a tree without them, such as
    # the gate's own test fixture, has nothing to check).
    if [[ -e tools/status-guard.py ]]; then
        if base=$(default_base); then
            python3 -I tools/status-guard.py --base "$base"
        else
            echo "   status-guard skipped: no upstream or origin HEAD, main or master to measure from"
        fi
    fi
    [[ -e tools/status-page.py ]] && python3 -I tools/status-page.py --audit --no-stamp
    # every flag and EVE_* variable has its row in docs/reference/launch-and-env.md
    [[ -e tools/env-ref.py ]] && python3 -I tools/env-ref.py --check
    took
}

scope=(--workspace)
[[ -n $only ]] && scope=(-p "$only")

# **The workspace's tests run as four concurrent jobs** (gate-speed lane): eve-bot's
# socket tests, its other targets, its doctests, and every other package. The split
# is decided here, from the tree: eve-bot's integration targets are `tests/*.rs`
# (the file's name) and `tests/*/main.rs` (the directory's name), so a new target
# lands in the second job by itself and is never skipped (a hard-coded "only
# tests/bots" check once fell back to the serial run unnoticed, a24bbb06). Anything
# the enumeration cannot cover (examples, benches, a scoped run) runs serially and
# says why. EVE_GATE_SERIAL_TESTS=1 forces the serial run; EVE_BOT_TEST_THREADS sets
# the socket job's --test-threads. Sets `jobs_mode` (split|serial), `jobs_why`, `jobs`.
jobs_mode=serial
jobs_why=""
jobs=()
test_jobs() {
    local bot=server/crates/eve-bot t n others="" bots_job
    jobs=()
    if [[ ${scope[*]} != --workspace ]]; then
        jobs_why="scoped run"; return 0
    elif [[ -n ${EVE_GATE_SERIAL_TESTS:-} ]]; then
        jobs_why="EVE_GATE_SERIAL_TESTS set"; return 0
    elif [[ ! -d $bot/tests/bots ]]; then
        jobs_why="no eve-bot tests/bots"; return 0
    elif [[ -e $bot/examples ]]; then
        jobs_why="eve-bot has examples"; return 0
    elif [[ -e $bot/benches ]]; then
        jobs_why="eve-bot has benches"; return 0
    elif grep -qE '^\s*(\[\[(test|bench|example)\]\]|\[lib\]|autotests|autobenches|autoexamples)' "$bot/Cargo.toml" 2>/dev/null; then
        jobs_why="eve-bot declares targets in Cargo.toml"; return 0
    fi
    for t in "$bot"/tests/*.rs "$bot"/tests/*/main.rs; do
        [[ -e $t ]] || continue
        if [[ $t == */main.rs ]]; then n=$(basename "$(dirname "$t")"); else n=$(basename "$t" .rs); fi
        [[ $n == bots ]] || others+=" --test $n"
    done
    bots_job="-p eve-bot --test bots"
    [[ -n ${EVE_BOT_TEST_THREADS:-} ]] && bots_job+=" -- --test-threads=$EVE_BOT_TEST_THREADS"
    jobs=("$bots_job" "-p eve-bot --lib --bins$others" "-p eve-bot --doc" "--workspace --exclude eve-bot")
    jobs_mode=split
    jobs_why=${#jobs[@]}
}
test_jobs
if ((print_jobs)); then
    echo "   test jobs: $jobs_mode ($jobs_why)"
    for t in "${jobs[@]}"; do echo "   job: $t"; done
    exit 0
fi

if ((dirty)); then
    echo "== tree hygiene SKIPPED (scoped run; cannot authorise a push)"
else
    echo "== tree hygiene (the gate tests what is on disk, not what is committed)"
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "gate: working tree is dirty; commit or stash first" >&2
        git status --short >&2
        exit 1
    fi
fi
took

if ((!dirty)) && base=$(default_base); then
    mapfile -t changed < <(git diff --name-only --no-renames "$base" HEAD)
    # One classification for both light tiers. docs_only: every file is Markdown or
    # private/. tools_only: also tools scripts (*.py, *.sh) that cannot affect a Rust
    # build, at least one of them. Anything else keeps the full gate.
    docs_only=$((${#changed[@]} > 0))
    tools_only=$docs_only
    tool_scripts=0
    for f in "${changed[@]}"; do
        case $f in
            # schema.md is read by code: mutator_data.rs include_str!s it and the
            # tools/test_content_*.py tests parse it, so it gets the full gate.
            docs/content/schema.md|server/*) docs_only=0; tools_only=0 ;;
            # The gate's own scripts, the build wrappers, the allowlist check, the
            # status guard and the hooks decide or run the build: full gate.
            tools/gate.sh|tools/land.sh|tools/dev.sh|tools/analysis.sh|tools/check_deps.py|tools/status-guard.py|tools/test-parallel.sh|tools/gate_reds.py|tools/hooks/*)
                docs_only=0; tools_only=0 ;;
            # Markdown under tools/ keeps the full gate, as before the tools tier.
            tools/*.md) docs_only=0; tools_only=0 ;;
            # Markdown anywhere (content/*.md included: the content build reads only
            # its .tsv tables) and the maintainer folder, which is never built or exported.
            *.md|private/*) ;;
            # A tools script: no Rust build reads it.
            tools/*.py|tools/*.sh) docs_only=0; tool_scripts=$((tool_scripts + 1)) ;;
            *) docs_only=0; tools_only=0 ;;
        esac
    done
    ((tool_scripts > 0)) || tools_only=0
    if ((tools_only)); then
        echo "== tools only (${#changed[@]} file(s)): no Rust build reads these; tools tests"
        echo "== dependency allowlist"
        python3 tools/check_deps.py
        took
        echo "== build deltas"
        if [[ -x tools/new-build.sh ]]; then
            tools/new-build.sh --gate-check
        else
            echo "   skipped: no tools/new-build.sh in this tree"
        fi
        took
        tool_tests=()
        for t in tools/test_*.py private/test_*.py; do
            [[ -e $t ]] && tool_tests+=("$t")
        done
        run_python_tests "${tool_tests[@]}"
        for s in tools/*.sh private/*.sh; do
            [[ -e $s ]] || continue
            bash -n "$s" || { echo "gate: $s fails bash -n" >&2; exit 1; }
        done
        took
        status_checks
        echo "== gate passed"
        exit 0
    fi
    if ((docs_only)); then
        echo "== docs only (${#changed[@]} file(s)): the docs tests"
        # A test the (public) tree does not hold is skipped here; a private doc is absent there.
        docs_tests=()
        for t in tools/test_docs.py tools/test_status_page.py tools/test_plan_owners.py \
            tools/test_client_kit.py tools/test_fci_scenarios.py tools/test_public_export.py; do
            [[ -e $t ]] && docs_tests+=("$t")
        done
        run_python_tests "${docs_tests[@]}"
        # private/post-land.sh runs after every landing; a syntax error there must not land.
        for s in private/*.sh; do
            [[ -e $s ]] || continue
            bash -n "$s" || { echo "gate: $s fails bash -n" >&2; exit 1; }
        done
        took
        status_checks
        echo "== gate passed"
        exit 0
    fi
fi

if ((!dirty)); then
    status_checks
fi

echo "== dependency allowlist"
python3 tools/check_deps.py
took

# **The extractors' unit tests** (2026-10-25, content-plan review R1 #1).
# Every `tools/test_*.py` runs on synthetic bytes only, never the client --
# the gate has no client -- so a broken extractor fails here instead of on
# the owner's next launch. Stdlib unittest, isolated interpreter.
echo "== python unit tests"
pytests=()
for t in tools/test_*.py; do
    [[ -e $t ]] && pytests+=("$t")
done
run_python_tests "${pytests[@]}"
took

# **The committed build deltas still reproduce** (R-NEWBUILD NB-1). For each
# findings/build-delta-<old>-<new>.tsv whose two installs exist on this
# machine, build-delta.py --check re-measures them (read-only, no network, the
# client never launched), so a changed install is caught. No install: skipped
# with a note. The logic lives in tools/new-build.sh --gate-check.
echo "== build deltas"
if [[ -x tools/new-build.sh ]]; then
    tools/new-build.sh --gate-check
else
    echo "   skipped: no tools/new-build.sh in this tree"
fi
took

# **Every shell script parses** (review V4 G3). A typo in `client-ci.sh` or
# `up.sh` used to surface at the next unattended run, minutes in. `bash -n`
# only, not shellcheck: shellcheck is a new image dependency.
echo "== shell syntax"
for s in tools/*.sh tools/hooks/*; do
    [[ -f $s ]] || continue
    bash -n "$s" || { echo "gate: $s has a syntax error" >&2; exit 1; }
done
took

# **Which compiler gated this** (review V6 F12). The build image's apt step is
# not pinned, so a rebuild can move rustc; the image records `rustc -V` in
# this file, and the gate log carries it. Informational: an image without the
# record (built before F12) prints a note and the gate goes on.
echo "== toolchain"
tools/dev.sh sh -c 'cat /etc/eve-offline-toolchain.txt 2>/dev/null || echo "(the build image has no toolchain record)"' \
    || echo "(could not read the build image's toolchain record)"
took

echo "== format"
tools/dev.sh cargo fmt --all --check
took

echo "== lint"
tools/dev.sh cargo clippy --locked "${scope[@]}" --all-targets -- -D warnings
took

echo "== test"
# **Captured with `|| status=$?`, never bare.** Under `set -e` a failing
# command substitution in an assignment exits on that line, before the
# `echo` below -- so every red test run printed `== test` and nothing else,
# and `land.sh` reported a bare `exit 101`. That hid a disk-full and then a
# real failing test (`docs/port-tracker.md` §64, §65).
#
status=0
echo "   test jobs: $jobs_mode ($jobs_why)"
if [[ $jobs_mode == split ]]; then
    # Built once first, in the same container (one memory limit), so a compile
    # error prints once.
    out="$(tools/dev.sh bash -c 'cargo test --locked --workspace --no-run && exec "$@"' sh \
        /repo/tools/test-parallel.sh "${jobs[@]}" 2>&1)" || status=$?
else
    out="$(tools/dev.sh cargo test --locked --no-fail-fast "${scope[@]}" 2>&1)" || status=$?
fi
echo "$out"
took
if ((status != 0)); then
    echo "gate: tests failed (exit $status)" >&2
    exit "$status"
fi
# cargo exits 0 when a crate has no tests; assert at least one actually ran.
#
# Matched with bash's own `=~` and NOT with `echo "$out" | grep -q`. That
# pipeline was here, and it was a trap that fires on success: `grep -q` exits
# the moment it matches, `echo` then takes SIGPIPE, and `set -o pipefail`
# turns the whole pipeline non-zero -- so the gate failed *because* the tests
# passed. It only started firing once the output outgrew the 64 KiB pipe
# buffer, which is why it sat here harmless for weeks and then appeared out of
# nowhere, in a run where nothing about the tests had changed. A gate that
# cries wolf is worse than no gate: the next person's instinct is to go and
# "fix" a test that was never broken.
ran='test result: ok\. [1-9][0-9]* passed'
[[ $out =~ $ran ]] || { echo "gate: no tests actually ran" >&2; exit 1; }

if ((dirty)); then
    echo "== partial gate passed"
    echo "gate: scoped run -- NOT a push authorisation; run tools/gate.sh bare"
else
    echo "== gate passed"
fi
