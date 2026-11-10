"""Unit tests for `tools/content_wallet.py` (lane wallet-l1).

Invented records only (ids in the 99xxxx band). The bundle-backed run is the
`wallet` section of `tools/content.py` (`tools/up.sh`).
Run: `python3 -I tools/test_content_wallet.py -q`.
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

import content_wallet as W  # noqa: E402


def rec(tid, name_id):
    return {"entryTypeID": tid, "entryTypeNameID": name_id, "name": "invented"}


class RowsTest(unittest.TestCase):
    def test_three_records_become_three_rows_sorted_by_entry_type(self):
        got = W.rows([rec(990_203, 990_303), rec(990_201, 990_301), rec(990_202, 990_302)])
        self.assertEqual(got, [(990_201, 990_301), (990_202, 990_302), (990_203, 990_303)])

    def test_a_record_without_a_name_id_gets_no_row(self):
        got = W.rows([rec(990_201, 990_301), rec(990_202, None), rec(990_203, 0),
                      {"entryTypeID": 990_204}])
        self.assertEqual(got, [(990_201, 990_301)])

    def test_the_same_entry_type_twice_is_refused(self):
        with self.assertRaises(ValueError):
            W.rows([rec(990_201, 990_301), rec(990_201, 990_302)])

    def test_ids_come_out_as_ints(self):
        self.assertEqual(W.rows([rec("990201", "990301")]), [(990_201, 990_301)])

    def test_the_module_is_a_section(self):
        self.assertEqual((W.SECTION, W.DEPENDS, W.CFSD), ("wallet", (), ("accountingentrytypes",)))
        self.assertEqual(W.HEADER, ("entry_type_id", "name_id"))


class RunTest(unittest.TestCase):
    def test_a_table_that_follows_the_run_has_no_disagreement(self):
        table = {t: W.RUN_BASE + t for t in range(0, W.RUN_MAX + 1) if t not in W.RUN_GAPS}
        table[990_201] = 990_301
        self.assertEqual(W.run_disagreements(table), [])

    def test_a_run_id_with_no_row_is_found(self):
        table = {t: W.RUN_BASE + t for t in range(0, W.RUN_MAX + 1) if t not in W.RUN_GAPS}
        del table[7]
        got = W.run_disagreements(table)
        self.assertEqual(len(got), 1)
        self.assertIn("no row", got[0])

    def test_a_gap_with_a_row_and_a_wrong_run_id_are_found(self):
        table = {t: W.RUN_BASE + t for t in range(0, W.RUN_MAX + 1) if t not in W.RUN_GAPS}
        table[W.RUN_GAPS[0]] = 990_301
        table[7] = 990_302
        self.assertEqual(len(W.run_disagreements(table)), 2)

    def test_a_row_past_the_run_that_restates_the_formula_is_found(self):
        past = W.RUN_MAX + 5
        table = {t: W.RUN_BASE + t for t in range(0, W.RUN_MAX + 1) if t not in W.RUN_GAPS}
        table[past] = W.RUN_BASE + past
        self.assertEqual(len(W.run_disagreements(table)), 1)


if __name__ == "__main__":
    unittest.main()
