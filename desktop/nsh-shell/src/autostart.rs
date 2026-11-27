//! The current application's login item, not server autostart: its status
//! for the Panel's setting, and the write when the user turns it on or off.
use aleph_protocol::panel_shell::{AutostartStatus, AutostartUnavailableReason};
use nsh_core::desktop::LaunchAtLoginError;
use nsh_core::element::desktop::set_launch_at_login;

#[derive(Clone)]
pub(crate) struct Reader {
    identity: std::sync::Arc<Result<String, &'static str>>,
    permit: std::sync::Arc<tokio::sync::Semaphore>,
}

impl Reader {
    pub(crate) fn new(nonce: Option<&str>) -> Self {
        Self {
            identity: std::sync::Arc::new(expected_identity(nonce)),
            permit: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    pub(crate) async fn query(&self) -> AutostartStatus {
        let Ok(identity) = self.identity.as_ref() else {
            return AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::IdentityMismatch,
            };
        };
        let Ok(permit) = self.permit.clone().try_acquire_owned() else {
            return AutostartStatus::Failed {};
        };
        #[cfg(not(target_os = "macos"))]
        {
            // Windows and Linux: nsh reads this copy's login item; it has no
            // pending-approval state to report.
            let _ = identity;
            let enabled = nsh_core::element::desktop::launch_at_login().await;
            drop(permit);
            if enabled {
                AutostartStatus::Enabled {}
            } else {
                AutostartStatus::Disabled {}
            }
        }
        #[cfg(target_os = "macos")]
        self.query_macos(identity.clone(), permit).await
    }

    /// Turn launching at login on or off, then read the status back. A copy
    /// whose status cannot be read (not an app bundle, another identity)
    /// is not written to.
    pub(crate) async fn set(&self, enabled: bool) -> AutostartStatus {
        let before = self.query().await;
        if matches!(
            before,
            AutostartStatus::Unavailable { .. } | AutostartStatus::Failed {}
        ) {
            return before;
        }
        match set_launch_at_login(enabled).await {
            Ok(()) => self.query().await,
            Err(LaunchAtLoginError::Unavailable) => AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::ApiUnavailable,
            },
            Err(error) => {
                eprintln!("NSH login item write failed: {error}");
                AutostartStatus::Failed {}
            }
        }
    }

    #[cfg(target_os = "macos")]
    async fn query_macos(
        &self,
        identity: String,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> AutostartStatus {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let worker = std::thread::Builder::new()
            .name("aleph-login-status".into())
            .spawn(move || {
                // Keep the permit through the real OS read, even if navigation
                // drops the receiver. No entered Tokio runtime is required.
                let status = read(&identity);
                drop(permit);
                let _ = sender.send(status);
            });
        if worker.is_err() {
            return AutostartStatus::Failed {};
        }
        receiver.await.unwrap_or(AutostartStatus::Failed {})
    }
}

pub(crate) fn expected_identity(nonce: Option<&str>) -> Result<String, &'static str> {
    let Some(nonce) = nonce else {
        return Ok(crate::shell_host::APP_ID.to_owned());
    };
    #[cfg(all(debug_assertions, feature = "qa-service"))]
    {
        if !crate::shell_host::valid_qa_nonce(nonce) {
            return Err("invalid QA identity");
        }
        Ok(format!("{}.qa.{nonce}", crate::shell_host::APP_ID))
    }
    #[cfg(not(all(debug_assertions, feature = "qa-service")))]
    {
        let _ = nonce;
        Err("QA status identity unavailable")
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read(_expected_identity: &str) -> AutostartStatus {
    AutostartStatus::Unavailable {
        reason: AutostartUnavailableReason::UnsupportedPlatform,
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn read(expected_identity: &str) -> AutostartStatus {
    use objc2::{rc::autoreleasepool, runtime::AnyClass};
    use objc2_foundation::NSBundle;
    use objc2_service_management::SMAppService;
    use std::path::Path;

    autoreleasepool(|_| {
        let bundle = NSBundle::mainBundle();
        let bundle_path = bundle.bundlePath().to_string();
        if Path::new(&bundle_path)
            .extension()
            .and_then(|value| value.to_str())
            != Some("app")
        {
            return AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::NotAppBundle,
            };
        }
        if bundle
            .bundleIdentifier()
            .is_none_or(|id| id.to_string() != expected_identity)
        {
            return AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::IdentityMismatch,
            };
        }
        // A bundle identifier alone must not authorize a different executable.
        let Some(executable) = bundle.executablePath() else {
            return AutostartStatus::Failed {};
        };
        let (Ok(current), Ok(declared), Ok(root)) = (
            std::env::current_exe().and_then(std::fs::canonicalize),
            std::fs::canonicalize(executable.to_string()),
            std::fs::canonicalize(bundle_path),
        ) else {
            return AutostartStatus::Failed {};
        };
        if current != declared || !current.starts_with(root.join("Contents/MacOS")) {
            return AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::IdentityMismatch,
            };
        }
        // SMAppService is available from macOS 13; do not send a selector
        // on older systems or infer Disabled from an unavailable class.
        if AnyClass::get(c"SMAppService").is_none() {
            return AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::ApiUnavailable,
            };
        }
        // SAFETY: the class exists, the main bundle identity/executable were
        // verified, and both calls are documented read-only OS properties.
        let raw = unsafe { SMAppService::mainAppService().status().0 };
        macos_status(raw)
    })
}

#[cfg(any(target_os = "macos", test))]
fn macos_status(raw: isize) -> AutostartStatus {
    match raw {
        0 => AutostartStatus::Disabled {},
        1 => AutostartStatus::Enabled {},
        2 => AutostartStatus::RequiresApproval {},
        3 => AutostartStatus::Unavailable {
            reason: AutostartUnavailableReason::NotFound,
        },
        _ => AutostartStatus::Failed {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Windows and Linux read through nsh, which needs its runtime.
    #[cfg(target_os = "macos")]
    #[test]
    fn async_status_read_needs_no_entered_tokio_runtime() {
        use std::future::Future;
        let reader = Reader::new(None);
        let mut query = std::pin::pin!(reader.query());
        let mut cx = std::task::Context::from_waker(futures_util::task::noop_waker_ref());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let status = loop {
            if let std::task::Poll::Ready(status) = query.as_mut().poll(&mut cx) {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "status worker did not return"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        assert_eq!(status, read(crate::shell_host::APP_ID));
        assert_eq!(reader.permit.available_permits(), 1);
    }

    #[test]
    fn preserves_all_os_outcomes_without_boolean_collapse() {
        assert_eq!(macos_status(0), AutostartStatus::Disabled {});
        assert_eq!(macos_status(1), AutostartStatus::Enabled {});
        assert_eq!(macos_status(2), AutostartStatus::RequiresApproval {});
        assert_eq!(
            macos_status(3),
            AutostartStatus::Unavailable {
                reason: AutostartUnavailableReason::NotFound
            }
        );
        for raw in [-1, 4, isize::MAX] {
            assert_eq!(macos_status(raw), AutostartStatus::Failed {});
        }
    }

    #[test]
    fn status_contract_rejects_extra_keys_and_non_boolean_shortcuts() {
        for status in [
            macos_status(0),
            macos_status(1),
            macos_status(2),
            macos_status(3),
            macos_status(4),
        ] {
            let value = serde_json::to_value(status).unwrap();
            assert_eq!(
                serde_json::from_value::<AutostartStatus>(value.clone()).unwrap(),
                status
            );
            let mut extra = value;
            extra
                .as_object_mut()
                .unwrap()
                .insert("extra".into(), true.into());
            assert!(serde_json::from_value::<AutostartStatus>(extra).is_err());
        }
        assert!(serde_json::from_str::<AutostartStatus>("false").is_err());
    }

    #[test]
    fn production_identity_is_the_native_host_identity() {
        assert_eq!(
            expected_identity(None),
            Ok(crate::shell_host::APP_ID.to_owned())
        );
    }

    #[test]
    fn invalid_qa_identity_never_falls_back_to_production() {
        for nonce in ["", "a", &"G".repeat(64), &"a".repeat(65)] {
            assert!(expected_identity(Some(nonce)).is_err());
        }
        let result = expected_identity(Some(&"a1".repeat(32)));
        #[cfg(all(debug_assertions, feature = "qa-service"))]
        assert_eq!(
            result,
            Ok(format!(
                "{}.qa.{}",
                crate::shell_host::APP_ID,
                "a1".repeat(32)
            ))
        );
        #[cfg(not(all(debug_assertions, feature = "qa-service")))]
        assert!(result.is_err());
    }

    #[test]
    fn unbundled_process_cannot_report_a_confirmed_disabled_item() {
        assert_eq!(
            read(crate::shell_host::APP_ID),
            AutostartStatus::Unavailable {
                reason: if cfg!(target_os = "macos") {
                    AutostartUnavailableReason::NotAppBundle
                } else {
                    AutostartUnavailableReason::UnsupportedPlatform
                },
            }
        );
    }
}
