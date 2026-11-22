//! The native shell hosts the canonical Panel; it owns no chat projection.
pub(crate) const APP_ID: &str = "com.aleph.nsh-shell";

pub(crate) fn valid_qa_nonce(nonce: &str) -> bool {
    nonce.len() == 64
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Clone, Debug)]
pub(crate) struct HostSettings {
    pub(crate) url: String,
    pub(crate) gateway: String,
    pub(crate) qa_nonce: Option<String>,
    #[cfg(feature = "embedded-core")]
    pub(crate) acquires_local_server: bool,
}

impl HostSettings {
    pub(crate) fn from_launch(
        args: &[String],
        url: Option<&str>,
        nonce: Option<&str>,
    ) -> Result<Self, String> {
        if args.is_empty() {
            return Self::from_inputs(url, nonce);
        }
        if url.is_some() || nonce.is_some() {
            return Err("Production target and QA configuration cannot be combined".into());
        }
        let [flag, raw] = args else {
            return Err("Expected --connect <origin> or --help".into());
        };
        if flag != "--connect"
            || raw.is_empty()
            || raw.len() > 2048
            || raw.chars().any(|c| c.is_ascii_control() || c == '\\')
        {
            return Err("Expected one bounded --connect origin".into());
        }
        let target = aleph_client::ConnectionTarget::parse(raw)
            .map_err(|_| "Invalid connection target".to_string())?;
        if let aleph_client::ConnectionTarget::Remote(url) = &target {
            if !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || url.path() != "/"
            {
                return Err("Connection target must be a root origin without credentials".into());
            }
        }
        Self::for_target(target, None)
    }

    fn for_target(
        target: aleph_client::ConnectionTarget,
        qa_nonce: Option<String>,
    ) -> Result<Self, String> {
        let gateway = target
            .credential_gateway_url()
            .map_err(|_| "Native credentials require HTTPS or loopback HTTP".to_string())?;
        Ok(Self {
            url: target.panel_url(),
            gateway,
            #[cfg(feature = "embedded-core")]
            acquires_local_server: qa_nonce.is_none()
                && matches!(target, aleph_client::ConnectionTarget::Local),
            qa_nonce,
        })
    }

    pub(crate) fn from_inputs(url: Option<&str>, nonce: Option<&str>) -> Result<Self, String> {
        let (url, nonce) = match (url, nonce) {
            (None, None) => return Self::for_target(aleph_client::ConnectionTarget::Local, None),
            (Some(url), Some(nonce)) if cfg!(debug_assertions) => (url, nonce),
            _ => return Err("QA endpoint and nonce must be paired in a debug build".into()),
        };
        let port = url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|value| value.strip_suffix('/'));
        if !port.is_some_and(|value| {
            !value.is_empty()
                && value.bytes().all(|byte| byte.is_ascii_digit())
                && value.parse::<u16>().is_ok_and(|port| port > 0)
        }) {
            return Err(
                "QA endpoint must be a loopback HTTP origin with an explicit nonzero port".into(),
            );
        }
        if !valid_qa_nonce(nonce) {
            return Err("QA nonce must be 32 bytes of lowercase hex".into());
        }
        Self::for_target(
            aleph_client::ConnectionTarget::parse(url)
                .map_err(|_| "Invalid QA loopback origin".to_string())?,
            Some(nonce.into()),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::HostSettings;

    #[test]
    fn production_launch_uses_a_sanitized_immutable_origin_without_qa() {
        for (input, expected) in [
            ("local", "http://127.0.0.1:18790/"),
            ("https://box.lan:8443", "https://box.lan:8443/"),
            ("https://box.lan", "https://box.lan/"),
            ("http://localhost:9876", "http://localhost:9876/"),
            ("http://[::1]:9876", "http://[::1]:9876/"),
        ] {
            let settings =
                HostSettings::from_launch(&["--connect".into(), input.into()], None, None).unwrap();
            assert_eq!(settings.url, expected);
            assert!(settings.qa_nonce.is_none());
        }
        assert_eq!(
            HostSettings::from_launch(&[], None, None).unwrap().url,
            "http://127.0.0.1:18790/"
        );
    }

    #[cfg(feature = "embedded-core")]
    #[test]
    fn only_canonical_local_target_acquires_bundled_service() {
        assert!(
            HostSettings::from_inputs(None, None)
                .unwrap()
                .acquires_local_server
        );
        for (name, acquires) in [
            ("local", true),
            ("http://127.0.0.1:18790/", false),
            ("https://box.lan", false),
        ] {
            let args = ["--connect".into(), name.into()];
            assert_eq!(
                HostSettings::from_launch(&args, None, None)
                    .unwrap()
                    .acquires_local_server,
                acquires
            );
        }
        if cfg!(debug_assertions) {
            let nonce = "a".repeat(64);
            assert!(
                !HostSettings::from_inputs(Some("http://127.0.0.1:18790/"), Some(&nonce))
                    .unwrap()
                    .acquires_local_server
            );
        }
    }

    #[test]
    fn launch_rejection_is_secret_free_and_never_selects_a_qa_or_plaintext_target() {
        for raw in [
            "http://box.lan",
            "box.lan",
            "https://u:synthetic-password@box.lan",
            "https://box.lan/?bt=synthetic-ticket",
            "https://box.lan/#synthetic-fragment",
            "https://box.lan/path",
            "https://box.lan/\\n",
            "",
            "file:///tmp/private",
        ] {
            let error = HostSettings::from_launch(&["--connect".into(), raw.into()], None, None)
                .unwrap_err();
            assert!(!error.contains("synthetic"));
            assert!(!error.contains("box.lan"));
            assert!(!error.contains("private"));
        }
        for args in [
            vec!["--unknown".into()],
            vec!["--connect".into()],
            vec!["--connect".into(), "local".into(), "local".into()],
        ] {
            assert!(HostSettings::from_launch(&args, None, None).is_err());
        }
        let args = ["--connect".into(), "https://box.lan".into()];
        let nonce = "a1".repeat(32);
        for (url, nonce) in [
            (Some("http://127.0.0.1:29871/"), None),
            (None, Some(nonce.as_str())),
            (Some("http://127.0.0.1:29871/"), Some(nonce.as_str())),
        ] {
            assert!(HostSettings::from_launch(&args, url, nonce).is_err());
        }
        let qa =
            HostSettings::from_launch(&[], Some("http://127.0.0.1:29871/"), Some(&nonce)).unwrap();
        assert_eq!(qa.qa_nonce.as_deref(), Some(nonce.as_str()));
    }

    #[test]
    fn default_host_uses_the_canonical_panel_root() {
        let settings = HostSettings::from_inputs(None, None).unwrap();
        assert_eq!(settings.url, "http://127.0.0.1:18790/");
        assert!(settings.qa_nonce.is_none());
    }

    #[test]
    fn debug_host_accepts_an_owned_loopback_endpoint_with_nonce() {
        let nonce = "a1".repeat(32);
        let settings =
            HostSettings::from_inputs(Some("http://127.0.0.1:29871/"), Some(&nonce)).unwrap();
        assert_eq!(settings.url, "http://127.0.0.1:29871/");
        assert_eq!(settings.qa_nonce.as_deref(), Some(nonce.as_str()));
    }

    #[test]
    fn qa_host_rejects_external_origins_credentials_paths_and_missing_nonce() {
        let nonce = "a1".repeat(32);
        for url in [
            "https://example.com/",
            "http://127.0.0.1:0/",
            "http://user@127.0.0.1:29871/",
            "http://127.0.0.1:29871/other",
            "http://127.0.0.1:29871/?token=secret",
        ] {
            assert!(
                HostSettings::from_inputs(Some(url), Some(&nonce)).is_err(),
                "{url}"
            );
        }
        assert!(HostSettings::from_inputs(Some("http://127.0.0.1:29871/"), None).is_err());
        assert!(HostSettings::from_inputs(None, Some(&nonce)).is_err());
    }

    #[test]
    fn qa_nonce_is_exactly_32_bytes_of_lowercase_hex() {
        for nonce in [
            "a".repeat(63),
            "A".repeat(64),
            "g".repeat(64),
            "a".repeat(65),
        ] {
            assert!(
                HostSettings::from_inputs(Some("http://127.0.0.1:29871/"), Some(&nonce)).is_err()
            );
        }
    }
}
