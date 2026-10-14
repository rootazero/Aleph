//! Test-only stand-in for ffmpeg, compiled by `rustc` into a native host
//! executable by the `recording` tests. It is NOT a production module: it is
//! not declared with `mod`, only `include_str!`-ed by the test helper, and is
//! std-only on purpose (no shell, no cmd, no extra crates).
//!
//! Behaviour is selected by the executable's own file stem, so one binary
//! image serves every scenario (the test copies it under a mode-named file):
//!
//! | stem          | behaviour                                                        |
//! |---------------|------------------------------------------------------------------|
//! | `wedged`      | never reads stdin; sleeps forever (until killed)                 |
//! | `slow`        | reads at most 1024 bytes per round, sleeps 50ms, until EOF       |
//! | `failing`     | copies stdin to the LAST argv (the output file), then exits 1    |
//! | `short-lived` | reads 256 bytes of stdin, then exits 0                           |
//! | `no-touch`    | drains stdin to EOF, never creates the output, exits 0           |
//!
//! Unknown mode or I/O failure exits non-zero with a message on stderr; it
//! never silently succeeds.

use std::io::{self, BufRead, Read, Write};
use std::time::Duration;

fn main() {
    if let Err(e) = run() {
        eprintln!("recording_test_encoder: {e}");
        std::process::exit(2);
    }
}

fn run() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mode = exe
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| io::Error::other("executable has no usable file stem"))?
        .to_owned();
    match mode.as_str() {
        "wedged" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        "slow" => {
            let mut stdin = io::stdin().lock();
            loop {
                let n = {
                    let buf = stdin.fill_buf()?;
                    buf.len().min(1024)
                };
                if n == 0 {
                    return Ok(());
                }
                stdin.consume(n);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        "failing" => {
            let out = std::env::args_os()
                .last()
                .filter(|_| std::env::args_os().count() > 1)
                .ok_or_else(|| io::Error::other("failing mode needs an output path argument"))?;
            let mut file = std::fs::File::create(out)?;
            io::copy(&mut io::stdin().lock(), &mut file)?;
            file.flush()?;
            drop(file);
            std::process::exit(1);
        }
        "short-lived" => {
            // `head -c 256` semantics: stop after 256 bytes, or at EOF if fewer.
            io::copy(&mut io::stdin().lock().take(256), &mut io::sink())?;
            Ok(())
        }
        "no-touch" => {
            io::copy(&mut io::stdin().lock(), &mut io::sink())?;
            Ok(())
        }
        other => Err(io::Error::other(format!("unknown encoder mode `{other}`"))),
    }
}
