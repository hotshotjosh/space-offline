#!/usr/bin/env python3
"""Is the gate's red also red on an untouched main? Prints the block for the PR.

    python3 -I tools/red-on-main.py [--log FILE] [--base REF] [--repo DIR]

Reads the failed gate log (`--log`, else the log of the last red row in
`var/gate-reds.tsv`, the file `tools/land.sh` writes). It collects every failed Python
test (`gate: tools/test_X.py failed`), checks out `--base` (default: the remote's main,
as `tools/status-guard.py` finds it) into a throwaway worktree under `out/`, and runs
each failed test there and on your branch with `python3 -I`. Cargo failures are not
rerun: they are printed as `red: <test> (cargo; rerun by hand on main)` and the verdict
stays unproven. The worktree is removed on every path.

Output (paste it under the template's Gate section):

    Gate: red-on-main-too
    red: tools/test_X.py: branch exit 1, main exit 1

Other verdicts: `Gate: red-caused-by-this-change` (a test passes on main) and
`Gate: red-unproven` (a red that cannot be rerun here). Exit 0 only for
`red-on-main-too`; 1 for the others; 2 when git or the log cannot be read.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins (#126).
import sys as _sys

_sys.path.append(_sys.path.pop(0))

import argparse
import importlib.util
import os
import shutil
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_ROOT = os.path.dirname(HERE)
TEST_TIMEOUT = 900


def load_gate_reds():
    spec = importlib.util.spec_from_file_location("gate_reds", os.path.join(HERE, "gate_reds.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def git(repo: str, *args: str) -> str:
    return subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True, text=True).stdout


def default_base(repo: str) -> str | None:
    for ref in [f"refs/remotes/{r}/{b}" for r in ("upstream", "origin") for b in ("HEAD", "main", "master")]:
        if subprocess.run(["git", "-C", repo, "rev-parse", "-q", "--verify", ref], capture_output=True).returncode == 0:
            return ref
    return None


def last_red_log(reds, repo: str) -> str | None:
    rows = reds.read_rows(os.path.join(repo, "var", "gate-reds.tsv"))
    for row in reversed(rows):
        if row[3] == "red":
            return row[-1]
    return None


def failures(reds, text: str) -> tuple[list[str], list[str]]:
    """(python test files, other reds as readable names), in log order, each once."""
    _tier, parsed = reds.parse(text)
    py, other = [], []
    for _stage, kind, target, test in parsed:
        if kind == "python":
            if target not in py:
                py.append(target)
        else:
            name = test if test not in ("-", "") else f"{kind} {target}"
            if kind == "test" and target != "-":
                name = f"{target} {test}"
            if name not in other:
                other.append(name)
    return py, other


def run_test(cwd: str, rel: str) -> int:
    try:
        return subprocess.run([sys.executable, "-I", rel], cwd=cwd, capture_output=True, timeout=TEST_TIMEOUT).returncode
    except subprocess.TimeoutExpired:
        return 124


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Is the gate's red also red on an untouched main?")
    ap.add_argument("--log")
    ap.add_argument("--base")
    ap.add_argument("--repo", default=DEFAULT_ROOT)
    a = ap.parse_args(argv)
    repo = os.path.abspath(a.repo)
    reds = load_gate_reds()
    log = a.log or last_red_log(reds, repo)
    if not log:
        print("red-on-main: no gate log: pass --log FILE (the log tools/land.sh names)", file=sys.stderr)
        return 2
    try:
        with open(log, encoding="utf-8", errors="replace") as f:
            text = f.read()
    except OSError as e:
        print(f"red-on-main: cannot read {log}: {e}", file=sys.stderr)
        return 2
    py, other = failures(reds, text)
    if not py and not other:
        print("red-on-main: the log holds no failure", file=sys.stderr)
        return 2
    base = a.base or default_base(repo)
    if not base:
        print("red-on-main: no upstream/main to compare with; pass --base REF", file=sys.stderr)
        return 2
    wt = os.path.join(repo, "out", "red-on-main-wt")
    results = []
    try:
        if py:
            os.makedirs(os.path.dirname(wt), exist_ok=True)
            if os.path.exists(wt):
                subprocess.run(["git", "-C", repo, "worktree", "remove", "--force", wt], capture_output=True)
                shutil.rmtree(wt, ignore_errors=True)
            git(repo, "worktree", "add", "--detach", "-q", wt, base)
            for rel in py:
                if os.path.isfile(os.path.join(wt, rel)):
                    results.append((rel, run_test(repo, rel), run_test(wt, rel)))
                else:
                    results.append((rel, run_test(repo, rel), None))  # new in this branch: not on main
    except subprocess.CalledProcessError as e:
        print(f"red-on-main: git failed: {(e.stderr or '').strip()}", file=sys.stderr)
        return 2
    finally:
        if os.path.exists(wt):
            subprocess.run(["git", "-C", repo, "worktree", "remove", "--force", wt], capture_output=True)
            shutil.rmtree(wt, ignore_errors=True)
            subprocess.run(["git", "-C", repo, "worktree", "prune"], capture_output=True)
    lines = []
    caused = unproven = False
    for rel, b, m in results:
        if m is None:
            lines.append(f"red: {rel}: branch exit {b}, absent on main (this change adds it)")
            caused = True
        elif m == 0 and b == 0:
            lines.append(f"red: {rel}: passes on the branch and on main now (flaky or environment; rerun the gate)")
            unproven = True
        elif m == 0:
            lines.append(f"red: {rel}: branch exit {b}, main exit 0")
            caused = True
        else:
            lines.append(f"red: {rel}: branch exit {b}, main exit {m}")
    for name in other:
        lines.append(f"red: {name} (cargo; rerun by hand on main)")
    if caused:
        verdict, code = "red-caused-by-this-change", 1
    elif other or unproven:
        verdict, code = "red-unproven", 1
    else:
        verdict, code = "red-on-main-too", 0
    print(f"Gate: {verdict}")
    print("\n".join(lines))
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
