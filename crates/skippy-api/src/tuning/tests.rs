use super::*;

fn sweep() -> Sweep {
    let path = std::env::temp_dir();
    let mut candidates = vec![];
    for batch in [64, 128, 256, 512] {
        candidates.push(SettingsPatch {
            context: 1024,
            batch,
            microbatch: 64,
            mmap: true,
            mlock: false,
            flash_attention: FlashAttention::Auto,
            speculative: Speculation::Disabled,
        });
    }
    Sweep {
        targets: vec![ModelTarget {
            id: "model".into(),
            path: path.join("fake.gguf"),
        }],
        runtime: Runtime {
            bundle_dirs: vec![],
            cache_dir: path,
            release: None,
            selection: None,
            identity: "fixture".into(),
        },
        hardware: Hardware {
            identity: "test GPU".into(),
            backend_device: None,
        },
        workload: Workload {
            prompts: vec!["Prompt".into()],
            warmup_runs: 1,
            repetitions: 2,
            max_tokens: 4,
            seed: 0,
        },
        candidates,
        timeout_secs: 1,
    }
}

struct FakeRunner;
impl TrialRunner for FakeRunner {
    fn run(&self, trial: Trial<'_>, _: &Cancellation) -> Result<Measurement, TrialError> {
        let (tokens, nanos) = match trial.settings.batch {
            64 => (4, 20),
            128 => (4, 10),
            256 => (8, 20),
            _ => return Err(TrialError::Timeout),
        };
        Ok(Measurement {
            prompt_tokens: 6,
            completion_tokens: tokens,
            elapsed_nanos: nanos,
            requests: 2,
        })
    }
}

#[test]
fn emits_failures_and_recommends_the_first_of_equal_throughput_candidates() {
    let report = run(&sweep(), &FakeRunner, &Cancellation::default()).unwrap();
    assert_eq!(report.models[0].trials.len(), 4);
    assert!(matches!(
        report.models[0].trials[3].result,
        Err(TrialError::Timeout)
    ));
    assert_eq!(
        report.models[0]
            .recommendation
            .as_ref()
            .unwrap()
            .patch
            .batch,
        128
    );
}

#[cfg(unix)]
#[test]
fn isolated_process_timeout_releases_its_scratch_directory() {
    use std::os::unix::fs::PermissionsExt;
    let files = tempfile::tempdir().unwrap();
    let output = files.path().join("working-directory");
    let helper = files.path().join("slow-trial");
    std::fs::write(
        &helper,
        "#!/bin/sh\n/bin/pwd > \"$TEST_RECORD\"\n/bin/sleep 10\n",
    )
    .unwrap();
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut sweep = sweep();
    sweep.candidates.truncate(1);
    let runner = ProcessTrialRunner {
        executable: helper,
        environment: [("TEST_RECORD".into(), output.clone().into_os_string())].into(),
    };
    let report = run(&sweep, &runner, &Cancellation::default()).unwrap();
    assert!(matches!(
        report.models[0].trials[0].result,
        Err(TrialError::Timeout)
    ));
    let cwd = std::fs::read_to_string(output).unwrap();
    assert!(!std::path::Path::new(cwd.trim()).exists());
}
