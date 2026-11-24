"""Unit tests for the `econ` content section, `tools/content_econ.py` (CP24).

Synthetic rows only (plan §1.6): invented ids in `schema.md`'s unused
ranges, and no [A] value asserted (the tests name `SALVAGE_TIER_BASE` and
read the multipliers from the fixture, never the shipped file). The
client-backed checks are `content.py --sections econ --check`, recorded in
`docs/lanes/cp24.md`. Run: `python3 -I tools/test_content_econ.py -q`.
"""

from __future__ import annotations

import json
import os
import sqlite3
import sys
import tempfile
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content  # noqa: E402
import content_econ as E  # noqa: E402

# Invented ids (`schema.md` §1.1).
MIN_A, MIN_B, ORE, FRIG, FRIG_BP, MOD, MOD_BP, VARIANT = (
    990030, 990031, 990010, 990001, 990101, 990002, 990102, 990003)
GOO, GOO_BP, P1, P2, UNPRICED, SALVAGE, RIG_T1, RIG_T2 = (
    990040, 990103, 990050, 990051, 990060, 990070, 990080, 990081)
MG = 99501  # a market group
META_FAKE = 99


def _market():
    def row(base=0.0, meta=None, parent=None, portion=1, group=99001, mg=MG):
        return {"group_id": group, "category_id": 7, "market_group_id": mg,
                "base_price": base, "meta_group_id": meta, "parent": parent,
                "portion_size": portion}
    return {
        MIN_A: row(base=2.0), MIN_B: row(base=8.0), ORE: row(base=1.0, portion=100),
        FRIG: row(base=1.0, meta=1), FRIG_BP: row(base=500.0), MOD: row(meta=1),
        VARIANT: row(meta=META_FAKE, parent=MOD), GOO: row(), P1: row(base=5.0),
        P2: row(), UNPRICED: row(mg=None), SALVAGE: row(group=E.GROUP_SALVAGE),
        RIG_T1: row(meta=1), RIG_T2: row(meta=E.META_T2),
    }


def _bp(time, runs, **acts):
    return {"blueprintTypeID": 0, "maxProductionLimit": runs,
            "activities": {k: dict(v, time=time) for k, v in acts.items()}}


def _blueprints():
    frig = _bp(600, 30,
               manufacturing={"materials": [{"typeID": MIN_A, "quantity": 100},
                                            {"typeID": MIN_B, "quantity": 10}],
                              "products": [{"typeID": FRIG, "quantity": 1}],
                              "skills": [{"typeID": 990900, "level": 1}]},
               copying={},
               invention={"materials": [{"typeID": MIN_A, "quantity": 1}],
                          "products": [{"typeID": MOD_BP, "quantity": 10,
                                        "probability": 0.25}]})
    frig["blueprintTypeID"] = FRIG_BP
    mod = _bp(60, 10, manufacturing={
        "materials": [{"typeID": MIN_A, "quantity": 5}, {"typeID": UNPRICED, "quantity": 1}],
        "products": [{"typeID": MOD, "quantity": 1}]})
    mod["blueprintTypeID"] = MOD_BP
    goo = _bp(3600, 1, reaction={"materials": [{"typeID": MIN_B, "quantity": 30}],
                                 "products": [{"typeID": GOO, "quantity": 3}]})
    goo["blueprintTypeID"] = GOO_BP
    rigs = {}
    for bp, rig in ((990104, RIG_T1), (990105, RIG_T2)):
        rigs[bp] = _bp(60, 10, manufacturing={
            "materials": [{"typeID": SALVAGE, "quantity": 2}],
            "products": [{"typeID": rig, "quantity": 1}]})
        rigs[bp]["blueprintTypeID"] = bp
    return {FRIG_BP: frig, MOD_BP: mod, GOO_BP: goo, **rigs}


MATERIALS = {ORE: [(MIN_A, 400)], FRIG: [(MIN_A, 100), (MIN_B, 10)],
             MOD: [(MIN_B, 50)]}
PI = {P2: ([(P1, 40)], 5)}
META = {1: 1.0, META_FAKE: 3.0}


def _oracle(overrides=None, salvage=True, markup=1.0):
    m = _market()
    rmap = E.recipes(E.blueprint_rows(_blueprints()))
    rule = E.salvage_prices(m, rmap) if salvage else {}
    return E.Oracle(m, rmap, PI, MATERIALS, META, overrides, rule, markup=markup)


class Headers(unittest.TestCase):
    def test_headers_are_schema_md_lines(self):
        # Copied from `docs/content/schema.md` §2 and §4, byte for byte.
        want = {
            "blueprints": "#bp_type_id\tactivity_id\tkind\ttype_id\tquantity\t"
                          "probability\ttime\tmax_runs",
            "stations": "#station_id\tsystem_id\tregion_id\towner_id\toperation_id\t"
                        "services\treproc_eff\treproc_take",
            "operations": "#operation_id\tactivity_id\tservices\tstation_types\t"
                          "manufacturing_factor\tresearch_factor",
            "prices": "#type_id\tref_price\tsource\tfloor_price\tcategory_id\tgroup_id\t"
                      "meta_group_id\tportion_size\tbuild_min",
            "modifier_sources": "#type_id\tactivity_id\tkind\tattribute_id\tfilter_id",
            "target_filters": "#filter_id\tcategory_ids\tgroup_ids",
            "overrides": "#type_id\tprice\twhy",
            "meta": "#meta_group_id\tmult\twhy",
            "markup": "#markup\twhy",
            "pimarkup": "#markup\twhy",
        }
        got = {"blueprints": E.BLUEPRINTS_HEADER, "stations": E.STATIONS_HEADER,
               "operations": E.OPERATIONS_HEADER, "prices": E.PRICES_HEADER,
               "modifier_sources": E.MODIFIER_SOURCES_HEADER,
               "target_filters": E.TARGET_FILTERS_HEADER,
               "overrides": E.OVERRIDES_HEADER, "meta": E.META_MULT_HEADER,
               "markup": E.RECIPE_MARKUP_HEADER, "pimarkup": E.PI_MARKUP_HEADER}
        for k, line in want.items():
            self.assertEqual("#" + "\t".join(got[k]), line, k)

    def test_the_section_is_discovered_after_types2(self):
        sections = content.discover()
        self.assertIn(E.SECTION, [s.name for s in sections])
        names = [s.name for s in content.order(sections, [E.SECTION])]
        self.assertLess(names.index("types2"), names.index(E.SECTION))

    def test_the_shipped_authored_files_parse(self):
        # Well-formedness only (plan §1.4): no value in them is asserted.
        for path, parse in ((E.META_MULT, E.parse_meta_mult),
                            (E.OVERRIDES, E.parse_overrides)):
            with open(path, encoding="utf-8") as fh:
                _rows, problems = parse(fh)
            self.assertEqual(problems, [], path)
        with open(E.RECIPE_MARKUP_FILE, encoding="utf-8") as fh:
            self.assertIsNone(E.markup_problem(E.parse_recipe_markup(fh)))
        with open(E.PI_MARKUP_FILE, encoding="utf-8") as fh:
            self.assertIsNone(E.markup_problem(
                E.parse_pi_markup(fh), E.MIN_PI_MARKUP, E.MAX_PI_MARKUP))


class Oracle(unittest.TestCase):
    def test_a_leaf_is_its_base_price(self):
        o = _oracle()
        self.assertEqual(o.price(MIN_A)[:2], (2.0, "base"))
        self.assertEqual(o.price(MIN_B)[:2], (8.0, "base"))

    def test_a_manufactured_type_is_the_recursion(self):
        o = _oracle()
        want = (100 * o.price(MIN_A)[0] + 10 * o.price(MIN_B)[0]) / 1
        self.assertEqual(o.price(FRIG)[:2], (want, "recipe"))
        self.assertNotEqual(want, _market()[FRIG]["base_price"])
        goo = o.price(GOO)
        self.assertEqual(goo[:2], (30 * o.price(MIN_B)[0] / 3, "reaction"))

    def test_pi_divides_by_the_output_quantity(self):
        o = _oracle()
        self.assertEqual(o.price(P2)[:2], (40 * o.price(P1)[0] / 5, "pi"))

    def test_an_unpriced_material_falls_through_to_the_parent_rule(self):
        o = _oracle()
        self.assertIsNone(o.price(UNPRICED))
        # MOD's recipe needs UNPRICED and it has no basePrice: only its floor
        # is known, and nothing prices it. VARIANT's parent is MOD.
        self.assertIsNone(o.price(MOD))
        self.assertIsNone(o.price(VARIANT))
        o = _oracle(overrides={MOD: 1000.0})
        self.assertEqual(o.price(VARIANT)[:2], (1000.0 * META[META_FAKE], "parent"))

    def test_every_priced_type_is_at_least_its_floor(self):
        o = _oracle(overrides={MOD: 1.0})
        rows = E.price_rows(_market(), o)
        self.assertTrue(rows)
        for r in rows:
            self.assertGreaterEqual(r[1], r[3], r)
        ore = o.price(ORE)
        self.assertEqual(ore[2], 400 * o.price(MIN_A)[0] / 100)
        self.assertEqual(ore[0], ore[2], "the floor lifts a base price under it")
        mod = o.price(MOD)
        self.assertEqual(mod, (50 * o.price(MIN_B)[0], "override", 50 * o.price(MIN_B)[0]))

    def test_an_override_replaces_the_derived_price(self):
        o = _oracle(overrides={FRIG: 123456.0})
        self.assertEqual(o.price(FRIG)[:2], (123456.0, "override"))
        self.assertEqual(_oracle(overrides={P2: 7.5e6}).price(P2)[:2], (7.5e6, "override"))

    def test_salvage_is_priced_by_tier_where_nothing_else_prices_it(self):
        o = _oracle()
        self.assertEqual(o.price(SALVAGE)[:2], (E.SALVAGE_TIER_BASE * 3, "override"))
        self.assertEqual(o.price(RIG_T1)[:2], (2 * o.price(SALVAGE)[0], "recipe"))
        self.assertIsNone(_oracle(salvage=False).price(RIG_T1))
        self.assertEqual(_oracle(overrides={SALVAGE: 5.0}).price(SALVAGE)[0], 5.0)

    def test_a_cycle_and_the_depth_cap_leave_a_type_unpriced(self):
        m = {990500 + i: {"base_price": 0.0, "portion_size": 1} for i in range(40)}
        cyc = {990500: [(1, 1, [(990501, 1)], 1)], 990501: [(1, 1, [(990500, 1)], 1)]}
        self.assertIsNone(E.Oracle(m, cyc, {}, {}, {}).price(990500))
        m[990539]["base_price"] = 1.0
        chain = {990500 + i: [(1, 1, [(990501 + i, 1)], 1)] for i in range(39)}
        o = E.Oracle(m, chain, {}, {}, {})
        self.assertIsNone(o.price(990500), "deeper than DEPTH_CAP")
        self.assertEqual(o.price(990539 - E.DEPTH_CAP)[0], 1.0)
        self.assertEqual(E.Oracle(m, chain, {}, {}, {}).price(990539 - E.DEPTH_CAP)[0], 1.0,
                         "the answer does not depend on what was asked first")

    def test_price_rows_are_market_types_only(self):
        rows = E.price_rows(_market(), _oracle())
        self.assertNotIn(UNPRICED, [r[0] for r in rows])
        self.assertEqual([r[0] for r in rows], sorted(r[0] for r in rows))
        self.assertEqual(len(rows[0]), len(E.PRICES_HEADER))


class Markup(unittest.TestCase):
    M = 1.25  # an invented test markup, not the shipped default

    def test_one_tier_is_the_markup_times_its_materials(self):
        o = _oracle(markup=self.M)
        cost = 100 * o.price(MIN_A)[0] + 10 * o.price(MIN_B)[0]
        self.assertAlmostEqual(o.price(FRIG)[0], self.M * cost)
        self.assertEqual(o.price(FRIG)[1], "recipe")
        goo = 30 * o.price(MIN_B)[0] / 3
        self.assertAlmostEqual(o.price(GOO)[0], self.M * goo)
        self.assertEqual(o.price(GOO)[1], "reaction")

    def test_a_markup_of_one_reproduces_the_unmarked_prices(self):
        a, b = _oracle(), _oracle(markup=1.0)
        for t in _market():
            self.assertEqual(a.price(t), b.price(t), t)

    def test_tiers_compound_their_own_margin(self):
        m = {MIN_A: {"base_price": 2.0, "portion_size": 1},
             990201: {"base_price": 0.0, "portion_size": 1},
             990202: {"base_price": 0.0, "portion_size": 1}}
        bps = {}
        for bp, prod, mat, q in ((990211, 990201, MIN_A, 10), (990212, 990202, 990201, 3)):
            b = _bp(60, 1, manufacturing={"materials": [{"typeID": mat, "quantity": q}],
                                          "products": [{"typeID": prod, "quantity": 1}]})
            b["blueprintTypeID"] = bp
            bps[bp] = b
        rmap = E.recipes(E.blueprint_rows(bps))
        o = E.Oracle(m, rmap, {}, {}, {}, markup=self.M)
        self.assertAlmostEqual(o.price(990201)[0], self.M * 10 * 2.0)
        self.assertAlmostEqual(o.price(990202)[0], self.M ** 2 * 3 * 10 * 2.0)

    def test_overrides_and_the_recipe_markup_leave_pi_unmarked(self):
        o = _oracle(overrides={FRIG: 123456.0}, markup=self.M)
        self.assertEqual(o.price(FRIG)[:2], (123456.0, "override"))
        self.assertEqual(o.price(P2)[:2], (40 * o.price(P1)[0] / 5, "pi"))
        self.assertEqual(o.price(MIN_A)[:2], (2.0, "base"))
        # A floor over unmarked minerals is unmarked, and still lifts a price.
        self.assertEqual(o.price(ORE)[2], 400 * o.price(MIN_A)[0] / 100)
        for r in E.price_rows(_market(), o):
            self.assertGreaterEqual(r[1], r[3], r)

    def test_a_parent_price_inherits_the_markup_of_the_recipe_it_is_built_on(self):
        child = 990090
        m = _market()
        m[child] = dict(m[VARIANT], parent=FRIG)
        rmap = E.recipes(E.blueprint_rows(_blueprints()))
        a = E.Oracle(m, rmap, PI, MATERIALS, META, markup=1.0).price(child)
        b = E.Oracle(m, rmap, PI, MATERIALS, META, markup=self.M).price(child)
        self.assertEqual((a[1], b[1]), ("parent", "parent"))
        self.assertAlmostEqual(b[0], self.M * a[0])

    def test_the_floor_of_a_type_that_reprocesses_into_a_built_item_rises(self):
        m = _market()
        rmap = E.recipes(E.blueprint_rows(_blueprints()))
        mats = {ORE: [(FRIG, 1)]}
        lo = E.Oracle(m, rmap, PI, mats, META, markup=1.0).price(ORE)
        hi = E.Oracle(m, rmap, PI, mats, META, markup=self.M).price(ORE)
        self.assertGreater(hi[2], lo[2])

    def test_a_markup_outside_the_range_is_a_problem(self):
        self.assertIsNone(E.markup_problem(1.0))
        self.assertIsNone(E.markup_problem(E.MAX_RECIPE_MARKUP))
        for bad in (0.99, float("nan"), float("inf"), E.MAX_RECIPE_MARKUP + 0.01):
            self.assertIsNotNone(E.markup_problem(bad), bad)

    def test_the_econ_derive_reads_the_authored_file(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "recipe-markup.tsv")
            seen = []

            class C:
                def input(self, p):
                    seen.append(p)
                    return p

            def write(text):
                with open(path, "w", encoding="utf-8") as fh:
                    fh.write(text)

            old = E.RECIPE_MARKUP_FILE
            E.RECIPE_MARKUP_FILE = path
            try:
                write("# c\nmarkup\twhy\n1.5\t[A] x\n")
                self.assertEqual(E.recipe_markup(C()), 1.5)
                self.assertEqual(seen, [path])  # recorded as a manifest input
                for bad in ("markup\twhy\n", "markup\twhy\n1.2\tx\n1.3\ty\n",
                            "markup\twhy\nabc\tx\n", "markup\twhy\nnan\tx\n",
                            "markup\twhy\ninf\tx\n", "markup\twhy\n0.5\tx\n",
                            "markup\twhy\n%r\tx\n" % (E.MAX_RECIPE_MARKUP + 1.0),
                            "markup\n1.2\n"):
                    write(bad)
                    with self.assertRaises(content.ContentError, msg=bad):
                        E.recipe_markup(C())
                os.remove(path)
                with self.assertRaises(content.ContentError):
                    E.recipe_markup(C())
            finally:
                E.RECIPE_MARKUP_FILE = old


class PiMarkup(unittest.TestCase):
    M = 1.25  # an invented test markup, not the shipped default
    T3, T4 = 990401, 990402  # invented PI products built over P2

    def _pi(self, pi_markup, markup=1.0):
        pi = dict(PI)
        pi[self.T3] = ([(P2, 3)], 2)
        pi[self.T4] = ([(self.T3, 2), (P1, 4)], 1)
        m = _market()
        for t in (self.T3, self.T4):
            m[t] = {"base_price": 0.0, "portion_size": 1}
        rmap = E.recipes(E.blueprint_rows(_blueprints()))
        return E.Oracle(m, rmap, pi, MATERIALS, META, None,
                        E.salvage_prices(m, rmap), markup=markup,
                        pi_markup=pi_markup)

    def test_one_tier_is_the_markup_times_its_inputs(self):
        o = self._pi(self.M)
        self.assertAlmostEqual(o.price(P2)[0], self.M * 40 * o.price(P1)[0] / 5)
        self.assertEqual(o.price(P2)[1], "pi")

    def test_tiers_compound_through_their_marked_inputs(self):
        o, u = self._pi(self.M), self._pi(1.0)
        p1 = o.price(P1)[0]
        self.assertEqual(p1, u.price(P1)[0])  # P1 is not a pi output
        p2 = self.M * 40 * p1 / 5
        p3 = self.M * 3 * p2 / 2
        p4 = self.M * (2 * p3 + 4 * p1)
        self.assertAlmostEqual(o.price(self.T3)[0], p3)
        self.assertAlmostEqual(o.price(self.T4)[0], p4)
        self.assertAlmostEqual(o.price(self.T3)[0], self.M ** 2 * u.price(self.T3)[0])

    def test_a_markup_of_one_reproduces_the_unmarked_prices(self):
        a, b = _oracle(), self._pi(1.0)
        for t in _market():
            self.assertEqual(a.price(t), b.price(t), t)

    def test_it_is_independent_of_the_recipe_markup(self):
        a, b = self._pi(self.M), self._pi(self.M, markup=1.5)
        self.assertEqual(a.price(self.T4), b.price(self.T4))
        self.assertEqual(b.price(FRIG)[1], "recipe")
        self.assertAlmostEqual(b.price(FRIG)[0], a.price(FRIG)[0] * 1.5)

    def test_a_pi_override_is_unmarked(self):
        o = self._pi(self.M)
        o.overrides = {P2: 77.0}
        self.assertEqual(o.price(P2)[:2], (77.0, "override"))

    def test_a_markup_outside_the_range_is_a_problem(self):
        lo, hi = E.MIN_PI_MARKUP, E.MAX_PI_MARKUP
        self.assertIsNone(E.markup_problem(lo, lo, hi))
        self.assertIsNone(E.markup_problem(hi, lo, hi))
        for bad in (lo - 0.01, float("nan"), float("inf"), hi + 0.01):
            self.assertIsNotNone(E.markup_problem(bad, lo, hi), bad)

    def test_the_econ_derive_reads_the_authored_file(self):
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "pi-markup.tsv")
            seen = []

            class C:
                def input(self, p):
                    seen.append(p)
                    return p

            def write(text):
                with open(path, "w", encoding="utf-8") as fh:
                    fh.write(text)

            old = E.PI_MARKUP_FILE
            E.PI_MARKUP_FILE = path
            try:
                write("# c\nmarkup\twhy\n1.5\t[A] x\n")
                self.assertEqual(E.pi_markup(C()), 1.5)
                self.assertEqual(seen, [path])  # recorded as a manifest input
                for bad in ("markup\twhy\n", "markup\twhy\n1.2\tx\n1.3\ty\n",
                            "markup\twhy\nabc\tx\n", "markup\twhy\nnan\tx\n",
                            "markup\twhy\ninf\tx\n", "markup\twhy\n0.5\tx\n",
                            "markup\twhy\n%r\tx\n" % (E.MAX_PI_MARKUP + 1.0),
                            "markup\n1.2\n"):
                    write(bad)
                    with self.assertRaises(content.ContentError, msg=bad):
                        E.pi_markup(C())
                os.remove(path)
                with self.assertRaises(content.ContentError):
                    E.pi_markup(C())
            finally:
                E.PI_MARKUP_FILE = old


class PiAnchor(unittest.TestCase):
    S = 0.8  # an invented scale, not the shipped one
    P0, A1, A1B, A2, A3 = 990500, 990501, 990502, 990503, 990504
    TAX = {990501: 3.0, 990502: 3.0, 990503: 250.0, 990504: 4000.0}  # invented

    def _o(self, scale, tax=None, overrides=None, pi_markup=1.25):
        pi = dict(PI)
        pi[self.A1] = ([(self.P0, 10)], 20)
        pi[self.A1B] = ([(self.P0, 10)], 20)
        pi[self.A2] = ([(self.A1, 40), (self.A1B, 40)], 5)
        pi[self.A3] = ([(self.A2, 2)], 1)
        m = _market()
        for t in (self.P0, self.A1, self.A1B, self.A2, self.A3):
            m[t] = {"base_price": 1.0 if t == self.A1 else 0.0, "portion_size": 1,
                    "market_group_id": MG}
        m[self.P0]["base_price"] = 0.0
        rmap = E.recipes(E.blueprint_rows(_blueprints()))
        return E.Oracle(m, rmap, pi, MATERIALS, META, overrides,
                        E.salvage_prices(m, rmap), pi_markup=pi_markup,
                        pi_anchor=scale, tax_mult=self.TAX if tax is None else tax)

    def test_a_tier_one_product_prices_at_scale_times_its_tax_multiplier(self):
        o = self._o(self.S)
        for t in (self.A1, self.A1B):
            self.assertAlmostEqual(o.price(t)[0], self.S * 3.0)
            self.assertEqual(o.price(t)[1], "pi")

    def test_the_tiers_above_compound_through_the_markup(self):
        o, m = self._o(self.S), 1.25
        p1 = self.S * 3.0
        p2 = m * (40 * p1 + 40 * p1) / 5
        self.assertAlmostEqual(o.price(self.A2)[0], p2)
        self.assertAlmostEqual(o.price(self.A3)[0], m * 2 * p2)

    def test_raw_p0_stays_unpriced(self):
        self.assertIsNone(self._o(self.S).price(self.P0))

    def test_an_override_still_wins(self):
        o = self._o(self.S, overrides={self.A1: 77.0})
        self.assertEqual(o.price(self.A1)[:2], (77.0, "override"))

    def test_a_product_without_a_tax_row_is_not_anchored(self):
        o = self._o(self.S, tax={})
        self.assertEqual(o.price(self.A1)[:2], (1.0, "base"))

    def test_a_non_pi_type_is_untouched(self):
        a, b = self._o(self.S), self._o(None)
        for t in (P1, MIN_A, FRIG):
            self.assertEqual(a.price(t), b.price(t), t)

    def test_none_reproduces_todays_output(self):
        a, b = self._o(None), self._o(None, tax={})
        for t in _market():
            self.assertEqual(a.price(t), b.price(t), t)
        self.assertEqual(a.price(self.A1)[:2], (1.0, "base"))
        c = _oracle()
        d = E.Oracle(_market(), E.recipes(E.blueprint_rows(_blueprints())), PI, MATERIALS,
                     META, None, E.salvage_prices(_market(), E.recipes(
                         E.blueprint_rows(_blueprints()))), pi_anchor=None,
                     tax_mult={P1: 9.0, P2: 9.0})
        for t in _market():
            self.assertEqual(c.price(t), d.price(t), t)

    def test_read_tax_mult_takes_only_taxmult_rows(self):
        lines = ["# c\n", "pintype\t990501\t1\n", "taxmult\t990501\t3.0\n",
                 "taxmult\t990502\tnan\n", "taxmult\t990503\t-1\n",
                 "taxmult\tx\t1\n", "taxmult\t990504\t4000\n"]
        self.assertEqual(E.read_tax_mult(lines), {990501: 3.0, 990504: 4000.0})

    def test_the_scale_file_is_one_row_in_range(self):
        import io
        ok = "scale\twhy\n0.8\t[A] x\n"
        self.assertEqual(E.parse_pi_anchor(io.StringIO(ok)), 0.8)
        for bad in ("scale\twhy\n", "scale\twhy\n1\tx\n1\ty\n", "scale\twhy\nabc\tx\n",
                    "scale\twhy\nnan\tx\n", "scale\twhy\n0\tx\n",
                    "scale\twhy\n%r\tx\n" % (E.MAX_PI_ANCHOR + 1.0), "scale\n1\n"):
            with self.assertRaises(content.ContentError, msg=bad):
                E.parse_pi_anchor(io.StringIO(bad))

    def test_the_shipped_scale_file_parses(self):
        with open(E.PI_ANCHOR_FILE, encoding="utf-8") as fh:
            self.assertIsNone(E.markup_problem(
                E.parse_pi_anchor(fh), E.MIN_PI_ANCHOR, E.MAX_PI_ANCHOR))

    def test_the_econ_section_depends_on_planets(self):
        self.assertIn("planets", E.DEPENDS)


class BuildMin(unittest.TestCase):
    M = 1.25  # invented

    def _chain(self, markup, reaction=False, overrides=None, extra=(), qty1=1):
        """MIN_A (base 2) -> T1 (10 per unit) -> T2 (3 T1 per unit).
        `extra` adds `(blueprint, product, material, qty)` manufacturing
        blueprints; `qty1` is the productQuantity of T1's first blueprint."""
        t1, t2 = 990201, 990202
        m = {MIN_A: {"base_price": 2.0, "portion_size": 1},
             t1: {"base_price": 0.0, "portion_size": 1},
             t2: {"base_price": 0.0, "portion_size": 1}}
        bps = {}
        specs = [(990211, t1, MIN_A, 10, "reaction" if reaction else "manufacturing"),
                 (990212, t2, t1, 3, "manufacturing")]
        specs += [(bp, prod, mat, q, "manufacturing") for bp, prod, mat, q in extra]
        for bp, prod, mat, q, act in specs:
            b = _bp(60, 1, **{act: {"materials": [{"typeID": mat, "quantity": q}],
                                    "products": [{"typeID": prod,
                                                  "quantity": qty1 if bp == 990211 else 1}]}})
            b["blueprintTypeID"] = bp
            bps[bp] = b
        rmap = E.recipes(E.blueprint_rows(bps))
        return E.Oracle(m, rmap, {}, {}, {}, overrides, markup=markup), t1, t2

    def test_a_leaf_has_none(self):
        o = _oracle(markup=self.M)
        for t in (MIN_A, P2, ORE):
            self.assertIsNone(o.build_min(t), t)

    def test_one_tier_is_the_me10_materials_without_markup(self):
        o, t1, _ = self._chain(self.M)
        self.assertAlmostEqual(o.build_min(t1), E.ME10_FACTOR * 10 * 2.0)
        self.assertAlmostEqual(o.price(t1)[0], self.M * 10 * 2.0)

    def test_a_reaction_has_no_me_factor(self):
        o, t1, _ = self._chain(self.M, reaction=True)
        self.assertAlmostEqual(o.build_min(t1), E.REACTION_FACTOR * 10 * 2.0)

    def test_tiers_chain_through_build_min_not_through_the_marked_price(self):
        o, t1, t2 = self._chain(self.M)
        self.assertAlmostEqual(
            o.build_min(t2), E.ME10_FACTOR * 3 * o.build_min(t1))
        # the marked price compounds; the build cost does not
        self.assertGreater(o.price(t2)[0], o.build_min(t2) * self.M ** 2)

    def test_a_markup_over_the_build_discount_prices_above_the_self_build_cost(self):
        # why `full = FULL_SHARE x P_ref` needs the cap: at ME 10 over two tiers
        # the build cost is ME10_FACTOR ** 2 of the markup-free price.
        o, _, t2 = self._chain(1.0)
        self.assertAlmostEqual(o.build_min(t2), E.ME10_FACTOR ** 2 * o.price(t2)[0])

    def test_an_override_priced_hull_with_a_recipe_has_a_build_min(self):
        o = _oracle(overrides={FRIG: 123456.0}, markup=self.M)
        self.assertEqual(o.price(FRIG)[1], "override")
        self.assertAlmostEqual(o.build_min(FRIG), E.ME10_FACTOR * (100 * 2.0 + 10 * 8.0))

    def test_an_unpriced_recipe_has_none(self):
        self.assertIsNone(_oracle().build_min(UNPRICED))

    def test_an_override_above_the_build_cost_leaves_the_build_cost(self):
        o, t1, t2 = self._chain(self.M, overrides={990201: 1000.0})
        self.assertGreater(1000.0, o.build_min(t1))
        self.assertEqual(o.price(t1)[1], "override")
        self.assertAlmostEqual(o.build_min(t1), E.ME10_FACTOR * 10 * 2.0)
        # the hull is built from the built material, not the dear override
        self.assertAlmostEqual(o.build_min(t2), E.ME10_FACTOR * 3 * E.ME10_FACTOR * 10 * 2.0)

    def test_a_material_priced_below_its_build_cost_is_bought(self):
        # unit(m) = P_ref(m) because it is below build_min(m): exact value
        o, t1, t2 = self._chain(self.M, overrides={990201: 5.0})
        self.assertLess(5.0, o.build_min(t1))
        self.assertAlmostEqual(o.build_min(t2), E.ME10_FACTOR * 3 * 5.0)

    def test_a_material_priced_above_its_build_cost_is_built(self):
        o, t1, t2 = self._chain(self.M)
        self.assertGreater(o.price(t1)[0], o.build_min(t1))
        self.assertAlmostEqual(o.build_min(t2), E.ME10_FACTOR * 3 * o.build_min(t1))

    def test_product_quantity_divides(self):
        o, t1, _ = self._chain(self.M, qty1=5)
        self.assertAlmostEqual(o.build_min(t1), E.ME10_FACTOR * 10 * 2.0 / 5)

    def test_the_cheaper_blueprint_sets_build_min_while_the_first_sets_the_price(self):
        o, t1, _ = self._chain(self.M, extra=[(990213, 990201, MIN_A, 4)])
        self.assertAlmostEqual(o.price(t1)[0], self.M * 10 * 2.0)
        self.assertAlmostEqual(o.build_min(t1), E.ME10_FACTOR * 4 * 2.0)

    def test_a_cheap_override_on_a_raw_material_feeds_build_min(self):
        o, t1, _ = self._chain(self.M, overrides={MIN_A: 0.5})
        self.assertAlmostEqual(o.build_min(t1), E.ME10_FACTOR * 10 * 0.5)

    def test_a_cycle_has_none_and_nothing_is_memoised_under_the_cut(self):
        a, b = 990301, 990302
        m = {MIN_A: {"base_price": 2.0, "portion_size": 1},
             a: {"base_price": 0.0, "portion_size": 1}, b: {"base_price": 0.0, "portion_size": 1}}
        bps = {}
        for bp, prod, mats in ((990311, a, [(b, 1)]), (990312, b, [(a, 1)])):
            x = _bp(60, 1, manufacturing={
                "materials": [{"typeID": t, "quantity": q} for t, q in mats],
                "products": [{"typeID": prod, "quantity": 1}]})
            x["blueprintTypeID"] = bp
            bps[bp] = x
        o = E.Oracle(m, E.recipes(E.blueprint_rows(bps)), {}, {}, {})
        self.assertIsNone(o.build_min(a))
        self.assertIsNone(o.build_min(b))

    def test_the_prices_row_carries_it_last(self):
        o, t1, _ = self._chain(self.M)
        m = {t: {**r, "market_group_id": MG, "group_id": 1, "category_id": 1,
                 "meta_group_id": None} for t, r in {
            MIN_A: {"base_price": 2.0, "portion_size": 1},
            t1: {"base_price": 0.0, "portion_size": 1}}.items()}
        rows = {r[0]: r for r in E.price_rows(m, o)}
        self.assertIsNone(rows[MIN_A][-1])
        self.assertAlmostEqual(rows[t1][-1], o.build_min(t1))
        self.assertEqual(len(rows[t1]), len(E.PRICES_HEADER))
        self.assertTrue(content.encode_row(rows[MIN_A]).endswith("\t"))


class Blueprints(unittest.TestCase):
    def test_rows_round_trip_the_documents(self):
        bps = _blueprints()
        rows = E.blueprint_rows(bps)
        lines = [content.encode_row(r).split("\t") for r in rows]
        decoded = [(int(f[0]), int(f[1]), f[2], int(f[3]), int(f[4]),
                    float(f[5]) if f[5] else None, int(f[6]), int(f[7])) for f in lines]
        self.assertEqual(E.blueprints_from_rows(decoded),
                         {k: E.normalise(v) for k, v in bps.items()})

    def test_an_activity_with_no_lists_keeps_its_row(self):
        rows = [r for r in E.blueprint_rows(_blueprints()) if r[0] == FRIG_BP]
        copying = [r for r in rows if r[1] == E.ACTIVITY_IDS["copying"]]
        self.assertEqual(copying, [(FRIG_BP, 5, "activity", 0, 0, None, 600, 30)])
        self.assertEqual([r[1] for r in rows], sorted(r[1] for r in rows))
        inv = [r for r in rows if r[2] == "product" and r[1] == 8]
        self.assertEqual(inv, [(FRIG_BP, 8, "product", MOD_BP, 10, 0.25, 600, 30)])

    def test_an_unknown_activity_is_refused(self):
        with self.assertRaises(E.EconError):
            E.blueprint_rows({FRIG_BP: _bp(1, 1, smelting={})})

    def test_the_blueprint_cache_decodes_from_synthetic_sqlite_bytes(self):
        # `blueprints.static` is an fsdlite cache: `cache(key, value, time)`
        # with one JSON document per blueprint (`fsd.load_sqlite`).
        fsd = content._imp("fsd")  # noqa: SLF001
        with tempfile.TemporaryDirectory() as d:
            path = os.path.join(d, "blueprints.static")
            con = sqlite3.connect(path)
            con.execute("CREATE TABLE cache (key, value, time)")
            con.execute("CREATE TABLE indexes (key, value)")
            for k, v in _blueprints().items():
                con.execute("INSERT INTO cache VALUES (?, ?, 0)", (str(k), json.dumps(v)))
            con.commit()
            con.close()
            got = {int(k): v for k, v in fsd.sqlite_rows(path)}
        self.assertEqual(E.blueprint_rows(got), E.blueprint_rows(_blueprints()))


def _static(d, name, docs):
    """A synthetic fsdlite `.static`: `cache(key, value, time)`, JSON values."""
    path = os.path.join(d, name)
    con = sqlite3.connect(path)
    con.execute("CREATE TABLE cache (key, value, time)")
    for k, v in docs.items():
        con.execute("INSERT INTO cache VALUES (?, ?, 0)", (str(k), json.dumps(v)))
    con.commit()
    con.close()
    return path


class StructureIndustry(unittest.TestCase):
    """#57: the two optional bundle files, from invented documents."""
    HULL, RIG, FILTER = 990401, 990402, 990501
    SOURCES = {
        HULL: {"manufacturing": {"time": [{"dogmaAttributeID": 990601}],
                                 "material": [{"dogmaAttributeID": 990602}],
                                 "cost": [{"dogmaAttributeID": 990603}]},
               "reaction": {"time": [{"dogmaAttributeID": 990604}]}},
        RIG: {"research_time": {"time": [{"dogmaAttributeID": 990605,
                                          "filterID": FILTER}]}},
    }

    def test_one_sorted_row_per_entry_with_the_filter_zero_when_absent(self):
        got = E.modifier_source_rows(self.SOURCES)
        self.assertEqual(got, sorted(got))
        self.assertEqual(len(got), 5)
        self.assertIn((self.HULL, 1, "material", 990602, 0), got)
        self.assertIn((self.HULL, 9, "time", 990604, 0), got)
        self.assertIn((self.RIG, 3, "time", 990605, self.FILTER), got)

    def test_an_unknown_activity_or_kind_is_refused(self):
        with self.assertRaises(E.EconError):
            E.modifier_source_rows({1: {"smelting": {"time": []}}})
        with self.assertRaises(E.EconError):
            E.modifier_source_rows({1: {"manufacturing": {"speed": []}}})

    def test_target_filters_join_their_lists_sorted_by_id(self):
        docs = {self.FILTER + 1: {"name": "b", "categoryIDs": [], "groupIDs": [9, 3]},
                self.FILTER: {"name": "a", "categoryIDs": [7, 6], "groupIDs": []}}
        self.assertEqual(E.target_filter_rows(docs),
                         [(self.FILTER, "6,7", ""), (self.FILTER + 1, "", "3,9")])

    def test_the_rows_read_back_from_synthetic_sqlite_bytes(self):
        fsd = content._imp("fsd")  # noqa: SLF001
        with tempfile.TemporaryDirectory() as d:
            path = _static(d, "m.static", self.SOURCES)
            got = {int(k): v for k, v in fsd.sqlite_rows(path)}
        self.assertEqual(E.modifier_source_rows(got), E.modifier_source_rows(self.SOURCES))

    def test_a_client_without_the_files_skips_them_with_a_note(self):
        notes = []

        class Client:
            def path(self, res):
                return os.path.join(tempfile.gettempdir(), "nonexistent-57", res.split("/")[-1])

        class Ctx:
            client = Client()

            def note(self, msg):
                notes.append(msg)

        self.assertEqual(E.optional_rows(Ctx()), {})
        self.assertEqual(len(notes), 2)
        self.assertTrue(all("not written" in n for n in notes))

    def test_an_unreadable_layout_skips_the_file_with_a_note(self):
        notes = []
        with tempfile.TemporaryDirectory() as d:
            # an unknown activity name, and a filter document with no lists key shape
            _static(d, "industry_activity_modifier_sources.static",
                    {self.HULL: {"smelting": {"time": [{"dogmaAttributeID": 1}]}}})
            _static(d, "industry_activity_target_filters.static",
                    {self.FILTER: {"name": "a", "categoryIDs": [6], "groupIDs": [3]}})

            class Client:
                def path(self, res):
                    return os.path.join(d, res.split("/")[-1])

            class Ctx:
                client = Client()

                def note(self, msg):
                    notes.append(msg)

            got = E.optional_rows(Ctx())
            # a missing dogmaAttributeID key is skipped too
            os.remove(os.path.join(d, "industry_activity_modifier_sources.static"))
            _static(d, "industry_activity_modifier_sources.static",
                    {self.HULL: {"manufacturing": {"time": [{"filterID": 1}]}}})
            got2 = E.optional_rows(Ctx())
        self.assertEqual(sorted(got), ["industry/target-filters.tsv"])
        self.assertEqual(sorted(got2), ["industry/target-filters.tsv"])
        self.assertEqual(len([n for n in notes if "unreadable layout" in n]), 2)

    def test_a_client_with_the_files_writes_both(self):
        with tempfile.TemporaryDirectory() as d:
            _static(d, "industry_activity_modifier_sources.static", self.SOURCES)
            _static(d, "industry_activity_target_filters.static",
                    {self.FILTER: {"name": "a", "categoryIDs": [6], "groupIDs": [3]}})

            class Client:
                def path(self, res):
                    return os.path.join(d, res.split("/")[-1])

            class Ctx:
                client = Client()

                def note(self, msg):
                    pass

            got = E.optional_rows(Ctx())
        self.assertEqual(sorted(got), ["industry/modifier-sources.tsv",
                                       "industry/target-filters.tsv"])
        self.assertEqual(got["industry/target-filters.tsv"][1], [(self.FILTER, "6", "3")])


class Stations(unittest.TestCase):
    OPS = {99: {"activityID": 1, "services": [7, 21, 25], "stationTypes": {2: 990002, 1: 990001},
                "manufacturingFactor": 1.0, "researchFactor": 0.98}}

    def test_one_sorted_row_per_station_with_its_operation_services(self):
        st = [(69000002, 39000001, 1999001, 99, 0.5, 0.05),
              (69000001, 39000002, 1999002, 99, 0.4, 0.025)]
        rows = E.station_rows(st, {39000001: 19990001, 39000002: 19000002}, self.OPS)
        self.assertEqual([r[0] for r in rows], [69000001, 69000002])
        self.assertEqual(content.encode_row(rows[0]),
                         "69000001\t39000002\t19000002\t1999002\t99\t7,21,25\t0.4\t0.025")

    def test_operations_write_the_station_type_map(self):
        self.assertEqual(content.encode_row(E.operation_rows(self.OPS)[0]),
                         "99\t1\t7,21,25\t1=990001;2=990002\t1.0\t0.98")


class Authored(unittest.TestCase):
    def test_a_bad_header_is_refused_and_a_bad_row_skipped(self):
        with self.assertRaises(E.EconError):
            E.parse_overrides(["# preamble\n", "type_id\tprice\n"])
        rows, problems = E.parse_overrides([
            "# preamble\n", "type_id\tprice\twhy\n", "990300\t18000.0\t[A] x\n",
            "990301\t-1\t[A] negative\n", "990302\t5\n"])
        self.assertEqual(rows, {990300: 18000.0})
        self.assertEqual(len(problems), 2)
        mult, _ = E.parse_meta_mult(["meta_group_id\tmult\twhy\n", "99\t2.0\t[A] x\n"])
        self.assertEqual(mult, {99: 2.0})


if __name__ == "__main__":
    unittest.main()
