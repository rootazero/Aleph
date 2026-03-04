//! ChatGPT security layer
//!
//! Handles CSRF tokens, chat-requirements, and proof-of-work challenges
//! required by the ChatGPT backend API.

use crate::error::{AlephError, Result};
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use super::types::{ChatRequirements, ProofOfWork};

const CSRF_URL: &str = "https://chatgpt.com/api/auth/csrf";
const REQUIREMENTS_URL: &str = "https://chatgpt.com/backend-api/sentinel/chat-requirements";

/// ChatGPT security token manager
pub struct ChatGptSecurity;

impl ChatGptSecurity {
    /// Fetch CSRF token from the auth endpoint
    pub async fn fetch_csrf(client: &Client) -> Result<String> {
        let response = client
            .get(CSRF_URL)
            .send()
            .await
            .map_err(|e| AlephError::network(format!("Failed to fetch CSRF token: {}", e)))?;

        if !response.status().is_success() {
            return Err(AlephError::provider(format!(
                "CSRF fetch failed with status: {}",
                response.status()
            )));
        }

        let json: Value = response
            .json()
            .await
            .map_err(|e| AlephError::provider(format!("Failed to parse CSRF response: {}", e)))?;

        json["csrfToken"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| AlephError::provider("CSRF token not found in response"))
    }

    /// Fetch chat-requirements (security tokens + proof-of-work params)
    pub async fn fetch_requirements(
        client: &Client,
        access_token: &str,
    ) -> Result<ChatRequirements> {
        let response = client
            .post(REQUIREMENTS_URL)
            .header("Authorization", format!("Bearer {}", access_token))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| {
                AlephError::network(format!("Failed to fetch chat requirements: {}", e))
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AlephError::provider(format!(
                "Chat requirements failed ({}): {}",
                status, body
            )));
        }

        let requirements: ChatRequirements = response.json().await.map_err(|e| {
            AlephError::provider(format!("Failed to parse chat requirements: {}", e))
        })?;

        debug!(
            has_pow = requirements.proofofwork.is_some(),
            "Fetched chat requirements"
        );

        Ok(requirements)
    }

    /// Solve proof-of-work challenge
    ///
    /// Finds a nonce such that SHA-256(seed + nonce) starts with the required
    /// difficulty prefix (hex zeros).
    pub fn solve_proof_of_work(seed: &str, difficulty: &str) -> Result<String> {
        if difficulty.is_empty() {
            return Ok(String::new());
        }

        let max_iterations: u64 = 10_000_000;

        for nonce in 0..max_iterations {
            let input = format!("{}{}", seed, nonce);
            let hash = Sha256::digest(input.as_bytes());
            let hex = format!("{:x}", hash);

            if hex.starts_with(difficulty) {
                debug!(nonce, "Proof-of-work solved");
                return Ok(format!("gAAAAAB{}", nonce));
            }
        }

        warn!(seed, difficulty, "Proof-of-work exhausted max iterations");
        Err(AlephError::provider(
            "Failed to solve proof-of-work within iteration limit",
        ))
    }

    /// Build all security headers needed for a conversation request.
    /// Returns (requirements_token, pow_token_option).
    pub async fn prepare_security_tokens(
        client: &Client,
        access_token: &str,
    ) -> Result<(String, Option<String>)> {
        let requirements = Self::fetch_requirements(client, access_token).await?;

        let pow_token = if let Some(ProofOfWork {
            required: true,
            seed: Some(ref seed),
            difficulty: Some(ref diff),
        }) = requirements.proofofwork
        {
            Some(Self::solve_proof_of_work(seed, diff)?)
        } else {
            None
        };

        Ok((requirements.token, pow_token))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solve_proof_of_work_finds_valid_hash() {
        let result = ChatGptSecurity::solve_proof_of_work("test_seed_123", "0000");
        assert!(result.is_ok());
        let answer = result.unwrap();
        assert!(!answer.is_empty());
    }

    #[test]
    fn test_solve_proof_of_work_empty_difficulty_returns_empty() {
        let result = ChatGptSecurity::solve_proof_of_work("seed", "");
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }
}
