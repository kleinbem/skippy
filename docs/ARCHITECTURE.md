# Skippy architecture and API boundaries

Skippy is a standalone model-serving product. This repository owns its CLI,
configuration, serving lifecycle, native runtime, package formats, and releases.
Embedding products consume explicit Skippy APIs and artifacts; they do not
change Skippy's defaults or build graph.

## Entry points and contracts

| Surface | Owner | Intended caller and contract |
|---|---|---|
| `skippy` CLI | `skippy-cli`, `skippy-commands` | Operators running model management, local OpenAI serving, or explicit stage workers. |
| HTTP inference frontend | `skippy-inference-api` | OpenAI-compatible, Anthropic Messages, and SystemOne routes share one backend and lifecycle contract. |
| Production Rust embedding | `skippy-api::execution` | Hosts load an explicitly pinned dynamic runtime and own model or admitted stage handles. See [production embedding](PRODUCTION_EMBEDDING.md). |
| Specialist Rust lifecycle | `skippy-api::serving`, `stage_load`, `stage_admission` | Hosts compose advanced serving policy and admitted stage controls at a pinned revision. |
| Native runtime selection | `skippy-api::native_runtime` | The caller supplies bundle and cache locations. The API neither chooses a product home directory nor downloads implicitly. |
| Package-v2 format | `skippy-package-format` | Producers and consumers share validated manifests and content identities. Format compatibility is separate from the native ABI. |
| Stage wire protocol | `skippy-protocol` | Internal, generation-gated stage traffic. Mixed generations fail closed; it is not a backwards-compatible public network API. |
| C ABI | `llama_cpp/patches`, `skippy-ffi` | Experimental version-0 capability API. The patched native header and Rust version constants advance together. |

The production execution handle owns the model, listener, scheduler and cache
resources. Specialist callers can opt into deeper Skippy types for a pinned
source revision. Mesh and other embedding hosts own coordination, transport
identity, and product configuration.

## Ownership flow

```text
operator/client -> Skippy CLI or embedding host
                -> inference frontend / Skippy lifecycle API
                -> serving + scheduler + cache
                -> Rust runtime -> Skippy C ABI -> patched llama.cpp

model reference -> artifact/HF adapter -> package format and admission
                -> runtime load
```

The lifecycle API takes explicit runtime and model locations rather than reading
another product's configuration. The CLI uses `SKIPPY_DATA_DIR` as its optional
application-data override. `HF_HUB_CACHE`,
`HUGGINGFACE_HUB_CACHE`, `HF_HOME`, `HF_XET_CACHE`, and `XDG_CACHE_HOME` still
take precedence where applicable.

For split execution, callers supply admitted workers, measured capacity and
transport latency. `skippy-coordinator::topology` owns placement, runtime
headroom, KV and compute reserve, and the capacity recheck after a cut moves.

`skippy-model-hf::store` uses the Skippy application cache root.
Its `_in` operations take an explicit application cache root for tests and
embedding applications. The persisted `mesh_managed` usage-record field is retained
for compatibility with existing records; it does not select a product path.

Skippy's canonical HTTP request extension for enabling its serving hooks is
`skippy_hooks`. Readers also accept the older `mesh_hooks` compatibility field.
If both are present, the canonical Skippy field wins. The legacy `mesh_compact` and
`_mesh_respond` guardrail wire names remain unchanged; changing those is a
distinct client-compatibility decision.
