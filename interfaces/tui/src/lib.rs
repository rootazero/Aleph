//! Aleph TUI Library
//!
//! Interactive terminal interface for Aleph Gateway.
//! Can be used as a library (from CLI's `chat` command) or standalone binary.

pub mod tui;
pub mod viewer_launcher;

use aleph_client::{CliError, DeviceCredentialStore};
use aleph_protocol::browser_live::{
    IssueViewerGrantRequest, IssueViewerGrantResponse, RevokeViewerGrantRequest,
    RevokeViewerGrantResponse, ViewerPermission,
};
use aleph_protocol::StreamEvent;
use tokio::sync::mpsc;

pub use aleph_client::{AlephClient, CliConfig, CliResult, DeviceCredential};
use viewer_launcher::ViewerLaunchRequest;

/// Launch the TUI, connecting to the given Gateway URL.
///
/// This is the main entry point when using the TUI as a library (e.g. from `aleph chat`).
///
/// # Arguments
/// * `server_url` — WebSocket URL of the Aleph Gateway (e.g. `ws://127.0.0.1:18790/ws`)
/// * `agent` — Optional agent name to bind this session to (reserved for future use)
/// * `session` — Optional session key; a new one is generated if `None`
/// * `continue_last` — Reopen the most recently active conversation
/// * `config` — CLI configuration (auth token, default session, etc.)
/// * `verbose` — Start with the TUI's verbose display mode on (shows reasoning)
///
/// # Errors
///
/// Returns an error if the gateway connection, handshake, or TUI launch fails.
pub async fn run(
    server_url: &str,
    _agent: Option<&str>,
    session: Option<&str>,
    continue_last: bool,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    // Connect to gateway (credentialless: LAN-trust / loopback; no keyring).
    let (client, events) = AlephClient::connect(server_url, config).await?;
    run_connected(client, events, session, continue_last, config, verbose).await
}

/// Like [`run`], but authenticates with an explicit [`DeviceCredential`].
///
/// This is an explicit opt-in: nothing calls it by default, and it is the only
/// path in this crate that touches the OS keyring. If `credential` is a
/// one-time bootstrap ticket, the long-lived device token the Gateway issues
/// in exchange is saved to [`DeviceCredentialStore`] (never the ticket) before
/// the TUI starts. If it cannot be saved the connection is closed and this
/// returns an error without starting the TUI: a ticket is single-use, so
/// carrying on would silently strand the device on its next launch.
///
/// A credential that is already a long-lived device token is used as-is and
/// not re-saved. The secret never enters [`CliConfig`] or any log line.
///
/// # Errors
///
/// Returns an error if the connection or credential exchange fails, if the
/// exchanged credential cannot be persisted, or if the TUI fails.
pub async fn run_with_device_credential(
    server_url: &str,
    credential: DeviceCredential,
    session: Option<&str>,
    continue_last: bool,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    let was_bootstrap = credential.is_bootstrap_ticket();
    let (client, events) =
        AlephClient::connect_with_credential(server_url, config, credential).await?;

    // Keyring I/O is blocking (and can hit a locked-keychain prompt), so it
    // runs on the blocking pool rather than stalling an async worker.
    let exchanged = client.credential_for_persistence();
    let url = server_url.to_string();
    let persisted = tokio::task::spawn_blocking(move || {
        persist_after_connect(was_bootstrap, exchanged, |c| {
            DeviceCredentialStore::for_gateway(&url)?.save(c)
        })
    })
    .await
    .unwrap_or_else(|e| {
        Err(CliError::Other(format!(
            "credential persistence task failed: {e}"
        )))
    });
    if let Err(e) = persisted {
        // Best effort: the failure to report is the persistence one.
        let _ = client.close().await;
        return Err(e);
    }

    run_connected(client, events, session, continue_last, config, verbose).await
}

/// Load the device credential previously saved for `server_url`, if any.
///
/// Explicit opt-in for callers that want to pass it to
/// [`run_with_device_credential`]. A keyring failure is an `Err`, not
/// `Ok(None)`: "could not look" must not read as "nothing stored".
///
/// # Errors
///
/// Returns an error if the OS keyring cannot be opened or read.
pub fn load_stored_device_credential(server_url: &str) -> CliResult<Option<DeviceCredential>> {
    DeviceCredentialStore::for_gateway(server_url)?.load()
}

/// Like [`run_with_device_credential`], with the credential loaded from the
/// OS keyring entry saved for `server_url` (the `aleph-tui --paired` path).
///
/// Fail-closed: a keyring error, or no stored credential, is an `Err` and the
/// TUI never starts - it does NOT fall back to the credentialless [`run`], so a
/// user who asked for a paired session is never silently downgraded. Keyring
/// I/O runs on the blocking pool. The credential itself is never logged, and
/// the returned errors never contain it.
///
/// # Errors
///
/// Returns an error if the keyring cannot be read, nothing is stored for
/// `server_url`, or [`run_with_device_credential`] fails.
pub async fn run_with_stored_device_credential(
    server_url: &str,
    session: Option<&str>,
    continue_last: bool,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    let url = server_url.to_string();
    let loaded = tokio::task::spawn_blocking(move || load_stored_device_credential(&url))
        .await
        .unwrap_or_else(|e| {
            Err(CliError::Other(format!(
                "credential load task failed: {e}"
            )))
        });
    let credential = require_stored_credential(loaded)?;
    run_with_device_credential(
        server_url,
        credential,
        session,
        continue_last,
        config,
        verbose,
    )
    .await
}

/// Turn "what the keyring said" into a credential or a fail-closed error.
/// "Could not look" and "nothing stored" are both errors, never a fallback.
fn require_stored_credential(
    loaded: CliResult<Option<DeviceCredential>>,
) -> CliResult<DeviceCredential> {
    match loaded {
        Ok(Some(credential)) => Ok(credential),
        Ok(None) => Err(CliError::Other(
            "no stored device credential for this Gateway; pair this device first".into(),
        )),
        Err(e) => Err(CliError::Other(format!(
            "could not read the stored device credential: {e}"
        ))),
    }
}

/// Persist the long-lived credential obtained by a bootstrap exchange.
///
/// Only a bootstrap exchange needs saving; a stored device token is already
/// stored. `exchanged` is `client.credential_for_persistence()`: `None` after
/// a bootstrap means no verified long-lived token exists, which is an error,
/// never a silent skip. `save` is injected so the policy is testable without
/// a real keyring.
fn persist_after_connect(
    was_bootstrap: bool,
    exchanged: Option<DeviceCredential>,
    save: impl FnOnce(&DeviceCredential) -> CliResult<()>,
) -> CliResult<()> {
    if !was_bootstrap {
        return Ok(());
    }
    let credential = exchanged.ok_or_else(|| {
        CliError::Other(
            "Gateway did not issue a verified device credential; refusing to continue because the bootstrap ticket is single-use"
                .into(),
        )
    })?;
    // Defence in depth: never write a ticket to the keyring.
    if credential.is_bootstrap_ticket() {
        return Err(CliError::Other(
            "refusing to persist a bootstrap ticket as a device credential".into(),
        ));
    }
    save(&credential)
}

/// Request a viewer launch for one exact, Gateway-published `resource_id`.
///
/// Calls `browser.viewer.grant.issue`, the existing issuer; this crate mints
/// nothing itself. Requests [`ViewerPermission::Frames`] only (read-only
/// frames); input and navigation are higher privileges that need their own
/// explicit product decision. The returned request wraps the one-time launch
/// URL (redacted in `Debug`); hand it to
/// [`viewer_launcher::ViewerLauncher`], which is disabled by default.
///
/// `resource_id` must come from a Gateway resource event (see
/// `AppState::browser_resources`) - never derived from a URL, tab or profile.
///
/// # Errors
///
/// Returns an error for an empty `resource_id`, or if the Gateway refuses.
pub async fn issue_browser_viewer_launch(
    client: &AlephClient,
    resource_id: &str,
) -> CliResult<ViewerLaunchRequest> {
    Ok(issue_browser_viewer_grant(client, resource_id)
        .await?
        .request)
}

/// A freshly issued viewer launch plus the exact `grant_id` the Gateway
/// assigned to it, so a launch that fails can revoke precisely that grant.
///
/// `grant_id` is an identifier, not a credential (the nonce lives only in the
/// launch URL, which stays redacted inside `request`).
pub struct IssuedBrowserViewerLaunch {
    pub request: ViewerLaunchRequest,
    pub grant_id: String,
}

/// Like [`issue_browser_viewer_launch`], but also returns the issued
/// `grant_id`.
///
/// # Errors
///
/// Returns an error for an empty `resource_id`, or if the Gateway refuses.
pub async fn issue_browser_viewer_grant(
    client: &AlephClient,
    resource_id: &str,
) -> CliResult<IssuedBrowserViewerLaunch> {
    let request = viewer_grant_request(resource_id)?;
    let response: IssueViewerGrantResponse = client
        .call("browser.viewer.grant.issue", Some(request))
        .await?;
    Ok(IssuedBrowserViewerLaunch {
        request: ViewerLaunchRequest::from_issuer_launch_url(response.launch_url),
        grant_id: response.grant_id,
    })
}

/// Best-effort revoke of one exact, previously issued `grant_id` via
/// `browser.viewer.grant.revoke`. Used when the launcher failed, so a grant
/// that will never be presented does not linger until its TTL.
///
/// # Errors
///
/// Returns an error if the Gateway call fails.
pub async fn revoke_browser_viewer_grant(client: &AlephClient, grant_id: &str) -> CliResult<()> {
    let _: RevokeViewerGrantResponse = client
        .call(
            "browser.viewer.grant.revoke",
            Some(RevokeViewerGrantRequest {
                grant_id: grant_id.to_string(),
            }),
        )
        .await?;
    Ok(())
}

fn viewer_grant_request(resource_id: &str) -> CliResult<IssueViewerGrantRequest> {
    if resource_id.trim().is_empty() {
        return Err(CliError::Config("resource_id must not be empty".into()));
    }
    Ok(IssueViewerGrantRequest {
        resource_id: resource_id.to_string(),
        permissions: vec![ViewerPermission::Frames],
    })
}

async fn run_connected(
    client: AlephClient,
    events: mpsc::Receiver<StreamEvent>,
    session: Option<&str>,
    continue_last: bool,
    config: &CliConfig,
    verbose: bool,
) -> CliResult<()> {
    // Determine the session to open. Precedence mirrors `aleph ask`:
    //   1. explicit --session
    //   2. --continue → newest by `last_active_at` (codex `resume` / pi
    //      `--continue` parity: continuing is opt-in, never the default, so a
    //      bare launch cannot silently append to yesterday's thread)
    //   3. config.default_session
    //   4. let the gateway route one
    //
    // Case 4 sends **no key at all** rather than inventing one. The TUI used to
    // mint `chat-<uuid8>`, which `SessionKey::parse` rejects (it wants
    // `agent:{id}:…`) — and an unparseable key does not fail the call, it makes
    // `AgentRouter::route` fall through and mint a fresh epoch. So the client
    // held a key the server had never heard of: every `/usage`, `/tier`,
    // `/compress`, `/undo` and history fetch answered about nothing, and each
    // launch silently started a new conversation. Asking the gateway to route
    // and then adopting what it reports keeps one authority for the key.
    let session_key = if let Some(s) = session {
        Some(s.to_string())
    } else if continue_last {
        match aleph_client::resolve_last_session(&client).await {
            Ok(key) => Some(key),
            Err(e) => {
                // A brand-new install has no session to continue. That is not
                // an error worth refusing to start over — say so and open a
                // fresh conversation.
                eprintln!("--continue: {e}; starting a new conversation");
                None
            }
        }
    } else {
        config.default_session.clone()
    };

    // Launch TUI
    tui::run(client, events, config, session_key, verbose).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn bootstrap() -> DeviceCredential {
        DeviceCredential::bootstrap_ticket("dev-1", "ticket-secret").unwrap()
    }
    fn token() -> DeviceCredential {
        DeviceCredential::device_token("dev-1", "token-secret").unwrap()
    }

    #[test]
    fn bootstrap_exchange_saves_the_issued_token() {
        let saved = Cell::new(false);
        persist_after_connect(true, Some(token()), |c| {
            assert!(!c.is_bootstrap_ticket());
            saved.set(true);
            Ok(())
        })
        .unwrap();
        assert!(saved.get());
    }

    #[test]
    fn stored_device_token_is_not_resaved() {
        persist_after_connect(false, Some(token()), |_| panic!("must not save")).unwrap();
    }

    #[test]
    fn missing_exchanged_credential_fails_closed() {
        let err = persist_after_connect(true, None, |_| panic!("must not save")).unwrap_err();
        assert!(matches!(err, CliError::Other(_)));
    }

    #[test]
    fn a_ticket_is_never_persisted() {
        let err = persist_after_connect(true, Some(bootstrap()), |_| panic!("must not save"))
            .unwrap_err();
        assert!(matches!(err, CliError::Other(_)));
    }

    #[test]
    fn keyring_failure_propagates() {
        let err = persist_after_connect(true, Some(token()), |_| {
            Err(CliError::Other("keyring locked".into()))
        })
        .unwrap_err();
        assert!(err.to_string().contains("keyring locked"));
    }

    #[test]
    fn grant_request_is_frames_only_and_rejects_empty_id() {
        let req = viewer_grant_request("res_a").unwrap();
        assert_eq!(req.resource_id, "res_a");
        assert_eq!(req.permissions, vec![ViewerPermission::Frames]);
        assert!(viewer_grant_request("  ").is_err());
    }

    #[test]
    fn stored_credential_loader_fails_closed() {
        let err = require_stored_credential(Ok(None)).unwrap_err();
        assert!(err.to_string().contains("no stored device credential"));

        let err =
            require_stored_credential(Err(CliError::Other("keyring locked".into()))).unwrap_err();
        assert!(matches!(err, CliError::Other(_)));
        assert!(err.to_string().contains("keyring locked"));

        let credential = require_stored_credential(Ok(Some(token()))).unwrap();
        assert!(!credential.is_bootstrap_ticket());
        // Neither error path can carry a secret: only fixed text + keyring status.
        let missing = require_stored_credential(Ok(None)).unwrap_err().to_string();
        assert!(!missing.contains("token-secret") && !missing.contains("ticket-secret"));
    }
}
