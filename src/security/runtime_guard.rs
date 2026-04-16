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
        resolver: &dyn AsyncSecretResolver,
        mut context: SecurityContext,
    ) -> Result<GuardResult, SecurityGuardError> {
        let mut current_text = text.to_string();

        // 1. Placeholder Extraction & Secret Resolution (no text replacement yet)
        let mut resolved_map: HashMap<String, String> = HashMap::new();
        if self.config.secret_injection {
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

        // 5. Placeholder Replacement (performed at the end)
        for (raw, value) in &resolved_map {
            current_text = current_text.replace(raw, value);
        }

        Ok(GuardResult::Clean { text: current_text })
    }

    /// Process inbound content received from LLM.
    pub fn process_inbound(
        &self,
        _text: &str,
    ) -> Result<GuardResult, SecurityGuardError> {
        Ok(GuardResult::Clean { text: _text.to_string() })
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
            .process_outbound(input, &MockResolver, context)
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
}
