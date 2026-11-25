use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result, ensure};

/// Parsed `plan-split` inputs, decoupled from clap.
#[derive(Debug, Clone)]
pub struct PlanSplitCommand {
    pub model_path: PathBuf,
    pub model_id: Option<String>,
    /// Ordered worker listen endpoints, one per stage. Use routable addresses across machines.
    pub workers: Vec<SocketAddr>,
    pub ctx_size: u32,
    pub lanes: u32,
    pub n_gpu_layers: i32,
    /// New directory for stage configs and their admission descriptors; never overwritten.
    pub output_dir: PathBuf,
}

fn validate(args: &PlanSplitCommand) -> Result<()> {
    ensure!(
        args.workers.len() >= 2,
        "a split needs at least two --worker endpoints"
    );
    ensure!(
        args.ctx_size > 0 && args.lanes > 0,
        "context size and lanes must be positive"
    );
    let unique = args
        .workers
        .iter()
        .collect::<std::collections::BTreeSet<_>>();
    ensure!(
        unique.len() == args.workers.len(),
        "worker endpoints must be distinct"
    );
    ensure!(
        args.workers
            .iter()
            .all(|a| a.port() != 0 && !a.ip().is_unspecified()),
        "workers require a nonzero port and a routable or loopback address"
    );
    ensure!(
        !args.output_dir.try_exists()?,
        "output directory already exists"
    );
    Ok(())
}

pub fn run(args: PlanSplitCommand) -> Result<()> {
    validate(&args)?;
    let run_id = format!("skippy-{}", uuid::Uuid::new_v4());
    let plan = skippy_api::split_execution::plan_direct_gguf(
        &skippy_api::split_execution::SplitRequest {
            model_path: args.model_path,
            model_id: args.model_id,
            expected_manifest_sha256: None,
            run_id: run_id.clone(),
            workers: args
                .workers
                .iter()
                .enumerate()
                .map(
                    |(index, &endpoint)| skippy_api::split_execution::SplitWorker {
                        id: format!("worker-{index}"),
                        endpoint,
                    },
                )
                .collect(),
            placement: skippy_api::split_execution::SplitPlacement::Balanced,
            ctx_size: args.ctx_size,
            lanes: args.lanes,
            n_gpu_layers: args.n_gpu_layers,
        },
    )?;
    let topology = plan.topology();
    let model_id = &topology.model_id;
    let identity = plan.identity();
    let admissions = plan.admissions();
    let configs = plan.configs();
    // All stages must be admitted before any output is published. Exclusive directory
    // creation prevents accidental replacement of a previous deployment plan.
    std::fs::create_dir(&args.output_dir).context("create new split plan directory")?;
    for (index, config) in configs.iter().enumerate() {
        std::fs::write(
            args.output_dir.join(format!("stage-{index}.json")),
            serde_json::to_vec_pretty(config)?,
        )?;
    }
    std::fs::write(
        args.output_dir.join("admissions.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2,
            "stages": admissions.iter().map(admission_json).collect::<Vec<_>>()
        }))?,
    )?;
    crate::console::write_json(
        &serde_json::json!({"schema_version":2,"run_id":run_id,"model_id":model_id,"output_dir":args.output_dir,"topology":topology,"source_sha256":identity.source_model_sha256}),
    )
}

// Human-readable evidence only; worker transport continues to use the canonical
// protobuf admission representation owned by skippy-protocol.
fn admission_json(a: &skippy_protocol::StageAdmissionDescriptor) -> serde_json::Value {
    serde_json::json!({
        "version": a.version, "package_id": a.package_id, "plan_id": a.plan_id,
        "layer_start": a.layer_start, "layer_end": a.layer_end,
        "resident_tensor_ids": a.resident_tensor_ids,
        "sidecars": a.sidecars.iter().map(|s| serde_json::json!({
            "kind": match s.kind { skippy_protocol::StageAdmissionSidecarKind::Mmproj => "mmproj" },
            "artifact_id":s.artifact_id, "name":s.name,
        })).collect::<Vec<_>>(),
        "profiles": a.profiles.iter().map(|p| serde_json::json!({
            "profile_id":p.profile_id,"graph_identity":p.graph_identity,"profile_identity":p.profile_identity,
            "slice_identity":p.slice_identity,"source_snapshot_identity":p.source_snapshot_identity,
            "graph_configuration_id":p.graph_configuration_id,"backend_id":p.backend_id,
            "activation_imports":p.activation_imports,"activation_exports":p.activation_exports,
            "activation_import_bindings":p.activation_import_bindings,"activation_export_bindings":p.activation_export_bindings,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checkpoint_directory_fails_before_native_planning_or_publication() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("model.safetensors"), b"weights").unwrap();
        let args = PlanSplitCommand {
            model_path: root.path().to_owned(),
            model_id: None,
            workers: vec![
                "127.0.0.1:9100".parse().unwrap(),
                "127.0.0.1:9101".parse().unwrap(),
            ],
            ctx_size: 512,
            lanes: 1,
            n_gpu_layers: 0,
            output_dir: root.path().join("plan"),
        };
        assert!(
            run(args)
                .unwrap_err()
                .to_string()
                .contains("Convert the SafeTensors checkpoint to GGUF")
        );
        assert!(!root.path().join("plan").exists());
    }

    #[test]
    fn invalid_workers_fail_before_reading_model_or_writing_output() {
        let root = tempfile::tempdir().unwrap();
        let args = PlanSplitCommand {
            model_path: root.path().join("missing.gguf"),
            model_id: None,
            workers: vec!["127.0.0.1:9100".parse().unwrap(); 2],
            ctx_size: 512,
            lanes: 1,
            n_gpu_layers: 0,
            output_dir: root.path().join("plan"),
        };
        assert!(run(args).unwrap_err().to_string().contains("distinct"));
        assert!(!root.path().join("plan").exists());
    }
}
