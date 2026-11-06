//! Isolated callback probes: a deadlocked child must not strand the suite.

use super::*;
use std::process::{Command, Stdio};
use std::time::Instant;

fn isolated(name: &str, probe: impl FnOnce()) {
    const CHILD: &str = "ALEPH_ACTIVATION_REENTRY_CHILD";
    let test = format!("capability::activation::reentry_tests::{name}");
    if std::env::var(CHILD).as_deref() == Ok(test.as_str()) {
        probe();
        return;
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--nocapture"])
        .env(CHILD, &test)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "child failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("activation callback deadlocked; isolated child killed and reaped");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn attached() -> CapabilityActivationController {
    let mut controller = CapabilityActivationController::new(None, OwnerGeneration(1), vec![]);
    controller.attach().unwrap();
    controller
}

#[test]
fn activation_hooks_can_inspect_shared_state() {
    isolated("activation_hooks_can_inspect_shared_state", || {
        struct Inspect(CapabilityActivationController, Mutex<Vec<LifecycleState>>);
        impl ActivationHook for Inspect {
            fn before_activation(&self, _: &ConstraintContext) -> Result<(), String> {
                self.1.lock().unwrap().push(self.0.state());
                Ok(())
            }
            fn after_activation(&self, _: &ConstraintContext) {
                self.1.lock().unwrap().push(self.0.state());
            }
        }
        let mut controller = attached();
        let hook = Inspect(controller.clone(), Mutex::new(Vec::new()));
        controller
            .activate_with_hook(
                &ConstraintContext::at_hour(12).unwrap(),
                OwnerGeneration(1),
                &hook,
            )
            .unwrap();
        assert_eq!(
            *hook.1.lock().unwrap(),
            [LifecycleState::Attached, LifecycleState::Active]
        );
        assert_eq!(controller.state(), LifecycleState::Active);
    });
}

#[test]
fn activation_pre_hook_revocation_cannot_be_overwritten() {
    isolated(
        "activation_pre_hook_revocation_cannot_be_overwritten",
        || {
            struct Revoke(CapabilityActivationController, Mutex<bool>);
            impl ActivationHook for Revoke {
                fn before_activation(&self, _: &ConstraintContext) -> Result<(), String> {
                    self.0.clone().revoke().unwrap();
                    Ok(())
                }
                fn after_activation(&self, _: &ConstraintContext) {
                    *self.1.lock().unwrap() = true;
                }
            }
            let mut controller = attached();
            let hook = Revoke(controller.clone(), Mutex::new(false));
            let result = controller.activate_with_hook(
                &ConstraintContext::at_hour(12).unwrap(),
                OwnerGeneration(1),
                &hook,
            );
            assert!(
                matches!(result, Err(ActivationError::Lifecycle(_))),
                "{result:?}"
            );
            assert_eq!(controller.state(), LifecycleState::Revoked);
            assert!(
                !*hook.1.lock().unwrap(),
                "failed commit must not call post-hook"
            );
        },
    );
}

#[test]
fn activation_pre_hook_competing_commit_has_only_one_winner() {
    isolated(
        "activation_pre_hook_competing_commit_has_only_one_winner",
        || {
            struct Activate(CapabilityActivationController, Mutex<bool>);
            impl ActivationHook for Activate {
                fn before_activation(&self, context: &ConstraintContext) -> Result<(), String> {
                    self.0
                        .clone()
                        .activate(context, OwnerGeneration(1))
                        .unwrap();
                    Ok(())
                }
                fn after_activation(&self, _: &ConstraintContext) {
                    *self.1.lock().unwrap() = true;
                }
            }
            let mut controller = attached();
            let hook = Activate(controller.clone(), Mutex::new(false));
            let result = controller.activate_with_hook(
                &ConstraintContext::at_hour(12).unwrap(),
                OwnerGeneration(1),
                &hook,
            );
            assert!(
                matches!(result, Err(ActivationError::Lifecycle(_))),
                "{result:?}"
            );
            assert_eq!(controller.state(), LifecycleState::Active);
            assert!(!*hook.1.lock().unwrap());
        },
    );
}
