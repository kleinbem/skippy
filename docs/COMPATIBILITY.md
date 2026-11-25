# Compatibility

## Served weights

GGUF is the only served weight format across the CLI, Rust API, and native
runtime. Skippy accepts a local GGUF file, the first shard of a multipart GGUF,
or the selected GGUF parts of an admitted immutable package. Hugging Face model
references resolve to GGUF weights.

For a SafeTensors checkpoint, select a published GGUF or convert the checkpoint
offline with `skippy-quantize convert /models/checkpoint /models/model.gguf`.
Then serve the GGUF path or build a package from it. Select weight quantization
and supply any importance matrix during offline quantization. Checkpoint
shards and model sidecars can still be used to produce a GGUF.

## Retained compatibility identifiers

The standalone repository contains no MeshLLM product packages, path
dependencies, build entrypoints, or release URLs. A small set of historical
identifiers remains byte-for-byte stable because changing it would invalidate
existing data or an external worker protocol.

- `mesh-llm-skippy-patch-queue-v2`, `mesh-llm-hf-multipart-view-v1`, and
  `mesh-llm-split-gguf-v1` are persisted hash-domain separators. They do not
  invoke or depend on MeshLLM.
- `MESH_HF_JOB_INPUT`, `MESH_HF_JOB_REQUEST`, and
  `MESH_HF_PUBLICATION_TOKEN` are fields in the deployed Hugging Face worker
  transport. Skippy owns the producer and consumer, but renaming the wire
  variables would break already provisioned workers.
- `mesh_version` and old `mesh-llm-*` paths are accepted only by the explicit
  legacy native-runtime import path and its offline fixtures. Current schema-two
  manifests use independent Skippy release identity.
- Historical issue references and third-party source provenance remain for
  attribution and regression context.

New code must use Skippy-owned names. Additions to this list require an explicit
compatibility reason and tests proving the old identifier is data or wire
format—not an active downstream product dependency.
