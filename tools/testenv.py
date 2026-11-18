"""Helpers for tests that must pass on a contributor's host (#292 class).

Standard library only. Not a test itself (the name does not match `test_*.py`).
A test imports it with `sys.path.append(HERE)` (last, so tools/dis.py does not shadow the stdlib), then `import testenv`.

- `hermetic_git_env`: git that ignores the caller's global and system config
  (`commit.gpgsign`, `core.hooksPath`, `init.defaultBranch`) and any `GIT_DIR`.
- `system_path`: a PATH that keeps the running python reachable.
- `clean_env`: the environment without the caller's `EVE_*` variables.
- `needs_module`, `needs_binary`: skip decorators that state why.
- `optional_load`: load a tools module only when a non-stdlib module it needs exists.
"""
import importlib.util
import os
import shutil
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))

_GIT_DROP = (
    "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES", "GIT_CEILING_DIRECTORIES",
)


def hermetic_git_env(base=None, **extra):
    """A copy of `base` (default `os.environ`) in which git reads no user config."""
    env = dict(os.environ if base is None else base)
    for k in _GIT_DROP:
        env.pop(k, None)
    env.update(
        GIT_CONFIG_GLOBAL=os.devnull,
        GIT_CONFIG_NOSYSTEM="1",
        GIT_AUTHOR_NAME="t",
        GIT_AUTHOR_EMAIL="t@test.invalid",
        GIT_COMMITTER_NAME="t",
        GIT_COMMITTER_EMAIL="t@test.invalid",
    )
    env.update(extra)
    return env


def system_path(*front):
    """`front`, then the running python's directory, then /usr/bin and /bin, no repeats."""
    parts = list(front) + [os.path.dirname(sys.executable), "/usr/bin", "/bin"]
    seen = []
    for p in parts:
        if p and p not in seen:
            seen.append(p)
    return os.pathsep.join(seen)


def clean_env(base=None, **extra):
    """A copy of `base` (default `os.environ`) without any `EVE_*` key, then `extra`."""
    env = {k: v for k, v in (os.environ if base is None else base).items()
           if not k.startswith("EVE_")}
    env.update(extra)
    return env


def has_module(name):
    try:
        return importlib.util.find_spec(name) is not None
    except (ImportError, ValueError):
        return False


def needs_module(name, why):
    return unittest.skipUnless(
        has_module(name),
        "needs the python module %s (%s); a standard-library host skips this" % (name, why))


def needs_binary(name, why):
    return unittest.skipUnless(
        shutil.which(name) is not None,
        "needs the program %s (%s); a host without it skips this" % (name, why))


def optional_load(path_stem, needs):
    """The module `tools/<path_stem>.py`, or None when `needs` is not importable."""
    if not has_module(needs):
        return None
    spec = importlib.util.spec_from_file_location(path_stem, os.path.join(HERE, path_stem + ".py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod
