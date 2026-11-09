#!/usr/bin/env python3
"""Read-only access to the shared issue tracker: who and where, issues, a branch's PR.

Stdlib only. Works against GitHub REST or Gitea REST (tracker-neutral). Every call is a
GET: this module never writes to the tracker, and its transport refuses any other method.
Issue titles are printed cleaned; issue and PR bodies are never printed.

Subcommands: whoami, repo, pr-for-branch, issues. See `issues --help`.

Configuration (first match wins):
  EVE_TRACKER       gitea | github
  EVE_TRACKER_API   e.g. https://api.github.com or https://gitea.example/api/v1
  EVE_TRACKER_REPO  owner/name
  otherwise the `upstream` remote if there is one, else `origin` (github.com ->
  GitHub, any other host -> Gitea at https://<host>/api/v1). A GitHub remote that
  is a fork resolves to its parent repo; one GET asks GitHub, and the answer is
  cached in `eve-tracker` in the git common dir, keyed by host and repo. If that call
  fails the remote's own repo is used, with a note on stderr.

The token comes from EVE_TRACKER_TOKEN, else the file named in
EVE_TRACKER_TOKEN_FILE, else GH_TOKEN, else GITHUB_TOKEN (those two only when the API is
https://api.github.com). It is sent only in the Authorization header and never printed.
Reads work without a token on a public repo.

Exit codes: 0 ok; 1 nothing (no task, no PR for the branch); 2 tracker error or no token.
"""

# Move `tools/` to the end of `sys.path` so the standard library wins; see
# `tools/dis.py` for why every tool here carries this line. Only when run as
# a script: imported, `sys.path[0]` is not ours to move (#126).
import sys as _sys

if __name__ == "__main__":
    _sys.path.append(_sys.path.pop(0))

import argparse
import datetime as dt
import importlib.util
import json
import os
import re
import subprocess
import sys
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
CACHE = os.path.join(ROOT, "out", "tracker-cache.json")
CACHE_TTL = 60
CONTRIBUTING = os.path.join(ROOT, "docs", "guides", "contributing.md")


class TrackerError(Exception):
    """A tracker call failed. Carries the HTTP status, method and path only."""

    def __init__(self, status, method, path):
        super().__init__(f"tracker error: {status} {method} {path}")
        self.status = status


class NoToken(Exception):
    pass


class LeakRefused(Exception):
    """A body to post matches the public export's scan. Holds line numbers and patterns only."""

    def __init__(self, hits):
        super().__init__("; ".join(hits))
        self.hits = hits


def scan_lines(path):
    """(line number, regex text) of each pattern line of a scan file; none when it is missing."""
    try:
        with open(path, encoding="utf-8") as f:
            return [(n, t.strip()) for n, t in enumerate(f, 1)
                    if t.strip() and not t.lstrip().startswith("#")]
    except OSError:
        return []


def export_patterns(root=None, post=False):
    """The scan, as (compiled, label) pairs: the export's BUILT_IN patterns plus
    private/export-scan.txt (optional), and with post=True an optional extra list. A
    BUILT_IN pattern's label is its text; a private pattern's label is
    `private pattern <file>#<line>`, never its text. A malformed pattern refuses
    everything (LeakRefused): a scan that cannot run must not pass."""
    root = root or ROOT
    spec = importlib.util.spec_from_file_location(
        "public_export", os.path.join(os.path.dirname(os.path.abspath(__file__)), "public-export.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    items = [(t, t, f"BUILT_IN#{k}") for k, t in enumerate(mod.BUILT_IN, 1)]
    names = ["export-scan.txt"] + (["post-scan.txt"] if post else [])
    for name in names:
        for n, t in scan_lines(os.path.join(root, mod.PRIVATE_DIR, name)):
            items.append((t, f"private pattern {name}#{n}", f"{name}#{n}"))
    out = []
    for t, label, where in items:
        try:
            out.append((re.compile(t), label))
        except re.error:
            raise LeakRefused([f"scan pattern {where} is malformed; nothing scanned"]) from None
    return out


def leak_lines(text, pats):
    """One entry per line that matches, never the matched text. pats: (compiled, label)
    pairs, or bare compiled patterns (labelled with their text)."""
    hits = []
    for n, line in enumerate((text or "").split("\n"), 1):
        for p in pats:
            pat, label = p if isinstance(p, tuple) else (p, p.pattern)
            if pat.search(line):
                hits.append(f"line {n}: matches /{label}/" if not label.startswith("private ")
                            else f"line {n}: matches {label}")
                break
    return hits


# ---------------------------------------------------------------- config


def git(*args):
    try:
        return subprocess.run(["git", "-C", ROOT, *args], capture_output=True, text=True,
                              timeout=10).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return ""


GITHUB_API = "https://api.github.com"
GITHUB_HOSTS = ("github.com", "www.github.com", "ssh.github.com")


def parse_remote(url):
    """(host, 'owner/name') from a git remote URL (https, ssh or scp form), or None."""
    m = re.match(r"(?:\w+://)?(?:[^@/]+@)?([^/:]+)(?::\d+)?[:/](.+?)(?:\.git)?/?$", url or "")
    if not m:
        return None
    host = m.group(1).lower()
    if host in GITHUB_HOSTS:
        host = "github.com"
    return host, "/".join(m.group(2).split("/")[-2:])


def cache_file(run=None):
    """`eve-tracker` in the common git dir (`git rev-parse --git-common-dir`), so every
    worktree of a clone shares one cache."""
    d = (run or git)("rev-parse", "--git-common-dir")
    if not d:
        return None
    return os.path.join(ROOT, d, "eve-tracker")


def github_get(url, token):
    """One anonymous-or-token GET of a GitHub API URL, parsed. Raises on any failure."""
    headers = {"Accept": "application/json", "User-Agent": "eve-offline-claims"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    req = urllib.request.Request(url, headers=headers, method="GET")
    with urllib.request.urlopen(req, timeout=10) as r:
        return json.loads(r.read())


def resolve_repo(run, env, fetch, err, key, repo):
    """The repo the board lives on: the parent when `repo` is a GitHub fork.

    The answer is cached in `eve-tracker` in the git common dir, keyed by host and
    repo (never the URL, which may hold credentials). A failed lookup (offline, rate
    limit) falls back to `repo` with a note and is not cached.
    """
    path = cache_file(run)
    try:
        with open(path, encoding="utf-8") as f:
            c = json.load(f)
        if c.get("key") == key and c.get("repo"):
            return c["repo"]
    except (OSError, TypeError, ValueError, AttributeError):
        pass
    try:
        info = fetch(f"https://api.github.com/repos/{repo}", read_token(env, GITHUB_API))
        root = None
        if info.get("fork"):
            # `source` is the root of the fork network; `parent` is only the next one up.
            root = ((info.get("source") or {}).get("full_name")
                    or (info.get("parent") or {}).get("full_name"))
        if root is not None and not re.fullmatch(r"[\w.-]+/[\w.-]+", str(root)):
            print(f"note: GitHub named an unusable parent of {repo}; using {repo} "
                  "(set EVE_TRACKER_REPO to choose another)", file=err)
            return repo
        answer = root or repo
    except (OSError, ValueError, TypeError, AttributeError, TrackerError):
        print(f"note: could not ask GitHub whether {repo} is a fork; using {repo} "
              "(set EVE_TRACKER_REPO to choose another)", file=err)
        return repo
    if path:
        try:
            with open(path, "w", encoding="utf-8") as f:
                json.dump({"key": key, "repo": answer}, f)
        except OSError:
            pass
    return answer


def config(env=None, run=None, fetch=None, err=None):
    """Where the claim board is. In order: all of EVE_TRACKER, EVE_TRACKER_API and
    EVE_TRACKER_REPO; else the `upstream` remote; else `origin`. A GitHub remote that
    is a fork resolves to its parent."""
    env = os.environ if env is None else env
    run = run or git
    origin = "remote"
    kind, api, repo = env.get("EVE_TRACKER"), env.get("EVE_TRACKER_API"), env.get("EVE_TRACKER_REPO")
    if not (kind and api and repo):
        url = run("remote", "get-url", "upstream") or run("remote", "get-url", "origin")
        parsed = parse_remote(url)
        if parsed:
            host, own = parsed
            gh = host == "github.com"
            if gh and not api and not repo:
                repo = resolve_repo(run, env, fetch or github_get, err or sys.stderr, f"{host}/{own}", own)
                if repo != own:
                    origin = "fork"
            kind = kind or ("github" if gh else "gitea")
            api = api or ("https://api.github.com" if gh else f"https://{host}/api/v1")
            repo = repo or own
    if kind not in ("github", "gitea") or not api or not repo:
        raise TrackerError("config", "-", "set EVE_TRACKER, EVE_TRACKER_API and EVE_TRACKER_REPO")
    if env.get("EVE_TRACKER_REPO") and origin == "remote":
        origin = "override"
    return {"kind": kind, "api": api.rstrip("/"), "repo": repo, "from": origin}


def read_token(env=None, api=GITHUB_API):
    """EVE_TRACKER_TOKEN, else EVE_TRACKER_TOKEN_FILE, else GH_TOKEN, else
    GITHUB_TOKEN; those two only when `api` is https://api.github.com, so a github.com
    token never goes to another host."""
    env = os.environ if env is None else env
    tok = env.get("EVE_TRACKER_TOKEN", "").strip()
    if not tok and env.get("EVE_TRACKER_TOKEN_FILE"):
        try:
            with open(env["EVE_TRACKER_TOKEN_FILE"], encoding="utf-8") as f:
                tok = f.read().strip()
        except OSError:
            tok = ""
    if not tok and (api or "").rstrip("/") == GITHUB_API:
        tok = env.get("GH_TOKEN", "").strip() or env.get("GITHUB_TOKEN", "").strip()
    return tok or None




# ---------------------------------------------------------------- transport


class Http:
    """urllib transport, GET only. The token lives only in this object and the header.

    `request` and `_send` refuse every other method. A subclass that must write
    (private/tracker-post.py) sets `writable` and overrides `request`.
    """

    writable = False

    def __init__(self, cfg, token, cache_path=CACHE):
        self.cfg = cfg
        self._token = token
        self.cache_path = cache_path
        self.has_token = bool(token)

    def _cache(self):
        try:
            with open(self.cache_path, encoding="utf-8") as f:
                return json.load(f)
        except (OSError, ValueError):
            return {}

    def _store(self, data):
        try:
            os.makedirs(os.path.dirname(self.cache_path), exist_ok=True)
            with open(self.cache_path, "w", encoding="utf-8") as f:
                json.dump(data, f)
        except OSError:
            pass

    def request(self, method, path, body=None):
        if method != "GET":
            raise TrackerError("read-only", method, path)
        return self._send(method, path, body)

    def _send(self, method, path, body=None):
        if method != "GET" and not self.writable:
            raise TrackerError("read-only", method, path)
        url = self.cfg["api"] + path
        key = ("auth " if self._token else "anon ") + url
        if method == "GET":
            c = self._cache().get(key)
            if c and time.time() - c["t"] < CACHE_TTL:
                return c["v"]
        headers = {"Accept": "application/json", "User-Agent": "eve-offline-tracker"}
        if self._token:
            scheme = "Bearer" if self.cfg["kind"] == "github" else "token"
            headers["Authorization"] = f"{scheme} {self._token}"
        data = None
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        status = None
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                raw = r.read()
        except urllib.error.HTTPError as e:
            status = e.code
        except (urllib.error.URLError, OSError, ValueError):
            status = "unreachable"
        if status is not None:
            raise TrackerError(status, method, path) from None
        val = json.loads(raw) if raw.strip() else None
        cache = self._cache() if method == "GET" else {}
        if method == "GET":
            cache[key] = {"t": time.time(), "v": val}
        self._store(cache)  # a write empties the cache
        return val


# ---------------------------------------------------------------- tracker


class Tracker:
    """Read-only calls over a transport."""

    def __init__(self, cfg, transport):
        self.cfg = cfg
        self.t = transport
        self.gh = cfg["kind"] == "github"
        self.r = f"/repos/{cfg['repo']}"
        self._me = None

    def get(self, path):
        return self.t.request("GET", path)

    def me(self):
        if self._me is None:
            if not getattr(self.t, "has_token", True):
                raise NoToken()
            self._me = self.get("/user")["login"]
        return self._me

    def paged(self, path):
        out, page = [], 1
        sep = "&" if "?" in path else "?"
        size = "per_page=100" if self.gh else "limit=50"
        while True:
            items = self.get(f"{path}{sep}page={page}&{size}") or []
            out.extend(items)
            if len(items) < (100 if self.gh else 50):
                return out
            page += 1

    def open_issues(self, labels=()):
        q = "state=open" + ("" if self.gh else "&type=issues")
        if labels:
            q += "&labels=" + urllib.parse.quote(",".join(labels))
        return [i for i in self.paged(f"{self.r}/issues?{q}") if "pull_request" not in i
                or i.get("pull_request") is None]

    def issue(self, n):
        return self.get(f"{self.r}/issues/{n}")

    def open_pulls(self):
        return self.paged(f"{self.r}/pulls?state=open")

    def pull_file_list(self, n):
        """The file objects of a PR (`filename`, `previous_filename`, `patch`, ...)."""
        return self.paged(f"{self.r}/pulls/{n}/files")

    def pull_files(self, n):
        return [f.get("filename", "") for f in self.pull_file_list(n)]


# ---------------------------------------------------------------- parsing


def labels_of(issue):
    return {lb["name"] if isinstance(lb, dict) else lb for lb in issue.get("labels") or []}


def parse_time(s):
    try:
        return dt.datetime.fromisoformat((s or "").replace("Z", "+00:00"))
    except ValueError:
        return None


def pr_issue_refs(pr):
    return {int(x) for x in re.findall(
        r"(?:close[sd]?|fix(?:e[sd])?|resolve[sd]?)\s+#(\d+)", pr.get("body") or "", re.I)}


TITLE_CAP = 100
BIDI = set("‎‏؜‪‫‬‭‮⁦⁧⁨⁩")


def clean_text(s, cap=TITLE_CAP):
    """Printable one-line text: tabs and newlines become spaces; control, format, private
    use and surrogate characters and bidi marks are dropped; capped at `cap` characters."""
    out = []
    for ch in str(s or ""):
        if ch in "\t\r\n":
            out.append(" ")
        elif unicodedata.category(ch) in ("Zl", "Zp"):
            out.append(" ")
        elif ch in BIDI or unicodedata.category(ch) in ("Cc", "Cf", "Co", "Cs"):
            continue
        else:
            out.append(ch)
    return "".join(out).strip()[:cap]


def is_login(s):
    return bool(re.fullmatch(r"[A-Za-z0-9-]{1,39}", s or ""))


def origin_owner():
    """The owner part of the `origin` remote, or None."""
    parsed = parse_remote(git("remote", "get-url", "origin"))
    return parsed[1].split("/")[0] if parsed else None


def pr_for_branch(prs, branch, mine):
    """The open PR whose head is `branch` and which `mine` (the origin owner) opened: a
    fork's PR sits on the parent repo with head label `<them>:<branch>`, and another
    person's branch of the same name is not mine."""
    for p in prs:
        head = p.get("head") or {}
        label = head.get("label") or ""
        if head.get("ref") == branch and (":" not in label or mine is None
                                          or label.split(":", 1)[0] == mine):
            return p
    return None


# ---------------------------------------------------------------- commands


def cmd_whoami(tr, a, out):
    print(tr.me(), file=out)
    return 0


def cmd_repo(tr, a, out):
    """board=, kind=, origin= and from= (override, remote or fork: where board came from)."""
    print(f"board={tr.cfg['repo']}\nkind={tr.cfg['kind']}", file=out)
    parsed = parse_remote(git("remote", "get-url", "origin"))
    print(f"origin={parsed[1] if parsed else ''}\nfrom={tr.cfg.get('from', 'remote')}", file=out)
    return 0


def cmd_pr_for_branch(tr, a, out):
    branch = a.branch or git("rev-parse", "--abbrev-ref", "HEAD")
    p = pr_for_branch(tr.open_pulls(), branch, origin_owner())
    if p:
        print(p["number"], file=out)
        return 0
    print("none", file=out)
    return 1


def cmd_issues(tr, a, out):
    """Open issues as a sorting hint: nothing here grants anything. Only the number, the
    labels, the milestone date, the open PRs that say `Closes #N` and a cleaned title are
    read; bodies are never printed."""
    pulls = tr.open_pulls()
    rows = []
    for i in tr.open_issues():
        labels = labels_of(i)
        if a.no_client and "needs-client" in labels:
            continue
        area = next((x[5:] for x in sorted(labels) if x.startswith("area/")), "")
        if a.area and area != a.area:
            continue
        held = bool(labels & {"needs-owner", "parked"})
        ready = "held" if held else ("yes" if "status/ready" in labels else "no")
        prio = next((int(x[5:]) for x in sorted(labels) if re.fullmatch(r"prio/[123]", x)), 9)
        due = ((i.get("milestone") or {}).get("due_on")) or "9999"
        prs = sorted(p["number"] for p in pulls if i["number"] in pr_issue_refs(p))
        key = (held, ready != "yes", prio, due, i["number"])
        rows.append((key, i, ready, prio, area, labels, prs))
    rows.sort(key=lambda r: r[0])
    if a.limit:
        rows = rows[:a.limit]
    if not rows:
        print("no issues", file=out)
        return 1
    print("issue\tready\tprio\tarea\tclient\tprs\ttitle", file=out)
    for _, i, ready, prio, area, labels, prs in rows:
        p = f"prio/{prio}" if prio < 9 else "-"
        cl = "yes" if "needs-client" in labels else "no"
        print(f"{i['number']}\t{ready}\t{p}\t{clean_text(area, 40) or '-'}\t{cl}\t"
              f"{','.join(map(str, prs)) or '-'}\t{clean_text(i.get('title'))}", file=out)
    return 0


def parser():
    ap = argparse.ArgumentParser(prog="tracker.py", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("whoami")
    sub.add_parser("repo")
    s = sub.add_parser("pr-for-branch")
    s.add_argument("--branch")
    s = sub.add_parser("issues")
    s.add_argument("--no-client", action="store_true")
    s.add_argument("--area")
    s.add_argument("--limit", type=int, default=20)
    return ap


COMMANDS = {"whoami": cmd_whoami, "repo": cmd_repo, "pr-for-branch": cmd_pr_for_branch,
            "issues": cmd_issues}

NO_TOKEN_TEXT = ("no-token: set GH_TOKEN (or GITHUB_TOKEN; `gh auth login` and "
                 "GH_TOKEN=$(gh auth token) is the usual way), or EVE_TRACKER_TOKEN / "
                 "EVE_TRACKER_TOKEN_FILE for another tracker (or ask your human to run "
                 "this command)")


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
        print(NO_TOKEN_TEXT, file=err)
        return 2
    except TrackerError as e:
        print(str(e), file=err)
        return 2


if __name__ == "__main__":
    sys.exit(main())
