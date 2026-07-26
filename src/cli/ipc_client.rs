//! HTTP client that forwards a CLI request to the running server's
//! `/v1/admin/*` namespace. Reads bearer token from `security.db` in
//! read-only WAL mode; auto-retries once on 401 to handle token rotation
//! that races with the request.

use std::path::Path;

use anyhow::Context;
use reqwest::StatusCode;

use crate::cli::endpoint::read_endpoint;
use crate::cli::policy::HttpMethod;
use crate::gateway::security::read_current_token_readonly;
use crate::utils::sqlite_open::open_sqlite_readonly;

const SECURITY_DB_FILENAME: &str = "security.db";

pub fn forward_to_server<T>(
    data_dir: &Path,
    method: HttpMethod,
    route: &str,
    body: serde_json::Value,
) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let endpoint = read_endpoint(data_dir)?.with_context(|| {
        format!(
            "server is initializing or crashed (no .ipc-endpoint.json at {}). \
             Try again or run `aleph stop` first.",
            data_dir.display()
        )
    })?;
    let url = format!(
        "{}/{}",
        endpoint.url.trim_end_matches('/'),
        route.trim_start_matches('/')
    );

    let token = read_token(data_dir)?;
    let resp = call_once(&url, method, &body, &token)?;

    if resp.status() == StatusCode::UNAUTHORIZED {
        // Token may have rotated between our read and our send.
        let fresh = read_token(data_dir)?;
        if fresh != token {
            let resp2 = call_once(&url, method, &body, &fresh)?;
            return finalize::<T>(resp2);
        }
        let text = resp
            .text()
            .unwrap_or_else(|e| format!("(failed to read response body: {e})"));
        anyhow::bail!("authentication failed — bearer token rejected by server: {text}");
    }
    finalize::<T>(resp)
}

fn read_token(data_dir: &Path) -> anyhow::Result<String> {
    let conn = open_sqlite_readonly(&data_dir.join(SECURITY_DB_FILENAME))
        .context("cannot open security.db read-only — is data_dir set up?")?;
    let token = read_current_token_readonly(&conn)?.ok_or_else(|| {
        anyhow::anyhow!("no bearer token in security.db — has the server ever been started?")
    })?;
    Ok(token)
}

fn call_once(
    url: &str,
    method: HttpMethod,
    body: &serde_json::Value,
    token: &str,
) -> anyhow::Result<reqwest::blocking::Response> {
    // Built per call rather than cached: the CLI is a one-shot process that
    // sends at most two requests (initial + one 401 retry), so connection
    // reuse is irrelevant, and propagating a build failure beats panicking
    // on a user-facing path.
    let client = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("failed to build HTTP client for admin IPC")?;
    let req = client
        .request(method.as_reqwest(), url)
        .bearer_auth(token)
        .json(body);
    Ok(req.send()?)
}

fn finalize<T>(resp: reqwest::blocking::Response) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = resp.status();
    if status.is_success() {
        let body = resp.json::<T>()?;
        Ok(body)
    } else {
        let text = resp
            .text()
            .unwrap_or_else(|e| format!("(failed to read response body: {e})"));
        anyhow::bail!("server returned {status}: {text}")
    }
}

#[cfg(test)]
mod tests {
    // Integration tests for forward_to_server live in tests/spec_c_cli_ipc.rs
    // because they need a real HTTP listener and a seeded security.db. Unit
    // tests here cover only the helpers that don't need a network.
}
