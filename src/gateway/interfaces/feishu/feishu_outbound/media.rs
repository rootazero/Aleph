use crate::gateway::channel::ChannelResult;
use crate::gateway::interfaces::feishu::api::FeishuApi;

pub struct MediaHelper;

impl MediaHelper {
    pub async fn upload_image(
        api: &FeishuApi,
        data: Vec<u8>,
        filename: &str,
    ) -> ChannelResult<String> {
        api.upload_image(data, filename)
            .await
            .map_err(crate::gateway::channel::ChannelError::SendFailed)
    }
}
