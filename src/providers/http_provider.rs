//! Generic HTTP-based AI provider
//!
//! Uses a `ProtocolAdapter` for protocol-specific logic.

use crate::config::ProviderConfig;
use crate::error::Result;
use crate::providers::adapter::{
    ProtocolAdapter, ProviderResponse, RequestPayload, StopReason, TokenUsage,
};
use crate::providers::message::{normalize_tool_pairs, ContentBlock, UnifiedMessage};
use crate::providers::{AiProvider, ProviderDelta};
use crate::secrets::leak_detector::{LeakDecision, LeakDetector};
use crate::sync_primitives::Arc;
use futures::StreamExt;
use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use tracing::debug;

/// On a provider HTTP rejection, log the outgoing request body (truncated) so a
/// request-shape bug can be pinned — most importantly a *cross-protocol*
/// failover conversion that an OpenAI-compatible endpoint rejects with a vendor
/// error like 302.ai's `-10003 "bad parameter"`, which cannot be isolated from the
/// converter source alone (black-box probing of the live endpoint accepted every
/// isolated shape). Scoped to [`AlephError::ProviderError`] (generic 4xx/5xx);
/// rate-limit and timeout errors carry no request-shape signal and are skipped
/// to keep logs clean. The body holds prompt content but no secrets — the API
/// key rides in the `Authorization` header, never the body — and is truncated to
/// bound log volume.
fn log_rejected_request_body(
    provider: &str,
    err: &crate::error::AlephError,
    diag: Option<reqwest::RequestBuilder>,
) {
    if !matches!(err, crate::error::AlephError::ProviderError { .. }) {
        return;
    }
    let Some(body) = diag
        .and_then(|rb| rb.build().ok())
        .as_ref()
        .and_then(|req| req.body())
        .and_then(|b| b.as_bytes())
        .map(|bytes| {
            String::from_utf8_lossy(bytes)
                .chars()
                .take(4000)
                .collect::<String>()
        })
    else {
        return;
    };
    tracing::warn!(
        provider = %provider,
        error = %err,
        request_body = %body,
        "provider rejected the request (HTTP error); logging the outgoing body (truncated) so a \
         request-shape / cross-protocol conversion bug can be diagnosed",
    );
}

/// True when a provider error reports a rejected encrypted reasoning replay.
///
/// The `OpenAI` Responses API rejects a replayed `reasoning` input item whose
/// `encrypted_content` blob was minted by a different endpoint/model (or has
/// expired) with HTTP 400 and the structured error code
/// `invalid_encrypted_content`. Scoped to [`AlephError::ProviderError`] so
/// unrelated error kinds (auth, rate limit, timeouts) never trigger the
/// strip-and-retry recovery.
///
/// The previous implementation substring-matched `encrypted_content` anywhere
/// in the message — that fired on any provider error mentioning the field
/// name (including unrelated request-log lines, documentation-style errors,
/// or third-party proxies that wrap the upstream 400 in their own envelope).
/// Once matched, [`strip_thinking_signatures`] stripped the signature from
/// *every* Thinking block in the entire history, not just the offending
/// blob's, breaking Anthropic's extended-thinking replay contract. We now
/// require the more specific `invalid_encrypted_content` token (the OpenAI
/// structured code) so an unrelated mention does not burn the recovery
/// path.
fn is_stale_encrypted_reasoning_error(err: &crate::error::AlephError) -> bool {
    let crate::error::AlephError::ProviderError { message, .. } = err else {
        return false;
    };
    let lower = message.to_ascii_lowercase();
    lower.contains("invalid_encrypted_content")
}

/// Rebuild the message list with every thinking signature dropped.
///
/// Returns `None` when no message carries a signature — the caller then knows
/// the encrypted-content error cannot be about this conversation and skips the
/// retry. Thinking *text* is preserved; only the opaque replay verifier (the
/// NDJSON `{"id","ec"}` blob for `OpenAI` Responses, the signed-block verifier
/// for Anthropic) is removed, so the retried request simply omits the
/// reasoning replay items.
fn strip_thinking_signatures(messages: &[UnifiedMessage]) -> Option<Vec<UnifiedMessage>> {
    let has_signature = messages.iter().any(|m| {
        matches!(m, UnifiedMessage::Assistant { content } if content.iter().any(
            |b| matches!(b, ContentBlock::Thinking { signature: Some(_), .. })
        ))
    });
    if !has_signature {
        return None;
    }
    Some(
        messages
            .iter()
            .map(|m| match m {
                UnifiedMessage::Assistant { content } => UnifiedMessage::Assistant {
                    content: content
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Thinking { thinking, .. } => ContentBlock::Thinking {
                                // rust-doctor-disable-next-line excessive-clone
                                thinking: thinking.clone(),
                                signature: None,
                            },
                            // rust-doctor-disable-next-line excessive-clone
                            other => other.clone(),
                        })
                        .collect(),
                },
                // rust-doctor-disable-next-line excessive-clone
                other => other.clone(),
            })
            .collect(),
    )
}

/// Generic HTTP-based AI provider
///
/// This provider uses a `ProtocolAdapter` for protocol-specific request/response handling.
/// It implements the `AiProvider` trait by delegating to the adapter.
pub struct HttpProvider {
    name: String,
    config: ProviderConfig,
    adapter: Arc<dyn ProtocolAdapter>,
}

impl std::fmt::Debug for HttpProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpProvider")
            .field("name", &self.name)
            .field("protocol", &self.adapter.name())
            .finish_non_exhaustive()
    }
}

/// The stream's *terminal* frames, read while the deltas go past.
///
/// [`DeltaCollector`](crate::providers::DeltaCollector) folds both facts away:
/// it drops `ProviderDelta::Error` outright, and it reports
/// `StopReason::EndTurn` both for a `Done(EndTurn)` the provider actually
/// stated and for a stream that produced no `Done` at all. After `finish()`
/// neither is recoverable, so they are captured here — the one place that can
/// still see them — instead of being asserted in prose at the use site.
#[derive(Default)]
struct TerminalFrames {
    /// The first fault reported on the stream. Later ones are the same
    /// incident told twice; the first is the one with a cause in it.
    fault: Option<String>,
    /// The fault was the last terminal frame — no `Done` followed it. No
    /// adapter stops parsing on an error frame (`anthropic/sse.rs` and
    /// `openai_chat/sse.rs` both push the delta and keep consuming), so a relay
    /// that emits a non-fatal `{"error": ...}` chunk and then finishes the turn
    /// normally ends `Error -> ... -> Done`, and that turn did complete.
    ///
    /// This reads the order the deltas are *delivered* in, which is the wire
    /// order only because no adapter is allowed to buffer a `Done` past a
    /// fault. One adapter does buffer: `openai_chat` holds the terminal `Done`
    /// back until the trailing `include_usage` chunk lands, and so releases it
    /// *ahead* of a queued `Error` rather than after it
    /// (`openai_chat::sse::defer_done_until_usage`) — buffering that outlived a
    /// fault would hand this field the opposite verdict for the same wire.
    fault_is_last: bool,
    /// A `Done` frame arrived at all, so the collector's stop reason is one the
    /// provider stated rather than its `EndTurn` default.
    stop_stated: bool,
}

/// What a reported fault means for the attempt. Derived once by
/// [`TerminalFrames::classify`] and applied once by [`apply_fault`], which
/// matches every variant exhaustively.
enum StreamFault<'a> {
    /// Nothing usable came through: the fault is the whole answer, so it
    /// becomes a hard `Err` and the retry/failover path classifies it as usual.
    Fatal(&'a str),
    /// A `Done` frame followed the fault: the error chunk was advisory inside
    /// a stream that then completed. The provider answered, so there is nothing
    /// to charge and no stop reason to doubt — but it is logged, because a
    /// provider doing this on every turn is worth an operator's attention.
    Advisory(&'a str),
    /// Content was emitted *and* the fault was how the stream ended. The
    /// partial answer is still returned — the user has already seen it and no
    /// later candidate can un-show it — but the fault rides out on
    /// [`ProviderResponse::provider_error`] for the failover walk to charge a
    /// circuit strike with.
    Charged {
        message: &'a str,
        /// No `Done` frame ever arrived, so the response's `EndTurn` is the
        /// collector's default, not a claim the provider made. Downgrading
        /// *that* to `Unknown` stops a faulted turn from reading as a natural
        /// completion; a stated stop reason is a fact and is kept.
        stop_reason_is_default: bool,
    },
}

impl TerminalFrames {
    fn observe(&mut self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::Error(msg) => {
                if self.fault.is_none() {
                    // rust-doctor-disable-next-line excessive-clone
                    self.fault = Some(msg.clone());
                }
                self.fault_is_last = true;
            }
            ProviderDelta::Done(_) => {
                self.stop_stated = true;
                self.fault_is_last = false;
            }
            _ => {}
        }
    }

    /// `None` when the stream reported no fault at all.
    fn classify(&self, has_content: bool) -> Option<StreamFault<'_>> {
        let message = self.fault.as_deref()?;
        if !has_content {
            // A fault with nothing to show for it is the whole answer,
            // whatever frame order produced it.
            return Some(StreamFault::Fatal(message));
        }
        if !self.fault_is_last {
            return Some(StreamFault::Advisory(message));
        }
        Some(StreamFault::Charged {
            message,
            stop_reason_is_default: !self.stop_stated,
        })
    }
}

/// Apply a fault verdict to the assembled response.
///
/// Split out of [`HttpProvider::execute_once`] so the *effects* of each verdict
/// can be asserted on their own: "the derivation picked `Charged`" is a
/// different claim from "the fault reached
/// [`ProviderResponse::provider_error`]", and the walk only ever reads the
/// second one. Every verdict is spelled out, so a new one is a compile error
/// rather than a silent no-op.
fn apply_fault(
    provider: &str,
    response: &mut ProviderResponse,
    fault: Option<StreamFault<'_>>,
) -> Result<()> {
    match fault {
        None => {}
        Some(StreamFault::Fatal(message)) => {
            return Err(crate::error::AlephError::provider(message.to_string()));
        }
        Some(StreamFault::Advisory(message)) => {
            tracing::warn!(
                provider = %provider,
                error = %message,
                "Provider reported a fault mid-stream but then completed the turn \
                 — keeping it as a success"
            );
        }
        Some(StreamFault::Charged {
            message,
            stop_reason_is_default,
        }) => {
            tracing::warn!(
                provider = %provider,
                error = %message,
                "Provider reported a fault after emitting content — returning the \
                 partial answer and recording the attempt as a failure"
            );
            if stop_reason_is_default {
                response.stop_reason = StopReason::Unknown;
            }
            response.provider_error = Some(message.to_string());
        }
    }
    Ok(())
}

impl HttpProvider {
    /// Create a new `HttpProvider` with the given adapter
    pub fn new(
        name: String,
        config: ProviderConfig,
        adapter: Arc<dyn ProtocolAdapter>,
    ) -> Result<Self> {
        debug!(
            name = %name,
            protocol = adapter.name(),
            model = %config.default_model(),
            "Creating HttpProvider"
        );

        Ok(Self {
            name,
            config,
            adapter,
        })
    }

    /// Apply outbound safety checks (PII filtering + secret leak detection).
    /// Returns filtered messages or a leak block reason.
    fn apply_outbound_safety(
        &self,
        messages: &[UnifiedMessage],
        platform: Option<&str>,
    ) -> std::result::Result<Vec<UnifiedMessage>, String> {
        let mut filtered_messages: Vec<UnifiedMessage> = messages.to_vec();

        // PII filtering: filter each text block individually. The per-platform
        // exclusion/override table (`[platform_policies.X]`) is consulted too:
        // the `runtime_guard` guardrail pipeline already applied it on the way
        // in, so re-filtering here with only the global `is_provider_excluded`
        // would silently re-redact a payload the operator excluded for this
        // platform (and would ignore per-platform action overrides like
        // `platform_policies.X.phone = warn`). `platform` is read from
        // `payload.metadata["platform"]` — stamped by the inbound router and
        // the Panel run path.
        if let Some(engine_lock) = crate::pii::PiiEngine::global() {
            // Poison convention: recover the inner engine instead of silently
            // skipping the PII filter when the lock is poisoned.
            let engine = engine_lock.read().unwrap_or_else(|e| e.into_inner());
            if !engine.is_platform_excluded(platform, &self.name) {
                for msg in &mut filtered_messages {
                    for block in msg.content_blocks_mut() {
                        if let ContentBlock::Text { ref mut text, .. } = block {
                            let result = engine.filter_with_platform(text, platform);
                            if result.has_detections() {
                                *text = result.text;
                            }
                        }
                    }
                }
            }
        }

        // Secret leak detection: scan all text content.
        //
        // The `LeakDetector::new()` here intentionally has NO `register_injected`
        // call — the InjectedSecret fingerprints are owned by
        // `RuntimeSecurityGuard::secret_leak_detector`, which scans the same
        // outbound payload before this provider is ever reached. The local scan
        // is a defense-in-depth backstop for the *static* vendor-regex matches
        // (`LEAK_PATTERNS` in `vendor_patterns.rs`) — those work without any
        // registration. Fingerprint-based detection of a freshly-injected,
        // non-vendor-shaped secret is the runtime_guard's responsibility, and
        // duplicating the fingerprint set here would split the state between
        // two detectors and silently weaken both.
        let detector = LeakDetector::new();
        let all_text = UnifiedMessage::extract_all_text(&filtered_messages);
        if let LeakDecision::Block { reason, .. } = detector.scan_outbound(&all_text) {
            return Err(reason);
        }

        Ok(filtered_messages)
    }

    /// Execute a request, collecting the SSE delta stream into a complete
    /// [`ProviderResponse`].
    ///
    /// When `sink` is `Some`, each [`ProviderDelta`] is also forwarded to the
    /// observer as it arrives — this is the seam the harness uses to surface
    /// live token deltas without bypassing any of the post-collection pipeline
    /// (cost-metering hooks, provider-error promotion, truncation diagnostics,
    /// `validate`, inbound secret-leak detection) that the non-streaming path
    /// relies on. With `sink = None` the behaviour is byte-identical to before.
    ///
    /// Recovery: when the provider rejects a replayed encrypted reasoning item
    /// (`OpenAI` Responses `invalid_encrypted_content` — the blob was minted by a
    /// different endpoint/model, or expired), the request is retried exactly
    /// once with all thinking signatures stripped. Without this, the stale blob
    /// in session history fails the turn on every attempt — the retry layer
    /// above correctly classifies the 400 as fatal (the *same* payload can
    /// never succeed), so the only layer that can recover is this one, which
    /// can rewrite the payload. Mirrors openclaw's encrypted-content retry.
    async fn execute(
        &self,
        payload: RequestPayload<'_>,
        sink: Option<&dyn crate::providers::DeltaSink>,
    ) -> Result<ProviderResponse> {
        let platform = payload
            .metadata
            .as_ref()
            .and_then(|m| m.get("platform"))
            .map(String::as_str);
        let mut filtered_messages = match self.apply_outbound_safety(payload.messages, platform) {
            Ok(msgs) => msgs,
            Err(reason) => {
                tracing::warn!(
                    provider = %self.name,
                    reason = %reason,
                    "Blocked outbound request: secret leak detected"
                );
                return Err(crate::error::AlephError::PermissionDenied {
                    message: format!("Secret leak blocked: {reason}"),
                    suggestion: Some("Remove secret values from the input before sending.".into()),
                });
            }
        };
        // The wire-level pairing repair: compaction, truncation, session-splits,
        // and interrupted turns can leave the history half-paired, and every
        // provider API rejects that. `normalize_tool_pairs` is the documented
        // single choke-point (`transform_messages`) — actually call it here, on
        // the owned copy, before the request is built. Idempotent; the retry
        // path below reuses the already-normalized list.
        normalize_tool_pairs(&mut filtered_messages);

        match self.execute_once(&filtered_messages, &payload, sink).await {
            Err(err) if is_stale_encrypted_reasoning_error(&err) => {
                match strip_thinking_signatures(&filtered_messages) {
                    Some(stripped) => {
                        tracing::warn!(
                            provider = %self.name,
                            error = %err,
                            "Provider rejected replayed encrypted reasoning — \
                             retrying once without thinking signatures"
                        );
                        self.execute_once(&stripped, &payload, sink).await
                    }
                    // No signature in the conversation — the error is about
                    // something else; don't burn a retry.
                    None => Err(err),
                }
            }
            other => other,
        }
    }

    /// One request/stream/collect attempt against the provider. Split out of
    /// [`HttpProvider::execute`] so the stale-encrypted-reasoning recovery can
    /// re-run the attempt with a rewritten message list.
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    async fn execute_once(
        &self,
        messages: &[UnifiedMessage],
        payload: &RequestPayload<'_>,
        sink: Option<&dyn crate::providers::DeltaSink>,
    ) -> Result<ProviderResponse> {
        let final_payload = RequestPayload {
            messages,
            system_prompt: payload.system_prompt,
            system_blocks: payload.system_blocks,
            tools: payload.tools,
            think_level: payload.think_level,
            temperature: payload.temperature,
            max_tokens: payload.max_tokens,
            // rust-doctor-disable-next-line excessive-clone
            tool_choice: payload.tool_choice.clone(),
            // rust-doctor-disable-next-line excessive-clone
            model: payload.model.clone(),
            // rust-doctor-disable-next-line excessive-clone
            metadata: payload.metadata.clone(),
        };

        // Extension hooks observe LLM provider traffic for cost metering.
        let session_id = hook_session_id(payload);
        let base_env = self.base_request_env(payload, false);
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PreApiRequest,
            &session_id,
            // rust-doctor-disable-next-line excessive-clone
            base_env.clone(),
        )
        .await;

        let request = self.adapter.build_request(&final_payload, &self.config)?;
        // Cheap clone of the outgoing request (the json body is `Bytes`, so
        // `try_clone` is an Arc bump, not a deep copy) so a provider HTTP
        // rejection can log the exact body that was rejected — the only way to
        // pin a cross-protocol conversion bug like 302.ai's -10003. Materialized
        // to a string only on the error path; the happy path drops it untouched.
        let diag_request = request.try_clone();
        // Time-to-first-byte watchdog. `request.send()` resolves only once the
        // upstream returns response headers; the streaming idle guard
        // (`wrap_idle_timeout`) only covers gaps *between* SSE events *after*
        // that. Without this, a provider that accepts the connection but stalls
        // before responding hangs the whole turn until the per-turn watchdog
        // kills the run — the operator-configured `[stability]
        // turn_timeout_secs`, which the library leaves unset and `aleph-server`
        // defaults to 120s (`build_stability_triple`). Either way that is far
        // too late to fail over. Reuse
        // `stream_idle_timeout_secs` (same "max gap with no upstream bytes"
        // semantics; `0` disables). On elapse, surface the typed `Timeout` that
        // the failover/retry path already classifies as transient, so the next
        // provider in the chain gets a turn.
        let ttfb_secs = crate::providers::protocols::stream_idle::effective_idle_secs(&self.config);
        let send_fut = request.send();
        let send_result = if ttfb_secs == 0 {
            send_fut.await
        } else {
            match tokio::time::timeout(std::time::Duration::from_secs(ttfb_secs), send_fut).await {
                Ok(res) => res,
                Err(_elapsed) => {
                    tracing::warn!(
                        provider = %self.name,
                        ttfb_secs,
                        "Provider produced no response headers within TTFB timeout — \
                         surfacing as transient error for failover"
                    );
                    return Err(crate::error::AlephError::Timeout {
                        suggestion: Some(format!(
                            "Provider '{}' sent no response for {ttfb_secs}s after the request \
                             was dispatched (time-to-first-byte timeout). The upstream may be \
                             unresponsive or throttling a large request; retry, switch \
                             providers, or raise ProviderConfig.stream_idle_timeout_secs.",
                            self.name
                        )),
                    });
                }
            }
        };
        let response = send_result.map_err(|e| {
            if e.is_timeout() {
                crate::error::AlephError::Timeout {
                    suggestion: Some("Request timed out. Try again or switch providers.".into()),
                }
            } else {
                crate::error::AlephError::network(e.to_string())
            }
        })?;

        // Collect streaming deltas into a ProviderResponse
        let stream = match self.adapter.stream_deltas(response).await {
            Ok(s) => s,
            Err(e) => {
                log_rejected_request_body(&self.name, &e, diag_request);
                return Err(e);
            }
        };
        let mut collector = crate::providers::DeltaCollector::new();
        // Read the stream's terminal frames on the way past — the collector
        // folds them away and `finish()` cannot give them back.
        let mut frames = TerminalFrames::default();
        futures::pin_mut!(stream);
        while let Some(delta) = stream.next().await {
            let delta = delta?;
            frames.observe(&delta);
            // Live observer (harness streaming): forward the delta before it is
            // folded into the collector. Cheap no-op when no sink is wired.
            if let Some(observer) = sink {
                observer.on_delta(&delta).await;
            }
            collector.push(delta);
        }
        let mut provider_response = collector.finish();

        // What a reported fault means for this attempt is derived once
        // (`TerminalFrames::classify`) and applied once (`apply_fault`).
        let has_content =
            provider_response.text.is_some() || !provider_response.tool_calls.is_empty();
        apply_fault(
            &self.name,
            &mut provider_response,
            frames.classify(has_content),
        )?;

        // A tool call whose streamed arguments were truncated mid-stream (the
        // upstream closed the body before the JSON finished) is unusable:
        // executing it with empty `{}` args surfaces as a misleading
        // "missing field" validation error, and the model — unable to fix an
        // infrastructure truncation — loops on it. Surface a transient error
        // (typed `Timeout`) with an honest diagnostic.
        //
        // It does NOT buy another provider, and that is not a branch here: a
        // truncated tool call implies its deltas were already forwarded to
        // `sink` in the loop above, so the walk's `EmissionGuard` has latched
        // and this error is always chain-terminal at this site. What it does
        // buy is the gateway's re-dispatch — the walk marks a post-emission
        // error `failover::PARTIAL_OUTPUT_EMITTED` only when *user-visible*
        // output was shown, and tool-call deltas are not that. So a truncation
        // with no text ahead of it gets a whole fresh attempt one layer up,
        // which is exactly the recovery a proxy-cut file write wants; a
        // truncation after the model had already written text does not, and
        // surfaces to the user instead of double-answering.
        if let Some(diag) = provider_response.truncated_tool_call {
            tracing::warn!(
                provider = %self.name,
                diagnostic = %diag,
                "Tool-call arguments truncated mid-stream — surfacing as transient error"
            );
            return Err(crate::error::AlephError::Timeout {
                suggestion: Some(format!(
                    "Tool-call arguments were truncated mid-stream ({diag}). The upstream \
                     likely closed the streaming response before the arguments finished — \
                     common when a large tool output (e.g. a big file write) crosses a \
                     proxy or idle timeout. Retry, switch providers, or write large files \
                     in smaller chunks."
                )),
            });
        }

        // Validate response
        provider_response.validate(self.adapter.name());

        // PostApiRequest fires once the response (and its token usage) is in
        // hand — before the inbound leak check, so the cost meter records the
        // request even when the response is later blocked.
        let mut post_env = base_env;
        if let Some(ref usage) = provider_response.usage {
            append_usage_env(&mut post_env, usage);
        }
        post_env.push((
            "STOP_REASON",
            format!("{:?}", provider_response.stop_reason),
        ));
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PostApiRequest,
            &session_id,
            post_env,
        )
        .await;

        // Secret leak detection: scan inbound response TEXT only.
        //
        // Same defense-in-depth pattern as the outbound scan above: the
        // fingerprint-registered detector lives in RuntimeSecurityGuard; this
        // local `LeakDetector::new()` only catches the static vendor-regex
        // matches. If a freshly-injected, non-vendor-shaped secret echoes
        // back, the runtime_guard pipeline is what catches it.
        let detector = LeakDetector::new();
        if let Some(ref text) = provider_response.text {
            if let LeakDecision::Block { reason, .. } = detector.scan_inbound(text) {
                tracing::warn!(
                    provider = %self.name,
                    reason = %reason,
                    "Blocked inbound response: secret leak detected"
                );
                return Err(crate::error::AlephError::PermissionDenied {
                    message: format!("Secret leak in response blocked: {reason}"),
                    suggestion: Some("The AI provider response contained a secret value.".into()),
                });
            }
        }

        Ok(provider_response)
    }

    /// Streaming variant of the non-streaming `process()` path: runs the exact
    /// same full pipeline as [`HttpProvider::execute`] (so cost metering,
    /// provider-error promotion, truncation handling, validation and inbound
    /// secret-leak detection all still apply to the assembled response) while
    /// forwarding each [`ProviderDelta`] to `sink` as it streams in.
    ///
    /// NOTE: the live deltas reach `sink` BEFORE the post-collection inbound
    /// leak scan runs, so a consumer that renders the live preview must treat
    /// the assembled `ProviderResponse` (or an `Err` from this call) as the
    /// authoritative, leak-checked result — same contract as `stream_raw`.
    pub async fn execute_streaming(
        &self,
        payload: RequestPayload<'_>,
        sink: &dyn crate::providers::DeltaSink,
    ) -> Result<ProviderResponse> {
        self.execute(payload, Some(sink)).await
    }

    /// Expose raw delta stream with outbound safety checks applied.
    ///
    /// Used by the OpenAI-compatible gateway passthrough
    /// (`gateway::openai_api::completions::passthrough`,
    /// `gateway::openai_api::responses`) to relay an upstream SSE stream
    /// verbatim. Inbound leak check is deferred to the `DeltaCollector`
    /// consumer.
    pub async fn stream_raw<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> anyhow::Result<
        futures::stream::BoxStream<'static, anyhow::Result<crate::providers::ProviderDelta>>,
    > {
        let platform = payload
            .metadata
            .as_ref()
            .and_then(|m| m.get("platform"))
            .map(String::as_str);
        let filtered_messages = self
            .apply_outbound_safety(payload.messages, platform)
            .map_err(|reason| anyhow::anyhow!("Secret leak blocked: {reason}"))?;

        let final_payload = RequestPayload {
            messages: &filtered_messages,
            system_prompt: payload.system_prompt,
            system_blocks: payload.system_blocks,
            tools: payload.tools,
            think_level: payload.think_level,
            temperature: payload.temperature,
            max_tokens: payload.max_tokens,
            // rust-doctor-disable-next-line excessive-clone
            tool_choice: payload.tool_choice.clone(),
            // rust-doctor-disable-next-line excessive-clone
            model: payload.model.clone(),
            // rust-doctor-disable-next-line excessive-clone
            metadata: payload.metadata.clone(),
        };

        // Extension hooks observe LLM provider traffic for cost metering.
        let session_id = hook_session_id(&payload);
        let base_env = self.base_request_env(&payload, true);
        crate::extension::hooks::fire_global_observer(
            crate::extension::HookEvent::PreApiRequest,
            &session_id,
            // rust-doctor-disable-next-line excessive-clone
            base_env.clone(),
        )
        .await;

        let request = self
            .adapter
            .build_request(&final_payload, &self.config)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        // Cheap clone for body-on-rejection diagnostics (see path above).
        let diag_request = request.try_clone();
        // TTFB watchdog — mirrors the `execute` path so a stalled upstream
        // produces a typed Timeout instead of hanging the turn.
        let ttfb_secs = crate::providers::protocols::stream_idle::effective_idle_secs(&self.config);
        let send_fut = request.send();
        let send_result = if ttfb_secs == 0 {
            send_fut.await
        } else {
            match tokio::time::timeout(std::time::Duration::from_secs(ttfb_secs), send_fut).await {
                Ok(res) => res,
                Err(_elapsed) => {
                    tracing::warn!(
                        provider = %self.name,
                        ttfb_secs,
                        "Provider produced no response headers within TTFB timeout (stream_raw)"
                    );
                    return Err(anyhow::anyhow!(
                        "Timeout: provider '{name}' sent no response for {ttfb_secs}s after \
                         the request was dispatched (time-to-first-byte timeout)",
                        name = self.name,
                    ));
                }
            }
        };
        let response = send_result.map_err(|e| anyhow::anyhow!("Network error: {e}"))?;
        let stream = match self.adapter.stream_deltas(response).await {
            Ok(s) => s,
            Err(e) => {
                log_rejected_request_body(&self.name, &e, diag_request);
                return Err(anyhow::anyhow!("{e}"));
            }
        };
        let inner = stream
            .map(|r| r.map_err(|e| anyhow::anyhow!("{e}")))
            .boxed();

        // Wrap the delta stream so `PostApiRequest` fires once — with the
        // accumulated token usage — when the stream completes naturally. An
        // aborted stream (client disconnect) skips the post hook.
        let wrapped = async_stream::stream! {
            let mut inner = inner;
            let mut usage: Option<TokenUsage> = None;
            let mut stop_reason: Option<StopReason> = None;
            while let Some(item) = inner.next().await {
                if let Ok(delta) = &item {
                    match delta {
                        ProviderDelta::Usage(u) => {
                            usage = Some(match usage.take() {
                                Some(prev) => {
                                    crate::providers::delta::merge_usage(prev, u.clone())
                                }
                                None => u.clone(),
                            });
                        }
                        ProviderDelta::Done(sr) => stop_reason = Some(sr.clone()),
                        _ => {}
                    }
                }
                yield item;
            }
            let mut env = base_env;
            if let Some(ref u) = usage {
                append_usage_env(&mut env, u);
            }
            if let Some(ref sr) = stop_reason {
                env.push(("STOP_REASON", format!("{sr:?}")));
            }
            crate::extension::hooks::fire_global_observer(
                crate::extension::HookEvent::PostApiRequest,
                &session_id,
                env,
            )
            .await;
        };

        Ok(wrapped.boxed())
    }

    /// Build the env shared by `PreApiRequest` / `PostApiRequest` hooks.
    fn base_request_env(
        &self,
        payload: &RequestPayload<'_>,
        streaming: bool,
    ) -> Vec<(&'static str, String)> {
        let model = payload
            .model
            .as_deref()
            .unwrap_or_else(|| self.config.default_model())
            .to_string();
        vec![
            ("PROVIDER_NAME", self.name.clone()),
            ("MODEL", model),
            ("PROTOCOL", self.adapter.name().to_string()),
            ("STREAMING", streaming.to_string()),
            ("MESSAGE_COUNT", payload.messages.len().to_string()),
        ]
    }
}

/// Resolve a session id for API-request hooks. Uses the `session_id` metadata
/// key when a caller threaded it through; otherwise a synthetic id so the cost
/// meter still aggregates by provider/model.
fn hook_session_id(payload: &RequestPayload<'_>) -> String {
    payload
        .metadata
        .as_ref()
        .and_then(|m| m.get("session_id"))
        .cloned()
        .unwrap_or_else(|| "provider".to_string())
}

/// Append `TokenUsage` figures to a `PostApiRequest` hook env.
fn append_usage_env(env: &mut Vec<(&'static str, String)>, usage: &TokenUsage) {
    env.push(("INPUT_TOKENS", usage.input_tokens.to_string()));
    env.push(("OUTPUT_TOKENS", usage.output_tokens.to_string()));
    if let Some(v) = usage.cache_read_tokens {
        env.push(("CACHE_READ_TOKENS", v.to_string()));
    }
    if let Some(v) = usage.cache_creation_tokens {
        env.push(("CACHE_CREATION_TOKENS", v.to_string()));
    }
    if let Some(v) = usage.thinking_tokens {
        env.push(("THINKING_TOKENS", v.to_string()));
    }
    if let Some(ref cost) = usage.cost {
        env.push(("COST_USD", format!("{:.6}", cost.calculate(usage))));
    }
}

impl AiProvider for HttpProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { self.execute(payload, None).await })
    }

    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: &'a dyn crate::providers::DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move { self.execute_streaming(payload, sink).await })
    }

    /// The one leaf that genuinely streams — every decorator's answer is a
    /// delegation that bottoms out here.
    fn supports_streaming(&self) -> bool {
        true
    }

    fn supports_native_tools(&self) -> bool {
        self.adapter.supports_native_tools()
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn color(&self) -> &str {
        &self.config.color
    }

    fn protocol(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.adapter.name())
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.config.model_behavior.as_deref().map(Cow::Borrowed)
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        crate::providers::model_behaviors::vendor_identity(
            self.config.base_url.as_deref(),
            self.config.default_model(),
        )
        .map(Cow::Borrowed)
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        let model = self.config.default_model();
        if model.is_empty() {
            None
        } else {
            Some(Cow::Borrowed(model))
        }
    }

    /// The leaf of the decorator stack: this provider's configured key
    /// (`anthropic` / `deepseek` / `kimi` / …) — the id the pricing table and
    /// the model catalog are keyed on.
    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        if self.name.is_empty() {
            None
        } else {
            Some(Cow::Borrowed(self.name.as_str()))
        }
    }

    fn as_http_provider(&self) -> Option<&HttpProvider> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {

    use super::{ProviderDelta, StreamFault, TerminalFrames};
    use crate::providers::adapter::{ProviderResponse, StopReason};

    /// Feed a scripted delta sequence through the terminal-frame reader.
    fn frames(deltas: Vec<ProviderDelta>) -> TerminalFrames {
        let mut f = TerminalFrames::default();
        for d in &deltas {
            f.observe(d);
        }
        f
    }

    /// A response as the collector would hand it over: content, and the
    /// `EndTurn` the collector reports for a stated *and* an absent `Done`.
    fn answered() -> ProviderResponse {
        ProviderResponse {
            text: Some("partial".into()),
            stop_reason: StopReason::EndTurn,
            ..ProviderResponse::default()
        }
    }

    fn text() -> ProviderDelta {
        ProviderDelta::TextDelta("partial".into())
    }

    #[test]
    fn a_stream_that_reported_no_fault_classifies_as_nothing() {
        let f = frames(vec![text(), ProviderDelta::Done(StopReason::EndTurn)]);
        assert!(f.classify(true).is_none());
    }

    #[test]
    fn a_fault_with_nothing_to_show_is_fatal_even_when_a_done_followed() {
        // Order does not rescue an empty answer: with no text and no tool call
        // the fault IS the response, so it must become a hard `Err` the retry
        // path can classify instead of a silent empty turn.
        let f = frames(vec![
            ProviderDelta::Error("overloaded".into()),
            ProviderDelta::Done(StopReason::EndTurn),
        ]);
        assert!(matches!(
            f.classify(false),
            Some(StreamFault::Fatal("overloaded"))
        ));
    }

    #[test]
    fn a_fault_that_ended_the_stream_is_charged_and_its_end_turn_is_a_default() {
        let f = frames(vec![text(), ProviderDelta::Error("overloaded".into())]);
        assert!(matches!(
            f.classify(true),
            Some(StreamFault::Charged {
                message: "overloaded",
                stop_reason_is_default: true,
            })
        ));
    }

    #[test]
    fn a_done_after_the_fault_makes_it_advisory_and_charges_nothing() {
        // No adapter stops parsing on an error frame, so a relay that pushes a
        // non-fatal `{"error": ...}` chunk and then finishes the turn normally
        // ends `Error -> ... -> Done`. That turn completed; charging it a
        // circuit strike would open the breaker on a provider that is answering.
        let f = frames(vec![
            text(),
            ProviderDelta::Error("upstream hiccup".into()),
            ProviderDelta::Done(StopReason::EndTurn),
        ]);
        assert!(
            matches!(f.classify(true), Some(StreamFault::Advisory(_))),
            "a fault the provider recovered from must not be charged"
        );
    }

    #[test]
    fn an_end_turn_the_provider_stated_before_faulting_is_not_a_default() {
        // `Done(EndTurn)` then a fault: the stop reason is a fact the provider
        // asserted, not the collector's placeholder, so it must survive. Reading
        // it off the folded `StopReason` value alone cannot tell the two apart.
        let f = frames(vec![
            text(),
            ProviderDelta::Done(StopReason::EndTurn),
            ProviderDelta::Error("died after finishing".into()),
        ]);
        assert!(matches!(
            f.classify(true),
            Some(StreamFault::Charged {
                stop_reason_is_default: false,
                ..
            })
        ));
    }

    #[test]
    fn only_the_first_fault_is_reported() {
        let f = frames(vec![
            text(),
            ProviderDelta::Error("first".into()),
            ProviderDelta::Error("second".into()),
        ]);
        assert!(matches!(
            f.classify(true),
            Some(StreamFault::Charged {
                message: "first",
                ..
            })
        ));
    }
    // ── The apply step ──────────────────────────────────────────────────
    // The derivation being right is not the same claim as its verdict reaching
    // the response. These assert the *effects* the failover walk reads, so
    // dropping the `provider_error` assignment or the stop-reason downgrade
    // turns a test red instead of leaving the walk silently un-charged.

    #[test]
    fn a_fatal_verdict_becomes_an_err_carrying_the_message() {
        let mut resp = ProviderResponse::default();
        let err = super::apply_fault("p", &mut resp, Some(StreamFault::Fatal("overloaded")))
            .expect_err("a fault with nothing to show must not return Ok");
        assert!(
            err.to_string().contains("overloaded"),
            "the provider's own message must survive into the error: {err}"
        );
    }

    #[test]
    fn a_charged_verdict_parks_the_fault_on_the_response() {
        let mut resp = answered();
        super::apply_fault(
            "p",
            &mut resp,
            Some(StreamFault::Charged {
                message: "upstream died",
                stop_reason_is_default: true,
            }),
        )
        .expect("a charged fault still returns the partial answer");
        assert_eq!(
            resp.provider_error.as_deref(),
            Some("upstream died"),
            "the walk reads this field and nothing else"
        );
        assert_eq!(
            resp.stop_reason,
            StopReason::Unknown,
            "no Done arrived, so the collector's EndTurn is a default and must be downgraded"
        );
        assert_eq!(resp.text.as_deref(), Some("partial"), "content is kept");
    }

    #[test]
    fn a_charged_verdict_keeps_a_stop_reason_the_provider_stated() {
        let mut resp = answered();
        super::apply_fault(
            "p",
            &mut resp,
            Some(StreamFault::Charged {
                message: "died after finishing",
                stop_reason_is_default: false,
            }),
        )
        .expect("a charged fault still returns the partial answer");
        assert_eq!(resp.provider_error.as_deref(), Some("died after finishing"));
        assert_eq!(
            resp.stop_reason,
            StopReason::EndTurn,
            "the provider said EndTurn — that is a fact, not the collector's placeholder"
        );
    }

    #[test]
    fn an_advisory_verdict_touches_nothing() {
        let mut resp = answered();
        super::apply_fault("p", &mut resp, Some(StreamFault::Advisory("hiccup")))
            .expect("a recovered fault is a success");
        assert!(
            resp.provider_error.is_none(),
            "charging a provider that finished the turn opens the breaker on one that is answering"
        );
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn no_fault_touches_nothing() {
        let mut resp = answered();
        super::apply_fault("p", &mut resp, None).expect("no fault, no error");
        assert!(resp.provider_error.is_none());
        assert_eq!(resp.stop_reason, StopReason::EndTurn);
    }

    #[test]
    fn test_http_provider_creation() {
        // This test just verifies the type compiles correctly
        // Actual functionality tested via integration tests
    }

    #[test]
    fn test_pii_filtering_integration() {
        use crate::config::PrivacyConfig;
        use crate::pii::PiiEngine;

        let engine = PiiEngine::new(PrivacyConfig::default());
        let result = engine.filter("User: Call 13812345678 for info");
        assert!(result.text.contains("[PHONE]"));
        assert!(!result.text.contains("13812345678"));
    }

    #[test]
    fn stale_encrypted_reasoning_error_matches_provider_400_only() {
        let stale = crate::error::AlephError::provider(
            "OpenAI Responses API error (400 Bad Request): {\"error\":{\"code\":\
             \"invalid_encrypted_content\",\"message\":\"Invalid encrypted_content\"}}",
        );
        assert!(super::is_stale_encrypted_reasoning_error(&stale));

        let unrelated = crate::error::AlephError::provider("500 Internal Server Error");
        assert!(!super::is_stale_encrypted_reasoning_error(&unrelated));

        // Non-ProviderError kinds never trigger the recovery, even if the
        // message mentions encrypted content.
        let timeout = crate::error::AlephError::Timeout {
            suggestion: Some("encrypted_content".into()),
        };
        assert!(!super::is_stale_encrypted_reasoning_error(&timeout));
    }

    #[test]
    fn strip_thinking_signatures_drops_verifier_keeps_text() {
        use crate::providers::message::{ContentBlock, UnifiedMessage};

        let messages = vec![
            UnifiedMessage::User {
                content: vec![ContentBlock::Text {
                    text: "hi".into(),
                    cache_control: None,
                }],
            },
            UnifiedMessage::Assistant {
                content: vec![
                    ContentBlock::Thinking {
                        thinking: "chain of thought".into(),
                        signature: Some("{\"id\":\"rs_1\",\"ec\":\"gAAA\"}\n".into()),
                    },
                    ContentBlock::Text {
                        text: "answer".into(),
                        cache_control: None,
                    },
                ],
            },
        ];

        let stripped = super::strip_thinking_signatures(&messages)
            .expect("a signed thinking block must yield a rewrite");
        let UnifiedMessage::Assistant { content } = &stripped[1] else {
            panic!("assistant message expected");
        };
        let ContentBlock::Thinking {
            thinking,
            signature,
        } = &content[0]
        else {
            panic!("thinking block expected");
        };
        assert_eq!(thinking, "chain of thought");
        assert!(signature.is_none(), "signature must be dropped");
        assert!(
            matches!(&content[1], ContentBlock::Text { text, .. } if text == "answer"),
            "sibling blocks must be preserved"
        );

        // No signature anywhere → None, so the caller skips the retry.
        assert!(super::strip_thinking_signatures(&stripped).is_none());
        assert!(super::strip_thinking_signatures(&messages[..1]).is_none());
    }

    #[test]
    fn hook_session_id_prefers_metadata_then_falls_back() {
        use crate::providers::adapter::RequestPayload;

        let mut meta = std::collections::HashMap::new();
        meta.insert("session_id".to_string(), "sess-42".to_string());
        let payload = RequestPayload {
            metadata: Some(meta),
            ..Default::default()
        };
        assert_eq!(super::hook_session_id(&payload), "sess-42");

        // No metadata → synthetic id so the cost meter still aggregates.
        assert_eq!(
            super::hook_session_id(&RequestPayload::default()),
            "provider"
        );
    }

    /// Reproduces the production hang: an upstream that accepts the TCP
    /// connection but never sends response headers. Before the TTFB guard,
    /// `execute` blocked until the per-turn watchdog (if one is configured at
    /// all — `[stability] turn_timeout_secs` is unset by default in the
    /// library); now it must surface
    /// `AlephError::Timeout` within `stream_idle_timeout_secs`.
    #[tokio::test]
    async fn ttfb_timeout_fires_when_upstream_never_responds() {
        use crate::config::ProviderConfig;
        use crate::providers::adapter::RequestPayload;
        use crate::providers::message::UnifiedMessage;
        use crate::providers::protocols::AnthropicProtocol;
        use crate::sync_primitives::Arc;

        // Bind a listener that accepts one connection and then hangs forever —
        // a live socket that never writes a byte (the exact api.kimi.com
        // failure mode).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            // Hold the socket open without responding.
            futures::future::pending::<()>().await;
        });

        let mut config = ProviderConfig::test_config("claude-test");
        config.base_url = Some(format!("http://{addr}"));
        config.stream_idle_timeout_secs = Some(1);

        let adapter = Arc::new(AnthropicProtocol::new(reqwest::Client::new()));
        let provider = super::HttpProvider::new("hanging".to_string(), config, adapter).unwrap();

        let messages = vec![UnifiedMessage::user("hi")];
        let payload = RequestPayload {
            messages: &messages,
            model: Some("claude-test".to_string()),
            ..Default::default()
        };

        // Outer guard so a regression (no TTFB timeout) fails fast instead of
        // hanging the test binary.
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            provider.execute(payload, None),
        )
        .await
        .expect("execute must return within 5s — TTFB guard did not fire");

        assert!(
            matches!(result, Err(crate::error::AlephError::Timeout { .. })),
            "stalled upstream must yield AlephError::Timeout, got {result:?}",
        );
    }

    #[test]
    fn append_usage_env_emits_present_fields_only() {
        use crate::providers::adapter::TokenUsage;

        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: Some(10),
            cache_creation_tokens: None,
            thinking_tokens: None,
            cost: None,
        };
        let mut env: Vec<(&'static str, String)> = Vec::new();
        super::append_usage_env(&mut env, &usage);
        let map: std::collections::HashMap<_, _> = env.into_iter().collect();

        assert_eq!(map.get("INPUT_TOKENS"), Some(&"100".to_string()));
        assert_eq!(map.get("OUTPUT_TOKENS"), Some(&"50".to_string()));
        assert_eq!(map.get("CACHE_READ_TOKENS"), Some(&"10".to_string()));
        // Absent Option fields must not emit env keys.
        assert!(!map.contains_key("CACHE_CREATION_TOKENS"));
        assert!(!map.contains_key("THINKING_TOKENS"));
        assert!(!map.contains_key("COST_USD"));
    }
}
