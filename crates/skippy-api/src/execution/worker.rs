use std::{sync::Arc, time::Duration};

use anyhow::{Context, ensure};
use skippy_protocol::{LoadMode, StageAdmissionDescriptor, validate_stage_admission_descriptor};
use skippy_serving::{
    EmbeddedServerHandle, EmbeddedState, binary_transport::BinaryStageOptions,
    compute_meter::StageComputeMeter, readiness::start_binary_stage_ready_probe,
};

use crate::stage_load::admitted_activation_frontier;

use super::{
    DraftRequirements, ExecutionError, ExecutionErrorKind, ExecutionSnapshot, ExecutionState,
    ResolvedSettings, RuntimeBundle, RuntimeIdentity, StageCompatibility,
};

/// Owning specialist downstream worker. The host authenticates its peers and
/// supplies their admission evidence and transport endpoints.
pub struct StageWorker {
    server: EmbeddedServerHandle,
    layout: ExecutionSnapshot,
    meter: Arc<StageComputeMeter>,
}

impl StageWorker {
    /// Start a worker from an independently admitted stage and resolved options.
    /// Each adjacent peer supplies exact compatibility evidence in upstream,
    /// then downstream order. The host binds this evidence to its endpoints.
    pub fn start_admitted(
        bundle: &RuntimeBundle,
        options: BinaryStageOptions,
        admission: &StageAdmissionDescriptor,
        peers: &[StageCompatibility],
    ) -> Result<Self, ExecutionError> {
        validate_admitted_worker(&options, admission).map_err(admission_failed)?;
        let expected_peers = 1 + usize::from(options.config.downstream.is_some());
        if peers.len() != expected_peers {
            return Err(admission_failed(anyhow::anyhow!(
                "expected compatibility evidence for {expected_peers} adjacent peers"
            )));
        }
        for peer in peers {
            StageCompatibility::current().require_peer(peer)?;
        }
        let runtime = bundle.activate()?;
        Self::start(options, runtime)
    }

    pub(super) fn start(
        mut options: BinaryStageOptions,
        runtime: RuntimeIdentity,
    ) -> Result<Self, ExecutionError> {
        tokio::runtime::Handle::try_current()
            .context("start binary worker in a Tokio runtime")
            .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidOptions, cause))?;
        // Keep an admitted specialist's existing meter when supplied.
        let meter = options
            .compute_meter
            .get_or_insert_with(Arc::default)
            .clone();
        let config = &options.config;
        let layout = ExecutionSnapshot {
            state: ExecutionState::Starting,
            runtime,
            layers: config.layer_start..config.layer_end,
            selected_device: config.selected_device.clone(),
            settings: ResolvedSettings::new(config, config.lane_count as usize),
            input: None,
            output: None,
            draft: None,
            compute: None,
            failure: None,
        };
        Ok(Self {
            server: skippy_serving::start_binary_stage(options),
            layout,
            meter,
        })
    }

    pub fn inspect(&self) -> ExecutionSnapshot {
        let status = self.server.status();
        let mut snapshot = self.layout.clone();
        snapshot.state = match status.state {
            EmbeddedState::Starting => ExecutionState::Starting,
            EmbeddedState::Ready => ExecutionState::Ready,
            EmbeddedState::Stopping => ExecutionState::Draining,
            EmbeddedState::Stopped => ExecutionState::Stopped,
            EmbeddedState::Failed => ExecutionState::Failed,
        };
        if status.input_activation_boundary.is_some() {
            snapshot.compute = Some(self.meter.snapshot().into());
        }
        snapshot.input = status.input_activation_boundary;
        snapshot.output = status.output_activation_boundary;
        snapshot.failure = status.last_error.or(snapshot.failure);
        if snapshot.failure.is_some() {
            snapshot.state = ExecutionState::Failed;
        }
        snapshot.draft = status
            .split_dflash_target_layers
            .map(|target_layers| DraftRequirements {
                target_layers,
                context_window: status.split_dflash_context_window,
                feature_encodings: status.split_dflash_feature_encodings,
            });
        snapshot
    }

    /// Wait for native load and a wire handshake. If startup fails, stop and
    /// join the worker before returning. A cancelled wait can be tried again.
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<(), ExecutionError> {
        if timeout.is_zero() || timeout > Duration::from_secs(14_400) {
            return Err(ExecutionError::new(
                ExecutionErrorKind::InvalidOptions,
                anyhow::anyhow!("worker readiness timeout must be positive and at most four hours"),
            ));
        }
        let mut probe = start_binary_stage_ready_probe(self.server.status().bind_addr, timeout);
        let result = tokio::select! {
            biased;
            result = self.server.join() => {
                probe.cancel_and_join().await;
                result.and_then(|()| anyhow::bail!("worker stopped before readiness"))
            }
            result = probe.wait() => result,
        };
        if let Err(cause) = result {
            self.server.request_shutdown();
            let cause = match self.server.join().await {
                Ok(()) => cause,
                Err(cleanup) => cause.context(format!("worker teardown also failed: {cleanup:#}")),
            };
            self.layout.failure = Some(format!("{cause:#}"));
            return Err(execution_failed(cause));
        }
        if self.server.status().state != EmbeddedState::Ready {
            self.stop().await?;
            return Err(execution_failed(anyhow::anyhow!(
                "worker did not publish ready state"
            )));
        }
        Ok(())
    }

    /// Stop accepting stage traffic and join the worker. Drain frontend work
    /// before stopping downstream workers in the chain.
    pub async fn stop(&mut self) -> Result<(), ExecutionError> {
        self.server.request_shutdown();
        self.server.join().await.map_err(execution_failed)
    }
}

fn validate_admitted_worker(
    options: &BinaryStageOptions,
    admission: &StageAdmissionDescriptor,
) -> anyhow::Result<()> {
    let config = &options.config;
    skippy_config::validate_config(config, options.topology.as_ref())?;
    validate_stage_admission_descriptor(&admission.clone().into())?;
    ensure!(
        config.stage_index > 0
            && config.upstream.is_some()
            && options.bind_addr.port() != 0
            && !options.bind_addr.ip().is_unspecified()
            && config.bind_addr.parse::<std::net::SocketAddr>()? == options.bind_addr,
        "downstream stage requires an upstream and an explicit matching bind endpoint"
    );
    let frontier = admitted_activation_frontier(admission)?;
    ensure!(
        config.load_mode != LoadMode::LayerPackage
            && (config.layer_start, config.layer_end)
                == (admission.layer_start, admission.layer_end)
            && config.execution_contract == admission.execution_contract
            && !config.execution_contract.is_empty()
            && !config.resident_tensor_names.is_empty()
            && config.activation_import_identities == frontier.activation_imports
            && config.activation_export_identities == frontier.activation_exports
            && config.activation_import_bindings == frontier.activation_import_bindings
            && config.activation_export_bindings == frontier.activation_export_bindings,
        "worker config differs from the admitted layer graph and frontier"
    );
    Ok(())
}

pub(super) fn admission_failed(cause: anyhow::Error) -> ExecutionError {
    ExecutionError::native(cause, ExecutionErrorKind::InvalidAdmission)
}

pub(super) fn execution_failed(cause: anyhow::Error) -> ExecutionError {
    ExecutionError::native(cause, ExecutionErrorKind::ExecutionFailed)
}
