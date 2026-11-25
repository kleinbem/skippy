//! Supervised measurement of the devices enumerated by a native helper.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    time::Duration,
};

use process_supervision::{
    Completion, Limits, Outcome, ProcessSpec, RawCaptureOptions, Readiness, Value, supervise_raw,
};
use serde::{Deserialize, Serialize};

use crate::{BenchmarkBackend, BenchmarkOutput};

pub use process_supervision::Cancellation;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceTarget {
    /// Ordinal in the helper's backend enumeration, after visibility filtering.
    pub ordinal: usize,
    /// Expected helper name, when known, checked as an additional identity guard.
    pub expected_name: Option<String>,
    /// The caller's stable identity for this ordinal.
    pub stable_id: String,
}

pub struct MeasurementRequest {
    pub backend: BenchmarkBackend,
    pub executable: PathBuf,
    /// Explicit environment for driver visibility and library search.
    pub environment: BTreeMap<OsString, OsString>,
    /// Empty enumerates all helper results without assigning stable identities.
    pub targets: Vec<DeviceTarget>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceMeasurement {
    pub ordinal: usize,
    pub stable_id: Option<String>,
    pub result: Result<BenchmarkOutput, DeviceError>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum DeviceError {
    #[error("helper did not return this ordinal")]
    Missing,
    #[error("expected device {expected}, helper reported {actual}")]
    NameMismatch { expected: String, actual: String },
}

#[derive(Debug, thiserror::Error)]
pub enum MeasurementError {
    #[error("no standard native benchmark helper for {0:?}")]
    Unsupported(BenchmarkBackend),
    #[error("device targets require distinct ordinals and nonempty, distinct stable IDs")]
    InvalidTargets,
    #[error("GPU measurement cancelled")]
    Cancelled,
    #[error("GPU measurement exceeded its deadline")]
    Timeout,
    #[error("GPU benchmark helper failed: {0}")]
    Helper(String),
    #[error("GPU benchmark helper returned invalid output: {0}")]
    Output(String),
}

pub trait MeasurementRunner {
    fn run(
        &self,
        request: &MeasurementRequest,
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>, MeasurementError>;
}

pub struct NativeMeasurementRunner;

impl MeasurementRunner for NativeMeasurementRunner {
    fn run(
        &self,
        request: &MeasurementRequest,
        cancellation: &Cancellation,
    ) -> Result<Vec<u8>, MeasurementError> {
        let executable = request.executable.canonicalize().map_err(helper_error)?;
        let cwd = executable.parent().unwrap_or(Path::new("/")).to_path_buf();
        let report = supervise_raw(
            &ProcessSpec {
                executable,
                cwd,
                arguments: vec![Value::Public("--json".into())],
                environment: request
                    .environment
                    .iter()
                    .map(|(key, value)| (key.clone(), Value::Public(value.clone())))
                    .collect(),
            },
            &Limits {
                execution: request.timeout,
                graceful_shutdown: Duration::from_millis(250),
                forced_shutdown: Duration::from_secs(2),
                retained_bytes_per_stream: 4096,
                readiness: Readiness::None,
                completion: Completion::Exit,
            },
            cancellation,
            RawCaptureOptions {
                stdout: NonZeroUsize::new(1024 * 1024),
                stderr: None,
            },
        )
        .map_err(helper_error)?;
        let process = report.process;
        if !process.cleanup.complete {
            return Err(MeasurementError::Helper(format!(
                "incomplete cleanup: {:?}",
                process.cleanup
            )));
        }
        match process.outcome {
            Outcome::Deadline => return Err(MeasurementError::Timeout),
            Outcome::Cancelled => return Err(MeasurementError::Cancelled),
            _ => {}
        }
        let stdout = report
            .stdout
            .as_ref()
            .map_or(&[][..], |bytes| bytes.as_bytes());
        if let Some(error) = reported_error(stdout) {
            return Err(MeasurementError::Helper(error));
        }
        if !process.success() {
            return Err(MeasurementError::Helper(format!(
                "{:?}, status {:?}: {}",
                process.outcome,
                process.status,
                String::from_utf8_lossy(&process.stderr.bytes_retained).trim()
            )));
        }
        Ok(stdout.to_vec())
    }
}

fn helper_error(error: impl std::fmt::Display) -> MeasurementError {
    MeasurementError::Helper(error.to_string())
}

fn reported_error(output: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(output)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

/// Run the same native workflow for standalone callers and embedders.
/// Output order is defined by the helper's backend device ordinal.
pub fn measure(
    runner: &impl MeasurementRunner,
    request: &MeasurementRequest,
    cancellation: &Cancellation,
) -> Result<Vec<DeviceMeasurement>, MeasurementError> {
    let mut ordinals = BTreeSet::new();
    let mut identities = BTreeSet::new();
    if request.targets.iter().any(|target| {
        target.stable_id.trim().is_empty()
            || !ordinals.insert(target.ordinal)
            || !identities.insert(target.stable_id.as_str())
    }) {
        return Err(MeasurementError::InvalidTargets);
    }
    if !request.backend.has_native_helper() {
        return Err(MeasurementError::Unsupported(request.backend));
    }
    if cancellation.is_cancelled() {
        return Err(MeasurementError::Cancelled);
    }
    let output = runner.run(request, cancellation)?;
    if cancellation.is_cancelled() {
        return Err(MeasurementError::Cancelled);
    }
    if let Some(error) = reported_error(&output) {
        return Err(MeasurementError::Helper(error));
    }
    let results: Vec<BenchmarkOutput> = serde_json::from_slice(&output)
        .map_err(|error| MeasurementError::Output(error.to_string()))?;
    if results.is_empty() && request.targets.is_empty() {
        return Err(MeasurementError::Output(
            "no GPU measurements returned".into(),
        ));
    }
    if request.targets.is_empty() {
        return Ok(results
            .into_iter()
            .enumerate()
            .map(|(ordinal, measurement)| DeviceMeasurement {
                ordinal,
                stable_id: None,
                result: Ok(measurement),
            })
            .collect());
    }
    Ok(request
        .targets
        .iter()
        .map(|target| DeviceMeasurement {
            ordinal: target.ordinal,
            stable_id: Some(target.stable_id.clone()),
            result: match results.get(target.ordinal) {
                None => Err(DeviceError::Missing),
                Some(result)
                    if target
                        .expected_name
                        .as_ref()
                        .is_some_and(|name| *name != result.device) =>
                {
                    Err(DeviceError::NameMismatch {
                        expected: target.expected_name.clone().unwrap_or_default(),
                        actual: result.device.clone(),
                    })
                }
                Some(result) => Ok(result.clone()),
            },
        })
        .collect())
}
