//! Managed in-process proxy for sandbox `NetworkPolicy::AllowHosts`.
//!
//! # Why
//!
//! Cycles 1-5 closed many platform parity gaps but left per-host network
//! filtering as a documented blocker (`SANDBOX.md § Network Filtering`).
//! Three constraints made it hard:
//! - macOS Seatbelt's `(remote ip "…")` matches IPs only — hostnames cannot
//!   be enforced at the kernel.
//! - Linux bubblewrap `--unshare-net` is all-or-nothing; per-host shaping
//!   needs `nftables` in `CAP_NET_ADMIN` (admin) or a netns→loopback bridge.
//! - Windows WFP requires admin and writes machine-global filters.
//!
//! Phase A (this module) sidesteps all three: a tiny in-process proxy
//! speaking HTTP CONNECT and SOCKS5 listens on `127.0.0.1:0` per session,
//! enforces the allowlist by hostname (or IP literal), and is reached by
//! injecting `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` into the sandboxed
//! process' environment. The OS-level network policy is collapsed to
//! "loopback only".
//!
//! - **macOS**: replaces the Cycle-1 IP-only allowlist; clients now route
//!   through the proxy and the Seatbelt profile permits only 127.0.0.1.
//! - **Windows**: replaces the `UnsupportedPolicy` hard-fail. AppContainer
//!   already grants `INetwork` for non-`None` policies; loopback is enabled
//!   in the same profile path.
//! - **Linux**: still hard-fails — `--unshare-net` strips loopback access
//!   inside the netns. Phase B (netns bridge, ported from codex
//!   `proxy_routing.rs`) will close this in a follow-up cycle.
//!
//! # Threat model
//!
//! The proxy is **internal to the host** and not exposed beyond the
//! loopback interface. Its allowlist is the only gate; the OS-level
//! network policy is configured to force all traffic through it (or to
//! deny everything else outright). A sandboxed process that somehow
//! bypasses the proxy gets a hard kernel-level deny.

mod allowlist;
mod connect;
mod lifecycle;
mod socks5;

pub use allowlist::AllowList;
pub use lifecycle::{spawn, ProxyHandle, ProxyStartError};
