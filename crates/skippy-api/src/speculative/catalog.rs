//! The release-pinned catalog and its offline fallback.

use std::{fs, path::Path, sync::LazyLock};

use anyhow::{Context, Result, ensure};
use hf_hub::{RepoTypeModel, progress::Progress};
use sha2::{Digest, Sha256};
use skippy_model_hf::{HfModelRepository, store::local::huggingface_repo_folder_name};
use skippy_package_format::{DFlashPairing, DFlashPairingIndex};

use super::DraftProvenance;

/// Existing publication source for the release-pinned index.
pub const DFLASH_PAIRING_INDEX_REPO: &str = "meshllm/dflash-pairings";
/// SHA-256 of the shipped index bytes, also used as the Hub filename.
pub const DFLASH_PAIRING_INDEX_SHA256: &str =
    "d3c66d8d895b504538a0c5f3e75caef1b95324543aaeb98a628feff9464d1e11";
const BOOTSTRAP: &[u8] = include_bytes!("../../data/dflash-pairings/index.json");

static BOOTSTRAP_INDEX: LazyLock<DFlashPairingIndex> = LazyLock::new(|| {
    verify_index(BOOTSTRAP, DFLASH_PAIRING_INDEX_SHA256)
        .expect("valid bundled DFlash pairing index")
});

/// The status of the external index when opening a catalog from cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DraftIndexStatus {
    Verified,
    Missing,
    Invalid(String),
}

/// An immutable pairing catalog that can be shared across concurrent selections.
#[derive(Debug, Default)]
pub struct DraftCatalog {
    index: Option<(DFlashPairingIndex, String)>,
}

impl DraftCatalog {
    /// Verify the exact index bytes against a digest from trusted release metadata.
    /// A verified index is authoritative, including when it contains no match.
    pub fn from_pinned_index(bytes: &[u8], sha256: &str) -> Result<Self> {
        Ok(Self {
            index: Some((verify_index(bytes, sha256)?, sha256.to_ascii_lowercase())),
        })
    }

    /// Look for the shipped index in the supplied Hugging Face cache. Try every
    /// cached snapshot before falling back to the bundled index. Performs local I/O.
    pub fn from_cache(cache: &Path) -> (Self, DraftIndexStatus) {
        let snapshots = cache
            .join(huggingface_repo_folder_name(
                DFLASH_PAIRING_INDEX_REPO,
                RepoTypeModel,
            ))
            .join("snapshots");
        let mut status = DraftIndexStatus::Missing;
        let entries = match fs::read_dir(&snapshots) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), status);
            }
            Err(error) => {
                return (
                    Self::default(),
                    DraftIndexStatus::Invalid(error.to_string()),
                );
            }
        };
        let file = index_file();
        for entry in entries {
            let result = entry.and_then(|entry| fs::read(entry.path().join(&file)));
            match result {
                Ok(bytes) => match Self::from_pinned_index(&bytes, DFLASH_PAIRING_INDEX_SHA256) {
                    Ok(catalog) => return (catalog, DraftIndexStatus::Verified),
                    Err(error) => status = DraftIndexStatus::Invalid(error.to_string()),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => status = DraftIndexStatus::Invalid(error.to_string()),
            }
        }
        (Self::default(), status)
    }

    /// Explicitly fetch and verify the shipped index using the configured Hub client.
    /// A caller can retain its current catalog if this returns an error.
    pub async fn fetch(repository: &HfModelRepository, progress: Option<Progress>) -> Result<Self> {
        let path = repository
            .download_file_with_progress(DFLASH_PAIRING_INDEX_REPO, "main", &index_file(), progress)
            .await?;
        let bytes = tokio::task::spawn_blocking(move || fs::read(path)).await??;
        Self::from_pinned_index(&bytes, DFLASH_PAIRING_INDEX_SHA256)
    }

    pub(super) fn pairings(&self) -> &[DFlashPairing] {
        self.index.as_ref().map_or_else(
            || BOOTSTRAP_INDEX.pairings.as_slice(),
            |(index, _)| index.pairings.as_slice(),
        )
    }

    pub(super) fn provenance(&self) -> DraftProvenance {
        match &self.index {
            Some((_, sha256)) => DraftProvenance::PinnedIndex {
                sha256: sha256.clone(),
            },
            None => DraftProvenance::Bootstrap,
        }
    }
}

fn index_file() -> String {
    format!("index/{DFLASH_PAIRING_INDEX_SHA256}.json")
}

fn verify_index(bytes: &[u8], sha256: &str) -> Result<DFlashPairingIndex> {
    let actual = hex::encode(Sha256::digest(bytes));
    ensure!(
        actual.eq_ignore_ascii_case(sha256),
        "pairing index has SHA-256 {actual}, expected {sha256}"
    );
    let json = std::str::from_utf8(bytes).context("pairing index is not UTF-8")?;
    DFlashPairingIndex::from_json(json).map_err(anyhow::Error::msg)
}
