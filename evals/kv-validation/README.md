# Native KV validation

The native regressions run through the same standalone build script as CI:

```sh
just native-full-replay cpu
just native-full-replay metal
```

The complete suite includes batched KV placement, release/compaction/append,
wide-to-narrow sampling-pool limits, recurrent checkpoint ownership, and
Metal row-list attention against the CPU reference. A required GPU run needs
the actual matching device; a CPU run does not qualify Metal or CUDA.

## Concurrent HTTP reuse

Start the exact candidate server with the chosen model and prefix caching,
then supply a sufficiently long prompt file:

```sh
just cache-reuse-smoke http://127.0.0.1:8080/v1 MODEL prompt.txt result.json 4
```

This checks sequential cold/repeat/growing requests, concurrent copies of the
grown conversation, and distinct follow-ups answered in a batch and alone.
A reuse miss or changed answer fails the check. Keep this separate from
throughput measurement and durable-restart evidence.

## Real-model K/V operator matrix

Build the evaluator against the prepared source and the matching packaged
runtime's shared-library directory, using absolute paths:

```sh
just kv-matrix-build /absolute/runtime/lib
target/kv-validation/indexed-real-kv-matrix /absolute/model.gguf MTL0 f16
```

Select CPU, MTL0 or CUDA0 explicitly. Omitting the dtype enumerates the
evaluator's K/V matrix; optional K_TYPE and V_TYPE arguments select a pair.
The evaluator verifies the backend, actual graph K/V types, row-list shape,
attention-layer coverage and finite logits, then records a greedy completion
for comparison across runs. Prefix-restore and numerical-parity checks belong
to the native regression suite above. Unsupported combinations
are reported separately and do not certify a required pair. Use the pinned
model identities in [models.json](models.json); verify their hashes before
running model qualification. Never substitute downloaded model bytes for
provided inputs.

The [historical validation plan](PLAN.md) records the original monorepo
campaign. Its xtask/JY campaign commands were not imported. Standalone
qualification uses the commands above and the repository's existing CI and
inference workflows. Hardware, matched performance and persistent restart
gates must be reported separately from offline unit/operator tests.
