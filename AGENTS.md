# Skippy contributor instructions

This private repository owns Skippy. Rust packages live under `crates/`;
the separate Rust facade workspace lives under `skippy-api-rs/`, and the C++
facade lives under `skippy_api/`.

Use Just for all builds. Prefer shell for CI orchestration and simple checks;
use Python for structured manifest parsing, artifact verification, and
qualification. Keep each command in a focused script under `scripts/`.
Honor supplied native source and dependency inputs without downloading replacements.

Run Cargo commands serially in each checkout. For changed Rust crates run
formatting, focused check, warning-denying all-target Clippy, and full tests
including integration and documentation tests. Never claim a build/test passed
until it exits successfully. Never silence new warnings without a documented
reason. Do not introduce generic utility modules or functions above configured
Clippy limits. Extract separable responsibilities from oversized files.

Keep `llama_cpp/upstream.txt` and the ordered core, model-support, and generated
patch queues authoritative. ABI declarations, Rust bindings/version constants,
generated documentation, and native source-recipe identities must remain consistent.
Model-specific behavior belongs in model builders, not family-name branches in
shared stage machinery. Keep stage parity, state handoff, and graph contracts.

Read `.agents/skills/strong-types/SKILL.md` before giving a value in `crates/`
a type in place of a sentinel or a bare count. `skippy_api/AGENTS.md` and
`skippy_api/TYPE-RULES.md` hold the same rules for the C++ facade and the bridge.

Read `.agents/skills/manage-ci/SKILL.md` before CI work. PR and main use the same
CI implementation; tagged releases reuse it. Builds have no publication secrets.
Trusted same-repository PRs may use standard native JY jobs on persistent
hardware, with fresh source-pinned workspaces and no publication credentials.
The standard JY job account is accepted; no separate account migration is required.
Fork and other untrusted PRs require isolated
ephemeral execution. See the CI skill for admission and cleanup requirements.

Default tests are offline and use isolated temporary state. Model and hardware
qualification is explicit and cannot pass through missing-fixture/device skips.
Keep all third-party licenses and preserve wire/hash compatibility deliberately.
Never commit credentials. The extraction plan and progress ledger are under
`docs/design/`.

## Pull requests

Use `.github/PULL_REQUEST_TEMPLATE.md` for PR descriptions, including when
creating a PR through an agent or CLI. Replace prompts with evidence from the
final diff; do not submit an empty template or only a list of changed files.
Keep descriptions current when the scope changes.
The required `PR checks` gate rejects missing or incomplete acceptance criteria.
Retain the template's standard criteria and check them only when satisfied;
additional criteria in that section must also be complete. Editing the PR
description re-evaluates the gate. Checkboxes do not replace CI or review.

- Give the PR a clear title describing the resulting capability or fix. Lead
  the description with the original problem and what users or maintainers can
  now do. For internal changes, explain the operational benefit.
- Include diagnostics or reproduction evidence and explain how the change
  addresses the cause. Link related issues, PRs, or failing runs when available;
  an issue is not required.
- For CLI changes, include example commands and representative output. Label
  illustrative output as such. For API changes, show the affected usage.
- Describe material architecture changes separately from the opening summary.
  Explain ABI, API, wire/hash, model/runtime, or artifact compatibility and any
  required migration when those contracts change.
- Record the exact validation commands, relevant configuration/features, and
  observed results. Distinguish completed checks from failed, pending, or
  unrun checks, and give the reason for gaps. Keep validation separate from the
  problem and fix.
- Performance claims need a baseline and changed result, workload, model,
  hardware/backend, and evidence. Label estimates explicitly. State whether
  model or hardware qualification ran when relevant; offline tests or skipped
  fixtures/devices do not establish qualification.
- Keep the description proportionate to the change. Omit optional sections
  that do not apply; include screenshots for visual changes when relevant.
