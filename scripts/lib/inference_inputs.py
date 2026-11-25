"""Admit native inference executables and immutable model inputs."""

import fcntl
import os
import re
import shutil
import tempfile
from contextlib import contextmanager
from pathlib import Path

from .ci_artifacts import digest, read_json, write_json
from .ci_process import capture, command

BINARIES = {"skippy": "cli_sha256", "skippy-correctness": "correctness_sha256"}
INPUT_MANIFESTS = {
    "model_manifest_sha256": "ci/inference-artifacts.json",
    "client_lock_sha256": "ci/aiperf-requirements.lock",
    "corpus_manifest_sha256": "ci/inference-corpus.json",
}
DEBUG_DIR = Path("target/debug")
BUILD_IDENTITY = Path("target/inference-build.json")


@contextmanager
def lock(path):
    with path.open("a+b") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX)
        yield


def input_hashes():
    return {field: digest(Path(path)) for field, path in INPUT_MANIFESTS.items()}


def check_executables(directory, identity, sha):
    if identity.get("stack_commit") != sha or any(
        digest(directory / name) != identity.get(field) for name, field in BINARIES.items()
    ):
        raise ValueError("cached inference executable provenance mismatch")


def record(output, sha):
    identity = read_json(BUILD_IDENTITY)
    check_executables(DEBUG_DIR, identity, sha)
    identity.update(input_hashes(), harness="aiperf-0.13.0-python-3.12")
    write_json(output / "provenance.json", identity)


def identity(sha):
    source = Path("crates/skippy-ffi/src/lib.rs").read_text(encoding="utf-8")
    abi = []
    for component in ("MAJOR", "MINOR", "PATCH"):
        match = re.search(rf"^pub const ABI_VERSION_{component}: u32 = ([0-9]+);", source, re.M)
        if not match:
            raise ValueError("native ABI source constant missing")
        abi.append(int(match[1]))
    cache = next(Path(".deps/llama-build").glob("*/CMakeCache.txt"), None)
    if cache is None:
        raise ValueError("native CUDA build cache missing")
    compiler = next((line.partition("=")[2] for line in cache.read_text(encoding="utf-8").splitlines()
                     if line.startswith("CMAKE_CUDA_COMPILER:")), None)
    if not compiler:
        raise ValueError("native CUDA compiler missing from build cache")
    return {
        "stack_commit": sha, "native_upstream": Path("llama_cpp/upstream.txt").read_text().strip(),
        "native_tree": capture(["git", "-C", ".deps/llama.cpp", "rev-parse", "HEAD^{tree}"]).strip(),
        "native_abi": abi, "harness": "aiperf-0.13.0-python-3.12",
        "cuda_compiler": capture([compiler, "--version"]), **input_hashes(),
        **{field: digest(DEBUG_DIR / name) for name, field in BINARIES.items()},
    }


def safe_relative(value):
    path = Path(value)
    if not path.parts or path.is_absolute() or ".." in path.parts or "\\" in value:
        raise ValueError(f"invalid fixture path: {value}")
    return path


def fixture(key):
    fixtures = read_json(Path(INPUT_MANIFESTS["model_manifest_sha256"]))
    value = fixtures[key]
    if not re.fullmatch(r"[0-9a-f]{40}", value["revision"]) or not value["files"]:
        raise ValueError("unpinned fixture")
    names = set()
    for file in value["files"]:
        name = safe_relative(file["path"]).name
        if name in names or not re.fullmatch(r"[0-9a-f]{64}", file["sha256"]):
            raise ValueError("invalid or duplicate model shard")
        if type(file["size"]) is not int or file["size"] <= 0:
            raise ValueError("invalid model shard size")
        names.add(name)
    return value


def downloaded_path(output):
    for line in reversed(output.splitlines()):
        value = line.strip()
        if value.startswith(("path:", "path=")):
            value = value[5:].strip()
        if value.startswith("/") and "\x1b" not in value:
            return Path(value)
    raise ValueError("hf download returned no absolute artifact path")


def stage(source, destination):
    destination.unlink(missing_ok=True)
    try:
        os.link(source.resolve(strict=True), destination)
    except OSError:
        shutil.copyfile(source, destination)


def valid_model(path, row):
    return path.stat().st_size == row["size"] and digest(path) == row["sha256"]


def download(row, model):
    path, repo, revision = row["path"], model["repo"], model["revision"]
    if os.environ.get("HF_HUB_CACHE"):
        cached = (Path(os.environ["HF_HUB_CACHE"]) / ("models--" + repo.replace("/", "--"))
                  / "snapshots" / revision / safe_relative(path))
        if cached.is_file():
            return cached
    args = ["hf", "download", repo, "--revision", revision, path]
    command(args, timeout=18000, overrides={"HF_HUB_DISABLE_XET": "1"})
    return downloaded_path(capture([*args, "--quiet"]))


def admit(key, model):
    destination = Path("target/inference-models") / key
    local = Path(os.environ["HOME"]) / ".cache/skippy/inference-models"
    for directory in (destination, local):
        directory.mkdir(parents=True, exist_ok=True)
    paths = []
    for row in model["files"]:
        cached = local / row["sha256"]
        target = destination / Path(row["path"]).name
        with lock(cached.with_suffix(".lock")):
            if cached.is_file() and valid_model(cached, row):
                print(f'Verified local model cache HIT: {row["path"]}', flush=True)
            else:
                print(f'Admitting {model["repo"]}@{model["revision"]}: {row["path"]}', flush=True)
                with tempfile.TemporaryDirectory(dir=local) as tmp:
                    staged = Path(tmp) / "model"
                    shutil.copyfile(download(row, model), staged)
                    if not valid_model(staged, row):
                        raise ValueError("model artifact digest mismatch")
                    with staged.open("rb+") as stream:
                        os.fsync(stream.fileno())
                    staged.replace(cached)
            stage(cached, target)
        paths.append(target.resolve(strict=True))
    return paths[0]
