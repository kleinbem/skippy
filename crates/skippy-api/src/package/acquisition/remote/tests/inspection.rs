use super::*;
use skippy_model::gguf_metadata::{GgufKv, write_kv};
use skippy_package_format::{
    Artifact, ArtifactCatalog, PACKAGE_SCHEMA_VERSION, PackageManifest, SourceFile, SourceModel,
    TensorCatalog, stage_admission::StageAdmissionDescriptor,
};
use std::sync::atomic::AtomicUsize;

const OLD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const REFERENCE: &str = "hf://owner/repo";
const CARRIER: &str = "shared/metadata.gguf";
const PAYLOAD: &str = "layers/layer-00000.gguf";
const TENSOR: &str = "blk.0.attn_q.weight";

fn carrier() -> Vec<u8> {
    let fields = [
        GgufKv::string("general.architecture", "llama"),
        GgufKv::u32("llama.embedding_length", 4),
        GgufKv::bool("skippy.package.metadata_only", true),
        GgufKv::u32("skippy.package.part_count", 1),
        GgufKv::u32("skippy.package.locator_schema", 1),
        GgufKv::array_u32("skippy.package.tensor_part", vec![0]),
        GgufKv::array_u32("skippy.package.tensor_offset", vec![0]),
        GgufKv::array_u32("skippy.package.tensor_size", vec![32]),
        GgufKv::array_u32("skippy.package.tensor_alignment", vec![32]),
    ];
    let mut bytes = b"GGUF".to_vec();
    bytes.extend(3_u32.to_le_bytes());
    bytes.extend(1_u64.to_le_bytes());
    bytes.extend((fields.len() as u64).to_le_bytes());
    for field in fields {
        write_kv(&mut bytes, &field).unwrap();
    }
    bytes.extend((TENSOR.len() as u64).to_le_bytes());
    bytes.extend(TENSOR.as_bytes());
    bytes.extend(2_u32.to_le_bytes());
    bytes.extend(4_u64.to_le_bytes());
    bytes.extend(4_u64.to_le_bytes());
    bytes.extend(1_u32.to_le_bytes()); // F16
    bytes.extend(0_u64.to_le_bytes());
    bytes.resize(bytes.len().next_multiple_of(32), 0);
    bytes
}

fn v2_files() -> (PackageManifest, BTreeMap<String, Vec<u8>>) {
    let carrier = carrier();
    let payload = vec![42; 32];
    let artifact = |id: &str, path: &str, bytes: &[u8]| Artifact {
        id: id.into(),
        path: path.into(),
        byte_size: bytes.len() as u64,
        sha256: sha256_hex(bytes),
    };
    let source_digest = sha256_hex(b"original model");
    let mut manifest = PackageManifest {
        schema_version: PACKAGE_SCHEMA_VERSION,
        package_id: String::new(),
        model_id: "owner/repo".into(),
        source_model: SourceModel {
            sha256: source_digest.clone(),
            metadata_artifact_id: "metadata".into(),
            repo: None,
            revision: None,
            primary_file: Some("original.gguf".into()),
            canonical_ref: None,
            distribution_id: None,
            files: vec![SourceFile {
                path: "original.gguf".into(),
                byte_size: 123,
                sha256: source_digest,
            }],
        },
        format: "gguf".into(),
        layer_count: 1,
        model_metadata: BTreeMap::new(),
        artifact_catalog: ArtifactCatalog {
            entries: vec![
                artifact("metadata", CARRIER, &carrier),
                artifact("layer", PAYLOAD, &payload),
            ],
        },
        tensor_catalog: TensorCatalog {
            entries: Vec::new(),
        },
        sidecars: Vec::new(),
        publisher_metadata: Vec::new(),
        publisher_defaults: None,
        generation: None,
        native_abi_version: "0.1.52".into(),
        generator_version: "fixture".into(),
        created_at_unix_secs: 1,
    };
    manifest.package_id = manifest.computed_package_id().unwrap();
    manifest.validate_root().unwrap();
    let files = BTreeMap::from([
        (
            "model-package.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (CARRIER.into(), carrier),
        (PAYLOAD.into(), payload),
    ]);
    (manifest, files)
}

fn cache_ref(cache: &Path, revision: &str, files: &BTreeMap<String, Vec<u8>>) -> PathBuf {
    let repo = cache.join("models--owner--repo");
    fs::create_dir_all(repo.join("refs")).unwrap();
    fs::write(repo.join("refs/main"), revision).unwrap();
    let snapshot = repo.join("snapshots").join(revision);
    for (name, bytes) in files {
        let path = snapshot.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    snapshot
}

fn obsolete_cache(cache: &Path) -> PathBuf {
    cache_ref(
        cache,
        OLD,
        &BTreeMap::from([(
            "model-package.json".into(),
            br#"{"schema_version":1}"#.to_vec(),
        )]),
    )
}

fn counted_acquisition(
    cache: &Path,
    server: &PackageServer,
) -> (PackageAcquisition, Arc<AtomicUsize>) {
    let mut acquisition = acquisition(cache, &server.endpoint);
    let count = Arc::new(AtomicUsize::new(0));
    let calls = count.clone();
    let factory = acquisition.build_client.clone();
    acquisition.build_client = Arc::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        factory()
    });
    (acquisition, count)
}

fn unsupported(error: &anyhow::Error, version: u64, source: PackageInspectionSource) {
    assert_eq!(
        error.downcast_ref(),
        Some(&PackageInspectionError::UnsupportedSchema { version, source }),
        "{error:#}"
    );
}

#[test]
fn stale_branch_refreshes_to_v2_and_later_stage_load_stays_on_that_commit() {
    let (manifest, files) = v2_files();
    let cache = tempfile::tempdir().unwrap();
    let stale = obsolete_cache(cache.path());
    let server = PackageServer::new(files.clone());
    let (mut acquisition, calls) = counted_acquisition(cache.path(), &server);
    let progress = RecordingProgress::default();
    acquisition.progress = Arc::new(progress.clone());
    let result = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.source, PackageInspectionSource::Upstream);
    assert_eq!(result.identity.package_ref, format!("{REFERENCE}@{COMMIT}"));
    assert_eq!(
        result.identity.manifest_sha256,
        sha256_hex(&files["model-package.json"])
    );
    assert_eq!(result.package_id, manifest.package_id);
    assert_eq!(result.identity.layer_weight_bytes, vec![32]);
    assert_eq!(result.identity.activation_width, 4);
    assert!(stale.join("model-package.json").is_file());
    assert!(!result.package_dir.join(PAYLOAD).exists());
    assert!(
        progress
            .0
            .lock()
            .unwrap()
            .contains(&format!("ready:{CARRIER}"))
    );
    let requests = server.requests.lock().unwrap().clone();
    assert_eq!(
        requests
            .iter()
            .filter(|path| path.ends_with("/revision/main"))
            .count(),
        1
    );
    assert!(
        requests
            .iter()
            .all(|path| !path.contains("/resolve/main/") && !path.ends_with(PAYLOAD))
    );

    let cached = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::CacheOnly)
        .unwrap();
    assert_eq!(result.identity, cached.identity);
    assert_eq!(cached.source, PackageInspectionSource::Cache);
    // Simulate another process observing a different branch tip after inspection.
    obsolete_cache(cache.path());
    let admission = StageAdmissionDescriptor {
        package_id: result.package_id,
        resident_tensor_ids: vec![TENSOR.into()],
        sidecars: Vec::new(),
    };
    let (root, parts, _) = acquisition
        .resolve_package_v2_stage_to_local(&result.identity.package_ref, &admission)
        .unwrap();
    crate::materialization::ensure_package_manifest_sha(&root, &result.identity.manifest_sha256)
        .unwrap();
    assert_eq!(Path::new(&root), result.package_dir);
    assert_eq!(
        parts,
        [
            result.package_dir.join(CARRIER),
            result.package_dir.join(PAYLOAD)
        ]
    );
    assert_eq!(fs::read(&parts[1]).unwrap(), files[PAYLOAD]);
    let requests = server.requests.lock().unwrap();
    assert!(requests[requests.len() - 1].ends_with(&format!("/resolve/{COMMIT}/{PAYLOAD}")));
}

#[test]
fn stale_branch_with_v1_upstream_returns_typed_error_after_one_refresh() {
    let cache = tempfile::tempdir().unwrap();
    let stale = obsolete_cache(cache.path());
    let server = PackageServer::new(fixture_files());
    let (acquisition, calls) = counted_acquisition(cache.path(), &server);
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap_err();
    unsupported(&error, 1, PackageInspectionSource::Upstream);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(stale.join("model-package.json").is_file());
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .filter(|path| path.contains("/resolve/"))
            .all(|path| path.ends_with("/model-package.json"))
    );
}

#[test]
fn cached_v2_works_with_zero_clients_or_requests_and_no_layers() {
    let cache = tempfile::tempdir().unwrap();
    let (manifest, mut files) = v2_files();
    files.remove(PAYLOAD);
    let snapshot = cache_ref(cache.path(), COMMIT, &files);
    let server = PackageServer::new(files);
    let (acquisition, calls) = counted_acquisition(cache.path(), &server);
    let local = acquisition
        .resolve_and_inspect_package(
            snapshot.to_str().unwrap(),
            PackageInspectionPolicy::CacheOnly,
        )
        .unwrap();
    assert_eq!(local.source, PackageInspectionSource::Local);
    let result = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap();
    assert_eq!(result.source, PackageInspectionSource::Cache);
    assert_eq!(result.package_id, manifest.package_id);
    assert_eq!(result.identity.package_ref, format!("{REFERENCE}@{COMMIT}"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn offline_and_pinned_obsolete_snapshots_never_refresh() {
    let cache = tempfile::tempdir().unwrap();
    obsolete_cache(cache.path());
    let (_, files) = v2_files();
    let server = PackageServer::new(files);
    let (acquisition, calls) = counted_acquisition(cache.path(), &server);
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::CacheOnly)
        .unwrap_err();
    unsupported(&error, 1, PackageInspectionSource::Cache);
    let pinned = format!("{REFERENCE}@{OLD}");
    let error = acquisition
        .resolve_and_inspect_package(&pinned, PackageInspectionPolicy::AllowUpstream)
        .unwrap_err();
    unsupported(&error, 1, PackageInspectionSource::Cache);
    let missing = format!("{REFERENCE}@{COMMIT}");
    let error = acquisition
        .resolve_and_inspect_package(&missing, PackageInspectionPolicy::CacheOnly)
        .unwrap_err();
    assert_eq!(
        error.downcast_ref(),
        Some(&PackageInspectionError::MetadataNotCached { reference: missing })
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn missing_immutable_pin_cannot_be_replaced_by_a_different_upstream_commit() {
    let cache = tempfile::tempdir().unwrap();
    let (_, files) = v2_files();
    let server = PackageServer::new(files);
    let acquisition = acquisition(cache.path(), &server.endpoint);
    let error = acquisition
        .resolve_and_inspect_package(
            &format!("{REFERENCE}@{OLD}"),
            PackageInspectionPolicy::AllowUpstream,
        )
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("differs from requested immutable commit"),
        "{error:#}"
    );
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().all(|path| path.contains(OLD)));
}

#[test]
fn partial_cached_v2_fetches_the_carrier_at_the_cached_commit() {
    let cache = tempfile::tempdir().unwrap();
    let (_, files) = v2_files();
    let cached_manifest = BTreeMap::from([(
        "model-package.json".into(),
        files["model-package.json"].clone(),
    )]);
    cache_ref(cache.path(), COMMIT, &cached_manifest);
    let server = PackageServer::new(files);
    let (acquisition, calls) = counted_acquisition(cache.path(), &server);
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::CacheOnly)
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref(),
        Some(PackageInspectionError::MetadataNotCached { .. })
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let result = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap();
    assert_eq!(result.identity.package_ref, format!("{REFERENCE}@{COMMIT}"));
    assert_eq!(result.source, PackageInspectionSource::Upstream);
    let requests = server.requests.lock().unwrap();
    assert!(requests.iter().any(|path| path.ends_with(CARRIER)));
    assert!(
        requests
            .iter()
            .all(|path| path.contains(COMMIT) && !path.ends_with(PAYLOAD))
    );
}

#[test]
fn malformed_cached_metadata_and_carrier_digest_fail_without_refresh() {
    let cache = tempfile::tempdir().unwrap();
    let (_, files) = v2_files();
    let server = PackageServer::new(files.clone());
    let (acquisition, calls) = counted_acquisition(cache.path(), &server);
    let snapshot = cache_ref(cache.path(), COMMIT, &files);
    for (bad_manifest, message) in [
        (&b"not json"[..], "parse package schema version"),
        (
            &br#"{"schema_version":2,"schema_version":1}"#[..],
            "duplicate field",
        ),
    ] {
        fs::write(snapshot.join("model-package.json"), bad_manifest).unwrap();
        let error = acquisition
            .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
            .unwrap_err();
        assert!(format!("{error:#}").contains(message), "{error:#}");
    }
    let mut altered: PackageManifest =
        serde_json::from_slice(&files["model-package.json"]).unwrap();
    altered.model_id = "owner/other".into();
    fs::write(
        snapshot.join("model-package.json"),
        serde_json::to_vec(&altered).unwrap(),
    )
    .unwrap();
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap_err();
    let validation = error
        .downcast_ref::<skippy_package_format::ValidationErrors>()
        .unwrap();
    assert!(
        validation
            .issues()
            .iter()
            .any(|issue| issue.code
                == skippy_package_format::ValidationCode::PackageIdentityMismatch)
    );
    fs::write(
        snapshot.join("model-package.json"),
        &files["model-package.json"],
    )
    .unwrap();
    fs::write(snapshot.join(CARRIER), vec![0; files[CARRIER].len()]).unwrap();
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("SHA-256 differs"),
        "{error:#}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(server.requests.lock().unwrap().is_empty());
}

#[test]
fn refreshed_carrier_must_match_the_manifest_digest() {
    let cache = tempfile::tempdir().unwrap();
    obsolete_cache(cache.path());
    let (_, mut files) = v2_files();
    files.get_mut(CARRIER).unwrap()[0] = 0;
    let server = PackageServer::new(files);
    let (acquisition, calls) = counted_acquisition(cache.path(), &server);
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap_err();
    assert!(
        format!("{error:#}").contains("SHA-256 differs"),
        "{error:#}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        server
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|path| !path.ends_with(PAYLOAD))
    );
}

#[cfg(unix)]
#[test]
fn cached_metadata_cannot_escape_through_an_artifact_or_parent_symlink() {
    let cache = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (_, mut files) = v2_files();
    let carrier = files.remove(CARRIER).unwrap();
    let snapshot = cache_ref(cache.path(), COMMIT, &files);
    let shared = snapshot.join("shared");
    std::os::unix::fs::symlink(outside.path(), &shared).unwrap();
    let acquisition =
        PackageAcquisition::new(cache.path().to_owned(), || panic!("unexpected Hub client"));
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::AllowUpstream)
        .unwrap_err();
    assert!(format!("{error:#}").contains("parent escapes"), "{error:#}");
    fs::write(outside.path().join("metadata.gguf"), carrier).unwrap();
    let error = acquisition
        .resolve_and_inspect_package(REFERENCE, PackageInspectionPolicy::CacheOnly)
        .unwrap_err();
    assert!(format!("{error:#}").contains("file escapes"), "{error:#}");
}
