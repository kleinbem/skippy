#!/usr/bin/env python3
"""Compile or qualify the standalone production embedding consumer."""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from lib import ci_artifacts as artifacts
from lib.ci_process import cancel_on_signal, command

ROOT = Path(__file__).resolve().parents[1]
FIXTURE = Path("fixtures/production-embedding")


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT)


def stage_source(destination, revision):
    destination.mkdir(parents=True)
    if revision is not None:
        if not re.fullmatch(r"[0-9a-f]{40}", revision) or git("cat-file", "-t", revision).strip() != b"commit":
            raise ValueError("source must be a full, locally available Git commit SHA")
        archive_path = destination.with_suffix(".tar")
        with archive_path.open("wb") as archive:
            subprocess.run(["git", "archive", revision], cwd=ROOT, stdout=archive, check=True)
        artifacts.extract_archive(archive_path, destination)
        archive_path.unlink()
        return
    # The candidate check includes worktree changes and new source files.
    # Git controls the source roster, keeping build outputs and caches out.
    for name in dict.fromkeys(git("ls-files", "-co", "--exclude-standard", "-z").split(b"\0")):
        if not name:
            continue
        relative = Path(os.fsdecode(name))
        source = ROOT / relative
        if source.is_file() or source.is_symlink():
            output = destination / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, output, follow_symlinks=False)


def review_lock(source_lock, fixture_lock):
    def identities(lock):
        # Cargo emits each package identity field as one quoted assignment.
        text = lock.read_text(encoding="utf-8")
        for package in text.split("[[package]]")[1:]:
            fields = {key: json.loads(value) for key, value in re.findall(
                r'^(name|version|source|checksum) = (".*")$', package, re.MULTILINE
            )}
            yield (fields["name"], fields["version"], fields.get("source"), fields.get("checksum"))

    added = set(identities(fixture_lock)) - set(identities(source_lock))
    if added != {("skippy-production-consumer", "0.1.0", None, None)}:
        raise ValueError(f"fixture resolved dependencies outside the Skippy lockfile: {added!r}")


def prepare_fixture(root, revision):
    source = root / "vendor" / "skippy"
    stage_source(source, revision)
    shutil.copyfile(source / FIXTURE / "Cargo.toml", root / "Cargo.toml")
    shutil.copytree(source / FIXTURE / "src", root / "src")
    shutil.copyfile(source / "Cargo.lock", root / "Cargo.lock")
    return source / "Cargo.lock"


def require_digest(expected, path):
    if not re.fullmatch(r"[0-9a-f]{64}", expected) or artifacts.digest(path) != expected:
        raise ValueError(f"SHA-256 mismatch: {path}")


def verify_inputs(args):
    bundle = args.runtime_bundle.resolve(strict=True)
    model = args.model.resolve(strict=True)
    manifest_path = bundle / "manifest.json"
    require_digest(args.runtime_manifest_sha256, manifest_path)
    require_digest(args.model_sha256, model)
    manifest = artifacts.read_json(manifest_path)
    receipt = artifacts.read_json(bundle / "provenance.json")
    runtime = manifest["runtime"]
    target = runtime["platform"]["target"]
    identity = {"schema_version": 1, "source": args.source, "kind": "runtime",
                "target": target, "variant": receipt["variant"]}
    actual = artifacts.files(bundle)
    actual.pop("provenance.json")
    if receipt != {**identity, "files": actual}:
        raise ValueError("runtime producer receipt does not match the pinned source or bundle contents")
    if runtime["backend"]["kind"] != "cpu" and (not args.device or args.device in {"CPU", "CPU0"}):
        raise ValueError("qualification on an accelerator runtime requires --device")
    command([sys.executable, str(ROOT / "scripts/native-inspect.py"), "verify-runtime-package",
             str(bundle)], cwd=ROOT, timeout=600)
    return bundle, model, manifest


def run(args):
    qualification = args.action == "qualify"
    if qualification:
        if args.evidence.exists():
            raise ValueError("qualification evidence path must be unused")
        bundle, model, manifest = verify_inputs(args)
    with tempfile.TemporaryDirectory(prefix="skippy-embedding-consumer-") as temporary:
        root = Path(temporary)
        source_lock = prepare_fixture(root, args.source)
        environment = {
            "CARGO_NET_OFFLINE": "true",
            "CARGO_TARGET_DIR": str(ROOT / "target" / "embedding-consumer"),
            "SKIPPY_LLAMA_AUTO_BUILD": "0",
            "SKIPPY_AUTO_BUILD_LLAMA": "0",
        }
        command(["cargo", "check", "--offline", "--all-targets"], cwd=root,
                overrides=environment, timeout=1200)
        fixture_lock = root / "Cargo.lock"
        review_lock(source_lock, fixture_lock)
        if not qualification:
            print(f"checked isolated fixture against {args.source or 'the candidate source tree'}")
            return
        command(["cargo", "run", "--offline", "--locked", "--",
                 str(bundle), args.runtime_manifest_sha256, str(model),
                 args.return_endpoint, args.worker_endpoint, args.device or "-"],
                cwd=root, overrides=environment, timeout=1200)
        artifacts.write_json(args.evidence, {
            "source": args.source,
            "source_lock_sha256": artifacts.digest(source_lock),
            "consumer_lock_sha256": artifacts.digest(fixture_lock),
            "runtime_id": manifest["runtime"]["id"],
            "runtime_abi": manifest["runtime"]["skippy_abi"],
            "runtime_release": manifest["runtime"]["release_version"],
            "runtime_manifest_sha256": args.runtime_manifest_sha256,
            "device": args.device or "CPU",
            "model_sha256": args.model_sha256,
            "native_build": manifest["build"],
            "qualification": "passed",
        })


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    check = actions.add_parser("check")
    check.add_argument("--source", help="full locally available commit SHA")
    qualify = actions.add_parser("qualify")
    qualify.add_argument("--source", required=True, help="full locally available commit SHA")
    qualify.add_argument("--runtime-bundle", type=Path, required=True)
    qualify.add_argument("--runtime-manifest-sha256", required=True)
    qualify.add_argument("--model", type=Path, required=True)
    qualify.add_argument("--model-sha256", required=True)
    qualify.add_argument("--return-endpoint", required=True)
    qualify.add_argument("--worker-endpoint", required=True)
    qualify.add_argument("--device", help="exact native backend device name for an accelerator runtime")
    qualify.add_argument("--evidence", type=Path, required=True)
    run(parser.parse_args())


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
