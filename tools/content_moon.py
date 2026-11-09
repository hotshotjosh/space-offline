"""The `moon` content section: moon mining beacons and the Metenox's component (#52).

    moon/beacons.tsv   one row per moon whose static record carries a `miningBeacon`:
                       the beacon's position (`moonmining/miningBeacons.py`
                       `GetMiningBeaconPositionForMoon` reads
                       `mapSolarSystemContentCache.moons[moonID].miningBeacon.position`
                       **[V]**), so the server agrees with the client on where a
                       beacon is. `beacon_id` is `miningBeacon.itemID` **[I]**: the
                       field may not exist, then the column is 0 everywhere.
    moon/metenox.tsv   the `autoMoonMiner` space component of the Upwell Auto Moon Miner
                       (81826), by the names `spacecomponents/client/components/
                       autoMoonMiner.py:118-123` reads **[V]**.

The derivation is pure (plain dicts in, rows out); `gather` is the only client reader.
"""
import os
import sys

SECTION = "moon"
DEPENDS = ("universe",)
CFSD = ("spacecomponentsbytype",)

_HERE = os.path.dirname(os.path.abspath(__file__))

RES = "res:/staticdata/"
SSC = RES + "solarsystemcontent.static"
COMPONENTS = RES + "spacecomponentsbytype.fsdbinary"

BEACONS_HEADER = ("moon_id", "system_id", "x", "y", "z", "beacon_id")
METENOX_HEADER = ("type_id", "cycle_s", "reagent_type_id", "reagents_per_cycle",
                  "mining_efficiency", "fields")
TYPE_UPWELL_AUTO_MOON_MINER = 81826   # inventorycommon/const.py:1828 [V]
TYPE_COLONY_REAGENT_LAVA = 81143      # inventorycommon/const.py:1440 [V]
#: `moonmining/const.py` `MAXIMUM_MINING_BEACON_DISTANCE` [V].
MAX_BEACON_DISTANCE = 250_000.0
#: The server's `MINIMUM_MOON_DISTANCE` (4,000 km): a beacon farther than this from its
#: moon, plus radius and the 250 km, would not be in the same frame as `celestials.tsv` [I].
MOON_DISTANCE = 4_000_000.0
#: The component keys that get their own column; the rest go to `fields`.
KNOWN = ("miningCycleTime", "reagentsConsumedPerCycle", "miningEfficiency")


def _num(v):
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, int):
        return v
    return float(v or 0.0)


def _get(rec, key):
    """A field of a record that may be a dict or an object with `get`."""
    if rec is None:
        return None
    try:
        return rec.get(key)
    except AttributeError:
        return None


def derive_beacons(content, system_ids):
    """`(rows, first_keys)`. `content` is `{system_id: system record}`; rows are
    `(moon_id, system_id, x, y, z, beacon_id)` sorted by moon id; `first_keys` the
    sorted key set of the first `miningBeacon` record seen (or `()`)."""
    rows, first = [], None
    for sid in system_ids:
        system = content.get(sid)
        if system is None:
            continue
        for _pid, planet in (system.get("planets") or {}).items():
            for mid, moon in (planet.get("moons") or {}).items():
                beacon = _get(moon, "miningBeacon")
                if not beacon:
                    continue
                if first is None:
                    first = tuple(sorted(str(k) for k in beacon.keys()))
                pos = beacon["position"]
                x, y, z = _pos(pos)
                rows.append((int(mid), int(sid), x, y, z, int(beacon.get("itemID") or 0)))
    rows.sort(key=lambda r: r[0])
    return rows, first or ()


def _pos(pos):
    """A position as three floats, from a tuple/list or an x/y/z record."""
    try:
        return (float(pos[0]), float(pos[1]), float(pos[2]))
    except (TypeError, KeyError, IndexError):
        return (float(pos["x"]), float(pos["y"]), float(pos["z"]))


def derive_metenox(components):
    """`(rows, problems)` from `{type_id: {component: dict}}`."""
    comp = (components.get(TYPE_UPWELL_AUTO_MOON_MINER) or {}).get("autoMoonMiner")
    if not comp:
        return [], ["type %d has no autoMoonMiner component" % TYPE_UPWELL_AUTO_MOON_MINER]
    rest = ";".join("%s=%s" % (k, comp[k]) for k in sorted(comp) if k not in KNOWN)
    return [(TYPE_UPWELL_AUTO_MOON_MINER, _num(comp.get("miningCycleTime")),
             TYPE_COLONY_REAGENT_LAVA, _num(comp.get("reagentsConsumedPerCycle")),
             _num(comp.get("miningEfficiency")), rest)], []


def security_band(security):
    """`high` (rounds to 0.5 or more), `low` (above 0) or `null`."""
    s = round(float(security) + 1e-9, 1)
    return "high" if s >= 0.5 else ("low" if s > 0.0 else "null")


def check_beacons(rows, moons, full_run):
    """Failures for `rows` against `moons` = `{moon_id: (x, y, z, radius)}`."""
    bad = []
    for moon_id, _sid, x, y, z, _b in rows:
        m = moons.get(moon_id)
        if m is None:
            bad.append("moon/beacons.tsv: moon %d is not a moon in universe/celestials.tsv" % moon_id)
            continue
        d = ((x - m[0]) ** 2 + (y - m[1]) ** 2 + (z - m[2]) ** 2) ** 0.5
        if d > MOON_DISTANCE + m[3] + MAX_BEACON_DISTANCE:
            bad.append("moon/beacons.tsv: the beacon of moon %d is %.0f m from it (frame mismatch?)"
                       % (moon_id, d))
    if full_run and not rows:
        bad.append("moon/beacons.tsv: no beacon rows in a full run")
    return bad


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """The client's component table `{type_id: {component: dict}}` for the Metenox."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    return {t: {k: v for k, v in dict(r).items() if v is not None}
            for t, r in fsd.rows(ctx.client, "spacecomponentsbytype").items()
            if t == TYPE_UPWELL_AUTO_MOON_MINER}


def _derive(ctx):
    content = ctx.load(SSC)
    sids = _content().selected_systems(ctx, content)
    beacons, first = derive_beacons(content, sids)
    metenox, problems = derive_metenox(gather(ctx))
    return content, beacons, first, metenox, problems


def section(ctx):
    content, beacons, first, metenox, problems = _derive(ctx)
    if problems:
        raise ValueError("moon: " + "; ".join(problems))
    ctx.note("moon: %d beacons; first miningBeacon keys %s" % (len(beacons), list(first)))
    return [("moon/beacons.tsv", BEACONS_HEADER, beacons, [SSC]),
            ("moon/metenox.tsv", METENOX_HEADER, metenox, [COMPONENTS])]


def _bundle_moons(ctx):
    moons = {}
    with open(ctx.path("universe/celestials.tsv"), encoding="utf-8") as fh:
        for ln in fh:
            f = ln.rstrip("\n").split("\t")
            if f[0] == "moon":
                moons[int(f[1])] = (float(f[4]), float(f[5]), float(f[6]), float(f[3]))
    return moons


def check(ctx):
    content = _content()
    ssc, beacons, _first, metenox, problems = _derive(ctx)
    bad = list(problems)
    for rel, want in (("moon/beacons.tsv", beacons), ("moon/metenox.tsv", metenox)):
        got = list(content.read_rows(ctx.path(rel)))
        if got != [content.encode_row(r).split("\t") for r in want]:
            bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                       % (rel, len(got), len(want)))
    if not metenox:
        bad.append("moon/metenox.tsv: no Metenox row")
    bad += check_beacons(beacons, _bundle_moons(ctx), ctx.systems is None)
    bands = {}
    for _m, sid, *_r in beacons:
        band = security_band((ssc.get(sid) or {}).get("security") or 0.0)
        bands[band] = bands.get(band, 0) + 1
    ctx.note("moon: beacons by security band %s" % dict(sorted(bands.items())))
    return bad
