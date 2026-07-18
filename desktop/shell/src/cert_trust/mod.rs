//! In-app TOFU trust for self-signed TLS certs (Approach A). Pure decision core
//! (this file + `store`/`fingerprint`); platform adapters feed it the cert from
//! each engine's TLS-error hook. Never touches the OS trust store; only an exact
//! pinned-fingerprint match is ever allowed (fail-closed).

pub mod fingerprint;
pub mod install;
pub mod pending;
pub mod store;

#[cfg(target_os = "macos")]
pub mod adapter_macos;

#[cfg(target_os = "windows")]
pub mod adapter_windows;

use serde::Serialize;

/// Cert facts shown to the user. Display-only — the decision pins the
/// fingerprint regardless of the specific failure reason.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CertInfo {
    pub sans: Vec<String>,
    pub subject: String,
    pub reason: String,
}

/// TOFU verdict for a presented leaf cert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    PromptUnknown {
        fp: String,
        info: CertInfo,
    },
    WarnChanged {
        old_fp: String,
        new_fp: String,
        info: CertInfo,
    },
}

/// Pure decision: compare the presented fingerprint against the pinned store.
#[must_use]
pub fn decide(
    host: &str,
    presented_fp: &str,
    info: CertInfo,
    store: &store::TrustStore,
) -> Decision {
    match store.lookup(host) {
        None => Decision::PromptUnknown {
            fp: presented_fp.to_string(),
            info,
        },
        Some(pinned) if pinned == presented_fp => Decision::Allow,
        Some(pinned) => Decision::WarnChanged {
            old_fp: pinned.to_string(),
            new_fp: presented_fp.to_string(),
            info,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use store::TrustStore;

    fn info() -> CertInfo {
        CertInfo {
            sans: vec!["172.245.43.211".into()],
            subject: "CN=Aleph".into(),
            reason: "self-signed".into(),
        }
    }

    #[test]
    fn unknown_host_prompts() {
        let store = TrustStore::empty();
        assert!(matches!(
            decide("h:1", "AA:BB", info(), &store),
            Decision::PromptUnknown { .. }
        ));
    }

    #[test]
    fn matching_fp_allows() {
        let mut store = TrustStore::empty();
        store.insert_mem("h:1", "AA:BB");
        assert_eq!(decide("h:1", "AA:BB", info(), &store), Decision::Allow);
    }

    #[test]
    fn changed_fp_warns() {
        let mut store = TrustStore::empty();
        store.insert_mem("h:1", "AA:BB");
        assert!(matches!(
            decide("h:1", "CC:DD", info(), &store),
            Decision::WarnChanged { .. }
        ));
    }
}
