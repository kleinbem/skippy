#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# The typed qualifier owns the full manifest, input admission, builds and receipts.
exec just --justfile "$ROOT/Justfile" automation-run upgrade certify \
  --backend "${LLAMA_STAGE_BACKEND:-cpu}" --suite supported \
  --pass "${SKIPPY_UPGRADE_PASS:-candidate}" "$@"
