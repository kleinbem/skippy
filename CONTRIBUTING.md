# Contributing to Skippy

Install Rust, Just, CMake, Ninja, and the native toolchain for the
backend you are building. Repository commands run from the root:

```console
just build
just test
just lint
```

Use `just build cpu` for a portable CPU runtime. macOS defaults to Metal;
Linux and Windows default to CPU unless a backend is supplied. CUDA builds
require the matching pinned toolkit and architecture list.

Default tests are offline. Model-backed and accelerator qualification use
explicit certification commands and immutable fixtures. Do not make default
tests depend on a network, writable home directory, or undeclared system tools.

The production Cargo workspace is rooted here. `skippy-api-rs/` is a separate
facade workspace and must be checked independently. Native source is prepared
from `llama_cpp/upstream.txt` plus the ordered patch queues under
`llama_cpp/patches/`.

Follow [AGENTS.md](AGENTS.md) and the relevant component README before changing
native ABI, release, or CI behavior.
