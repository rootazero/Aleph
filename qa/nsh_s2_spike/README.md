# NSH S2 runtime spike / S2 运行时探针

Throwaway reproduction for the S2 runtime spike receipt
([2026-11-23-nsh-panel-s2-runtime-spike.md](../../docs/superpowers/runs/2026-11-23-nsh-panel-s2-runtime-spike.md)).
It is not production code and nothing in `desktop/` or `interfaces/` depends on it.

一次性复现装置，只为 S2 运行时探针回执提供可复跑证据；不是生产代码，生产目录不依赖它。

- `app/`: an `nsh init --template mixed` app. `App.tsx` is the one TSX source rendered on desktop and web; `src/lib.rs` adds the spike probes (`Pulse`, `Probe`, `Tracked`, `spike_probe`).
- `host/`: a Rust-root NSH host shaped like `desktop/nsh-shell`. Its main window mounts `native("aleph/panel-surface")`, whose factory wraps an `AppView` running `out-bundle/bundle.js`, then unmounts it and prints one `ALEPH_S2_SPIKE {json}` report.
- `tools/`: the Chrome probe for the web package and a window lister for desktop captures.
- `evidence/`: the reports and screenshots the receipt cites.

## Replay / 复跑 (macOS arm64)

```bash
cd qa/nsh_s2_spike
nsh test app                       # headless runner, 1 test
nsh build app -o out-bundle        # bundle.js for the desktop host

# Desktop embed (Q1/Q2/Q4). Opens a window for a few seconds, then quits.
cp app/.nsh/rust/Cargo.lock host/Cargo.lock   # only after nsh regenerated it
(cd host && CARGO_TARGET_DIR=../app/target cargo build)
ALEPH_S2_CYCLES=3 ALEPH_S2_HOLD_MS=4000 ALEPH_S2_BUNDLE=$PWD/out-bundle/bundle.js \
  app/target/debug/aleph-s2-spike-host | grep ALEPH_S2_SPIKE

# Web package (Q3). zstd-sys needs a wasm-capable archiver and C compiler.
export AR_wasm32_unknown_unknown="$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/llvm-ar"
export CC_wasm32_unknown_unknown=/usr/bin/clang
nsh component add web              # once per machine
nsh package --target web -o out-web app
(cd "out-web/Aleph S2 Spike" && python3 -m http.server 61465 --bind 127.0.0.1) &
NODE_PATH=<repo root>/node_modules node tools/web-probe.cjs http://127.0.0.1:61465/ /tmp/web.png
```

The active toolchain needs the `llvm-tools` component (`rustup component add llvm-tools`);
without `AR_wasm32_unknown_unknown` the build fails with `failed to find tool "llvm-ar"`.
