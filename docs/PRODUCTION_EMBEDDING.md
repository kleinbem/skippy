# Production embedding

Pin `skippy-api` to a full Skippy source commit and enable
`dynamic-native-runtime`. The `execution` module is the in-process contract
for ordinary model serving. Its owners expose generation and observations
without lending the engine's runtime lock, scheduler, cache, or listeners to
the host. The separate [static Rust/C++ facade](../skippy-api-rs/README.md)
has its own build and native linkage closure. Use the dynamic runtime bundle
for this contract.

```rust
use skippy_api::execution::{
    ExecutionCapabilities, ExecutionError, ModelHandle, ModelOptions, RuntimeBundle,
};

fn open() -> Result<ModelHandle, ExecutionError> {
    let runtime = RuntimeBundle::new("/srv/skippy/runtime", "a".repeat(64))?;
    let model = ModelOptions::new("chat", "/srv/models/chat.gguf", runtime)?
        .with_limits(2048, 1, 128)
        .load()?;
    model.capabilities().require(ExecutionCapabilities::STREAMED_GENERATION)?;
    Ok(model)
}
```

Replace the example digest with the trusted manifest SHA-256 from the bundle
producer. Supply immutable, host-trusted paths. Skippy validates the manifest
and payload hashes, platform, expected runtime release, ABI, llama pin and
ordered patch recipe before opening native code. The loader verifies the
exported ABI as well. Bundle loading is process-wide. Reusing the same
canonical directory and digest reuses the resident libraries. An incompatible
or independently preloaded runtime is rejected. Create a new process to
switch bundles. Model resources have their own lifetime and may be released
while the process-wide libraries stay resident.

## Local lifecycle and observations

- `ModelOptions::load` performs synchronous load. The returned handle is ready.
  A failed load returns a semantic error and releases per-model resources
  acquired so far. A bundle successfully activated before that failure stays
  resident. Options express the model path, runtime bundle, context and lane
  limits, device selection, queue capacity, threads, and telemetry policy.
  The ordinary builder starts with speculation disabled.
- Use `ModelOptions::load_with_model_open_events` with
  `serving::ModelOpenEvents::observe` for typed load progress, diagnostics,
  and a summary. Delivery is best-effort and the load result determines
  readiness. Keep the observer short so it can drain promptly.
- Submit the protocol-neutral `GenerationRequest` with `generate` in a Tokio
  runtime. Read events with `next`. Cancel explicitly with `cancel`, or drop
  the stream or its pending startup future. Cancellation is cooperative.
  Native work can finish its current operation while the stream reports
  `Cancelled`. Dropping an awaiting `next` alone leaves the stream usable.
- `stop` closes admission and waits for execution leases retained by the
  workers. Consume, cancel, or drop outstanding streams so producers can
  finish. The operation can be awaited again if the caller drops its wait.
  Completion releases the owned model resources. Subsequent stops are
  idempotent. Dropping the handle closes access through that handle, and
  in-flight requests retain their resources until their execution leases end.
- `capabilities` describes the loaded handle. Use `require` before committing
  to a feature. Unknown capability names fail the requirement. `inspect`
  returns a point-in-time snapshot. The runtime identity reports the actual
  loaded bundle's backend flavor. Device selection is present when the loader
  binds an explicit device. Automatic and mixed placement can omit a single
  device. Settings report execution bounds and policy. Lanes are the effective
  admission limit. Optional batch bounds and automatic flash attention
  describe where the native runtime chooses details.
- Compute observations are scoped to one loaded stage. Busy time is measured
  in nanoseconds. Operations count timed native work, serial decode tokens
  count decode steps, and verify tokens count positions evaluated in
  speculative windows, including rejected positions. Counters are sampled
  independently and exclude work the meter does not instrument. An absent
  observation means unavailable. A present zero means no work recorded for
  that counter. The handle retains the final meter sample after stop. Hosts
  can separately opt into the existing serving telemetry sink through
  `ServingTelemetryOptions`.
- Inspect `ExecutionError::kind` for handling and retain the error chain for
  diagnostics. Categories cover incompatible runtime, unsupported
  capabilities, invalid options/package/admission, resource admission failure,
  cancellation and a closed handle. Failures without a more specific typed
  source remain execution failures. Hosts own retry and escalation policy.

## Specialist stage execution

`SplitOptions` builds a two-stage direct GGUF chain with explicit stage-zero
return and downstream worker endpoints. It verifies the source, independently
realizes native graphs, checks native stage admission and the activation frontier,
and waits for a binary readiness handshake. `SplitExecution` owns the
in-process frontend and joins the worker after frontend drain. Split defaults
keep MTP, speculative drafting, and cross-lane batching off. Experimental
split admission is an explicit opt-in. See the [external fixture](../fixtures/production-embedding/README.md).

Resolve a package reference with
`PackageAcquisition::resolve_and_inspect_package` before stage planning. A
Hub result pins the verified snapshot commit. Pass its package reference and
manifest digest through admission and stage acquisition. Use
`PackageInspectionPolicy::CacheOnly` when the host must stay offline.

For host-managed placement, staged package materialization, transport, and
advanced inference policy, use the Skippy-owned semantic types in
`stage_admission`, `stage_load`, `package`, `skippy-protocol`, and
`skippy-inference-api`. `ModelHandle::load_specialist` adopts
`serving::ModelLoadRequest` with its hooks, guardrails, lifecycle and KV
observers, and speculative configuration. `StageWorker::start_admitted`
adopts resolved `skippy_serving::binary_transport::BinaryStageOptions`
and the admitted descriptor. It exposes stage readiness, input/output
boundaries, DFlash requirements, a compute snapshot, and an owning stop
operation. `wait_ready` joins and cleans up a failed startup before reporting
failure. Drain stage-zero generation before stopping downstream workers.
Dropping a worker requests cooperative shutdown. Await `stop` when release
must be confirmed. A returned startup or stop error retains the underlying
diagnostic chain.

Hosts authenticate endpoints and bind each peer's compatibility evidence to
its admission and address. The wire readiness handshake confirms the worker
is serving. Stage protocol generation and native recipe compatibility are
validated separately. Routing, leases, membership, coordinator terms,
shutdown generations, plugin schemas, and product configuration belong to
the host. Cache and scheduler internals are implementation layers.
Specialist options remain available at a pinned source revision and can
evolve with that revision.

## Compatibility and evolution

| Layer | What to pin or check |
|---|---|
| Rust source | Pin the commit and lockfile. Use constructors and accessor methods. Accessor-based results can gain fields, and non-exhaustive engine enums can gain cases. |
| Native ABI | Verify the bundle manifest and payload, host platform, runtime release and exported ABI. The runtime loader enforces its supported ABI range. The embedding contract also binds the exact native recipe. |
| Package format | Validate the manifest schema, content identity and artifact closure through Skippy's package APIs. A producer ABI recorded in package metadata is provenance. |
| Stage protocol | Require exact generation and native recipe from each peer and use independently admitted package, plan and activation identities. Generations can differ across deployments. Keep all stages in a given chain on one compatible generation and recipe. |

Observation adoption is independent of feature adoption. For example,
`ComputeObservation::verify_tokens` is optional and the ordinary fixture
uses only `operations`, `busy_nanos` and capabilities. That fixture compiles
against the extended observation without a source adjustment. A host that
wants to present verify-window work deliberately reads the accessor and
updates its own presentation format. Feature discovery uses versioned
capability names. Execution strategies require explicit selection.

See the [Mesh migration map](design/MESH_EXECUTION_MIGRATION.md) for ownership
and wire adaptation.
