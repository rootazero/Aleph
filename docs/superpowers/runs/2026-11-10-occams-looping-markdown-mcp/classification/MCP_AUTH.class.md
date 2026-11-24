# Classification & Routing Matrix: Batch MCP_AUTH

## Executive Summary

Batch `src/mcp/auth` implements OAuth 2.0 authentication with PKCE, dynamic client registration (RFC 7591), authorization code callback server with strict loopback validation, token persistence with secure file permissions (0600 on Unix), out-of-process cache invalidation via file modification time checking, and RFC 9207 response issuer mix-up mitigation.

Following Occam's Razor principles and strict safety preservation for OAuth callbacks, token storage encryption/redaction semantics, and auth provider interface contracts, the batch is exceptionally clean, robust, and well-designed. No urgent refactoring or dead code removal is required. All helper abstractions (`url_decode`, `is_valid_state`, `validate_response_issuer`, `validate_metadata_origins`) are actively utilized and rigorously tested.

## Classification & Routing Matrix

| Location | Taxonomy | Smell Description | Route | Action |
|---|---|---|---|---|
| `src/mcp/auth/callback.rs` | Robustness / Parsing | Hand-rolled percent decoding and header parsing. | `DEFER` | Preserve as-is. Clean, correct, and self-contained without introducing heavy HTTP framework dependencies (satisfies R3 Core Lightweight). |
| `src/mcp/auth/storage.rs` | Concurrency / Cache | Multi-process mtime checking and `load_for_write` merging. | `FIX-CAREFULLY` (Evaluated & Preserved) | Already optimized and correct; ensures no lost updates across concurrent storage handles. |
| `src/mcp/auth/provider.rs` | SSRF & Origin Validation | `ensure_ssrf_safe` and `validate_metadata_origins` checks. | `FIX-CAREFULLY` (Evaluated & Preserved) | Essential security guards; zero overhead, high fidelity. |

## Invariant Safety Pre-Flight

1. **OAuth callback matching, state validation, and error redirection**: Preserved. `is_valid_state` strictly checks URL-safe base64 characters before constant-time comparison. RFC 9207 `iss` checks prevent mix-up attacks.
2. **Token storage security and encryption/redaction semantics**: Preserved. Manual `Debug` implementations redact `access_token`, `refresh_token`, `client_secret`, `code_verifier`, and `oauth_state`. Unix file permissions are locked to `0600`.
3. **Auth provider interface contracts**: Preserved. Public API surface for `OAuthProvider`, `CallbackServer`, and `OAuthStorage` remains completely untouched.
