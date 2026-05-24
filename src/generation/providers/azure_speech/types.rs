//! Request/response shapes for the Azure Cognitive Services Speech REST API.
//!
//! Azure's TTS endpoint takes an SSML XML body and returns raw audio bytes,
//! so there is no JSON request type. The only structured shape is the optional
//! error response Azure occasionally returns for 4xx responses (usually plain
//! text, sometimes JSON).

use serde::Deserialize;

/// Azure error envelope. The body may be raw text or JSON depending on the
/// failure mode; we tolerate both via Deserialize-with-defaults.
#[derive(Debug, Clone, Deserialize)]
pub struct AzureSpeechError {
    #[serde(default)]
    pub error: Option<AzureErrorDetail>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AzureErrorDetail {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

impl AzureSpeechError {
    pub fn best_message(&self) -> Option<String> {
        if let Some(detail) = &self.error {
            if let Some(m) = &detail.message {
                return Some(m.clone());
            }
        }
        self.message.clone()
    }
}
