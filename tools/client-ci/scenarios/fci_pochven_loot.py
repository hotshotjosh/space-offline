# -*- coding: utf-8 -*-
"""Pochven wrecks carry their own loot (#228, PV-9).

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Packet F-CI (`docs/lanes/f-ci.md`).

Regression for PV-9: a Triglavian wreck in a Pochven system holds what
`content/pochven/loot.tsv` draws (a Survey Database among it) beside what the NPC
class already gave.  Needs `pochven::loot::wreck_extras` called from
`combat::npc_wreck` (the maintainer's one hot line); without it check b fails.

GM setup: `/devskills all5`, a Tempest with six 800mm repeating cannons and EMP L,
undock, `/tr me` into a system of `const.factionTriglavian` read from the client map,
then five times `/entity deploy 1 <the Triglavian battleship read by name>` and kill it.
Checks:

a. A wreck is left where each battleship died, its loot list opens, and its slim's
   `ownerID` is the pilot, the loot-right holder **[I]**: the client treats an NPC-owned
   wreck as lootable by anyone (`Park.HaveLootRight`, michelle.py:1418-1423), so the server
   gives a loot-restricted NPC wreck the holder as owner (`combat::npc_wreck_owner`, itself [A]: from EvEJS, combat.rs:2915-2919).
   Falsifier: a TQ capture of an NPC wreck slim whose `ownerID` is the NPC corporation.
b. Over the five kills at least one wreck holds a Triglavian data item of group 1995
   (the Pochven table's battleship row draws any type of that group, five in the installed
   type data, not only the Survey Database).  The `[A]` rate of 0.9 per kill gives a
   99.999 % chance of at least one group-1995 stack over five kills, so a failure means the
   path is missing, not bad luck.  Whether PV-9 means the Survey Database itself is the
   owner's call (a content row change, not this scenario).
Before each kill the pilot is healed (`/heal me`): the battleship shoots back for the whole
fight and a dead Tempest ends the run.
c. No new client traceback.

No type id is written in this file; every type is read from the client by name.
"""

EOCI_SCENARIO = 'fci_pochven_loot'

# Seconds to pass to `tools/client-ci.sh run fci_pochven_loot --timeout`: login,
# fitting, five battleship kills (up to eight minutes each) and the quit.
EOCI_FCI_TIMEOUT_S = 3600

# SERVER-EXPECT: absent  UNANSWERED (invbroker|dogmaIM)
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

    HULL = 'tempest'
    GUN = '800mm repeating cannon ii'
    AMMO = 'emp l'
    GUNS = 6
    TRIG_BATTLESHIP = 'raznaborg leshak'
    SURVEY_GROUP = 1995     # [V-data] group of 'Triglavian Survey Database' (out/content/types/typedata.tsv)
    KILLS = 5
    KILL_WAIT_MS = 480000
    WRECK_NEAR_M = 20000

    def trig_faction():
        return const_of('factionTriglavian', None)

    def is_trig_row(row):
        f = trig_faction()
        return f is not None and getattr_safe(row, 'factionID') == f

    def type_name(type_id):
        ok, v = safe(lambda: __import__('evetypes').GetName(type_id))
        return text(v) if ok else ''

    def group_of(type_id):
        ok, g = safe(lambda: __import__('evetypes').GetGroupID(type_id))
        return g if ok else None

    def go_to(system):
        for attempt in (1, 2):
            ok, reply = gm('/tr me %d' % system)
            got, _w = wait_for(
                lambda: getattr_safe(builtin('session'), 'solarsystemid2') == system,
                timeout_ms=45000, step_ms=500)
            if got:
                def parked():
                    p = get_michelle().GetBallpark()
                    return bool(p is not None and getattr_safe(p, 'solarsystemID') == system
                                and getattr_safe(p, 'ego') and p.GetBall(p.ego))
                wait_for(parked, timeout_ms=90000, step_ms=500)
                import blue
                wait_for(lambda: (getattr_safe(builtin('session'), 'nextSessionChange') or 0)
                         <= blue.os.GetSimTime(), timeout_ms=30000, step_ms=500)
                sleep_ms(1000)
                return True
            sleep_ms(10000)
        return False

    def wreck_balls(park):
        group = const_of('groupWreck', None)
        if group is None:
            return []
        return slims_where(park, lambda s: getattr_safe(s, 'groupID') == group)

    def ball_pos(park, ball_id):
        ok, b = safe(lambda: park.GetBall(ball_id))
        if not ok or b is None:
            return None
        return (getattr_safe(b, 'x', 0.0), getattr_safe(b, 'y', 0.0), getattr_safe(b, 'z', 0.0))

    def apart(a, b):
        return ((a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 + (a[2] - b[2]) ** 2) ** 0.5

    def gm_info_check(name, line, expect):
        """`gm_check` for a command that answers as a raised `CustomInfo`
        (`service/gm.rs` VISIBLE, as `/heal` does): the raise IS the readout,
        as in fci_gm_pilot.py."""
        ok, value = rcall('slash', 'SlashCmd', line)
        reply = text(value)
        if not ok and 'CustomInfo' in reply:
            ok = True
        low = reply.lower()
        check(name, ok and all(e.lower() in low for e in expect), '%s -> %s' % (line, reply[:300]))
        return ok, reply

    def eoci_body():
        mark = log_mark()
        sess = builtin('session')
        station = getattr_safe(sess, 'stationid')
        precondition('the character is docked', bool(station), 'stationid = %s' % text(station))
        if not station:
            return
        hull = type_ids_named(HULL, exact=True)
        gun = type_ids_named(GUN, exact=True)
        ammo = type_ids_named(AMMO, exact=True)
        raz = type_ids_named(TRIG_BATTLESHIP, exact=True)
        ok_s, ids_s = safe(lambda: list(__import__('evetypes').GetTypeIDsByGroup(SURVEY_GROUP)))
        survey = ids_s if ok_s and ids_s else []
        precondition('the client knows the Tempest, the gun, the ammo, a Triglavian battleship '
                     'and a type of group 1995',
                     bool(hull and gun and ammo and raz and survey),
                     'hull=%s gun=%s ammo=%s raz=%s survey=%s'
                     % (text(hull), text(gun), text(ammo), text(raz), text(survey)))
        if not (hull and gun and ammo and raz and survey):
            return
        precondition('the client has groupWreck', const_of('groupWreck', None) is not None,
                     'invconst name')
        if trig_faction() is None:
            precondition('the client knows const.factionTriglavian', False, 'no constant')
            return
        pochven = map_systems(is_trig_row, limit=200)
        precondition('the client map holds Triglavian systems', bool(pochven),
                     '%d system(s)' % len(pochven))
        if not pochven:
            return
        # /createitem takes 1..1000 per call (item.rs SpawnError::BadQuantity), so the 2000
        # rounds come as two calls of 1000 [V ci-1110 pochven_loot: the single 2000 was refused
        # and the old gm_check had no `expect`, so the scenario went on without ammo].
        # needs --dev (regress.list); without it the answer is 'dev only' and the pilot cannot
        # fire the Tempest's guns [V ci-1111 pochven_loot: the Leshak lived 480 s].
        gm_check('GM: /devskills all5', '/devskills all5', expect=['skills at level'])
        for line in ('/createitem %d 1' % hull[0],
                     '/createitem %d %d' % (gun[0], GUNS),
                     '/createitem %d 1000' % ammo[0], '/createitem %d 1000' % ammo[0]):
            gm_check('GM: ' + line, line, expect=['created'])
        ok, detail = board_ship(hull[0])
        precondition('boarded the Tempest', ok, detail)
        if not ok:
            return
        guns = []
        for slot in range(FLAG_HI0, FLAG_HI0 + GUNS):
            ok, detail = fit_module(gun[0], slot)
            m = module_item(gun[0], slot) if ok else None
            if m is not None:
                online_module(m.itemID)
                guns.append(m)
        precondition('at least three guns are fitted (a battleship dies slowly)',
                     len(guns) >= 3, '%d of %d fitted' % (len(guns), GUNS))
        if len(guns) < 3:
            return
        charge, why = find_hangar_item(ammo[0])
        precondition('the ammo stack is in the hangar', charge is not None, why)
        if charge is None:
            return
        ok, detail = load_ammo([m.itemID for m in guns], ammo[0], charge)
        precondition('the guns take the ammo', ok, detail)
        undocked, detail = undock()
        precondition('undocked into space', undocked, detail)
        if not undocked:
            return
        system = sorted(pochven)[0]
        precondition('arrived in the Triglavian system', go_to(system), 'system %s' % text(system))
        # /tr arrives under the gate cloak, and a cloaked ship cannot lock or fire
        rbp = get_michelle().GetRemotePark()
        safe(lambda: rbp.CmdStop())

        def visible():
            p = get_michelle().GetBallpark()
            return p is not None and not getattr_safe(p.GetBall(p.ego), 'isCloaked')
        gone, waited = wait_for(visible, timeout_ms=30000, step_ms=500)
        precondition('the arrival cloak is off', bool(gone), 'after %s ms' % text(waited))
        if not gone:
            return
        park = get_michelle().GetBallpark()
        me = getattr_safe(sess, 'charid')
        hull_ship = getattr_safe(sess, 'shipid')
        held = []           # per kill: (wreck ball, owner, [(type, qty)])
        for n in range(KILLS):
            gm_info_check('GM: /heal me (kill %d)' % (n + 1), '/heal me', ['healed'])
            still = getattr_safe(sess, 'shipid') == hull_ship
            precondition('kill %d: still in the Tempest' % (n + 1), still,
                         'session.shipid %s, the Tempest %s' % (
                             text(getattr_safe(sess, 'shipid')), text(hull_ship)))
            if not still:
                break
            before = set([b for b, _s in wreck_balls(park)])
            gm_check('GM: deploy one Triglavian battleship (kill %d)' % (n + 1),
                     '/entity deploy 1 %s' % text(raz[0]))
            found, waited = wait_for(
                lambda: slims_where(park, lambda s: getattr_safe(s, 'typeID') == raz[0]),
                timeout_ms=30000)
            npcs = slims_where(park, lambda s: getattr_safe(s, 'typeID') == raz[0])
            check('kill %d: the battleship is in the park' % (n + 1), bool(npcs),
                  '%d ball(s) after %s ms' % (len(npcs), text(waited)))
            if not npcs:
                break
            npc_id = npcs[0][0]
            last_at = ball_pos(park, npc_id)
            ok, detail = approach(park, npc_id, 15000)
            ok2, detail2 = lock_target(npc_id)
            if not ok2:
                check('kill %d: the battleship locks' % (n + 1), False, detail2)
                break
            acts = [activate_module(m.itemID) for m in guns]
            check('kill %d: the guns activate' % (n + 1), any(a_[0] for a_ in acts),
                  text([a_[1] for a_ in acts][:3]))
            if not any(a_[0] for a_ in acts):
                break
            dead, waited = wait_for(lambda: npc_id not in park.slimItems,
                                    timeout_ms=KILL_WAIT_MS, step_ms=1000)
            for m in guns:
                deactivate_module(m.itemID)
            gm_info_check('GM: /heal me after kill %d' % (n + 1), '/heal me', ['healed'])
            check('kill %d: the battleship is destroyed (it leaves the park)' % (n + 1),
                  bool(dead), 'after %s ms; approach: %s' % (text(waited), detail))
            if not dead:
                break
            fresh = [w for w, _s in wreck_balls(park) if w not in before]
            near = []
            for w in fresh:
                at = ball_pos(park, w)
                if at is not None and last_at is not None and apart(at, last_at) <= WRECK_NEAR_M:
                    near.append(w)
            if not near:
                check('kill %d: a wreck appears where it died' % (n + 1), False,
                      '%d new wreck(s), none near' % len(fresh))
                continue
            wreck = near[0]
            ok, loot = safe(lambda: get_svc('invCache').GetInventoryFromId(wreck).List())
            rows = []
            if ok and loot:
                for r in list(loot):
                    rows.append((getattr_safe(r, 'typeID'), getattr_safe(r, 'stacksize') or 1))
            held.append((wreck, getattr_safe(park.slimItems.get(wreck), 'ownerID'), ok, rows))

        # a. a wreck whose slim ownerID is the pilot (the loot-right holder), and a loot window
        # that opens
        check('a: at least one wreck was left and its loot list opened without raising',
              any(h[2] for h in held), '%d wreck(s) over %d kill(s)' % (len(held), KILLS))
        for h in held:
            # [I] michelle.py:1418-1423 HaveLootRight: an NPC-owned slim is lootable by anyone,
            # so a restricted wreck carries the right holder as ownerID (combat.rs
            # npc_wreck_owner; that server rule is [A] too, from EvEJS).  Falsifier: a TQ NPC wreck slim with the NPC corp as ownerID.
            check("a: the wreck %s's ownerID is the pilot (the loot-right holder)" % text(h[0]),
                  bool(h[1]) and h[1] == me, 'ownerID %s, pilot %s' % (text(h[1]), text(me)))
        # b. over five kills at least one wreck holds a data item of group 1995.  The [A]
        # chance of 0.9 per kill makes none in five a one in a hundred thousand: a failure
        # means the path is missing (the wreck_extras line, the table, or the type pool).
        got = []
        for h in held:
            for t, q in h[3]:
                if t in survey:
                    got.append((h[0], t, q))
        check('b: a wreck holds a Triglavian data item of group 1995 (the Pochven table\'s row) '
              'among %d kill(s)' % len(held),
              len(got) > 0, 'group-1995 stacks %s; wreck contents %s'
              % (text(got), text([(h[0], h[3]) for h in held])))
        # c. no traceback
        no_new_tracebacks('pochven loot', mark)

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
