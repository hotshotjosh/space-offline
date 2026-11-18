#!/usr/bin/env python3
"""Fail when a test reaches a non-standard-library import at module level (#292).

A contributor's host has the standard library only. A `tools/test_*.py` that
imports (directly or through a tool it loads) a third-party module at import
time is red there, whatever the test does. Imports inside functions, or in a
`try` that catches ImportError, are fine; so is a module loaded through
`testenv.optional_load`.

    python3 -I tools/import_audit.py [tools-dir]

Exits 1 and prints one line per violation. Standard library only.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line.
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import ast
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_LIT = re.compile(r"""^([A-Za-z_][\w-]*)\.py$""")


def _stem(name):
    return name.replace("-", "_")


def _local_names(tools_dir):
    """stem (with - as _) -> file name, for tools/*.py."""
    return {_stem(n[:-3]): n for n in os.listdir(tools_dir) if n.endswith(".py")}


def _parse(path):
    try:
        with open(path, encoding="utf-8") as f:
            return ast.parse(f.read(), path)
    except (SyntaxError, UnicodeDecodeError, OSError):
        return None


def _catches_import_error(handlers):
    for h in handlers:
        t = h.type
        if t is None:
            return True
        names = [t] if not isinstance(t, ast.Tuple) else list(t.elts)
        for n in names:
            if isinstance(n, ast.Name) and n.id in ("ImportError", "ModuleNotFoundError", "Exception", "BaseException"):
                return True
    return False


def _toplevel_imports(body):
    """(lineno, top module name) for imports at module level, through `if`/`try`
    (a `try` that catches ImportError is skipped), `with`, and loops."""
    out = []
    for node in body:
        if isinstance(node, ast.Import):
            out += [(node.lineno, a.name.split(".")[0]) for a in node.names]
        elif isinstance(node, ast.ImportFrom):
            if node.level == 0 and node.module:
                out.append((node.lineno, node.module.split(".")[0]))
        elif isinstance(node, ast.If):
            out += _toplevel_imports(node.body) + _toplevel_imports(node.orelse)
        elif isinstance(node, (ast.Try, getattr(ast, "TryStar", ast.Try))):
            if not _catches_import_error(node.handlers):
                out += _toplevel_imports(node.body)
            out += _toplevel_imports(node.orelse) + _toplevel_imports(node.finalbody)
            for h in node.handlers:
                out += _toplevel_imports(h.body)
        elif isinstance(node, getattr(ast, "Match", ())):
            for case in node.cases:
                out += _toplevel_imports(case.body)
        elif isinstance(node, (ast.With, ast.AsyncWith, ast.For, ast.AsyncFor, ast.While, ast.ClassDef)):
            out += _toplevel_imports(node.body)
    return out


def _optional_names(tree):
    """String literals passed as the first argument of `*.optional_load(...)`."""
    names = set()
    for n in ast.walk(tree):
        if isinstance(n, ast.Call) and isinstance(n.func, (ast.Attribute, ast.Name)):
            f = n.func.attr if isinstance(n.func, ast.Attribute) else n.func.id
            if f == "optional_load" and n.args and isinstance(n.args[0], ast.Constant) \
                    and isinstance(n.args[0].value, str):
                names.add(_stem(n.args[0].value))
    return names


def _reach(tree, local, skip):
    """Local module stems a parsed file imports at module level or names as a literal."""
    found = set()
    for _line, name in _toplevel_imports(tree.body):
        if _stem(name) in local:
            found.add(_stem(name))
    for n in ast.walk(tree):
        if not (isinstance(n, ast.Constant) and isinstance(n.value, str)):
            continue
        v = n.value
        m = _LIT.match(os.path.basename(v)) if v.endswith(".py") else None
        if m:
            cands = [m.group(1)]
        elif re.fullmatch(r"[A-Za-z_][\w-]*", v):
            cands = [v]  # `_load("x")`, `spec_from_file_location("x", ...)`
        else:
            cands = []
        for c in cands:
            if _stem(c) in local and _stem(c) not in skip:
                found.add(_stem(c))
    return found


def violations(tools_dir=_HERE):
    local = _local_names(tools_dir)
    stdlib = set(sys.stdlib_module_names) | {"__future__"}
    out = []
    reported = set()
    tests = sorted(n for n in os.listdir(tools_dir) if n.startswith("test_") and n.endswith(".py"))
    for t in tests:
        tree = _parse(os.path.join(tools_dir, t))
        if tree is None:
            continue
        skip = _optional_names(tree)
        seen = {}
        stack = [(m, t) for m in _reach(tree, local, skip)]
        for line, name in _toplevel_imports(tree.body):
            if name not in stdlib and _stem(name) not in local:
                key = (t, line)
                if key not in reported:
                    reported.add(key)
                    out.append("%s:%d imports %s at module level, reached from %s" % (t, line, name, t))
        while stack:
            mod, via = stack.pop()
            if mod in seen:
                continue
            seen[mod] = via
            mtree = _parse(os.path.join(tools_dir, local[mod]))
            if mtree is None:
                continue
            mskip = _optional_names(mtree)
            for line, name in _toplevel_imports(mtree.body):
                if name in stdlib or _stem(name) in local:
                    continue
                key = (local[mod], line)
                if key not in reported:
                    reported.add(key)
                    out.append("%s:%d imports %s at module level, reached from %s" % (local[mod], line, name, t))
            stack += [(m, mod) for m in _reach(mtree, local, mskip) if m not in seen]
    return out


def main(argv):
    d = argv[1] if len(argv) > 1 else _HERE
    bad = violations(d)
    for line in bad:
        print(line)
    if bad:
        print("import_audit: %d violation(s): a standard-library host cannot import these" % len(bad), file=sys.stderr)
        return 1
    print("import_audit: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
