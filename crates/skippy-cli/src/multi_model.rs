use std::{collections::BTreeSet, net::SocketAddr};

use anyhow::{Context, Result, ensure};
use serde_json::json;
use skippy_api::serving::{
    LocalOpenAiOptions,
    multi_model::{LocalModelDefinition, MultiModelServer},
};
use skippy_commands::console;
use skippy_inference_api::ModelId;

use crate::{
    cli::{ServeCommandArgs, ServeOpenAiArgs},
    conversion::local_openai_options,
    serve::default_public_bind_addr,
    serve_settings::ServeSettings,
};

pub async fn run(mut args: ServeCommandArgs) -> Result<()> {
    let bind = args
        .public
        .bind_addr
        .unwrap_or_else(default_public_bind_addr);
    for model in &mut args.local_models {
        crate::model_source::resolve(model)
            .await
            .with_context(|| format!("resolve {}", describe_model(model)))?;
    }
    let server = tokio::task::spawn_blocking(move || {
        let models = prepare_models(args.local_models)?;
        if args.print_effective_config {
            print_effective_config(bind, models)?;
            return Ok(None);
        }
        let definitions = models
            .into_iter()
            .map(|model| {
                let id = model.id().to_owned();
                let (_, request) = model
                    .options
                    .into_request()
                    .with_context(|| format!("prepare model {id:?}"))?;
                Ok(LocalModelDefinition { id, request })
            })
            .collect::<Result<Vec<_>>>()?;
        MultiModelServer::load(definitions, None).map(Some)
    })
    .await??;
    let Some(server) = server else {
        return Ok(());
    };
    let listener = tokio::net::TcpListener::bind(bind).await?;
    console::status(&format!(
        "Serving local models at {}",
        listener.local_addr()?
    ))?;
    server.serve(listener, crate::shutdown_signal()?).await
}

pub(crate) fn describe_model(model: &ServeOpenAiArgs) -> String {
    if let Some(id) = &model.model_id {
        format!("model {id:?}")
    } else if let Some(reference) = &model.model {
        format!("model {reference:?}")
    } else if let Some(path) = &model.model_path {
        format!("model at {path:?}")
    } else {
        "model".into()
    }
}

struct PreparedModel {
    settings: ServeSettings,
    options: LocalOpenAiOptions,
}

impl PreparedModel {
    fn id(&self) -> &str {
        self.options
            .model_id
            .as_deref()
            .unwrap_or(&self.options.config.model_id)
    }
}

fn prepare_models(models: Vec<ServeOpenAiArgs>) -> Result<Vec<PreparedModel>> {
    let mut ids = BTreeSet::new();
    models
        .into_iter()
        .map(|model| {
            let label = describe_model(&model);
            let settings = model.settings.clone();
            let options =
                local_openai_options(model).with_context(|| format!("prepare {label}"))?;
            let prepared = PreparedModel { settings, options };
            let id = ModelId::new(prepared.id())?;
            ensure!(
                ids.insert(id.into_string()),
                "duplicate served model ID {:?}; set a distinct id on each entry",
                prepared.id()
            );
            Ok(prepared)
        })
        .collect()
}

fn print_effective_config(bind: SocketAddr, models: Vec<PreparedModel>) -> Result<()> {
    let models = models
        .into_iter()
        .map(|model| {
            let options = &model.options;
            let frontend = options.resolved_openai_options()?;
            Ok(json!({
                "id": model.id(),
                "config": model.settings.report(&options.config, Some(&frontend), &options.tuning),
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    console::write_json(&json!({
        "schema_version": 1,
        "resolution_phase": "before-model-load",
        "bind_addr": bind,
        "models": models,
    }))
}

#[cfg(test)]
mod tests;
