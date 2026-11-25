#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# Load only the test runner. The enclosing checker builds and rewrites llama.cpp.
eval "$(sed -n '/^run_native_tests() {$/,/^}$/p' "$root/scripts/check-skippy-generated-family-patch.sh")"
NATIVE_TEST_ARGS=()

ctest() {
  case "${1:-}" in
    -E) return "$parallel_result" ;;
    -R) return "$serial_result" ;;
    *) return "$parallel_result" ;;
  esac
}

check() {
  local expected="$1" actual=0
  shift
  # The production conditional pipeline suppresses errexit in run_native_tests.
  run_native_tests "$@" | cat || actual=$?
  [[ "$actual" == "$expected" ]]
}

export CTEST_PARALLEL_LEVEL=2
parallel_result=0 serial_result=0 check 0
parallel_result=8 serial_result=0 check 8
parallel_result=0 serial_result=8 check 8
parallel_result=8 serial_result=8 check 8
export CTEST_PARALLEL_LEVEL=1
parallel_result=8 serial_result=0 check 8
