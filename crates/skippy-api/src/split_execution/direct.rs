use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use skippy_coordinator::topology::{plan_locked_topology, plan_topology};
use skippy_protocol::{PeerConfig, StageAdmissionDescriptor, StageConfig};
use skippy_topology::{
    BoundaryDecision, NodeSpec, PlannerPolicy, TopologyPlan, TopologyPlanRequest,
    dense_attention_layers, infer_family_capability, plan_balanced_accepted_contiguous,
    plan_contiguous_with_splits,
};

use super::{AdmittedSplit, SplitPlacement, SplitRequest, validate_workers};
use crate::{package::SkippyPackageIdentity, source, stage_admission, stage_load};

/// Plan a direct GGUF execution and realize every native stage admission before
/// returning a plan that can be started. Run on a blocking thread.
pub fn plan_direct_gguf(args: &SplitRequest) -> Result<AdmittedSplit> {
    validate_workers(&args.workers)?;
    ensure!(
        args.ctx_size > 0 && args.lanes > 0 && !args.run_id.is_empty(),
        "split run ID, context size and lanes must be set"
    );
    let path = if args.model_path.is_absolute() {
        args.model_path.clone()
    } else {
        std::env::current_dir()?.join(&args.model_path)
    };
    skippy_model_ref::validate_gguf_model_path(&path).map_err(anyhow::Error::msg)?;
    ensure!(
        path.is_file(),
        "plan-split requires a GGUF file (the first shard for multipart models)"
    );
    let model_id = args.model_id.clone().unwrap_or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("local-model")
            .into()
    });
    let identity = source::synthetic_direct_gguf_package(&model_id, &path)?;
    ensure!(
        args.expected_manifest_sha256
            .as_ref()
            .is_none_or(|expected| *expected == identity.manifest_sha256),
        "model manifest changed before split admission"
    );
    ensure!(
        identity.layer_weight_bytes.len() == identity.layer_count as usize,
        "split planning requires weight sizes for every model layer"
    );
    let (manifest, _) =
        source::planning::direct_gguf_planning_manifest_from_identity(&model_id, &identity)?;
    let topology = topology_for(args, &model_id, &identity)?;
    let (configs, admissions) = realize_configs(args, &model_id, &identity, &manifest, &topology)?;
    Ok(AdmittedSplit {
        identity,
        topology,
        configs,
        admissions,
    })
}

fn topology_for(
    args: &SplitRequest,
    model_id: &str,
    identity: &SkippyPackageIdentity,
) -> Result<TopologyPlan> {
    let mut layers = dense_attention_layers(identity.layer_count, 1);
    for (layer, bytes) in layers.iter_mut().zip(&identity.layer_weight_bytes) {
        layer.parameter_bytes = *bytes;
    }
    let node = |id: &str, vram_bytes| NodeSpec {
        node_id: id.to_owned(),
        cached_slice_bytes: 0,
        vram_bytes,
    };
    let mut request = TopologyPlanRequest {
        topology_id: args.run_id.clone(),
        model_id: model_id.to_owned(),
        layers,
        nodes: args
            .workers
            .iter()
            .map(|worker| node(&worker.id, 0))
            .collect(),
        family: infer_family_capability(model_id, identity.layer_count, identity.activation_width),
        policy: PlannerPolicy::default(),
    };
    let SplitPlacement::Measured { input, locked } = &args.placement else {
        return Ok(plan_balanced_accepted_contiguous(
            &request,
            request.nodes.len(),
        )?);
    };
    ensure!(
        args.expected_manifest_sha256.is_some(),
        "measured split planning requires exact manifest identity"
    );
    ensure!(
        input.layer_count == identity.layer_count
            && input.layer_weight_bytes == identity.layer_weight_bytes,
        "capacity facts do not match model layers"
    );
    let mut input = input.clone();
    let worker_ids: std::collections::BTreeSet<_> = args
        .workers
        .iter()
        .map(|worker| worker.id.as_str())
        .collect();
    let capacity_ids: std::collections::BTreeSet<_> = input
        .nodes
        .iter()
        .map(|worker| worker.node_id.as_str())
        .collect();
    ensure!(
        worker_ids == capacity_ids && capacity_ids.len() == input.nodes.len(),
        "capacity facts must identify each worker exactly once"
    );
    input.minimum_nodes = input.minimum_nodes.max(2);
    input.context_length_override = Some(args.ctx_size);
    input.parallel_lanes_override = Some(usize::try_from(args.lanes)?);
    let plan = if locked.is_empty() {
        plan_topology(&input)?
    } else {
        plan_locked_topology(&input, locked)?
    };
    request.nodes = plan
        .stages
        .iter()
        .map(|stage| {
            let capacity = input
                .nodes
                .iter()
                .find(|node| node.node_id == stage.node_id)
                .context("coordinator assigned an unknown worker")?;
            Ok(node(&stage.node_id, capacity.detected_vram_bytes))
        })
        .collect::<Result<_>>()?;
    let splits: Vec<_> = plan
        .stages
        .iter()
        .skip(1)
        .map(|stage| stage.layer_start)
        .collect();
    let topology = plan_contiguous_with_splits(&request, &splits)?;
    ensure!(
        topology
            .boundaries
            .iter()
            .all(|boundary| boundary.decision == BoundaryDecision::Accepted),
        "measured ranges cross an incompatible stage boundary"
    );
    Ok(topology)
}

fn realize_configs(
    args: &SplitRequest,
    model_id: &str,
    identity: &SkippyPackageIdentity,
    manifest: &skippy_package_format::PackageManifest,
    topology: &TopologyPlan,
) -> Result<(Vec<StageConfig>, Vec<StageAdmissionDescriptor>)> {
    let ranges: Vec<_> = topology
        .stages
        .iter()
        .map(|stage| (stage.layer_start, stage.layer_end))
        .collect();
    let worker_endpoints: BTreeMap<_, _> = args
        .workers
        .iter()
        .map(|worker| (worker.id.as_str(), worker.endpoint))
        .collect();
    let batched = args
        .lanes
        .checked_mul(8)
        .context("planning batch overflow")?;
    let profiles = [
        ("batched", batched, args.lanes),
        ("decode", args.lanes, args.lanes),
        ("prefill", 8, 1),
    ]
    .map(
        |(id, tokens, sequences)| stage_admission::StagePlannerProfile {
            profile_id: id.into(),
            n_tokens: tokens,
            n_sequences: sequences,
            n_outputs: tokens,
            n_recurrent_rollback_sequences: 0,
        },
    );
    let backend = format!(
        "skippy-backend:requested-gpu-layers:{}:v1",
        args.n_gpu_layers
    );
    let mut graph = Sha256::new();
    graph.update(b"skippy-graph-configuration:v1\0");
    graph.update(args.ctx_size.to_le_bytes());
    graph.update(args.lanes.to_le_bytes());
    graph.update((backend.len() as u64).to_le_bytes());
    graph.update(backend.as_bytes());
    let graph_id = format!(
        "skippy-graph-configuration:v1:{}",
        hex::encode(graph.finalize())
    );
    let admissions = stage_admission::realize_direct_gguf_stage_admissions(
        model_id, identity, &ranges, &profiles, &graph_id, &backend,
    )?;
    ensure!(
        admissions.len() == topology.stages.len(),
        "admission stage count mismatch"
    );
    validate_frontiers(&admissions)?;
    let peer = |index: usize| PeerConfig {
        persistent_lane_marker: false,
        stage_id: topology.stages[index].stage_id.clone(),
        stage_index: index as u32,
        endpoint: worker_endpoints[topology.stages[index].node_id.as_str()].to_string(),
    };
    // The final stage runs integrated NextN heads as its MTP sidecar; a stage
    // without them ignores the flag.
    let native_mtp_enabled =
        skippy_model_artifact::gguf::supports_native_mtp(&identity.source_model_path);
    let mut configs = Vec::<StageConfig>::new();
    for (index, (stage, admission)) in topology.stages.iter().zip(&admissions).enumerate() {
        let names = stage_load::admitted_resident_tensor_names(admission, manifest)?;
        let load = stage_load::AdmittedStageOptions {
            topology_id: args.run_id.clone(),
            run_id: args.run_id.clone(),
            model_id: model_id.to_owned(),
            stage_id: stage.stage_id.clone(),
            layer_start: stage.layer_start,
            layer_end: stage.layer_end,
            ctx_size: args.ctx_size,
            lane_count: args.lanes,
            selected_device: None,
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
            stage_index: index as u32,
            n_batch: None,
            n_ubatch: None,
            n_gpu_layers: args.n_gpu_layers,
            mmap: None,
            mlock: false,
            runtime_settings: Default::default(),
            cache_type_k: "f16".into(),
            cache_type_v: "f16".into(),
            flash_attn_type: Default::default(),
            load_mode: skippy_protocol::LoadMode::RuntimeSlice,
            native_mtp_enabled,
            bind_addr: worker_endpoints[topology.stages[index].node_id.as_str()].to_string(),
            upstream: index.checked_sub(1).map(peer),
            downstream: (index + 1 < topology.stages.len()).then(|| peer(index + 1)),
            admission: admission.clone(),
        };
        configs.push(stage_load::admitted_stage_config(&load, None, names)?);
    }
    Ok((configs, admissions))
}

fn validate_frontiers(admissions: &[skippy_protocol::StageAdmissionDescriptor]) -> Result<()> {
    for pair in admissions.windows(2) {
        let upstream = stage_load::admitted_activation_frontier(&pair[0])?;
        let downstream = stage_load::admitted_activation_frontier(&pair[1])?;
        ensure!(
            upstream.activation_exports == downstream.activation_imports
                && upstream.activation_export_bindings == downstream.activation_import_bindings,
            "adjacent stage activation frontiers do not match",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_publication_rejects_a_neighbor_binding_mismatch() {
        use skippy_protocol::{StageAdmissionDescriptor, StageAdmissionProfile};
        let profile = StageAdmissionProfile {
            profile_id: "decode".into(),
            graph_identity: "graph".into(),
            profile_identity: "profile".into(),
            slice_identity: "slice".into(),
            source_snapshot_identity: "source".into(),
            graph_configuration_id: "config".into(),
            backend_id: "backend".into(),
            activation_imports: vec!["activation".into()],
            activation_exports: vec!["activation".into()],
            activation_import_bindings: vec!["binding".into()],
            activation_export_bindings: vec!["binding".into()],
        };
        let first = StageAdmissionDescriptor {
            kv_graph_state: String::new(),
            version: skippy_protocol::STAGE_ADMISSION_DESCRIPTOR_VERSION,
            package_id: "package".into(),
            plan_id: "plan".into(),
            execution_contract: String::new(),
            layer_start: 0,
            layer_end: 1,
            resident_tensor_ids: vec![],
            sidecars: vec![],
            profiles: vec![profile],
        };
        let mut second = first.clone();
        second.layer_start = 1;
        second.layer_end = 2;
        validate_frontiers(&[first.clone(), second.clone()]).unwrap();
        second.profiles[0].activation_import_bindings[0] = "different-source-binding".into();
        assert!(
            validate_frontiers(&[first, second])
                .unwrap_err()
                .to_string()
                .contains("frontiers")
        );
    }
}
