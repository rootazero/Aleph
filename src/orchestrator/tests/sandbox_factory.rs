use std::collections::HashMap;
use std::sync::Arc;

use crate::orchestrator::flow_spec::SandboxKind;
use crate::orchestrator::sandbox_factory::{build_sandbox_factory, DenyAllSandbox};
use crate::sandbox::capabilities::SandboxCapabilities;
use crate::sandbox::{Sandbox, SandboxCommand};

fn mk_test_cmd(program: &str, args: &[&str]) -> SandboxCommand {
    SandboxCommand {
        session_id: crate::routing::session_key::SessionKey::ephemeral("orch-test"),
        program: program.into(),
        args: args.iter().map(|s| (*s).into()).collect(),
        env: HashMap::new(),
        stdin: None,
        cwd: None,
        capabilities: SandboxCapabilities::strict(),
        timeout: None,
    }
}

#[tokio::test]
async fn deny_all_sandbox_rejects_exec_class() {
    let s: Arc<dyn Sandbox> = Arc::new(DenyAllSandbox::new());
    let err = s.execute(mk_test_cmd("echo", &["hi"])).await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("denied"),
        "got {err}"
    );
}

#[tokio::test]
async fn factory_for_none_returns_deny_all() {
    let factory = build_sandbox_factory(Arc::new(|_session_key| {
        panic!("Workspace builder should not be invoked for SandboxKind::None")
    }));
    let sb = factory(SandboxKind::None, "sess-abc").expect("deny-all");
    assert!(sb.execute(mk_test_cmd("ls", &[])).await.is_err());
}
