use skippy_protocol::StageConfig;
use skippy_serving::compute_meter::StageComputeSnapshot;

use super::{ActivationBoundaryDesc, ExecutionError, ExecutionErrorKind, FlashAttentionType};
use super::{RuntimeIdentity, StageDevice, WorkloadInfo};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExecutionState {
    Starting,
    Ready,
    Draining,
    Stopped,
    Failed,
}

/// Capabilities are fixed for the lifetime of a successfully loaded handle.
#[derive(Clone, Debug)]
pub struct ExecutionCapabilities {
    pub(super) workload: Option<WorkloadInfo>,
    pub(super) streaming: bool,
    pub(super) staged: bool,
}

impl ExecutionCapabilities {
    pub const STREAMED_GENERATION: &str = "streamed-generation-v1";
    pub const ADMITTED_STAGE: &str = "admitted-stage-v1";

    pub fn workload(&self) -> Option<WorkloadInfo> {
        self.workload
    }

    pub fn supports(&self, capability: &str) -> bool {
        match capability {
            Self::STREAMED_GENERATION => self.streaming,
            Self::ADMITTED_STAGE => self.staged,
            _ => false,
        }
    }

    pub fn require(&self, capability: &str) -> Result<(), ExecutionError> {
        if self.supports(capability) {
            return Ok(());
        }
        Err(ExecutionError::new(
            ExecutionErrorKind::UnsupportedCapability,
            anyhow::anyhow!("execution capability {capability:?} is unavailable"),
        ))
    }
}

/// Per-stage counters accumulated since this load. Values are sampled
/// independently. A present zero means the meter has recorded no such work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComputeObservation(StageComputeSnapshot);

impl From<StageComputeSnapshot> for ComputeObservation {
    fn from(snapshot: StageComputeSnapshot) -> Self {
        Self(snapshot)
    }
}

impl ComputeObservation {
    /// Native compute time recorded by the stage, in nanoseconds.
    pub fn busy_nanos(&self) -> u64 {
        self.0.busy_nanos
    }

    /// Compute operations counted in the busy time.
    pub fn operations(&self) -> u64 {
        self.0.operations
    }

    /// Serial decode steps. Excludes positions computed in a verify window.
    pub fn decode_tokens(&self) -> u64 {
        self.0.decode_tokens
    }

    /// Positions computed in speculative verify windows, including rejected
    /// positions. This optional observation is independent of committed output.
    pub fn verify_tokens(&self) -> Option<u64> {
        Some(self.0.verify_tokens)
    }
}

#[derive(Clone, Debug)]
pub struct DraftRequirements {
    pub(super) target_layers: Vec<u32>,
    pub(super) context_window: Option<u32>,
    pub(super) feature_encodings: Vec<String>,
}

impl DraftRequirements {
    pub fn target_layers(&self) -> &[u32] {
        &self.target_layers
    }

    /// Sliding-window size in token positions. `None` means full context.
    pub fn context_window(&self) -> Option<u32> {
        self.context_window
    }

    pub fn feature_encodings(&self) -> &[String] {
        &self.feature_encodings
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedSettings {
    pub(super) context_tokens: u32,
    pub(super) lanes: usize,
    pub(super) batch_tokens: Option<u32>,
    pub(super) microbatch_tokens: Option<u32>,
    pub(super) flash_attention: FlashAttentionType,
}

impl ResolvedSettings {
    pub(super) fn new(config: &StageConfig, effective_lanes: usize) -> Self {
        Self {
            context_tokens: config.ctx_size,
            lanes: effective_lanes,
            batch_tokens: config.n_batch,
            microbatch_tokens: config.n_ubatch,
            flash_attention: config.flash_attn_type,
        }
    }

    pub fn context_tokens(&self) -> u32 {
        self.context_tokens
    }

    pub fn lanes(&self) -> usize {
        self.lanes
    }

    /// Explicit batch bound; absent when the native runtime resolves it.
    pub fn batch_tokens(&self) -> Option<u32> {
        self.batch_tokens
    }

    /// Explicit microbatch bound; absent when the native runtime resolves it.
    pub fn microbatch_tokens(&self) -> Option<u32> {
        self.microbatch_tokens
    }

    /// Resolved policy, including automatic selection by the native runtime.
    pub fn flash_attention(&self) -> FlashAttentionType {
        self.flash_attention
    }
}

/// Point-in-time execution observations. This value has no transport schema.
#[derive(Clone, Debug)]
pub struct ExecutionSnapshot {
    pub(super) state: ExecutionState,
    pub(super) runtime: RuntimeIdentity,
    pub(super) layers: std::ops::Range<u32>,
    pub(super) selected_device: Option<StageDevice>,
    pub(super) settings: ResolvedSettings,
    pub(super) input: Option<ActivationBoundaryDesc>,
    pub(super) output: Option<ActivationBoundaryDesc>,
    pub(super) draft: Option<DraftRequirements>,
    pub(super) compute: Option<ComputeObservation>,
    pub(super) failure: Option<String>,
}

impl ExecutionSnapshot {
    pub fn state(&self) -> ExecutionState {
        self.state
    }

    pub fn runtime(&self) -> &RuntimeIdentity {
        &self.runtime
    }

    pub fn layers(&self) -> std::ops::Range<u32> {
        self.layers.clone()
    }

    /// Explicit device bound by the loader, when available. An automatic
    /// placement does not imply a single selected device.
    pub fn selected_device(&self) -> Option<&StageDevice> {
        self.selected_device.as_ref()
    }

    pub fn settings(&self) -> &ResolvedSettings {
        &self.settings
    }

    pub fn input_boundary(&self) -> Option<&ActivationBoundaryDesc> {
        self.input.as_ref()
    }

    pub fn output_boundary(&self) -> Option<&ActivationBoundaryDesc> {
        self.output.as_ref()
    }

    pub fn draft(&self) -> Option<&DraftRequirements> {
        self.draft.as_ref()
    }

    pub fn compute(&self) -> Option<ComputeObservation> {
        self.compute
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skippy_serving::compute_meter::StageComputeMeter;

    #[test]
    fn available_zero_and_speculative_work_are_distinct_from_unavailable() {
        let meter = StageComputeMeter::default();
        let unavailable: Option<ComputeObservation> = None;
        let measured_zero = ComputeObservation::from(meter.snapshot());
        assert_ne!(unavailable, Some(measured_zero));
        assert_eq!(measured_zero.operations(), 0);
        meter.record(std::time::Duration::from_nanos(7));
        meter.record_verify_tokens(2);
        let observed = ComputeObservation::from(meter.snapshot());
        assert_eq!(observed.busy_nanos(), 7);
        assert_eq!(observed.operations(), 1);
        assert_eq!(observed.decode_tokens(), 0);
        assert_eq!(observed.verify_tokens(), Some(2));
    }

    #[test]
    fn unknown_required_capabilities_fail_closed() {
        let capabilities = ExecutionCapabilities {
            workload: None,
            streaming: true,
            staged: false,
        };
        capabilities
            .require(ExecutionCapabilities::STREAMED_GENERATION)
            .unwrap();
        for name in [ExecutionCapabilities::ADMITTED_STAGE, "future-strategy"] {
            assert!(!capabilities.supports(name));
            assert_eq!(
                capabilities.require(name).unwrap_err().kind(),
                ExecutionErrorKind::UnsupportedCapability
            );
        }
    }
}
