# Configuration

Skippy's standalone configuration is owned by the `skippy` command. It does
not read MeshLLM configuration files or depend on a plugin host.

## Serving settings

Use `skippy serve --help` for the complete option list and
`skippy serve --print-effective-config` to inspect the resolved launch without
loading a model. Settings resolve in this order:

1. command-line options;
2. `SKIPPY_SERVE_*` environment variables;
3. a versioned TOML file supplied with `--settings`;
4. Skippy defaults.

The [serving settings reference](SERVING_SETTINGS.md) documents every TOML
section, environment mapping, default, and mode constraint.

## Models and publication

Hugging Face cache and authentication follow the standard `HF_HOME`,
`HF_HUB_CACHE`, `HUGGINGFACE_HUB_CACHE`, and token settings. Remote curated
catalog reads use `meshllm/catalog` by default.
`SKIPPY_MODEL_CATALOG=namespace/name` selects another Hugging Face dataset
repository with the same catalog format.
Layer-package publication also requires `SKIPPY_PACKAGE_BUCKET` and an explicit
authenticated destination.

## Native runtimes

The CLI discovers verified runtimes beside the executable and in the platform
cache directory. Override discovery with `--runtime-bundle` or
`SKIPPY_NATIVE_RUNTIME_BUNDLE_DIR`; override the cache with
`--runtime-cache` or `SKIPPY_NATIVE_RUNTIME_CACHE_DIR`.

Private release assets are not fetched anonymously. Download a complete bundle
with an authenticated GitHub client, verify it, and point Skippy at the local
runtime bundle.

## Repository automation

Build, test, and release settings are expressed through the root `Justfile` and
shell and Python commands under `scripts/`. See [CONTRIBUTING.md](../CONTRIBUTING.md).
