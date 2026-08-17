use crate::sync_primitives::Mutex as StdMutex;

use crate::gateway::interfaces::feishu::api::FeishuApi;
use crate::gateway::interfaces::feishu::types::TypingState;

pub struct ReactionHelper;

impl ReactionHelper {
    pub async fn start_typing(
        api: &FeishuApi,
        msg_id: &str,
        state: &StdMutex<Option<TypingState>>,
    ) {
        match api.add_reaction(msg_id, "Typing").await {
            Ok(reaction_id) => {
                let mut guard = state.lock().unwrap_or_else(|e| e.into_inner());
                *guard = Some(TypingState {
                    message_id: msg_id.to_string(),
                    reaction_id,
                });
            }
            Err(e) => tracing::debug!("Failed to add typing indicator: {e}"),
        }
    }

    pub async fn stop_typing(api: &FeishuApi, state: &StdMutex<Option<TypingState>>) {
        let guard = {
            let mut g = state.lock().unwrap_or_else(|e| e.into_inner());
            g.take()
        };
        if let Some(typing) = guard {
            if let Err(e) = api
                .remove_reaction(&typing.message_id, &typing.reaction_id)
                .await
            {
                tracing::debug!("Failed to remove typing indicator: {e}");
            }
        }
    }
}
