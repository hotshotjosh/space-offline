"""Unit tests for the `mutators` content section, `tools/content_mutators.py` (#222).

Invented rows only: mutators 981_000_00x, sources 981_100_00x, results
981_200_00x, attributes 981_300_00x. The bundle-backed check is
`content.py --check`.
Run: `python3 -I tools/test_content_mutators.py -q`.
"""

from __future__ import annotations

import os
import sys
import unittest

# `tools/` goes LAST: it holds a `dis.py` that would shadow the stdlib's.
_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content_mutators as M  # noqa: E402

MUT_A, MUT_B = 981_000_001, 981_000_002
SRC_1, SRC_2, SRC_3 = 981_100_001, 981_100_002, 981_100_003
RES_X, RES_Y = 981_200_001, 981_200_002
ATTR_P, ATTR_Q = 981_300_001, 981_300_002


def _table():
    return {
        MUT_B: {"attributeIDs": {str(ATTR_Q): {"min": 0.5, "max": 1.5}},
                "inputOutputMapping": [{"applicableTypes": [SRC_3], "resultingType": RES_Y}]},
        MUT_A: {"attributeIDs": {str(ATTR_Q): {"min": 0.75, "max": 1.25, "highIsGood": False},
                                 ATTR_P: {"min": 0.5, "max": 2.0, "highIsGood": True}},
                "inputOutputMapping": [
                    {"applicableTypes": [SRC_2, SRC_1], "resultingType": RES_X}]},
    }


def _clean():
    r, m = M.derive(_table())
    typedata = {MUT_A: (M.MUTATOR_GROUP, 17, True), MUT_B: (M.MUTATOR_GROUP, 17, True),
                SRC_1: (1, 7, True), SRC_2: (1, 7, False), SRC_3: (1, 7, True),
                RES_X: (1, 7, True), RES_Y: (1, 7, True)}
    return r, m, typedata, {ATTR_P, ATTR_Q}, {RES_X, RES_Y}, set()


class Rows(unittest.TestCase):
    def test_ranges_are_sorted_and_keys_are_ints(self):
        self.assertEqual(M.ranges_rows(_table()), [
            (MUT_A, ATTR_P, 0.5, 2.0, 1), (MUT_A, ATTR_Q, 0.75, 1.25, 0),
            (MUT_B, ATTR_Q, 0.5, 1.5, "")])

    def test_mappings_one_row_per_source_and_keep_unpublished(self):
        self.assertEqual(M.mapping_rows(_table()), [
            (MUT_A, RES_X, SRC_1), (MUT_A, RES_X, SRC_2), (MUT_B, RES_Y, SRC_3)])


class Problems(unittest.TestCase):
    def one(self, mutate):
        a = list(_clean())
        mutate(a)
        got = M.problems(*a)
        self.assertEqual(len(got), 1, got)
        return got[0]

    def test_clean_table_has_none(self):
        self.assertEqual(M.problems(*_clean()), [])

    def test_mutator_not_in_typedata(self):
        got = self.one(lambda a: a[2].pop(MUT_A))
        self.assertIn(str(MUT_A), got)
        self.assertIn("is not in typedata", got)

    def test_mutator_wrong_group(self):
        def f(a):
            a[2][MUT_A] = (7, 17, True)
        self.assertIn("group", self.one(f))

    def test_mutator_without_range(self):
        def f(a):
            a[0][:] = [x for x in a[0] if x[0] != MUT_B]
        self.assertIn("no range", self.one(f))

    def test_mutator_without_mapping(self):
        def f(a):
            a[1][:] = [x for x in a[1] if x[0] != MUT_B]
            a[4].discard(RES_Y)
        self.assertIn("no mapping", self.one(f))

    def test_unknown_attribute(self):
        got = self.one(lambda a: a[3].discard(ATTR_P))
        self.assertIn(str(ATTR_P), got)
        self.assertIn("attribute", got)

    def test_non_finite_range(self):
        def f(a):
            a[0][0] = (MUT_A, ATTR_P, float("nan"), 2.0, 1)
        self.assertIn("not finite", self.one(f))

    def test_source_with_two_results(self):
        def f(a):
            a[1].append((MUT_A, RES_Y, SRC_1))
        self.assertIn("maps to results", self.one(f))

    def test_result_not_in_typedata(self):
        self.assertIn("result type", self.one(lambda a: a[2].pop(RES_Y)))

    def test_source_not_in_typedata(self):
        self.assertIn("source type", self.one(lambda a: a[2].pop(SRC_3)))

    def test_result_that_is_also_a_source(self):
        def f(a):
            a[1].append((MUT_B, RES_Y, RES_X))
        self.assertIn("both a result and a source", self.one(f))

    def test_result_not_dynamic(self):
        self.assertIn("not an isDynamicType", self.one(lambda a: a[4].discard(RES_Y)))

    def test_dynamic_type_without_mutator(self):
        self.assertIn("no mutator's result", self.one(lambda a: a[4].add(981_200_099)))

    def test_mutator_named_by_npc_row(self):
        self.assertIn("NPC", self.one(lambda a: a[5].add(MUT_A)))


if __name__ == "__main__":
    unittest.main()
