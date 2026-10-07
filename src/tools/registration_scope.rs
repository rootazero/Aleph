//! [`ToolRegistrationScope`] — an ordered, idempotent set of tool
//! registrations owned by one logical component (an MCP server, a capability
//! builtin bundle, ...) and the one operation that takes them all back out.
//!
//! This deliberately mirrors [`EffectScope`](crate::extension::effects::EffectScope)
//! without reusing it: tool registration ownership is not plugin lifecycle, the
//! step count is variable, and every step is labelled by tool name rather than
//! the plugin scope's fixed [`STEP_LABELS`](crate::extension::effects::STEP_LABELS).
//! Only [`Disposer`], [`DisposeOutcome`] and [`sync_disposer`] are shared.
//!
//! A scope owns no background task and implements no `Drop`: dropping a scope
//! never runs cleanup implicitly. The only way to dispose a scope is to drive
//! [`ToolRegistrationScope::dispose`], which consumes `self`.

use crate::extension::effects::{sync_disposer, DisposeOutcome, Disposer};
use crate::tools::registry::RegistrationHandle;
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// Everything one logical component registered with the tool registry.
pub struct ToolRegistrationScope {
    owner: String,
    /// `(step_label, disposer)` in registration order. Dispose runs them in
    /// reverse, so the last registration is the first one removed.
    steps: Vec<(String, Disposer)>,
}

impl ToolRegistrationScope {
    /// Start an empty scope owned by `owner` (e.g. `"mcp:github"` or
    /// `"capability:builtins"`).
    #[must_use]
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            steps: Vec::new(),
        }
    }

    /// The owner label this scope reports in [`ToolDisposeReport`] and logs.
    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// Track the registration a [`RegistrationHandle`] refers to.
    ///
    /// The handle's `dispose` is generation-guarded and idempotent: it removes
    /// the entry only while the registry still holds the exact revision this
    /// handle registered. Once the registration was replaced or already
    /// cleaned up it reports `false`; that is the expected "nothing left to
    /// do" outcome, not an error, so the wrapped disposer still reports
    /// success and a stale handle never removes a replacement.
    pub fn track(&mut self, handle: RegistrationHandle) {
        let label = handle.name().to_string();
        self.steps.push((
            label,
            sync_disposer(move || {
                // `false` == already expired/cleaned; a successful no-op.
                let _removed = handle.dispose();
                Ok(())
            }),
        ));
    }

    /// Track an arbitrary labelled cleanup.
    ///
    /// Exposed so a caller that must register cleanup for something that is not
    /// a [`RegistrationHandle`] — and the tests that exercise the ordered,
    /// panic-tolerant disposal path — can reuse the same scope semantics.
    pub fn track_disposer(&mut self, label: impl Into<String>, d: Disposer) {
        self.steps.push((label.into(), d));
    }

    /// Labels of the tracked steps, in registration order.
    #[must_use]
    pub fn steps(&self) -> Vec<&str> {
        self.steps.iter().map(|(s, _)| s.as_str()).collect()
    }

    /// Number of tracked registrations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether this scope tracks nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Run every tracked disposer in REVERSE registration order. A failing or
    /// panicking disposer is recorded in the report (and logged with its step
    /// label) and does NOT stop the rest. Consumes `self`: a scope cannot be
    /// half-disposed, and dropping a scope never disposes implicitly.
    pub async fn dispose(self) -> ToolDisposeReport {
        let mut steps = Vec::with_capacity(self.steps.len());
        for (label, d) in self.steps.into_iter().rev() {
            let outcome = run_one(d).await;
            if let Err(e) = &outcome {
                tracing::warn!(
                    owner = %self.owner,
                    step = %label,
                    error = %e,
                    "tool registration disposer failed; continuing"
                );
            }
            steps.push((label, outcome));
        }
        ToolDisposeReport {
            owner: self.owner,
            steps,
        }
    }
}

/// Run one disposer, converting a panic on either side of the `await`
/// (building the future, or polling it) into an `Err`.
async fn run_one(d: Disposer) -> DisposeOutcome {
    let fut = match std::panic::catch_unwind(AssertUnwindSafe(d)) {
        Ok(fut) => fut,
        Err(payload) => return Err(format!("disposer panicked: {}", panic_message(&payload))),
    };
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(outcome) => outcome,
        Err(payload) => Err(format!("disposer panicked: {}", panic_message(&payload))),
    }
}

fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// What a [`ToolRegistrationScope::dispose`] did, step by step, in run order.
#[derive(Debug)]
pub struct ToolDisposeReport {
    /// The owner label of the scope that was disposed.
    pub owner: String,
    /// `(step_label, outcome)` in the order the disposers actually ran
    /// (reverse registration order).
    pub steps: Vec<(String, DisposeOutcome)>,
}

impl ToolDisposeReport {
    /// `true` when every tracked step disposed without error.
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.steps.iter().all(|(_, r)| r.is_ok())
    }

    /// The failed steps, in run order.
    pub fn failures(&self) -> impl Iterator<Item = (&str, &str)> + '_ {
        self.steps
            .iter()
            .filter_map(|(s, r)| r.as_ref().err().map(|e| (s.as_str(), e.as_str())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::events::ToolOutput;
    use crate::tools::descriptor::{
        ReplayPolicy, ToolCapabilityDescriptor, ToolKind, SCHEMA_VERSION,
    };
    use crate::tools::handlers::ToolHandler;
    use crate::tools::registry::{RegistryChange, ToolHandlerRegistry};
    use crate::tools::service::{ToolDefinition, ToolDefinitionMetadata, ToolError, ToolSource};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;

    struct FakeHandler {
        name: String,
    }

    #[async_trait]
    impl ToolHandler for FakeHandler {
        async fn invoke(&self, _input: Value) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput {
                value: serde_json::json!({ "tool": self.name }),
                metadata: Default::default(),
            })
        }
        fn definition(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.name.clone(),
                description: String::new(),
                input_schema: serde_json::json!({}),
                source: ToolSource::Builtin,
                metadata: ToolDefinitionMetadata::default(),
            }
        }
    }

    fn fake(name: &str) -> Arc<dyn ToolHandler> {
        Arc::new(FakeHandler { name: name.into() })
    }

    /// A descriptor whose every projected field matches [`fake`]. `revision` is
    /// left 0 on purpose: the registry assigns it.
    fn desc(name: &str) -> ToolCapabilityDescriptor {
        ToolCapabilityDescriptor {
            name: name.into(),
            kind: ToolKind::Tool,
            schema_version: SCHEMA_VERSION,
            description: String::new(),
            input_schema: serde_json::json!({}),
            source: ToolSource::Builtin,
            replay_policy: ReplayPolicy::Unsafe,
            requires_confirmation: false,
            idempotent: false,
            concurrent_safe: false,
            max_duration_ms: None,
            revision: 0,
            implementation_contract: None,
        }
    }

    fn drain_unregistered(
        rx: &mut tokio::sync::broadcast::Receiver<RegistryChange>,
    ) -> Vec<String> {
        let mut names = Vec::new();
        while let Ok(change) = rx.try_recv() {
            if let RegistryChange::Unregistered { name, .. } = change {
                names.push(name);
            }
        }
        names
    }

    #[tokio::test]
    async fn scope_disposes_registrations_in_reverse_order_and_only_once() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe();
        let mut scope = ToolRegistrationScope::new("mcp:server-1");
        for name in ["a", "b", "c"] {
            let handle = reg.register(desc(name), fake(name)).unwrap();
            scope.track(handle);
        }
        assert_eq!(scope.len(), 3);
        assert_eq!(scope.steps(), vec!["a", "b", "c"], "registration order");

        let report = scope.dispose().await;
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(report.owner, "mcp:server-1");
        assert_eq!(
            report
                .steps
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b", "a"],
            "reverse of registration order"
        );
        assert!(reg.snapshot().is_empty(), "all entries removed");
        assert_eq!(
            drain_unregistered(&mut rx),
            vec!["c", "b", "a"],
            "each registration disposed exactly once, in reverse order"
        );
    }

    #[tokio::test]
    async fn stale_handle_is_a_successful_no_op() {
        let reg = ToolHandlerRegistry::new();

        // A handle whose entry was already removed: dispose returns `false`.
        let stale = reg.register(desc("gone"), fake("gone")).unwrap();
        assert!(reg.unregister("gone").is_some());

        // A handle replaced by a newer revision: it must NOT delete the
        // replacement, and dispose returns `false`.
        let replaced = reg.register(desc("live"), fake("live")).unwrap();
        let _newer = reg.replace(desc("live"), fake("live")).unwrap();

        let mut scope = ToolRegistrationScope::new("mcp:server-stale");
        scope.track(stale);
        scope.track(replaced);
        let report = scope.dispose().await;

        assert!(report.all_ok(), "stale handles are no-ops, not errors");
        assert_eq!(report.failures().count(), 0);
        assert!(
            reg.resolve("live").is_some(),
            "a stale handle must not remove a replacement registration"
        );
    }

    #[tokio::test]
    async fn disposer_failure_does_not_skip_later_registration_cleanup() {
        let reg = ToolHandlerRegistry::new();
        let mut rx = reg.subscribe();
        let mut scope = ToolRegistrationScope::new("mcp:server-2");

        let first = reg.register(desc("keep1"), fake("keep1")).unwrap();
        scope.track(first);
        scope.track_disposer(
            "boom",
            sync_disposer(|| Err("remove_transient_server: channel closed".to_string())),
        );
        let second = reg.register(desc("keep2"), fake("keep2")).unwrap();
        scope.track(second);
        scope.track_disposer(
            "panic",
            sync_disposer(|| -> DisposeOutcome { panic!("sync boom") }),
        );

        let report = scope.dispose().await;

        assert!(!report.all_ok());
        assert_eq!(report.owner, "mcp:server-2");
        assert_eq!(
            report
                .steps
                .iter()
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>(),
            vec!["panic", "keep2", "boom", "keep1"],
            "reverse order preserved even across failures"
        );
        let failures: Vec<_> = report.failures().collect();
        assert_eq!(failures.len(), 2, "{report:?}");
        assert!(failures
            .iter()
            .any(|(s, e)| *s == "boom" && e.contains("channel closed")));
        assert!(failures
            .iter()
            .any(|(s, e)| *s == "panic" && e.contains("sync boom")));
        assert!(
            reg.resolve("keep1").is_none() && reg.resolve("keep2").is_none(),
            "real registrations still cleaned up despite failures"
        );
        assert_eq!(drain_unregistered(&mut rx), vec!["keep2", "keep1"]);
    }

    #[tokio::test]
    async fn empty_scope_disposes_to_an_ok_report() {
        let scope = ToolRegistrationScope::new("capability:builtins");
        assert!(scope.is_empty());
        let report = scope.dispose().await;
        assert!(report.all_ok());
        assert_eq!(report.owner, "capability:builtins");
        assert!(report.steps.is_empty());
    }
}
