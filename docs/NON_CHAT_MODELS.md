# Non-chat model workloads

Skippy's inference API supports model classes that do not use chat completion.
The admitted class is derived from native model metadata; callers cannot turn a
chat model into another workload by changing an endpoint.

Supported standalone surfaces include embeddings, reranking, encoder-decoder
generation and OCR, speech recognition, and speech synthesis. Each class owns
its request validation and response schema in `skippy-inference-api`, and its
runtime implementation in `skippy-serving` and `skippy-runtime`.

Serve an admitted model with the ordinary standalone command:

```console
skippy serve --model /models/model.gguf
```

Use `GET /v1/models` to confirm the loaded model identity and capabilities
before sending class-specific requests. Multimodal models also require the
exact compatible projector selected by the package or `--mmproj`.

The llama upgrade workflow certifies non-chat families with class-specific
smokes and independent monolithic oracles. Qwen3 TTS additionally requires a
valid waveform response. Missing model, projector, audio, image, device, or
oracle evidence is a failure; it is not converted into a passing skip.

See [`skippy-inference-api`](../crates/skippy-inference-api/README.md) for the
HTTP contracts and [`skippy-serving`](../crates/skippy-serving/README.md) for
runtime ownership.
