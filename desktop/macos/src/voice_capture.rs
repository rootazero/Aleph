//! Generation-bound native microphone capture over the private helper RPC.

use std::sync::Arc;

use aleph_desktop::bridge::client::{
    map_bridge_error, BoundBridgeGeneration, BoundStartReply, BridgeRemoteReply,
};
use aleph_desktop::media_types::VoiceCaptureObservation;
use aleph_desktop::traits::VoiceCapture;
use aleph_desktop::{CancellationToken, DesktopError, Result, SwiftBridge};
use aleph_protocol::desktop_bridge::methods::media::{
    voice_capture_start_rejected_for_legacy_busy, VoiceCaptureAck, VoiceCaptureCancelParams,
    VoiceCapturePollParams, VoiceCapturePollResult, VoiceCaptureSealParams, VoiceCaptureSealResult,
    VoiceCaptureStartParams, METHOD_VOICE_CAPTURE_CANCEL, METHOD_VOICE_CAPTURE_POLL,
    METHOD_VOICE_CAPTURE_SEAL, METHOD_VOICE_CAPTURE_START, VOICE_CAPTURE_FRAME_SAMPLES,
    VOICE_CAPTURE_MAX_POLL_FRAMES,
};
use aleph_protocol::panel_voice::{MAX_UTTERANCE_SAMPLES, SAMPLE_RATE_HZ, WAV_HEADER_BYTES};
use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::Value;

// The absolute cursor spans the whole attempt, not the rolling PCM buffer or
// the selected utterance. The owner enforces the 45-second attempt deadline.
const MAX_CAPTURE_OFFSET: u32 = SAMPLE_RATE_HZ * 45;
const MAX_CLIP_SAMPLES: u32 = MAX_UTTERANCE_SAMPLES as u32;
const SAMPLE_BYTES: usize = size_of::<i16>();

pub(crate) struct MacVoiceCapture {
    binding: BoundBridgeGeneration,
    capture_id: String,
}

impl MacVoiceCapture {
    pub(crate) async fn begin(
        bridge: &Arc<SwiftBridge>,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        let capture_id = uuid::Uuid::new_v4().to_string();
        let binding = bridge
            .call_bound_start(
                METHOD_VOICE_CAPTURE_START,
                VoiceCaptureStartParams {
                    capture_id: capture_id.clone(),
                    sample_rate_hz: SAMPLE_RATE_HZ,
                    frame_samples: VOICE_CAPTURE_FRAME_SAMPLES,
                    max_clip_samples: MAX_CLIP_SAMPLES,
                },
                cancel,
                capture_activation,
            )
            .await?;
        Ok(Self {
            binding,
            capture_id,
        })
    }
}

#[async_trait]
impl VoiceCapture for MacVoiceCapture {
    async fn poll(
        &mut self,
        after_sample: u32,
        cancel: &CancellationToken,
    ) -> Result<Vec<VoiceCaptureObservation>> {
        if after_sample > MAX_CAPTURE_OFFSET
            || !after_sample.is_multiple_of(VOICE_CAPTURE_FRAME_SAMPLES)
        {
            return Err(invalid_capture("invalid voice capture cursor"));
        }
        self.binding
            .call(
                METHOD_VOICE_CAPTURE_POLL,
                VoiceCapturePollParams {
                    capture_id: self.capture_id.clone(),
                    after_sample,
                },
                cancel,
                |reply| capture_observations(reply, after_sample),
            )
            .await
    }

    async fn seal(
        self: Box<Self>,
        first_sample: u32,
        end_sample: u32,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        let Some(samples) = end_sample.checked_sub(first_sample).filter(|samples| {
            end_sample <= MAX_CAPTURE_OFFSET && (1..=MAX_CLIP_SAMPLES).contains(samples)
        }) else {
            self.cancel().await?;
            return Err(invalid_capture("invalid voice capture seal range"));
        };
        self.binding
            .finish(
                METHOD_VOICE_CAPTURE_SEAL,
                VoiceCaptureSealParams {
                    capture_id: self.capture_id,
                    first_sample,
                    end_sample,
                },
                cancel,
                |reply| capture_wav(reply, samples),
            )
            .await
    }

    async fn cancel(self: Box<Self>) -> Result<()> {
        self.binding
            .finish(
                METHOD_VOICE_CAPTURE_CANCEL,
                VoiceCaptureCancelParams {
                    capture_id: self.capture_id,
                },
                &CancellationToken::new(),
                capture_ack,
            )
            .await
    }
}

fn invalid_capture(reason: &'static str) -> DesktopError {
    DesktopError::BridgeFailed(reason.into())
}

fn capture_object(reply: BridgeRemoteReply) -> Result<Value> {
    // An RPC error alone says nothing about microphone ownership or release.
    let result = reply.map_err(map_bridge_error)?;
    if !result.is_object() {
        return Err(invalid_capture("invalid voice capture reply object"));
    }
    Ok(result)
}

fn capture_activation(reply: BridgeRemoteReply) -> BoundStartReply {
    if reply
        .as_ref()
        .is_err_and(voice_capture_start_rejected_for_legacy_busy)
    {
        return BoundStartReply::NotAcquired(invalid_capture(
            "legacy recording owns the microphone",
        ));
    }
    match capture_ack(reply) {
        Ok(()) => BoundStartReply::Active,
        Err(error) => BoundStartReply::Unproven(error),
    }
}

fn capture_ack(reply: BridgeRemoteReply) -> Result<()> {
    let _: VoiceCaptureAck = serde_json::from_value(capture_object(reply)?)
        .map_err(|_| invalid_capture("invalid voice capture acknowledgement"))?;
    Ok(())
}

fn capture_observations(
    reply: BridgeRemoteReply,
    after: u32,
) -> Result<Vec<VoiceCaptureObservation>> {
    let result = capture_object(reply)?;
    let invalid = || invalid_capture("invalid voice capture observations");
    let frames = result["observations"].as_array().ok_or_else(invalid)?;
    if frames.len() > VOICE_CAPTURE_MAX_POLL_FRAMES || frames.iter().any(|frame| !frame.is_object())
    {
        return Err(invalid());
    }
    let result: VoiceCapturePollResult = serde_json::from_value(result).map_err(|_| invalid())?;
    let mut cursor = after;
    result
        .observations
        .into_iter()
        .map(|frame| {
            cursor += VOICE_CAPTURE_FRAME_SAMPLES;
            if cursor > MAX_CAPTURE_OFFSET
                || frame.end_sample != cursor
                || !(0.0..=1.0).contains(&frame.rms)
            {
                return Err(invalid());
            }
            Ok(VoiceCaptureObservation {
                end_sample: frame.end_sample,
                rms: frame.rms as f32,
            })
        })
        .collect()
}

fn capture_wav(reply: BridgeRemoteReply, samples: u32) -> Result<Vec<u8>> {
    let invalid = || invalid_capture("invalid voice capture WAV");
    let result: VoiceCaptureSealResult =
        serde_json::from_value(capture_object(reply)?).map_err(|_| invalid())?;
    let pcm_bytes = samples as usize * SAMPLE_BYTES;
    let wav_bytes = WAV_HEADER_BYTES + pcm_bytes;
    if result.wav_base64.len() != wav_bytes.div_ceil(3) * 4 {
        return Err(invalid());
    }
    let wav = STANDARD.decode(result.wav_base64).map_err(|_| invalid())?;
    if wav.len() != wav_bytes
        || &wav[..4] != b"RIFF"
        || wav[4..8] != ((wav_bytes - 8) as u32).to_le_bytes()
        || &wav[8..24] != b"WAVEfmt \x10\0\0\0\x01\0\x01\0" // PCM format, mono
        || wav[24..28] != SAMPLE_RATE_HZ.to_le_bytes()
        || wav[28..32] != (SAMPLE_RATE_HZ * SAMPLE_BYTES as u32).to_le_bytes()
        || wav[32..34] != (SAMPLE_BYTES as u16).to_le_bytes()
        || wav[34..36] != (i16::BITS as u16).to_le_bytes()
        || &wav[36..40] != b"data"
        || wav[40..44] != (pcm_bytes as u32).to_le_bytes()
    {
        return Err(invalid());
    }
    Ok(wav)
}

#[cfg(test)]
mod tests;
