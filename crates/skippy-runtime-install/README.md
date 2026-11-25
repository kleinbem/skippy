# skippy-runtime-install

`skippy-runtime-install` owns the public native runtime installation flow for
callers supplying a runtime release and catalog policy. Standalone defaults
belong to Skippy; embedding applications can supply their own locations.

It provides:

- release manifest loading from a file, URL, bundled runtime directory, or the
  explicitly configured release catalog
- compatible runtime resolution for the requested release
- cache path selection and installed runtime discovery
- checksum enforcement before installing downloaded archives
- download progress callbacks for SDK and CLI callers
- stale runtime pruning through `NativeRuntimeCache`

Native runtime versions must match the requested release exactly. The
installer rejects incompatible release manifest entries instead of building
native code through Cargo.

## Explicit catalogs

`NativeRuntimeInstallOptions::catalog` and `NativeRuntimeManifestOptions::catalog`
carry a `NativeRuntimeCatalog`: a release URL root and an optional release that
should track its latest catalog. Other releases use pinned version URLs. The
loader uses these inputs; it does not infer release channels from host build
metadata. Explicit manifest files and URLs, environment overrides and download
permissions keep their existing precedence.

For an independently supplied Skippy runtime catalog:

```rust
use skippy_runtime_install::{NativeRuntimeCatalog, NativeRuntimeInstallOptions};

let options = NativeRuntimeInstallOptions::new(
    "1.2.3",
    NativeRuntimeCatalog {
        release_tags: Default::default(),
        releases_url: "https://example.invalid/skippy/releases".to_string(),
        rolling_release: None,
    },
);
```

The example URL is illustrative. Options have no `Default` implementation:
callers must provide the release and catalog. Bundle discovery takes an explicit
release. Standalone settings use `SKIPPY_NATIVE_RUNTIME_BUNDLE_DIR`,
`SKIPPY_NATIVE_RUNTIME_CACHE_DIR`, and `SKIPPY_NATIVE_RUNTIME_MANIFEST_URL`.
Default caches live under the platform cache directory's `skippy/native-runtimes`;
installed bundles are discovered beside the executable or under
`lib/skippy[/<release>]/native-runtimes`.
Installer options, runtime manifests, and cache inventory name runtime identity
`release_version`. Normal readers accept schema-2 metadata and the published
Mesh-era format with `mesh_version` and no `schema_version`. The legacy version
is required and mapped to internal `release_version`; missing versions never
inherit a fallback. Runtime payload checksums and ABI eligibility still apply.

## Migrating callers

The package replaces `mesh-llm-runtime-install`; Rust imports use
`skippy_runtime_install`. Construct options with `new(release, catalog)` instead
of `Default`, and use `release_version` instead of the former `mesh_version`
option field. Bundle discovery and `discover_local_native_runtimes` now require
an explicit release argument so discovery and installation select the same
version. These are source API changes.

Embedding applications supply their product policy explicitly. Valid existing
cache entries remain readable when their directories are supplied. The explicit
importer can copy verified bytes and write current metadata to a separate cache
without changing its source.
Artifact names and environment names are separate migration surfaces.

## Private release downloads

The native HTTP downloader supports public releases. For a private release,
download it with an authorized `gh` login, verify its checksum, and extract the CLI
and runtime archives into one installation directory so `native-runtimes/` sits
beside `skippy` (or `skippy.exe`). Use the release's documented asset names:

```sh
gh release download vVERSION --repo Mesh-LLM/skippy --dir downloads
```

An extracted runtime can also be imported into the cache explicitly:

```sh
skippy --runtime-bundle /absolute/path/to/native-runtimes runtime install
```

This local-bundle path verifies payload hashes and native ABI compatibility.
An embedding application or managed distribution can instead supply an explicit
manifest file or its own accessible manifest URL; those sources retain their
existing precedence. Never put GitHub tokens into a URL or a runtime manifest.

## Example

```rust,no_run
use skippy_runtime_install::{
    NativeRuntimeCatalog, NativeRuntimeInstallOptions, RuntimeSelection, install_native_runtime,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let outcome = install_native_runtime(NativeRuntimeInstallOptions {
        selection: RuntimeSelection::Recommended,
        ..NativeRuntimeInstallOptions::new("1.2.3", NativeRuntimeCatalog {
            release_tags: Default::default(),
            releases_url: "https://example.invalid/skippy/releases".to_string(),
            rolling_release: None,
        })
    })
    .await?;

    println!("installed runtime at {}", outcome.runtime.path.display());
    Ok(())
}
```

Catalog `release_tags` explicitly maps runtime releases to publication tags. Tags
include their own prefix (for example `v99.0.0`); unmapped releases retain the
`v<release>` convention. A matching `rolling_release` takes precedence. This
keeps runtime identity independent of the product hosting its release assets.

Explicit cache migration can use `import_runtime_copy` after decoding source
metadata into a typed manifest. It verifies payloads, copies into a newly claimed
entry, writes current metadata, and verifies the result through the normal
reader. It never hardlinks, merges or overwrites an existing entry. An identical
verified destination is idempotent; dry-run validates without creating paths.
Legacy decoding and user-facing import commands are separate callers of this
primitive; it does not scan or migrate caches automatically.

`import_legacy_runtime_cache` is the explicit decoder for an unversioned
`<release>/<runtime-id>/manifest.json` cache. It checks directory identities,
reports missing/unknown entries as skips, and continues past per-runtime failures.
ABI mismatches produce preservation warnings; imports do not override resolver
eligibility. Callers must return failure when the report's `has_failures()` is
true. Generation-marked manifests are not accepted by the legacy reader.
Normal discovery reads valid legacy entries directly and does not call this API.
