//! Shared native runtime manifest, resolution, and cache policy.

mod cache;
mod flavor;
pub mod host;
mod load_plan;
mod manifest;
mod resolver;
mod unused_gpu;

pub use cache::{
    CachePrunePlan, GPU_BENCHMARK_TOOL_PATH, InstalledNativeRuntime, LenientInstalledScan,
    NativeRuntimeCache, NativeRuntimeCacheRoot, NativeRuntimePruneMode, SkippedNativeRuntime,
    native_runtime_cache_root,
};
pub use flavor::{
    CudaRuntimeRequirements, NativeRuntimeBackend, NativeRuntimeBackendKind, NativeRuntimeFlavor,
    NativeRuntimeFlavorParseError, RocmRuntimeRequirements, VulkanRuntimeRequirements,
};
pub use host::{
    HostCudaProfile, HostGpuProfile, HostRocmProfile, HostRuntimeProfile, HostVulkanProfile,
};
pub use load_plan::NativeRuntimeLoadPlan;
pub use manifest::{
    NATIVE_RUNTIME_MANIFEST_FILE, NativeRuntimeArtifact, NativeRuntimeManifest,
    NativeRuntimePlatform, NativeRuntimeReleaseManifest,
};
pub use resolver::{
    CandidateEvaluation, CandidateRejection, NativeRuntimeResolution, NativeRuntimeResolver,
    NativeRuntimeSource, RuntimeSelection, evaluate_native_runtime_artifact,
    has_startup_compatibility_metadata, select_native_runtime,
    select_native_runtime_for_skippy_abi, select_native_runtime_from_artifacts,
};
pub use unused_gpu::{
    UnusedGpuBackend, describe_unused_gpu_backends, describe_unused_gpus, unused_gpu_backends,
};

/// Native runtime artifact release: always the Skippy workspace version.
///
/// Every Skippy release is therefore a new runtime identity and cache
/// directory, so a runtime cached by an earlier release is never selected.
/// The packaging producer stamps the same workspace version. Native binary
/// compatibility is checked separately by ABI.
pub fn runtime_release_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
