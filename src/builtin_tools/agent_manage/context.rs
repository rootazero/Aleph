//! Shared construction-time context for the agent-management tools.
//!
//! [`AgentManageContext`] bundles every dependency the create / list / delete
//! / switch / unbind / update / info tools need. Splitting "what each tool
//! has access to" from "how it talks to the world" makes the optional
//! dependencies (event bus, TOML manager, raw-memory writer) explicit at the
//! type level: each tool takes only the slice it actually uses, and the
//! constructor in `executor::builtin_registry::builder::constructor::agent_acp_tools`
//! doesn't have to know which tool needs which slot.
//!
//! The split also keeps the test surface narrow: a builder that returns a
//! fully-wired context has to be exercised end-to-end, but each tool's unit
//! test only constructs what *it* needs via the targeted constructors
//! (`for_create_only`, `for_delete_only`, etc.).
//!
//! Mirrors the seam in `gateway::agent_binding` (single source for the
//! bind / unbind operations): this module owns the **construction seam**,
//! that one owns the **persistence seam**.

use crate::config::agent_manager::AgentManager;
use crate::gateway::agent_env::AgentEnvStore;
use crate::gateway::agent_instance::AgentRegistry;
use crate::gateway::event_bus::GatewayEventBus;
use crate::memory::store::raw_memory::RawMemoryStore;
use crate::sync_primitives::Arc;

/// Every dependency the agent-management toolset could possibly need.
///
/// Construction is intentionally permissive — every field except the runtime
/// [`runtime_registry`] and the [`store`] is `Option`-tagged. Tests, embedded
/// hosts, and minimal servers wire only what they need; production wires it
/// all.
#[derive(Clone)]
pub struct AgentManageContext {
    /// Runtime instance registry (lazy `Config` + on-demand `Instance`).
    /// Required for create / list / switch / delete / unbind; `agent_info`
    /// does **not** touch it (uses the catalog registry instead).
    pub runtime_registry: Arc<AgentRegistry>,

    /// Per-channel active-agent store (`channel_active_agent` SQLite table).
    /// Required for switch / unbind / list's `bound_channels` view.
    pub store: Arc<AgentEnvStore>,

    /// TOML definition manager (`[[agents.list]]` in the config file).
    /// Optional: when wired, `create` / `delete` / `update` persist across
    /// restarts; when absent (tests / embedded) the registry-only path is
    /// used and the agent is lost on the next boot.
    pub agent_manager: Option<Arc<AgentManager>>,

    /// Gateway event bus for [`crate::gateway::agent_lifecycle::AgentLifecycleEvent`]
    /// delivery. Optional: when absent, lifecycle events are dropped silently
    /// (matching the historical pre-event behavior — no subscriber means
    /// no observable change).
    pub event_bus: Option<Arc<GatewayEventBus>>,

    /// L0 raw-memory writer for `agent_create`. Optional: when absent, the
    /// new agent will not capture L0 transcript rows until gateway startup
    /// wires the registry-global writer.
    pub raw_memory_writer: Option<Arc<dyn RawMemoryStore>>,
}

impl AgentManageContext {
    /// Build a context with only the runtime registry + store. Sufficient
    /// for `agent_list` and (after `.with_*` calls) the read-only surfaces
    /// of `agent_info`.
    #[must_use]
    pub fn new(runtime_registry: Arc<AgentRegistry>, store: Arc<AgentEnvStore>) -> Self {
        Self {
            runtime_registry,
            store,
            agent_manager: None,
            event_bus: None,
            raw_memory_writer: None,
        }
    }

    /// Wire the TOML definition manager (builder pattern).
    #[must_use]
    pub fn with_agent_manager(mut self, manager: Arc<AgentManager>) -> Self {
        self.agent_manager = Some(manager);
        self
    }

    /// Wire the gateway event bus (builder pattern).
    #[must_use]
    pub fn with_event_bus(mut self, bus: Option<Arc<GatewayEventBus>>) -> Self {
        self.event_bus = bus;
        self
    }

    /// Wire the L0 raw-memory writer (builder pattern).
    #[must_use]
    pub fn with_raw_memory_writer(mut self, writer: Arc<dyn RawMemoryStore>) -> Self {
        self.raw_memory_writer = Some(writer);
        self
    }
}
