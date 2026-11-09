#!/usr/bin/env python3
"""killfits.py: what real pilots fit, counted from killmails.

Reads the daily killmail archives (verbatim ESI killmails from zKillboard /
EVE Ref) and counts the victims' fitted modules per hull, so the bots can
seed their fits from what players actually fly. Standard library only; run
it with ``python3 -I tools/killfits.py``.

Inputs (all CCP game data; they stay under ``out/``, never in the repo):
  --raw DIR       out/killmails/raw: killmails-YYYY-MM-DD.tar.bz2, each
                  member killmails/<id>.json. Streamed (tarfile + bz2), never
                  extracted to disk.
  --typedata F    out/content/types/typedata.tsv, the client build's type
                  universe (``type`` rows: typeID groupID categoryID ...
                  name last). A fit with a hull, module or rig that is not
                  in it is dropped and counted.

Slot flags (inventory flags): lo 11-18, med 19-26, hi 27-34, rig 92-99,
subsystem 125-132, drone bay 87. In a slot flag a category-7 (module) or
category-32 (subsystem) item is the fitted module; a category-8 item is the
charge loaded in it. Cargo and other holds are ignored.

Outputs (--out DIR, default out/bots; tab-separated, ``#`` header line):
  fits.tsv            one row per distinct fit (hull + sorted slot multiset)
                      seen at least --min-count times, plus every hull's top
                      fit: hull_type_id fit_hash count share_of_hull
                      hi med lo rig sub (comma-separated type ids, sorted,
                      repeated per module) charges (weapon:charge pairs, the
                      most common charge per weapon type) drones (the most
                      common drone-bay set, type x qty) hull_name
  fits-summary.tsv    per hull: hull_type_id kills fits_kept distinct_fits
                      top1_share top5_share empty_fits hull_name
  fit-shapes.tsv      the same fits pooled by module GROUP (meta variants of
                      one module count as one): hull_type_id shape_hash count
                      share_of_hull hi/med/lo/rig/sub group lists, the most
                      common concrete fit of the shape (fit_hash, count)
  attacker-ships.tsv  per attacker (ship, weapon) pair: ship_type_id
                      weapon_type_id appearances final_blows damage_done
                      ship_name weapon_name (types not in the bundle are
                      kept, named "?")

No character, corporation or alliance id is kept: types only.

``--check`` (REAL-FITS-SEED, #21) asks whether the server and the bots would
take every fit of ``out/bots/fits.tsv``: it runs ``eve-fitcheck`` (a Rust bin
that applies ``eve_server::fitting``'s rules and the paper engine's CPU and
power) in the build container through ``tools/dev.sh``, then summarises the
refusals (the server's rack, rig and subsystem rules): counts by reason and
by hull class, and the top fits by hull name. CPU and power are reported
apart, as fits that would not all come online: the server fits such a module
and it stays offline. The paths must be inside the repo.
No fit hash or killmail id is printed. ``--from FILE`` summarises a saved
``eve-fitcheck`` output instead of running it. The raw check lines go to
``--out/fits-check.tsv``.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import bz2
import collections
import hashlib
import json
import multiprocessing
import os
import sys
import tarfile
import time

LO = range(11, 19)
MED = range(19, 27)
HI = range(27, 35)
RIG = range(92, 100)
SUB = range(125, 133)
DRONE_BAY = 87
CAT_SHIP, CAT_MODULE, CAT_CHARGE, CAT_SUB = 6, 7, 8, 32
GROUP_CAPSULE = 29


def slot_of(flag):
    if flag in HI:
        return 0
    if flag in MED:
        return 1
    if flag in LO:
        return 2
    if flag in RIG:
        return 3
    if flag in SUB:
        return 4
    return None


def load_types(path):
    """typeID -> (groupID, categoryID, name) from typedata.tsv."""
    types = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            if not line.startswith("type\t"):
                continue
            cols = line.rstrip("\n").split("\t")
            types[int(cols[1])] = (int(cols[2]), int(cols[3]), cols[-1])
    return types


def qty(item):
    return item.get("quantity_destroyed", 0) + item.get("quantity_dropped", 0)


def fit_of(victim, types):
    """(key, charges, drones) or (None, reason). key = (hull, hi, med, lo, rig, sub)."""
    hull = victim.get("ship_type_id")
    t = types.get(hull)
    if t is None:
        return None, "unknown_hull"
    if t[1] != CAT_SHIP or t[0] == GROUP_CAPSULE:
        return None, "not_ship"
    slots = ([], [], [], [], [])
    by_flag = collections.defaultdict(list)
    drones = collections.Counter()
    for it in victim.get("items", ()):
        flag = it.get("flag")
        tid = it.get("item_type_id")
        if flag == DRONE_BAY:
            if tid in types:
                drones[tid] += qty(it)
            continue
        s = slot_of(flag)
        if s is None:
            continue
        tt = types.get(tid)
        if tt is None:
            return None, "unknown_module"
        by_flag[flag].append((tid, tt[1]))
    charges = []
    for flag, items in by_flag.items():
        mods = [tid for tid, cat in items if cat in (CAT_MODULE, CAT_SUB)]
        chs = [tid for tid, cat in items if cat == CAT_CHARGE]
        if not mods:
            continue  # a charge with no module in the slot (should not happen)
        s = slot_of(flag)
        mod = mods[0]
        slots[s].append(mod)
        if chs:
            charges.append((mod, chs[0]))
    key = (hull,) + tuple(tuple(sorted(x)) for x in slots)
    return (key, charges, tuple(sorted(drones.items()))), None


def work(path, types):
    fits = collections.Counter()
    charges = collections.defaultdict(collections.Counter)
    drones = collections.defaultdict(collections.Counter)
    kills = collections.Counter()
    dropped = collections.Counter()
    attackers = collections.Counter()  # (ship, weapon) -> [appear, final, dmg]
    finals = collections.Counter()
    damage = collections.Counter()
    n = 0
    with tarfile.open(path, "r:bz2") as tar:
        for m in tar:
            if not m.isfile() or not m.name.endswith(".json"):
                continue
            km = json.loads(tar.extractfile(m).read())
            n += 1
            for a in km.get("attackers", ()):
                k = (a.get("ship_type_id", 0), a.get("weapon_type_id", 0))
                attackers[k] += 1
                if a.get("final_blow"):
                    finals[k] += 1
                damage[k] += a.get("damage_done", 0)
            v = km.get("victim", {})
            res, why = fit_of(v, types)
            if res is None:
                dropped[why] += 1
                continue
            key, chs, dr = res
            kills[key[0]] += 1
            fits[key] += 1
            for wc in chs:
                charges[key][wc] += 1
            if dr:
                drones[key][dr] += 1
    return dict(n=n, fits=fits, charges=charges, drones=drones, kills=kills,
                dropped=dropped, attackers=attackers, finals=finals, damage=damage)


_TYPES = None


def _init(typedata):
    global _TYPES
    _TYPES = load_types(typedata)


def _work(path):
    return work(path, _TYPES)


CHECK_COLUMNS = 6  # hull count class reasons builder hull_name


def parse_check(text):
    """Rows of ``eve-fitcheck`` output: dicts with hull, count, class,
    reasons (list; empty when the fit would be taken), builder, name."""
    rows = []
    lines = text.splitlines()
    # The content loader logs to stdout before the table: start at the header.
    head = [i for i, l in enumerate(lines) if l.startswith("#hull\t")]
    for line in lines[head[0] + 1:] if head else lines:
        if not line or line.startswith("#"):
            continue
        c = line.split("\t")
        if len(c) != CHECK_COLUMNS:
            raise ValueError(f"check line with {len(c)} columns: {line!r}")
        rows.append(dict(
            hull=int(c[0]), count=int(c[1]), cls=c[2],
            reasons=[] if c[3] == "-" else c[3].split(","),
            builder=c[4], name=c[5]))
    return rows


# Findings of eve-fitcheck that are not refusals: the server fits a module
# whatever the grid (it stays offline), and no_engine is the paper engine
# declining the fit.
NOT_REFUSALS = ("offline_cpu", "offline_power", "no_engine")


def refusals(r):
    return [w for w in r["reasons"] if w not in NOT_REFUSALS]


def parse_level(text):
    """The skill level eve-fitcheck used (its ``#level N`` line), or None."""
    for line in text.splitlines():
        if line.startswith("#level "):
            return int(line.split()[1])
    return None


def summarize_check(rows, top=10, level=None):
    """The summary, as text. Refusals (what the server's fitting rules would
    refuse) by reason and by hull class, in fits and in killmails; apart from
    them the fits that would not all come online (CPU or power), the
    builder's verdicts, and the top refused fits by hull name. Nothing in it
    identifies a killmail."""
    out = []
    bad = [r for r in rows if refusals(r)]
    kills = sum(r["count"] for r in rows)
    out.append(f"fits checked {len(rows)}  killmails {kills}")
    if level is not None:
        out.append(f"skill level of the budget check: {level} (every fit-budget and "
                   "rigging skill; the default IV overstates the fits that would not "
                   "come online against flown ones, V is closer)")
    out.append(f"REFUSED by the server's fitting rules: {len(bad)} fits, "
               f"{sum(r['count'] for r in bad)} killmails")
    by_reason = collections.defaultdict(lambda: [0, 0])
    for r in bad:
        for w in refusals(r):
            by_reason[w][0] += 1
            by_reason[w][1] += r["count"]
    out.append("")
    out.append("refusals by reason (a fit with several counts under each):")
    out.append("reason\tfits\tkillmails")
    for w, (n, k) in sorted(by_reason.items(), key=lambda x: (-x[1][1], x[0])):
        out.append(f"{w}\t{n}\t{k}")
    only = collections.Counter(refusals(r)[0] for r in bad if len(refusals(r)) == 1)
    out.append("")
    out.append("refused for exactly one reason:")
    for w, n in sorted(only.items(), key=lambda x: (-x[1], x[0])):
        out.append(f"{w}\t{n}")
    by_class = collections.defaultdict(lambda: [0, 0, 0])
    for r in rows:
        e = by_class[r["cls"]]
        e[0] += 1
        if refusals(r):
            e[1] += 1
            e[2] += r["count"]
    out.append("")
    out.append("refusals by hull class:")
    out.append("class\tfits\trefused\trefused_killmails")
    for cl, (n, b, k) in sorted(by_class.items(), key=lambda x: (-x[1][2], x[0])):
        out.append(f"{cl}\t{n}\t{b}\t{k}")
    # Not refusals: the budget.
    out.append("")
    out.append("NOT refusals: fits that would not all come online (the server fits the "
               "module, it stays offline), on the paper engine at the skill level above:")
    out.append("finding\tfits\tkillmails")
    for w in NOT_REFUSALS:
        have = [r for r in rows if w in r["reasons"]]
        out.append(f"{w}\t{len(have)}\t{sum(r['count'] for r in have)}")
    verdicts = collections.Counter(r["builder"] for r in rows)
    verdict_kills = collections.Counter()
    for r in rows:
        verdict_kills[r["builder"]] += r["count"]
    out.append("")
    out.append("the doctrine builder (doctrine_real::try_build) on the same fits:")
    out.append("verdict\tfits\tkillmails")
    for w, n in sorted(verdicts.items(), key=lambda x: (-x[1], x[0])):
        out.append(f"{w}\t{n}\t{verdict_kills[w]}")
    rank = sorted(bad, key=lambda r: (-r["count"], r["name"], refusals(r)))
    out.append("")
    out.append(f"top {top} refused fits by killmail count (hull name, class, killmails, reasons):")
    for i, r in enumerate(rank[:top], 1):
        out.append(f"{i}\t{r['name']}\t{r['cls']}\t{r['count']}\t{','.join(refusals(r))}")
    hulls = collections.defaultdict(lambda: [0, 0])
    for r in bad:
        hulls[r["name"]][0] += 1
        hulls[r["name"]][1] += r["count"]
    out.append("")
    out.append(f"top {top} hulls by refused killmails (hull name, refused fits, killmails):")
    for i, (nm, (n, k)) in enumerate(
            sorted(hulls.items(), key=lambda x: (-x[1][1], x[0]))[:top], 1):
        out.append(f"{i}\t{nm}\t{n}\t{k}")
    return "\n".join(out) + "\n"


def repo_path(path, root):
    """``path`` (relative to the repo root unless absolute) as a path
    relative to ``root``; ValueError when it is outside the repo."""
    full = os.path.realpath(os.path.join(root, path))
    rel = os.path.relpath(full, os.path.realpath(root))
    if rel == ".." or rel.startswith(".." + os.sep):
        raise ValueError(f"{path}: outside the repo (the build container sees only the repo)")
    return rel


def run_check(a):
    """--check: run (or read) eve-fitcheck and print the summary."""
    if a.from_file:
        with open(a.from_file, encoding="utf-8") as f:
            text = f.read()
    else:
        import subprocess
        here = os.path.dirname(os.path.abspath(__file__))
        root = os.path.dirname(here)
        try:
            content, fits = repo_path(a.content, root), repo_path(a.fits, root)
        except ValueError as e:
            print(f"killfits: {e}", file=sys.stderr)
            return 2
        # dev.sh runs in <repo>/server, hence the "..".
        cmd = [os.path.join(here, "dev.sh"), "cargo", "run", "--locked", "-q",
               "--release", "-p", "eve-bot", "--bin", "eve-fitcheck", "--",
               "--content", os.path.join("..", content),
               "--fits", os.path.join("..", fits)]
        if a.fit_skills:
            cmd.append("--fit-skills")
        r = subprocess.run(cmd, stdout=subprocess.PIPE, text=True, check=False)
        if r.returncode != 0:
            print(f"killfits: eve-fitcheck exited {r.returncode}", file=sys.stderr)
            return r.returncode
        text = r.stdout
    rows = parse_check(text)
    os.makedirs(a.out, exist_ok=True)
    with open(os.path.join(a.out, "fits-check.tsv"), "w", encoding="utf-8") as f:
        f.write(text)
    print(summarize_check(rows, level=parse_level(text)), end="")
    return 0


def ids(xs):
    return ",".join(str(x) for x in xs)


def fit_hash(key):
    return hashlib.sha1(repr(key).encode()).hexdigest()[:12]


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--raw", default="out/killmails/raw")
    ap.add_argument("--typedata", default="out/content/types/typedata.tsv")
    ap.add_argument("--out", default="out/bots", help="output directory (resolved against the cwd)")
    ap.add_argument("--min-count", type=int, default=2)
    ap.add_argument("--jobs", type=int, default=4)
    ap.add_argument("--days", type=int, default=0, help="only the first N archives (0 = all)")
    ap.add_argument("--check", action="store_true",
                    help="check every fit of --fits against the fitting rules (see above)")
    ap.add_argument("--fits", default="out/bots/fits.tsv",
                    help="--check: the fits table (resolved against the repo root, inside it)")
    ap.add_argument("--content", default="out/content",
                    help="--check: the content bundle (resolved against the repo root, inside it)")
    ap.add_argument("--fit-skills", action="store_true",
                    help="--check: every fit-budget and rigging skill at V, not IV")
    ap.add_argument("--from", dest="from_file", default=None,
                    help="--check: summarise this saved eve-fitcheck output "
                         "(resolved against the cwd, like --out)")
    a = ap.parse_args(argv)
    if a.check:
        return run_check(a)

    t0 = time.time()
    paths = sorted(os.path.join(a.raw, f) for f in os.listdir(a.raw)
                   if f.startswith("killmails-") and f.endswith(".tar.bz2"))
    if a.days:
        paths = paths[:a.days]
    types = load_types(a.typedata)

    total = dict(n=0, fits=collections.Counter(),
                 charges=collections.defaultdict(collections.Counter),
                 drones=collections.defaultdict(collections.Counter),
                 kills=collections.Counter(), dropped=collections.Counter(),
                 attackers=collections.Counter(), finals=collections.Counter(),
                 damage=collections.Counter())
    with multiprocessing.Pool(a.jobs, _init, (a.typedata,)) as pool:
        for i, r in enumerate(pool.imap_unordered(_work, paths), 1):
            total["n"] += r["n"]
            for k in ("fits", "kills", "dropped", "attackers", "finals", "damage"):
                total[k].update(r[k])
            for k in ("charges", "drones"):
                for fk, c in r[k].items():
                    total[k][fk].update(c)
            print(f"[{i}/{len(paths)}] {total['n']} killmails, {time.time()-t0:.0f}s",
                  file=sys.stderr)

    name = lambda t: types[t][2] if t in types else "?"
    os.makedirs(a.out, exist_ok=True)
    per_hull = collections.defaultdict(list)
    for key, c in total["fits"].items():
        per_hull[key[0]].append((c, key))
    rows = 0
    with open(os.path.join(a.out, "fits.tsv"), "w", encoding="utf-8") as f, \
         open(os.path.join(a.out, "fits-summary.tsv"), "w", encoding="utf-8") as s:
        f.write("#hull_type_id\tfit_hash\tcount\tshare_of_hull\thi\tmed\tlo\trig\tsub"
                "\tcharges\tdrones\thull_name\n")
        s.write("#hull_type_id\tkills\tfits_kept\tdistinct_fits\ttop1_share\ttop5_share"
                "\tempty_fits\thull_name\n")
        for hull in sorted(per_hull, key=lambda h: -total["kills"][h]):
            fl = sorted(per_hull[hull], key=lambda x: (-x[0], x[1]))
            kills = total["kills"][hull]
            empty = sum(c for c, k in fl if not any(k[1:]))
            kept = 0
            for i, (c, key) in enumerate(fl):
                if c < a.min_count and i > 0:
                    break
                kept += 1
                best = {}
                for (w, ch), n in sorted(total["charges"][key].items(),
                                         key=lambda x: (-x[1], x[0])):
                    best.setdefault(w, ch)
                dr = total["drones"][key].most_common(1)
                drs = ",".join(f"{t}x{q}" for t, q in dr[0][0]) if dr else ""
                f.write(f"{hull}\t{fit_hash(key)}\t{c}\t{c/kills:.4f}\t"
                        + "\t".join(ids(x) for x in key[1:])
                        + f"\t{','.join(f'{w}:{ch}' for w, ch in sorted(best.items()))}"
                        + f"\t{drs}\t{name(hull)}\n")
                rows += 1
            top1 = fl[0][0] / kills
            top5 = sum(c for c, _ in fl[:5]) / kills
            s.write(f"{hull}\t{kills}\t{kept}\t{len(fl)}\t{top1:.4f}\t{top5:.4f}"
                    f"\t{empty}\t{name(hull)}\n")
    # Shapes: the fit with every module replaced by its group (a Compact and
    # an Enduring of one module count as one), so meta variants pool.
    shapes = collections.Counter()
    shape_top = {}
    for key, c in total["fits"].items():
        sk = (key[0],) + tuple(tuple(sorted(types[t][0] for t in x)) for x in key[1:])
        shapes[sk] += c
        if c > shape_top.get(sk, (0, None))[0]:
            shape_top[sk] = (c, key)
    srows = 0
    with open(os.path.join(a.out, "fit-shapes.tsv"), "w", encoding="utf-8") as f:
        f.write("#hull_type_id\tshape_hash\tcount\tshare_of_hull\thi_groups\tmed_groups"
                "\tlo_groups\trig_groups\tsub_groups\ttop_fit_hash\ttop_fit_count\thull_name\n")
        for sk, c in sorted(shapes.items(),
                            key=lambda x: (-total["kills"][x[0][0]], x[0][0], -x[1], x[0])):
            if c < a.min_count:
                continue
            tc, tk = shape_top[sk]
            f.write(f"{sk[0]}\t{fit_hash(sk)}\t{c}\t{c/total['kills'][sk[0]]:.4f}\t"
                    + "\t".join(ids(x) for x in sk[1:])
                    + f"\t{fit_hash(tk)}\t{tc}\t{name(sk[0])}\n")
            srows += 1
    print(f"fit-shapes.tsv rows {srows} (distinct shapes {len(shapes)})")
    with open(os.path.join(a.out, "attacker-ships.tsv"), "w", encoding="utf-8") as f:
        f.write("#ship_type_id\tweapon_type_id\tappearances\tfinal_blows\tdamage_done"
                "\tship_name\tweapon_name\n")
        for (sh, w), c in total["attackers"].most_common():
            f.write(f"{sh}\t{w}\t{c}\t{total['finals'][(sh, w)]}"
                    f"\t{total['damage'][(sh, w)]}\t{name(sh)}\t{name(w)}\n")
    fitted = sum(total["kills"].values())
    print(f"killmails {total['n']}  fits counted {fitted}  dropped "
          f"{dict(total['dropped'])}  hulls {len(per_hull)}  distinct fits "
          f"{len(total['fits'])}  fits.tsv rows {rows}  attacker pairs "
          f"{len(total['attackers'])}  {time.time()-t0:.0f}s")


if __name__ == "__main__":
    sys.exit(main() or 0)
