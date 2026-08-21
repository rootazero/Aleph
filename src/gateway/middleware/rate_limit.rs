//! Rate limit middleware for the gateway JSON-RPC handler pipeline.

use crate::sync_primitives::Arc;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tower::{Layer, Service};
use tracing::warn;

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse};
use crate::gateway::rate_limiter::{scope_for_method, RateLimitKey, RateLimitScope, RateLimiter};

#[derive(Clone)]
pub struct RateLimitLayer {
    limiter: Arc<RateLimiter>,
}

impl RateLimitLayer {
    #[must_use]
    pub const fn new(limiter: Arc<RateLimiter>) -> Self {
        Self { limiter }
    }
}

impl<S> Layer<S> for RateLimitLayer {
    type Service = RateLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RateLimitService {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

#[derive(Clone)]
pub struct RateLimitService<S> {
    inner: S,
    limiter: Arc<RateLimiter>,
}

impl<S> RateLimitService<S> {
    pub const fn new(inner: S, limiter: Arc<RateLimiter>) -> Self {
        Self { inner, limiter }
    }
}

/// The axis this layer rations on: **one window per authenticated principal**.
///
/// # Why the principal and not the connection
///
/// A rate limiter is a permission layer graded along some axis, and this file
/// used to grade along a constant: every remote caller was keyed on the literal
/// `"rpc"`. That is not "no isolation", it is *shared fate* — the defaults are
/// 30 `rpc_heavy`/min, 20 `rpc_write`/min and 100 `rpc_default`/min, and those
/// were the totals for **every remote principal combined**. The `rpc_heavy`
/// comment in `rate_limiter.rs` states that one live voice conversation issues
/// "easily 10-20 [chat.send]/min", so on a multi-user box two people talking
/// exhausted the server-wide window and every other member was answered
/// `RATE_LIMITED` until they stopped. Nobody had done anything wrong and no
/// setting could fix it.
///
/// The pooling was already known to be the wrong shape here — it is why the
/// `Auth` scope is remapped to the lockout-free default bucket a few lines
/// below, so one token-guessing client could not lock every remote Panel out of
/// the handshake. That fix treated the symptom on one arm; this is the axis.
///
/// # The three answers, in order
///
/// 1. **Loopback** keeps the literal `127.0.0.1`, which is what the limiter's
///    own `exempt_loopback` matches on. This arm is first *because*
///    `CALLER_USER` is `Some(OWNER_USER_ID)` for a loopback connection — reading
///    the principal first would key the desktop App's local Panel into a
///    rationed bucket and put its voice loop back on the air-time budget the
///    loopback exemption exists to remove. Single-user boxes are byte-identical.
/// 2. **An authenticated principal** gets its own window, namespaced so a
///    user id can never collide with the literals on either side of it. Two
///    devices of the same person share one window — that is the point: the
///    thing being rationed is a person's share of the server, not a socket's.
/// 3. **No principal** — a pre-login `connect`, or a walled connection — stays
///    pooled in `"rpc"`. There is no identity to ration yet, and inventing one
///    from something the caller supplies would be a bucket the caller chooses.
///    The WS dispatch loop's own per-client-IP check (`server/handler.rs`) is
///    the isolator for that traffic, and it is why this arm is safe to pool.
///
/// # What this layer does NOT claim to be
///
/// Two rate-limit layers now answer two different questions and both are
/// wanted: the dispatch loop bounds **one network origin**, this bounds **one
/// principal**. Neither is an aggregate server-load cap any more, and that is
/// deliberate — an aggregate cap is shared fate by construction, which is the
/// defect being removed. Growth of the key set is bounded by the principal
/// count plus these two literals, well under the limiter's `max_entries` floor.
fn rate_limit_identity() -> String {
    if crate::gateway::caller_identity::current_caller_is_loopback() {
        return "127.0.0.1".to_string();
    }
    match crate::gateway::caller_identity::current_caller_user() {
        Some(user) => format!("user:{user}"),
        None => "rpc".to_string(),
    }
}

impl<S> Service<JsonRpcRequest> for RateLimitService<S>
where
    S: Service<JsonRpcRequest, Response = JsonRpcResponse> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = JsonRpcResponse;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
        let limiter = self.limiter.clone();
        let inner = self.inner.clone();
        let mut inner_mut = std::mem::replace(&mut self.inner, inner);
        let method = req.method.clone();

        let scope = scope_for_method(&method);
        // `connect` maps to the strict Auth window (with lockout), which the
        // WS dispatch loop enforces per client IP upstream. This layer pools
        // every remote caller into the one shared "rpc" identity, so letting
        // the Auth lockout trip here would let a single token-guessing client
        // lock every remote Panel out of the handshake (cross-client DoS).
        // Pool handshakes into the lockout-free default bucket instead — it
        // still bounds aggregate dispatch load.
        let scope = if matches!(scope, RateLimitScope::Auth) {
            RateLimitScope::RpcDefault
        } else {
            scope
        };
        let identity = rate_limit_identity();
        let key = RateLimitKey::new(&identity, scope.clone());

        if let Err(e) = limiter.check_and_record(&key) {
            warn!(method = %method, scope = %scope, error = %e, "rate limited");
            // Real backoff hint (the old hardcoded 0 told clients to hammer).
            let retry_after_ms = e.retry_after_secs() * 1000;
            let response = JsonRpcResponse::error_with_data(
                req.id.clone(),
                crate::gateway::protocol::RATE_LIMITED,
                e.to_string(),
                serde_json::json!({"retry_after_ms": retry_after_ms}),
            );
            return Box::pin(async move { Ok(response) });
        }

        Box::pin(async move { inner_mut.call(req).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::caller_identity::{CALLER_IS_LOOPBACK, CALLER_USER};
    use crate::gateway::rate_limiter::RateLimitConfig;

    /// Terminal service that always succeeds.
    #[derive(Clone)]
    struct Ok200;
    impl Service<JsonRpcRequest> for Ok200 {
        type Response = JsonRpcResponse;
        type Error = std::convert::Infallible;
        type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
        fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: JsonRpcRequest) -> Self::Future {
            let resp = JsonRpcResponse::success(req.id, serde_json::json!({}));
            Box::pin(async move { Ok(resp) })
        }
    }

    fn tight_limiter() -> Arc<RateLimiter> {
        // rpc_heavy: 2 per second — exhausted on the 3rd chat.send.
        let mut config = RateLimitConfig::default();
        config.rpc_heavy.max_requests = 2;
        config.rpc_heavy.window_secs = 1;
        Arc::new(RateLimiter::new(config))
    }

    fn send_req() -> JsonRpcRequest {
        JsonRpcRequest::with_id("chat.send", None, serde_json::json!(1))
    }

    /// Scope the two task-locals the WS dispatch loop scopes around
    /// `process_request`, so a test call sees exactly what production sees.
    async fn as_caller<F: std::future::Future<Output = ()>>(
        loopback: bool,
        user: Option<&str>,
        body: F,
    ) {
        let user = user.map(str::to_string);
        CALLER_IS_LOOPBACK
            .scope(loopback, CALLER_USER.scope(user, body))
            .await;
    }

    /// Drive `chat.send` until the window refuses, and report how many got
    /// through. With `rpc_heavy = 2/s` a fresh principal answers 2.
    async fn admitted_sends(svc: &mut RateLimitService<Ok200>) -> usize {
        let mut n = 0;
        for _ in 0..4 {
            if svc.call(send_req()).await.unwrap().error.is_none() {
                n += 1;
            }
        }
        n
    }

    /// The defect this file's identity function exists to remove: with a
    /// constant identity, Alice talking exhausts the server-wide `rpc_heavy`
    /// window and Bob — who has sent nothing — is answered `RATE_LIMITED`.
    ///
    /// Mutation proof: make `rate_limit_identity` return a constant for the
    /// authenticated arm and this goes RED on Bob's first send.
    #[tokio::test]
    async fn two_principals_do_not_consume_each_others_window() {
        let limiter = tight_limiter();
        let mut svc = RateLimitService::new(Ok200, limiter);

        as_caller(false, Some("u-alice"), async {
            assert_eq!(admitted_sends(&mut svc).await, 2, "Alice owns 2 per window");
        })
        .await;

        as_caller(false, Some("u-bob"), async {
            assert_eq!(
                admitted_sends(&mut svc).await,
                2,
                "Bob has sent nothing; Alice's traffic must not spend his window"
            );
        })
        .await;
    }

    /// The other half of the axis: a person is rationed, not a socket. Two
    /// connections of the same principal (phone + laptop) share one window,
    /// otherwise the ceiling would be "30/min times however many devices you
    /// pair", which is a ceiling anyone can raise.
    #[tokio::test]
    async fn one_principal_shares_a_single_window_across_its_connections() {
        let mut svc = RateLimitService::new(Ok200, tight_limiter());

        as_caller(false, Some("u-alice"), async {
            assert_eq!(admitted_sends(&mut svc).await, 2);
        })
        .await;

        as_caller(false, Some("u-alice"), async {
            assert_eq!(
                admitted_sends(&mut svc).await,
                0,
                "the same principal on a second connection meets the window it already spent"
            );
        })
        .await;
    }

    /// A pre-login `connect` has no principal to ration. It stays in the
    /// pooled bucket on purpose — the isolator for that traffic is the
    /// dispatch loop's per-client-IP check, not this layer.
    #[tokio::test]
    async fn an_unauthenticated_remote_caller_stays_pooled() {
        as_caller(false, None, async {
            assert_eq!(rate_limit_identity(), "rpc");
        })
        .await;
    }

    /// Arm order is load-bearing, and only this shape shows it: a loopback
    /// connection carries `CALLER_USER = Some(OWNER_USER_ID)`, so reading the
    /// principal first would key the desktop App's local Panel into a rationed
    /// bucket instead of the loopback exemption.
    #[tokio::test]
    async fn loopback_wins_over_the_owner_principal_it_also_carries() {
        as_caller(true, Some(crate::gateway::security::store::OWNER_USER_ID), async {
            assert_eq!(
                rate_limit_identity(),
                "127.0.0.1",
                "loopback must be answered before the principal it also carries"
            );
        })
        .await;
    }

    /// A principal id can never be confused with either literal this function
    /// also returns — checked here rather than trusted to the `u-` prefix,
    /// which is a property of today's id minting and not of this decision.
    #[tokio::test]
    async fn a_principal_key_cannot_collide_with_the_pooled_or_loopback_literals() {
        for hostile in ["rpc", "127.0.0.1"] {
            as_caller(false, Some(hostile), async {
                let id = rate_limit_identity();
                assert_ne!(id, "rpc");
                assert_ne!(id, "127.0.0.1");
            })
            .await;
        }
    }

    #[tokio::test]
    async fn loopback_caller_is_exempt_from_the_shared_bucket() {
        let mut svc = RateLimitService::new(Ok200, tight_limiter());
        // Scoped exactly like the WS dispatch loop scopes a local Panel.
        CALLER_IS_LOOPBACK
            .scope(true, async {
                for i in 0..20 {
                    let resp = svc.call(send_req()).await.unwrap();
                    assert!(
                        resp.error.is_none(),
                        "loopback chat.send {i} must not be rate limited"
                    );
                }
            })
            .await;
    }

    #[tokio::test]
    async fn remote_caller_still_hits_the_limit_with_a_real_retry_hint() {
        let mut svc = RateLimitService::new(Ok200, tight_limiter());
        CALLER_IS_LOOPBACK
            .scope(false, async {
                for _ in 0..2 {
                    let resp = svc.call(send_req()).await.unwrap();
                    assert!(resp.error.is_none());
                }
                let resp = svc.call(send_req()).await.unwrap();
                let err = resp.error.expect("3rd chat.send in 1s should be limited");
                let retry = err
                    .data
                    .as_ref()
                    .and_then(|d| d.get("retry_after_ms"))
                    .and_then(serde_json::Value::as_u64)
                    .expect("retry_after_ms present");
                assert!(
                    retry > 0,
                    "retry hint must be real, not the old hardcoded 0"
                );
            })
            .await;
    }

    #[tokio::test]
    async fn connect_uses_the_default_bucket_here_not_the_pooled_auth_lockout() {
        // A hair-trigger auth window: 1 attempt/min with a 5-min lockout. If
        // this layer applied the Auth scope to its shared "rpc" identity, the
        // 2nd remote connect would lock every remote caller out.
        let mut config = RateLimitConfig::default();
        config.auth.max_requests = 1;
        config.auth.window_secs = 60;
        config.auth.lockout_secs = Some(300);
        let mut svc = RateLimitService::new(Ok200, Arc::new(RateLimiter::new(config)));
        CALLER_IS_LOOPBACK
            .scope(false, async {
                for i in 0..5 {
                    let req = JsonRpcRequest::with_id("connect", None, serde_json::json!(i));
                    let resp = svc.call(req).await.unwrap();
                    assert!(
                        resp.error.is_none(),
                        "connect {i} must use the lockout-free default bucket here"
                    );
                }
            })
            .await;
    }

    #[tokio::test]
    async fn outside_any_scope_behaves_like_remote() {
        // Non-gateway callers (no task-local scope) keep the shared bucket.
        let mut svc = RateLimitService::new(Ok200, tight_limiter());
        for _ in 0..2 {
            let resp = svc.call(send_req()).await.unwrap();
            assert!(resp.error.is_none());
        }
        let resp = svc.call(send_req()).await.unwrap();
        assert!(resp.error.is_some(), "unscoped callers are not exempt");
    }
}
