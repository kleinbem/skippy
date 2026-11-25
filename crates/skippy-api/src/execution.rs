//! Production in-process embedding against an explicitly supplied dynamic runtime.
//!
//! Use [ModelOptions] for local serving and [SplitOptions] for admitted GGUF
//! execution with an explicit worker endpoint. Compatibility and lifecycle
//! semantics are specified in the production embedding guide.

mod error;
mod model;
mod runtime;
mod snapshot;
mod split;
mod stream;
mod worker;

pub use error::{ExecutionError, ExecutionErrorKind};
pub use model::{ModelHandle, ModelOptions};
pub use runtime::{RuntimeBundle, RuntimeIdentity, StageCompatibility};
pub use skippy_inference_api::ReasoningEffort;
pub use skippy_inference_api::generation::{
    GenerationEvent, GenerationMessage, GenerationRequest, MessageContent,
};
pub use skippy_protocol::{FlashAttentionType, StageDevice};
pub use skippy_runtime::{ActivationBoundaryDesc, WorkloadInfo};
pub use snapshot::{
    ComputeObservation, DraftRequirements, ExecutionCapabilities, ExecutionSnapshot,
    ExecutionState, ResolvedSettings,
};
pub use split::{SplitExecution, SplitOptions};
pub use stream::GenerationStream;
pub use worker::StageWorker;
