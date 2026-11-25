# Mesh migration to the Skippy execution contract

The baseline is Mesh `1ea1ecf754533a6a4fcc0eff8d3f33ff5b73762f` and Skippy
`fbc3349cd406786e43f6579590c9a12b0cd49c05`. The host can adopt the owning
handles and observations incrementally with one pinned standalone Skippy
source and its matching runtime artifact. Mesh retains its authorization,
routing, product configuration and release policy. Dependencies flow from
Mesh to Skippy.

## Snapshot mapping

| Baseline Mesh `StageStatusSnapshot` fields | Source after adoption |
|---|---|
| `state`, `error` | `ExecutionSnapshot::state` and worker `failure`, or the typed load error when startup returns no handle. Mesh still projects orchestration states before a handle exists and after it is removed. |
| `backend`, `selected_device` | `runtime().backend()` and `selected_device()`. Preserve the existing requested-backend management value if it is a routing key. Expose the observed backend separately when the distinction matters. |
| `layer_start`, `layer_end`, activation boundaries | `layers`, `input_boundary`, `output_boundary`. Keep `admission`, codec and codec policy from the admitted Skippy plan in the startup record. |
| `ctx_size`, `lane_count`, `n_batch`, `n_ubatch`, `flash_attn_type` | `settings()`. The adapter can continue to send requested batch bounds and flash attention policy where the wire already represents them. |
| `compute_busy_nanos`, `compute_operations`, `compute_decode_tokens` | `compute()` and its unit-specific accessors. Preserve legacy defaults when emitting existing proto3 scalar fields. Preserve availability in the Mesh in-process adapter and choose a presence-aware field if a future management protocol exposes it. |
| `dflash_target_layers`, `dflash_context_window`, `dflash_feature_encodings` | `draft()` on the attached worker. A present draft with an absent window uses full context. Retain existing absence and empty-list projection. |
| `package_ref`, `manifest_sha256`, `source_model_path`, `source_model_sha256`, `source_model_bytes` | Skippy's package verification results, carried by Mesh's deployment record. The evolving live execution snapshot can be refreshed independently of those immutable resolution results. |
| `materialized_path`, `materialized_pinned`, `projector_path` | Mesh's acquired package/materialization lease and resolved local paths. Keep the lease for the lifetime of the affected Skippy handle. |
| `topology_id`, `run_id`, `model_id`, `stage_id`, `stage_index`, `bind_addr` | Mesh placement and address records. Pass their neutral engine counterparts at stage admission. |
| `coordinator_term`, `coordinator_id`, `lease_until_unix_ms`, `shutdown_generation` | Mesh control state. It owns authenticated requester identity, claim/lease checks, membership and confirmation of which worker was removed. |

The baseline mixes those fields in
[`stage/types.rs`](https://github.com/Mesh-LLM/mesh-llm/blob/1ea1ecf754533a6a4fcc0eff8d3f33ff5b73762f/mesh/crates/mesh-llm-host-runtime/src/inference/skippy/stage/types.rs).
The earlier metering rollout shows the intended management compatibility
projection across [adapter, transport and management API](https://github.com/Mesh-LLM/mesh-llm/commit/4128218ac4c366427d16f309398879be8d937005).

## Dependency and adapter changes

1. Resolve and pin the standalone Skippy source and verified runtime artifact
   at deployment. Start with `skippy-api::execution` for unpartitioned text
   generation. Use existing explicit package acquisition for models that
   need it.
2. Migrate the stage-zero adapter to `ModelHandle::load_specialist` where
   Mesh needs hooks or guardrails. Use `StageWorker::start_admitted` and
   `wait_ready` for specialist workers. Retain host identity and lease
   checks around those operations. Validate each authenticated peer's
   `StageCompatibility` alongside the current stage-control negotiation.
3. Store a Skippy snapshot beside host control state. In `stage_proto.rs`,
   `stage_transport.rs` and the management API adapter, project into the
   existing schema. Keep field numbers, ordering rules, generation, scalar
   defaults, optional presence, and the distinction between an idempotent
   STOP acknowledgement and an actual worker removal. Extend protocol
   compatibility tests for both observed and unavailable compute before
   changing any advertised capabilities or fields.
4. Remove direct `skippy-runtime`, `skippy-serving`, `skippy-cache` and
   `skippy-ffi` imports from ordinary model ownership and status projection.
   Specialist lifecycle policies can still require serving or cache types
   during migration. Keep direct `skippy-inference-api` for HTTP or
   frontend integration, `skippy-protocol` for stage transport,
   `skippy-package-format` and model crates for package/distribution work,
   and `skippy-coordinator` or `skippy-topology` where explicitly used in
   Mesh-owned orchestration. Review event/native diagnostics and hardware
   inventory imports separately before removing a crate from the manifest.

The first paired Mesh change can retain the current stage-control schema and
management output. A new Skippy observation can remain internal until Mesh
adopts it in a deliberate wire or UI change. The existing generation and
recipe gates continue to guard interoperability.
