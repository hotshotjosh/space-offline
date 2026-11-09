# Bench ramp: how many bots in one system before the server bends

`tools/bench-ramp.sh` starts its **own** release `eve-server` (a `--dev` world
copied from `var/bench/base`, port 26500, chat 26501, no client, no `--bubble`,
so nothing sheds) and one `eve-bots`, then ramps bots into one system: N0 at
the start, K more every H seconds, up to NMAX. It never touches `var/dev` or
port 26000, so it is safe beside a live session, but at full scale it loads the
machine: run the big ramps on a quiet one.

```sh
tools/bench-ramp.sh --prep --max 300                 # once: warm the accounts and kits
tools/bench-ramp.sh                                  # 50 -> 300, +50 every 75 s, one grid
tools/bench-ramp.sh --start 10 --add 10 --max 30 --hold 60     # the small proof (3 min)
tools/bench-ramp.sh --grid spread --mix miners=100             # miners in fives, a belt each
tools/bench-ramp.sh --stagger-ms 0 --start 100 --add 100       # a login burst per step
```

Options are in `--help`: `--start --add --max --hold --stagger-ms --grid --mix
--system --systems --brawl --stats --quiet --final-hold --login-rate --port --run --no-build --prep`.

## The two 1,000-scale ramps (#248)

```sh
# spread: 100 -> 1,000 bots, +100 every 90 s, fleets of five over 50 systems, 10 min at the top
tools/bench-ramp.sh --prep --grid spread --systems 50 --start 100 --add 100 --max 1000 --hold 90
tools/bench-ramp.sh --grid spread --systems 50 --start 100 --add 100 --max 1000 --hold 90 \
    --final-hold 600 --run spread1000
# fight: two sides on one grid, 50 -> 400, +50 per step (25 per side)
tools/bench-ramp.sh --prep --grid same --mix fighters=1,blues=1 --brawl --start 50 --add 50 --max 400 --hold 90
tools/bench-ramp.sh --grid same --mix fighters=1,blues=1 --brawl --start 50 --add 50 --max 400 --hold 90 \
    --run fight400
```

Run each alone on a quiet machine, never beside a soak or a client. The `--prep` of
a size is done once (it warms the accounts and kits on `var/bench/base`); the account
names depend on `--max` and `--mix`, not on `--systems`.

- `--systems K` (with `--grid spread`): the miners' fleets of five are seated
  round-robin in K systems through the roster's own `system` column, so a group's boss
  and its four followers share a home and the boss picks a belt there. The K are
  the `--system` home first, then the nearest **low or null systems with a station**
  by gate jumps (breadth first over `universe/celestials.tsv`, ties by id), read from
  `out/content` at run time and written to `var/bench/<run>/systems.txt`. 1 (default)
  is today's one-system run. `--prep` seats the same homes in `var/bench/base`, so a
  run on another K changes homes with a `/devhome` and a relog on first login.
- `--mix fighters=1,blues=1 --brawl`: `bf` fighters (side `red`) and `bg` blues
  (side `blu`) are two corporations, hostile to each other; with `--brawl` each
  closes on the nearest foe. A group is one fleet, at most 250. `--prep` seats every
  bot on the one side (`red`), so nobody fights while the kits are bought and the
  saved world is not damaged; the bots re-join their corporation (`/devcorp`) at each
  run's login, so the ramp's own roster puts the blues back.
- `--stats SECS` (default 10; 0 off) passes `eve-bots --stats`, which adds the columns
  below. `--quiet` also passes `--quiet` (no act lines, so `act_*` and `missing_lines`
  read 0; use it only when the log itself is the cost under test).
- `--final-hold SECS` holds the last step longer (the 10 minute series). `proc.tsv`
  beside `steps.tsv` has a row every 10 s: cpu seconds, RSS MB and threads of both
  processes.
- The shell raises the open-files soft limit to the hard one. With `--chat on` a bot holds
  about 2 sockets (game and chat), so 1,000 bots are ~2,000 in each process; the shell
  warns when the hard limit is below 2.5 x `--max`.

Measured by hand from `steps.tsv`: the knee, `srv_*` and `bots_*` per step, the
`decode : destiny : decide` split at 400 on one grid, `hot_systems`, `warm_systems`
and `work_p95_ms` at 1,000 spread, the RSS series of the last step.

**[A]** a hold of 90 s is enough per step at 1,000. Falsified when the summary prints
`WARNING ... windows` (fewer than 6) or `work p95 moved N% between its halves`
(more than 30%, and only when the step's p95 is above 10% of the 250 ms tick budget):
raise `--hold`. The halves are the step's windows after the first three; each half's p95 is
taken, not a mean. The 10 s tick lines are not aligned to the step start, so three skipped
windows cover any burst within the first 20 s of a step. **[A]** the batch's burst (logins,
undock, warp, fleet joins) is over by then; falsified when, across steps, the first kept
window's work p95 sits more than 30% above the median of the later windows: then a fleet
joining later than 20 s is what the check flags, and the skip must grow (with `--hold`).
A hold under 75 s leaves fewer than four windows after the skip, and the check is off. **[A]** K = 50 systems is representative of a
society; falsified by a soak's `[live]` line showing far fewer or far more warm
systems per bot (soak C: 20 warm for 60 bots).

The bots' ramp (`eve-bots --ramp`) logs in the roster in group-interleaved order (a
group's k-th member goes at (k+0.5)/n), so with fleets of five the first steps are
the bosses of every group, and the followers come in later steps; the fleets are whole
only after the fifth member's batch. The last step has all of them. [A] this does not
change the server's per-step load enough to matter; falsified when `destiny_bytes_per_bot_s` or `work_p95_ms` / `pilots` at the last two steps
(the fifth members sit at 80-100% of the login order, so the fleets are whole only
there) are more than 25% off the mean of the same figure over the steps at 50-80% of
`--max`: a jump at the step where fleets complete, not a slope.

## The login gate (#252, `--login-rate R`)

The ramp modes always pass `eve-bots --ramp`, and the bots' process-wide login gate
(`eve-bot/src/login_gate.rs`: a token bucket of burst 1, at most R logins a second, plus
a 0-10 s first-arrival jitter per bot) is installed only for Human pace without `--ramp`.
So `--login-rate R` is its own mode: all `--max` bots start at once at Human pace with
`--login-rate R`, one step, no ramp (`--start`, `--add`, `--stagger-ms` are ignored; not
with `--prep`, so run the `--prep` of that size first). The default rate to test is 2 a
second ([A] in `login_gate.rs`; falsifier: a burst at 20 a second, the old bench default,
showing no late ticks, in which case raise it).

```sh
tools/bench-ramp.sh --prep --max 1000        # once, as above
tools/bench-ramp.sh --max 1000 --login-rate 2 --run gate1000
```

The hold defaults to `--max / R + 40` s (500 + 40 = 540 s here); a shorter `--hold` is
refused (a non-zero `--final-hold` below it too; leave that out in this mode), since the
last bot would not be in. What it records, besides `steps.tsv`,
`summary.txt`, `proc.tsv` and the logs of a ramp:

- `logins.tsv`: `t_s  logins  cumulative`, the logins in each second since the bots
  started. The count is the server's `[svc] session change: character N entering the
  game` line, which has no clock, so the runner stamps each line with the time it read
  it (it polls the server log every 0.5 s in this mode; a stalled server therefore
  shows its backlog as one fat second, which is itself worth seeing). [A] The stamp
  is up to one poll (0.5 s) late, so the curve is good to about a second; falsified by
  a steady gate run whose `logins.tsv` alternates 0 and 4 in a second (then stamp
  finer, or have the server print a clock).
- `login-windows.tsv`: one row per 10 s tick-health window, with the logins since the
  previous window and its work p95 and late max, so the curve and the tick health sit
  side by side.
- A "login gate" block in `summary.txt`: logins seen of those wanted, the peak a second,
  the worst work p95 among windows with logins, and
  `WARNING login gate: N 5 s window(s) above 5R+1 logins` when a burst got through (a
  bucket of burst 1 allows 5R+1 in 5 s; judged over 5 s, not per second, because of the
  stamping skew, so it is approximate). The "N of M" figure counts distinct characters. `WARNING ... only N of M logins` means the hold
  ended first.

## What it does

- **Accounts**: made once, on `var/bench/base/world.dat` (the existing
  `tools/bot-accounts.sh`; passwords in `~/.config/eve-offline/bench-bots.tsv`,
  mode 600), then reused by every run. Names are `bm<G>x<N>` (miners),
  `br..` (ratters), `bf..` (fighters). A new run copies `base` to
  `var/bench/<run>/`, so every run starts alike.
- **`--prep`**: all NMAX bots log in at once on `base` and buy and fit their
  kits from the dev market, then it stops (at most 10 minutes; ~2 min for 30).
  Without it a ramp still runs, but its first minutes measure kit shopping
  (a roster bot is docked and buying for about its first minute).
- **Logins** use `eve-bots --ramp N0,K,H,GAP_MS`, not the human 3-13 s stagger:
  batch 0 is N0 bots, batch b is K bots, each `GAP_MS` apart (default 50 ms,
  20 a second; 0 = the whole batch in one burst, which is itself a test).
  The roster is interleaved in proportion to the group sizes, so every step
  adds the same mix.
- **Scenario**: `--grid same` puts the miners in one fleet (a group follows its
  boss to his belt; a fleet holds at most 256, so groups split at 250) and the
  ratters (`sites`) and fighters (`hunter`, Rifters) are one group each.
  `--grid spread` makes the miners fleets of five, each boss picking a belt of
  his own among the five nearest. `--mix` weights the three kinds (default
  `miners=100`; see the warning below). `--system` is the home (default Amamake).
  **[A]** that "same grid" really is one grid: fleets follow their boss, but
  nothing here reads the grid sets; the proof run's `in_space` shows the bots are
  out, Falsifier: the server's
  `[live]` line (pilots per system) fine but the `destiny_frames_per_tick`
  well under pilots x (pilots - 1) / pilots-per-grid.

**Warning, found by the proof**: in the dev world a roster Rifter (fighters) or
Vexor (ratters) cannot buy its ammo or guns (`MISSING: 1000 of type ..: asks
at station .. (cap 15.84 ..)`; the `cap` there is the bots' reservation price cap, not
hold space, `acquire.rs` ~1295), so those bots sit docked for ever and add connections, not grid load. That is
why the default mix is all miners; `--mix miners=60,ratters=25,fighters=15`
is there for when the kit is fixed. Check `bots.log` for `MISSING`: the bench
counts them (`missing_lines`, and a WARNING line in `summary.txt`). The dev
seller's flat ask is exempt from the reservation cap for every bot
(`acquire.rs` ~585-590), so the `cap 15.84` of the proof may be fixed already; **[A]**,
falsified by a `--prep --mix fighters=1,blues=1` run whose `bots.log` still has
`MISSING` lines. The bench has no lever on the bots' purchases (they buy in
`eve-bot/src/acquire.rs` by the order's own units), so a remaining cause there is
a drafted issue, not a bench fix.

## What it records

Per step (one hold), `var/bench/<run>/steps.tsv` (one row per step) and
`summary.txt`; the raw `server.log` and `bots.log` stay beside them.

| column | source |
| --- | --- |
| `pilots`, `in_space` | the last `[live]` line of the step |
| `work_p95_ms`, `work_max_ms`, `held_*`, `wait_max_ms`, `late_max_ms`, `over_budget`, `skipped` | the server's `[world] tick health` lines (one per 10 s window of 40 ticks). `work_p95` is the p95 over **ticks** (the server prints it now); `held_p95` is the p95 over windows of each window's worst hold |
| `srv_*`, `bots_*` cpu% (of one core), RSS MB, threads | /proc, every 2 s; RSS and threads are the step's peak |
| `out_kBps` | everything posted to connection outboxes (`Writers::post`) |
| `destiny_kBps`, `destiny_frames_per_tick`, `destiny_bytes_per_bot_s` | the `DoDestinyUpdate` pushes, per recipient |
| `act_failed`, `act_refused`, `usererrors` | the bots' act lines (`FAILED`, `refused`) and the server's UserErrors |
| `hot_systems`, `warm_systems`, `npcs`, `rocks` | the last `[live]` line of the step (`systems hot N warm M`, `npcs`, `rocks`) |
| `stats_n`, `stats_cpu_pct`, `stats_in_kBps` | the bots' `[stats]` lines in the step (`--stats`): how many, mean process CPU and frame bytes in |
| `decode_ms_s`, `observe_ms_s`, `destiny_ms_s`, `refresh_ms_s`, `decide_ms_s`, `absorb_ms_s`, `log_ms_s` | the same lines: mean ms of CPU per second of wall time, all bots together, spent in each phase |
| `p95_half_shift_pct` | p95 of the work p95s of the step's second half against its first (the first three 10 s windows dropped), in percent; over 30 flags the step in `summary.txt` when the larger of the two halves' p95s (`half_p95_max_ms`) is above 10% of the tick budget |
| `half_p95_max_ms` | the larger of the two halves' p95s (after the skipped windows), empty when too few windows are left |
| `missing_lines` | `MISSING` lines of the bots' log in the step (kits not bought) |

The server's tick-health line has three new trailing fields for this
(`work p95`, `out N frames B bytes`, `destiny N frames B bytes`; counters are
relaxed atomics, reset at each report). `tools/client-ci.sh` and
`tools/soak_summary.py` read the line's front and are unaffected.

**PA-10 coalescing** (`--coalesce-destiny`, default off) adds ` parts P merged M`
right after the destiny bytes: `P` is the mergeable Destiny pushes posted to
outboxes and `M` how many of them were appended to a part already queued, so
`M / P` is the reduction. With the flag on, `destiny N frames` counts the frames
the **writers** encode (a merged entry is one), not one per push per client; with
it off the clock counts them as before. Compare the two with
`tools/bench-ramp.sh --grid same` at 50, 262 and 500, flag off and on; in
process, `cargo test --release -p eve-server coalesce_bench -- --ignored --nocapture`
prints both side by side (ticker encode, writer encode, frames/s).

**The PA-0 profile lines** (lane pa0-prof, `live_status::prof`) follow the
tick-health line every window, in the same `[world] tick ...` style (measurement
only; `Instant` pairs and relaxed atomics):

| line | what |
| --- | --- |
| `tick sections` | per clock tick, the mean and max (ms) of `take_due` (scan, stamp, restamp), `filter` (`grid::filter`), `audience` (audience to client ids), `session_changes`, and `save` (the clock's `to_snapshot`; `n` is the saves) |
| `tick lock` | the world lock's occupancy over the window: the clock's hold and the payload turns' hold, each in ms and percent, payload wait, and the total. **From PA-12a the payload numbers are the EXCLUSIVE occupancy** (write-lock turns); turns served under `world.read()` overlap each other, so they are reported apart at the end of the line (`read turns N held X ms (shared, not in occupancy) ...`) and an older series is not comparable without `EVE_READ_TURNS=0` |
| `tick methods` | payload turns per `service.method`: top 10 by total hold and by count as `name:count:hold:max:ro|ro?|rw`: `ro` = served under the read lock at least once (PA-12a), `ro?` = a **guess** from the name (`Get`/`List`/`Is`/`Has`/`Find`/...), with the share of turns and of hold the guess covers; the line ends `ro-served turns X% hold Y%, fallbacks N` (read-eligible calls the write path served because output was queued). `EVE_READ_TURNS=0` turns read turns off for an A/B run |
| `tick writers` | the push-ticker: frames and bytes written, pass time, deflate (calls that compressed), and the longest single `write_all` |

`steps.tsv` gains the matching columns (`take_due_ms` ... `longest_write_ms`),
and `tools/soak_summary.py` prints a four-line digest.

**Not measured**: the bots' own lag (server stamp to the bot applying it): the
bots keep no such clock. Proxy: `act_failed` and `act_refused`, and bots
disconnecting (pilots falling). Balls on the grid are not counted either:
read `pilots`, and for the rest `npcs`/`rocks` in the `[live]` line.

## The knee and the stop

- **Knee** (`summary.txt`): the first step where `held_p95` or `work_p95`
  passes 50% of the 250 ms tick (125 ms), or any tick went over budget or
  was skipped.
- **Stop early**: 3 consecutive windows with ticks over budget, 20+ skipped
  ticks in a step, or pilots falling by more than 10% from the step before.
  The server then stops through its stop file (world saved) and the summary is
  written.

## How long

Steps cost `--hold` each plus the server start (about 5 s) and the stop (10-20 s).
50 -> 300 in steps of 50 is six steps: **6 x 75 s = 7.5 min, about 8 min** with
the defaults, plus `--prep` once (about 5-10 min for 300 bots, the accounts
once, minutes more). Hold of 60 s is the minimum worth having (six windows), but it turns
the halves check off; 75 s is the least that keeps it.
The proof (10 -> 30, hold 60) took 3 min.

## Where it should break (read from the code, [A] until a big run)

1. **The world write lock, per tick.** `clock_turn` runs every stage and the
   drain under one write lock. `grid::filter` runs per push, per audience
   member, so a system-wide push (a ship's Destiny update goes to every pilot
   on its grid) costs O(n) lookups, and n ships each moving means O(n^2) per
   tick on one grid. `grid::reconcile_in` runs every 4th tick (1 s) and is
   O(viewers x local balls) with a `BTreeSet` lookup each, plus a scene
   rebuild when anybody gains a ball: expect `held_max` and the
   `tick stages` line's `grid` stage to grow roughly with n^2 and to spike
   once a second. Look at `[world] tick stages` first.
2. **Frame bytes**: `destiny_frames_per_tick` x pilots; `out_kBps` per bot
   grows linearly with grid size, so total output is O(n^2). Every connection
   has a writer thread and an outbox (4096 frames / 16 MiB cap); a bot that
   reads slowly overflows it and is disconnected (a `[push] ... outbox over
   cap` line, and pilots falling).
3. **The bots' side**: one OS thread per bot (see `threads`) plus the park
   integration per bot: each bot applies every Destiny frame to its own park
   copy, so bots CPU is O(n) per bot, O(n^2) overall. The 13 MB per bot RSS is
   mostly its park and the content tables (shared).
4. **Not a limit here**: saves (every 5 s, render under the lock, but linear in
   world size), chat, the 166-thread count (one reader + writer per client).

If the columns are not enough, instrument next: per-stage times are already in
`[world] tick stages`; add a `grid` sub-timer for `filter` vs `reconcile_in`;
count balls per park (`Parks`) and the largest audience per push; time the bots'
apply of one `DoDestinyUpdate` (driver) for the lag.

## Server flags and the connection cap

`EVE_BENCH_SERVER_ARGS="--tick-threads 1" tools/bench-ramp.sh ...` passes extra
flags to `eve-server` (and the summary line names them). The bench listener
takes `cap=max(2048, 2 x --max)` live connections: the loopback default (128)
refused every bot past it. The one-shot sign-up server in
`tools/bot-accounts.sh` takes 4096 for the same reason. The PA-3 `slice_max`
field of the tick-health line becomes the `slice_max_ms` and `slice_p95_ms`
columns (0 for a server without it).

## Profiling the bots process (BOT-OPT)

`EVE_BENCH_BOT_ARGS="--stats 20 --quiet" tools/bench-ramp.sh ...` passes extra
flags to `eve-bots`. `--stats SECS` prints a `[stats]` line (process CPU, RSS,
threads, bytes in, and the CPU all bots spent decoding, applying destiny,
refreshing, deciding, absorbing, logging); `--quiet` prints no act lines. `eve-bots`
restarts itself once with `MALLOC_ARENA_MAX=2` (glibc's per-thread arenas
doubled the RSS of 40 bots); set the variable yourself to choose another value.

## A battle with a client (fci_battle)

`bench-ramp.sh` has no client. To put one on the grid of a growing brawl, run the
scenario `tools/client-ci/scenarios/fci_battle.py` with the bot link (`--with-bots`)
and from release builds, since a debug build says nothing about a stall:

    tools/client-ci.sh run fci_battle --rootless --with-bots --release --check -- --dev --turn-sched
    tools/client-ci.sh run fci_battle --rootless --with-bots --release --timeout 4800 -- --dev --turn-sched

- `--release` (needs `--rootless`) runs the server and `eve-bots` from
  `server/target/release`; `eve-dns` stays debug. The preflight prints `profile:`.
- `--bots-args "--brawl --stats 20"` adds words to the `eve-bots` command line. Only
  `--brawl`, `--quiet` and `--stats N` (1 to 3600) are allowed. With none given, the
  scenario's `# BOTS-ARGS:` header line applies (`regress.list` cannot quote a value).
- The scenario's defaults are the small run (20 a side, in steps of 10). The big run
  is set in `C:\eve-logs\client-ci\fci.params` in the client prefix, one `key=value`
  per line: `battle_per_side=200`, `battle_step=50`, `battle_hold_s=180`,
  `battle_final_hold_s=600`, `battle_step_wait_s=300`, and `battle_system=<id>` to
  pin a null-sec system with a station (the id goes in this file, never in git).
  Without `battle_system` it takes the first null-sec (security below 0) system `/devhome` accepts.
- The ego cloaks 80 km (plus the station radius) from the station, so the bots never target it.
- Run it alone: no gate, no soak, no builds. Clean up the params file afterwards.

Afterwards, copy the run directory (`eve-server.log`, `bots.log`, `result.json`) out
before the next run wipes it, and summarise it:

    python3 -I tools/battle_summary.py sample --out procs.tsv SERVER_PID BOTS_PID &
    python3 -I tools/battle_summary.py summarize --server-log run/eve-server.log \
        --bots-log run/bots.log --procs procs.tsv --result run/result.json \
        --per-side 200 --out summary

`summary/summary.txt` gives skipped ticks, the longest clock stall, the clock hold
p50/p95/max, the longest clock slice wait, the payload turn hold, `capped`, peak
pilots, CPU and RSS, and the client's own numbers, for the fight windows (in space at
least 75 % of 2 x per side) and for the whole run. The verdict is PASS when skipped is
0, no window has a late max over 100 ms, `capped` is 0 and the clock slice wait is
under 100 ms. `summary/battle.tsv` has one row per 10 s window; `summary/stalls/w<N>.txt`
holds the raw log block of every window with a late tick over 100 ms or any skipped
tick, ready to paste into a drafted issue. A late max of 1000 ms or more also makes
`client-ci.sh` exit 1 ("THE CLOCK DID NOT KEEP TIME").
