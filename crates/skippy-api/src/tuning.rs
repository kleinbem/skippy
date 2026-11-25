//! Reproducible settings sweeps over isolated Skippy model-serving trials.

use std::{path::PathBuf, time::Duration};

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

mod process;
pub use process::{ProcessTrialRunner, RESULT_PREFIX};
pub use process_supervision::Cancellation;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelTarget {
    pub id: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub bundle_dirs: Vec<PathBuf>,
    pub cache_dir: PathBuf,
    pub release: Option<String>,
    pub selection: Option<String>,
    pub identity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hardware {
    pub identity: String,
    pub backend_device: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub prompts: Vec<String>,
    pub warmup_runs: u32,
    pub repetitions: u32,
    pub max_tokens: u32,
    pub seed: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FlashAttention {
    Auto,
    Enabled,
    Disabled,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum Speculation {
    Auto,
    Disabled,
    NativeMtp,
    Ngram,
    DraftModel { path: PathBuf },
    Dflash { path: PathBuf },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    pub context: u32,
    pub batch: u32,
    pub microbatch: u32,
    pub mmap: bool,
    pub mlock: bool,
    pub flash_attention: FlashAttention,
    pub speculative: Speculation,
}

impl SettingsPatch {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.context > 0
                && self.batch > 0
                && self.microbatch > 0
                && self.microbatch <= self.batch,
            "context, batch and microbatch must be positive and microbatch must not exceed batch"
        );
        match &self.speculative {
            Speculation::DraftModel { path } | Speculation::Dflash { path } => ensure!(
                path.is_absolute(),
                "trial draft model path must be absolute"
            ),
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sweep {
    pub targets: Vec<ModelTarget>,
    pub runtime: Runtime,
    pub hardware: Hardware,
    pub workload: Workload,
    pub candidates: Vec<SettingsPatch>,
    pub timeout_secs: u64,
}

pub struct Trial<'a> {
    pub target: &'a ModelTarget,
    pub runtime: &'a Runtime,
    pub hardware: &'a Hardware,
    pub workload: &'a Workload,
    pub settings: &'a SettingsPatch,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialInput {
    pub target: ModelTarget,
    pub runtime: Runtime,
    pub hardware: Hardware,
    pub workload: Workload,
    pub settings: SettingsPatch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Measurement {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub elapsed_nanos: u64,
    pub requests: u64,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, thiserror::Error)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum TrialError {
    #[error("trial timed out")]
    Timeout,
    #[error("trial cancelled")]
    Cancelled,
    #[error("trial failed: {0}")]
    Failed(String),
}

pub trait TrialRunner {
    fn run(&self, trial: Trial<'_>, cancellation: &Cancellation)
    -> Result<Measurement, TrialError>;
}

#[derive(Debug, Serialize)]
pub struct TrialReport {
    pub candidate: usize,
    pub settings: SettingsPatch,
    pub result: Result<Measurement, TrialError>,
}

#[derive(Debug, Serialize)]
pub struct Recommendation {
    pub candidate: usize,
    pub patch: SettingsPatch,
}

#[derive(Debug, Serialize)]
pub struct ModelReport {
    pub target: ModelTarget,
    pub trials: Vec<TrialReport>,
    pub recommendation: Option<Recommendation>,
}

#[derive(Debug, Serialize)]
pub struct SweepReport {
    pub runtime: Runtime,
    pub hardware: Hardware,
    pub workload: Workload,
    pub models: Vec<ModelReport>,
    pub cancelled: bool,
}

pub fn run(
    sweep: &Sweep,
    runner: &impl TrialRunner,
    cancellation: &Cancellation,
) -> Result<SweepReport> {
    validate(sweep)?;
    let mut models = Vec::new();
    for target in &sweep.targets {
        let mut trials = Vec::new();
        for (candidate, settings) in sweep.candidates.iter().enumerate() {
            if cancellation.is_cancelled() {
                break;
            }
            let trial = Trial {
                target,
                runtime: &sweep.runtime,
                hardware: &sweep.hardware,
                workload: &sweep.workload,
                settings,
                timeout: Duration::from_secs(sweep.timeout_secs),
            };
            let result = settings
                .validate()
                .map_err(|error| TrialError::Failed(error.to_string()))
                .and_then(|()| runner.run(trial, cancellation));
            trials.push(TrialReport {
                candidate,
                settings: settings.clone(),
                result,
            });
        }
        let recommendation = recommend(&trials);
        models.push(ModelReport {
            target: target.clone(),
            trials,
            recommendation,
        });
    }
    Ok(SweepReport {
        runtime: sweep.runtime.clone(),
        hardware: sweep.hardware.clone(),
        workload: sweep.workload.clone(),
        models,
        cancelled: cancellation.is_cancelled(),
    })
}

fn validate(sweep: &Sweep) -> Result<()> {
    ensure!(
        !sweep.targets.is_empty() && !sweep.candidates.is_empty(),
        "provide model targets and candidates"
    );
    ensure!(
        (1..=86400).contains(&sweep.timeout_secs),
        "trial timeout must be between 1 and 86400 seconds"
    );
    ensure!(
        !sweep.runtime.identity.is_empty() && !sweep.hardware.identity.is_empty(),
        "runtime and hardware provenance must be specified"
    );
    ensure!(
        sweep.runtime.cache_dir.is_absolute()
            && sweep
                .runtime
                .bundle_dirs
                .iter()
                .all(|path| path.is_absolute()),
        "runtime paths must be absolute"
    );
    let mut ids = std::collections::BTreeSet::new();
    for target in &sweep.targets {
        skippy_inference_api::ModelId::new(&target.id)?;
        ensure!(
            target.path.is_absolute() && ids.insert(&target.id),
            "model targets require unique IDs and absolute paths"
        );
    }
    let workload = &sweep.workload;
    ensure!(
        !workload.prompts.is_empty()
            && workload
                .prompts
                .iter()
                .all(|prompt| !prompt.trim().is_empty())
            && workload.max_tokens > 0
            && workload.repetitions > 0,
        "prompts, repetitions, and output token limit must be nonempty"
    );
    Ok(())
}

fn recommend(trials: &[TrialReport]) -> Option<Recommendation> {
    let best = trials
        .iter()
        .filter_map(|trial| {
            let measure = trial.result.as_ref().ok()?;
            (measure.elapsed_nanos > 0 && measure.completion_tokens > 0 && measure.requests > 0)
                .then_some((trial, measure))
        })
        .max_by(|(a, left), (b, right)| {
            (u128::from(left.completion_tokens) * u128::from(right.elapsed_nanos))
                .cmp(&(u128::from(right.completion_tokens) * u128::from(left.elapsed_nanos)))
                .then_with(|| b.candidate.cmp(&a.candidate))
        })?
        .0;
    Some(Recommendation {
        candidate: best.candidate,
        patch: best.settings.clone(),
    })
}

#[cfg(test)]
mod tests;
