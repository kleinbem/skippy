#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python="${SKIPPY_PYTHON:-python3}"

usage() {
    echo 'usage: just automation-run {automation|upgrade|native|release|artifact|inference|ci} COMMAND [OPTIONS]'
}

case "${1:-}" in
    ''|-h|--help) usage; exit 0 ;;
esac

case "${1:-}:${2:-}:${3:-}:${4:-}" in
    automation:native-generator:contracts:api-doc)
        shift 4
        exec "$python" "$script_dir/generate-skippy-api-doc.py" "$@" ;;
    automation:native-generator:contracts:coverage)
        shift 4
        exec "$python" "$script_dir/verify-skippy-family-generator-coverage.py" "$@" ;;
    automation:native-generator:contracts:graph)
        shift 4
        exec "$python" "$script_dir/graph-contract-coverage.py" "$@" ;;
    automation:native-generator:contracts:select-shards)
        shift 4
        exec "$python" "$script_dir/select-skippy-family-shards.py" "$@" ;;
    automation:native-generator:contracts:--help)
        echo 'usage: automation native-generator contracts {api-doc|coverage|graph|select-shards}'
        exit 0 ;;
    automation:native-generator:contracts:*)
        echo 'usage: automation native-generator contracts {api-doc|coverage|graph|select-shards}' >&2
        exit 2 ;;
esac

case "${1:-}:${2:-}" in
    automation:native-generator)
        shift 2
        exec "$python" "$script_dir/generate-skippy-family-patch.py" "$@" ;;
    automation:rewriter-report)
        shift 2
        exec "$python" "$script_dir/rewriter-report.py" "$@" ;;
    automation:family-model-identity|automation:local-ports)
        shift
        exec "$python" "$script_dir/family-input.py" "$@" ;;
    automation:workload-smoke|automation:workload-monolithic-oracle)
        shift
        exec "$python" "$script_dir/workload-check.py" "$@" ;;
    automation:workload-media-oracle)
        shift 2
        exec "$python" "$script_dir/skippy-ocr-asr-oracle.py" "$@" ;;
    automation:workload-tts-oracle)
        shift 2
        exec "$python" "$script_dir/skippy-tts-oracle.py" "$@" ;;
    upgrade:runner-check)
        shift 2
        exec "$python" "$script_dir/upgrade-runner-check.py" "$@" ;;
    upgrade:publish|upgrade:diagnostics)
        action="$2"
        shift 2
        exec "$python" "$script_dir/upgrade-$action.py" "$@" ;;
    upgrade:resolve|upgrade:prepare|upgrade:restore|upgrade:prepared-source)
        shift
        exec "$python" "$script_dir/upgrade-source.py" "$@" ;;
    native:runtime-manifest-write)
        shift 2
        exec "$python" "$script_dir/native-manifest.py" "$@" ;;
    native:deduplicate-linux-libraries)
        shift 2
        exec "$python" "$script_dir/native-library-aliases.py" "$@" ;;
    native:package-source-version|native:cargo-target-directory)
        shift
        exec "$python" "$script_dir/native-source.py" "$@" ;;
    native:*)
        shift
        exec "$python" "$script_dir/native-inspect.py" "$@" ;;
    release:*)
        shift
        exec "$python" "$script_dir/ci-release.py" "$@" ;;
    artifact:*)
        shift
        if [[ "${1:-}" == file-projection ]]; then shift; fi
        exec bash "$script_dir/artifact-file.sh" "$@" ;;
    inference:build)
        shift 2
        exec "$python" "$script_dir/inference-build.py" "$@" ;;
    inference:execute)
        shift 2
        exec "$python" "$script_dir/inference-execute.py" "$@" ;;
    inference:admit|inference:plan|inference:matrix)
        shift
        exec "$python" "$script_dir/inference-plan.py" "$@" ;;
    inference:dispatch|inference:cancel|inference:control-preflight)
        shift
        exec "$python" "$script_dir/inference-dispatch.py" "$@" ;;
    ci:plan|ci:model-fetch)
        action="$2"
        shift 2
        exec bash "$script_dir/ci-$action.sh" "$@" ;;
    ci:host|ci:runtime|ci:package|ci:smoke|ci:artifact-smoke)
        shift
        exec "$python" "$script_dir/ci-product.py" "$@" ;;
    *)
        usage >&2
        exit 2 ;;
esac
