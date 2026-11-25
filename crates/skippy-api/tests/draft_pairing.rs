use std::{path::Path, time::Duration};

use sha2::{Digest, Sha256};
use skippy_api::speculative::{
    DFLASH_PAIRING_INDEX_SHA256, DraftAcquisition, DraftBackend, DraftCatalog, DraftIndexStatus,
    DraftOverride, DraftPreparationError, DraftProvenance, DraftSource, DraftTarget,
    DraftUnavailable,
};
use skippy_model_artifact::gguf::GgufCompactMeta;
use skippy_model_hf::HfModelRepository;
use skippy_package_format::{
    DFLASH_PAIRING_INDEX_SCHEMA_VERSION, DFlashBackends, DFlashDraft, DFlashPairing,
    DFlashPairingIndex, DFlashTarget,
};
use skippy_runtime::package::{
    PackageDFlashProposerInfo, PackageGenerationInfo, PackageSpeculativeDecodingInfo,
    PackageSpeculativeProposerInfo, PackageSpeculativeStrategyInfo,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

const BACKEND: DraftBackend = DraftBackend {
    metal: false,
    dflash_context: true,
    automatic_dflash: true,
};
const SHIPPED_INDEX: &[u8] = include_bytes!("../data/dflash-pairings/index.json");

fn metal() -> DraftBackend {
    DraftBackend {
        metal: true,
        ..BACKEND
    }
}

fn qwen() -> GgufCompactMeta {
    GgufCompactMeta {
        architecture: "qwen35".into(),
        embedding_size: 4096,
        layer_count: 33,
        nextn_predict_layers: 1,
        ..Default::default()
    }
}

fn target(metadata: &GgufCompactMeta) -> DraftTarget<'_> {
    DraftTarget {
        metadata: Some(metadata),
        name: None,
        package_generation: None,
    }
}

fn source(file: &str, bytes: &[u8]) -> DFlashDraft {
    DFlashDraft {
        repo: "example/pinned-drafts".into(),
        revision: "a".repeat(40),
        file: file.into(),
        sha256: sha256(bytes),
        bytes: bytes.len() as u64,
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn index(pairings: Vec<DFlashPairing>) -> Vec<u8> {
    serde_json::to_vec(&DFlashPairingIndex {
        schema_version: DFLASH_PAIRING_INDEX_SCHEMA_VERSION,
        pairings,
    })
    .unwrap()
}

fn pairing(draft: DFlashDraft) -> DFlashPairing {
    DFlashPairing {
        target: DFlashTarget {
            architecture: "qwen35".into(),
            layer_count: 32,
            embedding_length: 4096,
            expert_count: 0,
            family: None,
        },
        draft,
        max_draft_tokens: 7,
        backends: DFlashBackends::All,
    }
}

fn package(draft: DFlashDraft) -> PackageGenerationInfo {
    let proposer = |backends| PackageSpeculativeProposerInfo {
        proposer_type: "dflash".into(),
        prediction_depth: None,
        layer_indices: Vec::new(),
        ngram_min: None,
        ngram_max: None,
        max_proposal_tokens: None,
        history_scope: None,
        dflash: Some(PackageDFlashProposerInfo {
            draft: draft.clone(),
            target_layers: vec![2, 32],
            block_size: 8,
            max_draft_tokens: 7,
            backends,
        }),
    };
    PackageGenerationInfo {
        request_defaults: None,
        speculative_decoding: Some(PackageSpeculativeDecodingInfo {
            default: "default".into(),
            proposers: [
                ("metal".into(), proposer(DFlashBackends::Metal)),
                ("other".into(), proposer(DFlashBackends::NotMetal)),
            ]
            .into(),
            strategies: [(
                "default".into(),
                PackageSpeculativeStrategyInfo {
                    strategy_type: "dflash".into(),
                    prediction_depth: None,
                    layer_indices: Vec::new(),
                    window_policy: None,
                    proposer: None,
                    primary: None,
                    extender: None,
                    extension_policy: None,
                    proposers: vec!["metal".into(), "other".into()],
                },
            )]
            .into(),
        }),
    }
}

#[test]
fn explicit_then_package_then_index_then_bootstrap() {
    let metadata = qwen();
    let target = target(&metadata);
    let bootstrap = DraftCatalog::default()
        .resolve(target, BACKEND, DraftOverride::Auto)
        .unwrap();
    assert_eq!(bootstrap.provenance(), &DraftProvenance::Bootstrap);
    assert_eq!(bootstrap.max_draft_tokens(), 15);

    let index_draft = source("index.gguf", b"index");
    let bytes = index(vec![pairing(index_draft.clone())]);
    let digest = sha256(&bytes);
    let catalog = DraftCatalog::from_pinned_index(&bytes, &digest).unwrap();
    let indexed = catalog
        .resolve(target, BACKEND, DraftOverride::Auto)
        .unwrap();
    assert_eq!(
        indexed.provenance(),
        &DraftProvenance::PinnedIndex { sha256: digest }
    );
    assert_eq!(indexed.source(), &DraftSource::Pinned(index_draft));

    let package = package(source("package.gguf", b"package"));
    let target = DraftTarget {
        package_generation: Some(&package),
        ..target
    };
    let packaged = catalog
        .resolve(target, BACKEND, DraftOverride::Auto)
        .unwrap();
    assert_eq!(
        packaged.provenance(),
        &DraftProvenance::Package {
            strategy: "default".into(),
            proposer: "other".into(),
        }
    );
    assert_eq!(packaged.declared_target_layers(), Some([2, 32].as_slice()));
    let metallic = catalog
        .resolve(target, metal(), DraftOverride::PackageStrategy("default"))
        .unwrap();
    assert_eq!(
        metallic.provenance(),
        &DraftProvenance::Package {
            strategy: "default".into(),
            proposer: "metal".into(),
        }
    );
    assert_eq!(metallic.max_draft_tokens(), 6);

    let path = Path::new("/fixtures/supplied-draft.gguf");
    let explicit = catalog
        .resolve(
            target,
            BACKEND,
            DraftOverride::Local {
                path,
                max_draft_tokens: Some(4),
            },
        )
        .unwrap();
    assert_eq!(explicit.provenance(), &DraftProvenance::Explicit);
    assert_eq!(explicit.source(), &DraftSource::Local(path.into()));
    assert_eq!(explicit.max_draft_tokens(), 4);
    assert_eq!(
        catalog.resolve(target, BACKEND, DraftOverride::Disabled),
        Err(DraftUnavailable::ExplicitlyDisabled),
    );
}

#[test]
fn known_family_mismatch_and_package_policy_do_not_fall_through() {
    let catalog = DraftCatalog::default();
    let dsv4 = GgufCompactMeta {
        architecture: "deepseek4".into(),
        embedding_size: 4096,
        layer_count: 43,
        expert_count: 256,
        ..Default::default()
    };
    let unnamed = target(&dsv4);
    let named = DraftTarget {
        name: Some("DeepSeek_V4-Flash-0731"),
        ..unnamed
    };
    let plan = catalog
        .resolve(named, BACKEND, DraftOverride::Auto)
        .unwrap();
    assert_eq!(plan.max_draft_tokens(), 5);
    assert_eq!(
        catalog.resolve(named, metal(), DraftOverride::Auto),
        Err(DraftUnavailable::BackendMismatch)
    );
    assert_eq!(
        catalog.resolve(
            named,
            DraftBackend {
                dflash_context: false,
                ..BACKEND
            },
            DraftOverride::Auto
        ),
        Err(DraftUnavailable::DraftContextUnavailable),
    );
    let unknown = DraftTarget {
        name: Some("DeepSeek-V4-Flash-07315"),
        ..unnamed
    };
    assert_eq!(
        catalog.resolve(unknown, BACKEND, DraftOverride::Auto),
        Err(DraftUnavailable::UnknownTarget)
    );

    let metadata = qwen();
    let mut package = package(source("package.gguf", b"package"));
    let speculative = package.speculative_decoding.as_mut().unwrap();
    speculative.strategies.get_mut("default").unwrap().proposers = vec!["metal".into()];
    let packaged = DraftTarget {
        package_generation: Some(&package),
        ..target(&metadata)
    };
    assert_eq!(
        catalog.resolve(packaged, BACKEND, DraftOverride::Auto),
        Err(DraftUnavailable::BackendMismatch)
    );
    package
        .speculative_decoding
        .as_mut()
        .unwrap()
        .strategies
        .get_mut("default")
        .unwrap()
        .strategy_type = "native-mtp".into();
    let packaged = DraftTarget {
        package_generation: Some(&package),
        ..target(&metadata)
    };
    assert_eq!(
        catalog.resolve(packaged, BACKEND, DraftOverride::Auto),
        Err(DraftUnavailable::PackageSelectsOtherStrategy),
    );

    let bytes = index(Vec::new());
    let empty = DraftCatalog::from_pinned_index(&bytes, &sha256(&bytes)).unwrap();
    assert_eq!(
        empty.resolve(target(&metadata), BACKEND, DraftOverride::Auto),
        Err(DraftUnavailable::UnknownTarget),
    );
}

#[test]
fn absent_or_corrupt_cached_indexes_use_bootstrap_until_verified() {
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(
        DraftCatalog::from_cache(cache.path()).1,
        DraftIndexStatus::Missing
    );
    let snapshots = cache
        .path()
        .join("models--meshllm--dflash-pairings/snapshots");
    let index_at = |revision: &str| {
        snapshots
            .join(revision)
            .join(format!("index/{DFLASH_PAIRING_INDEX_SHA256}.json"))
    };
    let wrong = index_at("wrong");
    std::fs::create_dir_all(wrong.parent().unwrap()).unwrap();
    std::fs::write(&wrong, b"corrupt").unwrap();
    let (catalog, status) = DraftCatalog::from_cache(cache.path());
    assert!(matches!(status, DraftIndexStatus::Invalid(_)));
    let metadata = qwen();
    let bootstrap = catalog
        .resolve(target(&metadata), BACKEND, DraftOverride::Auto)
        .unwrap();
    assert_eq!(bootstrap.provenance(), &DraftProvenance::Bootstrap);

    let right = index_at("right");
    std::fs::create_dir_all(right.parent().unwrap()).unwrap();
    std::fs::write(&right, SHIPPED_INDEX).unwrap();
    let (catalog, status) = DraftCatalog::from_cache(cache.path());
    assert_eq!(status, DraftIndexStatus::Verified);
    let cached = catalog
        .resolve(target(&metadata), BACKEND, DraftOverride::Auto)
        .unwrap();
    assert_eq!(cached.source(), bootstrap.source());
    assert_eq!(
        cached.provenance(),
        &DraftProvenance::PinnedIndex {
            sha256: DFLASH_PAIRING_INDEX_SHA256.into()
        }
    );
    let bad_schema = b"{\"schema_version\":99,\"pairings\":[]}";
    assert!(DraftCatalog::from_pinned_index(bad_schema, &sha256(bad_schema)).is_err());
}

#[tokio::test]
async fn cached_drafts_are_checked_for_size_digest_and_compatibility() {
    let cache = tempfile::tempdir().unwrap();
    let bytes = draft_header(4096, &[2, 32]);
    let draft = source("draft.gguf", &bytes);
    let metadata = qwen();
    let package = package(draft.clone());
    let target = DraftTarget {
        package_generation: Some(&package),
        ..target(&metadata)
    };
    let plan = DraftCatalog::default()
        .resolve(target, BACKEND, DraftOverride::Auto)
        .unwrap();
    let path = cache
        .path()
        .join("models--example--pinned-drafts/snapshots")
        .join(&draft.revision)
        .join(&draft.file);
    let cache_only = || DraftAcquisition::CacheOnly {
        cache: cache.path(),
    };
    assert!(matches!(
        plan.prepare(None, cache_only()).await,
        Err(DraftPreparationError::Unavailable(_))
    ));

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"short").unwrap();
    assert!(matches!(
        plan.prepare(None, cache_only()).await,
        Err(DraftPreparationError::Integrity(_))
    ));
    let mut corrupt = bytes.clone();
    corrupt[0] = 0;
    std::fs::write(&path, corrupt).unwrap();
    assert!(matches!(
        plan.prepare(None, cache_only()).await,
        Err(DraftPreparationError::Integrity(_))
    ));

    std::fs::write(&path, &bytes).unwrap();
    let prepared = plan.prepare(None, cache_only()).await.unwrap();
    assert_eq!(prepared.config().target_layers, [2, 32]);
    assert_eq!(prepared.config().draft_model_path, path);

    let bad = cache.path().join("explicit.gguf");
    let explicit = DraftCatalog::default()
        .resolve(
            target,
            BACKEND,
            DraftOverride::Local {
                path: &bad,
                max_draft_tokens: None,
            },
        )
        .unwrap();
    for bytes in [draft_header(2048, &[2, 32]), draft_header(4096, &[33])] {
        std::fs::write(&bad, bytes).unwrap();
        assert!(matches!(
            explicit.prepare(None, cache_only()).await,
            Err(DraftPreparationError::Incompatible(_)),
        ));
    }
}

#[tokio::test]
async fn unavailable_download_returns_a_typed_failure_without_another_pairing() {
    let cache = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(stream.read_u8().await.unwrap());
        }
        stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8(request).unwrap()
    });
    let repo = HfModelRepository::builder()
        .endpoint(endpoint)
        .cache_dir(cache.path())
        .request_timeout(Duration::from_secs(3))
        .retry_max_attempts(0)
        .build()
        .unwrap();
    let metadata = qwen();
    let plan = DraftCatalog::default()
        .resolve(target(&metadata), BACKEND, DraftOverride::Auto)
        .unwrap();
    assert!(matches!(
        plan.prepare(
            None,
            DraftAcquisition::Download {
                repository: &repo,
                progress: None
            }
        )
        .await,
        Err(DraftPreparationError::Unavailable(_)),
    ));
    let request = tokio::time::timeout(Duration::from_secs(3), server)
        .await
        .unwrap()
        .unwrap();
    let DraftSource::Pinned(draft) = plan.source() else {
        panic!("expected a pinned draft")
    };
    assert!(request.starts_with(&format!(
        "HEAD /{}/resolve/{}/{} ",
        draft.repo, draft.revision, draft.file
    )));
}

fn draft_header(width: u32, layers: &[u32]) -> Vec<u8> {
    fn string(bytes: &mut Vec<u8>, value: &str) {
        bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
        bytes.extend_from_slice(value.as_bytes());
    }
    let mut bytes = b"GGUF".to_vec();
    bytes.extend_from_slice(&3u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&3u64.to_le_bytes());
    string(&mut bytes, "general.architecture");
    bytes.extend_from_slice(&8u32.to_le_bytes());
    string(&mut bytes, "dflash");
    string(&mut bytes, "dflash.embedding_length");
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&width.to_le_bytes());
    string(&mut bytes, "dflash.target_layers");
    bytes.extend_from_slice(&9u32.to_le_bytes());
    bytes.extend_from_slice(&4u32.to_le_bytes());
    bytes.extend_from_slice(&(layers.len() as u64).to_le_bytes());
    for layer in layers {
        bytes.extend_from_slice(&layer.to_le_bytes());
    }
    bytes
}
