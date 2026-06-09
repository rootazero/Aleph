//! Aleph 集群（单中心非对称节点联邦）。
//!
//! 本模块承载集群的中心侧基础设施。Phase 0a 只实现反向 RPC 传输原语
//! （服务端→已连客户端的带 id 请求/响应关联），后续 Phase 加 NodeRegistry、
//! node.invoke 路由、environments 聚合等。
//!
//! 红线：本模块不含任何 LLM 推理（R7），不进入 `src/harness/`（R10）。

mod node_approval;
mod node_file_cmd;
mod node_runtime;
mod registry;
mod reverse_rpc;

pub(crate) use node_file_cmd::sha256_hex;
pub use node_approval::{ApprovalSlot, CenterApprovalRequester, NODE_APPROVAL_TIMEOUT_MS};
pub use node_file_cmd::{FileReadCommand, FileWriteCommand, MAX_FILE_BYTES};
pub use node_runtime::{CommandTable, NodeCommand};
pub use registry::{
    maybe_register_node, CommandDescriptor, Environment, NodeRegistry, NodeSession,
};
pub use reverse_rpc::{PendingInvokes, ReverseRpcChannel, ReverseRpcError};
