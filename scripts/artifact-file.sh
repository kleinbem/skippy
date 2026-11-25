#!/usr/bin/env bash
set -euo pipefail

command="${1:?usage: artifact-file.sh sha256|canonical-path PATH}"
path="${2:?usage: artifact-file.sh sha256|canonical-path PATH}"
if [[ $# -ne 2 ]]; then
  echo 'usage: artifact-file.sh sha256|canonical-path PATH' >&2
  exit 2
fi

case "$command" in
  sha256)
    if command -v sha256sum >/dev/null 2>&1; then
      if [[ "$path" == - ]]; then sha256sum; else sha256sum < "$path"; fi | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
      if [[ "$path" == - ]]; then shasum -a 256; else shasum -a 256 < "$path"; fi | cut -d ' ' -f 1
    else
      "${SKIPPY_PYTHON:-python3}" -c '
import hashlib, sys
digest = hashlib.sha256()
if sys.argv[1] == "-":
    stream = sys.stdin.buffer
else:
    stream = open(sys.argv[1], "rb")
with stream:
    for block in iter(lambda: stream.read(65536), b""):
        digest.update(block)
print(digest.hexdigest())
' "$path"
    fi
    ;;
  canonical-path)
    if ! realpath -e -- "$path" 2>/dev/null; then
      "${SKIPPY_PYTHON:-python3}" -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve(strict=True))' "$path"
    fi
    ;;
  *) echo 'usage: artifact-file.sh sha256|canonical-path PATH' >&2; exit 2 ;;
esac
