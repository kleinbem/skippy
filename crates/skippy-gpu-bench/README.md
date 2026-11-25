# skippy-gpu-bench

GPU bandwidth benchmark parsing and runner selection owned by Skippy and used
by Skippy hardware detection and runtime planning.

Native sources move with the crate. Packaged helper names and native entry
points retain their current spelling until the coordinated runtime artifact
migration; this package move does not change their protocol or implementation.

Run `skippy --runtime-bundle /path/to/verified-runtime --output json gpu-benchmark`
with a Metal, CUDA or ROCm bundle and a supported accelerator. The command
fails if a compatible runtime, packaged tool, or actual measurement is
missing. Use `--timeout-secs` on the subcommand to bound execution. Interrupting
the command requests cancellation and waits for process cleanup.

Embedders can call `measure` with `NativeMeasurementRunner` and the helper
from their verified runtime. Mesh can attribute results by supplying the
helper's visible backend ordinals and their stable device IDs. Preserve the
same visibility environment during discovery and measurement. Results
without a caller mapping identify the helper's ordinal only. A missing
compute result remains absent and zero remains a measurement.

Use `store_fingerprint` and `load_fingerprint` with a caller-selected path.
Key the cache by the helper/runtime and driver identity, physical hardware
identity, and ordered visible device mapping. A changed key invalidates the
entry. Call `measure` again to refresh it.
