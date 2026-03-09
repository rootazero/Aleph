//! Media understanding pipeline — unified interface for image, audio, video, and document processing.
//!
//! This module defines the [`MediaProvider`] trait, [`MediaPipeline`] orchestrator,
//! and format detection utilities. Heavy processing is delegated to external
//! providers or plugins (per R1/R3).

pub mod detect;
pub mod error;
pub mod pipeline;
pub mod policy;
pub mod provider;
pub mod types;
pub mod processors;

pub use detect::{detect_by_extension, detect_by_magic, detect_from_path};
pub use error::MediaError;
pub use pipeline::MediaPipeline;
pub use policy::MediaPolicy;
pub use provider::MediaProvider;
pub use types::{
    AudioFormat, DocFormat, MediaChunk, MediaImageFormat, MediaInput, MediaOutput, MediaType,
    VideoFormat,
};
pub use processors::{AudioStubProvider, ImageMediaProvider, TextDocumentProvider};
