use super::*;
use std::{cell::Cell, time::Duration};

struct FakeRunner<'a> {
    results: &'a [u8],
    cancel: bool,
    calls: Cell<usize>,
}

impl MeasurementRunner for FakeRunner<'_> {
    fn run(
        &self,
        _: &MeasurementRequest,
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>, MeasurementError> {
        self.calls.set(self.calls.get() + 1);
        if self.cancel {
            cancellation.cancel();
        }
        Ok(self.results.to_vec())
    }
}

fn request() -> MeasurementRequest {
    MeasurementRequest {
        backend: BenchmarkBackend::Cuda,
        executable: "unused".into(),
        environment: Default::default(),
        targets: (0..3)
            .map(|ordinal| DeviceTarget {
                ordinal,
                expected_name: Some(if ordinal == 0 { "same" } else { "different" }.into()),
                stable_id: format!("gpu-{ordinal}"),
            })
            .collect(),
        timeout: Duration::from_secs(1),
    }
}

#[test]
fn attributes_identical_names_by_ordinal_and_preserves_measured_zero() {
    let payload = serde_json::to_vec(&[
        BenchmarkOutput {
            device: "same".into(),
            compute_tflops_fp32: Some(0.0),
            ..Default::default()
        },
        BenchmarkOutput {
            device: "same".into(),
            ..Default::default()
        },
    ])
    .unwrap();
    let runner = FakeRunner {
        results: &payload,
        cancel: false,
        calls: Cell::new(0),
    };
    let result = measure(&runner, &request(), &Cancellation::default()).unwrap();
    assert_eq!(result[0].stable_id.as_deref(), Some("gpu-0"));
    assert_eq!(
        result[0].result.as_ref().unwrap().compute_tflops_fp32,
        Some(0.0)
    );
    assert_eq!(result[0].result.as_ref().unwrap().compute_tflops_fp16, None);
    assert!(matches!(
        result[1].result,
        Err(DeviceError::NameMismatch { .. })
    ));
    assert!(matches!(result[2].result, Err(DeviceError::Missing)));
}

#[test]
fn cancellation_and_helper_errors_are_distinct() {
    let runner = FakeRunner {
        results: br#"{"error":"no devices"}"#,
        cancel: true,
        calls: Cell::new(0),
    };
    let cancellation = Cancellation::default();
    assert!(matches!(
        measure(&runner, &request(), &cancellation),
        Err(MeasurementError::Cancelled)
    ));
    assert!(matches!(
        measure(&runner, &request(), &cancellation),
        Err(MeasurementError::Cancelled)
    ));
    assert_eq!(runner.calls.get(), 1);
    let runner = FakeRunner {
        cancel: false,
        ..runner
    };
    assert!(
        matches!(measure(&runner, &request(), &Cancellation::default()), Err(MeasurementError::Helper(message)) if message == "no devices")
    );
}

#[test]
fn fingerprint_is_invalidated_by_runtime_or_hardware_change() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gpu.json");
    let mut key = FingerprintKey {
        backend: BenchmarkBackend::Cuda,
        runtime_identity: "runtime-1".into(),
        hardware_identity: "hardware-1".into(),
        devices: request().targets,
    };
    assert!(load_fingerprint(&path, &key).unwrap().is_none());
    store_fingerprint(&path, key.clone(), vec![]).unwrap();
    assert!(load_fingerprint(&path, &key).unwrap().is_some());
    key.runtime_identity = "runtime-2".into();
    assert!(load_fingerprint(&path, &key).unwrap().is_none());
    key.runtime_identity = "runtime-1".into();
    key.hardware_identity = "hardware-2".into();
    assert!(load_fingerprint(&path, &key).unwrap().is_none());
}
