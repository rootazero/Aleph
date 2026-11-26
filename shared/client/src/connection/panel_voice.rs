//! NSH Panel's bounded voice calls. Other client callers use the ordinary lane.

use std::{io, time::Duration};

use aleph_protocol::{
    framing::DEFAULT_MAX_PAYLOAD_BYTES,
    panel_voice::{is_capture_control, VOICE_TURN_CANCEL_METHOD},
};
use serde::Serialize;
use serde_json::Value;

use super::{requests::RequestLane, AlephClient, CliError, CliResult};

const CONTROL_BYTES: usize = 4096;
const MAX_REQUEST_ID: &str = "18446744073709551615"; // u64

fn voice_wire_limit(method: &str) -> usize {
    if is_capture_control(method) || method == "chat.abort" {
        CONTROL_BYTES
    } else {
        DEFAULT_MAX_PAYLOAD_BYTES as usize
    }
}

#[derive(Serialize)]
struct PanelVoiceRequest<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<&'a Value>,
    id: &'a str,
}

impl<'a> PanelVoiceRequest<'a> {
    fn new(method: &'a str, params: Option<&'a Value>, id: &'a str) -> Self {
        Self {
            jsonrpc: "2.0",
            method,
            params,
            id,
        }
    }

    fn fits(&self, limit: usize) -> bool {
        struct Remaining(usize);
        impl io::Write for Remaining {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0 = self.0.checked_sub(bytes.len()).ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Panel voice request exceeds wire budget",
                    )
                })?;
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        serde_json::to_writer(Remaining(limit), self).is_ok()
    }
}

impl AlephClient {
    /// Preflight an ordinary NSH RPC before spawning; allow for the longest request ID.
    pub fn panel_rpc_request_fits(method: &str, params: Option<&Value>) -> bool {
        PanelVoiceRequest::new(method, params, MAX_REQUEST_ID)
            .fits(DEFAULT_MAX_PAYLOAD_BYTES as usize)
    }

    /// Preflight an NSH voice RPC, applying tighter bounds to its controls.
    pub fn panel_voice_request_fits(method: &str, params: Option<&Value>) -> bool {
        PanelVoiceRequest::new(method, params, MAX_REQUEST_ID).fits(voice_wire_limit(method))
    }

    /// Dispatch one NSH voice or voice-bound Chat RPC. NSH uses a dedicated,
    /// never-reconnected Desktop IPC client. The guard runs with the socket's
    /// writer acquired, immediately before admitting the frame to that writer.
    /// An unacknowledged timeout retires this socket instead of freeing remote
    /// capacity for another request. Neither a timeout nor a refusal is replayed.
    pub async fn call_panel_voice(
        &self,
        method: &str,
        params: Option<&Value>,
        timeout: Duration,
        before_send: impl FnOnce() -> CliResult<()>,
    ) -> CliResult<Value> {
        let socket = self.socket();
        let id = self.next_id();
        let request = PanelVoiceRequest::new(method, params, &id);
        if !request.fits(voice_wire_limit(method)) {
            return Err(CliError::Other(
                "Panel voice request exceeds wire budget".into(),
            ));
        }
        let json = serde_json::to_string(&request)?;
        let lane = if is_capture_control(method) {
            RequestLane::CaptureControl
        } else if method == VOICE_TURN_CANCEL_METHOD {
            RequestLane::ProviderCancel
        } else if method == "chat.abort" {
            RequestLane::OwnedChatAbort
        } else {
            RequestLane::Ordinary
        };
        let result = tokio::time::timeout(timeout, async {
            let mut pending = socket.requests.reserve_in(id, method, false, lane)?;
            socket
                .send_text_if(json, || {
                    before_send()?;
                    // A cancelled writer may already have emitted part of a frame.
                    pending.retain_until_reply();
                    Ok(())
                })
                .await?;
            match pending.wait(timeout).await? {
                Ok(result) => Ok(result),
                Err(error) => Err(CliError::Rpc {
                    code: error.code,
                    message: error.message,
                }),
            }
        })
        .await;
        match result {
            Ok(result) if !matches!(result, Err(CliError::Timeout(_))) => result,
            _ => {
                socket.stop();
                Err(CliError::Timeout(
                    "Panel voice RPC deadline exceeded".into(),
                ))
            }
        }
    }

    /// Retire NSH's exact capture owner when its cleanup cannot be confirmed.
    /// This does not reconnect, and all outstanding requests fail on this socket.
    pub fn retire_panel_connection(&self) {
        self.socket().stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::panel_voice::VOICE_TURN_CAPTURE_FINISH_METHOD;
    use serde_json::json;

    #[test]
    fn panel_voice_capture_preflight_counts_jsonrpc_envelope_and_escaped_wire_bytes() {
        let method = VOICE_TURN_CAPTURE_FINISH_METHOD;
        let id = u64::MAX.to_string();
        let params = json!({"fixture":"\n界"});
        let prefix = serde_json::to_vec(&PanelVoiceRequest::new(method, Some(&params), &id))
            .unwrap()
            .len();
        let filler = "x".repeat(CONTROL_BYTES - prefix);
        let exact = json!({"fixture":format!("\n界{filler}")});
        let one_over = json!({"fixture":format!("\n界{filler}x")});
        assert_eq!(
            serde_json::to_vec(&PanelVoiceRequest::new(method, Some(&exact), &id))
                .unwrap()
                .len(),
            4096
        );
        assert!(AlephClient::panel_voice_request_fits(method, Some(&exact)));
        assert!(!AlephClient::panel_voice_request_fits(
            method,
            Some(&one_over)
        ));
    }
}
