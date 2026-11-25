# Native runtimes

Skippy releases a backend-neutral CLI separately from platform-specific native
runtimes. A product bundle combines one exact CLI producer with one exact
runtime producer; composition never recompiles either input.

## Identity and admission

Every runtime manifest binds:

- Skippy runtime release and ABI versions;
- source commit, llama.cpp pin, and patch digest;
- operating system and architecture;
- backend and toolkit identity;
- packaged libraries, tools, and SHA-256 hashes;
- Linux glibc requirements where applicable.

The resolver rejects ABI, platform, backend, toolkit, checksum, dependency, or
source mismatches before loading a library. Legacy schema-one manifests can be
imported only through the explicit compatibility API; normal standalone
selection requires the current schema.

## Discovery

Runtime candidates are searched in this order:

1. directories supplied with `--runtime-bundle`;
2. `SKIPPY_NATIVE_RUNTIME_BUNDLE_DIR`;
3. `native-runtimes/` beside the executable;
4. the platform cache selected by `--runtime-cache` or
   `SKIPPY_NATIVE_RUNTIME_CACHE_DIR`.

The working directory is never searched implicitly. Private GitHub release
assets are downloaded with an authenticated client and then used locally;
serving does not need repository credentials.

## Build and release

`just build` creates a development CLI with an adjacent verified runtime.
macOS defaults to Metal and other platforms default to CPU. Use
`just build cpu` or `just build cuda` for an explicit backend.

Release CI builds four host binaries and eleven runtime variants from
[`ci/targets.json`](../../ci/targets.json). It verifies import/dependency
closure, manifest identity, checksums, runtime discovery, no-driver CLI startup,
and packaged Qwen3 inference. Metal and CUDA rows require real devices and
positive GPU offload evidence.

Tag-triggered release composition publishes only the complete verified matrix.
See [`ci/README.md`](../../ci/README.md) for runner and evidence requirements.
