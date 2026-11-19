#!/usr/bin/env python3
"""Build the progress page from one table (issue #164).

`docs/status/progress.tsv` is the single source: one row per feature. This
tool writes `docs/status/README.md` and four SVGs beside it, so every number
on the page comes from that table and `docs/status/waves.tsv`. Nothing here
reads `var/`, the clock or the network when it writes, so two machines get the
same bytes and `--check` can run in the gate.

    python3 -I tools/status-page.py           # write the page
    python3 -I tools/status-page.py --check   # fail if it is stale or a row is bad
    python3 -I tools/status-page.py --audit   # compare rows with var/regress-*; clean: stamp checked-at
    python3 -I tools/status-page.py --audit --since BASE   # only rows changed since BASE fail
    python3 -I tools/status-page.py --waves   # add var/regress-wave* counts to waves.tsv

`--audit` and `--waves` read the gitignored `var/`, so only a machine with the
regression results can run them. `--audit` reports and never edits a row: a
person decides what a red means. `--audit --since BASE` is the landing gate's form: a
finding that rests on `var/` verdicts fails only for a row that changed since BASE; for any
other row it prints a `status-page: note:` line and passes (verdict drift is for
`tools/gate.sh --env-checks`). A clean audit stamps `# checked-at <HEAD>`
unless `--no-stamp` is given (the gate passes it). It also fails on a scenario
with a verdict, or a regress.list row, that no row cites; without `var/` it skips.

Issue numbers appear on the page only when `EVE_ISSUES_URL` is set (they are
rendered as links `$EVE_ISSUES_URL/<n>`); the committed page is built without
it, so it never names a tracker.

The SVGs have an opaque background of their own. A viewer picks a page theme
that the image cannot see, so an image without one would risk dark ink on a
dark page; with one it reads wherever it lands.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import glob
import importlib.util
import math
import os
import re
import subprocess
import tempfile
from typing import NamedTuple
import sys
from html import escape

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
STATUS_DIR = "docs/status"
COLUMNS = ["system", "feature", "state", "evidence", "issue", "theme", "note"]
WAVE_COLUMNS = ["wave", "date", "sha", "rows", "green", "red", "no_verdict", "scope", "note"]

# What each state means, in the order the page lists them.
STATES = {
    "client-proved": "A regression scenario passed on the real client in the newest wave that ran it. "
    "It proves what the scenario asserts and no more.",
    "done": "Finished, and proved some other way than a client scenario: a tool that runs, a measured result.",
    "built-unproved": "The code landed and unit tests pass, but no client has passed it: "
    "its scenario was red last time, has never run, or does not exist.",
    "partial": "Some of it works, and the row says what is not there.",
    "stubbed": "The server gives a correctly shaped empty answer so the client does not fail. No real feature behind it.",
    "missing": "Nothing yet.",
    "unknown": "Could not be placed from the evidence. Never a guess.",
    "parked": "Parked: not planned now. Left out of every percentage.",
}

# The four ways the graphs group the eight states, from most to least finished.
GROUPS = [
    ("proved", "Proved", ("client-proved", "done")),
    ("built", "Built, not proved", ("built-unproved",)),
    ("part", "Part built", ("partial", "stubbed")),
    ("missing", "Missing", ("missing",)),
    ("unknown", "Unknown", ("unknown",)),
]
GROUP_OF = {state: key for key, _, states in GROUPS for state in states}
FINISHED = ("client-proved", "done")
NO_THEME = "Not on the plan yet"

# Regression rows that test no feature, so --audit does not ask for a row.
NOT_FEATURES = {
    "fci_api_probe": "a read-only probe of the routes the scenarios use",
    "fci_gw_notice_echo": "skips itself: the gateway notice echo has no dev hook yet",
}

# Colours. One mark per feature, and the shape carries the state, so the colour is a second
# channel: filled dark is proved, filled mid is built, half-filled is part built, hollow is
# missing, orange is unknown. Two blues only; every fill and stroke clears 3:1 on its surface
# (checked with the dataviz skill's validator). Hollow marks also carry `fill="none"` as an
# attribute, so a renderer that ignores this CSS still draws them hollow.
STYLE = """<style>
.bg{fill:#fcfcfb}.t{fill:#0b0b0b}.s{fill:#52514e}.rule{fill:#e1e0d9}
.proved{fill:#184f95}.built{fill:#3987e5}.unknown{fill:#eb6834}
.part-o{fill:none;stroke:#3987e5;stroke-width:1.5}.part-f{fill:#3987e5}
.missing{fill:none;stroke:#8a8984;stroke-width:1.5}
.pass{fill:#184f95}.fail{fill:#eb6834}.none{fill:none;stroke:#8a8984;stroke-width:1.5}
@media (prefers-color-scheme:dark){
.bg{fill:#1a1a19}.t{fill:#f0efec}.s{fill:#c3c2b7}.rule{fill:#383835}
.proved{fill:#86b6ef}.built{fill:#3987e5}.unknown{fill:#d95926}
.part-o{stroke:#3987e5}.part-f{fill:#3987e5}
.missing{stroke:#8d8c85}
.pass{fill:#86b6ef}.fail{fill:#d95926}.none{stroke:#8d8c85}
}
text{font-family:system-ui,-apple-system,"Segoe UI",sans-serif;font-size:12px}
.big{font-size:26px;font-weight:600}.b{font-weight:600}.xs{font-size:11px}
</style>"""

WIDTH = 480  # the viewBox width and the rendered width: svg() sets both, so nothing is upscaled
PAD = 16
ROW = 16  # row pitch of the row charts
PITCH_MAX, PITCH_MIN = 8, 4  # pitch of one mark; the mark is one pixel smaller
LABEL_EM = 0.6  # width per label character, wide enough for DejaVu and Cantarell
SHORT = {"proved": "Proved", "built": "Built", "part": "Part built", "missing": "Missing", "unknown": "Unknown"}
GROUP_KEYS = [k for k, _, _ in GROUPS]
MAX_WAVES = 40  # the regression chart shows the newest this many
WAVE_KEYS = ("pass", "fail", "none")


class Bad(Exception):
    pass


def read_table(path: str, columns: list[str]) -> tuple[list[dict], list[str]]:
    """The rows and the leading `#` comment lines of a tsv."""
    rows, comments = [], []
    with open(path, encoding="utf-8") as f:
        lines = f.read().split("\n")
    header_seen = False
    for n, line in enumerate(lines, 1):
        if not line.strip():
            continue
        if line.startswith("#"):
            comments.append(line)
            continue
        cells = line.split("\t")
        if not header_seen:
            if cells != columns:
                raise Bad(f"{path}:{n}: the header must be {' '.join(columns)}")
            header_seen = True
            continue
        if len(cells) != len(columns):
            raise Bad(f"{path}:{n}: {len(cells)} cells, want {len(columns)}")
        row = dict(zip(columns, cells))
        row["line"] = n
        rows.append(row)
    return rows, comments


def scenario_names(root: str) -> set[str]:
    return {
        os.path.basename(p)[:-3]
        for p in glob.glob(os.path.join(root, "tools/client-ci/scenarios/*.py"))
        if not os.path.basename(p).startswith("_")
    }


def scenarios_in(text: str, known: set[str]) -> list[str]:
    """Scenario names a row's evidence cites, with any `@label` kept."""
    found = []
    for word in re.findall(r"[A-Za-z_][A-Za-z0-9_]*(?:@[a-z0-9_]+)?", text):
        if word.split("@")[0] in known or word.startswith("fci_"):
            found.append(word)
    return found


def validate(rows: list[dict], known: set[str], themes: dict[str, str]) -> list[str]:
    """Every complaint about the table; empty means it is sound."""
    fail, seen = [], set()
    for r in rows:
        at = f"line {r['line']} ({r['feature'] or 'no feature'})"
        for col in ("system", "feature", "state", "evidence"):
            if not r[col]:
                fail.append(f"{at}: empty {col}")
        if r["state"] and r["state"] not in STATES:
            fail.append(f"{at}: state {r['state']!r} is not one of {', '.join(STATES)}")
        key = (r["system"], r["feature"])
        if key in seen:
            fail.append(f"{at}: the same feature twice in {r['system']}")
        seen.add(key)
        if r["issue"] and not re.fullmatch(r"#\d+(, #\d+)*", r["issue"]):
            fail.append(f"{at}: issue must read #N or #N, #M")
        if r["theme"] and r["theme"] not in themes:
            fail.append(f"{at}: theme {r['theme']} is not a row of {THEMES}")
        if r["state"] and r["state"] not in FINISHED and not r["theme"]:
            fail.append(f"{at}: an open row needs a theme (a row of {THEMES})")
        if r["state"] in FINISHED and r["theme"]:
            fail.append(f"{at}: a finished row has no theme")
        cited = scenarios_in(r["evidence"], known)
        for name in cited:
            if name.split("@")[0] not in known:
                fail.append(f"{at}: no scenario called {name}")
        if r["state"] == "client-proved" and not cited and "census" not in r["evidence"]:
            fail.append(f"{at}: client-proved must name a scenario or a census")
        if re.search(r"(?<![\d.])\d{8,9}(?![\d.])", "\t".join(r[c] for c in COLUMNS)):
            fail.append(f"{at}: an 8-9 digit number looks like a client id")
    return fail


THEMES = "docs/status/themes.tsv"


def plan_themes(root: str) -> dict[str, str]:
    """The themes, `3.N<TAB>Name` per line of docs/status/themes.tsv (public; it
    mirrors the master plan's `### 3.N` headings, which a test checks)."""
    out = {}
    with open(os.path.join(root, THEMES), encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if line and not line.startswith("#"):
                key, _, name = line.partition("\t")
                out[key] = name
    return out


def issue_text(issue: str) -> str:
    """The issue cell as the page shows it: nothing unless EVE_ISSUES_URL is set."""
    base = os.environ.get("EVE_ISSUES_URL", "").rstrip("/")
    if not issue or not base:
        return ""
    return ", ".join(f"[{n}]({base}/{n[1:]})" for n in re.findall(r"#\d+", issue))


def counts(rows: list[dict]) -> dict[str, int]:
    out = {state: 0 for state in STATES}
    for r in rows:
        out[r["state"]] += 1
    return out


def group_counts(rows: list[dict]) -> dict[str, int]:
    out = {key: 0 for key, _, _ in GROUPS}
    for r in rows:
        if r["state"] in GROUP_OF:
            out[GROUP_OF[r["state"]]] += 1
    return out


def percent(part: int, whole: int) -> int:
    return round(100 * part / whole) if whole else 0


def headline(rows: list[dict]) -> dict[str, int]:
    """Row counts behind the two percentages. Parked rows are not counted."""
    g = group_counts(rows)
    counted = sum(g.values())
    return {
        "counted": counted,
        "proved": g["proved"],
        "built": g["proved"] + g["built"],
        "proved_pct": percent(g["proved"], counted),
        "built_pct": percent(g["proved"] + g["built"], counted),
        "parked": counts(rows)["parked"],
    }


def systems(rows: list[dict]) -> list[str]:
    """Systems in the table's own order."""
    return list(dict.fromkeys(r["system"] for r in rows))


def tw(s, size: int = 12) -> float:
    """A rough text width: system-ui averages about 0.58 em per character."""
    return len(str(s)) * size * 0.58


def text(x, y, s, cls: str = "t", anchor: str = "") -> str:
    a = f' text-anchor="{anchor}"' if anchor else ""
    return f'<text x="{x:.1f}" y="{y:.1f}" class="{cls}"{a}>{escape(str(s))}</text>'


def svg(height: int, label: str, body: list[str]) -> str:
    # width and height on the root give the image an intrinsic size, so a viewer's
    # max-width:100% only ever shrinks it (a phone) and never stretches it.
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
        f'viewBox="0 0 {WIDTH} {height}" role="img" aria-label="{escape(label, quote=True)}">\n'
        f"<title>{escape(label)}</title>\n{STYLE}\n"
        f'<rect width="{WIDTH}" height="{height}" rx="6" class="bg"/>\n' + "\n".join(body) + "\n</svg>\n"
    )


def cell(x, y, size, key: str) -> str:
    """One feature as one mark. Hollow rects also say fill="none" for CSS-less renderers."""
    if key == "part":
        h = size / 2
        return (
            f'<rect x="{x + .5:.1f}" y="{y + .5:.1f}" width="{size - 1}" height="{size - 1}" fill="none" class="part-o"/>'
            f'<rect x="{x:.1f}" y="{y + h:.1f}" width="{size}" height="{h:.1f}" class="part-f"/>'
        )
    if key in ("missing", "none"):
        return (
            f'<rect x="{x + .5:.1f}" y="{y + .5:.1f}" width="{size - 1}" height="{size - 1}" fill="none" class="{key}"/>'
        )
    return f'<rect x="{x:.1f}" y="{y:.1f}" width="{size}" height="{size}" class="{key}"/>'


def pitch_for(biggest: int, room: float) -> int:
    return max(PITCH_MIN, min(PITCH_MAX, int(room // max(biggest, 1))))


def legend(y, entries: list[tuple[str, str]], x0: int = PAD) -> tuple[list[str], float]:
    """A mark and a label on one line; wraps only when the next entry would pass the edge."""
    out, x = [], x0
    for key, label in entries:
        w = 14 + tw(label, 11) + 10
        if x + w > WIDTH - PAD:
            x, y = x0, y + 16
        out.append(cell(x, y - 8, 9, key))
        out.append(text(x + 14, y, label, "s xs"))
        x += w
    return out, y


def fit_label(label: str, width: float) -> str:
    """The label cut with an ellipsis so it fits `width` px at LABEL_EM per character."""
    room = max(int(width // (12 * LABEL_EM)), 1)
    return label if len(label) <= room else label[: room - 1].rstrip() + "\u2026"


def row_chart(groups: list[tuple[str, dict[str, int]]], right_of) -> tuple[list[str], float]:
    """Rows of `label | one mark per feature | right text`; `groups` is [(label, {key: n})].
    A row with more marks than fit in the room wraps at `room // pitch` marks and takes
    ROW more height per extra line. A label wider than the label column ends in an ellipsis."""
    label_w = min(220, max((len(n) * 12 * LABEL_EM for n, _ in groups), default=80))
    cell_x = PAD + label_w + 10
    biggest = max((sum(g.values()) for _, g in groups), default=1)
    room = WIDTH - PAD - cell_x - 44  # leave room for the right-hand text
    p = pitch_for(biggest, room)
    per_line = max(1, int(room // p))
    size = p - 1
    body, y = [], 8.0
    for name, g in groups:
        body.append(text(PAD, y + 10, fit_label(name, label_w), "t"))
        i = 0
        for key in GROUP_KEYS:
            for _ in range(g.get(key, 0)):
                line, col = divmod(i, per_line)
                body.append(cell(cell_x + col * p, y + line * ROW + (12 - size) / 2 + 1, size, key))
                i += 1
        line, col = divmod(i - 1, per_line) if i else (0, -1)
        body.append(text(cell_x + (col + 1) * p + 4, y + line * ROW + 10, right_of(name, g), "s xs"))
        y += ROW * (line + 1)
    return body, y


def overall_svg(rows: list[dict]) -> str:
    h, g = headline(rows), group_counts(rows)
    body = [
        text(PAD, 34, f"{h['proved_pct']}%", "t big"),
        text(PAD + len(f"{h['proved_pct']}%") * 26 * 0.68 + 6, 34, "proved", "s"),
        text(PAD + 150, 34, f"{h['built_pct']}%", "t big"),
        text(PAD + 150 + len(f"{h['built_pct']}%") * 26 * 0.68 + 6, 34, "built", "s"),
        text(WIDTH - PAD, 34, f"of {h['counted']} features", "s", "end"),
    ]
    # A waffle, filled column by column: one mark per counted feature, so a group of
    # two is two visible marks and not a sliver.
    p = 7
    nrows = max(1, math.ceil(h["counted"] / ((WIDTH - 2 * PAD) // p)))
    seq = [k for k in GROUP_KEYS for _ in range(g[k])]
    top = 46
    for i, key in enumerate(seq):
        c, r = divmod(i, nrows)
        body.append(cell(PAD + c * p, top + r * p, p - 1, key))
    marks, y = legend(top + nrows * p + 18, [(k, f"{SHORT[k]} {g[k]}") for k in GROUP_KEYS])
    body += marks
    label = f"{h['proved_pct']} percent proved, {h['built_pct']} percent built, of {h['counted']} features"
    return svg(int(y + 10), label, body)


def systems_svg(rows: list[dict]) -> str:
    groups = [(n, group_counts([r for r in rows if r["system"] == n])) for n in systems(rows)]
    groups = [(n, g) for n, g in groups if sum(g.values())]
    body, y = row_chart(groups, lambda n, g: f"{g['proved']}/{sum(g.values())}")
    marks, y = legend(y + 14, [(k, SHORT[k]) for k in GROUP_KEYS])
    body += marks
    body.append(text(WIDTH - PAD, y + 16, "right: proved / features", "s xs", "end"))
    return svg(int(y + 24), "Progress by system, one mark per feature", body)


def left_rows(rows: list[dict]) -> list[dict]:
    return [r for r in rows if r["state"] not in FINISHED and r["state"] != "parked"]


def theme_order(rows: list[dict], themes: dict[str, str]) -> list[tuple[str, str]]:
    """(key, title) for each theme with work left, in plan order, the unplanned last."""
    left = left_rows(rows)
    out = [(k, f"{k} {name}") for k, name in themes.items() if any(r["theme"] == k for r in left)]
    if any(not r["theme"] for r in left):
        out.append(("", NO_THEME))
    return out


def left_svg(rows: list[dict], themes: dict[str, str]) -> str:
    left = left_rows(rows)
    groups = [(title, group_counts([r for r in left if r["theme"] == k])) for k, title in theme_order(rows, themes)]
    body, y = row_chart(groups, lambda t, g: f"{sum(g.values())}")
    marks, y = legend(y + 14, [(k, SHORT[k]) for k in GROUP_KEYS if k != "proved"])
    body += marks
    return svg(int(y + 10), "What is left, by theme, one mark per open feature", body)


def regress_svg(waves: list[dict]) -> str:
    """One column per wave on a shared baseline, so the trend reads: passed, failed, no verdict."""
    cut = max(len(waves) - MAX_WAVES, 0)
    last_cut = waves[cut - 1]["wave"] if cut else ""
    waves = waves[cut:]  # the newest MAX_WAVES; the table lists them all
    # A wave whose parts add up to more than its rows must still fit the plot.
    biggest = max((max(int(w["rows"]), int(w["green"]) + int(w["red"]) + int(w["no_verdict"])) for w in waves), default=1) or 1
    x0, x1 = PAD + 30, WIDTH - PAD
    pitch = min(40, (x1 - x0) / max(len(waves), 1))
    bw = pitch * 0.68  # never wider than the pitch: columns cannot overlap
    sparse = pitch < 16  # then only every second wave (counting from the newest) is labelled
    top, hgt = 22, 84
    base = top + hgt
    body = [
        f'<rect x="{x0 - 4}" y="{base:.1f}" width="{x1 - x0 + 4}" height="1" class="rule"/>',
        text(x0 - 8, top + 4, biggest, "s xs", "end"),
        text(x0 - 8, base, 0, "s xs", "end"),
        f'<rect x="{x0 - 4}" y="{top:.1f}" width="{x1 - x0 + 4}" height="1" class="rule"/>',
    ]
    for i, w in enumerate(waves):
        green, red, none = int(w["green"]), int(w["red"]), int(w["no_verdict"])
        x, y = x0 + i * pitch + (pitch - bw) / 2, float(base)
        for cls, c in (("pass", green), ("fail", red), ("none", none)):
            if not c:
                continue
            seg = hgt * c / biggest
            y -= seg
            if cls == "none":
                body.append(
                    f'<rect x="{x + .5:.1f}" y="{y + .5:.1f}" width="{bw - 1:.1f}" '
                    f'height="{max(seg - 1.5, 1):.1f}" fill="none" class="none"/>'
                )
            else:
                body.append(f'<rect x="{x:.1f}" y="{y:.1f}" width="{bw:.1f}" height="{max(seg - 1, 1):.1f}" class="{cls}"/>')
        if not sparse or (len(waves) - 1 - i) % 2 == 0:
            body.append(text(x + bw / 2, y - 3, green, "s xs", "middle"))
            body.append(text(x + bw / 2, base + 14, w["wave"], "t xs", "middle"))
        mark = {"full": "", "partial": "p", "rerun": "r"}.get(w["scope"], "?")
        if mark:
            body.append(text(x + bw / 2, base + 26, mark, "s xs", "middle"))
    body.append(text(PAD, base + 14, "wave", "s xs"))
    marks, y = legend(base + 46, [("pass", "Passed (number above)"), ("fail", "Failed"), ("none", "No verdict")])
    body += marks
    note = "p partial run, r rerun of red rows only: compare full runs with full runs"
    body.append(text(PAD, y + 16, note, "s xs"))
    if cut:
        y += 16
        body.append(text(PAD, y + 16, f"waves 1\u2013{last_cut} in the table", "s xs"))
    return svg(int(y + 24), "Client regression waves: passed, failed and no verdict per wave", body)


def row_line(r: dict) -> str:
    mark = "x" if r["state"] in FINISHED else " "
    bits = [f"- [{mark}] {r['feature']}: *{r['state']}*, {r['evidence']}"]
    if issue_text(r["issue"]):
        bits.append(f" ({issue_text(r['issue'])})")
    if r["note"]:
        bits.append(f". {r['note']}")
    return "".join(bits)


GUIDE = """## What the states mean

| state | meaning |
|---|---|
{states}

The graphs draw one mark per feature and group the states: filled dark **Proved**
(client-proved, done), filled mid **Built** (built-unproved), half-filled **Part built**
(partial, stubbed), hollow **Missing**, orange **Unknown**.

## How to read the numbers

- **Each percentage counts rows, and rows are not equal.** One row may be a window and the next a
  whole game system. Treat the figures as a direction, not a measure of effort.
- **The table lists what the reviews and plans name**, not all of EVE, so 100% would mean this
  list is finished, not the game.
- **Proved** is rows proved divided by all rows except parked ones. **Built** adds the rows with
  landed code and no client pass. Partial, stubbed, missing and unknown rows count against both.
- **A green scenario proves what it asserts and no more.** A row is about a feature; its scenario
  may touch only the main path.
- **Red does not mean broken.** Many reds have been scenario bugs. A row with a red scenario is
  built-unproved until it passes.

## Words used here

- **Scenario** (`fci_cloak`): a scripted test that drives the real client against the server.
- **Wave** (`W8` is wave 8): one run of the scenarios. Rows name the wave that last ran them.
- **Soak**: a long run of many bots. **Gate**: the checks every change passes before it lands.
- **Lane**: one parallel piece of work.
- **`ABYSS-3`, `NW-10`**: names of planned pieces of work.

## How to update this page

Do not edit this file or the SVGs: they are generated, and the gate fails if they drift.

1. Edit `docs/status/progress.tsv`. Columns, separated by tabs: `system`, `feature`, `state`,
   `evidence` (code, a commit or a scenario), `issue` (`#N`, or `#N, #M`), `theme` (a `3.N` row of
   `docs/status/themes.tsv`; required while the row is open, empty when it is finished), `note`.
2. A lane that implements or changes a feature updates its rows or adds new ones. It uses
   `built-unproved` until its scenario has passed; only a regression result may say `client-proved`.
3. Run `python3 -I tools/status-page.py`.
4. After a wave, run `python3 -I tools/status-page.py --waves`, then `--audit`, on the machine
   that holds `var/`. The audit lists rows that disagree with the newest verdicts; fix those by hand.
   A clean audit stamps the table's `# checked-at` line.
5. Evidence names a scenario with its wave (`fci_cloak W8`), or a file. Names of features only:
   no client ids and no extracted data.
"""


def readme(rows: list[dict], waves: list[dict], themes: dict[str, str], checked_at: str, tq_block: str = "") -> str:
    h = headline(rows)
    last = waves[-1] if waves else None
    out = [
        "# Status of the reimplementation",
        "",
        f"**{h['proved_pct']}% proved** ({h['proved']} of {h['counted']}) · "
        f"**{h['built_pct']}% built** ({h['built']} of {h['counted']}) · "
        f"{h['parked']} parked, not counted.",
    ]
    if last:
        out.append("")
        out.append(f"Latest client wave: **{last['wave']}**, {last['green']} of {last['rows']} passed ({last['scope']}).")
    out += [
        "",
        "![Overall: one mark per feature, proved, built, part built, missing, unknown](overall.svg)",
        "",
        "<sub>Generated by `tools/status-page.py` from `docs/status/progress.tsv`, checked against "
        f"`{checked_at}`. [How to read the numbers](#how-to-read-the-numbers) · "
        "[what the states mean](#what-the-states-mean).</sub>",
        "",
        "## What is left, by theme",
        "",
        "![One mark per open feature in each theme](left.svg)",
        "",
    ]
    left = left_rows(rows)
    for key, title in theme_order(rows, themes):
        out += [f"### {title}", ""]
        out += [row_line(r) for r in left if r["theme"] == key]
        out.append("")
    out += [
        "## By system",
        "",
        "![One mark per feature in each system](systems.svg)",
        "",
        "<details><summary>The same as a table</summary>",
        "",
        "| system | proved | built | part | missing | unknown |",
        "|---|--:|--:|--:|--:|--:|",
    ]
    for name in systems(rows):
        g = group_counts([r for r in rows if r["system"] == name])
        if sum(g.values()):
            out.append(f"| {name} | {g['proved']} | {g['built']} | {g['part']} | {g['missing']} | {g['unknown']} |")
    out += ["", "</details>", ""]
    if tq_block:
        out += [tq_block.rstrip("\n"), ""]
    out += ["## Client regression waves", ""]
    if waves:
        out += [
            "![Passed, failed and no-verdict rows of each wave](regress.svg)",
            "",
            "<details><summary>Every wave as a table</summary>",
            "",
            "| wave | date | commit | rows | passed | failed | no verdict | what ran |",
            "|--:|---|---|--:|--:|--:|--:|---|",
        ]
        for w in waves:
            note = f"{w['scope']}. {w['note']}" if w["note"] else w["scope"]
            sha = f"`{w['sha']}`" if w["sha"] else ""
            out.append(
                f"| {w['wave']} | {w['date']} | {sha} | {w['rows']} | {w['green']} "
                f"| {w['red']} | {w['no_verdict']} | {note} |"
            )
        out += [
            "",
            "</details>",
            "",
            "A wave is one run of `tools/client-ci/regress.list` against real clients. Compare whole "
            "runs with whole runs: a rerun holds only the rows that were red.",
            "",
        ]
    out += ["## Every feature, by system", "", "Closed by default; each line says how much of the system is proved.", ""]
    for name in systems(rows):
        members = [r for r in rows if r["system"] == name and r["state"] != "parked"]
        if members:
            g = group_counts(members)
            n = sum(g.values())
            tail = f"all {n} proved" if g["proved"] == n else f"{g['proved']} of {n} proved, {n - g['proved']} open"
            out += [f"<details><summary><b>{escape(name)}</b>: {tail}</summary>", ""]
            out += [row_line(r) for r in members]
            out += ["", "</details>", ""]
    out += ["## Parked", ""]
    out += [
        f"- {r['feature']}: {r['evidence']}" + (f" ({issue_text(r['issue'])})" if issue_text(r["issue"]) else "")
        for r in rows
        if r["state"] == "parked"
    ]
    out.append("")
    table = "\n".join(f"| {k} | {v} |" for k, v in STATES.items())
    return "\n".join(out) + "\n" + GUIDE.format(states=table)


def tq_block(root: str) -> str:
    """The "Tracking TQ" block from docs/status/tq-builds.tsv, or "" when there is no such file.
    A pure function of that file: the tracker count is left out so the page stays reproducible."""
    path = os.path.join(root, STATUS_DIR, "tq-builds.tsv")
    if not os.path.exists(path):
        return ""
    spec = importlib.util.spec_from_file_location("tq_lag", os.path.join(os.path.dirname(os.path.abspath(__file__)), "tq-lag.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    try:
        _, rows = mod.read(path)
    except mod.Bad as e:
        raise Bad(str(e))
    return mod.md(rows, tracker_line=False)


def build(root: str) -> dict[str, str]:
    """Every generated file, by repository path. Raises Bad on an unsound table."""
    rows, comments = read_table(os.path.join(root, STATUS_DIR, "progress.tsv"), COLUMNS)
    waves, _ = read_table(os.path.join(root, STATUS_DIR, "waves.tsv"), WAVE_COLUMNS)
    themes = plan_themes(root)
    fail = validate(rows, scenario_names(root), themes)
    if fail:
        raise Bad("\n".join(fail))
    found = [c.split()[-1] for c in comments if c.startswith("# checked-at ")]
    checked_at = found[0] if found else "unknown"
    return {
        f"{STATUS_DIR}/README.md": readme(rows, waves, themes, checked_at, tq_block(root)),
        f"{STATUS_DIR}/overall.svg": overall_svg(rows),
        f"{STATUS_DIR}/systems.svg": systems_svg(rows),
        f"{STATUS_DIR}/left.svg": left_svg(rows, themes),
        f"{STATUS_DIR}/regress.svg": regress_svg(waves),
    }


def stale(root: str, files: dict[str, str]) -> list[str]:
    out = []
    for rel, content in files.items():
        path = os.path.join(root, rel)
        if not os.path.exists(path):
            out.append(rel)
            continue
        with open(path, encoding="utf-8") as f:
            if f.read() != content:
                out.append(rel)
    return out


def regress_dirs(var: str) -> list[tuple[str, str]]:
    """Every var/regress-*/regress.tsv, oldest first by mtime, as (run name, path)."""
    found = []
    for path in glob.glob(os.path.join(var, "regress-*/regress.tsv")):
        name = os.path.basename(os.path.dirname(path))[len("regress-"):]
        found.append((os.path.getmtime(path), name, path))
    return [(name, path) for _, name, path in sorted(found)]


def wave_dirs(var: str) -> list[tuple[str, str]]:
    found = []
    for path in glob.glob(os.path.join(var, "regress-wave*/regress.tsv")):
        m = re.search(r"regress-wave(\d+)/", path)
        if m:
            found.append((m.group(1), path))
    return sorted(found, key=lambda p: int(p[0]))


def read_verdicts(path: str) -> dict[str, str]:
    """name -> exit code of one wave. 245 is the runner's "no verdict"."""
    out = {}
    with open(path, encoding="utf-8") as f:
        for line in f:
            cells = line.rstrip("\n").split("\t")
            if len(cells) >= 2:
                out[cells[0]] = cells[1]
    return out


def newest_verdicts(paths: list[tuple[str, str]]) -> dict[str, tuple[str, bool]]:
    """name -> (run, passed) from the newest run that gave it a verdict; paths oldest first."""
    latest = {}
    for wave, path in paths:
        for name, code in read_verdicts(path).items():
            if code != "245":
                latest[name] = (wave, code == "0")
    return latest


def audit_items(rows: list[dict], known: set[str], latest: dict[str, tuple[str, bool]],
                listed: list[str]) -> list[tuple[str | None, str, bool, str]]:
    """Where the table and the regression results disagree, as (feature, text, from_verdicts, scenario).

    `feature` is the row the finding is about (None for a scenario no row cites, which names
    that scenario in `scenario`);
    `from_verdicts` is True when the finding rests on the verdicts in `var/`, which only
    some machines hold, and False when the committed tree alone decides it.
    """
    out: list[tuple[str | None, str, bool, str]] = []
    claimed = set()
    for r in rows:
        cited = scenarios_in(r["evidence"], known)
        claimed.update(cited)
        seen = [latest[c] for c in cited if c in latest]
        red = [c for c in cited if c in latest and not latest[c][1]]
        f = r["feature"]
        if r["state"] in FINISHED and red:
            out.append((f, f"{f}: {r['state']}, but the newest verdict is red: "
                        + ", ".join(f"{c} ({latest[c][0]})" for c in red), True, ""))
        elif r["state"] in ("built-unproved", "missing", "unknown") and seen and not red:
            out.append((f, f"{f}: {r['state']}, but the newest verdict is green: "
                        + ", ".join(f"{c} ({latest[c][0]})" for c in cited if c in latest), True, ""))
        if r["state"] == "client-proved" and cited and not seen:
            out.append((f, f"{f}: no verdict for {', '.join(cited)}", True, ""))
    bare = {c.split("@")[0] for c in claimed}
    for name in list(listed) + sorted(n for n in latest if n not in listed):
        if name in NOT_FEATURES or name.split("@")[0] in NOT_FEATURES:
            continue
        if name not in claimed and name.split("@")[0] not in bare:
            if name in listed:
                out.append((None, f"{name}: a regression row no feature cites", False, name))
            else:
                out.append((None, f"{name}: a scenario with a verdict ({latest[name][0]}) no feature cites", True, name))
    return out


def audit(rows: list[dict], known: set[str], latest: dict[str, tuple[str, bool]], listed: list[str]) -> list[str]:
    """Where the table and the regression results disagree."""
    return [text for _f, text, _v, _s in audit_items(rows, known, latest, listed)]


class Changed(NamedTuple):
    """What a range changed: rows (new or different), the scenarios the base's rows cited, and
    the scenarios whose file changed."""
    rows: set[str]
    cited: set[str]
    scenarios: set[str]


def changed_features(root: str, base: str) -> Changed | None:
    """What changed since `base`; None if `base` cannot be read."""
    try:
        old = subprocess.run(["git", "-C", root, "show", f"{base}:{STATUS_DIR}/progress.tsv"],
                             capture_output=True, text=True, check=True, timeout=60).stdout
        files = subprocess.run(["git", "-C", root, "diff", "--name-only", "--no-renames", base, "HEAD", "--",
                                "tools/client-ci/scenarios"],
                               capture_output=True, text=True, check=True, timeout=60).stdout.split()
    except (OSError, subprocess.SubprocessError):
        return None
    with tempfile.TemporaryDirectory(prefix="eve-status-base-") as d:
        path = os.path.join(d, "progress.tsv")
        with open(path, "w", encoding="utf-8") as f:
            f.write(old)
        try:
            before, _ = read_table(path, COLUMNS)
        except Bad:
            return None
    was = {r["feature"]: [r[c] for c in COLUMNS] for r in before}
    now, _ = read_table(os.path.join(root, STATUS_DIR, "progress.tsv"), COLUMNS)
    known = scenario_names(root)
    cited = {c.split("@")[0] for r in before for c in scenarios_in(r["evidence"], known)}
    scenarios = {os.path.basename(f)[:-3] for f in files if f.endswith(".py")}
    return Changed({r["feature"] for r in now if was.get(r["feature"]) != [r[c] for c in COLUMNS]}, cited, scenarios)


def split_findings(items: list[tuple[str | None, str, bool, str]],
                   changed: "Changed | set[str] | None") -> tuple[list[str], list[str]]:
    """(failures, notes). Without `changed` every finding fails; with it, a finding that rests on
    verdicts fails only for a row in `changed`, or, for a scenario no row cites, one that a row
    cited at the base or whose file changed in the range (an un-cited or edited scenario)."""
    if isinstance(changed, set):
        changed = Changed(changed, set(), set())
    fail, notes = [], []
    for feature, text, from_verdicts, scenario in items:
        if changed is None or not from_verdicts:
            fail.append(text)
            continue
        bare = scenario.split("@")[0]
        if feature is None:
            hit = bare in changed.cited or bare in changed.scenarios
        else:
            hit = feature in changed.rows
        (fail if hit else notes).append(text)
    return fail, notes


def listed_rows(root: str) -> list[str]:
    out = []
    with open(os.path.join(root, "tools/client-ci/regress.list"), encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line and not line.startswith("#"):
                out.append(line.split()[0])
    return out


def update_waves(root: str, var: str) -> int:
    """Refresh the counts in waves.tsv from var/, keeping what a person wrote.

    A wave with no results here keeps its row, so a worktree without `var/`
    cannot empty the table.
    """
    path = os.path.join(root, STATUS_DIR, "waves.tsv")
    old = {}
    comments: list[str] = []
    if os.path.exists(path):
        rows, comments = read_table(path, WAVE_COLUMNS)
        old = {r["wave"]: r for r in rows}
    found = wave_dirs(var)
    if not found:
        raise Bad(f"no regress-wave*/regress.tsv under {var}")
    out = []
    for wave, tsv in found:
        codes = list(read_verdicts(tsv).values())
        none = sum(c == "245" for c in codes)
        green = sum(c == "0" for c in codes)
        row = old.get(wave, {"date": "", "sha": "", "scope": "full", "note": ""})
        out.append([wave, row["date"], row["sha"], str(len(codes)), str(green),
                    str(len(codes) - green - none), str(none), row["scope"], row["note"]])
    fresh = {r[0] for r in out}
    out += [[r[c] for c in WAVE_COLUMNS] for w, r in old.items() if w not in fresh]
    out.sort(key=lambda r: int(r[0]))
    with open(path, "w", encoding="utf-8") as f:
        for c in comments:
            f.write(c + "\n")
        f.write("\t".join(WAVE_COLUMNS) + "\n")
        for r in out:
            f.write("\t".join(r) + "\n")
    return len(out)


def stamp_checked_at(root: str) -> None:
    """Rewrite the table's `# checked-at` line to the short HEAD (a clean audit only)."""
    try:
        head = subprocess.run(["git", "-C", root, "rev-parse", "--short", "HEAD"], capture_output=True,
                              text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return
    path = os.path.join(root, STATUS_DIR, "progress.tsv")
    with open(path, encoding="utf-8") as f:
        text = f.read()
    text = re.sub(r"^# checked-at \S+$", f"# checked-at {head}", text, count=1, flags=re.M)
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    print(f"status-page: checked-at {head}")


def main(argv: list[str] | None = None, root: str = ROOT) -> int:
    p = argparse.ArgumentParser(description="Build the progress page.")
    p.add_argument("--check", action="store_true", help="fail if a generated file is stale")
    p.add_argument("--audit", action="store_true", help="compare rows with var/regress-*")
    p.add_argument("--waves", action="store_true", help="refresh waves.tsv from var/regress-wave*")
    p.add_argument("--var", default=os.path.join(root, "var"), help="where the wave results are (default: var/)")
    p.add_argument("--since", metavar="BASE",
                   help="with --audit: a finding from var/ verdicts fails only for a row changed since BASE")
    p.add_argument("--no-stamp", action="store_true", help="with --audit: report only, never rewrite a file")
    args = p.parse_args(argv)
    try:
        if args.waves:
            print(f"status-page: {update_waves(root, args.var)} waves in waves.tsv")
        files = build(root)
    except (Bad, OSError) as e:
        print(f"status-page: {e}", file=sys.stderr)
        return 1
    if args.audit:
        if not regress_dirs(args.var):
            print(f"status-page: audit skipped, no regress-*/regress.tsv under {args.var}")
            return 0
        rows, _ = read_table(os.path.join(root, STATUS_DIR, "progress.tsv"), COLUMNS)
        items = audit_items(rows, scenario_names(root), newest_verdicts(regress_dirs(args.var)), listed_rows(root))
        changed = None
        if args.since:
            changed = changed_features(root, args.since)
            if changed is None:
                print(f"status-page: cannot read the table at {args.since}; every finding fails")
        found, notes = split_findings(items, changed)
        for line in found:
            print(line)
        for line in notes:
            print(f"status-page: note: {line}")
        print(f"status-page: {len(found)} to look at")
        if notes:
            print(f"status-page: {len(notes)} note(s) on rows this range did not change; "
                  "tools/gate.sh --env-checks fails on them")
        if not found and not notes and not args.no_stamp:
            stamp_checked_at(root)
            for rel, content in build(root).items():
                with open(os.path.join(root, rel), "w", encoding="utf-8") as f:
                    f.write(content)
        return 1 if found else 0
    if args.check:
        bad = stale(root, files)
        if bad:
            print("status-page: stale, run python3 -I tools/status-page.py: " + ", ".join(bad), file=sys.stderr)
            return 1
        print("status-page: current")
        return 0
    for rel, content in files.items():
        with open(os.path.join(root, rel), "w", encoding="utf-8") as f:
            f.write(content)
    print(f"status-page: wrote {len(files)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main())
