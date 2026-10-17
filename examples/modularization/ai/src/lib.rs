//! Standalone API-only consumer. No Core adapter or runtime is installed here.
use aleph_slot_api::{
    async_trait, AiProvider, AiProviderContract, DeltaSink, ProviderDelta, ProviderResponse,
    RequestPayload, Result, UnifiedMessage,
};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

struct Echo<'a> {
    prefix: &'a str,
}

impl AiProviderContract for Echo<'_> {
    fn process<'a>(
        &'a self,
        payload: RequestPayload<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async move {
            Ok(ProviderResponse::text_only(format!(
                "{}{}",
                self.prefix,
                UnifiedMessage::extract_all_text(payload.messages),
            )))
        })
    }

    fn name(&self) -> &str {
        "external-echo"
    }

    fn color(&self) -> &str {
        "#123456"
    }
}

struct BorrowedSink<'a>(&'a Mutex<Vec<String>>);

#[async_trait]
impl DeltaSink for BorrowedSink<'_> {
    async fn on_delta(&self, delta: &ProviderDelta) {
        match delta {
            ProviderDelta::TextDelta(text) => self.0.lock().unwrap().push(text.clone()),
            ProviderDelta::Done(reason) => self.0.lock().unwrap().push(format!("done:{reason:?}")),
            _ => {}
        }
    }
}

/// An external caller can supply its own executor; this crate needs none.
/// Both the message buffer and immutable dyn sink are borrowed, not 'static.
pub async fn borrowed_round_trip(question: &str) -> Result<(ProviderResponse, Vec<String>)> {
    let prefix = String::from("echo: ");
    let alias: Box<dyn AiProvider + '_> = Box::new(Echo { prefix: &prefix });
    let provider: &dyn AiProviderContract = alias.as_ref();
    let messages = vec![UnifiedMessage::user(question)];
    let system = String::from("Reply with the supplied message.");
    let events = Mutex::new(Vec::new());
    let sink = BorrowedSink(&events);
    let immutable_sink: &dyn DeltaSink = &sink;
    let response = provider
        .execute_streaming_dyn(
            RequestPayload::new(&messages).with_system(Some(&system)),
            immutable_sink,
        )
        .await?;
    Ok((response, events.into_inner().unwrap()))
}
