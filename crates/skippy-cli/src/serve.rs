//! One public serving entry point with binary internal stage transport.

use std::{
    future::Future,
    io::IsTerminal,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use skippy_api::serving::{
    ModelOpenEvent, ModelOpenEventKind, ModelOpenEvents, ModelOpenObservation,
    ModelOpenProgressUnit,
};
use skippy_commands::console::{self, OutputMode};
use tokio::{process::Command, sync::oneshot, task::JoinHandle};

use crate::{
    cli::{ServeBinaryArgs, ServeCommandArgs, ServeOpenAiArgs, StageTransport},
    conversion, shutdown_signal,
};

pub(crate) fn default_public_bind_addr() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 9337))
}

pub async fn run(mut args: ServeCommandArgs) -> Result<()> {
    validate(&args)?;
    if !args.local_models.is_empty() {
        return crate::multi_model::run(args).await;
    }
    crate::model_source::resolve(&mut args.public).await?;
    match args.stage_transport {
        Some(StageTransport::Binary) => serve_binary_stage(args).await,
        None => serve_public(args).await,
    }
}

pub(crate) fn validate(args: &ServeCommandArgs) -> Result<()> {
    args.public.settings.validate_mode(args)?;
    crate::residency::validate(args)?;
    if args.local_models.is_empty() {
        validate_public_settings(&args.public)?;
    } else {
        for model in &args.local_models {
            validate_public_settings(model).with_context(|| {
                format!("settings for {}", crate::multi_model::describe_model(model))
            })?;
        }
    }
    if args.stage_transport.is_some() && args.public.config.is_none() {
        bail!("--stage-transport requires --config, supplied directly or through serving settings");
    }
    if args.worker_only && args.prompt {
        bail!("--prompt requires a public inference API");
    }
    if args.worker_only && args.stage_transport.is_none() {
        bail!("--worker-only requires --stage-transport");
    }
    if args.prompt && (!std::io::stdin().is_terminal() || console::mode() != OutputMode::Human) {
        bail!("--prompt requires an interactive terminal and human output");
    }
    if console::mode() == OutputMode::Json && !args.print_effective_config {
        bail!("serving produces a stream of events; use --output jsonl or --output human");
    }
    if args.local_models.is_empty()
        && args.public.config.is_none()
        && args.public.model_path.is_none()
        && args.public.model.is_none()
    {
        bail!("provide --model, --model-path, --config, or a model in --settings");
    }
    Ok(())
}

fn validate_public_settings(args: &ServeOpenAiArgs) -> Result<()> {
    if let Some(model) = args.model.as_deref() {
        skippy_commands::models::reject_safetensors_model_ref(model)?;
    }
    args.settings.tuning(args.openai_guardrails)?;
    let mut stage = skippy_protocol::StageConfig::default();
    args.settings.apply_stage(&mut stage)
}

async fn serve_public(args: ServeCommandArgs) -> Result<()> {
    let bind_addr = args
        .public
        .bind_addr
        .unwrap_or_else(default_public_bind_addr);
    let startup_timeout = Duration::from_secs(args.public.startup_timeout_secs.max(1));
    console::status("🧠 Preparing model")?;
    let residency = crate::residency::options(&args.public);
    let settings = args.public.settings.clone();
    let discover_draft = args.public.speculative_config.is_none();
    let mut options = conversion::local_openai_options(args.public)?;
    let draft = if discover_draft {
        crate::draft::prepare_default_draft(&mut options, &settings, args.print_effective_config)
            .await
    } else {
        None
    };
    if args.print_effective_config {
        let mut frontend = options.resolved_openai_options()?;
        if let Some(draft) = &draft {
            draft.apply_to(&mut frontend);
            options.config.native_mtp_enabled = false;
        }
        return console::write_json(&settings.report(
            &options.config,
            Some(&frontend),
            &options.tuning,
        ));
    }
    if let Some(residency) = residency {
        return crate::residency::serve(options, residency, draft).await;
    }
    options.model_open_events = model_open_events();
    let model_id = options
        .model_id
        .clone()
        .unwrap_or_else(|| options.config.model_id.clone());
    let (stop, stopped) = oneshot::channel();
    let shutdown = shutdown_signal()?;
    let (shutdown, shutdown_requested) = track_shutdown_request(async move {
        tokio::select! { _ = shutdown => {}, _ = stopped => {} }
    });
    let server = async move {
        skippy_api::serving::serve_local_openai_with_default_draft(
            options,
            draft.as_ref(),
            shutdown,
        )
        .await
    };
    serve_with_readiness(
        ReadinessServer {
            future: server,
            shutdown_requested,
        },
        bind_addr,
        model_id,
        args.prompt,
        stop,
        startup_timeout,
    )
    .await
}

async fn serve_binary_stage(mut args: ServeCommandArgs) -> Result<()> {
    let disk_cache = crate::disk_cache::from_public_settings(
        args.public.kv_cache_disk.as_deref(),
        args.public.kv_cache_disk_dir.clone(),
        args.public.kv_cache_min_free.as_deref(),
    )?;
    apply_public_frontend_tuning(&args.public, &mut args.stage);
    args.stage.settings = args.public.settings.clone();
    let startup_timeout = Duration::from_secs(args.public.startup_timeout_secs.max(1));
    args.stage.config = args.public.config.clone().context("--config is required")?;
    args.stage.topology = args.public.topology.clone();
    args.stage.metrics_otlp_grpc = args.public.metrics_otlp_grpc.clone();
    args.stage.telemetry_queue_capacity = args.public.telemetry_queue_capacity;
    args.stage.telemetry_level = args.public.telemetry_level;
    args.stage.worker_only = args.worker_only;
    if args.worker_only {
        args.stage.bind_addr = args.public.bind_addr;
    }
    args.stage.api_bind_addr = Some(
        args.public
            .bind_addr
            .unwrap_or_else(default_public_bind_addr),
    );
    let mut options = conversion::binary_stage_options(args.stage)?;
    args.public
        .settings
        .validate_cache_dependencies(&options.config)?;
    if args.print_effective_config {
        let frontend = options.openai.as_ref().map(|stage| {
            let mut frontend = skippy_api::serving::InferenceOptions::embedded_stage_defaults(
                stage.model_id.clone(),
                stage.default_max_tokens,
                stage.generation_concurrency,
                0,
                options.native_mtp_enabled,
            );
            frontend.request_defaults = options.tuning.request_defaults.clone();
            frontend.continuous_batching = options.continuous_batching;
            frontend.pipeline_decode_groups = stage.pipeline_decode_groups;
            frontend.adaptive_generation_min_concurrency =
                stage.adaptive_generation_min_concurrency;
            frontend.generation_queue_capacity = stage.generation_queue_capacity;
            frontend.generation_admission_timeout_secs = stage.generation_admission_timeout_secs;
            frontend.prefill_chunk_size = stage.prefill_chunk_size;
            frontend.prefill_chunk_policy = stage.prefill_chunk_policy.clone();
            frontend.prefill_chunk_schedule = stage.prefill_chunk_schedule.clone();
            frontend.prefill_adaptive_start = stage.prefill_adaptive_start;
            frontend.prefill_adaptive_step = stage.prefill_adaptive_step;
            frontend.prefill_adaptive_max = stage.prefill_adaptive_max;
            frontend.prefill_adaptive_target_ms = stage.prefill_adaptive_target_ms;
            frontend.draft_model_path = stage.draft_model_path.clone();
            frontend.speculative_window = stage.speculative_window;
            frontend.adaptive_speculative_window = stage.adaptive_speculative_window;
            frontend.draft_n_gpu_layers = stage.draft_n_gpu_layers;
            frontend.speculative = stage.speculative.clone();
            frontend.native_mtp_draft_model_path = stage.native_mtp_draft_model_path.clone();
            frontend.native_mtp_max_tokens = stage.native_mtp_max_tokens;
            frontend.native_mtp_min_tokens = stage.native_mtp_min_tokens;
            frontend.reply_credit_limit = options.reply_credit_limit;
            frontend.downstream_connect_timeout_secs = options.downstream_connect_timeout_secs;
            frontend
        });
        return console::write_json(&args.public.settings.report(
            &options.config,
            frontend.as_ref(),
            &options.tuning,
        ));
    }
    options.l3_manager = disk_cache.and_then(skippy_api::serving::LocalDiskCacheOptions::acquire);
    let Some(openai) = options.openai.as_ref() else {
        if args.prompt {
            bail!("--prompt requires stage 0 to expose the public inference API");
        }
        if !args.worker_only {
            bail!("this stage has no public API; add --worker-only or serve stage 0");
        }
        let bind_addr = options.bind_addr;
        let model_id = options.config.model_id.clone();
        let (shutdown, shutdown_requested) = track_shutdown_request(shutdown_signal()?);
        let server =
            skippy_serving::binary_transport::serve_binary_stage_with_shutdown(options, shutdown);
        return serve_worker_with_readiness(
            ReadinessServer {
                future: server,
                shutdown_requested,
            },
            bind_addr,
            model_id,
            "binary",
            startup_timeout,
        )
        .await;
    };
    let bind_addr = openai.bind_addr;
    let model_id = openai
        .model_id
        .clone()
        .unwrap_or_else(|| options.config.model_id.clone());
    let (stop, stopped) = oneshot::channel();
    let shutdown = shutdown_signal()?;
    let (shutdown, shutdown_requested) = track_shutdown_request(async move {
        tokio::select! { _ = shutdown => {}, _ = stopped => {} }
    });
    let server =
        skippy_serving::binary_transport::serve_binary_stage_with_shutdown(options, shutdown);
    serve_with_readiness(
        ReadinessServer {
            future: server,
            shutdown_requested,
        },
        bind_addr,
        model_id,
        args.prompt,
        stop,
        startup_timeout,
    )
    .await
}

pub(crate) fn apply_public_frontend_tuning(public: &ServeOpenAiArgs, stage: &mut ServeBinaryArgs) {
    stage.openai_model_id = public.model_id.clone();
    stage.openai_default_max_tokens = public.default_max_tokens;
    stage.openai_generation_concurrency = public.generation_concurrency;
    stage.openai_adaptive_generation_concurrency = public.adaptive_generation_concurrency;
    stage.openai_adaptive_generation_min_concurrency = public.adaptive_generation_min_concurrency;
    stage.openai_generation_queue_capacity = public.generation_queue_capacity;
    stage.openai_generation_admission_timeout_secs = public.generation_admission_timeout_secs;
    stage.openai_prefill_chunk_size = public.prefill_chunk_size;
    stage.openai_prefill_chunk_policy = public.prefill_chunk_policy.clone();
    stage.openai_prefill_chunk_schedule = public.prefill_chunk_schedule.clone();
    stage.openai_prefill_adaptive_start = public.prefill_adaptive_start;
    stage.openai_prefill_adaptive_step = public.prefill_adaptive_step;
    stage.openai_prefill_adaptive_max = public.prefill_adaptive_max;
    stage.openai_prefill_adaptive_target_ms = public.prefill_adaptive_target_ms;
    stage.openai_speculative_config = public.speculative_config.clone();
}

// Keep the observation with the server future instead of adding unrelated
// shutdown arguments to every readiness call.
struct ReadinessServer<F> {
    future: F,
    shutdown_requested: Arc<AtomicBool>,
}

#[derive(Debug, PartialEq, Eq)]
enum ReadinessOutcome {
    Ready,
    ShutdownRequested,
}

fn track_shutdown_request(
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> (impl Future<Output = ()> + Send + 'static, Arc<AtomicBool>) {
    let requested = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&requested);
    let shutdown = async move {
        shutdown.await;
        observed.store(true, Ordering::SeqCst);
    };
    (shutdown, requested)
}

async fn serve_worker_with_readiness(
    server: ReadinessServer<impl Future<Output = Result<()>> + Send + 'static>,
    bind_addr: SocketAddr,
    model_id: String,
    transport: &str,
    startup_timeout: Duration,
) -> Result<()> {
    if bind_addr.port() == 0 {
        bail!("stage listener must use a fixed port so readiness can be reported");
    }
    let shutdown_requested = server.shutdown_requested;
    let mut server = tokio::spawn(server.future);
    console::status(&format!("🧠 Loading {transport} stage for {model_id}"))?;
    let deadline = Instant::now() + startup_timeout;
    let probe = readiness_addr(bind_addr);
    loop {
        tokio::select! {
            outcome = &mut server => {
                outcome.context("join serving task")??;
                if shutdown_requested.load(Ordering::SeqCst) {
                    return Ok(());
                }
                bail!("stage exited before its listener became ready");
            }
            connection = tokio::net::TcpStream::connect(probe) => {
                if connection.is_ok() && !server.is_finished() { break; }
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "stage did not become ready at {bind_addr} within {} seconds",
                startup_timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    console::event(
        "ready",
        &serde_json::json!({
            "model_id": model_id, "transport": transport, "bind_addr": bind_addr,
        }),
    )?;
    if console::mode() == OutputMode::Human {
        console::status(&format!("✅ {transport} stage ready at {bind_addr}"))?;
    }
    server.await.context("join serving task")?
}

async fn serve_with_readiness(
    server: ReadinessServer<impl Future<Output = Result<()>> + Send + 'static>,
    bind_addr: SocketAddr,
    model_id: String,
    prompt: bool,
    stop: oneshot::Sender<()>,
    startup_timeout: Duration,
) -> Result<()> {
    if bind_addr.port() == 0 {
        bail!("--bind-addr must use a fixed port so readiness can be reported");
    }
    let api_base = format!("http://{}/v1", readiness_addr(bind_addr));
    let shutdown_requested = server.shutdown_requested;
    console::status("🧠 Loading model and starting the API")?;
    let load_started = std::time::Instant::now();
    let mut server = tokio::spawn(server.future);
    let readiness = wait_for_ready(
        &api_base,
        &model_id,
        &mut server,
        startup_timeout,
        &shutdown_requested,
    )
    .await;
    if !matches!(readiness, Ok(ReadinessOutcome::ShutdownRequested)) {
        record_model_load(&model_id, load_started.elapsed(), readiness.is_ok());
    }
    if readiness? == ReadinessOutcome::ShutdownRequested {
        return Ok(());
    }
    console::event(
        "ready",
        &serde_json::json!({"model_id":model_id,"api_base":api_base}),
    )?;
    if console::mode() == OutputMode::Human {
        console::status(&format!("✅ {model_id} ready at {api_base}"))?;
    }
    if !prompt {
        return server.await.context("join serving task")?;
    }
    // Readline cannot interrupt a blocked terminal read. Keep it in a child
    // process so server shutdown can terminate and join the prompt reliably.
    let mut prompt_process =
        Command::new(std::env::current_exe().context("locate skippy executable")?)
            // Part of this serve session, which is already reported.
            .env(skippy_analytics::ENV_ANALYTICS, "0")
            .args([
                "--output",
                "human",
                "prompt",
                "--endpoint",
                &api_base,
                "--model",
                &model_id,
            ])
            .kill_on_drop(true)
            .spawn()
            .context("start interactive prompt")?;
    tokio::select! {
        result = prompt_process.wait() => {
            let status = result.context("join interactive prompt")?;
            let _ = stop.send(());
            server.await.context("join serving task")??;
            if !status.success() {
                bail!("interactive prompt exited with {status}");
            }
            Ok(())
        }
        result = &mut server => {
            let _ = prompt_process.start_kill();
            prompt_process.wait().await.context("join interactive prompt after server shutdown")?;
            result.context("join serving task")??;
            Ok(())
        }
    }
}

pub(crate) fn model_open_events() -> ModelOpenEvents {
    ModelOpenEvents::observe(|observation| {
        if let ModelOpenObservation::Event(event) = observation {
            let _ = report_model_open_event(event);
        }
    })
}

/// Report `model_load` for the public API's model.
pub(crate) fn record_model_load(model_id: &str, elapsed: Duration, succeeded: bool) {
    let properties = skippy_analytics::models::lookup(model_id)
        .apply(skippy_analytics::Properties::new())
        .with(
            "load_ms",
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        )
        .with("outcome", if succeeded { "loaded" } else { "failed" });
    skippy_analytics::capture(skippy_analytics::Event::ModelLoad, properties);
}

fn readiness_addr(bind_addr: SocketAddr) -> SocketAddr {
    // A wildcard listener accepts loopback; a LAN-only listener does not.
    if bind_addr.ip().is_unspecified() {
        SocketAddr::new(
            if bind_addr.is_ipv4() {
                std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)
            } else {
                std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)
            },
            bind_addr.port(),
        )
    } else {
        bind_addr
    }
}

/// The client that polls this process's own API until the model is served.
/// It only speaks plain HTTP to a local address, so it trusts no roots: building
/// it then doesn't load the system's CA certificates, and serving a local model
/// works where there are none. That address never redirects, so it doesn't
/// follow redirects either.
fn readiness_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .tls_certs_only([])
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(1))
        .build()
}

async fn wait_for_ready(
    api_base: &str,
    model_id: &str,
    server: &mut JoinHandle<Result<()>>,
    startup_timeout: Duration,
    shutdown_requested: &AtomicBool,
) -> Result<ReadinessOutcome> {
    let client = readiness_client()?;
    let deadline = Instant::now() + startup_timeout;
    loop {
        // AN ALREADY-FINISHED TASK IS NOT A RACE TO SAMPLE. `tokio::select!`
        // polls its branches in random order, so when the serving task has
        // already exited its outcome competes with the API probe's connection
        // error and the reason reported depends on which branch select! picked.
        // Both are "ready" immediately, so the coin is genuinely fair: the test
        // below for this distinction failed about 40% of the time.
        //
        // The exit is also the better answer whenever it is available. Probing
        // the API of a process that is gone can only produce a startup timeout,
        // which sends somebody looking at the network instead of at the reason
        // the server stopped.
        if server.is_finished() {
            (&mut *server).await.context("join serving task")??;
            if shutdown_requested.load(Ordering::SeqCst) {
                return Ok(ReadinessOutcome::ShutdownRequested);
            }
            bail!("server exited before the API became ready");
        }
        tokio::select! {
            outcome = &mut *server => {
                outcome.context("join serving task")??;
                if shutdown_requested.load(Ordering::SeqCst) {
                    return Ok(ReadinessOutcome::ShutdownRequested);
                }
                bail!("server exited before the API became ready");
            }
            response = client.get(format!("{api_base}/models")).send() => {
                if let Ok(response) = response && response.status().is_success() {
                    let body: serde_json::Value = response.json().await.context("read ready model list")?;
                    let listed = body["data"].as_array().is_some_and(|items| items.iter().any(|item| item["id"] == model_id));
                    if listed && !server.is_finished() {
                        return Ok(ReadinessOutcome::Ready);
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "API did not become ready within {} seconds at {api_base}",
                startup_timeout.as_secs()
            );
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn report_model_open_event(event: ModelOpenEvent) -> std::io::Result<()> {
    match event.kind {
        ModelOpenEventKind::Progress(progress) => {
            let unit = match progress.unit {
                ModelOpenProgressUnit::Bytes => "bytes",
                ModelOpenProgressUnit::Items => "items",
                ModelOpenProgressUnit::Tensors => "tensors",
                ModelOpenProgressUnit::Steps => "steps",
                _ => "units",
            };
            console::progress_with_unit(
                "Model load",
                progress.current,
                progress.total.unwrap_or(0),
                unit,
            )
        }
        ModelOpenEventKind::BackendDeviceSelected => console::event(
            "backend_device_selected",
            &serde_json::json!({"detail": String::from_utf8_lossy(&event.detail)}),
        ),
        ModelOpenEventKind::FailedHandled(_) => console::event(
            "model_open_failed_handled",
            &serde_json::json!({"detail": String::from_utf8_lossy(&event.detail)}),
        ),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;

    const READINESS_CLIENT_CHILD: &str = "SKIPPY_TEST_READINESS_CLIENT_CHILD";

    // Without CA certificates, building a client that loads the system's roots
    // fails ("No CA certificates were loaded from the system"), which stopped
    // `skippy serve` right after it started listening. SSL_CERT_FILE and
    // SSL_CERT_DIR point the root loading at an empty file and directory on
    // Linux; they are set on a child test process only.
    #[test]
    fn readiness_client_needs_no_ca_certificates() {
        let temp = tempfile::tempdir().unwrap();
        let empty_file = temp.path().join("empty.pem");
        std::fs::write(&empty_file, "").unwrap();
        let empty_dir = temp.path().join("certs");
        std::fs::create_dir(&empty_dir).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "serve::tests::readiness_client_child",
                "--nocapture",
            ])
            .env(READINESS_CLIENT_CHILD, "1")
            .env("SSL_CERT_FILE", &empty_file)
            .env("SSL_CERT_DIR", &empty_dir)
            .output()
            .expect("spawn readiness client child");
        assert!(
            output.status.success(),
            "readiness client child failed: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("1 passed"),
            "child test filter must execute its assertion"
        );
    }

    // The responder answers its first request with a redirect to itself. A
    // client that followed it would connect again, which the listener sees as
    // a second pending connection.
    #[tokio::test(flavor = "current_thread")]
    async fn readiness_client_does_not_follow_redirects() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let responder = listener.try_clone().unwrap();
        let location = url.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = responder.accept().unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while std::io::BufRead::read_line(&mut reader, &mut line).unwrap() > 2 {
                line.clear();
            }
            let response = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
        });

        let response = readiness_client()
            .unwrap()
            .get(&url)
            .send()
            .await
            .expect("the redirect response itself is the result");
        server.join().unwrap();

        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        listener.set_nonblocking(true).unwrap();
        assert_eq!(
            listener.accept().map(|_| ()).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock,
            "the client must not send a second request"
        );
    }

    #[test]
    fn readiness_client_child() {
        if std::env::var_os(READINESS_CLIENT_CHILD).is_none() {
            return;
        }
        readiness_client().expect("the readiness client must build without CA certificates");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn readiness_distinguishes_requested_shutdown_from_premature_completion() {
        let (stop, stopped) = oneshot::channel();
        let (shutdown, shutdown_requested) = track_shutdown_request(async move {
            stopped.await.unwrap();
        });
        let mut server = tokio::spawn(async move {
            shutdown.await;
            Ok(())
        });
        stop.send(()).unwrap();
        let outcome = wait_for_ready(
            "http://127.0.0.1:0/v1",
            "never-ready",
            &mut server,
            Duration::from_secs(1),
            &shutdown_requested,
        )
        .await
        .unwrap();
        assert_eq!(outcome, ReadinessOutcome::ShutdownRequested);

        let mut server = tokio::spawn(async { Ok(()) });
        let error = wait_for_ready(
            "http://127.0.0.1:0/v1",
            "never-ready",
            &mut server,
            Duration::from_secs(1),
            &AtomicBool::new(false),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("server exited before the API became ready")
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn requested_shutdown_does_not_hide_a_pre_ready_server_failure() {
        let (shutdown, shutdown_requested) = track_shutdown_request(async {});
        let mut server = tokio::spawn(async move {
            shutdown.await;
            anyhow::bail!("model open failed")
        });
        let error = wait_for_ready(
            "http://127.0.0.1:0/v1",
            "never-ready",
            &mut server,
            Duration::from_secs(1),
            &shutdown_requested,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("model open failed"));
    }

    #[test]
    fn binary_worker_bind_override_is_explicit() {
        for (flags, expected) in [
            (Vec::<&str>::new(), None),
            (
                vec!["--bind-addr", "192.0.2.10:9400"],
                Some("192.0.2.10:9400".parse().unwrap()),
            ),
        ] {
            let mut argv = vec![
                "skippy",
                "serve",
                "--config",
                "stage.json",
                "--worker-only",
                "--stage-transport",
                "binary",
            ];
            argv.extend(flags);
            let cli = Cli::try_parse_from(argv).unwrap();
            let Command::Serve(args) = cli.command else {
                panic!("expected serve");
            };
            assert_eq!(args.public.bind_addr, expected);
        }
    }

    #[test]
    fn readiness_uses_the_bound_interface_for_lan_only_listeners() {
        for (bound, expected) in [
            ("0.0.0.0:9337", "127.0.0.1:9337"),
            ("[::]:9337", "[::1]:9337"),
            ("192.0.2.10:9337", "192.0.2.10:9337"),
            ("[2001:db8::10]:9337", "[2001:db8::10]:9337"),
        ] {
            assert_eq!(
                readiness_addr(bound.parse().unwrap()),
                expected.parse().unwrap()
            );
        }
    }

    #[test]
    fn common_frontend_flags_reach_binary_stage_zero() {
        let cli = Cli::try_parse_from([
            "skippy",
            "serve",
            "--config",
            "stage-0.json",
            "--stage-transport",
            "binary",
            "--model-id",
            "served-model",
            "--default-max-tokens",
            "64",
            "--generation-concurrency",
            "2",
            "--prefill-chunk-size",
            "512",
        ])
        .unwrap();
        let Command::Serve(mut args) = cli.command else {
            panic!("expected serve");
        };
        apply_public_frontend_tuning(&args.public, &mut args.stage);
        args.stage.settings = args.public.settings.clone();
        assert_eq!(args.stage.openai_model_id.as_deref(), Some("served-model"));
        assert_eq!(args.stage.openai_default_max_tokens, 64);
        assert_eq!(args.stage.openai_generation_concurrency, Some(2));
        assert_eq!(args.stage.openai_prefill_chunk_size, 512);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn binary_stage_defaults_reach_config_loading() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("missing-stage.json");
        let cli = Cli::try_parse_from([
            "skippy",
            "serve",
            "--config",
            config.to_str().unwrap(),
            "--stage-transport",
            "binary",
            "--worker-only",
        ])
        .unwrap();
        let Command::Serve(args) = cli.command else {
            panic!("expected serve");
        };
        let error = serve_binary_stage(*args).await.unwrap_err();
        assert!(error.to_string().contains("load stage config"), "{error:#}");
    }
}
