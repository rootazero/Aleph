//! Request-boundary session-model resolution.
//!
//! `SessionModelProvider` closes the gap between "the user picked a model for
//! this session" (recorded by [`session_model_handle`](super::session_model_handle))
//! and "the next request actually goes to that model". It reads the live
//! selection exactly once, at the start of every `process` /
//! `execute_streaming_dyn` call, resolves it to an immutable provider + model
//! pair, and delegates the *entire* request to that pair. A concurrent
//! `select_model` that lands while a call is in flight cannot rewrite the
//! in-flight call — it only takes effect on the next request boundary.
//!
//! This is the request-time half of the A-layer dynamic routing (R8); the write
//! side is `select_model` → `set_session_model`, and the durable half is the
//! session row. There is deliberately no second process-global map here, no
//! prompt classifier, and no static fallback chain: the session handle and the
//! caller-supplied `named` / `default_provider` maps are the only sources of
//! truth.
//!
//! # Resolution contract
//!
//! * no selection → delegate the whole request to `fallback`;
//! * qualified selection (`provider = Some(name)`) → look `name` up in
//!   `named`; missing → fail closed with a provider error, never fallback;
//! * unqualified selection (`provider = None`) → `default_provider.current()`,
//!   the live default; the selected model is stamped onto it.
//!
//! In the selected cases the model is stamped via
//! [`ModelOverrideProvider`](super::model_override_provider::ModelOverrideProvider),
//! and the immutable [`ResolvedModelTarget`] is recorded so
//! [`AiProvider::last_request_target`](super::AiProvider::last_request_target)
//! can report it after the call completes.

use std::borrow::Cow;
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::error::{AlephError, Result};
use crate::providers::adapter::{ProviderResponse, RequestPayload};
use crate::providers::session_model_handle::{get_session_model_snapshot, SessionModelSnapshot};
use crate::providers::{AiProvider, DefaultProviderHandle, DeltaSink, ModelOverrideProvider};
use crate::sync_primitives::{Arc, Mutex};

/// The provider-neutral identity of the model a request was actually served
/// by, together with the session-selection generation that produced it.
///
/// `provider` is the provider id — the `named` key for a qualified pick, or the
/// default provider's serving id for an unqualified one; `model` is the stamped
/// model; `generation` is the [`SessionModelSnapshot::generation`] the selection
/// carried when the request began.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedModelTarget {
    pub provider: String,
    pub model: String,
    pub generation: u64,
}

/// A request-boundary adapter that resolves a session's live model selection.
///
/// See the module docs for the resolution contract.
pub struct SessionModelProvider {
    session_key: String,
    fallback: Arc<dyn AiProvider>,
    default_provider: Arc<dyn DefaultProviderHandle>,
    named: Arc<HashMap<String, Arc<dyn AiProvider>>>,
    /// The immutable target of the most recent request, when that request had a
    /// session selection. `None` when the most recent request delegated to
    /// `fallback` (no selection).
    last_target: Mutex<Option<ResolvedModelTarget>>,
}

impl SessionModelProvider {
    /// Wrap the request path with live session-model resolution.
    ///
    /// * `session_key` — the canonical session key the selection is recorded
    ///   under (the same string `select_model` wrote).
    /// * `fallback` — serves every request that has no session selection, and
    ///   backs the metadata contract.
    /// * `default_provider` — the live default resolver for unqualified picks.
    /// * `named` — provider id → provider, for qualified picks.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(
        session_key: String,
        fallback: Arc<dyn AiProvider>,
        default_provider: Arc<dyn DefaultProviderHandle>,
        named: Arc<HashMap<String, Arc<dyn AiProvider>>>,
    ) -> Arc<dyn AiProvider> {
        Arc::new(Self {
            session_key,
            fallback,
            default_provider,
            named,
            last_target: Mutex::new(None),
        })
    }

    /// Resolve one immutable target from a snapshot.
    ///
    /// Returns the provider to delegate to and the [`ResolvedModelTarget`] to
    /// record (`None` for the no-selection fallback case).
    fn resolve(
        &self,
        snapshot: &SessionModelSnapshot,
    ) -> Result<(Arc<dyn AiProvider>, Option<ResolvedModelTarget>)> {
        let Some(pref) = snapshot.pref.as_ref() else {
            return Ok((self.fallback.clone(), None));
        };

        let (base, provider) = match pref.provider.as_ref() {
            Some(name) => {
                let provider = self.named.get(name).cloned().ok_or_else(|| {
                    AlephError::provider(format!(
                        "session selected provider '{name}', but it is no longer registered"
                    ))
                })?;
                (provider, name.clone())
            }
            None => {
                let provider = self.default_provider.current();
                let id = provider
                    .serving_provider_hint()
                    .map(Cow::into_owned)
                    .unwrap_or_else(|| provider.name().to_string());
                (provider, id)
            }
        };

        let target = ResolvedModelTarget {
            provider,
            model: pref.model.clone(),
            generation: snapshot.generation,
        };
        let stamped: Arc<dyn AiProvider> =
            Arc::new(ModelOverrideProvider::new(base, pref.model.clone()));
        Ok((stamped, Some(target)))
    }

    /// The provider metadata should reflect: the resolved current target when a
    /// selection resolves, else the fallback. Metadata never fails — a
    /// selection that fails to resolve (provider removed) reports the
    /// fallback's shape for preflight, while the request itself fails closed.
    fn current_target(&self) -> Arc<dyn AiProvider> {
        let snapshot = get_session_model_snapshot(&self.session_key);
        match self.resolve(&snapshot) {
            Ok((provider, _)) => provider,
            Err(_) => self.fallback.clone(),
        }
    }
}

impl AiProvider for SessionModelProvider {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let snapshot = get_session_model_snapshot(&self.session_key);
        match self.resolve(&snapshot) {
            Ok((provider, target)) => Box::pin(async move {
                let result = provider.process(payload).await;
                let mut last_target = self.last_target.lock().unwrap_or_else(|e| e.into_inner());
                *last_target = result.as_ref().ok().and(target);
                result
            }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn execute_streaming_dyn<'a>(
        &'a self,
        payload: RequestPayload<'a>,
        sink: &'a dyn DeltaSink,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        let snapshot = get_session_model_snapshot(&self.session_key);
        match self.resolve(&snapshot) {
            Ok((provider, target)) => Box::pin(async move {
                let result = provider.execute_streaming_dyn(payload, sink).await;
                let mut last_target = self.last_target.lock().unwrap_or_else(|e| e.into_inner());
                *last_target = result.as_ref().ok().and(target);
                result
            }),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }

    fn last_request_target(&self) -> Option<ResolvedModelTarget> {
        // Only this adapter's own record. A no-selection request delegated to
        // `fallback` and left `last_target` at `None` — forwarding to
        // `fallback.last_request_target()` would surface whatever a *different*
        // session/chain served, polluting provenance across session boundaries.
        self.last_target
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn name(&self) -> &str {
        "session-model"
    }

    fn color(&self) -> &str {
        "#8b5cf6"
    }

    fn supports_native_tools(&self) -> bool {
        self.current_target().supports_native_tools()
    }

    fn protocol(&self) -> Cow<'_, str> {
        Cow::Owned(self.current_target().protocol().into_owned())
    }

    fn model_behavior_override(&self) -> Option<Cow<'_, str>> {
        self.current_target()
            .model_behavior_override()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    fn behavior_hint(&self) -> Option<Cow<'_, str>> {
        self.current_target()
            .behavior_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    fn serving_model_hint(&self) -> Option<Cow<'_, str>> {
        self.current_target()
            .serving_model_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    fn serving_provider_hint(&self) -> Option<Cow<'_, str>> {
        self.current_target()
            .serving_provider_hint()
            .map(|c| Cow::Owned(c.into_owned()))
    }

    fn reasoning_replay(
        &self,
        model: Option<&str>,
    ) -> crate::providers::reasoning_replay::ReasoningReplay {
        self.current_target().reasoning_replay(model)
    }

    fn clears_tool_results_server_side(&self) -> bool {
        self.current_target().clears_tool_results_server_side()
    }

    fn supports_streaming(&self) -> bool {
        self.current_target().supports_streaming()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::message::UnifiedMessage;
    use crate::providers::session_model_handle::set_session_model;
    use crate::providers::{NoopSink, StaticDefault};
    use tokio::sync::Notify;

    /// Provider that records `(name, model)` for every request it serves.
    struct Recorder {
        id: String,
        seen: Mutex<Vec<(String, Option<String>)>>,
    }

    impl Recorder {
        fn arc(id: &str) -> Arc<Self> {
            Arc::new(Self {
                id: id.to_string(),
                seen: Mutex::new(Vec::new()),
            })
        }
        fn seen(&self) -> Vec<(String, Option<String>)> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl AiProvider for Recorder {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            self.seen
                .lock()
                .unwrap()
                .push((self.id.clone(), payload.model.clone()));
            Box::pin(async move { Ok(ProviderResponse::text_only("ok".to_string())) })
        }
        fn name(&self) -> &str {
            &self.id
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    /// Provider whose streaming path records the model synchronously and then
    /// blocks on a `Notify` until the test releases it — a deterministic
    /// stand-in for "request started, response still streaming".
    struct BlockingRecorder {
        seen: Mutex<Option<String>>,
        gate: Arc<Notify>,
        release: Arc<Notify>,
    }

    impl BlockingRecorder {
        fn new() -> (Arc<Self>, Arc<Notify>, Arc<Notify>) {
            let gate = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let rec = Arc::new(Self {
                seen: Mutex::new(None),
                gate: gate.clone(),
                release: release.clone(),
            });
            (rec, gate, release)
        }
    }

    impl AiProvider for BlockingRecorder {
        fn process<'a>(
            &'a self,
            payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            *self.seen.lock().unwrap() = payload.model.clone();
            Box::pin(async move { Ok(ProviderResponse::text_only("done".to_string())) })
        }
        fn execute_streaming_dyn<'a>(
            &'a self,
            payload: RequestPayload<'a>,
            _sink: &'a dyn DeltaSink,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            *self.seen.lock().unwrap() = payload.model.clone();
            let gate = self.gate.clone();
            let release = self.release.clone();
            Box::pin(async move {
                gate.notify_one();
                release.notified().await;
                Ok(ProviderResponse::text_only("done".to_string()))
            })
        }
        fn name(&self) -> &str {
            "blocking"
        }
        fn color(&self) -> &str {
            "#000"
        }
    }

    fn default_handle(provider: Arc<dyn AiProvider>) -> Arc<dyn DefaultProviderHandle> {
        Arc::new(StaticDefault::new(provider))
    }

    fn named(
        map: HashMap<String, Arc<dyn AiProvider>>,
    ) -> Arc<HashMap<String, Arc<dyn AiProvider>>> {
        Arc::new(map)
    }

    async fn roundtrip(provider: &Arc<dyn AiProvider>) {
        let msgs = [UnifiedMessage::user("hi")];
        let _ = provider.process(RequestPayload::new(&msgs)).await.unwrap();
    }

    #[tokio::test]
    async fn no_selection_delegates_to_fallback() {
        let fallback = Recorder::arc("fallback");
        let default = Recorder::arc("default");
        let provider = SessionModelProvider::new(
            "no-sel".to_string(),
            fallback.clone(),
            default_handle(default),
            named(HashMap::new()),
        );
        roundtrip(&provider).await;
        assert_eq!(fallback.seen(), vec![("fallback".to_string(), None)]);
        assert_eq!(provider.last_request_target(), None);
    }

    /// A fallback that CLAIMS a target it served. The adapter must never
    /// surface it: a no-selection request has no session provenance, and
    /// forwarding the fallback's record would attribute this session to
    /// whatever a different chain last served.
    struct ProvenanceFallback(ResolvedModelTarget);

    impl AiProvider for ProvenanceFallback {
        fn process<'a>(
            &'a self,
            _payload: RequestPayload<'a>,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
            Box::pin(async { Ok(ProviderResponse::text_only("ok".to_string())) })
        }
        fn name(&self) -> &str {
            "provenance-fallback"
        }
        fn color(&self) -> &str {
            "#000"
        }
        fn last_request_target(&self) -> Option<ResolvedModelTarget> {
            Some(self.0.clone())
        }
    }

    #[tokio::test]
    async fn no_selection_does_not_leak_fallback_provenance() {
        let fallback = Arc::new(ProvenanceFallback(ResolvedModelTarget {
            provider: "other-provider".to_string(),
            model: "other-model".to_string(),
            generation: 99,
        }));
        let provider = SessionModelProvider::new(
            "no-sel-provenance".to_string(),
            fallback,
            default_handle(Recorder::arc("default")),
            named(HashMap::new()),
        );
        roundtrip(&provider).await;
        assert_eq!(
            provider.last_request_target(),
            None,
            "a no-selection request must not surface the fallback's provenance"
        );
    }

    #[tokio::test]
    async fn qualified_selection_uses_named_provider_and_stamps_model() {
        let openai = Recorder::arc("openai");
        let fallback = Recorder::arc("fallback");
        let mut map: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        map.insert("openai".to_string(), openai.clone());
        let provider = SessionModelProvider::new(
            "qualified".to_string(),
            fallback.clone(),
            default_handle(Recorder::arc("default")),
            named(map),
        );
        set_session_model("qualified", Some("openai".to_string()), "gpt-5".to_string());
        roundtrip(&provider).await;
        assert_eq!(
            openai.seen(),
            vec![("openai".to_string(), Some("gpt-5".to_string()))]
        );
        assert!(fallback.seen().is_empty());
        assert_eq!(
            provider.last_request_target(),
            Some(ResolvedModelTarget {
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
                generation: 1,
            })
        );
    }

    #[tokio::test]
    async fn unqualified_selection_uses_default_provider() {
        let default = Recorder::arc("default");
        let fallback = Recorder::arc("fallback");
        let provider = SessionModelProvider::new(
            "unqualified".to_string(),
            fallback.clone(),
            default_handle(default.clone()),
            named(HashMap::new()),
        );
        set_session_model("unqualified", None, "default-model".to_string());
        roundtrip(&provider).await;
        assert_eq!(
            default.seen(),
            vec![("default".to_string(), Some("default-model".to_string()))]
        );
        assert!(fallback.seen().is_empty());
        assert_eq!(
            provider.last_request_target(),
            Some(ResolvedModelTarget {
                provider: "default".to_string(),
                model: "default-model".to_string(),
                generation: 1,
            })
        );
    }

    #[tokio::test]
    async fn removed_provider_fails_closed_without_fallback() {
        let fallback = Recorder::arc("fallback");
        let default = Recorder::arc("default");
        let provider = SessionModelProvider::new(
            "removed".to_string(),
            fallback.clone(),
            default_handle(default.clone()),
            named(HashMap::new()), // "ghost" never registered
        );
        set_session_model("removed", Some("ghost".to_string()), "x".to_string());
        let msgs = [UnifiedMessage::user("hi")];
        let err = provider
            .process(RequestPayload::new(&msgs))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("ghost"), "error was: {err}");
        assert!(fallback.seen().is_empty());
        assert!(default.seen().is_empty());
    }

    #[tokio::test]
    async fn next_call_sees_changed_selection_and_generation() {
        let openai = Recorder::arc("openai");
        let fallback = Recorder::arc("fallback");
        let mut map: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        map.insert("openai".to_string(), openai.clone());
        let provider = SessionModelProvider::new(
            "next-call".to_string(),
            fallback,
            default_handle(Recorder::arc("default")),
            named(map),
        );

        set_session_model(
            "next-call",
            Some("openai".to_string()),
            "model-A".to_string(),
        );
        roundtrip(&provider).await;
        assert_eq!(
            provider.last_request_target(),
            Some(ResolvedModelTarget {
                provider: "openai".to_string(),
                model: "model-A".to_string(),
                generation: 1,
            })
        );

        set_session_model(
            "next-call",
            Some("openai".to_string()),
            "model-B".to_string(),
        );
        roundtrip(&provider).await;
        assert_eq!(
            openai.seen()[1],
            ("openai".to_string(), Some("model-B".to_string()))
        );
        assert_eq!(
            provider.last_request_target(),
            Some(ResolvedModelTarget {
                provider: "openai".to_string(),
                model: "model-B".to_string(),
                generation: 2,
            })
        );
    }

    #[tokio::test]
    async fn streaming_keeps_immutable_target_while_selection_changes() {
        let (rec, gate, release) = BlockingRecorder::new();
        let fallback = Recorder::arc("fallback");
        let mut map: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        map.insert("openai".to_string(), rec.clone());
        let provider = SessionModelProvider::new(
            "stream".to_string(),
            fallback,
            default_handle(Recorder::arc("default")),
            named(map),
        );
        set_session_model("stream", Some("openai".to_string()), "model-A".to_string());

        let handle = {
            let provider = provider.clone();
            tokio::spawn(async move {
                let msgs = [UnifiedMessage::user("hi")];
                let sink = NoopSink;
                provider
                    .execute_streaming_dyn(RequestPayload::new(&msgs), &sink)
                    .await
            })
        };

        // Wait until the request has resolved + captured its model.
        gate.notified().await;

        // Switch the selection while the first call is still streaming.
        set_session_model("stream", Some("openai".to_string()), "model-B".to_string());

        // The in-flight call must still finish on model-A (generation 1).
        release.notify_one();
        let _ = handle.await.unwrap().unwrap();
        assert_eq!(rec.seen.lock().unwrap().as_deref(), Some("model-A"));
        assert_eq!(
            provider.last_request_target(),
            Some(ResolvedModelTarget {
                provider: "openai".to_string(),
                model: "model-A".to_string(),
                generation: 1,
            })
        );

        // A later call sees the new selection.
        roundtrip(&provider).await;
        assert_eq!(rec.seen.lock().unwrap().as_deref(), Some("model-B"));
        assert_eq!(
            provider.last_request_target(),
            Some(ResolvedModelTarget {
                provider: "openai".to_string(),
                model: "model-B".to_string(),
                generation: 2,
            })
        );
    }

    /// The runner's precedence is structural: a present `turn_model` builds a
    /// `ModelOverrideProvider` DIRECTLY and never constructs a
    /// `SessionModelProvider` (see `run`), so a live `select_model` pick cannot
    /// shadow the explicit per-request choice. This seam test proves the half of
    /// that contract the wrapper itself cannot: the turn path (`ModelOverrideProvider`)
    /// serves its own stamped model and reports no session provenance, regardless
    /// of what `select_model` recorded. The complementary half — that the no-turn
    /// path DOES re-read the selection — is `next_call_sees_changed_selection_and_generation`.
    #[tokio::test]
    async fn explicit_turn_override_is_not_shadowed_by_session_selection() {
        let base = Recorder::arc("turn-base");
        let turn_llm: Arc<dyn AiProvider> = Arc::new(ModelOverrideProvider::new(
            base.clone(),
            "turn-model".to_string(),
        ));
        // A session pick exists — but the turn path never consults the wrapper.
        set_session_model(
            "turn-bypass",
            Some("openai".to_string()),
            "session-model".to_string(),
        );
        let msgs = [UnifiedMessage::user("hi")];
        let _ = turn_llm.process(RequestPayload::new(&msgs)).await.unwrap();
        assert_eq!(
            base.seen(),
            vec![("turn-base".to_string(), Some("turn-model".to_string()))],
            "the explicit turn override must serve its stamped model, not the session pick"
        );
        assert_eq!(
            turn_llm.last_request_target(),
            None,
            "a plain ModelOverrideProvider has no session provenance to report"
        );
    }

    /// The actual concurrency boundary in the harness: each `run` builds a fresh
    /// `SessionModelProvider`, and within a run requests are strictly awaited
    /// (never concurrent). Two instances over the same session key must not share
    /// a provenance slot — a request on instance B cannot claim the target that
    /// instance A recorded, and B's own record is read from its own selection.
    #[tokio::test]
    async fn separate_instances_do_not_share_a_provenance_slot() {
        let openai_a = Recorder::arc("openai");
        let openai_b = Recorder::arc("openai");
        let mut map_a: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        map_a.insert("openai".to_string(), openai_a.clone());
        let mut map_b: HashMap<String, Arc<dyn AiProvider>> = HashMap::new();
        map_b.insert("openai".to_string(), openai_b.clone());

        let a = SessionModelProvider::new(
            "shared-session".to_string(),
            Recorder::arc("fallback-a"),
            default_handle(Recorder::arc("default-a")),
            named(map_a),
        );
        let b = SessionModelProvider::new(
            "shared-session".to_string(),
            Recorder::arc("fallback-b"),
            default_handle(Recorder::arc("default-b")),
            named(map_b),
        );

        set_session_model(
            "shared-session",
            Some("openai".to_string()),
            "model-A".to_string(),
        );
        roundtrip(&a).await;
        assert_eq!(
            a.last_request_target().map(|t| t.model),
            Some("model-A".to_string())
        );

        // A request on B reads its OWN selection and must not overwrite A's slot.
        set_session_model(
            "shared-session",
            Some("openai".to_string()),
            "model-B".to_string(),
        );
        roundtrip(&b).await;
        assert_eq!(
            b.last_request_target().map(|t| t.model),
            Some("model-B".to_string())
        );
        // A's recorded target is untouched by B's request.
        assert_eq!(
            a.last_request_target().map(|t| t.model),
            Some("model-A".to_string())
        );
    }
}
