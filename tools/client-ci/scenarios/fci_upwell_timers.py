# -*- coding: utf-8 -*-
"""Structure timers the client shows: the anchoring timer, onlining, the upkeep state, the unanchor timer (issue #40), and the reinforce timers (issue #47).

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Packet F-CI (`docs/lanes/f-ci.md`).

Lane upwell-timers.  One client, no combat.  `/world anchortime 60` first, so
the deploy timer is a minute and not an hour.  Founds a corporation and
alliance (`ensure_alliance`), undocks, `/upwell give astrahus`, and calls
`structureDeployment.Anchor` with the arguments the client's own
`ConfirmDeployment` sends.  Every expected value comes from the client's OWN
`structures` package (`structures/const.py`), imported inside the client, not
copied here:

1. Deploy timer.  The structure's slim in the client's ballpark carries
   `state == STATE_ANCHORING`, a `timer` of `(start, end, paused)` with
   `paused` None, `end - start == anchortime` seconds (the client reads the
   duration as `(end - start) / const.SEC` in `spaceObject/structure.py`
   `_OnSlimItemUpdated`), `start` near the wallclock of the call, and `end`
   still ahead of the client's wallclock.  Read twice, `end` has not moved.
   The corporation tab (`GetMyCorporationStructures`, built into the client's
   own `StructureEntryController`) agrees: state, `GetTimerEnd() == end`, not
   unanchoring.
2. Upkeep.  The slim's `upkeepState` and the entry's both equal
   `structures.UPKEEP_STATE_FULL_POWER`; `IsLowPower()` and `IsAbandoned()`
   are false; `GetStructureInfo` says the same.
3. Onlining.  Within the bound, the slim's `state` is
   `STATE_ONLINING_VULNERABLE`, its `timer` is None (the client's
   `UpdateTimers` clears the countdown there), the entry's `timerEnd` is None,
   the upkeep is still full power, and the structure is dockable.
4. Unanchor.  `Unanchor` is accepted (`STATE_CANCELS_UNANCHOR` is False for
   the onlining state, read from the client); the entry's `unanchoring` is a
   time `structures.UNANCHORING_TIME` seconds ahead of the client's wallclock
   (the Astrahus is a medium, not a flex, hull), `IsUnanchoring()` is true
   and `GetUnanchorTime()` is that time, the slim's state is unchanged, and
   `CancelUnanchor` clears it.  A second `Unanchor` returns the same end (the
   client's `StartUnanchoring` is a no-op while one runs).

Steps 6-8 (issue #47, lane L4) run after step 4 in the same client, on a
second structure the GM command `/upwell spawn astrahus <name>` puts at the
pilot (online, shield-vulnerable, owned by the pilot's corporation).  A spy
on the client's `notificationSvc.OnNotificationReceived` records what the
server pushes.

6. Shield out.  `/upwell damage <id> 0 0.9 1` (the shield gone, the armour
   hurt) puts it in `STATE_ARMOR_REINFORCE`.  A notification of the client's
   `notificationTypeStructureLostShields` arrives for this structure; its
   `timestamp` equals the slim's `timer` end; `end - start` is at least the
   client's own `REINFORCE_DURATION_ARMOR_MEDIUM_BY_SECURITY[class]` less
   `REINFORCE_EXIT_JITTER_MEDIUM_SECONDS` (class read from the client's
   `map` service for the system, both constants from its `structures`
   package) and at most that plus seven days plus the jitter; the client's
   own formatter formats it.
7. `/upwell timer <id> 5` ends the reinforce timer in five seconds: within
   30 s the slim's state is `STATE_ARMOR_VULNERABLE`.
8. `structureVulnerability.UpdateReinforceTimesFromClient([(id, system,
   typeID)], structures.NO_REINFORCEMENT_WEEKDAY, 20)` (the weekday the
   client's own window always sends) returns; the corporation tab's row has
   `next_reinforce_weekday` 255, `next_reinforce_hour` 20 and a
   `next_reinforce_apply` in the future, the live `reinforce_*` still the
   old window; a `notificationTypeStructuresReinforcementChanged` arrives
   and the client's formatter formats it.

Step 5 (issue #48, the 184) runs last, in a null-sec system (an own-corporation
structure there is no crime): the pilot goes back to the station, boards a
GM-made Rifter with two autocannons, jumps to the first null-sec system of the
client's map, spawns a structure and `/upwell damage <id> 0.9501 1 1` (hurt, but
above the server's 95 % line, so nothing is said yet).  The guns do the rest:

5. Under attack.  A notification of the client's
   `notificationTypeStructureUnderAttack` arrives for this structure once the
   shield passes 95 %; its `charID` is the pilot's own (the last shooter, set
   by the volley), its `shieldPercentage` is under 95, `armorPercentage` and
   `hullPercentage` are 100, its `corpLinkData` is a three-item tuple, and the
   client's own formatter formats it.  (Fails without the 184: nothing
   arrives.  Fails with a shooter-less 184: `charID` is None.)

Steps 6-8 take the shield by GM command, not by guns, so there is no crime and
no null-sec move.  Low-power and abandoned upkeep
need service modules (U6); the expiry of an unanchor timer is a week.

No client log traceback may appear.

[A]s: `structureDeployment.Anchor` called as a remote service is the same call
`ConfirmDeployment` makes; the Astrahus is sized medium by the client (checked); the timer
tuple is in the wallclock's 100 ns units (the client subtracts
`blue.os.GetWallclockTime()` from `end` in the same file).  The harness runs
the server at real time: with `--time-scale` a start-near-wallclock check
fails and says so.  Falsifier for the server: any check below, or a traceback
in the client log.

[A]s for steps 6-8: a method assigned on the running `notificationSvc`
instance is the one `sm.ScatterEvent('OnNotificationReceived', ...)` calls
(falsifier: the spy never fires although the server logged the push; the
check then names the missing notification); the system's class from the
client's `map` service is the one the structure's length followed (the
server reads its own security table: a mismatch means the two disagree on
the system); and the client's formatter accepts the data as the server sends
it (a failure there names the formatter's exception).
"""

EOCI_SCENARIO = 'fci_upwell_timers'

# Seconds to pass to `tools/client-ci.sh run fci_upwell_timers --timeout`: login,
# the anchor minute with its bound, the unanchor calls, the reinforce steps, the
# Rifter, the jump to null sec and the guns of step 5, the quit.
EOCI_FCI_TIMEOUT_S = 2400

# SERVER-EXPECT: absent  UNANSWERED
# SERVER-EXPECT: absent  UNANSWERED structureVulnerability
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
            cache = os.path.join(os.path.dirname(PARAMS_FILE), 'typenames.tsv')
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
                gm('/giveskill %s %s' % (text(nm), text(lvl)))
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
                    gm('/giveskill %s %s' % (text(nm), text(lvl)))
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

    ANCHOR_SECS = 60
    TYPE_NAME = 'Astrahus'
    # Bound for the anchor minute to run out, ms.
    ONLINE_BOUND_MS = 200000

    def wall_ft():
        """The client's wallclock in 100 ns units, or None."""
        import blue
        for getter in ('GetWallclockTime', 'GetWallclockTimeNow'):
            try:
                return int(getattr(blue.os, getter)())
            except Exception:
                continue
        return None

    def sec_ticks():
        ok, v = safe(lambda: builtin('const').SEC)
        return int(v) if ok and v else None

    def slim_of(park, item_id):
        got = slims_where(park, lambda s: s.itemID == item_id)
        return got[0][1] if got else None

    def entry_of(item_id):
        ok, v = rcall('structureDirectory', 'GetMyCorporationStructures')
        return (v.get(item_id) if ok else None), (text(v)[:200] if not ok else '')

    def controller_of(entry):
        """The client's own entry controller over a directory row, or None."""
        ok, cls = safe(lambda: __import__(
            'eve.client.script.ui.structure.structureBrowser.controllers.structureEntryController',
            fromlist=['StructureEntryController']).StructureEntryController)
        if not ok or entry is None:
            return None
        ok, c = safe(lambda: cls(entry))
        return c if ok else None

    def timer_parts(slim):
        t = getattr_safe(slim, 'timer')
        if t is None or len(t) != 3:
            return None
        return t

    def timers_run():
        mark = log_mark()
        ok, st = safe(lambda: __import__('structures'))
        precondition("the client's structures package imports", ok, text(st)[:200])
        if not ok:
            return
        SEC = sec_ticks()
        precondition("the client's const.SEC reads", bool(SEC), text(SEC))
        if not SEC:
            return
        corp, alliance, why = ensure_alliance()
        precondition('a player corporation to own the structure', corp is not None, why)
        if corp is None:
            return
        gm_check('/world anchortime %d' % ANCHOR_SECS, '/world anchortime %d' % ANCHOR_SECS,
                 expect=[text(ANCHOR_SECS)])
        ok, v = undock()
        precondition('the pilot is in space', ok, text(v)[:200])
        if not ok:
            return
        # Away from every planet, belt, station and moon (see fci_upwell_anchor).
        gm_check('GM: out of the station\'s zone', '/tp 20000',
                 forbid=['pick between', 'not in space'])
        sleep_ms(5000)
        park, ball = get_ego_ball()
        precondition('the pilot has a ballpark', park is not None and ball is not None)
        if park is None or ball is None:
            return
        tids = type_ids_named(TYPE_NAME, exact=True)
        precondition('the client knows the %s type' % TYPE_NAME, bool(tids), text(tids))
        if not tids:
            return
        tid = tids[0]
        flex = safe(lambda: st.STRUCTURE_SIZE_BY_TYPE.get(tid))
        gm_check('/upwell give puts a deployable in the cargo', '/upwell give astrahus',
                 expect=['cargo'])

        def in_cargo():
            ok, rows = ship_items()
            if not ok:
                return None
            for r in rows:
                if getattr_safe(r, 'typeID') == tid:
                    return r
            return None
        row, _w = wait_for(in_cargo, timeout_ms=20000)
        precondition('the deployable shows in the ship inventory', row is not None)
        if row is None:
            return
        # A second invalidation while the hold tracker refetches lands on a
        # dropped container (client race, see fci_upwell_anchor).
        sleep_ms(3000)
        item_id = row.itemID
        name = 'Fci Timers ' + uniq(4)
        t_before = wall_ft()
        ok, v = rcall('structureDeployment', 'Anchor', item_id, 3000.0, 0.0, 0.5, None, name,
                      'f-ci', 3, 12, None)
        t_after = wall_ft()
        check('Anchor is accepted', ok, text(v)[:300])
        if not ok:
            return
        got, _w = wait_for(lambda: slim_of(park, item_id), timeout_ms=30000)
        check('the structure is a slim in the park', got is not None)
        if got is None:
            return

        # -- 1. the deploy timer -----------------------------------------------
        S = st
        check('the client sizes the Astrahus as medium, so the unanchor time is the long one',
              flex[0] and flex[1] == S.STRUCTURE_SIZE_MEDIUM, 'size %s' % text(flex[1]))
        slim = slim_of(park, item_id)
        state = getattr_safe(slim, 'state')
        check('the slim says STATE_ANCHORING', state == S.STATE_ANCHORING,
              'state %s want %s' % (text(state), text(S.STATE_ANCHORING)))
        check('the client counts anchoring as an offline, non-vulnerable state',
              state in S.OFFLINE_STATES and S.STATE_VULNERABILITY.get(state) is False,
              'state %s' % text(state))
        t1 = timer_parts(slim)
        check('the slim timer is (start, end, paused) with paused None',
              t1 is not None and t1[2] is None, text(getattr_safe(slim, 'timer')))
        if t1 is None:
            return
        start, end = t1[0], t1[1]
        check('the timer lasts the anchor time (end - start, in the client\'s SEC)',
              (end - start) == ANCHOR_SECS * SEC,
              'end-start %s s, want %s' % (fmt((end - start) / float(SEC), 1), ANCHOR_SECS))
        if t_before is not None and t_after is not None:
            slack = 30 * SEC
            check('the timer starts at the call (client wallclock +-30 s)',
                  t_before - slack <= start <= t_after + slack,
                  'start-before %s s, after-start %s s'
                  % (fmt((start - t_before) / float(SEC), 1), fmt((t_after - start) / float(SEC), 1)))
        now = wall_ft()
        remaining = (end - now) / float(SEC) if now is not None else None
        check('the end is ahead of the client\'s wallclock and within the anchor time',
              remaining is not None and 0 < remaining <= ANCHOR_SECS + 30,
              'remaining %s s' % fmt(remaining, 1))
        sleep_ms(8000)
        t2 = timer_parts(slim_of(park, item_id))
        if t2 is not None:
            check('a later read has the same end (the timer is not restarted)', t2[1] == end,
                  'end moved by %s s' % fmt((t2[1] - end) / float(SEC), 1))
        else:
            check('a later read still carries the timer', False,
                  text(getattr_safe(slim_of(park, item_id), 'state')))
        entry, why = entry_of(item_id)
        check('the corporation tab lists it', entry is not None, why)
        if entry is None:
            return
        ctl = controller_of(entry)
        precondition("the client's StructureEntryController builds over the row", ctl is not None)
        check('its row is anchoring with the same timer end and no unanchoring',
              entry.get('state') == S.STATE_ANCHORING and entry.get('timerEnd') == end
              and entry.get('unanchoring') is None,
              'state %s timerEnd-end %s unanchoring %s'
              % (text(entry.get('state')),
                 text((entry.get('timerEnd') or 0) - end), text(entry.get('unanchoring'))))
        if ctl is not None:
            ok, te = safe(ctl.GetTimerEnd)
            check('the controller reads GetTimerEnd and IsUnanchoring', ok and te == end
                  and ctl.IsUnanchoring() is False, text(te))

        # -- 2. the upkeep state ---------------------------------------------
        full = S.UPKEEP_STATE_FULL_POWER
        check('the slim upkeepState is the client\'s full power',
              getattr_safe(slim_of(park, item_id), 'upkeepState') == full,
              text(getattr_safe(slim_of(park, item_id), 'upkeepState')))
        check('the row upkeepState is full power and the controller is neither low nor abandoned',
              entry.get('upkeepState') == full and ctl is not None
              and ctl.IsLowPower() is False and ctl.IsAbandoned() is False,
              text(entry.get('upkeepState')))
        ok, info = rcall('structureDirectory', 'GetStructureInfo', item_id)
        check('GetStructureInfo agrees on state and upkeep',
              ok and getattr_safe(info, 'state') == S.STATE_ANCHORING
              and getattr_safe(info, 'upkeepState') == full, text(info)[:200])

        # -- 3. onlining -----------------------------------------------------
        online, waited = wait_for(
            lambda: getattr_safe(slim_of(park, item_id), 'state') == S.STATE_ONLINING_VULNERABLE,
            timeout_ms=ONLINE_BOUND_MS, step_ms=2000)
        check('after the anchor time the slim says STATE_ONLINING_VULNERABLE', bool(online),
              'state %s after %s ms' % (text(getattr_safe(slim_of(park, item_id), 'state')),
                                         text(waited)))
        if not online:
            return
        late = wall_ft()
        if late is not None:
            check('it came at or after the timer\'s end (not early)',
                  late >= end - 5 * SEC, 'now-end %s s' % fmt((late - end) / float(SEC), 1))
        slim = slim_of(park, item_id)
        check('the countdown is gone (the client clears it in onlining)',
              getattr_safe(slim, 'timer') is None, text(getattr_safe(slim, 'timer')))
        check('the upkeep is still full power', getattr_safe(slim, 'upkeepState') == full,
              text(getattr_safe(slim, 'upkeepState')))
        entry, why = entry_of(item_id)
        check('its row is onlining with no timerEnd',
              entry is not None and entry.get('state') == S.STATE_ONLINING_VULNERABLE
              and entry.get('timerEnd') is None and entry.get('upkeepState') == full,
              text(entry)[:200] if entry is not None else why)
        system = getattr_safe(builtin('session'), 'solarsystemid2') \
            or getattr_safe(builtin('session'), 'solarsystemid')
        ok, v = rcall('structureDirectory', 'GetMyDockableStructures', system)
        check('it is dockable then', ok and item_id in list(v), text(v)[:200])

        # -- 4. the unanchor timer -------------------------------------------
        cancels = S.STATE_CANCELS_UNANCHOR.get(S.STATE_ONLINING_VULNERABLE)
        check('the client allows an unanchor order in this state', cancels is False, text(cancels))
        t_u0 = wall_ft()
        ok, v = rcall('structureDeployment', 'Unanchor', item_id)
        check('Unanchor is accepted', ok, text(v)[:200])
        if not ok:
            return
        entry, why = entry_of(item_id)
        un = entry.get('unanchoring') if entry is not None else None
        check('the row carries an unanchoring time', un is not None, text(entry)[:200])
        if un is not None and t_u0 is not None:
            ahead = (un - t_u0) / float(SEC)
            check('it is UNANCHORING_TIME seconds ahead of the client\'s wallclock (+-60 s)',
                  abs(ahead - S.UNANCHORING_TIME) <= 60,
                  'ahead %s s, client wants %s' % (fmt(ahead, 0), text(S.UNANCHORING_TIME)))
        ctl = controller_of(entry)
        if ctl is not None:
            check('the controller IsUnanchoring and GetUnanchorTime is that time',
                  ctl.IsUnanchoring() is True and ctl.GetUnanchorTime() == un
                  and ctl.CanUnanchor() is False,
                  'time %s' % text(ctl.GetUnanchorTime()))
            ok, cc = safe(ctl.CanCancelUnanchor)
            say('CanCancelUnanchor (needs the director role in the session): %s' % text(cc))
        check('the slim state is unchanged by the order',
              getattr_safe(slim_of(park, item_id), 'state') == S.STATE_ONLINING_VULNERABLE,
              text(getattr_safe(slim_of(park, item_id), 'state')))
        sleep_ms(2000)
        ok, v = rcall('structureDeployment', 'Unanchor', item_id)
        entry2, _why = entry_of(item_id)
        check('a second Unanchor keeps the same end (StartUnanchoring is a no-op while running)',
              entry2 is not None and entry2.get('unanchoring') == un,
              'first %s second %s accepted %s' % (text(un),
                                                 text(entry2.get('unanchoring') if entry2 else None),
                                                 text(ok)))
        ok, v = rcall('structureDeployment', 'CancelUnanchor', item_id)
        check('CancelUnanchor is accepted', ok, text(v)[:200])
        entry, why = entry_of(item_id)
        ctl = controller_of(entry)
        check('and the unanchoring is gone from the row and the controller',
              entry is not None and entry.get('unanchoring') is None
              and ctl is not None and ctl.IsUnanchoring() is False,
              text(entry)[:200] if entry is not None else why)
        no_new_tracebacks('upwell timers', mark)

    # -- 6-8. the reinforce timers and their notifications (issue #47) --------

    def watch_notifications():
        """`(seen, svc)`: a spy on the client's `OnNotificationReceived`.
        `seen` gets `(typeID, notificationID, senderID, data)` per push."""
        seen = []
        svc = get_svc('notificationSvc')
        if svc is None:
            return seen, None
        orig = svc.OnNotificationReceived

        def spy(*a, **kw):
            try:
                data = kw.get('data')
                if data is None and len(a) > 4:
                    data = a[4]
                seen.append((a[1], a[0], a[2], data))
            except Exception:
                pass
            return orig(*a, **kw)
        svc.OnNotificationReceived = spy
        return seen, svc

    def unwatch(svc):
        try:
            del svc.OnNotificationReceived
        except Exception:
            pass

    def note_for(seen, type_id, struct_id=None):
        for t, nid, sender, data in seen:
            if t != type_id:
                continue
            if struct_id is not None:
                try:
                    if data.get('structureID') != struct_id:
                        continue
                except Exception:
                    continue
            return (nid, sender, data)
        return None

    def client_formats(type_id, nid, sender, data):
        """The client's own formatter on one pushed notification:
        `(ok, subject)`; ok needs a non-empty subject and body."""
        def fmt_it():
            from notifications.common.notification import Notification
            from notifications.common.formatting.notificationFormatMapping import GetFormatterForType
            n = Notification(nid, type_id, sender, None, False, 0, data)
            GetFormatterForType(type_id).Format(n)
            return n
        ok, n = safe(fmt_it)
        if not ok:
            return False, text(n)[:200]
        return bool(n.subject) and bool(n.body), text(n.subject)[:80]

    def reinforce_run():
        mark = log_mark()
        ok, st = safe(lambda: __import__('structures'))
        precondition("the client's structures package imports (steps 6-8)", ok, text(st)[:200])
        ok2, nc = safe(lambda: __import__(
            'eve.common.script.util.notificationconst', fromlist=['x']))
        precondition("the client's notificationconst imports", ok2, text(nc)[:200])
        if not ok or not ok2:
            return
        SEC = sec_ticks()
        park, ego = get_ego_ball()
        precondition('the pilot is in space (steps 6-8)', park is not None and ego is not None)
        precondition("the client's const.SEC reads (steps 6-8)", bool(SEC), text(SEC))
        if park is None or not SEC:
            return
        system = getattr_safe(builtin('session'), 'solarsystemid2') \
            or getattr_safe(builtin('session'), 'solarsystemid')
        tids = type_ids_named(TYPE_NAME, exact=True)
        precondition('the client knows the %s type' % TYPE_NAME, bool(tids), text(tids))
        if not tids:
            return
        cls_map = get_svc('map')
        ok, klass = safe(lambda: cls_map.GetSecurityClass(system))
        precondition("the client's map service gives the system's security class",
                     ok and klass in st.REINFORCE_DURATION_ARMOR_MEDIUM_BY_SECURITY,
                     'class %s' % text(klass))
        if not ok or klass not in st.REINFORCE_DURATION_ARMOR_MEDIUM_BY_SECURITY:
            return
        seen, svc = watch_notifications()
        precondition('the notification service can be watched', svc is not None)
        if svc is None:
            return
        try:
            before = set(k for k, _s in (slims_where(
                park, lambda s: getattr_safe(s, 'typeID') == tids[0]) or []))
            name = 'Fci Reinforce ' + uniq(4)
            gm_check('/upwell spawn puts an online structure here', '/upwell spawn astrahus ' + name,
                     expect=['online'])
            found, _w = wait_for(
                lambda: [f for f in (slims_where(
                    park, lambda s: getattr_safe(s, 'typeID') == tids[0]) or [])
                    if f[0] not in before],
                timeout_ms=30000)
            check('the spawned structure is a slim in the park', bool(found))
            if not found:
                return
            sid = found[0][0]
            entry0, why0 = entry_of(sid)
            precondition('the corporation tab lists it', entry0 is not None, why0)
            if entry0 is None:
                return
            window0 = (entry0.get('reinforce_weekday'), entry0.get('reinforce_hour'))
            no_day = st.NO_REINFORCEMENT_WEEKDAY
            precondition('the window hour is not 20 already', window0[1] != 20, text(window0))

            # -- 6. shield out ---------------------------------------------------
            gm_check('/upwell damage takes the shield and hurts the armour',
                     '/upwell damage %d 0 0.9 1' % sid, expect=['state'])
            reached, _w = wait_for(
                lambda: getattr_safe(slim_of(park, sid), 'state') == st.STATE_ARMOR_REINFORCE,
                timeout_ms=30000)
            check('the slim says STATE_ARMOR_REINFORCE', bool(reached),
                  text(getattr_safe(slim_of(park, sid), 'state')))
            t = timer_parts(slim_of(park, sid))
            check('the slim carries a reinforce timer (start, end, paused None)',
                  t is not None and t[0] is not None and t[1] is not None and t[2] is None,
                  text(t))
            note, _w = wait_for(lambda: note_for(
                seen, nc.notificationTypeStructureLostShields, sid), timeout_ms=30000)
            check('a StructureLostShields notification for it arrives', bool(note),
                  'seen types %s' % text([x[0] for x in seen]))
            if note and t is not None:
                nid, sender, data = note
                check('its timestamp equals the slim timer end', data.get('timestamp') == t[1],
                      'timestamp %s end %s' % (text(data.get('timestamp')), text(t[1])))
                left = data.get('timeLeft')
                check('its timeLeft is positive and no longer than the timer',
                      left is not None and 0 < left <= (t[1] - t[0]), text(left))
                base = st.REINFORCE_DURATION_ARMOR_MEDIUM_BY_SECURITY[klass]
                jitter = st.REINFORCE_EXIT_JITTER_MEDIUM_SECONDS
                span = (t[1] - t[0]) / float(SEC)
                check('the timer is the client\'s medium length for the class, less the jitter, '
                      'and no more than seven days and the jitter over',
                      base - jitter <= span <= base + 7 * 86400 + jitter,
                      'span %s s class %s want %s..%s' % (
                          fmt(span, 0), text(klass), text(base - jitter),
                          text(base + 7 * 86400 + jitter)))
                ok_f, why_f = client_formats(
                    nc.notificationTypeStructureLostShields, nid, sender, data)
                check("the client's own formatter formats it", ok_f, why_f)

            # -- 7. the GM timer -------------------------------------------------
            gm_check('/upwell timer ends the reinforce in five seconds',
                     '/upwell timer %d 5' % sid, expect=['5 s'])
            vuln, _w = wait_for(
                lambda: getattr_safe(slim_of(park, sid), 'state') == st.STATE_ARMOR_VULNERABLE,
                timeout_ms=30000)
            check('within 30 s the slim says STATE_ARMOR_VULNERABLE', bool(vuln),
                  text(getattr_safe(slim_of(park, sid), 'state')))

            # -- 8. UpdateReinforceTimesFromClient --------------------------------
            ok, v = rcall('structureVulnerability', 'UpdateReinforceTimesFromClient',
                          [(sid, system, tids[0])], no_day, 20)
            check('UpdateReinforceTimesFromClient returns', ok, text(v)[:300])
            if not ok:
                return
            entry, why = entry_of(sid)
            now = wall_ft()
            check('the row queues weekday 255 hour 20 with an apply time ahead',
                  entry is not None and entry.get('next_reinforce_weekday') == no_day
                  and entry.get('next_reinforce_hour') == 20
                  and entry.get('next_reinforce_apply') is not None
                  and now is not None and entry.get('next_reinforce_apply') > now,
                  text(entry)[:300] if entry is not None else why)
            check('the live window is still the old one',
                  entry is not None and (entry.get('reinforce_weekday'),
                                         entry.get('reinforce_hour')) == window0,
                  text(window0))
            note, _w = wait_for(lambda: note_for(
                seen, nc.notificationTypeStructuresReinforcementChanged), timeout_ms=30000)
            check('a StructuresReinforcementChanged notification arrives', bool(note),
                  'seen types %s' % text([x[0] for x in seen]))
            if note:
                nid, sender, data = note
                infos = data.get('allStructureInfo') or []
                check('it names this structure, the day and the hour',
                      data.get('numStructures') == 1 and len(infos) == 1
                      and infos[0][0] == sid and data.get('weekday') == no_day
                      and data.get('hour') == 20, text(data)[:300])
                ok_f, why_f = client_formats(
                    nc.notificationTypeStructuresReinforcementChanged, nid, sender, data)
                check("the client's own formatter formats it", ok_f, why_f)
        finally:
            unwatch(svc)
        no_new_tracebacks('upwell reinforce', mark)

    # -- 5. the 184, from real shots (issue #48) --------------------------------

    UA_GUN = '200mm autocannon i'
    UA_AMMO = 'emp s'
    UA_FLAG_HI0 = 27                # flagHiSlot0
    UA_GUN_RANGE_M = 2500           # [A] a distance at which the guns should reach the surface; a miss is a PRECONDITION
    UA_BOUND_MS = 600000            # a bound on the shots, never a claim

    def ua_near(a, b, tol=0.01):
        return a is not None and abs(float(a) - b) <= tol

    def ua_go_to(system):
        for attempt in (1, 2):
            gm('/tr me %d' % system)
            got, _w = wait_for(
                lambda: getattr_safe(builtin('session'), 'solarsystemid2') == system,
                timeout_ms=45000, step_ms=500)
            if got:
                return True
            sleep_ms(10000)
        return False

    def ua_kit():
        """In the station: skills, two guns and EMP S on the active Rifter.
        `(guns, ok)`."""
        gun = type_ids_named(UA_GUN, exact=True)
        ammo = type_ids_named(UA_AMMO, exact=True)
        precondition('the client knows the gun and the ammo', bool(gun and ammo),
                     'gun=%s ammo=%s' % (text(gun), text(ammo)))
        if not (gun and ammo):
            return [], False
        for line in ('/giveskill Gunnery 3', '/giveskill Small Projectile Turret 3',
                     '/createitem "200mm AutoCannon I" 2', '/createitem "EMP S" 300'):
            gm_check('GM: ' + line, line)
        guns = []
        for slot in (UA_FLAG_HI0, UA_FLAG_HI0 + 1):
            ok, detail = fit_module(gun[0], slot)
            check('a gun fits at flag %d' % slot, ok, detail)
            m = module_item(gun[0], slot)
            if m is not None:
                guns.append(m)
        if len(guns) < 2:
            return guns, False
        for m in guns:
            ok, detail = online_module(m.itemID)
            check('module %s onlines' % text(m.itemID), ok, detail)
        charge, why = find_hangar_item(ammo[0])
        check('the EMP S stack is in the hangar', charge is not None, why)
        if charge is None:
            return guns, False
        ok, detail = load_ammo([m.itemID for m in guns], ammo[0], charge)
        check('the guns take the EMP S', ok, detail)
        return guns, ok

    def underattack_run(station0):
        mark = log_mark()
        ok, nc = safe(lambda: __import__(
            'eve.common.script.util.notificationconst', fromlist=['x']))
        precondition("the client's notificationconst imports (step 5)", ok, text(nc)[:200])
        rifter = type_ids_named('rifter', exact=True)
        tids = type_ids_named(TYPE_NAME, exact=True)
        precondition('the client knows the Rifter and the %s' % TYPE_NAME,
                     bool(rifter and tids), '%s %s' % (text(rifter), text(tids)))
        precondition('the run began docked, so there is a station to go back to',
                     bool(station0), text(station0))
        if not (ok and rifter and tids and station0):
            return
        me = getattr_safe(builtin('session'), 'charid')
        gm('/tr me %d' % station0)
        docked, _w = wait_for(
            lambda: getattr_safe(builtin('session'), 'stationid') == station0,
            timeout_ms=60000, step_ms=500)
        precondition('back in the start station', bool(docked))
        if not docked:
            return
        gm_check('GM: /createitem rifter', '/createitem rifter')
        ok, detail = board_ship(rifter[0])
        check('boarded the Rifter', ok, detail)
        if not ok:
            return
        guns, kit_ok = ua_kit()
        if not kit_ok:
            return
        undocked, detail = undock()
        check('undocked into space', undocked, detail)
        if not undocked:
            return
        sleep_ms(3000)
        home = getattr_safe(builtin('session'), 'solarsystemid2')
        cands = [c for c in map_systems(
            lambda row: float(row.securityStatus) < 0.05, limit=40) if c != home]
        precondition('a null-sec system is known', bool(cands), '%d candidate(s)' % len(cands))
        if not cands:
            return
        old_park = get_michelle().GetBallpark()
        if not ua_go_to(cands[0]):
            precondition('the pilot reaches the first null-sec system', False, text(cands[0]))
            return
        wait_for(lambda: get_michelle().GetBallpark() is not old_park, timeout_ms=60000)
        park, ego = get_ego_ball()
        precondition('the pilot is in space in a null-sec system',
                     park is not None and ego is not None)
        if park is None:
            return
        gm_check('GM: out of the station\'s zone', '/tp 20000',
                 forbid=['pick between', 'not in space'])
        sleep_ms(5000)
        safe(lambda: get_svc('crimewatchSvc').SetSafetyLevel(0))
        seen, svc = watch_notifications()
        precondition('the notification service can be watched', svc is not None)
        if svc is None:
            return
        try:
            before = set(k for k, _s in (slims_where(
                park, lambda s: getattr_safe(s, 'typeID') == tids[0]) or []))
            gm_check('/upwell spawn puts an online structure here',
                     '/upwell spawn astrahus Fci Attack ' + uniq(4), expect=['online'])
            found, _w = wait_for(
                lambda: [f for f in (slims_where(
                    park, lambda s: getattr_safe(s, 'typeID') == tids[0]) or [])
                    if f[0] not in before],
                timeout_ms=30000)
            check('the structure is a slim in the park', bool(found))
            if not found:
                return
            sid = found[0][0]
            # Hurt but above the server's 95 % line: aggressed, nothing said.
            gm_check('/upwell damage hurts the shield a little',
                     '/upwell damage %d 0.9501 1 1' % sid, expect=['state'])
            sleep_ms(3000)
            check('the GM damage alone says nothing',
                  note_for(seen, nc.notificationTypeStructureUnderAttack, sid) is None,
                  'seen types %s' % text([x[0] for x in seen]))
            ok, detail = approach(park, sid, UA_GUN_RANGE_M)
            precondition('the Rifter closes on the structure', ok, detail)
            if not ok:
                return
            locked, detail = lock_target(sid, timeout_ms=60000)
            check('the structure is locked for the guns', locked, detail)
            if not locked:
                return
            for m in guns:
                ok, detail = activate_module(m.itemID)
                check('gun %s activates on the structure' % text(m.itemID), ok, detail)
            state = {'polls': 0}

            def told():
                state['polls'] += 1
                if state['polls'] % 15 == 0:
                    for m in guns:
                        if module_active(m.itemID) is False:
                            activate_module(m.itemID)
                return note_for(seen, nc.notificationTypeStructureUnderAttack, sid)
            note, waited = wait_for(told, timeout_ms=UA_BOUND_MS, step_ms=1000)
            for m in guns:
                deactivate_module(m.itemID)
            check('5. a StructureUnderAttack notification for it arrives', bool(note),
                  'seen types %s after %s ms' % (text([x[0] for x in seen]), text(waited)))
            if note:
                nid, sender, data = note
                check('5. its charID is the shooter (this pilot)', data.get('charID') == me,
                      '%s vs %s' % (text(data.get('charID')), text(me)))
                shield = data.get('shieldPercentage')
                check('5. the shield reads under 95 % and the armour and hull 100 %',
                      shield is not None and shield < 95.0
                      and ua_near(data.get('armorPercentage'), 100.0)
                      and ua_near(data.get('hullPercentage'), 100.0),
                      text((shield, data.get('armorPercentage'), data.get('hullPercentage'))))
                link = data.get('corpLinkData')
                check('5. corpLinkData is a three-item tuple', link is not None and len(link) == 3,
                      text(link))
                ok_f, why_f = client_formats(
                    nc.notificationTypeStructureUnderAttack, nid, sender, data)
                check("5. the client's own formatter formats it", ok_f, why_f)
        finally:
            unwatch(svc)
        no_new_tracebacks('upwell under attack', mark)

    def eoci_body():
        station0 = getattr_safe(builtin('session'), 'stationid')
        timers_run()
        reinforce_run()
        underattack_run(station0)

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
