"""Unit tests for `tools/content_conquest.py` (CQ-1, #291).

Invented ids, names and rows only; no [A] value is asserted. Run:
`python3 -I tools/test_content_conquest.py -q`.
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

import content_conquest as C  # noqa: E402

AMARR, CALDARI = C.FACTIONS["amarr"], C.FACTIONS["caldari"]


def emitter(coll, faction="AMARR", interval=30, duration=120):
    return {"dbuffCollections": coll, "filterFunction": "FW_%s_ENLISTED" % faction,
            "duration": duration, "interval": interval}


class Emitters(unittest.TestCase):
    def test_an_emitter_with_an_empty_dict_writes_no_row(self):
        comp = {7001: {"systemDbuffEmitter": emitter({})}}
        types = {7001: (C.GROUP_EMITTER, "SFW Amarr Conquest Track System Dbuff Emitter 3")}
        self.assertEqual(C.derive_emitters(comp, types), ([], []))

    def test_an_sfw_emitter_writes_one_row_per_collection(self):
        comp = {7002: {"systemDbuffEmitter": emitter({"9002": -4.0, "9001": 2.5}, interval=10)}}
        types = {7002: (C.GROUP_EMITTER, "SFW Amarr Conquest Track System Dbuff Emitter 12 - Invented")}
        rows, notes = C.derive_emitters(comp, types)
        self.assertEqual(notes, [])
        self.assertEqual(rows, [(7002, "sfw", AMARR, 12, "FW_AMARR_ENLISTED", 10, 120, 9001, 2.5),
                                (7002, "sfw", AMARR, 12, "FW_AMARR_ENLISTED", 10, 120, 9002, -4.0)])
        self.assertEqual(len(rows[0]), len(C.EMITTERS_HEADER))

    def test_the_engine_hitpoints_step_adds_engine_hp_rows(self):
        comp = {7003: {"systemDbuffEmitter": emitter({"9003": 1.0}, "CALDARI"),
                       "systemWideEffects": {"globalDebuffs": {"dbuffs": {"9100": 7.0},
                                                               "eligibleTypeListID": 4242}}}}
        types = {7003: (C.GROUP_EMITTER, "SFW Caldari Conquest Track System Dbuff Emitter 5")}
        rows, _ = C.derive_emitters(comp, types)
        self.assertEqual([r[1] for r in rows], ["sfw", "engine_hp"])
        self.assertEqual(rows[1], (7003, "engine_hp", CALDARI, 5, "typelist=4242", 0, 0, 9100, 7.0))

    def test_a_ptc_effect_uses_the_career_number_as_its_step(self):
        comp = {7004: {"systemWideEffects": {"globalDebuffs": {"dbuffs": {"9200": 3.0},
                                                               "eligibleTypeListID": 11}}}}
        types = {7004: (C.GROUP_EMITTER, "PTC - Caldari Soldier System Wide Effect - Invented")}
        rows, _ = C.derive_emitters(comp, types)
        self.assertEqual(rows, [(7004, "ptc", CALDARI, 4, "typelist=11", 0, 0, 9200, 3.0)])

    def test_a_general_is_read_by_group_and_its_filter(self):
        comp = {7005: {"systemDbuffEmitter": emitter({"9300": 20.0}, "CALDARI")}}
        types = {7005: (C.GENERAL_GROUPS[0], "Invented General")}
        rows, _ = C.derive_emitters(comp, types)
        self.assertEqual(rows, [(7005, "general", CALDARI, 0, "FW_CALDARI_ENLISTED", 30, 120, 9300, 20.0)])

    def test_a_faction_that_disagrees_with_its_filter_is_noted_and_skipped(self):
        comp = {7006: {"systemDbuffEmitter": emitter({"9400": 1.0}, "CALDARI")}}
        types = {7006: (C.GROUP_EMITTER, "SFW Amarr Conquest Track System Dbuff Emitter 2")}
        rows, notes = C.derive_emitters(comp, types)
        self.assertEqual(rows, [])
        self.assertEqual(len(notes), 1)

    def test_a_type_outside_the_emitter_groups_is_ignored(self):
        comp = {7007: {"systemDbuffEmitter": emitter({"9500": 1.0})}}
        self.assertEqual(C.derive_emitters(comp, {7007: (1, "SFW Amarr Conquest Track System Dbuff Emitter 1")}),
                         ([], []))


class Recipes(unittest.TestCase):
    COMP = {8001: {"itemTrader": {"recipes": [31, 31, 32, 33], "interactionRange": 500}}}
    RECIPES = {31: {"groupID": "INVENTED", "inputItems": {"9": 3, "8": 2}, "outputItems": {"7": 1},
                    "inputIsk": 0},
               32: {"groupID": None, "inputItems": {"6": 4}, "outputItems": {"5": 2, "4": 1},
                    "inputIsk": 99}}

    def test_a_recipe_with_two_inputs_writes_two_in_rows(self):
        rows, _ = C.derive_recipes(self.COMP, self.RECIPES)
        ins = [r for r in rows if r[0] == 31 and r[3] == "in"]
        self.assertEqual(ins, [(31, 8001, "INVENTED", "in", 8, 2, 0), (31, 8001, "INVENTED", "in", 9, 3, 0)])
        self.assertEqual([r for r in rows if r[0] == 31 and r[3] == "out"],
                         [(31, 8001, "INVENTED", "out", 7, 1, 0)])

    def test_a_listed_twice_recipe_is_written_once_and_isk_rides_on_each_row(self):
        rows, _ = C.derive_recipes(self.COMP, self.RECIPES)
        self.assertEqual(len([r for r in rows if r[0] == 31]), 3)
        self.assertTrue(all(r[6] == 99 and r[2] == "" for r in rows if r[0] == 32))

    def test_a_missing_recipe_is_noted_and_writes_nothing(self):
        rows, notes = C.derive_recipes(self.COMP, self.RECIPES)
        self.assertFalse([r for r in rows if r[0] == 33])
        self.assertEqual(len(notes), 1)

    def test_a_type_without_a_trader_component_writes_nothing(self):
        self.assertEqual(C.derive_recipes({1: {"systemDbuffEmitter": emitter({})}}, self.RECIPES), ([], []))


class Traders(unittest.TestCase):
    COMP = {8100: {"itemTrader": {"recipes": [], "interactionRange": 250}}}
    TYPES = {8100: (C.GROUP_TRADER, "Invented Trader")}

    def test_a_dungeon_without_a_faction_takes_the_traders_faction(self):
        rows, notes = C.derive_traders(self.COMP, self.TYPES, {1: AMARR, 2: 0},
                                       [(1, 8100, 1.0, 2.0, 3.0), (2, 8100, 4.0, 5.0, 6.0)])
        self.assertEqual(notes, [])
        self.assertEqual(rows, [(8100, AMARR, 1, 250, 1.0, 2.0, 3.0), (8100, AMARR, 2, 250, 4.0, 5.0, 6.0)])

    def test_a_trader_in_no_dungeon_is_noted(self):
        rows, notes = C.derive_traders(self.COMP, self.TYPES, {}, [])
        self.assertEqual((rows, len(notes)), ([], 1))

    def test_a_trader_outside_its_group_is_not_placed(self):
        rows, _ = C.derive_traders(self.COMP, {8100: (1, "x")}, {1: AMARR}, [(1, 8100, 0.0, 0.0, 0.0)])
        self.assertEqual(rows, [])

    def test_two_factions_for_one_trader_are_noted(self):
        _, notes = C.derive_traders(self.COMP, self.TYPES, {1: AMARR, 2: CALDARI},
                                    [(1, 8100, 0.0, 0.0, 0.0), (2, 8100, 0.0, 0.0, 0.0)])
        self.assertEqual(len(notes), 1)


class Texts(unittest.TestCase):
    def test_a_missing_label_writes_no_row_and_one_note(self):
        labels = {"Campaigns/2026_ptc/CampaignData/Amarr/Title": 11,
                  "Campaigns/2026_ptc/CampaignData/Amarr/Description": 12}
        rows, notes = C.derive_texts(labels)
        self.assertEqual(rows, [(AMARR, "title", 11), (AMARR, "goal_description", 12),
                                (AMARR, "briefing_goal_description_message_id", 12)])
        self.assertIn("no label Campaigns/2026_ptc/CampaignData/Amarr/CampaignSubtitle", notes)
        self.assertNotIn("no label Campaigns/2026_ptc/CampaignData/Amarr/Title", notes)

    def test_objective_labels_are_keyed_by_kind(self):
        labels = {"Campaigns/2026_ptc/ObjectiveData/Titles/Caldari/ptc_caldari_pvp": 21,
                  "Campaigns/2026_ptc/ObjectiveData/Descriptions/Caldari/ptc_caldari_pvp_description": 22}
        rows, _ = C.derive_texts(labels)
        self.assertEqual(rows, [(CALDARI, "objective_pvp_title", 21), (CALDARI, "objective_pvp_description", 22)])

    def test_no_labels_writes_nothing(self):
        rows, notes = C.derive_texts({})
        self.assertEqual(rows, [])
        self.assertTrue(notes)


class Textures(unittest.TestCase):
    P = C.TEXTURE_PREFIXES[0]

    def test_a_faction_token_becomes_the_faction_column(self):
        rows = C.derive_textures([self.P + "overview/ambient_amarr.png", self.P + "overview/middleground_caldari_2.png",
                                  self.P + "landing.png", "res:/elsewhere/x_amarr.png"])
        self.assertEqual(rows, [(0, "2026_q4/landing", self.P + "landing.png"),
                                (AMARR, "2026_q4/overview/ambient", self.P + "overview/ambient_amarr.png"),
                                (CALDARI, "2026_q4/overview/middleground_2", self.P + "overview/middleground_caldari_2.png")])

    def test_a_file_named_only_by_its_faction_keys_to_its_folder(self):
        self.assertEqual(C.derive_textures([self.P + "card/gallente.png"]),
                         [(C.FACTIONS["gallente"], "2026_q4/card", self.P + "card/gallente.png")])


class Npcs(unittest.TestCase):
    def test_role_and_faction_come_from_the_group_and_type_names(self):
        from unittest import mock
        types = {1: (6001, "Invented"), 2: (6002, "Republic Conquest Engine"),
                 3: (6003, "Invented"), 4: (6004, "Invented"), 5: (6009, "Invented")}
        groups = {6001: "FW Amarr Empire General", 6002: "Conquest Engine",
                  6003: "Strange Group", 6004: "Plain General", 6009: "FW Amarr Empire General"}
        with mock.patch.object(C, "NPC_GROUPS", (6001, 6002, 6003, 6004)):
            rows, notes = C.derive_npcs(types, groups)
        self.assertEqual(rows, [(1, 6001, "general", AMARR), (2, 6002, "engine", C.FACTIONS["minmatar"]),
                                (4, 6004, "general", 0)])
        self.assertEqual(len(notes), 2)  # 3: no role; 4: no faction; 5 is outside the groups

    def test_texture_sources_are_the_listed_files(self):
        self.assertEqual(C.TEXTURE_SOURCES([(0, "k", "res:/b"), (0, "j", "res:/a"), (1, "l", "res:/a")]),
                         ["res:/a", "res:/b"])


def season(**over):
    rows = {
        "season_mode": ("0", "-", "rolling"), "season_days": ("0", "-", "10"), "ptc_days": ("0", "-", "5"),
        "siege_start_day": ("0", "-", "9"), "siege_hours": ("0", "-", "1"),
        "siege_selection": ("0", "-", "most_wp"), "ptc_scope": ("0", "-", "ptc_only"),
        "client_patch": ("0", "-", "none"), "trader_access": ("0", "-", "militia"),
        "engine_shooters": ("0", "-", "anyone"), "tokens_count_as_earned": ("0", "-", "1"),
        "per_pilot_cap_pct": ("0", "-", "50"), "commit_limit": ("0", "-", "2"),
        "trader_multiplier_cap": ("0", "-", "9"), "wp_step": ("0", "-", "7"),
    }
    lines = ["# c", C.AUTHORED_HEADER]
    for k, (f, kind, v) in rows.items():
        lines.append("\t".join([k, f, kind, v, "why"]))
    for k in C.OBJECTIVE_KINDS:
        lines.append("\t".join(["target_multiplier", "0", k, "3", "why"]))
    for name, fid in C.FACTIONS.items():
        lines.append("\t".join(["war_hq_system", str(fid), "-", "1", "why"]))
        lines.append("\t".join(["war_hq_station", str(fid), "-", "2", "why"]))
    lines.extend(over.get("extra", []))
    return lines


class Season(unittest.TestCase):
    def test_a_valid_table_parses_with_whole_numbers(self):
        rows = C.parse_season(season())
        self.assertEqual(next(r for r in rows if r["key"] == "season_days")["value"], 10)

    def test_the_committed_table_parses(self):
        path = os.path.join(os.path.dirname(_HERE), "content", "conquest", "season.tsv")
        with open(path, encoding="utf-8") as fh:
            C.parse_season(fh.read().splitlines())

    def test_refusals(self):
        bad = {
            "wrong header": ["key\tvalue"],
            "unknown key": season(extra=["nope\t0\t-\t1\twhy"]),
            "empty why": season(extra=["wp_kill\t0\t-\t1\t "]),
            "duplicate": season(extra=["wp_step\t0\t-\t8\twhy"]),
            "bad number": season(extra=["wp_kill\t0\t-\tx\twhy"]),
            "below minimum": season(extra=["wp_step\t0\t-\t0\twhy"]) ,
            "bad enum": season(extra=["ptc_scope\t0\t-\tall\twhy"]),
            "faction on a global key": season(extra=["wp_kill\t500003\t-\t1\twhy"]),
            "bad kind": season(extra=["target_multiplier\t0\tpvx\t1\twhy"]),
            "short row": season(extra=["wp_kill\t0\t-"]),
        }
        for name, lines in bad.items():
            with self.subTest(name), self.assertRaises(C.ContentError):
                C.parse_season(lines)

    def test_every_alternative_of_an_owner_choice_key_parses(self):
        for key in ("siege_selection", "ptc_scope", "client_patch", "trader_access", "engine_shooters"):
            alts = C.KEYS[key][1]
            self.assertGreaterEqual(len(alts), 2, key)
            for alt in alts:
                lines = [l.replace("\t" + "x" + "\t", "") for l in season()]
                lines = ["\t".join([key, "0", "-", alt, "why"]) if l.startswith(key + "\t") else l
                         for l in lines]
                with self.subTest(key=key, alt=alt):
                    got = next(r for r in C.parse_season(lines) if r["key"] == key)
                    self.assertEqual(got["value"], alt)

    def test_a_missing_required_key_is_refused(self):
        lines = [l for l in season() if not l.startswith("commit_limit")]
        with self.assertRaises(C.ContentError):
            C.parse_season(lines)

    def test_a_missing_faction_row_is_refused(self):
        lines = season()
        first = next(l for l in lines if l.startswith("war_hq_system\t"))
        lines = [l for l in lines if l != first]
        with self.assertRaises(C.ContentError):
            C.parse_season(lines)


if __name__ == "__main__":
    unittest.main()
