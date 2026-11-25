#!/usr/bin/env python3
"""Check native JY preparation tools without invoking a repair session."""

import os
import shutil
import subprocess
import sys
from pathlib import Path

from lib.ci_process import command
from lib.upgrade_repair import repair_argv


def check():
    executable = repair_argv()[0]
    repair = shutil.which(executable)
    if not repair:
        raise ValueError(f"repair executable is not installed on the JY runner: {executable}")
    for name in ("git", "just", "cargo", "cmake", "ninja"):
        if not shutil.which(name):
            raise ValueError(f"required JY native tool is not installed: {name}")
    llvm = Path(os.environ["SKIPPY_REWRITER_LLVM_PREFIX"])
    if not (llvm / "bin/clang").is_file() or not Path(os.environ["LIBCLANG_PATH"]).is_dir():
        raise ValueError("JY runner requires the supplied LLVM/Clang development installation")
    print(f"Using repair executable: {repair}")
    if Path(repair).name == "goose":
        command([repair, "--version"], timeout=30)


if __name__ == "__main__":
    try:
        check()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        sys.exit(str(error))
