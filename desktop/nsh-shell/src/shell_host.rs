//! The native shell hosts the canonical Panel; it owns no chat projection.

#[derive(Clone, Debug)]
pub(crate) struct HostSettings {
    pub(crate) url: String,
    pub(crate) qa_nonce: Option<String>,
}

impl HostSettings {
    pub(crate) fn from_inputs(url: Option<&str>, nonce: Option<&str>) -> Result<Self, String> {
        let (url, nonce) = match (url, nonce) {
            (None, None) => {
                return Ok(Self {
                    url: "http://127.0.0.1:18790/".into(),
                    qa_nonce: None,
                })
            }
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
        if nonce.len() != 64
            || !nonce
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("QA nonce must be 32 bytes of lowercase hex".into());
        }
        Ok(Self {
            url: url.into(),
            qa_nonce: Some(nonce.into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HostSettings;

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
