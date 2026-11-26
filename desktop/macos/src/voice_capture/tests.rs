use super::*;
use serde_json::json;

#[test]
fn capture_ack_requires_exact_empty_object() {
    assert!(capture_ack(Ok(json!({}))).is_ok());
    for invalid in [json!({"released": true}), json!([]), Value::Null] {
        assert!(capture_ack(Ok(invalid)).is_err());
    }
}

#[test]
fn capture_activation_only_exempts_exact_legacy_mic_refusal() {
    use aleph_protocol::desktop_bridge::envelope::RpcError;
    let refusal = |code, data| {
        Err(RpcError {
            code,
            message: "busy".into(),
            data: Some(data),
        })
    };
    let legacy = json!({"reason": "voice_capture_legacy_busy"});
    assert!(matches!(
        capture_activation(refusal(-32004, legacy.clone())),
        BoundStartReply::NotAcquired(_)
    ));
    assert!(capture_ack(refusal(-32004, legacy.clone())).is_err());
    for error in [
        refusal(-32000, legacy),
        refusal(-32004, json!({"reason": "other"})),
        refusal(
            -32004,
            json!({"reason": "voice_capture_legacy_busy", "extra": true}),
        ),
    ] {
        assert!(matches!(
            capture_activation(error),
            BoundStartReply::Unproven(_)
        ));
    }
}

#[test]
fn capture_observations_accept_empty_and_contiguous_absolute_offsets() {
    let empty = capture_observations(Ok(json!({"observations": []})), MAX_CAPTURE_OFFSET).unwrap();
    assert!(empty.is_empty());
    let frames = capture_observations(
        Ok(json!({"observations": [
            {"end_sample": 400_800, "rms": 0.125},
            {"end_sample": 401_600, "rms": 0.5}
        ]})),
        400_000,
    )
    .unwrap();
    assert_eq!(
        frames[0],
        VoiceCaptureObservation {
            end_sample: 400_800,
            rms: 0.125
        }
    );
    assert_eq!(
        frames[1],
        VoiceCaptureObservation {
            end_sample: 401_600,
            rms: 0.5
        }
    );
}

#[test]
fn capture_observations_reject_gaps_duplicates_out_of_range_and_oversized_batches() {
    for frames in [
        json!([{"end_sample": 1600, "rms": 0.0}]),
        json!([{"end_sample": 800, "rms": 0.0}, {"end_sample": 800, "rms": 0.0}]),
        json!([{"end_sample": 800, "rms": 1.00000001}]),
        json!([{"end_sample": 800, "rms": -0.1}]),
        json!([[800, 0.0]]),
        json!((1..=17)
            .map(|i| json!({"end_sample": i * 800, "rms": 0.0}))
            .collect::<Vec<_>>()),
    ] {
        assert!(capture_observations(Ok(json!({"observations": frames})), 0).is_err());
    }
    assert!(capture_observations(
        Ok(json!({"observations": [{"end_sample": MAX_CAPTURE_OFFSET + 800, "rms": 0.0}]})),
        MAX_CAPTURE_OFFSET
    )
    .is_err());
}

const TWO_SAMPLE_WAV: &[u8] = b"RIFF\x28\0\0\0WAVEfmt \x10\0\0\0\x01\0\x01\0\x80\x3e\0\0\0\x7d\0\0\x02\0\x10\0data\x04\0\0\0\xff\xff\x01\0";

#[test]
fn capture_wav_requires_exact_sample_span_and_canonical_pcm_header() {
    let reply = |wav: &[u8]| Ok(json!({"wav_base64": STANDARD.encode(wav)}));
    assert_eq!(
        capture_wav(reply(TWO_SAMPLE_WAV), 2).unwrap(),
        TWO_SAMPLE_WAV
    );
    assert!(capture_wav(reply(TWO_SAMPLE_WAV), 1).is_err());
    assert!(capture_wav(reply(TWO_SAMPLE_WAV), 3).is_err());
    let reply_with_trailing_byte = [TWO_SAMPLE_WAV, &[0]].concat();
    assert!(capture_wav(reply(&reply_with_trailing_byte), 2).is_err());
    for offset in [0, 4, 8, 12, 16, 20, 22, 24, 28, 32, 34, 36, 40] {
        let mut invalid = TWO_SAMPLE_WAV.to_vec();
        invalid[offset] ^= 1;
        assert!(
            capture_wav(reply(&invalid), 2).is_err(),
            "accepted header mutation at {offset}"
        );
    }
    assert!(capture_wav(Ok(json!({"wav_base64": "A".repeat(1_000_000)})), 2).is_err());
}
