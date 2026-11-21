use std::net::SocketAddr;

use anyhow::{Context, ensure};
use skippy_inference_api::InferenceExecutionScope;
use skippy_protocol::{
    LoadMode, PeerConfig, StageAdmissionDescriptor, StageConfig, StageTopology, StageTopologyEntry,
};

use crate::{
    package::SkippyPackageIdentity,
    source::{
        planning::direct_gguf_planning_manifest_from_identity, synthetic_direct_gguf_package,
    },
    split_certification::require_split_certification,
    stage_admission::{StagePlannerProfile, realize_direct_gguf_stage_admissions},
    stage_load::{
        AdmittedStageOptions, ResolvedStagePackage, admitted_activation_frontier,
        admitted_resident_tensor_names, admitted_stage_config,
    },
};

use super::{SplitOptions, SplitPlan, admission_failed};
use crate::execution::{ExecutionError, ExecutionErrorKind, model::local_model_path};

impl SplitOptions {
    pub(super) fn plan(self) -> Result<SplitPlan, ExecutionError> {
        self.model
            .validate()
            .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidOptions, cause))?;
        let options = &self.model.stage;
        let path = local_model_path(&options.model_path)?;
        let runtime = self.model.runtime.activate()?;
        runtime.stage_compatibility().require_peer(&self.peer)?;
        let identity = synthetic_direct_gguf_package(&options.model_id, &path)
            .map_err(|cause| ExecutionError::new(ExecutionErrorKind::InvalidPackage, cause))?;
        let scope = InferenceExecutionScope::new();
        let (configs, topology) = self
            .admit(&identity, &scope.to_string(), runtime.backend())
            .map_err(admission_failed)?;
        let [front, worker] = configs;
        Ok(SplitPlan {
            front,
            worker,
            topology,
            scope,
            runtime,
            max_output_tokens: self.model.max_output_tokens,
        })
    }

    fn admit(
        &self,
        identity: &SkippyPackageIdentity,
        run: &str,
        backend: &str,
    ) -> anyhow::Result<([StageConfig; 2], StageTopology)> {
        let options = &self.model.stage;
        let (manifest, paths) =
            direct_gguf_planning_manifest_from_identity(&options.model_id, identity)?;
        let architecture = manifest
            .model_metadata
            .get("general.architecture")
            .and_then(serde_json::Value::as_str)
            .context("missing model architecture")?;
        require_split_certification(identity, architecture, self.allow_uncertified)?;
        ensure!(
            identity.layer_count >= 2,
            "two stages require at least two layers"
        );
        let middle = identity.layer_count / 2;
        let ranges = [(0, middle), (middle, identity.layer_count)];
        let lanes = u32::try_from(options.generation_concurrency)?;
        let profiles = planning_profiles(lanes, options.n_ubatch)?;
        let gpu_layers = options.n_gpu_layers;
        let device = options
            .selected_device
            .as_ref()
            .map(|device| device.backend_device.as_str());
        let backend_id =
            format!("skippy-backend:{backend}:gpu-layers:{gpu_layers}:device:{device:?}:v1");
        let graph_id = format!(
            "skippy-graph-configuration:v1:embed:{}:{lanes}:{:?}",
            options.ctx_size, options.n_batch
        );
        let stages = realize_direct_gguf_stage_admissions(
            &options.model_id,
            identity,
            &ranges,
            &profiles,
            &graph_id,
            &backend_id,
        )?;
        let [first, last]: [StageAdmissionDescriptor; 2] = stages
            .try_into()
            .map_err(|_| anyhow::anyhow!("planner did not admit two stages"))?;
        validate_frontier_pair(&first, &last)?;
        let package = ResolvedStagePackage {
            local_ref: identity.source_model_path.to_string_lossy().into_owned(),
            source_model_path: identity.source_model_path.to_string_lossy().into_owned(),
            source_model_sha256: identity.source_model_sha256.clone(),
            source_model_bytes: Some(identity.source_model_bytes),
            model_part_paths: paths,
            projector_path: None,
        };
        let topology = stage_topology(run, &options.model_id, self.endpoints, ranges);
        let stage_config = |index, admission| {
            let names = admitted_resident_tensor_names(&admission, &manifest)?;
            let load = self.stage_load(&topology, identity, index, admission);
            admitted_stage_config(&load, Some(&package), names)
        };
        Ok(([stage_config(0, first)?, stage_config(1, last)?], topology))
    }

    fn stage_load(
        &self,
        topology: &StageTopology,
        identity: &SkippyPackageIdentity,
        index: usize,
        admission: StageAdmissionDescriptor,
    ) -> AdmittedStageOptions {
        let options = &self.model.stage;
        let stage = &topology.stages[index];
        let peer = |index: usize| {
            let peer = &topology.stages[index];
            PeerConfig {
                stage_id: peer.stage_id.clone(),
                stage_index: peer.stage_index,
                endpoint: peer.endpoint.clone(),
                persistent_lane_marker: false,
            }
        };
        AdmittedStageOptions {
            topology_id: topology.topology_id.clone(),
            run_id: topology.topology_id.clone(),
            model_id: topology.model_id.clone(),
            stage_id: stage.stage_id.clone(),
            stage_index: stage.stage_index,
            layer_start: stage.layer_start,
            layer_end: stage.layer_end,
            ctx_size: options.ctx_size,
            lane_count: options.generation_concurrency as u32,
            selected_device: options.selected_device.clone(),
            max_request_tier: None,
            package_ref: identity.package_ref.clone(),
            manifest_sha256: identity.manifest_sha256.clone(),
            model_path: Some(identity.source_model_path.to_string_lossy().into_owned()),
            source_model_sha256: Some(identity.source_model_sha256.clone()),
            source_model_bytes: Some(identity.source_model_bytes),
            projector_path: None,
            projector_use_gpu: None,
            media_marker: None,
            image_min_tokens: None,
            image_max_tokens: None,
            batch_max_tokens: None,
            glm_dsa_policy: Default::default(),
            generation_signal_window: None,
            activation_codec: Default::default(),
            activation_codec_policy: Default::default(),
            n_batch: options.n_batch,
            n_ubatch: options.n_ubatch,
            n_gpu_layers: options.n_gpu_layers,
            mmap: None,
            mlock: false,
            runtime_settings: Default::default(),
            cache_type_k: options.cache_type_k.clone(),
            cache_type_v: options.cache_type_v.clone(),
            flash_attn_type: options.flash_attn_type,
            load_mode: LoadMode::RuntimeSlice,
            native_mtp_enabled: false,
            bind_addr: stage.endpoint.clone(),
            upstream: (index == 1).then(|| peer(0)),
            downstream: (index == 0).then(|| peer(1)),
            admission,
        }
    }
}

fn planning_profiles(lanes: u32, batch: Option<u32>) -> anyhow::Result<[StagePlannerProfile; 3]> {
    let prefill = batch.unwrap_or(8).min(8);
    let batched = lanes
        .checked_mul(prefill)
        .context("planning batch overflow")?;
    Ok([
        ("batched", batched, lanes),
        ("decode", lanes, lanes),
        ("prefill", prefill, 1),
    ]
    .map(|(id, tokens, sequences)| StagePlannerProfile {
        profile_id: id.to_owned(),
        n_tokens: tokens,
        n_sequences: sequences,
        n_outputs: tokens,
        n_recurrent_rollback_sequences: 0,
    }))
}

fn validate_frontier_pair(
    first: &StageAdmissionDescriptor,
    last: &StageAdmissionDescriptor,
) -> anyhow::Result<()> {
    ensure!(
        first.package_id == last.package_id && first.layer_end == last.layer_start,
        "stage admissions disagree on package or layer boundary"
    );
    let output = admitted_activation_frontier(first)?;
    let input = admitted_activation_frontier(last)?;
    ensure!(
        !output.activation_exports.is_empty()
            && output.activation_exports == input.activation_imports
            && output.activation_export_bindings == input.activation_import_bindings,
        "adjacent admitted activation frontiers do not match"
    );
    Ok(())
}

fn stage_topology(
    run: &str,
    model_id: &str,
    endpoints: [SocketAddr; 2],
    ranges: [(u32, u32); 2],
) -> StageTopology {
    let stages = endpoints
        .into_iter()
        .zip(ranges)
        .enumerate()
        .map(
            |(index, (endpoint, (layer_start, layer_end)))| StageTopologyEntry {
                stage_id: format!("stage-{index}"),
                stage_index: index as u32,
                host: None,
                endpoint: endpoint.to_string(),
                layer_start,
                layer_end,
                load_mode: LoadMode::RuntimeSlice,
            },
        )
        .collect();
    StageTopology {
        topology_id: run.into(),
        model_id: model_id.into(),
        stages,
    }
}
