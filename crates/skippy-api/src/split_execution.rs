//! Placement, native admission, and lifecycle for a model-scoped split.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::PathBuf,
};

use anyhow::{Context, Result, ensure};
use skippy_protocol::{StageAdmissionDescriptor, StageConfig};

use crate::package::SkippyPackageIdentity;

mod direct;
pub use direct::plan_direct_gguf;
pub use skippy_coordinator::topology::{LockedTopologyStage, TopologyNode, TopologyPlanningInput};
pub use skippy_serving::readiness::{StageReadinessProbe, start_binary_stage_ready_probe};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SplitWorker {
    pub id: String,
    pub endpoint: SocketAddr,
}

pub enum SplitPlacement {
    /// Explicit endpoints and proportionate ranges, as used by plan-split.
    Balanced,
    /// Explicit measured worker capacity with optional locked placements.
    Measured {
        input: Box<TopologyPlanningInput>,
        locked: Vec<LockedTopologyStage>,
    },
}

pub struct SplitRequest {
    pub model_path: PathBuf,
    pub model_id: Option<String>,
    pub expected_manifest_sha256: Option<String>,
    pub run_id: String,
    pub workers: Vec<SplitWorker>,
    pub placement: SplitPlacement,
    pub ctx_size: u32,
    pub lanes: u32,
    pub n_gpu_layers: i32,
}

pub struct AdmittedSplit {
    identity: SkippyPackageIdentity,
    topology: skippy_topology::TopologyPlan,
    configs: Vec<StageConfig>,
    admissions: Vec<StageAdmissionDescriptor>,
}

impl AdmittedSplit {
    pub fn identity(&self) -> &SkippyPackageIdentity {
        &self.identity
    }
    pub fn topology(&self) -> &skippy_topology::TopologyPlan {
        &self.topology
    }
    pub fn configs(&self) -> &[StageConfig] {
        &self.configs
    }
    pub fn admissions(&self) -> &[StageAdmissionDescriptor] {
        &self.admissions
    }
    /// Validate the caller's current identity and workers before starting
    /// anything. Start downstream first; release every previously started
    /// stage on failure. Workers must verify the supplied package/admission
    /// at their native load boundary.
    pub fn start<'a, O: StageOperations>(
        &'a self,
        manifest_sha256: &str,
        workers: &[SplitWorker],
        operations: &'a mut O,
    ) -> Result<SplitExecution<'a, O>> {
        ensure!(
            manifest_sha256 == self.identity.manifest_sha256,
            "stale split manifest identity"
        );
        validate_workers(workers)?;
        let inventory: BTreeMap<_, _> = workers
            .iter()
            .map(|worker| (worker.id.as_str(), worker))
            .collect();
        let assigned = self
            .topology
            .stages
            .iter()
            .zip(&self.configs)
            .map(|(stage, config)| {
                let worker = inventory
                    .get(stage.node_id.as_str())
                    .context("missing admitted split worker")?;
                ensure!(
                    config.bind_addr == worker.endpoint.to_string(),
                    "worker {} endpoint changed",
                    worker.id
                );
                Ok((*worker, config))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut execution = SplitExecution {
            operations,
            stages: Vec::new(),
        };
        for (worker, config) in assigned.into_iter().rev() {
            match execution.operations.start(worker, config) {
                Ok(handle) => execution.stages.push((config.stage_index, handle)),
                Err(error) => {
                    let error = error.context(format!(
                        "start split stage {} at {}",
                        config.stage_id, worker.id
                    ));
                    return match execution.cleanup() {
                        Ok(()) => Err(error),
                        Err(cleanup) => {
                            Err(error.context(format!("split rollback also failed: {cleanup:#}")))
                        }
                    };
                }
            }
        }
        Ok(execution)
    }
}

fn validate_workers(workers: &[SplitWorker]) -> Result<()> {
    ensure!(
        workers.len() >= 2,
        "a split needs at least two --worker endpoints"
    );
    let mut ids = BTreeSet::new();
    let mut endpoints = BTreeSet::new();
    for worker in workers {
        ensure!(
            !worker.id.is_empty() && ids.insert(&worker.id),
            "worker IDs must be nonempty and distinct"
        );
        ensure!(
            endpoints.insert(worker.endpoint),
            "worker endpoints must be distinct"
        );
        ensure!(
            worker.endpoint.port() != 0 && !worker.endpoint.ip().is_unspecified(),
            "workers require a nonzero port and a routable or loopback address"
        );
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageState {
    Starting,
    Ready,
    Exited,
}

/// Caller-owned worker transport and artifact acquisition. A failed start must
/// clean up its own partial work. Stop must be idempotent and release the
/// stage's resources before returning.
pub trait StageOperations {
    type Handle;

    fn start(&mut self, worker: &SplitWorker, config: &StageConfig) -> Result<Self::Handle>;
    fn observe(&mut self, handle: &Self::Handle) -> Result<StageState>;
    fn stop(&mut self, handle: &mut Self::Handle) -> Result<()>;
}

#[must_use = "retain the split execution until it is ready to stop"]
pub struct SplitExecution<'a, O: StageOperations> {
    operations: &'a mut O,
    stages: Vec<(u32, O::Handle)>,
}

impl<O: StageOperations> SplitExecution<'_, O> {
    pub fn observe(&mut self) -> Result<Vec<(u32, StageState)>> {
        self.stages
            .iter()
            .rev()
            .map(|(index, handle)| Ok((*index, self.operations.observe(handle)?)))
            .collect()
    }

    /// Cancel startup or stop a running execution. Reports all failed stops.
    pub fn cancel(mut self) -> Result<()> {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        while let Some((index, mut handle)) = self.stages.pop() {
            if let Err(error) = self.operations.stop(&mut handle) {
                errors.push(format!("stage {index}: {error:#}"));
            }
        }
        ensure!(errors.is_empty(), "{}", errors.join("; "));
        Ok(())
    }
}

impl<O: StageOperations> Drop for SplitExecution<'_, O> {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            tracing::warn!(%error, "split execution cleanup failed");
        }
    }
}

#[cfg(test)]
mod tests;
