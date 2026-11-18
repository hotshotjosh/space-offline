#!/usr/bin/env python3
"""Which client-CI scenarios does a client change break? The client symbols each scenario
touches, checked against the new bytecode.

    tools/analysis.sh python3 -I /work/tools/client-ci/fci-surface.py OLD_PYC NEW_PYC
                      [--scenarios tools/client-ci/scenarios] [--out DIR]

Run it in the analysis container (it reads client bytecode; `tools/new-build.sh --report`
does). The scenarios run inside the client and reach its internals; port-recheck checks only
their `path.py:N` citations. This tool reads each scenario's own source (Python 2.7, so a
tokenizer, not `ast`; comments and docstrings are not code) and collects, per line:

  import      `import a.b`, `from a.b import N`            the module and N exist in NEW
  service     `sm.GetService('x')`, `GetServiceIfRunning`,   the service name is a client
              `StartService`, `get_svc('x')`, and the        service in NEW; a method called
              method called on it                            on it still exists and its code
                                                             is unchanged (port-recheck's
                                                             function comparer)
  layer       `uicore.layer.<name>` and up to two more       each name is still a name or
              attributes                                     string in NEW
  settings    `settings.<a>.<b>` and the key of `.Get('k')`  likewise
  module str  `'a.b.c'` naming a module of OLD              the module is in NEW
  name        every `.attr` and identifier-like string       a name that OLD's bytecode holds
                                                             and NEW's no longer does is `gone`

`sm.RemoteSvc` and `sm.ProxySvc` name the server's services, not client code: not checked.
The prelude (`_prelude.py`, `fci-block.txt`) is read once; its copies inside each scenario
are skipped.

Output, in DIR (default <dir of NEW_PYC>/fci-surface when NEW_PYC ends in pyc, else
out/fci-surface/<name>): fci-surface.md (what needs a reader, by scenario) and fci-surface.tsv
(scenario, line, ref, status, detail), every row. Status: ok, gone (in OLD, not in NEW),
changed (the function's code moved), unresolved (neither tree knows it, or the tokenizer
stopped; the row is stale or the check cannot say).

Exit: 0 nothing gone or changed; 3 a row needs a reader (gone, changed, or the tokenizer
stopped); 2 usage or a missing tree; 1 the tool failed. An `unresolved` name that neither tree
knows cannot be broken by this build, so it is listed and counted but does not set exit 3.

[I] A name is "in the client" when it is a co_names entry or a short string constant of some
module of the tree; falsifier: a client symbol that only exists as a dynamic attribute, which
this cannot see (it shows as unresolved or is missed).
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import bisect
import collections
import importlib.util
import io
import os
import re
import sys
import tokenize

_HERE = os.path.dirname(os.path.abspath(__file__))
_TOOLS = os.path.dirname(_HERE)
if _TOOLS not in sys.path:
    sys.path.append(_TOOLS)

import pyc27  # noqa: E402

_spec = importlib.util.spec_from_file_location("port_recheck", os.path.join(_TOOLS, "port-recheck.py"))
pr = importlib.util.module_from_spec(_spec)
sys.modules["port_recheck"] = pr
_spec.loader.exec_module(pr)

CO_OPTIMIZED = 1
SERVICE_CALLS = ("GetService", "GetServiceIfRunning", "StartService", "IsServiceRunning")
SERVICE_FUNCS = ("get_svc", "svc", "_svc")
# The prelude and the fci block are copied into every scenario; cut them out of each copy.
PRE_A = (r"# PRELUDE -- byte-identical.*?# -- the scenario[^\n]*\n", "\n")
FCI = (r"    # -- fci block: .*?    # -- end of the fci block[^\n]*\n", "\n")
PRE_B = (r"# -- end of the scenario body.*\Z", "\n")
STRIP = (PRE_A, FCI, PRE_B)
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")
DOTTED = re.compile(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+\Z")


def text(b):
    return b.decode("utf8", "replace") if isinstance(b, (bytes, bytearray)) else b


# --- reading the trees ---------------------------------------------------------------

class Side(object):
    """One bytecode tree: its modules, vocabulary and client services."""

    def __init__(self, root):
        self.tree = pr.Tree(root)
        self.vocab = set()      # co_names entries and short string constants of every module
        self.services = {}      # service name -> (module path, class name)
        for rel, path in self.tree.paths.items():
            try:
                top = pyc27.load_pyc(path)
            except Exception:   # a damaged file: skip it, the rest still counts
                continue
            for c in pyc27.walk(top):
                for n in c.names or ():
                    self.vocab.add(text(n))
                for k in c.consts or ():
                    if isinstance(k, (bytes, str)) and len(k) < 100:
                        self.vocab.add(text(k))
            for k in top.consts or ():
                if isinstance(k, pyc27.Code) and not (k.flags & CO_OPTIMIZED) and (
                        {text(n) for n in (k.names or ())} & {"__guid__", "__servicename__"}):
                    self._service(rel, k)

    def _service(self, rel, cls):
        prev = None
        for _, op, arg, rep in pyc27.dis(cls):
            if op == "STORE_NAME" and rep in ("__guid__", "__servicename__") and prev is not None:
                v = text(prev)
                if isinstance(v, str):
                    name = v[4:] if rep == "__guid__" and v.startswith("svc.") else v
                    if rep == "__servicename__" or v.startswith("svc."):
                        self.services.setdefault(name, (rel, text(cls.name)))
            prev = cls.consts[arg] if op == "LOAD_CONST" else None

    def has_module(self, mod):
        rel = mod.replace(".", "/")
        return rel if rel in self.tree.paths else ("%s/__init__" % rel if "%s/__init__" % rel in self.tree.paths else None)

    def has_name(self, mod_rel, name):
        """`name` is defined at the top of the module, or is a submodule of it."""
        m = self.tree.module(mod_rel)
        if m is None:
            return False
        if name in m.env():
            return True
        if any(pr.base_key(k).split(".")[0] == name for k in m.entries):
            return True
        pkg = mod_rel[:-len("/__init__")] if mod_rel.endswith("/__init__") else mod_rel
        return ("%s/%s" % (pkg, name)) in self.tree.paths or ("%s/%s/__init__" % (pkg, name)) in self.tree.paths

    def methods(self, rel, cls):
        """Method names in a class body of one module."""
        m = self.tree.module(rel)
        if m is None:
            return set()
        return {pr.base_key(k).split(".", 1)[1] for k in m.entries if pr.base_key(k).startswith(cls + ".")}


# --- reading a scenario --------------------------------------------------------------

class Piece(object):
    __slots__ = ("line", "text", "toks")


def logical_lines(src):
    """`([(first line, text without comments, [(offset, line)])], stopped at line or None)`.
    A string token is kept as written; a comment is dropped."""
    out, cur, offs, n, first, prev = [], [], [], 0, None, None
    stopped = None
    last_row = 0
    try:
        for t in tokenize.generate_tokens(io.StringIO(src).readline):
            if t.type in (tokenize.COMMENT, tokenize.NL, tokenize.INDENT, tokenize.DEDENT, tokenize.ENDMARKER):
                continue
            if t.type == tokenize.NEWLINE or (t.type == tokenize.OP and t.string == ";"):
                if cur and not (len(cur) == 1 and cur[0].lstrip()[:1] in ("'", '"', "u", "U", "r", "R", "b", "B")
                                and string_value(cur[0].strip()) is not None):  # a lone string is a docstring
                    out.append((first, "".join(cur), offs))
                cur, offs, n, first, prev = [], [], 0, None, None
                last_row = t.start[0]
                continue
            if first is None:
                first = t.start[0]
            sep = " " if prev is not None and prev.type in (tokenize.NAME, tokenize.NUMBER) and t.type in (
                tokenize.NAME, tokenize.NUMBER, tokenize.STRING) else ""
            offs.append((n + len(sep), t.start[0]))
            # A multi-line string is prose (a docstring), not a name: keep its place, not its words.
            piece = '""' if t.type == tokenize.STRING and t.start[0] != t.end[0] else t.string
            cur.append(sep + piece)
            n += len(sep) + len(piece)
            prev = t
    except (tokenize.TokenError, IndentationError, SyntaxError) as e:
        stopped = last_row + 1
    if cur:
        out.append((first, "".join(cur), offs))
    return out, stopped


def string_value(tok):
    m = re.match(r"""[uUbBrR]*(['"])(.*)\1\Z""", tok, re.S)
    return m.group(2) if m else None


IMPORT_PLAIN = re.compile(r"import ([\w., ]+)\Z")
SVC_CALL = re.compile(r"(?:\bsm\.(?:%s)|\b(?:%s))\(\s*(['\"])(\w+)\1\s*\)((?:\.\w+)?)(\(?)" % (
    "|".join(SERVICE_CALLS), "|".join(SERVICE_FUNCS)))
LAYER = re.compile(r"\buicore\.layer\.(\w+)((?:\.\w+){0,2})")
SETTINGS = re.compile(r"\bsettings\.((?:\w+\.)*\w+)(?:\(\s*(['\"])(\w+)\2)?")
STR = re.compile(r"""(?<![\w])(?:[uUbB]?)(['"])([A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*)\1""")


def refs_of(src):
    """`([(line, kind, ref, extra)], stopped line or None)` for one source text."""
    lines, stopped = logical_lines(src)
    refs = []

    def at(offs, i):
        k = bisect.bisect_right([o for o, _ in offs], i) - 1
        return offs[max(k, 0)][1]

    for first, body, offs in lines:
        m = re.match(r"from ([\w.]+)\s*import\s*\(?([\w,*\s]+?)\)?\Z", body) or re.match(
            r"from([\w.]+)import\(?([\w,*\s]+?)\)?\Z", body)
        if m:
            for part in m.group(2).split(","):
                words = part.split()  # `b as c`: the name is b; the alias c is the scenario's own
                if words and IDENT.match(words[0]):
                    refs.append((first, "import", m.group(1), words[0]))
            continue
        m = IMPORT_PLAIN.match(body)
        if m:
            for part in m.group(1).split(","):
                mod = part.strip().split(" as ")[0].strip()
                if mod:
                    refs.append((first, "import", mod, ""))
            continue
        for m in SVC_CALL.finditer(body):
            meth = m.group(3)[1:] if m.group(3) and m.group(4) else ""
            refs.append((at(offs, m.start()), "service", m.group(2), meth))
        for m in LAYER.finditer(body):
            chain = [m.group(1)] + [x for x in m.group(2).split(".") if x]
            refs.append((at(offs, m.start()), "layer", ".".join(chain), ""))
        for m in SETTINGS.finditer(body):
            parts = m.group(1).split(".")
            refs.append((at(offs, m.start()), "settings", m.group(1), m.group(3) or ""))
        for m in re.finditer(r"(?<=[\w)\]])\.([A-Za-z_]\w*)", body):
            refs.append((at(offs, m.start()), "name", m.group(1), ""))
        for m in STR.finditer(body):
            v = m.group(2)
            refs.append((at(offs, m.start()), "modstr" if DOTTED.match(v) else "name", v, ""))
    return refs, stopped


# --- checking ------------------------------------------------------------------------

def known(old, new, name):
    """`ok`, `gone` or `unresolved` for one bare name."""
    if name in new.vocab:
        return "ok", ""
    if name in old.vocab:
        return "gone", "in OLD's bytecode, not in NEW's"
    return "unresolved", "in neither tree"


def check(old, new, pair, kind, ref, extra):
    """`[(ref text, status, detail)]` for one reference."""
    out = []
    if kind == "import":
        rel_new, rel_old = new.has_module(ref), old.has_module(ref)
        if rel_new is None and rel_old is None:
            return []  # a C extension or the standard library: not client code
        if rel_new is None:
            return [("import %s" % ref, "gone", "module in OLD, not in NEW")]
        if not extra:
            return [("import %s" % ref, "ok", "")]
        label = "from %s import %s" % (ref, extra)
        if new.has_name(rel_new, extra):
            if rel_old and old.has_name(rel_old, extra):
                st, why = pr.check_symbol(pair, rel_new, extra)
                if st in ("changed", "gone"):
                    return [(label, st, why)]
            return [(label, "ok", "")]
        if rel_old and old.has_name(rel_old, extra):
            return [(label, "gone", "name in OLD's module, not in NEW's")]
        return [(label, "unresolved", "in neither module")]
    if kind == "service":
        label = "service %s" % ref
        n = new.services.get(ref)
        o = old.services.get(ref)
        if n is None:
            return [(label, "gone" if o else "unresolved",
                     "service in OLD, not in NEW" if o else "not a client service in either build")]
        out.append((label, "ok", "%s.%s" % (n[0], n[1])))
        if extra:
            ml = "service %s.%s" % (ref, extra)
            if extra in new.methods(*n):
                if o and extra in old.methods(*o):
                    st, why = pr.check_symbol(pair, n[0], "%s.%s" % (n[1], extra))
                    out.append((ml, st if st in ("changed", "gone") else "ok", why if st in ("changed", "gone") else ""))
                else:
                    out.append((ml, "ok", "new in NEW"))
            elif o and extra in old.methods(*o):
                # not in the class body: an inherited method may still exist
                st, why = ("ok", "defined elsewhere in NEW") if extra in new.vocab else ("gone", "method in OLD's class, nowhere in NEW")
                out.append((ml, st, why))
            else:
                st, why = known(old, new, extra)
                out.append((ml, st, "inherited or dynamic: " + why if st == "ok" else why))
        return out
    if kind == "layer":
        for part in ref.split("."):
            st, why = known(old, new, part)
            out.append(("uicore.layer.%s" % ref if part == ref.split(".")[0] else "  .%s" % part, st, why))
        return [(out[0][0] if len(out) == 1 else "uicore.layer.%s" % ref, worst([s for _, s, _ in out]),
                 "; ".join("%s: %s" % (p, w) for p, (_, s, w) in zip(ref.split("."), out) if s != "ok"))]
    if kind == "settings":
        parts = ref.split(".") + ([extra] if extra else [])
        res = [(p,) + known(old, new, p) for p in parts]
        return [("settings.%s%s" % (ref, ("(%r)" % extra) if extra else ""), worst([r[1] for r in res]),
                 "; ".join("%s: %s" % (p, w) for p, s, w in res if s != "ok"))]
    if kind == "modstr":
        rel_o, rel_n = old.has_module(ref), new.has_module(ref)
        if rel_o and not rel_n:
            return [("module string %s" % ref, "gone", "module in OLD, not in NEW")]
        if rel_n:
            return [("module string %s" % ref, "ok", "")]
        return []  # not a module
    if kind == "name":
        if ref in old.vocab and ref not in new.vocab:
            return [("name %s" % ref, "gone", "in OLD's bytecode, not in NEW's")]
        return []  # in both, or a scenario's own name
    return []


def worst(states):
    for s in ("gone", "changed", "unresolved"):
        if s in states:
            return s
    return "ok"


# --- scenarios -----------------------------------------------------------------------

def scenario_sources(sdir):
    """`[(label, source text)]`: the prelude and fci block once, then each scenario's own body."""
    ci = os.path.dirname(sdir.rstrip(os.sep))
    out = []
    for label, path, strip in [("_prelude", os.path.join(sdir, "_prelude.py"), False),
                               ("fci-block", os.path.join(ci, "fci-block.txt"), False)]:
        if os.path.isfile(path):
            with open(path, encoding="utf8", errors="replace") as fh:
                out.append((label, fh.read()))
    for f in sorted(os.listdir(sdir)):
        if not f.endswith(".py") or f == "_prelude.py":
            continue
        with open(os.path.join(sdir, f), encoding="utf8", errors="replace") as fh:
            src = fh.read()
        for pat, _ in STRIP:
            # Keep the line count: each cut-out is replaced by as many newlines as it held.
            src = re.sub(pat, lambda m: "\n" * m.group(0).count("\n"), src, flags=re.S)
        out.append((f[:-3], src))
    return out


def run(old_root, new_root, sdir):
    old, new = Side(old_root), Side(new_root)
    pair = pr.Pair(old.tree, new.tree)
    rows, seen = [], set()
    for label, src in scenario_sources(sdir):
        refs, stopped = refs_of(src)
        if stopped is not None:
            rows.append((label, stopped, "(tokenizer stopped)", "unresolved", "the rest of the file was not read"))
        for line, kind, ref, extra in refs:
            for text_, st, why in check(old, new, pair, kind, ref, extra):
                key = (label, text_, st)
                if key in seen:
                    continue
                seen.add(key)
                rows.append((label, line, text_, st, why))
    return rows


def write(rows, out, old_root, new_root):
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "fci-surface.tsv"), "w", encoding="utf8") as fh:
        fh.write("scenario\tline\tref\tstatus\tdetail\n")
        for r in rows:
            fh.write("\t".join(str(c).replace("\t", " ").replace("\n", " ") for c in r) + "\n")
    bad = [r for r in rows if r[3] != "ok"]
    counts = collections.Counter(r[3] for r in rows)
    with open(os.path.join(out, "fci-surface.md"), "w", encoding="utf8") as fh:
        fh.write("# fci-surface\n\nOLD %s\nNEW %s\n\n" % (old_root, new_root))
        fh.write("%d references checked: %s.\n\n" % (len(rows), ", ".join("%s %d" % kv for kv in sorted(counts.items()))))
        if not bad:
            fh.write("Nothing needs a reader.\n")
        by = collections.OrderedDict()
        for r in bad:
            by.setdefault(r[0], []).append(r)
        for label, rs in by.items():
            fh.write("## %s\n\n" % label)
            for _, line, ref, st, why in rs:
                fh.write("- line %s: `%s` %s%s\n" % (line, ref, st, (": " + why) if why else ""))
            fh.write("\n")
    return bad


def main(argv=None):
    ap = argparse.ArgumentParser(description="Client-CI scenarios against a new client build's bytecode.")
    ap.add_argument("old_pyc")
    ap.add_argument("new_pyc")
    ap.add_argument("--scenarios", default=os.path.join(_HERE, "scenarios"))
    ap.add_argument("--out")
    a = ap.parse_args(argv)
    for d in (a.old_pyc, a.new_pyc, a.scenarios):
        if not os.path.isdir(d):
            print("fci-surface: %s: not a directory" % d, file=sys.stderr)
            return 2
    new = a.new_pyc.rstrip(os.sep)
    out = a.out or (os.path.join(os.path.dirname(new), "fci-surface") if os.path.basename(new) == "pyc"
                    else os.path.join("out", "fci-surface", os.path.basename(new)))
    try:
        rows = run(a.old_pyc, a.new_pyc, a.scenarios)
        bad = write(rows, out, a.old_pyc, a.new_pyc)
    except Exception as e:  # the tool failed: exit 1, never a silent pass
        print("fci-surface: failed: %s: %s" % (type(e).__name__, e), file=sys.stderr)
        return 1
    print("fci-surface: %d references, %d need a reader; wrote %s" % (len(rows), len(bad), out))
    return 3 if any(r[3] in ("gone", "changed") or r[2] == "(tokenizer stopped)" for r in bad) else 0


if __name__ == "__main__":
    sys.exit(main())
