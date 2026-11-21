# Native decode benchmark evidence

Status: draft, 2026-11-17. Three standalone campaigns per mode are complete and locally re-audited;
the matched vanilla comparison is complete as a fixed-budget observation, with output drift documented below.

## Matched comparison

The confirmed table compares Skippy main and the candidate PR; the separate three-way table below includes vanilla llama.cpp.
Vanilla uses the same llama.cpp upstream pin, not the Skippy native patch queue.
An empty hardware/model cell is unmeasured, never an inferred number.

Hardware: one 256 GB Apple M3 Ultra, macOS 26.7.1. Skippy main is
`fbc3349cd406786e43f6579590c9a12b0cd49c05`. The measured candidate snapshot is
`c2055c4a3bb20f601f0074e6fe06a7c85efef01d`; its product source matches this
PR, with subsequent evidence/documentation edits only. Both use llama.cpp
`8954f194837d2ec4d5f67f20c81b84abe63883ba`. The candidate includes the earlier
Mesh-hosted/Jared native work and subsequent engine changes, ported into the
standalone repository. These numbers do not isolate each contributor's changes.

Jared's placement/batching port was already merged into Mesh's `gi` branch as
[Mesh PR #33](https://github.com/general-inference/mesh-llm/pull/33). The measured
standalone extraction baseline is a different source line: its core native
patch queue ends at 0016, while PR #33 contains patches 0017 through 0035 as
well. This comparison includes bringing that work into standalone Skippy; the
headline is not an incremental gain over PR #33.

| Serving mode | Skippy main TPS | Skippy PR #9 TPS | Candidate vs main |
|---|---:|---:|---:|
| Plain, concurrency 1 | 35.90 | 98.98 | +175.70% |
| Plain, concurrency 2, aggregate | 62.71 | 162.17 | +158.62% |
| DFlash, concurrency 1 | 163.64 | 164.82 | +0.72% |
| DFlash, concurrency 2, aggregate | 155.27 | 206.15 | +32.77% |

These are serving throughput measurements: fixed output tokens divided by
pooled HTTP batch elapsed time, including prefill. Each concurrency is pooled
independently. They pool three fresh-process campaigns with three measured cycles each.
Single-request DFlash is effectively unchanged. These are not a cross-hardware
claim or a comparison with BaseRT/Splash.

The target is `unsloth/Qwen3.6-35B-A3B-GGUF`, revision
`a483e9e6cbd595906af30beda3187c2663a1118c`, file
`Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf` (22,360,456,160 bytes), SHA-256
`707a55a8a4397ecde44de0c499d3e68c1ad1d240d1da65826b4949d1043f4450`.
DFlash additionally uses `ethanwtodd/Qwen3.6-35B-A3B-DFlash2-Q8_0-GGUF`,
revision `65aaab9513e9c6608ada7c7d16e6532c198a2296`, file
`Qwen3.6-35B-A3B-DFlash2-Q8_0.gguf` (570,468,544 bytes), SHA-256
`986b8f24ac3469ed122946fb5be5b1fbe186467f2573f84e48bdbf7db278b327`.
There is no model tuning or quantization change between arms.

## Method and custody

- Eight fixed completion prompts, temperature zero, top-k one, 128 output
  tokens per request, concurrency one and two, three warmup and three measured
  cycles, and within-prompt old/new/new/old measured order.
- Context 8192, batch 2048, microbatch 512, four CPU threads and four batch
  threads, f16 K/V, generation concurrency two, native MTP and prompt/disk
  caches disabled; DFlash proposal cap seven when enabled.
- Both arms use packaged release CLIs and native libraries from their pinned
  source trees. All 21 loaded ggml/llama library paths were checked against
  each arm's own bundle. Model/draft and package hashes were verified.
- Whole ABBA blocks with competing load or non-nominal thermals are discarded
  and retried; 100 ms observations cover requests. Across the three campaigns, plain discarded 146 blocks and
  DFlash 52. Each mode accepted 864 requests including warmup.
- All accepted response text hashes match across arms and cycles; every
  response has 128 output tokens and zero prompt-cache hits. Independent
  pooling and artifact audits passed remotely and again on downloaded raw
  evidence locally.
- No build/test work runs on the benchmark host during timed windows.

Jianyang build job: `job_dae3gdjd1gvr22d`, run `run_bjf8g9z1gvr6fzy`.
Measurement job: `job_n64z1nezwe0wp85`, run `run_06cmr5v5yx35a14`.
The measurement job was intentionally cancelled after both retained-v-main
comparisons and audits completed, to publish those artifacts separately from
its unfinished cache experiment. Cancellation does not supply a successful
whole-job or cache-performance result.

The run retains `retained-{plain,dflash}-0-{summary,independent-audit,provenance}.json`
and `retained-{plain,dflash}-0-evidence.tar.gz`, including requests, raw
responses, accepted/rejected batches, thermal/process observations, commands,
and loaded-library paths. Local copies are under the ignored
`target/m3-decode-campaign/measure-artifacts-1/` directory.

## Remaining work

The two further fresh-process campaigns per mode completed in
`job_a3z1zb6g1w0p9vx`, run `run_8mb0n0bdpd81mgb`. All four additional raw
archives passed a second local independent audit.

### Three-way fixed-budget observation

All three arms used the same M3, upstream pin, Apple Clang build 21000334,
model/draft bytes, input token counts, sampling settings, and 128-output-token
budget. Each mode has one fresh-process campaign with three warm and three
measured cycles, measured order vanilla/main/PR/PR/main/vanilla, and the same
whole-block load/thermal exclusions. Vanilla uses `cache_prompt:false` on every
request; Skippy prefix/disk caches are disabled.

| Serving mode | Vanilla llama.cpp TPS | Skippy main TPS | Skippy PR #9 TPS |
|---|---:|---:|---:|
| Plain, concurrency 1 | 96.55 | 36.15 | 98.93 |
| Plain, concurrency 2, aggregate | 148.79 | 62.93 | 161.64 |
| DFlash, concurrency 1 | 169.20 | 164.08 | 165.55 |
| DFlash, concurrency 2, aggregate | 196.99 | 154.82 | 205.24 |

**Vanilla cross-engine exact-text parity failed.** Skippy main and PR text is
exact throughout. Vanilla differs on prompt 2 in both modes, and prompt 5 at
plain C2; plain prompt 2 also varies within vanilla C2 repeats. These are
fixed-budget throughput observations, not a byte-identical vanilla speedup
claim or proof of equivalent model quality. No prompts were removed. The
three-way result does not replace the confirmed three-process Skippy table.

The initial plain job `job_4b086x0mh18p2qg` (run `run_caphfdqssssaw4j`)
failed its strict cross-engine parity gate after collecting the whole plain
workload. Its timing integrity and Skippy parity were independently re-audited
post hoc; the failed gate remains recorded. The subsequent DFlash job
`job_9mrpbw1jg7nt0mx` (run `run_2f0fe28qzdyx2qb`) predeclared fixed-budget
comparison with strict Skippy parity and explicit vanilla drift reporting;
it completed, and its raw archive passed a second local audit. Each mode
accepted 432 requests including warmup; plain rejected 44 blocks and DFlash 21.

An untimed probability diagnostic (`job_3s9va64cxdk99p6`, run
`run_3yt8y8aa1v6hqd5`) found plain prompt 2 C1 first diverges at output token
66: vanilla selects “ Components” over “ Concepts” with log-probability margin
0.0903, while PR reverses that ranking with margin 0.0318. Both prompts matched
at C2 in all three diagnostic repeats. The cause remains unproven; do not
attribute this to a one-ULP difference. The upstream server documentation also
warns that batch sizes can change output, but that does not itself establish
quality equivalence here.

### Attribution

A separately authorized M1 Ultra ablation compared the same retained runtime
with `SKIPPY_RS_SEQ=2` versus `0` (`job_r1a6e5jzn1kvhd9`, run
`run_153re0gjqtjqykr`). Disabling snapshots improved plain C1 from 66.04 to
68.89 TPS (+4.30%) and C2 aggregate from 97.52 to 102.41 TPS (+5.02%), with
exact text. That different-hardware result is not pooled into the M3 table.

A second same-CLI M1 ablation changes only backend sampler row capacity from
legacy zero to one, with rollback depth zero in both arms (`job_k53yk8s75d55f2f`,
run `run_s70jtw5rzwq4bbw`). C1 measured 68.72 → 68.90 TPS (+0.27%); C2 measured
102.02 → 102.33 TPS (+0.31%). All 288 accepted responses match exactly;
37 whole blocks were rejected, and downloaded raw evidence passed a second
local audit. This single campaign does not demonstrate a material gain from
sampler row capacity. Neither ablation explains the full main-to-PR improvement;
further attribution remains necessary.

A same-CLI M1 convolution-launch ablation changes only channels per threadgroup
from one to 32, retaining the same shader math and all other native settings
(`job_ekbcxzxfy3bz7e6`, run `run_79ex575fm5mqtd6`). C1 measured 68.21 → 68.79
TPS (+0.86%); C2 aggregate measured 101.28 → 101.95 TPS (+0.67%). The latter's
three cycle changes were +1.27%, +1.06%, and -0.32%. All 288 accepted requests
have exact text, 35 blocks were rejected, and the raw archive passed a second
local audit. This is another small isolated contribution, not an explanation
of the full main-to-PR gain.

Two API tests remain unqualified because the bundled split-certification roster
was recorded against ABI 0.1.80 and its original recipe. The measured candidate
used ABI 0.1.85. The integration against current Skippy main uses ABI 0.1.88.
Split and family certification require qualification of the exact recipe.

Cached Metal dispatch prototypes, including serialized/deduplicated signatures
and in-place snapshots, remain outside this PR. Their correctness/profile
results do not contribute to any TPS number above.

## Unpromoted cached-encoding pilot

A separately built V3 cache runtime (`8b0e6bc6a60ae587e9beb75123b84c4f08bd9d2a`,
native tree `03ebbc56f70519b50ec66db1234a96f4a38791c9`) uses the same CLI and
runtime in both arms, changing only the cache switch. One M3 plain campaign
measured C1 99.27 → 101.95 TPS (+2.70%; cycles +2.33%, +2.88%, +2.89%) and
C2 aggregate 162.33 → 163.39 TPS (+0.65%; cycles +0.64%, +0.70%, +0.61%).
All 288 accepted responses match exactly; 10 blocks were rejected, and the
downloaded raw archive passed the independent audit again locally.

These are preliminary, single-campaign observations, excluded from this PR's
product code and confirmed headline. Job `job_n8hpce6fspce5my`, run
`run_p3kv0ya7ys4enht`, later failed its DFlash exact-text gate. That DFlash arm
also inherited a zero-rollback override from the plain-only ablation harness,
forcing host checkpoints instead of production speculative rollback. Both
cache-off and cache-on C2 produced the same two variants of prompt 2, with the
same counts. Those data do not qualify production DFlash behavior. A new
DFlash campaign restored the production defaults; no gate was waived.

That corrected production DFlash pilot (`job_t2yjbkrj1fthjzs`, run
`run_174vdj1s14bh7cn`) passed exact text for all 288 accepted requests, rejected
six blocks, and passed the downloaded raw-data audit. C1 measured 165.20 →
165.33 TPS (+0.07%); C2 aggregate measured 206.65 → 206.17 TPS (-0.23%).
These results are effectively flat. The rollback override was unset in both
arms. Fresh-process V4 confirmation remains outstanding.

The packaged cache runtime additionally passed actual Metal unit/invalidation
checks on a Dan-lab M4 Mac mini (`job_9trvfmjfdj54tf9`, run
`run_vv2132wyp91m1d9`): 54,016 exact values including retired-source cases,
with all loaded ggml paths from its own bundle and a local artifact re-audit.
This is hardware correctness evidence, not M4 serving throughput.

## Rejected paired-projection experiment

A private Q8_0 candidate combines two independent projections sharing one input
into one Metal dispatch. It retains their separate weight/output buffers and
the existing per-row dot-product body. Local M5 Max screens using original
weight bytes reduced GPU time by 28–31% for a small shared-expert pair and
4–5% for the tested query/key pair across three ABBA cycles. Outputs were byte
exact. Batch, distinct-input and overlapping-output cases declined fusion.
Four full-model state exports also matched exactly with flash attention auto,
context 8192, and the candidate pipeline observed. These local diagnostics do
not establish packaged serving performance.

The subsequent source-pinned M5 Pro 48 GB pilot (`job_66qwv9eg4ebffmn`, run
`run_whb7ey3jx2r11ws`) used one loaded engine and switched the fusion flag
between matched requests. The same compiled runtime served both arms. The
direct native iteration API included session creation, prefill, 128 sampled
tokens and cleanup; HTTP and frontend scheduling were absent. Three warm and
three measured cycles covered all eight fixed prompts in within-prompt ABBA.

Native C1 throughput measured **66.40 → 65.94 TPS (-0.70%)**, with measured
cycle changes -0.25%, -0.91% and -0.94%. All tokens matched, 192 full-length
requests were accepted including warmup, and two whole blocks were rejected.
The downloaded requests, block selection, load/thermal windows, library paths
and recomputed rates passed an independent local audit. This is one process
campaign on a separate hardware/compiler cohort, not a serving-table update.
The candidate remains excluded from this PR: the kernel win did not produce a
native decode gain. Pins and audit hashes are in
[`metal-decode-q8-pair-rejected.json`](evidence/metal-decode-q8-pair-rejected.json).
