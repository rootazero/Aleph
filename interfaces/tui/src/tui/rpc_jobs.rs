//! Bounded RPC work polled alongside terminal input. Futures borrow the
//! existing client; no second runtime or unbounded spawned task queue.

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::task::Poll;

use aleph_client::{AlephClient, CliResult};
use serde_json::Value;

pub(super) const MAX_RPC_JOBS: usize = 8;
pub(super) const EVENT_BATCH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RpcScope {
    pub session_key: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RpcTag {
    pub scope: RpcScope,
    pub method: &'static str,
    /// Request/tool identity, empty for session-wide snapshots.
    pub identity: String,
}

pub(super) struct RpcResult {
    pub tag: RpcTag,
    pub result: CliResult<Value>,
}

type Work<'a> = Pin<Box<dyn Future<Output = CliResult<Value>> + Send + 'a>>;

#[derive(Default)]
pub(super) struct RpcJobs<'a> {
    jobs: Vec<(RpcTag, Work<'a>)>,
}

impl<'a> RpcJobs<'a> {
    /// False means the caller keeps its due flag/draft and retries later.
    /// An identical request is never dispatched twice concurrently.
    pub fn enqueue(&mut self, client: &'a AlephClient, tag: RpcTag, params: Option<Value>) -> bool {
        let method = tag.method;
        self.enqueue_work(
            tag,
            Box::pin(async move { client.call(method, params).await }),
        )
    }

    /// Snapshots may combine several reads, while still occupying one bounded
    /// slot and returning through the same generation check as ordinary RPCs.
    pub fn enqueue_work(&mut self, tag: RpcTag, work: Work<'a>) -> bool {
        if self.jobs.len() >= MAX_RPC_JOBS || self.contains(&tag) {
            return false;
        }
        self.jobs.push((tag, work));
        true
    }

    pub fn contains(&self, tag: &RpcTag) -> bool {
        self.jobs.iter().any(|(pending, _)| pending == tag)
    }

    pub async fn next(&mut self) -> RpcResult {
        poll_fn(|cx| {
            for index in 0..self.jobs.len() {
                if let Poll::Ready(result) = self.jobs[index].1.as_mut().poll(cx) {
                    let (tag, _) = self.jobs.swap_remove(index);
                    return Poll::Ready(RpcResult { tag, result });
                }
            }
            // Empty is deliberately Pending: the terminal/tick branch will
            // wake the enclosing select when it can schedule more work.
            Poll::Pending
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    fn tag(identity: &str) -> RpcTag {
        RpcTag {
            scope: RpcScope {
                session_key: "s".into(),
                generation: 3,
            },
            method: "test",
            identity: identity.into(),
        }
    }

    #[test]
    fn queue_is_bounded_and_duplicate_identity_is_not_dispatched_twice() {
        let mut jobs = RpcJobs::default();
        assert!(jobs.enqueue_work(tag("one"), Box::pin(std::future::pending())));
        assert!(!jobs.enqueue_work(tag("one"), Box::pin(std::future::pending())));
        for id in 1..MAX_RPC_JOBS {
            assert!(jobs.enqueue_work(tag(&id.to_string()), Box::pin(std::future::pending())));
        }
        assert!(!jobs.enqueue_work(tag("overflow"), Box::pin(std::future::pending())));
        assert_eq!(jobs.jobs.len(), MAX_RPC_JOBS);
    }

    #[tokio::test]
    async fn ready_work_overtakes_slow_work_and_keeps_its_scope() {
        let (_tx, rx) = oneshot::channel::<()>();
        let mut jobs = RpcJobs::default();
        jobs.jobs.push((
            tag("slow"),
            Box::pin(async move {
                let _ = rx.await;
                Ok(Value::Null)
            }),
        ));
        jobs.jobs
            .push((tag("ready"), Box::pin(async { Ok(Value::Bool(true)) })));
        let result = jobs.next().await;
        assert_eq!(result.tag, tag("ready"));
        assert_eq!(result.result.unwrap(), Value::Bool(true));
        assert_eq!(jobs.jobs.len(), 1);
        assert!(jobs.contains(&tag("slow")));
    }

    #[tokio::test]
    async fn empty_queue_does_not_spin_and_a_pending_rpc_does_not_block_input() {
        let mut jobs = RpcJobs::default();
        let (tx, rx) = oneshot::channel();
        tx.send("input").unwrap();
        tokio::select! {
            _ = jobs.next() => panic!("empty jobs must stay pending"),
            input = rx => assert_eq!(input.unwrap(), "input"),
        }
    }
}
