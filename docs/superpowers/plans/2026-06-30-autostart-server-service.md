# Standalone Server Autostart Service — Implementation Plan (Track B)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `aleph-server service install/uninstall/enable/disable/status`, registering the headless server to start on boot/login via each platform's native supervisor (launchd / systemd-user / Task Scheduler), and make `install.sh` / `install.ps1` enable it by default (opt out with `ALEPH_AUTOSTART=0`).

**Architecture:** A new `commands/service` module owns all autostart logic in one place (single source of truth, testable). Pure descriptor *generators* (plist / unit / task XML / vbs shim) are unit-tested; the `install/uninstall/enable/disable/status` operations shell out to `launchctl` / `systemctl` / `loginctl` / `schtasks` / `wscript` via `std::process::Command`. Services run the **foreground** `aleph-server start` (not `--daemon`) under the OS supervisor — this sidesteps the Windows "no `--daemon`" limitation and gives macOS a stable launchd identity (mitigating the ad-hoc-daemon local-network-privacy TCC issue).

**Tech Stack:** Rust (std only — `std::fs`, `std::process::Command`, `std::env::current_exe`), `clap` (existing CLI), `dirs` (home dir, already a dependency).

## Global Constraints

- **R1 (brain–limb):** no platform-API crate linkage. Shelling out to `launchctl`/`systemctl`/`schtasks` + writing descriptor files is process invocation, not native-API linkage, and lives in the `aleph-server` bin crate — not `alephcore`. ✔
- **R3 (core minimalism):** std-only, zero new dependencies.
- **Default ON for the standalone server** on all platforms; opt out with `ALEPH_AUTOSTART=0`. (App products are separate — Track A — and default OFF.)
- **Service runs foreground `aleph-server start`**, NOT `--daemon` (no double-fork; the supervisor owns lifecycle). On Windows a `.vbs` shim launches it windowless.
- **Per-user, not root:** launchd LaunchAgent (`~/Library/LaunchAgents/`), systemd `--user`, Task Scheduler logon task. No root/admin required.
- **Binary path:** resolve via `std::env::current_exe()` at install time.
- **Commit format:** `<scope>: <description>` (e.g. `aleph-server: add service subcommand`).
- **Cargo discipline:** TDD the pure generators with `cargo test -p alephcore --bin aleph-server service` (confirm the bin/package wiring; the binary is `src/bin/aleph-server`). Avoid full builds; compile-check with `cargo check` once per task at most.
- **Singleton note:** the flock (`~/.aleph/data/aleph.lock`) already prevents a second server; if both the full App and the standalone service are installed, the later starter fails to acquire the lock (documented edge, not handled here).

---

### Task B1: `aleph-server service` subcommand + descriptor generators

**Files:**
- Create: `src/bin/aleph-server/commands/service/mod.rs`
- Create: `src/bin/aleph-server/commands/service/descriptors.rs`
- Modify: `src/bin/aleph-server/commands/mod.rs` (register `pub mod service;` + re-export)
- Modify: `src/bin/aleph-server/cli.rs` (add `Command::Service` + `ServiceAction`)
- Modify: `src/bin/aleph-server/main.rs` (dispatch in the sync section)

**Interfaces:**
- Produces: `pub fn handle_service_command(action: crate::cli::ServiceAction) -> Result<(), Box<dyn std::error::Error>>`.
- Produces (pure, tested): `descriptors::launchd_plist(exe: &Path, home: &Path) -> String`, `descriptors::systemd_unit(exe: &Path) -> String`, `descriptors::scheduled_task_xml(launcher: &Path) -> String`, `descriptors::vbs_shim(exe: &Path) -> String`.

- [ ] **Step 1: Write failing tests for the descriptor generators**

Create `src/bin/aleph-server/commands/service/descriptors.rs` with ONLY the tests first:

```rust
//! Pure generators for the per-platform service descriptors. Kept separate from
//! the shell-out operations so the file contents are unit-testable without
//! touching launchctl / systemd / schtasks.

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn launchd_plist_runs_start_at_load_and_keepalive() {
        let p = launchd_plist(Path::new("/Users/x/.local/bin/aleph-server"), Path::new("/Users/x"));
        assert!(p.contains("<string>ai.aleph.server</string>"));
        assert!(p.contains("<string>/Users/x/.local/bin/aleph-server</string>"));
        assert!(p.contains("<string>start</string>"));
        assert!(p.contains("<key>RunAtLoad</key>"));
        assert!(p.contains("<key>KeepAlive</key>"));
        assert!(p.contains("/Users/x/.aleph/logs/launchd.err.log"));
    }

    #[test]
    fn systemd_unit_is_simple_foreground_and_restarts() {
        let u = systemd_unit(Path::new("/home/x/.local/bin/aleph-server"));
        assert!(u.contains("Type=simple"));
        assert!(u.contains("ExecStart=/home/x/.local/bin/aleph-server start"));
        assert!(u.contains("Restart=on-failure"));
        assert!(u.contains("WantedBy=default.target"));
        // Must NOT daemonize itself — the supervisor owns the process.
        assert!(!u.contains("--daemon"));
        assert!(!u.contains(" -d"));
    }

    #[test]
    fn scheduled_task_xml_has_logon_trigger_and_launcher() {
        let x = scheduled_task_xml(&PathBuf::from(r"C:\Users\x\AppData\Local\Aleph\aleph-server-hidden.vbs"));
        assert!(x.contains("<LogonTrigger>"));
        assert!(x.contains("aleph-server-hidden.vbs"));
        assert!(x.contains("wscript.exe"));
        assert!(x.contains("<Enabled>true</Enabled>"));
    }

    #[test]
    fn vbs_shim_launches_start_hidden() {
        let v = vbs_shim(Path::new(r"C:\Users\x\AppData\Local\Aleph\aleph-server.exe"));
        // 0 = hidden window, False = don't wait.
        assert!(v.contains(r#""C:\Users\x\AppData\Local\Aleph\aleph-server.exe" start"#));
        assert!(v.contains(", 0, False"));
    }
}
```

- [ ] **Step 2: Run the tests — verify they fail**

Run: `cargo test -p alephcore --bin aleph-server descriptors`
Expected: FAIL to compile — `cannot find function launchd_plist` (and the other three).

- [ ] **Step 3: Implement the generators**

Add above the `#[cfg(test)]` module in `descriptors.rs`:

```rust
/// macOS LaunchAgent plist. `RunAtLoad` + `KeepAlive` → starts at login and is
/// resurrected if it exits. Runs the foreground `start` (no `--daemon`).
pub fn launchd_plist(exe: &Path, home: &Path) -> String {
    let exe = exe.display();
    let home = home.display();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>ai.aleph.server</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{home}/.aleph/logs/launchd.out.log</string>
    <key>StandardErrorPath</key>
    <string>{home}/.aleph/logs/launchd.err.log</string>
</dict>
</plist>
"#
    )
}

/// systemd *user* unit. `Type=simple` foreground process the user manager keeps
/// alive; `WantedBy=default.target` so `enable` arms it for login (boot too,
/// once linger is enabled — see the install op).
pub fn systemd_unit(exe: &Path) -> String {
    let exe = exe.display();
    format!(
        "[Unit]\n\
         Description=Aleph Server\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={exe} start\n\
         Restart=on-failure\n\
         RestartSec=3\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n"
    )
}

/// Task Scheduler task (logon trigger). Runs `wscript.exe <launcher.vbs>` so the
/// console window stays hidden (the vbs shim launches the server with window
/// style 0). `InteractiveToken` → runs in the user session, no stored password.
pub fn scheduled_task_xml(launcher: &Path) -> String {
    let launcher = launcher.display();
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Aleph Server</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Enabled>true</Enabled>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>wscript.exe</Command>
      <Arguments>//B //Nologo "{launcher}"</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// VBScript shim: launch `<exe> start` with window style 0 (hidden) and don't
/// wait. The standard windowless-console-launch trick on Windows.
pub fn vbs_shim(exe: &Path) -> String {
    let exe = exe.display();
    format!(
        "Set s = CreateObject(\"WScript.Shell\")\r\n\
         s.Run \"\"\"{exe}\"\" start\", 0, False\r\n"
    )
}
```

- [ ] **Step 4: Run the tests — verify they pass**

Run: `cargo test -p alephcore --bin aleph-server descriptors`
Expected: 4 tests PASS.

- [ ] **Step 5: Add the CLI surface**

In `src/bin/aleph-server/cli.rs`, add a variant to `enum Command` (after the `Gateway { … }` variant, `:121`):

```rust
    /// Manage start-on-boot for the standalone server (launchd / systemd / Task Scheduler)
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
```

And add the action enum near the other `*Action` enums (e.g. after `GatewayAction`, `:265`):

```rust
/// Service (start-on-boot) subcommands
#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Write the platform service descriptor, enable it, and start now
    Install,
    /// Stop and remove the service descriptor
    Uninstall,
    /// Arm the service to start on boot/login (no descriptor changes)
    Enable,
    /// Stop the service from starting on boot/login
    Disable,
    /// Report installed / enabled / running state
    Status,
}
```

- [ ] **Step 6: Implement the module operations + dispatcher**

Create `src/bin/aleph-server/commands/service/mod.rs`:

```rust
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

fn home() -> Result<PathBuf, Box<dyn Error>> {
    dirs::home_dir().ok_or_else(|| "could not resolve home directory".into())
}

/// Run a command, returning an error if it exits non-zero. `ok_codes` lists exit
/// codes treated as success besides 0 (e.g. schtasks "not found" on uninstall).
fn run(cmd: &mut Command, ok_codes: &[i32]) -> Res {
    let status = cmd.status()?;
    if status.success() || status.code().map(|c| ok_codes.contains(&c)).unwrap_or(false) {
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

    pub fn install() -> Res {
        let plist = plist_path()?;
        if let Some(parent) = plist.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(home()?.join(".aleph/logs"))?;
        std::fs::write(&plist, descriptors::launchd_plist(&exe_path()?, &home()?))?;
        // `load -w` arms RunAtLoad and starts it now.
        run(Command::new("launchctl").arg("load").arg("-w").arg(&plist), &[])?;
        println!("Installed launchd agent ai.aleph.server (starts at login).");
        Ok(())
    }

    pub fn uninstall() -> Res {
        let plist = plist_path()?;
        let _ = run(Command::new("launchctl").arg("unload").arg("-w").arg(&plist), &[]);
        let _ = std::fs::remove_file(&plist);
        println!("Removed launchd agent ai.aleph.server.");
        Ok(())
    }

    pub fn enable() -> Res {
        run(Command::new("launchctl").arg("load").arg("-w").arg(plist_path()?), &[])
    }

    pub fn disable() -> Res {
        run(Command::new("launchctl").arg("unload").arg("-w").arg(plist_path()?), &[])
    }

    pub fn status() -> Res {
        let installed = plist_path()?.exists();
        println!("descriptor installed: {installed}");
        let _ = Command::new("launchctl").arg("list").arg("ai.aleph.server").status();
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
            if run(Command::new("loginctl").arg("enable-linger").arg(&user), &[]).is_err() {
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
        systemctl(&["enable", "--now", "aleph-server.service"])
    }

    pub fn disable() -> Res {
        systemctl(&["disable", "--now", "aleph-server.service"])
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
        let _ = run(Command::new("schtasks").args(["/Run", "/TN", TASK_NAME]), &[]);
        println!("Installed scheduled task {TASK_NAME} (starts at logon).");
        Ok(())
    }

    pub fn uninstall() -> Res {
        let _ = run(Command::new("schtasks").args(["/Delete", "/TN", TASK_NAME, "/F"]), &[1]);
        let _ = std::fs::remove_file(launcher_path()?);
        println!("Removed scheduled task {TASK_NAME}.");
        Ok(())
    }

    pub fn enable() -> Res {
        run(Command::new("schtasks").args(["/Change", "/TN", TASK_NAME, "/ENABLE"]), &[])
    }

    pub fn disable() -> Res {
        run(Command::new("schtasks").args(["/Change", "/TN", TASK_NAME, "/DISABLE"]), &[])
    }

    pub fn status() -> Res {
        let _ = Command::new("schtasks").args(["/Query", "/TN", TASK_NAME]).status();
        Ok(())
    }
}
```

- [ ] **Step 7: Register the module + dispatch the command**

In `src/bin/aleph-server/commands/mod.rs`, add to the `pub mod` list:

```rust
pub mod service;
```

and to the re-exports:

```rust
pub use service::handle_service_command;
```

In `src/bin/aleph-server/main.rs`, add to the synchronous command match (the block that returns before `rt.block_on`, alongside `Some(Command::Secret { action }) => …` at `:127`):

```rust
        Some(Command::Service { action }) => return commands::handle_service_command(action),
```

- [ ] **Step 8: Compile + targeted test gate**

Run: `cargo test -p alephcore --bin aleph-server descriptors`
Expected: 4 generator tests PASS, binary compiles for the host platform.

- [ ] **Step 9: Commit**

```bash
git add src/bin/aleph-server/commands/service/ \
        src/bin/aleph-server/commands/mod.rs \
        src/bin/aleph-server/cli.rs \
        src/bin/aleph-server/main.rs
git commit -m "aleph-server: add service subcommand for start-on-boot"
```

- [ ] **Step 10: Operator verification (per platform, after build)**

Shell-out ops cannot be unit-tested (they mutate the real supervisor). Operator runs on each target: `aleph-server service install` → reboot/relogin → `aleph-server service status` shows running → `aleph-server service uninstall` cleans up. macOS: confirm no local-network-privacy denial under the launchd identity. Windows: confirm no visible console window.

---

### Task B2: Install scripts enable autostart by default

**Files:**
- Modify: `Scripts/install.sh` (call `service install` after the atomic `mv`, unless `ALEPH_AUTOSTART=0` or running as root)
- Modify: `Scripts/install.ps1` (call `service install` after download, unless `ALEPH_AUTOSTART=0`)

**Interfaces:**
- Consumes: `aleph-server service install` (Task B1).

- [ ] **Step 1: install.sh — enable autostart by default**

In `Scripts/install.sh`, replace the final two echo lines (`:47-48`):

```bash
echo "Installed. Start it with:  aleph-server start"
echo "LAN access: set [gateway] host = \"0.0.0.0\" in ~/.aleph/config.toml (trusts your whole LAN)."
```

with:

```bash
echo "Installed."
# Enable start-on-boot by default (per-user service). Opt out with ALEPH_AUTOSTART=0.
# Skip when running as root: the service is per-user and would otherwise register
# for root rather than the human user.
if [ "${ALEPH_AUTOSTART:-1}" = "1" ] && [ "$(id -u)" != "0" ]; then
  echo "Enabling start-on-boot (set ALEPH_AUTOSTART=0 to skip)…"
  if ! "$dest_dir/aleph-server" service install; then
    echo "  Could not enable autostart; run '$dest_dir/aleph-server service install' later."
  fi
else
  echo "Start it with:  aleph-server start"
  [ "$(id -u)" = "0" ] && echo "  (running as root: re-run 'aleph-server service install' as your normal user for boot autostart)"
fi
echo "LAN access: set [gateway] host = \"0.0.0.0\" in ~/.aleph/config.toml (trusts your whole LAN)."
```

- [ ] **Step 2: install.ps1 — enable autostart by default**

In `Scripts/install.ps1`, replace the final two lines (`:50-51`):

```powershell
Write-Host "Installed. Start it with:  aleph-server start"
Write-Host 'LAN access: set [gateway] host = "0.0.0.0" in ~/.aleph/config.toml (trusts your whole LAN).'
```

with:

```powershell
Write-Host "Installed."
# Enable start-on-boot by default (per-user logon task). Opt out with ALEPH_AUTOSTART=0.
if ($env:ALEPH_AUTOSTART -ne '0') {
    Write-Host "Enabling start-on-boot (set ALEPH_AUTOSTART=0 to skip)…"
    try { & $destExe service install }
    catch { Write-Warning "Could not enable autostart: $_. Run '$destExe service install' later." }
} else {
    Write-Host "Start it with:  aleph-server start"
}
Write-Host 'LAN access: set [gateway] host = "0.0.0.0" in ~/.aleph/config.toml (trusts your whole LAN).'
```

- [ ] **Step 3: Structural validation**

Run: `bash -n Scripts/install.sh`
Expected: no syntax errors (exit 0).

Run: `pwsh -NoProfile -Command "$null = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path Scripts/install.ps1), [ref]$null, [ref]$null); 'ok'"` (or, if pwsh is unavailable, visually confirm the `if/try/catch` braces balance).
Expected: prints `ok` / parses without error.

- [ ] **Step 4: Commit**

```bash
git add Scripts/install.sh Scripts/install.ps1
git commit -m "installer: enable server start-on-boot by default (ALEPH_AUTOSTART=0 to skip)"
```

- [ ] **Step 5: Operator verification**

On a real install: piping `install.sh` (non-root) registers the service and `aleph-server service status` shows it running; `ALEPH_AUTOSTART=0 bash install.sh` skips it. Same for `install.ps1` on Windows.

---

## Self-Review

**Spec coverage:** §4.1 `service install/uninstall/enable/disable/status` → B1 Steps 5-7; foreground `start` not `--daemon` → systemd_unit/launchd_plist assert no `--daemon` (B1 Step 1) + ExecStart `start`; §4.2 install-script default-on + `ALEPH_AUTOSTART` opt-out → B2; §5 platform descriptors (launchd `ai.aleph.server` / systemd user + linger / Task Scheduler logon) → B1 Steps 3 & 6; §6 flock edge noted in Global Constraints. ✔
**Placeholder scan:** none — full code in every code step; exact `schtasks`/`systemctl`/`launchctl` invocations.
**Type consistency:** `handle_service_command(ServiceAction)` defined B1 Step 6, registered B1 Step 7, dispatched B1 Step 7; `ServiceAction` variants (Install/Uninstall/Enable/Disable/Status) defined B1 Step 5 match the dispatcher arms. Generator names (`launchd_plist`/`systemd_unit`/`scheduled_task_xml`/`vbs_shim`) identical across tests (B1 Step 1), impls (B1 Step 3), and platform call sites (B1 Step 6). ✔
**Decision recorded:** Linux uses systemd-user + best-effort `enable-linger` (boot autostart) with a clear fallback message when linger needs root — matches the approved sub-decision (b).
