//! Input building for Replicate predictions
//!
//! This module handles building the input object for different generation types.

use crate::generation::{GenerationRequest, GenerationType};

/// Build the input object for a prediction request
pub fn build_input(request: &GenerationRequest) -> serde_json::Value {
    let mut input = serde_json::Map::new();

    // Always include the prompt
    input.insert("prompt".to_string(), serde_json::json!(request.prompt));

    // Add optional parameters based on generation type
    match request.generation_type {
        GenerationType::Image => {
            if let Some(width) = request.params.width {
                input.insert("width".to_string(), serde_json::json!(width));
            }
            if let Some(height) = request.params.height {
                input.insert("height".to_string(), serde_json::json!(height));
            }
            if let Some(n) = request.params.n {
                input.insert("num_outputs".to_string(), serde_json::json!(n));
            }
            if let Some(seed) = request.params.seed {
                input.insert("seed".to_string(), serde_json::json!(seed));
            }
            if let Some(ref negative) = request.params.negative_prompt {
                input.insert("negative_prompt".to_string(), serde_json::json!(negative));
            }
            if let Some(guidance) = request.params.guidance_scale {
                input.insert("guidance_scale".to_string(), serde_json::json!(guidance));
            }
            if let Some(steps) = request.params.steps {
                input.insert("num_inference_steps".to_string(), serde_json::json!(steps));
            }
        }
        GenerationType::Audio => {
            if let Some(duration) = request.params.duration_seconds {
                input.insert("duration".to_string(), serde_json::json!(duration));
            }
            if let Some(ref reference) = request.params.reference_audio {
                input.insert("melody".to_string(), serde_json::json!(reference));
            }
        }
        GenerationType::Video => {
            if let Some(duration) = request.params.duration_seconds {
                input.insert("duration".to_string(), serde_json::json!(duration));
            }
            if let Some(fps) = request.params.fps {
                input.insert("fps".to_string(), serde_json::json!(fps));
            }
        }
        GenerationType::Speech => {
            if let Some(ref voice) = request.params.voice {
                input.insert("voice".to_string(), serde_json::json!(voice));
            }
            if let Some(speed) = request.params.speed {
                input.insert("speed".to_string(), serde_json::json!(speed));
            }
        }
    }

    // Add any extra parameters
    for (key, value) in &request.params.extra {
        input.insert(key.clone(), value.clone());
    }

    serde_json::Value::Object(input)
}
