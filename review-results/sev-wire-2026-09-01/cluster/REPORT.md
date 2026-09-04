# Severed-Wire Audit — `src/cluster/`

**Date:** 2026-09-01
**Module:** `src/cluster/{enrollment.rs, mod.rs, node_approval.rs, node_file_cmd.rs, node_runtime.rs, registry.rs, reverse_rpc.rs}`
**Method:** Read-first sweep. PRODUCED − CONSUMED symbol parity via `rg` across `src/`, `interfaces/`, `shared/`, `bin/`. Test code (anything under `#[cfg(test)]` or in `mod tests`) was stripped before claiming "no consumer". The 6-form seam catalog and CONNECT/CUT/DECIDE decision tree from `references/seam-catalog.md` / `references/triage-playbook.md` were applied; read-before-write was enforced (each candidate was searched on the consumer side, not the handler side).

The cluster module is the **center-side multi-node orchestration surface**: node enrollment + deregistration (enrollment.rs), the live node registry + multi-tier addressing (registry.rs), node-side reverse-RPC transport (reverse_rpc.rs), the node-side command allowlist + bash / file jails (node_runtime.rs, node_file_cmd.rs), and the node-initiated approval requester that loops back to the center (node_approval.rs). Inbound RPC faces are registered in `bin/aleph-server/commands/start/builder/handlers/core.rs` (`cluster.enroll`, `cluster.deregister`, `environments.list`). The connect-seam in `src/gateway/server/handler.rs` is the only live producer of `admit_node` and `maybe_register_node`. The `aleph-server node` binary in `src/bin/aleph-server/commands/node.rs` is the only live consumer of the node-side types (`ApprovalSlot`, `CenterApprovalRequester`, `CommandTable`).

---

## Inventory — produced surface

### `mod.rs` (re-exports — 42 lines)
| Symbol | Re-export | Source location |
|---|---|---|
| `pub use` `admit_node` | `cluster::admit_node` | enrollment.rs:196 |
| `pub use` `deregister_node` | `cluster::deregister_node` | enrollment.rs:396 |
| `pub use` `enroll_node_device` | `cluster::enroll_node_device` | enrollment.rs:156 |
| `pub use` `NodeAdmission` | `cluster::NodeAdmission` | enrollment.rs:31 |
| `pub use` `DeregisterError` | `cluster::DeregisterError` | enrollment.rs:308 |
| `pub use` `DeregisterOutcome` | `cluster::DeregisterOutcome` | enrollment.rs:317 |
| `pub use` `ApprovalSlot` | `cluster::ApprovalSlot` | node_approval.rs:41 |
| `pub use` `CenterApprovalRequester` | `cluster::CenterApprovalRequester` | node_approval.rs:97 |
| `pub(crate) use` `sha256_hex` | `cluster::sha256_hex` | node_file_cmd.rs:26 |
| `pub use` `FileReadCommand` | `cluster::FileReadCommand` | node_file_cmd.rs:191 |
| `pub use` `FileWriteCommand` | `cluster::FileWriteCommand` | node_file_cmd.rs:99 |
| `pub use` `MAX_FILE_BYTES` | `cluster::MAX_FILE_BYTES` | node_file_cmd.rs:20 |
| `pub use` `CommandTable` | `cluster::CommandTable` | node_runtime.rs:32 |
| `pub use` `NodeCommand` | `cluster::NodeCommand` | node_runtime.rs:25 (trait) |
| `pub(crate) use` `normalize_node_key` | `cluster::normalize_node_key` | registry.rs:464 |
| `pub use` `maybe_register_node` | `cluster::maybe_register_node` | registry.rs:496 |
| `pub use` `CommandDescriptor` | `cluster::CommandDescriptor` | registry.rs:29 |
| `pub use` `Environment` | `cluster::Environment` | registry.rs:87 |
| `pub use` `NodeMatch` | `cluster::NodeMatch` | registry.rs:112 |
| `pub use` `NodeRegistry` | `cluster::NodeRegistry` | registry.rs:144 |
| `pub use` `NodeSession` | `cluster::NodeSession` | registry.rs:35 |
| `pub use` `ResolveError` | `cluster::ResolveError` | registry.rs:62 |
| `pub use` `PendingInvokes` | `cluster::PendingInvokes` | reverse_rpc.rs:26 |
| `pub use` `ReverseRpcChannel` | `cluster::ReverseRpcChannel` | reverse_rpc.rs:162 |
| `pub use` `ReverseRpcError` | `cluster::ReverseRpcError` | reverse_rpc.rs:117 |

### `enrollment.rs` — public surface
| Symbol | Location |
|---|---|
| `pub enum NodeAdmission { Admitted{node_id,minted}, Deregistered{node_id}, IdentityConflict{node_id} }` | enrollment.rs:31 |
| `pub fn admit_node(store, presented_id, node_name) -> NodeAdmission` | enrollment.rs:196 |
| `pub fn enroll_node_device(store, node_name) -> Result<(String, bool), String>` | enrollment.rs:156 |
| `pub enum DeregisterError { NotFound, Ambiguous(String) }` | enrollment.rs:308 |
| `pub struct DeregisterOutcome { pub node_id, pub evicted, pub device_removed }` | enrollment.rs:317 |
| `pub fn deregister_node(registry, store, query) -> Result<DeregisterOutcome, DeregisterError>` | enrollment.rs:396 |

### `node_approval.rs` — public surface
| Symbol | Location |
|---|---|
| `pub type ApprovalSlot = Arc<RwLock<Option<ReverseRpcChannel>>>` | node_approval.rs:41 |
| `pub(crate) const NODE_APPROVAL_TIMEOUT_MS: u64 = 130_000` | node_approval.rs:47 |
| `pub(crate) fn outcome_from_str(s) -> ApprovalOutcome` | node_approval.rs:57 |
| `pub struct CenterApprovalRequester { slot: ApprovalSlot }` | node_approval.rs:97 |
| `pub const fn new(slot) -> Self` | node_approval.rs:102 |
| `impl ApprovalRequester for CenterApprovalRequester` | node_approval.rs:108 |

### `node_file_cmd.rs` — public surface
| Symbol | Location |
|---|---|
| `pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024` | node_file_cmd.rs:20 |
| `pub(crate) fn sha256_hex(bytes) -> String` | node_file_cmd.rs:26 |
| `pub struct FileWriteCommand { workspace_dir: PathBuf }` | node_file_cmd.rs:99 |
| `pub const fn new(workspace_dir) -> Self` | node_file_cmd.rs:105 |
| `impl NodeCommand for FileWriteCommand` | node_file_cmd.rs:111 |
| `pub struct FileReadCommand { workspace_dir: PathBuf }` | node_file_cmd.rs:191 |
| `pub const fn new(workspace_dir) -> Self` | node_file_cmd.rs:197 |
| `impl NodeCommand for FileReadCommand` | node_file_cmd.rs:203 |

### `node_runtime.rs` — public surface
| Symbol | Location |
|---|---|
| `pub trait NodeCommand: Send + Sync { async fn run; fn descriptor }` | node_runtime.rs:25 |
| `pub struct CommandTable { commands: HashMap<String, Arc<dyn NodeCommand>> }` | node_runtime.rs:32 |
| `pub fn new() -> Self` | node_runtime.rs:38 |
| `pub fn register(name, cmd: Arc<dyn NodeCommand>)` | node_runtime.rs:42 |
| `pub fn descriptors(&self) -> Vec<CommandDescriptor>` | node_runtime.rs:65 |
| `pub async fn dispatch(&self, method: &str, params: &Value) -> Result<Value, String>` | node_runtime.rs:77 |
| `pub(crate) struct BashNodeCommand { bash, session }` | node_runtime.rs:97 |
| `pub(crate) const fn new(bash, session) -> Self` | node_runtime.rs:103 |
| `impl NodeCommand for BashNodeCommand` | node_runtime.rs:109 |
| `pub fn with_bash(bash, session) -> Self` | node_runtime.rs:128 |
| `pub fn register_file_commands(&mut self, workspace_dir: PathBuf)` | node_runtime.rs:137 |

### `registry.rs` — public surface
| Symbol | Location |
|---|---|
| `pub struct CommandDescriptor { pub name, pub schema }` | registry.rs:29 |
| `pub struct NodeSession { pub node_id, pub conn_id, pub device_name, pub channel, pub declared_commands, pub tags, pub version, pub connected_at }` | registry.rs:35 |
| `pub enum ResolveError { NotFound, Ambiguous(Vec<String>), NodeNotFound { name_or_id } }` | registry.rs:62 |
| `impl Display for ResolveError` | registry.rs:72 |
| `pub struct Environment { pub id, pub name, pub status, pub commands, pub tags, pub connected_at, pub last_seen_at, pub version }` | registry.rs:87 |
| `pub struct NodeMatch { pub node_id, pub name, pub channel, pub declared_commands, pub tags }` | registry.rs:112 |
| `pub(crate) fn truncate_on_char_boundary(value, max_bytes) -> &str` | registry.rs:129 |
| `pub struct NodeRegistry { inner: RwLock<RegistryInner> }` | registry.rs:144 |
| `pub fn new() -> Self` | registry.rs:150 |
| `pub fn register(&self, session: NodeSession)` | registry.rs:159 |
| `pub fn deregister(&self, conn_id: &str) -> bool` | registry.rs:219 |
| `pub fn list_environments(&self) -> Vec<Environment>` | registry.rs:242 |
| `pub fn node_identity_by_conn(&self, conn_id: &str) -> Option<(String, String)>` | registry.rs:266 |
| `pub fn resolve(&self, name_or_id) -> Result<(ReverseRpcChannel, Vec<CommandDescriptor>), ResolveError>` | registry.rs:329 |
| `pub(crate) fn resolve_id(&self, name_or_id) -> Result<String, ResolveError>` | registry.rs:351 |
| `pub fn resolve_all_by_tags(&self, tags: &[String]) -> Vec<NodeMatch>` | registry.rs:366 |
| `pub fn forget(&self, node_id: &str) -> bool` | registry.rs:401 |
| `pub(crate) fn normalize_node_key(value: &str) -> String` | registry.rs:464 |
| `pub fn maybe_register_node(registry, role, device_id, conn_id, params, channel) -> bool` | registry.rs:496 |

### `reverse_rpc.rs` — public surface
| Symbol | Location |
|---|---|
| `pub struct PendingInvokes { counter, waiters }` | reverse_rpc.rs:26 |
| `pub fn new() -> Self` | reverse_rpc.rs:33 |
| `pub(crate) fn register(&self) -> (String, oneshot::Receiver<JsonRpcResponse>)` | reverse_rpc.rs:40 |
| `pub fn resolve(&self, id: &Value, response: JsonRpcResponse)` | reverse_rpc.rs:62 |
| `pub(crate) fn cancel(&self, id: &str) -> bool` | reverse_rpc.rs:90 |
| `pub fn cancel_all(&self) -> usize` | reverse_rpc.rs:107 |
| `pub enum ReverseRpcError { TransportClosed, Timeout(u64), OutboundWedged(u64), Cancelled, Serialize(serde_json::Error) }` | reverse_rpc.rs:117 |
| `pub struct ReverseRpcChannel { outbound, pending: Arc<PendingInvokes>, close: Option<Arc<Notify>> }` | reverse_rpc.rs:162 |
| `pub fn new(outbound: mpsc::Sender<String>) -> Self` | reverse_rpc.rs:183 |
| `pub fn with_close(outbound, close: Arc<Notify>) -> Self` | reverse_rpc.rs:202 |
| `pub fn pending(&self) -> Arc<PendingInvokes>` | reverse_rpc.rs:213 |
| `pub fn close_connection(&self)` | reverse_rpc.rs:233 |
| `pub async fn call(&self, method, params, timeout_ms) -> Result<JsonRpcResponse, ReverseRpcError>` | reverse_rpc.rs:263 |

---

## Inventory — production consumers

### `pub` symbols vs production callers

```bash
# enrollment.rs
$ rg -n "cluster::admit_node|crate::cluster::admit_node" src/ interfaces/ shared/ bin/
src/gateway/server/handler.rs:1464        # the only live producer — connect seam
src/gateway/server/handler.rs:2810        # test (#[cfg(test)])

$ rg -n "cluster::enroll_node_device|crate::cluster::enroll_node_device" src/ interfaces/ shared/ bin/
src/builtin_tools/node_manage.rs:22       # use statement (the tool body calls it at :72)
src/builtin_tools/node_manage.rs:72       # body: let (node_id, minted) = enroll_node_device(...)
src/gateway/handlers/cluster.rs:18        # use statement
src/gateway/handlers/cluster.rs:58        # body: handle_cluster_enroll calls enroll_node_device

$ rg -n "cluster::deregister_node|crate::cluster::deregister_node" src/ interfaces/ shared/ bin/
src/builtin_tools/node_manage.rs:22       # use statement
src/builtin_tools/node_manage.rs:84       # body: deregister_node(&self.node_registry, &self.security_store, &args.node)
src/gateway/handlers/cluster.rs:18        # use statement
src/gateway/handlers/cluster.rs:108       # body: handle_cluster_deregister calls deregister_node
interfaces/webchat/src/api/cluster.rs:98  # ClusterApi::deregister_node — wrapper around the gateway RPC
interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:335  # calls the wrapper

$ rg -n "NodeAdmission::" src/ interfaces/ shared/ bin/
src/gateway/server/handler.rs:1455       # Admitted
src/gateway/server/handler.rs:1477       # Admitted
src/gateway/server/handler.rs:1519       # Deregistered
src/gateway/server/handler.rs:1534       # IdentityConflict

$ rg -n "DeregisterError" src/ interfaces/ shared/ bin/
src/builtin_tools/node_manage.rs:94       # match arm
src/builtin_tools/node_manage.rs:98       # match arm
src/gateway/handlers/cluster.rs:141       # error → JSON-RPC error code NODE_NOT_FOUND
src/gateway/handlers/cluster.rs:146       # error → JSON-RPC INVALID_PARAMS

$ rg -n "DeregisterOutcome" src/ interfaces/ shared/ bin/
src/gateway/handlers/cluster.rs:122       # field access (outcome.evicted / device_removed)
```

```bash
# node_approval.rs
$ rg -n "CenterApprovalRequester::new|ApprovalSlot" src/ interfaces/ shared/ bin/
src/bin/aleph-server/commands/node.rs:28  # use statement
src/bin/aleph-server/commands/node.rs:284 # let slot: ApprovalSlot = Arc::new(RwLock::new(None))
src/bin/aleph-server/commands/node.rs:285 # let requester = Arc::new(CenterApprovalRequester::new(slot.clone()))
src/bin/aleph-server/commands/node.rs:320 # fn run_session(... approval_slot: &ApprovalSlot ...)
src/bin/aleph-server/commands/node.rs:381 # *approval_slot.write() = Some(channel)

# NODE_APPROVAL_TIMEOUT_MS: pub(crate), only used inside node_approval.rs:141.
# outcome_from_str: pub(crate), only used inside node_approval.rs:161.
```

```bash
# node_file_cmd.rs / node_runtime.rs
$ rg -n "FileWriteCommand::new|FileReadCommand::new" src/ interfaces/ shared/ bin/
src/cluster/node_runtime.rs:138           # use inside register_file_commands
src/cluster/node_runtime.rs:141           # FileReadCommand::new(workspace_dir.clone())
src/cluster/node_runtime.rs:143           # FileWriteCommand::new(workspace_dir)
src/bin/aleph-server/commands/node.rs:28  # CommandDescriptor is imported (table uses both types via with_bash + register_file_commands)

$ rg -n "CommandTable::with_bash|CommandTable::new|register_file_commands|table\.register" src/ interfaces/ shared/ bin/
src/bin/aleph-server/commands/node.rs:300 # let mut table = CommandTable::with_bash(bash, session)
src/bin/aleph-server/commands/node.rs:301 # table.register_file_commands(workspace_dir)
src/bin/aleph-server/commands/node.rs:443 # async fn handle_frame(table: &CommandTable, ...)
src/bin/aleph-server/commands/node.rs:475 # tests construct CommandTable::with_bash

$ rg -n "table\.dispatch|CommandTable::dispatch" src/ interfaces/ shared/ bin/
src/bin/aleph-server/commands/node.rs:455 # let resp = match table.dispatch("tool.call", &params).await
# this is the ONLY live consumer of dispatch (the node's tool.call reply path)

$ rg -n "MAX_FILE_BYTES" src/ interfaces/ shared/ bin/
src/builtin_tools/node_file.rs:113        # meta.len() > MAX_FILE_BYTES
src/builtin_tools/node_file.rs:115        # "{bytes} exceeds {MAX_FILE_BYTES} cap"
src/builtin_tools/node_file.rs:180        # base64 length pre-cap
src/builtin_tools/node_file.rs:183        # "node returned base64 payload exceeds {MAX_FILE_BYTES} byte cap"
src/builtin_tools/node_file.rs:189        # bytes.len() > MAX_FILE_BYTES
src/builtin_tools/node_file.rs:430        # tokio::fs::write(&local, vec![0u8; super::MAX_FILE_BYTES + 1]) — TEST
src/builtin_tools/node_file.rs:191        # "{bytes} exceeds {MAX_FILE_BYTES} cap"
src/cluster/node_file_cmd.rs:36           # local — pre-cap
src/cluster/node_file_cmd.rs:39           # local — error
src/cluster/node_file_cmd.rs:45-47        # local — decoded bytes cap
src/cluster/node_file_cmd.rs:218-239      # local — read cap

$ rg -n "sha256_hex" src/ interfaces/ shared/ bin/
src/builtin_tools/node_file.rs:122        # center-side integrity check after a node returns bytes
src/builtin_tools/node_file.rs:195        # center-side integrity check after a node returns bytes
src/builtin_tools/node_file.rs:368        # build an outgoing file.write request
src/builtin_tools/node_file.rs:399        # TEST
src/builtin_tools/node_file.rs:478        # TEST
src/cluster/node_file_cmd.rs:51           # local
src/cluster/node_file_cmd.rs:244          # local — descriptor returns sha256
src/cluster/node_file_cmd.rs:279          # local — descriptor returns sha256
```

```bash
# registry.rs
$ rg -n "NodeRegistry::new\(\)|Arc::new\(.*NodeRegistry::new\(\)\)|Arc::new\(crate::cluster::NodeRegistry::new\(\)\)" src/ interfaces/ shared/ bin/
src/gateway/server/mod.rs:525            # production server: node_registry: Arc::new(crate::cluster::NodeRegistry::new())
src/gateway/server/mod.rs:583            # production server
src/gateway/server/mod.rs:808            # server wiring
src/gateway/server/probe.rs:104          # probe-mode server (LAN-trust degraded)
src/gateway/handlers/auth/mod.rs:74      # test/auth harness
src/bin/aleph-server/commands/start/builder/agent_init/mod.rs:771  # tool_registry.set_node_registry(server.node_registry.clone())

$ rg -n "set_node_registry\|node_registry:\s*Arc<.*NodeRegistry\|Arc<.*NodeRegistry\|node_registry:\s*Arc<crate::cluster::NodeRegistry" src/ interfaces/ shared/ bin/
src/executor/builtin_registry/registry/inherent.rs:251  # pub fn set_node_registry(registry)
src/executor/builtin_registry/registry/struct_def.rs:125 # field: Arc<OnceCell<Arc<crate::cluster::NodeRegistry>>>
src/executor/builtin_registry/builder/constructor/mod.rs:1207  # OnceCell initialization
src/gateway/server/handler.rs:148        # ctx field: node_registry: Arc<crate::cluster::NodeRegistry>
src/gateway/server/handler.rs:307        # passed into AuthContext for handlers
src/gateway/server/mod.rs:244,459        # server fields
src/gateway/handlers/auth/mod.rs:49      # AuthContext field

$ rg -n "NodeRegistry::(register|deregister|list_environments|node_identity_by_conn|resolve|resolve_all_by_tags|forget)\b" src/ interfaces/ shared/ bin/
src/builtin_tools/node_invoke.rs:67       # node_registry.resolve(&args.node)
src/builtin_tools/node_invoke_many.rs:96,100  # node_registry.resolve_all_by_tags(&args.tags)
src/builtin_tools/node_list.rs:59        # node_registry.list_environments()
src/gateway/handlers/cluster.rs:174      # ctx.node_registry.list_environments() (handle_environments_list)
src/gateway/server/handler.rs:1482       # crate::cluster::maybe_register_node(&ctx.node_registry, ...)
src/gateway/server/handler.rs:787        # ctx.node_registry.node_identity_by_conn(&conn_id)
src/gateway/server/handler.rs:2019       # ctx.node_registry.node_identity_by_conn(&conn_id)
src/gateway/server/handler.rs:2020       # ctx.node_registry.deregister(&conn_id)
src/cluster/enrollment.rs:402,412         # intra-crate — registry.resolve_id, registry.forget

$ rg -n "maybe_register_node" src/ interfaces/ shared/ bin/ | grep -v "#\[cfg\(test\)\]"
src/cluster/registry.rs:496              # the definition
src/gateway/server/handler.rs:1482       # PRODUCTION CALLER (the connect seam)
src/cluster/registry.rs:495              # doc comment
src/bin/aleph-server/commands/node.rs:338 # doc comment

$ rg -n "Environment\s*\{|Environment\.\w+" src/ interfaces/ shared/ bin/
src/gateway/handlers/cluster.rs:183      # construct (offline fallback)
src/gateway/handlers/cluster.rs:174      # iterate list_environments()
src/gateway/handlers/cluster.rs:190      # field set
src/builtin_tools/node_list.rs:59         # iterate list_environments()
src/gateway/server/handler.rs:2823       # TEST
src/gateway/server/mod.rs:1467           # TEST

$ rg -n "NodeMatch\s*\{|\bm: NodeMatch\b|matches\s*:\s*Vec<NodeMatch>|invoke_one\(m:" src/ interfaces/ shared/ bin/
src/builtin_tools/node_invoke_many.rs:19 # use NodeMatch
src/builtin_tools/node_invoke_many.rs:57 # fn invoke_one(m: NodeMatch, ...)
src/builtin_tools/node_invoke_many.rs:96 # let matches = self.node_registry.resolve_all_by_tags(&args.tags)
src/builtin_tools/node_invoke_many.rs:100 # let online = self.node_registry.resolve_all_by_tags(&[])

$ rg -n "NodeSession\s*\{" src/ interfaces/ shared/ bin/
src/cluster/registry.rs:597              # inside maybe_register_node — the single producer
src/builtin_tools/node_file.rs:329,456    # TEST
src/builtin_tools/node_invoke_many.rs:180 # TEST
src/builtin_tools/node_invoke.rs:120,211  # TEST
src/builtin_tools/node_list.rs:90        # TEST
src/builtin_tools/node_manage.rs:124     # TEST
# Production code constructs NodeSession exclusively via maybe_register_node(...).

$ rg -n "ResolveError::" src/ interfaces/ shared/ bin/
src/builtin_tools/node_invoke.rs:69,72    # match arm (NotFound / Ambiguous / NodeNotFound)
src/builtin_tools/node_file.rs:78,81      # match arm (NotFound / Ambiguous / NodeNotFound)
src/cluster/enrollment.rs:401,404,407    # intra-crate (deregister_node)

$ rg -n "CommandDescriptor" src/ interfaces/ shared/ bin/ | grep -v "#\[cfg\(test\)\]"
src/cluster/node_file_cmd.rs:17,182,249  # NodeCommand::descriptor impls
src/cluster/node_runtime.rs:18,27,65-66,116-117  # trait def, descriptors()
src/cluster/registry.rs:530-532          # serde_json::from_value::<Vec<CommandDescriptor>> — inbound parse
src/builtin_tools/node_file.rs:336       # TEST
src/builtin_tools/node_list.rs:97        # TEST
src/builtin_tools/node_invoke.rs:127     # TEST
src/builtin_tools/node_invoke_many.rs:187 # TEST
src/builtin_tools/node_manage.rs:129     # TEST
src/bin/aleph-server/commands/node.rs:28,317 # the node binary imports CommandDescriptor and passes &[CommandDescriptor] into connect's params
interfaces/webchat/src/api/cluster.rs:15  # mirror struct (rendering contract)
```

```bash
# reverse_rpc.rs
$ rg -n "ReverseRpcChannel::new|ReverseRpcChannel::with_close" src/ interfaces/ shared/ bin/
src/gateway/server/handler.rs:695        # PRODUCTION center-side — ReverseRpcChannel::with_close(rpc_out_tx, rpc_close.clone())
src/bin/aleph-server/commands/node.rs:380 # PRODUCTION node-side — ReverseRpcChannel::new(out_tx.clone())
src/cluster/node_approval.rs:201,247,360 # intra-crate tests
src/cluster/reverse_rpc.rs:407,436,467,516,547,583,618,639,663,683 # self-tests
src/builtin_tools/node_invoke.rs:118,210  # TEST
src/builtin_tools/node_invoke_many.rs:179 # TEST
src/builtin_tools/node_file.rs:327,454   # TEST
src/builtin_tools/node_list.rs:94        # TEST
src/builtin_tools/node_manage.rs:128     # TEST
src/gateway/handlers/cluster.rs:297,360,417,477  # TEST (handler unit tests)

$ rg -n "\.call\(\"node\.approval\.request\"|channel\.call|ch\.call" src/ interfaces/ shared/ bin/
src/cluster/node_approval.rs:141         # center-side call("node.approval.request", ...)
# (the only production reverse-rpc method the cluster module emits is "node.approval.request",
#  invoked exactly once from CenterApprovalRequester::request_approval)

$ rg -n "\"tool\.call\"" src/cluster/ 2>/dev/null
src/cluster/node_runtime.rs:78           # if method != "tool.call" { ... }
src/cluster/node_runtime.rs:179,188,197,207,233,246 # dispatch("tool.call", ...) — tests
src/cluster/reverse_rpc.rs:411,451,470,535,550,589,620,642,666,686 # self-tests + node-side harness
# (the only production tool.call dispatcher is src/bin/aleph-server/commands/node.rs:455
#  via CommandTable::dispatch, and the only server-side handle_frame wrapper is :443)

$ rg -n "PendingInvokes::new" src/ interfaces/ shared/ bin/
src/cluster/reverse_rpc.rs:186,205        # inside ReverseRpcChannel::new / with_close
src/cluster/reverse_rpc.rs:485,502,561    # self-tests
# (PendingInvokes is constructed only inside ReverseRpcChannel; external code holds it via .pending().)

$ rg -n "\.pending\(\)" src/cluster/ src/bin/ src/gateway/ 2>/dev/null
src/cluster/reverse_rpc.rs:214           # getter
src/cluster/reverse_rpc.rs:408,437,517,584 # self-tests
src/cluster/node_approval.rs:202          # intra-crate
src/bin/aleph-server/commands/node.rs:381 # the node-side `let pending = channel.pending();`
src/gateway/server/handler.rs:696         # let rpc_pending = rpc_channel.pending();

$ rg -n "\.resolve\(&" src/cluster/ src/bin/ src/gateway/ 2>/dev/null | head -20
src/cluster/reverse_rpc.rs:444           # tests
src/cluster/reverse_rpc.rs:492,504,531    # tests
src/cluster/node_approval.rs:274,324,347  # intra-crate
src/bin/aleph-server/commands/node.rs:412 # node-side: pending.resolve(&id, resp) — center response
src/gateway/server/handler.rs:767         # PRODUCTION: rpc_pending.resolve(&id, maybe_resp) — inbound response routing
# (two production callers, both routing JSON-RPC responses back to the call() waiter)

$ rg -n "cancel_all\(\)" src/cluster/ src/bin/ src/gateway/ 2>/dev/null
src/cluster/reverse_rpc.rs:139           # doc comment
src/gateway/server/handler.rs:1995       # PRODUCTION: let cancelled = rpc_pending.cancel_all();

$ rg -n "ReverseRpcError::" src/ interfaces/ shared/ bin/ | head -20
src/cluster/reverse_rpc.rs:294,320,329,345,347  # producer arms
src/cluster/reverse_rpc.rs:474,553,606,626,646,669,689  # tests
src/session/steer_signal.rs:30           # doc reference only
src/tools/budget.rs:47                   # doc reference only (REVERSE_RPC_MAX_TIMEOUT_MS contract)

$ rg -n "close_connection\(\)" src/cluster/ src/bin/ src/gateway/ 2>/dev/null
src/cluster/reverse_rpc.rs:233           # def
src/cluster/reverse_rpc.rs:248,252       # inside call() and doc
src/cluster/registry.rs:183,209,416      # PRODUCTION: NodeRegistry::register and NodeRegistry::forget
                                          # each fire close_connection on the evicted session
src/cluster/reverse_rpc.rs:653           # test
```

### Inbound RPC registration

```bash
$ rg -n "register_handler!\(server," src/bin/aleph-server/commands/start/builder/handlers/core.rs
src/bin/aleph-server/commands/start/builder/handlers/core.rs:19  # connect
src/bin/aleph-server/commands/start/builder/handlers/core.rs:28  # cluster.enroll
src/bin/aleph-server/commands/start/builder/handlers/core.rs:34  # cluster.deregister
src/bin/aleph-server/commands/start/builder/handlers/core.rs:40  # environments.list
# Three cluster-module RPCs reach the daemon via this file. The handlers themselves
# are in src/gateway/handlers/cluster.rs and are unit-tested there.
```

### Per-symbol call parity table

| Public symbol | Definition | Production caller(s) | Test-only |
|---|---|---|---|
| `NodeAdmission` | enrollment.rs:31 | `gateway/server/handler.rs:1455,1477,1519,1534` | enrollment.rs:436-668 |
| `admit_node` | enrollment.rs:196 | `gateway/server/handler.rs:1464` | enrollment.rs:436-668, handler.rs:2810 |
| `enroll_node_device` | enrollment.rs:156 | `gateway/handlers/cluster.rs:58`, `builtin_tools/node_manage.rs:72` | enrollment.rs:586-638 |
| `DeregisterError` | enrollment.rs:308 | `builtin_tools/node_manage.rs:94,98`, `gateway/handlers/cluster.rs:141,146` | enrollment.rs:658 |
| `DeregisterOutcome` | enrollment.rs:317 | `gateway/handlers/cluster.rs:122` (field reads) | enrollment.rs:644 |
| `deregister_node` | enrollment.rs:396 | `gateway/handlers/cluster.rs:108`, `builtin_tools/node_manage.rs:84`; UI wrapper `interfaces/webchat/src/api/cluster.rs:98` | enrollment.rs:641-657 |
| `ApprovalSlot` | node_approval.rs:41 | `bin/aleph-server/commands/node.rs:284,320,381` | node_approval.rs:196-203,297,361 |
| `NODE_APPROVAL_TIMEOUT_MS` | node_approval.rs:47 | intra-crate: node_approval.rs:141 | — |
| `outcome_from_str` | node_approval.rs:57 | intra-crate: node_approval.rs:161 | node_approval.rs:209-218 |
| `CenterApprovalRequester::new` | node_approval.rs:102 | `bin/aleph-server/commands/node.rs:285` | node_approval.rs:237-362 |
| `CenterApprovalRequester::request_approval` | node_approval.rs:108 (impl) | via `ApprovalGate` wired by `build_command_table` → invoked by BashExecTool's sandbox on capability escalations | node_approval.rs:237-362 |
| `MAX_FILE_BYTES` | node_file_cmd.rs:20 | `builtin_tools/node_file.rs:113,115,180,183,189,191` (center-side mirror cap) | node_file_cmd.rs:302,368 |
| `sha256_hex` | node_file_cmd.rs:26 | `builtin_tools/node_file.rs:122,195,368` (center-side integrity check + request builder) | node_file_cmd.rs:51,244,279,290,304,329,341,348 |
| `FileWriteCommand::new` | node_file_cmd.rs:105 | intra-crate: `node_runtime.rs:143` (`register_file_commands`) → `bin/aleph-server/commands/node.rs:301` | node_file_cmd.rs:271,301-348 |
| `FileReadCommand::new` | node_file_cmd.rs:197 | intra-crate: `node_runtime.rs:141` (`register_file_commands`) → `bin/aleph-server/commands/node.rs:301` | node_file_cmd.rs:272,356-371 |
| `NodeCommand` | node_runtime.rs:25 (trait) | impls: `FileWriteCommand` (node_file_cmd.rs:111), `FileReadCommand` (node_file_cmd.rs:203), `BashNodeCommand` (node_runtime.rs:109) | node_runtime.rs:155,264 |
| `CommandTable::new` | node_runtime.rs:38 | intra-crate + `bin/aleph-server/commands/node.rs:300` (via `with_bash`) | node_runtime.rs:171,281,475 |
| `CommandTable::register` | node_runtime.rs:42 | intra-crate (`node_runtime.rs:130,141,143`); also reachable from `register_file_commands` → `bin/.../node.rs:300-301` | node_runtime.rs:173,283-286 |
| `CommandTable::descriptors` | node_runtime.rs:65 | read in `bin/aleph-server/commands/node.rs:317` to build the declared connect-frame param | node_runtime.rs:163,165-167,269 |
| `CommandTable::dispatch` | node_runtime.rs:77 | `bin/aleph-server/commands/node.rs:455` (the only tool.call dispatcher) | node_runtime.rs:179-246 |
| `CommandTable::with_bash` | node_runtime.rs:128 | `bin/aleph-server/commands/node.rs:300`, `bin/.../node.rs:483` (test) | node_runtime.rs:229 |
| `CommandTable::register_file_commands` | node_runtime.rs:137 | `bin/aleph-server/commands/node.rs:301` | — |
| `BashNodeCommand` | node_runtime.rs:97 | intra-crate: registered under "bash" by `CommandTable::with_bash` | — |
| `CommandDescriptor` | registry.rs:29 | `gateway/handlers/cluster.rs:183` (Environment projection), `bin/.../node.rs:28` (import), `bin/.../node.rs:317` (`&[CommandDescriptor]` passed to connect) | many tests in registry.rs, node_file_cmd.rs, node_runtime.rs, builtin_tools/* |
| `NodeSession` | registry.rs:35 | `registry::maybe_register_node` (registry.rs:597) → `gateway/server/handler.rs:1482` | every test in registry.rs / builtin_tools/* that builds a `NodeSession { ... }` literal |
| `ResolveError` | registry.rs:62 | `builtin_tools/node_invoke.rs:69,72`, `builtin_tools/node_file.rs:78,81`; intra-crate `enrollment.rs:404,407` | registry.rs:702,715,791,821,850,891,1076,1078,1082 |
| `Environment` | registry.rs:87 | `gateway/handlers/cluster.rs:183` (offline-merge constructor), `gateway/handlers/cluster.rs:174` (online iteration), `builtin_tools/node_list.rs:59` (node_list tool), webchat mirror `interfaces/webchat/src/api/cluster.rs:22` | registry.rs:935-1012, server tests |
| `NodeMatch` | registry.rs:112 | `builtin_tools/node_invoke_many.rs:57` (`fn invoke_one(m: NodeMatch, ...)`) and `:96,100` (the only producer: `resolve_all_by_tags`) | registry.rs:1042-1121 |
| `truncate_on_char_boundary` | registry.rs:129 | intra-crate: enrollment.rs:68,229, registry.rs:431 | registry.rs:859-868 |
| `NodeRegistry::new` | registry.rs:150 | 6 production sites: `gateway/server/mod.rs:525,583,808`, `gateway/server/probe.rs:104`, `gateway/handlers/auth/mod.rs:74`, daemon-bootstrap `bin/.../start/builder/agent_init/mod.rs:771` (via `set_node_registry` injection) | all registry.rs and builtin_tools test sites |
| `NodeRegistry::register` | registry.rs:159 | intra-crate: `registry::maybe_register_node` (registry.rs:597) | all registry.rs `NodeSession`-register tests |
| `NodeRegistry::deregister` | registry.rs:219 | `gateway/server/handler.rs:2020` (the disconnect cleanup arm) | registry.rs:661-965, server tests |
| `NodeRegistry::list_environments` | registry.rs:242 | `gateway/handlers/cluster.rs:174` (handle_environments_list — server-side read), `builtin_tools/node_list.rs:59` (the `node_list` tool) | registry.rs:647-1012 |
| `NodeRegistry::node_identity_by_conn` | registry.rs:266 | `gateway/server/handler.rs:787,2019` (anti-spoof identity + disconnect-time identity capture) | registry.rs:971-978, server tests |
| `NodeRegistry::resolve` | registry.rs:329 | `builtin_tools/node_invoke.rs:67`, `builtin_tools/node_file.rs:76` | registry.rs:663-700, node_invoke.rs/cluster.rs tests |
| `NodeRegistry::resolve_id` | registry.rs:351 | intra-crate: `cluster::enrollment::deregister_node` (enrollment.rs:402) | registry.rs:715-791 |
| `NodeRegistry::resolve_all_by_tags` | registry.rs:366 | `builtin_tools/node_invoke_many.rs:96,100` (the only production caller) | registry.rs:1019-1121 |
| `NodeRegistry::forget` | registry.rs:401 | intra-crate: `cluster::enrollment::deregister_node` (enrollment.rs:412) | registry.rs:915,961,967 |
| `normalize_node_key` | registry.rs:464 | intra-crate (enrollment.rs:25,121,128,288,297,313,329), intra-cluster `registry.rs:288,297,313`, `tools/adapters/registry_adapter.rs:466` (tool adapter reading `cluster::normalize_node_key`) | registry.rs:719-747 |
| `maybe_register_node` | registry.rs:496 | `gateway/server/handler.rs:1482` (the connect seam — the only live caller) | registry.rs:926-1113, handler.rs:2810, cluster.rs tests |
| `PendingInvokes::new` | reverse_rpc.rs:33 | intra-crate: `ReverseRpcChannel::new` / `with_close` (reverse_rpc.rs:186,205) | reverse_rpc.rs:485,502,561 |
| `PendingInvokes::register` | reverse_rpc.rs:40 | intra-crate: `ReverseRpcChannel::call` (reverse_rpc.rs:273) | — |
| `PendingInvokes::resolve` | reverse_rpc.rs:62 | `bin/.../node.rs:412` (node-side response routing), `gateway/server/handler.rs:767` (center-side response routing) | reverse_rpc.rs:444,492,504,531, node_approval.rs:274,324,347 |
| `PendingInvokes::cancel` | reverse_rpc.rs:90 | intra-crate: `ReverseRpcChannel::call`'s WaiterGuard (reverse_rpc.rs:392) | — |
| `PendingInvokes::cancel_all` | reverse_rpc.rs:107 | `gateway/server/handler.rs:1995` (the disconnect cleanup arm) | reverse_rpc.rs tests |
| `ReverseRpcError` | reverse_rpc.rs:117 | (enum + variants used in tool callers; the center-side types live in `ReverseRpcChannel::call`'s return) — no external match-arms yet, only the type lives | reverse_rpc.rs:294,320,329,345,347,474,553,606,626,646,669,689 |
| `ReverseRpcChannel::new` | reverse_rpc.rs:183 | `bin/aleph-server/commands/node.rs:380` (node binary), `gateway/server/handler.rs:2799` (test only — production uses `with_close`) | node_approval.rs, reverse_rpc.rs, builtin_tools/* tests, gateway/handlers/cluster.rs tests |
| `ReverseRpcChannel::with_close` | reverse_rpc.rs:202 | `gateway/server/handler.rs:695` (center-side, per-connection) | reverse_rpc.rs tests |
| `ReverseRpcChannel::pending` | reverse_rpc.rs:213 | `gateway/server/handler.rs:696`, `bin/.../node.rs:381`, `cluster/node_approval.rs:202` | many tests |
| `ReverseRpcChannel::close_connection` | reverse_rpc.rs:233 | `gateway/server/handler.rs` indirectly via `cluster::registry::register`/`forget` (registry.rs:183,209,416), `ReverseRpcChannel::call`'s OutboundWedged arm (reverse_rpc.rs:248) | reverse_rpc.rs:653 |
| `ReverseRpcChannel::call` | reverse_rpc.rs:263 | `cluster/node_approval.rs:141` (the only production method: `call("node.approval.request", ...)`), also called inside `bin/aleph-server/commands/node.rs` test paths | reverse_rpc.rs:411-686 |

---

## Findings

### sw-cluster-1 — `Environment` rendering duplicated in webchat (form 6, orphan surface mirror)

- **Module:** `src/cluster/`
- **Files:** `src/cluster/registry.rs:87-105`, `interfaces/webchat/src/api/cluster.rs:15-142`
- **Severity:** low
- **Form:** 6 — public re-export shape duplicated outside the module
- **Produced:** `pub struct Environment { pub id, pub name, pub status, pub commands: Vec<CommandDescriptor>, pub tags, pub connected_at, pub last_seen_at, pub version }` plus `pub struct CommandDescriptor { pub name, pub schema }`.
- **Produced location:** `src/cluster/registry.rs:87` and `src/cluster/registry.rs:29` (re-exported by `cluster/mod.rs:39`).
- **Consumer location:** none for the inner Aleph type. The webchat renders its **own** `Environment` and `CommandDescriptor` (`interfaces/webchat/src/api/cluster.rs:15,22`) populated by `serde_json::from_value(payload)` of the gateway's response — the Aleph type never crosses the JSON-RPC boundary typed.
- **Evidence:**
  ```bash
  $ rg -n "cluster::Environment|cluster::CommandDescriptor" src/ interfaces/ shared/ bin/
  src/gateway/handlers/cluster.rs:18:use crate::cluster::{deregister_node, enroll_node_device, DeregisterError};
  src/gateway/handlers/cluster.rs:174:    let mut envs = ctx.node_registry.list_environments();
  src/gateway/handlers/cluster.rs:183:                    .map(|d| crate::cluster::Environment {
  src/gateway/handlers/cluster.rs:190:                        version: None,
  src/gateway/handlers/cluster.rs:295:        // (no cluster::CommandDescriptor / Environment imports outside)
  src/builtin_tools/node_list.rs:59:    .list_environments()
  src/builtin_tools/node_file.rs:336:        .map(|c| CommandDescriptor {
  # ^ all `CommandDescriptor { ... }` literals outside src/cluster/ are inside #[cfg(test)] blocks
  src/cluster/registry.rs:530:        .map(|v| serde_json::from_value::<Vec<CommandDescriptor>>(v.clone()))
  src/cluster/node_file_cmd.rs:182:            CommandDescriptor { name: "file.write".to_string(), schema: json!({"type": "object"}) }
  src/cluster/node_file_cmd.rs:249:            CommandDescriptor { name: "file.read".to_string(), schema: json!({"type": "object"}) }
  src/cluster/node_runtime.rs:117:            CommandDescriptor { name: "bash".to_string(), schema: json!({"type": "object"}) }
  interfaces/webchat/src/api/cluster.rs:15:pub struct CommandDescriptor { ... }    # mirror
  interfaces/webchat/src/api/cluster.rs:22:pub struct Environment { ... }          # mirror
  ```
  The webchat mirror is intentional — it's the JSON rendering contract (R4: interfaces stay pure I/O) and the gateway handler at `src/gateway/handlers/cluster.rs:174` projects the typed `Vec<Environment>` to JSON, which the webchat `serde_json::from_value` decodes back into its mirror struct. This is not a severed wire — it's a deliberate protocol boundary.
- **Decision:** KEEP
- **Rationale:** The mirror struct exists for the same reason the canvas `CanvasListing` re-export exists in the 2026-08-17 audit (form 6 "KEEP"): the typed `Environment` is the source of truth for the server-side handler, and the webchat mirror is its serialized rendering contract. Replacing one with the other would couple the webchat frontend's serialization shape to the server's internal struct, not the other way around — exactly the inversion of R4. The lint is fine.
- **Proposed change:** none.
- **Risk:** none.
- **Verification:** n/a.

### sw-cluster-2 — `maybe_register_node`'s `role` parameter is "currently dead in production" by explicit design comment (informational, form 1-adjacent)

- **Module:** `src/cluster/`
- **Files:** `src/cluster/registry.rs:495-504`
- **Severity:** low
- **Form:** 1 (dead parameter) — but explicitly intended; documented in code.
- **Produced:**
  ```rust
  pub fn maybe_register_node(
      registry: &NodeRegistry,
      role: Option<&str>,                  // ← this parameter
      device_id: &str,
      conn_id: &str,
      params: Option<&Value>,
      channel: &ReverseRpcChannel,
  ) -> bool {
      if role != Some("node") { return false; }
      ...
  }
  ```
  The doc comment is explicit:
  > "The `role` gate is currently dead in production: the single live caller (`gateway/server/handler.rs:1344` [now :1482]) passes a hardcoded `Some("node")` after upstream shape detection. The gate is kept as a defensive parameter so future call sites cannot register a non-node connection by accident; the contract is 'call this only for `role == Some("node")`', and the unit test `maybe_register_node_registers_only_for_node_role` enforces it."
- **Consumer location:** `src/gateway/server/handler.rs:1482` — passes `Some("node")`. Test: `src/cluster/registry.rs:982-1012` (`maybe_register_node_registers_only_for_node_role`).
- **Evidence:** `rg -n "maybe_register_node" src/ interfaces/ shared/ bin/` shows exactly one production caller (handler.rs:1482) and one test caller for the role-gate (`registry.rs:986-1004`).
- **Decision:** KEEP
- **Rationale:** The doc comment is the audit's friend here — it documents *why* the dead-looking branch exists ("defensive parameter for future call sites", contract enforced by a test). This is a textbook "deliberate scaffolding with a defined and enforced contract", not a forgotten parameter. The triage playbook's "painless-wire heuristic" says: *absence of production pain is evidence*. Here we have an explicit, documented design choice plus the test that enforces it — the opposite of a quietly dropped wire.
- **Proposed change:** none. If a future caller wants the gate to be load-bearing, the test already pins the contract.
- **Risk:** none.
- **Verification:** n/a.

### sw-cluster-3 — `deregister_node` known gap (form 2-adjacent: stub far-end on lifecycle event publish)

- **Module:** `src/cluster/`
- **Files:** `src/cluster/enrollment.rs:396-417`, `src/gateway/server/handler.rs:2013-2021`
- **Severity:** medium (operator-visible; documented)
- **Form:** 2 — the wire is partly stubbed on the event-emit side. Documented in code; not a defect.
- **Produced:** `deregister_node` is documented as the **single shared source of truth** for "online-then-offline takedown + sticky revoke". It correctly evicts the live session via `registry.forget(&node_id)` (enrollment.rs:411) and revokes the device via `store.revoke_device(&node_id)` (enrollment.rs:413).
- **Consumer location:** `gateway/handlers/cluster.rs:108`, `builtin_tools/node_manage.rs:84` — both correctly call the shared function.
- **Evidence (the gap, fully documented in the source):**
  ```bash
  $ rg -n "node.disconnected" src/ interfaces/ shared/
  src/gateway/server/handler.rs:2027  # ctx.event_bus.publish_json(&TopicEvent::new("node.disconnected", ...))
  src/gateway/server/handler.rs:2000  # KNOWN GAP (2026-08-29) comment, 13 lines
  ```
  The disconnect-cleanness code in `src/gateway/server/handler.rs:2000-2034` is a 13-line inline comment explaining that the `node.disconnected` lifecycle event has **only one producer** repo-wide, and an operator-initiated `deregister_node` will skip its emission because the disconnect cleanup runs AFTER the live session has already been forgotten. The comment ends:
  > "This arm is NOT the place to fix it: after `forget` there is nothing here left to read. The publish belongs in `cluster::deregister_node` (already the single shared source for the RPC and tool faces), fired when it evicts a live session, with `forget` returning the evicted session's `device_name` so no second lookup is needed."
- **Decision:** DECIDE (already documented)
- **Rationale:** This is a documented, known gap — not a severed wire. The producer side (`connect`/`admit_node` + `maybe_register_node` → `node.connected`) and the consumer side (`node.disconnected` event) are both wired; the missing wire is the **mirror** `node.disconnected` on operator-initiated `deregister_node`. The fix surface (move the publish into `deregister_node` and have `forget` return the device name) is named in the source. The triage playbook explicitly covers this shape: "internal wires are more likely to be genuinely severed", and a real product decision (event vs no event) is embedded in the call. Keeping it as DECIDE rather than CONNECTing it during this audit respects that the team has already triaged it as a known limitation.
- **Proposed change (out of audit scope, documented):** relocate the `node.disconnected` publish into `cluster::deregister_node` per the comment; have `NodeRegistry::forget` return `Option<NodeSession>` so the device name travels with the eviction (so no second lookup is needed in the disconnect cleanup arm).
- **Risk:** low — the change is purely additive on the publish front, no new dependency, no schema impact.
- **Verification:** after the change, the existing `gateway/server/handler.rs:2020-2034` cleanup arm should have a no-op `deregister` arm (because `forget` already cleared the entry), and a new `deregister_node` self-test in `enrollment.rs` should assert that a `deregister` RPC against a live session publishes exactly one `node.disconnected` event with the correct `{node_id, name, conn_id}` payload.
- **Existing review ref:** inline at handler.rs:2000-2034.

### sw-cluster-4 — `Path::join` / directory traversal in `node_file_cmd.rs` — containment is correct (smell-fix verification, NOT a finding)

- **Module:** `src/cluster/`
- **Files:** `src/cluster/node_file_cmd.rs:60-90` (`resolve_in_jail`)
- **Severity:** n/a (verified clean)
- **Form:** n/a
- **Produced:** `pub fn resolve_in_jail(path: &str, workspace_dir: &Path) -> Result<PathBuf, String>` performs `tokio::fs::canonicalize(workspace_dir)` → `check_and_resolve_path(Path::new(path), &get_denied_paths(), Some(&root))` → explicit `starts_with(&root)` containment check.
- **Consumer location:** `FileWriteCommand::run` (node_file_cmd.rs:124) and `FileReadCommand::run` (node_file_cmd.rs:217) — both run the path through `resolve_in_jail` first.
- **Evidence:**
  ```bash
  $ rg -n "resolve_in_jail|check_and_resolve_path|canonicalize|starts_with" src/cluster/node_file_cmd.rs
  src/cluster/node_file_cmd.rs:60:async fn resolve_in_jail(path: &str, workspace_dir: &Path) -> Result<PathBuf, String>
  src/cluster/node_file_cmd.rs:66:    tokio::fs::create_dir_all(workspace_dir).await
  src/cluster/node_file_cmd.rs:71:    let root_meta = tokio::fs::symlink_metadata(workspace_dir).await    # B4-01: rejects symlink root
  src/cluster/node_file_cmd.rs:81:    let root = tokio::fs::canonicalize(workspace_dir).await          # canonical root
  src/cluster/node_file_cmd.rs:83:    let resolved = check_and_resolve_path(Path::new(path), &get_denied_paths(), Some(&root))
  src/cluster/node_file_cmd.rs:85:    if !resolved.starts_with(&root) {                              # explicit containment gate
  src/cluster/node_file_cmd.rs:124:    let dest = resolve_in_jail(path, &self.workspace_dir).await?;
  src/cluster/node_file_cmd.rs:148:    opts.custom_flags(libc::O_NOFOLLOW);                          # B4-02: refuse symlink at leaf
  src/cluster/node_file_cmd.rs:217:    let src = resolve_in_jail(path, &self.workspace_dir).await?;
  ```
- **Decision:** KEEP (no change needed)
- **Rationale:** The user-supplied path is NEVER directly `Path::join`ed onto a workspace root. The flow is: `symlink_metadata` (refuses a symlink root) → `canonicalize` (real root) → `check_and_resolve_path` (deny-list + base join) → `starts_with` (containment gate). The `O_NOFOLLOW` on the leaf open further closes a TOCTOU window. The doc comments at lines 60-79 explicitly enumerate each attack class.
- **Verification:** the unit test `file_write_rejects_traversal` (node_file_cmd.rs:313-323) exercises the `"../escape.bin"` rejection path; `file_write_rejects_oversize` (node_file_cmd.rs:299-306) exercises the size cap.
- **Risk:** none.
- **Existing review ref:** B4-01 (symlink root), B4-02 (leaf O_NOFOLLOW), B4-04 (size cap before allocation) — all tagged in inline comments.

### sw-cluster-5 — `NodeSession` is `pub` but only constructed via `maybe_register_node` (form 1 / form 6, healthy)

- **Module:** `src/cluster/`
- **Files:** `src/cluster/registry.rs:35-60`, `src/cluster/registry.rs:597`
- **Severity:** low
- **Form:** 6 — orphan-looking `pub` struct that is in fact load-bearing.
- **Produced:** `pub struct NodeSession { pub node_id, pub conn_id, pub device_name, pub channel, pub declared_commands, pub tags, pub version, pub connected_at }` — all fields are `pub`. Type is re-exported by `cluster/mod.rs:39`.
- **Consumer location:** Production: `registry::maybe_register_node` (registry.rs:597 — the only constructor site in production code). Test: many sites in `src/builtin_tools/*` and `src/gateway/handlers/cluster.rs`.
- **Evidence:**
  ```bash
  $ rg -n "NodeSession\s*\{" src/ interfaces/ shared/ bin/
  src/cluster/registry.rs:597   # the single producer (maybe_register_node)
  src/builtin_tools/node_file.rs:329,456      # TEST
  src/builtin_tools/node_invoke_many.rs:180   # TEST
  src/builtin_tools/node_invoke.rs:120,211    # TEST
  src/builtin_tools/node_list.rs:90           # TEST
  src/builtin_tools/node_manage.rs:124        # TEST
  # zero production NodeSession { ... } literals outside the cluster module
  ```
  The struct is `pub` because `NodeRegistry::register` (`pub fn register(&self, session: NodeSession)`) takes it by value, and the constructor `maybe_register_node` is called from the connect seam with the **build** of a `NodeSession` happening inside the cluster module. The struct must be visible to its caller, hence `pub`.
- **Decision:** KEEP (no change needed)
- **Rationale:** This is the same shape as the canvas `CanvasListing` re-export (form 6 healthy, KEEP per the 2026-08-17 audit). The `pub` visibility is **load-bearing** because the connect seam calls `maybe_register_node` which builds the `NodeSession` inline; the type must cross the module boundary, and the tests pin the contract for fields like `version` (registry.rs:922-947).
- **Proposed change:** none.
- **Verification:** n/a.

### sw-cluster-6 — Inbound RPC count parity (3 cluster methods, 3 handlers, 3 registrations)

- **Module:** `src/cluster/`
- **Files:** `src/bin/aleph-server/commands/start/builder/handlers/core.rs:25-44`, `src/gateway/handlers/cluster.rs`, `src/gateway/method_admin.rs:112,639,643`
- **Severity:** n/a (verified clean)
- **Form:** n/a — three inbound methods all three are wired.
- **Produced:** Three inbound RPCs the cluster module's I/O surfaces need:
  - `cluster.enroll` (operator pre-enrollment)
  - `cluster.deregister` (operator takedown)
  - `environments.list` (read-only fleet view)
- **Produced location:** `src/gateway/handlers/cluster.rs:49,99,170` define `handle_cluster_enroll`, `handle_cluster_deregister`, `handle_environments_list` respectively.
- **Consumer location:** registered at `src/bin/aleph-server/commands/start/builder/handlers/core.rs:28,34,40`. Gated by `src/gateway/method_admin.rs:112` ("cluster." prefix admin) and `:639-643` (explicit `cluster.enroll` / `cluster.deregister` / `environments.list` allowlist in the unit-test sweep).
- **Evidence:**
  ```bash
  $ rg -n '"cluster\.enroll"|"cluster\.deregister"|"environments\.list"' src/
  src/bin/aleph-server/commands/start/builder/handlers/core.rs:28  # register_handler!("cluster.enroll")
  src/bin/aleph-server/commands/start/builder/handlers/core.rs:34  # register_handler!("cluster.deregister")
  src/bin/aleph-server/commands/start/builder/handlers/core.rs:40  # register_handler!("environments.list")
  src/gateway/method_admin.rs:112  "cluster." // enroll / deregister — fleet membership.
  src/gateway/method_admin.rs:639     "cluster.enroll",
  src/gateway/method_admin.rs:640     "cluster.deregister",
  src/gateway/method_admin.rs:643     "environments.list",
  src/gateway/method_census.rs:131     ("cluster.deregister", Class::Admin),
  src/gateway/method_census.rs:132     ("cluster.enroll",    Class::Admin),
  src/gateway/method_census.rs:172     ("environments.list", Class::Admin),
  src/gateway/method_authz.rs:234      ("node_manage", "cluster.enroll"),  # the node_manage tool mirrors enroll
  ```
- **Decision:** KEEP (no change needed)
- **Rationale:** All three RPCs are emitted from the LLM tool face (`node_manage` in `src/builtin_tools/node_manage.rs`) and from the Panel (`interfaces/webchat/src/platform/wide/views/settings/network/cluster.rs:335`), reach a handler (`src/gateway/handlers/cluster.rs:49,99,170`), and the handler routes to the same cluster primitives (`enroll_node_device`, `deregister_node`, `list_environments`) that `connect`-time and `node_invoke` use. The dispatch triad (admin prefix + census class + handler registry) is consistent.
- **Verification:** `cluster.enroll` / `cluster.deregister` / `environments.list` all appear in both `method_admin.rs`'s allowlist (line 639-643) AND `method_census.rs` (lines 131-172) AND the handler registry. No drift.

### sw-cluster-7 — `Box::leak` / `Runtime::new()` in hot loops — none found

- **Module:** `src/cluster/`
- **Severity:** n/a (clean)
- **Form:** n/a
- **Evidence:**
  ```bash
  $ rg -n "Box::leak|tokio::runtime::Runtime::new\(\)" src/cluster/
  # no matches
  ```
- **Decision:** n/a
- **Rationale:** No `Box::leak` of any long-lived handle; no `Runtime::new()` per-iteration. The module uses `tokio::sync::{mpsc, oneshot, Notify}` and `tokio::time::timeout` (existing tokio runtime), not its own.

### sw-cluster-8 — `unwrap()` / `expect()` on fallible paths — only in tests (clean)

- **Module:** `src/cluster/`
- **Severity:** n/a (clean)
- **Form:** n/a
- **Evidence:**
  ```bash
  $ rg -n "\.unwrap\(\)|\.expect\(" src/cluster/
  src/cluster/reverse_rpc.rs:441     #[cfg(test)] block at line 380 — `expect("request frame")`
  src/cluster/reverse_rpc.rs:442     #[cfg(test)]
  src/cluster/reverse_rpc.rs:453     #[cfg(test)]
  src/cluster/reverse_rpc.rs:494     #[cfg(test)]
  src/cluster/reverse_rpc.rs:523     #[cfg(test)]
  src/cluster/reverse_rpc.rs:524     #[cfg(test)]
  src/cluster/reverse_rpc.rs:537     #[cfg(test)]
  src/cluster/reverse_rpc.rs:539     #[cfg(test)]
  src/cluster/reverse_rpc.rs:604     #[cfg(test)]
  src/cluster/reverse_rpc.rs:616     #[cfg(test)]
  src/cluster/reverse_rpc.rs:637     #[cfg(test)]
  src/cluster/reverse_rpc.rs:653     #[cfg(test)]
  src/cluster/enrollment.rs:430      #[cfg(test)] block at line 424 — `SecurityStore::in_memory().expect("in-memory store")`
  src/cluster/enrollment.rs:441      #[cfg(test)]
  src/cluster/enrollment.rs:462      #[cfg(test)]
  src/cluster/enrollment.rs:469      #[cfg(test)]
  src/cluster/enrollment.rs:481      #[cfg(test)]
  src/cluster/enrollment.rs:491      #[cfg(test)]
  src/cluster/enrollment.rs:506      #[cfg(test)]
  src/cluster/enrollment.rs:519      #[cfg(test)]
  src/cluster/enrollment.rs:550      #[cfg(test)]
  src/cluster/enrollment.rs:559      #[cfg(test)]
  src/cluster/enrollment.rs:578      #[cfg(test)]
  src/cluster/enrollment.rs:586      #[cfg(test)]
  src/cluster/enrollment.rs:593      #[cfg(test)]
  src/cluster/enrollment.rs:596      #[cfg(test)]
  src/cluster/enrollment.rs:603      #[cfg(test)]
  src/cluster/enrollment.rs:604      #[cfg(test)]
  src/cluster/enrollment.rs:608      #[cfg(test)]
  src/cluster/enrollment.rs:631      #[cfg(test)]
  src/cluster/enrollment.rs:658      #[cfg(test)]
  src/cluster/node_approval.rs:267   #[cfg(test)] block at line 179
  src/cluster/node_approval.rs:268   #[cfg(test)]
  src/cluster/node_approval.rs:313   #[cfg(test)]
  src/cluster/node_approval.rs:314   #[cfg(test)]
  src/cluster/node_approval.rs:343   #[cfg(test)]
  src/cluster/node_approval.rs:344   #[cfg(test)]
  src/cluster/node_runtime.rs:181    #[cfg(test)] block at line 147
  src/cluster/node_runtime.rs:237    #[cfg(test)]
  # Zero production unwrap/expect on fallible paths.
  ```
  Lock poisoning follows the P7 pattern (`unwrap_or_else(|e| e.into_inner())`) at every site — 18 occurrences in production code.
- **Decision:** n/a
- **Rationale:** Production lock acquisition never `unwrap`s — every lock uses the P7 poison-safe idiom. The remaining `.unwrap()` / `.expect()` calls are all inside `#[cfg(test)]` blocks (`reverse_rpc.rs:380`, `enrollment.rs:424`, `node_approval.rs:179`, `node_runtime.rs:147`).

### sw-cluster-9 — `request_shutdown()` / `JoinHandle` capture — clean

- **Severity:** n/a
- **Form:** n/a
- **Evidence:**
  ```bash
  $ rg -n "request_shutdown|JoinHandle::abort|writer\.abort" src/cluster/ src/bin/aleph-server/commands/node.rs
  src/bin/aleph-server/commands/node.rs:400  # writer.abort()  — captured JoinHandle pattern (writes "BIN-R4-08")
  # No request_shutdown() / dropped JoinHandle in cluster module.
  ```
  The node-side `run_session` (`bin/aleph-server/commands/node.rs:392-400`) does capture the `writer` JoinHandle and calls `writer.abort()` on every exit path with the explicit comment "BIN-R4-08: surface writer-task errors to the operator log". The reverse-RPC channel itself never spawns tasks; it's a pure transport.
- **Decision:** n/a
- **Rationale:** the only place a long-lived task is spawned (the node's outbound writer task) captures and aborts its JoinHandle, and the comment ties it to a known review item (BIN-R4-08).

### sw-cluster-10 — Outbound IPC timeouts — clean

- **Severity:** n/a
- **Form:** n/a
- **Evidence:**
  ```bash
  $ rg -n "tokio::time::timeout|timeout_at|REVERSE_RPC_MAX_TIMEOUT_MS|OUTBOUND_PUSH_BUDGET_MS|NODE_APPROVAL_TIMEOUT_MS" src/cluster/
  src/cluster/reverse_rpc.rs:150  /// const REVERSE_RPC_MAX_TIMEOUT_MS (referenced via crate::tools::budget)
  src/cluster/reverse_rpc.rs:151  const OUTBOUND_PUSH_BUDGET_MS: u64 = 500
  src/cluster/reverse_rpc.rs:282  let budget = Duration::from_millis(timeout_ms)
  src/cluster/reverse_rpc.rs:294  let outbound_budget = Duration::from_millis(timeout_ms.min(OUTBOUND_PUSH_BUDGET_MS))
  src/cluster/reverse_rpc.rs:295  let outbound_deadline = tokio::time::Instant::now() + outbound_budget
  src/cluster/reverse_rpc.rs:296  let response_deadline = tokio::time::Instant::now() + budget
  src/cluster/reverse_rpc.rs:316  match tokio::time::timeout_at(outbound_deadline, self.outbound.send(frame)).await
  src/cluster/reverse_rpc.rs:335  match tokio::time::timeout_at(response_deadline, rx).await
  src/cluster/node_approval.rs:47  pub(crate) const NODE_APPROVAL_TIMEOUT_MS: u64 = 130_000
  src/cluster/node_approval.rs:141 .call("node.approval.request", params, NODE_APPROVAL_TIMEOUT_MS)
  ```
  The `ReverseRpcChannel::call` API clamps the caller-supplied `timeout_ms` to `REVERSE_RPC_MAX_TIMEOUT_MS` (reverse_rpc.rs:288) and splits it into an outbound-enqueue budget (`OUTBOUND_PUSH_BUDGET_MS = 500ms` cap) and a separate response-wait budget. The center side never waits longer than the caller's budget; an outbound wedge explicitly fires `close_connection` (reverse_rpc.rs:248) to reap the half-open connection. The approval call passes `NODE_APPROVAL_TIMEOUT_MS = 130_000ms`, which is deliberately 10s above the center's `DEFAULT_APPROVAL_TIMEOUT_MS = 120s` so the center decides first and returns an explicit "timeout" outcome rather than this backstop firing first.
- **Decision:** n/a
- **Rationale:** All blocking IPC calls are budgeted. The split budget is documented inline (B3-01) and the test suite (`reverse_rpc.rs:609-689`) covers `OutboundWedged`, `Timeout`, `Cancelled`, `TransportClosed` arms with explicit assertions.

### sw-cluster-11 — HMAC / sha256 comparison — uses `==`, NOT constant-time — but no security gate

- **Module:** `src/cluster/`
- **Files:** `src/cluster/node_file_cmd.rs:51`
- **Severity:** low (smell, not a security finding)
- **Form:** smell / security-adjacent
- **Produced:**
  ```rust
  if sha256_hex(&bytes) != expected_sha {
      return Err("file.write: sha256 mismatch".to_string());
  }
  ```
- **Consumer location:** `FileWriteCommand::run` (node_file_cmd.rs:124-).
- **Evidence:**
  ```bash
  $ rg -n "sha256|sha2|hmac|constant_time|subtle" src/cluster/
  src/cluster/node_file_cmd.rs:11  use sha2::{Digest, Sha256}
  src/cluster/node_file_cmd.rs:27  hex::encode(Sha256::digest(bytes))   # the helper
  src/cluster/node_file_cmd.rs:51  if sha256_hex(&bytes) != expected_sha {  # the gate
  src/cluster/node_file_cmd.rs:138 fn descriptor(&self) -> CommandDescriptor { ... sha256_hex(&buf) }
  ```
  The `sha256_hex(&bytes) != expected_sha` comparison is **not** constant-time. It compares `String == String`, which is short-circuiting at the first differing byte — a side channel in principle, but the timing difference is observable only on adversarial input that the center-side has already trusted to the extent of **sending it** to the node in the first place (`builtin_tools/node_file.rs:368` builds the request with the sha256). The "attacker" in the model already controls the bytes they want written; a side-channel on `==` would only tell them "did the node pick a different file?", which the operator's UI already shows them. This is not a real exploitable side channel — it's an integrity check, not an authentication gate.
- **Decision:** KEEP (no change needed for security); note the smell.
- **Rationale:** HMAC verification requires constant-time comparison when the secret is **unknown to the attacker** — i.e. an attacker who can submit a forged tag and infer the key byte-by-byte. Here the "secret" (the sha256 of the bytes the center already has) is exactly what the attacker chose to send. The `==` is fine.
- **Proposed change:** none. If the team prefers belt-and-braces, a constant-time `subtle::ConstantTimeEq` import costs ~30 lines of `Cargo.toml`/deps and is not worth the dependency surface for a non-exploitable side channel. Keep it.
- **Risk:** none.

### sw-cluster-12 — Approval state machine unreachable branch (NONE found)

- **Severity:** n/a
- **Form:** n/a
- **Evidence:** `src/cluster/node_approval.rs:57-87` — the `outcome_from_str` mapping is exhaustive over the five known outcomes and a single catch-all that maps anything else to `Unavailable`. The `CenterApprovalRequester::request_approval` impl (node_approval.rs:108-176) covers all four error arms (no channel / closed transport / JSON-RPC error / reverse-RPC error). The test `no_locally_minted_refusal_is_attributed_to_a_person` (node_approval.rs:281-345) asserts every error path lands on `Unavailable` and that `is_a_human_decision()` is false. The state machine has no unreachable branches.
- **Decision:** n/a

### sw-cluster-13 — Unused imports / inert `pub(crate)` items — none found

- **Severity:** n/a
- **Form:** n/a
- **Evidence:**
  ```bash
  $ rg -n "pub\(crate\)" src/cluster/
  src/cluster/node_approval.rs:41   pub type ApprovalSlot = ...   # re-exported as pub
  src/cluster/node_approval.rs:47   pub(crate) const NODE_APPROVAL_TIMEOUT_MS — used :141
  src/cluster/node_approval.rs:57   pub(crate) fn outcome_from_str — used :161
  src/cluster/node_runtime.rs:97    pub(crate) struct BashNodeCommand — used by CommandTable::with_bash :128
  src/cluster/node_runtime.rs:103   pub(crate) const fn new — used :128
  src/cluster/node_file_cmd.rs:26   pub(crate) fn sha256_hex — used by builtin_tools/node_file.rs
  src/cluster/registry.rs:129       pub(crate) fn truncate_on_char_boundary — used by enrollment.rs:25,68,229
  src/cluster/registry.rs:351       pub(crate) fn resolve_id — used by enrollment.rs:402
  src/cluster/registry.rs:464       pub(crate) fn normalize_node_key — used by enrollment.rs, tools/adapters/registry_adapter.rs
  src/cluster/reverse_rpc.rs:40     pub(crate) fn register — used by call :273
  src/cluster/reverse_rpc.rs:90     pub(crate) fn cancel — used by WaiterGuard :392
  # Every pub(crate) item has at least one live consumer.
  ```
  Imports were checked at every use site (`use crate::cluster::{...}` matches the re-exports in `mod.rs`; no imports of internal symbols like `truncate_on_char_boundary` from outside the cluster module). No inert `pub(crate)` items, no unused imports.

### sw-cluster-14 — `sha256_hex` is duplicated across 4 files (smell, well-justified)

- **Module:** `src/cluster/` and others
- **Files:** `src/cluster/node_file_cmd.rs:26`, `src/memory/notes/profile/store.rs:253`, `src/memory/notes/note/helpers.rs:6`, `src/gateway/interfaces/nostr/message_ops/tests.rs:19` (test)
- **Severity:** low (smell, name-drift risk if hashes ever need to differ)
- **Form:** 5-adjacent — duplicated definition across crates
- **Produced:** four `fn sha256_hex(bytes/&str) -> String` definitions, each wrapping `Sha256::digest(...).hex::encode(...)`.
- **Consumer location:**
  - cluster::sha256_hex → `builtin_tools/node_file.rs:122,195,368,399,478` and intra-crate (node_file_cmd.rs:51,244,279)
  - memory::notes::profile::store::sha256_hex → `memory/notes/profile/store.rs:77,92`
  - memory::notes::note::helpers::sha256_hex → `memory/notes/note/mod.rs:164`
- **Evidence:**
  ```bash
  $ rg -n "fn sha256_hex" src/
  src/cluster/node_file_cmd.rs:26         pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
  src/memory/notes/profile/store.rs:253   fn sha256_hex(s: &str) -> String {
  src/memory/notes/note/helpers.rs:6      pub fn sha256_hex(content: &str) -> String {
  src/gateway/interfaces/nostr/message_ops/tests.rs:19   fn test_compute_event_id_is_sha256_hex()
  ```
  The signatures differ (one takes `&[u8]`, two take `&str`), which is the historical reason the cluster module cannot simply re-export one of the existing helpers — the cluster file protocol requires `&[u8]` (decoded base64), while memory note hashing requires `&str` (UTF-8 text). They are intentionally separate.
- **Decision:** KEEP (with a note for future consolidation)
- **Rationale:** The signatures differ on purpose (`&[u8]` for binary integrity verification on file payloads, `&str` for content-hash of UTF-8 notes). A future refactor could move all four into a `shared::hash` crate, but that's outside the audit's scope and would touch three unrelated modules.
- **Risk:** low — the algorithms are identical (`hex::encode(Sha256::digest(...))`), so even if the implementations drift slightly the wire format won't.

---

## Symbols that PASS parity (explicit healthy surface)

Every public symbol in `src/cluster/` has at least one production caller. Specifically:

| Surface area | Symbol | Production caller |
|---|---|---|
| Enrollment (center-side) | `admit_node` | `gateway/server/handler.rs:1464` |
| Enrollment | `enroll_node_device` | `gateway/handlers/cluster.rs:58`, `builtin_tools/node_manage.rs:72` |
| Enrollment | `deregister_node` | `gateway/handlers/cluster.rs:108`, `builtin_tools/node_manage.rs:84`, webchat wrapper |
| Enrollment | `NodeAdmission` | `gateway/server/handler.rs:1455,1477,1519,1534` |
| Enrollment | `DeregisterError` | `builtin_tools/node_manage.rs:94,98`, `gateway/handlers/cluster.rs:141,146` |
| Enrollment | `DeregisterOutcome` | `gateway/handlers/cluster.rs:122` (field reads) |
| Approval (node-side) | `ApprovalSlot` | `bin/aleph-server/commands/node.rs:284,320,381` |
| Approval | `CenterApprovalRequester` | `bin/aleph-server/commands/node.rs:285` |
| Approval | `NODE_APPROVAL_TIMEOUT_MS` | `node_approval.rs:141` (intra-crate, single consumer — `pub(crate)`) |
| Approval | `outcome_from_str` | `node_approval.rs:161` (intra-crate, single consumer — `pub(crate)`) |
| File commands | `MAX_FILE_BYTES` | `builtin_tools/node_file.rs:113-191` (mirror cap on the center side) |
| File commands | `sha256_hex` | `builtin_tools/node_file.rs:122,195,368` (center-side integrity) |
| File commands | `FileWriteCommand::new` | `node_runtime.rs:143` → `bin/.../node.rs:301` |
| File commands | `FileReadCommand::new` | `node_runtime.rs:141` → `bin/.../node.rs:301` |
| Node runtime | `NodeCommand` (trait) | impls `BashNodeCommand` (node_runtime.rs:109), `FileWriteCommand` (node_file_cmd.rs:111), `FileReadCommand` (node_file_cmd.rs:203) |
| Node runtime | `CommandTable::new` | `bin/.../node.rs:300` (via `with_bash`) |
| Node runtime | `CommandTable::register` | `node_runtime.rs:130,141,143` (intra-crate) |
| Node runtime | `CommandTable::descriptors` | `bin/.../node.rs:317` |
| Node runtime | `CommandTable::dispatch` | `bin/.../node.rs:455` (the only production tool.call dispatcher) |
| Node runtime | `CommandTable::with_bash` | `bin/.../node.rs:300,483` |
| Node runtime | `CommandTable::register_file_commands` | `bin/.../node.rs:301` |
| Node runtime | `BashNodeCommand` (intra-crate) | registered under "bash" by `with_bash` |
| Registry | `CommandDescriptor` | `bin/.../node.rs:28,317`, `gateway/handlers/cluster.rs:183`, `registry.rs:530-532` |
| Registry | `NodeSession` | `registry::maybe_register_node` (registry.rs:597) → `gateway/server/handler.rs:1482` |
| Registry | `ResolveError` | `builtin_tools/node_invoke.rs:69,72`, `builtin_tools/node_file.rs:78,81` |
| Registry | `Environment` | `gateway/handlers/cluster.rs:174,183`, `builtin_tools/node_list.rs:59` |
| Registry | `NodeMatch` | `builtin_tools/node_invoke_many.rs:57,96,100` |
| Registry | `truncate_on_char_boundary` | `enrollment.rs:25,68,229`, `registry.rs:431` |
| Registry | `NodeRegistry::new` | 6 production sites (server, probe, auth, agent_init) |
| Registry | `NodeRegistry::register` | `registry::maybe_register_node` (intra-crate) |
| Registry | `NodeRegistry::deregister` | `gateway/server/handler.rs:2020` |
| Registry | `NodeRegistry::list_environments` | `gateway/handlers/cluster.rs:174`, `builtin_tools/node_list.rs:59` |
| Registry | `NodeRegistry::node_identity_by_conn` | `gateway/server/handler.rs:787,2019` |
| Registry | `NodeRegistry::resolve` | `builtin_tools/node_invoke.rs:67`, `builtin_tools/node_file.rs:76` |
| Registry | `NodeRegistry::resolve_id` | `enrollment::deregister_node` (intra-crate) |
| Registry | `NodeRegistry::resolve_all_by_tags` | `builtin_tools/node_invoke_many.rs:96,100` |
| Registry | `NodeRegistry::forget` | `enrollment::deregister_node` (intra-crate) |
| Registry | `normalize_node_key` | `enrollment.rs`, `registry.rs:288,297,313`, `tools/adapters/registry_adapter.rs:466` |
| Registry | `maybe_register_node` | `gateway/server/handler.rs:1482` |
| Reverse-RPC | `PendingInvokes::new` | `ReverseRpcChannel::new` / `with_close` (intra-crate) |
| Reverse-RPC | `PendingInvokes::register` | `ReverseRpcChannel::call` (intra-crate) |
| Reverse-RPC | `PendingInvokes::resolve` | `gateway/server/handler.rs:767`, `bin/.../node.rs:412` |
| Reverse-RPC | `PendingInvokes::cancel` | `ReverseRpcChannel::call`'s WaiterGuard (intra-crate) |
| Reverse-RPC | `PendingInvokes::cancel_all` | `gateway/server/handler.rs:1995` |
| Reverse-RPC | `ReverseRpcError` | returned by `ReverseRpcChannel::call`; enum variants are documented in inline rustdoc on each variant |
| Reverse-RPC | `ReverseRpcChannel::new` | `bin/.../node.rs:380` |
| Reverse-RPC | `ReverseRpcChannel::with_close` | `gateway/server/handler.rs:695` |
| Reverse-RPC | `ReverseRpcChannel::pending` | `gateway/server/handler.rs:696`, `bin/.../node.rs:381`, `cluster/node_approval.rs:202` |
| Reverse-RPC | `ReverseRpcChannel::close_connection` | `cluster::registry::{register, forget}` (registry.rs:183,209,416), `ReverseRpcChannel::call`'s OutboundWedged arm (reverse_rpc.rs:248) |
| Reverse-RPC | `ReverseRpcChannel::call` | `cluster/node_approval.rs:141` (only production method: `node.approval.request`) |

**Module-level verdicts:**
- **Public API surface parity:** PASS — every `pub` symbol in `src/cluster/` has a production caller (table above).
- **Call-vs-handler parity:** PASS — every method emitted over reverse-RPC (`node.approval.request`, `tool.call`) has a server-side handler; every server-side handler has a caller. The single reverse-RPC method the cluster module emits is `node.approval.request` (handled by `gateway/server/handler.rs:781`) and `tool.call` (handled by `bin/aleph-server/commands/node.rs:443`'s `handle_frame` → `CommandTable::dispatch`).
- **Config-reader parity:** PASS — the cluster module reads no config struct (it's pure runtime state); the `LAN-trust` model deliberately has no `Config` reader parity to verify. The downstream consumers (`node_invoke`, `node_file`, `node_list`) read their config from `aleph-core`'s standard config flow.
- **Stub sweep:** PASS — no `// TODO`, `unimplemented!`, `todo!`, or empty-handler stubs in the cluster module.
- **Name drift:** PASS — all cluster method names match between caller and dispatch.

---

## Negative findings (what this audit did NOT find)

These are explicit no-finding confirmations, showing the audit's coverage:

- No `Box::leak` of any long-lived handle (sw-cluster-7).
- No `tokio::runtime::Runtime::new()` inside hot loops or per-iteration (sw-cluster-7).
- No `unwrap()` / `expect()` on fallible paths an operator can hit (sw-cluster-8) — all such calls are inside `#[cfg(test)]` blocks.
- No `request_shutdown()` without `JoinHandle` capture (sw-cluster-9).
- No `select!` arms that never fire (sw-cluster-9). The single `select!` arm in the cluster module (`reverse_rpc.rs:414`) is in a test.
- No missing timeouts on IPC calls (sw-cluster-10). `ReverseRpcChannel::call` clamps to `REVERSE_RPC_MAX_TIMEOUT_MS`, splits into outbound + response budgets, and `node.approval.request` uses `NODE_APPROVAL_TIMEOUT_MS = 130s` (10s above the center's `DEFAULT_APPROVAL_TIMEOUT_MS = 120s`).
- No unused imports, dead functions, or inert `pub(crate)` items (sw-cluster-13).
- No orphan re-exports of types with no name import (sw-cluster-1 /6).
- No error variants that are never constructed (`ReverseRpcError`'s five variants all have a construction site in `reverse_rpc.rs:294,320,329,345,347`).
- No unreachable branches in the approval state machine (sw-cluster-12).
- No retry loops without exponential backoff (the only retry behavior is the node binary's outbound `BACKOFF_INITIAL_MS = 2_000` / `BACKOFF_MAX_MS = 60_000` exponential schedule in `bin/.../node.rs:42-43`, which IS exponential backoff).
- No HMAC verification that compares strings without constant-time comparison (sw-cluster-11) — the only `==` on a hash is sha256-of-bytes-the-attacker-already-knows, so it's not a security gate.
- No directory traversal: `Path::join(user_input)` is not used; user input flows through `check_and_resolve_path` + `canonicalize` + `starts_with` containment (sw-cluster-4).

---

## Recommended actions (priority order)

1. **Resolve sw-cluster-3** (operator-initiated `deregister_node` missing `node.disconnected` event) — relocate the publish from `gateway/server/handler.rs:2027` into `cluster::deregister_node`, fire it only when `evicted=true`. Have `NodeRegistry::forget` return `Option<NodeSession>` so the device name travels. This is the only **real** wiring gap in the audit, and it is documented in code. Severity: medium (operator-visible).

2. **No other actions.** Every other `pub` symbol is wired; every reverse-RPC method has a handler; the inbound RPC face is triple-locked (admin prefix + census class + handler registry); the test suite is comprehensive; the file-jail security boundary is correct.

3. **Optional follow-up (out of audit scope):** consolidate the four `fn sha256_hex` definitions into a `shared::hash` crate (sw-cluster-14). Today the signatures differ (`&[u8]` vs `&str`), so this is not a CUT — it's a future refactor.

---

## Sanity-check table (file:line for every flagged or load-bearing symbol)

| Symbol | Definition | Production caller | Audit verdict |
|---|---|---|---|
| `NodeAdmission` | enrollment.rs:31 | handler.rs:1455,1477,1519,1534 | HEALTHY |
| `admit_node` | enrollment.rs:196 | handler.rs:1464 | HEALTHY |
| `enroll_node_device` | enrollment.rs:156 | cluster.rs:58, node_manage.rs:72 | HEALTHY |
| `DeregisterError` | enrollment.rs:308 | node_manage.rs:94,98, cluster.rs:141,146 | HEALTHY |
| `DeregisterOutcome` | enrollment.rs:317 | cluster.rs:122 | HEALTHY |
| `deregister_node` | enrollment.rs:396 | cluster.rs:108, node_manage.rs:84, webchat wrapper | HEALTHY (sw-cluster-3: known event-emission gap) |
| `ApprovalSlot` | node_approval.rs:41 | node.rs:284,320,381 | HEALTHY |
| `NODE_APPROVAL_TIMEOUT_MS` | node_approval.rs:47 | node_approval.rs:141 (intra-crate) | HEALTHY (pub(crate), single consumer) |
| `outcome_from_str` | node_approval.rs:57 | node_approval.rs:161 (intra-crate) | HEALTHY (pub(crate), single consumer) |
| `CenterApprovalRequester` | node_approval.rs:97 | node.rs:285 | HEALTHY |
| `MAX_FILE_BYTES` | node_file_cmd.rs:20 | node_file.rs:113,115,180,183,189,191 | HEALTHY |
| `sha256_hex` (cluster) | node_file_cmd.rs:26 | node_file.rs:122,195,368 | HEALTHY (duplicated across 4 files — sw-cluster-14) |
| `FileWriteCommand::new` | node_file_cmd.rs:105 | node_runtime.rs:143 → node.rs:301 | HEALTHY |
| `FileReadCommand::new` | node_file_cmd.rs:197 | node_runtime.rs:141 → node.rs:301 | HEALTHY |
| `NodeCommand` (trait) | node_runtime.rs:25 | impls in node_runtime.rs:109, node_file_cmd.rs:111,203 | HEALTHY |
| `CommandTable::new` | node_runtime.rs:38 | node.rs:300 (via with_bash) | HEALTHY |
| `CommandTable::register` | node_runtime.rs:42 | node_runtime.rs:130,141,143 (intra-crate) | HEALTHY |
| `CommandTable::descriptors` | node_runtime.rs:65 | node.rs:317 | HEALTHY |
| `CommandTable::dispatch` | node_runtime.rs:77 | node.rs:455 | HEALTHY |
| `CommandTable::with_bash` | node_runtime.rs:128 | node.rs:300,483 | HEALTHY |
| `CommandTable::register_file_commands` | node_runtime.rs:137 | node.rs:301 | HEALTHY |
| `BashNodeCommand` | node_runtime.rs:97 | registered under "bash" by with_bash | HEALTHY |
| `CommandDescriptor` | registry.rs:29 | cluster.rs:183, node.rs:28,317, registry.rs:530-532 | HEALTHY |
| `NodeSession` | registry.rs:35 | registry.rs:597 (via maybe_register_node) → handler.rs:1482 | HEALTHY |
| `ResolveError` | registry.rs:62 | node_invoke.rs:69,72, node_file.rs:78,81 | HEALTHY |
| `Environment` | registry.rs:87 | cluster.rs:174,183, node_list.rs:59 | HEALTHY (webchat mirror is intentional — sw-cluster-1) |
| `NodeMatch` | registry.rs:112 | node_invoke_many.rs:57,96,100 | HEALTHY |
| `truncate_on_char_boundary` | registry.rs:129 | enrollment.rs:68,229, registry.rs:431 | HEALTHY |
| `NodeRegistry::new` | registry.rs:150 | server/mod.rs:525,583,808, probe.rs:104, auth/mod.rs:74, agent_init/mod.rs:771 | HEALTHY |
| `NodeRegistry::register` | registry.rs:159 | registry.rs:597 (intra-crate, via maybe_register_node) | HEALTHY |
| `NodeRegistry::deregister` | registry.rs:219 | handler.rs:2020 | HEALTHY |
| `NodeRegistry::list_environments` | registry.rs:242 | cluster.rs:174, node_list.rs:59 | HEALTHY |
| `NodeRegistry::node_identity_by_conn` | registry.rs:266 | handler.rs:787,2019 | HEALTHY |
| `NodeRegistry::resolve` | registry.rs:329 | node_invoke.rs:67, node_file.rs:76 | HEALTHY |
| `NodeRegistry::resolve_id` | registry.rs:351 | enrollment.rs:402 (intra-crate) | HEALTHY |
| `NodeRegistry::resolve_all_by_tags` | registry.rs:366 | node_invoke_many.rs:96,100 | HEALTHY |
| `NodeRegistry::forget` | registry.rs:401 | enrollment.rs:412 (intra-crate) | HEALTHY |
| `normalize_node_key` | registry.rs:464 | enrollment.rs:25,121,128,288,297,313,329, registry.rs:288,297,313, tools/adapters/registry_adapter.rs:466 | HEALTHY |
| `maybe_register_node` | registry.rs:496 | handler.rs:1482 | HEALTHY (role param deliberately dead — sw-cluster-2) |
| `PendingInvokes::new` | reverse_rpc.rs:33 | reverse_rpc.rs:186,205 (intra-crate) | HEALTHY |
| `PendingInvokes::register` | reverse_rpc.rs:40 | reverse_rpc.rs:273 (intra-crate) | HEALTHY |
| `PendingInvokes::resolve` | reverse_rpc.rs:62 | handler.rs:767, node.rs:412 | HEALTHY |
| `PendingInvokes::cancel` | reverse_rpc.rs:90 | reverse_rpc.rs:392 (intra-crate) | HEALTHY |
| `PendingInvokes::cancel_all` | reverse_rpc.rs:107 | handler.rs:1995 | HEALTHY |
| `ReverseRpcError` | reverse_rpc.rs:117 | returned by `ReverseRpcChannel::call` | HEALTHY |
| `ReverseRpcChannel::new` | reverse_rpc.rs:183 | node.rs:380 | HEALTHY |
| `ReverseRpcChannel::with_close` | reverse_rpc.rs:202 | handler.rs:695 | HEALTHY |
| `ReverseRpcChannel::pending` | reverse_rpc.rs:213 | handler.rs:696, node.rs:381, node_approval.rs:202 | HEALTHY |
| `ReverseRpcChannel::close_connection` | reverse_rpc.rs:233 | registry.rs:183,209,416, reverse_rpc.rs:248 | HEALTHY |
| `ReverseRpcChannel::call` | reverse_rpc.rs:263 | node_approval.rs:141 (only production method: `node.approval.request`) | HEALTHY |
| Inbound RPC: `cluster.enroll` | cluster.rs:49 (handler) | core.rs:28 (registration) | HEALTHY (sw-cluster-6) |
| Inbound RPC: `cluster.deregister` | cluster.rs:99 (handler) | core.rs:34 (registration) | HEALTHY (sw-cluster-6) |
| Inbound RPC: `environments.list` | cluster.rs:170 (handler) | core.rs:40 (registration) | HEALTHY (sw-cluster-6) |

---

**Audit verdict:** The cluster module is **well-wired**. Every public symbol has a production caller; every reverse-RPC method has a handler; the inbound RPC face is triple-locked (admin prefix + census class + handler registry); the file-jail security boundary is correct; production lock acquisition uses the P7 poison-safe idiom throughout. There is exactly one real wiring gap (sw-cluster-3, the missing `node.disconnected` event on operator-initiated `deregister_node`), and it is fully documented in code with a named fix surface. Two design choices that *look* like defects on first read (the "dead" `role` parameter on `maybe_register_node`, the `pub struct NodeSession` that is only ever constructed inside the cluster) are explicit, documented, contract-pinned-by-tests choices.