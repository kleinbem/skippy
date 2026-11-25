#!/usr/bin/env python3
"""Build and cache the exact-source inference executables on the JY host."""

import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from lib.ci_artifacts import digest, read_json, write_json
from lib.ci_process import cancel_on_signal, capture, command
from lib.inference_inputs import BINARIES, BUILD_IDENTITY, DEBUG_DIR, check_executables, identity, lock


def cache_key(inputs):
    data = json.dumps(inputs, separators=(",", ":"), sort_keys=True, ensure_ascii=False)
    return hashlib.sha256(data.encode("utf-8")).hexdigest()


def source_inputs(sha):
    environment = {
        name: value for name, value in os.environ.items()
        if name.startswith(("LLAMA_", "SKIPPY_")) or name in {
            "CC", "CXX", "CUDACXX", "CFLAGS", "CXXFLAGS", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"
        }
    }
    return {
        "schema": 1, "source": sha, "os": platform.system().lower(), "arch": platform.machine(),
        "cargo_lock_sha256": digest(Path("Cargo.lock")), "rust": capture(["rustc", "-vV"]),
        "cuda": capture(["nvcc", "--version"]), "cxx": capture(["c++", "--version"]),
        "cmake": capture(["cmake", "--version"]), "backend": "cuda", "architectures": "121",
        "link": "static", "debug": 0, "environment": environment,
    }


def install_binaries(directory):
    DEBUG_DIR.mkdir(parents=True, exist_ok=True)
    for name in BINARIES:
        shutil.copy(directory / name, DEBUG_DIR / name)


def build(sha, root, inputs, key, cached):
    dependency_key = cache_key({name: value for name, value in inputs.items() if name != "source"})
    cargo_root = root.parent / "cargo-v1"
    cargo_root.mkdir(parents=True, exist_ok=True)
    with lock(cargo_root / f"{dependency_key}.lock"):
        cargo_target = cargo_root / dependency_key
        environment = {
            "CARGO_TARGET_DIR": str(cargo_target), "SCCACHE_DIR": str(root.parent / "compiler"),
            "SCCACHE_BASEDIRS": str(Path.cwd()), "CARGO_INCREMENTAL": "0", "SCCACHE_SERVER_PORT": "4227",
            "SCCACHE_CACHE_SIZE": "20G", "CARGO_PROFILE_DEV_DEBUG": "0",
        }
        sccache = shutil.which("sccache")
        if sccache:
            environment["RUSTC_WRAPPER"] = "sccache"
            try:
                command([sccache, "--stop-server"], overrides=environment, timeout=30)
            except subprocess.CalledProcessError:
                pass
            command([sccache, "--start-server"], overrides=environment, timeout=30)
        command(["just", "inference-build-uncached"], overrides=environment, timeout=7200)
        install_binaries(cargo_target / "debug")
        build_id = identity(sha)
        build_id.update(cache_key=key, build_inputs=inputs, cache_hit=False)
        write_json(BUILD_IDENTITY, build_id)
        with tempfile.TemporaryDirectory(dir=root, prefix="publish-") as tmp:
            stage = Path(tmp) / "entry"
            stage.mkdir()
            for name in BINARIES:
                shutil.copy(DEBUG_DIR / name, stage / name)
            write_json(stage / "identity.json", build_id)
            stage.rename(cached)
        if sccache:
            command([sccache, "--show-stats"], overrides=environment, timeout=30)


def main():
    sha = os.environ["JIANYANG_COMMIT"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or capture(["git", "rev-parse", "HEAD"]).strip() != sha:
        raise ValueError("build source differs from admitted JY commit")
    command(["git", "diff", "--exit-code", "HEAD"], timeout=30)
    inputs = source_inputs(sha)
    key = cache_key(inputs)
    root = Path(os.environ["HOME"]) / ".cache/skippy/inference-build-v1"
    root.mkdir(parents=True, exist_ok=True)
    cached = root / key
    with lock(root / f"{key}.lock"):
        if cached.exists():
            try:
                build_id = read_json(cached / "identity.json")
                if build_id.get("cache_key") != key:
                    raise ValueError("cache source or toolchain key mismatch")
                check_executables(cached, build_id, sha)
            except (OSError, ValueError, KeyError) as error:
                print(f"Rejecting invalid inference cache entry: {error}", file=sys.stderr)
                shutil.rmtree(cached)
            else:
                install_binaries(cached)
                write_json(BUILD_IDENTITY, dict(build_id, cache_hit=True))
                print(f"Inference executable cache HIT: {key}")
                return
        print(f"Inference executable cache MISS: {key}", flush=True)
        build(sha, root, inputs, key, cached)


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
