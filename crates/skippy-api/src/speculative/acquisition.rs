//! Local validation and explicitly requested acquisition of a selected draft.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use hf_hub::{RepoTypeModel, progress::Progress};
use skippy_model_artifact::gguf::{scan_gguf_dflash_target_layers, scan_gguf_vocab_token_count};
use skippy_model_hf::{HfModelRepository, store::local::huggingface_repo_folder_name};
use skippy_serving::{DFLASH_STRATEGY, DFlashProposalConfig};

use super::{DraftPlan, DraftProvenance, DraftSource, scan_model_metadata};
use crate::serving::InferenceOptions;

/// Acquisition policy supplied by the host. Both modes prefer a cached
/// immutable revision and verify it before use.
pub enum DraftAcquisition<'a> {
    CacheOnly {
        cache: &'a Path,
    },
    Download {
        repository: &'a HfModelRepository,
        progress: Option<Progress>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum DraftPreparationError {
    Unavailable(String),
    Integrity(String),
    Incompatible(String),
}

impl fmt::Display for DraftPreparationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (kind, message) = match self {
            Self::Unavailable(message) => ("unavailable", message),
            Self::Integrity(message) => ("failed integrity verification", message),
            Self::Incompatible(message) => ("incompatible", message),
        };
        write!(formatter, "DFlash draft {kind}: {message}")
    }
}

impl std::error::Error for DraftPreparationError {}

/// A draft verified against the selected target and, when pinned, its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedDraft {
    config: DFlashProposalConfig,
    provenance: DraftProvenance,
}

impl PreparedDraft {
    pub fn config(&self) -> &DFlashProposalConfig {
        &self.config
    }

    pub fn provenance(&self) -> &DraftProvenance {
        &self.provenance
    }

    /// Install the verified draft into an existing inference plan.
    pub fn apply_to(&self, options: &mut InferenceOptions) {
        options.native_mtp_enabled = false;
        options.native_mtp_draft_model_path = None;
        options.draft_model_path = None;
        options.speculative_window = 0;
        options.adaptive_speculative_window = false;
        options.speculative.native_mtp.enabled = false;
        options.speculative.ngram = None;
        options.speculative.extension = None;
        options.speculative.ngram_fallback_draft = false;
        options.speculative.effective_strategy = DFLASH_STRATEGY.into();
        options.speculative.dflash = Some(self.config.clone());
    }
}

impl DraftPlan {
    /// Look up or explicitly download the exact selected artifact. Missing,
    /// corrupt or incompatible drafts return an error without switching
    /// proposers. A corrupt cache entry is left for the cache owner to repair.
    pub async fn prepare(
        &self,
        target_path: Option<&Path>,
        policy: DraftAcquisition<'_>,
    ) -> Result<PreparedDraft, DraftPreparationError> {
        let path = self.acquire(policy).await?;
        let target_path = target_path.map(Path::to_path_buf);
        let plan = self.clone();
        tokio::task::spawn_blocking(move || plan.verify_local_file(&path, target_path.as_deref()))
            .await
            .map_err(|error| DraftPreparationError::Unavailable(error.to_string()))?
    }

    /// Verify a file acquired by the caller. The caller can supply the target
    /// metadata file to check vocabulary size before runtime attachment.
    pub fn verify_local_file(
        &self,
        path: &Path,
        target_path: Option<&Path>,
    ) -> Result<PreparedDraft, DraftPreparationError> {
        if !path.is_file() {
            return Err(DraftPreparationError::Unavailable(
                path.display().to_string(),
            ));
        }
        match &self.source {
            DraftSource::Pinned(draft) => {
                let bytes = path.metadata().map_err(unavailable)?.len();
                if bytes != draft.bytes {
                    return Err(DraftPreparationError::Integrity(format!(
                        "{} has {bytes} bytes, expected {}",
                        path.display(),
                        draft.bytes
                    )));
                }
                let sha256 = crate::package::file_sha256(path).map_err(unavailable)?;
                if !sha256.eq_ignore_ascii_case(&draft.sha256) {
                    return Err(DraftPreparationError::Integrity(format!(
                        "{} has SHA-256 {sha256}, expected {}",
                        path.display(),
                        draft.sha256
                    )));
                }
            }
            DraftSource::Local(selected) if path != selected => {
                return Err(DraftPreparationError::Incompatible(
                    "file differs from the explicit override".into(),
                ));
            }
            DraftSource::Local(_) => {}
        }

        let layers = self.check_header(path, target_path)?;
        Ok(PreparedDraft {
            config: DFlashProposalConfig {
                draft_model_path: path.into(),
                max_draft_tokens: Some(self.max_draft_tokens as usize),
                target_layers: layers,
                context_window: None,
                feature_encoding: None,
            },
            provenance: self.provenance.clone(),
        })
    }

    async fn acquire(
        &self,
        policy: DraftAcquisition<'_>,
    ) -> Result<PathBuf, DraftPreparationError> {
        let draft = match &self.source {
            DraftSource::Pinned(draft) => draft,
            DraftSource::Local(path) => return Ok(path.clone()),
        };
        let cache = match &policy {
            DraftAcquisition::CacheOnly { cache } => cache,
            DraftAcquisition::Download { repository, .. } => repository.cache_dir(),
        };
        let path = cache
            .join(huggingface_repo_folder_name(&draft.repo, RepoTypeModel))
            .join("snapshots")
            .join(&draft.revision)
            .join(&draft.file);
        if path.is_file() {
            return Ok(path);
        }
        match policy {
            DraftAcquisition::CacheOnly { .. } => Err(DraftPreparationError::Unavailable(
                path.display().to_string(),
            )),
            DraftAcquisition::Download {
                repository,
                progress,
            } => repository
                .download_file_with_progress(&draft.repo, &draft.revision, &draft.file, progress)
                .await
                .map_err(unavailable),
        }
    }

    fn check_header(
        &self,
        path: &Path,
        target_path: Option<&Path>,
    ) -> Result<Vec<u32>, DraftPreparationError> {
        let meta = scan_model_metadata(path)
            .ok_or_else(|| incompatible("draft has no readable GGUF metadata"))?;
        if meta.architecture != DFLASH_STRATEGY {
            return Err(incompatible("draft GGUF architecture must be dflash"));
        }
        if meta.embedding_size != self.target.embedding_length {
            return Err(incompatible(format!(
                "draft hidden width {} differs from target width {}",
                meta.embedding_size, self.target.embedding_length
            )));
        }
        let layers = scan_gguf_dflash_target_layers(path)
            .filter(|layers| !layers.is_empty())
            .ok_or_else(|| incompatible("draft specifies no target layers"))?;
        // The output head's input may be named by the executable layer count.
        if layers.iter().any(|&layer| layer > self.target.layer_count) {
            return Err(incompatible(format!(
                "draft target layers {layers:?} exceed target depth {}",
                self.target.layer_count
            )));
        }
        if let Some(declared) = &self.target_layers
            && &layers != declared
        {
            return Err(incompatible(format!(
                "draft target layers {layers:?} differ from package layers {declared:?}"
            )));
        }
        if let (Some(draft), Some(target)) = (
            scan_gguf_vocab_token_count(path),
            target_path.and_then(|target| {
                scan_gguf_vocab_token_count(target)
                    .or_else(|| scan_gguf_vocab_token_count(&target.join("shared/metadata.gguf")))
            }),
        ) && draft != target
        {
            return Err(incompatible(format!(
                "draft vocabulary has {draft} tokens, target has {target}"
            )));
        }
        Ok(layers)
    }
}

fn unavailable(error: impl fmt::Display) -> DraftPreparationError {
    DraftPreparationError::Unavailable(error.to_string())
}

fn incompatible(message: impl Into<String>) -> DraftPreparationError {
    DraftPreparationError::Incompatible(message.into())
}
