#!/usr/bin/env python3
"""Tests for tools/client-ci/fci-surface.py on invented bytecode trees and invented scenarios.
Every module, service and name here is made up; the .pyc writer is the one test_port_recheck.py
uses. Nothing needs a client.

    python3 -I tools/test_fci_surface.py
"""
import contextlib
import importlib.util
import io
import os
import shutil
import sys
import tempfile
import unittest

TOOLS = os.path.dirname(os.path.abspath(__file__))
HERE = os.path.join(TOOLS, "client-ci")
sys.path.append(TOOLS)


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


T = load("test_port_recheck", os.path.join(TOOLS, "test_port_recheck.py"))
fs = load("fci_surface", os.path.join(HERE, "fci-surface.py"))


def svc_class(name, guid, *methods):
    """A client service class: `__guid__ = guid` and the given methods."""
    return T.C(name, [(1, [T.op(T.LOAD_CONST, 0), T.op(T.STORE_NAME, 0), T.op(T.LOAD_CONST, 1), T.op(T.RETURN_VALUE)])],
               consts=(guid, None) + methods, names=("__guid__",), flags=0)


def old_modules():
    return {
        "demo/mod": T.module(nested=[T.fn("Thing", 5, 1), T.fn("Stable", 9, 1), T.fn("Retired", 12, 1)]),
        "demo/removed": T.module(nested=[T.fn("Anything", 3)]),
        "demo/svc": T.module(nested=[svc_class("DemoService", "svc.demoSvc", T.fn("Ping", 3, 1), T.fn("Calm", 4, 1),
                                               T.fn("Dropped", 5, 1)),
                                     svc_class("GoneService", "svc.goneSvc", T.fn("Foo", 3))]),
        "demo/names": T.module([(1, "shipui", 1), (2, "oldKeyName", 2), (3, "sharedKey", 3)]),
    }


def new_modules():
    m = old_modules()
    del m["demo/removed"]
    m["demo/mod"] = T.module(nested=[T.fn("Thing", 5, 2), T.fn("Stable", 9, 1)])
    m["demo/svc"] = T.module(nested=[svc_class("DemoService", "svc.demoSvc", T.fn("Ping", 3, 2), T.fn("Calm", 4, 1))])
    m["demo/names"] = T.module([(1, "shipui", 1), (3, "sharedKey", 3), (4, "freshKey", 4)])
    return m


class Base(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.mkdtemp(prefix="fcisurf-")
        self.addCleanup(shutil.rmtree, self.td)
        self.old, self.new = os.path.join(self.td, "old"), os.path.join(self.td, "new", "pyc")
        for root, mods in ((self.old, old_modules()), (self.new, new_modules())):
            for rel, code in mods.items():
                T.pyc(os.path.join(root, rel + ".pyc"), code)
        self.sdir = os.path.join(self.td, "ci", "scenarios")
        os.makedirs(self.sdir)

    def scenario(self, name, src):
        with open(os.path.join(self.sdir, name + ".py"), "w") as fh:
            fh.write(src)

    def run_tool(self, *extra, new=None):
        out = os.path.join(self.td, "out")
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
            rc = fs.main([self.old, new or self.new, "--scenarios", self.sdir, "--out", out] + list(extra))
        return rc, out

    def rows(self, out):
        with open(os.path.join(out, "fci-surface.tsv")) as fh:
            lines = [l.rstrip("\n").split("\t") for l in fh][1:]
        return [dict(zip(("scenario", "line", "ref", "status", "detail"), l)) for l in lines]

    def status(self, rows, ref):
        hits = [r["status"] for r in rows if r["ref"] == ref]
        self.assertEqual(len(hits), 1, "%s in %s" % (ref, [r["ref"] for r in rows]))
        return hits[0]


class Imports(Base):
    def test_ok_exits_zero_and_writes_both_files(self):
        self.scenario("one", "from demo.mod import Stable\nimport demo.mod\n")
        rc, out = self.run_tool()
        self.assertEqual(rc, 0)
        self.assertTrue(os.path.isfile(os.path.join(out, "fci-surface.md")))
        self.assertEqual({r["status"] for r in self.rows(out)}, {"ok"})

    def test_default_out_is_beside_a_pyc_dir(self):
        self.scenario("one", "import demo.mod\n")
        old = os.getcwd()
        os.chdir(self.td)
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                fs.main([self.old, self.new, "--scenarios", self.sdir])
        finally:
            os.chdir(old)
        self.assertTrue(os.path.isfile(os.path.join(self.td, "new", "fci-surface", "fci-surface.tsv")))

    def test_gone_module_name_and_changed_function(self):
        self.scenario("one", "import demo.removed\nfrom demo.mod import Retired, Thing, Stable\n")
        rc, out = self.run_tool()
        self.assertEqual(rc, 3)
        rows = self.rows(out)
        self.assertEqual(self.status(rows, "import demo.removed"), "gone")
        self.assertEqual(self.status(rows, "from demo.mod import Retired"), "gone")
        self.assertEqual(self.status(rows, "from demo.mod import Thing"), "changed")
        self.assertEqual(self.status(rows, "from demo.mod import Stable"), "ok")

    def test_a_module_neither_tree_has_is_not_client_code(self):
        self.scenario("one", "import os\nimport blue\nfrom os import path\n")
        rc, out = self.run_tool()
        self.assertEqual(rc, 0)
        self.assertEqual(self.rows(out), [])

    def test_a_name_neither_module_has_is_unresolved_and_exits_zero(self):
        self.scenario("one", "from demo.mod import Nothing\n")
        rc, out = self.run_tool()
        self.assertEqual(self.status(self.rows(out), "from demo.mod import Nothing"), "unresolved")
        self.assertEqual(rc, 0)

    def test_module_string(self):
        self.scenario("one", "x = 'demo.removed'\ny = 'demo.mod'\nz = 'not.a.module'\n")
        _, out = self.run_tool()
        rows = self.rows(out)
        self.assertEqual(self.status(rows, "module string demo.removed"), "gone")
        self.assertEqual(self.status(rows, "module string demo.mod"), "ok")
        self.assertFalse([r for r in rows if "not.a.module" in r["ref"]])


class Services(Base):
    def test_service_and_method(self):
        self.scenario("one", "a = sm.GetService('demoSvc').Ping(1)\nb = sm.GetService('demoSvc').Calm()\n"
                             "c = sm.GetService('demoSvc').Dropped()\n")
        rc, out = self.run_tool()
        rows = self.rows(out)
        self.assertEqual(rc, 3)
        self.assertEqual(self.status(rows, "service demoSvc"), "ok")
        self.assertEqual(self.status(rows, "service demoSvc.Ping"), "changed")
        self.assertEqual(self.status(rows, "service demoSvc.Calm"), "ok")
        self.assertEqual(self.status(rows, "service demoSvc.Dropped"), "gone")

    def test_gone_and_unknown_service(self):
        self.scenario("one", "a = sm.GetService('goneSvc')\nb = sm.StartService('nobodySvc')\nc = get_svc('demoSvc')\n")
        _, out = self.run_tool()
        rows = self.rows(out)
        self.assertEqual(self.status(rows, "service goneSvc"), "gone")
        self.assertEqual(self.status(rows, "service nobodySvc"), "unresolved")
        self.assertEqual(self.status(rows, "service demoSvc"), "ok")

    def test_remote_services_are_not_client_code(self):
        self.scenario("one", "a = sm.RemoteSvc('serverOnly')\nb = sm.ProxySvc('serverOnly')\n")
        _, out = self.run_tool()
        self.assertEqual(self.rows(out), [])


class Names(Base):
    def test_layer_and_settings(self):
        self.scenario("one", "a = uicore.layer.shipui.sharedKey\nb = uicore.layer.oldKeyName\n"
                             "c = settings.user.sharedKey.Get('freshKey', 1)\n")
        _, out = self.run_tool()
        rows = self.rows(out)
        self.assertEqual(self.status(rows, "uicore.layer.shipui.sharedKey"), "ok")
        self.assertEqual(self.status(rows, "uicore.layer.oldKeyName"), "gone")
        self.assertEqual([r["status"] for r in rows if r["ref"].startswith("settings.")], ["unresolved"])

    def test_a_dict_key_the_client_dropped_is_gone(self):
        self.scenario("one", "row = {}\nv = row['oldKeyName']\nw = row['sharedKey']\nx = row.get('mine')\n")
        _, out = self.run_tool()
        rows = self.rows(out)
        self.assertEqual(self.status(rows, "name oldKeyName"), "gone")
        self.assertFalse([r for r in rows if r["ref"] in ("name sharedKey", "name mine")])

    def test_an_attribute_the_client_dropped_is_gone(self):
        self.scenario("one", "v = thing.oldKeyName\n")
        rc, out = self.run_tool()
        self.assertEqual(rc, 3)
        self.assertEqual(self.status(self.rows(out), "name oldKeyName"), "gone")

    def test_comments_and_docstrings_are_not_code(self):
        self.scenario("one", '"""sm.GetService(\'goneSvc\') and oldKeyName in prose."""\n'
                             "def f():\n    '''\n    from demo.removed import Anything\n    '''\n    return 1  # sm.GetService('goneSvc')\n"
                             "'oldKeyName'\n")
        rc, out = self.run_tool()
        self.assertEqual(self.rows(out), [])
        self.assertEqual(rc, 0)

    def test_the_prelude_copy_in_a_scenario_is_skipped_and_the_prelude_read_once(self):
        body = ("# PRELUDE -- byte-identical in every scenario in this directory, on purpose.\n"
                "x = row['oldKeyName']\n"
                "# -- the scenario ------\n"
                "y = 1\n"
                "# -- end of the scenario body ----\n"
                "z = row['oldKeyName']\n")
        self.scenario("one", body)
        self.scenario("two", body)
        with open(os.path.join(self.sdir, "_prelude.py"), "w") as fh:
            fh.write("p = row['oldKeyName']\n")
        _, out = self.run_tool()
        rows = [r for r in self.rows(out) if r["status"] == "gone"]
        self.assertEqual([r["scenario"] for r in rows], ["_prelude"])


class Lines(Base):
    def test_a_reference_after_the_prelude_keeps_its_file_line(self):
        body = ("# PRELUDE -- byte-identical in every scenario in this directory, on purpose.\n"   # 1
                "x = 1\n"                                                                          # 2
                "# -- the scenario ------\n"                                                       # 3
                "y = 1\n"                                                                          # 4
                "v = row['oldKeyName']\n"                                                          # 5
                "# -- end of the scenario body ----\n")                                            # 6
        self.scenario("one", body)
        _, out = self.run_tool()
        rows = [r for r in self.rows(out) if r["status"] == "gone"]
        self.assertEqual([(r["scenario"], r["line"]) for r in rows], [("one", "5")])

    def test_a_line_without_any_prelude(self):
        self.scenario("one", "a = 1\n\nv = row['oldKeyName']\n")
        _, out = self.run_tool()
        self.assertEqual([r["line"] for r in self.rows(out) if r["status"] == "gone"], ["3"])


class Aliases(Base):
    def test_the_alias_of_an_import_is_not_a_reference(self):
        self.scenario("one", "from demo.mod import Stable as oldKeyName, Thing as T2\n")
        _, out = self.run_tool()
        rows = self.rows(out)
        self.assertEqual(self.status(rows, "from demo.mod import Stable"), "ok")
        self.assertEqual(self.status(rows, "from demo.mod import Thing"), "changed")
        self.assertFalse([r for r in rows if "oldKeyName" in r["ref"] or "T2" in r["ref"]])


class Failures(Base):
    def test_a_tokenizer_stop_needs_a_reader(self):
        self.scenario("one", "x = (1,\n")
        rc, out = self.run_tool()
        self.assertEqual(rc, 3)
        self.assertEqual(self.rows(out)[0]["ref"], "(tokenizer stopped)")

    def test_missing_directory_is_exit_two(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(fs.main([os.path.join(self.td, "none"), self.new, "--scenarios", self.sdir]), 2)
            self.assertEqual(fs.main([self.old, self.new, "--scenarios", os.path.join(self.td, "none")]), 2)

    def test_python2_source_the_tokenizer_accepts(self):
        self.scenario("one", "print 'x'\ntry:\n    pass\nexcept Exception, e:\n    pass\nv = sm.GetService('demoSvc')\n")
        rc, out = self.run_tool()
        self.assertEqual(self.status(self.rows(out), "service demoSvc"), "ok")
        self.assertEqual(rc, 0)


if __name__ == "__main__":
    unittest.main()
