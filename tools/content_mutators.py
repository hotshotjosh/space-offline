"""The `mutators` content section: the client's mutaplasmid ranges and mappings.

Issue #222 MUTA-DATA (lane L1 of the mutaplasmid parity spec). Writes two bundle
files, every header `docs/content/schema.md`'s:

    mutators/ranges.tsv     one row per (mutator, attribute): the roll window
                            `min .. max` (a factor of the source type's value)
                            and whether a high value is good
    mutators/mappings.tsv   one row per (mutator, result type, source type):
                            a mutator turns the source into the result

The table is `res:/staticdata/dynamicitemattributes.fsdbinary`, a cFSD table
decoded by `fsd.rows` (layout: `findings/pyd-schemas.tsv`, stem
`dynamicitemattributes`) [V]. The roll formula is the client's
(`dynamicitemattributes/__init__.py`) and is the lane that applies the ranges,
not this one.

The derivation is pure (plain dicts in, rows out): the unit tests feed it
invented rows; `gather(ctx)` is the only bundle reader.
"""

import math
import os
import sys

SECTION = "mutators"
DEPENDS = ("types",)
CFSD = ("dynamicitemattributes", "types")

_HERE = os.path.dirname(os.path.abspath(__file__))

RES = "res:/staticdata/dynamicitemattributes.fsdbinary"
RANGES_HEADER = tuple("mutator_type_id attribute_id min max high_is_good".split())
MAPPINGS_HEADER = tuple("mutator_type_id result_type_id source_type_id".split())
#: Every mutator type's group [V-data]; checked, not trusted.
MUTATOR_GROUP = 1964

#: Bundle files that must not name a mutator (nothing from nowhere), and the
#: column of each that holds a type id.
NPC_FILES = (("npc/trades.tsv", 1), ("npc/loot.tsv", 4),
             ("explore/loot.tsv", 2), ("explore/site-loot.tsv", 2))


def ranges_rows(table):
    """`mutators/ranges.tsv` rows, ascending by `(mutator, attribute)`.
    `table` is `{mutator: {"attributeIDs": {attr: {"min", "max",
    "highIsGood"?}}, "inputOutputMapping": [...]}}`. Keys are `int()`ed: the
    layout says `dict<str, ...>` and a build may decode either. A range
    without `highIsGood` writes an empty column."""
    rows = []
    for m, r in table.items():
        for a, rng in (r.get("attributeIDs") or {}).items():
            h = rng.get("highIsGood")
            rows.append((int(m), int(a), float(rng["min"]), float(rng["max"]),
                         "" if h is None else (1 if h else 0)))
    rows.sort(key=lambda x: (x[0], x[1]))
    return rows


def mapping_rows(table):
    """`mutators/mappings.tsv` rows, ascending by all three columns. An
    unpublished source is written: the server filters by `TypeRow::published`."""
    rows = set()
    for m, r in table.items():
        for mp in r.get("inputOutputMapping") or []:
            for s in mp.get("applicableTypes") or []:
                rows.add((int(m), int(mp["resultingType"]), int(s)))
    return sorted(rows)


def derive(table):
    return ranges_rows(table), mapping_rows(table)


# --- the bundle readers ---------------------------------------------------------

def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    if "mutators" in ctx._cache:  # noqa: SLF001
        return ctx._cache["mutators"]  # noqa: SLF001
    fsd = _content()._imp("fsd")  # noqa: SLF001
    got = {int(k): v for k, v in fsd.rows(ctx.client, "dynamicitemattributes").items()}
    ctx._cache["mutators"] = got  # noqa: SLF001
    return got


def section(ctx):
    r, m = derive(gather(ctx))
    ctx.note("mutators: %d mutators, %d ranges, %d mappings"
             % (len({x[0] for x in r}), len(r), len(m)))
    return [("mutators/ranges.tsv", RANGES_HEADER, r, [RES]),
            ("mutators/mappings.tsv", MAPPINGS_HEADER, m, [RES])]


# --- the checks -------------------------------------------------------------------

def problems(ranges, mappings, typedata, attributes, dynamic_types, npc_type_ids):
    """What is wrong with the two row lists, as strings. Pure.

    `typedata` is `{type_id: (group_id, category_id, published)}`,
    `attributes` the attribute ids typedata holds, `dynamic_types` the set of
    types whose `types` row has `isDynamicType`, `npc_type_ids` every type id
    named by the NPC trade and loot files (`NPC_FILES`)."""
    bad = []
    have_ranges, have_maps = {}, {}
    for m, a, lo, hi, _h in ranges:
        have_ranges[m] = have_ranges.get(m, 0) + 1
        if a not in attributes:
            bad.append("ranges.tsv: mutator %d: attribute %d is not in typedata" % (m, a))
        if not (math.isfinite(lo) and math.isfinite(hi)):
            bad.append("ranges.tsv: mutator %d: attribute %d: min/max not finite" % (m, a))
    results, sources, made = set(), set(), {}
    for m, res, src in mappings:
        have_maps[m] = have_maps.get(m, 0) + 1
        if made.setdefault((m, src), res) != res:
            bad.append("mutator %d: source %d maps to results %d and %d"
                       % (m, src, made[(m, src)], res))
        results.add(res)
        sources.add(src)
        if res not in typedata:
            bad.append("mappings.tsv: mutator %d: result type %d is not in typedata" % (m, res))
        if src not in typedata:
            bad.append("mappings.tsv: mutator %d: source type %d is not in typedata" % (m, src))
    for m in sorted(set(have_ranges) | set(have_maps)):
        if m not in typedata:
            bad.append("mutator %d is not in typedata" % m)
        elif typedata[m][0] != MUTATOR_GROUP:
            bad.append("mutator %d: group %d, want %d" % (m, typedata[m][0], MUTATOR_GROUP))
        if not have_ranges.get(m):
            bad.append("mutator %d has no range" % m)
        if not have_maps.get(m):
            bad.append("mutator %d has no mapping" % m)
    for t in sorted(results & sources):
        bad.append("type %d is both a result and a source" % t)
    for t in sorted(results - dynamic_types):
        bad.append("result type %d is not an isDynamicType type" % t)
    for t in sorted(dynamic_types - results):
        bad.append("isDynamicType type %d is no mutator's result" % t)
    mutators = set(have_ranges) | set(have_maps)
    for t in sorted(mutators & npc_type_ids):
        bad.append("mutator %d is named by an NPC trade or loot row" % t)
    return bad


def _typedata_facts(path):
    """`({type_id: (group, category, published)}, {attribute ids})` from
    `types/typedata.tsv`."""
    types, attributes = {}, set()
    with open(path, encoding="utf-8", newline="") as fh:
        for line in fh.read().split("\n"):
            f = line.split("\t")
            if f[0] == "attribute":
                attributes.add(int(f[1]))
            elif f[0] == "type" and len(f) >= 5:
                types[int(f[1])] = (int(f[2]), int(f[3]), f[4] == "1")
    return types, attributes


def _npc_type_ids(ctx, content):
    ids, skipped = set(), []
    for rel, col in NPC_FILES:
        p = ctx.path(rel)
        if not os.path.exists(p):
            skipped.append(rel)
            continue
        for f in content.read_rows(p):
            if len(f) > col and f[col].lstrip("-").isdigit():
                ids.add(int(f[col]))
    if skipped:
        ctx.note("mutators: skipped (absent): " + ", ".join(skipped))
    return ids


def check(ctx):
    content = _content()
    r, m = derive(gather(ctx))
    bad = []
    for name, rows in (("ranges", r), ("mappings", m)):
        got = list(content.read_rows(ctx.path("mutators/%s.tsv" % name)))
        if got != [content.encode_row(x).split("\t") for x in rows]:
            want = [content.encode_row(x).split("\t") for x in rows]
            first = next((i for i, (g, w) in enumerate(zip(got, want)) if g != w),
                         min(len(got), len(want)))
            bad.append("mutators/%s.tsv: differs from a fresh derive (%d vs %d rows, "
                       "first difference at data line %d)"
                       % (name, len(got), len(rows), first + 1))
    typedata, attributes = _typedata_facts(ctx.path("types/typedata.tsv"))
    fsd = content._imp("fsd")  # noqa: SLF001
    dynamic = {int(t) for t, row in fsd.rows(ctx.client, "types").items()
               if row.get("isDynamicType")}
    bad += problems(r, m, typedata, attributes, dynamic, _npc_type_ids(ctx, content))
    ctx.note("mutators: %d mutators, %d ranges, %d mappings, %d result types, %d sources"
             % (len({x[0] for x in r}), len(r), len(m), len({x[1] for x in m}),
                len({x[2] for x in m})))
    return bad
