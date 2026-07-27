//! `ResourceGovernor` — load-aware admission gate for heavy local execution.
//!
//! Closes the third leg of the harness-governance triad (state · goal-loop ·
//! resources). It is the Rust-native, type-safe descendant of the shell
//! `check_and_run_cargo` pattern used to drive prior development sessions:
//!
//! ```text
//! check_and_run_cargo() {            ResourceGovernor::admit()
//!   while true; do                     loop {
//!     count=$(pgrep -c cargo)            let s = probe.sample();
//!     if [ $count -lt 3 ]; then         if thresholds.within(&s) {
//!       cargo "$cmd"; break               return Admit;
//!     else                              } else if past deadline {
//!       sleep 10                          return Reject;   // (better than the
//!     fi                                } else {           //  shell: never hangs
//!   done                                  sleep(backoff).await; //  forever)
//! }                                     }
//! ```
//!
//! ## Where it mounts (wire-first)
//!
//! It is a [`SandboxBeforeHook`], registered in
//! [`crate::sandbox::factory::build_sandbox`] alongside the existing
//! rate-limit / command-policy hooks. `Sandbox::execute` already awaits
//! `run_before` at the top of every sandboxed command, so the gate needs
//! **zero** changes to the execution path — it simply delays (or denies) the
//! `before` result until the machine can take the load.
//!
//! ## Scope (KISS / R3)
//!
//! Every command reaching `WorkspaceSandbox::execute` is already a real OS
//! subprocess spawn — the sandbox path is used exclusively by exec-class
//! tools (`code_exec` / `bash_exec`). That *is* the heavy class
//! `check_and_run_cargo` governed, so the gate applies to all of them when
//! enabled rather than re-deriving a category from the program name (which,
//! at this seam, is the interpreter — `bash` / `python` — not the tool).
//! When disabled (the default) the hook is a zero-cost pass-through.
//!
//! ## Honesty about the signal
//!
//! This is a *stateless ambient-load* gate: it samples OS-wide memory/CPU at
//! admission time rather than holding a permit for the command's lifetime.
//! That mirrors `check_and_run_cargo` (which sampled `pgrep`, held nothing)
//! and is leak-free — being stateless it holds no permit, so there is no
//! counter that could drift. A held-permit semaphore would be more precise
//! under bursts but requires execution-path surgery; deferred (YAGNI) until a
//! real burst problem is observed.

use std::time::Duration;

use async_trait::async_trait;
use tokio::time::{sleep, Instant};

use crate::sandbox::hooks::{SandboxBeforeHook, SandboxHookContext, SandboxHookResult};

/// A single reading of ambient system load.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadSample {
    /// Global CPU utilisation across all cores, as a percentage (0..=100).
    pub cpu_usage_percent: f32,
    /// Free system memory available for new allocations, in MiB.
    pub available_memory_mb: u64,
}

/// Source of ambient load readings. Abstracted as a trait so the gate is
/// unit-testable with a deterministic mock and so the OS-specific sampling
/// (cross-platform via `sysinfo`) is swappable.
pub trait LoadProbe: Send + Sync {
    /// Take an instantaneous reading. Must be cheap and non-blocking.
    fn sample(&self) -> LoadSample;
}

/// Soft thresholds the machine must satisfy to admit a heavy task. A `None`
/// field disables that dimension (so an all-`None` config is a no-op gate).
#[derive(Debug, Clone, Copy)]
pub struct GovernorThresholds {
    /// Refuse admission while free memory is below this many MiB.
    pub min_available_memory_mb: Option<u64>,
    /// Refuse admission while global CPU usage is above this percentage.
    pub max_cpu_percent: Option<f32>,
}

impl GovernorThresholds {
    /// `true` when the sample is within every configured threshold.
    #[must_use]
    pub fn within(&self, s: &LoadSample) -> bool {
        if let Some(min_mem) = self.min_available_memory_mb {
            if s.available_memory_mb < min_mem {
                return false;
            }
        }
        if let Some(max_cpu) = self.max_cpu_percent {
            if s.cpu_usage_percent > max_cpu {
                return false;
            }
        }
        true
    }
}

/// Outcome of an admission attempt. Exhaustive enum (vs. the shell's
/// stringly-typed loop) so callers handle both arms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionDecision {
    /// The machine can take the load — proceed.
    Admit,
    /// Load stayed too high for `max_wait`; the task is refused so the loop
    /// never hangs indefinitely (the key improvement over the shell version).
    Reject { reason: String },
}

/// Load-aware admission gate. Cheap to clone-share via `Arc`; holds no
/// per-task state.
pub struct ResourceGovernor {
    enabled: bool,
    probe: std::sync::Arc<dyn LoadProbe>,
    thresholds: GovernorThresholds,
    /// Maximum time to wait for load to subside before rejecting.
    max_wait: Duration,
    /// Delay between load re-samples while waiting.
    backoff: Duration,
}

impl ResourceGovernor {
    /// Build from the serde config schema, using the real `sysinfo` probe.
    #[must_use]
    pub fn from_schema(schema: &crate::sandbox::config::ResourceGovernorConfigSchema) -> Self {
        Self {
            enabled: schema.enabled,
            probe: std::sync::Arc::new(SysinfoLoadProbe::new()),
            thresholds: GovernorThresholds {
                min_available_memory_mb: schema.min_available_memory_mb,
                max_cpu_percent: schema.max_cpu_percent,
            },
            max_wait: Duration::from_secs(schema.max_wait_secs),
            backoff: Duration::from_millis(schema.backoff_ms.max(1)),
        }
    }

    /// Test/alternate constructor injecting a custom probe.
    pub fn with_probe(
        probe: std::sync::Arc<dyn LoadProbe>,
        thresholds: GovernorThresholds,
        max_wait: Duration,
        backoff: Duration,
    ) -> Self {
        Self {
            enabled: true,
            probe,
            thresholds,
            max_wait,
            backoff: backoff.max(Duration::from_millis(1)),
        }
    }

    /// `true` when this governor should be registered as a hook at all.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The `check_and_run_cargo` loop: sample load, admit when the machine
    /// can take it, otherwise back off and re-sample until `max_wait`.
    pub async fn admit(&self) -> AdmissionDecision {
        let deadline = Instant::now() + self.max_wait;
        loop {
            let sample = self.probe.sample();
            if self.thresholds.within(&sample) {
                return AdmissionDecision::Admit;
            }
            if Instant::now() >= deadline {
                return AdmissionDecision::Reject {
                    reason: format!(
                        "system under load (cpu={:.0}% avail_mem={}MiB); could not \
                         admit heavy task within {:?}",
                        sample.cpu_usage_percent, sample.available_memory_mb, self.max_wait
                    ),
                };
            }
            sleep(self.backoff).await;
        }
    }
}

#[async_trait]
impl SandboxBeforeHook for ResourceGovernor {
    fn name(&self) -> &'static str {
        "sandbox.resource_governor"
    }

    async fn before(&self, ctx: SandboxHookContext<'_>) -> SandboxHookResult {
        // Dormant unless enabled. Every command at this seam is a sandboxed
        // subprocess spawn → all are gated when the governor is on.
        if !self.enabled {
            return SandboxHookResult::Allow;
        }
        match self.admit().await {
            AdmissionDecision::Admit => SandboxHookResult::Allow,
            AdmissionDecision::Reject { reason } => {
                tracing::warn!(
                    target: "resource_governor",
                    tool_name = ctx.tool_name,
                    reason = %reason,
                    "heavy task refused by resource governor"
                );
                SandboxHookResult::Deny { reason }
            }
        }
    }
}

/// Real OS probe backed by `sysinfo` (cross-platform; already a dependency
/// and used by `gateway::memory_monitor` / `system_info`). Holds a persistent
/// `System` so consecutive CPU samples are spaced enough to be meaningful.
pub struct SysinfoLoadProbe {
    sys: crate::sync_primitives::Mutex<sysinfo::System>,
}

impl SysinfoLoadProbe {
    #[must_use]
    pub fn new() -> Self {
        let mut sys = sysinfo::System::new();
        sys.refresh_cpu_all();
        Self {
            sys: crate::sync_primitives::Mutex::new(sys),
        }
    }
}

impl Default for SysinfoLoadProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl LoadProbe for SysinfoLoadProbe {
    fn sample(&self) -> LoadSample {
        // Poison-safe lock per P7 (defensive design).
        let mut sys = self.sys.lock().unwrap_or_else(|e| e.into_inner());
        sys.refresh_memory();
        sys.refresh_cpu_all();
        LoadSample {
            cpu_usage_percent: sys.global_cpu_usage(),
            available_memory_mb: sys.available_memory() / (1024 * 1024),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Probe returning a fixed sample.
    struct FixedProbe(LoadSample);
    impl LoadProbe for FixedProbe {
        fn sample(&self) -> LoadSample {
            self.0
        }
    }

    /// Probe that reports "loaded" for the first `n` samples, then "idle".
    struct RecoveringProbe {
        loaded_samples: usize,
        seen: AtomicUsize,
    }
    impl LoadProbe for RecoveringProbe {
        fn sample(&self) -> LoadSample {
            let i = self.seen.fetch_add(1, Ordering::SeqCst);
            if i < self.loaded_samples {
                LoadSample {
                    cpu_usage_percent: 99.0,
                    available_memory_mb: 64,
                }
            } else {
                LoadSample {
                    cpu_usage_percent: 1.0,
                    available_memory_mb: 8192,
                }
            }
        }
    }

    fn thresholds() -> GovernorThresholds {
        GovernorThresholds {
            min_available_memory_mb: Some(512),
            max_cpu_percent: Some(90.0),
        }
    }

    #[test]
    fn within_respects_each_dimension() {
        let t = thresholds();
        assert!(t.within(&LoadSample {
            cpu_usage_percent: 10.0,
            available_memory_mb: 4096
        }));
        // Low memory fails.
        assert!(!t.within(&LoadSample {
            cpu_usage_percent: 10.0,
            available_memory_mb: 100
        }));
        // High CPU fails.
        assert!(!t.within(&LoadSample {
            cpu_usage_percent: 95.0,
            available_memory_mb: 4096
        }));
    }

    #[test]
    fn within_all_none_is_noop_pass() {
        let t = GovernorThresholds {
            min_available_memory_mb: None,
            max_cpu_percent: None,
        };
        assert!(t.within(&LoadSample {
            cpu_usage_percent: 100.0,
            available_memory_mb: 0
        }));
    }

    #[tokio::test(start_paused = true)]
    async fn admit_immediately_when_load_low() {
        let gov = ResourceGovernor::with_probe(
            Arc::new(FixedProbe(LoadSample {
                cpu_usage_percent: 5.0,
                available_memory_mb: 8192,
            })),
            thresholds(),
            Duration::from_secs(30),
            Duration::from_millis(500),
        );
        assert_eq!(gov.admit().await, AdmissionDecision::Admit);
    }

    #[tokio::test(start_paused = true)]
    async fn admit_rejects_after_deadline_under_persistent_load() {
        let gov = ResourceGovernor::with_probe(
            Arc::new(FixedProbe(LoadSample {
                cpu_usage_percent: 99.0,
                available_memory_mb: 64,
            })),
            thresholds(),
            Duration::from_secs(5),
            Duration::from_millis(500),
        );
        // Paused-time auto-advance drives the backoff loop to the deadline.
        match gov.admit().await {
            AdmissionDecision::Reject { reason } => {
                assert!(reason.contains("under load"), "reason: {reason}");
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn admit_waits_then_admits_when_load_subsides() {
        let gov = ResourceGovernor::with_probe(
            Arc::new(RecoveringProbe {
                loaded_samples: 3,
                seen: AtomicUsize::new(0),
            }),
            thresholds(),
            Duration::from_secs(60),
            Duration::from_millis(500),
        );
        // First 3 samples are loaded → waits; 4th is idle → admits.
        assert_eq!(gov.admit().await, AdmissionDecision::Admit);
    }

    #[tokio::test]
    async fn before_passes_through_when_disabled() {
        // A disabled governor must never consult the probe or gate anything,
        // even with a probe that would always reject.
        let mut gov = ResourceGovernor::with_probe(
            Arc::new(FixedProbe(LoadSample {
                cpu_usage_percent: 100.0,
                available_memory_mb: 0,
            })),
            thresholds(),
            Duration::from_millis(1),
            Duration::from_millis(1),
        );
        gov.enabled = false;
        let cmd = mk_cmd("bash");
        let ctx = SandboxHookContext::new("bash", &cmd);
        assert!(matches!(gov.before(ctx).await, SandboxHookResult::Allow));
    }

    #[tokio::test(start_paused = true)]
    async fn before_denies_under_load() {
        let gov = ResourceGovernor::with_probe(
            Arc::new(FixedProbe(LoadSample {
                cpu_usage_percent: 100.0,
                available_memory_mb: 0,
            })),
            thresholds(),
            Duration::from_secs(2),
            Duration::from_millis(500),
        );
        // `program` at this seam is the interpreter — any value is gated.
        let cmd = mk_cmd("bash");
        let ctx = SandboxHookContext::new("bash", &cmd);
        assert!(matches!(
            gov.before(ctx).await,
            SandboxHookResult::Deny { .. }
        ));
    }

    fn mk_cmd(program: &str) -> crate::sandbox::command::SandboxCommand {
        crate::sandbox::command::SandboxCommand {
            session_id: crate::routing::session_key::SessionKey::ephemeral("test"),
            program: program.into(),
            args: vec![],
            env: std::collections::HashMap::new(),
            stdin: None,
            cwd: None,
            capabilities: crate::sandbox::capabilities::SandboxCapabilities::default(),
            timeout: None,
        }
    }
}
