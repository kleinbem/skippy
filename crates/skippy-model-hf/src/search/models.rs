use crate::capabilities;
use crate::details::{
    gguf_variant_size_bytes_from_siblings, matching_remote_catalog_model_for_huggingface,
    merge_capabilities, remote_hf_size_label_with_api,
};
use crate::remote_catalog;
use skippy_model_artifact::capabilities::ModelCapabilities;
use skippy_model_artifact::selection::file_preference_score;
use skippy_model_ref::quant_selector_from_gguf_file;

pub use crate::search::{ArtifactFilter as SearchArtifactFilter, Sort as SearchSort};
use anyhow::{Context, Result};
use hf_hub::repository::ModelInfo;
use std::collections::HashSet;
use tokio::task::JoinSet;

#[derive(Clone, Debug)]
pub struct SearchHit {
    pub repo_id: String,
    pub kind: &'static str,
    pub exact_ref: String,
    pub variant_count: Option<usize>,
    pub size_label: Option<String>,
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    pub catalog: Option<remote_catalog::RemoteCatalogModel>,
    pub capabilities: ModelCapabilities,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchProgress {
    SearchingHub,
    InspectingRepos { completed: usize, total: usize },
}

pub trait SearchObserver {
    fn progress(&mut self, progress: SearchProgress);
    fn inspection_warning(&mut self, error: &anyhow::Error) -> Result<()>;
}

pub fn search_catalog_models(query: &str) -> Result<Vec<remote_catalog::RemoteCatalogModel>> {
    remote_catalog::ensure_catalog().context("load configured model catalog for catalog search")?;
    let q = query.to_lowercase();
    let mut results: Vec<_> = remote_catalog::loaded_models()
        .context("parse configured model catalog models for catalog search")?
        .into_iter()
        .filter(|model| {
            model.name.to_lowercase().contains(&q)
                || model.file.to_lowercase().contains(&q)
                || model
                    .description
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(&q)
        })
        .collect();
    results.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(results)
}

// Keep search custom for now. `hf-hub` handles cache and file transport well,
// but it does not expose a Hub search surface in this crate version.
pub async fn search_huggingface(
    query: &str,
    limit: usize,
    filter: SearchArtifactFilter,
    sort: SearchSort,
    observer: &mut (dyn SearchObserver + Send),
) -> Result<Vec<SearchHit>> {
    const SEARCH_CONCURRENCY: usize = 10;

    observer.progress(SearchProgress::SearchingHub);
    let api = crate::build_hf_async_api()?;
    let repos = crate::search::search_repositories(query, limit, filter, sort).await?;

    let total = repos.len();
    observer.progress(SearchProgress::InspectingRepos {
        completed: 0,
        total,
    });

    let mut pending = repos.into_iter().enumerate();
    let mut join_set = JoinSet::new();
    for _ in 0..SEARCH_CONCURRENCY.min(total.max(1)) {
        if let Some((index, repo)) = pending.next() {
            let api = api.clone();
            join_set.spawn(async move { (index, build_search_hit(api, repo).await) });
        }
    }

    let mut completed = 0usize;
    let mut indexed_hits = Vec::new();
    while let Some(joined) = join_set.join_next().await {
        let (index, result) = joined.context("Join Hugging Face repo inspection task")?;
        completed += 1;
        observer.progress(SearchProgress::InspectingRepos { completed, total });
        if let Some(hit) = inspected_hit(result, observer)? {
            indexed_hits.push((index, hit));
        }
        if let Some((next_index, repo)) = pending.next() {
            let api = api.clone();
            join_set.spawn(async move { (next_index, build_search_hit(api, repo).await) });
        }
    }

    indexed_hits.sort_by_key(|(index, _)| *index);
    let mut hits: Vec<SearchHit> = indexed_hits.into_iter().map(|(_, hit)| hit).collect();
    apply_local_search_sort(&mut hits, sort);
    hits.truncate(limit);
    Ok(hits)
}

fn inspected_hit(
    result: Result<Option<SearchHit>>,
    observer: &mut (dyn SearchObserver + Send),
) -> Result<Option<SearchHit>> {
    match result {
        Ok(hit) => Ok(hit),
        Err(error) => {
            observer.inspection_warning(&error)?;
            Ok(None)
        }
    }
}

async fn build_search_hit(api: hf_hub::HFClient, repo: ModelInfo) -> Result<Option<SearchHit>> {
    let repo_downloads = repo.downloads;
    let repo_likes = repo.likes;
    let detail = repo;
    let repo_id = detail.id.clone();
    let siblings = detail.siblings.clone().unwrap_or_default();
    let sibling_names: Vec<String> = siblings
        .iter()
        .map(|sibling| sibling.rfilename.clone())
        .collect();
    let sibling_size_entries: Vec<(String, Option<u64>)> = siblings
        .iter()
        .map(|sibling| (sibling.rfilename.clone(), sibling.size))
        .collect();
    let candidates = collect_repo_artifact_candidates(&sibling_names);
    let Some(candidate) = candidates.first() else {
        return Ok(None);
    };

    let variant_count = Some(gguf_variant_count_from_candidates(&repo_id, &candidates));
    let remote_metadata = capabilities::fetch_remote_hf_metadata_jsons(&repo_id, None).await;
    let catalog = matching_remote_catalog_model_for_huggingface(&repo_id, None, candidate);
    let size_label = match catalog {
        Some(ref model) => model.size.clone(),
        None => match size_label_from_sibling_entries(candidate, &sibling_size_entries) {
            Some(size_label) => Some(size_label),
            None => remote_hf_size_label_with_api(&api, &repo_id, None, candidate).await,
        },
    };
    let remote_caps = capabilities::infer_remote_hf_capabilities_with_metadata(
        &repo_id,
        candidate,
        Some(&sibling_names),
        &remote_metadata,
    );
    let capabilities = match catalog {
        Some(ref model) => {
            let base = capabilities::infer_remote_catalog_capabilities(model);
            merge_capabilities(base, remote_caps)
        }
        None => remote_caps,
    };
    Ok(Some(SearchHit {
        repo_id: repo_id.clone(),
        kind: "🦙 GGUF",
        exact_ref: display_exact_ref(&repo_id, candidate),
        variant_count,
        size_label,
        downloads: detail.downloads.or(repo_downloads),
        likes: detail.likes.or(repo_likes),
        catalog,
        capabilities,
    }))
}

fn apply_local_search_sort(hits: &mut [SearchHit], sort: SearchSort) {
    match sort {
        SearchSort::ParametersDesc => {
            hits.sort_by(|left, right| {
                approx_parameter_count_b(right)
                    .partial_cmp(&approx_parameter_count_b(left))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.repo_id.cmp(&right.repo_id))
            });
        }
        SearchSort::ParametersAsc => {
            hits.sort_by(|left, right| {
                approx_parameter_count_b(left)
                    .partial_cmp(&approx_parameter_count_b(right))
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.repo_id.cmp(&right.repo_id))
            });
        }
        _ => {}
    }
}

fn approx_parameter_count_b(hit: &SearchHit) -> f64 {
    crate::search::approximate_parameter_count_b_from_text(&format!(
        "{} {}",
        hit.repo_id, hit.exact_ref
    ))
    .unwrap_or(-1.0)
}

fn display_exact_ref(repo: &str, file: &str) -> String {
    match quant_selector_from_gguf_file(file) {
        Some(selector) => format!("{repo}:{selector}"),
        None => format!("{repo}/{}", display_ref_file(file)),
    }
}

fn display_ref_file(file: &str) -> String {
    if let Some(without_ext) = file.strip_suffix(".gguf") {
        if !without_ext.contains("-00001-of-") {
            return without_ext.to_string();
        }
        let Some((prefix, suffix)) = without_ext.rsplit_once("-00001-of-") else {
            return without_ext.to_string();
        };
        if suffix.len() == 5 && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            return prefix.to_string();
        }
        return without_ext.to_string();
    }

    file.to_string()
}

fn size_label_from_sibling_entries(
    file: &str,
    siblings: &[(String, Option<u64>)],
) -> Option<String> {
    gguf_variant_size_bytes_from_siblings(file, siblings)
        .map(skippy_model_artifact::sizes::format_size_bytes)
}

fn collect_repo_artifact_candidates(siblings: &[String]) -> Vec<String> {
    use skippy_model_artifact::{ModelArtifactFile, selection::is_gguf_model_candidate};

    let files = siblings
        .iter()
        .cloned()
        .map(ModelArtifactFile::new)
        .collect::<Vec<_>>();
    let mut candidates = siblings
        .iter()
        .filter(|file| is_gguf_model_candidate(file, &files))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        file_preference_score(left)
            .cmp(&file_preference_score(right))
            .then_with(|| left.cmp(right))
    });
    candidates
}

fn gguf_variant_count_from_candidates(repo: &str, candidates: &[String]) -> usize {
    candidates
        .iter()
        .filter_map(|file| {
            quant_selector_from_gguf_file(file).map(|_| display_exact_ref(repo, file))
        })
        .collect::<HashSet<_>>()
        .len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn failed_inspections_continue_unless_warning_delivery_fails() {
        struct Warnings {
            seen: Vec<String>,
            fail: bool,
        }
        impl SearchObserver for Warnings {
            fn progress(&mut self, _: SearchProgress) {}
            fn inspection_warning(&mut self, error: &anyhow::Error) -> Result<()> {
                self.seen.push(error.to_string());
                if self.fail {
                    anyhow::bail!("warning sink failed");
                }
                Ok(())
            }
        }
        let mut warnings = Warnings {
            seen: Vec::new(),
            fail: false,
        };
        let result = inspected_hit(Err(anyhow::anyhow!("repo failed")), &mut warnings).unwrap();
        assert!(result.is_none());
        assert_eq!(warnings.seen, ["repo failed"]);
        warnings.fail = true;
        assert_eq!(
            inspected_hit(Err(anyhow::anyhow!("second repo failed")), &mut warnings)
                .unwrap_err()
                .to_string(),
            "warning sink failed"
        );
    }

    struct Observer(Arc<Mutex<Vec<SearchProgress>>>);
    impl SearchObserver for Observer {
        fn progress(&mut self, progress: SearchProgress) {
            self.0.lock().unwrap().push(progress);
        }
        fn inspection_warning(&mut self, _: &anyhow::Error) -> Result<()> {
            Ok(())
        }
    }

    fn assert_progress_sequence(events: &[SearchProgress]) {
        assert!(
            events
                .first()
                .is_some_and(|event| matches!(event, SearchProgress::SearchingHub)),
            "expected initial SearchingHub event, got {events:?}"
        );

        let mut last_completed = 0usize;
        let mut last_total = None;
        for event in events {
            if let SearchProgress::InspectingRepos { completed, total } = *event {
                assert!(
                    completed <= total,
                    "completed {completed} exceeded total {total}"
                );
                assert!(
                    completed >= last_completed,
                    "progress regressed from {last_completed} to {completed}"
                );
                if let Some(previous_total) = last_total {
                    assert_eq!(
                        total, previous_total,
                        "repo inspection total changed from {previous_total} to {total}"
                    );
                }
                last_completed = completed;
                last_total = Some(total);
            }
        }

        if let Some(total) = last_total {
            assert_eq!(
                last_completed, total,
                "expected final inspection progress to reach total repos"
            );
        }
    }

    #[test]
    fn collect_repo_artifact_candidates_excludes_checkpoints() {
        let siblings = vec![
            "model.safetensors".to_string(),
            "model.safetensors.index.json".to_string(),
        ];
        let candidates = collect_repo_artifact_candidates(&siblings);
        assert!(candidates.is_empty());
    }

    #[test]
    fn collect_repo_artifact_candidates_keeps_gguf_first_split_only() {
        let siblings = vec![
            "GLM-5.1-UD-Q5_K_XL-00002-of-00002.gguf".to_string(),
            "GLM-5.1-UD-Q5_K_XL-00001-of-00002.gguf".to_string(),
            "GLM-5.1-UD-Q4_K_M.gguf".to_string(),
        ];
        let candidates = collect_repo_artifact_candidates(&siblings);
        assert_eq!(
            candidates,
            vec![
                "GLM-5.1-UD-Q5_K_XL-00001-of-00002.gguf".to_string(),
                "GLM-5.1-UD-Q4_K_M.gguf".to_string(),
            ]
        );
    }

    #[test]
    fn collect_repo_artifact_candidates_excludes_mmproj_gguf_sidecars() {
        let siblings = vec![
            "mmproj-BF16.gguf".to_string(),
            "vision/mmproj-F16.gguf".to_string(),
            "gemma-4-26B-A4B-it-UD-Q3_K_S.gguf".to_string(),
        ];
        let candidates = collect_repo_artifact_candidates(&siblings);
        assert_eq!(
            candidates,
            vec!["gemma-4-26B-A4B-it-UD-Q3_K_S.gguf".to_string()]
        );
    }

    #[test]
    fn gguf_variant_count_from_candidates_counts_selectable_variants() {
        let siblings = vec![
            "mmproj-BF16.gguf".to_string(),
            "BF16/Qwen3.6-35B-A3B-BF16-00001-of-00002.gguf".to_string(),
            "BF16/Qwen3.6-35B-A3B-BF16-00002-of-00002.gguf".to_string(),
            "Qwen3.6-35B-A3B-Q8_0.gguf".to_string(),
            "Qwen3.6-35B-A3B-Q4_K_M.gguf".to_string(),
        ];
        let candidates = collect_repo_artifact_candidates(&siblings);
        assert_eq!(
            gguf_variant_count_from_candidates("unsloth/Qwen3.6-35B-A3B-GGUF", &candidates),
            3
        );
    }

    #[test]
    fn size_label_from_sibling_entries_prefers_repo_metadata_size() {
        let siblings = vec![
            ("model-q4.gguf".to_string(), Some(16_900_000_000)),
            ("model-q5.gguf".to_string(), Some(18_800_000_000)),
        ];
        assert_eq!(
            size_label_from_sibling_entries("model-q4.gguf", &siblings).as_deref(),
            Some("16.9GB")
        );
    }

    #[test]
    fn size_label_from_sibling_entries_sums_split_variant_sizes() {
        let siblings = vec![
            (
                "IQ3_K/Kimi-K2.6-IQ3_K-00001-of-00012.gguf".to_string(),
                Some(6_912_800),
            ),
            (
                "IQ3_K/Kimi-K2.6-IQ3_K-00002-of-00012.gguf".to_string(),
                Some(45_004_320_032),
            ),
            (
                "IQ3_K/Kimi-K2.6-IQ3_K-00003-of-00012.gguf".to_string(),
                Some(45_669_680_480),
            ),
        ];
        assert_eq!(
            size_label_from_sibling_entries("IQ3_K/Kimi-K2.6-IQ3_K-00001-of-00012.gguf", &siblings)
                .as_deref(),
            Some("90.7GB")
        );
    }

    #[test]
    fn size_label_from_sibling_entries_returns_none_when_missing() {
        let siblings = vec![("model-q4.gguf".to_string(), None)];
        assert_eq!(
            size_label_from_sibling_entries("model-q4.gguf", &siblings),
            None
        );
        assert_eq!(
            size_label_from_sibling_entries("model-q5.gguf", &siblings),
            None
        );
    }

    #[test]
    fn display_ref_file_uses_gguf_stems() {
        assert_eq!(display_ref_file("Qwen3-8B-Q4_K_M.gguf"), "Qwen3-8B-Q4_K_M");
        assert_eq!(
            display_ref_file("GLM-5.1-UD-Q5_K_XL-00001-of-00002.gguf"),
            "GLM-5.1-UD-Q5_K_XL"
        );
    }

    #[test]
    fn display_exact_ref_uses_short_quant_for_gguf() {
        assert_eq!(
            display_exact_ref(
                "unsloth/gemma-4-26B-A4B-it-GGUF",
                "gemma-4-26B-A4B-it-UD-Q3_K_S-00001-of-00009.gguf"
            ),
            "unsloth/gemma-4-26B-A4B-it-GGUF:UD-Q3_K_S"
        );
        assert_eq!(
            display_exact_ref(
                "QuantFactory/Meta-Llama-3.1-8B-Instruct-GGUF",
                "Meta-Llama-3.1-8B-Instruct.Q4_K_M.gguf"
            ),
            "QuantFactory/Meta-Llama-3.1-8B-Instruct-GGUF:Q4_K_M"
        );
    }

    #[tokio::test]
    #[ignore = "live Hugging Face search; run explicitly when validating hub integration"]
    async fn live_search_huggingface_gguf_returns_results_and_reports_progress() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let results = search_huggingface(
            "llama",
            5,
            SearchArtifactFilter::Gguf,
            SearchSort::Trending,
            &mut Observer(Arc::clone(&events)),
        )
        .await
        .expect("live gguf search should succeed");

        assert!(
            !results.is_empty(),
            "expected at least one live gguf result"
        );
        assert!(
            results.iter().all(|hit| hit.kind == "🦙 GGUF"),
            "expected only GGUF hits, got {results:?}"
        );
        assert!(
            results
                .iter()
                .all(|hit| !hit.repo_id.is_empty() && !hit.exact_ref.is_empty()),
            "expected populated repo ids and refs, got {results:?}"
        );

        let events = events.lock().unwrap().clone();
        assert_progress_sequence(&events);
    }
}
