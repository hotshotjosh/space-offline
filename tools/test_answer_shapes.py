#!/usr/bin/env python3
"""Tests for tools/answer-shapes.py (answer shapes, Pass A and Pass B).

Every tree is invented: .pyc files are written from made-up code objects with the writer in
test_port_recheck.py, and the Rust is a made-up file. Nothing here needs a client.

    python3 -I tools/test_answer_shapes.py
"""
import contextlib
import importlib.util
import io
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import test_port_recheck as T  # noqa: E402

_spec = importlib.util.spec_from_file_location('answer_shapes', os.path.join(HERE, 'answer-shapes.py'))
A = importlib.util.module_from_spec(_spec)
sys.modules['answer_shapes'] = A
_spec.loader.exec_module(A)

RUST = '''
fn row() -> PyValue {
    pydict(vec![
        ("widgetKeepName", PyValue::Int(1)),
        (
            "widgetOldName",
            PyValue::Int(2),
        ),
        ("id", PyValue::Int(3)),
        ("tin", PyValue::Int(3)),
    ])
}
const COLS: &[&str] = &["colAlpha", "colBeta"];
'''
RUST_TESTS = 'fn t() { let _ = ("onlyInTests", 1); }\n'


def reader(*words, name='read'):
    """A module whose function `name` holds each word as a string constant."""
    return T.module([(1, 'x', 1)], nested=(T.fn(name, 3, consts=(None,) + tuple(words)),))


class Base(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.mkdtemp(prefix='shapes-')
        self.addCleanup(shutil.rmtree, self.td)
        self.old, self.new = os.path.join(self.td, 'old'), os.path.join(self.td, 'new', 'pyc')
        self.srv = os.path.join(self.td, 'crates')
        src = os.path.join(self.srv, 'eve-server', 'src')
        os.makedirs(os.path.join(src, 'service'))
        os.makedirs(os.path.join(src, 'tests'))
        os.makedirs(self.old)
        os.makedirs(self.new)
        for rel, text in (('service/thing.rs', RUST), ('tests.rs', RUST_TESTS), ('tests/more.rs', RUST_TESTS)):
            with open(os.path.join(src, rel), 'w') as f:
                f.write(text)
        # a sibling crate: never read
        os.makedirs(os.path.join(self.srv, 'eve-bot', 'src'))
        with open(os.path.join(self.srv, 'eve-bot', 'src', 'b.rs'), 'w') as f:
            f.write('fn b() { let _ = ("botOnlyKey", 1); }\n')

    def tree(self, side, rel, code):
        T.pyc(os.path.join(side, rel + '.pyc'), code)

    def run_tool(self, *extra):
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            rc = A.main([self.old, self.new, '--server', self.srv, *extra])
        return rc, err.getvalue()

    def rows(self):
        p = os.path.join(os.path.dirname(self.new), 'shapes', 'shapes.tsv')
        with open(p) as f:
            return [l.rstrip('\n').split('\t') for l in f][1:]


class PassA(Base):
    def test_inventory(self):
        keys = A.server_keys(self.srv)
        self.assertEqual(sorted(keys), ['colAlpha', 'colBeta', 'widgetKeepName', 'widgetOldName'])
        self.assertEqual(keys['widgetOldName'][0], 'service/thing.rs:5')
        self.assertEqual(keys['widgetKeepName'][1], 1)

    def test_short_keys_tests_and_other_crates_are_skipped(self):
        keys = A.server_keys(self.srv)
        for k in ('id', 'tin', 'onlyInTests', 'botOnlyKey'):
            self.assertNotIn(k, keys)


FORMS = '''
const KEY_CONST: &str = "constKeyOne";
pub static KEY_STATIC: &'static str = "staticKeyTwo";
fn a() {
    let _ = pydict(vec![(KEY_CONST, 1), (KEY_STATIC, 2), (UNKNOWN_CONST, 3)]);
    let _ = ("ownedKeyThree".to_owned(), 1);
    let _ = ("intoKeyFour".into(), 1);
    let _ = ("stringKeyFive".to_string(), 1);
    let _ = (String::from("fromKeySix"), 1);
    let _ = vec!["vecKeySeven", "vecKeyEight"];
    let _: [&str; 2] = ["arrKeyNine", "arrKeyTen"];
    let _ = &["singleKeyEleven"];
    let _ = ["bareSingleTwelve"];
    let _ = map["indexedKeyNotAKey"];
    let _ = &[A_CONST_IN_ARRAY, "mixedKeyThirteen"];
}
const A_CONST_IN_ARRAY: &str = "arrayConstFourteen";
#[cfg(test)]
mod tests {
    fn t() { let _ = ("cfgTestKeyHidden", 1); let s = "}"; let _ = ("afterBraceInString", 2); }
}
#[cfg(test)]
fn single() { let _ = ("cfgTestFnHidden", 1); }
fn after() { let _ = ("afterCfgTestKept", 1); }
'''


class PassAForms(Base):
    def keys(self):
        with open(os.path.join(self.srv, 'eve-server', 'src', 'forms.rs'), 'w') as f:
            f.write(FORMS)
        return A.server_keys(self.srv)

    def test_constants_naming_a_key_are_resolved(self):
        k = self.keys()
        self.assertIn('constKeyOne', k)
        self.assertIn('staticKeyTwo', k)
        self.assertIn('arrayConstFourteen', k)
        self.assertNotIn('UNKNOWN_CONST', k)

    def test_converted_literals_are_keys(self):
        k = self.keys()
        for key in ('ownedKeyThree', 'intoKeyFour', 'stringKeyFive', 'fromKeySix'):
            self.assertIn(key, k)

    def test_the_other_array_forms(self):
        k = self.keys()
        for key in ('vecKeySeven', 'vecKeyEight', 'arrKeyNine', 'arrKeyTen', 'singleKeyEleven',
                    'bareSingleTwelve', 'mixedKeyThirteen'):
            self.assertIn(key, k)
        self.assertNotIn('indexedKeyNotAKey', k)

    def test_cfg_test_items_are_stripped_by_brace_matching(self):
        k = self.keys()
        for key in ('cfgTestKeyHidden', 'afterBraceInString', 'cfgTestFnHidden'):
            self.assertNotIn(key, k)
        self.assertIn('afterCfgTestKept', k)

    def test_line_numbers_survive_the_stripping(self):
        k = self.keys()
        self.assertEqual(k['afterCfgTestKept'][0], 'forms.rs:%d' % (FORMS.count('\n', 0, FORMS.index('afterCfgTestKept')) + 1))


class PassB(Base):
    def test_renamed_key_is_served_gone_with_a_candidate(self):
        self.tree(self.old, 'ui/widget', reader('widgetKeepName', 'widgetOldName', 'colAlpha', 'colBeta'))
        self.tree(self.new, 'ui/widget', reader('widgetKeepName', 'widgetNewName', 'colAlpha', 'colBeta'))
        rc, err = self.run_tool()
        self.assertEqual(rc, 3, err)
        rows = self.rows()
        self.assertEqual([r[2] for r in rows], ['widgetOldName'])
        self.assertEqual(rows[0][:2], ['B', 'SERVED gone'])
        self.assertEqual(rows[0][3], 'service/thing.rs:5')
        self.assertEqual(rows[0][4:6], ['1', '0'])
        self.assertIn('candidate rename: widgetNewName', rows[0][6])

    def test_a_key_that_survives_gives_no_row(self):
        for side in (self.old, self.new):
            self.tree(side, 'ui/widget', reader('widgetKeepName', 'widgetOldName', 'colAlpha', 'colBeta'))
        rc, _ = self.run_tool()
        self.assertEqual(rc, 0)
        self.assertEqual(self.rows(), [])

    def test_a_key_read_elsewhere_in_new_survives(self):
        self.tree(self.old, 'ui/widget', reader('widgetKeepName', 'widgetOldName', 'colAlpha', 'colBeta'))
        self.tree(self.new, 'ui/other', reader('widgetKeepName', 'widgetOldName', 'colAlpha', 'colBeta'))
        rc, _ = self.run_tool()
        self.assertEqual(rc, 0)

    def test_a_key_the_client_never_read_is_not_gone(self):
        self.tree(self.old, 'ui/widget', reader('widgetKeepName', 'colAlpha', 'colBeta'))
        self.tree(self.new, 'ui/widget', reader('widgetKeepName', 'colAlpha', 'colBeta'))
        rc, _ = self.run_tool()
        self.assertEqual(rc, 0)

    def test_only_a_whole_marshalled_string_counts(self):
        # `widgetOldNameExtra` contains the key's bytes but is a different string
        self.tree(self.old, 'ui/widget', reader('widgetOldName', 'widgetKeepName', 'colAlpha', 'colBeta'))
        self.tree(self.new, 'ui/widget', reader('widgetOldNameExtra', 'widgetKeepName', 'colAlpha', 'colBeta'))
        rc, _ = self.run_tool()
        self.assertEqual(rc, 3)
        self.assertEqual([r[2] for r in self.rows()], ['widgetOldName'])

    def test_a_column_array_key_is_checked_too(self):
        self.tree(self.old, 'ui/widget', reader('widgetKeepName', 'widgetOldName', 'colAlpha', 'colBeta'))
        self.tree(self.new, 'ui/widget', reader('widgetKeepName', 'widgetOldName', 'colAlpha'))
        rc, _ = self.run_tool()
        self.assertEqual(rc, 3)
        self.assertEqual([r[2] for r in self.rows()], ['colBeta'])


class Usage(Base):
    def test_missing_tree_is_exit_2(self):
        out = io.StringIO()
        with contextlib.redirect_stderr(out):
            rc = A.main([os.path.join(self.td, 'nope'), self.new, '--server', self.srv])
        self.assertEqual(rc, 2)

    def test_no_arguments_is_exit_2(self):
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(A.main([]), 2)

    def test_default_out_is_beside_new_pyc(self):
        self.tree(self.old, 'ui/widget', reader('widgetKeepName'))
        self.tree(self.new, 'ui/widget', reader('widgetKeepName'))
        self.run_tool()
        self.assertTrue(os.path.isfile(os.path.join(os.path.dirname(self.new), 'shapes', 'shapes.md')))
        self.assertTrue(os.path.isfile(os.path.join(os.path.dirname(self.new), 'shapes', 'server-keys.tsv')))


if __name__ == '__main__':
    unittest.main()
