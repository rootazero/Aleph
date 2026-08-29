//! Prometheus text-exposition endpoint (`GET /metrics`).
//!
//! Surfaces the gateway's *already-collected* operational counters in the
//! Prometheus v0.0.4 text format so a scraper (or `curl`) can read them.
//! Before this endpoint existed, the [`RequestStateRegistry`] (wired live into
//! the JSON-RPC middleware chain) and the connection table tracked these
//! numbers but nothing ever exposed them — this is the missing export layer,
//! not a new metrics pipeline.
//!
//! Design choices:
//! - **No `prometheus` crate.** The exposition format is a handful of lines of
//!   text; hand-rendering it keeps the core lightweight (architectural redline
//!   R3) and avoids a heavy dependency for one endpoint.
//! - **Reuses existing state.** Request-lifecycle counts come from the global
//!   [`RequestStateRegistry`] snapshot; connection / uptime / rate-limit
//!   numbers come from [`GatewaySharedState`]. No new struct fields, no new
//!   collection path.
//! - **Unauthenticated, like `/health` and `/ready`.** Only low-cardinality
//!   aggregate counts are exported — never prompt/response/token text, secrets,
//!   or per-request payloads — so the exposure class matches the sibling
//!   probes (scraped on a trusted network / behind the same reverse proxy).
//!
//! Counter vs gauge semantics map cleanly onto the registry's behaviour:
//! terminal states (`completed`/`failed`/`cancelled`) are never decremented
//! (`RequestStateRegistry::remove` only adjusts non-terminal counts), so they
//! are monotonic **counters**; in-flight states rise and fall, so they are
//! **gauges**.

use crate::sync_primitives::Arc;

use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;

use std::collections::HashMap;

use super::GatewaySharedState;
use crate::gateway::middleware::latency::get_global_latency;
use crate::gateway::middleware::request_state::get_global_registry;

/// Prometheus exposition content type (format version 0.0.4).
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// How many of the open connections actually hold authority.
///
/// # Why `wall_admits` and not a local `matches!(role, "operator" | "member")`
///
/// This was `connections_active` — the collector returned `(active, active)`,
/// justified by a "LAN-trust: every connection is an implicit operator"
/// comment that stopped being true when the login wall and the
/// guest/member/operator split landed. The exported gauge's own HELP text
/// ("Open connections that have authenticated") had been contradicting it
/// since. Ten remote sockets parked at the login wall reported
/// `authenticated == active`, so an alert on "connections that never
/// authenticate" could never fire.
///
/// Reusing [`handler::wall_admits`] rather than re-deriving the role set keeps
/// ONE derivation of "does this connection hold authority" in the gateway: a
/// demotion via `handlers::users::restamp_live_connections` moves this gauge in
/// the same instant it closes the request and event planes. `method: ""` is the
/// same argument the event-forward arm passes — there is no `connect`
/// exemption to grant when nobody asked anything.
fn count_authenticated(conns: &HashMap<String, super::ConnectionState>) -> u64 {
    conns
        .values()
        .filter(|c| super::handler::wall_admits(Some(c.caller_role.as_str()), ""))
        .count() as u64
}

/// Flat, owned snapshot of every value the endpoint renders. Decoupling the
/// data-gathering (async, touches shared state) from the formatting (pure,
/// synchronous) keeps [`render_prometheus`] unit-testable without a running
/// server.
#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub version: &'static str,
    pub instance_id: String,
    pub uptime_secs: u64,
    pub connections_active: u64,
    pub connections_authenticated: u64,
    pub max_connections: u64,
    pub max_connections_per_ip: u64,
    pub rate_limit_tracked_entries: u64,
    // Request lifecycle (from RequestStateRegistry).
    pub req_pending: u64,
    pub req_validating: u64,
    pub req_processing: u64,
    pub req_awaiting: u64,
    pub req_completed: u64,
    pub req_failed: u64,
    pub req_cancelled: u64,
}

/// `GET /metrics` — render the current gateway counters as Prometheus text.
pub async fn handle_metrics(State(state): State<Arc<GatewaySharedState>>) -> impl IntoResponse {
    let uptime_secs = (chrono::Utc::now().timestamp() - state.started_at_unix).max(0) as u64;

    let (connections_active, connections_authenticated) = {
        let conns = state.connections.read().await;
        (conns.len() as u64, count_authenticated(&conns))
    };

    // Request-lifecycle counts come from the global registry the middleware
    // chain installs at startup. If it is somehow not yet set (e.g. probed
    // mid-boot before the chain is built), every field stays zero.
    let reg = get_global_registry().map(|r| r.snapshot());

    let snapshot = MetricsSnapshot {
        version: env!("ALEPH_VERSION"),
        instance_id: state.instance_id.clone(),
        uptime_secs,
        connections_active,
        connections_authenticated,
        max_connections: state.max_connections as u64,
        max_connections_per_ip: state.max_connections_per_ip as u64,
        rate_limit_tracked_entries: state.rate_limiter.tracked_entries() as u64,
        req_pending: reg.as_ref().map_or(0, |s| s.pending),
        req_validating: reg.as_ref().map_or(0, |s| s.validating),
        req_processing: reg.as_ref().map_or(0, |s| s.processing),
        req_awaiting: reg.as_ref().map_or(0, |s| s.awaiting),
        req_completed: reg.as_ref().map_or(0, |s| s.completed),
        req_failed: reg.as_ref().map_or(0, |s| s.failed),
        req_cancelled: reg.as_ref().map_or(0, |s| s.cancelled),
    };

    let mut body = render_prometheus(&snapshot);
    // Latency histogram is a separate, self-rendering metric family (variable
    // bucket lines) appended after the flat gauges/counters.
    if let Some(hist) = get_global_latency() {
        body.push_str(&hist.render_prometheus("aleph_gateway_request_duration_ms"));
    }

    ([(CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)], body)
}

/// Escape a Prometheus label value per the exposition spec: backslash,
/// double-quote, and newline are the only characters requiring escaping.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out
}

/// Pure renderer: turn a [`MetricsSnapshot`] into Prometheus text exposition.
pub fn render_prometheus(s: &MetricsSnapshot) -> String {
    let version = escape_label(s.version);
    let instance = escape_label(&s.instance_id);
    let mut out = String::with_capacity(1024);

    out.push_str("# HELP aleph_gateway_build_info Gateway build/version info (constant 1).\n");
    out.push_str("# TYPE aleph_gateway_build_info gauge\n");
    out.push_str(&format!(
        "aleph_gateway_build_info{{version=\"{version}\",instance=\"{instance}\"}} 1\n"
    ));

    out.push_str("# HELP aleph_gateway_uptime_seconds Seconds since server start.\n");
    out.push_str("# TYPE aleph_gateway_uptime_seconds gauge\n");
    out.push_str(&format!("aleph_gateway_uptime_seconds {}\n", s.uptime_secs));

    out.push_str("# HELP aleph_gateway_connections_active Currently open WebSocket connections.\n");
    out.push_str("# TYPE aleph_gateway_connections_active gauge\n");
    out.push_str(&format!(
        "aleph_gateway_connections_active {}\n",
        s.connections_active
    ));

    out.push_str(
        "# HELP aleph_gateway_connections_authenticated Open connections that have authenticated.\n",
    );
    out.push_str("# TYPE aleph_gateway_connections_authenticated gauge\n");
    out.push_str(&format!(
        "aleph_gateway_connections_authenticated {}\n",
        s.connections_authenticated
    ));

    out.push_str(
        "# HELP aleph_gateway_max_connections Configured maximum concurrent connections.\n",
    );
    out.push_str("# TYPE aleph_gateway_max_connections gauge\n");
    out.push_str(&format!(
        "aleph_gateway_max_connections {}\n",
        s.max_connections
    ));

    out.push_str(
        "# HELP aleph_gateway_max_connections_per_ip Configured per-IP concurrent connection cap (0=disabled).\n",
    );
    out.push_str("# TYPE aleph_gateway_max_connections_per_ip gauge\n");
    out.push_str(&format!(
        "aleph_gateway_max_connections_per_ip {}\n",
        s.max_connections_per_ip
    ));

    out.push_str(
        "# HELP aleph_gateway_rate_limit_tracked_entries Distinct (identity,scope) rate-limit windows held.\n",
    );
    out.push_str("# TYPE aleph_gateway_rate_limit_tracked_entries gauge\n");
    out.push_str(&format!(
        "aleph_gateway_rate_limit_tracked_entries {}\n",
        s.rate_limit_tracked_entries
    ));

    out.push_str(
        "# HELP aleph_gateway_requests_total Terminal JSON-RPC requests by final status.\n",
    );
    out.push_str("# TYPE aleph_gateway_requests_total counter\n");
    out.push_str(&format!(
        "aleph_gateway_requests_total{{status=\"completed\"}} {}\n",
        s.req_completed
    ));
    out.push_str(&format!(
        "aleph_gateway_requests_total{{status=\"failed\"}} {}\n",
        s.req_failed
    ));
    out.push_str(&format!(
        "aleph_gateway_requests_total{{status=\"cancelled\"}} {}\n",
        s.req_cancelled
    ));

    out.push_str(
        "# HELP aleph_gateway_requests_in_flight In-flight JSON-RPC requests by lifecycle state (sum() for total).\n",
    );
    out.push_str("# TYPE aleph_gateway_requests_in_flight gauge\n");
    out.push_str(&format!(
        "aleph_gateway_requests_in_flight{{state=\"pending\"}} {}\n",
        s.req_pending
    ));
    out.push_str(&format!(
        "aleph_gateway_requests_in_flight{{state=\"validating\"}} {}\n",
        s.req_validating
    ));
    out.push_str(&format!(
        "aleph_gateway_requests_in_flight{{state=\"processing\"}} {}\n",
        s.req_processing
    ));
    out.push_str(&format!(
        "aleph_gateway_requests_in_flight{{state=\"awaiting\"}} {}\n",
        s.req_awaiting
    ));

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MetricsSnapshot {
        MetricsSnapshot {
            version: "26.5.31",
            instance_id: "abc-123".to_string(),
            uptime_secs: 42,
            connections_active: 3,
            connections_authenticated: 2,
            max_connections: 1000,
            max_connections_per_ip: 64,
            rate_limit_tracked_entries: 7,
            req_pending: 1,
            req_validating: 0,
            req_processing: 2,
            req_awaiting: 0,
            req_completed: 100,
            req_failed: 5,
            req_cancelled: 1,
        }
    }

    #[test]
    fn renders_build_info_with_labels() {
        let out = render_prometheus(&sample());
        assert!(
            out.contains("aleph_gateway_build_info{version=\"26.5.31\",instance=\"abc-123\"} 1")
        );
    }

    #[test]
    fn renders_terminal_counters_and_inflight_gauges() {
        let out = render_prometheus(&sample());
        assert!(out.contains("aleph_gateway_requests_total{status=\"completed\"} 100"));
        assert!(out.contains("aleph_gateway_requests_total{status=\"failed\"} 5"));
        assert!(out.contains("aleph_gateway_requests_total{status=\"cancelled\"} 1"));
        assert!(out.contains("aleph_gateway_requests_in_flight{state=\"processing\"} 2"));
        assert!(out.contains("aleph_gateway_requests_in_flight{state=\"pending\"} 1"));
    }

    #[test]
    fn renders_connection_and_uptime_gauges() {
        let out = render_prometheus(&sample());
        assert!(out.contains("aleph_gateway_connections_active 3"));
        assert!(out.contains("aleph_gateway_connections_authenticated 2"));
        assert!(out.contains("aleph_gateway_uptime_seconds 42"));
        assert!(out.contains("aleph_gateway_rate_limit_tracked_entries 7"));
    }

    #[test]
    fn every_metric_has_help_and_type_lines() {
        let out = render_prometheus(&sample());
        // One HELP + one TYPE per flat metric family (9; histogram is appended
        // separately by the handler, not by render_prometheus).
        assert_eq!(out.matches("# HELP ").count(), 9);
        assert_eq!(out.matches("# TYPE ").count(), 9);
    }

    #[test]
    fn escapes_label_special_characters() {
        assert_eq!(escape_label(r#"a"b\c"#), r#"a\"b\\c"#);
        assert_eq!(escape_label("line\nbreak"), "line\\nbreak");
    }

    /// `renders_connection_and_uptime_gauges` above hand-builds
    /// `connections_active: 3, connections_authenticated: 2` — a pair the
    /// collector could not produce, because it returned `(active, active)`.
    /// The formatter was always right; nothing tested the collector. This
    /// tests the collector.
    #[test]
    fn the_authenticated_gauge_excludes_a_walled_guest() {
        use crate::gateway::server::ConnectionState;
        use std::net::IpAddr;

        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        let mut conns: HashMap<String, ConnectionState> = HashMap::new();
        for (id, role) in [("c1", "operator"), ("c2", "member"), ("c3", "guest")] {
            let mut st = ConnectionState::new(ip, false);
            st.caller_role = role.to_string();
            conns.insert(id.to_string(), st);
        }

        assert_eq!(conns.len(), 3, "three open connections");
        assert_eq!(
            count_authenticated(&conns),
            2,
            "a connection parked at the login wall with caller_role=\"guest\" \
             has not authenticated; reporting it as authenticated makes an \
             alert on \"connections that never authenticate\" unable to fire"
        );
    }

    /// The other half: the gauge must not become a second derivation of "holds
    /// authority". If someone re-writes it as a local role `matches!`, a
    /// demotion through `restamp_live_connections` would move the delivery
    /// planes and leave this behind.
    #[test]
    fn the_authenticated_gauge_reuses_the_login_walls_own_predicate() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/gateway/server/metrics_endpoint.rs"
        ))
        .expect("this file is readable from its own test");
        let production = crate::utils::source_scan::production_prefix(&src);
        let code = crate::utils::source_scan::strip_comment_lines(&production);
        assert!(
            code.contains("handler::wall_admits("),
            "count_authenticated must call the login wall's own predicate, not \
             restate the authorized role set"
        );
        // And the collector must actually use it. `count_authenticated` being
        // correct proves nothing while `handle_metrics` still exports
        // `(active, active)` — that is exactly the shape this started as, with
        // a passing formatter test beside it.
        assert!(
            code.contains("count_authenticated(&conns)"),
            "handle_metrics must derive the authenticated gauge from \
             count_authenticated; exporting connections_active twice makes the \
             two gauges structurally unable to differ"
        );
    }
}
