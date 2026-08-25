//! Concrete health checks registered with the [`DiagnosticEngine`](super::DiagnosticEngine).
//!
//! Each submodule is one diagnostic domain and owns both its detection and
//! (where safe) its mechanical repair. Add a new domain by implementing
//! [`HealthCheck`](super::HealthCheck) here and registering it in
//! [`super::DiagnosticEngine::default_registry`] (OCP — no engine changes).

pub mod browser_runtime;
pub mod cache_health;
pub mod cache_hit_rate;
pub mod capability_wiring;
pub mod config_parse;
pub mod data_dir;
pub mod disk_space;
pub mod duplicate_instance;
pub mod hooks_consent;
pub mod idle_extensions;
pub mod loop_graph;
pub mod media_codecs;
pub mod providers_connectivity;
pub mod sqlite_integrity;
pub mod stale_lock;
pub mod vault;

pub use browser_runtime::BrowserRuntimeCheck;
pub use cache_health::CacheHealthCheck;
pub use cache_hit_rate::CacheHitRateCheck;
pub use capability_wiring::CapabilityWiringCheck;
pub use config_parse::ConfigParseCheck;
pub use data_dir::DataDirCheck;
pub use disk_space::DiskSpaceCheck;
pub use duplicate_instance::DuplicateInstanceCheck;
pub use hooks_consent::HooksConsentCheck;
pub use idle_extensions::IdleExtensionsCheck;
pub use loop_graph::LoopGraphCheck;
pub use media_codecs::MediaCodecsCheck;
pub use providers_connectivity::ProvidersConnectivityCheck;
pub use sqlite_integrity::SqliteIntegrityCheck;
pub use stale_lock::StaleLockCheck;
pub use vault::VaultCheck;
