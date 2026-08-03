//! Shared helpers for the `adapter_tests::*` sub-modules.
//!
//! These mirror the request-builder utilities that lived inline in the
//! pre-split `adapter_tests.rs`. Keeping them in one place avoids 3-way
//! duplication across the per-topic test sub-files.

use crate::config::ProviderConfig;
use crate::providers::adapter::{ProtocolAdapter, RequestPayload};

use super::super::AnthropicProtocol;

pub(super) fn body_of(request: reqwest::RequestBuilder) -> serde_json::Value {
    // rust-doctor-disable-next-line unwrap-in-production
    let built = request.build().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    let body_bytes = built.body().unwrap().as_bytes().unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    serde_json::from_slice(body_bytes).unwrap()
}

pub(super) fn build_body(payload: &RequestPayload, config: &ProviderConfig) -> serde_json::Value {
    let protocol = AnthropicProtocol::new(reqwest::Client::new());
    let built = protocol
        .build_request(payload, config)
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
        .build()
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap();
    // rust-doctor-disable-next-line unwrap-in-production
    serde_json::from_slice(built.body().unwrap().as_bytes().unwrap()).unwrap()
}

/// Count `cache_control` markers anywhere in a JSON value.
///
/// Shared because the marker budget is asserted from two angles: the
/// prefix-stability contract caps it at `MAX_CACHE_BREAKPOINTS`, and the
/// retention/capability gate requires it to be exactly zero.
pub(super) fn marker_count(value: &serde_json::Value) -> usize {
    match value {
        serde_json::Value::Object(map) => {
            usize::from(map.contains_key("cache_control"))
                + map.values().map(marker_count).sum::<usize>()
        }
        serde_json::Value::Array(items) => items.iter().map(marker_count).sum(),
        _ => 0,
    }
}

pub(super) fn build_http(payload: &RequestPayload, config: &ProviderConfig) -> reqwest::Request {
    let protocol = AnthropicProtocol::new(reqwest::Client::new());
    protocol
        .build_request(payload, config)
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
        .build()
        // rust-doctor-disable-next-line unwrap-in-production
        .unwrap()
}
