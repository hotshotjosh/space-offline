"""A live check of the admin web page and API (ADMIN-WEB, #14).

Starts a dev server with `--admin` in a Docker container (the repo's build
image, the binary mounted read-only, no capabilities, a read-only root, one
tmpfs) on a free loopback port, reads its token from the container, and asks:

  * the page, its script and its stylesheet answer with no token;
  * every `GET /api/...` path answers 200 JSON with the token (`/api/metrics`
    answers Prometheus text, with the header or `Authorization: Bearer`) and 401 without
    it (and with a wrong one);
  * every `POST /api/action/...` answers with the token (`bots`, `broadcast`,
    `save`, and `kick`, which refuses an unknown pilot with a 409 and an
    error text), and 401 without it;
  * a second action of one name inside a second is refused (429); a request
    without a JSON content type is refused (415); an unknown action is a 404.

The kick that really hangs a connection up is a unit test in
`admin_actions.rs`; this check needs no game client, so no pilot is online.

It builds the server first (`tools/dev.sh cargo build`, in Docker), which takes
minutes on a cold cache, so it is skipped unless `EVE_ADMIN_WEB_LIVE=1`. The
container is named `eve-admin-web-test-<pid>-<port>` and removed by that name
in `tearDownClass` and at exit; nothing else is touched, and neither port
26000 nor `var/dev` is used (the game port inside is 26111, unpublished).

Run::

    EVE_ADMIN_WEB_LIVE=1 python3 -I tools/test_admin_web.py
"""

from __future__ import annotations

import atexit
import http.client
import json
import os
import socket
import subprocess
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
LIVE = os.environ.get("EVE_ADMIN_WEB_LIVE") == "1"
IMAGE = "eve-offline-build"
GETS = ["/api/status", "/api/pilots", "/api/systems", "/api/map", "/api/ticks", "/api/methods"]
METRICS = "/api/metrics"
ACTIONS = {
    "bots": {"words": "list"},
    "broadcast": {"text": "test announcement"},
    "save": {},
    "kick": {"pilot": "Nobody Invented"},
}
PAGES = ["/", "/admin.js", "/admin.css"]


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def docker(*args: str, check: bool = True, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["docker", *args],
        check=check,
        timeout=timeout,
        capture_output=True,
        text=True,
    )


@unittest.skipUnless(LIVE, "set EVE_ADMIN_WEB_LIVE=1: builds the server and runs it in Docker")
class AdminWebLive(unittest.TestCase):
    name = ""
    port = 0
    token = ""

    @classmethod
    def setUpClass(cls) -> None:
        env = dict(os.environ, EVE_MAX_JOBS=os.environ.get("EVE_MAX_JOBS", "3"))
        subprocess.run(
            [os.path.join(HERE, "dev.sh"), "cargo", "build", "--locked", "-p", "eve-server"],
            check=True,
            env=env,
            cwd=REPO,
        )
        target = os.environ.get("EVE_TARGET_DIR") or os.path.join(REPO, "server", "target")
        binary = os.path.join(target, "debug", "eve-server")
        assert os.path.isfile(binary), binary
        cls.port = free_port()
        assert cls.port != 26000
        cls.name = f"eve-admin-web-test-{os.getpid()}-{cls.port}"
        atexit.register(cls._remove)
        docker(
            "run", "-d", "--name", cls.name, "--init",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
            "--read-only", "--tmpfs", "/tmp:exec,size=256m", "-w", "/tmp",
            "--user", f"{os.getuid()}:{os.getgid()}", "--memory", "2g",
            "-p", f"127.0.0.1:{cls.port}:8090",
            "-v", f"{binary}:/eve-server:ro",
            IMAGE, "/eve-server", "--dev", "--bind", "127.0.0.1:26111",
            "--bot-control", "/tmp", "--admin", "0.0.0.0:8090",
        )  # fmt: skip
        deadline = time.time() + 60
        while time.time() < deadline:
            got = docker("exec", cls.name, "cat", "/tmp/admin.token", check=False)
            if got.returncode == 0 and got.stdout.strip():
                cls.token = got.stdout.strip()
                break
            time.sleep(0.5)
        else:
            logs = docker("logs", cls.name, check=False).stdout[-2000:]
            raise AssertionError("the server did not write its token\n" + logs)
        while time.time() < deadline:
            try:
                if cls.get("/api/status", cls.token)[0] == 200:
                    return
            except OSError:
                pass
            time.sleep(0.5)
        raise AssertionError("the admin page did not answer")

    @classmethod
    def tearDownClass(cls) -> None:
        cls._remove()

    @classmethod
    def _remove(cls) -> None:
        if cls.name:
            docker("rm", "-f", cls.name, check=False)

    @classmethod
    def request(cls, method: str, path: str, token: str | None, body=None, ctype="application/json"):
        c = http.client.HTTPConnection("127.0.0.1", cls.port, timeout=10)
        headers = {}
        if token is not None:
            headers["X-Admin-Token"] = token
        data = None
        if body is not None:
            data = json.dumps(body)
            if ctype:
                headers["Content-Type"] = ctype
        try:
            c.request(method, path, body=data, headers=headers)
            r = c.getresponse()
            return r.status, r.read().decode("utf-8", "replace")
        finally:
            c.close()

    @classmethod
    def get(cls, path: str, token: str | None):
        return cls.request("GET", path, token)

    def test_1_the_page_answers_without_a_token(self) -> None:
        for p in PAGES:
            status, body = self.get(p, None)
            self.assertEqual(status, 200, p)
            self.assertTrue(body, p)
        self.assertIn("<html", self.get("/", None)[1].lower())

    def test_2_every_api_path_answers_with_the_token_and_401_without(self) -> None:
        for p in GETS:
            status, body = self.get(p, self.token)
            self.assertEqual(status, 200, p)
            self.assertIsInstance(json.loads(body), dict, p)
            self.assertEqual(self.get(p, None)[0], 401, p + " without a token")
            self.assertEqual(self.get(p, "0" * 64)[0], 401, p + " with a wrong token")
        self.assertEqual(self.get("/api/nothing", self.token)[0], 404)

    def test_2b_metrics_are_plain_text_and_take_a_bearer_token(self) -> None:
        status, body = self.get(METRICS, self.token)
        self.assertEqual(status, 200)
        self.assertTrue(any(line.startswith("eve_uptime_seconds ") for line in body.splitlines()))
        self.assertEqual(self.get(METRICS, None)[0], 401, "metrics without a token")
        self.assertEqual(self.get(METRICS, "0" * 64)[0], 401, "metrics with a wrong token")
        # A scraper sends `Authorization: Bearer <token>`; the token is never printed.
        c = http.client.HTTPConnection("127.0.0.1", self.port, timeout=10)
        try:
            c.request("GET", METRICS, headers={"Authorization": "Bearer " + self.token})
            r = c.getresponse()
            self.assertEqual(r.status, 200)
            self.assertTrue(r.getheader("Content-Type", "").startswith("text/plain"))
            self.assertIn("eve_uptime_seconds ", r.read().decode("utf-8", "replace"))
        finally:
            c.close()

    def test_3_every_action_answers_and_401_without_the_token(self) -> None:
        for name, body in ACTIONS.items():
            path = f"/api/action/{name}"
            self.assertEqual(self.request("POST", path, None, body)[0], 401, name)
            self.assertEqual(self.request("POST", path, "0" * 64, body)[0], 401, name)
        for name, body in ACTIONS.items():
            status, text = self.request("POST", f"/api/action/{name}", self.token, body)
            got = json.loads(text)
            if name == "kick":
                # No pilot is online: a clear refusal, not a success.
                self.assertEqual(status, 409, text)
                self.assertIn("no pilot online", got["error"])
            else:
                self.assertEqual(status, 200, f"{name}: {text}")
                self.assertTrue(got["ok"], name)
                self.assertTrue(got["text"], name)

    def test_4_limits_and_refusals(self) -> None:
        time.sleep(1.2)  # the rate limit of test_3's actions
        path = "/api/action/broadcast"
        body = ACTIONS["broadcast"]
        self.assertEqual(self.request("POST", path, self.token, body)[0], 200)
        self.assertEqual(self.request("POST", path, self.token, body)[0], 429)
        self.assertEqual(self.request("POST", path, self.token, body, ctype="")[0], 415)
        self.assertEqual(self.request("POST", "/api/action/kick", self.token, {})[0], 400)
        self.assertEqual(self.request("POST", "/api/action/nothing", self.token, {})[0], 404)


if __name__ == "__main__":
    unittest.main()
