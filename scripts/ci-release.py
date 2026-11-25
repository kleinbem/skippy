#!/usr/bin/env python3
"""Admit release tags and inventory immutable release assets."""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from lib import ci_artifacts as artifacts
from lib.ci_process import cancel_on_signal, capture, command
from lib.ci_targets import targets, validate_native_identity
from lib.release_version import bump, parse_tag, workspace_version

# The host installer downloads this name; keep both in step with
# NativeRuntimeCatalog in crates/skippy-runtime-install/src/types.rs.
CATALOG = "native-runtimes.json"
RELEASES_URL = "https://github.com/Mesh-LLM/skippy/releases"


def version(tag):
    prerelease = parse_tag(tag)
    if workspace_version() != tag[1:]:
        raise ValueError("tag version differs from committed workspace version")
    return "true" if prerelease else "false"


def github_output(name, value):
    path = os.environ.get("GITHUB_OUTPUT")
    if path:
        with open(path, "a", encoding="utf-8") as stream:
            stream.write(f"{name}={value}\n")


def validate(tag):
    prerelease = version(tag)
    source = artifacts.source()
    if capture(["git", "rev-parse", f"refs/tags/{tag}^{{commit}}"]).strip() != source:
        raise ValueError("checkout is not the exact tagged commit")
    # A release is on main, or is the version commit a release run made on top of main.
    parents = capture(["git", "show", "-s", "--format=%P", source]).split()
    subject = capture(["git", "show", "-s", "--format=%s", source]).strip()
    tip = parents[0] if len(parents) == 1 and subject == f"release: prepare {tag}" else source
    for commit in dict.fromkeys((source, tip)):
        if subprocess.run(["git", "merge-base", "--is-ancestor", commit,
                           "refs/remotes/origin/main"], timeout=60).returncode == 0:
            break
    else:
        raise ValueError("release commit is not on main or a version commit on top of main")
    github_output("prerelease", prerelease)
    github_output("source", source)
    return source


def prepare(tag, input_dir):
    inventory(tag, input_dir, validate(tag))


def prepare_candidate(tag, input_dir):
    version(tag)
    inventory(tag, input_dir, artifacts.source())


def catalog_entry(runtime, tag, archive, sha256):
    """Describe one product archive as the download for the runtime it carries."""
    if runtime.get("release_version") != tag[1:]:
        raise ValueError(f'runtime {runtime.get("id")} was not built for release {tag[1:]}')
    return {**runtime, "url": f"{RELEASES_URL}/download/{tag}/{archive}", "sha256": sha256}


def catalog(tag, entries):
    abis = {entry["skippy_abi"] for entry in entries}
    if len(abis) != 1:
        raise ValueError("release runtimes disagree on the Skippy ABI")
    return {"schema_version": 2, "release_version": tag[1:], "skippy_abi": abis.pop(),
            "artifacts": entries}


def verify_catalog(value, tag, archives):
    """Every archive is the download of exactly one runtime of this release."""
    if (type(value) is not dict
            or set(value) != {"schema_version", "release_version", "skippy_abi", "artifacts"}
            or value["schema_version"] != 2 or value["release_version"] != tag[1:]
            or type(value["artifacts"]) is not list):
        raise ValueError("runtime catalog does not describe this release")
    downloads = {}
    for entry in value["artifacts"]:
        prefix = f"{RELEASES_URL}/download/{tag}/"
        if (type(entry) is not dict or not str(entry.get("url", "")).startswith(prefix)
                or entry.get("release_version") != tag[1:]
                or entry.get("skippy_abi") != value["skippy_abi"]):
            raise ValueError("runtime catalog entry does not belong to this release")
        downloads[entry["url"][len(prefix):]] = entry.get("sha256")
    if len(downloads) != len(value["artifacts"]) or downloads != archives:
        raise ValueError("runtime catalog does not match the release archives")


def verify_smoke(input_dir, variant, source, backend, archive_sha256):
    evidence_dir = input_dir / f"release-product-{variant}" / "evidence"
    if (evidence_dir / "artifact-smoke.json").is_file():
        evidence_dir = input_dir / f"release-device-evidence-{variant}"
    evidence = artifacts.read_json(evidence_dir / "smoke.json")
    if (evidence.get("source"), evidence.get("variant"), evidence.get("backend")) != (
        source, variant, backend
    ):
        raise ValueError(f"missing or mismatched smoke evidence for {variant}")
    if evidence.get("archive_sha256") != archive_sha256:
        raise ValueError(f"smoke evidence does not match release archive for {variant}")


def inventory(tag, input_dir, source):
    hosts, runtimes = targets()
    output = Path("dist/release")
    if output.exists() and any(output.iterdir()):
        raise ValueError("release output directory must be empty")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=output.parent, prefix=".release-") as tmp:
        stage = Path(tmp) / "release"
        stage.mkdir()
        assets, entries = {}, []
        for variant, runtime in runtimes.items():
            directory = input_dir / f"release-product-{variant}"
            host = hosts[runtime["host"]]
            with tempfile.TemporaryDirectory() as workspace:
                root = Path(workspace)
                artifacts.unpack(directory, root)
                artifacts.verify(root, "product", host["target"], variant)
                arches = runtime["gpu_architectures"] if runtime["backend"] == "cuda" else None
                validate_native_identity(root / "native-runtimes" / variant, host, runtime, arches)
                native = artifacts.read_json(root / "native-runtimes" / variant / "manifest.json")
            file = stage / f"skippy-{tag}-{variant}.tar.gz"
            shutil.copyfile(directory / f"skippy-{variant}.tar.gz", file)
            artifacts.write_checksum(file)
            assets[file.name] = artifacts.digest(file)
            verify_smoke(input_dir, variant, source, runtime["backend"], assets[file.name])
            entries.append(catalog_entry(native["runtime"], tag, file.name, assets[file.name]))
        runtime_catalog = stage / CATALOG
        artifacts.write_json(runtime_catalog, catalog(tag, entries))
        artifacts.write_checksum(runtime_catalog)
        assets[CATALOG] = artifacts.digest(runtime_catalog)
        index = stage / "release.json"
        artifacts.write_json(index, {
            "schema_version": 1, "tag": tag, "source": source,
            "upstream_pin": Path("llama_cpp/upstream.txt").read_text(encoding="utf-8").strip(),
            "assets": dict(sorted(assets.items())), "variants": list(runtimes),
        })
        artifacts.write_checksum(index)
        verify(stage)
        stage.rename(output)


def verify(directory):
    index = directory / "release.json"
    release = artifacts.read_json(index)
    _, runtimes = targets()
    if (
        set(release) != {"schema_version", "tag", "source", "upstream_pin", "assets", "variants"}
        or type(release["schema_version"]) is not int or release["schema_version"] != 1
        or release["source"] != artifacts.source() or release["variants"] != list(runtimes)
        or not isinstance(release["upstream_pin"], str)
    ):
        raise ValueError("release roster/source mismatch")
    tag = release["tag"]
    version(tag)
    archives = {f"skippy-{tag}-{variant}.tar.gz" for variant in runtimes}
    filenames = archives | {CATALOG}
    if type(release["assets"]) is not dict or set(release["assets"]) != filenames:
        raise ValueError("release archive roster mismatch")
    for name, expected in release["assets"].items():
        file = directory / name
        if artifacts.digest(file) != expected:
            raise ValueError(f"release archive identity mismatch: {name}")
        artifacts.check_checksum(file)
    artifacts.check_checksum(index)
    verify_catalog(artifacts.read_json(directory / CATALOG), tag,
                   {name: release["assets"][name] for name in archives})
    assets = filenames | {index.name}
    if {file.name for file in directory.iterdir()} != assets | {name + ".sha256" for name in assets}:
        raise ValueError("release contains unrecognized assets")
    return release


def admit_draft(path):
    release = artifacts.read_json(path)
    if release.get("isDraft") is not True:
        raise ValueError("refusing to overwrite a published release")
    source = validate(release["tagName"])
    if release.get("body") != f"Skippy source: {source}":
        raise ValueError("draft source identity does not match checkout")


def select(tag, path):
    validate(tag)
    releases = artifacts.read_json(path)
    if not isinstance(releases, list):
        raise ValueError("expected a release list")
    matches = [release for release in releases if release.get("tagName") == tag]
    if len(matches) > 1:
        raise ValueError("duplicate release tags in API response")
    github_output("exists", "true" if matches else "false")
    github_output("published", "true" if matches and matches[0]["isDraft"] is False else "false")


def upload_plan(directory, path):
    local = verify(directory)
    remote = artifacts.read_json(path)
    if (remote.get("draft") is not True or remote.get("tag_name") != local["tag"]
            or remote.get("body") != f'Skippy source: {local["source"]}'):
        raise ValueError("refusing a published or foreign draft release")
    if not isinstance(remote.get("assets"), list):
        raise ValueError("release API omitted assets")
    missing = artifacts.files(directory)
    for asset in remote["assets"]:
        name = asset["name"]
        expected = missing.pop(name, None)
        if expected is None or asset.get("digest") != "sha256:" + expected:
            raise ValueError(f"existing draft asset differs or has no verified digest: {name}")
    Path("release-upload.txt").write_text(
        "".join(f"{directory / name}\n" for name in missing), encoding="utf-8"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(dest="action", required=True)
    for func, arguments in (
        (bump, (("tag", str),)),
        (validate, (("tag", str),)),
        (prepare, (("tag", str), ("input_dir", Path))),
        (prepare_candidate, (("tag", str), ("input_dir", Path))),
        (verify, (("directory", Path),)),
        (admit_draft, (("path", Path),)),
        (select, (("tag", str), ("path", Path))),
        (upload_plan, (("directory", Path), ("path", Path))),
    ):
        action = actions.add_parser(func.__name__.replace("_", "-"))
        for name, kind in arguments:
            action.add_argument(name, type=kind)
        action.set_defaults(run=func, names=[name for name, _ in arguments])
    args = parser.parse_args()
    args.run(*(getattr(args, name) for name in args.names))


if __name__ == "__main__":
    try:
        with cancel_on_signal():
            main()
    except (OSError, ValueError, TypeError, KeyError, RuntimeError, subprocess.SubprocessError, KeyboardInterrupt) as error:
        sys.exit(str(error))
