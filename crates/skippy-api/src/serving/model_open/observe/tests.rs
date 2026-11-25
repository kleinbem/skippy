use super::*;
use crate::serving::ModelOpenObserver;
use crate::serving::{ModelOpenContradiction, ModelOpenEvents};
use skippy_ffi::{
    RUNTIME_EVENT_CATEGORY_MODEL_OPEN, RUNTIME_EVENT_EMITTER_WORKER_THREAD,
    RUNTIME_EVENT_FAILURE_MODEL_ERROR, RUNTIME_EVENT_KIND_MODEL_OPEN_FAILED_HANDLED,
    RUNTIME_EVENT_KIND_MODEL_OPEN_FINISHED, RUNTIME_EVENT_KIND_MODEL_OPEN_PROGRESS,
    RUNTIME_EVENT_PROGRESS_UNIT_TENSORS, SkippyRuntimeEventV1, Status,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn emit(queue: &ModelOpenEventQueue, kind: u32, sequence: u64, detail: &[u8]) {
    let raw = SkippyRuntimeEventV1 {
        abi_version: 1,
        struct_size: std::mem::size_of::<SkippyRuntimeEventV1>() as u32,
        category: RUNTIME_EVENT_CATEGORY_MODEL_OPEN,
        kind,
        emitter: RUNTIME_EVENT_EMITTER_WORKER_THREAD,
        reserved0: 0,
        sequence,
        timestamp_mono_ns: 0,
        model_id: 17,
        stage_id: 0,
        session_id: 0,
        progress_current: 3,
        progress_total: 4,
        progress_unit: RUNTIME_EVENT_PROGRESS_UNIT_TENSORS,
        failure_code: RUNTIME_EVENT_FAILURE_MODEL_ERROR,
        status: Status::Ok as i32,
        reserved1: 0,
        detail_ptr: detail.as_ptr().cast(),
        detail_len: detail.len() as u64,
        numeric_summary_0: 0,
        numeric_summary_1: 0,
        numeric_summary_2: 0,
        numeric_summary_3: 0,
    };
    // SAFETY: the queue copies the event and detail during this call.
    unsafe { queue.deliver_for_test(&raw) };
}

fn observe(
    load: impl FnOnce(&ModelOpenEventQueue) -> Result<u32, &'static str>,
) -> (
    Result<u32, &'static str>,
    Vec<ModelOpenObservation>,
    ModelOpenSummary,
) {
    let (tx, rx) = mpsc::channel();
    let ModelOpenEvents::Enabled(observer) = ModelOpenEvents::observe(move |item| {
        tx.send(item).unwrap();
    }) else {
        unreachable!();
    };
    let result = run_observed(observer, |queue| load(&queue.unwrap()));
    let mut observations: Vec<_> = rx.into_iter().collect();
    let Some(ModelOpenObservation::Summary(summary)) = observations.pop() else {
        panic!("final observation must be a summary");
    };
    (result, observations, summary)
}

#[test]
fn forwards_live_progress_from_a_separate_thread_and_finishes_in_queue_order() {
    let (tx, rx) = mpsc::channel();
    let loader = thread::current().id();
    let observer: ModelOpenObserver = Box::new(move |item| {
        tx.send((thread::current().id(), item)).unwrap();
    });
    let result = run_observed(Some(observer), |queue| {
        let queue = queue.unwrap();
        emit(&queue, RUNTIME_EVENT_KIND_MODEL_OPEN_PROGRESS, 9, b"");
        let (drainer, observation) = rx.recv_timeout(TEST_TIMEOUT).unwrap();
        assert_ne!(drainer, loader);
        let ModelOpenObservation::Event(event) = observation else {
            panic!("expected progress during the load");
        };
        assert_eq!(event.operation_id, queue.operation_id().0);
        assert_eq!(event.sequence, 9);
        assert_eq!(event.native_model_id, 17);
        assert_eq!(event.emitter, ModelOpenEmitter::WorkerThread);
        assert_eq!(
            event.kind,
            ModelOpenEventKind::Progress(ModelOpenProgress {
                current: 3,
                total: Some(4),
                unit: ModelOpenProgressUnit::Tensors,
            })
        );
        emit(&queue, RUNTIME_EVENT_KIND_MODEL_OPEN_FINISHED, 7, b"");
        Ok::<_, ()>((drainer, event.operation_id))
    });
    let (drainer, operation_id) = result.unwrap();
    let observations = rx.into_iter().collect::<Vec<_>>();
    let [
        (event_thread, ModelOpenObservation::Event(event)),
        (summary_thread, ModelOpenObservation::Summary(summary)),
    ] = observations.as_slice()
    else {
        panic!("expected finish followed by summary");
    };
    assert_eq!((*event_thread, *summary_thread), (drainer, drainer));
    assert_eq!(event.kind, ModelOpenEventKind::Finished);
    assert_eq!(summary.operation_id, operation_id);
    assert_eq!(summary.outcome, ModelLoadOutcome::Succeeded);
    assert_eq!(summary.records_observed, 2);
    assert_eq!(summary.last_sequence, Some(7));
    assert_eq!(summary.lost(), 0);
    assert_eq!(summary.contradiction(), None);
}

#[test]
fn handled_failure_delivers_detail_and_releases_the_queue() {
    let mut weak = None;
    let (tx, rx) = mpsc::channel();
    let observer: ModelOpenObserver = Box::new(move |item| tx.send(item).unwrap());
    let detail = vec![0xff; skippy_runtime::INLINE_DETAIL_BYTES + 1];
    let result = run_observed(Some(observer), |queue| {
        let queue = queue.unwrap();
        weak = Some(Arc::downgrade(&queue));
        emit(
            &queue,
            RUNTIME_EVENT_KIND_MODEL_OPEN_FAILED_HANDLED,
            2,
            &detail,
        );
        Err::<(), _>("original error")
    });
    assert_eq!(result, Err("original error"));
    assert!(weak.unwrap().upgrade().is_none());
    let observations = rx.into_iter().collect::<Vec<_>>();
    let [
        ModelOpenObservation::Event(event),
        ModelOpenObservation::Summary(summary),
    ] = observations.as_slice()
    else {
        panic!("expected a diagnostic followed by summary");
    };
    assert_eq!(
        event.kind,
        ModelOpenEventKind::FailedHandled(ModelOpenFailure::ModelError)
    );
    assert!(event.detail_truncated);
    assert_eq!(event.detail, detail[..skippy_runtime::INLINE_DETAIL_BYTES]);
    assert_eq!(summary.outcome, ModelLoadOutcome::Failed);
    assert!(summary.saw_failed_handled);
    assert_eq!(summary.contradiction(), None);
}

#[test]
fn event_carries_the_complete_native_record() {
    let (_, records, _) = observe(|queue| {
        let raw = SkippyRuntimeEventV1 {
            abi_version: 1,
            struct_size: std::mem::size_of::<SkippyRuntimeEventV1>() as u32,
            category: RUNTIME_EVENT_CATEGORY_MODEL_OPEN,
            kind: RUNTIME_EVENT_KIND_MODEL_OPEN_FAILED_HANDLED,
            emitter: RUNTIME_EVENT_EMITTER_WORKER_THREAD,
            reserved0: 0,
            sequence: 6,
            timestamp_mono_ns: 77,
            model_id: 17,
            stage_id: 23,
            session_id: 29,
            progress_current: 3,
            progress_total: 4,
            progress_unit: RUNTIME_EVENT_PROGRESS_UNIT_TENSORS,
            failure_code: RUNTIME_EVENT_FAILURE_MODEL_ERROR,
            status: Status::ModelError as i32,
            reserved1: 0,
            detail_ptr: b"bad header".as_ptr().cast(),
            detail_len: 10,
            numeric_summary_0: 101,
            numeric_summary_1: 102,
            numeric_summary_2: 0,
            numeric_summary_3: 104,
        };
        // SAFETY: the queue copies the event and detail during this call.
        unsafe { queue.deliver_for_test(&raw) };
        Err("load error")
    });
    let [ModelOpenObservation::Event(event)] = records.as_slice() else {
        panic!("expected one event");
    };
    let source = &event.source;
    assert_eq!(source.status, Status::ModelError);
    assert_eq!((source.stage_id, source.session_id), (23, 29));
    assert_eq!(
        [
            source.numeric_summary_0,
            source.numeric_summary_1,
            source.numeric_summary_2,
            source.numeric_summary_3,
        ],
        [Some(101), Some(102), Some(0), Some(104)]
    );
    assert_eq!(source.category.raw(), RUNTIME_EVENT_CATEGORY_MODEL_OPEN);
    assert_eq!(
        source.kind.raw(),
        RUNTIME_EVENT_KIND_MODEL_OPEN_FAILED_HANDLED
    );
    assert_eq!(source.emitter.raw(), RUNTIME_EVENT_EMITTER_WORKER_THREAD);
    assert_eq!(source.failure_code.raw(), RUNTIME_EVENT_FAILURE_MODEL_ERROR);
    assert_eq!(
        source.progress_unit.raw(),
        RUNTIME_EVENT_PROGRESS_UNIT_TENSORS
    );
    assert_eq!(
        (source.sequence, source.timestamp_mono_ns, source.model_id),
        (
            event.sequence,
            event.timestamp_mono_ns,
            event.native_model_id
        )
    );
    assert_eq!(source.detail_bytes, event.detail);
    assert_eq!(event.detail, b"bad header");
}

#[test]
fn contradictory_terminals_do_not_change_either_load_result() {
    for (returned, expected) in [
        (
            Ok(42),
            ModelOpenContradiction::FailedHandledButLoadSucceeded,
        ),
        (
            Err("load error"),
            ModelOpenContradiction::FinishedButLoadFailed,
        ),
    ] {
        let (result, records, summary) = observe(|queue| {
            emit(queue, RUNTIME_EVENT_KIND_MODEL_OPEN_FAILED_HANDLED, 1, b"");
            emit(queue, RUNTIME_EVENT_KIND_MODEL_OPEN_FINISHED, 2, b"");
            returned
        });
        assert_eq!(result, returned);
        assert_eq!(records.len(), 2);
        assert!(summary.saw_finished && summary.saw_failed_handled);
        assert_eq!(summary.contradiction(), Some(expected));
    }
}

#[test]
fn missing_and_unknown_records_do_not_invent_terminal_evidence() {
    let (result, records, summary) = observe(|_| Ok(42));
    assert_eq!(result, Ok(42));
    assert!(records.is_empty());
    assert_eq!(summary.last_sequence, None);
    assert_eq!(summary.contradiction(), None);

    let (_, records, summary) = observe(|queue| {
        emit(queue, u32::MAX, 5, b"future record");
        Err("load error")
    });
    let [ModelOpenObservation::Event(event)] = records.as_slice() else {
        panic!("expected the unknown observation");
    };
    assert_eq!(event.kind, ModelOpenEventKind::Other);
    assert_eq!(event.detail, b"future record");
    assert_eq!(summary.contradiction(), None);
    assert_eq!(summary.lost(), 0);
}

#[test]
fn stalled_observer_preserves_native_progress_and_reports_queue_loss() {
    let (start_tx, start_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (summary_tx, summary_rx) = mpsc::channel();
    let observer: ModelOpenObserver = Box::new(move |item| match item {
        ModelOpenObservation::Event(event) if event.sequence == 0 => {
            start_tx.send(()).unwrap();
            resume_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        }
        ModelOpenObservation::Summary(summary) => summary_tx.send(summary).unwrap(),
        _ => {}
    });
    let result = run_observed(Some(observer), |queue| {
        let queue = queue.unwrap();
        emit(&queue, RUNTIME_EVENT_KIND_MODEL_OPEN_PROGRESS, 0, b"");
        start_rx.recv_timeout(TEST_TIMEOUT).unwrap();
        for sequence in 1..=MODEL_OPEN_RECORD_CAPACITY as u64 + 2 {
            emit(
                &queue,
                RUNTIME_EVENT_KIND_MODEL_OPEN_PROGRESS,
                sequence,
                b"",
            );
        }
        // SAFETY: the ingress contract permits a null pointer, which it rejects.
        unsafe { queue.deliver_for_test(std::ptr::null()) };
        resume_tx.send(()).unwrap();
        Ok::<_, ()>(())
    });
    assert_eq!(result, Ok(()));
    let summary = summary_rx.try_recv().unwrap();
    assert_eq!(
        summary.records_observed,
        MODEL_OPEN_RECORD_CAPACITY as u64 + 1
    );
    assert_eq!(
        summary.last_sequence,
        Some(MODEL_OPEN_RECORD_CAPACITY as u64)
    );
    assert_eq!(
        (summary.dropped, summary.rejected, summary.lost()),
        (2, 1, 3)
    );
    assert_eq!(summary.contradiction(), None);
}

#[test]
fn enabled_without_observer_runs_the_load_without_a_queue() {
    let result = run_observed(None::<ModelOpenObserver>, |queue| {
        assert!(queue.is_none());
        Ok::<_, ()>(42)
    });
    assert_eq!(result, Ok(42));
}

#[test]
fn observer_panics_do_not_change_the_load_result() {
    let observer: ModelOpenObserver = Box::new(|_| panic!("observer panicked"));
    let result = run_observed(Some(observer), |_| Ok::<_, ()>(42));
    assert_eq!(result, Ok(42));
}

#[test]
fn load_unwinds_after_the_observer_is_dropped() {
    let (tx, rx) = mpsc::channel();
    let observer: ModelOpenObserver = Box::new(move |item| tx.send(item).unwrap());
    let result = catch_unwind(AssertUnwindSafe(|| {
        run_observed::<(), ()>(Some(observer), |_| panic!("load panicked"))
    }));
    assert!(result.is_err());
    assert!(matches!(
        rx.try_recv(),
        Err(mpsc::TryRecvError::Disconnected)
    ));
}
