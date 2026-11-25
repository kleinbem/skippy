//! Example host using only Skippy's serving API for model-open observations.
use std::sync::mpsc::{self, Receiver};

use skippy_api::serving::{
    InferenceOptions, ModelLoadOutcome, ModelLoadRequest, ModelOpenEventKind, ModelOpenEvents,
    ModelOpenObservation, ModelOpenProgress, ModelOpenSummary,
};
use skippy_protocol::StageConfig;
use skippy_serving::{EmbeddedRuntimeOptions, binary_transport::WireCondition};

#[derive(Default)]
struct HostDisplay {
    progress: Vec<ModelOpenProgress>,
    handled_failures: Vec<Vec<u8>>,
    summary: Option<ModelOpenSummary>,
}

impl HostDisplay {
    fn collect(rx: Receiver<ModelOpenObservation>) -> Self {
        let mut display = Self::default();
        for observation in rx {
            assert!(display.summary.is_none(), "summary must be last");
            match observation {
                ModelOpenObservation::Event(event) => match event.kind {
                    ModelOpenEventKind::Progress(progress) => display.progress.push(progress),
                    ModelOpenEventKind::FailedHandled(_) => {
                        display.handled_failures.push(event.detail);
                    }
                    _ => {}
                },
                ModelOpenObservation::Summary(summary) => display.summary = Some(summary),
                _ => {}
            }
        }
        display
    }
}

fn observed_request(config: StageConfig) -> (ModelLoadRequest, Receiver<ModelOpenObservation>) {
    let (tx, rx) = mpsc::channel();
    let request = ModelLoadRequest {
        openai: InferenceOptions::direct_single_stage_defaults(
            config.model_id.clone(),
            32,
            1,
            false,
        ),
        runtime: EmbeddedRuntimeOptions {
            config,
            topology: None,
            n_threads: None,
            n_threads_batch: None,
            mtp_source: Default::default(),
            recurrent_rollback_tokens: 0,
            metrics_otlp_grpc: None,
            telemetry_queue_capacity: 0,
            telemetry_level: skippy_serving::telemetry::TelemetryLevel::Off,
            session_lifecycle_observer: None,
        },
        open_events: ModelOpenEvents::observe(move |observation| {
            let _ = tx.send(observation);
        }),
        hooks_factory: None,
        generation_observer: None,
        kv_observer: None,
        hook_policy: None,
        guardrails: None,
        guardrail_telemetry: None,
        downstream_wire_condition: WireCondition::new(0.0, None).unwrap(),
        serving_telemetry: None,
        l3_manager: None,
    };
    (request, rx)
}

#[test]
fn embedding_host_gets_a_final_summary_for_validation_failure() {
    let (request, observations) = observed_request(StageConfig {
        stage_index: 1,
        ..StageConfig::default()
    });
    let error = request.load().err().expect("worker stage must be rejected");
    assert!(error.to_string().contains("requires stage 0"));
    let display = HostDisplay::collect(observations);
    assert!(display.progress.is_empty());
    assert!(display.handled_failures.is_empty());
    let summary = display.summary.unwrap();
    assert_eq!(summary.outcome, ModelLoadOutcome::Failed);
    assert_eq!(summary.records_observed, 0);
    assert_eq!(summary.contradiction(), None);
}

#[cfg(not(feature = "dynamic-native-runtime"))]
#[test]
fn embedding_host_gets_typed_diagnostics_from_a_failed_native_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.gguf");
    std::fs::write(&path, b"invalid GGUF fixture").unwrap();
    let mut config: StageConfig = serde_json::from_value(skippy_config::example_config()).unwrap();
    config.model_path = Some(path.to_str().unwrap().into());
    let (request, observations) = observed_request(config);

    let error = request.load().err().expect("invalid GGUF must fail");
    assert!(error.to_string().contains("load Skippy runtime"));
    let display = HostDisplay::collect(observations);
    let summary = display.summary.unwrap();
    assert_eq!(summary.outcome, ModelLoadOutcome::Failed);
    assert_eq!(summary.contradiction(), None);
    assert!(summary.saw_failed_handled, "{summary:?}");
    assert_eq!(display.handled_failures.len(), 1);
    assert!(!display.handled_failures[0].is_empty());
}

#[cfg(not(feature = "dynamic-native-runtime"))]
#[test]
#[ignore = "requires SKIPPY_TEST_GGUF_PATH pointing to a local text generation model"]
fn embedding_host_gets_typed_progress_from_a_real_model() {
    let path = std::path::PathBuf::from(
        std::env::var_os("SKIPPY_TEST_GGUF_PATH").expect("local GGUF fixture path"),
    );
    let model_id = "observation-fixture";
    let identity = skippy_api::source::synthetic_direct_gguf_package(model_id, &path).unwrap();
    let mut options = skippy_api::SingleStageOptions::new(model_id, &path);
    options.n_gpu_layers = 0;
    options.ctx_size = 512;
    options.generation_concurrency = 1;
    options.native_mtp_enabled = false;
    let config =
        skippy_api::single_stage_config(&options, identity.into(), "observer-test".into()).unwrap();
    let (request, observations) = observed_request(config);

    let _loaded = request.load().unwrap();
    let display = HostDisplay::collect(observations);
    assert!(display.progress.iter().any(|progress| progress.current > 0));
    assert!(display.handled_failures.is_empty());
    let summary = display.summary.unwrap();
    assert_eq!(summary.outcome, ModelLoadOutcome::Succeeded);
    assert!(summary.saw_finished);
    assert_eq!(summary.contradiction(), None);
}
