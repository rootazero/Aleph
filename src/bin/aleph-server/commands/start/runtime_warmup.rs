//! Background-spawned runtime capability probe (Bun / Python / browser / …).
//!
//! Fires from `start_server` via `tokio::spawn` and writes the result ledger
//! to `~/.aleph/runtimes/ledger.json`. Failure is best-effort: a missing or
//! unwritable runtimes dir simply emits a warning and the call returns.

const RUNTIME_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub(super) async fn runtime_startup_warmup() {
    use alephcore::runtimes::{
        self,
        ledger::{migrate_from_legacy, CapabilityEntry},
        CapabilityLedger, CapabilityStatus, SPECS,
    };
    use alephcore::sync_primitives::{Arc, AsyncRwLock as RwLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    let runtimes_dir = match runtimes::get_runtimes_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "runtime warmup skipped: cannot resolve runtimes dir");
            return;
        }
    };
    if let Err(e) = tokio::fs::create_dir_all(&runtimes_dir).await {
        tracing::warn!(error = %e, "runtime warmup skipped: cannot create runtimes dir");
        return;
    }
    let ledger_path = runtimes_dir.join("ledger.json");
    let migration_dir = runtimes_dir.clone();
    let ledger = match tokio::task::spawn_blocking(move || {
        match migrate_from_legacy(&migration_dir) {
            Ok(ledger) => ledger,
            Err(e) => {
                tracing::warn!(
                    "Legacy manifest migration failed: {e}, falling back to ledger load_or_create"
                );
                CapabilityLedger::load_or_create(&ledger_path)
            }
        }
    })
    .await
    {
        Ok(ledger) => Arc::new(RwLock::new(ledger)),
        Err(e) => {
            tracing::warn!("runtime warmup skipped: ledger migration task failed: {e}");
            return;
        }
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();

    let mut missing = Vec::new();
    for spec in SPECS {
        let probe_name = spec.name;
        let result = match tokio::time::timeout(
            RUNTIME_PROBE_TIMEOUT,
            tokio::task::spawn_blocking(move || runtimes::probe::probe(probe_name)),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                tracing::warn!("runtime warmup: probe task for {probe_name} failed: {e}");
                continue;
            }
            Err(_) => {
                // The probe API owns its child handle, so timing out detaches
                // this best-effort blocking task instead of killing the child.
                // Warmup and the async worker are no longer blocked by it.
                tracing::warn!(
                    "runtime warmup: probe for {probe_name} exceeded {}s; continuing",
                    RUNTIME_PROBE_TIMEOUT.as_secs()
                );
                continue;
            }
        };
        let mut g = ledger.write().await;
        if result.found {
            g.update(CapabilityEntry {
                name: spec.name.into(),
                bin_path: result.bin_path.unwrap_or_default(),
                version: result.version.unwrap_or_default(),
                status: CapabilityStatus::Ready,
                source: result.source,
                last_probed: now,
            });
        } else if runtimes::supported_on_current_os(spec.name) {
            // mark_missing (not update_status) so a stale path/version from a
            // ledger copied off another machine is cleared, not persisted.
            g.mark_missing(spec.name);
            missing.push(spec.name);
        }
    }
    let snapshot = ledger.read().await.clone();
    match tokio::task::spawn_blocking(move || snapshot.persist()).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("Failed to persist runtime ledger: {e}"),
        Err(e) => tracing::warn!("runtime ledger persistence task failed: {e}"),
    }
    if missing.is_empty() {
        tracing::info!("runtime warmup: all capabilities ready");
    } else {
        tracing::warn!(
            missing = ?missing,
            "runtime capabilities missing — browser / python tools will fail until installed. \
             Run 'aleph-server bootstrap-runtime' or open Panel → Settings → Runtime.",
        );
    }
}

#[cfg(test)]
mod warmup_tests {
    use super::runtime_startup_warmup;
    use std::process::{Command, Output, Stdio};
    use tempfile::TempDir;

    const WARMUP_TEST_ID: &str =
        "commands::start::runtime_warmup::warmup_tests::test_warmup_runs_and_persists_ledger";
    const CHILD_FLAG: &str = "ALEPH_TEST_RUNTIME_WARMUP_CHILD";
    const COMPLETION_MARKER: &str = "ALEPH_WARMUP_LEDGER_COMPLETE";

    fn warmup_child_completed(output: &Output) -> bool {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        output.status.success()
            && stdout
                .lines()
                .any(|line| line == format!("test {WARMUP_TEST_ID} ... ok"))
            && stdout.lines().any(|line| {
                line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; ")
            })
            && stderr
                .lines()
                .any(|line| line == format!("{COMPLETION_MARKER}:{WARMUP_TEST_ID}"))
    }

    #[tokio::test]
    async fn test_warmup_runs_and_persists_ledger() {
        if std::env::var(CHILD_FLAG).as_deref() != Ok(WARMUP_TEST_ID) {
            let dir = TempDir::new().unwrap();
            let home = dir.path().join("home");
            let aleph_home = dir.path().join("aleph-home");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&aleph_home).unwrap();
            // Only the exact child changes environment; it inherits the outer command's group/bound.
            let output = Command::new(std::env::current_exe().unwrap())
                .args([WARMUP_TEST_ID, "--exact", "--nocapture", "--test-threads=1"])
                .env(CHILD_FLAG, WARMUP_TEST_ID)
                .env("HOME", &home)
                .env("ALEPH_HOME", &aleph_home)
                .stdin(Stdio::null())
                .output()
                .expect("run the exact warmup child in the current test binary");
            assert!(
                warmup_child_completed(&output),
                "warmup child did not prove completion: status={}, stdout={:?}, stderr={:?}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            eprintln!(
                "warmup-child evidence: id={WARMUP_TEST_ID}, stdout={:?}, stderr={:?}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }

        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let aleph_home = std::path::PathBuf::from(std::env::var_os("ALEPH_HOME").unwrap());
        assert_eq!(
            alephcore::runtimes::get_runtimes_dir().unwrap(),
            aleph_home.join("runtimes")
        );
        runtime_startup_warmup().await;

        let ledger_path = aleph_home.join("runtimes/ledger.json");
        assert!(
            ledger_path.exists(),
            "ledger must be persisted at {}",
            ledger_path.display()
        );
        let content = tokio::fs::read_to_string(&ledger_path).await.unwrap();
        let _: serde_json::Value =
            serde_json::from_str(&content).expect("ledger must be valid JSON");
        assert!(!home.join(".aleph/runtimes/ledger.json").exists());
        eprintln!("{COMPLETION_MARKER}:{WARMUP_TEST_ID}");
    }

    #[test]
    fn warmup_parent_rejects_zero_match_child() {
        let dir = TempDir::new().unwrap();
        let unmatched = format!("{WARMUP_TEST_ID}::unmatched_child_control");
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                unmatched.as_str(),
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_FLAG, WARMUP_TEST_ID)
            .env("HOME", dir.path())
            .env("ALEPH_HOME", dir.path().join("aleph-home"))
            .stdin(Stdio::null())
            .output()
            .expect("run a real unmatched exact child");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "unmatched child must really exit zero"
        );
        assert!(stdout.contains("running 0 tests"));
        assert!(stdout
            .lines()
            .any(|line| line
                .starts_with("test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; ")));
        assert!(!warmup_child_completed(&output));
        eprintln!(
            "warmup-zero-match rejected: filter={unmatched}, stdout={stdout:?}, stderr={:?}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
