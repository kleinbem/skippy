//! Dispatch typed inference operations through a model-specific backend lease.

use std::{
    any::Any,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use async_trait::async_trait;
use futures_core::Stream;

use crate::{
    AudioResponse, AudioSpeechRequest, AudioTranscriptionRequest, AudioTranscriptionResponse,
    CompletionRequest, CompletionResponse, CompletionStream, EmbeddingResponse, EmbeddingsRequest,
    InferenceBackend, InferenceRequestContext, InferenceResult, ModelObject, RerankRequest,
    RerankResponse, SystemOneRequest, SystemOneResponse,
    generation::{GenerationEventStream, GenerationRequest, GenerationResponse},
    http_exchange::HttpExchangePolicy,
    realtime::{RealtimeTranscriptionConfig, RealtimeTranscriptionSession},
};

/// Keeps the backend and its owning runtime or admission permit alive through
/// the entire request, including consumption or drop of a streaming response.
#[derive(Clone)]
pub struct BackendLease {
    backend: Arc<dyn InferenceBackend>,
    _owner: Arc<dyn Any + Send + Sync>,
}

impl BackendLease {
    pub fn new(backend: Arc<dyn InferenceBackend>, owner: Arc<dyn Any + Send + Sync>) -> Self {
        Self {
            backend,
            _owner: owner,
        }
    }

    pub fn backend(&self) -> &Arc<dyn InferenceBackend> {
        &self.backend
    }
}

pub trait ModelLookup: Send + Sync + 'static {
    fn ready_models(&self) -> impl Future<Output = InferenceResult<Vec<ModelObject>>> + Send;

    fn acquire(
        &self,
        id: &str,
        context: Option<&InferenceRequestContext>,
    ) -> impl Future<Output = InferenceResult<BackendLease>> + Send;

    /// A policy applying to the public HTTP listener, before model dispatch.
    fn http_exchange_policy(&self) -> Option<Arc<dyn HttpExchangePolicy>> {
        None
    }
}

pub struct ModelDispatcher<L>(pub L);

/// Serve a composed registry on a caller-owned listener.
pub async fn serve_models(
    listener: tokio::net::TcpListener,
    backend: Arc<dyn InferenceBackend>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, crate::router_for(backend))
        .with_graceful_shutdown(shutdown)
        .await
}

#[async_trait]
impl<L: ModelLookup> InferenceBackend for ModelDispatcher<L> {
    fn http_exchange_policy(&self) -> Option<Arc<dyn HttpExchangePolicy>> {
        self.0.http_exchange_policy()
    }

    async fn models(&self) -> InferenceResult<Vec<ModelObject>> {
        self.0.ready_models().await
    }

    async fn count_chat_tokens(&self, request: GenerationRequest) -> InferenceResult<u32> {
        let lease = self.0.acquire(&request.model, None).await?;
        lease.backend.count_chat_tokens(request).await
    }

    async fn chat_completion(
        &self,
        request: GenerationRequest,
    ) -> InferenceResult<GenerationResponse> {
        self.chat_completion_with_context(request, InferenceRequestContext::new())
            .await
    }

    async fn chat_completion_with_context(
        &self,
        request: GenerationRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<GenerationResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease
            .backend
            .chat_completion_with_context(request, context)
            .await
    }

    async fn chat_completion_stream(
        &self,
        request: GenerationRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<GenerationEventStream> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        let stream = lease
            .backend
            .chat_completion_stream(request, context)
            .await?;
        Ok(retain_stream(stream, lease))
    }

    async fn completion(&self, request: CompletionRequest) -> InferenceResult<CompletionResponse> {
        self.completion_with_context(request, InferenceRequestContext::new())
            .await
    }

    async fn completion_with_context(
        &self,
        request: CompletionRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<CompletionResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease
            .backend
            .completion_with_context(request, context)
            .await
    }

    async fn completion_stream(
        &self,
        request: CompletionRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<CompletionStream> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        let stream = lease.backend.completion_stream(request, context).await?;
        Ok(retain_stream(stream, lease))
    }

    async fn system_one(&self, request: SystemOneRequest) -> InferenceResult<SystemOneResponse> {
        let lease = self.0.acquire(&request.model, None).await?;
        lease.backend.system_one(request).await
    }

    async fn embeddings(
        &self,
        request: EmbeddingsRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<EmbeddingResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease.backend.embeddings(request, context).await
    }

    async fn rerank(
        &self,
        request: RerankRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<RerankResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease.backend.rerank(request, context).await
    }

    async fn audio_speech(
        &self,
        request: AudioSpeechRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<AudioResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease.backend.audio_speech(request, context).await
    }

    async fn audio_transcription(
        &self,
        request: AudioTranscriptionRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<AudioTranscriptionResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease.backend.audio_transcription(request, context).await
    }

    async fn audio_translation(
        &self,
        request: AudioTranscriptionRequest,
        context: InferenceRequestContext,
    ) -> InferenceResult<AudioTranscriptionResponse> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        lease.backend.audio_translation(request, context).await
    }

    async fn realtime_transcription(
        &self,
        request: RealtimeTranscriptionConfig,
        context: InferenceRequestContext,
    ) -> InferenceResult<RealtimeTranscriptionSession> {
        let lease = self.0.acquire(&request.model, Some(&context)).await?;
        let session = lease
            .backend
            .realtime_transcription(request, context)
            .await?;
        Ok(retain_realtime(session, lease))
    }
}

fn retain_stream<T: 'static>(
    stream: Pin<Box<dyn Stream<Item = T> + Send>>,
    lease: BackendLease,
) -> Pin<Box<dyn Stream<Item = T> + Send>> {
    Box::pin(RetainedStream {
        stream: Some(stream),
        lease: Some(lease),
    })
}

struct RetainedStream<T> {
    stream: Option<Pin<Box<dyn Stream<Item = T> + Send>>>,
    lease: Option<BackendLease>,
}

impl<T> Stream for RetainedStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<T>> {
        let Some(stream) = self.stream.as_mut() else {
            return Poll::Ready(None);
        };
        let item = stream.as_mut().poll_next(context);
        if matches!(item, Poll::Ready(None)) {
            self.stream = None;
            self.lease = None;
        }
        item
    }
}

fn retain_realtime(
    session: RealtimeTranscriptionSession,
    lease: BackendLease,
) -> RealtimeTranscriptionSession {
    let RealtimeTranscriptionSession { input, mut output } = session;
    let (sender, receiver) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        let _lease = lease;
        loop {
            let item = tokio::select! {
                () = sender.closed() => break,
                item = output.recv() => item,
            };
            let Some(item) = item else { break };
            if sender.send(item).await.is_err() {
                break;
            }
        }
    });
    RealtimeTranscriptionSession {
        input,
        output: receiver,
    }
}
