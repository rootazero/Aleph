// core/src/providers/protocols/configurable.rs

//! Configurable protocol adapter loaded from YAML

use crate::config::ProviderConfig;
use crate::error::{AetherError, Result};
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};
use crate::providers::protocols::ProtocolDefinition;
use async_trait::async_trait;
use futures::stream::BoxStream;
use reqwest::Client;

/// Protocol adapter configured from YAML definition
pub struct ConfigurableProtocol {
    definition: ProtocolDefinition,
    client: Client,
}

impl ConfigurableProtocol {
    /// Create a new configurable protocol
    pub fn new(definition: ProtocolDefinition, client: Client) -> Self {
        Self { definition, client }
    }
}

#[async_trait]
impl ProtocolAdapter for ConfigurableProtocol {
    fn build_request(
        &self,
        _payload: &RequestPayload,
        _config: &ProviderConfig,
        _is_streaming: bool,
    ) -> Result<reqwest::RequestBuilder> {
        // TODO: Implement in follow-up task
        Err(AetherError::provider("ConfigurableProtocol not yet implemented"))
    }

    async fn parse_response(&self, _response: reqwest::Response) -> Result<String> {
        // TODO: Implement in follow-up task
        Err(AetherError::provider("ConfigurableProtocol not yet implemented"))
    }

    async fn parse_stream(
        &self,
        _response: reqwest::Response,
    ) -> Result<BoxStream<'static, Result<String>>> {
        // TODO: Implement in follow-up task
        Err(AetherError::provider("ConfigurableProtocol not yet implemented"))
    }

    fn name(&self) -> &'static str {
        // SAFETY: We leak the string to get a 'static lifetime
        // This is acceptable for protocol names which are created rarely
        Box::leak(self.definition.name.clone().into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_configurable_protocol_name() {
        let def = ProtocolDefinition {
            name: "test-proto".to_string(),
            extends: None,
            base_url: None,
            differences: None,
            custom: None,
        };

        let client = reqwest::Client::new();
        let proto = ConfigurableProtocol::new(def, client);
        assert_eq!(proto.name(), "test-proto");
    }
}
