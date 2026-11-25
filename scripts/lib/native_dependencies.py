"""Collect, verify, and order native package dependencies."""

import os
import shutil
from pathlib import Path

from .ci_artifacts import digest
from .native_binary import inspect
from .native_policy import admit_cuda, host_owned


def files(root, recursive):
    if not root.exists():
        return []
    result = []
    for path in root.iterdir():
        if path.is_file():
            path.name.encode("utf-8")
            result.append(path)
        elif recursive and not path.is_symlink() and path.is_dir():
            result.extend(files(path, True))
    return sorted(result)


class Dependencies:
    def __init__(self, args):
        self.kind = "elf" if args.action == "linux-runtime-deps" else "pe"
        self.lib = args.lib_dir
        self.scan = args.scan_dir
        self.search = args.search_dir
        self.arch = args.arch
        self.cuda_major = args.cuda_major
        self.primary = args.primary

    def key(self, name):
        return name.lower() if self.kind == "pe" else name

    def inspect_files(self):
        images = {}
        prefix = b"\x7fELF" if self.kind == "elf" else b"MZ"
        for root in [self.lib, *self.scan]:
            for path in files(root, True):
                with path.open("rb") as stream:
                    header = stream.read(4)
                if len(header) != 4 or not header.startswith(prefix):
                    continue
                image = inspect(path)
                if self.arch and self.arch != image.arch:
                    raise ValueError(f"wrong architecture: {path}")
                images[path] = image
        if not images:
            raise ValueError("runtime contains no native binaries")
        return images

    def index(self, images):
        index = {}
        for path, image in images.items():
            if not path.is_relative_to(self.lib):
                continue
            for alias in [path.name, image.soname] if image.soname else [path.name]:
                key = self.key(alias)
                if key in index and digest(index[key]) != digest(path):
                    raise ValueError(f"conflicting native providers for {alias}")
                index[key] = path
        return index

    def gaps(self, images, index):
        return sorted({
            dependency for image in images.values() for dependency in image.imports
            if not host_owned(dependency, self.kind) and self.key(dependency) not in index
        })

    def verify(self):
        images = self.inspect_files()
        index = self.index(images)
        missing = self.gaps(images, index)
        if missing:
            raise ValueError(f"unpackaged native dependencies: {missing}")
        return images, index

    def collect(self):
        search = self.search
        if self.kind == "pe":
            search = [*search, *(Path(path) for path in os.get_exec_path() if path)]
        candidates = {}
        for directory in search:
            for path in files(directory, self.kind == "elf"):
                if any(part.lower() == "stubs" for part in path.parts):
                    continue
                candidates.setdefault(self.key(path.name), []).append(path)
        for _ in range(256):
            images = self.inspect_files()
            missing = self.gaps(images, self.index(images))
            if not missing:
                return
            for dependency in missing:
                self.install(dependency, candidates.get(self.key(dependency), []))
        raise ValueError("native dependency closure exceeded 256 passes")

    def install(self, dependency, candidates):
        if (not dependency or dependency in {".", ".."}
                or any(ch in dependency for ch in "/\\:\0")):
            raise ValueError(f"invalid dependency filename: {dependency}")
        if self.kind == "elf":
            admit_cuda(dependency, self.cuda_major)
        selected = None
        for path in candidates:
            image = inspect(path)
            if self.arch and self.arch != image.arch:
                continue
            if selected is None:
                selected = path
            elif digest(selected) != digest(path):
                raise ValueError(f"conflicting dependency providers: {dependency}")
        if selected is None:
            raise ValueError(f"unresolved native dependency: {dependency}")
        destination = self.lib / dependency
        if destination.is_symlink():
            raise ValueError("dependency destination is a symlink")
        shutil.copy(selected, destination)

    def order(self):
        images, index = self.verify()
        primary = index.get(self.key(self.primary or ""))
        if primary is None:
            raise ValueError("missing primary native library")
        visiting, visited, ordered = set(), set(), []

        def visit(path):
            if path in visited:
                return
            if path in visiting:
                raise ValueError("cyclic native dependency graph")
            visiting.add(path)
            for dependency in images[path].imports:
                if self.key(dependency) in index:
                    visit(index[self.key(dependency)])
            visiting.remove(path)
            visited.add(path)
            ordered.append(path.name)

        for path in images:
            if path.parent == self.lib and path.name != self.primary:
                visit(path)
        visit(primary)
        print("\n".join(ordered))
