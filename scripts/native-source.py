#!/usr/bin/env python3
"""Read native package identities from their authoritative source files."""

import json
import re
import sys
from pathlib import Path


def source_version(kind, path):
    text = Path(path).read_bytes().decode("utf-8")
    if kind == "workspace":
        eligible = False
        for line in re.split(r"[\r\n]", text):
            line = line.strip()
            if line == "[workspace.package]":
                eligible = True
                continue
            if line.startswith("["):
                eligible = False
            if eligible:
                match = re.match(r'^version\s*=\s*"([^"]+)"', line)
                if match:
                    return match.group(1)
        raise ValueError("workspace package version not found")
    if kind == "abi":
        components = {}
        for line in re.split(r"[\r\n]", text):
            match = re.match(r"^pub const ABI_VERSION_(MAJOR|MINOR|PATCH): u32 = ([0-9]+);", line.strip())
            if match:
                components[match.group(1)] = match.group(2)
        for name in ("MAJOR", "MINOR", "PATCH"):
            if name not in components:
                raise ValueError("missing ABI component '{}'".format(name))
        return ".".join(components[name] for name in ("MAJOR", "MINOR", "PATCH"))
    raise ValueError("unknown source version: {}".format(kind))


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "cargo-target-directory":
        path = json.load(sys.stdin)["target_directory"]
        if not isinstance(path, str) or not Path(path).is_absolute():
            raise ValueError("Cargo target directory must be absolute")
        print(path)
    elif len(sys.argv) == 4 and sys.argv[1] == "package-source-version":
        print(source_version(sys.argv[2], sys.argv[3]))
    else:
        raise ValueError("usage: native-source.py cargo-target-directory|package-source-version KIND FILE")


if __name__ == "__main__":
    try:
        main()
    except (OSError, UnicodeError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        sys.exit(1)
