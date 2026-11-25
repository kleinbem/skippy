# Metal placement and batching migration

## Source and baseline

The product changes originate from `Mesh-LLM/mesh-llm` branch
`skippy/metal-placement-batching`, commit
`3afad3e60e19b22200009c85ec77e6dd0be7c5dc` (PR #3031).
The destination starts at standalone Skippy main
`32fa8c0f01ea39626bc45840d7eea50a3180222e` (fetched November 20, 2026).

The extraction baseline is Mesh commit
`937f236c3de65f496324af79739d4aed7b438733`, subtree `skippy/`, and
standalone commit `dfefa89fbf5ca718f62cb9555356dba11a9c75c9`.
Both have tree `a7e009afb3f71594d5ae0a72bf391e019765dedd`.
The migration uses that mapped ancestor for a three-way content merge, preserving
standalone changes. It does not import monorepo refs or commit ancestry.

## Ownership

The port includes resident attention and recurrent-checkpoint ownership,
cooperative prefill and capacity admission, batched placement, Metal row-list
attention, configurable batch policies and sampling, tracing, the concurrent
HTTP reuse checker, and native regression tests. Existing standalone physical
prefill admission tests and encoder-decoder lane limits remain covered.

Mesh host/adapter code, CI topology, root Cargo/Just files, AgentX campaign
orchestration, JY dispatch, and MLflow publication are excluded. The standalone
Justfile and existing `scripts/` automation remain authoritative. The portable
K/V evaluator has a standalone build command; historical campaign plans are
provenance, not runnable standalone xtask commands.

The legacy `scripts/skippy-ci-smoke.sh` and `docs/KV_CACHE_DISK.md` remain
deleted. Standalone validation uses `just native-full-replay BACKEND`,
`just cache-reuse-smoke BASE_URL MODEL PROMPT OUTPUT WAVE`, and
`just kv-matrix-build NATIVE_LIB_DIR`. User-facing cache documentation stays
under `docs-site/content/kv-caching/`.

## Native recipe

The llama.cpp pin stays at
`8954f194837d2ec4d5f67f20c81b84abe63883ba`.
Standalone core patches 0017–0019 are retained. Incoming core patches 0017–0039
become 0020–0042, in their original order. This includes the follow-up fixes for
released-page compaction, Metal row-list interiors, sampling helper limits,
and composite-cache memory controls. Model-support adaptations accompany them;
generated family patches remain unchanged.

Standalone patch 0043 updates the hybrid sampling-history regression to restore
both attention and recurrent checkpoint components, matching the new component
API. It retains byte-for-byte state and exact continuation-logit comparisons.
Patch 0044 aligns cache-capability fixtures with the separate resident flags;
patch 0045 fixes empty Metal command buffers being reported as mixed work.
The standalone replay build also includes the registered activation-validation
executable.
The port also preserves standalone prefill admission: an exactly fitting prompt
does not reserve an additional decode batch. An explicit loaded-runtime test
covers a 511-token prompt in a 512-token context with a 64-token batch.

Native ABI 0.1.86, Rust version constants, generated bindings, dynamic exports,
and generated native API documentation move together. Runtime packages must be
rebuilt from this complete recipe; old ABI 0.1.80 binaries cannot be mixed in.
Cache identities deliberately separate the new recurrent-component layout.

The balanced batch limit rises from 512 to 2048 tokens while its microbatch
stays at 512. The throughput profile uses a 2048-token batch and 1024-token
microbatch; saver remains 256/256. Explicit settings still override profiles.
Native pools default to lazy clearing, released free pages, and per-request
row attention where supported. `kv_pool_tokens` can bound total shared capacity;
each request still has its own context limit. Sampling can use pooled helpers,
with `sample_threads = 1` selecting inline execution.

Native prefix caching now retains attention cells and recurrent checkpoints
separately. `prefix_cache_ram = "off"` leaves the bounded native checkpoint
default in place, independently of disk caching; disable prefix caching to
retain neither. Serialized-runtime RAM caching retains its opt-in behavior.
These changes affect memory use and batch latency, so compare an unchanged
workload with explicit pool, retention, and batch settings before adopting
historical performance results.

Split planning validates native stage graphs, tensor closures and activation
frontiers. Qualification results bind to the exact source recipe and tested
configuration.

## Qualification

Offline tests, generated-fixture native contracts, real-model correctness,
durable restart, and performance measurements are separate evidence. Native
regressions and the Rust tests do not establish four-model CUDA qualification
or the historical AgentX throughput claims.

Required backend comparisons include `kv_release_pages=false`,
`kv_lazy_clear=false`, `kv_row_attention=false`, and sampling with 0/1/N threads.
Keep model bytes, total and per-request context, KV types, concurrency, and
generated work identical. Exercise cold/repeated/growing requests, concurrent
distinct follow-ups, prefix eviction and compaction, recurrent restore, and
durable restart. Missing devices or model inputs do not constitute a pass.

## Local validation

Validated on macOS ARM64 with Metal, November 20, 2026. The prepared native
commit is `7e4392fe2acf0f6a351bee77c39038f3caeb7613`, with patch digest
`49d46ccad5c85a653f94e472697825acc601acb1b3ce9465e4ec461475fe4b79`.

- `LLAMA_STAGE_BACKEND=metal SKIPPY_LLAMA_AUTO_BUILD=0 just lint`: formatting
  and warning-denying all-target Clippy passed in both Rust workspaces.
- `LLAMA_STAGE_BACKEND=metal SKIPPY_LLAMA_AUTO_BUILD=0 just test`: both Rust
  workspaces, integration and documentation tests passed; C++ facade 249/249.
- Workspace Clippy with dynamic-runtime and Tracy enabled passed. The serving
  suite with Tracy passed 1,357 tests, with 32 explicitly ignored fixtures.
- `just skippy-ffi-bindgen-check`: pinned-header freshness and self-test passed.
- `just docs-build` and `just automation-contracts` passed.
- `CTEST_PARALLEL_LEVEL=8 LLAMA_STAGE_BACKEND=metal
  LLAMA_STAGE_LINK_MODE=static just native-full-replay metal`: 335 tests passed;
  the production-model GPU checkpoint test was skipped for its absent fixture.
- `LLAMA_STAGE_LINK_MODE=dynamic LLAMA_STAGE_BACKEND=metal just llama-build`
  and `just kv-matrix-build /absolute/matching/runtime/lib` passed.
- Explicit loaded-runtime tests for a near-full context and a cold prefix
  surviving runtime restart passed without fixture skips. They used the
  generated 24-layer Qwen3.5 contract fixture, not a production model.

The real-model K/V dtype matrix, concurrent HTTP reuse campaign, matched
throughput/TTFT comparisons, and CUDA qualification have not been run by this
port. Cache performance admission remains required before merge under
`crates/skippy-cache/AGENTS.md`; this transfer does not merge or certify them.
The optional `skippy_resident_checkpoint_gpu_long` test lacks
`SKIPPY_NATIVE_TEST_REAL_MODEL` and is not production-model GPU evidence.
