//! Installed model artifacts in the shared Hugging Face cache.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::Serialize;
use skippy_model_artifact::ModelFormat;

use super::local;

#[derive(Clone, Debug, Serialize)]
pub struct InstalledModelArtifact {
    pub model_ref: String,
    pub path: PathBuf,
    pub load_path: PathBuf,
    pub format: ModelFormat,
    pub size_bytes: u64,
}

/// Scan named Hub snapshots and directly cached GGUFs without contacting the Hub.
pub fn scan_installed_artifacts_in(cache_root: &Path) -> Vec<InstalledModelArtifact> {
    let mut paths = local::direct_hf_cache_root_gguf_paths(cache_root);
    let Ok(entries) = std::fs::read_dir(cache_root) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("models--") {
            continue;
        }
        let snapshots = entry.path().join("snapshots");
        collect_model_files(&snapshots, &mut paths);
    }
    paths.sort();
    let mut installed = BTreeMap::new();
    for path in paths {
        let Some(format) = model_format_for_primary(&path) else {
            continue;
        };
        let model_ref = local::model_ref_for_path(&path);
        let size_bytes = std::fs::metadata(&path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let load_path = path.clone();
        installed
            .entry(model_ref.clone())
            .or_insert(InstalledModelArtifact {
                model_ref,
                path,
                load_path,
                format,
                size_bytes,
            });
    }
    installed.into_values().collect()
}

pub fn scan_installed_artifacts() -> Vec<InstalledModelArtifact> {
    scan_installed_artifacts_in(&crate::huggingface_hub_cache_dir())
}

fn collect_model_files(dir: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            collect_model_files(&path, paths);
        } else if path.is_file() {
            paths.push(path);
        }
    }
}

fn model_format_for_primary(path: &Path) -> Option<ModelFormat> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    (name.ends_with(".gguf")
        && !name.starts_with("mmproj")
        && skippy_model_ref::split_gguf_shard_info(&name).is_none_or(|shard| shard.part == "00001"))
    .then_some(ModelFormat::Gguf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_gguf_without_checkpoints_or_sidecars() {
        let temp = tempfile::tempdir().unwrap();
        let snapshot = temp.path().join("models--org--repo/snapshots/rev");
        std::fs::create_dir_all(&snapshot).unwrap();
        for file in [
            "model-00001-of-00002.safetensors",
            "model-00002-of-00002.safetensors",
            "model-Q4_K_M.gguf",
            "mmproj.gguf",
        ] {
            std::fs::write(snapshot.join(file), b"weight").unwrap();
        }
        let found = scan_installed_artifacts_in(temp.path());
        assert_eq!(found.len(), 1);
        assert!(found.iter().any(|entry| entry.format == ModelFormat::Gguf));
    }
}
