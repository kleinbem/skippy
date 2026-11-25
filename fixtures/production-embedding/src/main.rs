use std::{error::Error, net::SocketAddr, path::PathBuf, sync::mpsc, time::Duration};

use skippy_api::execution::{
    ExecutionCapabilities, ExecutionErrorKind, ExecutionState, GenerationMessage,
    GenerationRequest, GenerationStream, MessageContent, ModelOptions, ReasoningEffort,
    RuntimeBundle, SplitOptions, StageCompatibility,
};

use skippy_api::serving::{ModelLoadOutcome, ModelOpenEvents, ModelOpenObservation};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct Inputs {
    bundle: PathBuf,
    digest: String,
    model: PathBuf,
    return_endpoint: SocketAddr,
    worker_endpoint: SocketAddr,
    device: Option<String>,
}

impl Inputs {
    fn parse() -> Result<Self> {
        let mut args = std::env::args_os().skip(1);
        let bundle = PathBuf::from(args.next().ok_or("supply a verified runtime bundle")?);
        let digest = args
            .next()
            .ok_or("supply the runtime manifest digest")?
            .into_string()
            .map_err(|_| "digest must be UTF-8")?;
        let model = PathBuf::from(args.next().ok_or("supply a local GGUF")?);
        let endpoint = |value: Option<std::ffi::OsString>| -> Result<SocketAddr> {
            Ok(value
                .ok_or("supply an explicit endpoint")?
                .into_string()
                .map_err(|_| "endpoint must be UTF-8")?
                .parse()?)
        };
        let return_endpoint = endpoint(args.next())?;
        let worker_endpoint = endpoint(args.next())?;
        let device = args
            .next()
            .ok_or("supply the native device or '-' for CPU")?
            .into_string()
            .map_err(|_| "device must be UTF-8")?;
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
        Ok(Self {
            bundle,
            digest,
            model,
            return_endpoint,
            worker_endpoint,
            device: (device != "-").then_some(device),
        })
    }

    fn runtime(&self) -> Result<RuntimeBundle> {
        Ok(RuntimeBundle::new(&self.bundle, &self.digest)?)
    }

    fn split(&self, peer: StageCompatibility) -> Result<SplitOptions> {
        let options = SplitOptions::new(
            "qualification",
            &self.model,
            self.runtime()?,
            self.return_endpoint,
            self.worker_endpoint,
            peer,
        )?
        .with_limits(512, 1, 32)
        .with_startup_timeout(Duration::from_secs(90));
        Ok(match &self.device {
            Some(device) => options.with_backend_device(device, -1),
            None => options.with_backend_device("CPU", 0),
        })
    }
}

fn request() -> GenerationRequest {
    GenerationRequest {
        model: "qualification".to_owned(),
        messages: vec![GenerationMessage {
            role: "user".to_owned(),
            content: Some(MessageContent::Text(
                "Count slowly from one to ten.".to_owned(),
            )),
            extra: Default::default(),
        }],
        max_tokens: Some(32),
        temperature: Some(0.0),
        reasoning_effort: Some(ReasoningEffort::None),
        ..Default::default()
    }
}

async fn await_text(stream: &mut GenerationStream) -> Result<()> {
    while let Some(event) = tokio::time::timeout(Duration::from_secs(60), stream.next()).await? {
        if event?.choices.iter().any(|choice| {
            choice
                .delta
                .content
                .as_ref()
                .is_some_and(|content| !content.is_empty())
        }) {
            return Ok(());
        }
    }
    Err("generation ended without text".into())
}

fn reject_incompatible(inputs: &Inputs) -> Result<()> {
    let wrong_bundle = RuntimeBundle::new(&inputs.bundle, "0".repeat(64))?;
    let result = ModelOptions::new("qualification", &inputs.model, wrong_bundle)?.load();
    assert_eq!(
        result.err().expect("wrong runtime digest accepted").kind(),
        ExecutionErrorKind::IncompatibleRuntime
    );
    let current = StageCompatibility::current();
    for incompatible in [
        StageCompatibility::new(
            current.generation() - 1,
            current.llama_upstream(),
            current.patch_queue_sha256(),
            current.native_abi(),
        ),
        StageCompatibility::new(
            current.generation(),
            current.llama_upstream(),
            "0".repeat(64),
            current.native_abi(),
        ),
    ] {
        let result = SplitOptions::new(
            "qualification",
            &inputs.model,
            inputs.runtime()?,
            inputs.return_endpoint,
            inputs.worker_endpoint,
            incompatible,
        );
        assert_eq!(
            result.err().expect("incompatible stage accepted").kind(),
            ExecutionErrorKind::InvalidAdmission
        );
    }
    Ok(())
}

async fn local(inputs: &Inputs) -> Result<()> {
    let options = ModelOptions::new("qualification", &inputs.model, inputs.runtime()?)?
        .with_limits(512, 1, 32);
    let (summaries, reported) = mpsc::channel();
    let events = ModelOpenEvents::observe(move |observation| {
        if let ModelOpenObservation::Summary(summary) = observation {
            let _ = summaries.send(summary);
        }
    });
    let model = match &inputs.device {
        Some(device) => options.with_backend_device(device, -1),
        None => options.with_backend_device("CPU", 0),
    }
    .load_with_model_open_events(events)?;
    let summary = reported.try_recv()?;
    assert_eq!(summary.outcome, ModelLoadOutcome::Succeeded);
    let ready = model.inspect();
    assert_eq!(ready.state(), ExecutionState::Ready);
    assert_eq!(ready.settings().lanes(), 1);
    assert_eq!(ready.runtime().manifest_sha256(), inputs.digest);
    assert_eq!(
        ready
            .selected_device()
            .map(|device| device.backend_device.as_str()),
        Some(inputs.device.as_deref().unwrap_or("CPU"))
    );
    let caps = model.capabilities();
    caps.require(ExecutionCapabilities::STREAMED_GENERATION)?;
    assert_eq!(
        caps.require("unknown-required-strategy")
            .unwrap_err()
            .kind(),
        ExecutionErrorKind::UnsupportedCapability
    );
    let mut stream = model.generate(request()).await?;
    await_text(&mut stream).await?;
    stream.cancel();
    assert_eq!(
        stream
            .next()
            .await
            .ok_or("missing cancellation event")?
            .unwrap_err()
            .kind(),
        ExecutionErrorKind::Cancelled
    );
    tokio::time::timeout(Duration::from_secs(60), model.stop()).await?;
    model.stop().await;
    assert_eq!(model.inspect().state(), ExecutionState::Stopped);
    assert_eq!(
        model
            .generate(request())
            .await
            .err()
            .ok_or("stopped model accepted work")?
            .kind(),
        ExecutionErrorKind::NotReady
    );
    let observed = model.inspect();
    let compute = observed
        .compute()
        .ok_or("missing local compute observation")?;
    println!(
        "local runtime {} ({}) reported {} compute operations",
        observed.runtime().artifact(),
        observed.runtime().backend(),
        compute.operations()
    );
    Ok(())
}

async fn split(inputs: &Inputs) -> Result<()> {
    let mut split = inputs.split(StageCompatibility::current())?.load().await?;
    split
        .capabilities()
        .require(ExecutionCapabilities::ADMITTED_STAGE)?;
    let front = split.inspect_frontend();
    let worker = split.inspect_worker();
    assert_eq!(front.state(), ExecutionState::Ready);
    assert_eq!(worker.state(), ExecutionState::Ready);
    assert_eq!(
        worker
            .selected_device()
            .map(|device| device.backend_device.as_str()),
        Some(inputs.device.as_deref().unwrap_or("CPU"))
    );
    assert_eq!(front.layers().end, worker.layers().start);
    assert!(front.output_boundary().is_some() && worker.input_boundary().is_some());
    let initial = worker
        .compute()
        .ok_or("missing worker compute observation")?;
    let mut stream = split.generate(request()).await?;
    await_text(&mut stream).await?;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(60), split.stop()).await??;
    split.stop().await?;
    let worker = split.inspect_worker();
    assert_eq!(split.inspect_frontend().state(), ExecutionState::Stopped);
    assert_eq!(worker.state(), ExecutionState::Stopped);
    let compute = worker
        .compute()
        .ok_or("missing final worker compute observation")?;
    assert!(compute.operations() > initial.operations());
    for endpoint in [inputs.return_endpoint, inputs.worker_endpoint] {
        std::net::TcpListener::bind(endpoint)?;
    }
    println!(
        "admitted worker reported {} compute operations and {} ns busy",
        compute.operations(),
        compute.busy_nanos()
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let inputs = Inputs::parse()?;
    reject_incompatible(&inputs)?;
    local(&inputs).await?;
    split(&inputs).await
}
