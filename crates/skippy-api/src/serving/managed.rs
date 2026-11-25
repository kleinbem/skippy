//! Model lifecycle and admission for a running inference listener.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard, Weak},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use skippy_inference_api::{
    CancellationToken, InferenceBackend, InferenceError, InferenceRequestContext, InferenceResult,
    ModelId, ModelObject,
    http_exchange::HttpExchangePolicy,
    model_dispatch::{BackendLease, ModelDispatcher, ModelLookup},
};
use tokio::sync::Notify;

use super::ModelLoadRequest;

mod operations;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Loading,
    Ready,
    Draining,
    Unloaded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelStatus {
    pub id: String,
    pub instance: u64,
    pub phase: Phase,
    pub in_flight: usize,
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupPolicy {
    FailFast,
    BestEffort,
}

pub type Loader = Box<dyn FnOnce() -> Result<(ModelObject, BackendLease)> + Send>;

#[derive(Clone)]
pub struct ManagedModels(Arc<Registry>);

struct Registry {
    state: Mutex<State>,
    changed: Notify,
    http_policy: Option<Arc<dyn HttpExchangePolicy>>,
}

#[derive(Default)]
struct State {
    models: BTreeMap<String, Entry>,
    sequence: u64,
}

struct Entry {
    status: ModelStatus,
    cancel_load: bool,
    cleaning: bool,
    resident: Option<(ModelObject, BackendLease)>,
    active: BTreeMap<u64, CancellationToken>,
}

impl Registry {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn retire(&self, id: &str, instance: u64) {
        let resident = {
            let mut state = self.state();
            let Some(entry) = state.models.get_mut(id) else {
                return;
            };
            if entry.status.instance != instance
                || entry.status.phase != Phase::Draining
                || !entry.active.is_empty()
                || entry.cancel_load
                || entry.cleaning
            {
                return;
            }
            entry.cleaning = true;
            entry.resident.take()
        };
        // Dropping a model can join native workers or call embedder-owned
        // cleanup. Keep its instance draining while that work runs unlocked.
        drop(resident);
        let mut state = self.state();
        let entry = state
            .models
            .get_mut(id)
            .expect("draining model remains registered");
        entry.cleaning = false;
        entry.status.phase = Phase::Unloaded;
        entry.status.error = None;
        self.changed.notify_waiters();
    }
}

impl ManagedModels {
    pub fn new(http_policy: Option<Arc<dyn HttpExchangePolicy>>) -> Self {
        Self(Arc::new(Registry {
            state: Mutex::default(),
            changed: Notify::new(),
            http_policy,
        }))
    }

    pub fn backend(&self) -> Arc<dyn InferenceBackend> {
        Arc::new(ModelDispatcher(self.clone()))
    }

    pub fn status(&self) -> Vec<ModelStatus> {
        self.0
            .state()
            .models
            .values()
            .map(|entry| entry.status.clone())
            .collect()
    }

    /// Build a local loader that owns the full backend and releases it on unload.
    pub fn local_loader(id: String, mut request: ModelLoadRequest) -> Loader {
        Box::new(move || {
            let config = &request.runtime.config;
            ensure!(
                config.upstream.is_none()
                    && config.downstream.is_none()
                    && config.resident_tensor_names.is_empty(),
                "managed loading requires an unsplit local model"
            );
            request.openai.model_id = Some(id.clone());
            let loaded = Arc::new(request.load().with_context(|| format!("load {id}"))?);
            let model = ModelObject::try_new(id)?.with_thinking(loaded.thinking.clone());
            Ok((model, BackendLease::new(loaded.backend.clone(), loaded)))
        })
    }

    /// Eager startup reports the status of every attempted load. Fail-fast
    /// startup rolls back previously loaded instances before returning.
    pub async fn start(
        models: Vec<(String, Loader)>,
        policy: StartupPolicy,
        http_policy: Option<Arc<dyn HttpExchangePolicy>>,
    ) -> Result<Self> {
        let mut ids = std::collections::BTreeSet::new();
        for (id, _) in &models {
            ModelId::new(id)?;
            ensure!(ids.insert(id), "duplicate startup model {id}");
        }
        let registry = Self::new(http_policy);
        for (id, loader) in models {
            if let Err(error) = registry.load(id, loader).await
                && policy == StartupPolicy::FailFast
            {
                return Err(error);
            }
        }
        Ok(registry)
    }
}

impl ModelLookup for ManagedModels {
    async fn ready_models(&self) -> InferenceResult<Vec<ModelObject>> {
        Ok(self
            .0
            .state()
            .models
            .values()
            .filter(|entry| entry.status.phase == Phase::Ready)
            .filter_map(|entry| entry.resident.as_ref().map(|(model, _)| model.clone()))
            .collect())
    }

    async fn acquire(
        &self,
        id: &str,
        context: Option<&InferenceRequestContext>,
    ) -> InferenceResult<BackendLease> {
        let mut state = self.0.state();
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or_else(|| InferenceError::internal("lifecycle sequence exhausted"))?;
        let sequence = state.sequence;
        let entry = state
            .models
            .get_mut(id)
            .ok_or_else(|| InferenceError::model_not_found(id))?;
        if entry.status.phase != Phase::Ready {
            return Err(InferenceError::backend(format!(
                "model {id} is {:?}",
                entry.status.phase
            )));
        }
        let (_, resident) = entry
            .resident
            .as_ref()
            .ok_or_else(|| InferenceError::internal("ready model has no backend"))?;
        let cancellation = context
            .map(InferenceRequestContext::cancellation_token)
            .unwrap_or_default();
        entry.active.insert(sequence, cancellation);
        entry.status.in_flight = entry.active.len();
        Ok(BackendLease::new(
            resident.backend().clone(),
            Arc::new(Active {
                registry: Arc::downgrade(&self.0),
                id: id.to_owned(),
                sequence,
                owner: Some(resident.clone()),
            }),
        ))
    }

    fn http_exchange_policy(&self) -> Option<Arc<dyn HttpExchangePolicy>> {
        self.0.http_policy.clone()
    }
}

struct Active {
    registry: Weak<Registry>,
    id: String,
    sequence: u64,
    owner: Option<BackendLease>,
}

impl Drop for Active {
    fn drop(&mut self) {
        drop(self.owner.take());
        let Some(registry) = self.registry.upgrade() else {
            return;
        };
        let instance = {
            let mut state = registry.state();
            let Some(entry) = state.models.get_mut(&self.id) else {
                return;
            };
            entry.active.remove(&self.sequence);
            entry.status.in_flight = entry.active.len();
            entry.status.instance
        };
        registry.retire(&self.id, instance);
        registry.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests;
