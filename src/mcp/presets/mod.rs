//! Built-in curated MCP preset catalog.
//!
//! A vetted, in-binary list of recommended MCP servers users can enable with
//! one click (or via natural language). Pure data + pure decision logic; the
//! gateway handler is a thin adapter that drives `McpManagerHandle`.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use std::collections::HashMap;

use crate::mcp::manager::{McpManagerConfig, McpTransportType};

/// Curated MCP server the user can one-click enable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPreset {
    /// Stable slug, the unique install key (also used as the server id).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Audience bucket.
    pub category: PresetCategory,
    /// One-line description (Chinese, user-facing).
    pub description: String,
    /// Vendor name.
    pub vendor: String,
    /// First-party official server (vs community).
    pub official: bool,
    /// Mainland-China reachability hint (display only).
    pub reachability: Reachability,
    /// Launch options, ranked: remote first, stdio fallback.
    pub transports: Vec<PresetTransport>,
    /// Env vars the user must/should provide.
    #[serde(default)]
    pub required_env: Vec<PresetEnvVar>,
    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetCategory {
    Developer,
    Daily,
    ModelProvider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reachability {
    /// Built for mainland China, fully reachable.
    CnNative,
    /// Global service, mainland reachability not guaranteed.
    Global,
    /// Reported unreliable behind the GFW.
    CnUnreliable,
}

/// One launch option for a preset. `kind` reuses the manager transport enum.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetTransport {
    pub kind: McpTransportType,
    /// stdio executable (e.g. "npx", "uvx").
    #[serde(default)]
    pub command: Option<String>,
    /// stdio args; may contain `<ENV_KEY>` placeholders.
    #[serde(default)]
    pub args: Vec<String>,
    /// remote url; may contain `<ENV_KEY>` placeholders.
    #[serde(default)]
    pub url: Option<String>,
    /// Runtime needed for stdio (e.g. "node", "python"); None = no runtime
    /// (remote endpoints).
    #[serde(default)]
    pub requires_runtime: Option<String>,
}

/// An env var the preset needs; drives the key-entry UI and the NeedsKey reply.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetEnvVar {
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Secret-looking value: masked in UI, never echoed back.
    #[serde(default)]
    pub secret: bool,
    /// Must be present for install to proceed (unless `default` is set).
    #[serde(default)]
    pub required: bool,
    /// Fallback value when the user provides none.
    #[serde(default)]
    pub default: Option<String>,
    /// Where to obtain the value.
    #[serde(default)]
    pub how_to_get_url: Option<String>,
}

const CATALOG_JSON: &str = include_str!("catalog.json");

/// Parsed, in-binary preset catalog (single source of truth).
pub fn catalog() -> &'static [McpPreset] {
    static CELL: OnceLock<Vec<McpPreset>> = OnceLock::new();
    CELL.get_or_init(|| {
        serde_json::from_str(CATALOG_JSON).expect("bundled MCP preset catalog.json must be valid")
    })
}

/// Look up a preset by id.
pub fn find(id: &str) -> Option<&'static McpPreset> {
    catalog().iter().find(|p| p.id == id)
}

/// Outcome of planning an install (pure; no side effects).
#[derive(Debug)]
pub enum InstallPlan {
    /// Ready to hand to `McpManagerHandle::add_server`.
    Ready(Box<McpManagerConfig>),
    /// Missing required keys; do not start. Carries what to ask for.
    NeedsKey(Vec<PresetEnvVar>),
    /// A server with this id already exists.
    AlreadyInstalled,
    /// No transport's runtime is available; carries the runtime display name.
    NoRuntime(String),
}

impl McpPreset {
    /// Required env vars that are still missing (no provided value and no default).
    pub fn missing_required_env(&self, provided: &HashMap<String, String>) -> Vec<PresetEnvVar> {
        self.required_env
            .iter()
            .filter(|e| {
                e.required
                    && e.default.is_none()
                    && provided.get(&e.key).is_none_or(|v| v.trim().is_empty())
            })
            .cloned()
            .collect()
    }

    /// Effective env: provided non-blank value, else default. Only declared keys.
    fn effective_env(&self, provided: &HashMap<String, String>) -> HashMap<String, String> {
        let mut env = HashMap::new();
        for e in &self.required_env {
            let value = provided
                .get(&e.key)
                .filter(|v| !v.trim().is_empty())
                .cloned()
                .or_else(|| e.default.clone());
            if let Some(value) = value {
                env.insert(e.key.clone(), value);
            }
        }
        env
    }

    /// Materialize a manager config from a chosen transport + resolved env.
    /// `<ENV_KEY>` placeholders in url/args are replaced from `env`.
    pub fn materialize(
        &self,
        transport: &PresetTransport,
        env: &HashMap<String, String>,
    ) -> McpManagerConfig {
        let subst = |s: &str| {
            let mut out = s.to_string();
            for (k, v) in env {
                out = out.replace(&format!("<{k}>"), v);
            }
            out
        };
        match transport.kind {
            McpTransportType::Http | McpTransportType::Sse => {
                let url = subst(transport.url.as_deref().unwrap_or_default());
                let mut cfg = if transport.kind == McpTransportType::Http {
                    McpManagerConfig::http(&self.id, &self.name, url)
                } else {
                    McpManagerConfig::sse(&self.id, &self.name, url)
                };
                cfg.env = env.clone();
                cfg
            }
            McpTransportType::Stdio => {
                let command = transport.command.clone().unwrap_or_default();
                let args = transport.args.iter().map(|a| subst(a)).collect();
                let mut cfg = McpManagerConfig::stdio(&self.id, &self.name, command)
                    .with_args(args)
                    .with_env(env.clone());
                if let Some(rt) = &transport.requires_runtime {
                    cfg = cfg.with_runtime(rt.clone());
                }
                cfg
            }
        }
    }

    /// Plan an install: detect already-installed, missing keys, runtime gaps,
    /// or produce a ready-to-start config (first transport whose runtime is
    /// available; remote transports have no runtime so are always eligible).
    pub fn plan_install(
        &self,
        provided: &HashMap<String, String>,
        existing_ids: &[String],
        is_runtime_available: &dyn Fn(&str) -> bool,
    ) -> InstallPlan {
        if existing_ids.iter().any(|id| id == &self.id) {
            return InstallPlan::AlreadyInstalled;
        }
        let missing = self.missing_required_env(provided);
        if !missing.is_empty() {
            return InstallPlan::NeedsKey(missing);
        }
        let env = self.effective_env(provided);
        for transport in &self.transports {
            match &transport.requires_runtime {
                None => return InstallPlan::Ready(Box::new(self.materialize(transport, &env))),
                Some(rt) if is_runtime_available(rt) => {
                    return InstallPlan::Ready(Box::new(self.materialize(transport, &env)));
                }
                Some(_) => continue,
            }
        }
        let runtime = self
            .transports
            .iter()
            .filter_map(|t| t.requires_runtime.clone())
            .next()
            .unwrap_or_default();
        InstallPlan::NoRuntime(runtime)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_catalog_parses_and_has_first_batch() {
        let all = catalog();
        // 内置 5 个官方预设 id 必须都在
        for id in ["context7", "amap", "minimax", "volcengine-veimagex", "siliconflow"] {
            assert!(find(id).is_some(), "missing preset: {id}");
        }
        // id 唯一
        let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
        ids.sort_unstable();
        let mut dedup = ids.clone();
        dedup.dedup();
        assert_eq!(ids, dedup, "duplicate preset id in catalog.json");
        // 每个 preset 至少一个 transport
        assert!(all.iter().all(|p| !p.transports.is_empty()));
    }

    #[test]
    fn amap_requires_secret_key_and_has_remote_first() {
        let amap = find("amap").expect("amap present");
        assert!(amap
            .required_env
            .iter()
            .any(|e| e.key == "AMAP_MAPS_API_KEY" && e.secret && e.required));
        // 远程优先：第一个 transport 是 http 且 url 含 key 占位
        let first = &amap.transports[0];
        assert_eq!(first.kind, McpTransportType::Http);
        assert!(first
            .url
            .as_deref()
            .unwrap()
            .contains("<AMAP_MAPS_API_KEY>"));
    }

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn needs_key_when_required_secret_missing() {
        let amap = find("amap").unwrap();
        let plan = amap.plan_install(&HashMap::new(), &[], &|_| true);
        match plan {
            InstallPlan::NeedsKey(missing) => {
                assert!(missing.iter().any(|e| e.key == "AMAP_MAPS_API_KEY"));
            }
            other => panic!("expected NeedsKey, got {other:?}"),
        }
    }

    #[test]
    fn already_installed_when_id_present() {
        let p = find("context7").unwrap();
        let plan = p.plan_install(&HashMap::new(), &["context7".to_string()], &|_| true);
        assert!(matches!(plan, InstallPlan::AlreadyInstalled));
    }

    #[test]
    fn amap_remote_first_substitutes_key_into_url() {
        let amap = find("amap").unwrap();
        let provided = env(&[("AMAP_MAPS_API_KEY", "k-123")]);
        // 远程无 runtime 要求 → 选第一个 http transport
        let plan = amap.plan_install(&provided, &[], &|_| true);
        let cfg = match plan {
            InstallPlan::Ready(cfg) => *cfg,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(cfg.transport, McpTransportType::Http);
        assert_eq!(
            cfg.url.as_deref(),
            Some("https://mcp.amap.com/mcp?key=k-123")
        );
        assert_eq!(cfg.id, "amap");
    }

    #[test]
    fn no_runtime_when_only_transport_runtime_unavailable() {
        // minimax 只有 stdio(python)；提供 key 后 python 不可用 → NoRuntime
        let m = find("minimax").unwrap();
        let provided = env(&[("MINIMAX_API_KEY", "mk")]);
        let plan = m.plan_install(&provided, &[], &|rt| rt != "python");
        match plan {
            InstallPlan::NoRuntime(rt) => assert_eq!(rt, "python"),
            other => panic!("expected NoRuntime, got {other:?}"),
        }
    }

    #[test]
    fn minimax_applies_default_host() {
        let m = find("minimax").unwrap();
        // 只填 secret key；MINIMAX_API_HOST 用 default
        let provided = env(&[("MINIMAX_API_KEY", "mk")]);
        let cfg = match m.plan_install(&provided, &[], &|_| true) {
            InstallPlan::Ready(cfg) => *cfg,
            other => panic!("expected Ready, got {other:?}"),
        };
        assert_eq!(
            cfg.env.get("MINIMAX_API_HOST").map(String::as_str),
            Some("https://api.minimaxi.com")
        );
        assert_eq!(
            cfg.env.get("MINIMAX_API_KEY").map(String::as_str),
            Some("mk")
        );
        assert_eq!(cfg.requires_runtime.as_deref(), Some("python"));
    }
}
