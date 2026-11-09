"""Unit tests for `tools/content_dailygoals.py` (DG-1, #202).

Invented labels and ids only. Run: `python3 -I tools/test_content_dailygoals.py -q`.
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

import content_dailygoals as D  # noqa: E402

LABELS = {"UI/Test/A": 11, "UI/Test/B": 12, "UI/Test/C": 13}


def goal(key="g1", category="daily", method="kill_npc", target="5", career="none",
         name="UI/Test/A", desc="UI/Test/B", help="UI/Test/C", weight="1", isk="0", why="w"):
    return "\t".join([key, category, method, target, career, name, desc, help, weight, isk, why])


def parse(*rows):
    return D.parse_goals(["# a comment", D.AUTHORED_HEADER, ""] + list(rows))


def derive(*rows):
    return D.derive(parse(*rows), LABELS)


class Derive(unittest.TestCase):
    def test_labels_become_ids_in_bundle_column_order(self):
        rows = derive(goal(isk="7", weight="3"),
                      goal("b", "bonus", "complete_daily", "0", name="UI/Test/C"))
        self.assertEqual(rows[0], ("g1", "daily", "kill_npc", 5, "none", 11, 12, 13, 3, 7))
        self.assertEqual(rows[1][5:8], (13, 12, 13))
        self.assertEqual(len(rows[0]), len(D.BUNDLE_HEADER))

    def test_an_unresolvable_label_is_refused(self):
        for field in ("name", "desc", "help"):
            with self.subTest(field=field), self.assertRaises(D.ContentError):
                derive(goal(**{field: "UI/Test/Nope"}))

    def test_unknown_method_category_and_career_are_refused(self):
        for kw in ({"method": "teleport"}, {"category": "weekly"}, {"career": "pirate"}):
            with self.subTest(kw=kw), self.assertRaises(D.ContentError):
                derive(goal(**kw))

    def test_two_bonus_rows_are_refused(self):
        b = ("bonus", "complete_daily", "0")
        with self.assertRaises(D.ContentError):
            derive(goal("b1", b[0], b[1], b[2]), goal("b2", b[0], b[1], b[2]))

    def test_negative_and_non_numeric_numbers_are_refused(self):
        for kw in ({"target": "-1"}, {"weight": "-1"}, {"isk": "-5"}, {"isk": "x"}):
            with self.subTest(kw=kw), self.assertRaises(D.ContentError):
                derive(goal(**kw))

    def test_target_zero_is_only_for_the_bonus(self):
        with self.assertRaises(D.ContentError):
            derive(goal(target="0"))
        self.assertEqual(derive(goal("b", "bonus", "complete_daily", "0"))[0][3], 0)

    def test_duplicate_key_and_mismatched_bonus_are_refused(self):
        with self.assertRaises(D.ContentError):
            derive(goal(), goal())
        with self.assertRaises(D.ContentError):
            derive(goal("b", "bonus", "kill_npc", "5"))
        with self.assertRaises(D.ContentError):
            derive(goal("b", "daily", "complete_daily", "0"))


class Header(unittest.TestCase):
    def test_a_wrong_header_or_field_count_is_refused(self):
        with self.assertRaises(D.ContentError):
            D.parse_goals(["key\tcategory"])
        with self.assertRaises(D.ContentError):
            D.parse_goals([D.AUTHORED_HEADER, "a\tb"])

    def test_the_header_round_trips(self):
        cols = D.AUTHORED_HEADER.split("\t")
        self.assertEqual(list(parse(goal())[0]), cols)
        self.assertEqual(list(D.BUNDLE_HEADER), ["key", "category", "method", "target", "career",
                                                 "name_id", "desc_id", "help_id", "weight", "isk"])

    def test_the_shipped_file_parses_and_has_one_bonus(self):
        path = os.path.join(os.path.dirname(_HERE), "content", "dailygoals", "goals.tsv")
        with open(path, encoding="utf-8") as fh:
            rows = D.parse_goals(fh.read().splitlines())
        self.assertEqual(sum(r["category"] == "bonus" for r in rows), 1)
        # Every shipped label resolves once given ids (invented here).
        labels = {r[f]: i for i, r in enumerate(rows) for f in ("name", "description", "help")}
        self.assertEqual(len(D.derive(rows, labels)), len(rows))


if __name__ == "__main__":
    unittest.main()
