ci-image-build:
    #!/usr/bin/env bash
    set -euo pipefail
    cache_args=()
    if [[ -n "${SKIPPY_CI_CACHE_REF:-}" ]]; then
      cache_args+=(--cache-from "type=registry,ref=$SKIPPY_CI_CACHE_REF")
      cache_args+=(--build-arg BUILDKIT_INLINE_CACHE=1)
    fi
    docker buildx build ${cache_args[@]+"${cache_args[@]}"} --load --platform linux/amd64 --file ci/Dockerfile --tag skippy-ci-cpu:build --build-arg SKIPPY_CI_IMAGE_ID --build-arg SOURCE_REVISION .

ci-image-smoke:
    @docker run --rm --platform linux/amd64 --network none --read-only --tmpfs /github/home:uid=1001,gid=1001 --env HOME=/github/home skippy-ci-cpu:build bash -euc 'test -z "${CARGO_HOME:-}"; python3 --version; jq --version; rustc --version; cargo clippy --version; rustfmt --version; just --version; sccache --version; gcc --version; g++ --version; make --version; mold --version; clang++ --version; cmake --version; ninja --version; ssh -V; command -v ssh-keygen; test -r /usr/lib/llvm-22/lib/cmake/llvm/LLVMConfig.cmake; test -r /usr/lib/llvm-22/lib/cmake/clang/ClangConfig.cmake; test -s /etc/skippy-ci-image-id; test "$(id -u)" = 1001'

ci-gpu-image-build backend toolkit_base toolkit_version arch="amd64":
    #!/usr/bin/env bash
    set -euo pipefail
    cache_args=()
    if [[ -n "${SKIPPY_CI_CACHE_REF:-}" ]]; then
      cache_args+=(--cache-from "type=registry,ref=$SKIPPY_CI_CACHE_REF")
      cache_args+=(--build-arg BUILDKIT_INLINE_CACHE=1)
    fi
    docker buildx build ${cache_args[@]+"${cache_args[@]}"} --load --platform "linux/{{ arch }}" --file ci/Dockerfile.gpu --tag skippy-ci-{{ backend }}:build --build-arg TOOLKIT_BASE="{{ toolkit_base }}" --build-arg BACKEND="{{ backend }}" --build-arg TOOLKIT_VERSION="{{ toolkit_version }}" --build-arg SKIPPY_CI_IMAGE_ID --build-arg SOURCE_REVISION .

ci-gpu-image-smoke backend arch="amd64":
    #!/usr/bin/env bash
    set -euo pipefail
    docker run --rm --platform "linux/{{ arch }}" --network none --read-only "skippy-ci-{{ backend }}:build" bash -euc '
      test "$(dpkg --print-architecture)" = "{{ arch }}"
      python3 --version
      jq --version
      rustc --version
      cargo clippy --version
      rustfmt --version
      just --version
      sccache --version
      /usr/bin/clang --version
      /usr/bin/clang++ --version
      cmake --version
      ninja --version
      llvm-config --version
      test -r /usr/lib/llvm-18/lib/cmake/llvm/LLVMConfig.cmake
      test -r /usr/lib/llvm-18/lib/cmake/clang/ClangConfig.cmake
      test -s /etc/skippy-ci-image-id
      test "$(id -u)" = 1001
      test "$SKIPPY_CI_BACKEND" = "{{ backend }}"
      case "$SKIPPY_CI_BACKEND" in
        cuda13)
          test -s /usr/local/cuda/EULA.txt
          /usr/local/cuda-13.1/bin/nvcc --version | grep -F "release 13.1,"
          test -e /usr/local/cuda-13.1/lib64/libcudart.so
          ;;
        cuda12)
          test -s /usr/local/cuda/EULA.txt
          /usr/local/cuda-12.9/bin/nvcc --version | grep -F "release 12.9,"
          test -e /usr/local/cuda-12.9/lib64/libcudart.so
          ;;
        rocm) hipcc --version ;;
        *) exit 2 ;;
      esac
    '
