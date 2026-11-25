"""Seed old patch-result blobs so cold repositories can rebase ordered queues."""

from .ci_process import command
from .upgrade_source import PIN, validate_sha


def seed_patch_preimages(checkout, base_pin, base_patches):
    validate_sha(base_pin)
    if (checkout / PIN).read_text().strip() == base_pin:
        return
    # Later queue entries reference results of earlier patches, not only
    # upstream blobs. Use the protected base queue, even after candidate repair.
    # A fresh upstream clone cannot reconstruct those bases when earlier
    # patches produce different results on the new revision.
    directory = checkout / "target/llama-regeneration"
    directory.mkdir(parents=True, exist_ok=True)
    pin = directory / "base-upstream.txt"
    pin.write_text(base_pin + "\n", encoding="utf-8")
    print(f"Preparing current pin {base_pin} to seed patch merge bases")
    command(["just", "--justfile", str(checkout / "Justfile"), "llama-prepare"], overrides={
        "LLAMA_WORKDIR": str(checkout / ".deps/llama.cpp"),
        "LLAMA_PIN_FILE": str(pin), "LLAMA_PATCH_DIR": str(base_patches),
    }, timeout=3600)
