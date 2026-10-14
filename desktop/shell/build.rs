//! Registers every custom Tauri command in the app ACL manifest so each gets
//! an auto-generated `allow-<command>` permission. Without a manifest Tauri
//! implicitly allows all custom commands from local origins; with one (and a
//! `permissions/` dir already exists) a command is only reachable through an
//! explicit grant:
//!
//! * the nine legacy commands -> `capabilities/legacy-local-ipc.json`
//!   (local origins only, same reach as the implicit rule it replaces);
//! * the three `panel_credential_*` commands -> only the runtime capability
//!   added by `keyring_bridge::install` (remote Gateway origin, `main` window,
//!   origin-scoped), via the hand-written `allow-protected-*` permissions.
//!
//! Keep this list in sync with both `generate_handler!` arms in `src/main.rs`
//! (a unit test in `keyring_bridge` pins it). `discover_servers` / `connect_to`
//! exist only in the panel-only variant; listing them here is harmless in the
//! full app (an unregistered command is simply never invoked).

const COMMANDS: &[&str] = &[
    // legacy
    "get_connection_target",
    "set_connection_target",
    "clear_connection_target",
    "is_lite_shell",
    "discover_servers",
    "connect_to",
    "get_pending_cert",
    "approve_cert",
    "reject_cert",
    // Task 41
    "panel_credential_load",
    "panel_credential_save",
    "panel_credential_delete",
];

fn main() {
    let attributes = tauri_build::Attributes::new()
        .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS));
    if let Err(error) = tauri_build::try_build(attributes) {
        let error = format!("{error:#}");
        println!("{error}");
        std::process::exit(1);
    }
}
