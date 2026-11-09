"""Unit tests for `tools/content_moon.py` (#52). Invented rows only (ids in the 99xxxx band).
Run: `python3 -I tools/test_content_moon.py -q`."""

from __future__ import annotations

import os
import sys
import unittest

_HERE = os.path.dirname(os.path.abspath(__file__))
if _HERE in sys.path:
    sys.path.remove(_HERE)
sys.path.append(_HERE)

import content_moon as M  # noqa: E402

SYS_A, SYS_B = 990001, 990002
MOON_1, MOON_2, MOON_3 = 990101, 990102, 990103


def world():
    return {
        SYS_A: {"planets": {1: {"moons": {
            MOON_2: {"miningBeacon": {"position": (1.0, 2.0, 3.0), "itemID": 990900}},
            MOON_1: {"miningBeacon": {"position": (4.0, 5.0, 6.0)}},
            MOON_3: {},
        }}}},
        SYS_B: {"planets": {}},
    }


class Beacons(unittest.TestCase):
    def test_a_moon_with_a_beacon_gives_one_row(self):
        rows, _ = M.derive_beacons(world(), [SYS_A])
        self.assertEqual(len([r for r in rows if r[0] == MOON_2]), 1)
        self.assertEqual([r for r in rows if r[0] == MOON_2][0],
                         (MOON_2, SYS_A, 1.0, 2.0, 3.0, 990900))

    def test_a_moon_without_one_gives_none(self):
        rows, _ = M.derive_beacons(world(), [SYS_A, SYS_B])
        self.assertNotIn(MOON_3, [r[0] for r in rows])

    def test_rows_are_sorted_by_moon(self):
        rows, _ = M.derive_beacons(world(), [SYS_A])
        self.assertEqual([r[0] for r in rows], [MOON_1, MOON_2])

    def test_a_missing_item_id_is_zero(self):
        rows, first = M.derive_beacons(world(), [SYS_A])
        self.assertEqual(rows[0][5], 0)
        self.assertTrue(first)

    def test_check_flags_a_beacon_far_from_its_moon(self):
        moons = {MOON_1: (0.0, 0.0, 0.0, 1000.0)}
        near = [(MOON_1, SYS_A, 1.0e6, 0.0, 0.0, 0)]
        far = [(MOON_1, SYS_A, 1.0e9, 0.0, 0.0, 0)]
        self.assertEqual(M.check_beacons(near, moons, True), [])
        self.assertEqual(len(M.check_beacons(far, moons, True)), 1)
        self.assertEqual(len(M.check_beacons([(MOON_2, SYS_A, 0.0, 0.0, 0.0, 0)], moons, True)), 1)
        self.assertEqual(len(M.check_beacons([], moons, True)), 1)
        self.assertEqual(M.check_beacons([], moons, False), [])


class Metenox(unittest.TestCase):
    def test_the_metenox_row_reads_the_component(self):
        comp = {M.TYPE_UPWELL_AUTO_MOON_MINER: {"autoMoonMiner": {
            "miningCycleTime": 3600, "reagentsConsumedPerCycle": 7, "miningEfficiency": 0.5,
            "zeta": 1, "alpha": 2}}}
        rows, problems = M.derive_metenox(comp)
        self.assertEqual(problems, [])
        self.assertEqual(rows, [(M.TYPE_UPWELL_AUTO_MOON_MINER, 3600, M.TYPE_COLONY_REAGENT_LAVA,
                                 7, 0.5, "alpha=2;zeta=1")])

    def test_a_missing_component_is_a_problem(self):
        rows, problems = M.derive_metenox({})
        self.assertEqual(rows, [])
        self.assertEqual(len(problems), 1)


class Bands(unittest.TestCase):
    def test_security_bands(self):
        self.assertEqual(M.security_band(0.95), "high")
        self.assertEqual(M.security_band(0.3), "low")
        self.assertEqual(M.security_band(-0.2), "null")


if __name__ == "__main__":
    unittest.main()
