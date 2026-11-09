"""The `pochven` content section: the Triglavian NPC types (PV-4, issue #228).

One derived file, read by `pochven::npc` in the server:

    pochven/triglavians.tsv   the Triglavian NPC types, derived from the
                              bundle's `types/typedata.tsv` and
                              `types/market.tsv` (the shape of
                              `content_wormholes.py`'s sleepers file).

A type is kept when it is a category-11 (entity) type of the Triglavian faction
(its market `faction_id`, `types.factionID` [V-data]), has at least one combat
effect, is not a test or `TTC-` type, and its name's last word is the name of a
published player ship ("Starving Damavik" -> Damavik, a destroyer). The class
is that ship's group; the rank is the rest of the name.

The derivation is pure (plain rows in, lines out), so the unit tests feed it
invented rows; `section` and `check` are the only bundle readers.
"""
import os
import sys

SECTION = "pochven"
DEPENDS = ("types", "types2")

_HERE = os.path.dirname(os.path.abspath(__file__))

#: `appConst.factionTriglavian` [V].
TRIGLAVIAN_FACTION = 500026
CATEGORY_ENTITY = 11  # `categoryEntity` [V]
CATEGORY_SHIP = 6     # `categoryShip` [V]
#: Player ship group id -> class word (protocol constants, the client's groups).
#: The spelling is the server's: npc/types.tsv, npc_gen and combat::npc_class use
#: "BattleCruiser" (capital C), so a row must too.
CLASSES = {25: "Frigate", 420: "Destroyer", 26: "Cruiser", 419: "BattleCruiser",
           27: "Battleship", 485: "Dreadnought"}
HEADER = "type_id\tgroup_id\tclass\trank\tneeds\trole\twreck_type_id\twhy"
COMMENTS = (
    "# Triglavian NPC types (PV-4). Written by tools/content_pochven.py from the",
    "# player's own typedata and market; read by `pochven::npc::rows` and merged",
    "# into npc_data (faction 500026, context Deadspace, tier deadspace).",
    "# - class: the group of the player ship the name ends with (Frigate,",
    "#   Destroyer, Cruiser, BattleCruiser, Battleship, Dreadnought).",
    "# - rank: the name minus its last word (Liminal, Raznaborg, ...).",
    "# - needs, role: content_npc.needs_of / role_of over the type's own effects.",
    "# - wreck_type_id: types/market.tsv; empty when the type has none.",
)
#: [I] 159 of the 176 rows get role LOGISTIC: effect 6741 is on 158 of them and
#: `content_npc.role_of` puts REMOTE_REP first, so it looks like an effect every
#: Triglavian carries, not a repair role. Nothing reads `Role::Logistic` yet, so
#: the role is left as derived. Falsified by a Triglavian that really repairs
#: allies being the only one with 6741 (check when PV-6 reads the role).
FILE = "pochven/triglavians.tsv"
SOURCES_NOTE = "types"


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _npc():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content_npc
    return content_npc


def market_facts(market_rows):
    """`({type_id: faction_id}, {type_id: wreck_type_id})` from
    `types/market.tsv` data rows (split)."""
    faction, wreck = {}, {}
    for p in market_rows:
        if len(p) < 10 or not p[0].isdigit():
            continue
        if p[9].isdigit():
            faction[int(p[0])] = int(p[9])
        if len(p) > 11 and p[11].isdigit():
            wreck[int(p[0])] = int(p[11])
    return faction, wreck


def ship_classes(type_rows):
    """`{name: group_id}` of the published player ships of `CLASSES` (type rows
    split on TAB: `type`, id, group, category, published, ..., name at 13). A
    name two groups share is ambiguous and left out."""
    out, bad = {}, set()
    for p in type_rows:
        if len(p) < 14 or p[0] != "type" or p[3] != str(CATEGORY_SHIP) or p[4] != "1":
            continue
        group = int(p[2])
        name = p[13].strip()
        if group not in CLASSES or not name:
            continue
        if out.setdefault(name, group) != group:
            bad.add(name)
    return {k: v for k, v in out.items() if k not in bad}


def derive(type_rows, market_rows, needs_of, role_of, faction=TRIGLAVIAN_FACTION):
    """The triglavians file as lines.

    `type_rows` are `types/typedata.tsv` data rows (split on TAB: `type`, id,
    group, category at 3, published at 4, effects at 11, `attr=value;...` at
    12, name at 13), `market_rows` the split `types/market.tsv` rows."""
    fac, wreck = market_facts(market_rows)
    ships = ship_classes(type_rows)
    rows = []
    for p in type_rows:
        if len(p) < 14 or p[0] != "type" or p[3] != str(CATEGORY_ENTITY):
            continue
        tid, group = int(p[1]), int(p[2])
        if fac.get(tid) != faction:
            continue
        name = p[13].strip()
        words = name.split()
        if len(words) < 2 or "TTC-" in name or "Test" in name:
            continue
        effects = [int(e) for e in p[11].split(",") if e]
        if not effects or words[-1] not in ships:
            continue
        attrs = {int(k): float(v) for k, v in (x.split("=") for x in p[12].split(";") if x)}
        needs = needs_of(attrs, effects)
        rows.append((tid, group, CLASSES[ships[words[-1]]], " ".join(words[:-1]),
                     ",".join(needs), role_of(needs), wreck.get(tid, ""),
                     "[V-data] faction %d, ship %s, effects %s" % (
                         faction, words[-1], ",".join(map(str, effects)))))
    rows.sort()
    return list(COMMENTS) + [HEADER] + ["\t".join(map(str, r)) for r in rows]


def _bundle_rows(ctx, rel):
    return list(_content().read_rows(ctx.path(rel)))


def _derive(ctx):
    npc = _npc()
    return derive(_bundle_rows(ctx, "types/typedata.tsv"),
                  _bundle_rows(ctx, "types/market.tsv"), npc.needs_of, npc.role_of)


def section(ctx):
    lines = _derive(ctx)
    ctx.note("pochven: %d Triglavian types" % (len(lines) - len(COMMENTS) - 1))
    c = _content()
    return [(FILE, None, lines, [c.fsdbin("types")])]


def check(ctx):
    if not os.path.exists(ctx.path(FILE)):
        return ["%s: missing from the bundle" % FILE]
    with open(ctx.path(FILE), encoding="utf-8") as fh:
        got = fh.read().splitlines()
    want = _derive(ctx)
    if got != want:
        return ["%s: differs from a fresh derive (%d vs %d lines)" % (FILE, len(got), len(want))]
    return []
