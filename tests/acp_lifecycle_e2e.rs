//! Subprocess-level tests of native ACP owner changes.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use alephcore::acp::manager::AcpAdapterManager;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};

const HARNESS: &str = "lifecycle-test";
const DEADLINE: Duration = Duration::from_secs(8);

const PEER: &str = r#"
import json
import os
from pathlib import Path
import sys
import threading
import time

cwd = Path.cwd()
pid = os.getpid()

def record(line):
    with (cwd / 'frames').open('a') as frames:
        frames.write(f'{pid} {line}\n')

for line in sys.stdin:
    frame = json.loads(line)
    method = frame['method']
    record(method)
    if method == 'initialize':
        while (cwd / 'hold-initialize').exists():
            time.sleep(0.01)
        result = {'protocolVersion': 1, 'agentCapabilities': {'loadSession': True}}
    elif method == 'session/new':
        predecessor = cwd / 'predecessor'
        if predecessor.exists():
            try:
                os.kill(int(predecessor.read_text()), 0)
                record('predecessor-still-running')
            except ProcessLookupError:
                pass
        result = {'sessionId': f'peer-{pid}'}
    elif method == 'session/load':
        result = {}
    elif method == 'session/prompt':
        text = frame['params']['prompt'][0]['text']
        if text == 'stop reading':
            threading.Event().wait()
        result = {'content': text}
    elif method == 'session/cancel':
        continue
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': frame['id'], 'result': result}), flush=True)
"#;

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
        let config = serde_json::from_value(self.registration()).unwrap();
        AcpAdapterManager::from_entries([(HARNESS.into(), config)].into())
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
async fn shutdown_reaps_unresponsive_child_and_rejects_stale_waiter() {
    let fixture = PeerFixture::new();
    let dir = fixture.workspace("unresponsive");
    let manager = fixture.manager();
    let cwd = dir.to_str().unwrap();
    let active = manager.prompt(HARNESS, "stop reading", cwd, None, true, None);
    tokio::pin!(active);
    let pid = tokio::select! {
        result = &mut active => panic!("prompt completed before peer stalled: {result:?}"),
        pid = first_pid(&dir, "session/prompt") => pid,
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
