"""Update Skippy workspace versions without changing dependency resolution."""

import re
import tomllib
from pathlib import Path


def parse_tag(tag):
    match = re.fullmatch(
        r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
        r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?", tag,
    )
    if not match or any(int(part) >= 2**64 for part in match.groups()[:3]):
        raise ValueError("release tag is not canonical SemVer")
    prerelease = match.group(4)
    if prerelease and any(
        len(part) > 1 and part.startswith("0") and part.isdigit()
        for part in prerelease.split(".")
    ):
        raise ValueError("release tag is not canonical SemVer")
    return prerelease is not None


def workspace_version(root=Path(".")):
    return tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]


def dependency_scopes(manifest):
    yield manifest
    yield manifest.get("workspace", {})
    yield from manifest.get("target", {}).values()


def manifest_edit(path, members, old, new, root):
    original = path.read_text()
    expected = tomllib.loads(original)
    if path == root:
        expected["workspace"]["package"]["version"] = new
    elif expected["package"].get("version") == old:
        expected["package"]["version"] = new
    for scope in dependency_scopes(expected):
        for group in ("dependencies", "build-dependencies", "dev-dependencies"):
            for dep in scope.get(group, {}).values():
                if (isinstance(dep, dict) and dep.get("version") == old
                        and "path" in dep and (path.parent / dep["path"]).resolve() in members):
                    dep["version"] = new
    # Retain formatting. Reject ambiguous edits, including an unrelated dependency
    # with the same literal, unless the parsed result matches the intended edit.
    content = re.sub(r'(?m)(\bversion[ \t]*=[ \t]*")' + re.escape(old) + '"',
                     lambda match: match[1] + new + '"', original)
    if tomllib.loads(content) != expected:
        raise ValueError(f"cannot update only release versions in {path}")
    return content


def lockfile_edit(path, packages, old, new):
    original = path.read_text()
    expected = tomllib.loads(original)
    pinned = {f"{name} {old}" for name in packages}
    locked = set()
    for entry in expected["package"]:
        if entry["name"] in packages and "source" not in entry:
            if entry["version"] != old:
                raise ValueError(f"lockfile version mismatch for {entry['name']}")
            entry["version"] = new
            locked.add(entry["name"])
        for index, dep in enumerate(entry.get("dependencies", [])):
            if dep in pinned:
                entry["dependencies"][index] = f"{dep.rsplit(' ', 1)[0]} {new}"
    if locked != packages:
        raise ValueError(f"workspace packages missing from lockfile: {sorted(packages - locked)}")
    sections = original.split("[[package]]")
    for index, section in enumerate(sections[1:], 1):
        entry = tomllib.loads(section)
        if entry["name"] in packages and "source" not in entry:
            section = section.replace(f'version = "{old}"', f'version = "{new}"', 1)
        for dep in entry.get("dependencies", []):
            if dep in pinned:
                section = section.replace(f'"{dep}"', f'"{dep.rsplit(" ", 1)[0]} {new}"')
        sections[index] = section
    content = "[[package]]".join(sections)
    if tomllib.loads(content) != expected:
        raise ValueError("release bump would change lockfile resolution")
    return content


def bump(tag, root=Path(".")):
    parse_tag(tag)
    root = root.resolve()
    manifest = root / "Cargo.toml"
    workspace = tomllib.loads(manifest.read_text())["workspace"]
    old, new = workspace["package"]["version"], tag[1:]
    excluded = {path.resolve() for pattern in workspace.get("exclude", [])
                for path in root.glob(pattern)}
    members = {path.resolve() for pattern in workspace["members"]
               for path in root.glob(pattern) if (path / "Cargo.toml").is_file()} - excluded
    release_members = {}
    for member in members:
        package = tomllib.loads((member / "Cargo.toml").read_text())["package"]
        if package["version"] in (old, {"workspace": True}):
            release_members[member] = package["name"]
    if not release_members:
        raise ValueError("workspace has no release-versioned packages")
    lockfile = root / "Cargo.lock"
    edits = {path: manifest_edit(path, release_members, old, new, manifest)
             for path in [manifest, *(member / "Cargo.toml" for member in sorted(members))]}
    edits[lockfile] = lockfile_edit(lockfile, set(release_members.values()), old, new)
    for path, content in edits.items():
        if content != path.read_text():
            path.write_text(content)
