//! `aleph-server service …` — register the standalone server to start on boot.
//!
//! All autostart logic lives here (single source of truth). The per-platform
//! ops shell out to the native supervisor; the descriptor *contents* are pure
//! and unit-tested in `descriptors`. Per-user only (no root): launchd
//! LaunchAgent, systemd --user, Task Scheduler logon task. The service runs the
//! foreground `aleph-server start`, supervised by the OS — never `--daemon`.

pub mod descriptors;

use crate::cli::ServiceAction;
use std::error::Error;
use std::path::PathBuf;
use std::process::Command;

type Res = Result<(), Box<dyn Error>>;

/// Sync dispatcher (called from the pre-runtime command match in `main.rs`).
pub fn handle_service_command(action: ServiceAction) -> Res {
    match action {
        ServiceAction::Install => platform::install(),
        ServiceAction::Uninstall => platform::uninstall(),
        ServiceAction::Enable => platform::enable(),
        ServiceAction::Disable => platform::disable(),
        ServiceAction::Status => platform::status(),
    }
}

fn exe_path() -> Result<PathBuf, Box<dyn Error>> {
    Ok(std::env::current_exe()?)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn home() -> Result<PathBuf, Box<dyn Error>> {
    dirs::home_dir().ok_or_else(|| "could not resolve home directory".into())
}

/// Run a command, returning an error if it exits non-zero. `ok_codes` lists exit
/// codes treated as success besides 0 (e.g. schtasks "not found" on uninstall).
fn run(cmd: &mut Command, ok_codes: &[i32]) -> Res {
    let status = cmd.status()?;
    if status.success()
        || status
            .code()
            .map(|c| ok_codes.contains(&c))
            .unwrap_or(false)
    {
        Ok(())
    } else {
        Err(format!("`{cmd:?}` failed: {status}").into())
    }
}

// ----------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod platform {
    use super::*;

    fn plist_path() -> Result<PathBuf, Box<dyn Error>> {
        Ok(home()?.join("Library/LaunchAgents/ai.aleph.server.plist"))
    }

    /// launchd service target for the modern enable/disable subcommands, which
    /// toggle the persistent boot-start state WITHOUT loading/unloading (i.e.
    /// without starting/stopping the running agent). Spec §4.1.
    fn service_target() -> Result<String, Box<dyn Error>> {
        let out = Command::new("id").arg("-u").output()?;
        let uid = String::from_utf8(out.stdout)?.trim().to_string();
        Ok(format!("gui/{uid}/ai.aleph.server"))
    }

    pub fn install() -> Res {
        let plist = plist_path()?;
        if let Some(parent) = plist.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(home()?.join(".aleph/logs"))?;
        std::fs::write(&plist, descriptors::launchd_plist(&exe_path()?, &home()?))?;
        // `load -w` arms RunAtLoad and starts it now.
        run(
            Command::new("launchctl").arg("load").arg("-w").arg(&plist),
            &[],
        )?;
        println!("Installed launchd agent ai.aleph.server (starts at login).");
        Ok(())
    }

    pub fn uninstall() -> Res {
        let plist = plist_path()?;
        let _ = run(
            Command::new("launchctl")
                .arg("unload")
                .arg("-w")
                .arg(&plist),
            &[],
        );
        let _ = std::fs::remove_file(&plist);
        println!("Removed launchd agent ai.aleph.server.");
        Ok(())
    }

    pub fn enable() -> Res {
        run(
            Command::new("launchctl")
                .arg("enable")
                .arg(service_target()?),
            &[],
        )
    }

    pub fn disable() -> Res {
        run(
            Command::new("launchctl")
                .arg("disable")
                .arg(service_target()?),
            &[],
        )
    }

    pub fn status() -> Res {
        let installed = plist_path()?.exists();
        println!("descriptor installed: {installed}");
        let _ = Command::new("launchctl")
            .arg("list")
            .arg("ai.aleph.server")
            .status();
        Ok(())
    }
}

// ----------------------------------------------------------------------------
#[cfg(target_os = "linux")]
mod platform {
    use super::*;

    fn unit_path() -> Result<PathBuf, Box<dyn Error>> {
        Ok(home()?.join(".config/systemd/user/aleph-server.service"))
    }

    fn systemctl(args: &[&str]) -> Res {
        run(Command::new("systemctl").arg("--user").args(args), &[])
    }

    pub fn install() -> Res {
        let unit = unit_path()?;
        if let Some(parent) = unit.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&unit, descriptors::systemd_unit(&exe_path()?))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", "aleph-server.service"])?;
        // Best-effort: linger lets the user service start at *boot* without a
        // login session (what a home server wants). May require polkit/root on
        // some distros — warn but don't fail the install.
        if let Ok(user) = std::env::var("USER") {
            if run(
                Command::new("loginctl").arg("enable-linger").arg(&user),
                &[],
            )
            .is_err()
            {
                eprintln!(
                    "note: could not enable linger; the server starts at login, not boot. \
                     Run `sudo loginctl enable-linger {user}` for boot autostart."
                );
            }
        }
        println!("Installed systemd user service aleph-server.service.");
        Ok(())
    }

    pub fn uninstall() -> Res {
        let _ = systemctl(&["disable", "--now", "aleph-server.service"]);
        let _ = std::fs::remove_file(unit_path()?);
        let _ = systemctl(&["daemon-reload"]);
        println!("Removed systemd user service aleph-server.service.");
        Ok(())
    }

    pub fn enable() -> Res {
        // Arm for boot only; do not start the running process (spec §4.1).
        systemctl(&["enable", "aleph-server.service"])
    }

    pub fn disable() -> Res {
        // Disarm boot only; do not stop the running process (spec §4.1).
        systemctl(&["disable", "aleph-server.service"])
    }

    pub fn status() -> Res {
        let installed = unit_path()?.exists();
        println!("descriptor installed: {installed}");
        let _ = Command::new("systemctl")
            .arg("--user")
            .arg("status")
            .arg("aleph-server.service")
            .status();
        Ok(())
    }
}

// ----------------------------------------------------------------------------
#[cfg(target_os = "windows")]
mod platform {
    use super::*;

    const TASK_NAME: &str = "Aleph\\aleph-server";

    fn launcher_path() -> Result<PathBuf, Box<dyn Error>> {
        // Place the vbs shim next to the installed exe (%LOCALAPPDATA%\Aleph).
        Ok(exe_path()?
            .parent()
            .ok_or("exe has no parent dir")?
            .join("aleph-server-hidden.vbs"))
    }

    pub fn install() -> Res {
        let launcher = launcher_path()?;
        std::fs::write(&launcher, descriptors::vbs_shim(&exe_path()?))?;
        // Write the task XML to a temp file and register it.
        let xml = descriptors::scheduled_task_xml(&launcher);
        let xml_path = std::env::temp_dir().join("aleph-server-task.xml");
        std::fs::write(&xml_path, xml)?;
        run(
            Command::new("schtasks")
                .args(["/Create", "/TN", TASK_NAME, "/XML"])
                .arg(&xml_path)
                .arg("/F"),
            &[],
        )?;
        let _ = std::fs::remove_file(&xml_path);
        // Start now (next logon would otherwise be the first run).
        let _ = run(
            Command::new("schtasks").args(["/Run", "/TN", TASK_NAME]),
            &[],
        );
        println!("Installed scheduled task {TASK_NAME} (starts at logon).");
        Ok(())
    }

    pub fn uninstall() -> Res {
        let _ = run(
            Command::new("schtasks").args(["/Delete", "/TN", TASK_NAME, "/F"]),
            &[1],
        );
        let _ = std::fs::remove_file(launcher_path()?);
        println!("Removed scheduled task {TASK_NAME}.");
        Ok(())
    }

    pub fn enable() -> Res {
        run(
            Command::new("schtasks").args(["/Change", "/TN", TASK_NAME, "/ENABLE"]),
            &[],
        )
    }

    pub fn disable() -> Res {
        run(
            Command::new("schtasks").args(["/Change", "/TN", TASK_NAME, "/DISABLE"]),
            &[],
        )
    }

    pub fn status() -> Res {
        let _ = Command::new("schtasks")
            .args(["/Query", "/TN", TASK_NAME])
            .status();
        Ok(())
    }
}
