"""The `proving` content section: the proving-grounds match types (PG-1a, issue #64).

A real `content.py` section, found by glob. It reads the client's
`pvpfilamentmatchtypes` table (layout in `findings/pyd-schemas.tsv`, stride 96
`[V]`) and writes the two files the proving-grounds server reads:

    proving/match_types.tsv    one row per match type: the format (fleets and
                               fleet size), the filament type that opens it,
                               the restriction ids and the message ids. A
                               field the row's presence bit leaves clear is
                               written empty (the client's getter returns
                               `None` for it).
    proving/match_rewards.tsv  one row per `rewards` entry:
                               `match_type_id  amount  type_id`, in the
                               client's list order.

`filamentTypeID` must be a type of group 4050 (the filament group); `check`
flags any that is not. `provingGroundLootTableID` names a loot table the client
does not ship (`docs/re/proving-grounds.md` §3.3), so it is carried as an id
only. Message ids are written as ids; the server does not resolve them.

The derivation is pure (plain dicts in, rows out); the unit tests feed it
invented rows. `gather` is the only client reader.
"""
import os
import sys

SECTION = "proving"
DEPENDS = ()
CFSD = ("pvpfilamentmatchtypes", "types")

_HERE = os.path.dirname(os.path.abspath(__file__))

#: The filament item group. [V] every filamentTypeID of build 3670638's
#: pvpfilamentmatchtypes is a type of group 4050 (content.py --check, PG-1a regen).
FILAMENT_GROUP = 4050

TYPES_FILE = "proving/match_types.tsv"
REWARDS_FILE = "proving/match_rewards.tsv"

#: Scalar columns of `match_types.tsv`, in order: the client's field name each.
#: The first column, `match_type_id`, is the table key.
FIELDS = (
    "filamentTypeID", "numberOfFleets", "fleetSize", "minimumQueueSizeMultiplier",
    "shipRestrictions", "maxOfShipTypeRestriction", "maxOfShipGroupRestriction",
    "moduleAndImplantMetaLevelRestrictions", "bannedModulesTypeListID",
    "limitedAmountOfModulesTypeListID", "numberOfLimitedModules",
    "provingGroundLootTableID", "eventNameID", "eventDescriptionID",
    "filamentDescriptionID", "restrictionsDescriptionID", "rewardsHintID",
    "rulesHintID", "shipRestrictionsHintID",
)
TYPES_HEADER = ("match_type_id", "filament_type_id", "fleets", "fleet_size",
                "min_queue_multiplier", "ship_restrictions", "max_of_ship_type",
                "max_of_ship_group", "meta_level_cap", "banned_modules_list",
                "limited_modules_list", "limited_modules_count", "loot_table_id",
                "event_name_id", "event_description_id", "filament_description_id",
                "restrictions_description_id", "rewards_hint_id", "rules_hint_id",
                "ship_restrictions_hint_id")
REWARDS_HEADER = ("match_type_id", "amount", "type_id")
SOURCES = ["res:/staticdata/pvpfilamentmatchtypes.fsdbinary",
           "res:/staticdata/types.fsdbinary"]


def derive(matches):
    """`(type rows, reward rows)` from `{key: row}`.

    A field that is `None` (presence bit clear) becomes an empty field; a
    missing `rewards` is no reward row."""
    trows, rrows = [], []
    for key in sorted(matches):
        row = matches[key]
        trows.append((key,) + tuple(row.get(f) for f in FIELDS))
        for rew in row.get("rewards") or ():
            rrows.append((key, rew.get("amount"), rew.get("typeID")))
    return trows, rrows


def problems(matches, types):
    """Failure strings: a filament type that is not a type of group 4050."""
    bad = []
    for key in sorted(matches):
        tid = matches[key].get("filamentTypeID")
        group = (types.get(tid) or {}).get("groupID")
        if group != FILAMENT_GROUP:
            bad.append("match type %s: filamentTypeID %s is not in group %d"
                       % (key, tid, FILAMENT_GROUP))
    return bad


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _matches(ctx):
    """The client's match types, `{key: row}`."""
    return _content()._imp("fsd").rows(ctx.client, "pvpfilamentmatchtypes")  # noqa: SLF001


def gather(ctx):
    """The client's tables: `(match types, types)`."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    return _matches(ctx), fsd.rows(ctx.client, "types")


def section(ctx):
    trows, rrows = derive(_matches(ctx))
    ctx.note("proving: %d match types, %d reward rows" % (len(trows), len(rrows)))
    return [(TYPES_FILE, TYPES_HEADER, trows, SOURCES),
            (REWARDS_FILE, REWARDS_HEADER, rrows, SOURCES)]


def check(ctx):
    content = _content()
    matches, types = gather(ctx)
    bad = problems(matches, types)
    trows, rrows = derive(matches)
    for rel, want in ((TYPES_FILE, trows), (REWARDS_FILE, rrows)):
        got = list(content.read_rows(ctx.path(rel)))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want)))
    return bad
