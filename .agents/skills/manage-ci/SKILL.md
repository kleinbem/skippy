---
name: manage-ci
description: Maintain standalone Skippy CI, native hardware qualification, llama upgrades, and tagged releases.
---

# Skippy CI ownership

Read this file before inspecting or changing CI. This is the standalone Skippy
contract; no downstream product's workflow topology applies here.

- `.github/workflows/pr-checks.yml` owns the single intended required `PR checks`
  aggregate on the repository's selected Ubuntu runner. Protected-main code reads
  workflow/check/status results without executing candidate code or using lab
  credentials. Failures/cancellations fail it; pending or missing applicable work
  stays pending; success/neutral/intentional skips pass. Producer starts and
  completions re-evaluate reruns; latest exact-head results govern. Add new PR
  producer workflows to its event list. Activate the one-check branch rule only
  after the controller reaches main and its real aggregate is verified; preserve
  the previous two required checks until then, and retain all review protections.
  Acceptance criteria are also required: the live PR description must contain
  one `## Acceptance criteria` section with every task checked, including the
  standard tasks from the protected-main PR template. Missing, empty, duplicate,
  or incomplete sections fail. Comments and code examples are not evidence.
  Description edits re-evaluate the gate; re-read the live body before publishing
  success. This validates contributor declarations, not the truth of those
  declarations; retain CI and review requirements.

- `.github/workflows/ci.yml` owns code checks and package builds. Declarative
  native paths trigger CI on automation, code, package or qualification inputs.
  Independent declarative PR filters select hosted offline automation, source
  checks, and Linux ARM64 CUDA 13 plus macOS ARM64 Metal package/device smoke.
  Qualification scripts and reporting do not select PR package builds. Reporting
  and offline check recipes belong in `ci/automation.just`; root `Justfile` edits
  remain conservative build inputs. Main pushes retain conservative source
  checks. Inference, packaging and native main changes select both device
  smokes; native changes additionally select Graph Contracts. Compare main
  changes since a verified successful ancestor main-push CI run so coalesced
  and failed pushes cannot hide untested inputs. If previous coverage cannot
  be verified, require both device smokes and Graph Contracts. The protected
  inference workflow uses its separate qualification paths in its native
  trigger for the seven Spark configurations. PR qualification is manual: the
  jobs run only when the PR carries the `qualify` label, while the workflow
  still triggers on path matches so an intentionally skipped run satisfies
  the `PR checks` aggregate; nightly and releases stay automatic. Main pushes
  do not start the Spark ladder. `PR checks` reads protected trigger paths to determine
  which runs it expects. Hosted offline
  automation gates Quality canary and package jobs; Quality canary gates package
  jobs whenever source checks apply. Full CI
  runs Graph Contracts in parallel with eight build, package and smoke jobs.
  Each CPU job supplies a shared platform CLI to its backend jobs. Routine CI
  uses two device jobs and checks the three remaining CUDA archives on hosted
  compiler-image runners.
  Tagged releases add device smoke on those exact archives, including x64 CUDA 13.
  The lightweight CI gate checks the validated matrix and requires all applicable results.
- `.github/workflows/docs.yml` directly validates `docs-site/**` changes on PRs
  and deploys to beta on protected-main pushes. The protected PR aggregate
  expects this workflow when docs paths match. Tagged release validation calls
  the same workflow to produce the production docs artifact.
- `.github/workflows/ci-images.yml` builds and qualifies the CPU, CUDA and ROCm
  compiler images without publication credentials. Protected-main publication
  pushes the exact tested image. The package jobs use the matching x86_64 or
  ARM64 CUDA image. Bootstrap image publication before changing consumers, and
  require actual device results for GPU qualification.
- `.github/workflows/release.yml` runs daily and on dispatch from protected main
  (a daily run takes the next patch version and skips an already released main), stages the
  version-only candidate, and calls the shared validation implementations on
  its exact SHA. Only the publisher can promote the verified commit and create
  its tag together. It cannot move an existing tag or publish an incomplete
  artifact matrix. Canary runs leave main, tags, and publication untouched.
- `.github/workflows/llama-upgrade.yml` keeps control on protected main, prepares
  an immutable candidate and runs existing full CI once. Graph contracts and
  small synthetic execution tests remain the architectural regression gate.
  There is no additional live-model CPU, CUDA or Spark upgrade battery.
  Registered synthetic cases must match their metadata and test modes. Uncovered
  architectures/builders, including new and changed ones, are informational and
  stay visible in the graph coverage report. Coverage gaps alone never fail an
  upgrade; existing contract regressions and actual integration failures do.
  Candidate preparation and repair run directly on the native JY Linux runner
  (`jy-rnr_vfnz8k5p4yrh334`, `jy-linux-docker`) with its installed Goose and
  provider authentication; do not move them into a hosted CPU container or
  onto Depot. The workflow defaults to noninteractive Goose and preflights its
  native tools. Seed the protected current patch queue before replaying a new
  upstream revision so cold repositories have intermediate merge-base blobs.
  A bounded agent repair produces a fresh candidate and fresh full CI. Only the
  publisher receives scoped publication credentials. It can publish only the
  exact candidate selected by successful full CI. The four established inference
  models retain their separate PR/manual/nightly/release workflow.
- `ci/targets.json` owns four host definitions and eleven runtime definitions.
  `enabled_hosts` selects the active CI and release roster. Windows is temporarily
  deferred; the active roster has three hosts and eight runtimes. Do not publish
  artifacts for disabled hosts or describe those hosts as qualified.
  Keep CLI builds backend-neutral and build each host once. Package by copying
  exact verified producer outputs; consumers must not rebuild missing inputs.
- Keep workflow path routing declarative in YAML. Keep Just recipes and platform adapters thin. Use
  shell for orchestration and focused Python modules for structured build and
  qualification work under `scripts/`. PR lab-result comments use the focused Bash/gh/jq report
  script without Rust setup. Prefer shell for small CI checks, with Python as
  a fallback when structured parsing needs it.
- All builds use Just. Run Cargo serially within one checkout. Include both
  Rust workspaces, full crate tests, FFI contracts, and native API contracts.
- Ordinary tests are offline. Model admission is explicit and digest-bound.
  GPU qualification requires a real matching device; missing fixtures, devices,
  capacity, or evidence fail or queue, never become passing skips.
- Split admission is established by actual native stage plans, tensor closures,
  activation frontiers, artifact identities and backend compatibility. Qualification
  records exact tested configurations. There is no architecture certification
  allowlist or experimental bypass. PR, nightly, release and upgrade use the same
  native admission requirements.
- Trusted same-repository PRs from repository collaborators may run as native
  JY jobs on persistent dedicated lab hardware, including DGX Sparks. Admission
  must verify the source repository and contributor's collaborator status using
  GitHub; a branch name or same-repository check alone is not a trust decision.
  Dispatch and admission policy run from protected code, never from PR source.
  Fork and other untrusted PRs require isolated ephemeral execution and receive
  no native lab dispatch credentials.
- Trusted native PR jobs may use the standard JY account and its existing host
  permissions. Do not pass publication credentials or other jobs' secrets. Use a
  fresh source-pinned workspace, read-only admitted model inputs, exclusive JY
  machine reservations, bounded leases and process-tree/workspace cleanup on
  completion, failure and cancellation. JY's `unconfined` capability alone is
  not a rejection. No separate job-account migration is required.
  Cleanup failure quarantines the allocation until reconciled.
- Runner groups and dispatch credentials are repository-scoped. The Actions
  runner only orchestrates inference qualification; native JY jobs receive
  explicit model-sized resource budgets rather than the 6 GiB Actions-container
  limit. Split runs reserve both machines atomically. Publisher credentials
  remain separate from all build and inference jobs.
- Pin third-party actions by full commit SHA. Default permissions are read-only.
  Only release controller jobs may write the candidate refs. Release promotion
  and upgrade publication each use a scoped App credential. Do not expose
  publisher secrets to builds.
- The protected inference results-comment job alone receives pull-requests write
  permission for PR comments, plus read-only contents/actions. It receives no
  native dispatch credential and never executes candidate code.
- Cache keys bind source recipe, lockfile, toolchain, platform, and backend.
  Caches are accelerators, not artifact correctness or qualification evidence.
  Restore Cargo and compiler caches on native runners as well as hosted runners;
  image-provided tooling does not make fresh containers retain compiler outputs.
- Cancel in-progress validation only for superseded PRs. Main-push runs of
  `ci.yml` share one running slot and one pending slot. Newer pushes replace
  the pending run while the active run finishes. Release and upgrade concurrency
  must retain evidence and never cancel another release or an independent CI run.
- The code workflow's `CI` summary rejects failed, cancelled, or missing selected jobs;
  `PR checks` additionally covers the separate inference and image workflows.
  Upload diagnostic evidence on failure before cleanup.
- Runtime metadata includes the source SHA, llama pin, patch digest, ABI,
  architecture, backend, toolkit and file hashes. Signing, if configured, runs
  before final hashes. Preserve licenses and CUDA redistribution notices.

Before editing, inspect the affected workflows, Just commands, Rust owners,
target matrix and `ci/README.md`. Validate syntax, matrix consistency, relevant
Rust tests and the actual Just commands. Report unexecuted hardware checks and
unverified external runner/repository configuration explicitly. Never describe
workflow syntax validation as a successful build or hardware qualification.
