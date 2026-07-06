use crate::config::Config;
use crate::gateway::handlers::parse_params;
use crate::gateway::handlers::search_config::dto::resolve_api_key;
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};
use crate::gateway::security::SharedTokenManager;
use crate::search::providers::{
    BingProvider, BraveProvider, DuckDuckGoProvider, ExaProvider, FirecrawlProvider,
    GoogleProvider, JinaProvider, SearxngProvider, TavilyProvider,
};
use crate::search::{SearchOptions, SearchProvider};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTestResult {
    pub success: bool,
    pub message: String,
}

/// Test a search backend connection
pub async fn handle_test(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    vault: Arc<SharedTokenManager>,
) -> JsonRpcResponse {
    #[derive(Deserialize)]
    struct Params {
        /// Backend name (used to persist verified=true on success)
        name: String,
        #[serde(default)]
        api_key: Option<String>,
        #[serde(default)]
        base_url: Option<String>,
        #[serde(default)]
        engine_id: Option<String>,
        /// `SearXNG` only — comma-separated upstream engines to pin for the probe.
        #[serde(default)]
        engines: Option<String>,
    }

    let mut params: Params = match parse_params(&request) {
        Ok(p) => p,
        Err(e) => return e,
    };

    // Resolve API key from vault if not provided inline
    if params.api_key.is_none() {
        params.api_key = resolve_api_key(&params.name, &vault);
    }

    // Determine provider type from config or fallback to name
    let provider_type = {
        let cfg = config.read().await;
        cfg.search
            .as_ref()
            .and_then(|s| s.backends.get(&params.name))
            .map_or_else(|| params.name.clone(), |b| b.provider_type.clone())
    };

    // Create a temporary search provider and test it
    let test_result: SearchTestResult = match provider_type.as_str() {
        "tavily" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Tavily"}),
                );
            };
            match TavilyProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "brave" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Brave"}),
                );
            };
            match BraveProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "firecrawl" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Firecrawl"}),
                );
            };
            match FirecrawlProvider::new(api_key.clone(), params.base_url.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "searxng" => {
            let base_url = params
                .base_url
                .unwrap_or_else(|| "http://localhost:8888".to_string());
            // Connectivity test: pin the same engines the operator configured
            // (so a default-engine-set that returns nothing isn't mistaken for a
            // broken backend), no throttle (Some(0)) so the probe returns promptly.
            match SearxngProvider::new(base_url, params.engines.clone(), Some(0)) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "google" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Google"}),
                );
            };
            let Some(ref engine_id) = params.engine_id else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "Engine ID (cx) is required for Google CSE"}),
                );
            };
            match GoogleProvider::new(api_key.clone(), engine_id.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "bing" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Bing"}),
                );
            };
            match BingProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "exa" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Exa"}),
                );
            };
            match ExaProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "jina" => {
            let Some(ref api_key) = params.api_key else {
                return JsonRpcResponse::success(
                    request.id,
                    serde_json::json!({"success": false, "message": "API key is required for Jina"}),
                );
            };
            match JinaProvider::new(api_key.clone()) {
                Ok(provider) => {
                    let opts = SearchOptions {
                        max_results: 1,
                        ..Default::default()
                    };
                    match provider.search("test", &opts).await {
                        Ok(_) => SearchTestResult {
                            success: true,
                            message: "Connection successful".to_string(),
                        },
                        Err(e) => SearchTestResult {
                            success: false,
                            message: format!("Search failed: {e}"),
                        },
                    }
                }
                Err(e) => SearchTestResult {
                    success: false,
                    message: format!("Failed to create provider: {e}"),
                },
            }
        }
        "duckduckgo" => match DuckDuckGoProvider::new() {
            Ok(provider) => {
                let opts = SearchOptions {
                    max_results: 1,
                    ..Default::default()
                };
                match provider.search("test", &opts).await {
                    Ok(_) => SearchTestResult {
                        success: true,
                        message: "Connection successful".to_string(),
                    },
                    Err(e) => SearchTestResult {
                        success: false,
                        message: format!("Search failed: {e}"),
                    },
                }
            }
            Err(e) => SearchTestResult {
                success: false,
                message: format!("Failed to create provider: {e}"),
            },
        },
        _ => SearchTestResult {
            success: false,
            message: format!("Unknown provider type: {provider_type}"),
        },
    };

    // Persist verified=true on success
    if test_result.success {
        let mut cfg = config.write().await;
        if let Some(search) = &mut cfg.search {
            if let Some(backend) = search.backends.get_mut(&params.name) {
                backend.verified = true;
                if let Err(e) = cfg.save_incremental(&["search"]) {
                    tracing::error!(error = %e, "Failed to save config after search test");
                }
            }
        }
    }

    match serde_json::to_value(test_result) {
        Ok(v) => JsonRpcResponse::success(request.id, v),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize result: {e}"),
        ),
    }
}
