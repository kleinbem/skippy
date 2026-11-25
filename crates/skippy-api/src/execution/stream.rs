use std::future::poll_fn;

use skippy_inference_api::{CancellationToken, generation::GenerationEventStream};

use super::{ExecutionError, ExecutionErrorKind, GenerationEvent};

/// A single generation. Dropping it signals cooperative cancellation.
pub struct GenerationStream {
    stream: Option<GenerationEventStream>,
    cancellation: CancellationToken,
}

impl GenerationStream {
    pub(super) fn new(stream: GenerationEventStream, cancellation: CancellationToken) -> Self {
        Self {
            stream: Some(stream),
            cancellation,
        }
    }

    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub async fn next(&mut self) -> Option<Result<GenerationEvent, ExecutionError>> {
        let stream = self.stream.as_mut()?;
        let result = tokio::select! {
            biased;
            _ = self.cancellation.cancelled() => Some(Err(cancelled())),
            event = poll_fn(|cx| stream.as_mut().poll_next(cx)) => {
                event.map(|event| event.map_err(ExecutionError::inference))
            }
        };
        if !matches!(result, Some(Ok(_))) {
            self.stream.take();
        }
        result
    }
}

impl Drop for GenerationStream {
    fn drop(&mut self) {
        if self.stream.is_some() {
            self.cancel();
        }
    }
}

/// Signals cancellation if request startup is abandoned before a stream exists.
pub(super) struct CancelStartup(pub(super) Option<CancellationToken>);

impl Drop for CancelStartup {
    fn drop(&mut self) {
        if let Some(token) = &self.0 {
            token.cancel();
        }
    }
}

fn cancelled() -> ExecutionError {
    ExecutionError::new(
        ExecutionErrorKind::Cancelled,
        anyhow::anyhow!("generation cancelled"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_is_typed_and_terminal_even_if_backend_has_no_events() {
        let mut stream = GenerationStream::new(
            Box::pin(futures_util::stream::pending()),
            CancellationToken::new(),
        );
        stream.cancel();
        assert_eq!(
            stream.next().await.unwrap().unwrap_err().kind(),
            ExecutionErrorKind::Cancelled
        );
        assert!(stream.next().await.is_none());
    }
}
