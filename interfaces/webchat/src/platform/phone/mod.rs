//! iPhone (iOS-native) layer — panel-only, connects to a remote core.
//!
//! Screens are rebuilt 1:1 from the aleph-mobile design system
//! (`docs/design-system/aleph-mobile/screens/exported/*.dc.html` +
//! `styles/aleph.css`): Chat / Memory / Agents / Settings / Voice /
//! Notifications. iOS component classes (`.cell` / `.list` / `.cell-leading`
//! / `.tabbar` / `.swatch` …) are ported into `styles/ios.css`; shared data
//! hooks reuse the crate-root `api` / `state`.
//!
//! Isolated from [`super::wide`] by construction — phone code never touches the
//! desktop/browser UI. Screens are added in subsequent steps.

pub mod agents;
pub mod alerts;
pub mod canvas;
pub mod chat;
pub mod dashboard;
pub mod extensions;
pub mod memory;
pub mod more;
pub mod settings;
pub mod shell;
pub mod teams;
