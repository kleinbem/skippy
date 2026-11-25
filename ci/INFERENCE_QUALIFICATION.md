# DGX Spark inference qualification

The agreed inventory lives in `inference-ladder.json`. Generate a source-bound
plan with `just inference-plan pr FULL_COMMIT_SHA`, or use `nightly` / `release`
for the extended ladder. Split runs apply only to models with at least 27B total
parameters (not activated parameters for MoE). Smaller models run standalone.
PR selects four architecture representatives: four standalone and three split
runs (seven rows). Nightly and release select all eight models: eight standalone
and four split runs (twelve rows). The inventory's explicit `split` flag records
this selection. `just inference-contracts` checks this contract
offline. Planning does not load weights or produce qualification evidence.

The protected PR workflow always reports the `Inference qualification` status.
It checks the current PR's changed-file list before dispatch: serving and build
inputs require all seven smoke configurations; a complete list containing only
unrelated files reports that model tests were not required. A full first page is
treated conservatively as relevant. Required runs never accept skipped model
jobs, and admission failures report failure on the PR head. This permits the
status to be required without leaving documentation-only PRs waiting forever.
Enable that branch requirement only after this workflow has landed on protected
main and its live pilot has passed.

## Execution boundary

Reuse the Pilot GitHub App, signed workflow-job webhooks and JY-managed runner
integration. The Actions jobs orchestrate at most three configurations concurrently. Each hosted
job is bounded to six hours; model execution runs in native JY jobs with five-hour
leases. Model loading, serving,
correctness checks and AgentX run in native JY jobs, outside the Actions
container's 6 GiB limit. Do not raise the limit globally or expose the host
Docker socket to a workflow to work around it.

Trusted same-repository PRs from verified repository collaborators may use native
execution on persistent dedicated Sparks. Protected dispatch code must verify
repository identity and collaborator status before submitting the candidate SHA.
Jobs use the standard JY account and its existing host permissions without
publication credentials, with fresh source-pinned workspaces, read-only admitted
model inputs and exclusive reservations. Clean up processes and workspaces on
every outcome, quarantining allocations when cleanup fails. Fork and other
untrusted PRs require isolated ephemeral execution. An `unconfined` capability
alone does not reject a trusted native job. No separate account migration is required.

Standalone requires a GB10 Spark allocation. Split requires an atomic JY
formation containing two distinct GB10 Sparks on the same reachable network.
Both members must receive the same immutable source SHA and model identity.
Use JY rendezvous, member failure teardown and bounded leases; never reserve
only the coordinator and SSH into an unreserved second machine. Hardware
selection must apply to every formation member, not just a job selector that
the formation renderer discards.

Memory budgets must include actual quantized weights, native allocations,
context/cache capacity, concurrency and an operating-system reserve. Nominal
parameter counts are not memory budgets. Admit the measured footprint before
executing; insufficient capacity must queue or fail. There is no passing skip.

## Evidence and checks

Correctness, scored answer quality and serving performance are separate gates.
Retain raw outputs and diagnostics for each. PR checks are bounded endpoint
smoke tests and make no performance-score claim. Extended qualification
requires a locked harness/scenario/corpus, declared context and load, consistent
warmup, repeated measurements and retained request-level timings and errors.
Do not gate latency on an arbitrary threshold before measuring baseline noise.

Bind evidence to the source SHA, native pin and patch digest, ABI, model shard
hashes, tokenizer and chat template revisions, quantization, split boundaries,
backend/toolkit, actual JY machine identities and harness/settings digest.
Compare each model to its own trusted baseline. Check standalone against an
independent reference and split against standalone. A shared implementation
cannot establish independent numerical correctness by comparing with itself.

PR triggers must include serving, runtime, scheduler, cache, tokenizer, model
loading, topology/transport, native patch queues and their dependency/build
inputs, as well as changes to qualification itself. Existing ordinary CI stays
required; this lane supplements it. Cancel superseded PR allocations and tear
down the entire split formation. Retain nightly and release evidence.

## Implementation and activation

`inference.yml` is the protected controller: pull-request-target dispatch verifies an
open, current same-repository PR and a write-capable collaborator via GitHub.
It uses controller source from main, never executes the candidate on that runner,
and selects inference-related paths. Nightly and manual/reusable release runs
use the same dispatcher. Release publishing depends on qualification.

The repository secret `JIANYANG_INFERENCE_SSH_KEY` is a dedicated submitter SSH
identity. The controller signs a challenge, caches a short-lived session for at
most 30 minutes and renews it during long runs. No admin key is given to Actions
or model jobs. The controller records launches and terminal snapshots under
`target/ci-evidence/inference`; JY retains native logs and artifact directories.
Native artifacts include flat AgentX profile summaries and `agentx.tar.gz` with
the complete nested request evidence, including when the client fails.

`.gi/templates.yaml` defines standalone native jobs and atomic two-Spark
formations. The controller chooses an idle pair from the registered Spark inventory, excluding
its own runner and active native work, then passes explicit per-role node names
because the formation renderer does not apply job hardware selectors.
Each native member independently verifies Linux/aarch64 and an actual GB10.
The process supervisor bounds foreground commands; serving process groups are
terminated on normal and error paths. JY leases bound abrupt termination.

`inference-artifacts.json` pins the base GGUF shard hashes and sizes plus the
matching official tokenizer revision and layer count. Most fixtures use Q4_K_M;
Flash-Next uses the four-shard IQ4_XS base model, not similarly named MTP sidecars.
Admission reserves at least 16 GiB beyond weight bytes and checks available
unified memory. This floor is not proof every context fits; real allocation
failures remain failures. Model caches are explicit, hash-verified inputs.

Nightly and release run CPU chain parity and state-handoff checks against native
monolithic execution (sharing file-backed weights avoids duplicate GPU copies
for the largest model), then requires positive GPU offload evidence and scores three exact-output cases and deterministic
repeats through the serving endpoint. These are a small regression set, not a
comprehensive capability benchmark or an independent numerical implementation.

Performance uses AIPerf 0.13.0 on Python 3.12, seed 42, concurrency one and the
2026-06-21 256k trace variant admitted in `inference-corpus.json`. The upstream loader resolves
main; qualification checks it against the pin before and after replay and retains
both responses, failing if upstream moves. This guards the upstream resolution;
it does not modify the official harness. Tokenizer assets use explicit per-model
file lists, including vocabulary and merges, without CLI wildcard filtering.
PR runs a short endpoint smoke at context 4096: real GPU startup, the three
scored cases with deterministic repeats, and a streamed answer with a 60-second
request deadline and 16-token output cap. It does not run CPU parity or AgentX.
Nightly uses two 900-second AgentX measurements; release uses two 1800-second
measurements, both at context 65536. Streaming and first-turn-prefix cache busting are explicit. Extended
runs require exactly two complete, valid AIPerf trial exports; every profile
rejects nonzero request error rates, error summaries and missing error evidence.
These capacity-limited runs are not full-native-context leaderboard submissions.

Native builds use a host-local executable cache keyed by the immutable source,
Rust/C++/CUDA/CMake toolchains, backend, architecture and build environment.
Cached executable hashes are verified before use and retained in provenance;
invalid entries rebuild. A locked, persistent Cargo target directory is keyed
by toolchains, lockfile and build settings so dependency artifacts survive fresh
checkouts. A separate bounded sccache stores compiler results across commits;
its base directories are refreshed for each workspace. Model shards are copied once into host-local
content-addressed storage, verified by SHA-256 on every admission, then hardlinked
into each fresh job workspace. Hosted CI caches Cargo dependencies and debug
build outputs per OS, architecture, job, matrix row and toolchain/lockfile.
Cache hits accelerate execution and never replace the required smoke evidence.
Cold model startup has a separate five-minute allowance; scored PR requests have
a 60-second deadline and 64-token output cap. Streaming remains capped at 16 tokens.

At `f87a734fdee03dcec0196078a3873a1b8337d3e4`, all seven short PR
configurations passed on DGX Sparks with three configurations in parallel.
The first pass, including cache population, took 21m12s; a second pass took
8m47s. Placement can change between passes, so the second pass is not a
controlled measurement with every cache hot on identical hosts. These timings
cover the inference ladder, excluding unrelated repository CI and its queue.

Local checks and live template-schema admission must precede a native pilot.
No successful hardware qualification is implied by implementation or schema
validation. Record pilot job IDs and any failures before enabling required checks.
The schedule becomes active only when this workflow lands on the default branch.

## Verified bring-up

At source `4f82589ef774b26b6ac1e550c0b2e33ecb7976b7`, both PR-profile
pilots completed successfully on 2026-11-17:

| Configuration | JY job / formation | Result run |
| --- | --- | --- |
| Qwen3.5-4B standalone, Spark 000945 | `job_55bt0pvrgs7sdjr` | `run_b7sd2xddvw9q1pp` |
| Qwen3-Coder-30B-A3B split, Sparks 000137 + 000281 | `frm_q9bm9qy60g9w43p` | `run_j0h6577r5mjqzv2` (leader), `run_jce4kg43p2vc9q9` (worker) |

Both qualification receipts report `passed`, including native parity/state
handoff, positive GPU offload, scored endpoint repeats and AgentX smoke.
Both split members terminated successfully. These pilots do not establish
extended-ladder qualification. The archive retention fix followed these runs;
their nested AgentX files were validated in the workload but were not uploaded
by JY's flat artifact collector. Its regression test checks retention separately.

Standard JY jobs, including the existing job account permissions, are authorized
for trusted same-repository candidate dispatch. Activation requires passing CI
and merging the workflow onto the default branch.
The wider product CI and release roster currently excludes Windows through
`ci/targets.json`'s explicit `enabled_hosts` policy; its definitions are retained
for re-enablement. Disabled Windows rows are not qualified or published.

The 2026-11-17 native repair passed all 308 CPU contract tests, including the
48-layer, three-MTP MiMo2 fixture. Its focused Linux run
`job_r60cpsab1919a8z` peaked at 2,033,428 KiB RSS after releasing temporary MTP
planning contexts. The recipe binding was refreshed only after that full battery
passed; the certified architecture list is unchanged.

At source `a9e1ce8de42af64b779da333c73ff39b175f9b97`, nightly Qwen3.5-0.8B
passed both 900-second AgentX trials in `job_y68xn4m7cnk2f16`, run
`run_mcysg2qc4sd5f5f`. The retained exports contain 62 and 63 completed requests,
zero reported request errors and `submission_valid=true`. This is one extended
configuration, not a completed nightly ladder.

Cold GGUF downloads use HTTP instead of Xet's chunk cache on shared NFS and
remain bounded by the native JY lease. Admission and formation-member polling
share one controller deadline, leaving ten minutes before Actions' six-hour
limit for cleanup and evidence retention.

## Split admission

All profiles use ordinary `plan-split` native admission. Both formation members
retain `split-admissions.json`, containing the native stage descriptors and
activation frontiers. `qualification.json` binds their canonical digest to the
source, model, backend and mode. Missing or malformed native evidence fails
qualification. Nightly also requires chain parity, state handoff and both AgentX
trials. Qualification describes the exact configuration tested by that run.

## PR results comment

The protected inference workflow publishes one bot-owned results comment per PR,
updating it on reruns. The heading is
"Skippy Lab Qualification". It shows
the tested SHA, each serving configuration's Actions outcome, prefill/decode
tokens per second, workflow timing/log links, GitHub evidence and available
durable lab records. PR smoke does not claim AgentX performance measurements.
Rates describe the first response of each of the three scored quality cases at
concurrency 1, excluding the deterministic repeats. They are total phase tokens
divided by total server-reported phase seconds, rather than an average of request
rates. Prefill counts prompt tokens minus cached tokens; decode counts generated
tokens. These very short samples include startup/warmup effects and do not
establish sustained throughput or a controlled speedup comparison.

The comment also shows each phase's percentage change and main TPS against the
latest successful `main` inference workflow run, with its run, attempt and commit
linked. It reuses the matching PR-profile smoke artifacts, including those
produced alongside nightly AgentX by the nightly publisher. This is distinct
from the saved performance baseline in the public dataset, which can be older.
Comparison requires unchanged harness/model/template inputs, matching serving
host class and equal phase token counts. The reporter checks Git tree metadata
without executing candidate code. AgentX receipts never substitute for smoke.
Missing, expired or incompatible evidence shows `Unavailable`; it does not
search older runs or change qualification verdicts. Physical machine placement
and runtime conditions can differ, so these are descriptive smoke deltas.

The protected controller retrieves `qualification.json` and `quality.json` from
the successful native attempt and writes a numeric, per-configuration throughput
receipt into the Actions evidence. Qualification identity must match the tested
SHA, model, mode and PR profile; every scored case must match and pass. The report
checks the receipt against the job and attempt before rendering numbers. Raw
candidate text and logs are never inserted into the comment. Missing native
artifacts or any missing/invalid phase timing show `Unavailable` for the affected
rate; they do not manufacture zero TPS, a partial aggregate or a passing test.
Irrelevant changes explicitly report that model tests did not run; failed,
cancelled, skipped or missing configurations remain visible.

The reporting job runs on success or failure, uses only protected-main code and
has pull-requests-write permission without the native dispatch credential. Its payload
never includes PR titles, candidate-generated Markdown or native log contents.
It refuses to update a comment when the PR head changed or a newer run/attempt
has already reported. Missing controller artifacts leave JY links unavailable;
Actions outcomes still report. Comment failures are visible separately and do not
change the source-bound qualification verdict. GitHub artifact copies have a
30-day retention period; JY job records link to the native evidence.
