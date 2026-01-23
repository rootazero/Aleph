//! Request and Response types for Replicate API
//!
//! This module contains the data structures for Replicate's prediction API.

use serde::{Deserialize, Serialize};

/// Request body for creating a prediction
#[derive(Debug, Serialize)]
pub struct CreatePredictionRequest {
    /// Model version to run
    pub version: String,
    /// Input parameters for the model
    pub input: serde_json::Value,
}

/// Response from prediction endpoints
#[derive(Debug, Deserialize)]
pub struct PredictionResponse {
    /// Prediction ID
    pub id: String,
    /// Current status (starting, processing, succeeded, failed, canceled)
    pub status: String,
    /// Output data (when succeeded)
    pub output: Option<serde_json::Value>,
    /// Error message (when failed)
    pub error: Option<String>,
}

/// Error response format
#[derive(Debug, Deserialize)]
pub struct ErrorResponse {
    /// Error title
    pub title: Option<String>,
    /// Error detail message
    pub detail: Option<String>,
}
