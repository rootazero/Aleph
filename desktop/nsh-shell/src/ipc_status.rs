//! Explicit read-only probe of a prestarted Desktop IPC Gateway.

use std::{path::Path, process::ExitCode};

pub const FLAG: &str = "--desktop-ipc-status";

pub fn run(args: &[String], qa: bool) -> ExitCode {
    let result = parse_endpoint(args, qa).and_then(query);
    match result {
        Ok(health) => {
            println!("{health}");
            ExitCode::SUCCESS
        }
        Err(reason) => {
            eprintln!("NSH Desktop IPC status refused: {reason}");
            ExitCode::FAILURE
        }
    }
}

fn parse_endpoint(args: &[String], qa: bool) -> Result<&Path, &'static str> {
    if qa {
        return Err("QA configuration cannot be combined with Desktop IPC status");
    }
    const USAGE: &str = "expected only --desktop-ipc-status <local endpoint>";
    let [flag, endpoint] = args else {
        return Err(USAGE);
    };
    if flag != FLAG {
        return Err(USAGE);
    }
    if !aleph_client::local_ipc::AVAILABLE {
        return Err("Desktop IPC is unavailable on this platform");
    }
    let endpoint = Path::new(endpoint);
    #[cfg(windows)]
    aleph_client::local_ipc::windows::validate_endpoint(endpoint)
        .map_err(|_| r"Desktop IPC requires an explicit local \\.\pipe\name")?;
    #[cfg(not(windows))]
    if !endpoint.is_absolute() || endpoint.file_name().is_none() {
        return Err(USAGE);
    }
    Ok(endpoint)
}

fn query(endpoint: &Path) -> Result<String, &'static str> {
    use std::time::Duration;

    use aleph_client::AlephClient;
    use aleph_protocol::system_hello::{ClientHello, HelloParams, ProtocolVersion};
    use serde::{Deserialize, Serialize};
    use serde_json::Value;

    #[derive(Deserialize, Serialize)]
    struct Health {
        status: String,
        timestamp: String,
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "unable to start the status runtime")?;
    runtime.block_on(async {
        let hello = HelloParams {
            protocol: ProtocolVersion { major: 1, minor: 0 },
            client: ClientHello {
                name: "aleph-nsh-shell".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                platform: std::env::consts::OS.into(),
                ui_runtime: "nsh".into(),
            },
            features: vec![],
        };
        let (client, _events) = AlephClient::connect_local(endpoint, &hello)
            .await
            .map_err(|_| "unable to complete the local system.hello")?;
        let result = client
            .call_with_timeout::<Value, Health>("health", None, Duration::from_secs(5))
            .await;
        let closed = tokio::time::timeout(Duration::from_secs(2), client.close()).await;
        let health = result.map_err(|_| "Gateway health request failed")?;
        if !matches!(closed, Ok(Ok(()))) {
            return Err("unable to close the local status connection");
        }
        if health.status != "healthy" || health.timestamp.is_empty() {
            return Err("invalid Gateway health response");
        }
        serde_json::to_string(&health).map_err(|_| "unable to encode the Gateway health response")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn desktop_ipc_status_is_exclusive_and_absolute() {
        #[cfg(windows)]
        let endpoint = r"\\.\pipe\aleph-status-test";
        #[cfg(not(windows))]
        let endpoint_path = std::env::current_dir().unwrap().join("status.sock");
        #[cfg(not(windows))]
        let endpoint = endpoint_path.to_str().unwrap();
        let standalone = args(&[FLAG, endpoint]);
        let result = parse_endpoint(&standalone, false);
        if aleph_client::local_ipc::AVAILABLE {
            assert_eq!(result.unwrap(), Path::new(endpoint));
        } else {
            assert_eq!(result, Err("Desktop IPC is unavailable on this platform"));
        }
        assert!(parse_endpoint(&standalone, true).is_err());
        assert!(parse_endpoint(&args(&[FLAG]), false).is_err());
        assert!(parse_endpoint(&args(&[FLAG, "status.sock"]), false).is_err());
        assert!(parse_endpoint(
            &args(&[FLAG, endpoint, "--connect", "https://localhost"]),
            false
        )
        .is_err());
    }
}
