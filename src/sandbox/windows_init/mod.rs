//! SP-3a — Windows restricted-token + Low-IL init binary logic.
//!
//! Invoked as `aleph-server sandbox-init-windows --policy <json> --
//! <target> <target-args...>` by `WindowsSandboxDriver::run`. Lives in
//! a hidden CLI subcommand on the existing aleph-server binary (no
//! separate helper artifact — R3 core minimalism).
//!
//! The init prelude runs inside the `JobObject` that the driver already
//! assigned, *before* the untrusted target gets to execute. That's the
//! correct security hook point: the process container is already in
//! place, but the primary token is still the host's (full privileges,
//! Medium integrity). We derive a Chrome-pattern restricted token from
//! our own and use `CreateProcessAsUserW` to launch the target under it,
//! at Low integrity.
//!
//! Cross-platform parts (policy struct, JSON shape, argv parser) are not
//! gated, so they compile + unit-test on macOS / Linux dev boxes. The
//! actual `apply_*` and `run_init` Win32 entry point is
//! `#[cfg(target_os = "windows")]`-gated.
//!
//! Module layout:
//! - [`policy`] — `WindowsInitPolicy`, capability translation, DACL
//!   constants, protected-metadata classifier (cross-platform).
//! - [`args`] — the `--policy ... -- <target>` argv parser
//!   (cross-platform).
//! - `imp` — the Windows-only Win32 launch implementation
//!   (`#[cfg(target_os = "windows")]`).

mod args;
mod policy;

#[cfg(target_os = "windows")]
mod imp;

#[cfg(test)]
mod tests;

pub use policy::{capability_names_for_network, WindowsInitPolicy};

#[cfg(target_os = "windows")]
use self::args::parse_init_args;

/// Top-level entry point for the `sandbox-init-windows` subcommand. Never
/// returns: either calls `ExitProcess` with the target's exit code, or
/// `ExitProcess`es with a diagnostic code on init-side failure.
///
/// Exit codes (per spec §5):
/// - 64 → restricted token required but unavailable (`require_restricted_token=true`)
/// - 65 → unrecoverable Win32 setup error (OpenProcessToken / CreateRestrictedToken / SetTokenInformation / WaitForSingleObject / GetExitCodeProcess)
/// - 66 → argv parse failure
/// - 67 → all spawn paths failed (neither `CreateProcessAsUserW` nor `CreateProcessW`)
/// - 78 (`EX_CONFIG`) → invoked on a non-Windows host
#[cfg(target_os = "windows")]
pub fn run_init(args: Vec<String>) -> ! {
    let parsed = match parse_init_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("aleph sandbox-init-windows: argv parse failed: {e}");
            std::process::exit(66);
        }
    };

    // SP-6: try AppContainer first when enabled. Soft-degrade to SP-3a
    // restricted-token path on any AppContainer setup failure (unless
    // require_app_container=true escalates it).
    if parsed.policy.use_app_container {
        match imp::launch_with_app_container(&parsed) {
            Ok(code) => std::process::exit(code),
            Err(imp::LaunchError::AppContainerSetupFailed(msg))
                if !parsed.policy.require_app_container =>
            {
                eprintln!(
                    "aleph sandbox-init-windows: AppContainer setup failed ({msg}); \
                     falling back to restricted-token path"
                );
                // fall through to restricted-token branch below
            }
            Err(imp::LaunchError::AppContainerSetupFailed(msg)) => {
                eprintln!(
                    "aleph sandbox-init-windows: AppContainer setup failed ({msg}) \
                     and require_app_container=true"
                );
                std::process::exit(64);
            }
            Err(imp::LaunchError::WaitFailed(msg)) => {
                eprintln!("aleph sandbox-init-windows: AppContainer wait failed: {msg}");
                std::process::exit(65);
            }
            Err(other) => {
                eprintln!("aleph sandbox-init-windows: unexpected AppContainer error: {other:?}");
                std::process::exit(65);
            }
        }
    }

    let exit_code = match imp::launch_with_restricted_token(&parsed) {
        Ok(code) => code,
        Err(imp::LaunchError::PrivilegeNotHeld) if !parsed.policy.require_restricted_token => {
            eprintln!(
                "aleph sandbox-init-windows: restricted token unavailable \
                 (ERROR_PRIVILEGE_NOT_HELD); falling back to plain CreateProcessW \
                 (JobObject containment still applies)"
            );
            match imp::launch_with_host_token(&parsed) {
                Ok(code) => code,
                Err(e) => {
                    eprintln!("aleph sandbox-init-windows: fallback CreateProcessW failed: {e:?}");
                    std::process::exit(67);
                }
            }
        }
        Err(imp::LaunchError::PrivilegeNotHeld) => {
            eprintln!(
                "aleph sandbox-init-windows: restricted token required \
                 (ERROR_PRIVILEGE_NOT_HELD) and require_restricted_token=true"
            );
            std::process::exit(64);
        }
        Err(imp::LaunchError::SetupFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: setup failed: {msg}");
            std::process::exit(65);
        }
        Err(imp::LaunchError::SpawnFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: spawn failed: {msg}");
            std::process::exit(67);
        }
        Err(imp::LaunchError::WaitFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: wait failed: {msg}");
            std::process::exit(65);
        }
        // Unreachable here: AppContainerSetupFailed is only produced by
        // launch_with_app_container and is fully handled above. Pattern
        // is included to keep the match exhaustive against future
        // changes to LaunchError.
        Err(imp::LaunchError::AppContainerSetupFailed(msg)) => {
            eprintln!("aleph sandbox-init-windows: unreachable AppContainer error: {msg}");
            std::process::exit(65);
        }
    };

    std::process::exit(exit_code);
}

#[cfg(not(target_os = "windows"))]
pub fn run_init(_args: Vec<String>) -> ! {
    eprintln!("aleph sandbox-init-windows: only supported on Windows");
    std::process::exit(78); // EX_CONFIG
}
