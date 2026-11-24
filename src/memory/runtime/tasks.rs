use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::sync_primitives::Mutex;
use tokio::task::{JoinError, JoinHandle};

#[cfg(test)]
use tokio::task::Id;

#[derive(Default)]
pub(super) struct MemoryTasks {
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl MemoryTasks {
    pub(super) fn retain(&self, handle: JoinHandle<()>) {
        self.handles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(handle);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.handles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    }

    pub(super) fn poll_completed(&self, cx: &mut Context<'_>) -> Poll<Result<(), JoinError>> {
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let completed = handles
            .iter_mut()
            .enumerate()
            .find_map(|(index, handle)| match Pin::new(handle).poll(cx) {
                Poll::Ready(result) => Some((index, result)),
                Poll::Pending => None,
            });
        match completed {
            Some((index, result)) => {
                // Only an observed handle is removed; pending handles stay in place.
                drop(handles.swap_remove(index));
                Poll::Ready(result)
            }
            None => Poll::Pending,
        }
    }

    #[cfg(test)]
    pub(super) fn ids(&self) -> Vec<Id> {
        self.handles
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .map(JoinHandle::id)
            .collect()
    }
}
