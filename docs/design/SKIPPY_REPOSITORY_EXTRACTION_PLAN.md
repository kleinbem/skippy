# Standalone Skippy repository extraction plan

Status: implemented as a standalone repository tree. This document retains the
original extraction decisions and acceptance criteria; live runner, tag-release,
and downstream Mesh cutover acceptance remain separate operational work.

Prepared against source commit `937f236c3de65f496324af79739d4aed7b438733`.
Original destination: a standalone private Skippy repository.

## Outcome and scope

Skippy will own its source, native runtime, llama.cpp pin and patches, build
automation, tests, documentation, version, and releases. A clean checkout must
build and release without a MeshLLM checkout, executable, library, workflow,
runner image, or release service. Keep Just as the build entrypoint.

Use one CI workflow for both pull requests and pushes to main. Tag-triggered
releases call that same workflow, then publish its verified artifacts. A separate
scheduled/manual llama upgrade workflow qualifies upstream candidates and opens
ordinary pull requests.

This is a new Skippy CI contract. The existing Mesh repository's five-lane PR/main
contract and dispatch-only release contract remain unchanged until a separate
downstream migration. The new repository must get Skippy-specific AGENTS and
manage-ci instructions, rather than inheriting those Mesh topology requirements.

The initial release target set preserves the four existing CLI platforms and
CPU, Metal, CUDA 12, and CUDA 13 runtime coverage. It adds macOS arm64 CPU.
ROCm/Vulkan release jobs are outside this initial requested matrix; retain their
working backend implementations. Intel macOS and Windows arm64 are not existing
CLI release targets and are not assumed here.

## What moves

| Current source | Destination and treatment |
| --- | --- |
| `skippy/crates/` | `crates/`: all 34 Skippy-owned production/tool crates, with their tests and READMEs. |
| `skippy/llama_cpp/` | `llama_cpp/`: upstream pin, core patches, model-support patches, generated patches, and queue metadata. |
| `skippy/skippy_api/` | `skippy_api/`: public C++ facade, native tests, benchmarks, fuzzers, and analysis tooling. |
| `skippy/skippy-api-rs/` | `skippy-api-rs/`: preserve its separate two-crate Rust workspace and lockfile initially. |
| `skippy/scripts/`, `skippy/evals/`, `skippy/docs/` | `scripts/`, `evals/`, `docs/`: audit each retained file for Mesh implementation or workflow dependencies. Rewrite current product docs; omit Mesh integration history that is not useful standalone. |
| Root Cargo/Just/toolchain/linker configuration | Create Skippy-only root files; preserve applicable profiles, lints, toolchain selection, and dependency overrides. |
| Selected root `scripts/lib/` and linker drivers | Move the required native toolchain/platform adapters; remove root forwarders that only served the dual-product layout. |
| Selected `tools/xtask` commands/tests and process supervision | Extract native preparation, generation, ABI verification, runtime packaging, model admission, standalone smokes, and release validation. Do not copy the whole automation workspace. |
| Relevant `ci/` model manifests and native fixtures | Keep immutable model identities and Skippy-native contracts. Remove Mesh SDK, host, UI, plugin, and network fixtures. |
| Skippy-related skills and native API documentation generator | Retarget paths and output generated API reference under Skippy `docs/`. |

Exclude all `mesh/` implementation, Mesh SDKs and bindings, console/website,
plugins, peer discovery/routing, deployment files, release footer, registries,
packaging dispatch, and Mesh-specific CI orchestration. Keep required upstream
and third-party licenses and attribution.

Preserve Skippy development history by rewriting the original repository's
`skippy/` subtree onto the new repository root. Retain commit authors, dates,
messages, merges, and ancestry for commits that touch the subtree. Record the
source commit and history-filter procedure, and audit all refs before publishing.
Do not push original unfiltered refs, tags, reflogs, or unreachable Mesh objects.
New standalone infrastructure lands as ordinary commits on that filtered history.

Proposed root layout:

```text
Cargo.toml / Cargo.lock / Justfile / AGENTS.md
crates/
llama_cpp/
skippy_api/
skippy-api-rs/
scripts/
just/
tools/xtask/
tools/process-supervision/   # only if still needed by extracted automation
ci/
docs/
evals/
.github/workflows/{ci,release,llama-upgrade}.yml
```

## Coupling that must be removed

The production Rust manifests already depend only on sibling Skippy crates.
That does not yet make the directory independently buildable:

1. **Workspace and paths.** Production crates inherit the root workspace version,
   dependencies, and edition. Build scripts and shell entrypoints assume the
   current `skippy/` depth and root forwarders. Update those together, regenerate
   the root lockfile without unrelated dependency upgrades, and use `skippy-cli`
   as the default member. Keep the separate facade workspace's current version
   policy explicit rather than accidentally merging it into the production one.
2. **Automation.** `tools/xtask/Cargo.toml` depends directly on
   `mesh-llm-release-footer`; its commands also implement many Mesh workflows.
   Extract capability-owned Rust modules and their tests into a smaller xtask.
   Do not copy Mesh code under a Skippy name. Keep Just and platform adapters
   thin; do not replace this with new Python repository tooling.
3. **Windows.** The Skippy build currently invokes
   `mesh/scripts/build-windows.ps1` through a forwarder and `-SkippyOnly`.
   Extract native toolchain setup and Skippy build/package behavior without the
   Mesh/UI handlers. Validate on native Windows.
4. **Runtime defaults.** `skippy-runtime-install/src/types.rs` points to
   `Mesh-LLM/mesh-llm` releases; cache/discovery paths and build metadata use
   Mesh names. Give standalone Skippy its own paths, configuration, catalog,
   artifact names, and build-version variable. Embedders pass their policy
   explicitly; Mesh-specific defaults belong downstream.
5. **Remote model jobs.** The embedded `split-model-job.sh` clones MeshLLM and
   exposes `mesh_llm_ref`. Retarget its source revision and authentication to
   Skippy, updating job inputs, CLI help, generated instructions, and tests.
   Download a source-bound prepared artifact where that avoids putting a broad
   repository credential in a remote model job.
6. **Documentation and fixtures.** Native API generation currently writes into
   `mesh/website/`. Move that output and its freshness gate to Skippy docs.
   Replace Mesh SDK-backed test consumers with equivalent standalone API/native
   consumers where necessary; do not import the Mesh SDK to keep a gate running.
7. **Compatibility data.** Audit legacy names such as `mesh_hooks`,
   `mesh_compact`, persisted `mesh_managed`, and patch/package hash domains.
   Preserve protocol/data compatibility where required, or explicitly version
   the migration. These literals are not authorization to retain Mesh behavior;
   blindly renaming them can invalidate packages and caches.

Add a dependency/source contract that rejects Mesh workspace packages, external
path dependencies, Mesh repository fetches, and Mesh build/release entrypoints.
Review any retained legacy literals through a narrow documented compatibility
list. Scan release contents as well as manifests.

## Just interface

These are the proposed standalone commands; they are not implemented yet:

| Command | Contract |
| --- | --- |
| `just` / `just build` | Build the CLI and a complete adjacent native runtime; default Metal on supported macOS and CPU elsewhere. |
| `just build cuda` / `just build cpu` | Explicit local backend selection. |
| `just test` | Offline unit, integration, and documentation tests; include both Rust workspaces and the native facade through their owning commands. |
| `just lint` | Formatting, warning-denying Clippy, applicable native checks, and repository contracts. |
| `just ci <target> <backend>` | The same checks/build/package/smoke contract used by PR, main, and release CI for that row. Separate shared work so a matrix does not rebuild hosts repeatedly. |
| `just release-build <backend>` | Release-profile CLI plus native runtime for the local target. |
| `just release-package <target> <backend>` | Verify and package existing producer outputs; no compilation during composition. |
| `just llama-prepare` | Cold-capable replay of the pinned native queue. |
| `just llama-check` | Patch/generator/ABI/native contracts. |
| `just llama-certify <suite> <backend>` | Explicit model-backed certification, separate from default offline tests. |

Build automation must honor supplied source/toolchain/dependency inputs without
downloading replacements. Preserve read-only source/install support, configurable
state directories, and builds with vendored Cargo inputs. Run Cargo commands
serially within each checkout/job; separate matrix jobs have isolated outputs.

## One PR and CI pipeline

`ci.yml` has `pull_request`, `push` to `main`, and `workflow_call` entrypoints.
All entrypoints use the same matrix, commands, profiles, and pass criteria.
Start without changed-file routing or a planner service: every change gets the
same complete required check, `CI`.

The graph is:

```text
quality + offline tests
        |
        +-- host: one CLI per OS/architecture
        +-- native: one runtime per target/backend/toolkit
                        |
                 compose and verify
                        |
             packaged inference smoke
                        |
                    CI summary
```

Quality includes both Rust workspaces, applicable C++ API tests, required FFI
tests, patch/generator freshness, standalone dependency checks, and documentation
contracts. Explicitly select the second workspace; `--workspace` at root will
not cover it. Full crate tests include integration and doc tests, not only
`--lib`. Preserve separate native ABI/static facade validation even though CLI
releases use dynamic runtimes.

Build each backend-neutral host once per target. Runtime producers bind the
llama pin, patch digest, ABI, platform, backend, CUDA variant, and toolchain.
Consumers require exact source/digest-matching artifacts and never silently
rebuild missing inputs. Cache keys include those boundaries; caches accelerate
the pipeline but do not establish artifact correctness.

Every package gets import/dependency closure checks, manifest/checksum
verification, runtime discovery, no-driver CLI startup, and Qwen3 dense inference
through the normal standalone server. Metal/CUDA inference requires actual
hardware; installing a toolkit or building a library is not execution coverage.
Model fetching is an explicit pinned CI preparation step, not part of default
offline tests. Smokes consume verified cached inputs and record selected devices.

Use ordinary hosted runners for CPU/build work where adequate. Provision
Skippy-owned toolchain images for native builds. GPU PR jobs require isolated,
ephemeral hardware runners without release credentials; main uses the same test
contract. Do not place ordinary PR code on persistent lab machines. Runner
availability is an implementation prerequisite, especially Windows CUDA and
Linux arm64 CUDA; a missing required device must not become a passing skip.

Use read-only tokens, pinned actions, explicit timeouts, and no signing or
publishing secrets in CI. Cancel superseded PR runs only. A stable summary
rejects failures, cancellations, and missing required rows. Namespace concurrency
and artifacts so a release invocation cannot cancel or consume a PR invocation.

## Release matrix and tag flow

| OS | Architecture | Required runtime variants |
| --- | --- | --- |
| macOS | arm64 | CPU, Metal |
| Linux | x86_64 | CPU, CUDA 12, CUDA 13 |
| Linux | arm64 | CPU, CUDA 12, CUDA 13 |
| Windows | x86_64 | CPU, CUDA 12, CUDA 13 |

This yields four CLI producers and eleven native runtime variants. Define the
matrix once in a checked-in manifest consumed by CI and release validation.
Pin exact CUDA toolchains and supported GPU architecture lists within each
variant. Metal is macOS-only; CUDA is Linux/Windows-only. The initial plan does
not promise a Cartesian product of unsupported OS/backend combinations.

Publish a complete CLI-plus-runtime bundle for each row, plus optional
backend-neutral CLI archives and standalone runtime archives for embedders.
Include licenses, SHA-256 sidecars, runtime manifests, and release metadata
binding source commit, upstream pin, patch digest, ABI, and artifact hashes.

`release.yml` uses `push.tags: ['v*']`:

1. Validate strict SemVer, freeze the peeled tag commit, require a reviewed
   main-reachable commit, and require the tag version to match the committed
   production workspace version. Prerelease versions produce prereleases.
2. Call `ci.yml` on that exact commit. Build and test the entire matrix; do not
   trust a previous run identified only by a branch name.
3. Finalize any release-only platform signing, reverify affected outputs, then
   generate final hashes/manifests. The current Skippy packaging path does not
   provide Apple notarization or Windows Authenticode; those credentials and
   steps must be deliberately provisioned if wanted. Do not inherit Mesh's
   host footer signing machinery.
4. Require all eleven bundles and their verification evidence. Upload to a
   private draft release, verify the uploaded roster and checksums, then publish
   the complete release. Only this job receives publication permissions.
5. Reruns may finish the same draft using matching source/artifact identity, but
   must not overwrite a published release or move its tag. A failed release
   leaves diagnostics and no partially published final release.

Version changes happen in a normal PR before tagging. The workflow never bumps
versions, pushes main, generates a different source commit, or creates its own
trigger tag. Skippy versions evolve independently from Mesh; the native ABI and
package format retain their separate compatibility versions. Keep the imported
version as the baseline and choose the first new version in the release PR.

Tags are pushed by a maintainer or appropriately scoped GitHub App. A tag pushed
using a workflow's ordinary `GITHUB_TOKEN` does not start another push workflow;
do not accidentally build that trigger loop into a Just release convenience
command. Protect release tags from movement/deletion and restrict their creation.

### Private downloads

Changing the current public URL to a private repository URL is insufficient:
the runtime downloader currently performs unauthenticated HTTP requests.

For a private release, use authenticated `gh release download --repo
Mesh-LLM/skippy --pattern <bundle>` and install the complete bundle.
Adjacent `native-runtimes/` discovery then works offline without credentials in
the serving process. Standalone commands should report a clear private-download
instruction when no local runtime is present, rather than falling back to Mesh
releases. A native authenticated GitHub asset client can be a later feature.

Do not enable public crates.io, npm, website, or package-manager publication as a
side effect of a private binary release. Default new workspace packages to
`publish = false` unless a separate publication policy is approved.

## Llama upgrade pipeline, including Qwen3

Use one `llama-upgrade.yml`, scheduled daily and manually dispatchable with an
optional full upstream commit override and `force_certify` (default false) for
unchanged-pin certification. Keep the controller on protected main.
Start with deterministic preparation and certification; autonomous repair agents
are not necessary for the first standalone pipeline.

```text
resolve upstream release to immutable SHA
  -> replay core + model-support queues
  -> regenerate candidate generated queue, bindings, and docs
  -> cold replay complete candidate queue; check equality/idempotence
  -> native build/contracts + standalone CI
  -> model-family qualification
  -> independent clean build and qualification of identical candidate
  -> create/update upgrade PR with evidence
```

Use the current upstream release selection semantics: latest published
non-draft release, including prereleases, resolved to an exact SHA. Keep
`llama_cpp/upstream.txt` authoritative. An unchanged pin is a no-op unless
explicit certification was requested. Core/model-support conflicts or product failures
retain diagnostics and stop for a maintainer repair; they must not produce a
certified upgrade PR. A repair invalidates prior evidence and is requalified.

Preserve cold patch replay, queue ordering, generated-patch equality and
idempotence, C/C++ public-header checks, Rust binding freshness, ABI mirrors,
and the unfiltered native suite. Reuse the normal CI implementation for platform
builds and tests. Keep native, model, and infrastructure failures distinguishable.

Complete generation and certification-roster preparation before freezing the
candidate as a local commit. Upload a digest-bound Git bundle. The protected-main
upgrade workflow passes that bundle and its expected commit/tree identity to
`ci.yml`; source preparation restores it into isolated worktrees while workflow
and runner policy remain protected-main-owned. All normal CI rows run unchanged.
Independent verification restores the same bundle into fresh worktrees. Admit
this source mode only for the protected upgrade invocation; ordinary PR and
release calls use immutable GitHub SHA checkout. The final publisher pushes
exactly the verified commit, without subsequent regeneration or edits.

The checked-in family manifest already contains the following Qwen3 coverage.
Carry all of it forward with immutable revisions, sizes, hashes, and projectors:

| Family | Existing fixture | Required behavior |
| --- | --- | --- |
| Dense | Qwen3-0.6B Q8_0, about 0.64 GB | Generation, standalone API, split single-step/chain parity, state handoff. Also the routine CI smoke fixture. |
| MoE | Qwen3-30B-A3B-Instruct-2507 Q4_K_M, about 18.56 GB | Expert execution, planner-selected split parity, persistent state. |
| Next | Qwen3-Next-80B-A3B-Thinking IQ2_XS, about 22.22 GB | Hybrid/recurrent state, restore and handoff, full-model versus split parity. |
| VL | Qwen3-VL-8B Q4_K_M, about 5.03 GB plus projector | Actual image/projector execution locally and through supported splits. |
| VL MoE | Qwen3-VL-30B-A3B Q4_K_M, about 18.56 GB plus projector | Vision and expert execution with the existing split contract. |
| TTS | Qwen3-TTS-12Hz-1.7B Q8_0, about 1.85 GB plus sidecar | Existing speech workload smoke/oracle; do not claim distributed staging support. |

These are model file sizes, not runner memory requirements. Derive memory
admission from concurrent contexts, baseline copies, KV/state, and reserve.
Retain large-memory lab capacity for full certification; the existing pipeline
uses 128/256 GiB tiers. Do not size workers only by GGUF file size.

Require real CPU, Metal, and CUDA qualification for the Qwen3 contracts supported
on each backend. Preserve the current declared backend support matrix and make
any newly required backend cells a rollout gate, not an assumed pass. Full
family qualification need not repeat every CUDA toolkit/OS combination; the
ordinary CI matrix supplies packaged dense smoke evidence for those variants.

Qwen3 embedding and reranker certification are not currently full canary rows;
an optional facade reranker test is not equivalent evidence. Add pinned fixtures
and gates separately if those specific capabilities are to be advertised.

Retain the broader supported-family roster and existing profile-specific tests
when extracting the native engine. Passing Qwen3 alone must not preserve stale
certification claims for other families after a pin change. Regenerate and
validate actual native stage admissions against the exact candidate recipe, and require
the complete declared supported-family pass before promotion.

Model preparation belongs to a separate verified cache population step; the
hardware battery consumes offline immutable fixtures. Workers do not receive
GitHub write credentials. Only a final publisher can create the normal upgrade
branch/PR, using an App token so the ordinary PR CI runs automatically. It never
merges the upgrade, moves the main pin directly, or tags a release. Keep
candidate and independent-verification evidence bound to the same source and
native recipe. Publication fails on missing or mismatched evidence.

## Mesh consumption and cutover

Skippy never calls back into Mesh. The downstream Mesh repository keeps its
adapter and product policies, and replaces Skippy path dependencies in its
fourteen consuming crate manifests with one immutable Skippy revision, recorded
in `Cargo.lock`. Its native runtime recipe/artifact identity must match that
revision. Mesh integration tests remain in Mesh CI and do not gate ordinary
Skippy PRs by fetching or building Mesh.

A private Git dependency changes who can build Mesh from source. For an
authorized private consumer, use a repository-scoped read credential for source
and release assets. A public Mesh build cannot fetch private Skippy anonymously.
Keep the existing public snapshot usable until an explicit source-distribution
decision permits a vendored export or another accessible dependency channel.
Do not silently break existing public build instructions during extraction.

Cut over Mesh only after a standalone prerelease is verified. During transition,
freeze or explicitly synchronize the old Skippy copy; avoid two active owners.
After downstream compatibility passes, remove the duplicated active source and
obsolete combined native/build/release jobs in that consumer. Rollback pins Mesh
to its last known working revision/runtime; it does not move Skippy release tags.

## Implementation sequence and acceptance

1. **Freeze inventory and import.** Record source SHA, licenses, current pin/ABI,
   and supported target/family manifests. Rewrite the complete `skippy/` subtree
   history onto the repository root, remove original unfiltered refs, and audit
   the rewritten ancestry before creating/pushing the private repository.
2. **Make builds independent.** Relocate the workspace and native paths, extract
   automation and Windows support, rewrite Just entrypoints and product defaults.
   Keep the llama pin unchanged while proving the extraction.
3. **Restore standalone contracts.** Migrate both Rust workspaces, native facade,
   generators, docs, offline tests, runtime discovery, and remote model jobs.
   Verify dependency boundaries and supplied-input/offline builds.
4. **Bring up identical PR/main CI.** Provision target toolchains and ephemeral
   GPU runners, run the full matrix on an initial PR and main, then require its
   stable `CI` result. Exercise a deliberately failing fixture to prove required
   rows cannot be skipped or masked by the summary.
5. **Rehearse a tag release.** Push a prerelease tag with a matching committed
   version. Verify all eleven bundles, private authenticated download, clean
   offline installation, no-driver CLI startup, and actual CPU/Metal/CUDA serving.
   Test failed-upload recovery and refusal to replace a published release.
6. **Enable llama upgrades.** Certify the unchanged pin first, then one real
   upgrade candidate and independent rebuild. Prove Qwen3 and the retained family
   roster gates, failure evidence, PR creation, and ordinary CI on that PR before
   enabling the daily schedule.
7. **Migrate downstream Mesh.** Resolve private-source distribution/access, pin
   the accepted Skippy revision/runtime, run Mesh integration/compatibility
   checks, and remove obsolete ownership only after those pass.

The extraction is complete only when a clean clone with the Mesh checkout and
Mesh artifacts unavailable builds using `just build`; all promised release rows
execute; a tag publishes a complete private release; and a llama upgrade can be
qualified and proposed without Mesh code or services. Build success alone does
not establish GPU inference, private download, or release correctness.

## Planning evidence and external prerequisites

Primary source anchors in the current repository:

- [Product boundary](../ARCHITECTURE.md) and [Skippy guidance](../../AGENTS.md).
- [Root workspace](../../Cargo.toml) and [Just build interface](../../Justfile).
- [Automation dependencies](../../tools/xtask/Cargo.toml) and [Windows build owner](../../scripts/build-windows.ps1).
- [Runtime catalog](../../crates/skippy-runtime-install/src/types.rs) and [remote model job](../../crates/skippy-model-package/src/scripts/split-model-job.sh).
- [Family fixtures](../../ci/llama-canary/model-fixtures.json) and [upgrade orchestration](../../.github/workflows/llama-upgrade.yml).
- [CLI packaging](../../scripts/package-cli-release.sh), [runtime packaging](../../scripts/package-native-runtime.sh), and [nested Rust workspace](../../skippy-api-rs/Cargo.toml).

GitHub documents [reusable workflows](https://docs.github.com/en/actions/how-tos/reuse-automations/reuse-workflows),
[workflow triggering and token behavior](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/trigger-a-workflow),
and [authenticated release asset access](https://docs.github.com/en/rest/releases/assets).

GitHub did not resolve the standalone repository at planning time.
Publication therefore includes an explicit private-repository
creation and settings audit rather than assuming prior state. Local standalone
build, lint, Rust/native tests, history verification, workflow contracts, and
deterministic llama patch replay are implementation gates. GPU/Windows runner
capacity, GitHub App credentials, tag-release rehearsal, private release asset
download, and downstream Mesh cutover remain external acceptance prerequisites;
they are not implied by local validation.
