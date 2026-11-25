use std::{path::PathBuf, sync::Mutex};

use anyhow::{Context, Result, ensure};
use skippy_native_runtime::{
    InstalledNativeRuntime, NATIVE_RUNTIME_MANIFEST_FILE, NativeRuntimeManifest, RuntimeSelection,
    runtime_release_version,
};
use skippy_protocol::STAGE_PROTOCOL_GENERATION;
use skippy_runtime_install::{current_skippy_abi_version, host_runtime_profile};

use super::{ExecutionError, ExecutionErrorKind};

/// A trusted, immutable native bundle and its out-of-band manifest SHA-256.
#[derive(Clone, Debug)]
pub struct RuntimeBundle {
    directory: PathBuf,
    manifest_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeIdentity {
    release: String,
    artifact: String,
    backend: String,
    manifest_sha256: String,
    compatibility: StageCompatibility,
}

/// Exact identities exchanged in the host's stage admission control plane.
/// A wire readiness probe alone cannot attest the peer's native recipe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StageCompatibility {
    generation: u32,
    llama_upstream: String,
    patch_queue_sha256: String,
    native_abi: String,
}

impl StageCompatibility {
    /// Decode compatibility evidence obtained from a worker's deployment.
    pub fn new(
        generation: u32,
        llama_upstream: impl Into<String>,
        patch_queue_sha256: impl Into<String>,
        native_abi: impl Into<String>,
    ) -> Self {
        Self {
            generation,
            llama_upstream: llama_upstream.into(),
            patch_queue_sha256: patch_queue_sha256.into(),
            native_abi: native_abi.into(),
        }
    }

    pub fn current() -> Self {
        Self::new(
            STAGE_PROTOCOL_GENERATION,
            env!("SKIPPY_LLAMA_UPSTREAM_SHA"),
            env!("SKIPPY_PATCH_QUEUE_SHA256"),
            current_skippy_abi_version(),
        )
    }

    pub fn generation(&self) -> u32 {
        self.generation
    }

    pub fn llama_upstream(&self) -> &str {
        &self.llama_upstream
    }

    pub fn patch_queue_sha256(&self) -> &str {
        &self.patch_queue_sha256
    }

    pub fn native_abi(&self) -> &str {
        &self.native_abi
    }

    pub fn require_peer(&self, peer: &Self) -> Result<(), ExecutionError> {
        if self != peer {
            return Err(ExecutionError::new(
                ExecutionErrorKind::InvalidAdmission,
                anyhow::anyhow!("stage compatibility mismatch: local {self:?}, peer {peer:?}"),
            ));
        }
        Ok(())
    }
}

impl RuntimeIdentity {
    pub fn release(&self) -> &str {
        &self.release
    }

    pub fn artifact(&self) -> &str {
        &self.artifact
    }

    /// Backend flavor from the selected bundle. Individual tensor placement can vary.
    pub fn backend(&self) -> &str {
        &self.backend
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_sha256
    }

    pub fn stage_compatibility(&self) -> &StageCompatibility {
        &self.compatibility
    }
}

static LOADED: Mutex<Option<(PathBuf, RuntimeIdentity)>> = Mutex::new(None);

impl RuntimeBundle {
    pub fn new(
        directory: impl Into<PathBuf>,
        manifest_sha256: impl Into<String>,
    ) -> Result<Self, ExecutionError> {
        let directory = directory.into();
        let manifest_sha256 = manifest_sha256.into();
        if directory.as_os_str().is_empty()
            || manifest_sha256.len() != 64
            || !manifest_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(ExecutionError::new(
                ExecutionErrorKind::InvalidOptions,
                anyhow::anyhow!("supply a runtime bundle directory and a SHA-256 manifest digest"),
            ));
        }
        Ok(Self {
            directory,
            manifest_sha256: manifest_sha256.to_ascii_lowercase(),
        })
    }

    pub(super) fn activate(&self) -> Result<RuntimeIdentity, ExecutionError> {
        self.activate_inner()
            .map_err(|error| ExecutionError::new(ExecutionErrorKind::IncompatibleRuntime, error))
    }

    fn activate_inner(&self) -> Result<RuntimeIdentity> {
        let root = self
            .directory
            .canonicalize()
            .context("resolve runtime bundle")?;
        let mut loaded = LOADED.lock().expect("runtime activation mutex poisoned");
        if let Some((path, identity)) = loaded.as_ref() {
            ensure!(
                *path == root && identity.manifest_sha256 == self.manifest_sha256,
                "a different runtime bundle is already active in this process"
            );
            return Ok(identity.clone());
        }
        ensure!(
            !skippy_runtime::native_runtime_loaded(),
            "the process already loaded a runtime outside the embedding contract"
        );
        let path = root.join(NATIVE_RUNTIME_MANIFEST_FILE);
        ensure!(
            crate::package::file_sha256(&path)? == self.manifest_sha256,
            "runtime manifest digest mismatch at {}",
            path.display()
        );
        verify_recipe(&path)?;
        let manifest = NativeRuntimeManifest::read_from_dir(&root)?;
        let release = runtime_release_version();
        ensure!(
            manifest.runtime.release_version.as_deref() == Some(release),
            "runtime bundle must declare release {release}"
        );
        let artifact = InstalledNativeRuntime {
            release_version: release.to_owned(),
            native_runtime_id: manifest.runtime.id.clone(),
            flavor: manifest.runtime.backend.kind.to_string(),
            path: root.clone(),
            manifest,
        };
        let abi = current_skippy_abi_version();
        let plan = skippy_runtime_install::startup::select_local_native_runtime_plan(
            std::slice::from_ref(&artifact),
            &host_runtime_profile(),
            release,
            Some(&abi),
            &RuntimeSelection::Id(artifact.native_runtime_id.clone()),
        )?
        .with_context(|| {
            format!(
                "runtime {} is incompatible with host or ABI {abi}",
                artifact.native_runtime_id
            )
        })?;
        // The supplied bundle is trusted by the host. Its pinned manifest,
        // runtime compatibility and declared payloads were all verified above.
        unsafe { skippy_runtime::load_native_runtime_libraries(&plan.libraries) }
            .with_context(|| format!("load native runtime {}", plan.native_runtime_id))?;
        let identity = RuntimeIdentity {
            release: plan.release_version,
            artifact: plan.native_runtime_id,
            backend: artifact.flavor,
            manifest_sha256: self.manifest_sha256.clone(),
            compatibility: StageCompatibility::current(),
        };
        *loaded = Some((root, identity.clone()));
        Ok(identity)
    }
}

fn verify_recipe(path: &std::path::Path) -> Result<()> {
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let build = &manifest["build"];
    let native_digest = env!("SKIPPY_NATIVE_PATCH_DIGEST");
    ensure!(
        !native_digest.is_empty()
            && build["llama_upstream_sha"].as_str() == Some(env!("SKIPPY_LLAMA_UPSTREAM_SHA"))
            && build["llama_patch_digest"].as_str() == Some(native_digest),
        "runtime bundle recipe differs from this Skippy source revision"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runtime_recipe_is_checked_before_opening_libraries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(NATIVE_RUNTIME_MANIFEST_FILE);
        let manifest = json!({
            "build": {
                "llama_upstream_sha": env!("SKIPPY_LLAMA_UPSTREAM_SHA"),
                "llama_patch_digest": "0".repeat(64),
            }
        });
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let digest = crate::package::file_sha256(&path).unwrap();
        let bundle = RuntimeBundle::new(dir.path(), digest).unwrap();
        let error = bundle.activate().unwrap_err();
        assert_eq!(error.kind(), ExecutionErrorKind::IncompatibleRuntime);
        assert!(error.to_string().contains("recipe differs"));
        assert!(LOADED.lock().unwrap().is_none());
        assert!(!skippy_runtime::native_runtime_loaded());
    }

    #[test]
    fn stage_peers_need_the_exact_generation_and_recipe() {
        let current = StageCompatibility::current();
        current.require_peer(&current).unwrap();
        let mut different = current.clone();
        different.generation -= 1;
        assert_eq!(
            current.require_peer(&different).unwrap_err().kind(),
            ExecutionErrorKind::InvalidAdmission
        );
        different.generation = current.generation;
        different.patch_queue_sha256 = "0".repeat(64);
        assert_eq!(
            current.require_peer(&different).unwrap_err().kind(),
            ExecutionErrorKind::InvalidAdmission
        );
    }
}
