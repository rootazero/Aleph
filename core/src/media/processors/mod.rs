//! Media processors — concrete MediaProvider implementations.

pub mod audio;
pub mod document;
pub mod image;

pub use audio::AudioStubProvider;
pub use document::TextDocumentProvider;
pub use image::ImageMediaProvider;
