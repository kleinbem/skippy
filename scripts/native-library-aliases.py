#!/usr/bin/env python3
"""Coalesce identical staged Linux libraries after relocation."""

import os
import stat
import sys
import tempfile
from pathlib import Path

from lib.ci_artifacts import digest


def deduplicate(directory):
    if os.name != "posix" or not stat.S_ISDIR(directory.lstat().st_mode):
        raise ValueError("library deduplication needs a real directory on a Unix packaging host")
    originals = {}
    for path in sorted(directory.iterdir()):
        if not (path.name.endswith(".so") or ".so." in path.name):
            continue
        metadata = path.lstat()
        if not stat.S_ISREG(metadata.st_mode):
            raise ValueError(f"staged library must be a regular file: {path}")
        key = (metadata.st_size, metadata.st_mode, digest(path))
        original = originals.setdefault(key, path)
        if original != path and not original.samefile(path):
            with tempfile.TemporaryDirectory(dir=directory) as tmp:
                link = Path(tmp) / "alias"
                os.link(original, link)
                link.replace(path)


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("usage: native-library-aliases.py LIB_DIR")
        deduplicate(Path(sys.argv[1]))
    except (OSError, ValueError) as error:
        sys.exit(str(error))
