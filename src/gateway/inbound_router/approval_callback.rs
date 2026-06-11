//! Interface 层与 core 审批管理之间的窄接口。
//!
//! `InboundMessageRouter` 保持纯 I/O：把回调 `callback_data` 交给注入的
//! `ApprovalCallbackSink`、再把返回文案渲染回通道 —— 自身不解析、不 resolve。

use async_trait::async_trait;

/// 解析一次审批按钮回调的结果。
pub struct ApprovalCallbackResult {
    /// 是否真正投递了一个待决审批的决策（false = 过期 / 未知 id）。
    pub resolved: bool,
    /// 渲染回通道的用户可见文案。
    pub response_text: String,
}

/// router 注入的审批回调汇。具体实现包 `ExecApprovalManager`。
#[async_trait]
pub trait ApprovalCallbackSink: Send + Sync {
    /// 返回 `Some` 当且仅当 `callback_data` 是审批按钮回调；
    /// `None` 表示非审批回调，router 应放行进正常消息流。
    async fn handle_callback(
        &self,
        callback_data: &str,
        user_id: &str,
    ) -> Option<ApprovalCallbackResult>;
}
