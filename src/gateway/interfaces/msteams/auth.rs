//! Microsoft Teams Authentication
//!
//! Two components:
//! - `TokenCache`: outbound `OAuth2` token cache for Bot Framework REST API calls
//! - `JwtValidator`: inbound JWT validation for Bot Framework webhook requests

use crate::sync_primitives::{Arc, RwLock};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::gateway::channel::ChannelError;

// ── Outbound: TokenCache ──────────────────────────────────────────────────────

/// A cached access token with its expiry metadata.
struct CachedToken {
    token: String,
    /// Absolute time after which the token should be refreshed (80% of lifetime)
    refresh_at: Instant,
}

impl CachedToken {
    fn needs_refresh(&self) -> bool {
        Instant::now() >= self.refresh_at
    }
}

/// Outbound `OAuth2` token cache for Bot Framework REST API calls.
///
/// Caches `client_credentials` tokens and refreshes them proactively at 80% of
/// their lifetime so callers always receive a valid token.
pub struct TokenCache {
    app_id: String,
    app_password: String,
    tenant_id: String,
    cached: tokio::sync::RwLock<Option<CachedToken>>,
    http: Client,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl TokenCache {
    /// Create a new `TokenCache`.
    #[must_use]
    pub fn new(app_id: String, app_password: String, tenant_id: String) -> Self {
        Self {
            app_id,
            app_password,
            tenant_id,
            cached: tokio::sync::RwLock::new(None),
            http: Client::new(),
        }
    }

    /// Return a valid access token, refreshing if necessary.
    pub async fn get_token(&self) -> Result<String, ChannelError> {
        // Fast path: check under read lock
        {
            let guard = self.cached.read().await;
            if let Some(ref t) = *guard {
                if !t.needs_refresh() {
                    return Ok(t.token.clone());
                }
            }
        }

        // Slow path: refresh under write lock
        let mut guard = self.cached.write().await;
        // Double-check after acquiring write lock
        if let Some(ref t) = *guard {
            if !t.needs_refresh() {
                return Ok(t.token.clone());
            }
        }

        let token = self.fetch_token().await?;
        let token_str = token.token.clone();
        *guard = Some(token);
        Ok(token_str)
    }

    async fn fetch_token(&self) -> Result<CachedToken, ChannelError> {
        let url = format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            self.tenant_id
        );

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", self.app_id.as_str()),
            ("client_secret", self.app_password.as_str()),
            ("scope", "https://api.botframework.com/.default"),
        ];

        let resp = self
            .http
            .post(&url)
            .form(&params)
            .send()
            .await
            .map_err(|e| ChannelError::AuthFailed(format!("Token request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ChannelError::AuthFailed(format!(
                "Token endpoint returned {status}: {body}"
            )));
        }

        let tr: TokenResponse = resp
            .json()
            .await
            .map_err(|e| ChannelError::Internal(format!("Failed to parse token response: {e}")))?;

        let lifetime = Duration::from_secs(tr.expires_in);
        let refresh_after = lifetime.mul_f64(0.8);
        let now = Instant::now();

        debug!(
            expires_in = tr.expires_in,
            refresh_after_secs = refresh_after.as_secs(),
            "Fetched new Bot Framework token"
        );

        Ok(CachedToken {
            token: tr.access_token,
            refresh_at: now + refresh_after,
        })
    }
}

// ── Inbound: JwtValidator ─────────────────────────────────────────────────────

/// Claims extracted from Bot Framework JWT tokens.
#[derive(Debug, Deserialize, Serialize)]
struct BotClaims {
    /// Issuer
    iss: String,
    /// Audience — should equal the bot's `app_id`
    aud: String,
    /// Expiry (Unix timestamp)
    exp: u64,
    /// Issued-at (Unix timestamp)
    #[allow(dead_code)]
    iat: u64,
}

/// `OpenID` Connect configuration returned by Microsoft's well-known endpoint.
#[derive(Deserialize)]
struct OidcConfig {
    jwks_uri: String,
}

/// JWKS response from the key endpoint.
#[derive(Deserialize)]
struct JwksResponse {
    keys: Vec<JwkKey>,
}

/// A single JSON Web Key entry.
#[derive(Deserialize)]
struct JwkKey {
    /// Key type
    kty: String,
    /// Base64url-encoded modulus (RSA)
    n: Option<String>,
    /// Base64url-encoded exponent (RSA)
    e: Option<String>,
}

/// Inbound JWT validator for Bot Framework webhook requests.
///
/// Pre-fetches JWKS public keys and stores them so that `validate()` can be
/// called synchronously from webhook handler code.
pub struct JwtValidator {
    app_id: String,
    /// Cached RSA decoding keys (sync `RwLock` — `validate()` is sync)
    keys: Arc<RwLock<Vec<DecodingKey>>>,
    /// Signal that keys should be refreshed (set when no key matches)
    refresh_needed: Arc<crate::sync_primitives::AtomicBool>,
    http: Client,
}

impl JwtValidator {
    /// Create a new `JwtValidator`.
    #[must_use]
    pub fn new(app_id: String) -> Self {
        Self {
            app_id,
            keys: Arc::new(RwLock::new(Vec::new())),
            refresh_needed: Arc::new(crate::sync_primitives::AtomicBool::new(false)),
            http: Client::new(),
        }
    }

    /// Returns true if keys need refreshing (signature mismatch detected).
    #[must_use]
    pub fn key_refresh_needed(&self) -> bool {
        self.refresh_needed
            .load(crate::sync_primitives::Ordering::SeqCst)
    }

    /// Validate an inbound Bearer JWT token. Returns `true` if valid.
    ///
    /// This is **synchronous** because it is called from the sync webhook
    /// handler. Keys must have been pre-fetched via `refresh_keys()`.
    pub fn validate(&self, token: &str) -> bool {
        let keys = match self.keys.read().unwrap_or_else(|e| e.into_inner()) {
            guard if guard.is_empty() => {
                warn!("JWT validation skipped: no JWKS keys loaded");
                return false;
            }
            guard => guard.clone(),
        };

        // Check algorithm before decoding
        match decode_header(token) {
            Ok(header) if header.alg != Algorithm::RS256 => {
                warn!(alg = ?header.alg, "JWT rejected: unexpected algorithm (expected RS256)");
                return false;
            }
            Err(e) => {
                warn!("JWT rejected: failed to decode header: {e}");
                return false;
            }
            _ => {}
        }

        let mut validation = Validation::new(Algorithm::RS256);
        // Bot Framework issuers
        validation.set_issuer(&[
            "https://api.botframework.com",
            "https://sts.windows.net/d6d49420-f39b-4df7-a1dc-d59a935871db/",
        ]);
        validation.set_audience(&[self.app_id.as_str()]);
        // Small leeway for clock drift between Aleph server and Microsoft infrastructure
        validation.leeway = 30;

        for key in keys.iter() {
            match decode::<BotClaims>(token, key, &validation) {
                Ok(_) => {
                    debug!("JWT validated successfully");
                    return true;
                }
                Err(e) => {
                    use jsonwebtoken::errors::ErrorKind;
                    match e.kind() {
                        ErrorKind::ExpiredSignature => {
                            warn!("JWT rejected: token has expired");
                            return false;
                        }
                        ErrorKind::InvalidAudience => {
                            warn!("JWT rejected: invalid audience");
                            return false;
                        }
                        ErrorKind::InvalidIssuer => {
                            warn!("JWT rejected: invalid issuer");
                            return false;
                        }
                        // Signature mismatch with this key — try next key
                        ErrorKind::InvalidSignature => continue,
                        _ => {
                            warn!("JWT rejected: {e}");
                            return false;
                        }
                    }
                }
            }
        }

        warn!("JWT rejected: no key matched the signature — signaling key refresh");
        self.refresh_needed
            .store(true, crate::sync_primitives::Ordering::SeqCst);
        false
    }

    /// Fetch JWKS public keys from Microsoft's `OpenID` metadata endpoint.
    pub async fn refresh_keys(&self) -> Result<(), ChannelError> {
        let oidc_url = "https://login.botframework.com/v1/.well-known/openidconfiguration";

        let oidc: OidcConfig = self
            .http
            .get(oidc_url)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("OIDC metadata fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| ChannelError::Internal(format!("OIDC metadata parse failed: {e}")))?;

        let jwks: JwksResponse = self
            .http
            .get(&oidc.jwks_uri)
            .send()
            .await
            .map_err(|e| ChannelError::Internal(format!("JWKS fetch failed: {e}")))?
            .json()
            .await
            .map_err(|e| ChannelError::Internal(format!("JWKS parse failed: {e}")))?;

        let mut new_keys = Vec::new();
        for key in &jwks.keys {
            if key.kty != "RSA" {
                continue;
            }
            if let (Some(n), Some(e)) = (&key.n, &key.e) {
                match DecodingKey::from_rsa_components(n, e) {
                    Ok(dk) => new_keys.push(dk),
                    Err(err) => warn!("Skipping malformed JWK: {err}"),
                }
            }
        }

        if new_keys.is_empty() {
            return Err(ChannelError::Internal(
                "JWKS response contained no usable RSA keys".into(),
            ));
        }

        let mut guard = self.keys.write().unwrap_or_else(|e| e.into_inner());
        *guard = new_keys;

        debug!(
            "Loaded {} JWKS keys for Bot Framework JWT validation",
            guard.len()
        );
        self.refresh_needed
            .store(false, crate::sync_primitives::Ordering::SeqCst);
        Ok(())
    }

    /// Directly load decoding keys (used in tests).
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn load_keys_for_test(&self, keys: Vec<DecodingKey>) {
        let mut guard = self.keys.write().unwrap_or_else(|e| e.into_inner());
        *guard = keys;
    }
}

// ── Service URL validation ────────────────────────────────────────────────────

/// Validate that a Bot Framework `serviceUrl` points to an allowed domain.
///
/// Allowed:
/// - Any subdomain of `botframework.com`
/// - `smba.trafficmanager.net`
#[must_use]
pub fn validate_service_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" || parsed.username() != "" || parsed.password().is_some() {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };

    host == "botframework.com"
        || host.ends_with(".botframework.com")
        || host == "smba.trafficmanager.net"
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    use jsonwebtoken::{encode, EncodingKey, Header};

    // RSA 2048 key pair generated with `openssl genrsa 2048` for testing only.
    // DO NOT use these keys for anything real.
    const TEST_RSA_PRIVATE_PEM: &str = "-----BEGIN RSA PRIVATE KEY-----
MIIEogIBAAKCAQEAqndju9yaCKdXY4jNJT2xjFJoJWluzyqfWyWzlsKpZT9IWArt
f9M6Dzg8s8B2Ew++UXtFHJR1MYWVxQSsGJu+TvVYuHfIcagZdhHoN1rvUd6hA9zu
p+BOtiiBVetAbOrsKf4K3LmHPXCNcLSsQZZMepxGTcnul9Q27LQZiCf/vsJhd1+g
d0c4NIZqTsQixGeLYdBQYXWHHmXX9aKpcRJ2HU20B2ddINCK+0YKKrZ/Y0f8eSfx
edn/0fwpgIDEflB/mJzVGiYUa6eGa5I0mbH1metI4RuvzmtSW5Vv8w63+rsVRvhn
I6LLpfyVap6X7HTrujAK57B6H3wSrDwjRVK5DQIDAQABAoIBAEr6Jz5WadO7ktbf
MWgbag/voQovhQMO2reS7ho19Z7oRjAfYlXyOJvAHwbq9KQurQWWxA+thpxpBrZ4
9x79xadiB4tfeCTxjH1fo/VRBGlMlCKoVro1ETnSuAxr5FLjw0s3B10NJ1xROIzl
ksxoSKw3Nz74vf4+44eaMY6vFTA4A7xdnaWPg1Gw6NofaLOikOYd5Mlg/WMU54bw
ID2YKcQvVDKEboRIPlHsBHeyF0lOJJ3hYTkxk6n9U37pZH3LZ5FUagBORKX01pTb
ZSVSli3GBxEmVDVxKl78HVsKalGkCrMhYX3r/75Zj5o5EbB6EobyZHk4HFX6gImA
FKxhEysCgYEA4G9ihsT+tZ86cUK7od7zDT1Y+ha+g1bTeB4voXIzb2bAg1Jf6/xI
YoaXlz3nwY/bz2eUR8oFxgHjUSdKr/ct9QgUNHq34RSpthmK8DdWZkUo+f54RpTC
UH7CqWXMJR+YflgwJ276+K2Lbx2w8H21ASd1kNIu7AIEDgJ25ay0QUMCgYEAwnDm
y6qJnjJp1JlXYcNzoGUapq9WwoVrOvSmF1KBMeqLba0/hnyBR11exPPmCDtl1e/M
QHa68DlfcMV7F093E5egF7TcyHWIjqcC2UpZpWElbkGGLQuFGlrpbZWx0rSkPHgW
K31ic/QneRmoGmI+LGGihD9SVrN/VynWl7QSj28CgYAIJaLy93WzjBsn/18mShyS
j3aKZYb255D3nEjoWGfrlFRKsBPRUjAie3ZHRDUEfr9g8Qad8IRzIqBo0r9QUe22
JlvtZ8MDBaf/dz/m5mtZfQs2v/kHvuCq4V8ZnRtjAZmchIEC/XFY05vrJa3FnRqT
9yW6YxbW9F/HTmmYfsNwVwKBgBhcct4jkLhsUowbZjJOfacj47Hsl+8pLiUlz8Vu
RdeOLkfgg+wCn2Pkk+ITOMfhQUILmEifV46PcaC8bU6fWyjuP1WZCGxpJWHSFO5K
fW7V/A2TUg9EuTlzGHntXmkqzsTwur5aKEKk3WkzyLb9hhKjbOwqztMkDBlMmaFK
I2UrAoGAEEvDa4UDSL/pYjA9SHaGz18jBJNOaWmBH3duag7JAh8auDhHHsueGWT4
OUVUNqVEFvxlqApkg4fwXPWnWHXTYClYOoODfAOq0bAO6FazKsj8rbozFgpj8akw
50k7Py7h3DNPRIc2jCKM8Ulc+Mwz9WOh0BwXw6eUHVHU5KE6mTk=
-----END RSA PRIVATE KEY-----";

    const TEST_RSA_PUBLIC_PEM: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAqndju9yaCKdXY4jNJT2x
jFJoJWluzyqfWyWzlsKpZT9IWArtf9M6Dzg8s8B2Ew++UXtFHJR1MYWVxQSsGJu+
TvVYuHfIcagZdhHoN1rvUd6hA9zup+BOtiiBVetAbOrsKf4K3LmHPXCNcLSsQZZM
epxGTcnul9Q27LQZiCf/vsJhd1+gd0c4NIZqTsQixGeLYdBQYXWHHmXX9aKpcRJ2
HU20B2ddINCK+0YKKrZ/Y0f8eSfxedn/0fwpgIDEflB/mJzVGiYUa6eGa5I0mbH1
metI4RuvzmtSW5Vv8w63+rsVRvhnI6LLpfyVap6X7HTrujAK57B6H3wSrDwjRVK5
DQIDAQAB
-----END PUBLIC KEY-----";

    fn make_claims(aud: &str, iss: &str, exp_offset_secs: i64) -> BotClaims {
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let exp = (now + exp_offset_secs) as u64;
        BotClaims {
            iss: iss.to_string(),
            aud: aud.to_string(),
            exp,
            iat: now as u64,
        }
    }

    fn sign_token(claims: &BotClaims, pem: &str) -> String {
        let key = EncodingKey::from_rsa_pem(pem.as_bytes()).expect("test key must be valid PEM");
        encode(&Header::new(Algorithm::RS256), claims, &key).expect("encoding must succeed")
    }

    fn make_validator_with_test_key(app_id: &str) -> JwtValidator {
        let validator = JwtValidator::new(app_id.to_string());
        let dk = DecodingKey::from_rsa_pem(TEST_RSA_PUBLIC_PEM.as_bytes())
            .expect("test public key must be valid PEM");
        validator.load_keys_for_test(vec![dk]);
        validator
    }

    // ── validate_service_url ──────────────────────────────────────────────────

    #[test]
    fn test_validate_service_url_allows_botframework() {
        assert!(validate_service_url(
            "https://smba.trafficmanager.net/apis/"
        ));
        assert!(validate_service_url(
            "https://directline.botframework.com/v3/directline"
        ));
        assert!(validate_service_url("https://api.botframework.com"));
        assert!(validate_service_url("https://botframework.com/"));
    }

    #[test]
    fn test_validate_service_url_rejects_unknown() {
        assert!(!validate_service_url("https://evil.com/botframework.com"));
        assert!(!validate_service_url("https://botframework.com.evil.io/"));
        assert!(!validate_service_url("https://notbotframework.com/"));
        assert!(!validate_service_url("ftp://smba.trafficmanager.net/"));
        assert!(!validate_service_url("not-a-url"));
        assert!(!validate_service_url(""));
    }

    // ── token_cache_new ───────────────────────────────────────────────────────

    #[test]
    fn test_token_cache_new() {
        let cache = TokenCache::new(
            "app-id".to_string(),
            "app-password".to_string(),
            "common".to_string(),
        );
        assert_eq!(cache.app_id, "app-id");
        assert_eq!(cache.tenant_id, "common");
    }

    // ── jwt_validation ────────────────────────────────────────────────────────

    #[test]
    fn test_jwt_validation_accepts_valid_token() {
        let app_id = "my-bot-app-id";
        let validator = make_validator_with_test_key(app_id);

        let claims = make_claims(app_id, "https://api.botframework.com", 3600);
        let token = sign_token(&claims, TEST_RSA_PRIVATE_PEM);

        assert!(
            validator.validate(&token),
            "Valid token should pass validation"
        );
    }

    #[test]
    fn test_jwt_validation_rejects_expired() {
        let app_id = "my-bot-app-id";
        let validator = make_validator_with_test_key(app_id);

        // exp = 60 seconds in the past (well beyond the 30-second leeway)
        let claims = make_claims(app_id, "https://api.botframework.com", -60);
        let token = sign_token(&claims, TEST_RSA_PRIVATE_PEM);

        assert!(
            !validator.validate(&token),
            "Expired token should be rejected"
        );
    }

    #[test]
    fn test_jwt_validation_rejects_wrong_audience() {
        let app_id = "my-bot-app-id";
        let validator = make_validator_with_test_key(app_id);

        // Audience is a different app
        let claims = make_claims("wrong-app-id", "https://api.botframework.com", 3600);
        let token = sign_token(&claims, TEST_RSA_PRIVATE_PEM);

        assert!(
            !validator.validate(&token),
            "Wrong audience token should be rejected"
        );
    }

    #[test]
    fn test_jwt_validation_rejects_wrong_issuer() {
        let app_id = "my-bot-app-id";
        let validator = make_validator_with_test_key(app_id);

        let claims = make_claims(app_id, "https://attacker.example.com", 3600);
        let token = sign_token(&claims, TEST_RSA_PRIVATE_PEM);

        assert!(
            !validator.validate(&token),
            "Wrong issuer should be rejected"
        );
    }

    #[test]
    fn test_jwt_validation_rejects_no_keys() {
        let validator = JwtValidator::new("my-bot-app-id".to_string());
        // No keys loaded — should reject
        let claims = make_claims("my-bot-app-id", "https://api.botframework.com", 3600);
        let token = sign_token(&claims, TEST_RSA_PRIVATE_PEM);
        assert!(
            !validator.validate(&token),
            "Should reject when no keys are loaded"
        );
    }

    #[tokio::test]
    async fn test_certificate_load_valid_pem() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cert_path = temp_dir.path().join("cert.pem");
        let pem_content = "-----BEGIN CERTIFICATE-----\nMIICpDCCAYwCCQDU+pQ4P0aW5DANBgkqhkiG9w0BAQsFADAUMRIwEAYDVQQDDAls\nb2NhbGhvc3QwHhcNMjQwMTAxMDAwMDAwWhcNMjUwMTAxMDAwMDAwWjAUMRIwEAYDVQQD\nDAlsb2NhbGhvc3QwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQC7o5gELJvF\n-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC7o5gELJvF\n-----END PRIVATE KEY-----\n";
        tokio::fs::write(&cert_path, pem_content).await.unwrap();
        let cert = Certificate::load(cert_path).await;
        assert!(cert.is_ok(), "Certificate load failed: {:?}", cert.err());
    }

    #[tokio::test]
    async fn test_certificate_load_missing_file() {
        let result = Certificate::load(PathBuf::from("/nonexistent/cert.pem")).await;
        assert!(result.is_err());
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederatedCredential {
    pub certificate_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed_identity_client_id: Option<String>,
    pub authority_url: String,
}

impl FederatedCredential {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.certificate_path.exists() {
            return Err("Certificate file does not exist");
        }
        if !self.authority_url.contains("{tenant}") {
            return Err("authority_url must contain {tenant} placeholder");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum AuthFlow {
    ClientSecret(String),
    Federated(FederatedCredential),
}

impl AuthFlow {
    #[must_use]
    pub const fn is_federated(&self) -> bool {
        matches!(self, Self::Federated(_))
    }
}

#[derive(Debug, Clone)]
pub struct Certificate {
    pub der: Vec<u8>,
    pub private_key: Vec<u8>,
}

impl Certificate {
    pub async fn load(path: std::path::PathBuf) -> Result<Self, CertificateError> {
        let content = tokio::fs::read(&path)
            .await
            .map_err(|e| CertificateError::IoError(e.to_string()))?;
        Self::load_from_pem(&content)
    }

    fn load_from_pem(content: &[u8]) -> Result<Self, CertificateError> {
        let pem_str = String::from_utf8_lossy(content);
        let mut der = None;
        let mut private_key = None;

        let cert_re = regex::Regex::new(
            r"-----BEGIN CERTIFICATE-----\s*([A-Za-z0-9+/=\s]+)\s*-----END CERTIFICATE-----",
        )
        .map_err(|e| CertificateError::ParseError(format!("invalid certificate regex: {e}")))?;
        let key_re = regex::Regex::new(r"-----BEGIN (?:RSA )?PRIVATE KEY-----\s*([A-Za-z0-9+/=\s]+)\s*-----END (?:RSA )?PRIVATE KEY-----")
            .map_err(|e| CertificateError::ParseError(format!("invalid private key regex: {e}")))?;

        if let Some(caps) = cert_re.captures(&pem_str) {
            let base64_content = caps
                .get(1)
                .ok_or_else(|| CertificateError::ParseError("certificate capture missing".into()))?
                .as_str();
            let cleaned: String = base64_content
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            use base64::Engine;
            der = Some(
                base64::engine::general_purpose::STANDARD
                    .decode(&cleaned)
                    .map_err(|e| {
                        CertificateError::ParseError(format!("certificate base64 error: {e}"))
                    })?,
            );
        }

        if let Some(caps) = key_re.captures(&pem_str) {
            let base64_content = caps
                .get(1)
                .ok_or_else(|| CertificateError::ParseError("private key capture missing".into()))?
                .as_str();
            let cleaned: String = base64_content
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect();
            use base64::Engine;
            private_key = Some(
                base64::engine::general_purpose::STANDARD
                    .decode(&cleaned)
                    .map_err(|e| CertificateError::ParseError(format!("key base64 error: {e}")))?,
            );
        }

        let der = der.ok_or_else(|| CertificateError::ParseError("No certificate found".into()))?;
        let private_key = private_key
            .ok_or_else(|| CertificateError::ParseError("No private key found".into()))?;

        Ok(Self { der, private_key })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CertificateError {
    #[error("I/O error: {0}")]
    IoError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
}

pub struct ManagedIdentityTokenProvider {
    client: reqwest::Client,
    client_id: Option<String>,
}

impl ManagedIdentityTokenProvider {
    #[must_use]
    pub fn new(client_id: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            client_id,
        }
    }

    pub async fn get_token(&self, resource: &str) -> Result<String, MsTeamsAuthError> {
        let endpoint = "http://169.254.169.254/metadata/identity/oauth2/token";
        let mut url = format!("{endpoint}?api-version=2018-02-01&resource={resource}");
        if let Some(cid) = &self.client_id {
            url = format!("{url}&client_id={cid}");
        }

        let response = self
            .client
            .get(&url)
            .header("Metadata", "true")
            .send()
            .await
            .map_err(|e| MsTeamsAuthError::NetworkError(e.to_string()))?;

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
        }

        let token_resp: TokenResponse = response
            .json()
            .await
            .map_err(|e| MsTeamsAuthError::ParseError(e.to_string()))?;

        Ok(token_resp.access_token)
    }
}

#[derive(Clone)]
pub struct JwtAssertionGenerator {
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
    app_id: String,
    authority_url: String,
    client: reqwest::Client,
}

impl JwtAssertionGenerator {
    #[must_use]
    pub fn new(certificate: Certificate, app_id: String, authority_url: String) -> Self {
        Self {
            certificate_der: certificate.der,
            private_key_der: certificate.private_key,
            app_id,
            authority_url,
            client: reqwest::Client::new(),
        }
    }

    pub async fn exchange_for_token(
        &self,
        tenant_id: &str,
        scope: &str,
    ) -> Result<String, MsTeamsAuthError> {
        let assertion = self.generate_assertion()?;
        let endpoint = format!("{}/{}/oauth2/v2.0/token", self.authority_url, tenant_id);

        let params = [
            ("grant_type", "client_credentials"),
            ("client_id", &self.app_id),
            (
                "client_assertion_type",
                "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
            ),
            ("client_assertion", &assertion),
            ("scope", scope),
        ];

        let resp = self
            .client
            .post(&endpoint)
            .form(&params)
            .send()
            .await
            .map_err(|e| MsTeamsAuthError::NetworkError(e.to_string()))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(MsTeamsAuthError::TokenExchangeError(format!(
                "Token endpoint returned {status}: {body}"
            )));
        }

        #[derive(serde::Deserialize)]
        struct TokenResponse {
            access_token: String,
        }

        let token_resp: TokenResponse = resp
            .json()
            .await
            .map_err(|e| MsTeamsAuthError::ParseError(e.to_string()))?;

        Ok(token_resp.access_token)
    }

    fn generate_assertion(&self) -> Result<String, MsTeamsAuthError> {
        use base64::Engine;
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        use std::time::{SystemTime, UNIX_EPOCH};

        let x5c = base64::engine::general_purpose::STANDARD.encode(&self.certificate_der);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| MsTeamsAuthError::TokenExchangeError(format!("system clock error: {e}")))?
            .as_secs() as i64;

        #[derive(serde::Serialize)]
        struct MsClaims<'a> {
            iss: &'a str,
            sub: &'a str,
            aud: String,
            exp: i64,
            iat: i64,
            #[serde(rename = "x5c")]
            x5c: Vec<String>,
            #[serde(rename = "x5t")]
            x5t: String,
        }

        let x5t = base64::engine::general_purpose::STANDARD.encode(&self.certificate_der);

        let claims = MsClaims {
            iss: &self.app_id,
            sub: &self.app_id,
            aud: "49aba4-55c4-4c17-9d36-846e36a6f66a".to_string(),
            exp: now + 300,
            iat: now,
            x5c: vec![x5c],
            x5t,
        };

        let encoding_key = EncodingKey::from_rsa_der(&self.private_key_der);

        encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).map_err(|e| {
            MsTeamsAuthError::TokenExchangeError(format!("Failed to sign JWT assertion: {e}"))
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MsTeamsAuthError {
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Token exchange failed: {0}")]
    TokenExchangeError(String),
    #[error("Certificate error: {0}")]
    CertificateError(String),
}
