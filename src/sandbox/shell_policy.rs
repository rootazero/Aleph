//! Text-policy proof for the fixed Linux JSONL consumer.

use serde_json::Value;

use crate::config::types::ShellSecurityConfig;
use crate::exec::RiskLevel;
use crate::sync_primitives::Arc;

use super::command_policy::{self, CommandPolicyHook};
use super::security_kernel_hook::SecurityKernelHook;
use super::shell_input::{JsonlFrame, ShellEffectDenied, MAX_PREFIX};
use super::{SandboxCommand, SandboxConfig};

/// The same compiled hooks used for launch, not a recompilation of the live
/// config. The fingerprint lets the authority reject drift between boot and
/// the current app config even if a reload did not rebuild the sandbox.
pub(crate) struct ShellPolicy {
    config: Value,
    command: Arc<CommandPolicyHook>,
    operator: Option<Arc<SecurityKernelHook>>,
}

pub(crate) struct ShellPolicyWarnings {
    pub(crate) builtin: Vec<String>,
    pub(crate) operator: bool,
}

impl ShellPolicy {
    pub(super) fn new(
        sandbox: &SandboxConfig,
        security: &ShellSecurityConfig,
        command: Arc<CommandPolicyHook>,
        operator: Option<Arc<SecurityKernelHook>>,
    ) -> Option<Self> {
        Some(Self {
            config: Self::config_view(sandbox, security)?,
            command,
            operator,
        })
    }

    fn config_view(sandbox: &SandboxConfig, security: &ShellSecurityConfig) -> Option<Value> {
        Some(Value::Array(vec![
            serde_json::to_value(sandbox).ok()?,
            serde_json::to_value(security).ok()?,
        ]))
    }

    pub(crate) fn matches_config(&self, config: &crate::Config) -> bool {
        Self::config_view(&config.sandbox, &config.security).as_ref() == Some(&self.config)
    }

    pub(super) fn check_frame(
        &self,
        launch: &SandboxCommand,
        delivered: &[u8],
        frame: &JsonlFrame,
    ) -> Result<ShellPolicyWarnings, ShellEffectDenied> {
        if launch.stdin.is_some() {
            return Err(ShellEffectDenied::Stale);
        }
        let length = delivered
            .len()
            .checked_add(frame.bytes().len())
            .filter(|&len| len <= MAX_PREFIX)
            .ok_or(ShellEffectDenied::TooLarge)?;
        let mut bytes = Vec::with_capacity(length);
        bytes.extend_from_slice(delivered);
        bytes.extend_from_slice(frame.bytes());
        let mut candidate = launch.clone();
        candidate.stdin = Some(bytes);
        let text = command_policy::complete_command_text(&candidate)
            .ok_or(ShellEffectDenied::UnverifiedPolicy)?;
        let view = command_policy::normalize::normalize_complete(&text)
            .ok_or(ShellEffectDenied::UnverifiedPolicy)?;
        let operator = self
            .operator
            .as_ref()
            .and_then(|hook| hook.assess_view(&view));
        if matches!(operator, Some((RiskLevel::Blocked, _))) {
            return Err(ShellEffectDenied::PolicyBlocked);
        }
        let builtin = self.command.evaluate_complete_view(&view);
        if !builtin.blocked.is_empty() {
            return Err(ShellEffectDenied::PolicyBlocked);
        }
        Ok(ShellPolicyWarnings {
            builtin: builtin.warned,
            operator: matches!(operator, Some((RiskLevel::Danger, _))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::command_policy::CommandPolicy;
    use crate::sandbox::shell_input::{JSONL_ARGS, JSONL_PROGRAM};
    use crate::sandbox::SandboxCapabilities;
    use serde_json::json;

    fn test_policy_and_launch() -> (ShellPolicy, SandboxCommand) {
        let config: ShellSecurityConfig = serde_json::from_value(json!({
            "enable_custom_patterns": true,
            "custom_blocked": [
                {"pattern": "(?s)TEST_PREFIX_ALPHA.*TEST_SUFFIX_BETA"},
                {"pattern": "(?s)/records.*TEST_NATIVE_ARGUMENT"}
            ],
            "custom_danger": [{"pattern": "TEST_INPUT_ADVISORY"}],
        }))
        .unwrap();
        let kernel = crate::exec::SecurityKernel::from_config(&config);
        let command_hook = Arc::new(CommandPolicyHook::new(CommandPolicy::hardline_only()));
        let operator = Arc::new(SecurityKernelHook::new(kernel));
        let policy = ShellPolicy::new(
            &SandboxConfig::default(),
            &config,
            command_hook,
            Some(operator),
        )
        .unwrap();
        let launch = SandboxCommand {
            session_id: crate::routing::session_key::SessionKey::ephemeral("jsonl-policy"),
            tool_name: "bash".into(),
            program: JSONL_PROGRAM.into(),
            args: JSONL_ARGS.into_iter().map(str::to_owned).collect(),
            env: Default::default(),
            stdin: None,
            cwd: None,
            capabilities: SandboxCapabilities::session_baseline(),
            timeout: None,
        };
        (policy, launch)
    }

    #[test]
    fn shell_policy_matches_cumulative_prepared_input_without_hiding_normalization_limits() {
        let (policy, launch) = test_policy_and_launch();
        let frame = |text: &str| JsonlFrame::from_record(&json!(text)).unwrap();
        let a = frame("TEST_PREFIX_ALPHA");
        let b = frame("TEST_SUFFIX_BETA");
        assert!(policy.check_frame(&launch, &[], &a).is_ok());
        assert!(policy.check_frame(&launch, &[], &b).is_ok());
        assert!(matches!(
            policy.check_frame(&launch, a.bytes(), &b),
            Err(ShellEffectDenied::PolicyBlocked)
        ));
        assert!(matches!(
            policy.check_frame(&launch, &[], &frame("TEST_NATIVE_ARGUMENT")),
            Err(ShellEffectDenied::PolicyBlocked)
        ));
        let warning = policy
            .check_frame(&launch, &[], &frame("TEST_INPUT_ADVISORY"))
            .unwrap();
        assert!(warning.operator);
        assert!(warning.builtin.is_empty());
        // Literal encoded-looking data can exceed the completeness proof. It
        // is conservatively rejected rather than scanning an incomplete view.
        let ambiguous = frame("-e^nc QUJDREVGR0hJSktMTU5PUA==");
        assert!(matches!(
            policy.check_frame(&launch, &[], &ambiguous),
            Err(ShellEffectDenied::UnverifiedPolicy)
        ));
    }

    #[tokio::test]
    async fn shell_policy_approval_binds_one_frame_and_never_discloses_it() {
        use crate::approval::operator_requester::OperatorApprovalRequester;
        use crate::exec::{socket::ApprovalDecisionType, ExecApprovalManager};
        use crate::gateway::{event_bus::GatewayEventBus, events::GatewayEventFrame};
        use crate::sandbox::shell_input::{
            nonblocking_test_pipe, ShellEffectResult, ShellInputAction, ShellPipeAttempt,
        };
        use crate::sandbox::shell_lifecycle::ShellLifecycle;
        use std::io::{self, Read};
        use std::task::Poll;
        use std::time::{Duration, Instant};
        use tokio_util::sync::CancellationToken;

        let bus = Arc::new(GatewayEventBus::new());
        let mut events = bus.subscribe_typed();
        let manager = Arc::new(ExecApprovalManager::new());
        let requester = OperatorApprovalRequester::for_config_tier(manager.clone(), bus);
        let (policy, launch) = test_policy_and_launch();
        let (mut reader, writer) = nonblocking_test_pipe();
        let job = Arc::new(ShellLifecycle::default());
        job.spawn(|| Ok(())).unwrap();
        job.ready_with_input(writer).unwrap();
        let job_id = 1;
        let deadline = Instant::now() + Duration::from_secs(10);
        let cancel = CancellationToken::new();
        let write = || {
            ShellInputAction::Write(JsonlFrame::from_record(&json!("private test record")).unwrap())
        };
        let prepare = |action, token| {
            ShellPipeAttempt::prepare(
                job.clone(),
                job_id,
                &policy,
                &launch,
                &requester,
                deadline,
                token,
                action,
            )
        };
        let mut first = Box::pin(prepare(write(), cancel.clone()));
        assert!(futures::poll!(first.as_mut()).is_pending());
        let GatewayEventFrame::ApprovalRequested {
            approval_id,
            session_key,
            ..
        } = events.try_recv().unwrap()
        else {
            panic!("expected an operator-only approval request");
        };
        assert!(session_key.is_empty());
        let record = manager.get_pending(&approval_id).unwrap().record;
        assert!(record.operator_only);
        assert!(!record.command.contains("private test record"));
        assert_eq!(
            record.allowed_decisions,
            crate::exec::allowed_decisions::once_only()
        );
        assert!(record.grant_key.is_none());
        assert!(!manager.resolve(&approval_id, ApprovalDecisionType::AllowSession, None));
        assert!(manager.resolve(&approval_id, ApprovalDecisionType::AllowOnce, None));
        let first = first.await.unwrap();
        assert!(manager.get_pending(&approval_id).is_none());
        assert!(matches!(
            events.try_recv(),
            Ok(GatewayEventFrame::ApprovalResolved { .. })
        ));
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        let expected = b"\"private test record\"\n";
        assert_eq!(
            first.consume(),
            Ok(ShellEffectResult::Written(expected.len()))
        );
        let mut observed = [0; b"\"private test record\"\n".len()];
        reader.read_exact(&mut observed).unwrap();
        assert_eq!(&observed, expected);

        let mut pending = Box::pin(prepare(ShellInputAction::Close, cancel.clone()));
        assert!(futures::poll!(pending.as_mut()).is_pending());
        let GatewayEventFrame::ApprovalRequested { approval_id, .. } = events.try_recv().unwrap()
        else {
            panic!("expected a close approval request");
        };
        cancel.cancel();
        assert!(matches!(
            futures::poll!(pending.as_mut()),
            Poll::Ready(Err(ShellEffectDenied::Expired))
        ));
        assert!(manager.get_pending(&approval_id).is_none());
        assert!(matches!(
            events.try_recv(),
            Ok(GatewayEventFrame::ApprovalExpired { .. })
        ));
        assert_eq!(
            reader.read(&mut [0]).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
}
