# -*- coding: utf-8 -*-
"""Fitting window redraw after a drag-and-drop fit (FIT-DRAG, issue #185).

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Lane `fit-drag-fci`.

The owner drag-dropped modules onto the open fitting window; they were fitted
(the server did it) but the window did not redraw.  The window redraws from
dogma: `invCache.OnItemChange` -> godma / `clientDogmaIM` ->
`fittingController.OnDogmaItemChange` (fittingController.py:333) ->
`_UpdateSlots` (:112) -> `GetFittedItems` -> each slot controller's
`SetModuleAndChargeIDs`.  This scenario drives the client's OWN drop handlers
with the window open and the real ship active -- never the mouse -- and reads
what the slot controllers say:

(a) one hangar module -> `fittingSlotController.OnDropData` on an empty slot
    (`GetDroppedItems` -> `uthread.new(self.Add)` -> `AddRealItem` ->
    `FitModule` -> `dogmaLocation.TryFit` -> `Add(itemID, loc, qty=1, flag)`);
(b) three modules dropped on three slots at once (a uthread each);
(c) a stack of three identical modules dropped on ONE slot: three Adds with
    the same flag (the 2nd and 3rd are answered `SlotAlreadyOccupied` by the
    server, `fitting.rs` `Refusal::Occupied`; that raise is expected);
(d) three modules dropped on the ship centre: `ShipSceneParent.OnDropData` ->
    `controller.OnDropData` -> `TryFitter.FitModule` ->
    `MultiAdd(flag=flagAutoFit)` (tryFit.py:133).

(e) the owner's own sequence with REAL drag nodes (research-w24.md "Case (e)
    spec"): fit one medium module, unfit it to the hangar (now a singleton),
    open the hangar window, hover the module's inventory entry
    (`xtriui.InvItem.OnMouseEnter`, item.py:663), take `entry.GetDragData()`
    (item.py:836) and drop those nodes on the ship centre
    (`ShipSceneParent.OnDropData`, fittingCenter.py:307) or on the slot WIDGET
    (`FittingSlotBase.OnDropData`, baseSlot.py:187); unfit; twice over.

CHECKS, per case: the server fitted what was dropped (the client's inventory
lists it), and each fitted slot WIDGET (`FittingSlot`, found under
`FittingCont.slots`, baseFitting.py:38-42) shows the module: its `hint` equals
`evetypes.GetName(typeID)` within REDRAW_MS (slot.py:274-278); after an unfit
the widget's `hint` is its `_emptyHint` again (baseSlot.py:34, slot.py:283).
The slot controllers' `GetModuleID()` (the old check, which never looked at
the widget) is a DIAGNOSTIC in (a)-(d) and a check in (e).  DIAGNOSTICS (green, they assert nothing): every
`invCache.OnItemChange` change dict the client received (a fit must carry
`ixLocationID` 3 or `ixFlag` 4, fittingController.py:334-341), the dogma
location's and the controller's calls, `_UpdateSlots` count, any exception
raised inside that path, the dogma view, and the client log lines of the span.

(f) issue #13, the owner's large-hull sequence: board a large hull with the window
    open, fit a module, unfit it (a hangar singleton), drop it on the ship centre
    and on the slot widget with real hangar drag nodes, a large turret on the
    centre; three runs (the window re-opened for runs 2 and 3); then drops on a
    FULL high rack and on an occupied medium slot, which must change nothing.
    A tap at each stage -- S1 `invCache.OnItemChange`, S2 `clientDogmaIM.
    GodmaItemChanged`, S3 dogma `FitItem`, S4 `OnDogmaItemChange` + `_UpdateSlots`,
    S5 the slot widget's `UpdateFitting` -- records how far each drop gets.

Falsifiers: no frigate hull / gun / med / low the client knows [A: a miss is a
PRECONDITION]; the fitting window opens in simulation (ghost) mode [A: a
PRECONDITION says so]; `GetDroppedItems` wants more of the node than `.rec` and
`__guid__` [A: the (a) check then fails on "the server fitted ..."].
"""

EOCI_SCENARIO = 'fci_fit_drag'

# Seconds to pass to `tools/client-ci.sh run fci_fit_drag --timeout`.
EOCI_FCI_TIMEOUT_S = 900

# SERVER-EXPECT: absent  UNANSWERED (ship|invbroker|dogmaIM)
# SERVER-EXPECT: absent  panicked at


# PRELUDE -- byte-identical in every scenario in this directory, on purpose.
#
# The harness contract deploys exactly ONE file, `<name>.py`, into
# `c:\eve-logs\client-ci\`.  Nothing guarantees a shared library lands beside
# it, so a scenario that imported one could fail for a reason that has
# nothing to do with the thing it tests.  Every scenario is therefore
# self-contained and this block is copied verbatim.
# `tools/client-ci/scenarios/_prelude.py` is the canonical copy; if you change
# it, change it in all of them -- `docs/lanes/ci-scenarios.md` section 6.
#
# Why the whole scenario lives inside one function:
#   `execfile(path)` called from inside a *function* frame -- which is what
#   the planned trigger is (`crate::pycode` emits a code object whose body
#   execfiles the scenario) -- runs the file with `locals() is not globals()`.
#   Module-level `def`s then land in that frame's locals, and a sibling
#   function looking the other up through *globals* gets a NameError.  Putting
#   every helper and the body itself inside one function turns those into
#   closure lookups, which work under `/execfile`, under `exec` with a
#   separate locals mapping, and at true module level alike.
# ---------------------------------------------------------------------------

def eoci_main(EOCI_NAME):
    import os
    import sys
    import traceback

    OUTDIR = 'C:\\eve-logs\\client-ci'

    checks = []

    # -- output ------------------------------------------------------------

    def text(value):
        """`str()` that cannot itself raise and break the verdict file."""
        try:
            if isinstance(value, unicode):  # noqa: F821  (py2)
                return value.encode('ascii', 'replace')
            return str(value)
        except Exception:
            try:
                return repr(value)
            except Exception:
                return '<unprintable>'

    def jstr(value):
        """A JSON string literal.  Hand-rolled so nothing has to import."""
        src = value if isinstance(value, unicode) else text(value)  # noqa: F821
        out = ['"']
        for ch in src:
            o = ord(ch)
            if ch == '"':
                out.append('\\"')
            elif ch == '\\':
                out.append('\\\\')
            elif ch == '\n':
                out.append('\\n')
            elif ch == '\r':
                out.append('\\r')
            elif ch == '\t':
                out.append('\\t')
            elif o < 0x20 or o > 0x7E:
                out.append('\\u%04x' % o)
            else:
                out.append(ch)
        out.append('"')
        return ''.join(out)

    def check(name, ok, detail=''):
        """Record one assertion.  Never raises; a bad `detail` is survivable."""
        try:
            flag = bool(ok)
        except Exception:
            flag = False
        checks.append((text(name), flag, text(detail)))
        try:
            # live, so a scenario that never finishes still shows how far it got
            import log
            log.LogInfo('eve-offline client-ci [%s] live:' % EOCI_NAME,
                        '[%s] %s -- %s' % ('ok' if flag else 'FAIL', text(name), text(detail)[:200]))
        except Exception:
            pass
        return flag

    def say(message):
        """Best-effort line into the client's own log, for the failure case."""
        try:
            import log
            log.LogInfo('eve-offline client-ci [%s]:' % EOCI_NAME,
                        text(message))
        except Exception:
            pass

    def write_result(error):
        """Write the verdict.  Called from `finally`; must always succeed."""
        ok = error is None
        rows = []
        for (cname, cok, cdetail) in checks:
            ok = ok and cok
            rows.append('  {"name": %s, "ok": %s, "detail": %s}'
                        % (jstr(cname), 'true' if cok else 'false',
                           jstr(cdetail)))
        body = ('{"scenario": %s, "ok": %s,\n "checks": [\n%s\n ],\n'
                ' "error": %s}\n'
                % (jstr(EOCI_NAME),
                   'true' if ok else 'false',
                   (',\n').join(rows),
                   jstr(error) if error is not None else 'null'))
        try:
            os.makedirs(OUTDIR)
        except Exception:
            pass
        path = os.path.join(OUTDIR, '%s.json' % EOCI_NAME)
        try:
            f = open(path, 'wb')
            try:
                f.write(body)
                f.flush()
                try:
                    os.fsync(f.fileno())
                except Exception:
                    pass
            finally:
                f.close()
        except Exception:
            # The harness treats a missing file as its own diagnosis, so make
            # sure the client log carries the verdict as well.
            say('COULD NOT WRITE %s' % path)
        say('ok=%s checks=%d error=%s'
            % (ok, len(checks), 'yes' if error is not None else 'no'))
        for (cname, cok, cdetail) in checks:
            say('  [%s] %s -- %s' % ('ok' if cok else 'FAIL', cname, cdetail))
        if error is not None:
            say(error)
        return ok

    def quit_client(reason):
        """Exit the client, the way the client's own quit button does.

        **`blue.os.Quit()` -- which `docs/archive/plan-client-ci.md` section 3 names --
        does not exist in this build.**  The native call is
        `blue.os.Terminate(exitCode)` ("Terminates the process forcefully.",
        `|i:Terminate`), and the Python wrapper every caller uses is
        `bluepy.Terminate(exit_code, reason='')` (`bluepy/__init__.pyc:502`,
        reaching `blue.os.Terminate(exit_code)` at `:528`) [V].  That is what
        `carbonui/services/command.pyc:659` `_AppQuitGame` and
        `eve/client/script/ui/eveCommands.pyc:1254` `DoQuitGame` both call
        [V], and it fires `ProcessShutdown` and the atexit hooks on the way
        out, so a scenario exits the way a player quitting would.

        `log.Quit` (`logmodule/__init__.pyc:613`) reaches the same
        `bluepy.Terminate` at `:624` but dumps every tasklet's stack into the
        log first (`:616`), which is noise in the log the harness attaches to
        a failure -- so it is a fallback, not the first choice.
        """
        try:
            import bluepy
            bluepy.Terminate(0, text(reason))
            return 'bluepy.Terminate'
        except Exception:
            pass
        try:
            import blue
            blue.os.Terminate(0)
            return 'blue.os.Terminate'
        except Exception:
            pass
        try:
            import log
            log.Quit(text(reason))
            return 'log.Quit'
        except Exception:
            pass
        try:
            sys.exit(0)
        except SystemExit:
            raise
        except Exception:
            pass
        return 'none'

    # -- client access -----------------------------------------------------

    def builtin(name):
        """`session` and `sm` are installed as builtins by the client.

        `destiny_dump.py` reaches them this way for the same reason: the
        globals the trigger happens to run us under are not ours to assume.
        """
        try:
            return getattr(sys.modules['__builtin__'], name)
        except Exception:
            return None

    def sleep_ms(ms):
        """Yield to the client for `ms` wallclock milliseconds.

        Wallclock, not sim time: a scenario that has to survive a stalled
        simulation must not wait on the simulation's own clock.

        `blue.pyos.synchro.SleepWallclock(ms)` is the client's own name for it
        (`uthread/__init__.pyc:995`); `blue.synchro.SleepWallclock(ms)` is the
        same call reached through the shorter alias
        (`stationintrocinematic/client/cinematic_banner.pyc:123`); `SleepSim`
        is last because it is the wrong clock and is only better than not
        yielding at all (`michelle.pyc:508`).  All three take one argument,
        milliseconds [V].
        """
        import blue
        try:
            blue.pyos.synchro.SleepWallclock(int(ms))
            return True
        except Exception:
            pass
        try:
            blue.synchro.SleepWallclock(int(ms))
            return True
        except Exception:
            pass
        try:
            blue.pyos.synchro.SleepSim(int(ms))
            return True
        except Exception:
            return False

    def now_ms():
        """Wallclock milliseconds, or None if blue will not tell us.

        Blue keeps time in 100 ns units, so a difference divided by 10000 is
        milliseconds.  `blue.os.GetWallclockTimeNow` and
        `blue.os.GetWallclockTime` both exist in this build [V] (present in
        the decompiled tree); which one a given build prefers is [I], so both
        are tried.
        """
        import blue
        for getter in ('GetWallclockTimeNow', 'GetWallclockTime'):
            try:
                return getattr(blue.os, getter)() / 10000.0
            except Exception:
                continue
        return None

    def wait_for(predicate, timeout_ms=20000, step_ms=250):
        """Poll `predicate` until truthy.  Returns `(value, waited_ms)`.

        **`waited_ms` is measured, not counted.**  This used to return the sum
        of the `step_ms` values it had asked for, which is not the time that
        passed: `sleep_ms` yields to the client's own scheduler, and when the
        client is busy -- loading the destination scene during a warp, most
        obviously -- a requested 250 ms can take very much longer.  Every
        "after N ms" the suite printed was therefore a number the scenario had
        asked for rather than one it had observed.

        That is not cosmetic.  `throttle_and_warp` reported a ship travelling
        8,886,218 m with a peak of 3.9e9 m/s "in 3000 ms", and those three
        figures cannot all be true at once: our own warp model (a capsule at
        4.0 AU/s) puts a ship ~163 km along and at ~651 km/s three seconds in.
        The distance and the speed were real; the 3000 ms was not, so the
        check passed while carrying a number that could not be right.  A test
        that reports a quantity it did not measure can agree with a broken
        server as easily as a working one.

        The timeout is now honest too, which is a behaviour change: it is real
        elapsed time, so a client that stalls for longer than `timeout_ms`
        gives up instead of polling on past it.  The poll count comes back in
        the detail so a stall is visible rather than merely slow.

        A predicate that raises counts as "not yet", which is what every
        "that service is not up" case looks like from in here.
        """
        started = now_ms()
        counted = 0
        polls = 0
        while True:
            try:
                value = predicate()
            except Exception:
                value = None
            polls += 1
            if started is None:
                elapsed = counted
            else:
                here = now_ms()
                elapsed = counted if here is None else (here - started)
            if value:
                return value, int(elapsed)
            if elapsed >= timeout_ms:
                return None, int(elapsed)
            sleep_ms(step_ms)
            counted += step_ms

    def get_michelle():
        """`sm.GetService('michelle')` -- michelle.pyc:87 `GetMichelle` [V]."""
        sm = builtin('sm')
        if sm is None:
            return None
        return sm.GetService('michelle')

    def undock(timeout_ms=60000):
        """Leave the station, if we are in one.  `(ok, detail)`.

        **Nothing else undocks us.**  The harness logs in with
        `/autoSelectCharacter:0`, which lands the character in the station it
        was last in -- and every scenario that reads a ballpark needs to be in
        space, because there is no ballpark in a hangar.  The first real
        unattended run failed on exactly this: five checks green, and
        `session.solarsystemid = None, session.stationid = 60012526`.

        `sm.GetService('station')` is `StationSvc`, `__guid__ = 'svc.station'`
        (`eve/client/script/ui/station/base.pyc:43`) [V], and `UndockAttempt`
        (`:563`) is the client's own entry point -- the one a player's click
        reaches.  It is `self._DoUndockAttempt(False, True, shipID)` [V].

        **It does not prompt.**  With `observedSuppressed` false the body runs
        straight through `GetOnlineModules`, `space.PrioritizeLoadingForIDs`
        and then `sessionMgr.PerformSessionChange('undock', shipsvc.Undock,
        shipID)` (`:580-586`) [V].  The contraband question at `:597` is in
        the *exception* path, raised by the server, and ours answers `Undock`
        with `None`.  That matters here more than it would in a UI: a dialog
        nobody can click is an unattended run that hangs until its timeout.

        The return value of `Undock` is not the answer either -- the call site
        `POP_TOP`s it.  What takes the client out of the station is the
        session change that travels with it, so this waits for
        `session.solarsystemid` to appear rather than for the call to return.
        """
        sess = builtin('session')
        if sess is None:
            return False, 'no session to undock'
        if not getattr_safe(sess, 'stationid'):
            return True, 'already in space (session.stationid is unset)'
        sm = builtin('sm')
        if sm is None:
            return False, 'no sm to reach svc.station through'
        ship_id = getattr_safe(sess, 'shipid')
        if not ship_id:
            return False, 'session.shipid is unset; nothing to undock'
        # **Retried, because this scenario runs earlier than any human ever
        # clicks.**  The trigger fires from `GetSettingsInfo`, and the first
        # real run raised `KeyError: 100000001` out of
        # `clientDogmaIM.GetDogmaLocation().GetOnlineModules(shipID)` --
        # `_DoUndockAttempt`'s first statement (`base.pyc:580`) -- because the
        # client logs `BaseDogmaLocation::_InstantiateDogmaItem 100000001`
        # about 3,300 log lines *after* the scenario starts.  Nothing is
        # wrong with the server: a player clicks seconds later, by which time
        # the dogma location holds their ship.
        #
        # Retrying rather than waiting on that one precondition is deliberate.
        # The precise thing that is not ready yet is an implementation detail
        # of a path we do not own, and a scenario that waits on the wrong one
        # hangs for its whole timeout and blames the wrong thing.
        last = None
        deadline_steps = max(1, int(timeout_ms / 500))
        for _attempt in range(deadline_steps):
            try:
                sm.GetService('station').UndockAttempt(ship_id)
                last = None
                break
            except Exception, e:
                # The type matters more than the message here: the first run
                # reported `raised: 100000001`, which is a `KeyError`'s key
                # and reads like nonsense on its own.
                last = '%s: %s' % (e.__class__.__name__, text(e))
                sleep_ms(500)
        if last is not None:
            return False, ('UndockAttempt(%s) still raising after %s ms -- %s'
                           % (text(ship_id), text(timeout_ms), last))
        # The session object is replaced on a session change, so re-read it
        # rather than closing over the one we started with.
        def in_space():
            s = builtin('session')
            return s is not None and getattr_safe(s, 'solarsystemid')
        ssid, waited = wait_for(in_space, timeout_ms=timeout_ms)
        if not ssid:
            return False, ('still docked %s ms after UndockAttempt(%s)'
                           % (text(waited), text(ship_id)))
        # The SERVER puts the ship in its roster when the ballpark binds, and
        # a GM command or a module activation sent before that answers 'not in
        # space' ('Modules can only be activated in space', '/whjump: you are
        # not in space') [V: run 2026-10-28].  So undock() returns when the
        # ego ball is in the park, not when the session says so.
        park, ego = get_ego_ball(timeout_ms=timeout_ms)
        if ego is None:
            return False, ('solarsystemid %s after %s ms but no ego ball in the park'
                           % (text(ssid), text(waited)))
        return True, 'undocked into solarsystemid %s after %s ms (ego ball in the park)' % (
            text(ssid), text(waited))

    def get_ballpark(timeout_ms=120000):
        """The live client-side ballpark, polled rather than waited on.

        `michelle.GetBallpark()` returns `self.__bp` only once
        `michelle.bpReady` is true (`michelle.pyc:484`) [V].  The blocking
        form, `GetBallpark(doWait=True)`, spins `WaitForBallpark` -- 30 x
        `blue.pyos.synchro.SleepSim(1000)` (`michelle.pyc:502-508`) [V] -- on
        the *simulation* clock, so it is polled on the wallclock here instead:
        if the simulation is not advancing, the client's own loop is the thing
        that hangs.
        """
        mich = get_michelle()
        if mich is None:
            return None
        park, _waited = wait_for(lambda: mich.GetBallpark(),
                                 timeout_ms=timeout_ms)
        return park

    def get_ego_ball(timeout_ms=120000):
        """`(park, ball)` once the ego ball is really in the park.

        **`bpReady` is not "there are balls".**  `Michelle.AddBallpark` sets
        `bpReady = False` at `michelle.pyc:444` and `True` at `:460`,
        immediately after `self.__bp.Start()` at `:459` and before any ball
        has been added -- and `Park.__init__` starts with `slimItems = {}`
        (`:1019`) [V].  So every scenario that reads the ego ball has to wait
        for the ball, not for the park.

        `ballpark.ego` is the ego ball's id and `ballpark.GetBall(id)` is
        `self.balls.get(id, None)` (`michelle.pyc:1888`) [V]; this is the
        same pair `Michelle.GetMyPosition` uses (`:591-598`) [V].
        """
        park = get_ballpark(timeout_ms=timeout_ms)
        if park is None:
            return None, None

        def probe():
            ego = park.ego
            if not ego:
                return None
            return park.GetBall(ego)

        ball, _waited = wait_for(probe, timeout_ms=timeout_ms)
        return park, ball

    def fmt(value, places=1):
        """A number for a human to read inside a `detail` string."""
        try:
            return ('%%.%df' % places) % float(value)
        except Exception:
            return text(value)

    def getattr_safe(obj, name, default=None):
        try:
            return getattr(obj, name, default)
        except Exception:
            return default

    # -- the scenario ------------------------------------------------------

    # -- fci block: shared helpers of the f-ci scenarios --------------------
    # THIS BLOCK IS BYTE-IDENTICAL IN EVERY f-ci SCENARIO.  The canonical copy
    # is `tools/client-ci/fci-block.txt`; edit that and run
    # `python3 -I tools/client-ci/sync-fci.py` (`--check` for the gate).  It is
    # copied, not imported, for the reason the prelude is: the harness deploys
    # exactly one file per scenario.  What it adds to the prelude: GM slash
    # commands from inside the client, calls on server services, the client's
    # own log read for tracebacks, a window census, and system picking from
    # the client's own map data.  Every client API named here is [I] or [A]
    # unless a line says [V]; a route that does not exist fails ITS OWN check
    # with the route's name in the detail, never the whole scenario.

    LOG_DIR = 'C:\\eve-logs'
    LOG_CAP = 6 * 1024 * 1024
    PARAMS_FILE = 'C:\\eve-logs\\client-ci\\fci.params'

    def safe(fn, *args, **kw):
        """`(ok, value)`.  Never raises; a failure's value is `Type: message`."""
        try:
            return True, fn(*args, **kw)
        except Exception as e:
            return False, '%s: %s' % (e.__class__.__name__, text(e))

    def get_svc(name):
        """A client service by name, preferring the running instance."""
        sm = builtin('sm')
        if sm is None:
            return None
        try:
            s = sm.GetServiceIfRunning(name)
            if s is not None:
                return s
        except Exception:
            pass
        try:
            return sm.GetService(name)
        except Exception:
            return None

    def rcall(service, method, *args):
        """`(ok, value)`: one call on a SERVER service, `sm.RemoteSvc(name)`.

        The same route the client's own services use (every row of
        `findings/client-calls.tsv` with kind `remotesvc`).  The reply is
        decoded by the client's own marshal, so a reply shape the client
        cannot read is a failure here exactly as it would be on screen.
        """
        sm = builtin('sm')
        if sm is None:
            return False, 'no sm'
        return safe(lambda: getattr(sm.RemoteSvc(service), method)(*args))

    def rcall_kw(service, method, args, kwargs):
        """`rcall` with keyword arguments (`GetCashBalance(1, accountKey=...)`)."""
        sm = builtin('sm')
        if sm is None:
            return False, 'no sm'
        return safe(lambda: getattr(sm.RemoteSvc(service), method)(*args, **kwargs))

    def uniq(n=4):
        """A short base-36 suffix, so a re-run in a persisted world does not
        collide with a name or ticker the last run took."""
        import random
        import time
        digits = '0123456789abcdefghijklmnopqrstuvwxyz'
        rng = random.Random(int(time.time() * 1000) % 2147483647)
        return ''.join([digits[rng.randint(0, 35)] for _i in range(n)])

    def gm(line):
        """`(ok, reply_text)`: a GM slash command, sent to the SERVER.

        `sm.RemoteSvc('slash').SlashCmd(line)` [V] (`docs/reference/session-roles.md`
        :445); the client's own `/createitem` and friends reach the server the
        same way, and the reply is the string the console would print.  The
        session needs the GM role, which the sandbox grants.
        """
        ok, value = rcall('slash', 'SlashCmd', line)
        # `/bots` and `/world` answer as a raised CustomInfo on purpose
        # [V service/gm.rs visible_reply]: the client shows the reply as an
        # info box.  That is a success, not a failure.  A real refusal is
        # still caught by the `forbid` lists ('no eve-bots', 'dev server').
        if not ok and 'msg=CustomInfo' in text(value) \
                and line.split(' ')[0].lower() in ('/bots', '/world'):
            ok = True
        return ok, text(value)

    def grant_and_settle(name_or_id, type_id, lvl):
        """`/giveskill`, then wait (<= 5 s) for a NEW `timeStamp` on that skill.

        The client's skill refresh (`skillsvc.py:174-200`) sets `timeStamp` on
        the skill it fetched (`:192`) and the server's `SkillInfo` carries no
        such field (`skill.rs:36-37`) [V], so a changed stamp means the handler
        is past its refetch yield.  Sending the next grant earlier lets two
        refreshes interleave.  The stamp before the grant is taken first: it
        survives until the next reset, and a stale one must not pass the wait.
        `skills.myskills` is None during a reset, hence `safe()`.  A timeout is
        said, never silent; the caller's own checks decide the verdict."""
        def stamp():
            sk_svc = get_svc('skills')
            ok, mine = safe(lambda: sk_svc.myskills) if sk_svc is not None else (False, None)
            if not ok or not isinstance(mine, dict):
                return None
            return getattr_safe(mine.get(type_id), 'timeStamp', None)
        prev = stamp()
        res = gm('/giveskill %s %s' % (text(name_or_id), text(lvl)))
        settled, waited = wait_for(
            lambda: stamp() is not None and stamp() != prev, timeout_ms=5000)
        if not settled:
            say('grant_and_settle: no new timeStamp for skill %s after %s ms'
                % (text(type_id), text(waited)))
        return res

    def gm_check(name, line, expect=None, forbid=None):
        """`gm(line)` as a check.  `expect` and `forbid` are lists of
        lower-case substrings of the reply.  Returns `(ok, reply)`."""
        ok, reply = gm(line)
        low = reply.lower()
        good = ok
        why = ''
        if good and expect:
            for e in expect:
                if e.lower() not in low:
                    good = False
                    why = ' [missing %r]' % e
                    break
        if good and forbid:
            for f in forbid:
                if f.lower() in low:
                    good = False
                    why = ' [found %r]' % f
                    break
        check(name, good, '%s -> %s%s' % (line, reply[:300], why))
        return good, reply

    def precondition(name, ok, detail=''):
        """A check that says the scenario could not even set up.  Same as
        `check`, named so a verdict reads 'could not set up' and not
        'the server is wrong'."""
        return check('PRECONDITION: ' + name, ok, detail)

    def shape(value, depth=0):
        """A short description of a reply: type, length, first keys."""
        try:
            if value is None:
                return 'None'
            kind = value.__class__.__name__
            if isinstance(value, (list, tuple)):
                return '%s[%d]' % (kind, len(value))
            if isinstance(value, dict):
                return 'dict{%d: %s}' % (len(value), text(sorted(value.keys())[:4]))
            for attr in ('header', 'lines'):
                if hasattr(value, attr):
                    try:
                        return '%s(%s=%s)' % (kind, attr, text(getattr(value, attr))[:100])
                    except Exception:
                        pass
            return '%s %s' % (kind, text(value)[:100])
        except Exception:
            return '<unprintable>'

    def rows_of(value):
        """The rows of a reply that may be a list, a dict, a Rowset (`.lines`)
        or a CRowset.  Returns a list (possibly empty), or None if unknown."""
        if value is None:
            return None
        if isinstance(value, (list, tuple)):
            return list(value)
        if isinstance(value, dict):
            return list(value.values())
        if getattr_safe(value, 'header') is not None:
            # a Rowset iterates as Row objects (rowset.py), which is how the
            # journal and the job board read it; `.lines` are bare lists
            try:
                return list(value)
            except Exception:
                pass
        lines = getattr_safe(value, 'lines')
        if lines is not None:
            return list(lines)
        try:
            return list(value)
        except Exception:
            return None

    # -- the client's own log: tracebacks and lines, since a mark ----------

    def client_log_path():
        try:
            import glob
            best = None
            best_t = -1
            for f in glob.glob(os.path.join(LOG_DIR, 'client-*.log')):
                try:
                    t = os.path.getmtime(f)
                except Exception:
                    continue
                if t > best_t:
                    best = f
                    best_t = t
            return best
        except Exception:
            return None

    def log_mark():
        """Where the newest client log ends now."""
        p = client_log_path()
        if p is None:
            return (None, 0)
        try:
            return (p, os.path.getsize(p))
        except Exception:
            return (p, 0)

    def log_since(mark):
        """`(text or None, detail)`: everything the client logged since `mark`.
        Waits a moment first: the client's log is buffered."""
        sleep_ms(1500)
        p, off = mark
        now = client_log_path()
        if p is None or now is None:
            return None, 'no client-*.log under %s' % LOG_DIR
        spans = [(p, off)] if p == now else [(p, off), (now, 0)]
        chunks = []
        for path, start in spans:
            try:
                f = open(path, 'rb')
                try:
                    f.seek(start)
                    chunks.append(f.read(LOG_CAP))
                finally:
                    f.close()
            except Exception as e:
                return None, 'cannot read %s: %s' % (path, text(e))
        blob = ''.join(chunks)
        return blob, '%d bytes of %s' % (len(blob), os.path.basename(now))

    def log_exceptions(blob, ignore=()):
        """`(count, first_three)`: the client brackets a Python traceback with
        `EXCEPTION #n` or `STACKTRACE #n` (`tools/client-log.sh --errors`)."""
        # the client's own watchdog warning while a scenario runs inside a
        # service call (xmpphandler/settingsSvc): not a fault of ours
        ignore = tuple(ignore) + (
            'Possible service deadlock detected',
            # the jail gives the client's CCP-side gateway nothing to answer
            # (docs/guides/jail.md): its own timeouts, not ours
            'GetEquippedForCharacterRequest', 'get goal definitions',
            'fetch definitions', 'SkillPlansErrorTimeout',
            'BluepyTasklet::_build_profile', 'SendClientStackTraceAlert',
            'Formatted exception info: | TimeoutException',
            # board_ship retries TryActivateShip until the client's ~9 s
            # session-change lockout has passed: each early call throws this
            # inside the client's MakeShipActive tasklet [V: client.log,
            # sessions.py:394 -- a refusal, nothing was changed]
            'PerformSessionChangeInProgress',
            # the jail has no route to a public-IP service (docs/guides/jail.md)
            'api.ipify.org',
            # more of the CCP-side gateway's timeouts (same cause, same jail)
            'TimeoutException', 'GoalMessengerError', 'unable to cache',
            'Exception in Error Logging', 'achievement_system', 'dailygoals',
            # a CCP UI localisation property with no handler, at login
            # (client.log _GetByMessageID 'min_value'): nothing we send
            "Failed to find the value for 'min_value'",
            # the station view reloading the PREVIOUS ship's model (the pod)
            # right after the board's session change: hangarView.py:505 ->
            # baseHangarBehaviours.py:63/148 -> clientDogmaLocation.py:442.
            # Seen on every board; the boarded ship works.  [A] a client race
            # -- falsifier: the pod missing from the hangar listing.
            'Dogma was unable to get invItem to load dogmaItem',
            # a loaded charge's sublocation prime builds blue.DBRow(sublocrd,
            # nine values) against our ELEVEN-column item descriptor
            # (service.rs item_columns: `singleton`/`stacksize` are physical
            # here, virtual in CCP's; docs/reference/dbrow.md Gap 4).  A warning, the
            # row is still made -- 801 of them in one fci_doc_3 run.  Known
            # server deviation, not a new fault: the run is not failed by it.
            'sequence is too short',
            # Build 3662168 gave the Neocom's Aura Guidance button this label
            # and shipped no such label: the client's own fault, once per login.
            "No label with name 'AuraGuidanceWnd'",
            # The gateway answers a content checkpoint 404 on purpose: there is
            # no CDN, and the client keeps its empty default data (gateway.rs).
            'fetchRequest for flatbuffer data raises Not Found(404)',
            # A session change (a /tr, a jump) swaps the ballpark under a
            # service that still holds the old one, which then asks the new
            # park for a ball that is not in it yet.  [A] a race at the swap --
            # falsifier: the ball still missing from the new park after the
            # scenario's wait for the ego ball.
            "'NoneType' object has no attribute 'SendMessageToItem'",
            # The camera reset on arrival, run before the new scene has a
            # camera.  [A] a fast client does it on a jump -- falsifier: a
            # camera stuck on the old system after the arrival.
            'Failed to reset camera',
            # The client's own Disconnect kills its receive tasklet while it
            # is in recv (xmppConnection.py 242, 445): 10058, then a
            # SystemError from that tasklet.  Chat logs in again.  [A] a race
            # in blue's socket -- falsifier: chat not connected after the
            # next login.
            'recv tasklet block failed',
            'xmppConnection.py(445) _receive_t')
        lines = blob.split('\n')
        heads = []
        n = 0
        for i in range(len(lines)):
            ln = lines[i]
            if 'EXCEPTION #' in ln or 'STACKTRACE #' in ln:
                # the whole record, up to its END line (the frames that name
                # the culprit can sit a dozen lines down)
                end = i + 1
                while end < len(lines) and end < i + 60 and \
                        'EXCEPTION END' not in lines[end] and 'STACKTRACE END' not in lines[end]:
                    end += 1
                block = ' | '.join([x.strip() for x in lines[i:end]])
                skip = False
                for ig in ignore:
                    if ig in block:
                        skip = True
                if skip:
                    continue
                n += 1
                if len(heads) < 3:
                    heads.append(block[:320])
        return n, heads

    def no_new_tracebacks(label, mark, ignore=()):
        """The check a human makes with `tools/client-log.sh --errors`.  It
        fails on an unreadable log too: 'could not look' is not 'all clear'."""
        blob, detail = log_since(mark)
        if blob is None:
            return check('client log has no new traceback: ' + label, False,
                         'COULD NOT READ THE LOG (%s)' % detail)
        n, heads = log_exceptions(blob, ignore)
        return check('client log has no new traceback: ' + label, n == 0,
                     '%d traceback(s) in %s%s'
                     % (n, detail, (' -- first: ' + ' ## '.join(heads)) if heads else ''))

    def log_lines_with(blob, needle, limit=3):
        out = []
        for ln in blob.split('\n'):
            if needle in ln:
                out.append(ln.strip()[:200])
                if len(out) >= limit:
                    break
        return out

    # -- windows ------------------------------------------------------------

    def window_names():
        """Class names (and window ids) of the open windows.  [A]: three routes,
        the first that answers wins."""
        names = []
        try:
            from carbonui.uicore import uicore
        except Exception:
            return names
        reg = getattr_safe(uicore, 'registry')
        wins = None
        for route in (lambda: reg.GetWindows(), lambda: reg.windows,
                      lambda: uicore.layer.main.children):
            ok, wins = safe(route)
            if ok and wins:
                break
            wins = None
        try:
            if isinstance(wins, dict):
                wins = list(wins.values())
            for w in (wins or []):
                names.append('%s:%s' % (w.__class__.__name__,
                                        text(getattr_safe(w, 'windowID'))))
        except Exception:
            pass
        return names

    def window_like(sub):
        sub = sub.lower()
        return [n for n in window_names() if sub in n.lower()]

    # -- the client's own map data, and the optional params file -----------

    def params():
        """`key=value` lines from PARAMS_FILE (the host may drop one beside
        the scenario to pin an id).  Empty when absent."""
        out = {}
        try:
            f = open(PARAMS_FILE, 'rb')
            try:
                for ln in f.read().split('\n'):
                    if '=' in ln and not ln.strip().startswith('#'):
                        k, v = ln.split('=', 1)
                        out[k.strip()] = v.strip()
            finally:
                f.close()
        except Exception:
            pass
        return out

    def map_systems(pred, limit=60):
        """Solar system ids from the CLIENT's static map data whose row passes
        `pred(row)`.  [A] `cfg.mapSystemCache` (id -> row with
        `securityStatus` and `factionID`); no id is written in this file."""
        cfg = builtin('cfg')
        out = []
        for route in (lambda: cfg.mapSystemCache,):
            ok, table = safe(route)
            if not ok or table is None:
                continue
            try:
                items = table.items() if hasattr(table, 'items') else list(table)
                for key, row in items:
                    ok2, good = safe(pred, row)
                    if ok2 and good:
                        out.append(int(key))
                        if len(out) >= limit:
                            return out
            except Exception:
                continue
        return out

    def type_ids_named(sub, limit=10, exact=False):
        """Type ids whose English name contains `sub` (equals it when `exact`),
        from the client's own `evetypes` (`cfg.invtypes` does not exist in this
        build: docs/reference/item-spawn.md 4.1 [V]).  Names come from the client.  The
        id -> lower-case name table is read once (about 50 k localisation
        lookups) and kept; `type_ids_named.note` says how that went."""
        out = []
        sub = sub.lower()
        import time
        table = getattr(type_ids_named, 'table', None)
        if table is None:
            table = []
            type_ids_named.note = 'no types'
            t0 = time.time()
            # the table is kept in a file beside the params, so only the first
            # scenario of a prefix pays for ~50 k localisation lookups
            # keyed by the client's build, so a client update never reads the
            # old build's names.  `boot` is NOT a builtin; it is `eveprefs.boot`
            # [V: autoexec.pyc does `str(boot.build)` after importing it from
            # eveprefs, and eveprefs/__init__.pyc defines it; census_cc.py reads
            # `boot.region` the same way].  An unreadable build gets its own
            # 'unknown' file, so it never reuses a keyed one.
            try:
                from eveprefs import boot as _boot
                bld = getattr_safe(_boot, 'build')
            except Exception:
                bld = None
            bld = ''.join([c for c in text(bld or 'unknown') if c.isalnum()]) or 'unknown'
            cache = os.path.join(os.path.dirname(PARAMS_FILE), 'typenames-%s.tsv' % bld)
            say('type-name cache file: %s' % cache)
            try:
                f = open(cache, 'rb')
                try:
                    for ln in f.read().split('\n'):
                        if '\t' in ln:
                            a, b = ln.split('\t', 1)
                            table.append((int(a), b))
                finally:
                    f.close()
                type_ids_named.note = '%d names from %s' % (len(table), cache)
            except Exception:
                table = []
            if not table:
                try:
                    evetypes = __import__('evetypes')
                    for tid in sorted(evetypes.GetAllTypeIDs()):
                        try:
                            table.append((int(tid), text(evetypes.GetEnglishName(tid)).lower()))
                        except Exception as e:
                            type_ids_named.note = 'GetEnglishName(%s): %s' % (tid, text(e)[:100])
                    type_ids_named.note = '%d names read in %.1f s' % (len(table), time.time() - t0)
                    try:
                        f = open(cache, 'wb')
                        try:
                            f.write('\n'.join(['%d\t%s' % (a, b.encode('utf-8') if isinstance(b, unicode) else b)
                                               for a, b in table]))
                        finally:
                            f.close()
                    except Exception:
                        pass
                except Exception as e:
                    type_ids_named.note = 'evetypes: ' + text(e)[:160]
            type_ids_named.table = table
        for tid, nm in table:
            if (nm == sub) if exact else (sub in nm):
                out.append(tid)
                if len(out) >= limit:
                    break
        return out

    def ship_items(flag=None):
        """`(ok, rows)`: the active ship's inventory (client `invCache`), the
        route `eve/devtools/script/svc_slash.py` itself uses.  [A]."""
        sess = builtin('session')
        inv = get_svc('invCache')
        if inv is None or sess is None or not getattr_safe(sess, 'shipid'):
            return False, 'no invCache or no ship'
        if flag is None:
            return safe(lambda: inv.GetInventoryFromId(sess.shipid).List())
        return safe(lambda: inv.GetInventoryFromId(sess.shipid).List(flag))

    def hangar_items():
        """`(ok, rows)`: the item hangar of the station we are docked in. [A]"""
        inv = get_svc('invCache')
        if inv is None:
            return False, 'no invCache'
        const = builtin('const')
        flag = getattr_safe(const, 'containerHangar', 10004)
        return safe(lambda: inv.GetInventory(flag).List())

    def count_type(rows, type_id):
        n = 0
        for r in (rows or []):
            if getattr_safe(r, 'typeID') == type_id:
                n += 1
        return n

    # -- items, ships, modules (all [A] routes; each says which one answered) --

    FLAG_HI0 = 27     # flagHiSlot0; med 19, low 11, rig 92 (slot flags)
    FLAG_DRONE_BAY = 87
    FLAG_MINING_HOLD = 134

    def dogma_loc():
        """`clientDogmaIM.GetDogmaLocation()` [V] (the prelude's `undock`
        cites it)."""
        d = get_svc('clientDogmaIM')
        if d is None:
            return None
        ok, loc = safe(lambda: d.GetDogmaLocation())
        return loc if ok else None

    def find_hangar_item(type_id, timeout_ms=15000):
        # A GM-created item reaches the client's hangar list a moment after
        # the slash answer (OnItemChange + OnInvalidateCacheForLocations are
        # handled on the client's own tasklets): poll, do not read once
        # [V: run 2026-10-28, fci_fighters -- rows grew 2,2,3,3,4,5 as the
        # scenario went through its types].
        def one():
            ok, rows = hangar_items()
            if not ok:
                return None
            for r in (rows or []):
                if getattr_safe(r, 'typeID') == type_id:
                    return r
            return None
        found, _w = wait_for(one, timeout_ms=timeout_ms, step_ms=500)
        ok, rows = hangar_items()
        if not ok:
            return None, 'hangar unreadable: %s' % text(rows)
        if found is not None:
            return found, '%d row(s) in the hangar' % len(rows or [])
        return None, 'type %s is not among the %d hangar row(s) after %d ms' % (
            text(type_id), len(rows or []), timeout_ms)

    def missing_skills_for(type_id):
        """`[(skillTypeID, level)]` the client says `type_id` still needs
        (`dogmaLocation.GetMissingSkills`, clientDogmaLocation.py:432 [V])."""
        d = dogma_loc()
        ok2, m = safe(lambda: d.GetMissingSkills(type_id))
        if not ok2 or not m:
            return []
        try:
            return list(m.items())
        except Exception:
            return [(k, 1) for k in m]

    def grant_skills_for(type_id):
        """GM-grant what `missing_skills_for` lists, then wait for the client
        to agree.  Fitting a module without them is allowed; ONLINING it is
        not (`OnlineHasSkillPrerequisites`) [V: run 2026-10-28].  Returns the
        list still missing."""
        for sk, lvl in missing_skills_for(type_id):
            ok3, nm = safe(lambda: __import__('evetypes').GetEnglishName(sk))
            if ok3 and nm:
                grant_and_settle(nm, sk, lvl)
        wait_for(lambda: not missing_skills_for(type_id), timeout_ms=10000)
        return missing_skills_for(type_id)

    def fit_module(type_id, flag):
        """Fit a hangar module into the active ship at slot `flag` by the
        client's own inventory `Add` (the drag-and-drop route).  `(ok, detail)`"""
        item, why = find_hangar_item(type_id)
        if item is None:
            return False, why
        sess = builtin('session')
        inv = get_svc('invCache')
        if inv is None or sess is None or not getattr_safe(sess, 'shipid'):
            return False, 'no invCache or no active ship'
        grant_skills_for(type_id)
        # the new ship's inventory moniker is not always bound the moment the
        # board returns (invCacheContainer.Add: 'NoneType' has no attribute
        # 'Add' on the first fit only) [V: run 2026-10-28]: retry a few times
        ok, v = False, None
        for _attempt in range(4):
            try:
                inv.GetInventoryFromId(sess.shipid).Add(
                    item.itemID, item.locationID, qty=1, flag=flag)
                ok, v = True, None
                break
            except Exception:
                ok, v = False, ' | '.join(traceback.format_exc().strip().split('\n')[-7:])
                sleep_ms(3000)
        if not ok:
            return False, 'inventory.Add raised %s' % text(v)[:700]
        fitted, waited = wait_for(
            lambda: count_type(ship_items(flag)[1] if ship_items(flag)[0] else [],
                               type_id) > 0, timeout_ms=10000)
        return bool(fitted), ('fitted item %s at flag %s (%s ms)' % (
            text(item.itemID), text(flag), text(waited))
            if fitted else 'Add returned but the ship shows nothing at flag %s'
            % text(flag))

    def module_item(type_id, flag):
        ok, rows = ship_items(flag)
        if not ok:
            return None
        for r in (rows or []):
            if getattr_safe(r, 'typeID') == type_id:
                return r
        return None

    def online_module(item_id):
        """`(ok, detail)`: `clientDogmaLocation.OnlineModule` [V], then wait
        for the godma item to read online."""
        loc = dogma_loc()
        if loc is None:
            return False, 'no dogma location'
        # a freshly fitted module reaches the client's dogma location a moment
        # after the inventory shows it (KeyError otherwise) [V: run 2026-10-28]
        wait_for(lambda: item_id in getattr_safe(loc, 'dogmaItems', {}), timeout_ms=10000)
        ok, v = safe(lambda: loc.OnlineModule(item_id))
        if not ok and 'EffectAlreadyActive2' not in text(v):
            return False, 'OnlineModule raised %s' % text(v)
        # EffectAlreadyActive2: the online effect already runs in the client
        # (the server onlines a module it fits); fall through and check godma

        def on():
            g = get_svc('godma')
            it = g.GetItem(item_id) if g is not None else None
            return bool(getattr_safe(it, 'isOnline'))
        up, waited = wait_for(on, timeout_ms=10000)
        return bool(up), 'godma isOnline after %s ms: %s' % (text(waited), text(bool(up)))

    def module_effect_name(item_id):
        g = get_svc('godma')
        ok, it = safe(lambda: g.GetItem(item_id))
        if not ok or it is None:
            return None
        ok, name = safe(lambda: it.defaultEffect.effectName)
        return name if ok else None

    def activate_module(item_id, target_id=None):
        """`(ok, detail)`.  The module button's route: `godma.GetStateManager().
        Activate(itemID, effectName, target, repeat)` [V: godma.py:1984, called
        by the module wrapper at :947].  NOT `dogmaLocation.Activate`, which
        takes an effectID and only starts the effect locally (clientDogmaLocation
        .py:392) -- the server never hears of it [V: run 2026-10-28, no heat]."""
        g = get_svc('godma')
        ok0, stm = safe(lambda: g.GetStateManager())
        eff = module_effect_name(item_id)
        if not ok0 or stm is None or eff is None:
            return False, 'no state manager (%s) or no default effect (%s)' % (
                text(stm), text(eff))
        okc, cat = safe(lambda: g.GetItem(item_id).defaultEffect.effectCategory)
        if target_id is None and okc and cat == 2:
            # a targeted effect (dgmEffTarget, category 2) needs its target
            # (DeniedActivateTargetNotPresent): the module button passes the
            # active target [V: shipmodulebutton.py:1260]
            tsvc = get_svc('target')
            ok5, tid = safe(lambda: tsvc.GetActiveTargetID())
            target_id = tid if ok5 and tid else None
        ok, v = safe(lambda: stm.Activate(item_id, eff, target_id, 1000))  # the module button's repeat (smb:1565): 0 is a ONE-SHOT
        if ok:
            return True, 'statemanager.Activate(itemID, effectName, target, repeat) took (effect %s)' % eff
        return False, 'statemanager.Activate raised %s' % text(v)[:160]

    def deactivate_module(item_id):
        g = get_svc('godma')
        ok0, stm = safe(lambda: g.GetStateManager())
        eff = module_effect_name(item_id)
        if not ok0 or stm is None or eff is None:
            return False, 'no state manager or default effect'
        ok, v = safe(lambda: stm.Deactivate(item_id, eff))
        return ok, 'statemanager.Deactivate(itemID, effectName): %s' % text(v)[:120]

    first_board_done = []

    def board_ship(type_id, timeout_ms=30000):
        """Make the hangar ship of `type_id` active, as a double-click does:
        `svc.station.TryActivateShip(invItem)` [V: svc_fittingspawner calls it].
        `(ok, detail)`; waits for `session.shipid` to become that ship."""
        say('board_ship: looking for type %s in the hangar' % text(type_id))
        item, why = find_hangar_item(type_id)
        say('board_ship: hangar says %s' % text(why))
        if item is None:
            return False, why
        st = get_svc('station')
        if st is None:
            return False, 'no station service'
        # TryActivateShip with missing skills OPENS A MODAL DIALOG
        # (prompt_missing_skill_requirements, station/base.py:210) and never
        # returns: the scenario thread hangs for ever [V: client.log
        # skill_requirement_dialog].  So give the skills first (GM), and
        # refuse to call when the client still sees any missing.
        def missing_skills():
            """None when the client cannot say (an exception, e.g. while its
            skill table reloads after a /giveskill): never read as 'none'."""
            d = dogma_loc()
            ok2, m = safe(lambda: d.GetMissingSkills(type_id))
            if not ok2:
                return None
            if not m:
                return []
            try:
                return list(m.items())
            except Exception:
                return [(k, 1) for k in m]

        def settle_skills():
            for sk, lvl in (missing_skills() or []):
                ok3, nm = safe(lambda: __import__('evetypes').GetEnglishName(sk))
                if ok3 and nm:
                    grant_and_settle(nm, sk, lvl)
            wait_for(lambda: missing_skills() == [], timeout_ms=10000)
            return missing_skills()
        check('board_ship: found the ship in the hangar', True, text(why))
        left = settle_skills()
        if left != []:
            return False, 'the client still sees missing skills %s' % text(left)
        # The client refuses a session change within ~9 s of the last one
        # (login's 'charsel'): eveSessionMgr "Too frequent session change
        # attempts", PerformSessionChangeInProgress thrown inside the
        # MakeShipActive tasklet [V: client.log, sessions.py PerformSelectiveSessionChange].
        # TryActivateShip itself returns normally, so one call is silently
        # lost when boarding comes straight after login.  Retry until it takes.
        import time as _t
        t0 = _t.time()
        n = 0
        if not first_board_done:
            # the first change after login's charsel: let the lockout pass
            # before the first call (later boards are 10 s+ apart anyway)
            first_board_done.append(1)
            sleep_ms(9000)

        def changed():
            s = builtin('session')
            return getattr_safe(s, 'shipid') == item.itemID
        while True:
            n += 1
            # every call, not just the first: a call with a skill missing
            # opens the modal dialog and the thread never returns
            left = settle_skills()
            if left != []:
                return False, 'the client sees missing skills before call %d: %s' % (n, text(left))
            say('board_ship: TryActivateShip call %d' % n)
            check('board_ship: TryActivateShip call %d (skills settled: %s)' % (n, text(left)), True,
                  'missing now: %s' % text(missing_skills()))
            ok, v = safe(lambda: st.TryActivateShip(item))
            say('board_ship: call %d returned %s' % (n, text(ok)))
            if not ok:
                return False, 'TryActivateShip raised %s' % text(v)
            up, waited = wait_for(changed, timeout_ms=min(4000, timeout_ms))
            if up or (_t.time() - t0) * 1000 > timeout_ms:
                break
        return bool(up), 'session.shipid == %s after %s s, %d call(s): %s' % (
            text(item.itemID), text(int(_t.time() - t0)), n, text(bool(up)))

    def wait_ship_item(type_id, flag, timeout_ms=15000):
        got, waited = wait_for(lambda: module_item(type_id, flag), timeout_ms=timeout_ms)
        return got, waited

    # -- flying: belts, rocks, approach, lock (the ballpark API the older
    #    scenarios already use is [V]; the targeting calls are [A]) ----------

    GROUP_ASTEROID_BELT = 9
    CATEGORY_ASTEROID = 25
    MODE_WARP = 3          # DSTBALL_WARP, as `modes()` reads it in the prelude users

    def const_of(name, default):
        return getattr_safe(builtin('const'), name, default)

    def slims_where(park, pred):
        """`[(ballID, slim)]` of the park's slim items passing `pred(slim)`."""
        out = []
        try:
            for ballid, slim in park.slimItems.items():
                ok, good = safe(pred, slim)
                if ok and good:
                    out.append((ballid, slim))
        except Exception:
            pass
        return out

    def dist_to(park, ball_id):
        ok, d = safe(lambda: park.GetSurfaceDist(park.ego, ball_id))
        return d if ok else None

    def warp_to(park, ball_id, timeout_ms=120000, min_range=0.0):
        """Warp the ego ship to a ball and wait until the warp is over.
        `(ok, detail)`.  `michelle.CmdWarpToStuff` is the route
        `throttle_and_warp` and `warp_blob` run for real [V]."""
        mich = get_michelle()
        ego = park.GetBall(park.ego)
        ok, v = safe(lambda: mich.CmdWarpToStuff('item', ball_id, minRange=min_range))
        if not ok:
            return False, 'CmdWarpToStuff raised %s' % text(v)
        went, _w = wait_for(lambda: getattr_safe(ego, 'mode') == MODE_WARP, timeout_ms=30000)
        if not went:
            return False, 'never entered DSTBALL_WARP (mode %s)' % text(getattr_safe(ego, 'mode'))
        landed, waited = wait_for(lambda: getattr_safe(ego, 'mode') != MODE_WARP,
                                  timeout_ms=timeout_ms, step_ms=500)
        return bool(landed), 'warp over after %s ms, %s m from the target' % (
            text(waited), fmt(dist_to(park, ball_id), 0))

    def approach(park, ball_id, within_m, timeout_ms=200000):
        """`CmdFollowBall(ball, range)` [A] then wait to be inside `within_m`."""
        rbp = get_michelle().GetRemotePark()
        ok, v = safe(lambda: rbp.CmdFollowBall(ball_id, max(50, int(within_m / 2))))
        if not ok:
            return False, 'CmdFollowBall raised %s' % text(v)
        # a ship leaves the station with its speed fraction at 0 and Follow
        # does not raise it: the throttle is the pilot's [V: run 2026-10-28,
        # 25112 m after 200 s with the follow accepted; throttle_and_warp
        # sends CmdSetSpeedFraction(1.0) the same way]
        safe(lambda: rbp.CmdSetSpeedFraction(1.0))
        trail = []

        def close_enough():
            d = dist_to(park, ball_id)
            if len(trail) < 400 and (not trail or len(trail) % 40 == 0):
                eb = park.GetBall(park.ego)
                sp = (getattr_safe(eb, 'vx', 0.0) ** 2 + getattr_safe(eb, 'vy', 0.0) ** 2
                      + getattr_safe(eb, 'vz', 0.0) ** 2) ** 0.5
                trail.append('%s m @ %s m/s' % (fmt(d, 0), fmt(sp, 0)))
            trail.append('.')
            return (d or 1e12) < within_m
        near, waited = wait_for(close_enough, timeout_ms=timeout_ms, step_ms=500)
        return bool(near), '%s m from the target after %s ms; trail %s' % (
            fmt(dist_to(park, ball_id), 0), text(waited),
            text([t for t in trail if t != '.'][:8]))

    def lock_target(ball_id, timeout_ms=30000):
        """`svc.target.TryLockTarget` [A] and wait for it in `GetTargets()`."""
        tgt = get_svc('target')
        if tgt is None:
            return False, 'no svc.target'
        ok, v = safe(lambda: tgt.TryLockTarget(ball_id))
        if not ok:
            return False, 'TryLockTarget raised %s' % text(v)
        got, waited = wait_for(lambda: ball_id in (tgt.GetTargets() or {}),
                               timeout_ms=timeout_ms)
        return bool(got), 'in svc.target.GetTargets() after %s ms: %s' % (
            text(waited), text(bool(got)))

    def go_to_rock(park, within_m=3000):
        """Warp to the nearest belt, find a rock, approach it.
        `(rock_ball_id or None, detail)`."""
        belts = slims_where(park, lambda s: getattr_safe(s, 'groupID')
                            == const_of('groupAsteroidBelt', GROUP_ASTEROID_BELT))
        if not belts:
            return None, 'no asteroid belt in the park'
        best = sorted([(dist_to(park, b) or 1e18, b) for b, _s in belts])[0][1]
        ok, detail = warp_to(park, best)
        if not ok:
            return None, 'warp to belt %s: %s' % (text(best), detail)
        rocks, waited = wait_for(
            lambda: slims_where(park, lambda s: getattr_safe(s, 'categoryID')
                                == const_of('categoryAsteroid', CATEGORY_ASTEROID)),
            timeout_ms=20000)
        if not rocks:
            return None, 'no asteroid appeared within %s ms of landing' % text(waited)
        rock = sorted([(dist_to(park, r) or 1e18, r) for r, _s in rocks])[0][1]
        ok, d2 = approach(park, rock, within_m)
        return (rock if ok else None), 'belt %s; rock %s of %d; %s' % (
            text(best), text(rock), len(rocks), d2)

    def module_active(item_id):
        """True/False if the client can tell whether a module's default effect
        is running, else None.  [A] `godmaItem.effects[name].isActive`."""
        eff = module_effect_name(item_id)
        g = get_svc('godma')
        ok, it = safe(lambda: g.GetItem(item_id))
        if ok and it is not None and eff:
            ok2, v = safe(lambda: bool(it.effects[eff].isActive))
            if ok2:
                return v
        return None

    def load_ammo(module_ids, charge_type_id, charge_item):
        """Load a hangar/cargo charge stack into modules.  [A]
        `dogmaLocation.LoadAmmoToModules(shipID, moduleIDs, chargeItemIDs,
        ammoLocationID)` [V: clientDogmaLocation.py:957, 4 args + self].
        `charge_type_id` is kept for the callers and unused.  `(ok, detail)`."""
        loc = dogma_loc()
        sess = builtin('session')
        if loc is None:
            return False, 'no dogma location'
        ok, v = safe(lambda: loc.LoadAmmoToModules(
            sess.shipid, list(module_ids), [charge_item.itemID],
            charge_item.locationID))
        return ok, 'LoadAmmoToModules: %s' % (text(v)[:160] if not ok else 'accepted')

    # -- an alliance to hold things with (used by the sov scenarios) --------

    def ensure_alliance():
        """`(corp_id, alliance_id, detail)`.  The character's alliance, founded
        if it has none: GM ISK and skill, `AddCorporation`, a Master Wallet
        deposit, `CreateAlliance` (the calls `fci_corp_alliance` asserts one by
        one).  [A] `AddCorporation`'s positional shape; the server reads it by
        argument name."""
        sess = builtin('session')
        aid = getattr_safe(sess, 'allianceid')
        if aid:
            return getattr_safe(sess, 'corpid'), aid, 'already in alliance %s' % text(aid)
        gm('/giveisk 3000000000')
        gm('/giveskill Corporation Management 1')
        sfx = uniq(4)
        old = getattr_safe(sess, 'corpid')
        ok, v = rcall('corpRegistry', 'AddCorporation', 'Fci Corp ' + sfx, 'F' + sfx,
                      'f-ci', '', 0.0, 1, 1, 1, 1, 1, 1, True, False, 0.0)
        if not ok:
            return None, None, 'AddCorporation: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') != old,
                           timeout_ms=30000)
        corp = getattr_safe(builtin('session'), 'corpid')
        if not got:
            return None, None, 'session.corpid never changed from %s' % text(old)
        ok, v = rcall_kw('account', 'GiveCash', (corp, 1000000000.0, 'fee'),
                         {'toAccountKey': 1000})
        if not ok:
            return corp, None, 'Master Wallet deposit: ' + text(v)
        ok, v = rcall('corpRegistry', 'CreateAlliance', 'Fci Alliance ' + sfx, 'A' + sfx,
                      'f-ci', '')
        if not ok:
            return corp, None, 'CreateAlliance: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'allianceid'),
                           timeout_ms=30000)
        return corp, got, ('founded alliance %s' % text(got)) if got else 'allianceid never arrived'

    # -- end of the fci block ------------------------------------------------

    # Names are [A]; the first one the client knows is used.
    HULLS = ['Merlin', 'Rifter', 'Incursus', 'Punisher', 'Kestrel', 'Condor']
    HI = ['125mm Gatling AutoCannon I', '150mm Light AutoCannon I',
          '200mm AutoCannon I', 'Light Neutron Blaster I', '75mm Gatling Rail I',
          'Dual Light Pulse Laser I']
    MED = ['1MN Afterburner I', 'Warp Scrambler I', 'Small Shield Booster I']
    LOW = ['Damage Control I', 'Small Armor Repairer I', 'Reinforced Bulkheads I']

    # How long a person would wait for the window to follow a drop.
    REDRAW_MS = 6000

    def first_known(names):
        for nm in names:
            for t in type_ids_named(nm, limit=3, exact=True):
                return t, nm
        return None, None

    def dogma_has(item_id):
        loc = dogma_loc()
        return loc is not None and item_id in getattr_safe(loc, 'dogmaItems', {})

    def diag(name, detail):
        """A DIAGNOSTIC: recorded in the verdict file and the client log so the
        handback can read it.  Always green -- it asserts nothing."""
        say('DIAG %s -- %s' % (text(name), text(detail)))
        checks.append(('DIAG (not a verdict): ' + text(name), True, text(detail)[:1500]))

    # -- what the client received (diagnostics only) ---------------------------

    seen = {'item_change': [], 'dogma_change': [], 'dogma_raise': [], 'update_slots': [],
            'inv_raise': [], 'calls': []}

    def change_text(change):
        try:
            return '{%s}' % ', '.join('%s: %s' % (text(k), text(v))
                                      for k, v in sorted(change.items()))
        except Exception:
            return text(change)

    def install_taps(controller):
        """Wrap, on the live instances, what the redraw path goes through:
        `invCache.OnItemChange` (the push), the dogma location's
        `OnItemChange`, the controller's `OnDogmaItemChange` and
        `_UpdateSlots` (fittingController.py:112, :333).  Each wrapper records
        and calls the original; one that raises is recorded and re-raised."""
        undo = []

        def wrap(obj, name, record, raise_key):
            orig = getattr(obj, name)

            def wrapper(*a, **kw):
                try:
                    record(a, kw)
                except Exception:
                    pass
                try:
                    return orig(*a, **kw)
                except Exception:
                    seen[raise_key].append('%s: %s' % (name, ' | '.join(
                        traceback.format_exc().strip().split('\n')[-4:])))
                    raise
            setattr(obj, name, wrapper)
            undo.append((obj, name))

        def rec_item(a, kw):
            item, change = a[0], a[1]
            seen['item_change'].append('item %s type %s loc %s flag %s change %s' % (
                text(getattr_safe(item, 'itemID')), text(getattr_safe(item, 'typeID')),
                text(getattr_safe(item, 'locationID')), text(getattr_safe(item, 'flagID')),
                change_text(change)))

        def rec_dogma(a, kw):
            item, change = a[0], a[1]
            seen['dogma_change'].append('item %s loc %s flag %s change %s' % (
                text(getattr_safe(item, 'itemID')), text(getattr_safe(item, 'locationID')),
                text(getattr_safe(item, 'flagID')), change_text(change)))

        inv = get_svc('invCache')
        loc = dogma_loc()
        if inv is not None:
            wrap(inv, 'OnItemChange', rec_item, 'inv_raise')
        if loc is not None:
            wrap(loc, 'OnItemChange', lambda a, kw: None, 'dogma_raise')
        wrap(controller, 'OnDogmaItemChange', rec_dogma, 'dogma_raise')
        wrap(controller, '_UpdateSlots',
             lambda a, kw: seen['update_slots'].append('_UpdateSlots'), 'dogma_raise')

        def remove():
            for obj, name in undo:
                try:
                    delattr(obj, name)
                except Exception:
                    pass
        return remove

    def clear_seen():
        for k in seen:
            del seen[k][:]

    # -- the fitting window, as the owner has it open ------------------------

    def open_window():
        """`(window, why)`.  `FittingWindow.Open()` is what `eveCommands.OpenFitting`
        reaches (eveCommands.py:2358); the window builds its own controller for
        the active ship (fittingWnd.py:97)."""
        def go():
            mod = __import__('eve.client.script.ui.shared.fittingScreen.fittingWnd',
                             fromlist=['FittingWindow'])
            wnd = mod.FittingWindow.GetIfOpen()
            if wnd is None or getattr_safe(wnd, 'destroyed'):
                wnd = mod.FittingWindow.Open()
            return wnd
        ok, wnd = safe(go)
        if not ok:
            return None, text(wnd)
        return wnd, 'window %s' % text(wnd.__class__.__name__)

    def window_ready(wnd):
        ctl = getattr_safe(wnd, 'controller')
        return bool(getattr_safe(wnd, 'windowReady')) and ctl is not None and \
            bool(getattr_safe(ctl, 'slotsByFlagID'))

    def find_child(root, class_name, depth=0):
        """First descendant of `root` whose class is called `class_name`."""
        if depth > 12 or root is None:
            return None
        if root.__class__.__name__ == class_name:
            return root
        for ch in list(getattr_safe(root, 'children', None) or []):
            hit = find_child(ch, class_name, depth + 1)
            if hit is not None:
                return hit
        return None

    class DragNode(object):
        """What an inventory row drags: `listentry.InvItem` with `.rec`
        (fittingSlotController.py GetDroppedItems)."""
        __guid__ = 'listentry.InvItem'

        def __init__(self, rec):
            self.rec = rec
            self.itemID = getattr_safe(rec, 'itemID')
            self.typeID = getattr_safe(rec, 'typeID')

    SLOT_FLAGS = range(11, 35)     # lo 11-18, med 19-26, hi 27-34

    def fitted_modules():
        """`{flagID: row}` of the ship's modules by the client's inventory."""
        ok, rows = ship_items()
        out = {}
        if not ok:
            return out
        for r in (rows or []):
            f = getattr_safe(r, 'flagID')
            if f in SLOT_FLAGS:
                out[f] = r
        return out

    def slot_ids(controller):
        out = {}
        for f, s in (getattr_safe(controller, 'slotsByFlagID', {}) or {}).items():
            if f in SLOT_FLAGS:
                mid = s.GetModuleID()
                if mid:
                    out[f] = mid
        return out

    def hangar_rows(type_id):
        ok, rows = hangar_items()
        return [r for r in (rows or []) if ok and getattr_safe(r, 'typeID') == type_id]

    ctx = {'wnd': None, 'inv_wnd': None}

    def type_name(type_id):
        """`evetypes.GetName(type_id)`, the text slot.py:274 puts in `hint`."""
        ok, v = safe(lambda: __import__('evetypes').GetName(type_id))
        return v if ok else None

    def slot_widgets(wnd):
        """`{flagID: FittingSlot widget}` of the open fitting window:
        `FittingCont.slots` (baseFitting.py:38 sets it, :42 fills it from
        `FittingCenter.GetSlots()`, fittingCenter.py:68/:100) [A: keyed by
        flagID; falsifier: an empty dict, which callers make a PRECONDITION]."""
        cont = find_child(wnd, 'FittingCont')
        slots = getattr_safe(cont, 'slots', None)
        try:
            return dict(slots) if slots else {}
        except Exception:
            return {}

    def widget_view(wnd, flag):
        """`(hint, controller module typeID, destroyed)` of the slot widget at
        `flag`, or None if there is no such widget."""
        w = slot_widgets(wnd).get(flag)
        if w is None:
            return None
        ctl = getattr_safe(w, 'controller')
        ok, tid = safe(lambda: ctl.GetModuleTypeID())
        return (getattr_safe(w, 'hint'), tid if ok else None, bool(getattr_safe(w, 'destroyed')))

    def widgets_show(wnd, expect):
        """`{flag: hint}` when EVERY widget in `expect` ({flag: typeID}) has
        the module's name as its hint, else None.  An empty `expect` is None
        (nothing to show is not a pass)."""
        if not expect:
            return None
        out = {}
        for f, t in expect.items():
            v = widget_view(wnd, f)
            nm = type_name(t)
            if v is None or v[2] or nm is None or text(v[0]) != text(nm):
                return None
            out[f] = v[0]
        return out

    def widgets_empty(wnd, flags):
        """True when every widget in `flags` shows its empty-slot hint
        (`_emptyHint`, baseSlot.py:34; slot.py:283 sets it on an empty slot)."""
        if not flags:
            return None
        for f in flags:
            w = slot_widgets(wnd).get(f)
            if w is None or getattr_safe(w, 'destroyed'):
                return None
            if text(getattr_safe(w, 'hint')) != text(getattr_safe(w, '_emptyHint')):
                return None
        return True

    def widget_text(wnd, flags):
        return text(dict((f, widget_view(wnd, f)) for f in flags))

    def unfit_everything(controller):
        """Every module off the ship, by the inventory `Add` to the hangar, and
        wait until the window's slots agree."""
        const = builtin('const')
        hangar = get_svc('invCache').GetInventory(getattr_safe(const, 'containerHangar', 10004))
        shipid = getattr_safe(builtin('session'), 'shipid')
        for f, r in fitted_modules().items():
            safe(lambda: hangar.Add(r.itemID, shipid, flag=getattr_safe(const, 'flagHangar', 4)))
        wait_for(lambda: not fitted_modules() or None, timeout_ms=15000)
        wait_for(lambda: not slot_ids(controller) or None, timeout_ms=10000)
        return not fitted_modules(), not slot_ids(controller)

    def run_case(label, controller, expect_n, expect_types, drop):
        """Do one drop, then watch the window.

        Checks: the drop's modules are on the ship (the server did it), and
        each fitted slot's controller `GetModuleID()` becomes that item's id
        within REDRAW_MS.  Everything else is a diagnostic."""
        mark = log_mark()
        clear_seen()
        wnd = ctx['wnd']
        prev_flags = sorted(fitted_modules().keys())
        gone, ctl_gone = unfit_everything(controller)
        precondition('%s: the ship and the window start empty' % label, gone and ctl_gone,
                     'ship empty %s, slots empty %s' % (text(gone), text(ctl_gone)))
        if not (gone and ctl_gone):
            return
        if prev_flags:
            emp, _w = wait_for(lambda: widgets_empty(wnd, prev_flags), timeout_ms=REDRAW_MS)
            check('%s: after the unfit the slot widgets %s show the empty hint within %d ms'
                  % (label, text(prev_flags), REDRAW_MS), bool(emp),
                  'widgets (hint, controller type, destroyed): %s' % widget_text(wnd, prev_flags))
        clear_seen()
        ok, v = safe(drop)
        check('%s: the drop call itself did not raise' % label, ok, text(v)[:300])

        def on_ship():
            mods = fitted_modules()
            hits = dict((f, r) for f, r in mods.items()
                        if getattr_safe(r, 'typeID') in expect_types)
            return hits if len(hits) >= expect_n else None
        hits, w_ship = wait_for(on_ship, timeout_ms=15000)
        hits = hits or dict((f, r) for f, r in fitted_modules().items()
                            if getattr_safe(r, 'typeID') in expect_types)
        check('%s: the server fitted %d module(s) (client inventory)' % (label, expect_n),
              len(hits) >= expect_n, '%d fitted after %s ms: %s' % (
                  len(hits), text(w_ship),
                  text(dict((f, getattr_safe(r, 'itemID')) for f, r in hits.items()))))

        def redrawn():
            now = slot_ids(controller)
            for f, r in hits.items():
                if now.get(f) != getattr_safe(r, 'itemID'):
                    return None
            return now if hits else None
        expect = dict((f, getattr_safe(r, 'typeID')) for f, r in hits.items())
        shown, w_show = wait_for(lambda: widgets_show(wnd, expect), timeout_ms=REDRAW_MS)
        check('%s: the slot widgets show the fitted modules (hint == module name) within %d ms'
              % (label, REDRAW_MS), bool(shown),
              'widgets (hint, controller type, destroyed) %s, expected names %s (after %s ms)'
              % (widget_text(wnd, sorted(expect)),
                 text(dict((f, type_name(t)) for f, t in expect.items())), text(w_show)))
        drawn, w_draw = wait_for(redrawn, timeout_ms=REDRAW_MS) if not shown else (redrawn(), 0)
        diag('%s: slot controllers show the fitted ids (the old check) within %d ms'
             % (label, REDRAW_MS),
             '%s: controllers say %s, inventory says %s (ship seen after %s ms, slots after %s ms)'
             % ('yes' if drawn else 'NO', text(slot_ids(controller)),
                text(dict((f, getattr_safe(r, 'itemID')) for f, r in hits.items())),
                text(w_ship), text(w_draw)))
        # -- diagnostics: where it broke if it did --
        late, w_late = wait_for(lambda: widgets_show(wnd, expect), timeout_ms=15000) if not shown else (shown, 0)
        diag('%s: slot widgets caught up later' % label,
             'yes after a further %s ms' % text(w_late) if (late and not shown)
             else ('not within 15 s more' if not shown else 'not needed'))
        diag('%s: OnItemChange received by invCache (%d)' % (label, len(seen['item_change'])),
             ' || '.join(seen['item_change'][:12]))
        diag('%s: dogma OnDogmaItemChange calls (%d), _UpdateSlots calls (%d)' % (
            label, len(seen['dogma_change']), len(seen['update_slots'])),
            ' || '.join(seen['dogma_change'][:12]))
        diag('%s: exceptions in the redraw path' % label,
             ' || '.join(seen['dogma_raise'] + seen['inv_raise']) or 'none')
        dogma_view = {}
        for f, r in hits.items():
            dogma_view[f] = dogma_has(getattr_safe(r, 'itemID'))
        diag('%s: dogma location holds the fitted items' % label, text(dogma_view))
        ok_l, why = safe(lambda: [(m.flagID, m.itemID) for m in controller.GetFittedModules()])
        diag('%s: controller.GetFittedModules() (flag, id)' % label, text(why)[:400])
        blob, detail = log_since(mark)
        if blob is None:
            diag('%s: client log' % label, detail)
        else:
            n, heads = log_exceptions(blob)
            diag('%s: client log exceptions since the drop (%d)' % (label, n),
                 ' ## '.join(heads))
            diag('%s: client log lines on fitting' % label, ' || '.join(
                log_lines_with(blob, 'Fitting item as a result from item change', 3)
                + log_lines_with(blob, 'EffectAlreadyActive', 2)
                + log_lines_with(blob, 'SlotAlreadyOccupied', 3)))

    def hangar_window():
        """`(inventory window, why)` of the station hangar, opened the way the
        client opens it (invWindow.py:932 `Inventory.OpenOrShow`; invConst.py:61
        STATION_ITEMS = 'StationItems')."""
        def go():
            mod = __import__('eve.client.script.ui.shared.inventory.invWindow',
                             fromlist=['Inventory'])
            return mod.Inventory.OpenOrShow(
                invID=('StationItems', getattr_safe(builtin('session'), 'stationid')))
        ok, w = safe(go)
        if not ok:
            return None, text(w)[:300]
        return w, 'window %s' % text(w.__class__.__name__)

    def find_entry(root, item_id):
        """The hangar window's row widget for `item_id`: an `xtriui.InvItem`
        (icon view) or `listentry.InvItem` (list view) whose `.rec.itemID` it is
        (item.py:167, :1097; `.rec` item.py:668)."""
        todo = [(root, 0)]
        seen_n = 0
        while todo and seen_n < 30000:
            o, d = todo.pop()
            seen_n += 1
            rec = getattr_safe(o, 'rec')
            if rec is not None and getattr_safe(o, '__guid__') in (
                    'xtriui.InvItem', 'listentry.InvItem') \
                    and getattr_safe(rec, 'itemID') == item_id:
                return o
            if d < 30:
                for ch in list(getattr_safe(o, 'children', None) or []):
                    todo.append((ch, d + 1))
        return None

    def install_call_taps():
        """Record which server call a drop sends: the ship inventory's `Add`
        (clientDogmaLocation.TryFit, :645-648) or its moniker's `MultiAdd`
        (tryFit.py:133).  Returns `(remove, why)`."""
        undo = []
        sess = builtin('session')
        ok, sinv = safe(lambda: get_svc('invCache').GetInventoryFromId(
            sess.shipid, locationID=sess.stationid))
        if not ok:
            return (lambda: None), 'no ship inventory: %s' % text(sinv)[:200]
        notes = []

        def wrap(obj, name, fmt):
            try:
                orig = getattr(obj, name)

                def wrapper(*a, **kw):
                    try:
                        seen['calls'].append(fmt(a, kw))
                    except Exception:
                        seen['calls'].append('%s (args unreadable)' % name)
                    return orig(*a, **kw)
                setattr(obj, name, wrapper)
                undo.append((obj, name))
            except Exception as e:
                notes.append('%s not tapped: %s' % (name, text(e)[:100]))
        wrap(sinv, 'Add', lambda a, kw: 'Add item=%s flag=%s qty=%s' % (
            text(a[0]), text(kw.get('flag')), text(kw.get('qty'))))
        wrap(getattr_safe(sinv, 'moniker'), 'MultiAdd', lambda a, kw: 'MultiAdd n=%d flag=%s' % (
            len(a[0]), text(kw.get('flag'))))

        def remove():
            for obj, name in undo:
                try:
                    delattr(obj, name)
                except Exception:
                    pass
        return remove, '; '.join(notes) or 'Add and MultiAdd tapped'

    def case_e(controller, med, med0, via_slot, hover, rep):
        """The owner's sequence (see the module docstring, (e)).  Every
        `check` names something that can be false; what could not be set up
        is a PRECONDITION, never a pass."""
        wnd = ctx['wnd']
        label = '(e) %s%s, run %d' % ('slot widget' if via_slot else 'ship centre',
                                      ', hover' if hover else ', no hover', rep)
        const = builtin('const')
        shipid = getattr_safe(builtin('session'), 'shipid')
        hangar = get_svc('invCache').GetInventory(getattr_safe(const, 'containerHangar', 10004))
        flag_hangar = getattr_safe(const, 'flagHangar', 4)
        mark = log_mark()
        clear_seen()
        gone, ctl_gone = unfit_everything(controller)
        precondition('%s: the ship and the window start empty' % label, gone and ctl_gone,
                     'ship empty %s, slots empty %s' % (text(gone), text(ctl_gone)))
        if not (gone and ctl_gone):
            return
        # 1. fit one medium module the way (a) does (a split from the stack)
        rows = hangar_rows(med)
        stacks = [r for r in rows if not getattr_safe(r, 'singleton')]
        src = stacks[0] if stacks else (rows[0] if rows else None)
        precondition('%s: a hangar module to fit' % label, src is not None,
                     '%d row(s), %d stack(s)' % (len(rows), len(stacks)))
        if src is None:
            return
        safe(lambda: controller.GetSlotController(med0).OnDropData(None, [DragNode(src)]))

        def fitted_at_flag():
            r = fitted_modules().get(med0)
            return r if (r is not None and getattr_safe(r, 'typeID') == med) else None
        first, _w = wait_for(fitted_at_flag, timeout_ms=15000)
        check('%s: step 1, the first fit reached the ship' % label, first is not None,
              'ship at flag %s: %s' % (text(med0), text(getattr_safe(first, 'itemID'))))
        if first is None:
            return
        item_id = first.itemID
        # 2. unfit it to the hangar: it comes back as a singleton
        safe(lambda: hangar.Add(item_id, shipid, flag=flag_hangar))

        def in_hangar():
            for r in hangar_rows(med):
                if getattr_safe(r, 'itemID') == item_id:
                    return r
            return None
        row, _w = wait_for(lambda: in_hangar() if med0 not in fitted_modules() else None,
                           timeout_ms=15000)
        precondition('%s: the unfitted module is in the hangar and off the ship' % label,
                     row is not None, 'item %s' % text(item_id))
        if row is None:
            return
        diag('%s: the hangar row' % label, 'singleton=%s stacksize=%s' % (
            text(getattr_safe(row, 'singleton')), text(getattr_safe(row, 'stacksize'))))
        emp, _w = wait_for(lambda: widgets_empty(wnd, [med0]), timeout_ms=REDRAW_MS)
        check('%s: step 2, after the unfit the slot widget shows the empty hint' % label,
              bool(emp), 'widget (hint, controller type, destroyed): %s' % widget_text(wnd, [med0]))
        # 3. the real drag: hover the inventory entry, take its drag data, drop it
        if ctx['inv_wnd'] is None or getattr_safe(ctx['inv_wnd'], 'destroyed'):
            ctx['inv_wnd'], why = hangar_window()
            precondition('the hangar inventory window opened', ctx['inv_wnd'] is not None, why)
            if ctx['inv_wnd'] is None:
                return
            sleep_ms(2000)
        entry, _w = wait_for(lambda: find_entry(ctx['inv_wnd'], item_id), timeout_ms=10000)
        precondition('%s: the hangar window has an entry for the module' % label,
                     entry is not None, 'item %s' % text(item_id))
        if entry is None:
            return
        remove_taps, tap_note = install_call_taps()
        try:
            clear_seen()
            diag('%s: call taps' % label, tap_note)
            if hover:
                safe(entry.OnMouseEnter)
                sleep_ms(500)
                diag('%s: preview item after the hover' % label, text(safe(
                    lambda: getattr_safe(controller.GetPreviewFittedItem(), 'itemID'))[1]))
            ok, nodes = safe(entry.GetDragData)
            precondition('%s: the entry gave drag data' % label, ok and bool(nodes), text(nodes)[:200])
            if not (ok and nodes):
                return
            target = slot_widgets(wnd).get(med0) if via_slot else find_child(wnd, 'ShipSceneParent')
            precondition('%s: the drop target exists' % label, target is not None,
                         'slot widget' if via_slot else 'ShipSceneParent')
            if target is None:
                return
            diag('%s: nodes dragged' % label, text([(getattr_safe(n, '__guid__'),
                 getattr_safe(getattr_safe(n, 'rec'), 'itemID')) for n in nodes]))
            ok, v = safe(lambda: target.OnDropData(None, nodes))
            check('%s: the drop call itself did not raise' % label, ok, text(v)[:300])
            back, w_back = wait_for(fitted_at_flag, timeout_ms=15000)
            check('%s: inventory: the singleton is on the ship at flag %s' % (label, text(med0)),
                  back is not None and getattr_safe(back, 'itemID') == item_id,
                  'ship at that flag: %s after %s ms' % (
                      text(getattr_safe(back, 'itemID')), text(w_back)))
            ctl_ok, w_ctl = wait_for(lambda: controller.GetSlotController(med0).GetModuleID()
                                     == item_id or None, timeout_ms=REDRAW_MS)
            check('%s: controller: the slot controller holds the module' % label, bool(ctl_ok),
                  'GetModuleID %s after %s ms' % (
                      text(safe(lambda: controller.GetSlotController(med0).GetModuleID())[1]),
                      text(w_ctl)))
            shown, w_sh = wait_for(lambda: widgets_show(wnd, {med0: med}), timeout_ms=REDRAW_MS)
            check('%s: widget: the slot widget shows the module within %d ms' % (label, REDRAW_MS),
                  bool(shown), 'widget (hint, controller type, destroyed): %s, expected %s'
                  % (widget_text(wnd, [med0]), text(type_name(med))))
            diag('%s: server call that went out' % label,
                 ' || '.join(seen['calls']) or 'nothing seen by the taps (a drop that sent '
                 'no Add / MultiAdd, or the tapped objects are not the ones used)')
            diag('%s: change dicts received by invCache (%d)' % (label, len(seen['item_change'])),
                 ' || '.join(seen['item_change'][:8]))
            diag('%s: controller state' % label, 'IsSimulated=%s GetItemID=%s session.shipid=%s' % (
                text(safe(lambda: controller.IsSimulated())[1]),
                text(safe(lambda: controller.GetItemID())[1]), text(shipid)))
            diag('%s: exceptions in the redraw path' % label,
                 ' || '.join(seen['dogma_raise'] + seen['inv_raise']) or 'none')
        finally:
            remove_taps()
        # 4. unfit again: the widget must go back to empty
        safe(lambda: hangar.Add(item_id, shipid, flag=flag_hangar))
        wait_for(lambda: med0 not in fitted_modules() or None, timeout_ms=15000)
        c_emp, _w = wait_for(lambda: controller.GetSlotController(med0).GetModuleID() is None
                             or None, timeout_ms=REDRAW_MS)
        check('%s: step 4, after the second unfit the slot controller is empty' % label,
              bool(c_emp), 'GetModuleID %s' % text(
                  safe(lambda: controller.GetSlotController(med0).GetModuleID())[1]))
        w_emp, _w = wait_for(lambda: widgets_empty(wnd, [med0]), timeout_ms=REDRAW_MS)
        check('%s: step 4, after the second unfit the slot widget shows the empty hint' % label,
              bool(w_emp), 'widget (hint, controller type, destroyed): %s' % widget_text(wnd, [med0]))
        blob, detail = log_since(mark)
        if blob is not None:
            n, heads = log_exceptions(blob)
            diag('%s: client log exceptions since the start (%d)' % (label, n), ' ## '.join(heads))

    # -- (f) the owner's large-hull sequence, a tap at each stage (issue #13) --
    # Names are [A]; the first one the client knows is used (a miss is a PRECONDITION).
    LARGE_HULLS = ['Vargur', 'Rorqual', 'Raven', 'Megathron']
    LARGE_MED = ['Multispectrum Shield Hardener II', 'Adaptive Invulnerability Field I']
    LARGE_TURRETS = ['1400mm Howitzer Artillery I', '425mm Railgun I', 'Mega Pulse Laser I']

    STAGE_NAMES = ('S1', 'S2', 'S3', 'S4', 'S5')
    stages = dict((k, []) for k in STAGE_NAMES)
    stage_tapped = {}

    stage_t0 = [0]

    def clear_stages():
        for k in stages:
            del stages[k][:]
        stage_t0[0] = now_ms() or 0

    def put(stage, key, msg):
        stages[stage].append((now_ms() or 0, key, text(msg)))

    def install_stage_taps(controller, wnd):
        """One tap per stage of the redraw chain, as instance wraps (removed by
        the returned callable).  S1 `invCache.OnItemChange` (the push arrived),
        S2 `clientDogmaIM.GodmaItemChanged` (the ship gate), S3 the dogma
        location's `FitItem` (entry, exit, raised), S4 the controller's
        `OnDogmaItemChange` and `_UpdateSlots`, the widget `hint` is read by the caller (clientDogmaLocation.py:533,
        fittingController.py:333, slot.py:226).  A tap that could not be installed is named in
        `stage_tapped`, and the checks that need it are skipped, not passed."""
        undo = []
        stage_tapped.clear()

        def tap(label, obj, name, enter, leave):
            try:
                orig = getattr(obj, name)

                def wrapper(*a, **kw):
                    try:
                        enter(a, kw)
                    except Exception:
                        pass
                    raised = None
                    try:
                        return orig(*a, **kw)
                    except Exception:
                        raised = ' | '.join(traceback.format_exc().strip().split('\n')[-3:])
                        raise
                    finally:
                        try:
                            leave(a, kw, raised)
                        except Exception:
                            pass
                setattr(obj, name, wrapper)
                undo.append((obj, name))
                stage_tapped[label] = True
            except Exception as e:
                stage_tapped[label] = False
                say('stage tap %s not installed: %s' % (label, text(e)[:120]))

        def nothing(a, kw, raised):
            return None

        def iid(a):
            return getattr_safe(a[0], 'itemID')

        inv = get_svc('invCache')
        dim = get_svc('clientDogmaIM')
        loc = dogma_loc()
        if inv is not None:
            tap('S1', inv, 'OnItemChange', lambda a, kw: put(
                'S1', iid(a), 'item %s type %s loc %s flag %s change %s' % (
                    text(iid(a)), text(getattr_safe(a[0], 'typeID')),
                    text(getattr_safe(a[0], 'locationID')), text(getattr_safe(a[0], 'flagID')),
                    change_text(a[1]))), nothing)
        else:
            stage_tapped['S1'] = False
        if dim is not None:
            tap('S2', dim, 'GodmaItemChanged', lambda a, kw: put(
                'S2', iid(a), 'item %s loc %s, GetCurrentShipID %s' % (
                    text(iid(a)), text(getattr_safe(a[0], 'locationID')),
                    text(safe(lambda: dim.dogmaLocation.GetCurrentShipID())[1]))), nothing)
        else:
            stage_tapped['S2'] = False
        if loc is not None:
            tap('S3', loc, 'FitItem', lambda a, kw: put(
                'S3', iid(a), 'FitItem enter item %s' % text(iid(a))),
                lambda a, kw, raised: put('S3', iid(a), 'FitItem exit item %s raised %s' % (
                    text(iid(a)), text(raised))))
            # inside S3: the dogma location's own OnItemChange (clientDogmaLocation.py:550-595),
            # with the flag that gates the scatter (:593) and the online step (:597)
            tap('S3a', loc, 'OnItemChange', lambda a, kw: put(
                'S3', iid(a), 'loc.OnItemChange enter scatterAttributeChanges=%s' % text(
                    getattr_safe(loc, 'scatterAttributeChanges'))),
                lambda a, kw, raised: put('S3', iid(a), 'loc.OnItemChange exit raised %s scatter=%s' % (
                    text(raised), text(getattr_safe(loc, 'scatterAttributeChanges')))))
            tap('S3b', loc, '_OnlineModuleIfApplicable', lambda a, kw: put(
                'S3', iid(a), '_OnlineModuleIfApplicable enter'),
                lambda a, kw, raised: put('S3', iid(a), '_OnlineModuleIfApplicable exit raised %s' % text(raised)))
        else:
            stage_tapped['S3'] = False
        tap('S4', controller, 'OnDogmaItemChange', lambda a, kw: put(
            'S4', iid(a), 'OnDogmaItemChange item %s change %s' % (text(iid(a)), change_text(a[1]))),
            lambda a, kw, raised: put('S4', iid(a), 'OnDogmaItemChange exit item %s raised %s' % (
                text(iid(a)), text(raised))))
        tap('S4u', controller, '_UpdateSlots', lambda a, kw: put('S4', None, '_UpdateSlots'),
            nothing)
        tap('S4a', controller, 'OnAttributes', lambda a, kw: put('S4', None, 'OnAttributes'),
            nothing)
        # S5 cannot be tapped: the widget's `UpdateFitting` is connected to
        # `controller.on_item_fitted` as a bound method (baseSlot.py:61), so an
        # instance wrap is never called [V: run 1, 0 records].  S5 is read from
        # the widget's own `hint` instead (the check in `drop_watch`).
        n_widgets = 0
        say('stage taps: %s, %d slot widgets tapped' % (text(stage_tapped), n_widgets))

        def remove():
            for obj, name in undo:
                try:
                    delattr(obj, name)
                except Exception:
                    pass
        return remove

    def stage_reached(item_id):
        """`{stage: bool}`: how far the push for `item_id` got.  A stage counts
        only if an earlier stage's record for the same item precedes it."""
        def has(stage, needle=None):
            return [r for r in stages[stage] if r[1] == item_id
                    and (needle is None or needle in r[2])]
        s1 = has('S1')
        s2 = has('S2')
        s3 = [r for r in has('S3', 'FitItem exit') if 'raised None' in r[2]]
        s4 = has('S4', 'OnDogmaItemChange item')
        t4 = min([r[0] for r in s4]) if s4 else None
        upd = [r for r in stages['S4'] if r[1] is None and r[2] == '_UpdateSlots'
               and t4 is not None and r[0] >= t4]
        return {'S1': bool(s1), 'S2': bool(s2), 'S3': bool(s3),
                'S4': bool(s4) and bool(upd)}

    def widget_hints(wnd):
        return dict((f, text(getattr_safe(w, 'hint'))) for f, w in slot_widgets(wnd).items())

    def fitted_ids():
        return dict((f, getattr_safe(r, 'itemID')) for f, r in fitted_modules().items())

    def drag_nodes(item_id, entry_wnd):
        """The nodes the hangar window's entry drags, as a real drag takes
        them (hover, `GetDragData`), else a `DragNode` and a diagnostic."""
        row = None
        for r in (hangar_items()[1] or []):
            if getattr_safe(r, 'itemID') == item_id:
                row = r
        entry = find_entry(entry_wnd, item_id) if entry_wnd is not None else None
        if entry is not None:
            safe(entry.OnMouseEnter)
            sleep_ms(300)
            ok, nodes = safe(entry.GetDragData)
            if ok and nodes:
                return nodes
        diag('drag nodes', 'no hangar entry for item %s: a DragNode on its row is used' % text(item_id))
        return [DragNode(row)] if row is not None else None

    def drop_watch(label, wnd, controller, drop, refused=False):
        """One drop, then each stage.  Fitted drops: the ship takes the module,
        the slot widget shows it within REDRAW_MS, and the stage taps say how
        far the push got.  Refused drops (the caller proved the rack full):
        nothing is fitted and no widget changes."""
        import uthread
        before = fitted_ids()
        hints_before = widget_hints(wnd)
        clear_stages()
        mark = log_mark()
        if refused:
            ok, v = safe(lambda: uthread.new(drop))
        else:
            ok, v = safe(drop)
        check('%s: the drop call itself did not raise' % label, ok, text(v)[:300])

        def changed():
            now = fitted_ids()
            new = dict((f, i) for f, i in now.items() if before.get(f) != i)
            return new or None
        if refused:
            sleep_ms(4000)
            new = changed()
            check('%s: the refused drop fitted nothing' % label, not new,
                  'fitted after: %s' % text(new))
            sleep_ms(2000)
            check('%s: every slot widget is as it was' % label,
                  widget_hints(wnd) == hints_before,
                  'changed: %s' % text(dict((f, h) for f, h in widget_hints(wnd).items()
                                            if hints_before.get(f) != h)))
            diag('%s: push stages seen by a refused drop' % label,
                 'S1 %d, S4 %d (nothing is expected)' % (len(stages['S1']), len(stages['S4'])))
            return {}
        new, w_new = wait_for(changed, timeout_ms=15000)
        check('%s: the ship took a module (client inventory)' % label, bool(new),
              'new at flags %s after %s ms' % (text(new), text(w_new)))
        if not new:
            return {}
        types = dict((f, getattr_safe(r, 'typeID')) for f, r in fitted_modules().items()
                     if f in new)
        shown, w_sh = wait_for(lambda: widgets_show(wnd, types), timeout_ms=REDRAW_MS)
        check('%s: S5 the slot widgets show the module within %d ms' % (label, REDRAW_MS),
              bool(shown), 'widgets (hint, controller type, destroyed) %s, expected %s (after %s ms)'
              % (widget_text(wnd, sorted(types)),
                 text(dict((f, type_name(t)) for f, t in types.items())), text(w_sh)))
        if not shown:
            late, w_late = wait_for(lambda: widgets_show(wnd, types), timeout_ms=15000)
            diag('%s: the widgets caught up later' % label,
                 'yes, %s ms after the first wait' % text(w_late) if late else 'not within 15 s more')
            diag('%s: S4 records after the late wait' % label, ' || '.join(
                '+%d %s' % (r[0] - stage_t0[0], r[2]) for r in stages['S4'][:14]))
        sleep_ms(500)
        for f, i in sorted(new.items()):
            reach = stage_reached(i)
            reach['S5'] = bool(shown)
            diag('%s: stages reached by item %s at flag %s' % (label, text(i), text(f)),
                 ' '.join('%s=%s' % (k, 'yes' if reach[k] else 'NO') for k in STAGE_NAMES))
            for k, what in (('S1', 'invCache.OnItemChange got the push'),
                            ('S2', 'clientDogmaIM.GodmaItemChanged passed it on'),
                            ('S3', 'dogma FitItem ran without raising')):
                if stage_tapped.get(k):
                    check('%s: %s %s' % (label, k, what), reach[k],
                          'item %s; first records: %s' % (text(i), text(
                              [r[2] for r in stages[k] if r[1] in (i, None)][:3])))
        # S4 is a DIAGNOSTIC, not a check: a drop that lands while `_MakeShipActive`
        # runs never scatters OnDogmaItemChange (clientDogmaLocation.py:593), and
        # the slots are then redrawn by OnAttributes -> _UpdateSlots (the server's
        # plural OnModuleAttributeChanges).  S5, the widget, is the check.
        diag('%s: _UpdateSlots / OnAttributes seen after the drop' % label,
             'OnAttributes %d, _UpdateSlots %d' % (
                 len([r for r in stages['S4'] if r[2] == 'OnAttributes']),
                 len([r for r in stages['S4'] if r[2] == '_UpdateSlots'])))
        for k in STAGE_NAMES[:4]:
            diag('%s: %s records (%d), ms after the drop' % (label, k, len(stages[k])),
                 ' || '.join('+%d %s' % (r[0] - stage_t0[0], r[2]) for r in stages[k][:14]))
        blob, detail = log_since(mark)
        if blob is not None:
            diag('%s: client log lines on fitting' % label, ' || '.join(
                log_lines_with(blob, 'Fitting item as a result from item change', 3)
                + log_lines_with(blob, 'SlotAlreadyOccupied', 2)))
            n, heads = log_exceptions(blob, ignore=('SlotAlreadyOccupied', 'NoSlot', 'NoFreeSlots'))
            diag('%s: client log exceptions (%d)' % (label, n), ' ## '.join(heads))
        return new

    def unfit_watch(label, wnd, flags):
        """Unfit the modules at `flags` to the hangar; the widgets go empty."""
        const = builtin('const')
        hangar = get_svc('invCache').GetInventory(getattr_safe(const, 'containerHangar', 10004))
        shipid = getattr_safe(builtin('session'), 'shipid')
        ids = fitted_ids()
        for f in flags:
            if f in ids:
                safe(lambda f=f: hangar.Add(ids[f], shipid,
                                            flag=getattr_safe(const, 'flagHangar', 4)))
        gone, _w = wait_for(lambda: not [f for f in flags if f in fitted_modules()] or None,
                            timeout_ms=15000)
        precondition('%s: the modules are off the ship' % label, bool(gone), text(flags))
        emp, w_e = wait_for(lambda: widgets_empty(wnd, list(flags)), timeout_ms=REDRAW_MS)
        check('%s: S5 after the unfit the slot widgets show the empty hint within %d ms'
              % (label, REDRAW_MS), bool(emp),
              'widgets (hint, controller type, destroyed) %s (after %s ms)'
              % (widget_text(wnd, list(flags)), text(w_e)))
        return bool(gone)

    def to_large_hull(hull):
        """Board `hull` with the window open; `(window, controller)` or None."""
        ok, detail = board_ship(hull)
        check('(f) boarded the large hull', ok, detail)
        if not ok:
            return None
        return reopen_window(False)

    def reopen_window(close_first):
        wnd = ctx['wnd']
        if close_first and wnd is not None and not getattr_safe(wnd, 'destroyed'):
            safe(wnd.Close)
            wait_for(lambda: getattr_safe(wnd, 'destroyed') or None, timeout_ms=5000)
        wnd, why = open_window()
        check('(f) the fitting window is open', wnd is not None, why)
        if wnd is None:
            return None
        ready, w = wait_for(lambda: window_ready(wnd) or None, timeout_ms=20000)
        shipid = getattr_safe(builtin('session'), 'shipid')
        ctl = getattr_safe(wnd, 'controller')
        same, _w = wait_for(lambda: safe(lambda: ctl.GetItemID())[1] == shipid or None,
                            timeout_ms=10000)
        precondition('(f) the window has a controller on the boarded ship', bool(ready) and bool(same),
                     'ready %s after %s ms, controller %s, session %s' % (
                         text(bool(ready)), text(w), text(safe(lambda: ctl.GetItemID())[1]),
                         text(shipid)))
        if not (ready and same):
            return None
        if close_first:
            sleep_ms(1500)
        # (after a board there is no pause: a player drops as soon as the ship is theirs)
        ctx['wnd'] = wnd
        return wnd, ctl

    def fill_rack(label, wnd, controller, type_id, first_flag, maxn=10):
        """Drop `type_id` on the ship centre one at a time until the rack
        (8 flags from `first_flag`) stops taking them.  Returns the flags held."""
        flags = range(first_flag, first_flag + 8)
        held = lambda: [f for f in flags if f in fitted_modules()]
        stopped = False
        for n in range(maxn):
            rows = hangar_rows(type_id)
            if not rows:
                break
            before = len(held())

            def go(r=rows[0]):
                centre = find_child(wnd, 'ShipSceneParent')
                (centre or controller).OnDropData(None, [DragNode(r)])
            safe(go)
            grew, _w = wait_for(lambda: len(held()) > before or None, timeout_ms=8000)
            if not grew:
                stopped = True
                break
        sleep_ms(1000)
        shown = widgets_show(wnd, dict((f, type_id) for f in held())) if held() else None
        check('%s: the slot widgets show all %d fitted modules' % (label, len(held())),
              bool(shown), 'widgets %s' % widget_text(wnd, held()))
        return held(), stopped

    def case_f(hi0, med0):
        hull, hull_name = first_known(LARGE_HULLS)
        hard, hard_name = first_known(LARGE_MED)
        gun, gun_name = first_known(LARGE_TURRETS)
        precondition('(f) the client knows a large hull, a med module and a large turret',
                     None not in (hull, hard, gun), text((hull_name, hard_name, gun_name)))
        if None in (hull, hard, gun):
            return
        for t in (hull, hard, gun):
            grant_skills_for(t)
        gm_check('GM: one large hull', '/createitem %d 1' % hull)
        gm_check('GM: modules for the large hull', '/createitem %d 12' % hard)
        gm_check('GM: large turrets', '/createitem %d 24' % gun)
        for t in (hull, hard, gun):
            find_hangar_item(t)
        if ctx['wnd'] is not None:
            unfit_everything(ctx['wnd'].controller)
        got = to_large_hull(hull)
        if got is None:
            return
        wnd, controller = got
        slots_ok = [f for f in (hi0, med0) if f in getattr_safe(controller, 'slotsByFlagID', {})]
        precondition('(f) the large hull has a high and a medium slot controller',
                     len(slots_ok) == 2, text(slots_ok))
        if len(slots_ok) != 2:
            return
        if ctx['inv_wnd'] is None or getattr_safe(ctx['inv_wnd'], 'destroyed'):
            ctx['inv_wnd'], why = hangar_window()
            precondition('(f) the hangar inventory window opened', ctx['inv_wnd'] is not None, why)
            if ctx['inv_wnd'] is None:
                return
            sleep_ms(2000)
        const = builtin('const')
        hangar = get_svc('invCache').GetInventory(getattr_safe(const, 'containerHangar', 10004))
        shipid = getattr_safe(builtin('session'), 'shipid')
        diag('(f) hull', '%s type %s ship %s, high slots with a controller %s' % (
            text(hull_name), text(hull), text(shipid), text(sorted(
                f for f in getattr_safe(controller, 'slotsByFlagID', {}) if hi0 <= f < hi0 + 8))))
        rows = hangar_rows(hard)
        remove = install_stage_taps(controller, wnd)
        try:
            # the owner's sequence, three times; the window is re-opened for runs 2 and 3
            for rep in (1, 2, 3):
                label = '(f) %s run %d' % (hull_name, rep)
                if rep > 1:
                    remove()
                    got = reopen_window(True)
                    if got is None:
                        return
                    wnd, controller = got
                    remove = install_stage_taps(controller, wnd)
                unfit_everything(controller)
                # 1. fit one by the slot, unfit it: a hangar singleton
                src = [r for r in hangar_rows(hard)][0:1]
                precondition('%s: a hangar module to fit' % label, bool(src), '')
                if not src:
                    return
                new = drop_watch('%s step 1 slot drop of a stack unit' % label, wnd, controller,
                                 lambda: controller.GetSlotController(med0).OnDropData(
                                     None, [DragNode(src[0])]))
                if med0 not in new:
                    return
                item_id = new[med0]
                if not unfit_watch('%s step 1 unfit' % label, wnd, [med0]):
                    return
                # 2. the singleton, real drag nodes, on the ship centre
                nodes = drag_nodes(item_id, ctx['inv_wnd'])
                precondition('%s: drag nodes for the singleton' % label, bool(nodes), '')
                if not nodes:
                    return
                new = drop_watch('%s step 2 centre drop of the singleton' % label, wnd, controller,
                                 lambda: find_child(wnd, 'ShipSceneParent').OnDropData(None, nodes))
                unfit_watch('%s step 2 unfit' % label, wnd, sorted(new.keys()))
                # 3. the same on the slot widget
                nodes = drag_nodes(item_id, ctx['inv_wnd'])
                if not nodes:
                    return
                new = drop_watch('%s step 3 slot-widget drop of the singleton' % label, wnd,
                                 controller,
                                 lambda: slot_widgets(wnd)[med0].OnDropData(None, nodes))
                unfit_watch('%s step 3 unfit' % label, wnd, sorted(new.keys()))
                # 4. a large turret on the ship centre (the hardpoint branch)
                grows = hangar_rows(gun)
                if not grows:
                    return
                new = drop_watch('%s step 4 turret on the ship centre' % label, wnd, controller,
                                 lambda: find_child(wnd, 'ShipSceneParent').OnDropData(
                                     None, [DragNode(grows[0])]))
                unfit_watch('%s step 4 unfit' % label, wnd, sorted(new.keys()))
            # 5. a full high rack: one more turret on the centre
            unfit_everything(controller)
            held, stopped = fill_rack('(f) high rack', wnd, controller, gun, hi0)
            full = stopped
            precondition('(f) the high rack stopped taking turrets (full, or out of hardpoints)',
                         bool(held) and full,
                         'held flags %s, hull has %d high slots' % (text(held), 8))
            if held and full and hangar_rows(gun):
                r = hangar_rows(gun)[0]
                drop_watch('(f) full high rack, turret on the centre', wnd, controller,
                           lambda: find_child(wnd, 'ShipSceneParent').OnDropData(None, [DragNode(r)]),
                           refused=True)
            # 6. a full med rack: one more on an occupied slot
            unfit_everything(controller)
            held, stopped = fill_rack('(f) med rack', wnd, controller, hard, med0)
            full = stopped
            precondition('(f) the med rack stopped taking modules', bool(held) and full,
                         'held flags %s' % text(held))
            if held and full and hangar_rows(hard):
                r = hangar_rows(hard)[0]
                drop_watch('(f) full med rack, module on an occupied slot', wnd, controller,
                           lambda: slot_widgets(wnd)[med0].OnDropData(None, [DragNode(r)]),
                           refused=True)
        finally:
            remove()
        unfit_everything(controller)

    def eoci_body():
        mark = log_mark()
        hi0 = const_of('flagHiSlot0', 27)
        med0 = const_of('flagMedSlot0', 19)
        lo0 = const_of('flagLoSlot0', 11)

        hull, hull_name = first_known(HULLS)
        gun, gun_name = first_known(HI)
        med, med_name = first_known(MED)
        low, low_name = first_known(LOW)
        precondition('the client knows a frigate hull, a gun, a medium and a low module',
                     None not in (hull, gun, med, low),
                     text((hull_name, gun_name, med_name, low_name)))
        if None in (hull, gun, med, low):
            return
        for t in (gun, med, low):
            grant_skills_for(t)
        # modules: 1 gun and 1 med for (a), 3 of each kind for (b), a 3-stack of
        # the low for (c), one of each for (d).  /createitem makes stacks.
        gm_check('GM: one hull', '/createitem %d 1' % hull)
        gm_check('GM: six guns', '/createitem %d 6' % gun)
        gm_check('GM: twelve medium modules', '/createitem %d 12' % med)
        gm_check('GM: six low modules', '/createitem %d 6' % low)
        found, why = find_hangar_item(hull)
        precondition('the hull is in the hangar', found is not None, why)
        if found is None:
            return
        for t in (gun, med, low):
            find_hangar_item(t)
        ok, detail = board_ship(hull)
        check('boarded the hull', ok, detail)
        if not ok:
            return
        sleep_ms(3000)

        wnd, why = open_window()
        check('the fitting window opened', wnd is not None, why)
        if wnd is None:
            return
        ready, w = wait_for(lambda: window_ready(wnd) or None, timeout_ms=20000)
        precondition('the window has a controller with slot controllers', bool(ready),
                     'after %s ms' % text(w))
        if not ready:
            return
        controller = wnd.controller
        simulated = bool(safe(lambda: controller.IsSimulated())[1])
        precondition('the window is on the real ship, not the simulation (ghost) mode',
                     not simulated, 'IsSimulated=%s' % text(simulated))
        if simulated:
            return
        precondition('the window follows the boarded ship',
                     getattr_safe(controller, 'GetItemID', lambda: None)()
                     == getattr_safe(builtin('session'), 'shipid'),
                     'controller %s, session %s' % (
                         text(safe(lambda: controller.GetItemID())[1]),
                         text(getattr_safe(builtin('session'), 'shipid'))))
        slots_ok = [f for f in (hi0, med0, lo0)
                    if f in getattr_safe(controller, 'slotsByFlagID', {})]
        precondition('the window has a high, a medium and a low slot controller',
                     len(slots_ok) == 3, text(slots_ok))
        if len(slots_ok) != 3:
            return
        ctx['wnd'] = wnd
        widgets = slot_widgets(wnd)
        precondition('the window has a slot widget (with hint and _emptyHint) per slot',
                     all(f in widgets and getattr_safe(widgets[f], '_emptyHint') is not None
                         and getattr_safe(widgets[f], 'hint') is not None
                         for f in (hi0, med0, lo0)),
                     'FittingCont.slots flags %s; widget classes %s' % (
                         text(sorted(widgets.keys())),
                         text(dict((f, widgets[f].__class__.__name__)
                                   for f in (hi0, med0, lo0) if f in widgets))))
        if not all(f in widgets and getattr_safe(widgets[f], '_emptyHint') is not None
                   and getattr_safe(widgets[f], 'hint') is not None for f in (hi0, med0, lo0)):
            return
        remove = install_taps(controller)
        try:
            slot = controller.GetSlotController

            def one(type_id):
                rows = hangar_rows(type_id)
                return rows[0] if rows else None

            # (a) one hangar module on one empty slot
            def drop_a():
                r = one(gun)
                slot(hi0).OnDropData(None, [DragNode(r)])
            run_case('(a) one module on one slot', controller, 1, (gun,), drop_a)

            # (b) three modules on three slots at once, one uthread each
            def drop_b():
                import uthread
                for f, t in ((hi0, gun), (med0, med), (lo0, low)):
                    uthread.new(slot(f).OnDropData, None, [DragNode(one(t))])
            run_case('(b) three modules on three slots', controller, 3, (gun, med, low), drop_b)

            # (c) a stack of identical modules on ONE slot: N drops, same flag
            def drop_c():
                stacks = [r for r in hangar_rows(low) if not getattr_safe(r, 'singleton')]
                rows = hangar_rows(low)
                src = stacks[0] if stacks else rows[0]
                nodes = [DragNode(src) for _i in range(3)]
                diag('(c) what the drop carries',
                     '%d hangar row(s) of the type, stack row %s has stacksize %s; '
                     'three nodes of that row on flag %s' % (
                         len(rows), text(getattr_safe(src, 'itemID')),
                         text(getattr_safe(src, 'stacksize')), text(lo0)))
                slot(lo0).OnDropData(None, nodes)
            run_case('(c) a stack of three on one slot', controller, 1, (low,), drop_c)

            # (d) the ship centre: ShipSceneParent.OnDropData -> controller.OnDropData
            #     -> TryFitter.FitModule -> MultiAdd(flag=flagAutoFit)
            def drop_d():
                centre = find_child(wnd, 'ShipSceneParent')
                nodes = [DragNode(one(t)) for t in (gun, med, low)]
                if centre is not None:
                    diag('(d) the drop target', 'ShipSceneParent found; its OnDropData')
                    centre.OnDropData(None, nodes)
                else:
                    diag('(d) the drop target', 'ShipSceneParent NOT found in the window '
                         'tree; controller.OnDropData called directly')
                    controller.OnDropData(None, nodes)
            run_case('(d) three modules on the ship centre', controller, 3, (gun, med, low),
                     drop_d)

            # (e) the owner's sequence, real drag nodes from the hangar window
            for via_slot, hover in ((False, True), (True, True), (False, False)):
                for rep in (1, 2):
                    case_e(controller, med, med0, via_slot, hover, rep)
        finally:
            remove()
        unfit_everything(controller)
        case_f(hi0, med0)
        # (c) refuses the 2nd and 3rd Add on purpose (SlotAlreadyOccupied): not a fault
        no_new_tracebacks('fit drag cases', mark,
                          ignore=('SlotAlreadyOccupied', 'NoSlot', 'NoFreeSlots'))


    # -- end of the scenario body ------------------------------------------
    # PRELUDE (part 2) -- identical in every scenario.  Always writes the
    # verdict, even on catastrophe: a missing file and a failing check are
    # different diagnoses and the harness tells them apart.

    error = None
    try:
        eoci_body()
    except Exception:
        error = traceback.format_exc()
    finally:
        if not checks and error is None:
            check('scenario recorded at least one check', False,
                  'the body returned without asserting anything')
        try:
            write_result(error)
        except Exception:
            say('write_result itself raised:\n%s' % traceback.format_exc())
        quit_client('eve-offline client-ci: %s finished' % EOCI_NAME)


eoci_main(EOCI_SCENARIO)
