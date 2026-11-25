use super::*;

impl ManagedModels {
    /// Publish acceptance as Loading, then wait for completion. Dropping the
    /// future after acceptance leaves the operation observable by status. Use
    /// unload/drain to cancel a load while the native call unwinds.
    pub async fn load(&self, id: String, loader: Loader) -> Result<ModelStatus> {
        ModelId::new(&id)?;
        let instance = {
            let mut state = self.0.state();
            if let Some(existing) = state.models.get(&id) {
                match existing.status.phase {
                    Phase::Ready => return Ok(existing.status.clone()),
                    Phase::Loading | Phase::Draining => {
                        bail!("model {id} already has a lifecycle operation in progress")
                    }
                    Phase::Unloaded | Phase::Failed => {}
                }
            }
            state.sequence = state
                .sequence
                .checked_add(1)
                .context("lifecycle sequence exhausted")?;
            let instance = state.sequence;
            state.models.insert(
                id.clone(),
                Entry {
                    status: ModelStatus {
                        id: id.clone(),
                        instance,
                        phase: Phase::Loading,
                        in_flight: 0,
                        error: None,
                    },
                    cancel_load: false,
                    cleaning: false,
                    resident: None,
                    active: BTreeMap::new(),
                },
            );
            instance
        };
        self.0.changed.notify_waiters();
        let registry = self.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(loader)
                .await
                .context("join model loader")
                .and_then(|result| result);
            registry.finish_load(&id, instance, result)
        })
        .await
        .context("join lifecycle operation")?
    }

    fn finish_load(
        &self,
        id: &str,
        instance: u64,
        result: Result<(ModelObject, BackendLease)>,
    ) -> Result<ModelStatus> {
        let result = result.and_then(|(model, lease)| {
            ensure!(model.id == id, "loader returned a different model ID");
            ensure!(
                lease.backend().http_exchange_policy().is_none() || self.0.http_policy.is_some(),
                "model requires a public-listener HTTP exchange policy"
            );
            Ok((model, lease))
        });
        let mut state = self.0.state();
        let entry = matched_entry(&mut state, id, Some(instance))?;
        if entry.cancel_load {
            drop(state);
            drop(result);
            matched_entry(&mut self.0.state(), id, Some(instance))?.cancel_load = false;
            self.0.retire(id, instance);
            bail!("load cancelled for {id}");
        }
        let result = match result {
            Ok(resident) => {
                entry.resident = Some(resident);
                entry.status.phase = Phase::Ready;
                Ok(entry.status.clone())
            }
            Err(error) => {
                entry.status.phase = Phase::Failed;
                entry.status.error = Some(format!("{error:#}"));
                Err(error)
            }
        };
        self.0.changed.notify_waiters();
        result
    }

    /// Stop admission immediately and wait for active requests or loading to
    /// unwind. At the deadline, signal active requests to cancel. The model
    /// remains draining until all owners release it, including live streams.
    pub async fn drain(
        &self,
        id: &str,
        instance: Option<u64>,
        timeout: Duration,
    ) -> Result<ModelStatus> {
        let instance = {
            let mut state = self.0.state();
            let entry = matched_entry(&mut state, id, instance)?;
            match entry.status.phase {
                Phase::Loading => {
                    entry.status.phase = Phase::Draining;
                    entry.cancel_load = true;
                }
                _ => entry.status.phase = Phase::Draining,
            }
            entry.status.instance
        };
        self.0.changed.notify_waiters();
        self.0.retire(id, instance);
        let completed = tokio::time::timeout(timeout, async {
            loop {
                let changed = self.0.changed.notified();
                {
                    let mut state = self.0.state();
                    let entry = matched_entry(&mut state, id, Some(instance))?;
                    if entry.status.phase == Phase::Unloaded {
                        return Ok(entry.status.clone());
                    }
                }
                changed.await;
            }
        })
        .await;
        match completed {
            Ok(result) => result,
            Err(_) => {
                let mut state = self.0.state();
                let entry = matched_entry(&mut state, id, Some(instance))?;
                if entry.status.phase == Phase::Unloaded {
                    return Ok(entry.status.clone());
                }
                for cancellation in entry.active.values() {
                    cancellation.cancel();
                }
                bail!(
                    "drain deadline expired for {id} instance {instance}; release is still pending"
                )
            }
        }
    }

    /// Request immediate cancellation and release when the instance is idle.
    pub async fn unload(&self, id: &str, instance: Option<u64>) -> Result<ModelStatus> {
        self.drain(id, instance, Duration::ZERO).await
    }
}

fn matched_entry<'a>(
    state: &'a mut State,
    id: &str,
    instance: Option<u64>,
) -> Result<&'a mut Entry> {
    let entry = state
        .models
        .get_mut(id)
        .with_context(|| format!("unknown model {id}"))?;
    ensure!(
        instance.is_none_or(|instance| instance == entry.status.instance),
        "stale model instance"
    );
    Ok(entry)
}
