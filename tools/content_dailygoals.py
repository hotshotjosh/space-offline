"""The `dailygoals` content section: the daily-goal definitions, labels resolved to ids.

Lane DG-1 (#202; `specs/parity-dailygoals-hypernet.md` §2.2). A real
`content.py` section, so `tools/up.sh` extracts it into the bundle and
re-extracts when the authored input changes. Authored, `content/dailygoals/
goals.tsv` (our rows; label paths, never a message id):

    key category method target career name description help weight isk why

Written into the bundle:

    dailygoals/goals.tsv   #key category method target career name_id desc_id
                           help_id weight isk

`derive(rows, labels)` is pure (plain rows and a `{label: message id}` dict in);
the unit tests feed it invented labels. `gather(ctx)` is the only client reader.
"""
import os
import sys

SECTION = "dailygoals"
DEPENDS = ()

_HERE = os.path.dirname(os.path.abspath(__file__))

AUTHORED_HEADER = "key\tcategory\tmethod\ttarget\tcareer\tname\tdescription\thelp\tweight\tisk\twhy"
BUNDLE_HEADER = ("key", "category", "method", "target", "career", "name_id", "desc_id",
                 "help_id", "weight", "isk")
#: `monthly` is added by DG-5.
CATEGORIES = ("daily", "bonus")
#: DG-3 adds `scan_signature` and `manufacture`.
METHODS = ("kill_npc", "mine_ore", "space_jump", "salvage_wreck", "earn_lp", "complete_daily")
CAREERS = ("none", "explorer", "industrialist", "enforcer", "sof")
LABELS_RES = "res:/localizationfsd/localization_fsd_main.pickle"
SOURCES = [LABELS_RES]


class ContentError(ValueError):
    """A refusal: the authored file is wrong or a label does not resolve."""


def parse_goals(lines):
    """The rows of the authored file as dicts. The header must be the first
    non-comment line; any other shape is refused."""
    lines = [l.rstrip("\r") for l in lines]
    lines = [l for l in lines if l.strip() and not l.startswith("#")]
    if not lines or lines[0] != AUTHORED_HEADER:
        raise ContentError("goals.tsv: header is %r, expected %r" % (lines[:1], AUTHORED_HEADER))
    names = AUTHORED_HEADER.split("\t")
    out = []
    for n, l in enumerate(lines[1:], 2):
        f = l.split("\t")
        if len(f) != len(names):
            raise ContentError("goals.tsv: row %d: %d fields, expected %d" % (n, len(f), len(names)))
        out.append(dict(zip(names, f)))
    return out


def _count(row, field, lo):
    try:
        v = int(row[field])
    except ValueError:
        raise ContentError("goal %r: %s %r is not a whole number" % (row["key"], field, row[field]))
    if v < lo:
        raise ContentError("goal %r: %s %d is below %d" % (row["key"], field, v, lo))
    return v


def derive(rows, labels):
    """The bundle rows (tuples in `BUNDLE_HEADER` order) for the authored `rows`.

    `labels` is `{'Path/Label': message id}`. Raises `ContentError` on a label that
    does not resolve, an unknown category, method or career, a negative number, a
    duplicate key, a non-`complete_daily` goal with target 0, a bonus that is
    not `complete_daily` (or the reverse), and more than one `bonus` row."""
    out, seen, bonus = [], set(), 0
    for r in rows:
        key = r["key"]
        if not key or key in seen:
            raise ContentError("goal %r: empty or duplicate key" % key)
        seen.add(key)
        if r["category"] not in CATEGORIES:
            raise ContentError("goal %r: unknown category %r" % (key, r["category"]))
        if r["method"] not in METHODS:
            raise ContentError("goal %r: unknown method %r" % (key, r["method"]))
        if r["career"] not in CAREERS:
            raise ContentError("goal %r: unknown career %r" % (key, r["career"]))
        if (r["category"] == "bonus") != (r["method"] == "complete_daily"):
            raise ContentError("goal %r: category bonus and method complete_daily go together" % key)
        if r["category"] == "bonus":
            bonus += 1
        target = _count(r, "target", 0 if r["method"] == "complete_daily" else 1)
        weight, isk = _count(r, "weight", 0), _count(r, "isk", 0)
        ids = []
        for f in ("name", "description", "help"):
            if r[f] not in labels:
                raise ContentError("goal %r: %s label %r does not resolve" % (key, f, r[f]))
            ids.append(labels[r[f]])
        out.append((key, r["category"], r["method"], target, r["career"], ids[0], ids[1], ids[2],
                    weight, isk))
    if bonus > 1:
        raise ContentError("goals.tsv: %d bonus rows, at most one" % bonus)
    return out


def _lines(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read().splitlines()


def _driver(ctx):
    """The driver module that made `ctx` (`content`, or `__main__` when
    `content.py` is the script): its `ContentError` is the one `main()` reports."""
    return sys.modules[type(ctx).__module__]


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """`{'Path/Label': message id}` from the client's main localisation pickle."""
    loctable = _driver(ctx)._imp("loctable")  # noqa: SLF001
    return loctable.label_index(loctable.load_main(path=ctx.client.path(LABELS_RES)))


def _derive(ctx):
    path = ctx.input(os.path.join(os.path.dirname(_HERE), "content", "dailygoals", "goals.tsv"))
    return derive(parse_goals(_lines(path)), gather(ctx))


def section(ctx):
    try:
        rows = _derive(ctx)
    except ContentError as e:
        raise _driver(ctx).ContentError("dailygoals: %s" % e) from None
    ctx.note("dailygoals: %d goals" % len(rows))
    return [("dailygoals/goals.tsv", BUNDLE_HEADER, rows, SOURCES)]


def check(ctx):
    content = _content()
    path = ctx.path("dailygoals/goals.tsv")
    if not os.path.exists(path):
        return ["dailygoals/goals.tsv: missing from the bundle"]
    try:
        want = _derive(ctx)
    except ContentError as e:
        return ["dailygoals: %s" % e]
    got = list(content.read_rows(path))
    if got != [content.encode_row(r).split("\t") for r in want]:
        return ["dailygoals/goals.tsv: differs from a fresh derive (%d vs %d rows)"
                % (len(got), len(want))]
    ctx.note("dailygoals: %d goals match a fresh derive" % len(want))
    return []
