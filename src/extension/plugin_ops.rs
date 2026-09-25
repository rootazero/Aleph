//! Plugin execution operations for `ExtensionManager`

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::registry::PluginRegistry;
use crate::extension::types::{DirectCommandResult, PluginInfo, PluginRecord};
use crate::sync_primitives::Arc;

use super::ExtensionManager;

impl ExtensionManager {
    /// Call a tool on a runtime plugin (Node.js or WASM).
    ///
    /// The plugin's module must already be mounted (see
    /// [`Self::ensure_runtime_mounted`]).
    /// Acquires a write lock on the plugin loader for IPC. The guest call is
    /// CPU-bound untrusted code, so it runs on the blocking pool (bounded by
    /// the Extism manifest timeout) rather than on an async worker thread.
    pub async fn call_plugin_tool(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.ensure_runtime_mounted(plugin_id).await?;

        let loader = self.plugin_loader.clone().write_owned().await;
        let plugin_id = plugin_id.to_string();
        let handler = handler.to_string();
        tokio::task::spawn_blocking(move || loader.call_tool(&plugin_id, &handler, args))
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    async fn ensure_plugin_active(&self, plugin_id: &str) -> ExtensionResult<()> {
        let registry = self.plugin_registry.read().await;
        match registry.get_plugin(plugin_id) {
            Some(record) if record.status.is_active() => Ok(()),
            Some(_) => Err(ExtensionError::Runtime(format!(
                "Plugin '{plugin_id}' is disabled"
            ))),
            None => Err(ExtensionError::PluginNotFound(plugin_id.to_string())),
        }
    }

    /// A runtime call needs the plugin's module in the loader. Modules load
    /// at mount (`lifecycle.rs`, `wasm_module` step), never lazily here: a
    /// lazily created effect would sit outside the scope's dispose order.
    async fn ensure_runtime_mounted(&self, plugin_id: &str) -> ExtensionResult<()> {
        self.ensure_plugin_active(plugin_id).await?;
        if self.plugin_loader.read().await.is_loaded(plugin_id) {
            Ok(())
        } else {
            Err(ExtensionError::Runtime(format!(
                "Plugin '{plugin_id}' has no runtime mounted (WASM modules load at mount; \
                 MCP plugin tools are called through McpManager, not the plugin loader)"
            )))
        }
    }

    /// Execute a hook handler on a runtime plugin.
    ///
    /// The plugin's module must already be mounted (mirrors
    /// [`Self::call_plugin_tool`]): a hook fires only for a mounted plugin.
    pub async fn execute_plugin_hook(
        &self,
        plugin_id: &str,
        handler: &str,
        event_data: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        self.ensure_runtime_mounted(plugin_id).await?;

        let loader = self.plugin_loader.clone().read_owned().await;
        let plugin_id = plugin_id.to_string();
        let handler = handler.to_string();
        tokio::task::spawn_blocking(move || loader.execute_hook(&plugin_id, &handler, event_data))
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    /// Execute a direct command on a runtime plugin.
    pub async fn execute_plugin_command(
        &self,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<DirectCommandResult> {
        self.ensure_runtime_mounted(plugin_id).await?;

        let loader = self.plugin_loader.clone().read_owned().await;
        let plugin_id = plugin_id.to_string();
        let handler = handler.to_string();
        tokio::task::spawn_blocking(move || loader.execute_command(&plugin_id, &handler, args))
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?
    }

    /// Get the plugin registry (read access).
    pub async fn get_plugin_registry(&self) -> tokio::sync::RwLockReadGuard<'_, PluginRegistry> {
        self.plugin_registry.read().await
    }

    /// P3 Stage I — clone the shared registry handle for per-subagent MCP scope
    /// provisioning. Unlike `get_plugin_registry` (a borrowed read guard tied to
    /// `&self`), this hands out the `Arc<RwLock<..>>` itself so a subagent
    /// spawner can carry it across layers and snapshot the referenced tools
    /// under its own read guard at provision time.
    #[must_use]
    pub fn plugin_registry_handle(&self) -> Arc<tokio::sync::RwLock<PluginRegistry>> {
        self.plugin_registry.clone()
    }

    /// Get mutable access to the plugin registry.
    pub async fn get_plugin_registry_mut(
        &self,
    ) -> tokio::sync::RwLockWriteGuard<'_, PluginRegistry> {
        self.plugin_registry.write().await
    }

    /// Get the plugin loader (read access).
    pub async fn get_plugin_loader(&self) -> tokio::sync::RwLockReadGuard<'_, super::PluginLoader> {
        self.plugin_loader.read().await
    }

    /// Test-only: the loader handle itself, so a fixture can pre-load a module.
    #[cfg(test)]
    pub(crate) fn get_plugin_loader_for_test(
        &self,
    ) -> Arc<tokio::sync::RwLock<super::PluginLoader>> {
        self.plugin_loader.clone()
    }

    /// Get all plugin info from `PluginRegistry`.
    pub async fn get_plugin_info(&self) -> Vec<PluginInfo> {
        self.plugin_registry
            .read()
            .await
            .list_plugins()
            .into_iter()
            .map(|record| PluginInfo {
                name: record.id.clone(),
                version: record.version.clone(),
                description: record.description.clone(),
                enabled: record.status.is_active(),
                path: record.root_dir.display().to_string(),
                skills_count: record.skill_count,
                commands_count: record.command_count,
                agents_count: record.agent_count,
                hooks_count: record.hook_count,
                mcp_servers_count: record.mcp_server_count,
                tools_count: record.tool_names.len(),
                kind: record.kind.as_str().to_string(),
                status: record.status.label().to_string(),
                error: record.error.clone(),
            })
            .collect()
    }

    /// Get a specific plugin record by name.
    pub async fn get_plugin_record(&self, name: &str) -> Option<PluginRecord> {
        self.plugin_registry.read().await.get_plugin(name).cloned()
    }

    /// List all plugin records (cloned) for the store's installed reconciliation.
    pub async fn list_plugin_records(&self) -> Vec<PluginRecord> {
        self.plugin_registry
            .read()
            .await
            .list_plugins()
            .into_iter()
            .cloned()
            .collect()
    }

    /// Drop a plugin's durable activation preference.
    ///
    /// Called on uninstall. Without it a same-id re-install silently inherits
    /// the previous `enabled = false` and looks broken on arrival — the
    /// "an orphan sweep will catch it later" argument only covers things that
    /// changed name, and a same-id reinstall is never an orphan.
    pub async fn forget_plugin_preference(&self, plugin_id: &str) {
        let mut cfg = self.plugins_config.write().await;
        if cfg.forget(plugin_id) {
            if let Err(e) = cfg.save(&self.plugins_config_path).await {
                tracing::warn!(
                    plugin_id, error = %e,
                    "failed to persist plugin preference removal"
                );
            }
        }
    }

    /// This plugin's **stored** configuration, as JSON.
    ///
    /// `{}` for a plugin the operator has never configured — which is not an
    /// error, and is exactly what a plugin with no `config_schema` should see.
    ///
    /// Stored form: any `{{secret:NAME}}` reference is returned verbatim. This
    /// is what the display faces want — `plugin_manage(config_get / show)`
    /// puts this text in the model's context and `plugin.config.get` puts it
    /// in Panel, and neither is a place for a decrypted credential. Code
    /// handing configuration to plugin code wants
    /// [`Self::plugin_settings_for_runtime`] instead.
    pub async fn plugin_settings(&self, plugin_id: &str) -> serde_json::Value {
        self.plugins_config.read().await.settings_json(plugin_id)
    }

    /// This plugin's configuration with `{{secret:NAME}}` references resolved
    /// against the vault — the form plugin code receives.
    ///
    /// See [`crate::extension::plugin_secrets`] for why this is a separate
    /// function rather than a flag: resolving inside `plugin_settings` would
    /// have been one line and would have piped every configured secret into
    /// the transcript and the settings UI.
    pub async fn plugin_settings_for_runtime(&self, plugin_id: &str) -> serde_json::Value {
        let stored = self.plugin_settings(plugin_id).await;
        let resolver = crate::gateway::security::shared_token::SharedTokenManager::global()
            .map(crate::secrets::VaultSecretResolver::new);
        crate::extension::plugin_secrets::resolve_settings(
            &stored,
            resolver
                .as_ref()
                .map(|r| r as &dyn crate::secrets::AsyncSecretResolver),
            plugin_id,
        )
        .await
    }

    /// This plugin's configuration as a child process's environment
    /// (`plugin_vars::settings_env`), in `form` — the one derivation behind a
    /// plugin hook's environment and a plugin command's inline shell
    /// environment. Plugin-owned only: a settings-source label such as
    /// `user:project` is not a plugin id and gets nothing.
    pub(crate) async fn plugin_settings_env(
        &self,
        plugin_id: &str,
        form: crate::extension::plugin_secrets::SettingsForm,
    ) -> Vec<(String, String)> {
        use crate::extension::plugin_secrets::{without_secret_references, SettingsForm};
        if crate::extension::manifest::validate_plugin_id(plugin_id).is_err() {
            return Vec::new();
        }
        let settings = match form {
            SettingsForm::Runtime => self.plugin_settings_for_runtime(plugin_id).await,
            SettingsForm::WithoutSecrets => {
                without_secret_references(&self.plugin_settings(plugin_id).await)
            }
        };
        crate::extension::plugin_vars::settings_env(&settings)
    }

    /// Replace a plugin's configuration, validating it against the schema the
    /// plugin's own manifest declares.
    ///
    /// This is the first caller `validate_config_against_schema` has ever had
    /// outside the authoring-time linter. Before it existed a plugin could
    /// declare a `config_schema` and no surface could write a value against
    /// it, so the schema described a control that was not there.
    ///
    /// Validation is fail-closed and reports **every** violation rather than
    /// the first: an operator fixing a config one error per round-trip is the
    /// thing schema validation exists to avoid. A plugin that declares no
    /// schema accepts anything — refusing would make configuration impossible
    /// for the plugins that predate the schema field.
    ///
    /// Applying it to a *running* plugin needs a reload; the caller is told so
    /// rather than the reload happening implicitly, because reloading a plugin
    /// tears down its MCP servers and background services and that is not a
    /// side effect a config write should smuggle in.
    pub async fn set_plugin_settings(
        &self,
        plugin_id: &str,
        settings: serde_json::Value,
    ) -> Result<bool, Vec<String>> {
        let serde_json::Value::Object(map) = settings else {
            return Err(vec![
                "Plugin configuration must be a JSON object of field → value".to_string(),
            ]);
        };

        // Validate against the manifest's schema when there is one.
        let schema = {
            let registry = self.plugin_registry.read().await;
            registry
                .get_plugin(plugin_id)
                .map(|record| record.root_dir.clone())
        };
        if let Some(root_dir) = schema {
            if let Ok(manifest) =
                crate::extension::manifest::parse_manifest_from_dir_cached_global(&root_dir)
            {
                if let Some(ref schema) = manifest.config_schema {
                    let value = serde_json::Value::Object(map.clone());
                    let errors =
                        crate::extension::manifest::validate_config_against_schema(schema, &value);
                    if !errors.is_empty() {
                        return Err(errors);
                    }
                }
            }
        }

        // JSON → TOML. A value JSON can hold but TOML cannot (a bare `null`)
        // is refused by name rather than silently dropped, which would leave
        // the operator believing a field was set.
        let mut converted = std::collections::BTreeMap::new();
        for (key, value) in map {
            match toml::Value::try_from(&value) {
                Ok(v) => {
                    converted.insert(key, v);
                }
                Err(e) => {
                    return Err(vec![format!(
                        "Field '{key}' cannot be stored in plugins.toml: {e}. \
                         (TOML has no null; omit the field instead.)"
                    )])
                }
            }
        }

        let changed = {
            let mut cfg = self.plugins_config.write().await;
            let changed = cfg.set_settings(plugin_id, converted);
            if changed {
                if let Err(e) = cfg.save(&self.plugins_config_path).await {
                    tracing::warn!(
                        plugin_id, error = %e,
                        "failed to persist plugin configuration; it will not survive a restart"
                    );
                    return Err(vec![format!("Failed to persist configuration: {e}")]);
                }
            }
            changed
        };
        if changed {
            self.publish_plugin_settings().await;
        }
        Ok(changed)
    }

    /// Mirror every plugin's stored configuration into the loader.
    ///
    /// Without this the store would be a place values go and never come back:
    /// `plugins.toml` would hold them, the RPC face would read them back, and
    /// the plugin runtime would never see one. Called at boot and after each
    /// config write — the two moments the stored set can differ from the
    /// loader's copy.
    pub async fn publish_plugin_settings(&self) {
        // Ids first, under a short-lived read lock; resolution below awaits the
        // vault and must not hold the config lock while it does.
        let ids: Vec<String> = {
            let cfg = self.plugins_config.read().await;
            cfg.entries.keys().cloned().collect()
        };
        let mut snapshot: std::collections::HashMap<String, serde_json::Value> =
            std::collections::HashMap::with_capacity(ids.len());
        for id in ids {
            // Runtime form: this snapshot is what the WASM guest and the MCP
            // child actually receive, so `{{secret:NAME}}` resolves here.
            let settings = self.plugin_settings_for_runtime(&id).await;
            snapshot.insert(id, settings);
        }
        self.plugin_loader
            .write()
            .await
            .set_all_plugin_settings(snapshot);
    }

    /// Vouch for (or withdraw vouching for) a plugin under the owner trust
    /// policy, durably, and refresh the live policy so the next load sees it.
    ///
    /// Returns whether the stored value changed.
    ///
    /// Deliberately does **not** unload a plugin that is already running when
    /// trust is withdrawn. The trust policy is a *load* gate — it decides what
    /// gets to start — and making `untrust` also tear down MCP servers and
    /// background services would give one verb two jobs. The caller is told to
    /// reload rather than the teardown happening implicitly, which is the same
    /// contract `set_plugin_settings` already has.
    ///
    /// Withdrawing trust therefore does not stop code that is already running;
    /// say so rather than letting a caller infer otherwise. `disable` is the
    /// verb that stops a running plugin now.
    pub async fn set_plugin_trusted(&self, plugin_id: &str, trusted: bool) -> bool {
        let mut cfg = self.plugins_config.write().await;
        let changed = cfg.set_trusted(plugin_id, trusted);
        if changed {
            if let Err(e) = cfg.save(&self.plugins_config_path).await {
                tracing::warn!(
                    plugin_id, error = %e,
                    "failed to persist plugin trust preference; \
                     it will not survive a restart"
                );
            }
        }
        // Re-derive from the document rather than mutating the live policy in
        // parallel: `OwnerTrustPolicy::trust`/`untrust` exist, and using them
        // here would make the in-memory allowlist a second copy that can drift
        // from the file the next boot reads.
        self.set_owner_trust_policy(cfg.owner_trust_policy());
        changed
    }

    /// Turn owner-trust enforcement on or off, durably. Returns whether it
    /// changed.
    ///
    /// Same load-gate semantics as [`Self::set_plugin_trusted`]: switching
    /// enforcement on does not unload the plugins that would now be refused.
    pub async fn set_trust_enforced(&self, enforce: bool) -> bool {
        let mut cfg = self.plugins_config.write().await;
        let changed = cfg.set_trust_enforced(enforce);
        if changed {
            if let Err(e) = cfg.save(&self.plugins_config_path).await {
                tracing::warn!(
                    error = %e,
                    "failed to persist owner trust enforcement; \
                     it will not survive a restart"
                );
            }
        }
        self.set_owner_trust_policy(cfg.owner_trust_policy());
        changed
    }

    /// The current trust posture, for display: whether enforcement is on and
    /// which plugin ids are vouched for.
    pub async fn trust_status(&self) -> (bool, Vec<String>) {
        let cfg = self.plugins_config.read().await;
        let mut ids: Vec<String> = cfg
            .entries
            .iter()
            .filter(|(_, e)| e.trusted == Some(true))
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        (cfg.trust.enforce, ids)
    }

    /// Enable or disable a plugin: record the operator's preference durably,
    /// then reflect it in the live registry.
    ///
    /// The preference is written **whether or not the in-memory registry knows
    /// this id**. A toggle issued before `load_all` has run (or against a
    /// plugin whose manifest currently fails to parse) still has to stick —
    /// keying persistence off "did the registry row change?" is how a disable
    /// becomes a no-op that reports success.
    ///
    /// Returns whether anything changed at all (preference or live status).
    pub async fn set_plugin_enabled(&self, plugin_id: &str, enabled: bool) -> bool {
        let preference_changed = {
            let mut cfg = self.plugins_config.write().await;
            let changed = cfg.set_enabled(plugin_id, enabled);
            if changed {
                if let Err(e) = cfg.save(&self.plugins_config_path).await {
                    tracing::warn!(
                        plugin_id, error = %e,
                        "failed to persist plugin activation preference; \
                         it will not survive a restart"
                    );
                }
            }
            changed
        };

        let changed = if enabled {
            match self.mount(plugin_id).await {
                Ok(_) => true,
                Err(super::MountError::AlreadyMounted(_)) => false,
                Err(e) => {
                    // The row already says why (Blocked / Error / Disabled);
                    // the preference is recorded regardless.
                    tracing::warn!(plugin_id = %plugin_id, error = %e, "enable: mount failed");
                    false
                }
            }
        } else {
            match self.unmount(plugin_id).await {
                Ok(report) => {
                    if !report.all_ok() {
                        tracing::warn!(plugin_id = %plugin_id, ?report, "disable: some effects did not dispose cleanly");
                    }
                    true
                }
                // A row that was never mounted (Error / Blocked) still takes
                // the operator's verdict on its status.
                Err(super::UnmountError::NotMounted(_)) => {
                    self.plugin_registry.write().await.disable_plugin(plugin_id)
                }
                Err(super::UnmountError::NotFound(_)) => false,
            }
        };

        changed || preference_changed
    }
}
