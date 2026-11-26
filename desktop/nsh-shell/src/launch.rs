//! What one app process shows: the Gateway target and, in a debug QA run, the
//! isolated QA context. The command-line entry validates its arguments and
//! presets the launch before the app starts; the nsh runner starts the app
//! without a preset, so the app reads the production target itself.

use crate::shell_host::HostSettings;
use std::path::PathBuf;
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
}

static PRESET: OnceLock<Launch> = OnceLock::new();

/// Fixes this process's launch; call it once, before the app starts.
pub(crate) fn preset(launch: Launch) {
    preset_in(&PRESET, launch);
}

/// The preset launch, or the production one when nothing was preset.
pub(crate) fn current() -> Launch {
    current_in(&PRESET)
}

fn preset_in(cell: &OnceLock<Launch>, launch: Launch) {
    if cell.set(launch).is_err() {
        panic!("the launch is preset once, before the app starts");
    }
}

fn current_in(cell: &OnceLock<Launch>) -> Launch {
    cell.get().cloned().unwrap_or_else(production)
}

/// The packaged app's launch. QA runs only through the command-line entry,
/// which validates the QA environment and presets it, so QA variables here
/// are not read at all rather than half-honored.
fn production() -> Launch {
    Launch {
        settings: HostSettings::from_inputs(None, None)
            .expect("the local target is a loopback HTTP origin"),
        qa: None,
    }
}

#[cfg(test)]
mod tests {
    use super::{current_in, preset_in, Launch};
    use crate::shell_host::HostSettings;
    use std::sync::OnceLock;

    #[test]
    fn current_without_preset_reads_environment() {
        let cell = OnceLock::new();
        let launch = current_in(&cell);
        assert!(launch.qa.is_none());
        assert_eq!(launch.settings.url, "http://127.0.0.1:18790/");
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
        preset_in(&cell, Launch { settings, qa: None });
        assert_eq!(current_in(&cell).settings.url, "https://box.lan:8443/");
    }

    #[test]
    #[should_panic(expected = "preset once")]
    fn second_preset_panics() {
        let cell = OnceLock::new();
        let local = || Launch {
            settings: HostSettings::from_inputs(None, None).unwrap(),
            qa: None,
        };
        preset_in(&cell, local());
        preset_in(&cell, local());
    }
}
