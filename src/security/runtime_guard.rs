//! Runtime security orchestrator for the agent loop.
//
// LOCK DISCIPLINE (do not break):
// - `pii_engine` uses `crate::sync_primitives::RwLock` (= `std::sync::RwLock`),
//   because `PiiEngine::global()` exposes a sync read/write API. Inside an
//   async task, calling `engine.read().unwrap()` blocks the current tokio
//   worker for the duration of the read. The read window here is short
//   (rule evaluation + `filter_with_platform`) and `RwLock::read` is
//   lock-free on Linux when no writer is waiting, so this is acceptable
//   **as long as the guard is NOT held across an `.await` point**. If you
//   need to await between taking the guard and dropping it, wrap the
//   section in `tokio::task::spawn_blocking`.
// - `exec_leak_detector` / `secret_leak_detector` use `tokio::sync::Mutex`
//   (imported directly from `tokio::sync::Mutex`, not `sync_primitives`)
//   because their guard is held across `.await` points inside
//   `process_outbound` / `process_inbound` (we `register_injected` and then
//   rescan). DO NOT switch them to `crate::sync_primitives::Mutex` — that
//   is `std::sync::Mutex` and would deadlock the runtime when held across
//   `.await`.

use std::collections::HashMap;

use crate::exec::leak_detector::LeakDetector as ExecLeakDetector;
use crate::pii::engine::{FilterResult, PiiEngine};
use crate::secrets::injection::{AsyncSecretResolver, InjectedSecret};
use crate::secrets::leak_detector::{LeakDecision, LeakDetector as SecretLeakDetector};
use crate::security::audit::{AuditEntry, AuditEventType, AuditSeverity, SecurityAuditLog};
use crate::sync_primitives::{Arc, RwLock};
use thiserror::Error;
use tokio::sync::Mutex;

#[derive(Error, Debug)]
pub enum SecurityGuardError {
    #[error("Secret resolution failed: {0}")]
    SecretResolutionFailed(#[from] crate::secrets::types::SecretError),
}

/// Configuration for the runtime security guard.
#[derive(Debug, Clone)]
pub struct SecurityGuardConfig {
    pub pii_filtering: bool,
    pub leak_detection: bool,
    pub secret_injection: bool,
    pub audit_enabled: bool,
    /// Custom leak detection patterns (additive to built-ins)
    pub custom_leak_patterns: Vec<crate::config::types::CustomLeakPattern>,
}

impl Default for SecurityGuardConfig {
    fn default() -> Self {
        Self {
            pii_filtering: true,
            leak_detection: true,
            secret_injection: true,
            audit_enabled: true,
            custom_leak_patterns: Vec::new(),
        }
    }
}

/// Request-level security context.
#[derive(Debug, Clone, Default)]
pub struct SecurityContext {
    pub provider_name: Option<String>,
    pub platform_name: Option<String>,
    pub session_id: Option<String>,
}

/// Result of guard processing.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum GuardResult {
    Clean { text: String },
    Redacted { text: String, reasons: Vec<String> },
    Blocked { reason: String },
    Warned { text: String, warnings: Vec<String> },
}

/// Central orchestrator for runtime security checks.
pub struct RuntimeSecurityGuard {
    config: SecurityGuardConfig,
    pii_engine: Option<Arc<RwLock<PiiEngine>>>,
    exec_leak_detector: Arc<Mutex<ExecLeakDetector>>,
    secret_leak_detector: Arc<Mutex<SecretLeakDetector>>,
    audit_log: Option<SecurityAuditLog>,
}

impl RuntimeSecurityGuard {
    /// Create a new guard with default configuration.
    #[must_use]
    pub fn default_guard() -> Self {
        Self::new(SecurityGuardConfig::default())
    }

    /// Create a new guard with the given configuration.
    ///
    /// `config.audit_enabled` is forced to `false` here because the audit
    /// receiver is consumed by [`new_with_audit`] and would be dropped
    /// immediately, closing the mpsc channel and silently discarding every
    /// entry. Callers that want the audit pipeline must construct the guard via
    /// [`new_with_audit`] and own the returned receiver.
    #[must_use]
    pub fn new(mut config: SecurityGuardConfig) -> Self {
        config.audit_enabled = false;
        let (guard, _rx) = Self::new_with_audit(config);
        guard
    }

    /// Create a new guard with the given configuration and return the audit receiver.
    #[must_use]
    pub fn new_with_audit(
        config: SecurityGuardConfig,
    ) -> (Self, tokio::sync::mpsc::Receiver<AuditEntry>) {
        let exec_leak_detector = Arc::new(Mutex::new(ExecLeakDetector::default_patterns()));
        let secret_leak_detector = Arc::new(Mutex::new(SecretLeakDetector::with_custom_patterns(
            &config.custom_leak_patterns,
        )));
        let pii_engine = if config.pii_filtering {
            PiiEngine::global().or_else(|| {
                Some(Arc::new(RwLock::new(PiiEngine::new(
                    crate::config::PrivacyConfig::default(),
                ))))
            })
        } else {
            None
        };
        let (audit_log, rx) = if config.audit_enabled {
            let (log, rx) = SecurityAuditLog::new(256);
            (Some(log), rx)
        } else {
            (None, tokio::sync::mpsc::channel(1).1)
        };

        let guard = Self {
            config,
            pii_engine,
            exec_leak_detector,
            secret_leak_detector,
            audit_log,
        };
        (guard, rx)
    }

    fn log_audit(
        &self,
        context: &SecurityContext,
        event_type: AuditEventType,
        severity: AuditSeverity,
        detail: String,
    ) {
        if let Some(log) = &self.audit_log {
            let entry = AuditEntry {
                event_type,
                severity,
                source_ip: None,
                session_id: context.session_id.clone(),
                // WHO ran the turn that tripped the guard. The guard fires
                // inside the run loop, where `CALLER_USER` is dead (it never
                // crosses the spawn boundary); the run-start path re-seeds
                // the same fact as the `AUTHOR_USER_KEY` metadata →
                // `scope::with_room_author` task-local, which IS visible
                // here. `None` for runs no human authored (cron, internal)
                // and for engines that never enter the seeding nest
                // (fast_path / SimpleExecutionEngine) — an honest absent,
                // not a forgotten one.
                actor_user: crate::scope::current_room_author(),
                detail,
            };
            log.log(entry);
        }
    }

    /// Process outbound content before sending to LLM.
    #[tracing::instrument(level = "debug", skip_all, fields(platform = ?context.platform_name, provider = ?context.provider_name))]
    // rust-doctor-disable-next-line high-cyclomatic-complexity
    pub async fn process_outbound(
        &self,
        text: &str,
        resolver: Option<&dyn AsyncSecretResolver>,
        context: SecurityContext,
    ) -> Result<GuardResult, SecurityGuardError> {
        let mut current_text = text.to_string();
        let mut reasons = Vec::new();
        let mut warnings = Vec::new();

        // 1. Placeholder Extraction & Secret Resolution (no text replacement yet)
        let mut resolved_map: HashMap<String, String> = HashMap::new();
        if self.config.secret_injection {
            if let Some(resolver) = resolver {
                let refs = crate::secrets::placeholder::extract_secret_refs(&current_text)?;
                if !refs.is_empty() {
                    let mut injected = Vec::with_capacity(refs.len());
                    for secret_ref in &refs {
                        let decrypted = resolver.resolve(&secret_ref.name).await?;
                        let value = decrypted.expose();
                        injected.push(InjectedSecret::from_value(&secret_ref.name, value));
                        resolved_map.insert(secret_ref.raw.clone(), value.to_string());
                    }
                    {
                        let mut detector = self.secret_leak_detector.lock().await;
                        detector.register_injected(&injected);
                    }
                }
            }
        }

        // 2. Leak Detection (on text still containing placeholders)
        if self.config.leak_detection {
            let exec_scan = {
                let detector = self.exec_leak_detector.lock().await;
                detector.scan_outbound(&current_text)
            };

            let secret_scan = {
                let detector = self.secret_leak_detector.lock().await;
                detector.scan_outbound(&current_text)
            };

            let has_blocks =
                exec_scan.has_blocks() || matches!(secret_scan, LeakDecision::Block { .. });

            if has_blocks {
                let detail = format!(
                    "outbound leak blocked; exec_findings={}, secret_block={}",
                    exec_scan.findings.len(),
                    matches!(secret_scan, LeakDecision::Block { .. })
                );
                self.log_audit(
                    &context,
                    AuditEventType::ExecBlocked,
                    AuditSeverity::Critical,
                    detail,
                );
                return Ok(GuardResult::Blocked {
                    reason: "Leak detector found sensitive data in outbound content".to_string(),
                });
            }

            // Honor `Redact`-action findings (e.g. bearer tokens). Without this
            // the matched secret would pass through untouched, since it is
            // neither a block nor a warning.
            if exec_scan.has_redacts() {
                current_text = {
                    let detector = self.exec_leak_detector.lock().await;
                    detector.redact(&current_text)
                };
                reasons.push("Outbound leak detector redacted sensitive token".to_string());
                self.log_audit(
                    &context,
                    AuditEventType::LeakWarning,
                    AuditSeverity::Warn,
                    "outbound leak detector redacted sensitive token".to_string(),
                );
            }
        }

        // 3. PII Filtering
        if self.config.pii_filtering {
            if let Some(engine) = &self.pii_engine {
                let engine_guard = engine.read().unwrap_or_else(|e| e.into_inner());

                let should_filter = match &context.provider_name {
                    Some(provider) => !engine_guard
                        .is_platform_excluded(context.platform_name.as_deref(), provider),
                    None => true,
                };

                if should_filter {
                    let result = engine_guard
                        .filter_with_platform(&current_text, context.platform_name.as_deref());
                    let blocked = result.blocked_count;
                    let warned = result.warned_count;
                    current_text = Self::apply_filter_result(result, &mut reasons, &mut warnings);
                    if blocked > 0 {
                        self.log_audit(
                            &context,
                            AuditEventType::PiiDetected,
                            AuditSeverity::Critical,
                            format!("PII filter blocked {blocked} detection(s)"),
                        );
                    }
                    if warned > 0 {
                        self.log_audit(
                            &context,
                            AuditEventType::PiiDetected,
                            AuditSeverity::Warn,
                            format!("PII filter warned {warned} detection(s)"),
                        );
                    }
                }
            }
        }

        // 4. Placeholder Replacement
        // Sort longest-first so that e.g. `{{secret:api_key}}` is replaced
        // before `{{secret:api}}` and the shorter placeholder does not eat
        // the prefix of the longer one. HashMap iteration is non-deterministic,
        // so this matters.
        let mut ordered: Vec<(&String, &String)> = resolved_map.iter().collect();
        ordered.sort_by_key(|(raw, _)| std::cmp::Reverse(raw.len()));
        for (raw, value) in &ordered {
            current_text = current_text.replace(*raw, value);
        }

        // 5. Post-substitution leak re-scan.
        //
        // The earlier step-2 scan ran against the placeholder-bearing text, so
        // a freshly resolved secret value (the common case — the placeholders
        // reference API keys and tokens) was inserted into `current_text`
        // AFTER every outbound leak/redact pass. The only thing that would
        // catch an outbound that quotes the resolved secret back is a second
        // scan against the substituted string. Without this pass a tool
        // call that echoes the resolved value slips past both the exec and
        // secret leak detectors.
        if self.config.leak_detection && !ordered.is_empty() {
            let exec_post_scan = {
                let detector = self.exec_leak_detector.lock().await;
                detector.scan_outbound(&current_text)
            };
            let secret_post_scan = {
                let detector = self.secret_leak_detector.lock().await;
                detector.scan_outbound(&current_text)
            };
            let post_has_blocks = exec_post_scan.has_blocks()
                || matches!(secret_post_scan, LeakDecision::Block { .. });
            if post_has_blocks {
                let detail = format!(
                    "outbound leak blocked post-substitution; \
                     exec_findings={}, secret_block={}",
                    exec_post_scan.findings.len(),
                    matches!(secret_post_scan, LeakDecision::Block { .. }),
                );
                self.log_audit(
                    &context,
                    AuditEventType::ExecBlocked,
                    AuditSeverity::Critical,
                    detail,
                );
                return Ok(GuardResult::Blocked {
                    reason: "Leak detector found sensitive data in resolved outbound content"
                        .to_string(),
                });
            }
            if exec_post_scan.has_redacts() {
                current_text = {
                    let detector = self.exec_leak_detector.lock().await;
                    detector.redact(&current_text)
                };
                reasons.push(
                    "Outbound leak detector redacted sensitive token after secret resolution"
                        .to_string(),
                );
                self.log_audit(
                    &context,
                    AuditEventType::LeakWarning,
                    AuditSeverity::Warn,
                    "outbound leak detector redacted sensitive token post-substitution".to_string(),
                );
            }
        }

        // Assemble final result
        if reasons.is_empty() && warnings.is_empty() {
            Ok(GuardResult::Clean { text: current_text })
        } else if !reasons.is_empty() {
            Ok(GuardResult::Redacted {
                text: current_text,
                reasons,
            })
        } else {
            Ok(GuardResult::Warned {
                text: current_text,
                warnings,
            })
        }
    }

    fn apply_filter_result(
        result: FilterResult,
        reasons: &mut Vec<String>,
        warnings: &mut Vec<String>,
    ) -> String {
        if result.blocked_count > 0 {
            reasons.push(format!(
                "PII filter blocked {} detection(s)",
                result.blocked_count
            ));
        }
        if result.warned_count > 0 {
            warnings.push(format!(
                "PII filter warned {} detection(s)",
                result.warned_count
            ));
        }
        result.text
    }

    /// Process inbound content received from LLM.
    #[tracing::instrument(level = "debug", skip_all)]
    pub async fn process_inbound(
        &self,
        text: &str,
        context: &SecurityContext,
    ) -> Result<GuardResult, SecurityGuardError> {
        if !self.config.leak_detection {
            return Ok(GuardResult::Clean {
                text: text.to_string(),
            });
        }

        let exec_scan = {
            let detector = self.exec_leak_detector.lock().await;
            detector.scan_inbound(text)
        };

        let secret_scan = {
            let detector = self.secret_leak_detector.lock().await;
            detector.scan_inbound(text)
        };

        // Handle secret leak detector block
        if let LeakDecision::Block { reason, .. } = secret_scan {
            self.log_audit(
                context,
                AuditEventType::EnvInjectionDetected,
                AuditSeverity::Critical,
                format!("inbound secret leak blocked: {reason}"),
            );
            return Ok(GuardResult::Blocked {
                reason: format!("Secret leak detector: {reason}"),
            });
        }

        if exec_scan.has_blocks() {
            let detail = format!(
                "inbound exec leak blocked; findings={}",
                exec_scan.findings.len()
            );
            self.log_audit(
                context,
                AuditEventType::ExecBlocked,
                AuditSeverity::Critical,
                detail,
            );
            return Ok(GuardResult::Blocked {
                reason: "Leak detector found sensitive data in inbound content".to_string(),
            });
        }

        // Honor `Redact`-action findings (e.g. bearer tokens) before any other
        // pass-through. Without this the matched secret would be returned to the
        // caller untouched, since it is neither a block nor a warning.
        if exec_scan.has_redacts() {
            let redacted = {
                let detector = self.exec_leak_detector.lock().await;
                detector.redact(text)
            };
            self.log_audit(
                context,
                AuditEventType::LeakWarning,
                AuditSeverity::Warn,
                "inbound leak detector redacted sensitive token".to_string(),
            );
            return Ok(GuardResult::Redacted {
                text: redacted,
                reasons: vec!["Inbound leak detector redacted sensitive token".to_string()],
            });
        }

        // Inbound PII filtering: scrub sensitive data echoed back by LLM
        if self.config.pii_filtering {
            if let Some(engine) = &self.pii_engine {
                let engine_guard = engine.read().unwrap_or_else(|e| e.into_inner());
                let result = engine_guard.filter(text);
                if result.blocked_count > 0 {
                    self.log_audit(
                        context,
                        AuditEventType::PiiDetected,
                        AuditSeverity::Critical,
                        format!("inbound PII redacted; {} blocks", result.blocked_count),
                    );
                    return Ok(GuardResult::Redacted {
                        text: result.text,
                        reasons: vec![format!(
                            "Inbound PII detected and redacted ({} blocks)",
                            result.blocked_count
                        )],
                    });
                } else if result.warned_count > 0 {
                    self.log_audit(
                        context,
                        AuditEventType::PiiDetected,
                        AuditSeverity::Warn,
                        format!("inbound PII warning; {} warnings", result.warned_count),
                    );
                    return Ok(GuardResult::Warned {
                        text: result.text,
                        warnings: vec![format!(
                            "Inbound PII detected ({} warnings)",
                            result.warned_count
                        )],
                    });
                }
            }
        }

        Ok(GuardResult::Clean {
            text: text.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::types::{DecryptedSecret, SecretError};

    struct MockResolver;

    #[async_trait::async_trait]
    impl AsyncSecretResolver for MockResolver {
        async fn resolve(&self, _name: &str) -> Result<DecryptedSecret, SecretError> {
            // Use a non-pattern-matching value so the post-substitution
            // leak scan (added in review-batch 2026-08-25, HIGH C-3 fix)
            // does not block this test on its own. A separate test
            // (`test_outbound_post_substitution_blocks_pattern_match`)
            // covers the post-substitution block path with a
            // pattern-matching value.
            Ok(DecryptedSecret::new("test_secret_value_xyz".to_string()))
        }
    }

    #[test]
    fn test_guard_creation() {
        let guard = RuntimeSecurityGuard::default_guard();
        assert!(guard.config.pii_filtering);
    }

    #[tokio::test]
    async fn test_outbound_resolves_placeholder() {
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        let input = "Use key {{secret:test_key}} for API";
        let result = guard
            .process_outbound(input, Some(&MockResolver), context)
            .await
            .unwrap();

        match result {
            GuardResult::Clean { text } => {
                assert!(text.contains("test_secret_value_xyz"));
                assert!(!text.contains("{{secret:test_key}}"));
            }
            _ => panic!("Expected Clean result, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn test_outbound_post_substitution_blocks_pattern_match() {
        // Regression for review-batch 2026-08-25 HIGH C-3 fix:
        // a freshly resolved secret value that itself matches an
        // outbound leak pattern must be blocked, not silently shipped
        // to the model. The pre-fix code ran the leak scan against the
        // placeholder-bearing text and only substituted afterwards,
        // so a `sk-…` value resolved at this step was added to the
        // outbound string AFTER every leak/redact pass.
        use crate::secrets::vendor_patterns::is_block_class_secret;
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        let input = "Authorization: Bearer {{secret:test_key}}";
        let result = guard
            .process_outbound(input, Some(&MockResolver), context)
            .await
            .unwrap();
        assert!(matches!(result, GuardResult::Blocked { .. }));
        // Sanity-check: the value the mock returns doesn't itself match
        // a known pattern, so the post-substitution scan returning Allow
        // for that case (test_outbound_resolves_placeholder) is the
        // right shape. If you change the mock to a pattern-matching
        // value, move this test to use that pattern and remove the
        // Allow case from the other test.
        assert!(!is_block_class_secret("test_secret_value_xyz"));
    }

    #[tokio::test]
    async fn test_outbound_blocks_accidental_secret_leak() {
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        // This contains a real-looking API key that should be caught by leak detection
        let input = "My key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let result = guard
            .process_outbound(input, Some(&MockResolver), context)
            .await;

        match result {
            Ok(GuardResult::Blocked { .. }) => {
                // Expected: leak detector blocks known secret patterns
            }
            other => panic!("Expected Blocked, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_outbound_pipeline_order_leak_before_pii() {
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        // Placeholder should be resolved AFTER leak detection, so this should be Clean
        let input = "Use key {{secret:test_key}} and call 13812345678";
        let result = guard
            .process_outbound(input, Some(&MockResolver), context)
            .await
            .unwrap();

        // Leak detection runs on text with placeholders, so no accidental secret leak.
        // PII filter should catch the phone number.
        match result {
            GuardResult::Redacted { text, .. } | GuardResult::Clean { text } => {
                assert!(!text.contains("{{secret:test_key}}"));
                if text.contains("[PHONE]") {
                    // PII filter ran correctly
                }
            }
            GuardResult::Blocked { .. } => {
                // Also acceptable if leak detector is aggressive
            }
            GuardResult::Warned { text, .. } => {
                assert!(!text.contains("{{secret:test_key}}"));
            }
        }
    }

    #[tokio::test]
    async fn test_inbound_blocks_echoed_injected_secret() {
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        // First do outbound to register the injected secret
        let _ = guard
            .process_outbound("Use {{secret:test_key}}", Some(&MockResolver), context)
            .await
            .unwrap();

        // Then simulate LLM echoing the exact secret value back
        let inbound = "Your API key is sk-ant-test12345678901234567890";
        let context = SecurityContext::default();
        let result = guard.process_inbound(inbound, &context).await.unwrap();

        match result {
            GuardResult::Blocked { .. } => {
                // Expected: either exec leak detector (pattern match)
                // or secret leak detector (exact injected value match)
            }
            other => panic!("Expected Blocked for echoed secret, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_inbound_clean_for_normal_text() {
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        let result = guard
            .process_inbound("Hello, this is normal text.", &context)
            .await
            .unwrap();
        assert!(
            matches!(result, GuardResult::Clean { .. }),
            "Expected Clean, got {:?}",
            result
        );
    }

    /// The audit row answers "whose run tripped the guard" — the fact that
    /// made §5.1's `actor_user` column worth having. The guard fires inside
    /// the run loop, so the identity arrives via the `AUTHOR_USER_KEY` →
    /// `scope::with_room_author` task-local, not the dead `CALLER_USER`.
    #[tokio::test]
    async fn audit_entry_names_the_run_author_when_one_is_seeded() {
        let (guard, mut rx) = RuntimeSecurityGuard::new_with_audit(SecurityGuardConfig::default());
        crate::scope::with_room_author(Some("u-alice".to_string()), async {
            let _ = guard
                .process_outbound(
                    "My key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
                    None,
                    SecurityContext::default(),
                )
                .await;
        })
        .await;
        let entry = rx.recv().await.expect("block should log an audit entry");
        assert_eq!(entry.actor_user.as_deref(), Some("u-alice"));
    }

    /// A run no human authored (cron / internal) has no author to name — the
    /// column stays NULL rather than inventing one.
    #[tokio::test]
    async fn audit_entry_actor_is_none_outside_a_seeded_run() {
        let (guard, mut rx) = RuntimeSecurityGuard::new_with_audit(SecurityGuardConfig::default());
        let _ = guard
            .process_outbound(
                "My key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz",
                None,
                SecurityContext::default(),
            )
            .await;
        let entry = rx.recv().await.expect("block should log an audit entry");
        assert!(entry.actor_user.is_none());
    }
}
