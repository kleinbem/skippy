#!/usr/bin/env bash
# Install reporting test dependencies explicitly; the test recipes stay offline.
set -euo pipefail
"${SKIPPY_PYTHON:-python3}" -m venv "$1"
"$1/bin/python" -m pip install --requirement ci/inference-publish-requirements.txt
if [[ -n "${GITHUB_ENV:-}" ]]; then
  printf 'SKIPPY_PYTHON=%s/bin/python\n' "$1" >> "$GITHUB_ENV"
fi
