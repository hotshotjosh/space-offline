#!/usr/bin/env python3
"""Fail if the server could build against a crate that is not on the allowlist.

Checks, all from files on disk (no cargo, no network; review V6 F11, IP20;
policy: docs/reference/dependencies.md). server/deps.allow is a tab-separated
table, one row per external crate, and every crate a direct one pulls in:

    name  version  direct | via PARENT  licence  build(yes|no)  unsafe  used-in

1. Every `direct` row appears in [workspace.dependencies] and every workspace
   dependency has a `direct` row. A `via PARENT` row names a parent that is a
   row too, and must be in Cargo.lock.
2. Every crate dependency is a local path or `workspace = true`, so no crate
   can name a git URL or another registry.
3. Every Cargo.lock package with a `source` matches a row on name AND version
   (a version bump is a guarded diff), comes from the crates.io index (which
   config.toml replaces with the distro directory) and has a `checksum` key.
   The distro registry has no checksums, so cargo writes a placeholder text
   there; rule 9 is the real lock.
4. No `[patch]` or `[replace]` in any Cargo.toml or in server/.cargo/config.toml.
5. server/.cargo/config.toml keeps `[net] offline = true` and
   `[source.crates-io] replace-with = "distro"`, and `distro` is a
   `directory` source. Those lines are what make rule 3 hold for builds.
6. No crate of ours has a build script (`crates/*/build.rs`, or a `build = "..."`
   key) unless its name is in BUILD_RS_ALLOW.
7. Every row's licence is on the accepted list (LICENCE_OK); an owner-approved
   exception goes in LICENCE_EXCEPT by crate name.
8. A `direct` row's `used-in` file is the only .rs file under server/crates
   that names the crate (`name::`, `use name`, `extern crate name`); a `via`
   row's crates are named nowhere. A crate-using file must exist.
9. Registry mode, run in the build image:
   `tools/dev.sh python3 -I /repo/tools/check_deps.py --registry /usr/share/cargo/registry`
   Every row's crate directory exists with the row's version, licence and
   build-script flag, its tree digest equals the row in server/deps.digest,
   and server/THIRD-PARTY.txt is what tools/dep-notices.py writes now.
   `--print-digest` (same arguments) prints the deps.digest to commit.
   Without --registry, the host mode only checks deps.digest names the same
   crates and versions as deps.allow, and that THIRD-PARTY.txt exists.

Standard library only.
"""
# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import hashlib
import pathlib
import re
import sys

if sys.version_info < (3, 11):
    sys.exit("check_deps: python 3.11 or newer is needed (tomllib); this is %d.%d" % sys.version_info[:2])

import tomllib  # noqa: E402

SERVER = pathlib.Path(__file__).resolve().parent.parent / "server"
DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")
# Lock sources an allowlisted crate may come from. The distro directory
# registry stands in for crates.io, so Cargo.lock records the crates.io index.
LOCK_SOURCES = ("registry+https://github.com/rust-lang/crates.io-index",)
# Crates of OURS allowed a build script, by package directory name. Empty: a
# dependency's build script is declared in deps.allow (the `build` column).
BUILD_RS_ALLOW = frozenset()
# SPDX identifiers the owner has accepted (docs/reference/dependencies.md).
LICENCE_OK = frozenset({
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "0BSD",
    "BSL-1.0", "Unicode-3.0", "Unicode-DFS-2016", "CC0-1.0", "Unlicense",
})
# Crates with a licence off the list that the owner approved by name.
LICENCE_EXCEPT = frozenset()
ROW_FIELDS = ("name", "version", "kind", "licence", "build", "unsafe", "used_in")
DIGEST_NAME = "deps.digest"
NOTICES_NAME = "THIRD-PARTY.txt"


def load(path):
    with open(path, "rb") as f:
        return tomllib.load(f)


def parse_allow(text):
    """Return (rows, problems) from deps.allow text. A row is a dict keyed by ROW_FIELDS."""
    rows, problems = [], []
    for n, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        cells = [c.strip() for c in line.split("\t")]
        if len(cells) != len(ROW_FIELDS):
            problems.append(f"line {n}: want {len(ROW_FIELDS)} tab-separated fields "
                            f"({' '.join(ROW_FIELDS)}), got {len(cells)}")
            continue
        row = dict(zip(ROW_FIELDS, cells))
        row["line"] = n
        if not all(row[f] for f in ROW_FIELDS):
            problems.append(f"line {n}: '{row['name']}' has an empty field (use - for none)")
            continue
        if row["build"] not in ("yes", "no"):
            problems.append(f"line {n}: '{row['name']}' build must be yes or no")
        if row["kind"] != "direct" and not re.fullmatch(r"via \S+", row["kind"]):
            problems.append(f"line {n}: '{row['name']}' kind must be `direct` or `via PARENT`")
        rows.append(row)
    return rows, problems


def licence_ok(expr):
    """True if an SPDX expression of `AND`/`OR` over accepted ids is acceptable."""
    expr = expr.replace("(", " ").replace(")", " ")
    for group in re.split(r"\s+AND\s+", expr.strip()):
        terms = [t.strip() for t in re.split(r"\s+OR\s+", group.strip())]
        if not any(t in LICENCE_OK for t in terms):
            return False
    return True


def uses_crate(text, name):
    """Whether Rust source `text` names crate `name` outside comment lines."""
    ident = re.escape(name.replace("-", "_"))
    pat = re.compile(rf"(?<![\w:]){ident}::|\buse\s+{ident}\b|\bextern\s+crate\s+{ident}\b")
    return any(pat.search(ln) for ln in text.splitlines() if not ln.lstrip().startswith("//"))


def tree_digest(directory):
    """SHA-256 over the sorted `relative path NUL file-sha256` lines of a tree."""
    directory = pathlib.Path(directory)
    h = hashlib.sha256()
    for f in sorted(p for p in directory.rglob("*") if p.is_file()):
        rel = f.relative_to(directory).as_posix()
        h.update(f"{rel}\0{hashlib.sha256(f.read_bytes()).hexdigest()}\n".encode())
    return h.hexdigest()


def digest_lines(rows, registry):
    """The deps.digest text for `rows` taken from the registry directory."""
    out = ["# name\tversion\ttree digest (tools/check_deps.py --registry DIR --print-digest)"]
    for r in sorted(rows, key=lambda r: (r["name"], r["version"])):
        out.append(f"{r['name']}\t{r['version']}\t{tree_digest(pathlib.Path(registry) / (r['name'] + '-' + r['version']))}")
    return "\n".join(out) + "\n"


def parse_digest(text):
    return {(c[0], c[1]): c[2] for c in
            (ln.split("\t") for ln in text.splitlines() if ln.strip() and not ln.startswith("#"))
            if len(c) == 3}


def load_notices():
    import importlib.util
    spec = importlib.util.spec_from_file_location(
        "dep_notices", pathlib.Path(__file__).resolve().parent / "dep-notices.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def check(server, registry=None):
    """Return (errors, summary) for the server tree rooted at `server`."""
    errors = []
    top = server.parent

    def rel(p):
        return p.relative_to(top).as_posix()

    allow_path = server / "deps.allow"
    rows, problems = parse_allow(allow_path.read_text())
    errors += [f"{rel(allow_path)}: {p}" for p in problems]
    by_name = {}
    for r in rows:
        by_name.setdefault(r["name"], []).append(r)
    by_key = {(r["name"], r["version"]): r for r in rows}
    direct = {r["name"] for r in rows if r["kind"] == "direct"}
    for r in rows:
        parent = r["kind"][4:] if r["kind"].startswith("via ") else None
        if parent is not None and parent not in by_name:
            errors.append(f"{rel(allow_path)}: '{r['name']}' is via '{parent}', which is not a row")
        if r["kind"] == "direct" and r["used_in"] == "-":
            errors.append(f"{rel(allow_path)}: direct crate '{r['name']}' needs a used-in file")
        if parent is not None and r["used_in"] != "-":
            errors.append(f"{rel(allow_path)}: '{r['name']}' is via '{parent}' and takes used-in '-'")

    root_path = server / "Cargo.toml"
    root = load(root_path)
    workspace_deps = root.get("workspace", {}).get("dependencies", {})
    for name, spec in workspace_deps.items():
        if name not in direct:
            errors.append(f"{rel(root_path)}: '{name}' has no `direct` row in deps.allow")
        if isinstance(spec, dict) and any(k in spec for k in ("git", "registry", "path")):
            errors.append(f"{rel(root_path)}: '{name}' must come from the distro registry")

    def no_overrides(path, data):
        for key in ("patch", "replace"):
            if key in data:
                errors.append(f"{rel(path)}: [{key}] is not allowed")

    no_overrides(root_path, root)

    for manifest in sorted(server.glob("crates/*/Cargo.toml")):
        data = load(manifest)
        no_overrides(manifest, data)
        crate = manifest.parent.name
        tables = [data.get(t, {}) for t in DEP_TABLES]
        for target in data.get("target", {}).values():
            tables += [target.get(t, {}) for t in DEP_TABLES]
        for table in tables:
            for name, spec in table.items():
                if isinstance(spec, dict) and "path" in spec:
                    continue
                if isinstance(spec, dict) and spec.get("workspace") is True:
                    if name not in workspace_deps:
                        errors.append(f"{rel(manifest)}: '{name}' is not in [workspace.dependencies]")
                    continue
                errors.append(f"{rel(manifest)}: '{name}' must be a path or `workspace = true` dependency")
        build = data.get("package", {}).get("build")
        if build not in (None, False) and crate not in BUILD_RS_ALLOW:
            errors.append(f"{rel(manifest)}: `build = {build!r}` is not in BUILD_RS_ALLOW")
        script = manifest.parent / "build.rs"
        if script.exists() and crate not in BUILD_RS_ALLOW and build is not False:
            errors.append(f"{rel(script)}: build script is not in BUILD_RS_ALLOW")

    for name in sorted(direct - set(workspace_deps)):
        errors.append(f"{rel(allow_path)}: '{name}' is allowed but unused; remove it")

    lock_path = server / "Cargo.lock"
    lock = load(lock_path)
    external = []
    locked = set()
    for pkg in lock.get("package", []):
        if "source" not in pkg:
            continue
        name, version, source = pkg.get("name", "?"), pkg.get("version", "?"), pkg["source"]
        external.append(f"{name}-{version}")
        locked.add((name, version))
        if (name, version) not in by_key:
            have = ", ".join(r["version"] for r in by_name.get(name, ()))
            errors.append(f"{rel(lock_path)}: '{name}' {version} from {source} is not in deps.allow"
                          + (f" (allowed: {have})" if have else ""))
        elif not source.startswith(LOCK_SOURCES):
            errors.append(f"{rel(lock_path)}: '{name}' comes from {source}, not the distro registry")
        if "checksum" not in pkg:
            errors.append(f"{rel(lock_path)}: '{name}' {version} has no checksum key")
    for r in rows:
        if (r["name"], r["version"]) not in locked:
            errors.append(f"{rel(allow_path)}: '{r['name']}' {r['version']} is not in Cargo.lock; remove or fix the row")

    cfg_path = server / ".cargo" / "config.toml"
    if not cfg_path.exists():
        errors.append(f"{rel(cfg_path)}: missing; builds would reach crates.io")
    else:
        cfg = load(cfg_path)
        no_overrides(cfg_path, cfg)
        if cfg.get("net", {}).get("offline") is not True:
            errors.append(f"{rel(cfg_path)}: [net] offline = true is missing")
        sources = cfg.get("source", {})
        if sources.get("crates-io", {}).get("replace-with") != "distro":
            errors.append(f'{rel(cfg_path)}: [source.crates-io] replace-with = "distro" is missing')
        distro = sources.get("distro", {})
        if set(distro) != {"directory"}:
            errors.append(f"{rel(cfg_path)}: [source.distro] must be exactly one `directory` key")

    # Rule 7: licences.
    for r in rows:
        if r["name"] not in LICENCE_EXCEPT and not licence_ok(r["licence"]):
            errors.append(f"{rel(allow_path)}: '{r['name']}' licence '{r['licence']}' is not on the accepted list")

    # Rule 8: one using file.
    sources_rs = sorted((server / "crates").glob("**/*.rs"))
    for r in rows:
        allowed = None if r["used_in"] == "-" else (server / "crates" / r["used_in"])
        if allowed is not None and not allowed.is_file():
            errors.append(f"{rel(allow_path)}: '{r['name']}' used-in {r['used_in']} does not exist")
        for f in sources_rs:
            if f == allowed:
                continue
            if uses_crate(f.read_text(errors="replace"), r["name"]):
                errors.append(f"{rel(f)}: names crate '{r['name']}', which may be used only in "
                              f"{r['used_in'] if allowed else 'no file of ours'}")

    # Digest and notices: host consistency, then the registry check.
    digest_path, notices_path = server / DIGEST_NAME, server / NOTICES_NAME
    digests = {}
    if rows:
        if not digest_path.exists():
            errors.append(f"{rel(digest_path)}: missing")
        else:
            digests = parse_digest(digest_path.read_text())
            if set(digests) != set(by_key):
                errors.append(f"{rel(digest_path)}: names {sorted(digests)}, deps.allow names {sorted(by_key)}")
        if not notices_path.exists():
            errors.append(f"{rel(notices_path)}: missing")
    if registry is not None:
        reg = pathlib.Path(registry)
        for r in rows:
            d = reg / f"{r['name']}-{r['version']}"
            if not d.is_dir():
                errors.append(f"registry: {d} does not exist")
                continue
            try:
                manifest = load(d / "Cargo.toml")
            except (OSError, tomllib.TOMLDecodeError) as e:
                errors.append(f"registry: {d.name}/Cargo.toml unreadable: {e}")
                continue
            pkg = manifest.get("package", {})
            if pkg.get("version") != r["version"]:
                errors.append(f"registry: {d.name} says version {pkg.get('version')}")
            if pkg.get("license") != r["licence"]:
                errors.append(f"registry: {d.name} licence is '{pkg.get('license')}', row says '{r['licence']}'")
            build = pkg.get("build")
            has_build = build not in (None, False) or (build is None and (d / "build.rs").exists())
            if has_build != (r["build"] == "yes"):
                errors.append(f"registry: {d.name} build script is {'present' if has_build else 'absent'}, "
                              f"row says build={r['build']}")
            want = digests.get((r["name"], r["version"]))
            if want is not None and tree_digest(d) != want:
                errors.append(f"registry: {d.name} digest differs from {DIGEST_NAME}; the image's crate "
                              f"changed (review it, then regenerate with --print-digest)")
        if rows and notices_path.exists():
            if notices_path.read_text() != load_notices().render(rows, reg):
                errors.append(f"{rel(notices_path)}: stale; regenerate with tools/dep-notices.py")

    summary = (f"deps: {len(workspace_deps)} direct, {len(external)} external in Cargo.lock"
               + (f": {' '.join(sorted(external))}" if external else ""))
    return errors, summary


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    registry, print_digest = None, False
    args = list(argv)
    while args:
        a = args.pop(0)
        if a == "--registry" and args:
            registry = args.pop(0)
        elif a == "--print-digest":
            print_digest = True
        else:
            print("usage: check_deps.py [--registry DIR [--print-digest]]", file=sys.stderr)
            return 2
    if print_digest:
        if registry is None:
            print("check_deps: --print-digest needs --registry", file=sys.stderr)
            return 2
        rows, problems = parse_allow((SERVER / "deps.allow").read_text())
        if problems:
            print("\n".join(problems), file=sys.stderr)
            return 1
        sys.stdout.write(digest_lines(rows, registry))
        return 0
    errors, summary = check(SERVER, registry)
    print(summary)
    for e in errors:
        print(f"deps: FAIL {e}", file=sys.stderr)
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
