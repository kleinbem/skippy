# Explicit split serving

Standalone Skippy does not discover peers or create a mesh. A split is an
explicit ordered set of workers prepared from one verified model.

Create admitted stage configurations with `plan-split`:

```console
skippy plan-split --model-path /models/model.gguf --model-id local-model \
  --worker 10.0.0.10:9400 --worker 10.0.0.11:9401 --output-dir split-plan
```

Place each generated `stage-N.json` on its worker with the exact model bytes
recorded by the plan. Start the final stage first and stage zero last:

```console
skippy serve --config split-plan/stage-1.json --stage-transport binary --worker-only
skippy serve --config split-plan/stage-0.json --stage-transport binary
skippy prompt --endpoint http://127.0.0.1:9337/v1 --model local-model
```

Stage zero owns the public OpenAI- and Anthropic-compatible API. Downstream
stages expose only the binary stage transport. Use private, authenticated
network boundaries for workers; the stage transport is not a public ingress.

The plan output is authoritative. Regenerate it when the topology, model,
runtime, or layer boundaries change instead of editing stage JSON manually.
For all CLI flags and local single-machine examples, see the
[Skippy CLI guide](../crates/skippy-cli/README.md).
