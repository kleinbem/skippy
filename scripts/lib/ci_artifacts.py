"""Read and write source-bound CI producer archives and their receipts."""

import hashlib
import json
import os
import shutil
import tarfile
from pathlib import Path, PurePosixPath

from .ci_process import capture


def digest(path):
    state = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(65536), b""):
            state.update(block)
    return state.hexdigest()


def source():
    if capture(["git", "status", "--porcelain", "--untracked-files=no"]).strip():
        raise ValueError("artifact provenance requires an unchanged tracked source tree")
    return capture(["git", "rev-parse", "HEAD"]).strip()


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path, value):
    path.write_bytes(json.dumps(value, ensure_ascii=False, indent=2).encode("utf-8"))


def files(root):
    directory = root.resolve(strict=True)
    result = {}

    def collect(parent):
        for entry in parent.iterdir():
            if not entry.resolve(strict=True).is_relative_to(directory):
                raise ValueError(f"artifact link escapes its root: {entry}")
            if entry.is_dir() and not entry.is_symlink():
                collect(entry)
            elif entry.is_file():
                relative = entry.relative_to(directory).as_posix()
                relative.encode("utf-8")
                if "\\" in relative:
                    raise ValueError(f"invalid artifact path: {relative}")
                result[relative] = digest(entry)
            else:
                raise ValueError(f"unsupported artifact entry: {entry}")

    collect(directory)
    return dict(sorted(result.items()))


def receipt(root, kind, target, variant):
    write_json(root / "provenance.json", {
        "schema_version": 1, "source": source(), "kind": kind, "target": target,
        "variant": variant, "files": files(root),
    })


def verify(root, kind, target, variant):
    value = read_json(root / "provenance.json")
    actual = files(root)
    actual.pop("provenance.json", None)
    identity = {"schema_version": 1, "source": source(), "kind": kind,
                "target": target, "variant": variant}
    if type(value) is not dict or set(value) != {*identity, "files"} or any(
        type(value[key]) is not type(expected) or value[key] != expected
        for key, expected in identity.items()
    ):
        raise ValueError("artifact source/target/variant identity mismatch")
    if value["files"] != actual:
        raise ValueError("artifact file roster or checksum mismatch")


def checksum_line(path):
    return f"{digest(path)}  {path.name}\n".encode("utf-8")


def write_checksum(path):
    path.with_name(path.name + ".sha256").write_bytes(checksum_line(path))


def check_checksum(path):
    if path.with_name(path.name + ".sha256").read_bytes() != checksum_line(path):
        raise ValueError(f"checksum mismatch: {path}")


def archive(root, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tarfile.open(destination, "w:gz", format=tarfile.GNU_FORMAT) as output:
        output.add(root, arcname=".")
    write_checksum(destination)


def archive_path(name):
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts or "\\" in name or ":" in name:
        raise ValueError(f"invalid artifact archive path: {name}")
    return path


def link_target(path, value):
    if not value or value.startswith("/") or "\\" in value or ":" in value:
        raise ValueError(f"invalid artifact link: {path}")
    parts = list(path.parent.parts)
    for part in PurePosixPath(value).parts:
        if part == "..":
            if not parts:
                raise ValueError(f"artifact link escapes its root: {path}")
            parts.pop()
        else:
            parts.append(part)
    return PurePosixPath(*parts)


def unpack(directory, destination):
    archives = list(directory.glob("*.tar.gz"))
    if len(archives) != 1:
        raise ValueError("expected exactly one producer archive")
    check_checksum(archives[0])
    extract_archive(archives[0], destination)


def extract_archive(archive, destination, allow_symlinks=True):
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError("producer archive destination must be empty")

    with tarfile.open(archive, "r:*") as archive_file:
        members = {}
        for member in archive_file:
            path = archive_path(member.name)
            if path in members or (not path.parts and not member.isdir()):
                raise ValueError(f"invalid duplicate or root archive entry: {member.name}")
            if not (member.isdir() or member.isfile() or member.issym() or member.islnk()):
                raise ValueError(f"unsupported artifact archive entry: {member.name}")
            if member.issym() and not allow_symlinks:
                raise ValueError(f"runtime archive contains a symlink: {member.name}")
            members[path] = member

        for path in members:
            for parent in path.parents:
                if parent in members and not members[parent].isdir():
                    raise ValueError(f"archive entry nested under a file or link: {path}")

        for path, member in members.items():
            output = destination / path
            if member.isdir():
                output.mkdir(parents=True, exist_ok=True)
            elif member.isfile():
                output.parent.mkdir(parents=True, exist_ok=True)
                with archive_file.extractfile(member) as stream, output.open("xb") as file:
                    shutil.copyfileobj(stream, file)
                output.chmod(member.mode & 0o777)

        for path, member in members.items():
            if member.islnk():
                target = archive_path(member.linkname)
                if target not in members or not members[target].isfile():
                    raise ValueError(f"hardlink does not target an archived file: {path}")
                output = destination / path
                output.parent.mkdir(parents=True, exist_ok=True)
                os.link(destination / target, output)

        for path, member in members.items():
            if member.issym():
                link_target(path, member.linkname)
                output = destination / path
                output.parent.mkdir(parents=True, exist_ok=True)
                output.symlink_to(member.linkname)
    files(destination)


def copy_tree(root, destination):
    destination.mkdir(parents=True, exist_ok=True)
    for path in files(root):
        output = destination / path
        output.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy(root / path, output)


def binary(root, operating_system):
    return root / ("skippy.exe" if operating_system == "windows" else "skippy")
