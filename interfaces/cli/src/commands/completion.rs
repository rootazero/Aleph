//! Shell completion generation

use clap::CommandFactory;
use clap_complete::{generate, Shell};
use std::ffi::OsStr;
use std::io;
use std::path::Path;

use crate::Cli;

/// Generate shell completion script and print to stdout.
///
/// The script completes the name this process was invoked as: the package
/// installs the same program as `al` and `aleph`, and a script registered for
/// the other name completes nothing for the command the user actually types.
pub fn run(shell: Shell) {
    let mut cmd = Cli::command();
    let bin_name = completion_bin_name(std::env::args_os().next().as_deref(), cmd.get_name());
    generate(shell, &mut cmd, bin_name, &mut io::stdout());
}

/// The command name a completion script registers: the file name of
/// `argv[0]` without a Windows `.exe` suffix, or `fallback` when `argv[0]`
/// is absent, empty, or not UTF-8.
fn completion_bin_name(argv0: Option<&OsStr>, fallback: &str) -> String {
    argv0
        .and_then(|arg| Path::new(arg).file_name())
        .and_then(OsStr::to_str)
        .map(strip_exe_suffix)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

fn strip_exe_suffix(name: &str) -> &str {
    const EXE: &str = ".exe";
    let split = name.len().saturating_sub(EXE.len());
    match (name.get(..split), name.get(split..)) {
        (Some(stem), Some(ext)) if ext.eq_ignore_ascii_case(EXE) => stem,
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::completion_bin_name;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    fn name_for(argv0: &OsStr) -> String {
        completion_bin_name(Some(argv0), "aleph")
    }

    #[test]
    fn uses_the_file_name_of_an_invocation_path() {
        let path: PathBuf = ["usr", "local", "bin", "al"].iter().collect();
        assert_eq!(name_for(path.as_os_str()), "al");
    }

    #[test]
    fn a_bare_name_is_used_as_is() {
        assert_eq!(name_for(OsStr::new("al")), "al");
        assert_eq!(name_for(OsStr::new("aleph")), "aleph");
    }

    #[test]
    fn a_windows_exe_suffix_is_not_part_of_the_command_name() {
        let path: PathBuf = ["tools", "al.exe"].iter().collect();
        assert_eq!(name_for(path.as_os_str()), "al");
        assert_eq!(name_for(OsStr::new("ALEPH.EXE")), "ALEPH");
    }

    #[test]
    fn no_usable_argv0_falls_back() {
        assert_eq!(completion_bin_name(None, "aleph"), "aleph");
        assert_eq!(name_for(OsStr::new("")), "aleph");
        assert_eq!(name_for(OsStr::new(".exe")), "aleph");
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_argv0_falls_back() {
        use std::os::unix::ffi::OsStrExt;
        assert_eq!(name_for(OsStr::from_bytes(b"a\xffl")), "aleph");
    }
}
