#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"${SKIPPY_PYTHON:-python3}" "$SCRIPT_DIR/native-inspect.py" verify-runtime-package "$@"
