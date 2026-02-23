//! Installer — converts `InstallSpec` values into executable shell commands
//! and filters specs by the current operating system.

use crate::domain::skill::{InstallKind, InstallSpec};
use crate::skill::eligibility::current_os;

/// Build a shell command string for the given install spec.
///
/// Returns `None` for `Download` specs that have no URL.
pub fn build_install_command(spec: &InstallSpec) -> Option<String> {
    match spec.kind {
        InstallKind::Brew => Some(format!("brew install {}", spec.package)),
        InstallKind::Apt => Some(format!("sudo apt-get install -y {}", spec.package)),
        InstallKind::Npm => Some(format!("npm install -g {}", spec.package)),
        InstallKind::Uv => Some(format!("uv pip install {}", spec.package)),
        InstallKind::Go => Some(format!("go install {}", spec.package)),
        InstallKind::Download => {
            spec.url.as_ref().map(|url| {
                format!("curl -fsSL -o {} {}", spec.package, url)
            })
        }
    }
}

/// Filter install specs to only those matching the current OS.
///
/// Specs with no OS restriction (os is `None`) are always included.
pub fn filter_install_specs_for_current_os(specs: &[InstallSpec]) -> Vec<&InstallSpec> {
    let current = current_os();
    specs
        .iter()
        .filter(|spec| {
            match &spec.os {
                None => true, // No OS restriction — always included
                Some(os_list) => os_list.contains(&current),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::skill::{InstallKind, InstallSpec, Os};

    fn make_spec(kind: InstallKind, package: &str) -> InstallSpec {
        InstallSpec {
            id: package.to_string(),
            kind,
            package: package.to_string(),
            bins: vec![],
            os: None,
            url: None,
        }
    }

    #[test]
    fn brew_command() {
        let spec = make_spec(InstallKind::Brew, "ripgrep");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "brew install ripgrep");
    }

    #[test]
    fn apt_command() {
        let spec = make_spec(InstallKind::Apt, "ripgrep");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "sudo apt-get install -y ripgrep");
    }

    #[test]
    fn npm_command() {
        let spec = make_spec(InstallKind::Npm, "prettier");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "npm install -g prettier");
    }

    #[test]
    fn uv_command() {
        let spec = make_spec(InstallKind::Uv, "black");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "uv pip install black");
    }

    #[test]
    fn go_command() {
        let spec = make_spec(InstallKind::Go, "github.com/golangci/golangci-lint@latest");
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "go install github.com/golangci/golangci-lint@latest");
    }

    #[test]
    fn download_command_with_url() {
        let spec = InstallSpec {
            id: "tool".to_string(),
            kind: InstallKind::Download,
            package: "/usr/local/bin/tool".to_string(),
            bins: vec!["tool".to_string()],
            os: None,
            url: Some("https://example.com/tool".to_string()),
        };
        let cmd = build_install_command(&spec).unwrap();
        assert_eq!(cmd, "curl -fsSL -o /usr/local/bin/tool https://example.com/tool");
    }

    #[test]
    fn download_command_without_url() {
        let spec = make_spec(InstallKind::Download, "tool");
        let cmd = build_install_command(&spec);
        assert!(cmd.is_none());
    }

    #[test]
    fn os_filter_excludes_wrong_platform() {
        let current = current_os();

        // Spec matching current OS
        let matching = InstallSpec {
            id: "matching".to_string(),
            kind: InstallKind::Brew,
            package: "matching-pkg".to_string(),
            bins: vec![],
            os: Some(vec![current.clone()]),
            url: None,
        };

        // Spec for a different OS
        let wrong_os = match current {
            Os::Darwin => Os::Windows,
            Os::Linux => Os::Windows,
            Os::Windows => Os::Darwin,
        };
        let non_matching = InstallSpec {
            id: "non-matching".to_string(),
            kind: InstallKind::Apt,
            package: "non-matching-pkg".to_string(),
            bins: vec![],
            os: Some(vec![wrong_os]),
            url: None,
        };

        // Spec with no OS restriction (always included)
        let universal = InstallSpec {
            id: "universal".to_string(),
            kind: InstallKind::Npm,
            package: "universal-pkg".to_string(),
            bins: vec![],
            os: None,
            url: None,
        };

        let specs = vec![matching, non_matching, universal];
        let filtered = filter_install_specs_for_current_os(&specs);

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].id, "matching");
        assert_eq!(filtered[1].id, "universal");
    }
}
