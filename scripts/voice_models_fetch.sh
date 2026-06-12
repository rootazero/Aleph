#!/usr/bin/env bash
# Fetch SenseVoice + Kokoro + MeloTTS model packages for the aleph-voice spike.
# Usage: ./scripts/voice_models_fetch.sh [github|hf-mirror]
set -euo pipefail

DEST="${ALEPH_HOME:-$HOME/.aleph}/models/voice"
mkdir -p "$DEST"
SOURCE="${1:-github}"

SENSE_FILE="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2"
KOKORO_FILE="kokoro-multi-lang-v1_1.tar.bz2"
MELO_FILE="vits-melo-tts-zh_en.tar.bz2"

case "$SOURCE" in
  github)
    SENSE_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/$SENSE_FILE"
    KOKORO_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$KOKORO_FILE"
    MELO_URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$MELO_FILE"
    ;;
  hf-mirror)
    SENSE_URL="https://hf-mirror.com/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/main/$SENSE_FILE"
    KOKORO_URL="https://hf-mirror.com/csukuangfj/kokoro-multi-lang-v1_1/resolve/main/$KOKORO_FILE"
    MELO_URL="https://hf-mirror.com/csukuangfj/vits-melo-tts-zh_en/resolve/main/$MELO_FILE"
    ;;
  *) echo "unknown source: $SOURCE"; exit 1 ;;
esac

for pair in "sense-voice-small|$SENSE_URL|$SENSE_FILE" "kokoro-v1.1-zh|$KOKORO_URL|$KOKORO_FILE" "vits-melo-tts-zh_en|$MELO_URL|$MELO_FILE"; do
  IFS='|' read -r id url file <<< "$pair"
  echo "==> $id"
  curl -L -C - -o "$DEST/$file" "$url"
  echo "sha256:"
  shasum -a 256 "$DEST/$file"
  mkdir -p "$DEST/$id"
  tar -xjf "$DEST/$file" -C "$DEST/$id" --strip-components=1
  ls "$DEST/$id" | head -20
done
echo "Done. Models under $DEST"
