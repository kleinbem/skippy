use std::{ffi::OsString, path::PathBuf, time::Instant};

use anyhow::{Context, Result, ensure};
use clap::Args;
use skippy_api::{
    native_runtime::{NativeRuntimeOptions, local_native_runtime_plan},
    tuning::{self, Cancellation, Measurement, ProcessTrialRunner, Sweep, TrialInput},
};
use skippy_inference_api::{
    InferenceRequestContext, MessageContent,
    generation::{GenerationMessage, GenerationRequest},
};

mod config;

#[derive(Args)]
pub struct TuneArgs {
    /// JSON sweep specification containing models, candidates, workload and runtime.
    #[arg(long)]
    config: PathBuf,
    /// Apply the winning patch to a standalone TOML file.
    #[arg(long)]
    apply: Option<PathBuf>,
}

pub async fn run(args: TuneArgs) -> Result<()> {
    let sweep: Sweep = serde_json::from_reader(std::fs::File::open(&args.config)?)
        .with_context(|| format!("read sweep {}", args.config.display()))?;
    ensure!(
        args.apply.is_none() || sweep.targets.len() == 1,
        "--apply requires one model target"
    );
    let runner = ProcessTrialRunner {
        executable: std::env::current_exe()?,
        environment: [
            "HOME",
            "USERPROFILE",
            "PATH",
            "SystemRoot",
            "WINDIR",
            "TMPDIR",
            "TEMP",
            "CUDA_VISIBLE_DEVICES",
            "CUDA_DEVICE_ORDER",
            "HIP_VISIBLE_DEVICES",
            "ROCR_VISIBLE_DEVICES",
            "GPU_DEVICE_ORDINAL",
            "LD_LIBRARY_PATH",
            "DYLD_LIBRARY_PATH",
        ]
        .into_iter()
        .filter_map(|key| Some((key.into(), std::env::var_os(key)?)))
        .collect(),
    };
    let cancellation = Cancellation::default();
    let worker_cancellation = cancellation.clone();
    let mut worker =
        tokio::task::spawn_blocking(move || tuning::run(&sweep, &runner, &worker_cancellation));
    let report = tokio::select! {
        result = &mut worker => result??,
        signal = tokio::signal::ctrl_c() => {
            cancellation.cancel();
            let result = worker.await?;
            signal?;
            result?
        }
    };
    skippy_commands::console::write_json(&report)?;
    if let Some(path) = args.apply {
        ensure!(!report.cancelled, "cancelled sweep cannot apply settings");
        let patch = &report.models[0]
            .recommendation
            .as_ref()
            .context("no successful setting to apply")?
            .patch;
        config::apply(&path, patch)?;
    }
    Ok(())
}

pub async fn run_trial(input: PathBuf) -> Result<()> {
    let trial: TrialInput = serde_json::from_reader(std::fs::File::open(input)?)?;
    load_runtime(&trial)?;
    let mut argv = vec![
        OsString::from("skippy"),
        OsString::from("serve"),
        OsString::from("--model-path"),
        trial.target.path.as_os_str().to_owned(),
        format!("--model-id={}", trial.target.id).into(),
        "--prefix-cache=off".into(),
    ];
    argv.extend(
        config::options(&trial.settings)
            .into_iter()
            .map(|(_, name, value)| {
                let encoded = value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned);
                format!("--{}={encoded}", name.replace('_', "-")).into()
            }),
    );
    if let Some(device) = &trial.hardware.backend_device {
        argv.push(format!("--device={device}").into());
    }
    let parsed = crate::serve_settings::parse(argv)?;
    let crate::cli::Command::Serve(args) = parsed.command else {
        unreachable!("constructed serve command")
    };
    let (_, request) = crate::conversion::local_openai_options(args.public)?.into_request()?;
    let loaded = request.load()?;
    let mut result = Measurement {
        prompt_tokens: 0,
        completion_tokens: 0,
        elapsed_nanos: 0,
        requests: 0,
    };
    for prompt in trial
        .workload
        .prompts
        .iter()
        .cycle()
        .take(trial.workload.warmup_runs as usize)
    {
        let _ = loaded
            .backend
            .chat_completion_with_context(
                generation_request(&trial, prompt),
                InferenceRequestContext::new(),
            )
            .await?;
    }
    for _ in 0..trial.workload.repetitions {
        for prompt in &trial.workload.prompts {
            let request = generation_request(&trial, prompt);
            let start = Instant::now();
            let response = loaded
                .backend
                .chat_completion_with_context(request, InferenceRequestContext::new())
                .await?;
            result.elapsed_nanos = result
                .elapsed_nanos
                .saturating_add(u64::try_from(start.elapsed().as_nanos())?);
            result.prompt_tokens = result
                .prompt_tokens
                .saturating_add(u64::from(response.usage.prompt_tokens));
            result.completion_tokens = result
                .completion_tokens
                .saturating_add(u64::from(response.usage.completion_tokens));
            result.requests += 1;
        }
    }
    drop(loaded);
    println!(
        "{}{}",
        tuning::RESULT_PREFIX,
        serde_json::to_string(&result)?
    );
    Ok(())
}

fn load_runtime(trial: &TrialInput) -> Result<()> {
    let native = NativeRuntimeOptions {
        bundle_dirs: trial.runtime.bundle_dirs.clone(),
        cache_dir: Some(trial.runtime.cache_dir.clone()),
        release: trial.runtime.release.clone(),
        selection: trial.runtime.selection.clone(),
    };
    let plan = local_native_runtime_plan(&native)?;
    ensure!(
        plan.native_runtime_id == trial.runtime.identity,
        "trial selected runtime {}, expected {}",
        plan.native_runtime_id,
        trial.runtime.identity
    );
    #[cfg(feature = "dynamic-native-runtime")]
    skippy_api::native_runtime::load_local_native_runtime(&native)?;
    #[cfg(not(feature = "dynamic-native-runtime"))]
    anyhow::bail!("isolated native trial requires a dynamic-runtime CLI build");
    #[cfg(feature = "dynamic-native-runtime")]
    Ok(())
}

fn generation_request(trial: &TrialInput, prompt: &str) -> GenerationRequest {
    GenerationRequest {
        model: trial.target.id.clone(),
        messages: vec![GenerationMessage {
            role: "user".into(),
            content: Some(MessageContent::Text(prompt.to_owned())),
            extra: Default::default(),
        }],
        temperature: Some(0.0),
        seed: Some(trial.workload.seed),
        max_completion_tokens: Some(trial.workload.max_tokens),
        ..Default::default()
    }
}
