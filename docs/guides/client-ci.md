# Client CI: running the real client against the server, unattended

This is the one-page guide. It replaces `docs/archive/plan-client-ci.md` as the
starting point (that file is a plan written before the harness existed, and
stays as history). The detail lives in the lane notes this page points at.

## What it is

A scenario is one Python 2.7 file that runs **inside the real client**
(`tools/client-ci/scenarios/<name>.py`), drives it, and writes a verdict file.
`tools/client-ci.sh` starts the server with that scenario selected, launches the
jailed client, waits for the verdict and exits with a status that says what
happened. It is the oracle for what the *client* parses and shows; the bot
suite (`server/crates/eve-bot`) is the oracle for behaviour and timing. Neither
replaces the other (`docs/archive/roadmap.md`, "Where the testing layer stands").

## Run it

```sh
tools/client-ci.sh list                              # the scenarios
tools/client-ci.sh run <scenario> --check            # preflight only; needs no root
sudo -v && tools/client-ci.sh run <scenario>         # the whole thing
```

`--with-second` adds a second client for two-player scenarios (the server gets a
roster two deep by itself; a scenario header can add server arguments with
`# SERVER-ARGS: ...`), `--rootless`
runs without sudo, `--census RUN` is a phone-home census run
(`docs/guides/census.md`). The header of `tools/client-ci.sh` lists every flag
and is the authority. A whole regression is `tools/client-ci-regress.sh`, over the list in
`tools/client-ci/regress.list`; its tables land in `var/regress-*/regress.tsv`.

## Several clients at once

`tools/client-ci-regress.sh --jobs N` runs N **clients** at once (default 3; a
row costs 1, a `--with-second` row costs 2 and waits until 2 are free, and the
budget is never exceeded; `--jobs 1` is the old one-at-a-time run). Each
parallel slot has its own copy of the Wine
prefix (`tools/client-ci/slot-prefix.sh ensure N`, about 1.7 GB, made by the runner;
`client-ci-regress.sh --clean` deletes them; one runner at a time),
its own client install name, rootless jail, run directory, dev world and ports,
so the slots share nothing (`tools/client-ci/slot-lib.sh` lists them); the
main offline prefix is only used by `--jobs 1` and by the rows marked `--serial` in
`regress.list`, which run alone at the end. A single slot by hand:
`tools/client-ci.sh run <scenario> --rootless --slot N`. The jail check runs for
every slot. `var/regress/clock.tsv` has the world clock's worst lateness per row
for judging whether N can go up.

**Exit status:** 0 passed; 1 the client disagreed; 2 usage; 3 refused (no jail,
no display, foreign server); 4 the scenario never ran; 5 malformed verdict;
6 the scenario raised.

The harness jails the client (`docs/guides/jail.md`) and uses its own headless
display unless you pass `--inherit-display`, so it does not touch your desktop's
mouse or keyboard. A quiet machine gives steadier timings.

Every run is silent and on the lowest graphics, audio off and effects off.
`tools/client-ci/quiet-settings.py` forces that in the client's
`core_public__.yaml` for the run and puts your file back afterwards; a run
that died has it put back at the start of the next one. Without it, Wine opens the
sound card through ALSA, which the desktop mixer can't mute.

The client renders on the GPU: RADV on a `gamescope --backend headless` display,
which has the DRI3 that Xvfb lacks and shows no window. `--software` (and `--frame`,
whose oracle reads Xvfb) keeps lavapipe.

## Who runs what

- A PR that changes something the client shows ships a scenario,
  `tools/client-ci/scenarios/fci_<name>.py`, and checks it with
  `tools/client-ci.sh run <name> --check`. That check does not run the client.
- A maintainer runs the scenarios.
- Scenarios cite the client file and line they check, `[V]`.

## Where things are

| what | where |
|---|---|
| the runner | `tools/client-ci.sh` and `tools/client-ci/` |
| writing a scenario: the prelude and the verdict contract | the canonical prelude is `tools/client-ci/scenarios/_prelude.py` |
| the guard tests | `tools/test_fci_scenarios.py` |
| why passwordless root, and what it still grants | `docs/guides/jail.md`, "What the jail helper grants" |
| the original plan (history) | `docs/archive/plan-client-ci.md` |
