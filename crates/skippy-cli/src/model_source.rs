//! Resolve serving model references into local weights and projector paths.

use std::path::{Path, PathBuf};

use anyhow::Result;
use skippy_commands::console;

use crate::cli::ServeOpenAiArgs;

pub(crate) async fn resolve(args: &mut ServeOpenAiArgs) -> Result<()> {
    if let Some(model) = args.model.as_deref() {
        console::status(&format!("🔎 Checking local model cache for {model}"))?;
        let existing = installed_model_path(model);
        let catalog_model = (!Path::new(model).exists())
            .then(|| {
                let exact = if existing.is_some() && model.contains('/') {
                    skippy_model_hf::remote_catalog::find_loaded_model_exact(model)
                } else {
                    skippy_model_hf::remote_catalog::find_model_exact(model)
                };
                exact.or_else(|| {
                    (!model.contains('/'))
                        .then(|| skippy_model_hf::remote_catalog::resolve_model_download(model))
                        .flatten()
                        .and_then(|resolved| {
                            skippy_model_hf::remote_catalog::matching_model_for_huggingface(
                                &resolved.repo,
                                resolved.revision.as_deref(),
                                &resolved.file,
                            )
                        })
                })
            })
            .flatten();
        let catalog_name = catalog_model.as_ref().map(|entry| entry.name.clone());
        let (path, downloaded_projector) = if let Some(local_path) = existing {
            console::status("📂 Using cached model")?;
            (local_path, None)
        } else {
            let cache = skippy_commands::models::model_cache_dir();
            let downloaded =
                skippy_commands::models::download_model(&cache, model, None, None).await?;
            (downloaded.load_path, downloaded.projector_path)
        };
        args.model_path = Some(path);
        if args.mmproj.is_none() {
            args.mmproj = downloaded_projector;
        }
        if args.mmproj.is_none()
            && let Some(asset) = catalog_model.and_then(|entry| entry.mmproj)
        {
            let projector_ref = asset.download_ref();
            let cache = skippy_commands::models::model_cache_dir();
            args.mmproj = Some(
                skippy_commands::models::download_model(&cache, &projector_ref, None, None)
                    .await?
                    .primary_path,
            );
        }
        if !Path::new(model).exists() {
            args.model_id.get_or_insert_with(|| model.to_owned());
        }
        // Only a catalog entry is named on analytics events, by its catalog
        // name, whatever the model is served as. Anything else stays
        // `redacted`.
        if let (Some(catalog_name), Some(served)) = (catalog_name, args.model_id.as_deref()) {
            skippy_analytics::models::register(
                served,
                skippy_analytics::models::ModelIdentity {
                    model: skippy_analytics::Label::sanitize_or_redact(&catalog_name),
                    ..skippy_analytics::models::ModelIdentity::redacted()
                },
            );
        }
    }
    Ok(())
}

fn installed_model_path(model: &str) -> Option<PathBuf> {
    let path = Path::new(model);
    if path.is_file() || path.is_dir() {
        return Some(path.to_path_buf());
    }
    // Mesh prefers catalog resolution for short curated names, then falls
    // back to local cache stems. Exact Hub refs can use the cache directly.
    if model.contains('/') || skippy_model_hf::remote_catalog::find_model_exact(model).is_none() {
        let cached = skippy_model_hf::store::local::find_model_path(model);
        if cached.exists() {
            return Some(cached);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_local_model_path_is_used_without_hub_resolution() {
        let temp = tempfile::tempdir().unwrap();
        let model = temp.path().join("model.gguf");
        std::fs::write(&model, b"GGUF").unwrap();
        assert_eq!(installed_model_path(model.to_str().unwrap()), Some(model));
    }
}
