//! Compatibility checks for the official NSH version used by the shell.
//!
//! These tests intentionally compile against the crates.io release pinned in
//! `desktop/nsh-shell/Cargo.toml`. They prove the source/API boundary only;
//! opening a native window remains a macOS QA/runtime check.

#[cfg(target_os = "macos")]
#[test]
fn window_contract_is_available() {
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

#[cfg(target_os = "macos")]
#[test]
fn web_view_ref_contract_is_available() {
    use nsh_core::prelude::{Element, IntoElement, Prop, Props, component, element_ref, web_view};
    use nsh_platform_macos::Root;

    #[derive(Clone, PartialEq, Props)]
    struct WebViewProps {
        url: Prop<String>,
    }

    #[component]
    fn RefWebView(props: WebViewProps) -> impl IntoElement {
        let view = element_ref();
        web_view(props.url).element_ref(&view)
    }

    let _root = Root::new::<RefWebView>(move || WebViewProps {
        url: "http://127.0.0.1:18790/".to_string().into(),
    });
}
