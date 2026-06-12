//! Static model manifest. sha256 values were measured by the Tier-0 spike
//! (docs/superpowers/spikes/2026-06-12-aleph-voice-spike.md) — update both
//! together when bumping model versions.

/// A downloadable model package (bzip2 tarball, unpacked into `<root>/<id>/`).
pub struct ModelSpec {
    /// Directory name under the models root; also the config-facing model id.
    pub id: &'static str,
    /// Download sources in priority order (github → hf-mirror).
    pub urls: &'static [&'static str],
    /// sha256 of the tarball.
    pub sha256: &'static str,
    /// File that proves a complete unpack.
    pub marker: &'static str,
}

pub const SENSE_VOICE_SMALL: ModelSpec = ModelSpec {
    id: "sense-voice-small",
    urls: &[
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2",
        "https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2",
    ],
    sha256: "f6b2a72ebcb1ac7a764d4cfccd886e6bcb2a95c4657c2199d0ba95ed4b9ea71a",
    marker: "model.int8.onnx",
};

pub const KOKORO_V11_ZH: ModelSpec = ModelSpec {
    id: "kokoro-v1.1-zh",
    urls: &[
        "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/kokoro-multi-lang-v1_1.tar.bz2",
        "https://hf-mirror.com/csukuangfj/kokoro-multi-lang-v1_1/resolve/main/kokoro-multi-lang-v1_1.tar.bz2",
    ],
    sha256: "a3f4c73d043860e3fd2e5b06f36795eb81de0fc8e8de6df703245edddd87dbad",
    marker: "model.onnx",
};

/// Look up a spec by config-facing id.
pub fn spec_for(id: &str) -> Option<&'static ModelSpec> {
    match id {
        "sense-voice-small" => Some(&SENSE_VOICE_SMALL),
        "kokoro-v1.1-zh" => Some(&KOKORO_V11_ZH),
        _ => None,
    }
}
