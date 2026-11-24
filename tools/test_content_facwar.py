"""Unit tests for the `facwar` content section, `tools/content_facwar.py` (FW-3).

Synthetic rows only, invented ids in `schema.md`'s unused ranges. The
client-backed check is `content.py --sections facwar --check`.
Run: `python3 -I tools/test_content_facwar.py -q`.
"""

from __future__ import annotations

import os
import sys
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content_facwar as F  # noqa: E402

SPRITE = {"territoryIconFile": "x", "imageHeight": 1, "imageWidth": 1, "xOffset": 0, "yOffset": 0}


def row(sid, visible=1, sprite=True):
    r = {"solarSystemId": sid, "xpos": 1, "ypos": 1, "isVisible": visible}
    if sprite:
        r["territorySprite"] = SPRITE
    return r


class Derive(unittest.TestCase):
    def test_only_visible_sprite_rows_are_territory(self):
        zones = {98: {"systems": {"39000001": row(39000001),
                                  "39000002": row(39000002, sprite=False),
                                  "39000003": row(39000003, visible=0)}},
                 99: {"systems": {"39000004": row(39000004)}}}
        rows, problems = F.derive(zones)
        self.assertEqual(rows, [(39000001, 98), (39000004, 99)])
        self.assertEqual(problems, [])

    def test_a_system_in_two_warzones_is_a_problem(self):
        zones = {98: {"systems": {"39000001": row(39000001)}},
                 99: {"systems": {"39000001": row(39000001)}}}
        rows, problems = F.derive(zones)
        self.assertEqual(rows, [(39000001, 98)])
        self.assertEqual(len(problems), 1)

    def test_a_key_that_disagrees_with_its_row_is_a_problem(self):
        rows, problems = F.derive({98: {"systems": {"39000001": row(39000002)}}})
        self.assertEqual((rows, len(problems)), ([], 1))

    def test_rows_sort_by_system(self):
        zones = {99: {"systems": {"39000009": row(39000009)}},
                 98: {"systems": {"39000002": row(39000002)}}}
        self.assertEqual(F.derive(zones)[0], [(39000002, 98), (39000009, 99)])


def dict_ops(entries, name="facwarMaxLPPayout"):
    """The bytecode of `name = {key: value, ...}` as `dis` yields it."""
    ops = [("LOAD_CONST", 0, "1"), ("STORE_NAME", 0, "other"), ("BUILD_MAP", 2, "")]
    for key, val in entries:
        ops += [("LOAD_CONST", 1, str(val)), ("LOAD_NAME", 2, key), ("STORE_MAP", None, "")]
    return ops + [("STORE_NAME", 3, name)]


class MaxLp(unittest.TestCase):
    GROUPS = {"groupAlpha": 9001, "groupBeta": 9002}

    def test_the_dict_literal_becomes_sorted_group_rows(self):
        ops = dict_ops([("groupBeta", 700), ("groupAlpha", 300)])
        rows, problems = F.max_lp_rows(ops, self.GROUPS)
        self.assertEqual((rows, problems), ([(9001, 300), (9002, 700)], []))

    def test_an_unknown_group_name_is_a_problem(self):
        rows, problems = F.max_lp_rows(dict_ops([("groupGamma", 5)]), self.GROUPS)
        self.assertEqual((rows, len(problems)), ([], 1))

    def test_a_missing_or_non_literal_dict_is_a_problem(self):
        self.assertEqual(len(F.max_lp_rows([], self.GROUPS)[1]), 1)
        ops = [("LOAD_NAME", 0, "x"), ("STORE_NAME", 1, "facwarMaxLPPayout")]
        self.assertEqual(len(F.max_lp_rows(ops, self.GROUPS)[1]), 1)


class Scoreboards(unittest.TestCase):
    def test_a_type_with_the_component_is_a_row_and_the_sign_is_kept(self):
        comps = {990001: {"fwScoreboard": {"attackerWinsThreshold": 30,
                                           "occupierWinsThreshold": -40}},
                 990002: {"fwCapturePoint": {}}}
        rows, problems = F.derive_scoreboards(comps)
        self.assertEqual((rows, problems), ([(990001, 30, -40)], []))

    def test_an_absent_threshold_is_empty_and_the_rows_sort(self):
        comps = {990009: {"fwScoreboard": {}},
                 990003: {"fwScoreboard": {"occupierWinsThreshold": 25}}}
        rows, problems = F.derive_scoreboards(comps)
        self.assertEqual(rows, [(990003, None, 25), (990009, None, None)])
        self.assertEqual(problems, [])
        self.assertEqual(F._content().encode_row(rows[1]), "990009\t\t")

    def test_a_non_int_threshold_is_a_problem(self):
        rows, problems = F.derive_scoreboards(
            {990001: {"fwScoreboard": {"attackerWinsThreshold": 1.5}}})
        self.assertEqual((rows, len(problems)), ([(990001, None, None)], 1))


class CapturePoints(unittest.TestCase):
    FULL = {"numberOfSeconds": 45, "range": 7000, "pointValue": 2,
            "looping": 1, "capturingTypeListID": 990101,
            "blockingTypeListID": 990102}

    def test_a_type_with_the_component_is_a_row_in_column_order(self):
        comps = {990001: {"fwCapturePoint": self.FULL},
                 990002: {"fwScoreboard": {}}}
        rows, problems = F.derive_capture_points(comps)
        self.assertEqual((rows, problems),
                         ([(990001, 45, 7000, 2, 1, 990101, 990102)], []))

    def test_a_bool_looping_is_one_and_an_absent_field_is_empty(self):
        comps = {990009: {"fwCapturePoint": {"looping": False, "range": 5}},
                 990003: {"fwCapturePoint": {"looping": True}}}
        rows, problems = F.derive_capture_points(comps)
        self.assertEqual(rows, [(990003, None, None, None, 1, None, None),
                                (990009, None, 5, None, 0, None, None)])
        self.assertEqual(problems, [])
        self.assertEqual(F._content().encode_row(rows[0]), "990003\t\t\t\t1\t\t")

    def test_a_non_int_field_is_a_problem(self):
        rows, problems = F.derive_capture_points(
            {990001: {"fwCapturePoint": {"range": "far"}}})
        self.assertEqual((rows, len(problems)),
                         ([(990001, None, None, None, None, None, None)], 1))

    def test_a_looping_outside_zero_and_one_is_a_problem_and_not_written(self):
        rows, problems = F.derive_capture_points(
            {990001: {"fwCapturePoint": {"looping": 2}}})
        self.assertEqual((rows, len(problems)),
                         ([(990001, None, None, None, None, None, None)], 1))


class NoLayout(unittest.TestCase):
    """`spacecomponentsbytype` has no recoverable layout: the section still
    writes systems.tsv, and check reports problems instead of raising."""

    class FsdError(Exception):
        pass

    def setUp(self):
        outer = self

        class Fsd:
            FsdError = outer.FsdError

            @staticmethod
            def rows(_client, name):
                if name == "warzonespositiondata":
                    return {98: {"systems": {"39000001": row(39000001)}}}
                raise outer.FsdError(name)

        class Content:
            @staticmethod
            def _imp(_name):
                return Fsd

            @staticmethod
            def read_rows(path):
                raise OSError(path)

            @staticmethod
            def encode_row(r):
                return "\t".join(str(x) for x in r)

        class Ctx:
            client = None
            notes = []

            def note(self, m):
                self.notes.append(m)

            @staticmethod
            def path(p):
                return "/nonexistent/probe/" + p

        self.ctx = Ctx()
        self.saved = (F._content, F._max_lp)
        F._content = lambda: Content
        F._max_lp = lambda: None

    def tearDown(self):
        F._content, F._max_lp = self.saved

    def test_the_section_skips_the_two_files_with_a_note(self):
        out = F.section(self.ctx)
        self.assertEqual([o[0] for o in out], ["facwar/systems.tsv"])
        self.assertTrue(any("capture_points.tsv: no layout" in n for n in self.ctx.notes))

    def test_check_reports_problems_and_never_raises(self):
        bad = F.check(self.ctx)
        self.assertTrue(any("systems.tsv" in b for b in bad), bad)


if __name__ == "__main__":
    unittest.main()
