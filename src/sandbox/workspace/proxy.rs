//! Managed proxy handling for the workspace sandbox.

use std::sync::Arc;

use crate::sandbox::capabilities::NetworkPolicy;
use crate::sandbox::command::{SandboxCommand, SandboxError};
use crate::sandbox::driver::OsSandboxDriverTrait;
use crate::sandbox::proxy::{
    self as sandbox_proxy, AllowList, HostBridgeHandle, ProxyHandle, ProxyRouteSpec,
};

/// Standard proxy env vars injected for `AllowHosts`. Shared between the env
/// injection and the Linux route spec so the two never drift (the netns-side
/// `sandbox-init` rewrites exactly these keys to the local bridge port).
pub(crate) const PROXY_ENV_KEYS: [&str; 6] = [
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
];

/// Holds the managed proxy — and, on Linux, the netns host bridge — alive for
/// the duration of one sandboxed run. Drop shuts the proxy accept loop down
/// and removes the host bridge's UDS directory.
pub(crate) struct ActiveProxy {
    pub(crate) _proxy: ProxyHandle,
    pub(crate) _bridge: Option<HostBridgeHandle>,
}

/// Spawn a managed proxy if the current capabilities request `AllowHosts`
/// *and* the platform can reach it. Returns:
///
/// - `Ok(Some(active))` — proxy is live, `cmd.capabilities.network` has
///   been collapsed to `AllowHosts(["127.0.0.1"])`, and the standard proxy
///   env vars have been injected into `cmd.env`. On Linux a host bridge is
///   also live and `ALEPH_SANDBOX_PROXY_ROUTE_SPEC` is injected so the
///   driver bind-mounts the UDS and `sandbox-init` runs the local bridge.
/// - `Ok(None)` — policy is not `AllowHosts`, allowlist is empty, or the
///   platform cannot reach the proxy (Windows `AppContainer`; see Phase D).
/// - `Err(_)` — bind/spawn failure surfaces as `SandboxError::Other`.
///
/// The caller holds the returned guard alive across the OS driver `run`.
/// Drop = shutdown.
pub(crate) async fn maybe_spawn_proxy(
    os_driver: &Arc<dyn OsSandboxDriverTrait>,
    cmd: &mut SandboxCommand,
) -> Result<Option<ActiveProxy>, SandboxError> {
    let hosts = match &cmd.capabilities.network {
        NetworkPolicy::AllowHosts { hosts } if !hosts.is_empty() => hosts.clone(),
        _ => return Ok(None),
    };

    // Only macOS Seatbelt and Linux bwrap can reach the host managed
    // proxy. `windows/token` runs the target inside an AppContainer whose
    // default isolation blocks loopback (the `CheckNetIsolationEnable-
    // Loopback` API requires admin); Phase D will use WFP. On unsupported
    // platforms we leave capabilities untouched so the existing
    // platform-level hard-fail surfaces the documented gap — spawning a
    // proxy the sandbox cannot reach would turn a clean refusal into an
    // opaque "connection refused" at runtime.
    let platform = os_driver.platform();
    if platform != "macos/seatbelt" && platform != "linux/bwrap" {
        return Ok(None);
    }

    let allowlist = AllowList::new(&hosts);
    let handle = sandbox_proxy::spawn(allowlist)
        .await
        .map_err(|e| SandboxError::Other(format!("managed proxy spawn: {e}")))?;
    let proxy_url = handle.http_url();

    // Linux: `--unshare-net` isolates the sandbox in its own netns, so the
    // host loopback proxy is unreachable directly. Spawn a host bridge
    // (UDS ↔ host proxy) and hand `sandbox-init` a route spec so it can run
    // the netns-side local bridge and rewrite the proxy port to the bridge.
    // The host proxy address is what the bridge forwards to.
    let bridge = if platform == "linux/bwrap" {
        let socket_dir = sandbox_proxy::create_proxy_socket_dir()
            .map_err(|e| SandboxError::Other(format!("proxy socket dir: {e}")))?;
        let uds_path = socket_dir.join("proxy-route-0.sock");
        let bridge = sandbox_proxy::spawn_host_bridge(&uds_path, socket_dir, handle.local_addr())
            .await
            .map_err(|e| SandboxError::Other(format!("host bridge spawn: {e}")))?;
        let spec = ProxyRouteSpec {
            uds_path,
            env_keys: PROXY_ENV_KEYS.iter().map(|s| s.to_string()).collect(),
        };
        let spec_json = spec
            .to_env_json()
            .map_err(|e| SandboxError::Other(format!("serialize proxy route spec: {e}")))?;
        // Insert (not or_insert): this is internal routing state the driver
        // and sandbox-init must see, never caller-overridable.
        cmd.env
            .insert(sandbox_proxy::PROXY_ROUTE_SPEC_ENV.to_string(), spec_json);
        Some(bridge)
    } else {
        None
    };

    // Collapse the OS-level allowlist to the proxy's loopback address.
    // The proxy itself enforces the hostname allowlist; the OS just
    // ensures nothing escapes loopback.
    cmd.capabilities.network = NetworkPolicy::AllowHosts {
        hosts: vec!["127.0.0.1".into()],
    };

    // Inject standard proxy env vars. `or_insert` lets caller-supplied
    // env override our defaults — handy for tests that pin the URL
    // shape, and matches the existing `ALEPH_SANDBOX` env contract. On
    // Linux `sandbox-init` rewrites the port of these to the local bridge.
    for k in PROXY_ENV_KEYS {
        cmd.env
            .entry(k.to_string())
            .or_insert_with(|| proxy_url.clone());
    }
    // NO_PROXY exempts the loopback itself so curl/python clients
    // don't loop back through the proxy when talking to it.
    cmd.env
        .entry("NO_PROXY".to_string())
        .or_insert_with(|| "127.0.0.1,localhost,::1".to_string());
    cmd.env
        .entry("no_proxy".to_string())
        .or_insert_with(|| "127.0.0.1,localhost,::1".to_string());

    tracing::debug!(
        target: "sandbox.proxy",
        allowlist = ?hosts,
        proxy = %proxy_url,
        platform = %platform,
        bridged = bridge.is_some(),
        "spawned managed proxy for AllowHosts"
    );

    Ok(Some(ActiveProxy {
        _proxy: handle,
        _bridge: bridge,
    }))
}
