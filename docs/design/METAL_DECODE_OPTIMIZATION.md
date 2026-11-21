# Metal decode optimization work

The November 17, 2026 M3 Ultra measurements cover the original port across three process campaigns. The vanilla comparison uses a fixed token budget and records cross-engine text differences. See [the benchmark evidence](METAL_DECODE_BENCHMARKS.md).

## Baseline migration

The standalone extraction starts at `fbc3349cd406786e43f6579590c9a12b0cd49c05`
and pins llama.cpp `8954f194837d2ec4d5f67f20c81b84abe63883ba`.
It predates the shared draft/verification scheduler and native decode work
qualified in the Mesh-hosted Skippy tree. This branch ports those changes while
preserving the extraction's newer upstream changes and standalone ownership.

The earlier placement/batching work was already merged into Mesh's `gi` branch
in PR #33. It was absent from the measured standalone extraction baseline,
whose core native patch queue ends at 0016 rather than PR #33's 0035. Therefore
the standalone headline includes this catch-up as well as subsequent changes;
it does not measure our incremental contribution over Jared's work.

The port includes shared speculative forwards, concurrent sampling, resident
prefix handling, KV allocation and row attention improvements, gated activation
fusion, routed vector reuse, decode convolution changes, and tracing. The affine
conversion preserves integer codes and rounds affine coefficients to f16; that
conversion utility is not used in these unchanged-GGUF serving measurements.
Model tuning is outside scope.

Compatibility resolutions retain upstream per-layer KV strides, the standalone
context-capacity implementation, prompt shape statistics, and current sampling
state. Native API bindings are regenerated from the prepared source at ABI
0.1.85 rather than copied from the older upstream pin.

Skippy PR #71 subsequently landed the placement, batching, and resident-cache
baseline on standalone main. PR #9 now adds the remaining shared verification
and draft execution paths and nine native decode patches to that baseline, with
native ABI 0.1.88. The measurements below refer to the historical snapshots.

## Historical validation and remaining gates

Local evidence is under `target/migration-audit/`; those artifacts are ignored
and are not a published benchmark report.

- All 141 ordered native patches apply directly, with every intermediate tree
  matching the qualified native history. Two imported patches were regenerated
  against their actual parents after a fresh M3 checkout exposed hidden
  three-way Git-object dependencies. The resulting native source is unchanged.
- Rust all-target warning-denying Clippy passed again after the final
  documentation and test-assertion edits.
- Native routed policy and numerical checks passed, including the R4/R8 paths.
- The focused native CTest run passed all 51 fixture-backed tests, with no skips.
- The full-model control/candidate probe produced identical exported state in
  four comparisons covering rollback and shared recurrent-lane repair.
- The wider native family graph-contract run passed all 198 tests with no
  skips. The serial Rust
  workspace run excluding `skippy-api` passed, including integration and
  documentation tests. The API run passed 131 tests with its two unresolved
  split-certification tests explicitly filtered out; that is not a full pass.
- The split roster remains bound to its prior native recipe. Do not relabel
  those certificates as newly qualified to make tests pass.
- Regenerated API documentation and the binding freshness/mutation self-check
  passed. The standalone release CLI builds and reads the packaged native
  runtime successfully.
- The C++ facade CTest run completed: 248 passed and one skipped because this
  standard library lacks `std::start_lifetime_as_array`. The skip is not native
  hardware qualification.
- The separate Rust facade workspace passed formatting, all-target
  warning-denying Clippy, unit/integration tests, and documentation tests. Its
  optional real embedding/reranking fixtures are not decode qualification.
- Two unchanged process-supervision projection tests failed in the parallel
  workspace run and passed both in the focused serial rerun and the serial
  workspace run. Do not report an unconditional parallel workspace pass.
- A pristine standalone baseline checkout and its runtime/release CLI have been
  built separately. The confirmed Skippy serving results and qualified vanilla
  observations are recorded in the benchmark report.
- An untimed serving smoke loaded the original 35B GGUF, returned the expected
  one-word response, and completed two concurrent 64-token requests with prompt
  caching disabled. The owned server then shut down cleanly. Other validation
  was running, so request elapsed times are not benchmark evidence.

Historical Mesh-hosted engine-only results were +8.5% at concurrency one and
+45.3% aggregate throughput at concurrency two against Jared's PR baseline.
They are workload-specific historical results, not measurements of this
standalone port or a claim of parity with another engine.

The separate M1 parallel-encoding experiment failed its predeclared performance
gate: plain decode improved, but DFlash concurrency-two pooled throughput was
-0.097%, with mixed cycles. That prototype is not retained in this port.

## Subsequent experiments

After qualifying this baseline, measure independently implemented cached Metal
encoding plans, fewer host round trips across draft/verify/accept/commit, packed
4-bit TensorOps using unchanged model values, and shared-plus-routed expert
fusion. Static analysis and public research identify candidate mechanisms;
neither establishes their isolated speedup. Keep each experiment independently
switchable and compare matched model bytes, hardware, prompt/context, output
length, concurrency, and acceptance behavior.

### Cached encoding plan constraints

The current Metal encoder selects pipelines, binds buffers and inline arguments,
sets threadgroup memory, inserts barriers, and dispatches each graph on the CPU.
A replay experiment should record that command stream and replay ordinary Metal
encoder calls; changing command-buffer ordering is a separate experiment.

A tensor-shape-only key is insufficient. The key must cover operation parameters,
strides, source topology and aliasing, output flags, buffer usage and actual Metal
buffer identities/offsets, fusion configuration, and command-buffer boundaries.
Recorded buffers and pipeline objects need owned lifetimes with bounded eviction.
Debug capture and fusion statistics should use normal encoding.

Row-list attention currently examines host-visible row-list values to choose the
gather-free pipeline. Matching allocation and tensor geometry does not guarantee
that choice remains valid. Either recompute and key the classification on replay,
or invalidate the plan when the relevant values change. Never cache a dense-row
decision across a general-row layout merely because the list has the same shape.

Qualification must exercise buffer replacement, changed strides and operation
parameters, aliasing, row-layout transitions, graph reuse, and speculative
accept/reject/rollback. Keep plan creation and cache hits observable separately
from warmed pipeline compilation and GPU execution time.

### Isolated dispatch-plan prototype

The current default-off prototype remains outside this PR. Its native tree is
`03ebbc56f70519b50ec66db1234a96f4a38791c9`, packaged from standalone snapshot
`8b0e6bc6a60ae587e9beb75123b84c4f08bd9d2a`. It records ordinary Metal encoder
operations and compares live graph/tensor metadata against an in-place snapshot
before replay. It still resolves current Metal buffer identities, offsets and
usage and recomputes row-list density on every hit. A first pass validates node
and source identities before dereferencing saved source metadata, including
when old leaf metadata has been unmapped. Storage is bounded, resources remain
owned through command-buffer completion, and capture/debug paths encode normally.

The M3 build passed 49,152 exact ordinary-op values under invalidation cases,
3,072 exact row-list values across dense/general transitions, and 1,792 exact
values with retired source metadata, changed buffer usage and ignored debug
names. The tests require actual hits and misses. These checks establish
correctness coverage, not a serving speedup.

On the local M5 model-state probe, in-place snapshots reduce thread CPU time
spent validating the key by 84.43% versus the deduplicated serialized-key
prototype, across ABBA runs with 2,718 calls and four exact state exports per
process. This is time in one CPU component, not end-to-end TPS. The earlier
serialized-key deduplication reduced final signature storage by 45.7%; that is
also not a serving-throughput claim.

The separately built M3 runtime uses the same compiler as the retained serving
producer. Its serving comparison uses the same CLI and native runtime
in both arms, with only the cache switch changed, original target/draft hashes,
plain and DFlash modes, C1/C2, fixed output budgets, within-prompt ABBA, exact
response hashes, whole-block load/thermal exclusions, and independent raw-data
audits. A measured benefit without correctness or concurrency regressions is
required before promotion into the core queue.

The first M3 plain pilot measured +2.70% C1 and +0.65% C2 with exact text and
a local raw-data re-audit. The corrected production DFlash pilot passed with
exact text and was effectively flat (+0.07% C1, -0.23% C2). These single-process
observations do not promote the prototype. See the benchmark report for the
failed DFlash gate and corrected rollback setup.

The subsequent V4 prototype additionally keys routed-policy overrides and
bypasses replay while an attention-tuning override is active. Its M1 build
passed 57,856 exact values, including an actual alternate attention pipeline
and configuration invalidations. Fresh-process serving confirmation remains
unfinished, and V4 remains outside this PR.

### Paired dense projections

An untimed, class-split M5 Pro GPU diagnostic placed roughly 80% of captured
device duration in matrix multiplies: 55% in dense-only groups, 18% in
routed-only groups and 7.5% in mixed groups. Splitting changes scheduling and
fusion, so these shares guide experiments rather than describe normal serving
wall time or explain the main-to-PR gain.

A private paired-Q8 projection candidate preserved weight bytes and per-row
arithmetic. It passed local kernel/fallback and full-model state checks, but a
source-pinned native C1 pilot measured -0.70% throughput despite the isolated
kernel benefit. The code is excluded. See the benchmark report and rejected
experiment evidence; kernel latency alone is insufficient for promotion.
