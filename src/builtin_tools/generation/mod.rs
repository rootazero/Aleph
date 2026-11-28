//! Media generation tools
//!
//! Tools for generating images, speech, and other media using AI providers.
//! These tools implement rig's Tool trait for AI agent integration.

mod audio_generate;
mod image_generate;
mod speech_generate;
mod video_generate;

pub use audio_generate::{AudioGenerateArgs, AudioGenerateOutput, AudioGenerateTool};
pub use image_generate::{ImageGenerateArgs, ImageGenerateOutput, ImageGenerateTool};
pub use speech_generate::{SpeechGenerateArgs, SpeechGenerateOutput, SpeechGenerateTool};
pub use video_generate::{VideoGenerateArgs, VideoGenerateOutput, VideoGenerateTool};

// The provider error taxonomy reaches the tool layer through
// `AlephError::from(GenerationError)` (see `src/generation/error.rs`), which
// keeps a rate limit a rate limit and an authentication failure an
// authentication failure. The generation tools used to hop through
// `ToolError` first, and that hop collapsed every provider failure into
// "execution failed" — the classification the harness needs to decide
// whether a failure may be retried was destroyed on the way out.

#[cfg(test)]
mod default_provider_census {
    use std::path::Path;

    /// The operator's "set as default" choice has to reach the call.
    ///
    /// The Panel writes `default_image_provider` / `default_video_provider` /
    /// `default_audio_provider` / `default_speech_provider` and renders the
    /// badge, but the tools used to resolve their provider with
    /// `first_for_type()` — the alphabetically first provider of the right
    /// type. The badge changed and the call did not.
    #[test]
    fn every_generation_tool_prefers_the_configured_default_provider() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin_tools/generation");
        for tool in [
            "image_generate.rs",
            "video_generate.rs",
            "audio_generate.rs",
            "speech_generate.rs",
        ] {
            let source = std::fs::read_to_string(root.join(tool))
                .unwrap_or_else(|e| panic!("{tool} is readable: {e}"));
            assert!(
                source.contains("default_for_type"),
                "{tool} ignores the configured default provider"
            );
        }
    }
}

#[cfg(test)]
mod error_taxonomy_census {
    use std::path::Path;

    /// The generation tools must not collapse the provider error taxonomy
    /// again.
    ///
    /// `ToolError` has four variants and no way to say "rate limited": every
    /// provider failure became "execution failed", which the harness reads as
    /// a final verdict — the call is recorded in the cross-batch failure
    /// memo and the very retry the error text invites is refused for the rest
    /// of the run. The rich path is `AlephError::from(GenerationError)`.
    #[test]
    fn the_generation_tools_do_not_route_provider_failures_through_the_builtin_tool_error() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/builtin_tools/generation");
        let readers =
            crate::utils::source_scan::files_whose_production_code_reads(&root, "ToolError");
        assert!(
            readers.is_empty(),
            "a generation tool collapses the provider error taxonomy again: {readers:?}"
        );
    }
}
