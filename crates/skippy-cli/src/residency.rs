use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use skippy_api::serving::{
    LocalOpenAiOptions,
    residency::{CapacityBudget, Loader, LocalResidency, ResidencyPolicy, Startup},
};
use skippy_api::speculative::PreparedDraft;
use skippy_inference_api::{ModelObject, model_dispatch::BackendLease};

use crate::cli::{ServeCommandArgs, ServeOpenAiArgs};

pub struct Options {
    policy: ResidencyPolicy,
    capacity_bytes: Option<u64>,
}

pub fn options(args: &ServeOpenAiArgs) -> Option<Options> {
    (args.on_demand || args.idle_evict_secs.is_some() || args.residency_budget_bytes.is_some())
        .then(|| Options {
            policy: ResidencyPolicy {
                startup: if args.on_demand {
                    Startup::OnDemand
                } else {
                    Startup::Eager
                },
                idle_after: args.idle_evict_secs.map(Duration::from_secs),
                load_timeout: Duration::from_secs(args.startup_timeout_secs.max(1)),
            },
            capacity_bytes: args.residency_budget_bytes,
        })
}

pub fn validate(args: &ServeCommandArgs) -> Result<()> {
    if !args.local_models.is_empty() {
        ensure!(
            options(&args.public).is_none()
                && args
                    .local_models
                    .iter()
                    .all(|model| options(model).is_none()),
            "residency controls require a single unsplit model"
        );
    }
    if options(&args.public).is_some() {
        ensure!(
            args.stage_transport.is_none() && !args.prompt,
            "residency controls require an unsplit public server without an interactive prompt"
        );
        ensure!(
            args.public
                .idle_evict_secs
                .is_none_or(|value| (1..=86_400).contains(&value)),
            "idle eviction must be between 1 and 86400 seconds"
        );
        ensure!(
            args.public.residency_budget_bytes != Some(0),
            "residency budget must be positive"
        );
    }
    Ok(())
}

pub async fn serve(
    model: LocalOpenAiOptions,
    options: Options,
    draft: Option<PreparedDraft>,
) -> Result<()> {
    ensure!(
        model.config.upstream.is_none()
            && model.config.downstream.is_none()
            && model.config.resident_tensor_names.is_empty(),
        "residency requires an unsplit model"
    );
    let id = model
        .model_id
        .clone()
        .unwrap_or_else(|| model.config.model_id.clone());
    let model_bytes = model
        .config
        .source_model_bytes
        .filter(|bytes| *bytes > 0)
        .context("residency requires an identified local source with a known size")?;
    let capacity = options.capacity_bytes.unwrap_or(model_bytes);
    let address = model.bind_addr;
    let load_id = id.clone();
    let model = Mutex::new(model);
    let loader: Loader = Arc::new(move || {
        let (_, request) = model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .load_request(crate::serve::model_open_events())?;
        let started = Instant::now();
        let result = match &draft {
            Some(draft) => request.load_with_default_draft(draft),
            None => request.load(),
        };
        crate::serve::record_model_load(&load_id, started.elapsed(), result.is_ok());
        let loaded = Arc::new(result?);
        Ok(BackendLease::new(loaded.backend.clone(), loaded))
    });
    let residency = LocalResidency::start(
        ModelObject::try_new(id)?,
        options.policy,
        CapacityBudget::new(capacity),
        model_bytes,
        None,
        loader,
        None,
    )
    .await?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    skippy_commands::console::status(&format!("Inference ready at {}", listener.local_addr()?))?;
    skippy_inference_api::model_dispatch::serve_models(
        listener,
        residency.backend(),
        crate::shutdown_signal()?,
    )
    .await?;
    Ok(())
}
