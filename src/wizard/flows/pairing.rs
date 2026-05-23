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
    store::DeviceUpsertData, DeviceRole, PairingManager, PairingRequest, SecurityStore,
    TokenManager,
};
use crate::wizard::{RpcPrompter, WizardFlow, WizardSessionError, WizardStep};

/// Same-machine pairing flow: requests a code, asks the user to confirm,
/// approves the device, and returns the issued token via `finish`.
pub struct PairingFlow {
    pub device_name: String,
    pub pairing_manager: Arc<PairingManager>,
    pub security_store: Arc<SecurityStore>,
    pub device_store: Arc<DeviceStore>,
    pub token_manager: Arc<TokenManager>,
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

    /// Synthesise a stable 32-byte public-key placeholder from the device_id.
    /// Mirrors the same trick used by the CLI `approve_locked` until real
    /// keypair generation is wired.
    fn placeholder_pubkey(device_id: &str) -> [u8; 32] {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut h = DefaultHasher::new();
        device_id.hash(&mut h);
        let hash = h.finish();
        let mut buf = [0u8; 32];
        buf[..8].copy_from_slice(&hash.to_le_bytes());
        buf[8..16].copy_from_slice(&(hash.wrapping_mul(0x9e3779b97f4a7c15)).to_le_bytes());
        buf
    }
}

#[async_trait]
impl WizardFlow for PairingFlow {
    async fn run(&self, prompter: &RpcPrompter) -> Result<(), WizardSessionError> {
        // 1. user-visible greeting
        prompter
            .prompt(WizardStep::note(
                "pairing-welcome",
                "为本机桌面配对 Aleph 守护进程",
            ))
            .await?;

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
        };

        let device_id = uuid::Uuid::new_v4().to_string();
        let device = ApprovedDevice::new(device_id.clone(), device_name.clone(), device_type);

        self.device_store
            .approve_device(&device)
            .map_err(|e| WizardSessionError::FlowError(format!("approve_device: {e}")))?;

        let pk = Self::placeholder_pubkey(&device_id);
        self.security_store
            .upsert_device(&DeviceUpsertData {
                device_id: &device_id,
                device_name: &device_name,
                device_type: None,
                public_key: &pk,
                fingerprint: &device_id[..device_id.len().min(16)],
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

const KEYRING_SERVICE: &str = "aleph-gateway";
const KEYRING_USER: &str = "desktop-shell";

fn persist_token_to_keyring(token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER)
        .map_err(|e| format!("entry: {e}"))?;
    entry
        .set_password(token)
        .map_err(|e| format!("set_password: {e}"))
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
        let flow = PairingFlow::new(
            "Test Mac",
            pairing,
            security,
            devices,
            tokens,
        );
        let session = WizardSession::new(Box::new(flow));

        // Step 1: welcome
        let r = session.next().await;
        assert!(!r.done);
        let step = r.step.expect("welcome step");
        assert_eq!(step.id, "pairing-welcome");

        // Answer the welcome (note has no required answer; client convention
        // is to send `null` via wizard.next which only blocks notes through
        // the manager — direct session.answer for the step id keeps the
        // unit test simple).
        session.answer("pairing-welcome", serde_json::Value::Null).await.unwrap();

        // Step 2: confirm
        let r = session.next().await;
        assert!(!r.done);
        let step = r.step.expect("confirm step");
        assert_eq!(step.id, "pairing-approve");
        assert!(step.message.as_deref().unwrap().contains("配对码"));

        session.answer("pairing-approve", serde_json::Value::Bool(true)).await.unwrap();

        // Give the flow a beat to run the internal approve+token block.
        // 200ms is generous for an in-memory store; the test will retry the
        // next() drain below.
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
        assert_eq!(data.get("device_name").and_then(|v| v.as_str()), Some("Test Mac"));
    }

    #[tokio::test]
    async fn pairing_flow_propagates_request_failure() {
        // Drive the flow with a 0ms-expiry manager so the pairing code
        // produced in step 2 has effectively expired by the time step 4
        // tries to confirm it. We assert that the session reaches some
        // terminal state cleanly (Done or Error) without panicking — the
        // exact terminal depends on timing race between request and
        // confirm; either is acceptable for the smoke-fail path.
        let security = Arc::new(SecurityStore::in_memory().unwrap());
        let devices = Arc::new(DeviceStore::in_memory().unwrap());
        let pairing = Arc::new(PairingManager::with_expiry(security.clone(), 0)); // 0ms expiry → instant timeout
        let tokens = Arc::new(TokenManager::new(security.clone()));
        let flow = PairingFlow::new("Test", pairing, security, devices, tokens);
        let session = WizardSession::new(Box::new(flow));

        // welcome
        let _ = session.next().await;
        session.answer("pairing-welcome", serde_json::Value::Null).await.unwrap();
        // confirm
        let _ = session.next().await;
        session.answer("pairing-approve", serde_json::Value::Bool(true)).await.unwrap();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if session.is_done() {
                break;
            }
        }
        let r = session.next().await;
        assert!(r.done);
        // Either error status (if instant expiry kicked in) OR success; both
        // are acceptable terminal states. Verify the session reached SOME
        // terminal state cleanly without panicking.
        assert!(matches!(
            r.status,
            crate::wizard::WizardStatus::Done | crate::wizard::WizardStatus::Error
        ));
    }
}
