# The object wrapper codes `0x17`, `0x22`, `0x23` and the `0x2D` terminator

Client build V24.01, 3619879. Recovered by disassembling
`/client/bin64/blue.dll` (ImageBase `0x180000000`) and cross-checked against
the client's own unobfuscated Python. Closes gap 1 of the unpublished marshal-tags note
("`0x17`/`0x22`/`0x23` payload field order").

Confidence markers, as in `docs/reference/dbrow.md`: **[V]** verified from this build's
instructions or from its own source, **[I]** inferred, **[?]** open.

Canonical/published PyRep descriptions were not used as a source. Where the
result coincides with the folklore it is said so explicitly at the end.

---

## 1. The layouts, up front

All three are **tag byte, then whole marshal objects** — none of them has an
inline count, length or offset field of its own. `shared = tag & 0x40`
applies to all three and is the only flag.

```text
0x17   old-style instance                      [V]
  u8    tag  0x17 | shared
  obj   className   — a marshal string "module.ClassName"
  obj   state
  (nothing else; no terminator)

0x22   "object, 2.2 form"                      [V]
  u8    tag  0x22 | shared
  obj   header — a TUPLE: (callable, args)  or  (callable, args, state)
  obj*  list-append stream   (zero or more objects)
  u8    0x2D
  obj*  dict-setitem stream  (zero or more KEY,VALUE object pairs)
  u8    0x2D

0x23   "object, 2.3 form"                      [V]
  u8    tag  0x23 | shared
  obj   header — a TUPLE: ((cls, *args),)  or  ((cls, *args), state)
  obj*  list-append stream   (zero or more objects)
  u8    0x2D
  obj*  dict-setitem stream  (zero or more KEY,VALUE object pairs)
  u8    0x2D
```

Semantics the reader applies:

| code | object construction | state application |
|---|---|---|
| `0x17` | `PyInstance_NewRaw(klass, NULL)` — `__init__` is **not** called | `inst.__setstate__(state)` if the **class** has `__setstate__`, else `inst.__dict__.update(state)` |
| `0x22` | `obj = header[0](*header[1])` | if `len(header) > 2`: `obj.__setstate__(header[2])`, falling back to `obj.__dict__.update(header[2])` |
| `0x23` | `obj = header[0][0].__new__(*header[0])` — i.e. `cls.__new__(cls, *args)` | if `len(header) > 1`: same as `0x22`, with `header[1]` |

Four consequences a Rust encoder must honour:

1. **`0x17` carries no terminator and no append streams.** `0x22`/`0x23`
   carry **exactly two** `0x2D` bytes, always, even when both streams are
   empty.
2. **The dict-setitem stream is `key, value`** — the *opposite* order from
   the dict tag `0x16`, which is `value, key` (the unpublished marshal-tags note, re-verified
   there at `0x1802dd2f7`). Getting these the same way round is a bug.
3. **The two state carriers are different.** `0x22`/`0x23` put the state
   *inside the header tuple*; `0x17` puts it in a *second top-level object*.
   There is no inline `(key, value)` sequence for state in any of the three —
   that is what the dict-setitem stream is for, and it is a separate thing.
4. **`0x17`'s class name is a dotted `module.ClassName` string that must be in
   `blue.marshal.globalsWhitelist`.** It is not the `__guid__`, not a
   `(module, name)` tuple and not a shared-table reference. See §6.

---

## 2. `0x22` — handler `0x1802de1c0` **[V]**

Function bounds `0x1802de1c0-0x1802de424` (from `.pdata`; 612 bytes, so the
leaf-function caveat in the unpublished marshal-tags note does not apply).

The handler reserves the shared slot with `RegisterShared(stream, 0)` **before any
child is read** (`0x1802de1f8`), reads one child, the header, and takes
`callable = header[0]` and `args = header[1]` by `PyTuple_GetItem`. It builds the
object with `PyObject_CallObject(callable, args)` (`0x1802de268`) and commits the
reserved slot. When `header->ob_size` is 2 or less there is no state
(`0x1802de2be`); otherwise `state = header->ob_item[2]`. State goes in by
`obj.__setstate__(state)` (name from `ctx[0x78]`); when that attribute is missing
the error is cleared and `obj.__dict__.update(state)` runs instead (name from
`ctx[0x80]`). The list-append and dict-setitem streams are read last (§5).

`PyTupleObject` field offsets: `ob_refcnt 0x00, ob_type 0x08, ob_size 0x10,
ob_item 0x18`, so `[rsi+0x10]` is the length and `[rsi+0x28]` is
`ob_item[2]`. **[V]**

- The "reads 2" recorded for `0x22` in the unpublished wire-format note is **not two stream
  bytes**. It is the two `PyTuple_GetItem` index constants, `0` and `1`.
  **[V]** — worth correcting there.
- The header is required to be a tuple: `PyTuple_GetItem` raises on anything
  else and the handler bails on a NULL return. Minimum length 2. **[V]**
- The `0x22` reader is exactly the call `docs/reference/dbrow.md` inferred for
  `blue.DBRowDescriptor`: `callable = header[0]; args = header[1];
  PyObject_CallObject(callable, args)`. That inference is now **[V]**.

## 3. `0x23` — handler `0x1802de430` **[V]**

Function bounds `0x1802de430-0x1802de6b0` (640 bytes).

The handler reserves the shared slot before any child is read, reads the header
and takes `argsTuple = header[0]`, then `cls = argsTuple[0]` (`0x1802de4a9`,
`0x1802de4c0`). It looks `__new__` up on `cls` (name in `ctx[0x88]`) and calls
it with `*header[0]`, that is `cls.__new__(*argsTuple)` (`0x1802de4f9`), then
commits the slot. When `header->ob_size` is 1 or less there is no state
(`0x1802de530`); otherwise `state = header->ob_item[1]`. State goes in by the same
`__setstate__` / `__dict__.update` step as in `0x22`, and the list-append and
dict-setitem streams follow.

The key difference from `0x22`, and the thing easiest to get wrong:

- `0x23`'s second `PyTuple_GetItem` is applied to **`header[0]`, not to
  `header`** (`mov rcx, rax` at `0x1802de4bd` reuses the result of the first
  call). **[V]**
- The args tuple passed to `PyObject_CallObject` is **`header[0]` itself**,
  which still has `cls` as its element 0. So the call is
  `cls.__new__(cls, *args)` — the pickle protocol-2 `__newobj__` shape.
  **[V]**
- Minimum header: a 1-tuple whose single element is a tuple of at least 1
  element. **[V]**
- The "reads 1" recorded in the unpublished wire-format note is likewise the single
  `PyTuple_GetItem` index constant `0`, not a stream byte. **[V]**

`ctx[0x88]` is `"__new__"`: the MarshalStream constructor at `0x1802d9b60`
interns it there: the string at `0x18097cc40` goes through
`PyString_FromString` (`0x1802d9c4f`) and is stored at `ctx+0x88`
(`0x1802d9c5c`). **[V]**

### The constructor's string slots, in full **[V]**

Read off `0x1802d9b60`; these extend the `rcx` member table in
The unpublished marshal-tags note.

| off | string | used by |
|---|---|---|
| 0x60 | `__module__` | `0x17` writer, `0x02` writer |
| 0x68 | `__guid__` | *not referenced by any path examined here* **[?]** |
| 0x70 | `__getstate__` | `0x17` writer |
| 0x78 | `__setstate__` | `0x17`, `0x22`, `0x23` readers; `0x17` writer |
| 0x80 | `__dict__` | `0x17`, `0x22`, `0x23` readers |
| 0x88 | `__new__` | `0x23` reader |
| 0x90 | `__name__` | `0x23`/`0x22` writer, `0x02` writer |
| 0x98 | `append` | list-append stream reader |
| 0xa0 | `__setitem__` | dict-setitem stream reader |
| 0xa8..0xc8 | `Marshal::Load` / `Save` / `GetState` / `SetState` / `SaveCallback` / `LoadCallback` | Tracy zone names |

## 4. `0x17` — handler `0x1802dde40` **[V]**

Function bounds `0x1802dde40-0x1802de1be` (894 bytes).

The handler reserves the shared slot before any child is read, reads child 1, the
class name object, and resolves it with `ResolveGlobal` (`0x1802deb60`); the result
must have `ob_type == PyClass_Type`, otherwise the error is "global object %s
should be of class type but is of %s" (`0x1802ddf20`). A whitelist/audit hook
(`0x1802e1e70`) is called with "ClassType: %s". `PyInstance_NewRaw(klass, NULL)`
makes the instance (`0x1802ddfa8`), the slot is committed, and child 2, the state,
is read. `PyObject_HasAttr` probes the **class** for `__setstate__` (name in
`ctx[0x78]`); if present it is called on the **instance**. Otherwise
`inst.__dict__.update(state)` runs.

- **Exactly two children, in this order: the class name object, then the
  state.** Both are mandatory — a NULL from either `ReadObject` aborts.
  **[V]**
- **No list/dict append stream, no `0x2D`.** There is no call to
  `0x1802de6b0` or `0x1802de800` anywhere in the function. **[V]** (and see
  the exhaustive caller scan in §5).
- `PyInstance_NewRaw(klass, NULL)` means `__init__` is skipped and the
  instance dict starts empty. **[V]**
- The `__setstate__` probe is on the **class**, the call on the **instance** —
  unlike `0x22`/`0x23`, which probe the instance. A class-level
  `__setstate__` is therefore what matters for `0x17`. **[V]**
- `0x1802e1e70` builds `"ClassType: %s"` / `"TY_GLOBAL: %s"` /
  `"cPickle: %s.%s"` against `ctx[0xe8]`; it is the whitelist-collection /
  audit hook, not the enforcement point. Enforcement is inside
  `0x1802deb60` — see §6. **[V]**

## 5. The `0x2D` terminator and the two append streams **[V]**

`0x2D` is **not a type code**. The unpublished wire-format note already records its dispatch
slot as NULL ("Invalid type tag %d in stream."). It is read, and written, only
by two dedicated helpers.

### List-append stream — `0x1802de6b0`

The helper errors with "Can't read %d elements of %d bytes, only have %Iu bytes
left" when no bytes are left. Each round it **peeks** the next byte, storing the
position back unchanged (`0x1802de6f9`, `0x1802de6fd`). A byte equal to `0x2d`
(`0x1802de701`) ends the stream: the per-tag stats entry at `0xb4` (`0x2d * 4`) is
bumped, the position advances by one so the `0x2D` byte **is** consumed, and the
helper returns true. Otherwise `ReadObject()` reads a child, re-reading that byte
as a tag, and calls `obj.append(child)` (name in `ctx[0x98]`).

The peek is a genuine peek: `[rdi+0x30]` is re-stored with the value just
loaded from it. So a non-`0x2D` byte stays in the stream and `ReadObject`
consumes it as the child's tag. **[V]**

The comparison is `cmp cl, 0x2d` on the **raw byte**, with no `and 0x3f` and
no flag strip. A terminator byte carrying the shared bit (`0x6D`) would
therefore *not* be recognised. **[V]** — which is a strong cross-check that
the writer never produces one.

### Dict-setitem stream — `0x1802de800`

Same peek/terminator structure (`0x1802de879 cmp cl, 0x2d`, stats at
`+0xb4`, byte consumed at `0x1802e08a0`), but each iteration reads **two**
objects:

`ReadObject()` reads the FIRST object (`0x1802de8b1`) and the SECOND
(`0x1802de8d5`), then calls `PyObject_CallMethodObjArgs(obj, "__setitem__", FIRST,
SECOND, NULL)` with the name taken from `ctx[0xa0]`.

`PyObject_CallMethodObjArgs(o, name, a1, a2, NULL)` → `obj.__setitem__(FIRST,
SECOND)` → `obj[FIRST] = SECOND`. **The first object read is the KEY.** **[V]**

Note the terminator check happens *before* the first object of each pair, so
an odd number of objects is impossible — the terminator can only appear on a
pair boundary. **[V]**

### Who calls them

Exhaustive scan of every `.pdata` function in `blue.dll` for a direct
`call`/`jmp` to these addresses:

```text
0x1802de375 in 0x1802de1c0-0x1802de424 : call 0x1802de6b0     (0x22 handler)
0x1802de387 in 0x1802de1c0-0x1802de424 : call 0x1802de800     (0x22 handler)
0x1802de5e2 in 0x1802de430-0x1802de6b0 : call 0x1802de6b0     (0x23 handler)
0x1802de5f4 in 0x1802de430-0x1802de6b0 : call 0x1802de800     (0x23 handler)
```

Nothing else in the binary calls either. **[V]** So `0x2D` belongs to
`0x22`/`0x23` alone; `0x17` never produces or consumes one, and there is no
separate "ObjectEx reader" beyond these two handlers.

Both calls are unconditional on the success path of both handlers, so **both
terminators are always present** — `0x22`/`0x23` with no list items and no
dict items still end in `2D 2D`. **[V]**

## 6. The `0x17` class name: what it is, and the whitelist **[V]**

The resolver is `0x1802deb60` — the same helper tag `0x02` uses
(the unpublished marshal-tags note). It checks a resolution cache keyed by the
name object (`ctx[0x30]`), requires a **string** (else `RuntimeError "expected
string"`), splits the name at the last dot with `strrchr` (no dot means the module
`"__builtin__"`), imports the module and takes the attribute. The whitelist check
at `0x1802e1ca0` does `PyDict_GetItem(globalsWhitelist, obj)` and, on a miss,
raises `RuntimeError "HACKER WARNING! object %s.%s is not in whitelist"`; a
blacklist check with `"HACKER WARNING! object %s.%s is blacklisted"` sits next to
it. The result is cached.

So the `0x17` child #1 is **a marshal string holding a dotted
`module.ClassName`** — not a `(module, name)` tuple, not a shared-table
reference, not the `__guid__`. **[V]**

The client's own `carbon/common/lib/whitelist.py` (decompiled from
`code.ccp`) confirms the key space exactly: `InitWhitelist()` does
`mod, obj = item.rsplit('.', 1); mod = __import__(mod, ..., [obj], -1);
obj = getattr(mod, obj); res[obj] = None`, then
`blue.marshal.globalsWhitelist = res`. The dict is keyed **by the resolved
object**, and the strings it is built from are the dotted paths a server may
use.

**Size, corrected [V].** The list below is a selection, not the whole
whitelist, and an earlier reading of this file reported it as "92 entries".
`whitelist.py` concatenates **four** literals with `+=` — base 92, common 72,
server 88, client 6 — and adds the server blob only when
`boot.role == 'server'`. This build is `client`, so the real whitelist is
92 + 72 + 6 = 170 entries, **169 unique**
(`carbon.common.script.net.moniker.Moniker` appears twice). All 169 are in
`eve-rowset::whitelist::GLOBALS`.

**[V]** Relevant entries for the packet envelope:

```text
carbon.common.script.net.machoNetPacket.CallReq
carbon.common.script.net.machoNetPacket.CallRsp
carbon.common.script.net.machoNetPacket.ErrorResponse
carbon.common.script.net.machoNetPacket.IdentificationReq
carbon.common.script.net.machoNetPacket.IdentificationRsp
carbon.common.script.net.machoNetPacket.Notification
carbon.common.script.net.machoNetPacket.PingReq
carbon.common.script.net.machoNetPacket.PingRsp
carbon.common.script.net.machoNetPacket.SessionChangeNotification
carbon.common.script.net.machoNetPacket.SessionInitialStateNotification
carbon.common.script.net.machoNetPacket.TransportClosed
carbon.common.script.net.machoNetAddress.MachoAddress
carbon.common.script.sys.row.Row
carbon.common.script.sys.crowset.CRowset / CIndexedRowset / CFilterRowset
utillib.KeyVal
blue.DBRow, blue.DBRowDescriptor
```

**`MachoPacket` itself is *not* whitelisted** — only its subclasses. **[V]**

---

## 7. The writer — what we actually have to emit

### 7.1 Dispatch **[V]**

`Marshal::WriteObject` = `0x1802dee00` (`0x1802dee00-0x1802dfbfc`).

An object whose type is `PyInstance_Type` goes to the `0x17` writer (`0x1802dff60`).
After the other type tests, a shared / back-reference test runs (`0x1802db0f0`),
then the ObjectEx writer (`0x1802e02a0`, the `0x22`/`0x23` catch-all) is tried. If
that declines, a message "defaulting to cPickle for object %p of type %s" is
formed and the tag is `0x21`, the cPickle fallback.

So: **old-style instance → `0x17`; anything otherwise unrecognised → try
`__reduce_ex__`/`__reduce__` → `0x22` or `0x23`; if that declines →
`0x21` (cPickle).** `0x1802e02a0` is called from exactly one site, and
`0x1802dff60` from exactly one site (exhaustive `.pdata` scan). **[V]**

Also `cmp rdx, [PyType_Type]` at `0x1802defb2 → call 0x1802dfc70`: **a type
object is written as tag `0x02`, a global by `"%s.%s"` of `__module__` and
`__name__`** (`0x1802dfe28`). **[V]** That is how the `0x22` header's
callable is encoded in practice, confirming `docs/reference/dbrow.md`'s
`(<global 'blue.DBRowDescriptor'>, (cols,))` inference.

### 7.2 Tag emission and the shared flag **[V]**

`WriteTag` = `0x1802e2310`:

`WriteTag` bumps the save-side per-tag stats array (a pointer at `ctx[0xf0]`) at
index `tag & 0x3f`, reads and clears the pending flag bits from `stream[0x60]`,
and writes `flag | tag` as one byte (`0x1802e233c`).

The shared bit is staged in `stream[0x60]` and OR'd into the *next* tag byte;
`0x1802db0f0` sets `stream[0x60] = 0x40` (`0x1802db27f`) for a repeat object,
or retro-actively ORs `0x40` into an already-written tag byte
(`0x1802db1d5: or byte [buf + off], 0x40`) and then emits a `0x1B`
back-reference (`0x1802db215: or esi, 0x1b`). None of this is specific to the
object codes — a server may emit `0x17`/`0x22`/`0x23` with the flag clear and
simply never use `0x1B`. **[V]**

### 7.3 The `0x17` writer — `0x1802dff60` **[V]**

If `obj->ob_refcnt > 1` (`0x1802dff89`) the shared test (`0x1802db0f0`) may emit a
`0x1B` back-reference instead. If the save callback (`stream[0x70]`) is set it is
called with the object, and a result that is not `None` is written as tag `0x19`
followed by that object. Otherwise the real `0x17` is written: the class dict's
`__module__` is fetched (error "Instance %s has no __module__ attribute." if
absent), the name is built as `"%s.%s"` of the module and the class's `cl_name` and
interned, `WriteTag(0x17)` is emitted, then child 1 is the name, and child 2 is
`obj.__getstate__()` when the instance has `__getstate__` (name in `ctx[0x70]`)
and `inst.__dict__` otherwise.

`PyInstanceObject{ob_refcnt 0, ob_type 8, in_class 0x10, in_dict 0x18}` and
`PyClassObject{..., cl_dict 0x18, cl_name 0x20}`. **[V]**

Two things fall out:

- The name is `__module__ + "." + __name__`, **not** `__guid__`. **[V]**
- The save callback (`stream[0x70]`) is consulted **only here** — no other
  branch of `WriteObject` reads it. So tag `0x19` is reachable only for
  old-style instances, which supports (but does not prove the full scope of)
  gap 9 in the unpublished marshal-tags note. **[V]** for the mechanism, **[I]** for "`0x19`
  is the bound-object carrier".

### 7.4 The `0x22`/`0x23` writer — `0x1802e02a0` **[V]**

Signature `(ctx = rcx, bool *handled = rdx, stream = r8, obj = r9)`, from the
home-slot spills at `0x1802e02a3..0x1802e02ab` and the reloads at
`0x1802e0586` (`[rsp+0xc8]` → `*handled = 1`), `0x1802e0594`
(`[rsp+0xd0]` → stream) and `0x1802e059f` (`[rsp+0xc0]` → ctx).

The writer sets `*handled = false`, calls `obj.__reduce_ex__(2)` and, on failure,
clears the error and calls `obj.__reduce__()` (`0x1802e0313`). The result must be
a tuple; trailing `None`s are stripped and the length `n` must satisfy
`2 <= n <= 5`, else it bails (`0x1802e0386`). `callable = r[0]` must pass
`PyCallable_Check`, `args = r[1]` must be a tuple, and `state`, `listitems` and
`dictitems` are `r[2]`, `r[3]`, `r[4]` when present (`None` counts as absent). The
callable's `__name__` (name in `ctx[0x90]`) is compared with `"__newobj__"`; a
match sets `useNewObj` (`0x1802e0419`).

Header construction — the two branches:

- `useNewObj`, tag `0x23` (`0x1802e04c4`): a tuple of size `1 + (state ? 1 : 0)`
  (`0x1802e04cf`) with `header[0] = args` (the whole `(cls, *a)` tuple,
  `0x1802e04df`) and `header[1] = state` only if there is state
  (`0x1802e04ec`).
- Otherwise, tag `0x22` (`0x1802e0510`): a tuple of size
  `2 + (state ? 1 : 0)` (`0x1802e051b`) with `header[0] = callable`
  (`0x1802e0535`), `header[1] = args` (`0x1802e053d`) and `header[2] = state`
  only if there is state (`0x1802e054a`).

Emission:

Emission: it sets `*handled = true`, writes `WriteTag(0x22 or 0x23)` and then
`WriteObject(header)`, then each item of `listitems` (via `PyIter_Next`). **Terminator
#1** follows: the pending flag bits are OR'd with `0x2d`, cleared, and the byte is
written **unconditionally** (`0x1802e06e8`). The dict-items helper `0x1802e0840`
then iterates `dictitems`, requires each item to be a 2-tuple (otherwise the error
is "iterator should return 2-tuple"), writes `item[0]`, the KEY, then `item[1]`,
the VALUE, and writes **Terminator #2**, also unconditional (`0x1802e0961`).

Both terminators are written on every path through the function, reached
whether or not the corresponding iterator exists. **[V]** That is the writer
mirror of §5.

The `__reduce__` tuple → tag mapping, summarised **[V]**:

| `__reduce__` returns | tag | header written |
|---|---|---|
| `(callable, args)` | `0x22` | `(callable, args)` |
| `(callable, args, state)` | `0x22` | `(callable, args, state)` |
| `(copyreg.__newobj__, (cls, *a), ...)` | `0x23` | `((cls, *a),)` / `((cls, *a), state)` |
| 4th/5th element present | — | becomes the list-/dict-append streams |
| `state is None` | — | slot omitted, header one shorter |
| trailing `None`s | — | stripped before the length test |

The `0x23` selection is purely `callable.__name__ == "__newobj__"`; the
callable itself is then **dropped** from the header, because the reader
re-derives it as `cls.__new__`. **[V]**

---

## 8. Byte-level sketches for an encoder

`<obj>` means a complete nested marshal object (tag byte + payload).

### `0x17`, old-style instance

```text
17                              tag (53 if you also set the shared bit)
<obj:string "module.ClassName"> the class, dotted, whitelisted
<obj:state>                     __setstate__ argument, or a dict to .update()
```

Worked example, a `MachoPacket` subclass. `class MachoPacket():` in
`carbon/common/script/net/machoNetPacket.py` has no bases, so it is an
**old-style** class in Python 2.7 and every packet goes down the `0x17` path,
**not** `0x22`. It defines `__getstate__`/`__setstate__`, so the state is the
14-tuple the unpublished wire-format note already documents. **[V]**

```text
17                                              ; tag 0x17
  13 2F "carbon.common.script.net.machoNetPacket.CallReq"
                                                ; 0x13 = string, u8 len 0x2F, 47 bytes
  14 0E                                         ; 0x14 = tuple, 14 elements
     <command> <source> <destination> <userID> <body> <oob> <contextKey>
     <journeyID> <trace_id> <parent_span_id> <sampled> <ingress_id>
     <sample_rate> <cut_parent>
```

`source`/`destination` are `MachoAddress`, also an old-style class with
`__getstate__`/`__setstate__` (`machoNetAddress.py`), so each nests the same
`0x17` shape. **[V]**

The string tag: the writer's string branch picks `0x0E` (empty), `0x0F` (one
char), `0x11` (string-table hit, `0x1802df4dd`) or `0x13`
(`0x1802df54b`, `0x1802df806`). A 47-byte class name takes **`0x13`** with
the escaped-length encoding of the unpublished marshal-tags note. **[V]**

**Trap: `__module__` is whatever the class body says, not the file path.**
`eve/common/script/sys/strangeStrings.py` sets `__module__ = 'sys'` in the
class body, and the whitelist duly lists both `sys.A` and
`eve.common.script.sys.strangeStrings.A`. The `0x17` writer reads
`klass->cl_dict["__module__"]` (`0x1802e00de`), so an overriding assignment
wins. Take the name from the whitelist text, not from the module's location.
**[V]**

### `0x22`, 2.2 form

```text
22
  14 02  <obj:callable>  <obj:args-tuple>              ; no state
; or
  14 03  <obj:callable>  <obj:args-tuple>  <obj:state> ; with state
2D                                                      ; end of list-append stream
2D                                                      ; end of dict-setitem stream
```

With append streams:

```text
22
  14 02 <callable> <args>
  <item0> <item1> ... 2D
  <key0> <val0> <key1> <val1> ... 2D
```

Worked example, `blue.DBRowDescriptor` (cross-check against `docs/reference/dbrow.md`):

```text
22
  14 02
     02 <len> "blue.DBRowDescriptor"        ; tag 0x02, global by name
     14 01                                   ; the 1-tuple of args
        14 <ncols>                           ;   the tuple of columns
           14 02 <name> <dbtype> ...
  2D
  2D
```

### `0x23`, 2.3 form

```text
23
  14 01  <obj: tuple (cls, *args)>                       ; no state
; or
  14 02  <obj: tuple (cls, *args)>  <obj:state>          ; with state
2D
2D
```

Note the inner tuple **includes `cls` as its first element**: the reader does
`cls.__new__(cls, *args)` by handing that whole tuple to `PyObject_CallObject`.
A new-style class with no `__getnewargs__` reduces to
`((cls,), state)` — inner tuple of length 1. **[V]**

### Decoder side, the minimum

```text
0x17: name = read_object(); state = read_object()
0x22: h = read_object(); obj = h[0](*h[1]); if len(h)>2: set_state(obj, h[2])
      read_list_stream(obj); read_dict_stream(obj)
0x23: h = read_object(); obj = h[0][0].__new__(*h[0]); if len(h)>1: set_state(obj, h[1])
      read_list_stream(obj); read_dict_stream(obj)

read_list_stream: loop { if peek()==0x2D { pos+=1; return } obj.append(read_object()) }
read_dict_stream: loop { if peek()==0x2D { pos+=1; return }
                         k=read_object(); v=read_object(); obj[k]=v }
```

### Shared registration order **[V]**

All three call `RegisterShared(stream, 0)` (`0x1802dbdf0`) **before** reading
any child, and commit with `0x1802dbf30` only after the object exists
(`0x1802de2b1`, `0x1802de51e`, `0x1802ddfd0`). The unpublished marshal-tags note already
records this; it is re-confirmed here. A decoder must claim the ID at the tag
byte, not after the payload, or the numbering drifts.

---

## 9. Does this match the folklore?

Yes, with two refinements worth stating because they are the parts people get
wrong, and both are read off this build rather than imported:

- The canonical "ObjectEx" description — header object, list stream to a
  marker, dict stream to a marker, two marker bytes — **is this build's
  behaviour** for `0x22`/`0x23`, and `0x2D` is that marker. Established here
  at `0x1802de701` / `0x1802de879` (read) and `0x1802e06c0` /
  `0x1802e0961` (write).
- The canonical description of `0x17` as `(class, state)` is right, but the
  class is specifically a **dotted `module.ClassName` marshal string resolved
  through `globalsWhitelist`**, and `0x17` has **no** append streams and
  **no** terminator. Establishing that required the exhaustive caller scan in
  §5, because "ObjectEx has a terminator" invites assuming all three do.

## 10. Still open

1. **`ctx+0x68` is `__guid__`, and nothing in the paths examined here reads
   it.** **[?]** It is interned by the constructor at `0x1802d9c08`. If some
   other path prefers `__guid__` over `module.ClassName` for the `0x17` name,
   a server emitting the dotted path would still be accepted (the resolver
   only ever sees a string), so this is a curiosity, not a blocker. Settled
   by: an xref scan for `[reg+0x68]` loads across `blue.dll` paired with a
   `PyObject_GetAttr`/`PyDict_GetItem`.
2. **Whether the client ever emits a terminator byte with the shared bit set
   (`0x6D`).** **[I]: no.** The mechanism permits it (`or ebx, 0x2d` over
   `stream[0x60]`), but the reader's `cmp cl, 0x2d` would reject it, so the
   writer cannot be producing one in any working path. Settled by: proving
   `stream[0x60]` is necessarily zero at `0x1802e06bc` — i.e. that
   `0x1802db0f0` is never the last thing to touch it before a terminator.
3. **The exact `0x22` vs `0x23` choice for the types a server actually
   sends.** We have the rule (`callable.__name__ == "__newobj__"`), but which
   rule fires for, say, `util.KeyVal` or `crowset.CRowset` depends on each
   class's `__reduce__`, which was not read for every whitelisted type.
   **[?]** Settled by: decompiling each class's `__reduce__`/`__getstate__`,
   or one live capture.
4. **`0x1802e1e70`'s `ctx+0xe8` collection list** — it builds
   `"ClassType: %s"` strings and `PyDict_SetItem`s them somewhere. Looks like
   `collectWhitelist` telemetry (`whitelist.py` sets
   `blue.marshal.collectWhitelist = False`). Not read in full. **[?]** It is
   on the `0x17` path, so if it can *fail* it matters; the call's return is
   tested (`0x1802ddf98 test al, al`), and a `false` aborts the read.

## 11. Things found here that contradict or extend other documents

Reported, not edited — these files are other tracks' lanes.

1. **The unpublished wire-format note**, "Packet envelope — confirmed": *"These are old-style
   classes serialized as `(guid-object, state)` via the native OBJECT tag."*
   The class is identified by **`__module__ + "." + __name__`**, not by
   `__guid__` (`0x1802e00de`/`0x1802e0136`/`0x1802e0142`). For `CallReq` that
   is `carbon.common.script.net.machoNetPacket.CallReq`, not
   `macho.CallReq`. The whitelist in `carbon/common/lib/whitelist.py` lists
   the dotted paths, confirming it independently. **[V]**
2. **The unpublished wire-format note**, type table: `0x22` "reads 2" and `0x23` "reads 1" are
   not stream reads. They are `PyTuple_GetItem` index constants. Neither code
   reads any raw bytes beyond its tag. **[V]**
3. **The unpublished marshal-tags note**, "Dispatch": *"Per-tag statistics at `ctx+0x108`
   (64 × u32)"*. `ctx+0x108` is a **pointer to** that array, dereferenced
   first: `0x1802ddab6 mov rax, [r14+0x108]; 0x1802ddabd inc dword
   [rax+rcx*4]`. The save-side equivalent is `ctx+0xf0`
   (`0x1802e231a`). **[V]**
4. **The unpublished marshal-tags note**, stream member table: it lists `stream+0x70` as the
   cached `cPickle.Unpickler` for `0x21`. In the **save** direction
   `stream+0x70` is the **Save callback** (`0x1802dffce`, `0x1802e0023`) and
   `stream+0x68` is the cached `cPickle.Pickler` (`0x1802dfa79`,
   `0x1802dfad3`, `0x1802dfaf0`). Either the field is reused per direction or
   one of the two offsets is off by one slot; worth a look. **[V]** for what
   the writer does.
5. **The unpublished marshal-tags note gap 6** ("`0x0D` vs `0x13` — what makes the encoder
   pick one is unknown"): the writer's string branch emits **`0x0E`, `0x0F`,
   `0x11` or `0x13`** and there is no `mov r8d, 0xd` anywhere in
   `Marshal::WriteObject`. On this evidence **the client never emits `0x0D`**
   — `0x13` is the general string tag on the wire. **[I]**, because only the
   immediate tag constants in `WriteObject` were enumerated; a sub-writer
   could in principle carry it.
6. **The unpublished marshal-tags note gap 9** ("`0x19` as the bound-object carrier"): the
   save callback is consulted at exactly one place in the whole writer — the
   `0x17` branch — and a non-`None` return makes the object a tag `0x19`
   instead. So `0x19` is produced only for old-style instances offered to
   `blue.marshal.Save(obj, callback)`. **[V]** for the mechanism; the
   "bound object / `GetObjectID`" identification stays **[I]**.
7. **`docs/reference/dbrow.md` §2**, the `0x22` descriptor emission marked **[I]**
   (*"a server should therefore emit the descriptor as an ObjectEx (tag
   `0x22`) whose header tuple is `(<global 'blue.DBRowDescriptor'>, (cols,))`"*)
   is now **[V]** on the reader side: `0x1802de239`/`0x1802de253`/
   `0x1802de268` are literally `callable = header[0]; args = header[1];
   PyObject_CallObject(callable, args)`, and `0x1802defb2 → 0x1802dfc70`
   shows a type object is written as tag `0x02`. The one thing to add is the
   **two trailing `0x2D` bytes**, which `docs/reference/dbrow.md`'s sketch omits.
