//! Pinned NSH 0.5 compatibility contract checks.
//!
//! These tests intentionally compile against the release commit pinned in
//! `desktop/nsh-shell/Cargo.toml`. They prove the source/API boundary only;
//! opening a native window remains a macOS QA/runtime check.

#[cfg(target_os = "macos")]
#[test]
fn pinned_v05_window_contract_is_available() {
    use nsh_core::desktop::window::{declarations, WindowKind};
    use nsh_engine::HostChannel;
    use nsh_platform_macos::AppOptions;

    let declarations = declarations();
    assert!(declarations.contains("WindowKind"));
    assert!(declarations.contains("WindowRef"));
    assert!(matches!(WindowKind::Normal, WindowKind::Normal));
    assert!(matches!(WindowKind::Dialog, WindowKind::Dialog));
    assert!(matches!(WindowKind::Panel, WindowKind::Panel));
    assert!(matches!(WindowKind::Popup, WindowKind::Popup));
    assert!(matches!(WindowKind::Overlay, WindowKind::Overlay));

    let host = HostChannel::new();
    assert!(host.params().is_empty());
    let options = AppOptions::default();
    assert_eq!(options.size, (1200, 800));
}
