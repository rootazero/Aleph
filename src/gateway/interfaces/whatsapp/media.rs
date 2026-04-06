//! Media Processing for WhatsApp
//!
//! Handles media optimization and format conversion for outbound media.

use crate::gateway::channel::Attachment;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    pub max_inbound_mb: u64,
    pub max_outbound_mb: u64,
    pub auto_optimize: bool,
    pub jpeg_quality: u8,
    pub max_dimension: u32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            max_inbound_mb: 50,
            max_outbound_mb: 50,
            auto_optimize: true,
            jpeg_quality: 85,
            max_dimension: 1920,
        }
    }
}

pub struct MediaProcessor {
    config: MediaConfig,
}

impl MediaProcessor {
    pub fn new(config: MediaConfig) -> Self {
        Self { config }
    }
    
    pub async fn prepare_outbound(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        let max_bytes = self.config.max_outbound_mb * 1024 * 1024;
        
        if let Some(size) = attachment.size {
            if size > max_bytes {
                return Err(anyhow::anyhow!("Media file too large: {} bytes (max: {})", size, max_bytes));
            }
        }
        
        match attachment.mime_type.as_str() {
            "image/jpeg" | "image/png" | "image/gif" => {
                self.process_image(attachment).await
            }
            "video/mp4" | "video/quicktime" => {
                self.process_video(attachment).await
            }
            "audio/ogg" => {
                self.process_audio_ogg(attachment).await
            }
            _ => {
                self.process_document(attachment).await
            }
        }
    }
    
    async fn process_image(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        let Some(path) = &attachment.path else {
            return Err(anyhow::anyhow!("No path for image attachment"));
        };
        
        let img = image::open(path)?;
        
        let img = if img.width() > self.config.max_dimension 
                || img.height() > self.config.max_dimension {
            img.resize(
                self.config.max_dimension,
                self.config.max_dimension,
                image::imageops::FilterType::Lanczos3,
            )
        } else {
            img
        };
        
        let mut buf = Vec::new();
        let mut cursor = std::io::Cursor::new(&mut buf);
        img.write_to(&mut cursor, image::ImageFormat::Jpeg)?;
        
        Ok(OutboundMedia {
            data: buf,
            mime_type: "image/jpeg".to_string(),
            is_voice_note: false,
        })
    }
    
    async fn process_audio_ogg(&self, _attachment: &Attachment) -> Result<OutboundMedia> {
        Ok(OutboundMedia {
            data: Vec::new(),
            mime_type: "audio/ogg; codecs=opus".to_string(),
            is_voice_note: true,
        })
    }
    
    async fn process_document(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        Ok(OutboundMedia {
            data: Vec::new(),
            mime_type: attachment.mime_type.clone(),
            is_voice_note: false,
        })
    }
    
    async fn process_video(&self, attachment: &Attachment) -> Result<OutboundMedia> {
        Ok(OutboundMedia {
            data: Vec::new(),
            mime_type: attachment.mime_type.clone(),
            is_voice_note: false,
        })
    }
}

pub struct OutboundMedia {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub is_voice_note: bool,
}
