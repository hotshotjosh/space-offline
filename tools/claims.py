#!/usr/bin/env python3
"""Claims on the shared issue tracker: list tasks, check paths, claim, release.

Stdlib only. Works against GitHub REST or Gitea REST (tracker-neutral). It is
read-only unless a write subcommand (`claim`, `release`, `status`) is given.

Configuration (first match wins):
  EVE_TRACKER       gitea | github
  EVE_TRACKER_API   e.g. https://api.github.com or https://gitea.example/api/v1
  EVE_TRACKER_REPO  owner/name
  otherwise the `upstream` remote if there is one, else `origin` (github.com ->
  GitHub, any other host -> Gitea at https://<host>/api/v1). A GitHub remote that
  is a fork resolves to its parent repo; one GET asks GitHub, and the answer is
  cached in `eve-tracker` in the git common dir, keyed by host and repo. If that call fails the
  remote's own repo is used, with a note on stderr.

The token comes from EVE_TRACKER_TOKEN, else the file named in
EVE_TRACKER_TOKEN_FILE, else GH_TOKEN, else GITHUB_TOKEN (those two only when the API is
https://api.github.com). It is
sent only in the Authorization header and never printed. Reads work without a
token on a public repo.

A claim is an issue comment whose first line is `/claim`, with a line
`Owns: p1, p2, glob/*`. The earliest live `/claim` wins and its author is the
holder; a later `/claim` by the holder REPLACES the Owns list (this is how a
claim is widened or narrowed; `claim` re-checks overlaps first), while a later
`/claim` by anyone else is ignored. A `/release` by the holder ends the claim. Where the token may, the tool also assigns the holder and sets
the label `status/claimed`; on GitHub without triage rights the comment alone
is the claim.

Exit codes: 0 ok; 1 nothing (no task, not claimed, not mine); 2 tracker error
or no token; 3 path overlap; 4 someone else holds it (or it is closed,
assigned or needs-owner); 5 a hot file without the `hot-ok` label; 6 a body to post
matches the scan (the built-in patterns plus the optional private scan lists): refused, with
the line and pattern but never the matched text, and nothing is posted.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))


import argparse
import datetime as dt
import fnmatch
import importlib.util
import os
import re
import sys
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))


def _sibling(name):
    """A module of tools/ loaded by path (tools/ holds a dis.py that shadows the stdlib),
    shared through sys.modules so tests and other tools see one copy."""
    if name in sys.modules:
        return sys.modules[name]
    spec = importlib.util.spec_from_file_location(name, os.path.join(HERE, name + ".py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


# The read-only code lives in tools/tracker.py and the path overlap code in
# tools/pr_overlap.py; the names below stay importable from here.
tracker = _sibling("tracker")
pr_overlap = _sibling("pr_overlap")

ROOT, CACHE, CACHE_TTL = tracker.ROOT, tracker.CACHE, tracker.CACHE_TTL
CONTRIBUTING = pr_overlap.CONTRIBUTING
EXEMPT = pr_overlap.EXEMPT
TrackerError, NoToken, LeakRefused = tracker.TrackerError, tracker.NoToken, tracker.LeakRefused
scan_lines, export_patterns, leak_lines = (tracker.scan_lines, tracker.export_patterns,
                                           tracker.leak_lines)
git, parse_remote, cache_file, github_get = (tracker.git, tracker.parse_remote,
                                             tracker.cache_file, tracker.github_get)
resolve_repo, config, read_token = tracker.resolve_repo, tracker.config, tracker.read_token
labels_of, parse_time, pr_issue_refs = tracker.labels_of, tracker.parse_time, tracker.pr_issue_refs
hot_files = pr_overlap.hot_table   # fails closed: None when the table cannot be read
SERVER_SRC = pr_overlap.SERVER_SRC

DROP_LABELS = {"status/claimed", "needs-owner", "needs-human", "parked", "status/blocked"}
STALE_DAYS = 7


class Http(tracker.Http):
    """The tracker transport with writes: claims are comments, labels and assignees."""

    writable = True

    def request(self, method, path, body=None):
        return self._send(method, path, body)


class Tracker(tracker.Tracker):
    def __init__(self, cfg, transport):
        super().__init__(cfg, transport)
        self._label_ids = None
        self.scan = None   # compiled export patterns; loaded on the first post

    def check_body(self, body):
        if self.scan is None:
            self.scan = export_patterns(post=True)
        hits = leak_lines(body, self.scan)
        if hits:
            raise LeakRefused(hits)

    def comments(self, n):
        return self.paged(f"{self.r}/issues/{n}/comments")

    # writes
    def comment(self, n, body):
        self.check_body(body)
        self.t.request("POST", f"{self.r}/issues/{n}/comments", {"body": body})

    def assign(self, n, add):
        me = self.me()
        if self.gh:
            m = "POST" if add else "DELETE"
            self.t.request(m, f"{self.r}/issues/{n}/assignees", {"assignees": [me]})
        else:
            cur = [a["login"] for a in (self.issue(n).get("assignees") or [])]
            new = sorted(set(cur) | {me}) if add else [a for a in cur if a != me]
            self.t.request("PATCH", f"{self.r}/issues/{n}", {"assignees": new})

    def label_id(self, name):
        if self._label_ids is None:
            self._label_ids = {lb["name"]: lb["id"] for lb in self.paged(f"{self.r}/labels")}
        if name not in self._label_ids:
            raise TrackerError("no-label", "GET", f"{self.r}/labels ({name})")
        return self._label_ids[name]

    def add_label(self, n, name):
        val = name if self.gh else self.label_id(name)
        self.t.request("POST", f"{self.r}/issues/{n}/labels", {"labels": [val]})

    def remove_label(self, n, name):
        val = urllib.parse.quote(name, safe="") if self.gh else self.label_id(name)
        self.t.request("DELETE", f"{self.r}/issues/{n}/labels/{val}")




# ---------------------------------------------------------------- parsing


def assignees_of(issue):
    return [a["login"] for a in issue.get("assignees") or []]


def body_field(issue, name):
    m = re.search(rf"^\s*{name}:\s*(.+?)\s*$", issue.get("body") or "", re.M | re.I)
    return m.group(1) if m else ""


def split_paths(s):
    return [p.strip().strip("`") for p in re.split(r"[,\s]+", s or "") if p.strip().strip("`")]

def live_claim(comments):
    """The live claim: {holder, owns, at} or None.

    The earliest /claim sets the holder and `at`. A later /claim by that
    holder replaces `owns` (one with no parseable Owns: line changes
    nothing); a later /claim by anyone else is ignored.
    """
    holder = None
    for c in sorted(comments, key=lambda c: (c.get("created_at") or "", c.get("id") or 0)):
        text = (c.get("body") or "").strip()
        first = text.split("\n", 1)[0].strip()
        who = (c.get("user") or {}).get("login")
        if first == "/claim" and (holder is None or who == holder["holder"]):
            m = re.search(r"^\s*Owns:\s*(.+)$", text, re.M)
            owns = split_paths(m.group(1) if m else "")
            if holder is None:
                holder = {"holder": who, "owns": owns, "at": c.get("created_at")}
            elif owns:
                holder["owns"] = owns
        elif first == "/release" and holder is not None and who == holder["holder"]:
            holder = None
    return holder


def exempt(p):
    return any(p == e or fnmatch.fnmatch(p, e) for e in EXEMPT)


def overlaps(a, b):
    """Two path patterns overlap: either matches the other, or one is a dir prefix."""
    if exempt(a) or exempt(b):
        return False
    a2, b2 = a.rstrip("/"), b.rstrip("/")
    if a2 == b2 or fnmatch.fnmatch(a2, b2) or fnmatch.fnmatch(b2, a2):
        return True
    return b2.startswith(a2 + "/") or a2.startswith(b2 + "/")


def now():
    return dt.datetime.now(dt.timezone.utc)


# ---------------------------------------------------------------- commands


def all_claims(tr):
    """{issue number: claim} for every open issue with a live /claim."""
    out = {}
    for i in tr.open_issues():
        if not i.get("comments"):
            continue
        c = live_claim(tr.comments(i["number"]))
        if c:
            c["issue"] = i
            out[i["number"]] = c
    return out



def cmd_list(tr, a, out):
    pulls = tr.open_pulls()
    print("issue\tholder\tage_days\tstale\tpr\towns", file=out)
    for n, c in sorted(all_claims(tr).items()):
        at = parse_time(c["at"])
        age = (now() - at).days if at else 0
        pr = next((p for p in pulls if n in pr_issue_refs(p)), None)
        upd = parse_time(pr.get("updated_at")) if pr else None
        stale = age > STALE_DAYS and (upd is None or (now() - upd).days > STALE_DAYS)
        print(f"{n}\t{c['holder']}\t{age}\t{'yes' if stale else 'no'}\t"
              f"{pr['number'] if pr else 'none'}\t{','.join(c['owns'])}", file=out)
    return 0


def cmd_show(tr, a, out):
    c = live_claim(tr.comments(a.n))
    if not c:
        print(f"#{a.n}: unclaimed", file=out)
        return 1
    pr = next((p["number"] for p in tr.open_pulls() if a.n in pr_issue_refs(p)), "none")
    print(f"issue={a.n}\nholder={c['holder']}\nowns={','.join(c['owns'])}\npr={pr}", file=out)
    if a.holder_is_me and c["holder"] != tr.me():
        return 1
    return 0


def find_overlaps(tr, owns, issue=None):
    clashes = []
    for n, c in all_claims(tr).items():
        if n == issue:
            continue
        for p in owns:
            for q in c["owns"]:
                if overlaps(p, q):
                    clashes.append((n, c["holder"], p, q))
    for pr in tr.open_pulls():
        if issue is not None and issue in pr_issue_refs(pr):
            continue
        who = (pr.get("user") or {}).get("login")
        for f in tr.pull_files(pr["number"]):
            for p in owns:
                if overlaps(p, f):
                    clashes.append((f"PR {pr['number']}", who, p, f))
    return clashes


def cmd_check(tr, a, out):
    clashes = find_overlaps(tr, split_paths(a.owns), a.issue)
    for where, who, p, q in clashes:
        print(f"overlap\t{where}\t{who}\t{p}\t{q}", file=out)
    return 3 if clashes else 0


def cmd_next(tr, a, out):
    rows = []
    open_cache = {}

    def is_open(n):
        if n not in open_cache:
            try:
                open_cache[n] = tr.issue(n).get("state") == "open"
            except TrackerError:
                open_cache[n] = False
        return open_cache[n]

    for i in tr.open_issues(["status/ready"]):
        labels = labels_of(i)
        if assignees_of(i) or labels & DROP_LABELS:
            continue
        if a.no_client and "needs-client" in labels:
            continue
        area = next((x[5:] for x in labels if x.startswith("area/")), "") or body_field(i, "Area")
        if a.area and area != a.area:
            continue
        blocked = [int(x) for x in re.findall(r"#(\d+)", body_field(i, "Blocked by"))]
        if any(is_open(b) for b in blocked):
            continue
        if i.get("comments") and live_claim(tr.comments(i["number"])):
            continue
        prio = next((int(x[5:]) for x in sorted(labels) if re.fullmatch(r"prio/[123]", x)), 9)
        due = ((i.get("milestone") or {}).get("due_on")) or "9999"
        first = 0 if (a.first and "good-first" in labels) else 1
        rows.append(((prio, due, first, i["number"]), i, area, labels))
    rows.sort(key=lambda r: r[0])
    if a.limit:
        rows = rows[:a.limit]
    if not rows:
        print("no task: nothing open, ready, unclaimed and unblocked", file=out)
        return 1
    print("issue\tprio\tarea\tclient\ttitle\tread\towns", file=out)
    for key, i, area, labels in rows:
        p = f"prio/{key[0]}" if key[0] < 9 else "-"
        cl = "yes" if "needs-client" in labels else "no"
        print(f"{i['number']}\t{p}\t{area or '-'}\t{cl}\t{i.get('title', '')}\t"
              f"{body_field(i, 'Read') or '-'}\t{body_field(i, 'Owns') or '-'}", file=out)
    return 0


def soft(tr, out, what, fn, *args):
    """A write that GitHub may refuse without triage rights; the comment stands."""
    try:
        fn(*args)
    except TrackerError as e:
        if tr.gh and e.status in (403, 404, 422):
            print(f"note: could not {what} ({e.status}); the /claim comment alone is the claim",
                  file=out)
        else:
            raise


def cmd_claim(tr, a, out, hot=None):
    owns = split_paths(a.owns)
    if not owns:
        print("claim needs --owns", file=out)
        return 3
    me = tr.me()
    i = tr.issue(a.n)
    labels = labels_of(i)
    held = live_claim(tr.comments(a.n)) if i.get("comments") else None
    others = [x for x in assignees_of(i) if x != me]
    if (i.get("state") != "open" or others or "needs-owner" in labels
            or (held and held["holder"] != me)
            or ("status/claimed" in labels and not (held and held["holder"] == me))):
        print(f"#{a.n}: closed, assigned, claimed or needs-owner; pick another", file=out)
        return 4
    hot = hot_files() if hot is None else hot
    if hot is None:
        print("could not read the hot-file table in docs/guides/contributing.md §10; "
              "a maintainer must fix the doc or tools/claims.py", file=out)
        return 5
    hit = [(p, h) for p in owns for h in hot if overlaps(p, h)]
    if hit and "hot-ok" not in labels:
        for p, h in hit:
            print(f"hot\t{p}\t{h}", file=out)
        print("a hot file needs the label hot-ok from a maintainer", file=out)
        return 5
    clashes = find_overlaps(tr, owns, a.n)
    if clashes:
        for where, who, p, q in clashes:
            print(f"overlap\t{where}\t{who}\t{p}\t{q}", file=out)
        return 3
    if not held or sorted(set(held["owns"])) != sorted(set(owns)):
        tr.comment(a.n, "/claim\nOwns: " + ", ".join(owns))
    if not held:
        # A widening keeps the issue's status label (claimed, review, ...).
        soft(tr, out, "assign", tr.assign, a.n, True)
        if "status/ready" in labels:
            soft(tr, out, "remove status/ready", tr.remove_label, a.n, "status/ready")
        soft(tr, out, "label status/claimed", tr.add_label, a.n, "status/claimed")
    after = live_claim(tr.comments(a.n))
    if not after or after["holder"] != me:
        tr.comment(a.n, "/release")
        print(f"#{a.n}: someone claimed first; released", file=out)
        return 4
    print(f"claimed #{a.n} as {me}; owns {', '.join(owns)}", file=out)
    return 0


def cmd_release(tr, a, out):
    tr.comment(a.n, "/release")
    soft(tr, out, "unassign", tr.assign, a.n, False)
    i = tr.issue(a.n)
    labels = labels_of(i)
    if "status/claimed" in labels:
        soft(tr, out, "remove status/claimed", tr.remove_label, a.n, "status/claimed")
    if i.get("state") == "open" and "status/ready" not in labels:
        soft(tr, out, "label status/ready", tr.add_label, a.n, "status/ready")
    print(f"released #{a.n}", file=out)
    return 0


def cmd_status(tr, a, out):
    if a.why:
        tr.check_body(f"{a.state}: {a.why}")   # before any label write
    labels = labels_of(tr.issue(a.n))
    if a.state == "needs-human":
        soft(tr, out, "label needs-human", tr.add_label, a.n, "needs-human")
    else:
        want = f"status/{a.state}"
        for lb in sorted(labels):
            if lb.startswith("status/") and lb != want:
                soft(tr, out, f"remove {lb}", tr.remove_label, a.n, lb)
        if want not in labels:
            soft(tr, out, f"label {want}", tr.add_label, a.n, want)
    if a.why:
        tr.comment(a.n, f"{a.state}: {a.why}")
    print(f"#{a.n}: {a.state}", file=out)
    return 0


cmd_whoami, cmd_repo, cmd_pr_for_branch = (tracker.cmd_whoami, tracker.cmd_repo,
                                            tracker.cmd_pr_for_branch)


def parser():
    ap = argparse.ArgumentParser(prog="claims.py", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("whoami")
    sub.add_parser("repo")
    sub.add_parser("list")
    s = sub.add_parser("show")
    s.add_argument("n", type=int)
    s.add_argument("--holder-is-me", action="store_true")
    s = sub.add_parser("check")
    s.add_argument("--owns", required=True)
    s.add_argument("--issue", type=int)
    s = sub.add_parser("next")
    s.add_argument("--no-client", action="store_true")
    s.add_argument("--area")
    s.add_argument("--first", action="store_true")
    s.add_argument("--limit", type=int, default=5)
    s = sub.add_parser("claim")
    s.add_argument("n", type=int)
    s.add_argument("--owns", required=True)
    s = sub.add_parser("release")
    s.add_argument("n", type=int)
    s = sub.add_parser("status")
    s.add_argument("n", type=int)
    s.add_argument("state", choices=["review", "needs-human", "blocked"])
    s.add_argument("--why")
    s = sub.add_parser("pr-for-branch")
    s.add_argument("--branch")
    return ap


COMMANDS = {
    "whoami": cmd_whoami, "list": cmd_list, "show": cmd_show, "check": cmd_check,
    "next": cmd_next, "claim": cmd_claim, "release": cmd_release, "status": cmd_status,
    "pr-for-branch": cmd_pr_for_branch, "repo": cmd_repo,
}


def main(argv=None, tracker=None, out=None, err=None):
    out = out or sys.stdout
    err = err or sys.stderr
    a = parser().parse_args(argv)
    try:
        if tracker is None:
            cfg = config()
            tracker = Tracker(cfg, Http(cfg, read_token(api=cfg["api"])))
        return COMMANDS[a.cmd](tracker, a, out)
    except NoToken:
        print("no-token: set GH_TOKEN (or GITHUB_TOKEN; `gh auth login` and "
              "GH_TOKEN=$(gh auth token) is the usual way), or EVE_TRACKER_TOKEN / "
              "EVE_TRACKER_TOKEN_FILE for another tracker (or ask your human to run "
              "this command)", file=err)
        return 2
    except TrackerError as e:
        print(str(e), file=err)
        return 2
    except LeakRefused as e:
        for h in e.hits:
            print(f"refused: the text to post {h} (public-export scan); nothing was posted",
                  file=err)
        return 6


if __name__ == "__main__":
    sys.exit(main())

