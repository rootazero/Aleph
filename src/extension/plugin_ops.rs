//! Plugin execution operations for `ExtensionManager`

use crate::extension::error::{ExtensionError, ExtensionResult};
use crate::extension::registry::PluginRegistry;
use crate::extension::types::{PluginInfo, PluginRecord};
use crate::sync_primitives::Arc;

use super::ExtensionManager;

impl ExtensionManager {
    /// Execute a plugin tool after an exact canonical dispatch admission.
    ///
    /// Authority carries an already-Ready producer binding, never a Cold
    /// activation proxy. It is rechecked after waits; transport callers use
    /// [`Self::call_plugin_tool`], which remains mounted-only.
    pub(crate) async fn call_plugin_tool_with_authority(
        self: &Arc<Self>,
        authority: &crate::tools::dispatch_verdict::PluginActivationAuthority,
        plugin_id: &str,
        handler: &str,
        args: serde_json::Value,
    ) -> ExtensionResult<serde_json::Value> {
        if !authority.validates(plugin_id, handler, &args) {
            return Err(ExtensionError::Runtime(
                "plugin activation authority does not match this exact call".into(),
            ));
        }
        let admitted_input = args.clone();
        let owner = self.runtime_owner(plugin_id).ok_or_else(|| {
            ExtensionError::Runtime(format!(
                "plugin '{plugin_id}' has no controlled runtime owner"
            ))
        })?;
        if owner.record.id != plugin_id {
            return Err(ExtensionError::Runtime(
                "plugin runtime owner identity mismatch".into(),
            ));
        }
        if !authority.validates(plugin_id, handler, &args)
            || !self.owner_is_current(&owner)
            || owner.provider.state() != crate::capability::provider::ProviderState::Ready
        {
            return Err(ExtensionError::Runtime(
                "plugin activation became stale before execution".into(),
            ));
        }
        let expected = owner.identity.lock().unwrap().clone().ok_or_else(|| {
            ExtensionError::Runtime("ready plugin owner has no WASM identity".into())
        })?;
        let binding = authority
            .binding_for(&owner.provider, owner.owner_generation)
            .ok_or_else(|| ExtensionError::Runtime("plugin producer binding is stale".into()))?;
        let generation = owner.provider.generation();
        let permit = owner
            .acquire_permit()
            .await
            .map_err(ExtensionError::Runtime)?;
        let loader = self.plugin_loader.clone().write_owned().await;
        owner
            .provider
            .acquire_call(generation)
            .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
        if !authority.validates(plugin_id, handler, &args)
            || !authority
                .binding_for(&owner.provider, owner.owner_generation)
                .is_some_and(|current| binding.same_binding(&current))
            || !self.owner_is_current(&owner)
            || !owner.output.capabilities.iter().any(|capability| {
                matches!(
                    capability,
                    crate::extension::capability::CapabilityDeclaration::Tool(tool)
                        if tool.plugin_id == plugin_id && tool.handler == handler
                )
            })
        {
            return Err(ExtensionError::Runtime(
                "owner tool binding is not declared/current".into(),
            ));
        }
        let actual = loader.wasm_mount_identity(plugin_id).ok_or_else(|| {
            ExtensionError::Runtime("plugin WASM identity disappeared before execution".into())
        })?;
        if !expected.same_instance(&actual)
            || !binding.matches_mount(&expected)
            || !loader.wasm_binding_is_current(&binding)
        {
            return Err(ExtensionError::Runtime(
                "plugin WASM identity changed before execution".into(),
            ));
        }
        self.ensure_plugin_active(plugin_id).await?;
        if !authority.validates(plugin_id, handler, &args)
            || !authority
                .binding_for(&owner.provider, owner.owner_generation)
                .is_some_and(|current| binding.same_binding(&current))
            || !self.owner_is_current(&owner)
            || !loader.wasm_binding_is_current(&binding)
        {
            return Err(ExtensionError::Runtime(
                "plugin activation authority or WASM identity changed before execution".into(),
            ));
        }
        let operation = owner.clone();
        let proof = authority.publication_proof();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            operation
                .provider
                .acquire_call(generation)
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            if !proof.is_current() {
                return Err(ExtensionError::Runtime(
                    "plugin activation authority or WASM identity changed before execution".into(),
                ));
            }
            let result = loader.call_bound(&binding, args);
            operation
                .provider
                .acquire_call(generation)
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            result
        })
        .await
        .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?;
        if !authority.validates(plugin_id, handler, &admitted_input)
            || !self.owner_is_current(&owner)
            || owner.provider.acquire_call(generation).is_err()
            || self
                .plugin_loader
                .read()
                .await
                .wasm_mount_identity(plugin_id)
                .is_none_or(|current| !expected.same_instance(&current))
        {
            return Err(ExtensionError::Runtime(
                "plugin activation authority or WASM identity changed during execution".into(),
            ));
        }
        result
    }
    /// Call a tool on a runtime plugin (Node.js or WASM).
    ///
    /// The plugin's module must already be mounted (see
    /// `Self::ensure_runtime_mounted`).
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

        let owner = self.runtime_owner(plugin_id);
        self.reject_ownerless_canonical_tool(plugin_id, owner.is_some())?;
        let admission = if let Some(owner) = &owner {
            let generation = owner.provider.generation();
            owner
                .provider
                .acquire_call(generation)
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            Some((
                owner
                    .acquire_permit()
                    .await
                    .map_err(ExtensionError::Runtime)?,
                generation,
            ))
        } else {
            None
        };
        let loader = self.plugin_loader.clone().write_owned().await;
        self.ensure_plugin_active(plugin_id).await?;
        self.reject_ownerless_canonical_tool(plugin_id, owner.is_some())?;
        let bound = if let Some(owner) = &owner {
            owner
                .provider
                .acquire_call(admission.as_ref().unwrap().1)
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            if !self.owner_is_current(owner)
                || !owner.output.capabilities.iter().any(|capability| {
                    matches!(
                        capability,
                        crate::extension::capability::CapabilityDeclaration::Tool(tool)
                            if tool.plugin_id == plugin_id && tool.handler == handler
                    )
                })
            {
                return Err(ExtensionError::Runtime(
                    "owner tool binding is not declared/current".into(),
                ));
            }
            let expected = owner.identity.lock().unwrap().clone().ok_or_else(|| {
                ExtensionError::Runtime("ready owner has no mount identity".into())
            })?;
            let binding = loader.bind_wasm(plugin_id, handler)?;
            if !binding.matches_mount(&expected) {
                return Err(ExtensionError::Runtime(
                    "owner mount identity changed".into(),
                ));
            }
            Some(binding)
        } else {
            None
        };
        let plugin_id_owned = plugin_id.to_string();
        let handler_owned = handler.to_string();
        let operation = owner.clone();
        let result = tokio::task::spawn_blocking(move || {
            let generation = admission.as_ref().map(|(_, generation)| *generation);
            let _admission = admission;
            let result = if let Some(binding) = bound {
                loader.call_bound(&binding, args)
            } else {
                loader.call_tool(&plugin_id_owned, &handler_owned, args)
            };
            if let (Some(owner), Some(generation)) = (operation, generation) {
                owner
                    .provider
                    .acquire_call(generation)
                    .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            }
            result
        })
        .await
        .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?;
        if let Some(owner) = owner {
            if !self.owner_is_current(&owner) || !owner.provider.is_ready() {
                return Err(ExtensionError::Runtime(
                    "owner changed during execution".into(),
                ));
            }
        }
        result
    }

    fn reject_ownerless_canonical_tool(
        &self,
        plugin_id: &str,
        has_captured_owner: bool,
    ) -> ExtensionResult<()> {
        if !has_captured_owner
            && (self.runtime_owner(plugin_id).is_some()
                || self
                    .tool_handler_registry
                    .read()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|registry| registry.has_plugin_publication(plugin_id)))
        {
            return Err(ExtensionError::Runtime(
                "plugin tool has no current RuntimeOwner authority".into(),
            ));
        }
        Ok(())
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

        let owner = self.runtime_owner(plugin_id);
        if owner.is_none()
            && self
                .tool_handler_registry
                .read()
                .unwrap()
                .as_ref()
                .is_some_and(|registry| {
                    registry.canonical_publication().has_ready_plugin(plugin_id)
                })
        {
            return Err(ExtensionError::Runtime(
                "plugin hook has no current RuntimeOwner authority".into(),
            ));
        }
        if owner.is_none() {
            // Legacy mounted plugins predate RuntimeOwner publication. Their
            // compatibility hook path remains available until canonical
            // publication takes authority for the plugin.
            let loader = self.plugin_loader.clone().read_owned().await;
            self.ensure_plugin_active(plugin_id).await?;
            let plugin_id = plugin_id.to_owned();
            let handler = handler.to_owned();
            return tokio::task::spawn_blocking(move || {
                loader.execute_hook(&plugin_id, &handler, event_data)
            })
            .await
            .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?;
        }
        let owner = owner.expect("owner checked above");
        // Tool-only owners must not turn an arbitrary export into a hook.
        if !owner.output.capabilities.iter().any(|capability| {
            matches!(
                capability,
                crate::extension::capability::CapabilityDeclaration::Hook(hook)
                    if hook.plugin_id == plugin_id
                        && hook.handler == handler
                        && hook.actions.is_empty()
            )
        }) {
            return Err(ExtensionError::Runtime(
                "owner hook binding is not declared".into(),
            ));
        }
        let generation = owner.provider.generation();
        let admission = (
            owner
                .acquire_permit()
                .await
                .map_err(ExtensionError::Runtime)?,
            generation,
        );
        owner
            .provider
            .acquire_call(generation)
            .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
        let loader = self.plugin_loader.clone().read_owned().await;
        self.ensure_plugin_active(plugin_id).await?;
        let binding = loader.bind_wasm_hook(plugin_id, handler)?;
        owner
            .provider
            .acquire_call(admission.1)
            .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
        let identity = owner.identity.lock().unwrap().clone();
        if !self.owner_is_current(&owner)
            || identity
                .as_ref()
                .is_none_or(|mount| !binding.matches_mount(mount))
        {
            return Err(ExtensionError::Runtime(
                "owner hook binding changed before execution".into(),
            ));
        }
        let operation = owner.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _admission = admission;
            let result = loader.call_bound(&binding, event_data);
            operation
                .provider
                .acquire_call(_admission.1)
                .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            result
        })
        .await
        .map_err(|e| ExtensionError::Runtime(format!("WASM task join failed: {e}")))?;
        if !self.owner_is_current(&owner) || !owner.provider.is_ready() {
            return Err(ExtensionError::Runtime(
                "owner changed during hook execution".into(),
            ));
        }
        result
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

    /// Resolve mounted WASM backends dynamically, including mounts performed
    /// after a root/child/manual facade was assembled. No authority is copied.
    #[must_use]
    pub fn context_handler_resolver(&self) -> super::context_adapter::ContextHandlerResolver {
        super::context_adapter::handler_resolver(
            self.plugin_loader.clone(),
            self.plugin_registry.clone(),
        )
    }

    /// Discovery from immutable, validated facts on the actual mounted instance.
    /// This is not consent and never reads a replacement manifest from disk.
    pub async fn context_declarations(
        &self,
        plugin_id: &str,
    ) -> ExtensionResult<Vec<super::capability::ContextDeclaration>> {
        let _guard = self.load_guard.lock().await;
        self.ensure_plugin_active(plugin_id).await?;
        self.plugin_loader
            .read()
            .await
            .context_declarations(plugin_id)
    }

    /// Called only AFTER the canonical host confirmation boundary approves.
    /// Serialize with mount/unmount, issue from actual mounted declarations,
    /// and install the entire selected bundle in one registry transaction.
    pub async fn grant_context_capabilities(
        &self,
        plugin_id: &str,
        capability_ids: &[String],
        caller: &crate::context::fabric::AuthenticatedCaller,
        approval: super::capability::ContextGrantApproval,
        now: u64,
    ) -> ExtensionResult<usize> {
        use super::capability::ContextGrant;
        use super::registry::ContextRegistration;
        use crate::context::fabric::ContextNamespace;
        let reject =
            |message: &str| ExtensionError::Runtime(format!("context grant rejected: {message}"));
        let _guard = self.load_guard.lock().await;
        self.ensure_plugin_active(plugin_id).await?;
        if approval.scope.namespace != ContextNamespace::Session
            || approval.cross_session
            || approval.egress
            || approval.project_write
            || approval
                .scope
                .handles
                .iter()
                .any(|handle| handle != caller.session_id())
        {
            return Err(reject(
                "only session-local effect-free grants are supported",
            ));
        }
        let declarations = self
            .plugin_loader
            .read()
            .await
            .context_declarations(plugin_id)?;
        let ids: std::collections::HashSet<_> = capability_ids.iter().collect();
        if ids.is_empty() || ids.len() != capability_ids.len() {
            return Err(reject("capability selection is empty or duplicated"));
        }
        let selected: Vec<_> = declarations
            .iter()
            .filter(|declaration| ids.contains(&declaration.descriptor.id))
            .collect();
        if selected.len() != ids.len() {
            return Err(reject(
                "capability is not declared by the current mounted instance",
            ));
        }
        // Never install just one member of an advertised bundle, including on
        // its first registration (the registry cannot infer absent members).
        for declaration in &selected {
            if let Some(bundle) = &declaration.descriptor.bundle_id {
                if declarations.iter().any(|member| {
                    member.descriptor.bundle_id.as_ref() == Some(bundle)
                        && !ids.contains(&member.descriptor.id)
                }) {
                    return Err(reject(
                        "all mounted members of a bundle must be approved atomically",
                    ));
                }
            }
        }
        let mut registrations = Vec::with_capacity(selected.len());
        for declaration in selected {
            let grant = ContextGrant::host_issue(
                &declaration.descriptor,
                plugin_id,
                caller,
                approval.clone(),
                now,
            )
            .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
            registrations.push(ContextRegistration::new(
                declaration.descriptor.clone(),
                grant,
                Some(plugin_id.to_owned()),
            ));
        }
        let count = registrations.len();
        let notification_keys: Vec<_> =
            registrations.iter().map(ContextRegistration::key).collect();
        // No loader guard crosses this await. load_guard excludes lifecycle
        // replacement; re-check the live registry status at the write boundary.
        let mut registry = self.plugin_registry.write().await;
        if !registry
            .get_plugin(plugin_id)
            .is_some_and(|record| record.status.is_active())
        {
            return Err(reject("plugin is no longer active"));
        }
        registry
            .register_context_bundle(registrations, now)
            .map_err(|e| ExtensionError::Runtime(e.to_string()))?;
        drop(registry);
        drop(_guard);
        // Session start means this newly authorized plugin attachment. It is
        // metadata-only, never a second durable session event or an implicit
        // grant to observers on other mounted plugins.
        crate::context::fabric::ContextFabric::builder(self.plugin_registry_handle())
            .handler_resolver(self.context_handler_resolver())
            .build()
            .notify_lifecycle_selected(
                crate::context::fabric::ContextLifecycleEvent::SessionStart,
                caller,
                None,
                Some(&notification_keys),
            )
            .await;
        Ok(count)
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
                origin: record.origin.label().to_string(),
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
        let _guard = self.load_guard.lock().await;
        let mut candidate = self.plugins_config.read().await.clone();
        if candidate.forget(plugin_id) {
            if let Err(e) = self.commit_plugins_config_locked(candidate).await {
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
    /// is what the display face wants — `plugin_manage(config_get / show)`
    /// puts this text in the model's context, and that is not a place for a
    /// decrypted credential. Code handing configuration to plugin code wants
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

        let _guard = self.load_guard.lock().await;
        let mut candidate = self.plugins_config.read().await.clone();
        let changed = candidate.set_settings(plugin_id, converted);
        if changed {
            self.commit_plugins_config_locked(candidate)
                .await
                .map_err(|e| vec![format!("Failed to persist configuration: {e}")])?;
        }
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
        let _guard = self.load_guard.lock().await;
        let mut candidate = self.plugins_config.read().await.clone();
        let changed = candidate.set_trusted(plugin_id, trusted);
        let policy = candidate.owner_trust_policy();
        if changed {
            if let Err(e) = self.commit_plugins_config_locked(candidate).await {
                tracing::warn!(plugin_id, error = %e, "failed to persist plugin trust preference");
                return false;
            }
        }
        self.set_owner_trust_policy(policy);
        changed
    }

    /// Turn owner-trust enforcement on or off, durably. Returns whether it
    /// changed.
    ///
    /// Same load-gate semantics as [`Self::set_plugin_trusted`]: switching
    /// enforcement on does not unload the plugins that would now be refused.
    pub async fn set_trust_enforced(&self, enforce: bool) -> bool {
        let _guard = self.load_guard.lock().await;
        let mut candidate = self.plugins_config.read().await.clone();
        let changed = candidate.set_trust_enforced(enforce);
        let policy = candidate.owner_trust_policy();
        if changed {
            if let Err(e) = self.commit_plugins_config_locked(candidate).await {
                tracing::warn!(error = %e, "failed to persist owner trust enforcement");
                return false;
            }
        }
        self.set_owner_trust_policy(policy);
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

    /// Persist the candidate before publishing it. Caller MUST hold load_guard:
    /// all writers of this document use that same sequence, including migration.
    /// No config write guard crosses save or runtime reconciliation.
    pub(super) async fn commit_plugins_config_locked(
        &self,
        candidate: super::plugin_state::PluginsConfig,
    ) -> crate::error::Result<()> {
        candidate.save(&self.plugins_config_path).await?;
        *self.plugins_config.write().await = candidate;
        Ok(())
    }

    /// Serialized candidate save -> live publication -> runtime reconciliation.
    /// A runtime failure preserves the durable desired state and returns it in
    /// the error. Repeating the same preference still reconciles the runtime.
    pub async fn set_plugin_enabled(
        &self,
        plugin_id: &str,
        enabled: bool,
    ) -> Result<super::plugin_state::PluginControlOutcome, super::plugin_state::PluginControlError>
    {
        use super::plugin_state::{
            PluginControlError, PluginControlOutcome, PluginControlPhase, PluginRuntimeOutcome,
        };

        let guard = self.load_guard.lock().await;
        let mut candidate = self.plugins_config.read().await.clone();
        let preference_changed = candidate.set_enabled(plugin_id, enabled);
        let mut outcome = PluginControlOutcome {
            plugin_id: plugin_id.to_owned(),
            desired_enabled: enabled,
            preference_changed,
            persisted: false,
            actual_state: self.control_actual_state(plugin_id).await,
            provider_generation: None,
            operation_id: uuid::Uuid::new_v4().to_string(),
            runtime_outcome: PluginRuntimeOutcome::NotAttempted,
            error_phase: None,
            error: None,
            cleanup_report: Vec::new(),
        };
        // Save even an unchanged candidate: durable success is a fact, not an
        // inference from the in-memory value (the file could have been removed).
        if let Err(error) = self.commit_plugins_config_locked(candidate).await {
            outcome.error_phase = Some(PluginControlPhase::Persistence);
            outcome.error = Some(error.to_string());
            drop(guard);
            return Err(PluginControlError {
                outcome: Box::new(outcome),
            });
        }
        outcome.persisted = true;
        self.reconcile_plugin_control_locked(plugin_id, enabled, &mut outcome)
            .await;

        // `mount_inner` deliberately stops at Cold for eligible tool-only WASM.
        // Capture that exact incarnation while this same guard still excludes
        // replacement and disable. Activation itself must run after releasing
        // the guard: its retained task acquires the guard for mount effects.
        let cold_declared = enabled && outcome.runtime_outcome == PluginRuntimeOutcome::Cold;
        let activation_owner = if cold_declared {
            self.runtime_owner(plugin_id)
        } else {
            None
        };
        // A Cold declaration whose runtime owner is gone is an unreachable
        // state: the declaration and its owner are two facts about one
        // incarnation. Fail closed instead of reporting a Cold success that no
        // caller can reach.
        let owner_missing = cold_declared && activation_owner.is_none();
        let activation_manager = activation_owner.as_ref().map(|_| self.activation_manager());
        // Only a declared Cold incarnation needs the guard released, because its
        // retained activation task acquires that guard to mount effects. Every
        // other control op keeps holding it: the guard is what makes the state
        // this op reports at the end belong to its own transition instead of to
        // a toggle queued behind it.
        let retained_guard = if activation_owner.is_some() {
            drop(guard);
            None
        } else {
            Some(guard)
        };

        let activation_error = match (activation_owner.as_ref(), activation_manager) {
            (None, _) => None,
            (Some(owner), Some(Ok(manager))) => {
                // Activation is deliberately outside `load_guard`; the retained
                // task acquires that guard only when it begins mounting effects.
                manager.activate_owner(Arc::clone(owner)).await.err()
            }
            (Some(_), Some(Err(error))) => Some(error),
            (Some(_), None) => Some(ExtensionError::Runtime(
                "extension manager activation context is not attached".to_owned(),
            )),
        };

        let guard = match retained_guard {
            Some(guard) => guard,
            None => self.load_guard.lock().await,
        };
        if owner_missing {
            // The Cold declaration has no owner left to activate, so there is
            // no runtime behind it. Report the failure structurally instead
            // of a Cold success.
            outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
            outcome.error_phase = Some(PluginControlPhase::Reconcile);
            outcome.error = Some(format!(
                "runtime owner is missing for the cold declaration of plugin '{plugin_id}'"
            ));
        } else if let Some(owner) = activation_owner {
            outcome.provider_generation = Some(owner.provider.generation());
            let mounted = self.owner_activation_proof(plugin_id, &owner).await;
            if let Some(error) = activation_error {
                outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
                outcome.error_phase = Some(PluginControlPhase::Reconcile);
                outcome.error = Some(error.to_string());
            } else if mounted {
                outcome.runtime_outcome = PluginRuntimeOutcome::Mounted;
            } else {
                // A concurrent disable/reload may have fenced the owner after
                // activation returned. Never report Ready from the stale proof.
                outcome.runtime_outcome = PluginRuntimeOutcome::Failed;
                outcome.error_phase = Some(PluginControlPhase::Reconcile);
                outcome.error =
                    Some("activation completed without a current Ready mounted owner".to_owned());
            }
        }
        self.publish_runtime_transition_views().await;
        outcome.actual_state = self.control_actual_state(plugin_id).await;
        drop(guard);
        if outcome.error.is_some() {
            Err(PluginControlError {
                outcome: Box::new(outcome),
            })
        } else {
            Ok(outcome)
        }
    }

    /// The one proof that an owner is mounted, current and published.
    ///
    /// Extracted from the B5 inline check without changing what it asserts:
    /// the same owner instance is current for the id, its provider is
    /// `Ready`, its row reads `Loaded`, it holds a WASM mount identity and a
    /// batch receipt, and it owns the exact effect scope for the id.
    pub(super) async fn owner_activation_proof(
        &self,
        plugin_id: &str,
        owner: &Arc<crate::extension::runtime_owner::RuntimeOwner>,
    ) -> bool {
        let current = self.runtime_owner(plugin_id);
        let row_loaded = self
            .plugin_registry
            .read()
            .await
            .get_plugin(plugin_id)
            .is_some_and(|row| row.status == super::PluginStatus::Loaded);
        current
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, owner))
            && owner.provider.state() == crate::capability::provider::ProviderState::Ready
            && row_loaded
            && owner
                .identity
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
            && owner
                .receipt
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
            && self
                .scopes
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .contains_key(plugin_id)
    }

    async fn control_actual_state(&self, plugin_id: &str) -> String {
        match self.plugin_registry.read().await.get_plugin(plugin_id) {
            Some(record) if matches!(record.status, super::PluginStatus::Error(_)) => {
                "failed".to_owned()
            }
            Some(record) => record.status.label().to_owned(),
            None => "not_discovered".to_owned(),
        }
    }
}
