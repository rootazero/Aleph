//! Endpoint locality classifier — is a provider's base URL on this machine?
//!
//! Pure DATA, sibling of [`alias`](super::alias) /
//! [`capabilities`](super::capabilities). Given a provider's `base_url`,
//! [`endpoint_kind_for_base_url`] answers a single connectivity fact: does the
//! host resolve to *this machine / the local network* ([`EndpointKind::Local`])
//! or to a public API ([`EndpointKind::Cloud`])?
//!
//! Design stance (R7 LLM sovereignty): this is a connectivity *fact*, never an
//! inference. It never reads the prompt, never scores a task. It feeds the
//! route policy ([`crate::providers::route_policy`]) — a positional
//! infrastructure filter — with a hard signal so an operator's `[route]` mode
//! can shape the failover chain around on-machine vs off-machine providers.
//!
//! Locality keys on the **host** of `base_url`, deliberately *not* the vendor:
//! `ollama.com` (Cloud) and a local Ollama at `http://localhost:11434` (Local)
//! share vendor `ollama` but differ in kind — so [`infer_vendor`] is *not*
//! reused here. Absent or unparseable `base_url` is conservatively [`Cloud`]:
//! the official public API is the safe default when locality is unknown.
//!
//! [`infer_vendor`]: super::alias::infer_vendor
//! [`Cloud`]: EndpointKind::Cloud

use serde::Serialize;
use url::Url;

/// Where a provider endpoint physically lives.
///
/// Serialises to the same lowercase strings [`EndpointKind::as_str`] returns,
/// so embedding the enum in a record and formatting it by hand agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    /// On this machine or the local network (loopback / `.local` / RFC-1918).
    Local,
    /// A public, off-machine API. The conservative default when unknown.
    Cloud,
}

impl EndpointKind {
    /// Stable, lowercase identifier for serialization / RPC surfaces.
    ///
    /// Mirrors [`Modality::as_str`](crate::providers::metadata::Modality::as_str):
    /// these strings appear in the `providers.catalog` RPC and the
    /// `list_models` tool output, so callers (panel picker, the LLM choosing a
    /// model) can reason over "local vs cloud" without re-deriving it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Cloud => "cloud",
        }
    }
}

/// Classify a provider's `base_url` host as [`Local`](EndpointKind::Local) or
/// [`Cloud`](EndpointKind::Cloud).
///
/// `Local` iff the host is a loopback name/literal (`localhost`, `127.*`,
/// `::1`, `0.0.0.0`), ends in `.local` / `.localhost`, or is an RFC-1918 /
/// link-local private literal (`10.*`, `192.168.*`, `172.16-31.*`,
/// `169.254.*`). Everything else — including an absent or unparseable
/// `base_url` — is `Cloud` (the conservative public-API assumption).
///
/// Scheme-tolerant: a bare `host:port` with no scheme is still classified by
/// prefixing `http://` before parsing.
#[must_use]
pub fn endpoint_kind_for_base_url(base_url: Option<&str>) -> EndpointKind {
    let raw = match base_url {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return EndpointKind::Cloud,
    };

    let host = match extract_host(raw) {
        Some(h) => h,
        None => return EndpointKind::Cloud,
    };

    if host_is_local(&host) {
        EndpointKind::Local
    } else {
        EndpointKind::Cloud
    }
}

/// Pull the lowercase host out of a base URL, tolerating a missing scheme.
///
/// `pub(crate)` so sibling normalization code (e.g. the `OpenAI` model-id
/// canonicaliser) can make first-party-vs-aggregator host decisions without
/// duplicating this scheme-tolerant parse.
pub(crate) fn extract_host(raw: &str) -> Option<String> {
    // `Url::parse` requires a scheme; a bare `localhost:11434` parses with
    // `localhost` as the *scheme*, not the host. Prefix `http://` when the
    // input lacks a `://` separator so host extraction is reliable.
    let candidate = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&candidate).ok()?;
    url.host_str().map(|h| h.to_ascii_lowercase())
}

/// Whether a lowercase host string denotes this machine or the local network.
// rust-doctor-disable-next-line high-cyclomatic-complexity
fn host_is_local(host: &str) -> bool {
    // The `url` crate returns IPv6 hosts bracketed (`[::1]`); strip the brackets
    // so literal comparisons and the `Ipv6Addr` parse below see the bare address.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);

    // Loopback / wildcard names and the IPv6 loopback literal.
    if host == "localhost" || host == "::1" || host == "0.0.0.0" {
        return true;
    }
    // mDNS / local-suffix names (`printer.local`, `dev.localhost`).
    if host.ends_with(".local") || host.ends_with(".localhost") {
        return true;
    }

    // IPv6 literal: classify loopback / unspecified / link-local / unique-local.
    if let Ok(v6) = host.parse::<std::net::Ipv6Addr>() {
        if v6.is_loopback() || v6.is_unspecified() {
            return true;
        }
        let seg0 = v6.segments()[0];
        // fe80::/10 link-local
        if (seg0 & 0xffc0) == 0xfe80 {
            return true;
        }
        // fc00::/7 unique-local
        if (seg0 & 0xfe00) == 0xfc00 {
            return true;
        }
    }

    // IPv4 literal: classify by RFC-1918 / loopback / link-local octets.
    if let Some(octets) = parse_ipv4(host) {
        let [a, b, ..] = octets;
        // 127.0.0.0/8 loopback
        if a == 127 {
            return true;
        }
        // 10.0.0.0/8
        if a == 10 {
            return true;
        }
        // 192.168.0.0/16
        if a == 192 && b == 168 {
            return true;
        }
        // 172.16.0.0/12
        if a == 172 && (16..=31).contains(&b) {
            return true;
        }
        // 169.254.0.0/16 link-local
        if a == 169 && b == 254 {
            return true;
        }
    }

    false
}

/// Parse a dotted-quad IPv4 literal into its four octets, or `None`.
fn parse_ipv4(host: &str) -> Option<[u8; 4]> {
    let mut it = host.split('.');
    let a = it.next()?.parse::<u8>().ok()?;
    let b = it.next()?.parse::<u8>().ok()?;
    let c = it.next()?.parse::<u8>().ok()?;
    let d = it.next()?.parse::<u8>().ok()?;
    if it.next().is_some() {
        return None; // more than four parts — not a dotted quad
    }
    Some([a, b, c, d])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_names_and_literals_are_local() {
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://localhost:11434")),
            EndpointKind::Local
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://127.0.0.1:8080/v1")),
            EndpointKind::Local
        );
        // Bare host:port with no scheme is still classified.
        assert_eq!(
            endpoint_kind_for_base_url(Some("localhost:4000")),
            EndpointKind::Local
        );
    }

    #[test]
    fn rfc1918_and_link_local_literals_are_local() {
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://10.0.0.5:11434")),
            EndpointKind::Local
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://192.168.1.10/v1")),
            EndpointKind::Local
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://172.16.0.1:9000")),
            EndpointKind::Local
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://172.31.255.255")),
            EndpointKind::Local
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://169.254.0.1")),
            EndpointKind::Local
        );
    }

    #[test]
    fn dot_local_suffix_is_local() {
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://mybox.local:11434")),
            EndpointKind::Local
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://dev.localhost/v1")),
            EndpointKind::Local
        );
    }

    #[test]
    fn public_hosts_are_cloud() {
        assert_eq!(
            endpoint_kind_for_base_url(Some("https://api.openai.com/v1")),
            EndpointKind::Cloud
        );
        assert_eq!(
            endpoint_kind_for_base_url(Some("https://api.anthropic.com")),
            EndpointKind::Cloud
        );
        // ollama.com is Cloud despite the vendor — locality keys on host, not vendor.
        assert_eq!(
            endpoint_kind_for_base_url(Some("https://ollama.com")),
            EndpointKind::Cloud
        );
        // 172.32.* is OUTSIDE the 172.16-31 private range → public.
        assert_eq!(
            endpoint_kind_for_base_url(Some("http://172.32.0.1")),
            EndpointKind::Cloud
        );
    }

    #[test]
    fn absent_or_empty_base_url_is_cloud() {
        assert_eq!(endpoint_kind_for_base_url(None), EndpointKind::Cloud);
        assert_eq!(endpoint_kind_for_base_url(Some("")), EndpointKind::Cloud);
        assert_eq!(endpoint_kind_for_base_url(Some("   ")), EndpointKind::Cloud);
    }

    #[test]
    fn as_str_is_stable() {
        // Lock these strings — they appear in providers.catalog RPC and the
        // list_models tool output.
        assert_eq!(EndpointKind::Local.as_str(), "local");
        assert_eq!(EndpointKind::Cloud.as_str(), "cloud");
    }

    #[test]
    fn unparseable_base_url_is_cloud() {
        // Garbage with no extractable host → conservative Cloud.
        assert_eq!(
            endpoint_kind_for_base_url(Some("not a url at all")),
            EndpointKind::Cloud
        );
    }
}
