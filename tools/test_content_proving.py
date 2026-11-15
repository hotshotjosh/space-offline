"""Unit tests for `tools/content_proving.py` (PG-1a, #64).

Invented rows only (ids in the 99xxxx band). The bundle-backed run is the
`proving` section of `tools/content.py`.
Run: `python3 -I tools/test_content_proving.py -q`.
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

import content_proving as P  # noqa: E402

M1, M2 = 990801, 990802
FIL_A, FIL_B, NOT_FIL = 990811, 990812, 990813
GROUP_OTHER = 990821


def match(filament, **extra):
    row = {f: 990900 + i for i, f in enumerate(P.FIELDS)}
    row.update({"filamentTypeID": filament, "rewards": []})
    for f in ("minimumQueueSizeMultiplier", "maxOfShipTypeRestriction",
              "maxOfShipGroupRestriction", "moduleAndImplantMetaLevelRestrictions",
              "bannedModulesTypeListID", "limitedAmountOfModulesTypeListID",
              "numberOfLimitedModules", "provingGroundLootTableID"):
        row[f] = None
    row.update(extra)
    return row


class DeriveTest(unittest.TestCase):
    def test_optional_bit_clear_writes_empty(self):
        trows, _ = P.derive({M1: match(FIL_A)})
        col = {name: v for name, v in zip(P.TYPES_HEADER, trows[0])}
        self.assertEqual(col["max_of_ship_type"], None)
        self.assertEqual(col["loot_table_id"], None)
        self.assertEqual(col["fleets"], 990900 + P.FIELDS.index("numberOfFleets"))
        # and the encoded line holds an empty field, not the word None
        enc = _enc(trows[0]).split("\t")
        self.assertEqual(enc[P.TYPES_HEADER.index("max_of_ship_type")], "")
        self.assertEqual(len(enc), len(P.TYPES_HEADER))

    def test_columns_follow_fields(self):
        self.assertEqual(len(P.FIELDS) + 1, len(P.TYPES_HEADER))
        row = match(FIL_A, **{f: 990950 + i for i, f in enumerate(P.FIELDS)})
        trows, _ = P.derive({M1: row})
        for h, f in zip(P.TYPES_HEADER[1:], P.FIELDS):
            self.assertEqual(trows[0][P.TYPES_HEADER.index(h)], row[f], h)

    def test_a_set_optional_is_kept(self):
        trows, _ = P.derive({M1: match(FIL_A, maxOfShipTypeRestriction=3)})
        self.assertEqual(trows[0][P.TYPES_HEADER.index("max_of_ship_type")], 3)

    def test_rewards_one_row_per_entry(self):
        rewards = [{"amount": 5, "typeID": 990831}, {"amount": 7, "typeID": 990832}]
        trows, rrows = P.derive({M2: match(FIL_B), M1: match(FIL_A, rewards=rewards)})
        self.assertEqual([r[0] for r in trows], [M1, M2])
        self.assertEqual(rrows, [(M1, 5, 990831), (M1, 7, 990832)])

    def test_no_rewards_no_rows(self):
        self.assertEqual(P.derive({M1: match(FIL_A, rewards=None)})[1], [])


class CheckTest(unittest.TestCase):
    TYPES = {FIL_A: {"groupID": P.FILAMENT_GROUP}, FIL_B: {"groupID": P.FILAMENT_GROUP},
             NOT_FIL: {"groupID": GROUP_OTHER}}

    def test_check_passes_filaments(self):
        self.assertEqual(P.problems({M1: match(FIL_A), M2: match(FIL_B)}, self.TYPES), [])

    def test_check_flags_non_filament_type(self):
        bad = P.problems({M1: match(FIL_A), M2: match(NOT_FIL)}, self.TYPES)
        self.assertEqual(len(bad), 1)
        self.assertIn(str(M2), bad[0])

    def test_check_flags_unknown_type(self):
        self.assertEqual(len(P.problems({M1: match(990899)}, self.TYPES)), 1)


class StaleFileTest(unittest.TestCase):
    def setUp(self):
        self._gather = P.gather
        self.addCleanup(setattr, P, "gather", self._gather)

    def test_check_flags_stale_file(self):
        import tempfile
        content = P._content()  # noqa: SLF001
        matches = {M1: match(FIL_A, rewards=[{"amount": 5, "typeID": 990831}])}
        types = {FIL_A: {"groupID": P.FILAMENT_GROUP}}
        P.gather = lambda ctx: (matches, types)
        with tempfile.TemporaryDirectory() as d:
            os.makedirs(os.path.join(d, "proving"))

            class Ctx:
                @staticmethod
                def path(rel):
                    return os.path.join(d, rel)

            trows, rrows = P.derive(matches)
            for rel, rows in ((P.TYPES_FILE, trows), (P.REWARDS_FILE, rrows)):
                with open(Ctx.path(rel), "w", encoding="utf-8") as fh:
                    fh.write("".join(content.encode_row(r) + "\n" for r in rows))
            self.assertEqual(P.check(Ctx), [])
            matches[M1]["fleetSize"] += 1
            bad = P.check(Ctx)
            self.assertEqual(len(bad), 1)
            self.assertIn(P.TYPES_FILE, bad[0])


def _enc(row):
    P._content()  # noqa: SLF001
    import content
    return content.encode_row(row)


if __name__ == "__main__":
    unittest.main()
