//! Intent detection module for AI-powered conversation flow.
//!
//! This module provides AI-powered detection of user intent and automatic
//! capability invocation (search, video, skills, mcp).
//!
//! # Architecture
//!
//! The module uses a single AI-first approach:
//! - **AiIntentDetector**: AI-powered detection for language-agnostic classification
//!
//! AI analyzes user input and decides whether to:
//! 1. Respond directly (general conversation)
//! 2. Request capability invocation (search, video, etc.)
//! 3. Ask for clarification (missing required parameters)
//!
//! # Example
//!
//! ```ignore
//! let detector = AiIntentDetector::new(provider);
//! let result = detector.detect("¿Cómo está el clima en Madrid?").await?;
//! // result.intent == "search"
//! // result.params["location"] == "Madrid"
//! ```

pub mod ai_detector;

pub use ai_detector::{AiIntentDetector, AiIntentResult};
