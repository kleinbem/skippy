//! Eager, fail-fast composition of independently configured local models.

use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, Result, ensure};
use skippy_inference_api::{
    InferenceBackend, InferenceError, InferenceRequestContext, InferenceResult, ModelId,
    ModelObject,
    http_exchange::HttpExchangePolicy,
    model_dispatch::{BackendLease, ModelDispatcher, ModelLookup},
};

use super::ModelLoadRequest;

struct Models {
    entries: BTreeMap<String, (ModelObject, BackendLease)>,
    http_policy: Option<Arc<dyn HttpExchangePolicy>>,
}

impl ModelLookup for Models {
    async fn ready_models(&self) -> InferenceResult<Vec<ModelObject>> {
        Ok(self
            .entries
            .values()
            .map(|(model, _)| model.clone())
            .collect())
    }

    async fn acquire(
        &self,
        id: &str,
        _: Option<&InferenceRequestContext>,
    ) -> InferenceResult<BackendLease> {
        self.entries
            .get(id)
            .map(|(_, lease)| lease.clone())
            .ok_or_else(|| InferenceError::model_not_found(id))
    }

    fn http_exchange_policy(&self) -> Option<Arc<dyn HttpExchangePolicy>> {
        self.http_policy.clone()
    }
}

/// One admitted local model with an explicit served ID, independent inputs and hooks.
pub struct LocalModelDefinition {
    pub id: String,
    pub request: ModelLoadRequest,
}

/// Immutable model registry. Every entry is ready at publication. Failed
/// startup drops previously loaded models and never publishes a partial registry.
pub struct MultiModelServer {
    backend: Arc<dyn InferenceBackend>,
}

impl MultiModelServer {
    /// Loads models serially on the calling thread. Call from a blocking thread
    /// when using an async runtime. IDs are checked before any model is loaded.
    pub fn load(
        definitions: Vec<LocalModelDefinition>,
        http_policy: Option<Arc<dyn HttpExchangePolicy>>,
    ) -> Result<Self> {
        let mut ids = std::collections::BTreeSet::new();
        for definition in &definitions {
            ModelId::new(&definition.id)?;
            ensure!(
                ids.insert(&definition.id),
                "duplicate served model ID {:?}",
                definition.id
            );
            let config = &definition.request.runtime.config;
            ensure!(
                config.upstream.is_none()
                    && config.downstream.is_none()
                    && config.resident_tensor_names.is_empty(),
                "multi-model serving requires unsplit local model {}",
                definition.id
            );
        }

        let mut entries = Vec::with_capacity(definitions.len());
        for LocalModelDefinition { id, mut request } in definitions {
            request.openai.model_id = Some(id.clone());
            let loaded = Arc::new(request.load().with_context(|| format!("load model {id}"))?);
            ensure!(
                loaded.backend.http_exchange_policy().is_none() || http_policy.is_some(),
                "model {id} requested HTTP exchange admission; supply a public-listener policy"
            );
            let model = ModelObject::try_new(id)?.with_thinking(loaded.thinking.clone());
            entries.push((model, BackendLease::new(loaded.backend.clone(), loaded)));
        }
        Self::from_backends(entries, http_policy)
    }

    /// Compose ready, caller-supplied backends using the same exact-ID dispatch.
    pub fn from_backends(
        entries: Vec<(ModelObject, BackendLease)>,
        http_policy: Option<Arc<dyn HttpExchangePolicy>>,
    ) -> Result<Self> {
        ensure!(
            !entries.is_empty(),
            "multi-model serving requires at least one model"
        );
        let mut models = BTreeMap::new();
        for (model, backend) in entries {
            ModelId::new(&model.id)?;
            ensure!(
                backend.backend().http_exchange_policy().is_none() || http_policy.is_some(),
                "model {} requires a public-listener HTTP exchange policy",
                model.id
            );
            let id = model.id.clone();
            ensure!(
                models.insert(id.clone(), (model, backend)).is_none(),
                "duplicate served model ID {id:?}"
            );
        }
        Ok(Self {
            backend: Arc::new(ModelDispatcher(Models {
                entries: models,
                http_policy,
            })),
        })
    }

    pub fn backend(&self) -> Arc<dyn InferenceBackend> {
        self.backend.clone()
    }

    /// Serve every ready model on the same listener.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<()> {
        skippy_inference_api::model_dispatch::serve_models(listener, self.backend, shutdown)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
