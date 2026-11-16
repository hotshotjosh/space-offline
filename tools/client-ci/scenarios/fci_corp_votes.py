# -*- coding: utf-8 -*-
"""Corporation votes: a Director proposes, two shareholders vote by weight, a repeat is refused (issue #103).

    THIS FILE RUNS INSIDE THE REAL EVE CLIENT, under Stackless Python 2.7.
    Nothing in the server build imports it.  Lane `corp-votes`.

Two clients: `tools/client-ci.sh run fci_corp_votes --with-second`.  The pilot
with the lower character id is A (founder and CEO), the other B (invited and
joined as a plain member).  Everything goes through the client's own `corpvotes`
service (`services/corporation/voting.py`), the calls the Votes panels and
the vote wizard make:

0. A founds a corporation, invites B and gives B a quarter of the shares;
1. A and B both see `CanViewVotes` true; B, holding shares, `CanVote`;
2. B, a plain member, is refused `InsertVoteCase` by the server (only a
   Director proposes) and nothing opens;
3. A proposes a general vote with two options (`CreateVote`, the call the
   wizard's OK makes); both clients' `corpvotes` caches refill after the
   client's own `OnVoteCaseChanged` and list one open case, no closed one;
4. A votes for option 0 and B for option 1 (`InsertVote`); each reads its
   own vote back (`GetVotes`); a second vote from either is refused and the
   tally does not move;
5. the options' `votesFor` are the shares the SERVER reports each holder
   keeps, summed per option (computed from `GetShareholders`, never from an
   assumed founding grant); both clients see both.

Nothing here asserts a duration: every wait is a bound.  No client log
traceback may appear.  A fresh world is a PRECONDITION: A starts in an NPC
corporation.

6. (#153) B runs for CEO (`CreateVote` with the CEO type): A sees the case, its
   option 0 carries B as the candidate, A and B both vote for it, and A is
   still the CEO while the vote is open.  A then closes the vote with the GM
   line `/vote close <id>` (the shortest vote runs a day and no client call
   shortens it), one `voteManager` call settles it, and B is the CEO.

7. (#153) B, now the CEO, proposes an expulsion vote (kick type) naming A, who
   holds shares and does not vote.  B sees the case open with A as option 0's
   parameter (A may already be out when its step starts).  B votes yes, `/vote close`, and `GetCorpSanctionedActions` lists
   the case (kick type, parameter A, not acted upon).  `ActivateSanctionedAction`
   puts it in effect: A's `session.corpid` leaves the corporation (where A lands
   is read, not pinned), the row reads acted upon, and a second Activate is
   refused ("already been put in effect").

NOT here: the expiry by the clock and the other won vote types (unit-tested
in `corp_votes.rs` and `service/corp_votes.rs`), the Votes panels on screen
(a human check).
[A]s: `GetVoteCasesByCorporation(corp, STATUS_OPEN)` is a rowset the client
can `len()`; falsifier: the open-case check fails with a TypeError detail.
"""

EOCI_SCENARIO = 'fci_corp_votes'

# Seconds to pass to `tools/client-ci.sh run fci_corp_votes --timeout`.
EOCI_FCI_TIMEOUT_S = 1200

# SERVER-ARGS: --character Alice --character Bob
# SERVER-EXPECT: absent  UNANSWERED (voteManager|corpRegistry)
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


    # The harness sets this to the OTHER client's character id
    # (`tools/client-ci.sh`: `^ *EXPECT_CHARID *= *0 *$`).
    EXPECT_CHARID = 0
    PEER_WAIT_MS = 300000           # a bound on the other client's boot, never a claim
    STEP_WAIT_MS = 180000           # a bound on one step crossing between the clients
    LINGER_MS = 20000

    STATUS_CLOSED = 1               # voting.py VOTECASE_STATUS_* [V]
    STATUS_OPEN = 2
    VOTE_CEO = 0                    # voting.py voteCEO [V]
    VOTE_KICK = 3                   # voting.py voteKickMember [V]
    VOTE_GENERAL = 4                # voting.py voteGeneral [V]

    # -- the client's services ----------------------------------------------

    def corp_svc():
        """`svc.corp` (`base_corporation`) [V]."""
        return get_svc('corp')

    def votes_svc():
        """`svc.corpvotes` (`services/corporation/voting.py`) [V]."""
        return get_svc('corpvotes')

    def holders(corp):
        """`{shareholderID: shares}` from the SERVER's `GetShareholders`,
        read through the client's own service, or None when it raised."""
        ok, rows = safe(lambda: corp_svc().GetShareholders())
        if not ok:
            return None
        got = {}
        for r in list(rows.itervalues()):
            got[r.shareholderID] = r.shares
        return got

    def my_shares(corp):
        ok, idx = safe(lambda: corp_svc().GetSharesByShareholder())
        if not ok:
            return None
        row = idx.get(corp) if hasattr(idx, 'get') else None
        return row.shares if row is not None else 0

    def corp_of(charid):
        ok, info = rcall('charMgr', 'GetPublicInfo', charid)
        return getattr_safe(info, 'corporationID') if ok else None

    def join_corp_of_issuer(me):
        """B: take the invitation A sent (status 8) and accept it (status 2)."""
        def invited():
            ok, apps = rcall('corpRegistry', 'GetMyApplications')
            if not ok:
                return None
            for corpid in list(apps.keys()):
                for a in apps[corpid]:
                    if a.status == 8:
                        return a
            return None
        app, waited = wait_for(invited, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        if not app:
            return None, 'no invitation after %s ms' % text(waited)
        ok, v = rcall('corpRegistry', 'UpdateApplicationOffer', app.applicationID, me,
                      app.corporationID, '', 2)
        if not ok:
            return None, 'UpdateApplicationOffer: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') == app.corporationID,
                           timeout_ms=30000)
        return (app.corporationID, 'joined') if got else (None, 'session.corpid never became %s' % text(app.corporationID))

    def found_corp():
        """`(corp_id, detail)`: a corporation A is CEO of."""
        gm('/giveisk 3000000000')
        gm('/giveskill Corporation Management 1')
        sfx = uniq(4)
        old = getattr_safe(builtin('session'), 'corpid')
        ok, v = rcall('corpRegistry', 'AddCorporation', 'Fci Votes ' + sfx, 'V' + sfx,
                      'f-ci', '', 0.0, 1, 1, 1, 1, 1, 1, True, False, 0.0)
        if not ok:
            return None, 'AddCorporation: ' + text(v)
        got, _w = wait_for(lambda: getattr_safe(builtin('session'), 'corpid') != old,
                           timeout_ms=30000)
        if not got:
            return None, 'session.corpid never changed from %s' % text(old)
        return getattr_safe(builtin('session'), 'corpid'), 'founded'

    # -- the votes, as the client's own service reads them -------------------

    def open_cases(corp):
        """The open vote cases as a list of rows, or None when the call raised."""
        ok, rows = safe(lambda: votes_svc().GetVoteCasesByCorporation(corp, STATUS_OPEN))
        return list(rows) if ok else None

    def closed_cases(corp):
        ok, rows = safe(lambda: votes_svc().GetVoteCasesByCorporation(corp, STATUS_CLOSED))
        return list(rows) if ok else None

    def tally(corp, case_id):
        """`{optionID: votesFor}` for a case, or None."""
        ok, rows = safe(lambda: votes_svc().GetVoteCaseOptions(case_id, corp))
        if not ok:
            return None
        return dict((o.optionID, o.votesFor) for o in list(rows))

    def my_vote(corp, case_id):
        """The option the caller voted for, -1 for none, None when it raised."""
        ok, rows = safe(lambda: votes_svc().GetVotes(corp, case_id))
        if not ok:
            return None
        rows = list(rows)
        return rows[0].optionID if rows else -1

    def ceo_of_corp(corp):
        """The corporation's `ceoID` as the client reads it, or None."""
        ok, row = safe(lambda: corp_svc().GetCorporation(corp))
        return getattr_safe(row, 'ceoID') if ok else None

    def ceo_case(corp):
        """The open CEO-type case as a row, or None."""
        for c in (open_cases(corp) or []):
            if getattr_safe(c, 'voteType') == VOTE_CEO:
                return c
        return None

    def candidate_of(corp, case_id):
        """`parameter` of option 0 of a case, the candidate the server stored."""
        ok, rows = safe(lambda: votes_svc().GetVoteCaseOptions(case_id, corp))
        if not ok:
            return None
        for o in list(rows):
            if o.optionID == 0:
                return o.parameter
        return None

    def sanctioned_row(case_id):
        """The row of `GetCorpSanctionedActions()` for a case, or None.  The
        call is a voteManager call, so it also runs the server's settle."""
        ok, rows = rcall('voteManager', 'GetCorpSanctionedActions')
        if not ok or rows is None:
            return None
        try:
            return rows[case_id] if case_id in rows else None
        except Exception:
            return None

    def kick_case(corp):
        """The open kick-type case as a row, or None."""
        for c in (open_cases(corp) or []):
            if getattr_safe(c, 'voteType') == VOTE_KICK:
                return c
        return None

    def kick_as_a(corp, me, other):
        """7. (#153) A, a member holding shares, is put to an expulsion vote by
        B, the CEO.  A casts no vote: a kick needs no quorum, the option with
        the most weight wins (server rule: corp_votes.rs take_due puts a quorum on the CEO
        type only), so B's own weight carries it.  A watches the case, then
        its own corporation change.  A's sight of the open kick case is not
        proven: B finishes in seconds.  A's push path is the one step 6 proves."""
        # B proposes, votes, closes and puts the kick in effect within seconds, so
        # A may start this step already expelled; A's own sight of an open case is
        # proven by step 6 (the CEO vote).  Wait for either.
        out = lambda: (lambda c: c is not None and c != corp)(getattr_safe(builtin('session'), 'corpid'))
        got, waited = wait_for(lambda: kick_case(corp) is not None or out(),
                               timeout_ms=STEP_WAIT_MS, step_ms=500)
        k = kick_case(corp)
        say('A at step 7: %s after %s ms' % (
            'open' if k is not None else 'expelled' if out() else 'neither', text(waited)))
        if k is not None:
            check('its option 0 names A as the pilot to expel',
                  candidate_of(corp, k.voteCaseID) == me,
                  'parameter %s, A %s' % (text(candidate_of(corp, k.voteCaseID)), text(me)))
        got, waited = wait_for(out, timeout_ms=STEP_WAIT_MS, step_ms=2000)
        check('A\'s session.corpid is no longer the corporation after the expulsion',
              bool(got), 'corpid %s, corp %s after %s ms'
              % (text(getattr_safe(builtin('session'), 'corpid')), text(corp), text(waited)))
        say('expelled pilot A is now in corporation %s' % text(getattr_safe(builtin('session'), 'corpid')))

    def kick_as_b(corp, me, other):
        """7. (#153) B, now the CEO, proposes the expulsion of A, votes for it,
        has a GM close it, reads it as a sanctioned action and puts it in
        effect."""
        precondition('A is still a member before the expulsion', corp_of(other) == corp,
                     text(corp_of(other)))
        if (lambda c: c is None or c != corp)(corp_of(other)):
            return
        ok, v = safe(lambda: votes_svc().CreateVote(
            {'title': '', 'description': 'f-ci', 'votetype': VOTE_KICK, 'time': 1,
             'kickmember': other, 'memberName': text(other)}))
        check('B proposes an expulsion vote naming A (CreateVote -> InsertVoteCase, kick)', ok, text(v)[:200])
        if not ok:
            return
        got, waited = wait_for(lambda: kick_case(corp) is not None, timeout_ms=30000, step_ms=500)
        check('B sees the expulsion vote open after OnVoteCaseChanged', bool(got),
              'after %s ms' % text(waited))
        k = kick_case(corp)
        if k is None:
            return
        check('its option 0 names A as the pilot to expel',
              candidate_of(corp, k.voteCaseID) == other,
              'parameter %s, A %s' % (text(candidate_of(corp, k.voteCaseID)), text(other)))
        ok, v = safe(lambda: votes_svc().InsertVote(corp, k.voteCaseID, 0))
        check('B votes for option 0 (InsertVote)', ok, text(v)[:200])
        got, waited = wait_for(lambda: (tally(corp, k.voteCaseID) or {}).get(0, 0) > 0,
                               timeout_ms=30000, step_ms=500)
        check('B\'s weight lands on option 0', bool(got),
              '%s after %s ms' % (text(tally(corp, k.voteCaseID)), text(waited)))
        ok, reply = gm('/vote close %d' % k.voteCaseID)
        closed = bool(ok) and 'closed; it acts' in text(reply)
        check('a GM closes the expulsion vote (/vote close)', closed, text(reply)[:200])
        if not closed:
            return

        def listed():
            # Every poll is a voteManager call, so the settle has run.
            return sanctioned_row(k.voteCaseID)
        got, waited = wait_for(listed, timeout_ms=STEP_WAIT_MS, step_ms=2000)
        row = sanctioned_row(k.voteCaseID)
        check('GetCorpSanctionedActions lists the won expulsion', row is not None,
              'after %s ms' % text(waited))
        if row is None:
            return
        check('the row is a kick type naming A, not acted upon',
              getattr_safe(row, 'voteType') == VOTE_KICK
              and getattr_safe(row, 'parameter') == other
              and not getattr_safe(row, 'actedUpon'),
              'type %s parameter %s actedUpon %s' % (
                  text(getattr_safe(row, 'voteType')), text(getattr_safe(row, 'parameter')),
                  text(getattr_safe(row, 'actedUpon'))))
        ok, v = rcall('voteManager', 'ActivateSanctionedAction', k.voteCaseID)
        check('B puts the expulsion in effect (ActivateSanctionedAction)', ok, text(v)[:200])
        got, waited = wait_for(lambda: (lambda c: c is not None and c != corp)(corp_of(other)),
                               timeout_ms=30000, step_ms=1000)
        check('A is no longer in the corporation', bool(got),
              'A in %s, corp %s after %s ms' % (text(corp_of(other)), text(corp), text(waited)))
        say('expelled pilot A is in corporation %s' % text(corp_of(other)))
        row = sanctioned_row(k.voteCaseID)
        check('the row now reads acted upon', row is not None and bool(getattr_safe(row, 'actedUpon')),
              text(getattr_safe(row, 'actedUpon')))
        ok, v = rcall('voteManager', 'ActivateSanctionedAction', k.voteCaseID)
        check('a second ActivateSanctionedAction is refused', not ok, 'answered %s' % text(v)[:200])
        check('and says the action has already been put in effect',
              (not ok) and 'already been put in effect' in text(v), text(v)[:300])

    def phase_ceo(me, other):
        svc = corp_svc()
        precondition('the client corp and corpvotes services run',
                     svc is not None and votes_svc() is not None, text(svc))
        if svc is None or votes_svc() is None:
            return
        a_corp = getattr_safe(builtin('session'), 'corpid')
        precondition('A is in an NPC corporation (a fresh world)',
                     a_corp is not None and a_corp < 98000000, text(a_corp))
        corp, why = found_corp()
        check('A founds a corporation', corp is not None, why)
        if corp is None:
            return
        # 0. B joins and is given a quarter
        ok, v = rcall('corpRegistry', 'InsertInvitation', other)
        check('A invites B to the corporation (InsertInvitation)', ok, text(v)[:200])
        joined, waited = wait_for(lambda: corp_of(other) == corp, timeout_ms=STEP_WAIT_MS, step_ms=2000)
        check('B is a member of A\'s corporation', joined is True,
              'B in %s, A in %s, after %s ms' % (text(corp_of(other)), text(corp), text(waited)))
        if joined is not True:
            return
        held = holders(corp)
        total = sum(held.values()) if held else 0
        precondition('A holds every share before the gift', held is not None and held.keys() == [me] and total > 3,
                     text(held))
        if not held or total < 4:
            return
        ok, v = safe(lambda: svc.MovePrivateShares(corp, other, total // 4))
        check('A gives B a quarter of the shares (MovePrivateShares)', ok, text(v)[:200])
        # 1. may view; may vote
        ok, v = safe(lambda: votes_svc().CanViewVotes(corp))
        check('A, a shareholder, can view votes (CanViewVotes)', ok and bool(v), text(v)[:200])
        ok, v = safe(lambda: votes_svc().CanVote(corp))
        check('A can vote (CanVote)', ok and bool(v), text(v)[:200])
        check('no vote is open yet', open_cases(corp) == [], text(open_cases(corp)))
        # 3. propose
        data = {'title': 'Fci vote ' + uniq(4), 'description': 'f-ci', 'votetype': VOTE_GENERAL,
                'time': 1, 'options': ['Option zero', 'Option one']}
        ok, v = safe(lambda: votes_svc().CreateVote(data))
        check('A proposes a general vote (CreateVote -> InsertVoteCase)', ok, text(v)[:200])
        got, waited = wait_for(lambda: len(open_cases(corp) or []) == 1, timeout_ms=30000, step_ms=500)
        cases = open_cases(corp) or []
        check('the client\'s cache refills after OnVoteCaseChanged and lists one open vote',
              bool(got), '%d open after %s ms' % (len(cases), text(waited)))
        check('and no closed one', closed_cases(corp) == [], text(closed_cases(corp)))
        if len(cases) != 1:
            return
        case = cases[0]
        check('the case carries its title and an end after its start',
              case.voteCaseText == data['title'] and case.endDateTime > case.startDateTime,
              '%s %s %s' % (text(case.voteCaseText), text(case.startDateTime), text(case.endDateTime)))
        t = tally(corp, case.voteCaseID)
        check('it has two options, no weight yet', t is not None and sorted(t.keys()) == [0, 1]
              and sum(t.values()) == 0, text(t))
        # 4. A votes option 0
        check('A has not voted', my_vote(corp, case.voteCaseID) == -1, text(my_vote(corp, case.voteCaseID)))
        ok, v = safe(lambda: votes_svc().InsertVote(corp, case.voteCaseID, 0))
        check('A votes for option 0 (InsertVote)', ok, text(v)[:200])
        got, waited = wait_for(lambda: my_vote(corp, case.voteCaseID) == 0, timeout_ms=30000, step_ms=500)
        check('A reads its own vote back after OnVoteCast (GetVotes)', bool(got),
              '%s after %s ms' % (text(my_vote(corp, case.voteCaseID)), text(waited)))
        ok, v = safe(lambda: votes_svc().InsertVote(corp, case.voteCaseID, 1))
        check('A\'s second vote is refused', not ok, 'answered %s' % text(v)[:200])
        # 5. B's vote lands; the tally is by shares the server reports
        held = holders(corp) or {}
        want = {0: held.get(me, 0), 1: held.get(other, 0)}
        got, waited = wait_for(lambda: tally(corp, case.voteCaseID) == want
                               and want[0] > 0 and want[1] > 0,
                               timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('the tally is each holder\'s shares on the option they chose (A on 0, B on 1)',
              bool(got), 'wanted %s, tally %s after %s ms' % (text(want), text(tally(corp, case.voteCaseID)), text(waited)))
        check('the second vote moved nothing: the tally still adds up to the shares',
              sum((tally(corp, case.voteCaseID) or {}).values()) == total,
              '%s of %s' % (text(tally(corp, case.voteCaseID)), text(total)))
        # 6. B runs for CEO (#153): the proposal carries B as the candidate and,
        # while it is open, nothing has changed hands.  Then both vote for it,
        # a GM closes it and one voteManager call settles it.
        got, waited = wait_for(lambda: ceo_case(corp) is not None, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('A sees B\'s run for CEO as an open vote of the CEO type', bool(got),
              'after %s ms' % text(waited))
        run = ceo_case(corp)
        if run is None:
            return
        check('its option 0 names B as the candidate (the parameter the server acts on)',
              candidate_of(corp, run.voteCaseID) == other,
              'parameter %s, B %s' % (text(candidate_of(corp, run.voteCaseID)), text(other)))
        ok, v = safe(lambda: votes_svc().InsertVote(corp, run.voteCaseID, 0))
        check('A votes for it (InsertVote)', ok, text(v)[:200])
        check('while the vote is open A is still the CEO', ceo_of_corp(corp) == me,
              'ceoID %s, A %s' % (text(ceo_of_corp(corp)), text(me)))
        # With B's vote option 0 holds every issued share, which meets the quorum.
        total_now = sum((holders(corp) or {}).values())
        got, waited = wait_for(lambda: total_now > 0 and (tally(corp, run.voteCaseID) or {}).get(0) == total_now,
                               timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('B\'s vote lands: option 0 holds every share', bool(got),
              'tally %s, shares %s after %s ms' % (text(tally(corp, run.voteCaseID)), text(total_now), text(waited)))
        ok, reply = gm('/vote close %d' % run.voteCaseID)
        closed = bool(ok) and 'closed; it acts' in text(reply)
        check('a GM closes the vote (/vote close)', closed, text(reply)[:200])
        if not closed:
            return
        # One voteManager call that skips the client cache runs the server's settle.
        # [V] voting.py:47 remoteSvc = sm.RemoteSvc('voteManager')
        ok, v = safe(lambda: votes_svc().remoteSvc.CanRunForCEO())
        check('one voteManager call after the close (CanRunForCEO)', ok, text(v)[:200])

        def settled():
            # Each poll calls the server again, so a failed first call cannot stall the settle.
            safe(lambda: votes_svc().remoteSvc.CanRunForCEO())
            return ceo_of_corp(corp) == other
        got, waited = wait_for(settled, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('a won CEO vote with a share majority hands the seat to B', bool(got),
              'ceoID %s, B %s after %s ms' % (text(ceo_of_corp(corp)), text(other), text(waited)))
        if got:
            kick_as_a(corp, me, other)

    def phase_member(me, other):
        svc = corp_svc()
        precondition('the client corp and corpvotes services run',
                     svc is not None and votes_svc() is not None, text(svc))
        if svc is None or votes_svc() is None:
            return
        corp, why = join_corp_of_issuer(me)
        check('B joins A\'s corporation on A\'s invitation', corp is not None, why)
        if corp is None:
            return
        got, waited = wait_for(lambda: (my_shares(corp) or 0) > 0, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('B holds shares after A gave some', bool(got),
              'shares %s after %s ms' % (text(my_shares(corp)), text(waited)))
        if not got:
            return
        ok, v = safe(lambda: votes_svc().CanViewVotes(corp))
        check('B, a shareholder, can view votes (CanViewVotes)', ok and bool(v), text(v)[:200])
        ok, v = safe(lambda: votes_svc().CanVote(corp))
        check('B can vote (CanVote)', ok and bool(v), text(v)[:200])
        # 2. a plain member may not propose
        ok, v = rcall('voteManager', 'InsertVoteCase', 'Sneaky', '', VOTE_GENERAL, ['a', 'b'], 1)
        check('the server refuses a proposal from a member with no Director role', not ok,
              'answered %s' % text(v)[:200])
        # By title, not by count: A's own proposal may land while B checks.
        sneaky = [c for c in (open_cases(corp) or []) if getattr_safe(c, 'voteCaseText') == 'Sneaky']
        check('and nothing opened', open_cases(corp) is not None and not sneaky,
              text(open_cases(corp)))
        # the vote A opens
        got, waited = wait_for(lambda: len(open_cases(corp) or []) == 1, timeout_ms=PEER_WAIT_MS, step_ms=2000)
        cases = open_cases(corp) or []
        check('B sees A\'s open vote after OnVoteCaseChanged', bool(got),
              '%d open after %s ms' % (len(cases), text(waited)))
        if len(cases) != 1:
            return
        case = cases[0]
        # wait for A to have voted so the tally is not a race
        got, waited = wait_for(lambda: sum((tally(corp, case.voteCaseID) or {}).values()) > 0,
                               timeout_ms=PEER_WAIT_MS, step_ms=2000)
        check('B sees A\'s weight in the tally', bool(got),
              '%s after %s ms' % (text(tally(corp, case.voteCaseID)), text(waited)))
        check('B has not voted', my_vote(corp, case.voteCaseID) == -1, text(my_vote(corp, case.voteCaseID)))
        ok, v = safe(lambda: votes_svc().InsertVote(corp, case.voteCaseID, 1))
        check('B votes for option 1 (InsertVote)', ok, text(v)[:200])
        got, waited = wait_for(lambda: my_vote(corp, case.voteCaseID) == 1, timeout_ms=30000, step_ms=500)
        check('B reads its own vote back after OnVoteCast (GetVotes)', bool(got),
              '%s after %s ms' % (text(my_vote(corp, case.voteCaseID)), text(waited)))
        ok, v = safe(lambda: votes_svc().InsertVote(corp, case.voteCaseID, 0))
        check('B\'s second vote is refused', not ok, 'answered %s' % text(v)[:200])
        held = holders(corp) or {}
        want = {0: held.get(other, 0), 1: held.get(me, 0)}
        got, waited = wait_for(lambda: tally(corp, case.voteCaseID) == want and want[0] > 0 and want[1] > 0,
                               timeout_ms=30000, step_ms=500)
        check('B sees the tally by shares (A on 0, B on 1)', bool(got),
              'wanted %s, tally %s after %s ms' % (text(want), text(tally(corp, case.voteCaseID)), text(waited)))
        # 6. B, a shareholder and not the CEO, runs for CEO (#153)
        # The client's CanRunForCEO answers 0 before any server call unless
        # the pilot has Corporation Management (voting.py CanRunForCEO [V]);
        # A got the skill when founding, B gets it here.  Poll: the skill
        # table reaches the client after the GM grant.
        gm('/giveskill Corporation Management 1')
        def can_run():
            ok_, v_ = safe(lambda: votes_svc().CanRunForCEO())
            return bool(ok_ and v_)
        got, waited = wait_for(can_run, timeout_ms=30000, step_ms=500)
        ok, v = safe(lambda: votes_svc().CanRunForCEO())
        check('B can run for CEO (CanRunForCEO)', ok and bool(v),
              '%s after %s ms' % (text(v)[:200], text(waited)))
        data = {'title': '', 'description': 'f-ci', 'votetype': VOTE_CEO, 'time': 1}
        ok, v = safe(lambda: votes_svc().CreateVote(data))
        check('B runs for CEO (CreateVote -> InsertVoteCase)', ok, text(v)[:200])
        got, waited = wait_for(lambda: ceo_case(corp) is not None, timeout_ms=30000, step_ms=500)
        check('B sees its CEO vote open after OnVoteCaseChanged', bool(got), 'after %s ms' % text(waited))
        run = ceo_case(corp)
        check('while the vote is open A is still the CEO', ceo_of_corp(corp) == other,
              'ceoID %s, A %s' % (text(ceo_of_corp(corp)), text(other)))
        if run is not None:
            ok, v = safe(lambda: votes_svc().InsertVote(corp, run.voteCaseID, 0))
            check('B votes for itself (InsertVote)', ok, text(v)[:200])
        got, waited = wait_for(lambda: ceo_of_corp(corp) == me, timeout_ms=STEP_WAIT_MS, step_ms=2000)
        check('B is the CEO after the vote closes', bool(got),
              'ceoID %s, B %s after %s ms' % (text(ceo_of_corp(corp)), text(me), text(waited)))
        if got:
            kick_as_b(corp, me, other)

    def linger(ms):
        waited = 0
        while waited < ms:
            sleep_ms(2000)
            waited += 2000

    def eoci_body():
        mark = log_mark()
        sess = builtin('session')
        me = getattr_safe(sess, 'charid')
        precondition('the harness named the other pilot (run with --with-second)',
                     bool(EXPECT_CHARID), 'EXPECT_CHARID %s' % text(EXPECT_CHARID))
        if not EXPECT_CHARID:
            return
        is_a = me < EXPECT_CHARID
        say('corp votes role: %s (me %s, other %s)' % ('A' if is_a else 'B', text(me), text(EXPECT_CHARID)))
        if is_a:
            phase_ceo(me, EXPECT_CHARID)
        else:
            phase_member(me, EXPECT_CHARID)
        linger(LINGER_MS if not is_a else 5000)
        no_new_tracebacks('corp votes', mark)

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
