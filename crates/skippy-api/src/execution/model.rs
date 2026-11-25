use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, ensure};
use skippy_inference_api::{InferenceExecutionScope, InferenceRequestContext};
use skippy_protocol::{StageConfig, StageTopology};
use skippy_runtime::{ModelWorkload, MtpSource};
use skippy_serving::{
    EmbeddedRuntimeOptions, binary_transport::WireCondition, compute_meter::StageComputeMeter,
    runtime_state::panic_recovery::lock_runtime,
};

use crate::{
    SingleStageOptions,
    serving::{
        InferenceOptions, LoadedModelBackend, ModelLoadRequest, ModelOpenEvents,
        ServingTelemetryOptions,
    },
    single_stage_config,
};

use super::{
    ExecutionCapabilities, ExecutionError, ExecutionErrorKind, ExecutionSnapshot, ExecutionState,
    GenerationRequest, GenerationStream, ResolvedSettings, RuntimeBundle, RuntimeIdentity,
    StageDevice, stream::CancelStartup,
};

mod drain;
use drain::Drain;

/// Explicit inputs for one locally supplied model. Speculation is opt-in via
/// the specialist lifecycle and is disabled by this builder.
#[derive(Clone, Debug)]
pub struct ModelOptions {
    pub(super) runtime: RuntimeBundle,
    pub(super) stage: SingleStageOptions,
    telemetry: ServingTelemetryOptions,
    threads: Option<usize>,
    queue_capacity: Option<usize>,
    pub(super) max_output_tokens: u32,
}

impl ModelOptions {
    pub fn new(
        model_id: impl Into<String>,
        model_path: impl Into<PathBuf>,
        runtime: RuntimeBundle,
    ) -> Result<Self, ExecutionError> {
        let mut stage = SingleStageOptions::new(model_id, model_path);
        if stage.model_id.trim().is_empty() || stage.model_path.as_os_str().is_empty() {
            return Err(ExecutionError::new(
                ExecutionErrorKind::InvalidOptions,
                anyhow::anyhow!("model id and model path are required"),
            ));
        }
        stage.native_mtp_enabled = false;
        Ok(Self {
            runtime,
            stage,
            telemetry: ServingTelemetryOptions::off(),
            threads: None,
            queue_capacity: None,
            max_output_tokens: 128,
        })
    }

    pub fn with_limits(mut self, context_tokens: u32, lanes: u32, max_output_tokens: u32) -> Self {
        self.stage.ctx_size = context_tokens;
        self.stage.generation_concurrency = lanes as usize;
        self.max_output_tokens = max_output_tokens;
        self
    }

    pub fn with_device(mut self, device: StageDevice, gpu_layers: i32) -> Self {
        self.stage.selected_device = Some(device);
        self.stage.n_gpu_layers = gpu_layers;
        self
    }

    pub fn with_backend_device(self, name: impl Into<String>, gpu_layers: i32) -> Self {
        self.with_device(
            StageDevice {
                backend_device: name.into(),
                stable_id: None,
                index: None,
                vram_bytes: None,
            },
            gpu_layers,
        )
    }

    pub fn with_threads(mut self, threads: usize) -> Self {
        self.threads = Some(threads);
        self
    }

    pub fn with_queue_capacity(mut self, capacity: usize) -> Self {
        self.queue_capacity = Some(capacity);
        self
    }

    pub fn with_telemetry(mut self, telemetry: ServingTelemetryOptions) -> Self {
        self.telemetry = telemetry;
        self
    }

    /// Open model resources synchronously. On failure all per-model resources
    /// are dropped. A successfully activated native bundle remains process-wide.
    pub fn load(self) -> Result<ModelHandle, ExecutionError> {
        self.load_with_model_open_events(ModelOpenEvents::Disabled)
    }

    /// Open the model with typed diagnostics for this load. The returned
    /// result determines readiness independently of observation delivery.
    pub fn load_with_model_open_events(
        mut self,
        open_events: ModelOpenEvents,
    ) -> Result<ModelHandle, ExecutionError> {
        self.validate()
            .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidOptions, cause))?;
        let model_path = local_model_path(&self.stage.model_path)?;
        self.stage.model_path = model_path.clone();
        let runtime = self.runtime.activate()?;
        let identity =
            crate::source::synthetic_direct_gguf_package(&self.stage.model_id, &model_path)
                .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidPackage, cause))?;
        let scope = InferenceExecutionScope::new();
        let config = single_stage_config(&self.stage, identity.into(), scope.to_string())
            .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidPackage, cause))?;
        let mut request = self.request(config, None)?;
        request.open_events = open_events;
        if let Some(capacity) = self.queue_capacity {
            request.openai.generation_queue_capacity = capacity;
        }
        ModelHandle::load(request, runtime, scope)
    }

    pub(super) fn validate(&self) -> anyhow::Result<()> {
        self.stage.validate()?;
        ensure!(
            self.max_output_tokens > 0,
            "max output tokens must be positive"
        );
        ensure!(self.threads != Some(0), "thread count must be positive");
        ensure!(self.stage.n_gpu_layers >= -1, "GPU layer count is below -1");
        Ok(())
    }

    pub(super) fn request(
        &self,
        config: StageConfig,
        topology: Option<StageTopology>,
    ) -> Result<ModelLoadRequest, ExecutionError> {
        let mut request = model_request(config, topology, self.max_output_tokens)?;
        request.runtime.n_threads = self.threads;
        request.runtime.n_threads_batch = self.threads;
        request.serving_telemetry = Some(self.telemetry.clone());
        Ok(request)
    }
}

pub(super) fn local_model_path(path: &Path) -> Result<PathBuf, ExecutionError> {
    let locate = || -> anyhow::Result<PathBuf> {
        let path = path
            .canonicalize()
            .with_context(|| format!("resolve model {}", path.display()))?;
        ensure!(
            path.is_file() && path.to_str().is_some(),
            "model path must name a UTF-8 file"
        );
        Ok(path)
    };
    locate().map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidPackage, cause))
}

pub(super) fn model_request(
    config: StageConfig,
    topology: Option<StageTopology>,
    max_output_tokens: u32,
) -> Result<ModelLoadRequest, ExecutionError> {
    let mut openai = InferenceOptions::direct_single_stage_defaults(
        config.model_id.clone(),
        max_output_tokens,
        config.lane_count as usize,
        false,
    );
    // A single-stage scheduler can batch locally. The embedded split path
    // uses the existing serial pipeline unless explicitly tuned by a specialist.
    openai.continuous_batching &= config.downstream.is_none();
    let telemetry = ServingTelemetryOptions::off();
    let downstream_wire_condition = WireCondition::new(0.0, None)
        .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidOptions, cause))?;
    Ok(ModelLoadRequest {
        runtime: EmbeddedRuntimeOptions {
            config,
            topology,
            n_threads: None,
            n_threads_batch: None,
            mtp_source: MtpSource::Disabled,
            recurrent_rollback_tokens: 0,
            metrics_otlp_grpc: None,
            telemetry_queue_capacity: 0,
            telemetry_level: telemetry.level,
            session_lifecycle_observer: None,
        },
        openai,
        open_events: ModelOpenEvents::Disabled,
        hooks_factory: None,
        generation_observer: None,
        kv_observer: None,
        hook_policy: None,
        guardrails: None,
        guardrail_telemetry: None,
        downstream_wire_condition,
        serving_telemetry: None,
        l3_manager: None,
    })
}

/// Owns an in-process serving backend and its load-specific native resources.
pub struct ModelHandle {
    resources: Drain<LoadedModelBackend>,
    scope: InferenceExecutionScope,
    capabilities: ExecutionCapabilities,
    layout: ExecutionSnapshot,
    meter: Option<Arc<StageComputeMeter>>,
}

impl ModelHandle {
    /// Adopt the specialist lifecycle with the same stream and drain guarantees.
    /// The caller supplies the admitted topology, verified package and explicit
    /// serving policy through the existing lifecycle extension points.
    pub fn load_specialist(
        request: ModelLoadRequest,
        bundle: &RuntimeBundle,
    ) -> Result<Self, ExecutionError> {
        let runtime = bundle.activate()?;
        Self::load(request, runtime, InferenceExecutionScope::new())
    }

    pub(super) fn load(
        request: ModelLoadRequest,
        runtime: RuntimeIdentity,
        scope: InferenceExecutionScope,
    ) -> Result<Self, ExecutionError> {
        let loaded = request
            .load()
            .map_err(|cause| ExecutionError::native(cause, ExecutionErrorKind::ExecutionFailed))?;
        let workload = loaded.runtime.workload_info().map_err(|cause| {
            ExecutionError::native(cause, ExecutionErrorKind::UnsupportedCapability)
        })?;
        let streaming = matches!(
            workload.kind,
            ModelWorkload::CausalGeneration | ModelWorkload::EncoderDecoder
        );
        let config = &loaded.config;
        let staged = config.downstream.is_some()
            && !config.execution_contract.is_empty()
            && !config.activation_export_bindings.is_empty();
        let capabilities = ExecutionCapabilities {
            workload: Some(workload),
            streaming,
            staged,
        };
        let meter = streaming.then(|| lock_runtime(&loaded.runtime.runtime()).compute_meter());
        let layout = ExecutionSnapshot {
            state: ExecutionState::Ready,
            runtime,
            layers: config.layer_start..config.layer_end,
            selected_device: config.selected_device.clone(),
            settings: ResolvedSettings::new(config, loaded.runtime.status().sessions.lane_count),
            input: loaded.runtime.input_activation_boundary(),
            output: loaded.runtime.output_activation_boundary(),
            draft: None,
            failure: None,
            compute: None,
        };
        Ok(Self {
            resources: Drain::new(loaded),
            scope,
            capabilities,
            layout,
            meter,
        })
    }

    pub fn capabilities(&self) -> &ExecutionCapabilities {
        &self.capabilities
    }

    pub fn inspect(&self) -> ExecutionSnapshot {
        let mut snapshot = self.layout.clone();
        snapshot.state = self.resources.state();
        snapshot.compute = self.meter.as_ref().map(|meter| meter.snapshot().into());
        snapshot
    }

    /// Start a generation and return an owning response stream. Dropping
    /// startup or the stream signals cancellation to the native worker.
    pub async fn generate(
        &self,
        mut request: GenerationRequest,
    ) -> Result<GenerationStream, ExecutionError> {
        self.capabilities
            .require(ExecutionCapabilities::STREAMED_GENERATION)?;
        let lease = self.resources.admit()?;
        let context =
            InferenceRequestContext::new().with_execution_scope(self.scope, lease.clone());
        let cancellation = context.cancellation_token();
        let mut startup = CancelStartup(Some(cancellation.clone()));
        request.stream = true;
        let stream = lease
            .resources()
            .backend
            .chat_completion_stream(request, context)
            .await
            .map_err(ExecutionError::inference)?;
        startup.0 = None;
        Ok(GenerationStream::new(stream, cancellation))
    }

    /// Refuse new requests and wait for admitted work to release its native
    /// execution leases. Repeated calls complete after the same resource drop.
    pub async fn stop(&self) {
        self.resources.stop().await;
    }
}
