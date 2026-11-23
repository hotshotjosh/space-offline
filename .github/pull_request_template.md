<!-- space-offline-pr-template: keep this line; label.yml looks for it -->
Fill in every section and tick every box. The procedure is `AGENTS.md`. Labels sort
work; a maintainer peer-reviews the PR and the owner approves and merges it.

Closes #

Depends on: (another PR that must land first, as `#N`, or "none")

## What changed, and why

## Overlap
The output of `python3 -I tools/pr_overlap.py`:

## Tests
new: …   existing: …   (or "none, because ...")

## [A] values
Each assumed value with what would falsify it, or "none":
[A] … — falsifier: …

## Status rows touched (docs/status/progress.tsv)
The rows you changed or added (state, evidence, issue, theme), and that you ran
`python3 -I tools/status-page.py`. Or, when no feature state changed, a line:
Status: none (<why>)

## Human check
Only for player-visible work (label `client-visible`): what a human with the
client should see, and the scenario that shows it. Otherwise "none".

## Gate
The last lines of `tools/land.sh --no-push`: the `out/landed/<sha>` stamp.

## Review (optional)
Any review you ran (a person or a model), and its findings: each answered (fixed in <sha>, or why not). Or "none".

## Agent and model
The coding agent and model you used, or "none, written by hand":

## Checklist
- [ ] Status rows updated in `docs/status/progress.tsv` and the page regenerated, or a `Status: none (<why>)` line above.
- [ ] No extracted client data, in tests or anywhere else (tests use invented ids and rows).
- [ ] No new third-party crate (`server/deps.allow` and `server/deps.digest` unchanged) and no `unsafe` in our code.
- [ ] I touched no guarded path (see `.github/CODEOWNERS`), or I say here why: 
- [ ] I ran the gate (`tools/land.sh --no-push`) and it is green.
- [ ] I accept the Contributor Terms (`CONTRIBUTING-terms.md`) for this PR: I wrote it or have the right to submit it, it contains no AGPL, GPL or other incompatible code, and I grant thewhitekidney the rights set out there, including relicensing.
