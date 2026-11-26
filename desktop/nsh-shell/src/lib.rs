//! Aleph's desktop host as an nsh Rust app. nsh.json names [`App`] as the
//! root the nsh runner mounts in the packaged app; the `aleph-nsh-shell`
//! binary runs the same root through [`run_command_line`] for status
//! queries, `--connect` and debug QA.

mod autostart;
mod credentials;
mod launch;
mod platform;
mod shell_host;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod app;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod cli;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod ipc_status;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod panel_bridge;
#[cfg(feature = "tsx-panel")]
mod panel_host;
#[cfg(all(feature = "tsx-panel", target_os = "macos"))]
mod panel_surface;

#[cfg(all(feature = "tsx-panel", not(target_os = "macos")))]
compile_error!(
    "tsx-panel embeds the Panel with the macOS AppView only; Linux and Windows are outside S2"
);

#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../../../qa/phase9_clock.rs"]
mod clock;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use app::App;
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
pub use cli::run as run_command_line;

/// The app's UI set (nsh.json `"rust"`): the nsh runner mounts [`App`] from it.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
#[nsh_core::prelude::native_module("aleph/native")]
fn native(registry: &mut nsh_core::prelude::Registry) {
    registry.register::<App>();
}
