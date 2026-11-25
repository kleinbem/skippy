# Skippy CLI

`skippy` runs a model locally and exposes OpenAI-compatible and Anthropic
Messages APIs on the same address. It also downloads models, manages native
runtimes, and runs explicit split stages. The CLI owns argument parsing and
terminal output; `skippy-serving` owns the serving loops.

Use `skippy --version` to identify the CLI build and `skippy runtime list` to
inspect installed native runtime releases.

Build with `just build` to package a local native runtime and build the CLI.
The executable automatically discovers the verified `native-runtimes/` directory
beside it, including `target/debug/native-runtimes` from that build. Use
`just skippy-cli-build` only when a compatible runtime is already available.
Private release assets require an authorized GitHub login; download them with
`gh release download --repo Mesh-LLM/skippy`, verify and extract them.
Source builds can have a newer ABI than a published release. Use
`--runtime-bundle /path/to/bundle` to select another local bundle explicitly.

## Configure serving

`skippy serve --help` groups every supported operator control by purpose.

To serve several local models on one listener, list them as `[[models]]`
entries in one TOML file and run `skippy serve --settings serving.toml`.
Each entry can use the same catalog name, Hugging Face reference, or local
path accepted by `--model`. Set `id` when clients need an explicit model name.
See the [multi-model example](../../docs/SERVING_SETTINGS.md#serve-multiple-local-models).
Use `--settings serve.toml` for a complete serving configuration and
`--print-effective-config` to inspect the resolved launch before loading a model.
CLI options override `SKIPPY_SERVE_*` environment variables, file settings and
automatic defaults. See the [serving settings reference](../../docs/SERVING_SETTINGS.md)
for all controls, precedence, examples and mode constraints.

## Run one model on one machine

```sh
skippy models recommended
skippy serve --model Qwen3-0.6B-Q4_K_M
```

`--model` also accepts a Hugging Face GGUF reference such as
`unsloth/Qwen3-8B-GGUF:Q4_K_M`, or a local GGUF path. Skippy resolves remote
GGUF files to an immutable revision and reuses installed files from the shared
Hugging Face cache. It reports the model ID and API address when
`GET /v1/models` succeeds.

Skippy serves GGUF models only. If you have a SafeTensors checkpoint, choose
a published GGUF for the same model or convert the checkpoint offline.

```sh
skippy-quantize convert /models/checkpoint /models/model.gguf
skippy serve --model /models/model.gguf
```

For multipart GGUFs, pass the first shard. Choose the weight quantization when
downloading or producing the GGUF.

For a local GGUF, Skippy's memory-aware context planner sizes one unified KV
pool from the model metadata, weight footprint,
and available device memory, up to a 128k-token ceiling, then defaults to four
lanes sharing that pool. KV stays F16 by default; weight file size does not
change its quantization. `--ctx-size` and `--generation-concurrency` override
the corresponding automatic choices.

For multimodal GGUFs, Skippy looks for a matching installed `mmproj` sidecar.
Catalog downloads include the catalog's projector asset; select an explicit
local projector with `--mmproj /path/to/mmproj.gguf`.

To start the server and immediately chat with the model in the same terminal:

```sh
skippy serve --model Qwen3-0.6B-Q4_K_M --prompt
```

Serving defaults are owned by Skippy, with an
8,192-token completion ceiling, clamped to remaining context space, automatic
chat compaction, and disabled compatibility guardrails. Memory-aware context
and concurrency planning, F16 KV, 512-token batch/microbatch limits, prefix
caching, and prefill policies also use the same defaults. Automatic speculation
uses integrated native MTP at depth one when supported, or a compatible installed
sibling draft with a three-token window. Explicit settings still override defaults.

The prompt inherits the server and model request defaults, including output
length, sampling, and reasoning. Set `serve --default-max-tokens` to change the
server ceiling, or `prompt --max-new-tokens` for a client override.

The API remains available while the prompt is open. Enter `:quit` to end the
prompt and shut down this combined serving session. `:reset` clears chat
history. To connect a prompt to a server that is already running, use
`skippy prompt --endpoint http://127.0.0.1:9337/v1`.

Both API styles use the same model ID and port. In another terminal:

```sh
curl http://127.0.0.1:9337/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"model":"Qwen3-0.6B-Q4_K_M","messages":[{"role":"user","content":"Hello"}]}'
```

```sh
curl http://127.0.0.1:9337/v1/messages \
  -H 'Content-Type: application/json' \
  -H 'anthropic-version: 2023-06-01' \
  -d '{"model":"Qwen3-0.6B-Q4_K_M","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]}'
```

## Find and download models

`skippy models` provides model inspection, downloads and cache maintenance.
Model commands default to human-readable output, even
when piped. Every model subcommand accepts `--json`, alongside Skippy’s global
`--output` modes.

```sh
skippy models recommended
skippy models search qwen --limit 10
skippy models search qwen --sort downloads
skippy models show unsloth/Qwen3-8B-GGUF:Q4_K_M
skippy models download unsloth/Qwen3-8B-GGUF:Q4_K_M
skippy models installed
skippy models updates --check
skippy models updates unsloth/Qwen3-8B-GGUF
skippy models cleanup --unused-since 30d
```

`recommended` reads `meshllm/catalog` by default.
`SKIPPY_MODEL_CATALOG=namespace/name` selects another Hugging Face dataset
repository with the same catalog format. `search` queries Hugging Face for GGUF
repositories. `--catalog`
limits results to the selected catalog. `show` resolves an exact artifact and
displays its revision and file set. `download` uses a catalog layer package when available;
`--direct` downloads the exact Hub artifact instead, and `--draft` also fetches
the recommended speculative draft. Direct downloads verify selected files and
include a catalog projector when present. Serving the same reference reuses the
Hub cache. The reported
SHA-256 describes the bytes obtained but is not an external authenticity claim.

Skippy uses the Hugging Face Hub cache, honoring `HF_HUB_CACHE`,
`HUGGINGFACE_HUB_CACHE`, `HF_HOME`, and `XDG_CACHE_HOME` in that order. Hub
endpoint and token configuration follows Hugging Face settings. `skippy models
delete org/repo:Q4_K_M` previews the selected installed GGUF files; add
`--yes` to remove them. It never deletes a remote repository.

`installed` lists GGUF models and installed layer packages.
`cleanup` is a dry run by default and only targets managed model files recorded
by the CLI; use `--yes` after reviewing its preview. `delete` also previews
derived stage files associated with the selected model before removal.
`updates --check` compares cached repository refs to upstream revisions without
downloading; `updates <repo>` or `updates --all` refreshes cached files and
`config.json` using the verified artifact policy.

Skippy owns the layer-package workflow used by both local and distributed
serving. Package creation is a dry run unless `--confirm` is supplied:

```sh
skippy models package unsloth/Qwen3-8B-GGUF:Q4_K_M
skippy models package unsloth/Qwen3-8B-GGUF:Q4_K_M --confirm --follow
skippy models certify owner/Qwen3-8B-Q4_K_M-layers --package-only
skippy models prune
skippy models prune --yes
```

Publishing requires explicit `SKIPPY_MODEL_CATALOG` and `SKIPPY_PACKAGE_BUCKET`
settings plus `SKIPPY_GITHUB_TOKEN` for private source read access. It defaults
to the authenticated user's Hugging Face namespace.

`models package` also accepts job-management switches (`--status`,
`--logs`, `--cancel`, `--list`, and `--update-script`). Certification can use
`--api-base` for runtime smoke checks and `--report-out` for an auditable JSON
report. Stage-cache pruning is dry-run by default and preserves pinned stages.

## Run a split on one machine

Prepare one config per stage. The ordered `--worker` addresses are the internal
stage listeners; the public inference API remains on port `9337` by default.

```sh
skippy plan-split --model-path /models/model.gguf --model-id local-model \
  --worker 127.0.0.1:9400 --worker 127.0.0.1:9401 --output-dir new-plan
skippy serve --config new-plan/stage-1.json --stage-transport binary --worker-only
```

Start stage 1 in its own terminal, then start stage 0 in another:

```sh
skippy serve --config new-plan/stage-0.json --stage-transport binary
skippy prompt --endpoint http://127.0.0.1:9337/v1 --model local-model
```

`--prompt` can be added to the stage-0 `serve` command. A downstream stage has
no public API, so `--prompt` and `--worker-only` cannot be combined. The binary
stage transport is the only split path. Workers expose the binary protocol
on their stage listener; stage zero exposes the public OpenAI and Anthropic APIs.

Serving tuning flags use names such as `--bind-addr`,
`--generation-concurrency`, and `--prefill-chunk-size` for both local and
binary stage serving. Compatibility guardrails use `--guardrails`.

`plan-split` admits every native stage before writing configs. Its output
directory must not exist. The generated stage files, not the diagnostic
`admissions.json`, are the serving inputs. Regenerate the plan when changing
the topology rather than editing stage files by hand. The direct-GGUF planner
defaults to one lane, a 512-token context, and CPU execution
(`--n-gpu-layers 0`).

Split planning validates the loaded model's native stage graphs, resident tensor
closures and activation frontiers. The planner writes these descriptors in
`admissions.json` with schema version 2. Every model uses the same admission path.
Qualification reports identify the exact configurations that passed inference.

## Run stages on different machines

Use addresses reachable from the other machines in the plan:

```sh
skippy plan-split --model-path /models/model.gguf --model-id local-model \
  --worker 10.0.0.10:9400 --worker 10.0.0.11:9401 --output-dir remote-plan
```

Place each generated `stage-N.json` on its worker and make the exact verified
model files available at the paths recorded in that config. Start the final
stage first and stage 0 last. To shut down cleanly, stop stage 0 first and
wait for it to exit before stopping downstream workers.

## Inspect runtime and machine state

```sh
skippy doctor
skippy runtime list
skippy runtime list --available
skippy runtime install
skippy runtime install metal
skippy runtime install --manifest /path/to/runtime-catalog.json
skippy runtime remove <native-runtime-id>
skippy runtime prune
```

Runtime storage resolves from `--runtime-cache`, then
`SKIPPY_NATIVE_RUNTIME_CACHE_DIR`, then `skippy/native-runtimes` under the
platform cache directory. `SKIPPY_NATIVE_RUNTIME_BUNDLE_DIR` adds bundle roots. `runtime install`
selects the recommended compatible runtime by default; `--manifest` selects an
explicit release catalog. `runtime prune` keeps the active and previous release
unless `--active-only` is passed.

## Output for terminals and automation

Interactive terminals show concise status, download and native model-load progress, and a ready
summary. Use `--output human` to request that presentation explicitly.
Llama.cpp diagnostic logs stay quiet during successful runs. Skippy retains a
bounded recent log history and displays it when a native error occurs or the
command fails. Add `--debug` to stream native logs as they occur:

```sh
skippy serve --model Qwen3-0.6B-Q4_K_M --debug
```

In JSONL mode these diagnostics are `native_log` events with a `message` field.
Model commands retain their human-readable default when stdout is redirected;
use `--json` or `--output json` for a JSON document. Other commands that return
one result automatically use JSON when redirected. A long-running `serve` command uses
JSONL when redirected, or when `--output jsonl` is given:

```sh
skippy models installed --output json | jq .
skippy serve --model /models/model.gguf --output jsonl | jq -c .
```

Each JSONL line has `schema_version`, `sequence`, `type`, and `data`. Wait for
the `ready` event before sending API requests. Progress, diagnostics, and
errors are events in JSONL mode; failures also return a nonzero exit status.
Terminal control characters are never written to JSON or JSONL output.

`example-config` emits one JSON stage-config document without loading a
runtime. SIGINT and SIGTERM request graceful service shutdown; draining
in-flight requests depends on the serving backend.
