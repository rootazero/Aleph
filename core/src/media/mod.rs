//! Media understanding pipeline — unified interface for image, audio, video, and document processing.
//!
//! This module defines the [`MediaProvider`] trait, [`MediaPipeline`] orchestrator,
//! and format detection utilities. Heavy processing is delegated to external
//! providers or plugins (per R1/R3).

pub mod types;

pub use types::{
    AudioFormat, DocFormat, MediaChunk, MediaImageFormat, MediaInput, MediaOutput, MediaType,
    VideoFormat,
};
