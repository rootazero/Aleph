//! Same-machine PairingFlow.
//!
//! Walks the desktop shell through device pairing in two user-visible
//! beats — "welcome" then "approve" — and returns the issued device
//! token via `RpcPrompter::finish`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::gateway::device_store::{ApprovedDevice, DeviceStore};
use crate::gateway::security::{
    store::DeviceUpsertData, DeviceFingerprint, DeviceRole, PairingManager, PairingRequest,
    SecurityStore, TokenManager,
};
use crate::wizard::{RpcPrompter, WizardFlow, WizardPrompter, WizardSessionError, WizardStep};

/// Same-machine pairing flow: requests a code, asks the user to confirm,
/// approves the device, and returns the issued token via `finish`.
pub struct PairingFlow {
    device_name: String,
    pairing_manager: Arc<PairingManager>,
    security_store: Arc<SecurityStore>,
    device_store: Arc<DeviceStore>,
    token_manager: Arc<TokenManager>,
}

impl PairingFlow {
    /// Construct from the standard daemon security bundle.
    pub fn new(
        device_name: impl Into<String>,
        pairing_manager: Arc<PairingManager>,
        security_store: Arc<SecurityStore>,
        device_store: Arc<DeviceStore>,
        token_manager: Arc<TokenManager>,
    ) -> Self {
        Self {
            device_name: device_name.into(),
            pairing_manager,
            security_store,
            device_store,
            token_manager,
        }
    }

    /// Synthesise a deterministic 32-byte public-key placeholder from the device_id.
    /// Uses SHA-256 so the output is stable across restarts (unlike DefaultHasher).
    fn placeholder_pubkey(device_id: &str) -> [u8; 32] {
        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(device_id.as_bytes());
        let mut buf = [0u8; 32];
        buf.copy_from_slice(&hash);
        buf
    }
}

#[async_trait]
impl WizardFlow for PairingFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        // 1. user-visible greeting — note() uses prompt_no_wait; no answer expected
        prompter.note("为本机桌面配对 Aleph 守护进程", None).await?;

        // 2. internal: request a pairing code (uses a placeholder pubkey;
        // confirm() consumes the row regardless of pubkey content for
        // same-machine flows)
        let req = self
            .pairing_manager
            .request_device_pairing(self.device_name.clone(), None, vec![0u8; 32], None)
            .map_err(|e| WizardSessionError::FlowError(format!("request_device_pairing: {e}")))?;
        let code = req.code().to_string();

        // 3. user-visible confirm step
        prompter
            .prompt(WizardStep::confirm(
                "pairing-approve",
                format!("本机配对码：{code}\n点击「Approve」完成同机授权"),
            ))
            .await?;

        // 4. internal: consume the pairing row + register device + issue token
        let confirmed = self
            .pairing_manager
            .confirm_pairing(&code)
            .map_err(|e| WizardSessionError::FlowError(format!("confirm_pairing: {e}")))?;

        let (device_name, device_type) = match &confirmed {
            PairingRequest::Device {
                device_name,
                device_type,
                ..
            } => (
                device_name.clone(),
                device_type.map(|t| t.as_str().to_string()),
            ),
            PairingRequest::Channel { .. } => {
                return Err(WizardSessionError::FlowError(
                    "PairingFlow expects a device request, got a channel request".to_string(),
                ));
            }
            PairingRequest::Browser { .. } => {
                return Err(WizardSessionError::FlowError(
                    "PairingFlow expects a device request, got a browser request".to_string(),
                ));
            }
        };

        let device_id = uuid::Uuid::new_v4().to_string();
        let device = ApprovedDevice::new(device_id.clone(), device_name.clone(), device_type);

        self.device_store
            .approve_device(&device)
            .map_err(|e| WizardSessionError::FlowError(format!("approve_device: {e}")))?;

        let pk = Self::placeholder_pubkey(&device_id);
        let fingerprint = DeviceFingerprint::from_public_key(&pk);
        self.security_store
            .upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: &device_name,
                device_type: None,
                public_key: &pk,
                fingerprint: &fingerprint.0,
                role: "operator",
                scopes: &["*".to_string()],
            })
            .map_err(|e| WizardSessionError::FlowError(format!("upsert_device: {e}")))?;

        let signed = self
            .token_manager
            .issue_token(&device_id, DeviceRole::Operator, vec!["*".to_string()])
            .map_err(|e| WizardSessionError::FlowError(format!("issue_token: {e}")))?;
        let token = format!("{}:{}", signed.token, signed.signature);

        // 5. internal: persist to OS keychain — best-effort, non-blocking
        if let Err(e) = persist_token_to_keyring(&token) {
            tracing::warn!(error = %e, "keyring persist failed; pairing succeeded anyway");
        }

        // 6. finish: hand the token back through wizard.next's final response
        prompter
            .finish(json!({
                "token": token,
                "device_id": device_id,
                "device_name": device_name,
            }))
            .await?;
        Ok(())
    }

    fn name(&self) -> &str {
        "pairing"
    }
}

fn persist_token_to_keyring(token: &str) -> Result<(), keyring::Error> {
    // Skip the real keyring call in unit tests (cargo test --lib) — macOS
    // Keychain can pop a UI prompt that would hang the test.  Note: this
    // cfg gate does NOT apply to integration tests in `tests/`, where the
    // library is compiled without cfg(test).  Integration tests rely on
    // the best-effort error-swallow in PairingFlow::run so a keyring
    // failure becomes a warn-log, not a test failure.
    #[cfg(test)]
    {
        let _ = token;
        return Ok(());
    }

    #[cfg(not(test))]
    {
        const KEYRING_SERVICE: &str = "aleph-gateway";
        const KEYRING_USER: &str = "desktop-shell";
        let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)?;
        entry.set_password(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wizard::WizardSession;
    use std::sync::Arc;
    use std::time::Duration;

    fn test_bundle() -> (
        Arc<PairingManager>,
        Arc<SecurityStore>,
        Arc<DeviceStore>,
        Arc<TokenManager>,
    ) {
        let security = Arc::new(SecurityStore::in_memory().unwrap());
        let devices = Arc::new(DeviceStore::in_memory().unwrap());
        let pairing = Arc::new(PairingManager::new(security.clone()));
        let tokens = Arc::new(TokenManager::new(security.clone()));
        (pairing, security, devices, tokens)
    }

    #[tokio::test]
    async fn pairing_flow_emits_two_steps_then_returns_token() {
        let (pairing, security, devices, tokens) = test_bundle();
        let flow = PairingFlow::new("Test Mac", pairing, security, devices, tokens);
        let session = WizardSession::new(Box::new(flow));

        // Step 1: welcome note — uses prompt_no_wait, so no answer needed.
        // The flow advances on its own after emitting the note.
        let r = session.next().await;
        assert!(!r.done);
        let step = r.step.expect("welcome step");
        assert!(
            step.message.as_deref().unwrap_or("").contains("配对"),
            "welcome step should contain 配对"
        );

        // Step 2: confirm — blocks waiting for user approval
        let r = session.next().await;
        assert!(!r.done);
        let step = r.step.expect("confirm step");
        assert_eq!(step.id, "pairing-approve");
        assert!(step.message.as_deref().unwrap().contains("配对码"));

        session
            .answer("pairing-approve", serde_json::Value::Bool(true))
            .await
            .unwrap();

        // Give the flow a beat to run the internal approve+token block.
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if session.is_done() {
                break;
            }
        }

        let r = session.next().await;
        assert!(r.done);
        let data = r.data.expect("finish data");
        let token = data.get("token").and_then(|v| v.as_str()).unwrap();
        assert!(token.contains(':'), "token format: <body>:<sig>");
        assert!(data.get("device_id").is_some());
        assert_eq!(
            data.get("device_name").and_then(|v| v.as_str()),
            Some("Test Mac")
        );
    }

    #[tokio::test]
    async fn pairing_flow_propagates_confirm_failure() {
        // Use a 1ms-expiry PairingManager. After the flow emits the confirm
        // step we sleep 50ms before sending the answer, guaranteeing the code
        // has expired. confirm_pairing then errors → flow errors → WizardStatus::Error.
        let security = Arc::new(SecurityStore::in_memory().unwrap());
        let devices = Arc::new(DeviceStore::in_memory().unwrap());
        let pairing = Arc::new(PairingManager::with_expiry(security.clone(), 1)); // 1ms expiry
        let tokens = Arc::new(TokenManager::new(security.clone()));
        let flow = PairingFlow::new("Test", pairing, security, devices, tokens);
        let session = WizardSession::new(Box::new(flow));

        // welcome note — auto-advances, no answer needed
        let _ = session.next().await;
        // confirm step — wait for it to be emitted
        let _ = session.next().await;

        // Sleep long enough to guarantee 1ms expiry has elapsed before answering
        tokio::time::sleep(Duration::from_millis(50)).await;
        session
            .answer("pairing-approve", serde_json::Value::Bool(true))
            .await
            .unwrap();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if session.is_done() {
                break;
            }
        }
        let r = session.next().await;
        assert!(r.done);
        assert!(
            matches!(r.status, crate::wizard::WizardStatus::Error),
            "expected Error from expired confirm_pairing, got {:?}",
            r.status
        );
    }
}
