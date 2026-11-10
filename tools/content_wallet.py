"""The `wallet` content section: the wallet journal's entry types.

Lane wallet-l1 (`specs/client-data-out.md` B8, L1). A real `content.py`
section, found by glob, so `tools/up.sh` extracts it into the bundle. It moves
one table out of the Rust source, where its rows were hand-copied from the
client, into the bundle:

    wallet/entry-types.tsv   #entry_type_id name_id

One row per record of the client's `accountingentrytypes.fsdbinary`, both the
old arithmetic run (`entryTypeNameID == 59792 + entryTypeID` for ids
0..=108, **[V]** `eve-server` `ledger::ENTRY_TYPE_NAME_ID_BASE`) and the
individually allocated rest. The journal's Type column is the localised string
of `name_id` (`GetStaticData` overwrites `entryTypeName` with
`GetByMessageID(entryTypeNameID)`), so an entry type with no row, or no
`entryTypeNameID`, has no `name_id` here and the server leaves the cell empty.

`rows(records)` is pure (decoded records in, sorted rows out); the unit tests
feed it invented records. `gather(ctx)` is the only client reader, through
`tools/fsd.py`'s `accounting_entry_types` decoder.
"""
import os
import sys

SECTION = "wallet"
DEPENDS = ()
CFSD = ("accountingentrytypes",)

_HERE = os.path.dirname(os.path.abspath(__file__))

FILE = "wallet/entry-types.tsv"
HEADER = ("entry_type_id", "name_id")
SOURCES = ["res:/staticdata/accountingentrytypes.fsdbinary"]

#: The arithmetic run, **[V]** `ledger::ENTRY_TYPE_NAME_ID_BASE`, `_MAX` and
#: `_RUN_GAPS` (kept in step by hand; `check` compares them with the table).
RUN_BASE = 59792
RUN_MAX = 108
RUN_GAPS = (100, 104, 105)


def rows(records):
    """The `wallet/entry-types.tsv` rows of decoded records, sorted by entry
    type id. A record is a dict with `entryTypeID` and `entryTypeNameID`; one
    whose name id is absent or zero has nothing to show and gets no row.
    Two records for one id are an error: the client's table is keyed on it."""
    out = {}
    for r in records:
        tid, name_id = r.get("entryTypeID"), r.get("entryTypeNameID")
        if tid is None or not name_id:
            continue
        tid, name_id = int(tid), int(name_id)
        if tid in out:
            raise ValueError("wallet: entry type %d twice" % tid)
        out[tid] = name_id
    return sorted(out.items())


def run_disagreements(table):
    """Where `{entry_type_id: name_id}` and the arithmetic run disagree, as
    messages: a run id (not a gap) that is missing or has another name id, a
    gap that has a row, and a row past the run that restates the formula."""
    bad = []
    for tid in range(0, RUN_MAX + 1):
        want = None if tid in RUN_GAPS else RUN_BASE + tid
        got = table.get(tid)
        if tid in RUN_GAPS and got is not None:
            bad.append("entry type %d is a gap in the run but has a row" % tid)
        elif want is not None and got is not None and got != want:
            bad.append("entry type %d: name id is not run base + id" % tid)
        elif want is not None and got is None:
            bad.append("entry type %d is in the run but has no row" % tid)
    for tid, name_id in table.items():
        if tid > RUN_MAX and name_id == RUN_BASE + tid:
            bad.append("entry type %d restates the formula past the run" % tid)
    return bad


def _content():
    if _HERE not in sys.path:
        sys.path.append(_HERE)
    import content
    return content


def gather(ctx):
    """The client's records: a list of dicts from the cFSD decoder."""
    fsd = _content()._imp("fsd")  # noqa: SLF001
    return list(fsd.accounting_entry_types(ctx.client).values())


def section(ctx):
    got = rows(gather(ctx))
    if not got:
        raise ValueError("wallet: the client's accountingentrytypes has no named entry type")
    ctx.note("wallet: %d entry types" % len(got))
    return [(FILE, HEADER, got, SOURCES)]


def check(ctx):
    content = _content()
    fresh = rows(gather(ctx))
    bad = []
    if not os.path.isfile(ctx.path(FILE)):
        return ["%s: CHECK FAILED: the file is missing (re-extract with tools/up.sh)" % FILE]
    got = list(content.read_rows(ctx.path(FILE)))
    if got != [content.encode_row(r).split("\t") for r in fresh]:
        bad.append("%s: differs from a fresh derive (%d vs %d rows)"
                   % (FILE, len(got), len(fresh)))
    table = dict(fresh)
    messages = ctx.messages()
    missing = sum(1 for n in table.values() if n not in messages)
    if missing:
        bad.append("%s: %d of %d name ids do not resolve in the en-us localisation"
                   % (FILE, missing, len(table)))
    bad.extend("%s: %s" % (FILE, m) for m in run_disagreements(table))
    ctx.note("wallet: %d rows, %d unresolved, %d disagreements with the run"
             % (len(table), missing, len(run_disagreements(table))))
    return bad
