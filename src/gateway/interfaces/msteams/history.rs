//! Message History Fetcher

use crate::sync_primitives::Arc;

use crate::gateway::channel::ChannelError;
use crate::gateway::interfaces::msteams::graph::{GraphClient, GraphMessage};

pub struct HistoryFetcher {
    graph_client: Arc<GraphClient>,
    max_messages: usize,
}

impl HistoryFetcher {
    pub fn new(graph_client: Arc<GraphClient>, max_messages: usize) -> Self {
        Self {
            graph_client,
            max_messages,
        }
    }

    /// Fetch recent messages from a channel
    pub async fn fetch_history(
        &self,
        team_id: &str,
        channel_id: &str,
        before_message_id: Option<&str>,
    ) -> Result<Vec<GraphMessage>, ChannelError> {
        let team_encoded = encode_uri_component(team_id);
        let channel_encoded = encode_uri_component(channel_id);

        let url = if let Some(before) = before_message_id {
            let before_encoded = encode_uri_component(before);
            format!(
                "/teams/{}/channels/{}/messages?$top={}&$orderby=createdDateTime%20desc&$beforeMessage={}",
                team_encoded,
                channel_encoded,
                self.max_messages.min(50),
                before_encoded
            )
        } else {
            format!(
                "/teams/{}/channels/{}/messages?$top={}&$orderby=createdDateTime%20desc",
                team_encoded,
                channel_encoded,
                self.max_messages.min(50)
            )
        };

        #[derive(serde::Deserialize)]
        struct MessagesResponse {
            value: Vec<GraphMessage>,
        }

        let response: MessagesResponse = self.graph_client.get(&url).await?;
        Ok(response.value)
    }

    /// Fetch a single message with full context
    pub async fn fetch_message(
        &self,
        team_id: &str,
        channel_id: &str,
        message_id: &str,
    ) -> Result<GraphMessage, ChannelError> {
        let endpoint = format!(
            "/teams/{}/channels/{}/messages/{}",
            encode_uri_component(team_id),
            encode_uri_component(channel_id),
            encode_uri_component(message_id)
        );

        self.graph_client.get(&endpoint).await
    }
}

fn encode_uri_component(value: &str) -> String {
    let mut result = String::new();
    for c in value.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            ' ' => result.push_str("%20"),
            ':' => result.push_str("%3A"),
            '/' => result.push_str("%2F"),
            _ => result.push_str(&format!("%{:02X}", c as u8)),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_uri_component() {
        assert_eq!(encode_uri_component("simple"), "simple");
        assert_eq!(encode_uri_component("hello world"), "hello%20world");
        assert_eq!(
            encode_uri_component("19:conv@thread.v2"),
            "19%3Aconv%40thread.v2"
        );
    }
}
