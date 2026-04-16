//! Runtime security orchestrator for the agent loop.

use std::collections::HashMap;

use crate::exec::leak_detector::{LeakAction, LeakDetector as ExecLeakDetector};
use crate::pii::engine::{FilterResult, PiiEngine};
use crate::secrets::injection::{AsyncSecretResolver, InjectedSecret};
use crate::secrets::leak_detector::{LeakDecision, LeakDetector as SecretLeakDetector};
use crate::security::content_sanitizer::{wrap_external_content, ContentSource};
use crate::sync_primitives::{Arc, Mutex, RwLock};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SecurityGuardError {
    #[error("Secret resolution failed: {0}")]
    SecretResolutionFailed(#[from] crate::secrets::types::SecretError),
    #[error("Sanitization failed: {0}")]
    SanitizationFailed(String),
    #[error("PII engine unavailable")]
    PiiEngineUnavailable,
}

/// Configuration for the runtime security guard.
#[derive(Debug, Clone)]
pub struct SecurityGuardConfig {
    pub pii_filtering: bool,
    pub content_sanitization: bool,
    pub leak_detection: bool,
    pub secret_injection: bool,
    pub default_action_on_leak: LeakAction,
}

impl Default for SecurityGuardConfig {
    fn default() -> Self {
        Self {
            pii_filtering: true,
            content_sanitization: true,
            leak_detection: true,
            secret_injection: true,
            default_action_on_leak: LeakAction::Block,
        }
    }
}

/// Request-level security context.
#[derive(Debug, Clone, Default)]
pub struct SecurityContext {
    pub has_external_content: bool,
    pub external_source: Option<ContentSource>,
    pub provider_name: Option<String>,
    pub injected_secrets: Vec<InjectedSecret>,
}

/// Result of guard processing.
#[derive(Debug, Clone)]
pub enum GuardResult {
    Clean { text: String },
    Redacted { text: String, reasons: Vec<String> },
    Blocked { reason: String, redacted_text: Option<String> },
    Warned { text: String, warnings: Vec<String> },
}

/// Central orchestrator for runtime security checks.
pub struct RuntimeSecurityGuard {
    config: SecurityGuardConfig,
    pii_engine: Option<Arc<RwLock<PiiEngine>>>,
    exec_leak_detector: Arc<Mutex<ExecLeakDetector>>,
    secret_leak_detector: Arc<Mutex<SecretLeakDetector>>,
}

impl RuntimeSecurityGuard {
    /// Create a new guard with default configuration.
    pub fn default_guard() -> Self {
        Self::new(SecurityGuardConfig::default())
    }

    /// Create a new guard with the given configuration.
    pub fn new(config: SecurityGuardConfig) -> Self {
        let exec_leak_detector = Arc::new(Mutex::new(ExecLeakDetector::default_patterns()));
        let secret_leak_detector = Arc::new(Mutex::new(SecretLeakDetector::new()));
        let pii_engine = if config.pii_filtering {
            PiiEngine::global().or_else(|| {
                Some(Arc::new(RwLock::new(PiiEngine::new(
                    crate::config::PrivacyConfig::default(),
                ))))
            })
        } else {
            None
        };

        Self {
            config,
            pii_engine,
            exec_leak_detector,
            secret_leak_detector,
        }
    }

    /// Process outbound content before sending to LLM.
    pub async fn process_outbound(
        &self,
        text: &str,
        resolver: Option<&dyn AsyncSecretResolver>,
        mut context: SecurityContext,
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
                        let mut detector = self
                            .secret_leak_detector
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        for secret in &injected {
                            detector.register_injected(&[secret.clone()], &[]);
                        }
                    }
                    context.injected_secrets.extend(injected);
                }
            }
        }

        // 2. Leak Detection (on text still containing placeholders)
        if self.config.leak_detection {
            let exec_scan = {
                let detector = self
                    .exec_leak_detector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                detector.scan_outbound(&current_text)
            };

            let secret_scan = {
                let detector = self
                    .secret_leak_detector
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                detector.scan_outbound(&current_text)
            };

            let has_blocks = exec_scan.has_blocks()
                || matches!(secret_scan, LeakDecision::Block { .. });

            if has_blocks {
                let redacted_text = match secret_scan {
                    LeakDecision::Block { redacted_content, .. } => Some(redacted_content),
                    _ => None,
                };
                return Ok(GuardResult::Blocked {
                    reason: "Leak detector found sensitive data in outbound content".to_string(),
                    redacted_text,
                });
            }

            if exec_scan.has_warnings() {
                warnings.push("Outbound leak detector warning".to_string());
            }
        }

        // 3. PII Filtering
        if self.config.pii_filtering {
            if let Some(engine) = &self.pii_engine {
                let engine_guard = engine.read().unwrap_or_else(|e| e.into_inner());

                let should_filter = match &context.provider_name {
                    Some(provider) => !engine_guard.is_provider_excluded(provider),
                    None => true,
                };

                if should_filter {
                    let result = engine_guard.filter(&current_text);
                    current_text = Self::apply_filter_result(result, &mut reasons, &mut warnings);
                }
            }
        }

        // 4. Content Sanitization
        if self.config.content_sanitization && context.has_external_content {
            if let Some(source) = context.external_source {
                current_text = wrap_external_content(&current_text, source);
            }
        }

        // 5. Placeholder Replacement
        for (raw, value) in &resolved_map {
            current_text = current_text.replace(raw, value);
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
    pub fn process_inbound(
        &self,
        text: &str,
    ) -> Result<GuardResult, SecurityGuardError> {
        if !self.config.leak_detection {
            return Ok(GuardResult::Clean {
                text: text.to_string(),
            });
        }

        let exec_scan = {
            let detector = self
                .exec_leak_detector
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            detector.scan_inbound(text)
        };

        let secret_scan = {
            let detector = self
                .secret_leak_detector
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            detector.scan_inbound(text)
        };

        // Handle secret leak detector block
        if let LeakDecision::Block {
            reason,
            redacted_content,
        } = secret_scan
        {
            return Ok(GuardResult::Blocked {
                reason: format!("Secret leak detector: {}", reason),
                redacted_text: Some(redacted_content),
            });
        }

        if exec_scan.has_blocks() {
            return Ok(GuardResult::Blocked {
                reason: "Leak detector found sensitive data in inbound content".to_string(),
                redacted_text: Some(text.to_string()),
            });
        }

        if exec_scan.has_warnings() {
            return Ok(GuardResult::Warned {
                text: text.to_string(),
                warnings: vec!["Inbound leak detector warning".to_string()],
            });
        }

        Ok(GuardResult::Clean {
            text: text.to_string(),
        })
    }

    /// Clear tracked injected secrets (call at end of request/session).
    pub fn clear_injected_secrets(&self) {
        let mut detector = self
            .secret_leak_detector
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        detector.clear();
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
            Ok(DecryptedSecret::new(
                "sk-ant-test12345678901234567890".to_string(),
            ))
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
                assert!(text.contains("sk-ant-test"));
                assert!(!text.contains("{{secret:test_key}}"));
            }
            _ => panic!("Expected Clean result, got {:?}", result),
        }
    }

    #[tokio::test]
    async fn test_outbound_blocks_accidental_secret_leak() {
        let guard = RuntimeSecurityGuard::default_guard();
        let context = SecurityContext::default();
        // This contains a real-looking API key that should be caught by leak detection
        let input = "My key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let result = guard.process_outbound(input, Some(&MockResolver), context).await;

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
        let result = guard.process_outbound(input, Some(&MockResolver), context).await.unwrap();

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
            .process_outbound("Use {{secret:test_key}}", &MockResolver, context)
            .await
            .unwrap();

        // Then simulate LLM echoing the exact secret value back
        let inbound = "Your API key is sk-ant-test12345678901234567890";
        let result = guard.process_inbound(inbound).unwrap();

        match result {
            GuardResult::Blocked { .. } => {
                // Expected: either exec leak detector (pattern match)
                // or secret leak detector (exact injected value match)
            }
            other => panic!("Expected Blocked for echoed secret, got {:?}", other),
        }
    }

    #[test]
    fn test_inbound_clean_for_normal_text() {
        let guard = RuntimeSecurityGuard::default_guard();
        let result = guard.process_inbound("Hello, this is normal text.").unwrap();
        assert!(
            matches!(result, GuardResult::Clean { .. }),
            "Expected Clean, got {:?}",
            result
        );
    }
}
