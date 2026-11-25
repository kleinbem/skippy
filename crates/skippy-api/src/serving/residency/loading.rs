use super::*;

struct ChargedModel {
    _resident: BackendLease,
    _budget: Reservation,
}

impl Control {
    pub(super) fn begin_load(
        self: &Arc<Self>,
        state: &mut State,
    ) -> watch::Receiver<Option<LoadResult>> {
        if let Some(attempt) = &state.attempt {
            return attempt.result.clone();
        }
        let (sender, result) = watch::channel(None);
        state.attempt = Some(Attempt {
            result: result.clone(),
        });
        let control = self.clone();
        self.runtime.spawn(async move {
            let mut load = {
                let control = control.clone();
                tokio::task::spawn_blocking(move || control.load_charged())
            };
            match tokio::time::timeout(control.policy.load_timeout, &mut load).await {
                Ok(outcome) => {
                    let outcome = outcome
                        .map_err(|error| {
                            InferenceError::backend(format!("join model load: {error}"))
                        })
                        .and_then(|result| result);
                    let mut state = control.state();
                    let outcome = if state.paused || state.draining {
                        drop(state);
                        drop(outcome);
                        control.state().attempt = None;
                        control.finish_drain();
                        Err(InferenceError::cancelled("model residency was paused"))
                    } else {
                        let outcome = outcome.map(|resident| {
                            state.resident = Some(resident);
                            Self::idle(&control, &mut state);
                        });
                        state.attempt = None;
                        outcome
                    };
                    let _ = sender.send(Some(outcome));
                }
                Err(_) => {
                    let _ = sender.send(Some(Err(InferenceError::timeout(
                        "cold model load timed out",
                    ))));
                    // Do not admit another load until the blocking native load
                    // has returned and dropped both its runtime and reservation.
                    drop(load.await);
                    control.state().attempt = None;
                    control.finish_drain();
                }
            }
        });
        result
    }

    fn load_charged(&self) -> InferenceResult<BackendLease> {
        let budget = self.budget.reserve(self.model_bytes)?;
        let resident = (self.loader)().map_err(|error| {
            InferenceError::backend(format!("load {}: {error:#}", self.model.id))
        })?;
        if resident.backend().http_exchange_policy().is_some() && self.http_policy.is_none() {
            return Err(InferenceError::internal(
                "loaded model requires a public-listener HTTP exchange policy",
            ));
        }
        Ok(BackendLease::new(
            resident.backend().clone(),
            Arc::new(ChargedModel {
                _resident: resident,
                _budget: budget,
            }),
        ))
    }
}
