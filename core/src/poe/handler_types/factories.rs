//! POE factory types

use std::sync::Arc;

use crate::poe::{CompositeValidator, Worker};

/// Factory function type for creating workers
pub type WorkerFactory<W> = Arc<dyn Fn() -> W + Send + Sync>;

/// Factory function type for creating validators
pub type ValidatorFactory = Arc<dyn Fn() -> CompositeValidator + Send + Sync>;
