//! Subprocess-level tests of native ACP owner changes.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use alephcore::acp::manager::{load_persisted_sessions, wire_persistence, AcpAdapterManager};
use alephcore::acp::AcpSessionEvent;
use alephcore::gateway::handlers::acp_config::{
    handle_sessions_cancel, handle_sessions_list, handle_sessions_shutdown,
};
use alephcore::gateway::protocol::JsonRpcRequest;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

const HARNESS: &str = "lifecycle-test";
const DEADLINE: Duration = Duration::from_secs(8);

const PEER: &str = include_str!("fixtures/acp_lifecycle_peer.py");

struct PeerFixture {
    root: TempDir,
    script: PathBuf,
}

impl PeerFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("peer.py");
        std::fs::write(&script, PEER).unwrap();
        Self { root, script }
    }

    fn registration(&self) -> serde_json::Value {
        serde_json::json!({
            "executable": "/usr/bin/env",
            "args": ["python3", &self.script],
            "default_mode": "native_acp",
            "trust_level": "full",
            "timeout_seconds": 30,
        })
    }

    fn manager(&self) -> AcpAdapterManager {
        self.manager_with(&[HARNESS])
    }

    fn manager_with(&self, ids: &[&str]) -> AcpAdapterManager {
        AcpAdapterManager::from_entries(
            ids.iter()
                .map(|id| {
                    (
                        (*id).to_owned(),
                        serde_json::from_value(self.registration()).unwrap(),
                    )
                })
                .collect(),
        )
    }

    fn workspace(&self, name: &str) -> PathBuf {
        let dir = self.root.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        dir
    }
}

fn frames(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("frames")).unwrap_or_default()
}

async fn first_pid(dir: &Path, method: &str) -> i32 {
    timeout(DEADLINE, async {
        loop {
            if let Some(pid) = frames(dir)
                .lines()
                .filter_map(|line| line.split_once(' '))
                .find_map(|(pid, frame)| (frame == method).then(|| pid.parse().unwrap()))
            {
                return pid;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("peer did not receive {method}: {}", frames(dir)))
}

fn assert_exited(pid: i32) {
    // Signal zero checks the recorded process without changing it.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(result, -1, "ACP peer {pid} still exists");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}

#[tokio::test]
async fn same_key_starts_one_child_while_other_key_progresses() {
    let fixture = PeerFixture::new();
    let blocked = fixture.workspace("blocked");
    let free = fixture.workspace("free");
    let gate = blocked.join("hold-initialize");
    std::fs::write(&gate, "").unwrap();
    let manager = fixture.manager();
    let cwd = blocked.to_str().unwrap();
    let first = manager.ensure_session_named(HARNESS, cwd, Some("shared"));
    tokio::pin!(first);
    assert!(futures::poll!(&mut first).is_pending());
    first_pid(&blocked, "initialize").await;

    let alias = blocked.join(".");
    let second = manager.ensure_session_named(HARNESS, alias.to_str().unwrap(), Some("shared"));
    tokio::pin!(second);
    assert!(futures::poll!(&mut second).is_pending());
    timeout(
        DEADLINE,
        manager.ensure_session(HARNESS, free.to_str().unwrap()),
    )
    .await
    .expect("unrelated ACP start stalled")
    .unwrap();
    assert_eq!(frames(&free).matches(" initialize").count(), 1);

    std::fs::remove_file(gate).unwrap();
    let (first, second) = timeout(DEADLINE, async { tokio::join!(first, second) })
        .await
        .expect("same-key ACP starts failed to finish");
    first.unwrap();
    second.unwrap();
    assert_eq!(frames(&blocked).matches(" initialize").count(), 1);
    manager.shutdown_all().await;
}

#[tokio::test]
async fn harness_update_withdraws_pending_old_start() {
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("reconfigured");
    let cwd = dir.to_str().unwrap();
    let gate = dir.join("hold-initialize");
    std::fs::write(&gate, "").unwrap();
    let manager = fixture.manager();
    let old = manager.ensure_session(HARNESS, cwd);
    tokio::pin!(old);
    assert!(futures::poll!(&mut old).is_pending());
    let pid = first_pid(&dir, "initialize").await;

    let replacement = serde_json::from_value(fixture.registration()).unwrap();
    let update = manager.update_harness(HARNESS, replacement);
    let (old, update) = timeout(DEADLINE, async { tokio::join!(old, update) })
        .await
        .expect("harness update did not interrupt initialize");
    update.unwrap();
    assert!(old.is_err(), "stale startup published after harness update");
    assert_exited(pid);
    assert!(manager.list_sessions().await.is_empty());
    std::fs::remove_file(gate).unwrap();
    timeout(DEADLINE, manager.ensure_session(HARNESS, cwd))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frames(&dir).matches(" initialize").count(), 2);
    manager.shutdown_all().await;
}

#[tokio::test]
async fn explicit_replacement_waits_for_old_child_exit() {
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("replaced");
    let cwd = dir.to_str().unwrap();
    let manager = fixture.manager();
    manager
        .prompt(HARNESS, "first", cwd, None, true, None)
        .await
        .unwrap();
    let pid = first_pid(&dir, "session/new").await;
    std::fs::write(dir.join("predecessor"), pid.to_string()).unwrap();

    let result = timeout(
        DEADLINE,
        manager.prompt(HARNESS, "second", cwd, None, false, None),
    )
    .await
    .expect("replacement timed out")
    .unwrap();
    assert_eq!(result, "second");
    let observed = frames(&dir);
    assert_eq!(observed.matches(" session/new").count(), 2);
    assert!(
        !observed.contains("predecessor-still-running"),
        "{observed}"
    );
    assert_exited(pid);
    manager.shutdown_all().await;
}

#[tokio::test]
async fn shutdown_supersedes_a_replacement_waiting_for_its_predecessor() {
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("superseded");
    let cwd = dir.to_str().unwrap();
    let manager = fixture.manager();
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    manager
        .set_persistence_hook(Arc::new(move |event| recorded.lock().unwrap().push(event)))
        .await;
    let active = manager.prompt(HARNESS, "stop reading", cwd, None, true, None);
    tokio::pin!(active);
    let pid = tokio::select! {
        result = &mut active => panic!("prompt completed before peer stalled: {result:?}"),
        pid = first_pid(&dir, "session/prompt") => pid,
    };

    let replace = manager.prompt(HARNESS, "replacement", cwd, None, false, None);
    tokio::pin!(replace);
    assert!(futures::poll!(&mut replace).is_pending());
    let close = manager.shutdown_named(HARNESS, cwd, None);
    let (active, replace, close) =
        timeout(DEADLINE, async { tokio::join!(active, replace, close) })
            .await
            .expect("shutdown stalled while superseding replacement");
    assert!(active.is_err());
    assert!(replace.is_err());
    close.unwrap();
    assert_exited(pid);
    assert_eq!(frames(&dir).matches(" initialize").count(), 1);
    assert!(manager.list_sessions().await.is_empty());
    assert!(matches!(
        events.lock().unwrap().as_slice(),
        [
            AcpSessionEvent::Created { .. },
            AcpSessionEvent::Removed { .. }
        ]
    ));
}

#[tokio::test]
async fn shutdown_reaps_unresponsive_child_and_rejects_stale_waiter() {
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("unresponsive");
    let manager = fixture.manager();
    let cwd = dir.to_str().unwrap();
    assert_eq!(
        manager
            .prompt(HARNESS, "pause after response", cwd, None, true, None)
            .await
            .unwrap(),
        "pause after response"
    );
    // The peer sees data but never reads it. The frame is larger than the pipe.
    let large_prompt = "x".repeat(2 * 1024 * 1024);
    let active = manager.prompt(HARNESS, &large_prompt, cwd, None, true, None);
    tokio::pin!(active);
    let pid = tokio::select! {
        result = &mut active => panic!("prompt completed while stdin was blocked: {result:?}"),
        pid = first_pid(&dir, "stdin-readable") => pid,
    };
    let waiting = manager.set_mode(HARNESS, cwd, None, "review");
    tokio::pin!(waiting);
    assert!(futures::poll!(&mut waiting).is_pending());

    let close = manager.shutdown_named(HARNESS, cwd, None);
    let (active, waiting, close) =
        timeout(DEADLINE, async { tokio::join!(active, waiting, close) })
            .await
            .expect("explicit close waited for the prompt timeout");
    close.unwrap();
    assert!(active.is_err());
    assert!(waiting.is_err());
    assert_exited(pid);
    let observed = frames(&dir);
    assert!(!observed.contains("session/set_mode"), "{observed}");
    assert!(manager.list_sessions().await.is_empty());
}

#[tokio::test]
async fn cancel_reaches_busy_owner_without_waiting_for_prompt_lock() {
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("cancel");
    let cwd = dir.to_str().unwrap();
    let manager = Arc::new(fixture.manager());
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hook_counter = Arc::clone(&changes);
    manager
        .set_gateway_change_hook(Arc::new(move || {
            hook_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }))
        .await;
    let active = manager.prompt(HARNESS, "wait for cancel", cwd, None, true, None);
    tokio::pin!(active);
    let pid = tokio::select! {
        result = &mut active => panic!("prompt completed before cancel: {result:?}"),
        pid = first_pid(&dir, "session/prompt") => pid,
    };
    let rpc = |method| {
        JsonRpcRequest::with_id(
            method,
            Some(json!({"harness": HARNESS, "cwd": cwd})),
            json!(1),
        )
    };
    let live = handle_sessions_list(rpc("acp.sessions.list"), Arc::clone(&manager)).await;
    assert_eq!(
        live.result.unwrap()[0]["acp_session_id"],
        format!("peer-{pid}")
    );
    let before_close = changes.load(std::sync::atomic::Ordering::Relaxed);
    let cancel = handle_sessions_cancel(rpc("acp.sessions.cancel"), Arc::clone(&manager));
    let (active, cancel) = timeout(DEADLINE, async { tokio::join!(active, cancel) })
        .await
        .expect("cancel was blocked by the prompt");
    assert_eq!(active.unwrap(), "canceled");
    assert_eq!(cancel.result.unwrap()["cancelled"], true);
    assert!(frames(&dir).contains(&format!("{pid} cancel:peer-{pid}")));
    let closed = handle_sessions_shutdown(rpc("acp.sessions.shutdown"), Arc::clone(&manager)).await;
    assert_eq!(closed.result.unwrap()["success"], true);
    assert_exited(pid);
    assert!(changes.load(std::sync::atomic::Ordering::Relaxed) > before_close);
    let empty = handle_sessions_list(rpc("acp.sessions.list"), Arc::clone(&manager)).await;
    assert_eq!(empty.result.unwrap(), json!([]));
}

fn run_in_isolated_home(test: &str) -> bool {
    const TEST_HOME: &str = "ALEPH_ACP_LIFECYCLE_TEST_HOME";
    if std::env::var_os(TEST_HOME).is_some() {
        return false;
    }
    let data = tempfile::tempdir().unwrap();
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test])
        .env(TEST_HOME, data.path())
        .env("ALEPH_HOME", data.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    true
}

#[tokio::test]
async fn stop_preserves_saved_id_and_failed_load_does_not_create_a_replacement() {
    if run_in_isolated_home("stop_preserves_saved_id_and_failed_load_does_not_create_a_replacement")
    {
        return;
    }
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("recover");
    let cwd = dir.to_str().unwrap();
    let manager = fixture.manager();
    let (writer, persisted) = wire_persistence(&manager).await;
    assert!(persisted.is_empty());
    manager
        .prompt(HARNESS, "first", cwd, None, true, None)
        .await
        .unwrap();
    let pid = first_pid(&dir, "session/new").await;
    let peer_id = format!("peer-{pid}");
    manager.shutdown_all().await;
    writer.close_and_await().await.unwrap();
    assert_exited(pid);
    assert_eq!(load_persisted_sessions()[0].acp_session_id, peer_id);

    let reject = dir.join("reject-load");
    std::fs::write(&reject, "").unwrap();
    let manager = fixture.manager();
    let (writer, persisted) = wire_persistence(&manager).await;
    assert!(manager.restore_sessions(persisted).await.is_empty());
    let snapshot = manager.list_sessions().await.pop().unwrap();
    assert!(!snapshot.alive);
    assert_eq!(snapshot.acp_session_id.as_deref(), Some(peer_id.as_str()));
    assert!(snapshot.load_error.is_some());
    assert_eq!(frames(&dir).matches(" session/new").count(), 1);
    assert!(frames(&dir).contains(&format!(" load:{peer_id}")));

    std::fs::remove_file(reject).unwrap();
    manager
        .prompt(HARNESS, "retry", cwd, None, true, None)
        .await
        .unwrap();
    let snapshot = manager.list_sessions().await.pop().unwrap();
    assert!(snapshot.alive);
    assert_eq!(snapshot.acp_session_id.as_deref(), Some(peer_id.as_str()));
    assert!(snapshot.load_error.is_none());
    assert_eq!(frames(&dir).matches(" session/new").count(), 1);
    manager.shutdown_named(HARNESS, cwd, None).await.unwrap();
    manager.shutdown_all().await;
    writer.close_and_await().await.unwrap();
    assert!(load_persisted_sessions().is_empty());
}
