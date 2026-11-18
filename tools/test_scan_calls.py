#!/usr/bin/env python3
"""Tests for the `shape` column of tools/scan_calls.py and scan_calls_emit.py,
and for the shape comparison in tools/port_surface.py (answer shapes, Pass C).

Every tree is invented: a tiny assembler writes Python 2.7 .pyc files from made-up
services, methods and keys, so nothing here needs a client.

    python3 -I tools/test_scan_calls.py
"""
import contextlib
import io
import json
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import pyc27  # noqa: E402
import scan_calls as sc  # noqa: E402

OP = {n: v for v, n in pyc27.OPNAME.items()}
JREL = ('FOR_ITER', 'SETUP_LOOP', 'JUMP_FORWARD')


def asm(ops, consts, names, varnames=()):
    """ops: (mnemonic, arg) with arg an index, or a label name for jumps; ('L', name) marks a label."""
    labels, off = {}, 0
    for o in ops:
        if o[0] == 'L':
            labels[o[1]] = off
        else:
            off += 3 if OP[o[0]] >= 90 else 1
    out, off = bytearray(), 0
    for o in ops:
        if o[0] == 'L':
            continue
        code = OP[o[0]]
        if code < 90:
            out.append(code)
            off += 1
            continue
        arg = o[1]
        if isinstance(arg, str):
            arg = labels[arg] - (off + 3) if o[0] in JREL else labels[arg]
        out += bytes((code, arg & 255, arg >> 8))
        off += 3
    return bytes(out)


def w_obj(b, v):
    if v is None:
        b += b'N'
    elif isinstance(v, int):
        b += b'i' + struct.pack('<i', v)
    elif isinstance(v, str):
        b += b's' + struct.pack('<i', len(v)) + v.encode()
    elif isinstance(v, tuple):
        b += b'(' + struct.pack('<i', len(v))
        for x in v:
            w_obj(b, x)
    else:
        w_code(b, v)


def w_code(b, c):
    name, bytecode, consts, names, varnames, argcount, flags = c.f
    b += b'c' + struct.pack('<iiii', argcount, len(varnames), 8, flags)
    b += b's' + struct.pack('<i', len(bytecode)) + bytecode
    for t in (consts, names, varnames, (), ()):
        w_obj(b, t)
    for s in ('invented/mod.py', name):
        w_obj(b, s)
    b += struct.pack('<i', 1)
    b += b's' + struct.pack('<i', 0)


class Fn(object):
    def __init__(self, *f):
        self.f = f


def fn(name, ops, consts, names, varnames=(), argcount=0, flags=3):
    return Fn(name, asm(ops, consts, names, varnames), consts, names, varnames, argcount, flags)


def klass(name, *methods):
    """A class body: flags 0 (not a function), its methods as code constants."""
    return fn(name, [('LOAD_CONST', len(methods)), ('RETURN_VALUE', None)], tuple(methods) + (None,), (), flags=0)


def write_pyc(path, code):
    b = bytearray()
    w_code(b, code)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, 'wb') as f:
        f.write(b'\x03\xf3\r\n\0\0\0\0' + bytes(b))


def module(*funcs):
    """A module whose functions sit in the constants and are stored by name."""
    names = tuple(f.f[0] for f in funcs)
    ops = []
    for i, f in enumerate(funcs):
        ops += [('LOAD_CONST', i), ('MAKE_FUNCTION', 0), ('STORE_NAME', i)]
    ops += [('LOAD_CONST', len(funcs)), ('RETURN_VALUE', None)]
    return fn('<module>', ops, tuple(funcs) + (None,), names)


def call_head(method='GetThing'):
    """sm.RemoteSvc('inventedSvc').<method>()  -> on the stack. Names 0..2, const 0."""
    return [('LOAD_GLOBAL', 0), ('LOAD_ATTR', 1), ('LOAD_CONST', 0), ('CALL_FUNCTION', 1),
            ('LOAD_ATTR', 2), ('CALL_FUNCTION', 0)]


NAMES = ('sm', 'RemoteSvc', 'GetThing', 'lookup', 'betaAttr', 'get', 'self', 'data')


def census(tree):
    sc.ROOT = tree
    sc.GLOBAL_FUNC.clear()
    sc.GLOBAL_ATTR.clear()
    sc.PROBE_ATTR.clear()
    sink = os.path.join(tree, '..', 'sites.json')
    sc.SITES_OUT = sink
    with contextlib.redirect_stderr(io.StringIO()):
        sc.main()
    with open(sink) as f:
        return json.load(f)


class Shapes(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix='test-scan-calls-')
        self.tree = os.path.join(self.tmp, 'pyc')

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def shapes(self, *funcs):
        write_pyc(os.path.join(self.tree, 'inv.pyc'), module(*funcs))
        rows = census(self.tree)
        return {r['func']: r['shape'] for r in rows if r['method'] == 'GetThing'}

    def test_tuple_unpack_is_unpack_n(self):
        f = fn('f', call_head() + [('UNPACK_SEQUENCE', 2), ('STORE_FAST', 0), ('STORE_FAST', 1),
                                   ('LOAD_CONST', 1), ('RETURN_VALUE', None)],
               ('inventedSvc', None), NAMES, ('a', 'b'))
        self.assertEqual(self.shapes(f)['f'], ['unpack-2'])

    def test_iteration_reads_the_row_one_level(self):
        # for row in svc.GetThing(): row['alphaKey']; row.get('betaKey'); row.betaAttr
        ops = call_head() + [('GET_ITER', None), ('L', 'top'), ('FOR_ITER', 'end'), ('STORE_FAST', 0),
                             ('LOAD_FAST', 0), ('LOAD_CONST', 1), ('BINARY_SUBSCR', None), ('POP_TOP', None),
                             ('LOAD_FAST', 0), ('LOAD_ATTR', 5), ('LOAD_CONST', 2), ('CALL_FUNCTION', 1),
                             ('POP_TOP', None),
                             ('LOAD_FAST', 0), ('LOAD_ATTR', 4), ('POP_TOP', None),
                             ('JUMP_ABSOLUTE', 'top'), ('L', 'end'),
                             ('LOAD_CONST', 3), ('RETURN_VALUE', None)]
        f = fn('f', ops, ('inventedSvc', 'alphaKey', 'betaKey', None), NAMES, ('row',))
        self.assertEqual(self.shapes(f)['f'],
                         ['iter', 'row.attr:betaAttr', 'row.get:betaKey', 'row.key:alphaKey'])

    def test_a_local_read_many_times_keeps_every_read(self):
        # r = svc.GetThing(); r[0]; r['k']; len(r); if r: ...
        ops = call_head() + [('STORE_FAST', 0),
                             ('LOAD_FAST', 0), ('LOAD_CONST', 1), ('BINARY_SUBSCR', None), ('POP_TOP', None),
                             ('LOAD_FAST', 0), ('LOAD_CONST', 2), ('BINARY_SUBSCR', None), ('POP_TOP', None),
                             ('LOAD_GLOBAL', 3), ('LOAD_FAST', 0), ('CALL_FUNCTION', 1), ('POP_TOP', None),
                             ('LOAD_FAST', 0), ('POP_JUMP_IF_FALSE', 'end'), ('L', 'end'),
                             ('LOAD_CONST', 3), ('RETURN_VALUE', None)]
        names = NAMES[:3] + ('len',) + NAMES[4:]
        f = fn('f', ops, ('inventedSvc', 0, 'k', None), names, ('r',))
        self.assertEqual(self.shapes(f)['f'], ['index:0', 'key:k', 'len', 'truth'])

    def rows(self, *funcs):
        write_pyc(os.path.join(self.tree, 'inv.pyc'), module(*funcs))
        return sorted((r['func'], r['shape']) for r in census(self.tree) if r['method'] == 'GetThing')

    def test_in_test_is_has(self):
        ops = call_head() + [('STORE_FAST', 0), ('LOAD_CONST', 1), ('LOAD_FAST', 0), ('COMPARE_OP', 6),
                             ('POP_TOP', None), ('LOAD_CONST', 2), ('RETURN_VALUE', None)]
        f = fn('f', ops, ('inventedSvc', 'k', None), NAMES, ('r',))
        self.assertEqual(self.shapes(f)['f'], ['has:k'])

    RET_NAMES = ('sm', 'RemoteSvc', 'GetThing', 'GetFoo')

    def getter(self):
        return fn('GetFoo', call_head() + [('RETURN_VALUE', None)], ('inventedSvc',), self.RET_NAMES)

    def bare_caller(self):
        return fn('use', [('LOAD_GLOBAL', 3), ('CALL_FUNCTION', 0), ('LOAD_CONST', 0), ('BINARY_SUBSCR', None),
                          ('RETURN_VALUE', None)], ('k',), self.RET_NAMES)

    def self_caller(self):
        return fn('use2', [('LOAD_FAST', 0), ('LOAD_ATTR', 3), ('CALL_FUNCTION', 0), ('LOAD_CONST', 0),
                           ('BINARY_SUBSCR', None), ('RETURN_VALUE', None)], ('k',), self.RET_NAMES, ('self',), 1)

    def test_return_hop_follows_a_name_defined_once(self):
        self.assertEqual(self.rows(self.getter(), self.bare_caller()), [('GetFoo', ['key:k'])])

    def test_return_hop_ignores_a_name_defined_twice_unless_called_on_self(self):
        self.assertEqual(self.rows(self.getter(), self.getter(), self.bare_caller()),
                         [('GetFoo', []), ('GetFoo', [])])
        self.assertEqual(self.rows(self.getter(), self.getter(), self.self_caller()),
                         [('GetFoo', ['key:k']), ('GetFoo', ['key:k'])])

    def test_self_hop_is_keyed_by_class(self):
        load = fn('load', call_head() + [('LOAD_FAST', 0), ('STORE_ATTR', 7), ('LOAD_CONST', 1),
                                         ('RETURN_VALUE', None)], ('inventedSvc', None), NAMES, ('self',), 1)
        use = fn('use', [('LOAD_FAST', 0), ('LOAD_ATTR', 7), ('LOAD_CONST', 0), ('BINARY_SUBSCR', None),
                         ('RETURN_VALUE', None)], ('k',), NAMES, ('self',), 1)
        other = fn('use', [('LOAD_FAST', 0), ('LOAD_ATTR', 7), ('LOAD_CONST', 0), ('BINARY_SUBSCR', None),
                           ('RETURN_VALUE', None)], ('otherKey',), NAMES, ('self',), 1)
        # class A stores and reads 'k'; class B (same file, same attribute name) reads another key
        got = self.rows(klass('A', load, use), klass('B', other))
        self.assertEqual(got, [('load', ['key:k'])])

    def test_one_hop_through_self_reads_the_other_method(self):
        # method A: self.data = svc.GetThing()   method B: self.data['k']
        b = fn('use', [('LOAD_FAST', 0), ('LOAD_ATTR', 7), ('LOAD_CONST', 0), ('BINARY_SUBSCR', None),
                       ('RETURN_VALUE', None)], ('k',), NAMES, ('self',), 1)
        a = fn('load', call_head() + [('LOAD_FAST', 0), ('STORE_ATTR', 7), ('LOAD_CONST', 1),
                                      ('RETURN_VALUE', None)],
               ('inventedSvc', None), NAMES, ('self',), 1)
        got = self.shapes(a, b)
        self.assertEqual(got['load'], ['key:k'])


class Emit(unittest.TestCase):
    def test_identical_sites_dedupe_and_commas_in_a_key_are_escaped(self):
        tmp = tempfile.mkdtemp(prefix='test-scan-emit-')
        try:
            site = {'service': 'alpha', 'method': 'GetThing', 'path': 'inv/a.py', 'line': 3, 'kind': 'moniker',
                    'consumer': 'iterate', 'guard': 'none', 'notes': [], 'nargs': 0, 'func': 'f', 'indirect': None,
                    'shape': ['iter', 'row.key:a,b', 'row.key:50%']}
            with open(tmp + '/sites.json', 'w') as f:
                json.dump([site, site], f)
            with open(tmp + '/served.json', 'w') as f:
                json.dump({'services': ['alpha'], 'pairs': ['alpha\tGetThing']}, f)
            env = dict(os.environ, SCAN_SITES=tmp + '/sites.json', SCAN_SERVED=tmp + '/served.json',
                       SCAN_CALLS_OUT=tmp + '/calls.tsv', SCAN_STATS_OUT=tmp + '/stats.json')
            subprocess.run([sys.executable, '-I', os.path.join(HERE, 'scan_calls_emit.py')], env=env, check=True,
                           capture_output=True)
            with open(tmp + '/calls.tsv') as f:
                lines = f.read().splitlines()
            self.assertEqual(lines[0].split('\t')[-1], 'shape')
            self.assertEqual(len(lines), 2, 'two identical sites are one row')
            cols = lines[1].split('\t')
            self.assertEqual(len(cols), 9)
            self.assertEqual(cols[-1], 'iter,row.key:a%2Cb,row.key:50%25')
            self.assertIn('x2 identical sites', cols[-2])
        finally:
            shutil.rmtree(tmp, ignore_errors=True)


if __name__ == '__main__':
    unittest.main()
