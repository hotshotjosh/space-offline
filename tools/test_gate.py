"""Tests for the landing tools: `tools/gate.sh`, `tools/land.sh` and
`tools/hooks/pre-push` (review V4 G1-G3, RR1 L2; packet IP16).

**Why this file exists.** Every guard-script bug so far was found in
production, on a failure path nobody had watched: `;` for `&&`
(`docs/port-tracker.md` §48.5), a pipeline's status hidden by `tail`
(§54.6), `grep -q` + SIGPIPE + `pipefail` failing a green gate (the comment
in `gate.sh`), and a red test run that printed nothing (§64, §65). Each test
here drives one of those paths on purpose.

**Nothing real is touched.** Each test copies the three scripts into a
fresh temp git repo with a stub `tools/dev.sh` (cargo answers from a file;
anything else runs on the host) and a stub `tools/check_deps.py`, and pushes
to a bare repo in the same temp dir. The real `tools/land.sh` never runs,
and no real remote is ever named. `GIT_*` from the caller is scrubbed and
the user's git config is ignored, so the gate's own pre-push hook can run
this file.

Run::

    python3 -I tools/test_gate.py
"""

from __future__ import annotations

import hashlib
import os
import shutil
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

STUB_DEV = """#!/usr/bin/env bash
# Stub of tools/dev.sh for tools/test_gate.py: no container.
for a in "$@"; do [[ $a == --no-run ]] && exit 0; done
case "${1:-} ${2:-}" in
    "cargo test") cat "$STUB_TEST_OUT"; exit "${STUB_TEST_EXIT:-0}" ;;
    "cargo fmt") exit "${STUB_FMT_EXIT:-0}" ;;
    "cargo "*) exit 0 ;;
    # the registry check runs in the image (/repo is the mount); here it is answered
    "python3 -I") exit "${STUB_REGISTRY_EXIT:-0}" ;;
    "bash "*)
        printf '%s\n' "$*" >>"${STUB_ARGS:-/dev/null}"
        if [[ " $* " == *" --exact "* ]]; then
            # the flaky rerun (step 5): one call, answered from the rerun file
            printf 'rerun\n' >>"${STUB_RERUN_COUNT:-/dev/null}"
            cat "${STUB_RERUN_OUT:-/dev/null}"; exit "${STUB_RERUN_EXIT:-0}"
        fi
        cat "$STUB_TEST_OUT"; exit "${STUB_TEST_EXIT:-0}" ;;
    *) exec "$@" ;;
esac
"""

STUB_GATE_NO_PASS_LINE = """#!/usr/bin/env bash
# A gate that exits 0 without its pass line (one that stopped running a step).
echo "test result: ok. 5 passed; 0 failed"
exit 0
"""

DOCS_TESTS = (
    "test_docs", "test_status_page", "test_plan_owners", "test_client_kit", "test_fci_scenarios",
    "test_public_export",
)

PASS = "test result: ok. 3 passed; 0 failed; 0 ignored\n"
FAIL_MARK = "thread 'invented::case' panicked: FAIL-MARKER-7f3a\n"


class LandingTools(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory(prefix="eve-test-gate-")
        self.tmp = self._tmp.name
        self.repo = os.path.join(self.tmp, "repo")
        self.remote = os.path.join(self.tmp, "remote.git")
        self.test_out = os.path.join(self.tmp, "cargo-test.out")
        tmpdir = os.path.join(self.tmp, "t")
        os.mkdir(tmpdir)
        env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        env.update(
            GIT_CONFIG_GLOBAL=os.devnull,
            GIT_CONFIG_NOSYSTEM="1",
            GIT_AUTHOR_NAME="gate test",
            GIT_AUTHOR_EMAIL="gate@test.invalid",
            GIT_COMMITTER_NAME="gate test",
            GIT_COMMITTER_EMAIL="gate@test.invalid",
            TMPDIR=tmpdir,
            STUB_TEST_OUT=self.test_out,
            STUB_TEST_EXIT="0",
            STUB_ARGS=os.path.join(self.tmp, "stub-args"),
        )
        self.env = env
        self.cargo_test(PASS)

        self.git("init", "-q", "--bare", "-b", "master", self.remote, cwd=self.tmp)
        os.makedirs(os.path.join(self.repo, "tools", "hooks"))
        os.makedirs(os.path.join(self.repo, "server", "crates", "invented"))
        for rel in ("gate.sh", "land.sh", os.path.join("hooks", "pre-push")):
            shutil.copy2(os.path.join(HERE, rel), os.path.join(self.repo, "tools", rel))
        self.write("tools/dev.sh", STUB_DEV, exe=True)
        self.write("tools/check_deps.py", "pass\n")
        self.write("server/crates/invented/Cargo.toml", "")
        self.write(".gitignore", "out/\nvar/\n")
        self.git("init", "-q", "-b", "master")
        self.git("remote", "add", "origin", self.remote)
        self.commit("base")
        self.git("push", "-q", "origin", "master")
        self.base = self.rev("HEAD")
        # Installed from here on, as tools/install-hooks.sh does.
        self.git("config", "core.hooksPath", "tools/hooks")
        self.commit("change", ("change.txt", "x\n"))
        self.head = self.rev("HEAD")

    def tearDown(self) -> None:
        self._tmp.cleanup()

    # --- helpers ---------------------------------------------------------------

    def write(self, rel: str, text: str, exe: bool = False) -> None:
        path = os.path.join(self.repo, rel)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w") as f:
            f.write(text)
        if exe:
            os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    def cargo_test(self, text: str, exit_code: int = 0) -> None:
        with open(self.test_out, "w") as f:
            f.write(text)
        if hasattr(self, "env"):
            self.env["STUB_TEST_EXIT"] = str(exit_code)

    def git(self, *args: str, cwd: str | None = None) -> str:
        return subprocess.run(
            ["git", *args], cwd=cwd or self.repo, env=self.env,
            check=True, capture_output=True, text=True, timeout=60,
        ).stdout

    def rev(self, ref: str) -> str:
        return self.git("rev-parse", ref).strip()

    def remote_master(self) -> str:
        return self.git("rev-parse", "master", cwd=self.remote).strip()

    def commit(self, msg: str, *files: tuple[str, str]) -> None:
        for rel, text in files:
            self.write(rel, text)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", msg)

    def run_tool(self, script: str, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [os.path.join(self.repo, "tools", script), *args], cwd=self.repo,
            env=self.env, capture_output=True, text=True, timeout=120,
        )

    # --- V4 G2's five cases ----------------------------------------------------

    def test_a_red_test_run_prints_its_tail_and_land_pushes_nothing(self) -> None:
        self.cargo_test("compiling\n" + FAIL_MARK + "test result: FAILED. 2 passed; 1 failed\n", 101)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 101, r.stderr)
        self.assertIn("land: GATE FAILED (exit 101) -- nothing pushed", r.stderr)
        self.assertIn("FAIL-MARKER-7f3a", r.stderr)
        self.assertNotIn("land: pushed", r.stdout)
        self.assertEqual(self.remote_master(), self.base)
        self.assertFalse(os.path.exists(os.path.join(self.repo, "out", "landed", self.head)))

    def test_zero_tests_passed_is_refused(self) -> None:
        self.cargo_test("test result: ok. 0 passed; 0 failed\n")
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0)
        self.assertIn("gate: no tests actually ran", g.stderr)
        self.assertNotIn("== gate passed", g.stdout)
        r = self.run_tool("land.sh")
        self.assertNotEqual(r.returncode, 0)
        self.assertEqual(self.remote_master(), self.base)

    def docs_commit(self, rel: str) -> None:
        self.git("reset", "-q", "--hard", self.base)
        for name in DOCS_TESTS:
            self.write(f"tools/{name}.py", "print('docs ok')\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "docs test")
        self.git("push", "-q", "--no-verify", "--force", "origin", "master")
        self.commit("docs", (rel, "x\n"))

    def test_a_docs_only_range_skips_the_build_and_still_pushes(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.docs_commit("notes.md")
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.remote_master(), self.rev("HEAD"))

    def test_a_remote_whose_branch_is_main_is_the_base(self) -> None:
        # The public repo's branch is main: the docs-only path still finds its base,
        # first through origin/main alone, then through origin/HEAD.
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.docs_commit("notes.md")
        main_remote = os.path.join(self.tmp, "main.git")
        self.git("init", "-q", "--bare", "-b", "main", main_remote, cwd=self.tmp)
        self.git("push", "-q", "--no-verify", main_remote, f"{self.rev('HEAD~1')}:refs/heads/main")
        self.git("remote", "set-url", "origin", main_remote)
        self.git("fetch", "-q", "origin")
        self.git("update-ref", "-d", "refs/remotes/origin/master")
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stderr)
        self.assertIn("== docs only", g.stdout)
        # origin/HEAD alone: it names a branch that is neither main nor master.
        self.git("update-ref", "refs/remotes/origin/trunk", self.rev("HEAD~1"))
        self.git("symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/trunk")
        self.git("update-ref", "-d", "refs/remotes/origin/main")
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("== docs only", g.stdout)

    def test_markdown_under_content_and_the_maintainer_folder_are_docs_only(self) -> None:
        for rel in ("content/notes.md", "private/list.txt"):
            with self.subTest(rel=rel):
                self.docs_commit(rel)
                g = self.run_tool("gate.sh")
                self.assertEqual(g.returncode, 0, g.stderr)
                self.assertIn("== docs only", g.stdout)

    def test_a_content_table_or_a_tool_gets_the_full_gate(self) -> None:
        for rel in ("content/rows.tsv", "tools/helper.txt", "docs/content/schema.md"):
            with self.subTest(rel=rel):
                self.docs_commit(rel)
                g = self.run_tool("gate.sh")
                self.assertNotIn("== docs only", g.stdout)

    def test_the_docs_path_runs_every_docs_test_and_a_failure_in_any_stops_it(self) -> None:
        self.docs_commit("notes.md")
        r = self.run_tool("gate.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        for name in DOCS_TESTS:
            self.assertIn(f"tools/{name}.py", r.stdout)
        self.write("tools/test_status_page.py", "raise SystemExit(1)\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "page drifted")
        r = self.run_tool("gate.sh")
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("tools/test_status_page.py failed", r.stderr)
        self.assertNotIn("== gate passed", r.stdout)

    # --- the tools-only tier (#280) ----------------------------------------------

    def tier_commit(self, *rels: str) -> None:
        """Base with a stub of each tools test pushed, then one commit touching `rels`."""
        self.git("reset", "-q", "--hard", self.base)
        self.write("tools/test_stub.py", "print('tools ok')\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "tools test")
        self.git("push", "-q", "--no-verify", "--force", "origin", "master")
        files = []
        for r in rels:
            path = os.path.join(self.repo, r)
            old = open(path).read() if os.path.exists(path) else ""
            files.append((r, old + "# probe\n"))  # a comment keeps a stub script working
        if files:
            self.commit("tier", *files)

    def tier_gate(self, *rels: str, args: tuple[str, ...] = ()) -> subprocess.CompletedProcess:
        # A red cargo run: a tier that reaches cargo shows as a non-zero exit.
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.tier_commit(*rels)
        return self.run_tool("gate.sh", *args)

    def test_tools_scripts_docs_and_private_files_are_the_tools_only_tier(self) -> None:
        for rels in (("tools/foo.py",), ("tools/foo.sh",),
                     ("tools/foo.py", "notes/x.md", "private/y.txt")):
            with self.subTest(rels=rels):
                g = self.tier_gate(*rels)
                self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
                self.assertIn(f"== tools only ({len(rels)} file(s))", g.stdout)
                self.assertIn("tools/test_stub.py", g.stdout)
                self.assertIn("\n== gate passed\n", "\n" + g.stdout)
                self.assertNotIn("== docs only", g.stdout)

    def test_the_gate_scripts_and_the_allowlist_check_keep_the_full_gate(self) -> None:
        for rel in ("tools/gate.sh", "tools/land.sh", "tools/dev.sh", "tools/analysis.sh",
                    "tools/check_deps.py", "tools/status-guard.py", "tools/test-parallel.sh",
                    "tools/gate_reds.py", "tools/hooks/pre-push"):
            with self.subTest(rel=rel):
                g = self.tier_gate(rel, "tools/foo.py")
                self.assertEqual(g.returncode, 101, rel)
                self.assertNotIn("== tools only", g.stdout)

    def test_anything_else_beside_a_tools_script_keeps_the_full_gate(self) -> None:
        for rel in ("tools/builds.tsv", "tools/patches/x.patch", "server/crates/a/src/lib.rs",
                    "docs/content/schema.md", "deploy/x.yml", ".github/w.yml"):
            with self.subTest(rel=rel):
                g = self.tier_gate(rel, "tools/foo.py")
                self.assertEqual(g.returncode, 101, rel)
                self.assertNotIn("== tools only", g.stdout)

    def test_markdown_alone_is_still_docs_only_not_tools_only(self) -> None:
        self.docs_commit("notes.md")
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stderr)
        self.assertIn("== docs only", g.stdout)
        self.assertNotIn("== tools only", g.stdout)

    def test_dirty_never_takes_a_light_tier(self) -> None:
        for rels in (("tools/foo.py",), ("notes.md",)):
            with self.subTest(rels=rels):
                g = self.tier_gate(*rels, args=("--dirty",))
                self.assertEqual(g.returncode, 101, rels)
                self.assertNotIn("== tools only", g.stdout)
                self.assertNotIn("== docs only", g.stdout)

    def test_a_failing_tools_test_fails_the_tools_only_tier(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.tier_commit("tools/foo.py")
        self.write("tools/test_red.py", "raise SystemExit(1)\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "red")
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 1, g.stdout + g.stderr)
        self.assertIn("== tools only", g.stdout)
        self.assertIn("tools/test_red.py failed", g.stderr)
        self.assertNotIn("== gate passed", g.stdout)

    def test_a_syntax_error_in_a_tools_script_fails_the_tools_only_tier(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.tier_commit()
        self.commit("typo", ("tools/broken.sh", "if then fi (\n"))
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 1, g.stdout + g.stderr)
        self.assertIn("== tools only", g.stdout)
        self.assertIn("tools/broken.sh fails bash -n", g.stderr)

    def test_every_gate_stage_prints_its_time(self) -> None:
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stderr)
        self.assertGreaterEqual(g.stdout.count("   took "), 8, g.stdout)
        self.assertIn("\n== gate passed\n", "\n" + g.stdout)

    def test_every_failed_python_test_is_named(self) -> None:
        self.commit("py tests", ("tools/test_a_ok.py", "print('fine')\n"),
                    ("tools/test_b_red.py", "print('red b')\nraise SystemExit(1)\n"),
                    ("tools/test_c_red.py", "import sys\nprint('red c')\nsys.exit(3)\n"))
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0)
        # Both reds run to the end and both are named; the green one is not.
        self.assertIn("gate: tools/test_b_red.py failed", g.stderr)
        self.assertIn("gate: tools/test_c_red.py failed", g.stderr)
        self.assertNotIn("test_a_ok.py failed", g.stderr)
        self.assertIn("2 python test(s) failed", g.stderr)
        self.assertIn("red b", g.stdout)
        self.assertIn("red c", g.stdout)
        self.assertNotIn("== gate passed", g.stdout)
        self.assertNotIn("\n== test\n", g.stdout)

    def test_markdown_under_tools_or_any_other_file_runs_the_full_gate(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        for rel in ("tools/notes.md", "notes.txt"):
            self.docs_commit(rel)
            self.assertEqual(self.run_tool("gate.sh").returncode, 101, rel)

    def test_a_pass_with_output_over_the_pipe_buffer_is_still_a_pass(self) -> None:
        # The first pass line comes early and 64 KiB+ follows it, as with a
        # workspace of crates: a `grep -q` reader would exit there and SIGPIPE
        # the writer.
        filler = "test invented::case_%06d ... ok\n"
        body = "".join(filler % i for i in range(4000))
        self.assertGreater(len(body), 64 * 1024)
        self.cargo_test(PASS + body + "test result: ok. 4 passed; 0 failed\n")
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("land: gate passed, 7 tests\n", r.stdout)
        self.assertTrue(r.stdout.endswith("land: pushed\n"), r.stdout)
        self.assertEqual(self.remote_master(), self.head)

    def test_scoped_gates_never_print_the_full_pass_line(self) -> None:
        for args in (("--dirty",), ("--only", "invented"), ("--without", "invented")):
            with self.subTest(args=args):
                g = self.run_tool("gate.sh", *args)
                self.assertEqual(g.returncode, 0, g.stderr)
                self.assertIn("== partial gate passed", g.stdout)
                self.assertNotIn("\n== gate passed\n", "\n" + g.stdout)

    def test_land_refuses_an_exit_0_gate_without_its_pass_line(self) -> None:
        self.commit("stub gate", ("tools/gate.sh", STUB_GATE_NO_PASS_LINE))
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("never printed its pass line -- nothing pushed", r.stderr)
        self.assertEqual(self.remote_master(), self.base)

    # --- G1, L2: the stamp and the hook ---------------------------------------

    def stamp(self, sha: str, tree: str, log: str) -> None:
        d = os.path.join(self.repo, "out", "landed")
        os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, sha + ".log"), "w") as f:
            f.write(log)
        digest = hashlib.sha256(log.encode()).hexdigest()
        with open(os.path.join(d, sha), "w") as f:
            f.write(f"tree {tree}\nlog-sha256 {digest}\n")

    def test_land_head_pushes_from_a_dirty_tree_through_the_hook(self) -> None:
        self.write("lane-half-written.rs", "fn\n")
        r = self.run_tool("land.sh", "--head")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertIn("land: gate passed, 3 tests\n", r.stdout)
        self.assertIn("land: pushed\n", r.stdout)
        self.assertIn(f"pre-push: {self.head} was gated by land.sh", r.stderr)
        self.assertEqual(self.remote_master(), self.head)
        with open(os.path.join(self.repo, "out", "landed", self.head)) as f:
            self.assertIn(f"tree {self.rev('HEAD^{tree}')}\n", f.read())

    def test_a_stamp_for_another_tree_is_refused_and_the_gate_runs(self) -> None:
        self.cargo_test(FAIL_MARK, 101)  # so a gate run by the hook refuses
        self.stamp(self.head, self.rev(self.base + "^{tree}"), "== gate passed\n")
        p = subprocess.run(["git", "push", "origin", "master"], cwd=self.repo,
                           env=self.env, capture_output=True, text=True, timeout=120)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("has no valid land.sh stamp; running the gate", p.stderr)
        self.assertEqual(self.remote_master(), self.base)

    def test_a_stamp_whose_log_was_edited_is_refused(self) -> None:
        self.cargo_test(FAIL_MARK, 101)
        self.stamp(self.head, self.rev("HEAD^{tree}"), "== gate passed\n")
        with open(os.path.join(self.repo, "out", "landed", self.head + ".log"), "a") as f:
            f.write("edited\n")
        p = subprocess.run(["git", "push", "origin", "master"], cwd=self.repo,
                           env=self.env, capture_output=True, text=True, timeout=120)
        self.assertNotEqual(p.returncode, 0)
        self.assertEqual(self.remote_master(), self.base)

    # --- the red log (#282) -------------------------------------------------------

    def with_reds(self, stub: str | None = None) -> None:
        """Commit tools/gate_reds.py (the real one, or `stub`) into the fixture."""
        if stub is None:
            shutil.copy2(os.path.join(HERE, "gate_reds.py"), os.path.join(self.repo, "tools", "gate_reds.py"))
        else:
            self.write("tools/gate_reds.py", stub)
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "reds")

    def reds_rows(self) -> list[list[str]]:
        with open(os.path.join(self.repo, "var", "gate-reds.tsv")) as f:
            lines = f.read().splitlines()
        return [ln.split("\t") for ln in lines[2:]]

    def test_a_red_land_run_records_rows_with_the_heads_tree(self) -> None:
        self.cargo_test("compiling\n" + FAIL_MARK + "test result: FAILED. 2 passed; 1 failed\n", 101)
        self.with_reds()
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 101, r.stderr)
        self.assertIn("land: red recorded", r.stderr)
        rows = self.reds_rows()
        self.assertTrue(rows)
        for row in rows:
            self.assertEqual((row[3], row[6], row[2]), ("red", "101", self.rev("HEAD^{tree}")))
        self.assertEqual(self.remote_master(), self.base)
        self.assertEqual(len(os.listdir(os.path.join(self.repo, "var", "gate-logs"))), 1)

    def test_a_pass_land_run_records_a_pass_row_and_pushes(self) -> None:
        self.with_reds()
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        rows = self.reds_rows()
        self.assertEqual([(x[3], x[7]) for x in rows], [("pass", "-")])
        self.assertEqual(self.remote_master(), self.rev("HEAD"))

    def test_a_failing_recorder_changes_neither_exit_code(self) -> None:
        self.with_reds("import sys\nsys.exit(3)\n")
        self.cargo_test(FAIL_MARK, 101)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 101, r.stderr)
        self.cargo_test(PASS)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.remote_master(), self.rev("HEAD"))

    def test_a_hanging_recorder_is_cut_off_and_changes_no_exit_code(self) -> None:
        import time
        self.env["GATE_REDS_TIMEOUT"] = "1"
        self.with_reds("import time\ntime.sleep(60)\n")
        self.cargo_test(FAIL_MARK, 101)
        t0 = time.monotonic()
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 101, r.stderr)
        self.cargo_test(PASS)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(self.remote_master(), self.rev("HEAD"))
        self.assertLess(time.monotonic() - t0, 40)

    def test_without_the_recorder_land_behaves_as_before(self) -> None:
        self.assertFalse(os.path.exists(os.path.join(self.repo, "tools", "gate_reds.py")))
        self.cargo_test(FAIL_MARK, 101)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 101, r.stderr)
        self.assertNotIn("recorded", r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.repo, "var")))
        self.cargo_test(PASS)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.repo, "var")))

    # --- the parallel test split (#282) ------------------------------------------

    BOT = "server/crates/eve-bot"

    def split_tree(self, *extra: tuple[str, str]) -> None:
        self.commit("bot tests", (f"{self.BOT}/tests/bots/main.rs", "\n"),
                    (f"{self.BOT}/tests/extra.rs", "\n"), *extra)

    def test_the_real_tree_runs_the_parallel_split(self) -> None:
        root = os.path.dirname(HERE)
        r = subprocess.run([os.path.join(HERE, "gate.sh"), "--print-test-jobs"], cwd=root,
                           capture_output=True, text=True, timeout=60)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("test jobs: split (5)", r.stdout)
        tests = os.path.join(root, self.BOT, "tests")
        names = [n[:-3] for n in os.listdir(tests) if n.endswith(".rs")]
        names += [n for n in os.listdir(tests) if os.path.exists(os.path.join(tests, n, "main.rs"))]
        self.assertIn("bots", names)
        jobs = [ln for ln in r.stdout.splitlines() if ln.startswith("   job:")]
        self.assertEqual(len(jobs), 5, r.stdout)
        self.assertIn("-p eve-bot --test bots", jobs[0])
        self.assertIn("-p eve-bot --test bots", jobs[1])
        for n in names:
            if n != "bots":
                self.assertIn(f"--test {n}", jobs[2], r.stdout)

    def test_the_jobs_name_every_bot_test_target(self) -> None:
        self.split_tree()
        r = self.run_tool("gate.sh", "--print-test-jobs")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("test jobs: split (4)", r.stdout)
        self.assertIn("job: -p eve-bot --test bots\n", r.stdout)
        self.assertIn("job: -p eve-bot --lib --bins --test extra\n", r.stdout)
        self.assertIn("job: -p eve-bot --doc\n", r.stdout)
        self.assertIn("job: --workspace --exclude eve-bot\n", r.stdout)

    SHARDS = "tools/bots-shards.txt"

    def shard_jobs(self, text: str | None, **env: str) -> subprocess.CompletedProcess:
        files = [(self.SHARDS, text)] if text is not None else []
        self.split_tree(*files)
        self.env.update(env)
        return self.run_tool("gate.sh", "--print-test-jobs")

    def test_a_shard_file_splits_the_socket_tests_into_complementary_jobs(self) -> None:
        r = self.shard_jobs("# why\nalpha::\n\n  beta_gang::  # tail\n")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("test jobs: split (5)", r.stdout)
        self.assertIn("job: -p eve-bot --test bots -- alpha:: beta_gang::\n", r.stdout)
        self.assertIn("job: -p eve-bot --test bots -- --skip alpha:: --skip beta_gang::\n", r.stdout)
        jobs = [ln for ln in r.stdout.splitlines() if ln.startswith("   job:")]
        self.assertEqual(len(jobs), 5)
        for prefix in ("alpha::", "beta_gang::"):
            self.assertEqual(sum(f" {prefix}" in j and "--skip" not in j for j in jobs), 1)
            self.assertEqual(sum(f"--skip {prefix}" in j for j in jobs), 1)

    def test_the_socket_thread_count_goes_to_both_shards(self) -> None:
        r = self.shard_jobs("alpha::\n", EVE_BOT_TEST_THREADS="7")
        self.assertIn("job: -p eve-bot --test bots -- --test-threads=7 alpha::\n", r.stdout)
        self.assertIn("job: -p eve-bot --test bots -- --test-threads=7 --skip alpha::\n", r.stdout)

    def test_a_missing_empty_or_comments_only_shard_file_gives_one_bots_job(self) -> None:
        for text in (None, "", "\n  \n", "# nothing yet\n  # alpha::\n"):
            with self.subTest(text=text):
                r = self.shard_jobs(text)
                self.assertEqual(r.returncode, 0, r.stderr)
                self.assertIn("test jobs: split (4)", r.stdout)
                self.assertIn("job: -p eve-bot --test bots\n", r.stdout)
                self.assertNotIn("--skip", r.stdout)

    def test_a_shard_prefix_without_its_double_colon_is_refused(self) -> None:
        for bad in ("alpha\n", "alpha:\n", "::\n", "al pha::x\n", "alpha::beta\n", "alpha::\nbeta\n"):
            with self.subTest(bad=bad):
                r = self.shard_jobs(bad)
                self.assertNotEqual(r.returncode, 0, r.stdout)
                self.assertIn("must be a module prefix ending in '::'", r.stderr)
                self.assertNotIn("job:", r.stdout)

    def test_the_real_shard_prefixes_name_one_bots_module_each(self) -> None:
        root = os.path.dirname(HERE)
        bots = os.path.join(root, self.BOT, "tests", "bots")
        stems = {n[:-3] for n in os.listdir(bots) if n.endswith(".rs") and n != "main.rs"}
        stems |= {n for n in os.listdir(bots) if os.path.exists(os.path.join(bots, n, "mod.rs"))}
        with open(os.path.join(root, self.SHARDS)) as f:
            prefixes = [ln.split("#")[0].strip() for ln in f]
        prefixes = [p for p in prefixes if p]
        self.assertTrue(prefixes)
        for p in prefixes:
            with self.subTest(prefix=p):
                self.assertTrue(p.endswith("::"))
                self.assertIn(p[:-2], stems, "stale prefix: no such module")
                for m in stems - {p[:-2]}:
                    self.assertFalse(f"{m}::".endswith(p) or p in f"{m}::",
                                     f"{p} also matches module {m}")

    def test_the_shard_file_keeps_the_full_gate(self) -> None:
        g = self.tier_gate(self.SHARDS, "tools/foo.py")
        self.assertEqual(g.returncode, 101)
        self.assertNotIn("== tools only", g.stdout)

    def test_examples_or_a_forced_serial_run_print_why(self) -> None:
        self.split_tree((f"{self.BOT}/examples/e.rs", "\n"))
        r = self.run_tool("gate.sh", "--print-test-jobs")
        self.assertIn("test jobs: serial (eve-bot has examples)", r.stdout)
        self.assertNotIn("job:", r.stdout)
        self.env["EVE_GATE_SERIAL_TESTS"] = "1"
        r = self.run_tool("gate.sh", "--print-test-jobs")
        self.assertIn("serial (EVE_GATE_SERIAL_TESTS set)", r.stdout)

    def test_declared_targets_in_the_bot_manifest_run_serially(self) -> None:
        for decl in ("[[test]]\nname = \"x\"\npath = \"t/x.rs\"\n", "[lib]\npath = \"src/l.rs\"\n", "autotests = false\n"):
            with self.subTest(decl=decl):
                self.split_tree((f"{self.BOT}/Cargo.toml", "[package]\nname = \"eve-bot\"\n" + decl))
                r = self.run_tool("gate.sh", "--print-test-jobs")
                self.assertIn("test jobs: serial (eve-bot declares targets in Cargo.toml)", r.stdout)
                self.assertNotIn("job:", r.stdout)
        self.split_tree((f"{self.BOT}/Cargo.toml", "[package]\nname = \"eve-bot\"\n"))
        self.assertIn("split (4)", self.run_tool("gate.sh", "--print-test-jobs").stdout)

    def test_the_split_path_runs_and_the_serial_path_is_no_fail_fast(self) -> None:
        self.split_tree()
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("   test jobs: split (4)", g.stdout)
        with open(self.env["STUB_ARGS"]) as f:
            args = f.read()
        self.assertIn("test-parallel.sh -p eve-bot --test bots -p eve-bot --lib --bins --test extra", args)
        os.remove(self.env["STUB_ARGS"])
        g = self.run_tool("gate.sh", "--only", "invented")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("test jobs: serial (scoped run)", g.stdout)

    def test_parallel_script_runs_every_job_with_no_fail_fast(self) -> None:
        with open(os.path.join(HERE, "test-parallel.sh")) as f:
            self.assertIn("cargo test --locked --no-fail-fast", f.read())

    # --- the overlapped host stages (#282) ----------------------------------------

    MARK = "CARGO-TEST-RAN-3c9e\n"

    def test_a_failing_python_test_fails_after_lint_and_before_the_test_run(self) -> None:
        self.cargo_test(PASS + self.MARK)
        self.commit("red py", ("tools/test_red.py", "print('red')\nraise SystemExit(1)\n"))
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 1, g.stdout + g.stderr)
        self.assertIn("gate: tools/test_red.py failed", g.stderr)
        self.assertNotIn(self.MARK, g.stdout)
        self.assertNotIn("\n== test\n", g.stdout)
        order = [g.stdout.index(m) for m in ("== lint", "== test build", "== python unit tests")]
        self.assertEqual(order, sorted(order), g.stdout)

    def test_a_format_red_stops_before_any_python_output(self) -> None:
        flag = os.path.join(self.tmp, "py-ran")
        self.commit("py", ("tools/test_flag.py", f"open({flag!r}, 'w').close()\n"))
        self.env["STUB_FMT_EXIT"] = "1"
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 1, g.stdout + g.stderr)
        self.assertNotIn("== python unit tests", g.stdout)
        self.assertFalse(os.path.exists(flag))

    def test_a_registry_red_stops_before_the_format_stage(self) -> None:
        self.env["STUB_REGISTRY_EXIT"] = "1"
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("== dependency registry", g.stdout)
        self.assertNotIn("== format", g.stdout)

    def test_the_python_report_prints_before_the_test_run(self) -> None:
        self.commit("slow py", ("tools/test_a_slow.py", "import time\ntime.sleep(1)\nprint('slow done')\n"),
                    ("tools/test_b_fast.py", "print('fast done')\n"))
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        out = g.stdout
        idx = [out.index(m) for m in ("== python unit tests", "tools/test_a_slow.py", "tools/test_b_fast.py",
                                      "== test\n")]
        self.assertEqual(idx, sorted(idx), out)
        self.assertEqual(out.count("(overlapped)"), 1)

    # --- the machine-dependent checks (gate-analysis step 7) ----------------------------
    # Stubs record their calls in $STUB_ENV_CALLS; their exit codes come from $STUB_DELTAS_EXIT
    # and $STUB_AUDIT_EXIT.

    def env_stubs(self) -> None:
        self.env_calls = os.path.join(self.tmp, "env-calls")
        self.env.update(STUB_ENV_CALLS=self.env_calls, STUB_DELTAS_EXIT="0", STUB_AUDIT_EXIT="0")
        self.write("tools/new-build.sh", "#!/usr/bin/env bash\n"
                   "echo \"deltas $*\" >>\"$STUB_ENV_CALLS\"\necho '   deltas ran'\nexit \"$STUB_DELTAS_EXIT\"\n", exe=True)
        self.write("tools/status-page.py", "import os, sys\n"
                   "open(os.environ['STUB_ENV_CALLS'], 'a').write('audit ' + ' '.join(sys.argv[1:]) + '\\n')\n"
                   "print('audit ran')\nsys.exit(int(os.environ['STUB_AUDIT_EXIT']))\n")
        self.git("add", "-A")
        self.git("commit", "-q", "-m", "env stubs")

    def env_calls_seen(self) -> list[str]:
        try:
            with open(self.env_calls) as f:
                return f.read().splitlines()
        except FileNotFoundError:
            return []

    def test_the_full_gate_no_longer_runs_the_build_deltas(self) -> None:
        self.env_stubs()
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertNotIn("== build deltas", g.stdout)
        self.assertEqual([c for c in self.env_calls_seen() if c.startswith("deltas")], [])
        self.assertIn("== gate passed", g.stdout)

    def test_the_tools_only_tier_no_longer_runs_the_build_deltas(self) -> None:
        self.cargo_test("test result: FAILED. 0 passed; 1 failed\n", 101)
        self.tier_commit("tools/foo.py")
        self.env_stubs()
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("== tools only", g.stdout)
        self.assertNotIn("== build deltas", g.stdout)
        self.assertEqual([c for c in self.env_calls_seen() if c.startswith("deltas")], [])

    def test_the_landing_audit_is_measured_from_the_base(self) -> None:
        self.env_stubs()
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertEqual(self.env_calls_seen(), [f"audit --audit --no-stamp --since {self.base}"])

    def test_env_checks_run_both_checks_and_never_print_the_gate_pass_line(self) -> None:
        self.env_stubs()
        g = self.run_tool("gate.sh", "--env-checks")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("== env checks passed", g.stdout)
        self.assertNotIn("== gate passed", g.stdout)
        self.assertNotIn("partial gate passed", g.stdout)
        self.assertEqual(self.env_calls_seen(), ["deltas --gate-check", "audit --audit --no-stamp"])

    def test_env_checks_fail_when_either_check_fails_and_still_run_the_other(self) -> None:
        for var, want in (("STUB_DELTAS_EXIT", 3), ("STUB_AUDIT_EXIT", 1)):
            with self.subTest(var=var):
                self.env_stubs()
                self.env[var] = str(want)
                g = self.run_tool("gate.sh", "--env-checks")
                self.assertEqual(g.returncode, want, g.stdout + g.stderr)
                self.assertNotIn("== env checks passed", g.stdout)
                self.assertNotIn("== gate passed", g.stdout)
                self.assertEqual(len(self.env_calls_seen()), 2)
                os.remove(self.env_calls)
                self.git("reset", "-q", "--hard", "HEAD^")

    def test_env_checks_need_neither_a_clean_tree_nor_the_build(self) -> None:
        self.env_stubs()
        self.write("scratch.txt", "dirty\n")
        self.cargo_test(self.MARK, 101)
        g = self.run_tool("gate.sh", "--env-checks")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertNotIn(self.MARK, g.stdout)

    def test_land_refuses_an_env_checks_log_as_a_pass(self) -> None:
        # land.sh greps for "== gate passed"; --env-checks never prints it.
        with open(os.path.join(HERE, "gate.sh")) as f:
            text = f.read()
        self.assertEqual(text.count('echo "== gate passed"'), 3)
        env_block = text[text.index("if ((env_checks)); then"):text.index("# `--without` re-execs")]
        self.assertNotIn("gate passed\"", env_block)

    # --- the flaky rerun (gate-analysis step 5) ----------------------------------------
    # Invented names only. The first test run answers from STUB_TEST_OUT; the rerun is the
    # one stub call with `--exact`, answered from STUB_RERUN_OUT.

    FLAKY_TARGET = "eve_invented:src/lib.rs"
    FLAKY_HEAD = "#flaky 1\ntarget\ttest\tissue\tadded\n"

    @staticmethod
    def red_run(*failed: str, extra: str = "") -> str:
        return ("   Running unittests src/lib.rs (target/debug/deps/eve_invented-0123abcd)\n"
                + "".join(f"test {n} ... FAILED\n" for n in failed) + extra
                + "test result: FAILED. 1 passed; %d failed\n" % len(failed)
                + "error: test failed, to rerun pass `-p eve_invented --lib`\n")

    def flaky_tree(self, *listed: str, rerun: str = "", rerun_exit: int = 0, in_range: bool = False) -> None:
        """The list and the recorder are pushed as the base (the gate reads the list from the
        base); `in_range` leaves the list in the range under test instead."""
        rows = "".join(f"{self.FLAKY_TARGET}\t{n}\t#999\t2026-11-09\n" for n in listed)
        self.write("tools/flaky.tsv", self.FLAKY_HEAD + rows)
        self.with_reds()
        if not in_range:
            self.git("push", "-q", "--no-verify", "--force", "origin", "master")
            self.commit("change under test", ("change2.txt", "y\n"))
        self.rerun_out = os.path.join(self.tmp, "rerun.out")
        self.rerun_count = os.path.join(self.tmp, "rerun.count")
        with open(self.rerun_out, "w") as f:
            f.write(rerun)
        self.env.update(STUB_RERUN_OUT=self.rerun_out, STUB_RERUN_EXIT=str(rerun_exit),
                        STUB_RERUN_COUNT=self.rerun_count)

    def reruns(self) -> int:
        try:
            with open(self.rerun_count) as f:
                return len(f.read().splitlines())
        except FileNotFoundError:
            return 0

    def test_a_listed_failure_is_rerun_and_passes(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\ntest result: ok. 1 passed; 0 failed\n")
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("== flaky rerun: 1 test(s) on the flake list (once)", g.stdout)
        self.assertIn("-p eve_invented --lib -- --exact flaky::one", g.stdout)
        self.assertIn("test flaky::one ... FAILED", g.stdout)
        self.assertIn("test flaky::one ... ok", g.stdout)
        self.assertIn("== flaky rerun passed", g.stdout)
        self.assertTrue(g.stdout.rstrip().endswith("== gate passed"), g.stdout)
        self.assertEqual(self.reruns(), 1)

    def test_a_listed_failure_that_fails_again_fails_the_gate_with_the_first_exit(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... FAILED\n", rerun_exit=7)
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 101, g.stdout + g.stderr)
        self.assertIn("== flaky rerun: 1 test(s)", g.stdout)
        self.assertNotIn("== flaky rerun passed", g.stdout)
        self.assertNotIn("== gate passed", g.stdout)
        self.assertEqual(self.reruns(), 1)

    def test_a_rerun_that_never_prints_the_test_ok_is_not_a_pass(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="running 0 tests\ntest result: ok. 0 passed; 0 failed\n")
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 101, g.stdout + g.stderr)
        self.assertNotIn("== flaky rerun passed", g.stdout)

    def assert_no_rerun(self, why: str, code: int = 101) -> None:
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, code, g.stdout + g.stderr)
        self.assertEqual(self.reruns(), 0, g.stdout)
        self.assertNotIn("== flaky rerun", g.stdout)
        self.assertIn("no flaky rerun: " + why, g.stdout)
        self.assertNotIn("== gate passed", g.stdout)

    def test_a_new_red_beside_a_listed_one_never_reruns(self) -> None:
        self.cargo_test(self.red_run("flaky::one", "fresh::two"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n")
        self.assert_no_rerun("fresh::two on eve_invented:src/lib.rs is not on the flake list")

    def test_an_unlisted_failure_alone_never_reruns(self) -> None:
        self.cargo_test(self.red_run("fresh::two"), 101)
        self.flaky_tree("flaky::one", rerun="test fresh::two ... ok\n")
        self.assert_no_rerun("fresh::two on eve_invented:src/lib.rs is not on the flake list")

    def test_a_listed_failure_with_a_world_ticker_panic_never_reruns(self) -> None:
        self.cargo_test(self.red_run("flaky::one", extra="thread 'world-ticker' panicked at src/lib.rs:1:1:\n"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n")
        self.assert_no_rerun("a panic outside the test threads (world-ticker)")

    def test_four_listed_failures_never_rerun(self) -> None:
        names = ["flaky::a", "flaky::b", "flaky::c", "flaky::d"]
        self.cargo_test(self.red_run(*names), 101)
        self.flaky_tree(*names, rerun="".join(f"test {n} ... ok\n" for n in names))
        self.assert_no_rerun("4 failures (at most 3 are rerun)")

    def test_three_listed_failures_are_rerun_in_one_call(self) -> None:
        names = ["flaky::a", "flaky::b", "flaky::c"]
        self.cargo_test(self.red_run(*names), 101)
        self.flaky_tree(*names, rerun="".join(f"test {n} ... ok\n" for n in names) + PASS)
        g = self.run_tool("gate.sh")
        self.assertEqual(g.returncode, 0, g.stdout + g.stderr)
        self.assertIn("== flaky rerun: 3 test(s)", g.stdout)
        self.assertEqual(self.reruns(), 1)

    def test_a_list_that_is_part_of_the_change_under_test_never_reruns(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n", in_range=True)
        self.assert_no_rerun("the list changed in this range")

    def test_a_list_edited_in_the_range_is_not_trusted_even_if_the_base_lists_the_test(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n")
        self.commit("edit the list", ("tools/flaky.tsv", self.FLAKY_HEAD
                                      + f"{self.FLAKY_TARGET}\tflaky::one\t#999\t2026-11-10\n"))
        self.assert_no_rerun("the list changed in this range")

    def test_a_run_with_no_base_never_reruns(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n")
        g = self.run_tool("gate.sh", "--dirty")
        self.assertEqual(g.returncode, 101, g.stdout + g.stderr)
        self.assertEqual(self.reruns(), 0)
        self.assertIn("no flaky rerun: no base to read the flake list from", g.stdout)

    def test_a_rerun_that_names_no_test_is_not_a_pass(self) -> None:
        gate = open(os.path.join(HERE, "gate.sh")).read()
        self.assertIn("((${#rerun_names[@]} > 0)) || rerun_ok=0", gate)

    def test_the_switch_turns_the_rerun_off(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n")
        self.env["EVE_GATE_NO_RERUN"] = "1"
        self.assert_no_rerun("EVE_GATE_NO_RERUN is set")

    def test_a_compile_error_never_reruns(self) -> None:
        self.cargo_test("error[E0308]: mismatched types\nerror: could not compile `eve_invented`\n", 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n")
        self.assert_no_rerun("a compile error")

    def test_an_empty_flake_list_never_reruns(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree(rerun="test flaky::one ... ok\n")
        self.assert_no_rerun("the flake list has no rows")

    def test_the_real_flake_list_is_valid_and_empty(self) -> None:
        with open(os.path.join(HERE, "flaky.tsv")) as f:
            lines = f.read().splitlines()
        self.assertEqual(lines[0], "#flaky 1")
        rows = [ln for ln in lines[1:] if ln and not ln.startswith("#")]
        self.assertEqual(rows, ["target\ttest\tissue\tadded"])

    def test_land_records_the_rescued_failures_beside_the_pass(self) -> None:
        self.cargo_test(self.red_run("flaky::one"), 101)
        self.flaky_tree("flaky::one", rerun="test flaky::one ... ok\n" + PASS)
        r = self.run_tool("land.sh")
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)
        self.assertEqual(self.remote_master(), self.rev("HEAD"))
        results = [(row[3], row[9]) for row in self.reds_rows()]
        self.assertIn(("rescued", "flaky::one"), results)
        self.assertIn(("pass", "-"), results)

    # --- G3 --------------------------------------------------------------------

    def test_a_shell_script_with_a_syntax_error_fails_the_gate(self) -> None:
        self.commit("typo", ("tools/broken.sh", "if then fi (\n"))
        g = self.run_tool("gate.sh")
        self.assertNotEqual(g.returncode, 0)
        self.assertIn("gate: tools/broken.sh has a syntax error", g.stderr)
        self.assertNotIn("== gate passed", g.stdout)


class DefaultBase(unittest.TestCase):
    """gate.sh's `default_base`: upstream's HEAD, main, master, then origin's. Temp repos only."""

    def pick(self, refs: list[str]) -> str:
        with open(os.path.join(HERE, "gate.sh"), encoding="utf-8") as f:
            text = f.read()
        start = text.index("default_base() {")
        func = text[start:text.index("\n}\n", start) + 3]
        env = {"PATH": os.environ.get("PATH", ""), "HOME": "/nonexistent",
               "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null",
               "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@test.invalid",
               "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@test.invalid"}
        with tempfile.TemporaryDirectory() as d:
            def g(*a: str) -> str:
                return subprocess.run(["git", *a], cwd=d, env=env, check=True,
                                      capture_output=True, text=True).stdout.strip()
            g("init", "-q", "-b", "work")
            g("commit", "-q", "--allow-empty", "-m", "a")
            first = g("rev-parse", "HEAD")
            g("commit", "-q", "--allow-empty", "-m", "b")
            second = g("rev-parse", "HEAD")
            for r in refs:
                g("update-ref", f"refs/remotes/{r}", second if r.startswith("upstream") else first)
            out = subprocess.run(["bash", "-c", func + "\ndefault_base"], cwd=d, env=env,
                                 capture_output=True, text=True)
            if out.returncode != 0:
                return "none"
            return "upstream" if out.stdout.strip() == second else "origin"

    def test_upstream_wins_then_origin_then_nothing(self) -> None:
        self.assertEqual(self.pick(["upstream/main", "origin/master"]), "upstream")
        self.assertEqual(self.pick(["upstream/master", "origin/main"]), "upstream")
        self.assertEqual(self.pick(["origin/master"]), "origin")
        self.assertEqual(self.pick(["origin/main"]), "origin")
        self.assertEqual(self.pick([]), "none")


if __name__ == "__main__":
    unittest.main()
