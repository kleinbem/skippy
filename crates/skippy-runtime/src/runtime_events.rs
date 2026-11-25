use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use skippy_ffi::{
    Error as RawError, Model as RawModel, SkippyRuntimeEventReporterV1 as RawRuntimeEventReporter,
    Status,
};
pub(crate) use skippy_ffi::{
    skippy_model_open_from_parts_with_events_fn as model_open_from_parts_with_events_symbol,
    skippy_model_open_with_events_fn as model_open_with_events_symbol,
    try_abi_features as abi_features_bitmask,
};

mod model_open_queue;
mod native_record;
mod wire_types;
use model_open_queue::ModelOpenEventReporterRegistration;
pub use model_open_queue::{MODEL_OPEN_RECORD_CAPACITY, ModelOpenEventQueue};
pub use native_record::{INLINE_DETAIL_BYTES, NativeEventRecord, RecordRejection};
pub use wire_types::{
    RuntimeEvent, RuntimeEventCategory, RuntimeEventEmitterKind, RuntimeEventFailureCode,
    RuntimeEventKind, RuntimeEventProgressUnit,
};

pub(crate) const RUNTIME_EVENT_V1_ABI_VERSION: u32 = 1;

/// Correlates every runtime event emitted during one native model-open call.
/// Callers supply it when creating the [`ModelOpenEventQueue`] they pass to
/// `open_with_events`/`open_from_parts_with_events`, so a host-assigned
/// identity can be threaded in without another change here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

/// Default generator for callers with no host-assigned identity of their
/// own yet. Exposed so a caller's own id source can be swapped in later
/// without touching this crate again.
pub fn next_operation_id() -> OperationId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    OperationId(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn collect_model_open_events<OpenFn>(
    queue: &Arc<ModelOpenEventQueue>,
    open_fn: OpenFn,
) -> (*mut RawModel, Status, *mut RawError)
where
    OpenFn:
        FnOnce(*const RawRuntimeEventReporter, *mut *mut RawModel, *mut *mut RawError) -> Status,
{
    let registration = ModelOpenEventReporterRegistration::new(queue);
    let mut raw = ptr::null_mut();
    let mut error = ptr::null_mut();
    let status = open_fn(registration.reporter_ptr(), &mut raw, &mut error);
    (raw, status, error)
}

/// Takes the `_with_events` path only when a queue is supplied AND the
/// runtime supports events; otherwise the legacy open runs and the queue
/// stays empty. The native status/error return is authoritative either way.
pub(crate) fn run_model_open<OpenFn, OpenWithEventsFn>(
    open_fn: OpenFn,
    open_with_events_fn: OpenWithEventsFn,
    queue: Option<&Arc<ModelOpenEventQueue>>,
    use_event_reporter: bool,
) -> (*mut RawModel, Status, *mut RawError)
where
    OpenFn: FnOnce(*mut *mut RawModel, *mut *mut RawError) -> Status,
    OpenWithEventsFn:
        FnOnce(*const RawRuntimeEventReporter, *mut *mut RawModel, *mut *mut RawError) -> Status,
{
    match (queue, use_event_reporter) {
        (Some(queue), true) => collect_model_open_events(queue, open_with_events_fn),
        _ => {
            let mut raw = ptr::null_mut();
            let mut error = ptr::null_mut();
            let status = open_fn(&mut raw, &mut error);
            (raw, status, error)
        }
    }
}

// Gates purely on runtime-observable capability (native library loaded,
// feature bit advertised, `_with_events` symbols resolved) rather than a
// hardcoded ABI patch window. Exact-compatible loader probing is added by a
// later task; this function is the seam it extends.
pub(crate) fn model_open_events_supported() -> bool {
    skippy_ffi::native_runtime_loaded()
        && abi_features_bitmask()
            .is_some_and(|features| (features & skippy_ffi::FEATURE_RUNTIME_EVENTS) != 0)
        && model_open_with_events_symbol().is_some()
        && model_open_from_parts_with_events_symbol().is_some()
}

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod tests_hardening;
