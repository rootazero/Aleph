//! Kokoro v1.1-zh spike: synthesize zh/en/mixed sentences, report timing.
//! Run: cargo run -p aleph-voice --example tts_spike --release
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap().join(".aleph/models/voice/kokoro-v1.1-zh");
    let p = |f: &str| home.join(f).to_string_lossy().into_owned();

    let t0 = Instant::now();
    // Real sherpa-rs 0.6.8 API (verified against registry source):
    // KokoroTtsConfig is #[derive(Default)] — length_scale defaults to 0.0, must set 1.0.
    // Extra fields vs plan assumption: lang, onnx_config (OnnxConfig), common_config (CommonTtsConfig).
    // KokoroTts::new returns Self (no Result; null model handle only surfaces at create()).
    let mut tts = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
        model: p("model.onnx"),
        voices: p("voices.bin"),
        tokens: p("tokens.txt"),
        data_dir: p("espeak-ng-data"),
        lexicon: format!("{},{}", p("lexicon-us-en.txt"), p("lexicon-zh.txt")),
        dict_dir: p("dict"),
        length_scale: 1.0,
        ..Default::default()
    });
    println!("load: {:?}", t0.elapsed());

    let cases = [
        ("zh", "你好，我是 Aleph，本地语音引擎已经就绪。"),
        ("en", "Hello, this is the local text to speech engine."),
        ("mixed", "我们用 sherpa-onnx 跑 Kokoro 模型，首包延迟 first packet latency 很关键。"),
    ];
    // Try a few speaker ids — Chinese voices live at some sid range; record which sound right.
    for sid in [0_i32, 1, 50, 100] {
        for (tag, text) in &cases {
            let t = Instant::now();
            // create() returns eyre::Result — not `?`-convertible into anyhow; map explicitly.
            let audio = tts.create(text, sid, 1.0).map_err(|e| anyhow::anyhow!("{e}"))?;
            let ms = t.elapsed().as_millis();
            let out = format!("/tmp/aleph_spike_tts_sid{sid}_{tag}.wav");
            write_wav(&out, &audio.samples, audio.sample_rate)?;
            println!("sid={sid} {tag}: {}ms, {} samples @ {}Hz -> {out}", ms, audio.samples.len(), audio.sample_rate);
        }
    }
    Ok(())
}

fn write_wav(path: &str, samples: &[f32], rate: u32) -> anyhow::Result<()> {
    let spec = hound::WavSpec { channels: 1, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}
