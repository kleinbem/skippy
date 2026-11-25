# AgentX comparison provenance

The Metal placement work was originally measured with the external AgentX
client in Mesh PR #3031. Its monorepo campaign scheduler, JY deployment and
MLflow publication tooling are not dependencies of standalone Skippy. The
original campaign and its limitations are available in
[the source pull request](https://github.com/Mesh-LLM/mesh-llm/pull/3031).

Use Skippy's OpenAI endpoint for external fixed-work comparisons. Record the
source commit, native recipe and binary hashes, model and tokenizer hashes,
per-request and total KV capacity, concurrency, batch and microbatch sizes,
cache settings, complete generated output, TTFT and generation throughput.
Alternate repeated baseline/candidate runs on the same idle device. A warm
cache, changed output budget, different total context or CPU fallback is not
a matched performance comparison.

The local correctness entrypoints are documented in
[KV validation](../kv-validation/README.md). They establish separate evidence
from throughput measurements. The imported performance claims remain
historical until reproduced against the standalone native recipe.
