//! [`Disposer`] — one reversible side effect a plugin made on the running process.

use futures::future::BoxFuture;
use std::future::Future;

/// What a disposer reports. `Err` is a message for the log line
/// `EffectScope::dispose` writes with the step label attached; it never
/// stops the remaining disposers.
pub type DisposeOutcome = Result<(), String>;

/// One reversible side effect. Async because MCP server removal and service
/// stop are; a sync cleanup wraps itself with [`sync_disposer`].
pub type Disposer = Box<dyn FnOnce() -> BoxFuture<'static, DisposeOutcome> + Send>;

/// Wrap a synchronous cleanup as a [`Disposer`].
#[must_use]
pub fn sync_disposer(f: impl FnOnce() -> DisposeOutcome + Send + 'static) -> Disposer {
    Box::new(move || Box::pin(async move { f() }))
}

/// Wrap an async cleanup as a [`Disposer`].
#[must_use]
pub fn async_disposer<F, Fut>(f: F) -> Disposer
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = DisposeOutcome> + Send + 'static,
{
    Box::new(move || Box::pin(f()))
}
