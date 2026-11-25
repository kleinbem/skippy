//! A downloaded runtime must be the one its catalog entry named.

use anyhow::{Result, ensure};
use skippy_native_runtime::NativeRuntimeArtifact;

/// Refuses a downloaded runtime whose own manifest names a different runtime
/// than the catalog entry it was selected by. The archive checksum proves the
/// bytes are the published ones; this proves they are the runtime that was
/// resolved for this host. Transport fields (URL, checksum) are not compared.
pub(crate) fn ensure_downloaded_runtime_matches_catalog_entry(
    selected: &NativeRuntimeArtifact,
    downloaded: &NativeRuntimeArtifact,
) -> Result<()> {
    let fields = [
        ("id", selected.id == downloaded.id),
        (
            "release_version",
            selected.release_version == downloaded.release_version,
        ),
        ("skippy_abi", selected.skippy_abi == downloaded.skippy_abi),
        ("platform", selected.platform == downloaded.platform),
        ("backend", selected.backend == downloaded.backend),
    ];
    for (field, matches) in fields {
        ensure!(
            matches,
            "downloaded native runtime {} does not match its catalog entry {}: {field} differs",
            downloaded.id,
            selected.id
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skippy_native_runtime::{NativeRuntimeBackend, NativeRuntimeBackendKind};

    fn artifact() -> NativeRuntimeArtifact {
        NativeRuntimeArtifact {
            id: "skippy-runtime-linux-x86_64-cpu".to_string(),
            release_version: Some("1.2.3".to_string()),
            skippy_abi: "0.1.0".to_string(),
            platform: skippy_native_runtime::NativeRuntimePlatform {
                os: "linux".to_string(),
                arch: "x86_64".to_string(),
                target: None,
                min_glibc: None,
            },
            backend: NativeRuntimeBackend::cpu(),
            rank: 0,
            libraries: vec!["lib/libllama.so".to_string()],
            files: Default::default(),
            tools: Default::default(),
            url: None,
            sha256: None,
            signature: None,
        }
    }

    #[test]
    fn a_runtime_identical_to_its_catalog_entry_is_accepted() {
        ensure_downloaded_runtime_matches_catalog_entry(&artifact(), &artifact()).unwrap();
    }

    #[test]
    fn transport_fields_are_not_compared() {
        let mut selected = artifact();
        selected.url = Some("https://example.invalid/runtime.tar.gz".to_string());
        selected.sha256 = Some("0".repeat(64));
        ensure_downloaded_runtime_matches_catalog_entry(&selected, &artifact()).unwrap();
    }

    #[test]
    fn a_runtime_differing_from_its_catalog_entry_is_refused() {
        type Change = fn(&mut NativeRuntimeArtifact);
        let changes: [(&str, Change); 5] = [
            ("id", |runtime| runtime.id.push_str("-other")),
            ("release_version", |runtime| {
                runtime.release_version = Some("0.0.1".to_string());
            }),
            ("skippy_abi", |runtime| {
                runtime.skippy_abi = "0.0.0".to_string();
            }),
            ("platform", |runtime| {
                runtime.platform.arch = "aarch64".to_string();
            }),
            ("backend", |runtime| {
                runtime.backend.kind = NativeRuntimeBackendKind::Vulkan;
            }),
        ];
        for (field, change) in changes {
            let mut downloaded = artifact();
            change(&mut downloaded);
            let error = ensure_downloaded_runtime_matches_catalog_entry(&artifact(), &downloaded)
                .expect_err(field)
                .to_string();
            assert!(error.contains(&format!("{field} differs")), "{error}");
        }
    }
}
