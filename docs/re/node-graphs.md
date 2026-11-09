# Node graphs: the parameter variant, decoded

What this is: how the 32-byte `value`/`defaultValue` field of the node-graph
family of static tables (`nodegraphs`, `atoms`, `nodetypes`,
`dungeonobjectives`, `objectivechains`) is laid out, how that was read out of
the native loaders, how `tools/fsd.py` now decodes it, and the proof that the
decode is right. Lane HF-0 (#206). Read with
[`native-loaders.md`](native-loaders.md), which is the method, and
[`homefront.md`](homefront.md), which uses the decode.

**Evidence markers.** `[V]` read out of the pinned client's loader binaries
(build 3670638, `bin64/*Loader.pyd`, disassembled with `objdump` inside
`tools/analysis.sh`) or its shipped static data, with the place. `[I]`
inferred, with what would falsify it. Counts are of build 3670638's data. No
client value, id or text is in this file: only code identifiers (node type,
parameter and key names), loader addresses and counts.

---

## 1. Headline

- **[V] The field is a tagged variant**: the value in the first 24 bytes, a
  `uint32` tag at `+24`, 4 bytes of padding. The tag is the parameter's
  *type* (an atom id, a node-graph id, a bool, a duration, a string key...),
  and the loader maps each tag to one of nine storage kinds.
- **[V] Pointers inside it are payload-relative**, like every other arena
  pointer in the container: a string or list arm holds an 8-byte pointer
  whose vector length sits at `pointer - 8`. (The lane spec said the offsets
  were "not absolute"; for the value at `+0` that does not hold. **[I]** the
  earlier probe read the 32 bytes as the TSV's `list<i32>`, whose pointer is
  then an inline integer or a string pointer, not a list.)
- **[V] Two arm tables.** `atoms`, `nodegraphs` and `nodetypes` share one
  66-arm converter, identical arm for arm across the three loaders.
  `dungeonobjectives` and `objectivechains` use a 68-arm converter whose tags
  0..64 are the same, then 65 a nested dict, 66 a nested list, 67 the vector.
- **The decode closes.** Every `node_graph_id` a `Subgraph` node of the
  spec's proof graph holds is a graph in the table (28 of 28), every
  `Start Client Node Graph` node names a `client`-tagged graph (19 of 19), and
  across the table 879 of 879 present `node_graph_id` values, 7,551 of 7,551
  `atom_id` values and 570 of 570 integer `conversation_id` values close into
  their tables (§4).

## 2. The converter, from the binary

The getter for `nodegraphs` `struct:unnamed_8643665391442253824.value`
(`findings/pyd-schemas.tsv`: offset 16, presence bit `0x2` of the word at
`+48`) compares the attribute name, tests the presence bit, and tail-jumps to
one converter with a pointer to `+16` **[V]** (`nodeGraphsLoader.pyd`,
`0x180002086`..`0x18000209e`). The converter (`0x180001b10`) **[V]**:

1. loads the `uint32` at `[variant+0x18]`;
2. compares it with the last arm index and `ja` to a branch that increfs and
   returns `Py_None`;
3. dispatches through a jump table whose own RVA is `0x1ccc`: it loads the
   image base with `lea rcx, [rip-...]`, reads the 32-bit entry at
   `[rcx + rax*4 + 0x1ccc]`, adds the image base to it and `jmp`s. So
   `0x1ccc` is the table's RVA, and each entry is itself an RVA (the arm's
   address minus the image base).

Each arm was classified by what it loads and which Python constructor it
calls **[V]**:

| storage | how the arm reads it | Python object |
|---|---|---|
| `i32` | `mov ecx, DWORD [variant]` | `PyInt_FromLong` |
| `u8` | `movzx ecx, BYTE [variant]` | `PyInt_FromLong` (0/1, not a bool) |
| `f32` | `movss` + `cvtps2pd` | `PyFloat_FromDouble` |
| `i64` | `mov rcx, QWORD [variant]` | `PyLong_FromLongLong` |
| `str` | pointer at `+0`, length at `pointer - 8` | `PyString_FromStringAndSize` |
| `vec3<f64>` | three `movsd` from `+0`, `+8`, `+16` | a struct sequence of three floats |
| `list<u8>`, `list<f32>`, `list<i32>`, `list<str>` | a `cfsd.list` object that keeps the variant's address; its `sq_item` reads the pointer at `+0` and indexes bytes, `f32`s, `i32`s or string pointers | `cfsd.list` |

The full map is `CFSD_VARIANT_ARMS` in [`tools/fsd.py`](../../tools/fsd.py).
In short: 15 tags are `str`, `u8` is 2 tags, `f32`, `i64` and each list kind
are 1 tag each, the vector is the last tag, and every other tag (42 of the
66) is `i32`.

**The second converter** (`dungeonObjectivesLoader.pyd` `0x1800024d0`,
`objectiveChainsLoader.pyd` `0x1800028d0` for `value` and `0x180002b40` for
`defaultValue`) has 68 arms **[V]**: tags 0..64 classify the same as the
first converter's; 65 builds a `cfsd.dict` whose entries are an 8-byte string
key and a 32-byte inner variant; 66 builds a `cfsd.list` of 32-byte inner
variants; 67 is the vector. The inner variants go through two further
converters numbered one higher (their arm 0 is the nested container). No row
of build 3670638 uses tag 65 or 66, so `tools/fsd.py` returns them as
`CfsdUnknown('variant:65')` / `('variant:66')` and does not decode the inner
numbering. **[I]** the difference is a type enum with two container types
added before the vector, sorted by name; a row with tag 65 or 66 would show
it.

**Reproducing it.** The disassembly and the import table come from:

    tools/analysis.sh objdump -d -M intel --no-show-raw-insn /client/bin64/nodeGraphsLoader.pyd
    tools/analysis.sh objdump -p /client/bin64/nodeGraphsLoader.pyd

and the same for the other four loaders. The variant converter of each
(entry, arm count, jump-table RVA; image base `0x180000000`, build 3670638)
**[V]**:

| loader | field getter's converter | arms | jump-table RVA |
|---|---|---:|---|
| `nodeGraphsLoader.pyd` | `0x180001b10` | 66 | `0x1ccc` |
| `atomsLoader.pyd` | `0x180001780` | 66 | `0x193c` |
| `nodeTypesLoader.pyd` | `0x180001930` | 66 | `0x1aec` |
| `dungeonObjectivesLoader.pyd` | `0x1800024d0` (`defaultValue`) | 68 | `0x26a8` |
| `objectiveChainsLoader.pyd` | `0x1800028d0` (`value`), `0x180002b40` (`defaultValue`) | 68 | `0x2a30`, `0x2ca0` |

The two objective loaders also contain a 66-arm converter and two 67-arm
ones. The nested arms call the 67-arm ones; the 66-arm one is called from
elsewhere in the loader, not from these fields' getters.
Each converter is found by the pattern `mov eax, DWORD [rcx+0x18]`, `cmp eax,
<last arm>`, `ja`. Each arm is classified by following it to its first call
through the import table. For the `_PyObject_New` arms, read the type
object's `tp_as_sequence` (`+0x68`), then its `sq_item` (`+0x18`), and
classify the element load. That classifier was a lane-local stdlib script and
is not committed. **The arm-table match (§4) was verified by lane HF-0 on
3670638**; a follow-up should commit the classifier so the claim re-runs.

## 3. The decoder

`CfsdDecoder.variant` reads the tag, returns `None` for a tag past the last
arm, `CfsdUnknown('variant:<tag>')` for a nested arm, and otherwise decodes
the arm's kind at `+0` with the ordinary `CfsdDecoder.value`. It records
every tag it reads in `CfsdDecoder.variant_tags` (`{field path: {tag:
count}}`). The value is what the client's getter returns, so it is what a
graph node sees: the client reads a parameter as `.value` of the FSD object
and, when the presence bit is off, falls back to the node type's
`defaultValue` (`nodegraph/common/nodes/base.py:128-136`, decompiled 3662168)
**[V]**. A `None` value with a `connection` is a wired input: its value comes
from another node at run time.

Tests: `tools/test_fsd_generic.py` class `Variants` builds one invented
variant per arm and checks the decode, the `None` past the last arm, the
objective table's different last arms, and the raw fallback for a table with
no arm table.

## 4. Proof, against build 3670638

Run in `tools/analysis.sh` with `tools/fsd.py` (`fsd.rows`). The id checks
are a short probe of this shape, which prints the closure for each
`node_graph_id`, `atom_id` and `conversation_id` (the other id checks swap in
the parameter and table names):

    tools/analysis.sh python3 -I - <<'EOF'
    import sys; sys.path.insert(0, "/work/tools"); import fsd
    c = fsd.Client("/client")
    G, NT = fsd.rows(c, "nodegraphs"), fsd.rows(c, "nodetypes")
    name = {k: v["name"] for k, v in NT.items()}
    ids = [p["value"] for g in G.values() for n in g["nodes"].values()
           for f in ("nodeParameters", "atomParameters")
           for k, p in (n[f] or {}).items() if k == "node_graph_id" and p["value"] is not None]
    print(len(ids), sum(i in G for i in ids))
    start = [p["value"] for g in G.values() for n in g["nodes"].values()
             if name[n["nodeType"]] == "Start Client Node Graph"
             for k, p in (n["nodeParameters"] or {}).items() if k == "node_graph_id"]
    print(len(start), sum("client" in (G[i]["tags"] or []) for i in start if i in G))
    EOF


| check | result |
|---|---|
| `Subgraph` nodes of the spec's proof graph: `node_graph_id` decodes to an `int` that is a key of `nodegraphs` | **28 of 28** (26 distinct) |
| `Start Client Node Graph` nodes, whole table: id in `nodegraphs` | **19 of 19**; all 19 tagged `client` (5 distinct graphs; the hosts are `server`-tagged graphs) |
| every `node_graph_id` parameter, whole table | 880; 879 present, **879 of 879** in the table; the one absent value is wired (`connection` set, presence bit off) |
| `atom_id` parameters: id in `atoms` | **7,551 of 7,551** |
| integer `conversation_id` parameters: id in `conversations` | **570 of 570** |
| strings decoded | 19,733, none with a non-printable character |
| `f32` values | 790, all finite |
| `u8`-arm values of the flag parameters (`keep_listening`, `track_action`, ...) | only 0 and 1 |
| arm tables against the disassembly | all five loaders match (verified by lane HF-0 on 3670638; the classifier is not committed, §2) |
| `python3 -I tools/fsd.py generic-check /client` | every hand-written path still equal, 0 bad |

Census, for scale: `nodegraphs` has 585 graphs (482 tagged `client`, 100
`server`), 21,917 nodes and 41,894 parameters (4,785 absent), 37,109 of them
variants over 47 distinct tags; `atoms` 178 variants over 19 tags, `nodetypes`
29 over 5, `dungeonobjectives` 13 over 2 (`list<i32>` and `str`);
`objectivechains` has no `contextParameterTrigger` set on any of its 571
objectives, so none of its variants is read.

## 5. The node vocabulary a server needs

`nodetypes` names 45 node types **[V-data]**. The ones that touch the shared
context (the blackboard) or messages, with the parameter names they read
(`nodegraph/common/nodes/blackboard.py`, decompiled 3662168) **[V]**:

| node type | class | parameters | what it does |
|---|---|---|---|
| `Blackboard Changed` | `BlackboardEventNode` | `blackboard_key`, `keep_listening`, `validate_on_start` | fires on every update of the key |
| `Blackboard Value` | `BlackboardValidationNode` | `blackboard_key`, `blackboard_value`, `object_path`, `operator`, `flipped` | compares the key's value with a literal, after casting one side to the other's type (`nodegraph/common/util.py:87-121`) |
| `Get Blackboard Value` | `BlackboardReadNode` | `blackboard_key`, `object_path`, `default_value` | reads the key |
| `Blackboard Write` | `BlackboardWriteNode` | `blackboard_key`, `blackboard_value`, `object_path`, `force_scatter` | writes `literal_eval` of the string, or the string |
| `Node Graph Message Listener` | | `message_key`, `keep_listening` | waits for a message |
| `Send Message To Node Graph` | | `message_key`, `message_value` | sends one |

The dungeon side of that context is `dungeonTracking.ActiveDungeon`
(`eve/client/script/parklife/dungeonTracking.py:147-166`, decompiled
3662168) **[V]**: it seeds `dungeon_id` and `dungeon_room_id`, writes each
dungeon value (a dotted key is an object path) into the context, and
prefixes a server message's key with `dungeon_` before sending it.
[`homefront.md`](homefront.md) lists what the Homefront graphs read through
these nodes.

## 6. Not done

- The inner numbering of the objective tables' nested arms (tags 65, 66):
  unused in this build.
- The `dict<str,list>` fields of these tables (`graphNode.connections`,
  `objectivechains` `triggers`) are still `CfsdUnknown`: their element type is
  not stated by the loader, and it is a different question from the variant.
