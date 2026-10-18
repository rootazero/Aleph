//! OS-keyring persistence for a paired device's long-lived credential.
//!
//! The store holds exactly `device_id` + `device_token` — one JSON blob per
//! gateway endpoint, inside the platform's protected store (Windows Credential
//! Manager, macOS Keychain, Linux Secret Service). Design rules:
//!
//! * **Fail closed.** No keyring provider, a locked/unreachable keyring, or an
//!   unreadable payload is an error, never a reason to write the secret
//!   anywhere else. There is no plaintext-file fallback.
//! * **Only a device token is persisted.** A bootstrap ticket is one-shot and
//!   is rejected by [`DeviceCredentialStore::save`].
//! * **No secret in errors.** Backend failures are mapped to fixed strings by
//!   category; the backend's own message is dropped.
//! * **The secret stays out of config.** `CliConfig` / `config.toml` never see
//!   it; the only producers are [`crate::AlephClient::credential_for_persistence`]
//!   and [`DeviceCredentialStore::load`].
//!
//! Keyring calls are blocking (and on Linux may talk to D-Bus); async callers
//! should run them on a blocking thread.

use std::fmt;

use aleph_protocol::native_credentials::DeviceConnectCredential;
use serde::{Deserialize, Serialize};

use crate::credential::{ensure_credential_transport, DeviceCredential};
use crate::error::{CliError, CliResult};

/// Keyring "service" every Aleph device credential is filed under.
const SERVICE: &str = "aleph-client.device-credential";
/// Payload schema version; bump on any incompatible payload change.
const PAYLOAD_VERSION: u32 = 1;

/// QA-only (`qa-service` feature, and unit tests): every non-production service
/// namespace must be exactly this prefix + 32..=128 lowercase hex. It can never
/// equal [`SERVICE`], so a QA store cannot read or clobber a real credential.
#[cfg(any(test, feature = "qa-service"))]
const QA_SERVICE_PREFIX: &str = "aleph-qa.device-credential.";
#[cfg(any(test, feature = "qa-service"))]
const QA_SERVICE_NONCE_MIN: usize = 32;
#[cfg(any(test, feature = "qa-service"))]
const QA_SERVICE_NONCE_MAX: usize = 128;

/// The persisted secret payload. `deny_unknown_fields`: a payload written by a
/// future/other schema is refused rather than half-read.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredential {
    version: u32,
    device_id: String,
    device_token: String,
}

impl fmt::Debug for StoredCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredCredential")
            .field("device_id", &self.device_id)
            .field("device_token", &"[redacted]")
            .finish()
    }
}

/// Why a backend call failed. Deliberately carries no message: backend error
/// text is not trusted to be free of secrets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BackendError {
    /// No entry exists (not a failure for `load` / `delete`).
    NoEntry,
    /// The keyring denied access, is locked, or is unreachable.
    Unavailable,
    /// This platform has no keyring provider compiled in.
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    NoProvider,
    /// Any other backend failure.
    Other,
}

/// Minimal secret-blob storage the store needs. The production implementation
/// is the OS keyring; unit tests substitute an in-memory fake.
trait SecretBackend {
    fn get(&self, service: &str, account: &str) -> Result<String, BackendError>;
    fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), BackendError>;
    fn delete(&self, service: &str, account: &str) -> Result<(), BackendError>;
}

/// Keyring-backed [`SecretBackend`]. The real implementation is compiled only
/// where a native provider is linked; elsewhere it fails closed (`NoProvider`).
struct OsKeyring;

#[cfg(any(windows, target_os = "macos", target_os = "linux"))]
mod os {
    use super::{BackendError, OsKeyring, SecretBackend};

    pub(super) fn map_error(err: &keyring::Error) -> BackendError {
        match err {
            keyring::Error::NoEntry => BackendError::NoEntry,
            keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
                BackendError::Unavailable
            }
            _ => BackendError::Other,
        }
    }

    fn entry(service: &str, account: &str) -> Result<keyring::Entry, BackendError> {
        keyring::Entry::new(service, account).map_err(|e| map_error(&e))
    }

    impl SecretBackend for OsKeyring {
        fn get(&self, service: &str, account: &str) -> Result<String, BackendError> {
            entry(service, account)?
                .get_password()
                .map_err(|e| map_error(&e))
        }
        fn set(&self, service: &str, account: &str, secret: &str) -> Result<(), BackendError> {
            entry(service, account)?
                .set_password(secret)
                .map_err(|e| map_error(&e))
        }
        fn delete(&self, service: &str, account: &str) -> Result<(), BackendError> {
            entry(service, account)?
                .delete_credential()
                .map_err(|e| map_error(&e))
        }
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
impl SecretBackend for OsKeyring {
    fn get(&self, _: &str, _: &str) -> Result<String, BackendError> {
        Err(BackendError::NoProvider)
    }
    fn set(&self, _: &str, _: &str, _: &str) -> Result<(), BackendError> {
        Err(BackendError::NoProvider)
    }
    fn delete(&self, _: &str, _: &str) -> Result<(), BackendError> {
        Err(BackendError::NoProvider)
    }
}

/// Fixed, secret-free text for a backend failure (the operation names where it
/// happened; the category says why).
fn backend_failure(op: &str, err: BackendError) -> CliError {
    let why = match err {
        BackendError::NoEntry => "no stored credential",
        BackendError::Unavailable => "the OS keyring is unavailable or access was denied",
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        BackendError::NoProvider => "this platform has no OS keyring provider",
        BackendError::Other => "the OS keyring reported an error",
    };
    CliError::Config(format!(
        "cannot {op} the device credential: {why}; refusing to fall back to plaintext storage"
    ))
}

/// Stable per-gateway keyring location for a device credential.
///
/// The account is derived from the gateway endpoint (`scheme://host:port`)
/// only — userinfo, path, query and fragment are dropped — so two URLs for the
/// same endpoint share one credential and nothing sensitive is logged or
/// filed.
pub struct DeviceCredentialStore {
    service: String,
    account: String,
    backend: Box<dyn SecretBackend + Send + Sync>,
}

impl fmt::Debug for DeviceCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceCredentialStore")
            .field("service", &self.service)
            .field("account", &self.account)
            .finish()
    }
}

impl DeviceCredentialStore {
    /// A store for `gateway_url`, backed by the OS keyring.
    pub fn for_gateway(gateway_url: &str) -> CliResult<Self> {
        Self::with_backend(gateway_url, Box::new(OsKeyring))
    }

    /// QA-only (`qa-service` feature): like [`Self::for_gateway`] (real OS
    /// backend, same URL validation and `*_for_connect` transport rules) but
    /// filed under a caller-supplied QA service namespace instead of the
    /// production one. `service` must be exactly
    /// `aleph-qa.device-credential.` + 32..=128 lowercase hex; anything else
    /// (including the production service) is rejected before any OS I/O.
    #[cfg(feature = "qa-service")]
    pub fn for_gateway_in_service(gateway_url: &str, service: &str) -> CliResult<Self> {
        Self::with_backend_in_service(gateway_url, service, Box::new(OsKeyring))
    }

    /// QA-only (`qa-service` feature): the QA service namespace for a harness
    /// nonce (`aleph-qa.device-credential.<nonce>`). The nonce must be
    /// 32..=128 lowercase hex; nothing else can produce a service, so a cleanup
    /// tool derives the namespace from the nonce alone (never from a CLI
    /// service string). Pure: no OS I/O.
    #[cfg(feature = "qa-service")]
    pub fn qa_service_for_nonce(nonce: &str) -> CliResult<String> {
        qa_service_for_nonce(nonce)
    }

    #[cfg(any(test, feature = "qa-service"))]
    fn with_backend_in_service(
        gateway_url: &str,
        service: &str,
        backend: Box<dyn SecretBackend + Send + Sync>,
    ) -> CliResult<Self> {
        validate_qa_service(service)?;
        Ok(Self {
            service: service.to_owned(),
            account: account_for_gateway(gateway_url)?,
            backend,
        })
    }

    fn with_backend(
        gateway_url: &str,
        backend: Box<dyn SecretBackend + Send + Sync>,
    ) -> CliResult<Self> {
        Ok(Self {
            service: SERVICE.to_owned(),
            account: account_for_gateway(gateway_url)?,
            backend,
        })
    }

    /// The keyring service name (non-secret).
    #[must_use]
    pub fn service(&self) -> &str {
        &self.service
    }

    /// The keyring account name (non-secret): the normalised gateway endpoint.
    #[must_use]
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Persist `credential`. Refuses a bootstrap ticket: only a long-lived
    /// device token (see [`crate::AlephClient::credential_for_persistence`])
    /// may be stored.
    pub fn save(&self, credential: &DeviceCredential) -> CliResult<()> {
        let Some((device_id, device_token)) = credential.persistable_parts() else {
            return Err(CliError::Config(
                "refusing to persist a bootstrap ticket; only a device token may be stored".into(),
            ));
        };
        let payload = serde_json::to_string(&StoredCredential {
            version: PAYLOAD_VERSION,
            device_id: device_id.to_owned(),
            device_token: device_token.to_owned(),
        })
        .map_err(|_| CliError::Config("cannot encode the device credential".into()))?;
        self.backend
            .set(&self.service, &self.account, &payload)
            .map_err(|e| backend_failure("store", e))
    }

    /// Load the stored device credential. `Ok(None)` only when the keyring is
    /// healthy and holds nothing for this gateway; every other failure
    /// (including a corrupt payload) is an error.
    pub fn load(&self) -> CliResult<Option<DeviceCredential>> {
        match self.backend.get(&self.service, &self.account) {
            Ok(payload) => decode_payload(&payload).map(Some),
            Err(BackendError::NoEntry) => Ok(None),
            Err(e) => Err(backend_failure("read", e)),
        }
    }

    /// Remove the stored credential. `Ok(false)` when there was none.
    pub fn delete(&self) -> CliResult<bool> {
        match self.backend.delete(&self.service, &self.account) {
            Ok(()) => Ok(true),
            Err(BackendError::NoEntry) => Ok(false),
            Err(e) => Err(backend_failure("delete", e)),
        }
    }

    /// Native-Bridge adapter: like [`Self::load`], but the endpoint must be
    /// one a credential may travel to (`wss://`, or `ws://` to loopback) —
    /// checked BEFORE any OS keyring I/O — and the result is the narrow
    /// connect-only wire DTO instead of a `DeviceCredential`.
    pub fn load_for_connect(&self) -> CliResult<Option<DeviceConnectCredential>> {
        ensure_credential_transport(&self.account)?;
        self.load()?.as_ref().map(connect_projection).transpose()
    }

    /// Native-Bridge adapter: persist a connect-only credential. Same
    /// transport rule as [`Self::load_for_connect`], enforced before any OS
    /// I/O. Only a device token can arrive here (the DTO has no ticket field).
    pub fn save_for_connect(&self, credential: &DeviceConnectCredential) -> CliResult<()> {
        ensure_credential_transport(&self.account)?;
        credential
            .validate()
            .map_err(|_| CliError::Config("the device credential is invalid".into()))?;
        let credential = DeviceCredential::device_token(
            credential.device_id.clone(),
            credential.device_token.clone(),
        )?;
        self.save(&credential)
    }

    /// Native-Bridge adapter: like [`Self::delete`] with the same up-front
    /// transport rule, so every Bridge operation is equally strict.
    pub fn delete_for_connect(&self) -> CliResult<bool> {
        ensure_credential_transport(&self.account)?;
        self.delete()
    }
}

/// `QA_SERVICE_PREFIX` + `nonce`, re-validated by [`validate_qa_service`] (so a
/// nonce that is not 32..=128 lowercase hex yields no service at all).
#[cfg(any(test, feature = "qa-service"))]
fn qa_service_for_nonce(nonce: &str) -> CliResult<String> {
    let service = format!("{QA_SERVICE_PREFIX}{nonce}");
    validate_qa_service(&service)?;
    Ok(service)
}

/// Accept only `aleph-qa.device-credential.<32..=128 lowercase hex>`. The error
/// text never echoes the rejected value.
#[cfg(any(test, feature = "qa-service"))]
fn validate_qa_service(service: &str) -> CliResult<()> {
    let ok = service
        .strip_prefix(QA_SERVICE_PREFIX)
        .is_some_and(|nonce| {
            (QA_SERVICE_NONCE_MIN..=QA_SERVICE_NONCE_MAX).contains(&nonce.len())
                && nonce
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        });
    if ok {
        Ok(())
    } else {
        Err(CliError::Config(
            "qa credential service must be 'aleph-qa.device-credential.' plus 32..=128 lowercase hex"
                .into(),
        ))
    }
}

/// Project a loaded device credential onto the connect-only wire DTO. Goes
/// through the crate-private `persistable_parts` (no public secret getter);
/// a bootstrap ticket has no such projection and is refused.
fn connect_projection(credential: &DeviceCredential) -> CliResult<DeviceConnectCredential> {
    let (device_id, device_token) = credential.persistable_parts().ok_or_else(|| {
        CliError::Config("a bootstrap ticket cannot be exposed as a stored credential".into())
    })?;
    DeviceConnectCredential::new(device_id, device_token)
        .map_err(|_| CliError::Config("the stored device credential is invalid".into()))
}

fn decode_payload(payload: &str) -> CliResult<DeviceCredential> {
    // The serde error is dropped: it can quote parts of the payload.
    let stored: StoredCredential = serde_json::from_str(payload).map_err(|_| {
        CliError::Config(
            "the stored device credential is unreadable or has an unknown format".into(),
        )
    })?;
    if stored.version != PAYLOAD_VERSION {
        return Err(CliError::Config(
            "the stored device credential has an unsupported version".into(),
        ));
    }
    DeviceCredential::device_token(stored.device_id, stored.device_token)
}

/// Normalise a gateway URL to `scheme://host:port` (lowercase; default port
/// per scheme; no userinfo/path/query/fragment). Only `ws`/`wss` are accepted:
/// the credential is bound to the endpoint the handshake will dial.
fn account_for_gateway(url: &str) -> CliResult<String> {
    let bad = || CliError::Config("gateway url is not a valid ws:// or wss:// endpoint".into());
    let (scheme, rest) = url.split_once("://").ok_or_else(bad)?;
    let scheme = scheme.to_ascii_lowercase();
    let default_port: u16 = match scheme.as_str() {
        "wss" => 443,
        "ws" => 80,
        _ => return Err(bad()),
    };
    if rest
        .chars()
        .any(|c| c == '\\' || c.is_ascii_control() || c.is_whitespace())
    {
        return Err(bad());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let hostport = authority.rsplit('@').next().unwrap_or_default();
    let (host, port_str) = if let Some(after) = hostport.strip_prefix('[') {
        let (h, tail) = after.split_once(']').ok_or_else(bad)?;
        (format!("[{h}]"), tail.strip_prefix(':'))
    } else {
        match hostport.split_once(':') {
            Some((h, p)) => (h.to_owned(), Some(p)),
            None => (hostport.to_owned(), None),
        }
    };
    if host.is_empty() || host == "[]" {
        return Err(bad());
    }
    let port = match port_str {
        None | Some("") => default_port,
        Some(p) => p.parse::<u16>().map_err(|_| bad())?,
    };
    Ok(format!("{scheme}://{}:{port}", host.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    type Shelf = Arc<Mutex<HashMap<(String, String), String>>>;

    /// In-memory stand-in for the OS keyring. Optionally fails every call with
    /// a given category.
    struct Fake {
        shelf: Shelf,
        fail: Option<BackendError>,
        /// Number of backend calls (proves a rejected call never touched it).
        hits: Arc<AtomicUsize>,
    }

    impl SecretBackend for Fake {
        fn get(&self, s: &str, a: &str) -> Result<String, BackendError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.fail {
                return Err(e);
            }
            self.shelf
                .lock()
                .unwrap()
                .get(&(s.into(), a.into()))
                .cloned()
                .ok_or(BackendError::NoEntry)
        }
        fn set(&self, s: &str, a: &str, v: &str) -> Result<(), BackendError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.fail {
                return Err(e);
            }
            self.shelf
                .lock()
                .unwrap()
                .insert((s.into(), a.into()), v.into());
            Ok(())
        }
        fn delete(&self, s: &str, a: &str) -> Result<(), BackendError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.fail {
                return Err(e);
            }
            self.shelf
                .lock()
                .unwrap()
                .remove(&(s.into(), a.into()))
                .map(|_| ())
                .ok_or(BackendError::NoEntry)
        }
    }

    fn store(url: &str, fail: Option<BackendError>) -> (DeviceCredentialStore, Shelf) {
        let (store, shelf, _) = counting_store(url, fail);
        (store, shelf)
    }

    fn counting_store(
        url: &str,
        fail: Option<BackendError>,
    ) -> (DeviceCredentialStore, Shelf, Arc<AtomicUsize>) {
        let shelf = Shelf::default();
        let hits = Arc::new(AtomicUsize::new(0));
        let store = DeviceCredentialStore::with_backend(
            url,
            Box::new(Fake {
                shelf: shelf.clone(),
                fail,
                hits: hits.clone(),
            }),
        )
        .unwrap();
        (store, shelf, hits)
    }

    const URL: &str = "wss://gw.example.com/ws";

    fn qa_store(
        url: &str,
        service: &str,
    ) -> CliResult<(DeviceCredentialStore, Shelf, Arc<AtomicUsize>)> {
        let shelf = Shelf::default();
        let hits = Arc::new(AtomicUsize::new(0));
        let store = DeviceCredentialStore::with_backend_in_service(
            url,
            service,
            Box::new(Fake {
                shelf: shelf.clone(),
                fail: None,
                hits: hits.clone(),
            }),
        )?;
        Ok((store, shelf, hits))
    }

    #[test]
    fn qa_service_accepts_only_the_exact_namespace() {
        let good = format!("aleph-qa.device-credential.{}", "a0".repeat(16));
        let (s, shelf, _) = qa_store(URL, &good).unwrap();
        assert_eq!(s.service(), good);
        s.save_for_connect(&connect("aleph-dt-SECRET")).unwrap();
        // Filed under the QA service, never the production one.
        let keys: Vec<_> = shelf.lock().unwrap().keys().cloned().collect();
        assert_eq!(keys, [(good.clone(), s.account().to_owned())]);
        assert_ne!(s.service(), SERVICE);
        // Bounds: 32 and 128 hex accepted.
        for n in [32, 128] {
            let svc = format!("aleph-qa.device-credential.{}", "f".repeat(n));
            assert!(qa_store(URL, &svc).is_ok(), "len {n}");
        }
    }

    #[test]
    fn qa_service_rejects_everything_else_before_any_io() {
        let p = "aleph-qa.device-credential.";
        let bad = [
            SERVICE.to_owned(),
            String::new(),
            p.to_owned(),
            format!("{p}{}", "a".repeat(31)),
            format!("{p}{}", "a".repeat(129)),
            format!("{p}{}", "A".repeat(32)),
            format!("{p}{}", "g".repeat(32)),
            format!("{p}{}.", "a".repeat(32)),
            format!(" {p}{}", "a".repeat(32)),
            format!("{p}{}\n", "a".repeat(32)),
            format!("x{p}{}", "a".repeat(32)),
            format!("aleph-qa.device-credential{}", "a".repeat(32)),
        ];
        for svc in &bad {
            let err = qa_store(URL, svc).map(|_| ()).unwrap_err();
            let text = format!("{err} {err:?}");
            // The fixed message names only the bare prefix, never the input.
            if svc.len() > p.len() {
                assert!(!text.contains(svc.as_str()), "{text}");
            }
        }
    }

    #[test]
    fn qa_service_for_nonce_derives_only_from_valid_hex() {
        let nonce = "c1".repeat(16);
        assert_eq!(
            qa_service_for_nonce(&nonce).unwrap(),
            format!("aleph-qa.device-credential.{nonce}")
        );
        let bad: Vec<String> = vec![
            String::new(),
            "abc".to_owned(),
            "A".repeat(32),
            "g".repeat(32),
            "a".repeat(129),
            "x.y".to_owned(),
        ];
        for n in &bad {
            assert!(qa_service_for_nonce(n).is_err(), "{n}");
        }
    }

    #[test]
    fn qa_service_keeps_url_and_transport_rules() {
        let svc = format!("aleph-qa.device-credential.{}", "b".repeat(32));
        assert!(qa_store("http://gw.example.com", &svc).is_err());
        let (s, shelf, hits) = qa_store("ws://gw.example.com/ws", &svc).unwrap();
        assert!(s.save_for_connect(&connect("aleph-dt-SECRET")).is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 0);
        assert!(shelf.lock().unwrap().is_empty());
    }

    #[test]
    fn payload_roundtrips_through_the_store() {
        let (s, shelf) = store(URL, None);
        assert!(s.load().unwrap().is_none());
        s.save(&DeviceCredential::device_token("dev-1", "aleph-dt-SECRET").unwrap())
            .unwrap();
        let loaded = s.load().unwrap().expect("stored");
        assert_eq!(loaded.device_id(), "dev-1");
        assert_eq!(
            loaded.persistable_parts(),
            Some(("dev-1", "aleph-dt-SECRET"))
        );
        // One blob, exactly the three schema fields.
        let blob = shelf.lock().unwrap().values().next().unwrap().clone();
        let v: serde_json::Value = serde_json::from_str(&blob).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["device_id", "device_token", "version"]);
        assert!(s.delete().unwrap());
        assert!(!s.delete().unwrap());
        assert!(s.load().unwrap().is_none());
    }

    fn connect(token: &str) -> DeviceConnectCredential {
        DeviceConnectCredential::new("dev-1", token).unwrap()
    }

    #[test]
    fn connect_adapters_roundtrip_through_the_fake_backend() {
        let (s, shelf, hits) = counting_store(URL, None);
        assert!(s.load_for_connect().unwrap().is_none());
        s.save_for_connect(&connect("aleph-dt-SECRET")).unwrap();
        // Stored as the same strict v1 token-only payload as `save`.
        let blob = shelf.lock().unwrap().values().next().unwrap().clone();
        let v: serde_json::Value = serde_json::from_str(&blob).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["device_id", "device_token", "version"]);
        assert_eq!(v["version"], 1);
        assert_eq!(
            s.load_for_connect().unwrap(),
            Some(connect("aleph-dt-SECRET"))
        );
        // Interoperates with the DeviceCredential path.
        assert_eq!(s.load().unwrap().unwrap().device_id(), "dev-1");
        assert!(s.delete_for_connect().unwrap());
        assert!(!s.delete_for_connect().unwrap());
        assert!(s.load_for_connect().unwrap().is_none());
        assert!(hits.load(Ordering::SeqCst) > 0);
    }

    #[test]
    fn connect_adapters_accept_loopback_plain_ws() {
        let (s, _, _) = counting_store("ws://127.0.0.1:18790/ws", None);
        s.save_for_connect(&connect("t")).unwrap();
        assert_eq!(s.load_for_connect().unwrap(), Some(connect("t")));
    }

    #[test]
    fn connect_adapters_reject_remote_plain_transport_before_any_io() {
        for url in ["ws://gw.example.com/ws", "ws://10.0.0.5:9000/ws"] {
            let (s, shelf, hits) = counting_store(url, None);
            // A pre-existing entry must be neither readable nor deletable.
            shelf.lock().unwrap().insert(
                (s.service().into(), s.account().into()),
                r#"{"version":1,"device_id":"d","device_token":"aleph-dt-SECRET"}"#.into(),
            );
            for err in [
                s.load_for_connect().map(|_| ()).unwrap_err(),
                s.save_for_connect(&connect("aleph-dt-NEW")).unwrap_err(),
                s.delete_for_connect().map(|_| ()).unwrap_err(),
            ] {
                let text = format!("{err} {err:?}");
                assert!(
                    !text.contains("SECRET") && !text.contains("aleph-dt-NEW"),
                    "{text}"
                );
            }
            assert_eq!(hits.load(Ordering::SeqCst), 0, "{url}: backend was touched");
            assert_eq!(shelf.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn connect_adapters_refuse_blank_credentials_before_any_io() {
        let (s, _, hits) = counting_store(URL, None);
        let blank = DeviceConnectCredential {
            device_id: "d".into(),
            device_token: " ".into(),
        };
        assert!(s.save_for_connect(&blank).is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn connect_load_refuses_corrupt_payload_and_failures_are_secret_free() {
        let (s, shelf, _) = counting_store(URL, None);
        shelf.lock().unwrap().insert(
            (s.service().into(), s.account().into()),
            r#"{"version":1,"device_id":"d","device_token":"aleph-dt-SECRET","extra":1}"#.into(),
        );
        let err = s.load_for_connect().unwrap_err();
        assert!(!format!("{err} {err:?}").contains("SECRET"));

        let (s, _, _) = counting_store(URL, Some(BackendError::Unavailable));
        for err in [
            s.load_for_connect().map(|_| ()).unwrap_err(),
            s.save_for_connect(&connect("aleph-dt-SECRET")).unwrap_err(),
            s.delete_for_connect().map(|_| ()).unwrap_err(),
        ] {
            assert!(matches!(err, CliError::Config(_)));
            assert!(!format!("{err} {err:?}").contains("SECRET"));
        }
    }

    #[test]
    fn connect_dto_debug_redacts_the_token() {
        let (s, _, _) = counting_store(URL, None);
        s.save_for_connect(&connect("aleph-dt-SECRET")).unwrap();
        let loaded = s.load_for_connect().unwrap().unwrap();
        let dbg = format!("{loaded:?}");
        assert!(!dbg.contains("aleph-dt-SECRET") && dbg.contains("[redacted]"));
    }

    #[test]
    fn unknown_fields_and_bad_payloads_are_refused() {
        for bad in [
            r#"{"version":1,"device_id":"d","device_token":"t","extra":"x"}"#,
            r#"{"version":1,"device_id":"d"}"#,
            r#"{"version":2,"device_id":"d","device_token":"t"}"#,
            r#"{"version":1,"device_id":"","device_token":"t"}"#,
            r#"{"version":1,"device_id":"d","device_token":"  "}"#,
            "not json",
        ] {
            let (s, shelf) = store(URL, None);
            shelf
                .lock()
                .unwrap()
                .insert((s.service().into(), s.account().into()), bad.into());
            assert!(s.load().is_err(), "{bad}");
        }
    }

    #[test]
    fn bootstrap_ticket_is_never_persisted() {
        let (s, shelf) = store(URL, None);
        let ticket = DeviceCredential::bootstrap_ticket("dev-1", "aleph-bt-SECRET").unwrap();
        let err = s.save(&ticket).unwrap_err().to_string();
        assert!(!err.contains("aleph-bt-SECRET"), "{err}");
        assert!(shelf.lock().unwrap().is_empty());
        assert!(s.load().unwrap().is_none());
    }

    #[test]
    fn service_and_account_are_deterministic_and_secret_free() {
        let a = account_for_gateway("wss://GW.Example.com/ws?token=SECRET#frag").unwrap();
        assert_eq!(a, "wss://gw.example.com:443");
        assert_eq!(a, account_for_gateway("wss://gw.example.com:443").unwrap());
        assert_eq!(
            account_for_gateway("wss://user:pw@gw.example.com/").unwrap(),
            a
        );
        assert_eq!(
            account_for_gateway("ws://127.0.0.1:18789/x").unwrap(),
            "ws://127.0.0.1:18789"
        );
        assert_eq!(account_for_gateway("ws://[::1]").unwrap(), "ws://[::1]:80");
        assert_ne!(a, account_for_gateway("ws://gw.example.com").unwrap());
        assert!(!a.contains("pw") && !a.contains("SECRET"));
        let (s1, _) = store("wss://gw.example.com", None);
        let (s2, _) = store("wss://gw.example.com/other", None);
        assert_eq!((s1.service(), s1.account()), (s2.service(), s2.account()));
        for bad in [
            "http://gw.example.com",
            "gw.example.com",
            "wss://",
            "wss://:443",
            "wss://host:notaport",
            "wss://host:99999",
            "wss://ho\\st",
            "wss://[::1",
        ] {
            assert!(account_for_gateway(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn backend_failures_fail_closed_without_leaking_secrets() {
        for why in [
            BackendError::Unavailable,
            #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
            BackendError::NoProvider,
            BackendError::Other,
        ] {
            let (s, _) = store(URL, Some(why));
            let cred = DeviceCredential::device_token("dev-1", "aleph-dt-SECRET").unwrap();
            for err in [
                s.save(&cred).unwrap_err(),
                s.load().unwrap_err(),
                s.delete().unwrap_err(),
            ] {
                assert!(matches!(err, CliError::Config(_)));
                let text = format!("{err} {err:?}");
                assert!(
                    !text.contains("aleph-dt-SECRET") && !text.contains("dev-1"),
                    "{text}"
                );
            }
        }
    }

    #[test]
    fn debug_never_prints_the_secret() {
        let stored = StoredCredential {
            version: 1,
            device_id: "dev-1".into(),
            device_token: "aleph-dt-SECRET".into(),
        };
        assert!(!format!("{stored:?}").contains("aleph-dt-SECRET"));
        let (s, _) = store(URL, None);
        assert!(!format!("{s:?}").contains("SECRET"));
    }

    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    #[test]
    fn keyring_error_text_is_dropped() {
        let leaky =
            keyring::Error::PlatformFailure(Box::new(std::io::Error::other("aleph-dt-SECRET")));
        assert_eq!(os::map_error(&leaky), BackendError::Unavailable);
        assert_eq!(
            os::map_error(&keyring::Error::NoEntry),
            BackendError::NoEntry
        );
        let text = backend_failure("read", os::map_error(&leaky)).to_string();
        assert!(!text.contains("SECRET"), "{text}");
    }

    /// Best-effort removal of the synthetic entry if the real-keyring probe
    /// fails after it was written. The probe's explicit delete disarms it.
    #[cfg(windows)]
    struct EntryCleanup {
        service: String,
        account: String,
        armed: bool,
    }

    #[cfg(windows)]
    impl Drop for EntryCleanup {
        fn drop(&mut self) {
            if !self.armed {
                return;
            }
            // The outcome is reported, never assumed: a failed cleanup must be
            // visible so the synthetic entry can be removed by hand.
            match OsKeyring.delete(&self.service, &self.account) {
                Ok(()) | Err(BackendError::NoEntry) => {
                    eprintln!("os_keyring probe: guard deleted the synthetic entry");
                }
                Err(_) => eprintln!(
                    "os_keyring probe: GUARD CLEANUP FAILED; remove the synthetic credential \
                     service={} account={} manually",
                    self.service, self.account
                ),
            }
        }
    }

    /// Unwrap with the fixed, secret-free `CliError` text only.
    #[cfg(windows)]
    fn expect_ok<T>(r: CliResult<T>, what: &str) -> T {
        match r {
            Ok(v) => v,
            Err(e) => panic!("{what} failed: {e}"),
        }
    }

    /// Real Windows Credential Manager round trip through the public
    /// `DeviceCredentialStore` (`OsKeyring`, no fake). Writes one synthetic
    /// entry under a unique `.invalid` gateway account and removes it.
    /// Run explicitly with `--ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "writes a synthetic credential to the real OS keyring"]
    fn os_keyring_roundtrip_on_windows_credential_manager() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let pid = std::process::id();
        let gateway = format!("wss://aleph-keyring-probe-{pid}-{nanos}.invalid:443/ws");
        let device_id = format!("probe-dev-{pid}-{nanos}");
        let device_token = format!("aleph-dt-probe-{nanos}-{pid}");

        let store = expect_ok(DeviceCredentialStore::for_gateway(&gateway), "for_gateway");
        assert!(store.service() == SERVICE, "unexpected keyring service");
        assert!(
            store.account().ends_with(".invalid:443"),
            "unexpected keyring account"
        );

        // Never overwrite a pre-existing entry: proceed only if truly absent.
        assert!(
            expect_ok(store.load(), "initial load").is_none(),
            "unique probe account unexpectedly already exists"
        );

        // Armed before the write: any failure from here still attempts delete.
        let mut cleanup = EntryCleanup {
            service: store.service().to_owned(),
            account: store.account().to_owned(),
            armed: true,
        };

        let cred = expect_ok(
            DeviceCredential::device_token(device_id.clone(), device_token.clone()),
            "synthetic credential",
        );
        expect_ok(store.save(&cred), "save");

        // Public view: same id + secret after a real keyring round trip.
        let loaded = expect_ok(store.load(), "load after save").expect("entry stored");
        assert!(loaded.device_id() == device_id, "device id mismatch");
        assert!(
            loaded.persistable_parts() == Some((device_id.as_str(), device_token.as_str())),
            "persisted device token mismatch"
        );

        // Private view: the raw keyring payload has the exact schema.
        let raw = OsKeyring
            .get(store.service(), store.account())
            .unwrap_or_else(|_| panic!("raw keyring read failed"));
        let stored: StoredCredential =
            serde_json::from_str(&raw).unwrap_or_else(|_| panic!("raw payload not decodable"));
        assert!(
            stored.version == PAYLOAD_VERSION,
            "payload version mismatch"
        );
        assert!(
            stored.device_id == device_id && stored.device_token == device_token,
            "raw payload mismatch"
        );

        // Explicit cleanup is the acceptance path; Drop is only a safety net.
        assert!(expect_ok(store.delete(), "delete"), "entry was not present");
        assert!(
            expect_ok(store.load(), "load after delete").is_none(),
            "entry still present after delete"
        );
        assert!(
            !expect_ok(store.delete(), "second delete"),
            "second delete found an entry"
        );
        cleanup.armed = false;
    }
}
