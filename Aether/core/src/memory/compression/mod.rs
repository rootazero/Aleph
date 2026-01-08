//! Memory Compression Module
//!
//! This module provides functionality to compress raw conversation memories
//! into structured facts using LLM extraction. The dual-layer architecture:
//!
//! - **Layer 1 (Raw Logs)**: Original conversation pairs in `memories` table
//! - **Layer 2 (Compressed Facts)**: LLM-extracted facts in `memory_facts` table
//!
//! ## Components
//!
//! - [`CompressionService`]: Main service that orchestrates compression
//! - [`FactExtractor`]: Extracts facts from conversations using LLM
//! - [`ConflictDetector`]: Detects and resolves conflicting facts
//! - [`CompressionScheduler`]: Determines when to trigger compression

mod conflict;
mod extractor;
mod scheduler;
mod service;

pub use conflict::{ConflictConfig, ConflictDetector, ConflictResolution};
pub use extractor::{ExtractedFact, FactExtractor};
pub use scheduler::{CompressionScheduler, CompressionTrigger, SchedulerConfig};
pub use service::{CompressionConfig, CompressionService};
