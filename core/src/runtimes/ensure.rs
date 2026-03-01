//! Capability orchestration — Probe -> Bootstrap -> Register
//!
//! The central function `ensure_capability` is called when a tool
//! needs a runtime that may not be installed.

use crate::error::AlephError;
use crate::runtimes::bootstrap::{self, BootstrapResult};
use crate::runtimes::ledger::{
    CapabilityEntry, CapabilityLedger, CapabilitySource, CapabilityStatus,
};
use crate::runtimes::probe;
use std::path::PathBuf;
use crate::sync_primitives::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tracing::{info, warn};

/// Ensure a capability is ready, probing and bootstrapping if needed.
/// Returns the executable path on success.
///
/// Note: There is an inherent TOCTOU (Time-of-check-to-time-of-use) gap
/// between probing for a binary and the caller using it. This is acceptable
/// for our use case since runtime binaries are rarely deleted while in use.
pub async fn ensure_capability(
    capability: &str,
    ledger: &Arc<RwLock<CapabilityLedger>>,
) -> Result<PathBuf, AlephError> {
    // Fast path: already Ready
    {
        let mut guard = ledger.write().await;
        if guard.status(capability) == CapabilityStatus::Ready {
            if let Some(path) = guard.executable(capability) {
                if path.exists() {
                    return Ok(path.to_path_buf());
                }
                // Path gone — mark stale, fall through to re-probe
                warn!("Capability {} path no longer exists, marking stale", capability);
                guard.update_status(capability, CapabilityStatus::Stale);
            }
        }
    }

    // Probe phase
    info!("Probing for capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Probing);
    }

    let probe_result = probe::probe(capability);

    if probe_result.found {
        let bin_path = match probe_result.bin_path.clone() {
            Some(path) => path,
            None => {
                return Err(AlephError::other(
                    format!("Capability {} found but no binary path reported", capability),
                ));
            }
        };
        if let Some(ref warning) = probe_result.version_warning {
            warn!("{}", warning);
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut guard = ledger.write().await;
        guard.update(CapabilityEntry {
            name: capability.to_string(),
            bin_path: bin_path.clone(),
            version: probe_result.version.unwrap_or_default(),
            status: CapabilityStatus::Ready,
            source: probe_result.source,
            last_probed: now,
        });
        let _ = guard.persist();

        info!("Capability {} found at {}", capability, bin_path.display());
        return Ok(bin_path);
    }

    // Bootstrap phase — resolve dependencies first
    for dep in bootstrap::dependencies(capability) {
        Box::pin(ensure_capability(dep, ledger)).await?;
    }

    // Check if bootstrap spec exists
    if !bootstrap::has_spec(capability) {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Missing);
        return Err(AlephError::runtime(
            capability,
            format!(
                "Capability '{}' not found and no bootstrap available",
                capability
            ),
        ));
    }

    info!("Bootstrapping capability: {}", capability);
    {
        let mut guard = ledger.write().await;
        guard.update_status(capability, CapabilityStatus::Bootstrapping);
    }

    // Run bootstrap (blocking shell in spawn_blocking)
    let cap_owned = capability.to_string();
    let bootstrap_result = tokio::task::spawn_blocking(move || bootstrap::bootstrap(&cap_owned))
        .await
        .map_err(|e| {
            AlephError::runtime(capability, format!("Bootstrap task panicked: {}", e))
        })?
        .map_err(|e| AlephError::runtime(capability, format!("Bootstrap failed: {}", e)))?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    match bootstrap_result {
        BootstrapResult::Success { bin_path } => {
            // Re-probe to get version info
            let version = {
                let re_probe = probe::probe(capability);
                re_probe.version.unwrap_or_default()
            };

            let mut guard = ledger.write().await;
            guard.update(CapabilityEntry {
                name: capability.to_string(),
                bin_path: bin_path.clone(),
                version,
                status: CapabilityStatus::Ready,
                source: CapabilitySource::AlephManaged,
                last_probed: now,
            });
            let _ = guard.persist();

            info!(
                "Capability {} bootstrapped at {}",
                capability,
                bin_path.display()
            );
            Ok(bin_path)
        }
        BootstrapResult::PathNotFound { expected } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(AlephError::runtime(
                capability,
                format!(
                    "Bootstrap completed but binary not found at: {}",
                    expected.display()
                ),
            ))
        }
        BootstrapResult::Failed { stderr } => {
            let mut guard = ledger.write().await;
            guard.update_status(capability, CapabilityStatus::Missing);
            Err(AlephError::runtime(
                capability,
                format!(
                    "Failed to bootstrap {}. Error: {}. Please install manually.",
                    capability, stderr
                ),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_ensure_already_ready() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let mut ledger = CapabilityLedger::load_or_create(ledger_path);

        // Pre-populate with a "ready" entry pointing to a real binary
        let bin = PathBuf::from("/bin/sh");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        ledger.update(CapabilityEntry {
            name: "test-shell".into(),
            bin_path: bin.clone(),
            version: "1.0".into(),
            status: CapabilityStatus::Ready,
            source: CapabilitySource::System,
            last_probed: now,
        });

        let ledger = Arc::new(RwLock::new(ledger));
        let result = ensure_capability("test-shell", &ledger).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bin);
    }

    #[tokio::test]
    async fn test_ensure_unknown_capability() {
        let dir = TempDir::new().unwrap();
        let ledger_path = dir.path().join("ledger.json");
        let ledger = CapabilityLedger::load_or_create(ledger_path);
        let ledger = Arc::new(RwLock::new(ledger));

        let result = ensure_capability("totally_unknown_thing_xyz", &ledger).await;
        assert!(result.is_err());
    }
}
