"""`content_achievements.derive` and `find_definitions` on invented data."""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import content_achievements as ca  # noqa: E402

METHOD = "m-agent"


def tree(achievements, method=None):
    return {
        "methods": {METHOD: method or {"params": ["agent_division", "agent_level", "mission"],
                                       "set_of": "mission"},
                    "m-other": {"params": ["solar_system"], "set_of": None}},
        "career": {"id": "root", "name": "career", "achievements": [],
                   "categories": [{"id": "cat-a", "name": "a", "categories": [],
                                   "achievements": achievements}]},
    }


def ach(aid, division, milestones, method=METHOD, levels=()):
    params = {"agent_division": {"division": [str(division)]}}
    if levels:
        params["agent_level"] = {"level": [str(l) for l in levels]}
    return {"id": aid, "name": "invented_" + aid, "method": method, "params": params,
            "milestones": milestones}


def ms(mid, target, points, rewards):
    return {"id": mid, "name": "n_" + mid, "target": target, "points": points, "rewards": rewards}


class Derive(unittest.TestCase):
    def test_one_row_per_reward_in_bundle_column_order(self):
        t = tree([ach("a1", 98, [ms("m1", 1, 7, [{"id": "r1", "kind": "ISK", "amount": 111,
                                                  "type_id": None}]),
                                 ms("m2", 2, 7, [{"id": "r2", "kind": "ISK", "amount": 222,
                                                  "type_id": None},
                                                 {"id": "r3", "kind": "ITEM", "amount": 1,
                                                  "type_id": 990001}])])])
        rows = ca.derive(t)
        self.assertEqual(len(rows[0]), len(ca.HEADER))
        self.assertEqual(rows, [
            ("a1", "invented_a1", "cat-a", "98", "", "m1", "n_m1", 1, 7, "r1", "ISK", 111, ""),
            ("a1", "invented_a1", "cat-a", "98", "", "m2", "n_m2", 2, 7, "r2", "ISK", 222, ""),
            ("a1", "invented_a1", "cat-a", "98", "", "m2", "n_m2", 2, 7, "r3", "ITEM", 1, 990001),
        ])

    def test_other_contribution_methods_are_left_out(self):
        t = tree([ach("a1", 98, [ms("m1", 1, 7, [])], method="m-other")])
        self.assertEqual(ca.derive(t), [])

    def test_a_cumulative_agent_division_method_counts_too(self):
        # definitions 1.8.0: the career achievements count completions, not a set
        cum = {"params": ["agent_division", "agent_type", "agent_level", "agent_faction"],
               "set_of": None, "mode": ca.PROGRESS_CUMULATIVE}
        t = tree([ach("a1", 98, [ms("m1", 1, 7, [])])], method=cum)
        self.assertEqual([r[0] for r in ca.derive(t)], ["a1"])

    def test_a_cumulative_method_without_a_division_is_left_out(self):
        cum = {"params": ["solar_system"], "set_of": None, "mode": ca.PROGRESS_CUMULATIVE}
        t = tree([ach("a1", 98, [ms("m1", 1, 7, [])])], method=cum)
        self.assertEqual(ca.derive(t), [])

    def test_a_cumulative_achievement_without_a_division_filter_is_left_out(self):
        cum = {"params": ["agent_division", "agent_type", "agent_level"],
               "set_of": None, "mode": ca.PROGRESS_CUMULATIVE}
        a = ach("a1", 98, [ms("m1", 1, 7, [])])
        a["params"] = {"agent_type": {"agent_type": ["8"]}}
        self.assertEqual(ca.derive(tree([a], method=cum)), [])

    def test_levels_are_kept(self):
        t = tree([ach("a1", 98, [ms("m1", 1, 7, [])], levels=(1, 2))])
        self.assertEqual(ca.derive(t)[0][4], "1,2")

    def test_an_agent_mission_achievement_with_no_division_is_refused(self):
        a = ach("a1", 98, [ms("m1", 1, 7, [])])
        a["params"] = {}
        with self.assertRaises(ca.ContentError):
            ca.derive(tree([a]))

    def test_no_definitions_is_no_rows(self):
        self.assertEqual(ca.derive(None), [])


class Find(unittest.TestCase):
    def test_the_newest_version_wins_and_the_env_overrides(self):
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for name, ver in (("old", "1.2.0"), ("new", "1.10.1")):
                os.makedirs(os.path.join(d, name))
                p = os.path.join(d, name, "data.bin")
                open(p, "wb").write(b"x")
                open(os.path.join(d, name, "version.txt"), "w").write(ver)
                paths.append(p)
            pattern = os.path.join(d, "*", "data.bin")
            nolocal = os.path.join(d, "nolocal", "data.bin")
            self.assertEqual(ca.find_definitions({}, pattern, nolocal), (paths[1], "1.10.1"))
            self.assertEqual(ca.find_definitions({ca.ENV: paths[0]}, pattern, nolocal),
                             (paths[0], "1.2.0"))
            self.assertIsNone(ca.find_definitions({}, os.path.join(d, "none", "*.bin"), nolocal))

    def test_the_out_copy_is_found_and_competes_on_version(self):
        with tempfile.TemporaryDirectory() as d:
            def put(name, ver):
                os.makedirs(os.path.join(d, name))
                p = os.path.join(d, name, "data.bin")
                open(p, "wb").write(b"x")
                open(os.path.join(d, name, "version.txt"), "w").write(ver)
                return p
            local = put("out", "1.8.0")
            none = os.path.join(d, "none", "*.bin")
            self.assertEqual(ca.find_definitions({}, none, local), (local, "1.8.0"))
            cache = put("cache", "1.9.0")
            self.assertEqual(ca.find_definitions({}, os.path.join(d, "cache", "data.bin"), local),
                             (cache, "1.9.0"))

    def test_a_file_without_the_identifier_is_refused(self):
        with self.assertRaises(ca.ContentError):
            ca.read_definitions(b"\x08\x00\x00\x00NOPE")


if __name__ == "__main__":
    unittest.main()
