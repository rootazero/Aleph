use std::pin::Pin;

use futures::Stream;

use crate::a2a::domain::*;

use super::task_manager::A2AResult;

/// Port for handling incoming A2A messages.
///
/// Bridges external A2A requests to the internal agent loop.
/// The synchronous variant returns the final task state;
/// the streaming variant returns a stream of incremental update events.
#[async_trait::async_trait]
pub trait A2AMessageHandler: Send + Sync {
    /// Handle a message synchronously — returns the task after processing completes
    async fn handle_message(
        &self,
        task_id: &str,
        message: A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<A2ATask>;

    /// Handle a message with streaming updates — returns a stream of events
    async fn handle_message_stream(
        &self,
        task_id: &str,
        message: A2AMessage,
        session_id: Option<&str>,
    ) -> A2AResult<Pin<Box<dyn Stream<Item = A2AResult<UpdateEvent>> + Send>>>;
}
