#!/usr/bin/env bash
# The exported tree, run as a contributor gets it (#292).
#
#   tools/export-parity.sh            export HEAD to a scratch dir under out/, then run the
#                                     exported tree's static stages and tools tests in the
#                                     contrib container (python 3.11, stdlib only, hostile git
#                                     config, non-root, no network, empty HOME)
#   tools/export-parity.sh --image    build that image (the one networked step)
#
# Runs only where private/export-private-paths.txt exists (the maintainer's tree and the
# lanes' build copies); on the export itself it prints a skip. A red here is a red the
# public gate would show. Scratch is on disk (EVE_PARITY_DIR, default out/parity), never
# /tmp: an export is about 70 MB, twice.
#   --inside / --run-tests DIR are internal: the container's half, and the test runner.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="eve-offline/contrib:latest"

# Every tools/test_*.py in DIR, up to 8 at once, as gate.sh's run_python_tests does (copied:
# that function cannot be sourced without running the gate).
run_tests() {
    local dir=$1 pydir t i=0 s failed=() tests=()
    pydir=$(mktemp -d)
    for t in "$dir"/tools/test_*.py; do
        [[ -f $t ]] && tests+=("tools/$(basename "$t")")
    done
    for t in "${tests[@]}"; do
        while (($(jobs -rp | wc -l) >= 8)); do wait -n || true; done
        (
            s=0
            cd "$dir" && timeout 900 python3 -I "$t" -q >"$pydir/$i.out" 2>&1 || s=$?
            echo "$s" >"$pydir/$i.rc"
        ) &
        i=$((i + 1))
    done
    wait
    i=0
    for t in "${tests[@]}"; do
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
        echo "export-parity: ${#failed[@]} python test(s) failed:" >&2
        for t in "${failed[@]}"; do echo "export-parity: $t failed" >&2; done
        return 1
    fi
}

case "${1:-}" in
--image)
    cd "$REPO"
    exec docker build --network default -t "$IMAGE" -f containers/contrib.Dockerfile containers
    ;;
--run-tests)
    run_tests "${2:?usage: --run-tests DIR}"
    exit $?
    ;;
--inside)
    [[ -f /.dockerenv && -f /etc/contrib-gitconfig && ${HOME:-} == /home/contrib && $PWD == /repo ]] || {
        echo "export-parity: --inside runs only in the contrib container" >&2
        exit 2
    }
    cp /etc/contrib-gitconfig "$HOME/.gitconfig"
    if python3 -c 'import google.protobuf' 2>/dev/null; then pb=no; else pb=yes; fi
    echo "== export parity: python $(python3 -c 'import sys; print("%d.%d" % sys.version_info[:2])'), git $(git --version | cut -d' ' -f3), google.protobuf absent: $pb"
    stage() {
        local name=$1
        shift
        echo "-- $name"
        "$@" || {
            echo "export-parity: stage failed: $name" >&2
            exit 1
        }
    }
    stage "dependency allowlist" python3 tools/check_deps.py
    stage "status guard" python3 -I tools/status-guard.py --base refs/remotes/origin/HEAD
    stage "status audit" python3 -I tools/status-page.py --audit --no-stamp --since refs/remotes/origin/HEAD
    stage "env reference" python3 -I tools/env-ref.py --check
    for f in tools/*.sh tools/hooks/*; do
        [[ -f $f ]] || continue
        stage "shell syntax $f" bash -n "$f"
    done
    run_tests "$PWD" || exit 1
    echo "== export parity passed"
    exit 0
    ;;
"") ;;
*)
    echo "usage: tools/export-parity.sh [--image]" >&2
    exit 2
    ;;
esac

cd "$REPO"
[[ -f private/export-private-paths.txt ]] || {
    echo "   skipped: this tree has no private list (it is the export)"
    exit 0
}
docker info >/dev/null 2>&1 || {
    echo "export-parity: docker daemon unreachable: start Docker (or fix the socket permissions) and rerun" >&2
    exit 2
}
docker image inspect "$IMAGE" >/dev/null 2>&1 || {
    echo "export-parity: image eve-offline/contrib missing: run tools/export-parity.sh --image" >&2
    exit 2
}
root=${EVE_PARITY_DIR:-$PWD/out/parity}
mkdir -p "$root"
w=$(mktemp -d -p "$root" run-XXXXXX)
trap 'chmod -R u+w "$w" 2>/dev/null; rm -rf "$w"' EXIT
mkdir "$w/tree"
git archive HEAD | tar -x -C "$w/tree"
# The same path as tools/publish-github.py: the archived tree is a repo with everything added,
# ignored files included, before the export reads it.
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$w/tree" init -q
GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$w/tree" add -f -A
python3 -I tools/public-export.py --out "$w/export" --root "$w/tree" >"$w/export.log" 2>&1 || {
    cat "$w/export.log" >&2
    exit 1
}
# A fresh clone's shape, with the maintainer's own git config out of it.
export_git() { GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 git -C "$w/export" "$@"; }
export_git init -q -b main
export_git add -A
export_git -c user.name=export -c user.email=export@example.invalid -c commit.gpgsign=false commit -qm export
export_git update-ref refs/remotes/origin/main HEAD
export_git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/main
# TMPDIR is a directory on disk, mounted outside the repo (a test refuses a scratch dir inside a git tree; tests that want gigabytes of scratch
# must not fill RAM); /tmp stays a small tmpfs.
mkdir -p "$w/scratch"
# `exec` is explicit on both tmpfs mounts: Docker's default is noexec.
s=0
docker run --rm --network none --memory 6g --pids-limit 4096 --cap-drop ALL \
    --security-opt no-new-privileges -u "$(id -u):$(id -g)" \
    --tmpfs /tmp:rw,exec,size=2g --tmpfs /home/contrib:rw,exec,mode=1777 \
    -e HOME=/home/contrib -e TMPDIR=/scratch -v "$w/scratch:/scratch" -v "$w/export:/repo" \
    -v "$w/export/containers/contrib-gitconfig:/etc/contrib-gitconfig:ro" -w /repo \
    "$IMAGE" bash tools/export-parity.sh --inside || s=$?
exit "$s"
