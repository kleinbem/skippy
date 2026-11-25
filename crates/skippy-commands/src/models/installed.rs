use crate::models::storage::{
    huggingface_hub_cache_dir, huggingface_identity_for_path, layered_package_layer_count_for_path,
    layered_package_total_bytes_for_path, load_model_usage_record_for_path,
    scan_installed_artifacts_in,
};
use anyhow::Result;
use skippy_model_hf::details::{
    find_remote_catalog_model_exact, installed_model_capabilities, installed_model_display_name,
    installed_model_huggingface_ref, remote_catalog_model_ref,
};
use std::path::Path;

use super::formatters::{InstalledRow, models_formatter};

fn installed_layer_package_count(name: &str, detected_count: Option<usize>) -> Option<usize> {
    detected_count.or_else(|| name.ends_with("-layers").then_some(0))
}

fn build_installed_rows(cache_root: &Path) -> Vec<InstalledRow> {
    scan_installed_artifacts_in(cache_root)
        .into_iter()
        .map(|artifact| {
            let name = artifact.model_ref;
            let path = artifact.path;
            let display_name = installed_model_display_name(&name);
            let catalog_model = find_remote_catalog_model_exact(&name);
            let layer_count =
                installed_layer_package_count(&name, layered_package_layer_count_for_path(&path));
            let model_ref = if layer_count.is_some() {
                name.clone()
            } else if let Some(model) = catalog_model.as_ref() {
                remote_catalog_model_ref(model)
            } else if let Some(identity) = huggingface_identity_for_path(&path) {
                installed_model_huggingface_ref(&identity)
            } else {
                name.clone()
            };
            let show_command = layer_count.is_none().then(|| {
                format!(
                    "{program} models show {model_ref}",
                    program = crate::models::output::program()
                )
            });
            let download_command = layer_count.is_none().then(|| {
                format!(
                    "{program} models download {model_ref}",
                    program = crate::models::output::program()
                )
            });
            let delete_command = format!(
                "{program} models delete {model_ref}",
                program = crate::models::output::program()
            );
            let size = if let Some(bytes) = layered_package_total_bytes_for_path(&path) {
                Some(bytes)
            } else if path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
            {
                Some(crate::models::storage::total_model_bytes(&path))
            } else {
                std::fs::metadata(&path).map(|meta| meta.len()).ok()
            };
            let capabilities = installed_model_capabilities(&name);
            let usage = load_model_usage_record_for_path(&path);
            InstalledRow {
                name: display_name,
                model_ref,
                show_command,
                download_command,
                delete_command,
                path,
                size,
                layer_count,
                catalog_model,
                capabilities,
                managed_by_product: usage.as_ref().is_some_and(|record| record.mesh_managed),
                last_used_at: usage.map(|record| record.last_used_at),
            }
        })
        .collect()
}

pub(super) fn run_model_installed(json_output: bool) -> Result<()> {
    let formatter = models_formatter(json_output);
    let rows = build_installed_rows(&huggingface_hub_cache_dir());
    formatter.render_installed(&rows)
}

#[cfg(test)]
mod tests {
    use super::build_installed_rows;
    use std::path::{Path, PathBuf};

    fn create_cache_repo_file(
        root: &Path,
        repo_id: &str,
        revision: &str,
        relative_file: &str,
        size_bytes: usize,
    ) -> PathBuf {
        let repo_dir = root.join(format!("models--{}", repo_id.replace('/', "--")));
        let refs_dir = repo_dir.join("refs");
        let snapshot_dir = repo_dir.join("snapshots").join(revision);
        std::fs::create_dir_all(&refs_dir).unwrap();
        std::fs::create_dir_all(
            snapshot_dir.join(Path::new(relative_file).parent().unwrap_or(Path::new(""))),
        )
        .unwrap();
        std::fs::write(refs_dir.join("main"), revision).unwrap();

        let path = snapshot_dir.join(relative_file);
        std::fs::write(&path, vec![0u8; size_bytes]).unwrap();
        path
    }

    #[test]
    #[serial_test::serial]
    fn installed_rows_keep_layered_package_ref_and_safe_commands() {
        let _catalog = skippy_model_hf::remote_catalog::set_catalog_entries_for_test(Vec::new());
        let temp = tempfile::tempdir().unwrap();
        create_cache_repo_file(
            temp.path(),
            "meshllm/DeepSeek-V3.2-UD-Q4_K_XL-layers",
            "abcdef1234567890",
            "shared/embeddings.gguf",
            6,
        );
        create_cache_repo_file(
            temp.path(),
            "meshllm/DeepSeek-V3.2-UD-Q4_K_XL-layers",
            "abcdef1234567890",
            "layers/layer-000.gguf",
            9,
        );
        create_cache_repo_file(
            temp.path(),
            "meshllm/DeepSeek-V3.2-UD-Q4_K_XL-layers",
            "abcdef1234567890",
            "layers/layer-001.gguf",
            9,
        );

        let rows = build_installed_rows(temp.path());
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.model_ref, "meshllm/DeepSeek-V3.2-UD-Q4_K_XL-layers");
        assert_eq!(row.layer_count, Some(2));
        assert_eq!(row.show_command, None);
        assert_eq!(row.download_command, None);
        assert_eq!(
            row.delete_command,
            "skippy models delete meshllm/DeepSeek-V3.2-UD-Q4_K_XL-layers"
        );
    }

    #[test]
    #[serial_test::serial]
    fn installed_rows_keep_partial_layered_package_grouped() {
        let _catalog = skippy_model_hf::remote_catalog::set_catalog_entries_for_test(Vec::new());
        let temp = tempfile::tempdir().unwrap();
        let metadata = create_cache_repo_file(
            temp.path(),
            "meshllm/Qwen3-8B-Q4_K_M-layers",
            "abcdef1234567890",
            "shared/metadata.gguf",
            6,
        );

        let rows = build_installed_rows(temp.path());
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.path, metadata);
        assert_eq!(row.model_ref, "meshllm/Qwen3-8B-Q4_K_M-layers");
        assert_eq!(row.layer_count, Some(0));
        assert_eq!(row.show_command, None);
        assert_eq!(row.download_command, None);
        assert_eq!(
            row.delete_command,
            "skippy models delete meshllm/Qwen3-8B-Q4_K_M-layers"
        );
    }
}
