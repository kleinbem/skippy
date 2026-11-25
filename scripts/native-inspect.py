#!/usr/bin/env python3
"""Inspect native imports and verify the contents of packaged runtimes."""

import argparse
import json
import re
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

from lib.ci_artifacts import archive_path, digest, extract_archive, read_json, write_json
from lib.ci_process import cancel_on_signal, command
from lib.native_binary import inspect
from lib.native_dependencies import Dependencies
from lib.native_policy import HOST_TARGETS, NATIVE_TARGETS, basename, forbidden, host_owned


def check_glibc(path, floor):
    if floor == "declared":
        floor = Path(__file__).with_name("linux-glibc-floor.txt").read_text(encoding="utf-8")
    declared = [line.strip() for line in floor.splitlines()
                if line.strip() and not line.lstrip().startswith("#")]
    if len(declared) != 1 or not re.fullmatch(r"[0-9]+\.[0-9]+(?:\.[0-9]+)*", declared[0]):
        raise ValueError("GLIBC floor must contain exactly one version")
    maximum = tuple(map(int, declared[0].split(".")[:2]))
    output = command(["readelf", "-V", str(path)], overrides={"LC_ALL": "C"}, capture_stdout=True)
    needs = output.partition("Version needs section")[2]
    for major, minor in re.findall(r"GLIBC_([0-9]+)\.([0-9]+)", needs):
        if (int(major), int(minor)) > maximum:
            raise ValueError(f"GLIBC_{major}.{minor} exceeds {declared[0]}")
    if "GLIBC_ABI_DT_RELR" in needs and (2, 36) > maximum:
        raise ValueError("GLIBC_ABI_DT_RELR requires GLIBC 2.36")


def verify_host(args):
    identity = digest(args.binary)
    image = inspect(args.binary)
    if args.format and args.format != image.kind:
        raise ValueError("binary format mismatch")
    if image.kind == "elf" and args.max_glibc:
        check_glibc(args.binary, args.max_glibc)
    rejected = [] if args.no_import_policy else [name for name in image.imports if forbidden(name)]
    if digest(args.binary) != identity:
        raise ValueError("binary changed during inspection")
    report = {
        "binary": args.binary.name, "binary_sha256": identity, "format": image.kind,
        "arch": image.arch, "policy": "none" if args.no_import_policy else "skippy-dynamic-host-v1",
        "imports": image.imports, "rejected_imports": rejected,
    }
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        write_json(args.report, report)
    else:
        print(json.dumps(report, indent=2, ensure_ascii=False))
    if rejected:
        raise ValueError("backend-neutral host imports native backend libraries")


def cli_input(args):
    report = read_json(args.input)
    image = inspect(args.binary)
    if (
        (image.kind, image.arch) != HOST_TARGETS.get(args.target)
        or any(forbidden(name) for name in image.imports)
        or report.get("policy") != "skippy-dynamic-host-v1"
        or report.get("binary_sha256") != digest(args.binary)
        or report.get("imports") != image.imports
    ):
        raise ValueError("invalid host import report or native host identity")


def contained_file(root, name):
    relative = archive_path(name)
    if not relative.parts:
        raise ValueError("empty runtime file path")
    path = root / relative
    if path.is_symlink():
        raise ValueError("runtime manifest file is a symlink")
    path = path.resolve(strict=True)
    if not path.is_relative_to(root) or not path.is_file():
        raise ValueError(f"runtime file escapes bundle: {name}")
    return path


def verify_directory(root):
    root = root.resolve(strict=True)
    manifest = read_json(root / "manifest.json")
    runtime, build = manifest["runtime"], manifest["build"]
    if (type(manifest["schema_version"]) is not int or manifest["schema_version"] != 2
            or not runtime["id"].startswith("skippy-")
            or not all(isinstance(runtime[key], str) and runtime[key]
                       for key in ("release_version", "skippy_abi"))):
        raise ValueError("invalid standalone runtime identity")
    platform = runtime["platform"]
    if NATIVE_TARGETS.get(platform["target"]) != (platform["os"], platform["arch"]):
        raise ValueError("runtime OS/architecture does not match target")
    for key in ("llama_upstream_sha", "llama_patched_sha"):
        if not re.fullmatch(r"[0-9a-fA-F]{40}", build[key]):
            raise ValueError("invalid native source SHA")
    if not isinstance(build["llama_patch_digest"], str) or len(build["llama_patch_digest"]) != 64:
        raise ValueError("invalid patch digest")
    files, tools, libraries = runtime["files"], runtime["tools"], runtime["libraries"]
    names = set()
    for group in (files, tools):
        for name, expected in group.items():
            path = contained_file(root, name)
            if digest(path) != expected:
                raise ValueError(f"runtime checksum mismatch: {name}")
            names.add(path.name.lower())
    primary = build["primary_library"]
    if (not isinstance(libraries, list) or primary not in libraries or primary not in files
            or files[primary] != build["library_sha256"]):
        raise ValueError("invalid native primary library")
    for name in [*libraries, *tools]:
        if name not in files and name not in tools:
            raise ValueError(f"unhashed runtime library: {name}")
        image = inspect(contained_file(root, name))
        if image.arch != platform["arch"]:
            raise ValueError(f"runtime architecture mismatch: {name}")
        for imported in image.imports:
            if host_owned(imported, image.kind):
                continue
            if basename(imported).lower() not in names:
                raise ValueError(f"unpackaged native dependency: {name} -> {imported}")
            if image.kind == "macho" and not imported.startswith(("@rpath/", "@loader_path/")):
                raise ValueError(f"nonrelocatable native import: {imported}")
    print(f'verified runtime {runtime["id"]}')


def verify_runtime(args):
    if args.input.is_dir():
        verify_directory(args.input)
        return
    with tempfile.TemporaryDirectory() as tmp:
        extract_archive(args.input, Path(tmp), allow_symlinks=False)
        roots = list(Path(tmp).iterdir())
        if len(roots) != 1 or not roots[0].is_dir():
            raise ValueError("runtime archive must contain one root directory")
        verify_directory(roots[0])


def dependency_command(args):
    package = Dependencies(args)
    getattr(package, args.verb)()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    host = actions.add_parser("verify-host-dependencies")
    host.add_argument("binary", type=Path)
    host.add_argument("--report", type=Path)
    host.add_argument("--max-glibc")
    host.add_argument("--format")
    for flag in ("--json", "--bind-sha256", "--no-import-policy"):
        host.add_argument(flag, action="store_true")
    host.set_defaults(run=verify_host)
    cli = actions.add_parser("skippy-cli-input")
    cli.add_argument("input", type=Path)
    cli.add_argument("binary", type=Path)
    cli.add_argument("target")
    cli.set_defaults(run=cli_input)
    runtime = actions.add_parser("verify-runtime-package")
    runtime.add_argument("--portable", action="store_true")
    runtime.add_argument("input", type=Path)
    runtime.set_defaults(run=verify_runtime)
    for name in ("linux-runtime-deps", "windows-runtime-deps"):
        deps = actions.add_parser(name)
        deps.add_argument("verb", choices=["collect", "verify", "order"])
        deps.add_argument("--lib-dir", type=Path, required=True)
        deps.add_argument("--scan-dir", type=Path, action="append", default=[])
        deps.add_argument("--search-dir", type=Path, action="append", default=[])
        deps.add_argument("--arch")
        deps.add_argument("--cuda-major")
        deps.add_argument("--primary")
        deps.set_defaults(run=dependency_command)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, KeyError, TypeError, RuntimeError, tarfile.TarError,
            subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
