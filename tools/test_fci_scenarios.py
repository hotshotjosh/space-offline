"""Tests for the f-ci scenarios and their log checker (packet F-CI, docs/lanes/f-ci.md).

The scenarios run only inside the client, under Stackless Python 2.7, so
nothing here runs one.  Checked without a client:

- each `fci_*` scenario carries the canonical prelude and the canonical fci
  block (`sync-prelude.py --check`, `sync-fci.py --check`);
- each names itself, declares `EOCI_FCI_TIMEOUT_S`, and declares at least one
  `SERVER-EXPECT: absent UNANSWERED ...` directive that parses;
- each body parses and uses nothing Python 2.7 lacks, no `print`, no
  8-9 digit literal inside a string that could be a copied client id;
- `server-expect.py` evaluates present, absent and max directives correctly;
- every `CI: <name>` in docs/guides/human-checks.md names a scenario or log-check.

Stdlib unittest; reads files only.
"""
import ast
import importlib.util
import pathlib
import re
import sys
import textwrap
import unittest

TOOLS = pathlib.Path(__file__).resolve().parent
CI = TOOLS / "client-ci"
SCEN = CI / "scenarios"


def _load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


sync_prelude = _load("sync_prelude_f", CI / "sync-prelude.py")
sync_fci = _load("sync_fci", CI / "sync-fci.py")
expect = _load("server_expect", CI / "server-expect.py")
NAMES = sorted(p.stem for p in SCEN.glob("fci_*.py"))


def _parts(name):
    return sync_prelude.split((SCEN / f"{name}.py").read_text(), name)


class Shape(unittest.TestCase):
    def test_there_are_scenarios(self):
        self.assertGreaterEqual(len(NAMES), 10)

    def test_prelude_and_block_are_canonical(self):
        _, p1, _, p2 = sync_prelude.split((SCEN / "_prelude.py").read_text(), "_prelude")
        canon = (CI / "fci-block.txt").read_text()
        for n in NAMES:
            with self.subTest(n):
                _, h1, _body, h2 = _parts(n)
                self.assertEqual((h1, h2), (p1, p2))
                self.assertEqual(sync_fci.split((SCEN / f"{n}.py").read_text(), n)[1], canon)

    def test_names_timeout_and_expectations(self):
        for n in NAMES:
            with self.subTest(n):
                head = _parts(n)[0]
                ast.parse(head)  # a stray block after the docstring is a SyntaxError
                self.assertIn(f"EOCI_SCENARIO = '{n}'\n", head)
                self.assertRegex(head, r"(?m)^EOCI_FCI_TIMEOUT_S = \d+$")
                ds = expect.parse(head)
                self.assertTrue(any(d["want"] == "absent" and "UNANSWERED" in d["src"]
                                    for d in ds if d["kind"] == "expect"), n)


class Python27(unittest.TestCase):
    BANNED = (ast.JoinedStr, ast.Nonlocal, ast.YieldFrom, ast.AsyncFunctionDef,
              ast.Await, ast.NamedExpr, ast.AnnAssign)

    def test_bodies_parse_and_stay_27(self):
        for n in NAMES:
            with self.subTest(n):
                tree = ast.parse(textwrap.dedent(_parts(n)[2]))
                for node in ast.walk(tree):
                    self.assertNotIsInstance(node, self.BANNED)
                    if isinstance(node, ast.Call) and isinstance(node.func, ast.Name):
                        self.assertNotEqual(node.func.id, "print")
                    if isinstance(node, (ast.FunctionDef, ast.Lambda)):
                        self.assertFalse(node.args.kwonlyargs)

    def test_str_format_arity_matches_its_arguments(self):
        """A `'... 70% of ...' % (a, b)` raises TypeError before the body runs,
        and the trigger's bare `except` hides it (fci_doc_6 never ran a body
        because of one).  Evaluate every literal-left `%` with as many 1s as
        the tuple on its right has elements."""
        for n in NAMES:
            with self.subTest(n):
                src = textwrap.dedent(_parts(n)[2])
                src = re.sub(r"except\s+([\w.]+)\s*,\s*(\w+)\s*:", r"except \1 as \2:", src)
                src = re.sub(r"\b(\d+)[lL]\b", r"\1", src)
                for node in ast.walk(ast.parse(src)):
                    if not (isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mod)
                            and isinstance(node.left, ast.Constant)
                            and isinstance(node.left.value, str)):
                        continue
                    r = node.right
                    if isinstance(r, ast.Tuple):
                        if any(isinstance(e, ast.Starred) for e in r.elts):
                            continue
                        args = tuple(1 for _ in r.elts)
                    elif isinstance(r, (ast.Dict, ast.Name, ast.Attribute, ast.Call,
                                        ast.Subscript, ast.Starred)):
                        continue  # may be a dict or a tuple: not knowable here
                    else:
                        args = 1
                    try:
                        node.left.value % args
                    except (TypeError, ValueError) as e:
                        self.fail("%s line %d: %s: %r" % (n, node.lineno, e, node.left.value[:80]))

    def test_log_since_is_unpacked(self):
        """`log_since(mark)` answers `(text or None, detail)`.  Used as a
        value it is a tuple: `'x' in log_since(mark)` is always False and
        `log_lines_with(log_since(mark), ...)` raises (fci_cloak,
        fci_gate_cloak and fci_motherlode all did).  Every call must be the
        right side of a tuple assignment."""
        for n in NAMES:
            with self.subTest(n):
                tree = ast.parse(textwrap.dedent(_parts(n)[2]))
                unpacked = set()
                for node in ast.walk(tree):
                    if (isinstance(node, ast.Assign) and isinstance(node.value, ast.Call)
                            and isinstance(node.targets[0], ast.Tuple)):
                        unpacked.add(id(node.value))
                for node in ast.walk(tree):
                    if (isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
                            and node.func.id == "log_since"):
                        self.assertIn(id(node), unpacked,
                                      "%s line %d: log_since() returns a tuple; unpack it"
                                      % (n, node.lineno))

    def test_a_quiet_early_return_does_not_skip_the_traceback_check(self):
        """A scenario ends with `no_new_tracebacks(...)`.  A `return` before it
        that no failed check explains skips it, and the run goes green with
        tracebacks in the client log (fci_y_wh returned on "no anomaly row"
        and passed with six overview exceptions in its log).

        Every bare `return` in a function that calls `no_new_tracebacks`, and
        sits before the last call, must have one of: an earlier
        `no_new_tracebacks` in the function; a reporting call (`check`,
        `gm_check`, `precondition`, or a local helper that makes one) earlier
        in its own block; or an `if` test made only of names that such a call
        took, or that were assigned from one (so a refusal is red anyway).
        A return right after a `for` loop that sleeps is a relaunched copy idling
        until the harness kills it (fci_logoff_space, fci_upwell_relog); it is
        left alone.
        Otherwise call `no_new_tracebacks` just before the `return`.  This is
        a heuristic over the shape of the code: it cannot see whether the
        check really failed."""
        report = {"check", "gm_check", "precondition"}

        def ids(node):
            return {x.id for x in ast.walk(node) if isinstance(x, ast.Name)}

        def calls_of(node, names):
            return any(isinstance(x, ast.Call) and isinstance(x.func, ast.Name)
                       and x.func.id in names for x in ast.walk(node))

        bad = []
        for n in NAMES:
            with self.subTest(n):
                text = (SCEN / f"{n}.py").read_text()
                head, p1, body, _p2 = sync_prelude.split(text, n)
                first = (head + p1).count("\n")
                tree = ast.parse(textwrap.dedent(body))
                helpers = set()
                for _ in range(4):
                    for f in ast.walk(tree):
                        if (isinstance(f, ast.FunctionDef) and f.name != "eoci_body"
                                and f.name not in helpers
                                and calls_of(f, report | helpers)):
                            helpers.add(f.name)
                reporting = report | helpers
                for fn in ast.walk(tree):
                    if not isinstance(fn, ast.FunctionDef):
                        continue
                    returns = []

                    def walk(block, tests):
                        for i, s in enumerate(block):
                            if isinstance(s, ast.Return):
                                returns.append((s, block, i, tests))
                            if isinstance(s, (ast.FunctionDef, ast.ClassDef)):
                                continue
                            t2 = tests + [s.test] if isinstance(s, (ast.If, ast.While)) else tests
                            for part in ("body", "orelse", "finalbody"):
                                sub = getattr(s, part, None)
                                if isinstance(sub, list):
                                    walk(sub, t2)
                            for h in getattr(s, "handlers", []):
                                walk(h.body, t2)
                    walk(fn.body, [])
                    every = [x for x in ast.walk(fn) if isinstance(x, ast.Call)
                             and isinstance(x.func, ast.Name)]
                    guards = [c.lineno for c in every if c.func.id == "no_new_tracebacks"]
                    if not guards:
                        continue
                    for ret, block, i, tests in returns:
                        if ret.lineno > max(guards) or any(g < ret.lineno for g in guards):
                            continue
                        if any(calls_of(s, reporting) for s in block[:i]):
                            continue
                        if i and isinstance(block[i - 1], ast.For) and calls_of(block[i - 1], {"sleep_ms"}):
                            continue   # a relaunched copy idling until it is killed
                        seen = set()
                        for c in every:
                            if c.lineno < ret.lineno and c.func.id in reporting:
                                seen |= ids(c)
                        grew = True
                        while grew:
                            grew = False
                            for a in ast.walk(fn):
                                if (isinstance(a, ast.Assign) and a.lineno < ret.lineno
                                        and calls_of(a.value, reporting)):
                                    tg = set().union(*[ids(x) for x in a.targets])
                                    if not tg <= seen:
                                        seen |= tg
                                        grew = True
                        if tests and ids(tests[-1]) <= seen:
                            continue
                        bad.append("%s line %d: this return skips no_new_tracebacks (line %d) "
                                   "with no failed check behind it; call it first"
                                   % (n, ret.lineno + first, max(guards) + first))
        self.assertEqual(bad, [], "quiet early returns:\n" + "\n".join(bad))

    def test_no_eight_digit_number_in_a_string(self):
        for n in NAMES:
            with self.subTest(n):
                tree = ast.parse(textwrap.dedent(_parts(n)[2]))
                for c in ast.walk(tree):
                    if (isinstance(c, ast.Constant) and isinstance(c.value, str)
                            and not c.value.startswith('/giveisk')):
                        self.assertIsNone(re.search(r"\b\d{8,9}\b", c.value), c.value)


class EssPanelRatPick(unittest.TestCase):
    """fci_ess_panel kills the rat that orbits closest, and its kill wait also
    ends on our own ship's loss (#192).  `rank_rats` is pure, so it runs here
    out of the scenario body."""

    def _body(self):
        return ast.parse(textwrap.dedent(_parts("fci_ess_panel")[2]))

    def _rank(self):
        fn = [n for n in self._body().body
              if isinstance(n, ast.FunctionDef) and n.name == "rank_rats"]
        self.assertEqual(len(fn), 1, "rank_rats at body level")
        ns = {}
        exec(compile(ast.Module(body=fn, type_ignores=[]), "rank_rats", "exec"), ns)
        return ns["rank_rats"]

    def test_belt_order_is_nearest_first_and_capped(self):
        fn = [n for n in self._body().body
              if isinstance(n, ast.FunctionDef) and n.name == "belt_order"]
        self.assertEqual(len(fn), 1, "belt_order at body level")
        ns = {}
        exec(compile(ast.Module(body=fn, type_ignores=[]), "belt_order", "exec"), ns)
        order = ns["belt_order"]
        rows = [(9000.0, 5), (1000.0, 7), (None, 2), (1000.0, 3), (500.0, 9), (0.0, 8)]
        self.assertEqual(order(rows, 3), [8, 9, 3])
        self.assertEqual(order(rows, 99), [8, 9, 3, 7, 5, 2])
        self.assertEqual(order(rows, 0), [])

    def test_belt_verdict_stops_gives_up_or_goes_on(self):
        fn = [n for n in self._body().body
              if isinstance(n, ast.FunctionDef) and n.name == "belt_verdict"]
        self.assertEqual(len(fn), 1, "belt_verdict at body level")
        ns = {}
        exec(compile(ast.Module(body=fn, type_ignores=[]), "belt_verdict", "exec"), ns)
        v = ns["belt_verdict"]
        self.assertEqual(v(True, True, True), "stop")
        self.assertEqual(v(False, True, True), "next")
        self.assertEqual(v(False, False, True), "next")
        self.assertEqual(v(True, True, False), "give_up")

    def test_the_closest_orbit_wins_over_the_smallest_hull(self):
        rank = self._rank()
        rows = [(901, 12000.0, 20.0), (902, 5000.0, 40.0), (903, 1000.0, 45.0),
                (904, 30000.0, 150.0)]
        self.assertEqual(rank(rows, None), [901, 902, 903, 904])

    def test_a_large_rat_in_reach_loses_to_a_small_one_in_reach(self):
        rank = self._rank()
        rows = [(931, 1000.0, 150.0), (932, 3000.0, 20.0), (933, 9000.0, 10.0)]
        self.assertEqual(rank(rows, 4000.0), [932, 931, 933])

    def test_ties_go_to_the_smaller_hull_then_the_lower_id(self):
        rank = self._rank()
        self.assertEqual(rank([(912, 5000.0, 40.0), (911, 5000.0, 40.0),
                               (913, 5000.0, 30.0)], None), [913, 911, 912])
        self.assertEqual(rank([(942, 5000.0, 40.0), (941, 2000.0, 40.0)], None), [941, 942])

    def test_an_unknown_range_sorts_last(self):
        rank = self._rank()
        self.assertEqual(rank([(921, None, 10.0), (922, 9000.0, None)], 20000.0), [922, 921])

    def test_the_kill_wait_watches_our_ship(self):
        body = self._body()
        src = textwrap.dedent(_parts("fci_ess_panel")[2])
        self.assertIn("'our ship lives through the fight'", src)
        calls = [n for n in ast.walk(body) if isinstance(n, ast.Call)
                 and isinstance(n.func, ast.Name) and n.func.id == "wait_for"
                 and n.args and isinstance(n.args[0], ast.Name) and n.args[0].id == "over"]
        self.assertEqual(len(calls), 1, "one wait_for(over, ...)")
        defs = [n for n in ast.walk(body) if isinstance(n, ast.FunctionDef) and n.name == "over"]
        self.assertEqual(len(defs), 1, "one over()")
        # the comparison must be the test of a top-level `if` in over(), so a
        # comparison in dead code or in a string does not count
        guards = [n for n in defs[0].body if isinstance(n, ast.If)
                  and any(isinstance(c, ast.Compare) and any(isinstance(o, ast.NotEq) for o in c.ops)
                          and "'shipid'" in ast.dump(c) and "Name(id='ship'" in ast.dump(c)
                          for c in ast.walk(n.test))]
        self.assertTrue(guards, "over() guards on session.shipid != ship")
        before = defs[0].body[:defs[0].body.index(guards[0])]
        self.assertFalse([n for n in before if isinstance(n, ast.Return)],
                         "a return ahead of the guard makes it dead code")
        self.assertTrue([n for n in guards[0].body if isinstance(n, ast.Return)],
                        "the guard ends the wait")

    def test_the_kit_carries_a_gun_for_every_gun_flag_and_a_low_rack(self):
        """The fight is won by the fit, not a longer wait (#192 r3): the GM
        kit creates as many autocannons as the scenario has gun flags, the
        low rack is fitted and onlined, and KILL_WAIT_MS did not grow."""
        body = self._body()
        consts = {}
        for n in ast.walk(body):
            if isinstance(n, ast.Assign) and len(n.targets) == 1 \
                    and isinstance(n.targets[0], ast.Name):
                consts[n.targets[0].id] = n.value
        self.assertIsInstance(consts.get("GUN_FLAGS"), ast.Tuple)
        n_guns = len(consts["GUN_FLAGS"].elts)
        self.assertIsInstance(consts.get("LOW_FIT"), ast.Tuple)
        self.assertTrue(consts["LOW_FIT"].elts, "a low rack is fitted")
        src = textwrap.dedent(_parts("fci_ess_panel")[2])
        made = re.findall(r"/createitem \"200mm AutoCannon I\" (\d+)", src)
        self.assertEqual(made, [str(n_guns)], "one autocannon per gun flag")
        self.assertRegex(src, r"for name, flag in LOW_FIT:")
        self.assertRegex(src, r"for m in guns \+ tank:\n\s+ok, detail = online_module")
        self.assertLessEqual(ast.literal_eval(consts["KILL_WAIT_MS"]), 240000,
                             "fix the fit, not the timeout")


class EngineFixStopBound(unittest.TestCase):
    """fci_engine_fix bounds the gun's stop by the cycle it had left, read at
    run time, not by a fixed 2500 ms (#192 r3): the server ends a lost
    target's effect at the running cycle's boundary."""

    def _body(self):
        return ast.parse(textwrap.dedent(_parts("fci_engine_fix")[2]))

    def _fn(self, name):
        fn = [n for n in self._body().body
              if isinstance(n, ast.FunctionDef) and n.name == name]
        self.assertEqual(len(fn), 1, name + " at body level")
        ns = {}
        exec(compile(ast.Module(body=fn, type_ignores=[]), name, "exec"), ns)
        return ns[name]

    def test_the_time_left_is_measured_from_the_start_in_whole_cycles(self):
        left = self._fn("cycle_left_ms")
        self.assertEqual(left(1000.0, 5000.0, 5200.0), 800.0)
        self.assertEqual(left(1000.0, 5000.0, 7300.0), 700.0)
        # at a boundary a whole cycle is left: the next one has just begun
        self.assertEqual(left(1000.0, 5000.0, 6000.0), 1000.0)

    def test_an_unknown_input_gives_no_bound(self):
        left = self._fn("cycle_left_ms")
        self.assertIsNone(left(None, 5000.0, 5200.0))
        self.assertIsNone(left(1000.0, None, 5200.0))
        self.assertIsNone(left(1000.0, 5000.0, None))
        self.assertIsNone(left(0.0, 5000.0, 5200.0))

    def test_the_stop_wait_is_derived_and_the_relock_waits_for_the_stop(self):
        src = textwrap.dedent(_parts("fci_engine_fix")[2])
        self.assertNotIn("STOP_WAIT_MS", src)
        self.assertRegex(src, r"bound_ms = int\(left_ms \+ TICK_MS \+ STOP_SLACK_MS\)")
        self.assertRegex(src, r"module_active\(g\.itemID\) is False,\s+timeout_ms=bound_ms")
        self.assertRegex(src, r"if stopped:\n\s+ok, detail = lock_target\(rat\)")
        # the precondition keeps the bound shorter than one more cycle
        self.assertRegex(src, r"cycle_ms > TICK_MS \+ STOP_SLACK_MS")


class ServerExpect(unittest.TestCase):
    def test_directives(self):
        ds = expect.parse("# SERVER-EXPECT: absent  UNANSWERED x\n"
                          "# SERVER-EXPECT: present \\[hack\\]\n"
                          "# BOTS-EXPECT: present hello\n"
                          "# SERVER-MAX[skip=1]: held ([0-9.]+) ms <= 25\n")
        logs = {"SERVER": ["held 99 ms", "held 20 ms", "held 24.5 ms", "[hack] ok"],
                "BOTS": ["hello"]}
        self.assertEqual([x[0] for x in expect.evaluate(ds, logs)], [True, True, True, True])
        logs["SERVER"] += ["UNANSWERED x.y", "held 30 ms"]
        self.assertEqual([x[0] for x in expect.evaluate(ds, logs)], [False, True, True, False])

    def test_a_missing_log_is_not_a_pass(self):
        r = expect.evaluate(expect.parse("# BOTS-EXPECT: absent boom\n"), {"BOTS": None})
        self.assertIsNone(r[0][0])

    def test_bad_directives_are_refused(self):
        for bad in ("# SERVER-EXPECT: maybe x\n", "# SERVER-MAX: a <= 3\n", "# SERVER-MAX: (a) 3\n"):
            with self.assertRaises(ValueError):
                expect.parse(bad)

    def test_every_log_check_parses(self):
        for p in (CI / "log-checks").glob("*.expect"):
            self.assertTrue(expect.parse(p.read_text()), p)


class RegressList(unittest.TestCase):
    """`tools/client-ci/regress.list` is the runner's input: a scenario that is
    missing from it is never rerun, and one that needs a flag and lacks it
    fails for a reason that is not the server's."""

    @staticmethod
    def entries():
        out = {}
        for raw in (CI / "regress.list").read_text().splitlines():
            line = raw.split("#", 1)[0].split()
            if line:
                out[line[0]] = line[1:]
        return out

    def test_every_entry_is_a_scenario_and_every_scenario_an_entry(self):
        names = {k.split("@")[0] for k in self.entries()}
        have = {p.stem for p in SCEN.glob("*.py")
                if not p.stem.startswith(("_", "census_"))}
        self.assertEqual(names - have, set(), "listed but not a scenario")
        self.assertEqual(have - names, set(), "a scenario the list never runs")

    def test_keys_are_unique(self):
        keys = [r.split("#", 1)[0].split()[0]
                for r in (CI / "regress.list").read_text().splitlines()
                if r.split("#", 1)[0].split()]
        self.assertEqual(len(keys), len(set(keys)))

    def test_two_client_runs_have_a_roster_two_deep(self):
        for key, args in self.entries().items():
            if "--with-second" in args:
                self.assertGreaterEqual(args.count("--character"), 2, key)

    def test_scenarios_that_say_they_need_dev_get_it(self):
        for key, args in self.entries().items():
            text = (SCEN / (key.split("@")[0] + ".py")).read_text()
            if re.search(r"Needs (a )?`?--dev`?", text) or "Needs a `--dev` server" in text:
                self.assertIn("--dev", args, key)


class Docs(unittest.TestCase):
    def test_ci_references_resolve(self):
        card = TOOLS.parent / "docs" / "guides" / "human-checks.md"
        if not card.exists():
            self.skipTest("docs/guides/human-checks.md is private and absent from this tree")
        text = card.read_text()
        known = {p.stem for p in SCEN.glob("*.py")} | {
            p.stem for p in (CI / "log-checks").glob("*.expect")}
        for ref in re.findall(r"CI: ([a-z0-9_]+)", text):
            self.assertIn(ref, known, ref)


if __name__ == "__main__":
    unittest.main()
