use super::*;
use std::time::Duration;

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(30)
}

#[test]
fn offline_stt_windows_token_requires_exact_requested_locale() {
    let usa = requested_langid("en-US").unwrap();
    let uk = requested_langid("en-GB").unwrap();
    let neutral = requested_langid("en").unwrap();
    let china = requested_langid("zh-CN").unwrap();
    assert_eq!((usa, uk, neutral, china), (0x409, 0x809, 0x9, 0x804));
    let metadata = "409; 0804;invalid";
    assert!(token_supports_langid(metadata, usa));
    assert!(token_supports_langid(metadata, china));
    assert!(!token_supports_langid(metadata, uk));
    assert!(!token_supports_langid(metadata, neutral));
    assert!(!token_supports_langid("1409", usa));
    for invalid in ["", "en-US\0en-GB", "not-a-windows-locale"] {
        assert!(matches!(
            requested_langid(invalid),
            Err(DesktopError::UnsupportedLanguage(_))
        ));
    }
}

fn wav(pcm: &[u8]) -> Vec<u8> {
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend((WAV_HEADER_BYTES as u32 + pcm.len() as u32 - 8).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]);
    wav.extend(SAMPLE_RATE_HZ.to_le_bytes());
    wav.extend((SAMPLE_RATE_HZ * 2).to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend((pcm.len() as u32).to_le_bytes());
    wav.extend(pcm);
    wav
}

#[test]
fn offline_stt_windows_voice_pcm_excludes_header_and_rejects_invalid_input() {
    let pcm = [1, 0, 0, 0, 255, 255];
    let canonical = wav(&pcm);
    assert_eq!(voice_pcm(&canonical).unwrap(), pcm);
    let mut wrong_rate = canonical.clone();
    wrong_rate[24..28].copy_from_slice(&22050u32.to_le_bytes());
    for invalid in [
        wav(&[]),
        wav(&[0]),
        canonical[..canonical.len() - 1].to_vec(),
        wrong_rate,
        vec![0; MAX_WAV_BYTES + 1],
    ] {
        assert!(voice_pcm(&invalid).is_err());
    }
}

#[test]
fn offline_stt_windows_successful_eos_accepts_empty_finals_but_not_failure() {
    let mut transcript = Transcript::default();
    transcript.append("   ", 0).unwrap();
    assert_eq!(finish_stream(transcript, HRESULT(0)).unwrap(), "");
    let error = finish_stream(Transcript::default(), HRESULT(0x80004005_u32 as i32)).unwrap_err();
    assert_eq!(
        error.to_string(),
        platform("SAPI HRESULT 0x80004005").to_string()
    );
}

#[test]
fn offline_stt_windows_bad_input_keeps_typed_errors() {
    let absent = std::path::Path::new("missing-offline-voice-input.wav");
    assert!(matches!(
        recognize(absent, Some("not-a-windows-locale"), deadline()),
        Err(DesktopError::UnsupportedLanguage(_))
    ));
    let result = recognize(absent, Some("zh-CN"), deadline());
    assert!(
        matches!(result, Err(DesktopError::PlatformError(message)) if message == "read offline speech input: NotFound")
    );
}

#[test]
fn offline_stt_windows_transcript_keeps_all_finals_and_caps_utf8_bytes() {
    let mut transcript = Transcript::default();
    transcript
        .append("one", SPAF_ONE_TRAILING_SPACE.0 as u8)
        .unwrap();
    transcript
        .append("two", SPAF_TWO_TRAILING_SPACES.0 as u8)
        .unwrap();
    transcript
        .append(".", SPAF_CONSUME_LEADING_SPACES.0 as u8)
        .unwrap();
    transcript.append("   ", 0).unwrap();
    transcript.append("今天天气很好", 0).unwrap();
    assert_eq!(
        finish_stream(transcript, HRESULT(0)).unwrap(),
        "one two.今天天气很好"
    );

    let mut transcript = Transcript::default();
    transcript
        .append(&"é".repeat(MAX_TRANSCRIPT_BYTES / 2), 0)
        .unwrap();
    assert!(transcript.append("!", 0).is_err());
}

#[test]
fn offline_stt_windows_deadline_fails_closed() {
    assert!(recognize_pcm(&[], None, Instant::now(), |_| {}).is_err());
}

#[test]
#[ignore = "requires installed Chinese SAPI dictation and ALEPH_WINDOWS_SAPI_TEST_WAV"]
fn offline_stt_windows_installed_token_smoke() {
    let path = std::env::var("ALEPH_WINDOWS_SAPI_TEST_WAV").unwrap();
    let path = std::path::Path::new(&path);
    let expected = "今天天气很好让我们一起去公园散步";
    for locale in [Some("zh-CN"), None] {
        let result = recognize(path, locale, deadline()).unwrap();
        assert_eq!(
            result.language, "zh-CN",
            "smoke host needs Chinese as SAPI default"
        );
        assert!(
            result.text.replace('。', "") == expected,
            "synthetic transcription mismatch"
        );
    }
    assert!(matches!(
        recognize(path, Some("zh-TW"), deadline()),
        Err(DesktopError::UnsupportedLanguage(_))
    ));

    let wav = std::fs::read(path).unwrap();
    let mut events = Vec::new();
    let langid = requested_langid("zh-CN").unwrap();
    let result = recognize_pcm(
        voice_pcm(&wav).unwrap(),
        Some(("zh-CN", langid)),
        deadline(),
        |event| events.push(event.0),
    )
    .unwrap();
    assert!(
        events
            .iter()
            .filter(|&&id| id == SPEI_RECOGNITION.0)
            .count()
            >= 2
    );
    assert_eq!(events.first(), Some(&SPEI_START_SR_STREAM.0));
    assert_eq!(events.last(), Some(&SPEI_END_SR_STREAM.0));
    assert!(
        result.text.replace('。', "") == expected,
        "synthetic transcription mismatch"
    );
}

#[test]
#[ignore = "requires installed Chinese SAPI dictation as the default token"]
fn offline_stt_windows_installed_silence_and_real_eos_return_empty_text() {
    for (pcm, expected_events) in [
        (vec![0; 32_000], &[][..]),
        (
            [1, 0].repeat(16_000),
            &[SPEI_START_SR_STREAM.0, SPEI_END_SR_STREAM.0][..],
        ),
    ] {
        let langid = requested_langid("zh-CN").unwrap();
        for locale in [None, Some(("zh-CN", langid))] {
            let mut events = Vec::new();
            let result =
                recognize_pcm(&pcm, locale, deadline(), |event| events.push(event.0)).unwrap();
            assert!(result.text.is_empty());
            assert_eq!(result.language, "zh-CN");
            assert_eq!(events, expected_events);
        }
    }
}
