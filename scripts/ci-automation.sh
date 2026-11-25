#!/usr/bin/env bash
# Offline workflow, gate and report regressions, without Rust or model setup.
set -euo pipefail
node --test .github/scripts/*.test.cjs
bash scripts/tests/inference-report.sh
