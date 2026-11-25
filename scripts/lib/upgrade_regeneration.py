"""Regenerate patch shards, bindings, and the public API reference."""

import os
import platform
import shutil
import sys
from pathlib import Path

from .ci_process import capture, command
from .upgrade_source import PIN, git

SCRIPTS = Path(__file__).resolve().parents[1]


def patches_only(source, destination, exclude_generated=False):
    destination.mkdir(parents=True, exist_ok=True)
    for item in source.iterdir():
        if exclude_generated and item.name == "generated":
            continue
        output = destination / item.name
        if item.is_symlink():
            raise ValueError("native patch input must contain regular files only")
        if item.is_dir():
            patches_only(item, output)
        elif item.is_file():
            shutil.copyfile(item, output)
        else:
            raise ValueError("native patch input must contain regular files only")


def regenerate(checkout):
    scratch = checkout / "target/llama-regeneration"
    patches, native, tools = (scratch / name for name in ("core-patches", "native", "rewriter"))
    patches_only(checkout / "llama_cpp/patches", patches, exclude_generated=True)
    source = checkout / ".deps/llama.cpp"
    env = {"LLAMA_WORKDIR": str(source), "LLAMA_PIN_FILE": str(checkout / PIN),
           "LLAMA_PATCH_DIR": str(patches), "LLAMA_STAGE_BUILD_DIR": str(native),
           "LLAMA_BUILD_DIR": str(native), "LLAMA_STAGE_BACKEND": "cpu", "LLAMA_STAGE_LINK_MODE": "static"}
    just = ["just", "--justfile", str(checkout / "Justfile")]
    command([*just, "llama-build"], overrides=env, timeout=7200)
    base = git(source, "rev-parse", "HEAD")
    llvm = Path(os.environ["SKIPPY_REWRITER_LLVM_PREFIX"]).resolve(strict=True)
    command(["cmake", "-S", str(checkout / "scripts/tools/skippy-stage-rewriter"), "-B", str(tools),
             "-G", "Ninja", f"-DLLVM_DIR={llvm / 'lib/cmake/llvm'}",
             f"-DClang_DIR={llvm / 'lib/cmake/clang'}"], timeout=300)
    command(["cmake", "--build", str(tools), "--parallel"], timeout=3600)
    command(["ctest", "--test-dir", str(tools), "--output-on-failure", "--no-tests=error"], timeout=600)
    generated, report = scratch / "generated", scratch / "report.json"
    family_map = checkout / "ci/llama-canary/generated-family-map.json"
    manifest = checkout / "ci/llama-canary/model-fixtures.json"
    argv = [sys.executable, str(SCRIPTS / "generate-skippy-family-patch.py"), "generate",
            "--git", shutil.which("git"), "--source-root", str(source), "--build-dir", str(native),
            "--rewriter", str(tools / "skippy-stage-rewriter"), "--report", str(report),
            "--diff-base", base, "--output", str(scratch / "generated.patch"),
            "--shard-output-dir", str(generated), "--family-source-map", str(family_map),
            "--family-manifest", str(manifest), "--max-diff-bytes", "16777216",
            "--extra-arg", "-resource-dir", "--extra-arg",
            capture([str(llvm / "bin/clang"), "-print-resource-dir"]).strip()]
    if platform.system() == "Darwin":
        argv += ["--extra-arg", "-isysroot", "--extra-arg", capture(["xcrun", "--show-sdk-path"]).strip()]
    command(argv, timeout=1500)
    command([sys.executable, str(SCRIPTS / "verify-skippy-family-generator-coverage.py"),
             "--manifest", str(manifest), "--family-map", str(family_map), "--report", str(report)])
    command([sys.executable, str(SCRIPTS / "graph-contract-coverage.py"),
             "--root", str(checkout), "--source-root", str(source), "--report", str(report),
             "--output", str(scratch / "graph-coverage.json")])
    destination = checkout / "llama_cpp/patches/generated"
    shutil.rmtree(destination)
    patches_only(generated, destination)
    env.pop("LLAMA_PATCH_DIR")
    command([*just, "llama-build"], overrides=env, timeout=7200)
    command([*just, "with-lld", "cargo", "check", "--locked", "-p", "skippy-ffi"],
            overrides=dict(env, SKIPPY_FFI_BINDGEN="regen"), timeout=3600)
    command([sys.executable, str(SCRIPTS / "generate-skippy-api-doc.py"),
             "--include-dir", str(source / "include/skippy"), "--output", str(checkout / "docs/native-api.md")])
