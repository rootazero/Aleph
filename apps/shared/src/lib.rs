//! Aleph Client SDK
//!
//! Provides client-side functionality for connecting to and interacting with
//! Aleph Personal AI Hub instances.
//!
//! # Features
//!
//! - **Discovery**: Automatic mDNS discovery of local Aleph instances
//! - **Config**: Configuration management and synchronization
//!
//! # Example
//!
//! ```no_run
//! use aleph_sdk::discovery::MdnsScanner;
//! use std::time::Duration;
//!
//! # async fn example() -> Result<(), String> {
//! let scanner = MdnsScanner::new()?;
//! let instances = scanner.scan(Duration::from_secs(3)).await;
//! for instance in instances {
//!     println!("Found: {} at {}:{}", instance.name, instance.hostname, instance.port);
//! }
//! # Ok(())
//! # }
//! ```

pub mod discovery;
pub mod config;
