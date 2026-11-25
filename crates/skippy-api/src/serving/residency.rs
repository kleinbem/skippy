//! Native model cold-load coalescing and lease-based idle eviction.

use std::{
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, ensure};
use skippy_inference_api::{
    InferenceBackend, InferenceError, InferenceRequestContext, InferenceResult, ModelObject,
    http_exchange::HttpExchangePolicy,
    model_dispatch::{BackendLease, ModelDispatcher, ModelLookup},
};
use tokio::{sync::watch, time::Instant};

mod loading;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Startup {
    #[default]
    Eager,
    OnDemand,
}

#[derive(Clone, Copy, Debug)]
pub struct ResidencyPolicy {
    pub startup: Startup,
    pub idle_after: Option<Duration>,
    pub load_timeout: Duration,
}

impl Default for ResidencyPolicy {
    fn default() -> Self {
        Self {
            startup: Startup::Eager,
            idle_after: None,
            load_timeout: Duration::from_secs(900),
        }
    }
}

pub type Loader = Arc<dyn Fn() -> Result<BackendLease> + Send + Sync>;
pub type Prefetch = Arc<dyn Fn() -> Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct CapacityBudget {
    limit: u64,
    reserved: Arc<AtomicU64>,
}

impl CapacityBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            limit,
            reserved: Arc::default(),
        }
    }

    fn reserve(&self, bytes: u64) -> InferenceResult<Reservation> {
        self.reserved
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current
                    .checked_add(bytes)
                    .filter(|total| *total <= self.limit)
            })
            .map_err(|current| {
                InferenceError::backend(format!(
                    "model requires {bytes} bytes of residency capacity; {} bytes are available",
                    self.limit - current,
                ))
            })?;
        Ok(Reservation {
            reserved: self.reserved.clone(),
            bytes,
        })
    }
}

struct Reservation {
    reserved: Arc<AtomicU64>,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

type LoadResult = InferenceResult<()>;

struct Attempt {
    result: watch::Receiver<Option<LoadResult>>,
}

#[derive(Default)]
struct State {
    attempt: Option<Attempt>,
    resident: Option<BackendLease>,
    active: usize,
    idle_since: Option<Instant>,
    paused: bool,
    draining: bool,
    cleaning: bool,
}

struct Control {
    state: Mutex<State>,
    model: ModelObject,
    policy: ResidencyPolicy,
    loader: Loader,
    budget: CapacityBudget,
    model_bytes: u64,
    runtime: tokio::runtime::Handle,
    http_policy: Option<Arc<dyn HttpExchangePolicy>>,
}

impl Control {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Used for operator unload and idle eviction; a leased model can only
    /// retire after its last request (including an SSE or realtime stream).
    fn finish_drain(&self) {
        let resident = {
            let mut state = self.state();
            if !state.draining || state.active != 0 || state.attempt.is_some() || state.cleaning {
                return;
            }
            state.cleaning = true;
            state.resident.take()
        };
        // Keep admission closed while disposal releases native resources.
        // Model owners may query controller state during their cleanup.
        drop(resident);
        let mut state = self.state();
        state.idle_since = None;
        state.draining = false;
        state.cleaning = false;
    }

    fn idle(control: &Arc<Self>, state: &mut State) {
        let Some(duration) = control.policy.idle_after else {
            return;
        };
        let now = Instant::now();
        state.idle_since = Some(now);
        let weak = Arc::downgrade(control);
        control.runtime.spawn(async move {
            tokio::time::sleep_until(now + duration).await;
            let Some(control) = weak.upgrade() else {
                return;
            };
            let mut state = control.state();
            if state.active == 0 && state.idle_since == Some(now) {
                state.draining = true;
                drop(state);
                control.finish_drain();
            }
        });
    }
}

#[derive(Clone)]
pub struct LocalResidency(Arc<Control>);

impl LocalResidency {
    /// Prepare a residency controller. Prefetch runs once before readiness, if
    /// supplied. The loader is called without holding the admission lock.
    pub async fn start(
        model: ModelObject,
        policy: ResidencyPolicy,
        budget: CapacityBudget,
        model_bytes: u64,
        prefetch: Option<Prefetch>,
        loader: Loader,
        http_policy: Option<Arc<dyn HttpExchangePolicy>>,
    ) -> Result<Self> {
        skippy_inference_api::ModelId::new(&model.id)?;
        ensure!(model_bytes > 0, "model capacity estimate must be positive");
        ensure!(
            !policy.load_timeout.is_zero() && policy.load_timeout <= Duration::from_secs(86400),
            "load timeout must be between zero and one day"
        );
        ensure!(
            policy.idle_after.is_none_or(
                |duration| !duration.is_zero() && duration <= Duration::from_secs(86400)
            ),
            "idle deadline must be between zero and one day"
        );
        if let Some(prefetch) = prefetch {
            tokio::task::spawn_blocking(move || prefetch()).await??;
        }
        let control = Self(Arc::new(Control {
            state: Mutex::default(),
            model,
            policy,
            loader,
            budget,
            model_bytes,
            runtime: tokio::runtime::Handle::current(),
            http_policy,
        }));
        if policy.startup == Startup::Eager {
            let load = control.acquire(&control.0.model.id, None).await;
            load.map_err(|error| anyhow::anyhow!("{}", error.message()))?;
        }
        Ok(control)
    }

    pub fn backend(&self) -> Arc<dyn InferenceBackend> {
        Arc::new(ModelDispatcher(self.clone()))
    }

    /// Stop new admission and retire when active owners release the model.
    /// A pending native load is discarded when its worker returns.
    pub fn pause(&self) {
        let mut state = self.0.state();
        state.paused = true;
        state.draining = true;
        drop(state);
        self.0.finish_drain();
    }

    /// Re-enable loading. Admission stays closed until any previous drain ends.
    pub fn resume(&self) {
        self.0.state().paused = false;
    }

    pub fn is_resident(&self) -> bool {
        self.0.state().resident.is_some()
    }
}

impl ModelLookup for LocalResidency {
    async fn ready_models(&self) -> InferenceResult<Vec<ModelObject>> {
        Ok(if self.0.state().paused {
            vec![]
        } else {
            vec![self.0.model.clone()]
        })
    }

    async fn acquire(
        &self,
        id: &str,
        context: Option<&InferenceRequestContext>,
    ) -> InferenceResult<BackendLease> {
        if id != self.0.model.id {
            return Err(InferenceError::model_not_found(id));
        }
        let cancellation = context
            .map(InferenceRequestContext::cancellation_token)
            .unwrap_or_default();
        loop {
            if cancellation.is_cancelled() {
                return Err(InferenceError::cancelled("model request cancelled"));
            }
            let mut attempt = {
                let mut state = self.0.state();
                if state.paused || state.draining {
                    return Err(InferenceError::backend(
                        "model residency is paused or draining",
                    ));
                }
                if let Some(resident) = state.resident.clone() {
                    state.active += 1;
                    state.idle_since = None;
                    return Ok(BackendLease::new(
                        resident.backend().clone(),
                        Arc::new(Active {
                            control: Arc::downgrade(&self.0),
                            resident: Some(resident),
                        }),
                    ));
                }
                self.0.begin_load(&mut state)
            };
            let result = tokio::select! {
                () = cancellation.cancelled() => return Err(InferenceError::cancelled("model request cancelled")),
                status = attempt.wait_for(Option::is_some) => status,
            };
            match result {
                Ok(status) => status.as_ref().cloned().unwrap_or(Ok(()))?,
                Err(_) => return Err(InferenceError::backend("model load ended without a result")),
            }
        }
    }

    fn http_exchange_policy(&self) -> Option<Arc<dyn HttpExchangePolicy>> {
        self.0.http_policy.clone()
    }
}

struct Active {
    control: Weak<Control>,
    resident: Option<BackendLease>,
}

impl Drop for Active {
    fn drop(&mut self) {
        drop(self.resident.take());
        let Some(control) = self.control.upgrade() else {
            return;
        };
        let mut state = control.state();
        state.active -= 1;
        if state.active == 0 {
            if state.draining {
                drop(state);
                control.finish_drain();
            } else {
                Control::idle(&control, &mut state);
            }
        }
    }
}

#[cfg(test)]
mod tests;
