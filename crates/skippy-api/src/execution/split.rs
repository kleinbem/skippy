use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::Context;
use skippy_inference_api::InferenceExecutionScope;
use skippy_protocol::{StageConfig, StageTopology};
use skippy_serving::binary_transport::{BinaryStageOptions, WireCondition};

use super::{
    ExecutionCapabilities, ExecutionError, ExecutionErrorKind, ExecutionSnapshot, ExecutionState,
    GenerationRequest, GenerationStream, ModelHandle, ModelOptions, RuntimeBundle, RuntimeIdentity,
    StageCompatibility, StageDevice,
    model::model_request,
    worker::{StageWorker, admission_failed, execution_failed},
};

mod plan;

/// An admitted two-stage GGUF chain in one embedding process. Both bind
/// addresses are supplied by the host. Stage execution uses the native
/// activation transport, and generation is in process.
pub struct SplitOptions {
    model: ModelOptions,
    endpoints: [SocketAddr; 2],
    peer: StageCompatibility,
    startup_timeout: Duration,
}

impl SplitOptions {
    pub fn new(
        model_id: impl Into<String>,
        model_path: impl Into<PathBuf>,
        runtime: RuntimeBundle,
        return_endpoint: SocketAddr,
        worker_endpoint: SocketAddr,
        worker_compatibility: StageCompatibility,
    ) -> Result<Self, ExecutionError> {
        let endpoints = [return_endpoint, worker_endpoint];
        let valid = endpoints
            .iter()
            .all(|endpoint| endpoint.port() != 0 && !endpoint.ip().is_unspecified())
            && return_endpoint != worker_endpoint;
        if !valid {
            return Err(ExecutionError::new(
                ExecutionErrorKind::InvalidOptions,
                anyhow::anyhow!(
                    "split requires distinct, concrete stage endpoints with nonzero ports"
                ),
            ));
        }
        StageCompatibility::current().require_peer(&worker_compatibility)?;
        Ok(Self {
            model: ModelOptions::new(model_id, model_path, runtime)?,
            endpoints,
            peer: worker_compatibility,
            startup_timeout: Duration::from_secs(900),
        })
    }

    pub fn with_limits(mut self, context_tokens: u32, lanes: u32, max_output_tokens: u32) -> Self {
        self.model = self
            .model
            .with_limits(context_tokens, lanes, max_output_tokens);
        self
    }

    pub fn with_device(mut self, device: StageDevice, gpu_layers: i32) -> Self {
        self.model = self.model.with_device(device, gpu_layers);
        self
    }

    pub fn with_backend_device(mut self, name: impl Into<String>, gpu_layers: i32) -> Self {
        self.model = self.model.with_backend_device(name, gpu_layers);
        self
    }

    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    /// Admit both stage graphs, start the worker, and expose the frontend only
    /// after a wire readiness handshake. Requires a Tokio runtime.
    pub async fn load(self) -> Result<SplitExecution, ExecutionError> {
        if self.startup_timeout.is_zero() || self.startup_timeout > Duration::from_secs(14_400) {
            return Err(ExecutionError::new(
                ExecutionErrorKind::InvalidOptions,
                anyhow::anyhow!("worker startup timeout must be between zero and four hours"),
            ));
        }
        tokio::runtime::Handle::try_current()
            .context("start split execution in a Tokio runtime")
            .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidOptions, cause))?;
        let timeout = self.startup_timeout;
        let plan = tokio::task::spawn_blocking(move || self.plan())
            .await
            .context("join split planning")
            .map_err(execution_failed)??;
        load_admitted_split(plan, timeout).await
    }
}

struct SplitPlan {
    front: StageConfig,
    worker: StageConfig,
    topology: StageTopology,
    scope: InferenceExecutionScope,
    runtime: RuntimeIdentity,
    max_output_tokens: u32,
}

async fn load_admitted_split(
    plan: SplitPlan,
    timeout: Duration,
) -> Result<SplitExecution, ExecutionError> {
    let endpoint = plan
        .worker
        .bind_addr
        .parse()
        .context("parse admitted worker endpoint")
        .map_err(admission_failed)?;
    let options = worker_options(&plan, endpoint)?;
    let mut worker = StageWorker::start(options, plan.runtime.clone())?;
    worker.wait_ready(timeout).await?;
    let frontend = start_frontend(&plan, &worker).await;
    match frontend {
        Ok(front) => Ok(SplitExecution { front, worker }),
        Err(error) => {
            if let Err(cleanup) = worker.stop().await {
                return Err(ExecutionError::new(
                    error.kind(),
                    anyhow::Error::new(error)
                        .context(format!("worker teardown also failed: {cleanup:#}")),
                ));
            }
            Err(error)
        }
    }
}

async fn start_frontend(
    plan: &SplitPlan,
    worker: &StageWorker,
) -> Result<ModelHandle, ExecutionError> {
    let request = model_request(
        plan.front.clone(),
        Some(plan.topology.clone()),
        plan.max_output_tokens,
    )?;
    let runtime = plan.runtime.clone();
    let scope = plan.scope;
    let front = tokio::task::spawn_blocking(move || ModelHandle::load(request, runtime, scope))
        .await
        .context("join stage zero load")
        .map_err(execution_failed)??;
    front
        .capabilities()
        .require(ExecutionCapabilities::STREAMED_GENERATION)?;
    if worker.inspect().state() != ExecutionState::Ready {
        return Err(execution_failed(anyhow::anyhow!(
            "worker stopped while stage zero was loading"
        )));
    }
    Ok(front)
}

fn worker_options(
    plan: &SplitPlan,
    bind_addr: SocketAddr,
) -> Result<BinaryStageOptions, ExecutionError> {
    let config = plan.worker.clone();
    Ok(BinaryStageOptions {
        max_inflight: config.lane_count as usize,
        config,
        topology: Some(plan.topology.clone()),
        bind_addr,
        tuning: Default::default(),
        metrics_otlp_grpc: None,
        telemetry_queue_capacity: 0,
        telemetry_level: skippy_serving::telemetry::TelemetryLevel::Off,
        reply_credit_limit: None,
        async_prefill_forward: false,
        downstream_wire_condition: WireCondition::new(0.0, None).map_err(admission_failed)?,
        downstream_connect_timeout_secs:
            skippy_config::local_serving::DOWNSTREAM_CONNECT_TIMEOUT_SECS,
        native_mtp_enabled: false,
        last_stage_decode_batch: None,
        continuous_batching: false,
        openai: None,
        compute_meter: None,
        l3_manager: None,
        split_dflash: None,
    })
}

/// Owns the admitted frontend and its explicitly addressed stage worker.
pub struct SplitExecution {
    front: ModelHandle,
    worker: StageWorker,
}

impl SplitExecution {
    pub fn capabilities(&self) -> &ExecutionCapabilities {
        self.front.capabilities()
    }

    pub fn inspect_frontend(&self) -> ExecutionSnapshot {
        self.front.inspect()
    }

    pub fn inspect_worker(&self) -> ExecutionSnapshot {
        self.worker.inspect()
    }

    pub async fn generate(
        &self,
        request: GenerationRequest,
    ) -> Result<GenerationStream, ExecutionError> {
        if self.worker.inspect().state() != ExecutionState::Ready {
            return Err(ExecutionError::new(
                ExecutionErrorKind::NotReady,
                anyhow::anyhow!("downstream stage is not ready"),
            ));
        }
        self.front.generate(request).await
    }

    /// Drain admitted generation before stopping the downstream worker. A
    /// cancelled stop can be awaited again. A completed stop is idempotent.
    pub async fn stop(&mut self) -> Result<(), ExecutionError> {
        self.front.stop().await;
        self.worker.stop().await
    }
}
