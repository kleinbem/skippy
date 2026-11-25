#!/usr/bin/env python3
"""Write the versioned native runtime manifest from staged package files."""

import os
import re
import subprocess
import sys
from pathlib import Path

from lib.ci_artifacts import digest
from lib.ci_process import capture
from lib.native_policy import NATIVE_TARGETS


def hashes(root, paths):
    result = {}
    for relative in paths:
        if (not relative or relative.startswith("/") or ".." in relative.split("/")
                or any(ch in relative for ch in ("\\", "\0", ":"))):
            raise ValueError(f"unsafe artifact path: {relative}")
        path = (root / relative).resolve(strict=True)
        if not path.is_relative_to(root) or not path.is_file():
            raise ValueError(f"artifact file escapes root or is not a file: {relative}")
        result[relative] = digest(path)
    return result


def glibc_floor(root, paths):
    versions = []
    for relative in paths:
        path = root / relative
        with path.open("rb") as stream:
            if stream.read(4) != b"\x7fELF":
                continue
        needs = capture(["readelf", "-V", str(path)]).partition("Version needs section")[2]
        if "GLIBC_ABI_DT_RELR" in needs:
            versions.append((2, 36))
        versions.extend((int(major), int(minor))
                        for major, minor in re.findall(r"GLIBC_([0-9]+)\.([0-9]+)", needs))
    return ".".join(map(str, max(versions))) if versions else None


def arches(primary, fallback, default=""):
    value = os.environ.get(primary) or os.environ.get(fallback) or default
    return [part.strip() for part in re.split(r"[,;]", value) if part.strip()]


def add_environment(target, key, variable):
    value = os.environ.get(variable)
    if value:
        target[key] = value


def admit_rocm(root, targets, files):
    directory = root / "lib/rocblas/library"
    objects = hashes(root, sorted(
        path.relative_to(root).as_posix() for path in directory.iterdir()
        if path.suffix.lower() in {".co", ".hsaco"}
    ))
    kept, dropped = [], []
    for target in targets:
        pattern = rf"(?<![A-Za-z0-9]){re.escape(target)}(?![A-Za-z0-9])"
        bucket = kept if any(re.search(pattern, Path(name).name) for name in objects) else dropped
        bucket.append(target)
    if not kept:
        raise ValueError(f"no packaged rocBLAS kernels for any ROCm target: {' '.join(dropped)}")
    if dropped:
        print(f"not advertising {' '.join(dropped)}: no packaged rocBLAS code objects", file=sys.stderr)
    files.update(objects)
    return kept


def backend_metadata(backend, cuda_major, operating_system, root, files):
    if backend in {"cpu", "metal"}:
        return {"kind": backend}
    if backend in {"cuda", "cuda-blackwell"}:
        if not re.fullmatch(r"\+?[0-9]+", cuda_major) or int(cuda_major) >= 2**32:
            raise ValueError("invalid CUDA toolkit major")
        cuda = {"toolkit_major": int(cuda_major), "gpu_arches": arches(
            "LLAMA_STAGE_CUDA_ARCHITECTURES", "SKIPPY_CUDA_ARCHITECTURES",
            "sm_120" if backend == "cuda-blackwell" else "",
        )}
        add_environment(cuda, "min_driver", "SKIPPY_CUDA_MIN_DRIVER")
        return {"kind": "cuda", "cuda": cuda}
    if backend in {"rocm", "hip"}:
        targets = arches("LLAMA_STAGE_AMDGPU_TARGETS", "SKIPPY_AMDGPU_TARGETS")
        if operating_system == "windows" and targets and any(
            path.lower() == "lib/rocblas.dll" for path in files
        ):
            targets = admit_rocm(root, targets, files)
        rocm = {"gpu_arches": targets}
        add_environment(rocm, "version", "SKIPPY_ROCM_VERSION")
        return {"kind": "rocm", "rocm": rocm}
    if backend == "vulkan":
        vulkan = {}
        add_environment(vulkan, "min_api_version", "SKIPPY_VULKAN_MIN_API_VERSION")
        return {"kind": "vulkan", "vulkan": vulkan}
    raise ValueError(f"unsupported runtime backend: {backend}")


def optional(value):
    return value if value not in {"", "-"} else None


def write_manifest(args):
    if args[:1] == ["--schema-v2"]:
        args = args[1:]
    if len(args) < 17:
        raise ValueError("usage: native-manifest.py MANIFEST ID VERSION ABI OS ARCH TARGET PLATFORM "
                         "BACKEND CUDA_MAJOR PRIMARY UPSTREAM PATCHED PATCH_DIGEST "
                         "LIBRARIES -- TOOLS -- LICENSES -- RELOCATABLE")
    (output, identity, version, abi, os_name, arch, target, platform, backend, cuda_major,
     primary, upstream, patched, patch_digest, *rest) = args
    groups = [[]]
    for value in rest:
        if value == "--":
            groups.append([])
        else:
            groups[-1].append(value)
    if len(groups) != 4:
        raise ValueError("expected libraries, tools, licenses and relocatable groups")
    libraries, tools, licenses, relocatable = groups
    if NATIVE_TARGETS.get(target) != (os_name, arch):
        raise ValueError(f"unsupported native target or OS/architecture mismatch: {target}")
    if not identity or not version or not abi:
        raise ValueError("runtime id, release version and Skippy ABI must be non-empty")
    if (backend == "metal" and os_name != "macos") or (os_name == "macos" and backend not in {"cpu", "metal"}):
        raise ValueError("runtime backend is unsupported on selected OS")
    if primary not in libraries or set(relocatable) - set(libraries):
        raise ValueError("primary and relocatable libraries must belong to libraries")
    rank = os.environ.get("SKIPPY_NATIVE_RUNTIME_RANK") or "0"
    if not re.fullmatch(r"[+-]?[0-9]+", rank) or not -2**63 <= int(rank) < 2**63:
        raise ValueError("invalid native runtime rank")
    path = Path(output)
    root = path.parent.resolve(strict=True)
    files = hashes(root, [*libraries, *licenses])
    tool_hashes = hashes(root, tools)
    document = {
        "schema_version": 2,
        "runtime": {
            "id": identity, "release_version": version, "skippy_abi": abi,
            "platform": {"os": os_name, "arch": arch, "target": target,
                         "min_glibc": glibc_floor(root, [*libraries, *tools]) if os_name == "linux" else None},
            "backend": backend_metadata(backend, cuda_major, os_name, root, files),
            "rank": int(rank), "libraries": libraries, "files": files, "tools": tool_hashes,
            "url": None, "sha256": None, "signature": None,
        },
        "build": {
            "platform": platform, "backend": backend, "primary_library": primary,
            "relocatable_libraries": relocatable if os_name == "linux" else [],
            "library_sha256": files[primary], "llama_upstream_sha": optional(upstream),
            "llama_patched_sha": optional(patched), "llama_patch_digest": optional(patch_digest),
        },
    }
    import json
    path.write_bytes((json.dumps(document, ensure_ascii=False, sort_keys=True, indent=2) + "\n").encode("utf-8"))


if __name__ == "__main__":
    try:
        write_manifest(sys.argv[1:])
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
