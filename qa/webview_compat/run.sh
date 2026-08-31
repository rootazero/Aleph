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
# `flat-on-linux` manual step below: the shell's platform marker
# (`SHELL_MARKER_JS` in desktop/shell/src/main.rs) is three
# `#[cfg(target_os = ...)]` arms, and an arm is only as verified as the machine
# it was run on. The `macos` arm is now measured — `marker-origin` below drives
# it against a foreign origin and asserts the attribute it writes. The
# `not(any(macos, windows))` (linux) arm is still unverified in the strongest
# sense: never built, never run, never falsified. If `data-platform` is missing
# or wrong on Linux, that arm has never been proven to work at all — don't
# assume your install is broken before assuming the arm itself might be.
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

  # ── marker-origin ───────────────────────────────────────────────────────
  # Does `SHELL_MARKER_JS` reach a document served from an origin the shell did
  # NOT serve — and does it get there BEFORE the page's own first script?
  #
  # This block exists because that answer was once asserted in a comment and
  # never measured. Commit 4c31bfea4 fixed a real bug (the remote panel-only
  # shell's frameless window would not drag) with two changes at once: the
  # capability grant that actually fixed it, and an `on_page_load` re-eval
  # justified by "the init script does not run on a foreign origin". Only the
  # first was load-bearing; the second's justification was never tested, and it
  # propagated into four comments and cost a later round a wrong conclusion
  # about a startup layout flash. So measure it rather than argue it.
  #
  # Two absences are SKIP, never PASS — "I could not ask" is not "the answer is
  # yes": no LAN address (loopback is not a foreign origin, so the question
  # cannot be posed at all) and no panel-only shell binary.
  LANIP=$(ipconfig getifaddr en0 2>/dev/null || ipconfig getifaddr en1 2>/dev/null || true)
  SHELLBIN="${ALEPH_SHELL_BIN:-target/debug/aleph-desktop-shell}"
  if [ -z "$LANIP" ]; then
    skipit "marker-origin" "no LAN address on en0/en1 — a loopback origin is not foreign to the webview, so this cannot be asked here"
  elif [ ! -x "$SHELLBIN" ]; then
    skipit "marker-origin" "no shell binary at $SHELLBIN — build the panel-only variant: (cd desktop/shell && cargo build --no-default-features)"
  elif ! command -v strings >/dev/null 2>&1; then
    skipit "marker-origin" "no \`strings\` on PATH — cannot tell the panel-only binary from the full app, and guessing the wrong one turns a real answer into a misleading FAIL"
  elif [ "$(strings "$SHELLBIN" 2>/dev/null | grep -c 'desktop-shell-panel')" -eq 0 ]; then
    skipit "marker-origin" "$SHELLBIN is the FULL app (it would supervise a bundled daemon and never navigate to the fake Gateway); rebuild with --no-default-features"
  else
    MO_DIR=$(mktemp -d)
    mkdir -p "$MO_DIR/home/.aleph"
    python3 qa/webview_compat/foreign_origin_gateway.py > "$MO_DIR/reports.log" 2>&1 &
    MO_SRV=$!
    # Take the port from the fixture's own post-bind announcement rather than
    # picking one here. A port this script chose can already be held by a stray
    # server from an earlier run — which answers /ready, so the readiness check
    # goes green while the shell talks to that other process and this run's
    # report log stays empty.
    MO_PORT=""
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      MO_PORT=$(sed -n 's/^listening on 0\.0\.0\.0:\([0-9][0-9]*\)$/\1/p' "$MO_DIR/reports.log" 2>/dev/null | head -1)
      [ -n "$MO_PORT" ] && break
      sleep 0.5
    done
    printf 'http://%s:%s' "$LANIP" "$MO_PORT" > "$MO_DIR/home/.aleph/.desktop-shell-panel-target"
    # The shell only navigates to a target whose /ready answers; prove the
    # fixture is up before blaming the shell for not arriving.
    if [ -z "$MO_PORT" ]; then
      bad "marker-origin: the fixture Gateway bound a port" \
          "$(tail -3 "$MO_DIR/reports.log" 2>/dev/null | tr '\n' ' ')"
    elif ! curl -sS -o /dev/null -m 3 "http://$LANIP:$MO_PORT/ready" 2>/dev/null; then
      skipit "marker-origin" "the fixture Gateway bound $MO_PORT but did not answer on http://$LANIP:$MO_PORT — the firewall refused the LAN bind"
    else
      HOME="$MO_DIR/home" "$SHELLBIN" > "$MO_DIR/shell.log" 2>&1 &
      MO_SHELL=$!
      sleep 20
      kill "$MO_SHELL" 2>/dev/null; sleep 1; kill -9 "$MO_SHELL" 2>/dev/null
      first=$(grep -m1 'phase=1-inline-head' "$MO_DIR/reports.log" 2>/dev/null)
      echo "        observed phases:"
      grep '^REPORT' "$MO_DIR/reports.log" 2>/dev/null | sed 's/^/          /' || true
      if [ -z "$first" ]; then
        bad "marker-origin: the shell loaded the foreign-origin page" \
            "no phase-1 report arrived; shell log tail: $(tail -3 "$MO_DIR/shell.log" 2>/dev/null | tr '\n' ' ')"
      else
        ok "marker-origin: the shell loaded the foreign-origin page"
        case "$first" in
          *shell=aleph-tauri*) ok "marker-origin: data-shell is set before the page's first inline script" ;;
          *) bad "marker-origin: data-shell is set before the page's first inline script" \
                 "$first — a stylesheet keyed on [data-shell=\"aleph-tauri\"] would flash unstyled here" ;;
        esac
        case "$first" in
          *platform=macos*) ok "marker-origin: data-platform is set there too" ;;
          *) bad "marker-origin: data-platform is set there too" "$first" ;;
        esac
      fi
    fi
    kill "$MO_SRV" 2>/dev/null
    [ -n "${KEEP:-}" ] && echo "        kept: $MO_DIR" || rm -rf "$MO_DIR"
  fi


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
