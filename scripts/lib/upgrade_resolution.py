"""Resolve published upstream pins and find existing immutable upgrade PRs."""

import json
import os

from .ci_process import command
from .upgrade_source import validate_sha

REPO = "Mesh-LLM/skippy"


def gh(*args, timeout=180):
    token = os.environ.get("GH_TOKEN")
    env = {"GH_TOKEN": token} if token else None
    return command(["gh", *args], overrides=env, timeout=timeout, capture_stdout=True)


def open_upgrade(upstream):
    validate_sha(upstream)
    pulls = json.loads(gh("pr", "list", "--repo", REPO, "--state", "open", "--base", "main",
                          "--limit", "1000", "--json", "headRefName,headRefOid,baseRefName,url"))
    if not isinstance(pulls, list) or len(pulls) >= 1000:
        raise ValueError("open PR query may be truncated")
    for pr in pulls:
        branch = pr["headRefName"]
        if pr["baseRefName"] != "main" or not branch.startswith("automation/llama-"):
            continue
        oid = validate_sha(pr["headRefOid"])
        url = pr["url"]
        if branch != f"automation/llama-{oid[:12]}" or not url.startswith(f"https://github.com/{REPO}/pull/"):
            raise ValueError("open upgrade PR has inconsistent immutable branch identity")
        pin = gh("api", "-H", "Accept: application/vnd.github.raw+json",
                 f"/repos/{REPO}/contents/llama_cpp/upstream.txt?ref={oid}").strip()
        if validate_sha(pin) == upstream:
            return url
    return None


def latest():
    releases = gh("api", "--paginate", "/repos/ggml-org/llama.cpp/releases?per_page=100", "--jq",
                  ".[] | select(.draft == false and .published_at != null) | [.published_at, .tag_name] | @tsv",
                  timeout=600)
    latest_metadata = None
    for line in releases.splitlines():
        parts = line.split("\t")
        if len(parts) != 2 or not all(parts):
            raise ValueError("invalid published release metadata")
        if latest_metadata is None or parts[0] > latest_metadata[0]:
            latest_metadata = parts
    if latest_metadata is None:
        raise ValueError("no published upstream release")
    tag = latest_metadata[1]
    if tag.startswith("-") or any(ch in tag for ch in "\r\n "):
        raise ValueError("invalid upstream release tag")
    ref = f"refs/tags/{tag}"
    peeled = ref + "^{}"
    advertised = command(["git", "ls-remote", "https://github.com/ggml-org/llama.cpp.git", ref, peeled],
                         capture_stdout=True)
    refs = {}
    for line in advertised.splitlines():
        oid, name = line.split()
        refs[name] = validate_sha(oid)
    if ref not in refs and peeled not in refs:
        raise ValueError("published release tag has no immutable Git target")
    return refs.get(peeled) or refs[ref]
