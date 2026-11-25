# Skippy

Skippy is an AI inference engine for local and distributed workloads. It is the inference engine that powers Mesh LLM.

Built on llama.cpp, Skippy adds production-focused model and runtime management, OpenAI- and Anthropic-compatible APIs, explicit pipeline parallelism, multi-tier KV caching, memory-aware scheduling, and hardware-specific optimizations.

Skippy supports CPU inference and GPU acceleration through Metal, CUDA, ROCm, and Vulkan. Run it on a single machine or divide one verified model across an ordered set of workers. A single CLI and configuration model takes you from local inference to multi-machine serving.

## Build and run

Run repository commands from the workspace root:

```bash
just build
./target/debug/skippy --help
```

`just build` produces the CLI and a verified adjacent native runtime. Private
release bundles can be downloaded with `gh release download` and used offline.
A typical local invocation is:

```bash
./target/debug/skippy serve --model /path/to/model.gguf
```

See the [CLI guide](crates/skippy-cli/README.md) for runtime management, model
downloads, single-model serving, and explicit split-worker commands. See
[CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow.

## Source layout

| Path | Ownership |
|---|---|
| [`crates/`](crates/) | CLI, model tooling, runtime, serving, protocol, topology, caching, metrics, and benchmarks |
| [`docs/`](docs/) | Operator guides, compatibility status, designs, experiments, and runbooks |
| [`evals/`](evals/) | Skippy performance and correctness evaluation workloads |
| [`scripts/`](scripts/) | Skippy-owned build, packaging, certification, and benchmark automation |
| [`llama_cpp/`](llama_cpp/) | Pinned llama.cpp source metadata and the durable Skippy ABI patch queue |

The main entry point is [`crates/skippy-cli`](crates/skippy-cli/). Runtime
execution lives in [`crates/skippy-runtime`](crates/skippy-runtime/), serving
coordination in [`crates/skippy-serving`](crates/skippy-serving/), and the
shared HTTP surface in
[`crates/skippy-inference-api`](crates/skippy-inference-api/).

## Key guides

- [Architecture and API boundaries](docs/ARCHITECTURE.md)
- [Retained compatibility identifiers](docs/COMPATIBILITY.md)
- [Configuration reference](docs/CONFIGURATION.md)
- [Serving settings](docs/SERVING_SETTINGS.md)
- [Continuous integration](ci/README.md)
- [Private releases](RELEASE.md)
- [Split serving](docs/SKIPPY_SPLITS.md)
- [Layer package repositories](docs/LAYER_PACKAGE_REPOS.md)
- [Qwen3-MoE native MTP](docs/QWEN3_MOE_NATIVE_MTP.md)
- [Supported model families](docs/FAMILY_STATUS.md)
- [Data flow](docs/DATA_FLOW.md)

The repository was extracted with filtered Git history from the original
workspace. Its native ABI and selected compatibility field names remain stable
where changing them would invalidate existing packages or clients.

## Health

[![CI](https://github.com/Mesh-LLM/skippy/actions/workflows/ci.yml/badge.svg)](https://github.com/Mesh-LLM/skippy/actions/workflows/ci.yml)
[![Inference qualification](https://github.com/Mesh-LLM/skippy/actions/workflows/inference.yml/badge.svg?event=schedule)](https://github.com/Mesh-LLM/skippy/actions/workflows/inference.yml)
[![Release](https://github.com/Mesh-LLM/skippy/actions/workflows/release.yml/badge.svg)](https://github.com/Mesh-LLM/skippy/actions/workflows/release.yml)
[![Documentation](https://github.com/Mesh-LLM/skippy/actions/workflows/docs.yml/badge.svg)](https://github.com/Mesh-LLM/skippy/actions/workflows/docs.yml)
[![Llama upgrade](https://github.com/Mesh-LLM/skippy/actions/workflows/llama-upgrade.yml/badge.svg)](https://github.com/Mesh-LLM/skippy/actions/workflows/llama-upgrade.yml)
[![CI images](https://github.com/Mesh-LLM/skippy/actions/workflows/ci-images.yml/badge.svg)](https://github.com/Mesh-LLM/skippy/actions/workflows/ci-images.yml)
