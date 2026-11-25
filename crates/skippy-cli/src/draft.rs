//! Resolve standalone defaults through the same draft catalog as embedding hosts.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, ensure};
use skippy_api::{
    package::inspection::{StagePackageInfo, inspect_local_stage_package},
    serving::LocalOpenAiOptions,
    speculative::{
        DraftAcquisition, DraftBackend, DraftCatalog, DraftOverride, DraftPlan, DraftTarget,
        DraftUnavailable, PreparedDraft,
        download_progress::{DownloadEvent, ProgressEvent, ProgressHandler},
        scan_model_metadata,
    },
};
use skippy_commands::console;
use skippy_model_hf::HfModelRepository;
use skippy_protocol::StageConfig;

use crate::serve_settings::ServeSettings;

pub(super) async fn prepare_default_draft(
    options: &mut LocalOpenAiOptions,
    settings: &ServeSettings,
    inspect_only: bool,
) -> Option<PreparedDraft> {
    if !skippy_runtime::native_runtime_loaded()
        || !settings.allows_default_draft()
        || options.tuning.draft_model_path.is_some()
        || options.tuning.native_mtp_draft_model_path.is_some()
        || options.config.stage_index != 0
        || options.config.layer_start != 0
        || options.config.upstream.is_some()
        || options.config.downstream.is_some()
    {
        return None;
    }
    // Use the devices the runtime actually registered, including CPU-only
    // builds on machines with a GPU. Draft offload defaults to target offload.
    let devices = skippy_runtime::backend_devices().ok()?;
    let backend = DraftBackend::for_native_placement(
        &devices,
        options
            .tuning
            .draft_n_gpu_layers
            .unwrap_or(options.config.n_gpu_layers),
        options
            .config
            .selected_device
            .as_ref()
            .map(|d| d.backend_device.as_str()),
    )?;
    let cache = skippy_model_hf::huggingface_hub_cache_dir();
    let (catalog, _) = DraftCatalog::from_cache(&cache);
    let package = match local_package(&options.config) {
        Ok(package) => package,
        Err(error) => {
            options.keep_native_speculation_default();
            if !inspect_only {
                let _ = console::status(&format!("Default draft skipped: {error:#}"));
            }
            return None;
        }
    };
    let selection = select_for_stage(
        &catalog,
        &options.config,
        options.model_id.as_deref(),
        package.as_ref(),
        backend,
    );
    let package_decides = package.as_ref().is_some_and(|package| {
        package
            .generation
            .as_ref()
            .is_some_and(|generation| generation.speculative_decoding.is_some())
    });
    if selection.is_ok() || package_decides {
        options.keep_native_speculation_default();
    }
    let (plan, target_path) = selection.ok()?;
    let repository = (!inspect_only && !hub_offline())
        .then(|| HfModelRepository::builder().cache_dir(&cache).build().ok())
        .flatten();
    let policy = match &repository {
        Some(repository) => DraftAcquisition::Download {
            repository,
            progress: Some(DraftProgress::default().into()),
        },
        None => DraftAcquisition::CacheOnly { cache: &cache },
    };
    match plan.prepare(Some(&target_path), policy).await {
        Ok(draft) => Some(draft),
        Err(error) => {
            if !inspect_only {
                let _ = console::status(&format!("Default draft skipped: {error}"));
            }
            None
        }
    }
}

fn hub_offline() -> bool {
    std::env::var("HF_HUB_OFFLINE").is_ok_and(|value| {
        matches!(
            value.to_ascii_lowercase().as_str(),
            "1" | "on" | "yes" | "true"
        )
    })
}

fn local_package(stage: &StageConfig) -> Result<Option<StagePackageInfo>> {
    let Some(reference) = stage.package_ref.as_deref() else {
        return Ok(None);
    };
    let path = Path::new(reference);
    let root = if path.join("model-package.json").is_file() {
        path
    } else if reference.starts_with("hf://")
        || stage.load_mode == skippy_protocol::LoadMode::ArtifactSlice
    {
        stage
            .source_model_path
            .as_deref()
            .and_then(|source| {
                Path::new(source)
                    .ancestors()
                    .find(|path| path.join("model-package.json").is_file())
            })
            .context("no local package manifest for the configured stage")?
    } else {
        return Ok(None);
    };
    let package = inspect_local_stage_package(reference, &root.to_string_lossy(), None)?;
    ensure!(
        stage
            .manifest_sha256
            .as_ref()
            .is_none_or(|digest| digest == &package.manifest_sha256),
        "local package manifest digest differs from the stage"
    );
    Ok(Some(package))
}

fn select_for_stage(
    catalog: &DraftCatalog,
    stage: &StageConfig,
    name: Option<&str>,
    package: Option<&StagePackageInfo>,
    backend: DraftBackend,
) -> Result<(DraftPlan, PathBuf), DraftUnavailable> {
    let (target_path, metadata) = [
        package.map(|package| package.source_model_path.as_str()),
        stage.source_model_path.as_deref(),
        stage.model_part_paths.first().map(String::as_str),
        stage.model_path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(|path| {
        let path = Path::new(path);
        scan_model_metadata(path).map(|metadata| (path, metadata))
    })
    .ok_or(DraftUnavailable::TargetMetadataUnavailable)?;
    let plan = catalog.resolve(
        DraftTarget {
            metadata: Some(&metadata),
            name: name.or_else(|| package.map(|package| package.model_id.as_str())),
            package_generation: package.and_then(|package| package.generation.as_ref()),
        },
        backend,
        DraftOverride::Auto,
    )?;
    Ok((plan, target_path.into()))
}

#[derive(Default)]
struct DraftProgress(Mutex<Option<u64>>);

impl ProgressHandler for DraftProgress {
    fn on_progress(&self, event: &ProgressEvent) {
        let (current, total) = match event {
            ProgressEvent::Download(DownloadEvent::Progress { files }) => {
                let Some(file) = files.last() else { return };
                (file.bytes_completed, file.total_bytes)
            }
            ProgressEvent::Download(DownloadEvent::AggregateProgress {
                bytes_completed,
                total_bytes,
                ..
            }) => (*bytes_completed, *total_bytes),
            _ => return,
        };
        if total == 0 {
            return;
        }
        let percent = current.min(total).saturating_mul(100) / total;
        if let Ok(mut last) = self.0.lock()
            && *last != Some(percent)
        {
            *last = Some(percent);
            let _ = console::progress("DFlash draft download", current, total);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skippy_model_artifact::gguf::GgufCompactMeta;
    use skippy_runtime::{BackendDevice, BackendDeviceType};

    fn device(name: &str, device_type: BackendDeviceType) -> BackendDevice {
        BackendDevice {
            name: name.into(),
            description: None,
            device_id: None,
            memory_free: 0,
            memory_total: 0,
            device_type,
            caps: 0,
        }
    }

    #[test]
    fn standalone_and_embedding_choose_the_same_plan_on_actual_placements() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("qwen.gguf");
        write_qwen35_a3b_header(&path);
        let stage = StageConfig {
            source_model_path: Some(path.to_string_lossy().into_owned()),
            ..StageConfig::default()
        };
        let metadata = GgufCompactMeta {
            architecture: "qwen35moe".into(),
            embedding_size: 2048,
            layer_count: 41,
            nextn_predict_layers: 1,
            expert_count: 256,
            ..Default::default()
        };
        let devices = [
            device("CPU", BackendDeviceType::Cpu),
            device("MTL0", BackendDeviceType::IntegratedGpu),
            device("CUDA0", BackendDeviceType::Gpu),
        ];
        let catalog = DraftCatalog::default();
        for (device, n_gpu_layers, metal) in
            [("MTL0", -1, true), ("CUDA0", -1, false), ("MTL0", 0, false)]
        {
            let backend =
                DraftBackend::for_native_placement(&devices, n_gpu_layers, Some(device)).unwrap();
            assert_eq!(backend.metal, metal);
            let standalone =
                select_for_stage(&catalog, &stage, None, None, backend).map(|(plan, _)| plan);
            let embedded = catalog.resolve(
                DraftTarget {
                    metadata: Some(&metadata),
                    name: None,
                    package_generation: None,
                },
                backend,
                DraftOverride::Auto,
            );
            assert_eq!(standalone, embedded);
            if backend.automatic_dflash {
                assert_eq!(
                    standalone.unwrap().max_draft_tokens(),
                    if metal { 6 } else { 7 }
                );
            } else {
                assert_eq!(
                    standalone,
                    Err(DraftUnavailable::AutomaticDFlashUnavailable)
                );
            }
        }
        assert!(DraftBackend::for_native_placement(&devices, -1, None).is_none());
        assert!(
            !DraftBackend::for_native_placement(&devices[..1], -1, None)
                .unwrap()
                .metal
        );
    }

    fn write_qwen35_a3b_header(path: &Path) {
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&5u64.to_le_bytes());
        let string = |bytes: &mut Vec<u8>, value: &str| {
            bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
            bytes.extend_from_slice(value.as_bytes());
        };
        string(&mut bytes, "general.architecture");
        bytes.extend_from_slice(&8u32.to_le_bytes());
        string(&mut bytes, "qwen35moe");
        for (key, value) in [
            ("qwen35moe.block_count", 41u32),
            ("qwen35moe.nextn_predict_layers", 1),
            ("qwen35moe.embedding_length", 2048),
            ("qwen35moe.expert_count", 256),
        ] {
            string(&mut bytes, key);
            bytes.extend_from_slice(&4u32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(path, bytes).unwrap();
    }
}
