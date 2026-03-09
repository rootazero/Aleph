//! `audio_transcribe` tool — dedicated audio transcription entry point.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

use crate::error::Result;
use crate::media::detect::detect_from_path;
use crate::media::{MediaInput, MediaOutput, MediaPipeline};
use crate::sync_primitives::Arc;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AudioTranscribeArgs {
    /// Path to a local audio file (mp3, wav, ogg, flac, m4a).
    pub file_path: String,

    /// Optional language hint (e.g., "en", "zh", "ja").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioTranscribeOutput {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl AudioTranscribeOutput {
    fn ok(text: String) -> Self {
        Self {
            success: true,
            text: Some(text),
            message: None,
            data: None,
        }
    }

    fn ok_data(text: String, data: Value) -> Self {
        Self {
            success: true,
            text: Some(text),
            message: None,
            data: Some(data),
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            text: None,
            message: Some(msg.into()),
            data: None,
        }
    }
}

#[derive(Clone)]
pub struct AudioTranscribeTool {
    pipeline: Arc<MediaPipeline>,
}

impl AudioTranscribeTool {
    pub fn new(pipeline: Arc<MediaPipeline>) -> Self {
        Self { pipeline }
    }
}

#[async_trait]
impl AlephTool for AudioTranscribeTool {
    const NAME: &'static str = "audio_transcribe";
    const DESCRIPTION: &'static str = r#"Transcribe audio files to text.

Supports: mp3, wav, ogg, flac, m4a.

Parameters:
- file_path: Path to a local audio file
- language: Optional language hint (e.g., "en", "zh")

Example:
{"file_path":"/tmp/meeting.mp3","language":"en"}"#;

    type Args = AudioTranscribeArgs;
    type Output = AudioTranscribeOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        let path = PathBuf::from(&args.file_path);
        let mt = match detect_from_path(&path) {
            Ok(mt) => mt,
            Err(e) => {
                return Ok(AudioTranscribeOutput::err(format!(
                    "Format detection failed: {}",
                    e
                )))
            }
        };

        if mt.category() != "audio" {
            return Ok(AudioTranscribeOutput::err(format!(
                "Expected audio file, got {} format",
                mt.category()
            )));
        }

        let input = MediaInput::FilePath { path };
        let prompt = args
            .language
            .as_deref()
            .map(|lang| format!("Transcribe this audio. Language: {}", lang))
            .unwrap_or_else(|| "Transcribe this audio.".to_string());

        match self
            .pipeline
            .process(&input, &mt, Some(&prompt))
            .await
        {
            Ok(output) => {
                let data = serde_json::to_value(&output).unwrap_or_default();
                match &output {
                    MediaOutput::Text { text } => Ok(AudioTranscribeOutput::ok(text.clone())),
                    _ => Ok(AudioTranscribeOutput::ok_data(
                        "Transcription complete".into(),
                        data,
                    )),
                }
            }
            Err(e) => Ok(AudioTranscribeOutput::err(format!(
                "Transcription failed: {}",
                e
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool() -> AudioTranscribeTool {
        let mp = MediaPipeline::new();
        AudioTranscribeTool::new(Arc::new(mp))
    }

    #[tokio::test]
    async fn rejects_non_audio_file() {
        let tool = make_tool();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("image.png");
        // Create a minimal PNG (magic bytes)
        std::fs::write(&path, &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]).unwrap();

        let args = AudioTranscribeArgs {
            file_path: path.to_string_lossy().to_string(),
            language: None,
        };
        let result = AlephTool::call(&tool, args).await.unwrap();
        assert!(!result.success);
        assert!(result.message.unwrap().contains("audio"));
    }

    #[test]
    fn tool_definition() {
        let tool = make_tool();
        let def = AlephTool::definition(&tool);
        assert_eq!(def.name, "audio_transcribe");
    }
}
