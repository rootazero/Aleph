//! [`EffectScope`] — everything one mounted plugin put into the runtime, in
//! registration order, and the one operation that takes it all back out.

use super::disposer::{DisposeOutcome, Disposer};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// Plugin identifier as the registry spells it: a bare string
/// (`PluginRecord.id`, `PluginRegistry::get_plugin(&str)`).
pub type PluginId = String;

/// The six effect kinds `lifecycle.rs::mount` registers, in registration
/// order. Dispose runs the reverse: the registry row is first in and last
/// out, so when the views are re-derived it is already gone.
pub const STEP_LABELS: [&str; 6] = [
    "registry_row",
    "wasm_module",
    "mcp_server",
    "service",
    "memory_extension",
    "slash_command",
];

/// Everything one mounted plugin put into the runtime.
pub struct EffectScope {
    plugin_id: PluginId,
    disposers: Vec<(&'static str, Disposer)>,
    /// Steps `mount` could not perform because the runtime handle they need
    /// is not installed in this process (CLI/test paths). Recorded so the
    /// status can say "mounted, waiting on X" instead of silently `Loaded`.
    skipped: Vec<(&'static str, String)>,
}

impl EffectScope {
    #[must_use]
    pub fn new(plugin_id: impl Into<PluginId>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            disposers: Vec::new(),
            skipped: Vec::new(),
        }
    }

    #[must_use]
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Record one effect. `step` must be one of [`STEP_LABELS`]; a label
    /// outside that list is a programming error (debug-asserted) because the
    /// activation gate (P3) derives `waiting_on` from these names.
    pub fn effect(&mut self, step: &'static str, d: Disposer) {
        debug_assert!(
            STEP_LABELS.contains(&step),
            "unknown effect step label {step:?}; add it to STEP_LABELS first"
        );
        self.disposers.push((step, d));
    }

    /// Record that `step` was not performed because its handle is absent.
    pub fn skip(&mut self, step: &'static str, why: impl Into<String>) {
        debug_assert!(STEP_LABELS.contains(&step));
        self.skipped.push((step, why.into()));
    }

    #[must_use]
    pub fn skipped(&self) -> &[(&'static str, String)] {
        &self.skipped
    }

    /// Labels of the registered effects, in registration order.
    #[must_use]
    pub fn steps(&self) -> Vec<&'static str> {
        self.disposers.iter().map(|(s, _)| *s).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.disposers.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.disposers.is_empty()
    }

    /// Run every disposer in REVERSE registration order. A failing or
    /// panicking disposer is recorded in the report (and logged with its
    /// step label) and does NOT stop the rest (P7). Consumes `self`: a scope
    /// cannot be half-disposed.
    pub async fn dispose(self) -> DisposeReport {
        let mut steps = Vec::with_capacity(self.disposers.len());
        for (step, d) in self.disposers.into_iter().rev() {
            let outcome = run_one(d).await;
            if let Err(e) = &outcome {
                tracing::warn!(plugin_id = %self.plugin_id, step, error = %e, "disposer failed; continuing");
            }
            steps.push((step, outcome));
        }
        DisposeReport {
            plugin_id: self.plugin_id,
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

/// What `dispose` did, step by step, in the order it ran them.
#[derive(Debug)]
pub struct DisposeReport {
    pub plugin_id: PluginId,
    pub steps: Vec<(&'static str, DisposeOutcome)>,
}

impl DisposeReport {
    #[must_use]
    pub fn all_ok(&self) -> bool {
        self.steps.iter().all(|(_, r)| r.is_ok())
    }

    /// The failed steps, in run order.
    pub fn failures(&self) -> impl Iterator<Item = (&'static str, &str)> + '_ {
        self.steps
            .iter()
            .filter_map(|(s, r)| r.as_ref().err().map(|e| (*s, e.as_str())))
    }

    /// A report for a plugin that had nothing to dispose.
    #[must_use]
    pub fn empty(plugin_id: impl Into<PluginId>) -> Self {
        Self {
            plugin_id: plugin_id.into(),
            steps: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::effects::{async_disposer, sync_disposer};
    use std::sync::{Arc, Mutex};

    fn recorder() -> (
        Arc<Mutex<Vec<&'static str>>>,
        impl Fn(&'static str) -> Disposer,
    ) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = Arc::clone(&log);
        let mk = move |label: &'static str| {
            let l = Arc::clone(&l2);
            sync_disposer(move || {
                l.lock().unwrap().push(label);
                Ok(())
            })
        };
        (log, mk)
    }

    #[tokio::test]
    async fn disposers_run_in_reverse_registration_order() {
        let (log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        scope.effect("registry_row", mk("registry_row"));
        scope.effect("wasm_module", mk("wasm_module"));
        scope.effect("mcp_server", mk("mcp_server"));
        assert_eq!(scope.len(), 3);
        let report = scope.dispose().await;
        assert!(report.all_ok(), "{report:?}");
        assert_eq!(
            *log.lock().unwrap(),
            vec!["mcp_server", "wasm_module", "registry_row"],
            "reverse of registration order"
        );
        assert_eq!(
            report.steps.iter().map(|(s, _)| *s).collect::<Vec<_>>(),
            vec!["mcp_server", "wasm_module", "registry_row"]
        );
    }

    #[tokio::test]
    async fn async_disposer_is_awaited_before_the_next_one_runs() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut scope = EffectScope::new("p");
        let l = Arc::clone(&log);
        scope.effect(
            "registry_row",
            sync_disposer(move || {
                l.lock().unwrap().push("row");
                Ok(())
            }),
        );
        let l = Arc::clone(&log);
        scope.effect(
            "service",
            async_disposer(move || async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                l.lock().unwrap().push("service-done");
                Ok(())
            }),
        );
        let report = scope.dispose().await;
        assert!(report.all_ok());
        // If the async one were not awaited, "row" would land first.
        assert_eq!(*log.lock().unwrap(), vec!["service-done", "row"]);
    }

    #[tokio::test]
    async fn a_failing_disposer_is_recorded_and_the_rest_still_run() {
        let (log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        scope.effect("registry_row", mk("registry_row"));
        scope.effect(
            "mcp_server",
            sync_disposer(|| Err("remove_transient_server: channel closed".to_string())),
        );
        scope.effect("slash_command", mk("slash_command"));
        let report = scope.dispose().await;
        assert!(!report.all_ok());
        let failures: Vec<_> = report.failures().collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "mcp_server");
        assert!(failures[0].1.contains("channel closed"));
        assert_eq!(*log.lock().unwrap(), vec!["slash_command", "registry_row"]);
    }

    #[tokio::test]
    async fn a_panicking_disposer_is_recorded_and_the_rest_still_run() {
        let (log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        scope.effect("registry_row", mk("registry_row"));
        scope.effect(
            "wasm_module",
            async_disposer(|| async { panic!("guest unload exploded") }),
        );
        scope.effect(
            "service",
            sync_disposer(|| -> DisposeOutcome { panic!("sync boom") }),
        );
        let report = scope.dispose().await;
        let failures: Vec<_> = report.failures().collect();
        assert_eq!(failures.len(), 2, "{report:?}");
        assert!(failures
            .iter()
            .any(|(s, e)| *s == "wasm_module" && e.contains("guest unload exploded")));
        assert!(failures
            .iter()
            .any(|(s, e)| *s == "service" && e.contains("sync boom")));
        assert_eq!(*log.lock().unwrap(), vec!["registry_row"]);
    }

    #[tokio::test]
    async fn len_counts_effects_not_skips() {
        let (_log, mk) = recorder();
        let mut scope = EffectScope::new("p");
        assert!(scope.is_empty());
        scope.effect("registry_row", mk("registry_row"));
        scope.skip("mcp_server", "MCP manager not attached");
        assert_eq!(scope.len(), 1);
        assert_eq!(scope.skipped().len(), 1);
        assert_eq!(scope.skipped()[0].0, "mcp_server");
        assert_eq!(scope.steps(), vec!["registry_row"]);
        assert_eq!(scope.plugin_id(), "p");
    }

    #[test]
    fn every_step_label_is_distinct_and_in_mount_order() {
        let mut seen = std::collections::HashSet::new();
        for l in STEP_LABELS {
            assert!(seen.insert(l), "duplicate label {l}");
        }
        assert_eq!(
            STEP_LABELS[0], "registry_row",
            "the row is first in, last out"
        );
        assert_eq!(STEP_LABELS[5], "slash_command");
    }
}
