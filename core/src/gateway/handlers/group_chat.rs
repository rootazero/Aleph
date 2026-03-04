//! Group Chat RPC handlers (placeholders -- wired with GroupChatOrchestrator at runtime)

use crate::gateway::protocol::{JsonRpcRequest, JsonRpcResponse, INTERNAL_ERROR};

const RUNTIME_REQUIRED: &str = "requires GroupChatOrchestrator runtime - wire Gateway first";

pub async fn handle_start_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.start {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_continue_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.continue {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_mention_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.mention {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_end_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.end {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_list_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.list {}", RUNTIME_REQUIRED),
    )
}

pub async fn handle_history_placeholder(req: JsonRpcRequest) -> JsonRpcResponse {
    JsonRpcResponse::error(
        req.id,
        INTERNAL_ERROR,
        format!("group_chat.history {}", RUNTIME_REQUIRED),
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use crate::gateway::protocol::JsonRpcRequest;

    #[test]
    fn test_group_chat_handlers_registered() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        assert!(registry.has_method("group_chat.start"));
        assert!(registry.has_method("group_chat.continue"));
        assert!(registry.has_method("group_chat.mention"));
        assert!(registry.has_method("group_chat.end"));
        assert!(registry.has_method("group_chat.list"));
        assert!(registry.has_method("group_chat.history"));
    }

    #[tokio::test]
    async fn test_start_placeholder_returns_error() {
        let registry = crate::gateway::handlers::HandlerRegistry::new();
        let req = JsonRpcRequest::with_id("group_chat.start", Some(json!({})), json!(1));
        let resp = registry.handle(&req).await;
        assert!(resp.is_error());
    }
}
