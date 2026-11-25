# Default draft pairing in Skippy

`skippy_api::speculative::DraftCatalog` selects DFlash, DFlash2, and DSpark
drafts. It uses the target's GGUF architecture, executable decoder depth
(excluding NextN/MTP blocks), embedding width, and expert count. For shared
shapes, a catalog row can require a model family in the GGUF basename or name.
The host can supply a trusted model name when both GGUF fields are absent.
A family-specific row blocks the shape-only row for that family even when the
family-specific draft is ineligible on the current backend.

## Selection and acquisition

1. Translate the host's resolved intent to `DraftOverride`. Explicit disable
   wins immediately. An explicit local DFlash draft replaces automatic
   selection and is checked during preparation. A named package strategy can
   also be requested explicitly.
2. Pass the effective `DraftBackend` and `DraftTarget` to
   `DraftCatalog::resolve`. Native hosts can derive capabilities from the
   registered runtime devices, offload, and selected device using
   `DraftBackend::for_native_placement`. A package declaration owns automatic
   selection. Its first backend-eligible DFlash proposer wins in declared
   order. An unavailable DFlash proposer or a different package strategy
   ends DFlash selection.
3. With no package declaration, a verified pinned index wins. The compiled
   bootstrap data is used when the index is absent or invalid. A valid index
   with no matching entry is authoritative. `DraftCatalog::from_cache` reports
   the index status separately from the selection result.
4. Use `DraftPlan::prepare` with `DraftAcquisition::CacheOnly` or
   `DraftAcquisition::Download`. The latter accepts the existing
   `HfModelRepository` and Hub progress handler. The resolver itself performs
   no I/O. Hosts that refresh the release index can explicitly call
   `DraftCatalog::fetch` and keep their current catalog if verification fails.

A selected catalog or package draft must match its pinned revision, exact
file, byte count, and SHA-256. Preparation then checks GGUF architecture,
hidden width, and target layers, including any exact package declaration.
When both headers expose a vocabulary size, those sizes must agree. Runtime
attachment still checks the loaded target and draft. Missing files, transfer
failures, integrity failures and incompatibility return a typed error without
trying a lower-priority pairing. The cache owner can repair rejected files.
Verification leaves shared cache files intact.

An embedding host can use `ModelLoadRequest::load_with_default_draft` on an
otherwise valid baseline request. A verified default replaces native MTP or
the ordinary draft for that load. An attachment failure retries the original
request. A host using a lower-level entry point can apply `PreparedDraft` to
its inference options and preserve its original plan for fallback. Explicit
draft failures can be reported directly to the caller.

Standalone local `skippy serve` runs the same resolver. It can download a
missing pinned draft at startup and reports progress through the CLI. With
`HF_HUB_OFFLINE=1` or `--print-effective-config`, draft preparation uses
only the local cache. Config inspection without a loaded native runtime
omits automatic backend-dependent selection. On a selected default's
preparation failure it keeps native MTP when
enabled, or serves with plain decoding. Sibling discovery remains available
for automatic ordinary draft models when no package or default DFlash
pairing owns the choice. Binary split serving uses its existing explicit
stage plans and draft placement.

## Mesh adapter replacement points

| Mesh responsibility | Skippy API |
| --- | --- |
| `config/dflash_index.rs` loads and installs a pinned catalog | `DraftCatalog::from_cache` and `DraftCatalog::fetch` return catalog instances that the host can share across requests |
| `config/dflash_catalog.rs` holds bootstrap rows, matches GGUF metadata, checks backend eligibility, and selects package proposers | `DraftCatalog::resolve`, `DraftBackend`, and `DraftTarget` |
| `config/dflash.rs` chooses a draft and sets the backend cap | Map resolved caller intent to `DraftOverride` and consume the resulting `DraftPlan` |
| `inference/skippy/dflash_draft.rs` locates cached drafts, downloads and verifies them | `DraftPlan::prepare` or `DraftPlan::verify_local_file` for an artifact acquired by the host |
| Local load falls back when default attachment fails | `ModelLoadRequest::load_with_default_draft` |

Mesh continues to resolve its own configuration precedence, decide when to
run downloads in the background, show notices and progress, and route split
stages. It can use the selected pin and verified target-layer list when
producing its split plan.

## Shipping a pairing

Update [the bundled index](../../crates/skippy-api/data/dflash-pairings/index.json)
and its `DFLASH_PAIRING_INDEX_SHA256` in the same change. The index is also
the offline bootstrap data, so a Skippy dependency upgrade carries the new
pairing even if the content-addressed Hub copy has yet to be published. A
Hub publisher can upload the same bytes as `index/<sha256>.json` and retain
older pins for existing releases.

The initial Apache-2.0 catalog data comes from the Mesh adapter at the revision
audited for Issue #2. [Pairing index at the source revision](https://github.com/Mesh-LLM/mesh-llm/blob/1ea1ecf754533a6a4fcc0eff8d3f33ff5b73762f/mesh/crates/mesh-llm-skippy-adapter/data/dflash-pairings/index.json)
