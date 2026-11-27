//! Restarts the bundled server when the user grants Aleph Input Monitoring
//! or Screen Recording. macOS hands a process these permissions when it
//! starts, so the running server would not see a grant made in System
//! Settings until it starts again.
//!
//! The server is Aleph.app's child, so the grants it gets are the app's;
//! the app reads them in its own process every few seconds.

use crate::local_service::LocalService;
use crate::target_store;
use aleph_client::ConnectionTarget;
use std::path::PathBuf;
use std::time::Duration;

const POLL_INTERVAL: Duration = Duration::from_secs(3);

/// The macOS permissions the server needs at start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Grants {
    input_monitoring: bool,
    screen_recording: bool,
}

/// Whether a permission went from not granted to granted. Only a grant
/// restarts the server; a revocation leaves it running.
fn newly_granted(before: Grants, now: Grants) -> bool {
    (now.input_monitoring && !before.input_monitoring)
        || (now.screen_recording && !before.screen_recording)
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOHIDCheckAccess(request: u32) -> u32;
}

/// `kIOHIDRequestTypeListenEvent`: Input Monitoring.
const LISTEN_EVENT: u32 = 1;
/// `kIOHIDAccessTypeGranted`.
const ACCESS_GRANTED: u32 = 0;

/// Reads both permissions without asking the user for them.
fn read() -> Grants {
    // SAFETY: both are argument-free (or plain-integer) status reads with
    // no preconditions; neither shows a prompt.
    unsafe {
        Grants {
            input_monitoring: IOHIDCheckAccess(LISTEN_EVENT) == ACCESS_GRANTED,
            screen_recording: CGPreflightScreenCaptureAccess(),
        }
    }
}

/// Watches the permissions on a thread of its own for the app's life. On a
/// new grant, while the saved choice is this computer's server, it stops
/// the server and starts it again; the Panel follows the server's phase.
pub(crate) fn start(service: LocalService, home: Option<PathBuf>) {
    let watcher = std::thread::Builder::new()
        .name("aleph-permission-monitor".into())
        .spawn(move || {
            let mut before = read();
            loop {
                std::thread::sleep(POLL_INTERVAL);
                let now = read();
                let local = home
                    .as_deref()
                    .is_none_or(|home| target_store::load_target(home) == ConnectionTarget::Local);
                if newly_granted(before, now) && local && service.stop() {
                    // A connection change may have been saved while stop waited.
                    if home.as_deref().is_none_or(|home| {
                        target_store::load_target(home) == ConnectionTarget::Local
                    }) {
                        eprintln!("macOS permission granted: restarting the local service");
                        service.start();
                    }
                }
                before = now;
            }
        });
    if let Err(error) = watcher {
        eprintln!("could not start the permission monitor: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Grants = Grants {
        input_monitoring: false,
        screen_recording: false,
    };

    #[test]
    fn a_new_grant_restarts() {
        let input = Grants {
            input_monitoring: true,
            ..NONE
        };
        let screen = Grants {
            screen_recording: true,
            ..NONE
        };
        assert!(newly_granted(NONE, input));
        assert!(newly_granted(NONE, screen));
        assert!(newly_granted(
            input,
            Grants {
                input_monitoring: true,
                screen_recording: true,
            }
        ));
    }

    #[test]
    fn no_change_or_a_revocation_does_not() {
        let both = Grants {
            input_monitoring: true,
            screen_recording: true,
        };
        assert!(!newly_granted(NONE, NONE));
        assert!(!newly_granted(both, both));
        assert!(!newly_granted(both, NONE));
    }

    #[test]
    fn reading_the_permissions_does_not_prompt_or_crash() {
        let _ = read();
    }
}
