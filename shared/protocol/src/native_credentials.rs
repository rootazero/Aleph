//! Native protected-credential Bridge wire contract (Task 41).
//!
//! The desktop shell exposes exactly three fixed commands to the Panel:
//!
//! | command                  | args                                  | returns                 |
//! |--------------------------|---------------------------------------|-------------------------|
//! | `panel_credential_load`  | none                                  | [`PanelCredentialLease`] |
//! | `panel_credential_save`  | `lease_id: u64`, `credential`         | `()`                    |
//! | `panel_credential_delete`| `lease_id: u64`                       | `bool`                  |
//!
//! The command names are fixed here ([`COMMAND_LOAD`] / [`COMMAND_SAVE`] /
//! [`COMMAND_DELETE`]) so the shell registration and the Panel caller share one
//! spelling. None of these types carries an account, User, role, ticket or
//! viewer claim: the keyring account is derived by the shell from its own
//! native configuration, the Gateway remains the only authority for User and
//! resource authorization, and a one-shot bootstrap ticket is never persisted.
//!
//! These are **wire** DTOs only. The client's `DeviceCredential` keeps its
//! secret private; the store hands this narrow connect-only projection across
//! the Bridge and nowhere else. Both types decode strictly
//! (`deny_unknown_fields`, no blank id/token, nonzero lease) and `Debug` never
//! prints the token.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// Fixed Bridge command: begin a storage lease and read the stored credential.
pub const COMMAND_LOAD: &str = "panel_credential_load";
/// Fixed Bridge command: persist a credential under the current lease.
pub const COMMAND_SAVE: &str = "panel_credential_save";
/// Fixed Bridge command: delete the stored credential under the current lease.
pub const COMMAND_DELETE: &str = "panel_credential_delete";

/// The connect-only projection of a persisted device credential: exactly a
/// device id and its long-lived device token.
#[derive(Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct DeviceConnectCredential {
    /// Non-secret device id the token was minted for. Never blank.
    pub device_id: String,
    /// Long-lived device token. Secret; never blank; redacted in `Debug`.
    pub device_token: String,
}

impl DeviceConnectCredential {
    /// Build a credential, refusing a blank id or token.
    pub fn new(
        device_id: impl Into<String>,
        device_token: impl Into<String>,
    ) -> Result<Self, &'static str> {
        let this = Self {
            device_id: device_id.into(),
            device_token: device_token.into(),
        };
        this.validate()?;
        Ok(this)
    }

    /// Re-check the invariants (fields are `pub`, so a caller may have built
    /// the value literally). Fixed, secret-free messages.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.device_id.trim().is_empty() {
            return Err("device credential needs a device_id");
        }
        if self.device_token.trim().is_empty() {
            return Err("device credential token is empty");
        }
        Ok(())
    }
}

impl fmt::Debug for DeviceConnectCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceConnectCredential")
            .field("device_id", &self.device_id)
            .field("device_token", &"[redacted]")
            .finish()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceConnectCredentialWire {
    device_id: String,
    device_token: String,
}

impl<'de> Deserialize<'de> for DeviceConnectCredential {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = DeviceConnectCredentialWire::deserialize(deserializer)?;
        Self::new(wire.device_id, wire.device_token).map_err(serde::de::Error::custom)
    }
}

/// Result of `panel_credential_load`: a storage lease for the shell's
/// configured Gateway, plus the credential stored for it (if any).
///
/// `lease_id` is what `panel_credential_save` / `panel_credential_delete` must
/// present; a newer load or a target change invalidates older leases.
/// `gateway_url` is the WebSocket endpoint the shell derived from its own
/// configuration: the Panel must check it matches the endpoint it is about to
/// dial before using `credential`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, JsonSchema)]
pub struct PanelCredentialLease {
    /// Native-configured Gateway WebSocket endpoint (`ws(s)://host:port/ws`).
    pub gateway_url: String,
    /// Nonzero lease id.
    #[schemars(range(min = 1))]
    pub lease_id: u64,
    /// The stored credential, or `None` when the keyring is healthy and empty.
    #[schemars(required, schema_with = "nullable_credential_schema")]
    pub credential: Option<DeviceConnectCredential>,
}

impl PanelCredentialLease {
    /// Build a lease, refusing a zero id or blank Gateway URL.
    pub fn new(
        gateway_url: impl Into<String>,
        lease_id: u64,
        credential: Option<DeviceConnectCredential>,
    ) -> Result<Self, &'static str> {
        let gateway_url = gateway_url.into();
        if lease_id == 0 {
            return Err("credential lease id must be nonzero");
        }
        if gateway_url.trim().is_empty() {
            return Err("credential lease needs a gateway_url");
        }
        if let Some(credential) = &credential {
            credential.validate()?;
        }
        Ok(Self {
            gateway_url,
            lease_id,
            credential,
        })
    }
}

// `required` alone removes Option's null branch in the generated schema.
// Keep the mandatory field nullable, matching the strict wire decoder.
fn nullable_credential_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    generator.subschema_for::<Option<DeviceConnectCredential>>()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PanelCredentialLeaseWire {
    gateway_url: String,
    lease_id: u64,
    // `credential` is required (null = none): a reply that omits it must not
    // decode as "nothing stored". `deserialize_with` disables serde's implicit
    // missing-`Option` => `None`.
    #[serde(deserialize_with = "required_option")]
    credential: Option<DeviceConnectCredential>,
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

impl<'de> Deserialize<'de> for PanelCredentialLease {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = PanelCredentialLeaseWire::deserialize(deserializer)?;
        Self::new(wire.gateway_url, wire.lease_id, wire.credential)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lease_schema_requires_nullable_credential_and_nonzero_id() {
        let schema = serde_json::to_value(schemars::schema_for!(PanelCredentialLease)).unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|value| value == "credential"));
        assert_eq!(
            schema["properties"]["lease_id"]["minimum"].as_f64(),
            Some(1.0)
        );
        assert!(schema["properties"]["credential"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .any(|branch| branch["type"] == "null"));
    }

    fn cred() -> serde_json::Value {
        json!({"device_id": "dev-1", "device_token": "s3cret-token"})
    }

    #[test]
    fn credential_round_trips() {
        let c: DeviceConnectCredential = serde_json::from_value(cred()).unwrap();
        assert_eq!(c.device_id, "dev-1");
        assert_eq!(serde_json::to_value(&c).unwrap(), cred());
    }

    #[test]
    fn credential_rejects_unknown_ticket_and_role_fields() {
        for extra in ["bootstrap_ticket", "role", "user_id", "account"] {
            let mut v = cred();
            v[extra] = json!("x");
            assert!(
                serde_json::from_value::<DeviceConnectCredential>(v).is_err(),
                "{extra}"
            );
        }
    }

    #[test]
    fn credential_rejects_blank_and_missing_fields() {
        for v in [
            json!({"device_id": " ", "device_token": "t"}),
            json!({"device_id": "d", "device_token": ""}),
            json!({"device_id": "d"}),
            json!({"device_token": "t"}),
        ] {
            assert!(serde_json::from_value::<DeviceConnectCredential>(v).is_err());
        }
        assert!(DeviceConnectCredential::new("d", "  ").is_err());
    }

    #[test]
    fn credential_debug_redacts_token() {
        let c: DeviceConnectCredential = serde_json::from_value(cred()).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("s3cret-token"));
        assert!(dbg.contains("[redacted]"));
        assert!(dbg.contains("dev-1"));
    }

    fn lease_json(id: u64, credential: serde_json::Value) -> serde_json::Value {
        json!({"gateway_url": "ws://127.0.0.1:18790/ws", "lease_id": id, "credential": credential})
    }

    #[test]
    fn lease_round_trips_with_and_without_credential() {
        let with: PanelCredentialLease = serde_json::from_value(lease_json(7, cred())).unwrap();
        assert_eq!(with.lease_id, 7);
        assert_eq!(with.credential.as_ref().unwrap().device_id, "dev-1");
        assert_eq!(serde_json::to_value(&with).unwrap(), lease_json(7, cred()));

        let none: PanelCredentialLease =
            serde_json::from_value(lease_json(1, serde_json::Value::Null)).unwrap();
        assert!(none.credential.is_none());
    }

    #[test]
    fn lease_rejects_zero_id_blank_url_unknown_and_missing_fields() {
        assert!(serde_json::from_value::<PanelCredentialLease>(lease_json(0, cred())).is_err());
        let mut blank = lease_json(1, cred());
        blank["gateway_url"] = json!("  ");
        assert!(serde_json::from_value::<PanelCredentialLease>(blank).is_err());
        for extra in ["ticket", "role", "user", "account"] {
            let mut v = lease_json(1, cred());
            v[extra] = json!("x");
            assert!(
                serde_json::from_value::<PanelCredentialLease>(v).is_err(),
                "{extra}"
            );
        }
        // `credential` must be present (null is fine, absence is not).
        let absent = json!({"gateway_url": "ws://h:1/ws", "lease_id": 1});
        assert!(serde_json::from_value::<PanelCredentialLease>(absent).is_err());
        // Negative / fractional ids are not u64.
        let mut neg = lease_json(1, cred());
        neg["lease_id"] = json!(-1);
        assert!(serde_json::from_value::<PanelCredentialLease>(neg).is_err());
    }

    #[test]
    fn lease_debug_never_prints_token() {
        let lease: PanelCredentialLease = serde_json::from_value(lease_json(3, cred())).unwrap();
        assert!(!format!("{lease:?}").contains("s3cret-token"));
    }

    #[test]
    fn command_names_are_the_three_fixed_methods() {
        assert_eq!(COMMAND_LOAD, "panel_credential_load");
        assert_eq!(COMMAND_SAVE, "panel_credential_save");
        assert_eq!(COMMAND_DELETE, "panel_credential_delete");
    }
}
