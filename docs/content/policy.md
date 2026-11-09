# Content policy: what may be committed, and where authored data lives

This page is the short, public rule for data in this repository. It applies to
every table, constant and test. The longer research behind it is internal and
is not part of the public repo.

## Three classes of data

| class | what it is | where it lives | example |
|---|---|---|---|
| **A: protocol vocabulary** | ids and constants the wire and the logic need: service names, enum values, group ids, dialog keys, reference types, attribute ids | Rust source, each cited `[V]` | a group id the client checks |
| **B: client content rows** | per-type or per-row numbers and lists that came out of the client | `out/content/` only (gitignored), written by `tools/content.py` | hull numbers, reprocessing materials, blueprints, name lists |
| **C: what we invent** | policy tables and tuning no client file gives us | `content/*.tsv` for a table, a Rust const for a single number | `content/population.tsv`, reward multipliers |

## The rules

1. **Class B never enters the repo.** Not in a table, not in a test, not in a
   doc beyond a column name, a count or at most one sample row. A test that
   needs class-B-shaped data builds a `fixture()` table of invented rows and
   ids. Tests, `tools/test_content_*.py` included, hold no client id, count or
   name; a real count or known id is checked by the section's `check(ctx)` in
   `tools/content.py --check`, which runs against your own bundle. A small individual client id is allowed where a class C row needs one
   (for example a station or warzone id).
2. **A table or a const?** A value keyed by band, level, faction or region is a
   row in `content/`. A single number is a Rust const with an `[A]` doc comment
   that names its falsifier.
3. **Every `[A]` row carries a `why` column** (last) that says what observation
   would prove the value wrong. An `[A]` value never goes into a test.
4. **Controller-tuned values are world state, not files.** Prices, bids and
   other values the running server adjusts are stored as deltas in the saved
   world.
5. **Without a content bundle the server still boots**, docks and runs the wire
   tests on invented fixture content. That world is a test aid, not a place to
   play.

## Format of `content/`

TSV, UTF-8, `#` comment lines, a header row, one kind of record per file. The
header line of each file is frozen in `docs/content/schema.md` §4, and the
loader rejects a file whose header differs. See `content/README.md` for how the
server reads these files and how to add one.

## Related

- The client data rule in `AGENTS.md` ("No extracted client data in git").
- `docs/content/server-only.md`: what the server can derive from the client and
  what only a capture of the live game would add.
