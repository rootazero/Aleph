use crate::gateway::channel::Attachment;
use crate::gateway::interfaces::whatsapp::config::WhatsAppAccountConfig;

pub struct MediaProcessor;

impl MediaProcessor {
    pub fn preprocess(
        attachment: &Attachment,
        config: &WhatsAppAccountConfig,
    ) -> Result<ProcessedMedia, String> {
        let max_bytes = config.media.max_outbound_mb * 1024 * 1024;
        let data = attachment.data.as_ref().ok_or("Attachment has no data")?;
        if data.len() > max_bytes as usize {
            return Err(format!(
                "Attachment exceeds {} MB",
                config.media.max_outbound_mb
            ));
        }

        let mime = attachment.mime_type.clone();
        let final_mime = if mime == "audio/ogg" {
            "audio/ogg; codecs=opus".to_string()
        } else {
            mime
        };

        Ok(ProcessedMedia {
            data: data.clone(),
            mime_type: final_mime,
            filename: attachment.filename.clone(),
        })
    }
}

pub struct ProcessedMedia {
    pub data: Vec<u8>,
    pub mime_type: String,
    pub filename: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::channel::Attachment;

    #[test]
    fn test_ogg_opus_rewrite() {
        let att = Attachment {
            id: "a1".into(),
            mime_type: "audio/ogg".into(),
            filename: None,
            size: Some(4),
            url: None,
            path: None,
            data: Some(vec![0, 1, 2, 3]),
        };
        let config = WhatsAppAccountConfig {
            enabled: true,
            phone_number: None,
            access: Default::default(),
            delivery: Default::default(),
            reactions: Default::default(),
        };
        let proc = MediaProcessor::preprocess(&att, &config).unwrap();
        assert_eq!(proc.mime_type, "audio/ogg; codecs=opus");
    }
}
