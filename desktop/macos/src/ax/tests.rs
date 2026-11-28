use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use aleph_protocol::desktop_bridge::envelope::RpcError;
use aleph_protocol::desktop_bridge::methods::ax::NativeAxReference;

use super::*;

const GUARD: Duration = Duration::from_secs(5);

// Each shell process owns only its lease. It accepts a controlled release of
// the delayed reply and detects any attempted use of a legacy mutation.
const HELPER: &str = r#"#!/bin/sh
set -eu
dir=${0%/*}
mode=$(cat "$dir/mode")
request_id() { sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'; }
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | request_id)
  case "$line" in
    *'"method":"ax.perform_action"'*|*'"method":"ax.set_value"'*)
      : > "$dir/legacy-write"
      exit 1 ;;
    *'"method":"fixture.exit"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
      exit 0 ;;
    *'"method":"ax.snapshot_references"'*|*'"method":"ax.act_on_reference"'*)
      if [ "$mode" = old ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"method not found"}}\n' "$id"
        continue
      fi ;;
    *) exit 1 ;;
  esac
  case "$line" in
    *'"method":"ax.snapshot_references"'*)
      if [ -f "$dir/reap-required" ]; then
        old_pid=$(cat "$dir/reap-required")
        if kill -0 "$old_pid" 2>/dev/null; then
          : > "$dir/early-readback"
          exit 1
        fi
      fi
      printf '{"jsonrpc":"2.0","id":%s,"result":{"app_pid":42,"tree_available":true,"elements":[{"element":{"role":"AXButton","pid":42,"children":[]},"reference":{"lease":"%s","index":0}}],"tree_truncated":false,"truncated":false,"nodes_walked":1,"total_interactable":1}}\n' "$id" "$$" ;;
    *\"lease\":\""$$"\"*)
      printf '%s\n' "$$" > "$dir/received"
      if [ "$mode" = delay ]; then
        IFS= read -r release
        case "$release" in *'"method":"fixture.release"'*) ;; *) exit 1 ;; esac
        release_id=$(printf '%s\n' "$release" | request_id)
        printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$release_id"
      fi
      printf '%s\n' "$$" >> "$dir/writes"
      if [ "$mode" = malformed ]; then
        printf '{"jsonrpc":"2.0","id":%s,"result":{"performed":true,"path":"pointer"}}\n' "$id"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"performed":true,"path":"accessibility"}}\n' "$id"
      fi ;;
    *)
      printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32008,"message":"stale","data":{"code":"stale_reference"}}}\n' "$id" ;;
  esac
done
"#;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("aleph-ax-reference-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("mode"), mode).unwrap();
        let helper = dir.join("helper");
        fs::write(&helper, HELPER).unwrap();
        fs::set_permissions(helper, fs::Permissions::from_mode(0o700)).unwrap();
        Self { dir }
    }

    fn bridge(&self) -> Arc<SwiftBridge> {
        Arc::new(SwiftBridge::new(self.dir.join("helper")))
    }

    async fn received_pid(&self) -> String {
        tokio::time::timeout(GUARD, async {
            loop {
                if let Ok(pid) = fs::read_to_string(self.dir.join("received")) {
                    if !pid.trim().is_empty() {
                        return pid.trim().to_owned();
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the fake helper did not receive the AX mutation")
    }

    fn assert_reaped(&self, pid: &str) {
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success(),
            "the caller settled while the exact helper child {pid} still existed"
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn snapshot_request() -> AxSnapshotRequest {
    AxSnapshotRequest {
        pid: Some(42),
        window_id: None,
        max_depth: 6,
        max_nodes: 1_500,
        max_elements: 100,
    }
}

fn press(reference: NativeAxReference) -> AxReferenceAction {
    AxReferenceAction {
        reference,
        mutation: AxMutation::Perform {
            action: "AXPress".into(),
        },
    }
}

fn assert_refusal(error: DesktopError, expected: AxReferenceError) {
    assert!(
        matches!(error, DesktopError::AxReference { code, .. } if code == expected),
        "{error:?}"
    );
}

async fn snapshot(backend: &BridgeAccessibility) -> NativeAxReference {
    let snapshot = tokio::time::timeout(GUARD, async {
        loop {
            match backend.snapshot_references(snapshot_request()).await {
                Err(DesktopError::BridgeBackoff(_)) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                result => return result.unwrap(),
            }
        }
    })
    .await
    .expect("the fake helper did not provide a snapshot");
    assert_eq!(snapshot.app_pid, 42);
    snapshot.elements[0].reference.clone().unwrap()
}

#[tokio::test]
async fn ax_ref_bridge_generation_isolation() {
    let fixture = Fixture::new("old");
    let bridge = fixture.bridge();
    let backend = BridgeAccessibility::new(Arc::clone(&bridge));
    assert_refusal(
        backend
            .snapshot_references(snapshot_request())
            .await
            .unwrap_err(),
        AxReferenceError::ReferenceUnsupported,
    );
    assert_refusal(
        backend
            .act_on_reference(press(NativeAxReference {
                lease: "previous".into(),
                index: 0,
            }))
            .await
            .unwrap_err(),
        AxReferenceError::ReferenceUnsupported,
    );
    assert!(
        bridge.is_running(),
        "an unsupported RPC is a definitive reply"
    );

    fs::write(fixture.dir.join("mode"), "current").unwrap();
    async fn restart(bridge: &SwiftBridge) {
        bridge.call::<_, ()>("fixture.exit", ()).await.unwrap();
        tokio::time::timeout(GUARD, async {
            while bridge.is_running() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    restart(&bridge).await;
    let previous = snapshot(&backend).await;
    // An accepted definitive action keeps the same helper and lease alive.
    assert!(
        backend
            .act_on_reference(press(previous.clone()))
            .await
            .unwrap()
            .performed
    );
    assert!(
        backend
            .act_on_reference(press(previous.clone()))
            .await
            .unwrap()
            .performed
    );
    restart(&bridge).await;
    let current = snapshot(&backend).await;
    assert_ne!(current.lease, previous.lease);
    assert_refusal(
        backend
            .act_on_reference(press(previous.clone()))
            .await
            .unwrap_err(),
        AxReferenceError::StaleReference,
    );
    assert!(
        backend
            .act_on_reference(press(current.clone()))
            .await
            .unwrap()
            .performed
    );
    let writes = fs::read_to_string(fixture.dir.join("writes")).unwrap();
    assert_eq!(
        writes.lines().collect::<Vec<_>>(),
        [
            previous.lease.as_str(),
            previous.lease.as_str(),
            current.lease.as_str()
        ]
    );
    assert!(!fixture.dir.join("legacy-write").exists());
}

#[tokio::test]
async fn ax_ref_timeout_reaps_before_readback() {
    let fixture = Fixture::new("delay");
    let bridge = fixture.bridge();
    let backend = Arc::new(BridgeAccessibility::new(Arc::clone(&bridge)));
    let reference = snapshot(&backend).await;
    let action = press(reference.clone());
    let mut running = tokio::spawn({
        let backend = Arc::clone(&backend);
        async move {
            backend
                .act_on_reference_with_timeout(action, Duration::from_secs(1))
                .await
        }
    });
    let old_pid = fixture.received_pid().await;
    assert_eq!(old_pid, reference.lease);
    assert!(
        !running.is_finished(),
        "receipt requested before the delayed reply"
    );
    fs::write(fixture.dir.join("reap-required"), &old_pid).unwrap();
    assert_refusal(
        tokio::time::timeout(GUARD, &mut running)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err(),
        AxReferenceError::ActionOutcomeUnknown,
    );
    fixture.assert_reaped(&old_pid);
    assert_ne!(snapshot(&backend).await, reference);
    assert!(!fixture.dir.join("early-readback").exists());
    assert!(!fixture.dir.join("writes").exists());
    assert!(!fixture.dir.join("legacy-write").exists());
}

#[tokio::test]
async fn ax_ref_cancelling_the_caller_preserves_the_bound_reply_receiver() {
    let fixture = Fixture::new("delay");
    let bridge = fixture.bridge();
    let backend = Arc::new(BridgeAccessibility::new(Arc::clone(&bridge)));
    let reference = snapshot(&backend).await;
    let caller = tokio::spawn({
        let backend = Arc::clone(&backend);
        let action = press(reference.clone());
        async move { backend.act_on_reference_with_timeout(action, GUARD).await }
    });
    assert_eq!(fixture.received_pid().await, reference.lease);
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(!fixture.dir.join("writes").exists());

    bridge.call::<_, ()>("fixture.release", ()).await.unwrap();
    assert_eq!(snapshot(&backend).await.lease, reference.lease);
    assert_eq!(
        fs::read_to_string(fixture.dir.join("writes"))
            .unwrap()
            .trim(),
        reference.lease,
    );
    assert!(!fixture.dir.join("legacy-write").exists());
}

#[tokio::test]
async fn ax_ref_unproven_receipt_reaps_without_retrying_the_mutation() {
    let fixture = Fixture::new("malformed");
    let backend = BridgeAccessibility::new(fixture.bridge());
    let reference = snapshot(&backend).await;
    fs::write(fixture.dir.join("reap-required"), &reference.lease).unwrap();
    assert_refusal(
        tokio::time::timeout(GUARD, backend.act_on_reference(press(reference.clone())))
            .await
            .unwrap()
            .unwrap_err(),
        AxReferenceError::ActionOutcomeUnknown,
    );
    fixture.assert_reaped(&reference.lease);
    assert_ne!(snapshot(&backend).await.lease, reference.lease);
    assert_eq!(
        fs::read_to_string(fixture.dir.join("writes"))
            .unwrap()
            .trim(),
        reference.lease,
    );
    assert!(!fixture.dir.join("early-readback").exists());
    assert!(!fixture.dir.join("legacy-write").exists());
}

#[test]
fn ax_ref_reply_requires_the_expected_completion_shape() {
    for invalid in [
        serde_json::json!({"performed": false, "path": "accessibility"}),
        serde_json::json!({"performed": true, "path": "pointer"}),
        serde_json::json!({"performed": true, "path": "accessibility"}),
    ] {
        // All these shapes lack a definitive set-value receipt.
        assert!(definitive_ax_reply(&Ok(invalid), true).is_none());
    }
    let malformed_error = RpcError {
        code: ERR_AX_REFERENCE,
        message: "missing typed code".into(),
        data: None,
    };
    assert!(definitive_ax_reply(&Err(malformed_error), false).is_none());
}
