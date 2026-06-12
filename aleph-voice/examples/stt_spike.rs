//! SenseVoice spike: transcribe the TTS spike wavs (zh/en/mixed), report timing.
//! Run: cargo run -p aleph-voice --example stt_spike --release [prefix]
//! Optional arg picks which wav set to read: /tmp/aleph_spike_<prefix>_{zh,en,mixed}.wav
//! (default "tts_sid0"; e.g. "tts_sid50", "melo_sid0").
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let prefix = std::env::args().nth(1).unwrap_or_else(|| "tts_sid0".into());
    let home = dirs::home_dir().unwrap().join(".aleph/models/voice/sense-voice-small");
    let p = |f: &str| home.join(f).to_string_lossy().into_owned();

    let t0 = Instant::now();
    // Real sherpa-rs 0.6.8 API (verified against registry source):
    // type is SenseVoiceRecognizer (plan assumed `SenseVoice`); new() returns eyre::Result.
    let mut stt = sherpa_rs::sense_voice::SenseVoiceRecognizer::new(sherpa_rs::sense_voice::SenseVoiceConfig {
        model: p("model.int8.onnx"),
        tokens: p("tokens.txt"),
        language: "auto".into(),
        use_itn: true,
        ..Default::default()
    })
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("load: {:?}", t0.elapsed());

    for tag in ["zh", "en", "mixed"] {
        let path = format!("/tmp/aleph_spike_{prefix}_{tag}.wav");
        let mut r = hound::WavReader::open(&path)?;
        let rate = r.spec().sample_rate;
        let samples: Vec<f32> = r.samples::<i16>().map(|s| s.unwrap() as f32 / 32768.0).collect();
        let t = Instant::now();
        // transcribe takes &[f32] and returns OfflineRecognizerResult { lang, text, timestamps, tokens }.
        let result = stt.transcribe(rate, &samples);
        println!("{tag}: {}ms lang={} -> {:?}", t.elapsed().as_millis(), result.lang, result.text);
    }
    Ok(())
}
