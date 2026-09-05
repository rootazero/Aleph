//! Error classification for the search provider chain.
//!
//! # Why a kind, when `AlephError` already has variants
//!
//! The chain's consumers — the structured log line, the aggregate failure
//! report, the health memory — all ask the same question of an error: *which
//! lever fixes this?* A wrong API key and a rate-limited plan both arrive as
//! `Err`, but the first is fixed in the vault and the second by waiting, and
//! a report that cannot tell them apart sends the reader to the wrong one.
//! `SearchErrorKind` is that question, answered once.
//!
//! The mapping is deliberately derived, not carried: `base::send`'s funnel
//! already picks the `AlephError` variant at the one point where the HTTP
//! status is known, and every provider's error path goes through it (the
//! `error_funnel_census` guard keeps it that way). Carrying a kind field on
//! the error would let a production site and a classifier disagree; deriving
//! the kind from the variant makes that impossible.
//!
//! # Relationship to pi-web-access
//!
//! The model for this is pi-web-access's `SearchProviderError.kind`, trimmed
//! to what Aleph's providers actually produce: its `credential`-vs-`config`
//! split is one case here (a missing key never reaches the chain — the
//! backend reports `is_available() == false` and is skipped without a
//! request), and its same-provider retry semantics have no counterpart in a
//! chain that asks each backend exactly once.
//!
//! `pub(crate)` today: the only consumers live under `src/search/`. The tool
//! face (B2) can widen this when it starts rendering kinds itself.

use crate::error::AlephError;

/// What kind of failure a search backend produced.
///
/// The variants are ordered by the reader's question, not by HTTP: each one
/// names a different lever, which is the only reason to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchErrorKind {
    /// Network failure, timeout, or a 5xx from the backend. May heal on its
    /// own; retrying later is a reasonable move.
    Transient,
    /// 429 — quota or rate limit exhausted. Heals on the vendor's window,
    /// not on ours; retrying immediately just spends the chain.
    Quota,
    /// 401/403 — the credential was rejected. Does not heal; somebody has to
    /// fix the key.
    Auth,
    /// Missing key, malformed `base_url`, SSRF-blocked host. The backend was
    /// never reachable as configured; does not heal.
    Config,
    /// A success status with a body that did not parse — the backend changed
    /// its contract, or answered a challenge page where data was expected.
    InvalidResponse,
    /// A 4xx other than auth/quota — the backend refused the request as
    /// shaped. Sending the identical request again fails identically.
    RequestRejected,
    /// The caller went away mid-flight. Says nothing about the backend;
    /// recorded nowhere.
    Cancelled,
    /// Anything without a more specific home. Reachable (a provider can
    /// return any `AlephError`), but a kind a reader can act on beats it.
    Other,
}

impl SearchErrorKind {
    /// Classify an error a search provider returned.
    ///
    /// This is the only `AlephError` → kind mapping in the crate: the chain,
    /// the fan-out, the SERP fallback and the health memory all derive from
    /// it, so two consumers cannot drift into disagreeing about one failure.
    /// The funnel (`base::send`) is what keeps the mapping honest — the
    /// status code is only known there, and it chooses the variant this
    /// match reads.
    pub(crate) const fn of(error: &AlephError) -> Self {
        match error {
            AlephError::NetworkError { .. } | AlephError::Timeout { .. } => Self::Transient,
            AlephError::RateLimitError { .. } => Self::Quota,
            AlephError::AuthenticationError { .. } => Self::Auth,
            AlephError::InvalidConfig { .. } | AlephError::Validation(_) => Self::Config,
            AlephError::InvalidResponse { .. } => Self::InvalidResponse,
            AlephError::RequestRejected { .. } => Self::RequestRejected,
            AlephError::Cancelled => Self::Cancelled,
            // `ProviderError` is the funnel's 5xx bucket (and a few
            // provider-specific "the backend said something is wrong"
            // envelopes), all of which are transient from the chain's point
            // of view: the next backend may answer, and this one may heal.
            AlephError::ProviderError { .. } => Self::Transient,
            _ => Self::Other,
        }
    }

    /// The stable label for structured logs and the `name [kind] message`
    /// report — the token an operator greps `target=search` for.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Transient => "transient",
            Self::Quota => "quota",
            Self::Auth => "auth",
            Self::Config => "config",
            Self::InvalidResponse => "invalid-response",
            Self::RequestRejected => "request-rejected",
            Self::Cancelled => "cancelled",
            Self::Other => "other",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mapping is the module's contract: every variant a search provider
    /// can realistically produce lands on the kind whose lever actually fits.
    #[test]
    fn aleph_error_variants_map_to_the_kind_whose_lever_fits() {
        let cases: &[(AlephError, SearchErrorKind)] = &[
            (AlephError::network("dns failure"), SearchErrorKind::Transient),
            (
                AlephError::Timeout { suggestion: None },
                SearchErrorKind::Transient,
            ),
            (
                AlephError::provider("upstream 5xx"),
                SearchErrorKind::Transient,
            ),
            (AlephError::rate_limit("429"), SearchErrorKind::Quota),
            (
                AlephError::authentication("brave", "bad token"),
                SearchErrorKind::Auth,
            ),
            (
                AlephError::invalid_config("missing key"),
                SearchErrorKind::Config,
            ),
            (
                AlephError::Validation("bad input".into()),
                SearchErrorKind::Config,
            ),
            (
                AlephError::invalid_response("garbage body"),
                SearchErrorKind::InvalidResponse,
            ),
            (
                AlephError::request_rejected("400 bad param"),
                SearchErrorKind::RequestRejected,
            ),
            (AlephError::Cancelled, SearchErrorKind::Cancelled),
            (
                AlephError::other("something odd"),
                SearchErrorKind::Other,
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(
                SearchErrorKind::of(err),
                *expected,
                "wrong kind for: {err}",
            );
        }
    }

    /// Labels are a wire contract: an operator's saved grep and a log line
    /// written a release apart have to meet. Renaming one is a breaking
    /// change to that contract, so the spellings are pinned here.
    #[test]
    fn kind_labels_are_stable() {
        let cases: &[(SearchErrorKind, &str)] = &[
            (SearchErrorKind::Transient, "transient"),
            (SearchErrorKind::Quota, "quota"),
            (SearchErrorKind::Auth, "auth"),
            (SearchErrorKind::Config, "config"),
            (SearchErrorKind::InvalidResponse, "invalid-response"),
            (SearchErrorKind::RequestRejected, "request-rejected"),
            (SearchErrorKind::Cancelled, "cancelled"),
            (SearchErrorKind::Other, "other"),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.as_str(), *expected);
        }
    }

    /// No two kinds share a label — the label is the kind's identity in the
    /// report, so a collision would merge two levers into one line.
    #[test]
    fn no_two_kinds_share_a_label() {
        let all = [
            SearchErrorKind::Transient,
            SearchErrorKind::Quota,
            SearchErrorKind::Auth,
            SearchErrorKind::Config,
            SearchErrorKind::InvalidResponse,
            SearchErrorKind::RequestRejected,
            SearchErrorKind::Cancelled,
            SearchErrorKind::Other,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a.as_str(), b.as_str());
            }
        }
    }
}
