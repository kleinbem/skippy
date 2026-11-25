use super::*;
use skippy_inference_api::{
    Embedding, EmbeddingResponse, EmbeddingsRequest,
    generation::{GenerationEventStream, GenerationRequest, GenerationResponse},
};

struct FakeModel(f32);

#[async_trait::async_trait]
impl InferenceBackend for FakeModel {
    async fn models(&self) -> InferenceResult<Vec<ModelObject>> {
        Ok(vec![])
    }
    async fn chat_completion(&self, _: GenerationRequest) -> InferenceResult<GenerationResponse> {
        Err(InferenceError::unsupported("fake embeddings model"))
    }
    async fn chat_completion_stream(
        &self,
        _: GenerationRequest,
        _: InferenceRequestContext,
    ) -> InferenceResult<GenerationEventStream> {
        Err(InferenceError::unsupported("fake embeddings model"))
    }
    async fn embeddings(
        &self,
        request: EmbeddingsRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<EmbeddingResponse> {
        if context.is_cancelled() {
            return Err(InferenceError::cancelled("cancelled"));
        }
        Ok(EmbeddingResponse::from_embeddings(
            request.model,
            vec![Embedding {
                values: vec![self.0],
                index: 0,
            }],
            1,
            &request.encoding_format,
        ))
    }
}

fn entries() -> Vec<(ModelObject, BackendLease)> {
    [("one", 1.0), ("two", 2.0)]
        .into_iter()
        .map(|(id, value)| {
            (
                ModelObject::new(id),
                BackendLease::new(Arc::new(FakeModel(value)), Arc::new(())),
            )
        })
        .collect()
}

#[tokio::test]
async fn independent_models_share_a_listener_and_embedder() {
    let server = MultiModelServer::from_backends(entries(), None).unwrap();
    let request =
        serde_json::from_value(serde_json::json!({"model":"two", "input":"fixture"})).unwrap();
    let backend = server.backend();
    let context = InferenceRequestContext::new();
    assert!(backend.embeddings(request, context.clone()).await.is_ok());
    context.cancel();
    let request =
        serde_json::from_value(serde_json::json!({"model":"one", "input":"fixture"})).unwrap();
    assert!(backend.embeddings(request, context).await.is_err());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(server.serve(listener, async move {
        let _ = stopped.await;
    }));
    let client = reqwest::Client::new();
    let models: serde_json::Value = client
        .get(format!("http://{address}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"][0]["id"], "one");
    assert_eq!(models["data"][1]["id"], "two");
    for (id, value) in [("one", 1.0), ("two", 2.0)] {
        let response: serde_json::Value = client
            .post(format!("http://{address}/v1/embeddings"))
            .json(&serde_json::json!({"model":id,"input":"fixture"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(response["data"][0]["embedding"][0].as_f64().unwrap(), value);
    }
    let response = client
        .post(format!("http://{address}/v1/embeddings"))
        .json(&serde_json::json!({"model":"Two","input":"fixture"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
}
