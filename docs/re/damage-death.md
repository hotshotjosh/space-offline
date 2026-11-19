# Taking damage, and dying — an implementation spec

Lane: `damage-death`. Research only. The two files this lane may write are
this one and the unpublished damage-death lane note; no code was written and no other
document was touched.

**The question.** One shot lands on another player's ship, repeatedly, until
it explodes, leaves a wreck, and the pilot is in a capsule. What exactly goes
on the wire, in which direction, in what order — and, because this is the
first feature where **destiny** and **godma** both have to move, which of the
two owns which pixel.

Marking convention:

- `[V]` verified against **our** client, build V24.01 / 3619879: a named code
  object in a named `.pyc`, cited at the line `tools/dis.py` reads out of
  `co_lnotab`, so the number matches the client's own tracebacks. Shipped
  static data (`dialogs`, `dogmaeffects`, the localisation corpus) counts.
- `[V2]` a third party's decode of a different build. None is load-bearing
  here; there is one, flagged.
- `[I]` inferred, and says from what.
- `[A]` an assumption we choose to ship, with what would falsify it.

An unmarked claim is a defect. Bodies quoted below were read with
`uncompyle6` for legibility and every line number was taken from
`tools/dis.py`, never from the decompiler's own output.

Contents: §0 the minimum viable kill · §1 the two gauge families, settled ·
§2 the combat message · §3 recharge · §4 the death sequence · §5 pod and
clone · §6 the wreck · §7 who may shoot whom · §8 falsifiable tests · §9 open
questions, ranked · §10 what I could not settle, and corrections.

Companion documents, read before this one and **not** restated here:
The unpublished addballs lane note (`AddBalls2`, `RemoveBall`/`RemoveBalls`, the
`exploders` trap, the slim-item key list), `docs/re/targeting.md` (the lock
handshake and `OnTarget`), the unpublished port tracker §23 (both gauge families and
the two defects already fixed in them), `docs/reference/bound-objects.md` §3 (`dogmaIM`).

---

## 0. The minimum viable kill

Alice shoots Bob. Carol is a third party on the same grid. All three are
undocked in one solar system, all three balls are in all three parks, and
Alice has Bob locked (`docs/re/targeting.md`).

**What carries each gauge family, in one line each, because this is the thing
this lane exists to say:**

| the player sees | fed by | written by | wire vehicle |
|---|---|---|---|
| **the bars** — tactical overlay, overview, target bracket, selected item | `michelle.Park.damageState[shipID]` | `SetState`, `AddBalls`, `AddBalls2`, **`OnDamageStateChange`**, `OnFleetDamageStateChange` — *and nothing else* `[V]` | a **destiny action** inside `DoDestinyUpdate` |
| **the three rings** round the capacitor, own ship only | godma attributes 263/264, 265/266, 9/3 on the ship item | `OnModuleAttributeChange` / `OnModuleAttributeChanges` | a **dogma message**: a plain notification, or the `dogmaMessages` third argument of the same `DoDestinyUpdate` |

So the answer to the lane's central question is **both, and they are
separate pipes that happen to be able to share one packet.** §1 proves the
"nothing else", and §1.4 says why the packet-sharing is the design we want.

### 0.1 The exchange, ordered

`S` = server, `A` = Alice (shooter), `B` = Bob (victim), `C` = Carol
(observer). "destiny entry" means one `(stamp, (funcName, argsTuple))` element
of a `DoDestinyUpdate` batch; §1.2 has the envelope.

| # | dir | message | why |
|---|---|---|---|
| 1 | A→S | `dogmaIM` bound object, `Activate(moduleItemID, 'projectileFired', bobShipID, repeat)` | the only client→server call in the whole feature. `godma.Activate` → `self.GetDogmaLM().Activate(itemID, effectName, target, repeat)`, `godma.py:2441` `[V]`, `findings/client-calls.tsv:511` |
| 2 | S→A,B,C | destiny entry `('OnSpecialFX', (shooterShipID, gunItemID, gunTypeID, bobShipID, None, 'effects.ProjectileFired', True, 1, 0, cycleMs, repeat, startTime, 0, None))` | the turret visibly fires. 14 args, §2.4 |
| 3 | S→A,B | dogma message `('OnGodmaShipEffect', gunItemID, 34, t, 1, 0, None, startTime, durationMs, repeat, None, None)` | the module button cycles. Only the module's **owner** needs it |
| 4 | S→A,B,C | destiny entry `('OnDamageStateChange', (bobShipID, ((shieldFrac, tauMs, 0.0), armourFrac, hullFrac)))` | **the bars.** §1 |
| 5 | S→B | dogma messages, one per changed attribute: `('OnModuleAttributeChange', ownerID, bobShipID, 264, time, newValue, oldValue, wallclockTime)` — and 266 and 3 as the layers are eaten | **the rings.** §1.3. `ownerID` **must** be Bob's `charid` |
| 6 | S→A | notification `OnDamageMessage({'damage': 42.0, 'target': bobShipID, 'weapon': gunTypeID, 'hitQuality': 4, 'isBanked': False, 'attackType': 'me'})` | "42 to Bob's Punisher (Gatling Autocannon II) - Hits". §2 |
| 7 | S→B | notification `OnDamageMessage({'damage': 42.0, 'source': aliceShipID, 'attackerID': aliceShipID, 'hitQuality': 4, 'isBanked': False, 'attackType': 'otherPlayerWeapons'})` | "42 from Alice". Different key set, not a mirror. §2.2 |
| — | | *repeat 2–7 until the server's own hull number reaches zero* | |
| 8 | S→A,B,C | destiny entries, **one batch, one stamp**: `('TerminalPlayDestructionEffect', (bobShipID, 3))` **and** `('RemoveBalls', ([bobShipID],))` — or, simpler and trap-free, the single entry `('RemoveBall', (bobShipID, True))` | the explosion and the ball's removal. The unpublished addballs lane note §3 |
| 9 | S→A,B,C | destiny entry `('AddBalls2', ((wreckBlob, {wreckID: ({…slim…, 'launcherID': bobShipID, 'isEmpty': False, 'lootRights': (bobCharID, bobCorpID, None, False)}, None)}),))`, **in the same batch as 8 or before it** | the wreck. Its `launcherID` must still resolve to Bob's ball. §6 |
| 10 | S→A,B,C | destiny entry `('AddBalls2', ((podBlob, {podItemID: ({'itemID': podItemID, 'typeID': 670, 'ownerID': bobCharID, 'charID': bobCharID, 'corpID': bobCorpID}, ((1.0, 0.0, 0.0), 1.0, 1.0))}),))` | Bob's capsule appears for everybody |
| 11 | S→B | `OnMultiEvent`/notification: the capsule's godma item and its ten dogma attributes (`Registry::capsule_dogma_row`, port-tracker §23.5) | so Bob's rings are not three empty circles |
| 12 | S→B | **session change** `{'shipid': (bobOldShipID, podItemID)}` | `gameui._HandleSessionChangeInSpace:782` re-points `bp.ego`. **This is the one message without which Bob stares at a dead ship.** §4.2 |
| 13 | S→A,C | notification `OnTarget('lost', bobOldShipID, 'Exploding')` to anyone who had him locked | `docs/re/targeting.md` §3.3 |
| 14 | S→B | notification `OnPodOrCorvetteAssembly(podItemID, 670)` | cosmetic skin. Optional `[I]` |

If Alice then kills the pod, append:

| # | dir | message | why |
|---|---|---|---|
| 15 | S→A,B,C | `('TerminalPlayDestructionEffect', (podItemID, 3))` + `('RemoveBalls', ([podItemID],))`, one stamp | the pod pops |
| 16 | S→B | notification `OnPodKilled(podItemID)` | `gameui:1203` just **stores** it, so that step 18's session change is recognised as a podding. §5.1 |
| 17 | S→B | notification `OnPlayerPodDeath()` — **zero arguments** | the white-out: explode own ball, camera shake, `loading.FadeIn`. `evespacescene/playerdeath.py:56` `[V]` |
| 18 | S→B | **session change** `{'shipid': (podItemID, newCloneShipID), 'stationid': (None, cloneStationID), 'solarsystemid': (sysID, None), …}` | the actual clone jump. `wasPodded` is then true and the view transition reason becomes `'clone'` (`gameui:505`) |
| 19 | S→A,B,C | `AddBalls2` for Bob's **corpse** (`groupBiomass` 14) and `RemoveBalls` for the pod ball | optional flavour `[I]` |

### 0.2 The three things most likely to bite

1. **Step 4 cannot be sent as a notification.** `OnDamageStateChange` is a
   method of `michelle.Park`, which is not a service and has no
   `RegisterNotify`; michelle's own `__notifyevents__` (`michelle.py:151-161`)
   does not list it `[V]`. The only door is a `DoDestinyUpdate` batch. So
   **the bars are destiny's property and the rings are godma's, and a server
   that updates only one of them produces exactly the class of bug
   port-tracker §23 has now recorded twice.**
2. **The bars are polled, not pushed**, at 1 Hz
   (`baseTacticalEntry.UpdateDamage:44`, timer at `:68` → `AutoTimer(1000, …)`
   `[V]`) and the rings at 2 Hz wallclock
   (`shipUI.UpdateGauges:1342`, the sleep at `:1352` → `SleepWallclock(500)` `[V]`).
   There is **no** mechanism by which the two families can be made atomically
   consistent, so the requirement is *eventual* consistency within about a
   second. Do not design a two-phase update to get something the client
   cannot show.
3. **One stamp per batch, still.** `DoDestinyUpdate:659-666` builds
   `timestamps = {action[0] for action in state}` and calls
   `clientStatsSvc.OnFatalDesync()` if `len(timestamps) != 1` `[V]`. Every
   entry in steps 2, 4, 8, 9 and 10 that travels in one call must carry the
   same number. But see §1.2: damage-shaped actions are exempt from the
   *clock advance* rule, which is a real saving.

---

## 1. The two gauge families, settled

### 1.1 The bars: five writers, and that is the whole list

`michelle.Park.damageState` is `{itemID: (damageState, clientSimTime)}`. Every
write in the file `[V]`:

| line | writer | what it stores |
|---|---|---|
| `michelle.py:1266-1268` | `Park.SetState` | `damageState[k] = (v, blue.os.GetSimTime())` for each `k, v` in the blob's `bag.damageState` |
| `:1671` | `Park.AddBalls` | from the third element of the 3-tuple, `{ballID: damageState}` |
| `:1700` | `Park.AddBalls2` | from `extraBallData`'s pair form |
| `:2076` | `Park.OnDamageStateChange` | `damageState[shipID] = (damageState, blue.os.GetSimTime())` |
| `:2083` | `Park.OnFleetDamageStateChange` | identical store, plus its own scatter |

and three deletes, in `RemoveBall:1783`, `RemoveBalls:1858` and the three
`= {}` resets. Nothing else in the build touches it: `GetDamageState:1302` is
the only reader and the only consumers of *its* return are
`baseTacticalEntry.GetDamage:83`, `targetInBar.TargetHealthBars.UpdateDamage`
(`targetInBar.py:1168`, reading `GetDamageState` at `:1173`), `drone.py`,
`droneEntry.py`, `moondefencebutton.py` and `rwinfopanelhaulertaskentry.py` `[V]` (`/usr/bin/grep -rl SetDamageState`,
`-rl CalculateCurrentDamageStateValues`).

`Park.OnDamageStateChange(shipID, damageState)`, `michelle.py:2070`,
`argcount 3`, no defaults `[V]`:

`OnDamageStateChange` (`michelle.py:2071-2077`) logs the call, stores
`(damageState, current sim time)` in `self.damageState[shipID]`, and scatters
`OnDamageStateChange` with `shipID` and `GetDamageState(shipID)`. `[V]`

Two consequences worth having in writing:

- **Our stamp is discarded.** The entry's destiny timestamp is not used; the
  client timestamps the value with *its own* `GetSimTime()` at the moment it
  reads it. This is the same mechanism port-tracker §23.2 found in
  `SetState`, and it is why §3's recharge curve starts when the packet lands
  rather than when we computed it.
- **The scattered value is the decoded five-list, not what we sent.**
  `GetDamageState` returns
  `CalculateCurrentDamageStateValues(state, time) + list(state[-2:])` =
  `[shield, armour, hull, armour, hull]` (`:1302`, and the duplication is
  load-bearing for `progression.healthwidgets`, port-tracker §23.2) `[V]`.

The payload is the same `damageState` our `SetState` already sends, with the
decode port-tracker §23.1 corrected: `((shieldFrac, tauMs, anything), armourFrac, hullFrac)`.
`CalculateCurrentDamageStateValues` (`eveInflight/damageStateValue.py:7`) reads
`shieldValues[:2]` as `(shieldHealth, tau)`, appends it, then
`ret + list(damageState[-2:])` `[V]`. Fractions, `1.0` is full.

**`OnFleetDamageStateChange(shipID, damageState)`** (`:2080`) is a second,
equivalent door to the same dict — it stores identically and additionally
scatters its own name, which `fleetWatchList` consumes `[V]`. Use it for
fleet members who are **not** on grid; use `OnDamageStateChange` for the ones
who are. Both are in `nonDestinyCriticalFunctions`.

### 1.2 The envelope, and the one clock rule that relaxes

`michelle.DoDestinyUpdate(state, waitForBubble, dogmaMessages=None)`,
`michelle.py:625` `[V]`:

`DoDestinyUpdate`, by line `[V]`:

- `:632`: return if there is no ballpark.
- `:635-637`: if `dogmaMessages` is set, scatter them as `OnMultiEvent`.
- `:644-655`: expand every `PackagedAction` entry by unmarshalling its payload;
  other entries pass through.
- `:659-666`: collect the set of timestamps; if it holds more than one value,
  report a fatal desync, which does not return.
- `:668`: otherwise flush the state into the ballpark.

So **one `DoDestinyUpdate` can carry destiny actions and dogma events at
once**, and `dogmaMessages` is handed straight to godma's `OnMultiEvent`
without touching the park. That is the mechanism by which a real server keeps
the two gauge families from drifting, and it is available to us for free.

`Park.RealFlushState(state)`, `:1507`, then dispatches each entry
`(eventStamp, (funcName, args))` `[V]`:

`RealFlushState` `[V]`, by line:

- `:1515-1518`: it takes the first entry's `(funcName, args)`; a `SetState` entry
  goes to `self.SetState(*args)` (`:1525`).
- `:1527`, `:1594`: when the park is not in a valid state it logs "Events ignored"
  and returns.
- `:1537`: it collects `exploders`, a map built from every
  `TerminalPlayDestructionEffect` entry (key: the first argument; value: the last).
- `:1539-1544`: it then walks the entries again, skipping `SetState`.
- `:1551-1553`: a function in `nonDestinyCriticalFunctions` is called at once
  and the loop continues with the next entry.
- `:1558-1567`: otherwise it synchronises to the simulation time of the entry's
  stamp once; if that fails, it reports a recoverable desync, requests a reset
  and returns.
- `:1574-1578`: a function in `localActions` is called on the park itself; for
  `RemoveBalls`, `exploders` is appended to its arguments.
- `:1581`: any other function is called as `_parent_<funcName>`.
- `:1582-1583`: a function in `scatterEvents` is also announced as
  `OnBallparkCall`.

The three membership sets are built in `Park.__init__`, `michelle.py:1010`
`[V]`:

```
1035-1044  scatterEvents = ['Orbit','GotoDirection','WarpTo','SetBallRadius',
                           'GotoPoint','SetBallInteractive','SetBallFree',
                           'SetBallHarmonic','FollowBall','Stop']
1048-1057  nonDestinyCriticalFunctions = ['OnDamageStateChange','OnSpecialFX',
                           'OnFleetDamageStateChange','OnShipStateUpdate',
                           'OnSlimItemChange','OnDroneStateChange',
                           'OnSovereigntyChanged','OnDbuffUpdated',
                           'OnClientControllerEvent','OnDotVictimUpdated']
1060       localActions = ['AddBalls','AddBalls2','RemoveBalls','SetState',
                           'RemoveBall','TerminalPlayDestructionEffect']
```

**This is the find worth taking away.** `OnDamageStateChange`, `OnSpecialFX`
and `OnFleetDamageStateChange` are *non-destiny-critical*: the `continue` at
`:1553` means they are applied **without** `SynchroniseToSimulationTime` ever
being called, so a batch made only of them cannot produce
`OnRecoverableDesync` and cannot trigger `RequestReset` `[V]`. The uniform-stamp
rule (`DoDestinyUpdate:660`) still binds — that test is upstream — but a
combat server emitting 10 Hz of damage updates does **not** have to advance
the park clock in lockstep to do it. A batch of pure damage updates may repeat
the previous stamp.

Two preconditions, both `[V]`:

- `self.__bp is None` → the whole call is dropped at `:632`.
- `self.validState` is false → every entry is ignored at `:1527`, logged as
  `Events ignored`, and `validState` is only set true by
  `Park.SetState` (`:1260`). **A `SetState` must have landed first.** Our
  undock path already does this.

### 1.3 The rings: godma, own ship only, and a hard ownership check

`shipHud/hpGauges.Update:55` is three lines — `controller.GetShieldHPPortion()`,
`GetArmorHPPortion()`, `GetStructureHPPortion()` `[V]` — and the controller is
`ActiveShipController`, i.e. **the player's own ship and nothing else**. There
is no ring for anybody else's ship, so an observer of a kill needs §1.1 only;
the victim needs both. port-tracker §23.4/§23.5 has the attribute ids, the
float-vs-int trap, and the charged-attribute requirement; none of it changes
here.

What is new is the update message. `StateManager.OnModuleAttributeChange`,
`godma.py:1575` `[V]`:

`StateManager.OnModuleAttributeChange(ownerID, itemKey, attributeID, time,
newValue, oldValue, wallclockTime, scatterAttr=True)` is at `godma.py:1575`, and
its worker `OnModuleAttributeChange_` at `:1584` `[V]`:

1. `:1585`: return if `_IsAttributeChangeRelevant(itemKey, attributeID,
   wallclockTime)` says the change is stale.
2. `:1589`: when `wallclockTime` is set, remember it in `lastAttributeChange` for
   `(itemKey, attributeID)`.
3. `:1591-1598`: when neither `itemKey` nor `ownerID` is the session character,
   return if there is no session character, otherwise log a traceback ("Got
   moduleattributechange for object I do not own") and return.

So, `[V]`:

- **Seven positional arguments**, `scatterAttr` is the client's default.
- **`ownerID` must equal the receiving character's `charid`** (or `itemKey`
  must be the charid itself). Send the ship's *owner* here, not the ship.
  Get it wrong and the change is dropped after a traceback (`:1591`, `:1596`) — the ring simply
  does not move and the log says you do not own your own ship.
- `wallclockTime` of `None`/`0` makes `_IsAttributeChangeRelevant:2036`
  return `True` immediately (`if not wallclockTime: return True`), which is
  the safe value, exactly as port-tracker §23.5 concluded for the priming
  row. A *plausible-looking* non-`None` value that loses the comparison makes
  the client skip the attribute silently.

The batched form, `OnModuleAttributeChanges(changes)` (`godma.py:1620`) `[V]`,
reads the item key, attribute id and new value from positions 2, 3 and 5 of each
change (`:1630`), and calls `OnModuleAttributeChange_` for each with the rest of
the tuple plus a trailing `0` (`:1640`).

`each[1:]` is the seven arguments and the appended `0` is `scatterAttr`, so
each element is the **eight-tuple**
`('OnModuleAttributeChange', ownerID, itemKey, attributeID, time, newValue, oldValue, wallclockTime)`.
That is the same element shape `OnMultiEvent` carries, which is why both doors
accept the same rows.

`godma.OnMultiEvent`, `godma.py:268-272` `[V]`: it sorts the events by their last
element, groups them by it, and calls `OnMultiEvent_` once per group with the
first element of each event,
so the wire shape is **a list of `(eventTuple, groupingKey)` pairs**, and
`OnMultiEvent_:274` buckets by `ev[0]`, the event name: `OnDamageMessage` rows
are re-emitted as one `OnDamageMessages`, `OnModuleAttributeChange` rows go
through `BroadcastFilteredMAC:346` (sorted by `x[4]`, the `time`),
`OnGodmaShipEffect` rows through `BroadcastFilteredGSF:325` (which keeps only
the **last** row per `(itemID, effectID)` — `line[1], line[2]` — so sending a
start and a stop for the same module in one batch loses the start), and
anything else is `sm.ScatterEvent(*ev)` directly, or `sm.ChainEvent(*ev)` if
its name starts with `Process` `[V]`. `docs/re/targeting.md` §3.3 reached the
same shape independently from `targetMgr`'s side.

### 1.4 So: both, and how to keep them honest

Both families must be driven. They are *independent*: a server that sends
only `OnDamageStateChange` gives the victim correct bars on everyone's
overview and three motionless rings on his own HUD; a server that sends only
the attribute change gives him moving rings and a full bar that everybody
else can see.

They need only be **eventually** consistent, because both are polled (§0.2
point 2). The cheapest way to make them agree is to put them in one
`DoDestinyUpdate`: the destiny entry in `state`, the attribute rows in
`dogmaMessages`. One call, one stamp, no ordering question — and it is what
`DoDestinyUpdate:635` exists for.

`[A]` We should nonetheless derive both from **one** server-side number per
layer rather than from two code paths, and expose that number to tests as a
fraction and as an absolute pair. Falsified if a measurement of CCP's own
traffic ever shows the two families disagreeing by design (e.g. the bars
lagging a tick); nothing in the client suggests that.

---

## 2. The combat message the player reads

Three services subscribe to `OnDamageMessage`, and all three are in
`__notifyevents__` lists, so **a plain notification works** and we do not need
`OnMultiEvent` for it `[V]`:

| handler | line | what it does |
|---|---|---|
| `godma.OnDamageMessage` | `godma.py:603` | turret hit/miss flashes, then `logger.AddCombatMessageFromDict` — the text |
| `bracketMgr.OnDamageMessage` | `bracketMgr.py:1369` | accumulates `damage` per `attackerID` per second, blinks the attacker's bracket |
| `targetInBar.TargetHealthBars.OnDamageMessage` | `targetInBar.py:1302` | blinks the target's own health-bar widget |

The plural `OnDamageMessages(dmgmsgs)` exists in all three
(`godma.py:672`, `bracketMgr.py:1363`, `targetInBar.py:1294`) and each does
`self.OnDamageMessage(*msg[1:])`, so its elements are the `OnMultiEvent`
tuples `('OnDamageMessage', argsDict)` `[V]`. Send the singular.

### 2.1 The dictionary, key by key

`argsDict` is one dict. Required keys first.

| key | type | required? | read at |
|---|---|---|---|
| `hitQuality` | `int` 0..6+ | **mandatory** — bare subscript | `uilog.py:395` |
| `isBanked` | `bool` | **mandatory** — bare subscript | `uilog.py:396` |
| `damage` | `float` | **mandatory whenever `attackerID` is present** — bare subscript | `bracketMgr.py:1382` |
| `attackType` | `'me'` \| `'otherPlayerWeapons'` \| `'otherPlayer'` | defaults to `'me'`; **any fourth value aborts the message** with `LogError('attackType not valid!')` | `uilog.py:414-424` |
| `target` | ball/item id (`long`) | present iff `attackType == 'me'` | `uilog.py:428` |
| `source` | ball/item id (`long`) | present iff `attackType == 'otherPlayerWeapons'` | `uilog.py:428` |
| `owner` | `(2, ownerID)` — `2` is `const.UE_OWNERID` (`eveexceptions/const.py:3`) | present iff `attackType == 'otherPlayer'` | `uilog.py:432-433` |
| `attackerID` | ball id of the attacker | needed for the bracket blink and for the `owner` path's name lookup | `bracketMgr.py:1375`, `uilog.py:436` |
| `weapon` | **typeID**, not itemID | optional; `evetypes.GetName(weapon)` and `{[item]weapon.name}` | `uilog.py:559` |
| `observed` | any | optional; marks the row as a third party's shot, which only changes whose turrets flash | `godma.py:615` |
| `damageTypes` | anything | optional; re-scattered as `OnShipDamage(damageTypes=…)` | `godma.py:663` |
| `damageAttributes` | `{attributeID: amount}` | optional; `> 0` entries for attributes 114/116/117/118 add a damage-type icon | `uilog.py:382` |
| `deflectedDmg` | `float` | optional; **added to `damage`** before formatting and shown in a suffix | `uilog.py:535-539` |
| `sourceCharID` | charID | fallback name when the `source` ball is not in the park | `uilog.py:472` |
| `targetOwnerID` | ownerID | fallback name when the `target` ball is not in the park | `uilog.py:472` |

Two hazards `[V]`:

- **`AddCombatMessageFromDict` mutates the dict in place**: it writes
  `hitQualityText`, `typeName`, `tickerText`, `extraNameText`, replaces
  `args['target'|'source'|'owner']` with a **name string**, and adds
  `args['target_ID']` etc. (`uilog.py:408`, `:455-458`, `:467-468`). Every
  subscriber sees the same object, so handler order changes what the others
  read. `TargetHealthBars` copes by testing `isinstance(target, long)` and
  then `isinstance(target, basestring)` (`targetInBar.py:1315-1324`). Do not
  rely on a consumer seeing the id.
- **No ball and no `*CharID` fallback means no message at all.**
  `_GetObjectName:472` returns `None`, and `AddCombatMessageFromDict`
  `return`s for `source`/`target` (`uilog.py:461-466`) — silently, after one
  `LogError('Failed to display message', …)`. So either the attacker's ball
  is in the reader's park, or `sourceCharID` is on the wire.

### 2.2 Incoming, outgoing, and a miss

`msgKey` is assembled, not sent (`uilog.py:395-424`) `[V]`:

```
hitQuality == 0           -> 'AttackMiss'
0 < hitQuality <= 6       -> 'AttackHits', args['hitQualityText'] = GetByLabel('UI/Inflight/HitQuality%d' % hitQuality)   # GetHitQualityMessage:282
hitQuality  > 6           -> 'AttackHits', args['hitQualityText'] = ''
attackType == 'otherPlayer'        -> msgKey += 'RD'
attackType == 'otherPlayerWeapons' -> msgKey += 'R'
attackType == 'me' and isBanked    -> msgKey += 'Banked'
anything else                      -> LogError and return
```

All eight resulting keys exist in `dialogs.static`, read out of the shipped
corpus with `tools/loctable.py errors` `[V]`:

| key | body |
|---|---|
| `AttackHits` (3893) | `{faintColor}{fontMarkUpStart}to{fontMarkUpEnd} <b>{target}{extraNameText}</b>…` |
| `AttackHitsR` (3894) | `…from… <b>{source}{extraNameText}</b>…` |
| `AttackHitsRD` (3895) | `…from… <b>{owner}{extraNameText}</b>…` |
| `AttackHitsBanked` (3900) | `…to… <b>{target}{extraNameText}</b>…` |
| `AttackMiss` (3896) | `Your {[item]weapon.name} misses {target} completely` |
| `AttackMissBanked` (3897) | `Your group of {[item]weapon.name} misses {target} completely` |
| `AttackMissRD` (3898) | `{[item]weapon.name} belonging to {owner} misses you completely` |
| `AttackMissR` (3899) | `{source} misses you completely` |

Note that `AttackHits*` bodies contain **no damage number**: the figure is
prepended by `AddCombatMessage` as raw markup (`uilog.py:574-584`) `[V]`:

`AddCombatMessage` (`uilog.py:574-584`) `[V]` prepends a coloured damage figure
when `damage` is set and `hitQuality > 0`: the damage is rounded to an integer
and put in bold, in red (`0xffcc0000`) when `attackType` is not `me` and in cyan
(`0xff00ffff`) otherwise.

so **incoming damage is red and outgoing is cyan, decided entirely by
`attackType`**, and a `damage` of `0.0` with `hitQuality > 0` still prints
`0`. A miss (`hitQuality == 0`) prints no number, which is why the older
`AttackMiss1..3` rows bake the flavour text into the body and the modern path
does not.

Also in `AddCombatMessage` `[V]`: there are six user settings that can
suppress the line before it is drawn — `damageMessages`,
`damageMessagesNoDamage` (the gate on a `damage == 0.0` row,
`godma.py:658-659`), `damageMessagesEnemy`, `damageMessagesMine`
(`godma.py:664-668`), and the in-space notification toggles. **A missing
combat line is not necessarily our bug.**

### 2.3 The two shortcuts the client offers

- `svc.logger.AddCombatMessage(msgKey, msgTextArgs)` is in `Logger`'s
  `__exportedcalls__` with role `[]`, i.e. **directly server-callable on the
  client** (class at `uilog.py:109`, method at `:486`,
  `findings/client-callable.tsv:328`) `[V]`. It
  skips name resolution, so the server must supply `source`/`target`/`owner`
  as **strings** and `hitQualityText`/`extraNameText` itself. Useful for GM
  tools and for bot telemetry; not the right shape for real combat, because
  `bracketMgr` and `TargetHealthBars` never see it.
- `OnEffectMessage(msgkey, args)` (`godma.py:683`) is two lines —
  `cfg.GetMessage(msgkey, args)` then `message_player(msg.text)` — i.e. **a
  generic "print this `dialogs.static` key with these arguments" push** `[V]`.
  That is the cheapest way to say anything to a pilot in space.

### 2.4 `OnSpecialFX` — making the gun visibly fire

`Park.OnSpecialFX`, `michelle.py:2087`, `argcount 15` with five defaults
(`duration=-1, repeat=None, startTime=None, timeFromStart=0, graphicInfo=None`),
body `:2088-2094` `[V]`:

```
OnSpecialFX(shipID, moduleID, moduleTypeID, targetID, otherTypeID, guid,
            isOffensive, start, active, duration, repeat, startTime,
            timeFromStart, graphicInfo)
```

`moduleID` may be a list, in which case the park fans it out one scatter per
module (`:2089-2091`). `FxSequencer.OnSpecialFX` then requires `guid` to be a
**string with exactly one dot** (`guid.split('.')`, length 2, else
`LogInfo('No guid in trigger')` and return) `[V]`. For a projectile turret
that string is `effects.ProjectileFired`, from the shipped
`dogmaeffects.fsdbinary` row `effectID 34`, `effectName projectileFired`,
`isOffensive True`, `durationAttributeID 51`, `dischargeAttributeID 6` —
read with `tools/dogma.py effects` `[V]`. `effects.Laser` and
`effects.Mining` are the obvious siblings.

`ScatterEwars` (`:2096`) is the same signature re-emitted as
`OnEwarOnConnect` for the jamming visuals; `OnJamStart(sourceBallID, moduleID,
targetBallID, jammingType, startTime, duration)` and `OnJamEnd(…4 args…)` are
plain `godma` notifications (`godma.py:250`, `:262`) `[V]` and are out of
scope for a plain gun.

---

## 3. Recharge: client-side in **both** families, which is the answer we wanted

### 3.1 The bars

`eveInflight/damageStateValue.CalculateCurrentDamageStateValues:7`, read in
full `[V]`:

The steps, by line `[V]`:

1. `:10-11`: the shield entry is `damageState[0]`; it is extrapolated only when it
   is a list or tuple.
2. `:12-13`: `now` is the sim time, and `shieldHealth, tau` are its first two
   items.
3. `:14-17`: if `tau > 0`, with `sq = sqrt(shieldHealth)` and
   `x = exp((time - now) / dgmTauConstant / tau)`, the shield health becomes
   `(1 + (sq - 1) * x)^2`.
4. `:18-20`: the result is appended; a non-list shield entry gives `None`.
5. `:21`: the last two entries of `damageState` (armour and hull) are appended
   as they are.

with `dogma.const.dgmTauConstant = 10000` (`dogma/const.py:103`) `[V]`.
`time` is the stamp the *client* wrote when it stored the entry (§1.1), so
`time - now <= 0` and the exponent is negative: the shield fraction climbs
monotonically toward `1.0` and never overshoots. `blue.os.GetSimTime()` is in
100 ns units, so `(time - now) / 10000` is **milliseconds** and therefore
**`tau` is in milliseconds**.

So:

- **Shield regeneration on the bars is entirely client-side**, and the server
  sends a curve, not a sequence of samples. `tau = shieldRechargeRate / 5.0`,
  in ms — which is exactly CCP's own producer,
  `(shieldCharge/shieldCapacity, shieldRechargeRate/5.0, GetSimTime())`
  (port-tracker §23). **We must pre-divide by five on this path.**
- **`tau = 0.0` freezes the bar** at the fraction we sent, because the whole
  extrapolation sits inside `if tau > 0`. That is what we ship today and it
  is safe; it is also why the bar will not creep back up on its own.
- **Armour and hull never regenerate**: they are bare floats appended by
  `:21` with no extrapolation at all `[V]`. Every armour and structure change
  needs a message.

### 3.2 The rings

Same formula, different plumbing. `StateManager.UpdateAttribute:2045` and
`CreateChargedAttribute:2059` `[V]`:

`UpdateAttribute` (`godma.py:2049-2056`) defaults `time` to the current sim time
and, when an item's attributes include both a tau attribute and a capacity
attribute, calls `CreateChargedAttribute`. That function (`:2061-2063`) divides
the tau attribute's value by 5.0, reads the capacity attribute, and stores
`[value, time, tauValue, capValue]` in `chargedAttributesByItemAttribute` under
the item and attribute.

`GetChargeValue` (`godma.py:2413-2422`) `[V]` returns 0 when the capacity `Ec`
is 0. Otherwise, with `s = sqrt(max(oldVal / Ec, 0))`,
`dt = min(oldTime - newTime, 0) / dgmTauConstant` (`newTime` defaults to the
current sim time) and `x = exp(dt / tau)`, it returns
`(1 + (s - 1) * x)^2 * Ec`.

Three things follow `[V]`:

- **The client divides `shieldRechargeRate` by five itself here.** The /5 is
  on the *server* for `damageState` and on the *client* for godma. Do it twice
  and the bar and the ring recharge at different speeds — a slow, convincing,
  and very annoying bug.
- **`time: None` is safe and makes recharge start now** (`:2049`). That
  retroactively justifies the choice port-tracker §23.5 made for a different
  reason.
- `min(oldTime - newTime, 0)` clamps, so a stamp from the future cannot drain
  the gauge.

### 3.3 What this means for the server

**We do not need a per-tick health integrator for display.** The client draws
the shield curve itself in both families; it needs a new message only when
something other than passive regeneration moves a number.

The server still needs the authoritative value, because it decides when the
ship dies. `[A]` The right shape is a **lazy integrator**: store
`(value, blueTimeStamp)` per layer and evaluate the same closed form on
demand, in the same two places the client does, so that a shot landing
recomputes the shield from the last stamp rather than from a tick loop.
Falsified if the first bot fight shows the server's and the client's shield
fraction diverging by more than rounding after a minute of regeneration —
which is also exactly the test to write (§8.5).

---

## 4. The death sequence

### 4.1 What every client gets

Covered in detail by the unpublished addballs lane note §3; the summary with the
decision this lane recommends:

- **One entry, no trap:** `('RemoveBall', (bobShipID, True))`.
  `Park.RemoveBall(ballID, terminal=False)`, `michelle.py:1762`/`:1763`, sets
  `delay = ball.GetTotalDestructionEffectTime()` and
  `ball.destructionEffectId = EXPLOSION` itself, then
  `_parent_RemoveBall(ballID, delay)` and `SendEvent('DoBallRemove', ball,
  slimItem, terminal)` (`:1774-1804`) `[V]`. `:1575` names `RemoveBalls`
  only, so **no `exploders` argument is ever appended to the singular**.
- **Plural:** `('TerminalPlayDestructionEffect', (bobShipID, 3))` plus
  `('RemoveBalls', ([bobShipID],))`, same batch, same stamp, either order
  (`:1537` scans the whole batch before the loop). Send `RemoveBalls`
  **exactly one positional argument**: `:1577` appends the exploders dict
  unconditionally, and a second argument of ours lands in `isRelease`, which
  closes every bracket on the observer's screen (`addballs` §3.3).
- `destructionEffectId`: `NONE 0`, `EXPLOSION_OVERRIDE 1`, `DISSOLVE 2`,
  `EXPLOSION 3`, `REVERSE_EJECT 4`
  (`destructionEffect/destructionType.py:1-5`) `[V]`, and `None` is coerced
  to `EXPLOSION` while **`0` is kept** (`:1609`) — so `0` means "vanish, no
  bang".
- The observers' own machinery then runs with no further help from us:
  `spaceObject.Explode:963` scatters `OnObjectExplode(typeID, itemID)`
  **locally** (`spaceObject.py:971`) `[V]`, so that push in
  `findings/push-events.tsv` is *not* ours to send;
  `spaceCamera.DoBallRemove:99` hands a terminal removal of its look-at
  target to `_HandleTargetKilled:113`, which sleeps
  `GetExplosionLookAtDelay()` and then `LookAt(session.shipid)` `[V]`.

### 4.2 What the dying player additionally requires

`[V]` from `gameui._HandleSessionChangeInSpace:762` (the `shipid` branch at `:782`) and
`gameui.DoBallAdd:1116`:

The `shipid` branch (`gameui.py:782-792`) runs when `shipid` is in the change,
its new value is not `None` and a ballpark `bp` exists. It then: **[V]**

1. clears targets in a new tasklet (`:784`);
2. if `session.shipid` is in `bp.balls`: sets `bp.ego` to it, sets
   `wannaBeEgo` to `-1` and calls `OnNewState(bp)` (`:785-789`);
3. otherwise sets `wannaBeEgo = session.shipid` (`:791-792`).

`DoBallAdd(ball, slimItem)` (`:1116-1124`) does the second half: when the added
ball's id equals `wannaBeEgo`, it sets `bp.ego` to it and resets `wannaBeEgo`
to `-1`.

**The single indispensable message is a session change on `shipid`.** Without
it `bp.ego` still names a ball that has been removed: `shipUI.DoBallRemove:1192`
has already called `UnhookBall()` (`:1257`), so the HUD is detached, and the camera has
nothing to follow. The player sits in an empty scene — which is the symptom
the brief names.

And the ordering is **free**: if the capsule's `AddBalls2` lands first, the
session change assigns `ego` immediately; if the session change lands first,
`wannaBeEgo` is parked and `DoBallAdd` finishes the job when the ball arrives.
Either way, send both.

`[I]` Everything else in the ship-death sequence is presentation. There is no
`OnShipDeath` push: the only `OnShipDeath` and `OnPodDeath` handlers in the
build are zero-argument stubs in `packages/nodegraph/client/events/ship.py`
(`findings/push-events.tsv`), a scripting-graph bridge, and nothing in the
flight UI subscribes to them.

---

## 5. Pod and clone

### 5.1 The pod

Two halves, and the split the brief asks for is clean:

- **Destiny work:** an `AddBalls2` carrying the capsule ball and its slim item
  (`typeID 670`, `inventorycommon/const.py:1477`; `groupCapsule 29`, `:512`
  — but §6 note: `groupID` and `categoryID` are derived from `typeID` by
  `SlimItem.__getattr__` and **must not be sent**, `addballs` §1.3), plus the
  removal of the dead ship.
- **Session work:** the `shipid` session change of §4.2, plus the capsule's
  godma item and its ten dogma attributes, which we already build
  (`Registry::capsule_dogma_row`, port-tracker §23.5).

`OnPodOrCorvetteAssembly(item_id, type_id)` is a server push with no local
emitter; its only subscriber is `cosmeticsSvc:269` →
`apply_default_skin(item_id, type_id)` `[V]`. `[I]` cosmetic only; skipping it
costs a default hull skin.

### 5.2 The pod dying, and the clone

`[V]`:

| message | arity | subscriber | effect |
|---|---|---|---|
| `OnPodKilled(destroyedPodID)` | 1 | `gameui:1203` | **stores it and nothing else**: `self.podKillPodID = destroyedPodID` |
| `OnPlayerPodDeath()` | **0** | `evespacescene/playerdeath.py:56` | `viewState.ActivateView('inflight')`; `SpaceObject.Explode(bp.GetBall(bp.ego))`; `shipBall.Display(0)`; camera shake; `audio.SendUIEvent('transition_pod_play')`; `CopyMyScene()`; `loading.FadeIn(…, color=white)` |
| `OnPlayerPodRespawn()` | **0** | `playerdeath.py:167`, `autopilot.py:199` | the "Capsule Breach Detected / recloning" overlay with a gauge, then a fade. This is the **combat-respawn-booster** path, not the classic clone jump |
| `OnCloneDestruction(*args)` | any | `skillsvc:1116` | skill-related bookkeeping |

The clone jump itself is **session work, entirely**. `gameui._ParseStateChange:505`
`[V]`:

It computes four booleans from the `change` dict (`gameui.py:506-509`):

- `movedFromDestroyedPod`: `shipid` is in the change, `podKillPodID` is set and
  equals the old `shipid`;
- `movedToStation`: `stationid` is in the change and its new value is not `None`;
- `movedToStructure`: the same for `structureid`;
- `wasPodded`: `movedFromDestroyedPod` and (`movedToStation` or
  `movedToStructure`).

and the only consequence of `wasPodded` being true is the view-transition
reason string `'clone'` and, inside a structure, forcing the hangar view
(`gameui:505`, `:551-563`) `[V]`. So:

**A clone jump is a docked session change and nothing more.** `OnPodKilled`
exists only so that the client can label that session change as a podding
rather than an ordinary dock. Send `OnPodKilled` first, `OnPlayerPodDeath`
second (for the white-out), the session change third. Omit the first two and
the player is simply, abruptly, in a station — ugly, not broken.

`[A]` We should model the home station as the clone station and send exactly
the session change our existing dock path sends, with `shipid` moving from the
pod to a freshly created ship (or to a corvette). Falsified if the client
refuses a `stationid` session change whose `shipid[0]` names a ball that no
longer exists — nothing in `_ParseStateChange` or `DoSessionChanging` reads the
old ship, so this looks safe.

---

## 6. The wreck

**It is an ordinary ball with an ordinary slim item, and its contents come
through the ordinary solar-system inventory manager.** Nothing about it is a
new kind of entity `[V]`.

- **Class selection is by `groupID`.** `spaceObject/repository.py`'s
  `GROUP_TO_SPACEOBJECT_MAPPING` maps `const.groupWreck` → the `Wreck` class
  `[V]`. `groupWreck = 186` (`inventorycommon/const.py:1053`), and a wreck's
  category is `categoryCelestial` (`ballparkCommon/parkhelpers/__init__.py:14`, test at `:15`,
  treats `categoryCelestial` + `groupID != groupWreck` as warp-in-able) `[V]`.
  Since `SlimItem.__getattr__` derives `groupID` and `categoryID` from
  `typeID` (`addballs` §1.3), **sending the right wreck `typeID` is the whole
  of it** — the client finds the group itself.
- **Three slim-item fields are specific to wrecks**, all `[V]`:
  - **`launcherID`** — the itemID of the ball that exploded.
    `Wreck.Prepare:74` does
    `michelle.GetBall(slimItem.launcherID)` and, if it finds a ball with a
    model, inherits its rotation and synchronises the wreck's appearance to the
    explosion timeline via
    `SpaceObjectExplosionManager.GetExplosionTimesWhenAvailable`
    (`SetBallRotation:47`). Missing or stale → it falls back to
    `slimItem.dunRotation`, then to `SetRandomRotation()`. **So the wreck's
    `AddBalls2` must reach the client while the dead ship's ball is still in
    the park** — same batch, or earlier.
  - **`isEmpty`** — read by `invCtrl.ItemWreck.GetIconName`
    (`invControllers.py:1910`) to choose the full/empty bracket icon.
  - **`lootRights`** — a **4-tuple `(ownerID, corpID, fleetID, abandoned)`**,
    unpacked in `Park.IsAbandoned:1948` and `Park.HaveLootRight:1959`.
- **Contents.** `invCtrl.ItemWreck` inherits `BaseCelestialContainer`
  (`:1485`), whose `_GetItems` is
  `self._GetInvCacheContainer().List()` (`:135`, `:161`) and whose
  `_GetInvCacheContainer` is `invCache.GetInventoryFromId(self.itemID)`
  (`invCache.py:590`), i.e. the **`invMgr` bound to `session.locationid`** —
  the solar system. `[I]` Our existing inventory service already answers that
  bind for ships and containers; a wreck is one more location id in the same
  table. `GetInvID()` is `(inventoryLocationID, flag)` (`:99`, `:124`).
  `ItemWreck.hasCapacity = False`, so no capacity row is requested.
- `svc.wreck` is **purely client-side**: it tracks which wrecks you have
  opened in `settings.char.ui['viewedWrecks']` and sets the local
  `flagWreckAlreadyOpened` UI state (`wrecksvc.py`, read in full) `[V]`. No
  server dependency.
- `OnWreckLootAll(invID, items)` has `local_emitters=1`
  (`invControllers.LootAll:1597` scatters it at `:1615`) `[V]` — **client-local, not a
  push we send**.

`[A]` For a first kill, give the wreck `lootRights = (victimCharID,
victimCorpID, None, False)` and set its slim `ownerID` to the victim, so that
`HaveLootRight` is true for the victim's own corp and for anyone we later
decide may loot. The simplest thing that always works is an **NPC
`ownerID`**: `HaveLootRight:1959` returns `True` for `IsNPC(slim.ownerID)`
before it even reads `lootRights`. Falsified if the client then shows the
wreck with the wrong ownership colour in the overview — cosmetic, and worth
the simplicity.

---

## 7. Who may shoot whom — where the client refuses

This matters because a refusal happens **before** anything reaches us: the
module never activates, no call is made, and the symptom looks like a server
bug.

### 7.1 The gate

`shipmodulebutton.ActivateEffect` (`shipmodulebutton.py:1529`) `[V]`:

`shipmodulebutton.py:1560-1561`: when the relevant effect `isOffensive`, the
button calls `consider.DoAttackConfirmations(targetID, relevantEffect)` and
returns without activating if that is false. `[V]`

`consider.DoAttackConfirmations:118` `[V]`:

`consider.py:123-133` `[V]`:

1. `:123`: if `IsAggressionBlockedInStarterSpace(targetID, effect)` holds, show a
   message and return `False`.
2. `:127`: ask `crimewatchSvc.GetSafetyLevelRestrictionForAttackingTarget` for
   the required safety level.
3. `:128`: if `SafetyCheckPasses(requiredSafetyLevel)` fails, return `False`.
4. `:133`: otherwise return the result of
   `ShowNPCAtttackConfirmationWindowIfAppropriate(requiredSafetyLevel, targetID)`.

`consider.SafetyCheckPasses:111` (`CheckUnsafe` at `:112`, `SafetyActivated` at
`:114`) → `crimewatchSvc.CheckUnsafe:629`, which is one comparison at `:634`:
`return requiredSafetyLevel < self.safetyLevel`, with
`shipSafetyLevelNone 0 / Partial 1 / Full 2` (`crimewatch/const.py:46-48`)
and `self.safetyLevel` initialised to **Full** at `crimewatchSvc.Initialize:81`,
line `:104` `[V]`.

### 7.2 And the way out — one server call, and our test system is the wrong one

`crimewatchSvc.GetSafetyLevelRestrictionForAttackingTarget:461` `[V]`:

`crimewatchSvc.py:468-503` does this `[V]`: it takes the security class from
`securitySvc.get_modified_security_class(session.solarsystemid)` (`:468`), starts
from `shipSafetyLevelFull` as the minimum (`:470`), runs the whole crimewatch
decision tree (`:473-502`) only when the class is low sec or higher (`:472`), and
returns the minimum (`:503`).

`eveuniverse/security.py`, read in full `[V]`:
`securityClassZeroSec 0, LowSec 1, HighSec 2, SafeSec 3`;
`SecurityClassFromLevel(level)` returns `ZeroSec` for `level <= 0.0`, `LowSec`
below `0.45`, `HighSec` below `0.95`, else `SafeSec`; `is_low_sec_or_higher(c)`
is `c >= 1`.

**So if the class is `ZeroSec`, the guard at `:472` is false, the function
returns `Full`, `CheckUnsafe(2)` is `2 < 2 == False`, and the client never
refuses an offensive module.** No engagement timers, no suspect flags, no
`DoesConcordReact`. Everything hangs on that one class.

#### 7.2.1 Where the number comes from, and a correction to two of our own documents

`crimewatchSvc` does **not** call `eveuniverse.get_solar_system_security_class`.
It calls `securitySvc.get_modified_security_class`, which is a different
number. `security/client/securitySvc.py`, read in full `[V]`:

Its parts, by line `[V]`:

- `calculate_pseudo_security(security)` (`:24-27`) returns `0.05` for a value
  strictly between `0.0` and `0.05`, and the value unchanged otherwise.
- `Initialize` (`:53-57`) asks the `securityMgr` remote service for
  `get_modified_systems()` and keeps, per solar system id, a
  `modified_security_level` built from `calculate_pseudo_security` of the row's
  `modifiedSecurity`.
- `OnSecurityModified(solar_system_id, modifier_amount, new_security)` (`:65`)
  updates that table.
- `get_modified_security_level(solar_system_id)` (`:73-76`) returns the stored
  modified level when there is one, and `cfg.mapSystemCache[id].pseudoSecurity`
  otherwise.
- `get_modified_security_class(solar_system_id)` (`:85-86`) is
  `SecurityClassFromLevel` of that level.

and `cfg.mapSystemCache` is a lazy property on `eveCfg.Config`,
`eveCfg.py:795`, whose body at `:799` is

```
fsdBinaryLoader.LoadFSDDataForCFG('res:/staticdata/systems.static',
                                  'res:/staticdata/systems.schema', optimize=False)
```

**`mapSystemCache` is shipped client static data, not a server cache** `[V]`.
`res:/staticdata/systems.static` has **8490 rows** and every one carries
`securityStatus`, `pseudoSecurity` and `securityClass`
(`tools/fsd_dump.py rows res:/staticdata/systems.static`) `[V]`. The census,
counted with `SecurityClassFromLevel` over all 8490:

| class | `securityStatus` | systems |
|---|---|---|
| ZeroSec | `<= 0.0` | **6556** |
| LowSec | `< 0.45` | 687 |
| HighSec | `< 0.95` | 1193 |
| SafeSec | `>= 0.95` | 54 |

> **Correction.** The unpublished industry lane note says
> "`cfg.mapSystemCache[id].securityStatus` is a *server*-supplied cache, so
> this can only be settled by deciding it", and the doc comment at
> `server/crates/eve-server/src/industry.rs:1186` repeats it. Both are wrong
> for this build: the table is in the client. The industry lane's `safe_sec`
> is therefore **readable**, not a null result — and it is `false` for our
> system anyway, so the shipped behaviour is right for the wrong reason.

#### 7.2.2 Our test system is high sec, so the client will refuse

`systems.static[30000001]` is `securityStatus 0.8583`,
`pseudoSecurity 0.8583`, `securityClass 'B'` `[V]`.
`SecurityClassFromLevel(0.8583)` is `securityClassHighSec` (2), so:

- `is_low_sec_or_higher(2)` is true → the whole crimewatch tree at `:473-502`
  runs;
- `is_high_sec_or_higher(2)` is true inside it → for a target that
  `CanAttackFreely:536` rejects, `minSafetyLevel` becomes `Partial` (1) or
  `None` (0);
- `CheckUnsafe(1)` against the default `safetyLevel` of `Full` (2) is true →
  `DoAttackConfirmations` returns `False` → **`ActivateEffect` returns before
  any call reaches us.**

Two players shooting each other in 30000001 will simply not fire, and the only
visible trace is `crimewatchSvc.SafetyActivated:642`'s
`LogInfo('Safeties activated', …)` plus a scattered
`OnCrimewatchSafetyCheckFailed`. **This will look exactly like our bug.**

#### 7.2.3 Three fixes, in order of how little they cost

1. **`securityMgr.get_modified_systems()` — the one-call fix.** It is already
   a route we answer: `service/base.rs` returns `{}` today, with a note that
   `{}` makes the caller "fall through to
   `cfg.mapSystemCache[...].pseudoSecurity`" — which is precisely the
   high-sec number. Return instead a dict keyed by solar-system id whose
   values expose **`.solarSystemID` and `.modifiedSecurity` as attributes**
   (`:57` uses `data.solarSystemID`, not a subscript), with
   `modifiedSecurity = 0.0`, and `get_modified_security_class` returns
   `ZeroSec` for that system `[V]`. **Send exactly `0.0` or a negative
   number, never `0.01`:** `calculate_pseudo_security:26` lifts anything in
   `(0.0, 0.05)` to `0.05`, which is `LowSec`.
   Same mechanism, pushed: `OnSecurityModified(solarSystemID, modifierAmount,
   newSecurity)` (`:65`) changes one system live and scatters
   `OnCurrentSystemSecurityChanged`.
   `[I]` Cost: the UI will draw the system's security as `0.0` with a
   "security modified" icon (`get_security_modifier_icon_res_path:100`),
   which for a PvP sandbox is arguably the truth.
2. **Move the fight to one of the 6556 null-sec systems.** `30000208`
   (`securityStatus -0.173`, region `10000003`) is the lowest-numbered one in
   ordinary space; `30000021` is `-1.0`. No server change, but it moves the
   whole world and interacts with the station we dock in.
3. **Lower the safety level.** `CheckUnsafe:634` is
   `required < self.safetyLevel`, so a `safetyLevel` of `None` (0) passes
   every check. The server supplies it as the **fourth element** of
   `CharGetCrimewatchLocation().GetClientStates()` (§7.3), and
   `_UpdateSafetyLevel:171` only overrides it when
   `is_safe_sec_solar_system(session.solarsystemid)` — i.e.
   `securityStatus >= 0.95`, true of only **54** systems and not of 30000001.
   So this works today in our system and needs no new data. It is third
   because the player can turn the safety back on from the HUD.

**Avoid the 54 `SafeSec` systems entirely**, under any of the three: there
`IsSafetyLockedToFullLevel:187` returns a truthy string and
`_UpdateSafetyLevel` forces `Full` on every session change `[V]`.

`IsStarterSystem(session.solarsystemid)` is the other, independent block
(`consider.IsAggressionBlockedInStarterSpace:98`, the character test at `:104`):
in a starter system, any target whose `ownerID` is a character is refused
outright, **whatever the security class** `[V]`. Avoid the starter system
list; this lane did not enumerate it.

### 7.3 The one server dependency crimewatch has

`crimewatchSvc.ProcessSessionChange` and `OnSessionChanged` both call
`eveMoniker.CharGetCrimewatchLocation().GetClientStates()` on any
`locationid`/`charid`/`solarsystemid`/`shipid` change, and unpack the result as
`(myCombatTimers, myEngagements, flaggedCharacters, safetyLevel)` where
`myCombatTimers` is a 5-tuple of `(state, expiryTime)` pairs and
`flaggedCharacters` is `(criminals, suspects)` `[V]`. **This bound object is
required for every undock and every system change, independently of whether
anyone shoots.** `docs/reference/bound-objects.md` lists `crimewatch` among the bound
families; this lane did not check whether we serve it.

`[A]` Serve `GetClientStates()` as
`(((0, None), (0, None), (0, None), (0, None), (0, None)), {}, ((), ()), 0)` —
five idle timers, no engagements, nobody flagged, safety level `None` (0).
Safety `0` makes `CheckUnsafe` false for every required level, so it is also
belt-and-braces for §7.2. Falsified if the client renders a "safety: off"
warning the user then has to clear, or if `_UpdateSafetyLevel:171` →
`IsSafetyLockedToFullLevel` overrides it (it does, but only in
`is_safe_sec_solar_system`, i.e. `securityStatus >= 0.95` — true of only 54
systems, and of neither null sec nor our 30000001).

### 7.4 The looting gate, same shape

`crimewatchSvc.CheckCanTakeItems:650` `[V]`: if the system is not a wormhole
**and** `GetSafetyLevel() == Full` (`:657`) **and** the container's `groupID` is
in `crimewatch.const.containerGroupsWithLootRights` (`:660`) **and**
`not bp.HaveLootRight(containerID)` (`:663`) → `False`, and
`invControllers.LootAll:1597` then raises (at `:1610`, after
`SafetyActivated` at `:1609`)
`UserError('LootTheftDeniedSafetyPreventsSuspect', {'containerName': (4, typeID)})`
— `const.UE_TYPEID` is `4` (`eveexceptions/const.py:5`). §6's NPC-owner or
matching-`lootRights` choice defuses it; a safety level below Full defuses it
too.

### 7.5 Error vocabulary, from `findings/user-errors.tsv`

The keys a server or a mis-shaped payload will actually produce:
`NoCharges` (`:2987`, raised at `shipmodulebutton.py:1539`),
`ModuleReactivationDelayed2` (`:2898`, caught at `:1582` and used to restart
the cooldown ring from `errorDict['timeSinceLastStop']` and `['itemID']`),
`EffectStillActive` (`:2014`, caught at `:1597`, reads `dict['timeLeft']`),
`TargetNotLocked` / `TargetNotLocked2` (`:3982-3`),
`ModuleActivationDeniedSafetyPreventsSuspect` / `…Criminal` /
`…LimitedEngagement` / `…Disapproval` (`:2882-2885`),
`LootTheftDeniedSafetyPreventsSuspect` (`:2667`),
`AttackCanceledByAura` (`:263`).

---

## 8. Falsifiable tests

port-tracker §23.3 records this subject's canonical mistake: a test that
asserted our own payload's **shape** passed on a payload the client drew as an
empty bar. So every test below asserts a **property the player would see**,
through a transcription of the client's own decoder where one exists
(`DamageState::as_the_client_reads_it` already exists for the bars).

**8.1 A damage event moves both families, independently.** Apply one shot to
a modelled ship and assert *two* things about the emitted traffic: that the
destiny batch contains an `OnDamageStateChange` entry whose decoded three
values each went **down**, and that the dogma messages contain an
`OnModuleAttributeChange` row for the layer that was actually hit. Assert the
pair, not either alone — a single-family assertion is how §23 happened twice.

**8.2 The two families agree to within rounding.** From one server state,
compute the bar fraction and the ring fraction (`(hp - damage)/hp`, with the
client's own `BINARY_DIVIDE` semantics) and require them equal to 1e-6.
This is the test that catches the double `/5` of §3.2 and the
float-vs-integer trap of §23.5. Run it at a **damaged** state — a full-health
fixture cannot fail it.

**8.3 `tau` is divided by five exactly once.** Given a hull whose
`shieldRechargeRate` is `R`, assert the `damageState` shield slot's second
element is `R / 5.0` **and** the godma attribute 479 we send is `R`. Two
assertions, opposite directions, because the client does the division on one
path and we do it on the other.

**8.4 The combat message survives the client's own decoder.** Transcribe
`AddCombatMessageFromDict`'s key handling (10 lines) and assert, for each of
the three `attackType` values, that the computed `msgKey` is one of the eight
keys in §2.2 and that every token the chosen body names
(`target`/`source`/`owner`, `weapon`, `extraNameText`) is present. Assert
further that `hitQuality` and `isBanked` are present **as a property of the
dict**, because the client subscripts them bare — a `.get` in our producer
would pass a shape test and KeyError on the wire.

**8.5 The shield integrator matches the client's curve.** Implement
`(1 + (sqrt(h0) - 1) * e^(-dt_ms/tau_ms))^2` once, in the test, from §3.1's
transcription, and assert the server's own lazy value equals it at
t = 0, 1 s, 10 s and 10·tau. Property, not implementation: the assertion is
"our number is on the curve the client is drawing", which is the only thing
that can be wrong in a way a player notices.

**8.6 The removal is single-argument.** Assert that every `RemoveBalls` entry
we ever emit has exactly one element in its args tuple, and that `ballIDs` is
a list or tuple rather than an iterator. `addballs` §3.2 explains why
asserting the *symptom* instead would pass or fail depending on an unrelated
entry in the same batch.

**8.7 A kill batch carries one stamp.** Assert that the batch containing the
explosion, the removal, the wreck's add and the pod's add has
`len({entry.stamp}) == 1`, because `DoDestinyUpdate:660` calls
`OnFatalDesync` otherwise and `OnFatalDesync` does not return.

**8.8 The wreck can find its ship.** Assert that in the emitted stream, the
`AddBalls2` carrying a slim item with `launcherID == X` is not preceded, in an
earlier batch, by a removal of `X`. This is an ordering property of the
stream, checkable without a client.

**8.9 The system we fight in is not high sec, by whichever route we chose.**
Assert the property the client computes, not our configuration: transcribe
`SecurityClassFromLevel` and `calculate_pseudo_security` (six lines between
them) and assert that, for the system a fixture puts the players in,
`SecurityClassFromLevel(calculate_pseudo_security(level))` is
`securityClassZeroSec`, where `level` is our `modifiedSecurity` if we send one
and `systems.static`'s `pseudoSecurity` otherwise. This catches the
`0.01 -> 0.05` lift, and it fails loudly today for system 30000001
(§7.2.2), which is the point.

**8.10 `ownerID` on an attribute change is the character, not the ship.**
Assert it for every `OnModuleAttributeChange` row we emit. `godma.py:1591`
drops the row otherwise, after a traceback, and the ring just does not move —
a silent failure with a loud log.

---

## 9. Open questions, ranked by what they cost if wrong

1. **Is `OnMultiEvent`'s grouping key a destiny stamp, a sequence number, or
   something else?** `[I]` The shape is `(eventTuple, key)` and the key is
   only used to group and order (`godma.py:269`) — nothing reads its value.
   `[A]` Send the batch's destiny stamp. Cost if wrong: low, because the
   client only groups by it; but a key that compares unstably across types
   (e.g. mixing `None` and ints) would make `sorted()` raise in Python 2 in a
   way that is cheap to avoid and expensive to diagnose.
2. **Does our `invMgr` bind answer for a wreck's itemID?** `[I]` §6 says a
   wreck is just a location under `session.locationid`. If it does not, the
   wreck opens an empty window, which reads as "loot is broken". This is the
   single largest unchecked dependency in the feature and it belongs to the
   inventory lane, not this one.
3. **Do we serve `CharGetCrimewatchLocation`?** §7.3. If not, every undock
   runs `crimewatchSvc.ProcessSessionChange` into an exception. Cost: possibly
   an undock that half-works, which is the worst kind. Its fourth return
   element is also fix 3 of §7.2.3, so this one call sits on the critical path
   twice.
4. **Is there a server→client message that tells the client a module's shot
   *missed* beyond `hitQuality == 0`?** Not found. `[A]` `hitQuality 0` plus
   `damage 0.0` is the whole of a miss, and the `damageMessagesNoDamage`
   setting is what decides whether the player sees it. Falsified by a capture
   of CCP's traffic showing a distinct push for misses.
5. **`OnSpecialFX`'s `graphicInfo` and `otherTypeID`.** Passed through to
   `FxSequencer`'s `Trigger` untouched; `None` is the client's own default for
   both. `[A]` `None` is correct for a plain turret. Falsified by a turret that
   fires with no visible projectile.
6. **Whether the corpse (`groupBiomass` 14) is required after a pod death.**
   `playerdeath.GetCorpseModel` builds one for the *local* death scene from
   `corpse.GetCorpseModelForCharacter(session.charid)`, i.e. client-side
   `[V]`. `[I]` the in-space corpse ball is independent flavour. Cost of
   omitting: nobody can loot a frozen corpse. Nothing breaks.
7. **`isBanked` and grouped weapons.** `AttackHitsBanked` and
   `godma.DamageMessageTurretPropogation:532`'s `banked` parameter are about
   several turrets firing as one. `[A]` send `isBanked: False` and one message
   per gun until we have weapon grouping. Falsified if the client visibly
   fires only one turret of a set.
8. **Drone damage.** `attackType == 'otherPlayer'` with
   `owner = (2, ownerID)` is the drone/fighter case, and
   `packages/eveDrones/droneDamageTracker.py:97` subscribes to
   `OnDamageStateChange` for drones in the bay. Not investigated; out of scope
   for the first kill.

---

## 10. What I could not settle, and corrections

**Could not settle, and deliberately did not try:**

- The server side of `dogmaIM.Activate` — what it returns, what a cycle timer
  looks like, how `repeat` is honoured. That is the `dogma`/module-activation
  feature and it is strictly upstream of this one. `docs/reference/bound-objects.md`
  §3.1-3.3 is the starting point.
- The lock handshake. `docs/re/targeting.md` owns it. Only one fact crosses
  the boundary: `OnTarget('lost', deadShipID, 'Exploding')` is the message
  that clears a lock on a ship that blew up, confirmed by
  `nodegraph/client/events/target.py`'s `TargetExploded.OnTargets` reading
  `event == 'lost' and reason == 'Exploding'` (quoted in
  `docs/re/targeting.md` §3.3) `[V]`.
- Whether any of this works. Nothing here was run against a client; the whole
  document is a read of bytecode and shipped data.
- `damageSvc` (`svc.damage`, `parklife/damageSvc.py`) turned out to be about
  **damage decals and locator geometry**, not damage numbers
  (`GetTargetLocators`, `GenerateDamageLocators`, `CopyAttributes`) `[V]`. The
  name is a trap; it is not part of this feature.

**Corrections to existing documents** (recorded here only; this lane edits no
other file):

1. The unpublished port tracker §23.4 says "the cheapest discriminator costs seconds
   at a running client: hover a gauge". True, and it can be cheaper still:
   §1.1 above shows the bars have exactly five writers and none of them is a
   godma path, so *any* bar/ring disagreement is by construction a
   two-messages problem and never a decode problem. The discriminator is now
   "did we send both messages", answerable from a capture.
2. The unpublished port tracker §23's standing framing is "two independent gauge
   families". §1.2 adds the thing that makes them manageable:
   `DoDestinyUpdate(state, waitForBubble, dogmaMessages)` carries **both** in
   one call (`michelle.py:635-637`), so the independence is in the client's
   plumbing, not in ours.
3. **The unpublished industry lane note and `server/crates/eve-server/src/industry.rs:1186`
   are wrong that `cfg.mapSystemCache` is server-supplied.** It is
   `res:/staticdata/systems.static`, shipped, 8490 rows, with `securityStatus`,
   `pseudoSecurity` and `securityClass` columns (`eveCfg.py:795`, `:799`)
   `[V]`. §7.2.1. The industry lane's `safe_sec` can be read rather than
   decided — and the same table says our test system is high sec, which
   §7.2.2 argues is a live defect waiting in the next feature.
4. Nothing in the unpublished addballs lane note needed correcting; its reading of the
   `exploders` comprehension at `michelle.py:1537` is right, including the
   value/key order, which CPython 2.7 emits value-first for a dict
   comprehension. This lane re-derived it from the opcodes and agrees.

**What the next person should read first:** §0 of this file, then §7.2
(because the client will refuse to fire in the system we currently use), then
The unpublished addballs lane note §3, then the unpublished port tracker §23.5.
