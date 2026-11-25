use std::{error::Error, fmt};

use skippy_inference_api::{InferenceError, errors::InferenceErrorKind};
use skippy_runtime::{NativeError, Status};

/// A semantic failure for an embedding operation. Hosts choose their retry policy.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionErrorKind {
    IncompatibleRuntime,
    UnsupportedCapability,
    InvalidOptions,
    InvalidPackage,
    InvalidAdmission,
    InsufficientResources,
    Cancelled,
    NotReady,
    ExecutionFailed,
}

#[derive(Debug)]
pub struct ExecutionError {
    kind: ExecutionErrorKind,
    cause: anyhow::Error,
}

impl ExecutionError {
    pub fn kind(&self) -> ExecutionErrorKind {
        self.kind
    }

    pub(super) fn new(kind: ExecutionErrorKind, cause: impl Into<anyhow::Error>) -> Self {
        Self {
            kind,
            cause: cause.into(),
        }
    }

    pub(super) fn native(cause: anyhow::Error, fallback: ExecutionErrorKind) -> Self {
        let kind = match NativeError::find(&cause).map(|error| error.status) {
            Some(Status::Unsupported) => ExecutionErrorKind::UnsupportedCapability,
            Some(Status::ModelError) => ExecutionErrorKind::InvalidPackage,
            _ if cause.chain().any(|error| {
                error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::OutOfMemory)
            }) =>
            {
                ExecutionErrorKind::InsufficientResources
            }
            _ => fallback,
        };
        Self::new(kind, cause)
    }

    pub(super) fn inference(error: InferenceError) -> Self {
        let kind = match error.kind() {
            InferenceErrorKind::InvalidRequest
            | InferenceErrorKind::PayloadTooLarge
            | InferenceErrorKind::ContextLengthExceeded => ExecutionErrorKind::InvalidOptions,
            InferenceErrorKind::RateLimit => ExecutionErrorKind::InsufficientResources,
            InferenceErrorKind::Cancelled => ExecutionErrorKind::Cancelled,
            InferenceErrorKind::UnsupportedFeature => ExecutionErrorKind::UnsupportedCapability,
            _ => ExecutionErrorKind::ExecutionFailed,
        };
        Self::new(kind, error)
    }
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:#}", self.cause)
    }
}

impl Error for ExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_the_semantic_error_and_original_diagnostics() {
        let source = InferenceError::unsupported("requested cache strategy is unavailable");
        let error = ExecutionError::inference(source);
        assert_eq!(error.kind(), ExecutionErrorKind::UnsupportedCapability);
        let cause = error
            .source()
            .unwrap()
            .downcast_ref::<InferenceError>()
            .unwrap();
        assert_eq!(cause.message(), "requested cache strategy is unavailable");
    }
}
