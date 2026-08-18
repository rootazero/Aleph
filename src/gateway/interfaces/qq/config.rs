use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQConfig {
    pub accounts: Vec<QQAccountConfig>,
}

impl QQConfig {
    /// Normalise a raw `[channels.qq]` block into the one shape the channel
    /// consumes.
    ///
    /// `[channels.qq]` has two accepted spellings and exactly one meaning:
    ///
    /// * `accounts = [{ ... }]` — the original array form;
    /// * the account fields written flat on the section itself — the form the
    ///   Panel's channel card can produce, because the generic renderer patches
    ///   `channels.<id>` with a flat map of scalars and has no way to express a
    ///   repeated group of objects.
    ///
    /// Both land in `accounts`, so nothing downstream learns that a second
    /// spelling exists. This is the only place that decision is made — the
    /// factory is the sole path from config to `QQConfig`, and every test
    /// builds the struct directly.
    ///
    /// The array branch is kept because configs written before the card
    /// existed use it; it is not a second source of truth, it is a second
    /// spelling normalised here on arrival.
    pub fn from_wire(raw: serde_json::Value) -> Result<Self, String> {
        if raw.get("accounts").is_some() {
            return serde_json::from_value(raw).map_err(|e| format!("Invalid QQ config: {e}"));
        }
        let account: QQAccountConfig =
            serde_json::from_value(raw).map_err(|e| format!("Invalid QQ config: {e}"))?;
        Ok(Self {
            accounts: vec![account],
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QQAccountConfig {
    /// Label for this account in logs and access decisions.
    ///
    /// Defaulted rather than required: the flat spelling has no natural place
    /// to ask for it, and it names the account to a human rather than
    /// addressing it — nothing looks an account up by this id.
    #[serde(default = "default_account_id")]
    pub id: String,
    pub app_id: String,
    pub client_secret: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub allowed_users: Vec<String>,
    #[serde(default)]
    pub allowed_groups: Vec<String>,
    #[serde(default)]
    pub dm_policy: QQDmPolicy,
    #[serde(default)]
    pub group_policy: QQGroupPolicy,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QQDmPolicy {
    #[default]
    Allowlist,
    Open,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QQGroupPolicy {
    #[default]
    Allowlist,
    MentionOnly,
    Open,
}

const fn default_true() -> bool {
    true
}

fn default_account_id() -> String {
    "default".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_flat_spelling_becomes_one_account() {
        let cfg = QQConfig::from_wire(json!({
            "app_id": "102",
            "client_secret": "s3cr3t",
            "group_policy": "mention_only",
        }))
        .expect("flat spelling must parse");
        assert_eq!(cfg.accounts.len(), 1);
        assert_eq!(cfg.accounts[0].app_id, "102");
        assert_eq!(cfg.accounts[0].id, "default");
        assert_eq!(cfg.accounts[0].group_policy, QQGroupPolicy::MentionOnly);
        assert!(cfg.accounts[0].enabled, "enabled still defaults to true");
    }

    #[test]
    fn the_array_spelling_is_unchanged() {
        let cfg = QQConfig::from_wire(json!({
            "accounts": [
                { "id": "primary", "app_id": "102", "client_secret": "a" },
                { "id": "spare", "app_id": "103", "client_secret": "b" },
            ]
        }))
        .expect("array spelling must keep parsing");
        assert_eq!(cfg.accounts.len(), 2);
        assert_eq!(cfg.accounts[0].id, "primary");
        assert_eq!(cfg.accounts[1].id, "spare");
    }

    /// An empty `accounts = []` must stay an empty list rather than falling
    /// through to the flat branch, where it would fail with a message about a
    /// missing `app_id` and send the operator looking for the wrong mistake.
    #[test]
    fn an_empty_array_is_still_the_array_spelling() {
        let cfg = QQConfig::from_wire(json!({ "accounts": [] })).expect("must parse");
        assert!(cfg.accounts.is_empty());
    }

    /// The flat branch's error has to name the field that is missing. Routing
    /// a malformed array config through it would produce a message about
    /// `app_id` for a config that never claimed to have one.
    #[test]
    fn the_flat_branch_names_the_missing_field() {
        let err =
            QQConfig::from_wire(json!({ "client_secret": "s" })).expect_err("app_id is required");
        assert!(
            err.contains("app_id"),
            "error should name the missing field, got: {err}"
        );
    }
}
