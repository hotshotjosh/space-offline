#!/usr/bin/env bash
# Say what is wrong with this host before the gate breaks on it (#292 class).
#
#   tools/host-check.sh
#
# Checks, in order, and exits 1 on the first certain failure:
#   python3 3.11 or newer; the checkout is not on a network or Windows
#   filesystem; no file is checked out with CRLF; the temp directory can run
#   a script (a noexec /tmp breaks the tests). Too little RAM for a build is
#   only a warning. Standard library and coreutils only; changes nothing.
set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 2

fail() { echo "host-check: $1" >&2; exit 1; }

echo "== host"

if ! python3 -c 'import sys; sys.exit(sys.version_info < (3, 11))' 2>/dev/null; then
    v=$(python3 -c 'import sys; print("%d.%d" % sys.version_info[:2])' 2>/dev/null || echo "missing")
    fail "python3 is $v; the gate needs 3.11 or newer (docs/guides/getting-started.md section 1)"
fi
echo "   python3 ok"

fstype=$(findmnt -n -o FSTYPE -T . 2>/dev/null | head -n 1)
case " 9p v9fs drvfs fuse.sshfs cifs smb3 nfs nfs4 " in
    *" $fstype "*) fail "this checkout is on a $fstype filesystem; clone it inside the Linux filesystem (~/...)" ;;
esac
echo "   filesystem ok (${fstype:-unknown})"

if git rev-parse --git-dir >/dev/null 2>&1; then
    crlf=$(git ls-files --eol 2>/dev/null | awk '$2 ~ /^w\/(crlf|mixed)$/' | wc -l)
    if ((crlf > 0)); then
        fail "$crlf files are checked out with CRLF; run \`git config core.autocrlf false\` and re-checkout"
    fi
    echo "   line endings ok"
fi

t=$(mktemp -d) || fail "cannot make a temp directory; set TMPDIR to a directory on disk"
printf '#!/bin/sh\nexit 0\n' >"$t/probe.sh"
chmod +x "$t/probe.sh"
if ! "$t/probe.sh" 2>/dev/null; then
    rm -rf "$t"
    fail "the temp directory is noexec; set TMPDIR to a directory on disk"
fi
rm -rf "$t"
echo "   temp directory runs scripts"

want_gb=${EVE_MIN_FREE_GB:-12}
total_kb=$(awk '/^MemTotal:/ {print $2}' "${EVE_MEMINFO:-/proc/meminfo}" 2>/dev/null)
if [[ -n $total_kb ]] && ((total_kb < want_gb * 1024 * 1024)); then
    echo "   warning: $((total_kb / 1048576)) GB of RAM in all; a full build wants about $want_gb GB (on WSL2 raise memory= in .wslconfig)"
fi
echo "== host ok"
