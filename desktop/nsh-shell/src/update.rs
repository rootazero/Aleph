//! Updates through the nsh toolchain's updater. `nsh package` writes the
//! package's `nsh-updater.json` from nsh.json's `"updater"`: where the signed
//! manifest is, and the key it is signed with. A newer version for this
//! platform is downloaded and checked against the manifest's hash, then
//! unpacked beside the app and verified (on macOS, its code signature); only
//! then is it swapped in, after the bundled server has stopped.

use aleph_client::ConnectionTarget;
use nsh_updater::install::{Installation, Staged};
use nsh_updater::{Available, Config, Download};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The id of the notification about an update; pressing it installs one
/// that was offered.
pub(crate) const NOTIFICATION: &str = "aleph-update";

const FIRST_CHECK: Duration = Duration::from_secs(90);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long to wait before the next scheduled check: the first comes 90 s
/// after launch, the later ones 6 h after the previous one. Both arguments
/// are measured from launch.
pub(crate) fn next_check_delay(since_launch: Duration, last_check: Option<Duration>) -> Duration {
    let due = match last_check {
        None => FIRST_CHECK,
        Some(at) => at + CHECK_INTERVAL,
    };
    due.saturating_sub(since_launch)
}

/// One step of installing a downloaded and verified update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstallStep {
    /// Stop the bundled server: it runs from inside the app being replaced,
    /// and the new app starts its own.
    StopLocalServer,
    /// Swap the new app in.
    Install,
    /// Quit and start the new app.
    Restart,
}

/// The steps for an app showing `target`. A remote server is someone
/// else's: the update never touches it.
pub(crate) fn install_plan(target: &ConnectionTarget) -> Vec<InstallStep> {
    match target {
        ConnectionTarget::Local => vec![
            InstallStep::StopLocalServer,
            InstallStep::Install,
            InstallStep::Restart,
        ],
        ConnectionTarget::Remote(_) => vec![InstallStep::Install, InstallStep::Restart],
    }
}

/// The installed, packaged app and where its updates come from.
#[derive(Clone, Debug)]
pub(crate) struct Updater {
    config: Config,
    installation: Installation,
}

impl Updater {
    /// The running app if it is a package `nsh package` made with an
    /// `"updater"`, installed where the user can replace it; `None` under
    /// `nsh run`, `cargo run` or a read-only install, which offer no update.
    pub(crate) fn current() -> Option<Self> {
        let executable = std::env::current_exe().ok()?;
        let bytes = std::fs::read(config_file(&executable)?).ok()?;
        let config = Config::parse(&bytes)
            .inspect_err(|error| eprintln!("update configuration ignored: {error}"))
            .ok()?;
        let installation = Installation::current(&config.id, &bytes)?;
        if !installation.can_install() {
            eprintln!("updates off: the app is installed where this user cannot replace it");
            return None;
        }
        // What an earlier update could not remove (Windows: the old
        // executable, in use until it exited).
        installation.remove_leftovers();
        Some(Self {
            config,
            installation,
        })
    }

    /// The running version.
    pub(crate) fn version(&self) -> &str {
        &self.config.version
    }

    /// The newer version the signed manifest offers this platform, if any.
    pub(crate) async fn check(&self) -> Result<Option<Available>, String> {
        let client = client()?;
        let manifest = fetch(&client, &self.config.manifest_url()).await?;
        let signature = match self.config.signature_url() {
            Some(url) => Some(fetch(&client, &url).await?),
            None => None,
        };
        self.config
            .check(&manifest, signature.as_deref(), &nsh_updater::platform())
            .map_err(|error| error.to_string())
    }

    /// Downloads `available`, checked against the manifest's hash, and
    /// unpacks it beside the app, verified. The installed app is untouched
    /// until [`Staged::finish`]; dropping the result removes what was
    /// unpacked.
    pub(crate) async fn prepare(&self, available: &Available) -> Result<Staged, String> {
        let sha256 = available
            .sha256
            .clone()
            .ok_or_else(|| format!("{} is a download page, not an archive", available.url))?;
        let directory = std::env::temp_dir().join(format!("aleph-update-{}", std::process::id()));
        let archive = download(&available.url, &sha256, available.size, &directory).await;
        let installation = self.installation.clone();
        let staged = tokio::task::spawn_blocking(move || {
            let staged = installation.stage(&archive?).map_err(|error| error.to_string());
            let _ = std::fs::remove_dir_all(&directory);
            staged
        })
        .await
        .map_err(|_| "the update could not be unpacked".to_owned())?;
        staged
    }
}

/// `nsh-updater.json` in the package's assets.
fn config_file(executable: &Path) -> Option<PathBuf> {
    let resources = if cfg!(target_os = "macos") {
        // <App>.app/Contents/MacOS/<exe> → <App>.app/Contents/Resources/nsh
        executable.parent()?.parent()?.join("Resources").join("nsh")
    } else {
        executable.parent()?.join("nsh")
    };
    Some(resources.join("assets").join(nsh_updater::CONFIG_ASSET))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .user_agent(concat!("Aleph-Desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| format!("could not set up HTTP: {error}"))
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let response = client
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("{url}: {error}"))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("{url}: {error}"))?;
    Ok(bytes.to_vec())
}

/// The archive's file name: the URL's last path segment, which must name a
/// `.tar.gz` or `.zip` (the installer goes by the extension).
fn archive_name(url: &str) -> Result<String, String> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or_default();
    let plain = !name.is_empty() && !name.contains(['\\', ':']) && !name.starts_with('.');
    if plain && (name.ends_with(".tar.gz") || name.ends_with(".zip")) {
        Ok(name.to_owned())
    } else {
        Err(format!("{url} does not name a .tar.gz or .zip archive"))
    }
}

/// Downloads `url` into `directory`, refusing anything but the manifest's
/// size and hash.
async fn download(
    url: &str,
    sha256: &str,
    size: Option<u64>,
    directory: &Path,
) -> Result<PathBuf, String> {
    let name = archive_name(url)?;
    let mut response = client()?
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| format!("{url}: {error}"))?;
    std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let path = directory.join(name);
    let mut file = std::fs::File::create(&path).map_err(|error| error.to_string())?;
    let mut check = Download::new(sha256, size);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("{url}: {error}"))?
    {
        check.update(&chunk).map_err(|error| error.to_string())?;
        file.write_all(&chunk).map_err(|error| error.to_string())?;
    }
    file.sync_all().map_err(|error| error.to_string())?;
    check.finish().map_err(|error| error.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_check_waits_90s() {
        assert_eq!(next_check_delay(Duration::ZERO, None), FIRST_CHECK);
        assert_eq!(
            next_check_delay(Duration::from_secs(30), None),
            Duration::from_secs(60)
        );
        assert_eq!(next_check_delay(Duration::from_secs(600), None), Duration::ZERO);
    }

    #[test]
    fn later_checks_every_6h() {
        let first = FIRST_CHECK;
        assert_eq!(next_check_delay(first, Some(first)), CHECK_INTERVAL);
        assert_eq!(
            next_check_delay(first + Duration::from_secs(3600), Some(first)),
            CHECK_INTERVAL - Duration::from_secs(3600)
        );
    }

    #[test]
    fn local_install_stops_server_first() {
        let plan = install_plan(&ConnectionTarget::Local);
        assert_eq!(
            plan,
            [
                InstallStep::StopLocalServer,
                InstallStep::Install,
                InstallStep::Restart
            ]
        );
    }

    #[test]
    fn remote_install_does_not_touch_server() {
        let target = ConnectionTarget::parse("https://gw.example.com").unwrap();
        let plan = install_plan(&target);
        assert!(!plan.contains(&InstallStep::StopLocalServer));
        assert_eq!(plan, [InstallStep::Install, InstallStep::Restart]);
    }

    #[test]
    fn archives_are_plain_file_names() {
        assert_eq!(
            archive_name("https://h/download/Aleph-26.9.2-macos-aarch64.tar.gz").unwrap(),
            "Aleph-26.9.2-macos-aarch64.tar.gz"
        );
        assert_eq!(
            archive_name("https://h/Aleph-26.9.2-windows-x86_64.zip?x=1").unwrap(),
            "Aleph-26.9.2-windows-x86_64.zip"
        );
        for bad in ["https://h/", "https://h/setup.exe", "https://h/.tar.gz", "https://h/a\\b.zip"] {
            assert!(archive_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_config_sits_in_the_package_assets() {
        let file = if cfg!(target_os = "macos") {
            config_file(Path::new("/A/Aleph.app/Contents/MacOS/Aleph")).unwrap()
        } else {
            config_file(Path::new("/A/Aleph/Aleph")).unwrap()
        };
        let expected = if cfg!(target_os = "macos") {
            Path::new("/A/Aleph.app/Contents/Resources/nsh/assets/nsh-updater.json")
        } else {
            Path::new("/A/Aleph/nsh/assets/nsh-updater.json")
        };
        assert_eq!(file, expected);
    }
}
