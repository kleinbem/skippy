# Releasing Skippy

Releases are published on GitHub in `Mesh-LLM/skippy` by
the **Release** workflow from `main`. The workflow writes the version commit
itself, so no version PR or hand-made tag is needed.

A release runs every day at 17:00 UTC with the next patch version: the patch
after the newest `vX.Y.*` tag for the workspace's `X.Y`. A day on which `main`
has no commits since its last release publishes nothing. To start a new `X.Y`
series, for example when the native ABI or another compatibility contract
changes, merge a PR that sets the workspace version to `X.Y.0`; the next release
uses that version unchanged.

Dispatch by hand with `just release vX.Y.Z` for a chosen version, or
`just release vX.Y.Z true` for a canary that publishes nothing. A prerelease
version such as `v1.2.3-rc.1` creates a prerelease. Keep the version tag
namespace protected from movement and deletion.

The workspace version is also the native runtime release identity and cache
directory. Every release therefore makes hosts download its runtime, and changed
runtime contents can never ship under an earlier release's identity.

`release.yml` stages the version commit, then calls the same `ci.yml` used by PRs and main in release CUDA
architecture mode. It requires all three enabled hosts and eight complete
bundles. Windows targets remain declared but disabled until infrastructure is
available:

| Platform | Bundles |
| --- | --- |
| macOS arm64 | CPU, Metal |
| Linux x86_64 | CPU, CUDA 12, CUDA 13 |
| Linux arm64 | CPU, CUDA 12, CUDA 13 |

Each `skippy-<tag>-<variant>.tar.gz` contains the backend-neutral CLI, its verified
native runtime under `native-runtimes/`, licenses and provenance. SHA-256 sidecars
and `release.json` bind the complete roster and source. Native manifests include
ABI and llama recipe identities. CUDA release products compile and advertise the
complete architecture lists from `ci/targets.json`; release preparation rejects
native-only CUDA products.

The release also publishes `native-runtimes.json`, the catalog a host reads to
download a runtime (`skippy runtime install`, and products that embed Skippy).
Each entry names one of the release archives and its SHA-256, and `release.json`
binds the catalog. Only the `skippy-*` archives, the catalog, `release.json` and
their checksums are published. No public registry or package-manager publication
happens as a side effect.

The publisher uploads to a source-bound draft, downloads every asset, checks all
hashes and the exact roster, then publishes. A partial draft may be resumed only
when existing assets have matching GitHub SHA-256 identities. Reruns upload only
missing assets; they cannot overwrite a published release or move a tag. A
conflicting draft is an error requiring investigation. Preserve diagnostics.

After the GitHub release is published, the documentation job downloads the
exact `release-docs-site` artifact built by the shared validation workflow and
deploys it to `skippy.sh`. It does not rebuild the site. Configure the
`SKIPPY_DOCS_CLOUDFLARE_API_TOKEN` repository secret as described in
[CI prerequisites](ci/README.md) before creating a version tag.

No Apple Developer ID/notarization or Windows Authenticode identity is configured
by this extraction. If platform signing is added, it must precede final hashes
and require fresh verification; never reuse obsolete checksums after signing.

Download a release with the GitHub CLI. Sign in to access private releases.

```sh
gh release download v1.2.3 --repo Mesh-LLM/skippy \
  --pattern 'skippy-v1.2.3-linux-x86_64-cpu.tar.gz*'
sha256sum --check skippy-v1.2.3-linux-x86_64-cpu.tar.gz.sha256
tar -xzf skippy-v1.2.3-linux-x86_64-cpu.tar.gz
./skippy --version
```

Use `shasum -a 256 --check` on macOS or `Get-FileHash -Algorithm SHA256` on
Windows to compare against the sidecar before extracting. Keep `native-runtimes`
adjacent to the executable. Serving then works without a GitHub credential.
Automatic unauthenticated download from a private repository is not supported.

[CI prerequisites and bring-up gates](ci/README.md) cover runner/toolchain and
hardware qualification. Workflow syntax or a successful local CLI check does not
prove that the full release matrix has executed.
