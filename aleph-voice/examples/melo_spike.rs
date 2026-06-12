//! MeloTTS-zh_en spike (TTS fallback after Kokoro v1.1-zh failed the zh
//! listening gate): synthesize the same zh/en/mixed sentences, report timing
//! and RSS, write wavs for the second listening round.
//! Run: cargo run -p aleph-voice --example melo_spike --release
use std::time::Instant;

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

fn main() -> anyhow::Result<()> {
    let home = dirs::home_dir().unwrap().join(".aleph/models/voice/vits-melo-tts-zh_en");
    let p = |f: &str| home.join(f).to_string_lossy().into_owned();

    println!("baseline rss: {:.1} MB", rss_mb());
    let t0 = Instant::now();
    // sherpa-rs 0.6.8 VitsTtsConfig is #[derive(Default)] — noise_scale /
    // noise_scale_w / length_scale all default to 0.0; set the sherpa-onnx
    // canonical VITS values explicitly. Melo zh_en uses lexicon + jieba dict
    // (no espeak data_dir).
    //
    // KNOWN sherpa-rs 0.6.8 BUG: CommonTtsConfig::to_raw() does
    // `rule_fsts.map(|v| v.as_ptr())` — the CString is dropped inside the map
    // closure, so any non-empty rule_fsts/rule_fars passes a DANGLING pointer
    // to the C API (observed: C side read the model.onnx path as the FST list
    // and aborted). Leave rule_fsts empty here (spike sentences carry no
    // digits); Task 6 must not use rule_fsts through sherpa-rs 0.6.8.
    let mut tts = sherpa_rs::tts::VitsTts::new(sherpa_rs::tts::VitsTtsConfig {
        model: p("model.onnx"),
        lexicon: p("lexicon.txt"),
        tokens: p("tokens.txt"),
        dict_dir: p("dict"),
        length_scale: 1.0,
        noise_scale: 0.667,
        noise_scale_w: 0.8,
        silence_scale: 0.2,
        ..Default::default()
    });
    println!("load: {:?}, rss after load: {:.1} MB", t0.elapsed(), rss_mb());

    let cases = [
        ("zh", "你好，我是 Aleph，本地语音引擎已经就绪。"),
        ("en", "Hello, this is the local text to speech engine."),
        ("mixed", "我们用 sherpa-onnx 跑 Kokoro 模型，首包延迟 first packet latency 很关键。"),
    ];
    // Melo zh_en is expected to be single-speaker; probe sid 0 and 1 to confirm.
    for sid in [0_i32, 1] {
        for (tag, text) in &cases {
            let t = Instant::now();
            match tts.create(text, sid, 1.0) {
                Ok(audio) => {
                    let ms = t.elapsed().as_millis();
                    let out = format!("/tmp/aleph_spike_melo_sid{sid}_{tag}.wav");
                    write_wav(&out, &audio.samples, audio.sample_rate)?;
                    let secs = audio.samples.len() as f64 / audio.sample_rate as f64;
                    println!("sid={sid} {tag}: {}ms, {} samples @ {}Hz ({:.2}s audio, RTF={:.2}) -> {out}",
                        ms, audio.samples.len(), audio.sample_rate, secs, ms as f64 / 1000.0 / secs);
                }
                Err(e) => println!("sid={sid} {tag}: FAILED — {e}"),
            }
        }
    }
    println!("rss after synth: {:.1} MB", rss_mb());
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
