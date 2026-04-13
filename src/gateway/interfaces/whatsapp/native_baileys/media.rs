use crate::gateway::interfaces::whatsapp::native_baileys::errors::NativeBaileysError;

pub struct MediaProcessor {
    http_client: reqwest::Client,
}

impl MediaProcessor {
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn download_media(&self, _url: &str) -> Result<Vec<u8>, NativeBaileysError> {
        Err(NativeBaileysError::MediaError("Not implemented".into()))
    }
}

impl Default for MediaProcessor {
    fn default() -> Self {
        Self::new()
    }
}
