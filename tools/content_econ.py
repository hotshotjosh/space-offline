"""The `econ` content section: blueprints, stations and the price oracle.

Packet CP24 (`docs/content/plan.md` §6.2, W13). Spec: C5 §3.2-§3.5 (E1 +
E10), C1 §2.8; every header is `docs/content/schema.md`'s. Writes four
bundle files:

    industry/blueprints.tsv   `blueprints.static`, one row per activity and
                              per material, product and skill of it
    stations/stations.tsv     `mapObjects.db npcStations`, sorted by id
    stations/operations.tsv   `stationoperations`
    econ/prices.tsv           `P_ref`, C5 §3.2, one row per priced market type

It reads four authored files, `content/econ/meta-mult.tsv`,
`content/econ/price-overrides.tsv`, `content/econ/recipe-markup.tsv` and
`content/econ/pi-markup.tsv` (manifest `input` rows), and two bundle
files of `types2` (CP12) at extract time: `types/market.tsv` and
`types/materials.tsv`. With `industry/blueprints.tsv` these replace the
committed `industry/tables.rs` and `reprocess/tables.rs` (tracker §66):
blueprints, science and invention are `blueprints.tsv`; the adjusted and
base prices are `prices.tsv`; the per-station reprocessing terms are
`stations.tsv`; materials and compression are `types2`'s.

The derivation is pure (plain dicts in, rows out), so the unit tests feed it
invented rows; `gather(ctx)` is the only client reader.
"""

import math
import os
import sqlite3
import sys

SECTION = "econ"
DEPENDS = ("types2",)
#: The cFSD stems the schema guard covers (`schematics` is read through
#: `dogma`'s hand layout, which the guard checks against the same loader).
CFSD = ("stationoperations", "schematics")

_HERE = os.path.dirname(os.path.abspath(__file__))
AUTHORED = os.path.join(os.path.dirname(_HERE), "content", "econ")
OVERRIDES = os.path.join(AUTHORED, "price-overrides.tsv")
META_MULT = os.path.join(AUTHORED, "meta-mult.tsv")

# The header lines, copied from `schema.md` §2 and §4.
BLUEPRINTS_HEADER = tuple(
    "bp_type_id activity_id kind type_id quantity probability time max_runs".split())
STATIONS_HEADER = tuple(
    "station_id system_id region_id owner_id operation_id services reproc_eff "
    "reproc_take".split())
OPERATIONS_HEADER = tuple(
    "operation_id activity_id services station_types manufacturing_factor "
    "research_factor".split())
PRICES_HEADER = tuple(
    "type_id ref_price source floor_price category_id group_id meta_group_id "
    "portion_size build_min".split())
MODIFIER_SOURCES_HEADER = tuple(
    "type_id activity_id kind attribute_id filter_id".split())
TARGET_FILTERS_HEADER = ("filter_id", "category_ids", "group_ids")
OVERRIDES_HEADER = ("type_id", "price", "why")
META_MULT_HEADER = ("meta_group_id", "mult", "why")

#: `blueprints.static` activity keys -> activity ids. [V] the keys are
#: `industry.ACTIVITY_NAMES` (`docs/lanes/industry.md` §2.1); the ids and
#: their meaning are `industry_activities.static` (§2.2: 3 is time, 4 is
#: material efficiency). `check` proves the id set equals that table's.
ACTIVITY_IDS = {"manufacturing": 1, "research_time": 3, "research_material": 4,
                "copying": 5, "invention": 8, "reaction": 9}
MANUFACTURING, REACTION = 1, 9
#: The per-element list of each activity, in `blueprints.tsv` order, with the
#: field that is written as `quantity`.
LISTS = (("material", "materials", "quantity"), ("product", "products", "quantity"),
         ("skill", "skills", "level"))

#: C5 §3.2: the recursion's depth cap. [V] never reached in the census.
DEPTH_CAP = 12
#: `groupSalvagedMaterials` [V C5 §3.5]: the keystone for rig recipes.
GROUP_SALVAGE = 754
#: `metaGroupID` of a T2 product [V C5 §2.1].
META_T2 = 2
#: [A] C5 §3.5: salvage of tier `i` is `SALVAGE_TIER_BASE × 3^i`, tier 0 when
#: only T1 recipes use it, 2 when only T2 ones do and 1 when both do.
#: Falsifier: rigs whose prices players find wildly off against their hulls.
#: Applied only where nothing else prices the type; a file override wins.
SALVAGE_TIER_BASE = 2000.0
#: `content/econ/recipe-markup.tsv` carries the markup (an [A], see there); it
#: is an authored input, so editing it re-extracts on the next `up.sh`.
RECIPE_MARKUP_FILE = os.path.join(AUTHORED, "recipe-markup.tsv")
RECIPE_MARKUP_HEADER = ("markup", "why")
#: [A] The sanity range of the markup: finite, `MIN_RECIPE_MARKUP <= X <=
#: MAX_RECIPE_MARKUP`. The top is not derived from anything: a markup past 2
#: (a built item worth double its materials, compounding per tier) is a typo,
#: not a setting. Falsifier: an owner decision to run a larger markup. The
#: insurance guard does NOT rest on this range: `service/insurance.rs` caps a
#: payout by `BUILD_COVER x build_min`, the self-build cost.
MIN_RECIPE_MARKUP = 1.0
MAX_RECIPE_MARKUP = 2.0
#: The ME 10 quantity factor of a manufacturing job **[V]** (`build.rs`
#: values materials at the blueprint's ME; ME 10 is the most a builder gets).
#: A reaction has no ME. `build_min` assumes the best case on both, so it
#: stays a LOWER bound of a real build's materials. Falsifier: a facility
#: material bonus (< 1.0) ever added to the server: it would lower real cost
#: below this bound, and the factor would need to follow it.
ME10_FACTOR = 0.9
REACTION_FACTOR = 1.0
#: `content/econ/pi-markup.tsv` carries the planetary-processing markup (an
#: [A], see there); an authored input like the recipe markup (#144).
PI_MARKUP_FILE = os.path.join(AUTHORED, "pi-markup.tsv")
PI_MARKUP_HEADER = ("markup", "why")
#: [A] The sanity range of the PI markup, as `MIN_RECIPE_MARKUP`: a typo guard,
#: not a derived bound. Falsifier: an owner decision to run a larger markup.
MIN_PI_MARKUP = 1.0
MAX_PI_MARKUP = 2.0


def markup_problem(x, lo=None, hi=None):
    """Why `x` is not a usable markup, or `None`. The range defaults to the
    recipe markup's; the PI markup passes its own."""
    lo = MIN_RECIPE_MARKUP if lo is None else lo
    hi = MAX_RECIPE_MARKUP if hi is None else hi
    if not isinstance(x, float) or not math.isfinite(x):
        return "must be a finite number, got %r" % (x,)
    if not lo <= x <= hi:
        return "must be between %s and %s, got %r" % (lo, hi, x)
    return None


class EconError(Exception):
    """A refusal: an authored file's header, or an activity key unknown here."""


# --- authored files (`schema.md` §4, `content/README.md`) ---------------------

def parse_authored(lines, header, name, convs):
    """`#` lines anywhere; the first other line is the header (a mismatch is
    a refusal); every later line a row, converted by `convs` (one per column
    but `why`). Returns `(rows, problems)`: a bad row is skipped and named."""
    rows, problems, seen = [], [], False
    for no, line in enumerate(lines, 1):
        line = line.rstrip("\n")
        if not line.strip() or line.startswith("#"):
            continue
        f = line.split("\t")
        if not seen:
            if tuple(f) != header:
                raise EconError("%s:%d: header %r, want %r"
                                % (name, no, line, "\t".join(header)))
            seen = True
            continue
        try:
            if len(f) != len(header):
                raise ValueError("want %d fields, got %d" % (len(header), len(f)))
            rows.append(tuple(c(v) for c, v in zip(convs, f)))
        except ValueError as e:
            problems.append("%s:%d: skipped (%s)" % (name, no, e))
    return rows, problems


def _positive(v):
    x = float(v)
    if not x > 0:
        raise ValueError("%r is not above 0" % v)
    return x


def parse_overrides(lines):
    rows, problems = parse_authored(lines, OVERRIDES_HEADER, "price-overrides.tsv",
                                    (int, _positive))
    return {t: p for t, p in rows}, problems


def parse_meta_mult(lines):
    rows, problems = parse_authored(lines, META_MULT_HEADER, "meta-mult.tsv",
                                    (int, _positive))
    return {g: m for g, m in rows}, problems


def parse_recipe_markup(lines):
    """`content/econ/recipe-markup.tsv`: exactly one row. Returns the markup;
    a bad file or value is a `ContentError` naming the file, never a traceback."""
    err = _content().ContentError
    name = "recipe-markup.tsv"
    try:
        rows, problems = parse_authored(lines, RECIPE_MARKUP_HEADER, name, (float,))
    except EconError as e:
        raise err(str(e)) from None
    if problems or len(rows) != 1:
        raise err("%s: want exactly one row (markup, why); %s"
                  % (name, "; ".join(problems) or "got %d" % len(rows)))
    why = markup_problem(rows[0][0])
    if why:
        raise err("%s: the markup %s" % (name, why))
    return rows[0][0]


def parse_pi_markup(lines):
    """`content/econ/pi-markup.tsv`: exactly one row. Returns the markup;
    a bad file or value is a `ContentError` naming the file, never a traceback."""
    err = _content().ContentError
    name = "pi-markup.tsv"
    try:
        rows, problems = parse_authored(lines, PI_MARKUP_HEADER, name, (float,))
    except EconError as e:
        raise err(str(e)) from None
    if problems or len(rows) != 1:
        raise err("%s: want exactly one row (markup, why); %s"
                  % (name, "; ".join(problems) or "got %d" % len(rows)))
    why = markup_problem(rows[0][0], MIN_PI_MARKUP, MAX_PI_MARKUP)
    if why:
        raise err("%s: the markup %s" % (name, why))
    return rows[0][0]


# --- industry/blueprints.tsv ---------------------------------------------------

def blueprint_rows(bps):
    """`bps` is `{bp_type_id: blueprints.static document}`. Per blueprint,
    activities ascending by id: the `activity` row, then its materials,
    products and skills in the client's own order."""
    out = []
    for bp in sorted(bps):
        doc = bps[bp]
        runs = int(doc.get("maxProductionLimit") or 0)
        acts = doc.get("activities") or {}
        unknown = sorted(set(acts) - set(ACTIVITY_IDS))
        if unknown:
            raise EconError("blueprint %d: activity %s has no id here" % (bp, unknown))
        for name in sorted(acts, key=ACTIVITY_IDS.get):
            act, aid = acts[name], ACTIVITY_IDS[name]
            time = int(act.get("time") or 0)
            out.append((int(bp), aid, "activity", 0, 0, None, time, runs))
            for kind, key, qty in LISTS:
                for e in act.get(key) or ():
                    p = e.get("probability")
                    out.append((int(bp), aid, kind, int(e["typeID"]), int(e[qty]),
                                None if p is None else float(p), time, runs))
    return out


def blueprints_from_rows(rows):
    """`blueprints.tsv` rows (decoded) back to `blueprints.static` documents,
    with empty lists left out: the round-trip `check` compares."""
    names = {v: k for k, v in ACTIVITY_IDS.items()}
    keys = {kind: (key, qty) for kind, key, qty in LISTS}
    out = {}
    for bp, aid, kind, tid, q, prob, time, runs in rows:
        doc = out.setdefault(bp, {"blueprintTypeID": bp, "maxProductionLimit": runs,
                                  "activities": {}})
        act = doc["activities"].setdefault(names[aid], {"time": time})
        if kind == "activity":
            continue
        key, qty = keys[kind]
        e = {"typeID": tid, qty: q}
        if prob is not None:
            e["probability"] = prob
        act.setdefault(key, []).append(e)
    return out


def normalise(doc):
    """A `blueprints.static` document with empty lists dropped and the
    numbers made plain, so two readings compare equal."""
    acts = {}
    for name, act in (doc.get("activities") or {}).items():
        a = {"time": int(act.get("time") or 0)}
        for _kind, key, qty in LISTS:
            es = [dict({"typeID": int(e["typeID"]), qty: int(e[qty])},
                       **({"probability": float(e["probability"])}
                          if e.get("probability") is not None else {}))
                  for e in act.get(key) or ()]
            if es:
                a[key] = es
        acts[name] = a
    return {"blueprintTypeID": int(doc["blueprintTypeID"]),
            "maxProductionLimit": int(doc.get("maxProductionLimit") or 0),
            "activities": acts}


def recipes(bp_rows):
    """`{product: [(bp, activity_id, [(material, qty)], product_qty)]}` from
    the manufacturing and reaction activities, ascending by blueprint."""
    mats, prods = {}, {}
    for bp, aid, kind, tid, q, _p, _t, _r in bp_rows:
        if aid not in (MANUFACTURING, REACTION):
            continue
        if kind == "material":
            mats.setdefault((bp, aid), []).append((tid, q))
        elif kind == "product":
            prods.setdefault((bp, aid), []).append((tid, q))
    out = {}
    for key in sorted(prods):
        for tid, q in prods[key]:
            if q > 0:
                out.setdefault(tid, []).append((key[0], key[1], mats.get(key, []), q))
    return out


# --- econ/prices.tsv: the oracle (C5 §3.2) -------------------------------------

def salvage_prices(market, recipe_map):
    """C5 §3.5's tier rule over salvage (group `GROUP_SALVAGE`) that some
    recipe uses: `{type_id: price}`. See `SALVAGE_TIER_BASE`."""
    uses = {}
    for product, rs in recipe_map.items():
        t2 = (market.get(product) or {}).get("meta_group_id") == META_T2
        for _bp, _aid, mats, _q in rs:
            for m, _qty in mats:
                if (market.get(m) or {}).get("group_id") == GROUP_SALVAGE:
                    uses.setdefault(m, set()).add(t2)
    tier = {frozenset([False]): 0, frozenset([False, True]): 1, frozenset([True]): 2}
    return {m: SALVAGE_TIER_BASE * 3 ** tier[frozenset(s)] for m, s in uses.items()}


class Oracle:
    """`P_ref`, first match, memoised, depth-capped (C5 §3.2):

    0. a `price-overrides.tsv` row replaces the derived price;
    1. `recipe`/`reaction`: `markup × Σ P_ref(m)·qty / productQuantity` at ME 0
       when every material prices above 0 (lowest blueprint id first);
    2. `pi`: `pi_markup ×` the same over a schematic's inputs and output
       quantity (each tier compounds, through its marked inputs);
    3. `base`: `basePrice` above 0;
    4. `parent`: `P_ref(variationParent) × meta_mult[metaGroupID]`;
    5. `override` again for a rule-seeded price (`rule_overrides`, salvage);
    6. the floor `Σ P_ref(m)·q / portionSize` over `typematerials`, always
       last. A material with no price adds 0, so the floor stays a lower
       bound of the refine value.

    A type on the current path (a cycle) or past `DEPTH_CAP` is unpriced
    for that path, and nothing computed under such a cut is memoised, so a
    result never depends on the order types are asked for. `market` is `{type_id: {base_price, meta_group_id,
    parent, portion_size, ...}}`, `materials` `{type_id: [(m, q)]}`, `pi`
    `{output: ([(input, q)], out_qty)}`."""

    def __init__(self, market, recipe_map, pi, materials, meta_mult,
                 overrides=None, rule_overrides=None, markup=1.0, pi_markup=1.0):
        self.markup = markup
        self.pi_markup = pi_markup
        self.market, self.recipes, self.pi = market, recipe_map, pi
        self.materials, self.meta_mult = materials, meta_mult
        self.overrides, self.rule = overrides or {}, rule_overrides or {}
        self.memo, self.path, self.cut = {}, set(), False
        self.bm_memo, self.bm_path, self.bm_cut = {}, set(), False

    def price(self, t, depth=0):
        """`(ref_price, source, floor_price)`, or None when unpriced."""
        if t in self.memo:
            return self.memo[t]
        if t in self.path or depth > DEPTH_CAP:
            self.cut = True
            return None
        outer, self.cut = self.cut, False
        self.path.add(t)
        try:
            got = self._derive(t, depth)
            if got is not None:
                floor = self._floor(t, depth)
                got = (max(got[0], floor), got[1], floor)
        finally:
            self.path.discard(t)
        if not self.cut:
            self.memo[t] = got
        self.cut = self.cut or outer
        return got

    def ref(self, t, depth):
        got = self.price(t, depth + 1)
        return None if got is None else got[0]

    def _sum(self, parts, depth):
        total = 0.0
        for m, q in parts:
            p = self.ref(m, depth)
            if p is None or not p > 0:
                return None
            total += p * q
        return total

    def _derive(self, t, depth):
        if t in self.overrides:
            return self.overrides[t], "override"
        for _bp, aid, mats, q in self.recipes.get(t, ()):
            s = self._sum(mats, depth) if mats else None
            if s is not None:
                return self.markup * s / q, "reaction" if aid == REACTION else "recipe"
        if t in self.pi:
            ins, q = self.pi[t]
            s = self._sum(ins, depth) if ins and q > 0 else None
            if s is not None:
                return self.pi_markup * s / q, "pi"
        row = self.market.get(t) or {}
        if (row.get("base_price") or 0.0) > 0:
            return float(row["base_price"]), "base"
        mult = self.meta_mult.get(row.get("meta_group_id"))
        if row.get("parent") is not None and mult is not None:
            p = self.ref(row["parent"], depth)
            if p is not None and p > 0:
                return p * mult, "parent"
        if t in self.rule:
            return self.rule[t], "override"
        return None

    def build_min(self, t, depth=0):
        """A lower bound of what one unit of `t` costs a pilot who builds the
        whole chain, or `None` when `t` has no recipe whose materials all price
        (its price source does not matter).

        Over `t`'s blueprints whose materials all price: `min Σ qty · f · unit(m)
        / productQuantity`, `f` = `ME10_FACTOR` (manufacturing) or
        `REACTION_FACTOR`, and `unit(m)` the cheaper of `P_ref(m)` and
        `build_min(m)` (a material that is itself built is built, if that is
        cheaper). The SCC surcharge and job fees are left out: they only add
        cost. Same cycle and `DEPTH_CAP` rules as `price`: a cut result is
        not memoised."""
        if t in self.bm_memo:
            return self.bm_memo[t]
        if t in self.bm_path or depth > DEPTH_CAP:
            self.bm_cut = True
            return None
        outer, self.bm_cut = self.bm_cut, False
        self.bm_path.add(t)
        try:
            got = self._build_min(t, depth)
        finally:
            self.bm_path.discard(t)
        if not self.bm_cut:
            self.bm_memo[t] = got
        self.bm_cut = self.bm_cut or outer
        return got

    def _build_min(self, t, depth):
        # From `t`'s own recipes, whatever its price source: an override-priced
        # type that can be built still has a self-build cost.
        best = None
        for _bp, aid, mats, q in self.recipes.get(t, ()):
            if not mats:
                continue
            f = ME10_FACTOR if aid == MANUFACTURING else REACTION_FACTOR
            total = 0.0
            for m, qty in mats:
                p = self.ref(m, depth)
                if p is None or not p > 0:
                    total = None
                    break
                built = self.build_min(m, depth + 1)
                total += qty * f * (p if built is None else min(p, built))
            if total is not None and (best is None or total / q < best):
                best = total / q
        return best

    def _floor(self, t, depth):
        portion = (self.market.get(t) or {}).get("portion_size") or 1
        total = 0.0
        for m, q in self.materials.get(t, ()):
            if m != t:
                total += (self.ref(m, depth) or 0.0) * q
        return total / portion


def price_rows(market, oracle):
    """`econ/prices.tsv`: every market type (a `market_group_id`) that prices."""
    out = []
    for t in sorted(market):
        row = market[t]
        if row.get("market_group_id") is None:
            continue
        got = oracle.price(t)
        if got is not None:
            out.append((int(t), float(got[0]), got[1], float(got[2]), row.get("category_id"),
                        row.get("group_id"), row.get("meta_group_id"),
                        int(row.get("portion_size") or 1), oracle.build_min(t)))
    return out


# --- stations/ -----------------------------------------------------------------

def operation_rows(ops):
    """`ops` is `{operation_id: stationoperations row}`."""
    return [(int(o), int(r["activityID"]), [int(s) for s in r.get("services") or ()],
             {int(k): int(v) for k, v in sorted((r.get("stationTypes") or {}).items())},
             float(r["manufacturingFactor"]), float(r["researchFactor"]))
            for o, r in sorted(ops.items())]


def station_rows(stations, region_of, ops):
    """`stations` is `[(station_id, system_id, owner_id, operation_id,
    reproc_eff, reproc_take)]`; `region_of` `{system_id: region_id}`."""
    return [(int(s), int(sy), region_of.get(sy), int(own), int(op),
             [int(x) for x in (ops.get(op) or {}).get("services") or ()],
             float(eff), float(take))
            for s, sy, own, op, eff, take in sorted(stations)]


# --- reading the bundle and the client -----------------------------------------

def _content():
    return sys.modules.get("content") or __import__("content")


def _opt(v):
    return int(v) if v else None


def read_market(path):
    """`types/market.tsv` (CP12) as the dict `Oracle` reads."""
    out = {}
    for f in _content().read_rows(path):
        out[int(f[0])] = {"group_id": int(f[1]), "category_id": _opt(f[2]),
                          "market_group_id": _opt(f[3]), "base_price": float(f[4]),
                          "meta_group_id": _opt(f[5]), "parent": _opt(f[8]),
                          "portion_size": int(f[12])}
    return out


def read_materials(path):
    out = {}
    for f in _content().read_rows(path):
        out.setdefault(int(f[0]), []).append((int(f[1]), int(f[2])))
    return out


def read_blueprints(path):
    return [(int(f[0]), int(f[1]), f[2], int(f[3]), int(f[4]),
             float(f[5]) if f[5] else None, int(f[6]), int(f[7]))
            for f in _content().read_rows(path)]


BLUEPRINTS_RES = "res:/staticdata/blueprints.static"
ACTIVITIES_RES = "res:/staticdata/industry_activities.static"
#: #57: what a structure and its rigs change in an industry job. Both are
#: OPTIONAL: a client without them gives no file and the server loads without
#: structure or rig modifiers. [V-data] the layouts are in
#: `parity-structure-services.md` §L2.1.
MODIFIER_SOURCES_RES = "res:/staticdata/industry_activity_modifier_sources.static"
TARGET_FILTERS_RES = "res:/staticdata/industry_activity_target_filters.static"
MODIFIER_KINDS = ("time", "material", "cost")


def modifier_source_rows(doc):
    """`doc` is `{type_id: {activity_name: {time|material|cost: [{dogmaAttributeID,
    filterID?}]}}}` (`industry_activity_modifier_sources.static`). One row per
    list entry, `filter_id` 0 when absent, sorted. An unknown activity name or
    kind is an `EconError`, as in `blueprint_rows`."""
    out = []
    for t in doc:
        for name, kinds in (doc[t] or {}).items():
            if name not in ACTIVITY_IDS:
                raise EconError("modifier sources %s: activity %r has no id here" % (t, name))
            unknown = sorted(set(kinds or {}) - set(MODIFIER_KINDS))
            if unknown:
                raise EconError("modifier sources %s: kind %s is not time, material or cost"
                                % (t, unknown))
            for kind in MODIFIER_KINDS:
                for e in (kinds or {}).get(kind) or ():
                    out.append((int(t), ACTIVITY_IDS[name], kind,
                                int(e["dogmaAttributeID"]), int(e.get("filterID") or 0)))
    return sorted(out)


def target_filter_rows(doc):
    """`doc` is `{filterID: {name, categoryIDs, groupIDs}}`
    (`industry_activity_target_filters.static`): `(filter_id, category_ids,
    group_ids)`, the lists comma-joined, sorted by id."""
    return [(int(k), ",".join(str(int(c)) for c in sorted(doc[k].get("categoryIDs") or ())),
             ",".join(str(int(g)) for g in sorted(doc[k].get("groupIDs") or ())))
            for k in sorted(doc, key=int)]


def _static_doc(ctx, res, rel):
    """A `.static` cache as `{int(key): document}`, or `None` (noted) when the
    client has no such file or it is not a readable sqlite cache."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    try:
        path = ctx.client.path(res)
        if not os.path.isfile(path):
            raise OSError("absent")
        return {int(k): v for k, v in fsd.sqlite_rows(path)}
    except (OSError, KeyError, ValueError, sqlite3.DatabaseError):
        ctx.note("%s: no source layout (%s), not written" % (rel, res))
        return None


def optional_rows(ctx):
    """`{relpath: (header, rows, sources)}` for the two #57 files the client
    has; an absent source is skipped with a note."""
    out = {}
    for rel, res, header, build in (
            ("industry/modifier-sources.tsv", MODIFIER_SOURCES_RES, MODIFIER_SOURCES_HEADER,
             modifier_source_rows),
            ("industry/target-filters.tsv", TARGET_FILTERS_RES, TARGET_FILTERS_HEADER,
             target_filter_rows)):
        doc = _static_doc(ctx, res, rel)
        if doc is None:
            continue
        # The files are optional: a document that is not what this reads (an
        # unknown activity or kind, a missing key) skips the file with a note
        # and never aborts the econ section.
        try:
            rows = build(doc)
        except (EconError, KeyError, ValueError, TypeError, AttributeError) as e:
            ctx.note("%s: unreadable layout (%s: %s), not written"
                     % (rel, e.__class__.__name__, e))
            continue
        ctx.note("%s: %d rows" % (rel, len(rows)))
        out[rel] = (header, rows, [res])
    return out


def map_objects(client):
    """`bin64/staticdata/mapObjects.db`: not a `res:` file, so it has no
    manifest `source` row; `client_build` pins it."""
    return os.path.join(client.bin64, "staticdata", "mapObjects.db")


def gather(ctx):
    """The client side of the derivation, as plain dicts."""
    if "econ" in ctx._cache:  # noqa: SLF001
        return ctx._cache["econ"]  # noqa: SLF001
    content = _content()
    fsd, dogma = content._imp("fsd"), content._imp("dogma")  # noqa: SLF001
    c = ctx.client
    bps = {int(k): v for k, v in fsd.sqlite_rows(c.path(BLUEPRINTS_RES))}
    pi = {}
    for _sid, row in dogma.Dogma(c).table("schematics").rows():
        ts = row.get("types") or {}
        ins = [(int(t), int(e["quantity"])) for t, e in ts.items() if e["isInput"]]
        for t, e in ts.items():
            if not e["isInput"]:
                pi.setdefault(int(t), (sorted(ins), int(e["quantity"])))
    ops = {int(k): dict(v) for k, v in fsd.rows(c, "stationoperations").items()}
    meta = fsd.load_sqlite(map_objects(c))
    try:
        stations = [tuple(r) for r in meta["_connection"].execute(
            "SELECT stationID, solarSystemID, ownerID, operationID, "
            "reprocessingEfficiency, reprocessingStationsTake FROM npcStations")]
    finally:
        meta["_connection"].close()
    systems = ctx.load(content.SYSTEMS)
    region_of = {int(s): systems.get(s).get("regionID") for s in systems}
    got = {"blueprints": bps, "pi": pi, "ops": ops, "stations": stations,
           "region_of": region_of}
    ctx._cache["econ"] = got  # noqa: SLF001
    return got


def authored(ctx, path, parse):
    """An authored file through `ctx.input` (a manifest `input` row)."""
    if not os.path.isfile(path):
        ctx.note("%s: absent, using no rows" % os.path.basename(path))
        return {}
    with open(ctx.input(path), encoding="utf-8") as fh:
        rows, problems = parse(fh)
    for p in problems:
        ctx.note(p)
    return rows


def recipe_markup(ctx):
    """The markup in `content/econ/recipe-markup.tsv`, read through
    `ctx.input` so the manifest records it and `up.sh` re-extracts on an
    edit. A missing file is a `ContentError`."""
    if not os.path.isfile(RECIPE_MARKUP_FILE):
        raise _content().ContentError(
            "%s: missing (the recipe markup is authored there)" % RECIPE_MARKUP_FILE)
    with open(ctx.input(RECIPE_MARKUP_FILE), encoding="utf-8") as fh:
        return parse_recipe_markup(fh)


def pi_markup(ctx):
    """The markup in `content/econ/pi-markup.tsv`, read through `ctx.input`
    (a manifest input, so an edit re-extracts). A missing file is a
    `ContentError`."""
    if not os.path.isfile(PI_MARKUP_FILE):
        raise _content().ContentError(
            "%s: missing (the PI markup is authored there)" % PI_MARKUP_FILE)
    with open(ctx.input(PI_MARKUP_FILE), encoding="utf-8") as fh:
        return parse_pi_markup(fh)


def derive(ctx):
    """`{relpath: rows}`; reads `types2`'s files from this run's bundle."""
    data = gather(ctx)
    bp = blueprint_rows(data["blueprints"])
    market = read_market(ctx.path("types/market.tsv"))
    rmap = recipes(bp)
    oracle = Oracle(market, rmap, data["pi"], read_materials(ctx.path("types/materials.tsv")),
                    authored(ctx, META_MULT, parse_meta_mult),
                    authored(ctx, OVERRIDES, parse_overrides), salvage_prices(market, rmap),
                    markup=recipe_markup(ctx), pi_markup=pi_markup(ctx))
    return {
        "industry/blueprints.tsv": bp,
        "stations/stations.tsv": station_rows(data["stations"], data["region_of"], data["ops"]),
        "stations/operations.tsv": operation_rows(data["ops"]),
        "econ/prices.tsv": price_rows(market, oracle),
    }


def section(ctx):
    rows = derive(ctx)
    content = _content()
    fb = content.fsdbin
    optional = [(rel, h, r, src) for rel, (h, r, src) in sorted(optional_rows(ctx).items())]
    return [
        ("industry/blueprints.tsv", BLUEPRINTS_HEADER, rows["industry/blueprints.tsv"],
         [BLUEPRINTS_RES]),
        ("stations/stations.tsv", STATIONS_HEADER, rows["stations/stations.tsv"],
         [content.SYSTEMS, fb("stationoperations")]),
        ("stations/operations.tsv", OPERATIONS_HEADER, rows["stations/operations.tsv"],
         [fb("stationoperations")]),
        ("econ/prices.tsv", PRICES_HEADER, rows["econ/prices.tsv"],
         [BLUEPRINTS_RES, fb("schematics"), fb("types"), fb("groups"), fb("typematerials")]),
    ] + optional


# --- the client-backed check (real ids and counts live only here, plan §1.6) ---

TRITANIUM, RIFTER = 34, 587  # [V] C5 §0, §3.2


def check(ctx):
    content = _content()
    data = gather(ctx)
    bad = []
    fresh = derive(ctx)
    for rel, want in fresh.items():
        got = list(content.read_rows(ctx.path(rel)))
        ctx.note("%s: %d rows" % (rel, len(got)))
        if got != [content.encode_row(w).split("\t") for w in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want)))
    for rel, (_h, want_rows, _src) in sorted(optional_rows(ctx).items()):
        try:
            got = list(content.read_rows(ctx.path(rel)))
        except OSError:
            bad.append("%s: missing (re-extract the econ section)" % rel)
            continue
        if got != [content.encode_row(w).split("\t") for w in want_rows]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want_rows)))
    fsd = content._imp("fsd")  # noqa: SLF001
    ids = sorted(int(r["activityID"]) for _k, r in
                 fsd.sqlite_rows(ctx.client.path(ACTIVITIES_RES)))
    if ids != sorted(ACTIVITY_IDS.values()):
        bad.append("industry_activities.static ids %s, ACTIVITY_IDS %s"
                   % (ids, sorted(ACTIVITY_IDS.values())))
    back = blueprints_from_rows(read_blueprints(ctx.path("industry/blueprints.tsv")))
    want = {k: normalise(v) for k, v in data["blueprints"].items()}
    diff = sorted(k for k in set(want) | set(back) if want.get(k) != back.get(k))
    ctx.note("industry/blueprints.tsv: %d blueprints, %d differ from blueprints.static"
             % (len(back), len(diff)))
    if diff:
        bad.append("industry/blueprints.tsv: does not round-trip (first %d)" % diff[0])
    st = [int(f[0]) for f in content.read_rows(ctx.path("stations/stations.tsv"))]
    if st != sorted(set(st)) or len(st) != len(data["stations"]):
        bad.append("stations/stations.tsv: %d rows, %d npcStations, sorted+unique %s"
                   % (len(st), len(data["stations"]), st == sorted(set(st))))
    ops = {int(f[0]) for f in content.read_rows(ctx.path("stations/operations.tsv"))}
    loose = {s[3] for s in data["stations"]} - ops
    if loose:
        bad.append("stations/stations.tsv: operation ids not in operations.tsv: %s"
                   % sorted(loose)[:10])
    prices = {int(f[0]): f for f in content.read_rows(ctx.path("econ/prices.tsv"))}
    market = read_market(ctx.path("types/market.tsv"))
    tri = prices.get(TRITANIUM)
    if not tri or float(tri[1]) != market[TRITANIUM]["base_price"] or tri[2] != "base":
        bad.append("econ/prices.tsv: Tritanium %r, want its basePrice %r [V]"
                   % (tri, market[TRITANIUM]["base_price"]))
    rif = prices.get(RIFTER)
    if not rif or rif[2] != "recipe":
        bad.append("econ/prices.tsv: Rifter %r, want source recipe [V]" % (rif,))
    under = [t for t, f in prices.items() if float(f[1]) < float(f[3])]
    if under:
        bad.append("econ/prices.tsv: %d types under their floor (first %d)"
                   % (len(under), under[0]))
    by = {}
    for f in prices.values():
        by[f[2]] = by.get(f[2], 0) + 1
    lifted = sum(1 for f in prices.values() if float(f[1]) == float(f[3]) > 0)
    ctx.note("econ/prices.tsv: %s; %d at their floor; Rifter %s"
             % (", ".join("%s %d" % kv for kv in sorted(by.items())), lifted,
                rif[1] if rif else None))
    return bad
