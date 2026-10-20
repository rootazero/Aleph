//! Source-only ownership regressions. Main runs these after the writers stop.
use super::*;
use crate::capability::ownership::{ClaimState, RunId};
#[cfg(unix)]
use crate::extension::hooks::HookMemoSink;
use crate::extension::hooks::{HookContext, HookResult};
use crate::extension::types::{HookAction, HookEvent, HookKind, PluginKind, PluginOrigin};
#[cfg(unix)]
use std::sync::Mutex;

fn declaration(label: &str) -> HookConfig {
    HookConfig {
        event: HookEvent::BeforeAgentStart,
        kind: HookKind::Interceptor,
        priority: HookPriority::Normal,
        matcher: None,
        actions: vec![HookAction::Prompt {
            prompt: label.into(),
        }],
        plugin_name: label.into(),
        plugin_root: PathBuf::new(),
        handler: None,
        timeout_secs: None,
        declared_event: Some("BeforeAgentStart".into()),
        scope_key: ScopeKey::Global,
    }
}

fn user(config: HookConfig, scope: &ScopeKey) -> HookInput {
    HookInput::user(config, Path::new("/task11-c/hooks.json"), scope)
}

fn exact_id(input: &HookInput) -> CapabilityId {
    CapabilityId {
        namespace: "hook".into(),
        name: identity(&input.config, &input.origin).unwrap().0 .0,
    }
}

fn registry(tree: &Arc<OwnershipTree>) -> HookRegistry {
    HookRegistry::new_with_ownership_tree(HookExecutor::empty(), Arc::clone(tree))
}

async fn fire(executor: &HookExecutor) -> HookResult {
    executor
        .execute_interceptors(
            HookEvent::BeforeAgentStart,
            HookContext::new("ownership-test"),
        )
        .await
        .unwrap()
        .1
}

fn assert_denied(result: &HookResult, cause: &str) {
    assert!(result.blocked && result.denied, "{result:?}");
    assert!(
        !result.action_failed,
        "ownership refusal must survive the stop gate"
    );
    assert!(result.block_reason.as_deref().unwrap().contains(cause));
    assert!(result.deny_reason.as_deref().unwrap().contains(cause));
    assert!(result.additional_contexts.is_empty());
    assert!(result.action_results.is_empty());
    assert!(result.agents_to_invoke.is_empty());
}

#[tokio::test]
async fn supplied_tree_binds_exact_global_and_project_keys() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let project = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let global = user(declaration("global"), &ScopeKey::Global);
    let scoped = user(declaration("project"), &ScopeKey::project(project.path()));
    let global_id = exact_id(&global);
    let project_id = exact_id(&scoped);
    let project_visibility = VisibilityScope {
        workspace: Some(canonical_root(project.path()).to_str().unwrap().into()),
        ..VisibilityScope::default()
    };
    let wrong_visibility = VisibilityScope {
        workspace: Some(canonical_root(other.path()).to_str().unwrap().into()),
        ..VisibilityScope::default()
    };
    registry.replace_users(vec![global, scoped]).await.unwrap();
    assert!(tree
        .resolve(&global_id, &VisibilityScope::default())
        .is_some());
    assert!(tree.resolve(&project_id, &project_visibility).is_some());
    assert!(tree
        .resolve(&project_id, &VisibilityScope::default())
        .is_none());
    assert!(tree.resolve(&project_id, &wrong_visibility).is_none());
    assert!(tree.resolve(&global_id, &project_visibility).is_none());
    let snapshot = registry.snapshot();
    assert!(Arc::ptr_eq(
        &snapshot.value.users[0]
            .metadata
            .binding
            .as_ref()
            .unwrap()
            .tree,
        &tree
    ));
    let rows = snapshot.value.executor.inventory();
    assert!(rows
        .iter()
        .all(|row| row.admission.as_deref() == Some("accepted")));
    assert!(rows
        .iter()
        .any(|row| row.hook_id.as_deref() == Some(global_id.name.as_str())));
    assert!(rows
        .iter()
        .any(|row| row.hook_id.as_deref() == Some(project_id.name.as_str())));
    let in_project = crate::projects::with_project_root(
        Some(project.path().to_path_buf()),
        fire(&snapshot.value.executor),
    )
    .await;
    assert_eq!(in_project.additional_contexts, ["global", "project"]);
    let elsewhere = crate::projects::with_project_root(
        Some(other.path().to_path_buf()),
        fire(&snapshot.value.executor),
    )
    .await;
    assert_eq!(elsewhere.additional_contexts, ["global"]);
}

#[tokio::test]
async fn publication_and_rebuild_preserve_incumbent_owner_generation_claim_and_lifetime() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let input = user(declaration("incumbent"), &ScopeKey::Global);
    let id = exact_id(&input);
    let visibility = VisibilityScope::default();
    let owner = OwnerRef::Run(RunId("incumbent-run".into()));
    tree.register(
        id.clone(),
        owner.clone(),
        LifetimeScope::Run,
        visibility.clone(),
    )
    .unwrap();
    tree.bump(LifetimeScope::Runtime);
    let generation = tree.generation(&id, &visibility).unwrap();
    let claim = tree
        .claim(&id, &visibility, owner.clone(), "incumbent-request".into())
        .unwrap();
    registry.replace_users(vec![input.clone()]).await.unwrap();
    registry
        .publish_transition(registry.begin_transition(), Vec::new(), vec![input])
        .await
        .unwrap();
    assert_eq!(tree.generation(&id, &visibility), Some(generation));
    assert_eq!(
        tree.claim_state(&claim, &id, &visibility),
        ClaimState::Active
    );
    assert!(tree
        .claim(&id, &visibility, OwnerRef::Runtime, "wrong-owner".into())
        .is_none());
    assert!(tree
        .claim(&id, &visibility, owner, "same-owner".into())
        .is_some());
    let captured = registry.snapshot();
    assert_eq!(
        fire(&captured.value.executor).await.additional_contexts,
        ["incumbent"]
    );
    // Disposing Run proves registration did not replace the incumbent's lifetime with Runtime.
    assert!(tree.dispose(LifetimeScope::Run));
    assert!(tree.resolve(&id, &visibility).is_none());
    assert_denied(&fire(&captured.value.executor).await, "disposed");
}

#[tokio::test]
async fn captured_interceptor_denies_after_revoke_without_prompt_or_infrastructure_failure() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let input = user(declaration("must-not-leak-after-revoke"), &ScopeKey::Global);
    let id = exact_id(&input);
    registry.replace_users(vec![input]).await.unwrap();
    let captured = registry.snapshot();
    let before = fire(&captured.value.executor).await;
    assert_eq!(before.additional_contexts, ["must-not-leak-after-revoke"]);
    assert_eq!(before.action_results.len(), 1);
    assert!(tree.revoke(&id));
    assert_denied(&fire(&captured.value.executor).await, "revoked");
    let row = captured.value.executor.inventory().remove(0);
    assert!(!row.reachable);
    assert!(row.issue.unwrap().contains("revoked"));
}

#[tokio::test]
async fn runtime_disposal_denies_captured_execution_and_republication() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let input = user(declaration("runtime"), &ScopeKey::Global);
    let id = exact_id(&input);
    registry.replace_users(vec![input.clone()]).await.unwrap();
    let captured = registry.snapshot();
    assert_eq!(
        fire(&captured.value.executor).await.additional_contexts,
        ["runtime"]
    );
    assert!(tree.dispose(LifetimeScope::Runtime));
    assert_denied(&fire(&captured.value.executor).await, "disposed");
    registry.replace_users(vec![input]).await.unwrap();
    let row = registry.read().await.inventory().remove(0);
    assert_eq!(row.admission.as_deref(), Some("rejected"));
    assert!(row.effective_priority.is_none() && !row.reachable);
    assert!(row.admission_reason.unwrap().contains("disposed lifetime"));
    assert!(tree.resolve(&id, &VisibilityScope::default()).is_none());
    assert_denied(&fire(&registry.snapshot().value.executor).await, "disposed");
}

#[tokio::test]
async fn revoked_republication_stays_rejected_and_never_resurrects() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let input = user(declaration("revoked"), &ScopeKey::Global);
    let id = exact_id(&input);
    registry.replace_users(vec![input.clone()]).await.unwrap();
    let captured = registry.snapshot();
    assert_eq!(
        fire(&captured.value.executor).await.additional_contexts,
        ["revoked"]
    );
    tree.revoke(&id);
    registry.replace_users(vec![input.clone()]).await.unwrap();
    registry
        .publish_transition(registry.begin_transition(), Vec::new(), vec![input])
        .await
        .unwrap();
    let snapshot = registry.snapshot();
    let row = snapshot.value.executor.inventory().remove(0);
    assert_eq!(row.hook_id.as_deref(), Some(id.name.as_str()));
    assert_eq!(row.admission.as_deref(), Some("rejected"));
    assert!(row.effective_priority.is_none() && !row.reachable);
    assert!(row.admission_reason.unwrap().contains("revoked capability"));
    assert!(tree.is_revoked(&id));
    assert!(tree.resolve(&id, &VisibilityScope::default()).is_none());
    assert_eq!(snapshot.value.executor.inventory().len(), 1);
    assert_denied(&fire(&captured.value.executor).await, "revoked");
    assert_denied(&fire(&snapshot.value.executor).await, "revoked");
    assert!(snapshot.value.users[0].metadata.binding.is_none());
    assert!(matches!(
        snapshot.value.users[0].metadata.ownership_refusal,
        Some(OwnershipRefusal::Revoked)
    ));
}

#[tokio::test]
async fn user_and_plugin_system_rejections_remain_unbound_even_with_an_incumbent() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let mut config = declaration("user-system");
    config.priority = HookPriority::System;
    let user = user(config.clone(), &ScopeKey::Global);
    let user_id = exact_id(&user);
    let row = PluginRecord::new(
        "trusted-row".into(),
        "display".into(),
        PluginKind::Static,
        PluginOrigin::Global,
    );
    let plugin = HookInput::plugin(config, &row);
    let plugin_id = exact_id(&plugin);
    registry
        .publish_transition(
            registry.begin_transition(),
            vec![plugin],
            vec![user.clone()],
        )
        .await
        .unwrap();
    for id in [&user_id, &plugin_id] {
        assert!(tree.resolve(id, &VisibilityScope::default()).is_none());
    }
    // An independently existing owner cannot grant a rejected declaration admission.
    tree.register(
        user_id,
        OwnerRef::Runtime,
        LifetimeScope::Runtime,
        VisibilityScope::default(),
    )
    .unwrap();
    registry.replace_users(vec![user]).await.unwrap();
    let snapshot = registry.snapshot();
    assert_eq!(snapshot.value.executor.hook_count(), 0);
    assert!(snapshot
        .value
        .executor
        .inventory()
        .iter()
        .all(|row| row.admission.as_deref() == Some("rejected")
            && row.effective_priority.is_none()
            && !row.reachable));
    assert!(snapshot.value.users[0].metadata.binding.is_none());
    assert!(snapshot.value.plugins[0].metadata.binding.is_none());
    assert!(fire(&snapshot.value.executor)
        .await
        .action_results
        .is_empty());
}

#[tokio::test]
async fn remove_and_readd_preserve_owner_and_duplicate_ordered_effects() {
    let tree = Arc::new(OwnershipTree::new());
    let registry = registry(&tree);
    let a = user(declaration("A"), &ScopeKey::Global);
    let b = user(declaration("B"), &ScopeKey::Global);
    let id = exact_id(&a);
    registry.replace_users(vec![a.clone()]).await.unwrap();
    let claim = tree
        .claim(
            &id,
            &VisibilityScope::default(),
            OwnerRef::Runtime,
            "before-removal".into(),
        )
        .unwrap();
    registry.replace_users(Vec::new()).await.unwrap();
    assert_eq!(registry.read().await.hook_count(), 0);
    assert!(fire(&registry.snapshot().value.executor)
        .await
        .action_results
        .is_empty());
    assert!(!tree.is_revoked(&id));
    assert!(tree.resolve(&id, &VisibilityScope::default()).is_some());
    assert_eq!(
        tree.claim_state(&claim, &id, &VisibilityScope::default()),
        ClaimState::Active
    );
    registry.replace_users(vec![a.clone(), b, a]).await.unwrap();
    let snapshot = registry.snapshot();
    assert_eq!(
        snapshot.value.users[0].metadata.hook_id,
        snapshot.value.users[2].metadata.hook_id
    );
    assert_eq!(
        tree.claim_state(&claim, &id, &VisibilityScope::default()),
        ClaimState::Active
    );
    let result = fire(&snapshot.value.executor).await;
    assert_eq!(result.additional_contexts, ["A", "B", "A"]);
    assert_eq!(result.action_results.len(), 3);
}

#[cfg(unix)]
#[derive(Default)]
struct Memo(Mutex<Vec<String>>);
#[cfg(unix)]
#[async_trait::async_trait]
impl HookMemoSink for Memo {
    async fn record(&self, _event: HookEvent, _capability: Option<&str>, outcome: &str) {
        self.0.lock().unwrap().push(outcome.into());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn captured_observer_has_real_effect_before_revoke_and_only_error_memo_after() {
    let tree = Arc::new(OwnershipTree::new());
    let memo = Arc::new(Memo::default());
    let registry = HookRegistry::new_with_ownership_tree(
        HookExecutor::empty().with_memo_sink(memo.clone()),
        Arc::clone(&tree),
    );
    let dir = tempfile::tempdir().unwrap();
    let mut config = declaration("observer-effect");
    config.kind = HookKind::Observer;
    config.actions = vec![HookAction::Command {
        command: "printf x >> ownership-effect".into(),
    }];
    let input = user(config, &ScopeKey::Global);
    let id = exact_id(&input);
    registry.replace_users(vec![input.clone()]).await.unwrap();
    let captured = registry.snapshot();
    let context = HookContext::new("observer").with_working_dir(dir.path());
    captured
        .value
        .executor
        .execute_observers(HookEvent::BeforeAgentStart, &context)
        .await;
    assert_eq!(
        std::fs::read(dir.path().join("ownership-effect")).unwrap(),
        b"x"
    );
    assert_eq!(*memo.0.lock().unwrap(), ["completed"]);
    tree.revoke(&id);
    captured
        .value
        .executor
        .execute_observers(HookEvent::BeforeAgentStart, &context)
        .await;
    assert_eq!(
        std::fs::read(dir.path().join("ownership-effect")).unwrap(),
        b"x"
    );
    assert_eq!(*memo.0.lock().unwrap(), ["completed", "error"]);
    registry.replace_users(vec![input]).await.unwrap();
    let republished = registry.snapshot();
    republished
        .value
        .executor
        .execute_observers(HookEvent::BeforeAgentStart, &context)
        .await;
    assert_eq!(
        std::fs::read(dir.path().join("ownership-effect")).unwrap(),
        b"x"
    );
    assert_eq!(*memo.0.lock().unwrap(), ["completed", "error", "error"]);
    let rows = republished.value.executor.inventory();
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].reachable && rows[0].effective_priority.is_none());
    assert_eq!(rows[0].admission.as_deref(), Some("rejected"));
}

#[cfg(unix)]
struct RevokeOnCompleted {
    tree: Arc<OwnershipTree>,
    id: CapabilityId,
    outcomes: Mutex<Vec<String>>,
}
#[cfg(unix)]
#[async_trait::async_trait]
impl HookMemoSink for RevokeOnCompleted {
    async fn record(&self, _event: HookEvent, _capability: Option<&str>, outcome: &str) {
        self.outcomes.lock().unwrap().push(outcome.into());
        if outcome == "completed" {
            self.tree.revoke(&self.id);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn observer_rechecks_owner_between_actions_without_sleep_or_global_state() {
    let tree = Arc::new(OwnershipTree::new());
    let dir = tempfile::tempdir().unwrap();
    let mut config = declaration("observer-two-actions");
    config.kind = HookKind::Observer;
    config.actions = vec![
        HookAction::Command {
            command: "printf first >> ownership-effect".into(),
        },
        HookAction::Command {
            command: "printf second >> ownership-effect".into(),
        },
    ];
    let input = user(config, &ScopeKey::Global);
    let memo = Arc::new(RevokeOnCompleted {
        tree: Arc::clone(&tree),
        id: exact_id(&input),
        outcomes: Mutex::new(Vec::new()),
    });
    let registry = HookRegistry::new_with_ownership_tree(
        HookExecutor::empty().with_memo_sink(memo.clone()),
        Arc::clone(&tree),
    );
    registry.replace_users(vec![input]).await.unwrap();
    registry
        .snapshot()
        .value
        .executor
        .execute_observers(
            HookEvent::BeforeAgentStart,
            &HookContext::new("between-actions").with_working_dir(dir.path()),
        )
        .await;
    assert_eq!(
        std::fs::read(dir.path().join("ownership-effect")).unwrap(),
        b"first"
    );
    assert_eq!(*memo.outcomes.lock().unwrap(), ["completed", "error"]);
}

#[tokio::test]
async fn interceptor_rechecks_owner_after_an_already_started_http_action() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let tree = Arc::new(OwnershipTree::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = declaration("interceptor-two-actions");
    config.actions = vec![
        HookAction::Http {
            url: format!("http://{}/", listener.local_addr().unwrap()),
            headers: Default::default(),
        },
        HookAction::Prompt {
            prompt: "must-not-start".into(),
        },
    ];
    let input = user(config, &ScopeKey::Global);
    let id = exact_id(&input);
    let registry = registry(&tree);
    registry.replace_users(vec![input]).await.unwrap();
    let captured = registry.snapshot();
    let server = async {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 4096];
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before completing request");
            request.extend_from_slice(&chunk[..count]);
            if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + length {
                    break;
                }
            }
        }
        assert!(tree.revoke(&id));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
    };
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        tokio::join!(fire(&captured.value.executor), server)
    })
    .await
    .expect("local HTTP action must complete without sleeps");
    assert!(
        result.blocked && result.denied && !result.action_failed,
        "{result:?}"
    );
    assert!(result.deny_reason.unwrap().contains("revoked"));
    assert_eq!(
        result.action_results.len(),
        1,
        "the started action can finish"
    );
    assert!(
        result.additional_contexts.is_empty(),
        "the next Prompt never starts"
    );
}

#[test]
fn startup_and_default_constructors_forward_the_single_runtime_tree() {
    fn compact(source: &str) -> String {
        crate::utils::source_scan::code_text(&crate::utils::source_scan::production_prefix(source))
            .split_whitespace()
            .collect()
    }
    let startup = compact(include_str!(
        "../../../bin/aleph-server/commands/start/mod.rs"
    ));
    let construct = "letownership_tree=Arc::new(OwnershipTree::new());";
    let install = "initialize_extension_manager(args.daemon,Arc::clone(&ownership_tree)).await;";
    let mount = "ProjectionHost::mount_with_diagnostics((*tool_registry_phase2).clone(),Arc::clone(&ownership_tree),diagnostics_enabled,";
    assert_eq!(startup.matches(construct).count(), 1);
    assert!(startup.find(construct).unwrap() < startup.find(install).unwrap());
    assert!(startup.find(install).unwrap() < startup.find(mount).unwrap());
    let helper = compact(include_str!(
        "../../../bin/aleph-server/commands/start/helpers.rs"
    ));
    assert!(helper
        .contains("ExtensionManager::with_defaults_with_ownership_tree(ownership_tree).await"));
    let manager = compact(include_str!("../../mod.rs"));
    assert!(
        manager.contains("Self::new_with_ownership_tree(ExtensionConfig::default(),tree).await")
    );
    assert!(manager.contains("Self::new(ExtensionConfig::default()).await"));
    assert!(manager.contains("HookRegistry::new_with_ownership_tree("));
}

#[tokio::test]
async fn isolated_registry_constructor_and_legacy_executor_preserve_old_behavior() {
    let tree = Arc::new(OwnershipTree::new());
    let shared = registry(&tree);
    let input = user(declaration("constructor-effect"), &ScopeKey::Global);
    let id = exact_id(&input);
    shared.replace_users(vec![input.clone()]).await.unwrap();
    tree.revoke(&id);
    let isolated = HookRegistry::new(HookExecutor::empty());
    assert!(!Arc::ptr_eq(&isolated.ownership_tree, &tree));
    isolated.replace_users(vec![input]).await.unwrap();
    assert_eq!(
        fire(&isolated.snapshot().value.executor)
            .await
            .additional_contexts,
        ["constructor-effect"]
    );
    assert_eq!(
        fire(&HookExecutor::new(vec![declaration("legacy")]))
            .await
            .additional_contexts,
        ["legacy"]
    );
}
