pub mod capabilities;
pub mod sizes;

pub mod checkpoint;
pub mod gguf;
pub mod selection;

use std::path::Path;

/// Scheme and host spellings that name the Hugging Face resolve endpoint. The host also
/// accepts its trailing-dot FQDN form (`huggingface.co.`), which DNS and URL parsers
/// normalize to the same host.
const HF_RESOLVE_PREFIXES: &[&str] = &[
    "https://huggingface.co/",
    "https://huggingface.co:443/",
    "https://huggingface.co./",
    "https://huggingface.co.:443/",
    "http://huggingface.co/",
    "http://huggingface.co:80/",
    "http://huggingface.co./",
    "http://huggingface.co.:80/",
];

/// Parses `https://huggingface.co/<owner>/<repo>/resolve/<rev>/<file>` (https or http).
/// The scheme and hostname are matched case-insensitively (including the trailing-dot host
/// form) while the returned repository, revision, and file preserve the URL's original case.
/// Accepts the explicit default ports (`:443` for https, `:80` for http), `<repo>` or
/// `<owner>/<repo>`; drops `?query`/`#fragment` before parsing, and rejects empty parts, a file
/// ending in `/` or a percent-encoded slash (`%2F`, case-insensitive), a file path whose `.`/`..`
/// segments collapse to a directory, and extra repo components.
pub fn parse_hf_resolve_url_parts(url: &str) -> Option<(&str, Option<&str>, &str)> {
    let lower = url.to_ascii_lowercase();
    let prefix_len = HF_RESOLVE_PREFIXES
        .iter()
        .find(|prefix| lower.starts_with(*prefix))
        .map(|prefix| prefix.len())?;
    let tail = &url[prefix_len..];
    // A query or fragment terminates the path, so anything after it can never
    // contain the `/resolve/` marker or be part of the repository or artifact.
    let tail = &tail[..tail.find(['?', '#']).unwrap_or(tail.len())];
    let (repo, rest) = tail.split_once("/resolve/")?;
    let (revision, file) = rest.split_once('/')?;
    if file.ends_with('/')
        || ends_with_encoded_slash(file)
        || normalizes_to_directory(file)
        || repo.split('/').count() > 2
        || repo.split('/').chain([revision, file]).any(str::is_empty)
    {
        return None;
    }
    Some((repo, Some(revision), file))
}

/// A trailing percent-encoded slash (`%2F`, case-insensitive hex) names a directory
/// even though the final byte is not a literal `/`. The suffix is read as bytes so a
/// multibyte UTF-8 final character can never split a character boundary.
fn ends_with_encoded_slash(path: &str) -> bool {
    let bytes = path.as_bytes();
    if bytes.len() < 3 {
        return false;
    }
    let tail = &bytes[bytes.len() - 3..];
    tail[0] == b'%' && tail[1..].eq_ignore_ascii_case(b"2f")
}

/// Returns `true` when the path's `.` and `..` segments collapse (RFC 3986, section 5.2.4)
/// to no remaining component, so the URL names a directory rather than an artifact file.
fn normalizes_to_directory(path: &str) -> bool {
    let mut depth = 0u32;
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => depth = depth.saturating_sub(1),
            _ => depth += 1,
        }
    }
    depth == 0
}

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use skippy_model_ref::{
    ModelRef, SAFETENSORS_SERVING_GUIDANCE, format_canonical_ref, gguf_matches_quant_selector,
    is_safetensors_model_ref, normalize_gguf_distribution_id, parse_model_ref,
    split_gguf_shard_info,
};

/// Resolves revisions and lists artifacts for model repositories.
#[async_trait]
pub trait ModelRepository: Send + Sync {
    /// Resolves `revision` (or the repository default when `None`) to a concrete revision.
    async fn resolve_revision(&self, repo: &str, revision: Option<&str>) -> Result<String>;

    /// Lists the artifact files present in `repo` at `revision`.
    async fn list_files(&self, repo: &str, revision: &str) -> Result<Vec<ModelArtifactFile>>;
}

/// A model artifact resolved against a repository: its identity, format, and the
/// complete file set needed to serve the primary artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedModelArtifact {
    pub model_id: String,
    pub source_repo: String,
    pub source_revision: String,
    pub selector: Option<String>,
    pub format: ModelFormat,
    pub files: Vec<ModelArtifactFile>,
    pub primary_file: String,
    pub canonical_ref: String,
    pub distribution_id: String,
}

/// The stable identity of a served model, including its remote source
/// coordinates when the model came from a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelIdentity {
    pub model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distribution_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
}

impl ModelIdentity {
    /// Builds an identity from a bare model id with no source coordinates.
    pub fn from_model_id(model_id: impl Into<String>) -> Self {
        Self {
            model_id: model_id.into(),
            source_repo: None,
            source_revision: None,
            source_file: None,
            canonical_ref: None,
            distribution_id: None,
            selector: None,
        }
    }
}

impl From<&ResolvedModelArtifact> for ModelIdentity {
    fn from(artifact: &ResolvedModelArtifact) -> Self {
        Self {
            model_id: artifact.model_id.clone(),
            source_repo: Some(artifact.source_repo.clone()),
            source_revision: Some(artifact.source_revision.clone()),
            source_file: Some(artifact.primary_file.clone()),
            canonical_ref: Some(artifact.canonical_ref.clone()),
            distribution_id: Some(artifact.distribution_id.clone()),
            selector: artifact.selector.clone(),
        }
    }
}

/// The on-disk format of a model artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    Gguf,
}

/// A single file in a model artifact: its path plus optional size and SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelArtifactFile {
    pub path: String,
    pub size_bytes: Option<u64>,
    pub sha256: Option<String>,
}

impl ModelArtifactFile {
    /// Builds an entry with only a path; size and digest stay unknown.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            size_bytes: None,
            sha256: None,
        }
    }
}

/// Parses `model_ref` and resolves the matching artifact from `repository`.
pub async fn resolve_model_artifact_ref(
    model_ref: &str,
    repository: &impl ModelRepository,
) -> Result<ResolvedModelArtifact> {
    let parsed = parse_artifact_ref(model_ref)?;
    resolve_model_artifact(&parsed, repository).await
}

/// Like [`resolve_model_artifact_ref`], but selects within `available_bytes`
/// when the repository exposes file sizes.
pub async fn resolve_model_artifact_ref_with_budget(
    model_ref: &str,
    repository: &impl ModelRepository,
    available_bytes: u64,
) -> Result<ResolvedModelArtifact> {
    let parsed = parse_artifact_ref(model_ref)?;
    resolve_model_artifact_with_budget(&parsed, repository, available_bytes).await
}

fn parse_artifact_ref(input: &str) -> Result<ModelRef> {
    let input = input.trim();
    if is_safetensors_model_ref(input) {
        bail!("{input}: {SAFETENSORS_SERVING_GUIDANCE}");
    }
    if let Some((repo, revision, file)) = parse_hf_resolve_url_parts(input) {
        return Ok(ModelRef {
            repo: repo.to_owned(),
            revision: revision.map(str::to_owned),
            selector: Some(file.to_owned()),
        });
    }
    let mut components = input.splitn(3, '/');
    if let (Some(owner), Some(repo), Some(file)) =
        (components.next(), components.next(), components.next())
        && !owner.contains(':')
        && !repo.contains(':')
        && file.ends_with(".gguf")
    {
        let mut model_ref = parse_model_ref(&format!("{owner}/{repo}"))?;
        model_ref.selector = Some(file.to_owned());
        return Ok(model_ref);
    }
    Ok(parse_model_ref(input)?)
}

/// Resolves the artifact for an already-parsed `model_ref`.
pub async fn resolve_model_artifact(
    model_ref: &ModelRef,
    repository: &impl ModelRepository,
) -> Result<ResolvedModelArtifact> {
    resolve_model_artifact_with_budget(model_ref, repository, 0).await
}

/// Resolves the artifact for `model_ref`, preferring files that fit `available_bytes`.
pub async fn resolve_model_artifact_with_budget(
    model_ref: &ModelRef,
    repository: &impl ModelRepository,
    available_bytes: u64,
) -> Result<ResolvedModelArtifact> {
    if let Some(selector) = model_ref
        .selector
        .as_deref()
        .filter(|value| is_safetensors_model_ref(value))
    {
        bail!("{selector}: {SAFETENSORS_SERVING_GUIDANCE}");
    }
    let source_revision = repository
        .resolve_revision(&model_ref.repo, model_ref.revision.as_deref())
        .await?;
    let mut repo_files = repository
        .list_files(&model_ref.repo, &source_revision)
        .await?;
    repo_files.sort_by(|left, right| left.path.cmp(&right.path));

    let primary_file = match model_ref.selector.as_deref() {
        Some(selector) => select_primary_file(selector, &repo_files)?,
        None => selection::select_default_gguf_file(&repo_files, available_bytes)
            .ok_or_else(|| anyhow::anyhow!(
                "no runnable GGUF model files found in {}@{source_revision}. {SAFETENSORS_SERVING_GUIDANCE}",
                model_ref.repo
            ))?,
    };
    let files = artifact_file_set(&primary_file.path, &repo_files);
    let distribution_id = normalize_gguf_distribution_id(&primary_file.path)
        .ok_or_else(|| anyhow::anyhow!("invalid GGUF artifact file name: {}", primary_file.path))?;

    Ok(ResolvedModelArtifact {
        model_id: model_ref.display_id(),
        source_repo: model_ref.repo.clone(),
        source_revision: source_revision.clone(),
        selector: model_ref.selector.clone(),
        format: ModelFormat::Gguf,
        files,
        primary_file: primary_file.path.clone(),
        canonical_ref: format_canonical_ref(&model_ref.repo, &source_revision, &primary_file.path),
        distribution_id,
    })
}

/// Picks the primary artifact file for `selector` (or the repository default
/// when `None`) from `files`.
pub fn select_primary_artifact_file(
    selector: Option<&str>,
    files: &[ModelArtifactFile],
) -> Result<ModelArtifactFile> {
    match selector {
        Some(selector) => select_primary_file(selector, files),
        None => selection::select_default_gguf_file(files, 0).ok_or_else(|| {
            anyhow::anyhow!(
                "no runnable GGUF model files found in repository. {SAFETENSORS_SERVING_GUIDANCE}"
            )
        }),
    }
}

/// Returns the full file set that serves `primary_file`, including every
/// shard when it is a split GGUF.
pub fn artifact_files_for_primary(
    primary_file: &str,
    files: &[ModelArtifactFile],
) -> Vec<ModelArtifactFile> {
    artifact_file_set(primary_file, files)
}

fn select_primary_file(selector: &str, files: &[ModelArtifactFile]) -> Result<ModelArtifactFile> {
    if is_safetensors_model_ref(selector) {
        bail!("{selector}: {SAFETENSORS_SERVING_GUIDANCE}");
    }
    let selector_lower = selector.to_ascii_lowercase();
    let gguf_exact = format!("{selector_lower}.gguf");
    let gguf_split_prefix = format!("{selector_lower}-00001-of-");

    select_ranked_file(files, |file, lower, basename| {
        if !lower.ends_with(".gguf") {
            return None;
        }
        // Explicit GGUF projector references are used for catalog sidecar downloads.
        if lower == selector_lower || basename == selector_lower {
            return selection::has_complete_gguf_parts(&file.path, files).then_some(0);
        }
        if !selection::is_gguf_model_candidate(&file.path, files) {
            return None;
        }
        if gguf_matches_quant_selector(&file.path, selector) {
            Some(1)
        } else if basename == gguf_exact {
            Some(2)
        } else if basename.starts_with(&gguf_split_prefix) {
            Some(3)
        } else {
            None
        }
    })
    .ok_or_else(|| {
        anyhow::anyhow!("no runnable GGUF model file matching selector '{selector}' in repository. {SAFETENSORS_SERVING_GUIDANCE}")
    })
}

fn select_ranked_file(
    files: &[ModelArtifactFile],
    mut rank: impl FnMut(&ModelArtifactFile, &str, &str) -> Option<u8>,
) -> Option<ModelArtifactFile> {
    files
        .iter()
        .filter_map(|file| {
            let lower = file.path.to_ascii_lowercase();
            let basename = basename_lower(&file.path);
            rank(file, &lower, &basename).map(|rank| {
                (
                    rank,
                    selection::file_preference_score(&file.path),
                    file.path.as_str(),
                    file,
                )
            })
        })
        .min_by(|left, right| (left.0, left.1, left.2).cmp(&(right.0, right.1, right.2)))
        .map(|(_, _, _, file)| file.clone())
}

fn artifact_file_set(primary_file: &str, files: &[ModelArtifactFile]) -> Vec<ModelArtifactFile> {
    if let Some(primary) = split_gguf_shard_info(primary_file) {
        let mut shards = files
            .iter()
            .filter(|file| {
                split_gguf_shard_info(&file.path)
                    .map(|candidate| {
                        candidate.prefix == primary.prefix && candidate.total == primary.total
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect::<Vec<_>>();
        shards.sort_by(|left, right| left.path.cmp(&right.path));
        if !shards.is_empty() {
            return shards;
        }
    }

    vec![
        files
            .iter()
            .find(|file| file.path == primary_file)
            .cloned()
            .unwrap_or_else(|| ModelArtifactFile::new(primary_file)),
    ]
}

fn basename_lower(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(path)
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct MemoryRepository {
        revision: String,
        files: HashMap<String, Vec<ModelArtifactFile>>,
    }

    #[async_trait]
    impl ModelRepository for MemoryRepository {
        async fn resolve_revision(&self, _repo: &str, revision: Option<&str>) -> Result<String> {
            Ok(revision.unwrap_or(&self.revision).to_string())
        }

        async fn list_files(&self, repo: &str, _revision: &str) -> Result<Vec<ModelArtifactFile>> {
            Ok(self.files.get(repo).cloned().unwrap_or_default())
        }
    }

    fn repo(files: Vec<&str>) -> MemoryRepository {
        MemoryRepository {
            revision: "abc123".to_string(),
            files: HashMap::from([(
                "org/repo".to_string(),
                files.into_iter().map(ModelArtifactFile::new).collect(),
            )]),
        }
    }

    fn files(paths: &[&str]) -> Vec<ModelArtifactFile> {
        paths.iter().copied().map(ModelArtifactFile::new).collect()
    }

    #[tokio::test]
    async fn resolves_quant_selector_to_gguf_file() {
        let repository = repo(vec!["Model-Q5_K_M.gguf", "Model-Q4_K_M.gguf", "README.md"]);

        let resolved = resolve_model_artifact_ref("org/repo:Q4_K_M", &repository)
            .await
            .unwrap();

        assert_eq!(resolved.model_id, "org/repo:Q4_K_M");
        assert_eq!(resolved.source_revision, "abc123");
        assert_eq!(resolved.primary_file, "Model-Q4_K_M.gguf");
        assert_eq!(resolved.canonical_ref, "org/repo@abc123/Model-Q4_K_M.gguf");
        assert_eq!(resolved.distribution_id, "Model-Q4_K_M");
        assert_eq!(resolved.files.len(), 1);
    }

    #[tokio::test]
    async fn gguf_repository_uses_shared_fit_selection_instead_of_safetensors() {
        let repository = MemoryRepository {
            revision: "abc123".into(),
            files: HashMap::from([(
                "org/repo-GGUF".into(),
                vec![
                    ModelArtifactFile::new("model.safetensors"),
                    ModelArtifactFile {
                        path: "model-Q4_K_M.gguf".into(),
                        size_bytes: Some(4),
                        sha256: None,
                    },
                    ModelArtifactFile {
                        path: "model-Q8_0.gguf".into(),
                        size_bytes: Some(8),
                        sha256: None,
                    },
                ],
            )]),
        };
        let selected = resolve_model_artifact_ref_with_budget("org/repo-GGUF", &repository, 10)
            .await
            .unwrap();
        assert_eq!(selected.primary_file, "model-Q8_0.gguf");
    }

    #[tokio::test]
    async fn resolves_split_gguf_selector_to_all_shards() {
        let repository = repo(vec![
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00002-of-00003.gguf",
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00001-of-00003.gguf",
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00003-of-00003.gguf",
            "UD-Q4_K_M/GLM-5.1-UD-Q4_K_M-00001-of-00003.gguf",
        ]);

        let resolved = resolve_model_artifact_ref("org/repo:UD-IQ2_M", &repository)
            .await
            .unwrap();

        assert_eq!(
            resolved.primary_file,
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00001-of-00003.gguf"
        );
        assert_eq!(resolved.distribution_id, "GLM-5.1-UD-IQ2_M");
        assert_eq!(resolved.files.len(), 3);
        assert_eq!(
            resolved.files[2].path,
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00003-of-00003.gguf"
        );
    }

    #[test]
    fn public_selector_api_resolves_mesh_split_stem_to_first_part() {
        let files = files(&[
            "zai-org.GLM-5.1.Q2_K-00002-of-00002.gguf",
            "zai-org.GLM-5.1.Q2_K-00001-of-00002.gguf",
        ]);

        let selected = select_primary_artifact_file(Some("zai-org.GLM-5.1.Q2_K"), &files).unwrap();

        assert_eq!(selected.path, "zai-org.GLM-5.1.Q2_K-00001-of-00002.gguf");
    }

    #[test]
    fn public_selector_api_resolves_mesh_quant_aliases() {
        let files = files(&[
            "qwen3.5-moe-0.87B-d0.8B.Q2_K.gguf",
            "gemma-4-31B-it-Q4_0.gguf",
            "Qwen3-8B-Q4_K_M.gguf",
        ]);

        assert_eq!(
            select_primary_artifact_file(Some("Q2_K"), &files)
                .unwrap()
                .path,
            "qwen3.5-moe-0.87B-d0.8B.Q2_K.gguf"
        );
        assert_eq!(
            select_primary_artifact_file(Some("Q4_0"), &files)
                .unwrap()
                .path,
            "gemma-4-31B-it-Q4_0.gguf"
        );
    }

    #[test]
    fn exact_filename_with_other_format_cannot_override_gguf_match() {
        let files = files(&["Model-Q4_K_M.gguf", "Q4_K_M"]);

        let selected = select_primary_artifact_file(Some("Q4_K_M"), &files).unwrap();

        assert_eq!(selected.path, "Model-Q4_K_M.gguf");
    }

    #[test]
    fn selector_ranking_is_independent_of_input_order() {
        let first_order = files(&["z/Model-Q4_K_M.gguf", "a/Model-Q4_K_M.gguf"]);
        let second_order = files(&["a/Model-Q4_K_M.gguf", "z/Model-Q4_K_M.gguf"]);

        let first = select_primary_artifact_file(Some("Q4_K_M"), &first_order).unwrap();
        let second = select_primary_artifact_file(Some("Q4_K_M"), &second_order).unwrap();

        assert_eq!(first.path, "a/Model-Q4_K_M.gguf");
        assert_eq!(second.path, first.path);
    }

    #[test]
    fn checkpoint_only_repository_has_no_runnable_artifact() {
        let files = files(&[
            "model-00002-of-00048.safetensors",
            "model-00001-of-00048.safetensors",
            "model.safetensors.index.json",
        ]);

        for selector in [None, Some("model"), Some("model.safetensors.index.json")] {
            let error = select_primary_artifact_file(selector, &files).unwrap_err();
            assert!(error.to_string().contains(SAFETENSORS_SERVING_GUIDANCE));
        }
    }

    #[test]
    fn public_default_api_preserves_mesh_default_ordering() {
        let files = files(&[
            "Qwen3-8B-Q8_0.gguf",
            "mmproj-BF16.gguf",
            "Qwen3-8B-Q4_K_M.gguf",
        ]);

        let selected = select_primary_artifact_file(None, &files).unwrap();

        assert_eq!(selected.path, "Qwen3-8B-Q4_K_M.gguf");
    }

    #[test]
    fn public_default_api_skips_safetensors_in_mixed_repository() {
        let files = files(&[
            "Qwen3-8B-Q4_K_M.gguf",
            "model.safetensors",
            "model.safetensors.index.json",
        ]);

        let selected = select_primary_artifact_file(None, &files).unwrap();

        assert_eq!(selected.path, "Qwen3-8B-Q4_K_M.gguf");
    }

    #[test]
    fn default_selection_rejects_sidecars_and_non_first_split_shards() {
        let files = files(&[
            "mmproj-model-f16.gguf",
            "Model-Q4_K_M-00002-of-00002.gguf",
            "Model-Q5_K_M-00001-of-00002.gguf",
        ]);

        let error = select_primary_artifact_file(None, &files).unwrap_err();

        assert!(error.to_string().contains("no runnable GGUF model files"));
    }

    #[test]
    fn public_artifact_set_returns_all_split_gguf_shards() {
        let files = files(&[
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00002-of-00003.gguf",
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00001-of-00003.gguf",
            "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00003-of-00003.gguf",
            "UD-Q4_K_M/GLM-5.1-UD-Q4_K_M-00001-of-00003.gguf",
        ]);

        let shards =
            artifact_files_for_primary("UD-IQ2_M/GLM-5.1-UD-IQ2_M-00001-of-00003.gguf", &files);

        assert_eq!(
            shards
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00001-of-00003.gguf",
                "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00002-of-00003.gguf",
                "UD-IQ2_M/GLM-5.1-UD-IQ2_M-00003-of-00003.gguf",
            ]
        );
    }

    #[tokio::test]
    async fn accepts_revisioned_selector_refs() {
        let repository = repo(vec!["Model-Q4_K_M.gguf"]);

        let resolved = resolve_model_artifact_ref("org/repo:Q4_K_M@rev-1", &repository)
            .await
            .unwrap();

        assert_eq!(resolved.model_id, "org/repo@rev-1:Q4_K_M");
        assert_eq!(resolved.source_revision, "rev-1");
        assert_eq!(resolved.canonical_ref, "org/repo@rev-1/Model-Q4_K_M.gguf");
    }

    #[tokio::test]
    async fn default_selection_prefers_primary_weights() {
        let repository = repo(vec![
            "README.md",
            "Qwen3-8B-Q4_K_M.gguf",
            "Qwen3-8B-Q5_K_M.gguf",
        ]);

        let resolved = resolve_model_artifact_ref("org/repo", &repository)
            .await
            .unwrap();

        assert_eq!(resolved.primary_file, "Qwen3-8B-Q4_K_M.gguf");
        assert_eq!(resolved.format, ModelFormat::Gguf);
    }

    #[tokio::test]
    async fn resolves_exact_gguf_file_refs_at_the_requested_revision() {
        let repository = repo(vec!["nested/Model-Q4_K_M.gguf"]);
        for reference in [
            "org/repo@rev-1/nested/Model-Q4_K_M.gguf",
            "https://huggingface.co/org/repo/resolve/rev-1/nested/Model-Q4_K_M.gguf",
        ] {
            let artifact = resolve_model_artifact_ref(reference, &repository)
                .await
                .unwrap();
            assert_eq!(artifact.source_revision, "rev-1");
            assert_eq!(artifact.primary_file, "nested/Model-Q4_K_M.gguf");
        }
    }

    #[tokio::test]
    async fn explicit_checkpoint_refs_are_rejected() {
        let repository = repo(vec!["model.safetensors", "Model-Q4_K_M.gguf"]);
        for reference in [
            "org/repo",
            "org/repo/model.safetensors",
            "org/repo:weights/model.safetensors.index.json",
            "https://huggingface.co/org/repo/resolve/main/model.safetensors?download=1",
        ] {
            let result = resolve_model_artifact_ref(reference, &repository).await;
            if reference == "org/repo" {
                assert_eq!(result.unwrap().primary_file, "Model-Q4_K_M.gguf");
            } else {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains(SAFETENSORS_SERVING_GUIDANCE)
                );
            }
        }
    }

    #[tokio::test]
    async fn unknown_selector_returns_error() {
        let repository = repo(vec!["Model-Q4_K_M.gguf"]);

        let error = resolve_model_artifact_ref("org/repo:Q5_K_M", &repository)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("no runnable GGUF model file matching selector 'Q5_K_M'")
        );
    }
}

#[cfg(test)]
mod hf_resolve_url_tests {
    use super::parse_hf_resolve_url_parts;

    #[test]
    fn parses_https_and_http_urls() {
        let expected = Some(("org/repo", Some("main"), "dir/m.gguf"));
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co/org/repo/resolve/main/dir/m.gguf"),
            expected
        );
        assert_eq!(
            parse_hf_resolve_url_parts("http://huggingface.co/org/repo/resolve/main/dir/m.gguf"),
            expected
        );
    }

    #[test]
    fn accepts_explicit_default_ports() {
        let expected = Some(("org/repo", Some("main"), "dir/m.gguf"));
        assert_eq!(
            parse_hf_resolve_url_parts(
                "https://huggingface.co:443/org/repo/resolve/main/dir/m.gguf"
            ),
            expected
        );
        assert_eq!(
            parse_hf_resolve_url_parts("http://huggingface.co:80/org/repo/resolve/main/dir/m.gguf"),
            expected
        );
    }

    /// A resolve path ending in a literal slash names a directory, not a file.
    #[test]
    fn rejects_file_ending_in_slash() {
        for url in [
            "https://huggingface.co/org/repo/resolve/main/dir/",
            "http://huggingface.co:80/org/repo/resolve/main/dir/",
        ] {
            assert_eq!(parse_hf_resolve_url_parts(url), None, "{url}");
        }
    }

    /// A trailing percent-encoded slash names a directory even though the
    /// final byte is not a literal `/`; an encoded slash inside the path
    /// still names a file.
    #[test]
    fn rejects_file_ending_in_encoded_slash() {
        for url in [
            "https://huggingface.co/org/repo/resolve/main/dir%2F",
            "https://huggingface.co/org/repo/resolve/main/dir%2f",
            "http://huggingface.co:80/org/repo/resolve/main/dir%2F",
        ] {
            assert_eq!(parse_hf_resolve_url_parts(url), None, "{url}");
        }
        // An encoded slash inside the path still names a file.
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co/org/repo/resolve/main/dir%2Fm.gguf"),
            Some(("org/repo", Some("main"), "dir%2Fm.gguf"))
        );
    }

    /// File paths whose `.`/`..` segments collapse to nothing name a
    /// directory rather than an artifact file.
    #[test]
    fn rejects_paths_that_normalize_to_directories() {
        for url in [
            "https://huggingface.co/org/repo/resolve/main/dir/..",
            "https://huggingface.co/org/repo/resolve/main/a/..",
            "https://huggingface.co/org/repo/resolve/main/..",
            "http://huggingface.co:80/org/repo/resolve/main/dir/..",
        ] {
            assert_eq!(parse_hf_resolve_url_parts(url), None, "{url}");
        }
        // `a/b/..` collapses to the artifact file `a`, so it stays accepted.
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co/org/repo/resolve/main/a/b/.."),
            Some(("org/repo", Some("main"), "a/b/.."))
        );
    }

    #[test]
    fn accepts_mixed_case_hostname() {
        let expected = Some(("org/repo", Some("main"), "dir/m.gguf"));
        assert_eq!(
            parse_hf_resolve_url_parts("https://HuggingFace.Co/org/repo/resolve/main/dir/m.gguf"),
            expected
        );
        assert_eq!(
            parse_hf_resolve_url_parts("http://HUGGINGFACE.CO:80/org/repo/resolve/main/dir/m.gguf"),
            expected
        );
    }

    #[test]
    fn accepts_trailing_dot_hostname() {
        let expected = Some(("org/repo", Some("main"), "dir/m.gguf"));
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co./org/repo/resolve/main/dir/m.gguf"),
            expected
        );
        assert_eq!(
            parse_hf_resolve_url_parts(
                "http://huggingface.co.:80/org/repo/resolve/main/dir/m.gguf"
            ),
            expected
        );
    }

    #[test]
    fn parses_unicode_file_components_without_panic() {
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co/org/repo/resolve/main/aéé"),
            Some(("org/repo", Some("main"), "aéé"))
        );
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co/org/repo/resolve/main/模"),
            Some(("org/repo", Some("main"), "模"))
        );
    }

    /// Query or fragment text before `/resolve/` would pollute the repository
    /// id, so such URLs must be rejected rather than parsed.
    #[test]
    fn rejects_query_text_before_resolve() {
        for url in [
            "https://huggingface.co/org/repo?x/resolve/main/m.gguf",
            "https://huggingface.co/org#frag/repo/resolve/main/m.gguf",
        ] {
            assert_eq!(parse_hf_resolve_url_parts(url), None, "{url}");
        }
    }

    #[test]
    fn accepts_top_level_repository() {
        assert_eq!(
            parse_hf_resolve_url_parts("https://huggingface.co/gpt2/resolve/main/m.gguf"),
            Some(("gpt2", Some("main"), "m.gguf"))
        );
    }

    #[test]
    fn strips_query_and_fragment() {
        for suffix in ["?download=true", "#frag"] {
            let url = format!("https://huggingface.co/org/repo/resolve/main/m.gguf{suffix}");
            assert_eq!(
                parse_hf_resolve_url_parts(&url),
                Some(("org/repo", Some("main"), "m.gguf")),
                "{url}"
            );
        }
    }

    #[test]
    fn rejects_malformed_urls() {
        for url in [
            "https://huggingface.co/org/repo/resolve/main/",
            "https://huggingface.co/org/repo/resolve/main",
            "https://huggingface.co/org/repo/resolve//m.gguf",
            "https://huggingface.co/org/repo/extra/resolve/main/m.gguf",
            "https://huggingface.co//repo/resolve/main/m.gguf",
            "https://huggingface.co/org//resolve/main/m.gguf",
            "https://example.com/org/repo/resolve/main/m.gguf",
        ] {
            assert_eq!(parse_hf_resolve_url_parts(url), None, "{url}");
        }
    }
}
