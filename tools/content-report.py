#!/usr/bin/env python3
"""The per-build content report: what a new client build changed in the data, and what needs work.

  python3 -I tools/content-report.py OLD_BUNDLE NEW_BUNDLE [--notes FILE] [--out DIR] [--uses]
        [--root DIR]

OLD_BUNDLE and NEW_BUNDLE are content bundle directories (each holds a MANIFEST.tsv) from the
same extractor commit; a different `extractor` hash is exit 2, because the difference would
be the extractor's. It reuses tools/bundle-diff.py (its loader, KEYS and the --uses grep) and
adds the sorting: per area, one verdict.

  --notes FILE  a plain-text outline of the public patch notes (docs/guides/tracking-tq.md 5.2),
                kept under out/, never committed. Each capitalised word run of three letters or
                more is matched, case-insensitively, against the names of added and renamed
                types, corporations and systems. A match sets `in_notes=yes` on that row; an
                outline line that matches nothing becomes a `notes-only` row.
  --out DIR     default out/new-build/<N>/content/ under --root (N = the new bundle's build).
                Writes content-report.md and intake.tsv (the table tools/intake-issues.py reads:
                area, kind, what, count, sample_ids, server_readers, in_notes, verdict,
                suggested_title). Both hold CCP names and ids: under out/, never committed.
  --allow-extractor-mismatch  go on with a NOTE instead of exit 2 (a first look only)
  --uses        grep the changed ids against server/crates, content/ (fills server_readers and
                lets a REMOVED id or a published flip with a reader count as needs-work).

Verdicts: needs-work (code or authored data probably has to change), review (a human looks),
notes-only (only the notes mention it), data-driven (flows in with the data, nothing to do).
Exit: 0 no differences; 1 differences or a notes-only row, none needing work; 3 at least one needs-work or review
row; 2 usage or an extractor mismatch.

Dogma semantics, loot tables and dungeon behaviour are not judged here: a row says that
something changed and which rule it tripped, and a lane reads the rest.

Std-only. [A] area slugs (AREAS below) are the closest contributing.md section 10 areas;
falsifier: intake-issues.py or the maintainer labelling a draft with another area.
[A] "a modifier func the server knows" is a func seen in the OLD bundle's modifier rows or in
server/crates source; falsifier: a func the server handles by name that is in neither.
"""

from __future__ import annotations

import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import collections
import importlib.util
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("bundle_diff", os.path.join(HERE, "bundle-diff.py"))
bd = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(bd)

COLS = ("area", "kind", "what", "count", "sample_ids", "server_readers", "in_notes", "verdict", "suggested_title")
MAX_IDS = 5
AREAS = {
    "types": "market", "dogma": "fitting", "dungeons": "pve", "npcs": "pve", "industry": "industry",
    "map": "space", "agents": "pve", "lp": "market", "skins": "market", "tables": "tools", "notes": "tools",
}
META_ATTR = "1692"  # [V] the metaGroupID attribute id in typedata attrs (stable dogma attribute id)
NOTE_STOP = {"the", "and", "for", "with", "new", "all", "this", "that", "from", "has", "have", "are",
             "was", "can", "now", "more", "less", "will", "when", "your", "you", "their"}
RUN = re.compile(r"[A-Z][A-Za-z'\-]{2,}(?:\s+[A-Z][A-Za-z'\-]{2,})*")


def nk(s):
    """Sort key: numbers by value first, anything else after, by text."""
    return (0, int(s), "") if re.fullmatch(r"-?\d+", s) else (1, 0, s)


class Row:
    def __init__(self, section, kind, what, ids=(), verdict="data-driven", count=None, names=(),
                 title=None, note=""):
        self.section, self.kind, self.what = section, kind, what
        self.ids = [str(i) for i in ids]
        self.count = len(self.ids) if count is None else count
        self.verdict = verdict
        self.names = [n for n in names if n]
        self.title = title or what
        self.note = note
        self.in_notes = "-"
        self.readers = "-"


def tsv(bundle, rel):
    """Rows of one bundle file; no file, no rows."""
    p = os.path.join(bundle, rel)
    if not os.path.isfile(p):
        return []
    return bd.read_rows(p)[1]


class Ctx:
    def __init__(self, old, new, root, uses):
        self.old, self.new, self.root, self.uses = old, new, root, uses
        so, fo = bd.read_manifest(old)
        sn, fn = bd.read_manifest(new)
        self.so, self.sn, self.fo, self.fn = so, sn, fo, fn
        self.differ = sorted(f for f in set(fo) & set(fn) if fo[f] != fn[f])
        self.only_new = sorted(set(fn) - set(fo))
        self.only_old = sorted(set(fo) - set(fn))
        self._diffs = {}
        self.rows = []
        self._src = None

    def diff(self, rel):
        if rel not in self.differ:
            return None
        if rel not in self._diffs:
            self._diffs[rel] = bd.diff_file(rel, os.path.join(self.old, rel), os.path.join(self.new, rel))
        return self._diffs[rel]

    def source_text(self):
        """All text under server/crates and content/ (for the modifier func check), once."""
        if self._src is None:
            parts = []
            for top in ("server/crates", "content"):
                for dp, dn, fn in os.walk(os.path.join(self.root, top)):
                    dn[:] = [x for x in dn if x != "target"]
                    for f in fn:
                        p = os.path.join(dp, f)
                        try:
                            if os.path.getsize(p) < (4 << 20):
                                with open(p, "r", encoding="utf-8") as fh:
                                    parts.append(fh.read())
                        except (OSError, UnicodeDecodeError):
                            pass
            self._src = "\n".join(parts)
        return self._src

    def add(self, *a, **k):
        r = Row(*a, **k)
        self.rows.append(r)
        return r


def read_ids(ctx, ids):
    """`{id: [paths]}` for ids with a reader in the server or content; empty without --uses."""
    if not ctx.uses:
        return {}
    found = bd.find_uses({i for i in ids if i.isdigit()}, ctx.root)
    return {i: sorted(found[i]) for i in found}


def type_rows(bundle):
    d = {}
    for r in tsv(bundle, bd.TYPEDATA):
        if r and r[0] == "type" and len(r) > 4:
            d[r[1]] = r
    return d


def type_meta(r):
    """The metaGroupID attribute of a type row's attrs column, or ''."""
    if len(r) > 12:
        for kv in r[12].split(";"):
            k, _, v = kv.partition("=")
            if k == META_ATTR:
                return v.split(".")[0]
    return ""


def type_name(r):
    return r[13] if len(r) > 13 else ""


def sec_types(ctx):
    o, n = type_rows(ctx.old), type_rows(ctx.new)
    if not (set(o) != set(n) or any(o[i] != n[i] for i in set(o) & set(n))):
        return
    added = sorted(set(n) - set(o), key=nk)
    removed = sorted(set(o) - set(n), key=nk)
    common = set(o) & set(n)
    renamed = sorted((i for i in common if type_name(o[i]) != type_name(n[i])), key=nk)
    flips = sorted((i for i in common if len(o[i]) > 4 and len(n[i]) > 4 and o[i][4] != n[i][4]), key=nk)
    regroup = sorted((i for i in common if len(o[i]) > 3 and len(n[i]) > 3 and o[i][2:4] != n[i][2:4]), key=nk)
    # [I] the typedata has no group or metagroup name rows, so "a new group" is a group id with
    # no type in OLD, and a new metagroup is a metaGroupID attr value no type in OLD carries.
    old_groups = {r[2] for r in o.values() if len(r) > 2}
    old_meta = {type_meta(r) for r in o.values()} - {""}
    new_groups = sorted({r[2] for r in n.values() if len(r) > 2} - old_groups, key=nk)
    new_meta = sorted({type_meta(r) for r in n.values()} - old_meta - {""}, key=nk)
    if added:
        ctx.add("types", "added", "types added", added, names=[type_name(n[i]) for i in added],
                title="the %d new types are seeded, sold and made through the data" % len(added))
    if new_groups:
        ctx.add("types", "new-group", "group ids with no type in OLD (code may switch on groups)", new_groups,
                "needs-work", title="a new item group is accepted wherever groups are listed")
    if new_meta:
        ctx.add("types", "new-metagroup", "metagroup values with no type in OLD", new_meta, "needs-work",
                title="a new metagroup is handled by the meta multipliers and the market")
    if removed:
        r = ctx.add("types", "removed", "type ids gone from NEW", removed, "review",
                    title="types removed by the client no longer appear in the world")
        r.check = "removed"
    if renamed:
        ctx.add("types", "renamed", "types renamed", renamed, names=[type_name(n[i]) for i in renamed],
                title="renamed types show their new names")
    if flips:
        r = ctx.add("types", "published-flip", "types whose published bit flipped", flips, "review",
                    title="types whose published flag changed are still seeded or sold correctly")
        r.check = "flip"
    if regroup:
        ctx.add("types", "regrouped", "types whose group or category changed", regroup, "review",
                title="types that changed group are treated as their new group")


def sec_dogma(ctx):
    d = ctx.diff(bd.TYPEDATA)
    if d is None:
        return
    kinds = collections.defaultdict(lambda: [[], [], []])
    for k in d.added:
        if k[0] in bd.DOGMA_KINDS:
            kinds[k[0]][0].append(k[1])
    for k in d.removed:
        if k[0] in bd.DOGMA_KINDS:
            kinds[k[0]][1].append(k[1])
    for k in d.changed:
        if k[0] in bd.DOGMA_KINDS:
            kinds[k[0]][2].append(k[1])
    for kind in sorted(kinds):
        a, r, c = kinds[kind]
        ctx.add("dogma", kind + "-counts", "%s rows: %d added, %d removed, %d changed" % (kind, len(a), len(r), len(c)),
                a + r + c, "review" if (r or c) else "data-driven", count=len(a) + len(r) + len(c),
                title="the %s rows that changed are read by the dogma code" % kind)
    # New modifier funcs: modifier rows are `modifier id domain func attr ...`.
    old_funcs = {r[3] for r in tsv(ctx.old, bd.TYPEDATA) if r and r[0] == "modifier" and len(r) > 3}
    new_rows = [r for r in tsv(ctx.new, bd.TYPEDATA) if r and r[0] == "modifier" and len(r) > 3]
    unknown = {}
    for r in new_rows:
        f = r[3]
        if f and f not in old_funcs and f not in unknown:
            if not re.search(r"\b%s\b" % re.escape(f), ctx.source_text()):
                unknown[f] = r[1]
    if unknown:
        ctx.add("dogma", "modifier-func", "modifier funcs unknown to OLD and the server: " + ", ".join(sorted(unknown)),
                sorted(set(unknown.values()), key=nk), "needs-work",
                title="the new modifier function %s is applied by the dogma code" % sorted(unknown)[0])
    # An effect with a Python guid and no modifiers, among the added or changed effect rows.
    mods = {r[1] for r in new_rows}
    touched = [k[1] for k in list(d.added) + list(d.changed) if k[0] == "effect"]
    erows = {r[1]: r for r in tsv(ctx.new, bd.TYPEDATA) if r and r[0] == "effect"}
    pyonly = sorted((i for i in touched if i in erows and len(erows[i]) > 10 and erows[i][10].startswith("effects.")
                     and i not in mods), key=nk)
    if pyonly:
        ctx.add("dogma", "python-effect", "added or changed effects with a Python guid and no modifiers", pyonly,
                "needs-work", title="effects that run Python in the client have a server formula")


def sec_dungeons(ctx):
    if "dungeons/archetypes.tsv" in ctx.differ or "dungeons/archetypes.tsv" in ctx.only_new:
        o = {r[0] for r in tsv(ctx.old, "dungeons/archetypes.tsv")}
        n = {r[0] for r in tsv(ctx.new, "dungeons/archetypes.tsv")}
        new = sorted(n - o, key=nk)
        if new:
            ctx.add("dungeons", "new-archetype", "dungeon archetypes added", new, "needs-work",
                    title="a site of each new archetype is seeded and completes")
    for rel, kind in (("dungeons/dungeons.tsv", "dungeons"), ("dungeons/objects.tsv", "objects"),
                      ("dungeons/spawns.tsv", "spawns"), ("dungeons/rooms.tsv", "rooms")):
        d = ctx.diff(rel)
        if d is None:
            continue
        ids = [k[0] for k in d.added + d.removed + list(d.changed)]
        ctx.add("dungeons", kind, "%s: %d added, %d removed, %d changed" % (kind, len(d.added), len(d.removed), len(d.changed)),
                sorted(set(ids), key=lambda s: (len(s), s)), "data-driven", count=len(d.added) + len(d.removed) + len(d.changed),
                title="the changed dungeon %s load" % kind)
    # A dungeon that had spawn rows in OLD and has none in NEW.
    if "dungeons/spawns.tsv" in ctx.differ:
        o = {r[0] for r in tsv(ctx.old, "dungeons/spawns.tsv")}
        n = {r[0] for r in tsv(ctx.new, "dungeons/spawns.tsv")}
        gone = sorted(o - n, key=nk)
        if gone:
            ctx.add("dungeons", "lost-spawns", "dungeons that lost every spawn row", gone, "needs-work",
                    title="a dungeon with no spawn rows does not complete as cleared")


def sec_npcs(ctx):
    d = ctx.diff("npc/components.tsv")
    if d is not None:
        co = bd._component_counts(os.path.join(ctx.old, "npc/components.tsv"))
        cn = bd._component_counts(os.path.join(ctx.new, "npc/components.tsv"))
        new = sorted(set(cn) - set(co))
        zero = sorted(c for c in co if co[c] and not cn.get(c))
        if new:
            ctx.add("npcs", "new-component", "NPC component kinds added: " + ", ".join(new), [], "needs-work",
                    count=len(new), title="the new NPC component %s is read by the NPC code" % new[0])
        if zero:
            ctx.add("npcs", "component-zero", "NPC component kinds whose row count fell to zero: " + ", ".join(zero), [],
                    "needs-work", count=len(zero), title="an NPC component with no rows is not read as present")
        moved = [c for c in set(co) & set(cn) if co[c] != cn[c] and c not in zero]
        if moved:
            ctx.add("npcs", "component-counts", "component row counts moved: " +
                    ", ".join("%s %d->%d" % (c, co[c], cn[c]) for c in sorted(moved)[:8]), [], "data-driven",
                    count=len(moved), title="the changed NPC component counts load")
    for rel, kind in (("npc/types.tsv", "types"), ("npc/bounty.tsv", "bounty"), ("npc/regions.tsv", "regions"),
                      ("npc/loot.tsv", "loot"), ("npc/reactive-spawn.tsv", "reactive-spawn")):
        d = ctx.diff(rel)
        if d is None:
            continue
        ids = [k[0] for k in d.added + d.removed + list(d.changed)]
        ctx.add("npcs", kind, "npc %s: %d added, %d removed, %d changed" % (kind, len(d.added), len(d.removed), len(d.changed)),
                sorted(set(ids), key=lambda s: (len(s), s)), "review" if d.removed else "data-driven",
                count=len(d.added) + len(d.removed) + len(d.changed), title="the changed NPC %s are spawned and paid correctly" % kind)


def sec_industry(ctx):
    d = ctx.diff("industry/blueprints.tsv")
    for rel in ("types/materials.tsv", "types/compress.tsv"):
        e = ctx.diff(rel)
        if e is not None:
            ids = [k[0] for k in e.added + e.removed + list(e.changed)]
            ctx.add("industry", os.path.basename(rel)[:-4], "%s: %d added, %d removed, %d changed" %
                    (rel, len(e.added), len(e.removed), len(e.changed)), sorted(set(ids), key=nk), "data-driven",
                    count=len(e.added) + len(e.removed) + len(e.changed), title="the changed %s rows are used" % rel)
    if d is None:
        return
    ctx.add("industry", "blueprints", "blueprints: %d added, %d removed, %d changed" % (len(d.added), len(d.removed), len(d.changed)),
            sorted({k[0] for k in d.added + d.removed + list(d.changed)}, key=nk), "review" if d.removed else "data-driven",
            count=len(d.added) + len(d.removed) + len(d.changed), title="the changed blueprints produce what they list")
    # A material of a new or changed blueprint that has no price: nothing can value it.
    priced = {r[0] for r in tsv(ctx.new, "econ/prices.tsv")}
    touched = {k[0] for k in d.added + list(d.changed)}
    unpriced = sorted({r[3] for r in tsv(ctx.new, "industry/blueprints.tsv")
                       if len(r) > 3 and r[0] in touched and r[2] == "material" and r[3] not in priced}, key=nk)
    if unpriced:
        ctx.add("industry", "unpriced-material", "materials of added or changed blueprints with no price row", unpriced,
                "needs-work", title="every material of a blueprint has a price")


def sec_map(ctx):
    d = ctx.diff("universe/systems.tsv")
    if d is not None:
        o = {r[0]: r for r in tsv(ctx.old, "universe/systems.tsv")}
        n = {r[0]: r for r in tsv(ctx.new, "universe/systems.tsv")}
        renamed = sorted((i for i in set(o) & set(n) if o[i][-1] != n[i][-1]), key=nk)
        reown = sorted((i for i in set(o) & set(n) if o[i][3:5] != n[i][3:5]), key=nk)
        text = ctx.source_text() if renamed else ""
        named = [i for i in renamed if o[i][-1] and re.search(r"\b%s\b" % re.escape(o[i][-1]), text)]
        if named:
            ctx.add("map", "renamed-used", "systems renamed whose old name an authored table or source names", named,
                    "needs-work", names=[n[i][-1] for i in named], title="authored tables name the renamed systems by their new names")
        other = [i for i in renamed if i not in named]
        if other:
            ctx.add("map", "systems-renamed", "systems renamed", other, "data-driven", names=[n[i][-1] for i in other],
                    title="renamed systems show their new names")
        if reown:
            ctx.add("map", "re-owned", "systems whose faction or owner changed", reown, "review",
                    title="re-owned systems have the right NPC and sov presence")
        add = sorted(set(n) - set(o), key=nk)
        if add:
            ctx.add("map", "systems-added", "systems added", add, "review", names=[n[i][-1] for i in add],
                    title="the new systems exist in the graph and the world")
    for rel in ("universe/regions.tsv", "universe/constellations.tsv", "universe/celestials.tsv", "universe/system-effects.tsv"):
        e = ctx.diff(rel)
        if e is not None:
            ctx.add("map", os.path.basename(rel)[:-4], "%s: %d added, %d removed, %d changed" % (rel, len(e.added), len(e.removed), len(e.changed)),
                    sorted({k[0] for k in e.added + e.removed + list(e.changed)}, key=nk)[:200], "data-driven",
                    count=len(e.added) + len(e.removed) + len(e.changed), title="the changed %s rows load" % rel)
    d = ctx.diff("stations/stations.tsv")
    if d is not None and d.added:
        # A kept world gains a station only on reseed. [I]
        ctx.add("map", "stations-added", "stations added (a kept world gains them only on reseed)", [k[0] for k in d.added],
                "review", title="the new stations exist in a kept world")


def sec_agents(ctx):
    o = {r[0]: r for r in tsv(ctx.old, "npc/corporations.tsv")}
    n = {r[0]: r for r in tsv(ctx.new, "npc/corporations.tsv")}
    fac = sorted((i for i in set(o) & set(n) if len(o[i]) > 1 and o[i][1] != n[i][1]), key=nk)
    if fac:
        ctx.add("agents", "corp-faction", "corporations that changed faction", fac, "needs-work",
                names=[n[i][-1] for i in fac], title="standings follow a corporation that changed faction")
    cadd = sorted(set(n) - set(o), key=nk)
    crem = sorted(set(o) - set(n), key=nk)
    if cadd or crem:
        ctx.add("agents", "corps", "corporations: %d added, %d removed" % (len(cadd), len(crem)), cadd + crem,
                "review" if crem else "data-driven", names=[n[i][-1] for i in cadd], title="the changed corporations are seeded")
    d = ctx.diff("agents/agents.tsv")
    if d is not None and (d.added or d.removed):
        ctx.add("agents", "agents", "agents: %d added, %d removed" % (len(d.added), len(d.removed)),
                [k[0] for k in d.added + d.removed], "needs-work", title="agents that came or went are in the stations and the tables")
    d = ctx.diff("agents/missions.tsv")
    if d is not None:
        hdr = d.header or []
        objcols = {i for i, h in enumerate(hdr) if h.startswith("obj_") or h in ("template", "family", "dungeon_id")}
        objchg = []
        for k, (a, b) in d.rows.items():
            if len(a) == 1 and len(b) == 1 and any(a[0][i] != b[0][i] for i in objcols if i < len(a[0]) and i < len(b[0])):
                objchg.append(k[0])
        if objchg:
            ctx.add("agents", "mission-objective", "missions whose objective changed", sorted(objchg, key=nk), "needs-work",
                    title="the changed missions complete on their new objective")
        rest = len(d.added) + len(d.removed) + len(d.changed) - len(objchg)
        if rest > 0:
            ctx.add("agents", "missions", "missions: %d added, %d removed, %d changed in other columns" %
                    (len(d.added), len(d.removed), len(d.changed)), sorted({k[0] for k in d.added + d.removed + list(d.changed)}, key=nk),
                    "review" if d.removed else "data-driven", count=rest, title="the changed missions are offered and paid")
    for rel in ("agents/epic-arcs.tsv", "agents/schools.tsv", "agents/mission-messages.tsv"):
        e = ctx.diff(rel)
        if e is not None:
            ctx.add("agents", os.path.basename(rel)[:-4], "%s: %d added, %d removed, %d changed" % (rel, len(e.added), len(e.removed), len(e.changed)),
                    sorted({k[0] for k in e.added + e.removed + list(e.changed)}, key=lambda s: (len(s), s)), "data-driven",
                    count=len(e.added) + len(e.removed) + len(e.changed), title="the changed %s rows load" % rel)


def sec_lp(ctx):
    for rel in ("agents/lp-offers.tsv", "econ/prices.tsv", "store/catalogue.tsv", "store/types.tsv"):
        e = ctx.diff(rel)
        if e is not None:
            ctx.add("lp", os.path.basename(rel)[:-4], "%s: %d added, %d removed, %d changed" % (rel, len(e.added), len(e.removed), len(e.changed)),
                    sorted({k[0] for k in e.added + e.removed + list(e.changed)}, key=lambda s: (len(s), s)),
                    "review" if e.removed else "data-driven", count=len(e.added) + len(e.removed) + len(e.changed),
                    title="the changed %s rows are offered and priced" % rel)
    # A reprocess output with no price (materials.tsv: type_id, material_type_id, quantity).
    mats = tsv(ctx.new, "types/materials.tsv")
    if mats and os.path.isfile(os.path.join(ctx.new, "econ/prices.tsv")):
        priced = {r[0] for r in tsv(ctx.new, "econ/prices.tsv")}
        oldmats = {r[1] for r in tsv(ctx.old, "types/materials.tsv") if len(r) > 1}
        out = sorted({r[1] for r in mats if len(r) > 1 and r[1] not in priced and r[1] not in oldmats}, key=nk)
        if out:
            ctx.add("lp", "unpriced-output", "new reprocess outputs with no price row", out, "needs-work",
                    title="every reprocess output has a price")


def sec_skins(ctx):
    for rel in sorted(set(ctx.differ) | set(ctx.only_new)):
        if not (rel.startswith("skins/") or rel.startswith("cosmetics/")):
            continue
        e = ctx.diff(rel)
        if e is None:
            continue
        ctx.add("skins", os.path.basename(rel)[:-4], "%s: %d added, %d removed, %d changed" % (rel, len(e.added), len(e.removed), len(e.changed)),
                sorted({k[0] for k in e.added + e.removed + list(e.changed)}, key=lambda s: (len(s), s)), "data-driven",
                count=len(e.added) + len(e.removed) + len(e.changed), title="the changed %s rows load" % rel)


SEEN = ("types/typedata.tsv", "dungeons/", "npc/components.tsv", "npc/types.tsv", "npc/bounty.tsv", "npc/regions.tsv",
        "npc/loot.tsv", "npc/reactive-spawn.tsv", "industry/", "types/materials.tsv", "types/compress.tsv", "universe/",
        "stations/stations.tsv", "npc/corporations.tsv", "agents/", "econ/prices.tsv", "store/", "skins/", "cosmetics/")


def sec_tables(ctx):
    if ctx.only_new:
        ctx.add("tables", "new-files", "files only in NEW: " + ", ".join(ctx.only_new), [], "needs-work", count=len(ctx.only_new),
                title="the new data tables are read by the server")
    if ctx.only_old:
        ctx.add("tables", "gone-files", "files gone from NEW: " + ", ".join(ctx.only_old), [], "needs-work", count=len(ctx.only_old),
                title="nothing reads the data tables the client dropped")
    other = [f for f in ctx.differ if not any(f == s or (s.endswith("/") and f.startswith(s)) for s in SEEN)]
    if other:
        ctx.add("tables", "other-changed", "other changed tables: " + ", ".join(other), [], "review", count=len(other),
                title="the changed data tables are read correctly")
    # `source` rows of the manifest: a new res:/staticdata table the extractor now reads.
    srcs = lambda m: {l.split("\t")[1] for l in open(os.path.join(m, bd.MANIFEST), encoding="utf-8") if l.startswith("source\t") and l.count("\t") >= 1}
    new_src = sorted(srcs(ctx.new) - srcs(ctx.old))
    if new_src:
        ctx.add("tables", "new-sources", "new client tables the bundle reads: " + ", ".join(new_src), [], "needs-work",
                count=len(new_src), title="the new client tables are used")


def candidates(line):
    """Capitalised runs of three letters or more, leading stop words dropped."""
    out = []
    for m in RUN.finditer(line):
        words = m.group(0).split()
        while words and words[0].lower() in NOTE_STOP:
            words.pop(0)
        if words:
            out.append(" ".join(words))
    return out


def join_notes(ctx, notes_path):
    with open(notes_path, "r", encoding="utf-8", errors="replace") as fh:
        lines = [l.strip() for l in fh if l.strip() and not l.lstrip().startswith("#")]  # "#" lines are headings
    names = [(r, re.compile(r"(?<!\w)%s(?!\w)" % re.escape(n), re.I)) for r in ctx.rows for n in r.names if len(n) >= 3]
    for r in ctx.rows:
        r.in_notes = "no"
    unmatched = []
    for line in lines:
        hit = False
        for c in candidates(line):
            for r, pat in names:
                if pat.search(c):  # the whole name, on word boundaries, inside a capitalised run
                    r.in_notes = "yes"
                    hit = True
        if not hit:
            unmatched.append(line)
    for line in unmatched:
        # Only a short piece of the outline is copied: its longest capitalised run, else about 40 characters.
        runs = candidates(line)
        short = max(runs, key=len)[:40] if runs else line[:40]
        ctx.add("notes", "notes-only", "patch notes item with no changed row: " + short, [], "notes-only", count=1,
                title="the server applies the patch notes change: " + short).in_notes = "yes"


def apply_uses(ctx):
    """Fill server_readers, and settle the rows whose rule needs a reader."""
    allids = {i for r in ctx.rows for i in r.ids}
    found = read_ids(ctx, allids)
    for r in ctx.rows:
        paths = collections.Counter()
        hit = []
        for i in r.ids:
            for p in found.get(i, ()):
                paths[p] += 1
                if i not in hit:
                    hit.append(i)
        if paths:
            r.readers = ",".join(p for p, _ in paths.most_common(3)) + (" (%d ids read)" % len(hit) if len(hit) > 1 else "")
        check = getattr(r, "check", None)
        if check and hit:
            r.verdict = "needs-work"
        elif check and ctx.uses:
            r.verdict = "data-driven" if check == "flip" else "review"


def run(old, new, root, uses=False, notes=None):
    """`(ctx, exit code)`; the caller writes the files."""
    ctx = Ctx(old, new, root, uses)
    for fn in (sec_types, sec_dogma, sec_dungeons, sec_npcs, sec_industry, sec_map, sec_agents, sec_lp, sec_skins, sec_tables):
        fn(ctx)
    if uses:
        apply_uses(ctx)
    if notes:
        join_notes(ctx, notes)
    elif not uses:
        pass
    for r in ctx.rows:
        if r.in_notes == "-" and notes:
            r.in_notes = "no"
    differences = bool(ctx.differ or ctx.only_new or ctx.only_old)
    work = any(r.verdict in ("needs-work", "review") for r in ctx.rows)
    notes_only = any(r.verdict == "notes-only" for r in ctx.rows)
    return ctx, (3 if work else (1 if (differences or notes_only) else 0))


def intake_rows(ctx):
    for r in ctx.rows:
        yield (AREAS[r.section], r.kind, r.what, str(r.count), ",".join(r.ids[:MAX_IDS]) or "-", r.readers,
               r.in_notes, r.verdict, r.title)


def clean(s):
    return s.replace("\t", " ").replace("\n", " ")


def markdown(ctx, code):
    L = []
    w = L.append
    w("# Content report: build %s -> %s" % (ctx.so.get("client_build", "?"), ctx.sn.get("client_build", "?")))
    w("")
    w("CCP's names and ids: keep under out/, never commit.")
    w("")
    if getattr(ctx, "mismatch", None):
        w("**WARNING: the extractors differ (%s): some rows may be the extractor's, not the client's.**" % ctx.mismatch)
        w("")
    w("Files: %d differ, %d only in NEW, %d only in OLD. Exit %d." % (len(ctx.differ), len(ctx.only_new), len(ctx.only_old), code))
    w("")
    counts = collections.Counter(r.verdict for r in ctx.rows)
    w("Verdicts: " + ", ".join("%s %d" % (k, counts[k]) for k in ("needs-work", "review", "notes-only", "data-driven")))
    w("")
    order = ["types", "dogma", "dungeons", "npcs", "industry", "map", "agents", "lp", "skins", "tables", "notes"]
    for sec in order:
        rows = [r for r in ctx.rows if r.section == sec]
        verdict = "none"
        for v in ("needs-work", "review", "notes-only", "data-driven"):
            if any(r.verdict == v for r in rows):
                verdict = v
                break
        w("## %s (area %s): %s" % (sec, AREAS[sec], verdict if rows else "no change"))
        w("")
        for r in rows:
            w("- **%s** %s: %s (count %d, in_notes %s, readers %s)" % (r.verdict, r.kind, clean(r.what), r.count, r.in_notes, r.readers))
            if r.ids:
                w("  ids: " + ", ".join(r.ids[:MAX_IDS]) + (" ..." if len(r.ids) > MAX_IDS else ""))
        w("")
    return "\n".join(L)


def main(argv=None):
    ap = argparse.ArgumentParser(description="Per-build content report: two bundles, one verdict per area.")
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--notes", metavar="FILE")
    ap.add_argument("--out", metavar="DIR")
    ap.add_argument("--uses", action="store_true")
    ap.add_argument("--allow-extractor-mismatch", action="store_true",
                    help="go on when the extractors differ (the report says so; differences may be the extractor's)")
    ap.add_argument("--root", metavar="DIR", default=os.path.dirname(HERE))
    a = ap.parse_args(argv)
    for d in (a.old, a.new):
        if not os.path.isfile(os.path.join(d, bd.MANIFEST)):
            print("content-report: %s: no %s" % (d, bd.MANIFEST), file=sys.stderr)
            return 2
    if a.notes and not os.path.isfile(a.notes):
        print("content-report: %s: no such notes file" % a.notes, file=sys.stderr)
        return 2
    so, _ = bd.read_manifest(a.old)
    sn, _ = bd.read_manifest(a.new)
    # A missing extractor on either side cannot be shown equal: that is a mismatch too.
    mismatch = not so.get("extractor") or not sn.get("extractor") or so["extractor"] != sn["extractor"]
    if mismatch and a.allow_extractor_mismatch:
        print("content-report: NOTE the extractors differ; some rows may be the extractor's, not the client's.", file=sys.stderr)
    elif mismatch:
        print("content-report: the extractors differ (OLD %s, NEW %s): re-extract one bundle with the other's "
              "extractor commit, or the difference is the extractor's, not the client's."
              % (so.get("extractor", "?")[:12], sn.get("extractor", "?")[:12]), file=sys.stderr)
        return 2
    ctx, code = run(a.old, a.new, a.root, a.uses, a.notes)
    ctx.mismatch = ("OLD %s, NEW %s" % (so.get("extractor", "none")[:12] or "none", sn.get("extractor", "none")[:12] or "none")) if mismatch else None
    out = a.out or os.path.join(a.root, "out", "new-build", sn.get("client_build", "unknown"), "content")
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(out, "content-report.md"), "w", encoding="utf-8") as fh:
        fh.write(markdown(ctx, code))
    with open(os.path.join(out, "intake.tsv"), "w", encoding="utf-8") as fh:
        fh.write("\t".join(COLS) + "\n")
        for row in intake_rows(ctx):
            fh.write("\t".join(clean(c) for c in row) + "\n")
    counts = collections.Counter(r.verdict for r in ctx.rows)
    print("content-report: %d rows (%s); wrote %s" % (len(ctx.rows), ", ".join("%s %d" % kv for kv in sorted(counts.items())), out))
    if ctx.mismatch:
        print("content-report: WARNING the extractors differ (%s): some rows may be the extractor's, not the client's." % ctx.mismatch)
    return code


if __name__ == "__main__":
    sys.exit(main())
