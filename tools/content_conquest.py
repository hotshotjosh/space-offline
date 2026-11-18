"""The `conquest` content section: the client's FW Conquest and Path to Conquest data (#291, CQ-1).

Lane CQ-1 (`specs/conquests.md` §3 and §8). A real `content.py` section, found by glob.
It reads the client's `spacecomponentsbytype`, `itemtraderrecipes`, `types`, `groups`,
the localisation label index and the resource index, plus the bundle's own `dungeons/`
files (so `DEPENDS` names `dungeons`), and writes six files into the bundle:

    conquest/emitters.tsv  #type_id kind faction_id step filter interval duration collection_id value
                           kind: sfw (the 80 War Points Threshold emitters), ptc (the 16 Path to
                           Conquest system-wide effects), general (the 4 FW generals' own
                           emitters), engine_hp (the Conquest Engine Hitpoints step's
                           `globalDebuffs`). One row per (collection, value): an emitter with an
                           empty dict writes none. `filter` is the client's `FW_<FACTION>_ENLISTED`,
                           or `typelist=<id>` for a `globalDebuffs` row.
    conquest/traders.tsv   #type_id faction_id dungeon_id interaction_range x y z
    conquest/recipes.tsv   #recipe_id trader_type_id group side type_id qty input_isk
    conquest/texts.tsv     #faction_id key message_id
    conquest/textures.tsv  #faction_id key res_path
    conquest/npcs.tsv      #type_id group_id role faction_id

Authored, `content/conquest/season.tsv` (design numbers, each [A], read through `ctx.input`
so a change re-extracts; validated here, not copied into the bundle):

    key faction_id kind value why

Every derivation is pure (plain rows in, tuples out); the unit tests feed invented rows.
`gather(ctx)` is the only client reader. A thing the client does not carry (a missing label,
a recipe a trader names but the table lacks, a trader in no dungeon) writes no row and one
note; `--check` repeats the notes.

Faction ids: the client's own `FW_<FACTION>_ENLISTED` filter and the type names carry the
faction; the ids are the `factions` rows [V] (500001 Caldari, 500002 Minmatar, 500003 Amarr,
500004 Gallente).
"""
import os
import re
import sys

SECTION = "conquest"
DEPENDS = ("dungeons",)
CFSD = ("spacecomponentsbytype", "itemtraderrecipes", "types", "groups")

_HERE = os.path.dirname(os.path.abspath(__file__))

RES = "res:/staticdata/"
LABELS_RES = "res:/localizationfsd/localization_fsd_main.pickle"
SOURCES = [RES + "spacecomponentsbytype.fsdbinary", RES + "itemtraderrecipes.fsdbinary",
           RES + "types.fsdbinary", RES + "groups.fsdbinary",
           "res:/localizationfsd/localization_fsd_en-us.pickle", LABELS_RES]

EMITTERS_HEADER = ("type_id", "kind", "faction_id", "step", "filter", "interval", "duration",
                   "collection_id", "value")
TRADERS_HEADER = ("type_id", "faction_id", "dungeon_id", "interaction_range", "x", "y", "z")
RECIPES_HEADER = ("recipe_id", "trader_type_id", "group", "side", "type_id", "qty", "input_isk")
TEXTS_HEADER = ("faction_id", "key", "message_id")
TEXTURES_HEADER = ("faction_id", "key", "res_path")
NPCS_HEADER = ("type_id", "group_id", "role", "faction_id")

#: The `factions` ids [V] by the word the client's names use.
FACTIONS = {"amarr": 500003, "caldari": 500001, "gallente": 500004, "minmatar": 500002}
#: Words that name a faction in a group or type name (`Imperial Conquest Engine`, `FW Amarr
#: Empire General`) [V names].
FACTION_WORDS = {"amarr": "amarr", "imperial": "amarr", "caldari": "caldari", "state": "caldari",
                 "gallente": "gallente", "federation": "gallente", "minmatar": "minmatar",
                 "republic": "minmatar"}

#: `groupSystemDbuffEmitter`-class type group of both the SFW and the PTC types [V comp rows].
GROUP_EMITTER = 227
#: The groups of the item traders (`Into the Light Trader`, ...) [V groups].
GROUP_TRADER = 5254
#: The 17 groups the 3707965 build adds for the conquest NPCs and engines [V groups: 5186-5189
#: generals, 5190-5193 dreadnoughts, 5194-5197 titans (no type), 5198 engines, 5265-5268
#: shuttles].
NPC_GROUPS = tuple(range(5186, 5199)) + tuple(range(5265, 5269))
GENERAL_GROUPS = tuple(range(5186, 5190))
#: The role by the last word of the group name [V names].
ROLE_BY_SUFFIX = (("general", "general"), ("dreadnought", "besieger"), ("titan", "titan"),
                  ("shuttle", "shuttle"), ("conquest engine", "engine"))
#: The PTC career word in a type name -> the client's career number [V `career_paths`:
#: EXPLORATION 1, INDUSTRIALIST 2, ENFORCER 3, SOLDIER_OF_FORTUNE 4]. It is the `step` of a
#: `ptc` row.
PTC_CAREERS = {"explorer": 1, "industrialist": 2, "enforcer": 3, "soldier": 4}

SFW_RE = re.compile(r"^SFW (\w+) Conquest Track System Dbuff Emitter (\d+)\b")
PTC_RE = re.compile(r"^PTC - (\w+) (\w+) System Wide Effect\b")
FILTER_RE = re.compile(r"^FW_([A-Z]+)_ENLISTED$")

#: Texture folders scanned in the resource index. The spec names `2026_q4`; the faction art
#: of the military campaigns sits in `2026_q2` [V resfileindex], which the season-2 theme packs
#: may use [I].
TEXTURE_PREFIXES = ("res:/ui/texture/classes/campaigns/2026_q4/",
                    "res:/ui/texture/classes/campaigns/2026_q2/")

#: The campaign label -> the keys it fills (`specs/conquests.md` §2.1) [V `campaign_controller*`].
#: Attribute fields have no suffix, annotations end in `_message_id`. `Description` fills both
#: the goal attribute and the briefing annotation [I].
CAMPAIGN_LABELS = (
    ("Title", ("title",)),
    ("CampaignSubtitle", ("subtitle",)),
    ("Description", ("goal_description", "briefing_goal_description_message_id")),
    ("CompletionSuccessDescription", ("success_description",)),
    ("CompletionFailureDescription", ("failure_description",)),
    ("CampaignBriefingVictory", ("briefing_success_description_message_id",)),
    ("CampaignBriefingDefeat", ("briefing_failure_description_message_id",)),
    ("CampaignFinalWords", ("briefing_final_words_message_id",)),
    ("MapScreenSubheader", ("map_subheader_message_id",)),
    ("MapScreenSection1Title", ("map_section_1_title_message_id",)),
    ("MapScreenSection1Paragraph", ("map_section_1_paragraph_message_id",)),
    ("info_label_message_id", ("info_label_message_id",)),
    ("info_label_hint_message_id", ("info_label_hint_message_id",)),
)
CAMPAIGN_LABEL_ROOT = "Campaigns/2026_ptc/CampaignData/%s/%s"
OBJECTIVE_KINDS = ("pve", "pvp", "industry", "hacking")
OBJECTIVE_TITLE = "Campaigns/2026_ptc/ObjectiveData/Titles/%s/ptc_%s_%s"
OBJECTIVE_DESCRIPTION = "Campaigns/2026_ptc/ObjectiveData/Descriptions/%s/ptc_%s_%s_description"


class ContentError(ValueError):
    """A refusal: the authored file is wrong."""


# --- the authored season table ----------------------------------------------------

AUTHORED_HEADER = "key\tfaction_id\tkind\tvalue\twhy"
OBJECTIVE_KIND_KEYS = ("target_multiplier",)
#: key -> ("int", minimum) or ("enum", allowed). The owner-choice enums list every option the
#: spec weighed, so another ruling is an edit of season.tsv, not of code. `per_faction` keys need a faction id (not 0);
#: `per_kind` keys need a kind in `OBJECTIVE_KINDS`; every other key is `0` and `-`.
KEYS = {
    "season_mode": ("enum", ("rolling",)),
    "season_days": ("int", 1),
    "ptc_days": ("int", 1),
    "siege_start_day": ("int", 1),
    "siege_hours": ("int", 1),
    "siege_selection": ("enum", ("most_wp", "losing_side_frontline")),
    "ptc_scope": ("enum", ("ptc_only", "ptc_and_conquest")),
    "client_patch": ("enum", ("none", "show_conquest_pages")),
    "trader_access": ("enum", ("militia", "anyone")),
    "engine_shooters": ("enum", ("anyone", "enemy_militia")),
    "tokens_count_as_earned": ("int", 0),
    "target_multiplier": ("int", 1),
    "per_pilot_cap_pct": ("int", 1),
    "commit_limit": ("int", 1),
    "trader_multiplier_cap": ("int", 1),
    "wp_step": ("int", 1),
    "wp_complex_vp_divisor": ("int", 1),
    "wp_kill": ("int", 0),
    "wp_objective": ("int", 0),
    "wp_flip": ("int", 0),
    "war_hq_system": ("int", 1),
    "war_hq_station": ("int", 1),
}
PER_FACTION = ("war_hq_system", "war_hq_station")
PER_KIND = ("target_multiplier",)
#: Keys a season must carry.
REQUIRED = ("season_mode", "season_days", "ptc_days", "siege_start_day", "siege_hours",
            "siege_selection", "ptc_scope", "client_patch", "trader_access", "engine_shooters",
            "tokens_count_as_earned", "target_multiplier", "per_pilot_cap_pct", "commit_limit",
            "trader_multiplier_cap", "wp_step", "war_hq_system", "war_hq_station")


def parse_season(lines):
    """The authored rows as dicts `{key, faction_id, kind, value, why}` (value is an `int` for
    an `int` key). The header must be the first non-comment line. Refuses a wrong header or
    field count, an unknown key, a bad value, a missing `why`, a duplicate row, a faction id
    where none belongs (or none where one is needed), a bad kind, and a missing required key."""
    lines = [l.rstrip("\r") for l in lines]
    lines = [l for l in lines if l.strip() and not l.startswith("#")]
    if not lines or lines[0] != AUTHORED_HEADER:
        raise ContentError("season.tsv: header is %r, expected %r" % (lines[:1], AUTHORED_HEADER))
    names = AUTHORED_HEADER.split("\t")
    out, seen = [], set()
    for n, l in enumerate(lines[1:], 2):
        f = l.split("\t")
        if len(f) != len(names):
            raise ContentError("season.tsv: row %d: %d fields, expected %d" % (n, len(f), len(names)))
        r = dict(zip(names, f))
        key = r["key"]
        if key not in KEYS:
            raise ContentError("season.tsv: row %d: unknown key %r" % (n, key))
        if not r["why"].strip():
            raise ContentError("season.tsv: %s: the why column is empty" % key)
        kind_of, arg = KEYS[key]
        if kind_of == "int":
            try:
                v = int(r["value"])
            except ValueError:
                raise ContentError("season.tsv: %s: value %r is not a whole number" % (key, r["value"]))
            if v < arg:
                raise ContentError("season.tsv: %s: value %d is below %d" % (key, v, arg))
            r["value"] = v
        elif r["value"] not in arg:
            raise ContentError("season.tsv: %s: value %r is not one of %s" % (key, r["value"], "|".join(arg)))
        try:
            fid = int(r["faction_id"])
        except ValueError:
            raise ContentError("season.tsv: %s: faction_id %r is not a number" % (key, r["faction_id"]))
        if key in PER_FACTION:
            if fid not in FACTIONS.values():
                raise ContentError("season.tsv: %s: faction_id %d is not a faction" % (key, fid))
        elif fid != 0:
            raise ContentError("season.tsv: %s: faction_id must be 0" % key)
        r["faction_id"] = fid
        if key in PER_KIND:
            if r["kind"] not in OBJECTIVE_KINDS:
                raise ContentError("season.tsv: %s: kind %r is not one of %s"
                                   % (key, r["kind"], "|".join(OBJECTIVE_KINDS)))
        elif r["kind"] != "-":
            raise ContentError("season.tsv: %s: kind must be '-'" % key)
        ident = (key, fid, r["kind"])
        if ident in seen:
            raise ContentError("season.tsv: %s is listed twice for faction %d kind %s" % ident)
        seen.add(ident)
        out.append(r)
    have = {r["key"] for r in out}
    for key in REQUIRED:
        if key not in have:
            raise ContentError("season.tsv: required key %s is missing" % key)
    for key in PER_FACTION:
        got = {r["faction_id"] for r in out if r["key"] == key}
        if got != set(FACTIONS.values()):
            raise ContentError("season.tsv: %s needs one row per faction" % key)
    got = {r["kind"] for r in out if r["key"] == "target_multiplier"}
    if got != set(OBJECTIVE_KINDS):
        raise ContentError("season.tsv: target_multiplier needs one row per objective kind")
    return out


# --- derivation: pure ----------------------------------------------------------------

def _faction_of_words(text):
    """The faction a name's words point at, or None (also None when two disagree)."""
    found = {FACTION_WORDS[w] for w in re.findall(r"[a-z]+", (text or "").lower())
             if w in FACTION_WORDS}
    return FACTIONS[found.pop()] if len(found) == 1 else None


def _filter_faction(filt):
    m = FILTER_RE.match(filt or "")
    return FACTIONS.get(m.group(1).lower()) if m else None


def _collections(d):
    """`[(collection id, value)]` of a `{collection: value}` dict, by id."""
    return sorted((int(k), float(v)) for k, v in (d or {}).items())


def derive_emitters(comp, types):
    """`(rows, notes)`. `comp` is `{type_id: {component: dict}}`; `types` is `{type_id:
    (group_id, name)}`. A type is read by its name: an SFW threshold, a PTC effect, or (by
    group) a general. An emitter whose faction in the name and in its filter disagree, a PTC
    career the table does not know, or a name that fits no kind is noted and writes nothing."""
    rows, notes = [], []
    for tid in sorted(comp):
        c = comp[tid]
        emitter, wide = c.get("systemDbuffEmitter"), c.get("systemWideEffects")
        glob = (wide or {}).get("globalDebuffs")
        if not emitter and not glob:
            continue
        group, name = types.get(tid, (None, ""))
        if group != GROUP_EMITTER and group not in GENERAL_GROUPS:
            continue
        m = SFW_RE.match(name)
        p = PTC_RE.match(name)
        if m and emitter:
            if not _collections(emitter.get("dbuffCollections")) and not glob:
                continue  # an empty step: the client gives it no filter either
            faction = FACTIONS.get(m.group(1).lower())
            step = int(m.group(2))
            if faction is None or faction != _filter_faction(emitter.get("filterFunction")):
                notes.append("emitter %d: faction %r disagrees with filter %r"
                             % (tid, m.group(1), emitter.get("filterFunction")))
                continue
            for cid, val in _collections(emitter.get("dbuffCollections")):
                rows.append((tid, "sfw", faction, step, emitter["filterFunction"],
                             int(emitter.get("interval") or 0), int(emitter.get("duration") or 0),
                             cid, val))
            if glob:
                for cid, val in _collections(glob.get("dbuffs")):
                    rows.append((tid, "engine_hp", faction, step,
                                 "typelist=%d" % glob["eligibleTypeListID"], 0, 0, cid, val))
        elif p and glob:
            faction = FACTIONS.get(p.group(1).lower())
            career = PTC_CAREERS.get(p.group(2).lower())
            if faction is None or career is None:
                notes.append("emitter %d: PTC faction or career %r %r is not known"
                             % (tid, p.group(1), p.group(2)))
                continue
            for cid, val in _collections(glob.get("dbuffs")):
                rows.append((tid, "ptc", faction, career,
                             "typelist=%d" % glob["eligibleTypeListID"], 0, 0, cid, val))
        elif group in GENERAL_GROUPS and emitter:
            faction = _filter_faction(emitter.get("filterFunction"))
            if faction is None:
                notes.append("emitter %d: general with filter %r" % (tid, emitter.get("filterFunction")))
                continue
            for cid, val in _collections(emitter.get("dbuffCollections")):
                rows.append((tid, "general", faction, 0, emitter["filterFunction"],
                             int(emitter.get("interval") or 0), int(emitter.get("duration") or 0),
                             cid, val))
        else:
            notes.append("emitter %d (%r) fits no conquest kind" % (tid, name))
    return rows, notes


def derive_traders(comp, types, dungeon_factions, objects):
    """`(rows, notes)`. A trader is a type of `GROUP_TRADER` with an `itemTrader` component.
    `dungeon_factions` is `{dungeon_id: faction id or 0}`; `objects` is `[(dungeon_id, type_id,
    x, y, z)]`. A dungeon without a faction takes the faction of the other dungeons holding
    the same trader; two factions for one trader, or none, is noted (faction 0)."""
    rows, notes = [], []
    for tid in sorted(comp):
        trader = comp[tid].get("itemTrader")
        if not trader or types.get(tid, (None, ""))[0] != GROUP_TRADER:
            continue
        placed = [(d, x, y, z) for d, t, x, y, z in objects if t == tid]
        if not placed:
            notes.append("trader %d is in no dungeon" % tid)
            continue
        facs = {dungeon_factions.get(d, 0) for d, _, _, _ in placed} - {0}
        if len(facs) > 1:
            notes.append("trader %d: dungeons name %d factions" % (tid, len(facs)))
        faction = sorted(facs)[0] if facs else 0
        if not facs:
            notes.append("trader %d: no dungeon names a faction" % tid)
        seen = set()
        for d, x, y, z in sorted(placed):
            if d in seen:
                continue
            seen.add(d)
            rows.append((tid, faction, d, int(trader.get("interactionRange") or 0), x, y, z))
    return rows, notes


def derive_recipes(comp, recipes):
    """`(rows, notes)`: for every `itemTrader` type, each recipe it lists, an `in` row per
    input and an `out` row per output. A recipe the table lacks is noted and writes nothing.
    `recipes` is `{recipe_id: {groupID, inputItems, outputItems, inputIsk}}` (item ids may be
    strings)."""
    rows, notes = [], []
    for tid in sorted(comp):
        trader = comp[tid].get("itemTrader")
        if not trader:
            continue
        done = set()
        for rid in trader.get("recipes") or []:
            rid = int(rid)
            if rid in done:
                continue
            done.add(rid)
            r = recipes.get(rid)
            if r is None:
                notes.append("trader %d lists recipe %d, which the table lacks" % (tid, rid))
                continue
            group = r.get("groupID") or ""
            isk = int(r.get("inputIsk") or 0)
            for side, key in (("in", "inputItems"), ("out", "outputItems")):
                for item, qty in sorted((int(k), int(v)) for k, v in (r.get(key) or {}).items()):
                    rows.append((rid, tid, group, side, item, qty, isk))
    return rows, notes


def derive_texts(labels):
    """`(rows, notes)` from `{'Path/Label': message id}`. A label the client lacks writes no
    row and one note."""
    rows, notes = [], []
    for fname, fid in sorted(FACTIONS.items(), key=lambda kv: kv[1]):
        word = fname.capitalize()
        wanted = []
        for label, keys in CAMPAIGN_LABELS:
            wanted.append((CAMPAIGN_LABEL_ROOT % (word, label), keys))
        for kind in OBJECTIVE_KINDS:
            wanted.append((OBJECTIVE_TITLE % (word, fname, kind), ("objective_%s_title" % kind,)))
            wanted.append((OBJECTIVE_DESCRIPTION % (word, fname, kind),
                           ("objective_%s_description" % kind,)))
        for path, keys in wanted:
            mid = labels.get(path)
            if mid is None:
                notes.append("no label %s" % path)
                continue
            for key in keys:
                rows.append((fid, key, int(mid)))
    return rows, notes


def derive_textures(index_keys):
    """`rows` from the resource index keys (lower case `res:/...`): every file under
    `TEXTURE_PREFIXES`. A file whose name carries a faction word as one `_` token is that
    faction's (the token is dropped from the key); the others are faction 0. The key is
    `<season>/<folder>/<name>`; the path is the index's spelling."""
    rows = []
    for path in sorted(index_keys):
        low = path.lower()
        for prefix in TEXTURE_PREFIXES:
            if not low.startswith(prefix):
                continue
            rest = low[len(prefix):]
            season = prefix.rstrip("/").rsplit("/", 1)[1]
            folder, _, name = rest.rpartition("/")
            stem = name.rsplit(".", 1)[0]
            tokens = stem.split("_")
            hit = [t for t in tokens if t in FACTIONS]
            faction = FACTIONS[hit[0]] if len(hit) == 1 else 0
            if faction:
                tokens.remove(hit[0])
            rest_name = "_".join(tokens)
            parts = [season] + ([folder] if folder else []) + ([rest_name] if rest_name else [])
            rows.append((faction, "/".join(parts), path))
            break
    rows.sort(key=lambda r: (r[1], r[0]))
    return rows


def derive_npcs(types, groups):
    """`rows`: every type of `NPC_GROUPS`. `types` is `{type_id: (group_id, name)}`, `groups`
    `{group_id: name}`. The role is the group name's last words, the faction the faction word
    in the group or type name. Returns `(rows, notes)`: a group whose name gives no role writes no
    row and a note; a faction that cannot be read is written as 0 and noted."""
    rows, notes = [], []
    for tid in sorted(types):
        gid, name = types[tid]
        if gid not in NPC_GROUPS:
            continue
        gname = (groups.get(gid) or "").lower()
        role = next((r for suffix, r in ROLE_BY_SUFFIX if gname.endswith(suffix)), None)
        if role is None:
            notes.append("npc %d: group %d (%r) names no role" % (tid, gid, groups.get(gid)))
            continue
        faction = _faction_of_words(groups.get(gid)) or _faction_of_words(name) or 0
        if not faction:
            notes.append("npc %d: no faction in group %r or type %r" % (tid, groups.get(gid), name))
        rows.append((tid, gid, role, faction))
    return rows, notes


# --- the client readers -----------------------------------------------------------------

def _driver(ctx):
    return sys.modules[type(ctx).__module__]


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def _plain(v):
    """A cFSD row value as plain dicts, lists and scalars; `None` fields dropped."""
    if isinstance(v, dict):
        return {k: _plain(x) for k, x in v.items() if x is not None}
    if isinstance(v, (list, tuple)):
        return [_plain(x) for x in v]
    if isinstance(v, (int, float, str, bool)) or v is None:
        return v
    try:
        return {str(k): _plain(x) for k, x in dict(v).items() if x is not None}
    except (TypeError, ValueError):
        return repr(v)


def gather(ctx):
    """`(comp, recipes, types, groups, labels, index keys)` from the client."""
    drv = _driver(ctx)
    fsd = drv._imp("fsd")  # noqa: SLF001
    loctable = drv._imp("loctable")  # noqa: SLF001
    c = ctx.client
    comp = {}
    for t, r in fsd.rows(c, "spacecomponentsbytype").items():
        d = _plain(dict(r))
        keep = {k: d[k] for k in ("systemDbuffEmitter", "systemWideEffects", "itemTrader") if d.get(k)}
        if keep:
            comp[int(t)] = keep
    recipes = {int(k): _plain(dict(r)) for k, r in fsd.rows(c, "itemtraderrecipes").items()}
    types = {int(t): (r.get("groupID"), ctx.text(r.get("typeNameID")))
             for t, r in fsd.rows(c, "types").items()}
    groups = {int(g): ctx.text(r.get("groupNameID")) for g, r in fsd.rows(c, "groups").items()}
    labels = loctable.label_index(loctable.load_main(path=c.path(LABELS_RES)))
    return comp, recipes, types, groups, labels, list(c.index)


def _bundle_dungeons(ctx):
    """`(dungeon_factions, objects)` from the bundle's `dungeons/` files: the dungeons that hold
    a trader are not read from the client twice."""
    content = _content()
    factions = {}
    for r in content.read_rows(ctx.path("dungeons/dungeons.tsv")):
        factions[int(r[0])] = int(r[2]) if r[2] else 0
    objects = []
    for r in content.read_rows(ctx.path("dungeons/objects.tsv")):
        if r[4] == "" or r[5] == "" or r[6] == "":
            continue
        objects.append((int(r[0]), int(r[3]), float(r[4]), float(r[5]), float(r[6])))
    return factions, objects


def _lines(path):
    with open(path, encoding="utf-8") as fh:
        return fh.read().splitlines()


def _season(ctx):
    path = ctx.input(os.path.join(os.path.dirname(_HERE), "content", "conquest", "season.tsv"))
    try:
        return parse_season(_lines(path))
    except ContentError as e:
        raise _driver(ctx).ContentError(str(e)) from None


def _derive(ctx):
    """`({relpath: (header, rows)}, notes)`."""
    _season(ctx)
    comp, recipes, types, groups, labels, index = gather(ctx)
    dfac, objects = _bundle_dungeons(ctx)
    emitters, n1 = derive_emitters(comp, types)
    traders, n2 = derive_traders(comp, types, dfac, objects)
    recs, n3 = derive_recipes(comp, recipes)
    texts, n4 = derive_texts(labels)
    npcs, n5 = derive_npcs(types, groups)
    out = {"conquest/emitters.tsv": (EMITTERS_HEADER, emitters),
           "conquest/traders.tsv": (TRADERS_HEADER, traders),
           "conquest/recipes.tsv": (RECIPES_HEADER, recs),
           "conquest/texts.tsv": (TEXTS_HEADER, texts),
           "conquest/textures.tsv": (TEXTURES_HEADER, derive_textures(index)),
           "conquest/npcs.tsv": (NPCS_HEADER, npcs)}
    return out, n1 + n2 + n3 + n4 + n5


def TEXTURE_SOURCES(rows):  # noqa: N802
    """textures.tsv reads the resource index only; the manifest has no entry for the index
    itself, so its sources are the texture files it lists (each is an index row with an md5)."""
    return sorted({r[2] for r in rows})


def section(ctx):
    files, notes = _derive(ctx)
    for n in notes:
        ctx.note("conquest: " + n)
    ctx.note("conquest: " + ", ".join("%s %d rows" % (p.split("/")[1][:-4], len(v[1]))
                                      for p, v in files.items()))
    return [(p, h, rows, TEXTURE_SOURCES(rows) if p.endswith("textures.tsv") else SOURCES)
            for p, (h, rows) in files.items()]


def check(ctx):
    content = _content()
    try:
        files, notes = _derive(ctx)
    except (ContentError, _driver(ctx).ContentError) as e:
        return ["conquest: %s" % e]
    bad = []
    for n in notes:
        ctx.note("conquest: " + n)
    for rel, (_h, want) in files.items():
        path = ctx.path(rel)
        if not os.path.exists(path):
            bad.append("%s: missing from the bundle" % rel)
            continue
        got = list(content.read_rows(path))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)" % (rel, len(got), len(want)))
    if not bad:
        ctx.note("conquest: %d files match a fresh derive" % len(files))
    return bad
