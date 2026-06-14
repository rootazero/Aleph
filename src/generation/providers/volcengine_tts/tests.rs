//! Unit tests for the Volcengine TTS provider — pure helpers and the response
//! envelope. Network calls are out of scope (covered by integration tests).

use super::*;

#[test]
fn rejects_empty_api_key() {
    let err = VolcengineTtsProvider::new("", None, None, None).unwrap_err();
    assert!(err.to_string().to_lowercase().contains("token"));
}

#[test]
fn rejects_whitespace_api_key() {
    assert!(VolcengineTtsProvider::new("   ", None, None, None).is_err());
}

// === resolve_endpoint: appid / cluster carried in the query string ===

#[test]
fn resolve_endpoint_appends_path_to_bare_host() {
    let (endpoint, appid, cluster) = resolve_endpoint("https://openspeech.bytedance.com");
    assert_eq!(endpoint, "https://openspeech.bytedance.com/api/v1/tts");
    assert_eq!(appid, None);
    assert_eq!(cluster, None);
}

#[test]
fn resolve_endpoint_extracts_appid_from_query() {
    let (endpoint, appid, cluster) =
        resolve_endpoint("https://openspeech.bytedance.com?appid=1234567890");
    assert_eq!(endpoint, "https://openspeech.bytedance.com/api/v1/tts");
    assert_eq!(appid.as_deref(), Some("1234567890"));
    assert_eq!(cluster, None);
}

#[test]
fn resolve_endpoint_extracts_appid_and_cluster() {
    let (endpoint, appid, cluster) =
        resolve_endpoint("https://openspeech.bytedance.com?appid=abc&cluster=volcano_tts_concurr");
    assert_eq!(endpoint, "https://openspeech.bytedance.com/api/v1/tts");
    assert_eq!(appid.as_deref(), Some("abc"));
    assert_eq!(cluster.as_deref(), Some("volcano_tts_concurr"));
}

#[test]
fn resolve_endpoint_preserves_full_path() {
    let (endpoint, appid, _) =
        resolve_endpoint("https://openspeech.bytedance.com/api/v1/tts?appid=xyz");
    assert_eq!(endpoint, "https://openspeech.bytedance.com/api/v1/tts");
    assert_eq!(appid.as_deref(), Some("xyz"));
}

#[test]
fn resolve_endpoint_blank_appid_is_none() {
    let (_, appid, _) = resolve_endpoint("https://openspeech.bytedance.com?appid=");
    assert_eq!(appid, None);
}

#[test]
fn new_parses_appid_into_struct() {
    let p = VolcengineTtsProvider::new("tok", Some("https://h?appid=999".to_string()), None, None)
        .unwrap();
    assert_eq!(p.appid, "999");
    assert_eq!(p.cluster, DEFAULT_CLUSTER);
    assert_eq!(p.voice, DEFAULT_VOICE);
}

// === build_request ===

#[test]
fn build_request_shape() {
    let req = build_request(
        "app1",
        "tok1",
        "volcano_tts",
        "zh_female_cancan_mars_bigtts",
        "mp3",
        Some(1.5),
        "你好",
    );
    assert_eq!(req.app.appid, "app1");
    assert_eq!(req.app.token, "tok1");
    assert_eq!(req.app.cluster, "volcano_tts");
    assert_eq!(req.audio.voice_type, "zh_female_cancan_mars_bigtts");
    assert_eq!(req.audio.encoding, "mp3");
    assert_eq!(req.audio.speed_ratio, Some(1.5));
    assert_eq!(req.request.text, "你好");
    assert_eq!(req.request.operation, "query");
    assert_eq!(req.request.text_type, "plain");
    assert!(!req.request.reqid.is_empty());
}

#[test]
fn build_request_clamps_speed() {
    let too_fast = build_request("a", "t", "c", "v", "mp3", Some(9.0), "x");
    assert_eq!(too_fast.audio.speed_ratio, Some(SPEED_MAX));
    let too_slow = build_request("a", "t", "c", "v", "mp3", Some(0.01), "x");
    assert_eq!(too_slow.audio.speed_ratio, Some(SPEED_MIN));
    let none = build_request("a", "t", "c", "v", "mp3", None, "x");
    assert_eq!(none.audio.speed_ratio, None);
}

// === resolve_format ===

#[test]
fn resolve_format_defaults_to_mp3() {
    assert_eq!(resolve_format(None).unwrap(), "mp3");
}

#[test]
fn resolve_format_accepts_known() {
    for f in ["mp3", "wav", "pcm", "ogg_opus"] {
        assert_eq!(resolve_format(Some(f)).unwrap(), f);
    }
}

#[test]
fn resolve_format_rejects_unknown() {
    assert!(resolve_format(Some("flac")).is_err());
}

// === response envelope ===

#[test]
fn response_success_requires_code_3000() {
    let ok: TtsResponse =
        serde_json::from_str(r#"{"code":3000,"message":"Success","data":"AAAA"}"#).unwrap();
    assert!(ok.is_success());

    let bad: TtsResponse =
        serde_json::from_str(r#"{"code":3001,"message":"bad request"}"#).unwrap();
    assert!(!bad.is_success());
    assert_eq!(bad.best_message(), "bad request");
}

#[test]
fn response_best_message_falls_back_to_code() {
    let r: TtsResponse = serde_json::from_str(r#"{"code":3050}"#).unwrap();
    assert_eq!(r.best_message(), "Volcengine code=3050");
}

#[test]
fn content_type_mapping() {
    assert_eq!(content_type_for_format("mp3"), "audio/mpeg");
    assert_eq!(content_type_for_format("wav"), "audio/wav");
    assert_eq!(content_type_for_format("pcm"), "audio/pcm");
    assert_eq!(content_type_for_format("ogg_opus"), "audio/ogg");
}
