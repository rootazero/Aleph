//! Verify deterministic memory release on engine drop (spec gate #3).
//! Run: cargo run -p aleph-voice --example mem_spike --release
fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

// Compile-time Send/Sync probe (Task 6 consumes this verdict).
fn is_send<T: Send>() {}
fn is_sync<T: Sync>() {}

fn main() -> anyhow::Result<()> {
    is_send::<sherpa_rs::tts::KokoroTts>();
    is_sync::<sherpa_rs::tts::KokoroTts>();
    is_send::<sherpa_rs::sense_voice::SenseVoiceRecognizer>();
    is_sync::<sherpa_rs::sense_voice::SenseVoiceRecognizer>();

    let home = dirs::home_dir().unwrap().join(".aleph/models/voice");
    let k = |f: &str| home.join("kokoro-v1.1-zh").join(f).to_string_lossy().into_owned();
    println!("baseline: {:.1} MB", rss_mb());
    // Two load/drop cycles: distinguishes a true leak (second load stacks
    // another ~1.7GB) from allocator/ORT arena page caching (RSS plateaus).
    for cycle in 1..=2 {
        {
            let mut tts = sherpa_rs::tts::KokoroTts::new(sherpa_rs::tts::KokoroTtsConfig {
                model: k("model.onnx"), voices: k("voices.bin"), tokens: k("tokens.txt"),
                data_dir: k("espeak-ng-data"),
                lexicon: format!("{},{}", k("lexicon-us-en.txt"), k("lexicon-zh.txt")),
                dict_dir: k("dict"), length_scale: 1.0, ..Default::default()
            });
            let _ = tts.create("预热一句。", 0, 1.0).map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("cycle {cycle} tts loaded: {:.1} MB", rss_mb());
        } // drop
        std::thread::sleep(std::time::Duration::from_secs(2));
        println!("cycle {cycle} tts dropped: {:.1} MB", rss_mb());
    }
    Ok(())
}
