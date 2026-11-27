//! The desktop WebView and its sole Panel share fixed, document-bound intents.

use crate::native_credentials::{DeviceConnectCredential, PanelCredentialLease};
use serde::{Deserialize, Serialize};

/// Explicit stop request on the bundled server’s private stdin pipe.
pub const DESKTOP_SHUTDOWN_BYTE: u8 = b'\n';

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutostartStatus {
    Enabled {},
    Disabled {},
    RequiresApproval {},
    Unavailable { reason: AutostartUnavailableReason },
    Failed {},
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutostartUnavailableReason {
    UnsupportedPlatform,
    NotAppBundle,
    IdentityMismatch,
    ApiUnavailable,
    NotFound,
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
    AutostartReply {
        request: u32,
        document: String,
        status: AutostartStatus,
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
    AutostartRequest {
        request: u32,
        document: String,
    },
    /// Turn launching at login on or off; the host answers with
    /// [`ShellRequest::AutostartReply`] carrying the status read back after
    /// the write.
    AutostartSet {
        request: u32,
        document: String,
        enabled: bool,
    },
    CredentialRequest {
        request: u32,
        document: String,
        call: CredentialCall,
    },
    /// A link off the Panel's pages the user followed; the host decides
    /// again whether it is one and hands it to the system browser or mail
    /// app. No reply.
    OpenExternal {
        document: String,
        url: String,
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
    fn autostart_request_is_a_document_bound_read_not_a_setter_or_target_query() {
        let document = "a".repeat(32);
        let request =
            serde_json::json!({"kind":"autostart-request", "request": 1, "document":document});
        assert!(serde_json::from_value::<PanelReply>(request.clone()).is_ok());
        for (key, value) in [
            ("enabled", serde_json::json!(true)),
            ("application", serde_json::json!("other")),
        ] {
            let mut bad = request.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<PanelReply>(bad).is_err());
        }
        for status in [
            serde_json::json!({"status":"enabled"}),
            serde_json::json!({"status":"disabled"}),
            serde_json::json!({"status":"requires_approval"}),
            serde_json::json!({"status":"unavailable", "reason":"not_found"}),
            serde_json::json!({"status":"failed"}),
        ] {
            let reply = serde_json::json!({"kind":"autostart-reply", "request": 1, "document":document, "status": status});
            assert!(serde_json::from_value::<ShellRequest>(reply.clone()).is_ok());
            let mut bad = reply;
            bad["status"]["extra"] = serde_json::json!(true);
            assert!(serde_json::from_value::<ShellRequest>(bad).is_err());
        }
    }

    #[test]
    fn open_external_carries_only_the_document_and_the_url() {
        let document = "a".repeat(32);
        let open = serde_json::json!({"kind":"open-external", "document":document, "url":"https://github.com/rootazero/Aleph"});
        let parsed = serde_json::from_value::<PanelReply>(open.clone()).expect("open-external");
        assert!(matches!(
            &parsed,
            PanelReply::OpenExternal { url, .. } if url == "https://github.com/rootazero/Aleph"
        ));
        assert_eq!(serde_json::to_value(&parsed).expect("serializes"), open);
        let mut bad = open;
        bad["target"] = serde_json::json!("_self");
        assert!(serde_json::from_value::<PanelReply>(bad).is_err());
    }

    #[test]
    fn autostart_set_carries_only_the_wanted_state() {
        let document = "a".repeat(32);
        let set = serde_json::json!({"kind":"autostart-set", "request": 1, "document":document, "enabled": true});
        assert!(matches!(
            serde_json::from_value::<PanelReply>(set.clone()),
            Ok(PanelReply::AutostartSet { enabled: true, .. })
        ));
        for (key, value) in [
            ("application", serde_json::json!("other")),
            ("path", serde_json::json!("/bin/sh")),
        ] {
            let mut bad = set.clone();
            bad[key] = value;
            assert!(serde_json::from_value::<PanelReply>(bad).is_err());
        }
        let mut not_a_bool = set;
        not_a_bool["enabled"] = serde_json::json!("yes");
        assert!(serde_json::from_value::<PanelReply>(not_a_bool).is_err());
    }

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
