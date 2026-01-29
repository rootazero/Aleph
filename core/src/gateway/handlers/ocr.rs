//! OCR RPC Handlers
//!
//! Handlers for optical character recognition (text extraction from images).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use super::super::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::config::Config;
use crate::vision::{VisionConfig, VisionService};

/// Parameters for ocr.extractText
#[derive(Debug, Deserialize)]
pub struct ExtractTextParams {
    /// Base64-encoded image data (PNG or JPEG)
    pub image: String,
}

/// Result of text extraction
#[derive(Debug, Clone, Serialize)]
pub struct ExtractTextResult {
    /// Extracted text from the image
    pub text: String,
    /// Processing time in milliseconds
    pub processing_time_ms: u64,
}

/// Extract text from an image (OCR)
///
/// Takes a base64-encoded image and returns the extracted text.
/// Uses the default AI provider configured in the application.
///
/// # Example Request
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "method": "ocr.extractText",
///   "params": {
///     "image": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJ..."
///   },
///   "id": 1
/// }
/// ```
///
/// # Example Response
///
/// ```json
/// {
///   "jsonrpc": "2.0",
///   "result": {
///     "text": "Hello, World!",
///     "processing_time_ms": 523
///   },
///   "id": 1
/// }
/// ```
pub async fn handle_extract_text(
    request: JsonRpcRequest,
    config: Arc<Config>,
) -> JsonRpcResponse {
    // Parse parameters
    let params: ExtractTextParams = match request.params {
        Some(ref p) => match serde_json::from_value(p.clone()) {
            Ok(p) => p,
            Err(e) => {
                return JsonRpcResponse::error(
                    request.id,
                    INVALID_PARAMS,
                    format!("Invalid params: {}", e),
                );
            }
        },
        None => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                "Missing params: image required".to_string(),
            );
        }
    };

    // Decode base64 image
    let image_data = match BASE64.decode(&params.image) {
        Ok(data) => data,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid base64 image data: {}", e),
            );
        }
    };

    // Create vision service
    let vision_service = VisionService::new(VisionConfig::default());

    // Extract text
    let start = std::time::Instant::now();
    match vision_service.extract_text(image_data, &config).await {
        Ok(text) => {
            let processing_time_ms = start.elapsed().as_millis() as u64;
            JsonRpcResponse::success(
                request.id,
                json!(ExtractTextResult {
                    text,
                    processing_time_ms,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("OCR failed: {}", e),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_text_params_deserialize() {
        let json = json!({
            "image": "dGVzdA=="  // "test" in base64
        });

        let params: ExtractTextParams = serde_json::from_value(json).unwrap();
        assert_eq!(params.image, "dGVzdA==");
    }

    #[test]
    fn test_extract_text_result_serialize() {
        let result = ExtractTextResult {
            text: "Hello, World!".to_string(),
            processing_time_ms: 523,
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["text"], "Hello, World!");
        assert_eq!(json["processing_time_ms"], 523);
    }

    // Note: Full integration tests require a configured AI provider
    // which is not available in unit tests.
}
