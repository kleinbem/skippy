FROM ubuntu:26.04@sha256:d66b366dac61b575bee2aef07e9c26e8070096fbe84a3d4d3eb0da069b5d76db

LABEL org.opencontainers.image.source="https://github.com/Mesh-LLM/skippy" \
      org.opencontainers.image.description="Skippy CPU CI toolchain"

# Image builds have no interactive package-configuration terminal.
ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential ca-certificates clang-22 clang-format-22 clang-tidy-22 \
      cmake curl git libc++-22-dev libc++abi-22-dev libclang-22-dev \
      libclang-cpp22-dev libclang-rt-22-dev libcurl4-openssl-dev \
      libedit-dev libomp-22-dev libssl-dev libzstd-dev lld-22 llvm-22-dev mold ninja-build \
      openssh-client patchelf pkg-config jq python3 rustup xz-utils zlib1g-dev zstd \
    && rm -rf /var/lib/apt/lists/*

# Cargo keeps its default $HOME/.cargo layout, including when Actions mounts a
# fresh /github/home. Rustup's preinstalled toolchains stay outside that mount.
ENV RUSTUP_HOME=/opt/rust/rustup \
    PATH=/usr/lib/llvm-22/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    RUSTC_WRAPPER=/usr/local/bin/sccache \
    SCCACHE=/usr/local/bin/sccache \
    CMAKE_C_COMPILER_LAUNCHER=/usr/local/bin/sccache \
    CMAKE_CXX_COMPILER_LAUNCHER=/usr/local/bin/sccache \
    CC=/usr/lib/llvm-22/bin/clang \
    CXX=/usr/lib/llvm-22/bin/clang++ \
    LIBCLANG_PATH=/usr/lib/llvm-22/lib \
    SKIPPY_REWRITER_LLVM_PREFIX=/usr/lib/llvm-22

# Versioned release binaries are verified before installation.
RUN test "$(dpkg --print-architecture)" = amd64 \
    && curl --fail --location --retry 3 -o /tmp/just.tar.gz \
      https://github.com/casey/just/releases/download/1.58.0/just-1.58.0-x86_64-unknown-linux-musl.tar.gz \
    && echo '4a5cc2f53e6f0f8c59092a6cc38291eb729d46a7dd95d3ae582008881b84931d  /tmp/just.tar.gz' | sha256sum --check \
    && tar -xzf /tmp/just.tar.gz -C /usr/local/bin just \
    && curl --fail --location --retry 3 -o /tmp/sccache.tar.gz \
      https://github.com/mozilla/sccache/releases/download/v0.19.0/sccache-v0.19.0-x86_64-unknown-linux-musl.tar.gz \
    && echo 'aa73c7270c997894038c0d8f0d297865f57569e1687883322bc431713d4bb9dd  /tmp/sccache.tar.gz' | sha256sum --check \
    && tar -xzf /tmp/sccache.tar.gz --strip-components=1 -C /usr/local/bin \
      sccache-v0.19.0-x86_64-unknown-linux-musl/sccache \
    && rm /tmp/just.tar.gz /tmp/sccache.tar.gz

RUN groupadd --gid 1001 runner \
    && useradd --uid 1001 --gid 1001 --create-home runner \
    && mkdir -p /opt/rust/rustup /workspace \
    && chown runner:runner /opt/rust/rustup /workspace
USER runner
COPY rust-toolchain.toml /opt/skippy-ci/rust-toolchain.toml
WORKDIR /opt/skippy-ci
# Rustup reads the repository's toolchain file, including profile and components.
RUN rustc --version \
    && rustup default "$(rustup show active-toolchain | cut -d ' ' -f 1)"

# Compile and execute Rust/mold and C++23/OpenMP/lld contracts offline.
RUN --network=none image_probe_dir="$(mktemp -d)" \
    && printf 'fn main() { assert_eq!(6 * 7, 42); }\n' > "$image_probe_dir/probe.rs" \
    && rustc -C linker=clang -C link-arg=-fuse-ld=mold "$image_probe_dir/probe.rs" -o "$image_probe_dir/rust-probe" \
    && "$image_probe_dir/rust-probe" \
    && printf '#include <expected>\n#include <omp.h>\nint main() { std::expected<int,int> v{41}; auto r = v.and_then([](int n) { return std::expected<int,int>{n+1}; }); return r && *r == 42 && omp_get_max_threads() > 0 ? 0 : 1; }\n' > "$image_probe_dir/probe.cpp" \
    && clang++ -std=c++23 -Wall -Wextra -Werror -fopenmp -fuse-ld=lld -fsanitize=address,undefined \
      "$image_probe_dir/probe.cpp" -o "$image_probe_dir/cpp-probe" \
    && "$image_probe_dir/cpp-probe" \
    && clang++ -std=c++23 -stdlib=libc++ -Wall -Wextra -Werror -fopenmp -fuse-ld=lld \
      -fsanitize=address,undefined "$image_probe_dir/probe.cpp" -o "$image_probe_dir/libcxx-probe" \
    && "$image_probe_dir/libcxx-probe" \
    && ssh-keygen -q -t ed25519 -N '' -C skippy-ci-smoke -f "$image_probe_dir/key" \
    && ssh-keygen -y -f "$image_probe_dir/key" > "$image_probe_dir/loaded.pub" \
    && cmp "$image_probe_dir/key.pub" "$image_probe_dir/loaded.pub" \
    && rm -rf "$image_probe_dir"

USER root
ARG SKIPPY_CI_IMAGE_ID=local
ARG SOURCE_REVISION=local
LABEL org.opencontainers.image.revision="$SOURCE_REVISION"
RUN test -n "$SKIPPY_CI_IMAGE_ID" \
    && printf '%s\n' "$SKIPPY_CI_IMAGE_ID" > /etc/skippy-ci-image-id
USER runner
WORKDIR /workspace
CMD ["bash"]
