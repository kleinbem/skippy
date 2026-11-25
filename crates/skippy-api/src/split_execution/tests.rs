use super::*;

fn workers() -> Vec<SplitWorker> {
    (0..2)
        .map(|index| SplitWorker {
            id: format!("worker-{index}"),
            endpoint: SocketAddr::from(([127, 0, 0, 1], 9200 + index)),
        })
        .collect()
}

fn fixture(workers: Vec<SplitWorker>) -> AdmittedSplit {
    let identity = SkippyPackageIdentity {
        package_ref: "test".into(),
        manifest_sha256: "manifest-1".into(),
        source_model_path: PathBuf::from("fake.gguf"),
        source_model_sha256: "source-1".into(),
        source_model_bytes: 64,
        source_files: vec![],
        layer_weight_bytes: vec![16; 4],
        layer_count: 4,
        activation_width: 256,
        tensor_count: 4,
        generation: None,
        publisher_defaults: None,
    };
    let topology = skippy_topology::plan_balanced_accepted_contiguous(
        &skippy_topology::TopologyPlanRequest {
            topology_id: "test".into(),
            model_id: "model".into(),
            layers: skippy_topology::dense_attention_layers(4, 16),
            nodes: workers
                .iter()
                .map(|worker| skippy_topology::NodeSpec {
                    node_id: worker.id.clone(),
                    cached_slice_bytes: 0,
                    vram_bytes: 0,
                })
                .collect(),
            family: None,
            policy: Default::default(),
        },
        2,
    )
    .unwrap();
    let configs = topology
        .stages
        .iter()
        .zip(workers)
        .map(|(stage, worker)| StageConfig {
            bind_addr: worker.endpoint.to_string(),
            stage_id: stage.stage_id.clone(),
            stage_index: stage.stage_index,
            layer_start: stage.layer_start,
            layer_end: stage.layer_end,
            ..Default::default()
        })
        .collect();
    AdmittedSplit {
        identity,
        topology,
        configs,
        admissions: vec![],
    }
}

#[derive(Default)]
struct FakeOperations {
    events: Vec<(bool, u32)>,
    fail: bool,
}

impl StageOperations for FakeOperations {
    type Handle = u32;
    fn start(&mut self, _: &SplitWorker, config: &StageConfig) -> Result<u32> {
        ensure!(
            !self.fail || config.stage_index != 0,
            "stage startup failed"
        );
        self.events.push((true, config.stage_index));
        Ok(config.stage_index)
    }
    fn observe(&mut self, _: &u32) -> Result<StageState> {
        Ok(StageState::Ready)
    }
    fn stop(&mut self, handle: &mut u32) -> Result<()> {
        self.events.push((false, *handle));
        Ok(())
    }
}

#[test]
fn two_workers_start_downstream_first_and_cancel_upstream_first() {
    let workers = workers();
    let plan = fixture(workers.clone());
    let mut operations = FakeOperations::default();
    let mut deployment = plan.start("manifest-1", &workers, &mut operations).unwrap();
    assert_eq!(
        deployment.observe().unwrap(),
        vec![(0, StageState::Ready), (1, StageState::Ready)]
    );
    deployment.cancel().unwrap();
    assert_eq!(
        operations.events,
        vec![(true, 1), (true, 0), (false, 0), (false, 1)]
    );
}

#[test]
fn partial_start_rolls_back_and_stale_inputs_never_start() {
    let workers = workers();
    let plan = fixture(workers.clone());
    let mut operations = FakeOperations {
        fail: true,
        ..Default::default()
    };
    assert!(plan.start("stale", &workers, &mut operations).is_err());
    let mut missing = workers.clone();
    missing[0].id = "replacement".into();
    assert!(plan.start("manifest-1", &missing, &mut operations).is_err());
    assert!(operations.events.is_empty());
    assert!(plan.start("manifest-1", &workers, &mut operations).is_err());
    assert_eq!(operations.events, vec![(true, 1), (false, 1)]);
}
