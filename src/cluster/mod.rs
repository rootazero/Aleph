//! Aleph 集群（单中心非对称节点联邦）。
//!
//! 中心侧基础设施全栈：反向 RPC 传输原语（[`ReverseRpcChannel`] / [`PendingInvokes`]，
//! 服务端→已连客户端的带 id 请求/响应，靠结构而非 id 区分）、节点登记表
//! （[`NodeRegistry`] 多级寻址 + tag 扇出 + 断线 fail-fast）、节点侧命令分发
//! （[`CommandTable`] allowlist 权威）、文件命令 jail（[`FileReadCommand`] /
//! [`FileWriteCommand`]）与节点审批回中心（[`CenterApprovalRequester`] fail-closed）。
//! 中心侧 LLM 工具 `node_list` / `node_invoke` / `node_invoke_many` / `node_file`
//! 在 `crate::builtin_tools` 经这些原语驱动节点。信任模型为 LAN-trust：节点不持
//! token，连接身份由 connect 帧的参数形状（`commands` + `tags`）声明（见
//! [`maybe_register_node`]）。工程视角全貌见 `docs/reference/CLUSTER.md`。
//!
//! 红线：本模块不含任何 LLM 推理（R7），不进入 `src/harness/`（R10）。

mod node_approval;
mod node_file_cmd;
mod node_runtime;
mod registry;
mod reverse_rpc;

pub use node_approval::{ApprovalSlot, CenterApprovalRequester, NODE_APPROVAL_TIMEOUT_MS};
pub(crate) use node_file_cmd::sha256_hex;
pub use node_file_cmd::{FileReadCommand, FileWriteCommand, MAX_FILE_BYTES};
pub use node_runtime::{CommandTable, NodeCommand};
pub use registry::{
    maybe_register_node, CommandDescriptor, Environment, NodeMatch, NodeRegistry, NodeSession,
    ResolveError,
};
pub use reverse_rpc::{PendingInvokes, ReverseRpcChannel, ReverseRpcError};
