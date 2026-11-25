#!/usr/bin/env bash
set -euo pipefail

phase="${1:?expected a build phase}"
evidence_dir=target/ci-evidence
stats_file="$evidence_dir/sccache-$phase.json"
mkdir -p "$evidence_dir"

printf '### sccache statistics after %s\n\n' "$phase" >> "$GITHUB_STEP_SUMMARY"
if stats=$("$SCCACHE" --show-stats); then
  printf '%s\n' "$stats"
  printf '~~~text\n%s\n~~~\n\n' "$stats" >> "$GITHUB_STEP_SUMMARY"
else
  printf '::warning::sccache statistics unavailable after %s.\n' "$phase"
  printf 'sccache statistics unavailable.\n\n' >> "$GITHUB_STEP_SUMMARY"
  exit 1
fi

if ! "$SCCACHE" --show-stats --stats-format json > "$stats_file"; then
  rm -f "$stats_file"
  printf '::warning::Could not save sccache JSON after %s.\n' "$phase"
  exit 1
fi
