#!/usr/bin/env python3
"""Poll the admin page's /api/metrics into a long-format TSV (#201, for #100).

    python3 -I tools/metrics_sample.py --url http://127.0.0.1:8090 \
        --token-file var/battle100/admin.token --out var/battle100/p0/metrics.tsv \
        [--every 10] [--secs N]

Every --every seconds it GETs /api/metrics with the X-Admin-Token header (the
token is read from the file and never printed) and appends one row per sample:

    unix<TAB>name<TAB>labels<TAB>value

`labels` is the text between the braces exactly as the server wrote it
(`system="39000001",kind="human"`), empty when there are none. It stops after
--secs seconds (default: run until stopped) or when the server has not
answered 3 times in a row. Stdlib only; start the server with
`--admin ADDR` (or EVE_ADMIN_ADDR). Self-test: tools/test_metrics_sample.py.
"""

from __future__ import annotations

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` (#126). Only when run as a script.
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import sys
import time
import urllib.error
import urllib.request

MISSES = 3  # consecutive failed polls before it gives up


def parse_exposition(text: str) -> list[tuple[str, str, str]]:
    """`(name, labels, value)` for each sample line of Prometheus text.

    Comment and blank lines are skipped. A label value may hold spaces and
    braces (the server escapes only backslash, quote and newline), so the
    closing brace is found by walking the quoted values.
    """
    out = []
    for line in text.splitlines():
        if not line or line.startswith("#"):
            continue
        brace = line.find("{")
        space = line.find(" ")
        if brace != -1 and (space == -1 or brace < space):
            name = line[:brace]
            i, quoted = brace + 1, False
            while i < len(line):
                c = line[i]
                if quoted and c == "\\":
                    i += 1
                elif c == '"':
                    quoted = not quoted
                elif c == "}" and not quoted:
                    break
                i += 1
            else:
                continue
            labels, rest = line[brace + 1 : i], line[i + 1 :].strip()
        elif space != -1:
            name, labels, rest = line[:space], "", line[space + 1 :].strip()
        else:
            continue
        value = rest.split(" ")[0]
        if name and value:
            out.append((name, labels.replace("\t", "\\t"), value))
    return out


def rows_for(unix: int, text: str) -> list[str]:
    return [f"{unix}\t{n}\t{labels}\t{v}\n" for n, labels, v in parse_exposition(text)]


def fetch(url: str, token: str, timeout: float = 10.0) -> str:
    req = urllib.request.Request(
        url.rstrip("/") + "/api/metrics", headers={"X-Admin-Token": token}
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:  # noqa: S310 (operator-given URL)
        return r.read().decode("utf-8", "replace")


def run(url: str, token: str, out_path: str, every: float, secs: float | None) -> int:
    start = time.monotonic()
    misses = 0
    with open(out_path, "a", encoding="utf-8") as out:
        if out.tell() == 0:
            out.write("#unix\tname\tlabels\tvalue\n")
        while secs is None or time.monotonic() - start < secs:
            tick = time.monotonic()
            try:
                body = fetch(url, token)
                misses = 0
                out.writelines(rows_for(int(time.time()), body))
                out.flush()
            except (urllib.error.URLError, OSError, ValueError) as e:
                misses += 1
                print(f"metrics_sample: no answer ({type(e).__name__})", file=sys.stderr)
                if misses >= MISSES:
                    print("metrics_sample: server stopped answering", file=sys.stderr)
                    return 1
            time.sleep(max(0.0, every - (time.monotonic() - tick)))
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--url", required=True, help="the admin base URL, e.g. http://127.0.0.1:8090")
    ap.add_argument("--token-file", required=True, help="the admin.token file")
    ap.add_argument("--out", required=True, help="the TSV to append to")
    ap.add_argument("--every", type=float, default=10.0, help="seconds between polls")
    ap.add_argument("--secs", type=float, default=None, help="stop after this many seconds")
    a = ap.parse_args(argv)
    if a.every <= 0:
        ap.error("--every must be positive")
    with open(a.token_file, encoding="utf-8") as f:
        token = f.read().strip()
    return run(a.url, token, a.out, a.every, a.secs)


if __name__ == "__main__":
    sys.exit(main())
