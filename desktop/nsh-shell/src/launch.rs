//! What one app process shows: the Gateway target and, in a debug QA run, the
//! isolated QA context. The command-line entry validates its arguments and
//! presets the launch before the app starts; the nsh runner starts the app
//! without a preset, so the app reads the production target itself.

use crate::shell_host::HostSettings;
use aleph_client::ConnectionTarget;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The QA context of a debug run; present exactly when
/// `HostSettings::qa_nonce` is.
#[derive(Clone, Debug)]
pub(crate) struct QaLaunch {
    pub(crate) run_id: String,
    pub(crate) message: String,
    /// Inject the status and pairing drivers instead of the chat driver.
    pub(crate) status: bool,
    pub(crate) status_labels: String,
    /// Isolated placement storage, so a QA run keeps its window bounds
    /// without touching the user's.
    pub(crate) window_data_dir: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub(crate) struct Launch {
    pub(crate) settings: HostSettings,
    pub(crate) qa: Option<QaLaunch>,
    pub(crate) rejected_address: Option<String>,
}

static PRESET: OnceLock<Launch> = OnceLock::new();

/// Fixes this process's launch; call it once, before the app starts.
pub(crate) fn preset(launch: Launch) {
    preset_in(&PRESET, launch);
}

/// The preset launch, or the production one when nothing was preset.
pub(crate) fn current() -> Launch {
    current_in(&PRESET, std::env::home_dir().as_deref())
}

fn preset_in(cell: &OnceLock<Launch>, launch: Launch) {
    if cell.set(launch).is_err() {
        panic!("the launch is preset once, before the app starts");
    }
}

fn current_in(cell: &OnceLock<Launch>, home: Option<&Path>) -> Launch {
    cell.get().cloned().unwrap_or_else(|| production(home))
}

/// The packaged app's launch: the target saved under `home`, or the local
/// server. QA runs only through the command-line entry, which validates the
/// QA environment and presets it, so QA variables here are not read at all
/// rather than half-honored.
fn production(home: Option<&Path>) -> Launch {
    let saved = home.map_or(ConnectionTarget::Local, crate::target_store::load_target);
    let (settings, rejected_address) = match HostSettings::for_target(saved.clone(), None) {
        Ok(settings) => (settings, None),
        Err(_) => (
            HostSettings::for_target(ConnectionTarget::Local, None)
                .expect("the local target is a loopback HTTP origin"),
            Some(saved.panel_url()),
        ),
    };
    Launch {
        settings,
        qa: None,
        rejected_address,
    }
}

#[cfg(test)]
mod tests {
    use super::{current_in, preset_in, Launch};
    use crate::shell_host::HostSettings;
    use aleph_client::ConnectionTarget;
    use std::sync::OnceLock;

    /// An empty directory standing in for the user's home.
    fn scratch_home(name: &str) -> std::path::PathBuf {
        let home =
            std::env::temp_dir().join(format!("aleph-nsh-launch-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    #[test]
    fn current_without_preset_or_home_is_local() {
        let cell = OnceLock::new();
        let launch = current_in(&cell, None);
        assert!(launch.qa.is_none());
        assert_eq!(launch.settings.url, "http://127.0.0.1:18790/");
    }

    #[test]
    fn current_without_preset_reads_the_saved_target() {
        let home = scratch_home("saved");
        let remote = ConnectionTarget::parse("https://gw.example.com").unwrap();
        crate::target_store::save_target(&home, &remote).unwrap();
        let launch = current_in(&OnceLock::new(), Some(&home));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(launch.settings.url, "https://gw.example.com/");
    }

    #[test]
    fn saved_plain_http_remote_is_recoverable_in_the_connect_form() {
        let home = scratch_home("plain-http");
        let remote = ConnectionTarget::parse("http://box.lan:18790").unwrap();
        crate::target_store::save_target(&home, &remote).unwrap();
        let launch = current_in(&OnceLock::new(), Some(&home));
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(
            launch.rejected_address.as_deref(),
            Some("http://box.lan:18790/")
        );
        assert!(matches!(
            crate::connect_form::initial_view(&launch.settings.target, launch.rejected_address),
            crate::connect_form::View::Connect { back: None, .. }
        ));
    }

    #[test]
    fn preset_wins_over_environment() {
        let cell = OnceLock::new();
        let settings = HostSettings::from_launch(
            &["--connect".into(), "https://box.lan:8443".into()],
            None,
            None,
        )
        .unwrap();
        preset_in(
            &cell,
            Launch {
                settings,
                qa: None,
                rejected_address: None,
            },
        );
        let home = scratch_home("preset");
        crate::target_store::save_target(
            &home,
            &ConnectionTarget::parse("https://other.example.com").unwrap(),
        )
        .unwrap();
        let url = current_in(&cell, Some(&home)).settings.url;
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(url, "https://box.lan:8443/");
    }

    #[test]
    #[should_panic(expected = "preset once")]
    fn second_preset_panics() {
        let cell = OnceLock::new();
        let local = || Launch {
            rejected_address: None,
            settings: HostSettings::from_inputs(None, None).unwrap(),
            qa: None,
        };
        preset_in(&cell, local());
        preset_in(&cell, local());
    }
}
