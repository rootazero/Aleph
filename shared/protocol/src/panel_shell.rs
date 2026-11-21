//! The desktop WebView may request only the Panel's Chat landing page.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ShellRequest {
    Probe { request: u32 },
    OpenChat { request: u32, document: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PanelReply {
    Ready {
        request: u32,
        document: String,
    },
    ChatOpened {
        request: u32,
        document: String,
    },
    Notify {
        document: String,
        title: String,
        body: String,
    },
}

pub fn valid_document(document: &str) -> bool {
    document.len() == 32
        && document
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn valid_notification(title: &str, body: &str) -> bool {
    !title.is_empty() && title.len() <= 256 && body.len() <= 2048
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_cannot_encode_an_arbitrary_destination() {
        assert!(serde_json::from_str::<ShellRequest>(
            r#"{"kind":"open-chat","request":1,"document":"a","path":"/settings"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<ShellRequest>(
            r#"{"kind":"navigate","request":1,"document":"a"}"#
        )
        .is_err());
    }
}
