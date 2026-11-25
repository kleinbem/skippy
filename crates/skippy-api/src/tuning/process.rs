use std::{collections::BTreeMap, ffi::OsString, num::NonZeroUsize, path::PathBuf};

use process_supervision::{
    Completion, Limits, Outcome, ProcessSpec, RawCaptureOptions, Readiness, Value, supervise_raw,
};

use super::*;

pub const RESULT_PREFIX: &str = "SKIPPY_TUNING_RESULT ";

/// Launch one Skippy process per trial with its own scratch directory and a
/// bounded child tree. The executable and its explicit environment are
/// provided by the embedding host.
pub struct ProcessTrialRunner {
    pub executable: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
}

impl TrialRunner for ProcessTrialRunner {
    fn run(
        &self,
        trial: Trial<'_>,
        cancellation: &Cancellation,
    ) -> Result<Measurement, TrialError> {
        let workdir = tempfile::tempdir().map_err(failed)?;
        let cwd = workdir.path().canonicalize().map_err(failed)?;
        let path = cwd.join("trial.json");
        let input = TrialInput {
            target: trial.target.clone(),
            runtime: trial.runtime.clone(),
            hardware: trial.hardware.clone(),
            workload: trial.workload.clone(),
            settings: trial.settings.clone(),
        };
        serde_json::to_writer(std::fs::File::create(&path).map_err(failed)?, &input)
            .map_err(failed)?;
        let mut environment: BTreeMap<_, _> = self
            .environment
            .iter()
            .map(|(key, val)| (key.clone(), Value::Public(val.clone())))
            .collect();
        environment.insert(
            "SKIPPY_DATA_DIR".into(),
            Value::Public(cwd.join("data").into_os_string()),
        );
        let process = ProcessSpec {
            executable: self.executable.canonicalize().map_err(failed)?,
            arguments: vec![
                Value::Public("tune-trial".into()),
                Value::Public(path.into_os_string()),
            ],
            cwd,
            environment,
        };
        let report = supervise_raw(
            &process,
            &Limits {
                execution: trial.timeout,
                graceful_shutdown: Duration::from_millis(250),
                forced_shutdown: Duration::from_secs(3),
                retained_bytes_per_stream: 8192,
                readiness: Readiness::None,
                completion: Completion::Exit,
            },
            cancellation,
            RawCaptureOptions {
                stdout: NonZeroUsize::new(1024 * 1024),
                stderr: None,
            },
        )
        .map_err(failed)?;
        let status = report.process;
        if !status.cleanup.complete {
            return Err(TrialError::Failed(format!(
                "trial cleanup incomplete: {:?}",
                status.cleanup
            )));
        }
        match status.outcome {
            Outcome::Deadline => return Err(TrialError::Timeout),
            Outcome::Cancelled => return Err(TrialError::Cancelled),
            _ => {}
        }
        if !status.success() {
            let diagnostic = if status.stderr.bytes_retained.is_empty() {
                &status.stdout.bytes_retained
            } else {
                &status.stderr.bytes_retained
            };
            return Err(TrialError::Failed(format!(
                "{:?} {:?}: {}",
                status.outcome,
                status.status,
                String::from_utf8_lossy(diagnostic).trim()
            )));
        }
        let result = report
            .stdout
            .as_ref()
            .map(|out| out.as_bytes())
            .unwrap_or_default()
            .split(|byte| *byte == b'\n')
            .filter_map(|line| line.strip_prefix(RESULT_PREFIX.as_bytes()))
            .next_back()
            .ok_or_else(|| TrialError::Failed("trial produced no measurement".into()))?;
        serde_json::from_slice(result).map_err(failed)
    }
}

fn failed(error: impl std::fmt::Display) -> TrialError {
    TrialError::Failed(error.to_string())
}
