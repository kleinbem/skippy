use std::sync::{Arc, Mutex, Weak};

use skippy_inference_api::InferenceExecutionLease;
use tokio::sync::Notify;

use crate::execution::{ExecutionError, ExecutionErrorKind, ExecutionState};

pub(super) struct Drain<T> {
    shared: Arc<Shared<T>>,
}

struct Shared<T> {
    slot: Mutex<Slot<T>>,
    changed: Notify,
}

struct Slot<T> {
    state: ExecutionState,
    resources: Option<Arc<T>>,
    active: usize,
}

pub(super) struct Lease<T> {
    shared: Weak<Shared<T>>,
    resources: Option<Arc<T>>,
}

impl<T> Drain<T> {
    pub(super) fn new(resources: T) -> Self {
        Self {
            shared: Arc::new(Shared {
                slot: Mutex::new(Slot {
                    state: ExecutionState::Ready,
                    resources: Some(Arc::new(resources)),
                    active: 0,
                }),
                changed: Notify::new(),
            }),
        }
    }

    pub(super) fn state(&self) -> ExecutionState {
        self.shared
            .slot
            .lock()
            .expect("execution slot poisoned")
            .state
    }

    pub(super) fn admit(&self) -> Result<Arc<Lease<T>>, ExecutionError> {
        let mut slot = self.shared.slot.lock().expect("execution slot poisoned");
        if slot.state != ExecutionState::Ready {
            return Err(ExecutionError::new(
                ExecutionErrorKind::NotReady,
                anyhow::anyhow!("execution handle is {:?}", slot.state),
            ));
        }
        let resources = slot.resources.clone();
        slot.active += 1;
        Ok(Arc::new(Lease {
            shared: Arc::downgrade(&self.shared),
            resources,
        }))
    }

    pub(super) async fn stop(&self) {
        loop {
            let changed = self.shared.changed.notified();
            let resources = {
                let mut slot = self.shared.slot.lock().expect("execution slot poisoned");
                if slot.state == ExecutionState::Stopped {
                    return;
                }
                slot.state = ExecutionState::Draining;
                if slot.active == 0 {
                    slot.resources.take()
                } else {
                    None
                }
            };
            if let Some(resources) = resources {
                drop(resources);
                {
                    let mut slot = self.shared.slot.lock().expect("execution slot poisoned");
                    slot.state = ExecutionState::Stopped;
                }
                self.shared.changed.notify_waiters();
                return;
            }
            changed.await;
        }
    }
}

impl<T> Lease<T> {
    pub(super) fn resources(&self) -> &T {
        self.resources
            .as_deref()
            .expect("admitted lease has resources")
    }
}

impl<T> std::fmt::Debug for Lease<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExecutionLease")
            .finish_non_exhaustive()
    }
}

impl<T: Send + Sync + 'static> InferenceExecutionLease for Lease<T> {}

impl<T> Drop for Lease<T> {
    fn drop(&mut self) {
        drop(self.resources.take());
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut slot = shared.slot.lock().expect("execution slot poisoned");
        slot.active -= 1;
        if slot.active == 0 {
            shared.changed.notify_waiters();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn draining_refuses_new_work_and_waits_for_the_worker_lease() {
        let resources = Arc::new(());
        let probe = Arc::downgrade(&resources);
        let drain = Drain::new(resources);
        let response = drain.admit().unwrap();
        let worker = response.clone();
        drop(response);
        let mut stop = std::pin::pin!(drain.stop());
        tokio::select! {
            biased;
            _ = &mut stop => panic!("stopped with worker in flight"),
            _ = tokio::task::yield_now() => {}
        }
        assert_eq!(drain.state(), ExecutionState::Draining);
        assert_eq!(
            drain.admit().unwrap_err().kind(),
            ExecutionErrorKind::NotReady
        );
        assert!(probe.upgrade().is_some());
        drop(worker);
        stop.await;
        assert_eq!(drain.state(), ExecutionState::Stopped);
        assert!(probe.upgrade().is_none());
        drain.stop().await;
    }
}
