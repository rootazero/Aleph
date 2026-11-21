//! The desktop WebView and its sole Panel share fixed, document-bound intents.

use crate::native_credentials::{DeviceConnectCredential, PanelCredentialLease};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CredentialCall {
    Load {},
    Save {
        lease_id: u64,
        credential: DeviceConnectCredential,
    },
    Delete {
        lease_id: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialError {
    Unavailable,
    Rejected,
    Malformed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CredentialReply {
    Loaded { lease: PanelCredentialLease },
    Saved {},
    Deleted { existed: bool },
    Failed { error: CredentialError },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ShellRequest {
    Probe {
        request: u32,
    },
    OpenChat {
        request: u32,
        document: String,
    },
    CredentialsReady {
        document: String,
    },
    CredentialReply {
        request: u32,
        document: String,
        reply: CredentialReply,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PanelReply {
    Ready {
        request: u32,
        document: String,
    },
    ChatOpened {
        request: u32,
        document: String,
    },
    Notify {
        document: String,
        title: String,
        body: String,
    },
    CredentialRequest {
        request: u32,
        document: String,
        call: CredentialCall,
    },
}

pub fn valid_document(document: &str) -> bool {
    document.len() == 32
        && document
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_notification(title: &str, body: &str) -> bool {
    !title.is_empty() && title.len() <= 256 && body.len() <= 2048
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_cannot_encode_an_arbitrary_destination() {
        assert!(serde_json::from_str::<ShellRequest>(
            r#"{"kind":"open-chat","request":1,"document":"a","path":"/settings"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ShellRequest>(
            r#"{"kind":"navigate","request":1,"document":"a"}"#
        )
        .is_err());
    }

    #[test]
    fn credentials_are_a_closed_contract_not_an_invoke_tunnel() {
        for raw in [
            r#"{"operation":"invoke","command":"shell_exec"}"#,
            r#"{"operation":"load","gateway_url":"wss://other/ws"}"#,
            r#"{"operation":"save","lease_id":1,"credential":{"device_id":"d","device_token":"t"},"account":"other"}"#,
            r#"{"operation":"delete","lease_id":1,"role":"admin"}"#,
        ] {
            assert!(serde_json::from_str::<CredentialCall>(raw).is_err());
        }
    }

    #[test]
    fn saved_ack_roundtrips_without_a_nullable_value_ambiguity() {
        let raw = serde_json::to_string(&CredentialReply::Saved {}).unwrap();
        assert_eq!(raw, r#"{"outcome":"saved"}"#);
        assert_eq!(
            serde_json::from_str::<CredentialReply>(&raw).unwrap(),
            CredentialReply::Saved {}
        );
        assert!(serde_json::from_str::<CredentialReply>(r#"{"outcome":"loaded"}"#).is_err());
    }

    #[test]
    fn nested_credential_debug_redacts_tokens() {
        let message = PanelReply::CredentialRequest {
            request: 1,
            document: "a".repeat(32),
            call: CredentialCall::Save {
                lease_id: 1,
                credential: DeviceConnectCredential::new("d", "sensitive-test-value").unwrap(),
            },
        };
        assert!(!format!("{message:?}").contains("sensitive-test-value"));
    }
}
