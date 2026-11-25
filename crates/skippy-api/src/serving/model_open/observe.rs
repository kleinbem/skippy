use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use skippy_runtime::{
    MODEL_OPEN_RECORD_CAPACITY, ModelOpenEventQueue, NativeEventRecord, RuntimeEventEmitterKind,
    RuntimeEventFailureCode, RuntimeEventKind, RuntimeEventProgressUnit, next_operation_id,
};

use super::{
    ModelLoadOutcome, ModelOpenEmitter, ModelOpenEvent, ModelOpenEventKind, ModelOpenFailure,
    ModelOpenObservation, ModelOpenProgress, ModelOpenProgressUnit, ModelOpenSummary,
};

const DRAIN_INTERVAL: Duration = Duration::from_millis(20);

pub(in crate::serving) fn run_observed<T, E>(
    observer: Option<impl FnMut(ModelOpenObservation) + Send>,
    load: impl FnOnce(Option<Arc<ModelOpenEventQueue>>) -> Result<T, E>,
) -> Result<T, E> {
    let Some(observer) = observer else {
        return load(None);
    };
    let queue = ModelOpenEventQueue::new(next_operation_id());
    thread::scope(|scope| {
        let (send_outcome, outcome) = mpsc::channel();
        let drainer = thread::Builder::new()
            .name("skippy-model-open".into())
            .spawn_scoped(scope, || drain(&queue, outcome, observer));
        let drainer = match drainer {
            Ok(drainer) => drainer,
            Err(error) => {
                tracing::warn!(%error, "could not start the model-open observer");
                return load(None);
            }
        };

        let result = load(Some(Arc::clone(&queue)));
        let outcome = if result.is_ok() {
            ModelLoadOutcome::Succeeded
        } else {
            ModelLoadOutcome::Failed
        };
        let _ = send_outcome.send(outcome);
        if drainer.join().is_err() {
            tracing::warn!("model-open observation thread panicked");
        }
        result
    })
}

#[derive(Default)]
struct ObservedRecords {
    count: u64,
    last_sequence: Option<u64>,
    saw_finished: bool,
    saw_failed_handled: bool,
}

fn drain(
    queue: &ModelOpenEventQueue,
    outcome: mpsc::Receiver<ModelLoadOutcome>,
    mut observer: impl FnMut(ModelOpenObservation),
) {
    let mut observed = ObservedRecords::default();
    let mut records = Vec::new();
    let operation_id = queue.operation_id().0;
    loop {
        // The single sender is dropped if the load unwinds. Both a result and
        // disconnection mean the producer has stopped and this drain is final.
        let state = outcome.recv_timeout(DRAIN_INTERVAL);
        queue.drain(&mut records, MODEL_OPEN_RECORD_CAPACITY);
        for record in records.drain(..) {
            let event = decode(operation_id, &record);
            observed.count += 1;
            observed.last_sequence = Some(event.sequence);
            observed.saw_finished |= matches!(event.kind, ModelOpenEventKind::Finished);
            observed.saw_failed_handled |=
                matches!(event.kind, ModelOpenEventKind::FailedHandled(_));
            if !deliver(&mut observer, ModelOpenObservation::Event(event)) {
                return;
            }
        }
        match state {
            Ok(outcome) => {
                deliver(
                    &mut observer,
                    ModelOpenObservation::Summary(ModelOpenSummary {
                        operation_id,
                        outcome,
                        records_observed: observed.count,
                        last_sequence: observed.last_sequence,
                        dropped: queue.dropped(),
                        rejected: queue.rejected(),
                        saw_finished: observed.saw_finished,
                        saw_failed_handled: observed.saw_failed_handled,
                    }),
                );
                return;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn deliver(
    observer: &mut impl FnMut(ModelOpenObservation),
    observation: ModelOpenObservation,
) -> bool {
    if catch_unwind(AssertUnwindSafe(|| observer(observation))).is_err() {
        tracing::warn!("model-open observer panicked; stopping observation delivery");
        return false;
    }
    true
}

fn decode(operation_id: u64, record: &NativeEventRecord) -> ModelOpenEvent {
    let event = record.to_event();
    let kind = match event.kind {
        RuntimeEventKind::ModelOpenStarted => ModelOpenEventKind::Started,
        RuntimeEventKind::ModelOpenProgress => ModelOpenEventKind::Progress(ModelOpenProgress {
            current: event.progress_current,
            total: (event.progress_total > 0).then_some(event.progress_total),
            unit: progress_unit(event.progress_unit),
        }),
        RuntimeEventKind::BackendDeviceSelected => ModelOpenEventKind::BackendDeviceSelected,
        RuntimeEventKind::ModelLoadPhaseChanged => ModelOpenEventKind::PhaseChanged,
        RuntimeEventKind::ModelLoadMemoryAllocated => ModelOpenEventKind::MemoryAllocated,
        RuntimeEventKind::ModelLoadTensorsOffloaded => ModelOpenEventKind::TensorsOffloaded,
        RuntimeEventKind::ModelLoadTokenizerReady => ModelOpenEventKind::TokenizerReady,
        RuntimeEventKind::ModelLoadAuxComponentReady => ModelOpenEventKind::AuxComponentReady,
        RuntimeEventKind::ModelOpenFinished => ModelOpenEventKind::Finished,
        RuntimeEventKind::ModelOpenFailedHandled => {
            ModelOpenEventKind::FailedHandled(failure(event.failure_code))
        }
        _ => ModelOpenEventKind::Other,
    };
    ModelOpenEvent {
        operation_id,
        sequence: event.sequence,
        timestamp_mono_ns: event.timestamp_mono_ns,
        native_model_id: event.model_id,
        emitter: match event.emitter {
            RuntimeEventEmitterKind::OpenThread => ModelOpenEmitter::OpenThread,
            RuntimeEventEmitterKind::WorkerThread => ModelOpenEmitter::WorkerThread,
            RuntimeEventEmitterKind::Unknown | RuntimeEventEmitterKind::Other(_) => {
                ModelOpenEmitter::Unknown
            }
        },
        kind,
        detail: event.detail_bytes.clone(),
        detail_truncated: record.detail_truncated,
        source: Box::new(event),
    }
}

fn progress_unit(unit: RuntimeEventProgressUnit) -> ModelOpenProgressUnit {
    match unit {
        RuntimeEventProgressUnit::Bytes => ModelOpenProgressUnit::Bytes,
        RuntimeEventProgressUnit::Items => ModelOpenProgressUnit::Items,
        RuntimeEventProgressUnit::Tensors => ModelOpenProgressUnit::Tensors,
        RuntimeEventProgressUnit::Steps => ModelOpenProgressUnit::Steps,
        RuntimeEventProgressUnit::None => ModelOpenProgressUnit::Unspecified,
        RuntimeEventProgressUnit::Unknown(_) => ModelOpenProgressUnit::Unknown,
    }
}

fn failure(code: RuntimeEventFailureCode) -> ModelOpenFailure {
    match code {
        RuntimeEventFailureCode::InvalidArgument => ModelOpenFailure::InvalidArgument,
        RuntimeEventFailureCode::IoError => ModelOpenFailure::IoError,
        RuntimeEventFailureCode::ModelError => ModelOpenFailure::ModelError,
        RuntimeEventFailureCode::RuntimeError => ModelOpenFailure::RuntimeError,
        RuntimeEventFailureCode::BackendError => ModelOpenFailure::BackendError,
        RuntimeEventFailureCode::Cancelled => ModelOpenFailure::Cancelled,
        RuntimeEventFailureCode::InternalError => ModelOpenFailure::InternalError,
        RuntimeEventFailureCode::None => ModelOpenFailure::Unspecified,
        RuntimeEventFailureCode::Unknown(_) => ModelOpenFailure::Unknown,
    }
}

#[cfg(test)]
mod tests;
