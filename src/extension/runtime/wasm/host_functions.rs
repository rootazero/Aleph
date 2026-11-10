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
    /// Context payloads must not be persisted through the guest logging import.
    pub suppress_guest_logs: bool,
}

/// Maximum bytes `workspace_read` will load from a single file.
const MAX_WORKSPACE_READ_BYTES: u64 = 1024 * 1024;

host_fn!(pub host_log(state: HostState; level: String, message: String) {
    let state = state.get()?;
    let state = state.lock().unwrap_or_else(|e| e.into_inner());
    if !state.suppress_guest_logs {
        let _ = state.kernel.log(&level, &message);
    }
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

    // Check capability (lexical prefix check against the declared allowed paths)
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
    // Re-apply the allowed-prefix boundary check to the canonical path so that
    // an in-root symlink (e.g. docs/link -> ../secrets) cannot be used to reach
    // a target outside the granted prefix. We strip the canonical workspace root
    // from the canonical path to obtain the workspace-relative suffix, then
    // re-run the same capability check against that suffix.
    let canonical_relative = match canonical.strip_prefix(&root) {
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => return Ok(serde_json::json!({"error": "path escapes workspace"}).to_string()),
    };
    if let Err(e) = state.kernel.check_workspace_read(&canonical_relative) {
        return Ok(serde_json::json!({"error": e.to_string()}).to_string());
    }
    // Cap the read: a plugin with the workspace capability must not pull
    // arbitrarily large files into guest memory.
    // Open the file and confirm it is a regular file before reading, so that
    // FIFOs and other special files cannot block indefinitely. Then read
    // through a bounded reader: if the content exceeds the cap we reject it
    // even when metadata().len() reported a smaller (or zero) size.
    let file = match std::fs::File::open(&canonical) {
        Ok(f) => f,
        Err(e) => return Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    };
    let metadata = match file.metadata() {
        Ok(m) => m,
        Err(e) => return Ok(serde_json::json!({"error": e.to_string()}).to_string()),
    };
    if !metadata.is_file() {
        return Ok(serde_json::json!({"error": "path is not a regular file"}).to_string());
    }
    // Read at most MAX_WORKSPACE_READ_BYTES + 1 bytes so we can distinguish
    // "fits within cap" from "exceeds cap" without reading the whole file.
    let mut limited = std::io::Read::take(file, MAX_WORKSPACE_READ_BYTES + 1);
    let mut buf = Vec::new();
    if let Err(e) = std::io::Read::read_to_end(&mut limited, &mut buf) {
        return Ok(serde_json::json!({"error": e.to_string()}).to_string());
    }
    if buf.len() as u64 > MAX_WORKSPACE_READ_BYTES {
        return Ok(serde_json::json!({
            "error": format!("file exceeds cap {MAX_WORKSPACE_READ_BYTES}")
        })
        .to_string());
    }
    match String::from_utf8(buf) {
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

/// Headers that name a target other than the URL the allowlist validated.
///
/// The allowlist matches on the URL (scheme, host, path, method). A guest
/// that can set these headers can name a different target through the
/// server's own virtual-host / path-rewrite handling — `Host` for the
/// former, `X-Forwarded-Host` / `X-Original-URL` / `X-Rewrite-URL` for the
/// latter — so the host-routing headers cannot come from the guest.
const HOST_ROUTING_HEADERS: &[&str] = &[
    "host",
    "forwarded",
    "x-forwarded-host",
    "x-forwarded-port",
    "x-forwarded-proto",
    "x-forwarded-server",
    "x-original-host",
    "x-original-url",
    "x-rewrite-url",
];

/// Drop every host-routing header from the outbound header list.
///
/// Called *after* credential injection: a `CredentialInject::Header` binding
/// names its header freely, so it can reintroduce a `Host` the guest's own
/// headers were not allowed to set.
fn strip_host_routing_headers(headers: &mut Vec<(String, String)>) {
    headers.retain(|(name, _)| {
        !HOST_ROUTING_HEADERS
            .iter()
            .any(|blocked| name.eq_ignore_ascii_case(blocked))
    });
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
        // A binding naming a secret outside `[capabilities.secrets]
        // allowed_patterns` is a manifest error, not a runtime condition: the
        // author declared a credential the plugin has not declared the right to
        // read. `resolve_secret` refuses it silently (a guest *asking* is
        // normal), so the diagnostic has to be raised here, where the manifest
        // is the thing at fault. Without it the request just fails closed with
        // "secret not found", which sends the author looking in the vault for a
        // key that is present.
        if !kernel.check_secret_pattern(&binding.secret_name) {
            tracing::warn!(
                plugin_id = %kernel.plugin_id(),
                secret = %binding.secret_name,
                "credential binding names a secret outside this plugin's \
                 [capabilities.secrets] allowed_patterns; add the pattern there \
                 or the binding can never resolve"
            );
        }
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

    // The URL is the only thing the allowlist checked, so the headers must
    // not be able to name another target: strip them here, after the
    // injection loop, so a `Header` binding cannot reintroduce one either.
    // With no `Host` of its own, the client derives it from the URL — the
    // host the allowlist matched.
    strip_host_routing_headers(&mut egress_headers);
    // Credential injection may change the path; validate the final target too.
    kernel
        .check_http_request(&method, &egress_url)
        .map_err(|_| "Not allowed: credential-injected URL failed allowlist".to_string())?;

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
                            .map_err(|e| format!("client build failed: {}", e.without_url()))?;
                        let parsed_method = reqwest::Method::from_bytes(method.as_bytes())
                            .map_err(|e| format!("invalid method: {e}"))?;
                        let mut builder = client.request(parsed_method, egress_url.as_str());
                        for (k, v) in &egress_headers {
                            builder = builder.header(k.as_str(), v.as_str());
                        }
                        if !body.is_empty() {
                            builder = builder.body(body.clone());
                        }
                        let resp = builder
                            .send()
                            .map_err(|e| format!("request failed: {}", e.without_url()))?;
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
                .map_err(|e| match e.downcast_ref::<String>() {
                    Some(s) => format!("http worker thread panicked: {s}"),
                    None => match e.downcast_ref::<&'static str>() {
                        Some(s) => format!("http worker thread panicked: {s}"),
                        None => "http worker thread panicked (non-string payload)".to_string(),
                    },
                })?
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

fn read_workspace_file(state: &HostState, relative_path: &str) -> Result<Vec<u8>, String> {
    use std::path::{Component, Path, PathBuf};

    let root = state.workspace_root.canonicalize().map_err(|e| {
        format!(
            "workspace root {} is not accessible: {e}",
            state.workspace_root.display()
        )
    })?;

    let mut resolved = PathBuf::from(&root);
    for component in Path::new(relative_path).components() {
        match component {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => {}
            _ => return Err(format!("path {relative_path} escapes the workspace")),
        }
    }

    let canonical = resolved
        .canonicalize()
        .map_err(|e| format!("workspace path {relative_path} is not accessible: {e}"))?;

    if !canonical.starts_with(&root) {
        return Err(format!("path {relative_path} escapes the workspace"));
    }

    std::fs::read(&canonical).map_err(|e| format!("failed to read {}: {e}", canonical.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::runtime::wasm::capabilities::{
        CredentialBinding, CredentialInject, EndpointPattern, HttpCapability, SecretsCapability,
        WasmCapabilities, WorkspaceCapability,
    };
    use crate::extension::runtime::wasm::limits::WasmResourceLimits;
    use crate::extension::runtime::wasm::secret_resolver::InMemorySecretResolver;

    fn workspace_state(root: &std::path::Path) -> HostState {
        HostState {
            kernel: Arc::new(WasmCapabilityKernel::new(
                "workspace-test".into(),
                WasmCapabilities {
                    workspace: Some(WorkspaceCapability {
                        allowed_prefixes: vec!["docs/".into()],
                    }),
                    ..Default::default()
                },
                WasmResourceLimits::default(),
            )),
            workspace_root: root.into(),
            suppress_guest_logs: false,
        }
    }

    #[cfg(unix)]
    #[test]
    fn workspace_symlinks_must_remain_in_granted_prefix() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("docs")).unwrap();
        std::fs::create_dir(root.join("secrets")).unwrap();
        std::fs::write(root.join("docs/readme"), "allowed").unwrap();
        std::fs::write(root.join("secrets/key.pem"), "private").unwrap();
        std::os::unix::fs::symlink("../secrets", root.join("docs/link")).unwrap();
        std::os::unix::fs::symlink("readme", root.join("docs/alias")).unwrap();
        std::os::unix::fs::symlink(
            directory.path().parent().unwrap(),
            root.join("docs/outside"),
        )
        .unwrap();
        let state = workspace_state(root);
        assert!(read_workspace_file(&state, "docs/link/key.pem").is_err());
        assert!(read_workspace_file(&state, "docs/outside").is_err());
        assert!(read_workspace_file(&state, "docs/../secrets/key.pem").is_err());
        assert_eq!(
            read_workspace_file(&state, "docs/alias").unwrap(),
            "allowed"
        );
    }

    #[test]
    fn workspace_reads_reject_non_files_oversize_and_invalid_utf8() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("docs")).unwrap();
        let path = directory.path().join("docs/file");
        let state = workspace_state(directory.path());
        assert!(read_workspace_file(&state, "docs").is_err());
        std::fs::write(&path, vec![b'a'; MAX_WORKSPACE_READ_BYTES as usize]).unwrap();
        assert_eq!(
            read_workspace_file(&state, "docs/file").unwrap().len(),
            MAX_WORKSPACE_READ_BYTES as usize
        );
        std::fs::write(&path, vec![b'a'; MAX_WORKSPACE_READ_BYTES as usize + 1]).unwrap();
        assert!(read_workspace_file(&state, "docs/file")
            .unwrap_err()
            .contains("exceeds cap"));
        std::fs::write(&path, [0xff]).unwrap();
        assert!(read_workspace_file(&state, "docs/file").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn workspace_fifo_is_rejected_without_opening() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("docs")).unwrap();
        let fifo = directory.path().join("docs/pipe");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        assert!(read_workspace_file(&workspace_state(directory.path()), "docs/pipe").is_err());
    }

    #[test]
    fn injected_path_must_pass_allowlist_before_egress() {
        let resolver = InMemorySecretResolver::new(vec![("key".into(), "v2".into())]);
        let kernel = WasmCapabilityKernel::new(
            "final-url-test".into(),
            WasmCapabilities {
                http: Some(HttpCapability {
                    allowlist: vec![EndpointPattern {
                        host: "api.example.com".into(),
                        path_prefix: "/v1/".into(),
                        methods: vec!["GET".into()],
                    }],
                    credentials: vec![CredentialBinding {
                        secret_name: "key".into(),
                        inject: CredentialInject::UrlPath {
                            placeholder: "v1".into(),
                        },
                        host_patterns: vec!["api.example.com".into()],
                    }],
                    rate_limit: None,
                    timeout_secs: 1,
                    max_request_bytes: 1024,
                    max_response_bytes: 2048,
                }),
                secrets: Some(SecretsCapability {
                    allowed_patterns: vec!["key".into()],
                }),
                ..Default::default()
            },
            WasmResourceLimits::default(),
        )
        .with_secret_resolver(Arc::new(resolver));
        assert!(kernel
            .check_http_request("GET", "https://api.example.com/v1/data")
            .is_ok());
        let error = try_http_fetch(
            &kernel,
            r#"{"method":"GET","url":"https://api.example.com/v1/data"}"#,
        )
        .unwrap_err();
        assert!(error.starts_with("Not allowed:"), "{error}");
        assert!(
            !error.contains("v2"),
            "injected secret must not reach errors"
        );
    }

    /// The allowlist validated the URL; a guest-supplied header must not be
    /// able to name a different host or path than the one it validated.
    #[test]
    fn host_routing_headers_are_stripped() {
        let mut headers = vec![
            ("Host".to_string(), "evil.example".to_string()),
            ("x-forwarded-host".to_string(), "evil.example".to_string()),
            ("X-Original-URL".to_string(), "/admin".to_string()),
            ("Authorization".to_string(), "Bearer token".to_string()),
        ];
        strip_host_routing_headers(&mut headers);
        assert_eq!(
            headers,
            vec![("Authorization".to_string(), "Bearer token".to_string())],
            "host-routing headers must not reach egress"
        );
    }
}
