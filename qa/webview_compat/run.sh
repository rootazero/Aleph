#!/usr/bin/env bash
# Real-machine assertions for the cross-platform WebView resource-control work.
#
#   Usage: qa/webview_compat/run.sh <linux|macos> [base-url]
#
# Windows is verified on the developer machine directly (see the plan, task 17);
# this script exists because Linux (WebKitGTK/GStreamer) and macOS (WKWebView)
# behaviours cannot be observed from there.
#
# READ THIS BEFORE FILING A FAILURE: only the Windows-side guards in this change
# were falsified by mutation. The assertions below are correct in SHAPE, but the
# first time one goes red the red may be the assertion rather than the code.
# Every assertion therefore prints the value it actually read.
#
# One more asymmetry worth knowing before you file anything against the
# `flat-on-linux` / `wkwebview-baseline` manual steps below: the shell's
# platform marker (`SHELL_MARKER_JS` in desktop/shell/src/main.rs) is three
# `#[cfg(target_os = ...)]` arms, and this repo has only ever been built and
# tested on Windows — so only the `windows` arm has ever even been *compiled*,
# let alone mutation-tested. The `macos` arm and the `not(any(macos, windows))`
# (linux) arm are unverified in the strongest sense: never built, never run,
# never falsified. If `data-platform` is missing or wrong on Linux, that arm
# has never been proven to work at all — don't assume your install is broken
# before assuming the arm itself might be.
set -uo pipefail

PLATFORM="${1:-}"
BASE="${2:-http://127.0.0.1:18790}"
DIST="interfaces/webchat/dist"

case "$PLATFORM" in
  linux|macos) ;;
  *) echo "usage: $0 <linux|macos> [base-url]" >&2; exit 2 ;;
esac

pass=0; fail=0; skip=0
ok()   { echo "  PASS  $1"; pass=$((pass+1)); }
bad()  { echo "  FAIL  $1"; echo "        observed: $2"; fail=$((fail+1)); }
skipit(){ echo "  SKIP  $1"; echo "        reason: $2"; skip=$((skip+1)); }

echo "== webview_compat ($PLATFORM) against $BASE =="

# ── br-negotiation ────────────────────────────────────────────────────────
hdr=$(curl -sS -o /tmp/wc_wasm.br -D - -H 'Accept-Encoding: br' \
      "$BASE/aleph_panel_bg.wasm" 2>/dev/null)
enc=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-encoding"{print $2}')
size=$(wc -c 2>/dev/null < /tmp/wc_wasm.br | tr -d ' '); size="${size:-0}"
if [ "$enc" = "br" ]; then ok "br-negotiation: content-encoding"
else bad "br-negotiation: content-encoding" "content-encoding='$enc' (expected 'br')"; fi
if [ "$size" -gt 0 ] && [ "$size" -lt 4194304 ]; then ok "br-negotiation: body under 4 MiB"
else bad "br-negotiation: body under 4 MiB" "$size bytes"; fi
if command -v python3 >/dev/null 2>&1 && python3 -c 'import brotli' >/dev/null 2>&1 \
   && [ -f "$DIST/aleph_panel_bg.wasm" ]; then
  same=$(python3 - "$DIST/aleph_panel_bg.wasm" /tmp/wc_wasm.br <<'PY'
import brotli, hashlib, sys
src = open(sys.argv[1],'rb').read()
try:
    got = brotli.decompress(open(sys.argv[2],'rb').read())
except Exception as e:
    print("decompress-failed:%s" % e); raise SystemExit
print("same" if hashlib.sha256(src).digest()==hashlib.sha256(got).digest()
      else "sha-mismatch src=%s got=%s" % (hashlib.sha256(src).hexdigest()[:12],
                                           hashlib.sha256(got).hexdigest()[:12]))
PY
)
  if [ "$same" = "same" ]; then ok "br-negotiation: decompresses to the dist wasm"
  else bad "br-negotiation: decompresses to the dist wasm" "$same"; fi
else
  skipit "br-negotiation: sha comparison" "python3 with the 'brotli' module not available"
fi

# A negotiation is two directions. The block above only proves brotli ARRIVES
# when asked for; these two prove it can be DECLINED — serving br bytes to a
# client that said it cannot decode them yields a blank Panel, and a naive
# `Accept-Encoding.contains("br")` check gets the q=0 case backwards.
hdr=$(curl -sS -o /dev/null -D - -H 'Accept-Encoding: identity' \
      "$BASE/aleph_panel_bg.wasm" 2>/dev/null)
code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
enc=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-encoding"{print $2}')
if [ "$code" = "200" ] && [ "$enc" != "br" ]; then ok "br-negotiation: identity is honoured"
else bad "br-negotiation: identity is honoured" "http=$code content-encoding='$enc' (asked for identity, br must not arrive)"; fi

hdr=$(curl -sS -o /dev/null -D - -H 'Accept-Encoding: br;q=0' \
      "$BASE/aleph_panel_bg.wasm" 2>/dev/null)
code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
enc=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-encoding"{print $2}')
if [ "$code" = "200" ] && [ "$enc" != "br" ]; then ok "br-negotiation: an explicit br;q=0 refusal is honoured"
else bad "br-negotiation: an explicit br;q=0 refusal is honoured" "http=$code content-encoding='$enc' (client named br and refused it with q=0, br must not arrive)"; fi

# ── range-206 / range-416 ─────────────────────────────────────────────────
# ARTIFACT_URL must be a capability URL for a >=200-byte artifact. Mint one from
# a Panel session (open any artifact and copy its URL) and export it.
if [ -n "${ARTIFACT_URL:-}" ]; then
  hdr=$(curl -sS -o /tmp/wc_slice -D - -H 'Range: bytes=100-199' "$ARTIFACT_URL" 2>/dev/null)
  code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
  cr=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-range"{print $2}')
  n=$(wc -c 2>/dev/null < /tmp/wc_slice | tr -d ' '); n="${n:-0}"
  [ "$code" = "206" ] && ok "range-206: status" || bad "range-206: status" "HTTP $code"
  [ "$n" = "100" ]    && ok "range-206: exactly 100 bytes" || bad "range-206: exactly 100 bytes" "$n bytes"
  case "$cr" in bytes\ 100-199/*) ok "range-206: content-range" ;;
                *) bad "range-206: content-range" "'$cr'" ;; esac

  hdr=$(curl -sS -o /dev/null -D - -H 'Range: bytes=999999999-' "$ARTIFACT_URL" 2>/dev/null)
  code=$(printf '%s' "$hdr" | head -1 | awk '{print $2}')
  cr=$(printf '%s' "$hdr" | tr -d '\r' | awk -F': ' 'tolower($1)=="content-range"{print $2}')
  [ "$code" = "416" ] && ok "range-416: status" || bad "range-416: status" "HTTP $code"
  case "$cr" in bytes\ \*/*) ok "range-416: content-range" ;;
                *) bad "range-416: content-range" "'$cr'" ;; esac
else
  skipit "range-206 / range-416" "set ARTIFACT_URL to a capability URL for an artifact of >=200 bytes"
fi

if [ "$PLATFORM" = "linux" ]; then
  # ── gst-codecs ──────────────────────────────────────────────────────────
  if command -v gst-inspect-1.0 >/dev/null 2>&1; then
    miss=""
    for e in mpg123audiodec avdec_mp3; do gst-inspect-1.0 --exists "$e" >/dev/null 2>&1 && { miss=""; break; } || miss="MP3"; done
    if [ -z "$miss" ]; then ok "gst-codecs: MP3 decoder present"
    else bad "gst-codecs: MP3 decoder present" "neither mpg123audiodec nor avdec_mp3 exists — install gstreamer1.0-plugins-ugly"; fi
    echo "        (this is a SPOT CHECK on MP3 only — \`aleph doctor\`'s media/codecs check is the full answer, it also covers AAC, Opus, and VP8/VP9; a green line here does not mean those agree too)"
  else
    skipit "gst-codecs" "gst-inspect-1.0 absent (gstreamer1.0-tools) — the doctor check must report UNKNOWN, not missing; verify that"
  fi

  # ── flat-on-linux ───────────────────────────────────────────────────────
  echo "  MANUAL  flat-on-linux: open the Panel in the shell, then in the WebKit inspector run:"
  echo "          document.documentElement.dataset.flat"
  echo "          getComputedStyle(document.querySelector('.glass')).backdropFilter"
  echo "          expected: \"1\"  and  \"none\""

  # ── tts-playback (BOTH directions) ──────────────────────────────────────
  echo "  MANUAL  tts-playback: trigger a spoken reply, then assert ONE of:"
  echo "          success -> audio plays AND no warning bar under the composer"
  echo "          failure -> a warning bar appears naming the GStreamer plugins"
  echo "          A silent failure with NO bar is the defect this change exists to remove."
fi

if [ "$PLATFORM" = "macos" ]; then
  # ── min-system-version ──────────────────────────────────────────────────
  APP="${ALEPH_APP:-/Applications/Aleph.app}"
  if [ -f "$APP/Contents/Info.plist" ]; then
    v=$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$APP/Contents/Info.plist" 2>/dev/null)
    [ "$v" = "13.3" ] && ok "min-system-version" || bad "min-system-version" "LSMinimumSystemVersion='$v' (expected '13.3')"
  else
    skipit "min-system-version" "no app bundle at $APP — set ALEPH_APP"
  fi
  skipit "install-refusal below 13.3" "requires a machine running macOS < 13.3; NOT VERIFIED anywhere"

  echo "  MANUAL  wkwebview-baseline: in the Panel's inspector, all four must be true:"
  echo "          CSS.supports('color','oklch(0 0 0)')"
  echo "          CSS.supports('color','color-mix(in oklab, red, red)')"
  echo "          typeof CSS.registerProperty === 'function'"
  echo "          typeof WebAssembly === 'object'"
  echo "  MANUAL  tts-blob: trigger a spoken reply; it must play (blob object URL, not data:)"
  echo "  MANUAL  vibrancy: the window is still translucent and the material is visible"
fi

echo
echo "== $pass passed, $fail failed, $skip skipped =="
[ "$fail" -eq 0 ]
