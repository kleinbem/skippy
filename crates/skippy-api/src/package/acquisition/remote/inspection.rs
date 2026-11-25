use super::{
    ForwardProgress, PackageAcquisition, Progress, StagePackageRef, download_layer_package_file,
    layer_package_progress_label, lock_layer_package_downloads,
};
use crate::materialization::safe_manifest_file_path;
use crate::package::{PACKAGE_V2_MANIFEST, SkippyPackageIdentity, metadata};
use anyhow::{Context, Result, ensure};
use skippy_package_format::{PACKAGE_SCHEMA_VERSION, PackageManifest};
use std::{
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Network policy for metadata inspection. Local packages always use local files.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PackageInspectionPolicy {
    /// Use only the requested package's local files or exact cached snapshot.
    #[default]
    CacheOnly,
    /// Fetch missing metadata and refresh a mutable revision once if its cached schema is v1.
    AllowUpstream,
}

/// Where inspection obtained the package metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PackageInspectionSource {
    /// The caller supplied a local package directory.
    Local,
    /// All metadata came from the exact cached Hub snapshot.
    Cache,
    /// Inspection used the Hub to resolve a snapshot or acquire missing metadata.
    Upstream,
}

/// Verified metadata and the inputs needed to plan and acquire an admitted stage.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct InspectedPackage {
    /// Model identity, including the resolved reference and the raw manifest's SHA-256.
    pub identity: SkippyPackageIdentity,
    /// Content-derived package ID used by stage admission descriptors.
    pub package_id: String,
    /// Local directory containing the verified manifest and metadata carrier.
    pub package_dir: PathBuf,
    /// How inspection obtained the metadata.
    pub source: PackageInspectionSource,
}

/// Policy and schema failures returned by [`PackageAcquisition::resolve_and_inspect_package`].
#[derive(Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PackageInspectionError {
    /// The requested Hub snapshot or its metadata is missing under `CacheOnly`.
    MetadataNotCached { reference: String },
    /// The manifest declares an unsupported schema.
    UnsupportedSchema {
        version: u64,
        source: PackageInspectionSource,
    },
}

impl fmt::Display for PackageInspectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MetadataNotCached { reference } => {
                write!(
                    f,
                    "package metadata is unavailable in the cache for {reference}"
                )
            }
            Self::UnsupportedSchema { version, source } => write!(
                f,
                "{source:?} package schema {version} is unsupported (expected {PACKAGE_SCHEMA_VERSION})"
            ),
        }
    }
}

impl std::error::Error for PackageInspectionError {}

impl PackageAcquisition {
    /// Resolve and verify the manifest and metadata carrier of a supported package.
    ///
    /// For Hub packages, `identity.package_ref` contains a full commit pin. Use it
    /// with [`Self::resolve_package_v2_stage_to_local`] after producing an exact
    /// admission for `package_id`. Local results carry a canonical directory reference
    /// and verified digests. Inspection always checks the carrier bytes and only
    /// fetches the manifest and carrier. The caller's client must use `hub_cache`.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use skippy_api::package::acquisition::remote::{PackageAcquisition, PackageInspectionPolicy};
    ///
    /// # fn plan(acquisition: &PackageAcquisition) -> anyhow::Result<()> {
    /// let package = acquisition.resolve_and_inspect_package(
    ///     "hf://owner/model@main",
    ///     PackageInspectionPolicy::AllowUpstream,
    /// )?;
    /// println!("Plan package {} at {}", package.package_id, package.identity.package_ref);
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// An unavailable offline snapshot or an unsupported schema yields a
    /// [`PackageInspectionError`] in the error chain. Invalid references, corrupt
    /// metadata, filesystem failures and Hub failures return their own errors.
    pub fn resolve_and_inspect_package(
        &self,
        package_ref: &str,
        policy: PackageInspectionPolicy,
    ) -> Result<InspectedPackage> {
        match StagePackageRef::parse(package_ref)? {
            StagePackageRef::LocalPackage(path) => inspect_local_package(&path),
            StagePackageRef::HuggingFacePackage { repo, revision } => {
                self.inspect_hf_package(&repo, revision.as_deref().unwrap_or("main"), policy)
            }
            StagePackageRef::SyntheticDirectGguf(_) => {
                anyhow::bail!("package inspection requires a package manifest")
            }
        }
        .with_context(|| format!("inspect package {package_ref}"))
    }

    fn inspect_hf_package(
        &self,
        repo: &str,
        revision: &str,
        policy: PackageInspectionPolicy,
    ) -> Result<InspectedPackage> {
        let cached_commit = hf_hub::cache::resolve_cached_revision(
            &self.hub_cache,
            repo,
            hf_hub::RepoTypeModel,
            revision,
        )
        .context("resolve exact cached package revision")?;
        let repo_folder = format!("models--{}", repo.replace('/', "--"));
        let mut acquire_revision = revision;
        if let Some(commit) = &cached_commit {
            let snapshot = HubSnapshot::new(&self.hub_cache, &repo_folder, commit);
            if let Some(path) = snapshot.file(Path::new(PACKAGE_V2_MANIFEST))? {
                match PackageRoot::read(&path, PackageInspectionSource::Cache) {
                    Ok(root) => {
                        if let Some(carrier) = snapshot.file(&root.carrier_path()?)? {
                            return root.inspect(
                                snapshot.reference(repo),
                                snapshot.dir,
                                carrier,
                                PackageInspectionSource::Cache,
                            );
                        }
                        // Finish the same snapshot when its valid root is already cached.
                        acquire_revision = commit;
                    }
                    Err(error)
                        if policy == PackageInspectionPolicy::AllowUpstream
                            && !is_commit(revision)
                            && matches!(
                                error.downcast_ref(),
                                Some(PackageInspectionError::UnsupportedSchema { version: 1, .. })
                            ) => {}
                    Err(error) => return Err(error),
                }
            }
        }
        if policy == PackageInspectionPolicy::CacheOnly {
            return Err(PackageInspectionError::MetadataNotCached {
                reference: format!("hf://{repo}@{revision}"),
            }
            .into());
        }
        let (acquisition, repo, revision) =
            (self.clone(), repo.to_owned(), acquire_revision.to_owned());
        skippy_model_hf::blocking::run_hf_sync(move || {
            acquisition.inspect_hf_upstream(&repo, &repo_folder, &revision)
        })
    }

    fn inspect_hf_upstream(
        &self,
        repo: &str,
        repo_folder: &str,
        revision: &str,
    ) -> Result<InspectedPackage> {
        let _download_guard = lock_layer_package_downloads();
        let api = (self.build_client)()?;
        let (owner, name) = repo.split_once('/').context("invalid HF repo format")?;
        let model_api = api.model(owner, name);
        let label = layer_package_progress_label(repo, revision);
        let manifest_progress = self.progress.file(&label, PACKAGE_V2_MANIFEST, None, 0);
        manifest_progress.ensuring();
        let progress: Progress = Arc::new(ForwardProgress(manifest_progress.clone())).into();
        // The Hub snapshot operation resolves the revision once and transfers the
        // selected manifest at that commit, even if the named branch moves mid-request.
        let dir = model_api
            .snapshot_download()
            .revision(revision)
            .allow_patterns(vec![PACKAGE_V2_MANIFEST.to_owned()])
            .progress(progress)
            .send()
            .context("acquire package manifest snapshot from Hub")?;
        let commit = dir
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|commit| is_commit(commit))
            .context("Hub package snapshot is missing an immutable commit")?;
        ensure!(
            !is_commit(revision) || commit.eq_ignore_ascii_case(revision),
            "Hub package snapshot differs from the requested immutable commit"
        );
        let snapshot = HubSnapshot::new(&self.hub_cache, repo_folder, commit);
        ensure!(
            dir.canonicalize()? == snapshot.dir.canonicalize()?,
            "Hub client returned a snapshot outside the configured cache"
        );
        let manifest_path = snapshot
            .file(Path::new(PACKAGE_V2_MANIFEST))?
            .context("Hub package snapshot is missing its manifest")?;
        manifest_progress.ready(&manifest_path);
        drop(manifest_progress);
        let source = PackageInspectionSource::Upstream;
        let root = PackageRoot::read(&manifest_path, source)?;
        let artifact = metadata::metadata_artifact(&root.manifest)?;
        let carrier_path = root.carrier_path()?;
        let carrier = match snapshot.file(&carrier_path)? {
            Some(path) => path,
            None => {
                let scope = self.progress.batch(&label, 2);
                download_layer_package_file(
                    &model_api,
                    commit,
                    &label,
                    &artifact.path,
                    Some(artifact.byte_size),
                    scope.as_ref(),
                    1,
                )?;
                snapshot
                    .file(&carrier_path)?
                    .context("downloaded metadata artifact is missing")?
            }
        };
        root.inspect(snapshot.reference(repo), snapshot.dir, carrier, source)
    }
}

fn inspect_local_package(path: &Path) -> Result<InspectedPackage> {
    let dir = path
        .canonicalize()
        .context("canonicalize local package directory")?;
    let source = PackageInspectionSource::Local;
    let manifest_path = confined_file(&dir.join(PACKAGE_V2_MANIFEST), &dir, None)?
        .context("local package manifest is missing")?;
    let root = PackageRoot::read(&manifest_path, source)?;
    let carrier = confined_file(&dir.join(root.carrier_path()?), &dir, None)?
        .context("local metadata artifact is missing")?;
    root.inspect(dir.to_string_lossy().into_owned(), dir, carrier, source)
}

struct PackageRoot {
    bytes: Vec<u8>,
    manifest: PackageManifest,
}

impl PackageRoot {
    fn read(path: &Path, source: PackageInspectionSource) -> Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("read package manifest {}", path.display()))?;
        // Deserialize a typed envelope so missing, malformed and duplicate schema
        // fields stay errors, including when inspecting an obsolete snapshot.
        #[derive(serde::Deserialize)]
        struct Envelope {
            schema_version: u64,
        }
        let Envelope { schema_version } =
            serde_json::from_slice(&bytes).context("parse package schema version")?;
        if schema_version != u64::from(PACKAGE_SCHEMA_VERSION) {
            return Err(PackageInspectionError::UnsupportedSchema {
                version: schema_version,
                source,
            }
            .into());
        }
        let manifest: PackageManifest =
            serde_json::from_slice(&bytes).context("parse package-v2 manifest")?;
        manifest
            .validate_root()
            .context("validate package-v2 manifest")?;
        Ok(Self { bytes, manifest })
    }

    fn carrier_path(&self) -> Result<PathBuf> {
        safe_manifest_file_path(&metadata::metadata_artifact(&self.manifest)?.path)
    }

    fn inspect(
        self,
        reference: String,
        package_dir: PathBuf,
        carrier: PathBuf,
        source: PackageInspectionSource,
    ) -> Result<InspectedPackage> {
        let package_id = self.manifest.package_id.clone();
        let identity = metadata::verified_metadata_identity(
            reference,
            &self.bytes,
            self.manifest,
            carrier,
            None,
        )?;
        Ok(InspectedPackage {
            identity,
            package_id,
            package_dir,
            source,
        })
    }
}

struct HubSnapshot<'a> {
    cache: &'a Path,
    repo_folder: &'a str,
    commit: &'a str,
    dir: PathBuf,
}

impl<'a> HubSnapshot<'a> {
    fn new(cache: &'a Path, repo_folder: &'a str, commit: &'a str) -> Self {
        let dir = cache.join(repo_folder).join("snapshots").join(commit);
        Self {
            cache,
            repo_folder,
            commit,
            dir,
        }
    }

    fn reference(&self, repo: &str) -> String {
        format!("hf://{repo}@{}", self.commit)
    }

    fn file(&self, relative: &Path) -> Result<Option<PathBuf>> {
        let canonical = match self.dir.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("resolve cached package directory"),
        };
        let repo = self.cache.canonicalize()?.join(self.repo_folder);
        ensure!(
            canonical == repo.join("snapshots").join(self.commit),
            "cached package directory escapes the requested snapshot"
        );
        confined_file(
            &self.dir.join(relative),
            &canonical,
            Some(&repo.join("blobs")),
        )
    }
}

fn is_commit(revision: &str) -> bool {
    revision.len() == 40 && revision.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn confined_file(path: &Path, directory: &Path, blobs: Option<&Path>) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // The caller can download a missing file. Check its nearest existing
            // parent so acquisition cannot follow a directory link out of the snapshot.
            for parent in path.ancestors().skip(1) {
                match parent.canonicalize() {
                    Ok(parent) => {
                        ensure!(
                            parent.starts_with(directory),
                            "package file parent escapes its package directory: {}",
                            path.display()
                        );
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error).context("resolve package file parent"),
                }
            }
            return Ok(None);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("stat package file {}", path.display()));
        }
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolve package file {}", path.display()))?;
    ensure!(
        canonical.starts_with(directory) || blobs.is_some_and(|dir| canonical.starts_with(dir)),
        "package file escapes its package directory and cache: {}",
        path.display()
    );
    ensure!(
        canonical.is_file(),
        "package path is not a file: {}",
        path.display()
    );
    Ok(Some(path.to_owned()))
}
