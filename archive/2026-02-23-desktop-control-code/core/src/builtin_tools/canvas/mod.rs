//! Canvas Tool Module
//!
//! Agent-driven visual rendering system with A2UI protocol support.
//!
//! # Overview
//!
//! Canvas provides agents with the ability to:
//! - Display visual content in a dedicated window
//! - Execute JavaScript in a browser context
//! - Capture screenshots
//! - Render dynamic A2UI components
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────┐
//! │                     Canvas System                        │
//! ├─────────────────────────────────────────────────────────┤
//! │                                                          │
//! │  ┌──────────────┐    ┌──────────────┐                  │
//! │  │  CanvasTool  │───▶│  Controller  │                  │
//! │  └──────────────┘    └──────┬───────┘                  │
//! │                             │                           │
//! │                     ┌───────▼───────┐                  │
//! │                     │CanvasBackend  │                  │
//! │                     │   (trait)     │                  │
//! │                     └───────┬───────┘                  │
//! │            ┌────────────────┼────────────────┐         │
//! │            ▼                ▼                ▼         │
//! │     ┌──────────┐    ┌──────────┐    ┌──────────┐      │
//! │     │ NoOpBknd │    │ TauriBknd│    │ WebView  │      │
//! │     │ (test)   │    │ (desktop)│    │ (mobile) │      │
//! │     └──────────┘    └──────────┘    └──────────┘      │
//! │                                                          │
//! └─────────────────────────────────────────────────────────┘
//! ```
//!
//! # A2UI Protocol
//!
//! A2UI (Agent-to-UI) is a JSONL-based protocol for dynamic component rendering.
//! Agents send JSONL messages to update surfaces with components.
//!
//! Example A2UI message:
//! ```json
//! {"surfaceUpdate":{"surfaceId":"main","components":[{"id":"root","type":"container"}]}}
//! ```
//!
//! # Canvas Host
//!
//! The Canvas Host is an HTTP server that serves:
//! - The main canvas HTML page (`/`)
//! - A2UI runtime assets (`/__moltbot__/a2ui/*`)
//! - User content (`/__moltbot__/canvas/*`)
//!
//! # Usage
//!
//! ```rust,ignore
//! use alephcore::builtin_tools::canvas::{CanvasTool, CanvasController, CanvasToolArgs};
//! use std::sync::Arc;
//!
//! // Create tool with no-op backend (for testing)
//! let tool = CanvasTool::new_noop();
//!
//! // Present the canvas
//! let output = tool.call(CanvasToolArgs::present()).await?;
//! assert!(output.success);
//!
//! // Push A2UI content
//! let jsonl = r#"{"surfaceUpdate":{"surfaceId":"main","components":[]}}"#;
//! let output = tool.call(CanvasToolArgs::a2ui_push(jsonl)).await?;
//! ```

mod a2ui;
mod controller;
mod host;
mod tool;
mod types;

// Re-export public types
pub use a2ui::{
    parse_jsonl, parse_message, validate_jsonl, A2uiMessage, A2uiParseError, BeginRendering,
    Component, ComponentType, DataModelUpdate, DataUpdate, EventHandler, Surface, SurfaceManager,
    SurfaceUpdate, UserAction,
};
pub use controller::{CanvasBackend, CanvasController, NoOpBackend};
pub use host::{create_router, CanvasHostConfig, CanvasHostState};
pub use tool::CanvasTool;
pub use types::{
    CanvasAction, CanvasState, CanvasToolArgs, CanvasToolOutput, SnapshotFormat, WindowPlacement,
};
