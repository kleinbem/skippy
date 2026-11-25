//! Best-effort observations for one blocking [`super::ModelLoadRequest::load`].

mod observe;
pub(super) use observe::run_observed;

/// Whether to use the model-open event path and deliver observations.
#[derive(Default)]
pub enum ModelOpenEvents {
    /// Use the regular load path.
    #[default]
    Disabled,
    /// Use the event-capable load path. `None` runs without a queue or observer.
    Enabled(Option<ModelOpenObserver>),
}

impl ModelOpenEvents {
    /// Enable model-open events and observe them during the load.
    #[must_use]
    pub fn observe(observer: impl FnMut(ModelOpenObservation) + Send + 'static) -> Self {
        Self::Enabled(Some(Box::new(observer)))
    }
}

/// A callback invoked serially on a Skippy-owned drain thread for one load.
///
/// The native producer can drop records when its bounded queue fills. Slow
/// observers allow the queue to fill and delay the load's return while Skippy
/// finishes delivery. Keep the callback short and use nonblocking application
/// ingress. Never wait in the callback for the load to return.
///
/// On a normal return (success or error), Skippy drains pending records, sends a
/// summary, and joins the thread before returning. If the load unwinds, Skippy
/// joins the thread without sending a summary. If the observer panics, delivery
/// stops. Failure to start the drain thread skips delivery. The load result is
/// independent of observation delivery.
pub type ModelOpenObserver = Box<dyn FnMut(ModelOpenObservation) + Send>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelOpenObservation {
    Event(ModelOpenEvent),
    Summary(ModelOpenSummary),
}

/// An event observed during a model open. Terminal events are diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelOpenEvent {
    /// Skippy-assigned process-local ID shared with the summary.
    pub operation_id: u64,
    /// Producer-assigned sequence. Delivery follows queue order and can differ
    /// from sequence order when producers emit concurrently.
    pub sequence: u64,
    pub timestamp_mono_ns: u64,
    /// Runtime-assigned identity for correlation within this process.
    pub native_model_id: u64,
    pub emitter: ModelOpenEmitter,
    pub kind: ModelOpenEventKind,
    /// Diagnostic detail as bytes. May be non-UTF-8 or truncated.
    pub detail: Vec<u8>,
    pub detail_truncated: bool,
    /// The complete native record this event was decoded from, for hosts
    /// that store its provenance: status, stage and session identities, the
    /// numeric summaries, and the raw codes behind `kind` and `emitter`.
    /// A numeric summary is `None` when the runtime did not send it.
    pub source: Box<skippy_runtime::RuntimeEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelOpenEmitter {
    OpenThread,
    WorkerThread,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelOpenEventKind {
    Started,
    Progress(ModelOpenProgress),
    BackendDeviceSelected,
    PhaseChanged,
    MemoryAllocated,
    TensorsOffloaded,
    TokenizerReady,
    AuxComponentReady,
    Finished,
    FailedHandled(ModelOpenFailure),
    /// A record without a recognized model-open kind.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelOpenProgress {
    pub current: u64,
    /// The total when the producer supplies it.
    pub total: Option<u64>,
    pub unit: ModelOpenProgressUnit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelOpenProgressUnit {
    Bytes,
    Items,
    Tensors,
    Steps,
    Unspecified,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ModelOpenFailure {
    InvalidArgument,
    IoError,
    ModelError,
    RuntimeError,
    BackendError,
    Cancelled,
    InternalError,
    Unspecified,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelLoadOutcome {
    Succeeded,
    Failed,
}

/// A terminal event that disagrees with the model load's return.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelOpenContradiction {
    FinishedButLoadFailed,
    FailedHandledButLoadSucceeded,
}

/// Final delivery statistics and the outcome of the enclosing model load.
///
/// The load includes serving setup and validation. An older runtime, a load
/// rejected before opening the model, or loss can leave the observation empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ModelOpenSummary {
    pub operation_id: u64,
    pub outcome: ModelLoadOutcome,
    pub records_observed: u64,
    /// Sequence of the last record dequeued, in arrival order.
    pub last_sequence: Option<u64>,
    /// Valid records dropped by a full queue.
    pub dropped: u64,
    /// Records rejected by the ingress decoder.
    pub rejected: u64,
    pub saw_finished: bool,
    pub saw_failed_handled: bool,
}

impl ModelOpenSummary {
    /// Number of records lost at ingress. Sequence gaps alone do not prove loss.
    #[must_use]
    pub fn lost(&self) -> u64 {
        self.dropped.saturating_add(self.rejected)
    }

    /// A missing terminal event is normal for a best-effort stream.
    #[must_use]
    pub fn contradiction(&self) -> Option<ModelOpenContradiction> {
        match self.outcome {
            ModelLoadOutcome::Failed if self.saw_finished => {
                Some(ModelOpenContradiction::FinishedButLoadFailed)
            }
            ModelLoadOutcome::Succeeded if self.saw_failed_handled => {
                Some(ModelOpenContradiction::FailedHandledButLoadSucceeded)
            }
            _ => None,
        }
    }
}
