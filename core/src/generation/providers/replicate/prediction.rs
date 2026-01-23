//! Prediction creation and polling for Replicate API
//!
//! This module handles the async prediction lifecycle including creation,
//! polling, and output fetching.

use super::constants::{DEFAULT_TIMEOUT_SECS, MAX_POLL_ATTEMPTS, POLL_INTERVAL_MS};
use super::error::parse_error_response;
use super::types::{CreatePredictionRequest, PredictionResponse};
use crate::generation::{GenerationError, GenerationResult};
use reqwest::Client;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Create a new prediction and return its ID
pub async fn create_prediction(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    model: &str,
    input: serde_json::Value,
) -> GenerationResult<String> {
    let url = format!("{}/v1/predictions", endpoint);

    let request_body = CreatePredictionRequest {
        version: model.to_string(),
        input,
    };

    debug!(
        model = %model,
        url = %url,
        "Creating Replicate prediction"
    );

    let response = client
        .post(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                GenerationError::timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            } else if e.is_connect() {
                GenerationError::network(format!("Connection failed: {}", e))
            } else {
                GenerationError::network(e.to_string())
            }
        })?;

    let status = response.status();
    let response_text = response
        .text()
        .await
        .map_err(|e| GenerationError::network(format!("Failed to read response: {}", e)))?;

    if !status.is_success() {
        error!(
            status = %status,
            body = %response_text,
            "Replicate prediction creation failed"
        );
        return Err(parse_error_response(status.as_u16(), &response_text));
    }

    let prediction: PredictionResponse = serde_json::from_str(&response_text).map_err(|e| {
        GenerationError::serialization(format!("Failed to parse response: {}", e))
    })?;

    debug!(
        id = %prediction.id,
        status = %prediction.status,
        "Prediction created"
    );

    Ok(prediction.id)
}

/// Poll a prediction until it completes or fails
pub async fn poll_prediction(
    client: &Client,
    endpoint: &str,
    api_key: &str,
    id: &str,
) -> GenerationResult<PredictionResponse> {
    let url = format!("{}/v1/predictions/{}", endpoint, id);
    let mut attempts = 0;

    loop {
        attempts += 1;
        if attempts > MAX_POLL_ATTEMPTS {
            return Err(GenerationError::timeout(Duration::from_millis(
                POLL_INTERVAL_MS * MAX_POLL_ATTEMPTS as u64,
            )));
        }

        let response = client
            .get(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .send()
            .await
            .map_err(|e| GenerationError::network(e.to_string()))?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| GenerationError::network(format!("Failed to read response: {}", e)))?;

        if !status.is_success() {
            error!(
                status = %status,
                body = %response_text,
                "Failed to poll prediction"
            );
            return Err(parse_error_response(status.as_u16(), &response_text));
        }

        let prediction: PredictionResponse =
            serde_json::from_str(&response_text).map_err(|e| {
                GenerationError::serialization(format!("Failed to parse response: {}", e))
            })?;

        match prediction.status.as_str() {
            "succeeded" => {
                info!(
                    id = %id,
                    attempts = attempts,
                    "Prediction succeeded"
                );
                return Ok(prediction);
            }
            "failed" => {
                error!(
                    id = %id,
                    error = ?prediction.error,
                    "Prediction failed"
                );
                return Err(GenerationError::provider(
                    prediction
                        .error
                        .unwrap_or_else(|| "Prediction failed".to_string()),
                    None,
                    "replicate",
                ));
            }
            "canceled" => {
                warn!(id = %id, "Prediction was canceled");
                return Err(GenerationError::provider(
                    "Prediction was canceled",
                    None,
                    "replicate",
                ));
            }
            status => {
                debug!(
                    id = %id,
                    status = %status,
                    attempts = attempts,
                    "Prediction in progress, polling..."
                );
                tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
            }
        }
    }
}

/// Fetch the output from a URL and return as bytes
pub async fn fetch_output(
    client: &Client,
    url: &str,
) -> GenerationResult<(Vec<u8>, Option<String>)> {
    debug!(url = %url, "Fetching prediction output");

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| GenerationError::download(e.to_string(), Some(url.to_string())))?;

    if !response.status().is_success() {
        return Err(GenerationError::download(
            format!("HTTP {}", response.status()),
            Some(url.to_string()),
        ));
    }

    // Get content type from headers
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let bytes = response
        .bytes()
        .await
        .map_err(|e| GenerationError::download(e.to_string(), Some(url.to_string())))?;

    Ok((bytes.to_vec(), content_type))
}
