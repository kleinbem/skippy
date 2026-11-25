use super::*;
use skippy_inference_api::generation::{
    GenerationEventStream, GenerationRequest, GenerationResponse,
};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fake;
#[async_trait::async_trait]
impl InferenceBackend for Fake {
    async fn models(&self) -> InferenceResult<Vec<ModelObject>> {
        Ok(vec![])
    }
    async fn chat_completion(&self, _: GenerationRequest) -> InferenceResult<GenerationResponse> {
        Err(InferenceError::unsupported("fake model"))
    }
    async fn chat_completion_stream(
        &self,
        _: GenerationRequest,
        _: InferenceRequestContext,
    ) -> InferenceResult<GenerationEventStream> {
        Ok(Box::pin(futures_util::stream::pending()))
    }
}

struct Owner(Arc<AtomicUsize>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

fn model(id: &str, dropped: Arc<AtomicUsize>) -> Loader {
    let model = ModelObject::new(id);
    Box::new(move || {
        Ok((
            model,
            BackendLease::new(Arc::new(Fake), Arc::new(Owner(dropped))),
        ))
    })
}

#[tokio::test]
async fn empty_startup_and_stream_draining_release_the_correct_instance() {
    let registry = ManagedModels::new(None);
    assert!(registry.backend().models().await.unwrap().is_empty());
    let dropped = Arc::new(AtomicUsize::new(0));
    let ready = registry
        .load("local".into(), model("local", dropped.clone()))
        .await
        .unwrap();
    let context = InferenceRequestContext::new();
    let backend = registry.backend();
    let request = GenerationRequest {
        model: "local".into(),
        ..Default::default()
    };
    let stream = backend
        .chat_completion_stream(request, context.clone())
        .await
        .unwrap();
    assert!(
        registry
            .drain("local", Some(ready.instance), Duration::ZERO)
            .await
            .is_err()
    );
    assert!(context.is_cancelled());
    assert!(registry.acquire("local", None).await.is_err());
    assert_eq!(registry.status()[0].phase, Phase::Draining);
    assert_eq!(dropped.load(Ordering::Relaxed), 0);
    drop(stream);
    assert_eq!(registry.status()[0].phase, Phase::Unloaded);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    let next = registry
        .load("local".into(), model("local", dropped.clone()))
        .await
        .unwrap();
    assert_ne!(next.instance, ready.instance);
    assert!(
        registry
            .unload("local", Some(ready.instance))
            .await
            .is_err()
    );
    assert!(registry.unload("local", Some(next.instance)).await.is_ok());
}

#[tokio::test]
async fn best_effort_continues_and_fail_fast_releases_already_loaded_models() {
    fn definitions(dropped: Arc<AtomicUsize>) -> Vec<(String, Loader)> {
        let fail: Loader = Box::new(|| anyhow::bail!("unavailable"));
        vec![("one".into(), model("one", dropped)), ("two".into(), fail)]
    }
    let dropped = Arc::new(AtomicUsize::new(0));
    assert!(
        ManagedModels::start(definitions(dropped.clone()), StartupPolicy::FailFast, None)
            .await
            .is_err()
    );
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    let partial = ManagedModels::start(definitions(dropped), StartupPolicy::BestEffort, None)
        .await
        .unwrap();
    assert_eq!(partial.backend().models().await.unwrap().len(), 1);
    assert_eq!(partial.status()[1].phase, Phase::Failed);
}

#[tokio::test]
async fn native_owner_can_observe_lifecycle_during_unload() {
    struct ObservedDrop(Weak<Registry>, Arc<std::sync::atomic::AtomicBool>);
    impl Drop for ObservedDrop {
        fn drop(&mut self) {
            let Some(registry) = self.0.upgrade() else {
                return;
            };
            let seen = registry
                .state
                .try_lock()
                .is_ok_and(|state| state.models["test"].status.phase == Phase::Draining);
            self.1.store(seen, Ordering::Release);
        }
    }

    let registry = ManagedModels::new(None);
    let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let owner = ObservedDrop(Arc::downgrade(&registry.0), seen.clone());
    registry
        .load(
            "test".into(),
            Box::new(|| {
                Ok((
                    ModelObject::new("test"),
                    BackendLease::new(Arc::new(Fake), Arc::new(owner)),
                ))
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        registry.unload("test", None).await.unwrap().phase,
        Phase::Unloaded
    );
    assert!(seen.load(Ordering::Acquire));
}
