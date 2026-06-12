ISSUE|src/diagnostics/checks/data_dir.rs:31|medium|Blocking filesystem I/O inside async HealthCheck::run|write_probe uses std::fs::write and std::fs::remove_file directly in async fn run; stalls Tokio worker thread
ISSUE|src/diagnostics/checks/data_dir.rs:61|medium|Blocking filesystem I/O inside async HealthCheck::run|std::fs::create_dir_all called directly in async fn run; stalls Tokio worker thread
ISSUE|src/diagnostics/checks/data_dir.rs:61|low|Repair does not verify newly created directory is writable|create_dir_all succeeds in Fix posture but write_probe is never run to confirm writability
ISSUE|src/diagnostics/checks/config_parse.rs:51|medium|Blocking filesystem I/O inside async HealthCheck::run|Config::load_from_file reads config file synchronously in async fn run
ISSUE|src/diagnostics/checks/hooks_consent.rs:120|medium|Blocking registry read inside async HealthCheck::run|self.diagnose() calls self.consent.entries() synchronously in async fn run
ISSUE|src/diagnostics/checks/stale_lock.rs:45|medium|Blocking filesystem I/O inside async HealthCheck::run|diagnose_holder reads lock file synchronously in async fn run
ISSUE|src/diagnostics/checks/stale_lock.rs:83|medium|Blocking filesystem I/O inside async HealthCheck::run|std::fs::remove_file called directly in async fn run
ISSUE|src/diagnostics/checks/stale_lock.rs:82|medium|TOCTOU race when removing stale lock file|decision to remove based on diagnose_holder is stale by the time remove_file executes; may remove a fresh lock
ISSUE|src/diagnostics/checks/vault.rs:73|medium|Blocking filesystem I/O inside async HealthCheck::run|SecretVault::open reads vault file synchronously in async fn run
