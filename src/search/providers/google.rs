use crate::error::{AlephError, Result};
use crate::search::providers::base::build_client;
use crate::search::{SearchOptions, SearchProvider, SearchResult};
use async_trait::async_trait;
use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;

/// Google Custom Search Engine provider
///
/// Google CSE provides comprehensive search coverage
const NAME: &str = "google";

#[derive(Debug)]
pub struct GoogleProvider {
    api_key: String,
    engine_id: String,
    client: Client,
}

#[derive(Deserialize)]
struct GoogleResponse {
    #[serde(default)]
    items: Option<Vec<GoogleItem>>,
}

#[derive(Deserialize)]
struct GoogleItem {
    title: String,
    link: String,
    #[serde(default)]
    snippet: Option<String>,
}

/// Sanitize a message by replacing occurrences of the API key.
fn sanitize_api_key(msg: String, key: &str) -> String {
    if key.is_empty() {
        return msg;
    }
    msg.replace(key, "***REDACTED***")
}

/// Check HTTP response status with API-key sanitization in error messages.
fn check_status_google(response: Response, provider_name: &str, api_key: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let msg = sanitize_api_key(
            format!("{} API error: {}", provider_name, status),
            api_key,
        );
        Err(AlephError::authentication(provider_name, msg))
    } else {
        let msg = sanitize_api_key(
            format!("{} API error: {}", provider_name, status),
            api_key,
        );
        Err(AlephError::provider(msg))
    }
}

impl GoogleProvider {
    pub fn new(api_key: impl Into<String>, engine_id: impl Into<String>) -> Result<Self> {
        let api_key = api_key.into();
        let engine_id = engine_id.into();
        if api_key.is_empty() {
            return Err(AlephError::invalid_config("Google API key is required"));
        }
        if engine_id.is_empty() {
            return Err(AlephError::invalid_config(
                "Google Custom Search Engine ID is required",
            ));
        }

        Ok(Self {
            api_key,
            engine_id,
            client: build_client()?,
        })
    }
}

#[async_trait]
impl SearchProvider for GoogleProvider {
    async fn search(&self, query: &str, options: &SearchOptions) -> Result<Vec<SearchResult>> {
        // Google CSE API limits num parameter to maximum 10
        let max_results = options.validated_max_results().min(10);
        let response = self
            .client
            .get("https://www.googleapis.com/customsearch/v1")
            .query(&[
                ("key", self.api_key.as_str()),
                ("cx", self.engine_id.as_str()),
                ("q", query),
                ("num", &max_results.to_string()),
            ])
            .timeout(std::time::Duration::from_secs(options.validated_timeout()))
            .send()
            .await
            .map_err(|e| {
                let msg = sanitize_api_key(e.to_string(), &self.api_key);
                AlephError::network(msg)
            })?;

        let response = check_status_google(response, NAME, &self.api_key)?;

        let google_response: GoogleResponse = response
            .json()
            .await
            .map_err(|e| {
                let msg = sanitize_api_key(e.to_string(), &self.api_key);
                AlephError::provider(format!("Failed to parse Google response: {}", msg))
            })?;

        let results = google_response
            .items
            .unwrap_or_default()
            .into_iter()
            .map(|item| SearchResult {
                title: item.title,
                url: item.link,
                snippet: item.snippet.unwrap_or_default(),
                published_date: None,
                relevance_score: None,
                source_type: None,
                full_content: None,
                provider: Some(NAME.to_string()),
            })
            .collect();

        Ok(results)
    }

    fn name(&self) -> &str {
        NAME
    }

    fn is_available(&self) -> bool {
        !self.api_key.is_empty() && !self.engine_id.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_google_provider_creation() {
        let provider =
            GoogleProvider::new("AIza_test_key".to_string(), "cx_test_engine".to_string()).unwrap();
        assert_eq!(provider.name(), "google");
        assert!(provider.is_available());
    }

    #[test]
    fn test_google_provider_requires_both_keys() {
        let result1 = GoogleProvider::new("".to_string(), "engine".to_string());
        assert!(result1.is_err());

        let result2 = GoogleProvider::new("key".to_string(), "".to_string());
        assert!(result2.is_err());
    }

    #[test]
    fn test_sanitize_api_key() {
        let msg = "error for key=SECRET123".to_string();
        let sanitized = sanitize_api_key(msg, "SECRET123");
        assert!(!sanitized.contains("SECRET123"));
        assert!(sanitized.contains("***REDACTED***"));
    }

    #[test]
    fn test_sanitize_api_key_empty_key() {
        let msg = "some error".to_string();
        let sanitized = sanitize_api_key(msg.clone(), "");
        assert_eq!(sanitized, msg);
    }
}
