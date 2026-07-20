//! Pure generators for the per-platform service descriptors. Kept separate from
//! the shell-out operations so the file contents are unit-testable without
//! touching launchctl / systemd / schtasks.

use std::path::Path;

/// macOS LaunchAgent plist. `RunAtLoad` + `KeepAlive` → starts at login and is
/// resurrected if it exits. Runs the foreground `start` (no `--daemon`).
#[allow(dead_code)] // cross-platform descriptor set: each generator is used in production on one OS and tested on all
pub fn launchd_plist(exe: &Path, home: &Path) -> String {
    debug_assert!(
        !exe.to_string_lossy().contains('{') && !exe.to_string_lossy().contains('}')
    );
    debug_assert!(
        !home.to_string_lossy().contains('{') && !home.to_string_lossy().contains('}')
    );
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
#[allow(dead_code)] // cross-platform descriptor set: each generator is used in production on one OS and tested on all
pub fn systemd_unit(exe: &Path) -> String {
    debug_assert!(
        !exe.to_string_lossy().contains('{') && !exe.to_string_lossy().contains('}')
    );
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
#[allow(dead_code)] // cross-platform descriptor set: each generator is used in production on one OS and tested on all
pub fn scheduled_task_xml(launcher: &Path) -> String {
    debug_assert!(
        !launcher.to_string_lossy().contains('{')
            && !launcher.to_string_lossy().contains('}')
    );
    let launcher = launcher.display();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
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
#[allow(dead_code)] // cross-platform descriptor set: each generator is used in production on one OS and tested on all
pub fn vbs_shim(exe: &Path) -> String {
    debug_assert!(
        !exe.to_string_lossy().contains('{') && !exe.to_string_lossy().contains('}')
    );
    let exe = exe.display();
    format!(
        "Set s = CreateObject(\"WScript.Shell\")\r\n\
         s.Run \"\"\"{exe}\"\" start\", 0, False\r\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn launchd_plist_runs_start_at_load_and_keepalive() {
        let p = launchd_plist(
            Path::new("/Users/x/.local/bin/aleph-server"),
            Path::new("/Users/x"),
        );
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
        let x = scheduled_task_xml(&PathBuf::from(
            r"C:\Users\x\AppData\Local\Aleph\aleph-server-hidden.vbs",
        ));
        assert!(x.contains("<LogonTrigger>"));
        assert!(x.contains("aleph-server-hidden.vbs"));
        assert!(x.contains("wscript.exe"));
        assert!(x.contains("<Enabled>true</Enabled>"));
    }

    #[test]
    fn vbs_shim_launches_start_hidden() {
        let v = vbs_shim(Path::new(
            r"C:\Users\x\AppData\Local\Aleph\aleph-server.exe",
        ));
        // VBScript doubles the literal quotes around the exe path, so the .vbs source
        // reads:  s.Run """<exe>"" start", 0, False  — assert the escaped form.
        assert!(v.contains(r#"aleph-server.exe"" start"#));
        assert!(v.contains(", 0, False"));
    }

    #[test]
    fn launchd_plist_rejects_braces_in_paths() {
        for exe in ["/Users/x/{aleph-server", "/Users/x/aleph-server}"] {
            assert!(std::panic::catch_unwind(|| {
                launchd_plist(Path::new(exe), Path::new("/Users/x"))
            })
            .is_err());
        }
        for home in ["/Users/{x", "/Users/x}"] {
            assert!(std::panic::catch_unwind(|| {
                launchd_plist(Path::new("/usr/local/bin/aleph-server"), Path::new(home))
            })
            .is_err());
        }
    }

    #[test]
    fn systemd_unit_rejects_braces_in_exe_path() {
        for exe in ["/home/x/{aleph-server", "/home/x/aleph-server}"] {
            assert!(std::panic::catch_unwind(|| systemd_unit(Path::new(exe))).is_err());
        }
    }

    #[test]
    fn scheduled_task_xml_rejects_braces_in_launcher_path() {
        for launcher in [
            r"C:\Users\x\{aleph-server-hidden.vbs",
            r"C:\Users\x\aleph-server-hidden.vbs}",
        ] {
            assert!(
                std::panic::catch_unwind(|| scheduled_task_xml(Path::new(launcher))).is_err()
            );
        }
    }

    #[test]
    fn vbs_shim_rejects_braces_in_exe_path() {
        for exe in [
            r"C:\Users\x\{aleph-server.exe",
            r"C:\Users\x\aleph-server.exe}",
        ] {
            assert!(std::panic::catch_unwind(|| vbs_shim(Path::new(exe))).is_err());
        }
    }
}
