#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
entry="$(jq -cer '
  .models[] | select(.family == "qwen3-dense")
  | .artifact as $artifact | $artifact.files[0] as $file
  | {size: $artifact.file_integrity[$file].size_bytes,
     sha256: $artifact.file_integrity[$file].blob_id}
' "$root/ci/llama-canary/model-fixtures.json")"
expected_size="$(jq -er '.size' <<<"$entry")"
expected_sha="$(jq -er '.sha256' <<<"$entry")"

verify_model() {
  local file="$1" actual_size actual_sha
  actual_size="$(wc -c < "$file" | tr -d '[:space:]')"
  if command -v sha256sum >/dev/null 2>&1; then
    actual_sha="$(sha256sum "$file")"
  else
    actual_sha="$(shasum -a 256 "$file")"
  fi
  actual_sha="${actual_sha%% *}"
  if [[ "$actual_size" != "$expected_size" || "$actual_sha" != "$expected_sha" ]]; then
    echo 'Qwen3 fixture size or SHA-256 mismatch' >&2
    return 1
  fi
}

if [[ ${SKIPPY_CI_MODEL+x} ]]; then
  destination="$SKIPPY_CI_MODEL"
else
  model_dir="$root/target/ci-models"
  mkdir -p "$model_dir"
  destination="$model_dir/Qwen3-0.6B-Q8_0.gguf"
  if [[ ! -e "$destination" ]]; then
    temporary="$(mktemp "$model_dir/.qwen3.XXXXXXXX")"
    trap 'rm -f "$temporary"' EXIT
    curl --fail --location --proto '=https' --max-time 900 --output "$temporary" \
      'https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/23749fefcc72300e3a2ad315e1317431b06b590a/Qwen3-0.6B-Q8_0.gguf'
    verify_model "$temporary"
    mv "$temporary" "$destination"
    trap - EXIT
  fi
fi

verify_model "$destination"
if [[ -n "${GITHUB_ENV:-}" ]]; then
  absolute="$(realpath "$destination")"
  case "$absolute" in
    *$'\n'*|*$'\r'*) echo 'invalid model path for GITHUB_ENV' >&2; exit 1 ;;
  esac
  printf 'SKIPPY_CI_MODEL=%s\n' "$absolute" >> "$GITHUB_ENV"
fi
