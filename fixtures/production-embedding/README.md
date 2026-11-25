# Standalone production consumer

This fixture is a separate Cargo workspace. Its only direct Skippy dependency
is `skippy-api` with dynamic native runtime support. Tokio supplies the
executor. Source materialization puts Skippy under `vendor/skippy` inside a
temporary fixture directory so the fixture can use an immutable source without
depending on repository layout or a pre-populated Cargo Git checkout.

From the Skippy repository, `just embedding-contract-check` runs the offline
Rust checks and compiles a copy of the fixture against the candidate.
`just embedding-consumer-check --source <full-commit-sha>` uses `git archive`
of an already available commit for the same offline proof. Cargo preserves
the compatible transitive versions from the pinned source lockfile.
Run `just embedding-serving-test /absolute/dist/native-runtimes` for the
full serving suite, including tests that call the native ABI. Supply the
parent directory containing the verified bundle.

The strict native run requires a runtime directory from `just ci-runtime`
with its producer `provenance.json`, a local GGUF and two free explicit
worker endpoints. Provide trustworthy SHA-256 values for the bundle manifest
and model. Set `--device` to the native device name on accelerator builds.
The supplied GGUF must support text generation and split admission.

```sh
just embedding-consumer-qualify \
  --source <full-commit-sha> \
  --runtime-bundle /absolute/verified-runtime-directory \
  --runtime-manifest-sha256 <manifest-sha256> \
  --model /absolute/model.gguf \
  --model-sha256 <model-sha256> \
  --return-endpoint 127.0.0.1:21910 \
  --worker-endpoint 127.0.0.1:21911 \
  --evidence /absolute/qualification.json
```

The fixture checks incompatible runtime and stage identities, in-process
generation, typed cancellation, drain, compute observations, admitted
two-stage output and endpoint release. Absent inputs, incompatible recipes,
failure to load, and missing execution evidence fail the command.
Split qualification validates native stage plans, runtime recipe compatibility and
matching activation frontiers. The evidence identifies the exact tested model,
backend and topology.
