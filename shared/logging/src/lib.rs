//! Shared logging infrastructure for all Aleph components
//!
//! Provides unified file + console logging with:
//! - Per-component log files (`~/.aleph/logs/aleph-{component}.log.YYYY-MM-DD`)
//! - Daily rotation via tracing-appender
//! - PII scrubbing on all output
//! - Automatic retention-based cleanup
//!
//! # Quick Start
//!
//! ```rust,no_run
//! use aleph_logging::init_component_logging;
//!
//! // Server
//! init_component_logging("server", 7, "info").unwrap();
//!
//! // Desktop Bridge
//! init_component_logging("desktop", 7, "aleph_desktop=debug").unwrap();
//!
//! // CLI
//! init_component_logging("cli", 7, "info").unwrap();
//! ```

pub mod file_appender;
pub mod pii;
pub mod pii_filter;
pub mod retention;

pub use file_appender::{get_log_directory, init_component_logging, set_log_level};
pub use pii::scrub_pii;
#[allow(deprecated)]
pub use pii_filter::PiiScrubbingLayer;
pub use pii_filter::{create_pii_scrubbing_layer, PiiScrubbingFormat};
pub use retention::cleanup_old_logs;
