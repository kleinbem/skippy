# skippy-model-package

`skippy-model-package` supports the Hugging Face job workflow that prepares
and publishes Skippy layer-package repositories. It discovers source GGUF
quantizations and projectors, checks publishing permissions, prepares job
specifications, submits and inspects Hugging Face Jobs, and carries the
versioned split-job script. `skippy models package` is its CLI consumer.

This is job orchestration, not the package-v2 manifest contract or the local
package writer. [`skippy-package-format`](../skippy-package-format/README.md)
owns validated package manifests and identities;
[`skippy-package-builder`](../skippy-package-builder/README.md) builds a local
source-complete package. Runtime loading and materialization belong to the
Skippy runtime and lifecycle API.

Publication requires explicit `SKIPPY_MODEL_CATALOG=namespace/name` and
`SKIPPY_PACKAGE_BUCKET=namespace/name` destinations. There is no implicit
organization destination. Target repositories and jobs default to the
authenticated user's namespace. Its `queue-unsloth-layer-packages` binary is a
specialized batch tool, not the general package-builder command.
See the [model onboarding guide](../../docs/NEW_MODEL_ONBOARDING.md) for a
dry-run example. The binary defaults to dry-run; `--confirm` submits jobs and
can incur Hugging Face Jobs charges.

Source-build jobs clone `Mesh-LLM/skippy` at
`--skippy-ref`. Submission requires `SKIPPY_GITHUB_TOKEN` with read-only access
to that repository in addition to the Hugging Face token. The credential is
sent as a job secret, omitted from dry-run specs, used through Git's credential
helper without writing it into the checkout, and removed from the environment
before the build. Job images must provide the dependencies documented by Skippy's
build guide; builds run through `just package-builder-release-build cpu`.
