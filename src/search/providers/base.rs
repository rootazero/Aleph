use crate::error::{AlephError, Result};
use reqwest::{Client, Response, StatusCode};
use std::time::Duration;

/// Build a default HTTP client with 30-second connect timeout.
pub(crate) fn build_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| AlephError::network(e.to_string()))
}

/// Check HTTP response status and map to typed errors.
///
/// Returns the response unchanged on success. Maps 401/403 to
/// `AlephError::AuthenticationError`, everything else to
/// `AlephError::ProviderError`.
pub(crate) fn check_status(response: Response, provider_name: &str) -> Result<Response> {
    let status = response.status();
    if status.is_success() {
        Ok(response)
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        Err(AlephError::authentication(
            provider_name,
            format!("{provider_name} API error: {status}"),
        ))
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        Err(AlephError::rate_limit(format!(
            "{provider_name} API error: {status} (rate limited)"
        )))
    } else {
        Err(AlephError::provider(format!(
            "{provider_name} API error: {status}"
        )))
    }
}

/// Parse JSON response body with provider-specific error context.
pub(crate) async fn parse_json<T: serde::de::DeserializeOwned>(
    response: Response,
    provider_name: &str,
) -> Result<T> {
    response
        .json::<T>()
        .await
        .map_err(|e| AlephError::provider(format!("Failed to parse {provider_name} response: {e}")))
}
