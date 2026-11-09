# -*- coding: utf-8 -*-
"""Sovereignty campaign: an entosis attack reinforces a hub, the campaign starts, command nodes move the score (issue #92).

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Lane `sov-campaign`.

One client, two alliances.  The character founds alliance A and then alliance B
(`/devcorp` puts it in a fresh corporation each time, so each founding is the
ordinary `CreateAlliance`).  It carries a Tayra full of fuel blocks and
Strontium Clathrates with one Entosis Link II fitted, undocks, goes to a null-sec
system and then, in rounds:

1. DEFENDER.  Pilot back in A: `/sov set here A` gives A the system and a hub;
   the pilot warps to the hub ball (group 1012) and `/sovfuel all` fuels it
   (only the holder's alliance may, `sov_hub::refuel`; without fuel the claim
   lapses after about three game days, before a 48 hour reinforcement ends).
   The client's `svc.sov` structure list for the system names A on the hub and
   carries no campaign.
2. ATTACK.  Pilot in B (the attacker), the hub locked.  Wait for a moment the
   client's own `vulnerabilityState` window is open with room for the link's
   warm-up cycle, then activate the Entosis Link on the hub.  What the client
   shows: the hub slim's `entosis_score` names the attacking team (-A, "everyone
   but A") scoring up against defender A; then the hub is reinforced: the
   structure list carries `campaignState` = (event type, defender, start time,
   {team: score}) with the two teams of the bar and no vulnerability window,
   the link stops, and a new link on the reinforced hub does not run.
3. CAMPAIGN.  Wait (bounded) for the start time: the hub row stays in its
   campaign, command nodes (group 1316) appear in the park, each slim's
   `campaign_sourceInfo` names the system, the event type, the hub and A.
4. NODE.  Warp to a node, lock it, link it for real: the node leaves the park
   and the bar the client draws moves, the defender's share falling and B
   getting a key of its own, the total staying 1.
5. END.  GM `/sov nodes capture <id> attackers` until the hub falls: the
   structure list for the system empties and the hub ball leaves the park.

How the timers fit a run (`# SERVER-ARGS` below, `docs/guides/gm-commands.md`):
`--time-scale 60` (the server's largest) compresses every GAME span: the 48 hour
reinforcement is still 48 minutes, a game day (the window's period, the fuel
burn's day) 24 minutes, the 18 hour vulnerability window 18 minutes, the 595 s
hub capture 10 s, the 235 s node capture 4 s.  The 48 minutes are too long, so
after the link's reinforcement is checked the GM ends it (`/sov resolve here
defenders`) and starts a campaign for now (`/sov reinforce here now`), and
`/sov prime` opens the defender's window at once instead of waiting for it.  The module's cycle (Entosis Link II: 120 s) is real
time, so the window has to be open for the warm-up cycle after the click: the
scenario waits for the first part of a window.  Fuel is 240 blocks per game day
per hub, so about five game days of campaign need about 1200 blocks, hence the
Tayra's 7300 m3 hold.  `--dev` for `/devcorp`.

NOT here, for a person (`docs/guides/human-checks.md`): the hub's and the
nodes' brackets, the countdown timers, the dashboard's drawing of the bar.
What is read is what the client holds: `svc.sov`'s cache, the slim items, the
module's state.  The soak half is #91.

Assumptions, each with its falsifier ([A]):
- `Tayra`, `Entosis Link II`, `Nitrogen Fuel Block` and `Strontium Clathrates`
  are what the client calls them; a miss is a PRECONDITION.
- `/devcorp <ticker>` in space changes `session.corpid` and `allianceid` to the
  corporation's (falsifier: the PRECONDITIONs on the alliance ids).
- the first cycle of the link is warm-up and scoring starts when it ends
  (`entosis.rs` module note, [A]); falsifier: `entosis_score` never turns up
  within two cycles of the click.
- the client's wallclock (`blue.os`) and the server's FILETIME agree, both being
  the host's clock; falsifier: no window ever reads as open.
- the sov hub of a GM claim sits in the park as a ball of group 1012 and the
  nodes as balls of group 1316 (`sov_hub.rs`, `command_nodes.rs`); falsifier:
  the PRECONDITION that finds the ball.
- the bar's two starting teams are A and -A and their shares sum to 1; the
  shares themselves and the step a node takes are the server's numbers, not the
  client's, and are never compared with a constant here.
"""

EOCI_SCENARIO = 'fci_sov_campaign'

# Seconds to pass to `tools/client-ci.sh run fci_sov_campaign --timeout`.  Setup
# 5 min, flight 3, the wait for an open window up to 5, the warm-up 2, the
# GM-started campaign 3, the node 4, the end 3: about 25 minutes at most.
EOCI_FCI_TIMEOUT_S = 2400

# SERVER-ARGS: --dev --time-scale 60

# SERVER-EXPECT: absent  UNANSWERED (sovMgr|dogmaIM|ship|invbroker|corpRegistry|allianceRegistry|account|beyonce|michelle)
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

    # -- this scenario ------------------------------------------------------

    GROUP_HUB = 1012          # sov_hub::GROUP_INFRASTRUCTURE_HUB (client const groupInfrastructureHub)
    GROUP_NODE = 1316         # command_nodes::GROUP_COMMAND_NODE
    EVENT_HUB = 2             # entosisConst.EVENT_TYPE_IHUB_DEFENSE [V] (entosis.rs)
    FLAG_CARGO = 5            # flagCargo
    FT_S = 10000000.0         # FILETIME ticks in a second
    SHIP_NAME = 'tayra'
    LINK_NAME = 'entosis link ii'
    BLOCK_NAME = 'nitrogen fuel block'
    STRONT_NAME = 'strontium clathrates'
    BLOCK_UNITS = 1400        # 5 m3 each; two /createitem calls (the cap is 1000 a call)
    STRONT_UNITS = 40
    STAGE_MS = 120000
    TIME_SCALE = 60           # the server's --time-scale (the most it takes)

    def filetime_now():
        n = now_ms()
        return None if n is None else n * 10000.0

    def cargo_units(type_id):
        ok, rows = ship_items(FLAG_CARGO)
        if not ok:
            return None
        total = 0
        for r in (rows or []):
            if getattr_safe(r, 'typeID') == type_id:
                total += int(getattr_safe(r, 'stacksize', None) or getattr_safe(r, 'quantity', 0) or 0)
        return total

    def hangar_units(type_id):
        ok, rows = hangar_items()
        if not ok:
            return None
        total = 0
        for r in (rows or []):
            if getattr_safe(r, 'typeID') == type_id:
                total += int(getattr_safe(r, 'stacksize', None) or getattr_safe(r, 'quantity', 0) or 0)
        return total

    def stow(type_id, units):
        """Move `units` of a hangar type into the ship's cargo hold by the
        client's own inventory `Add`, stack by stack.  `(moved, detail)`."""
        ok, rows = hangar_items()
        if not ok:
            return 0, 'hangar unreadable: %s' % text(rows)
        moved = 0
        notes = []
        for r in (rows or []):
            if getattr_safe(r, 'typeID') != type_id or moved >= units:
                continue
            have = int(getattr_safe(r, 'stacksize', None) or getattr_safe(r, 'quantity', 0) or 0)
            take = min(have, units - moved)
            if take <= 0:
                continue
            # A race, not a refusal (runs w29a/w30a): every move makes the
            # server push `OnInvalidateCacheForLocations([station, ship])`
            # after its reply (`Registry::apply_items`) [V: client.log w30a].
            # The next stack's `GetInventoryFromId(ship)` builds a fresh
            # `invCacheContainer`, and its `Add` reads `self.typeID`, which
            # blocks on `GetSelfInvItem` (invCache.py:936-943, :978); the
            # queued push runs `InvalidateLocationCache` meanwhile, which sets
            # that container's `moniker = None` (invCache.py:582-588), and
            # `self.moniker.Add` (invCache.py:983) raises
            # `AttributeError: 'NoneType' object has no attribute 'Add'`
            # before anything is sent [V: w30a client.log, TryLockItem then
            # UnlockItem for 100000007 with no Add on the wire].  So: wait for
            # the previous stack to land (its pushes drain first), resolve the
            # container's item before `Add`, and on that AttributeError build
            # a fresh container and try again.
            # None when the hold is unreadable (the same race): then the
            # 'already landed' shortcut below is off and only a retry counts
            before = cargo_units(type_id)
            ok2, v = False, None
            for _attempt in range(6):
                okg, inv = safe(lambda: get_svc('invCache').GetInventoryFromId(
                    builtin('session').shipid))
                if not okg:
                    ok2, v = False, inv
                    break
                if inv is None:
                    ok2, v = False, 'ship inventory not bound yet'
                    sleep_ms(2000)
                    continue
                # the blocking `GetSelfInvItem`, outside `Add`
                safe(lambda inv=inv: inv.typeID)
                if getattr_safe(inv, 'moniker') is None:
                    ok2, v = False, 'ship inventory invalidated before Add'
                    sleep_ms(1000)
                    continue
                ok2, v = safe(lambda r=r, take=take, inv=inv: inv.Add(
                    r.itemID, r.locationID, qty=take, flag=FLAG_CARGO))
                if ok2 or "no attribute 'Add'" not in text(v):
                    break
                # raised before the call left the client; still, never move twice
                if before is not None and (cargo_units(type_id) or 0) >= before + take:
                    ok2, v = True, None
                    break
                sleep_ms(1000)
            if not ok2:
                notes.append(text(v)[:120])
                continue
            moved += take
            landed, _w = wait_for(lambda: (cargo_units(type_id) or 0) >= (before or 0) + take,
                                  timeout_ms=10000)
            if not landed:
                notes.append('stack of %d not seen in the hold after 10 s' % take)
        return moved, 'moved %d of %d; %s' % (moved, units, text(notes))

    def session_gate(timeout_ms=45000):
        """Wait for the client's session-change lockout to run out
        (carbon/common/script/sys/sessions.py:430-441 [V]: a change inside it
        is refused with `PerformSessionChangeInProgress`)."""
        import blue
        got, _w = wait_for(lambda: (getattr_safe(builtin('session'), 'nextSessionChange') or 0)
                           <= blue.os.GetSimTime(), timeout_ms=timeout_ms, step_ms=500)
        return bool(got)

    last_tr = []

    def go_to(system):
        """Teleport and wait for the session to change (the pilot lands in
        space).  The GM reply is kept in `last_tr`."""
        for attempt in (1, 2):
            session_gate()
            ok, reply = gm('/tr me %d' % system)
            got, _w = wait_for(
                lambda: getattr_safe(builtin('session'), 'solarsystemid2') == system,
                timeout_ms=45000, step_ms=500)
            del last_tr[:]
            last_tr.append('attempt %d: /tr me %d -> %s; now in %s' % (
                attempt, system, text(reply)[:160],
                text(getattr_safe(builtin('session'), 'solarsystemid2'))))
            if got:
                return True
            sleep_ms(10000)
        return False

    def park_now():
        mich = get_michelle()
        return mich.GetBallpark() if mich is not None else None

    def balls_of_group(group):
        park = park_now()
        if park is None:
            return []
        return slims_where(park, lambda s: getattr_safe(s, 'groupID') == group)

    # -- the client's sov service ------------------------------------------

    def sov_rows(system):
        """The structure list of `system` as the client's `svc.sov` holds it:
        pushed by `OnSolarSystemSovStructuresUpdated`, else fetched (cached
        30 s) [V: sovSvc.py:89-123]."""
        svc = get_svc('sov')
        if svc is None:
            return None
        ok, v = safe(lambda: svc.GetSovStructuresInfoForSolarSystem(system))
        return rows_of(v) if ok else None

    def hub_row(system):
        rows = sov_rows(system)
        return rows[0] if rows else None

    def campaign_of(row):
        """`(eventType, defender, startTime, {team: score})` or None."""
        cs = getattr_safe(row, 'campaignState') if row is not None else None
        if not cs:
            return None
        try:
            scores = {}
            for k in cs[3].keys():
                scores[int(k)] = float(cs[3][k])
            return (int(cs[0]), int(cs[1]), int(cs[2]), scores)
        except Exception:
            return None

    def window_of(row):
        vs = getattr_safe(row, 'vulnerabilityState') if row is not None else None
        if not vs:
            return None
        try:
            return (int(vs[0]), int(vs[1]))
        except Exception:
            return None

    def total_of(scores):
        t = 0.0
        for k in scores:
            t += scores[k]
        return t

    def entosis_of(ball_id):
        """The slim's `entosis_score` as `(controlling, scoring, (s0, t0, rate,
        blocked), defender)` or None [V: entosisCaptureTarget.py:40]."""
        park = park_now()
        if park is None:
            return None
        slim = park.slimItems.get(ball_id)
        es = getattr_safe(slim, 'entosis_score') if slim is not None else None
        if not es:
            return None
        try:
            return (es[0], es[1], tuple(es[2]) if es[2] else None, es[3])
        except Exception:
            return None

    def rate_of(es):
        try:
            return float(es[2][2])
        except Exception:
            return None

    def module_stopped(item_id):
        return module_active(item_id) is False

    # -- the two alliances ---------------------------------------------------

    def found_alliance(ticker, label):
        """`/devcorp <ticker>` makes the character CEO of a new corporation, a
        Master Wallet deposit and the ordinary `CreateAlliance` found an
        alliance round it.  `(corp, alliance, detail)`."""
        before = getattr_safe(builtin('session'), 'corpid')
        ok, reply = gm('/devcorp ' + ticker)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') != before,
                           timeout_ms=30000)
        corp = getattr_safe(builtin('session'), 'corpid')
        if not got:
            return None, None, '/devcorp %s -> %s; corpid never changed' % (ticker, reply[:200])
        if getattr_safe(builtin('session'), 'allianceid'):
            return corp, None, 'a new corporation already has alliance %s' % text(
                getattr_safe(builtin('session'), 'allianceid'))
        gm('/giveisk 3000000000')
        ok, v = rcall_kw('account', 'GiveCash', (corp, 1000000000.0, 'fee'),
                         {'toAccountKey': 1000})
        if not ok:
            return corp, None, 'Master Wallet deposit: ' + text(v)
        ok, v = rcall('corpRegistry', 'CreateAlliance', 'Fci Camp ' + label + ' ' + ticker,
                      label + ticker[-3:], 'f-ci', '')
        if not ok:
            return corp, None, 'CreateAlliance: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'allianceid'),
                           timeout_ms=30000)
        return corp, got, ('founded alliance %s' % text(got)) if got else 'allianceid never arrived'

    def switch_to(ticker, alliance, timeout_ms=30000):
        """`/devcorp <existing ticker>` joins that corporation; wait for the
        session to carry its alliance.  `(ok, detail)`."""
        ok, reply = gm('/devcorp ' + ticker)
        got, waited = wait_for(
            lambda: getattr_safe(builtin('session'), 'allianceid') == alliance,
            timeout_ms=timeout_ms)
        return bool(got), '/devcorp %s -> %s; session.allianceid %s after %s ms' % (
            ticker, reply[:100], text(getattr_safe(builtin('session'), 'allianceid')), text(waited))

    # -- the scenario --------------------------------------------------------

    def eoci_body():
        mark = log_mark()
        sess0 = builtin('session')
        fresh = not getattr_safe(sess0, 'allianceid')
        precondition('the character starts without an alliance (a fresh dev world)', fresh,
                     'session.allianceid %s' % text(getattr_safe(sess0, 'allianceid')))
        if not fresh:
            return
        gm('/giveskill Corporation Management 1')
        sfx = uniq(3)
        tick_a, tick_b = 'S' + sfx + 'A', 'S' + sfx + 'B'
        corp_a, alliance_a, detail = found_alliance(tick_a, 'A')
        precondition('alliance A (the defender) is founded', bool(alliance_a), detail)
        if not alliance_a:
            return
        corp_b, alliance_b, detail = found_alliance(tick_b, 'B')
        precondition('alliance B (the attacker) is founded', bool(alliance_b), detail)
        if not alliance_b:
            return
        precondition('the two alliances differ', alliance_a != alliance_b,
                     '%s %s' % (text(alliance_a), text(alliance_b)))
        if alliance_a == alliance_b:
            return

        # --- the ship, the link and the fuel (docked) --------------------
        ship = type_ids_named(SHIP_NAME, exact=True)
        link = type_ids_named(LINK_NAME, exact=True)
        block = type_ids_named(BLOCK_NAME, exact=True)
        stront = type_ids_named(STRONT_NAME, exact=True)
        have_types = bool(ship and link and block and stront)
        precondition('the client knows the hull, the link and the fuels', have_types,
                     'ship=%s link=%s block=%s stront=%s' % (
                         text(ship), text(link), text(block), text(stront)))
        if not have_types:
            return
        for line in ('/createitem %d' % ship[0], '/createitem %d' % link[0],
                     '/createitem %d 700' % block[0], '/createitem %d 700' % block[0],
                     '/createitem %d %d' % (stront[0], STRONT_UNITS)):
            gm_check('GM: ' + line, line, expect=['created'])
        wait_for(lambda: (hangar_units(block[0]) or 0) >= BLOCK_UNITS, timeout_ms=20000)
        ok, detail = board_ship(ship[0])
        check('boarded the Tayra', ok, detail)
        if not ok:
            return
        ok, detail = fit_module(link[0], FLAG_HI0)
        check('the Entosis Link fits in a high slot', ok, detail)
        m = module_item(link[0], FLAG_HI0)
        check('the fitted link is in its slot', m is not None, 'module_item found none at the flag')
        if m is None:
            return
        ok, detail = online_module(m.itemID)
        check('the link onlines', ok, detail)
        if not ok:
            return
        moved_b, d_b = stow(block[0], BLOCK_UNITS)
        moved_s, d_s = stow(stront[0], STRONT_UNITS)
        wait_for(lambda: (cargo_units(block[0]) or 0) >= BLOCK_UNITS
                 and (cargo_units(stront[0]) or 0) >= STRONT_UNITS, timeout_ms=20000)
        blocks_aboard = cargo_units(block[0])
        stront_aboard = cargo_units(stront[0])
        check('the fuel blocks and the strontium are in the cargo hold',
              (blocks_aboard or 0) >= BLOCK_UNITS and (stront_aboard or 0) >= STRONT_UNITS,
              'blocks %s (%s); strontium %s (%s)' % (
                  text(blocks_aboard), d_b, text(stront_aboard), d_s))
        if (blocks_aboard or 0) < BLOCK_UNITS or (stront_aboard or 0) < STRONT_UNITS:
            return
        cycle_ms = getattr_safe(get_svc('godma').GetItem(m.itemID), 'duration') or STAGE_MS
        cycle_s = float(cycle_ms) / 1000.0

        # --- into space, to a claimable null-sec system --------------------
        undocked, detail = undock()
        precondition('undocked into space', undocked, detail)
        if not undocked:
            return
        home = getattr_safe(builtin('session'), 'solarsystemid2')
        pinned = params().get('sov_system')
        cands = [int(pinned)] if pinned else map_systems(
            lambda row: float(row.securityStatus) <= 0.0 and not getattr_safe(row, 'factionID'),
            limit=12)
        cands = [c for c in cands if c != home]
        precondition('null-sec NPC-free systems other than home', bool(cands),
                     '%d candidate(s)' % len(cands))
        if not cands:
            return
        # the first session change after login/undock needs its lockout out
        session_gate()
        sleep_ms(4000)
        system = None
        claim_reply = ''
        for cand in cands[:4]:
            arrived = go_to(cand)
            precondition('arrived in space in candidate system %s' % text(cand), arrived,
                         text(last_tr))
            if not arrived:
                continue
            ok, reply = switch_to(tick_a, alliance_a)
            precondition('pilot is in alliance A (the defender)', ok, reply)
            if not ok:
                return
            ok, claim_reply = gm('/sov set here %d' % alliance_a)
            if ok and 'not claimable' not in claim_reply and 'usage' not in claim_reply.lower():
                system = cand
                break
        check('/sov set here gives alliance A a system with a hub', system is not None,
              claim_reply[:240])
        if system is None:
            return

        # --- round 1: the defender and the hub -------------------------------
        hubs, _w = wait_for(lambda: balls_of_group(GROUP_HUB), timeout_ms=60000, step_ms=1000)
        precondition('the hub is a ball of its group in the park', bool(hubs),
                     '%d hub ball(s)' % len(hubs or []))
        if not hubs:
            return
        hub_ball = hubs[0][0]
        got, waited = wait_for(lambda: getattr_safe(hub_row(system), 'allianceID') == alliance_a,
                               timeout_ms=45000, step_ms=2000)
        row = hub_row(system)
        check('the client sov list names A on the hub, and the row is the hub ball',
              bool(got) and getattr_safe(row, 'itemID') == hub_ball,
              'allianceID %s itemID %s (ball %s) after %s ms' % (
                  text(getattr_safe(row, 'allianceID')), text(getattr_safe(row, 'itemID')),
                  text(hub_ball), text(waited)))
        check('before any attack the hub row carries no campaign and no entosis score',
              campaign_of(row) is None and entosis_of(hub_ball) is None,
              'campaignState %s entosis_score %s' % (
                  text(getattr_safe(row, 'campaignState')), text(entosis_of(hub_ball))))

        park = park_now()
        # A target inside the warp floor is refused, not flown ('You cannot
        # warp to a destination closer than 150 km.' [V: run w30b]); the GM
        # landing can put the hub that close, and then the approach does it,
        # given time for the distance at a slow hull's speed (127 m/s seen in
        # w30c on the Tayra; 100 m/s [A] here, falsified by a timeout).
        start = dist_to(park, hub_ball)
        if start is not None and start < 150000.0:
            ok, detail = True, 'already %s m from the hub, inside the warp floor; no warp' % fmt(start, 0)
        else:
            ok, detail = warp_to(park, hub_ball, min_range=1000.0)
        precondition('warped to the hub', ok, detail)
        if not ok:
            return
        left = dist_to(park, hub_ball) or 0.0
        near, detail = approach(park, hub_ball, 1500,
                                timeout_ms=max(200000, int(left / 100.0 * 1000) + 60000))
        precondition('beside the hub (fuel transfers within 2.5 km of its surface)', near, detail)
        if not near:
            return
        ok, reply = gm('/sovfuel all')
        emptied, _w = wait_for(lambda: cargo_units(block[0]) == 0, timeout_ms=20000)
        check('/sovfuel all puts the fuel blocks into the hub (the hold is empty)',
              bool(emptied) and 'no fuel' not in reply.lower(),
              '%s; blocks in the hold now %s' % (reply[:200], text(cargo_units(block[0]))))

        # --- round 2: the attacker links the hub ------------------------------
        ok, reply = switch_to(tick_b, alliance_b)
        precondition('pilot is in alliance B (the attacker)', ok, reply)
        if not ok:
            return
        locked, detail = lock_target(hub_ball, timeout_ms=90000)
        precondition('the hub is locked', locked, detail)
        if not locked:
            return
        need_s = cycle_s + 75.0
        # Open the window now instead of waiting for the day's: `/sov prime`
        # sets the alliance's prime hour at once.  The hour is read off the
        # client's clock for the compressed day [A: TIME_SCALE below is the
        # server's --time-scale; falsifier: the window wait times out].  The
        # window is centred on the middle of the prime hour, so priming the
        # hour it is now leaves as little as half a window minus half an hour
        # (run w30f: a 6 game hour window, 150 wall s left, the warm-up needs
        # 195): prime two hours ahead, so now is inside and at least
        # PRIME_AHEAD + 0.5 game hours of the window's second half is left
        # [A: a window of at least 2 x PRIME_AHEAD game hours; falsifier: the
        # window wait below times out].
        PRIME_AHEAD = 2
        day_ft = 86400.0 / TIME_SCALE * FT_S
        n0 = filetime_now() or 0.0
        prime_hour = (int((n0 % day_ft) / day_ft * 24.0) + PRIME_AHEAD) % 24
        ok, reply = gm('/sov prime %d %d' % (alliance_a, prime_hour))
        check('/sov prime moves the defender\'s window around the hour it is now',
              ok and 'prime hour' in reply, reply[:200])

        def open_window():
            r = hub_row(system)
            w = window_of(r)
            n = filetime_now()
            if w is None or n is None:
                return None
            if w[0] <= n and (w[1] - n) / FT_S >= need_s:
                return w
            return None
        win, waited = wait_for(open_window, timeout_ms=300000, step_ms=2000)
        precondition('the hub row shows an open vulnerability window with room for the warm-up '
                     '(%d s)' % int(need_s), bool(win),
                     'window %s after %s ms; row %s; now %s' % (
                         text(window_of(hub_row(system))), text(waited),
                         text(getattr_safe(hub_row(system), 'vulnerabilityState')),
                         text(filetime_now())))
        if not win:
            gm('/sov window here')
            return
        ok, reply = gm('/sov window here')
        check('the server agrees the hub is vulnerable', 'vulnerable' in reply.lower(), reply[:240])
        t_click = filetime_now()
        ok, detail = activate_module(m.itemID, target_id=hub_ball)
        check('the Entosis Link activates on the hub', ok, detail)
        if not ok:
            return
        # the scoring phase of a hub lasts 595 s / scale (3 s): sample fast
        scoring, waited = wait_for(
            lambda: (entosis_of(hub_ball) or (0, 0, None, 0))[2] and entosis_of(hub_ball),
            timeout_ms=int(2 * cycle_ms + 60000), step_ms=250)
        es = scoring or entosis_of(hub_ball)
        check('after the warm-up the hub slim names the attacking team scoring against defender A',
              bool(scoring) and es is not None and es[1] == -alliance_a and es[3] == alliance_a,
              'entosis_score %s (A=%s) after %s ms' % (text(es), text(alliance_a), text(waited)))
        check('the client extrapolates a rising, unblocked score',
              es is not None and (rate_of(es) or 0.0) > 0.0 and not (es[2] and es[2][3]),
              'entosis_score %s' % text(es))

        def reinforced():
            c = campaign_of(hub_row(system))
            return c
        camp, waited = wait_for(reinforced, timeout_ms=int(2 * cycle_ms + 90000), step_ms=2000)
        row = hub_row(system)
        check('the hub is reinforced: the client list carries a campaign of the hub-defence type '
              'for defender A', bool(camp) and camp[0] == EVENT_HUB and camp[1] == alliance_a,
              'campaignState %s after %s ms' % (text(getattr_safe(row, 'campaignState')), text(waited)))
        if not camp:
            gm('/sov entosis here')
            no_new_tracebacks('sov campaign, no reinforcement', mark)
            return
        start_ft = camp[2]
        check('the campaign start time is in the future', start_ft > (filetime_now() or 0),
              'start %s now %s' % (text(start_ft), text(filetime_now())))
        check('the bar has the defender and everyone-but-defender teams, summing to one',
              sorted(camp[3].keys()) == sorted([alliance_a, -alliance_a])
              and abs(total_of(camp[3]) - 1.0) < 0.001 and min(camp[3].values()) > 0.0,
              'scores %s' % text(camp[3]))
        check('a reinforced hub shows no vulnerability window', window_of(row) is None,
              'vulnerabilityState %s' % text(getattr_safe(row, 'vulnerabilityState')))
        ok, reply = gm('/sov entosis here')
        check('the server calls the hub reinforced', 'reinforced' in reply.lower(), reply[:240])
        stopped, waited = wait_for(lambda: module_stopped(m.itemID), timeout_ms=30000, step_ms=1000)
        check('the link stopped when the hub was reinforced', bool(stopped),
              'module_active %s after %s ms' % (text(module_active(m.itemID)), text(waited)))
        ok, detail = activate_module(m.itemID, target_id=hub_ball)
        sleep_ms(8000)
        check('a link started on the reinforced hub does not run',
              module_active(m.itemID) is not True,
              'activate: %s; module_active %s' % (detail[:120], text(module_active(m.itemID))))
        es = entosis_of(hub_ball)
        check('the hub slim no longer scores (idle tuple or none)',
              es is None or (rate_of(es) or 0.0) == 0.0, 'entosis_score %s' % text(es))

        # --- round 3: the campaign starts ------------------------------------
        # The real delay is 48 game hours = 48 minutes at the server's largest
        # scale: too long for a bounded run.  So the link's reinforcement is
        # proved above (campaign scheduled, start time 48 game hours away),
        # then the GM ends it (`/sov resolve here defenders`: the hub is
        # claimed again, the client list drops the campaign) and starts a
        # campaign for now (`/sov reinforce here now`).  What is NOT proved
        # on a client: that the tick begins a campaign at its scheduled time
        # (the same `begin` runs here, only the start time is now).
        ok, reply = gm('/sov resolve here defenders')
        check('GM ends the reinforcement (defenders hold)', ok and 'resolved' in reply, reply[:200])
        cleared, waited = wait_for(lambda: campaign_of(hub_row(system)) is None
                                   and getattr_safe(hub_row(system), 'allianceID') == alliance_a,
                                   timeout_ms=60000, step_ms=2000)
        check('the client list drops the campaign and still names A', bool(cleared),
              'campaignState %s after %s ms' % (
                  text(getattr_safe(hub_row(system), 'campaignState')), text(waited)))
        ok, reply = gm('/sov reinforce here now')
        started = ok and 'reinforced: campaign starts' in reply
        check('GM reinforces the hub with a campaign for now', started, reply[:200])
        if not started:
            # "hub N is already reinforced" and the like: no campaign will
            # start, so waiting for nodes would only run into the timeout.
            no_new_tracebacks('sov campaign, reinforce refused', mark)
            return
        fresh, waited = wait_for(lambda: campaign_of(hub_row(system)), timeout_ms=60000, step_ms=2000)
        start_ft = fresh[2] if fresh else start_ft
        wait_ms = int(max(0.0, (start_ft - (filetime_now() or start_ft)) / FT_S) * 1000.0) + 150000
        nodes, waited = wait_for(lambda: balls_of_group(GROUP_NODE), timeout_ms=wait_ms, step_ms=3000)
        check('the campaign begins: command nodes appear in the park', bool(nodes),
              '%d node ball(s) after %s ms (waited up to %s)' % (len(nodes or []), text(waited), text(wait_ms)))
        if not nodes:
            gm('/sov nodes here')
            no_new_tracebacks('sov campaign, no nodes', mark)
            return
        infos = []
        for ball, slim in nodes:
            infos.append(getattr_safe(slim, 'campaign_sourceInfo'))
        sound = True
        for info in infos:
            try:
                sound = sound and info[0] == system and info[1] == EVENT_HUB \
                    and info[2] == hub_ball and info[3] == alliance_a
            except Exception:
                sound = False
        check('every node names the hub system, the event type, the hub and defender A',
              sound, 'campaign_sourceInfo %s (system %s hub %s A %s)' % (
                  text(infos[:3]), text(system), text(hub_ball), text(alliance_a)))
        row = hub_row(system)
        c2 = campaign_of(row)
        check('during the fight the hub row still carries the campaign, with the same start',
              c2 is not None and c2[2] == start_ft and c2[1] == alliance_a,
              'campaignState %s' % text(getattr_safe(row, 'campaignState')))
        ok, reply = gm('/sov entosis here')
        check('the server calls the hub in its node fight', 'node fight' in reply.lower(), reply[:240])
        ok, reply = gm('/sov nodes here')
        check('the server lists the nodes of the campaign', 'campaign on hub' in reply, reply[:300])

        # --- round 4: a real link on a node ---------------------------------
        bar_before = (c2 or camp)[3]
        park = park_now()
        node_ball = sorted([(dist_to(park, b) or 1e18, b) for b, _s in nodes])[0][1]
        ok, detail = warp_to(park, node_ball, min_range=10000.0)
        precondition('warped to the nearest command node', ok, detail)
        if not ok:
            return
        d = dist_to(park, node_ball)
        if d is None or d > 30000:
            near, detail = approach(park, node_ball, 30000)
            precondition('within the link\'s range of the node', near, detail)
            if not near:
                return
        locked, detail = lock_target(node_ball, timeout_ms=90000)
        precondition('the node is locked', locked, detail)
        if not locked:
            return
        # The link scores only from its second cycle: the first is the
        # warm-up.  So the capacitor must pay the first cycle now *and* the
        # second at the warm-up's end.  Run w30h started the link at 505 GJ
        # (need 500) after the hub's cycles and the warp had drained it; 120 s
        # of recharge brought it to 483 GJ and the server stopped the link at
        # the boundary with NotEnoughEnergy (need 500, got 483) [V: the stop
        # push in that run's client.log] -- before it ever scored.  Wait for a
        # charge that recharges to `need` within one cycle after paying it,
        # by godma's own charge curve (modules.rs `recharged`:
        # (1 + (sqrt(c/cap) - 1) * exp(-t/tau))^2 * cap, tau = rechargeRate/5)
        # [I], plus a margin.  There is no GM refill.
        sid = builtin('session').shipid

        def ship_attr(name):
            return getattr_safe(get_svc('godma').GetItem(sid), name)

        need = float(getattr_safe(get_svc('godma').GetItem(m.itemID), 'capacitorNeed') or 0.0)
        cap = float(ship_attr('capacitorCapacity') or 0.0)
        tau_ms = float(ship_attr('rechargeRate') or 0.0) / 5.0
        if need > 0.0 and cap > 0.0 and tau_ms > 0.0:
            import math
            k = math.exp(-float(cycle_ms) / tau_ms)
            s = 1.0 - (1.0 - math.sqrt(min(need / cap, 1.0))) / k
            start = need + (cap * s * s if s > 0.0 else 0.0)
            precondition('the hull\'s capacitor can pay the link\'s warm-up and its first scoring cycle',
                         start <= cap, 'start at %s GJ of %s (need %s per %s ms cycle, tau %s ms)' % (
                             fmt(start, 0), fmt(cap, 0), fmt(need, 0), text(cycle_ms), fmt(tau_ms, 0)))
            if start > cap:
                return
            want = min(start + 0.02 * cap, 0.995 * cap)
        else:
            # [A] godma does not name the numbers: wait for a full capacitor.
            want = 0.995 * cap if cap > 0.0 else 0.0
        full, waited = wait_for(lambda: float(ship_attr('charge') or 0.0) >= want,
                                timeout_ms=600000, step_ms=2000)
        precondition('the capacitor holds enough for two link cycles', bool(full),
                     '%s GJ (want %s, need %s per cycle) after %s ms' % (
                         fmt(ship_attr('charge'), 0), fmt(want, 0), fmt(need, 0), text(waited)))
        if not full:
            return
        ok, detail = activate_module(m.itemID, target_id=node_ball)
        check('the Entosis Link activates on the node', ok, detail)
        if not ok:
            return
        # a node is taken 1 s after it starts scoring: sample fast, or see it gone
        scored, waited = wait_for(
            lambda: entosis_of(node_ball)
            or not any(b == node_ball for b, _s in balls_of_group(GROUP_NODE)),
            timeout_ms=int(2 * cycle_ms + 60000), step_ms=250)
        es = entosis_of(node_ball)
        check('the node slim shows the attackers scoring (everyone but A), or the node is already taken',
              (es is not None and es[1] == -alliance_a)
              or not any(b == node_ball for b, _s in balls_of_group(GROUP_NODE)),
              'entosis_score %s after %s ms; link active %s, capacitor %s GJ' % (
                  text(es), text(waited), text(module_active(m.itemID)),
                  fmt(ship_attr('charge'), 0)))
        gone, waited = wait_for(
            lambda: not any(b == node_ball for b, _s in balls_of_group(GROUP_NODE)),
            timeout_ms=int(2 * cycle_ms + 90000), step_ms=2000)
        check('the captured node leaves the park', bool(gone), 'after %s ms' % text(waited))

        def bar_moved():
            c = campaign_of(hub_row(system))
            if c is None:
                return None
            if c[3].get(alliance_a, 0.0) < bar_before.get(alliance_a, 0.0) - 1e-6:
                return c
            return None
        moved, waited = wait_for(bar_moved, timeout_ms=60000, step_ms=2000)
        c3 = campaign_of(hub_row(system))
        shares = c3[3] if c3 else {}
        check('the bar the client draws moved: the defender\'s share fell', bool(moved),
              'before %s after %s (%s ms)' % (text(bar_before), text(shares), text(waited)))
        key_b = shares.get(alliance_b, 0.0)
        check('attacking alliance B has a share of its own', key_b > 0.0, 'bar %s' % text(shares))
        check('the shares still sum to one', abs(total_of(shares) - 1.0) < 0.001,
              'bar %s' % text(shares))
        check('the link stopped when the node was taken',
              bool(wait_for(lambda: module_stopped(m.itemID), timeout_ms=30000, step_ms=1000)[0]),
              'module_active %s' % text(module_active(m.itemID)))

        # --- round 5: the end of the fight -----------------------------------
        # The node ids are only the part after "nodes:": the line's head,
        # "campaign on hub <hub> in <system> (fighting)", has the same shape,
        # and run w30h captured the hub id 14 times ("no node ...") once the
        # standing nodes were gone.  With none standing, the GM makes the next
        # one instead of waiting half a game hour for the wave.
        import re
        rounds = 0
        while rounds < 24:
            rounds += 1
            ok, listing = gm('/sov nodes here')
            if 'no campaign' in listing:
                break
            ids = re.findall(r'(\d+) in \d+ \(', listing.split('nodes:', 1)[1]) \
                if 'nodes:' in listing else []
            if not ids:
                gm('/sov nodes spawn here')
                sleep_ms(1000)
                continue
            for nid in ids[:3]:
                gm('/sov nodes capture %s attackers' % nid)
            ok, info = gm('/sov info here')
            if 'no hub' in info:
                break
        ok, info = gm('/sov info here')
        check('the server: the attackers\' captures took the hub (no hub in the system)',
              'no hub' in info, info[:240])
        empty, waited = wait_for(lambda: sov_rows(system) == [], timeout_ms=90000, step_ms=2000)
        check('the client sov list for the system is empty once the hub fell', bool(empty),
              'rows %s after %s ms' % (text(sov_rows(system)), text(waited)))
        gone, waited = wait_for(lambda: not balls_of_group(GROUP_HUB), timeout_ms=60000, step_ms=2000)
        check('the hub ball left the park', bool(gone), '%d hub ball(s)' % len(balls_of_group(GROUP_HUB)))
        gone, waited = wait_for(lambda: not balls_of_group(GROUP_NODE), timeout_ms=60000, step_ms=2000)
        check('the remaining command nodes left the park', bool(gone),
              '%d node ball(s)' % len(balls_of_group(GROUP_NODE)))
        no_new_tracebacks('sov campaign', mark)

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
