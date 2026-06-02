//! Concrete health checks registered with the [`DiagnosticEngine`](super::DiagnosticEngine).
//!
//! Each submodule is one diagnostic domain and owns both its detection and
//! (where safe) its mechanical repair. Add a new domain by implementing
//! [`HealthCheck`](super::HealthCheck) here and registering it in
//! [`super::DiagnosticEngine::default_registry`] (OCP — no engine changes).

pub mod config_parse;
pub mod data_dir;
pub mod hooks_consent;
pub mod stale_lock;

pub use config_parse::ConfigParseCheck;
pub use data_dir::DataDirCheck;
pub use hooks_consent::HooksConsentCheck;
pub use stale_lock::StaleLockCheck;
