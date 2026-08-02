//! Media processors — concrete `MediaProvider` implementations.

pub mod document;
pub mod image;

pub use document::TextDocumentProvider;
pub use image::ImageMediaProvider;
