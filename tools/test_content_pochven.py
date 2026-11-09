"""Unit tests for `tools/content_pochven.py` (PV-4). Invented rows only.

Run: `python3 -I tools/test_content_pochven.py -q`.
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

import content_pochven as P  # noqa: E402

FAC, OTHER_FAC, WRECK = 599026, 599027, 990705
SHIP_G, SHIP_D = 990701, 990702


def trow(tid, group, name, category=11, effects="1,2", published="1", attrs="9=1.5"):
    f = [""] * 14
    f[0], f[1], f[2], f[3], f[4] = "type", str(tid), str(group), str(category), published
    f[11], f[12], f[13] = effects, attrs, name
    return f


def mrow(tid, faction, wreck=""):
    f = [""] * 12
    f[0], f[9], f[11] = str(tid), str(faction), str(wreck)
    return f


def needs_of(attrs, effects):
    return ["TURRET"] if effects else []


def role_of(needs):
    return "DPS"


HULLS = [trow(SHIP_G, 25, "Glorp", category=6), trow(SHIP_D, 420, "Blarf", category=6)]


def run(rows, market, faction=FAC):
    return P.derive(HULLS + rows, market, needs_of, role_of, faction)


def data(lines):
    return [l.split("\t") for l in lines[len(P.COMMENTS) + 1:]]


class Derive(unittest.TestCase):
    def test_class_rank_wreck_and_header(self):
        lines = run([trow(990711, 990, "Zed Glorp"), trow(990712, 990, "Big Zed Blarf")],
                    [mrow(990711, FAC, WRECK), mrow(990712, FAC)])
        self.assertEqual(lines[:len(P.COMMENTS)], list(P.COMMENTS))
        self.assertEqual(lines[len(P.COMMENTS)], P.HEADER)
        rows = data(lines)
        self.assertEqual(rows[0][:7], ["990711", "990", "Frigate", "Zed", "TURRET", "DPS", str(WRECK)])
        self.assertEqual(rows[1][2:4], ["Destroyer", "Big Zed"])
        self.assertEqual(rows[1][6], "")

    def test_battlecruiser_is_spelled_as_the_server_does(self):
        hulls = [trow(5, 419, "Bcglorp", category=6)]
        lines = P.derive(hulls + [trow(990713, 990, "Zed Bcglorp")], [mrow(990713, FAC)],
                         needs_of, role_of, FAC)
        self.assertEqual(data(lines)[0][2], "BattleCruiser")

    def test_unmatched_last_word_is_dropped(self):
        self.assertEqual(data(run([trow(990711, 990, "Zed Nothull")], [mrow(990711, FAC)])), [])

    def test_ttc_and_test_names_are_dropped(self):
        rows = [trow(990711, 990, "TTC-Zed Glorp"), trow(990712, 990, "Test Glorp")]
        self.assertEqual(data(run(rows, [mrow(990711, FAC), mrow(990712, FAC)])), [])

    def test_no_effects_is_dropped(self):
        self.assertEqual(data(run([trow(990711, 990, "Zed Glorp", effects="")],
                                  [mrow(990711, FAC)])), [])

    def test_other_faction_and_other_category_are_dropped(self):
        rows = [trow(990711, 990, "Zed Glorp"), trow(990712, 990, "Zed Glorp", category=6)]
        self.assertEqual(data(run(rows, [mrow(990711, OTHER_FAC), mrow(990712, FAC)])), [])

    def test_unpublished_or_ambiguous_player_ship_is_no_class(self):
        hulls = [trow(1, 25, "Glorp", category=6, published="0"),
                 trow(2, 420, "Blarf", category=6), trow(3, 26, "Blarf", category=6)]
        self.assertEqual(P.ship_classes(hulls), {})

    def test_market_facts(self):
        self.assertEqual(P.market_facts([mrow(5, FAC, WRECK), ["x"], mrow(6, "")]),
                         ({5: FAC}, {5: WRECK}))


if __name__ == "__main__":
    unittest.main()
