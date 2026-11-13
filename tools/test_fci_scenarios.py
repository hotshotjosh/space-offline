"""Tests for the f-ci scenarios and their log checker (packet F-CI, docs/lanes/f-ci.md).

The scenarios run only inside the client, under Stackless Python 2.7, so
nothing here runs one.  Checked without a client:

- each `fci_*` scenario carries the canonical prelude and the canonical fci
  block (`sync-prelude.py --check`, `sync-fci.py --check`);
- each names itself, declares `EOCI_FCI_TIMEOUT_S`, and declares at least one
  `SERVER-EXPECT: absent UNANSWERED ...` directive that parses;
- every name read inside `eoci_main` is bound in its scope chain, at module
  level or by Python 2.7 itself (`undefined_names`);
- each body parses and uses nothing Python 2.7 lacks, no `print`, no
  8-9 digit literal inside a string that could be a copied client id;
- `server-expect.py` evaluates present, absent and max directives correctly;
- every `CI: <name>` in docs/guides/human-checks.md names a scenario or log-check.

Stdlib unittest; reads files only.
"""
import ast
import functools
import importlib.util
import pathlib
import re
import symtable
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


# The builtins of Python 2.7 (not the host's): the scenarios run there.
PY27_BUILTINS = frozenset("""
ArithmeticError AssertionError AttributeError BaseException BufferError BytesWarning
DeprecationWarning EOFError Ellipsis EnvironmentError Exception False FloatingPointError
FutureWarning GeneratorExit IOError ImportError ImportWarning IndentationError IndexError
KeyError KeyboardInterrupt LookupError MemoryError NameError None NotImplemented
NotImplementedError OSError OverflowError PendingDeprecationWarning ReferenceError
RuntimeError RuntimeWarning StandardError StopIteration SyntaxError SyntaxWarning
SystemError SystemExit TabError True TypeError UnboundLocalError UnicodeDecodeError
UnicodeEncodeError UnicodeError UnicodeTranslateError UnicodeWarning UserWarning
ValueError Warning WindowsError ZeroDivisionError
__debug__ __doc__ __name__ __package__ __import__
abs all any apply basestring bin bool buffer bytearray bytes callable chr classmethod
cmp coerce compile complex copyright credits delattr dict dir divmod enumerate eval
execfile exit file filter float format frozenset getattr globals hasattr hash help hex
id input int intern isinstance issubclass iter len license list locals long map max
memoryview min next object oct open ord pow print property quit range raw_input reduce
reload repr reversed round set setattr slice sorted staticmethod str sum super tuple
type unichr unicode vars xrange zip
""".split())
# Names the client installs into `__builtin__`, so a bare read resolves there
# (the prelude's `builtin(name)` reads them the same way).  Kept to the four
# the scenarios use; each is verified in the decompiled client (out/research/_w/src):
#   sm       [V] carbon/common/script/sys/serviceManager.py:53 `__builtin__.sm = self`
#   session  [V] autoexec_client_core.py:103 `__builtin__.session = session`
#   const    [V] builtinmangler.py:20 `__builtin__.const = constmodule`
#   cfg      [V] carbon/common/script/sys/cfg.py:45 `__builtin__.cfg = ...`
# `uicore` is NOT one of them: the client imports it (`from carbonui.uicore import uicore`).
CLIENT_BUILTINS = frozenset(["sm", "session", "const", "cfg"])


@functools.lru_cache(maxsize=None)
def undefined_names(source):
    """[(name, line)] for every name read inside `eoci_main` that no enclosing
    function, class-free scope chain, module binding or Python 2.7 builtin
    binds.  `symtable` does the scoping (comprehensions, lambdas, defaults,
    global, except-as, with-as, imports, loop targets, class bodies); two
    Python 2 differences are patched: a list comprehension's variable lives in
    the enclosing function, and a `global x` that assigns binds x at module
    level.  Flow is not modelled: a binding anywhere in a scope counts, so order,
    branches and `del` are not checked.  Returns a tuple of (name, line)."""
    # the prelude still writes `except E, e:` and `10L`; same lines, py3 syntax
    source = re.sub(r"(?m)^(\s*except\s+[^:\n]+?)\s*,\s*(\w+)\s*:", r"\1 as \2:", source)
    source = re.sub(r"\b(\d+)[lL]\b", r"\1", source)
    top = symtable.symtable(source, "scenario", "exec")
    tree = ast.parse(source)
    mod_bound = {s.get_name() for s in top.get_symbols()
                 if s.is_assigned() or s.is_imported() or s.is_namespace()}

    def tables(t):
        yield t
        for c in t.get_children():
            yield from tables(c)

    mains = [c for c in top.get_children() if c.get_name() == "eoci_main"]
    if len(mains) != 1:
        return [("eoci_main", 0)]
    main = mains[0]
    for t in tables(main):
        for s in t.get_symbols():
            if s.is_declared_global() and s.is_assigned():
                mod_bound.add(s.get_name())

    scopes = {}  # (name, line) -> ast node, to find a table's own list comprehensions
    for x in ast.walk(tree):
        if isinstance(x, (ast.FunctionDef, ast.ClassDef)):
            scopes[(x.name, x.lineno)] = x
        elif isinstance(x, ast.Lambda):
            scopes[("lambda", x.lineno)] = x

    def listcomp_targets(t):
        """The names the list comprehensions written directly in this scope
        bind: Python 2 leaves them in the scope, Python 3 hides them."""
        node = scopes.get((t.get_name(), t.get_lineno()))
        if node is None:
            return set()
        out = set()

        def go(n):
            if isinstance(n, ast.ListComp):
                for g in n.generators:
                    out.update(y.id for y in ast.walk(g.target) if isinstance(y, ast.Name))
            if isinstance(n, (ast.GeneratorExp, ast.SetComp, ast.DictComp)):
                go(n.generators[0].iter)  # their own scope; only the first iterable is ours
                return
            for c in ast.iter_child_nodes(n):
                if not isinstance(c, (ast.arguments, ast.FunctionDef, ast.ClassDef, ast.Lambda)):
                    go(c)
        go(node)
        return out

    flagged = []

    def visit(t, leaks):
        here = set(leaks) | listcomp_targets(t)
        for s in t.get_symbols():
            n = s.get_name()
            if (s.is_referenced() and s.is_global() and n not in mod_bound
                    and n not in PY27_BUILTINS and n not in CLIENT_BUILTINS
                    and n not in here):
                flagged.append((n, t))
        for c in t.get_children():
            if c.get_type() != "annotation":
                visit(c, leaks if t.get_type() == "class" else here)

    visit(main, set())
    out = []
    for n, t in flagged:
        node = scopes.get((t.get_name(), t.get_lineno()))
        lines = [x.lineno for x in ast.walk(node) if isinstance(x, ast.Name)
                 and x.id == n and isinstance(x.ctx, ast.Load)]
        out.append((n, min(lines) if lines else t.get_lineno()))
    return tuple(sorted(out, key=lambda x: (x[1], x[0])))


# (scenario, name) pairs that really are unbound and wait for their owner to fix.
KNOWN_UNBOUND = frozenset()  # (scenario, name, line); a new read of the name is not silenced


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

    def test_no_name_is_read_that_nothing_binds(self):
        """A name read in `eoci_main` that is bound nowhere in its scope chain
        dies with a NameError mid-run on a client (a `time.time()` with no
        `import time`; a variable whose assignment an edit deleted)."""
        bad = []
        for n in NAMES:
            src = (SCEN / f"{n}.py").read_text()
            for name, line in undefined_names(src):
                if (n, name, line) not in KNOWN_UNBOUND:
                    bad.append("%s.py:%d: %s is not bound anywhere it can be reached" % (n, line, name))
        self.assertEqual(bad, [], "unbound names:\n" + "\n".join(bad))

    def test_the_known_list_has_no_stale_entries(self):
        live = {(n, name, line) for n in NAMES
                for name, line in undefined_names((SCEN / f"{n}.py").read_text())}
        self.assertEqual(KNOWN_UNBOUND - live, set())


class UndefinedNames(unittest.TestCase):
    """Self-tests for `undefined_names` on tiny inline sources."""

    @staticmethod
    def _flags(body):
        src = "import os\nMOD = 1\ndef eoci_main(N):\n" + textwrap.indent(
            textwrap.dedent(body).strip("\n"), "    ") + "\n"
        return [n for n, _ in undefined_names(src)]

    def test_an_unbound_name_is_flagged(self):
        self.assertEqual(self._flags("x = time.time()"), ["time"])
        self.assertEqual(self._flags("def f():\n    return gone\nf()"), ["gone"])
        self.assertEqual(self._flags("a = 1\ndel a\ndef f():\n    return a\n"), [])  # still a closure cell
        self.assertEqual(self._flags("def f():\n    y = 1\nz = y"), ["y"])

    def test_every_binding_form_passes(self):
        ok = [
            "import time\nx = time.time()",
            "def f():\n    import time\n    return time\nf()",
            "from os import path\nx = path",
            "x = 1\ndef f():\n    return x\nf()",
            "def g():\n    return h()\ndef h():\n    return 1\ng()",
            "for i in range(3):\n    pass\nx = i",
            "for a, (b, c) in []:\n    pass\nx = a + b + c",
            "try:\n    pass\nexcept Exception as e:\n    x = e",
            "with open('x') as fh:\n    x = fh",
            "x = [i for i in range(3)]",
            "x = [j for i in [[1]] for j in i]",
            "x = [i for i in range(3)]\ny = i",  # py2 list comprehension leaks
            "x = {k: v for k, v in []}",
            "x = set(i for i in [])",
            "f = lambda a, b=MOD: a + b + N",
            "def f(a, b=MOD, *r, **kw):\n    return a, b, r, kw\nf(1)",
            "x = 1\ndef f():\n    global x\n    x = 2\nf()",
            "def f():\n    global late\n    late = 1\nf()\ny = late",
            "class C(object):\n    a = 1\n    b = a + 1\n    def m(self):\n        return self\nz = C",
            "x = unicode('a') + str(long(1)) + str(list(xrange(2))) + basestring.__name__ + unichr(1)",
            "y = [reduce, execfile, raw_input, file, buffer, cmp, apply, intern, coerce, reload]",
            "x = MOD + len(os.sep)",
            "def f():\n    a = 1\n    def g():\n        return a\n    return g\nf()",
        ]
        for body in ok:
            with self.subTest(body):
                self.assertEqual(self._flags(body), [])

    def test_a_class_body_does_not_bind_for_its_methods(self):
        self.assertEqual(self._flags("class C(object):\n    a = 1\n    def m(self):\n        return a\n"), ["a"])

    def test_a_list_comprehension_variable_stays_in_its_own_function(self):
        self.assertEqual(self._flags("def a():\n    return [k for k in []]\ndef b():\n    return k\n"), ["k"])
        self.assertEqual(self._flags("def a():\n    return [k for k in []]\ndef b(n):\n    return n.foo\n"), [])
        self.assertEqual(self._flags("def b():\n    return n.foo\ndef a():\n    return [n for n in []]\n"), ["n"])
        self.assertEqual(self._flags("x = list([w for w in r] for r in [[1]])\ny = w"), ["w"])

    def test_py3_only_builtins_are_not_py27(self):
        self.assertEqual(self._flags("x = ascii(1)"), ["ascii"])

    def test_names_outside_eoci_main_are_not_checked(self):
        src = "def other():\n    return nothing\ndef eoci_main(N):\n    return N\n"
        self.assertEqual(undefined_names(src), ())

    def test_client_builtins_resolve_bare(self):
        self.assertEqual(self._flags("x = (sm, session, const, cfg)"), [])
        self.assertEqual(self._flags("x = uicore"), ["uicore"])


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
