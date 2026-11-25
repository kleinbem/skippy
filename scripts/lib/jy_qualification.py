"""JY submitter authentication and native qualification API requests."""

import json
import os
import re
import tempfile
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import HTTPRedirectHandler, Request, build_opener

from .ci_process import capture

ORIGIN = "https://pilot.jianyang.cloud"


class PilotHTTPError(RuntimeError):
    def __init__(self, path, status, detail):
        super().__init__(f"JY HTTP {status}: {detail}")
        self.path = path
        self.status = status


class RejectRedirects(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Pilot:
    def __init__(self):
        self.opener = build_opener(RejectRedirects())
        self.session = None

    def request(self, method, path, body=None, token=None, timeout=60):
        if not path.startswith("/v1/") or re.search(r"[\r\n]", path):
            raise ValueError("invalid API path")
        headers = {
            "Content-Type": "application/json",
            "User-Agent": "skippy-inference-ci/1.0",
        }
        if token is not None:
            if not token or re.search(r"[\r\n]", token):
                raise ValueError("invalid API credential")
            headers["Authorization"] = "Bearer " + token
        data = json.dumps(body).encode("utf-8") if body is not None else None
        request = Request(ORIGIN + path, data=data, headers=headers, method=method)
        try:
            with self.opener.open(request, timeout=timeout) as response:
                result = response.read(16 * 1024 * 1024 + 1)
        except HTTPError as error:
            detail = (error.read(65536).decode("utf-8", errors="replace")
                      if not path.startswith("/v1/auth/") else f"{path} authentication request failed")
            raise PilotHTTPError(path, error.code, detail) from None
        if len(result) > 16 * 1024 * 1024:
            raise ValueError("JY response exceeded capture limit")
        return json.loads(result)

    def auth(self):
        if "JIANYANG_API_KEY" in os.environ:
            return os.environ["JIANYANG_API_KEY"]
        if self.session and time.monotonic() - self.session[0] < 1800:
            return self.session[1]
        private = os.environ["JIANYANG_INFERENCE_SSH_KEY"].replace("\r\n", "\n").strip()
        if not private:
            raise ValueError("JIANYANG_INFERENCE_SSH_KEY is empty")
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            key, public, nonce = (root / name for name in ("key", "key.pub", "nonce"))
            fd = os.open(key, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as stream:
                stream.write((private + "\n").encode("utf-8"))
            public.write_text(capture(["ssh-keygen", "-y", "-f", str(key)]), encoding="utf-8")
            fingerprint = capture(["ssh-keygen", "-lf", str(public)]).split()[1]
            challenge = self.request("POST", "/v1/auth/challenge", {}, timeout=30)
            if challenge["namespace"] != "jianyang":
                raise ValueError("unexpected SSH signature namespace")
            nonce.write_bytes(challenge["nonce"].encode("utf-8"))
            capture(["ssh-keygen", "-Y", "sign", "-f", str(key), "-n", "jianyang", str(nonce)])
            signature = nonce.with_suffix(".sig").read_text(encoding="utf-8")
            session = self.request("POST", "/v1/auth/login", {
                "nonce": challenge["nonce"], "fingerprint": fingerprint, "signature": signature,
            }, timeout=30)
        if session["role"] != "submitter":
            raise ValueError("inference automation requires a submitter key")
        self.session = (time.monotonic(), session["api_key"])
        return self.session[1]

    def api(self, method, path, body=None):
        return self.request(method, path, body, self.auth())


def placement(inventory, runner, attempt, source):
    def pair(allow_owned):
        networks = {}
        for entry in inventory["agents"]:
            identifier, node = entry.get("id"), entry.get("node")
            network, runs = entry.get("network"), entry.get("active_runs")
            if (entry.get("online") is not True or entry.get("enabled") is not True
                    or entry.get("drain") is not False or network not in {"james-lab", "dan-lab"}
                    or (entry.get("host") or {}).get("gpu") != "NVIDIA GB10"
                    or not isinstance(identifier, str) or identifier in runner
                    or not isinstance(node, str) or not isinstance(runs, list)):
                continue
            if runs and (not allow_owned or not all(owned_run(run) for run in runs)):
                continue
            networks.setdefault(network, set()).add(node)
        groups = [sorted(nodes) for _, nodes in sorted(networks.items()) if len(nodes) >= 2]
        if groups:
            nodes = groups[attempt % len(groups)]
            index = (attempt // len(groups)) % len(nodes)
            return nodes[index], nodes[(index + 1) % len(nodes)]
        return None

    def owned_run(run):
        name = run.get("job_name")
        return (isinstance(name, str) and name.startswith("skippy-inference-")
                and f"-{source[:12]}" in name)

    result = pair(False) or pair(True)
    if result is None:
        raise ValueError("fewer than two idle Sparks on one network available")
    return result
