//! Platform Abstraction Layer (PAL) for cross-platform perception
//!
//! This module provides a unified interface for UI perception and input
//! simulation across different platforms (macOS, Windows, Linux).
//!
//! # Architecture
//!
//! The PAL follows a tiered perception strategy:
//! - Level 1: Structured API (Accessibility API, UI Automation, AT-SPI)
//! - Level 2: Local Vision (Screenshot + OCR)
//! - Level 3: Cloud Vision (Multimodal API)
//!
//! # Core Traits
//!
//! - [`SystemSensor`]: Cross-platform UI sensing
//! - [`InputActuator`]: Cross-platform input simulation
//!
//! # Health Checking
//!
//! Use [`PerceptionHealth::check()`] to verify permissions and capabilities.

pub mod actuator;
pub mod health;
pub mod sensor;
pub mod types;

// Re-export main types
pub use actuator::{InputActuator, Key, Modifier};
pub use health::{PerceptionHealth, PlatformSupport};
pub use sensor::SystemSensor;
pub use types::{PalRect, Platform, SensorCapabilities, UINode, UINodeState, UINodeTree};
