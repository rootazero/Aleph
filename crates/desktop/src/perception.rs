//! Perception capabilities — screen capture, OCR, accessibility tree.
//!
//! This module will contain platform-specific implementations for:
//! - Screenshot capture via `xcap`
//! - OCR via platform APIs (WinRT on Windows, Vision on macOS)
//! - Accessibility tree inspection
