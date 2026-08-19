//! Extism host function registrations for WASM plugins.
//!
//! Registers host functions that are injected into the WASM sandbox:
//! - log(level, message) — controlled logging
//! - `now_millis()` -> u64 — current timestamp
//! - `workspace_read(path)` -> JSON string — read workspace files
//! - `secret_exists(name)` -> "true"/"false" — check secret availability
//! - `http_fetch(request)` -> JSON string — sandboxed, allowlisted outbound HTTP

use std::collections::HashMap;

use crate::sync_primitives::Arc;

use extism::host_fn;
use serde::Deserialize;

use super::capability_kernel::WasmCapabilityKernel;

/// Shared state passed to all host functions via Extism `UserData`
pub struct HostState {
    pub kernel: Arc<WasmCapabilityKernel>,
    pub workspace_root: std::path::PathBuf,
}

/// Maximum bytes `workspace_read` will load from a single file.
const MAX_WORKSPACE_READ_BYTES: u64 = 1024 * 1024;

host_fn!(pub host_log(state: HostState; level: String, message: String) {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    let _ = state.kernel.log(&level, &message);
    Ok(())
});

host_fn!(pub host_now_millis(state: HostState;) -> u64 {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    Ok(state.kernel.now_millis())
});

host_fn!(pub host_workspace_read(state: HostState; path: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());

    // Check capability
    if let Err(e) = state.kernel.check_workspace_read(&path) {
        return Ok(serde_json::json!({"error": e.to_string()}).to_string());
    }

    // Read file from workspace. The capability check above is lexical only and
    // cannot see symlinks, so canonicalize the resolved path and confine it to
    // the workspace root — a symlink inside the workspace must not escape the
    // sandbox.
    let full_path = state.workspace_root.join(&path);
    let canonical = match std::fs::canonicalize(&full_path) {
        Ok(p) => p,
        Err(e) => return Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    };
    let root = match std::fs::canonicalize(&state.workspace_root) {
        Ok(p) => p,
        Err(e) => return Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    };
    if !canonical.starts_with(&root) {
        return Ok(serde_json::json!({"error": "path escapes workspace"}).to_string());
    }
    // Cap the read: a plugin with the workspace capability must not pull
    // arbitrarily large files into guest memory.
    let file_len = match std::fs::metadata(&canonical) {
        Ok(m) => m.len(),
        Err(e) => return Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    };
    if file_len > MAX_WORKSPACE_READ_BYTES {
        return Ok(serde_json::json!({
            "error": format!("file {file_len} bytes exceeds cap {MAX_WORKSPACE_READ_BYTES}")
        })
        .to_string());
    }
    match std::fs::read_to_string(&canonical) {
        Ok(content) => Ok(serde_json::json!({"content": content}).to_string()),
        Err(e) => Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    }
});

// Existence has to be answered by the same two things resolution consults,
// or the guest is told a secret exists and then every request that uses it
// hard-fails. This asked only the manifest allowlist until 2026-08-19 — the
// plugin's own declaration of what it is *permitted* to see, which says
// nothing about what the host can actually resolve.
host_fn!(pub host_secret_exists(state: HostState; name: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    let exists = state.kernel.check_secret_pattern(&name)
        && state.kernel.resolve_secret(&name).is_some();
    Ok(exists.to_string())
});

host_fn!(pub host_http_fetch(state: HostState; request: String) -> String {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    Ok(do_http_fetch(state.kernel.as_ref(), &request))
});

/// Request envelope a plugin passes to the `http_fetch` host function.
#[derive(Deserialize)]
struct HttpRequest {
    method: String,
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
    #[serde(default)]
    body: Option<String>,
}

/// Orchestrate a sandboxed outbound HTTP request for a WASM plugin.
///
/// Enforces, in order: the `http` capability must be declared, the request must
/// pass the allowlist (HTTPS-only, anti host-confusion, path-traversal safe),
/// the per-execution call-count cap, the rate limit, and the request/response
/// body size caps. Always returns a JSON string — `{"status","headers","body"}`
/// on success or `{"error":...}` on any rejection — and never panics.
fn do_http_fetch(kernel: &WasmCapabilityKernel, request: &str) -> String {
    match try_http_fetch(kernel, request) {
        Ok(json) => json,
        Err(msg) => serde_json::json!({ "error": msg }).to_string(),
    }
}

fn try_http_fetch(kernel: &WasmCapabilityKernel, request: &str) -> Result<String, String> {
    let req: HttpRequest =
        serde_json::from_str(request).map_err(|e| format!("invalid request: {e}"))?;
    let method = req.method.to_uppercase();

    // Capability + allowlist, call-count, and rate-limit gates (in that order).
    kernel
        .check_http_request(&method, &req.url)
        .map_err(|e| e.to_string())?;
    kernel.check_http_limit().map_err(|e| e.to_string())?;
    kernel.check_rate_limit().map_err(|e| e.to_string())?;

    // Present once check_http_request succeeded; size caps + timeout live here.
    let http = kernel
        .http_config()
        .ok_or_else(|| "http capability missing".to_string())?;

    let body = req.body.clone().unwrap_or_default();
    if body.len() > http.max_request_bytes {
        return Err(format!(
            "request body {} bytes exceeds cap {}",
            body.len(),
            http.max_request_bytes
        ));
    }

    let timeout = std::time::Duration::from_secs(http.timeout_secs);
    let max_response_bytes = http.max_response_bytes;

    // ─── Credential injection (host-side, before egress) ──────────────────
    // The plugin declares `http.credentials: Vec<CredentialBinding>` in its
    // manifest; each binding names a secret + injection strategy + host
    // patterns. The resolver supplies the secret value, the injector applies
    // it to headers / URL — the plugin guest never sees the plaintext.
    //
    // We collect every binding's resolved value into a single slice up front
    // (rather than calling inject_credential once per binding with a fresh
    // Vec) so the host has one consistent view of the secret store across
    // all bindings. This matters when two bindings share a `secret_name`:
    // resolving once prevents a TOCTOU race against a resolver that mutates.
    let mut resolved_secrets: Vec<(String, String)> = Vec::with_capacity(http.credentials.len());
    for binding in &http.credentials {
        if let Some(value) = kernel.resolve_secret(&binding.secret_name) {
            // `inject_credential` looks up by name; duplicates are harmless
            // (first-match wins).
            if !resolved_secrets
                .iter()
                .any(|(n, _)| n == &binding.secret_name)
            {
                resolved_secrets.push((binding.secret_name.clone(), value));
            }
        }
    }

    // Apply each binding in declaration order. Order matters when two
    // bindings target the same URL: a later Bearer binding can override an
    // earlier Authorization header.
    let mut egress_headers: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let mut egress_url = req.url.clone();
    for binding in &http.credentials {
        match super::credential_injector::inject_credential(
            binding,
            &egress_url,
            &mut egress_headers,
            &resolved_secrets,
        ) {
            Ok(Some(modified_url)) => {
                egress_url = modified_url;
            }
            Ok(None) => {
                // Header-only mutation, or host-pattern did not match (silent
                // skip). Both are correct outcomes.
            }
            Err(err) => {
                // A declared binding matched the URL host but the resolver
                // returned `None` for its secret name — surface as a hard
                // failure rather than silently dropping the credential and
                // letting the request through unauthenticated.
                return Err(format!("credential injection failed: {err}"));
            }
        }
    }

    // Run the blocking client on a dedicated OS thread: reqwest::blocking spins
    // its own runtime internally, which would panic if nested inside the host's
    // tokio worker. A fresh std thread carries no ambient runtime.
    let (status, headers, bytes): (u16, HashMap<String, String>, Vec<u8>) =
        std::thread::scope(|scope| {
            scope
                .spawn(
                    || -> Result<(u16, HashMap<String, String>, Vec<u8>), String> {
                        use std::io::Read;

                        let client = reqwest::blocking::Client::builder()
                            .timeout(timeout)
                            // Do not follow redirects: the allowlist was
                            // validated against the request URL only, and a
                            // 30x from an allowlisted host would otherwise
                            // silently egress to an arbitrary target.
                            .redirect(reqwest::redirect::Policy::none())
                            .build()
                            .map_err(|e| format!("client build failed: {e}"))?;
                        let parsed_method = reqwest::Method::from_bytes(method.as_bytes())
                            .map_err(|e| format!("invalid method: {e}"))?;
                        let mut builder = client.request(parsed_method, egress_url.as_str());
                        for (k, v) in &egress_headers {
                            builder = builder.header(k.as_str(), v.as_str());
                        }
                        if !body.is_empty() {
                            builder = builder.body(body.clone());
                        }
                        let resp = builder.send().map_err(|e| format!("request failed: {e}"))?;
                        let status = resp.status().as_u16();
                        let headers = resp
                            .headers()
                            .iter()
                            .map(|(k, v)| {
                                (k.as_str().to_string(), v.to_str().unwrap_or("").to_string())
                            })
                            .collect();
                        let mut bytes = Vec::new();
                        let response_cap = u64::try_from(max_response_bytes)
                            .map_or(u64::MAX, |size| size.saturating_add(1));
                        resp.take(response_cap)
                            .read_to_end(&mut bytes)
                            .map_err(|e| format!("failed to read response: {e}"))?;
                        Ok((status, headers, bytes))
                    },
                )
                .join()
                .map_err(|_| "http worker thread panicked".to_string())?
        })?;

    if bytes.len() > max_response_bytes {
        return Err(format!(
            "response {} bytes exceeds cap {}",
            bytes.len(),
            max_response_bytes
        ));
    }
    let body_text = String::from_utf8_lossy(&bytes).to_string();

    Ok(serde_json::json!({
        "status": status,
        "headers": headers,
        "body": body_text,
    })
    .to_string())
}
