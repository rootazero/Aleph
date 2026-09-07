//! Browser configuration RPC handlers
//!
//! Provides RPC methods for managing browser system settings from the Panel UI.
//!
//! Admin-gated as a family: `gateway::method_admin` lists the `browser_config.`
//! prefix, so both methods here require an operator-tier caller, and
//! `browser_config.update` is additionally named in the mutating-method list.
//! Nothing in this module re-derives that decision — a method added to this
//! file inherits the prefix gate rather than opting into it.

use crate::browser::profile::{BrowserDriver, BrowserSystemConfig, ProfileConfig};
use crate::config::{Config, ReloadImpact};
use crate::gateway::event_bus::{ConfigChangedEvent, GatewayEvent, GatewayEventBus};
use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR, INVALID_PARAMS};
use crate::sync_primitives::Arc;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Browser config for the Panel UI.
///
/// One type serves both directions: it is the `browser_config.get` response and
/// the `browser_config.update` params. That is why every field a client may
/// legitimately not know about is `Option` / `#[serde(default)]` — a client
/// built against an older field set must still be able to save, and the server
/// must be able to tell "the caller wants this off" from "the caller never
/// heard of this switch".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BrowserConfigResponse {
    /// Default profile driver: "managed", "`existing_session`", or "cdp".
    ///
    /// `None` on the way *in* means "leave as persisted", the same
    /// absent-means-unchanged rule the three secret-exfiltration switches
    /// below use — a client that omits this field is not changing it. `Some(s)`
    /// where `s` is not a recognised wire spelling is the caller saying
    /// something this server does not understand, and [`driver_from_update`]
    /// refuses it rather than silently keeping the old value: reporting
    /// success while discarding what the operator typed is the report-success
    /// no-op shape (判据 §11), whether the fallback lands on `Managed` (the old
    /// shape) or on whatever was already persisted. Always `Some(..)` on the
    /// way out.
    #[serde(default)]
    pub default_driver: Option<String>,
    /// Global Playwright CLI headless default.
    ///
    /// The *global* flag only. A `Managed` profile carrying its own `headless`
    /// override wins over it at runtime, so writing this field changes nothing
    /// for that profile — [`Self::headless_shadowed_by`] names which ones.
    pub headless: bool,
    /// `DevTools` profile: "user" (user's Chrome) or "managed" (Aleph-managed instance)
    pub devtools_profile: String,
    /// SSRF protection: block private network access
    pub block_private: bool,
    /// SSRF: blocked domain patterns
    pub blocked_domains: Vec<String>,
    /// SSRF: allowed domain patterns (whitelist mode)
    pub allowed_domains: Vec<String>,
    /// Timeout (seconds) for navigate / `wait_for_text`
    pub nav_timeout_secs: u64,
    /// Timeout (seconds) for other actions (click/fill/type/etc)
    pub action_timeout_secs: u64,

    /// Block navigating to a URL that embeds a credential.
    ///
    /// `None` on the way *in* means "leave as persisted", not "turn off": this
    /// surface predates the three secret-exfiltration switches, so a client
    /// that does not send them must not silently reset an operator's `false`
    /// back to the default `true`. Always `Some(..)` on the way out.
    #[serde(default)]
    pub block_secrets_in_url: Option<bool>,
    /// Block form input (type/fill/select/dialog text) that embeds a credential.
    /// Absent-means-unchanged, as [`Self::block_secrets_in_url`].
    #[serde(default)]
    pub block_secrets_in_input: Option<bool>,
    /// Redact credentials out of page-derived text before it reaches the model.
    /// Absent-means-unchanged, as [`Self::block_secrets_in_url`].
    #[serde(default)]
    pub redact_secrets_in_content: Option<bool>,

    /// Report-only: `Managed` profiles whose own `headless` override shadows
    /// [`Self::headless`]. Ignored on update — see [`headless_shadowed_by`].
    #[serde(default)]
    pub headless_shadowed_by: Vec<String>,
}

/// Project the persisted browser config into the shape this surface speaks.
///
/// One projection, two callers: the `get` response and the `ConfigChanged`
/// payload that `update` broadcasts. The payload used to echo the *request*,
/// which is not the same value as what landed — a request omits any switch its
/// client does not know about, so a subscriber refreshing from the event would
/// read a hole where the config on disk has a value.
fn snapshot(browser: &BrowserSystemConfig) -> BrowserConfigResponse {
    // Find the "default" profile's driver, or fallback to "managed"
    let default_driver = Some(
        browser
            .profiles
            .get("default")
            .map_or(BrowserDriver::default(), |p| p.driver)
            .as_wire()
            .to_string(),
    );

    // DevTools profile: "user" (Your Chrome) unless explicitly set to Managed
    let devtools_profile = if browser
        .profiles
        .get("user")
        .is_some_and(|p| p.driver == BrowserDriver::Managed)
    {
        "managed"
    } else {
        "user"
    }
    .to_string();

    BrowserConfigResponse {
        default_driver,
        headless: browser.playwright_cli.headless,
        devtools_profile,
        block_private: browser.policy.block_private,
        blocked_domains: browser.policy.blocked_domains.clone(),
        allowed_domains: browser.policy.allowed_domains.clone(),
        nav_timeout_secs: browser.playwright_cli.nav_timeout_secs,
        action_timeout_secs: browser.playwright_cli.action_timeout_secs,
        block_secrets_in_url: Some(browser.policy.block_secrets_in_url),
        block_secrets_in_input: Some(browser.policy.block_secrets_in_input),
        redact_secrets_in_content: Some(browser.policy.redact_secrets_in_content),
        headless_shadowed_by: headless_shadowed_by(browser),
    }
}

/// What `handle_update` makes of the driver string a client sent.
///
/// Two different questions, two different answers (R70):
/// - `wire = None` — the caller's request does not carry this field at all.
///   That means "I am not changing this", the same absent-means-unchanged
///   rule the three secret-exfiltration switches use, so the persisted driver
///   is kept.
/// - `wire = Some(s)` where `s` is not one of [`BrowserDriver::ALL`]'s wire
///   spellings — the caller sent something and this server does not
///   understand it. That is refused, naming the field and listing the values
///   it accepts, rather than silently kept: the old `== "existing_session" ?
///   … : Managed` shape coerced every unrecognised value to `Managed`, and
///   silently falling back to whatever was already persisted is the same
///   report-success no-op shape (判据 §11) wearing a friendlier value — either
///   way the operator's save reports success while discarding what they
///   typed.
fn driver_from_update(wire: Option<&str>, current: BrowserDriver) -> Result<BrowserDriver, String> {
    match wire {
        None => Ok(current),
        Some(s) => BrowserDriver::from_wire(s).ok_or_else(|| {
            let accepted: Vec<&str> = BrowserDriver::ALL.iter().map(|d| d.as_wire()).collect();
            format!(
                "default_driver: unrecognised value {s:?}; accepted values are {}",
                accepted.join(", ")
            )
        }),
    }
}

/// `Managed` profiles whose own `headless` override shadows the global toggle.
///
/// `ProfileManager::get_backend` resolves it as
/// `profile.headless.unwrap_or(playwright_cli.headless)`, so a profile that
/// sets the override wins and the global switch this surface writes is inert
/// *for that profile* — the toggle appears to do nothing, with nothing on
/// screen saying why. Naming the shadowing profiles is deliberately all this
/// surface does about it: clearing an override the operator hand-wrote into
/// `config.toml` would silently discard a setting they chose.
///
/// `ExistingSession` profiles are absent on purpose — that driver builds a
/// `ChromeMcpBackend`, which never reads headless at all, so listing them
/// would be a false alarm.
///
/// Sorted, because `profiles` is a `HashMap` and an operator-facing list whose
/// order changes between two reads of an unchanged config reads as a change.
fn headless_shadowed_by(browser: &BrowserSystemConfig) -> Vec<String> {
    let mut names: Vec<String> = browser
        .profiles
        .iter()
        .filter(|(_, p)| p.driver == BrowserDriver::Managed && p.headless.is_some())
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

// =============================================================================
// RPC Handlers
// =============================================================================

/// Get browser configuration
pub async fn handle_get(request: JsonRpcRequest, config: Arc<RwLock<Config>>) -> JsonRpcResponse {
    let cfg = config.read().await;
    let response = snapshot(&cfg.general.browser);

    match serde_json::to_value(&response) {
        Ok(value) => JsonRpcResponse::success(request.id, value),
        Err(e) => JsonRpcResponse::error(
            request.id,
            INTERNAL_ERROR,
            format!("Failed to serialize config: {e}"),
        ),
    }
}

/// Update browser configuration
///
/// The response reports, per group, whether the change is already in force or
/// waits on a restart — and the SSRF verdict is *verified*, not declared. That
/// is [`crate::config::live_apply::classify_verified`]'s rule applied to a
/// handle that module does not own: the SSRF policy hot-applies only when a
/// `ProfileManager` has published itself
/// ([`crate::browser::manager::apply_policy_live`]), and in a process where
/// none has, answering "live" would be exactly the silent failure that rule
/// exists to prevent.
///
/// The remaining fields are not lumped in with it: profile drivers and the
/// Playwright CLI settings are captured by `ProfileManager` at construction, so
/// they are honestly `Restart` and are reported as such beside the one group
/// that is not.
pub async fn handle_update(
    request: JsonRpcRequest,
    config: Arc<RwLock<Config>>,
    event_bus: Arc<GatewayEventBus>,
) -> JsonRpcResponse {
    let params = match request.params {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, "Missing params");
        }
    };

    let update: BrowserConfigResponse = match serde_json::from_value(params) {
        Ok(u) => u,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id,
                INVALID_PARAMS,
                format!("Invalid params: {e}"),
            );
        }
    };

    let (effective, policy) = {
        let mut cfg = config.write().await;

        // Mutate a CLONE, not `cfg.general.browser` directly. `cfg` is the
        // live, running config — a `return` from the K3 refusal below must
        // leave it byte-for-byte as it was, or a rejected update would still
        // rewrite the in-memory driver (just never persist it to disk),
        // leaving the daemon serving a config no operator asked for and no
        // file agrees with. Everything below reads and writes `browser`; the
        // real `cfg.general.browser` is assigned from it only after
        // `validate()` passes.
        let mut browser = cfg.general.browser.clone();
        let browser = &mut browser;

        // Update default profile driver
        let current_default = browser
            .profiles
            .get("default")
            .map_or(BrowserDriver::default(), |p| p.driver);
        let driver = match driver_from_update(update.default_driver.as_deref(), current_default) {
            Ok(d) => d,
            Err(msg) => {
                return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg);
            }
        };

        // Update or create the "default" profile with the chosen driver
        if let Some(profile) = browser.profiles.get_mut("default") {
            profile.driver = driver;
        } else {
            browser.profiles.insert(
                "default".to_string(),
                ProfileConfig {
                    driver,
                    ..Default::default()
                },
            );
        }

        // Update "user" profile based on devtools_profile setting
        let user_driver = if update.devtools_profile == "user" {
            BrowserDriver::ExistingSession
        } else {
            BrowserDriver::Managed
        };
        if let Some(profile) = browser.profiles.get_mut("user") {
            profile.driver = user_driver;
        } else {
            browser.profiles.insert(
                "user".to_string(),
                ProfileConfig {
                    driver: user_driver,
                    browser: crate::browser::profile::BrowserType::Chrome,
                    ..Default::default()
                },
            );
        }

        // Update playwright_cli fields
        browser.playwright_cli.headless = update.headless;
        browser.playwright_cli.nav_timeout_secs = update.nav_timeout_secs;
        browser.playwright_cli.action_timeout_secs = update.action_timeout_secs;

        // Update SSRF policy. The three secret-exfiltration switches are
        // `Option` on the wire: absent means the caller does not carry them,
        // and writing a default over that would turn a client's silence into an
        // operator-invisible security downgrade.
        browser.policy.block_private = update.block_private;
        browser.policy.blocked_domains = update.blocked_domains.clone();
        browser.policy.allowed_domains = update.allowed_domains.clone();
        if let Some(v) = update.block_secrets_in_url {
            browser.policy.block_secrets_in_url = v;
        }
        if let Some(v) = update.block_secrets_in_input {
            browser.policy.block_secrets_in_input = v;
        }
        if let Some(v) = update.redact_secrets_in_content {
            browser.policy.redact_secrets_in_content = v;
        }

        // K3 (§9): the same engine/driver contradiction load-time
        // `Config::validate` refuses must be refused here too, not merely
        // producible here. Without this, a profile that separately carries
        // `engine = "obscura"` (hand-edited into config.toml, or once a later
        // task exposes it in the Panel) survives a save that only changes its
        // `driver` back to a legacy value — landing a config file that the
        // very next boot's `Config::validate()` would refuse to load. One
        // rule, checked through the one function
        // `browser::profile::validate_engine_driver` lives in — not
        // re-derived here as a second, drifting copy of the same question
        // (判据 §1, §9).
        //
        // Checked against the CLONE, before `cfg.general.browser` is touched:
        // a refusal here must leave the live config exactly as it was, not
        // merely leave the file on disk as it was.
        if let Err(msg) = browser.validate() {
            return JsonRpcResponse::error(request.id, INVALID_PARAMS, msg);
        }

        let effective = snapshot(browser);
        let policy = browser.policy.clone();

        // Validation passed — commit the clone as the real config, then save.
        cfg.general.browser = browser.clone();

        // Save to disk — use precise path to avoid overwriting other general settings
        if let Err(e) = cfg.save_incremental(&["general.browser"]) {
            return JsonRpcResponse::error(
                request.id,
                INTERNAL_ERROR,
                format!("Failed to save config: {e}"),
            );
        }
        (effective, policy)
    };

    // Push the SSRF policy onto the manager the daemon actually serves browser
    // tools from. Persisting alone left the running guard on its boot-time
    // policy while this response reported the change as done.
    let ssrf_impact = if crate::browser::manager::apply_policy_live(policy) {
        ReloadImpact::Live
    } else {
        ReloadImpact::Restart
    };

    // Broadcast change event — carrying what landed, not what was asked for.
    let event = GatewayEvent::ConfigChanged(ConfigChangedEvent {
        section: Some("browser".to_string()),
        value: serde_json::to_value(&effective).unwrap_or(serde_json::Value::Null),
        timestamp: chrono::Utc::now().timestamp_millis(),
    });
    let _ = event_bus.publish_gateway_event(&event);

    JsonRpcResponse::success(
        request.id,
        serde_json::json!({
            "success": true,
            "reload_impact": {
                "ssrf_policy": ssrf_impact,
                "profile_drivers": ReloadImpact::Restart,
                "playwright_cli": ReloadImpact::Restart,
            },
            "headless_shadowed_by": effective.headless_shadowed_by,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::manager::ProfileManager;
    use crate::browser::profile::BrowserType;
    use crate::config::Config;
    use crate::utils::paths::AlephHomeEnvGuard;
    use serde_json::json;

    fn make_config() -> Arc<RwLock<Config>> {
        Arc::new(RwLock::new(Config::default()))
    }

    fn make_event_bus() -> Arc<GatewayEventBus> {
        Arc::new(GatewayEventBus::new())
    }

    /// Exactly the field set the Panel sends today, so a test can vary one
    /// thing and still exercise the real request shape.
    fn base_params() -> serde_json::Value {
        json!({
            "default_driver": "managed",
            "headless": true,
            "devtools_profile": "user",
            "block_private": true,
            "blocked_domains": [],
            "allowed_domains": [],
            "nav_timeout_secs": 30,
            "action_timeout_secs": 10,
        })
    }

    async fn update(config: &Arc<RwLock<Config>>, params: serde_json::Value) -> JsonRpcResponse {
        let request = JsonRpcRequest::with_id("browser_config.update", Some(params), json!(1));
        handle_update(request, Arc::clone(config), make_event_bus()).await
    }

    async fn get(config: &Arc<RwLock<Config>>) -> serde_json::Value {
        let request = JsonRpcRequest::with_id("browser_config.get", None, json!(1));
        handle_get(request, Arc::clone(config))
            .await
            .result
            .expect("browser_config.get must succeed on a default config")
    }

    #[tokio::test]
    async fn test_browser_config_exposes_timeouts() {
        let v = get(&make_config()).await;
        assert_eq!(v["nav_timeout_secs"], 30);
        assert_eq!(v["action_timeout_secs"], 10);
    }

    /// L6: `default_driver` became `Option<String>` on a type that serves
    /// BOTH the `get` response and the `update` params. `snapshot` always
    /// wraps it in `Some(..)`, so nothing today sends a bare JSON `null` for
    /// it — but a future outbound `None` would fail deserialization of the
    /// WHOLE `BrowserConfigResponse` in the Panel, not just this field
    /// (`String`, not `Option<String>`, on that side). Pinning "the GET
    /// response's `default_driver` is never null" here means that failure
    /// mode reddens this test instead of surfacing only as a Panel-side
    /// deserialization error nobody connects back to this file.
    #[tokio::test]
    async fn get_response_default_driver_is_never_null() {
        let v = get(&make_config()).await;
        assert_ne!(
            v["default_driver"],
            serde_json::Value::Null,
            "the GET response must always carry a driver string"
        );
        assert!(
            v["default_driver"].is_string(),
            "default_driver must be a JSON string, not null: {:?}",
            v["default_driver"]
        );
    }

    /// The three secret-exfiltration switches must be readable: an operator who
    /// cannot see them cannot know whether page content is being redacted.
    #[tokio::test]
    async fn get_exposes_the_secret_exfiltration_switches() {
        let v = get(&make_config()).await;
        assert_eq!(v["block_secrets_in_url"], true);
        assert_eq!(v["block_secrets_in_input"], true);
        assert_eq!(v["redact_secrets_in_content"], true);
    }

    #[tokio::test]
    async fn update_can_change_a_secret_exfiltration_switch() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let config = make_config();
        let mut params = base_params();
        params["redact_secrets_in_content"] = json!(false);
        assert!(update(&config, params).await.is_success());

        assert_eq!(get(&config).await["redact_secrets_in_content"], false);
        assert!(
            !config
                .read()
                .await
                .general
                .browser
                .policy
                .redact_secrets_in_content,
            "the switch must reach the policy every guard is built from, not just the response"
        );
    }

    /// A client that predates these switches sends eight fields. Its silence
    /// must not be read as "turn them back on".
    #[tokio::test]
    async fn a_client_that_omits_the_switches_leaves_them_unchanged() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let config = make_config();
        config
            .write()
            .await
            .general
            .browser
            .policy
            .block_secrets_in_input = false;

        assert!(update(&config, base_params()).await.is_success());
        assert_eq!(get(&config).await["block_secrets_in_input"], false);
    }

    /// Honest downgrade: with no `ProfileManager` published, the SSRF change is
    /// on disk only and the response must not call it live.
    #[tokio::test]
    async fn ssrf_policy_is_reported_restart_when_no_manager_is_published() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let config = make_config();
        let v = update(&config, base_params()).await.result.unwrap();
        assert_eq!(v["reload_impact"]["ssrf_policy"], "restart");
        // The restart-scoped groups are named beside it rather than lumped in.
        assert_eq!(v["reload_impact"]["profile_drivers"], "restart");
        assert_eq!(v["reload_impact"]["playwright_cli"], "restart");
    }

    /// The positive arm, asserted by the *effect* rather than by the call: the
    /// published manager has to serve the new policy afterwards. Drop the
    /// `apply_policy_live` wiring and the manager keeps refusing loopback while
    /// the response says the change took effect.
    #[tokio::test]
    #[serial_test::serial(browser_live_manager)]
    async fn a_published_manager_receives_the_policy_and_earns_the_live_verdict() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let manager = Arc::new(ProfileManager::new(BrowserSystemConfig::default()));
        assert!(
            manager
                .check_url("http://127.0.0.1:9000/admin")
                .await
                .is_err(),
            "precondition: the boot policy blocks private networks"
        );
        // The daemon's one boot hook that publishes the served manager.
        manager.spawn_idle_reaper(3600);

        let config = make_config();
        let mut params = base_params();
        params["block_private"] = json!(false);
        let v = update(&config, params).await.result.unwrap();

        assert_eq!(v["reload_impact"]["ssrf_policy"], "live");
        assert!(
            manager
                .check_url("http://127.0.0.1:9000/admin")
                .await
                .is_ok(),
            "a 'live' verdict must mean the running manager serves the new policy"
        );
    }

    /// A `Managed` profile's own override wins over the global toggle, so the
    /// surface has to name it — otherwise the switch silently does nothing.
    #[tokio::test]
    async fn managed_profiles_that_shadow_the_headless_toggle_are_named() {
        let config = make_config();
        {
            let mut cfg = config.write().await;
            let profiles = &mut cfg.general.browser.profiles;
            profiles.insert(
                "default".into(),
                ProfileConfig {
                    driver: BrowserDriver::Managed,
                    headless: Some(false),
                    ..Default::default()
                },
            );
            // ExistingSession never reads headless, so naming it would be a
            // false alarm — it must stay out of the list.
            profiles.insert(
                "user".into(),
                ProfileConfig {
                    driver: BrowserDriver::ExistingSession,
                    browser: BrowserType::Chrome,
                    headless: Some(false),
                    ..Default::default()
                },
            );
        }

        assert_eq!(
            get(&config).await["headless_shadowed_by"],
            json!(["default"])
        );
    }

    #[tokio::test]
    async fn an_unshadowed_config_names_nobody() {
        assert_eq!(
            get(&make_config()).await["headless_shadowed_by"],
            json!([]),
            "the default config carries no override, so the toggle is authoritative"
        );
    }

    /// A GET followed by a PUT must not rewrite the operator's driver: a
    /// value this server itself produced (via `snapshot`) must always
    /// round-trip back through `driver_from_update`, for every driver
    /// including the one the Panel's two radio buttons cannot render.
    #[test]
    fn a_driver_the_panel_cannot_render_still_round_trips_through_a_save() {
        let mut browser = BrowserSystemConfig::default();
        browser.profiles.insert(
            "default".to_string(),
            ProfileConfig {
                driver: BrowserDriver::Cdp,
                ..Default::default()
            },
        );
        let shown = snapshot(&browser);
        assert_eq!(shown.default_driver, Some("cdp".to_string()));

        // What handle_update does with that string, isolated.
        assert_eq!(
            driver_from_update(shown.default_driver.as_deref(), BrowserDriver::Managed),
            Ok(BrowserDriver::Cdp)
        );
        assert_eq!(
            driver_from_update(Some("existing_session"), BrowserDriver::Cdp),
            Ok(BrowserDriver::ExistingSession)
        );
    }

    /// R70, absent half: a request that does not carry `default_driver` at
    /// all is not changing it — the same absent-means-unchanged rule the
    /// three secret-exfiltration switches already use — so the persisted
    /// driver is kept. This is the half that silently rots if only the error
    /// half below gets a test: it is easy to make "garbage errors" true while
    /// accidentally also making "missing errors".
    #[test]
    fn an_absent_driver_field_preserves_the_persisted_value() {
        assert_eq!(
            driver_from_update(None, BrowserDriver::Cdp),
            Ok(BrowserDriver::Cdp)
        );
        assert_eq!(
            driver_from_update(None, BrowserDriver::ExistingSession),
            Ok(BrowserDriver::ExistingSession)
        );
    }

    /// R70, present-but-garbage half: a driver string this server does not
    /// recognise is the caller saying something it does not understand, and
    /// that must be a refusal naming the field and listing what it accepts —
    /// not a silent fallback to whatever was already persisted. Falling back
    /// to the persisted value here would be the very report-success no-op
    /// shape (判据 §11) that motivated dropping the old `== "existing_session"
    /// ? … : Managed` coercion in the first place; it would just wear a
    /// friendlier value.
    #[test]
    fn an_unrecognised_driver_string_is_refused_not_silently_kept() {
        let err = driver_from_update(Some("quantum"), BrowserDriver::Cdp)
            .expect_err("an unrecognised driver string must be refused");
        assert!(err.contains("quantum"), "must quote the bad value: {err}");
        assert!(err.contains("default_driver"), "must name the field: {err}");
        for wire in ["managed", "existing_session", "cdp"] {
            assert!(
                err.contains(wire),
                "must list the accepted values, missing {wire}: {err}"
            );
        }
    }

    /// K3 (§9): the pairing `Config::validate` refuses at load must be
    /// refused here too. Before this fix, a profile that already carried
    /// `engine = "obscura"` with `driver = "cdp"` (a legal, previously-saved
    /// state) could have its driver silently rewritten to a legacy value by a
    /// save that only meant to flip the default driver — landing a config
    /// file the very next boot's `Config::validate()` would refuse to load.
    /// One rule must give the same answer from both doors.
    #[tokio::test]
    async fn saving_a_legacy_driver_over_an_obscura_profile_is_refused_not_persisted() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let config = make_config();
        {
            let mut cfg = config.write().await;
            cfg.general.browser.profiles.insert(
                "default".to_string(),
                ProfileConfig {
                    driver: BrowserDriver::Cdp,
                    engine: Some(crate::browser::engine::Engine::Obscura),
                    ..Default::default()
                },
            );
        }

        let mut params = base_params();
        params["default_driver"] = json!("managed");
        let response = update(&config, params).await;
        assert!(
            !response.is_success(),
            "engine=\"obscura\" + driver=\"managed\" must be refused — the same \
             pairing Config::validate refuses at load"
        );

        // And nothing landed: the profile's driver is still cdp.
        assert_eq!(
            config.read().await.general.browser.profiles["default"].driver,
            BrowserDriver::Cdp,
            "a refused update must not partially persist"
        );
    }

    /// L1: R70's refusal, asserted at the WIRE FACE, not just at
    /// `driver_from_update` in isolation. The two unit tests above prove the
    /// function is falsifiable; neither one proves `handle_update` actually
    /// reaches the `Err` arm and turns it into a refused RPC response —
    /// `base_params()` and every other test-side write in this file only
    /// ever send `"managed"`, so nothing before this test ever drove a
    /// garbage `default_driver` through the real request path (判据 §4: a
    /// test that only proves the function is called is not enough — the
    /// effect at the boundary must be asserted).
    #[tokio::test]
    async fn an_unrecognised_driver_at_the_wire_face_is_refused_not_persisted() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = AlephHomeEnvGuard::acquire_and_set(home.path());

        let config = make_config();
        {
            let mut cfg = config.write().await;
            cfg.general.browser.profiles.insert(
                "default".to_string(),
                ProfileConfig {
                    driver: BrowserDriver::Cdp,
                    ..Default::default()
                },
            );
        }

        let mut params = base_params();
        params["default_driver"] = json!("quantum");
        let response = update(&config, params).await;
        assert!(
            !response.is_success(),
            "an unrecognised driver string must be refused at the wire face, \
             not silently kept — reporting success while discarding what the \
             operator typed is the report-success no-op shape (判据 §11)"
        );

        // And nothing landed: the profile's driver is still cdp, not silently
        // kept-as-was by an Err arm that fell through to the old value.
        assert_eq!(
            config.read().await.general.browser.profiles["default"].driver,
            BrowserDriver::Cdp,
            "a refused update must not persist any driver at all"
        );
    }
}
