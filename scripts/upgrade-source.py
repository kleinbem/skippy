#!/usr/bin/env python3
"""Resolve and prepare source-pinned llama.cpp update candidates."""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from lib.ci_artifacts import digest, write_json
from lib.ci_process import cancel_on_signal, command
from lib.upgrade_regeneration import regenerate
from lib.upgrade_repair import repair
from lib.upgrade_preimages import seed_patch_preimages
from lib.native_recipe import write_native_recipe
from lib.graph_contracts import check_upgrade_coverage
from lib.upgrade_resolution import latest, open_upgrade
from lib.upgrade_source import ALLOWED, PIN, check_changes, clean_root, git, output, prepared_head, restore, restore_at, validate_sha


def resolve(upstream, force):
    sha = validate_sha(upstream or latest())
    current = Path(PIN).read_text().strip()
    existing = open_upgrade(sha)
    if existing:
        output("existing_pr", existing)
    output("upstream_sha", sha)
    output("changed", str(current != sha).lower())
    output("validate", str(existing is None and (current != sha or force == "true")).lower())


def save_preparation_diagnostics(checkout, destination):
    """Keep patch failures inspectable after the disposable checkout is removed."""
    source = checkout / ".deps/llama.cpp"
    coverage = checkout / "target/llama-regeneration/graph-coverage.json"
    if coverage.is_file():
        destination.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(coverage, destination / coverage.name)
    if not (source / ".git").is_dir():
        return
    destination.mkdir(parents=True, exist_ok=True)
    for name, args in {
        "source-status.txt": ("status", "--short"),
        "source.diff": ("diff", "HEAD", "--"),
        # Even this read-only am operation checks committer identity on clean CI hosts.
        "failed.patch": ("-c", "user.name=Skippy diagnostics", "-c", "user.email=ci@skippy.local",
                         "am", "--show-current-patch=diff"),
    }.items():
        try:
            text = git(source, *args)
        except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
            # There is no active mail patch when failure occurs during compilation.
            continue
        (destination / name).write_text(text + "\n", encoding="utf-8")


def prepare(upstream, repair_bundle=None, diagnostics=None):
    validate_sha(upstream)
    if bool(repair_bundle) != bool(diagnostics):
        raise ValueError("repair bundle and diagnostics are required together")
    root = clean_root()
    base = git(root, "rev-parse", "HEAD")
    directory = root / "target/llama-upgrade"
    directory.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=directory) as scratch:
        checkout = Path(scratch) / "source"
        command(["git", "clone", "--no-hardlinks", "--no-checkout", str(root), str(checkout)], timeout=600)
        git(checkout, "checkout", "--detach", base)
        if repair_bundle:
            metadata = json.loads(repair_bundle.with_name("candidate.json").read_text())
            if (metadata["base_sha"] != base or metadata["upstream_sha"] != upstream
                    or digest(repair_bundle) != metadata["bundle_sha256"]):
                raise ValueError("repair bundle does not match the protected base")
            restore_at(checkout, repair_bundle.resolve(), metadata["candidate_sha"])
            repair(checkout, diagnostics.resolve())
            git(checkout, "reset", "--soft", base)
        base_pin = git(root, "show", f"{base}:{PIN}")
        (checkout / PIN).write_text(upstream + "\n", encoding="utf-8")
        try:
            try:
                seed_patch_preimages(checkout, base_pin, root / "llama_cpp/patches")
                regenerate(checkout)
                check_upgrade_coverage(checkout, base_pin)
            except (OSError, ValueError, RuntimeError, subprocess.SubprocessError):
                save_preparation_diagnostics(checkout, directory / "diagnostics")
                if not os.environ.get("SKIPPY_UPGRADE_REPAIR_COMMAND"):
                    print("Automatic repair did not run: configure SKIPPY_UPGRADE_REPAIR_COMMAND "
                          "for an installed repair agent. Preparation diagnostics were retained.", file=sys.stderr)
                    raise
                repair(checkout, directory / "diagnostics")
                regenerate(checkout)
                check_upgrade_coverage(checkout, base_pin)
            env = {"LLAMA_WORKDIR": str(checkout / ".deps/llama.cpp"),
                   "LLAMA_PIN_FILE": str(checkout / PIN), "LLAMA_PATCH_DIR": str(checkout / "llama_cpp/patches")}
            command(["just", "--justfile", str(checkout / "Justfile"), "llama-prepare"],
                    overrides=env, timeout=3600)
            write_native_recipe(checkout)
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt):
            try:
                save_preparation_diagnostics(checkout, directory / "diagnostics")
            except OSError as diagnostic_error:
                print(f"could not retain preparation diagnostics: {diagnostic_error}", file=sys.stderr)
            raise
        check_changes(checkout, "HEAD")
        git(checkout, "add", "--", *(path for path in ALLOWED if (checkout / path).exists()))
        timestamp = int(git(checkout, "show", "-s", "--format=%ct", base)) + 1
        env = {f"GIT_{role}_{key}": value for role in ("AUTHOR", "COMMITTER") for key, value in {
            "NAME": "Skippy llama updater", "EMAIL": "ci@skippy.local", "DATE": f"@{timestamp} +0000",
        }.items()}
        git(checkout, "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m",
            f"Update llama.cpp to {upstream}", overrides=env, timeout=120)
        candidate = git(checkout, "rev-parse", "HEAD")
        bundle = Path(scratch) / "candidate.bundle"
        git(checkout, "bundle", "create", str(bundle), f"{base}..HEAD")
        metadata = Path(scratch) / "candidate.json"
        write_json(metadata, {"schema_version": 1, "base_sha": base, "candidate_sha": candidate,
                              "upstream_sha": upstream, "bundle_sha256": digest(bundle)})
        bundle.replace(directory / bundle.name)
        metadata.replace(directory / metadata.name)
    output("candidate_sha", candidate)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    action = actions.add_parser("resolve")
    action.add_argument("--upstream")
    action.add_argument("--force-check", choices=("true", "false"), default="false")
    action = actions.add_parser("prepare")
    action.add_argument("upstream")
    action.add_argument("--repair-bundle", type=Path)
    action.add_argument("--diagnostics", type=Path)
    action = actions.add_parser("restore")
    action.add_argument("bundle", type=Path)
    action.add_argument("expected")
    actions.add_parser("prepared-source").add_argument("--root", required=True, type=Path)
    args = parser.parse_args()
    if args.action == "resolve":
        resolve(args.upstream, args.force_check)
    elif args.action == "prepare":
        prepare(args.upstream, args.repair_bundle, args.diagnostics)
    elif args.action == "restore":
        restore(args.bundle, args.expected)
        output("candidate_sha", args.expected)
    else:
        print(prepared_head(args.root))


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
