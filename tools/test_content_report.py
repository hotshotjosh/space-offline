#!/usr/bin/env python3
"""Tests for tools/content-report.py on two invented bundles and an invented notes outline.
No client byte is read; every id and name here is made up.

    python3 -I tools/test_content_report.py
"""
from __future__ import annotations

import contextlib
import hashlib
import importlib.util
import io
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("content_report", os.path.join(HERE, "content-report.py"))
cr = importlib.util.module_from_spec(_spec)
sys.modules["content_report"] = cr
_spec.loader.exec_module(cr)


def write_bundle(root, files, build, extractor="aa"):
    os.makedirs(root, exist_ok=True)
    recs = []
    for rel, rows in files.items():
        p = os.path.join(root, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        text = "\n".join(rows) + "\n"
        with open(p, "w", encoding="utf-8") as fh:
            fh.write(text)
        recs.append("file\t%s\t%d\t%s\tres:/x" % (rel, len(rows), hashlib.sha256(text.encode()).hexdigest()))
    with open(os.path.join(root, "MANIFEST.tsv"), "w", encoding="utf-8") as fh:
        fh.write("# invented\nformat\t1.4\nclient_build\t%s\nextractor\t%s\n" % (build, extractor))
        fh.write("\n".join(recs) + "\n")


def typ(i, group, cat, pub, name, attrs="100=1.0"):
    return "type\t%s\t%s\t%s\t%s\t1\t1.0\t0\t0\t0\t0\t\t%s\t%s" % (i, group, cat, pub, attrs, name)


def base():
    return {
        "types/typedata.tsv": [
            typ("910001", "9101", "91", "1", "Quartzite Rock"),
            typ("910002", "9101", "91", "1", "Mica Rock"),
            typ("910003", "9101", "91", "1", "Gone Widget"),
            typ("910004", "9101", "91", "1", "Flip Widget"),
            typ("910005", "9101", "91", "1", "Old Name"),
            "attribute\t9201\t0.0\tmass",
            "effect\t9301\t1\t0\t73\t6\t0\t0\t0\tfx\teffects.Known\t1\t0",
            "modifier\t9301\tshipID\tItemModifier\t9201\t9201\t2\t\t",
        ],
        "dungeons/archetypes.tsv": ["#archetype_id\ttitle_id\tcontent_tags\tlocation", "1\t\t\t"],
        "dungeons/dungeons.tsv": ["#dungeon_id\tarchetype_id", "9401\t1", "9402\t1"],
        "dungeons/spawns.tsv": ["#dungeon_id\troom_id", "9401\t1", "9402\t1"],
        "npc/components.tsv": ["#type_id\tcomponent", "910001\tcompA", "910002\tcompB"],
        "industry/blueprints.tsv": ["#bp_type_id\tactivity_id\tkind\ttype_id\tquantity", "910100\t1\tmaterial\t910001\t5"],
        "universe/systems.tsv": ["#system_id\tregion_id\tconstellation_id\tfaction_id\towner_faction\tname",
                                 "9501\t9601\t9701\t9801\t9801\tOldburg", "9502\t9601\t9701\t9801\t9801\tSteady"],
        "npc/corporations.tsv": ["#corp_id\tfaction_id\tname", "9901\t9801\tAcme Corp"],
        "agents/agents.tsv": ["#agent_id\tcorp_id", "9951\t9901"],
        "econ/prices.tsv": ["#type_id\tref_price", "910001\t10.0", "910002\t20.0"],
        "types/materials.tsv": ["#type_id\tmaterial_type_id\tquantity", "910002\t910001\t5"],
        "skins/skins.tsv": ["#skin_id\tname", "9961\tPlain"],
    }


def new():
    b = base()
    b["types/typedata.tsv"] = [
        typ("910001", "9101", "91", "1", "Quartzite Rock"),
        typ("910002", "9101", "91", "1", "Mica Rock"),
        typ("910004", "9101", "91", "0", "Flip Widget"),
        typ("910005", "9101", "91", "1", "Fresh Name"),
        typ("910006", "9102", "91", "1", "Aberrant Gizmo", "100=1.0;%s=777.0" % cr.META_ATTR),
        typ("910007", "9101", "91", "1", "Gizmo Rock"),
        "attribute\t9201\t0.0\tmass",
        "attribute\t9202\t0.0\tnewattr",
        "effect\t9301\t1\t0\t73\t6\t0\t0\t0\tfx\teffects.Known\t1\t0",
        "effect\t9302\t1\t0\t73\t6\t0\t0\t0\tfx2\teffects.PyOnly\t1\t0",
        "modifier\t9301\tshipID\tItemModifier\t9201\t9201\t2\t\t",
        "modifier\t9303\tshipID\tZzbrandNewFunc\t9201\t9201\t2\t\t",
    ]
    b["dungeons/archetypes.tsv"] = ["#archetype_id\ttitle_id\tcontent_tags\tlocation", "1\t\t\t", "2\t\t\t"]
    b["dungeons/dungeons.tsv"] = ["#dungeon_id\tarchetype_id", "9401\t1", "9402\t1", "9403\t2"]
    b["dungeons/spawns.tsv"] = ["#dungeon_id\troom_id", "9401\t1"]
    b["npc/components.tsv"] = ["#type_id\tcomponent", "910001\tcompA", "910006\tnewComp"]
    b["industry/blueprints.tsv"] = ["#bp_type_id\tactivity_id\tkind\ttype_id\tquantity",
                                    "910100\t1\tmaterial\t910001\t5", "910101\t1\tmaterial\t910099\t2"]
    b["universe/systems.tsv"] = ["#system_id\tregion_id\tconstellation_id\tfaction_id\towner_faction\tname",
                                 "9501\t9601\t9701\t9801\t9801\tNewburg", "9502\t9601\t9701\t9801\t9801\tSteady"]
    b["npc/corporations.tsv"] = ["#corp_id\tfaction_id\tname", "9901\t9802\tAcme Corp"]
    b["agents/agents.tsv"] = ["#agent_id\tcorp_id", "9951\t9901", "9952\t9901"]
    b["types/materials.tsv"] = ["#type_id\tmaterial_type_id\tquantity", "910002\t910001\t5", "910006\t910098\t3"]
    b["skins/skins.tsv"] = ["#skin_id\tname", "9961\tPlain", "9962\tShiny"]
    b["brandnew/table.tsv"] = ["#id\tx", "1\t2"]
    return b


class Base(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.d = self.tmp.name
        self.old = os.path.join(self.d, "old")
        self.new = os.path.join(self.d, "new")
        write_bundle(self.old, base(), "1000")
        write_bundle(self.new, new(), "1001")
        self.root = os.path.join(self.d, "root")  # an empty tree: nothing reads any id
        os.makedirs(self.root)
        self.out = os.path.join(self.d, "out")

    def run_main(self, *extra, old=None, new_=None):
        argv = [old or self.old, new_ or self.new, "--out", self.out, "--root", self.root, *extra]
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            code = cr.main(argv)
        return code, err.getvalue()

    def intake(self):
        with open(os.path.join(self.out, "intake.tsv"), encoding="utf-8") as fh:
            lines = [l.rstrip("\n").split("\t") for l in fh]
        head = lines[0]
        return [dict(zip(head, l)) for l in lines[1:]]

    def find(self, rows, kind):
        hits = [r for r in rows if r["kind"] == kind]
        self.assertEqual(len(hits), 1, "%s in %s" % (kind, [r["kind"] for r in rows]))
        return hits[0]


class Sections(Base):
    def test_exit_and_files(self):
        code, _ = self.run_main()
        self.assertEqual(code, 3)
        self.assertTrue(os.path.isfile(os.path.join(self.out, "content-report.md")))
        rows = self.intake()
        self.assertTrue(all(len(r) == 9 for r in rows))
        self.assertEqual(list(rows[0]), list(cr.COLS))

    def test_equal_bundles_exit_zero(self):
        write_bundle(self.new, base(), "1001")
        self.assertEqual(self.run_main()[0], 0)

    def test_only_data_driven_exit_one(self):
        b = base()
        b["skins/skins.tsv"] = ["#skin_id\tname", "9961\tPlain", "9962\tShiny"]
        write_bundle(self.new, b, "1001")
        self.assertEqual(self.run_main()[0], 1)
        self.assertEqual(self.find(self.intake(), "skins")["verdict"], "data-driven")

    def test_types(self):
        self.run_main()
        rows = self.intake()
        self.assertEqual(self.find(rows, "added")["verdict"], "data-driven")
        self.assertEqual(self.find(rows, "added")["count"], "2")
        self.assertEqual(self.find(rows, "new-group")["sample_ids"], "9102")
        self.assertEqual(self.find(rows, "new-group")["verdict"], "needs-work")
        self.assertEqual(self.find(rows, "new-metagroup")["sample_ids"], "777")
        self.assertEqual(self.find(rows, "removed")["sample_ids"], "910003")
        self.assertEqual(self.find(rows, "published-flip")["sample_ids"], "910004")
        self.assertEqual(self.find(rows, "renamed")["sample_ids"], "910005")

    def test_dogma(self):
        self.run_main()
        rows = self.intake()
        self.assertIn("ZzbrandNewFunc", self.find(rows, "modifier-func")["what"])
        self.assertEqual(self.find(rows, "modifier-func")["verdict"], "needs-work")
        self.assertEqual(self.find(rows, "python-effect")["sample_ids"], "9302")
        self.assertEqual(self.find(rows, "attribute-counts")["verdict"], "data-driven")

    def test_known_modifier_func_in_source_is_not_flagged(self):
        src = os.path.join(self.root, "server", "crates", "x")
        os.makedirs(src)
        with open(os.path.join(src, "a.rs"), "w") as fh:
            fh.write('match f { "ZzbrandNewFunc" => 1 }\n')
        self.run_main()
        self.assertFalse([r for r in self.intake() if r["kind"] == "modifier-func"])

    def test_dungeons(self):
        self.run_main()
        rows = self.intake()
        self.assertEqual(self.find(rows, "new-archetype")["sample_ids"], "2")
        self.assertEqual(self.find(rows, "lost-spawns")["sample_ids"], "9402")
        self.assertEqual(self.find(rows, "lost-spawns")["verdict"], "needs-work")

    def test_npcs(self):
        self.run_main()
        rows = self.intake()
        self.assertIn("newComp", self.find(rows, "new-component")["what"])
        self.assertIn("compB", self.find(rows, "component-zero")["what"])
        self.assertEqual(self.find(rows, "component-zero")["verdict"], "needs-work")

    def test_industry_and_lp(self):
        self.run_main()
        rows = self.intake()
        self.assertEqual(self.find(rows, "unpriced-material")["sample_ids"], "910099")
        self.assertEqual(self.find(rows, "unpriced-output")["sample_ids"], "910098")

    def test_map(self):
        # The old name is named by an authored table, so the rename needs work.
        os.makedirs(os.path.join(self.root, "content"))
        with open(os.path.join(self.root, "content", "t.tsv"), "w") as fh:
            fh.write("Oldburg\t1\n")
        self.run_main()
        r = self.find(self.intake(), "renamed-used")
        self.assertEqual((r["sample_ids"], r["verdict"]), ("9501", "needs-work"))

    def test_map_rename_unused_is_data_driven(self):
        self.run_main()
        self.assertEqual(self.find(self.intake(), "systems-renamed")["verdict"], "data-driven")

    def test_agents(self):
        self.run_main()
        rows = self.intake()
        self.assertEqual(self.find(rows, "corp-faction")["sample_ids"], "9901")
        self.assertEqual(self.find(rows, "agents")["sample_ids"], "9952")
        self.assertEqual(self.find(rows, "agents")["verdict"], "needs-work")

    def test_new_tables(self):
        self.run_main()
        r = self.find(self.intake(), "new-files")
        self.assertIn("brandnew/table.tsv", r["what"])
        self.assertEqual(r["verdict"], "needs-work")


class Uses(Base):
    def test_removed_id_with_reader_needs_work(self):
        os.makedirs(os.path.join(self.root, "content"))
        with open(os.path.join(self.root, "content", "t.tsv"), "w") as fh:
            fh.write("910003\tx\n910004\ty\n")
        self.run_main("--uses")
        rows = self.intake()
        self.assertEqual(self.find(rows, "removed")["verdict"], "needs-work")
        self.assertIn("content/t.tsv", self.find(rows, "removed")["server_readers"])
        self.assertEqual(self.find(rows, "published-flip")["verdict"], "needs-work")

    def test_removed_id_without_reader_is_review(self):
        self.run_main("--uses")
        self.assertEqual(self.find(self.intake(), "removed")["verdict"], "review")
        self.assertEqual(self.find(self.intake(), "published-flip")["verdict"], "data-driven")


class Notes(Base):
    def write_notes(self, text):
        p = os.path.join(self.d, "notes.txt")
        with open(p, "w") as fh:
            fh.write(text)
        return p

    def test_join(self):
        p = self.write_notes("# Heading\nThe Aberrant Gizmo is added\nSomething about Zephyr Storms\n")
        self.run_main("--notes", p)
        rows = self.intake()
        self.assertEqual(self.find(rows, "added")["in_notes"], "yes")
        self.assertEqual(self.find(rows, "renamed")["in_notes"], "no")
        only = [r for r in rows if r["verdict"] == "notes-only"]
        self.assertEqual(len(only), 1)
        self.assertIn("Zephyr Storms", only[0]["what"])

    def test_only_a_whole_name_matches(self):
        p = self.write_notes("Gizmo Rocket launchers are new\nthe Fresh Names list grew\n")
        self.run_main("--notes", p)
        rows = self.intake()
        self.assertEqual(self.find(rows, "added")["in_notes"], "no")      # "Gizmo Rock" is not in "Gizmo Rocket"
        self.assertEqual(self.find(rows, "renamed")["in_notes"], "no")    # "Fresh Name" is not in "Fresh Names"

    def test_a_notes_only_row_copies_a_short_piece(self):
        long = "Zephyr Storms " + "word " * 40
        p = self.write_notes(long + "\nplain lowercase text that has no capitalised run in it at all, long enough to cut\n")
        self.run_main("--notes", p)
        only = [r for r in self.intake() if r["verdict"] == "notes-only"]
        self.assertEqual(len(only), 2)
        for r in only:
            self.assertLess(len(r["what"]), 100)
            self.assertLess(len(r["suggested_title"]), 100)
            self.assertNotIn("word word word", r["what"])

    def test_a_notes_only_row_alone_is_exit_one(self):
        write_bundle(self.new, base(), "1001")
        p = self.write_notes("Zephyr Storms arrive\n")
        self.assertEqual(self.run_main("--notes", p)[0], 1)

    def test_without_notes_in_notes_is_dash(self):
        self.run_main()
        self.assertTrue(all(r["in_notes"] == "-" for r in self.intake()))

    def test_candidates(self):
        self.assertEqual(cr.candidates("The Aberrant Gizmo and a rock"), ["Aberrant Gizmo"])
        self.assertEqual(cr.candidates("it is ok"), [])


class Refusals(Base):
    def test_extractor_mismatch_is_exit_two(self):
        write_bundle(self.new, new(), "1001", extractor="bb")
        code, err = self.run_main()
        self.assertEqual(code, 2)
        self.assertIn("extractors differ", err)
        self.assertFalse(os.path.exists(self.out))

    def test_mismatch_override_says_so_in_the_report_and_on_stdout(self):
        write_bundle(self.new, new(), "1001", extractor="bb")
        argv = [self.old, self.new, "--out", self.out, "--root", self.root, "--allow-extractor-mismatch"]
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(io.StringIO()):
            code = cr.main(argv)
        self.assertEqual(code, 3)
        self.assertIn("extractors differ", buf.getvalue())
        with open(os.path.join(self.out, "content-report.md")) as fh:
            self.assertIn("extractors differ", fh.read())

    def test_a_missing_extractor_is_a_mismatch(self):
        for root in (self.old, self.new):
            p = os.path.join(root, "MANIFEST.tsv")
            with open(p) as fh:
                lines = [l for l in fh if not l.startswith("extractor")]
            with open(p, "w") as fh:
                fh.writelines(lines)
        code, err = self.run_main()
        self.assertEqual(code, 2)
        self.assertIn("extractors differ", err)

    def test_an_agreeing_run_has_no_warning(self):
        self.run_main()
        with open(os.path.join(self.out, "content-report.md")) as fh:
            self.assertNotIn("extractors differ", fh.read())

    def test_missing_bundle_is_exit_two(self):
        self.assertEqual(self.run_main(old=os.path.join(self.d, "nope"))[0], 2)

    def test_missing_notes_is_exit_two(self):
        self.assertEqual(self.run_main("--notes", os.path.join(self.d, "nope.txt"))[0], 2)


if __name__ == "__main__":
    unittest.main()
