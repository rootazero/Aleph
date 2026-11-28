use crate::a2a::domain::A2AError;

/// Result of an A2A operation.
pub type A2AResult<T> = Result<T, A2AError>;
