"""Retain the portable native source recipe without architecture permissions."""

import hashlib

from .ci_artifacts import write_json
from .upgrade_source import PIN


def write_native_recipe(checkout):
    """Retain source identity for packaged embedding APIs without an architecture allowlist."""
    directory = checkout / "llama_cpp/patches"
    patches = [(path.name, path) for path in sorted(directory.glob("*.patch"))]
    for subdir in ("model_support", "generated"):
        folder = directory / subdir
        for name in (folder / "series").read_text().splitlines():
            if not name or "/" in name or "\\" in name:
                raise ValueError("invalid native patch series member")
            patches.append((f"{subdir}/{name}", folder / name))
    digest = hashlib.sha256()
    digest.update(b"mesh-llm-skippy-patch-queue-v2\0")
    digest.update(len(patches).to_bytes(8, "little"))
    for name, path in patches:
        for value in (name.encode("utf-8"), path.read_bytes()):
            digest.update(len(value).to_bytes(8, "little"))
            digest.update(value)
    write_json(checkout / "crates/skippy-api/src/native-recipe.json", {
        "llama_upstream_sha": (checkout / PIN).read_text().strip(),
        "patch_queue_sha256": digest.hexdigest(),
    })
