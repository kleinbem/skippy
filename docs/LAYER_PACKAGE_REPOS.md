# Layer-package repositories

Skippy layer packages hold immutable stage artifacts plus the metadata needed
to reproduce and admit a split. Package creation, publication, certification,
and consumption are owned by this repository; they do not require a MeshLLM
checkout or host.

Preview and create a package with the standalone CLI:

```console
skippy models package unsloth/Qwen3-8B-GGUF:Q4_K_M
skippy models package unsloth/Qwen3-8B-GGUF:Q4_K_M --confirm --follow
skippy models certify owner/Qwen3-8B-Q4_K_M-layers --package-only \
  --report-out certification.json
```

Publication requires explicit `SKIPPY_MODEL_CATALOG` and
`SKIPPY_PACKAGE_BUCKET` destinations. Private source checkout jobs require
`SKIPPY_GITHUB_TOKEN`; the credential is scoped to source acquisition and is
removed before build commands run. No implicit organization namespace or
rolling release destination is used.

Package consumers must pin a repository revision. Certification verifies the
manifest, complete file roster, hashes, layer coverage, native ABI, recipe, and
model identity before executing a stage. A locator is never treated as proof of
content.

The wire schema and compatibility rules live in
[`skippy-package-format`](../crates/skippy-package-format/README.md). Package
construction lives in
[`skippy-model-package`](../crates/skippy-model-package/README.md), and runtime
admission lives in [`skippy-runtime`](../crates/skippy-runtime/README.md).
