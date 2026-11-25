#!/usr/bin/env bash
set -euo pipefail

event="${SKIPPY_CI_EVENT:-}"
base="${SKIPPY_CI_BASE_SHA:-}"
head="${SKIPPY_CI_HEAD_SHA:-}"
required=true
reason="${event:-unknown}-runs-graph-contracts"

if [[ "$event" == "pull_request" ]] \
    && [[ "$base" =~ ^[0-9a-f]{40}$ ]] \
    && [[ "$head" =~ ^[0-9a-f]{40}$ ]] \
    && git cat-file -e "$base^{commit}" \
    && git cat-file -e "$head^{commit}"; then
  changed_list="$(mktemp "${TMPDIR:-/tmp}/skippy-graph-impact.XXXXXX")"
  trap 'rm -f "$changed_list"' EXIT
  git diff --name-only --diff-filter=ACDMRTUXB -z "$base" "$head" > "$changed_list"
  required=false
  reason="classified-non-graph-pull-request"
  saw_change=false
  while IFS= read -r -d '' changed_file; do
    saw_change=true
    case "$changed_file" in
      llama_cpp/* | skippy_api/* | crates/skippy-ffi/* | ci/llama-canary/* | \
      scripts/build-llama.sh | scripts/prepare-llama.sh | \
      scripts/check-skippy-generated-family-patch.sh | scripts/check-skippy-public-headers.sh | \
      scripts/family-qualify.sh | scripts/skippy-workload-certify.sh | \
      scripts/generate-skippy-*.py | scripts/lib/rewriter_report.py | scripts/rewriter-report.py | \
      scripts/verify-skippy-family-generator-coverage.py | scripts/select-skippy-family-shards.py | \
      .github/workflows/llama-upgrade.yml | Cargo.toml | Cargo.lock | \
      rust-toolchain.toml | .cargo/*)
        required=true
        reason="graph-sensitive-path"
        break
        ;;
      AGENTS.md | CONTRIBUTING.md | LICENSE | NOTICE | README.md | RELEASE.md | \
      docs/* | crates/* | skippy-api-rs/* | tools/* | ci/* | .github/* | \
      clippy.toml | rustfmt.toml | .gitignore | .gitattributes)
        ;;
      *)
        required=true
        reason="unclassified-path"
        break
        ;;
    esac
  done < "$changed_list"
  if [[ "$saw_change" == "false" ]]; then
    required=true
    reason="empty-or-unavailable-pull-request-diff"
  fi
fi

printf 'graph_required=%s\ngraph_reason=%s\n' "$required" "$reason"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  printf 'graph_required=%s\ngraph_reason=%s\n' "$required" "$reason" >> "$GITHUB_OUTPUT"
fi
