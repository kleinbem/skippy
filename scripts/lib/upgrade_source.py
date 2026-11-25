"""Bind upgrade preparation and restoration to immutable source identities."""

import os
import re
from pathlib import Path

from .ci_process import command

PIN = "llama_cpp/upstream.txt"
WORKFLOW = "Mesh-LLM/skippy/.github/workflows/llama-upgrade.yml@refs/heads/main"
ALLOWED = (PIN, "llama_cpp/patches", "crates/skippy-ffi/src/bindings", "docs/native-api.md",
           "crates/skippy-api/src/native-recipe.json")


def validate_sha(sha):
    if not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("expected a complete lowercase Git SHA-1")
    return sha


def git(root, *args, timeout=180, overrides=None):
    return command(["git", "-C", str(root), *args], timeout=timeout,
                   overrides=overrides, capture_stdout=True).strip()


def prepared_head(root):
    source = root / ".deps/llama.cpp"
    head = validate_sha(git(source, "rev-parse", "HEAD"))
    if ((source / ".skippy-patched-sha").read_text().strip() != head
            or git(source, "status", "--porcelain", "--untracked-files=no")):
        raise ValueError("prepared llama source is dirty or unstamped")
    if (source / ".skippy-upstream-sha").read_text().strip() != (root / PIN).read_text().strip():
        raise ValueError("prepared llama source does not match pin")
    return head


def clean_root():
    root = Path.cwd().resolve()
    if Path(git(root, "rev-parse", "--show-toplevel")).resolve() != root:
        raise ValueError("upgrade commands must run at the repository root")
    if git(root, "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("upgrade source checkout must be clean")
    return root


def check_changes(root, *revisions):
    paths = command(["git", "-C", str(root), "diff", "--no-renames", "--name-only",
                     "-z", *revisions, "--"], capture_stdout=True).split("\0")
    if len(revisions) <= 1:
        paths += command(["git", "-C", str(root), "ls-files", "--others", "--exclude-standard", "-z"],
                         capture_stdout=True).split("\0")
    for path in filter(None, paths):
        if not any(path == allowed or path.startswith(allowed + "/") for allowed in ALLOWED):
            raise ValueError(f"candidate modified a path outside the update allowlist: {path}")


def restore_at(root, bundle, expected):
    validate_sha(expected)
    base = git(root, "rev-parse", "HEAD")
    bundle = str(bundle.resolve(strict=True))
    git(root, "bundle", "verify", bundle)
    if git(root, "bundle", "list-heads", bundle) != expected + " HEAD":
        raise ValueError("candidate bundle advertises unexpected refs")
    git(root, "fetch", "--no-tags", bundle, "HEAD")
    if git(root, "rev-list", "--parents", "-n", "1", expected) != f"{expected} {base}":
        raise ValueError("candidate must directly descend from the protected checkout")
    check_changes(root, base, expected)
    validate_sha(git(root, "show", f"{expected}:{PIN}"))
    git(root, "checkout", "--detach", expected)


def restore(bundle, expected):
    if os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("GITHUB_WORKFLOW_REF") != WORKFLOW:
        raise ValueError("candidate restoration requires the protected llama-upgrade workflow")
    restore_at(clean_root(), bundle, expected)


def output(key, value):
    if not isinstance(value, str) or "\r" in value or "\n" in value:
        raise ValueError("invalid workflow output")
    print(f"{key}={value}")
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as stream:
            stream.write(f"{key}={value}\n")
