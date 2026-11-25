use super::*;

use skippy_inference_api::generation::{
    GenerationEventStream, GenerationRequest, GenerationResponse,
};
use std::sync::atomic::AtomicUsize;

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

fn policy() -> ResidencyPolicy {
    ResidencyPolicy {
        startup: Startup::OnDemand,
        idle_after: Some(Duration::from_millis(25)),
        load_timeout: Duration::from_secs(2),
    }
}

fn resident() -> BackendLease {
    BackendLease::new(Arc::new(Fake), Arc::new(()))
}

#[tokio::test]
async fn coalesces_requests_holds_residency_then_evicts_and_reloads() {
    let loads = Arc::new(AtomicUsize::new(0));
    let counter = loads.clone();
    let controller = LocalResidency::start(
        ModelObject::new("fixture"),
        policy(),
        CapacityBudget::new(128),
        128,
        None,
        Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
            Ok(resident())
        }),
        None,
    )
    .await
    .unwrap();
    assert!(!controller.is_resident());
    let (first, second) = tokio::join!(
        controller.acquire("fixture", None),
        controller.acquire("fixture", None)
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(controller.is_resident());
    drop((first, second));
    tokio::time::timeout(Duration::from_secs(1), async {
        while controller.is_resident() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let backend = controller.backend();
    let request = GenerationRequest {
        model: "fixture".into(),
        ..Default::default()
    };
    let next = backend
        .chat_completion_stream(request, InferenceRequestContext::new())
        .await
        .unwrap();
    assert_eq!(loads.load(Ordering::Relaxed), 2);
    controller.pause();
    assert!(controller.acquire("fixture", None).await.is_err());
    assert!(controller.is_resident());
    drop(next);
    assert!(!controller.is_resident());
}

#[tokio::test]
async fn failed_load_returns_capacity_and_retry_can_admit() {
    let count = AtomicUsize::new(0);
    let controller = LocalResidency::start(
        ModelObject::new("fixture"),
        policy(),
        CapacityBudget::new(2),
        2,
        None,
        Arc::new(move || {
            ensure!(count.fetch_add(1, Ordering::Relaxed) > 0, "retryable load");
            Ok(resident())
        }),
        None,
    )
    .await
    .unwrap();
    assert!(controller.acquire("fixture", None).await.is_err());
    assert!(controller.acquire("fixture", None).await.is_ok());
}

#[tokio::test]
async fn timeout_does_not_publish_a_late_native_result() {
    let (release, released) = std::sync::mpsc::channel();
    let released = Mutex::new(released);
    let budget = CapacityBudget::new(2);
    let controller = LocalResidency::start(
        ModelObject::new("fixture"),
        ResidencyPolicy {
            load_timeout: Duration::from_millis(30),
            ..policy()
        },
        budget.clone(),
        2,
        None,
        Arc::new(move || {
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(2))?;
            Ok(resident())
        }),
        None,
    )
    .await
    .unwrap();
    let result = controller.acquire("fixture", None).await;
    assert!(matches!(result, Err(error) if error.message().contains("timed out")));
    assert!(!controller.is_resident());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while controller.0.state().attempt.is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!controller.is_resident());
    assert!(budget.reserve(2).is_ok());
}
